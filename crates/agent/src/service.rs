// Windows service dispatcher integration for cmtraceopen-agent.
//
// Registers the binary as a proper Windows service so the SCM can start/stop
// it. When the process is invoked by the SCM the `try_run_as_service` entry
// point takes over; when it is invoked from a normal console the dispatcher
// returns ERROR_FAILED_SERVICE_CONTROLLER_CONNECT (1063) and the caller falls
// through to the existing CLI mode.
//
// The `define_windows_service!` macro expands to an `extern "system"` FFI
// trampoline that contains compiler-generated unsafe. We allow unsafe in this
// module only; the rest of the crate remains under `#[deny(unsafe_code)]`.

#![allow(unsafe_code)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use windows_service::define_windows_service;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

use crate::collectors::dsregcmd::DsRegCmdCollector;
use crate::collectors::event_logs::EventLogsCollector;
use crate::collectors::evidence::EvidenceOrchestrator;
use crate::collectors::logs::LogsCollector;
use crate::config::AgentConfig;
use crate::queue::{Queue, QueueState};
use crate::tls::TlsClientOptions;
use crate::uploader::{Uploader, UploaderConfig};

/// SCM service name — must match what the installer registers.
pub const SERVICE_NAME: &str = "CMTraceOpenAgent";

/// Win32 error code returned when `service_dispatcher::start` is called
/// outside of the SCM context (i.e. from a normal console session).
/// Value: 1063 / 0x427 — ERROR_FAILED_SERVICE_CONTROLLER_CONNECT.
const ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: i32 = 1063;

/// How often to run an evidence collection pass.
const COLLECT_INTERVAL: Duration = Duration::from_secs(60 * 15);

/// How often to drain the upload queue.
const DRAIN_INTERVAL: Duration = Duration::from_secs(30);

/// Queue-level backoff when an upload fails.
const QUEUE_FAIL_BACKOFF: Duration = Duration::from_secs(300);

/// Maximum time the service waits for an in-flight drain to complete on Stop.
const STOP_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

// FFI trampoline produced by the macro. The macro expands into an
// `extern "system" fn ffi_service_main(...)` which is the entry point the
// SCM calls after `service_dispatcher::start` connects.
define_windows_service!(ffi_service_main, service_main);

/// Called by the SCM via the `ffi_service_main` trampoline.
fn service_main(args: Vec<OsString>) {
    if let Err(e) = run_service(args) {
        error!(error = %e, "service_main failed");
    }
}

