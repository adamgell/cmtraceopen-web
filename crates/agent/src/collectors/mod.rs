//! Evidence collectors.
//!
//! Each concrete collector implements [`Collector`] and writes its output
//! into a caller-supplied directory under the in-progress bundle staging
//! area. The orchestrator in [`evidence`] fans them out in parallel, zips
//! the result, and returns a [`BundleMetadata`] ready for the upload queue.
//!
//! ## cfg-gating
//!
//! The real collectors are Windows-only — they shell out to `wevtutil`,
//! `dsregcmd`, or read paths that only exist under `C:\Windows`. On Linux
//! CI runners the crate still needs to compile, so each collector module
//! exposes a Linux stub that returns [`CollectorResult::NotSupported`].
//! The orchestrator is the same on both platforms; it just gets stub
//! manifests with `NotSupported` markers in the Linux build.
//!
//! ## Why not `trait`-object dispatch?
//!
//! For MVP the orchestrator just calls each collector concretely and joins
//! on `tokio::join!`. Using a `dyn Collector` would buy us dynamic
//! registration (e.g. from config), which we don't need until the
//! scheduler lands. Keeping the types concrete also sidesteps async-trait
//! object-safety headaches.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod agent_logs;
pub mod dsregcmd;
pub mod event_logs;
pub mod evidence;
pub mod logs;

/// Manifest entry recorded per collector run. Written into the bundle as
/// `evidence/manifest.json` so the parse worker can see what was collected
/// (and what was skipped / failed) without re-inspecting every file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectorManifest {
    /// Stable identifier, e.g. `"logs"`, `"event-logs"`, `"dsregcmd"`.
    pub name: String,
    /// Outcome of the run. `NotSupported` on non-Windows platforms; `Ok`
    /// on a successful pass (possibly empty); `Failed` on a hard error
    /// (the bundle still uploads — we'd rather ship a partial bundle than
    /// drop the whole collection because one collector tripped).
    pub result: CollectorResult,
    /// Relative paths (under the bundle root) of files this collector
    /// produced. Empty on `NotSupported` / `Failed`.
    pub files: Vec<String>,
    /// Optional free-form note (e.g. which paths were walked, exit code,
    /// timing). Not wire-parsed.
    pub note: Option<String>,
}

/// Outcome variant. Split from `CollectorError` because a collector can
/// "succeed with nothing to ship" (e.g. configured log path is empty on
/// this particular host) and that's not an error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CollectorResult {
    Ok,
    /// Used by the Linux stubs and any collector the config disables.
    NotSupported,
    /// Collector ran but hit a non-fatal error. Captured so the manifest
    /// preserves the failure for the parse worker to reason about.
    Failed { message: String },
}

/// Structured error surface. The orchestrator converts hard I/O failures
/// into `CollectorResult::Failed` on the manifest rather than aborting —
/// these variants are for callers that want to short-circuit explicitly
/// (e.g. unit tests, future retry logic).
#[derive(Debug, thiserror::Error)]
pub enum CollectorError {
    #[error("collector i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("collector not supported on this platform")]
    NotSupported,

    #[error("collector subprocess failed: {0}")]
    Subprocess(String),

    #[error("collector glob error: {0}")]
    Glob(String),
}

/// Contract every collector implements.
///
/// `collect` writes zero-or-more files under `out_dir` (which the caller
/// has already created) and returns a [`CollectorManifest`] describing the
/// outcome. Implementations MUST NOT create siblings of `out_dir` or write
/// outside it — the orchestrator's zip step assumes the bundle root is
/// self-contained.
#[async_trait]
pub trait Collector: Send + Sync {
    fn name(&self) -> &'static str;

    async fn collect(&self, out_dir: &Path) -> Result<CollectorManifest, CollectorError>;
}

/// Final bundle metadata the orchestrator produces, ready to hand to the
/// upload queue. Mirrors the subset of [`common_wire::ingest::BundleInitRequest`]
/// fields we can compute locally: the server assigns `upload_id` at init
/// time, not here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleMetadata {
    pub bundle_id: uuid::Uuid,
    /// Lowercase hex sha256 over the zipped bundle bytes.
    pub sha256: String,
    pub size_bytes: u64,
    /// One of `common_wire::ingest::content_kind::*`. Always
    /// `"evidence-zip"` for bundles produced by this orchestrator today.
    pub content_kind: String,
}

/// Convert a filesystem path to a bundle-relative POSIX-style string.
/// Zip archives are POSIX-pathed even on Windows, and the manifest we
/// ship inside the bundle should match.
pub(crate) fn to_bundle_relative(root: &Path, p: &Path) -> String {
    let rel = p.strip_prefix(root).unwrap_or(p);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Ensure `dir` exists. Thin wrapper so collectors don't each re-implement
/// the "create_dir_all if missing" dance.
pub(crate) async fn ensure_dir(dir: &Path) -> std::io::Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    Ok(dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_metadata_round_trips_camel_case() {
        let m = BundleMetadata {
            bundle_id: uuid::Uuid::nil(),
            sha256: "deadbeef".into(),
            size_bytes: 42,
            content_kind: "evidence-zip".into(),
        };
        let v = serde_json::to_value(&m).unwrap();
        assert!(v.get("bundleId").is_some());
        assert!(v.get("sha256").is_some());
        assert!(v.get("sizeBytes").is_some());
        assert!(v.get("contentKind").is_some());
    }

    #[test]
    fn collector_result_not_supported_variant_serializes() {
        let r = CollectorResult::NotSupported;
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, "\"notSupported\"");
    }

    #[test]
    fn to_bundle_relative_uses_posix_seps() {
        let root = Path::new("/tmp/bundle");
        let p = root.join("logs").join("ccmexec.log");
        assert_eq!(to_bundle_relative(root, &p), "logs/ccmexec.log");
    }
}
