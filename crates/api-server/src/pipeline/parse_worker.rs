//! Background parse worker.
//!
//! Spawned from the ingest finalize handler with the freshly-committed
//! session, this worker:
//!
//!   1. Reads the finalized blob through the [`crate::storage::BlobStore`]
//!      trait (`head_blob` for the size cap, `read_blob` for the bytes).
//!      Backend-agnostic: local-FS in dev, Azure Blob in cloud, S3 / GCS
//!      when those land — the worker doesn't care.
//!   2. For `evidence-zip` content kind, walks every file in the archive,
//!      filters to text logs, runs each through `cmtraceopen_parser`, and
//!      bulk-inserts the parsed entries.
//!   3. Flips `sessions.parse_state` from `pending` to `ok` / `partial` /
//!      `failed` based on outcomes.
//!
//! Constraints baked in for MVP:
//!   - Unzip is in-memory; bundle size is capped at [`MAX_EVIDENCE_ZIP_BYTES`]
//!     (50 MiB) to avoid OOM on a misbehaving client.
//!   - Only `evidence-zip` is implemented. `raw-file` and `ndjson-entries`
//!     fall through to `parse_state = failed` with a logged reason.
//!   - The worker logs progress via `tracing` rather than writing structured
//!     errors back to the DB. A future migration can add a `parse_errors`
//!     table; for now `parse_error_count` on `files` is the breadcrumb.

use std::sync::Arc;

use cmtraceopen_parser::models::log_entry::{LogEntry, Severity};
use cmtraceopen_parser::parser::{decode_bytes, detect_encoding, parse_content, ResolvedParser};
use serde_json::json;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::storage::{BlobStore, MetadataStore, NewEntry, NewFile};

/// Hard cap on evidence-zip bundle size processed by the in-memory unzip
/// path. Keeps a runaway client from OOM'ing the server. Larger bundles get
/// `parse_state = failed` with a logged reason.
pub const MAX_EVIDENCE_ZIP_BYTES: u64 = 50 * 1024 * 1024;

/// Possible terminal `parse_state` values written back to `sessions`.
///
/// Four-state semantic (introduced after the IisW3c classifier fix showed
/// that real-world bundles virtually never hit zero fallbacks):
///   - `ok`                — every file parsed cleanly, zero fallbacks.
///   - `ok-with-fallbacks` — every file produced entries, but at least one
///     file's parser couldn't match some lines (minor fallback noise). This
///     is the expected steady state for real Windows logs.
///   - `partial`           — bundle was empty OR at least one file produced
///     zero entries (parser completely failed on a file).
///   - `failed`            — the parse worker itself panicked / hit a fatal
///     error before writing anything.
const STATE_OK: &str = "ok";
const STATE_OK_WITH_FALLBACKS: &str = "ok-with-fallbacks";
const STATE_PARTIAL: &str = "partial";
const STATE_FAILED: &str = "failed";

/// Inputs the worker pulls off of [`crate::AppState`]. Held as a small
/// struct so callers don't have to construct an entire AppState in tests.
pub struct ParseDeps {
    pub meta: Arc<dyn MetadataStore>,
    pub blobs: Arc<dyn BlobStore>,
}

