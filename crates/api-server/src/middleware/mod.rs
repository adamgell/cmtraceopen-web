//! Tower / Axum middleware for the api-server.
//!
//! Currently contains:
//!   - [`audit`] — appends one row to `audit_log` per auditable admin request.
//!   - [`proxy`] — request-processing helpers for proxied / fronted deployments.
//!   - [`rate_limit`] — per-device + per-IP rate limiting for DoS protection.

pub mod audit;
pub mod proxy;
pub mod rate_limit;
