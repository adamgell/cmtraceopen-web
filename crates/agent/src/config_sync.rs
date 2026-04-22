//! Server-side config pull + safe-rollback (Wave 4).
//!
//! [`ConfigSync`] fetches [`AgentConfigOverride`] from the api-server at
//! startup, every [`CONFIG_FETCH_INTERVAL`] hours, and after every successful
//! upload.  Received overrides are **merged** on top of the agent's local
//! config (the base) — only the fields listed in `AgentConfigOverride` can
//! be changed; sensitive fields (`api_endpoint`, TLS paths) are immutable.
//!
//! ## Reload semantics — read this before pushing an override
//!
//! Today (v1) the agent applies overrides at **next startup**, not live.
//! `Uploader` and `EvidenceOrchestrator` are constructed once from the
//! initial `effective_config()` snapshot. Subsequent `sync()` calls update
//! `applied_override` (and persist it via the rollback clock + state file)
//! but do not rebuild those components. That means:
//!
//! * `request_timeout_secs`, `log_paths`, `evidence_schedule`,
//!   `queue_max_bundles`: **next restart only**.
//! * `log_level`: **next restart only** (the tracing subscriber is
//!   installed once at startup; mutating the filter would require an
//!   `arc-swap`'d `EnvFilter` and is intentionally out of scope for v1).
//!
//! Operators that need an immediate effect must restart the agent service
//! (`Restart-Service CMTraceOpenAgent`). True live reload is tracked as a
//! follow-up — see the issue board for the `config-push-live-reload` tag.
//!
//! ## Safe rollback
//!
//! If the agent cannot successfully connect / upload for
//! [`ROLLBACK_THRESHOLD`] after applying a remote override, it reverts to the
//! last-known-good local config.  This guards against a mis-configured
//! override (e.g. `requestTimeoutSecs: 0`) that would otherwise brick the
//! agent until the next MSI re-deploy.
//!
//! The rollback timer is wall-clock based and **persisted** to
//! `%ProgramData%\CMTraceOpen\Agent\config-state.json` so a crash-loop can't
//! reset the 24-hour clock and let a bricking override "win" the rollback
//! race forever. The first failure timestamp is loaded at construction; it
//! is cleared on every successful upload.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use common_wire::AgentConfigOverride;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::config::AgentConfig;

/// How long between periodic config-fetch attempts in daemon mode (base).
/// The actual interval per-device is `CONFIG_FETCH_INTERVAL +
/// device_jitter()` so a fleet that all booted at the same time doesn't
/// hit the endpoint simultaneously every six hours.
pub const CONFIG_FETCH_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// Maximum random offset added to [`CONFIG_FETCH_INTERVAL`]. Up to 30 min
/// of spread keeps a typical 1000-device fleet's config-fetch QPS smooth
/// without making the per-device interval visibly inconsistent to ops.
pub const CONFIG_FETCH_JITTER_MAX: Duration = Duration::from_secs(30 * 60);

/// If the agent has had zero successful uploads for this long since an
/// override was applied, the override is dropped and the local config is
/// restored.
pub const ROLLBACK_THRESHOLD: Duration = Duration::from_secs(24 * 3600);

/// On-disk state file persisted between agent restarts so the rollback
/// clock survives a crash-loop. Without this, a bricking override could
/// crash the agent every minute, reset `first_failure` on every restart,
/// and never trigger the safe-rollback.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedState {
    /// Wall-clock timestamp of the first failure since the current
    /// override was applied. Cleared on success and on rollback.
    first_failure_at: Option<DateTime<Utc>>,
}

/// Manages fetching a remote config override and merging it into the running
/// agent config.
pub struct ConfigSync {
    client: Client,
    api_endpoint: String,
    device_id: String,
    /// The baseline config loaded from disk / environment at startup.
    /// Used as the merge base and as the rollback target.
    ///
    /// **Stale-on-edit caveat:** this is a snapshot taken at agent
    /// startup. If an operator edits `config.toml` on disk and the agent
    /// is later restarted, the new `local_config` is whatever startup
    /// re-read. The agent does NOT pick up disk edits without a restart.
    local_config: AgentConfig,
    /// The current remote override, `None` if no override is active.
    applied_override: Option<AgentConfigOverride>,
    /// Wall-clock instant of the last successful upload.  `None` until the
    /// first success.
    last_success: Option<Instant>,
    /// Wall-clock instant of the first consecutive failure *after* the current
    /// override was applied.  Cleared on every success. Mirrored to
    /// [`ConfigSync::state_path`] so a crash-loop can't reset the clock.
    first_failure: Option<Instant>,
    /// Where `PersistedState` is mirrored. `None` disables persistence
    /// (used by unit tests so they don't touch the real `%ProgramData%`
    /// path on the test host).
    state_path: Option<PathBuf>,
}

