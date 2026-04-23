//! Integration tests for the PII-redaction layer.
//!
//! Run with: `cargo test -p agent --features redaction`
//!
//! These tests exercise the full redaction pipeline end-to-end:
//! default rules, custom rules, the `enabled = false` bypass, and the
//! [`EvidenceOrchestrator`] staging-dir walk that applies redaction to
//! collected text files before zipping.

#![cfg(feature = "redaction")]

use cmtraceopen_agent::collectors::agent_logs::AgentLogsCollector;
use cmtraceopen_agent::collectors::dsregcmd::DsRegCmdCollector;
use cmtraceopen_agent::collectors::event_logs::EventLogsCollector;
use cmtraceopen_agent::collectors::evidence::EvidenceOrchestrator;
use cmtraceopen_agent::collectors::logs::LogsCollector;
use cmtraceopen_agent::config::{AgentConfig, RedactionConfig, RedactionRule};
use cmtraceopen_agent::redact::Redactor;
use tempfile::TempDir;

// ─── Helper ──────────────────────────────────────────────────────────────────

fn default_redactor() -> Redactor {
    Redactor::from_config(&AgentConfig::default()).expect("compile default rules")
}

// ─── Unit-level integration: default ruleset ─────────────────────────────────

#[test]
fn default_rules_redact_username_path() {
    let r = default_redactor();
    let input = r"C:\Users\alice\AppData\Local\Temp\intune.log";
    let out = r.apply(input);
    assert_eq!(out, r"C:\Users\<USER>\AppData\Local\Temp\intune.log");
}

#[test]
fn default_rules_redact_guid() {
    let r = default_redactor();
    let input = "EnrollmentID: 6ba7b810-9dad-11d1-80b4-00c04fd430c8 enrolled";
    let out = r.apply(input);
    assert_eq!(out, "EnrollmentID: <GUID> enrolled");
}

#[test]
fn default_rules_redact_email() {
    let r = default_redactor();
    let input = "Principal: john.doe@contoso.com authorized";
    let out = r.apply(input);
    assert_eq!(out, "Principal: <EMAIL> authorized");
}

#[test]
fn default_rules_redact_internal_ipv4() {
    let r = default_redactor();
    let input = "MDM server at 10.1.2.3 responded 200";
    let out = r.apply(input);
    assert_eq!(out, "MDM server at <INTERNAL_IP> responded 200");
}

#[test]
fn default_rules_preserve_public_ip() {
    let r = default_redactor();
    let input = "NTP sync from 203.0.113.5 ok";
    let out = r.apply(input);
    assert_eq!(out, "NTP sync from 203.0.113.5 ok");
}

#[test]
fn all_pii_types_in_one_fixture() {
    let r = default_redactor();
    // Fixture line mixing username, GUID, email, and internal IP.
    let input = concat!(
        r"C:\Users\jsmith\ccmexec.log",
        " device=550e8400-e29b-41d4-a716-446655440000",
        " admin=jsmith@corp.example.com",
        " server=10.20.30.40"
    );
    let out = r.apply(input);
    assert!(
        out.contains("<USER>"),
        "username should be redacted: {out}"
    );
    assert!(out.contains("<GUID>"), "GUID should be redacted: {out}");
    assert!(out.contains("<EMAIL>"), "email should be redacted: {out}");
    assert!(
        out.contains("<INTERNAL_IP>"),
        "internal IP should be redacted: {out}"
    );
    // Confirm the raw PII is gone.
    assert!(!out.contains("jsmith"), "raw username must not appear: {out}");
    assert!(
        !out.contains("550e8400"),
        "raw GUID must not appear: {out}"
    );
    assert!(
        !out.contains("10.20.30.40"),
        "raw IP must not appear: {out}"
    );
}

// ─── enabled = false bypass ───────────────────────────────────────────────────

#[test]
fn disabled_redaction_round_trips_raw_data() {
    let cfg = AgentConfig {
        redaction: RedactionConfig {
            enabled: false,
            patterns: Vec::new(),
        },
        ..AgentConfig::default()
    };
    let r = Redactor::from_config(&cfg).unwrap();
    let input = r"C:\Users\bob\file.log user=bob@example.com ip=10.0.0.1";
    let out = r.apply(input);
    // Exact round-trip: no substitutions.
    assert_eq!(out.as_ref(), input);
}

// ─── Custom rules ─────────────────────────────────────────────────────────────

#[test]
fn custom_rule_is_applied_after_defaults() {
    let cfg = AgentConfig {
        redaction: RedactionConfig {
            enabled: true,
            patterns: vec![RedactionRule {
                name: "hostname".into(),
                regex: r"\bWIN-[A-Z0-9]{6,}\b".into(),
                replacement: "<HOSTNAME>".into(),
            }],
        },
        ..AgentConfig::default()
    };
    let r = Redactor::from_config(&cfg).unwrap();
    let input = "Device WIN-ABC12345 joined domain";
    let out = r.apply(input);
    assert_eq!(out, "Device <HOSTNAME> joined domain");
}

// ─── Bundle integration: orchestrator applies redaction before zip ────────────

