use parse_load_harness::bundle::{Bundle, BundleResult};
use parse_load_harness::config::StopCondition;
use parse_load_harness::driver::run_driver;
use parse_load_harness::driver::DriverInputs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct CountingTarget {
    sent: AtomicU64,
}

#[async_trait::async_trait]
impl parse_load_harness::target::Target for CountingTarget {
    async fn send(&self, bundle: Bundle) -> BundleResult {
        self.sent.fetch_add(1, Ordering::SeqCst);
        BundleResult {
            seq: bundle.seq,
            device: bundle.device_id,
            n_files: bundle.n_files,
            bundle_bytes: bundle.zip_bytes.len() as u64,
            finalize_ms: Some(0),
            parse_ms: 0,
            parse_state: "ok".into(),
            files_with_fallbacks: None,
            http_status: Some(201),
            error: None,
        }
    }
    async fn sample_system(&self) -> serde_json::Value { serde_json::json!({}) }
    async fn shutdown(self: Box<Self>) -> anyhow::Result<()> { Ok(()) }
}

struct StaticCorpus { n_files: u32 }

#[async_trait::async_trait]
impl parse_load_harness::corpus::Corpus for StaticCorpus {
    async fn next(&mut self) -> anyhow::Result<Bundle> {
        Ok(Bundle {
            seq: 0,
            device_id: String::new(),
            bundle_id: uuid::Uuid::now_v7(),
            n_files: self.n_files,
            zip_bytes: vec![0u8; 64],
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn total_bundles_is_exact() {
    let target = Arc::new(CountingTarget::default());
    let corpus = Box::new(StaticCorpus { n_files: 3 });
    let (tx, mut rx) = tokio::sync::mpsc::channel::<BundleResult>(64);
    let inputs = DriverInputs {
        target: target.clone(),
        corpus,
        concurrency: 8,
        ramp: Duration::ZERO,
        stop: StopCondition::TotalBundles(50),
        device_pool_size: 5,
        run_uuid: "test-run".into(),
        reporter_tx: tx,
    };
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    run_driver(inputs).await.unwrap();
    assert_eq!(target.sent.load(Ordering::SeqCst), 50);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_mode_honors_deadline() {
    let target = Arc::new(CountingTarget::default());
    let corpus = Box::new(StaticCorpus { n_files: 1 });
    let (tx, mut rx) = tokio::sync::mpsc::channel::<BundleResult>(64);
    let inputs = DriverInputs {
        target: target.clone(),
        corpus,
        concurrency: 4,
        ramp: Duration::ZERO,
        stop: StopCondition::SoakDeadline(Duration::from_millis(200)),
        device_pool_size: 4,
        run_uuid: "test-soak".into(),
        reporter_tx: tx,
    };
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let started = Instant::now();
    run_driver(inputs).await.unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_millis(800), "elapsed {:?}", elapsed);
    assert!(target.sent.load(Ordering::SeqCst) > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ramp_delays_full_concurrency() {
    let target = Arc::new(CountingTarget::default());
    let corpus = Box::new(StaticCorpus { n_files: 1 });
    let (tx, mut rx) = tokio::sync::mpsc::channel::<BundleResult>(64);
    let inputs = DriverInputs {
        target: target.clone(),
        corpus,
        concurrency: 100,
        ramp: Duration::from_millis(300),
        stop: StopCondition::TotalBundles(200),
        device_pool_size: 100,
        run_uuid: "test-ramp".into(),
        reporter_tx: tx,
    };
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let started = Instant::now();
    run_driver(inputs).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(150),
        "ramp should impose at least half the ramp window before saturation");
}