impl ConfigSync {
    /// Create a new `ConfigSync` with state persistence enabled.
    ///
    /// `client` should be the same `reqwest::Client` the uploader uses so TLS
    /// settings (client cert, CA bundle) are consistent.
    ///
    /// The persisted-state file is loaded from
    /// `%ProgramData%\CMTraceOpen\Agent\config-state.json` on Windows (or
    /// `~/.cmtraceopen-agent/config-state.json` elsewhere). If the file
    /// is missing, malformed, or unreadable the agent starts fresh — the
    /// state file is best-effort, never required.
    pub fn new(client: Client, api_endpoint: String, device_id: String, local: AgentConfig) -> Self {
        Self::new_with_state_path(client, api_endpoint, device_id, local, Some(default_state_path()))
    }

    /// Same as [`ConfigSync::new`] but lets the caller override the
    /// state-file location (or pass `None` to disable persistence).
    /// Used by tests.
    pub fn new_with_state_path(
        client: Client,
        api_endpoint: String,
        device_id: String,
        local: AgentConfig,
        state_path: Option<PathBuf>,
    ) -> Self {
        // Load any previously-persisted failure clock so the 24h timer
        // survives a restart / crash-loop.
        let initial_failure = state_path
            .as_ref()
            .and_then(|p| load_persisted_state(p))
            .and_then(|s| s.first_failure_at)
            .and_then(|t| {
                // Convert chrono → Instant via SystemTime so the relative
                // age is preserved across the restart.
                let now_ut = Utc::now();
                let age = (now_ut - t).to_std().ok()?;
                Instant::now().checked_sub(age)
            });

        Self {
            client,
            api_endpoint,
            device_id,
            local_config: local,
            applied_override: None,
            last_success: None,
            first_failure: initial_failure,
            state_path,
        }
    }

    /// Returns the per-device config-fetch interval = base + deterministic
    /// jitter derived from a SHA-256 of the device id. Stable per-device:
    /// the same device always gets the same offset across restarts, so an
    /// operator following a single device's logs can predict the cadence.
    /// Different devices get different offsets, smoothing the fleet-wide
    /// QPS to the config endpoint.
    pub fn fetch_interval(&self) -> Duration {
        CONFIG_FETCH_INTERVAL + device_id_jitter(&self.device_id)
    }

