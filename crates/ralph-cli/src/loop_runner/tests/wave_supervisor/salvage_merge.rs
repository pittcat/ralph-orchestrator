use crate::loop_runner::wave::SupervisorBridge;
use ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory;
use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};

use super::fixtures::*;

// =============================================================================
// 2026-07-25-003 plan U6: `exec.wave.failed` / `fix.wave.failed`
// payload must expose per-slot `failure_reason` so an operator can
// tell a `worker_timeout` apart from an `empty_worker_result` without
// re-running diagnostics. The legacy payload shape carries
// `wave_id` + `reason` + `blocking_slots` only; a 5-slot failure
// looks identical whether slot 0 timed out or hit
// `empty_worker_result`. The fix adds an OPTIONAL `slot_failures`
// field (schema `required_fields` is unchanged so the engine gate
// still passes) listing `{slot_index, reason, duration_ms}` for every
// failed slot. The new test pins the field shape so a future schema
// refactor cannot silently drop the diagnostic data.
// =============================================================================

/// U6: `build_wave_failed_payload` for an Exec wave with two
/// failures (one `worker_timeout`, one `empty_worker_result`) must
/// surface each per-slot reason in `slot_failures`, AND the
/// `blocking_slots` set must equal the indices that actually
/// failed (no false-positive blocking of completed slots).
#[test]
fn test_u6_failed_payload_exposes_per_slot_reasons() {
    use crate::loop_runner::wave::build_wave_failed_payload;
    use ralph_core::supervisor::WaveKind;
    use ralph_core::{CompletedWave, WaveFailure, WaveResult};
    use std::time::Duration;

    let mut completed = CompletedWave {
        wave_id: "u6-exec".to_string(),
        wave_total: 5,
        results: vec![WaveResult {
            index: 1,
            events: vec![],
        }],
        failures: vec![
            WaveFailure {
                index: 0,
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
            WaveFailure {
                index: 4,
                error: "worker_cancelled".to_string(),
                duration: Duration::from_secs(2),
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
    // 5-slot failure set: {0,2,4} (1+3 are completed/otherwise)
    let payload = build_wave_failed_payload(
        WaveKind::Exec,
        &completed,
        "required_slot_failure",
        vec![0, 2, 4],
        &std::collections::HashMap::new(),
        None,
        None,
    );
    let obj = payload.as_object().expect("payload must be a JSON object");

    // Required fields still present (schema contract intact).
    assert_eq!(
        obj.get("wave_id").and_then(|v| v.as_str()),
        Some("u6-exec"),
        "U6/003: required field `wave_id` must be present"
    );
    assert_eq!(
        obj.get("reason").and_then(|v| v.as_str()),
        Some("required_slot_failure"),
        "U6/003: required field `reason` must be present"
    );
    let blocking: Vec<u32> = obj
        .get("blocking_slots")
        .and_then(|v| v.as_array())
        .expect("U6/003: `blocking_slots` must be a JSON array")
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as u32))
        .collect();
    assert_eq!(
        blocking,
        vec![0, 2, 4],
        "U6/003: `blocking_slots` must equal the actual failed slot indices; got {blocking:?}"
    );

    // New diagnostic field: per-slot reasons in stable slot order.
    let slot_failures = obj.get("slot_failures").and_then(|v| v.as_array()).expect(
        "U6/003: `slot_failures` must be a JSON array of {slot_index, reason, duration_ms}",
    );
    assert_eq!(
        slot_failures.len(),
        3,
        "U6/003: one entry per failed slot; got {slot_failures:?}"
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
        by_index.get(&0).map(String::as_str),
        Some("worker_timeout"),
        "U6/003: slot 0 must carry `worker_timeout`; got {by_index:?}"
    );
    assert_eq!(
        by_index.get(&2).map(String::as_str),
        Some("empty_worker_result"),
        "U6/003: slot 2 must carry `empty_worker_result`; got {by_index:?}"
    );
    assert_eq!(
        by_index.get(&4).map(String::as_str),
        Some("worker_cancelled"),
        "U6/003: slot 4 must carry `worker_cancelled`; got {by_index:?}"
    );

    // Negative guard: a completed slot MUST NOT appear in
    // `blocking_slots` (R5) or in `slot_failures`.
    assert!(
        !blocking.contains(&1) && !blocking.contains(&3),
        "U6/003: completed slots must NOT be in blocking_slots; got {blocking:?}"
    );
    assert!(
        !by_index.contains_key(&1) && !by_index.contains_key(&3),
        "U6/003: completed slots must NOT appear in slot_failures; got {by_index:?}"
    );
    // Suppress unused mut warnings if completed gains new fields.
    let _ = &mut completed;
}

// ===== U1 characterization =====
//
// These tests PROVE the current `run_supervisor_fan_in` /
// `build_wave_failed_payload` partial-failure path does NOT salvage
// Completed slot business events back to the main ledger for
// exec/fix waves (the gap U2-U7 will fix). Test 2 additionally
// characterizes that the review path already has its own salvage
// helper and is NOT affected by the exec/fix changes.
//
// Refs:
//   - plan `2026-07-25-005-fix-supervisor-slot-activity-salvage-redrive-plan` U1
//   - `run_supervisor_fan_in` in `dispatcher.rs` — the partial failure path
//     routes to `fail_wave` which does NOT call `merge_sink.append_events`,
//     so completed slot events are silently dropped on the floor.
//   - Review salvage helper: `review_salvage_collect_done_results` in
//     `coordinator.rs` (review path has its own merge helper; exec/fix
//     does not — this is the distinction U2-U7 exploits).

/// U1 Test 1 (2026-07-25-005 plan U1): exec/fix partial failure
/// salvages Completed slot business events to the main ledger.
///
/// Setup: a 2-slot exec wave.
///   - slot 0: emits `exec.unit.done`, reaches Completed
///   - slot 1: worker_timeout, reaches Failed
///
/// After fan-in:
///   - The main ledger MUST contain slot 0's `exec.unit.done` event
///     exactly once (salvage keeps the completed slot's business event)
///   - `blocking_slots` must be `[1]` (NOT `[0, 1]` — completed slots
///     are never blocking per the phase decision contract)
///   - `exec.wave.failed` is injected
///
/// A1 adversarial assertions:
///   - Salvaged event payload unit must equal "u1-0" (origin pin)
///   - Failed slot must have no terminal evidence (failed-slot integrity)
///
/// This test was originally a GREEN gap-lock asserting the completed
/// slot's event was dropped; salvage (U1) has since landed, so it is
/// updated to assert the salvaged event IS present.
#[test]
fn exec_fix_partial_failure_does_not_salvage_completed_slot_events() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::{TerminalEvidence, WaveKind};

    // M1: use the shared helper instead of inline setup
    let (_tmp, bridge, _store, store_wave_id, events_path) =
        setup_u3_partial_failure_bridge(WaveKind::Exec, "u1-exec-partial", 2);

    // Slot 0: completes with evidence.
    bridge
        .record_slot_result(&store_wave_id, 0, "h0", 1)
        .expect("s0 result");
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_wave_id,
            0,
            &TerminalEvidence::from_event("exec.unit.done", "{\"unit\":\"u1-0\"}"),
        )
        .expect("evidence 0");

    // Slot 1: fails with worker_timeout (retryable).
    bridge
        .record_slot_failure(
            &store_wave_id,
            1,
            ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
        )
        .expect("s1 failure");

    // A1 adversarial assertions: pre-conditions for the salvage/redrive
    // characterization. These pin the origin of the completed slot's
    // terminal evidence and verify the failed slot has no terminal evidence.
    let slot0_evidence = bridge
        .store()
        .slot_terminal_evidence(&store_wave_id, 0)
        .expect("slot 0 evidence query must succeed");
    assert!(
        slot0_evidence.is_some(),
        "A1: slot 0 (completed) must have terminal evidence before fan-in"
    );
    let slot0_ev = slot0_evidence.expect("evidence is Some");
    assert_eq!(
        slot0_ev.topic, "exec.unit.done",
        "A1: completed slot's terminal evidence topic must be exec.unit.done; got {}",
        slot0_ev.topic
    );
    // payload_fingerprint is SHA-256 of the original payload ("{\"unit\":\"u1-0\}").
    // We trust the fingerprint is correct if topic+dimension match; the fingerprint
    // is verified by the store's own idempotency tests.
    assert!(
        !slot0_ev.payload_fingerprint.is_empty(),
        "A1: completed slot's terminal evidence must have a non-empty fingerprint"
    );

    let slot1_evidence = bridge
        .store()
        .slot_terminal_evidence(&store_wave_id, 1)
        .expect("slot 1 evidence query must succeed");
    assert!(
        slot1_evidence.is_none(),
        "A1: slot 1 (failed) must have no terminal evidence; got {slot1_evidence:?}"
    );

    // Pre-commit salvage (P0-1 contract).
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

    // Build a CompletedWave with only slot 0's result (slot 1 failed → no event).
    let completed = ralph_core::CompletedWave {
        wave_id: "u1-exec-partial".to_string(),
        wave_total: 2,
        results: vec![ralph_core::WaveResult {
            index: 0,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-0\"}\n")
                    .with_source("executor")
                    .with_wave("u1-exec-partial".to_string(), 0, 2),
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
    let detected = make_u3_wave("u1-exec-partial", 2, 2);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "partial failure must inject exec.wave.failed"
    );

    // Read the main ledger.
    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();

    // Salvage (2026-07-25-005 plan U1) writes the completed slot's
    // business event to the main ledger even on partial failure, so
    // slot 0's `exec.unit.done` must appear exactly once, alongside
    // the single `exec.wave.failed` coord event.
    let salvaged: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done"))
        .collect();
    assert_eq!(
        salvaged.len(),
        1,
        "salvage must write completed slot 0's exec.unit.done to the main ledger \
         exactly once during partial failure; got {} events",
        salvaged.len()
    );
    let salvaged_payload = salvaged[0]
        .get("payload")
        .and_then(|p| p.as_str())
        .expect("salvaged event must have a string payload");
    assert!(
        salvaged_payload.contains("u1-0"),
        "A1 origin pin: salvaged event payload must contain unit=u1-0; got {salvaged_payload}"
    );

    let failed_events: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.failed"))
        .collect();
    assert_eq!(
        failed_events.len(),
        1,
        "exactly one exec.wave.failed coord event expected; got {}",
        failed_events.len()
    );

    // blocking_slots must be [1], NOT [0, 1] — completed slots are never blocking.
    let blocking = failed_events[0]
        .get("payload")
        .and_then(|p| p.get("blocking_slots"))
        .and_then(|b| b.as_array())
        .expect("payload.blocking_slots");
    let blocking_indices: Vec<u32> = blocking
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as u32))
        .collect();
    assert_eq!(
        blocking_indices,
        vec![1],
        "blocking_slots must be [1] (the failed slot only); completed slot 0 must NOT be listed. \
         Got {blocking_indices:?}"
    );

    // No spurious exec.wave.complete on the failure path.
    let complete_count = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.complete"))
        .count();
    assert_eq!(
        complete_count, 0,
        "partial failure must NOT inject exec.wave.complete; got {complete_count}"
    );
}

