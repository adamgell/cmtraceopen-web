//! Scheduler integration tests.
//!
//! These tests exercise the `CollectionScheduler` end-to-end against real
//! (but minimal) infrastructure: a temporary log file, a real
//! `EvidenceOrchestrator`, and a temporary on-disk queue.
//!
//! Time-sensitive tests use `#[tokio::test(start_paused = true)]` +
//! `tokio::time::advance` to advance the virtual clock without waiting for
//! wall-clock seconds to pass.

use std::time::Duration;

use cmtraceopen_agent::collectors::dsregcmd::DsRegCmdCollector;
use cmtraceopen_agent::collectors::event_logs::EventLogsCollector;
use cmtraceopen_agent::collectors::evidence::EvidenceOrchestrator;
use cmtraceopen_agent::collectors::logs::LogsCollector;
use cmtraceopen_agent::config::{ScheduleConfig, ScheduleMode};
use cmtraceopen_agent::queue::{Queue, QueuedBundle};
use cmtraceopen_agent::scheduler::{apply_jitter, CollectionScheduler};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::Instant;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal orchestrator that collects one small log file from
/// `source_dir`. Works on any platform (Linux CI or Windows).
fn make_orchestrator(source_dir: &TempDir, work_dir: &TempDir) -> EvidenceOrchestrator {
    let pattern = format!(
        "{}/*.log",
        source_dir.path().to_string_lossy().replace('\\', "/")
    );
    EvidenceOrchestrator::new(
        LogsCollector::new(vec![pattern]),
        EventLogsCollector::with_defaults(), // NotSupported on Linux — fine
        DsRegCmdCollector::new(),            // NotSupported on Linux — fine
        work_dir.path().to_path_buf(),
    )
}

/// Make a minimal `ScheduleConfig` for interval mode with no jitter.
fn interval_config(interval_hours: u64) -> ScheduleConfig {
    ScheduleConfig {
        mode: ScheduleMode::Interval,
        interval_hours,
        cron_expr: "0 3 * * *".into(),
        jitter_minutes: 0,
    }
}

