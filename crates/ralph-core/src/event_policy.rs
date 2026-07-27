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

/// Runtime state for policy validation across events.
#[derive(Debug, Default, Clone)]
pub struct PolicyRuntimeState {
    pub terminal_observed: bool,
    pub observed_topics: HashSet<String>,
    /// Whether a completion promise has been honored in this loop.
    pub completion_honored: bool,
    /// The topic that triggered the honored completion.
    pub completion_topic: Option<String>,
    /// The event index at which completion was honored.
    pub completion_event_index: Option<u64>,
    /// The iteration at which completion was honored.
    pub completion_iteration: Option<u32>,
    /// The current plan_name extracted from the most recent `work.ready` event.
    /// Used for plan_name equality validation (U4).
    pub current_plan_name: Option<String>,
    /// U4 (2026-06-17-003 plan): dedup set for `work.done` events.
    /// Key format: `{plan_name}::{step}::{task_id}`. Populated when
    /// a `work.done` is accepted by `validate_event_with_hat`;
    /// consumed by the event loop for per-batch pruning. The
    /// per-loop lifetime set lives in `LoopState::work_done_seen_tasks`
    /// (see `event_loop/loop_state.rs`); this set is the
    /// `PolicyRuntimeState` mirror used during `validate_event`
    /// for **in-batch** dedup (when the same `work.done` appears
    /// twice in the same `process_output` batch).
    pub work_done_seen_keys: HashSet<String>,
    /// U-fixes-2026-07-04: canonical `task_id` → `task_key`
    /// binding observed on the first accepted `work.done`.
    /// Used to surface `task_id_task_key_mismatch` BEFORE
    /// dedup so agent retry storms that swap `task_key` on
    /// re-emit get an actionable error (not a generic
    /// "duplicate"). Per-loop lifetime set; pruned on
    /// step boundaries alongside `work_done_seen_tasks`.
    pub work_done_task_id_to_key: HashMap<String, String>,
    /// U5 (2026-06-17-003 plan, R6): dedup set for
    /// `review.dimension.ready` events. Key format:
    /// `{plan_name}::{step}::{task_id}::{dimension}`. Populated
    /// when a `review.dimension.ready` is accepted by
    /// `validate_event_with_hat`; a 2nd emit with the same key
    /// is rejected as `DuplicateWorkDone` (variant reused —
    /// same retry-key semantics, smaller blast radius than
    /// introducing a new ViolationType). Mirrors the
    /// `work.done` dedup pattern: this is the in-batch mirror;
    /// the per-loop lifetime set is also populated in
    /// `from_events` for cross-batch replay.
    pub review_dimension_ready_seen_keys: HashSet<String>,
    /// U5 (2026-06-18-004 plan, R4, KTD3): dedup set for
    /// `review.dimensions.complete` events. Key format:
    /// `{plan_name}::{step}::{task_id}::{fix_round}`. Populated
    /// when a `review.dimensions.complete` is accepted by
    /// `validate_event_with_hat`; a 2nd emit with the same key
    /// is rejected as `DuplicateWorkDone`. The `fix_round`
    /// segment distinguishes re-review rounds so a
    /// `fix.applied`-pruned bucket allows a 2nd
    /// `review.dimensions.complete` to land for a new fix round
    /// without colliding with the 1st round's key. Defaults to
    /// `0` when the payload omits `fix_round` so legacy emitters
    /// still get deduped.
    pub review_dimensions_complete_seen_keys: HashSet<String>,
    /// 2026-06-24 P1-3: dedup map for `work.ready` events. Key
    /// format: `{plan_name}::{step}::{task_id}`. A 2nd
    /// `work.ready` with the same key (same task, same step) is
    /// rejected as `DuplicateWorkDone` so the agent stops
    /// re-announcing an already-started unit. Pruned on
    /// step-boundary events (`fix.applied` / step close) so a
    /// legitimate re-emit after a fix round is allowed.
    ///
    /// U5 of plan 2026-07-05-005 (R8): the value carries the
    /// dedup hit count so post-mortem tooling can distinguish
    /// a single duplicate from a "dup storm" (the same key
    /// re-emitted 50 times in a tight loop). The count is
    /// bumped on every observed hit; `fix.applied` pruning
    /// does NOT reset the counter (count is observation, not
    /// dedup state). Only the work.ready bucket is instrumented
    /// — the other 7 seen_keys fields stay as `HashSet<String>`
    /// to keep the change blast radius small (plan U5 §
    /// "scope-bounded").
    pub work_ready_seen_keys: HashMap<String, u32>,
    /// U5 of plan 2026-07-05-005 (fix-plan §R8): side-table
    /// recording which `work_ready_seen_keys` entries have
    /// had their `(plan_name, step)` bucket pruned. The bucket
    /// classification lives here, separate from the dedup
    /// count in `work_ready_seen_keys`, so a re-emit after
    /// `fix.applied` continues to increment the count without
    /// resetting it.
    pub pruned_work_ready_buckets: HashSet<String>,
    /// 2026-06-24 P1-3: dedup set for `test.passed` events. Key
    /// format: `{plan_name}::{step}::{task_id}::{fix_round}`.
    /// The `fix_round` segment distinguishes re-test rounds so
    /// a `fix.applied`-pruned bucket allows a 2nd `test.passed`
    /// to land for a new fix round without colliding with the
    /// prior round's entry. Missing `fix_round` falls through
    /// (mirrors `review.dimensions.complete` U6 KTD4 behavior)
    /// so the schema validator reports `missing_required_field`
    /// instead of hiding the failure behind `DuplicateWorkDone`.
    pub test_passed_seen_keys: HashSet<String>,
    /// 2026-06-24 P1-3: dedup set for `test.failed` events. Key
    /// format mirrors `test_passed_seen_keys`. Same fall-through
    /// rule for missing/non-numeric `fix_round`.
    pub test_failed_seen_keys: HashSet<String>,
    /// 2026-07-01-001 U1: dedup set for `review.start` events.
    /// Key format: `{plan_name}::{task_id}` when `step` is absent,
    /// `{plan_name}::{task_id}::{step}` when present. A 2nd emit
    /// with the same key is rejected as `DuplicateWorkDone` so the
    /// runtime stops a coordinator from starting multiple review
    /// sequences for the same plan/task. Pruned on `fix.applied`
    /// so a legitimate re-review after a fix round is allowed.
    pub review_start_seen_keys: HashSet<String>,
    /// 2026-07-02-004 U7 (R6): pending precheck candidate keys.
    /// Format: `{guarded_topic}::{payload}`. Populated when
    /// `<X>.proposed` is accepted; pruned when the gate emits
    /// `<X>` (pass) or `<X>.rejected` (fail) so a retry after
    /// rejection can re-emit the same payload.
    pub precheck_proposed_pending_keys: HashSet<String>,
    /// U7 of plan 2026-07-02-005: last accepted `plan.blocked.reason`
    /// for shipper strict-match runtime routing on `REVIEW_COMPLETE`.
    pub last_plan_blocked_reason: Option<String>,
}

/// Dedup key for a precheck `<X>.proposed` candidate (U7 / R6).
pub fn precheck_proposed_dedup_key(guarded_topic: &str, payload: &str) -> String {
    format!("{guarded_topic}::{}", payload.trim())
}

/// Build the dedup key for `review.start`.
///
/// U8 of plan 2026-07-02-005: prefer the semantic key
/// `(plan_name, fix_round, total_units)` when the payload
/// carries both. This is the 175407 root-cause fix: the
/// 2nd `review.start` had identical `plan_name + task_id + step`
/// but a different `triggered` value (e.g. `ralph` vs
/// `review-coordinator`); byte equality rejected the 1st
/// emit but the semantic-identity 2nd slipped through. The
/// semantic key is `triggered`-agnostic by construction.
///
/// When `fix_round` / `total_units` are absent from the
/// payload (legacy / pre-fix emits), fall back to the
/// pre-U8 `(plan_name, task_id [, step])` key to preserve
/// backward compatibility.
fn review_start_dedup_key(
    plan_name: &str,
    step: Option<&str>,
    task_id: &str,
    fix_round: Option<u32>,
    total_units: Option<u32>,
) -> String {
    match (fix_round, total_units) {
        (Some(fr), Some(tu)) => format!("{plan_name}::fr={fr}::tu={tu}"),
        _ => {
            if let Some(st) = step {
                format!("{plan_name}::{task_id}::{st}")
            } else {
                format!("{plan_name}::{task_id}")
            }
        }
    }
}

impl PolicyRuntimeState {
    /// U1 (2026-06-18-004 plan, R1, KTD1): prune every
    /// `review_dimension_ready_seen_keys` entry that belongs to a
    /// given `(plan_name, step, task_id)` bucket. Called when
    /// `fix.applied` is policy-accepted so that
    /// `review-coordinator` can legally re-emit
    /// `review.dimension.ready` for the same `(plan, step, task)`
    /// in a new fix round (the original dedup key lacks
    /// `fix_round`, so without this prune a fix → re-review
    /// attempt always gets `DuplicateWorkDone` — this is the
    /// root cause of the perky-maple P1-3 / P2-5 spiral).
    ///
    /// The companion `LoopState::prune_work_done_bucket` (callers
    /// in `event_loop/mod.rs`) handles the per-loop lifetime
    /// mirror; this method only touches the in-batch
    /// `PolicyRuntimeState` mirror. Both must be pruned
    /// together at the `fix.applied` accept site.
    pub fn prune_review_dimension_ready_bucket(
        &mut self,
        plan_name: &str,
        step: &str,
        task_id: &str,
    ) {
        let prefix = format!("{plan_name}::{step}::{task_id}::");
        self.review_dimension_ready_seen_keys
            .retain(|key| !key.starts_with(&prefix));
    }

    /// U1 (2026-06-18-004 plan, KTD1, symmetry fix): mirror of
    /// `LoopState::prune_work_done_bucket` for the
    /// `PolicyRuntimeState::work_done_seen_keys` mirror. Prior
    /// to this addition the in-batch mirror was never pruned on
    /// step-boundary events, leaving a 1-batch stale window
    /// after `queue.advance` / `review.failed` / `fix.applied`
    /// where a re-emit would still be rejected by
    /// `validate_event_with_hat`. Always pair with the
    /// `LoopState::prune_work_done_bucket` call at the accept
    /// site.
    pub fn prune_work_done_bucket(&mut self, plan_name: &str, step: &str) {
        let prefix = format!("{plan_name}::{step}::");
        self.work_done_seen_keys
            .retain(|key| !key.starts_with(&prefix));
        // U-fixes-2026-07-04: step boundary invalidates every
        // (task_id, task_key) binding too — task_ids from a
        // closed step can be re-minted under a new task_key in
        // the next step, so keeping stale bindings would
        // produce false `task_id_task_key_mismatch` rejections.
        self.work_done_task_id_to_key.clear();
    }

    /// U5 (2026-06-18-004 plan, R4): prune every
    /// `review_dimensions_complete_seen_keys` entry that
    /// belongs to a given `(plan_name, step, task_id)` bucket
    /// across ALL `fix_round` values. Called when `fix.applied`
    /// is policy-accepted so that the next round's
    /// `review.dimensions.complete` (carrying `fix_round=N+1`)
    /// lands without colliding with the previous round's
    /// `fix_round=N` entry. The implementation deliberately
    /// does NOT scope the prune to a single `fix_round` —
    /// scoping would require re-doing the dedup key for every
    /// possible round, and the per-task bucket is small
    /// enough (4 dims × at most a handful of rounds) that
    /// over-pruning has no observable blast radius.
    pub fn prune_review_dimensions_complete_bucket(
        &mut self,
        plan_name: &str,
        step: &str,
        task_id: &str,
    ) {
        let prefix = format!("{plan_name}::{step}::{task_id}::");
        self.review_dimensions_complete_seen_keys
            .retain(|key| !key.starts_with(&prefix));
    }

    /// 2026-06-24 P1-3: prune the `work_ready_seen_keys` entries
    /// that belong to a given `(plan_name, step)` bucket. Called
    /// on `fix.applied` / step close so a legitimate re-emit
    /// after a fix round is allowed. Mirrors
    /// `prune_work_done_bucket` (same key shape).
    ///
    /// U5 of plan 2026-07-05-005 (fix-plan §R8 / §C5): the dedup
    /// hit counter is **preserved** across pruning — the count is
    /// observation, not dedup state, so losing it would hide
    /// legitimate dup-storm signals. We achieve this by:
    ///
    /// 1. **Not** removing the pruned entries from
    ///    `work_ready_seen_keys` (the HashMap value carries the
    ///    running count and must survive the prune).
    /// 2. Carrying the bucket classification in a separate
    ///    `pruned_work_ready_buckets: HashSet<String>` side-table
    ///    so the dedup validator can recognise "this key is
    ///    bucket-pruned but the count is still real".
    ///
    /// On the next `work.ready` emit, `validate_event_with_hat`
    /// sees `pruned_work_ready_buckets.contains(&key)` and
    /// increments `work_ready_seen_keys[key]` (no reset to 1).
    pub fn prune_work_ready_bucket(&mut self, plan_name: &str, step: &str) {
        let prefix = format!("{plan_name}::{step}::");
        // Record the bucket as pruned. We intentionally do NOT
        // remove the dedup entries — their counts survive.
        for key in self.work_ready_seen_keys.keys() {
            if key.starts_with(&prefix) {
                self.pruned_work_ready_buckets.insert(key.clone());
            }
        }
    }

    /// 2026-06-24 P1-3: prune the `test_passed_seen_keys` /
    /// `test_failed_seen_keys` entries that belong to a given
    /// `(plan_name, step, task_id)` bucket across ALL
    /// `fix_round` values. Called when `fix.applied` is
    /// policy-accepted so the next round's `test.passed` /
    /// `test.failed` (carrying `fix_round=N+1`) lands without
    /// colliding with the previous round's entry. Mirrors
    /// `prune_review_dimensions_complete_bucket`.
    pub fn prune_test_result_buckets(&mut self, plan_name: &str, step: &str, task_id: &str) {
        let prefix = format!("{plan_name}::{step}::{task_id}::");
        self.test_passed_seen_keys
            .retain(|key| !key.starts_with(&prefix));
        self.test_failed_seen_keys
            .retain(|key| !key.starts_with(&prefix));
    }

    /// 2026-07-02-004 U7: drop pending precheck candidates for
    /// guarded topic `X` after the gate emits `<X>` or
    /// `<X>.rejected`.
    pub fn prune_precheck_proposed_bucket(&mut self, guarded_topic: &str) {
        let prefix = format!("{guarded_topic}::");
        self.precheck_proposed_pending_keys
            .retain(|key| !key.starts_with(&prefix));
    }

    /// 2026-07-01-001 U1: prune every `review_start_seen_keys`
    /// entry that belongs to a given `(plan_name, task_id)` bucket,
    /// including keys that carry an optional `step` suffix. Called
    /// when `fix.applied` is policy-accepted so that a coordinator
    /// can legally start a fresh review round after fixes land.
    pub fn prune_review_start_bucket(&mut self, plan_name: &str, task_id: &str) {
        let base = format!("{plan_name}::{task_id}");
        self.review_start_seen_keys
            .retain(|key| !(key == &base || key.starts_with(&format!("{base}::"))));
    }

    /// Replays events from a JSONL file to build up the policy runtime state.
    ///
    /// Reads all events from the file, tracking which terminal topics have been
    /// observed and which business topics have been seen. Malformed lines are
    /// skipped. String, object, and null payloads are all handled with the same
    /// compatibility semantics as `EventReader`.
    ///
    /// Also extracts `current_plan_name` from the most recent `work.ready` event,
    /// used by the plan_name equality guard (U4).
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or read.
    pub fn from_events(
        events_path: impl AsRef<std::path::Path>,
        policy: &EventPolicyConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut reader = EventReader::new(events_path.as_ref());
        let result = reader.read_new_events()?;

        let mut state = Self::default();
        for event in result.events {
            state.observed_topics.insert(event.topic.clone());
            if policy.terminal_topics.contains(&event.topic) {
                state.terminal_observed = true;
            }
            // U4: Extract current_plan_name from work.ready events
            if event.topic == "work.ready"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
                && let Some(name) = obj.get("plan_name").and_then(|v| v.as_str())
            {
                state.current_plan_name = Some(name.to_string());
            }
            // U5 (2026-06-17-003 plan, R6): replay prior
            // `review.dimension.ready` events to populate the
            // dedup set so cross-batch re-emits (e.g. on loop
            // restart or in a new process_output batch) are
            // still rejected. The key shape matches the
            // in-batch check: `{plan_name}::{step}::{task_id}::{dimension}`.
            if event.topic == "review.dimension.ready"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                let dimension = obj.get("dimension").and_then(|v| v.as_str());
                if let (Some(pn), Some(st), Some(ti), Some(dim)) =
                    (plan_name, step, task_id, dimension)
                {
                    state
                        .review_dimension_ready_seen_keys
                        .insert(format!("{pn}::{st}::{ti}::{dim}"));
                }
            }
            // 2026-07-01-001 U1: replay prior `review.start` events
            // so a loop restart or new `process_output` batch does
            // not accept a duplicate review kick-off for the same
            // `(plan_name, task_id)`.
            if event.topic == "review.start"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                let fix_round = obj
                    .get("fix_round")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32);
                let total_units = obj
                    .get("total_units")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32);
                if let (Some(pn), Some(ti)) = (plan_name, task_id) {
                    state.review_start_seen_keys.insert(review_start_dedup_key(
                        pn,
                        step,
                        ti,
                        fix_round,
                        total_units,
                    ));
                }
            }
            // U1 (2026-06-18-004 plan, KTD1): replay prior
            // `work.done` events so the in-batch mirror mirrors
            // `LoopState::work_done_seen_tasks`. Without this,
            // the very next `process_output` batch after a
            // loop rehydrate would accept a duplicate `work.done`
            // for the same `(plan, step, task)` because
            // `validate_event_with_hat` only consults
            // `PolicyRuntimeState::work_done_seen_keys`.
            if event.topic == "work.done"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                let task_key = obj.get("task_key").and_then(|v| v.as_str());
                if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
                    state
                        .work_done_seen_keys
                        .insert(format!("{pn}::{st}::{ti}"));
                    // U-fixes-2026-07-04: mirror (task_id) →
                    // task_key binding so rehydrate produces
                    // the same task_id_task_key_mismatch
                    // detection as the live accept path.
                    if let Some(tk) = task_key {
                        state
                            .work_done_task_id_to_key
                            .insert(ti.to_string(), tk.to_string());
                    }
                }
            }
            // 2026-07-02-004 U7: replay precheck gate lifecycle.
            if let Some(guarded) = event.topic.strip_suffix(".rejected") {
                state.prune_precheck_proposed_bucket(guarded);
            } else if event.topic.ends_with(".proposed")
                && let Some(p) = event.payload.as_deref()
            {
                let guarded = event
                    .topic
                    .strip_suffix(".proposed")
                    .unwrap_or(event.topic.as_str());
                state
                    .precheck_proposed_pending_keys
                    .insert(precheck_proposed_dedup_key(guarded, p));
            } else if !event.topic.ends_with(".proposed") {
                state.prune_precheck_proposed_bucket(&event.topic);
            }
            if event.topic == "plan.blocked" {
                state.last_plan_blocked_reason =
                    crate::shipper_reason::extract_plan_blocked_reason(event.payload.as_deref());
            } else if event.topic == "plan.complete" {
                state.last_plan_blocked_reason = None;
            }
            // U1 (2026-06-18-004 plan, KTD1, symmetry fix):
            // when a `fix.applied` is replayed, also prune the
            // `(plan, step, task)` bucket for both
            // `review_dimension_ready_seen_keys` and
            // `work_done_seen_keys` mirrors. This is the
            // `from_events` analog of the live accept-site
            // pruning in `event_loop/mod.rs` — both paths must
            // execute the same prune or loop rehydrate would
            // re-introduce the perky-maple P1-3 dedup block.
            if event.topic == "fix.applied"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
                    state.prune_review_dimension_ready_bucket(pn, st, ti);
                    state.prune_work_done_bucket(pn, st);
                    // U5 (2026-06-18-004 plan, R4):
                    // also prune the
                    // `review.dimensions.complete`
                    // bucket so the next round's
                    // `review.dimensions.complete`
                    // with `fix_round=N+1` lands
                    // without colliding with the
                    // prior round's
                    // `fix_round=N` entry.
                    state.prune_review_dimensions_complete_bucket(pn, st, ti);
                    // 2026-06-24 P1-3: prune the new
                    // `work.ready` / `test.passed` /
                    // `test.failed` buckets so the next
                    // round's emits land without colliding
                    // with the prior round's entries.
                    state.prune_work_ready_bucket(pn, st);
                    state.prune_test_result_buckets(pn, st, ti);
                    // 2026-07-01-001 U1: prune `review.start`
                    // so a coordinator can start a fresh review
                    // sequence after fixes land.
                    state.prune_review_start_bucket(pn, ti);
                }
            }
            // U5 (2026-06-18-004 plan, R4): replay prior
            // `review.dimensions.complete` events so the
            // in-batch mirror reflects the dedup key shape
            // `{plan}::{step}::{task}::{fix_round}`. Missing
            // `fix_round` defaults to `0` so legacy emitters
            // are deduped against the same key the live
            // accept site would record.
            if event.topic == "review.dimensions.complete"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                let fix_round = obj.get("fix_round").and_then(|v| v.as_u64()).unwrap_or(0);
                if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
                    state
                        .review_dimensions_complete_seen_keys
                        .insert(format!("{pn}::{st}::{ti}::{fix_round}"));
                }
            }
            // 2026-06-24 P1-3: replay prior `work.ready` events
            // so the in-batch mirror reflects the dedup key
            // shape `{plan}::{step}::{task_id}`. Without this,
            // the very next `process_output` batch after a loop
            // rehydrate would accept a duplicate `work.ready`
            // for the same `(plan, step, task)`.
            if event.topic == "work.ready"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
                    // U5 of plan 2026-07-05-005 (R8): bump the
                    // per-key counter on every replayed hit so
                    // cross-loop resume keeps the dup-storm
                    // signal consistent with the in-memory view.
                    let key = format!("{pn}::{st}::{ti}");
                    let entry = state.work_ready_seen_keys.entry(key).or_insert(0);
                    *entry = entry.saturating_add(1);
                }
            }
            // 2026-06-24 P1-3: replay prior `test.passed` /
            // `test.failed` events so the in-batch mirror
            // reflects the dedup key shape
            // `{plan}::{step}::{task_id}::{fix_round}`. Missing
            // or non-numeric `fix_round` falls through (mirrors
            // the live accept-site U6 KTD4 rule) so the schema
            // validator reports the precise error on rehydrate.
            if (event.topic == "test.passed" || event.topic == "test.failed")
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                let fix_round = match obj.get("fix_round") {
                    Some(Value::Number(n)) => n.as_u64(),
                    _ => None,
                };
                if let (Some(pn), Some(st), Some(ti), Some(fr)) =
                    (plan_name, step, task_id, fix_round)
                {
                    let key = format!("{pn}::{st}::{ti}::{fr}");
                    if event.topic == "test.passed" {
                        state.test_passed_seen_keys.insert(key);
                    } else {
                        state.test_failed_seen_keys.insert(key);
                    }
                }
            }
            // U1 (2026-06-18-004 plan, KTD1, symmetry fix):
            // `queue.advance` and `review.failed` are the other
            // step-boundary events that should clear the
            // work_done mirror on rehydrate (matches the live
            // accept-site behavior).
            if (event.topic == "queue.advance" || event.topic == "review.failed")
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj
                    .get("completed_step")
                    .or_else(|| obj.get("step"))
                    .and_then(|v| v.as_str());
                if let (Some(pn), Some(st)) = (plan_name, step) {
                    state.prune_work_done_bucket(pn, st);
                    // 2026-06-24 P1-3: mirror the live
                    // accept-site behavior for `work.ready`.
                    state.prune_work_ready_bucket(pn, st);
                }
            }
        }
        Ok(state)
    }

    /// Parse an event payload string into an owned JSON object map.
    ///
    /// Returns `Some(map)` only when the payload is a valid JSON object
    /// (i.e. `{...}`). String payloads, null, arrays, and malformed
    /// JSON all return `None`. The map is owned because
    /// `serde_json::from_str` produces owned `Value`s — we cannot
    /// borrow into the transient `Value` while the caller lives.
    /// 2026-06-18-006 plan U7 (R7, KTD3): collapses six near-identical
    /// payload-parsing blocks in `from_events` into one helper.
    fn payload_object(payload: Option<&str>) -> Option<serde_json::Map<String, Value>> {
        let p = payload?;
        let val = serde_json::from_str::<Value>(p).ok()?;
        if let Value::Object(obj) = val {
            Some(obj)
        } else {
            None
        }
    }
}

/// Check if an event should be handled differently because a completion promise
/// has already been honored in this loop.
///
/// When `state.completion_honored` is true, subsequent terminal events and
/// business events are subject to the `completion_after_terminal` configuration.
/// Non-terminal/non-business events pass through unchanged.
pub fn check_completion_honored(
    topic: &str,
    config: &EventPolicyConfig,
    state: &PolicyRuntimeState,
) -> Option<PolicyDecision> {
    check_completion_guard(topic, config, state.completion_honored)
}

/// Check if an event should be guarded when a completion signal has been seen.
///
/// This is the core logic used both for persistent `completion_honored` state
/// and for per-batch same-batch guarding.
pub fn check_completion_guard(
    topic: &str,
    config: &EventPolicyConfig,
    guard_active: bool,
) -> Option<PolicyDecision> {
    if !guard_active {
        return None;
    }

    if config.terminal_topics.contains(&topic.to_string()) {
        Some(apply_completion_after_terminal_action(
            &config.completion_after_terminal.duplicate_terminal,
            topic,
            ViolationType::DuplicateTerminalEvent {
                topic: topic.to_string(),
            },
        ))
    } else if config.business_topics.contains(&topic.to_string()) {
        Some(apply_completion_after_terminal_action(
            &config.completion_after_terminal.business_after_completion,
            topic,
            ViolationType::BusinessEventAfterCompletion {
                topic: topic.to_string(),
            },
        ))
    } else {
        None
    }
}

fn apply_completion_after_terminal_action(
    action: &CompletionAfterTerminalAction,
    topic: &str,
    violation_type: ViolationType,
) -> PolicyDecision {
    let finding = PolicyFinding {
        topic: topic.to_string(),
        violation_type,
        message: format!("Event '{}' arrived after completion was honored", topic),
    };

    match action {
        CompletionAfterTerminalAction::Reject => PolicyDecision::Block(finding),
        CompletionAfterTerminalAction::Ignore => PolicyDecision::Ignore(finding),
        CompletionAfterTerminalAction::Warn => PolicyDecision::Warn(vec![finding]),
    }
}

/// R9: Check topic format against the whitelist of known topics.
///
/// Rejects topics not in the whitelist **before** payload schema validation.
/// Rejection is non-retryable — only writes a recovery signal (R10), no
/// `task.resume` is emitted.
///
/// The whitelist is built from:
/// - All hat `publishes` topics (from hat registry)
/// - System/control topics (`event.*`, `human.*`, `loop.cancel`, `task.resume`,
///   `build.task.abandoned`, completion promise)
///
/// Returns `None` if the topic is valid (accepted), or `Some(PolicyDecision::Block(...))`
/// if the topic is not in the whitelist.
/// 2026-07-06-004 plan U8: handoff envelope validator. When
/// `event_loop.handoff_envelope.validate_payload` is true, every
/// event whose payload parses as a JSON object must contain a
/// valid `payload.handoff_envelope` (per `handoff_envelope.v1`).
/// Returns `Some(PolicyFinding)` on failure, `None` on success.
/// The validator delegates to `handoff_envelope::validate_handoff_envelope_payload`
/// so the (code, message) error envelope is shared between the
/// prompt-injection path and the policy-check pipeline.
pub fn check_handoff_envelope(topic: &str, payload: &Value) -> Option<PolicyFinding> {
    use crate::handoff_envelope;
    // U3 (2026-07-06-004 fix-plan): callers in the CLI
    // boundary cannot construct a `HatRegistry` from inside
    // `check_handoff_envelope` (the registry lives on the
    // pipeline, not on the policy), so the registry check is
    // performed by callers that hold a registry reference
    // (production path: `EventPolicyRule::validate` in
    // `validation/rules_event_policy.rs`). The policy gate
    // here keeps the no-registry shape for parity with
    // pre-fix callers.
    match handoff_envelope::validate_handoff_envelope_payload(payload, None) {
        Ok(_) => None,
        Err(err) => Some(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::MissingRequiredField {
                field: "handoff_envelope".to_string(),
            },
            message: format!("handoff_envelope validation failed: {}", err),
        }),
    }
}

/// 2026-07-06-004 plan U8: in-process gating helper. Returns
/// true iff the policy-check pipeline should run
/// `check_handoff_envelope` for the supplied payload. The
/// condition is exactly `handoff_config.enabled &&
/// handoff_config.validate_payload && payload.is_some()`.
// 2026-07-16 cleanup U4 (KTD-3): reserved for U15 / future
// policy-check parity; pinning the public signature now avoids
// churn when downstream consumers start importing it.
#[allow(dead_code)]
pub fn handoff_envelope_validation_enabled<H: HandoffEnvelopeConfigAccess>(
    payload: Option<&str>,
    handoff_config: &H,
) -> bool {
    handoff_config.handoff_envelope_enabled()
        && handoff_config.handoff_envelope_validate_payload()
        && payload.is_some()
}

