//! 2026-07-02-006 plan U17: progress_gate helper.
//!
//! Pure decision mirroring U16 for the progress gate:
//! `Option<&str>` (workflow_phase.phase_id) → `bool`.
//!
//! Convention:
//! - Disabled engine → `true` (keep pre-006 behaviour).
//! - `unit_loop` / `fix_units` → `true` (gate can skip; the
//!   engine handles step progression).
//! - `plan_end` / `ship` / `terminal` → `false` (gate must
//!   enforce).
//! - `review` is interesting: the gate runs at every emit,
//!   so during the review walk the gate must keep enforcing
//!   the missing-step rule. Return `false`.
//! - Any other / unknown phase → `false` (conservative: the
//!   gate must enforce rather than risk a missing-step leak).

pub fn progress_gate_should_skip_missing_current_step(phase_id: Option<&str>) -> bool {
    match phase_id {
        None => true,
        Some("unit_loop") | Some("fix_units") => true,
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::progress_gate_should_skip_missing_current_step as skip;

    #[test]
    fn disabled_engine_skips() {
        assert!(skip(None));
    }

    #[test]
    fn unit_loop_skips() {
        assert!(skip(Some("unit_loop")));
    }

    #[test]
    fn fix_units_skips() {
        assert!(skip(Some("fix_units")));
    }

    #[test]
    fn plan_end_does_not_skip() {
        assert!(!skip(Some("plan_end")));
    }

    #[test]
    fn review_does_not_skip() {
        assert!(!skip(Some("review")));
    }

    #[test]
    fn ship_does_not_skip() {
        assert!(!skip(Some("ship")));
    }

    #[test]
    fn terminal_does_not_skip() {
        assert!(!skip(Some("terminal")));
    }

    #[test]
    fn unknown_phase_does_not_skip() {
        assert!(!skip(Some("mystery")));
    }
}