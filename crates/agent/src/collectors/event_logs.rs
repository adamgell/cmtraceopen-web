//! `EventLogsCollector` — shells out to `wevtutil epl` to export the
//! configured Windows Event Log channels into `evidence/event-logs/*.evtx`.
//!
//! Windows-only. The Linux build returns [`CollectorResult::NotSupported`]
//! so CI still compiles the bundle-level orchestrator path.

use std::path::Path;

use async_trait::async_trait;

use super::{Collector, CollectorError, CollectorManifest, CollectorResult};

/// Default channels shipped with the collector. Kept in one place so
/// policy overrides (config file / registry / ADMX once M3 lands) can
/// replace them wholesale.
pub const DEFAULT_CHANNELS: &[&str] = &[
    "Application",
    "System",
    "Security",
    "Microsoft-Windows-DeviceManagement-Enterprise-Diagnostics-Provider/Admin",
];

pub struct EventLogsCollector {
    // Read only by the Windows `collect()` impl below. The Linux stub
    // ignores `self`, so without this attribute the workspace clippy job
    // (`-D warnings`) trips on dead-code.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    channels: Vec<String>,
}

impl EventLogsCollector {
    pub fn new(channels: Vec<String>) -> Self {
        Self { channels }
    }

    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_CHANNELS.iter().map(|s| s.to_string()).collect())
    }
}

#[async_trait]
impl Collector for EventLogsCollector {
    fn name(&self) -> &'static str {
        "event-logs"
    }

    #[cfg(target_os = "windows")]
    async fn collect(&self, out_dir: &Path) -> Result<CollectorManifest, CollectorError> {
        use tracing::warn;

        let target_dir = out_dir.join("event-logs");
        super::ensure_dir(&target_dir).await?;

        let mut written: Vec<String> = Vec::new();
        let mut failures: Vec<String> = Vec::new();

        for channel in &self.channels {
            // File name: channel path with '/' and '\\' squashed to '-' so
            // the channel round-trips cleanly from filesystem to manifest.
            let safe_name: String = channel
                .chars()
                .map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c })
                .collect();
            let dest = target_dir.join(format!("{safe_name}.evtx"));

            // `wevtutil epl <channel> <path>` — "export log". Overwrites
            // the destination if it already exists (which it won't, we
            // just created the dir).
            let output = tokio::process::Command::new("wevtutil")
                .arg("epl")
                .arg(channel)
                .arg(&dest)
                .output()
                .await;

            match output {
                Ok(out) if out.status.success() => {
                    written.push(super::to_bundle_relative(out_dir, &dest));
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    warn!(%channel, status = ?out.status, stderr = %stderr, "wevtutil epl failed");
                    failures.push(format!("{channel}: exit {:?}", out.status.code()));
                }
                Err(e) => {
                    warn!(%channel, error = %e, "failed to spawn wevtutil");
                    failures.push(format!("{channel}: spawn error {e}"));
                }
            }
        }

        let (result, note) = if failures.is_empty() {
            (CollectorResult::Ok, None)
        } else if written.is_empty() {
            (
                CollectorResult::Failed {
                    message: failures.join("; "),
                },
                None,
            )
        } else {
            // Partial success: report as Ok with a note so the bundle
            // still uploads with whatever we managed to export.
            (
                CollectorResult::Ok,
                Some(format!("partial: {}", failures.join("; "))),
            )
        };

        Ok(CollectorManifest {
            name: self.name().into(),
            result,
            files: written,
            note,
        })
    }

    #[cfg(not(target_os = "windows"))]
    async fn collect(&self, _out_dir: &Path) -> Result<CollectorManifest, CollectorError> {
        Ok(CollectorManifest {
            name: self.name().into(),
            result: CollectorResult::NotSupported,
            files: Vec::new(),
            note: Some("wevtutil is Windows-only".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn linux_stub_returns_not_supported() {
        let tmp = tempfile::TempDir::new().unwrap();
        let collector = EventLogsCollector::with_defaults();
        let manifest = collector.collect(tmp.path()).await.unwrap();
        assert_eq!(manifest.result, CollectorResult::NotSupported);
        assert!(manifest.files.is_empty());
    }

    #[test]
    fn default_channels_include_application() {
        // Asserting `!DEFAULT_CHANNELS.is_empty()` would trip
        // clippy::const_is_empty (the const is statically non-empty);
        // checking for a known channel proves both shape and contents.
        assert!(DEFAULT_CHANNELS.contains(&"Application"));
    }
}
