//! Plan 2026-08-26-1104 Unit 3 acceptance tests: policy and origin
//! decision receipts.
//!
//! Locks the per-event `kind=policy_receipt` wire format that the
//! attribution engine (U8) and the `ralph diagnose --causal`
//! command (U9) consume:
//!
//! - Accept receipts carry `decision=accept`, `rule_refs`,
//!   `event_digest`, `topic`, `hat`, `contract_digest` (S3.1).
//! - Policy reject receipts carry `decision=reject`,
//!   `reason_code` (stable machine-readable string) and a
//!   `retry_key` that reconciles byte-for-byte with the
//!   corresponding `.ralph/recovery.jsonl` `RejectionRecord`
//!   row (S3.2).
//! - Origin-guard reject receipts persist a row (today this gate
//!   has no recovery.jsonl path); `rule_refs` contains
//!   `origin_guard` (S3.3).
//! - Each row stays bounded: single field ≤ 8 KiB, no full
//!   payload is copied onto the wire (S3.4).
//!
//! The tests deliberately avoid spinning up a full `EventLoop`
//! to stay hermetic — they drive the production
//! `DiagnosticsCollector::emit_policy_receipt` API directly
//! and assert against the on-disk `runtime-trace.jsonl`.

use std::fs;
use std::path::Path;

use ralph_core::diagnosis::read_runtime_trace_report;
use ralph_core::diagnostics::{
    CausalContext, DiagnosticsCollector, DiagnosticsOptions, PolicyReceiptDecision,
    RuntimeTraceEntry, RuntimeTracePhase,
};
use ralph_core::state::{RejectionRecord, append_rejection, read_rejection_log};
use serde_json::json;
use tempfile::TempDir;

fn causal_evidence_options(session_dir: &Path) -> DiagnosticsOptions {
    DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: false,
        trace_only: false,
        session_dir: Some(session_dir.to_path_buf()),
        workspace_root: None,
        causal_evidence: true,

        causal_evidence_window_capacity: None,
    }
}

fn collector_with_session(session_dir: &Path) -> DiagnosticsCollector {
    DiagnosticsCollector::with_options(session_dir, &causal_evidence_options(session_dir))
        .expect("DiagnosticsCollector::with_options")
}

/// Read every line of `runtime-trace.jsonl` as a `serde_json::Value`.
fn read_trace_lines(session: &Path) -> Vec<serde_json::Value> {
    let path = session.join("runtime-trace.jsonl");
    let body = fs::read_to_string(&path).expect("read runtime-trace.jsonl");
    body.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("parse trace line"))
        .collect()
}

fn policy_receipt_rows(session: &Path) -> Vec<serde_json::Value> {
    read_trace_lines(session)
        .into_iter()
        .filter(|entry| entry.get("kind").and_then(|k| k.as_str()) == Some("policy_receipt"))
        .collect()
}

#[test]
fn policy_receipt_accept_writes_row_with_required_fields() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    // Contract receipt must land before policy_receipt so the
    // cache is populated and `contract_digest` propagates.
    collector.emit_contract_receipt(json!({
        "contract_digest": "abc123def4567890",
        "terminal_topics_digest": "1111111111111111",
        "hats_digest": "2222222222222222",
        "preset_label": "test-preset",
    }));
    collector.set_causal_context(CausalContext {
        loop_id: "loop-s31".to_string(),
        iteration: 1,
    });

    let event_payload = json!({
        "topic": "plan.ready",
        "payload": "{\"plan_id\":\"p-1\"}",
    });
    collector.emit_policy_receipt(
        PolicyReceiptDecision::Accept,
        "plan.ready",
        Some("executor"),
        &["event_policy"],
        None,
        Some(&event_payload),
    );

    let rows = policy_receipt_rows(&session);
    assert_eq!(rows.len(), 1, "expected exactly one policy_receipt row");
    let row = &rows[0];
    assert_eq!(row["phase"], json!("decision"));
    assert_eq!(row["kind"], json!("policy_receipt"));
    assert_eq!(row["hat"], json!("executor"));
    assert_eq!(row["topic"], json!("plan.ready"));

    let fields = &row["fields"];
    assert_eq!(fields["decision"], json!("accept"));
    assert_eq!(fields["rule_refs"], json!(["event_policy"]));
    assert_eq!(fields["topic"], json!("plan.ready"));
    assert_eq!(fields["hat"], json!("executor"));
    assert_eq!(
        fields["contract_digest"],
        json!("abc123def4567890"),
        "contract_digest must come from the cache populated by emit_contract_receipt"
    );
    assert!(
        fields["event_digest"].is_string() && fields["event_digest"].as_str().unwrap().len() == 16,
        "event_digest must be the 16-hex-char SHA-256 prefix (got: {:?})",
        fields["event_digest"]
    );
    assert!(
        fields.get("reason_code").is_none(),
        "accept receipt must NOT carry reason_code"
    );
    assert!(
        fields.get("retry_key").is_none(),
        "accept receipt must NOT carry retry_key"
    );
}

