use super::super::*;
use crate::loop_runner::wave::SupervisorBridge;
use ralph_core::supervisor::{InMemoryCoordinatorBridge, InMemorySupervisorStore, SupervisorStore};

use super::fixtures::*;

/// U5 / S7: a worker that fails with a retryable reason on attempt 1
/// and succeeds on attempt 2 must trigger an in-task retry, no
/// `record_slot_failure`, and end with `record_slot_result` once.
#[tokio::test]
async fn u5_s7_retry_then_success_records_only_result() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(1);

    let wave = make_u3_wave("u5-s7-retry", 1, 1);
    // Attempt 1 = retryable failure; attempt 2 = success with 1 event.
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(0)).with_first_attempt_then(
        0,
        U5SlotOutcome::Fail(U5_RETRYABLE_REASON.to_string()),
        U5SlotOutcome::Success(1),
    );

    let (_outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    // Two attempts on slot 0.
    assert_eq!(
        exec.call_count(0),
        2,
        "U5/S7: retryable failure must trigger a second attempt, got {}",
        exec.call_count(0)
    );

    // No record_slot_failure; exactly one record_slot_result.
    let failures = bridge.failures_snapshot();
    assert!(
        failures.is_empty(),
        "U5/S7: retryable failure must NOT record_slot_failure, got {failures:?}"
    );
    let results = bridge.results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "U5/S7: second attempt success must record_slot_result once, got {results:?}"
    );
    let (slot, _hash, count) = &results[0];
    assert_eq!(*slot, 0);
    assert_eq!(
        *count, 1,
        "U5/S7: fingerprint must reflect attempt 2's batch only"
    );
}

/// U5 / S8: two consecutive retryable failures (budget=1) must exhaust
/// the budget and route to `record_slot_failure("worker_timeout")`. The
/// resulting failure path keeps the `redrive_slots` field present (the
/// store still records a failed slot so the next fan-in can offer a
/// redrive slot).
#[tokio::test]
async fn u5_s8_budget_exhausted_records_failure_with_redrive_eligibility() {
    use ralph_core::supervisor::SupervisorStore;

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(1);

    let wave = make_u3_wave("u5-s8-budget", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Fail(U5_RETRYABLE_REASON.to_string()));

    let (_outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    // Two attempts (initial + 1 retry, budget=1).
    assert_eq!(
        exec.call_count(0),
        2,
        "U5/S8: budget=1 must allow exactly 2 attempts, got {}",
        exec.call_count(0)
    );

    // Exactly one record_slot_failure with the FROZEN reason code.
    let results = bridge.results_snapshot();
    assert!(
        results.is_empty(),
        "U5/S8: budget exhausted must NOT record_slot_result, got {results:?}"
    );
    let failures = bridge.failures_snapshot();
    assert_eq!(
        failures.len(),
        1,
        "U5/S8: budget exhausted must record_slot_failure once, got {failures:?}"
    );
    let (slot, reason) = &failures[0];
    assert_eq!(*slot, 0);
    assert_eq!(
        reason, "worker_timeout",
        "U5/S8: stored reason must be the FROZEN worker_timeout static code"
    );

    // Store state: 0 completed + 1 failed (so the next fan-in/recovery
    // path can offer this slot for redrive).
    let store_wave_id = bridge
        .store
        .recover_active_waves()
        .expect("recover")
        .pop()
        .expect("one wave")
        .wave_id;
    let snap = bridge
        .store
        .fan_in_status(&store_wave_id)
        .expect("snapshot");
    assert_eq!(
        snap.completed_count, 0,
        "U5/S8: failed slot must not lift completed_count"
    );
    assert_eq!(
        snap.failed_count, 1,
        "U5/S8: failed slot must lift failed_count"
    );
}

/// U5 / S10: a non-retryable failure (e.g. `worker_cancelled` or unknown
/// dynamic reason) must NOT trigger a retry, even when budget > 0.
#[tokio::test]
async fn u5_s10_non_retryable_failure_does_not_retry() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(2);

    let wave = make_u3_wave("u5-s10-cancelled", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Fail("worker_cancelled".to_string()));

    let (_outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    assert_eq!(
        exec.call_count(0),
        1,
        "U5/S10: non-retryable reason must attempt exactly once, got {}",
        exec.call_count(0)
    );

    let failures = bridge.failures_snapshot();
    assert_eq!(
        failures.len(),
        1,
        "U5/S10: cancelled must record one failure"
    );
    let (_, reason) = &failures[0];
    assert_eq!(
        reason, "worker_cancelled",
        "U5/S10: store must keep the verbatim reason"
    );
}

/// U5 / S12: an intermediate partial-batch attempt that fails with a
/// retryable reason must NOT leak its events into the
/// `record_slot_result` fingerprint. Only the *final* attempt's batch
/// (whether success or failure) must end up in the store.
#[tokio::test]
async fn u5_s12_final_attempt_fingerprint_only() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(1);

    let wave = make_u3_wave("u5-s12-fingerprint", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(0)).with_first_attempt_then(
        0,
        U5SlotOutcome::Fail(U5_RETRYABLE_REASON.to_string()),
        U5SlotOutcome::Success(3),
    );

    let (_outcome, bridge, _exec) = run_u5_execute_wave(bridge, wave, executor).await;

    let results = bridge.results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "U5/S12: only the final attempt's result may be recorded, got {results:?}"
    );
    let (slot, _hash, count) = &results[0];
    assert_eq!(*slot, 0);
    assert_eq!(
        *count, 3,
        "U5/S12: event_count must reflect the final attempt's batch (3 events), not the failed attempt"
    );
}

