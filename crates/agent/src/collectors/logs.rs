//! `LogsCollector` — walks configured glob patterns and copies matched
//! `.log` / `.txt` files into `evidence/logs/` under the bundle root.
//!
//! Intentionally not Windows-gated: glob walking + file copy works fine on
//! any OS, and that lets the Linux integration test exercise this path
//! against a fixture directory. The default glob patterns (in
//! [`crate::config::default_log_paths`]) point at Windows trees — on
//! Linux / CI you override via `config.log_paths` or `AgentConfig::from_file`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tracing::{debug, warn};

use super::{
    ensure_dir, to_bundle_relative, Collector, CollectorError, CollectorManifest,
    CollectorResult,
};

/// Copies files matching the configured glob patterns into
/// `<out_dir>/logs/`.
pub struct LogsCollector {
    patterns: Vec<String>,
}

impl LogsCollector {
    pub fn new(patterns: Vec<String>) -> Self {
        Self { patterns }
    }
}

#[async_trait]
impl Collector for LogsCollector {
    fn name(&self) -> &'static str {
        "logs"
    }

    async fn collect(&self, out_dir: &Path) -> Result<CollectorManifest, CollectorError> {
        let logs_dir = out_dir.join("logs");
        ensure_dir(&logs_dir).await?;

        let mut written: Vec<String> = Vec::new();
        let mut walk_errors = 0usize;

        for pattern in &self.patterns {
            // `glob` is synchronous; patterns are small and this runs once
            // per collection pass, so the perf hit is irrelevant. If it
            // ever starts mattering, wrap in `tokio::task::spawn_blocking`.
            let paths = match glob::glob(pattern) {
                Ok(it) => it,
                Err(e) => {
                    // A malformed pattern is a config bug, not a runtime
                    // fault — log + skip so other patterns still produce.
                    warn!(%pattern, error = %e, "skipping malformed glob pattern");
                    continue;
                }
            };

            for entry in paths {
                let path = match entry {
                    Ok(p) => p,
                    Err(e) => {
                        // `GlobError` wraps the per-path walk failure
                        // (permissions, broken symlink). Count + keep
                        // going.
                        walk_errors += 1;
                        debug!(error = %e, "glob entry error");
                        continue;
                    }
                };

                if !path.is_file() {
                    continue;
                }

                let ext_ok = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("log") || e.eq_ignore_ascii_case("txt"))
                    .unwrap_or(false);
                if !ext_ok {
                    continue;
                }

                let Some(dest) = safe_copy_name(&logs_dir, &path) else {
                    debug!(path = %path.display(), "skipping path with empty filename");
                    continue;
                };

                if let Err(e) = tokio::fs::copy(&path, &dest).await {
                    warn!(src = %path.display(), dst = %dest.display(), error = %e, "copy failed");
                    walk_errors += 1;
                    continue;
                }

                written.push(to_bundle_relative(out_dir, &dest));
            }
        }

        let note = if walk_errors == 0 {
            None
        } else {
            Some(format!("{walk_errors} file(s) skipped due to walk / copy errors"))
        };

        Ok(CollectorManifest {
            name: self.name().into(),
            result: CollectorResult::Ok,
            files: written,
            note,
        })
    }
}

/// Pick a destination name under `dir` for the source `src`, avoiding
/// collisions by prefixing with a short hash of the parent path when
/// needed. Returns `None` if `src` has no file name (e.g. root path).
fn safe_copy_name(dir: &Path, src: &Path) -> Option<PathBuf> {
    let file_name = src.file_name()?.to_string_lossy().into_owned();
    let mut candidate = dir.join(&file_name);
    if !candidate.exists() {
        return Some(candidate);
    }
    // Collision: mix in a short hash of the full source path so two
    // different ccmexec.log files from different subfolders don't clobber
    // each other.
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(src.to_string_lossy().as_bytes());
    let short = &hex::encode(h.finalize())[..8];
    candidate = dir.join(format!("{short}-{file_name}"));
    Some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch(p: &Path, body: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[tokio::test]
    async fn collects_matching_logs_and_ignores_other_files() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();

        touch(&src.path().join("a.log"), b"hello a");
        touch(&src.path().join("deep/b.log"), b"hello b");
        touch(&src.path().join("c.txt"), b"hello c");
        touch(&src.path().join("ignored.bin"), b"\x00\x01");

        let pattern = format!(
            "{}/**/*",
            src.path().to_string_lossy().replace('\\', "/")
        );
        let collector = LogsCollector::new(vec![pattern]);
        let manifest = collector.collect(dst.path()).await.expect("collect");

        assert_eq!(manifest.result, CollectorResult::Ok);
        assert_eq!(manifest.files.len(), 3, "{:?}", manifest.files);
        for f in &manifest.files {
            assert!(f.starts_with("logs/"));
        }
    }

    #[tokio::test]
    async fn collision_does_not_clobber() {
        let src1 = TempDir::new().unwrap();
        let src2 = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();

        // Same filename, different parents.
        touch(&src1.path().join("ccmexec.log"), b"from one");
        touch(&src2.path().join("ccmexec.log"), b"from two");

        let pat1 = format!(
            "{}/*.log",
            src1.path().to_string_lossy().replace('\\', "/")
        );
        let pat2 = format!(
            "{}/*.log",
            src2.path().to_string_lossy().replace('\\', "/")
        );
        let collector = LogsCollector::new(vec![pat1, pat2]);
        let manifest = collector.collect(dst.path()).await.expect("collect");

        // Two files under logs/, both preserved.
        let entries = std::fs::read_dir(dst.path().join("logs")).unwrap();
        assert_eq!(entries.count(), 2);
        assert_eq!(manifest.files.len(), 2);
    }

    #[tokio::test]
    async fn bad_pattern_is_skipped_not_fatal() {
        let dst = TempDir::new().unwrap();
        // `***` is not a valid glob.
        let collector = LogsCollector::new(vec!["/***/***".into()]);
        let manifest = collector.collect(dst.path()).await.expect("collect");
        assert_eq!(manifest.result, CollectorResult::Ok);
        assert!(manifest.files.is_empty());
    }
}
