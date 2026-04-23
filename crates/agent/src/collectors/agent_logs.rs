//! `AgentLogsCollector` — ships the agent's own self-tracing logs alongside
//! the customer-facing evidence so operators can grep agent self-diagnostics
//! per-device via the web viewer.
//!
//! The agent's Windows-service tracing subscriber (see `service.rs`) writes
//! daily-rolling JSON lines to `%ProgramData%\CMTraceOpen\Agent\logs\agent.log`
//! via `tracing_appender::rolling::daily(...)`. That appender produces files
//! named `agent.log.YYYY-MM-DD` (the current file *includes* the date suffix;
//! there is no separate un-dated "live" file when using `daily`).
//!
//! This collector copies the last 24h of those files into the evidence bundle
//! under `agent/agent-<DATE>.log`. The path prefix `agent/` namespaces them so
//! the server-side parser registry can't confuse them with customer logs — a
//! dedicated tracing-JSON parser in the cmtraceopen submodule can land later
//! as a follow-up; today they fall through to `plain_text` / `timestamped`.
//!
//! ## Policy
//!
//! * **Window**: only files modified in the last 24 hours are shipped. The
//!   rolling appender rotates once a day, so with a 24h window we pick up
//!   today's file plus (at a day boundary) yesterday's file if it was still
//!   being written to recently.
//! * **Cap**: cumulative bytes across all shipped files are capped at 10 MB.
//!   A single day of INFO-level structured tracing is well under that, but
//!   the cap protects against a debug-logging blowout. Files are considered
//!   newest-first (by mtime): an oversized current-day file is skipped, and
//!   we continue looking at older siblings until the cap would be exceeded.
//!   Any skipped file is logged via `tracing::warn!` with the path, size, and
//!   remaining budget so operators can see what didn't ship.
//!
//! ## Platform
//!
//! Not `cfg(windows)`-gated — the file discovery + copy logic is the same on
//! any OS, and the tests below drive it with a `TempDir` under Linux CI. The
//! *default directory* is OS-specific (`%ProgramData%\...` on Windows; a
//! tempdir-friendly path on other OSes), resolved in [`Self::with_defaults`].
//! Callers that need to point at a custom directory (unit tests, alternate
//! deploys) construct via [`Self::new`].

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tracing::{debug, warn};

use super::{
    ensure_dir, to_bundle_relative, Collector, CollectorError, CollectorManifest, CollectorResult,
};

/// Cumulative byte cap across all files we copy in one pass. 10 MiB is well
/// above a day of INFO-level tracing (typically <1 MiB) but caps the blast
/// radius of a debug-logging blowout.
const MAX_CUMULATIVE_BYTES: u64 = 10 * 1024 * 1024;

/// Only include files modified within the last N hours. Keeps us from
/// ballooning the bundle with historical rotations if the agent has been
/// quietly running for weeks.
const WINDOW_HOURS: u64 = 24;

/// Copies recent agent self-log files into `<out_dir>/agent/`.
pub struct AgentLogsCollector {
    /// Directory to scan for `agent.log[.YYYY-MM-DD]` files. Typically
    /// `%ProgramData%\CMTraceOpen\Agent\logs` in production; a tempdir in
    /// tests. Held on the struct (rather than re-resolved each call) so
    /// tests can inject a per-run directory without touching env vars.
    log_dir: PathBuf,
}

impl AgentLogsCollector {
    /// Construct with an explicit log directory. Prefer [`Self::with_defaults`]
    /// in production; use this constructor for tests or alternate deploys
    /// that ship the agent logs under a non-default path.
    pub fn new(log_dir: PathBuf) -> Self {
        Self { log_dir }
    }

    /// Resolve the default log directory and construct. Mirrors the layout
    /// used by `service.rs::agent_program_data_dir` — kept as a free fn
    /// there rather than exported to avoid `pub`-ifying service internals.
    ///
    /// On Windows: `%ProgramData%\CMTraceOpen\Agent\logs` (falls back to
    /// `C:\ProgramData\...` if `%ProgramData%` is unset — mirrors the
    /// service's own fallback).
    ///
    /// On non-Windows: `/var/log/cmtraceopen/agent` as a sensible Unix-ish
    /// default. The non-Windows path is not exercised in production (the
    /// agent is a Windows service); it exists so the Linux CI build can
    /// compile and so local-dev `cargo run` on macOS / Linux can exercise
    /// the collector with env-var overrides pointing at a custom dir.
    pub fn with_defaults() -> Self {
        Self::new(default_log_dir())
    }
}

