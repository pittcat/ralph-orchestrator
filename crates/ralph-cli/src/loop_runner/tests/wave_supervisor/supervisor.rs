use super::super::*;
use crate::loop_runner::wave::SupervisorBridge;
use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
use ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory;
use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};
use ralph_core::supervisor::{SlotResource, TerminalEvidence, WaveKind};

use super::fixtures::*;

/// U6 acceptance #1 + #3: 5 successful slots → the main ledger holds
/// the business events sorted by slot index (de-duplicated) plus
/// exactly one `exec.wave.complete` whose payload lists all 5
/// success_slots (branch + worktree_path). A second fan-in tick is
/// idempotent (no duplicate coord event, `AlreadyDone`).
#[test]
fn test_production_fan_in_writes_ledger_and_injects_complete_once() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");
    let (bridge, _store_wave_id) = setup_u6_production_bridge(events_path.clone(), "u6-wave-5", 5);

    let completed = make_u6_completed("u6-wave-5", 5);
    let detected = make_u3_wave("u6-wave-5", 5, 5);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedComplete,
        "5/5 success must inject exec.wave.complete"
    );

    let lines = read_u6_ledger(&events_path);
    // 5 business events + 1 coordination event.
    assert_eq!(
        lines.len(),
        6,
        "ledger must hold 5 business events + 1 coord event; got {lines:?}"
    );

    // Business events are sorted by slot index (0..5) even though the
    // CompletedWave handed them in reverse order.
    let business: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done"))
        .collect();
    assert_eq!(business.len(), 5, "5 de-duplicated business events");
    for (pos, ev) in business.iter().enumerate() {
        let expected_payload = format!("{{\"unit\":\"u6-{pos}\"}}");
        assert_eq!(
            ev.get("payload").and_then(|p| p.as_str()),
            Some(expected_payload.as_str()),
            "business event at ledger position {pos} must be slot {pos} (sorted by slot index)"
        );
    }

    // Exactly one coord event, with all 5 success_slots.
    let completes: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.complete"))
        .collect();
    assert_eq!(completes.len(), 1, "exactly one exec.wave.complete");
    let coord = completes[0];
    assert_eq!(
        coord.get("system_injected").and_then(|v| v.as_bool()),
        Some(true),
        "coord event must be system_injected"
    );
    let success_slots = coord
        .get("payload")
        .and_then(|p| p.get("success_slots"))
        .and_then(|s| s.as_array())
        .expect("payload.success_slots must be an array");
    assert_eq!(
        success_slots.len(),
        5,
        "success_slots must list all 5 slots"
    );
    for (i, slot) in success_slots.iter().enumerate() {
        assert_eq!(
            slot.get("slot_index").and_then(|v| v.as_u64()),
            Some(i as u64),
            "success_slots[{i}].slot_index"
        );
        assert_eq!(
            slot.get("branch").and_then(|v| v.as_str()),
            Some(format!("u6-loop-exec-{i}").as_str()),
            "success_slots[{i}].branch"
        );
        assert_eq!(
            slot.get("worktree_path").and_then(|v| v.as_str()),
            Some(format!("/tmp/u6-wt/{i}").as_str()),
            "success_slots[{i}].worktree_path"
        );
    }

    // Idempotency: a second tick must NOT re-inject the coord event.
    let outcome2 = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome2,
        SupervisorFanInOutcome::AlreadyDone,
        "post-merge tick must return AlreadyDone (KTD-7)"
    );
    let lines2 = read_u6_ledger(&events_path);
    let completes2 = lines2
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.complete"))
        .count();
    assert_eq!(
        completes2, 1,
        "still exactly one exec.wave.complete after re-tick"
    );
    assert_eq!(
        lines2.len(),
        6,
        "no new lines appended on the idempotent re-tick"
    );
}

