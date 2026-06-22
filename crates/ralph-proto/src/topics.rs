//! Stable event topic constants.
//!
//! Topics live as free-form strings throughout the runtime
//! (`Event::new("task.resume", ...)`), but the orchestrator also
//! needs to recognise them as `loop.resume`, `task.resume`, and
//! other well-known control topics.  Centralising the
//! well-known values here keeps the routing layer and the
//! drift / recovery layers in sync — adding a new control
//! topic becomes a one-line change here plus the matching
//! `is_orchestrator_control_topic` allowlist.
//!
//! Plan ref: U7b (2026-06-21-002) introduces
//! [`LOOP_RESUME`] as the deterministic replacement for the
//! legacy `task.resume` boot event on `--continue`.  The
//! constant is here so U9 can route the new topic through the
//! same allowlist as `task.resume`.

/// Boot topic used by `--continue` (U7b).
///
/// Replaces the legacy `task.resume` start event the loop used
/// to publish on resume mode.  Carries a [`crate::correction::ResumeContext`]
/// block in the next prompt instead of being consumed by a
/// hat.
pub const LOOP_RESUME: &str = "loop.resume";

/// Legacy resume boot topic.  Still in use when the
/// `UNIFIED_DETERMINISTIC_CORRECTION` env var is unset
/// (default).  The constant is preserved so tests that pin the
/// topic string (e.g. `event_policy.rs::task.resume` allowlist)
/// keep passing without touching the literal.
pub const TASK_RESUME: &str = "task.resume";

/// Orchestrator control topic used to terminate the loop.
pub const LOOP_COMPLETE: &str = "loop.complete";

/// Orchestrator control topic used to cancel the loop.
pub const LOOP_CANCEL: &str = "loop.cancel";

/// Orchestrator control topic used to inject a human-facing
/// guidance message.
pub const HUMAN_GUIDANCE: &str = "human.guidance";

/// Orchestrator diagnostic topic published whenever a boundary
/// (origin / scope / pseudo-hat) gate fires.
pub const EVENT_ISOLATION_BOUNDARY_VIOLATION: &str = "event.isolation.boundary_violation";

/// Return `true` when `topic` is one of the well-known
/// orchestrator control topics.  Mirrors the U2/U4b allowlist
/// in `is_orchestrator_control_topic` so callers that don't
/// have access to the orchestrator config can still test the
/// topic name cheaply.
pub fn is_orchestrator_control(topic: &str) -> bool {
    matches!(
        topic,
        LOOP_RESUME | TASK_RESUME | LOOP_COMPLETE | LOOP_CANCEL | HUMAN_GUIDANCE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_resume_constant_matches_canonical_string() {
        assert_eq!(LOOP_RESUME, "loop.resume");
    }

    #[test]
    fn task_resume_constant_matches_canonical_string() {
        assert_eq!(TASK_RESUME, "task.resume");
    }

    #[test]
    fn is_orchestrator_control_recognises_known_topics() {
        assert!(is_orchestrator_control(LOOP_RESUME));
        assert!(is_orchestrator_control(TASK_RESUME));
        assert!(is_orchestrator_control(LOOP_COMPLETE));
        assert!(is_orchestrator_control(LOOP_CANCEL));
        assert!(is_orchestrator_control(HUMAN_GUIDANCE));
        assert!(!is_orchestrator_control("work.done"));
        assert!(!is_orchestrator_control("loop.suspend"));
    }
}
