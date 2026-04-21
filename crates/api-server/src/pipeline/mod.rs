//! Async post-ingest pipelines.
//!
//! Today this is a single background parse worker spawned from the finalize
//! handler. The module exists so the next pipeline (e.g. retention sweeper,
//! re-parse on parser upgrade) has an obvious home.

pub mod parse_worker;
