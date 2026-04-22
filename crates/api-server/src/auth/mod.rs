//! Operator bearer-token authentication against Microsoft Entra ID (Azure AD).
//!
//! # Overview
//! Query routes (devices, sessions, files, entries) are gated on a valid
//! Entra-issued JWT in `Authorization: Bearer <token>`. Ingest routes stay
//! unauthenticated for now — they will move to mTLS in Wave 3. See
//! `routes/ingest.rs` + the `X-Device-Id` placeholder in `extract.rs`.
//!
//! # Trust chain
//!  1. Token's header `kid` identifies the signing key.
//!  2. Public key is fetched from Entra's JWKS endpoint
//!     (`CMTRACE_ENTRA_JWKS_URI`) and cached in-process for 1h.
//!  3. Signature is verified with the cached RSA public key; `aud` must
//!     match `CMTRACE_ENTRA_AUDIENCE`, `iss` must match
//!     `https://login.microsoftonline.com/<tenant-id>/v2.0`, and `exp`/`nbf`
//!     are checked with default 15 s clock skew.
//!  4. Authorisation: `scp` must contain `CmtraceOpen.Query` OR (for
//!     client-credential / app-only tokens) `roles` must contain the same
//!     role name. This keeps the matrix "anyone with the Query scope OR app
//!     permission may read".
//!
//! # Dev-mode bypass
//! When `CMTRACE_AUTH_MODE=disabled` is set the extractor short-circuits to
//! a synthetic `OperatorPrincipal { subject: "dev", ... }`. Intended for
//! `cargo run` on a laptop; MUST NOT be flipped in production. The config
//! layer logs a loud WARN on startup every time it's observed.
//!
//! # Crate choice
//! We use `jwt-simple` rather than `jsonwebtoken` because the latter drags
//! `ring` into the build tree and we have a project-wide no-ring rule (see
//! README's TLS-feature note in `crates/api-server/Cargo.toml`).
//! `jwt-simple` ships with pure-rust crypto (`rsa` / `p256` / `ed25519-dalek`
//! via the `pure-rust` feature) and exposes `RS256PublicKey::from_components`
//! for loading a JWK's `n` + `e` parameters directly.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
// Explicit imports from jwt-simple — we avoid `prelude::*` because it
// re-exports `coarsetime::Duration`, which would collide with the
// `std::time::Duration` we use for the JWKS TTL above.
use jwt_simple::algorithms::{RS256PublicKey, RSAPublicKeyLike};
use jwt_simple::common::VerificationOptions;
use jwt_simple::token::Token;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Required scope / role on the access token authorizing operator queries.
pub const REQUIRED_SCOPE: &str = "CmtraceOpen.Query";

/// Default JWKS cache lifetime. One hour matches Entra's key-rotation cadence
/// guidance and avoids hammering the discovery endpoint.
pub const DEFAULT_JWKS_TTL: Duration = Duration::from_secs(60 * 60);

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Auth enforcement mode. Parsed from `CMTRACE_AUTH_MODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// Default — operator tokens are required on all query routes.
    Enabled,
    /// DEV-ONLY. Bypasses extractor; injects synthetic principal.
    Disabled,
}

impl AuthMode {
    pub fn from_env_str(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("disabled") | Some("off") | Some("false") => AuthMode::Disabled,
            _ => AuthMode::Enabled,
        }
    }
}

/// Subset of Entra config needed to validate inbound JWTs. Populated from env
/// in [`crate::config::Config::from_env`].
#[derive(Debug, Clone)]
pub struct EntraConfig {
    /// Azure AD tenant id (GUID). Used to assemble the expected `iss`.
    pub tenant_id: String,
    /// JWT `aud` value our API expects. Conventionally `api://<api-client-id>`.
    pub audience: String,
    /// JWKS discovery URI. Conventionally
    /// `https://login.microsoftonline.com/<tenant-id>/discovery/v2.0/keys`.
    pub jwks_uri: String,
}

impl EntraConfig {
    /// Expected `iss` claim value — `https://login.microsoftonline.com/<tenant>/v2.0`.
    pub fn expected_issuer(&self) -> String {
        format!("https://login.microsoftonline.com/{}/v2.0", self.tenant_id)
    }
}