#[test]
fn policy_receipt_reject_reconciles_with_recovery_jsonl_by_retry_key() {
    let temp = TempDir::new().expect("TempDir");
    let workspace = temp.path().to_path_buf();
    let session = workspace.join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    // Stabilise the contract digest so the cached value matches
    // what policy_receipt will report back.
    collector.emit_contract_receipt(json!({
        "contract_digest": "digest-s32",
        "terminal_topics_digest": "tt-digest-s32",
        "hats_digest": "h-digest-s32",
        "preset_label": "test-preset",
    }));

    // 1) Emit the policy_receipt row.
    let rejection_topic = "plan.ready".to_string();
    let rejection_hat = "executor".to_string();
    let rejection_reason_code = "policy:missing_required_field".to_string();
    let finding = json!({
        "topic": rejection_topic,
        "reason_code": "missing_required_field",
        "message": "required field `payload` missing",
    });
    collector.emit_policy_receipt(
        PolicyReceiptDecision::Reject,
        rejection_topic.clone(),
        Some(&rejection_hat),
        &["event_policy"],
        Some(&rejection_reason_code),
        Some(&finding),
    );

    // 2) Mirror what `correction::emit_correction_context` writes
    //    to recovery.jsonl for a unified-policy rejection: a
    //    `RejectionRecord` carrying the same `(hat, topic,
    //    reason_code)` triple.
    let record = RejectionRecord::new(
        rejection_hat.clone(),
        rejection_topic.clone(),
        rejection_reason_code.clone(),
        1,
    );
    append_rejection(&workspace, &record).expect("append_rejection");

    // 3) Read both back and reconcile.
    let policy_rows = policy_receipt_rows(&session);
    assert_eq!(policy_rows.len(), 1);
    let policy_row = &policy_rows[0];
    let fields = &policy_row["fields"];
    let policy_retry_key = fields["retry_key"].as_str().expect("retry_key is string");

    let recovery_records = read_rejection_log(&workspace).expect("read_rejection_log");
    assert_eq!(recovery_records.len(), 1);
    let recovery_retry_key = recovery_records[0].retry_key();

    assert_eq!(
        policy_retry_key, recovery_retry_key,
        "policy_receipt.retry_key must equal RejectionRecord.retry_key byte-for-byte"
    );

    // The policy_receipt must also carry the stable reason_code
    // (without truncation) and the contract_digest cache pointer.
    assert_eq!(fields["reason_code"], json!(rejection_reason_code));
    assert_eq!(fields["contract_digest"], json!("digest-s32"));
    assert_eq!(fields["decision"], json!("reject"));
    assert_eq!(fields["rule_refs"], json!(["event_policy"]));
}

