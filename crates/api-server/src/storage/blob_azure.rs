//! Azure Blob Storage [`BlobStore`] factory.
//!
//! Wraps `object_store::azure::MicrosoftAzureBuilder` so the api-server
//! ingest path can write finalized bundles to a real Azure storage account
//! when `CMTRACE_BLOB_BACKEND=Azure` is set in the environment. Returns an
//! [`ObjectStoreBlobStore`] — every method on the [`BlobStore`] trait is
//! implemented there, so this module is intentionally tiny: build the
//! object_store client, hand it to the generic adapter, done.
//!
//! ## Auth
//!
//! Two paths are supported, picked at startup from config:
//!
//!   - **Managed identity** (recommended for prod): set
//!     `CMTRACE_AZURE_USE_MANAGED_IDENTITY=true`. `object_store` will pick
//!     up the IMDS-issued token from the host (App Service, AKS pod with
//!     workload identity, VM with system-assigned identity). No secret
//!     leaves the cluster.
//!
//!   - **Account key** (DEV only): set `CMTRACE_AZURE_STORAGE_ACCOUNT_KEY`.
//!     Useful for spinning up against Azurite locally or smoke-testing a
//!     real account from a developer workstation; never deploy this way.
//!
//! The factory rejects ambiguous combinations (both auth modes set, or
//! neither set) at startup with a clear error so misconfigured deployments
//! fail loud rather than running with surprising auth semantics.
//!
//! ## Staging
//!
//! Same staging contract as the local-FS backend: in-progress chunks are
//! assembled on local disk under `<staging_root>/<upload_id>` so the
//! sequential-offset wire protocol stays simple, then the finalized bundle
//! is shipped to the configured Azure container as
//! `blobs/<session_id>`. See [`crate::storage::blob_object_store`] for the
//! rationale behind keeping staging local-disk.

use std::path::PathBuf;
use std::sync::Arc;

use object_store::azure::MicrosoftAzureBuilder;
use object_store::ObjectStore;

use super::blob_object_store::ObjectStoreBlobStore;
use super::StorageError;

/// Settings for the Azure factory. Populated from `Config` (env vars) in
/// `main.rs` — broken out into its own struct so this module doesn't depend
/// on the full `Config` type and stays easy to unit-test.
#[derive(Debug, Clone)]
pub struct AzureBlobConfig {
    /// Storage account name (the `<account>` in `<account>.blob.core.windows.net`).
    pub account_name: String,
    /// Container the api-server writes blobs into. Must already exist —
    /// the factory does NOT create containers; that's a deploy-time concern
    /// (Bicep / Terraform / `az storage container create`).
    pub container_name: String,
    /// Local directory used to assemble in-progress uploads before they're
    /// shipped to Azure on finalize. Typically `<CMTRACE_DATA_DIR>/staging`.
    pub staging_root: PathBuf,
    /// One of: account key (DEV), or managed identity (prod). See module
    /// docs.
    pub auth: AzureAuth,
}

/// Authentication strategy for the Azure backend.
#[derive(Debug, Clone)]
pub enum AzureAuth {
    /// Storage-account shared key. Easy to misuse — restrict to dev / CI.
    AccountKey(String),
    /// IMDS-issued managed-identity token. Production default.
    ManagedIdentity,
}

/// Build the Azure-backed [`crate::storage::BlobStore`] (returned as the
/// generic [`ObjectStoreBlobStore`] adapter so the rest of the server
/// doesn't have to know about the Azure type).
///
/// Container existence is NOT verified here — `object_store::head` on the
/// first finalize would surface a 404, but most deployments fail earlier at
/// the auth handshake. Operators can validate end-to-end by running the
/// `azure-test` integration test against a real account before flipping
/// production traffic.
pub async fn build(config: AzureBlobConfig) -> Result<ObjectStoreBlobStore, StorageError> {
    let mut builder = MicrosoftAzureBuilder::new()
        .with_account(&config.account_name)
        .with_container_name(&config.container_name);

    builder = match &config.auth {
        AzureAuth::AccountKey(key) => builder.with_access_key(key),
        AzureAuth::ManagedIdentity => {
            let b = builder.with_use_azure_cli(false);
            // ACA / App Service expose IDENTITY_ENDPOINT instead of the
            // VM-style IMDS at 169.254.169.254. Pass it through so
            // object_store hits the right token endpoint.
            if let Ok(endpoint) = std::env::var("IDENTITY_ENDPOINT") {
                b.with_msi_endpoint(endpoint)
            } else {
                b
            }
        }
    };

    let store = builder
        .build()
        .map_err(|e| StorageError::ObjectStore(e.to_string()))?;
    let inner: Arc<dyn ObjectStore> = Arc::new(store);

    // URI scheme for finalized blobs: `azure://<container>/blobs/<session_id>`.
    // The `head_blob` / `read_blob` round-trip in ObjectStoreBlobStore only
    // looks at the `<obj_path>` tail, so this string is purely cosmetic for
    // the status page and tracing — but matching the de-facto Azure URI
    // convention makes operator tooling (`az storage blob show`) easy.
    ObjectStoreBlobStore::new(
        inner,
        config.staging_root,
        "azure",
        config.container_name.clone(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The factory should at least construct an `ObjectStoreBlobStore` when
    /// pointed at a syntactically-valid account + key without performing any
    /// network I/O. Real round-trip is exercised under the
    /// `azure-test`-style integration test in `tests/azure_blob_integration.rs`
    /// only when `CMTRACE_AZURE_STORAGE_ACCOUNT` is set.
    #[tokio::test]
    async fn builder_accepts_account_key_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = AzureBlobConfig {
            account_name: "devstoreaccount1".to_string(),
            container_name: "test-bucket".to_string(),
            staging_root: tmp.path().to_path_buf(),
            auth: AzureAuth::AccountKey(
                // 64 base64 chars from the well-known azurite emulator key.
                "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6\
                 IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=="
                    .to_string(),
            ),
        };
        let store = build(cfg).await;
        assert!(store.is_ok(), "factory should succeed on valid config");
    }
}