    /// Fetch the current config override from `GET /v1/config/{device_id}`.
    ///
    /// Returns `Some(override)` when the server has a non-empty override, or
    /// `None` on a 204 / network error / invalid response.  Failures are
    /// logged at `warn` level but do NOT propagate — the caller should
    /// continue with the existing effective config.
    pub async fn fetch_override(&self) -> Option<AgentConfigOverride> {
        let url = format!("{}/v1/config/{}", self.api_endpoint, self.device_id);
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                warn!(url = %url, error = %e, "config fetch failed");
                return None;
            }
        };

        match resp.status().as_u16() {
            200 => {
                match resp.json::<AgentConfigOverride>().await {
                    Ok(over) => {
                        if over.validate().is_err() {
                            warn!(
                                "server returned an invalid config override — ignoring"
                            );
                            return None;
                        }
                        Some(over)
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to deserialize config override — ignoring");
                        None
                    }
                }
            }
            204 => {
                // Server says "no overrides configured for this device".
                None
            }
            other => {
                warn!(status = other, url = %url, "unexpected status from config endpoint");
                None
            }
        }
    }

    /// Pull the latest override from the server and update internal state.
    /// Returns the new effective config.
    pub async fn sync(&mut self) -> AgentConfig {
        let new_override = self.fetch_override().await;
        let mut state_dirty = false;
        match &new_override {
            Some(over) => {
                let is_new = self.applied_override.as_ref() != Some(over);
                if is_new {
                    info!("applying new remote config override");
                    self.applied_override = Some(over.clone());
                    // Reset the failure clock so the 24-hour window restarts.
                    if self.first_failure.take().is_some() {
                        state_dirty = true;
                    }
                } else {
                    debug!("remote config override unchanged");
                }
            }
            None => {
                if self.applied_override.is_some() {
                    info!("server has no override for this device; clearing applied override");
                    self.applied_override = None;
                    if self.first_failure.take().is_some() {
                        state_dirty = true;
                    }
                }
            }
        }
        if state_dirty {
            self.persist_state();
        }
        self.effective_config()
    }

    /// Record that an upload or connection attempt succeeded.  Clears the
    /// failure clock (in-memory + persisted).
    pub fn record_success(&mut self) {
        self.last_success = Some(Instant::now());
        if self.first_failure.take().is_some() {
            self.persist_state();
        }
    }

    /// Record that an upload or connection attempt failed.  Starts the 24-hour
    /// rollback clock if it isn't already running, and mirrors the timestamp
    /// to disk so a crash-loop can't reset it.
    pub fn record_failure(&mut self) {
        if self.applied_override.is_some() && self.first_failure.is_none() {
            self.first_failure = Some(Instant::now());
            self.persist_state();
        }
    }

    /// Returns `true` when the agent has been failing for longer than
    /// [`ROLLBACK_THRESHOLD`] since the current override was applied.  In that
    /// case [`ConfigSync::rollback`] should be called.
    pub fn should_rollback(&self) -> bool {
        // Only roll back if an override is active and failures have been
        // accumulating long enough.
        if self.applied_override.is_none() {
            return false;
        }
        match self.first_failure {
            Some(t) => t.elapsed() >= ROLLBACK_THRESHOLD,
            None => false,
        }
    }

    /// Drop the current override and revert to the local config.  Logs a
    /// warning so the rollback is visible in telemetry. Clears the
    /// persisted failure clock so the next operator push gets a fresh
    /// 24-hour window.
    pub fn rollback(&mut self) {
        warn!(
            "rolling back to local config: agent has had no successful upload for ≥ {} h \
             since the remote override was applied",
            ROLLBACK_THRESHOLD.as_secs() / 3600
        );
        self.applied_override = None;
        self.first_failure = None;
        self.persist_state();
    }

    /// Best-effort write of the current failure clock to the state file.
    /// Errors are logged but never propagated — the state file is purely
    /// crash-resilience belt-and-suspenders.
    fn persist_state(&self) {
        let Some(ref path) = self.state_path else {
            return;
        };
        // Convert `Instant` → `DateTime<Utc>` via SystemTime so the value
        // round-trips through the JSON file. We intentionally store the
        // wall-clock time rather than a monotonic offset: the rollback
        // window is "≥ 24 h since first failure", which is a wall-clock
        // semantic the user cares about.
        let state = PersistedState {
            first_failure_at: self.first_failure.map(|i| {
                let now_inst = Instant::now();
                let now_ut = Utc::now();
                if i <= now_inst {
                    let age = now_inst - i;
                    now_ut - chrono::Duration::from_std(age).unwrap_or_default()
                } else {
                    now_ut
                }
            }),
        };
        if let Err(e) = write_persisted_state(path, &state) {
            warn!(path = %path.display(), error = %e, "failed to persist config-sync state");
        }
    }

    /// Return the effective config: local config with any active override
    /// merged on top.
    pub fn effective_config(&self) -> AgentConfig {
        match &self.applied_override {
            Some(over) => merge_override(&self.local_config, over),
            None => self.local_config.clone(),
        }
    }
}

/// Merge `over` on top of `base`.  Only the fields in [`AgentConfigOverride`]
/// (the safe-to-push whitelist) are overridable; `api_endpoint` and TLS fields
/// are **always** taken from `base`.
pub fn merge_override(base: &AgentConfig, over: &AgentConfigOverride) -> AgentConfig {
    AgentConfig {
        // --- NOT overridable (safety boundary) ---
        api_endpoint: base.api_endpoint.clone(),
        tls_client_cert_pem: base.tls_client_cert_pem.clone(),
        tls_client_key_pem: base.tls_client_key_pem.clone(),
        tls_ca_bundle_pem: base.tls_ca_bundle_pem.clone(),
        device_id: base.device_id.clone(),

        // --- Overridable via AgentConfigOverride ---
        log_level: over
            .log_level
            .clone()
            .unwrap_or_else(|| base.log_level.clone()),
        request_timeout_secs: over
            .request_timeout_secs
            .unwrap_or(base.request_timeout_secs),
        evidence_schedule: over
            .evidence_schedule
            .clone()
            .unwrap_or_else(|| base.evidence_schedule.clone()),
        queue_max_bundles: over
            .queue_max_bundles
            .unwrap_or(base.queue_max_bundles),
        log_paths: over
            .log_paths
            .clone()
            .unwrap_or_else(|| base.log_paths.clone()),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Default location of the persisted state file, mirroring `Queue::default_root`'s
/// platform conventions so all of the agent's runtime state lives under one
/// `%ProgramData%` subtree.
fn default_state_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\ProgramData"));
        base.join("CMTraceOpen")
            .join("Agent")
            .join("config-state.json")
    }
    #[cfg(not(target_os = "windows"))]
    {
        let base = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        base.join(".cmtraceopen-agent").join("config-state.json")
    }
}

