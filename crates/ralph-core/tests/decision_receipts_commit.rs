//! Plan 2026-08-26-1104 Unit 4 acceptance tests: state machine
//! commit receipt.
//!
//! Locks the per-projection `kind=commit_receipt` wire format that
//! the attribution engine (U8) and the `ralph diagnose --causal`
//! command (U9) consume:
//!
//! - Committed receipts carry `commit_status=committed` and a
//!   `transition_id` mirroring the `OutboxEntry.transition_id` so
//!   the engine can join the receipt back to the durable outbox row
//!   (S4.1).
//! - Rolled-back receipts carry `commit_status=rolled_back` plus a
//!   truncated `failure_reason` summary so operators can pinpoint
//!   the rollback cause without grepping the loop logs (S4.2).
//! - When an outbox entry exists for an accepted transition but no
//!   `commit_receipt` follows it, the (U8) attribution engine
//!   reports a runtime-domain fracture; this test pins the
//!   structural invariant the engine reads against (S4.3).
//! - Each row stays bounded: single field ≤ 8 KiB, no full event
//!   payload or projection delta is copied onto the wire (R12).
//!
//! The tests deliberately avoid spinning up a full `EventLoop` to
//! stay hermetic — they drive the production
//! `DiagnosticsCollector::emit_commit_receipt` API directly and
//! assert against the on-disk `runtime-trace.jsonl`.

use std::fs;
use std::path::Path;

use ralph_core::diagnostics::{
    CommitReceiptStatus, DiagnosticsCollector, DiagnosticsOptions, RuntimeTraceEntry,
    RuntimeTracePhase,
};
use ralph_core::event_loop::accepted_transition::AcceptedTransition;
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

fn commit_receipt_rows(session: &Path) -> Vec<serde_json::Value> {
    read_trace_lines(session)
        .into_iter()
        .filter(|entry| entry.get("kind").and_then(|k| k.as_str()) == Some("commit_receipt"))
        .collect()
}

/// S4.1: the `committed` receipt carries `commit_status=committed`
/// and `transition_id` exactly equal to the `OutboxEntry.transition_id`
/// that the disposition helper writes to the durable outbox. We
/// synthesise a stable transition_id via the production
/// `AcceptedTransition::compute_transition_id` helper so the
/// "what would have been written to the outbox" comparison is byte-
/// for-byte stable.
#[test]
fn commit_receipt_committed_matches_outbox_transition_id() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    // Stable contract digest so the receipt carries the matching
    // `contract_digest` field (mirror U3's policy_receipt pattern).
    collector.emit_contract_receipt(json!({
        "contract_digest": "digest-s41",
        "terminal_topics_digest": "tt-s41",
        "hats_digest": "h-s41",
        "preset_label": "test-preset",
    }));

    // What the outbox would record for this accepted transition.
    let loop_id = "loop-s41";
    let activation_id = "act-s41";
    let contract_revision = "rev-s41";
    let event_identity = "topic=plan.ready|payload={\"plan_id\":\"p-41\"}";
    let canonical_digest = "canonical-s41";
    let outbox_transition_id = AcceptedTransition::compute_transition_id(
        loop_id,
        activation_id,
        contract_revision,
        event_identity,
        canonical_digest,
    );

    collector.emit_commit_receipt(
        CommitReceiptStatus::Committed,
        &outbox_transition_id,
        "plan.ready",
        None,
    );

    let rows = commit_receipt_rows(&session);
    assert_eq!(rows.len(), 1, "expected exactly one commit_receipt row");
    let row = &rows[0];
    assert_eq!(row["phase"], json!("decision"));
    assert_eq!(row["kind"], json!("commit_receipt"));
    assert_eq!(row["topic"], json!("plan.ready"));

    let fields = &row["fields"];
    assert_eq!(fields["commit_status"], json!("committed"));
    assert_eq!(
        fields["transition_id"],
        json!(outbox_transition_id),
        "fields.transition_id must equal OutboxEntry.transition_id byte-for-byte (S4.1)"
    );
    assert_eq!(fields["topic"], json!("plan.ready"));
    assert_eq!(
        fields["contract_digest"],
        json!("digest-s41"),
        "contract_digest must come from the cache populated by emit_contract_receipt"
    );
    // S4.1 only mandates commit_status + transition_id for the
    // committed branch; failure_reason must be absent (not a
    // phantom empty string).
    assert!(
        fields.get("failure_reason").is_none(),
        "committed receipt must NOT carry failure_reason (got: {:?})",
        fields.get("failure_reason")
    );
}

