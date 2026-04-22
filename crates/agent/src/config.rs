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

/// A single PII-redaction rule.
///
/// `regex` is compiled once at agent startup (see [`crate::redact::Redactor`]).
/// `replacement` supports back-references (`$1`, `${name}`) as accepted by
/// the [`regex::Regex::replace_all`] replacement syntax.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedactionRule {
    /// Human-readable identifier, e.g. `"username_path"`. Used only in
    /// error messages and operator tooling output.
    pub name: String,
    /// ECMAScript-compatible regular expression. Compiled by the `regex`
    /// crate; Unicode enabled, case-sensitive by default.
    pub regex: String,
    /// Replacement string. May contain back-references (`$1`, `${name}`).
    pub replacement: String,
}

/// Redaction configuration table (`[redaction]` in TOML).
///
/// Default rules are baked into [`crate::redact::Redactor`]; the `patterns`
/// field here adds *extra* rules on top of the defaults. To suppress a
/// default rule, set `enabled = false` and re-add only the rules you want.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RedactionConfig {
    /// Master switch. When `false` the agent forwards raw collected text
    /// with no substitutions applied. Defaults to `true`.
    pub enabled: bool,
    /// Operator-supplied rules appended after the built-in defaults.
    /// An empty list keeps just the defaults (the common case).
    pub patterns: Vec<RedactionRule>,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            patterns: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Schedule config
// ---------------------------------------------------------------------------

/// How the collection scheduler decides when to fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleMode {
    /// Fire every `interval_hours` hours (default).
    #[default]
    Interval,
    /// Fire according to a 5-field cron expression in `cron_expr`.
    Cron,
    /// Never fire automatically. Intended for service deployments that
    /// trigger collection via an external mechanism (e.g., a management
    /// script calling `--oneshot`). The scheduler loop simply blocks on
    /// the stop signal.
    Manual,
}

/// Schedule configuration nested under `[collection.schedule]` in the TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleConfig {
    /// How the scheduler decides when to fire.
    pub mode: ScheduleMode,

    /// Hours between collections when `mode = "interval"`.
    pub interval_hours: u64,

    /// Standard 5-field cron expression (min hour dom mon dow) used when
    /// `mode = "cron"`. Evaluated in local time.
    pub cron_expr: String,

    /// Randomize the fire time within ±N minutes so that a fleet of 1000+
    /// devices doesn't hammer the server all at once. Applied to both
    /// interval and cron modes. Set to `0` to disable jitter.
    pub jitter_minutes: u64,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            mode: ScheduleMode::Interval,
            interval_hours: 24,
            cron_expr: String::from("0 3 * * *"),
            jitter_minutes: 30,
        }
    }
}

/// Wraps `ScheduleConfig` so it can be nested as `[collection.schedule]`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CollectionConfig {
    #[serde(default)]
    pub schedule: ScheduleConfig,
}

// ---------------------------------------------------------------------------
// Top-level agent config
// ---------------------------------------------------------------------------


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
    ///
    /// **Deprecated** — use `[collection.schedule]` instead. This field is
    /// kept for backward-compatibility and is ignored when `collection` is
    /// explicitly configured.
    pub evidence_schedule: String,

    /// Collection scheduler configuration (`[collection.schedule]` table).
    pub collection: CollectionConfig,

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

    /// PII-redaction settings. Applied to all text collector output before
    /// it is bundled. Binary files (`.evtx`, `.reg`) are not redacted in
    /// v1 — see `docs/wave4/14-redaction.md` for rationale and how to add
    /// custom rules.
    #[serde(default)]
    pub redaction: RedactionConfig,
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
            collection: CollectionConfig::default(),
            queue_max_bundles: 50,
            log_level: String::from("info"),
            device_id: String::new(),
            log_paths: default_log_paths(),
            tls_client_cert_pem: None,
            tls_client_key_pem: None,
            tls_ca_bundle_pem: None,
            redaction: RedactionConfig::default(),
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
        if let Ok(v) = std::env::var("CMTRACE_SCHEDULE_MODE") {
            match v.to_lowercase().as_str() {
                "interval" => cfg.collection.schedule.mode = ScheduleMode::Interval,
                "cron" => cfg.collection.schedule.mode = ScheduleMode::Cron,
                "manual" => cfg.collection.schedule.mode = ScheduleMode::Manual,
                _ => eprintln!(
                    "warning: CMTRACE_SCHEDULE_MODE={v:?} is not one of interval/cron/manual; \
                     falling back to default {:?}",
                    cfg.collection.schedule.mode,
                ),
            }
        }
        if let Ok(v) = std::env::var("CMTRACE_SCHEDULE_INTERVAL_HOURS") {
            match v.parse::<u64>() {
                Ok(parsed) => cfg.collection.schedule.interval_hours = parsed,
                Err(e) => eprintln!(
                    "warning: CMTRACE_SCHEDULE_INTERVAL_HOURS={v:?} failed to parse ({e}); \
                     falling back to default {}",
                    cfg.collection.schedule.interval_hours
                ),
            }
        }
        if let Ok(v) = std::env::var("CMTRACE_SCHEDULE_CRON_EXPR") {
            cfg.collection.schedule.cron_expr = v;
        }
        if let Ok(v) = std::env::var("CMTRACE_SCHEDULE_JITTER_MINUTES") {
            match v.parse::<u64>() {
                Ok(parsed) => cfg.collection.schedule.jitter_minutes = parsed,
                Err(e) => eprintln!(
                    "warning: CMTRACE_SCHEDULE_JITTER_MINUTES={v:?} failed to parse ({e}); \
                     falling back to default {}",
                    cfg.collection.schedule.jitter_minutes
                ),
            }
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
        // Schedule defaults.
        assert_eq!(cfg.collection.schedule.mode, ScheduleMode::Interval);
        assert_eq!(cfg.collection.schedule.interval_hours, 24);
        assert_eq!(cfg.collection.schedule.cron_expr, "0 3 * * *");
        assert_eq!(cfg.collection.schedule.jitter_minutes, 30);
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

    #[test]
    fn schedule_config_parses_from_toml() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            r#"
[collection.schedule]
mode = "cron"
cron_expr = "0 2 * * 1"
interval_hours = 12
jitter_minutes = 10
"#,
        )
        .unwrap();

        let cfg = AgentConfig::from_file(&path).expect("parse");
        assert_eq!(cfg.collection.schedule.mode, ScheduleMode::Cron);
        assert_eq!(cfg.collection.schedule.cron_expr, "0 2 * * 1");
        assert_eq!(cfg.collection.schedule.interval_hours, 12);
        assert_eq!(cfg.collection.schedule.jitter_minutes, 10);
    }

    #[test]
    fn schedule_config_defaults_when_omitted() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        std::fs::write(&path, r#"api_endpoint = "https://test.example.com""#).unwrap();

        let cfg = AgentConfig::from_file(&path).expect("parse");
        assert_eq!(cfg.collection.schedule.mode, ScheduleMode::Interval);
        assert_eq!(cfg.collection.schedule.interval_hours, 24);
        assert_eq!(cfg.collection.schedule.cron_expr, "0 3 * * *");
        assert_eq!(cfg.collection.schedule.jitter_minutes, 30);
    }

    #[test]
    fn schedule_manual_mode_parses() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            r#"
[collection.schedule]
mode = "manual"
"#,
        )
        .unwrap();

        let cfg = AgentConfig::from_file(&path).expect("parse");
        assert_eq!(cfg.collection.schedule.mode, ScheduleMode::Manual);
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
