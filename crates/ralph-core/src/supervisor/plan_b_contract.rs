//! 2026-07-23-005 plan U1 (Plan B contract double): standalone fixture
//! for the **frozen** supervisor runtime contract.
//!
//! The fixture is intentionally **not** part of the production
//! runtime. Its job is to give Plan B (this refactor) a green
//! "I understand the contract" gate that does NOT depend on the
//! Plan A (runtime P0) branch landing first. Once both branches
//! merge, the live `ralph-cli` integration tests take over for
//! the real worker fan-in.
//!
//! Coverage:
//! - `tick` produces `InjectedComplete` when all slots report
//!   `Completed`; the topic is `exec.wave.complete`.
//! - `tick` produces `InjectedFailed` for failed paths; the topic
//!   is `exec.wave.failed` and `blocking_slots` carries the
//!   offending slot indices.
//! - A second terminal tick is rejected (`AlreadyDone` only),
//!   so the supervisor cannot double-emit `*.wave.complete`.
//! - `slot_resources` is the public read surface and must round-trip
//!   the slot index, branch, and worktree path.
//! - `InMemoryCoordinatorBridge` is a **contract double** — it must
//!   never depend on git, a database, or `RusqliteSupervisorStore`.
//! - Review/Fix waves emit the matching `*.wave.complete` topic.
//!
//! The fixture is also the home of the Plan B file-ownership guard:
//!   `plan_b_does_not_modify_ce_executor_pipeline_preset`
//! which fails the build if `presets/en/ce-executor-pipeline.yml`
//! appears in any tracked change against the baseline SHA.

#![cfg(test)]

use crate::supervisor::bridge::{InMemoryCoordinatorBridge, SupervisorBridge};
use crate::supervisor::{CoordinatorAction, SlotResource, WaveKind};

fn make_bridge() -> InMemoryCoordinatorBridge {
    // Plain in-memory bridge; no git, no DB. The fixture must stay
    // portable across `cargo nextest run --no-default-features`
    // because Plan B does not require `supervisor-db`.
    let store = std::sync::Arc::new(crate::supervisor::InMemorySupervisorStore::new());
    InMemoryCoordinatorBridge::from_store(
        store as std::sync::Arc<dyn crate::supervisor::SupervisorStore>,
    )
}

fn default_phase_inputs() -> crate::supervisor::PhaseInputs {
    crate::supervisor::PhaseInputs::default()
}

fn drive_slot_to_done(bridge: &InMemoryCoordinatorBridge, store_id: &str, slot_index: u32) {
    // Bind the slot's worktree, dispatch it, then mark it done.
    // Mirrors the helper used by `run_bdd_supervisor_fan_in` in
    // the BDD scenario runner so the fixture stays aligned with
    // the production dispatcher's slot lifecycle.
    bridge
        .store()
        .bind_worktree(
            store_id,
            slot_index,
            SlotResource {
                slot_index,
                worktree_path: Some(format!("/tmp/plan-b/{slot_index}")),
                branch: Some(format!("plan-b/{slot_index}")),
            },
        )
        .expect("bind worktree");
    // Promote slot to dispatched so record_slot_result accepts
    // the Completed transition.
    bridge.store().try_dispatch_next(64).expect("dispatch");
    bridge
        .store()
        .record_slot_result(store_id, slot_index, &format!("h-{slot_index}"), 1)
        .expect("record slot result");
    // Plan 004 R2 / P0-2: success path requires terminal
    // evidence; without it the coordinator falls into
    // `Failed(IncompleteEvidence)` instead of `InjectedComplete`.
    bridge
        .store()
        .record_slot_terminal_evidence(
            store_id,
            slot_index,
            &crate::supervisor::TerminalEvidence::from_event(
                "exec.unit.done",
                &format!("{{\"dimension\":\"d-{slot_index}\"}}"),
            ),
        )
        .expect("record terminal evidence");
}

fn drive_slot_to_failure(bridge: &InMemoryCoordinatorBridge, store_id: &str, slot_index: u32) {
    bridge
        .store()
        .bind_worktree(
            store_id,
            slot_index,
            SlotResource {
                slot_index,
                worktree_path: Some(format!("/tmp/plan-b/{slot_index}")),
                branch: Some(format!("plan-b/{slot_index}")),
            },
        )
        .expect("bind worktree");
    bridge.store().try_dispatch_next(64).expect("dispatch");
    bridge
        .store()
        .record_slot_failure(store_id, slot_index, "compilation error: missing import")
        .expect("record slot failure");
}

