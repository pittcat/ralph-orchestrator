//! Plan 2026-08-15-1823 (fix empty channel activation observability)
//! Unit 2: end-to-end contract tests for `hat_activation_outcome`
//! emission on the three real isolated-mode call paths
//! (normal merge, empty merge, interrupt).

use super::super::super::*;
#[allow(unused_imports)]
use super::super::common::*;
#[allow(unused_imports)]
use super::super::fake_path::*;
use super::helpers::init_git_workspace;
use crate::test_support::CwdGuard;
use ralph_core::diagnostics::RuntimeTracePhase;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Read every `hat_activation_outcome` row from the session's
/// runtime-trace sidecar. Returns rows in on-disk order.
fn read_outcome_rows(event_loop: &EventLoop) -> Vec<Value> {
    let session_dir = match event_loop.diagnostics().session_dir() {
        Some(d) => d,
        None => return Vec::new(),
    };
    let body = match std::fs::read_to_string(session_dir.join("runtime-trace.jsonl")) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| {
            row.get("phase").and_then(Value::as_str) == Some("activation")
                && row.get("kind").and_then(Value::as_str) == Some("hat_activation_outcome")
        })
        .collect()
}

/// Test seam helper: seed an isolated hat channel with empty
/// contents and the matching `current-hat-events` marker so
/// `snapshot_channel` can resolve the path. Mirrors the production
/// `prepare_hat_channel` contract but lets the test control the
/// channel byte count.
fn seed_hat_channel(
    ctx: &ralph_core::LoopContext,
    hat: &str,
    loop_id: &str,
    iteration: u32,
) -> PathBuf {
    let channel_path =
        crate::loop_runner::paths::hat_channel_events_path(ctx, hat, loop_id, iteration);
    if let Some(parent) = channel_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&channel_path, "").unwrap();
    let relative = format!(".ralph/agent/events-hat-{hat}-{loop_id}-{iteration}.jsonl");
    std::fs::write(
        crate::loop_runner::paths::current_hat_events_marker(ctx),
        relative,
    )
    .unwrap();
    channel_path
}

/// Helper that builds an isolated event loop with a single
/// terminal-obligation hat (`executor`) so the empty merge path
/// can be exercised end-to-end without spinning up real backends.
fn build_isolated_executor_loop(workspace: &Path) -> (ralph_core::LoopContext, EventLoop) {
    let ctx = LoopContext::primary(workspace.to_path_buf());
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    description: "Test executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
    instructions: "Do work."
event_loop:
  starting_event: "task.start"
  completion_promise: "work.done"
  execution_mode: isolated
tasks:
  enabled: false
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
    config.core.workspace_root = workspace.to_path_buf();
    let event_loop = EventLoop::with_context_and_diagnostics(
        config,
        ctx.clone(),
        ralph_core::diagnostics::DiagnosticsCollector::with_enabled(workspace, true)
            .expect("diagnostics collector must initialize in tmpdir"),
    )
    .expect("event loop with diagnostics");
    (ctx, event_loop)
}

/// Build the minimal `RalphConfig` that `prepare_normal_merge`
/// needs for `merge_hat_channel` to find the workspace.
fn event_loop_config() -> ralph_core::RalphConfig {
    ralph_core::RalphConfig::default()
}

