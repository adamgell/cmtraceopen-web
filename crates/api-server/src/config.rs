use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::auth::{AuthMode, EntraConfig};

/// Runtime configuration, populated from environment variables.
///
/// All variables use the `CMTRACE_` prefix so they're easy to spot in a
/// `docker-compose` env block or a systemd unit.
#[derive(Debug, Clone)]
pub struct Config {
    /// Socket address to bind the HTTP listener to. Env: `CMTRACE_LISTEN_ADDR`.
    /// Default: `0.0.0.0:8080`.
    pub listen_addr: SocketAddr,

    /// Root directory for blob staging + finalized blobs. Env:
    /// `CMTRACE_DATA_DIR`. Default: `./data`.
    pub data_dir: PathBuf,

    /// SQLite DB path (file or `:memory:`). Env: `CMTRACE_SQLITE_PATH`.
    /// Default: `<data_dir>/meta.sqlite`.
    pub sqlite_path: String,

    /// Auth enforcement mode. Env: `CMTRACE_AUTH_MODE` (`enabled` | `disabled`).
    /// Default: `enabled`.
    ///
    /// `disabled` is DEV-ONLY — it bypasses the operator-bearer extractor
    /// and injects a synthetic principal. See [`crate::auth`] for details.
    pub auth_mode: AuthMode,

    /// Entra (Azure AD) config for operator bearer-token validation. `None`
    /// is only legal when `auth_mode == Disabled` (local dev).
    pub entra: Option<EntraConfig>,

    /// Exact origins permitted to call the API from a browser context. Env:
    /// `CMTRACE_CORS_ORIGINS`, comma-separated (e.g.
    /// `http://localhost:5173,http://localhost:4173`). Default: empty, which
    /// means the CORS layer rejects every cross-origin request (fail closed).
    ///
    /// Typical dev values:
    /// - `http://localhost:5173` — Vite dev server
    /// - `http://localhost:4173` — Vite preview server
    ///
    /// Prod deployments that serve the viewer same-origin can leave this
    /// empty; set it to the viewer's public origin only if the viewer lives
    /// on a different host/port.
    pub allowed_origins: Vec<String>,

    /// Whether browsers may include credentials (cookies, `Authorization`
    /// headers set via `fetch({ credentials: "include" })`) on cross-origin
    /// requests. Env: `CMTRACE_CORS_CREDENTIALS`, default `false`.
    ///
    /// Per the CORS spec, when this is `true` the server MUST echo an exact
    /// origin (not a `*` wildcard) in `Access-Control-Allow-Origin`. We always
    /// use an exact-list `AllowOrigin::list(...)`, so this constraint is
    /// already satisfied — but we document it here so the config surface is
    /// self-explanatory.
    pub allow_credentials: bool,

    /// TLS termination + mTLS client-cert verification. See [`TlsConfig`] for
    /// the full env-var contract.
    pub tls: TlsConfig,

    /// CRL distribution points to poll for client-cert revocation. Env:
    /// `CMTRACE_CRL_URLS`, comma-separated. Default: empty (no polling, no
    /// revocation enforcement).
    ///
    /// For the live Gell Cloud PKI tenant (see
    /// `~/.claude/projects/F--Repo/memory/reference_cloud_pki.md`) both CRLs
    /// must be polled — the Issuing CA's CRL covers the leaf certs the
    /// agents present, while the Root CA's CRL covers the Issuing CA itself
    /// (in case it ever has to be cross-revoked):
    ///
    /// - Root CA:    `http://primary-cdn.pki.azure.net/centralus/crls/9a8a2d279a7243fc96a508cbfca8f5d0/ad11b686-5970-42de-9827-91700269875b_v1/current.crl`
    /// - Issuing CA: `http://primary-cdn.pki.azure.net/centralus/crls/9a8a2d279a7243fc96a508cbfca8f5d0/7ff044a8-9c28-4529-9d79-76bdb94df99d_v1/current.crl`
    ///
    /// Both URLs are public HTTP — no auth, no client cert needed to fetch.
    pub crl_urls: Vec<String>,

    /// Interval between CRL refresh polls, in seconds. Env:
    /// `CMTRACE_CRL_REFRESH_SECS`, default `3600` (1 hour). Matches the
    /// Cloud PKI publishing cadence; tightening below 5 minutes risks
    /// hammering the CDN.
    pub crl_refresh_secs: u64,