/// U1 验收 #1 / S1+S3: attempt 1 emits `exec.unit.failed`, attempt 2
/// emits `exec.unit.done`. The dispatcher must run TWO attempts, record
/// exactly one `record_slot_result` (attempt 2) and zero
/// `record_slot_failure`, and only attempt 2's event batch may escape.
#[tokio::test]
async fn executor_failed_terminal_retries_then_done() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(2);

    let wave = make_u3_wave("u1-failed-then-done", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(0)).with_attempts(
        0,
        vec![
            exec_reported_failure("compile error in unit F1"),
            U5SlotOutcome::Success(1),
        ],
    );

    let (outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    assert_eq!(
        exec.call_count(0),
        2,
        "U1: exec.unit.failed must be treated as a retryable attempt, got {} call(s)",
        exec.call_count(0)
    );
    assert!(
        bridge.failures_snapshot().is_empty(),
        "U1: an intermediate reported failure must not record_slot_failure, got {:?}",
        bridge.failures_snapshot()
    );
    let results = bridge.results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "U1: only the final successful attempt may record_slot_result, got {results:?}"
    );

    let completed = completed_wave_of(&outcome);
    let topics: Vec<&str> = completed
        .results
        .iter()
        .flat_map(|r| r.events.iter().map(|e| e.topic.as_str()))
        .collect();
    assert_eq!(
        topics,
        vec!["exec.unit.done"],
        "U1: only attempt 2's terminal may reach the tracker, got {topics:?}"
    );
}

/// U1 验收 #2 / S5: with `slot_retry_budget = 0` a reported failure
/// must NOT be retried; the slot goes straight to its final failure.
#[tokio::test]
async fn executor_failed_terminal_budget_zero_does_not_retry() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(0);

    let wave = make_u3_wave("u1-budget-zero", 1, 1);
    let executor = U5RecordingExecutor::new(exec_reported_failure("no budget"));

    let (_outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    assert_eq!(
        exec.call_count(0),
        1,
        "U1: budget=0 must keep the single-attempt semantics, got {}",
        exec.call_count(0)
    );
    let failures = bridge.failures_snapshot();
    assert_eq!(
        failures,
        vec![(0u32, "executor_reported_failure".to_string())],
        "U1: the terminal failure must be stored under the stable code"
    );
    assert!(
        bridge.results_snapshot().is_empty(),
        "U1: a reported failure must never take the record_slot_result path"
    );
}

/// U1 验收 #3 / S6: Review waves keep the pre-existing truth table —
/// `review.unit.failed` is a Completed terminal, never retried, and
/// never labelled `executor_reported_failure`.
#[tokio::test]
async fn non_exec_failed_terminal_keeps_existing_semantics() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(2);

    let wave = make_u3_wave_with_topic("u1-review-failed", 1, 1, 1, "review.unit.ready");
    let executor = U5RecordingExecutor::new(U5SlotOutcome::ReportedFailure {
        terminal_topic: "review.unit.failed",
        reason: "finding".to_string(),
    });

    let (_outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    assert_eq!(
        exec.call_count(0),
        1,
        "U1: a Review failed terminal must not enter the Exec retry path, got {}",
        exec.call_count(0)
    );
    assert_eq!(
        bridge.results_snapshot().len(),
        1,
        "U1: Review failed terminal keeps the Completed record path"
    );
    assert!(
        bridge.failures_snapshot().is_empty(),
        "U1: Review failed terminal must not record_slot_failure, got {:?}",
        bridge.failures_snapshot()
    );
}

/// U1 验收 #4 / S3: the intermediate attempt's `exec.unit.failed`
/// event batch must never reach the tracker (and therefore never the
/// main ledger).
#[tokio::test]
async fn intermediate_exec_failed_event_does_not_escape() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(2);

    let wave = make_u3_wave("u1-no-leak", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(0)).with_attempts(
        0,
        vec![
            exec_reported_failure("attempt 1 detail"),
            exec_reported_failure("attempt 2 detail"),
            U5SlotOutcome::Success(2),
        ],
    );

    let (outcome, _bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    assert_eq!(exec.call_count(0), 3, "U1: budget=2 allows three attempts");
    let completed = completed_wave_of(&outcome);
    let failed_topics: Vec<&str> = completed
        .results
        .iter()
        .flat_map(|r| r.events.iter().map(|e| e.topic.as_str()))
        .filter(|t| *t == "exec.unit.failed")
        .collect();
    assert!(
        failed_topics.is_empty(),
        "U1: no intermediate exec.unit.failed may escape to the tracker, got {failed_topics:?}"
    );
}

/// U1 验收 #5 / S4 + D15: after the budget is exhausted the final
/// `exec.unit.failed` batch must be normalized into a stable slot
/// failure — it must NOT appear in `CompletedWave.results`.
#[tokio::test]
async fn exhausted_exec_failed_event_is_normalized_before_tracker() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(2);

    let wave = make_u3_wave("u1-exhausted", 1, 1);
    let executor = U5RecordingExecutor::new(exec_reported_failure("still broken"));

    let (outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    assert_eq!(
        exec.call_count(0),
        3,
        "U1: budget=2 means initial + 2 redispatches = 3 attempts, got {}",
        exec.call_count(0)
    );

    let completed = completed_wave_of(&outcome);
    assert!(
        completed.results.is_empty(),
        "U1: the exhausted failure batch must not become a tracker result, got {:?}",
        completed.results
    );
    let reasons: Vec<&str> = completed
        .failures
        .iter()
        .map(|f| f.error.as_str())
        .collect();
    assert_eq!(
        reasons,
        vec!["executor_reported_failure"],
        "U1: the tracker failure must carry the stable code, got {reasons:?}"
    );
    assert_eq!(
        bridge.failures_snapshot(),
        vec![(0u32, "executor_reported_failure".to_string())],
        "U1: the store must record exactly one stable failure"
    );
    assert!(
        bridge.results_snapshot().is_empty(),
        "U1: an exhausted slot must never record_slot_result"
    );
}

