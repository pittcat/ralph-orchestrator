//! Plan 2026-08-26-1104 Unit 2 acceptance tests: causal identity
//! + contract receipt.
//!
//! Locks the cross-cutting wire format that the attribution engine
//! (U8), the run-diagnosis skill (U10), and the runtime receipt
//! helpers (U3/U4/U5) all consume:
//!
//! - `RuntimeTraceEntry.causal` survives a serde round-trip with
//!   `loop_id` and `iteration` intact (S2.1).
//! - `RuntimeTracePhase::Decision` serializes as `"decision"` and
//!   the only exhaustive match on the enum still covers the
//!   variant (S2.1).
//! - `DiagnosticsCollector::set_causal_context` auto-stamps every
//!   subsequent `log_runtime_trace` row in the same session
//!   (S2.1: same `loop_id` / `iteration`, sequence strictly
//!   monotonic).
//! - `DiagnosticsCollector::emit_contract_receipt` writes
//!   **exactly one** `kind=contract_receipt` row per session
//!   carrying `contract_digest` / `terminal_topics_digest` /
//!   `hats_digest` / `preset_label` (S2.2), and re-emits are
//!   idempotent.
//! - Two configs that differ only in `event_policy.schemas`
//!   produce different `contract_digest`s; identical inputs
//!   produce identical digests (S2.3).
//! - Pre-U02 rows (no `causal` field) still parse with
//!   `causal == None` and 0 malformed lines (S2.4).
//!
//! The tests deliberately avoid spinning up a full `EventLoop`
//! to stay hermetic — they drive the production
//! `DiagnosticsCollector` API directly and assert against the
//! on-disk `runtime-trace.jsonl` via the existing
//! `read_runtime_trace_report` reader.

use std::collections::HashMap;
use std::fs;

use ralph_core::config::{EventPolicyConfig, EventSchema, HatConfig, PayloadType};
use ralph_core::diagnosis::read_runtime_trace_report;
use ralph_core::diagnostics::{
    CausalContext, DiagnosticsCollector, DiagnosticsOptions, RuntimeTraceEntry, RuntimeTraceLogger,
    RuntimeTracePhase, compute_contract_digest,
};
use serde_json::json;
use tempfile::TempDir;

fn causal_evidence_options(session_dir: &std::path::Path) -> DiagnosticsOptions {
    DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: false,
        trace_only: false,
        session_dir: Some(session_dir.to_path_buf()),
        workspace_root: None,
        causal_evidence: true,
    }
}

fn collector_with_session(session_dir: &std::path::Path) -> DiagnosticsCollector {
    DiagnosticsCollector::with_options(session_dir, &causal_evidence_options(session_dir))
        .expect("DiagnosticsCollector::with_options")
}

#[test]
fn causal_context_serde_roundtrip() {
    let ctx = CausalContext {
        loop_id: "loop-abc".to_string(),
        iteration: 7,
    };
    let encoded = serde_json::to_string(&ctx).expect("encode");
    // Field order is whatever serde decides; both keys must be present.
    assert!(encoded.contains("\"loop_id\":\"loop-abc\""));
    assert!(encoded.contains("\"iteration\":7"));
    let decoded: CausalContext = serde_json::from_str(&encoded).expect("decode");
    assert_eq!(decoded, ctx);
}

#[test]
fn decision_phase_serializes_as_decision() {
    let entry = RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Decision);
    assert_eq!(entry.kind, "decision");
    let encoded = serde_json::to_value(&entry).expect("encode");
    assert_eq!(encoded["phase"], json!("decision"));
}

