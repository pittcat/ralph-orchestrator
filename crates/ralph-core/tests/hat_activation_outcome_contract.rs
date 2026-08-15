//! Plan 2026-08-15-1823 (fix empty channel activation observability)
//! Unit 1: contract tests for `hat_activation_outcome` rows in the
//! existing `runtime-trace.jsonl` sidecar.
//!
//! The runtime trace schema stays at `run-diagnosis-trace/v1`; the new
//! `phase=activation`/`kind=hat_activation_outcome` rows are additive
//! and must round-trip, coexist with existing rows, and respect the
//! same field cap as every other row. The contract tests below lock
//! these guarantees so Unit 2's runner code and Unit 3's diagnosis
//! skill can rely on them.

use ralph_core::diagnosis::read_runtime_trace_report;
use ralph_core::diagnostics::{
    RUNTIME_TRACE_SCHEMA_VERSION, RuntimeTraceEntry, RuntimeTraceLogger, RuntimeTracePhase,
};
use serde_json::json;
use tempfile::TempDir;

/// Stable kind tag for the activation outcome row.
const ACTIVATION_OUTCOME_KIND: &str = "hat_activation_outcome";

fn session_dir(tmp: &TempDir) -> std::path::PathBuf {
    let dir = tmp.path().join("session");
    std::fs::create_dir_all(&dir).expect("session dir");
    dir
}

/// T1 — `hat_activation_outcome` round-trip: the new outcome row must
/// serialize cleanly, carry the bounded raw facts, and be readable
/// via the same `read_runtime_trace_report` reader used for every
/// other trace kind. Schema version stays at v1.
#[test]
fn hat_activation_outcome_round_trip_preserves_bounded_fields() {
    let tmp = TempDir::new().expect("TempDir");
    let session = session_dir(&tmp);
    let mut logger = RuntimeTraceLogger::new(&session).expect("RuntimeTraceLogger::new");

    let fields = json!({
        "loop_id": "loop-7",
        "channel_exists": true,
        "channel_bytes": 0,
        "channel_readable": true,
        "merge_succeeded": true,
        "backend_success": true,
        "backend_exit_code": 0,
        "watchdog_timeout": false,
        "backend_termination": false,
        "output_bytes": 42,
        "output_mentions_emit": false,
        "candidate_event_count": 0,
        "accepted_event_count": 0,
        "rejected_event_count": 0,
        "wave_policy_rejection_count": 0,
        "terminal_obligation_topics": ["work.done"],
    });

    let entry = RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation)
        .with_kind(ACTIVATION_OUTCOME_KIND)
        .with_hat("executor")
        .with_source_ref("hat-channel:executor:loop-7:1")
        .with_status("empty")
        .with_fields(fields.clone());
    logger.append(entry);

    let report = read_runtime_trace_report(&session);
    assert_eq!(report.status, ralph_core::diagnosis::BundleStatus::Present);
    assert_eq!(report.record_count, 1);
    assert_eq!(report.first_sequence, Some(1));
    assert!(report.monotonic_sequences);

    let body = std::fs::read_to_string(session.join("runtime-trace.jsonl")).expect("read trace");
    assert_eq!(body.lines().count(), 1, "exactly one row must be written");
    let row: serde_json::Value = serde_json::from_str(body.trim()).expect("row is valid JSON");

    // Schema version stays at v1 — additive contract.
    assert_eq!(
        row.get("schema_version").and_then(|v| v.as_str()),
        Some(RUNTIME_TRACE_SCHEMA_VERSION),
        "schema_version must stay at {RUNTIME_TRACE_SCHEMA_VERSION}, got row={row}"
    );

    // Phase / kind / status / hat / source_ref round-trip.
    assert_eq!(
        row.get("phase").and_then(|v| v.as_str()),
        Some("activation")
    );
    assert_eq!(
        row.get("kind").and_then(|v| v.as_str()),
        Some(ACTIVATION_OUTCOME_KIND)
    );
    assert_eq!(row.get("status").and_then(|v| v.as_str()), Some("empty"));
    assert_eq!(row.get("hat").and_then(|v| v.as_str()), Some("executor"));
    assert_eq!(
        row.get("source_ref")
            .or_else(|| row.get("ref"))
            .and_then(|v| v.as_str()),
        Some("hat-channel:executor:loop-7:1"),
        "source_ref must round-trip (under either source_ref or ref alias)"
    );

    // All bounded scalar fields round-trip exactly.
    let row_fields = row.get("fields").expect("fields present");
    for (key, expected) in [
        ("channel_exists", json!(true)),
        ("channel_bytes", json!(0)),
        ("merge_succeeded", json!(true)),
        ("backend_success", json!(true)),
        ("backend_exit_code", json!(0)),
        ("watchdog_timeout", json!(false)),
        ("output_bytes", json!(42)),
        ("output_mentions_emit", json!(false)),
        ("candidate_event_count", json!(0)),
        ("accepted_event_count", json!(0)),
        ("rejected_event_count", json!(0)),
        ("wave_policy_rejection_count", json!(0)),
    ] {
        assert_eq!(
            row_fields.get(key),
            Some(&expected),
            "field {key} must round-trip exactly, got row_fields={row_fields}"
        );
    }
    assert_eq!(
        row_fields
            .get("terminal_obligation_topics")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(1),
        "terminal_obligation_topics must round-trip as an array of bounded topics"
    );
    assert_eq!(
        row_fields
            .get("terminal_obligation_topics")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str()),
        Some("work.done")
    );
}

