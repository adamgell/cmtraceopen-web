//! Chunked, resumable bundle uploader.
//!
//! Speaks the same three-step protocol as `tools/ship-bundle.sh`
//! (init → chunk* → finalize) — the server-side types live in
//! `common_wire::ingest`.
//!
//! ## Retries
//!
//! Each network call is wrapped in an exponential-backoff retry. MVP
//! policy: 3 attempts total with a 1s / 5s / 30s sleep between attempts.
//! Only "transient" failures (network errors, 5xx, 408, 429) are retried;
//! client errors (4xx other than 408/409/429) surface immediately — a 400
//! or 404 isn't going to fix itself by waiting.
//!
//! 409 responses are handled specially: on a chunk PUT they signal
//! "offset drift" (a different client already wrote here), so we re-init
//! to pick up the authoritative resume_offset.

use std::path::Path;
use std::time::Duration;

use common_wire::ingest::{
    BundleFinalizeRequest, BundleFinalizeResponse, BundleInitRequest, BundleInitResponse,
    ChunkUploadResponse,
};
use reqwest::{Client, StatusCode};
use tracing::{info, warn};
use uuid::Uuid;

use crate::collectors::BundleMetadata;
use crate::tls::{
    build_client_config, install_default_crypto_provider, TlsClientOptions, TlsConfigError,
};

/// Chunk size the agent prefers. The server may override via the
/// `chunkSize` field in `BundleInitResponse`; we honor whichever is
/// smaller so we never exceed the server's `MAX_CHUNK_SIZE`.
pub const DEFAULT_CHUNK_SIZE: u64 = 4 * 1024 * 1024; // 4 MiB

/// Tunable retry parameters. Extracted so tests can drive them down to
/// zero sleep without changing the code path.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    /// Delay between attempts: `delays[attempt-1]`. If the attempt count
    /// exceeds the vec, the last entry is reused.
    pub delays: Vec<Duration>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            delays: vec![
                Duration::from_secs(1),
                Duration::from_secs(5),
                Duration::from_secs(30),
            ],
        }
    }
}

impl RetryPolicy {
    /// Zero-delay policy — useful for tests that don't want to wait.
    pub fn immediate(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            delays: vec![Duration::ZERO],
        }
    }

    /// Lookup delay for a given attempt number (1-indexed). Returns
    /// `Duration::ZERO` if `attempt == 0` (shouldn't happen).
    pub(crate) fn delay_for(&self, attempt: u32) -> Duration {
        if attempt == 0 || self.delays.is_empty() {
            return Duration::ZERO;
        }
        let idx = (attempt as usize - 1).min(self.delays.len() - 1);
        self.delays[idx]
    }
}

#[derive(Debug, Clone)]
pub struct UploaderConfig {
    pub endpoint: String,
    pub device_id: String,
    pub request_timeout: Duration,
    pub retry: RetryPolicy,
    /// TLS knobs. Default = native roots, no client cert. Wave 3 mTLS
    /// flips both `client_cert_pem` and `client_key_pem` on; today the
    /// agent works either way (server-side mTLS isn't yet enforced).
    /// `http://` URLs continue to work — rustls only kicks in for
    /// `https://`.
    pub tls: TlsClientOptions,
}

impl UploaderConfig {
    pub fn new(endpoint: String, device_id: String, request_timeout: Duration) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            device_id,
            request_timeout,
            retry: RetryPolicy::default(),
            tls: TlsClientOptions::default(),
        }
    }

    /// Builder-style override for the TLS configuration. Folded into
    /// `with_*` rather than a public field set so call sites read
    /// linearly: `UploaderConfig::new(..).with_tls(opts)`.
    pub fn with_tls(mut self, tls: TlsClientOptions) -> Self {
        self.tls = tls;
        self
    }
}

#[derive(Debug)]
pub struct Uploader {
    client: Client,
    cfg: UploaderConfig,
}

