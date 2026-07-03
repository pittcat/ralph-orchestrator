//! 2026-07-02-006 plan U16: `plan_gate_should_skip_review_not_terminal`.
//!
//! Pure helper: `Option<&str>` (the `workflow_phase.phase_id`)
//! → `bool`. The plan gate calls this when it is about to
//! reject a `plan.complete` or `plan.blocked` emit as
//! "review not terminal"; the helper says whether the engine
//! has a definitive answer.
//!
//! **Convention (this unit only):**
//! - When `phase_id` is `None` (engine disabled or snapshot not
//!   projected), the gate runs normally — return `false`.
//! - When the engine is enabled and the phase is one of the
//!   pre-flight values (`unit_loop`, `fix_units`), the gate
//!   should SKIP its check — the engine will eventually
//!   drive the loop into `plan_end` via the verdict matrix.
//!   Return `true`.
//! - When the engine is enabled and the phase is `plan_end`,
//!   the gate should NOT skip — the runtime must accept the
//!   emit. Return `false`.
//! - When the engine is enabled and the phase is anything
//!   else (`review`, `ship`, `terminal`, etc.), the gate
//!   keeps its pre-006 behaviour and returns `false`.

/// Pure decision.
pub fn plan_gate_should_skip_review_not_terminal(phase_id: Option<&str>) -> bool {
    match phase_id {
        None => false,
        // Engine says we're not yet at plan_end; gate can
        // safely skip.
        Some("unit_loop") | Some("fix_units") => true,
        // Engine says we're at plan_end or beyond; the gate
        // must evaluate the emit normally.
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::plan_gate_should_skip_review_not_terminal as skip;

    #[test]
    fn disabled_engine_keeps_pre_006_behaviour() {
        assert!(!skip(None));
    }

    #[test]
    fn unit_loop_phase_skips() {
        assert!(skip(Some("unit_loop")));
    }

    #[test]
    fn fix_units_phase_skips() {
        assert!(skip(Some("fix_units")));
    }

    #[test]
    fn plan_end_phase_does_not_skip() {
        assert!(!skip(Some("plan_end")));
    }

    #[test]
    fn review_phase_does_not_skip() {
        // review is a transient phase; the gate must still
        // enforce its rules so a malformed `plan.complete`
        // cannot leak past the review wall.
        assert!(!skip(Some("review")));
    }

    #[test]
    fn ship_phase_does_not_skip() {
        assert!(!skip(Some("ship")));
    }

    #[test]
    fn terminal_phase_does_not_skip() {
        assert!(!skip(Some("terminal")));
    }
}