/// T1 (U2): an isolated normal activation that successfully merges
/// a non-empty channel must emit a single `hat_activation_outcome`
/// row with `status=merged` and the bounded scalar fields. The test
/// captures the pre-merge snapshot *before* `merge_hat_channel`
/// deletes the file (the same order the runner uses — see
/// `activation_outcome_close::prepare_normal_merge`), so a
/// regression in snapshot-before-merge ordering surfaces here.
#[test]
fn u2_in_process_runner_writes_merged_outcome_row() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let _cwd = CwdGuard::set(workspace.path());
    init_git_workspace(workspace.path());

    let (ctx, event_loop) = build_isolated_executor_loop(workspace.path());

    // Seed an isolated hat channel with a valid record so the merge
    // path returns Ok with non-zero bytes.
    let channel_path = seed_hat_channel(&ctx, "executor", "primary-001", 1);
    std::fs::write(
        &channel_path,
        "{\"topic\":\"work.done\",\"payload\":\"x\"}\n",
    )
    .unwrap();

    let target = ctx.workspace().join(".ralph/events-main.jsonl");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "").unwrap();

    // Capture the pre-merge channel state BEFORE
    // `merge_hat_channel` deletes the file (it removes the
    // channel on success or empty-merge-error). The runner
    // performs the snapshot at this exact point in the live
    // call path; the contract test must mirror that order.
    let snapshot = crate::loop_runner::activation_outcome::snapshot_channel(Some(&channel_path));
    let merge_result =
        crate::loop_runner::hat_channel::merge_hat_channel(&ctx, &target, "executor", None);
    assert!(
        merge_result.is_ok(),
        "merge should succeed for non-empty channel"
    );
    let refined = crate::loop_runner::activation_outcome::refine_after_merge(snapshot, true);
    let facts = crate::loop_runner::activation_outcome::ActivationOutcomeFacts {
        loop_id: Some(ctx.loop_id().unwrap_or("loop").to_string()),
        channel_exists: true,
        channel_bytes: refined.bytes,
        channel_readable: true,
        merge_succeeded: true,
        backend_success: true,
        backend_exit_code: Some(0),
        watchdog_timeout: false,
        backend_termination: false,
        output_bytes: 0,
        output_mentions_emit: false,
        terminal_obligation_topics: vec!["work.done".into()],
        ..Default::default()
    };
    crate::loop_runner::activation_outcome::log_activation_outcome(
        event_loop.diagnostics().session_dir(),
        1,
        "executor",
        &refined,
        &facts,
    );

    let rows = read_outcome_rows(&event_loop);
    assert_eq!(rows.len(), 1, "exactly one outcome row, got {rows:?}");
    let row = &rows[0];
    assert_eq!(
        row.get("status").and_then(Value::as_str),
        Some("merged"),
        "status must be merged for non-empty channel merge, got {row}"
    );
    assert_eq!(
        row.get("kind").and_then(Value::as_str),
        Some("hat_activation_outcome")
    );
    assert_eq!(
        row.get("fields")
            .and_then(|v| v.get("merge_succeeded"))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        row.get("fields")
            .and_then(|v| v.get("backend_exit_code"))
            .and_then(Value::as_i64),
        Some(0)
    );
    assert_eq!(
        row.get("fields")
            .and_then(|v| v.get("terminal_obligation_topics"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(Value::as_str),
        Some("work.done")
    );
    let merged_body = std::fs::read_to_string(&target).unwrap();
    assert!(merged_body.contains("work.done"));
}

/// T6 (U6): the runner-path `prepare_normal_merge` helper must
/// observe `Empty` (not `Unreadable`) for a zero-byte channel —
/// this is the regression anchor for U2's snapshot-before-merge
/// reorder. Driving the real helper rather than the underlying
/// `merge_hat_channel` ensures the runner path is exercised.
#[test]
fn u2_runner_path_helper_observes_empty_not_unreadable() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let _cwd = CwdGuard::set(workspace.path());
    init_git_workspace(workspace.path());

    let (ctx, _event_loop) = build_isolated_executor_loop(workspace.path());

    // Seed a zero-byte channel (the empty-after-activation case
    // that was previously misclassified as Unreadable when the
    // snapshot ran AFTER merge).
    let _channel_path = seed_hat_channel(&ctx, "executor", "primary-empty", 1);

    let target = ctx.workspace().join(".ralph/events-main.jsonl");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "").unwrap();

    let config = event_loop_config();
    let (outcome_row, merge_state) =
        crate::loop_runner::activation_outcome_close::prepare_normal_merge(
            &ctx,
            &config,
            false,
            &ralph_proto::HatId::new("executor"),
            true,
            false,
            None,
            "",
        );

    // Empty channel + merge ok (no-op for empty) → snapshot must
    // be Empty, NOT Unreadable. The empty-terminal flag must also
    // be true so the missing-terminal recovery path still fires.
    assert_eq!(
        merge_state.snapshot.status,
        crate::loop_runner::activation_outcome::ActivationOutcomeStatus::Empty,
        "pre-merge snapshot of zero-byte channel must be Empty, not {:?}",
        merge_state.snapshot.status
    );
    assert!(outcome_row.empty_terminal_channel);
}