impl Default for AgentLogsCollector {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[async_trait]
impl Collector for AgentLogsCollector {
    fn name(&self) -> &'static str {
        "agent-logs"
    }

    async fn collect(&self, out_dir: &Path) -> Result<CollectorManifest, CollectorError> {
        let dest_dir = out_dir.join("agent");
        ensure_dir(&dest_dir).await?;

        // Read the directory. `NotFound` is fine — means the agent hasn't
        // written any logs yet (first-run MSI install, or we're a stub).
        let mut entries = match tokio::fs::read_dir(&self.log_dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!(
                    dir = %self.log_dir.display(),
                    "agent log dir missing; returning empty manifest",
                );
                return Ok(CollectorManifest {
                    name: self.name().into(),
                    result: CollectorResult::Ok,
                    files: Vec::new(),
                    note: Some("agent log directory missing".into()),
                });
            }
            Err(e) => {
                return Err(CollectorError::Io(e));
            }
        };

        // Collect candidates, then sort newest-first so we always ship the
        // current day before falling back to older rotated siblings.
        let now = std::time::SystemTime::now();
        let window = std::time::Duration::from_secs(WINDOW_HOURS * 3600);
        let mut candidates: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let meta = entry.metadata().await?;
            if !meta.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if !is_agent_log_name(file_name) {
                continue;
            }
            let mtime = meta.modified().unwrap_or(now);
            if let Ok(age) = now.duration_since(mtime) {
                if age > window {
                    debug!(
                        path = %path.display(),
                        age_hours = age.as_secs() / 3600,
                        "agent log outside {WINDOW_HOURS}h window; skipping",
                    );
                    continue;
                }
            }
            candidates.push((path, mtime, meta.len()));
        }

        // Newest mtime first. The cap policy below shipped-newest-wins, so
        // if we have a huge current-day file that would blow the cap, we
        // still try smaller historical rotations after it.
        candidates.sort_by(|a, b| b.1.cmp(&a.1));

        let mut written: Vec<String> = Vec::new();
        let mut cumulative: u64 = 0;
        let mut skipped_over_cap = 0usize;

        for (src, _mtime, size) in candidates {
            if cumulative.saturating_add(size) > MAX_CUMULATIVE_BYTES {
                let remaining = MAX_CUMULATIVE_BYTES.saturating_sub(cumulative);
                warn!(
                    path = %src.display(),
                    size_bytes = size,
                    remaining_budget = remaining,
                    cap_bytes = MAX_CUMULATIVE_BYTES,
                    "agent log exceeds cumulative cap; skipping",
                );
                skipped_over_cap += 1;
                continue;
            }

            let Some(dest_name) = bundle_dest_name(&src) else {
                debug!(path = %src.display(), "could not derive dest name; skipping");
                continue;
            };
            let dest = dest_dir.join(&dest_name);

            if let Err(e) = tokio::fs::copy(&src, &dest).await {
                warn!(
                    src = %src.display(),
                    dst = %dest.display(),
                    error = %e,
                    "agent log copy failed",
                );
                continue;
            }

            cumulative = cumulative.saturating_add(size);
            written.push(to_bundle_relative(out_dir, &dest));
        }

        let note = if skipped_over_cap == 0 {
            None
        } else {
            Some(format!(
                "{skipped_over_cap} file(s) skipped due to 10 MB cumulative cap"
            ))
        };

        Ok(CollectorManifest {
            name: self.name().into(),
            result: CollectorResult::Ok,
            files: written,
            note,
        })
    }
}

/// Return the default log directory the service writes to.
fn default_log_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\ProgramData"));
        base.join("CMTraceOpen").join("Agent").join("logs")
    }
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from("/var/log/cmtraceopen/agent")
    }
}

/// Does this filename match an agent-self-log rotation artefact?
///
/// Recognised patterns:
///   * `agent.log` (if someone were to switch the service to `Rotation::never`)
///   * `agent.log.YYYY-MM-DD` (what `Rotation::daily` with prefix `agent.log`
///     actually produces — the common case in production)
///   * `agent-YYYY-MM-DD.log` (the layout the evidence bundle itself uses;
///     harmless to recognise in case an operator hand-copies or an alternate
///     appender ever writes it)
fn is_agent_log_name(name: &str) -> bool {
    if name == "agent.log" {
        return true;
    }
    if let Some(rest) = name.strip_prefix("agent.log.") {
        return looks_like_iso_date(rest);
    }
    if let Some(date_part) = name
        .strip_prefix("agent-")
        .and_then(|s| s.strip_suffix(".log"))
    {
        return looks_like_iso_date(date_part);
    }
    false
}

