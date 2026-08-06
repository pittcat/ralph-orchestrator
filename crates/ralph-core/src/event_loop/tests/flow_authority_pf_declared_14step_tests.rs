    use super::super::*;
    use crate::config::{
        EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig,
    };

    /// Build a RalphConfig mirroring the target 14-step parallel-forge
    /// flow declaration from plan §3.1.
    fn parallel_forge_14step_flow() -> RalphConfig {
        let mk = |id: &str,
                  kind: Option<&str>,
                  allowed: Vec<&str>,
                  on: Option<&str>,
                  on_any_of: Vec<&str>,
                  runs: Option<&str>| FlowStepConfig {
            id: id.to_string(),
            kind: kind.map(String::from),
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
                            Some("linear"),
                            vec!["forge.plan.inspected", "forge.plan.blocked"],
                            None,
                            vec![],
                            None,
                        ),
                        mk(
                            "plan_authoring",
                            Some("linear"),
                            vec!["forge.plan.ready", "forge.plan.blocked"],
                            Some("forge.plan.inspected"),
                            vec![],
                            None,
                        ),
                        mk(
                            "concurrency_review",
                            Some("linear"),
                            vec!["forge.concurrency.approved", "forge.plan.blocked"],
                            Some("forge.plan.ready"),
                            vec![],
                            None,
                        ),
                        mk(
                            "worktree_setup",
                            Some("linear"),
                            vec!["forge.worktrees.ready", "forge.plan.blocked"],
                            Some("forge.concurrency.approved"),
                            vec![],
                            None,
                        ),
                        mk(
                            "exec_wave",
                            Some("side_effect"),
                            vec![
                                "exec.unit.ready",
                                "exec.unit.done",
                                "exec.unit.failed",
                                "exec.wave.complete",
                                "exec.wave.failed",
                            ],
                            Some("forge.worktrees.ready"),
                            vec![],
                            Some("supervisor.exec.wave"),
                        ),
                        mk(
                            "exec_finalize",
                            Some("await"),
                            vec!["forge.exec.development.done"],
                            Some("exec.wave.complete"),
                            vec![],
                            None,
                        ),
                        mk(
                            "exec_failure",
                            Some("await"),
                            vec!["work.failed", "forge.report.done"],
                            Some("exec.wave.failed"),
                            vec![],
                            None,
                        ),
                        mk(
                            "unit_review",
                            Some("linear"),
                            vec!["forge.units.reviewed", "forge.plan.blocked"],
                            Some("forge.exec.development.done"),
                            vec![],
                            None,
                        ),
                        mk(
                            "integration",
                            Some("linear"),
                            vec!["forge.integration.done", "work.failed", "forge.report.done"],
                            Some("forge.units.reviewed"),
                            vec![],
                            None,
                        ),
                        mk(
                            "incremental_verify",
                            Some("linear"),
                            vec![
                                "forge.incremental.verified",
                                "work.failed",
                                "forge.report.done",
                            ],
                            Some("forge.integration.done"),
                            vec![],
                            None,
                        ),
                        mk(
                            "full_verify",
                            Some("linear"),
                            vec!["forge.full.verified", "work.failed", "forge.report.done"],
                            Some("forge.incremental.verified"),
                            vec![],
                            None,
                        ),
                        mk(
                            "audit",
                            Some("linear"),
                            vec!["forge.audit.done", "forge.plan.blocked"],
                            Some("forge.full.verified"),
                            vec![],
                            None,
                        ),
                        mk(
                            "report",
                            Some("await"),
                            vec!["forge.report.done"],
                            None,
                            // U7 (plan 2026-07-29-001): plan-level
                            // `work.failed` is now a transition.
                            // The `report` step is the universal
                            // funnel for terminal failures.
                            vec!["forge.audit.done", "forge.plan.blocked", "work.failed"],
                            None,
                        ),
                        mk(
                            "plan_end",
                            Some("terminal"),
                            vec!["LOOP_COMPLETE"],
                            Some("forge.report.done"),
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

    // ── R1/S1: planning handoff steps ──────────────────────────────────────

    /// R1: forge.plan.inspected enters plan_authoring (not exec_wave).
    #[test]
    fn pf_14step_inspected_enters_plan_authoring_not_exec_wave() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "planning", "forge.plan.inspected");
        assert_eq!(
            next,
            Some("plan_authoring".to_string()),
            "R1: forge.plan.inspected must advance planning → plan_authoring"
        );
    }

    /// R1: forge.plan.ready enters concurrency_review.
    #[test]
    fn pf_14step_plan_ready_enters_concurrency_review() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "plan_authoring", "forge.plan.ready");
        assert_eq!(
            next,
            Some("concurrency_review".to_string()),
            "R1: forge.plan.ready must advance plan_authoring → concurrency_review"
        );
    }

    /// R1: forge.concurrency.approved enters worktree_setup.
    #[test]
    fn pf_14step_concurrency_approved_enters_worktree_setup() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "concurrency_review", "forge.concurrency.approved");
        assert_eq!(
            next,
            Some("worktree_setup".to_string()),
            "R1: forge.concurrency.approved must advance concurrency_review → worktree_setup"
        );
    }

    /// R1: forge.worktrees.ready enters exec_wave.
    #[test]
    fn pf_14step_worktrees_ready_enters_exec_wave() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "worktree_setup", "forge.worktrees.ready");
        assert_eq!(
            next,
            Some("exec_wave".to_string()),
            "R1: forge.worktrees.ready must advance worktree_setup → exec_wave"
        );
    }

    // ── R2/S2: blocked branches into report ────────────────────────────────

    /// R2: forge.plan.blocked at planning enters report (not exec_wave).
    #[test]
    fn pf_14step_plan_blocked_at_planning_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "planning", "forge.plan.blocked");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.plan.blocked at planning must advance → report"
        );
    }

    /// R2: forge.plan.blocked at plan_authoring enters report.
    #[test]
    fn pf_14step_plan_blocked_at_plan_authoring_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "plan_authoring", "forge.plan.blocked");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.plan.blocked at plan_authoring must advance → report"
        );
    }

    /// R2: forge.plan.blocked at concurrency_review enters report.
    #[test]
    fn pf_14step_plan_blocked_at_concurrency_review_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "concurrency_review", "forge.plan.blocked");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.plan.blocked at concurrency_review must advance → report"
        );
    }

    /// R2: forge.plan.blocked at worktree_setup enters report.
    #[test]
    fn pf_14step_plan_blocked_at_worktree_setup_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "worktree_setup", "forge.plan.blocked");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.plan.blocked at worktree_setup must advance → report"
        );
    }

    /// R2: forge.plan.blocked at audit enters report.
    #[test]
    fn pf_14step_plan_blocked_at_audit_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "audit", "forge.plan.blocked");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.plan.blocked at audit must advance → report"
        );
    }

    /// R2: forge.audit.done enters report (on_any_of branch).
    #[test]
    fn pf_14step_audit_done_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "audit", "forge.audit.done");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.audit.done must advance audit → report"
        );
    }

    // ── R3/S3: exec_wave unit topics are non-transitions ────────────────────

    /// R3: exec.unit.done stays at exec_wave.
    #[test]
    fn pf_14step_exec_unit_done_is_non_transition() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.unit.done");
        assert_eq!(next, None, "R3: exec.unit.done must not advance exec_wave");
    }

    /// R3: exec.unit.failed stays at exec_wave.
    #[test]
    fn pf_14step_exec_unit_failed_is_non_transition() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.unit.failed");
        assert_eq!(
            next, None,
            "R3: exec.unit.failed must not advance exec_wave"
        );
    }

    /// S3: exec.unit.ready stays at exec_wave.
    #[test]
    fn pf_14step_exec_unit_ready_is_non_transition() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.unit.ready");
        assert_eq!(next, None, "S3: exec.unit.ready must not advance exec_wave");
    }

    // ── R4/S4: exec.wave.complete / exec.wave.failed branch distinctly ─────

    /// R4: exec.wave.complete enters exec_finalize (not unit_review).
    #[test]
    fn pf_14step_exec_wave_complete_enters_exec_finalize() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.wave.complete");
        assert_eq!(
            next,
            Some("exec_finalize".to_string()),
            "R4: exec.wave.complete must advance exec_wave → exec_finalize"
        );
    }

    /// R4: exec.wave.failed enters exec_failure (distinct from success).
    #[test]
    fn pf_14step_exec_wave_failed_enters_exec_failure() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.wave.failed");
        assert_eq!(
            next,
            Some("exec_failure".to_string()),
            "R4: exec.wave.failed must advance exec_wave → exec_failure"
        );
    }

    /// R4: forge.exec.development.done enters unit_review (from exec_finalize).
    #[test]
    fn pf_14step_development_done_enters_unit_review() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_finalize", "forge.exec.development.done");
        assert_eq!(
            next,
            Some("unit_review".to_string()),
            "R4: forge.exec.development.done must advance exec_finalize → unit_review"
        );
    }

    /// R4 (U7): work.failed at exec_failure is now a transition
    /// (drives the `report` step via `on_any_of`). The legacy
    /// non-transition contract applied only to per-unit `work.failed`
    /// inside the exec_wave step; the plan-level `work.failed` at
    /// exec_failure / integration must advance to keep the route
    /// open.
    #[test]
    fn pf_14step_work_failed_at_exec_failure_advances_to_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_failure", "work.failed");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R4 (U7): work.failed at exec_failure must advance → report"
        );
    }

    /// R4: forge.report.done at exec_failure enters plan_end.
    #[test]
    fn pf_14step_report_done_at_exec_failure_enters_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_failure", "forge.report.done");
        assert_eq!(
            next,
            Some("plan_end".to_string()),
            "R4: forge.report.done at exec_failure must advance → plan_end"
        );
    }

    // ── R5/S5: post-exec success chain ─────────────────────────────────────

    /// R5: forge.units.reviewed enters integration.
    #[test]
    fn pf_14step_units_reviewed_enters_integration() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "unit_review", "forge.units.reviewed");
        assert_eq!(
            next,
            Some("integration".to_string()),
            "R5: forge.units.reviewed must advance unit_review → integration"
        );
    }

    /// R5: forge.integration.done enters incremental_verify.
    #[test]
    fn pf_14step_integration_done_enters_incremental_verify() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "integration", "forge.integration.done");
        assert_eq!(
            next,
            Some("incremental_verify".to_string()),
            "R5: forge.integration.done must advance integration → incremental_verify"
        );
    }

    /// R5: forge.incremental.verified enters full_verify.
    #[test]
    fn pf_14step_incremental_verified_enters_full_verify() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "incremental_verify", "forge.incremental.verified");
        assert_eq!(
            next,
            Some("full_verify".to_string()),
            "R5: forge.incremental.verified must advance incremental_verify → full_verify"
        );
    }

    /// R5: forge.full.verified enters audit.
    #[test]
    fn pf_14step_full_verified_enters_audit() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "full_verify", "forge.full.verified");
        assert_eq!(
            next,
            Some("audit".to_string()),
            "R5: forge.full.verified must advance full_verify → audit"
        );
    }

    /// R5: forge.report.done at report enters plan_end.
    #[test]
    fn pf_14step_report_done_at_report_enters_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "report", "forge.report.done");
        assert_eq!(
            next,
            Some("plan_end".to_string()),
            "R5: forge.report.done must advance report → plan_end"
        );
    }

    /// R5: plan_end rejects LOOP_COMPLETE as transition (terminal).
    #[test]
    fn pf_14step_plan_end_loop_complete_is_non_transition() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "plan_end", "LOOP_COMPLETE");
        assert_eq!(next, None, "plan_end is the terminal step");
    }

    // ── R6/S6: failure-capable post-exec steps route to plan_end ────────────

    /// R6 (U7): work.failed at integration is now a transition to
    /// `report` (via `on_any_of`). The legacy non-transition
    /// contract was relaxed for plan-level `work.failed`.
    #[test]
    fn pf_14step_work_failed_at_integration_advances_to_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "integration", "work.failed");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R6 (U7): work.failed at integration must advance → report"
        );
    }

    /// R6: forge.report.done at integration enters plan_end.
    #[test]
    fn pf_14step_report_done_at_integration_enters_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "integration", "forge.report.done");
        assert_eq!(
            next,
            Some("plan_end".to_string()),
            "R6: forge.report.done at integration must advance → plan_end"
        );
    }

    /// R6: forge.report.done at incremental_verify enters plan_end.
    #[test]
    fn pf_14step_report_done_at_incremental_verify_enters_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "incremental_verify", "forge.report.done");
        assert_eq!(
            next,
            Some("plan_end".to_string()),
            "R6: forge.report.done at incremental_verify must advance → plan_end"
        );
    }

    /// R6: forge.report.done at full_verify enters plan_end.
    #[test]
    fn pf_14step_report_done_at_full_verify_enters_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "full_verify", "forge.report.done");
        assert_eq!(
            next,
            Some("plan_end".to_string()),
            "R6: forge.report.done at full_verify must advance → plan_end"
        );
    }

    // ── R7/S7: replay/live equivalence + idempotency ────────────────────────

    /// R7: full happy-path fold reaches plan_end.
    #[test]
    fn pf_14step_recover_full_happy_path_folds_to_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.ready",
                "forge.concurrency.approved",
                "forge.worktrees.ready",
                "exec.wave.complete",
                "forge.exec.development.done",
                "forge.units.reviewed",
                "forge.integration.done",
                "forge.incremental.verified",
                "forge.full.verified",
                "forge.audit.done",
                "forge.report.done",
            ],
        );
        assert_eq!(
            recovered, "plan_end",
            "R7: full happy-path fold must reach plan_end"
        );
    }

    /// R7: replay yields the same step (no retrograde).
    #[test]
    fn pf_14step_recover_replay_is_idempotent() {
        let cfg = parallel_forge_14step_flow();
        let seq = [
            "forge.plan.inspected",
            "forge.plan.ready",
            "forge.concurrency.approved",
            "forge.worktrees.ready",
        ];
        let first = recover_current_plan_step(&cfg, &seq);
        let second = recover_current_plan_step(&cfg, &seq);
        assert_eq!(first, second, "R7: replay must yield the same step");
        assert_eq!(first, "exec_wave");
    }

    /// R7: failed-path fold reaches plan_end via exec_failure.
    #[test]
    fn pf_14step_recover_failed_path_folds_to_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.ready",
                "forge.concurrency.approved",
                "forge.worktrees.ready",
                "exec.wave.failed",
                "forge.report.done",
            ],
        );
        assert_eq!(
            recovered, "plan_end",
            "R7: failed-path fold must reach plan_end via exec_failure"
        );
    }

    /// R7: blocked-path fold reaches plan_end via report.
    #[test]
    fn pf_14step_recover_blocked_path_folds_to_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.blocked",
                "forge.report.done",
            ],
        );
        assert_eq!(
            recovered, "plan_end",
            "R7: blocked-path fold must reach plan_end via report"
        );
    }

    /// R7: forge.plan.blocked at exec_wave is not in allowed_emits → stays.
    #[test]
    fn pf_14step_plan_blocked_at_exec_wave_not_in_allowed_emits() {
        let cfg = parallel_forge_14step_flow();
        // exec_wave.allowed_emits does NOT include forge.plan.blocked.
        let next = advance_plan_step(&cfg, "exec_wave", "forge.plan.blocked");
        assert_eq!(
            next, None,
            "R7: forge.plan.blocked at exec_wave must not trigger a transition"
        );
    }

    /// R7: initial step is planning.
    #[test]
    fn pf_14step_initial_step_is_planning() {
        let cfg = parallel_forge_14step_flow();
        assert_eq!(
            initial_current_plan_step(&cfg),
            "planning",
            "R7: initial step must be planning"
        );
    }

    /// R9: old/duplicate forge.concurrency.approved after exec_wave stays put.
    #[test]
    fn pf_14step_old_handoff_after_exec_wave_no_backstep() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.ready",
                "forge.concurrency.approved",
                "forge.worktrees.ready",
                "forge.plan.ready", // old handoff, must not backstep
            ],
        );
        assert_eq!(
            recovered, "exec_wave",
            "R9: old forge.plan.ready after exec_wave must not backstep"
        );
    }

    /// R9: repeated forge.plan.inspected at plan_authoring stays put.
    #[test]
    fn pf_14step_repeated_inspected_no_backstep() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.inspected", // duplicate
            ],
        );
        assert_eq!(
            recovered, "plan_authoring",
            "R9: repeated forge.plan.inspected must not backstep"
        );
    }

    /// R9: old forge.plan.inspected after exec_wave stays put.
    #[test]
    fn pf_14step_old_inspected_after_exec_wave_no_backstep() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.ready",
                "forge.concurrency.approved",
                "forge.worktrees.ready",
                "forge.plan.inspected", // old handoff
            ],
        );
        assert_eq!(
            recovered, "exec_wave",
            "R9: old forge.plan.inspected after exec_wave must not backstep"
        );
    }