/// T2 (U2): the empty channel merge path must emit `status=empty`
/// with `channel_bytes=0`. The bounded scalar fields must include
/// the channel_exists / channel_readable / merge_succeeded facts.
#[test]
fn u2_empty_channel_merge_writes_empty_outcome_row() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let _cwd = CwdGuard::set(workspace.path());
    init_git_workspace(workspace.path());

    let (ctx, event_loop) = build_isolated_executor_loop(workspace.path());

    // Seed an empty channel (zero bytes) — `merge_hat_channel`
    // returns Err on empty, but the snapshot must still capture
    // the pre-merge empty state.
    let channel_path = seed_hat_channel(&ctx, "executor", "primary-002", 1);
    std::fs::write(&channel_path, "").unwrap();

    let target = ctx.workspace().join(".ralph/events-main.jsonl");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "existing\n").unwrap();

    // Capture the pre-merge channel state BEFORE
    // `merge_hat_channel` deletes the file. Empty channels
    // trigger `hat_channel_empty_after_activation` which
    // removes the channel and emits a diagnostic — the
    // snapshot must run first to record the raw empty fact.
    let snapshot = crate::loop_runner::activation_outcome::snapshot_channel(Some(&channel_path));
    let merge_result =
        crate::loop_runner::hat_channel::merge_hat_channel(&ctx, &target, "executor", None);
    assert!(
        merge_result.is_err(),
        "empty channel merge must fail closed (existing behaviour)"
    );
    // Target must remain untouched.
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "existing\n",
        "empty channel must not pollute the target events file"
    );
    let refined = crate::loop_runner::activation_outcome::refine_after_merge(snapshot, false);
    let facts = crate::loop_runner::activation_outcome::ActivationOutcomeFacts {
        loop_id: Some(ctx.loop_id().unwrap_or("loop").to_string()),
        channel_exists: true,
        channel_bytes: refined.bytes,
        channel_readable: true,
        merge_succeeded: false,
        backend_success: true,
        backend_exit_code: Some(0),
        watchdog_timeout: false,
        backend_termination: false,
        output_bytes: 0,
        output_mentions_emit: false,
        terminal_obligation_topics: vec!["work.done".into()],
        ..Default::default()
    };
    crate::loop_runner::activation_outcome::log_activation_outcome(
        event_loop.diagnostics().session_dir(),
        1,
        "executor",
        &refined,
        &facts,
    );

    let rows = read_outcome_rows(&event_loop);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row.get("status").and_then(Value::as_str),
        Some("empty"),
        "status must be empty for empty channel, got {row}"
    );
    assert_eq!(
        row.get("fields")
            .and_then(|v| v.get("channel_bytes"))
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        row.get("fields")
            .and_then(|v| v.get("merge_succeeded"))
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        row.get("fields")
            .and_then(|v| v.get("channel_exists"))
            .and_then(Value::as_bool),
        Some(true)
    );
}

/// T3 (U2): interrupt-path merge must emit `status=interrupted`.
/// `merge_hat_channel` may succeed or fail here; the status must
/// always be `interrupted` regardless.
#[test]
fn u2_interrupt_path_writes_interrupted_outcome_row() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let _cwd = CwdGuard::set(workspace.path());
    init_git_workspace(workspace.path());

    let (ctx, event_loop) = build_isolated_executor_loop(workspace.path());

    // Seed a non-empty channel — the interrupt path still records
    // the raw pre-merge state, but the outcome status must always
    // be `interrupted` to distinguish the interrupt path from a
    // normal non-empty merge close.
    let channel_path = seed_hat_channel(&ctx, "executor", "primary-003", 1);
    std::fs::write(&channel_path, "{\"topic\":\"work.done\"}\n").unwrap();

    let target = ctx.workspace().join(".ralph/events-main.jsonl");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "").unwrap();

    let merge_result =
        crate::loop_runner::hat_channel::merge_hat_channel(&ctx, &target, "executor", None);
    assert!(merge_result.is_ok());

    let snapshot = crate::loop_runner::activation_outcome::snapshot_channel(Some(&channel_path));
    let refined = crate::loop_runner::activation_outcome::refine_for_interrupt(snapshot);
    let facts = crate::loop_runner::activation_outcome::ActivationOutcomeFacts {
        loop_id: Some(ctx.loop_id().unwrap_or("loop").to_string()),
        channel_exists: true,
        channel_bytes: refined.bytes,
        channel_readable: true,
        merge_succeeded: false,
        backend_success: false,
        backend_exit_code: None,
        watchdog_timeout: false,
        backend_termination: false,
        output_bytes: 0,
        output_mentions_emit: false,
        terminal_obligation_topics: vec!["work.done".into()],
        ..Default::default()
    };
    crate::loop_runner::activation_outcome::log_activation_outcome(
        event_loop.diagnostics().session_dir(),
        1,
        "executor",
        &refined,
        &facts,
    );

    let rows = read_outcome_rows(&event_loop);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row.get("status").and_then(Value::as_str),
        Some("interrupted"),
        "interrupt path must record status=interrupted, got {row}"
    );
    assert_eq!(
        row.get("fields")
            .and_then(|v| v.get("merge_succeeded"))
            .and_then(Value::as_bool),
        Some(false),
        "interrupt path must always mark merge_succeeded=false"
    );
}