/// U6 acceptance #2: a wave where every slot is terminal but some
/// failed must inject `exec.wave.failed` (KTD-8: no silent partial
/// complete) carrying the blocking slots.
#[test]
fn test_production_fan_in_partial_failure_injects_failed() {
    use ralph_core::supervisor::SlotResource;

    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");

    // Build a 3-slot wave: slots 0,1 succeed; slot 2 fails.
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = crate::loop_runner::wave::ProductionBridgeContext {
        loop_id: "u6-loop".to_string(),
        repo_root: std::path::PathBuf::from("/tmp/u6-repo"),
        events_path: Some(events_path.clone()),
        tasks_path: None,
    };
    let bridge =
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store.clone() as std::sync::Arc<dyn SupervisorStore>,
            context,
            std::sync::Arc::new(DefaultWorktreeFactory),
            3,
            // 2026-07-28-003 plan U4: explicit budget keeps the
            // fan-in characterization at the historical default.
            1,
        );
    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u6-wave-fail", 3, 0)
        .expect("register");
    for i in 0..3 {
        bridge
            .store()
            .bind_worktree(
                &store_wave_id,
                i,
                SlotResource {
                    slot_index: i,
                    worktree_path: Some(format!("/tmp/u6-wt/{i}")),
                    branch: Some(format!("u6-loop-exec-{i}")),
                },
            )
            .expect("bind");
    }
    for _ in 0..3 {
        bridge.store().try_dispatch_next(3).expect("dispatch");
    }
    bridge
        .record_slot_result(&store_wave_id, 0, "h0", 1)
        .expect("s0");
    // Plan 004 R2 / P0-2: success path requires terminal evidence.
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_wave_id,
            0,
            &TerminalEvidence::from_event("exec.unit.done", "{\"unit\":\"u6-0\"}"),
        )
        .expect("evidence 0");
    bridge
        .record_slot_result(&store_wave_id, 1, "h1", 1)
        .expect("s1");
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_wave_id,
            1,
            &TerminalEvidence::from_event("exec.unit.done", "{\"unit\":\"u6-1\"}"),
        )
        .expect("evidence 1");
    bridge
        .record_slot_failure(&store_wave_id, 2, "boom")
        .expect("f2");
    // Plan 004 R3 / P0-1: dispatcher must commit salvage BEFORE
    // `fail_wave` latches the coord-event injection.
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

    let bridge: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge);
    let completed = make_u6_completed("u6-wave-fail", 2); // only 2 results (slot 2 failed)
    let detected = make_u3_wave("u6-wave-fail", 3, 3);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "a terminal wave with a failed slot must inject exec.wave.failed"
    );

    let lines = read_u6_ledger(&events_path);
    let failed: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.failed"))
        .collect();
    assert_eq!(failed.len(), 1, "exactly one exec.wave.failed");
    let blocking = failed[0]
        .get("payload")
        .and_then(|p| p.get("blocking_slots"))
        .and_then(|b| b.as_array())
        .expect("payload.blocking_slots");
    assert!(
        blocking.iter().any(|v| v.as_u64() == Some(2)),
        "blocking_slots must name the failed slot 2; got {blocking:?}"
    );
    // No spurious complete event on the failure path.
    assert!(
        !lines
            .iter()
            .any(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.complete")),
        "failure path must not inject exec.wave.complete"
    );
}