/// Cheap YYYY-MM-DD shape check. We don't parse the date; we just want to
/// avoid matching totally unrelated files that happen to start with `agent.`.
fn looks_like_iso_date(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 10 {
        return false;
    }
    bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[0..4].iter().all(|b| b.is_ascii_digit())
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[8..10].iter().all(|b| b.is_ascii_digit())
}

/// Map an on-disk agent-log filename to its destination name inside the
/// bundle. The bundle always uses `agent-<DATE>.log`; if we can't extract a
/// date (legacy `agent.log` with no suffix) we use today's date so the
/// bundle layout is consistent.
fn bundle_dest_name(src: &Path) -> Option<String> {
    let name = src.file_name()?.to_str()?;

    // Pattern 1: `agent.log.YYYY-MM-DD` → date is the suffix.
    if let Some(date) = name.strip_prefix("agent.log.") {
        if looks_like_iso_date(date) {
            return Some(format!("agent-{date}.log"));
        }
    }

    // Pattern 2: `agent-YYYY-MM-DD.log` → already in dest format.
    if name.starts_with("agent-") && name.ends_with(".log") {
        return Some(name.to_string());
    }

    // Pattern 3: legacy `agent.log` — stamp with today's date.
    if name == "agent.log" {
        let today = current_date_stamp();
        return Some(format!("agent-{today}.log"));
    }

    None
}

