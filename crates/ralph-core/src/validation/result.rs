//! Validation result types (U4).
//!
//! All values are plain data so the pipeline is `Send + Sync` and
//! the result can be moved across the bus without further locking.
//! The `ReasonCode` is a [`String`] (vs `&'static str`) because
//! some reason codes are dynamically assembled from the offending
//! payload field name (e.g. `engine_rejected:required_field:foo`).

use serde::{Deserialize, Serialize};

/// Outcome of running a single [`crate::validation::ValidationRule`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    /// `true` when the rule passed (event may proceed).
    pub accepted: bool,
    /// Stable reason code identifying *why* the event was rejected.
    /// `None` when `accepted == true`. Format: `<stage>:<reason>`
    /// or `<stage>:<reason>:<detail>` for richer codes. Examples:
    ///   * `origin:ralph_control_only`
    ///   * `engine_rejected:required_field:plan_name`
    ///   * `execution_contract:missing_task_id`
    ///   * `step_handoff:progress_task_mismatch`
    ///   * `hat_handoff:missing_section`
    ///   * `workflow_guard:out_of_order`
    ///   * `publisher:topic_not_allowed`
    pub reason_code: Option<String>,
    /// Stage name (`ValidationRule::name()`). Echoed for diagnostics
    /// so the reason_code prefix can be cross-checked in tests
    /// without parsing the string.
    pub stage: ValidationStage,
    /// Optional human-readable correction hint for the agent. Kept
    /// short — the `HUMAN GUIDANCE` injection path embeds these
    /// strings into a numbered list, so embedded newlines would
    /// break the layout.
    pub correction_hint: Option<String>,
    /// `true` when the rejection is `recoverable` — i.e. the agent
    /// can fix the event and retry without orchestrator-state
    /// changes. `false` for terminal failures (unknown hat, scope
    /// violation) that need human steering.
    pub retry_eligible: bool,
}

impl ValidationResult {
    /// Convenience constructor for an accepted result. The
    /// `stage` defaults to [`ValidationStage::Origin`] to keep
    /// existing call sites compiling; new call sites should use
    /// [`Self::accept_with`] so the `stage` reflects the rule
    /// that emitted the verdict. Mismatched stages are a known
    /// source of misleading diagnostic logs (see
    /// `2026-06-21-002-adversarial-review.md` P2-#1) — the
    /// default stays only as a compatibility shim, not a
    /// recommendation.
    pub fn accept() -> Self {
        Self::accept_with(ValidationStage::Origin)
    }

    /// Build an accepted result tagged with the rule's stage.
    /// Prefer this over [`Self::accept`] so the `stage` field
    /// matches the rule that produced the verdict.
    pub fn accept_with(stage: ValidationStage) -> Self {
        Self {
            accepted: true,
            reason_code: None,
            stage,
            correction_hint: None,
            retry_eligible: false,
        }
    }

    /// Convenience constructor for a rejection. `stage` is the rule
    /// that emitted the rejection; `reason_code` is the full code
    /// including any dynamic detail.
    pub fn reject(
        stage: ValidationStage,
        reason_code: impl Into<String>,
        correction_hint: Option<String>,
        retry_eligible: bool,
    ) -> Self {
        Self {
            accepted: false,
            reason_code: Some(reason_code.into()),
            stage,
            correction_hint,
            retry_eligible,
        }
    }
}

/// Stable identifier for each validation stage. The string form is
/// used in `reason_code` prefixes so the enum variants must remain
/// in sync with `reason_code` constants in the legacy code paths
/// (`event_origin`, `execution_contract`, `hat_handoff::gate`,
/// `step_handoff::progress_task_gate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStage {
    /// Event origin guard — `event_origin::validate_event_origin`.
    Origin,
    /// Topic publisher scope check — `ProtocolView::topic_publisher_allowed`.
    Publisher,
    /// Required fields check — `ProtocolView::required_fields_for`.
    RequiredFields,
    /// Execution contract — `execution_contract::validate_execution_contract`.
    /// PostCommit phase.
    ExecutionContract,
    /// Step handoff gate — `step_handoff::progress_task_gate`.
    StepHandoff,
    /// Hat-handoff gate — `hat_handoff::gate::evaluate_event`.
    HatHandoff,
    /// Workflow guard — `event_loop::apply_workflow_guard_validation`.
    /// PostCommit phase.
    WorkflowGuard,
}

impl ValidationStage {
    /// Stable string identifier (the `name()` of the matching
    /// [`crate::validation::ValidationRule`]). Used in
    /// `reason_code` prefixes.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Origin => "origin",
            Self::Publisher => "publisher",
            Self::RequiredFields => "required_fields",
            Self::ExecutionContract => "execution_contract",
            Self::StepHandoff => "step_handoff",
            Self::HatHandoff => "hat_handoff",
            Self::WorkflowGuard => "workflow_guard",
        }
    }
}

impl std::fmt::Display for ValidationStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Reason code constants. These match the legacy reason_code
/// strings emitted by the existing gate stack; keeping them as
/// constants (vs assembled per-call) gives the U4 pipeline a
/// stable surface for tests and downstream tooling.
pub struct ReasonCode;

