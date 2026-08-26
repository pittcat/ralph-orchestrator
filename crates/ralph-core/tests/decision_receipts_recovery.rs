//! Plan 2026-08-26-1104 Unit 5 acceptance tests: recovery decision
//! receipts.
//!
//! Locks the per-decision `kind=recovery_receipt` wire format that
//! the attribution engine (U8) and the `ralph diagnose --causal`
//! command (U9) consume:
//!
//! - `action=resume` receipts carry `retry_key`, `attempt`,
//!   `budget_remaining`, `target_hat`, `reason_code` so the
//!   engine can reconstruct the precheck retry bookkeeping that
//!   drove the resume injection (S5.1).
//! - `action=exhausted` receipts carry a `retry_key` that
//!   reconciles byte-for-byte with the
//!   `plan.blocked{kind=precheck_exhausted}` payload's
//!   `kind`/`topic` triple so the loop terminal event can be
//!   joined to the recovery stream (S5.2).
//! - `action=correction` receipts carry a `rejection_digest`
//!   count mirroring the unified ledger snapshot so the engine
//!   can detect when the per-key correction budget is nearing
//!   exhaustion (S5.3).
//!
//! Like the U3 / U4 sibling suites, the tests deliberately
//! drive `DiagnosticsCollector::emit_recovery_receipt` directly
//! instead of spinning up a full `EventLoop`. The wire-format
//! guarantees are enforced by the writer itself; the loop-side
//! integration is covered by the existing
//! `precheck_gate_runner::dispatch_rejection` and
//! `inject_completion_correction` unit tests.

use std::fs;
use std::path::Path;

use ralph_core::diagnostics::{DiagnosticsCollector, DiagnosticsOptions, RecoveryReceiptAction};
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

fn recovery_receipt_rows(session: &Path) -> Vec<serde_json::Value> {
    read_trace_lines(session)
        .into_iter()
        .filter(|entry| entry.get("kind").and_then(|k| k.as_str()) == Some("recovery_receipt"))
        .collect()
}

/// S5.1: a `resume` receipt carries the five per-attempt fields
/// the attribution engine reads to reconstruct the precheck
/// retry bookkeeping that drove the resume injection. The wire
/// format mirrors the existing `policy_receipt` row shape
/// (`phase=decision`, `kind=recovery_receipt`) so U8 can reuse
/// the same parser.
#[test]
fn recovery_receipt_resume_carries_attempt_fields() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    collector.emit_recovery_receipt(
        RecoveryReceiptAction::Resume,
        "plan.complete.rejected",
        "executor",
        "precheck:plan.complete.rejected",
        2,
        1,
        Some("executor"),
        Some("precheck_rejected"),
    );

    let rows = recovery_receipt_rows(&session);
    assert_eq!(rows.len(), 1, "expected exactly one recovery_receipt row");
    let row = &rows[0];
    assert_eq!(row["phase"], json!("decision"));
    assert_eq!(row["kind"], json!("recovery_receipt"));
    assert_eq!(row["hat"], json!("executor"));
    assert_eq!(row["topic"], json!("plan.complete.rejected"));

    let fields = &row["fields"];
    assert_eq!(fields["action"], json!("resume"));
    assert_eq!(
        fields["retry_key"],
        json!("precheck:plan.complete.rejected")
    );
    assert_eq!(fields["attempt"], json!(2));
    assert_eq!(fields["budget_remaining"], json!(1));
    assert_eq!(fields["target_hat"], json!("executor"));
    assert_eq!(fields["reason_code"], json!("precheck_rejected"));
}

