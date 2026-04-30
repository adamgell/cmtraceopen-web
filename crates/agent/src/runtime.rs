//! Shared agent runtime: the collect + drain task loop.
//!
//! Both CLI (`main.rs`) and service (`service.rs`) modes need the same
//! long-lived work: periodic queue drain, ctrl-c/stop handling, and a
//! final bounded drain on shutdown. That code used to be duplicated
//! between the two entry points; it now lives here so a future change
//! to (say) the upload-retry contract can't silently diverge between
//! CLI and service modes.
//!
//! **Agent 0.2.0 is pull-only.** The periodic `collect_tick` is removed.
//! Collection is now triggered exclusively by the server via a
//! `ServerFrame::RequestBundle` WS message handled by
//! [`crate::ws::request_handler`]. The `--oneshot` flag and the
//! [`collect_and_enqueue`] / [`drain`] helpers are kept so existing
//! operator tooling continues to work.
//!
//! ## Entry points
//!
//! * [`build_components`] — one-shot constructor for `Queue`,
//!   `EvidenceOrchestrator`, `Uploader`, and the `work_root` path. Used
//!   by both oneshot and daemon flows.
//! * [`run_task_loop`] — drives the drain loop (no collect tick) until a
//!   stop signal arrives, then runs one final bounded drain.
//! * [`collect_and_enqueue`] / [`drain`] — the actual work fns. `pub`
//!   because the oneshot path calls them directly from `main.rs`.
//! * [`RuntimeSnapshot`] — concrete [`crate::ws::heartbeat::Snapshot`]
//!   implementation used by the heartbeat sender.
//! * [`ScheduledBundleRunner`] — concrete
//!   [`crate::ws::request_handler::BundleRunner`] implementation wired
//!   to the existing collect + upload pipeline.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::collectors::agent_logs::AgentLogsCollector;
use crate::collectors::dsregcmd::DsRegCmdCollector;
use crate::collectors::event_logs::EventLogsCollector;
use crate::collectors::evidence::EvidenceOrchestrator;
use crate::collectors::logs::LogsCollector;
use crate::config::AgentConfig;
use crate::config_sync::ConfigSync;
use crate::queue::{Queue, QueueState};
use crate::redact::Redactor;
use crate::tls::{self, TlsClientOptions};
use crate::uploader::{Uploader, UploaderConfig};

/// How often to run an evidence collection pass.
pub const COLLECT_INTERVAL: Duration = Duration::from_secs(60 * 15);

/// How often to drain the upload queue.
pub const DRAIN_INTERVAL: Duration = Duration::from_secs(30);

/// Queue-level backoff when an upload fails. 10 min gives the server
/// time to recover from rate-limit windows or transient outages.
pub const QUEUE_FAIL_BACKOFF: Duration = Duration::from_secs(600);

/// Maximum time the shutdown path waits for an in-flight drain to complete.
pub const STOP_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Bundle of long-lived components needed by the task loop.
///
/// Constructed once per agent process. Both CLI daemon mode and the
/// Windows service dispatcher go through this builder so the set of
/// dependencies can't drift between the two entry points.
pub struct AgentComponents {
    /// Wrapped in `Arc` so the heartbeat-driven on-demand runner can
    /// share the same instance with the drain/config-sync task loop.
    pub queue: Arc<Queue>,
    pub orchestrator: Arc<EvidenceOrchestrator>,
    pub uploader: Arc<Uploader>,
    pub work_root: PathBuf,
    /// Server-pushed config overrides. Held behind a `Mutex` because
    /// `ConfigSync::sync` / `record_*` take `&mut self` and the task loop
    /// touches it from multiple `select!` branches.
    pub config_sync: Mutex<ConfigSync>,
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

    // A misconfigured regex is a fatal startup error: silently falling back to
    // a no-op redactor would leave PII unredacted without any visible signal.
    let redactor = Redactor::from_config(config)?;

    let tls_opts = TlsClientOptions {
        client_cert_pem: config.tls_client_cert_pem.clone(),
        client_key_pem: config.tls_client_key_pem.clone(),
        ca_bundle_pem: config.tls_ca_bundle_pem.clone(),
    };

    // Build the reqwest client once so TLS settings are applied uniformly
    // to both the config-sync fetches and the bundle uploads.
    let http_client = tls::build_reqwest_client(tls_opts.clone())?;

    // ConfigSync: fetch remote overrides at startup and periodically. Apply
    // any server-pushed config to the effective AgentConfig before constructing
    // the orchestrator + uploader so they pick up the override on first run.
    let mut config_sync = ConfigSync::new(
        http_client,
        config.api_endpoint.clone(),
        config.resolved_device_id(),
        config.clone(),
    );
    let effective = config_sync.sync().await;