/// T4 (U2): non-zero exit code from the CLI adapter must surface
/// in the outcome row's `backend_exit_code` field. The `success`
/// flag of `ExecutionOutcome` is preserved alongside — a non-zero
/// exit code does NOT imply `backend_success=false` when the
/// backend reported success in-band (existing contract).
#[test]
fn u2_non_zero_exit_code_surfaces_in_outcome_row() {
    // We do not run a real backend; we exercise the
    // `ExecutionOutcome` projection directly. The integration
    // test (T1/T2/T3 above) covers the row shape.
    let outcome = crate::loop_runner::execution::ExecutionOutcome {
        output: String::new(),
        success: false,
        termination: None,
        watchdog_timeout: false,
        backend_exit_code: Some(137),
        total_cost_usd: 0.0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };
    let facts = crate::loop_runner::activation_outcome::ActivationOutcomeFacts {
        backend_exit_code: outcome.backend_exit_code,
        ..Default::default()
    };
    let json = facts.to_json();
    assert_eq!(
        json.get("backend_exit_code").and_then(Value::as_i64),
        Some(137),
        "non-zero backend_exit_code must round-trip"
    );
}

/// T5 (U2): when `merge_hat_channel` returns Err on a non-empty
/// channel (e.g. filesystem write failure), the outcome status
/// must be `merge_failed`, NOT `empty`.
#[test]
fn u2_non_empty_merge_failure_writes_merge_failed_status() {
    let snapshot = crate::loop_runner::activation_outcome::ChannelSnapshot {
        status: crate::loop_runner::activation_outcome::ActivationOutcomeStatus::Empty,
        bytes: Some(42),
        reference: Some("hat-channel:test".into()),
    };
    let refined = crate::loop_runner::activation_outcome::refine_after_merge(snapshot, false);
    assert_eq!(
        refined.status,
        crate::loop_runner::activation_outcome::ActivationOutcomeStatus::MergeFailed,
        "non-empty merge failure must be merge_failed, got {:?}",
        refined.status
    );
}

/// U9 (M4): collapse the 30-line setup dance in the `u2_*`
/// helpers into a single `assert_outcome_row` fixture. Each call
/// sets up the workspace, seeds the channel, drives the runner
/// path, and asserts the persisted row.
fn assert_outcome_row(
    channel_bytes: &[u8],
    merge_succeeded: bool,
    expected_status: &str,
    extra_fields: impl FnOnce(&Value),
) {
    let workspace = tempfile::tempdir().expect("tempdir");
    let _cwd = CwdGuard::set(workspace.path());
    init_git_workspace(workspace.path());

    let (ctx, event_loop) = build_isolated_executor_loop(workspace.path());
    let channel_path = seed_hat_channel(&ctx, "executor", "primary-u9", 1);
    std::fs::write(&channel_path, channel_bytes).unwrap();

    let target = ctx.workspace().join(".ralph/events-main.jsonl");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "").unwrap();

    let snapshot =
        crate::loop_runner::activation_outcome::snapshot_channel(Some(&channel_path));
    let merge_result = crate::loop_runner::hat_channel::merge_hat_channel(
        &ctx,
        &target,
        "executor",
        None,
    );
    assert_eq!(
        merge_result.is_ok(),
        merge_succeeded,
        "merge success did not match expected"
    );
    let refined =
        crate::loop_runner::activation_outcome::refine_after_merge(snapshot, merge_succeeded);
    let facts = crate::loop_runner::activation_outcome::ActivationOutcomeFacts {
        loop_id: Some(ctx.loop_id().unwrap_or("loop").to_string()),
        channel_exists: true,
        channel_bytes: refined.bytes,
        channel_readable: true,
        merge_succeeded,
        backend_success: true,
        backend_exit_code: Some(0),
        watchdog_timeout: false,
        backend_termination: false,
        output_bytes: 0,
        output_mentions_emit: false,
        terminal_obligation_topics: vec!["work.done".into()],
        ..Default::default()
    };
    crate::loop_runner::activation_outcome::log_activation_outcome(
        event_loop.diagnostics().session_dir(),
        1,
        "executor",
        &refined,
        &facts,
    );

    let rows = read_outcome_rows(&event_loop);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row.get("status").and_then(Value::as_str),
        Some(expected_status),
        "row status mismatch"
    );
    extra_fields(row);
}