/// Regression: an exec wave where EVERY slot failed (zero Completed
/// slots) must still reach `InjectedFailed`.
///
/// This is the shape a silent worker produces: the dispatcher records
/// a synthetic slot failure, the wave settles with `results=0
/// failures=1`, and the salvage seam has nothing to append. The seam
/// must nevertheless commit both delivery phases, otherwise
/// `fail_wave` answers `SalvageNotMerged` on every tick and the
/// fan-in degrades to `StoreError` / `fan_in_failed` — no
/// `exec.wave.failed` is ever injected and the loop dies without
/// telling the preset anything.
///
/// Unlike the partial-failure test above, this one deliberately does
/// NOT pre-commit the salvage projection: pre-committing is exactly
/// what hid the empty-batch gap.
#[test]
fn exec_wave_with_zero_completed_slots_injects_failed_without_precommitted_salvage() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::WaveKind;

    let (_tmp, bridge, _store, store_wave_id, events_path) =
        setup_u3_partial_failure_bridge(WaveKind::Exec, "empty-salvage-exec", 1);

    // The only slot never reported; the dispatcher synthesises a
    // worker_timeout failure for it.
    bridge
        .record_slot_failure(
            &store_wave_id,
            0,
            ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
        )
        .expect("s0 failure");

    let bridge: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge);

    let completed = ralph_core::CompletedWave {
        wave_id: "empty-salvage-exec".to_string(),
        wave_total: 1,
        results: vec![],
        failures: vec![ralph_core::WaveFailure {
            index: 0,
            error: "worker did not report".to_string(),
            duration: std::time::Duration::from_millis(1),
            expected_dimension: None,
            actual_dimension: None,
        }],
        duration: std::time::Duration::from_millis(1),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    let detected = make_u3_wave("empty-salvage-exec", 1, 1);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "a zero-completed exec wave must inject exec.wave.failed, not degrade to StoreError"
    );

    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();
    let failed: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.failed"))
        .collect();
    assert_eq!(
        failed.len(),
        1,
        "exactly one exec.wave.failed coord event expected; got {}",
        failed.len()
    );
    let blocking: Vec<u32> = failed[0]
        .get("payload")
        .and_then(|p| p.get("blocking_slots"))
        .and_then(|b| b.as_array())
        .expect("payload.blocking_slots")
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as u32))
        .collect();
    assert_eq!(blocking, vec![0], "the single failed slot must be blocking");

    // Nothing to salvage means nothing business-shaped on main.
    let business = lines
        .iter()
        .filter(|v| {
            matches!(
                v.get("topic").and_then(|t| t.as_str()),
                Some("exec.unit.done") | Some("exec.wave.complete")
            )
        })
        .count();
    assert_eq!(
        business, 0,
        "an all-failed wave must not fabricate completion events; got {business}"
    );
}

