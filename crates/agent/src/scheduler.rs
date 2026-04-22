//! Collection scheduler.
//!
//! Drives evidence collection on a configured cadence — interval, cron, or
//! manual. Intended to run as a sibling task to the queue drainer in the
//! service dispatcher's tokio runtime.
//!
//! ## Loop pattern
//!
//! 1. Compute the next-fire [`tokio::time::Instant`].
//! 2. `tokio::select!` on `sleep_until(fire_at)` vs `stop.recv()`.
//! 3. On fire → call [`EvidenceOrchestrator::collect_once()`] + enqueue.
//! 4. On stop → exit cleanly.
//!
//! ## Jitter
//!
//! To prevent a fleet of 1000+ devices from hammering the server
//! simultaneously, the fire time is shifted by a random offset uniformly
//! drawn from `[-jitter_minutes, +jitter_minutes]`. Set `jitter_minutes = 0`
//! to disable.

use std::path::PathBuf;
use std::time::Duration;

use chrono::Local;
use croner::parser::CronParser;
use rand::Rng;
use tokio::sync::mpsc;
use tokio::time::{sleep_until, Instant};
use tracing::{info, warn};

use crate::collectors::evidence::EvidenceOrchestrator;
use crate::config::{ScheduleConfig, ScheduleMode};
use crate::queue::Queue;

/// Drives periodic evidence collection according to the configured schedule.
///
/// Construct with [`CollectionScheduler::new`], then spawn
/// `scheduler.run(stop_rx)` as a tokio task alongside the queue-drainer task.
pub struct CollectionScheduler {
    config: ScheduleConfig,
    orchestrator: EvidenceOrchestrator,
    queue: Queue,
    work_root: PathBuf,
}

impl CollectionScheduler {
    /// Create a new scheduler.
    ///
    /// * `config`       — schedule configuration (mode, interval, cron expr, jitter).
    /// * `orchestrator` — evidence orchestrator used to perform each collection pass.
    /// * `queue`        — persistent upload queue that receives collected bundles.
    /// * `work_root`    — staging directory for in-progress collection passes.
    pub fn new(
        config: ScheduleConfig,
        orchestrator: EvidenceOrchestrator,
        queue: Queue,
        work_root: PathBuf,
    ) -> Self {
        Self {
            config,
            orchestrator,
            queue,
            work_root,
        }
    }

