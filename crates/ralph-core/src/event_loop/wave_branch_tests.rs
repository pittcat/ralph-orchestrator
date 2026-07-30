// 2026-07-28-001 plan U2: typed branch tests for exec_wave state machine.
// Covers:
//   R3/S3 (unit topics non-transition): exec.unit.{ready,done,failed}
//     do not advance exec_wave step
//   R4/S4 (wave terminal transition): exec.wave.complete / exec.wave.failed
//     advance to exec_finalize / exec_failure
//
// Parse the actual preset so this test cannot drift into a second flow model.

// Plan 2026-07-29-001 U7: the parallel-forge flow now uses
// `development_loop` (kind: loop) instead of the legacy single-shot
// `exec_wave` / `exec_finalize` / `exec_failure` triple. The
// previous R3/R4/S3/S4 invariants still hold — per-unit emits stay
// non-transition within the loop, and `exec.wave.complete` /
// `exec.wave.failed` are no longer transition topics either (they
// live inside the loop's allowed_emits but exit only via
// `transition_emits`). The tests below name the new step id.

#[cfg(test)]
mod tests {
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
    // R3 / S3: per-unit topics are NON-transitions within development_loop
    // -------------------------------------------------------------------------

    /// R3 (U7): exec.unit.done does not advance development_loop.
    #[test]
    fn pf_wave_r3_exec_unit_done_is_non_transition() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(
            at_loop, "development_loop",
            "pre-condition: must be at development_loop"
        );
        let next = advance_plan_step(&cfg, &at_loop, "exec.unit.done");
        assert_eq!(
            next, None,
            "R3: exec.unit.done must not advance development_loop step"
        );
    }

    /// R3 (U7): exec.unit.failed does not advance development_loop.
    #[test]
    fn pf_wave_r3_exec_unit_failed_is_non_transition() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(
            at_loop, "development_loop",
            "pre-condition: must be at development_loop"
        );
        let next = advance_plan_step(&cfg, &at_loop, "exec.unit.failed");
        assert_eq!(
            next, None,
            "R3: exec.unit.failed must not advance development_loop step"
        );
    }

    /// S3 (U7): exec.unit.ready does not advance development_loop.
    #[test]
    fn pf_wave_s3_exec_unit_ready_is_non_transition() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(
            at_loop, "development_loop",
            "pre-condition: must be at development_loop"
        );
        let next = advance_plan_step(&cfg, &at_loop, "exec.unit.ready");
        assert_eq!(
            next, None,
            "S3: exec.unit.ready must not advance development_loop step"
        );
    }

    /// S3: recover_current_plan_step fold with exec.unit.done stays at exec_wave.
    /// S3 (U7): recover_current_plan_step fold with exec.unit.done stays at development_loop.
    #[test]
    fn pf_wave_s3_recover_fold_exec_unit_done_stays() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(
            at_loop, "development_loop",
            "pre-condition: must be at development_loop"
        );
        let recovered = recover_from_exec(&cfg, &["exec.unit.done"]);
        assert_eq!(
            recovered, "development_loop",
            "S3 (U7): recover_current_plan_step fold with exec.unit.done stays at development_loop"
        );
    }

    /// S3 (U7): recover_current_plan_step fold with exec.unit.failed stays at development_loop.
    #[test]
    fn pf_wave_s3_recover_fold_exec_unit_failed_stays() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(
            at_loop, "development_loop",
            "pre-condition: must be at development_loop"
        );
        let recovered = recover_from_exec(&cfg, &["exec.unit.failed"]);
        assert_eq!(
            recovered, "development_loop",
            "S3 (U7): recover_current_plan_step fold with exec.unit.failed stays at development_loop"
        );
    }

    // -------------------------------------------------------------------------
    // R4 / S4 (U7): exec.wave.complete / exec.wave.failed stay inside
    // development_loop; only forge.exec.development.done / work.failed
    // exit the loop.
    // -------------------------------------------------------------------------

    /// R4 (U7): exec.wave.complete is in `allowed_emits` but NOT in
    /// `transition_emits` — it stays inside development_loop.
    #[test]
    fn pf_wave_r4_exec_wave_complete_stays_in_development_loop() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(at_loop, "development_loop");
        let next = advance_plan_step(&cfg, &at_loop, "exec.wave.complete");
        assert_eq!(
            next, None,
            "R4 (U7): exec.wave.complete must not advance development_loop"
        );
    }

    /// R4 (U7): exec.wave.failed stays inside development_loop.
    #[test]
    fn pf_wave_r4_exec_wave_failed_stays_in_development_loop() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(at_loop, "development_loop");
        let next = advance_plan_step(&cfg, &at_loop, "exec.wave.failed");
        assert_eq!(
            next, None,
            "R4 (U7): exec.wave.failed must not advance development_loop"
        );
    }

    /// S4 (U7): recover_current_plan_step fold exec.wave.complete stays put.
    #[test]
    fn pf_wave_s4_recover_fold_exec_wave_complete_stays_in_development_loop() {
        let cfg = parallel_forge_flow();
        let recovered = recover_from_exec(&cfg, &["exec.wave.complete"]);
        assert_eq!(
            recovered, "development_loop",
            "S4 (U7): exec.wave.complete alone stays inside development_loop"
        );
    }

    /// S4 (U7): recover_current_plan_step fold exec.wave.failed stays put.
    #[test]
    fn pf_wave_s4_recover_fold_exec_wave_failed_stays_in_development_loop() {
        let cfg = parallel_forge_flow();
        let recovered = recover_from_exec(&cfg, &["exec.wave.failed"]);
        assert_eq!(
            recovered, "development_loop",
            "S4 (U7): exec.wave.failed alone stays inside development_loop"
        );
    }

    // -------------------------------------------------------------------------
    // R5 / R7 (U7): forge.exec.development.done / work.failed are the
    // development_loop's only transition-emits.
    // -------------------------------------------------------------------------

    /// R5 (U7): forge.exec.development.done exits the loop into full_verify.
    #[test]
    fn pf_wave_r5_loop_exit_via_development_done() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(at_loop, "development_loop");
        let next = advance_plan_step(&cfg, &at_loop, "forge.exec.development.done");
        assert_eq!(
            next,
            Some("full_verify".to_string()),
            "R5: forge.exec.development.done must advance development_loop → full_verify"
        );
    }

    /// R7 (U7): work.failed exits the loop into report.
    #[test]
    fn pf_wave_r7_loop_exit_via_work_failed() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(at_loop, "development_loop");
        let next = advance_plan_step(&cfg, &at_loop, "work.failed");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R7: work.failed must advance development_loop → report"
        );
    }

    // -------------------------------------------------------------------------
    // Idempotency: repeated wave terminals do not double-advance
    // -------------------------------------------------------------------------

    /// Idempotency: repeated exec.wave.complete at development_loop stays put.
    #[test]
    fn pf_wave_repeated_exec_wave_complete_stays_in_development_loop() {
        let cfg = parallel_forge_flow();
        let at_review = recover_from_exec(&cfg, &["exec.wave.complete"]);
        assert_eq!(
            at_review, "development_loop",
            "pre-condition: must be at development_loop"
        );
        let recovered = recover_from_exec(&cfg, &["exec.wave.complete", "exec.wave.complete"]);
        assert_eq!(
            recovered, "development_loop",
            "repeated exec.wave.complete must not backstep from development_loop"
        );
    }

    /// Idempotency: repeated exec.wave.failed at development_loop stays put.
    #[test]
    fn pf_wave_repeated_exec_wave_failed_stays_in_development_loop() {
        let cfg = parallel_forge_flow();
        let at_review = recover_from_exec(&cfg, &["exec.wave.failed"]);
        assert_eq!(
            at_review, "development_loop",
            "pre-condition: must be at development_loop"
        );
        let recovered = recover_from_exec(&cfg, &["exec.wave.failed", "exec.wave.failed"]);
        assert_eq!(
            recovered, "development_loop",
            "repeated exec.wave.failed must not backstep from development_loop"
        );
    }

    // -------------------------------------------------------------------------
    // Cross-check: NON_TRANSITION_TOPICS constant covers all per-unit exec topics
    // -------------------------------------------------------------------------

    /// Verify all per-unit exec topics are non-transitions (defence-in-depth).
    #[test]
    fn pf_wave_all_exec_unit_topics_are_non_transitions() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(at_loop, "development_loop");
        for topic in &["exec.unit.ready", "exec.unit.done", "exec.unit.failed"] {
            let next = advance_plan_step(&cfg, at_loop.as_str(), topic);
            assert_eq!(
                next, None,
                "{topic} must be a non-transition within development_loop (NON_TRANSITION_TOPICS)"
            );
        }
    }

    /// Cross-check: in the new flow, exec.wave.* stays inside the
    /// loop because they're allowed_emits but not transition_emits.
    /// `forge.exec.development.done` / `work.failed` are the only
    /// topics that exit the loop.
    #[test]
    fn pf_wave_all_loop_exit_topics_are_transitions() {
        let cfg = parallel_forge_flow();
        let at_loop = recover_from_exec(&cfg, &[]);
        assert_eq!(at_loop, "development_loop");
        for topic in &["exec.wave.complete", "exec.wave.failed"] {
            let next = advance_plan_step(&cfg, at_loop.as_str(), topic);
            assert_eq!(
                next, None,
                "{topic} must NOT exit development_loop (only transition_emits advance)"
            );
        }
        // Sanity: the declared transition_emits do exit the loop.
        let dev = advance_plan_step(&cfg, "development_loop", "forge.exec.development.done");
        assert_eq!(dev, Some("full_verify".to_string()));
        let fail = advance_plan_step(&cfg, "development_loop", "work.failed");
        assert_eq!(fail, Some("report".to_string()));
    }
}
