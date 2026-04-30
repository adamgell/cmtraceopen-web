//! HTTP client against an external api-server URL. Used directly by the
//! `--target=remote-http` mode and as the inner client of `local-http`.
//!
//! Wire protocol (from `crates/api-server/src/routes/ingest.rs`):
//!
//!   POST /v1/ingest/bundles
//!        body: {bundleId, sha256, sizeBytes, contentKind, deviceHint?}
//!        → 201 {uploadId, chunkSize, resumeOffset}  (camelCase)
//!
//!   PUT  /v1/ingest/bundles/{upload_id}/chunks?offset=N  (byte-offset cursor)
//!        body: raw bytes
//!        → 200 {nextOffset}
//!
//!   POST /v1/ingest/bundles/{upload_id}/finalize
//!        body: {finalSha256}  (camelCase)
//!        → 201 {sessionId, parseState}
//!
//!   GET  /v1/sessions/{session_id}
//!        → {sessionId, parseState, ...}

use crate::auth::Auth;
use crate::bundle::{Bundle, BundleResult};
use crate::target::Target;
use reqwest::Client;
use serde_json::json;
use sha2::Digest;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct RemoteHttpTarget {
    base_url: String,
    client: Client,
    auth: Arc<dyn Auth>,
    poll_interval: Duration,
    poll_timeout: Duration,
}

impl RemoteHttpTarget {
    pub fn new(base_url: String, client: Client, auth: Arc<dyn Auth>) -> Self {
        Self {
            base_url,
            client,
            auth,
            poll_interval: Duration::from_millis(250),
            poll_timeout: Duration::from_secs(120),
        }
    }

    fn url(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{base}{path}")
    }
}

/// Chunk size for PUT uploads. The server enforces MAX_CHUNK_SIZE; we use 4 MiB
/// which is well under the 8 MiB server default, keeping individual requests
/// small enough for proxies and load-balancers.
const CHUNK_SIZE: usize = 4 * 1024 * 1024;

#[async_trait::async_trait]
impl Target for RemoteHttpTarget {
    async fn send(&self, bundle: Bundle) -> BundleResult {
        let started = Instant::now();
        let device_id = bundle.device_id.clone();
        let bundle_bytes = bundle.zip_bytes.len() as u64;
        let n_files = bundle.n_files;
        let seq = bundle.seq;

        let result = async {
            // Compute sha256 of the full bundle upfront — the server needs it
            // at init time (not just finalize) to support resume detection.
            let sha_hex = {
                let mut hasher = sha2::Sha256::new();
                hasher.update(&bundle.zip_bytes);
                format!("{:x}", hasher.finalize())
            };

            // 1. Init upload. Body is camelCase (all DTOs use `rename_all = "camelCase"`).
            let resp = self
                .auth
                .apply(
                    self.client.post(self.url("/v1/ingest/bundles")),
                    &device_id,
                )
                .json(&json!({
                    "bundleId":    bundle.bundle_id,
                    "sha256":      sha_hex,
                    "sizeBytes":   bundle_bytes,
                    "contentKind": "evidence-zip",
                }))
                .send()
                .await?;
            let status = resp.status().as_u16();
            anyhow::ensure!(resp.status().is_success(), "init upload status {status}");
            let body: serde_json::Value = resp.json().await?;
            // Response field is camelCase: `uploadId`.
            let upload_id = body["uploadId"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing uploadId in init response"))?
                .to_string();

            // 2. Chunked PUT upload (offset-based, not seq-based).
            let mut offset: u64 = 0;
            for chunk in bundle.zip_bytes.chunks(CHUNK_SIZE) {
                let resp = self
                    .auth
                    .apply(
                        self.client.put(self.url(&format!(
                            "/v1/ingest/bundles/{upload_id}/chunks?offset={offset}"
                        ))),
                        &device_id,
                    )
                    .body(chunk.to_vec())
                    .send()
                    .await?;
                anyhow::ensure!(
                    resp.status().is_success(),
                    "chunk at offset {offset} status {}",
                    resp.status()
                );
                offset += chunk.len() as u64;
            }

            // 3. Finalize. Only `finalSha256` is required — content_kind and
            // size_bytes were already given at init.
            let resp = self
                .auth
                .apply(
                    self.client.post(self.url(&format!(
                        "/v1/ingest/bundles/{upload_id}/finalize"
                    ))),
                    &device_id,
                )
                .json(&json!({ "finalSha256": sha_hex }))
                .send()
                .await?;
            let finalize_status = resp.status().as_u16();
            anyhow::ensure!(
                resp.status().is_success(),
                "finalize status {finalize_status}"
            );
            let body: serde_json::Value = resp.json().await?;
            // Response field is camelCase: `sessionId`.
            let session_id = body["sessionId"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing sessionId in finalize response"))?
                .to_string();
            let finalize_ms = started.elapsed().as_millis() as u64;
            let parse_started = Instant::now();

            // 4. Poll GET /v1/sessions/{session_id} until parse_state != "pending".
            //    SessionSummary has no files_with_fallbacks field, so we always
            //    set that to None.
            let parse_state = loop {
                if parse_started.elapsed() > self.poll_timeout {
                    anyhow::bail!("parse timed out after {:?}", self.poll_timeout);
                }
                let resp = self
                    .auth
                    .apply(
                        self.client
                            .get(self.url(&format!("/v1/sessions/{session_id}"))),
                        &device_id,
                    )
                    .send()
                    .await?;
                anyhow::ensure!(
                    resp.status().is_success(),
                    "poll status {}",
                    resp.status()
                );
                let body: serde_json::Value = resp.json().await?;
                // Response field is camelCase: `parseState`.
                let state = body["parseState"]
                    .as_str()
                    .unwrap_or("pending")
                    .to_string();
                if state != "pending" {
                    break state;
                }
                tokio::time::sleep(self.poll_interval).await;
            };
            let parse_ms = parse_started.elapsed().as_millis() as u64;

            anyhow::Ok((finalize_status, finalize_ms, parse_ms, parse_state))
        }
        .await;

        match result {
            Ok((http_status, finalize_ms, parse_ms, parse_state)) => BundleResult {
                seq,
                device: device_id,
                n_files,
                bundle_bytes,
                finalize_ms: Some(finalize_ms),
                parse_ms,
                parse_state,
                files_with_fallbacks: None, // SessionSummary has no such field
                http_status: Some(http_status),
                error: None,
            },
            Err(e) => BundleResult {
                seq,
                device: device_id,
                n_files,
                bundle_bytes,
                finalize_ms: None,
                parse_ms: started.elapsed().as_millis() as u64,
                parse_state: "harness-error".into(),
                files_with_fallbacks: None,
                http_status: None,
                error: Some(format!("{e:#}")),
            },
        }
    }

    async fn sample_system(&self) -> serde_json::Value {
        let resp = self.client.get(self.url("/metrics")).send().await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let text = r.text().await.unwrap_or_default();
                json!({"metrics_raw_bytes": text.len(), "scrape_ok": true})
            }
            _ => json!({"scrape_ok": false}),
        }
    }

    async fn shutdown(self: Box<Self>) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_join_strips_trailing_slash() {
        let t = RemoteHttpTarget {
            base_url: "https://x.example/".into(),
            client: Client::new(),
            auth: Arc::new(crate::auth::header::HeaderAuth),
            poll_interval: Duration::from_millis(1),
            poll_timeout: Duration::from_millis(1),
        };
        assert_eq!(t.url("/v1/ingest/bundles"), "https://x.example/v1/ingest/bundles");
    }
}