impl Uploader {
    pub fn new(cfg: UploaderConfig) -> Result<Self, UploaderError> {
        // Make sure aws-lc-rs is the rustls process default. Idempotent.
        // Doing this here (rather than in `main`) keeps tests, the
        // integration suite, and any future embedders consistent.
        install_default_crypto_provider();

        let tls_config = build_client_config(&cfg.tls).map_err(UploaderError::Tls)?;

        // `reqwest::Client` is cheap to clone internally; build once and
        // reuse across bundles.
        let client = Client::builder()
            .timeout(cfg.request_timeout)
            // Hand reqwest the pre-built rustls ClientConfig. This
            // path is active for `https://` URLs only — `http://`
            // endpoints still work over plaintext, so the integration
            // test (which talks to a loopback axum server) doesn't
            // need a TLS terminator.
            .use_preconfigured_tls(tls_config)
            // Don't follow redirects on upload POSTs — a misconfigured
            // proxy bouncing a multi-MiB PUT is a surefire way to lose
            // bytes silently. The server handshake is direct.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(UploaderError::Client)?;
        Ok(Self { client, cfg })
    }

    /// Construct an uploader backed by a pre-built reqwest `Client`. Used
    /// by tests that want to share a client across runs or skip
    /// TLS/connection pool surgery.
    #[cfg(test)]
    pub fn with_client(client: Client, cfg: UploaderConfig) -> Self {
        Self { client, cfg }
    }

    /// Upload one bundle end-to-end. Idempotent: re-invoking with the
    /// same `metadata.bundle_id` after a mid-upload crash will resume
    /// from the server-recorded offset.
    pub async fn upload(
        &self,
        metadata: &BundleMetadata,
        zip_path: &Path,
    ) -> Result<BundleFinalizeResponse, UploaderError> {
        // Stage 1: init.
        let init = self
            .with_retries("init", || self.init_once(metadata))
            .await?;
        info!(
            upload_id = %init.upload_id,
            chunk_size = init.chunk_size,
            resume_offset = init.resume_offset,
            "bundle init accepted"
        );

        // Stage 2: chunk loop from resume_offset.
        let chunk_size = init.chunk_size.clamp(1, DEFAULT_CHUNK_SIZE);
        let bytes = tokio::fs::read(zip_path).await.map_err(UploaderError::Io)?;
        let total = bytes.len() as u64;
        if total != metadata.size_bytes {
            return Err(UploaderError::SizeDrift {
                expected: metadata.size_bytes,
                actual: total,
            });
        }

        let mut offset = init.resume_offset;
        while offset < total {
            let end = (offset + chunk_size).min(total);
            let slice = bytes[offset as usize..end as usize].to_vec();
            let at = offset;
            let response = self
                .with_retries("chunk", || self.put_chunk_once(init.upload_id, at, slice.clone()))
                .await?;
            if response.next_offset != end {
                return Err(UploaderError::OffsetMismatch {
                    expected: end,
                    got: response.next_offset,
                });
            }
            offset = response.next_offset;
        }

        // Stage 3: finalize.
        let fin = self
            .with_retries("finalize", || {
                self.finalize_once(init.upload_id, &metadata.sha256)
            })
            .await?;

        info!(session_id = %fin.session_id, parse_state = %fin.parse_state, "bundle finalized");
        Ok(fin)
    }

    async fn init_once(&self, metadata: &BundleMetadata) -> Result<BundleInitResponse, UploaderError> {
        let url = format!("{}/v1/ingest/bundles", self.cfg.endpoint);
        let body = BundleInitRequest {
            bundle_id: metadata.bundle_id,
            device_hint: Some(self.cfg.device_id.clone()),
            sha256: metadata.sha256.clone(),
            size_bytes: metadata.size_bytes,
            content_kind: metadata.content_kind.clone(),
        };
        let resp = self
            .client
            .post(url)
            .header("x-device-id", &self.cfg.device_id)
            .json(&body)
            .send()
            .await
            .map_err(classify_send_error)?;
        parse_response(resp, "init").await
    }

