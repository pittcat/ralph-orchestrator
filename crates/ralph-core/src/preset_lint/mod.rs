//! Preset Static Lint — entry point and shared types.
//!
//! This module is the public face of the preset-lint subsystem. It hosts
//! shared types (`LintSeverity`, `LintFinding`, `LintStrictness`), the
//! U3 orchestrator (`run_preset_lint`), and the lint→contract adapter.
//! The four rule families live in sibling modules:
//!
//! - [`finding_id`] — stable finding ID constants.
//! - [`topic_format`] — U1 topic format rules + surface enumeration.
//! - [`ownership`] — U2 R2/R3/R4 ownership rules.
//! - [`coordinator`] — U2 R5 coordinator rules.
//!
//! Implementation Plan Unit: U1/U2/U3 of `2026-06-08-003-feat-preset-static-lint-plan`.
//!
//! Stability rules:
//! - The `finding_id` constants are part of the public contract.
//! - The `TopicSurface` enum variants are source of truth for which
//!   config locations are linted.
//! - `TopicOccurrence` fields (`topic`, `surface`, `hat`) are stable.

use crate::config::RalphConfig;
use crate::runtime_contract::{
    FindingSeverity, FindingSource, FindingStage, RuntimeContractFinding,
};

pub mod coordinator;
pub mod finding_id;
pub mod multi_hat;
pub mod ownership;
pub mod topic_format;
pub mod workflow_activation;

#[cfg(test)]
mod tests;

pub use coordinator::check_coordinator_rules;
pub use finding_id::{
    FINDING_ACTIVATION_EGRESS_MISSING, FINDING_COORDINATOR_MISSING,
    FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH, FINDING_HANDOFF_PAIRING_BROKEN,
    FINDING_HANDOFF_SEED_DERIVED_CONFLICT, FINDING_INVALID_TOPIC_FORMAT,
    FINDING_MISSING_TOPIC_OWNER, FINDING_MULTI_HAT_REQUIRES_ISOLATED,
    FINDING_OWNER_NOT_PUBLISHER, FINDING_OWNER_UNKNOWN_HAT, FINDING_RE_EMIT_TRAP,
    FINDING_TASK_PUBLISHER_NOT_COORDINATED, FINDING_TRIGGER_PUBLISH_ASYMMETRY,
    FINDING_WHITELIST_EXEMPT_TOPIC,
};

// Re-export the WAC top-level entry point so callers (and the
// WAC-U8 BDD scenarios) can invoke the rule family without
// reaching into the module directly. The function is NOT yet
// wired into `run_preset_lint`: per the
// `2026-06-12-002-feat-workflow-activation-contract-plan`
// phasing, WAC joins the public `run_preset_lint` pipeline in
// WAC-U8 once the builtin presets have been migrated to pass
// the WAC rules (WAC-U4). Until then the function is reachable
// for direct callers (CLI diagnostic, BDD scenarios) but does
// not affect the run gate.
pub use workflow_activation::{
    HandoffGraph, run_workflow_activation_contract,
};
pub use multi_hat::check_multi_hat_isolation;
pub use ownership::{check_owner_references, check_ownership_rules};
pub use topic_format::{
    TopicFormatResult, TopicOccurrence, TopicSurface, enumerate_topics, suggest_topic_fix,
    validate_all_topics, validate_topic_format,
};

// ──────────────────────────────────────────────────────────────────────────
// U2: Shared types — strictness, severity, finding
// ──────────────────────────────────────────────────────────────────────────

/// Severity override for strict mode (U2 checks that are warn-by-default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintStrictness {
    /// Default mode: ownership warnings remain warnings.
    Default,
    /// Strict mode: ownership warnings become errors.
    Strict,
}

impl LintStrictness {
    /// Returns the severity to use for checks that are warn-by-default.
    pub fn ownership_severity(self) -> LintSeverity {
        match self {
            Self::Default => LintSeverity::Warn,
            Self::Strict => LintSeverity::Error,
        }
    }
}

/// Severity level for a lint finding.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum LintSeverity {
    /// Hard error — must be fixed before proceeding.
    Error,
    /// Warning — should be fixed, but non-blocking in default mode.
    Warn,
    /// Informational pass — the check succeeded.
    Pass,
}

impl std::fmt::Display for LintSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error => write!(f, "error"),
            Self::Warn => write!(f, "warn"),
            Self::Pass => write!(f, "pass"),
        }
    }
}

impl LintSeverity {
    /// Parse a severity string, returning `None` for unknown values.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "error" => Some(Self::Error),
            "warn" => Some(Self::Warn),
            "pass" => Some(Self::Pass),
            _ => None,
        }
    }
}

/// Result of a single U2 ownership / coordinator check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintFinding {
    /// Stable machine finding ID (e.g. `preset.owner_unknown_hat`).
    pub id: &'static str,
    /// Severity level — type-safe enum preventing invalid values.
    pub severity: LintSeverity,
    /// Human-readable summary.
    pub message: String,
    /// Optional topic involved.
    pub topic: Option<String>,
    /// Optional hat involved.
    pub hat: Option<String>,
    /// Optional owner hat.
    pub owner: Option<String>,
    /// Optional fix hint.
    pub action_hint: Option<String>,
}

impl LintFinding {
    pub(crate) fn error(id: &'static str, message: impl Into<String>) -> Self {
        Self {
            id,
            severity: LintSeverity::Error,
            message: message.into(),
            topic: None,
            hat: None,
            owner: None,
            action_hint: None,
        }
    }

    pub(crate) fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    pub(crate) fn with_hat(mut self, hat: impl Into<String>) -> Self {
        self.hat = Some(hat.into());
        self
    }

