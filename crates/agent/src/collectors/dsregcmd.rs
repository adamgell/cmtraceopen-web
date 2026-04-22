//! `DsRegCmdCollector` — runs `dsregcmd /status` and captures stdout to
//! `evidence/dsregcmd-status.txt`.
//!
//! Windows-only. On Linux the collector returns
//! [`CollectorResult::NotSupported`].

use std::path::Path;

use async_trait::async_trait;

use super::{Collector, CollectorError, CollectorManifest, CollectorResult};

pub struct DsRegCmdCollector;

impl DsRegCmdCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DsRegCmdCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Collector for DsRegCmdCollector {
    fn name(&self) -> &'static str {
        "dsregcmd"
    }

    #[cfg(target_os = "windows")]
    async fn collect(&self, out_dir: &Path) -> Result<CollectorManifest, CollectorError> {
        super::ensure_dir(out_dir).await?;
        let dest = out_dir.join("dsregcmd-status.txt");

        let out = tokio::process::Command::new("dsregcmd")
            .arg("/status")
            .output()
            .await;

        let (result, files, note) = match out {
            Ok(output) => {
                // Write combined stdout + stderr — some dsregcmd versions
                // stream to stderr when /status hits a non-joined device.
                let mut combined = output.stdout.clone();
                if !output.stderr.is_empty() {
                    combined.extend_from_slice(b"\n--- stderr ---\n");
                    combined.extend_from_slice(&output.stderr);
                }
                tokio::fs::write(&dest, &combined).await?;
                let rel = super::to_bundle_relative(out_dir, &dest);
                if output.status.success() {
                    (CollectorResult::Ok, vec![rel], None)
                } else {
                    // Non-zero exit but stdout was still captured — flag as
                    // partial via note, ship what we got.
                    (
                        CollectorResult::Ok,
                        vec![rel],
                        Some(format!("dsregcmd exit {:?}", output.status.code())),
                    )
                }
            }
            Err(e) => (
                CollectorResult::Failed {
                    message: format!("failed to spawn dsregcmd: {e}"),
                },
                Vec::new(),
                None,
            ),
        };

        Ok(CollectorManifest {
            name: self.name().into(),
            result,
            files,
            note,
        })
    }

    #[cfg(not(target_os = "windows"))]
    async fn collect(&self, _out_dir: &Path) -> Result<CollectorManifest, CollectorError> {
        Ok(CollectorManifest {
            name: self.name().into(),
            result: CollectorResult::NotSupported,
            files: Vec::new(),
            note: Some("dsregcmd is Windows-only".into()),
        })
    }
}

#[cfg(test)]
#[cfg(not(target_os = "windows"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn linux_stub_returns_not_supported() {
        let tmp = tempfile::TempDir::new().unwrap();
        let collector = DsRegCmdCollector::new();
        let manifest = collector.collect(tmp.path()).await.unwrap();
        assert_eq!(manifest.result, CollectorResult::NotSupported);
    }
}