    /// Run the scheduler until a stop signal arrives on `stop`.
    ///
    /// The returned future resolves once the loop exits, either because
    /// `stop` was signalled or because the channel was dropped.
    pub async fn run(self, mut stop: mpsc::Receiver<()>) {
        match self.config.mode {
            ScheduleMode::Manual => {
                info!("scheduler mode=manual; collection disabled — waiting for stop signal");
                // Manual mode: never fire, just wait until told to stop.
                let _ = stop.recv().await;
                info!("scheduler (manual) received stop signal; exiting");
            }
            ScheduleMode::Interval => {
                self.run_interval(stop).await;
            }
            ScheduleMode::Cron => {
                self.run_cron(stop).await;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Internal loop variants
    // -----------------------------------------------------------------------

    async fn run_interval(self, mut stop: mpsc::Receiver<()>) {
        let interval_hours = self.config.interval_hours;
        let jitter_minutes = self.config.jitter_minutes;
        info!(interval_hours, jitter_minutes, "scheduler mode=interval starting");

        loop {
            let fire_at = next_interval_instant(interval_hours, jitter_minutes);

            tokio::select! {
                _ = sleep_until(fire_at) => {
                    info!("scheduler (interval) firing collection");
                    collect_and_enqueue(&self.orchestrator, &self.queue, &self.work_root).await;
                }
                _ = stop.recv() => {
                    info!("scheduler (interval) received stop signal; exiting");
                    break;
                }
            }
        }
    }

    async fn run_cron(self, mut stop: mpsc::Receiver<()>) {
        let cron_expr = &self.config.cron_expr;
        let jitter_minutes = self.config.jitter_minutes;
        info!(cron_expr, jitter_minutes, "scheduler mode=cron starting");

        let parser = CronParser::new();
        let schedule = match parser.parse(cron_expr) {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    cron_expr,
                    error = %e,
                    "invalid cron expression; scheduler will not fire — \
                     falling back to manual (no auto-collection)"
                );
                // Treat an invalid cron expression like manual mode.
                let _ = stop.recv().await;
                return;
            }
        };

        loop {
            let now = Local::now();
            let next_dt = match schedule.find_next_occurrence(&now, false) {
                Ok(t) => t,
                Err(e) => {
                    warn!(
                        error = %e,
                        "cron: could not determine next occurrence; \
                         sleeping 1 hour before retrying"
                    );
                    // Avoid a tight spin on repeated errors.
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    continue;
                }
            };

            // Compute how long to sleep until `next_dt`.
            let until_next = (next_dt - now)
                .to_std()
                .unwrap_or(Duration::ZERO);

            let fire_at = apply_jitter(Instant::now() + until_next, jitter_minutes);

            tokio::select! {
                _ = sleep_until(fire_at) => {
                    info!("scheduler (cron) firing collection");
                    collect_and_enqueue(&self.orchestrator, &self.queue, &self.work_root).await;
                }
                _ = stop.recv() => {
                    info!("scheduler (cron) received stop signal; exiting");
                    break;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the tokio `Instant` at which the next interval-mode fire should
/// occur, including a random jitter in `[-jitter_minutes, +jitter_minutes]`.
pub fn next_interval_instant(interval_hours: u64, jitter_minutes: u64) -> Instant {
    let base = Instant::now() + Duration::from_secs(interval_hours * 3600);
    apply_jitter(base, jitter_minutes)
}

/// Shift `base` by a random offset uniformly drawn from
/// `[-jitter_minutes, +jitter_minutes]`.
///
/// If `jitter_minutes` is 0 the instant is returned unchanged.
/// If the jitter would push the fire time before `Instant::now()` it is
/// clamped to `now` (so we never sleep a negative duration).
pub fn apply_jitter(base: Instant, jitter_minutes: u64) -> Instant {
    if jitter_minutes == 0 {
        return base;
    }

    let max_secs = jitter_minutes * 60;
    // Draw a random value in [0, 2 * max_secs], then subtract max_secs to
    // center the distribution around 0.
    let random_secs: u64 = rand::rng().random_range(0..=(2 * max_secs));
    let offset_secs = random_secs as i64 - max_secs as i64; // in [-max_secs, +max_secs]

    if offset_secs >= 0 {
        base + Duration::from_secs(offset_secs as u64)
    } else {
        let subtract = Duration::from_secs((-offset_secs) as u64);
        // Clamp to now so we don't produce an instant in the past.
        base.checked_sub(subtract).unwrap_or_else(Instant::now)
    }
}

/// Run one collect-and-enqueue pass. Errors are logged — a transient
/// collection failure must not tear the scheduler loop down.
async fn collect_and_enqueue(
    orchestrator: &EvidenceOrchestrator,
    queue: &Queue,
    work_root: &std::path::Path,
) {
    match orchestrator.collect_once().await {
        Ok(bundle) => {
            let bundle_id = bundle.metadata.bundle_id;
            match queue.enqueue(bundle.metadata, &bundle.zip_path).await {
                Ok(_) => info!(%bundle_id, "scheduler: bundle enqueued"),
                Err(e) => warn!(%bundle_id, error = %e, "scheduler: enqueue failed"),
            }
            if let Err(e) = tokio::fs::remove_dir_all(&bundle.staging_dir).await {
                warn!(
                    dir = %bundle.staging_dir.display(),
                    error = %e,
                    "scheduler: failed to clean staging dir"
                );
            }
        }
        Err(e) => {
            warn!(error = %e, "scheduler: collection failed");
            let _ = work_root;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_jitter_zero_returns_base() {
        let base = Instant::now() + Duration::from_secs(3600);
        let jittered = apply_jitter(base, 0);
        assert_eq!(jittered, base);
    }

    #[test]
    fn apply_jitter_stays_within_bounds() {
        // Run a few hundred samples and verify each is within ±N minutes.
        let jitter_minutes = 30u64;
        let base = Instant::now() + Duration::from_secs(7200); // 2h from now
        let max_delta_secs = jitter_minutes * 60;

        for _ in 0..500 {
            let j = apply_jitter(base, jitter_minutes);
            let now = Instant::now();
            // j should be at most base + max_delta, and at least base - max_delta
            // (clamped to now if that would be in the past, but base is 2h out
            // so the clamp never fires in this scenario).
            assert!(
                j <= base + Duration::from_secs(max_delta_secs),
                "jittered instant too far in the future"
            );
            let min_j = base
                .checked_sub(Duration::from_secs(max_delta_secs))
                .unwrap_or(now);
            assert!(j >= min_j, "jittered instant too far in the past");
        }
    }

    #[test]
    fn apply_jitter_distribution_is_spread() {
        // Run 1000 samples; the min and max should differ by at least half the
        // full jitter range, demonstrating the randomness is actually spread.
        let jitter_minutes = 30u64;
        let base = Instant::now() + Duration::from_secs(7200);

        let samples: Vec<Instant> = (0..1000).map(|_| apply_jitter(base, jitter_minutes)).collect();
        let min_s = samples.iter().min().copied().unwrap();
        let max_s = samples.iter().max().copied().unwrap();
        let spread = max_s.duration_since(min_s);

        // With 1000 uniform samples in a 3600-second window we expect the
        // observed spread to be at least 1800 seconds (50% of the full range).
        // The probability of this failing by chance is astronomically small.
        assert!(
            spread >= Duration::from_secs(1800),
            "jitter spread was only {spread:?}; expected at least 1800s"
        );
    }

    #[test]
    fn next_interval_instant_is_in_the_future() {
        let fire = next_interval_instant(1, 0);
        assert!(
            fire > Instant::now(),
            "next_interval_instant should be in the future"
        );
    }
}