/// Bundle of auth-related state injected into `AppState`.
#[derive(Clone)]
pub struct AuthState {
    pub mode: AuthMode,
    /// `None` iff `mode == Disabled` and no Entra config was supplied.
    pub entra: Option<EntraConfig>,
    pub jwks: Arc<JwksCache>,
}

// ---------------------------------------------------------------------------
// JWKS cache
// ---------------------------------------------------------------------------

/// In-memory JWKS cache keyed by `kid`. Read-heavy via `RwLock`; the refresh
/// path takes the write lock + a separate `last_refresh` mutex to avoid a
/// thundering-herd of concurrent HTTP fetches on a cache miss.
pub struct JwksCache {
    keys: RwLock<HashMap<String, Arc<RS256PublicKey>>>,
    last_refresh: Mutex<Option<Instant>>,
    ttl: Duration,
    jwks_uri: String,
    http: reqwest::Client,
}

impl JwksCache {
    pub fn new(jwks_uri: String) -> Self {
        Self::with_ttl(jwks_uri, DEFAULT_JWKS_TTL)
    }

    pub fn with_ttl(jwks_uri: String, ttl: Duration) -> Self {
        Self {
            keys: RwLock::new(HashMap::new()),
            last_refresh: Mutex::new(None),
            ttl,
            jwks_uri,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client builds with default config"),
        }
    }

    /// Get a key for `kid`, refreshing from the JWKS URI if missing or
    /// stale. On fetch failure, returns whatever is currently cached and logs
    /// a warning — this keeps query routes responsive through brief blips.
    pub async fn get(&self, kid: &str) -> Option<Arc<RS256PublicKey>> {
        if let Some(k) = self.lookup(kid) {
            if !self.is_stale() {
                return Some(k);
            }
        }
        // Either miss, or stale — try a refresh.
        if let Err(err) = self.refresh().await {
            warn!(error = %err, "JWKS refresh failed; using stale cache");
        }
        self.lookup(kid)
    }

    /// Insert a key directly — used by tests to stash a hand-minted pubkey
    /// without hitting the network.
    pub fn insert_for_test(&self, kid: impl Into<String>, key: RS256PublicKey) {
        self.keys.write().insert(kid.into(), Arc::new(key));
        *self.last_refresh.lock() = Some(Instant::now());
    }

    fn lookup(&self, kid: &str) -> Option<Arc<RS256PublicKey>> {
        self.keys.read().get(kid).cloned()
    }

    fn is_stale(&self) -> bool {
        match *self.last_refresh.lock() {
            None => true,
            Some(t) => t.elapsed() >= self.ttl,
        }
    }