#[tokio::test]
async fn bundle_has_no_pii_after_redaction() {
    // Seed a fake log file containing all PII types the default ruleset covers.
    let src = TempDir::new().unwrap();
    let log_content = concat!(
        "Log from C:\\Users\\testuser\\AppData\\Local\\Temp\\install.log\n",
        "EnrollmentId: 6ba7b810-9dad-11d1-80b4-00c04fd430c8\n",
        "Admin: admin@corp.example.com connected from 10.50.1.99\n",
    );
    std::fs::write(src.path().join("sensitive.log"), log_content).unwrap();

    let work = TempDir::new().unwrap();
    let pattern = format!(
        "{}/*.log",
        src.path().to_string_lossy().replace('\\', "/")
    );

    let redactor = default_redactor();
    let orch = EvidenceOrchestrator::new(
        LogsCollector::new(vec![pattern]),
        EventLogsCollector::with_defaults(),
        DsRegCmdCollector::new(),
        AgentLogsCollector::new(work.path().join("_no_agent_logs")),
        work.path().to_path_buf(),
        redactor,
    );

    let bundle = orch.collect_once().await.expect("collect");
    assert!(bundle.zip_path.exists());

    // Unzip and find the log file.
    let bytes = std::fs::read(&bundle.zip_path).unwrap();
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).unwrap();

    let mut found_log = false;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        if entry.name().ends_with(".log") {
            found_log = true;
            let mut content = String::new();
            std::io::Read::read_to_string(&mut entry, &mut content).unwrap();

            // PII must not appear.
            assert!(
                !content.contains("testuser"),
                "raw username must not be in bundle: {content}"
            );
            assert!(
                !content.contains("6ba7b810"),
                "raw GUID must not be in bundle: {content}"
            );
            assert!(
                !content.contains("admin@corp"),
                "raw email must not be in bundle: {content}"
            );
            assert!(
                !content.contains("10.50.1.99"),
                "raw internal IP must not be in bundle: {content}"
            );

            // Redaction tokens must be present.
            assert!(content.contains("<USER>"), "USER token missing: {content}");
            assert!(content.contains("<GUID>"), "GUID token missing: {content}");
            assert!(
                content.contains("<EMAIL>"),
                "EMAIL token missing: {content}"
            );
            assert!(
                content.contains("<INTERNAL_IP>"),
                "INTERNAL_IP token missing: {content}"
            );
        }
    }
    assert!(found_log, "log file must be present in the bundle");
}

/// `manifest.json` carries a UUID-v7 bundleId. The GUID rule (with
/// `\b` anchors) would otherwise rewrite it to `<GUID>` and destroy
/// forensic provenance. The skip list in `redact_staging_dir` must
/// preserve the file untouched even when redaction is enabled.
#[tokio::test]
async fn bundle_manifest_json_is_not_redacted() {
    let src = TempDir::new().unwrap();
    std::fs::write(src.path().join("noise.log"), "nothing sensitive\n").unwrap();

    let work = TempDir::new().unwrap();
    let pattern = format!(
        "{}/*.log",
        src.path().to_string_lossy().replace('\\', "/")
    );

    let redactor = default_redactor();
    let orch = EvidenceOrchestrator::new(
        LogsCollector::new(vec![pattern]),
        EventLogsCollector::with_defaults(),
        DsRegCmdCollector::new(),
        AgentLogsCollector::new(work.path().join("_no_agent_logs")),
        work.path().to_path_buf(),
        redactor,
    );

    let bundle = orch.collect_once().await.expect("collect");
    let bytes = std::fs::read(&bundle.zip_path).unwrap();
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).unwrap();

    let mut found_manifest = false;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        if entry.name().ends_with("manifest.json") {
            found_manifest = true;
            let mut content = String::new();
            std::io::Read::read_to_string(&mut entry, &mut content).unwrap();
            // The bundleId must STILL be a valid GUID (not <GUID>).
            assert!(
                !content.contains("<GUID>"),
                "manifest.json must not be redacted: {content}"
            );
            // Must still contain the bundleId field with a UUID-shaped
            // value matching the bundle metadata.
            let expected = bundle.metadata.bundle_id.to_string();
            assert!(
                content.contains(&expected),
                "manifest.json should retain its bundleId {expected}: {content}"
            );
        }
    }
    assert!(found_manifest, "manifest.json must be present in the bundle");
}

#[tokio::test]
async fn bundle_with_disabled_redaction_preserves_pii() {
    let src = TempDir::new().unwrap();
    let log_content = "User C:\\Users\\alice\\file.log ip=10.0.0.1\n";
    std::fs::write(src.path().join("raw.log"), log_content).unwrap();

    let work = TempDir::new().unwrap();
    let pattern = format!(
        "{}/*.log",
        src.path().to_string_lossy().replace('\\', "/")
    );

    let cfg = AgentConfig {
        redaction: RedactionConfig {
            enabled: false,
            patterns: Vec::new(),
        },
        ..AgentConfig::default()
    };
    let redactor = Redactor::from_config(&cfg).unwrap();

    let orch = EvidenceOrchestrator::new(
        LogsCollector::new(vec![pattern]),
        EventLogsCollector::with_defaults(),
        DsRegCmdCollector::new(),
        AgentLogsCollector::new(work.path().join("_no_agent_logs")),
        work.path().to_path_buf(),
        redactor,
    );

    let bundle = orch.collect_once().await.expect("collect");
    let bytes = std::fs::read(&bundle.zip_path).unwrap();
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).unwrap();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        if entry.name().ends_with(".log") {
            let mut content = String::new();
            std::io::Read::read_to_string(&mut entry, &mut content).unwrap();
            // Raw PII must still be present when redaction is disabled.
            assert!(
                content.contains("alice"),
                "raw username must survive disabled redaction: {content}"
            );
            assert!(
                content.contains("10.0.0.1"),
                "raw IP must survive disabled redaction: {content}"
            );
        }
    }
}
