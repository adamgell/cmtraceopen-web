//! Device-identity extraction for ingest routes.
//!
//! Replaces the temporary `X-Device-Id` header with a cert-derived
//! identity sourced from the client cert's SAN URI on mTLS connections.
//!
//! # Sources, in priority order
//!
//! 1. **Reverse-proxy cert header** ([`DeviceIdentitySource::PeerCertHeader`]) —
//!    when `CMTRACE_PEER_CERT_HEADER` is set (AppGW-terminated mTLS path),
//!    the peer cert PEM (or base64-PEM) is read from the named header.
//!    The header is only honoured when the TCP peer address falls within
//!    `CMTRACE_TRUSTED_PROXY_CIDR`; requests from outside that CIDR have
//!    the header silently stripped.
//! 2. **mTLS client certificate** ([`DeviceIdentitySource::ClientCertificate`]) —
//!    when the TLS layer attached a [`crate::tls::PeerCertChain`] extension
//!    and the leaf cert carries a `SAN URI` matching the configured
//!    scheme (`device://{tenant}/{aad-device-id}` per the runbook in
//!    `docs/provisioning/03-intune-cloud-pki.md`), the device id is the
//!    URI's path component (the AAD device id GUID).
//! 3. **`X-Device-Id` header** ([`DeviceIdentitySource::HeaderTemp`]) —
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

#[cfg(feature = "mtls")]
use std::net::SocketAddr;

#[cfg(feature = "mtls")]
use axum::extract::ConnectInfo;
use axum::extract::FromRequestParts;
#[cfg(all(feature = "mtls", feature = "crl"))]
use axum::http::header::RETRY_AFTER;
use axum::http::header::WWW_AUTHENTICATE;
#[cfg(all(feature = "mtls", feature = "crl"))]
use axum::http::HeaderValue;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
#[cfg(feature = "mtls")]
use base64::engine::general_purpose::{STANDARD, URL_SAFE};
#[cfg(feature = "mtls")]
use base64::Engine as _;
use common_wire::ErrorBody;
#[cfg(feature = "mtls")]
use sha2::{Digest, Sha256};
#[cfg(feature = "mtls")]
use tracing::debug;
use tracing::warn;

#[cfg(all(feature = "mtls", feature = "crl"))]
use crate::auth::RevocationStatus;

use crate::extract::DEVICE_ID_HEADER;
#[cfg(feature = "mtls")]
use crate::middleware::proxy::is_trusted_proxy;
use crate::state::AppState;

/// Public name of the legacy header so router-layer CORS configuration
/// stays in sync.
pub const X_DEVICE_ID_HEADER: &str = DEVICE_ID_HEADER;

/// Metric counter name for peer-cert source tracking.
const METRIC_PEER_CERT_SOURCE: &str = "cmtrace_peer_cert_source_total";

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
    /// SAN URI parsed from a PEM cert forwarded by a trusted reverse proxy
    /// (e.g. Azure Application Gateway via `X-ARR-ClientCert`).
    PeerCertHeader,
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

