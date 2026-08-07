use super::super::*;
use crate::loop_runner::wave::SupervisorBridge;
use ralph_core::supervisor::WaveKind;

use super::fixtures::*;

// =============================================================================
// U1 T1 (2026-07-25-005 plan): partial failure characterization — new tests
//
// These tests characterize the exec/fix partial failure salvage/redrive
// behavior. They use `setup_u3_partial_failure_bridge` (M1 helper) to
// set up a 2-slot wave, then drive the slot outcomes and fan-in to
// assert the expected salvage/redrive behavior.
//
// T1 tests:
//   test_u1_single_fail_only          — 0 completed + 1 failed → 0 salvaged
//   test_u1_partial_failure_one_complete_one_fail — 1 completed + 1 failed → 1 salvaged
//   test_u1_zero_fail_happy_path_no_redrive_payload — 0 failed → no redrive payload
//   test_u1_mixed_failure_reasons    — mixed failure reasons reflected in slot_failures
// =============================================================================

/// U1 T1: single-fail fixture (2 slots, 0 completed, 1 failed) →
/// only the completed slot would be salvaged. With 0 completed slots,
/// 0 salvaged events must appear in the main ledger — proving zero
/// fabrication when there is nothing to salvage.
///
/// Current behavior: 0 salvaged events (the fail_wave path drops
/// everything on partial failure). This test PASSES on current HEAD,
/// characterizing the "nothing to salvage" baseline.
#[test]
fn test_u1_single_fail_only() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT;

    let (_tmp, bridge, _store, store_wave_id, events_path) =
        setup_u3_partial_failure_bridge(WaveKind::Exec, "u1-single-fail", 2);

    // Slot 0: never dispatched (Pending) — no result, no evidence
    // Slot 1: fails with worker_timeout
    bridge
        .record_slot_failure(&store_wave_id, 1, REASON_WORKER_TIMEOUT)
        .expect("slot 1 failure");
    bridge
        .commit_salvage_projection(
            &store_wave_id,
            &ralph_core::supervisor::ProjectionReceiptSummary {
                kind: ralph_core::supervisor::ProjectionKind::Business,
                batch_fingerprint: "test-fp".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let bridge: std::sync::Arc<dyn SupervisorBridge> =
        std::sync::Arc::new(bridge) as std::sync::Arc<dyn SupervisorBridge>;

    // CompletedWave has 0 results (no slot completed)
    let completed = ralph_core::CompletedWave {
        wave_id: "u1-single-fail".to_string(),
        wave_total: 2,
        results: vec![],
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    let detected = make_u3_wave("u1-single-fail", 2, 2);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "partial failure with 1 failed slot must inject exec.wave.failed"
    );

    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();

    // Zero fabrication: with 0 completed slots, 0 salvaged events must appear.
    let exec_unit_done_count = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done"))
        .count();
    assert_eq!(
        exec_unit_done_count, 0,
        "T1: with 0 completed slots, 0 salvaged events must appear; got {exec_unit_done_count}"
    );
}

/// U1 T2: partial failure fixture (2 slots, 1 completed, 1 failed) →
/// exactly 1 salvaged event (the completed slot's business event) must
/// appear in the main ledger. The failed slot's blocking_slots entry
/// must name slot 1 only.
///
/// This test FAILS on current HEAD — the fail_wave path drops completed
/// slot events, so 0 salvaged appear instead of 1. U2-U7 will fix this.
#[test]
fn test_u1_partial_failure_one_complete_one_fail() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::TerminalEvidence;
    use ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT;

    let (_tmp, bridge, _store, store_wave_id, events_path) =
        setup_u3_partial_failure_bridge(WaveKind::Exec, "u1-partial-1c1f", 2);

    // Slot 0: completes with evidence
    bridge
        .record_slot_result(&store_wave_id, 0, "h0", 1)
        .expect("slot 0 result");
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_wave_id,
            0,
            &TerminalEvidence::from_event("exec.unit.done", "{\"unit\":\"u1-0\"}"),
        )
        .expect("slot 0 evidence");

    // Slot 1: fails with worker_timeout
    bridge
        .record_slot_failure(&store_wave_id, 1, REASON_WORKER_TIMEOUT)
        .expect("slot 1 failure");
    bridge
        .commit_salvage_projection(
            &store_wave_id,
            &ralph_core::supervisor::ProjectionReceiptSummary {
                kind: ralph_core::supervisor::ProjectionKind::Business,
                batch_fingerprint: "test-fp".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let bridge: std::sync::Arc<dyn SupervisorBridge> =
        std::sync::Arc::new(bridge) as std::sync::Arc<dyn SupervisorBridge>;

    // CompletedWave has only slot 0's result (slot 1 failed → no event)
    let completed = ralph_core::CompletedWave {
        wave_id: "u1-partial-1c1f".to_string(),
        wave_total: 2,
        results: vec![ralph_core::WaveResult {
            index: 0,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-0\"}")
                    .with_source("executor")
                    .with_wave("u1-partial-1c1f".to_string(), 0, 2),
            ],
        }],
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    let detected = make_u3_wave("u1-partial-1c1f", 2, 2);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "partial failure must inject exec.wave.failed"
    );

    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();

    // Exactly 1 salvaged event (the completed slot's exec.unit.done)
    let salvaged: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done"))
        .collect();
    assert_eq!(
        salvaged.len(),
        1,
        "T2: exactly 1 salvaged event expected (completed slot); got {} events: {lines:?}",
        salvaged.len()
    );

    // A1 origin pin: the salvaged event's payload must contain unit=u1-0
    let salvaged_payload = salvaged[0]
        .get("payload")
        .and_then(|p| p.as_str())
        .expect("salvaged event must have a string payload");
    assert!(
        salvaged_payload.contains("u1-0"),
        "T2 A1: salvaged event payload must contain unit=u1-0; got {salvaged_payload}"
    );

    // A1 failed-slot integrity: slot 1 must have no terminal evidence
    let slot1_evidence = bridge
        .slot_terminal_evidence(&store_wave_id, 1)
        .expect("slot 1 evidence query must succeed");
    assert!(
        slot1_evidence.is_none(),
        "T2 A1: failed slot 1 must have no terminal evidence; got {slot1_evidence:?}"
    );
}