/// T2 — Backwards compatibility: rows that pre-date the new outcome
/// kind (no `hat_activation_outcome` kind, no new fields) must still
/// parse cleanly under `read_runtime_trace_report` and contribute to
/// the existing record_count / sequence / monotonic invariants.
#[test]
fn legacy_runtime_trace_rows_still_pass_through_reader() {
    let tmp = TempDir::new().expect("TempDir");
    let session = session_dir(&tmp);
    let mut logger = RuntimeTraceLogger::new(&session).expect("RuntimeTraceLogger::new");

    // Three rows that look like the v1 baseline: phase=batch /
    // accepted / commit, plain kind names, no new outcome fields.
    logger.append(RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Batch).with_hat("executor"));
    logger.append(
        RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Accepted)
            .with_hat("executor")
            .with_topic("work.done"),
    );
    logger.append(RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Commit));

    let report = read_runtime_trace_report(&session);
    assert_eq!(report.status, ralph_core::diagnosis::BundleStatus::Present);
    assert_eq!(
        report.record_count, 3,
        "reader must count all 3 legacy rows"
    );
    assert_eq!(report.first_sequence, Some(1));
    assert_eq!(report.last_sequence, Some(3));
    assert!(report.monotonic_sequences);
    assert_eq!(report.malformed_lines, 0);

    let body = std::fs::read_to_string(session.join("runtime-trace.jsonl")).expect("read trace");
    // Every legacy row still uses the v1 schema string and never
    // carries the activation outcome kind.
    for line in body.lines() {
        let row: serde_json::Value = serde_json::from_str(line).expect("legacy row is valid JSON");
        assert_eq!(
            row.get("schema_version").and_then(|v| v.as_str()),
            Some(RUNTIME_TRACE_SCHEMA_VERSION)
        );
        assert_ne!(
            row.get("kind").and_then(|v| v.as_str()),
            Some(ACTIVATION_OUTCOME_KIND),
            "legacy rows must not be misclassified as activation outcome: {row}"
        );
    }
}

/// T2b — Mixed stream: legacy rows coexist with the new outcome row;
/// the reader keeps the contiguous-sequence invariant.
#[test]
fn legacy_rows_and_activation_outcome_rows_share_one_session() {
    let tmp = TempDir::new().expect("TempDir");
    let session = session_dir(&tmp);
    let mut logger = RuntimeTraceLogger::new(&session).expect("RuntimeTraceLogger::new");

    logger.append(RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Batch));
    logger.append(
        RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation)
            .with_kind(ACTIVATION_OUTCOME_KIND)
            .with_hat("executor")
            .with_status("empty")
            .with_fields(json!({"channel_bytes": 0})),
    );
    logger.append(RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Commit));

    let report = read_runtime_trace_report(&session);
    assert_eq!(report.status, ralph_core::diagnosis::BundleStatus::Present);
    assert_eq!(report.record_count, 3);
    assert_eq!(report.first_sequence, Some(1));
    assert_eq!(report.last_sequence, Some(3));
    assert!(
        report.monotonic_sequences,
        "contiguous sequences across legacy + activation outcome rows"
    );
}