    let orchestrator = EvidenceOrchestrator::new(
        LogsCollector::new(effective.log_paths.clone()),
        EventLogsCollector::with_defaults(),
        DsRegCmdCollector::new(),
        // Agent self-logs: ship the agent's own daily-rolling tracing output
        // inside the evidence bundle so operators can diagnose the agent
        // itself via the web viewer. No config knob in v1 — always on; a
        // follow-up commit can add an opt-out or wire a dedicated
        // tracing-JSON parser server-side (today these fall through to the
        // generic `plain_text` / `timestamped` parser).
        AgentLogsCollector::with_defaults(),
        work_root.clone(),
        redactor,
    );
    let uploader = Uploader::new(
        UploaderConfig::new(
            effective.api_endpoint.clone(),
            effective.resolved_device_id(),
            Duration::from_secs(effective.request_timeout_secs),
        )
        .with_tls(tls_opts),
    )?;

    Ok(AgentComponents {
        queue: Arc::new(queue),
        orchestrator: Arc::new(orchestrator),
        uploader: Arc::new(uploader),
        work_root,
        config_sync: Mutex::new(config_sync),
    })
}

/// Drive the drain + config-sync task loop until `stop_rx` flips to
/// `true` (or the sender is dropped), then run one final bounded drain.
///
/// **Agent 0.2.0:** the periodic `collect_tick` has been removed.
/// Collection is now triggered exclusively by the server via
/// `ServerFrame::RequestBundle`. The drain tick and config-sync tick
/// continue as before.
///
/// This is the shared body of CLI daemon mode and the service
/// dispatcher's task loop. It never returns until a stop signal is
/// received — the caller is expected to wire `stop_rx` to ctrl-c (CLI)
/// or to the SCM control handler (service).
pub async fn run_task_loop(
    components: &AgentComponents,
    mut stop_rx: watch::Receiver<bool>,
) {
    let mut drain_tick = tokio::time::interval(DRAIN_INTERVAL);
    // Per-device-jittered fetch interval keeps the fleet from stampeding
    // /v1/config/{id} simultaneously. Read once here so the interval is
    // stable for the life of the loop; future config rotations on the
    // device id would require a daemon restart anyway.
    let config_fetch_interval = components.config_sync.lock().await.fetch_interval();
    let mut config_tick = tokio::time::interval(config_fetch_interval);

    // Drain fires immediately so crash-survivor queue entries are
    // uploaded quickly after a restart. ConfigSync's initial fetch
    // already happened in `build_components`; skip the first immediate
    // config tick to avoid back-to-back fetches.
    config_tick.tick().await;

    info!("entering agent task loop (pull-only, no collect tick)");

    loop {
        tokio::select! {
            _ = drain_tick.tick() => {
                drain(&components.queue, &components.uploader).await;
                // Treat "drain happened without surfacing a panic" as a
                // success heartbeat for ConfigSync's rollback timer.
                // ConfigSync's own internal failure-tracking handles the
                // detailed bookkeeping on a per-fetch basis.
                components.config_sync.lock().await.record_success();
            }
            _ = config_tick.tick() => {
                let mut cs = components.config_sync.lock().await;
                let _ = cs.sync().await;
                if cs.should_rollback() {
                    cs.rollback();
                }
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

// ---------------------------------------------------------------------------
// WS subsystem concrete implementations
// ---------------------------------------------------------------------------

/// Concrete [`crate::ws::heartbeat::Snapshot`] that reads real agent state.
///
/// Holds an `Arc<AgentConfig>` so it can cheaply cross the `tokio::spawn`
/// boundary without cloning the full config. More dynamic fields (queue
/// depth, last_collect_at, uptime) are left at their zero values for the
/// 0.2.0 release; a follow-up PR can wire them once the agent accumulates
/// runtime state in a shared struct.
pub struct RuntimeSnapshot {
    config: Arc<crate::config::AgentConfig>,
}

impl RuntimeSnapshot {
    pub fn new(config: Arc<crate::config::AgentConfig>) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl crate::ws::heartbeat::Snapshot for RuntimeSnapshot {
    async fn snapshot(&self) -> crate::ws::heartbeat::SnapshotData {
        crate::ws::heartbeat::SnapshotData {
            device_id: self.config.resolved_device_id(),
            device_name: hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_default(),
            intune_device_id: None,
            ninjaone_device_id: None,
            asset_tag: None,
            agent_version: env!("CARGO_PKG_VERSION").into(),
            os_version: os_info::get().version().to_string(),
            last_collect_at: None,
            queue_depth: 0,
            errors_24h: 0,
            disk_free_pct: 100,
            uptime_seconds: 0,
        }
    }
}

/// Concrete [`crate::ws::request_handler::BundleRunner`] that drives the
/// existing collection + upload pipeline on demand.
///
/// When the server sends a `RequestBundle` frame, this runner:
///   1. Runs the evidence orchestrator to produce a new bundle.
///   2. Enqueues it and uploads immediately, passing `request_id` as
///      `X-Bundle-Request-Id` on the init POST so the server-side T12
///      correlation can match the bundle to the operator request.
///
/// The bundle_id returned is the uuid from the evidence orchestrator;
/// the uploader's `session_id` is a separate server-assigned identifier.
///
/// Fields are held by `Arc` so the struct is cheaply `Clone` + `Send`.
pub struct ScheduledBundleRunner {
    config: Arc<crate::config::AgentConfig>,
    orchestrator: Arc<crate::collectors::evidence::EvidenceOrchestrator>,
    queue: Arc<crate::queue::Queue>,
    uploader: Arc<crate::uploader::Uploader>,
}

impl ScheduledBundleRunner {
    /// Construct from a config `Arc` and the *already-built* components.
    /// Takes ownership of the three component fields it needs; call this
    /// before moving `components` into the task loop.
    pub fn new(
        config: Arc<crate::config::AgentConfig>,
        orchestrator: Arc<crate::collectors::evidence::EvidenceOrchestrator>,
        queue: Arc<crate::queue::Queue>,
        uploader: Arc<crate::uploader::Uploader>,
    ) -> Self {
        Self {
            config,
            orchestrator,
            queue,
            uploader,
        }
    }
}

#[async_trait::async_trait]
impl crate::ws::request_handler::BundleRunner for ScheduledBundleRunner {
    async fn run(&self, request_id: Uuid) -> anyhow::Result<Uuid> {
        use crate::queue::QueueState;

        // Step 1: collect evidence.
        let bundle = self
            .orchestrator
            .collect_once()
            .await
            .map_err(|e| anyhow::anyhow!("collection failed: {e}"))?;

        let bundle_id = bundle.metadata.bundle_id;
        let metadata = bundle.metadata.clone();
        let staging_zip = bundle.zip_path.clone();
        let staging_dir = bundle.staging_dir.clone();

        // Step 2: enqueue. The queue renames the zip from staging into its
        // own root; the returned `QueuedBundle.zip_path` is the queue-owned
        // path we must upload from.
        let queued = self
            .queue
            .enqueue(metadata.clone(), &staging_zip)
            .await
            .map_err(|e| anyhow::anyhow!("enqueue failed: {e}"))?;

        // Clean up staging dir now that zip is in the queue (best-effort).
        if let Err(e) = tokio::fs::remove_dir_all(&staging_dir).await {
            warn!(dir = %staging_dir.display(), error = %e, "failed to clean staging dir after on-demand collect");
        }

        info!(%bundle_id, %request_id, "on-demand bundle enqueued");

        // Step 3: upload immediately with the request_id for correlation.
        // Using `upload_with_request_id` so the `X-Bundle-Request-Id` header
        // is set on the init POST, enabling server-side T12 correlation.
        if let Err(e) = self.queue.mark_uploading(bundle_id).await {
            warn!(%bundle_id, error = %e, "mark_uploading failed");
        }

        let resp = self
            .uploader
            .upload_with_request_id(&metadata, &queued.zip_path, Some(request_id))
            .await
            .map_err(|e| anyhow::anyhow!("upload failed: {e}"))?;

        info!(
            %bundle_id,
            session_id = %resp.session_id,
            parse_state = %resp.parse_state,
            %request_id,
            "on-demand upload succeeded"
        );

        if let Err(e) = self.queue.mark_done(bundle_id).await {
            warn!(%bundle_id, error = %e, "mark_done failed after on-demand upload");
        }

        // Purge the zip now that it's committed — keep the sidecar for
        // queue inspection. Mirror the normal drain path.
        if let Ok(current) = self.queue.get(bundle_id).await {
            if matches!(current.state, QueueState::Done { .. }) {
                let _ = tokio::fs::remove_file(&current.zip_path).await;
            }
        }

        Ok(bundle_id)
    }

    fn device_id(&self) -> String {
        self.config.resolved_device_id()
    }

    fn device_name(&self) -> String {
        hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_default()
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