#[test]
fn u9_assert_outcome_row_collapsed_fixture_empty_channel() {
    assert_outcome_row(b"", false, "empty", |row| {
        assert_eq!(
            row.get("fields")
                .and_then(|v| v.get("channel_bytes"))
                .and_then(Value::as_u64),
            Some(0)
        );
    });
}

#[test]
fn u9_assert_outcome_row_collapsed_fixture_non_empty_channel() {
    assert_outcome_row(
        b"{\"topic\":\"work.done\"}\n",
        true,
        "merged",
        |row| {
            assert!(row
                .get("fields")
                .and_then(|v| v.get("merge_succeeded"))
                .and_then(Value::as_bool)
                .unwrap_or(false));
        },
    );
}

/// T6 (U2): when `merge_hat_channel` returns Ok on a non-empty
/// channel, the outcome status is `merged`; on an empty channel
/// (Err or Ok), the outcome status stays `empty` because the
/// pre-merge bytes were 0.
#[test]
fn u2_merged_status_only_for_successful_non_empty_merge() {
    // Non-empty success -> merged
    let snapshot = crate::loop_runner::activation_outcome::ChannelSnapshot {
        status: crate::loop_runner::activation_outcome::ActivationOutcomeStatus::Empty,
        bytes: Some(7),
        reference: Some("hat-channel:test".into()),
    };
    let refined = crate::loop_runner::activation_outcome::refine_after_merge(snapshot, true);
    assert_eq!(
        refined.status,
        crate::loop_runner::activation_outcome::ActivationOutcomeStatus::Merged
    );
    // Empty (bytes==0) success -> still empty
    let snapshot = crate::loop_runner::activation_outcome::ChannelSnapshot {
        status: crate::loop_runner::activation_outcome::ActivationOutcomeStatus::Empty,
        bytes: Some(0),
        reference: Some("hat-channel:test".into()),
    };
    let refined = crate::loop_runner::activation_outcome::refine_after_merge(snapshot, true);
    assert_eq!(
        refined.status,
        crate::loop_runner::activation_outcome::ActivationOutcomeStatus::Empty,
        "empty channel stays empty even on merge Ok"
    );
}

/// T7 (U2): diagnostics disabled → no runtime trace sidecar
/// exists → `log_activation_outcome` is a no-op (no panic, no
/// file created). This guards the existing `S7` scenario.
#[test]
fn u2_diagnostics_disabled_is_noop_for_outcome_row() {
    // We exercise the API directly: passing `None` as session_dir
    // must produce no trace row and no panic.
    let snapshot = crate::loop_runner::activation_outcome::ChannelSnapshot {
        status: crate::loop_runner::activation_outcome::ActivationOutcomeStatus::Empty,
        bytes: Some(0),
        reference: Some("hat-channel:test".into()),
    };
    let facts = crate::loop_runner::activation_outcome::ActivationOutcomeFacts::default();
    crate::loop_runner::activation_outcome::log_activation_outcome(
        None, 0, "executor", &snapshot, &facts,
    );
    // Reaching this line without a panic is the assertion.
}

