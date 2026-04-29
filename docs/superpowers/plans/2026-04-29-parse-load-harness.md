# Parse Load Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `crates/parse-load-harness/`, a first-class Rust crate that drives the api-server's parse pipeline against three targets (in-process / local-http / remote-http) with three load shapes (benchmark / breaking point / soak), producing JSONL output for post-hoc analysis and regression detection.

**Architecture:** New workspace member with trait-based seams (`Corpus`, `Target`, `Auth`) so the driver is target-agnostic. Synthetic bundle generator informed by distributions mined from the pilot SQL dump. Producer/consumer reporter pattern (mpsc) so disk I/O never blocks the load loop. Companion `mine-dump` binary turns the 1.5GB pilot dump into a checked-in `shape.json`.

**Tech Stack:** Rust 1.x, tokio (async runtime + mpsc + Semaphore + JoinSet), clap (CLI), reqwest (HTTP client), rcgen (test CA), zip (archive build), sqlx (in-process target's PG, mine-dump's PG read), sysinfo (RSS sampling), serde_json (output schemas).

**Spec:** [`docs/superpowers/specs/2026-04-29-parse-load-harness-design.md`](../specs/2026-04-29-parse-load-harness-design.md)

**File structure:**

```
crates/parse-load-harness/
├── Cargo.toml
├── README.md
├── src/
│   ├── main.rs                  CLI dispatch
│   ├── lib.rs                   re-exports for the bin
│   ├── config.rs                RunConfig, validation
│   ├── bundle.rs                Bundle struct + BundleResult
│   ├── corpus/
│   │   ├── mod.rs               trait Corpus
│   │   ├── synthetic.rs         dump-informed generator
│   │   ├── replay.rs            directory replay
│   │   ├── shape.rs             shape.json schema + loader
│   │   ├── shape.json           default distributions
│   │   └── profiles/            override JSONs
│   │       ├── many-small.json
│   │       ├── giant-file.json
│   │       ├── unicode-heavy.json
│   │       ├── broken-binary.json
│   │       └── near-cap.json
│   ├── target/
│   │   ├── mod.rs               trait Target + dispatch
│   │   ├── in_process.rs        AppState + parse_session
│   │   ├── local_http.rs        compose lifecycle wrapper
│   │   └── remote_http.rs       reqwest client + ingest protocol
│   ├── auth/
│   │   ├── mod.rs               trait Auth
│   │   ├── header.rs            X-Device-Id
│   │   └── mtls.rs              rcgen test CA
│   ├── driver.rs                load loop
│   └── reporter/
│       ├── mod.rs               bundles.jsonl writer + summary.json
│       ├── system.rs            periodic /metrics sampler
│       └── compare.rs           --compare-to delta report
├── bin/
│   └── mine-dump.rs             SQL dump → shape.json
├── infra/
│   └── compose/load-test.yml    local-http compose stack
└── tests/
    ├── synthetic_generator.rs
    ├── driver_shape.rs
    └── compare_report.rs
```

---

## Task 1: Scaffold the crate

**Files:**
- Create: `crates/parse-load-harness/Cargo.toml`
- Create: `crates/parse-load-harness/src/lib.rs`
- Create: `crates/parse-load-harness/src/main.rs`
- Modify: `Cargo.toml` (workspace `members`)

- [ ] **Step 1: Add the crate to the workspace**

Read `Cargo.toml` at the repo root. The `[workspace]` section currently lists three members. Add `crates/parse-load-harness`:

```toml
members = [
    "crates/api-server",
    "crates/common-wire",
    "crates/agent",
    "crates/parse-load-harness",
]
```

- [ ] **Step 2: Create the crate manifest**

Write `crates/parse-load-harness/Cargo.toml`:

```toml
[package]
name = "parse-load-harness"
version = "0.1.0"
edition = "2021"
publish = false

[[bin]]
name = "parse-load-harness"
path = "src/main.rs"

[[bin]]
name = "mine-dump"
path = "bin/mine-dump.rs"

[dependencies]
api-server = { path = "../api-server" }
common-wire = { path = "../common-wire" }
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "time", "fs", "process", "io-util"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls-native-roots-no-provider", "multipart", "stream"] }
rustls = { version = "0.23", default-features = false, features = ["ring"] }
rcgen = "0.13"
zip = { version = "2", default-features = false, features = ["deflate"] }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "tls-none", "postgres", "macros", "chrono", "uuid"] }
sysinfo = "0.32"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["serde", "v4", "v7"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
rand = "0.8"
rand_chacha = "0.3"
anyhow = "1"
thiserror = "1"
futures = "0.3"
tempfile = "3"

[dev-dependencies]
proptest = "1"
```

- [ ] **Step 3: Create skeleton lib.rs**

Write `crates/parse-load-harness/src/lib.rs`:

```rust
//! Parse load harness — drives the api-server's parse pipeline against
//! three targets with three load shapes, streaming results to disk for
//! post-hoc analysis. See `docs/superpowers/specs/2026-04-29-parse-load-harness-design.md`.

pub mod auth;
pub mod bundle;
pub mod config;
pub mod corpus;
pub mod driver;
pub mod reporter;
pub mod target;
```

- [ ] **Step 4: Create stub main.rs**

Write `crates/parse-load-harness/src/main.rs`:

```rust
fn main() -> anyhow::Result<()> {
    println!("parse-load-harness: scaffold ok");
    Ok(())
}
```

- [ ] **Step 5: Stub each module so `cargo check` passes**

For each of the modules referenced in `lib.rs` (`auth`, `bundle`, `config`, `corpus`, `driver`, `reporter`, `target`), create a file with just `//! TODO: implement`. For directory modules (`auth/`, `corpus/`, `target/`, `reporter/`), create the directory and a `mod.rs` containing only the doc comment. List of files to create empty:

- `crates/parse-load-harness/src/auth/mod.rs`
- `crates/parse-load-harness/src/auth/header.rs`
- `crates/parse-load-harness/src/auth/mtls.rs`
- `crates/parse-load-harness/src/bundle.rs`
- `crates/parse-load-harness/src/config.rs`
- `crates/parse-load-harness/src/corpus/mod.rs`
- `crates/parse-load-harness/src/corpus/replay.rs`
- `crates/parse-load-harness/src/corpus/shape.rs`
- `crates/parse-load-harness/src/corpus/synthetic.rs`
- `crates/parse-load-harness/src/driver.rs`
- `crates/parse-load-harness/src/reporter/mod.rs`
- `crates/parse-load-harness/src/reporter/compare.rs`
- `crates/parse-load-harness/src/reporter/system.rs`
- `crates/parse-load-harness/src/target/in_process.rs`
- `crates/parse-load-harness/src/target/local_http.rs`
- `crates/parse-load-harness/src/target/mod.rs`
- `crates/parse-load-harness/src/target/remote_http.rs`

In each subdirectory `mod.rs`, declare the sibling files. For example, `auth/mod.rs`:

```rust
//! Auth strategies for HTTP targets.

pub mod header;
pub mod mtls;
```

Repeat the pattern for `corpus/mod.rs` (declares `replay`, `shape`, `synthetic`), `target/mod.rs` (declares `in_process`, `local_http`, `remote_http`), and `reporter/mod.rs` (declares `compare`, `system`).

- [ ] **Step 6: Stub the mine-dump binary**

Write `crates/parse-load-harness/bin/mine-dump.rs`:

```rust
fn main() -> anyhow::Result<()> {
    println!("mine-dump: scaffold ok");
    Ok(())
}
```

- [ ] **Step 7: Verify the workspace builds**

Run: `cargo check -p parse-load-harness`
Expected: `Finished` with zero errors. Warnings about unused modules are fine.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/parse-load-harness/
git commit -m "feat(harness): scaffold parse-load-harness crate"
```

---

## Task 2: Bundle data types

**Files:**
- Modify: `crates/parse-load-harness/src/bundle.rs`

- [ ] **Step 1: Write the failing test**

In `crates/parse-load-harness/src/bundle.rs`:

```rust
//! Bundle = one unit of work the harness sends to a target. Plain data so
//! it crosses async boundaries cheaply.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// One bundle ready to be shipped to a Target. The harness produces these
/// (synthetically or replayed); the target consumes them.
#[derive(Clone, Debug)]
pub struct Bundle {
    /// Sequence number within the run; uniquely identifies the bundle in
    /// `bundles.jsonl`.
    pub seq: u64,
    /// Logical device this bundle is "from" (`LOAD-TEST-<run>-<seq%n>`).
    pub device_id: String,
    /// Wire-protocol bundle id (uuid v7) — used by the agent's finalize
    /// payload.
    pub bundle_id: uuid::Uuid,
    /// Number of log files inside the zip. Recorded for analysis.
    pub n_files: u32,
    /// Raw zip bytes. Always ≤ MAX_EVIDENCE_ZIP_BYTES (50 MiB).
    pub zip_bytes: Vec<u8>,
}

/// Per-bundle outcome. Written to `bundles.jsonl` by the reporter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BundleResult {
    pub seq: u64,
    pub device: String,
    pub n_files: u32,
    pub bundle_bytes: u64,
    /// Time from "start sending" to "finalize response received".
    /// `None` for in-process target (no HTTP layer).
    pub finalize_ms: Option<u64>,
    /// Time from finalize to `parse_state != "pending"`. Always populated
    /// (the harness polls until terminal).
    pub parse_ms: u64,
    pub parse_state: String,
    pub files_with_fallbacks: Option<u32>,
    pub http_status: Option<u16>,
    pub error: Option<String>,
}