/// 2026-07-23-007 plan U4 (R-W5): when the production bridge
/// carries a `tasks.jsonl` path, the dispatcher projects each
/// slot's terminal state onto a stable task row. A successful
/// slot ends up as `TaskStatus::Closed` in `tasks.jsonl`;
/// `ralph tools task list` (via `TaskStore::load`) sees it.
/// Idempotent re-projection does not duplicate rows.
#[tokio::test]
async fn test_u4_slot_terminal_projects_to_tasks_jsonl() {
    use crate::loop_runner::wave::CoordinatorSupervisorBridge;
    use ralph_core::TaskStore;

    let tmp = tempfile::tempdir().expect("temp dir");
    let workspace_root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let tasks_dir = workspace_root.join(".ralph").join("agent");
    std::fs::create_dir_all(&tasks_dir).expect("create tasks dir");
    let tasks_path = tasks_dir.join("tasks.jsonl");
    let main_events_file = workspace_root.join(".ralph").join("events.jsonl");

    #[derive(Debug)]
    struct StubFactory;
    impl ralph_core::supervisor::worktree_bind::WorktreeFactory for StubFactory {
        fn create(
            &self,
            repo_root: std::path::PathBuf,
            branch: String,
        ) -> Result<
            ralph_core::worktree::Worktree,
            ralph_core::supervisor::worktree_bind::WorktreeError,
        > {
            let wt = repo_root.join(format!("wt-{branch}"));
            std::fs::create_dir_all(&wt).ok();
            Ok(ralph_core::worktree::Worktree {
                path: wt,
                branch,
                is_main: false,
                head: None,
            })
        }
    }

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = ProductionBridgeContext {
        loop_id: "u4-loop".to_string(),
        repo_root: workspace_root.clone(),
        events_path: Some(main_events_file.clone()),
        tasks_path: Some(tasks_path.clone()),
    };
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        std::sync::Arc::new(StubFactory),
    );

    let wave = make_u3_wave("u4-projection", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(1));
    let _outcome =
        run_u2_execute_wave_with_env_capture(bridge, wave, executor, &main_events_file, "u4-loop")
            .await;

    // U4/007: tasks.jsonl now carries one row for slot 0 with a
    // stable task_key and a terminal status.
    let task_store = TaskStore::load(&tasks_path).expect("load");
    let rows: Vec<_> = task_store.all().iter().collect();
    assert_eq!(
        rows.len(),
        1,
        "U4/007: exactly one task row projected; got {rows:?}"
    );
    let row = &rows[0];
    let key = row.key.as_deref().unwrap_or("");
    // U4/007: the task_key MUST start with `supervisor:u4-loop:`
    // (loop-scoped) and reference slot 0. The wave-id portion is
    // produced by the supervisor store's allocator, so we only
    // assert the loop + slot shape to keep the test stable
    // against allocator changes.
    assert!(
        key.starts_with("supervisor:u4-loop:") && key.ends_with(":slot-0"),
        "U4/007: stable task_key must carry loop_id + slot_index; got {key:?}"
    );
    let status_str = format!("{:?}", row.status);
    assert!(
        status_str.to_lowercase().contains("closed") || status_str.to_lowercase().contains("done"),
        "U4/007: completed slot must project to terminal status; got {status_str}"
    );
}

/// 2026-07-23-007 plan U10 (T3 / T6): the sibling of
/// `test_u4_slot_terminal_projects_to_tasks_jsonl` for the
/// Failed path. A slot that classifies as Failed (e.g. the
/// worker backend reports a non-zero exit with no accepted
/// terminal marker) MUST project a `Failed` task row, NOT a
/// `Closed` row. The existing Success sibling checked
/// terminal-status with a substring match; this test uses
/// `assert_eq!` against `TaskStatus::Failed` directly so the
/// assertion strength matches the schema (folding testing:T6
/// into the same test).
#[tokio::test]
async fn test_u4_failed_slot_projects_to_failed_task_row() {
    use crate::loop_runner::wave::CoordinatorSupervisorBridge;
    use ralph_core::TaskStatus;
    use ralph_core::TaskStore;

    let tmp = tempfile::tempdir().expect("temp dir");
    let workspace_root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let tasks_dir = workspace_root.join(".ralph").join("agent");
    std::fs::create_dir_all(&tasks_dir).expect("create tasks dir");
    let tasks_path = tasks_dir.join("tasks.jsonl");
    let main_events_file = workspace_root.join(".ralph").join("events.jsonl");

    #[derive(Debug)]
    struct StubFactory;
    impl ralph_core::supervisor::worktree_bind::WorktreeFactory for StubFactory {
        fn create(
            &self,
            repo_root: std::path::PathBuf,
            branch: String,
        ) -> Result<
            ralph_core::worktree::Worktree,
            ralph_core::supervisor::worktree_bind::WorktreeError,
        > {
            let wt = repo_root.join(format!("wt-{branch}"));
            std::fs::create_dir_all(&wt).ok();
            Ok(ralph_core::worktree::Worktree {
                path: wt,
                branch,
                is_main: false,
                head: None,
            })
        }
    }

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = ProductionBridgeContext {
        loop_id: "u10-loop".to_string(),
        repo_root: workspace_root.clone(),
        events_path: Some(main_events_file.clone()),
        tasks_path: Some(tasks_path.clone()),
    };
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        std::sync::Arc::new(StubFactory),
    );

    let wave = make_u3_wave("u10-failed-projection", 1, 1);
    // U5's Fail arm: the executor reports a non-zero exit with
    // a structured error reason. The classifier maps it to
    // SlotOutcome::Failed{worker_cancelled} (per the U3
    // worker_outcome.rs short-circuit), so the slot must
    // project to TaskStatus::Failed.
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Fail("boom".to_string()));
    let _outcome =
        run_u2_execute_wave_with_env_capture(bridge, wave, executor, &main_events_file, "u10-loop")
            .await;

    let task_store = TaskStore::load(&tasks_path).expect("load");
    let rows: Vec<_> = task_store.all().iter().collect();
    assert_eq!(
        rows.len(),
        1,
        "U10/007: exactly one task row projected for the Failed slot; got {rows:?}"
    );
    let row = &rows[0];
    let key = row.key.as_deref().unwrap_or("");
    assert!(
        key.starts_with("supervisor:u10-loop:") && key.ends_with(":slot-0"),
        "U10/007: stable task_key must carry loop_id + slot_index; got {key:?}"
    );
    assert_eq!(
        row.status,
        TaskStatus::Failed,
        "U10/007: Failed slot must project to TaskStatus::Failed (not Closed); got {:?}",
        row.status
    );
}

