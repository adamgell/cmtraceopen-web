//! Device-identity extraction for ingest routes.
//!
//! Replaces the temporary `X-Device-Id` header with a cert-derived
//! identity sourced from the client cert's SAN URI on mTLS connections.
//!
//! # Sources, in priority order
//!
//! 1. **mTLS client certificate** ([`DeviceIdentitySource::ClientCertificate`]) —
//!    when the TLS layer attached a [`crate::tls::PeerCertChain`] extension
//!    and the leaf cert carries a `SAN URI` matching the configured
//!    scheme (`device://{tenant}/{aad-device-id}` per the runbook in
//!    `docs/provisioning/03-intune-cloud-pki.md`), the device id is the
//!    URI's path component (the AAD device id GUID).
//! 2. **`X-Device-Id` header** ([`DeviceIdentitySource::HeaderTemp`]) —
//!    transitional fallback while the device fleet rolls over to PKCS-
//!    issued client certs. Each header-based extraction emits a
//!    `tracing::warn!` so the migration is grep-able from production
//!    logs.
//!
//! When [`crate::state::MtlsRuntimeConfig::require_on_ingest`] is `true`
//! and no client cert is present, the header fallback is suppressed and
//! the request is rejected `401 Unauthorized` with a
//! `WWW-Authenticate` challenge advertising both auth surfaces.
//!
//! # Why not just trust the header always
//!
//! The header path is unauthenticated — anyone who can reach the API can
//! claim to be any device. The cert path binds the device id to the
//! Intune-issued private key, which is TPM-bound + non-exportable per
//! the cert profile (Step 4 of the runbook). The fallback exists only so
//! we can ship the TLS termination before every device has rolled over.
//!
//! # OID: 1.3.6.1.5.5.7.3.2 (clientAuth EKU)
//!
//! EKU enforcement is handled at the rustls verifier layer (which only
//! accepts certs with `id-kp-clientAuth`). We don't re-check it here;
//! double-validating in the extractor would be belt-and-suspenders but
//! also a divergence point if the rustls config ever loosens.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::header::WWW_AUTHENTICATE;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use common_wire::ErrorBody;
#[cfg(feature = "mtls")]
use sha2::{Digest, Sha256};
#[cfg(feature = "mtls")]
use tracing::debug;
use tracing::warn;

use crate::extract::DEVICE_ID_HEADER;
use crate::state::AppState;

/// Public name of the legacy header so router-layer CORS configuration
/// stays in sync.
pub const X_DEVICE_ID_HEADER: &str = DEVICE_ID_HEADER;

/// An authenticated device identity. Returned by the [`FromRequestParts`]
/// impl on this type and consumed by ingest handlers in place of the
/// previous `DeviceId(String)` newtype.
#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    /// Stable device identifier — the AAD/Entra device ID GUID derived
    /// from the SAN URI's path, or the trimmed `X-Device-Id` header
    /// value under the transitional fallback.
    pub device_id: String,
    /// Tenant GUID derived from the SAN URI's host component. `None`
    /// under header-based fallback (the header carries no tenant claim).
    pub tenant_id: Option<String>,
    /// Lowercase hex SHA-256 of the leaf cert DER. `None` under
    /// header-based fallback.
    pub cert_fingerprint: Option<String>,
    /// How this identity was established. Useful for downstream
    /// authorization decisions ("only allow header path on these
    /// endpoints during migration", etc.) and for log/metric labelling.
    pub source: DeviceIdentitySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceIdentitySource {
    /// `X-Device-Id` HTTP header (deprecated path, transitional fallback).
    HeaderTemp,
    /// SAN URI from the verified mTLS peer certificate.
    ClientCertificate,
}

// ---------------------------------------------------------------------------
// SAN URI parser (pure-data; unit-tested)
// ---------------------------------------------------------------------------

