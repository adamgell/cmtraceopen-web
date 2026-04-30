use clap::Parser;
use parse_load_harness::auth::{self, Auth};
use parse_load_harness::config::{Args, AuthKind, Cli, Cmd, RunConfig, TargetKind};
use parse_load_harness::corpus::{replay::ReplayCorpus, shape::Shape, synthetic::SyntheticCorpus, Corpus};
use parse_load_harness::driver::{run_driver, DriverInputs};
use parse_load_harness::reporter::{
    compare::Comparison, run_reporter, system::run_system_sampler, ReporterInputs,
    system::SystemSamplerInputs,
};
use parse_load_harness::target::{
    in_process::InProcessTarget, local_http::LocalHttpTarget, remote_http::RemoteHttpTarget,
    Target,
};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Boxed async shutdown callback, used to tear down `LocalHttpTarget` after
/// dropping the shared `Arc<dyn Target>`.
type ShutdownFn = Box<dyn FnOnce() -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>>>>>;

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
        Cmd::Run(args) => run_command(*args).await,
        Cmd::MineDump(_) => {
            anyhow::bail!("use the mine-dump binary directly: cargo run --bin mine-dump")
        }
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

    // Construct auth. For mTLS we keep a typed reference so we can extract the
    // CA + a leaf cert before erasing to Arc<dyn Auth>.
    let mtls_typed: Option<auth::mtls::MtlsAuth> = match cfg.args.auth {
        AuthKind::Mtls => Some(auth::mtls::MtlsAuth::new()?),
        AuthKind::Header => None,
    };
    let auth: Arc<dyn Auth> = match (&cfg.args.auth, &mtls_typed) {
        (AuthKind::Header, _) => Arc::new(auth::header::HeaderAuth),
        (AuthKind::Mtls, Some(_)) => {
            // Re-mint a fresh MtlsAuth for the trait object — cheap, and avoids
            // the downcast dance.
            Arc::new(auth::mtls::MtlsAuth::new()?)
        }
        (AuthKind::Mtls, None) => unreachable!(),
    };

    // Build the target. We keep a separate shutdown callback so we don't need
    // to unwrap the Arc<dyn Target> at the end (which is unsized and can't be
    // moved out of an Arc directly).
    let shutdown_fn: Option<ShutdownFn>;

    let target: Arc<dyn Target> = match cfg.args.target {
        TargetKind::InProcess => {
            let pg = cfg
                .args
                .in_process_pg
                .clone()
                .ok_or_else(|| anyhow::anyhow!("--target=in-process requires --in-process-pg"))?;
            let t = Arc::new(InProcessTarget::new(pg).await?);
            shutdown_fn = None; // shutdown is a no-op for in-process
            t
        }
        TargetKind::LocalHttp => {
            let compose = PathBuf::from("crates/parse-load-harness/infra/compose/load-test.yml");
            let ca_dir = if let Some(mtls) = mtls_typed.as_ref() {
                let dir = tempfile::tempdir()?;
                let (cert, key) = mtls.leaf_for("LOAD-TEST-LEAF-0001")?;
                tokio::fs::write(dir.path().join("ca.pem"), mtls.ca_pem()).await?;
                tokio::fs::write(dir.path().join("client.crt"), cert).await?;
                tokio::fs::write(dir.path().join("client.key"), key).await?;
                Some(dir.keep())
            } else {
                None
            };
            let t = LocalHttpTarget::up(
                compose,
                cfg.args.image.clone(),
                ca_dir,
                auth.clone(),
                cfg.args.no_cleanup,
            )
            .await?;
            let t = Arc::new(t);
            let weak = Arc::downgrade(&t);
            shutdown_fn = Some(Box::new(move || {
                Box::pin(async move {
                    match weak.upgrade() {
                        None => Ok(()),
                        Some(arc) => match Arc::try_unwrap(arc) {
                            Ok(inner) => Box::new(inner).shutdown().await,
                            Err(_) => {
                                tracing::warn!("target had outstanding refs at shutdown; skipping teardown");
                                Ok(())
                            }
                        },
                    }
                })
            }));
            t
        }
        TargetKind::RemoteHttp => {
            let url = cfg.args.target_url.clone().unwrap();
            let t = Arc::new(RemoteHttpTarget::new(url, reqwest::Client::new(), auth.clone()));
            shutdown_fn = None; // remote-http shutdown is a no-op
            t
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
    if let Some(threshold) = cfg.args.max_rss_mb {
        let path = cfg.out_dir.join("system.jsonl");
        let mut max_rss: u64 = 0;
        if let Ok(body) = tokio::fs::read_to_string(&path).await {
            for line in body.lines() {
                let v: serde_json::Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(rss) = v.get("rss_mb").and_then(|r| r.as_u64()) {
                    max_rss = max_rss.max(rss);
                }
            }
        }
        if max_rss > threshold {
            tracing::warn!(max_rss, threshold, "RSS exceeded --max-rss-mb threshold");
            exit_code = 1;
        } else {
            tracing::info!(max_rss, threshold, "max observed RSS within threshold");
        }
    }

    if let Some(prev) = &cfg.args.compare_to {
        let cmp = Comparison::build(prev, &cfg.out_dir, cfg.args.regression_threshold)?;
        let md = cmp.render_markdown();
        tokio::fs::write(cfg.out_dir.join("comparison.md"), md).await?;
        if cmp.has_regression() {
            tracing::warn!(
                "regression detected past threshold {}%",
                cfg.args.regression_threshold
            );
            exit_code = 1;
        }
    }

    if cfg.args.strict && summary.error_count > 0 {
        exit_code = 1;
    }
    if let Some(rate) = cfg.args.max_error_rate {
        let observed = if summary.total_bundles == 0 {
            0.0
        } else {
            summary.error_count as f64 / summary.total_bundles as f64
        };
        if observed > rate {
            exit_code = 1;
        }
    }

    // Tear down target. Drop the shared Arc first so the shutdown callback
    // can attempt to take sole ownership.
    drop(target);
    if let Some(f) = shutdown_fn {
        f().await?;
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}