#[test]
fn policy_receipt_origin_reject_includes_origin_guard_in_rule_refs() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.emit_contract_receipt(json!({
        "contract_digest": "digest-s33",
        "terminal_topics_digest": "tt-s33",
        "hats_digest": "h-s33",
        "preset_label": "test-preset",
    }));

    let reason_code = "origin:out-of-scope topic for declared hat".to_string();
    collector.emit_policy_receipt(
        PolicyReceiptDecision::Reject,
        "plan.ready",
        Some("worker"),
        &["origin_guard"],
        Some(&reason_code),
        None,
    );

    let rows = policy_receipt_rows(&session);
    assert_eq!(rows.len(), 1);
    let fields = &rows[0]["fields"];
    assert_eq!(fields["decision"], json!("reject"));
    assert_eq!(
        fields["rule_refs"],
        json!(["origin_guard"]),
        "origin-guard rejection MUST carry `origin_guard` in rule_refs (S3.3)"
    );
    assert_eq!(fields["reason_code"], json!(reason_code));
    assert!(
        fields.get("retry_key").is_some(),
        "origin-guard reject receipt must carry retry_key for future recovery.jsonl reconciliation"
    );
    // event_digest must still be present even though the caller
    // did not supply a payload (the helper falls back to a
    // stable hash over topic+hat+reason_code).
    assert!(fields["event_digest"].is_string());
}

#[test]
fn policy_receipt_event_digest_is_stable_for_identical_payload() {
    // Determinism guard: the same payload must hash to the same
    // event_digest across two collector instances (S3.1 contract).
    let mut digests = Vec::new();
    for _ in 0..2 {
        let temp = TempDir::new().expect("TempDir");
        let session = temp.path().join("session");
        fs::create_dir_all(&session).expect("create session");
        let collector = collector_with_session(&session);
        let payload = json!({"topic": "plan.ready", "x": 42});
        collector.emit_policy_receipt(
            PolicyReceiptDecision::Accept,
            "plan.ready",
            Some("executor"),
            &["event_policy"],
            None,
            Some(&payload),
        );
        let row = &policy_receipt_rows(&session)[0];
        digests.push(row["fields"]["event_digest"].as_str().unwrap().to_string());
    }
    assert_eq!(digests[0], digests[1]);
    assert_eq!(digests[0].len(), 16);
}

#[test]
fn policy_receipt_field_cap_8kib() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    // Build a payload whose JSON `message` field is 50 KiB.
    // S3.4: even with a runaway upstream input the on-disk row
    // must NOT carry the full payload — only the digest +
    // bounded receipt fields. The per-field cap is enforced
    // inside the writer; here we verify the *invariant* the
    // caller cares about: the on-disk row stays small and no
    // field matches the original oversized input.
    let huge_message = "x".repeat(50 * 1024);
    let payload = json!({
        "topic": "plan.ready",
        "message": huge_message,
    });
    collector.emit_policy_receipt(
        PolicyReceiptDecision::Accept,
        "plan.ready",
        Some("executor"),
        &["event_policy"],
        None,
        Some(&payload),
    );

    // Read the raw line and check the on-disk byte length.
    let path = session.join("runtime-trace.jsonl");
    let contents = fs::read_to_string(&path).expect("read runtime-trace.jsonl");
    let line = contents
        .lines()
        .find(|l| l.contains("\"policy_receipt\""))
        .expect("policy_receipt line present");
    let line_bytes = line.len();
    assert!(
        line_bytes < 8 * 1024,
        "policy_receipt row exceeded 8 KiB ({line_bytes} bytes); the 50 KiB upstream field must NOT be copied verbatim (S3.4)"
    );
    // The huge `message` field must not appear verbatim in the
    // receipt row (the receipt only carries the digest).
    assert!(
        !line.contains(&"x".repeat(50 * 1024)),
        "policy_receipt row leaked the upstream 50 KiB field (S3.4 violated)"
    );
    // Even a partial repeat must be capped well below the
    // original 50 KiB; one hundred 'x' chars is plenty for a
    // smoke check that no truncated copy snuck through.
    assert!(
        !line.contains(&"x".repeat(100)),
        "policy_receipt row carried a long repeated substring (>=100 chars); the upstream field was not bounded (S3.4 violated)"
    );
}

