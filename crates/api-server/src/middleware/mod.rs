//! Tower / Axum middleware for the api-server.
//!
//! Currently contains:
//!   - [`audit`] — appends one row to `audit_log` per auditable admin request.
//!   - [`proxy`] — request-processing helpers for proxied / fronted deployments.

pub mod audit;
pub mod proxy;
