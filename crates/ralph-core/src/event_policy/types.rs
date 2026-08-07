//! Event policy validation for typed payload schema enforcement.
//!
//! Provides pure-function validation that can be used by the event loop,
//! CLI emit commands, and API layers.

use crate::config::RalphConfig;
use crate::event_reader::EventReader;
use crate::hat_registry::HatRegistry;
use ralph_proto::HatId;
use ralph_proto::Topic;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

// Re-export config types for convenience
pub use crate::config::{
    CompletionAfterTerminalAction, EventPolicyConfig, EventPolicyMode, PayloadType, ViolationAction,
};

/// Types of policy violations.
#[derive(Debug, Clone, PartialEq)]
pub enum ViolationType {
    PayloadTypeMismatch {
        expected: String,
        actual: String,
    },
    MissingRequiredField {
        field: String,
    },
    InvalidFieldValue {
        field: String,
        value: Value,
    },
    TerminalMonotonicityViolation {
        terminal_topic: String,
        business_topic: String,
    },
    DuplicateTerminalEvent {
        topic: String,
    },
    BusinessEventAfterCompletion {
        topic: String,
    },
    /// Topic is not in the whitelist of known topics (R9).
    /// Rejected without retry — only writes a recovery signal (R10).
    InvalidTopicFormat {
        topic: String,
        allowed_topics: Vec<String>,
    },
    /// Event matched a topic-deny rule (hat_id + topic exact match).
    /// The hat is explicitly forbidden from publishing this topic.
    TopicDenied {
        rule_hat: String,
        rule_topic: String,
    },
    /// U1 (2026-06-17-003 plan): semantic gate violation. The event
    /// passed schema validation but violates an orchestrator-level
    /// invariant (e.g. `review.passed` while a review wave is still
    /// open). Distinct from `InvalidFieldValue` because the payload
    /// itself is well-formed — the violation is in the **timing /
    /// state** relative to other events tracked by
    /// `ReviewStepTracker`. Kept fail-closed (event does NOT enter
    /// the bus) but loop continues — see
    /// `is_recoverable_policy_finding` for the bucket mapping.
    /// U3 (2026-07-22-004 plan): this variant ALSO covers same-payload
    /// consistency gates, identified by the `payload_consistency:<rule_id>`
    /// gate prefix — distinct from timing/state gates because the
    /// violation is internal to the current payload (no event history).
    ///
    /// U2 (2026-07-23-002 plan, KTD2): `referenced_fields` is the
    /// stable, declaration-order set of business fields the rule's
    /// predicate AST references. For timing/state gates (e.g.
    /// `review_passed_while_wave_open`) it is empty because the
    /// violation is not field-scoped. For payload-consistency gates
    /// it carries every `field:` appearing in the rule's `when`
    /// AST, deduplicated by first-occurrence. It is **not** the
    /// short-circuited "matched" subset — it is the static set the
    /// rule author declared. Agent repair tooling reads this list
    /// to know which payload fields to inspect, and never parses
    /// `message` to recover them.
    SemanticGateViolation {
        gate: String,
        context: String,
        referenced_fields: Vec<String>,
    },
    /// U4 (2026-06-17-003 plan): duplicate `work.done` for the
    /// same `(plan_name, step, task_id)` tuple. The `hint`
    /// distinguishes:
    ///   - `duplicate_stall_bypass`: the current event carries
    ///     `wave_id` (wave is still open) or is part of a stall
    ///     recovery flow — the agent is trying to re-send
    ///     `work.done` to bypass a stalled review cycle.
    ///   - `duplicate_same_step`: pure same-step re-emit (fix-round
    ///     did not advance, or the agent is not following the
    ///     `fix.applied` → re-`work.done` contract).
    DuplicateWorkDone {
        key: String,
        hint: DuplicateWorkDoneHint,
        /// U5 of plan 2026-07-05-005 (R8): dedup hit counter for
        /// `work.ready` storms. `None` for non-counter dedup lanes.
        seen_count: Option<u32>,
    },
}