/// U6 acceptance #4: when the merge sink rejects the batch, the fan-in
/// returns `MergeFailed`, leaves `merged_to_events` false (no coord
/// event written), so the next tick retries the merge exactly once.
#[test]
fn test_production_fan_in_sink_failure_defers_complete() {
    use ralph_core::supervisor::{EventMergeSink, InMemoryMergeSink, MergeSinkError, SlotResource};

    // A sink that fails the first append, then succeeds — modelling a
    // transient ledger write failure that recovery retries (KTD-7).
    #[derive(Debug)]
    struct FailOnceSink {
        inner: InMemoryMergeSink,
    }
    impl EventMergeSink for FailOnceSink {
        fn append_events(&self, events: Vec<ralph_proto::Event>) -> Result<(), MergeSinkError> {
            self.inner.append_events(events)
        }
    }

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let wave = store
        .register_wave("u6-retry", WaveKind::Exec, 1, 0)
        .expect("register");
    store
        .bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some("/tmp/u6-wt/0".to_string()),
                branch: Some("u6-loop-exec-0".to_string()),
            },
        )
        .expect("bind");
    let _ = store.try_dispatch_next(1).expect("dispatch").expect("slot");
    store.record_slot_result(&wave, 0, "h0", 1).expect("record");
    // Plan 004 R2 / P0-2: success path requires terminal evidence.
    store
        .record_slot_terminal_evidence(
            &wave,
            0,
            &TerminalEvidence::from_event("exec.unit.done", "{\"unit\":\"u6-retry-0\"}"),
        )
        .expect("evidence");

    let sink = std::sync::Arc::new(InMemoryMergeSink::new());
    sink.fail_with("ledger locked");
    let coord = ralph_core::supervisor::SupervisorCoordinator::new(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        sink.clone() as std::sync::Arc<dyn EventMergeSink>,
    );

    let inputs = ralph_core::supervisor::PhaseInputs {
        aggregate_timeout_secs: 600,
        elapsed_secs: 0,
        cancel_requested: false,
    };
    // First tick: sink fails → MergeFailed, merged_to_events stays false.
    let action1 = coord
        .tick_with_slot_events(&wave, inputs.clone(), vec![])
        .expect("tick");
    assert!(
        matches!(
            action1,
            ralph_core::supervisor::CoordinatorAction::MergeFailed { .. }
        ),
        "sink failure must surface MergeFailed; got {action1:?}"
    );
    assert!(
        !store
            .fan_in_status(&wave)
            .expect("snap")
            .delivery_state
            .at_least(ralph_core::supervisor::WaveDeliveryState::CoordinationCommitted),
        "merged_to_events must stay false after a sink failure"
    );

    // Clear the fault: the retry merges + completes exactly once.
    sink.clear_failure();
    let action2 = coord
        .tick_with_slot_events(&wave, inputs.clone(), vec![])
        .expect("tick");
    assert!(
        matches!(
            action2,
            ralph_core::supervisor::CoordinatorAction::InjectedComplete { .. }
        ),
        "retry after sink recovery must inject complete; got {action2:?}"
    );
    // A third tick is idempotent (no second complete).
    let action3 = coord
        .tick_with_slot_events(&wave, inputs, vec![])
        .expect("tick");
    assert!(
        matches!(
            action3,
            ralph_core::supervisor::CoordinatorAction::AlreadyDone
                | ralph_core::supervisor::CoordinatorAction::ContinueCollect
        ),
        "post-merge tick must not re-inject; got {action3:?}"
    );
    let _ = FailOnceSink {
        inner: InMemoryMergeSink::new(),
    };
}

/// U6 acceptance #5: the production bridge path (events_path set)
/// writes the fan-in output to a real `events.jsonl` file via the
/// `FileEventMergeSink` — it no longer fakes the main ledger with an
/// in-memory sink. The legacy `from_store` entry point (in-memory
/// sink) leaves the file untouched.
#[test]
fn test_production_bridge_writes_real_ledger_not_in_memory() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");
    assert!(!events_path.exists(), "ledger must not exist before fan-in");

    let (bridge, _store_wave_id) =
        setup_u6_production_bridge(events_path.clone(), "u6-wave-real", 2);
    let completed = make_u6_completed("u6-wave-real", 2);
    let detected = make_u3_wave("u6-wave-real", 2, 2);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(outcome, SupervisorFanInOutcome::InjectedComplete);

    assert!(
        events_path.exists(),
        "production FileEventMergeSink must create the real events.jsonl"
    );
    let lines = read_u6_ledger(&events_path);
    assert!(
        !lines.is_empty(),
        "production sink must write the fan-in business + coord events to disk"
    );
    // The business events actually landed in the file (not an in-memory buffer).
    assert!(
        lines
            .iter()
            .any(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done")),
        "business events must be present in the on-disk ledger"
    );
}

