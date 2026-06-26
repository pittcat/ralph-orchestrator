// 2026-06-26 plan U1: typed `Verdict` SSOT for terminal state semantics.
//
// Three-state model — the OLD code did binary `verdict == "fail"` matching
// which caused `pass_with_residuals` to be mis-classified by shipper,
// reporter and verdict_gate independently (see
// docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md).
//
// This module is the single source of truth: the same `Verdict::resolve()`
// function is called from Rust code paths (`verdict_payload_is_fail`,
// `check_completion_event`) and is described in the shipper/reporter
// prompts (`presets/en/ce-executor-serial.yml`) so the three layers share
// one definition of `Pass` / `PassWithResiduals` / `Fail`.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Terminal state for a review pass.
///
/// `PassWithResiduals` is structurally distinct from `Pass`:
/// the review found findings but the operator / preset has decided
/// they are within tolerance. The `count` carries the number of
/// residual findings so downstream consumers (reporter) can show
/// the figure without re-parsing the payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum Verdict {
    /// Verdict: pass (no findings, or residuals suppressed).
    Pass,
    /// Verdict: pass with residual findings within tolerance.
    /// `count` is the number of residual findings the review
    /// recorded (P0+P1 typically; the preset decides what counts).
    PassWithResiduals {
        /// Number of residual findings the review recorded.
        /// Required field — emit it on every `pass_with_residuals`
        /// event so consumers do not have to re-parse nested arrays.
        count: u32,
    },
    /// Verdict: fail. `reason` is a free-form human-readable
    /// explanation that the shipper / reporter forwards to the
    /// terminal `report.done` payload.
    Fail {
        /// Free-form reason string.
        reason: String,
    },
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Verdict::Pass => f.write_str("pass"),
            Verdict::PassWithResiduals { .. } => f.write_str("pass_with_residuals"),
            Verdict::Fail { .. } => f.write_str("fail"),
        }
    }
}

impl Verdict {
    /// Parse a verdict payload (JSON object) into a typed `Verdict`.
    ///
    /// The legacy gate used `gate.fail_field` (default `"pass_or_fail"`)
    /// to decide pass/fail. The new typed parser still supports that
    /// field name as the default `verdict_field` — the new name is
    /// `verdict` and the old name is read as a fallback so existing
    /// presets keep working without a config migration.
    ///
    /// - `verdict_field` overrides which JSON field names the parser
    ///   looks for (default: `"verdict"`, fallback: `"pass_or_fail"`).
    /// - `count_field` overrides the residual-count field name
    ///   (default: `"final_findings_count"`, fallback:
    ///   `"residuals_count"`).
    /// - `reason_field` overrides the fail-reason field name
    ///   (default: `"reason"`, fallback: `"fail_reason"`).
    pub fn from_payload(
        payload: &str,
        verdict_field: &str,
        count_field: Option<&str>,
    ) -> Result<Self, VerdictParseError> {
        let value: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| VerdictParseError::MalformedJson(e.to_string()))?;

        let count_keys: &[&str] = match count_field {
            Some(c) => &[c, "final_findings_count", "residuals_count"][..],
            None => &["final_findings_count", "residuals_count"][..],
        };

        // Look up the verdict string. Try the configured field first,
        // then the legacy `pass_or_fail` field as a fallback so the
        // typed parser is backwards-compatible with existing gates
        // that still write the old key.
        let verdict_str = value
            .get(verdict_field)
            .and_then(|v| v.as_str())
            .or_else(|| value.get("pass_or_fail").and_then(|v| v.as_str()))
            .ok_or_else(|| VerdictParseError::MissingField(verdict_field.to_string()))?;

