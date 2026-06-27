use super::*;
use crate::state::idempotent_log::IdempotentLog;
use serde_json::json;
use tempfile::TempDir;

fn log(workspace: &std::path::Path, loop_id: &str) -> IdempotentLog {
    IdempotentLog::open(workspace, loop_id).unwrap()
}

#[test]
fn wiring_write_task_rejects_empty_loop_id() {
    let dir = TempDir::new().unwrap();
    let mut lg = log(dir.path(), "loop-1");
    let err = write_task(&mut lg, "task-1", "", json!({}), false).unwrap_err();
    assert!(matches!(err, WiringError::MissingLoopId(ref t) if t == "task-1"));
}

#[test]
fn wiring_write_task_persists_record() {
    let dir = TempDir::new().unwrap();
    let mut lg = log(dir.path(), "loop-1");
    write_task(
        &mut lg,
        "task-1",
        "loop-1",
        json!({"status": "closed"}),
        true,
    )
    .unwrap();
    assert_eq!(lg.final_count(), 1);
}

#[test]
fn wiring_write_recovery_persists_record() {
    let dir = TempDir::new().unwrap();
    let mut lg = log(dir.path(), "loop-1");
    write_recovery(
        &mut lg,
        "retry-abc",
        "loop-1",
        json!({"reason": "stalled"}),
        false,
    )
    .unwrap();
    write_recovery(
        &mut lg,
        "retry-abc",
        "loop-1",
        json!({"reason": "stalled"}),
        true,
    )
    .unwrap();
    assert_eq!(lg.final_count(), 1);
}

#[test]
fn wiring_write_drift_persists_record() {
    let dir = TempDir::new().unwrap();
    let mut lg = log(dir.path(), "loop-1");
    write_drift(
        &mut lg,
        "missing-field-x",
        "loop-1",
        json!({"topic": "plan.blocked"}),
    )
    .unwrap();
    assert_eq!(lg.final_count(), 1);
}

#[test]
fn wiring_summary_counts_only_final_records() {
    let dir = TempDir::new().unwrap();
    let mut lg = log(dir.path(), "loop-1");
    write_task(&mut lg, "t1", "loop-1", json!({}), true).unwrap();
    write_task(&mut lg, "t2", "loop-1", json!({}), true).unwrap();
    write_task(&mut lg, "t3", "loop-1", json!({}), false).unwrap();
    write_recovery(&mut lg, "r1", "loop-1", json!({}), true).unwrap();
    write_drift(&mut lg, "f1", "loop-1", json!({})).unwrap();

    let finals = lg.final_records();
    let summary = DiagnosisSummary::from_final_records(&finals);
    assert_eq!(summary.task_count, 2);
    assert_eq!(summary.recovery_count, 1);
    assert_eq!(summary.drift_finding_count, 1);
}

#[test]
fn wiring_summary_distinguishes_keys_with_prefix() {
    // A key that happens to start with "task:" but is actually
    // a "task-relocate" record is still counted as a task —
    // the summary trusts the prefix contract.
    let records = vec![
        IdempotentRecord::new("task:foo:loop:l1").with_final(true),
        IdempotentRecord::new("recovery:bar:loop:l1").with_final(true),
        IdempotentRecord::new("drift:baz:loop:l1").with_final(true),
        IdempotentRecord::new("other-key:loop:l1").with_final(true),
    ];
    let summary = DiagnosisSummary::from_final_records(&records);
    assert_eq!(summary.task_count, 1);
    assert_eq!(summary.recovery_count, 1);
    assert_eq!(summary.drift_finding_count, 1);
}

#[test]
fn wiring_key_helpers_match_documented_format() {
    assert_eq!(task_key("t1", "loop-1"), "task:t1:loop:loop-1");
    assert_eq!(recovery_key("retry-1", "loop-1"), "recovery:retry-1:loop:loop-1");
    assert_eq!(drift_key("finding-1", "loop-1"), "drift:finding-1:loop:loop-1");
}