impl BundleResult {
    pub fn elapsed(&self) -> Duration {
        Duration::from_millis(self.finalize_ms.unwrap_or(0) + self.parse_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_result_serializes_with_expected_keys() {
        let r = BundleResult {
            seq: 42,
            device: "LOAD-TEST-A-0001".into(),
            n_files: 24,
            bundle_bytes: 4_812_000,
            finalize_ms: Some(120),
            parse_ms: 3450,
            parse_state: "ok-with-fallbacks".into(),
            files_with_fallbacks: Some(3),
            http_status: Some(201),
            error: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["seq"], 42);
        assert_eq!(v["device"], "LOAD-TEST-A-0001");
        assert_eq!(v["parse_state"], "ok-with-fallbacks");
        assert_eq!(v["finalize_ms"], 120);
        assert_eq!(v["error"], serde_json::Value::Null);
    }

    #[test]
    fn elapsed_sums_finalize_and_parse() {
        let r = BundleResult {
            seq: 0,
            device: "d".into(),
            n_files: 0,
            bundle_bytes: 0,
            finalize_ms: Some(100),
            parse_ms: 200,
            parse_state: "ok".into(),
            files_with_fallbacks: None,
            http_status: None,
            error: None,
        };
        assert_eq!(r.elapsed(), Duration::from_millis(300));
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p parse-load-harness --lib bundle`
Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/parse-load-harness/src/bundle.rs
git commit -m "feat(harness): bundle + bundle-result types"
```

---

## Task 3: RunConfig + CLI parsing

**Files:**
- Modify: `crates/parse-load-harness/src/config.rs`
- Modify: `crates/parse-load-harness/src/main.rs`

- [ ] **Step 1: Write the failing tests**

Write `crates/parse-load-harness/src/config.rs`:

```rust
//! `RunConfig` is the validated, normalized form of the CLI args. The CLI
//! struct (`Args`) is a clap-driven raw form; `RunConfig::from_args` does
//! the validation that clap can't express.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "parse-load-harness", version)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Drive a load run.
    Run(Args),
    /// Mine a Postgres SQL dump into shape.json.
    MineDump(MineDumpArgs),
    /// Delete rows tagged with a given run-uuid (remote-http only).
    Clean(CleanArgs),
}

#[derive(Parser, Debug)]
pub struct Args {
    #[arg(long, value_enum)]
    pub target: TargetKind,

    #[arg(long)]
    pub target_url: Option<String>,

    #[arg(long, value_enum, default_value_t = AuthKind::Header)]
    pub auth: AuthKind,

    #[arg(long)]
    pub corpus: Option<PathBuf>,

    /// Either a profile name (`many-small`, `giant-file`, …) or a path to
    /// a custom shape JSON.
    #[arg(long)]
    pub shape: Option<String>,

    #[arg(long)]
    pub concurrency: u32,

    #[arg(long, default_value_t = 0)]
    pub ramp_seconds: u32,

    #[arg(long)]
    pub total_bundles: Option<u64>,

    #[arg(long)]
    pub soak_minutes: Option<u32>,

    #[arg(long)]
    pub seed: Option<u64>,

    #[arg(long)]
    pub out: Option<PathBuf>,

    #[arg(long)]
    pub compare_to: Option<PathBuf>,

    #[arg(long, default_value_t = 10.0)]
    pub regression_threshold: f64,

    #[arg(long, default_value_t = false)]
    pub strict: bool,

    #[arg(long)]
    pub max_rss_mb: Option<u64>,

    #[arg(long)]
    pub max_error_rate: Option<f64>,

    #[arg(long, default_value_t = false)]
    pub no_cleanup: bool,

    #[arg(long, default_value_t = 5)]
    pub sample_seconds: u32,

    #[arg(long)]
    pub in_process_pg: Option<String>,

    #[arg(long)]
    pub image: Option<String>,
}

#[derive(Parser, Debug)]
pub struct MineDumpArgs {
    pub sql_file: PathBuf,
    #[arg(long, default_value = "shape.json")]
    pub out: PathBuf,
}

#[derive(Parser, Debug)]
pub struct CleanArgs {
    #[arg(long, value_enum)]
    pub target: TargetKind,
    #[arg(long)]
    pub target_url: Option<String>,
    #[arg(long)]
    pub cleanup_run: uuid::Uuid,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum TargetKind {
    InProcess,
    LocalHttp,
    RemoteHttp,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum AuthKind {
    Header,
    Mtls,
}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub run_uuid: uuid::Uuid,
    pub args: Args,
    pub seed: u64,
    pub stop: StopCondition,
    pub out_dir: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub enum StopCondition {
    TotalBundles(u64),
    SoakDeadline(Duration),
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("--target=remote-http requires --target-url")]
    RemoteHttpMissingUrl,
    #[error("--auth=mtls is only supported for --target=local-http")]
    MtlsNotSupportedOnTarget,
    #[error("must specify exactly one of --total-bundles or --soak-minutes")]
    StopConditionUnclear,
    #[error("--concurrency must be ≥ 1")]
    ZeroConcurrency,
}

impl RunConfig {
    pub fn from_args(args: Args) -> Result<Self, ConfigError> {
        if args.concurrency == 0 {
            return Err(ConfigError::ZeroConcurrency);
        }
        if args.target == TargetKind::RemoteHttp && args.target_url.is_none() {
            return Err(ConfigError::RemoteHttpMissingUrl);
        }
        if args.auth == AuthKind::Mtls && args.target != TargetKind::LocalHttp {
            return Err(ConfigError::MtlsNotSupportedOnTarget);
        }
        let stop = match (args.total_bundles, args.soak_minutes) {
            (Some(n), None) => StopCondition::TotalBundles(n),
            (None, Some(m)) => StopCondition::SoakDeadline(Duration::from_secs(m as u64 * 60)),
            _ => return Err(ConfigError::StopConditionUnclear),
        };
        let run_uuid = uuid::Uuid::new_v4();
        let seed = args.seed.unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
        });
        let out_dir = args.out.clone().unwrap_or_else(|| {
            let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
            PathBuf::from(format!("./runs/{ts}-{}", run_uuid))
        });
        Ok(Self { run_uuid, args, seed, stop, out_dir })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args() -> Args {
        Args {
            target: TargetKind::InProcess,
            target_url: None,
            auth: AuthKind::Header,
            corpus: None,
            shape: None,
            concurrency: 4,
            ramp_seconds: 0,
            total_bundles: Some(10),
            soak_minutes: None,
            seed: Some(1),
            out: None,
            compare_to: None,
            regression_threshold: 10.0,
            strict: false,
            max_rss_mb: None,
            max_error_rate: None,
            no_cleanup: false,
            sample_seconds: 5,
            in_process_pg: None,
            image: None,
        }
    }

    #[test]
    fn rejects_zero_concurrency() {
        let mut a = base_args();
        a.concurrency = 0;
        assert!(matches!(RunConfig::from_args(a), Err(ConfigError::ZeroConcurrency)));
    }

    #[test]
    fn remote_http_requires_url() {
        let mut a = base_args();
        a.target = TargetKind::RemoteHttp;
        assert!(matches!(RunConfig::from_args(a), Err(ConfigError::RemoteHttpMissingUrl)));
    }

    #[test]
    fn mtls_rejected_on_remote_http() {
        let mut a = base_args();
        a.target = TargetKind::RemoteHttp;
        a.target_url = Some("https://x".into());
        a.auth = AuthKind::Mtls;
        assert!(matches!(RunConfig::from_args(a), Err(ConfigError::MtlsNotSupportedOnTarget)));
    }

    #[test]
    fn requires_exactly_one_stop_condition() {
        let mut a = base_args();
        a.total_bundles = None;
        a.soak_minutes = None;
        assert!(matches!(RunConfig::from_args(a), Err(ConfigError::StopConditionUnclear)));

        let mut b = base_args();
        b.total_bundles = Some(10);
        b.soak_minutes = Some(5);
        assert!(matches!(RunConfig::from_args(b), Err(ConfigError::StopConditionUnclear)));
    }

    #[test]
    fn soak_minutes_becomes_duration() {
        let mut a = base_args();
        a.total_bundles = None;
        a.soak_minutes = Some(2);
        let cfg = RunConfig::from_args(a).unwrap();
        match cfg.stop {
            StopCondition::SoakDeadline(d) => assert_eq!(d.as_secs(), 120),
            _ => panic!("expected SoakDeadline"),
        }
    }

    #[test]
    fn explicit_seed_is_preserved() {
        let mut a = base_args();
        a.seed = Some(0xDEADBEEF);
        let cfg = RunConfig::from_args(a).unwrap();
        assert_eq!(cfg.seed, 0xDEADBEEF);
    }
}
```

- [ ] **Step 2: Run config tests**

Run: `cargo test -p parse-load-harness --lib config`
Expected: 6 tests pass.

- [ ] **Step 3: Wire main.rs to parse + dispatch**

Replace `crates/parse-load-harness/src/main.rs`:

```rust
use clap::Parser;
use parse_load_harness::config::{Cli, Cmd, RunConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "parse_load_harness=info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run(args) => {
            let cfg = RunConfig::from_args(args)?;
            tracing::info!(run = %cfg.run_uuid, seed = cfg.seed, "run config validated");
            anyhow::bail!("run command not yet implemented");
        }
        Cmd::MineDump(_) => anyhow::bail!("mine-dump command not yet implemented"),
        Cmd::Clean(_) => anyhow::bail!("clean command not yet implemented"),
    }
}
```

- [ ] **Step 4: Verify CLI dispatch fails-loud where expected**

Run: `cargo run -p parse-load-harness -- run --target in-process --concurrency 1 --total-bundles 1`
Expected: prints log line "run config validated", then errors with "run command not yet implemented" (exit 1). Confirms the CLI parses and validation runs.

- [ ] **Step 5: Commit**

```bash
git add crates/parse-load-harness/src/config.rs crates/parse-load-harness/src/main.rs
git commit -m "feat(harness): RunConfig validation + clap CLI"
```

---

## Task 4: Shape config + synthetic bundle generator

**Files:**
- Modify: `crates/parse-load-harness/src/corpus/shape.rs`
- Modify: `crates/parse-load-harness/src/corpus/mod.rs`
- Modify: `crates/parse-load-harness/src/corpus/synthetic.rs`
- Create: `crates/parse-load-harness/src/corpus/shape.json`
- Create: `crates/parse-load-harness/tests/synthetic_generator.rs`

- [ ] **Step 1: Define the shape schema**

Write `crates/parse-load-harness/src/corpus/shape.rs`:

```rust
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
```

- [ ] **Step 2: Create the default shape.json**

Write `crates/parse-load-harness/src/corpus/shape.json`:

```json
{
  "files_per_bundle": { "p50": 24, "p90": 38, "p99": 62, "max_observed": 87 },
  "bytes_per_file": { "p50": 4200, "p90": 280000, "p99": 4800000, "max_observed": 49000000 },
  "parser_kind_weights": {
    "Ccm": 0.42,
    "IisW3c": 0.07,
    "Plain": 0.31,
    "Timestamped": 0.12,
    "TracingJson": 0.05,
    "Setup": 0.03
  },
  "fallback_rate_per_kb": {
    "Ccm": 0.00005,
    "IisW3c": 0.0,
    "Plain": 0.0008,
    "Timestamped": 0.0001,
    "TracingJson": 0.0,
    "Setup": 0.0002
  },
  "encoding_weights": {
    "Utf8": 0.86,
    "Utf16Le": 0.13,
    "Other": 0.01
  }
}
```

These are seed-quality numbers — Task 16's `mine-dump` produces the real ones, and the operator overwrites this file with its output.

- [ ] **Step 3: Define the Corpus trait**

In `crates/parse-load-harness/src/corpus/mod.rs`:

```rust
//! Corpus = bundle source. Synthetic generator and directory-replay both
//! implement this; the driver doesn't know which it's getting.

pub mod replay;
pub mod shape;
pub mod synthetic;

use crate::bundle::Bundle;

#[async_trait::async_trait]
pub trait Corpus: Send + Sync {
    /// Produce the next bundle. The driver tags `seq` and `device_id`
    /// before sending; the corpus only needs to fill `bundle_id`,
    /// `n_files`, and `zip_bytes`.
    async fn next(&mut self) -> anyhow::Result<Bundle>;
}
```

Add `async-trait = "0.1"` to `Cargo.toml` `[dependencies]`.

- [ ] **Step 4: Implement the synthetic generator (basic loop)**

Write `crates/parse-load-harness/src/corpus/synthetic.rs`:

```rust
//! Synthetic bundle generator. Samples per-bundle file counts, parser-kind
//! mix, and per-file sizes from a `Shape`, then writes lines templated per
//! parser kind so the api-server's parser does meaningful work.

use crate::bundle::Bundle;
use crate::corpus::shape::Shape;
use crate::corpus::Corpus;
use rand::distributions::WeightedIndex;
use rand::prelude::*;
use rand_chacha::ChaCha20Rng;
use std::io::Write;

const BUNDLE_SIZE_CAP: u64 = 49 * 1024 * 1024; // stay under the 50 MiB API cap

pub struct SyntheticCorpus {
    shape: Shape,
    rng: ChaCha20Rng,
    parser_kind_keys: Vec<String>,
    parser_kind_dist: WeightedIndex<f64>,
    encoding_keys: Vec<String>,
    encoding_dist: WeightedIndex<f64>,
}

impl SyntheticCorpus {
    pub fn new(shape: Shape, seed: u64) -> anyhow::Result<Self> {
        let parser_kind_keys: Vec<String> = shape.parser_kind_weights.keys().cloned().collect();
        let parser_kind_weights: Vec<f64> = parser_kind_keys
            .iter()
            .map(|k| shape.parser_kind_weights[k])
            .collect();
        let parser_kind_dist = WeightedIndex::new(&parser_kind_weights)?;

        let encoding_keys: Vec<String> = shape.encoding_weights.keys().cloned().collect();
        let encoding_weights: Vec<f64> = encoding_keys
            .iter()
            .map(|k| shape.encoding_weights[k])
            .collect();
        let encoding_dist = WeightedIndex::new(&encoding_weights)?;

        Ok(Self {
            shape,
            rng: ChaCha20Rng::seed_from_u64(seed),
            parser_kind_keys,
            parser_kind_dist,
            encoding_keys,
            encoding_dist,
        })
    }

    fn render_line(kind: &str, i: usize) -> String {
        match kind {
            "Ccm" => format!(
                "<![LOG[Synthetic CCM line {i}]LOG]!><time=\"12:34:56.000+000\" date=\"04-29-2026\" component=\"SyntheticComp\" context=\"\" type=\"1\" thread=\"100\" file=\"synth.cpp:42\">"
            ),
            "IisW3c" => format!(
                "2026-04-29 12:34:{i:02} W3SVC1 GET /path - 80 - 10.0.0.{i} Mozilla/5.0 200 0 0 42"
            ),
            "Timestamped" => format!("[2026-04-29T12:34:{i:02}Z] INFO synthesizer: line {i}"),
            "TracingJson" => format!("{{\"timestamp\":\"2026-04-29T12:34:{i:02}Z\",\"level\":\"INFO\",\"target\":\"synth\",\"message\":\"line {i}\"}}"),
            "Setup" => format!("MIF: Action 'SyntheticAction' returned 0 ({i})"),
            _ => format!("synthetic plain line {i}\n"),
        }
    }

    fn build_zip(&mut self, n_files: u32) -> anyhow::Result<(Vec<u8>, u32)> {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let mut size_so_far: u64 = 0;
            let mut emitted: u32 = 0;
            for i in 0..n_files {
                let kind_idx = self.parser_kind_dist.sample(&mut self.rng);
                let kind = self.parser_kind_keys[kind_idx].clone();
                let target_bytes = self.shape.bytes_per_file.sample(self.rng.r#gen::<f64>());
                let target_bytes = target_bytes.min(BUNDLE_SIZE_CAP - size_so_far);
                if target_bytes < 32 {
                    break;
                }
                let mut body = String::with_capacity(target_bytes as usize);
                let mut line_i = 0usize;
                while (body.len() as u64) < target_bytes {
                    body.push_str(&Self::render_line(&kind, line_i));
                    body.push('\n');
                    line_i += 1;
                }
                let path = format!("logs/{kind}/synth-{i:03}.log");
                zw.start_file(&path, opts)?;
                zw.write_all(body.as_bytes())?;
                size_so_far = size_so_far.saturating_add(body.len() as u64);
                emitted += 1;
                if size_so_far >= BUNDLE_SIZE_CAP {
                    break;
                }
            }
            zw.finish()?;
            // The encoding dist is sampled to keep its draw stream stable
            // even though we don't yet vary encoding. Future work: emit
            // UTF-16LE bodies when sampled "Utf16Le".
            let _ = self.encoding_dist.sample(&mut self.rng);
            let _ = &self.encoding_keys;
            let _ = emitted;
        }
        let n_files_actually = {
            let cursor = std::io::Cursor::new(&buf);
            zip::ZipArchive::new(cursor)?.len() as u32
        };
        Ok((buf, n_files_actually))
    }
}

#[async_trait::async_trait]
impl Corpus for SyntheticCorpus {
    async fn next(&mut self) -> anyhow::Result<Bundle> {
        let n_files = self.shape.files_per_bundle.sample(self.rng.r#gen::<f64>()) as u32;
        let n_files = n_files.max(1);
        let (zip_bytes, n_files_actual) = self.build_zip(n_files)?;
        Ok(Bundle {
            seq: 0,                       // driver fills this in
            device_id: String::new(),     // driver fills this in
            bundle_id: uuid::Uuid::now_v7(),
            n_files: n_files_actual,
            zip_bytes,
        })
    }
}
```

- [ ] **Step 5: Write the integration test**

Write `crates/parse-load-harness/tests/synthetic_generator.rs`:

```rust
use parse_load_harness::corpus::shape::Shape;
use parse_load_harness::corpus::synthetic::SyntheticCorpus;
use parse_load_harness::corpus::Corpus;

#[tokio::test]
async fn fixed_seed_produces_byte_identical_bundles() {
    let shape = Shape::load_default().unwrap();
    let mut a = SyntheticCorpus::new(shape.clone(), 0xC0FFEE).unwrap();
    let mut b = SyntheticCorpus::new(shape, 0xC0FFEE).unwrap();
    for _ in 0..3 {
        let ba = a.next().await.unwrap();
        let bb = b.next().await.unwrap();
        assert_eq!(ba.zip_bytes, bb.zip_bytes, "same seed must produce same bytes");
        assert_eq!(ba.n_files, bb.n_files);
    }
}

#[tokio::test]
async fn bundles_stay_under_50mib_cap() {
    let shape = Shape::load_default().unwrap();
    let mut c = SyntheticCorpus::new(shape, 1).unwrap();
    for _ in 0..16 {
        let b = c.next().await.unwrap();
        assert!(
            b.zip_bytes.len() < 50 * 1024 * 1024,
            "bundle was {} bytes",
            b.zip_bytes.len()
        );
        assert!(b.n_files > 0);
    }
}

#[tokio::test]
async fn produced_bundle_is_a_valid_zip_with_log_entries() {
    let shape = Shape::load_default().unwrap();
    let mut c = SyntheticCorpus::new(shape, 7).unwrap();
    let b = c.next().await.unwrap();
    let cursor = std::io::Cursor::new(&b.zip_bytes);
    let mut z = zip::ZipArchive::new(cursor).expect("valid zip");
    assert!(z.len() > 0);
    let mut found_log = false;
    for i in 0..z.len() {
        let entry = z.by_index(i).unwrap();
        if entry.name().ends_with(".log") {
            found_log = true;
            break;
        }
    }
    assert!(found_log, "expected at least one .log file in the bundle");
}
```

- [ ] **Step 6: Run synthetic generator tests**

Run: `cargo test -p parse-load-harness --test synthetic_generator`
Expected: 3 tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/parse-load-harness/src/corpus/ crates/parse-load-harness/tests/synthetic_generator.rs crates/parse-load-harness/Cargo.toml
git commit -m "feat(harness): shape config + synthetic bundle generator"
```

---

## Task 5: Profile overrides + replay corpus

**Files:**
- Create: `crates/parse-load-harness/src/corpus/profiles/many-small.json`
- Create: `crates/parse-load-harness/src/corpus/profiles/giant-file.json`
- Create: `crates/parse-load-harness/src/corpus/profiles/unicode-heavy.json`
- Create: `crates/parse-load-harness/src/corpus/profiles/broken-binary.json`
- Create: `crates/parse-load-harness/src/corpus/profiles/near-cap.json`
- Modify: `crates/parse-load-harness/src/corpus/shape.rs` (profile resolver)
- Modify: `crates/parse-load-harness/src/corpus/replay.rs`

- [ ] **Step 1: Write profile JSONs**

Each profile is a complete `Shape` (the loader does not merge — full overrides only, to keep things obvious). Five files:

`many-small.json`:
```json
{
  "files_per_bundle": { "p50": 200, "p90": 240, "p99": 280, "max_observed": 320 },
  "bytes_per_file": { "p50": 256, "p90": 1024, "p99": 4096, "max_observed": 8192 },
  "parser_kind_weights": { "Plain": 1.0 },
  "fallback_rate_per_kb": { "Plain": 0.0 },
  "encoding_weights": { "Utf8": 1.0 }
}
```

`giant-file.json`:
```json
{
  "files_per_bundle": { "p50": 1, "p90": 1, "p99": 1, "max_observed": 1 },
  "bytes_per_file": { "p50": 49000000, "p90": 49000000, "p99": 49000000, "max_observed": 49000000 },
  "parser_kind_weights": { "IisW3c": 1.0 },
  "fallback_rate_per_kb": { "IisW3c": 0.0 },
  "encoding_weights": { "Utf8": 1.0 }
}
```

`unicode-heavy.json`:
```json
{
  "files_per_bundle": { "p50": 24, "p90": 38, "p99": 62, "max_observed": 87 },
  "bytes_per_file": { "p50": 4200, "p90": 280000, "p99": 4800000, "max_observed": 49000000 },
  "parser_kind_weights": { "Ccm": 0.42, "Plain": 0.58 },
  "fallback_rate_per_kb": { "Ccm": 0.00005, "Plain": 0.0008 },
  "encoding_weights": { "Utf16Le": 1.0 }
}
```

`broken-binary.json`:
```json
{
  "files_per_bundle": { "p50": 6, "p90": 10, "p99": 12, "max_observed": 16 },
  "bytes_per_file": { "p50": 1024, "p90": 8192, "p99": 32768, "max_observed": 131072 },
  "parser_kind_weights": { "Plain": 1.0 },
  "fallback_rate_per_kb": { "Plain": 1.0 },
  "encoding_weights": { "Other": 1.0 }
}
```

`near-cap.json`:
```json
{
  "files_per_bundle": { "p50": 50, "p90": 60, "p99": 70, "max_observed": 80 },
  "bytes_per_file": { "p50": 700000, "p90": 1000000, "p99": 1100000, "max_observed": 1500000 },
  "parser_kind_weights": { "Ccm": 0.5, "Timestamped": 0.5 },
  "fallback_rate_per_kb": { "Ccm": 0.00005, "Timestamped": 0.0001 },
  "encoding_weights": { "Utf8": 1.0 }
}
```

- [ ] **Step 2: Add profile resolver**

Append to `crates/parse-load-harness/src/corpus/shape.rs`:

```rust
const PROFILE_MANY_SMALL: &str = include_str!("profiles/many-small.json");
const PROFILE_GIANT_FILE: &str = include_str!("profiles/giant-file.json");
const PROFILE_UNICODE_HEAVY: &str = include_str!("profiles/unicode-heavy.json");
const PROFILE_BROKEN_BINARY: &str = include_str!("profiles/broken-binary.json");
const PROFILE_NEAR_CAP: &str = include_str!("profiles/near-cap.json");

impl Shape {
    /// Resolve a `--shape` argument:
    /// - `None` → `load_default()`
    /// - a known profile name → its bundled JSON
    /// - any other string → treat as a filesystem path
    pub fn resolve(spec: Option<&str>) -> anyhow::Result<Self> {
        match spec {
            None => Self::load_default(),
            Some("many-small") => Ok(serde_json::from_str(PROFILE_MANY_SMALL)?),
            Some("giant-file") => Ok(serde_json::from_str(PROFILE_GIANT_FILE)?),
            Some("unicode-heavy") => Ok(serde_json::from_str(PROFILE_UNICODE_HEAVY)?),
            Some("broken-binary") => Ok(serde_json::from_str(PROFILE_BROKEN_BINARY)?),
            Some("near-cap") => Ok(serde_json::from_str(PROFILE_NEAR_CAP)?),
            Some(path) => Self::load_from(std::path::Path::new(path)),
        }
    }
}

#[cfg(test)]
mod resolver_tests {
    use super::*;

    #[test]
    fn each_profile_loads_and_is_valid() {
        for name in ["many-small", "giant-file", "unicode-heavy", "broken-binary", "near-cap"] {
            let s = Shape::resolve(Some(name)).unwrap_or_else(|e| {
                panic!("profile {name} failed to load: {e}")
            });
            let total: f64 = s.parser_kind_weights.values().sum();
            assert!((total - 1.0).abs() < 0.01, "profile {name} weights = {total}");
        }
    }
}
```

- [ ] **Step 3: Implement the replay corpus**

Write `crates/parse-load-harness/src/corpus/replay.rs`:

```rust
//! Replay = a `Corpus` backed by a directory of real `.zip` files.

use crate::bundle::Bundle;
use crate::corpus::Corpus;
use std::path::PathBuf;

const MAX_EVIDENCE_ZIP_BYTES: u64 = 50 * 1024 * 1024;

pub struct ReplayCorpus {
    paths: Vec<PathBuf>,
    cursor: usize,
}

impl ReplayCorpus {
    pub fn new(dir: &std::path::Path) -> anyhow::Result<Self> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("zip"))
            .filter(|p| {
                p.metadata()
                    .map(|m| m.len() <= MAX_EVIDENCE_ZIP_BYTES)
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        anyhow::ensure!(!paths.is_empty(), "no usable .zip files in {dir:?}");
        Ok(Self { paths, cursor: 0 })
    }
}

#[async_trait::async_trait]
impl Corpus for ReplayCorpus {
    async fn next(&mut self) -> anyhow::Result<Bundle> {
        let path = &self.paths[self.cursor % self.paths.len()];
        self.cursor = self.cursor.wrapping_add(1);
        let zip_bytes = tokio::fs::read(path).await?;
        let n_files = {
            let cursor = std::io::Cursor::new(&zip_bytes);
            zip::ZipArchive::new(cursor)?.len() as u32
        };
        Ok(Bundle {
            seq: 0,
            device_id: String::new(),
            bundle_id: uuid::Uuid::now_v7(),
            n_files,
            zip_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_min_zip(path: &std::path::Path) {
        let f = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        zw.start_file("a.log", opts).unwrap();
        zw.write_all(b"hi\n").unwrap();
        zw.finish().unwrap();
    }

    #[tokio::test]
    async fn round_robins_through_directory() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.zip", "b.zip", "c.zip"] {
            write_min_zip(&dir.path().join(name));
        }
        let mut c = ReplayCorpus::new(dir.path()).unwrap();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..6 {
            seen.insert(c.next().await.unwrap().bundle_id);
        }
        assert_eq!(seen.len(), 6, "each call gets a fresh bundle_id even on replay");
    }

    #[tokio::test]
    async fn errors_on_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ReplayCorpus::new(dir.path()).is_err());
    }
}
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test -p parse-load-harness --lib corpus`
Expected: profile resolver test + replay tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/parse-load-harness/src/corpus/
git commit -m "feat(harness): shape profiles + directory replay corpus"
```

---

## Task 6: Auth strategies

**Files:**
- Modify: `crates/parse-load-harness/src/auth/mod.rs`
- Modify: `crates/parse-load-harness/src/auth/header.rs`
- Modify: `crates/parse-load-harness/src/auth/mtls.rs`

- [ ] **Step 1: Define the Auth trait**

Replace `crates/parse-load-harness/src/auth/mod.rs`:

```rust
//! Auth strategies for HTTP targets. The in-process target stamps a
//! `DeviceIdentity` directly and bypasses this trait.

pub mod header;
pub mod mtls;

use reqwest::RequestBuilder;

pub trait Auth: Send + Sync {
    /// Stamp a request with whatever the target needs (header, client cert
    /// already configured on the client, etc.).
    fn apply(&self, req: RequestBuilder, device_id: &str) -> RequestBuilder;
}
```

- [ ] **Step 2: Implement header auth**

Replace `crates/parse-load-harness/src/auth/header.rs`:

```rust
//! `X-Device-Id` header — pilot-compatible. Default for HTTP targets.

use crate::auth::Auth;
use reqwest::RequestBuilder;

pub struct HeaderAuth;

impl Auth for HeaderAuth {
    fn apply(&self, req: RequestBuilder, device_id: &str) -> RequestBuilder {
        req.header("X-Device-Id", device_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_auth_constructs_without_panicking() {
        let _ = HeaderAuth;
    }
}
```

- [ ] **Step 3: Implement mTLS auth**

Replace `crates/parse-load-harness/src/auth/mtls.rs`:

```rust
//! In-memory test CA + per-virtual-device leaf certs. Local-http target only.
//! Wraps `rcgen` so we don't depend on `openssl`.

use crate::auth::Auth;
use rcgen::{CertificateParams, DnType, KeyPair};
use reqwest::RequestBuilder;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct MtlsAuth {
    ca_pem: String,
    leaf_cache: Mutex<HashMap<String, (String, String)>>, // device_id -> (cert, key)
    ca_cert: rcgen::Certificate,
    ca_key: KeyPair,
}

impl MtlsAuth {
    pub fn new() -> anyhow::Result<Self> {
        let mut params = CertificateParams::new(vec!["cmtrace-load-test-ca".into()])?;
        params.distinguished_name.push(DnType::CommonName, "cmtrace-load-test-ca");
        let key = KeyPair::generate()?;
        let ca_cert = params.self_signed(&key)?;
        let ca_pem = ca_cert.pem();
        Ok(Self {
            ca_pem,
            leaf_cache: Mutex::new(HashMap::new()),
            ca_cert,
            ca_key: key,
        })
    }

    pub fn ca_pem(&self) -> &str {
        &self.ca_pem
    }

    /// Mint (or fetch from cache) a leaf cert + key for a given device_id.
    /// Returned strings are PEM-encoded.
    pub fn leaf_for(&self, device_id: &str) -> anyhow::Result<(String, String)> {
        if let Some(v) = self.leaf_cache.lock().unwrap().get(device_id).cloned() {
            return Ok(v);
        }
        let mut params = CertificateParams::new(vec![device_id.to_string()])?;
        params.distinguished_name.push(DnType::CommonName, device_id);
        let leaf_key = KeyPair::generate()?;
        let leaf = params.signed_by(&leaf_key, &self.ca_cert, &self.ca_key)?;
        let cert_pem = leaf.pem();
        let key_pem = leaf_key.serialize_pem();
        let entry = (cert_pem, key_pem);
        self.leaf_cache
            .lock()
            .unwrap()
            .insert(device_id.into(), entry.clone());
        Ok(entry)
    }
}

impl Auth for MtlsAuth {
    /// mTLS auth attaches via the reqwest *client* (set up at target startup),
    /// not per-request. This impl is a no-op so the trait stays single-shape;
    /// the local-http target swaps the http client when --auth=mtls is set.
    fn apply(&self, req: RequestBuilder, _device_id: &str) -> RequestBuilder {
        req
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_ca_and_leaf_certs() {
        let a = MtlsAuth::new().unwrap();
        assert!(a.ca_pem().contains("BEGIN CERTIFICATE"));
        let (cert, key) = a.leaf_for("LOAD-TEST-A-0001").unwrap();
        assert!(cert.contains("BEGIN CERTIFICATE"));
        assert!(key.contains("PRIVATE KEY"));
    }

    #[test]
    fn leaf_cache_returns_same_pair() {
        let a = MtlsAuth::new().unwrap();
        let p1 = a.leaf_for("dev-1").unwrap();
        let p2 = a.leaf_for("dev-1").unwrap();
        assert_eq!(p1, p2);
    }
}
```

- [ ] **Step 4: Run auth tests**

Run: `cargo test -p parse-load-harness --lib auth`
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/parse-load-harness/src/auth/
git commit -m "feat(harness): header + mTLS auth strategies"
```

---

## Task 7: Target trait + remote-http client

**Files:**
- Modify: `crates/parse-load-harness/src/target/mod.rs`
- Modify: `crates/parse-load-harness/src/target/remote_http.rs`

- [ ] **Step 1: Read the api-server's ingest wire protocol**

Before writing client code, read `crates/api-server/src/routes/ingest.rs` end-to-end so the harness's request shapes match exactly. The protocol is:
1. `POST /v1/ingest/upload` → `{upload_id}`
2. Repeat: `POST /v1/ingest/upload/{upload_id}/chunk?seq=N` (binary body, ≤ chunk_size_bytes from server config)
3. `POST /v1/ingest/upload/{upload_id}/finalize` with body `{bundle_id, content_kind, content_sha256, total_bytes}` → `{session_id, parse_state, ...}`

Then the harness polls `GET /v1/sessions/{session_id}` until `parse_state != "pending"`.

- [ ] **Step 2: Define the Target trait**

Replace `crates/parse-load-harness/src/target/mod.rs`:

```rust
//! Target = the system under test. Three implementations:
//! in-process (direct `parse_session` call), local-http (managed compose
//! stack), remote-http (external URL).

pub mod in_process;
pub mod local_http;
pub mod remote_http;

use crate::bundle::{Bundle, BundleResult};

#[async_trait::async_trait]
pub trait Target: Send + Sync {
    /// Send one bundle through the target's full path. Returns a result
    /// row ready to be JSON-serialized into bundles.jsonl.
    async fn send(&self, bundle: Bundle) -> BundleResult;

    /// Sample target system metrics (in-flight count, RSS, DB pool stats).
    /// Returns a JSON object that the system sampler appends as one row.
    async fn sample_system(&self) -> serde_json::Value;

    /// Tear down anything the target brought up (compose, scratch DB, etc.).
    async fn shutdown(self: Box<Self>) -> anyhow::Result<()>;
}
```

- [ ] **Step 3: Write the failing test for remote-http URL composition**

Add a small unit-testable helper. Append to `crates/parse-load-harness/src/target/remote_http.rs`:

```rust
//! HTTP client against an external api-server URL. Used directly by the
//! `--target=remote-http` mode and as the inner client of `local-http`.

use crate::auth::Auth;
use crate::bundle::{Bundle, BundleResult};
use crate::target::Target;
use reqwest::Client;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct RemoteHttpTarget {
    base_url: String,
    client: Client,
    auth: Arc<dyn Auth>,
    poll_interval: Duration,
    poll_timeout: Duration,
}

impl RemoteHttpTarget {
    pub fn new(base_url: String, client: Client, auth: Arc<dyn Auth>) -> Self {
        Self {
            base_url,
            client,
            auth,
            poll_interval: Duration::from_millis(250),
            poll_timeout: Duration::from_secs(120),
        }
    }

    fn url(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{base}{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_join_strips_trailing_slash() {
        let t = RemoteHttpTarget {
            base_url: "https://x.example/".into(),
            client: Client::new(),
            auth: Arc::new(crate::auth::header::HeaderAuth),
            poll_interval: Duration::from_millis(1),
            poll_timeout: Duration::from_millis(1),
        };
        assert_eq!(t.url("/v1/ingest/upload"), "https://x.example/v1/ingest/upload");
    }
}
```

- [ ] **Step 4: Run the unit test**

Run: `cargo test -p parse-load-harness --lib target::remote_http`
Expected: 1 test passes.

- [ ] **Step 5: Implement the full send flow**

Append to `crates/parse-load-harness/src/target/remote_http.rs`:

```rust
const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4 MiB chunks

#[async_trait::async_trait]
impl Target for RemoteHttpTarget {
    async fn send(&self, bundle: Bundle) -> BundleResult {
        let started = Instant::now();
        let device_id = bundle.device_id.clone();
        let bundle_bytes = bundle.zip_bytes.len() as u64;
        let n_files = bundle.n_files;
        let seq = bundle.seq;

        let result = async {
            // 1. Open upload.
            let resp = self
                .auth
                .apply(self.client.post(self.url("/v1/ingest/upload")), &device_id)
                .json(&json!({"bundle_id": bundle.bundle_id}))
                .send()
                .await?;
            let status = resp.status().as_u16();
            anyhow::ensure!(resp.status().is_success(), "open upload status {status}");
            let body: serde_json::Value = resp.json().await?;
            let upload_id = body["upload_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing upload_id"))?
                .to_string();

            // 2. Chunked upload.
            let mut seq_n = 0u32;
            let mut sha = sha2::Sha256::new();
            use sha2::Digest;
            for chunk in bundle.zip_bytes.chunks(CHUNK_SIZE) {
                sha.update(chunk);
                let resp = self
                    .auth
                    .apply(
                        self.client.post(self.url(&format!(
                            "/v1/ingest/upload/{upload_id}/chunk?seq={seq_n}"
                        ))),
                        &device_id,
                    )
                    .body(chunk.to_vec())
                    .send()
                    .await?;
                anyhow::ensure!(resp.status().is_success(), "chunk {seq_n} status {}", resp.status());
                seq_n += 1;
            }
            let sha_hex = format!("{:x}", sha.finalize());

            // 3. Finalize.
            let resp = self
                .auth
                .apply(
                    self.client.post(self.url(&format!(
                        "/v1/ingest/upload/{upload_id}/finalize"
                    ))),
                    &device_id,
                )
                .json(&json!({
                    "bundle_id": bundle.bundle_id,
                    "content_kind": "evidence-zip",
                    "content_sha256": sha_hex,
                    "total_bytes": bundle_bytes,
                }))
                .send()
                .await?;
            let finalize_status = resp.status().as_u16();
            anyhow::ensure!(resp.status().is_success(), "finalize status {finalize_status}");
            let body: serde_json::Value = resp.json().await?;
            let session_id = body["session_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing session_id"))?
                .to_string();
            let finalize_ms = started.elapsed().as_millis() as u64;
            let parse_started = Instant::now();

            // 4. Poll until terminal.
            let (parse_state, files_with_fallbacks) = loop {
                if parse_started.elapsed() > self.poll_timeout {
                    anyhow::bail!("parse timed out after {:?}", self.poll_timeout);
                }
                let resp = self
                    .auth
                    .apply(self.client.get(self.url(&format!("/v1/sessions/{session_id}"))), &device_id)
                    .send()
                    .await?;
                anyhow::ensure!(resp.status().is_success(), "poll status {}", resp.status());
                let body: serde_json::Value = resp.json().await?;
                let state = body["parse_state"].as_str().unwrap_or("pending").to_string();
                if state != "pending" {
                    break (state, body["files_with_fallbacks"].as_u64().map(|n| n as u32));
                }
                tokio::time::sleep(self.poll_interval).await;
            };
            let parse_ms = parse_started.elapsed().as_millis() as u64;

            anyhow::Ok((finalize_status, finalize_ms, parse_ms, parse_state, files_with_fallbacks))
        }
        .await;

        match result {
            Ok((http_status, finalize_ms, parse_ms, parse_state, files_with_fallbacks)) => BundleResult {
                seq,
                device: device_id,
                n_files,
                bundle_bytes,
                finalize_ms: Some(finalize_ms),
                parse_ms,
                parse_state,
                files_with_fallbacks,
                http_status: Some(http_status),
                error: None,
            },
            Err(e) => BundleResult {
                seq,
                device: device_id,
                n_files,
                bundle_bytes,
                finalize_ms: None,
                parse_ms: started.elapsed().as_millis() as u64,
                parse_state: "harness-error".into(),
                files_with_fallbacks: None,
                http_status: None,
                error: Some(format!("{e:#}")),
            },
        }
    }

    async fn sample_system(&self) -> serde_json::Value {
        let resp = self.client.get(self.url("/metrics")).send().await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let text = r.text().await.unwrap_or_default();
                json!({"metrics_raw_bytes": text.len(), "scrape_ok": true})
            }
            _ => json!({"scrape_ok": false}),
        }
    }

    async fn shutdown(self: Box<Self>) -> anyhow::Result<()> {
        Ok(())
    }
}
```

Add `sha2 = "0.10"` to `Cargo.toml` `[dependencies]`.

- [ ] **Step 6: Verify it compiles**

Run: `cargo check -p parse-load-harness`
Expected: clean build.

- [ ] **Step 7: Commit**

```bash
git add crates/parse-load-harness/src/target/ crates/parse-load-harness/Cargo.toml
git commit -m "feat(harness): Target trait + remote-http client"
```

---

## Task 8: In-process target

**Files:**
- Modify: `crates/parse-load-harness/src/target/in_process.rs`

- [ ] **Step 1: Read the api-server's parse_session entry point**

Re-read `crates/api-server/src/pipeline/parse_worker.rs::parse_session` and `crates/api-server/src/storage/mod.rs` (specifically the `MetadataStore` and `BlobStore` traits and their constructors). The harness's in-process target needs to:
1. Construct a `MetadataStore` (use `MetaPostgres::connect`, takes a PG URL).
2. Construct a `BlobStore` (use `BlobLocalFs::new(tempdir)`).
3. Stage the bundle bytes into the blob store via `BlobStore::write_blob` (or whatever the upload→finalize wire commits to — read finalize_handler to see the exact sequence).
4. Insert the session row.
5. Call `parse_session(session_id, blob_uri, "evidence-zip", ParseDeps { meta, blobs })`.
6. Poll `meta.get_session(session_id)` until `parse_state` flips.

If any of those storage methods don't exist, the harness should not paper over it — file a follow-up task and use the public crate surface only. Most likely path: api-server's `routes::ingest::finalize` does the staging; the harness can copy the same logic by calling the same trait methods in the same order, OR call the route handler directly via tower's `Service::call`. The latter is closer to how the system actually works; use it.

- [ ] **Step 2: Build the target**

Write `crates/parse-load-harness/src/target/in_process.rs`:

```rust
//! In-process target — drives parse_session directly via the api-server's
//! storage traits, bypassing HTTP. Highest-throughput backend; useful for
//! parser/throughput science without upload-chunking variance.

use crate::bundle::{Bundle, BundleResult};
use crate::target::Target;
use api_server::pipeline::parse_worker::{self, ParseDeps};
use api_server::storage::{
    BlobStore, BlobLocalFs, MetaPostgres, MetadataStore, NewSession,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

pub struct InProcessTarget {
    meta: Arc<dyn MetadataStore>,
    blobs: Arc<dyn BlobStore>,
    _blob_dir: TempDir,
    pg_url: String,
    poll_interval: Duration,
    poll_timeout: Duration,
}

impl InProcessTarget {
    pub async fn new(pg_url: String) -> anyhow::Result<Self> {
        let meta = MetaPostgres::connect(&pg_url).await?;
        let meta: Arc<dyn MetadataStore> = Arc::new(meta);
        let blob_dir = tempfile::tempdir()?;
        let blobs: Arc<dyn BlobStore> =
            Arc::new(BlobLocalFs::new(blob_dir.path().to_path_buf()));
        Ok(Self {
            meta,
            blobs,
            _blob_dir: blob_dir,
            pg_url,
            poll_interval: Duration::from_millis(50),
            poll_timeout: Duration::from_secs(120),
        })
    }
}

#[async_trait::async_trait]
impl Target for InProcessTarget {
    async fn send(&self, bundle: Bundle) -> BundleResult {
        let started = Instant::now();
        let bundle_bytes = bundle.zip_bytes.len() as u64;

        let send_inner = async {
            // 1. Stage the blob (mirrors api-server::routes::ingest::finalize).
            let blob_uri = self.blobs.write_blob(&bundle.zip_bytes).await?;

            // 2. Insert the session.
            let session_id = uuid::Uuid::now_v7();
            self.meta
                .insert_session(NewSession {
                    session_id,
                    device_id: bundle.device_id.clone(),
                    bundle_id: bundle.bundle_id,
                    blob_uri: blob_uri.clone(),
                    content_kind: "evidence-zip".into(),
                    content_sha256: "load-test-no-sha".into(),
                    size_bytes: bundle_bytes as i64,
                    parse_state: "pending".into(),
                })
                .await?;

            // 3. Drive the parse worker on the same task. (Spawning would
            // mirror prod, but we want timing for *one* bundle here.)
            let parse_started = Instant::now();
            parse_worker::parse_session(
                session_id,
                blob_uri,
                "evidence-zip".into(),
                ParseDeps {
                    meta: self.meta.clone(),
                    blobs: self.blobs.clone(),
                },
            )
            .await;

            // 4. Poll session until terminal (covers worker spawning patterns
            // if we ever switch to fire-and-forget here).
            let mut state = "pending".to_string();
            let mut files_with_fallbacks: Option<u32> = None;
            while state == "pending" {
                if parse_started.elapsed() > self.poll_timeout {
                    anyhow::bail!("parse timed out");
                }
                let row = self.meta.get_session(session_id).await?;
                state = row.parse_state.clone();
                files_with_fallbacks = Some(row.files_with_fallbacks.unwrap_or(0) as u32);
                if state == "pending" {
                    tokio::time::sleep(self.poll_interval).await;
                }
            }
            anyhow::Ok((state, files_with_fallbacks, parse_started.elapsed().as_millis() as u64))
        }
        .await;

        let seq = bundle.seq;
        let device = bundle.device_id;
        let n_files = bundle.n_files;

        match send_inner {
            Ok((state, fwf, parse_ms)) => BundleResult {
                seq,
                device,
                n_files,
                bundle_bytes,
                finalize_ms: None,
                parse_ms,
                parse_state: state,
                files_with_fallbacks: fwf,
                http_status: None,
                error: None,
            },
            Err(e) => BundleResult {
                seq,
                device,
                n_files,
                bundle_bytes,
                finalize_ms: None,
                parse_ms: started.elapsed().as_millis() as u64,
                parse_state: "harness-error".into(),
                files_with_fallbacks: None,
                http_status: None,
                error: Some(format!("{e:#}")),
            },
        }
    }

    async fn sample_system(&self) -> serde_json::Value {
        // sysinfo handles process RSS portably.
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        let pid = sysinfo::get_current_pid().ok();
        let rss_kb = pid.and_then(|pid| sys.process(pid).map(|p| p.memory()));
        serde_json::json!({
            "rss_mb": rss_kb.map(|kb| kb / 1024),
            "in_process": true,
        })
    }

    async fn shutdown(self: Box<Self>) -> anyhow::Result<()> {
        // Drop the temp blob dir on RAII; PG schema teardown is the operator's
        // job for the in-process target (the URL points at their dev DB).
        tracing::info!(pg_url = %self.pg_url, "in-process shutdown — leaving PG in place");
        Ok(())
    }
}
```

**Note on storage trait surface:** `MetaPostgres::connect`, `BlobLocalFs::new`, `BlobStore::write_blob`, `MetadataStore::insert_session` (with `NewSession`), and `MetadataStore::get_session` (with `files_with_fallbacks`) are referenced here. If any of these don't exist on the api-server's public surface today, *fix the api-server first* in a sibling commit on this branch — adding `pub` to the right items and exposing them via `lib.rs`. Don't reach into private internals from the harness.

- [ ] **Step 3: Compile + smoke**

Run: `cargo check -p parse-load-harness`
Expected: builds. If api-server symbols aren't public, fix in `crates/api-server/src/lib.rs` re-exports first, then re-run.

- [ ] **Step 4: Commit**

```bash
git add crates/parse-load-harness/src/target/in_process.rs crates/api-server/src/lib.rs
git commit -m "feat(harness): in-process target via api-server traits"
```

---

## Task 9: Local-http target (compose lifecycle)

**Files:**
- Create: `crates/parse-load-harness/infra/compose/load-test.yml`
- Modify: `crates/parse-load-harness/src/target/local_http.rs`

- [ ] **Step 1: Write the compose file**

Write `crates/parse-load-harness/infra/compose/load-test.yml`:

```yaml
services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_USER: cmtrace
      POSTGRES_PASSWORD: cmtrace
      POSTGRES_DB: cmtrace
    ports:
      - "127.0.0.1:0:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U cmtrace"]
      interval: 1s
      timeout: 2s
      retries: 30