        Self::from_str_with_payload(verdict_str, &value, count_keys)
    }

    fn from_str_with_payload(
        verdict_str: &str,
        value: &serde_json::Value,
        count_keys: &[&str],
    ) -> Result<Self, VerdictParseError> {
        match verdict_str {
            "pass" => Ok(Verdict::Pass),
            "pass_with_residuals" => {
                let count = count_keys
                    .iter()
                    .find_map(|k| value.get(k).and_then(|v| v.as_u64()))
                    .map(|n| n as u32)
                    .ok_or_else(|| VerdictParseError::MissingResidualCount)?;
                Ok(Verdict::PassWithResiduals { count })
            }
            "fail" => {
                let reason = value
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.get("fail_reason").and_then(|v| v.as_str()))
                    .unwrap_or("verdict failed (no reason provided)")
                    .to_string();
                Ok(Verdict::Fail { reason })
            }
            other => Err(VerdictParseError::UnknownVerdict(other.to_string())),
        }
    }

    /// Resolve the verdict against the gate's `max_residuals` threshold.
    ///
    /// - `Pass` is unchanged.
    /// - `PassWithResiduals` with `count <= max_residuals` is
    ///   promoted to `Pass`.
    /// - `PassWithResiduals` with `count > max_residuals` is
    ///   downgraded to `Fail { reason: "residuals exceed max_residuals" }`.
    /// - `Fail` is unchanged.
    ///
    /// `max_residuals == None` keeps `PassWithResiduals` as-is
    /// (no promotion, no downgrade) so the gate is opt-in for the
    /// promotion rule.
    pub fn resolve(self, max_residuals: Option<u32>) -> Verdict {
        match self {
            Verdict::PassWithResiduals { count } => match max_residuals {
                Some(max) if count > max => Verdict::Fail {
                    reason: format!("residuals ({count}) exceed max_residuals ({max})"),
                },
                Some(_) => Verdict::Pass,
                None => Verdict::PassWithResiduals { count },
            },
            other => other,
        }
    }

    /// Returns true if this verdict is `Fail` after resolution.
    /// Convenience helper for the gate (`matches!(verdict, Fail { .. })`).
    pub fn is_fail(&self) -> bool {
        matches!(self, Verdict::Fail { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerdictParseError {
    MalformedJson(String),
    MissingField(String),
    MissingResidualCount,
    UnknownVerdict(String),
}

impl fmt::Display for VerdictParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerdictParseError::MalformedJson(msg) => {
                write!(f, "verdict payload not valid JSON: {msg}")
            }
            VerdictParseError::MissingField(name) => {
                write!(f, "verdict payload missing field `{name}`")
            }
            VerdictParseError::MissingResidualCount => {
                write!(
                    f,
                    "pass_with_residuals verdict missing residual count field"
                )
            }
            VerdictParseError::UnknownVerdict(v) => write!(f, "unknown verdict value: {v}"),
        }
    }
}