/// `YYYY-MM-DD` for "today" in UTC. We use UTC rather than local time so the
/// bundle's internal layout is reproducible regardless of the endpoint's
/// timezone. The rotating appender itself writes local dates; the slight
/// mismatch is acceptable for a fallback-only code path.
fn current_date_stamp() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;

    /// Set a file's mtime so we can exercise the 24h window without
    /// sleeping. Returns the path for chaining.
    fn set_mtime(path: &Path, mtime: SystemTime) {
        let ft = filetime::FileTime::from_system_time(mtime);
        filetime::set_file_mtime(path, ft).expect("set_file_mtime");
    }

    #[tokio::test]
    async fn collects_today_and_recent_rotated_sibling() {
        let log_dir = TempDir::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        // Today's live-rolling file + yesterday's rotated sibling. Both
        // should be picked up since they're inside the 24h window.
        let today = log_dir.path().join("agent.log.2026-04-23");
        let yesterday = log_dir.path().join("agent.log.2026-04-22");
        std::fs::write(&today, b"{\"msg\":\"today\"}\n").unwrap();
        std::fs::write(&yesterday, b"{\"msg\":\"yesterday\"}\n").unwrap();
        // Force mtimes inside the window so the test doesn't depend on
        // the test-runner's wall clock.
        let now = SystemTime::now();
        set_mtime(&today, now);
        set_mtime(&yesterday, now - Duration::from_secs(3600));

        let collector = AgentLogsCollector::new(log_dir.path().to_path_buf());
        let manifest = collector.collect(out_dir.path()).await.expect("collect");

        assert_eq!(manifest.result, CollectorResult::Ok);
        // Both files shipped, remapped to `agent/agent-<DATE>.log`.
        assert_eq!(manifest.files.len(), 2, "{:?}", manifest.files);
        for f in &manifest.files {
            assert!(f.starts_with("agent/agent-"), "unexpected path: {f}");
            assert!(f.ends_with(".log"), "unexpected path: {f}");
        }

        // Total bytes under the cap.
        let mut total: u64 = 0;
        for f in &manifest.files {
            total += std::fs::metadata(out_dir.path().join(f)).unwrap().len();
        }
        assert!(total < MAX_CUMULATIVE_BYTES);
    }

    #[tokio::test]
    async fn skips_files_outside_window() {
        let log_dir = TempDir::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        // Today's file (in window) + an ancient one that should be dropped.
        let today = log_dir.path().join("agent.log.2026-04-23");
        let ancient = log_dir.path().join("agent.log.2026-01-01");
        std::fs::write(&today, b"today").unwrap();
        std::fs::write(&ancient, b"old").unwrap();
        let now = SystemTime::now();
        set_mtime(&today, now);
        set_mtime(&ancient, now - Duration::from_secs(60 * 60 * 24 * 30)); // 30 days ago

        let collector = AgentLogsCollector::new(log_dir.path().to_path_buf());
        let manifest = collector.collect(out_dir.path()).await.expect("collect");

        assert_eq!(manifest.files.len(), 1);
        assert!(manifest.files[0].contains("2026-04-23"));
    }

    #[tokio::test]
    async fn oversized_file_is_skipped_and_smaller_sibling_ships() {
        let log_dir = TempDir::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        // A 15 MB file (over the 10 MB cap) and a 10 KiB sibling. Newest
        // first ordering means we see the big one first, skip it, then
        // still ship the smaller rotated file.
        let big = log_dir.path().join("agent.log.2026-04-23");
        let small = log_dir.path().join("agent.log.2026-04-22");
        let big_bytes = vec![b'x'; 15 * 1024 * 1024];
        std::fs::write(&big, &big_bytes).unwrap();
        std::fs::write(&small, b"small log content").unwrap();
        let now = SystemTime::now();
        set_mtime(&big, now);
        set_mtime(&small, now - Duration::from_secs(3600));

        let collector = AgentLogsCollector::new(log_dir.path().to_path_buf());
        let manifest = collector.collect(out_dir.path()).await.expect("collect");

        assert_eq!(manifest.result, CollectorResult::Ok);
        // Policy: oversized current-day file is skipped; smaller sibling
        // still ships so operators get *some* signal for the day.
        assert_eq!(manifest.files.len(), 1, "{:?}", manifest.files);
        assert!(
            manifest.files[0].contains("2026-04-22"),
            "smaller sibling should have shipped: {:?}",
            manifest.files,
        );
        assert!(manifest.note.is_some(), "note should record the skip");
    }

    #[tokio::test]
    async fn missing_log_dir_returns_empty_ok() {
        let out_dir = TempDir::new().unwrap();
        let collector = AgentLogsCollector::new(PathBuf::from(
            "/definitely/does/not/exist/cmtraceopen-agent",
        ));
        let manifest = collector.collect(out_dir.path()).await.expect("collect");
        assert_eq!(manifest.result, CollectorResult::Ok);
        assert!(manifest.files.is_empty());
    }

    #[tokio::test]
    async fn ignores_unrelated_files_in_the_log_dir() {
        let log_dir = TempDir::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        // A real-looking agent log alongside noise (e.g. .tmp files from
        // a crash, an unrelated `README.txt` an operator left behind).
        let good = log_dir.path().join("agent.log.2026-04-23");
        let noise1 = log_dir.path().join("README.txt");
        let noise2 = log_dir.path().join("agent.log.swp");
        std::fs::write(&good, b"real log").unwrap();
        std::fs::write(&noise1, b"hi").unwrap();
        std::fs::write(&noise2, b"editor temp").unwrap();
        let now = SystemTime::now();
        set_mtime(&good, now);

        let collector = AgentLogsCollector::new(log_dir.path().to_path_buf());
        let manifest = collector.collect(out_dir.path()).await.expect("collect");

        assert_eq!(manifest.files.len(), 1);
        assert!(manifest.files[0].contains("2026-04-23"));
    }

    #[test]
    fn filename_matcher_accepts_expected_shapes() {
        assert!(is_agent_log_name("agent.log"));
        assert!(is_agent_log_name("agent.log.2026-04-23"));
        assert!(is_agent_log_name("agent-2026-04-23.log"));
        assert!(!is_agent_log_name("agent.log.swp"));
        assert!(!is_agent_log_name("agent.log.something"));
        assert!(!is_agent_log_name("agent-readme.log"));
        assert!(!is_agent_log_name("something-2026-04-23.log"));
        assert!(!is_agent_log_name("README.txt"));
    }

    #[test]
    fn bundle_dest_name_rewrites_rotation_suffix_form() {
        let p = PathBuf::from("/x/agent.log.2026-04-23");
        assert_eq!(
            bundle_dest_name(&p).as_deref(),
            Some("agent-2026-04-23.log")
        );
    }

    #[test]
    fn bundle_dest_name_passes_through_dest_form() {
        let p = PathBuf::from("/x/agent-2026-04-23.log");
        assert_eq!(
            bundle_dest_name(&p).as_deref(),
            Some("agent-2026-04-23.log")
        );
    }
}
