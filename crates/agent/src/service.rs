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
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use windows_service::define_windows_service;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

use crate::config::AgentConfig;
use crate::runtime;

/// SCM service name — must match what the installer registers.
pub const SERVICE_NAME: &str = "CMTraceOpenAgent";

/// Win32 error code returned when `service_dispatcher::start` is called
/// outside of the SCM context (i.e. from a normal console session).
/// Value: 1063 / 0x427 — ERROR_FAILED_SERVICE_CONTROLLER_CONNECT.
const ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: i32 = 1063;

/// Wait hint we report alongside `StartPending`. The SCM uses this to
/// decide how long to wait before assuming the service is hung. We bound
/// it generously (queue open + work-dir create + uploader init can hit
/// disk on a cold-boot) but stay well under the SCM default timeout.
const START_PENDING_WAIT_HINT: Duration = Duration::from_secs(30);

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

/// Core service lifecycle: register handler, report StartPending,
/// initialise components, report Running, run tasks, report Stopped.
fn run_service(_args: Vec<OsString>) -> Result<(), Box<dyn std::error::Error>> {
    // Initialise tracing before anything else so early errors are captured.
    // The guard has to outlive this function for the non-blocking appender
    // to flush on drop — bind it to a _guard local that the compiler keeps
    // alive for the whole run_service scope.
    let config = load_service_config();
    let _log_guard = init_service_tracing(&config.log_level);

    info!(
        service = SERVICE_NAME,
        api_endpoint = %config.api_endpoint,
        schedule = %config.evidence_schedule,
        "service_main entered"
    );

    // Stop-signal channel shared between the control handler (sender) and the
    // async task loop (receiver).
    let (stop_tx, stop_rx) = watch::channel(false);

    // Register the SCM control handler. The closure is called on a separate
    // thread managed by the SCM, so it must be `Send + 'static`.
    //
    // We accept both `Stop` and `Shutdown`. `Shutdown` is sent by the SCM
    // when the OS itself is shutting down — without opting in we'd be killed
    // mid-upload instead of getting our bounded final-drain window.
    let status_handle =
        service_control_handler::register(SERVICE_NAME, move |ctrl| match ctrl {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                info!(?ctrl, "SCM stop/shutdown received");
                // Best-effort: ignore send errors (receiver may already be gone).
                let _ = stop_tx.send(true);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        })?;

    // Report StartPending while we initialise. On a fresh install with a
    // cold disk, queue dir creation + uploader TLS setup can take real
    // wall-clock; reporting Pending first lets operators see "service is
    // starting" in the event log instead of a Running -> Stopped flap if
    // init fails.
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::StartPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 1,
        wait_hint: START_PENDING_WAIT_HINT,
        process_id: None,
    })?;
    info!("service status set to StartPending");

    // Build a tokio runtime up-front so component construction (which is
    // async) can run on it. The runtime lives for the life of the service.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Initialise queue / orchestrator / uploader before flipping to Running.
    // If this fails the service stops with a non-zero exit code and the SCM
    // event log shows the StartPending → Stopped transition (with the error
    // we logged via tracing).
    let components = match rt.block_on(runtime::build_components(&config)) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "component init failed");
            // Report Stopped with non-zero exit so SCM surfaces the failure.
            let _ = status_handle.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: ServiceState::Stopped,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::Win32(1),
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            });
            return Err(e);
        }
    };

    // Now we're ready to accept work — flip to Running and accept Stop +
    // Shutdown.
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;
    info!("service status set to Running");

    // Drive the shared task loop until a stop/shutdown signal arrives.
    rt.block_on(runtime::run_task_loop(&components, stop_rx));

    // Report Stopped (clean exit; runtime::run_task_loop returns `()` so
    // we always treat the post-loop state as success).
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;
    info!("service status set to Stopped");

    Ok(())
}