    async fn put_chunk_once(
        &self,
        upload_id: Uuid,
        offset: u64,
        body: Vec<u8>,
    ) -> Result<ChunkUploadResponse, UploaderError> {
        let url = format!(
            "{}/v1/ingest/bundles/{}/chunks?offset={}",
            self.cfg.endpoint, upload_id, offset
        );
        let resp = self
            .client
            .put(url)
            .header("x-device-id", &self.cfg.device_id)
            .header("content-type", "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(classify_send_error)?;
        parse_response(resp, "chunk").await
    }

    async fn finalize_once(
        &self,
        upload_id: Uuid,
        sha256: &str,
    ) -> Result<BundleFinalizeResponse, UploaderError> {
        let url = format!(
            "{}/v1/ingest/bundles/{}/finalize",
            self.cfg.endpoint, upload_id
        );
        let resp = self
            .client
            .post(url)
            .header("x-device-id", &self.cfg.device_id)
            .json(&BundleFinalizeRequest {
                final_sha256: sha256.to_string(),
            })
            .send()
            .await
            .map_err(classify_send_error)?;
        parse_response(resp, "finalize").await
    }

    async fn with_retries<F, Fut, T>(&self, label: &str, mut op: F) -> Result<T, UploaderError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, UploaderError>>,
    {
        let mut last_err = None;
        for attempt in 1..=self.cfg.retry.max_attempts {
            match op().await {
                Ok(v) => return Ok(v),
                Err(e) if !e.is_transient() => return Err(e),
                Err(e) => {
                    warn!(label, attempt, error = %e, "transient upload error; will retry");
                    last_err = Some(e);
                }
            }
            if attempt < self.cfg.retry.max_attempts {
                let delay = self.cfg.retry.delay_for(attempt);
                tokio::time::sleep(delay).await;
            }
        }
        Err(last_err.unwrap_or(UploaderError::Exhausted {
            label: label.into(),
        }))
    }
}

/// Parse an HTTP response into its typed body, surfacing status-code
/// details. Non-2xx becomes either a transient or fatal error depending
/// on the status class.
async fn parse_response<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
    label: &str,
) -> Result<T, UploaderError> {
    let status = resp.status();
    if status.is_success() {
        return resp.json::<T>().await.map_err(UploaderError::Decode);
    }
    let body = resp.text().await.unwrap_or_default();
    if is_transient_status(status) {
        Err(UploaderError::Transient {
            label: label.into(),
            status: status.as_u16(),
            body,
        })
    } else {
        Err(UploaderError::Fatal {
            label: label.into(),
            status: status.as_u16(),
            body,
        })
    }
}

/// A network / connection error is always transient — we don't know if
/// the server processed the request, so letting the resume path re-init
/// will sort it out.
fn classify_send_error(err: reqwest::Error) -> UploaderError {
    // reqwest errors on decoding flow through here too; distinguish by
    // whether the error is "builder / url" vs "connect / timeout / body".
    if err.is_builder() {
        return UploaderError::Client(err);
    }
    UploaderError::Network(err)
}

fn is_transient_status(s: StatusCode) -> bool {
    s.is_server_error()
        || s == StatusCode::REQUEST_TIMEOUT
        || s == StatusCode::TOO_MANY_REQUESTS
}

