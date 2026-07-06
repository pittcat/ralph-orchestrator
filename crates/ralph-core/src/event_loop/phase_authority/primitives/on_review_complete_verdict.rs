//! 2026-07-02-006 plan U8: `on_review_complete_verdict` primitive.
//!
//! Pure decision over `review.complete` payloads. The primitive
//! encodes the KTD4 verdict matrix:
//!
//! | verdict               | fix_plan_file | → phase    |
//! |-----------------------|---------------|------------|
//! | pass                  | (any)         | plan_end   |
//! | pass_with_residuals   | absent        | plan_end   |
//! | pass_with_residuals   | present       | fix_units  |
//! | fail                  | present       | fix_units  |
//! | fail                  | absent        | plan_end   |
//!
//! The matrix is parameterised by a `MatrixId` — future presets
//! register a new matrix in `MATRICES` without touching this
//! module. The primitive is preset-name agnostic.

use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::collections::BTreeMap;

/// Verdict values carried by `review.complete` payloads.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Verdict {
    Pass,
    PassWithResiduals,
    Fail,
}

impl Verdict {
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "pass" => Some(Self::Pass),
            "pass_with_residuals" => Some(Self::PassWithResiduals),
            "fail" => Some(Self::Fail),
            _ => None,
        }
    }
}

/// Stable identifier for a verdict matrix. New matrices
/// register here in lockstep with `MATRICES`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MatrixId {
    /// The serial preset's matrix — the canonical 5-row table
    /// above (KTD4).
    SerialDefault,
}

impl MatrixId {
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "serial_default" => Some(Self::SerialDefault),
            _ => None,
        }
    }
}

/// Inputs the primitive needs from the runtime: a parsed
/// verdict and whether a `fix_plan_file` was attached.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewCompleteFixture {
    pub verdict: Verdict,
    /// `true` when the `review.complete` payload carried a
    /// non-empty `fix_plan_file` field.
    pub fix_plan_attached: bool,
}

/// Pure decision: return the target phase id when the trigger
/// matches `review.complete` and the matrix yields a target.
pub fn evaluate(
    trigger: &Value,
    event_topic: &str,
    fixture: &ReviewCompleteFixture,
) -> Option<String> {
    if event_topic != "review.complete" {
        return None;
    }

    let mapping = trigger.as_mapping()?;
    let primitive = mapping
        .get(&Value::String("primitive".to_string()))?
        .as_str()?;
    if primitive != "on_review_complete_verdict" {
        return None;
    }

    let matrix_id = mapping
        .get(&Value::String("matrix".to_string()))
        .and_then(|v| v.as_str())
        .and_then(MatrixId::from_token)?;

    let matrix = MATRICES.get(&matrix_id)?;
    let target = lookup_target(matrix, fixture)?;
    Some(target.to_string())
}

fn lookup_target<'a>(
    matrix: &'a BTreeMap<(Verdict, Option<bool>), String>,
    fixture: &ReviewCompleteFixture,
) -> Option<&'a String> {
    // The matrix key is `(verdict, Option<bool>)`. `None`
    // matches both `fix_plan_attached=true` and
    // `fix_plan_attached=false`; `Some(true)` and `Some(false)`
    // match exactly.
    let keys = [
        (fixture.verdict, Some(fixture.fix_plan_attached)),
        (fixture.verdict, None),
    ];
    for key in keys {
        if let Some(target) = matrix.get(&key) {
            return Some(target);
        }
    }
    None
}

/// KTD4 verdict matrix for `serial_default`. New presets add
/// new variants here in lockstep with `MatrixId`.
pub static MATRICES: std::sync::LazyLock<
    BTreeMap<MatrixId, BTreeMap<(Verdict, Option<bool>), String>>,
> = std::sync::LazyLock::new(|| {
    let mut serial_default: BTreeMap<(Verdict, Option<bool>), String> = BTreeMap::new();
    // pass — fix_plan ignored
    serial_default.insert((Verdict::Pass, None), "plan_end".to_string());
    // pass_with_residuals + no fix → plan_end
    serial_default.insert(
        (Verdict::PassWithResiduals, Some(false)),
        "plan_end".to_string(),
    );
    // pass_with_residuals + fix → fix_units
    serial_default.insert(
        (Verdict::PassWithResiduals, Some(true)),
        "fix_units".to_string(),
    );
    // fail + fix → fix_units
    serial_default.insert((Verdict::Fail, Some(true)), "fix_units".to_string());
    // fail + no fix → plan_end
    serial_default.insert((Verdict::Fail, Some(false)), "plan_end".to_string());

    let mut map: BTreeMap<MatrixId, BTreeMap<(Verdict, Option<bool>), String>> = BTreeMap::new();
    map.insert(MatrixId::SerialDefault, serial_default);
    map
});

#[cfg(test)]
mod tests {
    use super::*;

    fn serial_default_trigger() -> Value {
        serde_yaml::from_str(
            r#"
primitive: on_review_complete_verdict
matrix: serial_default
"#,
        )
        .unwrap()
    }

    #[test]
    fn pass_with_residuals_no_fix_routes_to_plan_end() {
        let fx = ReviewCompleteFixture {
            verdict: Verdict::PassWithResiduals,
            fix_plan_attached: false,
        };
        assert_eq!(
            evaluate(&serial_default_trigger(), "review.complete", &fx),
            Some("plan_end".to_string())
        );
    }

    #[test]
    fn pass_with_residuals_with_fix_routes_to_fix_units() {
        let fx = ReviewCompleteFixture {
            verdict: Verdict::PassWithResiduals,
            fix_plan_attached: true,
        };
        assert_eq!(
            evaluate(&serial_default_trigger(), "review.complete", &fx),
            Some("fix_units".to_string())
        );
    }

    #[test]
    fn fail_with_fix_routes_to_fix_units() {
        let fx = ReviewCompleteFixture {
            verdict: Verdict::Fail,
            fix_plan_attached: true,
        };
        assert_eq!(
            evaluate(&serial_default_trigger(), "review.complete", &fx),
            Some("fix_units".to_string())
        );
    }

    #[test]
    fn fail_without_fix_routes_to_plan_end() {
        let fx = ReviewCompleteFixture {
            verdict: Verdict::Fail,
            fix_plan_attached: false,
        };
        assert_eq!(
            evaluate(&serial_default_trigger(), "review.complete", &fx),
            Some("plan_end".to_string())
        );
    }

    #[test]
    fn pass_ignores_fix_plan() {
        // pass always routes to plan_end regardless of fix_plan.
        let fx = ReviewCompleteFixture {
            verdict: Verdict::Pass,
            fix_plan_attached: true,
        };
        assert_eq!(
            evaluate(&serial_default_trigger(), "review.complete", &fx),
            Some("plan_end".to_string())
        );
    }

    #[test]
    fn wrong_topic_does_not_match() {
        let fx = ReviewCompleteFixture {
            verdict: Verdict::Fail,
            fix_plan_attached: true,
        };
        assert_eq!(
            evaluate(&serial_default_trigger(), "test.passed", &fx),
            None
        );
    }

    #[test]
    fn unknown_matrix_id_does_not_match() {
        let trigger: Value = serde_yaml::from_str(
            r#"
primitive: on_review_complete_verdict
matrix: future_unknown
"#,
        )
        .unwrap();
        let fx = ReviewCompleteFixture {
            verdict: Verdict::Fail,
            fix_plan_attached: true,
        };
        assert_eq!(evaluate(&trigger, "review.complete", &fx), None);
    }
}