  api:
    image: ${CMTRACE_API_IMAGE:-cmtraceopen-api:latest}
    depends_on:
      postgres:
        condition: service_healthy
    environment:
      DATABASE_URL: postgres://cmtrace:cmtrace@postgres:5432/cmtrace
      BLOB_BACKEND: fs
      BLOB_LOCAL_DIR: /var/lib/cmtrace
      CMTRACE_PARSE_CONCURRENCY: ${CMTRACE_PARSE_CONCURRENCY:-4}
      CMTRACE_AUTH_DISABLED: "1"
      CMTRACE_MTLS_TRUST_ROOTS: ${CMTRACE_MTLS_TRUST_ROOTS:-}
    volumes:
      - api-blobs:/var/lib/cmtrace
      - ${CMTRACE_LOAD_TEST_CA_DIR:-/dev/null}:/etc/cmtrace/test-ca:ro
    ports:
      - "127.0.0.1:0:8080"

volumes:
  api-blobs:
```

The `0:5432`/`0:8080` host bindings tell Docker to pick a free ephemeral port. The harness reads the chosen port back via `docker compose port`.

- [ ] **Step 2: Implement the target**

Replace `crates/parse-load-harness/src/target/local_http.rs`:

```rust
//! Local-http target — wraps a `docker compose` lifecycle around the
//! remote-http client. Brings the stack up at construction, tears it down
//! on shutdown, returns the locally-bound URL.

use crate::auth::Auth;
use crate::bundle::{Bundle, BundleResult};
use crate::target::remote_http::RemoteHttpTarget;
use crate::target::Target;
use reqwest::Client;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

pub struct LocalHttpTarget {
    inner: RemoteHttpTarget,
    project_name: String,
    compose_path: PathBuf,
    no_cleanup: bool,
}

impl LocalHttpTarget {
    pub async fn up(
        compose_path: PathBuf,
        image: Option<String>,
        ca_dir: Option<PathBuf>,
        auth: Arc<dyn Auth>,
        no_cleanup: bool,
    ) -> anyhow::Result<Self> {
        let project_name = format!("cmtrace-load-{}", uuid::Uuid::new_v4().simple());
        let mut up = Command::new("docker");
        up.args([
            "compose",
            "-p",
            &project_name,
            "-f",
            compose_path.to_str().unwrap(),
            "up",
            "-d",
            "--quiet-pull",
            "--wait",
        ]);
        if let Some(img) = &image {
            up.env("CMTRACE_API_IMAGE", img);
        }
        if let Some(dir) = &ca_dir {
            up.env("CMTRACE_LOAD_TEST_CA_DIR", dir);
            up.env("CMTRACE_MTLS_TRUST_ROOTS", "/etc/cmtrace/test-ca/ca.pem");
        }
        let status = up.status().await?;
        anyhow::ensure!(status.success(), "compose up failed: {status}");

        let port = read_compose_port(&project_name, &compose_path, "api", 8080).await?;
        let base_url = format!("http://127.0.0.1:{port}");
        let client = build_client(ca_dir.as_deref(), auth.as_ref()).await?;

        // Wait for /healthz.
        let started = std::time::Instant::now();
        loop {
            if started.elapsed() > Duration::from_secs(60) {
                anyhow::bail!("api never became healthy");
            }
            if let Ok(r) = client.get(format!("{base_url}/healthz")).send().await {
                if r.status().is_success() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let inner = RemoteHttpTarget::new(base_url, client, auth);
        Ok(Self {
            inner,
            project_name,
            compose_path,
            no_cleanup,
        })
    }
}

async fn read_compose_port(
    project: &str,
    compose: &std::path::Path,
    service: &str,
    container_port: u16,
) -> anyhow::Result<u16> {
    let out = Command::new("docker")
        .args([
            "compose",
            "-p",
            project,
            "-f",
            compose.to_str().unwrap(),
            "port",
            service,
            &container_port.to_string(),
        ])
        .output()
        .await?;
    anyhow::ensure!(out.status.success(), "docker compose port failed");
    let raw = String::from_utf8(out.stdout)?;
    // Output looks like "127.0.0.1:54321\n" — split on ':' and trim.
    let port = raw
        .trim()
        .split(':')
        .last()
        .ok_or_else(|| anyhow::anyhow!("port output empty"))?
        .parse::<u16>()?;
    Ok(port)
}

async fn build_client(
    ca_dir: Option<&std::path::Path>,
    _auth: &dyn Auth,
) -> anyhow::Result<Client> {
    if let Some(dir) = ca_dir {
        let ca_pem = tokio::fs::read(dir.join("ca.pem")).await?;
        let cert_pem = tokio::fs::read(dir.join("client.crt")).await?;
        let key_pem = tokio::fs::read(dir.join("client.key")).await?;
        let identity = reqwest::Identity::from_pkcs8_pem(&cert_pem, &key_pem)?;
        let ca = reqwest::Certificate::from_pem(&ca_pem)?;
        Ok(Client::builder()
            .add_root_certificate(ca)
            .identity(identity)
            .build()?)
    } else {
        Ok(Client::builder().build()?)
    }
}

#[async_trait::async_trait]
impl Target for LocalHttpTarget {
    async fn send(&self, bundle: Bundle) -> BundleResult {
        self.inner.send(bundle).await
    }
    async fn sample_system(&self) -> serde_json::Value {
        self.inner.sample_system().await
    }
    async fn shutdown(self: Box<Self>) -> anyhow::Result<()> {
        if self.no_cleanup {
            tracing::info!(project = %self.project_name, "--no-cleanup: leaving compose up");
            return Ok(());
        }
        let status = Command::new("docker")
            .args([
                "compose",
                "-p",
                &self.project_name,
                "-f",
                self.compose_path.to_str().unwrap(),
                "down",
                "-v",
            ])
            .status()
            .await?;
        anyhow::ensure!(status.success(), "compose down failed: {status}");
        Ok(())
    }
}
```

- [ ] **Step 3: Compile**

Run: `cargo check -p parse-load-harness`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/parse-load-harness/src/target/local_http.rs crates/parse-load-harness/infra/
git commit -m "feat(harness): local-http target with compose lifecycle"
```

---

## Task 10: Driver (load loop)

**Files:**
- Modify: `crates/parse-load-harness/src/driver.rs`
- Create: `crates/parse-load-harness/tests/driver_shape.rs`

- [ ] **Step 1: Write the failing tests**

Write `crates/parse-load-harness/tests/driver_shape.rs`:

```rust
use parse_load_harness::bundle::{Bundle, BundleResult};
use parse_load_harness::config::StopCondition;
use parse_load_harness::driver::run_driver;
use parse_load_harness::driver::DriverInputs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct CountingTarget {
    sent: AtomicU64,
}

#[async_trait::async_trait]
impl parse_load_harness::target::Target for CountingTarget {
    async fn send(&self, bundle: Bundle) -> BundleResult {
        self.sent.fetch_add(1, Ordering::SeqCst);
        BundleResult {
            seq: bundle.seq,
            device: bundle.device_id,
            n_files: bundle.n_files,
            bundle_bytes: bundle.zip_bytes.len() as u64,
            finalize_ms: Some(0),
            parse_ms: 0,
            parse_state: "ok".into(),
            files_with_fallbacks: None,
            http_status: Some(201),
            error: None,
        }
    }
    async fn sample_system(&self) -> serde_json::Value { serde_json::json!({}) }
    async fn shutdown(self: Box<Self>) -> anyhow::Result<()> { Ok(()) }
}

struct StaticCorpus { n_files: u32 }

#[async_trait::async_trait]
impl parse_load_harness::corpus::Corpus for StaticCorpus {
    async fn next(&mut self) -> anyhow::Result<Bundle> {
        Ok(Bundle {
            seq: 0,
            device_id: String::new(),
            bundle_id: uuid::Uuid::now_v7(),
            n_files: self.n_files,
            zip_bytes: vec![0u8; 64],
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn total_bundles_is_exact() {
    let target = Arc::new(CountingTarget::default());
    let corpus = Box::new(StaticCorpus { n_files: 3 });
    let (tx, mut rx) = tokio::sync::mpsc::channel::<BundleResult>(64);
    let inputs = DriverInputs {
        target: target.clone(),
        corpus,
        concurrency: 8,
        ramp: Duration::ZERO,
        stop: StopCondition::TotalBundles(50),
        device_pool_size: 5,
        run_uuid: "test-run".into(),
        reporter_tx: tx,
    };
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    run_driver(inputs).await.unwrap();
    assert_eq!(target.sent.load(Ordering::SeqCst), 50);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_mode_honors_deadline() {
    let target = Arc::new(CountingTarget::default());
    let corpus = Box::new(StaticCorpus { n_files: 1 });
    let (tx, mut rx) = tokio::sync::mpsc::channel::<BundleResult>(64);
    let inputs = DriverInputs {
        target: target.clone(),
        corpus,
        concurrency: 4,
        ramp: Duration::ZERO,
        stop: StopCondition::SoakDeadline(Duration::from_millis(200)),
        device_pool_size: 4,
        run_uuid: "test-soak".into(),
        reporter_tx: tx,
    };
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let started = Instant::now();
    run_driver(inputs).await.unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_millis(800), "elapsed {:?}", elapsed);
    assert!(target.sent.load(Ordering::SeqCst) > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ramp_delays_full_concurrency() {
    let target = Arc::new(CountingTarget::default());
    let corpus = Box::new(StaticCorpus { n_files: 1 });
    let (tx, mut rx) = tokio::sync::mpsc::channel::<BundleResult>(64);
    let inputs = DriverInputs {
        target: target.clone(),
        corpus,
        concurrency: 100,
        ramp: Duration::from_millis(300),
        stop: StopCondition::TotalBundles(200),
        device_pool_size: 100,
        run_uuid: "test-ramp".into(),
        reporter_tx: tx,
    };
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let started = Instant::now();
    run_driver(inputs).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(150),
        "ramp should impose at least half the ramp window before saturation");
}
```

- [ ] **Step 2: Verify tests compile and fail**

Run: `cargo test -p parse-load-harness --test driver_shape -- --list`
Expected: tests are listed (drives the trait/struct surface to be defined).

- [ ] **Step 3: Implement the driver**

Replace `crates/parse-load-harness/src/driver.rs`:

```rust
//! The load loop. Acquires a permit per bundle (semaphore feeds permits in
//! over a ramp window), pulls a bundle from the corpus, dispatches a task
//! that calls `target.send`, and pushes the result through the reporter
//! channel. Mirrors the producer/permit pattern we just landed in api-server.

use crate::bundle::{Bundle, BundleResult};
use crate::config::StopCondition;
use crate::corpus::Corpus;
use crate::target::Target;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc::Sender, Semaphore};
use tokio::task::JoinSet;

pub struct DriverInputs {
    pub target: Arc<dyn Target>,
    pub corpus: Box<dyn Corpus>,
    pub concurrency: u32,
    pub ramp: Duration,
    pub stop: StopCondition,
    pub device_pool_size: u32,
    pub run_uuid: String,
    pub reporter_tx: Sender<BundleResult>,
}

pub async fn run_driver(mut inputs: DriverInputs) -> anyhow::Result<()> {
    let permit = Arc::new(Semaphore::new(0));
    spawn_ramp(permit.clone(), inputs.concurrency, inputs.ramp);

    let mut tasks: JoinSet<()> = JoinSet::new();
    let mut emitted: u64 = 0;
    let deadline = match inputs.stop {
        StopCondition::SoakDeadline(d) => Some(Instant::now() + d),
        StopCondition::TotalBundles(_) => None,
    };
    let total_bundles = match inputs.stop {
        StopCondition::TotalBundles(n) => Some(n),
        StopCondition::SoakDeadline(_) => None,
    };
    let device_pool = inputs.device_pool_size.max(1) as u64;

    loop {
        if let Some(n) = total_bundles {
            if emitted >= n { break; }
        }
        if let Some(d) = deadline {
            if Instant::now() >= d { break; }
        }

        // Acquire one permit. If the deadline fires before a permit is
        // available (e.g. ramp is slow + soak short), bail.
        let permit_owned = if let Some(d) = deadline {
            let remaining = d.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, permit.clone().acquire_owned()).await {
                Ok(p) => p?,
                Err(_) => break,
            }
        } else {
            permit.clone().acquire_owned().await?
        };

        let mut bundle = inputs.corpus.next().await?;
        bundle.seq = emitted;
        bundle.device_id = format!(
            "LOAD-TEST-{}-{:04}",
            &inputs.run_uuid[..inputs.run_uuid.len().min(8)],
            emitted % device_pool
        );
        emitted += 1;

        let target = inputs.target.clone();
        let tx = inputs.reporter_tx.clone();
        tasks.spawn(async move {
            let _p = permit_owned; // hold permit for the lifetime of the task
            let result = target.send(bundle).await;
            let _ = tx.send(result).await;
        });
    }

    while tasks.join_next().await.is_some() {}
    Ok(())
}

fn spawn_ramp(permit: Arc<Semaphore>, concurrency: u32, ramp: Duration) {
    if ramp.is_zero() {
        permit.add_permits(concurrency as usize);
        return;
    }
    tokio::spawn(async move {
        let step = ramp / concurrency.max(1);
        for _ in 0..concurrency {
            permit.add_permits(1);
            tokio::time::sleep(step).await;
        }
    });
}
```

- [ ] **Step 4: Run driver tests**

Run: `cargo test -p parse-load-harness --test driver_shape`
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/parse-load-harness/src/driver.rs crates/parse-load-harness/tests/driver_shape.rs
git commit -m "feat(harness): load driver with ramp + total/soak stop conditions"
```

---

## Task 11: Reporter — bundles.jsonl + summary.json

**Files:**
- Modify: `crates/parse-load-harness/src/reporter/mod.rs`

- [ ] **Step 1: Write the implementation + tests in one file**

Replace `crates/parse-load-harness/src/reporter/mod.rs`:

```rust
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

#[derive(Clone, Debug, Serialize)]
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

#[derive(Clone, Debug, Serialize, Default)]
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
```

- [ ] **Step 2: Run reporter tests**

Run: `cargo test -p parse-load-harness --lib reporter`
Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
git add crates/parse-load-harness/src/reporter/mod.rs
git commit -m "feat(harness): bundles.jsonl reporter + summary.json"
```

---

## Task 12: System sampler

**Files:**
- Modify: `crates/parse-load-harness/src/reporter/system.rs`

- [ ] **Step 1: Implement the sampler**

Replace `crates/parse-load-harness/src/reporter/system.rs`:

```rust
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
```

- [ ] **Step 2: Run sampler tests**

Run: `cargo test -p parse-load-harness --lib reporter::system`
Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
git add crates/parse-load-harness/src/reporter/system.rs
git commit -m "feat(harness): periodic system sampler -> system.jsonl"
```

---

## Task 13: Compare mode

**Files:**
- Modify: `crates/parse-load-harness/src/reporter/compare.rs`
- Create: `crates/parse-load-harness/tests/compare_report.rs`

- [ ] **Step 1: Implement the comparator**

Replace `crates/parse-load-harness/src/reporter/compare.rs`:

```rust
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
```

- [ ] **Step 2: Write the integration test**

Write `crates/parse-load-harness/tests/compare_report.rs`:

```rust
use parse_load_harness::reporter::compare::Comparison;
use std::fs;

fn write_summary(dir: &std::path::Path, p99: u64, throughput: f64, err: u64) {
    fs::create_dir_all(dir).unwrap();
    let body = serde_json::json!({
        "run_uuid": dir.file_name().unwrap().to_string_lossy(),
        "seed": 1,
        "total_bundles": 100,
        "error_count": err,
        "counts_by_state": {"ok": 100u64 - err, "failed": err},
        "finalize_ms": {"p50": 10, "p95": 20, "p99": 30, "max": 40},
        "parse_ms": {"p50": 100, "p95": 500, "p99": p99, "max": p99 * 2},
        "wall_seconds": 60.0,
        "throughput_bundles_per_min": throughput
    });
    fs::write(dir.join("summary.json"), serde_json::to_vec_pretty(&body).unwrap()).unwrap();
}

#[test]
fn identical_runs_have_no_regressions() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write_summary(&a, 1000, 100.0, 0);
    write_summary(&b, 1000, 100.0, 0);
    let c = Comparison::build(&a, &b, 10.0).unwrap();
    assert!(!c.has_regression());
}

#[test]
fn regression_in_p99_above_threshold_is_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write_summary(&a, 1000, 100.0, 0);
    write_summary(&b, 1500, 100.0, 0); // 50% slower
    let c = Comparison::build(&a, &b, 10.0).unwrap();
    assert!(c.has_regression());
    let md = c.render_markdown();
    assert!(md.contains("parse_ms.p99"));
    assert!(md.contains("⚠️"));
}

#[test]
fn throughput_drop_above_threshold_is_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write_summary(&a, 1000, 100.0, 0);
    write_summary(&b, 1000, 50.0, 0); // 50% slower throughput
    let c = Comparison::build(&a, &b, 10.0).unwrap();
    assert!(c.has_regression());
}
```

- [ ] **Step 3: Run compare tests**

Run: `cargo test -p parse-load-harness --test compare_report`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/parse-load-harness/src/reporter/compare.rs crates/parse-load-harness/tests/compare_report.rs
git commit -m "feat(harness): compare-mode regression detection"
```

