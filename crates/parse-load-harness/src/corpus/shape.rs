//! `shape.json` — distributions the synthetic generator samples from.
//! Mined once from a real Postgres dump via the `mine-dump` binary.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Shape {
    pub files_per_bundle: PercentileBucket,
    pub bytes_per_file: PercentileBucket,
    pub parser_kind_weights: BTreeMap<String, f64>,
    pub fallback_rate_per_kb: BTreeMap<String, f64>,
    pub encoding_weights: BTreeMap<String, f64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PercentileBucket {
    pub p50: u64,
    pub p90: u64,
    pub p99: u64,
    pub max_observed: u64,
}

impl PercentileBucket {
    /// Sample a value from the bucket using a piecewise-linear inverse CDF.
    /// Quick and good enough for shape-fidelity load testing.
    pub fn sample(&self, u: f64) -> u64 {
        match u {
            u if u < 0.5 => lerp(0.0, self.p50 as f64, u / 0.5) as u64,
            u if u < 0.9 => lerp(self.p50 as f64, self.p90 as f64, (u - 0.5) / 0.4) as u64,
            u if u < 0.99 => lerp(self.p90 as f64, self.p99 as f64, (u - 0.9) / 0.09) as u64,
            u => lerp(self.p99 as f64, self.max_observed as f64, (u - 0.99) / 0.01) as u64,
        }
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

impl Shape {
    pub fn load_default() -> anyhow::Result<Self> {
        let bytes = include_bytes!("shape.json");
        Ok(serde_json::from_slice(bytes)?)
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_bucket_lerp_endpoints() {
        let b = PercentileBucket { p50: 10, p90: 100, p99: 1000, max_observed: 5000 };
        assert_eq!(b.sample(0.0), 0);
        assert_eq!(b.sample(0.5), 10);
        assert_eq!(b.sample(0.9), 100);
        assert_eq!(b.sample(0.99), 1000);
        assert!(b.sample(1.0) == 5000);
    }

    #[test]
    fn default_shape_loads_and_has_known_kinds() {
        let s = Shape::load_default().expect("default shape parses");
        assert!(s.parser_kind_weights.contains_key("Ccm"));
        assert!(s.parser_kind_weights.contains_key("Plain"));
        let total: f64 = s.parser_kind_weights.values().sum();
        assert!((total - 1.0).abs() < 0.01, "weights should sum to ~1.0, got {total}");
    }
}
