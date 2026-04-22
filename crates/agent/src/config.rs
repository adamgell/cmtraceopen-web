//! Agent runtime configuration.
//!
//! Two population paths are supported today:
//!
//!   * [`AgentConfig::from_file`] — TOML file, typically at
//!     `%ProgramData%\CMTraceOpen\Agent\config.toml` on Windows. The caller
//!     passes the path so unit tests / alternate deploys can point elsewhere.
//!   * [`AgentConfig::from_env_or_default`] — per-field `CMTRACE_*` env-var
//!     overrides on top of [`AgentConfig::default`]. Handy for local dev
//!     and for MSI-deployed fleets that want to override a single field
//!     (e.g. `CMTRACE_API_ENDPOINT`) without shipping a full TOML.
//!
//! HKLM registry overrides and ADMX policy surfaces are planned but
//! explicitly out of scope for the scaffold — see the TODO comments below.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Agent configuration. Mirrors the layout in the project plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Base URL of the api-server, e.g. `https://api.corp.example.com`.
    /// No trailing slash.
    pub api_endpoint: String,

    /// HTTP request timeout, in seconds. Applies to uploads and control
    /// plane calls alike; individual collectors may stack their own.
    pub request_timeout_secs: u64,

    /// Cron-like schedule for the evidence collector, e.g. `"0 3 * * *"`.
    /// Evaluated by the scheduler (not wired up yet).
    pub evidence_schedule: String,

    /// Maximum number of bundles the upload queue will hold on disk before
    /// it starts dropping the oldest to make room.
    pub queue_max_bundles: usize,

    /// `tracing` filter directive. Accepts anything `EnvFilter::new` takes,
    /// but most deployments will just set `"info"` / `"debug"`.
    pub log_level: String,

    /// Device identity string sent in the `X-Device-Id` header until mTLS
    /// lands (Wave 3). Empty string means "fall back to hostname" in the
    /// runtime (see `main.rs`); this lets tests override without changing
    /// machine hostname.
    #[serde(default)]
    pub device_id: String,

    /// Directories the `logs` collector walks for `.log` / `.txt` files.
    /// Defaults cover the ConfigMgr + Intune + Entra-join log trees.
    #[serde(default = "default_log_paths")]
    pub log_paths: Vec<String>,

    /// Path to a PEM-encoded client certificate (chain) that the agent
    /// presents to the api-server during the TLS handshake. Wave 3 will
    /// flip server-side enforcement on; today this is loaded if both
    /// this and [`Self::tls_client_key_pem`] are set, but the server
    /// doesn't reject a missing cert. Leave `None` to skip client
    /// auth entirely.
    #[serde(default)]
    pub tls_client_cert_pem: Option<PathBuf>,

    /// Path to the PEM-encoded private key matching
    /// [`Self::tls_client_cert_pem`]. PKCS#8 / SEC1 / RSA PKCS#1 are all
    /// accepted (rustls-pemfile decides). Required when
    /// `tls_client_cert_pem` is set; ignored otherwise.
    #[serde(default)]
    pub tls_client_key_pem: Option<PathBuf>,

    /// Optional PEM bundle of additional trusted root CAs. When set, the
    /// agent uses **only** these roots — the OS native trust store is
    /// *not* layered on top. Leave `None` to use the OS native roots
    /// (the common case for fleets that trust a public CA).
    #[serde(default)]
    pub tls_ca_bundle_pem: Option<PathBuf>,
}

fn default_log_paths() -> Vec<String> {
    vec![
        // ConfigMgr client logs.
        "C:\\Windows\\CCM\\Logs\\**\\*.log".into(),
        // Intune Management Extension.
        "C:\\ProgramData\\Microsoft\\IntuneManagementExtension\\Logs\\**\\*.log".into(),
        // DSRegCmd / Entra-join diagnostics drop here.
        "C:\\Windows\\Logs\\DSRegCmd\\**\\*.log".into(),
    ]
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            api_endpoint: String::from("https://api.corp.example.com"),
            request_timeout_secs: 60,
            evidence_schedule: String::from("0 3 * * *"),
            queue_max_bundles: 50,
            log_level: String::from("info"),
            device_id: String::new(),
            log_paths: default_log_paths(),
            tls_client_cert_pem: None,
            tls_client_key_pem: None,
            tls_ca_bundle_pem: None,
        }
    }
}

/// Errors surfaced by [`AgentConfig::from_file`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
}