    /// Fetch + replace the cache. Public so tests can drive it with a
    /// mocked HTTP endpoint; production callers go through [`Self::get`].
    pub async fn refresh(&self) -> Result<(), JwksError> {
        let body = self
            .http
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(|e| JwksError::Fetch(e.to_string()))?
            .error_for_status()
            .map_err(|e| JwksError::Fetch(e.to_string()))?
            .text()
            .await
            .map_err(|e| JwksError::Fetch(e.to_string()))?;

        let doc: JwksDoc =
            serde_json::from_str(&body).map_err(|e| JwksError::Parse(e.to_string()))?;
        let mut next = HashMap::with_capacity(doc.keys.len());
        for jwk in doc.keys {
            // Ignore non-RSA-signing keys; Entra currently only publishes
            // `RS256` signing keys under this endpoint but the doc schema
            // allows `enc` too, which we reject by filter.
            if jwk.kty != "RSA" {
                continue;
            }
            if jwk.use_.as_deref().is_some_and(|u| u != "sig") {
                continue;
            }
            let n = decode_b64url(&jwk.n).map_err(JwksError::Parse)?;
            let e = decode_b64url(&jwk.e).map_err(JwksError::Parse)?;
            let pk = RS256PublicKey::from_components(&n, &e)
                .map_err(|err| JwksError::Parse(err.to_string()))?;
            next.insert(jwk.kid, Arc::new(pk));
        }
        debug!(n = next.len(), "JWKS cache refreshed");
        *self.keys.write() = next;
        *self.last_refresh.lock() = Some(Instant::now());
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JwksError {
    #[error("JWKS fetch: {0}")]
    Fetch(String),
    #[error("JWKS parse: {0}")]
    Parse(String),
}

#[derive(Deserialize)]
struct JwksDoc {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    kid: String,
    #[serde(default, rename = "use")]
    use_: Option<String>,
    n: String,
    e: String,
}

fn decode_b64url(s: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|e| format!("invalid base64url: {e}"))
}

// ---------------------------------------------------------------------------
// Principal
// ---------------------------------------------------------------------------

/// An authenticated operator. Populated from validated-JWT claims.
#[derive(Debug, Clone, Serialize)]
pub struct OperatorPrincipal {
    /// `sub` — stable per-user identifier inside the tenant.
    pub subject: String,
    /// `name` — human-readable display name (best-effort; optional).
    pub name: Option<String>,
    /// `tid` — Entra tenant id as reported in the token. Logged on every
    /// request so cross-tenant tokens are trivially observable.
    pub tenant_id: String,
    /// Parsed `scp` (space-delimited) + `roles` array. Deduplicated.
    pub scopes: Vec<String>,
}

impl OperatorPrincipal {
    /// Synthetic principal used under `CMTRACE_AUTH_MODE=disabled`.
    pub fn dev_bypass() -> Self {
        Self {
            subject: "dev".to_string(),
            name: Some("dev-bypass".to_string()),
            tenant_id: "dev".to_string(),
            scopes: vec![REQUIRED_SCOPE.to_string()],
        }
    }
}

// Custom-claims struct we hand `jwt-simple` for deserialization.
#[derive(Debug, Deserialize, Serialize)]
struct EntraCustomClaims {
    #[serde(default)]
    scp: Option<String>,
    #[serde(default)]
    roles: Option<Vec<String>>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tid: Option<String>,
}

// ---------------------------------------------------------------------------
// Error / response
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing Authorization header")]
    MissingHeader,
    #[error("malformed Authorization header")]
    MalformedHeader,
    #[error("token missing kid")]
    MissingKid,
    #[error("unknown signing key")]
    UnknownKid,
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("insufficient scope: requires '{REQUIRED_SCOPE}'")]
    InsufficientScope,
    #[error("auth not configured")]
    NotConfigured,
}

impl AuthError {
    fn description(&self) -> &'static str {
        match self {
            AuthError::MissingHeader => "missing Authorization header",
            AuthError::MalformedHeader => "malformed Authorization header",
            AuthError::MissingKid => "token missing kid",
            AuthError::UnknownKid => "unknown signing key",
            AuthError::InvalidToken(_) => "token validation failed",
            AuthError::InsufficientScope => "insufficient scope",
            AuthError::NotConfigured => "server auth misconfigured",
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let status = match self {
            AuthError::NotConfigured => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::UNAUTHORIZED,
        };
        let challenge = format!(
            "Bearer error=\"invalid_token\", error_description=\"{}\"",
            self.description()
        );
        let body = Json(serde_json::json!({
            "error": "unauthorized",
            "message": self.to_string(),
        }));
        let mut resp = (status, body).into_response();
        if let Ok(val) = challenge.parse() {
            resp.headers_mut().insert(WWW_AUTHENTICATE, val);
        }
        resp
    }
}

// ---------------------------------------------------------------------------
// Token verification
// ---------------------------------------------------------------------------

