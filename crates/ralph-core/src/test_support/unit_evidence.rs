//! Shared test-support constants for unit-evidence field coverage.
//!
//! Plan 2026-07-07-006 fix-plan U7 (R8 / SR-M1): `UNIT_EVIDENCE_FIELDS`
//! was duplicated as `UNIT_EVIDENCE_FIELDS` (in
//! `crates/ralph-cli/src/presets.rs`) and `PIPELINE_UNIT_EVIDENCE_FIELDS`
//! (in `crates/ralph-core/tests/scenarios.rs`). The two were
//! byte-equivalent but named differently, so a future edit to one
//! side would silently diverge from the other. This module is the
//! single source of truth: any test (or downstream consumer) that
//! needs to assert "this preset's `work.done` covers the unit-
//! evidence fields the executor mode promises" should reference the
//! constant exported here.
//!
//! Adding or removing a field here is a contract change: update the
//! pipeline preset's `event_loop.event_policy.schemas.work.done.required_fields`
//! and the corresponding lock tests in lockstep.

/// Unit-evidence fields the executor mode promises in `work.done`.
/// Pipeline's `work.done.required_fields` must be a superset of
/// these so the single-chain (synthesizer → fix-planner → fixer →
/// alignment → reporter) can consume the evidence without schema
/// extensions.
pub const UNIT_EVIDENCE_FIELDS: &[&str] = &[
    "executor_head_sha",
    "tests_run",
    "tests_passed",
    "commit_count",
    "changed_lines",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// SSOT content lock: the constant must carry exactly the five
    /// unit-evidence fields the executor mode contract promises. If
    /// a future refactor drops a field, the pipeline lock test
    /// (`test_pipeline_work_done_required_fields_covers_unit_evidence`)
    /// starts failing for the wrong reason — this guard prevents
    /// the helper itself from drifting away from the contract.
    #[test]
    fn test_unit_evidence_fields_constant_includes_required_keys() {
        let expected = [
            "executor_head_sha",
            "tests_run",
            "tests_passed",
            "commit_count",
            "changed_lines",
        ];
        let actual: Vec<&str> = UNIT_EVIDENCE_FIELDS.to_vec();
        assert_eq!(
            actual, expected,
            "UNIT_EVIDENCE_FIELDS SSOT drift — the constant must carry exactly \
             the five unit-evidence fields the executor mode promises"
        );
    }
}
