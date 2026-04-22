//! `GET /v1/config/{device_id}` — server-side config push (Wave 4).
//!
//! The agent calls this endpoint at startup, every 6 h, and after every
//! successful upload.  The server merges the tenant-wide default override
//! (if any) with any per-device override (per-device wins) and returns the
//! resulting [`AgentConfigOverride`] JSON.  A `204 No Content` response means
//! "no overrides are configured for this device; use your local config".
//!
//! ## Authentication
//!
//! The endpoint requires a [`DeviceIdentity`] extractor (mTLS client cert
//! preferred, `X-Device-Id` header as transitional fallback — same identity
//! surface as `routes::ingest`). The `device_id` from the URL **must** match
//! the authenticated identity, otherwise the request is rejected `403`.
//! Without this gate any network-reachable caller could enumerate per-device
//! overrides — including custom `log_paths` and cron schedules that leak the
//! tenant's collection policy.
//!
//! The admin routes that *write* overrides live in `routes::admin` and are
//! gated on `RequireRole<AdminTag>`.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use common_wire::AgentConfigOverride;
use tracing::warn;

use crate::auth::DeviceIdentity;
use crate::state::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/config/{device_id}", get(get_device_config))
        .with_state(state)
}

/// `GET /v1/config/{device_id}`
///
/// Returns the effective config override for a device:
///   - Start with the tenant-wide default override (if any).
///   - Apply per-device override on top (per-device fields win).
///   - Return the merged result.
///
/// Returns `204 No Content` when no overrides are configured for this device
/// (the agent should continue using its local config).
///
/// Returns `403 Forbidden` when the authenticated identity's `device_id`
/// doesn't match the path parameter (a device may only read its own config).
async fn get_device_config(
    identity: DeviceIdentity,
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
) -> Result<(StatusCode, Json<AgentConfigOverride>), (StatusCode, Json<serde_json::Value>)> {
    // Enforce same-device read. We accept the device's own identity only;
    // operator/admin tooling must use the admin write routes (which already
    // accept arbitrary device ids) plus a separate read-back path if/when
    // one is needed. This keeps the device-facing surface trivially safe.
    if identity.device_id != device_id {
        warn!(
            authenticated_device = %identity.device_id,
            requested_device = %device_id,
            identity_source = ?identity.source,
            "rejected cross-device config read",
        );
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "forbidden",
                "message": "device may only read its own config override"
            })),
        ));
    }

    // Load the tenant-wide default (may be None).
    let default = state.configs.get_default_config().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "internal", "message": e.to_string() })),
        )
    })?;

    // Load per-device override (may be None).
    let device = state
        .configs
        .get_device_config(&device_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "internal", "message": e.to_string() })),
            )
        })?;

    // Merge: start from default, apply device-level fields on top.
    let merged = merge_overrides(default, device);

    if merged.is_empty() {
        // No overrides at all — agent should use local config.
        return Err((
            StatusCode::NO_CONTENT,
            Json(serde_json::json!({})),
        ));
    }

    Ok((StatusCode::OK, Json(merged)))
}

/// Merge two optional overrides: `base` is the tenant-wide default; `top` is
/// the per-device override that wins.  Per-device `Some` fields replace the
/// corresponding field from `base`.  Fields absent in `top` fall through to
/// `base`.
fn merge_overrides(
    base: Option<AgentConfigOverride>,
    top: Option<AgentConfigOverride>,
) -> AgentConfigOverride {
    let base = base.unwrap_or_default();
    let top = top.unwrap_or_default();

    AgentConfigOverride {
        log_level: top.log_level.or(base.log_level),
        request_timeout_secs: top.request_timeout_secs.or(base.request_timeout_secs),
        evidence_schedule: top.evidence_schedule.or(base.evidence_schedule),
        queue_max_bundles: top.queue_max_bundles.or(base.queue_max_bundles),
        log_paths: top.log_paths.or(base.log_paths),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_overrides_device_wins() {
        let base = AgentConfigOverride {
            log_level: Some("info".into()),
            queue_max_bundles: Some(50),
            ..AgentConfigOverride::default()
        };
        let top = AgentConfigOverride {
            log_level: Some("debug".into()),
            ..AgentConfigOverride::default()
        };
        let merged = merge_overrides(Some(base), Some(top));
        // Per-device "debug" overrides tenant-wide "info".
        assert_eq!(merged.log_level.as_deref(), Some("debug"));
        // No per-device value → falls through to base.
        assert_eq!(merged.queue_max_bundles, Some(50));
    }

    #[test]
    fn merge_overrides_none_base_and_top() {
        let merged = merge_overrides(None, None);
        assert!(merged.is_empty());
    }

    #[test]
    fn merge_overrides_only_base() {
        let base = AgentConfigOverride {
            request_timeout_secs: Some(30),
            ..AgentConfigOverride::default()
        };
        let merged = merge_overrides(Some(base), None);
        assert_eq!(merged.request_timeout_secs, Some(30));
    }
}