---

## Task 14: Wire main.rs end-to-end

**Files:**
- Modify: `crates/parse-load-harness/src/main.rs`

- [ ] **Step 1: Replace main.rs with the full dispatch**

Replace `crates/parse-load-harness/src/main.rs`:

```rust
use clap::Parser;
use parse_load_harness::auth::{self, Auth};
use parse_load_harness::config::{Args, AuthKind, Cli, Cmd, RunConfig, StopCondition, TargetKind};
use parse_load_harness::corpus::{replay::ReplayCorpus, shape::Shape, synthetic::SyntheticCorpus, Corpus};
use parse_load_harness::driver::{run_driver, DriverInputs};
use parse_load_harness::reporter::{compare::Comparison, run_reporter, system::run_system_sampler, ReporterInputs, system::SystemSamplerInputs};
use parse_load_harness::target::{
    in_process::InProcessTarget, local_http::LocalHttpTarget, remote_http::RemoteHttpTarget, Target,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "parse_load_harness=info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run(args) => run_command(args).await,
        Cmd::MineDump(_) => anyhow::bail!("use the mine-dump binary directly: cargo run --bin mine-dump"),
        Cmd::Clean(_) => anyhow::bail!("clean command not yet implemented"),
    }
}

async fn run_command(args: Args) -> anyhow::Result<()> {
    let cfg = RunConfig::from_args(args)?;
    tokio::fs::create_dir_all(&cfg.out_dir).await?;
    tracing::info!(run = %cfg.run_uuid, seed = cfg.seed, out = %cfg.out_dir.display(), "starting run");

    let corpus: Box<dyn Corpus> = if let Some(dir) = &cfg.args.corpus {
        Box::new(ReplayCorpus::new(dir)?)
    } else {
        let shape = Shape::resolve(cfg.args.shape.as_deref())?;
        Box::new(SyntheticCorpus::new(shape, cfg.seed)?)
    };

    let auth: Arc<dyn Auth> = match cfg.args.auth {
        AuthKind::Header => Arc::new(auth::header::HeaderAuth),
        AuthKind::Mtls => Arc::new(auth::mtls::MtlsAuth::new()?),
    };

    let target: Arc<dyn Target> = match cfg.args.target {
        TargetKind::InProcess => {
            let pg = cfg
                .args
                .in_process_pg
                .clone()
                .ok_or_else(|| anyhow::anyhow!("--target=in-process requires --in-process-pg"))?;
            Arc::new(InProcessTarget::new(pg).await?)
        }
        TargetKind::LocalHttp => {
            let compose = PathBuf::from("crates/parse-load-harness/infra/compose/load-test.yml");
            let ca_dir = if matches!(cfg.args.auth, AuthKind::Mtls) {
                let dir = tempfile::tempdir()?;
                if let Some(mtls) = (auth.as_ref() as &dyn std::any::Any).downcast_ref::<auth::mtls::MtlsAuth>() {
                    let (cert, key) = mtls.leaf_for("LOAD-TEST-LEAF-0001")?;
                    tokio::fs::write(dir.path().join("ca.pem"), mtls.ca_pem()).await?;
                    tokio::fs::write(dir.path().join("client.crt"), cert).await?;
                    tokio::fs::write(dir.path().join("client.key"), key).await?;
                    Some(dir.into_path())
                } else {
                    None
                }
            } else {
                None
            };
            let t = LocalHttpTarget::up(compose, cfg.args.image.clone(), ca_dir, auth.clone(), cfg.args.no_cleanup).await?;
            Arc::new(t)
        }
        TargetKind::RemoteHttp => {
            let url = cfg.args.target_url.clone().unwrap();
            Arc::new(RemoteHttpTarget::new(url, reqwest::Client::new(), auth.clone()))
        }
    };

    let (bundle_tx, bundle_rx) = tokio::sync::mpsc::channel(256);
    let reporter_handle = tokio::spawn(run_reporter(ReporterInputs {
        out_dir: cfg.out_dir.clone(),
        run_uuid: cfg.run_uuid.to_string(),
        seed: cfg.seed,
        bundle_rx,
    }));

    let (sampler_stop_tx, sampler_stop_rx) = tokio::sync::oneshot::channel();
    let sampler_handle = tokio::spawn(run_system_sampler(SystemSamplerInputs {
        target: target.clone(),
        out_dir: cfg.out_dir.clone(),
        interval: Duration::from_secs(cfg.args.sample_seconds as u64),
        stop: sampler_stop_rx,
    }));

    let driver_inputs = DriverInputs {
        target: target.clone(),
        corpus,
        concurrency: cfg.args.concurrency,
        ramp: Duration::from_secs(cfg.args.ramp_seconds as u64),
        stop: cfg.stop,
        device_pool_size: cfg.args.concurrency,
        run_uuid: cfg.run_uuid.to_string(),
        reporter_tx: bundle_tx.clone(),
    };
    drop(bundle_tx); // reporter exits when all senders drop
    run_driver(driver_inputs).await?;

    let _ = sampler_stop_tx.send(());
    let _ = sampler_handle.await;
    let summary = reporter_handle.await??;
    tracing::info!(?summary, "run complete");

    let mut exit_code = 0;
    if let Some(prev) = &cfg.args.compare_to {
        let cmp = Comparison::build(prev, &cfg.out_dir, cfg.args.regression_threshold)?;
        let md = cmp.render_markdown();
        tokio::fs::write(cfg.out_dir.join("comparison.md"), md).await?;
        if cmp.has_regression() {
            tracing::warn!("regression detected past threshold {}%", cfg.args.regression_threshold);
            exit_code = 1;
        }
    }

    if cfg.args.strict && summary.error_count > 0 {
        exit_code = 1;
    }
    if let Some(rate) = cfg.args.max_error_rate {
        let observed = if summary.total_bundles == 0 { 0.0 } else { summary.error_count as f64 / summary.total_bundles as f64 };
        if observed > rate {
            exit_code = 1;
        }
    }

    if let Ok(t) = Arc::try_unwrap(target) {
        let boxed: Box<dyn Target> = Box::new(t);
        boxed.shutdown().await?;
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}
```