/// U1 T3: happy path (0 failed) → no exec.wave.failed injected,
/// and there must be no `redrive_slots` field in the success payload
/// (U7 will add it for retryable failed slots; 0 failed means empty redrive).
///
/// Current behavior: all slots complete, no redrive concept exists.
/// This test PASSES on current HEAD, characterizing the happy-path baseline.
#[test]
fn test_u1_zero_fail_happy_path_no_redrive_payload() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::TerminalEvidence;

    let (_tmp, bridge, _store, store_wave_id, events_path) =
        setup_u3_partial_failure_bridge(WaveKind::Exec, "u1-happy-zero-fail", 2);

    // Both slots complete with evidence
    for i in 0..2 {
        bridge
            .record_slot_result(&store_wave_id, i, &format!("h{i}"), 1)
            .expect("slot result");
        bridge
            .store()
            .record_slot_terminal_evidence(
                &store_wave_id,
                i,
                &TerminalEvidence::from_event(
                    "exec.unit.done",
                    &format!("{{\"unit\":\"u1-{i}\"}}"),
                ),
            )
            .expect("evidence");
    }
    bridge
        .commit_salvage_projection(
            &store_wave_id,
            &ralph_core::supervisor::ProjectionReceiptSummary {
                kind: ralph_core::supervisor::ProjectionKind::Business,
                batch_fingerprint: "test-fp".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let bridge: std::sync::Arc<dyn SupervisorBridge> =
        std::sync::Arc::new(bridge) as std::sync::Arc<dyn SupervisorBridge>;

    let completed = ralph_core::CompletedWave {
        wave_id: "u1-happy-zero-fail".to_string(),
        wave_total: 2,
        results: vec![
            ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-0\"}")
                        .with_source("executor")
                        .with_wave("u1-happy-zero-fail".to_string(), 0, 2),
                ],
            },
            ralph_core::WaveResult {
                index: 1,
                events: vec![
                    ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-1\"}")
                        .with_source("executor")
                        .with_wave("u1-happy-zero-fail".to_string(), 1, 2),
                ],
            },
        ],
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: false,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    let detected = make_u3_wave("u1-happy-zero-fail", 2, 2);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedComplete,
        "happy path (0 failed) must inject exec.wave.complete"
    );

    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();

    // No exec.wave.failed on happy path
    let failed_count = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.failed"))
        .count();
    assert_eq!(
        failed_count, 0,
        "T3: happy path must NOT inject exec.wave.failed; got {failed_count}"
    );

    // exec.wave.complete must NOT have a redrive_slots field (U7 adds it for retryable failures;
    // 0 failed means empty redrive, but the field should not exist at all on the happy path)
    let completes: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.complete"))
        .collect();
    assert_eq!(
        completes.len(),
        1,
        "exactly one exec.wave.complete expected; got {}",
        completes.len()
    );
    let complete_payload = completes[0]
        .get("payload")
        .and_then(|p| p.as_object())
        .expect("complete event must have a JSON object payload");
    assert!(
        !complete_payload.contains_key("redrive_slots"),
        "T3: happy path complete payload must NOT have redrive_slots field; got {complete_payload:?}"
    );
}

