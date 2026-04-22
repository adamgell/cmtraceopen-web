//! Admin-only routes.
//!
//! Routes here are gated on [`RequireRole<AdminTag>`][crate::auth::RequireRole]
//! — i.e. the caller's JWT must carry the `CmtraceOpen.Admin` app role.
//! Operator-level tokens (delegated `CmtraceOpen.Query` scope or the
//! `CmtraceOpen.Operator` app role) are explicitly NOT accepted here, so
//! that destructive admin actions can never be invoked through the
//! interactive web viewer's user-delegated token.
//!
//! The MVP exposes a single placeholder endpoint:
//!
//!   POST /v1/admin/devices/{device_id}/disable   →   501 Not Implemented
//!
//! It exists to nail down the URL surface, the role-gating wiring, and the
//! response shape for the disable-device function envisioned in the platform
//! plan; the actual MDM-side disable workflow lands in a future PR.
//!
//! Adding more admin routes? Wire them through this same router so the
//! `RequireRole<AdminTag>` discipline is uniform — don't sprinkle the
//! extractor across the wider route tree where it's easy to miss in review.
//!
//! See `docs/provisioning/02-entra-app-registration.md` §6 for how the
//! admin app role is defined in Entra and how operators get assigned to it
//! via the Enterprise Application's Users-and-groups blade.

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};

use crate::auth::{AdminTag, RequireRole};
use crate::state::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/v1/admin/devices/{device_id}/disable",
            post(disable_device),
        )
        .with_state(state)
}

/// Placeholder admin route. Returns 501 Not Implemented. Exists so:
///   1. The URL + verb are reserved (clients can stub against it).
///   2. The `RequireRole<AdminTag>` extractor is exercised in integration
///      tests, locking in the 401/403/501 status matrix.
///   3. The OpenAPI surface (added in a later PR) has something concrete to
///      point at when documenting the admin role.
async fn disable_device(
    _principal: RequireRole<AdminTag>,
    Path(device_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let body = serde_json::json!({
        "error": "not_implemented",
        "message": format!(
            "device-disable is not yet implemented; reserved admin route for device '{device_id}'"
        ),
    });
    (StatusCode::NOT_IMPLEMENTED, Json(body))
}