/// U4 (2026-06-17-003 plan): hint carried in
/// [`ViolationType::DuplicateWorkDone`]. Lets the runner pick the
/// correct recovery payload (stall-bypass has a different message
/// from pure duplicate-same-step).
///
/// 2026-07-04-024019 run P0-1: added `ReviewDimensionDuplicate` so the
/// runtime can distinguish a `review.dimension.ready` collision from
/// a generic `work.done` collision in logs / dashboard / agent context.
/// `reason_code` is derived per-variant so dashboards see a stable
/// `duplicate_review_dimension_ready` rather than the misleading
/// generic `duplicate_work_done`.
///
/// U6 (plan 2026-07-04-004): added `ReviewDimensionsComplete` so the
/// `review.dimensions.complete` dedup branch (U2 carve-out) emits a
/// distinct reason code (`duplicate_review_dimensions_complete`)
/// rather than the misleading `duplicate_work_done`. Used in tandem
/// with `PolicyDecision::AcknowledgeAndForward` so dashboards can
/// tell the silent-success branch from a real policy violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateWorkDoneHint {
    /// Wave is open (event has `wave_id` set) or the agent is
    /// known to be in a stall-recovery flow. The re-emit is
    /// likely an attempt to bypass the stalled review cycle.
    DuplicateStallBypass,
    /// Pure same-step re-emit: no wave open, no stall recovery in
    /// progress. The agent simply re-emitted `work.done` for a
    /// step that has already closed.
    DuplicateSameStep,
    /// 2026-07-04-024019 run P0-1: `review.dimension.ready` already
    /// accepted for the same `(plan_name, step, task_id, dimension)`
    /// tuple. Distinct from `DuplicateSameStep` so dashboards and
    /// agents can recognize that the collision is in the review-
    /// coordinator's serial-walk lane, not the unit-execution lane.
    ReviewDimensionDuplicate,
    /// U6 (plan 2026-07-04-004): `review.dimensions.complete` already
    /// accepted for the same `(plan_name, step, task_id, fix_round)`
    /// tuple. Distinct from `DuplicateSameStep` /
    /// `ReviewDimensionDuplicate` so dashboards and agents can
    /// recognize the silent-success lane. Pairs with U2's
    /// `PolicyDecision::AcknowledgeAndForward` so the duplicate
    /// reaches the bus without a `task.resume` storm.
    ReviewDimensionsComplete,
}

impl DuplicateWorkDoneHint {
    /// U3 of plan 2026-07-05-005 (fix-plan §R3 / §R4): stable
    /// snake_case string for the `RecoveryDiagnosisEnvelope::hint`
    /// field. The `reason_code` itself stays the legacy literal
    /// `"duplicate_work_done"` (per KTD-3 — do not break dashboards
    /// that pin on that literal); the hint carries the variant
    /// distinction at the envelope top level so `ralph diagnose` can
    /// render the difference without parsing `reason_code`.
    pub fn as_hint_str(&self) -> &'static str {
        match self {
            // U3 fix-plan §R3: hint strings are the legacy
            // discriminator literals (`duplicate_work_done_same_step`
            // / `duplicate_work_done_stall_bypass`) so dashboards and
            // CLI precheck JSON that previously matched these
            // strings continue to work after the reason_code
            // collapse.
            DuplicateWorkDoneHint::DuplicateStallBypass => "duplicate_work_done_stall_bypass",
            DuplicateWorkDoneHint::DuplicateSameStep => "duplicate_work_done_same_step",
            DuplicateWorkDoneHint::ReviewDimensionDuplicate => "duplicate_review_dimension_ready",
            DuplicateWorkDoneHint::ReviewDimensionsComplete => {
                "duplicate_review_dimensions_complete"
            }
        }
    }
}

impl ViolationType {
    /// Field name that triggered the violation, when the violation
    /// is field-scoped. Returns `None` for topic-scoped violations
    /// (terminal-monotonicity, topic-format, topic-deny, etc.) and
    /// for semantic-gate violations (which use `gate` instead).
    pub fn field(&self) -> Option<&str> {
        match self {
            Self::MissingRequiredField { field } | Self::InvalidFieldValue { field, .. } => {
                Some(field.as_str())
            }
            _ => None,
        }
    }