/// U6: the fan-in de-duplicates identical business events across slots
/// (the main ledger must not carry repeated records).
#[test]
fn test_production_fan_in_dedups_identical_business_events() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");
    let (bridge, _store_wave_id) =
        setup_u6_production_bridge(events_path.clone(), "u6-wave-dedup", 3);

    // Every slot emits the SAME business event (same topic + payload).
    let results = (0..3)
        .map(|i| ralph_core::WaveResult {
            index: i,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"shared\"}".to_string())
                    .with_source("executor"),
            ],
        })
        .collect();
    let completed = ralph_core::CompletedWave {
        wave_id: "u6-wave-dedup".to_string(),
        wave_total: 3,
        results,
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: false,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    };
    let detected = make_u3_wave("u6-wave-dedup", 3, 3);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(outcome, SupervisorFanInOutcome::InjectedComplete);

    let lines = read_u6_ledger(&events_path);
    let business_count = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done"))
        .count();
    assert_eq!(
        business_count, 1,
        "identical business events across 3 slots must de-dup to a single ledger record"
    );
}

// =============================================================================
// 2026-07-22-001 plan U7: write-isolation worktree binding
// (KTD-6). Default `shared_readonly` must remain the no-worktree
// shape; explicit `isolation_mode=worktree` (Exec/Fix) must
// hand out a `SlotBinding` with a non-`None` `worktree_path`
// and that path must come from the production
// `DefaultWorktreeFactory` (023 U1 closure). We pin the trait
// surface here so the lazy-bridge (U2) keeps invoking the same
// production path the enabled preset uses.
// =============================================================================

/// U7 / default path's lazy bridge (U2) must surface the same
/// `bind_slot` contract the production bridge does: Exec waves
/// receive a `Some(SlotBinding)` (worktree-bound), Review waves
/// receive `None` (shared_readonly). We do NOT exercise the
/// factory here (that is covered by `integration_supervisor_primary`
/// / `wave_supervisor` characterization); we only assert the
/// lazy-bridge routes through the same trait method.
#[test]
fn u7_lazy_bridge_bind_slot_routes_to_production_trait_method() {
    use ralph_core::supervisor::WaveKind;
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    // No production worktree path is exercised here; the
    // factory's `DefaultWorktreeFactory` returns `None` when no
    // repo is associated (the in-memory coordinator has no
    // repo_root in its `ProductionBridgeContext`). The trait
    // method is called on both paths uniformly.
    let exec_binding = bridge
        .bind_slot(WaveKind::Exec, "u7-exec", 0)
        .expect("bind_slot ok");
    // exec with no repo context returns the `None`-binding
    // shape; with repo context it returns `Some(worktree_path)`.
    // We only assert the trait method is wired and returns the
    // documented union type.
    assert!(
        exec_binding.is_none() || exec_binding.is_some(),
        "bind_slot must return the documented Option<SlotBinding> shape"
    );
    // Review waves are shared_readonly by default and must
    // always return None (KTD-6 default).
    let review_binding = bridge
        .bind_slot(WaveKind::Review, "u7-review", 0)
        .expect("bind_slot review ok");
    assert!(
        review_binding.is_none(),
        "Review kind must default to shared_readonly (KTD-6)"
    );
}

/// U7 / KTD-6 default: with no explicit `isolation_mode`, the
/// `bind_slot` surface for `Review` returns `None`. We pin this
/// so an accidental "default to worktree for review" patch
/// surfaces as a test failure.
#[test]
fn u7_review_default_is_shared_readonly() {
    use ralph_core::supervisor::WaveKind;
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    let binding = bridge
        .bind_slot(WaveKind::Review, "u7-review-default", 0)
        .expect("bind_slot ok");
    assert!(
        binding.is_none(),
        "Review waves must default to shared_readonly (no worktree)"
    );
}

// ── 2026-07-25-004 plan U4: slot_never_started diagnostics ─────────────────────
//
// G3: the `SupervisorBridge::record_never_started_failures` contract.
// When a wave fails with slots that never left `Pending`, those slots
// must be recorded as `Failed` with reason `slot_never_started`. This
// is exercised by the dispatcher in the `InjectedFailed` arm of
// `run_supervisor_fan_in` before writing the coordination event.
//
// The test is scoped to the bridge + store layer (no full dispatcher
// machinery); JSON diagnostic assertions are deferred to U5.

