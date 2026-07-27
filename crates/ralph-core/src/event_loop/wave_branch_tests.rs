// 2026-07-28-001 plan U2: typed branch tests for exec_wave state machine.
// Covers:
//   R3/S3 (unit topics non-transition): exec.unit.{ready,done,failed}
//     do not advance exec_wave step
//   R4/S4 (wave terminal transition): exec.wave.complete / exec.wave.failed
//     advance exec_wave → unit_review
//
// This module is self-contained: it duplicates parallel_forge_flow()
// so it does not depend on sibling cfg(test) modules. The BDD fixture
// (tests/scenarios/parallel_forge_exec_wave_branch.yml) exercises the
// same topology via run_workflow_guard_scenario (real EventLoop).

#[cfg(test)]
mod wave_branch_tests {
    use crate::config::{
        EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig,
    };
    // Access helpers at the crate root (mod.rs level) — these are pub(crate)
    // and visible to all cfg(test) modules in the event_loop module group.
    use crate::event_loop::advance_plan_step;
    use crate::event_loop::recover_current_plan_step;

    /// Build a RalphConfig mirroring parallel-forge's flow declaration.
    /// Self-contained duplicate — does NOT pull from flow_authority_pf_recovery_tests
    /// so this module compiles independently.
    fn parallel_forge_flow() -> RalphConfig {
        let mk = |id: &str,
                  allowed: Vec<&str>,
                  on: Option<&str>,
                  on_any_of: Vec<&str>,
                  runs: Option<&str>| FlowStepConfig {
            id: id.to_string(),
            kind: if runs.is_some() {
                Some("side_effect".to_string())
            } else if id == "planning" {
                Some("linear".to_string())
            } else if id == "integration" {
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

    // -------------------------------------------------------------------------
    // R3 / S3: per-unit topics are NON-transitions within exec_wave
    // -------------------------------------------------------------------------

    /// R3: exec.unit.done does not advance exec_wave.
    #[test]
    fn pf_wave_r3_exec_unit_done_is_non_transition() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let next = advance_plan_step(&cfg, &at_exec, "exec.unit.done");
        assert_eq!(
            next, None,
            "R3: exec.unit.done must not advance exec_wave step"
        );
    }

    /// R3: exec.unit.failed does not advance exec_wave.
    #[test]
    fn pf_wave_r3_exec_unit_failed_is_non_transition() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let next = advance_plan_step(&cfg, &at_exec, "exec.unit.failed");
        assert_eq!(
            next, None,
            "R3: exec.unit.failed must not advance exec_wave step"
        );
    }

    /// S3: exec.unit.ready does not advance exec_wave.
    #[test]
    fn pf_wave_s3_exec_unit_ready_is_non_transition() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let next = advance_plan_step(&cfg, &at_exec, "exec.unit.ready");
        assert_eq!(
            next, None,
            "S3: exec.unit.ready must not advance exec_wave step"
        );
    }