fn load_persisted_state(path: &std::path::Path) -> Option<PersistedState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<PersistedState>(&text).ok()
}

fn write_persisted_state(
    path: &std::path::Path,
    state: &PersistedState,
) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Atomic-rename: serialize → tempfile → rename. Same pattern Queue
    // uses for its sidecars.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = path.with_extension(format!("json.{nanos}.tmp"));
    let json = serde_json::to_vec_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Deterministic, per-device-stable jitter in `[0, CONFIG_FETCH_JITTER_MAX]`.
/// Hashes the device id with SHA-256 and maps the leading 8 bytes into the
/// jitter range. Stable per-device + uniform across the fleet.
fn device_id_jitter(device_id: &str) -> Duration {
    let mut h = Sha256::new();
    h.update(device_id.as_bytes());
    let digest = h.finalize();
    let mut leading = [0u8; 8];
    leading.copy_from_slice(&digest[..8]);
    let n = u64::from_be_bytes(leading);
    let max_secs = CONFIG_FETCH_JITTER_MAX.as_secs().max(1);
    Duration::from_secs(n % max_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> AgentConfig {
        AgentConfig::default()
    }

    fn fresh_config_sync() -> ConfigSync {
        ConfigSync::new_with_state_path(
            reqwest::Client::new(),
            "http://localhost:8080".into(),
            "WIN-TEST".into(),
            base(),
            None, // disable persistence for tests
        )
    }

    #[test]
    fn merge_override_applies_safe_fields() {
        let base = base();
        let over = AgentConfigOverride {
            log_level: Some("debug".into()),
            queue_max_bundles: Some(5),
            ..AgentConfigOverride::default()
        };
        let merged = merge_override(&base, &over);
        assert_eq!(merged.log_level, "debug");
        assert_eq!(merged.queue_max_bundles, 5);
        // Untouched fields keep the base value.
        assert_eq!(merged.request_timeout_secs, base.request_timeout_secs);
        assert_eq!(merged.evidence_schedule, base.evidence_schedule);
    }

    #[test]
    fn merge_override_api_endpoint_not_overridable() {
        // Even if AgentConfigOverride had an api_endpoint field (it doesn't),
        // the merge function explicitly preserves the base value.
        let mut base = base();
        base.api_endpoint = "https://real-server.corp.example.com".into();

        // The override struct has no api_endpoint field — the test confirms
        // the merged result always retains the base endpoint.
        let over = AgentConfigOverride {
            log_level: Some("info".into()),
            ..AgentConfigOverride::default()
        };
        let merged = merge_override(&base, &over);
        assert_eq!(merged.api_endpoint, "https://real-server.corp.example.com");
    }

    #[test]
    fn merge_override_empty_noop() {
        let base = base();
        let merged = merge_override(&base, &AgentConfigOverride::default());
        // All fields identical to base.
        assert_eq!(merged.log_level, base.log_level);
        assert_eq!(merged.request_timeout_secs, base.request_timeout_secs);
        assert_eq!(merged.queue_max_bundles, base.queue_max_bundles);
    }

    #[test]
    fn record_failure_starts_clock_when_override_active() {
        let mut cs = fresh_config_sync();
        // No override → failure clock never starts.
        cs.record_failure();
        assert!(cs.first_failure.is_none());
        assert!(!cs.should_rollback());

        // Apply an override manually then record a failure.
        cs.applied_override = Some(AgentConfigOverride {
            log_level: Some("debug".into()),
            ..AgentConfigOverride::default()
        });
        cs.record_failure();
        assert!(cs.first_failure.is_some());
        // Hasn't been 24h yet.
        assert!(!cs.should_rollback());
    }

    #[test]
    fn record_success_clears_failure_clock() {
        let mut cs = fresh_config_sync();
        cs.applied_override = Some(AgentConfigOverride::default());
        cs.record_failure();
        assert!(cs.first_failure.is_some());

        cs.record_success();
        assert!(cs.first_failure.is_none());
    }

    #[test]
    fn rollback_clears_override() {
        let mut cs = fresh_config_sync();
        cs.applied_override = Some(AgentConfigOverride {
            log_level: Some("debug".into()),
            ..AgentConfigOverride::default()
        });
        cs.first_failure = Some(Instant::now());

        cs.rollback();
        assert!(cs.applied_override.is_none());
        assert!(cs.first_failure.is_none());
        // After rollback the effective config is the local base.
        assert_eq!(cs.effective_config().log_level, AgentConfig::default().log_level);
    }

    #[test]
    fn should_rollback_triggers_after_threshold() {
        let mut cs = fresh_config_sync();
        cs.applied_override = Some(AgentConfigOverride::default());
        // Fake an old first_failure by subtracting more than the threshold.
        cs.first_failure = Some(Instant::now() - ROLLBACK_THRESHOLD - Duration::from_secs(1));
        assert!(cs.should_rollback());
    }

    #[test]
    fn fetch_interval_is_deterministic_per_device() {
        let cs1 = ConfigSync::new_with_state_path(
            reqwest::Client::new(),
            "http://localhost:8080".into(),
            "WIN-TEST".into(),
            base(),
            None,
        );
        let cs1_again = ConfigSync::new_with_state_path(
            reqwest::Client::new(),
            "http://localhost:8080".into(),
            "WIN-TEST".into(),
            base(),
            None,
        );
        assert_eq!(cs1.fetch_interval(), cs1_again.fetch_interval());

        let cs2 = ConfigSync::new_with_state_path(
            reqwest::Client::new(),
            "http://localhost:8080".into(),
            "WIN-OTHER".into(),
            base(),
            None,
        );
        // Different device id → different jitter → different interval.
        // (Astronomically unlikely to collide on a 30-min range with two
        // unrelated SHA-256 prefixes.)
        assert_ne!(cs1.fetch_interval(), cs2.fetch_interval());
    }

    #[test]
    fn fetch_interval_is_within_bounds() {
        let cs = fresh_config_sync();
        let interval = cs.fetch_interval();
        assert!(interval >= CONFIG_FETCH_INTERVAL);
        assert!(interval <= CONFIG_FETCH_INTERVAL + CONFIG_FETCH_JITTER_MAX);
    }

    #[test]
    fn failure_clock_persists_across_recreate() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("config-state.json");

        // First instance: apply override, record failure → clock starts +
        // persists.
        {
            let mut cs = ConfigSync::new_with_state_path(
                reqwest::Client::new(),
                "http://localhost:8080".into(),
                "WIN-TEST".into(),
                base(),
                Some(state_path.clone()),
            );
            cs.applied_override = Some(AgentConfigOverride {
                log_level: Some("debug".into()),
                ..AgentConfigOverride::default()
            });
            cs.record_failure();
            assert!(cs.first_failure.is_some());
            assert!(state_path.exists(), "state file should be written");
        } // drop, simulating agent crash

        // Second instance: should reload the failure timestamp, NOT reset
        // the clock to "no failure".
        let cs2 = ConfigSync::new_with_state_path(
            reqwest::Client::new(),
            "http://localhost:8080".into(),
            "WIN-TEST".into(),
            base(),
            Some(state_path.clone()),
        );
        assert!(
            cs2.first_failure.is_some(),
            "failure clock must survive restart"
        );
    }

    #[test]
    fn record_success_clears_persisted_clock() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("config-state.json");

        let mut cs = ConfigSync::new_with_state_path(
            reqwest::Client::new(),
            "http://localhost:8080".into(),
            "WIN-TEST".into(),
            base(),
            Some(state_path.clone()),
        );
        cs.applied_override = Some(AgentConfigOverride::default());
        cs.record_failure();
        assert!(state_path.exists());

        cs.record_success();
        // After clearing the clock the file should reflect no failure.
        let reloaded = load_persisted_state(&state_path).expect("state file should exist");
        assert!(reloaded.first_failure_at.is_none());
    }
}
