use super::super::*;
use crate::loop_runner::wave::SupervisorBridge;
use ralph_core::supervisor::WaveKind;
use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};

use super::fixtures::*;

/// U2 验收 #3: after a reported failure the slot is handed to a NEW
/// backend process, running in the SAME worktree, whose prompt carries
/// the retry block describing attempt 1.
#[cfg(unix)]
#[tokio::test]
async fn executor_retry_uses_fresh_pid_same_cwd() {
    let (attempts, bridge, worktree, _tmp) =
        run_u2_fresh_process_wave("u2-fresh-pid", 2, |_| {}).await;

    assert_eq!(
        attempts.len(),
        2,
        "U2: a reported failure must be followed by exactly one more attempt"
    );
    assert_ne!(
        attempts[0].pid, attempts[1].pid,
        "U2: the retry must be a fresh process, not the same one resumed"
    );

    let expected_cwd = worktree.canonicalize().expect("canonicalize worktree");
    for (i, attempt) in attempts.iter().enumerate() {
        assert_eq!(
            attempt
                .cwd
                .canonicalize()
                .expect("canonicalize attempt cwd"),
            expected_cwd,
            "U2: attempt {} must run in the slot's worktree",
            i + 1
        );
    }

    assert!(
        !attempts[0].prompt.contains("# Retry Context"),
        "U2: the first attempt has no history and must not carry a retry block"
    );
    let retry_prompt = &attempts[1].prompt;
    assert!(
        retry_prompt.contains("# Retry Context"),
        "U2: the retry must be told it is a retry; got prompt:\n{retry_prompt}"
    );
    assert!(
        retry_prompt.contains("attempt **2/3**"),
        "U2: the retry must know which attempt it is; got prompt:\n{retry_prompt}"
    );
    assert!(
        retry_prompt.contains("executor_reported_failure"),
        "U2: the retry must see the stable code of attempt 1"
    );
    assert!(
        retry_prompt.contains("u2 first attempt left the unit tests red"),
        "U2: the retry must see the detail attempt 1 reported"
    );
    assert!(
        retry_prompt.contains(attempts[0].prompt.trim()),
        "U2: the retry block must be appended to the original prompt, not replace it"
    );

    assert_eq!(
        bridge.results_snapshot().len(),
        1,
        "U2: only the successful final attempt may be recorded"
    );
    assert!(
        bridge.failures_snapshot().is_empty(),
        "U2: a slot that succeeded on retry must not record a failure"
    );
}

/// U2 验收 #4: a worktree that already carries a commit from an earlier
/// attempt is NOT a success signal — the slot only completes because the
/// second process published its own terminal event.
#[cfg(unix)]
#[tokio::test]
async fn timeout_retry_does_not_claim_existing_commit_success() {
    let (attempts, bridge, _worktree, _tmp) =
        run_u2_fresh_process_wave("u2-existing-commit", 2, |worktree| {
            std::fs::write(worktree.join("done-by-attempt-1.txt"), "partial work")
                .expect("seed prior attempt output");
        })
        .await;

    assert_eq!(
        attempts.len(),
        2,
        "U2: pre-existing work must not short-circuit the retry"
    );
    assert_eq!(
        bridge.results_snapshot().len(),
        1,
        "U2: the slot completes only because attempt 2 published its own terminal"
    );
    assert!(
        attempts[1]
            .prompt
            .contains("Re-run this task's tests to find out what actually still fails"),
        "U2: the retry must be told to verify the existing state itself"
    );
}

/// U2 验收 #5: a third attempt is told about BOTH earlier failures, in
/// order, each with its own stable code — the reported failure of
/// attempt 1 and the timeout of attempt 2 (which leaves no detail).
#[tokio::test]
async fn third_attempt_prompt_contains_both_prior_failures() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(2);

    let wave = make_u3_wave("u2-third-attempt", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(0)).with_attempts(
        0,
        vec![
            exec_reported_failure("attempt 1 left the build broken"),
            U5SlotOutcome::Fail(U5_RETRYABLE_REASON.to_string()),
            U5SlotOutcome::Success(1),
        ],
    );

    let (_outcome, _bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    let prompts = exec.prompts_for(0);
    assert_eq!(prompts.len(), 3, "U2: budget=2 must allow three attempts");
    assert!(
        !prompts[0].contains("# Retry Context"),
        "U2: attempt 1 has no history"
    );

    let third = &prompts[2];
    assert!(
        third.contains("attempt **3/3**"),
        "U2: attempt 3 must know it is the last one; got:\n{third}"
    );
    let first_line = third
        .find("- attempt 1: failure code `executor_reported_failure`")
        .expect("U2: attempt 1's reported failure must survive into attempt 3");
    let second_line = third
        .find("- attempt 2: failure code `worker_timeout`")
        .expect("U2: attempt 2's timeout must be listed too");
    assert!(
        first_line < second_line,
        "U2: prior failures must be listed in ascending attempt order"
    );
    assert!(
        third.contains("reported detail: attempt 1 left the build broken"),
        "U2: attempt 1's detail must reach attempt 3"
    );
    assert!(
        third.contains("reported detail: unavailable"),
        "U2: a timeout leaves no detail, and that must be stated rather than guessed"
    );
    assert_eq!(
        third.matches("- attempt 1:").count(),
        1,
        "U2: the retry block must not stack across attempts"
    );
}