/// T3 — Bounded fields: oversized `ref` / `fields` are bounded by
/// the existing `MAX_SIDECAR_FIELD_BYTES` cap; the on-disk row does
/// not blow past the cap regardless of how pathological the input
/// is. This protects the new outcome row from accidentally leaking
/// full backend output / prompts (plan §6 implementation constraint
/// 5).
#[test]
fn hat_activation_outcome_fields_are_bounded_by_field_cap() {
    use ralph_core::diagnostics::MAX_SIDECAR_FIELD_BYTES;

    let tmp = TempDir::new().expect("TempDir");
    let session = session_dir(&tmp);
    let mut logger = RuntimeTraceLogger::new(&session).expect("RuntimeTraceLogger::new");

    // Pathological source_ref: 4 MiB of arbitrary text. The writer
    // must cap it, never balloon the row.
    let huge_ref = "z".repeat(4 * 1024 * 1024);
    // Pathological `fields`: large nested blobs in iteration order
    // so the recursive cap eventually drops trailing keys to fit
    // under the cap. `cap_json_field` walks scalars and shortens
    // them when the serialized form exceeds the cap.
    let huge_fields = json!({
        "channel_path": "x".repeat(4 * 1024 * 1024),
        "nested": { "deep": "y".repeat(2 * 1024 * 1024) },
    });

    logger.append(
        RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation)
            .with_kind(ACTIVATION_OUTCOME_KIND)
            .with_hat("executor")
            .with_source_ref(huge_ref)
            .with_status("merged")
            .with_fields(huge_fields),
    );

    let body = std::fs::read_to_string(session.join("runtime-trace.jsonl")).expect("read trace");
    // Cap is 8 KiB per field plus a small envelope. The whole row
    // must be far below the input size and bounded.
    assert!(
        body.len() <= MAX_SIDECAR_FIELD_BYTES + 1024,
        "activation outcome row must be bounded by the field cap, got {} bytes",
        body.len()
    );
    assert!(
        body.contains("...[truncated]"),
        "oversized fields must show the truncation marker, got body[0..200]={}",
        &body[..body.len().min(200)]
    );

    // The small scalar `status` (which has its own per-string cap)
    // must survive the cap walk.
    let row: serde_json::Value = serde_json::from_str(body.trim()).expect("row is valid JSON");
    assert_eq!(
        row.get("status").and_then(|v| v.as_str()),
        Some("merged"),
        "small scalar status must survive the cap walk"
    );
}

// Plan 2026-08-15-1823 U15 (R13): a stream containing
// `(malformed JSONL) + (legacy valid row) + (new outcome row)`
// must produce `record_count == 2, malformed_lines == 1` from
// `read_runtime_trace_report`. This locks the malformed-line
// counter so a future reader change cannot silently lose the
// distinction between legacy and new rows in the same file.
#[test]
fn malformed_lines_counter_distinguishes_legacy_and_new_outcome_rows() {
    use std::io::Write;

    let tmp = TempDir::new().expect("TempDir");
    let session = session_dir(&tmp);

    // Manually compose the trace file so we control which lines
    // are valid and which are malformed.
    let trace_path = session.join("runtime-trace.jsonl");
    let mut file = std::fs::File::create(&trace_path).expect("create trace file");
    // 1. Legacy valid row (kind=legacy_kind, no activation outcome).
    writeln!(
        file,
        r#"{{"phase":"activation","kind":"legacy_kind","schema_version":"run-diagnosis-trace/v1","iteration":1,"sequence":1,"ts":"2026-08-15T00:00:00Z"}}"#
    )
    .unwrap();
    // 2. Malformed line.
    writeln!(file, "this is not json").unwrap();
    // 3. New outcome row.
    writeln!(
        file,
        r#"{{"phase":"activation","kind":"hat_activation_outcome","schema_version":"run-diagnosis-trace/v1","iteration":1,"sequence":2,"ts":"2026-08-15T00:00:01Z","status":"empty","fields":{{"channel_exists":true,"channel_bytes":0,"channel_readable":true,"merge_succeeded":true}}}}"#
    )
    .unwrap();
    drop(file);

    let report = read_runtime_trace_report(&session);
    assert_eq!(
        report.record_count, 2,
        "exactly two valid rows (legacy + new outcome) expected, got {}",
        report.record_count
    );
    assert_eq!(
        report.malformed_lines, 1,
        "exactly one malformed line expected, got {}",
        report.malformed_lines
    );

    // Negative case: only malformed rows.
    let tmp = TempDir::new().expect("TempDir");
    let session = session_dir(&tmp);
    let trace_path = session.join("runtime-trace.jsonl");
    let mut file = std::fs::File::create(&trace_path).expect("create trace file");
    writeln!(file, "garbage line 1").unwrap();
    writeln!(file, "garbage line 2").unwrap();
    drop(file);

    let report = read_runtime_trace_report(&session);
    assert_eq!(report.record_count, 0);
    assert_eq!(report.malformed_lines, 2);

    // Negative case: only legacy valid rows.
    let tmp = TempDir::new().expect("TempDir");
    let session = session_dir(&tmp);
    let trace_path = session.join("runtime-trace.jsonl");
    let mut file = std::fs::File::create(&trace_path).expect("create trace file");
    writeln!(
        file,
        r#"{{"phase":"activation","kind":"legacy_kind","schema_version":"run-diagnosis-trace/v1","iteration":1,"sequence":1,"ts":"2026-08-15T00:00:00Z"}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"phase":"activation","kind":"legacy_kind","schema_version":"run-diagnosis-trace/v1","iteration":1,"sequence":2,"ts":"2026-08-15T00:00:01Z"}}"#
    )
    .unwrap();
    drop(file);

    let report = read_runtime_trace_report(&session);
    assert_eq!(report.record_count, 2);
    assert_eq!(report.malformed_lines, 0);
}
