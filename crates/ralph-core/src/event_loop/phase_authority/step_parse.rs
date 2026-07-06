//! 2026-07-02-006 plan U21: step parsing for `test.passed`.
//!
//! Pure parsers that turn a `test.passed` payload into a
//! `StepProgressFixture` so U7's primitive can evaluate the
//! transition rule without touching disk or the broader
//! state. The runtime's `drive_step_transition` calls these
//! once per accepted `test.passed` and feeds the result to
//! `WorkflowPhaseAuthority::on_event_accepted`.

use super::primitives::on_test_passed_step::{StepKind, StepProgressFixture};
use serde::Deserialize;

/// Parsed step record. Mirrors the fields the runtime
/// projects into the `test.passed` payload via
/// `ralph emit` (the event is JSON-encoded; we deserialize
/// the same shape here).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TestPassedRecord {
    /// 1-based step index. `total_units` mirrors the
    /// plan's per-unit count; `total_fix_units` mirrors the
    /// fix batch.
    #[serde(default = "default_index")]
    pub index: u32,
    /// Plan's total unit count. `None` when the field is
    /// absent (legacy payloads).
    #[serde(default)]
    pub total_units: Option<u32>,
    /// Fix batch total. `None` when the emit is for a
    /// plan unit; `Some(n)` when the emit is for a fix unit.
    #[serde(default)]
    pub total_fix_units: Option<u32>,
    /// When `true` the runtime classifies the step as a
    /// fix unit. When `false` or absent the step is a
    /// plan unit.
    #[serde(default)]
    pub is_fix_unit: bool,
}

fn default_index() -> u32 {
    1
}

/// Build a `StepProgressFixture` from a parsed record. Pure.
pub fn fixture_from_record(record: &TestPassedRecord) -> StepProgressFixture {
    let kind = if record.is_fix_unit {
        StepKind::FixUnit
    } else {
        StepKind::PlanUnit
    };
    let total = if record.is_fix_unit {
        record
            .total_fix_units
            .unwrap_or(record.total_units.unwrap_or(0))
    } else {
        record.total_units.unwrap_or(0)
    };
    StepProgressFixture {
        kind,
        index: record.index,
        total,
    }
}

/// Parse a `test.passed` payload (`serde_json::Value`
/// representation, since the runtime parses it once at
/// event-read time) and return a `StepProgressFixture`.
///
/// Returns `None` when the payload is missing required
/// fields (`index`); the runtime falls back to the legacy
/// step-close path in that case.
pub fn parse_test_passed_step(payload: &serde_json::Value) -> Option<StepProgressFixture> {
    let record: TestPassedRecord = serde_json::from_value(payload.clone()).ok()?;
    Some(fixture_from_record(&record))
}

/// Pure decision: does this `test.passed` record represent
/// the LAST step in its batch? The fixture's `is_last()`
/// method does the actual comparison; this convenience
/// helper keeps the call-site terse.
pub fn is_fix_unit_completion(record: &TestPassedRecord) -> bool {
    record.is_fix_unit
        && record.total_fix_units.unwrap_or(0) > 0
        && record.index >= record.total_fix_units.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_plan_unit_step() {
        let payload = json!({
            "index": 3,
            "total_units": 8,
        });
        let fx = parse_test_passed_step(&payload).expect("fixture");
        assert_eq!(fx.kind, StepKind::PlanUnit);
        assert_eq!(fx.index, 3);
        assert_eq!(fx.total, 8);
        assert!(!fx.is_last());
    }

    #[test]
    fn parses_last_plan_unit_step() {
        let payload = json!({
            "index": 8,
            "total_units": 8,
        });
        let fx = parse_test_passed_step(&payload).expect("fixture");
        assert_eq!(fx.kind, StepKind::PlanUnit);
        assert!(fx.is_last());
    }

    #[test]
    fn parses_fix_unit_step() {
        let payload = json!({
            "index": 1,
            "total_fix_units": 2,
            "is_fix_unit": true,
        });
        let fx = parse_test_passed_step(&payload).expect("fixture");
        assert_eq!(fx.kind, StepKind::FixUnit);
        assert_eq!(fx.index, 1);
        assert_eq!(fx.total, 2);
        assert!(!fx.is_last());
    }

    #[test]
    fn parses_last_fix_unit_step() {
        let payload = json!({
            "index": 2,
            "total_fix_units": 2,
            "is_fix_unit": true,
        });
        let fx = parse_test_passed_step(&payload).expect("fixture");
        assert_eq!(fx.kind, StepKind::FixUnit);
        assert!(fx.is_last());
    }

    #[test]
    fn malformed_payload_returns_none() {
        let payload = json!("not an object");
        assert!(parse_test_passed_step(&payload).is_none());
    }

    #[test]
    fn is_fix_unit_completion_true_for_last_fix_step() {
        let record = TestPassedRecord {
            index: 3,
            total_units: None,
            total_fix_units: Some(3),
            is_fix_unit: true,
        };
        assert!(is_fix_unit_completion(&record));
    }

    #[test]
    fn is_fix_unit_completion_false_for_plan_unit() {
        let record = TestPassedRecord {
            index: 8,
            total_units: Some(8),
            total_fix_units: None,
            is_fix_unit: false,
        };
        assert!(!is_fix_unit_completion(&record));
    }

    #[test]
    fn is_fix_unit_completion_false_for_non_last_fix_step() {
        let record = TestPassedRecord {
            index: 1,
            total_units: None,
            total_fix_units: Some(3),
            is_fix_unit: true,
        };
        assert!(!is_fix_unit_completion(&record));
    }
}
