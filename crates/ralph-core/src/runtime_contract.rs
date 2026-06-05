//! Unified Runtime Contract report data model.
//!
//! This module defines the shared `RuntimeContractReport` /
//! `RuntimeContractFinding` types that preset / workflow contract checks
//! (config, topology, orphan, payload) converge into. Different entry points
//! (`ralph preset check`, `ralph hats validate`, `ralph preflight`,
//! `enforce_payload_contract_gate`) feed the same report shape so callers
//! can rely on stable JSON output and consistent strict semantics.
//!
//! Implementation Units:
//! - U1: introduce this data model (this file).
//! - U2: aggregator that maps existing validators into findings.
//! - U3+: CLI adapters that expose the report.
//!
//! Stability rules:
//! - The JSON field names (`id`, `source`, `severity`, `stage`,
//!   `message`, `details`, `action_hint`, `source_label`, `payload_strict`,
//!   `fail_on_warnings`, `passed`, `findings`, `checked_at`) are part of
//!   the public contract; renaming or repurposing them is a breaking
//!   change for downstream diagnostics / CI consumers.
//! - `source=preflight` is reserved for CLI/preflight adapter wrapper
//!   reports. The core preset contract aggregator must not stamp findings
//!   with `source=preflight`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Origin of a finding within the runtime contract surface.
///
/// `Preflight` is reserved for adapter-layer wrapper reports; the core
/// preset contract aggregator must not produce findings with this source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSource {
    /// `RalphConfig::validate()` findings.
    Config,
    /// `validate_preset_topology()` findings.
    Topology,
    /// Orphan topic detection findings.
    Orphan,
    /// `validate_payload_contract()` findings.
    Payload,
    /// Reserved for CLI/preflight adapter wrapper reports.
    Preflight,
}

impl FindingSource {
    /// Returns the stable string label used in JSON output and machine IDs.
    pub fn as_str(self) -> &'static str {
        match self {
            FindingSource::Config => "config",
            FindingSource::Topology => "topology",
            FindingSource::Orphan => "orphan",
            FindingSource::Payload => "payload",
            FindingSource::Preflight => "preflight",
        }
    }
}

/// Severity classification for a finding.
///
/// The triple `Pass` / `Warn` / `Error` mirrors `CheckStatus` in
/// `preflight.rs` and is used uniformly across all entry points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    /// Check produced no actionable signal.
    Pass,
    /// Check produced a non-blocking signal.
    Warn,
    /// Check produced a blocking signal.
    Error,
}

impl FindingSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingSeverity::Pass => "pass",
            FindingSeverity::Warn => "warn",
            FindingSeverity::Error => "error",
        }
    }
}

/// Lifecycle stage at which a finding is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStage {
    /// Preset / workflow authoring time (`ralph preset check`).
    Authoring,
    /// Optional pre-run preflight (`ralph preflight`,
    /// `run_auto_preflight()`).
    Preflight,
    /// `enforce_payload_contract_gate()` at `ralph run` startup.
    RunHardGate,
}

impl FindingStage {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingStage::Authoring => "authoring",
            FindingStage::Preflight => "preflight",
            FindingStage::RunHardGate => "run_hard_gate",
        }
    }
}

/// A single normalized runtime contract finding.
///
/// `id` is a stable machine identifier (e.g.
/// `config.invalid_concurrency`, `topology.unreachable_completion`,
/// `payload.field_missing_from_schema`). `source` and `stage` together
/// describe where and when the finding fires; `severity` describes how
/// it interacts with the surrounding `RuntimeContractReport` strict
/// dimensions.
///
/// `details` carries structured, source-specific context. Keys are
/// stable within a source (for example `hat`, `topic`, `field`,
/// `schema_source`, `source_hats`); other keys are accepted but not
/// part of the stability contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeContractFinding {
    /// Stable machine ID, source-prefixed (e.g. `topology.unreachable_start`).
    pub id: String,
    /// Origin of the finding.
    pub source: FindingSource,
    /// Severity classification.
    pub severity: FindingSeverity,
    /// Lifecycle stage at which the finding was produced.
    pub stage: FindingStage,
    /// Human-readable summary.
    pub message: String,
    /// Optional structured context (e.g. `hat`, `topic`, `field`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, String>,
    /// Optional fix suggestion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_hint: Option<String>,
}

