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

        Ok(Self {
            listen_addr,
            data_dir,
            sqlite_path,
            auth_mode,
            entra,
        })
    }
}