/// `%ProgramData%\CMTraceOpen\Agent` — ships-with-the-MSI layout used by
/// both the installer's `config.toml` and the file log appender below.
fn agent_program_data_dir() -> PathBuf {
    let base = std::env::var("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("C:\\ProgramData"));
    base.join("CMTraceOpen").join("Agent")
}

/// Load the config the service actually runs with. Order of precedence:
///   1. TOML file at `%ProgramData%\CMTraceOpen\Agent\config.toml` (shipped
///      by the MSI; operator-editable).
///   2. CMTRACE_* env-var overlay on top of whatever 1 produced.
///   3. Hardcoded defaults (never the production path — api.corp.example.com
///      only hits here on a bare CLI test run with no file + no env).
///
/// Historical note: the service used to call `from_env_or_default()` directly
/// which silently ignored the shipped TOML and hit the example.com default.
/// Every bundle enqueued under that bug failed with a DNS-ish network error
/// (see queue sidecars with `last_error: ... api.corp.example.com`).
fn load_service_config() -> AgentConfig {
    let cfg_path = agent_program_data_dir().join("config.toml");
    let base = match AgentConfig::from_file(&cfg_path) {
        Ok(c) => {
            // `tracing` isn't initialised yet, so emit this to stderr — it
            // survives iff the service was launched from a console (CLI
            // diag mode). Under SCM stderr is discarded; the follow-up
            // `info!` inside run_service captures the same api_endpoint
            // once the file appender is up.
            eprintln!("config: loaded {}", cfg_path.display());
            c
        }
        Err(e) => {
            eprintln!(
                "config: could not load {} ({e}); falling back to env/default. \
                 This is almost certainly a packaging bug — expected the MSI \
                 to have dropped a config.toml at that path.",
                cfg_path.display()
            );
            AgentConfig::from_env_or_default()
        }
    };
    apply_env_overrides(base)
}

/// Layer env-var overrides on an already-loaded config. Mirrors the env
/// knobs in `AgentConfig::from_env_or_default` so operators can still
/// break-glass with `CMTRACE_API_ENDPOINT=...` without editing the TOML.
fn apply_env_overrides(mut cfg: AgentConfig) -> AgentConfig {
    if let Ok(v) = std::env::var("CMTRACE_API_ENDPOINT") {
        cfg.api_endpoint = v;
    }
    if let Ok(v) = std::env::var("CMTRACE_REQUEST_TIMEOUT_SECS") {
        if let Ok(n) = v.parse::<u64>() {
            cfg.request_timeout_secs = n;
        }
    }
    if let Ok(v) = std::env::var("CMTRACE_LOG_LEVEL") {
        cfg.log_level = v;
    }
    cfg
}

/// Install the tracing subscriber. Writes JSON lines to a daily-rolling
/// file in `%ProgramData%\CMTraceOpen\Agent\logs\agent.log[.YYYY-MM-DD]`.
///
/// Why a file sink: the Windows SCM discards stdout/stderr from service
/// processes. The previous implementation wrote to stderr assuming SCM
/// captured it, which left the agent silent in production — no scheduler
/// ticks, no upload attempts, no errors. The only diagnostic signal was
/// each bundle's per-queue sidecar JSON (state=failed, last_error=...),
/// which required SSH-like access to read.
///
/// Returns a `WorkerGuard` the caller must keep alive for the whole
/// service lifetime so the non-blocking writer flushes on shutdown.
fn init_service_tracing(log_level: &str) -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("cmtraceopen_agent={log_level},warn")));

    let log_dir = agent_program_data_dir().join("logs");
    // Best-effort: directory creation failures degrade to stderr-only (still
    // useful from a CLI run) rather than panicking the service.
    let dir_ok = std::fs::create_dir_all(&log_dir).is_ok();

    let (file_writer, guard) = if dir_ok {
        let appender = tracing_appender::rolling::daily(&log_dir, "agent.log");
        let (nb, g) = tracing_appender::non_blocking(appender);
        (Some(nb), Some(g))
    } else {
        (None, None)
    };

    let registry = tracing_subscriber::registry().with(filter);

    let init_result = if let Some(writer) = file_writer {
        registry
            .with(fmt::layer().json().with_current_span(false).with_writer(writer))
            .try_init()
    } else {
        registry
            .with(fmt::layer().json().with_current_span(false))
            .try_init()
    };

    if let Err(e) = init_result {
        // Re-entered `service_main` in the same process (MSI repair, SCM
        // restart-on-failure). The existing subscriber is still live; emit
        // a warn through it and carry on.
        warn!(error = %e, "tracing subscriber already initialised; reusing existing");
    }

    guard
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
