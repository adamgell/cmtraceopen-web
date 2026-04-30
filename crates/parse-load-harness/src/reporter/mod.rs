//! Reporter — consumes BundleResults from the driver via mpsc, streams
//! bundles.jsonl to disk, accumulates summary stats, writes summary.json
//! at end of run.

pub mod compare;
pub mod system;

use crate::bundle::BundleResult;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::Receiver;

pub struct ReporterInputs {
    pub out_dir: PathBuf,
    pub run_uuid: String,
    pub seed: u64,
    pub bundle_rx: Receiver<BundleResult>,
}

pub async fn run_reporter(mut inputs: ReporterInputs) -> anyhow::Result<Summary> {
    tokio::fs::create_dir_all(&inputs.out_dir).await?;
    let bundles_path = inputs.out_dir.join("bundles.jsonl");
    let mut bundles = tokio::fs::File::create(&bundles_path).await?;

    let mut acc = StatsAccumulator::default();
    let started = std::time::Instant::now();

    while let Some(r) = inputs.bundle_rx.recv().await {
        let line = serde_json::to_string(&r)? + "\n";
        bundles.write_all(line.as_bytes()).await?;
        acc.observe(&r);
    }
    bundles.flush().await?;

    let elapsed = started.elapsed();
    let summary = acc.finalize(elapsed, inputs.run_uuid.clone(), inputs.seed);
    let summary_path = inputs.out_dir.join("summary.json");
    let summary_bytes = serde_json::to_vec_pretty(&summary)?;
    tokio::fs::write(&summary_path, summary_bytes).await?;
    Ok(summary)
}

#[derive(Default)]
struct StatsAccumulator {
    counts: BTreeMap<String, u64>,
    finalize_ms: Vec<u64>,
    parse_ms: Vec<u64>,
    errors: u64,
    total: u64,
}

impl StatsAccumulator {
    fn observe(&mut self, r: &BundleResult) {
        *self.counts.entry(r.parse_state.clone()).or_default() += 1;
        if let Some(f) = r.finalize_ms {
            self.finalize_ms.push(f);
        }
        self.parse_ms.push(r.parse_ms);
        if r.error.is_some() {
            self.errors += 1;
        }
        self.total += 1;
    }

    fn finalize(mut self, elapsed: std::time::Duration, run_uuid: String, seed: u64) -> Summary {
        Summary {
            run_uuid,
            seed,
            total_bundles: self.total,
            error_count: self.errors,
            counts_by_state: self.counts,
            finalize_ms: percentiles(&mut self.finalize_ms),
            parse_ms: percentiles(&mut self.parse_ms),
            wall_seconds: elapsed.as_secs_f64(),
            throughput_bundles_per_min: if elapsed.as_secs_f64() > 0.0 {
                (self.total as f64) * 60.0 / elapsed.as_secs_f64()
            } else {
                0.0
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct Summary {
    pub run_uuid: String,
    pub seed: u64,
    pub total_bundles: u64,
    pub error_count: u64,
    pub counts_by_state: BTreeMap<String, u64>,
    pub finalize_ms: Percentiles,
    pub parse_ms: Percentiles,
    pub wall_seconds: f64,
    pub throughput_bundles_per_min: f64,
}

#[derive(Clone, Debug, Serialize, serde::Deserialize, Default)]
pub struct Percentiles {
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub max: u64,
}

fn percentiles(values: &mut [u64]) -> Percentiles {
    if values.is_empty() {
        return Percentiles::default();
    }
    values.sort_unstable();
    Percentiles {
        p50: values[values.len() * 50 / 100],
        p95: values[(values.len() * 95 / 100).min(values.len() - 1)],
        p99: values[(values.len() * 99 / 100).min(values.len() - 1)],
        max: *values.last().unwrap(),
    }
}

pub fn load_summary(dir: &Path) -> anyhow::Result<Summary> {
    let bytes = std::fs::read(dir.join("summary.json"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(state: &str, parse_ms: u64) -> BundleResult {
        BundleResult {
            seq: 0,
            device: "d".into(),
            n_files: 1,
            bundle_bytes: 1,
            finalize_ms: Some(10),
            parse_ms,
            parse_state: state.into(),
            files_with_fallbacks: None,
            http_status: Some(201),
            error: None,
        }
    }

    #[tokio::test]
    async fn writes_jsonl_and_summary() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel::<BundleResult>(8);
        let task = tokio::spawn(run_reporter(ReporterInputs {
            out_dir: dir.path().to_path_buf(),
            run_uuid: "r".into(),
            seed: 1,
            bundle_rx: rx,
        }));
        for ms in [10u64, 20, 30, 40, 50] {
            tx.send(fake("ok", ms)).await.unwrap();
        }
        drop(tx);
        let summary = task.await.unwrap().unwrap();

        assert_eq!(summary.total_bundles, 5);
        assert_eq!(summary.counts_by_state["ok"], 5);
        assert_eq!(summary.parse_ms.max, 50);

        let bundles = std::fs::read_to_string(dir.path().join("bundles.jsonl")).unwrap();
        assert_eq!(bundles.lines().count(), 5);
    }
}
