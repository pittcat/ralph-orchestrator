//! Shared test-support constants and helpers used by both
//! `ralph-core`'s own integration tests and downstream crates
//! (`ralph-cli` lock tests in particular).
//!
//! Plan 2026-07-07-006 fix-plan U7 (R8 / SR-M1): single SSOT for
//! unit-evidence field coverage so the two crates no longer carry
//! `UNIT_EVIDENCE_FIELDS` vs. `PIPELINE_UNIT_EVIDENCE_FIELDS` as
//! divergent constants with the same byte content.

pub mod unit_evidence;
