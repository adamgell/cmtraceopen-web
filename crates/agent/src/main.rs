// cmtraceopen-agent binary entrypoint.
//
// Wave 2 M1 shape: runs as a foreground daemon (or a `--oneshot` mode
// for testing) that collects evidence, enqueues it, and drains the
// queue to the api-server. On Windows the binary also registers as a
// proper SCM service via `crates/agent/src/service.rs`; when invoked
// from a console it falls through to CLI/daemon mode automatically.
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

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use cmtraceopen_agent::collectors::dsregcmd::DsRegCmdCollector;
use cmtraceopen_agent::collectors::event_logs::EventLogsCollector;
use cmtraceopen_agent::collectors::evidence::EvidenceOrchestrator;
use cmtraceopen_agent::collectors::logs::LogsCollector;
use cmtraceopen_agent::config::AgentConfig;
use cmtraceopen_agent::queue::{Queue, QueueState};
use cmtraceopen_agent::tls::TlsClientOptions;
use cmtraceopen_agent::uploader::{Uploader, UploaderConfig};
use cmtraceopen_agent::banner;
use tokio::signal;
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// MVP collection cadence when running in foreground-daemon mode. The
/// real scheduler reads `config.evidence_schedule` — see TODO above.
const COLLECT_INTERVAL: Duration = Duration::from_secs(60 * 15);

/// How often to drain the upload queue. Independent of the collect
/// cadence so failed uploads still retry on a short clock even if new
/// bundles are collected slowly.
const DRAIN_INTERVAL: Duration = Duration::from_secs(30);

/// Backoff applied to a bundle that failed upload. Separate from the
/// per-HTTP-call retry inside [`Uploader`] — this is queue-level.
const QUEUE_FAIL_BACKOFF: Duration = Duration::from_secs(300);

#[tokio::main]
async fn main() -> ExitCode {
    // On Windows: try to connect to the SCM first. If we are running as a
    // service the dispatcher takes over and this call never returns until the
    // service stops. If we are running from a console
    // (ERROR_FAILED_SERVICE_CONTROLLER_CONNECT), it returns `None` and we fall
    // through to CLI mode below.
    #[cfg(windows)]
    if let Some(exit_code) = cmtraceopen_agent::service::try_run_as_service() {
        return exit_code;
    }

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
    // Queue lives under %ProgramData% in production; the integration
    // test overrides via a different `Queue::open(..)` path.
    let queue_root = Queue::default_root();
    let queue = Queue::open(&queue_root).await?;

    // Staging area for in-progress evidence collection. Sibling of the
    // queue so a `du -sh` over the agent's data dir is one walk.
    let work_root = queue_root
        .parent()
        .map(|p| p.join("work"))
        .unwrap_or_else(|| PathBuf::from("./work"));
    tokio::fs::create_dir_all(&work_root).await?;

    let orchestrator = build_orchestrator(&config, work_root.clone());
    let uploader = Uploader::new(
        UploaderConfig::new(
            config.api_endpoint.clone(),
            config.resolved_device_id(),
            Duration::from_secs(config.request_timeout_secs),
        )
        .with_tls(TlsClientOptions {
            client_cert_pem: config.tls_client_cert_pem.clone(),
            client_key_pem: config.tls_client_key_pem.clone(),
            ca_bundle_pem: config.tls_ca_bundle_pem.clone(),
        }),
    )?;

    if oneshot {
        // One pass: collect + enqueue + drain once, exit.
        collect_and_enqueue(&orchestrator, &queue, &work_root).await;
        drain(&queue, &uploader).await;
        info!("oneshot complete");
        return Ok(());
    }

    // Daemon mode: two concurrent tasks + ctrl-c.
    let mut collect_tick = tokio::time::interval(COLLECT_INTERVAL);
    let mut drain_tick = tokio::time::interval(DRAIN_INTERVAL);
    // Skip the initial immediate tick on the collect side — we don't want
    // to fire a collection before the daemon is even done booting. Drain
    // we DO want right away in case the queue has crash-survivor entries.
    collect_tick.tick().await;

    loop {
        tokio::select! {
            _ = collect_tick.tick() => {
                collect_and_enqueue(&orchestrator, &queue, &work_root).await;
            }
            _ = drain_tick.tick() => {
                drain(&queue, &uploader).await;
            }
            result = signal::ctrl_c() => {
                if let Err(e) = result {
                    warn!(error = %e, "ctrl-c handler failed");
                }
                info!("received shutdown signal, exiting daemon loop");
                break;
            }
        }
    }

    info!("cmtraceopen-agent stopped cleanly");
    Ok(())
}

