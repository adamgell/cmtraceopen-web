//! The load loop. Two control mechanisms:
//!
//! 1. **Concurrency semaphore** (`conc`) — a standard bounded semaphore of
//!    size `concurrency`. Permits are held for the lifetime of a task so that
//!    at most `concurrency` tasks are in-flight at once.
//!
//! 2. **Ramp gate** (`ramp_gate`) — a one-time-use gate that starts empty and
//!    has permits added one-by-one over the ramp window. Each permit is
//!    *forgotten* (not returned) after acquisition, so the gate enforces the
//!    dispatch rate during warmup. Once all `concurrency` ramp tokens are
//!    exhausted the gate is permanently open (the concurrency semaphore alone
//!    governs throughput).

use crate::bundle::BundleResult;
use crate::config::StopCondition;
use crate::corpus::Corpus;
use crate::target::Target;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc::Sender, Semaphore};
use tokio::task::JoinSet;

pub struct DriverInputs {
    pub target: Arc<dyn Target>,
    pub corpus: Box<dyn Corpus>,
    pub concurrency: u32,
    pub ramp: Duration,
    pub stop: StopCondition,
    pub device_pool_size: u32,
    pub run_uuid: String,
    pub reporter_tx: Sender<BundleResult>,
}

pub async fn run_driver(mut inputs: DriverInputs) -> anyhow::Result<()> {
    let concurrency = inputs.concurrency.max(1) as usize;

    // Concurrency semaphore: bounds how many tasks are in-flight simultaneously.
    let conc = Arc::new(Semaphore::new(concurrency));

    // Ramp gate: one-time-use permits added gradually over the ramp window.
    // After the ramp, this semaphore has an effectively infinite permit count
    // so it no longer blocks dispatch.
    let ramp_gate = Arc::new(Semaphore::new(0));
    spawn_ramp(ramp_gate.clone(), inputs.concurrency, inputs.ramp);

    let mut tasks: JoinSet<()> = JoinSet::new();
    let mut emitted: u64 = 0;
    let deadline = match inputs.stop {
        StopCondition::SoakDeadline(d) => Some(Instant::now() + d),
        StopCondition::TotalBundles(_) => None,
    };
    let total_bundles = match inputs.stop {
        StopCondition::TotalBundles(n) => Some(n),
        StopCondition::SoakDeadline(_) => None,
    };
    let device_pool = inputs.device_pool_size.max(1) as u64;

    loop {
        if let Some(n) = total_bundles {
            if emitted >= n { break; }
        }
        if let Some(d) = deadline {
            if Instant::now() >= d { break; }
        }

        // Helper: acquire from a semaphore with optional deadline.
        macro_rules! acquire_with_deadline {
            ($sem:expr) => {{
                if let Some(d) = deadline {
                    let remaining = d.saturating_duration_since(Instant::now());
                    match tokio::time::timeout(remaining, $sem.clone().acquire_owned()).await {
                        Ok(p) => p?,
                        Err(_) => break,
                    }
                } else {
                    $sem.clone().acquire_owned().await?
                }
            }};
        }

        // Wait for a ramp token (rate gate). Forget it immediately — it is
        // one-time use and must not be recycled.
        let ramp_permit = acquire_with_deadline!(ramp_gate);
        ramp_permit.forget();

        // Wait for a concurrency slot.
        let conc_permit = acquire_with_deadline!(conc);

        let mut bundle = inputs.corpus.next().await?;
        bundle.seq = emitted;
        bundle.device_id = format!(
            "LOAD-TEST-{}-{:04}",
            &inputs.run_uuid[..inputs.run_uuid.len().min(8)],
            emitted % device_pool
        );
        emitted += 1;

        let target = inputs.target.clone();
        let tx = inputs.reporter_tx.clone();
        tasks.spawn(async move {
            let _p = conc_permit; // released when task finishes, freeing a slot
            let result = target.send(bundle).await;
            let _ = tx.send(result).await;
        });
    }

    while tasks.join_next().await.is_some() {}
    Ok(())
}

fn spawn_ramp(gate: Arc<Semaphore>, concurrency: u32, ramp: Duration) {
    if ramp.is_zero() {
        // Open the gate immediately with enough permits for any run.
        gate.add_permits(Semaphore::MAX_PERMITS);
        return;
    }
    // Intentionally detached: this task runs for the lifetime of the run; the driver loop only reads permits.
    tokio::spawn(async move {
        let step = ramp / concurrency.max(1);
        loop {
            // Keep adding permits indefinitely so that after the ramp the gate
            // is never the bottleneck (the concurrency semaphore takes over).
            gate.add_permits(1);
            tokio::time::sleep(step).await;
        }
    });
}