impl AgentConfig {
    /// Load config from a TOML file. Missing fields fall back to
    /// [`AgentConfig::default`] via `#[serde(default)]` on the struct.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.display().to_string(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.display().to_string(),
            source,
        })
    }

    /// Start from [`AgentConfig::default`] and apply per-field overrides from
    /// `CMTRACE_*` environment variables. Invalid numeric values are ignored
    /// (with the default kept) rather than failing startup — the agent needs
    /// to boot even if something upstream misconfigured one knob.
    ///
    /// TODO: once the service wrapper exists, also layer HKLM registry
    /// overrides (`HKLM\Software\CMTraceOpen\Agent`) between the file and
    /// env-var sources, per the plan.
    pub fn from_env_or_default() -> Self {
        let mut cfg = Self::default();

        if let Ok(v) = std::env::var("CMTRACE_API_ENDPOINT") {
            cfg.api_endpoint = v;
        }
        if let Ok(v) = std::env::var("CMTRACE_REQUEST_TIMEOUT_SECS") {
            match v.parse::<u64>() {
                Ok(parsed) => cfg.request_timeout_secs = parsed,
                // NOTE: `eprintln!` rather than `tracing::warn!` because this
                // runs BEFORE `tracing-subscriber` is installed in `main.rs`.
                // A tracing call here would be silently dropped; stderr is
                // guaranteed visible and keeps this function dependency-free.
                Err(e) => eprintln!(
                    "warning: CMTRACE_REQUEST_TIMEOUT_SECS={v:?} failed to parse ({e}); falling back to default {}",
                    cfg.request_timeout_secs
                ),
            }
        }
        if let Ok(v) = std::env::var("CMTRACE_EVIDENCE_SCHEDULE") {
            cfg.evidence_schedule = v;
        }
        if let Ok(v) = std::env::var("CMTRACE_QUEUE_MAX_BUNDLES") {
            match v.parse::<usize>() {
                Ok(parsed) => cfg.queue_max_bundles = parsed,
                // See note above re: `eprintln!` vs `tracing::warn!`.
                Err(e) => eprintln!(
                    "warning: CMTRACE_QUEUE_MAX_BUNDLES={v:?} failed to parse ({e}); falling back to default {}",
                    cfg.queue_max_bundles
                ),
            }
        }
        if let Ok(v) = std::env::var("CMTRACE_LOG_LEVEL") {
            cfg.log_level = v;
        }
        if let Ok(v) = std::env::var("CMTRACE_DEVICE_ID") {
            cfg.device_id = v;
        }
        if let Ok(v) = std::env::var("CMTRACE_TLS_CLIENT_CERT") {
            if !v.is_empty() {
                cfg.tls_client_cert_pem = Some(PathBuf::from(v));
            }
        }
        if let Ok(v) = std::env::var("CMTRACE_TLS_CLIENT_KEY") {
            if !v.is_empty() {
                cfg.tls_client_key_pem = Some(PathBuf::from(v));
            }
        }
        if let Ok(v) = std::env::var("CMTRACE_TLS_CA_BUNDLE") {
            if !v.is_empty() {
                cfg.tls_ca_bundle_pem = Some(PathBuf::from(v));
            }
        }

        cfg
    }

    /// Resolve the device identity to send on the wire.
    /// Precedence: explicit `device_id` config / env var → OS hostname → the
    /// string `"unknown-device"`. Wave 3 replaces this with an mTLS-derived
    /// identity; until then the `X-Device-Id` header is authoritative.
    pub fn resolved_device_id(&self) -> String {
        if !self.device_id.is_empty() {
            return self.device_id.clone();
        }
        // `hostname` isn't in std; call the platform's env shim. Both
        // Windows (`COMPUTERNAME`) and Unix (`HOSTNAME`) usually export one;
        // we fall through to a constant rather than panicking.
        if let Ok(h) = std::env::var("COMPUTERNAME") {
            if !h.is_empty() {
                return h;
            }
        }
        if let Ok(h) = std::env::var("HOSTNAME") {
            if !h.is_empty() {
                return h;
            }
        }
        "unknown-device".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.request_timeout_secs, 60);
        assert_eq!(cfg.queue_max_bundles, 50);
        assert_eq!(cfg.log_level, "info");
        assert!(cfg.api_endpoint.starts_with("https://"));
        assert!(!cfg.evidence_schedule.is_empty());
        assert!(cfg.device_id.is_empty());
        assert!(!cfg.log_paths.is_empty());
        // TLS knobs default to "use OS native roots, no client cert".
        assert!(cfg.tls_client_cert_pem.is_none());
        assert!(cfg.tls_client_key_pem.is_none());
        assert!(cfg.tls_ca_bundle_pem.is_none());
    }

    #[test]
    fn resolved_device_id_prefers_explicit() {
        let cfg = AgentConfig {
            device_id: "WIN-UNIT-01".into(),
            ..AgentConfig::default()
        };
        assert_eq!(cfg.resolved_device_id(), "WIN-UNIT-01");
    }

    #[test]
    fn from_file_parses_partial_toml() {
        // Missing fields should fall through to Default via serde(default).
        let dir = tempdir();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            r#"
api_endpoint = "https://api.example.test"
queue_max_bundles = 7
"#,
        )
        .unwrap();

        let cfg = AgentConfig::from_file(&path).expect("parse");
        assert_eq!(cfg.api_endpoint, "https://api.example.test");
        assert_eq!(cfg.queue_max_bundles, 7);
        // Untouched fields still hold defaults.
        assert_eq!(cfg.request_timeout_secs, 60);
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn from_file_surfaces_missing_path() {
        let err = AgentConfig::from_file(Path::new("/definitely/not/here.toml"))
            .expect_err("missing file should error");
        assert!(
            matches!(err, ConfigError::Io { .. }),
            "expected ConfigError::Io, got {err:?}"
        );
    }

    /// Minimal ad-hoc temp dir; avoids pulling `tempfile` into deps just for
    /// the scaffold. Uses the process id + a nanosecond timestamp to keep
    /// parallel test runs from colliding.
    fn tempdir() -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "cmtraceopen-agent-test-{}-{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