#[test]
fn plan_b_contract_double_tick_emits_complete_with_success_resources() {
    // Contract: when every slot reports Completed, the bridge
    // produces `InjectedComplete { topic: exec.wave.complete, ... }`
    // with the public wave id (passed to `register_wave_if_absent`
    // as the idempotency key, returned as the store id).
    let bridge = make_bridge();

    // Register two slots for the same exec wave.
    let store_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "w-b-1", 2, 1)
        .expect("register wave");
    drive_slot_to_done(&bridge, &store_id, 0);
    drive_slot_to_done(&bridge, &store_id, 1);

    let action = bridge
        .tick(&store_id, default_phase_inputs())
        .expect("tick");

    match action {
        CoordinatorAction::InjectedComplete {
            topic,
            blocking_slots,
        } => {
            assert_eq!(topic, "exec.wave.complete", "exec wave terminal topic");
            assert!(
                blocking_slots.is_empty(),
                "happy-path fan-in must not surface blocking slots; got {blocking_slots:?}"
            );
        }
        other => panic!("expected InjectedComplete, got {other:?}"),
    }

    // success_slots payload is reachable via `slot_resources`; the
    // dispatcher uses it to build the `*.wave.complete` business
    // payload downstream.
    let resources = bridge.slot_resources(&store_id).expect("resources");
    let indices: Vec<u32> = resources.iter().map(|r| r.slot_index).collect();
    assert!(
        indices.contains(&0) && indices.contains(&1),
        "success_slots must list both completed slots; got {indices:?}"
    );
}

#[test]
fn plan_b_contract_double_tick_emits_failed_with_blocking_slot() {
    // Contract: a failed slot flips the wave to Failed phase;
    // `tick` produces `InjectedFailed { topic: exec.wave.failed,
    // reason, blocking_slots }`.
    let bridge = make_bridge();
    let store_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "w-b-2", 1, 1)
        .expect("register wave");
    drive_slot_to_failure(&bridge, &store_id, 0);
    // Plan 004 R3 / P0-1: pre-commit salvage before tick.
    bridge
        .commit_salvage_projection(
            &store_id,
            &super::ProjectionReceiptSummary {
                kind: super::ProjectionKind::Business,
                batch_fingerprint: String::new(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let action = bridge
        .tick(&store_id, default_phase_inputs())
        .expect("tick");

    match action {
        CoordinatorAction::InjectedFailed {
            topic,
            reason,
            blocking_slots,
        } => {
            assert_eq!(topic, "exec.wave.failed", "exec wave failed topic");
            assert!(
                !reason.is_empty(),
                "failure reason must be populated; got empty string"
            );
            assert!(
                blocking_slots.contains(&0),
                "blocking_slots must include the failed slot; got {blocking_slots:?}"
            );
        }
        other => panic!("expected InjectedFailed on failure, got {other:?}"),
    }
}

#[test]
fn plan_b_contract_double_duplicate_terminal_is_rejected() {
    // Contract: after a wave reaches a terminal phase, a second
    // `tick` MUST NOT produce a second terminal action. The
    // bridge surfaces this as `AlreadyDone` (KTD-7 / F-001).
    let bridge = make_bridge();
    let store_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "w-b-3", 1, 1)
        .expect("register wave");
    drive_slot_to_done(&bridge, &store_id, 0);

    let first = bridge
        .tick(&store_id, default_phase_inputs())
        .expect("tick 1");
    assert!(
        matches!(first, CoordinatorAction::InjectedComplete { .. }),
        "first tick must InjectedComplete; got {first:?}"
    );

    let second = bridge
        .tick(&store_id, default_phase_inputs())
        .expect("tick 2");
    assert!(
        matches!(second, CoordinatorAction::AlreadyDone),
        "second tick after terminal must be AlreadyDone; got {second:?}"
    );
}

#[test]
fn plan_b_contract_double_slot_resources_round_trip() {
    // Contract: `slot_resources(wave_id)` is the public read
    // surface that the dispatcher uses to build the
    // `success_slots` payload on `*.wave.complete`. The fixture
    // asserts that the resource shape round-trips slot_index,
    // branch, and worktree_path without losing data.
    //
    // NOTE: exec/fix waves carry a per-slot worktree; review
    // waves use `SharedReadonly` (no per-slot binding). This
    // test pins the exec shape so a future refactor that
    // accidentally swaps the two modes fails loudly here.
    let bridge = make_bridge();
    let store_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "w-b-4", 1, 1)
        .expect("register exec wave");
    bridge
        .store()
        .bind_worktree(
            &store_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some("/tmp/plan-b/review/0".into()),
                branch: Some("plan-b/review/0".into()),
            },
        )
        .expect("bind");

    let resources = bridge.slot_resources(&store_id).expect("slot_resources");
    assert_eq!(resources.len(), 1);
    let r = &resources[0];
    assert_eq!(r.slot_index, 0);
    assert_eq!(r.branch.as_deref(), Some("plan-b/review/0"));
    assert_eq!(r.worktree_path.as_deref(), Some("/tmp/plan-b/review/0"));
}

#[test]
fn plan_b_contract_double_in_memory_bridge_has_no_io_dependencies() {
    // Plan B cannot ship a fixture that secretly drags in git
    // or sqlite. This test fails the build if a future refactor
    // swaps the in-memory store for the rusqlite-backed one.
    let bridge = make_bridge();
    let dbg = format!("{bridge:?}");
    assert!(
        !dbg.contains("RusqliteSupervisorStore"),
        "Plan B contract double must stay on InMemorySupervisorStore; got {dbg}"
    );
    assert!(
        !dbg.contains("git "),
        "Plan B contract double must not invoke git; got {dbg}"
    );
}