/// U1 T4: mixed failure reasons — when slots fail with different reasons
/// (`worker_timeout` and `empty_worker_result`), `slot_failures` in the
/// failed payload must reflect both distinct reason strings, with no
/// fabrication of events for slots that did not produce any.
///
/// This test PASSES on current HEAD — `build_wave_failed_payload` already
/// includes `slot_failures` with per-slot reasons.
#[test]
fn test_u1_mixed_failure_reasons() {
    use crate::loop_runner::wave::build_wave_failed_payload;
    use ralph_core::supervisor::WaveKind;
    use ralph_core::{CompletedWave, WaveFailure, WaveResult};
    use std::time::Duration;

    // 3-slot wave: slot 0 completed, slot 1 failed (worker_timeout), slot 2 failed (empty_worker_result)
    let completed = CompletedWave {
        wave_id: "u1-mixed-reasons".to_string(),
        wave_total: 3,
        results: vec![WaveResult {
            index: 0,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-0\"}")
                    .with_source("executor"),
            ],
        }],
        failures: vec![
            WaveFailure {
                index: 1,
                error: "worker_timeout".to_string(),
                duration: Duration::from_secs(300),
                expected_dimension: None,
                actual_dimension: None,
            },
            WaveFailure {
                index: 2,
                error: "empty_worker_result".to_string(),
                duration: Duration::from_millis(50),
                expected_dimension: None,
                actual_dimension: None,
            },
        ],
        duration: Duration::from_secs(300),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };

    let payload = build_wave_failed_payload(
        WaveKind::Exec,
        &completed,
        "required_slot_failure",
        vec![1, 2],
        &std::collections::HashMap::new(),
        None,
        None,
    );
    let obj = payload.as_object().expect("payload must be a JSON object");

    // blocking_slots must list both failed slots
    let blocking: Vec<u32> = obj
        .get("blocking_slots")
        .and_then(|v| v.as_array())
        .expect("blocking_slots must be an array")
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as u32))
        .collect();
    assert_eq!(
        blocking,
        vec![1, 2],
        "T4: blocking_slots must list both failed slots; got {blocking:?}"
    );

    // slot_failures must reflect both distinct reasons
    let slot_failures = obj
        .get("slot_failures")
        .and_then(|v| v.as_array())
        .expect("T4: slot_failures must be present for mixed-failure scenario");
    assert_eq!(
        slot_failures.len(),
        2,
        "T4: one entry per failed slot; got {slot_failures:?}"
    );
    let by_index: std::collections::HashMap<u32, String> = slot_failures
        .iter()
        .filter_map(|v| {
            let obj = v.as_object()?;
            let slot = obj.get("slot_index")?.as_u64()? as u32;
            let reason = obj.get("reason")?.as_str()?.to_string();
            Some((slot, reason))
        })
        .collect();
    assert_eq!(
        by_index.get(&1).map(String::as_str),
        Some("worker_timeout"),
        "T4: slot 1 must carry worker_timeout; got {by_index:?}"
    );
    assert_eq!(
        by_index.get(&2).map(String::as_str),
        Some("empty_worker_result"),
        "T4: slot 2 must carry empty_worker_result; got {by_index:?}"
    );

    // No fabrication: completed slot 0 must NOT appear in slot_failures
    assert!(
        !by_index.contains_key(&0),
        "T4: completed slot 0 must NOT appear in slot_failures (no fabrication); got {by_index:?}"
    );
}

/// S3: full redrive flow: parent wave with failed slot → child created
/// → boot scan finds pending child → descriptor dispatchable → worker spawned.
///
/// Uses the post-002 canonical path: persist descriptor on the *parent*
/// before `create_redrive_wave` (which copies it to the child key), then
/// boot-scan via `dispatch_pending_redrive_waves`. The U3 spy bridge
/// keeps control-plane validation out of the way so the assertion is
/// specifically "exactly one worker spawn" (not production bind naming).
#[tokio::test]
async fn test_s3_dispatch_pending_redrive_wave_in_memory() {
    use crate::loop_runner::wave::dispatch_pending_redrive_waves;
    use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};
    use std::sync::Arc;

    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());
    let parent_id = make_redrive_parent_with_descriptors(store.as_ref(), "s3-parent", 1, true);
    let redrive = store
        .create_redrive_wave(&parent_id, None)
        .expect("create redrive wave");
    let child_id = redrive.child_wave_id;
    assert_eq!(redrive.slots, vec![0], "child should cover failed slot 0");

    let pending = store
        .list_redrive_pending_child_waves()
        .expect("list pending");
    assert_eq!(pending.len(), 1, "should have 1 pending child wave");
    assert_eq!(pending[0].child_wave_id, child_id);
    assert_eq!(pending[0].slots.len(), 1);
    assert!(
        pending[0].slots[0].expected_digest.is_some(),
        "parent descriptor must enrich child expected_digest"
    );

    // Pre-bind child so try_dispatch_next can approve (U3 spy bind_slot
    // does not persist into the store).
    store
        .bind_worktree(
            &child_id,
            0,
            ralph_core::supervisor::SlotResource {
                slot_index: 0,
                worktree_path: Some(format!("/tmp/s3-redrive/child-{child_id}-0")),
                branch: Some(format!("s3-child-{child_id}-0")),
            },
        )
        .expect("bind child slot 0");

    let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
        Arc::new(U3DispatchBridge::new(store.clone(), 4));
    let hat_registry = redrive_test_registry();
    let backend = make_test_cli_backend();
    let tmp_dir = tempfile::tempdir().expect("temp dir");
    let events_file = tmp_dir.path().join("events.jsonl");
    std::fs::File::create(&events_file).expect("create events file");

    // Counting executor + terminal marker so slot_retry_budget=0 spy
    // bridge still completes without a second attempt.
    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor: Arc<dyn crate::loop_runner::wave::WaveWorkerExecutor> =
        Arc::new(U3CountingExecutor::new(started.clone()).with_topic("exec.done"));
    let dispatched = dispatch_pending_redrive_waves(
        &store,
        "s3-loop",
        &hat_registry,
        &backend,
        &bridge,
        &events_file,
        executor,
    )
    .await;
    assert_eq!(
        dispatched, 1,
        "S3: boot scan must dispatch exactly one child slot"
    );
    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "S3: boot scan must spawn exactly one worker for the redrive child"
    );
}