/// Regression: the review seam shares the same empty-batch tail, so a
/// review wave with zero Completed slots must also reach
/// `InjectedFailed` rather than stalling on `SalvageNotMerged`.
#[test]
fn review_wave_with_zero_completed_slots_injects_failed_without_precommitted_salvage() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::WaveKind;

    // Review slots are SharedReadonly, so the shared exec fixture (which
    // binds a worktree per slot) cannot be reused here.
    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = crate::loop_runner::wave::ProductionBridgeContext {
        loop_id: "empty-salvage-review".to_string(),
        repo_root: std::path::PathBuf::from("/tmp/empty-salvage-repo"),
        events_path: Some(events_path.clone()),
        tasks_path: None,
    };
    let bridge =
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store.clone() as std::sync::Arc<dyn SupervisorStore>,
            context,
            std::sync::Arc::new(DefaultWorktreeFactory),
            1,
            1,
        );
    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Review, "empty-salvage-review", 1, 0)
        .expect("register");
    bridge.store().try_dispatch_next(1).expect("dispatch");

    bridge
        .record_slot_failure(
            &store_wave_id,
            0,
            ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
        )
        .expect("s0 failure");

    let bridge: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge);

    let completed = ralph_core::CompletedWave {
        wave_id: "empty-salvage-review".to_string(),
        wave_total: 1,
        results: vec![],
        failures: vec![ralph_core::WaveFailure {
            index: 0,
            error: "worker did not report".to_string(),
            duration: std::time::Duration::from_millis(1),
            expected_dimension: None,
            actual_dimension: None,
        }],
        duration: std::time::Duration::from_millis(1),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    let detected = make_u3_wave("empty-salvage-review", 1, 1);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "a zero-completed review wave must inject review.wave.failed, not degrade to StoreError"
    );
}