/// Run a parse for the freshly-finalized session and update `parse_state`
/// when finished.
///
/// `blob_uri` is the opaque URI returned by `BlobStore::finalize` (e.g.
/// `file://...` for local-FS, `azure://...` for Azure). The worker passes
/// it back through the trait rather than parsing the scheme itself, so
/// adding a new backend doesn't require touching this file. `content_kind`
/// is the agent-declared bundle kind from the ingest wire protocol.
///
/// Failures are surfaced through `tracing::error!` and a final
/// `parse_state = failed` write. The function never returns a Result so the
/// caller doesn't need to remember to log it; the spawned task is fire-and-
/// forget by design.
pub async fn parse_session(
    session_id: Uuid,
    blob_uri: String,
    content_kind: String,
    deps: ParseDeps,
) {
    info!(
        %session_id,
        %content_kind,
        "starting background parse"
    );

    let started = std::time::Instant::now();
    let outcome = match content_kind.as_str() {
        "evidence-zip" => parse_evidence_zip(session_id, &blob_uri, &deps).await,
        other => {
            warn!(
                %session_id,
                kind = %other,
                "content kind not yet supported by parse worker"
            );
            Err(format!("content kind {other:?} not yet supported"))
        }
    };
    // Record the wall-clock parse duration regardless of outcome — slow
    // parses are interesting whether they succeed or fail. The histogram
    // is in seconds (Prometheus convention); the recorder applies the
    // default bucket boundaries set in `state::install_metrics_recorder`.
    metrics::histogram!("cmtrace_parse_worker_duration_seconds")
        .record(started.elapsed().as_secs_f64());

    let final_state = match &outcome {
        Ok(ParseOutcome::Ok) => STATE_OK,
        Ok(ParseOutcome::OkWithFallbacks) => STATE_OK_WITH_FALLBACKS,
        Ok(ParseOutcome::Partial) => STATE_PARTIAL,
        Err(reason) => {
            error!(%session_id, %reason, "parse failed");
            STATE_FAILED
        }
    };
    // Mirror `final_state` into the Prometheus counter. Keeping the label
    // values aligned with the DB's `parse_state` column means alert rules
    // can be expressed identically against either source.
    metrics::counter!("cmtrace_parse_worker_runs_total", "result" => final_state).increment(1);

    if let Err(e) = deps
        .meta
        .update_session_parse_state(session_id, final_state)
        .await
    {
        // Last-resort log: if we can't even flip the state, the session
        // will appear stuck on "pending" forever. That's worth a loud error
        // rather than a silent panic.
        error!(
            %session_id,
            error = %e,
            "failed to update sessions.parse_state after parse"
        );
    } else {
        info!(%session_id, %final_state, "parse complete");
    }
}

/// Internal result of a successful parse. See the STATE_* docstring for the
/// full four-state rubric. Short version:
///   - `Ok`                — every file was clean (0 fallbacks, >0 entries).
///   - `OkWithFallbacks`   — every file produced entries, but some parsers
///     emitted fallback errors on un-matched lines. Steady state for real
///     Windows logs (CCM fallback ratio ~0.05%, MSI 1-2%, etc.).
///   - `Partial`           — bundle empty OR at least one file produced
///     zero entries (parser shape-matched but emitted nothing).
enum ParseOutcome {
    Ok,
    OkWithFallbacks,
    Partial,
}