fn build_orchestrator(config: &AgentConfig, work_root: PathBuf) -> EvidenceOrchestrator {
    EvidenceOrchestrator::new(
        LogsCollector::new(config.log_paths.clone()),
        EventLogsCollector::with_defaults(),
        DsRegCmdCollector::new(),
        work_root,
    )
}

/// Run one collect pass and enqueue the result. Errors are logged — a
/// transient collection failure shouldn't tear the daemon down.
async fn collect_and_enqueue(
    orch: &EvidenceOrchestrator,
    queue: &Queue,
    work_root: &std::path::Path,
) {
    match orch.collect_once().await {
        Ok(bundle) => {
            let bundle_id = bundle.metadata.bundle_id;
            match queue.enqueue(bundle.metadata, &bundle.zip_path).await {
                Ok(_) => info!(%bundle_id, "bundle enqueued"),
                Err(e) => warn!(%bundle_id, error = %e, "enqueue failed"),
            }
            // Clean up the collector staging dir; zip has been moved.
            if let Err(e) = tokio::fs::remove_dir_all(&bundle.staging_dir).await {
                warn!(dir = %bundle.staging_dir.display(), error = %e, "failed to clean staging dir");
            }
        }
        Err(e) => {
            warn!(error = %e, "collection failed");
            // Touch work_root so rustc doesn't optimize the borrow away
            // in release — also handy for future use when we partition
            // staging by collection run id.
            let _ = work_root;
        }
    }
}

/// Drain pending bundles from the queue. Upload errors are recorded on
/// the queue entry so the bundle is retried on the next drain tick.
async fn drain(queue: &Queue, uploader: &Uploader) {
    // MVP: process one pending bundle per drain tick. Keeps the drain
    // cadence predictable and prevents a burst of queued bundles from
    // hogging the reactor.
    let next = match queue.next_pending().await {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "queue read failed");
            return;
        }
    };
    let Some(entry) = next else {
        return;
    };
    let bundle_id = entry.metadata.bundle_id;
    if let Err(e) = queue.mark_uploading(bundle_id).await {
        warn!(%bundle_id, error = %e, "mark_uploading failed");
        return;
    }

    match uploader.upload(&entry.metadata, &entry.zip_path).await {
        Ok(resp) => {
            info!(
                %bundle_id,
                session_id = %resp.session_id,
                parse_state = %resp.parse_state,
                "upload succeeded"
            );
            if let Err(e) = queue.mark_done(bundle_id).await {
                warn!(%bundle_id, error = %e, "mark_done failed");
            }
        }
        Err(e) => {
            warn!(%bundle_id, error = %e, "upload failed; will retry");
            if let Err(markerr) = queue
                .mark_failed(bundle_id, e.to_string(), QUEUE_FAIL_BACKOFF)
                .await
            {
                warn!(%bundle_id, error = %markerr, "mark_failed failed");
            }
        }
    }

    // If we successfully uploaded and the entry is now Done, we can
    // purge the bundle zip immediately — but keep the sidecar so ops
    // can see the Done state. For MVP we purge zips only; the sidecar
    // sweeper comes later.
    if let Ok(current) = queue.get(bundle_id).await {
        if matches!(current.state, QueueState::Done { .. }) {
            if let Err(e) = tokio::fs::remove_file(&current.zip_path).await {
                warn!(%bundle_id, error = %e, "post-upload zip purge failed");
            }
        }
    }
}

fn init_tracing(log_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("cmtraceopen_agent={log_level},warn")));

    tracing_subscriber::registry()
        .with(fmt::layer().json().with_current_span(false))
        .with(filter)
        .init();
}