    /// Behaviour when CRL fetch / parse fails and no cached CRL is available
    /// yet. Env: `CMTRACE_CRL_FAIL_OPEN`, default `false`.
    ///
    /// **Trade-off**:
    /// - `false` (fail-closed, default) — the server rejects every cert
    ///   lookup until at least one successful CRL fetch lands. This is the
    ///   secure default: a network outage during startup must not let
    ///   freshly-revoked credentials slip through. The cost is that
    ///   api-server cannot serve mTLS-gated traffic until the CRL CDN is
    ///   reachable.
    /// - `true` (fail-open) — the server accepts certs whose revocation
    ///   status is unknown. Useful for air-gapped / offline lab deployments
    ///   where reaching `primary-cdn.pki.azure.net` is infeasible, at the
    ///   cost of letting any not-yet-expired revoked cert authenticate.
    ///   Never flip this on without a compensating control (short-lived
    ///   leaf cert TTL, allow-listed device IDs, etc.).
    pub crl_fail_open: bool,
}

/// TLS-termination + mTLS client-cert verification. Populated from
/// `CMTRACE_TLS_*` and `CMTRACE_CLIENT_CA_BUNDLE`.
///
/// All fields are present on every build, but the `mtls` Cargo feature
/// controls whether the server binary can actually act on them. With the
/// feature off, [`Config::from_env`] forces `enabled = false` even if
/// `CMTRACE_TLS_ENABLED=true` was set, and emits a tracing warning at
/// startup (logged from `main.rs`, not here, since the config layer must
/// stay free of side effects).
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Master switch. Env: `CMTRACE_TLS_ENABLED`, default `false` so the
    /// existing plaintext dev path stays the no-config path.
    pub enabled: bool,

    /// Server's own cert chain in PEM format (the listener's identity).
    /// Env: `CMTRACE_TLS_CERT`. Required when `enabled`.
    pub server_cert_pem: Option<PathBuf>,

    /// Server's private key in PEM format. Env: `CMTRACE_TLS_KEY`.
    /// Required when `enabled`.
    pub server_key_pem: Option<PathBuf>,

    /// PEM bundle of trust anchors for client certs (Gell - PKI Root +
    /// Issuing per the runbook in `docs/provisioning/03-intune-cloud-pki.md`).
    /// Env: `CMTRACE_CLIENT_CA_BUNDLE`. Required when `enabled`.
    pub client_ca_bundle: Option<PathBuf>,

    /// Whether to require a valid client cert on ingest routes. Env:
    /// `CMTRACE_MTLS_REQUIRE_INGEST`. Defaults to `true` when [`Self::enabled`]
    /// is true, otherwise `false`.
    ///
    /// `true`: ingest 401s without a cert (closed). `false`: ingest still
    /// works without a client cert, falling back to the legacy `X-Device-Id`
    /// header (transitional path while devices roll over to PKCS-issued
    /// certs).
    pub require_on_ingest: bool,

    /// Expected URI scheme in the client cert SAN. Env:
    /// `CMTRACE_SAN_URI_SCHEME`, default `device`. The full SAN URI shape
    /// is `device://{tenant-id}/{aad-device-id}` per the Intune PKCS profile.
    pub expected_san_uri_scheme: String,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_cert_pem: None,
            server_key_pem: None,
            client_ca_bundle: None,
            require_on_ingest: false,
            expected_san_uri_scheme: "device".to_string(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid CMTRACE_LISTEN_ADDR: {0}")]
    InvalidListenAddr(String),

    #[error(
        "auth enabled but Entra config incomplete: set CMTRACE_ENTRA_TENANT_ID, \
         CMTRACE_ENTRA_AUDIENCE, and CMTRACE_ENTRA_JWKS_URI, or set \
         CMTRACE_AUTH_MODE=disabled for local dev"
    )]
    MissingEntraConfig,

    #[error("invalid CMTRACE_CORS_CREDENTIALS: {0} (expected true/false/1/0)")]
    InvalidCorsCredentials(String),

    #[error("invalid CMTRACE_TLS_ENABLED: {0} (expected true/false/1/0)")]
    InvalidTlsEnabled(String),

    #[error("invalid CMTRACE_MTLS_REQUIRE_INGEST: {0} (expected true/false/1/0)")]
    InvalidMtlsRequireIngest(String),

    #[error(
        "CMTRACE_TLS_ENABLED=true but {0} is not set; mTLS bring-up needs the \
         server cert, key, and client-CA bundle paths"
    )]
    MissingTlsPath(&'static str),

    #[error("invalid CMTRACE_CRL_REFRESH_SECS: {0} (expected positive integer)")]
    InvalidCrlRefreshSecs(String),

    #[error("invalid CMTRACE_CRL_FAIL_OPEN: {0} (expected true/false/1/0)")]
    InvalidCrlFailOpen(String),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let listen_addr = match env::var("CMTRACE_LISTEN_ADDR") {
            Ok(value) => value
                .parse()
                .map_err(|_| ConfigError::InvalidListenAddr(value))?,
            Err(_) => "0.0.0.0:8080".parse().expect("static default parses"),
        };

        let data_dir = env::var("CMTRACE_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./data"));

        let sqlite_path = env::var("CMTRACE_SQLITE_PATH").unwrap_or_else(|_| {
            data_dir
                .join("meta.sqlite")
                .to_string_lossy()
                .to_string()
        });

        let auth_mode = AuthMode::from_env_str(env::var("CMTRACE_AUTH_MODE").ok().as_deref());

        // Entra config is optional iff auth is disabled. Partial config is
        // always rejected — "only two of three set" is almost always a typo.
        let tenant = env::var("CMTRACE_ENTRA_TENANT_ID").ok();
        let audience = env::var("CMTRACE_ENTRA_AUDIENCE").ok();
        let jwks_uri = env::var("CMTRACE_ENTRA_JWKS_URI").ok();
        let entra = match (tenant, audience, jwks_uri) {
            (Some(t), Some(a), Some(j)) if !t.is_empty() && !a.is_empty() && !j.is_empty() => {
                Some(EntraConfig {
                    tenant_id: t,
                    audience: a,
                    jwks_uri: j,
                })
            }
            (None, None, None) => None,
            _ => return Err(ConfigError::MissingEntraConfig),
        };

        if matches!(auth_mode, AuthMode::Enabled) && entra.is_none() {
            return Err(ConfigError::MissingEntraConfig);
        }

        let allowed_origins = env::var("CMTRACE_CORS_ORIGINS")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let allow_credentials = match env::var("CMTRACE_CORS_CREDENTIALS") {
            Ok(v) => parse_bool(&v).ok_or(ConfigError::InvalidCorsCredentials(v))?,
            Err(_) => false,
        };

        let tls = TlsConfig::from_env()?;

        let crl_urls = env::var("CMTRACE_CRL_URLS")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let crl_refresh_secs = match env::var("CMTRACE_CRL_REFRESH_SECS") {
            Ok(v) => v
                .parse::<u64>()
                .ok()
                .filter(|&n| n > 0)
                .ok_or(ConfigError::InvalidCrlRefreshSecs(v))?,
            Err(_) => 3600,
        };

        let crl_fail_open = match env::var("CMTRACE_CRL_FAIL_OPEN") {
            Ok(v) => parse_bool(&v).ok_or(ConfigError::InvalidCrlFailOpen(v))?,
            Err(_) => false,
        };

        Ok(Self {
            listen_addr,
            data_dir,
            sqlite_path,
            auth_mode,
            entra,
            allowed_origins,
            allow_credentials,
            tls,
            crl_urls,
            crl_refresh_secs,
            crl_fail_open,
        })
    }
}