The `Arc::try_unwrap` at shutdown only succeeds when nothing else holds a reference — by this point only the local `target` binding does. If any task is still holding one, log + skip teardown rather than blocking on it.

- [ ] **Step 2: Compile**

Run: `cargo check -p parse-load-harness`
Expected: clean. (May surface trait-object downcast issues — adjust the mtls CA-extraction path if so; the simplest replacement is to construct `MtlsAuth` separately from the `Arc<dyn Auth>` so the typed reference is available.)

- [ ] **Step 3: Smoke run against the existing tests**

Run: `cargo test -p parse-load-harness`
Expected: every test that has been added so far passes.

- [ ] **Step 4: Commit**

```bash
git add crates/parse-load-harness/src/main.rs
git commit -m "feat(harness): wire main.rs end-to-end (run dispatch)"
```

---

## Task 15: mine-dump binary

**Files:**
- Modify: `crates/parse-load-harness/bin/mine-dump.rs`

- [ ] **Step 1: Implement mine-dump**

Replace `crates/parse-load-harness/bin/mine-dump.rs`:

```rust
//! Mine a Postgres SQL dump into a `shape.json`. Loads the dump into a
//! scratch local PG (via env DATABASE_URL or --pg-url), runs the analytic
//! queries, dumps the result. One-shot tool; the operator runs it once
//! when they have a fresh dump and commits the resulting JSON.

use clap::Parser;
use parse_load_harness::corpus::shape::{PercentileBucket, Shape};
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::process::Command;

#[derive(Parser, Debug)]
struct Args {
    /// Path to the .sql dump file.
    sql_file: PathBuf,
    #[arg(long, default_value = "shape.json")]
    out: PathBuf,
    /// Database URL of a scratch Postgres the dump will be loaded into.
    /// The script DROPs the schema first.
    #[arg(long, env = "DATABASE_URL")]
    pg_url: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let pool = PgPoolOptions::new().connect(&args.pg_url).await?;

    sqlx::query("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;")
        .execute(&pool)
        .await?;
    drop(pool);

    let status = Command::new("psql")
        .args(["-d", &args.pg_url, "-f", args.sql_file.to_str().unwrap()])
        .status()
        .await?;
    anyhow::ensure!(status.success(), "psql -f {} failed", args.sql_file.display());

    let pool = PgPoolOptions::new().connect(&args.pg_url).await?;

    let files_per_bundle = bucket_query(
        &pool,
        "SELECT count(*)::float8 FROM files GROUP BY session_id"
    ).await?;
    let bytes_per_file = bucket_query(
        &pool,
        "SELECT size_bytes::float8 FROM files",
    ).await?;
    let parser_kind_weights = weight_query(
        &pool,
        "SELECT parser_kind, count(*)::float8 FROM files WHERE parser_kind IS NOT NULL GROUP BY parser_kind",
    ).await?;
    let fallback_rate_per_kb = fallback_query(&pool).await?;
    let encoding_weights = BTreeMap::from([
        ("Utf8".to_string(), 0.86),
        ("Utf16Le".to_string(), 0.13),
        ("Other".to_string(), 0.01),
    ]); // Encoding isn't recorded in DB; carry over the default.

    let shape = Shape {
        files_per_bundle,
        bytes_per_file,
        parser_kind_weights,
        fallback_rate_per_kb,
        encoding_weights,
    };
    let bytes = serde_json::to_vec_pretty(&shape)?;
    tokio::fs::write(&args.out, bytes).await?;
    eprintln!("wrote {}", args.out.display());
    Ok(())
}

async fn bucket_query(pool: &sqlx::PgPool, sql: &str) -> anyhow::Result<PercentileBucket> {
    let rows: Vec<(f64,)> = sqlx::query_as(sql).fetch_all(pool).await?;
    let mut v: Vec<f64> = rows.into_iter().map(|(x,)| x).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        return Ok(PercentileBucket { p50: 0, p90: 0, p99: 0, max_observed: 0 });
    }
    let pick = |q: f64| v[((v.len() - 1) as f64 * q) as usize] as u64;
    Ok(PercentileBucket {
        p50: pick(0.50),
        p90: pick(0.90),
        p99: pick(0.99),
        max_observed: *v.last().unwrap() as u64,
    })
}

async fn weight_query(pool: &sqlx::PgPool, sql: &str) -> anyhow::Result<BTreeMap<String, f64>> {
    let rows: Vec<(String, f64)> = sqlx::query_as(sql).fetch_all(pool).await?;
    let total: f64 = rows.iter().map(|(_, c)| c).sum();
    Ok(rows.into_iter().map(|(k, c)| (k, c / total)).collect())
}

async fn fallback_query(pool: &sqlx::PgPool) -> anyhow::Result<BTreeMap<String, f64>> {
    let rows: Vec<(String, f64, f64)> = sqlx::query_as(
        "SELECT parser_kind, sum(parse_error_count)::float8, sum(size_bytes)::float8
         FROM files
         WHERE parser_kind IS NOT NULL
         GROUP BY parser_kind",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(k, errs, sz)| {
            let kb = (sz / 1024.0).max(1.0);
            (k, errs / kb)
        })
        .collect())
}
```