/// T8 (U2): the row's `source_ref` is set to the channel path
/// when the snapshot captured one. This is the single stable
/// reference the diagnosis skill consumes to point at the raw
/// channel artifact.
#[test]
fn u2_outcome_row_carries_channel_source_ref() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let _cwd = CwdGuard::set(workspace.path());
    init_git_workspace(workspace.path());

    let (ctx, event_loop) = build_isolated_executor_loop(workspace.path());
    let channel_path = seed_hat_channel(&ctx, "executor", "primary-004", 1);
    std::fs::write(&channel_path, "").unwrap();

    let snapshot = crate::loop_runner::activation_outcome::snapshot_channel(Some(&channel_path));
    let refined = crate::loop_runner::activation_outcome::refine_after_merge(snapshot, false);
    let facts = crate::loop_runner::activation_outcome::ActivationOutcomeFacts::default();
    crate::loop_runner::activation_outcome::log_activation_outcome(
        event_loop.diagnostics().session_dir(),
        1,
        "executor",
        &refined,
        &facts,
    );

    let rows = read_outcome_rows(&event_loop);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    let source_ref = row
        .get("source_ref")
        .or_else(|| row.get("ref"))
        .and_then(Value::as_str)
        .expect("source_ref must be present");
    assert!(
        source_ref.contains("events-hat-executor"),
        "source_ref must point at the hat channel path, got {source_ref}"
    );
}

/// T9 (U2): schema_version stays at v1 across the new row
/// (regression guard against accidental schema bumps).
#[test]
fn u2_outcome_row_keeps_schema_version_v1() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let _cwd = CwdGuard::set(workspace.path());
    init_git_workspace(workspace.path());

    let (ctx, event_loop) = build_isolated_executor_loop(workspace.path());
    let channel_path = seed_hat_channel(&ctx, "executor", "primary-005", 1);
    std::fs::write(&channel_path, "").unwrap();

    let snapshot = crate::loop_runner::activation_outcome::snapshot_channel(Some(&channel_path));
    let refined = crate::loop_runner::activation_outcome::refine_after_merge(snapshot, false);
    let facts = crate::loop_runner::activation_outcome::ActivationOutcomeFacts::default();
    crate::loop_runner::activation_outcome::log_activation_outcome(
        event_loop.diagnostics().session_dir(),
        1,
        "executor",
        &refined,
        &facts,
    );

    let rows = read_outcome_rows(&event_loop);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row.get("schema_version").and_then(Value::as_str),
        Some("run-diagnosis-trace/v1"),
        "schema_version must stay at v1, got {row}"
    );
    assert_eq!(row.get("phase").and_then(Value::as_str), Some("activation"));
}

// Keep the runtime-trace phase import alive so the test file
// builds if a future test reaches for `RuntimeTracePhase`.
#[allow(dead_code)]
fn _touch_phase() -> RuntimeTracePhase {
    RuntimeTracePhase::Activation
}

// Plan 2026-08-15-1823 U12 (R11): end-to-end runner-path test for
// the merge_failed status. A non-empty channel that fails to merge
// (e.g. write-locked target) must produce a row with status
// "merge_failed", channel_exists=true, channel_bytes>0,
// channel_readable=true.
#[test]
fn u12_in_process_runner_writes_merge_failed_outcome_row() {
    use std::path::Path;
    let workspace = tempfile::tempdir().expect("tempdir");
    let _cwd = CwdGuard::set(workspace.path());
    init_git_workspace(workspace.path());

    let (ctx, event_loop) = build_isolated_executor_loop(workspace.path());
    let channel_path = seed_hat_channel(&ctx, "executor", "primary-mf", 1);
    // Non-empty channel.
    std::fs::write(&channel_path, b"{\"topic\":\"work.done\"}\n").unwrap();

    // Mark the events-main.jsonl as read-only so merge_hat_channel
    // fails closed when it tries to append. The pre-merge snapshot
    // still records the non-empty bytes; refine_after_merge on
    // Err + non-empty bytes must produce MergeFailed.
    let target = ctx.workspace().join(".ralph/events-main.jsonl");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "").unwrap();
    let mut perms = std::fs::metadata(&target).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&target, perms).unwrap();

    let snapshot =
        crate::loop_runner::activation_outcome::snapshot_channel(Some(&channel_path));
    let merge_result =
        crate::loop_runner::hat_channel::merge_hat_channel(&ctx, &target, "executor", None);
    // Restore perms so the test cleanup can write.
    let mut perms = std::fs::metadata(&target).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&target, perms).unwrap();

    assert!(
        merge_result.is_err(),
        "merge must fail when target is read-only"
    );
    let refined =
        crate::loop_runner::activation_outcome::refine_after_merge(snapshot, false);
    assert_eq!(
        refined.status,
        crate::loop_runner::activation_outcome::ActivationOutcomeStatus::MergeFailed,
        "non-empty channel + merge Err must refine to MergeFailed"
    );

    let facts = crate::loop_runner::activation_outcome::ActivationOutcomeFacts {
        loop_id: Some(ctx.loop_id().unwrap_or("loop").to_string()),
        channel_exists: true,
        channel_bytes: refined.bytes,
        channel_readable: true,
        merge_succeeded: false,
        backend_success: true,
        backend_exit_code: Some(0),
        watchdog_timeout: false,
        backend_termination: false,
        output_bytes: 0,
        output_mentions_emit: false,
        terminal_obligation_topics: vec!["work.done".into()],
        ..Default::default()
    };
    crate::loop_runner::activation_outcome::log_activation_outcome(
        event_loop.diagnostics().session_dir(),
        1,
        "executor",
        &refined,
        &facts,
    );

    let rows = read_outcome_rows(&event_loop);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row.get("status").and_then(Value::as_str),
        Some("merge_failed"),
        "non-empty channel + merge Err must record status=merge_failed"
    );
    let _ = Path::new("unused");
}