/// 2026-07-06-004 plan U8: typed adapter that bridges
/// `EventLoopConfig.handoff_envelope` into the policy pipeline
/// via the `HandoffEnvelopeConfigAccess` trait. Used by the
/// `ralph emit --policy-check` path once U10 wires the real
/// config in.
pub struct EventLoopHandoffConfig<'a> {
    pub handoff_envelope: &'a crate::config::HandoffEnvelopeConfig,
}

impl HandoffEnvelopeConfigAccess for EventLoopHandoffConfig<'_> {
    fn handoff_envelope_enabled(&self) -> bool {
        self.handoff_envelope.enabled
    }
    fn handoff_envelope_validate_payload(&self) -> bool {
        self.handoff_envelope.validate_payload
    }
}

/// - System/control topics (`event.*`, `human.*`, `loop.cancel`, `task.resume`,
///   `build.task.abandoned`, completion promise)
///
/// Returns `None` if the topic is valid (accepted), or `Some(PolicyDecision::Block(...))`
/// if the topic is not in the whitelist.
pub fn check_topic_format(topic: &str, allowed_topics: &HashSet<String>) -> Option<PolicyDecision> {
    if allowed_topics.contains(topic) {
        return None;
    }

    // R6 (2026-06-17-004 plan): make the diagnostic list deterministic.
    // `HashSet` iteration order is undefined, so sort before serialising
    // into the finding/message to keep regression snapshots stable.
    let mut allowed_list: Vec<String> = allowed_topics.iter().cloned().collect();
    allowed_list.sort();

    let finding = PolicyFinding {
        topic: topic.to_string(),
        violation_type: ViolationType::InvalidTopicFormat {
            topic: topic.to_string(),
            allowed_topics: allowed_list.clone(),
        },
        message: format!(
            "Topic '{}' is not in the whitelist of known topics. \
             Valid topics: {:?}",
            topic, allowed_list
        ),
    };

    // R10: Block (not RejectWithResume) — no retry, only recovery signal
    Some(PolicyDecision::Block(finding))
}

/// Build the set of allowed topics from hat configs and system control topics.
///
/// Includes:
/// - All hat `publishes` topics (what hats emit)
/// - All hat `triggers` topics (what activates hats)
/// - Event policy `terminal_topics` and `business_topics` (if configured)
/// - System control topics: `loop.cancel`, `task.resume`, `build.task.abandoned`,
///   completion promise
///
/// Note: `event.*` and `human.*` topics are NOT stored here as prefixes.
/// They are allowed by the `is_system_topic()` check which is applied
/// BEFORE `check_topic_format` in the event loop validation flow.
pub fn build_allowed_topics(
    hats: &std::collections::HashMap<String, crate::config::HatConfig>,
    completion_promise: &str,
    event_policy: Option<&EventPolicyConfig>,
) -> HashSet<String> {
    let mut allowed = HashSet::new();

    // Add all hat publishes and triggers topics
    for hat_config in hats.values() {
        for topic in &hat_config.publishes {
            allowed.insert(topic.clone());
        }
        for topic in &hat_config.triggers {
            allowed.insert(topic.clone());
        }
    }

    // Add event policy terminal and business topics
    if let Some(policy) = event_policy {
        for topic in &policy.terminal_topics {
            allowed.insert(topic.clone());
        }
        for topic in &policy.business_topics {
            allowed.insert(topic.clone());
        }
    }

    // System/control topics (exact match)
    allowed.insert("loop.cancel".to_string());
    allowed.insert("task.resume".to_string());
    allowed.insert("build.task.abandoned".to_string());
    allowed.insert(completion_promise.to_string());

    // Note: event.* and human.* topics are handled by is_system_topic() check
    // (tested BEFORE check_topic_format in the event loop), not by prefix
    // matching in this set. The comment above about "stored as actual prefixes"
    // was incorrect - they are not inserted here.

    allowed
}

/// Check if a topic matches a system/control prefix pattern.
///
/// System topics start with `event.` or `human.` and are always allowed
/// regardless of the whitelist. This check is applied BEFORE
/// check_topic_format in the event loop.
pub fn is_system_topic(topic: &str) -> bool {
    topic.starts_with("event.") || topic.starts_with("human.")
}

/// Check if a topic is a system control topic the loop runner
/// itself publishes (`loop.cancel`, `task.resume`,
/// `build.task.abandoned`).
///
/// Unlike [`is_system_topic`], this matches exact topic
/// strings rather than `event.*` / `human.*` prefixes.  The
/// unified validation pipeline calls
/// `check_topic_deny_rules` after `is_system_topic` has
/// already admitted the prefix-matched topics; this
/// short-circuit covers the remaining runner-emitted topics
/// so a deny rule that happens to match the originating hat
/// cannot reject a recovery injection.  See
/// `check_topic_deny_rules` for the regression that motivated
/// the helper.
pub fn is_system_control_topic(topic: &str) -> bool {
    matches!(
        topic,
        "loop.cancel" | "task.resume" | "build.task.abandoned"
    )
}

/// WAC-U7 (2026-06-12-002) R10: hard-reject topics for which a
/// null payload is never acceptable. Any event whose topic is in
/// this set and whose payload is `None` is rejected with
/// `RejectWithResume` regardless of `EventPolicyMode::Observe`.
/// The list is the minimum required by R10; it is intentionally
/// not configurable so the operational contract is uniform
/// across presets.
///
/// Step-handoff (2026-06-17-002) U5: extended with `work.ready`,
/// `plan.complete`, `plan.blocked` so the hard gate uniformly
/// covers every handoff/terminal topic in the ce-executor step
/// chain — independent of whether the preset ships a
/// `payload: json_object` schema for that topic (Observe mode
/// would otherwise let null payloads slip past the schema layer).
pub const NULL_PAYLOAD_REJECT_TOPICS: &[&str] = &[
    "review.passed",
    "review.failed",
    "review.complete",
    "work.done",
    "queue.advance",
    "review.wave.ready",
    "work.ready",
    "plan.complete",
    "plan.blocked",
];

/// Returns `true` if `topic` is in [`NULL_PAYLOAD_REJECT_TOPICS`].
pub fn is_null_payload_rejected_topic(topic: &str) -> bool {
    NULL_PAYLOAD_REJECT_TOPICS.contains(&topic)
}

/// Check topic-deny rules against a (hat, topic) pair.
///
/// When the event policy is in `Enforce` mode and the (hat_id, topic) pair
/// matches any `topic_deny_rules` entry, returns `Some(PolicyDecision::Block)`
/// with reason `"topic_denied"`.  Otherwise returns `None`.
///
/// In `Observe` mode, matching a deny rule produces a `Warn` decision instead.
///
/// Topic matching supports glob patterns:
/// - Exact match: `build.done` matches `build.done`
/// - Segment wildcard: `debug.*` matches `debug.step`, `debug.done`, etc.
/// - Global wildcard: `*` matches any topic
pub fn check_topic_deny_rules(
    hat: Option<&str>,
    topic: &str,
    config: &EventPolicyConfig,
) -> Option<PolicyDecision> {
    let hat_id = hat.unwrap_or("");
    // 2026-06-30 P0-2 (primary-20260629-170451 diagnosis):
    //
    // System control topics (`loop.cancel`, `task.resume`,
    // `build.task.abandoned`) are orchestrated by the loop
    // runner — the per-hat `topic_deny_rules` must not gate
    // them, even when `event.hat` falls under a hat the preset
    // declared a deny rule for (e.g. validator / coordinator /
    // executor are all on the deny list for `task.resume`).
    // Without this short-circuit the runner's stall-recovery
    // `task.resume` injection was rejected with
    // `EVENT_POLICY_TOPIC_DENIED` while the events file still
    // captured it, leaving ledger vs events out-of-sync and
    // deadlocking the loop on `consecutive_failures` once a
    // single retry exhaustion happened
    // (`loop-termination-reason.json: "consecutive_failures"`).
    //
    // We deliberately do NOT special-case `event.*` or
    // `human.*` here — those are admitted by the existing
    // `is_system_topic` short-circuit that runs BEFORE this
    // function in the unified validation pipeline. The
    // completion promise (`LOOP_COMPLETE` by default) is
    // matched against the deny rules directly: a denial there
    // is the legitimate guard against a hat driving past
    // terminal, so we do not bypass it.
    if is_system_control_topic(topic) {
        return None;
    }
    for rule in &config.topic_deny_rules {
        if rule.hat_id == hat_id {
            let matches = if rule.topic.contains('*') {
                Topic::new(&rule.topic).matches_str(topic)
            } else {
                rule.topic == topic
            };
            if matches {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::TopicDenied {
                        rule_hat: rule.hat_id.clone(),
                        rule_topic: rule.topic.clone(),
                    },
                    message: format!(
                        "Hat '{}' is denied from publishing topic '{}'",
                        rule.hat_id, rule.topic
                    ),
                };
                return Some(match config.mode {
                    EventPolicyMode::Observe => PolicyDecision::Warn(vec![finding]),
                    EventPolicyMode::Enforce => match config.on_violation {
                        ViolationAction::Warn => PolicyDecision::Warn(vec![finding]),
                        ViolationAction::RejectWithResume => {
                            PolicyDecision::RejectWithResume(finding)
                        }
                        ViolationAction::Hold => PolicyDecision::Hold(finding),
                        ViolationAction::Block => PolicyDecision::Block(finding),
                    },
                });
            }
        }
    }
    None
}

/// Validates an event against the event policy.
pub fn validate_event(
    topic: &str,
    payload: Option<&str>,
    config: &EventPolicyConfig,
    state: &mut PolicyRuntimeState,
) -> PolicyDecision {
    validate_event_with_hat(topic, payload, config, state, None)
}

/// Validates an event against the event policy with hat-aware checks.
///
/// `hat` is the emitting hat id (if known). When provided, it enables
/// hat-specific schema restrictions such as per-hat allowed values and
/// topic-deny rules. When omitted, only hat-agnostic checks run.
pub fn validate_event_with_hat(
    topic: &str,
    payload: Option<&str>,
    config: &EventPolicyConfig,
    state: &mut PolicyRuntimeState,
    hat: Option<&str>,
) -> PolicyDecision {
    validate_event_with_options(topic, payload, config, state, hat, &DefaultHandoffConfig)
}

/// 2026-07-06-004 plan U8: the handoff envelope validator is
/// opt-in per preset. The default no-op implementation returns
/// `false` so existing call sites see zero behavioural change
/// (regression defence #5). U10 is the unit that wires the real
/// config into the policy pipeline.
pub trait HandoffEnvelopeConfigAccess {
    fn handoff_envelope_enabled(&self) -> bool;
    fn handoff_envelope_validate_payload(&self) -> bool;
}

pub struct DefaultHandoffConfig;

impl HandoffEnvelopeConfigAccess for DefaultHandoffConfig {
    fn handoff_envelope_enabled(&self) -> bool {
        false
    }
    fn handoff_envelope_validate_payload(&self) -> bool {
        false
    }
}

/// Public entry point used by U8's wiring tests and by the real
/// `ralph emit --policy-check` path once U10 feeds the typed
/// config in. Returns the policy decision for the supplied
/// payload against the supplied event policy.
pub fn validate_event_with_options<H: HandoffEnvelopeConfigAccess>(
    topic: &str,
    payload: Option<&str>,
    config: &EventPolicyConfig,
    state: &mut PolicyRuntimeState,
    hat: Option<&str>,
    handoff_config: &H,
) -> PolicyDecision {
    if !config.enabled {
        return PolicyDecision::Accept;
    }

    state.observed_topics.insert(topic.to_string());

    let mut findings = Vec::new();

    // 2026-07-02-004 U7 (R6): close or advance the precheck gate
    // obligation — prune pending `<X>.proposed` keys when the
    // gate emits `<X>.rejected` (fail) or bare `<X>` (pass).
    if let Some(guarded) = topic.strip_suffix(".rejected") {
        state.prune_precheck_proposed_bucket(guarded);
    } else if !topic.ends_with(".proposed") {
        state.prune_precheck_proposed_bucket(topic);
    }

    // 2026-07-02-004 U7 (R6): duplicate `<X>.proposed` detection.
    // A 2nd emit with the same `(guarded, payload)` while the
    // gate obligation is still open is rejected so the runtime
    // does not schedule two gate activations for one candidate.
    if let Some(guarded) = topic.strip_suffix(".proposed")
        && let Some(p) = payload
    {
        let key = precheck_proposed_dedup_key(guarded, p);
        if state.precheck_proposed_pending_keys.contains(&key) {
            let finding = PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::DuplicateWorkDone {
                    key: key.clone(),
                    hint: DuplicateWorkDoneHint::DuplicateSameStep,
                    seen_count: None,
                },
                message: format!(
                    "duplicate_precheck_proposed: {topic} for key '{key}' was already accepted. \
                     Wait for the precheck gate to emit {guarded} or {guarded}.rejected before \
                     re-emitting the same candidate."
                ),
            };
            return PolicyDecision::RejectWithResume(finding);
        }
        state.precheck_proposed_pending_keys.insert(key);
    }

    // 2026-07-01-001 U1: duplicate `review.start` detection.
    // U8 of plan 2026-07-02-005: prefer the semantic key
    // `(plan_name, fix_round, total_units)` so a 2nd emit with
    // only `triggered` differing (175407 root cause) is still
    // recognised as a duplicate. Falls back to the legacy
    // `(plan_name, task_id [, step])` key when fix_round /
    // total_units are absent.
    if topic == "review.start"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        let fix_round = obj
            .get("fix_round")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        let total_units = obj
            .get("total_units")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        if let (Some(pn), Some(ti)) = (plan_name, task_id) {
            let dedup_key = review_start_dedup_key(pn, step, ti, fix_round, total_units);
            if state.review_start_seen_keys.contains(&dedup_key) {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_review_start: review.start for key '{dedup_key}' was already accepted. \
                         Wait for the review sequence to complete before re-sending review.start."
                    ),
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            state.review_start_seen_keys.insert(dedup_key);
        }
    }

    // U4 (2026-06-17-003 plan): duplicate `work.done` detection.
    // The dedup key is `(plan_name, step, task_id)`. A 2nd
    // `work.done` with the same key is rejected as
    // `RecoverableRejection` (NOT fatal) so the runner can
    // re-route to the source hat with a `task.resume` carrying
    // the correct `fix_hint`. The hint distinguishes
    // `duplicate_stall_bypass` (wave_id is set → agent trying
    // to bypass a stalled review cycle) from `duplicate_same_step`
    // (no wave → pure same-step re-emit, fix-round did not
    // advance). The check is applied before all other policy
    // layers so a duplicate is a duplicate regardless of
    // schema/terminal state.
    if topic == "work.done"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        let task_key = obj.get("task_key").and_then(|v| v.as_str());
        if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
            let dedup_key = format!("{pn}::{st}::{ti}");
            // U-fixes-2026-07-04: (task_id, task_key) binding check
            // must come BEFORE dedup. Without it, an agent that
            // changes task_key on retry is misclassified as a
            // duplicate and routed to `task.resume` with no
            // actionable hint. We track the canonical
            // `(task_id) -> task_key` binding seen on the first
            // accept and reject later emits that disagree.
            if let Some(seen_key) = state.work_done_task_id_to_key.get(ti).cloned()
                && let Some(tk) = task_key
                && seen_key != tk
            {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::InvalidFieldValue {
                        field: "task_key".to_string(),
                        value: Value::String(tk.to_string()),
                    },
                    message: format!(
                        "task_id_task_key_mismatch: work.done task_id '{ti}' was first \
                         accepted with task_key '{seen_key}', but this emit uses \
                         task_key '{tk}'. Re-emit with the SAME task_key that \
                         coordinator published in work.ready, OR mint a fresh \
                         task_id via `ralph tools task ensure` before re-sending \
                         work.done."
                    ),
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            if state.work_done_seen_keys.contains(&dedup_key) {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_same_step: work.done for key '{dedup_key}' was already accepted. \
                         Wait for fix.applied / queue.advance / step close before re-sending work.done \
                         for the same (plan_name, step, task_id)."
                    ),
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            // Record the key so a 3rd emit in the same batch is
            // also rejected. The in-batch set is drained by the
            // event loop after `process_output` completes; the
            // per-loop lifetime set lives in
            // `LoopState::work_done_seen_tasks` and is pruned
            // on step-boundary events.
            state.work_done_seen_keys.insert(dedup_key);
            // Track (task_id) → task_key binding so a later
            // emit with the same task_id but a different
            // task_key is rejected as InvalidFieldValue (not
            // DuplicateWorkDone). Without this, retry storms
            // from agents that swap task_key on re-emit
            // silently cycle through dedup rejections with no
            // actionable hint. Pruned alongside
            // `work_done_seen_tasks` on step boundaries.
            if let Some(tk) = task_key {
                state
                    .work_done_task_id_to_key
                    .insert(ti.to_string(), tk.to_string());
            }
        }
    }

    // U5 (2026-06-17-003 plan, R6): duplicate
    // `review.dimension.ready` detection. The dedup key is
    // `(plan_name, step, task_id, dimension)`. A 2nd
    // `review.dimension.ready` with the same key is rejected
    // as `RejectWithResume` so the runner publishes a
    // `task.resume` with `fix_hint` pointing the agent to wait
    // for the matching `review.dimension.done` /
    // `review.dimension.failed` before re-sending
    // `review.dimension.ready`. The check is applied before
    // schema/terminal layers so a duplicate is a duplicate
    // regardless of state.
    //
    // We reuse the `DuplicateWorkDone` variant (same key/hint
    // shape) rather than introducing a new ViolationType
    // because the recovery flow is identical: both are
    // recoverable rejections that carry a retry-key, and
    // `is_recoverable_policy_finding` already maps the variant
    // to the correct bucket. Adding a new variant would force
    // a parallel `is_recoverable` arm and a parallel
    // `reason_code` mapping for no behavioral gain.
    if topic == "review.dimension.ready"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        let dimension = obj.get("dimension").and_then(|v| v.as_str());
        if let (Some(pn), Some(st), Some(ti), Some(dim)) = (plan_name, step, task_id, dimension) {
            let dedup_key = format!("{pn}::{st}::{ti}::{dim}");
            if state.review_dimension_ready_seen_keys.contains(&dedup_key) {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        // 2026-07-04-024019 run P0-1: distinct hint so
                        // `reason_code` reports `duplicate_review_dimension_ready`
                        // instead of `duplicate_work_done_same_step`.
                        hint: DuplicateWorkDoneHint::ReviewDimensionDuplicate,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_dimension_ready: review.dimension.ready for key '{dedup_key}' \
                         was already accepted. Wait for review.dimension.done / \
                         review.dimension.failed for the same dimension before re-sending \
                         review.dimension.ready."
                    ),
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            // Record the key so a 3rd emit in the same batch
            // is also rejected. The in-batch set is drained by
            // the event loop after `process_output` completes;
            // the per-loop lifetime set is populated by
            // `from_events` on restart so cross-batch replays
            // honor the dedup.
            state.review_dimension_ready_seen_keys.insert(dedup_key);
        }
    }

    // 2026-07-02 P1-A: `review.dimension.failed` schema gate.
    // The dedup / stage gate only checks `(hat, topic)`, not the
    // payload, so a `dimension-reviewer` emit with
    // `dimension=unknown` (or missing the field entirely) would
    // slip through and leave review-coordinator with an unknown
    // dimension to retry. The 6-dimension whitelist mirrors
    // `ce-executor-serial.yml` line 1505-1528
    // (goal-alignment → correctness → testing →
    // maintainability → project-standards → adversarial) so
    // a wrong / missing dimension is rejected as
    // `InvalidFieldValue` instead of surfacing as
    // `flow_unknown_emit` downstream. The check sits in the
    // policy layer (not the flow-scope stage) so the same gate
    // fires for both the in-loop emit path and the
    // CLI precheck emit path; the reason code is
    // `invalid_field_value` so the existing
    // `InvalidFieldValue` recovery hint is reused.
    if topic == "review.dimension.failed"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        const DIMENSION_WHITELIST: &[&str] = &[
            "goal-alignment",
            "correctness",
            "testing",
            "maintainability",
            "project-standards",
            "adversarial",
        ];
        match obj.get("dimension") {
            None => {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::MissingRequiredField {
                        field: "dimension".to_string(),
                    },
                    message: format!(
                        "review.dimension.failed payload is missing required 'dimension' field \
                         (allowed: {})",
                        DIMENSION_WHITELIST.join(", ")
                    ),
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            Some(Value::String(dim)) if !DIMENSION_WHITELIST.contains(&dim.as_str()) => {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::InvalidFieldValue {
                        field: "dimension".to_string(),
                        value: Value::String(dim.clone()),
                    },
                    message: format!(
                        "review.dimension.failed payload has unknown 'dimension' value \
                         '{dim}'; allowed: {}",
                        DIMENSION_WHITELIST.join(", ")
                    ),
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            _ => {} // allowed dimension, fall through.
        }
    }

    // U5 (2026-06-18-004 plan, R4, KTD3) + U6 (2026-06-18-006
    // plan, R6, KTD4): duplicate `review.dimensions.complete`
    // detection. The dedup key is
    // `(plan_name, step, task_id, fix_round)`. A 2nd emit with
    // the same key is rejected as `RejectWithResume` so the
    // runner publishes a `task.resume` with `fix_hint`. The
    // `fix_round` segment distinguishes re-review rounds so a
    // `fix.applied`-pruned bucket (U1) lets a 2nd
    // `review.dimensions.complete` land for `fix_round=N+1`
    // without colliding with `fix_round=N`.
    //
    // U6 (KTD4): `fix_round` is required by the schema
    // (2026-06-18-004 plan U0 made it required). The dedup
    // layer now mirrors that requirement — missing or
    // non-numeric `fix_round` falls through without recording
    // the dedup key, so the schema validator reports
    // `missing_required_field` (or `type_mismatch`) instead of
    // the dedup layer hiding the failure behind
    // `DuplicateWorkDone`. The previous behavior (defaults
    // `0`, silent dedup) masked schema-invalid emits behind a
    // misleading "duplicate" recovery hint.
    //
    // We reuse the `DuplicateWorkDone` variant for parity
    // with the `review.dimension.ready` check above — same
    // recovery shape, same hint bucket.
    if topic == "review.dimensions.complete"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        // U6 (KTD4): only treat the dedup key as a real
        // dimension-complete when `fix_round` is a present
        // u64. Missing or non-numeric `fix_round` falls
        // through — the event will be rejected by the schema
        // layer with `missing_required_field` (or
        // `type_mismatch`), which is the correct error
        // message for the agent. Deduping a schema-invalid
        // event hides the real failure mode behind
        // `DuplicateWorkDone`.
        let fix_round = match obj.get("fix_round") {
            Some(Value::Number(n)) => n.as_u64(),
            _ => None, // missing or non-numeric → None (not Some(0))
        };
        if let (Some(pn), Some(st), Some(ti), Some(fr)) = (plan_name, step, task_id, fix_round) {
            let dedup_key = format!("{pn}::{st}::{ti}::{fr}");
            if state
                .review_dimensions_complete_seen_keys
                .contains(&dedup_key)
            {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        // U6 (plan 2026-07-04-004): switch to the
                        // dedicated `ReviewDimensionsComplete` hint
                        // so the dedup reason code is
                        // `duplicate_review_dimensions_complete`
                        // rather than the misleading generic
                        // `duplicate_work_done`.
                        hint: DuplicateWorkDoneHint::ReviewDimensionsComplete,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_dimensions_complete: review.dimensions.complete for key \
                         '{dedup_key}' was already accepted for the same fix_round. \
                         After fix.applied the next round must use fix_round=N+1 and walk \
                         review.dimension.ready first (see U3 obligations)."
                    ),
                };
                // U2 (plan 2026-07-04-004): silently-success
                // `review.dimensions.complete` re-emits must not
                // trigger `task.resume` storms (per
                // `docs/report/2026-07-04-...` silent-success
                // diagnosis). Returning `AcknowledgeAndForward`
                // keeps the dedup invariant (mirror is unchanged)
                // while letting the event reach the bus without
                // injecting a recovery directive. Other dedup
                // branches continue to surface
                // `RejectWithResume` so existing semantics stay
                // intact; this carve-out is intentionally narrow.
                return PolicyDecision::AcknowledgeAndForward(finding);
            }
            state.review_dimensions_complete_seen_keys.insert(dedup_key);
        }
        // else: any of `plan_name`/`step`/`task_id`/`fix_round`
        // missing or non-string/non-u64 → no dedup mirror write,
        // no `DuplicateWorkDone` rejection. The downstream schema
        // validator is responsible for emitting the precise
        // `missing_required_field` / `type_mismatch` message.
    }

    // 2026-06-24 P1-3: duplicate `work.ready` detection. The
    // dedup key is `(plan_name, step, task_id)` — same shape as
    // `work.done`. A 2nd `work.ready` with the same key is
    // rejected as `RejectWithResume` so the agent stops
    // re-announcing an already-started unit. The check fires
    // before schema/terminal layers so a duplicate is a
    // duplicate regardless of state. Pruned on `fix.applied` /
    // step close so a legitimate re-emit after a fix round is
    // allowed.
    if topic == "work.ready"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
            let dedup_key = format!("{pn}::{st}::{ti}");
            // U5 of plan 2026-07-05-005 (fix-plan §R8): a re-emit
            // after `fix.applied` pruning is allowed (the bucket
            // classification is cleared), but the dedup hit
            // counter must survive — we increment it without
            // rejecting so the dup-storm signal remains
            // observable. The bucket prune marks the key in
            // `pruned_work_ready_buckets`; the check below uses
            // that side-table to accept the emit and bump the
            // counter.
            if state.pruned_work_ready_buckets.contains(&dedup_key) {
                let count = state
                    .work_ready_seen_keys
                    .get(&dedup_key)
                    .copied()
                    .unwrap_or(0);
                state
                    .work_ready_seen_keys
                    .insert(dedup_key.clone(), count.saturating_add(1));
                // Bucket-pruned emit falls through to Accept —
                // count is observation, not dedup state.
            } else if state.work_ready_seen_keys.contains_key(&dedup_key) {
                // U5 of plan 2026-07-05-005 (R8): bump the
                // counter on every observed hit. The counter is
                // observation, not dedup state — `fix.applied`
                // pruning never resets it (see the prune helper
                // below).
                let count = state
                    .work_ready_seen_keys
                    .get(&dedup_key)
                    .copied()
                    .unwrap_or(0);
                state
                    .work_ready_seen_keys
                    .insert(dedup_key.clone(), count.saturating_add(1));
                let hit_count = count.saturating_add(1);
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                        seen_count: Some(hit_count),
                    },
                    message: format!(
                        "duplicate_work_ready: work.ready for key '{dedup_key}' was already accepted \
                         (seen_count={hit_count}). Wait for fix.applied / step close before re-sending \
                         work.ready for the same (plan_name, step, task_id)."
                    ),
                };
                return PolicyDecision::RejectWithResume(finding);
            } else {
                // First acceptance: seed the counter at 1 so a
                // subsequent hit reads `seen_count: 2`.
                state.work_ready_seen_keys.insert(dedup_key, 1);
            }
        }
    }

    // 2026-06-24 P1-3: duplicate `test.passed` / `test.failed`
    // detection. The dedup key is
    // `(plan_name, step, task_id, fix_round)` — same shape as
    // `review.dimensions.complete`. A 2nd emit with the same
    // key is rejected as `RejectWithResume`. The `fix_round`
    // segment distinguishes re-test rounds so a
    // `fix.applied`-pruned bucket allows a 2nd `test.passed` /
    // `test.failed` to land for a new fix round without
    // colliding with the prior round's entry.
    //
    // Mirrors the U6 KTD4 rule: missing or non-numeric
    // `fix_round` falls through without recording the dedup
    // key, so the schema validator reports
    // `missing_required_field` (or `type_mismatch`) instead of
    // the dedup layer hiding the failure behind
    // `DuplicateWorkDone`.
    if (topic == "test.passed" || topic == "test.failed")
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        let fix_round = match obj.get("fix_round") {
            Some(Value::Number(n)) => n.as_u64(),
            _ => None,
        };
        if let (Some(pn), Some(st), Some(ti), Some(fr)) = (plan_name, step, task_id, fix_round) {
            let dedup_key = format!("{pn}::{st}::{ti}::{fr}");
            let seen = if topic == "test.passed" {
                state.test_passed_seen_keys.contains(&dedup_key)
            } else {
                state.test_failed_seen_keys.contains(&dedup_key)
            };
            if seen {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_test_result: {topic} for key '{dedup_key}' was already accepted \
                         for the same fix_round. After fix.applied the next round must use \
                         fix_round=N+1 before re-sending {topic}."
                    ),
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            if topic == "test.passed" {
                state.test_passed_seen_keys.insert(dedup_key);
            } else {
                state.test_failed_seen_keys.insert(dedup_key);
            }
        }
        // else: missing/non-numeric `fix_round` or missing
        // `plan_name`/`step`/`task_id` → no dedup mirror write.
        // The schema validator reports the precise
        // `missing_required_field` / `type_mismatch` error.
    }

    // WAC-U7 R10 (2026-06-12-002): null payloads on the
    // `NULL_PAYLOAD_REJECT_TOPICS` whitelist are hard-rejected
    // with `RejectWithResume`, overriding any `Observe`-mode
    // downgrades. The check is applied before schema
    // validation so a topic without an explicit `schemas`
    // entry still gets the R10 treatment. KTD-9.
    if payload.is_none() && is_null_payload_rejected_topic(topic) {
        let finding = PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::PayloadTypeMismatch {
                expected: "non-null payload".to_string(),
                actual: "null".to_string(),
            },
            message: format!(
                "WAC R10: null payload on whitelist topic `{}` is hard-rejected; \
                 a structured payload is required for this topic",
                topic
            ),
        };
        return PolicyDecision::RejectWithResume(finding);
    }

    // Terminal monotonicity check (read-only on state; caller applies terminal_observed)
    if state.terminal_observed && config.business_topics.contains(&topic.to_string()) {
        let terminal_topic = config.terminal_topics.first().cloned().unwrap_or_default();
        findings.push(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::TerminalMonotonicityViolation {
                terminal_topic: terminal_topic.clone(),
                business_topic: topic.to_string(),
            },
            message: format!(
                "Business event '{}' after terminal topic '{}' violates monotonicity",
                topic, terminal_topic
            ),
        });
    }

    // Duplicate terminal check (read-only on state; caller applies terminal_observed)
    if state.terminal_observed && config.terminal_topics.contains(&topic.to_string()) {
        findings.push(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::DuplicateTerminalEvent {
                topic: topic.to_string(),
            },
            message: format!(
                "Duplicate terminal event '{}' after terminal topic was already observed",
                topic
            ),
        });
    }

    // Schema validation
    if let Some(schema) = config.schemas.get(topic) {
        if let Some(expected_type) = &schema.payload
            && matches!(expected_type, PayloadType::JsonObject)
        {
            // WAC-U7 R11 (2026-06-12-002) KTD-10: a string payload
            // that parses to a JSON object is normalized to the
            // serialized object form before required-field
            // validation runs. Non-object strings fall through
            // to the regular type-mismatch finding. The
            // normalized string is captured in
            // `normalized_payload` so the required-fields block
            // below sees the object form.
            let mut normalized_payload: Option<String> = None;
            match payload {
                Some(p) => match serde_json::from_str::<Value>(p) {
                    Ok(Value::Object(map)) => {
                        normalized_payload = Some(
                            serde_json::to_string(&Value::Object(map))
                                .unwrap_or_else(|_| p.to_string()),
                        );
                    }
                    Ok(other) => {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::PayloadTypeMismatch {
                                expected: "json_object".to_string(),
                                actual: format!("{:?}", other),
                            },
                            message: format!("Payload must be JSON object, got {:?}", other),
                        });
                        normalized_payload = Some(p.to_string());
                    }
                    Err(e) => {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::PayloadTypeMismatch {
                                expected: "json_object".to_string(),
                                actual: format!("parse error: {}", e),
                            },
                            message: format!("Payload is not valid JSON: {}", e),
                        });
                        normalized_payload = Some(p.to_string());
                    }
                },
                None => {
                    findings.push(PolicyFinding {
                        topic: topic.to_string(),
                        violation_type: ViolationType::PayloadTypeMismatch {
                            expected: "json_object".to_string(),
                            actual: "null".to_string(),
                        },
                        message: "Payload is required to be JSON object but is missing".to_string(),
                    });
                }
            }

            // Required fields — applied AFTER normalize (KTD-10).
            if !schema.required_fields.is_empty() {
                let payload_for_required = normalized_payload.as_deref().or(payload);
                if let Some(p) = payload_for_required {
                    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p) {
                        for field in &schema.required_fields {
                            if extract_json_field(&Value::Object(obj.clone()), field).is_none() {
                                findings.push(PolicyFinding {
                                    topic: topic.to_string(),
                                    violation_type: ViolationType::MissingRequiredField {
                                        field: field.clone(),
                                    },
                                    message: format!("Missing required field: {}", field),
                                });
                            }
                        }
                    }
                } else {
                    // Payload is missing but required fields are specified
                    for field in &schema.required_fields {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::MissingRequiredField {
                                field: field.clone(),
                            },
                            message: format!(
                                "Missing required field '{}' (payload is missing)",
                                field
                            ),
                        });
                    }
                }
            }
        } else {
            // Required fields (no json_object payload requirement)
            if !schema.required_fields.is_empty() {
                if let Some(p) = payload {
                    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p) {
                        for field in &schema.required_fields {
                            if extract_json_field(&Value::Object(obj.clone()), field).is_none() {
                                findings.push(PolicyFinding {
                                    topic: topic.to_string(),
                                    violation_type: ViolationType::MissingRequiredField {
                                        field: field.clone(),
                                    },
                                    message: format!("Missing required field: {}", field),
                                });
                            }
                        }
                    }
                } else {
                    for field in &schema.required_fields {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::MissingRequiredField {
                                field: field.clone(),
                            },
                            message: format!(
                                "Missing required field '{}' (payload is missing)",
                                field
                            ),
                        });
                    }
                }
            }
        }

        // String-only field guard (2026-06-24 P0-D regression).
        //
        // `review.complete.fix_plan_file` is documented in the SSOT
        // (`presets/schemas/ce-executor-serial.yml` `review.complete` schema
        // and the ce-executor-serial preset coordinator instructions) as the
        // literal string `"null"` when there are no P0/P1 findings. The
        // 2026-06-24 ralph-e2e run on `python-sort-algorithms` shipped
        // `fix_plan_file: null` (a JSON `null` literal) for the fix-01
        // review round, which slipped through `required_fields` (the field
        // existed), passed through the orchestrator, and broke the
        // downstream coordinator's `fix_plan_file == "null"` string
        // equality check — leaving `plan.complete` un-emitted and the
        // loop stuck for 30+ minutes until progress-steward eventually
        // rescued it.
        //
        // `required_fields` only asserts the field exists; it does NOT
        // assert a JSON value type. This block fills that gap for the
        // single field where the runtime contract is type-strict.
        // `allowed_values` cannot enforce it cleanly because `"null"` is
        // a single-element allowed set — JSON `null` would compare
        // unequal but the runner would never see the violation as a
        // `PayloadTypeMismatch`. A dedicated violation keeps the error
        // message actionable.
        if topic == "review.complete"
            && let Some(p) = payload
            && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
            && let Some(field_value) = obj.get("fix_plan_file")
            && !field_value.is_string()
        {
            findings.push(PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::PayloadTypeMismatch {
                    expected: "string".to_string(),
                    actual: type_name(field_value).to_string(),
                },
                message: format!(
                    "review.complete.fix_plan_file must be a string (use the literal \"null\" for no fix plan), got JSON {}",
                    type_name(field_value)
                ),
            });
        }

        // Allowed values (hat-agnostic)
        for (field_path, allowed) in &schema.allowed_values {
            if let Some(p) = payload
                && let Ok(value) = serde_json::from_str::<Value>(p)
                && let Some(field_value) = extract_json_field(&value, field_path)
                && !allowed.contains(&field_value)
            {
                findings.push(PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::InvalidFieldValue {
                        field: field_path.clone(),
                        value: field_value.clone(),
                    },
                    message: format!(
                        "Field '{}' has invalid value {:?}. Allowed: {:?}",
                        field_path, field_value, allowed
                    ),
                });
            }
        }

        // Hat-aware allowed values.
        // U1 (2026-06-17-004 plan, R2): fail-closed when provenance is
        // missing and the schema carries per-hat restrictions. Without
        // a known hat, no hat-specific value can be validated, so the
        // event must be rejected — leaving the question of "which hat"
        // to the caller (CLI emit pipeline enforces `check_emit_provenance`
        // before reaching this function; programmatic callers are still
        // required to supply a hat for topics with `hat_allowed_values`).
        //
        // The previous code silently skipped the entire hat-aware block
        // when `hat = None`. That let a hat-less emit bypass the
        // per-hat restriction (e.g. review-coordinator could emit
        // `review.passed(skip_reason=aggregate_timeout)` by dropping
        // the `--hat` flag). This is now a hard `MissingRequiredField`
        // finding — the gate fails closed.
        if schema.hat_allowed_values.is_empty() {
            // No per-hat restrictions on this topic — skip the block.
            // (Implicit: when `hat = None` and no `hat_allowed_values`
            // are configured, nothing to validate.)
        } else if let Some(hat_id) = hat {
            for (field_path, per_hat_rules) in &schema.hat_allowed_values {
                if let Some(rule) = per_hat_rules.iter().find(|r| r.hat_id == hat_id)
                    && let Some(p) = payload
                    && let Ok(value) = serde_json::from_str::<Value>(p)
                    && let Some(field_value) = extract_json_field(&value, field_path)
                    && !rule.values.contains(&field_value)
                {
                    findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::InvalidFieldValue {
                                field: field_path.clone(),
                                value: field_value.clone(),
                            },
                            message: format!(
                                "Hat '{}' may not use value {:?} for field '{}'. Allowed for this hat: {:?}",
                                hat_id, field_value, field_path, rule.values
                            ),
                        });
                }
            }
        } else {
            // Hat is missing but schema has hat-specific allowed values.
            // Without provenance we cannot pick the right rule, so we
            // emit a single finding that names the topic + the per-hat
            // restrictions. The CLI emit pipeline's
            // `check_emit_provenance` rejects this event earlier; this
            // finding covers programmatic callers (API server,
            // in-process emitters) that go straight to
            // `validate_event_with_hat`.
            let mut per_hat_summary: Vec<String> = Vec::new();
            for (field_path, per_hat_rules) in &schema.hat_allowed_values {
                for rule in per_hat_rules {
                    per_hat_summary.push(format!(
                        "hat='{}' field='{}' allowed={:?}",
                        rule.hat_id, field_path, rule.values
                    ));
                }
            }
            findings.push(PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::MissingRequiredField {
                    field: "hat".to_string(),
                },
                message: format!(
                    "Topic '{topic}' has hat-specific allowed values; a hat is required \
                     to validate the payload. Provenance rules: {per_hat_summary:?}. \
                     Pass --hat <hat-id> or set RALPH_CURRENT_HAT=<hat-id>."
                ),
            });
        }
    }

    // U4: plan_name equality — when enabled, work.done's plan_name must equal
    // the current_plan_name extracted from the most recent work.ready event.
    if config.plan_name_equality_required
        && topic == "work.done"
        && let Some(expected) = &state.current_plan_name
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let actual = obj.get("plan_name").and_then(|v| v.as_str());
        if actual != Some(expected.as_str()) {
            findings.push(PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::InvalidFieldValue {
                    field: "plan_name".to_string(),
                    value: actual
                        .map(|s| Value::String(s.to_string()))
                        .unwrap_or(Value::Null),
                },
                message: format!(
                    "work.done plan_name mismatch: expected '{}', got {:?}",
                    expected,
                    actual.unwrap_or("(missing)")
                ),
            });
        }
    }

    // U7 of plan 2026-07-02-005: runtime shipper strict-match backstop.
    if topic == "REVIEW_COMPLETE"
        && let Some(finding) = crate::shipper_reason::check_review_complete_shipper_routing(
            payload,
            state.last_plan_blocked_reason.as_deref(),
        )
    {
        findings.push(finding);
    }

    // 2026-07-03-005 plan (P0 fix C7): per-element shape validation for
    // array fields declared in the schema's `element_constraints` map.
    // Today this single-handedly closes the
    // `review.dimensions.complete` silent-drop bug — when the agent
    // fabricates a `status: done` element with a null findings_file,
    // the schema rejects the element and the runtime surfaces the
    // real cause instead of accepting the inflated review summary.
    if !config.schemas.is_empty()
        && let Some(p) = payload
        && let Ok(Value::Object(_)) = serde_json::from_str::<Value>(p)
    {
        let topic_schema = config.schemas.get(topic);
        if let Some(schema) = topic_schema
            && !schema.element_constraints.is_empty()
            && let Some(value) = serde_json::from_str::<Value>(p).ok()
        {
            for (array_field, constraint) in &schema.element_constraints {
                if let Some(field) = obj_get(&value, array_field) {
                    if let Value::Array(elements) = field {
                        for (idx, element) in elements.iter().enumerate() {
                            if let Some(finding) =
                                validate_element_shape(topic, array_field, idx, element, constraint)
                            {
                                findings.push(finding);
                            }
                        }
                    } else {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::PayloadTypeMismatch {
                                expected: "array".to_string(),
                                actual: type_name(field).to_string(),
                            },
                            message: format!(
                                "element_constraints: field '{}' must be an array, got {}",
                                array_field,
                                type_name(field)
                            ),
                        });
                    }
                }
            }
        }
    }

    // 2026-07-06-004 plan U8: handoff envelope validation. When
    // `event_loop.handoff_envelope.enabled` is on AND
    // `validate_payload` is on, every business event whose
    // payload parses as a JSON object must contain a valid
    // `payload.handoff_envelope` (per `handoff_envelope.v1`).
    // Runtime-injected control topics (`task.resume`) skip
    // the check — the recovery path is synthesised by the
    // runner and cannot carry an agent-authored envelope.
    // The check is gated on the typed config so non-serial
    // presets and ad-hoc loops are unaffected (regression
    // defence #5). When the flag fires for a `task.resume`
    // it would otherwise deadlock the recovery channel.
    if handoff_config.handoff_envelope_enabled()
        && handoff_config.handoff_envelope_validate_payload()
        && topic != "task.resume"
        && let Some(p) = payload
    {
        match serde_json::from_str::<Value>(p) {
            Ok(value) => {
                if let Some(finding) = check_handoff_envelope(topic, &value) {
                    findings.push(finding);
                }
            }
            Err(_) => {
                // If the payload does not parse as JSON we
                // don't add a finding here — earlier
                // validation layers will surface that.
            }
        }
    }

    // U3 (plan 2026-07-22-004): opt-in same-payload consistency gates.
    // After schema / allowed-values / hat-aware / element_constraints
    // checks have gathered their findings, evaluate any enabled
    // `payload_consistency` rule whose `topic` matches the current
    // topic against the CURRENT payload only (R2 — never event
    // history). The first hit in stable declaration order is surfaced
    // as a `SemanticGateViolation` with gate `payload_consistency:<id>`;
    // the decision mapper below takes `findings.into_iter().next()`, so
    // we push only the first hit and break (simplest correct approach,
    // preserves declaration order). Reuses the existing
    // `ViolationType::SemanticGateViolation` variant (KTD3) — the
    // `payload_consistency:` gate prefix distinguishes it from
    // timing/state semantic gates. A missing or non-object payload
    // cannot satisfy a field predicate, so it is treated as no-hit
    // (NOT an error) — schema validation already handles payload shape.
    if config.payload_consistency.enabled
        && let Some(p) = payload
        && let Ok(value) = serde_json::from_str::<Value>(p)
        && value.is_object()
    {
        for rule in &config.payload_consistency.rules {
            if rule.topic != topic {
                continue;
            }
            if crate::event_policy_payload_consistency::evaluate(&rule.when, &value)
                == crate::event_policy_payload_consistency::EvalOutcome::Hit
            {
                let gate = format!("payload_consistency:{}", rule.id);
                // U2 (2026-07-23-002 plan, KTD2): collect the stable,
                // declaration-order set of business fields the rule's
                // predicate AST references so agent repair tooling can
                // know which payload fields to inspect without parsing
                // `rule.message`. This is the static declared set, not
                // the short-circuited "matched" subset.
                let referenced_fields =
                    crate::event_policy_payload_consistency::collect_referenced_fields(&rule.when);
                findings.push(PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::SemanticGateViolation {
                        gate: gate.clone(),
                        context: rule.message.clone(),
                        referenced_fields,
                    },
                    message: format!("{gate}: {}", rule.message),
                });
                break;
            }
        }
    }

    if findings.is_empty() {
        if topic == "plan.blocked" {
            state.last_plan_blocked_reason =
                crate::shipper_reason::extract_plan_blocked_reason(payload);
        } else if topic == "plan.complete" {
            state.last_plan_blocked_reason = None;
        }
        return PolicyDecision::Accept;
    }

    match config.mode {
        EventPolicyMode::Observe => PolicyDecision::Warn(findings),
        EventPolicyMode::Enforce => match config.on_violation {
            ViolationAction::Warn => PolicyDecision::Warn(findings),
            ViolationAction::RejectWithResume => {
                PolicyDecision::RejectWithResume(findings.into_iter().next().unwrap())
            }
            ViolationAction::Hold => PolicyDecision::Hold(findings.into_iter().next().unwrap()),
            ViolationAction::Block => PolicyDecision::Block(findings.into_iter().next().unwrap()),
        },
    }
}

