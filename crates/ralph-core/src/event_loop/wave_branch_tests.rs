// 2026-07-28-001 plan U2: typed branch tests for exec_wave state machine.
// Covers:
//   R3/S3 (unit topics non-transition): exec.unit.{ready,done,failed}
//     do not advance exec_wave step
//   R4/S4 (wave terminal transition): exec.wave.complete / exec.wave.failed
//     advance to exec_finalize / exec_failure
//
// Parse the actual preset so this test cannot drift into a second flow model.

#[cfg(test)]
mod wave_branch_tests {
    use crate::config::RalphConfig;
    use crate::event_loop::advance_plan_step;
    use crate::event_loop::recover_current_plan_step;

    fn parallel_forge_flow() -> RalphConfig {
        RalphConfig::parse_yaml(include_str!("../../../../presets/en/parallel-forge.yml"))
            .expect("parallel-forge preset must parse")
    }

    fn recover_from_exec(cfg: &RalphConfig, subsequent_topics: &[&str]) -> String {
        let mut topics = vec![
            "forge.plan.inspected",
            "forge.plan.ready",
            "forge.concurrency.approved",
            "forge.worktrees.ready",
        ];
        topics.extend_from_slice(subsequent_topics);
        recover_current_plan_step(cfg, &topics)
    }

    // -------------------------------------------------------------------------
    // R3 / S3: per-unit topics are NON-transitions within exec_wave
    // -------------------------------------------------------------------------

    /// R3: exec.unit.done does not advance exec_wave.
    #[test]
    fn pf_wave_r3_exec_unit_done_is_non_transition() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_from_exec(&cfg, &[]);
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
        let at_exec = recover_from_exec(&cfg, &[]);
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
        let at_exec = recover_from_exec(&cfg, &[]);
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
        let at_exec = recover_from_exec(&cfg, &[]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let recovered = recover_from_exec(&cfg, &["exec.unit.done"]);
        assert_eq!(
            recovered, "exec_wave",
            "S3: recover_current_plan_step fold with exec.unit.done stays at exec_wave"
        );
    }

    /// S3: recover_current_plan_step fold with exec.unit.failed stays at exec_wave.
    #[test]
    fn pf_wave_s3_recover_fold_exec_unit_failed_stays() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_from_exec(&cfg, &[]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let recovered = recover_from_exec(&cfg, &["exec.unit.failed"]);
        assert_eq!(
            recovered, "exec_wave",
            "S3: recover_current_plan_step fold with exec.unit.failed stays at exec_wave"
        );
    }

    // -------------------------------------------------------------------------
    // R4 / S4: exec.wave.complete / exec.wave.failed ARE step transitions
    // -------------------------------------------------------------------------

    /// R4: exec.wave.complete advances exec_wave → exec_finalize.
    #[test]
    fn pf_wave_r4_exec_wave_complete_advances_to_exec_finalize() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_from_exec(&cfg, &[]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let next = advance_plan_step(&cfg, &at_exec, "exec.wave.complete");
        assert_eq!(
            next,
            Some("exec_finalize".to_string()),
            "R4: exec.wave.complete must advance exec_wave → exec_finalize"
        );
    }

    /// R4: exec.wave.failed advances exec_wave → exec_failure.
    #[test]
    fn pf_wave_r4_exec_wave_failed_advances_to_exec_failure() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_from_exec(&cfg, &[]);
        assert_eq!(at_exec, "exec_wave", "pre-condition: must be at exec_wave");
        let next = advance_plan_step(&cfg, &at_exec, "exec.wave.failed");
        assert_eq!(
            next,
            Some("exec_failure".to_string()),
            "R4: exec.wave.failed must advance exec_wave → exec_failure"
        );
    }

    /// S4: recover_current_plan_step fold exec.wave.complete → exec_finalize.
    #[test]
    fn pf_wave_s4_recover_fold_exec_wave_complete_to_exec_finalize() {
        let cfg = parallel_forge_flow();
        let recovered = recover_from_exec(&cfg, &["exec.wave.complete"]);
        assert_eq!(
            recovered, "exec_finalize",
            "S4: recover_current_plan_step fold exec.wave.complete → exec_finalize"
        );
    }

    /// S4: recover_current_plan_step fold exec.wave.failed → exec_failure.
    #[test]
    fn pf_wave_s4_recover_fold_exec_wave_failed_to_exec_failure() {
        let cfg = parallel_forge_flow();
        let recovered = recover_from_exec(&cfg, &["exec.wave.failed"]);
        assert_eq!(
            recovered, "exec_failure",
            "S4: recover_current_plan_step fold exec.wave.failed → exec_failure"
        );
    }

    // -------------------------------------------------------------------------
    // Idempotency: repeated wave terminals do not double-advance
    // -------------------------------------------------------------------------

    /// Idempotency: repeated exec.wave.complete at exec_finalize stays put.
    #[test]
    fn pf_wave_repeated_exec_wave_complete_stays_at_exec_finalize() {
        let cfg = parallel_forge_flow();
        let at_review = recover_from_exec(&cfg, &["exec.wave.complete"]);
        assert_eq!(
            at_review, "exec_finalize",
            "pre-condition: must be at exec_finalize"
        );
        let recovered = recover_from_exec(&cfg, &["exec.wave.complete", "exec.wave.complete"]);
        assert_eq!(
            recovered, "exec_finalize",
            "repeated exec.wave.complete must not backstep from exec_finalize"
        );
    }

    /// Idempotency: repeated exec.wave.failed at exec_failure stays put.
    #[test]
    fn pf_wave_repeated_exec_wave_failed_stays_at_exec_failure() {
        let cfg = parallel_forge_flow();
        let at_review = recover_from_exec(&cfg, &["exec.wave.failed"]);
        assert_eq!(
            at_review, "exec_failure",
            "pre-condition: must be at exec_failure"
        );
        let recovered = recover_from_exec(&cfg, &["exec.wave.failed", "exec.wave.failed"]);
        assert_eq!(
            recovered, "exec_failure",
            "repeated exec.wave.failed must not backstep from exec_failure"
        );
    }

    // -------------------------------------------------------------------------
    // Cross-check: NON_TRANSITION_TOPICS constant covers all per-unit exec topics
    // -------------------------------------------------------------------------

    /// Verify all per-unit exec topics are non-transitions (defence-in-depth).
    #[test]
    fn pf_wave_all_exec_unit_topics_are_non_transitions() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_from_exec(&cfg, &[]);
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
        let at_exec = recover_from_exec(&cfg, &[]);
        assert_eq!(at_exec, "exec_wave");
        for (topic, expected) in [
            ("exec.wave.complete", "exec_finalize"),
            ("exec.wave.failed", "exec_failure"),
        ] {
            let next = advance_plan_step(&cfg, at_exec.as_str(), topic);
            assert_eq!(
                next,
                Some(expected.to_string()),
                "{topic} must take its declared transition from exec_wave"
            );
        }
    }
}