async fn parse_evidence_zip(
    session_id: Uuid,
    blob_uri: &str,
    deps: &ParseDeps,
) -> Result<ParseOutcome, String> {
    // Cap-then-fetch: ask the blob store for the size first so cloud
    // backends (Azure / S3) don't end up streaming a multi-GB bundle just
    // to find out it's over the in-memory unzip limit. For local-FS this
    // is a single stat call; same cost as the old direct `tokio::fs::metadata`.
    let size = deps
        .blobs
        .head_blob(blob_uri)
        .await
        .map_err(|e| format!("head {blob_uri}: {e}"))?;
    if size > MAX_EVIDENCE_ZIP_BYTES {
        return Err(format!(
            "bundle is {size} bytes, exceeds in-memory cap of {} bytes",
            MAX_EVIDENCE_ZIP_BYTES
        ));
    }

    let bytes = deps
        .blobs
        .read_blob(blob_uri)
        .await
        .map_err(|e| format!("read {blob_uri}: {e}"))?;

    // Hand the actual zip walk + parse off to spawn_blocking so the parser
    // (CPU-heavy, fully sync) doesn't block the runtime. The DB writes
    // stay on the async runtime — collect parsed work first, then await
    // the inserts.
    let parsed = tokio::task::spawn_blocking(move || extract_and_parse(bytes))
        .await
        .map_err(|e| format!("parse task panicked: {e}"))??;

    if parsed.is_empty() {
        // No log files in the bundle isn't strictly a failure — the agent
        // might have shipped a manifest-only debug zip — but it leaves
        // nothing for the viewer. Mark partial so the operator can spot it.
        info!(%session_id, "no parseable log files found in bundle");
        return Ok(ParseOutcome::Partial);
    }

    // Track the two error signals separately so we can distinguish "noisy
    // but usable" (fallback errors on some lines of otherwise-parsing files)
    // from "actually broken" (a file the parser couldn't extract anything
    // from). See ParseOutcome variants for the mapping.
    let mut any_file_had_fallbacks = false;
    let mut any_file_produced_nothing = false;
    for file in parsed {
        if file.parse_error_count > 0 {
            any_file_had_fallbacks = true;
        }
        if file.entries.is_empty() {
            any_file_produced_nothing = true;
        }

        let file_id = Uuid::now_v7();
        let new_file = NewFile {
            file_id,
            session_id,
            relative_path: file.relative_path.clone(),
            size_bytes: file.size_bytes,
            format_detected: Some(file.format_detected.clone()),
            parser_kind: Some(file.parser_kind.clone()),
            entry_count: file.entries.len() as u32,
            parse_error_count: file.parse_error_count,
        };

        deps.meta
            .insert_file(new_file)
            .await
            .map_err(|e| format!("insert_file({}): {e}", file.relative_path))?;

        let new_entries: Vec<NewEntry> = file
            .entries
            .into_iter()
            .enumerate()
            .map(|(idx, e)| log_entry_to_new_entry(session_id, file_id, idx, e))
            .collect();

        let n = new_entries.len();
        deps.meta
            .insert_entries_batch(new_entries)
            .await
            .map_err(|e| format!("insert_entries_batch({}): {e}", file.relative_path))?;
        debug!(
            %session_id,
            relative_path = %file.relative_path,
            entries = n,
            "wrote parsed entries"
        );
    }

    Ok(classify_outcome(
        /* parsed_empty */ false, // already handled above via early return
        any_file_produced_nothing,
        any_file_had_fallbacks,
    ))
}

/// Pure classifier for the terminal ParseOutcome, factored out so the rule
/// table is unit-testable without spinning up a blob store.
fn classify_outcome(
    parsed_empty: bool,
    any_file_produced_nothing: bool,
    any_file_had_fallbacks: bool,
) -> ParseOutcome {
    if parsed_empty || any_file_produced_nothing {
        ParseOutcome::Partial
    } else if any_file_had_fallbacks {
        ParseOutcome::OkWithFallbacks
    } else {
        ParseOutcome::Ok
    }
}

/// Per-file parse output collected by the blocking task. Owned plain-data
/// only so it crosses the spawn_blocking boundary cheaply.
struct ParsedFile {
    relative_path: String,
    size_bytes: u64,
    format_detected: String,
    parser_kind: String,
    parse_error_count: u32,
    entries: Vec<LogEntry>,
}

fn extract_and_parse(zip_bytes: Vec<u8>) -> Result<Vec<ParsedFile>, String> {
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("open zip: {e}"))?;

    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("zip entry {i}: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if !is_log_path(&name) {
            debug!(path = %name, "skipping non-log entry");
            continue;
        }

        let size = entry.size();
        let mut buf = Vec::with_capacity(size as usize);
        std::io::copy(&mut entry, &mut buf).map_err(|e| format!("read {name}: {e}"))?;

        let encoding = detect_encoding(&buf);
        let content = match decode_bytes(&buf, encoding) {
            Ok(s) => s,
            Err(e) => {
                warn!(path = %name, error = %e, "failed to decode log file; skipping");
                continue;
            }
        };

        let (result, selection) = parse_content(&content, &name, size);
        let parser_kind = parser_kind_label(&selection);
        let format_detected = format!("{:?}", result.format_detected);

        out.push(ParsedFile {
            relative_path: name,
            size_bytes: size,
            format_detected,
            parser_kind,
            parse_error_count: result.parse_errors,
            entries: result.entries,
        });
    }

    Ok(out)
}

/// Heuristic: is this archive entry a parseable text log?
///
/// Conservative on purpose — we'd rather skip a binary EVTX (which the
/// pure-Rust parser can't handle anyway) than attempt to parse it.
fn is_log_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".log") || lower.ends_with(".txt") || lower.ends_with(".cmtlog") {
        return true;
    }
    false
}