/// Core validation routine. Split out of the extractor so unit tests can
/// exercise it without spinning a Tokio runtime + Axum request.
pub async fn validate_bearer(
    token: &str,
    entra: &EntraConfig,
    jwks: &JwksCache,
) -> Result<OperatorPrincipal, AuthError> {
    // Pull `kid` from the header *before* touching the signature so we can
    // point the cache at the right key.
    let metadata = Token::decode_metadata(token)
        .map_err(|e| AuthError::InvalidToken(format!("decode header: {e}")))?;
    let kid = metadata.key_id().ok_or(AuthError::MissingKid)?.to_string();
    let pk = jwks.get(&kid).await.ok_or(AuthError::UnknownKid)?;

    let mut opts = VerificationOptions::default();
    // jwt-simple's `VerificationOptions` expects `HashSet<String>` for both
    // allowed-issuer / allowed-audience sets. We build them inline rather
    // than pulling the prelude's anonymous `HashSetFromStringsT` trait.
    let mut issuers = HashSet::new();
    issuers.insert(entra.expected_issuer());
    let mut audiences = HashSet::new();
    audiences.insert(entra.audience.clone());
    opts.allowed_issuers = Some(issuers);
    opts.allowed_audiences = Some(audiences);
    opts.required_key_id = Some(kid.clone());

    let claims = pk
        .verify_token::<EntraCustomClaims>(token, Some(opts))
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

    // Scope check: `scp` (space-delimited) OR `roles` array must include
    // REQUIRED_SCOPE.
    let mut scopes: Vec<String> = Vec::new();
    if let Some(scp) = claims.custom.scp.as_deref() {
        for s in scp.split_ascii_whitespace() {
            scopes.push(s.to_string());
        }
    }
    if let Some(roles) = claims.custom.roles.as_ref() {
        for r in roles {
            scopes.push(r.clone());
        }
    }
    scopes.sort();
    scopes.dedup();
    if !scopes.iter().any(|s| s == REQUIRED_SCOPE) {
        return Err(AuthError::InsufficientScope);
    }

    Ok(OperatorPrincipal {
        subject: claims.subject.unwrap_or_default(),
        name: claims.custom.name,
        tenant_id: claims.custom.tid.unwrap_or_default(),
        scopes,
    })
}

// ---------------------------------------------------------------------------
// Extractor
// ---------------------------------------------------------------------------