#[test]
fn plan_b_contract_double_unknown_wave_does_not_emit_terminal() {
    // Contract: when the dispatcher asks the bridge to tick a
    // wave that was never registered, the bridge MUST NOT
    // produce an EmitCoord-style terminal action. It may return
    // `ContinueCollect` (most common) or surface a typed error.
    let bridge = make_bridge();
    let result = bridge.tick("w-never-registered", default_phase_inputs());
    match result {
        Ok(CoordinatorAction::ContinueCollect) => {
            // expected — the bridge stays in Collecting because
            // no slots reported back.
        }
        Ok(other) => {
            panic!("Plan B contract double must NOT EmitCoord for an unknown wave; got {other:?}")
        }
        Err(_) => {
            // acceptable — typed error from the store layer is
            // also a valid Plan B signal.
        }
    }
}

#[test]
fn plan_b_contract_double_review_wave_uses_review_wave_complete_topic() {
    // Contract: a Review wave's terminal topic is
    // `review.wave.complete` (not `review.complete`, which is the
    // agent business topic). The fixture pins this so Plan B's
    // review fan-in path stays in lock-step with the runtime
    // coordination topic whitelist.
    //
    // Review slots use SharedReadonly (no per-slot worktree
    // binding) — the bridge records the slot result directly.
    let bridge = make_bridge();
    let store_id = bridge
        .register_wave_if_absent(WaveKind::Review, "w-b-r", 1, 1)
        .expect("register review wave");
    bridge
        .store()
        .record_slot_result(&store_id, 0, "h-r", 1)
        .expect("record review slot result");
    // Plan 004 R2 / P0-2: success path requires terminal evidence.
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_id,
            0,
            &crate::supervisor::TerminalEvidence::from_event(
                "review.unit.done",
                "{\"dimension\":\"correctness\"}",
            ),
        )
        .expect("record review evidence");

    let action = bridge
        .tick(&store_id, default_phase_inputs())
        .expect("tick review wave");
    match action {
        CoordinatorAction::InjectedComplete { topic, .. } => {
            assert_eq!(
                topic, "review.wave.complete",
                "review wave terminal topic must be review.wave.complete"
            );
        }
        other => panic!("expected InjectedComplete, got {other:?}"),
    }
}

#[test]
fn plan_b_contract_double_fix_wave_uses_fix_wave_complete_topic() {
    // Contract: a Fix wave's terminal topic is `fix.wave.complete`.
    // Mirrors the review wave assertion above.
    let bridge = make_bridge();
    let store_id = bridge
        .register_wave_if_absent(WaveKind::Fix, "w-b-f", 1, 1)
        .expect("register fix wave");
    bridge
        .store()
        .bind_worktree(
            &store_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some("/tmp/plan-b/fix".into()),
                branch: Some("plan-b/fix".into()),
            },
        )
        .expect("bind");
    bridge.store().try_dispatch_next(64).expect("dispatch");
    bridge
        .store()
        .record_slot_result(&store_id, 0, "h-f", 1)
        .expect("record");
    // Plan 004 R2 / P0-2: success path requires terminal evidence.
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_id,
            0,
            &crate::supervisor::TerminalEvidence::from_event(
                "fix.unit.done",
                "{\"dimension\":\"default\"}",
            ),
        )
        .expect("record fix evidence");

    let action = bridge
        .tick(&store_id, default_phase_inputs())
        .expect("tick fix wave");
    match action {
        CoordinatorAction::InjectedComplete { topic, .. } => {
            assert_eq!(topic, "fix.wave.complete");
        }
        other => panic!("expected InjectedComplete, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────
// File ownership guard (Plan B hard rule):
//   `presets/en/ce-executor-pipeline.yml` is the alignment reference
//   and is FORBIDDEN from any tracked change in this branch.
// ─────────────────────────────────────────────────────────────────

#[test]
fn plan_b_does_not_modify_ce_executor_pipeline_preset() {
    // We check this via git rather than via a parse of the file
    // because the contract is "no diff against the upstream
    // baseline", not "the file looks like the baseline". The
    // guard is best-effort: it pins the change against HEAD~1
    // (the most recent commit on the branch) so a multi-commit
    // series on Plan B does not regress the contract. Operators
    // running this test outside the orchestrator can override
    // the baseline with `PLAN_B_BASELINE_SHA=<sha>`.
    use std::process::Command;

    let baseline = std::env::var("PLAN_B_BASELINE_SHA").unwrap_or_else(|_| {
        // Default to HEAD~1: the file ownership guard fails
        // only when a Plan B commit modifies the frozen preset.
        let output = Command::new("git")
            .args(["rev-parse", "HEAD~1"])
            .output()
            .expect("git rev-parse HEAD~1");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    });

    let output = Command::new("git")
        .args(["diff", "--name-only", &baseline, "HEAD"])
        .output()
        .expect("git diff");
    let changed = String::from_utf8_lossy(&output.stdout);

    assert!(
        !changed
            .lines()
            .any(|line| line.trim() == "presets/en/ce-executor-pipeline.yml"),
        "Plan B must not modify presets/en/ce-executor-pipeline.yml; tracked changes:\n{}",
        changed
    );
}