impl RuntimeContractFinding {
    /// Construct a new finding. `details` starts empty and can be
    /// extended via `with_detail`.
    pub fn new(
        id: impl Into<String>,
        source: FindingSource,
        severity: FindingSeverity,
        stage: FindingStage,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            source,
            severity,
            stage,
            message: message.into(),
            details: BTreeMap::new(),
            action_hint: None,
        }
    }

    /// Add a structured detail key/value pair. Returns `self` for chaining.
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    /// Set the action hint. Returns `self` for chaining.
    pub fn with_action_hint(mut self, hint: impl Into<String>) -> Self {
        self.action_hint = Some(hint.into());
        self
    }

    /// Returns `true` if this finding should cause the surrounding
    /// `RuntimeContractReport` to be marked failed.
    ///
    /// Strict semantics:
    /// - `severity = Error` always fails the report.
    /// - `severity = Warn` only fails the report when
    ///   `fail_on_warnings = true`.
    /// - `severity = Pass` never fails the report.
    pub fn is_blocking(&self, fail_on_warnings: bool) -> bool {
        match self.severity {
            FindingSeverity::Error => true,
            FindingSeverity::Warn => fail_on_warnings,
            FindingSeverity::Pass => false,
        }
    }
}

/// Configuration for the strict dimensions of a `RuntimeContractReport`.
///
/// `payload_strict` and `fail_on_warnings` are independent axes:
/// - `payload_strict` only affects the severity of payload findings
///   (e.g. missing schema is `warn` in non-strict, `error` in strict).
/// - `fail_on_warnings` controls whether warnings cause the overall
///   report to fail (`ralph preflight --strict` uses
///   `fail_on_warnings=true`; `ralph hats validate --strict` only sets
///   `payload_strict=true`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeContractStrictness {
    pub payload_strict: bool,
    pub fail_on_warnings: bool,
}

impl Default for RuntimeContractStrictness {
    fn default() -> Self {
        Self {
            payload_strict: false,
            fail_on_warnings: false,
        }
    }
}

impl RuntimeContractStrictness {
    /// Strict profile used by `ralph preset check --strict`:
    /// `payload_strict=true` AND `fail_on_warnings=true`.
    pub fn preset_check_strict() -> Self {
        Self {
            payload_strict: true,
            fail_on_warnings: true,
        }
    }
}

/// Aggregated runtime contract report.
///
/// `source_label` is a short, human-readable identifier of what was
/// checked (e.g. `builtin:ce-executor`, `path/to/preset.yml`,
/// `current-config`). It is informational and does not participate in
/// any logic; callers should treat it as the report's "name".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeContractReport {
    /// Human-readable identifier of the checked target.
    pub source_label: String,
    /// Whether payload findings should treat missing schemas as errors.
    pub payload_strict: bool,
    /// Whether warning findings should fail the report.
    pub fail_on_warnings: bool,
    /// Whether the report passed under the configured strictness.
    pub passed: bool,
    /// Number of warning findings (any severity, any source).
    pub warnings: usize,
    /// Number of error findings (any source).
    pub errors: usize,
    /// All findings collected for this report.
    pub findings: Vec<RuntimeContractFinding>,
    /// RFC3339 timestamp at which the report was assembled.
    pub checked_at: String,
}