/// S4.2: the `rolled_back` receipt carries `commit_status=rolled_back`
/// and a bounded `failure_reason` summary. The transition_id field
/// is still present so the U8 engine can join the rolled_back row
/// back to the projection that was rolled back.
#[test]
fn commit_receipt_rolled_back_carries_failure_reason() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.emit_contract_receipt(json!({
        "contract_digest": "digest-s42",
        "terminal_topics_digest": "tt-s42",
        "hats_digest": "h-s42",
        "preset_label": "test-preset",
    }));

    // Outbox write never happened on the rolled_back path; the
    // transition_id still belongs to the projection so the engine
    // can join the rolled_back row back to the projection.
    let projection_transition_id = AcceptedTransition::compute_transition_id(
        "loop-s42",
        "act-s42",
        "rev-s42",
        "topic=plan.ready|payload={\"plan_id\":\"p-42\"}",
        "canonical-s42",
    );
    let failure_summary = "state machine projection commit failed for ts: ledger io error";

    collector.emit_commit_receipt(
        CommitReceiptStatus::RolledBack,
        &projection_transition_id,
        "plan.ready",
        Some(failure_summary),
    );

    let rows = commit_receipt_rows(&session);
    assert_eq!(rows.len(), 1);
    let fields = &rows[0]["fields"];
    assert_eq!(fields["commit_status"], json!("rolled_back"));
    assert_eq!(
        fields["transition_id"],
        json!(projection_transition_id),
        "rolled_back receipt must still carry the projection's transition_id"
    );
    assert_eq!(fields["topic"], json!("plan.ready"));
    assert_eq!(
        fields["failure_reason"],
        json!(failure_summary),
        "rolled_back receipt must carry the underlying failure summary (S4.2)"
    );
}

/// S4.3: when an outbox entry exists for an accepted transition but
/// no `commit_receipt` follows it, the U8 attribution engine reports
/// a runtime-domain fracture. Here we verify the structural
/// invariant the engine reads against: the `commit_receipt` stream
/// for a given `transition_id` can be queried independently from
/// the outbox stream, so a missing row is detectable by string
/// match across the two files (no in-memory state required).
#[test]
fn commit_receipt_fracture_is_structurally_detectable() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    // Synthesise a transition_id and emit ZERO commit_receipt
    // rows for it. The structural test: with no receipt, the U8
    // engine can read the receipt stream and report the missing
    // counterpart against the corresponding outbox row (the
    // outbox row itself is the production's responsibility; here
    // we only verify the receipt stream is empty).
    let outbox_transition_id = AcceptedTransition::compute_transition_id(
        "loop-s43",
        "act-s43",
        "rev-s43",
        "topic=plan.ready|payload={\"plan_id\":\"p-43\"}",
        "canonical-s43",
    );
    let rows = commit_receipt_rows(&session);
    assert!(
        rows.is_empty(),
        "S4.3 setup: no commit_receipt rows must be present (engine will read 0 vs 1 outbox entry for transition_id={outbox_transition_id})"
    );

    // Now emit the rolled_back receipt to confirm the helper can
    // retroactively close the fracture for a recovered flow. The
    // engine will read the rolled_back row and treat the outbox
    // entry as "told-then-rolled-back" (no committed counterpart
    // is required for a rolled_back outcome).
    collector.emit_commit_receipt(
        CommitReceiptStatus::RolledBack,
        &outbox_transition_id,
        "plan.ready",
        Some("recovered after delay"),
    );
    let rows_after = commit_receipt_rows(&session);
    assert_eq!(rows_after.len(), 1);
    assert_eq!(
        rows_after[0]["fields"]["commit_status"],
        json!("rolled_back")
    );
    assert_eq!(
        rows_after[0]["fields"]["transition_id"],
        json!(outbox_transition_id),
        "rolled_back receipt's transition_id must match the projection/outbox transition_id so the engine can join them"
    );
}

/// R12 / S4.3: the on-disk row stays bounded (≤ 8 KiB) and never
/// carries the upstream 50 KiB field verbatim. The cap machinery
/// has two valid outcomes for a runaway field: (a) the field is
/// truncated with the `...[truncated]` suffix (when the field
/// alone is over the per-field cap but the rest of the row leaves
/// headroom), or (b) the field is dropped from the JSON object
/// when including any copy would still push the row past the cap.
/// Both outcomes preserve the bounded-row invariant the operator
/// cares about; we accept either here.
#[test]
fn commit_receipt_field_cap_8kib() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    // 50 KiB upstream failure_reason; the on-disk row must NOT
    // carry the full string verbatim.
    let huge_reason = "x".repeat(50 * 1024);
    collector.emit_commit_receipt(
        CommitReceiptStatus::RolledBack,
        "transition-s44",
        "plan.ready",
        Some(&huge_reason),
    );

    let path = session.join("runtime-trace.jsonl");
    let contents = fs::read_to_string(&path).expect("read runtime-trace.jsonl");
    let line = contents
        .lines()
        .find(|l| l.contains("\"commit_receipt\""))
        .expect("commit_receipt line present");
    let line_bytes = line.len();
    assert!(
        line_bytes < 8 * 1024,
        "commit_receipt row exceeded 8 KiB ({line_bytes} bytes); the 50 KiB upstream field must NOT be copied verbatim (R12 violated)"
    );
    assert!(
        !line.contains(&"x".repeat(50 * 1024)),
        "commit_receipt row leaked the upstream 50 KiB failure_reason verbatim (R12 violated)"
    );
    // Even a long repeated substring (>=100 chars) must not
    // appear — the cap machinery either truncates with
    // `...[truncated]` (per-field cap) or drops the key entirely
    // (object-level cap). No partial verbatim copy is allowed.
    assert!(
        !line.contains(&"x".repeat(100)),
        "commit_receipt row carried a long repeated substring (>=100 chars); the upstream field was not bounded (R12 violated): {line}"
    );
    // The commit_status field must survive either outcome so the
    // U8 engine can still see that a commit (vs. a hypothetical
    // dropped row) actually happened.
    assert!(
        line.contains("\"commit_status\":\"rolled_back\""),
        "commit_status must survive the cap so the engine can still classify the receipt (got: {line})"
    );
}