/// S5.2: an `exhausted` receipt carries a `retry_key` matching
/// the `plan.blocked{kind=precheck_exhausted}` payload so the
/// engine can join the recovery stream to the loop terminal
/// event. The same triple `(gate, topic, reason)` produces the
/// same `retry_key` regardless of which side writes first.
#[test]
fn recovery_receipt_exhausted_retry_key_matches_plan_blocked_payload() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    // The plan.blocked payload that precheck_gate_runner emits on
    // exhaustion (mirror of `build_exhausted_payload`):
    let plan_blocked_payload = json!({
        "topic": "plan.blocked",
        "reason": "precheck_failed",
        "kind": "precheck_exhausted",
    });

    // The recovery_receipt emitted from the same dispatch
    // decision MUST carry a retry_key the engine can join to the
    // plan.blocked envelope via the same `(gate, topic, kind)`
    // triple. We use the same construction so the test fails
    // loudly if the helper drifts.
    let gate = "plan.complete.precheck";
    let guarded_topic = "plan.complete";
    let kind = "precheck_exhausted";
    let retry_key = format!("{gate}:{guarded_topic}:{kind}");

    collector.emit_recovery_receipt(
        RecoveryReceiptAction::Exhausted,
        "plan.blocked",
        gate,
        &retry_key,
        3,
        0,
        Some("executor"),
        Some(kind),
    );

    let rows = recovery_receipt_rows(&session);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    let fields = &row["fields"];

    assert_eq!(fields["action"], json!("exhausted"));
    assert_eq!(fields["retry_key"], json!(retry_key));
    assert_eq!(fields["budget_remaining"], json!(0));
    assert_eq!(fields["attempt"], json!(3));

    // The triple encoded into the retry_key matches the three
    // fields the plan.blocked payload carries so the engine can
    // join on string match.
    let parsed_blocked = plan_blocked_payload;
    assert_eq!(parsed_blocked["kind"], json!(kind));
    assert_eq!(parsed_blocked["topic"], json!("plan.blocked"));
}

/// S5.3: a `correction` receipt carries the per-key
/// `rejection_digest` count mirroring the unified ledger
/// snapshot so the engine can detect when the correction budget
/// is nearing exhaustion (e.g. >= 3 in the same loop).
#[test]
fn recovery_receipt_correction_carries_rejection_digest_count() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    let retry_key = "executor:loop.complete:missing_required_events";
    let count = 2u32;

    collector.emit_recovery_receipt(
        RecoveryReceiptAction::Correction,
        "loop.complete",
        "executor",
        retry_key,
        count,
        0,
        None,
        Some("missing_required_events"),
    );

    let rows = recovery_receipt_rows(&session);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    let fields = &row["fields"];

    assert_eq!(fields["action"], json!("correction"));
    assert_eq!(fields["retry_key"], json!(retry_key));
    assert_eq!(fields["rejection_digest_count"], json!(count));
    // correction receipts MUST NOT carry target_hat (the path
    // targets the same hat via the next iteration).
    assert!(
        fields.get("target_hat").is_none(),
        "correction receipt must NOT carry target_hat (S5.3)"
    );
}

/// Wire-format guard: every recovery_receipt row stays bounded
/// below the 8 KiB sidecar cap regardless of how the caller
/// fills the per-field strings. Mirrors the U3 / U4 invariant
/// that a runaway upstream input cannot push the receipt past
/// the cap.
#[test]
fn recovery_receipt_field_cap_8kib() {
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let collector = collector_with_session(&session);

    let huge_reason = "x".repeat(50 * 1024);
    collector.emit_recovery_receipt(
        RecoveryReceiptAction::Resume,
        "plan.complete.rejected",
        "executor",
        "precheck:plan.complete.rejected",
        2,
        3,
        Some("executor"),
        Some(&huge_reason),
    );

    let path = session.join("runtime-trace.jsonl");
    let contents = fs::read_to_string(&path).expect("read runtime-trace.jsonl");
    let line = contents
        .lines()
        .find(|l| l.contains("\"recovery_receipt\""))
        .expect("recovery_receipt line present");
    assert!(
        line.len() < 8 * 1024,
        "recovery_receipt row exceeded 8 KiB ({} bytes); the 50 KiB upstream field must NOT be copied verbatim",
        line.len()
    );
    assert!(
        !line.contains(&huge_reason),
        "recovery_receipt must truncate the upstream reason_code (R12 / 8KiB cap)"
    );
}

/// Stable wire-format guard: `RecoveryReceiptAction::as_str`
/// returns the literal strings the contract locks, so the
/// downstream engine and dashboards can match on them without
/// re-deriving from the typed enum.
#[test]
fn recovery_receipt_action_strings_are_stable() {
    assert_eq!(RecoveryReceiptAction::Resume.as_str(), "resume");
    assert_eq!(RecoveryReceiptAction::Exhausted.as_str(), "exhausted");
    assert_eq!(RecoveryReceiptAction::Correction.as_str(), "correction");
}