/// Stringify the parser kind for the `files.parser_kind` column.
///
/// Uses Debug formatting because the parser's `ParserKind` doesn't
/// implement `Display` and we don't want to take ownership of the naming
/// scheme — the value is opaque to the API server, only displayed.
fn parser_kind_label(selection: &ResolvedParser) -> String {
    format!("{:?}", selection.parser)
}

/// Map a parsed [`LogEntry`] into the storage-layer [`NewEntry`] shape.
///
/// Pulled out so it can be unit-tested without a DB. `idx` is the 0-based
/// position within the file's entry list and is added to `LogEntry.line_number`
/// to keep the storage column 1-based when the parser hasn't filled it.
fn log_entry_to_new_entry(
    session_id: Uuid,
    file_id: Uuid,
    idx: usize,
    e: LogEntry,
) -> NewEntry {
    let line_number = if e.line_number == 0 {
        // Some parsers (e.g. plain) leave line_number unset. Synthesize a
        // 1-based fallback so the entries column always has a usable value.
        (idx as u32).saturating_add(1)
    } else {
        e.line_number
    };

    // Build extras_json from format-specific fields when present. Only
    // include keys that have a value so the column stays compact and
    // queries can use json_extract(...) without checking for nulls.
    let mut extras = serde_json::Map::new();
    if let Some(v) = e.timestamp_display.as_ref() {
        extras.insert("timestampDisplay".into(), json!(v));
    }
    if let Some(v) = e.source_file.as_ref() {
        extras.insert("sourceFile".into(), json!(v));
    }
    if let Some(v) = e.timezone_offset {
        extras.insert("timezoneOffsetMinutes".into(), json!(v));
    }
    if !e.error_code_spans.is_empty() {
        extras.insert("errorCodeSpans".into(), json!(e.error_code_spans));
    }
    if let Some(v) = e.result_code.as_ref() {
        extras.insert("resultCode".into(), json!(v));
    }
    if let Some(v) = e.gle_code.as_ref() {
        extras.insert("gleCode".into(), json!(v));
    }
    if let Some(v) = e.setup_phase.as_ref() {
        extras.insert("setupPhase".into(), json!(v));
    }
    if let Some(v) = e.operation_name.as_ref() {
        extras.insert("operationName".into(), json!(v));
    }
    extras.insert("format".into(), json!(format!("{:?}", e.format)));

    let extras_json = if extras.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(extras).to_string())
    };

    NewEntry {
        session_id,
        file_id,
        line_number,
        ts_ms: e.timestamp,
        severity: severity_to_int(e.severity),
        component: e.component,
        thread: e.thread_display.or_else(|| e.thread.map(|t| t.to_string())),
        message: e.message,
        extras_json,
    }
}