impl TlsConfig {
    /// Read `CMTRACE_TLS_*` + related env vars. Validates that when TLS is
    /// enabled all three required paths are set; doesn't open the files
    /// (that happens in `main.rs` so failures show up in the startup log
    /// alongside the bind-port error path).
    pub fn from_env() -> Result<Self, ConfigError> {
        let enabled = match env::var("CMTRACE_TLS_ENABLED") {
            Ok(v) => parse_bool(&v).ok_or(ConfigError::InvalidTlsEnabled(v))?,
            Err(_) => false,
        };

        let server_cert_pem = env::var("CMTRACE_TLS_CERT").ok().map(PathBuf::from);
        let server_key_pem = env::var("CMTRACE_TLS_KEY").ok().map(PathBuf::from);
        let client_ca_bundle = env::var("CMTRACE_CLIENT_CA_BUNDLE")
            .ok()
            .map(PathBuf::from);

        // Default `require_on_ingest` mirrors `enabled`: if you've turned
        // TLS on you almost certainly want mTLS enforced on the ingest
        // surface too. Override via env when rolling over from header-based
        // identity (transitional).
        let require_on_ingest = match env::var("CMTRACE_MTLS_REQUIRE_INGEST") {
            Ok(v) => parse_bool(&v).ok_or(ConfigError::InvalidMtlsRequireIngest(v))?,
            Err(_) => enabled,
        };

        let expected_san_uri_scheme = env::var("CMTRACE_SAN_URI_SCHEME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "device".to_string());

        if enabled {
            if server_cert_pem.is_none() {
                return Err(ConfigError::MissingTlsPath("CMTRACE_TLS_CERT"));
            }
            if server_key_pem.is_none() {
                return Err(ConfigError::MissingTlsPath("CMTRACE_TLS_KEY"));
            }
            if client_ca_bundle.is_none() {
                return Err(ConfigError::MissingTlsPath("CMTRACE_CLIENT_CA_BUNDLE"));
            }
        }

        Ok(Self {
            enabled,
            server_cert_pem,
            server_key_pem,
            client_ca_bundle,
            require_on_ingest,
            expected_san_uri_scheme,
        })
    }
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}
