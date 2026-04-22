//! Live round-trip test against a real Azure Blob Storage account (or the
//! Azurite emulator) — gated on env vars so default `cargo test` runs are a
//! no-op.
//!
//! ## How it runs
//!
//! Skipped silently unless `CMTRACE_AZURE_STORAGE_ACCOUNT` is set in the
//! test environment. CI and developer machines without Azure creds will
//! see this test pass without performing any I/O. To exercise it locally
//! against the Azurite emulator:
//!
//! ```text
//! podman run -p 10000:10000 mcr.microsoft.com/azure-storage/azurite
//! podman run --net=host mcr.microsoft.com/azure-cli az storage container \
//!     create -n cmtrace-test --connection-string \
//!     'DefaultEndpointsProtocol=http;AccountName=devstoreaccount1;\
//!      AccountKey=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6\
//!      IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==;\
//!      BlobEndpoint=http://127.0.0.1:10000/devstoreaccount1;'
//!
//! CMTRACE_AZURE_STORAGE_ACCOUNT=devstoreaccount1 \
//! CMTRACE_AZURE_STORAGE_CONTAINER=cmtrace-test \
//! CMTRACE_AZURE_STORAGE_ACCOUNT_KEY='Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==' \
//! cargo test --features azure --test azure_blob_integration
//! ```
//!
//! Against a real account in Azure, swap `devstoreaccount1` for the real
//! account name and use a managed-identity-enabled host (drop the
//! `_ACCOUNT_KEY` env, set `CMTRACE_AZURE_USE_MANAGED_IDENTITY=true`).
//!
//! ## What it verifies
//!
//! End-to-end via the [`api_server::storage::BlobStore`] trait:
//!   1. `create_staging` + `put_chunk` lay bytes down on the local stager.
//!   2. `hash` returns the expected sha256 of the assembled bytes.
//!   3. `finalize` ships the bundle to Azure as
//!      `blobs/<session_id>` and returns an `azure://...` URI.
//!   4. `read_blob` round-trips the same bytes back from Azure.
//!   5. `head_blob` reports the right size.
//!
//! This is the canonical confidence check that flipping
//! `CMTRACE_BLOB_BACKEND=Azure` in production will work — every method on
//! the trait is exercised against the real backend.

#![cfg(feature = "azure")]

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use api_server::storage::blob_azure::{self, AzureAuth, AzureBlobConfig};
use api_server::storage::BlobStore;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

/// Read all required env vars; return None if any are missing so the test
/// can skip cleanly. The `account_name` + `container` pair are mandatory;
/// auth is whichever of (key, managed-identity) is set.
fn env_config(staging_root: PathBuf) -> Option<AzureBlobConfig> {
    let account_name = env::var("CMTRACE_AZURE_STORAGE_ACCOUNT").ok()?;
    let container_name = env::var("CMTRACE_AZURE_STORAGE_CONTAINER").ok()?;

    let auth = if let Ok(key) = env::var("CMTRACE_AZURE_STORAGE_ACCOUNT_KEY") {
        AzureAuth::AccountKey(key)
    } else if env::var("CMTRACE_AZURE_USE_MANAGED_IDENTITY")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
    {
        AzureAuth::ManagedIdentity
    } else {
        // Account name + container present but no auth — incomplete config.
        // Skip rather than fail; this is the same shape `Config::from_env`
        // would reject at startup.
        eprintln!(
            "azure_blob_integration: account+container set but neither \
             CMTRACE_AZURE_STORAGE_ACCOUNT_KEY nor \
             CMTRACE_AZURE_USE_MANAGED_IDENTITY=true; skipping"
        );
        return None;
    };

    Some(AzureBlobConfig {
        account_name,
        container_name,
        staging_root,
        auth,
    })
}

#[tokio::test]
async fn round_trip_against_real_azure() {
    // Hard skip when no creds are present. This is the default path on CI
    // and on contributor laptops — nothing to assert when there's nothing
    // to talk to.
    let tmp = TempDir::new().expect("tempdir");
    let cfg = match env_config(tmp.path().join("staging")) {
        Some(c) => c,
        None => {
            eprintln!(
                "azure_blob_integration: CMTRACE_AZURE_STORAGE_ACCOUNT not \
                 set; skipping (this is the expected default)"
            );
            return;
        }
    };

    let store: Arc<dyn BlobStore> =
        Arc::new(blob_azure::build(cfg).await.expect("build azure store"));

    let upload_id = Uuid::now_v7();
    let session_id = Uuid::now_v7();
    let payload: Vec<u8> = (0..(64 * 1024)).map(|i| (i % 251) as u8).collect();
    let expected_sha = {
        let mut h = Sha256::new();
        h.update(&payload);
        hex::encode(h.finalize())
    };

    store.create_staging(upload_id).await.expect("create_staging");
    // Split into two chunks so put_chunk's offset path gets exercised.
    let split = payload.len() / 2;
    store
        .put_chunk(upload_id, 0, &payload[..split])
        .await
        .expect("put_chunk #1");
    store
        .put_chunk(upload_id, split as u64, &payload[split..])
        .await
        .expect("put_chunk #2");

    let observed_sha = store.hash(upload_id).await.expect("hash");
    assert_eq!(observed_sha, expected_sha, "staged sha must match input");

    let handle = store
        .finalize(upload_id, session_id)
        .await
        .expect("finalize → azure");
    assert_eq!(handle.size_bytes, payload.len() as u64);
    assert_eq!(handle.sha256, expected_sha);
    assert!(
        handle.uri.starts_with("azure://"),
        "expected azure:// URI, got {}",
        handle.uri
    );

    // Round-trip through the trait.
    let size = store.head_blob(&handle.uri).await.expect("head_blob");
    assert_eq!(size, payload.len() as u64);

    let got = store.read_blob(&handle.uri).await.expect("read_blob");
    assert_eq!(got, payload, "read_blob must round-trip the finalized bytes");
}
