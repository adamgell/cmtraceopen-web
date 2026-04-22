//! Async post-ingest pipelines.
//!
//! Two background tasks live here:
//!   - [`parse_worker`] — fire-and-forget parse spawned from the ingest
//!     finalize handler.
//!   - [`retention`] — periodic sweep that purges sessions past the
//!     configured `CMTRACE_BUNDLE_TTL_DAYS` window. Spawned once at
//!     startup from `main.rs`.

pub mod parse_worker;
pub mod retention;