/// Pieces extracted from a SAN URI of shape
/// `<scheme>://{tenant}/{device-id}` (per the Intune PKCS profile in
/// `docs/provisioning/03-intune-cloud-pki.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSanUri {
    pub tenant_id: String,
    pub device_id: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SanUriError {
    #[error("missing scheme separator '://'")]
    MissingSchemeSeparator,
    #[error("scheme '{0}' does not match expected '{1}'")]
    SchemeMismatch(String, String),
    #[error("missing tenant component")]
    MissingTenant,
    #[error("missing device-id component")]
    MissingDeviceId,
    #[error("trailing content after device-id: {0:?}")]
    TrailingContent(String),
}

/// Parse `device://{tenant}/{device-id}` style SAN URIs.
///
/// The scheme parameter is checked against the env-configured
/// [`crate::config::TlsConfig::expected_san_uri_scheme`]; production
/// always passes `"device"`.
///
/// Tolerates a stray trailing `/` after the device-id (some Intune
/// templates emit one) but rejects extra path segments — those are
/// almost certainly an operator typo on the SAN template.
pub fn parse_san_uri(raw: &str, expected_scheme: &str) -> Result<ParsedSanUri, SanUriError> {
    let (scheme, rest) = raw
        .split_once("://")
        .ok_or(SanUriError::MissingSchemeSeparator)?;
    if !scheme.eq_ignore_ascii_case(expected_scheme) {
        return Err(SanUriError::SchemeMismatch(
            scheme.to_string(),
            expected_scheme.to_string(),
        ));
    }
    let (tenant, after_tenant) = rest
        .split_once('/')
        .ok_or(SanUriError::MissingDeviceId)?;
    if tenant.is_empty() {
        return Err(SanUriError::MissingTenant);
    }
    // Strip a single trailing slash. Anything else after another `/` is
    // a structural mismatch — fail loudly rather than silently truncate.
    let device_part = after_tenant.strip_suffix('/').unwrap_or(after_tenant);
    if device_part.is_empty() {
        return Err(SanUriError::MissingDeviceId);
    }
    if let Some((_, trailing)) = device_part.split_once('/') {
        return Err(SanUriError::TrailingContent(trailing.to_string()));
    }
    Ok(ParsedSanUri {
        tenant_id: tenant.to_string(),
        device_id: device_part.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Cert -> identity helpers
// ---------------------------------------------------------------------------

/// Find the first SAN URI on a leaf cert that parses as the configured
/// scheme. Returns `None` for any cert that has no SAN URI extension or
/// whose only SAN URIs use a different scheme.
#[cfg(feature = "mtls")]
fn extract_identity_from_leaf(
    leaf_der: &[u8],
    expected_scheme: &str,
) -> Option<(ParsedSanUri, String)> {
    use x509_parser::extensions::{GeneralName, ParsedExtension};
    use x509_parser::prelude::*;

    let (_, cert) = X509Certificate::from_der(leaf_der).ok()?;
    // Walk the cert's extensions for a SAN, then iterate that SAN's
    // GeneralName entries for URI variants. There may be multiple URIs;
    // the first one whose scheme matches wins, mirroring how rustls
    // picks the first SAN that satisfies the verifier's expectations.
    for ext in cert.extensions() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            for gn in &san.general_names {
                if let GeneralName::URI(uri) = gn {
                    if let Ok(parsed) = parse_san_uri(uri, expected_scheme) {
                        let fingerprint = sha256_hex(leaf_der);
                        return Some((parsed, fingerprint));
                    }
                }
            }
        }
    }
    None
}

#[cfg(feature = "mtls")]
fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

// ---------------------------------------------------------------------------
// Extractor
// ---------------------------------------------------------------------------

impl<S> FromRequestParts<S> for DeviceIdentity
where
    S: Send + Sync,
    Arc<AppState>: axum::extract::FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state: Arc<AppState> = axum::extract::FromRef::from_ref(state);
        let mtls_cfg = &app_state.mtls;

        // 1. Try the mTLS path: peer-cert extension stashed by the TLS
        //    acceptor. The extension is only present when the binary was
        //    built with `--features mtls` AND the request landed via the
        //    TLS-terminating server.
        #[cfg(feature = "mtls")]
        if let Some(chain) = parts.extensions.get::<crate::tls::PeerCertChain>() {
            if let Some(leaf) = chain.leaf() {
                if let Some((parsed, fingerprint)) = extract_identity_from_leaf(
                    leaf.as_ref(),
                    &mtls_cfg.expected_san_uri_scheme,
                ) {
                    debug!(
                        device_id = %parsed.device_id,
                        tenant_id = %parsed.tenant_id,
                        cert_sha256 = %fingerprint,
                        "device identity from client cert SAN URI",
                    );
                    return Ok(DeviceIdentity {
                        device_id: parsed.device_id,
                        tenant_id: Some(parsed.tenant_id),
                        cert_fingerprint: Some(fingerprint),
                        source: DeviceIdentitySource::ClientCertificate,
                    });
                }
                // Cert was presented but its SAN URI didn't parse —
                // log so misconfigured cert templates are observable.
                warn!(
                    "client cert presented but no SAN URI matched scheme {:?}; \
                     falling through to header",
                    mtls_cfg.expected_san_uri_scheme,
                );
            }
        }

        // 2. mTLS-required mode short-circuits before falling back to
        //    the header path. We've already confirmed no usable cert
        //    above; reject with a structured 401.
        if mtls_cfg.require_on_ingest {
            return Err(unauthorized_response(
                "client certificate required for ingest routes (CMTRACE_MTLS_REQUIRE_INGEST=true)",
            ));
        }

        // 3. Legacy header fallback. Logged at WARN so production grep
        //    can drive the cutover deadline.
        if let Some(hv) = parts.headers.get(DEVICE_ID_HEADER) {
            let s = hv.to_str().map_err(|_| {
                bad_request_response("X-Device-Id must be ASCII")
            })?;
            let trimmed = s.trim();
            if trimmed.is_empty() || trimmed.len() > 256 {
                return Err(bad_request_response("X-Device-Id must be 1..=256 chars"));
            }
            warn!(
                device_id = %trimmed,
                "device identity from X-Device-Id header (deprecated; migrate to mTLS)",
            );
            return Ok(DeviceIdentity {
                device_id: trimmed.to_string(),
                tenant_id: None,
                cert_fingerprint: None,
                source: DeviceIdentitySource::HeaderTemp,
            });
        }

        Err(unauthorized_response(
            "missing device identity: present a client certificate (mTLS) or X-Device-Id header",
        ))
    }
}

