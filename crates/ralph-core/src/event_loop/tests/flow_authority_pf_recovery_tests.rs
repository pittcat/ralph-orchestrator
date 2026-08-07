use super::super::*;
use crate::config::{
    EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig,
};

/// Build a RalphConfig that mirrors parallel-forge's flow declaration.
/// Identical to the version in u4_current_plan_step_tests (mod.rs:15301).
fn parallel_forge_flow() -> RalphConfig {
    let mk = |id: &str,
              allowed: Vec<&str>,
              on: Option<&str>,
              on_any_of: Vec<&str>,
              runs: Option<&str>| FlowStepConfig {
        id: id.to_string(),
        kind: if runs.is_some() {
            Some("side_effect".to_string())
        } else if matches!(id, "planning" | "integration") {
            Some("linear".to_string())
        } else {
            None
        },
        allowed_emits: allowed.into_iter().map(String::from).collect(),
        terminal_when: None,
        on_partial: std::collections::BTreeMap::new(),
        runs: runs.map(String::from),
        on: on.map(String::from),
        on_any_of: on_any_of.into_iter().map(String::from).collect(),
        transition_emits: Vec::new(),
    };

    let mut cfg = RalphConfig::default();
    cfg.event_loop = EventLoopConfig {
        mechanism: Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: vec![
                    mk(
                        "planning",
                        vec![
                            "forge.plan.inspected",
                            "forge.plan.ready",
                            "forge.concurrency.approved",
                            "forge.worktrees.ready",
                            "forge.plan.blocked",
                        ],
                        None,
                        vec![],
                        None,
                    ),
                    mk(
                        "exec_wave",
                        vec![
                            "exec.wave.complete",
                            "exec.wave.failed",
                            "exec.unit.ready",
                            "exec.unit.done",
                            "exec.unit.failed",
                            "forge.exec.development.done",
                        ],
                        Some("forge.worktrees.ready"),
                        vec![],
                        Some("supervisor.exec.wave"),
                    ),
                    mk(
                        "unit_review",
                        vec!["forge.units.reviewed"],
                        Some("forge.exec.development.done"),
                        vec![],
                        None,
                    ),
                    mk(
                        "integration",
                        vec![
                            "forge.integration.done",
                            "forge.incremental.verified",
                            "forge.full.verified",
                            "forge.audit.done",
                            "forge.report.done",
                            "work.failed",
                        ],
                        Some("forge.units.reviewed"),
                        vec![],
                        None,
                    ),
                    mk(
                        "plan_end",
                        vec!["forge.report.done", "LOOP_COMPLETE"],
                        None,
                        vec![],
                        None,
                    ),
                ],
                ..FlowDeclarationConfig::default()
            }),
            phase_authority: None,
        }),
        ..EventLoopConfig::default()
    };
    cfg
}

// R1/S1: recover_current_plan_step folds the planning handoff
// sequence correctly: empty → forge.concurrency.approved → exec_wave.
#[test]
fn pf_recovery_r1_planning_handoff_folds_to_exec_wave() {
    let cfg = parallel_forge_flow();
    let initial = initial_current_plan_step(&cfg);
    assert_eq!(initial, "planning", "R1: initial step must be planning");
    let recovered = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
    assert_eq!(
        recovered, "exec_wave",
        "R1: forge.concurrency.approved must advance planning → exec_wave"
    );
}

/// R1/S1 variant: forge.worktrees.ready is the concurrency approval signal.
#[test]
fn pf_recovery_r1_worktrees_ready_folds_to_exec_wave() {
    let cfg = parallel_forge_flow();
    let recovered = recover_current_plan_step(&cfg, &["forge.worktrees.ready"]);
    assert_eq!(
        recovered, "exec_wave",
        "R1: forge.worktrees.ready must advance planning → exec_wave"
    );
}

/// R2/S2: forge.plan.blocked at planning is in allowed_emits but has no
/// declared `on` transition, so advance_plan_step falls back to linear
/// advance (planning → exec_wave). This is a known plan-vs-rule gap:
/// the executor HARDS RULES forbid editing presets/en/, so the
/// terminal-report semantics for forge.plan.blocked cannot be wired
/// here. Recorded as a plan flaw in .ralph/agent/decisions.md.
#[test]
fn pf_recovery_r2_plan_blocked_at_planning_linear_advance_to_exec_wave() {
    let cfg = parallel_forge_flow();
    let recovered = recover_current_plan_step(&cfg, &["forge.plan.blocked"]);
    assert_eq!(
        recovered, "exec_wave",
        "R2 GAP: forge.plan.blocked currently advances via linear fallback \
             (terminal-report semantics require preset YAML edit; out of executor scope)"
    );
}

/// R7/S7: forge.plan.blocked is idempotent on repeat (same linear advance
/// applies on both first and second emission). The terminal semantics
/// (staying put) requires an explicit non-transition declaration in YAML.
#[test]
fn pf_recovery_r7_forge_plan_blocked_idempotent_linear_fallback() {
    let cfg = parallel_forge_flow();
    // First emission: linear fallback advances planning → exec_wave
    let after_block = recover_current_plan_step(&cfg, &["forge.plan.blocked"]);
    assert_eq!(after_block, "exec_wave");
    // Second emission: same linear fallback, still idempotent (no double-advance)
    let recovered = recover_current_plan_step(&cfg, &["forge.plan.blocked", "forge.plan.blocked"]);
    assert_eq!(
        recovered, "exec_wave",
        "R7: repeated forge.plan.blocked is idempotent (linear fallback is deterministic)"
    );
}