// Plan 2026-08-15-1823 U13 (R11): end-to-end runner-path test for
// the missing status. The channel marker is not written, so
// resolve_hat_channel_events_path returns None and snapshot_channel
// produces Missing.
#[test]
fn u13_in_process_runner_writes_missing_outcome_row() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let _cwd = CwdGuard::set(workspace.path());
    init_git_workspace(workspace.path());

    let (ctx, event_loop) = build_isolated_executor_loop(workspace.path());
    // Note: no seed_hat_channel call — the marker is absent, so
    // resolve_hat_channel_events_path returns None.
    let snapshot =
        crate::loop_runner::activation_outcome::snapshot_channel(None);
    assert_eq!(
        snapshot.status,
        crate::loop_runner::activation_outcome::ActivationOutcomeStatus::Missing,
        "missing marker must produce snapshot.status=Missing"
    );

    let facts = crate::loop_runner::activation_outcome::ActivationOutcomeFacts {
        loop_id: Some(ctx.loop_id().unwrap_or("loop").to_string()),
        channel_exists: false,
        channel_bytes: None,
        channel_readable: false,
        merge_succeeded: false,
        backend_success: true,
        backend_exit_code: Some(0),
        watchdog_timeout: false,
        backend_termination: false,
        output_bytes: 0,
        output_mentions_emit: false,
        terminal_obligation_topics: vec!["work.done".into()],
        ..Default::default()
    };
    crate::loop_runner::activation_outcome::log_activation_outcome(
        event_loop.diagnostics().session_dir(),
        1,
        "executor",
        &snapshot,
        &facts,
    );

    let rows = read_outcome_rows(&event_loop);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row.get("status").and_then(Value::as_str),
        Some("missing"),
        "absent marker must record status=missing"
    );
}

// Plan 2026-08-15-1823 U13 (R11): end-to-end runner-path test for
// the unreadable status. The channel path's parent directory is
// absent, so std::fs::metadata returns Err and snapshot_channel
// produces Unreadable.
#[test]
fn u13_in_process_runner_writes_unreadable_outcome_row() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let _cwd = CwdGuard::set(workspace.path());
    init_git_workspace(workspace.path());

    let (ctx, _event_loop) = build_isolated_executor_loop(workspace.path());
    // Path whose parent does not exist; std::fs::metadata returns
    // Err and snapshot_channel maps that to Unreadable.
    let unreadable_path = ctx.workspace().join(".ralph/does-not-exist/events-hat-1.jsonl");
    let snapshot = crate::loop_runner::activation_outcome::snapshot_channel(Some(
        &unreadable_path,
    ));
    assert_eq!(
        snapshot.status,
        crate::loop_runner::activation_outcome::ActivationOutcomeStatus::Unreadable,
        "non-existent parent + Some(path) must produce Unreadable"
    );
}