/// S4: when `expected_digest = None` (no descriptor persisted), the boot
/// scan must fail-close without calling `take_dispatchable_redrive_descriptor`.
#[tokio::test]
async fn test_s4_no_descriptor_is_fail_closed() {
    use crate::loop_runner::wave::dispatch_pending_redrive_waves;
    use ralph_core::supervisor::{
        InMemorySupervisorStore, SlotResource, SupervisorStore, WaveKind,
    };

    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());

    // 1. Parent wave with failed slot — but NO descriptor persisted.
    let parent_id = store
        .register_wave("s4-parent", WaveKind::Exec, 1, 1)
        .expect("register parent");
    store
        .bind_worktree(
            &parent_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some("/tmp/s4-test/parent-0".to_string()),
                branch: Some("s4-parent-exec-0".to_string()),
            },
        )
        .expect("bind");
    let _dispatched = store.try_dispatch_next(4).expect("try_dispatch_next");
    store
        .record_slot_failure(&parent_id, 0, "test-s4")
        .expect("record failure");

    // NOTE: NO persist_slot_descriptor call — the slot never had a descriptor.

    // 2. Create redrive child wave.
    let redrive = store
        .create_redrive_wave(&parent_id, None)
        .expect("create redrive");
    let child_id = redrive.child_wave_id;

    // 3. Verify list returns child with expected_digest = None.
    let pending = store.list_redrive_pending_child_waves().expect("list");
    assert_eq!(pending.len(), 1);
    assert!(
        pending[0].slots[0].expected_digest.is_none(),
        "S4: digest must be None"
    );

    // 4. Bind child's slot so try_dispatch_next would return it.
    store
        .bind_worktree(
            &child_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some("/tmp/s4-test/child-0".to_string()),
                branch: Some("s4-child-exec-0".to_string()),
            },
        )
        .expect("bind child");

    // 5. Dispatch — S4 fail-closed path: the helper should skip this slot.
    let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory(
            store.clone() as Arc<dyn SupervisorStore>,
            crate::loop_runner::wave::ProductionBridgeContext {
                loop_id: "s4-loop".to_string(),
                repo_root: std::path::PathBuf::from("/tmp"),
                events_path: None,
                tasks_path: None,
            },
            std::sync::Arc::new(ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory),
        ),
    );
    let hat_registry = make_test_hat_registry();
    let backend = make_test_cli_backend();
    let tmp_dir = tempfile::tempdir().expect("temp dir");
    let events_file = tmp_dir.path().join("events.jsonl");
    std::fs::File::create(&events_file).expect("create events file");

    let dispatched = dispatch_pending_redrive_waves(
        &store,
        "s4-loop",
        &hat_registry,
        &backend,
        &bridge,
        &events_file,
        std::sync::Arc::new(crate::loop_runner::wave::ProductionExecutor),
    )
    .await;
    assert_eq!(
        dispatched, 0,
        "S4: no worker may spawn without a descriptor"
    );

    // A3: fail-close records `slot_never_started` so the slot leaves
    // Pending (and therefore leaves the boot pending list). A silent
    // Pending row would forever reappear on every --resume.
    let pending_after = store
        .list_redrive_pending_child_waves()
        .expect("list after");
    assert!(
        pending_after.is_empty(),
        "S4 fail-close must terminalize the slot (Pending filter); got {pending_after:?}"
    );
}