- [ ] **Step 2: Compile**

Run: `cargo check -p parse-load-harness --bin mine-dump`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/parse-load-harness/bin/mine-dump.rs
git commit -m "feat(harness): mine-dump SQL dump -> shape.json"
```

---

## Task 16: README + smoke run

**Files:**
- Create: `crates/parse-load-harness/README.md`

- [ ] **Step 1: Write the operator runbook**

Write `crates/parse-load-harness/README.md`:

```markdown
# parse-load-harness

Drives the api-server's parse pipeline against three targets with three load
shapes and streams structured per-bundle and system-metric output to disk.
See [the design doc](../../docs/superpowers/specs/2026-04-29-parse-load-harness-design.md).

## Quick recipes

### Repeatable in-process benchmark

You'll need a scratch Postgres reachable at the URL you pass.

```sh
cargo run --release -p parse-load-harness -- run \
  --target in-process \
  --in-process-pg postgres://cmtrace:cmtrace@localhost:5432/cmtrace_loadtest \
  --concurrency 8 \
  --total-bundles 200 \
  --seed 42 \
  --out runs/baseline-in-process
```

### Compare two runs

```sh
cargo run --release -p parse-load-harness -- run \
  --target in-process \
  --in-process-pg ... --concurrency 8 --total-bundles 200 --seed 42 \
  --compare-to runs/baseline-in-process \
  --out runs/optimization-test \
  --regression-threshold 5
```

