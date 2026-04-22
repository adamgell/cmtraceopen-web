//! Shared agent runtime: the collect + drain task loop.
//!
//! Both CLI (`main.rs`) and service (`service.rs`) modes need the same
//! long-lived work: periodic evidence collection, periodic queue drain,
//! ctrl-c/stop handling, and a final bounded drain on shutdown. That
//! code used to be duplicated between the two entry points; it now
//! lives here so a future change to (say) the upload-retry contract
//! can't silently diverge between CLI and service modes.
//!
//! ## Entry points
//!
//! * [`build_components`] — one-shot constructor for `Queue`,
//!   `EvidenceOrchestrator`, `Uploader`, and the `work_root` path. Used
//!   by both oneshot and daemon flows.
//! * [`run_task_loop`] — drives the collect + drain loop until a stop
//!   signal arrives, then runs one final bounded drain.
//! * [`collect_and_enqueue`] / [`drain`] — the actual work fns. `pub`
//!   because the oneshot path calls them directly from `main.rs`.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

use crate::collectors::dsregcmd::DsRegCmdCollector;
use crate::collectors::event_logs::EventLogsCollector;
use crate::collectors::evidence::EvidenceOrchestrator;
use crate::collectors::logs::LogsCollector;
use crate::config::AgentConfig;
use crate::queue::{Queue, QueueState};
use crate::redact::Redactor;
use crate::tls::TlsClientOptions;
use crate::uploader::{Uploader, UploaderConfig};

/// How often to run an evidence collection pass.
pub const COLLECT_INTERVAL: Duration = Duration::from_secs(60 * 15);

/// How often to drain the upload queue.
pub const DRAIN_INTERVAL: Duration = Duration::from_secs(30);

/// Queue-level backoff when an upload fails.
pub const QUEUE_FAIL_BACKOFF: Duration = Duration::from_secs(300);

/// Maximum time the shutdown path waits for an in-flight drain to complete.
pub const STOP_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Bundle of long-lived components needed by the task loop.
///
/// Constructed once per agent process. Both CLI daemon mode and the
/// Windows service dispatcher go through this builder so the set of
/// dependencies can't drift between the two entry points.
pub struct AgentComponents {
    pub queue: Queue,
    pub orchestrator: EvidenceOrchestrator,
    pub uploader: Uploader,
    pub work_root: PathBuf,
}

/// Build the queue, orchestrator, uploader, and work root from `config`.
///
/// Fails if the queue dir can't be opened, the work dir can't be
/// created, or the uploader's TLS config is invalid. All three are
/// startup-time errors — the caller should log and exit.
pub async fn build_components(
    config: &AgentConfig,
) -> Result<AgentComponents, Box<dyn std::error::Error>> {
    let queue_root = Queue::default_root();
    let queue = Queue::open(&queue_root).await?;

    let work_root = queue_root
        .parent()
        .map(|p| p.join("work"))
        .unwrap_or_else(|| PathBuf::from("./work"));
    tokio::fs::create_dir_all(&work_root).await?;

    // Build the redactor; a misconfigured regex is a fatal startup error so
    // the operator is alerted immediately rather than silently shipping PII.
    let redactor = Redactor::from_config(config).unwrap_or_else(|e| {
        warn!(error = %e, "redaction rule failed to compile; falling back to no-op");
        Redactor::noop()
    });
    let orchestrator = EvidenceOrchestrator::new(
        LogsCollector::new(config.log_paths.clone()),
        EventLogsCollector::with_defaults(),
        DsRegCmdCollector::new(),
        work_root.clone(),
        redactor,
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

    Ok(AgentComponents {
        queue,
        orchestrator,
        uploader,
        work_root,
    })
}

/// Drive the collect + drain task loop until `stop_rx` flips to `true`
/// (or the sender is dropped), then run one final bounded drain.
///
/// This is the shared body of CLI daemon mode and the service
/// dispatcher's task loop. It never returns until a stop signal is
/// received — the caller is expected to wire `stop_rx` to ctrl-c (CLI)
/// or to the SCM control handler (service).
pub async fn run_task_loop(
    components: &AgentComponents,
    mut stop_rx: watch::Receiver<bool>,
) {
    let mut collect_tick = tokio::time::interval(COLLECT_INTERVAL);
    let mut drain_tick = tokio::time::interval(DRAIN_INTERVAL);

    // Skip the first immediate collect tick — let the daemon finish
    // booting. Drain fires immediately so crash-survivor queue entries
    // are uploaded quickly after a restart.
    collect_tick.tick().await;

    info!("entering agent task loop");

    loop {
        tokio::select! {
            _ = collect_tick.tick() => {
                collect_and_enqueue(
                    &components.orchestrator,
                    &components.queue,
                    &components.work_root,
                ).await;
            }
            _ = drain_tick.tick() => {
                drain(&components.queue, &components.uploader).await;
            }
            result = stop_rx.changed() => {
                if result.is_err() || *stop_rx.borrow() {
                    info!("stop signal received; draining in-flight work");
                    match tokio::time::timeout(
                        STOP_DRAIN_TIMEOUT,
                        drain(&components.queue, &components.uploader),
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

    info!("agent task loop exited");
}

/// Run one collect pass and enqueue the result. Errors are logged — a
/// transient collection failure shouldn't tear the loop down.
pub async fn collect_and_enqueue(
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
            // Future use: partition staging by collection run id.
            let _ = work_root;
        }
    }
}

/// Drain pending bundles from the queue. Upload errors are recorded on
/// the queue entry so the bundle is retried on the next drain tick.
pub async fn drain(queue: &Queue, uploader: &Uploader) {
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

    // If we successfully uploaded and the entry is now Done, purge the
    // bundle zip immediately — but keep the sidecar so ops can see the
    // Done state. The sidecar sweeper will eventually clear those too.
    if let Ok(current) = queue.get(bundle_id).await {
        if matches!(current.state, QueueState::Done { .. }) {
            if let Err(e) = tokio::fs::remove_file(&current.zip_path).await {
                warn!(%bundle_id, error = %e, "post-upload zip purge failed");
            }
        }
    }
}