    pub(crate) fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub(crate) fn with_action_hint(mut self, hint: impl Into<String>) -> Self {
        self.action_hint = Some(hint.into());
        self
    }
}

/// Run all U2 ownership and coordinator checks.
///
/// Returns a sorted, deterministic list of findings.
pub fn validate_ownership_and_coordinator(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    findings.extend(check_owner_references(config));
    findings.extend(check_ownership_rules(config, strictness));
    findings.extend(check_coordinator_rules(config));

    // Sort by (id, topic, hat) for deterministic output.
    findings.sort_by(|a, b| {
        a.id.cmp(b.id)
            .then(a.topic.cmp(&b.topic))
            .then(a.hat.cmp(&b.hat))
    });

    findings
}

// ──────────────────────────────────────────────────────────────────────────
// U3: Convert preset_lint findings → RuntimeContractFinding
// ──────────────────────────────────────────────────────────────────────────

/// Convert a `LintSeverity` to a `RuntimeContractFinding` severity.
fn lint_severity_to_contract(severity: LintSeverity) -> FindingSeverity {
    match severity {
        LintSeverity::Error => FindingSeverity::Error,
        LintSeverity::Warn => FindingSeverity::Warn,
        LintSeverity::Pass => FindingSeverity::Pass,
    }
}

/// Convert a single `LintFinding` into a `RuntimeContractFinding`.
///
/// The `source` is always `FindingSource::Lint` and the `stage` is
/// `FindingStage::Authoring`. The `id` is prefixed with `lint.` to
/// distinguish it from config/topology/payload/orphan findings.
fn lint_finding_to_contract(finding: &LintFinding) -> RuntimeContractFinding {
    // Prefix the id with "lint." for machine-readable source separation.
    let id = format!("lint.{}", finding.id);
    let severity = lint_severity_to_contract(finding.severity);

    let mut contract_finding = RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Lint,
        severity,
        FindingStage::Authoring,
        finding.message.clone(),
    )
    .expect("lint findings never use the reserved Preflight source");

    if let Some(topic) = &finding.topic {
        contract_finding = contract_finding.with_detail("topic", topic.clone());
    }
    if let Some(hat) = &finding.hat {
        contract_finding = contract_finding.with_detail("hat", hat.clone());
    }
    if let Some(owner) = &finding.owner {
        contract_finding = contract_finding.with_detail("owner", owner.clone());
    }
    if let Some(hint) = &finding.action_hint {
        contract_finding = contract_finding.with_action_hint(hint.clone());
    }

    contract_finding
}

/// Convert a batch of `LintFinding` entries into `RuntimeContractFinding`
/// entries suitable for inclusion in a `RuntimeContractReport`.
///
/// Returns findings in deterministic order (sorted by id, topic, hat).
pub fn lint_findings_to_contract_findings(findings: &[LintFinding]) -> Vec<RuntimeContractFinding> {
    findings.iter().map(lint_finding_to_contract).collect()
}

/// Run all U3 lint checks (topic format + ownership + coordinator)
/// and return findings as `RuntimeContractFinding` entries.
///
/// This is the single entry point called by `RuntimeContractAggregator`.
/// Findings are deterministic and sorted.
pub fn run_preset_lint(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<RuntimeContractFinding> {
    let mut findings: Vec<RuntimeContractFinding> = Vec::new();

    // Topic format validation (U1)
    let format_results = validate_all_topics(config);
    for result in format_results {
        if result.is_valid {
            if result.is_whitelisted {
                // Whitelisted topics are informational passes.
                let id = format!("lint.{}", FINDING_WHITELIST_EXEMPT_TOPIC);
                let finding = RuntimeContractFinding::try_new_core(
                    id,
                    FindingSource::Lint,
                    FindingSeverity::Pass,
                    FindingStage::Authoring,
                    format!(
                        "topic \"{}\" is in the whitelist and exempt from format checks",
                        result.token
                    ),
                )
                .expect("lint findings never use the reserved Preflight source")
                .with_detail("topic", result.token.clone());
                findings.push(finding);
            }
            // Valid non-whitelisted topics produce no finding.
        } else {
            // Invalid topic format.
            let id = format!("lint.{}", FINDING_INVALID_TOPIC_FORMAT);
            let mut finding = RuntimeContractFinding::try_new_core(
                id,
                FindingSource::Lint,
                FindingSeverity::Warn,
                FindingStage::Authoring,
                format!(
                    "topic \"{}\" violates the lowercase dot-case format",
                    result.token
                ),
            )
            .expect("lint findings never use the reserved Preflight source")
            .with_detail("topic", result.token.clone());

            if let Some(suggestion) = &result.suggestion {
                finding = finding.with_action_hint(format!(
                    "Rename to \"{}\" or add to topic_format_whitelist",
                    suggestion
                ));
            }
            findings.push(finding);
        }
    }

    // Ownership & coordinator checks (U2)
    let ownership_findings = validate_ownership_and_coordinator(config, strictness);
    findings.extend(lint_findings_to_contract_findings(&ownership_findings));

    // Multi-hat isolation policy (U1 of 2026-06-11-003): always
    // Error, never downgraded by `LintStrictness`. Produces
    // `RuntimeContractFinding` directly because the structured
    // details `actual` / `limit` / `required_mode` must flow
    // through to the runtime contract aggregator's `details` map.
    findings.extend(check_multi_hat_isolation(config));

    // Sort by id, then topic for deterministic output.
    findings.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then(a.details.get("topic").cmp(&b.details.get("topic")))
            .then(a.details.get("hat").cmp(&b.details.get("hat")))
    });

    // Filter out Pass findings — they are informational only and do not
    // affect the report's pass/fail status.
    findings
        .into_iter()
        .filter(|f| f.severity != FindingSeverity::Pass)
        .collect()
}