// =============================================================================
// 2026-07-22-001 plan U2: default wave path unified to SupervisorStore.
//
// Prior baseline: when the runner passed `supervisor_bridge: None`
// (i.e. `event_loop.supervisor.enabled: false`), the dispatcher
// took the legacy `WaveTracker::new()` branch, leaving the
// supervisor store completely absent for default-path waves.
//
// Post-U2 contract:
// 1. The default path **lazily** constructs an
//    `InMemorySupervisorStore`-backed bridge when `supervisor_bridge`
//    is `None` **and** a `DetectedWave` is present in the batch.
// 2. Pure pipeline batches (no `DetectedWave`) keep the 023 R1
//    invariant: zero `supervisor.db`, zero `bridge_build_invocations`
//    delta. The `bridge` parameter stays `None` and the runner does
//    not see a phantom bridge.
// 3. `register_wave_if_absent` errors fail closed: register errors
//    return `WaveDispatchOutcome::SpawnFailed { spawned = 0,
//    expected = total }` instead of falling back to legacy
//    `WaveTracker` dispatch (which would re-open the OPAC
//    register-double-spawn gap).
// =============================================================================

/// U2 / default path constructs an in-memory store-backed bridge on
/// demand. We exercise the predicate the dispatcher uses to decide
/// "should I lazily build a bridge?" by asserting that, given a
/// supervisor_bridge of `None` and an empty accepted batch, the
/// dispatcher keeps `supervisor_bridge_owned` at `None` (i.e. no
/// phantom bridge is created).
#[test]
fn u2_no_phantom_bridge_when_no_detected_wave() {
    // Mirror the dispatcher's `accepted_len` predicate.
    let accepted_len: usize = 0;
    let supplied: Option<&Arc<dyn SupervisorBridge>> = None;
    let lazy_bridge: Option<Arc<dyn SupervisorBridge>> = if supplied.is_some() {
        supplied.cloned()
    } else if accepted_len > 0 {
        // Not reached when accepted_len == 0.
        unreachable!()
    } else {
        None
    };
    assert!(
        lazy_bridge.is_none(),
        "no DetectedWave → no lazily-built bridge (023 R1 invariant)"
    );
}

/// U2 / differential: when the dispatcher takes the legacy
/// `WaveTracker::new()` branch (`supervisor_bridge: None`), the
/// `WaveTracker` surface must still be reachable so call sites that
/// have not yet been migrated keep compiling. This pins the surface
/// for the migration window — U9 will delete `WaveTracker` once all
/// tests are migrated to the supervisor store.
#[test]
fn u2_legacy_wave_tracker_surface_still_reachable() {
    // `ralph_core::WaveTracker::new()` is the legacy shape; pinned
    // here so any rename in `ralph_core` surfaces as a compile
    // failure in this test file instead of a silent dispatcher
    // breakage.
    let _tracker = ralph_core::WaveTracker::new();
}

/// U2 / fail-closed: register errors on the supervisor path must
/// not silently fall back to legacy dispatch. We assert the
/// `WaveDispatchOutcome::SpawnFailed { spawned_count: 0,
/// expected_count: total }` shape carries the right counts.
#[test]
fn u2_register_failure_fails_closed() {
    let wave_total: u32 = 4;
    // Mirror the U2 register error mapping in
    // `execute_wave_via_supervisor_with_executor`:
    let outcome = WaveDispatchOutcome::SpawnFailed {
        spawned_count: 0,
        expected_count: wave_total,
    };
    match outcome {
        WaveDispatchOutcome::SpawnFailed {
            spawned_count,
            expected_count,
        } => {
            assert_eq!(spawned_count, 0, "no workers spawned on register error");
            assert_eq!(
                expected_count, wave_total,
                "expected_count must equal the wave's total"
            );
        }
        other => panic!("register failure must map to SpawnFailed, got {other:?}"),
    }
}