    /// Stable machine-readable code for the violation type. Used as
    /// the dedupe key in dedup-by-`(topic, field, reason_code)` and
    /// as the `reason_code` in CLI precheck JSON output.
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::MissingRequiredField { .. } => "missing_required_field",
            Self::InvalidFieldValue { .. } => "invalid_field_value",
            Self::PayloadTypeMismatch { .. } => "payload_type_mismatch",
            Self::TerminalMonotonicityViolation { .. } => "terminal_monotonicity_violation",
            Self::DuplicateTerminalEvent { .. } => "duplicate_terminal_event",
            Self::BusinessEventAfterCompletion { .. } => "business_event_after_completion",
            Self::InvalidTopicFormat { .. } => "invalid_topic_format",
            Self::TopicDenied { .. } => "topic_denied",
            Self::SemanticGateViolation { .. } => "semantic_gate_violation",
            Self::DuplicateWorkDone { hint, .. } => match hint {
                // 2026-07-04-024019 run P0-1: surface the review-dim
                // collision under its own code so dashboards /
                // agents don't confuse it with `duplicate_work_done`.
                DuplicateWorkDoneHint::ReviewDimensionDuplicate => {
                    "duplicate_review_dimension_ready"
                }
                // U6 (plan 2026-07-04-004): split off
                // `ReviewDimensionsComplete` so the U2 silent-success
                // carve-out has a distinct stable code. Dashboards
                // / pin tests can match this literal without
                // mistaking it for a generic `duplicate_work_done`.
                DuplicateWorkDoneHint::ReviewDimensionsComplete => {
                    "duplicate_review_dimensions_complete"
                }
                // U3 of plan 2026-07-05-005 (fix-plan §R3): restore
                // the stable external contract per KTD-3 — single
                // `duplicate_work_done` reason_code for the
                // `DuplicateSameStep` and `DuplicateStallBypass`
                // variants. The `hint` field on
                // `RecoveryDiagnosisEnvelope` carries the
                // discriminator (`duplicate_work_done_same_step` /
                // `duplicate_work_done_stall_bypass`) so post-mortem
                // tooling can still distinguish the two paths.
                DuplicateWorkDoneHint::DuplicateStallBypass
                | DuplicateWorkDoneHint::DuplicateSameStep => "duplicate_work_done",
            },
        }
    }
}

/// A single policy finding.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyFinding {
    pub topic: String,
    pub violation_type: ViolationType,
    pub message: String,
    /// U2 (plan 2026-08-06-001): structured evidence the
    /// finding carries.  Populated by the consistency /
    /// precheck paths so the correction prompt and the CLI
    /// `--policy-check` JSON share one source of observed
    /// facts / violated invariant / required proof.  `None`
    /// for findings that do not have evidence to surface
    /// (legacy / diagnosis-fallback).  Defaults to `None`
    /// for back-compat with callers that build `PolicyFinding`
    /// via struct literal.
    pub evidence: Option<crate::correction::EvidenceDetail>,
}

impl PolicyFinding {
    /// Convenience: back-compat shim for callers / tests that
    /// construct `PolicyFinding` from the original 3-field shape.
    /// Existing call sites migrate at their own pace; new code
    /// should prefer the explicit `evidence` field.
    pub fn legacy(
        topic: impl Into<String>,
        violation_type: ViolationType,
        message: impl Into<String>,
    ) -> Self {
        Self {
            topic: topic.into(),
            violation_type,
            message: message.into(),
            evidence: None,
        }
    }
}

/// Decision from policy validation.
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyDecision {
    Accept,
    Warn(Vec<PolicyFinding>),
    RejectWithResume(PolicyFinding),
    Hold(PolicyFinding),
    /// U2 (plan 2026-07-04-004): event is acknowledged (logged,
    /// counted, mirrored into the dedup bucket) but **forwarded** to
    /// the bus **without** producing a `task.resume` recovery
    /// payload. Used by `review.dimensions.complete` dedup hits so
    /// the silent-success run doesn't drown the runtime in
    /// `task.resume` storms while still preserving the dedup
    /// invariant (a re-emit after `fix.applied` + new fix_round
    /// is the only legitimate path). The carried `PolicyFinding`
    /// is the same shape `RejectWithResume` would produce so
    /// dashboards / dashboards / dashboards continue to read it.
    AcknowledgeAndForward(PolicyFinding),
    /// Silently drop the event without publishing recovery or hold artifacts.
    Block(PolicyFinding),
    /// Silently ignore the event without recovery artifacts.
    /// Semantically equivalent to `Block`; used for explicit completion-guard ignore actions.
    Ignore(PolicyFinding),
}

/// Information about an event that was rejected by policy validation.
/// Used by the CLI runner to produce unified recovery diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyRejection {
    /// Topic of the rejected event.
    pub topic: String,
    /// Source hat from the JSONL event (hat field).
    pub source_hat: Option<String>,
    /// The policy finding describing the violation.
    pub finding: PolicyFinding,
    /// Unit 2 (2026-06-16-002 plan) recoverable-bucket. `Some(_)` means
    /// the rejection is in the **recoverable** set (R-B1) and the
    /// runner should publish a `task.resume` with `fix_hint` rather
    /// than the U6 fast-fail. `None` means the rejection is
    /// non-recoverable (R-B2): the existing U6
    /// `payload_contract_violation` path still applies.
    pub reason_class: Option<ReasonClass>,
}