impl ReasonCode {
    /// `ralph` pseudo-hat emitted a non-control topic.
    pub const RALPH_CONTROL_ONLY: &'static str = "origin:ralph_control_only";
    /// Hat field is not in the registry.
    pub const ORIGIN_UNKNOWN_HAT: &'static str = "origin:unknown_hat";
    /// Hat is registered but cannot publish this topic.
    pub const ORIGIN_OUT_OF_SCOPE: &'static str = "origin:out_of_scope";
    /// Topic is in the orchestrator control or diagnostic allowlist
    /// (always passes; presence kept for symmetry).
    pub const ORIGIN_CONTROL_TOPIC: &'static str = "origin:control_topic";

    /// Topic is not in the allowed publisher set.
    pub const PUBLISHER_NOT_ALLOWED: &'static str = "publisher:topic_not_allowed";

    /// Engine-required field missing from payload.
    pub const REQUIRED_FIELD_MISSING: &'static str = "engine_rejected:required_field";

    /// Execution contract: payload missing `task_id` field.
    pub const CONTRACT_MISSING_TASK_ID: &'static str = "contract:missing_task_id";
    /// Execution contract: task is not in terminal status.
    pub const CONTRACT_TASK_NOT_TERMINAL: &'static str = "contract:task_not_terminal";
    /// Execution contract: task does not exist.
    pub const CONTRACT_TASK_NOT_FOUND: &'static str = "contract:task_not_found";
    /// Execution contract: payload is malformed.
    pub const CONTRACT_INVALID_PAYLOAD: &'static str = "contract:invalid_payload";
    /// Execution contract: no git evidence.
    pub const CONTRACT_NO_GIT_EVIDENCE: &'static str = "contract:no_git_evidence";
    /// Execution contract: no test evidence.
    pub const CONTRACT_NO_TEST_EVIDENCE: &'static str = "contract:no_test_evidence";

    /// Step handoff: progress ↔ tasks mismatch. The detailed reason
    /// is appended after the prefix.
    pub const STEP_HANDOFF_MISMATCH_PREFIX: &'static str = "step_handoff:";

    /// Hat handoff: macro edge but `handoff_path` missing.
    pub const HAT_HANDOFF_MISSING_PATH: &'static str = "hat_handoff:missing_path";
    /// Hat handoff: required artifact section missing.
    pub const HAT_HANDOFF_MISSING_SECTION: &'static str = "hat_handoff:missing_section";
    /// Hat handoff: artifact structure invalid.
    pub const HAT_HANDOFF_STRUCTURE_INVALID: &'static str = "hat_handoff:structure_invalid";
    /// Hat handoff: not required (passthrough, accepted).
    pub const HAT_HANDOFF_NOT_REQUIRED: &'static str = "hat_handoff:not_required";

    /// Workflow guard: event out of order for the configured chain.
    pub const WORKFLOW_GUARD_OUT_OF_ORDER: &'static str = "workflow_guard:out_of_order";
}

/// Rejection hint helpers. Kept tiny so the `HUMAN GUIDANCE`
/// injection path stays a single-line string per entry.
pub struct RejectionHint;

impl RejectionHint {
    /// Hint for the `missing_task_id` case — agent must include
    /// the `task_id` field in the `work.done` payload.
    pub fn missing_task_id(_task_id_field: &str) -> String {
        "work.done payload missing required `task_id` field".to_string()
    }

    /// Hint for the `task_not_terminal` case.
    pub fn task_not_terminal(task_id: &str) -> String {
        format!(
            "Run `ralph tools task close {task_id}` first, then re-emit work.done with task_id={task_id}."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_strings_match_legacy_names() {
        // Belt-and-suspenders: stage names are part of the public
        // surface (used in `reason_code` prefixes). Drift here
        // would silently break downstream tooling.
        assert_eq!(ValidationStage::Origin.as_str(), "origin");
        assert_eq!(ValidationStage::Publisher.as_str(), "publisher");
        assert_eq!(ValidationStage::RequiredFields.as_str(), "required_fields");
        assert_eq!(
            ValidationStage::ExecutionContract.as_str(),
            "execution_contract"
        );
        assert_eq!(ValidationStage::StepHandoff.as_str(), "step_handoff");
        assert_eq!(ValidationStage::HatHandoff.as_str(), "hat_handoff");
        assert_eq!(ValidationStage::WorkflowGuard.as_str(), "workflow_guard");
    }

    #[test]
    fn reject_helper_populates_fields() {
        let r = ValidationResult::reject(
            ValidationStage::ExecutionContract,
            ReasonCode::CONTRACT_MISSING_TASK_ID,
            Some("add `task_id` field".to_string()),
            true,
        );
        assert!(!r.accepted);
        assert_eq!(
            r.reason_code.as_deref(),
            Some(ReasonCode::CONTRACT_MISSING_TASK_ID)
        );
        assert_eq!(r.stage, ValidationStage::ExecutionContract);
        assert_eq!(r.correction_hint.as_deref(), Some("add `task_id` field"));
        assert!(r.retry_eligible);
    }

    #[test]
    fn accept_helper_has_no_reason_code() {
        let r = ValidationResult::accept();
        assert!(r.accepted);
        assert!(r.reason_code.is_none());
        // P2-#1: `accept_with(stage)` lets the rule tag its
        // own stage. The default `accept()` still returns
        // `Origin` (compat shim) — verified below.
        assert_eq!(r.stage, ValidationStage::Origin);
        let r = ValidationResult::accept_with(ValidationStage::WorkflowGuard);
        assert!(r.accepted);
        assert_eq!(r.stage, ValidationStage::WorkflowGuard);
    }
}