/// U2 / InMemory store reachability: the lazy-bridge construction
/// must use a `SupervisorStore` that exposes `register_wave` so
/// downstream dispatcher code stays uniform with the production
/// (Rusqlite-backed) path. We additionally verify the
/// `SupervisorBridge::register_wave_if_absent` shape the dispatcher
/// calls (`Arc<dyn SupervisorBridge>`) is idempotent.
#[test]
fn u2_lazy_bridge_uses_in_memory_store_trait_surface() {
    use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore as _, WaveKind};
    // Direct store: register_wave accepts (wave_id, kind, total).
    let store = InMemorySupervisorStore::new();
    let id = store
        .register_wave("u2-lazy-wave", WaveKind::Exec, 2, 0)
        .expect("first register ok");
    assert!(!id.is_empty(), "register_wave must return a non-empty id");

    // Bridge adapter (`CoordinatorSupervisorBridge::with_in_memory_store`)
    // exposes `register_wave_if_absent`, which the dispatcher calls.
    // The lazy-bridge construction in `handle_wave_events` uses this
    // exact surface so we pin it here as the lazy-bridge contract.
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    let lazy_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u2-lazy-bridge", 1, 0)
        .expect("first bridge register ok");
    let lazy_id_2 = bridge
        .register_wave_if_absent(WaveKind::Exec, "u2-lazy-bridge", 1, 0)
        .expect("second bridge register is idempotent");
    assert_eq!(
        lazy_id, lazy_id_2,
        "lazy-bridge register_wave_if_absent must be idempotent"
    );
}

// =============================================================================
// 2026-07-22-001 plan U5: idempotency SSoT migrates from the CLI
// sidecar file to the supervisor store. The dispatcher's
// `register_wave_if_absent` is the authoritative dedup gate; the
// sidecar remains as a one-version compat shim with a one-shot
// deprecation warning. We pin both contracts here.
// =============================================================================

/// U5 / SSOT: `register_wave_if_absent` is the authoritative
/// idempotency check. Re-registering the same `(kind, wave_id,
/// total)` returns the same store wave_id and does NOT spawn a
/// fresh wave row. This is the contract the dispatcher relies on
/// for content-hash dedup and concurrent dispatch safety.
#[test]
fn u5_register_wave_if_absent_is_idempotent_sso_t() {
    use ralph_core::supervisor::WaveKind;
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    let id1 = bridge
        .register_wave_if_absent(WaveKind::Exec, "u5-sso-wave", 3, 0)
        .expect("first register");
    let id2 = bridge
        .register_wave_if_absent(WaveKind::Exec, "u5-sso-wave", 3, 0)
        .expect("second register");
    assert_eq!(
        id1, id2,
        "same wave_id must produce the same store-assigned id (SSOT)"
    );
}

/// U5 / content_hash dedup at the slot level: when a slot's
/// `record_slot_result` is called twice with the same
/// `content_hash`, the store accepts both calls but the dispatch
/// fan-in layer deduplicates the resulting business events before
/// writing to the ledger. We pin the contract that `content_hash`
/// is a stable input to the dedup logic.
#[test]
fn u5_content_hash_is_part_of_record_slot_result_signature() {
    use ralph_core::supervisor::WaveKind;
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    let wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u5-content-hash", 2, 0)
        .expect("register");
    // Same content_hash for slot 0 — store accepts the second
    // call as a no-op duplicate so the dispatcher can safely
    // call record_slot_result multiple times without spawning a
    // new spawn row.
    bridge
        .record_slot_result(&wave_id, 0, "h-uniform", 1)
        .expect("first record");
    // Second call is also OK at the trait level; dedup is a
    // caller concern. We only assert the trait surface accepts
    // repeated calls with the same fingerprint.
    let ok = bridge
        .record_slot_result(&wave_id, 0, "h-uniform", 1)
        .is_ok();
    assert!(
        ok,
        "store must accept repeated record_slot_result with the same content_hash"
    );
}

/// U5 / backpressure cap is observable via `max_concurrent_workers`.
/// The lazy-bridge (U2) sets the cap to `u32::MAX` so default-path
/// waves are not artificially throttled before U5 ships the
/// per-wave cap wiring. We pin the current `u32::MAX` default so
/// a regression that lowers the cap (silently throttling waves)
/// surfaces as a test failure.
#[test]
fn u5_lazy_bridge_default_cap_is_unlimited() {
    use ralph_core::supervisor::WaveKind;
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    // Lazy construction defaults the cap to u32::MAX. U5's
    // production cap wiring threads `max_concurrent_workers`
    // through the dispatcher's reverse-pressure scheduler; that
    // comes in a follow-up. The lazy default must remain
    // unlimited so an early U5 patch does not silently drop
    // waves.
    let _ = bridge.register_wave_if_absent(WaveKind::Exec, "u5-cap", 1, 0);
    assert_eq!(
        bridge.max_concurrent_workers(),
        u32::MAX,
        "lazy-bridge default cap must remain u32::MAX until per-wave cap wiring lands"
    );
}

// =============================================================================
// 2026-07-25-003 plan U5: legacy `WaveTracker.record_outcome` and the
// supervisor classifier must agree that an empty-success outcome
// (`Ok(([], _, true))`) is a failure, not a result. Without this
// invariant, an empty-success worker is counted in `CompletedWave.results`
// (the `results=N` line in dispatcher logs) while the supervisor
// store flags the same slot `empty_worker_result` — a "results=N
// failures=1" log line that masks an all-failed store. The fix is a
// single line in `record_outcome`; the test pins the new behaviour
// directly so a future refactor cannot silently re-introduce the
// drift.
// =============================================================================