/// Stable parser-`Severity` → DB int mapping. Kept private + tiny so the
/// schema's "0=Info, 1=Warning, 2=Error" contract has exactly one source
/// of truth.
fn severity_to_int(s: Severity) -> i32 {
    match s {
        Severity::Info => 0,
        Severity::Warning => 1,
        Severity::Error => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmtraceopen_parser::error_db::lookup::ErrorCodeSpan;
    use cmtraceopen_parser::models::log_entry::LogFormat;

    fn empty_entry() -> LogEntry {
        LogEntry {
            id: 0,
            line_number: 0,
            message: String::new(),
            component: None,
            timestamp: None,
            timestamp_display: None,
            severity: Severity::Info,
            thread: None,
            thread_display: None,
            source_file: None,
            format: LogFormat::Plain,
            file_path: String::new(),
            timezone_offset: None,
            error_code_spans: vec![],
            ip_address: None,
            host_name: None,
            mac_address: None,
            result_code: None,
            gle_code: None,
            setup_phase: None,
            operation_name: None,
            http_method: None,
            uri_stem: None,
            uri_query: None,
            status_code: None,
            sub_status: None,
            time_taken_ms: None,
            client_ip: None,
            server_ip: None,
            user_agent: None,
            server_port: None,
            username: None,
            win32_status: None,
            query_name: None,
            query_type: None,
            response_code: None,
            dns_direction: None,
            dns_protocol: None,
            source_ip: None,
            dns_flags: None,
            dns_event_id: None,
            zone_name: None,
            entry_kind: None,
            whatif: None,
            section_name: None,
            section_color: None,
            iteration: None,
            tags: None,
        }
    }

    #[test]
    fn severity_int_mapping_is_0_1_2() {
        assert_eq!(severity_to_int(Severity::Info), 0);
        assert_eq!(severity_to_int(Severity::Warning), 1);
        assert_eq!(severity_to_int(Severity::Error), 2);
    }

    #[test]
    fn log_entry_to_new_entry_passes_ts_ms_through() {
        let mut e = empty_entry();
        e.timestamp = Some(1_700_000_000_000);
        e.severity = Severity::Warning;
        e.line_number = 42;
        e.message = "boom".into();
        e.component = Some("CompA".into());
        e.thread = Some(7);
        e.thread_display = Some("7 (0x7)".into());

        let session = Uuid::now_v7();
        let file = Uuid::now_v7();
        let n = log_entry_to_new_entry(session, file, 0, e);

        assert_eq!(n.session_id, session);
        assert_eq!(n.file_id, file);
        assert_eq!(n.line_number, 42);
        assert_eq!(n.ts_ms, Some(1_700_000_000_000));
        assert_eq!(n.severity, 1);
        assert_eq!(n.component.as_deref(), Some("CompA"));
        assert_eq!(n.thread.as_deref(), Some("7 (0x7)"));
        assert_eq!(n.message, "boom");
    }

    #[test]
    fn log_entry_with_zero_line_falls_back_to_index_plus_one() {
        let mut e = empty_entry();
        e.line_number = 0;
        let n = log_entry_to_new_entry(Uuid::now_v7(), Uuid::now_v7(), 4, e);
        assert_eq!(n.line_number, 5);
    }

    #[test]
    fn extras_json_includes_error_code_spans_when_present() {
        let mut e = empty_entry();
        e.error_code_spans = vec![ErrorCodeSpan {
            start: 0,
            end: 10,
            code_hex: "0x80070005".into(),
            code_decimal: "5".into(),
            description: "ACCESS_DENIED".into(),
            category: "Win32".into(),
        }];
        let n = log_entry_to_new_entry(Uuid::now_v7(), Uuid::now_v7(), 0, e);
        let json: serde_json::Value =
            serde_json::from_str(n.extras_json.as_deref().expect("extras_json")).unwrap();
        assert_eq!(json["errorCodeSpans"][0]["codeHex"], "0x80070005");
    }

    #[test]
    fn is_log_path_accepts_common_extensions_and_rejects_others() {
        assert!(is_log_path("evidence/logs/test.log"));
        assert!(is_log_path("foo.txt"));
        assert!(is_log_path("foo.cmtlog"));
        assert!(is_log_path("DEEP/DIR/UPPER.LOG"));
        assert!(!is_log_path("manifest.json"));
        assert!(!is_log_path("evtx/system.evtx"));
        assert!(!is_log_path("photo.png"));
    }

    #[test]
    fn classify_outcome_empty_bundle_is_partial() {
        assert!(matches!(
            classify_outcome(true, false, false),
            ParseOutcome::Partial
        ));
    }

    #[test]
    fn classify_outcome_broken_file_is_partial() {
        // A file that produced zero entries (parser matched nothing) still
        // trips partial regardless of fallback state.
        assert!(matches!(
            classify_outcome(false, true, false),
            ParseOutcome::Partial
        ));
        assert!(matches!(
            classify_outcome(false, true, true),
            ParseOutcome::Partial
        ));
    }

    #[test]
    fn classify_outcome_fallbacks_only_is_ok_with_fallbacks() {
        // This is the real-world steady state — CCM / Timestamped parsers
        // emit a handful of fallback errors on un-matched lines but every
        // file still has entries.
        assert!(matches!(
            classify_outcome(false, false, true),
            ParseOutcome::OkWithFallbacks
        ));
    }

    #[test]
    fn classify_outcome_clean_bundle_is_ok() {
        assert!(matches!(
            classify_outcome(false, false, false),
            ParseOutcome::Ok
        ));
    }
}
