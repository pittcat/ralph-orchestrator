//! 2026-07-02-006 plan U6–U9: transition primitives.
//!
//! Each primitive is a pure function over an accepted event and
//! the transition's `on:` payload. New primitives land here
//! and add a matching entry to
//! `preset_lint::phase_authority::KNOWN_PRIMITIVES`.

pub mod on_event;
// U7: on_test_passed_step — handles `test.passed` topic.
pub mod on_test_passed_step;