/// Extract a nested field from a JSON value using dot notation.
///
/// Shared with the payload-consistency evaluator and lint. Keep this
/// single implementation; do not re-introduce local copies.
pub(crate) fn extract_json_field(value: &Value, path: &str) -> Option<Value> {
    let mut current = value;
    for part in path.split('.') {
        match current {
            Value::Object(obj) => {
                current = obj.get(part)?;
            }
            _ => return None,
        }
    }
    Some(current.clone())
}

/// Human-friendly name for a JSON value's runtime type.
/// Used by the 2026-06-24 P0-D `review.complete.fix_plan_file` string-only
/// guard to produce actionable error messages.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 2026-07-03-005 plan (P0 fix C7): look up a top-level field on a JSON
/// value (avoids the dot-notation `extract_json_field` semantics, which
/// would split on `.` and is wrong for array element field names that
/// may contain dots in the future). Returns `Some(value)` for both
/// present-and-null and present-with-value.
fn obj_get<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    value.as_object()?.get(field)
}

/// 2026-07-03-005 plan (P0 fix C7): validate one element of an array
/// field against its `ElementConstraint`. Returns `Some(PolicyFinding)`
/// on the first violation per element, or `None` if the element
/// passes. The constraint covers: existence (`required`), value
/// restriction (`allowed_values`), conditional existence
/// (`required_when` + `forbid_null_when_required`).
fn validate_element_shape(
    topic: &str,
    array_field: &str,
    idx: usize,
    element: &Value,
    constraint: &crate::config::ElementConstraint,
) -> Option<PolicyFinding> {
    // 1. required field exists
    let present = obj_get(element, &constraint.field);
    if constraint.required && present.is_none() {
        return Some(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::MissingRequiredField {
                field: format!("{}[{}].{}", array_field, idx, constraint.field),
            },
            message: format!(
                "element_constraints: {}[{}] is missing required field '{}'",
                array_field, idx, constraint.field
            ),
        });
    }

    // 2. allowed_values check
    if !constraint.allowed_values.is_empty()
        && let Some(value) = present
        && !constraint.allowed_values.iter().any(|v| v == value)
    {
        return Some(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::InvalidFieldValue {
                field: format!("{}[{}].{}", array_field, idx, constraint.field),
                value: value.clone(),
            },
            message: format!(
                "element_constraints: {}[{}].{} = {} not in allowed list {:?}",
                array_field,
                idx,
                constraint.field,
                type_name(value),
                constraint.allowed_values
            ),
        });
    }

    // 3. required_when + forbid_null_when_required
    if !constraint.required_when.is_empty() {
        let mut all_conditions_match = true;
        for (key, expected) in &constraint.required_when {
            let actual = obj_get(element, key);
            if actual != Some(expected) {
                all_conditions_match = false;
                break;
            }
        }
        if all_conditions_match {
            if present.is_none() {
                return Some(PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::MissingRequiredField {
                        field: format!("{}[{}].{}", array_field, idx, constraint.field),
                    },
                    message: format!(
                        "element_constraints: {}[{}].{} is required when sibling conditions {:?} match",
                        array_field, idx, constraint.field, constraint.required_when
                    ),
                });
            }
            if constraint.forbid_null_when_required && matches!(present, Some(Value::Null)) {
                return Some(PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::InvalidFieldValue {
                        field: format!("{}[{}].{}", array_field, idx, constraint.field),
                        value: Value::Null,
                    },
                    message: format!(
                        "element_constraints: {}[{}].{} is null but must be non-null when sibling conditions {:?} match",
                        array_field, idx, constraint.field, constraint.required_when
                    ),
                });
            }
        }
    }

    None
}

// ─────────────────────────────────────────────────────────────────────
// Unit 2 of plan 2026-07-27-002: read-only `evaluate_candidate_emit`
// preview for the `ralph inspect prompt --topic` path.
// ─────────────────────────────────────────────────────────────────────

/// Result of evaluating a candidate event for preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateEmitPreview {
    /// "accept" | "reject"
    pub policy_decision: String,
    /// Reasons when rejected (empty when accepted).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<PolicyReasonEntry>,
    /// Projection preview (what state changes would occur).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection: Option<ProjectionPreview>,
    /// Next hat candidates (who receives this event).
    pub next_hat_candidates: NextHatCandidates,
}

/// One structured reason for rejection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyReasonEntry {
    pub gate: String,
    pub field: String,
    pub reason_code: String,
}

/// Projection (state changes) that would result from the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionPreview {
    pub state_changes: Vec<ProjectionAction>,
}

/// One projection action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionAction {
    pub field: String,
    pub action: String,
    pub value: serde_json::Value,
}

/// Who receives the event downstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NextHatCandidates {
    /// At least one matching hat, all verified against config.hats.
    Verified { hats: Vec<String> },
    /// No hat registry available (hatless mode / empty registry).
    Unverified,
    /// Some hats matched but were not all verifiable.
    Mixed { entries: Vec<CandidateHatEntry> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateHatEntry {
    pub hat_id: String,
    pub verified: bool,
}

/// Evaluate a candidate event for the inspect prompt path (read-only).
///
/// Returns a structured preview of whether the event would be accepted
/// by the policy gateway, what projections would apply, and which hats
/// would receive the event.
///
/// This function is **read-only**: it never writes to disk, never
/// publishes events, and never mutates real runtime state. It uses
/// `PolicyRuntimeState::default()` for a dry-run evaluation.
pub fn evaluate_candidate_emit(
    config: &RalphConfig,
    hat_id: &HatId,
    topic: &str,
    payload_json: &str,
    triggered: Option<&str>,
) -> Result<CandidateEmitPreview, String> {
    // 1. Check that hat_id exists in config.
    let hat_config = config
        .hats
        .get(hat_id.as_str())
        .ok_or_else(|| format!("hat {} not found in config", hat_id.as_str()))?;

    // U6: reject if the hat does not publish this topic (and it is not the
    // default_publishes fallback).  Checked before topic_format so the
    // topology gate takes priority over format validation.
    let hat_publishes_topic = hat_config.publishes.iter().any(|p| p == topic)
        || hat_config
            .default_publishes
            .as_deref()
            .is_some_and(|d| d == topic);

    if !hat_publishes_topic {
        return Ok(CandidateEmitPreview {
            policy_decision: "reject".to_string(),
            reasons: vec![PolicyReasonEntry {
                gate: "topic_publishes".to_string(),
                field: "topic".to_string(),
                reason_code: "hat_does_not_publish_topic".to_string(),
            }],
            projection: None,
            next_hat_candidates: NextHatCandidates::Unverified,
        });
    }

    // 2. Check topic format via build_allowed_topics + check_topic_format.
    let completion_promise = &config.event_loop.completion_promise;
    let event_policy = config.event_loop.event_policy.as_ref();
    let allowed_topics = build_allowed_topics(&config.hats, completion_promise, event_policy);

    // System topics are allowed regardless.
    let topic_format_ok =
        is_system_topic(topic) || check_topic_format(topic, &allowed_topics).is_none();

    if !topic_format_ok {
        return Ok(CandidateEmitPreview {
            policy_decision: "reject".to_string(),
            reasons: vec![PolicyReasonEntry {
                gate: "topic_format".to_string(),
                field: "topic".to_string(),
                reason_code: "invalid_topic_format".to_string(),
            }],
            projection: None,
            next_hat_candidates: NextHatCandidates::Unverified,
        });
    }

    // U6: reject if `triggered` is specified but is not a registered hat.
    if let Some(triggered_hat) = triggered
        && !config.hats.contains_key(triggered_hat)
    {
        return Ok(CandidateEmitPreview {
            policy_decision: "reject".to_string(),
            reasons: vec![PolicyReasonEntry {
                gate: "triggered_not_in_topology".to_string(),
                field: "triggered".to_string(),
                reason_code: "triggered_hat_not_in_config".to_string(),
            }],
            projection: None,
            next_hat_candidates: NextHatCandidates::Unverified,
        });
    }

    // 3. Run validate_event_with_hat for policy validation (dry-run with default state).
    let policy_config = match event_policy {
        Some(ep) => ep.clone(),
        None => {
            // No policy config — accept by default.
            return Ok(CandidateEmitPreview {
                policy_decision: "accept".to_string(),
                reasons: Vec::new(),
                projection: None,
                next_hat_candidates: compute_next_hat_candidates(config, topic),
            });
        }
    };

    let mut state = PolicyRuntimeState::default();
    let hat_str = Some(hat_id.as_str());
    let decision = validate_event_with_hat(
        topic,
        Some(payload_json),
        &policy_config,
        &mut state,
        hat_str,
    );

    // Build the preview from the policy decision.
    let (policy_decision, reasons) = match &decision {
        PolicyDecision::Accept => ("accept".to_string(), Vec::new()),
        PolicyDecision::Warn(_findings) => {
            // Warnings are still accepted.
            ("accept".to_string(), Vec::new())
        }
        PolicyDecision::RejectWithResume(finding) => {
            let reason = policy_reason_entry_from_finding(finding);
            ("reject".to_string(), vec![reason])
        }
        PolicyDecision::Hold(finding) => {
            let reason = policy_reason_entry_from_finding(finding);
            ("reject".to_string(), vec![reason])
        }
        PolicyDecision::AcknowledgeAndForward(finding) => {
            let reason = policy_reason_entry_from_finding(finding);
            ("accept".to_string(), vec![reason])
        }
        PolicyDecision::Block(finding) => {
            let reason = policy_reason_entry_from_finding(finding);
            ("reject".to_string(), vec![reason])
        }
        PolicyDecision::Ignore(finding) => {
            let reason = policy_reason_entry_from_finding(finding);
            ("reject".to_string(), vec![reason])
        }
    };

    // Build projection preview from state changes (minimal for now).
    let projection = build_projection_preview(&state);

    Ok(CandidateEmitPreview {
        policy_decision,
        reasons,
        projection,
        next_hat_candidates: compute_next_hat_candidates(config, topic),
    })
}

/// Convert a `PolicyFinding` into a structured `PolicyReasonEntry`.
fn policy_reason_entry_from_finding(finding: &PolicyFinding) -> PolicyReasonEntry {
    let (gate, field, reason_code) = match &finding.violation_type {
        ViolationType::PayloadTypeMismatch { expected, actual } => (
            "payload_type".to_string(),
            format!("expected={expected}, actual={actual}"),
            "payload_type_mismatch".to_string(),
        ),
        ViolationType::MissingRequiredField { field } => (
            "required_fields".to_string(),
            field.clone(),
            "missing_required_field".to_string(),
        ),
        ViolationType::InvalidFieldValue { field, value: _ } => (
            "field_value".to_string(),
            field.clone(),
            "invalid_field_value".to_string(),
        ),
        ViolationType::TerminalMonotonicityViolation {
            terminal_topic,
            business_topic,
        } => (
            "terminal_monotonicity".to_string(),
            format!("terminal={terminal_topic}, business={business_topic}"),
            "terminal_monotonicity_violation".to_string(),
        ),
        ViolationType::DuplicateTerminalEvent { topic } => (
            "terminal_duplicate".to_string(),
            topic.clone(),
            "duplicate_terminal_event".to_string(),
        ),
        ViolationType::BusinessEventAfterCompletion { topic } => (
            "completion_guard".to_string(),
            topic.clone(),
            "business_event_after_completion".to_string(),
        ),
        ViolationType::InvalidTopicFormat {
            topic,
            allowed_topics: _,
        } => (
            "topic_format".to_string(),
            topic.clone(),
            "invalid_topic_format".to_string(),
        ),
        ViolationType::TopicDenied {
            rule_hat,
            rule_topic,
        } => (
            "topic_denied".to_string(),
            format!("rule_hat={rule_hat}, topic={rule_topic}"),
            "topic_denied".to_string(),
        ),
        ViolationType::SemanticGateViolation { gate, .. } => (
            "semantic_gate".to_string(),
            gate.clone(),
            "semantic_gate_violation".to_string(),
        ),
        ViolationType::DuplicateWorkDone { key, .. } => (
            "duplicate_work_done".to_string(),
            key.clone(),
            "duplicate_work_done".to_string(),
        ),
    };

    PolicyReasonEntry {
        gate,
        field,
        reason_code,
    }
}

/// Build a projection preview from the policy runtime state after validation.
fn build_projection_preview(state: &PolicyRuntimeState) -> Option<ProjectionPreview> {
    let default = PolicyRuntimeState::default();
    let mut actions = Vec::new();

    if state.terminal_observed != default.terminal_observed {
        actions.push(ProjectionAction {
            field: "terminal_observed".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.terminal_observed),
        });
    }

    if state.completion_honored != default.completion_honored {
        actions.push(ProjectionAction {
            field: "completion_honored".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.completion_honored),
        });
    }

    if state.completion_topic != default.completion_topic {
        actions.push(ProjectionAction {
            field: "completion_topic".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.completion_topic),
        });
    }

    if state.completion_event_index != default.completion_event_index {
        actions.push(ProjectionAction {
            field: "completion_event_index".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.completion_event_index),
        });
    }

    if state.completion_iteration != default.completion_iteration {
        actions.push(ProjectionAction {
            field: "completion_iteration".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.completion_iteration),
        });
    }

    if state.current_plan_name != default.current_plan_name {
        actions.push(ProjectionAction {
            field: "current_plan_name".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.current_plan_name),
        });
    }

    if !state.work_done_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "work_done_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.work_done_seen_keys),
        });
    }

    if !state.work_done_task_id_to_key.is_empty() {
        actions.push(ProjectionAction {
            field: "work_done_task_id_to_key".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.work_done_task_id_to_key),
        });
    }

    if !state.review_dimension_ready_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "review_dimension_ready_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.review_dimension_ready_seen_keys),
        });
    }

    if !state.review_dimensions_complete_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "review_dimensions_complete_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.review_dimensions_complete_seen_keys),
        });
    }

    if !state.work_ready_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "work_ready_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.work_ready_seen_keys),
        });
    }

    if !state.pruned_work_ready_buckets.is_empty() {
        actions.push(ProjectionAction {
            field: "pruned_work_ready_buckets".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.pruned_work_ready_buckets),
        });
    }

    if !state.test_passed_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "test_passed_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.test_passed_seen_keys),
        });
    }

    if !state.test_failed_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "test_failed_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.test_failed_seen_keys),
        });
    }

    if !state.review_start_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "review_start_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.review_start_seen_keys),
        });
    }

    if !state.precheck_proposed_pending_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "precheck_proposed_pending_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.precheck_proposed_pending_keys),
        });
    }

    if state.last_plan_blocked_reason != default.last_plan_blocked_reason {
        actions.push(ProjectionAction {
            field: "last_plan_blocked_reason".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.last_plan_blocked_reason),
        });
    }

    if actions.is_empty() {
        None
    } else {
        Some(ProjectionPreview {
            state_changes: actions,
        })
    }
}