impl RuntimeContractReport {
    /// Build a new report from a `source_label` and a strictness profile.
    ///
    /// `checked_at` is captured at construction time. Findings must be
    /// pushed via `add_finding` / `extend_findings`; `passed`,
    /// `warnings`, and `errors` are recomputed on demand.
    pub fn new(source_label: impl Into<String>, strictness: RuntimeContractStrictness) -> Self {
        Self {
            source_label: source_label.into(),
            payload_strict: strictness.payload_strict,
            fail_on_warnings: strictness.fail_on_warnings,
            passed: true,
            warnings: 0,
            errors: 0,
            findings: Vec::new(),
            checked_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Append a finding and refresh aggregate counters.
    pub fn add_finding(&mut self, finding: RuntimeContractFinding) -> &mut Self {
        match finding.severity {
            FindingSeverity::Error => self.errors += 1,
            FindingSeverity::Warn => self.warnings += 1,
            FindingSeverity::Pass => {}
        }
        self.findings.push(finding);
        self.recompute_passed();
        self
    }

    /// Append multiple findings and refresh aggregate counters.
    pub fn extend_findings<I: IntoIterator<Item = RuntimeContractFinding>>(
        &mut self,
        findings: I,
    ) -> &mut Self {
        for finding in findings {
            self.add_finding(finding);
        }
        self
    }

    /// Recompute `passed` from current `findings` and the configured
    /// strictness. A finding is blocking if its `is_blocking` method
    /// returns `true` under the report's `fail_on_warnings` setting.
    pub fn recompute_passed(&mut self) {
        self.passed = !self
            .findings
            .iter()
            .any(|f| f.is_blocking(self.fail_on_warnings));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warn_finding() -> RuntimeContractFinding {
        RuntimeContractFinding::new(
            "config.deferred_feature",
            FindingSource::Config,
            FindingSeverity::Warn,
            FindingStage::Authoring,
            "feature deferred",
        )
    }

    fn error_finding() -> RuntimeContractFinding {
        RuntimeContractFinding::new(
            "topology.unreachable_completion",
            FindingSource::Topology,
            FindingSeverity::Error,
            FindingStage::Authoring,
            "completion promise unreachable",
        )
    }

    #[test]
    fn empty_report_passes() {
        let report = RuntimeContractReport::new("empty", RuntimeContractStrictness::default());
        assert!(report.passed);
        assert_eq!(report.warnings, 0);
        assert_eq!(report.errors, 0);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn single_warning_finding_does_not_fail_non_strict_report() {
        let mut report =
            RuntimeContractReport::new("warn-only", RuntimeContractStrictness::default());
        report.add_finding(warn_finding());
        assert!(
            report.passed,
            "non-strict report should pass with one warning"
        );
        assert_eq!(report.warnings, 1);
        assert_eq!(report.errors, 0);
    }

    #[test]
    fn single_error_finding_fails_report() {
        let mut report =
            RuntimeContractReport::new("err-only", RuntimeContractStrictness::default());
        report.add_finding(error_finding());
        assert!(!report.passed);
        assert_eq!(report.warnings, 0);
        assert_eq!(report.errors, 1);
    }

    #[test]
    fn fail_on_warnings_makes_warnings_blocking() {
        let mut report = RuntimeContractReport::new(
            "strict-warn",
            RuntimeContractStrictness {
                payload_strict: false,
                fail_on_warnings: true,
            },
        );
        report.add_finding(warn_finding());
        assert!(!report.passed);
        assert_eq!(report.warnings, 1);
    }

    #[test]
    fn payload_strict_does_not_alter_non_payload_severity() {
        // payload_strict must only affect payload findings; config/orphan/topology
        // findings keep their intrinsic severity regardless of the flag.
        let mut report = RuntimeContractReport::new(
            "payload-strict",
            RuntimeContractStrictness {
                payload_strict: true,
                fail_on_warnings: false,
            },
        );
        report.add_finding(warn_finding());
        assert!(report.passed);
        assert_eq!(report.warnings, 1);
    }

    #[test]
    fn finding_is_blocking_respects_strictness() {
        let warn = warn_finding();
        assert!(!warn.is_blocking(false));
        assert!(warn.is_blocking(true));

        let err = error_finding();
        assert!(err.is_blocking(false));
        assert!(err.is_blocking(true));

        let pass = RuntimeContractFinding::new(
            "config.ok",
            FindingSource::Config,
            FindingSeverity::Pass,
            FindingStage::Authoring,
            "ok",
        );
        assert!(!pass.is_blocking(false));
        assert!(!pass.is_blocking(true));
    }

    #[test]
    fn json_serialization_contains_stable_fields() {
        let mut report = RuntimeContractReport::new(
            "json-fixture",
            RuntimeContractStrictness {
                payload_strict: true,
                fail_on_warnings: false,
            },
        );
        report.add_finding(
            error_finding()
                .with_detail("topic", "LOOP_COMPLETE")
                .with_detail("start", "task.start")
                .with_action_hint("add a hat that publishes the completion topic"),
        );
        let value: serde_json::Value = serde_json::to_value(&report).expect("serialize report");
        let obj = value
            .as_object()
            .expect("report should serialize to object");

        for key in [
            "source_label",
            "payload_strict",
            "fail_on_warnings",
            "passed",
            "warnings",
            "errors",
            "findings",
            "checked_at",
        ] {
            assert!(
                obj.contains_key(key),
                "report JSON missing stable key: {key}"
            );
        }

        let findings = obj
            .get("findings")
            .and_then(|v| v.as_array())
            .expect("findings should be an array");
        assert_eq!(findings.len(), 1);
        let finding = findings[0]
            .as_object()
            .expect("finding should be an object");
        for key in ["id", "source", "severity", "stage", "message", "details"] {
            assert!(
                finding.contains_key(key),
                "finding JSON missing stable key: {key}"
            );
        }
        assert_eq!(
            finding.get("source").and_then(|v| v.as_str()),
            Some("topology")
        );
        assert_eq!(
            finding.get("severity").and_then(|v| v.as_str()),
            Some("error")
        );
        assert_eq!(
            finding.get("stage").and_then(|v| v.as_str()),
            Some("authoring")
        );
        let details = finding
            .get("details")
            .and_then(|v| v.as_object())
            .expect("details should serialize to object");
        assert_eq!(
            details.get("topic").and_then(|v| v.as_str()),
            Some("LOOP_COMPLETE")
        );
    }

    #[test]
    fn finding_with_detail_and_action_hint_chains() {
        let finding = warn_finding()
            .with_detail("field", "concurrency")
            .with_action_hint("reduce concurrency to >= 1");
        assert_eq!(
            finding.details.get("field").map(String::as_str),
            Some("concurrency")
        );
        assert_eq!(
            finding.action_hint.as_deref(),
            Some("reduce concurrency to >= 1")
        );
    }

    #[test]
    fn recompute_passed_handles_mixed_findings() {
        let mut report = RuntimeContractReport::new(
            "mixed",
            RuntimeContractStrictness {
                payload_strict: false,
                fail_on_warnings: true,
            },
        );
        report.add_finding(warn_finding());
        assert!(!report.passed, "fail_on_warnings should make warn blocking");
        report.add_finding(error_finding());
        assert!(!report.passed);
        assert_eq!(report.warnings, 1);
        assert_eq!(report.errors, 1);
    }

    #[test]
    fn preset_check_strict_sets_both_axes() {
        let strictness = RuntimeContractStrictness::preset_check_strict();
        assert!(strictness.payload_strict);
        assert!(strictness.fail_on_warnings);
    }

    #[test]
    fn extend_findings_appends_in_order() {
        let mut report = RuntimeContractReport::new("extend", RuntimeContractStrictness::default());
        report.extend_findings([warn_finding(), error_finding()]);
        assert_eq!(report.findings.len(), 2);
        assert_eq!(report.warnings, 1);
        assert_eq!(report.errors, 1);
        assert!(!report.passed);
    }

    #[test]
    fn source_and_severity_and_stage_have_stable_strs() {
        assert_eq!(FindingSource::Config.as_str(), "config");
        assert_eq!(FindingSource::Topology.as_str(), "topology");
        assert_eq!(FindingSource::Orphan.as_str(), "orphan");
        assert_eq!(FindingSource::Payload.as_str(), "payload");
        assert_eq!(FindingSource::Preflight.as_str(), "preflight");

        assert_eq!(FindingSeverity::Pass.as_str(), "pass");
        assert_eq!(FindingSeverity::Warn.as_str(), "warn");
        assert_eq!(FindingSeverity::Error.as_str(), "error");

        assert_eq!(FindingStage::Authoring.as_str(), "authoring");
        assert_eq!(FindingStage::Preflight.as_str(), "preflight");
        assert_eq!(FindingStage::RunHardGate.as_str(), "run_hard_gate");
    }
}