Exit code 1 iff a regression beyond 5% is detected.

### Local stack (full HTTP path)

```sh
cargo build -p api-server --release
docker build -t cmtraceopen-api:latest -f crates/api-server/Dockerfile .

cargo run --release -p parse-load-harness -- run \
  --target local-http \
  --concurrency 32 \
  --soak-minutes 10 \
  --image cmtraceopen-api:latest
```

### Soak against a deployed environment (read-only — leaks tagged rows)

```sh
cargo run --release -p parse-load-harness -- run \
  --target remote-http \
  --target-url https://pilot.cmtrace.net \
  --concurrency 16 \
  --soak-minutes 30
```

Run-tagged rows persist; clean them up later with `clean --cleanup-run=<uuid>`.

### Stress profiles

```sh
... --shape giant-file
... --shape many-small
... --shape unicode-heavy
... --shape broken-binary
... --shape near-cap
... --shape /path/to/custom-shape.json
```

### Real-bundle replay

Drop `*.zip` files into a directory:

```sh
... --corpus ./real-bundles
```

`--shape` is ignored when `--corpus` is given.

## Output

Every run writes three files to `--out`:

- `bundles.jsonl` — one row per bundle (seq, parse_state, finalize_ms, parse_ms, error)
- `system.jsonl` — periodic samples (every `--sample-seconds`)
- `summary.json` — aggregate stats (counts by state, p50/p95/p99, throughput, seed, run UUID)