/// `commit_receipt` rows share the same phase as every other
/// decision receipt (contract_receipt, policy_receipt) so the U8
/// attribution engine can pull the entire decision stream with one
/// `phase=decision` filter.
#[test]
fn commit_receipt_phase_and_kind_match_decision_phase() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.emit_commit_receipt(
        CommitReceiptStatus::Committed,
        "transition-s45",
        "plan.ready",
        None,
    );

    for entry in read_trace_lines(&session) {
        if entry.get("kind").and_then(|k| k.as_str()) == Some("commit_receipt") {
            assert_eq!(entry["phase"], json!("decision"));
        }
    }
    // Confirm the runtime_trace enum is still exhaustive on the
    // helper-side construction path.
    let direct = RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Decision);
    assert_eq!(direct.kind, "decision");
}

/// Per-projection receipts are NOT idempotent: each accepted /
/// rolled-back projection produces exactly one row so the U8 engine
/// can count commits + rollbacks. Sequence numbers must be strictly
/// monotonic across the entire session.
#[test]
fn commit_receipt_rows_have_monotonic_sequences() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.emit_commit_receipt(
        CommitReceiptStatus::Committed,
        "transition-s46-1",
        "plan.ready",
        None,
    );
    collector.emit_commit_receipt(
        CommitReceiptStatus::RolledBack,
        "transition-s46-2",
        "plan.ready",
        Some("ledger io error"),
    );
    collector.emit_commit_receipt(
        CommitReceiptStatus::Committed,
        "transition-s46-3",
        "exec.unit.done",
        None,
    );

    let rows = commit_receipt_rows(&session);
    assert_eq!(
        rows.len(),
        3,
        "commit_receipt is per-projection: each emit produces exactly one row (got: {})",
        rows.len()
    );
    let statuses: Vec<&str> = rows
        .iter()
        .map(|r| r["fields"]["commit_status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        vec!["committed", "rolled_back", "committed"],
        "commit_status values must mirror the helper call order"
    );
    let seq0 = rows[0]["sequence"].as_u64().unwrap();
    let seq1 = rows[1]["sequence"].as_u64().unwrap();
    let seq2 = rows[2]["sequence"].as_u64().unwrap();
    assert!(
        seq0 < seq1 && seq1 < seq2,
        "commit_receipt rows must have strictly monotonic sequences (got {seq0}, {seq1}, {seq2})"
    );
}

/// `commit_receipt` does NOT require a prior `emit_contract_receipt`
/// call. Without one, the row simply omits `contract_digest` (not a
/// phantom empty string) so downstream readers can distinguish
/// "no contract yet" from "contract_digest=0".
#[test]
fn commit_receipt_contract_digest_absent_until_contract_receipt_emitted() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.emit_commit_receipt(
        CommitReceiptStatus::Committed,
        "transition-s47",
        "plan.ready",
        None,
    );

    let rows = commit_receipt_rows(&session);
    assert_eq!(rows.len(), 1);
    let fields = &rows[0]["fields"];
    assert!(
        fields.get("contract_digest").is_none(),
        "no contract_digest must be written when emit_contract_receipt has not run yet"
    );
}

/// `CommitReceiptStatus::as_str` returns the literal wire string the
/// receipt row uses; this pins the public contract so dashboards
/// matching on the literal cannot silently regress to e.g.
/// `"rolled-back"` (hyphen) or `"rollback"` (missing past tense).
#[test]
fn commit_receipt_status_as_str_is_stable() {
    assert_eq!(CommitReceiptStatus::Committed.as_str(), "committed");
    assert_eq!(CommitReceiptStatus::RolledBack.as_str(), "rolled_back");
}
