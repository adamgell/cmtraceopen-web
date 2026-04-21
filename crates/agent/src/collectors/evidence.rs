//! Evidence-bundle orchestrator.
//!
//! Fans the four concrete collectors out in parallel, writes their output
//! into a temp staging dir, zips the result, and computes the sha256 over
//! the final zip bytes. The return value (zip path + [`BundleMetadata`])
//! is everything the upload queue needs.
//!
//! ## Parallelism
//!
//! `tokio::join!` runs all four collectors concurrently. They write to
//! disjoint subdirectories under the staging root, so there's no shared
//! mutable state — no mutex, no channel. Each collector captures its own
//! errors into the manifest rather than panicking, so a single flaky
//! collector can't poison the whole bundle.
//!
//! ## Zip format
//!
//! Store + deflate (no bzip2 / zstd / xz) to match the api-server's
//! reader (see `crates/api-server/Cargo.toml`: `zip = { ..., features =
//! ["deflate"] }`). Keeping the feature surface identical on both sides
//! means a bundle zipped here can always be unzipped there.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use common_wire::ingest::content_kind;
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use uuid::Uuid;

use super::{
    dsregcmd::DsRegCmdCollector, event_logs::EventLogsCollector, logs::LogsCollector, BundleMetadata,
    Collector, CollectorManifest,
};

/// Output of a full evidence pass. The caller (typically the main loop)
/// enqueues `(metadata, zip_path)` on the upload queue.
pub struct CollectedBundle {
    pub metadata: BundleMetadata,
    pub zip_path: PathBuf,
    /// Staging dir (temp) the collectors wrote into. Kept on the struct
    /// so the caller can clean it up AFTER enqueue has moved the zip to
    /// its final home.
    pub staging_dir: PathBuf,
}

/// Owning configuration for a single evidence-collection pass.
pub struct EvidenceOrchestrator {
    logs: LogsCollector,
    event_logs: EventLogsCollector,
    dsregcmd: DsRegCmdCollector,
    /// Parent directory under which a timestamped per-run staging dir
    /// will be created. Typically `%ProgramData%\CMTraceOpen\Agent\work`
    /// in production; tempdir in tests.
    work_root: PathBuf,
}

impl EvidenceOrchestrator {
    pub fn new(
        logs: LogsCollector,
        event_logs: EventLogsCollector,
        dsregcmd: DsRegCmdCollector,
        work_root: PathBuf,
    ) -> Self {
        Self {
            logs,
            event_logs,
            dsregcmd,
            work_root,
        }
    }

    /// Run one collection pass. Returns the zip path + metadata. The
    /// caller owns the zip from here on.
    pub async fn collect_once(&self) -> Result<CollectedBundle, EvidenceError> {
        let bundle_id = Uuid::now_v7();
        let staging = self.work_root.join(format!("bundle-{bundle_id}"));
        tokio::fs::create_dir_all(&staging).await?;

        // Each collector gets its own subdir for its output — but we pass
        // the bundle root so manifest paths are bundle-relative.
        let (m_logs, m_events, m_dsreg) = tokio::join!(
            self.logs.collect(&staging),
            self.event_logs.collect(&staging),
            self.dsregcmd.collect(&staging),
        );

        let manifests: Vec<CollectorManifest> = [m_logs, m_events, m_dsreg]
            .into_iter()
            .map(|r| {
                r.unwrap_or_else(|e| {
                    // Convert a hard collector error into a Failed
                    // manifest entry so the bundle still ships. We log
                    // the error so the operator has a breadcrumb.
                    warn!(error = %e, "collector returned error");
                    CollectorManifest {
                        name: "unknown".into(),
                        result: super::CollectorResult::Failed {
                            message: e.to_string(),
                        },
                        files: Vec::new(),
                        note: None,
                    }
                })
            })
            .collect();

        // Ship the manifest alongside the evidence so the parse worker
        // can read "what was collected" without re-walking.
        let manifest_path = staging.join("manifest.json");
        let manifest_bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "bundleId": bundle_id,
            "collectors": manifests,
        }))
        .expect("manifest serialization");
        tokio::fs::write(&manifest_path, &manifest_bytes).await?;

        // Zip the whole staging dir into a sibling file. We hand the
        // actual zip work off to a blocking task — the `zip` crate is
        // synchronous and CPU + syscall heavy, so running it on the
        // async runtime would block the reactor for the duration.
        let zip_path = self.work_root.join(format!("bundle-{bundle_id}.zip"));
        let zip_path_clone = zip_path.clone();
        let staging_clone = staging.clone();
        let (sha, size) = tokio::task::spawn_blocking(move || -> Result<(String, u64), EvidenceError> {
            zip_directory(&staging_clone, &zip_path_clone)?;
            let bytes = std::fs::read(&zip_path_clone)?;
            let mut h = Sha256::new();
            h.update(&bytes);
            let sha = hex::encode(h.finalize());
            Ok((sha, bytes.len() as u64))
        })
        .await
        .map_err(|e| EvidenceError::Join(e.to_string()))??;

        info!(%bundle_id, %sha, size_bytes = size, "evidence bundle produced");

        Ok(CollectedBundle {
            metadata: BundleMetadata {
                bundle_id,
                sha256: sha,
                size_bytes: size,
                content_kind: content_kind::EVIDENCE_ZIP.into(),
            },
            zip_path,
            staging_dir: staging,
        })
    }
}

