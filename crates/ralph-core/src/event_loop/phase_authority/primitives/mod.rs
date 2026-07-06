//! 2026-07-02-006 plan U6–U9: transition primitives.
//!
//! Each primitive is a pure function over an accepted event and
//! the transition's `on:` payload. New primitives land here
//! and add a matching entry to
//! `preset_lint::phase_authority::KNOWN_PRIMITIVES`.

pub mod on_event;
// Plan terminal acceptance (`plan.complete` / `plan.blocked`).
pub mod on_plan_terminal_accepted;
// U7: on_test_passed_step — handles `test.passed` topic.
pub mod on_test_passed_step;
// U8: on_review_complete_verdict — KTD4 matrix (parameterised
// by MatrixId, never by preset name).
pub mod on_review_complete_verdict;
// U9: on_loop_complete_honored — honored LOOP_COMPLETE → terminal.
pub mod on_loop_complete_honored;
