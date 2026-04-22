// cmtraceopen-agent binary entrypoint.
//
// Wave 2 M1 shape: runs as a foreground daemon (or a `--oneshot` mode
// for testing) that collects evidence, enqueues it, and drains the
// queue to the api-server. On Windows the binary also registers as a
// proper SCM service via `crates/agent/src/service.rs`; when invoked
// from a console it falls through to CLI/daemon mode automatically.
//
// **`main` is intentionally a synchronous `fn`** — not `#[tokio::main]`.
// On Windows, `service_dispatcher::start` is a blocking Win32 call that
// does not return until the service stops, and the SCM-spawned
// `service_main` builds its own multi-threaded tokio runtime. If we
// kept `#[tokio::main]` here we'd block an outer tokio worker for the
// service's entire lifetime AND nest a second runtime inside it — at
// minimum a wasted runtime + thread, at worst a nested-runtime
// deadlock. The CLI path builds its own runtime explicitly below.
//
// Planned follow-up work:
//
//   1. mTLS client cert loading from `Cert:\LocalMachine\My` (or a
//      provisioned PFX under `%ProgramData%\CMTraceOpen\Agent`). Until
//      that lands, identity comes from the `X-Device-Id` header via the
//      `CMTRACE_DEVICE_ID` config knob.
//   2. Collection scheduler (cron-like from `config.evidence_schedule`).
//      MVP loop runs on a simple `tokio::time::interval` and the
//      `--oneshot` flag skips the loop entirely.
//   3. HKLM registry overrides + ADMX policy ingestion.

// `deny` rather than `forbid`: the Windows service module
// (`crates/agent/src/service.rs`) expands the `define_windows_service!`
// macro which contains an `extern "system"` FFI trampoline. That module
// carries its own `#[allow(unsafe_code)]`; the rest of the crate still
// has unsafe forbidden via `deny`.
#![deny(unsafe_code)]

use std::process::ExitCode;

use cmtraceopen_agent::config::AgentConfig;
use cmtraceopen_agent::runtime::{self, AgentComponents};
use cmtraceopen_agent::banner;
use tokio::signal;
use tokio::sync::watch;
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

fn main() -> ExitCode {
    // On Windows: try to connect to the SCM first. If we are running as a
    // service the dispatcher takes over and this call never returns until the
    // service stops. If we are running from a console
    // (ERROR_FAILED_SERVICE_CONTROLLER_CONNECT), it returns `None` and we fall
    // through to CLI mode below. Done in a sync context BEFORE any tokio
    // runtime exists so the service-side runtime doesn't nest.
    #[cfg(windows)]
    if let Some(exit_code) = cmtraceopen_agent::service::try_run_as_service() {
        return exit_code;
    }

    // CLI mode: build the runtime here. The service path builds its own
    // runtime inside `service_main` so the two never coexist.
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to build tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    rt.block_on(run_cli())
}

/// CLI / foreground-daemon entry point. Owned by the runtime built in
/// `main` above.
async fn run_cli() -> ExitCode {
    // Minimal arg parse — one flag, no need for a full arg crate.
    let oneshot = std::env::args().any(|a| a == "--oneshot");

    let config = AgentConfig::from_env_or_default();
    init_tracing(&config.log_level);

    info!(banner = %banner(), oneshot, "cmtraceopen-agent starting");
    info!(
        api_endpoint = %config.api_endpoint,
        device_id = %config.resolved_device_id(),
        queue_max_bundles = config.queue_max_bundles,
        "loaded config"
    );

    match run(config, oneshot).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(error = %e, "agent exited with error");
            ExitCode::from(1)
        }
    }
}

/// Concrete runtime. Returns on graceful shutdown or fatal error.
async fn run(config: AgentConfig, oneshot: bool) -> Result<(), Box<dyn std::error::Error>> {
    let components: AgentComponents = runtime::build_components(&config).await?;

    if oneshot {
        // One pass: collect + enqueue + drain once, exit.
        runtime::collect_and_enqueue(
            &components.orchestrator,
            &components.queue,
            &components.work_root,
        )
        .await;
        runtime::drain(&components.queue, &components.uploader).await;
        info!("oneshot complete");
        return Ok(());
    }

    // Daemon mode: drive the shared task loop with a watch channel that
    // ctrl-c flips to `true`. Mirrors how `service.rs` drives the same
    // loop from the SCM control handler. The CollectionScheduler module
    // (added by this PR) is wired into the task loop in a follow-up;
    // for now run_task_loop runs the queue drainer with the existing
    // tick-based collection cadence.
    let (stop_tx, stop_rx) = watch::channel(false);

    let task_loop = tokio::spawn(async move {
        runtime::run_task_loop(&components, stop_rx).await
    });

    match signal::ctrl_c().await {
        Ok(()) => info!("received shutdown signal, exiting daemon loop"),
        Err(e) => warn!(error = %e, "ctrl-c handler failed; exiting"),
    }
    let _ = stop_tx.send(true);

    if let Err(e) = task_loop.await {
        warn!(error = %e, "task loop join failed");
    }

    info!("cmtraceopen-agent stopped cleanly");
    Ok(())
}

fn init_tracing(log_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("cmtraceopen_agent={log_level},warn")));

    // CLI: use the global subscriber. `try_init` is fine here too, but
    // CLI mode constructs the subscriber exactly once per process.
    let _ = tracing_subscriber::registry()
        .with(fmt::layer().json().with_current_span(false))
        .with(filter)
        .try_init();
}