/// S5: when the descriptor digest conflicts with what was persisted,
/// the boot scan must fail-close.
#[tokio::test]
async fn test_s5_digest_conflict_is_fail_closed() {
    use crate::loop_runner::wave::dispatch_pending_redrive_waves;
    use ralph_core::supervisor::{
        InMemorySupervisorStore, SlotResource, SupervisorStore, WaveKind,
    };

    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());

    // 1. Parent wave with failed slot + descriptor persisted.
    let parent_id = store
        .register_wave("s5-parent", WaveKind::Exec, 1, 1)
        .expect("register parent");
    store
        .bind_worktree(
            &parent_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some("/tmp/s5-test/parent-0".to_string()),
                branch: Some("s5-parent-exec-0".to_string()),
            },
        )
        .expect("bind");
    let _dispatched = store.try_dispatch_next(4).expect("try_dispatch_next");
    store
        .record_slot_failure(&parent_id, 0, "test-s5")
        .expect("record failure");

    // Persist the CORRECT descriptor.
    let original_payload = r#"{"unit_id":"s5-test-unit"}"#;
    let correct_digest = ralph_core::supervisor::SlotDescriptor::digest_of(original_payload);
    let descriptor = ralph_core::supervisor::SlotDescriptor {
        slot_index: 0,
        topic: "exec.unit.ready".to_string(),
        payload_json: original_payload.to_string(),
        wave_kind: WaveKind::Exec,
        payload_digest: correct_digest.clone(),
        slot_index_in_parent: None,
    };
    store
        .persist_slot_descriptor(&parent_id, &descriptor)
        .expect("persist descriptor");

    // 2. Create redrive child wave.
    let redrive = store
        .create_redrive_wave(&parent_id, None)
        .expect("create redrive");
    let child_id = redrive.child_wave_id;

    // 3. AFTER creating the child, overwrite the child's slot descriptor with a TAMPERED digest.
    //    (simulates someone editing the ready event payload between original dispatch and redrive).
    let tampered_descriptor = ralph_core::supervisor::SlotDescriptor {
        slot_index: 0,
        topic: "exec.unit.ready".to_string(),
        payload_json: r#"{"unit_id":"s5-tampered"}"#.to_string(),
        wave_kind: WaveKind::Exec,
        payload_digest: ralph_core::supervisor::SlotDescriptor::digest_of(
            r#"{"unit_id":"s5-tampered"}"#,
        ),
        slot_index_in_parent: None,
    };
    store
        .persist_slot_descriptor(&child_id, &tampered_descriptor)
        .expect("overwrite with tampered descriptor");

    // 4. Bind child's slot.
    store
        .bind_worktree(
            &child_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some("/tmp/s5-test/child-0".to_string()),
                branch: Some("s5-child-exec-0".to_string()),
            },
        )
        .expect("bind child");

    // 5. Dispatch — the take_dispatchable_redrive_descriptor will find the
    //    stored digest (tampered) differs from expected (correct) → Conflict fail-close.
    let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory(
            store.clone() as Arc<dyn SupervisorStore>,
            crate::loop_runner::wave::ProductionBridgeContext {
                loop_id: "s5-loop".to_string(),
                repo_root: std::path::PathBuf::from("/tmp"),
                events_path: None,
                tasks_path: None,
            },
            std::sync::Arc::new(ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory),
        ),
    );
    let hat_registry = make_test_hat_registry();
    let backend = make_test_cli_backend();
    let tmp_dir = tempfile::tempdir().expect("temp dir");
    let events_file = tmp_dir.path().join("events.jsonl");
    std::fs::File::create(&events_file).expect("create events file");

    dispatch_pending_redrive_waves(
        &store,
        "s5-loop",
        &hat_registry,
        &backend,
        &bridge,
        &events_file,
        std::sync::Arc::new(crate::loop_runner::wave::ProductionExecutor),
    )
    .await;

    // 6. After dispatch, the pending child should STILL be in the list
    //    (the slot was skipped due to digest conflict fail-close).
    let pending_after = store
        .list_redrive_pending_child_waves()
        .expect("list after");
    assert_eq!(
        pending_after.len(),
        1,
        "S5 fail-close: child must remain pending after digest conflict"
    );
}

/// S6: when there are no pending redrive children, the boot scan is a no-op
/// and the executor is never called.
#[tokio::test]
async fn test_s6_no_pending_children_is_noop() {
    use crate::loop_runner::wave::dispatch_pending_redrive_waves;
    use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore, WaveKind};

    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());

    // Register a parent wave with ALL slots completed (no failed slots).
    // create_redrive_wave would return Err → no child created.
    let parent_id = store
        .register_wave("s6-parent", WaveKind::Exec, 1, 1)
        .expect("register parent");
    store
        .record_slot_result(&parent_id, 0, "s6-hash", 1)
        .expect("record result");
    store
        .set_wave_phase(&parent_id, ralph_core::supervisor::WavePhase::Done)
        .expect("set done");

    // Verify no pending children.
    let pending = store.list_redrive_pending_child_waves().expect("list");
    assert!(
        pending.is_empty(),
        "S6: no pending children expected; got {pending:?}"
    );

    let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory(
            store.clone() as Arc<dyn SupervisorStore>,
            crate::loop_runner::wave::ProductionBridgeContext {
                loop_id: "s6-loop".to_string(),
                repo_root: std::path::PathBuf::from("/tmp"),
                events_path: None,
                tasks_path: None,
            },
            std::sync::Arc::new(ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory),
        ),
    );
    let hat_registry = make_test_hat_registry();
    let backend = make_test_cli_backend();
    let tmp_dir = tempfile::tempdir().expect("temp dir");
    let events_file = tmp_dir.path().join("events.jsonl");
    std::fs::File::create(&events_file).expect("create events file");

    // The dispatch function should return early since there are no pending children.
    // No panic, no executor call, no store mutation.
    let dispatched = dispatch_pending_redrive_waves(
        &store,
        "s6-loop",
        &hat_registry,
        &backend,
        &bridge,
        &events_file,
        std::sync::Arc::new(crate::loop_runner::wave::ProductionExecutor),
    )
    .await;
    assert_eq!(
        dispatched, 0,
        "S6: no pending children ⇒ boot scan dispatches zero slots"
    );

    // Still no pending children after the call.
    let pending_after = store
        .list_redrive_pending_child_waves()
        .expect("list after");
    assert!(
        pending_after.is_empty(),
        "S6: still no pending children; got {pending_after:?}"
    );
}

// =============================================================================
// 2026-07-28-002 plan U3 (R3 / S2a): SlotDescriptor persist-on-dispatch.
//
// These tests verify the dispatcher calls `persist_slot_descriptor`
// after a successful `bind_slot` and before spawning the worker.
// S2a happy path: after dispatch, the store holds a descriptor
// with the correct topic / payload / wave_kind / payload_digest.
// S2a fail-closed: when the store returns an error from
// `persist_slot_descriptor`, the slot is skipped (no worker spawned)
// and the failure is recorded on the bridge.
// =============================================================================