#[test]
fn set_causal_context_auto_stamps_subsequent_entries() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.set_causal_context(CausalContext {
        loop_id: "loop-42".to_string(),
        iteration: 3,
    });
    collector.log_runtime_trace(
        RuntimeTraceEntry::new(3, 0, RuntimeTracePhase::Activation).with_hat("executor"),
    );
    collector.log_runtime_trace(
        RuntimeTraceEntry::new(3, 0, RuntimeTracePhase::Batch).with_topic("plan.ready"),
    );

    let body = fs::read_to_string(session.join("runtime-trace.jsonl")).expect("read");
    let lines: Vec<serde_json::Value> = body
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("parse"))
        .collect();
    assert_eq!(lines.len(), 2, "two rows must be persisted");
    for line in &lines {
        assert_eq!(line["causal"]["loop_id"], json!("loop-42"));
        assert_eq!(line["causal"]["iteration"], json!(3));
    }
    // Sequence strictly monotonic (U0 contract).
    assert!(
        lines[0]["sequence"].as_u64().unwrap() < lines[1]["sequence"].as_u64().unwrap(),
        "sequence must be strictly monotonic"
    );

    let report = read_runtime_trace_report(&session);
    assert_eq!(report.record_count, 2);
    assert!(
        report.monotonic_sequences,
        "monotonic_sequences must be true"
    );
    assert_eq!(report.malformed_lines, 0, "no malformed lines expected");
}

#[test]
fn entry_can_override_collector_causal_context() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.set_causal_context(CausalContext {
        loop_id: "loop-default".to_string(),
        iteration: 0,
    });
    // Caller-supplied causal must win over the collector default
    // (lets tests / hand-built envelopes pin the value without
    // threading through the collector).
    collector.log_runtime_trace(
        RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Batch).with_causal(CausalContext {
            loop_id: "loop-override".to_string(),
            iteration: 5,
        }),
    );
    let body = fs::read_to_string(session.join("runtime-trace.jsonl")).expect("read");
    let value: serde_json::Value =
        serde_json::from_str(body.lines().next().unwrap()).expect("parse");
    assert_eq!(value["causal"]["loop_id"], json!("loop-override"));
    assert_eq!(value["causal"]["iteration"], json!(5));
}

#[test]
fn set_causal_context_re_stamps_iteration_each_iteration() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    for iter in [0u64, 1, 2] {
        collector.set_causal_context(CausalContext {
            loop_id: "loop-evolving".to_string(),
            iteration: iter,
        });
        collector.log_runtime_trace(
            RuntimeTraceEntry::new(iter, 0, RuntimeTracePhase::Activation)
                .with_hat(format!("hat-{iter}")),
        );
    }
    let body = fs::read_to_string(session.join("runtime-trace.jsonl")).expect("read");
    let iterations: Vec<u64> = body
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("parse");
            v["causal"]["iteration"].as_u64().unwrap()
        })
        .collect();
    assert_eq!(iterations, vec![0, 1, 2]);
}

#[test]
fn emit_contract_receipt_writes_exactly_one_row() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    let fields = json!({
        "contract_digest": "0000000000000000",
        "terminal_topics_digest": "1111111111111111",
        "hats_digest": "2222222222222222",
        "preset_label": "ce-executor-pipeline",
    });
    collector.emit_contract_receipt(fields.clone());
    // Second emit must be a no-op (S2.2: 恰好一条).
    collector.emit_contract_receipt(json!({
        "contract_digest": "ffffffffffffffff",
        "preset_label": "different",
    }));

    let body = fs::read_to_string(session.join("runtime-trace.jsonl")).expect("read");
    let contract_rows: Vec<serde_json::Value> = body
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("parse"))
        .filter(|v: &serde_json::Value| v["kind"] == json!("contract_receipt"))
        .collect();
    assert_eq!(
        contract_rows.len(),
        1,
        "exactly one contract_receipt row expected"
    );
    let row = &contract_rows[0];
    assert_eq!(row["phase"], json!("decision"));
    assert_eq!(row["fields"]["contract_digest"], json!("0000000000000000"));
    assert_eq!(row["fields"]["preset_label"], json!("ce-executor-pipeline"));
    assert_eq!(
        row["fields"]["terminal_topics_digest"],
        json!("1111111111111111")
    );
    assert_eq!(row["fields"]["hats_digest"], json!("2222222222222222"));
}

