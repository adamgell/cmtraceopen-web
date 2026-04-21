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
use std::path::Path;

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
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            api_endpoint: String::from("https://api.corp.example.com"),
            request_timeout_secs: 60,
            evidence_schedule: String::from("0 3 * * *"),
            queue_max_bundles: 50,
            log_level: String::from("info"),
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
            if let Ok(parsed) = v.parse::<u64>() {
                cfg.request_timeout_secs = parsed;
            }
        }
        if let Ok(v) = std::env::var("CMTRACE_EVIDENCE_SCHEDULE") {
            cfg.evidence_schedule = v;
        }
        if let Ok(v) = std::env::var("CMTRACE_QUEUE_MAX_BUNDLES") {
            if let Ok(parsed) = v.parse::<usize>() {
                cfg.queue_max_bundles = parsed;
            }
        }
        if let Ok(v) = std::env::var("CMTRACE_LOG_LEVEL") {
            cfg.log_level = v;
        }

        cfg
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
        matches!(err, ConfigError::Io { .. });
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
