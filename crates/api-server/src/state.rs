//! Shared application state injected into handlers via `State`.
//!
//! Holds the two storage traits as trait objects so handlers don't care
//! whether the backend is local-fs + SQLite (MVP) or S3 + Postgres (later).

use std::sync::Arc;

use crate::storage::{BlobStore, MetadataStore};

/// Default chunk size we advertise to agents: 8 MiB. Picked to balance
/// round-trip overhead against memory-per-request. Agents MAY send smaller
/// chunks; the server enforces a hard cap (32 MiB) per chunk regardless.
pub const DEFAULT_CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// Hard per-chunk cap. Protects the server from OOM if a misbehaving agent
/// sends a multi-GB PUT body.
///
/// This value is the single source of truth for the ingest-route body limit:
/// the Axum router layers [`axum::extract::DefaultBodyLimit::max`] at
/// exactly this size on the ingest sub-router so it can never drift from the
/// runtime check in the chunk handler.
pub const MAX_CHUNK_SIZE: u64 = 32 * 1024 * 1024;

pub struct AppState {
    pub meta: Arc<dyn MetadataStore>,
    pub blobs: Arc<dyn BlobStore>,
}

impl AppState {
    pub fn new(meta: Arc<dyn MetadataStore>, blobs: Arc<dyn BlobStore>) -> Arc<Self> {
        Arc::new(Self { meta, blobs })
    }
}