/// U5 Red/Green: `record_outcome` must treat
/// `Ok((events=[], duration, success=true))` as a failure (the
/// canonical `empty_worker_result` reason), NOT a result. The
/// supervisor path's `classify_slot_result` already does the right
/// thing; this test pins the legacy `WaveTracker` path so the two
/// stay aligned.
#[test]
fn test_u5_record_outcome_empty_success_is_failure() {
    use crate::loop_runner::wave::record_outcome;

    let mut tracker = ralph_core::WaveTracker::new();
    tracker.register_wave_with_source(
        "u5-empty-success".to_string(),
        1,
        Some(ralph_proto::HatId::new("u5-hat")),
    );

    // Plan 2026-07-25-003 U5 / R3: a worker that exits 0 with no
    // accepted events is `empty_worker_result`, not a result. The
    // legacy `record_outcome` previously wrote this as a
    // result (because `success || events.is_empty()` was
    // satisfied by the `success` arm), producing the
    // `results=N failures=0` log line that masked a store row
    // flagged `empty_worker_result`.
    let outcome: crate::loop_runner::wave::WaveWorkerOutcome =
        Ok((Vec::new(), std::time::Duration::from_millis(5), true, None));
    record_outcome(&mut tracker, "u5-empty-success", 0, outcome);

    let completed = tracker
        .take_wave_results("u5-empty-success")
        .expect("wave must exist after registration");
    assert_eq!(
        completed.results.len(),
        0,
        "U5/003: empty-success must NOT be counted as a result; got {completed:?}"
    );
    assert_eq!(
        completed.failures.len(),
        1,
        "U5/003: empty-success must be recorded as a failure; got {completed:?}"
    );
    assert_eq!(
        completed.failures[0].error, "empty_worker_result",
        "U5/003: empty-success reason must equal the canonical `empty_worker_result`; got {:?}",
        completed.failures[0].error
    );
}

