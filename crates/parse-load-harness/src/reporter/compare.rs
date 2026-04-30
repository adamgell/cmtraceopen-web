//! Compare two run directories and produce a delta report. Returns
//! `Comparison` (in-memory) and renders Markdown for the on-disk report.
//! Driver/CLI uses `Comparison::has_regression()` to set the exit code.

use crate::reporter::{load_summary, Summary};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::Path;

pub struct Comparison {
    pub prev: Summary,
    pub curr: Summary,
    pub threshold_pct: f64,
    pub deltas: Vec<MetricDelta>,
}

#[derive(Clone, Debug)]
pub struct MetricDelta {
    pub name: String,
    pub prev: f64,
    pub curr: f64,
    pub pct_change: f64,
    pub direction: Direction,
    pub regressed: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    HigherIsWorse,
    LowerIsWorse,
}

impl Comparison {
    pub fn build(prev_dir: &Path, curr_dir: &Path, threshold_pct: f64) -> anyhow::Result<Self> {
        let prev = load_summary(prev_dir)?;
        let curr = load_summary(curr_dir)?;
        let mut deltas = Vec::new();
        push_delta(&mut deltas, "parse_ms.p50", prev.parse_ms.p50 as f64, curr.parse_ms.p50 as f64, Direction::HigherIsWorse, threshold_pct);
        push_delta(&mut deltas, "parse_ms.p95", prev.parse_ms.p95 as f64, curr.parse_ms.p95 as f64, Direction::HigherIsWorse, threshold_pct);
        push_delta(&mut deltas, "parse_ms.p99", prev.parse_ms.p99 as f64, curr.parse_ms.p99 as f64, Direction::HigherIsWorse, threshold_pct);
        push_delta(&mut deltas, "throughput_bundles_per_min", prev.throughput_bundles_per_min, curr.throughput_bundles_per_min, Direction::LowerIsWorse, threshold_pct);
        push_delta(&mut deltas, "error_count", prev.error_count as f64, curr.error_count as f64, Direction::HigherIsWorse, threshold_pct);

        // Per-state count deltas (informational; no regression flag — counts
        // shifting can be intentional).
        let states: BTreeSet<&String> = prev.counts_by_state.keys().chain(curr.counts_by_state.keys()).collect();
        for s in states {
            let p = *prev.counts_by_state.get(s).unwrap_or(&0) as f64;
            let c = *curr.counts_by_state.get(s).unwrap_or(&0) as f64;
            deltas.push(MetricDelta {
                name: format!("count.{s}"),
                prev: p,
                curr: c,
                pct_change: pct(p, c),
                direction: Direction::HigherIsWorse,
                regressed: false,
            });
        }
        Ok(Self { prev, curr, threshold_pct, deltas })
    }

    pub fn has_regression(&self) -> bool {
        self.deltas.iter().any(|d| d.regressed)
    }

    pub fn render_markdown(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# Run comparison\n");
        let _ = writeln!(s, "- prev: `{}`", self.prev.run_uuid);
        let _ = writeln!(s, "- curr: `{}`", self.curr.run_uuid);
        let _ = writeln!(s, "- threshold: {:.1}%\n", self.threshold_pct);
        let _ = writeln!(s, "| metric | prev | curr | Δ% | flag |");
        let _ = writeln!(s, "|---|---:|---:|---:|:---:|");
        for d in &self.deltas {
            let flag = if d.regressed { "⚠️" } else { "" };
            let _ = writeln!(s, "| {} | {:.2} | {:.2} | {:+.2}% | {} |", d.name, d.prev, d.curr, d.pct_change, flag);
        }
        s
    }
}

fn pct(prev: f64, curr: f64) -> f64 {
    if prev.abs() < f64::EPSILON {
        if curr.abs() < f64::EPSILON { 0.0 } else { f64::INFINITY }
    } else {
        (curr - prev) / prev * 100.0
    }
}

fn push_delta(out: &mut Vec<MetricDelta>, name: &str, prev: f64, curr: f64, dir: Direction, threshold_pct: f64) {
    let p = pct(prev, curr);
    let regressed = match dir {
        Direction::HigherIsWorse => p > threshold_pct,
        Direction::LowerIsWorse => -p > threshold_pct,
    };
    out.push(MetricDelta {
        name: name.into(),
        prev,
        curr,
        pct_change: p,
        direction: dir,
        regressed,
    });
}
