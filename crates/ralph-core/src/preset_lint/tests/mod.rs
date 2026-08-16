//! Tests for `preset_lint` — split per rule category.
//!
//! The original `preset_lint.rs` test module contained 46 tests covering
//! U1 (topic format), U2 (ownership / coordinator), and U3 (integration).
//! They are split across three sibling test modules:
//!
//! - [`topic_format`] — 26 tests (format validation, whitelist,
//!   suggestion generation, surface enumeration, validate_all_topics,
//!   TopicSurface labels, finding ID constants).
//! - [`ownership`] — 16 tests (R2/R3/R4 ownership rules, R5
//!   coordinator rules, severity mapping, deterministic sorting).
//! - [`run_preset_lint`] — 4 tests (U3 end-to-end integration).
//! - [`finding_id_lock`] — 1 test (Unit 4 finding-id surface lock).

use super::*;

mod finding_id_lock;
mod ownership;
mod precheck_gate_hat;
mod run_preset_lint;
mod target_routing_tests;
mod topic_format;