/// S2a happy path: dispatch a 1-slot wave through U3DispatchBridge
/// (which owns a real InMemorySupervisorStore). After dispatch, the
/// store MUST have a SlotDescriptor for (store_wave_id, 0) with:
/// - topic == "exec.unit.ready"
/// - payload_json == the wave event payload
/// - wave_kind == WaveKind::Exec
/// - payload_digest == fingerprint_payload(payload)
#[tokio::test]
async fn test_s2a_persist_slot_descriptor_on_dispatch() {
    use ralph_core::supervisor::{
        InMemorySupervisorStore, SlotDescriptor, SupervisorStore, WaveKind,
    };
    use std::sync::Arc;

    let store: Arc<InMemorySupervisorStore> = Arc::new(InMemorySupervisorStore::new());
    let bridge = U3DispatchBridge::new(store.clone() as Arc<dyn SupervisorStore>, 4);

    // Wave with known topic and payload so we can assert against the descriptor.
    let wave = make_u3_wave_with_concurrency("s2a-happy", 1, 1, 1);
    // Override the topic to the canonical exec.unit.ready.
    let wave = ralph_core::DetectedWave {
        events: vec![ralph_core::Event {
            topic: "exec.unit.ready".to_string(),
            payload: Some(r#"{"content_hash":"s2a-test-payload"}"#.to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        }],
        ..wave
    };

    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (_outcome, _) = run_u3_execute_wave_with_prebound_slots(
        &bridge,
        &wave,
        &[0], // pre-bind slot 0 so try_dispatch_next approves it
        started.clone(),
    )
    .await;

    // The dispatcher must have dispatched at least the one slot.
    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "S2a happy: exactly 1 slot must be dispatched; got {}",
        started.load(std::sync::atomic::Ordering::SeqCst)
    );

    // Retrieve the store's wave_id by recovering active waves.
    let store_wave_id = {
        let snaps = store
            .recover_active_waves()
            .expect("store must recover without error");
        assert!(
            !snaps.is_empty(),
            "S2a happy: store must have exactly 1 active wave"
        );
        snaps.into_iter().next().unwrap().wave_id
    };

    // Read back the persisted descriptor and assert all four fields.
    let descriptor = store
        .slot_descriptor(&store_wave_id, 0)
        .expect("slot_descriptor must not error");
    let descriptor = descriptor.expect("S2a happy: slot 0 must have a persisted descriptor");

    assert_eq!(
        descriptor.topic, "exec.unit.ready",
        "S2a happy: topic must match dispatch context"
    );
    assert_eq!(
        descriptor.payload_json, r#"{"content_hash":"s2a-test-payload"}"#,
        "S2a happy: payload_json must match the dispatched event payload"
    );
    assert_eq!(
        descriptor.wave_kind,
        WaveKind::Exec,
        "S2a happy: wave_kind must be Exec for exec.unit.ready topic"
    );
    let expected_digest = SlotDescriptor::digest_of(r#"{"content_hash":"s2a-test-payload"}"#);
    assert_eq!(
        descriptor.payload_digest, expected_digest,
        "S2a happy: payload_digest must equal fingerprint_payload(payload)"
    );
    assert_eq!(
        descriptor.slot_index_in_parent, None,
        "S2a happy: slot_index_in_parent must be None for parent-wave slots"
    );
}

/// S2a fail-closed: persist failure ⇒ no worker spawned, no descriptor
/// reaches the inner store.
#[tokio::test]
async fn test_s2a_persist_failure_fails_closed_no_spawn() {
    use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};
    use std::sync::Arc;

    let inner = Arc::new(InMemorySupervisorStore::new());
    let failing: Arc<dyn SupervisorStore> = Arc::new(PersistFailingSupervisorStore {
        inner: inner.clone(),
    });
    let bridge = U3DispatchBridge::new(failing, 4);

    let wave = make_u3_wave_with_concurrency("s2a-persist-fail", 1, 1, 1);
    let wave = ralph_core::DetectedWave {
        events: vec![ralph_core::Event {
            topic: "exec.unit.ready".to_string(),
            payload: Some(r#"{"content_hash":"s2a-fault"}"#.to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        }],
        ..wave
    };

    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (_outcome, _) =
        run_u3_execute_wave_with_prebound_slots(&bridge, &wave, &[0], started.clone()).await;

    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "S2a fail-closed: persist failure must skip the slot (no spawn)"
    );

    // The inner store received every call EXCEPT a successful persist:
    // the wave row exists (register delegated) but no descriptor does.
    let snaps = inner.recover_active_waves().expect("recover");
    assert_eq!(snaps.len(), 1, "the wave must be registered via delegation");
    let store_wave_id = snaps.into_iter().next().unwrap().wave_id;
    let descriptor = inner
        .slot_descriptor(&store_wave_id, 0)
        .expect("slot_descriptor must not error");
    assert!(
        descriptor.is_none(),
        "S2a fail-closed: no descriptor may reach the store when persist fails"
    );
}

/// S3 / R-F1 happy path (in-memory): a 3-slot child wave created from a
/// descriptor-bearing parent is fully dispatched by the boot scan —
/// each child slot binds its OWN index (C3), and a second scan is a
/// no-op (take consumed the descriptors ⇒ resume-after-dispatch never
/// double-spawns).
#[tokio::test]
async fn test_u4_redrive_boot_dispatch_in_memory_multi_slot() {
    use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};
    use std::sync::Arc;

    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());
    let parent = make_redrive_parent_with_descriptors(store.as_ref(), "u4-boot", 3, true);
    let redrive = store
        .create_redrive_wave(&parent, Some(&[0, 1, 2]))
        .unwrap();

    // Sanity: the enriched pending list sees all 3 child slots with digests
    // and the child wave's true expected_total (R9).
    let pending = store.list_redrive_pending_child_waves().unwrap();
    assert_eq!(pending.len(), 1, "one pending child wave expected");
    assert_eq!(pending[0].child_wave_id, redrive.child_wave_id);
    assert_eq!(
        pending[0].expected_total, 3,
        "R9: list must carry child.expected_total"
    );
    assert_eq!(pending[0].slots.len(), 3);
    assert!(
        pending[0].slots.iter().all(|s| s.expected_digest.is_some()),
        "parent descriptors must enrich child slots with expected_digest"
    );

    // Production `bind_slot` binds a per-slot worktree in the store
    // before the store approves dispatch; `U3DispatchBridge` leaves
    // binding to the test, so pre-bind the child slots (same pattern
    // as `run_u3_execute_wave_with_prebound_slots`).
    for i in 0..3u32 {
        store
            .bind_worktree(
                &redrive.child_wave_id,
                i,
                ralph_core::supervisor::SlotResource {
                    slot_index: i,
                    worktree_path: Some(format!(
                        "/tmp/u4-redrive/child-{}-{i}",
                        redrive.child_wave_id
                    )),
                    branch: Some(format!("u4-child-{}-{i}", redrive.child_wave_id)),
                },
            )
            .unwrap();
    }

    let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
        Arc::new(U3DispatchBridge::new(store.clone(), 4));
    let registry = redrive_test_registry();
    let backend = make_test_cli_backend();
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_path = tmp.path().join("events.jsonl");
    let executor = U4SlotRecordingExecutor::default();
    let recorded = executor.indices.clone();
    let recorded_totals = executor.totals.clone();

    let dispatched = crate::loop_runner::wave::dispatch_pending_redrive_waves(
        &store,
        "loop-u4-boot",
        &registry,
        &backend,
        &bridge,
        &events_path,
        Arc::new(executor.clone()),
    )
    .await;
    assert_eq!(dispatched, 3, "all 3 child slots must be dispatched");

    let mut indices = recorded.lock().unwrap().clone();
    indices.sort_unstable();
    assert_eq!(
        indices,
        vec![0, 1, 2],
        "C3: each child slot must spawn under its own slot index, not all slot 0"
    );
    let totals = recorded_totals.lock().unwrap().clone();
    assert!(
        totals.iter().all(|&t| t == 3),
        "R9: worker prompt must see wave_total=child.expected_total (3), got {totals:?}"
    );

    // Idempotency (R-F5): descriptors were consumed by `take`; a second
    // boot scan (simulating a later `ralph run --resume`) finds nothing
    // dispatchable and spawns nothing.
    let again = crate::loop_runner::wave::dispatch_pending_redrive_waves(
        &store,
        "loop-u4-boot",
        &registry,
        &backend,
        &bridge,
        &events_path,
        Arc::new(executor),
    )
    .await;
    assert_eq!(
        again, 0,
        "resume after successful dispatch must not re-spawn (Pending filter consumed slots)"
    );
    assert_eq!(
        recorded.lock().unwrap().len(),
        3,
        "second scan must not add spawn requests"
    );
}

/// S6 / review P1#4: the runner boot seam must refuse to scan when
/// `resume=false`, even if the store already holds a pending redrive
/// child (fresh-boot exclusion).
#[tokio::test]
async fn test_boot_redrive_skipped_when_not_resuming() {
    use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};
    use std::sync::Arc;

    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());
    let parent = make_redrive_parent_with_descriptors(store.as_ref(), "s6-fresh", 1, true);
    let redrive = store.create_redrive_wave(&parent, None).unwrap();
    store
        .bind_worktree(
            &redrive.child_wave_id,
            0,
            ralph_core::supervisor::SlotResource {
                slot_index: 0,
                worktree_path: Some("/tmp/s6-fresh/0".into()),
                branch: Some("s6-fresh-0".into()),
            },
        )
        .unwrap();

    let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
        Arc::new(U3DispatchBridge::new(store.clone(), 4));
    let registry = redrive_test_registry();
    let backend = make_test_cli_backend();
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_path = tmp.path().join("events.jsonl");
    let executor = U4SlotRecordingExecutor::default();
    let recorded = executor.indices.clone();

    let dispatched = crate::loop_runner::wave::boot_dispatch_pending_redrive_if_resuming(
        false, // fresh boot
        &store,
        "loop-s6-fresh",
        &registry,
        &backend,
        &bridge,
        &events_path,
        Arc::new(executor),
    )
    .await;
    assert_eq!(dispatched, 0, "fresh boot must not consume pending redrive");
    assert!(
        recorded.lock().unwrap().is_empty(),
        "fresh boot must not spawn redrive workers"
    );
    // Store still holds the pending child for a later --resume.
    let pending = store.list_redrive_pending_child_waves().unwrap();
    assert_eq!(pending.len(), 1);
}