/// Unit 2 (2026-06-16-002 plan): the **bucket** used to separate
/// recoverable policy rejections from non-recoverable ones, and the
/// per-key dimension for the bounded-retry counter at the loop
/// level.
///
/// Bucketing follows the plan's R-B1 / R-B2 table:
///
/// | Recoverable                              | Non-recoverable
/// |------------------------------------------|----------------------------------
/// | `PayloadTypeMismatch` (incl. non-JSON string) | `plan_name` / task key mismatch
/// | `MissingRequiredField`                   | duplicate terminal / completion guard
/// | `TopicDenied` (deny rules + isolated scope)   | `InvalidFieldValue` / `AllowedValueMismatch` (deferred)
/// | `SemanticGateViolation` (U1, 2026-06-17-003) | 4th attempt on the same (hat, reason_class)
///
/// The bucket is the last segment of the loop's
/// `record_rejection_key` counter so two distinct buckets on the
/// same `(hat, topic)` keep **independent** counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonClass {
    /// `PayloadTypeMismatch` — agent emitted a payload that does not
    /// match the schema's declared type. Recoverable on first 3 attempts.
    PayloadTypeMismatch,
    /// `MissingRequiredField` — agent omitted a required field.
    /// Recoverable on first 3 attempts.
    MissingRequiredField,
    /// `TopicDenied` — the (hat, topic) pair matched a deny rule.
    /// Recoverable on first 3 attempts.
    TopicDenied,
    /// U1 (2026-06-17-003 plan): `SemanticGateViolation` — event
    /// passed schema validation but violates an orchestrator-level
    /// invariant (e.g. `review.passed` while a review wave is still
    /// open). Recoverable on first 3 attempts AND bypasses the U6
    /// `PayloadContractViolation` fatal path. The bucket is its own
    /// dimension on the loop-level retry counter so semantic-gate
    /// rejections never compete with payload-typed rejections for
    /// the budget.
    SemanticGateViolation,
    /// U4 (2026-06-17-003 plan): duplicate `work.done` for the
    /// same `(plan_name, step, task_id)` tuple. The 2nd emit is
    /// rejected as recoverable (agent can re-emit with a different
    /// step or task_id, or wait for `fix.applied` / step close to
    /// re-send legitimately).
    DuplicateWorkDone,
}

impl ReasonClass {
    /// Stable snake_case label used in retry-key construction and
    /// operator-facing logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            ReasonClass::PayloadTypeMismatch => "payload_type_mismatch",
            ReasonClass::MissingRequiredField => "missing_required_field",
            ReasonClass::TopicDenied => "topic_denied",
            ReasonClass::SemanticGateViolation => "semantic_gate_violation",
            ReasonClass::DuplicateWorkDone => "duplicate_work_done",
        }
    }
}

/// Unit 2: pure mapping from a [`PolicyFinding`] to a recoverable
/// [`ReasonClass`]. Returns `Some(class)` when the finding is in the
/// recoverable set (R-B1), `None` otherwise.
pub fn is_recoverable_policy_finding(finding: &PolicyFinding) -> Option<ReasonClass> {
    match &finding.violation_type {
        ViolationType::PayloadTypeMismatch { .. } => Some(ReasonClass::PayloadTypeMismatch),
        ViolationType::MissingRequiredField { .. } => Some(ReasonClass::MissingRequiredField),
        ViolationType::TopicDenied { .. } => Some(ReasonClass::TopicDenied),
        // U1 (2026-06-17-003 plan): semantic gate violations are
        // recoverable on first 3 attempts AND bypass U6 fatal
        // termination (see runner.rs `TerminationReason::PayloadContractViolation`
        // branch). The bucket is independent of the other three
        // reason classes so a misbehaving coordinator can be
        // corrected via `task.resume` without exhausting the schema
        // validation budget for unrelated events.
        ViolationType::SemanticGateViolation { .. } => Some(ReasonClass::SemanticGateViolation),
        ViolationType::DuplicateWorkDone { .. } => Some(ReasonClass::DuplicateWorkDone),
        _ => None,
    }
}