fn unauthorized_response(message: &str) -> Response {
    let body = Json(ErrorBody {
        error: "unauthorized".into(),
        message: message.to_string(),
    });
    let challenge = "Mutual error=\"client_cert_required\", \
                     error_description=\"present an Intune-issued client cert with SAN URI \
                     device://{tenant}/{aad-device-id}; transitional X-Device-Id header is \
                     accepted only when CMTRACE_MTLS_REQUIRE_INGEST=false\"";
    let mut resp = (StatusCode::UNAUTHORIZED, body).into_response();
    if let Ok(val) = challenge.parse() {
        resp.headers_mut().insert(WWW_AUTHENTICATE, val);
    }
    resp
}

fn bad_request_response(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorBody {
            error: "bad_request".into(),
            message: message.to_string(),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_san_uri_happy_path() {
        let parsed = parse_san_uri(
            "device://00000000-0000-0000-0000-000000000000/11111111-2222-3333-4444-555555555555",
            "device",
        )
        .expect("parses");
        assert_eq!(parsed.tenant_id, "00000000-0000-0000-0000-000000000000");
        assert_eq!(parsed.device_id, "11111111-2222-3333-4444-555555555555");
    }

    #[test]
    fn parse_san_uri_tolerates_trailing_slash() {
        let parsed = parse_san_uri("device://tenant/dev/", "device").expect("parses");
        assert_eq!(parsed.tenant_id, "tenant");
        assert_eq!(parsed.device_id, "dev");
    }

    #[test]
    fn parse_san_uri_case_insensitive_scheme() {
        let parsed = parse_san_uri("Device://tenant/dev", "device").expect("parses");
        assert_eq!(parsed.tenant_id, "tenant");
        assert_eq!(parsed.device_id, "dev");
    }

    #[test]
    fn parse_san_uri_rejects_missing_scheme_separator() {
        let err = parse_san_uri("device:/tenant/dev", "device").unwrap_err();
        assert_eq!(err, SanUriError::MissingSchemeSeparator);
    }

    #[test]
    fn parse_san_uri_rejects_wrong_scheme() {
        let err = parse_san_uri("https://tenant/dev", "device").unwrap_err();
        assert!(matches!(err, SanUriError::SchemeMismatch(_, _)), "got {err:?}");
    }

    #[test]
    fn parse_san_uri_rejects_missing_tenant() {
        let err = parse_san_uri("device:///dev", "device").unwrap_err();
        assert_eq!(err, SanUriError::MissingTenant);
    }

    #[test]
    fn parse_san_uri_rejects_missing_device_id() {
        let err = parse_san_uri("device://tenant", "device").unwrap_err();
        assert_eq!(err, SanUriError::MissingDeviceId);

        let err = parse_san_uri("device://tenant/", "device").unwrap_err();
        assert_eq!(err, SanUriError::MissingDeviceId);
    }

    #[test]
    fn parse_san_uri_rejects_extra_path_segments() {
        let err = parse_san_uri("device://tenant/dev/extra/seg", "device").unwrap_err();
        assert!(matches!(err, SanUriError::TrailingContent(_)), "got {err:?}");
    }

    #[test]
    fn parse_san_uri_respects_custom_scheme() {
        let parsed = parse_san_uri("agent://t/d", "agent").expect("parses");
        assert_eq!(parsed.device_id, "d");
    }

    #[tokio::test]
    async fn header_fallback_when_no_cert_and_not_required() {
        // Build a minimal AppState with mtls.require_on_ingest = false.
        use crate::storage::{LocalFsBlobStore, SqliteMetadataStore};
        use std::sync::Arc;

        let tmp = tempfile::TempDir::new().unwrap();
        let blobs = Arc::new(LocalFsBlobStore::new(tmp.path()).await.unwrap());
        let meta = Arc::new(SqliteMetadataStore::connect(":memory:").await.unwrap());
        let state = AppState::new_auth_disabled(meta, blobs, "127.0.0.1:0".to_string());

        // Forge a request with the X-Device-Id header set.
        let req = axum::http::Request::builder()
            .uri("/anything")
            .header(DEVICE_ID_HEADER, "WIN-FALLBACK-01")
            .body(())
            .unwrap();
        let (mut parts, _) = req.into_parts();
        let id = DeviceIdentity::from_request_parts(&mut parts, &state)
            .await
            .expect("header path should succeed");
        assert_eq!(id.device_id, "WIN-FALLBACK-01");
        assert_eq!(id.source, DeviceIdentitySource::HeaderTemp);
        assert!(id.tenant_id.is_none());
        assert!(id.cert_fingerprint.is_none());
    }

    #[tokio::test]
    async fn unauthorized_when_required_and_no_cert() {
        use crate::state::MtlsRuntimeConfig;
        use crate::storage::{LocalFsBlobStore, SqliteMetadataStore};
        use std::sync::Arc;

        let tmp = tempfile::TempDir::new().unwrap();
        let blobs = Arc::new(LocalFsBlobStore::new(tmp.path()).await.unwrap());
        let meta = Arc::new(SqliteMetadataStore::connect(":memory:").await.unwrap());
        let auth = crate::auth::AuthState {
            mode: crate::auth::AuthMode::Disabled,
            entra: None,
            jwks: Arc::new(crate::auth::JwksCache::new(
                "http://127.0.0.1:1/unused".to_string(),
            )),
        };
        let mtls = MtlsRuntimeConfig {
            require_on_ingest: true,
            expected_san_uri_scheme: "device".into(),
        };
        let state = AppState::full(
            meta,
            blobs,
            "127.0.0.1:0".to_string(),
            auth,
            crate::state::CorsConfig::default(),
            mtls,
        );

        let req = axum::http::Request::builder()
            .uri("/anything")
            .header(DEVICE_ID_HEADER, "WIN-NOPE-01")
            .body(())
            .unwrap();
        let (mut parts, _) = req.into_parts();
        let resp = DeviceIdentity::from_request_parts(&mut parts, &state)
            .await
            .expect_err("must reject when cert is required");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().contains_key(WWW_AUTHENTICATE));
    }
}