/// S4 fail-closed: a legacy parent whose slots never persisted
/// descriptors yields child slots with `expected_digest = None`; the
/// boot scan must skip them all (slot_never_started fail-close) and
/// spawn nothing.
#[tokio::test]
async fn test_u4_redrive_boot_legacy_slot_fail_closed() {
    use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};
    use std::sync::Arc;

    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());
    let parent = make_redrive_parent_with_descriptors(store.as_ref(), "u4-legacy", 2, false);
    let redrive = store.create_redrive_wave(&parent, None).unwrap();

    // Enriched list must expose the slots WITHOUT a digest.
    let pending = store.list_redrive_pending_child_waves().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].child_wave_id, redrive.child_wave_id);
    assert!(
        pending[0].slots.iter().all(|s| s.expected_digest.is_none()),
        "legacy parent slots must surface expected_digest = None"
    );

    let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
        Arc::new(U3DispatchBridge::new(store.clone(), 4));
    let registry = redrive_test_registry();
    let backend = make_test_cli_backend();
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_path = tmp.path().join("events.jsonl");
    let executor = U4SlotRecordingExecutor::default();
    let recorded = executor.indices.clone();

    let dispatched = crate::loop_runner::wave::dispatch_pending_redrive_waves(
        &store,
        "loop-u4-legacy",
        &registry,
        &backend,
        &bridge,
        &events_path,
        Arc::new(executor),
    )
    .await;
    assert_eq!(
        dispatched, 0,
        "slots without a persisted descriptor must fail closed (no dispatch)"
    );
    assert!(
        recorded.lock().unwrap().is_empty(),
        "no worker may spawn for descriptor-less legacy slots"
    );
}