impl std::error::Error for VerdictParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pass_verdict() {
        let payload = r#"{"verdict":"pass"}"#;
        let v = Verdict::from_payload(payload, "verdict", None).unwrap();
        assert_eq!(v, Verdict::Pass);
        assert!(!v.is_fail());
    }

    #[test]
    fn promotes_pass_with_residuals_below_threshold() {
        let payload = r#"{"verdict":"pass_with_residuals","final_findings_count":5}"#;
        let v = Verdict::from_payload(payload, "verdict", None).unwrap();
        let resolved = v.resolve(Some(8));
        assert_eq!(resolved, Verdict::Pass);
        assert!(!resolved.is_fail());
    }

    #[test]
    fn keeps_pass_with_residuals_at_threshold() {
        // `count == max_residuals` is at the boundary — must promote
        // to Pass (the threshold is inclusive: `count > max` downgrades).
        let payload = r#"{"verdict":"pass_with_residuals","final_findings_count":8}"#;
        let v = Verdict::from_payload(payload, "verdict", None).unwrap();
        let resolved = v.resolve(Some(8));
        assert_eq!(resolved, Verdict::Pass);
    }

    #[test]
    fn downgrades_pass_with_residuals_above_threshold() {
        let payload = r#"{"verdict":"pass_with_residuals","final_findings_count":12}"#;
        let v = Verdict::from_payload(payload, "verdict", None).unwrap();
        let resolved = v.resolve(Some(8));
        assert!(resolved.is_fail(), "expected Fail, got {resolved}");
        if let Verdict::Fail { reason } = resolved {
            assert!(
                reason.contains("12") && reason.contains("8"),
                "reason should reference the count and threshold, got: {reason}"
            );
        }
    }

    #[test]
    fn keeps_pass_with_residuals_when_threshold_unset() {
        // `max_residuals: None` preserves PassWithResiduals as-is.
        let payload = r#"{"verdict":"pass_with_residuals","final_findings_count":3}"#;
        let v = Verdict::from_payload(payload, "verdict", None).unwrap();
        let resolved = v.resolve(None);
        assert_eq!(
            resolved,
            Verdict::PassWithResiduals { count: 3 },
            "None threshold must keep PassWithResiduals unchanged"
        );
    }

    #[test]
    fn parses_fail_with_reason() {
        let payload = r#"{"verdict":"fail","reason":"tests broke"}"#;
        let v = Verdict::from_payload(payload, "verdict", None).unwrap();
        assert_eq!(
            v,
            Verdict::Fail {
                reason: "tests broke".to_string()
            }
        );
        assert!(v.is_fail());
    }

    #[test]
    fn parses_fail_without_reason_uses_default() {
        let payload = r#"{"verdict":"fail"}"#;
        let v = Verdict::from_payload(payload, "verdict", None).unwrap();
        if let Verdict::Fail { reason } = v {
            assert!(reason.contains("no reason"), "default reason: {reason}");
        } else {
            panic!("expected Fail");
        }
    }

    #[test]
    fn missing_verdict_field_returns_error() {
        let payload = r#"{"foo":"bar"}"#;
        let err = Verdict::from_payload(payload, "verdict", None).unwrap_err();
        assert!(matches!(err, VerdictParseError::MissingField(_)));
    }

    #[test]
    fn pass_with_residuals_missing_count_returns_error() {
        let payload = r#"{"verdict":"pass_with_residuals"}"#;
        let err = Verdict::from_payload(payload, "verdict", None).unwrap_err();
        assert!(matches!(err, VerdictParseError::MissingResidualCount));
    }

    #[test]
    fn unknown_verdict_returns_error() {
        let payload = r#"{"verdict":"maybe"}"#;
        let err = Verdict::from_payload(payload, "verdict", None).unwrap_err();
        assert!(matches!(err, VerdictParseError::UnknownVerdict(s) if s == "maybe"));
    }

    #[test]
    fn malformed_json_returns_error() {
        let payload = "not json";
        let err = Verdict::from_payload(payload, "verdict", None).unwrap_err();
        assert!(matches!(err, VerdictParseError::MalformedJson(_)));
    }

    #[test]
    fn falls_back_to_pass_or_fail_field() {
        // Legacy payload that still uses the old key — the parser
        // must accept it for backwards compatibility.
        let payload = r#"{"pass_or_fail":"pass"}"#;
        let v = Verdict::from_payload(payload, "verdict", None).unwrap();
        assert_eq!(v, Verdict::Pass);
    }

    #[test]
    fn custom_count_field_name_is_honored() {
        let payload = r#"{"verdict":"pass_with_residuals","residual_count":2}"#;
        let v = Verdict::from_payload(payload, "verdict", Some("residual_count")).unwrap();
        assert_eq!(v.resolve(Some(8)), Verdict::Pass);
    }

    #[test]
    fn display_renders_verdict_string() {
        assert_eq!(Verdict::Pass.to_string(), "pass");
        assert_eq!(
            Verdict::PassWithResiduals { count: 3 }.to_string(),
            "pass_with_residuals"
        );
        assert_eq!(
            Verdict::Fail {
                reason: "x".to_string()
            }
            .to_string(),
            "fail"
        );
    }
}