/// Poll `queue.next_pending()` up to `max_rounds` times (yielding between each)
/// until a bundle appears. Returns the bundle if found, `None` otherwise.
///
/// This is needed in paused-time tests: after `tokio::time::advance()` wakes
/// the scheduler's sleep, the actual collection involves multiple async ops
/// (create_dir, join!, write, spawn_blocking, enqueue). Each `yield_now()`
/// advances the event loop one step.
async fn poll_until_bundle(
    queue: &Queue,
    max_rounds: usize,
) -> Option<QueuedBundle> {
    for _ in 0..max_rounds {
        tokio::task::yield_now().await;
        if let Ok(Some(b)) = queue.next_pending().await {
            return Some(b);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Test 1: interval mode fires after the configured interval
// ---------------------------------------------------------------------------

/// After the configured interval elapses (advanced via virtual time), the
/// scheduler must enqueue exactly one bundle.
#[tokio::test(start_paused = true)]
async fn interval_mode_fires_after_configured_interval() {
    let source_dir = TempDir::new().unwrap();
    std::fs::write(source_dir.path().join("ccmexec.log"), b"<![LOG[smoke]LOG]!>\r\n").unwrap();
    let work_dir = TempDir::new().unwrap();
    let queue_dir = TempDir::new().unwrap();

    let orch = make_orchestrator(&source_dir, &work_dir);
    let queue = Queue::open(queue_dir.path()).await.unwrap();

    // Separate queue handle for assertion; same path → same on-disk state.
    let check_queue = Queue::open(queue_dir.path()).await.unwrap();

    let config = interval_config(1); // fire every 1 virtual hour
    let (stop_tx, stop_rx) = mpsc::channel::<()>(1);
    let scheduler =
        CollectionScheduler::new(config, orch, queue);
    let handle = tokio::spawn(scheduler.run(stop_rx));

    // Yield so the scheduler registers its sleep before we advance.
    tokio::task::yield_now().await;

    // Before the interval elapses — no collection yet.
    assert!(
        check_queue.next_pending().await.unwrap().is_none(),
        "queue must be empty before interval elapses"
    );

    // Advance virtual clock past the 1-hour interval.
    tokio::time::advance(Duration::from_secs(3601)).await;

    // Poll until the collection is enqueued. The scheduler's collection
    // involves multiple async operations (create_dir, join!, write, spawn_blocking,
    // enqueue); each round of the event loop advances it one step. We give
    // it up to 200 rounds, which is far more than enough.
    let entry = poll_until_bundle(&check_queue, 200).await;

    // Stop the scheduler cleanly.
    let _ = stop_tx.send(()).await;
    handle.await.expect("scheduler task panicked");

    assert!(entry.is_some(), "expected one queued bundle after interval elapsed");
    assert!(
        entry.unwrap().metadata.size_bytes > 0,
        "bundle should not be empty"
    );
}

// ---------------------------------------------------------------------------
// Test 2: manual mode never fires
// ---------------------------------------------------------------------------

/// In manual mode the scheduler never triggers a collection regardless of
/// how much virtual time passes.
#[tokio::test(start_paused = true)]
async fn manual_mode_never_fires() {
    let source_dir = TempDir::new().unwrap();
    std::fs::write(source_dir.path().join("ccmexec.log"), b"manual mode test\r\n").unwrap();
    let work_dir = TempDir::new().unwrap();
    let queue_dir = TempDir::new().unwrap();

    let orch = make_orchestrator(&source_dir, &work_dir);
    let queue = Queue::open(queue_dir.path()).await.unwrap();
    let check_queue = Queue::open(queue_dir.path()).await.unwrap();

    let config = ScheduleConfig {
        mode: ScheduleMode::Manual,
        ..ScheduleConfig::default()
    };
    let (stop_tx, stop_rx) = mpsc::channel::<()>(1);
    let scheduler =
        CollectionScheduler::new(config, orch, queue);
    let handle = tokio::spawn(scheduler.run(stop_rx));

    // Advance a long way — manual mode should never fire.
    tokio::time::advance(Duration::from_secs(365 * 24 * 3600)).await;

    // Queue remains empty.
    assert!(
        check_queue.next_pending().await.unwrap().is_none(),
        "manual mode must not enqueue bundles automatically"
    );

    // Graceful shutdown.
    let _ = stop_tx.send(()).await;
    handle.await.expect("scheduler task panicked");
}

// ---------------------------------------------------------------------------
// Test 3: stop signal cancels mid-sleep cleanly
// ---------------------------------------------------------------------------

/// Sending a stop signal while the scheduler is sleeping (no collection in
/// progress) must cause the run() future to resolve promptly.
#[tokio::test(start_paused = true)]
async fn stop_signal_cancels_mid_sleep() {
    let source_dir = TempDir::new().unwrap();
    let work_dir = TempDir::new().unwrap();
    let queue_dir = TempDir::new().unwrap();

    let orch = make_orchestrator(&source_dir, &work_dir);
    let queue = Queue::open(queue_dir.path()).await.unwrap();

    // Very long interval so we know no collection fires in virtual time.
    let config = interval_config(1000);
    let (stop_tx, stop_rx) = mpsc::channel::<()>(1);
    let scheduler =
        CollectionScheduler::new(config, orch, queue);
    let handle = tokio::spawn(scheduler.run(stop_rx));

    // Let the scheduler register its sleep.
    tokio::task::yield_now().await;

    // Send stop — do NOT advance time; the scheduler is mid-sleep.
    let _ = stop_tx.send(()).await;

    // The task must exit: the `stop.recv()` branch of `select!` fires when
    // the stop message is delivered while the sleep is still in progress.
    tokio::time::advance(Duration::from_millis(1)).await;
    handle.await.expect("scheduler task panicked");
}

// ---------------------------------------------------------------------------
// Test 4: cron mode fires on the next matching time
// ---------------------------------------------------------------------------

/// With "* * * * *" (every minute), advancing virtual time by two minutes
/// guarantees at least one collection fires.
#[tokio::test(start_paused = true)]
async fn cron_mode_fires_on_next_matching_minute() {
    let source_dir = TempDir::new().unwrap();
    std::fs::write(source_dir.path().join("ccmexec.log"), b"cron test\r\n").unwrap();
    let work_dir = TempDir::new().unwrap();
    let queue_dir = TempDir::new().unwrap();

    let orch = make_orchestrator(&source_dir, &work_dir);
    let queue = Queue::open(queue_dir.path()).await.unwrap();
    let check_queue = Queue::open(queue_dir.path()).await.unwrap();

    // "* * * * *" fires at the top of every minute.
    // The next occurrence is 0–60 s from now (real clock); after 2 virtual
    // minutes we are guaranteed at least one fire.
    let config = ScheduleConfig {
        mode: ScheduleMode::Cron,
        cron_expr: "* * * * *".into(),
        jitter_minutes: 0,
        ..ScheduleConfig::default()
    };
    let (stop_tx, stop_rx) = mpsc::channel::<()>(1);
    let scheduler =
        CollectionScheduler::new(config, orch, queue);
    let handle = tokio::spawn(scheduler.run(stop_rx));

    // Yield so the scheduler computes and registers its first sleep.
    tokio::task::yield_now().await;

    // Advance past two whole minutes (guarantees at least one cron trigger).
    tokio::time::advance(Duration::from_secs(121)).await;

    // Poll until the collection is enqueued (same as interval test).
    let entry = poll_until_bundle(&check_queue, 200).await;

    // Stop and wait for any in-progress collection to finish.
    let _ = stop_tx.send(()).await;
    handle.await.expect("scheduler task panicked");

    assert!(
        entry.is_some(),
        "cron mode should have enqueued at least one bundle after 2 minutes"
    );
}

// ---------------------------------------------------------------------------
// Test 5: jitter spread across 1000 samples
// ---------------------------------------------------------------------------

/// Run `apply_jitter` 1000 times and verify the resulting instants are spread
/// across the full ±jitter_minutes window. This demonstrates that the
/// randomization is effective: a fleet of 1000 devices won't all fire at
/// the same wall-clock second.
#[test]
fn jitter_spread_across_1000_samples() {
    let jitter_minutes = 30u64;
    // Pick a base far enough in the future that the negative-offset clamp
    // never fires (base is 2 h from now, max negative jitter is 30 min).
    let base = Instant::now() + Duration::from_secs(7200);

    let samples: Vec<Instant> = (0..1000)
        .map(|_| apply_jitter(base, jitter_minutes))
        .collect();

    let min_s = *samples.iter().min().unwrap();
    let max_s = *samples.iter().max().unwrap();
    let spread = max_s.duration_since(min_s);

    // With 1000 uniform samples over a 3600-second window the observed spread
    // must be at least 1800 s (50% of the range). Failing by chance is
    // astronomically unlikely.
    assert!(
        spread >= Duration::from_secs(1800),
        "jitter spread was only {spread:?}; want ≥ 1800 s across 1000 samples"
    );

    // Also verify every sample lies within the allowed [base - N, base + N] range.
    let max_delta = Duration::from_secs(jitter_minutes * 60);
    let min_allowed = base.checked_sub(max_delta).unwrap_or(base);
    let max_allowed = base + max_delta;
    for s in &samples {
        assert!(
            *s >= min_allowed && *s <= max_allowed,
            "sample {s:?} lies outside allowed jitter range [{min_allowed:?}, {max_allowed:?}]"
        );
    }
}