/// Apply the CRL decision matrix to a leaf cert. Returns `None` to
/// continue extractor flow (accept), or `Some(Response)` carrying the
/// rejection to return to the client. See `docs/wave4/06-crl-wiring.md`
/// for the full table.
///
/// Re-parses the leaf DER inline rather than threading the parsed cert
/// through `extract_identity_from_leaf`; the parse is cheap (microseconds
/// on a 2 KiB leaf) and keeping the surfaces independent means a future
/// rework of either path doesn't drag the other.
#[cfg(all(feature = "mtls", feature = "crl"))]
fn check_revocation(
    crl: &crate::auth::CrlCache,
    leaf_der: &[u8],
) -> Option<Response> {
    use x509_parser::prelude::{FromDer, X509Certificate};

    // If the leaf can't be parsed at all, the SAN-URI extractor below
    // will also fail and the request will end up rejected for a
    // different reason. Don't conflate that with revocation here —
    // accept-and-let-the-next-stage-decide.
    let (_, parsed) = match X509Certificate::from_der(leaf_der) {
        Ok(p) => p,
        Err(err) => {
            warn!(%err, "CRL check: leaf cert failed to parse; deferring to identity extractor");
            return None;
        }
    };
    let serial = parsed.tbs_certificate.raw_serial();

    match crl.revocation_status(serial) {
        RevocationStatus::Revoked => {
            warn!(
                serial = %hex::encode(serial),
                "client cert revoked by CRL; rejecting request",
            );
            metrics::counter!(
                "cmtrace_crl_revocations_total",
                "result" => "rejected",
            )
            .increment(1);
            let mut resp = (
                StatusCode::UNAUTHORIZED,
                Json(ErrorBody {
                    error: "unauthorized".into(),
                    message: "client certificate revoked".into(),
                }),
            )
                .into_response();
            resp.headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("cert-revoked"));
            Some(resp)
        }
        RevocationStatus::NotRevoked => None,
        RevocationStatus::Unknown => {
            if crl.fail_open() {
                metrics::counter!(
                    "cmtrace_crl_revocations_total",
                    "result" => "unknown_fail_open",
                )
                .increment(1);
                None
            } else {
                warn!(
                    serial = %hex::encode(serial),
                    "client cert revocation status unknown (CRL cache cold); rejecting per crl_fail_open=false",
                );
                metrics::counter!(
                    "cmtrace_crl_revocations_total",
                    "result" => "unknown_fail_closed",
                )
                .increment(1);
                let mut resp = (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(ErrorBody {
                        error: "service_unavailable".into(),
                        message:
                            "client cert revocation status unknown; CRL cache not yet warm. Retry shortly."
                                .into(),
                    }),
                )
                    .into_response();
                resp.headers_mut()
                    .insert(RETRY_AFTER, HeaderValue::from_static("60"));
                Some(resp)
            }
        }
    }
}

/// Decode a cert from a reverse-proxy header value.
///
/// Azure Application Gateway sends the client cert as a PEM string
/// base64-encoded in the `X-ARR-ClientCert` header. Some other proxies
/// (nginx `ssl_client_cert`, HAProxy) send the PEM inline with newlines
/// percent-encoded (`%0A`). This function tries the following in order:
///
/// 1. If the trimmed value starts with `-----BEGIN`, it is raw PEM.
/// 2. Try standard base64 decode, then URL-safe base64 (AppGW default).
///    - If decoded bytes start with `-----BEGIN` → base64(PEM); parse to DER.
///    - If decoded bytes start with `0x30` (ASN.1 SEQUENCE tag) → base64(DER).
///    - Otherwise, the base64 payload is not a cert; skip this path.
/// 3. Try percent-decoding (nginx `ssl_client_cert` uses `%0A` for newlines,
///    but other percent-sequences may appear). Returns `None` if the
///    percent-decoded result does not start with `-----BEGIN`.
///
/// Returning `None` for any format that doesn't match gives the caller a
/// clear signal ("this header value is not a cert") rather than passing
/// garbage bytes to the DER parser and getting a cryptic 401.
///
/// Note: PEM is ASCII-only, so `percent_decode_pem` → `String::from_utf8`
/// is lossless for any well-formed cert header. Non-UTF-8 sequences after
/// percent-decoding return `None`.
#[cfg(feature = "mtls")]
fn decode_peer_cert_header(value: &str) -> Option<Vec<u8>> {
    use rustls_pemfile::certs;
    use std::io::BufReader;

    let trimmed = value.trim();

    // Nothing to decode.
    if trimmed.is_empty() {
        return None;
    }

    // Path 1: raw PEM — starts with the standard PEM banner.
    if trimmed.starts_with("-----BEGIN") {
        let mut reader = BufReader::new(trimmed.as_bytes());
        return certs(&mut reader).find_map(|r| r.ok()).map(|c| c.to_vec());
    }

    // Path 2: base64-encoded payload (AppGW default). Try standard, then
    // URL-safe alphabet. Note: `%` is not in either base64 alphabet, so
    // percent-encoded values will fail here and fall through to path 3.
    let b64_decoded = STANDARD
        .decode(trimmed)
        .or_else(|_| URL_SAFE.decode(trimmed))
        .ok();
    if let Some(decoded) = b64_decoded {
        if decoded.starts_with(b"-----BEGIN") {
            // base64(PEM) — AppGW default format.
            let pem_text = String::from_utf8(decoded).ok()?;
            let mut reader = BufReader::new(pem_text.as_bytes());
            return certs(&mut reader).find_map(|r| r.ok()).map(|c| c.to_vec());
        }
        // base64(DER) — only accept if the bytes look like an ASN.1
        // SEQUENCE (tag byte 0x30). Accepting arbitrary base64 payloads
        // that decode to non-PEM, non-DER bytes would surface as a cryptic
        // 401 rather than "the header value is not a cert".
        if decoded.first().copied() == Some(0x30) {
            return Some(decoded);
        }
        // Decoded bytes are neither PEM nor DER — fall through to path 3.
    }

    // Path 3: percent-encoded PEM (nginx ssl_client_cert uses %0A for
    // newlines; other percent-sequences are also handled). Tried as a
    // fallback so no gate heuristic is needed — percent-decoding is cheap
    // and the result is validated against `-----BEGIN`.
    let pct_decoded = percent_decode_pem(trimmed)?;
    if pct_decoded.starts_with("-----BEGIN") {
        let mut reader = BufReader::new(pct_decoded.as_bytes());
        return certs(&mut reader).find_map(|r| r.ok()).map(|c| c.to_vec());
    }

    None
}

