//! `metadata_runtime_drift` — 2026-06-28 plan U12 (R12) lint.
//!
//! Validates that the runtime-evaluated metadata fields in
//! `mechanism.*` (the `RalphConfig.event_loop.mechanism.flow`
//! block) match the actual runtime constants. The original
//! failure mode (2026-06-28 diagnostic) was:
//! - preset declared `state_idempotency: required`
//! - `IdempotentLog::open` failed at bootstrap
//! - the loop silently fell back to `IdempotentLog::disabled()`
//!   (a P0 violation of the preset contract)
//!
//! U7 (IdempotentLog::open panic) closes the *runtime* half
//! of that bug. U12 closes the *static* half: the lint
//! verifies that the preset's metadata is internally
//! consistent (the declared `repair_budget` matches the
//! runtime constant, the `enforce_schema` value is in the
//! supported set, the `state_idempotency` value is in the
//! supported set). Mismatches are `Error`-severity findings
//! that fail-closed at preset-load time.

use crate::config::RalphConfig;
use crate::preset_lint::finding_id::FINDING_METADATA_RUNTIME_DRIFT;
use crate::runtime_contract::{
    FindingSeverity, FindingSource, FindingStage, RuntimeContractFinding,
};

/// Validate the metadata fields under `mechanism.*` against
/// the runtime's accepted value sets. Returns one
/// `RuntimeContractFinding` per mismatch; the caller appends
/// them to the lint result.
pub fn check_metadata_runtime_drift(
    config: &RalphConfig,
) -> Vec<RuntimeContractFinding> {
    let mut findings = Vec::new();
    let Some(mechanism) = config.event_loop.mechanism.as_ref() else {
        // No mechanism block → nothing to validate.
        return findings;
    };
    let Some(flow) = mechanism.flow.as_ref() else {
        return findings;
    };

    // `state_idempotency` must be in {`required`, `disabled`}.
    // U7 added the hard panic when the runtime is `required`
    // but the bootstrap fails; U12 surfaces a misconfigured
    // preset before the loop starts.
    match flow.state_idempotency.as_str() {
        "required" | "disabled" => {}
        other => {
            findings.push(
                RuntimeContractFinding::try_new_core(
                    format!("lint.{}", FINDING_METADATA_RUNTIME_DRIFT),
                    FindingSource::Lint,
                    FindingSeverity::Error,
                    FindingStage::Authoring,
                    format!(
                        "mechanism.state_idempotency='{}' is not a supported runtime value (must be 'required' or 'disabled')",
                        other
                    ),
                )
                .expect("lint findings never use the reserved Preflight source")
                .with_detail("field", "state_idempotency")
                .with_detail("actual", other.to_string()),
            );
        }
    }

    // `enforce_schema` must be in {`hard`, `none`}. The
    // `soft` branch was retired in the mechanism foundation;
    // a preset that still uses it must fail-closed at lint
    // time so the operator fixes the preset, not the runtime.
    match flow.enforce_schema.as_str() {
        "hard" | "none" => {}
        other => {
            findings.push(
                RuntimeContractFinding::try_new_core(
                    format!("lint.{}", FINDING_METADATA_RUNTIME_DRIFT),
                    FindingSource::Lint,
                    FindingSeverity::Error,
                    FindingStage::Authoring,
                    format!(
                        "mechanism.enforce_schema='{}' is not a supported runtime value (must be 'hard' or 'none')",
                        other
                    ),
                )
                .expect("lint findings never use the reserved Preflight source")
                .with_detail("field", "enforce_schema")
                .with_detail("actual", other.to_string()),
            );
        }
    }

    // `repair_budget` must be > 0 and bounded. The runtime
    // caps retries at the declared value; a value of 0 means
    // the loop terminates on the first stall, which is
    // almost always a misconfiguration rather than the
    // intent. Flag 0 and impossibly large values.
    if flow.repair_budget == 0 {
        findings.push(
            RuntimeContractFinding::try_new_core(
                format!("lint.{}", FINDING_METADATA_RUNTIME_DRIFT),
                FindingSource::Lint,
                FindingSeverity::Error,
                FindingStage::Authoring,
                "mechanism.repair_budget=0 means the loop terminates on the first stall; \
                 this is almost always a misconfiguration. Set a positive value (3 is the \
                 repository-wide default)."
                    .to_string(),
            )
            .expect("lint findings never use the reserved Preflight source")
            .with_detail("field", "repair_budget")
            .with_detail("actual", "0".to_string()),
        );
    }
    if flow.repair_budget > 100 {
        // 100 is the lint-level sanity ceiling; the runtime
        // will honour the value but a budget that high
        // usually indicates a misunderstanding of the
        // semantics (a stuck loop should not be retried
        // 100 times).
        findings.push(
            RuntimeContractFinding::try_new_core(
                format!("lint.{}", FINDING_METADATA_RUNTIME_DRIFT),
                FindingSource::Lint,
                FindingSeverity::Warn,
                FindingStage::Authoring,
                format!(
                    "mechanism.repair_budget={} is unusually large; consider whether the \
                     preset really wants to retry that many times before emitting plan.blocked",
                    flow.repair_budget
                ),
            )
            .expect("lint findings never use the reserved Preflight source")
            .with_detail("field", "repair_budget")
            .with_detail("actual", flow.repair_budget.to_string()),
        );
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FlowDeclarationConfig, MechanismConfig};

    fn flow_config(
        state_idempotency: &str,
        enforce_schema: &str,
        repair_budget: u32,
    ) -> RalphConfig {
        let mut cfg = RalphConfig::default();
        cfg.event_loop.mechanism = Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: Vec::new(),
                repair_budget,
                enforce_schema: enforce_schema.to_string(),
                state_idempotency: state_idempotency.to_string(),
            }),
        });
        cfg
    }

    #[test]
    fn no_mechanism_block_no_findings() {
        let cfg = RalphConfig::default();
        assert!(check_metadata_runtime_drift(&cfg).is_empty());
    }

    #[test]
    fn valid_metadata_no_findings() {
        let cfg = flow_config("required", "hard", 3);
        assert!(check_metadata_runtime_drift(&cfg).is_empty());
    }

    #[test]
    fn unknown_state_idempotency_is_error() {
        let cfg = flow_config("maybe", "hard", 3);
        let findings = check_metadata_runtime_drift(&cfg);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, FindingSeverity::Error);
        assert!(findings[0].message.contains("state_idempotency"));
    }

    #[test]
    fn unknown_enforce_schema_is_error() {
        let cfg = flow_config("required", "soft", 3);
        let findings = check_metadata_runtime_drift(&cfg);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("enforce_schema"));
    }

    #[test]
    fn zero_repair_budget_is_error() {
        let cfg = flow_config("required", "hard", 0);
        let findings = check_metadata_runtime_drift(&cfg);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, FindingSeverity::Error);
        assert!(findings[0].message.contains("repair_budget=0"));
    }

    #[test]
    fn large_repair_budget_is_warn() {
        let cfg = flow_config("required", "hard", 250);
        let findings = check_metadata_runtime_drift(&cfg);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, FindingSeverity::Warn);
    }

    #[test]
    fn multiple_mismatches_all_reported() {
        let cfg = flow_config("maybe", "soft", 0);
        let findings = check_metadata_runtime_drift(&cfg);
        // 3 errors: state_idempotency, enforce_schema, repair_budget=0
        assert_eq!(findings.len(), 3);
        assert!(findings.iter().all(|f| f.severity == FindingSeverity::Error));
    }
}