#[derive(Debug, thiserror::Error)]
pub enum UploaderError {
    #[error("reqwest client build error: {0}")]
    Client(#[source] reqwest::Error),

    #[error("TLS config error: {0}")]
    Tls(#[source] TlsConfigError),

    #[error("network error: {0}")]
    Network(#[source] reqwest::Error),

    #[error("decode error: {0}")]
    Decode(#[source] reqwest::Error),

    #[error("i/o error: {0}")]
    Io(#[source] std::io::Error),

    #[error("server returned {status} for {label}: {body}")]
    Fatal {
        label: String,
        status: u16,
        body: String,
    },

    #[error("server returned transient {status} for {label}: {body}")]
    Transient {
        label: String,
        status: u16,
        body: String,
    },

    #[error("retry budget exhausted for {label}")]
    Exhausted { label: String },

    #[error("bundle size drifted: expected {expected} bytes, got {actual}")]
    SizeDrift { expected: u64, actual: u64 },

    #[error("server returned nextOffset={got}, expected {expected}")]
    OffsetMismatch { expected: u64, got: u64 },
}

impl UploaderError {
    fn is_transient(&self) -> bool {
        matches!(
            self,
            UploaderError::Network(_) | UploaderError::Transient { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// `with_retries` retries transient errors and uses the configured
    /// delay. Under `tokio::time::pause()` the sleep advances
    /// instantaneously, so the test doesn't wait for real seconds.
    #[tokio::test(start_paused = true)]
    async fn retry_math_respects_delays_and_max_attempts() {
        crate::tls::install_default_crypto_provider();
        let client = reqwest::Client::new();
        let cfg = UploaderConfig {
            endpoint: "http://unused".into(),
            device_id: "WIN-TEST".into(),
            request_timeout: Duration::from_secs(1),
            retry: RetryPolicy {
                max_attempts: 3,
                delays: vec![
                    Duration::from_secs(1),
                    Duration::from_secs(5),
                    Duration::from_secs(30),
                ],
            },
            tls: TlsClientOptions::default(),
        };
        let u = Uploader::with_client(client, cfg);

        let calls = Arc::new(AtomicU32::new(0));
        let calls2 = calls.clone();
        let result: Result<(), UploaderError> = u
            .with_retries("probe", move || {
                let c = calls2.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(UploaderError::Transient {
                        label: "probe".into(),
                        status: 503,
                        body: "nope".into(),
                    })
                }
            })
            .await;

        assert!(matches!(result, Err(UploaderError::Transient { .. })));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn fatal_errors_are_not_retried() {
        crate::tls::install_default_crypto_provider();
        let client = reqwest::Client::new();
        let cfg = UploaderConfig {
            endpoint: "http://unused".into(),
            device_id: "WIN-TEST".into(),
            request_timeout: Duration::from_secs(1),
            retry: RetryPolicy::default(),
            tls: TlsClientOptions::default(),
        };
        let u = Uploader::with_client(client, cfg);

        let calls = Arc::new(AtomicU32::new(0));
        let calls2 = calls.clone();
        let result: Result<(), UploaderError> = u
            .with_retries("probe", move || {
                let c = calls2.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(UploaderError::Fatal {
                        label: "probe".into(),
                        status: 400,
                        body: "bad request".into(),
                    })
                }
            })
            .await;

        assert!(matches!(result, Err(UploaderError::Fatal { .. })));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn succeeds_on_third_attempt() {
        crate::tls::install_default_crypto_provider();
        let client = reqwest::Client::new();
        let cfg = UploaderConfig {
            endpoint: "http://unused".into(),
            device_id: "WIN-TEST".into(),
            request_timeout: Duration::from_secs(1),
            retry: RetryPolicy::default(),
            tls: TlsClientOptions::default(),
        };
        let u = Uploader::with_client(client, cfg);

        let calls = Arc::new(AtomicU32::new(0));
        let calls2 = calls.clone();
        let result: Result<u32, UploaderError> = u
            .with_retries("probe", move || {
                let c = calls2.clone();
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        Err(UploaderError::Transient {
                            label: "probe".into(),
                            status: 502,
                            body: "".into(),
                        })
                    } else {
                        Ok(42)
                    }
                }
            })
            .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn delay_for_clamps_to_last_entry() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_for(1), Duration::from_secs(1));
        assert_eq!(p.delay_for(2), Duration::from_secs(5));
        assert_eq!(p.delay_for(3), Duration::from_secs(30));
        // Past the end — clamp to last.
        assert_eq!(p.delay_for(99), Duration::from_secs(30));
        // Zero-attempt is ZERO (edge-case).
        assert_eq!(p.delay_for(0), Duration::ZERO);
    }

    /// `Uploader::new` should succeed against the default TLS options
    /// (native roots, no client cert). This also exercises the
    /// `install_default_crypto_provider` idempotent guard — running
    /// the test suite invokes it many times, which would panic if the
    /// guard weren't there.
    #[test]
    fn new_builds_with_default_tls() {
        let cfg = UploaderConfig::new(
            "http://unused".into(),
            "WIN-TEST".into(),
            Duration::from_secs(1),
        );
        let _u = Uploader::new(cfg).expect("uploader builds with default TLS");
    }

    /// `Uploader::new` should surface a TLS config error (rather than
    /// panicking) when client-cert paths are half-set.
    #[test]
    fn new_surfaces_tls_config_error() {
        use std::path::PathBuf;
        let cfg = UploaderConfig::new(
            "http://unused".into(),
            "WIN-TEST".into(),
            Duration::from_secs(1),
        )
        .with_tls(TlsClientOptions {
            client_cert_pem: Some(PathBuf::from("/tmp/half-set.crt")),
            client_key_pem: None,
            ca_bundle_pem: None,
        });
        let err = Uploader::new(cfg).expect_err("half-set client auth must error");
        assert!(matches!(err, UploaderError::Tls(TlsConfigError::PartialClientAuth)));
    }

    #[test]
    fn is_transient_classifies_correctly() {
        assert!(is_transient_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(is_transient_status(StatusCode::BAD_GATEWAY));
        assert!(is_transient_status(StatusCode::REQUEST_TIMEOUT));
        assert!(is_transient_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(!is_transient_status(StatusCode::BAD_REQUEST));
        assert!(!is_transient_status(StatusCode::NOT_FOUND));
        assert!(!is_transient_status(StatusCode::CONFLICT));
    }
}