/// Core service lifecycle: register handler, report Running, run tasks,
/// report Stopped.
fn run_service(_args: Vec<OsString>) -> Result<(), Box<dyn std::error::Error>> {
    // Initialise tracing before anything else so early errors are captured.
    let config = AgentConfig::from_env_or_default();
    init_service_tracing(&config.log_level);

    info!(service = SERVICE_NAME, "service_main entered");

    // Stop-signal channel shared between the control handler (sender) and the
    // async task loop (receiver).
    let (stop_tx, stop_rx) = watch::channel(false);

    // Register the SCM control handler. The closure is called on a separate
    // thread managed by the SCM, so it must be `Send + 'static`.
    let status_handle =
        service_control_handler::register(SERVICE_NAME, move |ctrl| match ctrl {
            ServiceControl::Stop => {
                info!("SCM Stop received");
                // Best-effort: ignore send errors (receiver may already be gone).
                let _ = stop_tx.send(true);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        })?;

    // Report Running to the SCM.
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;
    info!("service status set to Running");

    // Build a tokio runtime and run the long-lived tasks.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = rt.block_on(run_tasks(config, stop_rx));
    if let Err(ref e) = result {
        error!(error = %e, "run_tasks returned an error");
    }

    // Report Stopped regardless of whether run_tasks succeeded.
    let exit_code = if result.is_ok() {
        ServiceExitCode::Win32(0)
    } else {
        ServiceExitCode::Win32(1)
    };
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;
    info!("service status set to Stopped");

    result
}

/// Async task loop: collects evidence and drains the upload queue until a
/// stop signal arrives, then drains once more (with a timeout) before
/// returning.
async fn run_tasks(
    config: AgentConfig,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error>> {
    let queue_root = Queue::default_root();
    let queue = Queue::open(&queue_root).await?;

    let work_root = queue_root
        .parent()
        .map(|p| p.join("work"))
        .unwrap_or_else(|| PathBuf::from("./work"));
    tokio::fs::create_dir_all(&work_root).await?;

    let orchestrator = EvidenceOrchestrator::new(
        LogsCollector::new(config.log_paths.clone()),
        EventLogsCollector::with_defaults(),
        DsRegCmdCollector::new(),
        work_root.clone(),
    );
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

    let mut collect_tick = tokio::time::interval(COLLECT_INTERVAL);
    let mut drain_tick = tokio::time::interval(DRAIN_INTERVAL);

    // Skip the first immediate collect tick — let the daemon finish booting.
    // Drain fires immediately so crash-survivor queue entries are uploaded
    // quickly after a restart.
    collect_tick.tick().await;

    info!("entering service task loop");

    loop {
        tokio::select! {
            _ = collect_tick.tick() => {
                svc_collect_and_enqueue(&orchestrator, &queue, &work_root).await;
            }
            _ = drain_tick.tick() => {
                svc_drain(&queue, &uploader).await;
            }
            result = stop_rx.changed() => {
                if result.is_err() || *stop_rx.borrow() {
                    info!("stop signal received; draining in-flight work");
                    // One last drain with a bounded timeout so in-flight uploads
                    // can complete before we report Stopped to the SCM.
                    match tokio::time::timeout(
                        STOP_DRAIN_TIMEOUT,
                        svc_drain(&queue, &uploader),
                    )
                    .await
                    {
                        Ok(()) => info!("final drain completed"),
                        Err(_) => warn!(
                            timeout_secs = STOP_DRAIN_TIMEOUT.as_secs(),
                            "final drain timed out"
                        ),
                    }
                    break;
                }
            }
        }
    }

    info!("service task loop exited");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers — mirrors of the same-named functions in main.rs, scoped to the
// service module so main.rs doesn't need to expose private helpers.
// ---------------------------------------------------------------------------

async fn svc_collect_and_enqueue(
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
            if let Err(e) = tokio::fs::remove_dir_all(&bundle.staging_dir).await {
                warn!(dir = %bundle.staging_dir.display(), error = %e, "failed to clean staging dir");
            }
        }
        Err(e) => {
            warn!(error = %e, "collection failed");
        }
    }
}

async fn svc_drain(queue: &Queue, uploader: &Uploader) {
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

    if let Ok(current) = queue.get(bundle_id).await {
        if matches!(current.state, QueueState::Done { .. }) {
            if let Err(e) = tokio::fs::remove_file(&current.zip_path).await {
                warn!(%bundle_id, error = %e, "post-upload zip purge failed");
            }
        }
    }
}

fn init_service_tracing(log_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("cmtraceopen_agent={log_level},warn")));

    // SCM captures stderr, so write JSON lines there.
    tracing_subscriber::registry()
        .with(fmt::layer().json().with_current_span(false))
        .with(filter)
        .init();
}

// ---------------------------------------------------------------------------
// Public entry point called by main.rs
// ---------------------------------------------------------------------------

/// Attempt to run the process as a Windows service.
///
/// Returns:
/// - `None`  — the process was **not** invoked by the SCM
///   (`ERROR_FAILED_SERVICE_CONTROLLER_CONNECT`); the caller should fall
///   through to CLI mode.
/// - `Some(ExitCode::SUCCESS)` — the service ran and stopped cleanly.
/// - `Some(ExitCode::FAILURE)` — the service dispatcher returned an
///   unexpected error.
pub fn try_run_as_service() -> Option<ExitCode> {
    match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => Some(ExitCode::SUCCESS),
        Err(windows_service::Error::Winapi(ref e))
            if e.raw_os_error() == Some(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT) =>
        {
            // Not running under SCM — tell the caller to use CLI mode.
            None
        }
        Err(e) => {
            error!(error = %e, "service_dispatcher::start failed");
            Some(ExitCode::FAILURE)
        }
    }
}