    /// S3: recover_current_plan_step fold with exec.unit.done stays at exec_wave.
    #[test]
    fn pf_wave_s3_recover_fold_exec_unit_done_stays() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let recovered =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.unit.done"]);
        assert_eq!(
            recovered, "exec_wave",
            "S3: recover_current_plan_step fold with exec.unit.done stays at exec_wave"
        );
    }

    /// S3: recover_current_plan_step fold with exec.unit.failed stays at exec_wave.
    #[test]
    fn pf_wave_s3_recover_fold_exec_unit_failed_stays() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let recovered =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.unit.failed"]);
        assert_eq!(
            recovered, "exec_wave",
            "S3: recover_current_plan_step fold with exec.unit.failed stays at exec_wave"
        );
    }

    // -------------------------------------------------------------------------
    // R4 / S4: exec.wave.complete / exec.wave.failed ARE step transitions
    // -------------------------------------------------------------------------

    /// R4: exec.wave.complete advances exec_wave → unit_review.
    #[test]
    fn pf_wave_r4_exec_wave_complete_advances_to_unit_review() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let next = advance_plan_step(&cfg, &at_exec, "exec.wave.complete");
        assert_eq!(
            next,
            Some("unit_review".to_string()),
            "R4: exec.wave.complete must advance exec_wave → unit_review"
        );
    }

    /// R4: exec.wave.failed advances exec_wave → unit_review.
    #[test]
    fn pf_wave_r4_exec_wave_failed_advances_to_unit_review() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let next = advance_plan_step(&cfg, &at_exec, "exec.wave.failed");
        assert_eq!(
            next,
            Some("unit_review".to_string()),
            "R4: exec.wave.failed must advance exec_wave → unit_review"
        );
    }

    /// S4: recover_current_plan_step fold exec.wave.complete → unit_review.
    #[test]
    fn pf_wave_s4_recover_fold_exec_wave_complete_to_unit_review() {
        let cfg = parallel_forge_flow();
        let recovered =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.wave.complete"]);
        assert_eq!(
            recovered, "unit_review",
            "S4: recover_current_plan_step fold exec.wave.complete → unit_review"
        );
    }

    /// S4: recover_current_plan_step fold exec.wave.failed → unit_review.
    #[test]
    fn pf_wave_s4_recover_fold_exec_wave_failed_to_unit_review() {
        let cfg = parallel_forge_flow();
        let recovered =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.wave.failed"]);
        assert_eq!(
            recovered, "unit_review",
            "S4: recover_current_plan_step fold exec.wave.failed → unit_review"
        );
    }

    // -------------------------------------------------------------------------
    // Idempotency: repeated wave terminals do not double-advance
    // -------------------------------------------------------------------------

    /// Idempotency: repeated exec.wave.complete at unit_review stays put.
    #[test]
    fn pf_wave_repeated_exec_wave_complete_stays_at_unit_review() {
        let cfg = parallel_forge_flow();
        let at_review =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.wave.complete"]);
        assert_eq!(
            at_review, "unit_review",
            "pre-condition: must be at unit_review"
        );
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
            "repeated exec.wave.complete must not backstep from unit_review"
        );
    }

    /// Idempotency: repeated exec.wave.failed at unit_review stays put.
    #[test]
    fn pf_wave_repeated_exec_wave_failed_stays_at_unit_review() {
        let cfg = parallel_forge_flow();
        let at_review =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.wave.failed"]);
        assert_eq!(
            at_review, "unit_review",
            "pre-condition: must be at unit_review"
        );
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.concurrency.approved",
                "exec.wave.failed",
                "exec.wave.failed",
            ],
        );
        assert_eq!(
            recovered, "unit_review",
            "repeated exec.wave.failed must not backstep from unit_review"
        );
    }

    // -------------------------------------------------------------------------
    // Cross-check: NON_TRANSITION_TOPICS constant covers all per-unit exec topics
    // -------------------------------------------------------------------------

    /// Verify all per-unit exec topics are non-transitions (defence-in-depth).
    #[test]
    fn pf_wave_all_exec_unit_topics_are_non_transitions() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave");
        for topic in &["exec.unit.ready", "exec.unit.done", "exec.unit.failed"] {
            let next = advance_plan_step(&cfg, at_exec.as_str(), topic);
            assert_eq!(
                next, None,
                "{topic} must be a non-transition within exec_wave (NON_TRANSITION_TOPICS)"
            );
        }
    }

    /// Verify all exec.wave.* topics ARE transitions (constant not over-applied).
    #[test]
    fn pf_wave_all_exec_wave_topics_are_transitions() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave");
        for topic in &["exec.wave.complete", "exec.wave.failed"] {
            let next = advance_plan_step(&cfg, at_exec.as_str(), topic);
            assert_eq!(
                next,
                Some("unit_review".to_string()),
                "{topic} must be a transition within exec_wave → unit_review"
            );
        }
    }
}