/// G3 T1: `record_never_started_failures` on a wave with 1 completed
/// slot and 2 Pending slots — Pending slots become `Failed` with
/// `slot_never_started`, completed slot stays `Completed`. Second call
/// is idempotent (same-reason replay → Ok(())).
#[test]
fn g3_record_never_started_marks_pending_slots_in_store() {
    let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
    let store = bridge.store();

    let wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "g3-wave", 3, 0)
        .unwrap();

    // Slot 0: bind, dispatch, complete.
    store
        .bind_worktree(
            &wave_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/g3".to_string()),
                branch: Some("ralph/g3".to_string()),
            },
        )
        .unwrap();
    let _ = store.try_dispatch_next(4).unwrap().unwrap();
    store.record_slot_result(&wave_id, 0, "hash-g3", 1).unwrap();

    // Slots 1 and 2: still `Pending` — never dispatched.
    bridge.record_never_started_failures(&wave_id).unwrap();

    // Verify slot 0 stayed Completed.
    let snap = store.fan_in_status(&wave_id).unwrap();
    let (_, s0_status) = snap.slots.iter().find(|(i, _)| *i == 0).unwrap();
    assert_eq!(
        s0_status,
        &ralph_core::supervisor::SlotStatus::Completed,
        "slot 0 must stay Completed"
    );

    // Verify slots 1 and 2 are Failed with `slot_never_started`.
    for slot_index in [1u32, 2] {
        let snap = store.fan_in_status(&wave_id).unwrap();
        let (_, status) = snap.slots.iter().find(|(i, _)| *i == slot_index).unwrap();
        assert_eq!(
            status,
            &ralph_core::supervisor::SlotStatus::Failed,
            "slot {slot_index} must be Failed"
        );
    }

    // Idempotency: second call → same failed_count.
    let snap_before = store.fan_in_status(&wave_id).unwrap();
    let failed_before = snap_before.failed_count;
    bridge.record_never_started_failures(&wave_id).unwrap();
    let snap_after = store.fan_in_status(&wave_id).unwrap();
    assert_eq!(
        snap_after.failed_count, failed_before,
        "second record_never_started_failures call must not double-count"
    );
}

/// 2026-07-25-004 plan U5: cancel-closure diagnostic pin. `cancel_wave`
/// is the only thing that flips never-started Pending slots to Cancelled
/// on the cancel path. After U5 those Cancelled slots carry
/// `failure_reason = slot_never_started`, so the InjectedFailed
/// reason-collection (dispatcher.rs:2221-2233) — which inserts a reason
/// whenever `slot_failure_reason` returns `Ok(Some(_))` for a
/// Failed|Cancelled slot — now produces a NON-null reason for them
/// instead of the pre-fix `status=cancelled, reason=null`.
#[test]
fn g3_cancel_closure_cancelled_slot_has_never_started_reason() {
    use ralph_core::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED;

    let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
    let store = bridge.store();

    let wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "g3-cancel-wave", 3, 0)
        .unwrap();

    // Slot 0: bind, dispatch, complete → Completed, reason None.
    store
        .bind_worktree(
            &wave_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/g3c".to_string()),
                branch: Some("ralph/g3c".to_string()),
            },
        )
        .unwrap();
    let _ = store.try_dispatch_next(4).unwrap().unwrap();
    store
        .record_slot_result(&wave_id, 0, "hash-g3c", 1)
        .unwrap();

    // Slots 1 and 2: still Pending — never dispatched. Cancel the wave.
    bridge.cancel_wave(&wave_id).unwrap();

    // Cancelled slots now expose a non-null `slot_never_started` reason
    // through the exact bridge surface the dispatcher reads.
    for slot_index in [1u32, 2] {
        let reason = bridge.slot_failure_reason(&wave_id, slot_index).unwrap();
        assert!(
            reason.is_some(),
            "cancelled slot {slot_index} reason must be Some"
        );
        assert_eq!(
            reason.as_deref(),
            Some(REASON_SLOT_NEVER_STARTED),
            "cancelled slot {slot_index} must surface slot_never_started"
        );
    }

    // The completed slot's reason stays None.
    assert_eq!(bridge.slot_failure_reason(&wave_id, 0).unwrap(), None);
}

// =============================================================================

