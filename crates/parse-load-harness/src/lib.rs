//! Parse load harness — drives the api-server's parse pipeline against
//! three targets with three load shapes, streaming results to disk for
//! post-hoc analysis. See `docs/superpowers/specs/2026-04-29-parse-load-harness-design.md`.

pub mod auth;
pub mod bundle;
pub mod config;
pub mod corpus;
pub mod driver;
pub mod reporter;
pub mod target;