#[test]
fn policy_receipt_rejects_with_unknown_hat_use_unknown_key() {
    // S3.3: when the source hat is unknown, retry_key uses the
    // sentinel `unknown` (matches RejectionRecord::retry_key
    // which calls `hat.unwrap_or("unknown")`).
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    let reason_code = "origin:unknown hat rejected".to_string();
    collector.emit_policy_receipt(
        PolicyReceiptDecision::Reject,
        "plan.ready",
        None,
        &["origin_guard"],
        Some(&reason_code),
        None,
    );

    let rows = policy_receipt_rows(&session);
    let fields = &rows[0]["fields"];
    let retry_key = fields["retry_key"].as_str().expect("retry_key");
    assert!(
        retry_key.starts_with("unknown:"),
        "missing source_hat must serialise to `unknown:` prefix (got: {retry_key:?})"
    );
    assert!(
        fields.get("hat").is_none(),
        "row must not invent a hat when none was given"
    );
}

#[test]
fn policy_receipt_contract_digest_absent_until_contract_receipt_emitted() {
    // Edge case: a fresh collector with no prior
    // `emit_contract_receipt` call must still produce a valid
    // policy_receipt row — the `contract_digest` field is simply
    // absent (not a phantom empty string), so downstream readers
    // can distinguish "no contract yet" from "contract_digest=0".
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.emit_policy_receipt(
        PolicyReceiptDecision::Accept,
        "plan.ready",
        Some("executor"),
        &["event_policy"],
        None,
        Some(&json!({"topic": "plan.ready"})),
    );

    let rows = policy_receipt_rows(&session);
    assert_eq!(rows.len(), 1);
    let fields = &rows[0]["fields"];
    assert!(
        fields.get("contract_digest").is_none(),
        "no contract_digest must be written when emit_contract_receipt has not run yet"
    );
}

#[test]
fn policy_receipt_phase_and_kind_match_decision_phase() {
    // Belt-and-braces: the receipt row lives under phase=decision
    // (matching U02's contract_receipt) so consumers can pull
    // the entire receipt stream with a single phase filter.
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.emit_policy_receipt(
        PolicyReceiptDecision::Accept,
        "plan.ready",
        Some("executor"),
        &["event_policy"],
        None,
        Some(&json!({"x": 1})),
    );

    for entry in read_trace_lines(&session) {
        if entry.get("kind").and_then(|k| k.as_str()) == Some("policy_receipt") {
            assert_eq!(entry["phase"], json!("decision"));
        }
    }
    // Confirm the runtime_trace enum is still exhaustive on the
    // helper-side construction path.
    let direct = RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Decision);
    assert_eq!(direct.kind, "decision");
}

#[test]
fn policy_receipt_idempotent_under_repeated_emit() {
    // Per-event receipts ARE NOT idempotent (one row per event
    // decision, by design). This test pins that contract: two
    // emits produce two rows so the attribution engine can count
    // decisions; the same was NOT true for `contract_receipt`
    // (U02 enforced exactly one per session).
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.emit_policy_receipt(
        PolicyReceiptDecision::Accept,
        "plan.ready",
        Some("executor"),
        &["event_policy"],
        None,
        Some(&json!({"topic": "plan.ready"})),
    );
    collector.emit_policy_receipt(
        PolicyReceiptDecision::Accept,
        "plan.ready",
        Some("executor"),
        &["event_policy"],
        None,
        Some(&json!({"topic": "plan.ready"})),
    );

    let rows = policy_receipt_rows(&session);
    assert_eq!(
        rows.len(),
        2,
        "policy_receipt is per-event; no idempotency latch"
    );
    // Sequence numbers must be strictly monotonic.
    let seq0 = rows[0]["sequence"].as_u64().unwrap();
    let seq1 = rows[1]["sequence"].as_u64().unwrap();
    assert!(
        seq0 < seq1,
        "policy_receipt rows must have strictly monotonic sequences (got {seq0}, {seq1})"
    );
    // Reporter summary still confirms no malformed lines.
    let report = read_runtime_trace_report(&session);
    assert_eq!(report.malformed_lines, 0);
}
