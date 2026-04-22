//! Audit-log middleware for admin routes.
//!
//! Wraps any route inside the admin sub-router and, after the inner handler
//! returns, appends one row to `audit_log` recording who did what, to which
//! resource, and whether it succeeded.
//!
//! ## Principal extraction
//!
//! The middleware calls [`OperatorPrincipal::from_request_parts`] directly
//! (the same extractor the per-handler `RequireRole` gate uses) so the JWT
//! parse happens at most twice per request.  If auth fails the principal
//! fields fall back to anonymous placeholders — the audit row is still
//! written so failed authentication attempts are visible in the log.
//!
//! ## Action + target mapping
//!
//! Route templates are mapped to dot-namespaced action strings via
//! [`route_to_action`].  Target kind/id are derived from the template and
//! the actual request URI.
//!
//! ## PII policy
//!
//! `details_json` is intentionally empty for the MVP. Any future extras MUST
//! NOT include device hostnames, user-agent strings, or free-text fields from
//! request bodies — those are PII under the compliance scope this log serves.

use std::sync::Arc;

use axum::extract::{FromRequestParts, MatchedPath, State};
use axum::http::{Method, Request};
use axum::middleware::Next;
use axum::response::Response;
use chrono::Utc;
use tracing::warn;
use uuid::Uuid;

use crate::auth::OperatorPrincipal;
use crate::state::AppState;
use crate::storage::NewAuditRow;

/// Map an Axum route template + HTTP method to a dot-namespaced action string.
///
/// Unknown templates fall back to `"<METHOD>.<PATH>"` so new routes are at
/// least traceable before they're given a friendly name here.
fn route_to_action(method: &Method, template: Option<&str>) -> String {
    match (method, template) {
        (&Method::POST, Some("/v1/admin/devices/{device_id}/disable")) => {
            "device.disable".to_string()
        }
        (&Method::GET, Some("/v1/admin/audit")) => "audit.list".to_string(),
        (m, Some(p)) => format!("{}.{p}", m.as_str().to_ascii_lowercase()),
        (m, None) => format!("{}.unknown", m.as_str().to_ascii_lowercase()),
    }
}

/// Derive `(target_kind, target_id)` from a known route template and the
/// actual request path.
///
/// Returns `(None, None)` for routes that don't operate on a single
/// identifiable resource (e.g. list/read routes).
fn route_to_target(template: Option<&str>, path: &str) -> (Option<String>, Option<String>) {
    match template {
        Some("/v1/admin/devices/{device_id}/disable") => {
            // /v1/admin/devices/<device_id>/disable → parts[4] is device_id
            let device_id = path.split('/').nth(4).map(str::to_string);
            (Some("device".to_string()), device_id)
        }
        _ => (None, None),
    }
}

/// Derive `principal_kind` from an [`OperatorPrincipal`]'s role set.
fn principal_kind(p: &OperatorPrincipal) -> &'static str {
    use crate::auth::Role;
    if p.roles.contains(&Role::Admin) {
        "admin"
    } else {
        "operator"
    }
}

/// Axum middleware that appends one [`audit_log`] row per request.
///
/// Apply to admin sub-routers via
/// `router.layer(axum::middleware::from_fn_with_state(state, audit_middleware))`.
///
/// The `GET /v1/admin/audit` route itself is *excluded* from logging to
/// prevent the audit log from growing unboundedly with self-referential reads.
pub async fn audit_middleware(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // Capture metadata before consuming the request.
    let method = request.method().clone();
    let uri_path = request.uri().path().to_string();
    let matched = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string());

    // Skip self-auditing reads of the audit log.
    if matched.as_deref() == Some("/v1/admin/audit") && method == Method::GET {
        return next.run(request).await;
    }

    // Try to extract the principal from the request parts.  We decompose
    // the request only to read headers, then reassemble it so the handler
    // still sees the full request.
    let (mut parts, body) = request.into_parts();
    let principal = OperatorPrincipal::from_request_parts(&mut parts, &state)
        .await
        .ok();
    let request = Request::from_parts(parts, body);

    // Run the actual handler.
    let response = next.run(request).await;

    let result = if response.status().is_success() {
        "success"
    } else {
        "failure"
    };

    let (p_kind, p_id, p_display) = match &principal {
        Some(p) => (
            principal_kind(p).to_string(),
            p.subject.clone(),
            p.name.clone(),
        ),
        None => ("anonymous".to_string(), String::new(), None),
    };

    let action = route_to_action(&method, matched.as_deref());
    let (target_kind, target_id) = route_to_target(matched.as_deref(), &uri_path);

    let row = NewAuditRow {
        id: Uuid::now_v7(),
        ts_utc: Utc::now(),
        principal_kind: p_kind,
        principal_id: p_id,
        principal_display: p_display,
        action,
        target_kind,
        target_id,
        result: result.to_string(),
        details_json: None,
        request_id: None,
    };

    if let Err(err) = state.audit.insert_audit_row(row).await {
        // Audit write failures MUST be logged loudly but MUST NOT
        // propagate to the caller — the admin action already completed and
        // we should not roll it back due to a logging side-effect.
        warn!(error = %err, "audit_log write failed");
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_mapping_disable_device() {
        assert_eq!(
            route_to_action(
                &Method::POST,
                Some("/v1/admin/devices/{device_id}/disable")
            ),
            "device.disable"
        );
    }

    #[test]
    fn action_mapping_audit_list() {
        assert_eq!(
            route_to_action(&Method::GET, Some("/v1/admin/audit")),
            "audit.list"
        );
    }

    #[test]
    fn target_extraction_disable_device() {
        let (kind, id) = route_to_target(
            Some("/v1/admin/devices/{device_id}/disable"),
            "/v1/admin/devices/my-device-123/disable",
        );
        assert_eq!(kind.as_deref(), Some("device"));
        assert_eq!(id.as_deref(), Some("my-device-123"));
    }

    #[test]
    fn target_extraction_unknown_route() {
        let (kind, id) = route_to_target(Some("/v1/admin/audit"), "/v1/admin/audit");
        assert!(kind.is_none());
        assert!(id.is_none());
    }
}
