use parse_load_harness::reporter::compare::Comparison;
use std::fs;

fn write_summary(dir: &std::path::Path, p99: u64, throughput: f64, err: u64) {
    fs::create_dir_all(dir).unwrap();
    let body = serde_json::json!({
        "run_uuid": dir.file_name().unwrap().to_string_lossy(),
        "seed": 1,
        "total_bundles": 100,
        "error_count": err,
        "counts_by_state": {"ok": 100u64 - err, "failed": err},
        "finalize_ms": {"p50": 10, "p95": 20, "p99": 30, "max": 40},
        "parse_ms": {"p50": 100, "p95": 500, "p99": p99, "max": p99 * 2},
        "wall_seconds": 60.0,
        "throughput_bundles_per_min": throughput
    });
    fs::write(dir.join("summary.json"), serde_json::to_vec_pretty(&body).unwrap()).unwrap();
}

#[test]
fn identical_runs_have_no_regressions() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write_summary(&a, 1000, 100.0, 0);
    write_summary(&b, 1000, 100.0, 0);
    let c = Comparison::build(&a, &b, 10.0).unwrap();
    assert!(!c.has_regression());
}

#[test]
fn regression_in_p99_above_threshold_is_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write_summary(&a, 1000, 100.0, 0);
    write_summary(&b, 1500, 100.0, 0); // 50% slower
    let c = Comparison::build(&a, &b, 10.0).unwrap();
    assert!(c.has_regression());
    let md = c.render_markdown();
    assert!(md.contains("parse_ms.p99"));
    assert!(md.contains("⚠️"));
}

#[test]
fn throughput_drop_above_threshold_is_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write_summary(&a, 1000, 100.0, 0);
    write_summary(&b, 1000, 50.0, 0); // 50% slower throughput
    let c = Comparison::build(&a, &b, 10.0).unwrap();
    assert!(c.has_regression());
}
