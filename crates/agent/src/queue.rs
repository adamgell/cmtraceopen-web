//! On-disk persistent queue for bundles pending upload.
//!
//! ## Layout
//!
//! ```text
//! queue_root/
//!   {bundle-id}.zip           # the bundle bytes (moved in by enqueue)
//!   {bundle-id}.json          # sidecar — QueuedBundle state
//!   {bundle-id}.json.tmp      # transient, only during atomic-rename writes
//! ```
//!
//! Each sidecar carries the [`BundleMetadata`], the bundle zip path, and
//! a [`QueueState`] enum (pending / uploading / done / failed). Atomic
//! rename (`tempfile -> real`) guarantees readers never see a
//! half-written sidecar: Windows and Linux both provide
//! crash-consistent rename semantics on same-filesystem moves.
//!
//! ## Why not SQLite?
//!
//! SQLite would be nicer long-term (see the TODO in `main.rs`), but for
//! MVP we want the smallest possible dependency surface and zero schema
//! migrations. A flat-file queue moves the entire persistence story
//! into ~150 lines of code that's trivial to audit and to debug via
//! `notepad state.json` in the field.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use uuid::Uuid;

use crate::collectors::BundleMetadata;

/// State of a queued bundle. Drives the `next_pending` selection and
/// `mark_failed` backoff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum QueueState {
    /// Ready to upload on the next queue drain.
    Pending,
    /// Currently being uploaded. Used as a soft lock so a second
    /// `next_pending` call from a concurrent drain won't pick the same
    /// bundle. Not durable across crashes — on startup any `Uploading`
    /// rows are flipped back to `Pending`.
    Uploading,
    /// Uploaded successfully. Kept on disk until the periodic purge
    /// sweeper hits it — gives operators a window to inspect.
    Done { finished_at: DateTime<Utc> },
    /// Failed at least once; do not retry until `retry_at` has elapsed.
    Failed {
        attempts: u32,
        last_error: String,
        retry_at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedBundle {
    pub metadata: BundleMetadata,
    pub zip_path: PathBuf,
    pub state: QueueState,
    pub enqueued_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("queue i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("queue serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("queue entry not found: {0}")]
    NotFound(Uuid),
}

/// Persistent on-disk queue rooted at `root_dir`.
///
/// `Queue` is cheaply cloneable — all clones share the same `root` path and
/// operate on the same on-disk files. Each queue operation uses per-bundle
/// filenames (UUID-keyed) and atomic-rename writes, so concurrent clones
/// do not corrupt each other's state. The one invariant callers must
/// maintain: a single bundle must not have two callers concurrently
/// transitioning its state (the upload drain and the scheduler naturally
/// satisfy this — they operate on different bundles at the same time).
#[derive(Clone)]
pub struct Queue {
    root: PathBuf,
}

impl Queue {
    /// Open (and create if missing) the queue at `root`. Also flips any
    /// `Uploading` rows back to `Pending` — they're survivors of a
    /// previous crash and the server-side resume path will deal with
    /// the byte-level idempotency.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, QueueError> {
        let root = root.into();
        fs::create_dir_all(&root).await?;
        let q = Self { root };
        q.recover_stuck_uploading().await?;
        Ok(q)
    }

    /// Default on-disk location. `%ProgramData%\cmtraceopen-agent\queue\`
    /// on Windows, `~/.cmtraceopen-agent/queue/` elsewhere. Matches the
    /// path baked into the MSI (see WiX script — not in this crate).
    pub fn default_root() -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            let base = std::env::var("ProgramData")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("C:\\ProgramData"));
            base.join("cmtraceopen-agent").join("queue")
        }
        #[cfg(not(target_os = "windows"))]
        {
            let base = std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/tmp"));
            base.join(".cmtraceopen-agent").join("queue")
        }
    }

    /// Move the bundle zip into the queue directory and write its
    /// sidecar. The caller's original zip path is consumed — we rename
    /// if on the same filesystem, fall back to copy + delete otherwise.
    pub async fn enqueue(
        &self,
        metadata: BundleMetadata,
        zip_source: &Path,
    ) -> Result<QueuedBundle, QueueError> {
        let bundle_id = metadata.bundle_id;
        let final_zip = self.root.join(format!("{bundle_id}.zip"));

        // Try rename first (same-fs fast path).
        if let Err(rename_err) = fs::rename(zip_source, &final_zip).await {
            // Cross-device? Fall back to copy + delete. We can't use
            // `std::io::ErrorKind::CrossesDevices` here — it's stabilized
            // in 1.85 and the workspace MSRV is 1.77 — so match on the
            // two common raw OS codes instead (EXDEV on Unix = 18,
            // Windows' MOVEFILE_REPLACE_EXISTING cross-volume = 17).
            if matches!(rename_err.raw_os_error(), Some(18) | Some(17)) {
                fs::copy(zip_source, &final_zip).await?;
                fs::remove_file(zip_source).await.ok();
            } else {
                return Err(rename_err.into());
            }
        }

        let entry = QueuedBundle {
            metadata,
            zip_path: final_zip,
            state: QueueState::Pending,
            enqueued_at: Utc::now(),
        };
        self.write_sidecar(bundle_id, &entry).await?;
        Ok(entry)
    }

    /// Return the oldest pending bundle whose `retry_at` (if Failed) has
    /// elapsed. Does NOT flip state — callers who intend to actually
    /// upload must call [`Queue::mark_uploading`] first.
    pub async fn next_pending(&self) -> Result<Option<QueuedBundle>, QueueError> {
        let mut entries: Vec<QueuedBundle> = self.load_all().await?;
        // Oldest-first — stable ordering by enqueue time.
        entries.sort_by_key(|a| a.enqueued_at);
        let now = Utc::now();
        Ok(entries.into_iter().find(|e| match &e.state {
            QueueState::Pending => true,
            QueueState::Failed { retry_at, .. } => *retry_at <= now,
            _ => false,
        }))
    }

    pub async fn mark_uploading(&self, bundle_id: Uuid) -> Result<(), QueueError> {
        self.transition(bundle_id, |_| QueueState::Uploading).await
    }

    pub async fn mark_done(&self, bundle_id: Uuid) -> Result<(), QueueError> {
        self.transition(bundle_id, |_| QueueState::Done {
            finished_at: Utc::now(),
        })
        .await
    }

    pub async fn mark_failed(
        &self,
        bundle_id: Uuid,
        error: impl Into<String>,
        retry_delay: Duration,
    ) -> Result<(), QueueError> {
        let error = error.into();
        self.transition(bundle_id, move |prev| {
            let prev_attempts = match &prev.state {
                QueueState::Failed { attempts, .. } => *attempts,
                _ => 0,
            };
            let retry_at =
                Utc::now() + chrono::Duration::from_std(retry_delay).unwrap_or_default();
            QueueState::Failed {
                attempts: prev_attempts + 1,
                last_error: error.clone(),
                retry_at,
            }
        })
        .await
    }

    /// Remove both the sidecar and the zip for a bundle. Used by the
    /// periodic purge sweeper (not yet wired up in `main.rs`).
    pub async fn purge(&self, bundle_id: Uuid) -> Result<(), QueueError> {
        let sidecar = self.sidecar_path(bundle_id);
        let zip = self.zip_path(bundle_id);
        if sidecar.exists() {
            fs::remove_file(&sidecar).await?;
        }
        if zip.exists() {
            fs::remove_file(&zip).await?;
        }
        Ok(())
    }

    pub async fn get(&self, bundle_id: Uuid) -> Result<QueuedBundle, QueueError> {
        let path = self.sidecar_path(bundle_id);
        if !path.exists() {
            return Err(QueueError::NotFound(bundle_id));
        }
        let text = fs::read_to_string(&path).await?;
        Ok(serde_json::from_str(&text)?)
    }

    // ------------------- internals -------------------

    fn sidecar_path(&self, bundle_id: Uuid) -> PathBuf {
        self.root.join(format!("{bundle_id}.json"))
    }

    fn zip_path(&self, bundle_id: Uuid) -> PathBuf {
        self.root.join(format!("{bundle_id}.zip"))
    }

    async fn transition<F>(&self, bundle_id: Uuid, f: F) -> Result<(), QueueError>
    where
        F: FnOnce(&QueuedBundle) -> QueueState,
    {
        let mut entry = self.get(bundle_id).await?;
        entry.state = f(&entry);
        self.write_sidecar(bundle_id, &entry).await
    }

    /// Write a sidecar atomically: serialize → tempfile → rename. A
    /// crash partway through leaves the previous (complete) sidecar on
    /// disk, never a truncated one.
    async fn write_sidecar(&self, bundle_id: Uuid, entry: &QueuedBundle) -> Result<(), QueueError> {
        let final_path = self.sidecar_path(bundle_id);
        // Unique tmp suffix so concurrent writers to different bundles
        // don't collide. (Same bundle: callers are expected to serialize
        // transitions themselves — MVP queue has no internal locks.)
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = self
            .root
            .join(format!("{bundle_id}.json.{nanos}.tmp"));

        let text = serde_json::to_vec_pretty(entry)?;
        fs::write(&tmp, &text).await?;
        // Windows refuses to rename over an existing file by default
        // unless we use the Win32 MoveFileEx(REPLACE_EXISTING) path —
        // `tokio::fs::rename` on recent Rust does that under the hood.
        fs::rename(&tmp, &final_path).await?;
        Ok(())
    }

    async fn load_all(&self) -> Result<Vec<QueuedBundle>, QueueError> {
        let mut out = Vec::new();
        let mut rd = fs::read_dir(&self.root).await?;
        while let Some(entry) = rd.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // Skip transient `.json.NNN.tmp` files — only `.json` is real.
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains(".json.") && n.ends_with(".tmp"))
                .unwrap_or(false)
            {
                continue;
            }
            match fs::read_to_string(&path).await {
                Ok(text) => match serde_json::from_str::<QueuedBundle>(&text) {
                    Ok(e) => out.push(e),
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "skipping corrupt queue sidecar");
                    }
                },
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to read queue sidecar");
                }
            }
        }
        Ok(out)
    }

    /// On open, any `Uploading` entries are crash survivors — flip them
    /// to `Pending` so they'll be retried. The server resume path
    /// dedupes at the byte level.
    async fn recover_stuck_uploading(&self) -> Result<(), QueueError> {
        let entries = self.load_all().await?;
        for e in entries {
            if matches!(e.state, QueueState::Uploading) {
                self.transition(e.metadata.bundle_id, |_| QueueState::Pending)
                    .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::BundleMetadata;
    use common_wire::ingest::content_kind;
    use tempfile::TempDir;

    fn fake_metadata() -> BundleMetadata {
        BundleMetadata {
            bundle_id: Uuid::now_v7(),
            sha256: "0".repeat(64),
            size_bytes: 10,
            content_kind: content_kind::EVIDENCE_ZIP.into(),
        }
    }

    async fn write_fake_zip(tmp: &TempDir, bytes: &[u8]) -> PathBuf {
        let p = tmp.path().join("bundle.zip");
        tokio::fs::write(&p, bytes).await.unwrap();
        p
    }

    #[tokio::test]
    async fn enqueue_moves_zip_and_writes_sidecar() {
        let src_dir = TempDir::new().unwrap();
        let queue_dir = TempDir::new().unwrap();
        let zip = write_fake_zip(&src_dir, b"fake bytes").await;

        let q = Queue::open(queue_dir.path()).await.unwrap();
        let md = fake_metadata();
        let bundle_id = md.bundle_id;
        let entry = q.enqueue(md, &zip).await.unwrap();

        assert_eq!(entry.state, QueueState::Pending);
        assert!(entry.zip_path.exists());
        assert!(!zip.exists(), "source should have been moved");

        let fetched = q.get(bundle_id).await.unwrap();
        assert_eq!(fetched.metadata.bundle_id, bundle_id);
        assert_eq!(fetched.state, QueueState::Pending);
    }

    #[tokio::test]
    async fn next_pending_returns_oldest_first() {
        let src_dir = TempDir::new().unwrap();
        let queue_dir = TempDir::new().unwrap();

        let q = Queue::open(queue_dir.path()).await.unwrap();

        let zip1 = write_fake_zip(&src_dir, b"a").await;
        let md1 = fake_metadata();
        let first_id = md1.bundle_id;
        q.enqueue(md1, &zip1).await.unwrap();

        // Tiny sleep so enqueue timestamps differ. Chrono's Utc::now()
        // has nanosecond resolution on all our platforms but Windows can
        // sometimes tie on fast enqueues — a millisecond of real time
        // is cheap insurance for stable ordering.
        tokio::time::sleep(Duration::from_millis(2)).await;

        let zip2_src = src_dir.path().join("bundle2.zip");
        tokio::fs::write(&zip2_src, b"b").await.unwrap();
        let md2 = fake_metadata();
        q.enqueue(md2, &zip2_src).await.unwrap();

        let next = q.next_pending().await.unwrap().unwrap();
        assert_eq!(next.metadata.bundle_id, first_id);
    }

    #[tokio::test]
    async fn mark_uploading_done_failed_transitions() {
        let src_dir = TempDir::new().unwrap();
        let queue_dir = TempDir::new().unwrap();
        let zip = write_fake_zip(&src_dir, b"x").await;

        let q = Queue::open(queue_dir.path()).await.unwrap();
        let md = fake_metadata();
        let bundle_id = md.bundle_id;
        q.enqueue(md, &zip).await.unwrap();

        q.mark_uploading(bundle_id).await.unwrap();
        assert_eq!(q.get(bundle_id).await.unwrap().state, QueueState::Uploading);

        q.mark_failed(bundle_id, "boom", Duration::from_secs(60))
            .await
            .unwrap();
        match q.get(bundle_id).await.unwrap().state {
            QueueState::Failed {
                attempts,
                last_error,
                ..
            } => {
                assert_eq!(attempts, 1);
                assert_eq!(last_error, "boom");
            }
            other => panic!("expected Failed, got {other:?}"),
        }

        // Second failure bumps the attempt counter.
        q.mark_failed(bundle_id, "boom again", Duration::from_secs(60))
            .await
            .unwrap();
        match q.get(bundle_id).await.unwrap().state {
            QueueState::Failed { attempts, .. } => assert_eq!(attempts, 2),
            other => panic!("expected Failed, got {other:?}"),
        }

        q.mark_done(bundle_id).await.unwrap();
        assert!(matches!(
            q.get(bundle_id).await.unwrap().state,
            QueueState::Done { .. }
        ));
    }

    #[tokio::test]
    async fn failed_entry_hidden_until_retry_at() {
        let src_dir = TempDir::new().unwrap();
        let queue_dir = TempDir::new().unwrap();
        let zip = write_fake_zip(&src_dir, b"x").await;

        let q = Queue::open(queue_dir.path()).await.unwrap();
        let md = fake_metadata();
        let bundle_id = md.bundle_id;
        q.enqueue(md, &zip).await.unwrap();

        q.mark_failed(bundle_id, "boom", Duration::from_secs(3600))
            .await
            .unwrap();
        // Should be invisible to next_pending right after failure.
        assert!(q.next_pending().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn recover_flips_uploading_to_pending_on_open() {
        let src_dir = TempDir::new().unwrap();
        let queue_dir = TempDir::new().unwrap();
        let zip = write_fake_zip(&src_dir, b"x").await;

        let bundle_id;
        {
            let q = Queue::open(queue_dir.path()).await.unwrap();
            let md = fake_metadata();
            bundle_id = md.bundle_id;
            q.enqueue(md, &zip).await.unwrap();
            q.mark_uploading(bundle_id).await.unwrap();
        } // drop, simulate crash

        let q2 = Queue::open(queue_dir.path()).await.unwrap();
        assert_eq!(q2.get(bundle_id).await.unwrap().state, QueueState::Pending);
    }

    /// Two `Queue` clones pointing at the same directory can enqueue
    /// different bundles concurrently without corrupting each other's
    /// state. This covers the daemon-mode topology where the scheduler
    /// clone and the drain-loop clone live side-by-side.
    #[tokio::test]
    async fn cloned_queues_on_same_dir_are_safe() {
        let src_dir = TempDir::new().unwrap();
        let queue_dir = TempDir::new().unwrap();

        let q1 = Queue::open(queue_dir.path()).await.unwrap();
        let q2 = q1.clone(); // second handle, same dir

        // Enqueue two different bundles from the two handles concurrently.
        let md1 = fake_metadata();
        let md2 = fake_metadata();
        let id1 = md1.bundle_id;
        let id2 = md2.bundle_id;

        let zip1 = write_fake_zip(&src_dir, b"bundle-1").await;
        let zip2_path = src_dir.path().join("b2.zip");
        tokio::fs::write(&zip2_path, b"bundle-2").await.unwrap();

        let (r1, r2) = tokio::join!(
            q1.enqueue(md1, &zip1),
            q2.enqueue(md2, &zip2_path),
        );
        r1.unwrap();
        r2.unwrap();

        // Both bundles must be independently readable and in Pending state.
        let e1 = q1.get(id1).await.unwrap();
        let e2 = q1.get(id2).await.unwrap();
        assert_eq!(e1.state, QueueState::Pending);
        assert_eq!(e2.state, QueueState::Pending);

        // A transition on one bundle must not affect the other.
        q2.mark_uploading(id2).await.unwrap();
        assert_eq!(q1.get(id1).await.unwrap().state, QueueState::Pending);
        assert_eq!(q1.get(id2).await.unwrap().state, QueueState::Uploading);
    }
}
