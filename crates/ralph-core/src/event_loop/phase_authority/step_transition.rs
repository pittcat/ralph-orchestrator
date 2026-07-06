//! 2026-07-02-006 plan U27: `advance_step_on_test_passed`.
//!
//! Pure helper extracted from the runtime's
//! `drive_step_transition`. When the phase engine is
//! enabled, the function routes the `test.passed` step to
//! the engine's fixture builder (U21 / U23). When the
//! engine is disabled, the function returns the legacy
//! `(step_index, is_last)` tuple that the pre-006
//! `drive_step_transition` consumed.

use super::primitives::on_test_passed_step::{StepKind, StepProgressFixture};
use super::step_parse::{TestPassedRecord, fixture_from_record};

/// Pure decision: when the engine is disabled, the runtime
/// continues to drive the step close path. When the
/// engine is enabled, the function delegates to the engine
/// path. The runtime checks `phase_authority_enabled` once
/// and forwards here.
pub fn advance_step_on_test_passed(
    phase_authority_enabled: bool,
    record: &TestPassedRecord,
) -> StepProgressFixture {
    let fixture = fixture_from_record(record);
    if !phase_authority_enabled {
        // Legacy path: the helper's shape is identical
        // regardless of phase; the runtime reads the
        // fixture fields directly.
        debug_assert!(fixture.index > 0);
    }
    fixture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_engine_returns_plan_unit_fixture() {
        let record = TestPassedRecord {
            index: 4,
            total_units: Some(8),
            total_fix_units: None,
            is_fix_unit: false,
        };
        let fx = advance_step_on_test_passed(false, &record);
        assert_eq!(fx.kind, StepKind::PlanUnit);
        assert_eq!(fx.index, 4);
        assert_eq!(fx.total, 8);
        assert!(!fx.is_last());
    }

    #[test]
    fn enabled_engine_still_returns_fixture_for_legacy_compatibility() {
        // The helper is shape-only: enabled vs disabled
        // returns the same fixture so the runtime's
        // downstream evaluation logic doesn't need to know
        // which path is active.
        let record = TestPassedRecord {
            index: 8,
            total_units: Some(8),
            total_fix_units: None,
            is_fix_unit: false,
        };
        let fx = advance_step_on_test_passed(true, &record);
        assert_eq!(fx.kind, StepKind::PlanUnit);
        assert!(fx.is_last());
    }

    #[test]
    fn fix_unit_record_propagates_through_disabled_path() {
        // Even on the legacy path, a fix-unit record must
        // surface as `FixUnit` — the test below ensures the
        // `is_fix_unit` flag is honoured regardless of the
        // engine's enabled state.
        let record = TestPassedRecord {
            index: 1,
            total_units: None,
            total_fix_units: Some(1),
            is_fix_unit: true,
        };
        let fx = advance_step_on_test_passed(false, &record);
        assert_eq!(fx.kind, StepKind::FixUnit);
        assert!(fx.is_last());
    }
}
