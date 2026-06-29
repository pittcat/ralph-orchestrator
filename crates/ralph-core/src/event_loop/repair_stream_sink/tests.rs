//! U6 tests for `RepairStreamSink` (and its free
//! function form `record_repair_event`). The four
//! pinned scenarios:
//!
//! 1. Happy path: writing one repair event appends the
//!    expected envelope line to `<workspace>/recovery.jsonl`.
//! 2. Edge case: two writes to the same workspace append
//!    two lines (no truncation).
//! 3. Error path: a read-only workspace returns `Err`
//!    (the FS error is surfaced to the caller, not
//!    silently swallowed).
//! 4. Pin: the envelope's `reason_code` is the stable
//!    `repair_dispatch` string so `ralph diagnose` can
//!    attribute the record.

use super::{REPAIR_SINK_REASON_CODE, RepairStreamSink, record_repair_event};
use ralph_proto::Event;

fn ev(topic: &str, payload: &str) -> Event {
    Event::new(topic, payload)
}

#[test]
fn u6_sink_writes_one_envelope_per_event() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path();
    let event = ev("task.relocate_legacy", r#"{"task_key":"legacy-1"}"#);

    RepairStreamSink::new()
        .record(&event, workspace)
        .expect("first write must succeed");

    let path = workspace.join("recovery.jsonl");
    let content = std::fs::read_to_string(&path).expect("read recovery.jsonl");
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "expected exactly one line, got {lines:?}");
    assert!(
        lines[0].contains(REPAIR_SINK_REASON_CODE),
        "envelope must carry reason_code={REPAIR_SINK_REASON_CODE}, got: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("task.relocate_legacy"),
        "envelope must mention the topic, got: {}",
        lines[0]
    );
}

#[test]
fn u6_sink_appends_two_lines_for_two_events() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path();

    record_repair_event(
        &ev("task.relocate_legacy", r#"{"task_key":"a"}"#),
        workspace,
    )
    .expect("first write");
    record_repair_event(&ev("repair.close", r#"{"task_key":"a"}"#), workspace)
        .expect("second write");

    let content =
        std::fs::read_to_string(workspace.join("recovery.jsonl")).expect("read recovery.jsonl");
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected two lines, got {lines:?}");
    assert!(lines[0].contains("task.relocate_legacy"));
    assert!(lines[1].contains("repair.close"));
}

#[test]
fn u6_sink_returns_io_error_on_read_only_workspace() {
    // Use a non-existent parent path so `create_dir_all`
    // fails: the workspace's grandparent is a file, not
    // a directory, so the sink cannot create the leaf
    // directory.
    let temp = tempfile::tempdir().unwrap();
    let blocking_file = temp.path().join("blocker");
    std::fs::write(&blocking_file, "blocker").unwrap();
    let bogus_workspace = blocking_file.join("recovery");
    let event = ev("task.relocate_legacy", r#"{"task_key":"x"}"#);

    let result = record_repair_event(&event, &bogus_workspace);
    assert!(
        result.is_err(),
        "expected I/O error when workspace parent is a file, got: {result:?}"
    );
}

#[test]
fn u6_sink_envelope_carries_stable_reason_code() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path();
    let event = ev("task.relocate_legacy", r#"{"task_key":"k"}"#);
    record_repair_event(&event, workspace).expect("write");
    let content = std::fs::read_to_string(workspace.join("recovery.jsonl")).unwrap();
    // Pin the contract: the reason_code field is the
    // stable `repair_dispatch` string. Any drift here
    // breaks `ralph diagnose`'s attribution.
    assert!(
        content.contains("\"reason_code\":\"repair_dispatch\""),
        "expected stable reason_code, got: {content}"
    );
}