/// Percent-decode a string, handling the `%XX` escape sequences used by
/// proxies like nginx when forwarding PEM in HTTP headers.
///
/// Collects decoded bytes first, then validates as UTF-8 via
/// `String::from_utf8`. PEM is ASCII-only, so any well-formed cert header
/// will round-trip cleanly. Returns `None` if the decoded bytes are not
/// valid UTF-8 (which would indicate a non-PEM, non-ASCII payload).
#[cfg(feature = "mtls")]
fn percent_decode_pem(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                hex_nibble(bytes[i + 1]),
                hex_nibble(bytes[i + 2]),
            ) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(feature = "mtls")]
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Re-validate `leaf_der` against the operator-supplied CA trust anchors.
///
/// Builds a [`rustls::RootCertStore`] from `ca_ders` and calls
/// [`rustls::server::WebPkiClientVerifier::verify_client_cert`] with an
/// empty intermediate chain. The cert must chain directly to one of the
/// trust anchors (i.e., the **issuing** CA cert must be in the bundle —
/// both Root CA and Issuing CA belong in `CMTRACE_CLIENT_CA_BUNDLE`).
///
/// Also checks the cert's validity period (`notBefore` ≤ `now` ≤
/// `notAfter`) because the proxy may have already validated this, but we
/// cannot assume that without reading the cert ourselves.
///
/// Returns `Ok(())` when the cert is valid; `Err(reason)` with a
/// log-friendly description of the failure otherwise.
///
/// # Why this matters
///
/// When `CMTRACE_PEER_CERT_HEADER` is used, AppGW is supposed to verify
/// the client cert against its `clientCertSettings.trustedRootCertificate`
/// before forwarding it. But if the AppGW is misconfigured (no client-cert
/// policy, or a test proxy forwards headers unconditionally), a caller with
/// HTTP access could inject an arbitrary cert — including a self-signed one
/// — and claim any device identity. Re-validating here closes that gap.
#[cfg(feature = "mtls")]
fn validate_cert_against_ca_bundle(
    leaf_der: &[u8],
    ca_ders: &[Vec<u8>],
) -> Result<(), String> {
    use rustls::server::WebPkiClientVerifier;
    use rustls::RootCertStore;
    use rustls_pki_types::{CertificateDer, UnixTime};

    debug_assert!(!ca_ders.is_empty(), "caller should check for empty ca_ders");

    let mut roots = RootCertStore::empty();
    for der in ca_ders {
        roots
            .add(CertificateDer::from(der.clone()))
            .map_err(|e| format!("invalid CA cert DER in bundle: {e}"))?;
    }

    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|e| format!("failed to build CA verifier: {e}"))?;

    let leaf = CertificateDer::from(leaf_der.to_vec());
    let now = UnixTime::now();

    // No intermediates — the issuing CA must be a trust anchor in the
    // bundle. The `allow_unauthenticated` flag is NOT set, so an untrusted
    // or expired cert is a hard error.
    verifier
        .verify_client_cert(&leaf, &[], now)
        .map_err(|e| e.to_string())
        .map(|_| ())
}