/// Recursively zip every file under `src_dir` into `zip_path`, storing
/// paths relative to `src_dir` with forward-slash separators (POSIX-zip).
fn zip_directory(src_dir: &Path, zip_path: &Path) -> Result<(), EvidenceError> {
    let file = std::fs::File::create(zip_path)?;
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    walk_and_zip(src_dir, src_dir, &mut writer, options)?;
    writer.finish()?;
    Ok(())
}

fn walk_and_zip<W: Write + Seek>(
    root: &Path,
    dir: &Path,
    writer: &mut zip::ZipWriter<W>,
    options: zip::write::FileOptions<'_, ()>,
) -> Result<(), EvidenceError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_and_zip(root, &path, writer, options)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            writer.start_file(rel_str, options)?;
            let mut f = std::fs::File::open(&path)?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                writer.write_all(&buf[..n])?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum EvidenceError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("join error: {0}")]
    Join(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn orchestrator_produces_zip_with_logs_and_manifest() {
        // Source of fake log content.
        let src = TempDir::new().unwrap();
        std::fs::write(src.path().join("ccmexec.log"), b"entry 1\n").unwrap();
        std::fs::write(src.path().join("sidecar.txt"), b"note").unwrap();

        let work = TempDir::new().unwrap();
        let pattern = format!(
            "{}/*",
            src.path().to_string_lossy().replace('\\', "/")
        );
        let orch = EvidenceOrchestrator::new(
            LogsCollector::new(vec![pattern]),
            // On Linux these both return NotSupported; on Windows they'll
            // run for real (but this test runs under `cargo test`, so the
            // Windows hosts would actually invoke wevtutil — we scope the
            // assertions below to what we actually produced).
            EventLogsCollector::with_defaults(),
            DsRegCmdCollector::new(),
            work.path().to_path_buf(),
        );

        let bundle = orch.collect_once().await.expect("collect");

        assert!(bundle.zip_path.exists(), "zip was created");
        assert!(bundle.metadata.size_bytes > 0);
        assert_eq!(bundle.metadata.sha256.len(), 64);
        assert_eq!(bundle.metadata.content_kind, content_kind::EVIDENCE_ZIP);

        // Re-hash the file on disk and confirm it matches metadata.
        let bytes = std::fs::read(&bundle.zip_path).unwrap();
        let mut h = Sha256::new();
        h.update(&bytes);
        assert_eq!(hex::encode(h.finalize()), bundle.metadata.sha256);

        // Walk the zip and make sure manifest.json + at least one log
        // entry are present. We look for the log filename and the
        // manifest specifically.
        let reader = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(reader).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "manifest.json"),
            "manifest present: {names:?}"
        );
        assert!(
            names.iter().any(|n| n.starts_with("logs/")),
            "logs present: {names:?}"
        );
    }
}
