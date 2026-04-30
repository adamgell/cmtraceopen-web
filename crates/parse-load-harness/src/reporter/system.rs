//! Periodic system-state sampler. One row per `--sample-seconds` written to
//! `system.jsonl`. Source of the row is target-specific; `Target::sample_system`
//! returns whatever JSON the target cares to expose.

use crate::target::Target;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

pub struct SystemSamplerInputs {
    pub target: Arc<dyn Target>,
    pub out_dir: PathBuf,
    pub interval: Duration,
    pub stop: oneshot::Receiver<()>,
}

pub async fn run_system_sampler(inputs: SystemSamplerInputs) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(&inputs.out_dir).await?;
    let path = inputs.out_dir.join("system.jsonl");
    let mut file = tokio::fs::File::create(&path).await?;

    let mut stop = inputs.stop;
    let mut ticker = tokio::time::interval(inputs.interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let mut row = inputs.target.sample_system().await;
                if let Some(obj) = row.as_object_mut() {
                    obj.insert(
                        "t".into(),
                        serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
                    );
                }
                let line = serde_json::to_string(&row)? + "\n";
                file.write_all(line.as_bytes()).await?;
            }
            _ = &mut stop => {
                file.flush().await?;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{Bundle, BundleResult};

    struct StubTarget;

    #[async_trait::async_trait]
    impl Target for StubTarget {
        async fn send(&self, _b: Bundle) -> BundleResult { unreachable!() }
        async fn sample_system(&self) -> serde_json::Value {
            serde_json::json!({"in_flight": 7, "rss_mb": 1024})
        }
        async fn shutdown(self: Box<Self>) -> anyhow::Result<()> { Ok(()) }
    }

    #[tokio::test]
    async fn samples_and_writes_rows() {
        let dir = tempfile::tempdir().unwrap();
        let (stop_tx, stop_rx) = oneshot::channel();
        let target = Arc::new(StubTarget);
        let task = tokio::spawn(run_system_sampler(SystemSamplerInputs {
            target,
            out_dir: dir.path().to_path_buf(),
            interval: Duration::from_millis(20),
            stop: stop_rx,
        }));
        tokio::time::sleep(Duration::from_millis(75)).await;
        let _ = stop_tx.send(());
        task.await.unwrap().unwrap();

        let body = std::fs::read_to_string(dir.path().join("system.jsonl")).unwrap();
        let rows: Vec<&str> = body.lines().collect();
        assert!(rows.len() >= 2, "expected ≥2 sampled rows, got {}", rows.len());
        let row: serde_json::Value = serde_json::from_str(rows[0]).unwrap();
        assert_eq!(row["in_flight"], 7);
        assert!(row["t"].is_string());
    }
}
