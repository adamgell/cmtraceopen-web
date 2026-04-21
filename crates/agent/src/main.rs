// cmtraceopen-agent binary entrypoint.
//
// This is the skeleton. Today the Windows build just boots tracing, loads
// config, logs the banner, and parks on ctrl-c. The Linux build is a stub
// so CI can still run `cargo check -p agent` — we ship a single crate, not
// a matrix of per-OS crates.
//
// Planned follow-up work (ordered by how we think we'll land it):
//
//   1. Windows service registration via
//      `windows_service::service_dispatcher::start`, including status
//      reporting (StartPending -> Running -> StopPending -> Stopped) and
//      wiring SERVICE_CONTROL_STOP into graceful shutdown.
//   2. mTLS client cert loading from `Cert:\LocalMachine\My` (or an
//      enrollment-provisioned PFX under %ProgramData%\CMTraceOpen\Agent),
//      then plumbing the resulting `rustls::ClientConfig` into the upload
//      client.
//   3. Collection scheduler (cron-like, driven by `config.evidence_schedule`)
//      that invokes the individual collectors and hands their output to
//      the upload queue.
//   4. Collectors: `logs.rs` (ccmexec.log etc.), `event_logs.rs` (Windows
//      Event Log pull), `dsregcmd.rs` (Entra / AD join status), `evidence.rs`
//      (scheduled full pulls).
//   5. Upload queue: chunked, resumable uploads mirroring the api-server's
//      ingest protocol (see common-wire) plus retry/backoff.
//   6. State DB: SQLite cursor DB at
//      `%ProgramData%\CMTraceOpen\Agent\state.db` recording last-offset
//      per log source + upload queue state.
//   7. MSI packaging via WiX — separate repo track; this crate just
//      produces the .exe the MSI wraps.
//   8. HKLM registry overrides + ADMX policy ingestion.

#![forbid(unsafe_code)]

#[cfg(target_os = "windows")]
mod windows_entry {
    use std::process::ExitCode;

    use cmtraceopen_agent::{banner, config::AgentConfig};
    use tokio::signal;
    use tracing::{info, warn};
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    #[tokio::main]
    pub async fn main() -> ExitCode {
        // Env-var layered config is the right default for the scaffold. Once
        // the service wrapper lands, swap this for the
        // file-then-registry-then-env precedence described in the plan.
        let config = AgentConfig::from_env_or_default();
        init_tracing(&config.log_level);

        info!(banner = %banner(), "cmtraceopen-agent starting");
        info!(
            api_endpoint = %config.api_endpoint,
            request_timeout_secs = config.request_timeout_secs,
            evidence_schedule = %config.evidence_schedule,
            queue_max_bundles = config.queue_max_bundles,
            "loaded config"
        );

        // TODO: register as a Windows service:
        //   windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
        // TODO: load mTLS client cert from LocalMachine\My.
        // TODO: open the SQLite cursor/queue DB.
        // TODO: spawn the collection scheduler.
        // TODO: spawn the upload queue drain task.

        // Scaffold no-op loop: park until ctrl-c so a human can still smoke
        // test the binary from a dev shell.
        match signal::ctrl_c().await {
            Ok(()) => info!("received ctrl-c, shutting down"),
            Err(err) => warn!(%err, "failed to install ctrl-c handler"),
        }

        info!("cmtraceopen-agent stopped cleanly");
        ExitCode::SUCCESS
    }

    fn init_tracing(log_level: &str) {
        // JSON output, matching the api-server so downstream log shippers
        // can parse both streams with one pipeline. RUST_LOG wins if set.
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(format!("cmtraceopen_agent={log_level},warn")));

        tracing_subscriber::registry()
            .with(fmt::layer().json().with_current_span(false))
            .with(filter)
            .init();
    }
}

#[cfg(target_os = "windows")]
fn main() -> std::process::ExitCode {
    windows_entry::main()
}

#[cfg(not(target_os = "windows"))]
fn main() -> std::process::ExitCode {
    // Stub for non-Windows targets. The crate still builds on Linux so CI
    // can run `cargo check -p agent`, but the resulting binary refuses to
    // do anything — all the real surface area is Win32-specific.
    eprintln!(
        "cmtraceopen-agent only runs on Windows (this binary exists in the build matrix for CI compile checks)."
    );
    std::process::ExitCode::from(1)
}
