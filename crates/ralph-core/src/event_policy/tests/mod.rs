//! Test cases for event_policy (Plan 2026-08-07-002 §7 U4).
//! 103 tests split between tests_part1.rs + tests_part2.rs,
//! inlined via include! to preserve baseline test IDs
//! `event_policy::tests::<fn>` (matching e49c018a namespace).
pub mod helpers;

#[cfg(test)]
include!("tests_part1.rs");

#[cfg(test)]
include!("tests_part2.rs");