#[test]
fn contract_digest_changes_when_schemas_change() {
    let mut hats_a: HashMap<String, HatConfig> = HashMap::new();
    hats_a.insert(
        "executor".to_string(),
        HatConfig {
            name: "Executor".to_string(),
            ..HatConfig::default()
        },
    );
    let mut policy_a = EventPolicyConfig::default();
    let mut schemas_a = std::collections::HashMap::new();
    schemas_a.insert(
        "work.done".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            ..EventSchema::default()
        },
    );
    policy_a.schemas = schemas_a;

    let mut policy_b = policy_a.clone();
    let mut schemas_b = std::collections::HashMap::new();
    schemas_b.insert(
        "work.failed".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            ..EventSchema::default()
        },
    );
    policy_b.schemas = schemas_b;

    let bundle_a = compute_contract_digest(Some(&policy_a), &hats_a, "ce-executor");
    let bundle_b = compute_contract_digest(Some(&policy_b), &hats_a, "ce-executor");
    assert_ne!(
        bundle_a["contract_digest"], bundle_b["contract_digest"],
        "different schemas must produce different contract_digest"
    );
    // The hats digest is computed over the hat map + preset label
    // and must stay stable across the schema-only flip.
    assert_eq!(bundle_a["hats_digest"], bundle_b["hats_digest"]);
    assert_eq!(bundle_a["preset_label"], bundle_b["preset_label"]);
}

#[test]
fn contract_digest_is_deterministic_across_runs() {
    let mut hats: HashMap<String, HatConfig> = HashMap::new();
    hats.insert(
        "executor".to_string(),
        HatConfig {
            name: "Executor".to_string(),
            ..HatConfig::default()
        },
    );
    let mut policy = EventPolicyConfig::default();
    let mut schemas = std::collections::HashMap::new();
    schemas.insert(
        "work.done".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            ..EventSchema::default()
        },
    );
    policy.schemas = schemas;

    // Two independent calls — HashMap iteration order is
    // randomized so a non-deterministic implementation would
    // produce two different digests here. The BTreeMap-sorted
    // implementation must agree across calls.
    let first = compute_contract_digest(Some(&policy), &hats, "ce-executor");
    let second = compute_contract_digest(Some(&policy), &hats, "ce-executor");
    assert_eq!(first["contract_digest"], second["contract_digest"]);
    assert_eq!(
        first["terminal_topics_digest"],
        second["terminal_topics_digest"]
    );
    assert_eq!(first["hats_digest"], second["hats_digest"]);
}

#[test]
fn pre_u02_row_without_causal_field_parses_cleanly() {
    // S2.4: pre-U02 writer code stamped rows without `causal`.
    // The reader must accept those rows without bumping
    // `malformed_lines` and surface them with `causal == None`.
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");

    let mut logger = RuntimeTraceLogger::new(&session).expect("logger");
    // Build a v1-shaped entry: no `causal` field present.
    let legacy_entry =
        RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation).with_hat("legacy-hat");
    logger.append(legacy_entry);

    let report = read_runtime_trace_report(&session);
    assert_eq!(report.record_count, 1, "one row must be projected");
    assert_eq!(
        report.malformed_lines, 0,
        "pre-U02 row must not count as malformed"
    );

    let body = fs::read_to_string(session.join("runtime-trace.jsonl")).expect("read");
    let v: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).expect("parse");
    assert!(
        v.get("causal").is_none(),
        "pre-U02 row must serialize without a causal field"
    );
}

#[test]
fn contract_receipt_emitted_into_session_alongside_lifecycle_rows() {
    // End-to-end shape: a session that has been told its causal
    // identity and asked for a contract receipt must contain
    // BOTH the lifecycle rows (with auto-stamped causal) and the
    // single contract_receipt row, with `malformed_lines == 0`.
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.set_causal_context(CausalContext {
        loop_id: "loop-z".to_string(),
        iteration: 0,
    });
    collector.emit_contract_receipt(json!({
        "contract_digest": "abcdef0123456789",
        "terminal_topics_digest": "1111111111111111",
        "hats_digest": "2222222222222222",
        "preset_label": "ce-executor-pipeline",
    }));
    collector.log_runtime_trace(
        RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation).with_hat("executor"),
    );

    let report = read_runtime_trace_report(&session);
    assert_eq!(report.record_count, 2);
    assert_eq!(report.malformed_lines, 0);
    assert!(report.monotonic_sequences);
}