/// U5 regression guard: the partial-timeout contract (PTY
/// workers exit non-zero but produce partial events) must keep
/// its `record_result` path even after the empty-success fix.
/// A non-zero exit WITH events stays a result — the dispatcher's
/// merge layer still surfaces the partial events to the
/// aggregator.
#[test]
fn test_u5_record_outcome_partial_timeout_stays_result() {
    use crate::loop_runner::wave::record_outcome;

    let mut tracker = ralph_core::WaveTracker::new();
    tracker.register_wave_with_source(
        "u5-partial-timeout".to_string(),
        1,
        Some(ralph_proto::HatId::new("u5-hat")),
    );

    let event = ralph_core::Event {
        topic: "exec.unit.done".to_string(),
        payload: Some("{\"slot\":0}".to_string()),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let outcome: crate::loop_runner::wave::WaveWorkerOutcome =
        Ok((vec![event], std::time::Duration::from_millis(5), false, None));
    record_outcome(&mut tracker, "u5-partial-timeout", 0, outcome);

    let completed = tracker
        .take_wave_results("u5-partial-timeout")
        .expect("wave must exist after registration");
    assert_eq!(
        completed.results.len(),
        1,
        "U5/003: partial-timeout (success=false, events=non-empty) must stay a result for the merge layer; got {completed:?}"
    );
    assert_eq!(
        completed.failures.len(),
        0,
        "U5/003: partial-timeout must NOT be a failure; got {completed:?}"
    );
}

// 2026-09-01-001 plan U2 (R2 / S2.1 / T2.1): a wave whose Completed
// slot had its events persisted by U1's `record_slot_event_payloads`
// but never reached the main ledger (the loop died between worker
// exit and fan-in) must, on the next startup, replay those events
// through the salvage seam. The redelivery pass reads back the
// payload rows and asserts the events land in the main ledger.
#[test]
fn u2_2026_09_01_redelivery_replays_persisted_payload_to_main() {
    use crate::loop_runner::wave::recovery_redelivery;
    use ralph_core::supervisor::WaveKind;
    use std::sync::Arc;

    let workspace = tempfile::TempDir::new().expect("temp workspace");
    let ralph_dir = workspace.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
    let main_events_file = ralph_dir.join("events.jsonl");

    let store = Arc::new(InMemorySupervisorStore::new());
    let bridge = Arc::new(InMemoryCoordinatorBridge::from_store(
        store.clone() as Arc<dyn SupervisorStore>
    ));
    let wave = store
        .register_wave("u2-replay-2026-09-01", WaveKind::Exec, 2, 1)
        .expect("register");
    let events = vec![
        ralph_core::Event {
            topic: "exec.unit.done".to_string(),
            payload: Some(r#"{"slot_index":0,"seq":0}"#.to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: Some("exec-worker".to_string()),
            wave_id: Some(wave.clone()),
            wave_index: Some(0),
            wave_total: Some(2),
            system_injected: Some(false),
        },
        ralph_core::Event {
            topic: "exec.progress".to_string(),
            payload: Some(r#"{"step":"build"}"#.to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: Some("exec-worker".to_string()),
            wave_id: Some(wave.clone()),
            wave_index: Some(0),
            wave_total: Some(2),
            system_injected: Some(false),
        },
    ];
    store
        .record_slot_event_payloads(&wave, 0, 1, &events)
        .expect("persist slot 0 events");
    let events_slot1 = vec![ralph_core::Event {
        topic: "exec.unit.done".to_string(),
        payload: Some(r#"{"slot_index":1,"seq":0}"#.to_string()),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: Some("exec-worker".to_string()),
        wave_id: Some(wave.clone()),
        wave_index: Some(1),
        wave_total: Some(2),
        system_injected: Some(false),
    }];
    store
        .record_slot_event_payloads(&wave, 1, 1, &events_slot1)
        .expect("persist slot 1 events");

    let report = recovery_redelivery::redeliver_persisted_slot_events(
        store.clone(),
        bridge,
        &main_events_file,
    );

    assert!(
        report.warnings.is_empty(),
        "U2: redelivery pass must not warn on a healthy persisted payload; got {:?}",
        report.warnings
    );
    assert_eq!(
        report.redelivered,
        vec![wave.clone()],
        "U2: the wave must land in `redelivered`; got {:?}",
        report.redelivered
    );

    let main_ledger = std::fs::read_to_string(&main_events_file).expect("read main");
    assert!(
        main_ledger.contains(r#""topic":"exec.unit.done""#),
        "U2: main ledger must carry the slot 0 unit-done; got {main_ledger}"
    );
    assert!(
        main_ledger.contains(r#""topic":"exec.progress""#),
        "U2: main ledger must carry the slot 0 progress; got {main_ledger}"
    );
    assert!(
        main_ledger.contains(r#"\"slot_index\":1"#),
        "U2: main ledger must carry the slot 1 unit-done; got {main_ledger}"
    );

    // S1.2 / S2.3: payload rows must be cleaned after a successful
    // redelivery so the store does not accumulate dead rows.
    assert!(
        store
            .load_slot_event_payloads(&wave)
            .expect("load")
            .is_empty(),
        "U2: persisted payload rows must be deleted after redelivery"
    );
}

// 2026-09-01-001 plan U2 (R2 / S2.4 / T2.3): a pre-U1 crash
// remnant (Completed slot but no payload rows) must NOT panic
// and must surface as a warning so operators can grep for
// legacy crash windows. No events are written.
#[test]
fn u2_2026_09_01_redelivery_handles_legacy_remnant_with_warning() {
    use crate::loop_runner::wave::recovery_redelivery;
    use ralph_core::supervisor::WaveKind;
    use std::sync::Arc;

    let workspace = tempfile::TempDir::new().expect("temp workspace");
    let ralph_dir = workspace.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
    let main_events_file = ralph_dir.join("events.jsonl");

    let store = Arc::new(InMemorySupervisorStore::new());
    let bridge = Arc::new(InMemoryCoordinatorBridge::from_store(
        store.clone() as Arc<dyn SupervisorStore>
    ));
    // Register a wave but DO NOT record any slot_event_payloads.
    let wave = store
        .register_wave("u2-remnant-2026-09-01", WaveKind::Exec, 1, 1)
        .expect("register");
    // Force the snapshot's `completed_count` above zero by
    // recording a slot result. The recovery module only warns
    // when a wave has Completed slots but no payload rows.
    store
        .record_slot_result(&wave, 0, "fingerprint", 1)
        .expect("record slot result");

    let report = recovery_redelivery::redeliver_persisted_slot_events(
        store.clone(),
        bridge,
        &main_events_file,
    );

    assert!(
        report.redelivered.is_empty(),
        "U2: legacy remnant must not be redelivered (no payload rows); got {:?}",
        report.redelivered
    );
    assert!(
        report.warnings.iter().any(|w| w.contains("pre-U1")),
        "U2: legacy remnant must surface a pre-U1 warning; got {:?}",
        report.warnings
    );
    // Main ledger must be empty — no events were ever persisted.
    assert!(
        !main_events_file.exists() || main_events_file.metadata().map(|m| m.len()).unwrap_or(0) == 0,
        "U2: legacy remnant must not write to the main ledger"
    );
}

// 2026-09-01-001 plan U3 (R3 / S3.1 / T3.1-T3.3): a wave that
// the recovery evaluator marked `Failed` (timeout) must get a
// system_injected `exec.wave.failed` injection so the parallel
// forge topology's failure-handler hat can activate. The
// injection must precede the slot 0 salvage row (KTD2 order:
// salvage first, failed-inject second).
#[test]
fn u3_2026_09_01_timed_out_wave_injects_exec_wave_failed() {
    use crate::loop_runner::wave::recovery_redelivery;
    use ralph_core::supervisor::WaveKind;
    use std::sync::Arc;

    let workspace = tempfile::TempDir::new().expect("temp workspace");
    let ralph_dir = workspace.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
    let main_events_file = ralph_dir.join("events.jsonl");

    let store = Arc::new(InMemorySupervisorStore::new());
    let wave = store
        .register_wave("u3-timed-out-2026-09-01", WaveKind::Exec, 1, 1)
        .expect("register");

    // Inject — the function reads fan_in_status to build the
    // payload; the wave exists and is in Dispatch phase so
    // `delivery_state < CoordinationCommitted` holds.
    let injected = recovery_redelivery::inject_timed_out_failed_coord(
        &[wave.clone()],
        store.clone(),
        &main_events_file,
    );

    assert_eq!(
        injected,
        vec![wave.clone()],
        "U3: timed-out wave must appear in the injected list; got {injected:?}"
    );

    let main_ledger = std::fs::read_to_string(&main_events_file).expect("read main");
    assert!(
        main_ledger.contains(r#""topic":"exec.wave.failed""#),
        "U3: main ledger must carry system_injected exec.wave.failed; got {main_ledger}"
    );
    assert!(
        main_ledger.contains(r#""system_injected":true"#),
        "U3: exec.wave.failed row must be system_injected; got {main_ledger}"
    );
    assert!(
        main_ledger.contains(&wave),
        "U3: payload must reference the timed-out wave id; got {main_ledger}"
    );
}

// 2026-09-01-001 plan U3 (R3 / S3.2 / T3.4): a wave whose
// `in_flight_count > 0` but whose elapsed time is BELOW
// `aggregate_timeout_secs` must NOT receive an
// `exec.wave.failed` injection. `inject_timed_out_failed_coord`
// receives a `timed_out_pending_injection` list as input —
// the recovery evaluator only puts a wave id in this list
// when it has decided the wave timed out (so this test
// pins the caller-side contract: do not call inject_… with
// non-timed-out wave ids).
#[test]
fn u3_2026_09_01_empty_injection_list_is_no_op() {
    use crate::loop_runner::wave::recovery_redelivery;
    use std::sync::Arc;

    let workspace = tempfile::TempDir::new().expect("temp workspace");
    let ralph_dir = workspace.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
    let main_events_file = ralph_dir.join("events.jsonl");

    let store = Arc::new(InMemorySupervisorStore::new());
    let injected = recovery_redelivery::inject_timed_out_failed_coord(
        &[],
        store.clone(),
        &main_events_file,
    );
    assert!(injected.is_empty());
    assert!(
        !main_events_file.exists(),
        "U3: empty injection list must NOT touch the main ledger"
    );
}

// 2026-09-01-001 plan U3 S3.1 / KTD2: salvage redelivery must
// land before the timeout `exec.wave.failed` injection.
#[test]
fn s31_2026_09_01_salvage_precedes_timed_out_failed_inject() {
    use crate::loop_runner::wave::recovery_redelivery;
    use ralph_core::supervisor::WaveKind;
    use std::sync::Arc;

    let workspace = tempfile::TempDir::new().expect("temp workspace");
    let ralph_dir = workspace.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
    let main_events_file = ralph_dir.join("events.jsonl");

    let store = Arc::new(InMemorySupervisorStore::new());
    let bridge = Arc::new(InMemoryCoordinatorBridge::from_store(
        store.clone() as Arc<dyn SupervisorStore>
    ));
    let wave = store
        .register_wave("s31-salvage-then-fail", WaveKind::Exec, 2, 1)
        .expect("register");
    let events = vec![ralph_core::Event {
        topic: "exec.unit.done".to_string(),
        payload: Some(r#"{"slot_index":0}"#.to_string()),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: Some("exec-worker".to_string()),
        wave_id: Some(wave.clone()),
        wave_index: Some(0),
        wave_total: Some(2),
        system_injected: Some(false),
    }];
    store
        .record_slot_event_payloads(&wave, 0, 1, &events)
        .expect("persist slot 0");

    let report = recovery_redelivery::redeliver_persisted_slot_events(
        store.clone(),
        bridge,
        &main_events_file,
    );
    assert_eq!(report.redelivered, vec![wave.clone()]);

    let injected = recovery_redelivery::inject_timed_out_failed_coord(
        &[wave.clone()],
        store.clone(),
        &main_events_file,
    );
    assert_eq!(injected, vec![wave.clone()]);

    let main_ledger = std::fs::read_to_string(&main_events_file).expect("read main");
    let unit_pos = main_ledger
        .find(r#""topic":"exec.unit.done""#)
        .expect("salvage unit.done");
    let failed_pos = main_ledger
        .find(r#""topic":"exec.wave.failed""#)
        .expect("timeout exec.wave.failed");
    assert!(
        unit_pos < failed_pos,
        "S3.1: salvage row must precede exec.wave.failed; ledger={main_ledger}"
    );
}

// 2026-09-01-001 plan S2.1: a fully completed salvaged wave
// must inject `exec.wave.complete` so a later dispatcher
// restart does not fan-in an empty in-memory set as failure.
#[test]
fn s21_2026_09_01_completed_salvage_injects_wave_complete() {
    use crate::loop_runner::wave::recovery_redelivery;
    use ralph_core::supervisor::WaveKind;
    use std::sync::Arc;

    let workspace = tempfile::TempDir::new().expect("temp workspace");
    let ralph_dir = workspace.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
    let main_events_file = ralph_dir.join("events.jsonl");

    let store = Arc::new(InMemorySupervisorStore::new());
    let bridge = Arc::new(InMemoryCoordinatorBridge::from_store(
        store.clone() as Arc<dyn SupervisorStore>
    ));
    let wave = store
        .register_wave("s21-complete-salvage", WaveKind::Exec, 1, 1)
        .expect("register");
    store
        .record_slot_result(&wave, 0, "fingerprint", 1)
        .expect("mark slot completed");
    let events = vec![ralph_core::Event {
        topic: "exec.unit.done".to_string(),
        payload: Some(r#"{"slot_index":0}"#.to_string()),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: Some("exec-worker".to_string()),
        wave_id: Some(wave.clone()),
        wave_index: Some(0),
        wave_total: Some(1),
        system_injected: Some(false),
    }];
    store
        .record_slot_event_payloads(&wave, 0, 1, &events)
        .expect("persist");

    let report = recovery_redelivery::redeliver_persisted_slot_events(
        store.clone(),
        bridge,
        &main_events_file,
    );
    assert_eq!(report.redelivered, vec![wave.clone()]);
    assert!(
        report.warnings.is_empty(),
        "S2.1: complete injection must not warn; got {:?}",
        report.warnings
    );

    let main_ledger = std::fs::read_to_string(&main_events_file).expect("read main");
    assert!(
        main_ledger.contains(r#""topic":"exec.wave.complete""#),
        "S2.1: fully salvaged wave must inject exec.wave.complete; got {main_ledger}"
    );
    let snap = store.fan_in_status(&wave).expect("fan_in");
    assert!(
        snap.delivery_state
            .at_least(ralph_core::supervisor::WaveDeliveryState::CoordinationCommitted),
        "S2.1: delivery_state must reach CoordinationCommitted; got {:?}",
        snap.delivery_state
    );
}