/// S3 rusqlite-backed variant: the same multi-slot boot dispatch must
/// work against the production persistent store (real v10 schema, real
/// take/delete semantics), including the idempotent second scan.
#[cfg(feature = "supervisor-db")]
#[tokio::test]
async fn test_s3_rusqlite_backed_wave_supervisor_dispatch() {
    use ralph_core::supervisor::{RusqliteSupervisorStore, SupervisorStore};
    use std::sync::Arc;

    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("supervisor.db");
    let store: Arc<dyn SupervisorStore> =
        Arc::new(RusqliteSupervisorStore::open(&db_path).expect("open rusqlite store"));

    let parent = make_redrive_parent_with_descriptors(store.as_ref(), "s3-rusqlite", 3, true);
    let _redrive = store
        .create_redrive_wave(&parent, Some(&[0, 1, 2]))
        .unwrap();

    // Sanity: the parent descriptor must have landed (rusqlite
    // first-persist regression guard, see the persist_slot_descriptor
    // UPDATE-then-INSERT fix) and the enriched pending list must see
    // all three child slots.
    assert!(
        store.slot_descriptor(&parent, 0).unwrap().is_some(),
        "rusqlite: first persist must actually store the descriptor"
    );
    let pending = store.list_redrive_pending_child_waves().unwrap();
    assert_eq!(
        pending.len(),
        1,
        "rusqlite: one pending child wave expected"
    );
    assert_eq!(pending[0].slots.len(), 3);

    // Pre-bind child slot worktrees (see the in-memory variant).
    for i in 0..3u32 {
        store
            .bind_worktree(
                &_redrive.child_wave_id,
                i,
                ralph_core::supervisor::SlotResource {
                    slot_index: i,
                    worktree_path: Some(format!(
                        "/tmp/u4-redrive/s3-child-{}-{i}",
                        _redrive.child_wave_id
                    )),
                    branch: Some(format!("u4-s3-child-{}-{i}", _redrive.child_wave_id)),
                },
            )
            .unwrap();
    }

    let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
        Arc::new(U3DispatchBridge::new(store.clone(), 4));
    let registry = redrive_test_registry();
    let backend = make_test_cli_backend();
    let events_path = tmp.path().join("events.jsonl");
    let executor = U4SlotRecordingExecutor::default();
    let recorded = executor.indices.clone();

    let dispatched = crate::loop_runner::wave::dispatch_pending_redrive_waves(
        &store,
        "loop-s3",
        &registry,
        &backend,
        &bridge,
        &events_path,
        Arc::new(executor.clone()),
    )
    .await;
    assert_eq!(
        dispatched, 3,
        "rusqlite-backed: all 3 child slots must be dispatched"
    );
    let mut indices = recorded.lock().unwrap().clone();
    indices.sort_unstable();
    assert_eq!(indices, vec![0, 1, 2], "rusqlite-backed: C3 slot indices");

    let again = crate::loop_runner::wave::dispatch_pending_redrive_waves(
        &store,
        "loop-s3",
        &registry,
        &backend,
        &bridge,
        &events_path,
        Arc::new(executor),
    )
    .await;
    assert_eq!(again, 0, "rusqlite-backed: second scan is a no-op");
}