/// U3 验收 #6: a slot that uses its full per-worker budget on every
/// legal attempt must not be preempted by the wave's partial threshold.
///
/// The wave is `T=300s` per worker, 2 events at concurrency 1 and a
/// retry budget of 2, so the legal work budget is
/// `300 × 2 batches × 3 attempts + 30s = 1830s`. Each attempt here
/// burns 250s, for 1000s total — comfortably inside that budget but
/// well past the 504s partial threshold the pre-plan aggregate
/// (`300 × 2 + 30 = 630s`) would have produced.
#[tokio::test(start_paused = true)]
async fn healthy_worker_is_not_preempted_before_attempt_budget() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(2);

    let wave = make_u3_wave_with_concurrency("u3-healthy", 2, 2, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(1))
        .with_delay(Duration::from_secs(250))
        .with_attempts(
            0,
            vec![
                exec_reported_failure("attempt 1 still red"),
                exec_reported_failure("attempt 2 still red"),
                U5SlotOutcome::Success(1),
            ],
        );

    let (outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    assert_eq!(
        exec.call_count(0),
        3,
        "U3: the wave budget must cover all three legal attempts"
    );
    let completed = completed_wave_of(&outcome);
    assert_eq!(
        completed.results.len(),
        2,
        "U3: both slots must finish inside the attempt-aware budget, got {:?} / {:?}",
        completed.results,
        completed.failures
    );
    assert!(
        bridge.failures_snapshot().is_empty(),
        "U3: no slot may be recorded as failed, got {:?}",
        bridge.failures_snapshot()
    );
}

/// U4 验收 #2: with the `parallel-forge` budget of 2, a unit that keeps
/// reporting its own failure burns exactly three attempts, and only the
/// exhausted result reaches the wave level — as one `exec.wave.failed`
/// naming the slot as redrivable.
///
/// Phase 1 runs the real supervisor dispatch; phase 2 feeds the
/// `CompletedWave` it produced into the real fan-in, so the "three
/// attempts" and "one wave failure" facts are linked by the same
/// artifact rather than asserted independently.
#[tokio::test]
async fn parallel_forge_executor_retry_exhaustion() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::worker_outcome::REASON_EXECUTOR_REPORTED_FAILURE;

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let spy = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(2);
    let executor = U5RecordingExecutor::new(exec_reported_failure("unit still failing"));

    let (outcome, spy, exec) =
        run_u5_execute_wave(spy, make_u3_wave("forge-exhaust", 1, 1), executor).await;

    assert_eq!(
        exec.call_count(0),
        3,
        "U4: the parallel-forge budget must yield three total attempts"
    );
    let completed = completed_wave_of(&outcome);
    assert!(
        completed.results.is_empty(),
        "U4: an exhausted unit must not publish a business result, got {:?}",
        completed.results
    );
    assert_eq!(
        spy.failures_snapshot(),
        vec![(0u32, REASON_EXECUTOR_REPORTED_FAILURE.to_string())],
        "U4: exactly one stable slot failure, recorded only after exhaustion"
    );

    // Phase 2: the exhausted wave goes through the production fan-in.
    let (_tmp, bridge, _store, store_wave_id, events_path) =
        setup_u3_partial_failure_bridge(WaveKind::Exec, "forge-exhaust", 1);
    bridge
        .record_slot_failure(&store_wave_id, 0, REASON_EXECUTOR_REPORTED_FAILURE)
        .expect("record exhausted slot failure");
    bridge
        .commit_salvage_projection(
            &store_wave_id,
            &ralph_core::supervisor::ProjectionReceiptSummary {
                kind: ralph_core::supervisor::ProjectionKind::Business,
                batch_fingerprint: "forge-exhaust-fp".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let bridge: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge);
    let fan_in = run_supervisor_fan_in(
        &bridge,
        completed,
        &make_u3_wave("forge-exhaust", 1, 1),
        &events_path,
        600,
        None,
    );
    assert_eq!(
        fan_in,
        SupervisorFanInOutcome::InjectedFailed,
        "U4: an exhausted unit must reach the wave-level failure path"
    );

    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let failed: Vec<serde_json::Value> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("ledger line must be JSON"))
        .filter(|v: &serde_json::Value| {
            v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.failed")
        })
        .collect();
    assert_eq!(
        failed.len(),
        1,
        "U4: three attempts must produce exactly one exec.wave.failed, got {failed:?}"
    );

    let payload = failed[0].get("payload").expect("wave failure payload");
    let redrive: Vec<u64> = payload
        .get("redrive_slots")
        .and_then(|v| v.as_array())
        .expect("payload.redrive_slots")
        .iter()
        .filter_map(serde_json::Value::as_u64)
        .collect();
    assert_eq!(
        redrive,
        vec![0],
        "U4: the exhausted slot stays redrivable by an operator"
    );
    let slot_reason = payload
        .get("slot_failures")
        .and_then(|v| v.as_array())
        .and_then(|slots| slots.first())
        .and_then(|slot| slot.get("reason"))
        .and_then(|r| r.as_str());
    assert_eq!(
        slot_reason,
        Some(REASON_EXECUTOR_REPORTED_FAILURE),
        "U4: the wave failure must carry the stable exhaustion reason"
    );
}