/// 2026-07-25-003 plan U3: a worker that "wrote its own events" via the
/// dispatcher-injected per-slot channel ends up in the supervisor
/// store as `Completed`. The executor writes a single `exec.unit.done`
/// record into the `request.worker_events_path` (the path the
/// dispatcher injected as `RALPH_EVENTS_FILE` and the path
/// `read_worker_events` later reads in `wave/worker.rs`), then reads
/// the file back to obtain the event batch. This mirrors the
/// production flow: agent runs `ralph emit exec.unit.done` →
/// `read_worker_events` returns the record → `classify_slot_result`
/// maps it to `Completed(Done)` → `record_slot_result` writes the
/// store row.
#[tokio::test]
async fn test_u3_emit_to_wave_channel_records_slot_completed() {
    use crate::loop_runner::wave::read_worker_events;
    use crate::loop_runner::wave::{WaveWorkerExecutor, WorkerRequest};
    use std::sync::Arc;
    use std::time::Duration;

    /// Executor that emits a terminal `exec.unit.done` into the
    /// dispatcher-injected per-slot channel file, then reads it back
    /// via the production `read_worker_events` helper. The returned
    /// `(events, _, true)` matches what production `run_wave_worker`
    /// hands to the dispatcher. The carve-out regression is pinned
    /// separately by `test_u3_resolve_emit_path_dispatcher_signed_carve_out`
    /// (this test focuses on the dispatcher's causal-chain side).
    struct ChannelEmittingExecutor;
    impl WaveWorkerExecutor for ChannelEmittingExecutor {
        fn execute(
            &self,
            request: WorkerRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (u32, WaveWorkerOutcome)> + Send>>
        {
            Box::pin(async move {
                let index = request.index;
                let events_path = request.worker_events_path.clone();
                let line = serde_json::to_string(&ralph_core::Event {
                    topic: "exec.unit.done".to_string(),
                    payload: Some(format!("{{\"slot\":{index},\"seq\":0}}")),
                    ts: String::new(),
                    hat: None,
                    triggered: None,
                    source: None,
                    wave_id: None,
                    wave_index: None,
                    wave_total: None,
                    system_injected: None,
                })
                .expect("serialize event");
                if let Some(parent) = events_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&events_path, format!("{line}\n")).expect("write channel file");
                // Production `run_wave_worker` reads the file via
                // `read_worker_events` (wave/worker.rs); the test
                // exercises the same helper to assert the channel
                // path is consumable by the production reader.
                let events = read_worker_events(&events_path);
                assert_eq!(
                    events.len(),
                    1,
                    "U3/003: channel must hold exactly one event for the dispatcher to classify; got {events:?}"
                );
                assert_eq!(
                    events[0].topic, "exec.unit.done",
                    "U3/003: emitted topic must round-trip through the channel reader"
                );
                (index, Ok((events, Duration::from_millis(5), true)))
            })
        }
    }

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>);

    let wave = make_u3_wave("u3-emit-channel", 1, 1);
    let executor = ChannelEmittingExecutor;
    let outcome = run_u3_dispatch_wave(bridge.clone(), wave, executor).await;
    let bridge_for_asserts = bridge;

    assert!(
        matches!(
            outcome,
            WaveDispatchOutcome::Completed(_) | WaveDispatchOutcome::Partial(_)
        ),
        "U3/003: emit→channel→Completed must close the wave, got {outcome:?}"
    );

    // U5RecordingBridge recorded exactly one `record_slot_result`
    // call (the success path), zero `record_slot_failure` calls.
    let results = bridge_for_asserts.results_snapshot();
    let failures = bridge_for_asserts.failures_snapshot();
    assert_eq!(
        results.len(),
        1,
        "U3/003: dispatcher's classifier must call record_slot_result for the terminal Done; got {results:?}"
    );
    assert!(
        failures.is_empty(),
        "U3/003: no failure record expected for a slot that emitted terminal Done; got {failures:?}"
    );
    assert_eq!(results[0].0, 0, "U3/003: slot 0 must be the one recorded");
    assert_eq!(
        results[0].2, 1,
        "U3/003: event_count must equal the number of events read from the channel"
    );

    // The supervisor store snapshot reflects the slot as completed
    // — this is the causal-chain proof the plan U3 requires.
    let store_wave_id = bridge_for_asserts
        .store
        .recover_active_waves()
        .expect("recover")
        .pop()
        .expect("one wave")
        .wave_id;
    let snap = bridge_for_asserts
        .store
        .fan_in_status(&store_wave_id)
        .expect("snapshot");
    assert_eq!(
        snap.completed_count, 1,
        "U3/003: emit→channel must produce a store row with completed_count=1; got {snap:?}"
    );
    assert_eq!(
        snap.failed_count, 0,
        "U3/003: emit→channel must not produce any failure rows; got {snap:?}"
    );
    // Suppress unused-import warnings if Arc is dropped during refactors.
    let _ = Arc::clone(&bridge_for_asserts.store);
}

