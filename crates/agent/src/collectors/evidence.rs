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
use crate::redact::Redactor;

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
    /// PII-redaction engine. Applied to every text file in the staging
    /// directory before zipping. Binary files (.evtx) are skipped.
    redactor: Redactor,
}

impl EvidenceOrchestrator {
    pub fn new(
        logs: LogsCollector,
        event_logs: EventLogsCollector,
        dsregcmd: DsRegCmdCollector,
        work_root: PathBuf,
        redactor: Redactor,
    ) -> Self {
        Self {
            logs,
            event_logs,
            dsregcmd,
            work_root,
            redactor,
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

        // Apply PII redaction to all text files in the staging dir before
        // zipping. Binary collectors (.evtx, .reg) are skipped. The
        // redactor is a no-op when `config.redaction.enabled = false`.
        if !self.redactor.is_noop() {
            self.redact_staging_dir(&staging).await?;
        }

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

    /// Walk every file in `dir` and apply PII redaction to text files
    /// in-place. Files with extensions `.evtx` or `.reg` are skipped
    /// (binary — redacting them requires a parse-extract-redact-reserialize
    /// pipeline deferred to v2). Files in [`SKIP_FILES_BY_NAME`] (currently
    /// `manifest.json`) are also skipped — see comment on the constant for
    /// why. Non-UTF-8 files are skipped silently.
    ///
    /// Files larger than [`STREAMING_THRESHOLD_BYTES`] are processed
    /// line-by-line via `BufReader` + temp file + atomic rename, capping
    /// peak memory at ~2× the longest line rather than ~3× the whole file.
    /// Smaller files take the simpler whole-file fast path.
    async fn redact_staging_dir(&self, dir: &Path) -> Result<(), EvidenceError> {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&current).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let meta = entry.file_type().await?;
                if meta.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !meta.is_file() {
                    continue;
                }
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase());
                // Skip known binary formats.
                if matches!(ext.as_deref(), Some("evtx") | Some("reg")) {
                    continue;
                }
                // Skip files we MUST NOT redact (the bundle's own manifest
                // carries a UUID-v7 `bundleId` that the GUID rule would
                // otherwise clobber, destroying forensic provenance).
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if SKIP_FILES_BY_NAME.contains(&name) {
                        continue;
                    }
                }

                // Decide between streaming (large) and whole-file (small).
                let file_size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
                let result = if file_size > STREAMING_THRESHOLD_BYTES {
                    redact_file_streaming(&path, &self.redactor).await
                } else {
                    redact_file_whole(&path, &self.redactor).await
                };

                if let Err(e) = result {
                    warn!(path = %path.display(), error = %e, "redact: failed, skipping");
                }
            }
        }
        Ok(())
    }
}

/// Files that must NEVER be touched by the redactor. `manifest.json`
/// carries a UUID-v7 `bundleId` that the GUID rule would otherwise
/// rewrite to `<GUID>` — destroying forensic provenance and making
/// operator-side debugging impossible.
const SKIP_FILES_BY_NAME: &[&str] = &["manifest.json"];

/// Files larger than this take the line-by-line streaming path. 4 MiB
/// is large enough that small ConfigMgr / Intune logs (typically
/// <500 KiB) keep the simpler whole-file fast path, but small enough
/// that the 100 MB+ logs the spec calls out are streamed.
const STREAMING_THRESHOLD_BYTES: u64 = 4 * 1024 * 1024;

/// Whole-file redact: read → apply → conditional write. The fast path
/// for small files where the simplicity is worth the ~2× peak memory.
async fn redact_file_whole(path: &Path, redactor: &Redactor) -> std::io::Result<()> {
    let raw = tokio::fs::read(path).await?;
    let text = match std::str::from_utf8(&raw) {
        Ok(s) => s,
        Err(_) => return Ok(()), // non-UTF-8 → skip silently
    };
    let redacted = redactor.apply(text);
    if let std::borrow::Cow::Owned(out) = redacted {
        tokio::fs::write(path, out.as_bytes()).await?;
    }
    Ok(())
}

/// Streaming redact: read line-by-line, write to a sibling temp file,
/// atomic rename on success. Caps peak memory at ~2× the longest line
/// rather than ~3× the whole file (read + UTF-8 validate + Cow::Owned
/// clone). Required for the "<5% perf overhead on 100 MB logs" claim
/// in the redaction spec.
async fn redact_file_streaming(
    path: &Path,
    redactor: &Redactor,
) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let tmp_path = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => path.with_file_name(format!("{name}.redact.tmp")),
        None => return Ok(()), // pathological — skip
    };

    let in_file = tokio::fs::File::open(path).await?;
    let mut reader = BufReader::new(in_file);
    let mut out_file = tokio::fs::File::create(&tmp_path).await?;

    let mut line_buf = Vec::with_capacity(4096);
    loop {
        line_buf.clear();
        let n = reader.read_until(b'\n', &mut line_buf).await?;
        if n == 0 {
            break;
        }
        // Best-effort UTF-8: a non-UTF-8 line is written through unchanged
        // (so binary blobs spliced into a log don't tank the redaction
        // pass; we don't pretend to redact bytes we can't decode).
        match std::str::from_utf8(&line_buf) {
            Ok(s) => {
                let red = redactor.apply(s);
                out_file.write_all(red.as_bytes()).await?;
            }
            Err(_) => {
                out_file.write_all(&line_buf).await?;
            }
        }
    }
    out_file.flush().await?;
    drop(out_file);
    drop(reader);

    // Atomic rename — Windows MoveFileEx(REPLACE_EXISTING) under the hood.
    tokio::fs::rename(&tmp_path, path).await?;
    Ok(())
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
    use crate::redact::Redactor;
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
            Redactor::noop(),
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
