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
    Run(Box<Args>),
    /// Mine a Postgres SQL dump into shape.json.
    MineDump(MineDumpArgs),
    /// Delete rows tagged with a given run-uuid (remote-http only).
    Clean(CleanArgs),
}

#[derive(Parser, Debug, Clone)]
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
