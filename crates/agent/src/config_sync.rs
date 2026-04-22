//! Server-side config pull + safe-rollback (Wave 4).
//!
//! [`ConfigSync`] fetches [`AgentConfigOverride`] from the api-server at
//! startup, every [`CONFIG_FETCH_INTERVAL`] hours, and after every successful
//! upload.  Received overrides are **merged** on top of the agent's local
//! config (the base) — only the fields listed in `AgentConfigOverride` can
//! be changed; sensitive fields (`api_endpoint`, TLS paths) are immutable.
//!
//! ## Safe rollback
//!
//! If the agent cannot successfully connect / upload for
//! [`ROLLBACK_THRESHOLD`] after applying a remote override, it reverts to the
//! last-known-good local config.  This guards against a mis-configured
//! override (e.g. `requestTimeoutSecs: 0`) that would otherwise brick the
//! agent until the next MSI re-deploy.
//!
//! The rollback timer is wall-clock based: it starts the first time an upload
//! fails *after* an override was applied and resets every time an upload
//! succeeds.

use std::time::{Duration, Instant};

use common_wire::AgentConfigOverride;
use reqwest::Client;
use tracing::{debug, info, warn};

use crate::config::AgentConfig;

/// How long between periodic config-fetch attempts in daemon mode.
pub const CONFIG_FETCH_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// If the agent has had zero successful uploads for this long since an
/// override was applied, the override is dropped and the local config is
/// restored.
pub const ROLLBACK_THRESHOLD: Duration = Duration::from_secs(24 * 3600);

/// Manages fetching a remote config override and merging it into the running
/// agent config.
pub struct ConfigSync {
    client: Client,
    api_endpoint: String,
    device_id: String,
    /// The baseline config loaded from disk / environment at startup.
    /// Used as the merge base and as the rollback target.
    local_config: AgentConfig,
    /// The current remote override, `None` if no override is active.
    applied_override: Option<AgentConfigOverride>,
    /// Wall-clock instant of the last successful upload.  `None` until the
    /// first success.
    last_success: Option<Instant>,
    /// Wall-clock instant of the first consecutive failure *after* the current
    /// override was applied.  Cleared on every success.
    first_failure: Option<Instant>,
}

impl ConfigSync {
    /// Create a new `ConfigSync`.
    ///
    /// `client` should be the same `reqwest::Client` the uploader uses so TLS
    /// settings (client cert, CA bundle) are consistent.
    pub fn new(client: Client, api_endpoint: String, device_id: String, local: AgentConfig) -> Self {
        Self {
            client,
            api_endpoint,
            device_id,
            local_config: local,
            applied_override: None,
            last_success: None,
            first_failure: None,
        }
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
        match &new_override {
            Some(over) => {
                let is_new = self.applied_override.as_ref() != Some(over);
                if is_new {
                    info!("applying new remote config override");
                    self.applied_override = Some(over.clone());
                    // Reset the failure clock so the 24-hour window restarts.
                    self.first_failure = None;
                } else {
                    debug!("remote config override unchanged");
                }
            }
            None => {
                if self.applied_override.is_some() {
                    info!("server has no override for this device; clearing applied override");
                    self.applied_override = None;
                    self.first_failure = None;
                }
            }
        }
        self.effective_config()
    }

    /// Record that an upload or connection attempt succeeded.  Clears the
    /// failure clock.
    pub fn record_success(&mut self) {
        self.last_success = Some(Instant::now());
        self.first_failure = None;
    }

    /// Record that an upload or connection attempt failed.  Starts the 24-hour
    /// rollback clock if it isn't already running.
    pub fn record_failure(&mut self) {
        if self.applied_override.is_some() && self.first_failure.is_none() {
            self.first_failure = Some(Instant::now());
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
    /// warning so the rollback is visible in telemetry.
    pub fn rollback(&mut self) {
        warn!(
            "rolling back to local config: agent has had no successful upload for ≥ {} h \
             since the remote override was applied",
            ROLLBACK_THRESHOLD.as_secs() / 3600
        );
        self.applied_override = None;
        self.first_failure = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> AgentConfig {
        AgentConfig::default()
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
        let mut cs = ConfigSync::new(
            reqwest::Client::new(),
            "http://localhost:8080".into(),
            "WIN-TEST".into(),
            base(),
        );
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
        let mut cs = ConfigSync::new(
            reqwest::Client::new(),
            "http://localhost:8080".into(),
            "WIN-TEST".into(),
            base(),
        );
        cs.applied_override = Some(AgentConfigOverride::default());
        cs.record_failure();
        assert!(cs.first_failure.is_some());

        cs.record_success();
        assert!(cs.first_failure.is_none());
    }

    #[test]
    fn rollback_clears_override() {
        let mut cs = ConfigSync::new(
            reqwest::Client::new(),
            "http://localhost:8080".into(),
            "WIN-TEST".into(),
            base(),
        );
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
        let mut cs = ConfigSync::new(
            reqwest::Client::new(),
            "http://localhost:8080".into(),
            "WIN-TEST".into(),
            base(),
        );
        cs.applied_override = Some(AgentConfigOverride::default());
        // Fake an old first_failure by subtracting more than the threshold.
        cs.first_failure = Some(Instant::now() - ROLLBACK_THRESHOLD - Duration::from_secs(1));
        assert!(cs.should_rollback());
    }
}