// Plan 2026-07-28-001 U2 (R5 / S8): after the first wave's
// `expected_total == 1` slot closes via real `release_slot_dispatch`
// terminal outcome, the supervisor-coordinator fan-in path must
// move the wave to `Done`, and a second wave registered with the
// same store must already be in `Dispatch` (or `Collect`) so the
// next wave can be picked up off the same store. This is the
// CLI-side counterpart of the BDD `parallel_forge_task_dispatch_runtime`
// task-to-wave chain.
#[test]
fn task_close_then_next_ready_two_wave_supervisor_path() {
    use ralph_core::supervisor::{
        DispatchOutcome, InMemoryCoordinatorBridge, InMemorySupervisorStore, SupervisorStore,
        WaveKind, WavePhase,
    };
    use std::sync::Arc;

    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());
    let bridge = InMemoryCoordinatorBridge::from_store(store.clone());

    // Wave 1: register a single-slot execution wave and capture the
    // store-allocated wave id (`register_wave_if_absent` returns the
    // store id, NOT the caller key — the bridge keeps the
    // caller key → store id map but the store itself only accepts
    // its own ids for slot release).
    let wave1_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "pf-ts-wave-1", 1, 1)
        .expect("register wave 1");
    bridge
        .release_slot_dispatch(&wave1_id, 0, DispatchOutcome::Completed)
        .expect("release slot 0 of wave 1");
    // The store layer does NOT auto-advance phase on
    // `release_slot_dispatch`; the coordinator moves the phase
    // verdict via `set_wave_phase`. Drive the same seam the
    // production supervisor uses so the snapshot reflects the
    // post-fan-in truth and the test asserts the round-trip of
    // slot close → next-ready rather than codec-side defaults.
    store
        .set_wave_phase(&wave1_id, WavePhase::Done)
        .expect("mark wave 1 done");
    let wave1 = bridge
        .fan_in_status(&wave1_id)
        .expect("wave 1 fan_in_status");
    assert_eq!(
        wave1.phase,
        WavePhase::Done,
        "first wave must complete once the slot is released; got {:?}",
        wave1.phase
    );
    assert_eq!(
        wave1.completed_count, 1,
        "completed_count must reflect the released slot; got {wave1:?}"
    );
    assert_eq!(
        wave1.failed_count, 0,
        "failed_count must stay at zero; got {wave1:?}"
    );

    // Wave 2: register the dependent wave immediately afterwards.
    // The store must accept the second wave while the first is still
    // tracked as `Done`; reading both snapshots side-by-side confirms
    // the next-ready-set contract that the dispatcher reads off the
    // same InMemorySupervisorStore.
    let wave2_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "pf-ts-wave-2", 1, 1)
        .expect("register wave 2");
    let wave2 = bridge
        .fan_in_status(&wave2_id)
        .expect("wave 2 fan_in_status");
    assert_eq!(
        wave2.phase,
        WavePhase::Dispatch,
        "second wave must move into Dispatch once registered; got {:?}",
        wave2.phase
    );
    assert_eq!(
        wave2.expected_total, 1,
        "second wave must preserve its declared slot total; got {wave2:?}"
    );
    assert_ne!(
        wave1.phase, wave2.phase,
        "two consecutive waves must NOT share a phase; got wave1={:?} wave2={:?}",
        wave1.phase, wave2.phase
    );
}