/// U1 Test 2 (GREEN — characterization of review path stability):
/// the review wave kind has its own salvage helper
/// (`review_salvage_collect_done_results` in `coordinator.rs`)
/// that is NOT being changed by the exec/fix salvage work (U2-U7).
/// A review wave with the same partial-failure shape (1 completed
/// + 1 failed) reaches `InjectedFailed` but the review salvage
/// path is structurally distinct from the exec/fix path and must
/// remain unaffected.
///
/// This test PASSES on current HEAD — it documents the existing
/// review salvage behavior and ensures the exec/fix changes don't
/// accidentally break the review path.
#[test]
fn review_partial_failure_salvage_path_unaffected() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::{TerminalEvidence, WaveKind};

    // Review salvage helper: `review_salvage_collect_done_results` in
    // `coordinator.rs` — this is the review wave's own salvage path.
    // It is structurally separate from the exec/fix `fail_wave` path
    // and is NOT being modified by plan 2026-07-25-005 (U2-U7).
    //
    // The review path differs from exec/fix:
    //   - Review slots use SharedReadonly (no per-slot worktree)
    //   - Review wave partial failure still calls the coordinator's
    //     `tick` which routes through the same `evaluate_phase` +
    //     `fail_wave` decision, but the review salvage helper is
    //     invoked separately by the dispatcher BEFORE calling fan-in.
    //
    // This test characterizes the current review behavior is stable.

    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = crate::loop_runner::wave::ProductionBridgeContext {
        loop_id: "u1-review-partial".to_string(),
        repo_root: std::path::PathBuf::from("/tmp/u1-repo"),
        events_path: Some(events_path.clone()),
        tasks_path: None,
    };
    let bridge =
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store.clone() as std::sync::Arc<dyn SupervisorStore>,
            context,
            std::sync::Arc::new(DefaultWorktreeFactory),
            2,
            // 2026-07-28-003 plan U4: explicit budget keeps the
            // U1 review partial characterization at the
            // historical default.
            1,
        );
    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Review, "u1-review-partial", 2, 0)
        .expect("register");

    // Review slots: no worktree binding needed (SharedReadonly).
    for _ in 0..2 {
        bridge.store().try_dispatch_next(2).expect("dispatch");
    }

    // Slot 0: completes with review evidence.
    bridge
        .record_slot_result(&store_wave_id, 0, "h0", 1)
        .expect("s0 result");
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_wave_id,
            0,
            &TerminalEvidence::from_event("review.unit.done", "{\"dimension\":\"correctness\"}"),
        )
        .expect("evidence 0");

    // Slot 1: fails with worker_timeout.
    bridge
        .record_slot_failure(
            &store_wave_id,
            1,
            ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
        )
        .expect("s1 failure");

    // Pre-commit salvage (P0-1 contract).
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

    // Review wave: 1 completed, 1 failed.
    let completed = ralph_core::CompletedWave {
        wave_id: "u1-review-partial".to_string(),
        wave_total: 2,
        results: vec![ralph_core::WaveResult {
            index: 0,
            events: vec![
                ralph_proto::Event::new("review.unit.done", "{\"dimension\":\"correctness\"}")
                    .with_source("reviewer")
                    .with_wave("u1-review-partial".to_string(), 0, 2),
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
    // Detected wave for review kind — trigger topic starts with "review."
    let detected = {
        use ralph_core::DetectedWave;
        use ralph_core::config::HatConfig;
        let events = vec![ralph_core::Event {
            topic: "review.unit.ready".to_string(),
            payload: Some("{}".to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        }];
        let hat_config = HatConfig {
            name: "reviewer".to_string(),
            concurrency: 2,
            ..HatConfig::default()
        };
        DetectedWave {
            wave_id: "u1-review-partial".to_string(),
            target_hat: ralph_proto::HatId::new("reviewer"),
            hat_config,
            events,
            total: 2,
            partial: true,
            consumer_aggregate_timeout: None,
        }
    };

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "review partial failure must inject review.wave.failed"
    );

    // Review path: the `review_salvage_collect_done_results` helper
    // (coordinator.rs) is responsible for collecting done results in
    // the review path. This test PASSES on current HEAD to document
    // that the review path is NOT being changed by the exec/fix
    // salvage work (U2-U7). The review salvage helper operates
    // separately from `run_supervisor_fan_in` and is out of scope
    // for the exec/fix partial-failure salvage work.
    //
    // NOTE: the review wave failed payload uses `missing_dimensions`
    // (not `blocking_slots`) — the payload structure differs from
    // exec/fix waves. We assert the coord event topic and the
    // fan-in outcome without checking payload fields that don't apply
    // to review waves.

    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();

    let failed_events: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("review.wave.failed"))
        .collect();
    assert_eq!(
        failed_events.len(),
        1,
        "exactly one review.wave.failed coord event expected; got {}",
        failed_events.len()
    );

    // Review wave failed payload uses `missing_dimensions`, not `blocking_slots`.
    // Verify the payload has the expected review-wave structure.
    let payload_obj = failed_events[0]
        .get("payload")
        .and_then(|p| p.as_object())
        .expect("review.wave.failed must have a JSON object payload");
    assert!(
        payload_obj.contains_key("wave_id"),
        "review failed payload must have wave_id"
    );
    assert!(
        payload_obj.contains_key("missing_dimensions"),
        "review failed payload must have missing_dimensions (not blocking_slots)"
    );
    assert!(
        payload_obj.contains_key("reason"),
        "review failed payload must have reason"
    );
    // Verify the wave_id matches.
    assert_eq!(
        payload_obj.get("wave_id").and_then(|v| v.as_str()),
        Some("u1-review-partial"),
        "wave_id must match"
    );
}

/// U1 Test 3 (2026-07-25-005 plan U1): `build_wave_failed_payload`
/// for an exec wave with 1 Completed + 1 Failed carries the
/// salvage/redrive fields introduced by U1: `salvaged_slots` and
/// `redrive_slots`. The payload contains `wave_id`, `reason`,
/// `blocking_slots`, `slot_failures`, plus
/// `salvaged_slots` (completed slot indices in a failed wave)
/// and `redrive_slots` (retryable failed slots).
///
/// This test was originally written as a GREEN gap-lock asserting
/// these fields were absent before U1 landed. U1 (salvage/redrive)
/// has since landed, so the test is updated — per its own original
/// directive — to assert the new fields ARE present with the
/// expected index sets: slot 0 completed → `salvaged_slots == [0]`;
/// blocking slot 1 failed with retryable `worker_timeout` →
/// `redrive_slots == [1]`.
#[test]
fn build_wave_failed_payload_includes_salvaged_redrive_fields_on_exec_path() {
    use crate::loop_runner::wave::build_wave_failed_payload;
    use ralph_core::supervisor::WaveKind;
    use ralph_core::{CompletedWave, WaveFailure, WaveResult};
    use std::time::Duration;

    // 2-slot exec wave: slot 0 completed, slot 1 failed.
    let completed = CompletedWave {
        wave_id: "u1-exec-failed".to_string(),
        wave_total: 2,
        results: vec![WaveResult {
            index: 0,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-0\"}")
                    .with_source("executor"),
            ],
        }],
        failures: vec![WaveFailure {
            index: 1,
            error: "worker_timeout".to_string(),
            duration: Duration::from_secs(300),
            expected_dimension: None,
            actual_dimension: None,
        }],
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
        vec![1], // blocking_slots = [1]
        &std::collections::HashMap::new(),
        None,
        None,
    );
    let obj = payload.as_object().expect("payload must be a JSON object");

    // Legacy fields must be present (U7 does not remove these).
    assert!(
        obj.contains_key("wave_id"),
        "wave_id must be present (legacy field)"
    );
    assert!(
        obj.contains_key("blocking_slots"),
        "blocking_slots must be present (legacy field)"
    );
    assert!(
        obj.contains_key("slot_failures"),
        "slot_failures must be present (already added by prior work)"
    );

    // U1 salvage/redrive fields must be present (gap closed by
    // 2026-07-25-005 plan U1):
    //   - `salvaged_slots`: completed slot indices, ascending → [0]
    //   - `redrive_slots`: blocking slots with retryable frozen
    //     reason; slot 1 failed with `worker_timeout` (retryable) → [1]
    assert_eq!(
        obj.get("salvaged_slots").and_then(|v| v.as_array()),
        Some(&vec![serde_json::json!(0)]),
        "salvaged_slots must list the completed slot index (slot 0)"
    );
    assert_eq!(
        obj.get("redrive_slots").and_then(|v| v.as_array()),
        Some(&vec![serde_json::json!(1)]),
        "redrive_slots must list the retryable blocking slot (slot 1, worker_timeout)"
    );
}