impl<S> FromRequestParts<S> for DeviceIdentity
where
    S: Send + Sync,
    Arc<AppState>: axum::extract::FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state: Arc<AppState> = axum::extract::FromRef::from_ref(state);
        let mtls_cfg = &app_state.mtls;

        // 1. Try the reverse-proxy cert-header path. This path is active when
        //    CMTRACE_PEER_CERT_HEADER is configured (AppGW-terminated mTLS).
        //    Gate: the request must originate from within TRUSTED_PROXY_CIDR.
        #[cfg(feature = "mtls")]
        if let Some(header_name) = &mtls_cfg.peer_cert_header {
            if let Some(cidr) = &mtls_cfg.trusted_proxy_cidr {
                // Peer IP is injected by axum's ConnectInfo layer
                // (into_make_service_with_connect_info). Absence means the
                // server was started without that wrapper (e.g. unit tests
                // or an axum::serve call that wasn't updated) — treat as
                // untrusted to fail safe.
                let peer_addr = parts
                    .extensions
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|ci| ci.0.ip());

                match peer_addr {
                    Some(ip) if is_trusted_proxy(ip, cidr) => {
                        // Trusted source — read and decode the cert header.
                        if let Some(hv) = parts.headers.get(header_name.as_str()) {
                            if let Ok(cert_str) = hv.to_str() {
                                let decoded = decode_peer_cert_header(cert_str);
                                if decoded.is_none() && !cert_str.trim().is_empty() {
                                    // Header was present and non-empty but decode
                                    // failed (none of raw-PEM / base64-PEM /
                                    // base64-DER / percent-encoded paths matched).
                                    // Surface as `header_invalid` so an operator
                                    // pointing the header at the wrong source has a
                                    // clear signal in the metric instead of cryptic
                                    // downstream 401s.
                                    warn!(
                                        peer_ip = %ip,
                                        header = %header_name,
                                        "cert header present from trusted proxy but \
                                         could not be decoded as PEM/DER (raw or \
                                         base64 or percent-encoded); falling through",
                                    );
                                    metrics::counter!(
                                        METRIC_PEER_CERT_SOURCE,
                                        "source" => "header_invalid"
                                    )
                                    .increment(1);
                                }
                                if let Some(der) = decoded {
                                    // Re-validate the cert against our CA bundle
                                    // before trusting its SAN URI. AppGW should
                                    // have already verified the chain, but we
                                    // check here so a misconfigured proxy (or a
                                    // test environment that forwards headers
                                    // unconditionally) cannot inject an arbitrary
                                    // self-signed cert and claim any device id.
                                    if !mtls_cfg.trusted_ca_ders.is_empty() {
                                        if let Err(reason) = validate_cert_against_ca_bundle(
                                            &der,
                                            &mtls_cfg.trusted_ca_ders,
                                        ) {
                                            warn!(
                                                peer_ip = %ip,
                                                header = %header_name,
                                                reason = %reason,
                                                "cert header from trusted proxy rejected by CA \
                                                 bundle validation (chain does not lead to a \
                                                 configured trust anchor); falling through",
                                            );
                                            // Bump the dedicated `header_invalid` metric label
                                            // so security ops can spot forged-cert probe
                                            // attempts in `cmtrace_peer_cert_source_total`
                                            // independently from the all-paths-failed `none`
                                            // bucket. Per-route enforcement
                                            // (`require_on_ingest=true`) will still 401 on the
                                            // fall-through.
                                            metrics::counter!(
                                                METRIC_PEER_CERT_SOURCE,
                                                "source" => "header_invalid"
                                            )
                                            .increment(1);
                                        } else if let Some((parsed, fingerprint)) =
                                            extract_identity_from_leaf(
                                                &der,
                                                &mtls_cfg.expected_san_uri_scheme,
                                            )
                                        {
                                            debug!(
                                                device_id = %parsed.device_id,
                                                tenant_id = %parsed.tenant_id,
                                                cert_sha256 = %fingerprint,
                                                peer_ip = %ip,
                                                header = %header_name,
                                                "device identity from reverse-proxy cert header \
                                                 (chain-validated against CA bundle)",
                                            );
                                            metrics::counter!(
                                                METRIC_PEER_CERT_SOURCE,
                                                "source" => "header"
                                            )
                                            .increment(1);
                                            return Ok(DeviceIdentity {
                                                device_id: parsed.device_id,
                                                tenant_id: Some(parsed.tenant_id),
                                                cert_fingerprint: Some(fingerprint),
                                                source: DeviceIdentitySource::PeerCertHeader,
                                            });
                                        } else {
                                            // Cert validated but no matching SAN URI — fall
                                            // through.
                                            warn!(
                                                peer_ip = %ip,
                                                header = %header_name,
                                                "cert header CA-validated but no SAN URI matched \
                                                 scheme {:?}; falling through to next identity \
                                                 source",
                                                mtls_cfg.expected_san_uri_scheme,
                                            );
                                        }
                                    } else if let Some((parsed, fingerprint)) =
                                        extract_identity_from_leaf(
                                            &der,
                                            &mtls_cfg.expected_san_uri_scheme,
                                        )
                                    {
                                        // No CA bundle configured (test or transitional
                                        // mode) — accept without chain validation. In
                                        // production, TlsConfig::from_env ensures the
                                        // bundle is always present.
                                        debug!(
                                            device_id = %parsed.device_id,
                                            tenant_id = %parsed.tenant_id,
                                            cert_sha256 = %fingerprint,
                                            peer_ip = %ip,
                                            header = %header_name,
                                            "device identity from reverse-proxy cert header \
                                             (no CA bundle; skipping chain validation)",
                                        );
                                        metrics::counter!(
                                            METRIC_PEER_CERT_SOURCE,
                                            "source" => "header"
                                        )
                                        .increment(1);
                                        return Ok(DeviceIdentity {
                                            device_id: parsed.device_id,
                                            tenant_id: Some(parsed.tenant_id),
                                            cert_fingerprint: Some(fingerprint),
                                            source: DeviceIdentitySource::PeerCertHeader,
                                        });
                                    } else {
                                        // Cert decoded but no valid SAN URI — fall through.
                                        warn!(
                                            peer_ip = %ip,
                                            header = %header_name,
                                            "cert header present but no SAN URI matched scheme {:?}; \
                                             falling through to next identity source",
                                            mtls_cfg.expected_san_uri_scheme,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Some(ip) => {
                        // The cert header was present but came from an
                        // untrusted IP — strip it and log so security ops
                        // can spot probe attempts.
                        warn!(
                            peer_ip = %ip,
                            header = %header_name,
                            "cert header received from untrusted peer IP (outside \
                             CMTRACE_TRUSTED_PROXY_CIDR); ignoring header",
                        );
                    }
                    None => {
                        // No ConnectInfo in extensions — server was not started
                        // with into_make_service_with_connect_info; treat as
                        // untrusted (fail safe).
                        warn!(
                            header = %header_name,
                            "CMTRACE_PEER_CERT_HEADER is configured but \
                             ConnectInfo<SocketAddr> is not available; \
                             ignoring cert header (start with \
                             into_make_service_with_connect_info to enable \
                             trusted-proxy gating)",
                        );
                    }
                }
            }
        }

        // 2. Try the mTLS path: peer-cert extension stashed by the TLS
        //    acceptor. The extension is only present when the binary was
        //    built with `--features mtls` AND the request landed via the
        //    TLS-terminating server.
        #[cfg(feature = "mtls")]
        if let Some(chain) = parts.extensions.get::<crate::tls::PeerCertChain>() {
            if let Some(leaf) = chain.leaf() {
                // 1a. CRL revocation check, before any identity is
                //     constructed. See `docs/wave4/06-crl-wiring.md`
                //     for the full decision matrix. This is the entire
                //     payoff of PR #47's polling loop: without this
                //     block the cache populates but nothing consults it.
                #[cfg(feature = "crl")]
                if let Some(crl) = app_state.crl_cache.as_ref() {
                    if let Some(rejection) = check_revocation(crl, leaf.as_ref()) {
                        return Err(rejection);
                    }
                }

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
                    metrics::counter!(METRIC_PEER_CERT_SOURCE, "source" => "tls").increment(1);
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

        // 3. mTLS-required mode short-circuits before falling back to
        //    the header path. We've already confirmed no usable cert
        //    above; reject with a structured 401.
        if mtls_cfg.require_on_ingest {
            metrics::counter!(METRIC_PEER_CERT_SOURCE, "source" => "none").increment(1);
            return Err(unauthorized_response(
                "client certificate required for ingest routes (CMTRACE_MTLS_REQUIRE_INGEST=true)",
            ));
        }

        // 4. Legacy header fallback. Logged at WARN so production grep
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

        metrics::counter!(METRIC_PEER_CERT_SOURCE, "source" => "none").increment(1);
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

    // ---- CRL wiring ---------------------------------------------------
    //
    // These tests exercise the new CRL plumbing added by
    // `feat/wire-crl-revocation`. They drive `check_revocation` directly
    // instead of constructing a full extractor pipeline because:
    //   - the function is the entire decision surface (status code,
    //     header, metric label, body shape all live there);
    //   - building a `PeerCertChain` requires either rcgen (test-mtls
    //     feature only) or a hand-rolled X.509 leaf, both of which add
    //     complexity that doesn't change what's being verified.
    //
    // We hand-roll a minimal DER cert just rich enough for x509-parser
    // to expose `tbs_certificate.raw_serial()`. This mirrors the
    // hand-rolled CRL DER used by `crl::tests::build_minimal_crl`.
    #[cfg(all(feature = "mtls", feature = "crl"))]
    mod crl_wiring {
        use super::super::check_revocation;
        use crate::auth::CrlCache;
        use axum::http::header::{RETRY_AFTER, WWW_AUTHENTICATE};
        use axum::http::StatusCode;
        use std::sync::Arc;
        use std::time::Duration;

        /// Build a minimal DER X.509 v1 cert with the given serial.
        ///
        /// Mirrors `crl::tests::build_minimal_crl`'s ASN.1 layout: just
        /// enough structure for `x509-parser` to parse it and surface
        /// `tbs_certificate.raw_serial()`. Not signed — `x509-parser`
        /// without the `verify` feature doesn't check signatures, and
        /// the workspace bans the `verify` feature (pulls ring).
        fn build_minimal_leaf(serial: &[u8]) -> Vec<u8> {
            // sha256WithRSAEncryption AlgorithmIdentifier
            let alg_id: Vec<u8> = vec![
                0x30, 0x0d,
                0x06, 0x09,
                0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b,
                0x05, 0x00,
            ];

            // Issuer + Subject: SEQUENCE { SET { SEQUENCE { OID CN, UTF8 "test" }}}
            fn name(cn: &[u8]) -> Vec<u8> {
                let mut atv: Vec<u8> = Vec::new();
                atv.extend_from_slice(&[0x06, 0x03, 0x55, 0x04, 0x03]);
                atv.push(0x0c);
                atv.push(cn.len() as u8);
                atv.extend_from_slice(cn);
                let mut atv_seq = vec![0x30, atv.len() as u8];
                atv_seq.extend_from_slice(&atv);
                let mut rdn = vec![0x31, atv_seq.len() as u8];
                rdn.extend_from_slice(&atv_seq);
                let mut out = vec![0x30, rdn.len() as u8];
                out.extend_from_slice(&rdn);
                out
            }
            let issuer = name(b"test-ca");
            let subject = name(b"test-leaf");

            // Validity: SEQUENCE { UTCTime "250101000000Z", UTCTime "350101000000Z" }
            let validity: Vec<u8> = vec![
                0x30, 0x1e,
                0x17, 0x0d, b'2', b'5', b'0', b'1', b'0', b'1', b'0', b'0', b'0', b'0', b'0', b'0', b'Z',
                0x17, 0x0d, b'3', b'5', b'0', b'1', b'0', b'1', b'0', b'0', b'0', b'0', b'0', b'0', b'Z',
            ];

            // SubjectPublicKeyInfo: minimal — RSA OID, NULL params,
            // BIT STRING wrapping a SEQUENCE { INTEGER 1, INTEGER 1 }.
            // (The exact RSA modulus/exponent are unused — x509-parser
            // only walks the structure to surface tbsCertificate fields.)
            //
            // Layout:
            //   inner_pubkey  = 0x30 0x06 0x02 0x01 0x01 0x02 0x01 0x01  (8 bytes)
            //   bit_string    = 0x03 0x09 0x00 <inner_pubkey>            (11 bytes)
            //   alg_id        = 0x30 0x0d 0x06 0x09 <rsaEncryption OID> 0x05 0x00  (15 bytes)
            //   spki body     = alg_id ++ bit_string                     (15 + 11 = 26 bytes)
            let spki: Vec<u8> = vec![
                0x30, 0x1a,                                     // SEQUENCE len 26
                0x30, 0x0d,                                     // AlgorithmIdentifier len 13
                0x06, 0x09,                                     // OID len 9
                0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, // rsaEncryption
                0x05, 0x00,                                     // NULL params
                0x03, 0x09,                                     // BIT STRING len 9
                0x00,                                           // 0 unused bits
                0x30, 0x06,                                     // SEQUENCE len 6
                0x02, 0x01, 0x01,                               // INTEGER 1 (modulus)
                0x02, 0x01, 0x01,                               // INTEGER 1 (exponent)
            ];

            // tbsCertificate (v1): SEQUENCE { serial INTEGER, sigAlg, issuer, validity, subject, spki }
            let mut serial_tlv = vec![0x02, serial.len() as u8];
            serial_tlv.extend_from_slice(serial);
            let mut tbs: Vec<u8> = Vec::new();
            tbs.extend_from_slice(&serial_tlv);
            tbs.extend_from_slice(&alg_id);
            tbs.extend_from_slice(&issuer);
            tbs.extend_from_slice(&validity);
            tbs.extend_from_slice(&subject);
            tbs.extend_from_slice(&spki);
            let tbs_seq = wrap_seq(&tbs);

            // Dummy signature.
            let sig: Vec<u8> = vec![0x03, 0x05, 0x00, 0xde, 0xad, 0xbe, 0xef];

            let mut cert: Vec<u8> = Vec::new();
            cert.extend_from_slice(&tbs_seq);
            cert.extend_from_slice(&alg_id);
            cert.extend_from_slice(&sig);
            wrap_seq(&cert)
        }

        fn wrap_seq(body: &[u8]) -> Vec<u8> {
            let mut out = vec![0x30];
            let len = body.len();
            if len < 128 {
                out.push(len as u8);
            } else if len < 256 {
                out.push(0x81);
                out.push(len as u8);
            } else {
                out.push(0x82);
                out.push((len >> 8) as u8);
                out.push((len & 0xff) as u8);
            }
            out.extend_from_slice(body);
            out
        }

        #[test]
        fn crl_revoked_serial_returns_401() {
            // Cache contains serial 0x42; leaf cert has serial 0x42.
            // Expect: 401 Unauthorized + WWW-Authenticate: cert-revoked.
            let cache = Arc::new(CrlCache::new(
                ["http://example.invalid/crl".to_string()],
                Duration::from_secs(3600),
                false,
            ));
            let url: reqwest::Url = "http://example.invalid/crl".parse().unwrap();
            cache.insert_for_test(url, vec![vec![0x42]]);

            let leaf = build_minimal_leaf(&[0x42]);
            let resp = check_revocation(&cache, &leaf)
                .expect("revoked serial must produce a rejection response");
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(
                resp.headers()
                    .get(WWW_AUTHENTICATE)
                    .map(|v| v.to_str().unwrap()),
                Some("cert-revoked"),
            );
        }

        #[test]
        fn crl_unknown_serial_fail_open_passes() {
            // Cache empty (no successful fetch ever), fail_open=true.
            // Expect: None (accept and continue extractor flow).
            let cache = Arc::new(CrlCache::new(
                std::iter::empty::<String>(),
                Duration::from_secs(3600),
                true,
            ));

            let leaf = build_minimal_leaf(&[0x99]);
            assert!(
                check_revocation(&cache, &leaf).is_none(),
                "fail_open=true with cold cache must accept",
            );
        }

        #[test]
        fn crl_unknown_serial_fail_closed_returns_503() {
            // Cache empty, fail_open=false.
            // Expect: 503 Service Unavailable + Retry-After: 60.
            let cache = Arc::new(CrlCache::new(
                std::iter::empty::<String>(),
                Duration::from_secs(3600),
                false,
            ));

            let leaf = build_minimal_leaf(&[0x99]);
            let resp = check_revocation(&cache, &leaf)
                .expect("fail_open=false with cold cache must reject");
            assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(
                resp.headers().get(RETRY_AFTER).map(|v| v.to_str().unwrap()),
                Some("60"),
            );
        }
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
            ..Default::default()
        };
        let state = AppState::full(
            meta,
            blobs,
            "127.0.0.1:0".to_string(),
            auth,
            crate::state::CorsConfig::default(),
            mtls,
            Arc::new(crate::state::RateLimitState::disabled()),
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

    // --- decode_peer_cert_header unit tests (mtls-gated) ---

    #[cfg(feature = "mtls")]
    #[test]
    fn decode_peer_cert_header_rejects_empty() {
        // Empty / whitespace-only values should return None.
        assert!(decode_peer_cert_header("").is_none());
        assert!(decode_peer_cert_header("   ").is_none());
    }

    #[cfg(feature = "mtls")]
    #[test]
    fn percent_decode_pem_roundtrip() {
        let pem = "-----BEGIN CERTIFICATE-----\nABCD\n-----END CERTIFICATE-----\n";
        let encoded = pem.replace('\n', "%0a");
        let decoded = percent_decode_pem(&encoded);
        assert_eq!(decoded, Some(pem.to_string()));
    }

    #[cfg(feature = "mtls")]
    #[test]
    fn percent_decode_handles_upper_and_lower_hex() {
        // %0A (upper) and %0a (lower) should both decode to newline.
        let upper = "abc%0Axyz";
        let lower = "abc%0axyz";
        assert_eq!(percent_decode_pem(upper), Some("abc\nxyz".to_string()));
        assert_eq!(percent_decode_pem(lower), Some("abc\nxyz".to_string()));
    }
}