/// R7/S7: forge.plan.blocked at exec_wave is not a transition; fold stays.
#[test]
fn pf_recovery_r7_plan_blocked_at_exec_wave_stays_at_exec_wave() {
    let cfg = parallel_forge_flow();
    let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
    assert_eq!(at_exec, "exec_wave");
    let recovered =
        recover_current_plan_step(&cfg, &["forge.concurrency.approved", "forge.plan.blocked"]);
    assert_eq!(
        recovered, "exec_wave",
        "R7: forge.plan.blocked at exec_wave must not trigger a transition"
    );
}

/// R9/S9: old planning events do NOT backstep after advancing to exec_wave.
#[test]
fn pf_recovery_r9_old_planning_events_do_not_backstep_at_exec_wave() {
    let cfg = parallel_forge_flow();
    let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
    assert_eq!(at_exec, "exec_wave");
    let recovered =
        recover_current_plan_step(&cfg, &["forge.concurrency.approved", "forge.plan.ready"]);
    assert_eq!(
        recovered, "exec_wave",
        "R9: old forge.plan.ready after exec_wave must not backstep"
    );
}

/// R9/S9: repeated transition event is idempotent — stays at exec_wave.
#[test]
fn pf_recovery_r9_repeated_concurrency_approved_stays_at_exec_wave() {
    let cfg = parallel_forge_flow();
    let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
    assert_eq!(at_exec, "exec_wave");
    let recovered = recover_current_plan_step(
        &cfg,
        &["forge.concurrency.approved", "forge.concurrency.approved"],
    );
    assert_eq!(
        recovered, "exec_wave",
        "R9: repeated forge.concurrency.approved must not backstep"
    );
}

/// Full happy-path fold: planning → exec_wave → unit_review → integration → plan_end.
#[test]
fn pf_recovery_full_happy_path_folds_through_all_steps() {
    let cfg = parallel_forge_flow();
    let step1 = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
    assert_eq!(step1, "exec_wave");
    let step2 =
        recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.wave.complete"]);
    assert_eq!(step2, "unit_review");
    let step3 = recover_current_plan_step(
        &cfg,
        &[
            "forge.concurrency.approved",
            "exec.wave.complete",
            "forge.units.reviewed",
        ],
    );
    assert_eq!(step3, "integration");
    let step4 = recover_current_plan_step(
        &cfg,
        &[
            "forge.concurrency.approved",
            "exec.wave.complete",
            "forge.units.reviewed",
            "forge.report.done",
        ],
    );
    assert_eq!(step4, "plan_end");
}

/// S1: exec.unit.done is a per-unit terminal, NOT a step transition.
#[test]
fn pf_recovery_s1_exec_unit_done_is_non_transition() {
    let cfg = parallel_forge_flow();
    let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
    assert_eq!(at_exec, "exec_wave");
    let recovered =
        recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.unit.done"]);
    assert_eq!(
        recovered, "exec_wave",
        "S1: exec.unit.done must not advance exec_wave step"
    );
}

/// S2: exec.unit.failed is a per-unit terminal, NOT a step transition.
#[test]
fn pf_recovery_s2_exec_unit_failed_is_non_transition() {
    let cfg = parallel_forge_flow();
    let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
    assert_eq!(at_exec, "exec_wave");
    let recovered =
        recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.unit.failed"]);
    assert_eq!(
        recovered, "exec_wave",
        "S2: exec.unit.failed must not advance exec_wave step"
    );
}

/// R7/S7: forge.plan.blocked at integration is not in allowed_emits; fold stays.
#[test]
fn pf_recovery_r7_plan_blocked_at_integration_not_in_allowed_emits() {
    let cfg = parallel_forge_flow();
    let at_integration = recover_current_plan_step(
        &cfg,
        &[
            "forge.concurrency.approved",
            "exec.wave.complete",
            "forge.units.reviewed",
        ],
    );
    assert_eq!(at_integration, "integration");
    let recovered = recover_current_plan_step(
        &cfg,
        &[
            "forge.concurrency.approved",
            "exec.wave.complete",
            "forge.units.reviewed",
            "forge.plan.blocked",
        ],
    );
    assert_eq!(
        recovered, "integration",
        "R7: forge.plan.blocked at integration must not trigger a transition"
    );
}

/// R9/S9: repeated exec.wave.complete must not backstep from unit_review.
#[test]
fn pf_recovery_r9_repeated_exec_wave_complete_stays_at_unit_review() {
    let cfg = parallel_forge_flow();
    let at_review =
        recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.wave.complete"]);
    assert_eq!(at_review, "unit_review");
    let recovered = recover_current_plan_step(
        &cfg,
        &[
            "forge.concurrency.approved",
            "exec.wave.complete",
            "exec.wave.complete",
        ],
    );
    assert_eq!(
        recovered, "unit_review",
        "R9: repeated exec.wave.complete must not backstep to exec_wave"
    );
}