/// Compute which hats receive the event downstream.
pub(crate) fn compute_next_hat_candidates(config: &RalphConfig, topic: &str) -> NextHatCandidates {
    let registry = HatRegistry::from_config(config);

    // Find all hats subscribed to this topic.
    let topic_ref = ralph_proto::Topic::new(topic);
    let subscribers = registry.subscribers(&topic_ref);

    if subscribers.is_empty() {
        return NextHatCandidates::Unverified;
    }

    // Separate subscribers into those that are in config.hats (verified) vs unknown.
    let mut verified_ids = Vec::new();
    let mut entries = Vec::new();

    for hat in subscribers {
        let hat_id_str = hat.id.as_str();
        if config.hats.contains_key(hat_id_str) {
            verified_ids.push(hat_id_str.to_string());
        } else {
            entries.push(CandidateHatEntry {
                hat_id: hat_id_str.to_string(),
                verified: false,
            });
        }
    }

    if entries.is_empty() {
        // All subscribers are known hats → Verified.
        NextHatCandidates::Verified { hats: verified_ids }
    } else {
        // Mixed: some verified, some not.
        // Prepend verified entries to entries list.
        for id in verified_ids.into_iter().rev() {
            entries.insert(
                0,
                CandidateHatEntry {
                    hat_id: id,
                    verified: true,
                },
            );
        }
        NextHatCandidates::Mixed { entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ElementConstraint, EventSchema, HatAllowedValues, TopicDenyRule};
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn test_config() -> EventPolicyConfig {
        EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::RejectWithResume,
            schemas: HashMap::new(),
            terminal_topics: vec!["LOOP_COMPLETE".to_string()],
            business_topics: vec!["experiment.planned".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn test_accept_when_disabled() {
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("{}"), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_accept_valid_json_object() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some(r#"{"key": "value"}"#), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_string_payload_when_json_object_required() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("plain string"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_missing_required_field() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec!["task_key".to_string()],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some(r#"{"other": "value"}"#), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_invalid_allowed_value() {
        let mut config = test_config();
        let mut schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        schema.allowed_values.insert(
            "decision".to_string(),
            vec![
                Value::String("keep".to_string()),
                Value::String("discard".to_string()),
            ],
        );
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event(
            "test",
            Some(r#"{"decision": "blocked"}"#),
            &config,
            &mut state,
        );
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_terminal_then_business_violation() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        // validate_event no longer mutates terminal_observed; caller applies it
        // after all validation layers have passed. We simulate that here.
        state.terminal_observed = true;
        let decision = validate_event("experiment.planned", Some("{}"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_observe_mode_does_not_reject() {
        let mut config = test_config();
        config.mode = EventPolicyMode::Observe;
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("plain string"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::Warn(_)));
    }

    #[test]
    fn test_enforce_reject_with_resume() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("plain string"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_nested_field_extraction() {
        let value = serde_json::json!({"evaluation": {"decision": "keep"}});
        let result = extract_json_field(&value, "evaluation.decision");
        assert_eq!(result, Some(Value::String("keep".to_string())));
    }

    #[test]
    fn test_extract_json_field_nonexistent_path() {
        let value = serde_json::json!({"a": {"b": 1}});
        assert_eq!(extract_json_field(&value, "a.c"), None);
        assert_eq!(extract_json_field(&value, "x.y"), None);
        assert_eq!(extract_json_field(&value, ""), None);
    }

    #[test]
    fn test_extract_json_field_intermediate_non_object() {
        let value = serde_json::json!({"a": [1, 2, 3]});
        assert_eq!(extract_json_field(&value, "a.b"), None);
        let value2 = serde_json::json!({"a": "string"});
        assert_eq!(extract_json_field(&value2, "a.b"), None);
    }

    #[test]
    fn test_required_fields_when_payload_missing() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: None,
            required_fields: vec!["task_key".to_string()],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "Missing payload with required fields should be rejected"
        );
    }

    #[test]
    fn test_nested_allowed_values_validation() {
        let mut config = test_config();
        let mut schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        schema.allowed_values.insert(
            "evaluation.decision".to_string(),
            vec![
                Value::String("keep".to_string()),
                Value::String("discard".to_string()),
            ],
        );
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();

        // Valid nested value
        let decision = validate_event(
            "test",
            Some(r#"{"evaluation": {"decision": "keep"}}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);

        // Invalid nested value
        let decision = validate_event(
            "test",
            Some(r#"{"evaluation": {"decision": "blocked"}}"#),
            &config,
            &mut state,
        );
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_duplicate_terminal_event_violation() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        // Caller sets terminal_observed after the first terminal event passes validation
        state.terminal_observed = true;
        let decision = validate_event("LOOP_COMPLETE", None, &config, &mut state);
        assert!(
            matches!(
                decision,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateTerminalEvent { ref topic },
                    ..
                }) if topic == "LOOP_COMPLETE"
            ),
            "Expected DuplicateTerminalEvent violation, got {:?}",
            decision
        );
    }

    #[test]
    fn test_duplicate_terminal_accepted_when_disabled() {
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        state.terminal_observed = true;
        let decision = validate_event("LOOP_COMPLETE", None, &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_duplicate_terminal_observe_mode_warns() {
        let mut config = test_config();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        state.terminal_observed = true;
        let decision = validate_event("LOOP_COMPLETE", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::Warn(ref findings) if findings.iter().any(|f| matches!(f.violation_type, ViolationType::DuplicateTerminalEvent { .. }))),
            "Expected Warn with DuplicateTerminalEvent, got {:?}",
            decision
        );
    }

    #[test]
    fn test_from_events_replays_terminal_and_business() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"experiment.planned","payload":"{{}}","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let config = test_config();
        let state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();

        assert!(state.terminal_observed);
        assert!(state.observed_topics.contains("experiment.planned"));
        assert!(state.observed_topics.contains("LOOP_COMPLETE"));
    }

    #[test]
    fn test_from_events_payload_compatibility() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        // String payload
        writeln!(
            file,
            r#"{{"topic":"task.start","payload":"Start work","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        // Object payload
        writeln!(
            file,
            r#"{{"topic":"task.done","payload":{{"result":"success"}},"ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        // Null payload
        writeln!(
            file,
            r#"{{"topic":"heartbeat","payload":null,"ts":"2024-01-01T00:00:02Z"}}"#
        )
        .unwrap();
        // Missing payload
        writeln!(file, r#"{{"topic":"noop","ts":"2024-01-01T00:00:03Z"}}"#).unwrap();
        file.flush().unwrap();

        let config = test_config();
        let state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();

        assert_eq!(state.observed_topics.len(), 4);
        assert!(state.observed_topics.contains("task.start"));
        assert!(state.observed_topics.contains("task.done"));
        assert!(state.observed_topics.contains("heartbeat"));
        assert!(state.observed_topics.contains("noop"));
    }

    #[test]
    fn test_from_events_missing_file() {
        let config = test_config();
        let state = PolicyRuntimeState::from_events("/nonexistent/events.jsonl", &config).unwrap();
        assert!(!state.terminal_observed);
        assert!(state.observed_topics.is_empty());
    }

    #[test]
    fn test_from_events_skips_malformed_lines() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"experiment.planned","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(file, "this is not json").unwrap();
        writeln!(
            file,
            r#"{{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let config = test_config();
        let state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();

        assert!(state.terminal_observed);
        assert!(state.observed_topics.contains("experiment.planned"));
        assert!(state.observed_topics.contains("LOOP_COMPLETE"));
    }

    // -------------------------------------------------------------------------
    // Completion honored guard tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_check_completion_honored_inactive_returns_none() {
        let config = test_config();
        let state = PolicyRuntimeState::default();
        assert_eq!(
            check_completion_honored("LOOP_COMPLETE", &config, &state),
            None
        );
        assert_eq!(
            check_completion_honored("experiment.planned", &config, &state),
            None
        );
    }

    #[test]
    fn test_check_completion_honored_warns_duplicate_terminal_by_default() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("LOOP_COMPLETE", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Warn(_))),
            "Expected Warn for duplicate terminal by default, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_honored_warns_business_after_completion_by_default() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("experiment.planned", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Warn(_))),
            "Expected Warn for business after completion by default, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_honored_allows_unrelated_events() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        assert_eq!(
            check_completion_honored("task.resume", &config, &state),
            None
        );
    }

    #[test]
    fn test_check_completion_honored_ignore_action() {
        let mut config = test_config();
        config.completion_after_terminal.duplicate_terminal = CompletionAfterTerminalAction::Ignore;
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("LOOP_COMPLETE", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Ignore(_))),
            "Expected Ignore, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_honored_warn_action() {
        let mut config = test_config();
        config.completion_after_terminal.business_after_completion =
            CompletionAfterTerminalAction::Warn;
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("experiment.planned", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Warn(_))),
            "Expected Warn, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_guard_respects_guard_active_flag() {
        let config = test_config();
        assert_eq!(
            check_completion_guard("LOOP_COMPLETE", &config, false),
            None
        );
        assert!(matches!(
            check_completion_guard("LOOP_COMPLETE", &config, true),
            Some(PolicyDecision::Warn(_))
        ));
    }

    // -------------------------------------------------------------------------
    // Shared fixture tests (U6)
    // -------------------------------------------------------------------------

    const FIXTURE_VALID_CHAIN: &str = r#"{"topic":"experiment.planned","payload":{"task_key":"a","hypothesis":"h","falsification_condition":"f"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:01Z"}"#;

    const FIXTURE_DUPLICATE_TERMINAL: &str = r#"{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"LOOP_COMPLETE","payload":{"reason":"retry"},"ts":"2026-05-22T00:00:01Z"}"#;

    const FIXTURE_BUSINESS_AFTER_TERMINAL: &str = r#"{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"experiment.planned","payload":{"task_key":"b","hypothesis":"h","falsification_condition":"f"},"ts":"2026-05-22T00:00:01Z"}"#;

    const FIXTURE_MISSING_REQUIRED_FIELDS: &str =
        r#"{"topic":"experiment.planned","payload":{"task_key":"a"},"ts":"2026-05-22T00:00:00Z"}"#;

    fn fixture_config() -> EventPolicyConfig {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![
                "task_key".to_string(),
                "hypothesis".to_string(),
                "falsification_condition".to_string(),
            ],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config
            .schemas
            .insert("experiment.planned".to_string(), schema);
        config.completion_after_terminal.duplicate_terminal = CompletionAfterTerminalAction::Reject;
        config.completion_after_terminal.business_after_completion =
            CompletionAfterTerminalAction::Reject;
        config
    }

    fn parse_fixture_line(line: &str) -> (String, Option<String>) {
        let event: crate::event_reader::Event =
            serde_json::from_str(line).expect("valid fixture line");
        (event.topic, event.payload)
    }

    fn is_accept(decision: &PolicyDecision) -> bool {
        matches!(decision, PolicyDecision::Accept)
    }

    /// Write all lines except the last to a temp file, replay state, then validate the last line.
    fn replay_and_validate(fixture: &str) -> (PolicyRuntimeState, PolicyDecision) {
        let config = fixture_config();
        let lines: Vec<&str> = fixture.lines().collect();
        let mut file = NamedTempFile::new().unwrap();
        for line in &lines[..lines.len().saturating_sub(1)] {
            writeln!(file, "{}", line).unwrap();
        }
        file.flush().unwrap();
        let mut state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();
        // Simulate the event loop marking completion as honored once a terminal
        // event has been observed in the replayed history.
        if state.terminal_observed {
            state.completion_honored = true;
        }
        let (topic, payload) = parse_fixture_line(lines.last().unwrap());
        let decision = validate_event(&topic, payload.as_deref(), &config, &mut state);
        (state, decision)
    }

    #[test]
    fn test_fixture_valid_chain_accepted() {
        let (_, decision) = replay_and_validate(FIXTURE_VALID_CHAIN);
        assert!(
            is_accept(&decision),
            "Expected Accept for valid chain, got {:?}",
            decision
        );
    }

    #[test]
    fn test_fixture_duplicate_terminal_rejected_or_ignored() {
        let (_, decision) = replay_and_validate(FIXTURE_DUPLICATE_TERMINAL);
        assert!(
            !is_accept(&decision),
            "Expected reject/ignore for duplicate terminal, got {:?}",
            decision
        );
    }

    #[test]
    fn test_fixture_business_after_terminal_rejected_or_ignored() {
        let (_, decision) = replay_and_validate(FIXTURE_BUSINESS_AFTER_TERMINAL);
        assert!(
            !is_accept(&decision),
            "Expected reject/ignore for business after terminal, got {:?}",
            decision
        );
    }

    #[test]
    fn test_fixture_missing_required_fields_rejected_when_strict() {
        let config = fixture_config();
        let mut state =
            PolicyRuntimeState::from_events("/nonexistent/events.jsonl", &config).unwrap();
        let (topic, payload) = parse_fixture_line(FIXTURE_MISSING_REQUIRED_FIELDS);
        let decision = validate_event(&topic, payload.as_deref(), &config, &mut state);
        assert!(
            !is_accept(&decision),
            "Expected reject for missing provenance under strict config, got {:?}",
            decision
        );
    }

    #[test]
    fn test_provenance_fields_preserved_by_reader() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"experiment.planned","payload":{{"task_key":"x"}},"ts":"2024-01-01T00:00:00Z","hat":"strategist","triggered":"implementer","source":"cli"}}"#
        ).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 1);
        let event = &result.events[0];
        assert_eq!(event.hat, Some("strategist".to_string()));
        assert_eq!(event.triggered, Some("implementer".to_string()));
        assert_eq!(event.source, Some("cli".to_string()));
    }

    #[test]
    fn test_old_simple_event_fixtures_still_parse() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"task.start","payload":"Start work","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"task.done","payload":null,"ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        writeln!(file, r#"{{"topic":"noop","ts":"2024-01-01T00:00:02Z"}}"#).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 3);
        assert_eq!(result.events[0].topic, "task.start");
        assert_eq!(result.events[0].payload, Some("Start work".to_string()));
        assert!(result.events[1].payload.is_none());
        assert!(result.events[2].payload.is_none());
    }

    // -------------------------------------------------------------------------
    // Topic format check tests (U5)
    // -------------------------------------------------------------------------

    #[test]
    fn test_check_topic_format_accepts_whitelisted_topic() {
        let mut allowed = HashSet::new();
        allowed.insert("work.done".to_string());
        allowed.insert("review.passed".to_string());
        assert_eq!(check_topic_format("work.done", &allowed), None);
        assert_eq!(check_topic_format("review.passed", &allowed), None);
    }

    #[test]
    fn test_check_topic_format_rejects_unknown_topic() {
        let mut allowed = HashSet::new();
        allowed.insert("work.done".to_string());
        let result = check_topic_format("REVIEW_COMPLETE", &allowed);
        assert!(result.is_some());
        let decision = result.unwrap();
        assert!(matches!(decision, PolicyDecision::Block(_)));
    }

    #[test]
    fn test_check_topic_format_rejects_uppercase_topic() {
        let mut allowed = HashSet::new();
        allowed.insert("work.done".to_string());
        // AE2: uppercase topic is rejected
        let result = check_topic_format("LOOP_COMPLETE", &allowed);
        assert!(result.is_some());
        let decision = result.unwrap();
        match decision {
            PolicyDecision::Block(finding) => {
                assert!(matches!(
                    finding.violation_type,
                    ViolationType::InvalidTopicFormat { .. }
                ));
            }
            _ => panic!("Expected Block decision"),
        }
    }

    #[test]
    fn test_check_topic_format_accepts_loop_complete_when_whitelisted() {
        // AE5: whitelisted completion token is accepted
        let mut allowed = HashSet::new();
        allowed.insert("LOOP_COMPLETE".to_string());
        assert_eq!(check_topic_format("LOOP_COMPLETE", &allowed), None);
    }

    #[test]
    fn test_is_system_topic_event_prefix() {
        assert!(is_system_topic("event.malformed"));
        assert!(is_system_topic("event.scope_violation"));
        assert!(is_system_topic("event.policy_warning"));
        assert!(!is_system_topic("work.done"));
        assert!(!is_system_topic("review.passed"));
    }

    #[test]
    fn test_is_system_topic_human_prefix() {
        assert!(is_system_topic("human.guidance"));
        assert!(!is_system_topic("humanx.guidance")); // no dot after prefix
    }

    #[test]
    fn test_build_allowed_topics_includes_hat_publishes() {
        let mut hats = std::collections::HashMap::new();
        let mut hat_config = crate::config::HatConfig::default();
        hat_config.publishes = vec!["work.done".to_string(), "review.passed".to_string()];
        hats.insert("executor".to_string(), hat_config);

        let allowed = build_allowed_topics(&hats, "LOOP_COMPLETE", None);
        assert!(allowed.contains("work.done"));
        assert!(allowed.contains("review.passed"));
        assert!(allowed.contains("LOOP_COMPLETE"));
        assert!(allowed.contains("loop.cancel"));
        assert!(allowed.contains("task.resume"));
        assert!(allowed.contains("build.task.abandoned"));
    }

    #[test]
    fn test_build_allowed_topics_empty_hats() {
        let hats = std::collections::HashMap::new();
        let allowed = build_allowed_topics(&hats, "LOOP_COMPLETE", None);
        // Only system topics
        assert!(allowed.contains("LOOP_COMPLETE"));
        assert!(allowed.contains("loop.cancel"));
        assert!(allowed.contains("task.resume"));
        assert!(allowed.contains("build.task.abandoned"));
        assert!(!allowed.contains("work.done"));
    }

    #[test]
    fn test_build_allowed_topics_includes_event_policy_topics() {
        let hats = std::collections::HashMap::new();
        let policy = EventPolicyConfig {
            terminal_topics: vec!["review.file".to_string()],
            business_topics: vec!["task.update".to_string()],
            ..Default::default()
        };
        let allowed = build_allowed_topics(&hats, "LOOP_COMPLETE", Some(&policy));
        assert!(allowed.contains("review.file"));
        assert!(allowed.contains("task.update"));
        assert!(allowed.contains("LOOP_COMPLETE"));
    }

    // P2 #20: regression guard for `is_system_topic` short-circuit.
    //
    // The `build_allowed_topics` doc (line 235-238) explicitly states
    // that `event.*` and `human.*` topics are NOT inserted into the
    // allowed-topics set; they are admitted by the `is_system_topic()`
    // short-circuit, which the event loop applies BEFORE
    // `check_topic_format`. If a future refactor ever:
    //
    // (a) reorders the event-loop partition so `check_topic_format` runs
    //     first, OR
    // (b) removes the `is_system_topic` short-circuit (e.g. by trying to
    //     be "uniform" with the rest of the validation), OR
    // (c) starts inserting `event.*` / `human.*` as prefix members into
    //     `allowed_topics`,
    //
    // then `event.*` / `human.*` topics that have NEVER been declared
    // anywhere would start failing format checks. The two halves of the
    // contract (`is_system_topic` admits unknown system topics;
    // `check_topic_format` rejects unknown business topics) must stay
    // disjoint and applied in the documented order.
    //
    // This test pins both halves together by simulating the event-loop
    // validation flow as a single composed operation and asserting that
    // a "rogue" system topic (uppercase, would otherwise fail
    // `check_topic_format`) is admitted ONLY when `is_system_topic` is
    // consulted first.
    #[test]
    fn system_topic_short_circuit_runs_before_format_check() {
        // Empty whitelist — `check_topic_format` would reject ANY non-empty
        // topic that is not in the whitelist.
        let allowed = build_allowed_topics(&HashMap::new(), "LOOP_COMPLETE", None);

        // A topic that:
        //   - has uppercase letters → would normally fail format checks
        //   - is an `event.*` topic → admitted by `is_system_topic`
        //   - is NOT in the whitelist (and never will be, by U3 design)
        let rogue_system_topic = "event.foo.BAR";

        // Sanity: the system-topic short-circuit admits it.
        assert!(
            is_system_topic(rogue_system_topic),
            "test premise: '{rogue_system_topic}' must satisfy is_system_topic"
        );

        // Sanity: `check_topic_format` would reject it on its own — this
        // is the whole reason we need the short-circuit.
        assert!(
            check_topic_format(rogue_system_topic, &allowed).is_some(),
            "test premise: '{rogue_system_topic}' must be rejected by check_topic_format \
             when called in isolation, so that the short-circuit is load-bearing"
        );

        // Now compose the two checks in the documented order
        // (`is_system_topic` → `check_topic_format`). The composed
        // operation MUST accept the system topic even though
        // `check_topic_format` alone would reject it.
        let composed_admits = |topic: &str| -> bool {
            if is_system_topic(topic) {
                return true;
            }
            check_topic_format(topic, &allowed).is_none()
        };
        assert!(
            composed_admits(rogue_system_topic),
            "composed validation (is_system_topic → check_topic_format) must admit \
             '{rogue_system_topic}' — this is the order documented in build_allowed_topics"
        );

        // A non-system rogue topic (uppercase business topic) must STILL
        // be rejected by the composed operation — proving we did not
        // accidentally turn the short-circuit into a blanket bypass.
        let rogue_business_topic = "WORK.DONE.WITH_UPPERCASE";
        assert!(!is_system_topic(rogue_business_topic));
        assert!(
            !composed_admits(rogue_business_topic),
            "composed validation must still reject unknown business topics; \
             the short-circuit is for system topics only"
        );

        // And a well-formed business topic that's in the whitelist must
        // still be admitted — proving `check_topic_format` is still
        // doing its real job on the non-system side. Add "work.done"
        // to the whitelist to exercise the admit path explicitly.
        let mut allowed_with_work = allowed.clone();
        allowed_with_work.insert("work.done".to_string());
        let composed_admits_work = |topic: &str| -> bool {
            if is_system_topic(topic) {
                return true;
            }
            check_topic_format(topic, &allowed_with_work).is_none()
        };
        assert!(
            composed_admits_work("work.done"),
            "composed validation must admit whitelisted business topics"
        );
    }

    // -------------------------------------------------------------------------
    // U3: topic-deny rules tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_topic_deny_rules_match_rejected() {
        // Matching deny rule → Block when mode=Enforce
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            }],
            ..Default::default()
        };
        let decision = check_topic_deny_rules(Some("executor"), "build.done", &config);
        assert!(matches!(decision, Some(PolicyDecision::Block(_))));
    }

    // 2026-06-30 P0-2 (primary-20260629-170451 diagnosis):
    // System control topics (`loop.cancel`, `task.resume`,
    // `build.task.abandoned`) must NEVER be matched against
    // `topic_deny_rules` — the loop runner injects them and
    // the originating hat field can fall on a hat that the
    // preset declared a deny rule for (validator, executor,
    // coordinator etc. are all on the deny list for
    // `task.resume` in `ce-executor-serial`). Without the
    // short-circuit `primary-20260629-170451` rejected the
    // stall-recovery `task.resume` twice, blocked the loop
    // from advancing past fix-02, and terminated the loop on
    // `consecutive_failures` despite the events file
    // capturing every retry. The denylist still applies to
    // business topics — only the runner-published topics
    // are short-circuited.
    #[test]
    fn test_p0_2_system_control_topics_short_circuit_deny_rules() {
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![
                TopicDenyRule {
                    hat_id: "validator".to_string(),
                    topic: "task.resume".to_string(),
                },
                TopicDenyRule {
                    hat_id: "executor".to_string(),
                    topic: "task.resume".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "loop.cancel".to_string(),
                },
                TopicDenyRule {
                    hat_id: "shipper".to_string(),
                    topic: "build.task.abandoned".to_string(),
                },
            ],
            ..Default::default()
        };
        assert!(
            check_topic_deny_rules(Some("validator"), "task.resume", &config).is_none(),
            "P0-2: task.resume must be admitted for every hat — runner injection"
        );
        assert!(
            check_topic_deny_rules(Some("executor"), "task.resume", &config).is_none(),
            "P0-2: task.resume short-circuit is independent of originating hat"
        );
        assert!(
            check_topic_deny_rules(Some("ralph"), "loop.cancel", &config).is_none(),
            "P0-2: loop.cancel short-circuit must preempt the ralph deny rule"
        );
        assert!(
            check_topic_deny_rules(Some("shipper"), "build.task.abandoned", &config).is_none(),
            "P0-2: build.task.abandoned short-circuit must preempt the shipper deny rule"
        );
        // Sanity: the short-circuit is precisely scoped.
        // A business topic still matches its deny rule.
        let config_with_business_block = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            }],
            ..Default::default()
        };
        assert!(
            check_topic_deny_rules(Some("executor"), "build.done", &config_with_business_block)
                .is_some(),
            "deny rules still fire for business topics"
        );
    }

    #[test]
    fn test_topic_deny_rules_non_matching_accepted() {
        // Non-matching hat_id → None (allowed)
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            }],
            ..Default::default()
        };
        // Different hat, same topic → no match
        assert!(check_topic_deny_rules(Some("reviewer"), "build.done", &config).is_none());
        // Same hat, different topic → no match
        assert!(check_topic_deny_rules(Some("executor"), "work.done", &config).is_none());
        // No hat → no match (empty string not matched)
        assert!(check_topic_deny_rules(None, "build.done", &config).is_none());
    }

    #[test]
    fn test_topic_deny_rules_observe_mode_warns() {
        // Observe mode → Warn even when rule matches
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Observe,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            }],
            ..Default::default()
        };
        let decision = check_topic_deny_rules(Some("executor"), "build.done", &config);
        assert!(matches!(decision, Some(PolicyDecision::Warn(_))));
    }

    // -------------------------------------------------------------------------
    // U4: review.passed skip_reason allowlist + ralph topic_deny_rules
    // (mirrors the three edits in `presets/en/ce-executor.yml`).
    // -------------------------------------------------------------------------

    fn review_passed_allowlist_config() -> EventPolicyConfig {
        let mut config = test_config();
        let mut schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![
                "plan_name".into(),
                "task_id".into(),
                "task_key".into(),
                "step".into(),
                "findings_count".into(),
                "fix_round".into(),
                "verdict".into(),
                "skip_reason".into(),
            ],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        // Mirror the ce-executor.yml U4 allowlist exactly.
        schema.allowed_values.insert(
            "skip_reason".to_string(),
            vec![
                Value::String("empty_diff".to_string()),
                Value::String("trivial_step".to_string()),
                Value::String("aggregate_timeout".to_string()),
            ],
        );
        // U8: hat-aware restrictions mirror the preset.
        schema.hat_allowed_values.insert(
            "skip_reason".to_string(),
            vec![
                HatAllowedValues {
                    hat_id: "review-coordinator".to_string(),
                    values: vec![Value::String("empty_diff".to_string())],
                },
                HatAllowedValues {
                    hat_id: "review-synthesizer".to_string(),
                    values: vec![Value::String("aggregate_timeout".to_string())],
                },
            ],
        );
        config.schemas.insert("review.passed".to_string(), schema);
        config
    }

    #[test]
    fn test_u4_review_passed_skip_reason_allowlist_accepts_legal_values() {
        let config = review_passed_allowlist_config();
        // Each legal value is paired with a hat that allows it (per the
        // hat_allowed_values below). `trivial_step` is allowed by the
        // global allowlist only — no hat-specific entry — so the
        // hat-aware check skips and the value passes through the
        // global allowlist. With a hat, the check passes when the
        // value is either: (a) in the hat's per-hat list, or (b) not
        // restricted per-hat (i.e. the schema has no entry for that
        // hat, in which case only the global allowlist applies).
        let cases: &[(&str, &str)] = &[
            ("empty_diff", "review-coordinator"),
            ("aggregate_timeout", "review-synthesizer"),
            // trivial_step is in the global allowlist but not in any
            // hat-specific entry. The hat-aware block only fires when
            // the schema has a rule for the emitting hat; pick a hat
            // without a per-hat rule (the schema only has rules for
            // review-coordinator / review-synthesizer, so use any
            // other hat id to exercise the "no rule → skip" branch).
            ("trivial_step", "executor"),
        ];
        for (legal, hat_id) in cases {
            let payload = format!(
                r#"{{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"{legal}"}}"#
            );
            let mut state = PolicyRuntimeState::default();
            let decision = validate_event_with_hat(
                "review.passed",
                Some(&payload),
                &config,
                &mut state,
                Some(hat_id),
            );
            assert_eq!(
                decision,
                PolicyDecision::Accept,
                "skip_reason='{legal}' with hat='{hat_id}' should be accepted by the allowlist, got {:?}",
                decision
            );
        }
    }

    #[test]
    fn test_u4_review_passed_skip_reason_allowlist_rejects_fabricated() {
        // The P1 root cause: review-synthesizer invented
        // `dimension_reviewer_no_response` as a skip_reason when the
        // aggregate timeout fired. Without the allowlist this passes
        // the required_fields gate. U4 closes that hole.
        let config = review_passed_allowlist_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"dimension_reviewer_no_response"}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "fabricated skip_reason must be rejected, got {:?}",
            decision
        );
    }

    #[test]
    fn test_u4_review_passed_skip_reason_allowlist_rejects_empty_string() {
        let config = review_passed_allowlist_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":""}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_u8_review_passed_hat_aware_allowed_values() {
        let config = review_passed_allowlist_config();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"aggregate_timeout"}"#;

        // review-coordinator may only use skip_reason='empty_diff'.
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_hat(
            "review.passed",
            Some(payload),
            &config,
            &mut state,
            Some("review-coordinator"),
        );
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "review-coordinator emitting review.passed(skip_reason=aggregate_timeout) must be rejected, got {:?}",
            decision
        );

        // review-synthesizer may use skip_reason='aggregate_timeout'.
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_hat(
            "review.passed",
            Some(payload),
            &config,
            &mut state,
            Some("review-synthesizer"),
        );
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "review-synthesizer emitting review.passed(skip_reason=aggregate_timeout) must be accepted, got {:?}",
            decision
        );

        // U1 (2026-06-17-004 plan, R2): no hat provided + schema has
        // hat_allowed_values → fail-closed with a MissingRequiredField
        // finding. The CLI emit pipeline's `check_emit_provenance` gate
        // rejects hat-less business-topic emits earlier; this test pins
        // the programmatic-caller contract (validate_event / API
        // server path) so the old "skip hat-aware when None" behavior
        // cannot silently re-appear. The `validate_event` convenience
        // wraps `validate_event_with_hat(..., None)` so it inherits
        // the same fail-closed semantics.
        let mut state = PolicyRuntimeState::default();
        let decision =
            validate_event_with_hat("review.passed", Some(payload), &config, &mut state, None);
        match decision {
            PolicyDecision::RejectWithResume(finding) => {
                assert!(
                    matches!(
                        finding.violation_type,
                        ViolationType::MissingRequiredField { .. }
                    ),
                    "no-hat + hat_allowed_values must yield MissingRequiredField, got {:?}",
                    finding.violation_type
                );
                assert!(
                    finding.message.contains("hat-specific allowed values"),
                    "message must explain the provenance requirement, got: {}",
                    finding.message
                );
            }
            other => panic!(
                "no-hat + hat_allowed_values must be rejected (fail-closed), got {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_u4_topic_deny_rules_ralph_blocked_from_workflow_topics() {
        // Mirrors the five new deny rules in ce-executor.yml:
        //   {hat_id: ralph, topic: review.wave.ready / review.passed /
        //    queue.advance / plan.complete / plan.blocked}
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "review.wave.ready".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "review.passed".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "queue.advance".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "plan.complete".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "plan.blocked".to_string(),
                },
            ],
            ..Default::default()
        };
        for topic in [
            "review.wave.ready",
            "review.passed",
            "queue.advance",
            "plan.complete",
            "plan.blocked",
        ] {
            let decision = check_topic_deny_rules(Some("ralph"), topic, &config);
            assert!(
                matches!(decision, Some(PolicyDecision::Block(_))),
                "ralph must be blocked from '{topic}', got {:?}",
                decision
            );
        }
    }

    #[test]
    fn test_u4_topic_deny_rules_ralph_unchanged_for_control_topics() {
        // Control topics (e.g. task.resume, LOOP_COMPLETE) must NOT be
        // blocked for ralph — they are ralph's legitimate surface.
        // The ralph deny list only covers business topics.
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            topic_deny_rules: vec![
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "review.wave.ready".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "review.passed".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "queue.advance".to_string(),
                },
            ],
            ..Default::default()
        };
        assert!(check_topic_deny_rules(Some("ralph"), "task.resume", &config).is_none());
        assert!(check_topic_deny_rules(Some("ralph"), "LOOP_COMPLETE", &config).is_none());
        assert!(check_topic_deny_rules(Some("ralph"), "human.guidance", &config).is_none());
    }

    #[test]
    fn test_u4_topic_deny_rules_executor_build_done_preserved() {
        // Regression: the original `executor → build.done` deny rule must
        // still fire after the U4 additions. Otherwise a worktree-loop
        // executor could impersonate the review-synthesizer again.
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![
                TopicDenyRule {
                    hat_id: "executor".to_string(),
                    topic: "build.done".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "review.passed".to_string(),
                },
            ],
            ..Default::default()
        };
        assert!(matches!(
            check_topic_deny_rules(Some("executor"), "build.done", &config),
            Some(PolicyDecision::Block(_))
        ));
        // And the new ralph rule still fires.
        assert!(matches!(
            check_topic_deny_rules(Some("ralph"), "review.passed", &config),
            Some(PolicyDecision::Block(_))
        ));
    }

    #[test]
    fn test_topic_deny_rules_glob_pattern_matches() {
        // Glob pattern `debug.*` matches `debug.step`, `debug.done`, etc.
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::RejectWithResume,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "coordinator".to_string(),
                topic: "debug.*".to_string(),
            }],
            ..Default::default()
        };
        // Segment wildcard matches
        assert!(matches!(
            check_topic_deny_rules(Some("coordinator"), "debug.step", &config),
            Some(PolicyDecision::RejectWithResume(_))
        ));
        assert!(matches!(
            check_topic_deny_rules(Some("coordinator"), "debug.done", &config),
            Some(PolicyDecision::RejectWithResume(_))
        ));
        // Non-matching topic not matched
        assert!(check_topic_deny_rules(Some("coordinator"), "debug", &config).is_none());
        // Non-matching hat not matched
        assert!(check_topic_deny_rules(Some("executor"), "debug.step", &config).is_none());
    }

    #[test]
    fn test_topic_deny_rules_glob_exact_overlap() {
        // When glob and exact rule both exist for same hat, first match wins.
        // Exact rule for `build.done` and glob rule for `debug.*` on coordinator.
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![
                TopicDenyRule {
                    hat_id: "coordinator".to_string(),
                    topic: "build.done".to_string(),
                },
                TopicDenyRule {
                    hat_id: "coordinator".to_string(),
                    topic: "debug.*".to_string(),
                },
            ],
            ..Default::default()
        };
        let decision = check_topic_deny_rules(Some("coordinator"), "build.done", &config);
        // Exact match found first (Block, not RejectWithResume from glob)
        assert!(matches!(decision, Some(PolicyDecision::Block(_))));
    }

    // -------------------------------------------------------------------------
    // U4: plan_name equality tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_plan_name_equality_matches_accepted() {
        // work.ready with plan_name=A → work.done with plan_name=A → Accept
        let mut config = test_config();
        config.plan_name_equality_required = true;
        let mut state = PolicyRuntimeState::default();
        state.current_plan_name = Some("plan-x".to_string());

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "plan-x"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_plan_name_equality_mismatch_rejected() {
        // work.ready with plan_name=A → work.done with plan_name=B → Reject
        let mut config = test_config();
        config.plan_name_equality_required = true;
        let mut state = PolicyRuntimeState::default();
        state.current_plan_name = Some("plan-x".to_string());

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "plan-y"}"#),
            &config,
            &mut state,
        );
        let is_rejected = matches!(decision, PolicyDecision::RejectWithResume(PolicyFinding {
            violation_type: ViolationType::InvalidFieldValue { ref field, .. }, ..
        }) if field == "plan_name");
        assert!(
            is_rejected,
            "Expected RejectWithResume for plan_name mismatch, got {:?}",
            decision
        );
    }

    #[test]
    fn test_plan_name_equality_disabled_accepts_mismatch() {
        // plan_name_equality_required=false (default) → work.done plan_name=B still accepted
        let config = test_config(); // default has plan_name_equality_required=false
        let mut state = PolicyRuntimeState::default();
        state.current_plan_name = Some("plan-x".to_string());

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "plan-y"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_plan_name_equality_no_work_ready_skips_check() {
        // No work.ready → current_plan_name is None → skip check
        let mut config = test_config();
        config.plan_name_equality_required = true;
        let mut state = PolicyRuntimeState::default();
        // current_plan_name is None (no work.ready received)

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "anything"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    // -------------------------------------------------------------------------
    // U4 (2026-06-17-003 plan): duplicate `work.done` dedup tests
    //
    // Same `(plan_name, step, task_id)` tuple — 2nd `work.done` is
    // rejected with `RecoverableRejection` (NOT fatal).
    // -------------------------------------------------------------------------

    fn work_done_payload(plan: &str, step: &str, task: &str) -> String {
        format!(r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","task_key":"k"}}"#)
    }

    #[test]
    fn test_u4_duplicate_work_done_first_accepted() {
        // Happy path: first `work.done` for a (plan, step, task) tuple is accepted.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = work_done_payload("p1", "step-01", "t1");
        let decision = validate_event("work.done", Some(&payload), &config, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "First work.done for a new (plan, step, task) tuple must be accepted"
        );
        // The dedup key should now be in the per-batch set
        assert!(state.work_done_seen_keys.contains("p1::step-01::t1"));
    }

    #[test]
    fn test_u4_duplicate_work_done_second_rejected() {
        // Error path: 2nd `work.done` with the same (plan, step, task)
        // tuple is rejected with `RejectWithResume` (RecoverableRejection
        // — the policy validator routes it through the recoverable bucket).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = work_done_payload("p1", "step-01", "t1");

        // First emit: accepted
        let first = validate_event("work.done", Some(&payload), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        // Second emit (same key, same batch): rejected
        let second = validate_event("work.done", Some(&payload), &config, &mut state);
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "p1::step-01::t1"
            ),
            "Second work.done for same key must be rejected with DuplicateWorkDone, got {:?}",
            second
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_different_step_accepted() {
        // Edge case: same (plan, task_id) but different `step` key →
        // still accepted (key includes step).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        // step-01 emit
        let p1 = work_done_payload("p1", "step-01", "t1");
        let first = validate_event("work.done", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        // Same task, different step: accepted
        let p2 = work_done_payload("p1", "step-02", "t1");
        let second = validate_event("work.done", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "work.done for same task but different step must be accepted, got {:?}",
            second
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_different_task_accepted() {
        // Edge case: same (plan, step) but different `task_id` →
        // still accepted (key includes task_id).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let p1 = work_done_payload("p1", "step-01", "t1");
        let first = validate_event("work.done", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let p2 = work_done_payload("p1", "step-01", "t2");
        let second = validate_event("work.done", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "work.done for same step but different task must be accepted, got {:?}",
            second
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_is_recoverable() {
        // The DuplicateWorkDone violation must be in the recoverable
        // bucket (R-B1) so the runner publishes a `task.resume` with
        // `fix_hint` instead of the U6 fast-fail.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = work_done_payload("p1", "step-01", "t1");
        let first = validate_event("work.done", Some(&payload), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event("work.done", Some(&payload), &config, &mut state);
        let finding = match second {
            PolicyDecision::RejectWithResume(f) => f,
            other => panic!("expected RejectWithResume, got {:?}", other),
        };
        let class = is_recoverable_policy_finding(&finding);
        assert_eq!(
            class,
            Some(ReasonClass::DuplicateWorkDone),
            "DuplicateWorkDone must map to the recoverable bucket, got {:?}",
            class
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_disabled_policy_accepts_all() {
        // When event policy is disabled, the dedup check must be
        // skipped (mirrors all other policy checks).
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        let payload = work_done_payload("p1", "step-01", "t1");

        let first = validate_event("work.done", Some(&payload), &config, &mut state);
        let second = validate_event("work.done", Some(&payload), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "disabled policy must not dedup, got {:?}",
            second
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_missing_fields_skips_dedup() {
        // If the payload is missing plan_name/step/task_id, the dedup
        // check cannot run — fall through to other policy layers.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"task_key":"k"}"#; // missing plan_name/step/task_id
        let first = validate_event("work.done", Some(payload), &config, &mut state);
        // First emit: not rejected by dedup (no key to compare).
        // May be rejected by other policies (e.g. required fields), but
        // the dedup violation type must NOT appear.
        if let PolicyDecision::RejectWithResume(f) = &first {
            assert!(
                !matches!(f.violation_type, ViolationType::DuplicateWorkDone { .. }),
                "missing-fields payload must not trigger DuplicateWorkDone, got {:?}",
                f.violation_type
            );
        }
    }

    #[test]
    fn test_u4_duplicate_work_done_hint_mapped_to_reason_code() {
        // U3 of plan 2026-07-05-005 (fix-plan §R3): restore the
        // single `duplicate_work_done` reason_code for both
        // `DuplicateSameStep` and `DuplicateStallBypass` per KTD-3.
        // The `hint` field on `RecoveryDiagnosisEnvelope` carries
        // the disambiguation so post-mortem tooling can still
        // distinguish the two paths.
        let same_step = PolicyFinding {
            topic: "work.done".to_string(),
            violation_type: ViolationType::DuplicateWorkDone {
                key: "p::s::t".to_string(),
                hint: DuplicateWorkDoneHint::DuplicateSameStep,
                seen_count: None,
            },
            message: "test".to_string(),
        };
        assert_eq!(
            same_step.violation_type.reason_code(),
            "duplicate_work_done",
            "U3: DuplicateSameStep must surface as duplicate_work_done (hint carries the discriminator)"
        );
        assert_eq!(
            DuplicateWorkDoneHint::DuplicateSameStep.as_hint_str(),
            "duplicate_work_done_same_step",
            "U3: DuplicateSameStep hint string stays stable for recovery envelope"
        );
        let stall = PolicyFinding {
            topic: "work.done".to_string(),
            violation_type: ViolationType::DuplicateWorkDone {
                key: "p::s::t".to_string(),
                hint: DuplicateWorkDoneHint::DuplicateStallBypass,
                seen_count: None,
            },
            message: "test".to_string(),
        };
        assert_eq!(
            stall.violation_type.reason_code(),
            "duplicate_work_done",
            "U3: DuplicateStallBypass must surface as duplicate_work_done (hint carries the discriminator)"
        );
        assert_eq!(
            DuplicateWorkDoneHint::DuplicateStallBypass.as_hint_str(),
            "duplicate_work_done_stall_bypass",
            "U3: DuplicateStallBypass hint string stays stable for recovery envelope"
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_hint_distinct() {
        // The two hints are distinct enum values so the runtime can
        // branch on them.
        assert_ne!(
            DuplicateWorkDoneHint::DuplicateSameStep,
            DuplicateWorkDoneHint::DuplicateStallBypass
        );
    }

    // -------------------------------------------------------------------------
    // U5 (2026-06-17-003 plan, R6): `review.dimension.ready` dedup
    //
    // Mirrors the U4 work.done dedup pattern. Key is
    // `(plan_name, step, task_id, dimension)`. A 2nd emit with
    // the same key is rejected as `DuplicateWorkDone` (variant
    // reused for retry-key parity).
    // -------------------------------------------------------------------------

    fn review_dimension_ready_payload(plan: &str, step: &str, task: &str, dim: &str) -> String {
        format!(
            r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","dimension":"{dim}","wave_id":"w1"}}"#
        )
    }

    fn review_start_payload(plan: &str, step: Option<&str>, task: &str) -> String {
        if let Some(st) = step {
            format!(
                r#"{{"plan_name":"{plan}","step":"{st}","task_id":"{task}","task_key":"k-{task}"}}"#
            )
        } else {
            format!(r#"{{"plan_name":"{plan}","task_id":"{task}","task_key":"k-{task}"}}"#)
        }
    }

    #[test]
    fn review_dimension_ready_dedup_first_accepted() {
        // Happy path: first `review.dimension.ready` for a
        // (plan, step, task, dimension) tuple is accepted.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let decision = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "First review.dimension.ready for a new key must be accepted"
        );
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness")
        );
    }

    #[test]
    fn review_dimension_ready_dedup_rejects_second_emit() {
        // Error path: 2nd `review.dimension.ready` with the
        // same (plan, step, task, dimension) tuple is rejected
        // with `RejectWithResume`.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");

        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "p1::step-01::t1::correctness"
            ),
            "Second review.dimension.ready for same key must be rejected with DuplicateWorkDone, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_different_dimensions_both_accepted() {
        // Edge case: same (plan, step, task) but different
        // `dimension` → both accepted (serial walk through
        // review dimensions).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let p1 = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let first = validate_event("review.dimension.ready", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let p2 = review_dimension_ready_payload("p1", "step-01", "t1", "security");
        let second = validate_event("review.dimension.ready", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "review.dimension.ready for same task but different dimension must be accepted, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_different_step_accepted() {
        // Edge case: same (plan, task, dimension) but different
        // `step` → still accepted (key includes step).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let p1 = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let first = validate_event("review.dimension.ready", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let p2 = review_dimension_ready_payload("p1", "step-02", "t1", "correctness");
        let second = validate_event("review.dimension.ready", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "review.dimension.ready for same dim but different step must be accepted, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_is_recoverable() {
        // The DuplicateWorkDone violation (reused for
        // review.dimension.ready) must map to the recoverable
        // bucket so the runner publishes a `task.resume` with
        // a fix_hint.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        let finding = match second {
            PolicyDecision::RejectWithResume(f) => f,
            other => panic!("expected RejectWithResume, got {:?}", other),
        };
        let class = is_recoverable_policy_finding(&finding);
        assert_eq!(
            class,
            Some(ReasonClass::DuplicateWorkDone),
            "review.dimension.ready dup must map to recoverable bucket, got {:?}",
            class
        );
    }

    #[test]
    fn review_dimension_ready_dedup_disabled_policy_accepts_all() {
        // When event policy is disabled, the dedup check must
        // be skipped.
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");

        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "disabled policy must not dedup review.dimension.ready, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_missing_fields_skips_dedup() {
        // If payload is missing any of the dedup fields, the
        // dedup check cannot run — fall through to other
        // policy layers. The DuplicateWorkDone variant must
        // NOT appear.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"dimension":"correctness"}"#; // missing plan_name/step/task_id
        let decision = validate_event("review.dimension.ready", Some(payload), &config, &mut state);
        if let PolicyDecision::RejectWithResume(f) = &decision {
            assert!(
                !matches!(f.violation_type, ViolationType::DuplicateWorkDone { .. }),
                "missing-fields payload must not trigger DuplicateWorkDone, got {:?}",
                f.violation_type
            );
        }
    }

    // 2026-07-02 P1-A: review.dimension.failed schema gate.
    // The whitelist mirrors ce-executor-serial.yml
    // 6-dimension sequence. See event_policy.rs:1170 above.
    fn review_dimension_failed_payload(dim: Option<&str>) -> String {
        match dim {
            Some(d) => {
                format!(r#"{{"dimension":"{d}","plan_name":"p1","step":"step-01","task_id":"t1"}}"#)
            }
            None => r#"{"plan_name":"p1","step":"step-01","task_id":"t1"}"#.to_string(),
        }
    }

    #[test]
    fn review_dimension_failed_unknown_dimension_rejected() {
        // The 2026-07-01 ralph-e2e run emitted
        // `review.dimension.failed(dimension=unknown)` from a
        // dimension-reviewer payload that lost its
        // `original_dimension` field. The P1-A gate must
        // reject the unknown value with InvalidFieldValue
        // BEFORE the flow-scope stage can surface it as
        // `flow_unknown_emit`.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_failed_payload(Some("unknown"));
        let decision = validate_event(
            "review.dimension.failed",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(
                decision,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::InvalidFieldValue { ref field, .. },
                    ..
                }) if field == "dimension"
            ),
            "unknown dimension must be rejected with InvalidFieldValue, got {:?}",
            decision
        );
    }

    #[test]
    fn review_dimension_failed_missing_dimension_rejected() {
        // The P1-A gate must also catch payloads that omit
        // the `dimension` field entirely. This is the
        // `MissingRequiredField` arm (not InvalidFieldValue).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_failed_payload(None);
        let decision = validate_event(
            "review.dimension.failed",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(
                decision,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::MissingRequiredField { ref field },
                    ..
                }) if field == "dimension"
            ),
            "missing dimension must be rejected with MissingRequiredField, got {:?}",
            decision
        );
    }

    #[test]
    fn review_dimension_failed_whitelisted_dimension_accepted() {
        // Happy path: any of the 6 known dimensions is
        // accepted by the P1-A gate.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        for dim in &[
            "goal-alignment",
            "correctness",
            "testing",
            "maintainability",
            "project-standards",
            "adversarial",
        ] {
            let payload = review_dimension_failed_payload(Some(dim));
            let decision = validate_event(
                "review.dimension.failed",
                Some(&payload),
                &config,
                &mut state,
            );
            assert_eq!(
                decision,
                PolicyDecision::Accept,
                "whitelisted dimension {dim} must be accepted, got {:?}",
                decision
            );
        }
    }

    #[test]
    fn review_dimension_failed_missing_payload_falls_through() {
        // If the event has no payload (legacy / synthetic),
        // the P1-A gate cannot decode the dimension. The
        // check must fall through (no rejection from this
        // layer) so downstream schema/terminal layers can
        // surface their own precise error.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("review.dimension.failed", None, &config, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "missing-payload event must fall through, got {:?}",
            decision
        );
    }

    #[test]
    fn review_dimension_ready_replay_from_events_populates_seen_keys() {
        // `PolicyRuntimeState::from_events` must populate the
        // dedup set from any prior `review.dimension.ready`
        // events in the JSONL so cross-batch replay is
        // honored.
        use std::io::Write;

        let jsonl = r#"{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"review.dimension.done","hat":"dimension-reviewer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness"),
            "from_events must populate dedup set from prior review.dimension.ready, got {:?}",
            state.review_dimension_ready_seen_keys
        );
    }

    // -------------------------------------------------------------------------
    // 2026-07-01-001 U1: `review.start` dedup and replay tests.
    // -------------------------------------------------------------------------

    #[test]
    fn review_start_dedup_first_accepted() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_start_payload("p1", Some("step-01"), "t1");
        let decision = validate_event("review.start", Some(&payload), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
        assert!(state.review_start_seen_keys.contains("p1::t1::step-01"));
    }

    #[test]
    fn review_start_dedup_duplicate_rejected() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_start_payload("p1", Some("step-01"), "t1");
        assert_eq!(
            validate_event("review.start", Some(&payload), &config, &mut state),
            PolicyDecision::Accept
        );
        let second = validate_event("review.start", Some(&payload), &config, &mut state);
        assert!(
            matches!(second, PolicyDecision::RejectWithResume(_)),
            "duplicate review.start must be rejected, got {:?}",
            second
        );
    }

    #[test]
    fn review_start_dedup_different_task_accepted() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let p1 = review_start_payload("p1", Some("step-01"), "t1");
        let p2 = review_start_payload("p1", Some("step-01"), "t2");
        assert_eq!(
            validate_event("review.start", Some(&p1), &config, &mut state),
            PolicyDecision::Accept
        );
        assert_eq!(
            validate_event("review.start", Some(&p2), &config, &mut state),
            PolicyDecision::Accept
        );
    }

    #[test]
    fn review_start_dedup_missing_task_id_skips_dedup() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p1","step":"step-01"}"#; // missing task_id
        let decision = validate_event("review.start", Some(payload), &config, &mut state);
        if let PolicyDecision::RejectWithResume(f) = &decision {
            assert!(
                !matches!(f.violation_type, ViolationType::DuplicateWorkDone { .. }),
                "missing task_id must not trigger DuplicateWorkDone, got {:?}",
                f.violation_type
            );
        }
    }

    #[test]
    fn review_start_dedup_step_in_key() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let without_step = review_start_payload("p1", None, "t1");
        let with_step = review_start_payload("p1", Some("step-01"), "t1");
        assert_eq!(
            validate_event("review.start", Some(&without_step), &config, &mut state),
            PolicyDecision::Accept
        );
        // Same plan/task but now with a step is a different key, so accepted.
        assert_eq!(
            validate_event("review.start", Some(&with_step), &config, &mut state),
            PolicyDecision::Accept
        );
        // Re-emitting the no-step payload should still be rejected.
        assert!(
            matches!(
                validate_event("review.start", Some(&without_step), &config, &mut state),
                PolicyDecision::RejectWithResume(_)
            ),
            "re-emitting no-step review.start must be rejected"
        );
    }

    // U8 of plan 2026-07-02-005: semantic-key dedup. The 175407
    // failure: 2nd `review.start` had identical `plan_name +
    // task_id + step` but a different `triggered` value (e.g.
    // `ralph` vs `review-coordinator`); byte equality rejected
    // the 1st emit but the semantic-identity 2nd slipped
    // through. The fix: when the payload carries
    // `fix_round` AND `total_units`, the dedup key is built
    // from those two fields only — `triggered` is intentionally
    // ignored, regardless of which hat produced the event.

    #[test]
    fn u8_review_start_semantic_dedup_ignores_triggered_field() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        // 1st emit: triggered=ralph, fix_round=0, total_units=11.
        let first = r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","fix_round":0,"total_units":11,"triggered":"ralph"}"#;
        assert_eq!(
            validate_event("review.start", Some(first), &config, &mut state),
            PolicyDecision::Accept
        );
        // 2nd emit: triggered=review-coordinator, identical
        // (plan_name, task_id, fix_round, total_units). 175407
        // root cause: this slipped through before U8. After U8,
        // the dedup key is `p1::fr=0::tu=11` and matches.
        let second = r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","fix_round":0,"total_units":11,"triggered":"review-coordinator"}"#;
        assert!(
            matches!(
                validate_event("review.start", Some(second), &config, &mut state),
                PolicyDecision::RejectWithResume(_)
            ),
            "U8: 2nd review.start with identical fix_round+total_units must be rejected \
             regardless of `triggered`"
        );
    }

    #[test]
    fn u8_review_start_different_total_units_allowed() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let first = r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","fix_round":0,"total_units":11,"triggered":"ralph"}"#;
        let second = r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","fix_round":0,"total_units":7,"triggered":"ralph"}"#;
        assert_eq!(
            validate_event("review.start", Some(first), &config, &mut state),
            PolicyDecision::Accept
        );
        // Different total_units is a different semantic key, so
        // accepted (e.g. plan was re-planned mid-review).
        assert_eq!(
            validate_event("review.start", Some(second), &config, &mut state),
            PolicyDecision::Accept
        );
    }

    #[test]
    fn u8_review_start_legacy_fallback_when_fix_round_missing() {
        // Pre-U8 emits that don't carry `fix_round` /
        // `total_units` must still use the legacy
        // `(plan_name, task_id [, step])` key — backward
        // compatibility for older recovery journals.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let first = r#"{"plan_name":"p1","task_id":"t1","task_key":"k1"}"#;
        let second =
            r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","triggered":"review-coordinator"}"#;
        assert_eq!(
            validate_event("review.start", Some(first), &config, &mut state),
            PolicyDecision::Accept
        );
        // Without `fix_round` / `total_units`, the legacy key
        // `p1::t1` matches.
        assert!(
            matches!(
                validate_event("review.start", Some(second), &config, &mut state),
                PolicyDecision::RejectWithResume(_)
            ),
            "U8: legacy fallback (no fix_round / total_units) must still dedup"
        );
    }

    #[test]
    fn review_start_replay_from_events_populates_seen_keys() {
        use std::io::Write;
        let jsonl = r#"{"topic":"review.start","hat":"coordinator","payload":"{\"plan_name\":\"p1\",\"task_id\":\"t1\",\"task_key\":\"k1\"}"}"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            state.review_start_seen_keys.contains("p1::t1"),
            "from_events must populate review.start dedup set, got {:?}",
            state.review_start_seen_keys
        );
    }

    #[test]
    fn review_start_replay_from_events_with_step_populates_seen_keys() {
        use std::io::Write;
        let jsonl = r#"{"topic":"review.start","hat":"coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"task_key\":\"k1\"}"}"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            state.review_start_seen_keys.contains("p1::t1::step-01"),
            "from_events must include step in review.start key, got {:?}",
            state.review_start_seen_keys
        );
    }

    #[test]
    fn review_start_prune_on_fix_applied_from_events() {
        use std::io::Write;
        let jsonl = r#"{"topic":"review.start","hat":"coordinator","payload":"{\"plan_name\":\"p1\",\"task_id\":\"t1\",\"task_key\":\"k1\"}"}
{"topic":"fix.applied","hat":"fixer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"task_key\":\"k1\"}"}"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            !state.review_start_seen_keys.contains("p1::t1"),
            "fix.applied replay must prune review.start dedup set, got {:?}",
            state.review_start_seen_keys
        );
    }

    #[test]
    fn review_start_prune_bucket_manual() {
        let mut state = PolicyRuntimeState::default();
        state.review_start_seen_keys.insert("p1::t1".into());
        state
            .review_start_seen_keys
            .insert("p1::t1::step-01".into());
        state.review_start_seen_keys.insert("p2::t1".into());

        state.prune_review_start_bucket("p1", "t1");

        assert!(!state.review_start_seen_keys.contains("p1::t1"));
        assert!(!state.review_start_seen_keys.contains("p1::t1::step-01"));
        assert!(state.review_start_seen_keys.contains("p2::t1"));
    }

    // -------------------------------------------------------------------------
    // U1 (2026-06-18-004 plan, R1, KTD1):
    // `fix.applied` prunes the `(plan, step, task)` bucket of
    // `review_dimension_ready_seen_keys` so a fix → re-review
    // walk can legally emit `review.dimension.ready` for the
    // same `(plan, step, task, dimension)` tuple. Without this
    // prune the perky-maple run falls into a HARD GATE spiral
    // (P1-3 / P2-5 in the diagnosis report).
    //
    // Both paths must execute the same prune:
    //   1. Live accept site in `event_loop/mod.rs`
    //      (paired with `LoopState::prune_work_done_bucket`).
    //   2. `PolicyRuntimeState::from_events` replay path so a
    //      loop rehydrate does not re-introduce the dedup
    //      block.
    // Both are covered by the unit tests below.
    // -------------------------------------------------------------------------

    #[test]
    fn u1_prune_review_dimension_ready_bucket_clears_matching_prefix() {
        let mut state = PolicyRuntimeState::default();
        state
            .review_dimension_ready_seen_keys
            .insert("p1::step-01::t1::correctness".into());
        state
            .review_dimension_ready_seen_keys
            .insert("p1::step-01::t1::testing".into());
        state
            .review_dimension_ready_seen_keys
            .insert("p1::step-02::t1::correctness".into());

        state.prune_review_dimension_ready_bucket("p1", "step-01", "t1");

        assert!(
            !state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness"),
            "matching-prefix entry should be pruned, got {:?}",
            state.review_dimension_ready_seen_keys
        );
        assert!(
            !state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::testing"),
            "matching-prefix entry should be pruned, got {:?}",
            state.review_dimension_ready_seen_keys
        );
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-02::t1::correctness"),
            "non-matching-prefix entry should remain, got {:?}",
            state.review_dimension_ready_seen_keys
        );
    }

    #[test]
    fn u1_prune_work_done_bucket_mirror_clears_matching_prefix() {
        let mut state = PolicyRuntimeState::default();
        state.work_done_seen_keys.insert("p1::step-01::t1".into());
        state.work_done_seen_keys.insert("p1::step-02::t1".into());
        state.work_done_seen_keys.insert("p2::step-01::t1".into());

        state.prune_work_done_bucket("p1", "step-01");

        assert!(!state.work_done_seen_keys.contains("p1::step-01::t1"));
        assert!(state.work_done_seen_keys.contains("p1::step-02::t1"));
        assert!(state.work_done_seen_keys.contains("p2::step-01::t1"));
    }

    #[test]
    fn u1_fix_applied_replay_prunes_dimension_ready_keys() {
        use std::io::Write;

        let jsonl = r#"{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"review.dimension.done","hat":"dimension-reviewer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"fix.applied","hat":"fixer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":1,\"applied_count\":8,\"failed_count\":0,\"commit_count\":1,\"changed_lines\":96}"}
"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            !state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness"),
            "from_events replay of fix.applied must prune the bucket, got {:?}",
            state.review_dimension_ready_seen_keys
        );
    }

    #[test]
    fn u1_fix_applied_replay_populates_work_done_seen_keys_for_prior_work_done() {
        use std::io::Write;

        let jsonl = r#"{"topic":"work.done","hat":"executor","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"commit_count\":1}"}
"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            state.work_done_seen_keys.contains("p1::step-01::t1"),
            "from_events must mirror prior work.done into work_done_seen_keys, got {:?}",
            state.work_done_seen_keys
        );
    }

    #[test]
    fn u_fixes_2026_07_04_task_id_task_key_mismatch_surfaces_invalid_field_value() {
        // Regression test: agent emits work.done with same
        // task_id as the prior accepted event but a different
        // task_key. Must be rejected as InvalidFieldValue
        // (`task_id_task_key_mismatch`), NOT DuplicateWorkDone,
        // so the resume hint is actionable.
        let mut state = PolicyRuntimeState::default();
        let config = test_config();
        let payload1 = serde_json::json!({
            "plan_name": "p1",
            "step": "step-01",
            "task_id": "t1",
            "task_key": "ce-executor:p1:step-01:u1-skeleton",
            "commit_count": 1,
            "changed_lines": 10,
        })
        .to_string();
        let payload2 = serde_json::json!({
            "plan_name": "p1",
            "step": "step-01",
            "task_id": "t1",
            "task_key": "ce-executor:p1:step-01:u0-impl",
            "commit_count": 1,
            "changed_lines": 10,
        })
        .to_string();
        let first = super::validate_event("work.done", Some(&payload1), &config, &mut state);
        assert!(matches!(first, super::PolicyDecision::Accept));
        let second = super::validate_event("work.done", Some(&payload2), &config, &mut state);
        match second {
            super::PolicyDecision::RejectWithResume(finding) => {
                assert!(
                    matches!(
                        finding.violation_type,
                        super::ViolationType::InvalidFieldValue { .. }
                    ),
                    "expected InvalidFieldValue(task_id_task_key_mismatch), got {:?}",
                    finding.violation_type
                );
                assert!(
                    finding.message.contains("task_id_task_key_mismatch"),
                    "message should name the failure mode, got: {}",
                    finding.message
                );
            }
            other => panic!("expected RejectWithResume, got {other:?}"),
        }
    }

    #[test]
    fn u1_fix_applied_replay_then_rereview_ready_accepted() {
        use std::io::Write;

        let jsonl = r#"{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"fix.applied","hat":"fixer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":1,\"applied_count\":8,\"failed_count\":0,\"commit_count\":1,\"changed_lines\":96}"}
"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let mut state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let decision = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &test_config(),
            &mut state,
        );
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "Re-review ready after fix.applied must be accepted, got {:?}",
            decision
        );
    }

    #[test]
    fn u1_fix_applied_prune_helper_keeps_other_task_keys() {
        let mut state = PolicyRuntimeState::default();
        state
            .review_dimension_ready_seen_keys
            .insert("p1::step-01::t1::correctness".into());
        state
            .review_dimension_ready_seen_keys
            .insert("p1::step-01::t2::correctness".into());

        state.prune_review_dimension_ready_bucket("p1", "step-01", "t1");

        assert!(
            !state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness")
        );
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t2::correctness")
        );
    }

    #[test]
    fn u1_fix_applied_replay_does_not_prune_other_task_dimension_ready() {
        // Defensive: `fix.applied` payload's task_id bounds the
        // prune scope. A sibling task in the same (plan, step)
        // must keep its dedup key.
        use std::io::Write;

        let jsonl = r#"{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t2\",\"dimension\":\"correctness\"}"}
{"topic":"fix.applied","hat":"fixer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":1,\"applied_count\":8,\"failed_count\":0,\"commit_count\":1,\"changed_lines\":96}"}
"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            !state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness"),
            "fix.applied on t1 must prune t1 bucket, got {:?}",
            state.review_dimension_ready_seen_keys
        );
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t2::correctness"),
            "fix.applied on t1 must NOT prune t2 bucket, got {:?}",
            state.review_dimension_ready_seen_keys
        );
    }

    #[test]
    fn u1_prune_review_dimension_ready_does_not_affect_other_steps() {
        // Defensive: prune is scoped to (plan, step, task). A
        // different step in the same plan must keep its key.
        let mut state = PolicyRuntimeState::default();
        state
            .review_dimension_ready_seen_keys
            .insert("p1::step-01::t1::correctness".into());
        state
            .review_dimension_ready_seen_keys
            .insert("p1::step-02::t1::correctness".into());

        state.prune_review_dimension_ready_bucket("p1", "step-01", "t1");

        assert!(
            !state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness")
        );
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-02::t1::correctness")
        );
    }

    #[test]
    fn u1_dedup_helper_prunes_allow_fix_round_rereview() {
        // End-to-end happy path: first ready accept, second
        // emit blocked, fix.applied prune, third emit (re-review)
        // accepted.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");

        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(matches!(
            second,
            PolicyDecision::RejectWithResume(PolicyFinding {
                violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                ..
            }) if key == "p1::step-01::t1::correctness"
        ));

        // fix.applied accept path runs prune.
        state.prune_review_dimension_ready_bucket("p1", "step-01", "t1");

        let third = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(
            third,
            PolicyDecision::Accept,
            "after fix.applied prune the re-review ready must be accepted, got {:?}",
            third
        );
    }

    #[test]
    fn u4_no_prune_blocks_re_review_ready() {
        // 2026-06-18-006 plan U4 (R4): negative counterpart of
        // `u1_dedup_helper_prunes_allow_fix_round_rereview`.
        // Without the U1 prune (which is triggered when `fix.applied`
        // is accepted via `prune_review_dimension_ready_bucket`),
        // re-emitting a `review.dimension.ready` for the same
        // `(plan, step, task, dimension)` MUST be rejected as
        // `DuplicateWorkDone` — the dedup mirror still holds the
        // round-0 key. This pins that U1's prune is the load-bearing
        // step that lets the re-review round walk. The
        // `review_dimension_ready_dedup_*` cluster above already
        // covers the first/second emit round-trip on a fresh
        // state; this test isolates the specific post-accept
        // failure mode (round 1 emit blocked because round 0
        // still lingers).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");

        // Round 0: accept once so the dedup mirror learns the key.
        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(
            first,
            PolicyDecision::Accept,
            "round 0 review.dimension.ready must be accepted, got {:?}",
            first
        );
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness"),
            "dedup mirror must hold round 0 key after accept, got {:?}",
            state.review_dimension_ready_seen_keys
        );

        // Intentionally DO NOT call
        // `state.prune_review_dimension_ready_bucket("p1", "step-01", "t1")`.
        // This simulates the bug scenario where the `fix.applied`
        // acceptance path (which normally prunes the bucket) is
        // missing — the in-batch mirror still holds the round-0 key.

        // Round 1 (re-review): re-emit the same ready. Without
        // the prune, this must be rejected as `DuplicateWorkDone`.
        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "p1::step-01::t1::correctness"
            ),
            "without U1 prune, re-review ready must be rejected as DuplicateWorkDone, got {:?}",
            second
        );

        // The dedup mirror must STILL hold the round-0 key after
        // the rejection — that's exactly the load the prune is
        // meant to lift. Pinning this prevents a future "helpful"
        // edit from clearing the mirror on rejection and silently
        // re-enabling duplicate work-done emits.
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness"),
            "dedup mirror must keep round-0 key when prune is skipped, got {:?}",
            state.review_dimension_ready_seen_keys
        );
    }

    // -------------------------------------------------------------------------
    // U5 (2026-06-18-004 plan, R4, KTD3):
    // `review.dimensions.complete` dedup keyed on
    // `(plan_name, step, task_id, fix_round)`. A 2nd emit with
    // the same key is rejected as `DuplicateWorkDone`. After
    // `fix.applied` the bucket is pruned so the next round's
    // `review.dimensions.complete` (with `fix_round=N+1`) can
    // land without colliding with the prior round's entry.
    //
    // Mirrors the U5 `review.dimension.ready` test cluster
    // above. Together they pin the re-review dedup contract
    // end-to-end.
    // -------------------------------------------------------------------------

    fn review_dimensions_complete_payload(
        plan: &str,
        step: &str,
        task: &str,
        fix_round: u32,
    ) -> String {
        format!(
            r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","fix_round":{fix_round},"dimensions":[]}}"#
        )
    }

    #[test]
    fn u5_review_dimensions_complete_dedup_first_accepted() {
        // Happy path: first emit with `fix_round=0` is accepted.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimensions_complete_payload("p1", "step-01", "t1", 0);
        let decision = validate_event(
            "review.dimensions.complete",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "first review.dimensions.complete must be accepted, got {:?}",
            decision
        );
        assert!(
            state
                .review_dimensions_complete_seen_keys
                .contains("p1::step-01::t1::0"),
            "seen_keys must include the round-0 entry, got {:?}",
            state.review_dimensions_complete_seen_keys
        );
    }

    #[test]
    fn u5_review_dimensions_complete_dedup_rejects_second_emit_same_round() {
        // Error path: 2nd emit with the same `fix_round` is
        // acknowledged + forwarded (U2 carve-out: silent-success
        // lane) instead of being rejected as `DuplicateWorkDone`.
        // The 4× duplicate `review.dimensions.complete` events
        // from the perky-maple P2-1 run are now silently accepted
        // by policy and forwarded to the bus; downstream code
        // observes the dedup hint via the carried `PolicyFinding`.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimensions_complete_payload("p1", "step-01", "t1", 0);

        let first = validate_event(
            "review.dimensions.complete",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event(
            "review.dimensions.complete",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(
                second,
                PolicyDecision::AcknowledgeAndForward(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "p1::step-01::t1::0"
            ),
            "2nd review.dimensions.complete same round must be AcknowledgeAndForward per U2, got {:?}",
            second
        );
    }

    #[test]
    fn u5_review_dimensions_complete_dedup_different_fix_round_accepted() {
        // Edge case: 1st round (fix_round=0) accepted, 2nd
        // round (fix_round=1) accepted (after fix.applied
        // prune) — the fix_round segment keeps them distinct.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let first = review_dimensions_complete_payload("p1", "step-01", "t1", 0);
        let first_decision = validate_event(
            "review.dimensions.complete",
            Some(&first),
            &config,
            &mut state,
        );
        assert_eq!(first_decision, PolicyDecision::Accept);

        // Simulate fix.applied accept site (U1 path).
        state.prune_review_dimensions_complete_bucket("p1", "step-01", "t1");

        let second = review_dimensions_complete_payload("p1", "step-01", "t1", 1);
        let second_decision = validate_event(
            "review.dimensions.complete",
            Some(&second),
            &config,
            &mut state,
        );
        assert_eq!(
            second_decision,
            PolicyDecision::Accept,
            "fix_round=1 must be accepted after fix.applied prune, got {:?}",
            second_decision
        );
    }

    #[test]
    fn u5_review_dimensions_complete_dedup_disabled_policy_accepts_all() {
        // When policy is disabled, dedup must NOT fire.
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimensions_complete_payload("p1", "step-01", "t1", 0);

        let first = validate_event(
            "review.dimensions.complete",
            Some(&payload),
            &config,
            &mut state,
        );
        let second = validate_event(
            "review.dimensions.complete",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "disabled policy must NOT dedup review.dimensions.complete, got {:?}",
            second
        );
    }

    #[test]
    fn u6_review_dimensions_complete_missing_fix_round_skips_dedup() {
        // U6 (2026-06-18-006 plan, R6, KTD4): missing `fix_round`
        // no longer silently defaults to `0`. The dedup layer
        // must skip recording the key so the schema validator
        // (downstream of `validate_event`) reports the precise
        // `missing_required_field` error to the agent, rather
        // than the dedup layer hiding the failure behind a
        // misleading `DuplicateWorkDone` rejection.
        //
        // This test replaces the prior U5 assertion that
        // expected missing `fix_round` to default to `0` and
        // dedup against the round-0 key. U6 reverses that
        // behavior now that `fix_round` is a required schema
        // field (2026-06-18-004 plan U0).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        // Intentionally omit `fix_round` — schema invalid.
        let payload = r#"{"plan_name":"p1","step":"step-01","task_id":"t1","dimensions":[]}"#;

        let first = validate_event(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
        );
        // First emit also doesn't get a dedup key written —
        // dedup layer is silent on schema-invalid emits.
        // (Schema validation is downstream; we assert the dedup
        // layer's contract here: it does NOT insert a key.)
        assert_eq!(
            first,
            PolicyDecision::Accept,
            "missing fix_round must NOT be dedup-rejected by the policy layer (schema layer reports the real error), got {:?}",
            first
        );
        assert!(
            state.review_dimensions_complete_seen_keys.is_empty(),
            "missing fix_round must NOT populate the dedup mirror, got {:?}",
            state.review_dimensions_complete_seen_keys
        );

        let second = validate_event(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
        );
        // 2nd emit with the same invalid payload: still no
        // dedup, no `DuplicateWorkDone`. The dedup layer's
        // contract is to stay out of the way when the event is
        // schema-invalid.
        assert!(
            !matches!(second, PolicyDecision::RejectWithResume(_)),
            "missing fix_round must NOT trigger DuplicateWorkDone on a 2nd emit — schema layer owns the error, got {:?}",
            second
        );
        assert!(
            state.review_dimensions_complete_seen_keys.is_empty(),
            "seen_keys must still be empty after 2nd schema-invalid emit, got {:?}",
            state.review_dimensions_complete_seen_keys
        );
    }

    #[test]
    fn u6_review_dimensions_complete_same_fix_round_still_dedups() {
        // U6 regression guard: the KTD4 change must NOT break
        // the round-0 dedup contract. Two emits both carrying
        // `fix_round=0` for the same `(plan, step, task)` are
        // still dedup-handled — U2 changes the *decision* from
        // `RejectWithResume` to `AcknowledgeAndForward` so the
        // silent-success run does not produce `task.resume`
        // storms, but the dedup invariant (mirror is populated,
        // second emit carries a `DuplicateWorkDone` finding) is
        // intact. Only schema-invalid emits are exempted.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimensions_complete_payload("p1", "step-01", "t1", 0);

        let first = validate_event(
            "review.dimensions.complete",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);
        assert!(
            state
                .review_dimensions_complete_seen_keys
                .contains("p1::step-01::t1::0"),
            "round-0 emit must populate the dedup mirror, got {:?}",
            state.review_dimensions_complete_seen_keys
        );

        let second = validate_event(
            "review.dimensions.complete",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(
                second,
                PolicyDecision::AcknowledgeAndForward(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "p1::step-01::t1::0"
            ),
            "2nd round-0 emit must STILL be dedup-handled per U2 (AcknowledgeAndForward), got {:?}",
            second
        );
    }

    #[test]
    fn u6_review_dimensions_complete_string_fix_round_skips_dedup() {
        // U6 (KTD4): non-numeric `fix_round` (e.g. string `"1"`)
        // is also treated as schema-invalid. The dedup layer
        // must not write a key for it, leaving the schema
        // validator free to report `type_mismatch`. This is the
        // same root-cause class as missing `fix_round` — a
        // schema-level error that must not be hidden behind
        // `DuplicateWorkDone`.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload =
            r#"{"plan_name":"p1","step":"step-01","task_id":"t1","fix_round":"1","dimensions":[]}"#;

        let first = validate_event(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
        );
        assert_eq!(
            first,
            PolicyDecision::Accept,
            "string fix_round must NOT be dedup-rejected (schema layer reports type_mismatch), got {:?}",
            first
        );
        assert!(
            state.review_dimensions_complete_seen_keys.is_empty(),
            "string fix_round must NOT populate the dedup mirror, got {:?}",
            state.review_dimensions_complete_seen_keys
        );

        let second = validate_event(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
        );
        assert!(
            !matches!(second, PolicyDecision::RejectWithResume(_)),
            "string fix_round must NOT trigger DuplicateWorkDone on 2nd emit, got {:?}",
            second
        );
        assert!(
            state.review_dimensions_complete_seen_keys.is_empty(),
            "seen_keys must still be empty after 2nd string fix_round emit, got {:?}",
            state.review_dimensions_complete_seen_keys
        );
    }

    #[test]
    fn u5_prune_helper_keeps_other_task_keys() {
        // Defensive: prune is scoped to (plan, step, task). A
        // sibling task in the same (plan, step) must keep its
        // dedup key.
        let mut state = PolicyRuntimeState::default();
        state
            .review_dimensions_complete_seen_keys
            .insert("p1::step-01::t1::0".into());
        state
            .review_dimensions_complete_seen_keys
            .insert("p1::step-01::t2::0".into());

        state.prune_review_dimensions_complete_bucket("p1", "step-01", "t1");

        assert!(
            !state
                .review_dimensions_complete_seen_keys
                .contains("p1::step-01::t1::0")
        );
        assert!(
            state
                .review_dimensions_complete_seen_keys
                .contains("p1::step-01::t2::0")
        );
    }

    #[test]
    fn u5_review_dimensions_complete_replay_populates_seen_keys() {
        // KTD3 / KTD1 symmetry: `from_events` mirrors the
        // dedup set from prior `review.dimensions.complete`
        // events so loop rehydrate does not accept a
        // duplicate.
        use std::io::Write;

        let jsonl = r#"{"topic":"review.dimensions.complete","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":0,\"dimensions\":[]}"}
{"topic":"review.dimensions.complete","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":1,\"dimensions\":[]}"}
"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            state
                .review_dimensions_complete_seen_keys
                .contains("p1::step-01::t1::0"),
            "from_events must mirror round-0 complete into the dedup set, got {:?}",
            state.review_dimensions_complete_seen_keys
        );
        assert!(
            state
                .review_dimensions_complete_seen_keys
                .contains("p1::step-01::t1::1"),
            "from_events must mirror round-1 complete into the dedup set, got {:?}",
            state.review_dimensions_complete_seen_keys
        );
    }

    #[test]
    fn u5_fix_applied_replay_prunes_dimensions_complete_keys() {
        // KTD1 symmetry for the complete dedup: replay of
        // `fix.applied` MUST also clear the
        // `review.dimensions.complete` bucket so the next
        // round's complete (fix_round=N+1) does not collide
        // with the prior round's entry on rehydrate.
        use std::io::Write;

        let jsonl = r#"{"topic":"review.dimensions.complete","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":0,\"dimensions\":[]}"}
{"topic":"fix.applied","hat":"fixer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":1,\"applied_count\":1,\"failed_count\":0,\"commit_count\":1,\"changed_lines\":5}"}
"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            !state
                .review_dimensions_complete_seen_keys
                .contains("p1::step-01::t1::0"),
            "from_events replay of fix.applied MUST prune the complete bucket, got {:?}",
            state.review_dimensions_complete_seen_keys
        );
    }

    // ── WAC-U7 (2026-06-12-002): payload hard gate ──

    /// T-U7-01 / R10: null `review.passed` payload is hard-rejected
    /// even when `EventPolicyMode::Observe` is configured.
    #[test]
    fn wac_r10_null_payload_on_whitelist_topic_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        // Switch the policy into Observe mode to confirm the R10
        // gate is mode-agnostic.
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("review.passed", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "R10 must RejectWithResume even in Observe mode, got {:?}",
            decision
        );
    }

    /// R10 also covers the other whitelist topics:
    /// `work.done`, `queue.advance`, `review.wave.ready`, etc.
    /// Step-handoff (2026-06-17-002) U5 extends the whitelist with
    /// the handoff/terminal topics `work.ready`, `plan.complete`,
    /// `plan.blocked` so the hard gate uniformly covers every
    /// handoff/terminal topic in the ce-executor step chain.
    #[test]
    fn wac_r10_null_payload_rejects_every_whitelist_topic() {
        let config = test_config_with_enforce_and_resume();
        for topic in [
            "review.passed",
            "review.failed",
            "review.complete",
            "work.done",
            "work.ready",
            "queue.advance",
            "review.wave.ready",
            "plan.complete",
            "plan.blocked",
        ] {
            let mut s = PolicyRuntimeState::default();
            let decision = validate_event(topic, None, &config, &mut s);
            assert!(
                matches!(decision, PolicyDecision::RejectWithResume(_)),
                "R10 must reject null payload on `{topic}`, got {:?}",
                decision
            );
        }
    }

    /// Step-handoff (2026-06-17-002) U5: `is_null_payload_rejected_topic` is the
    /// single source of truth for the whitelist. Pin the exact
    /// membership (original 6 + 3 U5 additions appended in place) so
    /// future edits cannot silently drop a topic from the hard gate.
    #[test]
    fn step_handoff_u5_whitelist_membership_pinned() {
        let expected = [
            "review.passed",
            "review.failed",
            "review.complete",
            "work.done",
            "queue.advance",
            "review.wave.ready",
            "work.ready",
            "plan.complete",
            "plan.blocked",
        ];
        assert_eq!(NULL_PAYLOAD_REJECT_TOPICS, expected);
        for topic in expected {
            assert!(
                is_null_payload_rejected_topic(topic),
                "is_null_payload_rejected_topic must accept `{topic}`"
            );
        }
        // A non-whitelist topic is unaffected.
        assert!(!is_null_payload_rejected_topic("human.guidance"));
    }

    /// Step-handoff U5: a null `work.ready` payload is
    /// hard-rejected even in Observe mode. This is the per-topic
    /// pin for `work.ready` after the list extension.
    #[test]
    fn step_handoff_u5_work_ready_null_payload_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("work.ready", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "U5: null work.ready must RejectWithResume even in Observe, got {:?}",
            decision
        );
    }

    /// Step-handoff U5: a null `plan.complete` payload is
    /// hard-rejected even in Observe mode.
    #[test]
    fn step_handoff_u5_plan_complete_null_payload_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("plan.complete", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "U5: null plan.complete must RejectWithResume even in Observe, got {:?}",
            decision
        );
    }

    /// Step-handoff U5: a null `plan.blocked` payload is
    /// hard-rejected even in Observe mode.
    #[test]
    fn step_handoff_u5_plan_blocked_null_payload_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("plan.blocked", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "U5: null plan.blocked must RejectWithResume even in Observe, got {:?}",
            decision
        );
    }

    /// 2026-06-24 P0-D regression guard: a `review.complete` whose
    /// `fix_plan_file` is a JSON `null` literal (instead of the
    /// schema-required string `"null"`) must be rejected with a
    /// `PayloadTypeMismatch` finding. The check must run regardless
    /// of `EventPolicyMode` (defense-in-depth mirrors the U5
    /// null-payload hard-reject list).
    ///
    /// Background: the ralph-e2e python-sort-algorithms run shipped
    /// `fix_plan_file: null` (JSON literal) for the fix-01 review
    /// round, the runtime accepted it, and the downstream
    /// coordinator's `fix_plan_file == "null"` string equality check
    /// failed — leaving `plan.complete` un-emitted.
    #[test]
    fn p0d_review_complete_fix_plan_file_null_literal_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        config.schemas.insert(
            "review.complete".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".to_string(),
                    "fix_round".to_string(),
                    "fix_plan_file".to_string(),
                    "verdict".to_string(),
                    "residual_findings_count".to_string(),
                    "findings_summary".to_string(),
                    "task_id".to_string(),
                    "task_key".to_string(),
                    "step".to_string(),
                    "findings_count".to_string(),
                ],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
                ..Default::default()
            },
        );
        let mut state = PolicyRuntimeState::default();
        // Same payload the ralph-e2e run emitted (note: `fix_plan_file: null`
        // is JSON null, not the string `"null"`).
        let payload = r#"{"plan_name":"python-sort-algorithms","fix_round":1,"fix_plan_file":null,"verdict":"pass","residual_findings_count":0,"findings_summary":"no findings","task_id":"task-1782311559-5071","task_key":"ce-executor:python-sort-algorithms:fix-01:u1-sorted-comparison-impl","step":"fix-01","findings_count":0}"#;
        let decision = validate_event("review.complete", Some(payload), &config, &mut state);
        match decision {
            PolicyDecision::RejectWithResume(finding) => {
                assert!(
                    matches!(
                        finding.violation_type,
                        ViolationType::PayloadTypeMismatch { ref expected, ref actual }
                        if expected == "string" && actual == "null"
                    ),
                    "P0-D: expected PayloadTypeMismatch(string, null), got {:?}",
                    finding
                );
            }
            other => panic!("P0-D: expected RejectWithResume, got {:?}", other),
        }
    }

    /// 2026-06-24 P0-D positive case: `fix_plan_file` as the
    /// schema-required string `"null"` (no fix plan) must be
    /// accepted when all required fields are present.
    #[test]
    fn p0d_review_complete_fix_plan_file_string_null_is_accepted() {
        let mut config = test_config_with_enforce_and_resume();
        config.schemas.insert(
            "review.complete".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".to_string(),
                    "fix_round".to_string(),
                    "fix_plan_file".to_string(),
                    "verdict".to_string(),
                    "residual_findings_count".to_string(),
                    "findings_summary".to_string(),
                    "task_id".to_string(),
                    "task_key".to_string(),
                    "step".to_string(),
                    "findings_count".to_string(),
                ],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
                ..Default::default()
            },
        );
        let mut state = PolicyRuntimeState::default();
        // `fix_plan_file` is the literal string `"null"` (note the
        // escaped quotes inside the JSON string).
        let payload = r#"{"plan_name":"python-sort-algorithms","fix_round":0,"fix_plan_file":"null","verdict":"pass","residual_findings_count":0,"findings_summary":"no findings","task_id":"task-1782310833-0494","task_key":"ce-executor:python-sort-algorithms:step-02:u0-quick-sort-impl","step":"step-02","findings_count":0}"#;
        let decision = validate_event("review.complete", Some(payload), &config, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "P0-D positive: string \"null\" must be accepted, got {:?}",
            decision
        );
    }

    // 2026-07-03-005 plan (P0 fix C7): element_constraints rejects
    // `review.dimensions.complete` with `status: done` and a null
    // `findings_file`. Without this, the agent fabricates 4 of 6
    // dimensions as `status: done, findings_file: null` and the shipper
    // walks `pass_with_residuals` based on the inflated summary.

    fn insert_review_dimensions_schema(
        config: &mut EventPolicyConfig,
        field: &str,
        required: bool,
        allowed: Vec<serde_json::Value>,
        required_when: HashMap<String, serde_json::Value>,
        forbid_null: bool,
    ) {
        let constraint = ElementConstraint {
            field: field.to_string(),
            required,
            allowed_values: allowed,
            required_when,
            forbid_null_when_required: forbid_null,
        };
        let mut ec = HashMap::new();
        ec.insert("dimensions".to_string(), constraint);
        config.schemas.insert(
            "review.dimensions.complete".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec!["dimensions".to_string()],
                element_constraints: ec,
                ..Default::default()
            },
        );
    }

    #[test]
    fn c7_review_dimensions_complete_silent_drop_done_with_null_findings() {
        let mut config = test_config_with_enforce_and_resume();
        let mut rw = HashMap::new();
        rw.insert("status".to_string(), serde_json::json!("done"));
        insert_review_dimensions_schema(&mut config, "findings_file", true, Vec::new(), rw, true);
        let mut state = PolicyRuntimeState::default();
        // 6 dimensions, last 4 are fake `status: done, findings_file: null`.
        let payload = r#"{
            "dimensions": [
                {"dimension":"goal-alignment","status":"done","findings_file":"/tmp/ga.md"},
                {"dimension":"correctness","status":"done","findings_file":"/tmp/co.md"},
                {"dimension":"testing","status":"done","findings_file":null},
                {"dimension":"maintainability","status":"done","findings_file":null},
                {"dimension":"project-standards","status":"done","findings_file":null},
                {"dimension":"adversarial","status":"done","findings_file":null}
            ]
        }"#;
        let decision = validate_event(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "C7: status=done with null findings_file MUST be rejected, got {:?}",
            decision
        );
    }

    #[test]
    fn c7_review_dimensions_complete_accepts_skipped_with_null_findings() {
        let mut config = test_config_with_enforce_and_resume();
        let mut rw = HashMap::new();
        rw.insert("status".to_string(), serde_json::json!("done"));
        insert_review_dimensions_schema(&mut config, "findings_file", true, Vec::new(), rw, true);
        let mut state = PolicyRuntimeState::default();
        // `status: skipped` with null findings_file must be accepted.
        let payload = r#"{
            "dimensions": [
                {"dimension":"goal-alignment","status":"done","findings_file":"/tmp/ga.md"},
                {"dimension":"correctness","status":"skipped","findings_file":null}
            ]
        }"#;
        let decision = validate_event(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
        );
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "C7 positive: status=skipped with null findings_file is allowed, got {:?}",
            decision
        );
    }

    #[test]
    fn c7_review_dimensions_complete_allowed_values_on_status() {
        let mut config = test_config_with_enforce_and_resume();
        insert_review_dimensions_schema(
            &mut config,
            "status",
            true,
            vec![
                serde_json::json!("done"),
                serde_json::json!("skipped"),
                serde_json::json!("failed"),
            ],
            HashMap::new(),
            false,
        );
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{
            "dimensions": [
                {"dimension":"goal-alignment","status":"bogus"}
            ]
        }"#;
        let decision = validate_event(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "C7 allowed_values: status='bogus' MUST be rejected, got {:?}",
            decision
        );
    }

    #[test]
    fn c7_review_dimensions_complete_missing_required_field() {
        let mut config = test_config_with_enforce_and_resume();
        insert_review_dimensions_schema(
            &mut config,
            "findings_file",
            true,
            Vec::new(),
            HashMap::new(),
            false,
        );
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{
            "dimensions": [
                {"dimension":"goal-alignment"}
            ]
        }"#;
        let decision = validate_event(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "C7 required: missing findings_file MUST be rejected, got {:?}",
            decision
        );
    }

    /// T-U7-02 / R11: a string payload that is a parseable JSON
    /// object is normalized to the object form and accepted.
    /// Required-field validation runs against the normalized
    /// object, not the original string.
    #[test]
    fn wac_r11_string_payload_normalizes_to_object() {
        let mut config = test_config_with_enforce_and_resume();
        config.schemas.insert(
            "review.wave.ready".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec!["dimension".to_string(), "plan_name".to_string()],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
                ..Default::default()
            },
        );
        let mut state = PolicyRuntimeState::default();
        // The payload is a JSON-string-of-an-object.
        let payload = r#"{"dimension":"code-quality","plan_name":"p1"}"#;
        let decision = validate_event("review.wave.ready", Some(payload), &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::Accept),
            "string-as-object must normalize and accept, got {:?}",
            decision
        );
    }

    /// T-U7-03 / R11: a string payload that is NOT a valid JSON
    /// object is rejected (cannot be normalized).
    #[test]
    fn wac_r11_string_payload_not_json_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        config.schemas.insert(
            "review.wave.ready".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec!["dimension".to_string()],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
                ..Default::default()
            },
        );
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("review.wave.ready", Some("not-a-json"), &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "non-JSON string must be rejected, got {:?}",
            decision
        );
    }

    /// T-U7-07: R10 hard-rejects null payloads even when the
    /// rest of the policy is in `Observe` mode. The other
    /// findings (terminal monotonicity, etc.) still fall through
    /// to `Warn` per the existing behaviour, but R10 specifically
    /// escalates to `RejectWithResume`.
    #[test]
    fn wac_r10_overrides_observe_mode_for_null_whitelist_payload() {
        let mut config = test_config_with_enforce_and_resume();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("work.done", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "R10 must not downgraded by Observe mode, got {:?}",
            decision
        );
    }

    /// Helper: build a minimal `EventPolicyConfig` with Enforce
    /// mode + RejectWithResume. Reused by the WAC tests above.
    fn test_config_with_enforce_and_resume() -> EventPolicyConfig {
        let mut config = EventPolicyConfig::default();
        config.enabled = true;
        config.mode = EventPolicyMode::Enforce;
        config.on_violation = ViolationAction::RejectWithResume;
        config
    }

    // U1 (2026-06-17-003 plan): the new `SemanticGateViolation`
    // variant must be in the recoverable set with its own bucket
    // — and its reason_code must NOT collide with the schema-level
    // `invalid_field_value` so diagnostics stay unambiguous.
    #[test]
    fn u1_semantic_gate_violation_is_recoverable_with_own_bucket() {
        let finding = PolicyFinding {
            topic: "review.passed".to_string(),
            violation_type: ViolationType::SemanticGateViolation {
                gate: "review_passed_while_wave_open".to_string(),
                context: "wave='w-1' received=0/3 expected".to_string(),
                referenced_fields: Vec::new(),
            },
            message: "review-coordinator must not emit review.passed while wave is incomplete"
                .to_string(),
        };
        let class = is_recoverable_policy_finding(&finding)
            .expect("SemanticGateViolation must be in the recoverable set");
        assert_eq!(class, ReasonClass::SemanticGateViolation);
        assert_eq!(class.as_str(), "semantic_gate_violation");
        assert_eq!(
            finding.violation_type.reason_code(),
            "semantic_gate_violation"
        );
        // field() returns None — semantic-gate violations are
        // state-scoped, not field-scoped.
        assert!(finding.violation_type.field().is_none());
    }

    // U1 (2026-06-17-003 plan): the four existing recoverable
    // buckets must keep their stable labels — adding
    // `SemanticGateViolation` to the enum must not shift them.
    #[test]
    fn u1_semantic_gate_violation_does_not_perturb_other_buckets() {
        assert_eq!(
            ReasonClass::PayloadTypeMismatch.as_str(),
            "payload_type_mismatch"
        );
        assert_eq!(
            ReasonClass::MissingRequiredField.as_str(),
            "missing_required_field"
        );
        assert_eq!(ReasonClass::TopicDenied.as_str(), "topic_denied");
        // And the non-recoverable ones stay non-recoverable.
        let finding = PolicyFinding {
            topic: "review.passed".to_string(),
            violation_type: ViolationType::TerminalMonotonicityViolation {
                terminal_topic: "plan.complete".to_string(),
                business_topic: "review.passed".to_string(),
            },
            message: "terminal monotonicity".to_string(),
        };
        assert!(is_recoverable_policy_finding(&finding).is_none());
    }

    // U1 (2026-06-17-003 plan): the `finding_to_payload_contract_violation`
    // bridge (in `event_loop/mod.rs`) maps a `PolicyFinding` to a
    // `PayloadContractViolation` only when the violation is
    // schema-derived. `SemanticGateViolation` must NOT be in that
    // set so the runner's `PayloadContractViolation` fatal branch
    // never fires for `review_passed_while_wave_open`. We re-test
    // here at the policy layer because the bridge is in `mod.rs`
    // and not exposed for direct unit testing without spinning up
    // an `EventLoop`. The downstream guarantee is:
    //   is_recoverable_policy_finding == Some(SemanticGateViolation)
    //   → bridge returns None → runner skips the fatal branch.
    #[test]
    fn u1_semantic_gate_is_recoverable_implies_not_fatal() {
        let finding = PolicyFinding {
            topic: "review.passed".to_string(),
            violation_type: ViolationType::SemanticGateViolation {
                gate: "review_passed_while_wave_open".to_string(),
                context: "wave='w-1' received=0/3 expected".to_string(),
                referenced_fields: Vec::new(),
            },
            message: "review-coordinator must not emit review.passed while wave is incomplete"
                .to_string(),
        };
        // Recoverable → never feeds the U6 fast-fail
        // (`capture_violation` in `event_loop/mod.rs` early-returns
        // when this returns `Some`).
        assert!(is_recoverable_policy_finding(&finding).is_some());
        // And the bridge arms for `AllowedValueMismatch` /
        // `MissingRequiredField` / `PayloadTypeMismatch` only —
        // `SemanticGateViolation` falls through to `return None`.
        // We re-state the arms here so a future enum expansion
        // that accidentally adds a new fatal mapping is caught
        // by this test.
        assert!(matches!(
            finding.violation_type,
            ViolationType::SemanticGateViolation { .. }
        ));
    }

    // -------------------------------------------------------------------------
    // 2026-06-24 P1-3: `work.ready` / `test.passed` / `test.failed` dedup
    //
    // Mirrors the U4 `work.done` and U5 `review.dimensions.complete`
    // dedup patterns. `work.ready` key is `(plan, step, task_id)`;
    // `test.passed` / `test.failed` key is
    // `(plan, step, task_id, fix_round)`.
    // -------------------------------------------------------------------------

    fn work_ready_payload(plan: &str, step: &str, task: &str) -> String {
        format!(r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","task_key":"k"}}"#)
    }

    fn test_result_payload(plan: &str, step: &str, task: &str, fix_round: u64) -> String {
        format!(
            r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","fix_round":{fix_round},"tests_run":10,"tests_passed":10}}"#
        )
    }

    #[test]
    fn p1_3_duplicate_work_ready_first_accepted() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = work_ready_payload("p1", "step-01", "t1");
        let decision = validate_event("work.ready", Some(&payload), &config, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "First work.ready for a new (plan, step, task) tuple must be accepted"
        );
        assert!(state.work_ready_seen_keys.contains_key("p1::step-01::t1"));
        assert_eq!(state.work_ready_seen_keys["p1::step-01::t1"], 1);
    }

    #[test]
    fn p1_3_duplicate_work_ready_second_rejected() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = work_ready_payload("p1", "step-01", "t1");

        let first = validate_event("work.ready", Some(&payload), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event("work.ready", Some(&payload), &config, &mut state);
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "p1::step-01::t1"
            ),
            "Second work.ready for same key must be rejected with DuplicateWorkDone, got {:?}",
            second
        );
    }

    #[test]
    fn p1_3_duplicate_work_ready_different_step_accepted() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let p1 = work_ready_payload("p1", "step-01", "t1");
        let first = validate_event("work.ready", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let p2 = work_ready_payload("p1", "step-02", "t1");
        let second = validate_event("work.ready", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "work.ready for a different step must be accepted"
        );
    }

    #[test]
    fn p1_3_duplicate_test_passed_same_fix_round_rejected() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = test_result_payload("p1", "step-01", "t1", 0);

        let first = validate_event("test.passed", Some(&payload), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event("test.passed", Some(&payload), &config, &mut state);
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "p1::step-01::t1::0"
            ),
            "Second test.passed for same fix_round must be rejected, got {:?}",
            second
        );
    }

    #[test]
    fn p1_3_duplicate_test_failed_same_fix_round_rejected() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = test_result_payload("p1", "step-01", "t1", 0);

        let first = validate_event("test.failed", Some(&payload), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event("test.failed", Some(&payload), &config, &mut state);
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "p1::step-01::t1::0"
            ),
            "Second test.failed for same fix_round must be rejected, got {:?}",
            second
        );
    }

    #[test]
    fn p1_3_test_passed_different_fix_round_accepted() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let p1 = test_result_payload("p1", "step-01", "t1", 0);
        let first = validate_event("test.passed", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let p2 = test_result_payload("p1", "step-01", "t1", 1);
        let second = validate_event("test.passed", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "test.passed with a different fix_round must be accepted"
        );
    }

    #[test]
    fn p1_3_test_passed_missing_fix_round_skips_dedup() {
        // Mirrors U6 KTD4: missing `fix_round` falls through so
        // the schema validator reports `missing_required_field`.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p1","step":"step-01","task_id":"t1","tests_run":10,"tests_passed":10}"#;

        let first = validate_event("test.passed", Some(payload), &config, &mut state);
        assert_eq!(
            first,
            PolicyDecision::Accept,
            "Missing fix_round must NOT be dedup-rejected"
        );
        assert!(
            state.test_passed_seen_keys.is_empty(),
            "Missing fix_round must NOT populate the dedup mirror"
        );
    }

    #[test]
    fn p1_3_fix_applied_prunes_test_result_buckets() {
        let mut state = PolicyRuntimeState::default();
        state
            .test_passed_seen_keys
            .insert("p1::step-01::t1::0".into());
        state
            .test_failed_seen_keys
            .insert("p1::step-01::t1::0".into());
        state
            .test_passed_seen_keys
            .insert("p1::step-01::t2::0".into());

        state.prune_test_result_buckets("p1", "step-01", "t1");

        assert!(!state.test_passed_seen_keys.contains("p1::step-01::t1::0"));
        assert!(!state.test_failed_seen_keys.contains("p1::step-01::t1::0"));
        // Sibling task t2 is preserved
        assert!(state.test_passed_seen_keys.contains("p1::step-01::t2::0"));
    }

    #[test]
    fn p1_3_fix_applied_prunes_work_ready_bucket() {
        // U5 of plan 2026-07-05-005 (fix-plan §R8): the dedup
        // counter is observation, not dedup state. The bucket
        // classification moves to `pruned_work_ready_buckets`,
        // but the dedup entries (and their counts) survive the
        // prune. Update the assertion accordingly: keys under
        // the pruned bucket stay in `work_ready_seen_keys`, and
        // keys outside it are untouched.
        let mut state = PolicyRuntimeState::default();
        state
            .work_ready_seen_keys
            .insert("p1::step-01::t1".into(), 1);
        state
            .work_ready_seen_keys
            .insert("p1::step-01::t2".into(), 1);
        state
            .work_ready_seen_keys
            .insert("p1::step-02::t1".into(), 1);

        state.prune_work_ready_bucket("p1", "step-01");

        // Pruned keys survive with their counts intact.
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
            Some(1)
        );
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t2").copied(),
            Some(1)
        );
        // Different step preserved.
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-02::t1").copied(),
            Some(1)
        );
        // Bucket side-table records the prune.
        assert!(state.pruned_work_ready_buckets.contains("p1::step-01::t1"));
        assert!(state.pruned_work_ready_buckets.contains("p1::step-01::t2"));
        assert!(!state.pruned_work_ready_buckets.contains("p1::step-02::t1"));
    }

    // 2026-07-02-004 U7: precheck `<X>.proposed` dedup (R6).

    // ─────────────────────────────────────────────────────────────────
    // U5 of plan 2026-07-05-005 (R8): work_ready_seen_keys is now a
    // HashMap<String, u32> so post-mortem tooling can distinguish a
    // single duplicate from a "dup storm". Only the work.ready
    // bucket is instrumented; the other 7 seen_keys fields stay as
    // HashSet to keep the change blast radius small.
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn u5_work_ready_dedup_counter_first_hit_is_one() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = work_ready_payload("p1", "step-01", "t1");
        let decision = validate_event("work.ready", Some(&payload), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
            Some(1),
            "U5: first work.ready hit must seed the counter at 1"
        );
    }

    #[test]
    fn u5_work_ready_dedup_counter_increments_on_repeat() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = work_ready_payload("p1", "step-01", "t1");

        validate_event("work.ready", Some(&payload), &config, &mut state);
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
            Some(1)
        );

        let second = validate_event("work.ready", Some(&payload), &config, &mut state);
        assert!(matches!(second, PolicyDecision::RejectWithResume(_)));
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
            Some(2),
            "U5: counter must bump on every observed hit"
        );

        let third = validate_event("work.ready", Some(&payload), &config, &mut state);
        assert!(matches!(third, PolicyDecision::RejectWithResume(_)));
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
            Some(3)
        );
    }

    #[test]
    fn u5_work_ready_prune_preserves_counter_on_remaining_keys() {
        let mut state = PolicyRuntimeState::default();
        state
            .work_ready_seen_keys
            .insert("p1::step-01::t1".into(), 7);
        state
            .work_ready_seen_keys
            .insert("p1::step-02::t2".into(), 3);

        state.prune_work_ready_bucket("p1", "step-01");

        // U5 of plan 2026-07-05-005 (fix-plan §R8): the dedup
        // hit counter is observation, not dedup state. The
        // pruned bucket's entry MUST survive (its count must
        // survive), only the bucket classification moves to the
        // side-table `pruned_work_ready_buckets`. Keys outside
        // the pruned bucket are untouched (counter preserved).
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
            Some(7),
            "U5: pruned key's counter is preserved across pruning"
        );
        assert!(state.pruned_work_ready_buckets.contains("p1::step-01::t1"));
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-02::t2").copied(),
            Some(3),
            "U5: counter is observation, not dedup state — pruning \
             other buckets does not reset surviving keys' counts"
        );
        assert!(!state.pruned_work_ready_buckets.contains("p1::step-02::t2"));
    }

    /// U5 of plan 2026-07-05-005 (fix-plan §R8): after a bucket
    /// prune, a re-emit with the same `(plan_name, step, task_id)`
    /// lands as Accept (the bucket classification is cleared), and
    /// the existing counter is incremented — **not** reset to 1.
    #[test]
    fn u5_work_ready_prune_preserves_counter_on_pruned_key() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p1","step":"step-01","task_id":"t1"}"#;

        // First emit accepted → seed count=1.
        let first = validate_event("work.ready", Some(payload), &config, &mut state);
        assert!(matches!(first, PolicyDecision::Accept));
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
            Some(1)
        );

        // Second emit (without prune) → RejectWithResume, count=2.
        let second = validate_event("work.ready", Some(payload), &config, &mut state);
        assert!(matches!(second, PolicyDecision::RejectWithResume(_)));
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
            Some(2)
        );

        // Prune the bucket; dedup entry must survive in the
        // counter map; the bucket side-table records the prune.
        state.prune_work_ready_bucket("p1", "step-01");
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
            Some(2),
            "U5: count survives the prune"
        );
        assert!(state.pruned_work_ready_buckets.contains("p1::step-01::t1"));

        // Third emit (post-prune) → Accept, count increments to 3.
        let third = validate_event("work.ready", Some(payload), &config, &mut state);
        assert!(
            matches!(third, PolicyDecision::Accept),
            "U5: post-prune re-emit must accept, got {third:?}"
        );
        assert_eq!(
            state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
            Some(3),
            "U5: count is incremented, not reset to 1"
        );
    }

    #[test]
    fn u5_other_seen_keys_still_hashset() {
        // Anti-regression: the other 7 seen_keys fields MUST
        // remain HashSet<String>; only work_ready_seen_keys was
        // widened to HashMap<String, u32>.
        //
        // Type-system guard: this test is a tautology in the
        // sense that converting any of these fields from
        // `HashSet<String>` to `HashMap<String, _>` would be a
        // compile error (the field types are pinned by the
        // struct definition). The assert is a sanity belt-and-
        // suspenders check; the real protection is the type
        // system. If you see this test "failing" because of a
        // future refactor, the right answer is to widen
        // work_ready_seen_keys's pattern to a sibling field
        // deliberately — not to weaken this assertion.
        use std::collections::HashSet;
        let mut state = PolicyRuntimeState::default();
        let work_done_keys: HashSet<String> = HashSet::new();
        state.work_done_seen_keys = work_done_keys;
        let dim_ready_keys: HashSet<String> = HashSet::new();
        state.review_dimension_ready_seen_keys = dim_ready_keys;
        let dim_complete_keys: HashSet<String> = HashSet::new();
        state.review_dimensions_complete_seen_keys = dim_complete_keys;
        let passed_keys: HashSet<String> = HashSet::new();
        state.test_passed_seen_keys = passed_keys;
        let failed_keys: HashSet<String> = HashSet::new();
        state.test_failed_seen_keys = failed_keys;
        let review_start_keys: HashSet<String> = HashSet::new();
        state.review_start_seen_keys = review_start_keys;
        assert!(state.work_ready_seen_keys.is_empty());
    }

    #[test]
    fn u7_precheck_proposed_dedup_rejects_duplicate_candidate() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"step":"s1"}"#;

        let first = validate_event("work.done.proposed", Some(payload), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event("work.done.proposed", Some(payload), &config, &mut state);
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "work.done::{\"step\":\"s1\"}"
            ),
            "duplicate work.done.proposed must be rejected, got {:?}",
            second
        );
    }

    #[test]
    fn u7_precheck_proposed_cleared_on_rejected_allows_retry() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"step":"s1"}"#;

        assert_eq!(
            validate_event("work.done.proposed", Some(payload), &config, &mut state),
            PolicyDecision::Accept
        );
        assert_eq!(
            validate_event(
                "work.done.rejected",
                Some(r#"{"failed_checks":[1],"reason":"no","synthetic":false}"#),
                &config,
                &mut state
            ),
            PolicyDecision::Accept
        );
        assert_eq!(
            validate_event("work.done.proposed", Some(payload), &config, &mut state),
            PolicyDecision::Accept,
            "after gate rejection the same candidate may be re-proposed"
        );
    }

    #[test]
    fn u7_build_allowed_topics_includes_precheck_derived_topics() {
        use crate::config::RalphConfig;
        let yaml = r#"
event_loop:
  precheck:
    enabled: true
    rules:
      work.done:
        prompt: ["ok"]
        on_fail:
          target: executor
hats:
  executor:
    name: "Executor"
    triggers: ["task.start"]
    publishes: ["work.done"]
"#;
        let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        config.normalize();
        let allowed = build_allowed_topics(
            &config.hats,
            "LOOP_COMPLETE",
            config.event_loop.event_policy.as_ref(),
        );
        assert!(allowed.contains("work.done.proposed"));
        assert!(allowed.contains("work.done.rejected"));
        assert!(allowed.contains("work.done"));
    }

    // ─────────────────────────────────────────────────────────────────────
    // U2 + U6 (plan 2026-07-04-004): `PolicyDecision::AcknowledgeAndForward`
    // + the `ReviewDimensionsComplete` hint mapping. Together they
    // carve out the silent-success lane so `review.dimensions.complete`
    // re-emits do not trigger `task.resume` storms, while keeping the
    // dedup invariant intact.
    // ─────────────────────────────────────────────────────────────────────

    /// `PolicyDecision` now exposes a 7th variant: `AcknowledgeAndForward`.
    /// Pin the variant count + the new variant's existence so static
    /// assertions across the workspace stay in sync (the project
    /// sealed-style helper `ensure_sealed_enum()` no longer compiles
    /// when a new variant is added without updating the call sites
    /// listed in `find_referencing_symbols`).
    #[test]
    fn test_policy_decision_has_acknowledge_and_forward_variant() {
        // (a) AcknowledgeAndForward is constructible with a PolicyFinding.
        let finding = PolicyFinding {
            topic: "review.dimensions.complete".to_string(),
            violation_type: ViolationType::DuplicateWorkDone {
                key: "k".to_string(),
                hint: DuplicateWorkDoneHint::ReviewDimensionsComplete,
                seen_count: None,
            },
            message: "test".to_string(),
        };
        let decision = PolicyDecision::AcknowledgeAndForward(finding.clone());
        match decision {
            PolicyDecision::AcknowledgeAndForward(f) => {
                assert_eq!(f.topic, "review.dimensions.complete");
                // reason_code is derived from `violation_type` (per
                // `ViolationType::reason_code()`); verify the
                // `ReviewDimensionsComplete` hint mapping here.
                assert_eq!(
                    f.violation_type.reason_code(),
                    "duplicate_review_dimensions_complete"
                );
            }
            other => panic!("expected AcknowledgeAndForward, got {other:?}"),
        }

        // (b) Total enum variant count is 7 (Accept / Warn /
        // RejectWithResume / Hold / AcknowledgeAndForward / Block /
        // Ignore). If a future Unit adds another variant this
        // assertion fails fast and the author is forced to re-pin
        // the contract here.
        let all = [
            std::mem::discriminant(&PolicyDecision::Accept),
            std::mem::discriminant(&PolicyDecision::Warn(vec![])),
            std::mem::discriminant(&PolicyDecision::RejectWithResume(finding.clone())),
            std::mem::discriminant(&PolicyDecision::Hold(finding.clone())),
            std::mem::discriminant(&PolicyDecision::AcknowledgeAndForward(finding.clone())),
            std::mem::discriminant(&PolicyDecision::Block(finding.clone())),
            std::mem::discriminant(&PolicyDecision::Ignore(finding)),
        ];
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(
            unique.len(),
            7,
            "PolicyDecision must have 7 distinct variants after U2"
        );
    }

    /// Second emit of `review.dimensions.complete` for the same
    /// `(plan, step, task, fix_round)` tuple returns
    /// `AcknowledgeAndForward(PolicyFinding{ reason_code: "duplicate_review_dimensions_complete", ... })`
    /// instead of `RejectWithResume`. This is the U2 carve-out that
    /// prevents silent-success dedup storms.
    #[test]
    fn test_review_dimensions_complete_dedup_hit_returns_acknowledge_and_forward() {
        let mut config = test_config();
        // Allow the topic + fields used by `review.dimensions.complete`.
        config.schemas.insert(
            "review.dimensions.complete".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".to_string(),
                    "step".to_string(),
                    "task_id".to_string(),
                    "fix_round".to_string(),
                ],
                ..Default::default()
            },
        );
        let mut state = PolicyRuntimeState::default();

        let payload = r#"{"plan_name":"p","step":"s","task_id":"t","fix_round":1}"#;
        // First emit is accepted.
        let first = validate_event_with_hat(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
            None,
        );
        assert_eq!(first, PolicyDecision::Accept);

        // Second emit with the same key returns AcknowledgeAndForward.
        let second = validate_event_with_hat(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
            None,
        );
        match second {
            PolicyDecision::AcknowledgeAndForward(finding) => {
                assert_eq!(finding.topic, "review.dimensions.complete");
                assert_eq!(
                    finding.violation_type.reason_code(),
                    "duplicate_review_dimensions_complete"
                );
            }
            other => panic!(
                "expected AcknowledgeAndForward, got {other:?}; \
                 the U2 silent-success carve-out must apply to dedup hits"
            ),
        }
    }

    /// First emit of `review.dimensions.complete` is still accepted;
    /// the carve-out must not regress the happy path.
    #[test]
    fn test_review_dimensions_complete_first_emit_still_accepts() {
        let mut config = test_config();
        config.schemas.insert(
            "review.dimensions.complete".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".to_string(),
                    "step".to_string(),
                    "task_id".to_string(),
                    "fix_round".to_string(),
                ],
                ..Default::default()
            },
        );
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","step":"s","task_id":"t","fix_round":7}"#;
        let decision = validate_event_with_hat(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
            None,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    /// Other-topic dedup branches (e.g. `work.done`) continue to
    /// return `RejectWithResume` — the U2 carve-out is intentionally
    /// narrow and applies only to `review.dimensions.complete`.
    #[test]
    fn test_other_topic_dedup_still_rejects_with_resume() {
        let mut config = test_config();
        config.schemas.insert(
            "work.done".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".to_string(),
                    "step".to_string(),
                    "task_id".to_string(),
                ],
                ..Default::default()
            },
        );
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","step":"s","task_id":"t"}"#;
        // First emit accepted.
        let first = validate_event_with_hat("work.done", Some(payload), &config, &mut state, None);
        assert_eq!(first, PolicyDecision::Accept);
        // Second emit returns RejectWithResume (unchanged behaviour).
        let second = validate_event_with_hat("work.done", Some(payload), &config, &mut state, None);
        match second {
            PolicyDecision::RejectWithResume(finding) => {
                assert_eq!(finding.topic, "work.done");
            }
            other => panic!(
                "expected RejectWithResume for work.done dedup, got {other:?}; \
                 the U2 carve-out must NOT extend to work.done"
            ),
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // U6 (plan 2026-07-04-004): `DuplicateWorkDoneHint::ReviewDimensionsComplete`
    // split. The hint + reason_code are distinct from the legacy
    // `DuplicateSameStep` / `duplicate_work_done` so dashboards
    // can match the silent-success dedup lane independently.
    // The hint was added in U2 alongside `AcknowledgeAndForward`;
    // U6 pins the reason_code mapping via dedicated tests so
    // future renames of the code literal are caught.
    // ─────────────────────────────────────────────────────────────────────

    /// `DuplicateWorkDoneHint` exposes 4 variants after U6
    /// (DuplicateStallBypass / DuplicateSameStep /
    /// ReviewDimensionDuplicate / ReviewDimensionsComplete).
    /// Pin the variant count so static assertions across the
    /// workspace stay in sync.
    #[test]
    fn test_duplicate_work_done_hint_has_review_dimensions_complete_variant() {
        let all = [
            DuplicateWorkDoneHint::DuplicateStallBypass,
            DuplicateWorkDoneHint::DuplicateSameStep,
            DuplicateWorkDoneHint::ReviewDimensionDuplicate,
            DuplicateWorkDoneHint::ReviewDimensionsComplete,
        ];
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(
            unique.len(),
            4,
            "DuplicateWorkDoneHint must have 4 distinct variants after U6"
        );
    }

    /// `ReviewDimensionsComplete` hint maps to a distinct
    /// `duplicate_review_dimensions_complete` reason code (NOT
    /// the misleading generic `duplicate_work_done`). The
    /// distinct code is what dashboards / BDD scenarios pin
    /// against to match the silent-success dedup lane.
    #[test]
    fn test_review_dimensions_complete_duplicate_emits_distinct_reason_code() {
        let finding = PolicyFinding {
            topic: "review.dimensions.complete".to_string(),
            violation_type: ViolationType::DuplicateWorkDone {
                key: "p::s::t::0".to_string(),
                hint: DuplicateWorkDoneHint::ReviewDimensionsComplete,
                seen_count: None,
            },
            message: "test".to_string(),
        };
        assert_eq!(
            finding.violation_type.reason_code(),
            "duplicate_review_dimensions_complete",
            "ReviewDimensionsComplete hint MUST map to its own distinct reason_code"
        );
    }

    /// `ReviewDimensionDuplicate` hint (used by
    /// `review.dimension.ready` dedup) keeps its distinct
    /// `duplicate_review_dimension_ready` reason code; U6
    /// must NOT regress that mapping.
    #[test]
    fn test_review_dimension_ready_duplicate_still_uses_review_dimension_duplicate() {
        let finding = PolicyFinding {
            topic: "review.dimension.ready".to_string(),
            violation_type: ViolationType::DuplicateWorkDone {
                key: "p::s::t::d".to_string(),
                hint: DuplicateWorkDoneHint::ReviewDimensionDuplicate,
                seen_count: None,
            },
            message: "test".to_string(),
        };
        assert_eq!(
            finding.violation_type.reason_code(),
            "duplicate_review_dimension_ready",
            "ReviewDimensionDuplicate hint must keep its distinct code (regression guard)"
        );
    }

    /// `DuplicateStallBypass` / `DuplicateSameStep` share the legacy
    /// `duplicate_work_done` reason code (per plan 2026-07-05-005
    /// fix-plan U3 / KTD-3); the disambiguation hint string travels
    /// on `RecoveryDiagnosisEnvelope.hint`. The two review-dimension
    /// hints keep their distinct codes (regression guard).
    #[test]
    fn test_other_topics_dedup_hint_carries_in_envelope() {
        // U3 of plan 2026-07-05-005 (R3, R9): restore the stable
        // external contract per KTD-3 — single `duplicate_work_done`
        // reason_code for the `DuplicateSameStep` and
        // `DuplicateStallBypass` variants. The `hint` field on
        // `RecoveryDiagnosisEnvelope` carries the discriminator
        // (`duplicate_work_done_same_step` /
        // `duplicate_work_done_stall_bypass`) so post-mortem
        // tooling can still distinguish the two paths. This test
        // pins both surfaces.
        let cases = [
            (
                DuplicateWorkDoneHint::DuplicateStallBypass,
                "duplicate_work_done",
                "duplicate_work_done_stall_bypass",
            ),
            (
                DuplicateWorkDoneHint::DuplicateSameStep,
                "duplicate_work_done",
                "duplicate_work_done_same_step",
            ),
            (
                DuplicateWorkDoneHint::ReviewDimensionDuplicate,
                "duplicate_review_dimension_ready",
                "duplicate_review_dimension_ready",
            ),
            (
                DuplicateWorkDoneHint::ReviewDimensionsComplete,
                "duplicate_review_dimensions_complete",
                "duplicate_review_dimensions_complete",
            ),
        ];
        for (hint, expected_code, expected_hint) in cases {
            let finding = PolicyFinding {
                topic: "work.done".to_string(),
                violation_type: ViolationType::DuplicateWorkDone {
                    key: "p::s::t".to_string(),
                    hint,
                    seen_count: None,
                },
                message: "test".to_string(),
            };
            assert_eq!(
                finding.violation_type.reason_code(),
                expected_code,
                "{hint:?} must surface its stable reason_code"
            );
            assert_eq!(
                hint.as_hint_str(),
                expected_hint,
                "{hint:?} must surface its stable hint string"
            );
        }
    }

    /// The 4 hint → reason_code mappings are pairwise distinct
    /// across the three "named" lanes (StallBypass + SameStep
    /// share the legacy `duplicate_work_done` code by design —
    /// see plan 2026-07-04-004 KTD-2: keep those two collapsed
    /// so existing dashboards / CLI precheck JSON / static
    /// assertions remain green). The U6 invariant is therefore
    /// "3 distinct codes" not "4 distinct codes"; this test
    /// guards against accidentally collapsing a `review.*` lane
    /// into the legacy generic code (which would re-introduce
    /// the silent-success misclassification).
    #[test]
    fn test_distinct_reason_codes_invariant() {
        let codes = [
            (
                "DuplicateStallBypass",
                ViolationType::DuplicateWorkDone {
                    key: "k".to_string(),
                    hint: DuplicateWorkDoneHint::DuplicateStallBypass,
                    seen_count: None,
                }
                .reason_code(),
            ),
            (
                "DuplicateSameStep",
                ViolationType::DuplicateWorkDone {
                    key: "k".to_string(),
                    hint: DuplicateWorkDoneHint::DuplicateSameStep,
                    seen_count: None,
                }
                .reason_code(),
            ),
            (
                "ReviewDimensionDuplicate",
                ViolationType::DuplicateWorkDone {
                    key: "k".to_string(),
                    hint: DuplicateWorkDoneHint::ReviewDimensionDuplicate,
                    seen_count: None,
                }
                .reason_code(),
            ),
            (
                "ReviewDimensionsComplete",
                ViolationType::DuplicateWorkDone {
                    key: "k".to_string(),
                    hint: DuplicateWorkDoneHint::ReviewDimensionsComplete,
                    seen_count: None,
                }
                .reason_code(),
            ),
        ];
        let mut unique = std::collections::HashSet::new();
        for (_, code) in &codes {
            unique.insert(*code);
        }
        // U3 of plan 2026-07-05-005 (fix-plan §R3 / KTD-3): StallBypass
        // and SameStep share the legacy `duplicate_work_done`
        // reason_code per the stable external contract; the
        // disambiguation hint string travels on
        // `RecoveryDiagnosisEnvelope.hint`. The three "named"
        // lanes (StallBypass+SameStep collapsed, plus
        // ReviewDimensionDuplicate and ReviewDimensionsComplete)
        // must produce **3 distinct** codes. Merging any of
        // them would re-introduce the silent-success
        // misclassification — fail fast.
        assert_eq!(
            unique.len(),
            3,
            "expected 3 distinct reason codes (StallBypass+SameStep collapsed → \
             duplicate_work_done, plus ReviewDimensionDuplicate and ReviewDimensionsComplete); \
             got {codes:?}"
        );
        assert!(
            unique.contains("duplicate_work_done"),
            "DuplicateSameStep+DuplicateStallBypass must collapse to duplicate_work_done under U3"
        );
        assert!(
            unique.contains("duplicate_review_dimension_ready"),
            "ReviewDimensionDuplicate keeps its distinct code under U3"
        );
        assert!(
            unique.contains("duplicate_review_dimensions_complete"),
            "ReviewDimensionsComplete keeps its distinct code under U3"
        );
    }

    // ------------------------------------------------------------------
    // U8 tests: handoff_envelope policy-check wiring.
    // ------------------------------------------------------------------

    use crate::config::HandoffEnvelopeConfig;

    struct StubHandoff {
        enabled: bool,
        validate_payload: bool,
    }

    impl HandoffEnvelopeConfigAccess for StubHandoff {
        fn handoff_envelope_enabled(&self) -> bool {
            self.enabled
        }
        fn handoff_envelope_validate_payload(&self) -> bool {
            self.validate_payload
        }
    }

    fn full_payload() -> serde_json::Value {
        serde_json::json!({
            "plan_name": "2026-07-06-u8-fixture",
            "plan_path": "docs/plans/2026-07-06-u8-fixture.md",
            "task_id": "task-live-id",
            "task_key": "2026-07-06-u8-fixture:step-3:implement",
            "step": "step-3",
            "handoff_envelope": {
                "schema_version": "handoff-envelope.v1",
                "root_goal": "ship the plan without regressions",
                "plan": {
                    "name": "2026-07-06-u8-fixture",
                    "path": "docs/plans/2026-07-06-u8-fixture.md",
                    "current_step": "step-3",
                    "completed_steps": ["step-1", "step-2"]
                },
                "state": {
                    "current_status": "ready_for_review",
                    "last_signal": "work.done",
                    "blocking_reason": null
                },
                "receiver_contract": {
                    "to_hat": "goal-alignment-reviewer",
                    "must_do": ["review step-3"],
                    "must_not_do": ["regress step-2"],
                    "success_signal": "work.done",
                    "failure_signal": "work.failed"
                }
            }
        })
    }

    fn policy_minimal() -> EventPolicyConfig {
        use crate::config::{EventPolicyMode, ViolationAction};
        // U1 (2026-07-06-004 fix-plan) does NOT change the
        // gate semantics — `validate_event_with_options`'s
        // `check_handoff_envelope` gate keeps running for
        // every event whose payload parses as a JSON object
        // whenever the typed `validate_payload: true` flag
        // is on. The wire-up merely replaces the no-op
        // `DefaultHandoffConfig` with the real typed
        // `EventLoopHandoffConfig` so the gate fires at the
        // production CLI / loop boundary instead of being
        // invisible behind a default-off trait.
        EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::RejectWithResume,
            schemas: HashMap::new(),
            schema_file: None,
            terminal_topics: vec![],
            business_topics: vec![],
            require_policy_check_for_cli_emit: false,
            allow_unsafe_cli_emit: true,
            require_emit_provenance: false,
            completion_after_terminal: Default::default(),
            topic_deny_rules: vec![],
            payload_consistency: Default::default(),
            plan_name_equality_required: false,
        }
    }

    #[test]
    fn policy_check_does_not_require_handoff_envelope_when_disabled() {
        // Default-closed: no flags set, no envelope required.
        let policy = policy_minimal();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_options(
            "work.done",
            Some(&serde_json::to_string(&full_payload()).unwrap()),
            &policy,
            &mut state,
            None,
            &StubHandoff {
                enabled: false,
                validate_payload: false,
            },
        );
        assert!(
            matches!(decision, PolicyDecision::Accept),
            "disabled flag must not gate; got {:?}",
            decision
        );
    }

    #[test]
    fn policy_check_rejects_missing_handoff_envelope_when_validation_enabled() {
        let mut payload = full_payload();
        payload.as_object_mut().unwrap().remove("handoff_envelope");

        // U1: now actually wired — uses the original
        // `policy_minimal()` (no schema declared) and asserts
        // the `check_handoff_envelope` validator fires.
        let policy = policy_minimal();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_options(
            "work.done",
            Some(&serde_json::to_string(&payload).unwrap()),
            &policy,
            &mut state,
            None,
            &StubHandoff {
                enabled: true,
                validate_payload: true,
            },
        );
        match decision {
            PolicyDecision::RejectWithResume(f)
            | PolicyDecision::Hold(f)
            | PolicyDecision::AcknowledgeAndForward(f) => {
                assert!(
                    f.message.contains("handoff_envelope_missing"),
                    "missing envelope must surface; got finding: {:?}",
                    f
                );
            }
            other => panic!("expected rejection, got {:?}", other),
        }
    }

    #[test]
    fn policy_check_rejects_invalid_handoff_envelope_when_validation_enabled() {
        let mut payload = full_payload();
        payload["handoff_envelope"]["schema_version"] = serde_json::json!("wrong");
        let policy = policy_minimal();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_options(
            "work.done",
            Some(&serde_json::to_string(&payload).unwrap()),
            &policy,
            &mut state,
            None,
            &StubHandoff {
                enabled: true,
                validate_payload: true,
            },
        );
        match decision {
            PolicyDecision::RejectWithResume(f)
            | PolicyDecision::Hold(f)
            | PolicyDecision::AcknowledgeAndForward(f) => {
                assert!(
                    f.message
                        .contains("handoff_envelope_invalid_schema_version"),
                    "invalid schema version must surface; got finding: {:?}",
                    f
                );
            }
            other => panic!("expected rejection, got {:?}", other),
        }
    }

    #[test]
    fn policy_check_accepts_valid_handoff_envelope_when_validation_enabled() {
        let policy = policy_minimal();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_options(
            "work.done",
            Some(&serde_json::to_string(&full_payload()).unwrap()),
            &policy,
            &mut state,
            None,
            &StubHandoff {
                enabled: true,
                validate_payload: true,
            },
        );
        assert!(
            matches!(decision, PolicyDecision::Accept),
            "valid envelope must accept; got {:?}",
            decision
        );
    }

    #[test]
    fn handoff_envelope_validation_enabled_gate_is_correct() {
        let cfg_disabled = StubHandoff {
            enabled: false,
            validate_payload: true,
        };
        let cfg_validate_only = StubHandoff {
            enabled: true,
            validate_payload: false,
        };
        let cfg_full = StubHandoff {
            enabled: true,
            validate_payload: true,
        };
        assert!(!handoff_envelope_validation_enabled(
            Some("{}"),
            &cfg_disabled
        ));
        assert!(!handoff_envelope_validation_enabled(
            Some("{}"),
            &cfg_validate_only
        ));
        assert!(handoff_envelope_validation_enabled(Some("{}"), &cfg_full));
        assert!(!handoff_envelope_validation_enabled(None, &cfg_full));
    }

    #[test]
    fn event_loop_handoff_config_adapter_projects_typed_config() {
        let cfg = HandoffEnvelopeConfig {
            enabled: true,
            prompt_injection: true,
            validate_payload: true,
            emit_result_summary: false,
        };
        let adapter = EventLoopHandoffConfig {
            handoff_envelope: &cfg,
        };
        assert!(adapter.handoff_envelope_enabled());
        assert!(adapter.handoff_envelope_validate_payload());

        let cfg_off = HandoffEnvelopeConfig::default();
        let adapter = EventLoopHandoffConfig {
            handoff_envelope: &cfg_off,
        };
        assert!(!adapter.handoff_envelope_enabled());
        assert!(!adapter.handoff_envelope_validate_payload());
    }

    // -----------------------------------------------------------------
    // U3 (plan 2026-07-22-004): payload_consistency gate wiring tests.
    // The gate reuses `ViolationType::SemanticGateViolation` with a
    // `payload_consistency:<rule_id>` gate prefix and fires only when
    // `config.payload_consistency.enabled` is true and a rule declared
    // for the current topic hits the current payload (R2: current
    // payload only — no event history).
    // -----------------------------------------------------------------

    fn consistency_rule(
        id: &str,
        topic: &str,
        when: Value,
        message: &str,
    ) -> crate::config::PayloadConsistencyRule {
        crate::config::PayloadConsistencyRule {
            id: id.to_string(),
            topic: topic.to_string(),
            when,
            message: message.to_string(),
        }
    }

    /// The canonical `fix.done` self-contradiction rule used across tests.
    fn fix_done_contradiction_rule() -> crate::config::PayloadConsistencyRule {
        consistency_rule(
            "fix-done-no-fixes",
            "fix.done",
            serde_json::json!({"all": [
                {"field": "review_verdict", "eq": "blocked"},
                {"field": "fixes_applied", "eq": 0},
                {"field": "fix_status", "eq": "applied"}
            ]}),
            "fix.done claims applied but no fixes were applied while verdict is blocked",
        )
    }

    fn consistency_config(
        enabled: bool,
        rules: Vec<crate::config::PayloadConsistencyRule>,
    ) -> EventPolicyConfig {
        let mut config = test_config();
        config.payload_consistency = crate::config::PayloadConsistencyConfig { enabled, rules };
        config
    }

    const HITTING_PAYLOAD: &str =
        r#"{"review_verdict":"blocked","fixes_applied":0,"fix_status":"applied"}"#;

    #[test]
    fn payload_consistency_happy_payload_not_matching_rule_is_accepted() {
        // S1: payload does NOT satisfy the rule's `when` → Accept, no finding.
        let config = consistency_config(true, vec![fix_done_contradiction_rule()]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event(
            "fix.done",
            Some(r#"{"review_verdict":"passed","fixes_applied":2,"fix_status":"applied"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn payload_consistency_hitting_payload_is_rejected_with_semantic_gate() {
        // S2: payload satisfies `when` → RejectWithResume carrying a
        // SemanticGateViolation whose gate is `payload_consistency:<id>`.
        let config = consistency_config(true, vec![fix_done_contradiction_rule()]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(HITTING_PAYLOAD), &config, &mut state);

        let PolicyDecision::RejectWithResume(finding) = decision else {
            panic!("expected RejectWithResume, got {decision:?}");
        };
        let ViolationType::SemanticGateViolation { gate, context, .. } = &finding.violation_type
        else {
            panic!(
                "expected SemanticGateViolation, got {:?}",
                finding.violation_type
            );
        };
        assert!(
            gate.starts_with("payload_consistency:"),
            "gate must start with payload_consistency: prefix, got {gate}"
        );
        assert!(
            gate.contains("fix-done-no-fixes"),
            "gate must contain the rule id, got {gate}"
        );
        assert_eq!(gate, "payload_consistency:fix-done-no-fixes");
        // context should carry the rule's actionable message.
        assert!(
            context.contains("no fixes were applied"),
            "context should reflect rule message, got {context}"
        );
    }

    #[test]
    fn payload_consistency_disabled_does_not_fire() {
        // S4: enabled=false with a hitting payload → Accept (gate off by default).
        let config = consistency_config(false, vec![fix_done_contradiction_rule()]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(HITTING_PAYLOAD), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn payload_consistency_rule_is_scoped_to_its_topic() {
        // Topic filter: rule declared for fix.done, emit work.done with a
        // hitting payload → Accept (rule must not fire for other topics).
        let config = consistency_config(true, vec![fix_done_contradiction_rule()]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("work.done", Some(HITTING_PAYLOAD), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn payload_consistency_first_hit_in_declaration_order_wins() {
        // Two rules both hit; the surfaced finding carries the FIRST
        // rule's id (stable declaration order).
        let first = consistency_rule(
            "first-rule",
            "fix.done",
            serde_json::json!({"field": "review_verdict", "eq": "blocked"}),
            "first rule message",
        );
        let second = consistency_rule(
            "second-rule",
            "fix.done",
            serde_json::json!({"field": "fix_status", "eq": "applied"}),
            "second rule message",
        );
        let config = consistency_config(true, vec![first, second]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(HITTING_PAYLOAD), &config, &mut state);

        let PolicyDecision::RejectWithResume(finding) = decision else {
            panic!("expected RejectWithResume, got {decision:?}");
        };
        let ViolationType::SemanticGateViolation { gate, .. } = &finding.violation_type else {
            panic!(
                "expected SemanticGateViolation, got {:?}",
                finding.violation_type
            );
        };
        assert_eq!(gate, "payload_consistency:first-rule");
    }

    #[test]
    fn payload_consistency_rule_message_is_surfaced_to_agent() {
        // Message passthrough: the finding message/context reflects the
        // rule's `message` so the agent gets actionable guidance.
        let rule = consistency_rule(
            "msg-rule",
            "fix.done",
            serde_json::json!({"field": "review_verdict", "eq": "blocked"}),
            "ACTIONABLE: re-run the fixer before claiming fix.done",
        );
        let config = consistency_config(true, vec![rule]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(HITTING_PAYLOAD), &config, &mut state);

        let PolicyDecision::RejectWithResume(finding) = decision else {
            panic!("expected RejectWithResume, got {decision:?}");
        };
        let ViolationType::SemanticGateViolation { context, .. } = &finding.violation_type else {
            panic!(
                "expected SemanticGateViolation, got {:?}",
                finding.violation_type
            );
        };
        assert!(
            context.contains("ACTIONABLE: re-run the fixer"),
            "context must carry the rule message, got {context}"
        );
        assert!(
            finding.message.contains("ACTIONABLE: re-run the fixer"),
            "finding.message must carry the rule message, got {}",
            finding.message
        );
        assert!(
            finding.message.contains("payload_consistency:msg-rule"),
            "finding.message must name the gate, got {}",
            finding.message
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Unit 2 of plan 2026-07-27-002: evaluate_candidate_emit.
    // ─────────────────────────────────────────────────────────────────────

    /// Helper: build a minimal config with a single hat that publishes and
    /// triggers on `work.ready`, with an EventPolicyConfig that requires
    /// `task_key` as a required field on `work.ready`.
    fn candidate_emit_test_config() -> RalphConfig {
        use crate::config::{
            EventPolicyConfig, EventPolicyMode, EventSchema, HatConfig, PayloadType,
            ViolationAction,
        };
        let mut cfg = RalphConfig::default();

        let hat_cfg = HatConfig {
            name: "worker".to_string(),
            publishes: vec!["work.ready".to_string()],
            triggers: vec!["build.task".to_string()],
            ..Default::default()
        };
        cfg.hats.insert("worker".to_string(), hat_cfg);

        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec!["task_key".to_string()],
            ..Default::default()
        };
        let policy = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::RejectWithResume,
            schemas: [("work.ready".to_string(), schema)].into_iter().collect(),
            ..Default::default()
        };
        cfg.event_loop.event_policy = Some(policy);
        cfg
    }

    #[test]
    fn evaluate_candidate_emit_accepts_valid_payload() {
        let config = candidate_emit_test_config();
        let hat_id = ralph_proto::HatId::new("worker");
        let payload = r#"{"task_key": "task-123"}"#;

        let result = evaluate_candidate_emit(&config, &hat_id, "work.ready", payload, None)
            .expect("evaluation should succeed");
        assert_eq!(result.policy_decision, "accept");
        assert!(
            result.reasons.is_empty(),
            "accepted emit should have no reasons, got {:?}",
            result.reasons
        );
    }

    #[test]
    fn evaluate_candidate_emit_rejects_missing_required_field() {
        let config = candidate_emit_test_config();
        let hat_id = ralph_proto::HatId::new("worker");
        // Missing the required `task_key` field.
        let payload = r#"{"other_field": "value"}"#;

        let result = evaluate_candidate_emit(&config, &hat_id, "work.ready", payload, None)
            .expect("evaluation should succeed");
        assert_eq!(result.policy_decision, "reject");
        assert!(
            !result.reasons.is_empty(),
            "rejected emit should have at least one reason"
        );
        // The reason should mention the missing field (exact gate label
        // depends on the validation path; check that at least one reason
        // exists).
        assert_eq!(
            result.reasons[0].reason_code, "missing_required_field",
            "expected missing_required_field reason, got {:?}",
            result.reasons[0]
        );
    }

    #[test]
    fn evaluate_candidate_emit_equivalence_with_validate() {
        let config = candidate_emit_test_config();
        let hat_id = ralph_proto::HatId::new("worker");
        let policy_config = config.event_loop.event_policy.as_ref().unwrap();

        // Same valid payload: both paths should accept.
        let valid_payload = r#"{"task_key": "abc"}"#;
        let candidate =
            evaluate_candidate_emit(&config, &hat_id, "work.ready", valid_payload, None)
                .expect("evaluation");

        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_hat(
            "work.ready",
            Some(valid_payload),
            policy_config,
            &mut state,
            Some("worker"),
        );

        // evaluate_candidate_emit should say accept when validate_event_with_hat
        // says Accept or Warn.
        assert_eq!(
            candidate.policy_decision, "accept",
            "evaluate_candidate_emit must accept when validate_event_with_hat is {:?}",
            decision
        );

        // Same invalid payload (missing field): both paths should reject.
        let invalid_payload = r#"{}"#;
        let candidate2 =
            evaluate_candidate_emit(&config, &hat_id, "work.ready", invalid_payload, None)
                .expect("evaluation");
        assert_eq!(candidate2.policy_decision, "reject");
    }

    // ─────────────────────────────────────────────────────────────────────
    // Unit 3 of plan 2026-07-27-002: build_projection_preview must return
    // real state_changes when (and only when) the candidate was accepted.
    // ─────────────────────────────────────────────────────────────────────

    /// Helper: config for testing projection on accepted/rejected review.start.
    fn projection_test_config() -> RalphConfig {
        use crate::config::{
            EventPolicyConfig, EventPolicyMode, EventSchema, HatConfig, PayloadType,
            ViolationAction,
        };
        let mut cfg = RalphConfig::default();

        let hat_cfg = HatConfig {
            name: "reviewer".to_string(),
            publishes: vec!["review.start".to_string()],
            triggers: vec!["build.task".to_string()],
            ..Default::default()
        };
        cfg.hats.insert("reviewer".to_string(), hat_cfg);

        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec!["plan_name".to_string(), "task_id".to_string()],
            ..Default::default()
        };
        let policy = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::RejectWithResume,
            schemas: [("review.start".to_string(), schema)].into_iter().collect(),
            ..Default::default()
        };
        cfg.event_loop.event_policy = Some(policy);
        cfg
    }

    #[test]
    fn evaluate_candidate_emit_accepted_includes_projection() {
        // RED phase: build_projection_preview currently returns None unconditionally.
        // After U3 GREEN, accepted events must include a projection with state_changes.
        let config = projection_test_config();
        let hat_id = ralph_proto::HatId::new("reviewer");
        let payload = serde_json::json!({
            "plan_name": "myplan",
            "task_id": "task-1"
        });

        let result =
            evaluate_candidate_emit(&config, &hat_id, "review.start", &payload.to_string(), None)
                .expect("evaluation should succeed");

        assert_eq!(
            result.policy_decision, "accept",
            "review.start with plan_name and task_id must be accepted"
        );
        assert!(
            result.projection.is_some(),
            "accepted event MUST include projection with state_changes, got None"
        );
        let preview = result.projection.unwrap();
        assert!(
            !preview.state_changes.is_empty(),
            "accepted event projection state_changes must not be empty"
        );
    }

    #[test]
    fn evaluate_candidate_emit_rejected_has_no_projection() {
        // Rejected events must NOT include a projection.
        let config = projection_test_config();
        let hat_id = ralph_proto::HatId::new("reviewer");
        // Missing required plan_name and task_id.
        let payload = serde_json::json!({});

        let result =
            evaluate_candidate_emit(&config, &hat_id, "review.start", &payload.to_string(), None)
                .expect("evaluation should succeed");

        assert_eq!(
            result.policy_decision, "reject",
            "review.start without required fields must be rejected"
        );
        assert!(
            result.projection.is_none(),
            "rejected event must NOT include projection, got {:?}",
            result.projection
        );
    }
}
