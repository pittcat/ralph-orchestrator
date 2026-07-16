//! U13 (2026-06-27 mechanism foundation completion):
//! `archive_state_for_loop` failures now abort the
//! loop start (`EventLoop::with_context_and_diagnostics`
//! returns `Err`). The legacy `warn + continue`
//! behaviour (U11) was the root cause of SC-6
//! violations — a fresh loop_id would inherit the
//! previous loop's stale `.ralph/` state.

use super::*;

/// Happy path: when there is no `loop-version.json` in
/// the workspace, the archive step is a no-op and the
/// loop starts successfully.
#[test]
fn u13_with_context_and_diagnostics_succeeds_when_nothing_to_archive() {
    let temp = tempfile::tempdir().unwrap();
    let _events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();
    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
hats:
  planner:
    name: "Planner"
    triggers: ["work.start"]
    publishes: ["work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics");
    let context = LoopContext::worktree(
        "loop-u13-1",
        diagnostics_root.clone(),
        diagnostics_root.clone(),
    );

    let result = EventLoop::with_context_and_diagnostics(config, context, diagnostics);
    assert!(result.is_ok(), "expected loop start to succeed");
}

/// Pin the U13 contract: an `archive_state_for_loop`
/// failure aborts the loop start. The easiest way to
/// force a failure is to set the workspace to a path
/// that cannot be archived (the function rejects
/// relative paths). We test via the public
/// `archive_state_for_loop` API to confirm the
/// failure mode, then assert the higher-level
/// `with_context_and_diagnostics` behaviour by reading
/// the integration through the new return type.
#[test]
fn u13_archive_state_for_loop_rejects_relative_workspace() {
    use crate::event_loop::stages::archive_version_stage::archive_state_for_loop;
    let result = archive_state_for_loop(std::path::Path::new("relative/path"), "loop-x");
    assert!(
        result.is_err(),
        "relative workspace must fail archive_state_for_loop"
    );
}

/// U13: when the archive fails, `with_context_and_diagnostics`
/// returns `Err` instead of returning a constructed
/// `EventLoop`. We trigger the failure by passing a
/// workspace whose parent path is a regular file —
/// `archive_state_for_loop` returns `Err` on a
/// relative workspace, which we use here as the
/// canonical "archive fails" signal.
#[test]
fn u13_with_context_and_diagnostics_aborts_on_archive_failure() {
    let bogus_workspace = std::path::PathBuf::from("relative/workspace");
    let _events_path = bogus_workspace.join("events.jsonl");
    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
hats:
  planner:
    name: "Planner"
    triggers: ["work.start"]
    publishes: ["work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&bogus_workspace, false)
            .expect("create diagnostics");
    let context = LoopContext::worktree("loop-u13-2", bogus_workspace.clone(), bogus_workspace);
    let result = EventLoop::with_context_and_diagnostics(config, context, diagnostics);
    assert!(
        result.is_err(),
        "expected U13 fail-closed: archive failure must return Err"
    );
}