/// U2 验收 #1: production supervisor bridge with a real
/// `ProductionBridgeContext.repo_root` — the spawned worker MUST
/// observe `RALPH_WORKSPACE_ROOT == repo_root` and
/// `RALPH_EVENTS_FILE == <validated absolute channel>` even when
/// the parent process has polluted values for both. The
/// `merge_event_channel_env` SSOT overrides whatever was in the
/// worker_backend env before validation.
#[tokio::test]
async fn test_u2_workspace_root_and_channel_injected_into_worker_env() {
    use crate::loop_runner::wave::CoordinatorSupervisorBridge;

    let tmp = tempfile::tempdir().expect("temp dir");
    // U2/007: canonicalize the tempdir so symlinked paths
    // (e.g. /var/tmp → /private/var/tmp on macOS) match the
    // canonicalized workspace root in the validator. Without
    // this, validate_control_plane_binding rejects every channel
    // it observes in CI on macOS.
    let workspace_root = std::fs::canonicalize(tmp.path()).expect("canonicalize workspace root");
    let wave_dir = workspace_root.join(".ralph");
    std::fs::create_dir_all(&wave_dir).expect("create wave dir");
    let main_events_file = wave_dir.join("events.jsonl");

    // U2/007: stub worktree factory — `DefaultWorktreeFactory`
    // would invoke the real `git worktree add`, but the tempdir
    // is not a git repo, so the bind would fail-closed and the
    // executor would never run. The stub returns a synthetic
    // worktree under the workspace root so the slot-subtree
    // validator has something to reject / accept against.
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
        loop_id: "u2-loop".to_string(),
        repo_root: workspace_root.clone(),
        events_path: Some(main_events_file.clone()),
        tasks_path: None,
    };
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        std::sync::Arc::new(StubFactory),
    );

    let wave = make_u3_wave("u2-routing", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(1));

    let capture = captured_env();
    capture.lock().unwrap().clear();
    let _outcome =
        run_u2_execute_wave_with_env_capture(bridge, wave, executor, &main_events_file, "u2-loop")
            .await;

    let snap = capture.lock().unwrap().clone();
    assert_eq!(snap.len(), 1, "U2/007: one slot captured; got {snap:?}");
    let env_map: std::collections::HashMap<String, String> = snap
        .get(&0)
        .expect("slot 0 captured")
        .iter()
        .cloned()
        .collect();

    let canonical_root = std::fs::canonicalize(&workspace_root).unwrap_or(workspace_root.clone());
    let expected_root = canonical_root.display().to_string();
    let observed_root = env_map
        .get("RALPH_WORKSPACE_ROOT")
        .expect("RALPH_WORKSPACE_ROOT must be injected")
        .clone();
    assert!(
        observed_root == expected_root || observed_root == workspace_root.display().to_string(),
        "U2/007: RALPH_WORKSPACE_ROOT must equal the production repo_root; expected={expected_root}, got={observed_root}"
    );

    // The per-worker channel (RALPH_EVENTS_FILE) is the validated
    // absolute path of the slot's per-worker JSONL (NOT the main
    // events file). The dispatcher builds
    // `<wave_dir>/wave-{id}-{index}.jsonl`, validates it, and the
    // SSOT injects the validated canonical path here.
    let observed_events = env_map
        .get("RALPH_EVENTS_FILE")
        .expect("RALPH_EVENTS_FILE must be injected")
        .clone();
    let observed_events_path = std::path::Path::new(&observed_events);
    assert!(
        observed_events_path.is_absolute(),
        "U2/007: RALPH_EVENTS_FILE must be an absolute path; got {observed_events}"
    );
    // The injected path MUST live under the validated workspace root.
    assert!(
        observed_events_path.starts_with(&canonical_root),
        "U2/007: RALPH_EVENTS_FILE must live under workspace_root; got {observed_events}, expected prefix {expected_root}"
    );
    // And it must NOT live inside the slot worktree (slot-subtree
    // rejection contract).
    assert!(
        !observed_events_path.starts_with(workspace_root.join("wt-")),
        "U2/007: RALPH_EVENTS_FILE must NOT live in slot subtree; got {observed_events}"
    );
}
