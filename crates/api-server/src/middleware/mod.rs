//! Tower / Axum middleware for the api-server.
//!
//! Currently contains:
//!   - [`audit`] — appends one row to `audit_log` per auditable admin request.

pub mod audit;