impl<S> FromRequestParts<S> for OperatorPrincipal
where
    S: Send + Sync,
    Arc<crate::state::AppState>: axum::extract::FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state: Arc<crate::state::AppState> = axum::extract::FromRef::from_ref(state);
        let auth = &app_state.auth;

        if auth.mode == AuthMode::Disabled {
            debug!("auth bypassed (CMTRACE_AUTH_MODE=disabled)");
            return Ok(OperatorPrincipal::dev_bypass());
        }

        let entra = auth
            .entra
            .as_ref()
            .ok_or_else(|| AuthError::NotConfigured.into_response())?;

        let token = parts
            .headers
            .get(AUTHORIZATION)
            .ok_or_else(|| AuthError::MissingHeader.into_response())?
            .to_str()
            .map_err(|_| AuthError::MalformedHeader.into_response())?
            .strip_prefix("Bearer ")
            .ok_or_else(|| AuthError::MalformedHeader.into_response())?
            .trim()
            .to_string();

        validate_bearer(&token, entra, &auth.jwks)
            .await
            .map_err(|e| e.into_response())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // Tests sign test-JWTs with jwt-simple's pure-rust RSA impl. The prelude
    // import is scoped to the `tests` module so production code keeps its
    // explicit-imports-only hygiene.
    use jwt_simple::prelude::{
        Claims, Duration as JwtDuration, RS256KeyPair, RSAKeyPairLike, UnixTimeStamp,
    };

    fn test_entra() -> EntraConfig {
        EntraConfig {
            tenant_id: "00000000-0000-0000-0000-000000000000".to_string(),
            audience: "api://cmtraceopen-test".to_string(),
            jwks_uri: "https://example.invalid/discovery/v2.0/keys".to_string(),
        }
    }

    fn mint_token(
        kp: &RS256KeyPair,
        kid: &str,
        iss: &str,
        aud: &str,
        scp: Option<&str>,
        expired: bool,
    ) -> String {
        let custom = EntraCustomClaims {
            scp: scp.map(str::to_string),
            roles: None,
            name: Some("Test Operator".to_string()),
            tid: Some("00000000-0000-0000-0000-000000000000".to_string()),
        };
        // A past exp is achieved by creating short-lived claims then
        // overwriting `expires_at` / `issued_at` to the past.
        let mut claims = Claims::with_custom_claims(custom, JwtDuration::from_secs(300))
            .with_issuer(iss)
            .with_audience(aud)
            .with_subject("alice@example.com");
        if expired {
            let past = UnixTimeStamp::from_secs(1);
            claims.issued_at = Some(past);
            claims.expires_at = Some(past);
            claims.invalid_before = Some(past);
        }
        let kp_with_kid = kp.clone().with_key_id(kid);
        kp_with_kid.sign(claims).expect("sign")
    }

    #[tokio::test]
    async fn happy_path_accepts_signed_token() {
        let kp = RS256KeyPair::generate(2048).unwrap();
        let entra = test_entra();
        let jwks = JwksCache::new(entra.jwks_uri.clone());
        jwks.insert_for_test("k1", kp.public_key());

        let token = mint_token(
            &kp,
            "k1",
            &entra.expected_issuer(),
            &entra.audience,
            Some("CmtraceOpen.Query openid profile"),
            false,
        );

        let principal = validate_bearer(&token, &entra, &jwks).await.unwrap();
        assert_eq!(principal.subject, "alice@example.com");
        assert!(principal.scopes.iter().any(|s| s == REQUIRED_SCOPE));
        assert_eq!(principal.name.as_deref(), Some("Test Operator"));
    }

    #[tokio::test]
    async fn rejects_aud_mismatch() {
        let kp = RS256KeyPair::generate(2048).unwrap();
        let entra = test_entra();
        let jwks = JwksCache::new(entra.jwks_uri.clone());
        jwks.insert_for_test("k1", kp.public_key());

        let token = mint_token(
            &kp,
            "k1",
            &entra.expected_issuer(),
            "api://some-other-app",
            Some(REQUIRED_SCOPE),
            false,
        );

        let err = validate_bearer(&token, &entra, &jwks).await.unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn rejects_expired_token() {
        let kp = RS256KeyPair::generate(2048).unwrap();
        let entra = test_entra();
        let jwks = JwksCache::new(entra.jwks_uri.clone());
        jwks.insert_for_test("k1", kp.public_key());

        let token = mint_token(
            &kp,
            "k1",
            &entra.expected_issuer(),
            &entra.audience,
            Some(REQUIRED_SCOPE),
            true,
        );

        let err = validate_bearer(&token, &entra, &jwks).await.unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn rejects_missing_scope() {
        let kp = RS256KeyPair::generate(2048).unwrap();
        let entra = test_entra();
        let jwks = JwksCache::new(entra.jwks_uri.clone());
        jwks.insert_for_test("k1", kp.public_key());

        let token = mint_token(
            &kp,
            "k1",
            &entra.expected_issuer(),
            &entra.audience,
            // `openid profile` is present but `CmtraceOpen.Query` isn't.
            Some("openid profile"),
            false,
        );

        let err = validate_bearer(&token, &entra, &jwks).await.unwrap_err();
        assert!(matches!(err, AuthError::InsufficientScope), "got {err:?}");
    }

    #[tokio::test]
    async fn cache_miss_triggers_refresh_lookup() {
        // Covers the "new kid appears" branch of JwksCache::get. We can't
        // actually hit the network under test, so we assert the code path:
        //  - empty cache
        //  - `get("unknown")` returns None (refresh fails, network unreachable).
        let jwks = JwksCache::with_ttl(
            "http://127.0.0.1:1/nope".to_string(),
            Duration::from_millis(1),
        );
        assert!(jwks.get("never-seen-before").await.is_none());
        // Now seed and confirm lookup hits without another refresh attempt
        // bringing down the cache (this exercises the stale + present branch).
        let kp = RS256KeyPair::generate(2048).unwrap();
        jwks.insert_for_test("k1", kp.public_key());
        assert!(jwks.get("k1").await.is_some());
    }

    #[test]
    fn auth_mode_parses_disabled_variants() {
        assert_eq!(AuthMode::from_env_str(Some("disabled")), AuthMode::Disabled);
        assert_eq!(AuthMode::from_env_str(Some("OFF")), AuthMode::Disabled);
        assert_eq!(AuthMode::from_env_str(Some("false")), AuthMode::Disabled);
        assert_eq!(AuthMode::from_env_str(Some("enabled")), AuthMode::Enabled);
        assert_eq!(AuthMode::from_env_str(None), AuthMode::Enabled);
    }

    #[test]
    fn expected_issuer_uses_v2_path() {
        let cfg = test_entra();
        assert_eq!(
            cfg.expected_issuer(),
            "https://login.microsoftonline.com/00000000-0000-0000-0000-000000000000/v2.0"
        );
    }
}