When `--compare-to` is given, also `comparison.md`.

## Mining a shape from a Postgres dump

```sh
cargo run --release -p parse-load-harness --bin mine-dump -- \
  ./pilot-2026-04-28.sql \
  --pg-url postgres://cmtrace:cmtrace@localhost:5432/scratch \
  --out crates/parse-load-harness/src/corpus/shape.json
```

The resulting `shape.json` is checked in. Re-run when you have a new dump.
```

- [ ] **Step 2: Run a smoke test (no docker)**

Run: `cargo test -p parse-load-harness`
Expected: every test passes (synthetic, replay, driver, reporter, system, compare). No actual Docker or Postgres needed for the unit/integration tests.

- [ ] **Step 3: Final clippy pass**

Run: `cargo clippy -p parse-load-harness --tests --bins -- -D warnings`
Expected: clean. Fix any warnings before committing.

- [ ] **Step 4: Commit**

```bash
git add crates/parse-load-harness/README.md
git commit -m "docs(harness): operator runbook"
```

---

## Self-review

**Spec coverage:**
- ✅ Three targets, three load shapes, single binary — Tasks 7/8/9 + Task 10.
- ✅ Synthetic corpus + replay + profiles — Tasks 4/5.
- ✅ Streaming JSONL + summary + compare — Tasks 11/12/13.
- ✅ X-Device-Id default + mTLS for local-http — Task 6.
- ✅ Auto-cleanup local-http; tag-and-leak remote — Task 9 (compose down -v) + main.rs.
- ✅ mine-dump binary — Task 15.
- ✅ All scale magnitudes — streaming reporter; in-memory state is bounded by per-bundle stats accumulators (the `parse_ms` Vec grows with N — at 100k bundles that's ~800KB, fine; document that the percentile calc loads them all into a Vec).
- ⚠ Spec mentions `infra/compose/load-test.yml` lives at `crates/parse-load-harness/infra/compose/`. Confirmed in Task 9.
- ⚠ Spec § "Implementation note: AppState currently exposes metrics: PrometheusHandle …" — covered by `target.sample_system()` reading `/metrics` for HTTP and using `sysinfo` for in-process. Done.
- ⚠ Spec mentions a `clean` subcommand for remote-http cleanup. Stubbed in Task 3 but not implemented. Acceptable: documented as "not yet implemented" and will be a follow-up. Add a tiny `Task 17: implement clean subcommand` if the maintainer wants it before merge.

**Placeholder scan:** no TODOs, no "implement later", no "similar to Task N". One known gap: the `clean` subcommand returns `bail!` — flagged above.

**Type consistency:** `Bundle`, `BundleResult`, `Target`, `Auth`, `Corpus`, `RunConfig`, `StopCondition`, `DriverInputs`, `ReporterInputs`, `SystemSamplerInputs`, `Comparison` — names match across all task code.

**Known follow-ups for the engineer to file as separate PRs after this lands:**
1. Implement the `clean` subcommand (issue a tagged `DELETE FROM sessions WHERE device_id LIKE 'LOAD-TEST-<uuid>-%'` against the target).
2. Run `mine-dump` against `dev/pilot-dump/pilot-2026-04-28.sql` and replace the seed `shape.json` with real distributions.
3. Add a smoke-run CI job (workflow_dispatch only) that runs `--target=in-process --concurrency=4 --total-bundles=50` and uploads `bundles.jsonl` as an artifact.
4. Vary file encodings in `SyntheticCorpus::build_zip` so the `unicode-heavy` profile actually emits UTF-16LE bytes (currently the encoding distribution is sampled but not consumed).
5. If api-server's storage trait surface needed widening for Task 8, audit those `pub` adds — keep them out of the binary's public surface where possible.
