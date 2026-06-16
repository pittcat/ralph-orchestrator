//! Event loop orchestration.
//!
//! The event loop coordinates the execution of hats via pub/sub messaging.

mod loop_state;
pub mod rejection;
pub mod review_step_state;
#[cfg(test)]
mod tests;

pub use loop_state::{LoopState, U2_REJECTION_RETRY_LIMIT, WorkflowProgress};
// Items are also re-exported from `crate::*` via `lib.rs`. The lib-side
// re-export keeps the public API stable; the `pub use` here is a
// convenience path for in-crate consumers (the runner).
#[allow(unused_imports)]
pub use rejection::{
    NonRetryableReason, Rejection, RejectionStage, build_task_resume_payload,
    rejection_from_origin, resolve_target_hat,
};

use crate::config::{HatBackend, HatExecutionMode, InjectMode, RalphConfig, ScratchpadConfig};

use crate::diagnosis::{
    RUNTIME_DIAGNOSIS_ALERT_HEADER, RecoveryDiagnosisEnvelope, RecoveryJournalEntry,
    RecoveryResponder,
};
use crate::diagnostics::OrchestrationEvent;
use crate::event_origin::filter_events_by_origin;
use crate::event_parser::{
    BuildStatus, EventParser, MutationEvidence, MutationStatus, ReviewStatus,
    parse_backpressure_json, parse_review_json,
};
use crate::event_policy::{
    DuplicateWorkDoneHint, PolicyDecision, PolicyFinding, PolicyRuntimeState, ReasonClass,
    ViolationType, check_completion_guard, check_completion_honored, check_topic_deny_rules,
    is_recoverable_policy_finding, validate_event,
};
use crate::event_reader::{Event as JsonlEvent, EventReader};
use crate::execution_contract::{
    DefaultGitEvidenceProvider, ExecutionContractDecision, ExecutionContractFinding,
    validate_execution_contract,
};
use crate::hat_lifecycle::{ActivationKey, ActivationLifecycleTracker, SystemTimeClock};
use crate::hat_registry::HatRegistry;
use crate::hatless_ralph::HatlessRalph;
use crate::instructions::InstructionBuilder;
use crate::loop_context::LoopContext;
use crate::memory_store::{MarkdownMemoryStore, format_memories_as_markdown, truncate_to_budget};
use crate::skill_registry::SkillRegistry;
use crate::state_machine::{StateMachineDecision, StateMachineRuntimeState};
use crate::text::floor_char_boundary;
use ralph_proto::{CheckinContext, Event, EventBus, Hat, HatId, RobotService};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Result of processing events from JSONL.
#[derive(Debug, Clone)]
pub struct ProcessedEvents {
    /// Whether any valid events were found and published.
    pub had_events: bool,
    /// Whether any events were present at the contract validation layer (passed or rejected).
    pub had_raw_events: bool,
    /// Whether any events were rejected by origin, policy, payload, or
    /// execution-contract validation.
    pub had_rejected_events: bool,
    /// Whether any published events matched the semantic `plan.*` topic family.
    pub had_plan_events: bool,
    /// Structured context for the first processed `human.interact` event,
    /// including the question payload and post-dispatch outcome metadata.
    pub human_interact_context: Option<Value>,
    /// Whether any events lacked specific hat subscribers (orphans handled by Ralph).
    pub has_orphans: bool,
    /// Events accepted by runtime validation and published to the bus.
    pub accepted_events: Vec<Event>,
    /// Findings from execution contract rejections (U5).
    pub contract_rejections: Vec<ExecutionContractFinding>,
    /// U6: payload contract violation detected at runtime (if any).
    /// When present, the loop must pause and emit a structured diagnostic.
    pub payload_contract_violation: Option<crate::payload_contract::PayloadContractViolation>,
}

impl Default for ProcessedEvents {
    fn default() -> Self {
        Self {
            had_events: false,
            had_raw_events: false,
            had_rejected_events: false,
            had_plan_events: false,
            human_interact_context: None,
            has_orphans: false,
            accepted_events: Vec::new(),
            contract_rejections: Vec::new(),
            payload_contract_violation: None,
        }
    }
}

/// Result of processing events from JSONL with wave events partitioned out.
#[derive(Debug)]
pub struct ProcessedEventsWithWaves {
    /// Normal event processing results.
    pub processed: ProcessedEvents,
    /// Wave events extracted before normal processing (have wave_id set).
    pub wave_events: Vec<crate::event_reader::Event>,
    /// U1 (2026-06-13-001): policy rejections collected from the wave
    /// partition. Every wave event that the event policy rejected
    /// (e.g. a `review.wave.ready` missing the required `depth` field)
    /// is exposed here so the runner can:
    /// 1. Skip the `missing_event_gate` (the agent DID try to emit).
    /// 2. Inject a schema-level guidance payload naming the missing
    ///    field instead of a generic "you forgot to emit" message.
    /// 3. Surface a recovery envelope so `ralph diagnose` can attribute
    ///    the failed fan-out to a policy contract failure, not a missing
    ///    emission.
    pub wave_policy_rejections: Vec<crate::event_policy::PolicyRejection>,
    /// U1 (2026-06-13-001): number of wave-partition events that entered
    /// policy validation. This is the "raw" wave count after the origin
    /// guard and topic-format check, immediately before the policy
    /// validator. It is captured so the recovery envelope's `evidence`
    /// can distinguish "all N rejected" from "N rejected out of M" —
    /// critical for the `wave_dispatch_blocked` R7 envelope shape.
    pub wave_raw_count: usize,
}

/// Reason the event loop terminated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    /// Completion promise was detected in output.
    CompletionPromise,
    /// Maximum iterations reached.
    MaxIterations,
    /// Maximum runtime exceeded.
    MaxRuntime,
    /// Maximum cost exceeded.
    MaxCost,
    /// Too many consecutive failures.
    ConsecutiveFailures,
    /// Loop thrashing detected (repeated blocked events).
    LoopThrashing,
    /// Stale loop detected (same topic emitted 3+ times consecutively).
    LoopStale,
    /// Too many consecutive malformed JSONL lines in events file.
    ValidationFailure,
    /// Manually stopped.
    Stopped,
    /// Interrupted by signal (SIGINT/SIGTERM).
    Interrupted,
    /// Restart requested via Telegram `/restart` command.
    RestartRequested,
    /// Workspace directory (worktree) was removed externally.
    WorkspaceGone,
    /// Loop was cancelled gracefully via loop.cancel event (human rejection, timeout).
    Cancelled,
    /// U6: runtime payload contract violation caused the loop to pause.
    PayloadContractViolation,
    /// U6: recovery responder's retry window exhausted for a tracked
    /// diagnosis key. The responder produced a `TerminationHint` of
    /// severity `Error` or `Critical` and the runner promoted the
    /// hint into a real termination. The carried `retry_key` and
    /// `reason` are the values the responder produced so the
    /// summary report can point operators to the diagnosis.
    RecoveryExhausted {
        /// The retry key the responder flagged as exhausted. Empty
        /// when the responder produced a key-less hint (e.g. a
        /// payload-contract-shaped Final escalation).
        retry_key: String,
        /// The free-form reason the responder attached to the
        /// hint. Surfaced in `loop.terminate` payload and in
        /// `summary.md`.
        reason: String,
    },
    /// P0-C (2026-06-10): fail-path auto-termination. Triggered when
    /// the verdict gate has observed a failing verdict (`fail_field ==
    /// fail_value`) AND the verdict has propagated to the LAST
    /// configured topic in the gate's mirror chain (i.e. either
    /// `gate.topic` alone, or the final entry in `gate.additional_topics`).
    ///
    /// This closes the "loop hangs after a failing review" gap: the
    /// verdict gate forbids `LOOP_COMPLETE` on fail (by design, to
    /// prevent a rogue completion from masking the failure), but until
    /// this fix there was no other exit signal — the loop would burn
    /// iterations forever. Now the runner exits with a clear reason
    /// once the fail verdict has reached the workflow's final
    /// downstream mirror event (e.g. `report.done` for ce-executor).
    ReviewFailed {
        /// The topic where the fail verdict was last observed.
        /// Surfaced in `loop.terminate` payload for operators.
        topic: String,
    },
    /// 2026-06-14-004 plan U2: isolated-scope circuit breaker.
    /// Triggered when the same (hat, topic) pair crosses the
    /// `U2_REJECTION_RETRY_LIMIT` threshold (4th attempt), meaning
    /// the hat keeps emitting an out-of-scope topic despite receiving
    /// `task.resume` guidance 3 times already.  This is a hard stop
    /// that prevents the loop from spiraling forever.
    ScopeViolationCircuitBreakerTripped {
        /// The hat that keeps emitting out-of-scope events.
        hat: String,
        /// The topic the hat is not allowed to publish.
        topic: String,
        /// The violation count (how many times this (hat, topic) was rejected).
        violation_count: u32,
        /// Topics the hat IS allowed to publish (from registry config).
        allowed_topics: Vec<String>,
    },
    /// Unit 2 (2026-06-16-002 plan) recoverable-payload budget
    /// exhausted.  Distinct from `PayloadContractViolation` (which
    /// terminates the loop on the **first** non-recoverable contract
    /// violation) and from `ScopeViolationCircuitBreakerTripped`
    /// (which only fires for the isolated-scope sub-path).  This
    /// variant fires when the recoverable set
    /// (`PayloadTypeMismatch` / `MissingRequiredField` /
    /// `TopicDenied`) has been retried past the bounded budget for
    /// the SAME `(hat, topic, reason_class)` triple (the 4th
    /// attempt).
    RecoverablePayloadExhausted {
        /// Hat that kept emitting the bad payload past the budget.
        hat: String,
        /// Topic the hat was emitting.
        topic: String,
        /// Reason class the budget was burned on
        /// (`payload_type_mismatch` / `missing_required_field` /
        /// `topic_denied`).
        reason_class: String,
        /// Post-increment count (always `> U2_REJECTION_RETRY_LIMIT`).
        count: u32,
    },
}

impl TerminationReason {
    /// Returns the exit code for this termination reason per spec.
    ///
    /// Per spec "Loop Termination" section:
    /// - 0: Completion promise detected (success)
    /// - 1: Consecutive failures or unrecoverable error (failure)
    /// - 2: Max iterations, max runtime, or max cost exceeded (limit)
    /// - 130: User interrupt (SIGINT = 128 + 2)
    pub fn exit_code(&self) -> i32 {
        match self {
            TerminationReason::CompletionPromise => 0,
            TerminationReason::ConsecutiveFailures
            | TerminationReason::LoopThrashing
            | TerminationReason::LoopStale
            | TerminationReason::ValidationFailure
            | TerminationReason::Stopped
            | TerminationReason::WorkspaceGone
            | TerminationReason::PayloadContractViolation
            | TerminationReason::RecoveryExhausted { .. }
            | TerminationReason::ReviewFailed { .. }
            | TerminationReason::ScopeViolationCircuitBreakerTripped { .. } => 1,
            | TerminationReason::RecoverablePayloadExhausted { .. } => 1,
            TerminationReason::MaxIterations
            | TerminationReason::MaxRuntime
            | TerminationReason::MaxCost => 2,
            TerminationReason::Interrupted => 130,
            // Restart uses exit code 3 to signal the caller to exec-replace
            TerminationReason::RestartRequested => 3,
            // Cancelled is a clean exit (0) — the loop stopped intentionally
            TerminationReason::Cancelled => 0,
        }
    }

    /// Returns the reason string for use in loop.terminate event payload.
    ///
    /// Per spec event payload format:
    /// `completed | max_iterations | max_runtime | consecutive_failures | interrupted | error`
    pub fn as_str(&self) -> &'static str {
        match self {
            TerminationReason::CompletionPromise => "completed",
            TerminationReason::MaxIterations => "max_iterations",
            TerminationReason::MaxRuntime => "max_runtime",
            TerminationReason::MaxCost => "max_cost",
            TerminationReason::ConsecutiveFailures => "consecutive_failures",
            TerminationReason::LoopThrashing => "loop_thrashing",
            TerminationReason::LoopStale => "loop_stale",
            TerminationReason::ValidationFailure => "validation_failure",
            TerminationReason::Stopped => "stopped",
            TerminationReason::Interrupted => "interrupted",
            TerminationReason::RestartRequested => "restart_requested",
            TerminationReason::WorkspaceGone => "workspace_gone",
            TerminationReason::Cancelled => "cancelled",
            TerminationReason::PayloadContractViolation => "payload_contract_violation",
            TerminationReason::RecoveryExhausted { .. } => "recovery_exhausted",
            TerminationReason::ReviewFailed { .. } => "review_failed",
            TerminationReason::ScopeViolationCircuitBreakerTripped { .. } => {
                "scope_violation_circuit_breaker_tripped"
            }
            TerminationReason::RecoverablePayloadExhausted { .. } => {
                "recoverable_payload_exhausted"
            }
        }
    }

    /// Returns true if this is a successful completion (not an error or limit).
    pub fn is_success(&self) -> bool {
        matches!(self, TerminationReason::CompletionPromise)
    }
}

/// Result of workflow guard completion validation.
#[derive(Debug)]
struct WorkflowGuardRejection {
    /// Human-readable message describing the incomplete instance.
    message: String,
}

/// The main event loop orchestrator.
pub struct EventLoop {
    config: RalphConfig,
    registry: HatRegistry,
    bus: EventBus,
    state: LoopState,
    instruction_builder: InstructionBuilder,
    ralph: HatlessRalph,
    /// Cached human guidance messages that should persist across iterations.
    robot_guidance: Vec<String>,
    /// Event reader for consuming events from JSONL file.
    /// Made pub(crate) to allow tests to override the path.
    pub(crate) event_reader: EventReader,
    diagnostics: crate::diagnostics::DiagnosticsCollector,
    /// Loop context for path resolution (None for legacy single-loop mode).
    loop_context: Option<LoopContext>,
    /// Skill registry for the current loop.
    skill_registry: SkillRegistry,
    /// Robot service for human-in-the-loop communication.
    /// Injected externally when `human.enabled` is true and this is the primary loop.
    robot_service: Option<Box<dyn RobotService>>,
    /// WAC-U3 / WAC-U5 (2026-06-12-002): handoff priority index,
    /// built once at construction. The dispatcher's priority pass
    /// consults `index.consumer_of(topic)` on every selection
    /// tick. `None` when the config is in coordinator mode or
    /// the index is empty (no priority-eligible handoffs).
    handoff_index: crate::workflow_contract::HandoffIndex,
    /// U6: Recovery responder — aggregates per-`retry_key` state and
    /// decides whether the next prompt should fold a soft alert, the
    /// runner should publish a targeted `task.resume`, or the loop
    /// should surface a `TerminationHint`. The responder is
    /// in-memory only; it never touches the diagnostics loggers
    /// directly.
    recovery_responder: RecoveryResponder,
    /// U3: Activation lifecycle tracker — tracks each hat activation from
    /// activate → observe_accepted_event → complete. Write APIs are called
    /// by the event loop; read API (`active_activations`) is consumed only
    /// by the `ralph diagnose` reporter (U4). Decision paths must NOT read
    /// the tracker to avoid implicit feedback loops.
    hat_lifecycle_tracker: ActivationLifecycleTracker<SystemTimeClock>,

    /// R3 (2026-06-14-003 plan): ephemeral file isolation engine.
    /// Used by `process_output` to relocate agent-written runtime
    /// artefacts (scratchpad.md / tmp*.md) out of source trees into
    /// `.ralph/agent/scratchpad-{loop_id}.md`.  The engine is
    /// opt-in: callers must enable `EventLoopConfig.ephemeral_isolation`
    /// for it to fire.  The field is owned by `EventLoop` so the
    /// per-iteration cache (mtime/size sentinel) survives across calls.
    ephemeral_isolation: crate::ephemeral_isolation::EphemeralIsolation,
}

/// Publish a `task.resume` event in response to a policy rejection.
/// R5 (2026-06-14-003 plan): the resume event's `target` is the
/// source hat (so the next activation lands on the offending hat,
/// not the alphabetically-first hat) and the payload carries
/// `wave_id` / `wave_index` / `wave_total` when the source event
/// was a wave record.  Falls back to an un-targeted publish when
/// the source hat is unknown (preserves the pre-R5 behaviour of
/// letting `Ralph` recover).
///
/// U3 (2026-06-17-003 plan): when the source event is `work.done`
/// and `tracker` reports an open wave, append a `## WAVE_OPEN
/// HINT` block to the payload instructing the agent to NOT emit
/// `review.passed(empty_diff)` while a wave is in progress and
/// NOT to repeat `work.done` (both are hard semantic violations
/// — the `review_passed_while_wave_open` gate is recoverable but
/// `work.done` dedup is the U4 sibling). This is the textual
/// counterpart to the mechanism: the gate rejects, the hint
/// explains why.
fn publish_policy_rejection_resume(
    bus: &mut EventBus,
    event: &JsonlEvent,
    payload: String,
    tracker: Option<&crate::event_loop::review_step_state::ReviewStepTracker>,
) {
    let payload = enrich_payload_with_wave(&payload, event);
    let payload = enrich_payload_with_wave_open_hint(&payload, event, tracker);
    let mut resume = Event::new("task.resume", payload);
    if let Some(hat) = event.hat.as_deref() {
        if !hat.is_empty() {
            resume = resume.with_target(HatId::new(hat.to_string()));
        }
    }
    bus.publish(resume);
}

/// U3 (2026-06-17-003 plan) — when the source event is `work.done`
/// and the tracker reports any open wave, append a structured
/// `## WAVE_OPEN HINT` block to the resume payload. The block
/// tells the agent:
///   - a wave is currently open (`open_wave_id=<id>`,
///     `received=<n>/<total>`),
///   - do NOT emit `review.passed(empty_diff)` while the wave
///     is open (semantic gate `review_passed_while_wave_open`
///     will reject and recover — see U1),
///   - do NOT re-emit `work.done` (duplicate dedup is U4).
///   - the mechanism will emit `plan.blocked` via U2 staleness
///     if the wave does not close; agents should let it close
///     instead of attempting empty_diff fast-path.
fn enrich_payload_with_wave_open_hint(
    payload: &str,
    event: &JsonlEvent,
    tracker: Option<&crate::event_loop::review_step_state::ReviewStepTracker>,
) -> String {
    if event.topic != "work.done" {
        return payload.to_string();
    }
    let Some(tracker) = tracker else {
        return payload.to_string();
    };
    let Some(snapshot) = tracker.first_open_wave_snapshot() else {
        return payload.to_string();
    };
    format!(
        "{payload}\n\n\
         ## WAVE_OPEN HINT (U3)\n\
         - reason: work.done rejected while a review wave is open; do not bypass with empty_diff\n\
         - open_wave_id: {wave_id}\n\
         - received: {received}/{expected}\n\
         - prohibition: do NOT emit `review.passed(empty_diff)` while a wave is open — semantic gate rejects with `review_passed_while_wave_open` (recoverable, not fatal); do NOT re-emit `work.done` either (U4 dedup blocks it).\n\
         - fallback: let the wave close naturally, or wait for mechanism `plan.blocked` via U2 staleness if the wave stalls past the aggregate window.\n"
        ,
        wave_id = snapshot.wave_id,
        received = snapshot.received,
        expected = snapshot.expected,
    )
}

/// Append a `<!-- wave_id=... wave_index=... wave_total=... -->`
/// HTML-comment block to the payload when the source event carries
/// wave metadata.  The block is intentionally machine-greppable
/// (so downstream tooling can recover the wave without parsing
/// the entire payload) and harmless to human readers.
fn enrich_payload_with_wave(payload: &str, event: &JsonlEvent) -> String {
    let Some(wave_id) = event.wave_id.as_deref() else {
        return payload.to_string();
    };
    let block = match (event.wave_index, event.wave_total) {
        (Some(i), Some(t)) => {
            format!("\n\n<!-- wave_id={wave_id} wave_index={i} wave_total={t} -->")
        }
        _ => format!("\n\n<!-- wave_id={wave_id} -->"),
    };
    format!("{payload}{block}")
}

/// Result of extracting a correlation key from an event payload.
enum CorrelationKeyResult {
    /// Chain has no correlation config — use global instance tracking.
    Global,
    /// Successfully extracted instance key from payload.
    Instance(String),
    /// Correlation config exists but extraction failed (missing payload, invalid JSON,
    /// path not found, or value is not a string). Event should be rejected.
    ExtractFailed,
}

/// Extracts the correlation key from an event's payload based on chain config.
///
/// Returns [`CorrelationKeyResult::Global`] for chains without correlation config,
/// [`CorrelationKeyResult::Instance`] when extraction succeeds, and
/// [`CorrelationKeyResult::ExtractFailed`] when the chain has correlation config
/// but the payload is missing, malformed, or does not contain the configured path.
fn extract_correlation_key(
    event: &JsonlEvent,
    chain: &crate::config::WorkflowChain,
) -> CorrelationKeyResult {
    let Some(correlation) = chain.correlation.as_ref() else {
        return CorrelationKeyResult::Global;
    };
    let Some(payload) = event.payload.as_ref() else {
        return CorrelationKeyResult::ExtractFailed;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return CorrelationKeyResult::ExtractFailed;
    };

    // Navigate the JSON path (dot notation)
    let parts: Vec<&str> = correlation.from_payload.split('.').collect();
    let mut current = &value;
    for part in parts {
        let Some(next) = current.get(part) else {
            return CorrelationKeyResult::ExtractFailed;
        };
        current = next;
    }

    match current.as_str() {
        Some(s) => CorrelationKeyResult::Instance(s.to_string()),
        None => CorrelationKeyResult::ExtractFailed,
    }
}

/// Validates events against configured workflow guards.
///
/// Events that are out-of-order relative to a configured chain are rejected
/// and replaced with a recovery signal (task.resume). The event is NOT recorded
/// as seen and is NOT published to the bus.
///
/// Side-channel events (e.g., `periodic.review`) that are not part of any chain
/// are accepted but do not advance the workflow progress.
/// One rejected workflow-guard event. Returned from
/// [`apply_workflow_guard_validation`] so the caller (in
/// `process_events_from_jsonl`) can record a U4 recovery envelope
/// without re-running the validation logic.
///
/// The function itself is still pure with respect to the diagnostics
/// collector; it does not call `log_recovery`. The caller maps each
/// rejection to a `RecoveryDiagnosisEnvelope` and writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowGuardRejectionDetail {
    /// The chain that rejected the event (e.g. `experiment`).
    pub chain_name: String,
    /// The instance key (e.g. `exp-1`) when the chain is correlation-scoped.
    pub instance_key: Option<String>,
    /// The topic that was rejected.
    pub rejected_topic: String,
    /// The current phase the chain is at (0-based). `None` when the
    /// chain is at the start or correlation extraction failed.
    pub current_phase: Option<usize>,
    /// Human-readable summary of the current phase topic.
    pub current_topic: String,
    /// The next expected topic, or `terminal` when the chain is
    /// already at the end.
    pub next_expected: String,
    /// Source hat the event was attributed to (via `event.hat`).
    pub source_hat: Option<String>,
    /// The full rejection reason (concatenation across chains).
    pub reason: String,
}

/// Output of [`apply_workflow_guard_validation`]. The accepted events
/// keep flowing downstream exactly as before; the rejections list
/// carries the metadata the caller needs to emit a U4 recovery
/// envelope per rejection. U4 plan: "要么返回 rejection diagnostics，
/// 要么注入一个轻量 sink/callback，由调用方统一写 log_recovery()".
/// The lighter-touch approach is a return value; that is what this
/// struct implements.
#[derive(Debug, Default)]
pub struct WorkflowGuardOutcome {
    /// Events that passed workflow-guard validation.
    pub accepted_events: Vec<JsonlEvent>,
    /// Events that were rejected, in the order they were seen.
    pub rejections: Vec<WorkflowGuardRejectionDetail>,
}

fn apply_workflow_guard_validation(
    events: Vec<JsonlEvent>,
    guards: &crate::config::WorkflowGuardsConfig,
    workflow_progress: &mut WorkflowProgress,
    bus: &mut EventBus,
    review_step_tracker: &review_step_state::ReviewStepTracker,
) -> WorkflowGuardOutcome {
    let mut outcome = WorkflowGuardOutcome {
        accepted_events: Vec::with_capacity(events.len()),
        rejections: Vec::new(),
    };

    for event in events {
        // Find which chain(s) this topic belongs to
        let matching_chains: Vec<&crate::config::WorkflowChain> = guards
            .chains
            .iter()
            .filter(|chain| chain.topics.contains(&event.topic))
            .collect();

        if matching_chains.is_empty() {
            // Topic not in any chain — accept as side-channel (no progress tracking)
            outcome.accepted_events.push(event);
            continue;
        }

        // Extract instance keys and phases once per chain, then validate and advance
        let mut rejections: Vec<(String, Option<String>, Option<usize>, String, String)> =
            Vec::new();
        let mut chain_extractions: Vec<(&crate::config::WorkflowChain, Option<String>, usize)> =
            Vec::new();

        for chain in &matching_chains {
            let instance_key = match extract_correlation_key(&event, chain) {
                CorrelationKeyResult::Global => None,
                CorrelationKeyResult::Instance(key) => Some(key),
                CorrelationKeyResult::ExtractFailed => {
                    rejections.push((
                        chain.name.clone(),
                        None,
                        None,
                        "none".to_string(),
                        "unknown (correlation extraction failed)".to_string(),
                    ));
                    continue;
                }
            };

            // Find the phase index of this topic in the chain
            let phase = chain.topics.iter().position(|t| *t == event.topic).unwrap();

            // Strict mode rejects out-of-order events; Advisory mode accepts all
            if matches!(chain.mode, crate::config::WorkflowChainMode::Strict) {
                let is_valid =
                    workflow_progress.is_phase_valid(&chain.name, instance_key.as_deref(), phase);

                if !is_valid {
                    let current_phase =
                        workflow_progress.get_phase(&chain.name, instance_key.as_deref());
                    let current_topic = current_phase
                        .and_then(|p| chain.topics.get(p).cloned())
                        .unwrap_or_else(|| "none".to_string());
                    let next_expected = current_phase
                        .and_then(|p| chain.topics.get(p + 1).cloned())
                        .unwrap_or_else(|| "terminal".to_string());
                    rejections.push((
                        chain.name.clone(),
                        instance_key.clone(),
                        current_phase,
                        current_topic,
                        next_expected,
                    ));
                }
            }

            chain_extractions.push((chain, instance_key, phase));
        }

        if !rejections.is_empty() {
            let rejection_details: Vec<String> = rejections
                .iter()
                .map(|(chain_name, instance_key, current_phase, current_topic, next_expected)| {
                    format!(
                        "chain '{}' (instance '{}'): current='{}' (phase {}), next expected='{}'",
                        chain_name,
                        instance_key.as_deref().unwrap_or("global"),
                        current_topic,
                        current_phase.map(|p| p.to_string()).unwrap_or_else(|| "none".to_string()),
                        next_expected
                    )
                })
                .collect();

            let rejection_reason = format!(
                "Workflow guard rejected '{}': {}.",
                event.topic,
                rejection_details.join("; ")
            );

            warn!(
                reason = %rejection_reason,
                topic = %event.topic,
                "Out-of-order workflow event rejected by guard"
            );

            // Publish recovery signal with actionable context
            let recovery_payload = format!(
                "WORKFLOW_GUARD_REJECTED: out-of-order event '{}'.\n{}\n\n\
                 Wait for the correct phase before emitting this event. \
                 The loop will continue to allow recovery.",
                event.topic,
                rejection_details.join("\n")
            );
            publish_policy_rejection_resume(bus, &event, recovery_payload, Some(review_step_tracker));

            // U4: surface the rejection metadata to the caller. The
            // helper itself stays free of diagnostics dependencies;
            // the caller writes the recovery journal + audit event.
            // (One rejection entry per rejected event, regardless of
            // how many chains rejected it — the loop summary in
            // `reason` already concatenates chain details.)
            let source_hat = event.hat.clone();
            let mut envelope_rejection = None;
            for (chain_name, instance_key, current_phase, current_topic, next_expected) in
                rejections.into_iter()
            {
                if envelope_rejection.is_none() {
                    envelope_rejection = Some(WorkflowGuardRejectionDetail {
                        chain_name,
                        instance_key,
                        rejected_topic: event.topic.clone(),
                        current_phase,
                        current_topic,
                        next_expected,
                        source_hat: source_hat.clone(),
                        reason: rejection_reason.clone(),
                    });
                }
            }
            if let Some(rejection) = envelope_rejection {
                outcome.rejections.push(rejection);
            }

            // Do NOT record the rejected event or advance progress
            continue;
        }

        // Event is valid — accept it
        outcome.accepted_events.push(event);

        // Advance workflow progress for all matching chains (both strict and advisory).
        // Advisory chains track progress for in-order events but never reject.
        for (chain, instance_key, phase) in chain_extractions {
            workflow_progress.advance(&chain.name, instance_key.as_deref(), phase);
        }
    }

    outcome
}

/// Validates events against configured event policy.
///
/// Events that violate the policy are handled according to the configured
/// `on_violation` action. In `observe` mode, violations are logged as diagnostics
/// but events still pass through. In `enforce` mode, violations may reject or
/// hold events.
/// Result of event policy validation including events and hold status.
#[derive(Debug)]
struct PolicyValidationResult {
    events: Vec<JsonlEvent>,
    hold_triggered: bool,
    hold_reason: Option<String>,
    /// U6: payload contract violation captured during policy validation
    /// (if any). When set, the loop should pause and emit a diagnostic.
    /// Unit 2 (2026-06-16-002 plan) R-B1/R-B2: this field is ONLY
    /// populated for **non-recoverable** violations so the U6
    /// `NotRetriable` fast-fail does not trigger on the recoverable
    /// set.
    payload_contract_violation: Option<crate::payload_contract::PayloadContractViolation>,
    /// Origin/policy/payload rejections collected during validation.
    /// Used by the CLI runner to produce unified recovery diagnostics.
    /// Each entry carries an optional `reason_class` — when `Some`,
    /// the rejection is in the recoverable bucket.
    policy_rejections: Vec<crate::event_policy::PolicyRejection>,
    /// Unit 2 (2026-06-16-002 plan) recoverable-bucket budget
    /// exhaustions. Each entry represents a (hat, topic,
    /// DEPRECATED by `recoverable_candidates` (Unit 2 take-3):
    /// the function no longer takes `&mut LoopState`, so the
    /// actual counter bookkeeping happens in the caller.  The
    /// field is kept for back-compat with diagnostic snapshots
    /// but is no longer populated — the caller merges candidates
    /// into `state.recoverable_exhaustion_buffer` instead.
    /// The `#[allow(dead_code)]` is required because the field
    /// is now always written as `Vec::new()` from the validator
    /// and never read; the validator still constructs it for
    /// shape compatibility.
    #[allow(dead_code)]
    recoverable_exhausted: Vec<RecoverableExhaustion>,
    /// Unit 2 (2026-06-16-002 plan) take-3: candidates the
    /// caller still needs to record against the recoverable
    /// budget.  Each entry is a `(hat, topic, reason_class)`
    /// triple produced by a recoverable rejection in this
    /// pass.  The caller is responsible for calling
    /// `state.record_recoverable_rejection_key(...)` for each
    /// entry and pushing the exhausted ones into
    /// `state.recoverable_exhaustion_buffer`.
    recoverable_candidates: Vec<RecoverableExhaustionCandidate>,
}

/// Unit 2 (2026-06-16-002 plan) recoverable-bucket budget exhaustion.
/// The runner turns each of these into a
/// `RecoverablePayloadExhausted` termination reason (or, when the
/// iteration can still proceed, a `DiagnosisOutcome::Failed`
/// recovery envelope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableExhaustion {
    /// Hat that emitted the (hat, topic) pair whose budget just
    /// crossed the limit.
    pub hat: String,
    /// Topic the hat kept emitting despite the `task.resume` guidance.
    pub topic: String,
    /// Reason class the budget was burned on.
    pub reason_class: crate::event_policy::ReasonClass,
    /// Post-increment count (always `> U2_REJECTION_RETRY_LIMIT`).
    pub count: u32,
}

/// Unit 2 (2026-06-16-002 plan) take-3: a single recoverable
/// rejection surfaced from the policy validator.  The validator
/// does **not** call `state.record_recoverable_rejection_key`
/// itself (it does not own `&mut LoopState`); it just records
/// the candidate `(hat, topic, reason_class)` triple.  The
/// caller is responsible for the counter bookkeeping and the
/// promotion into a `RecoverableExhaustion` if the budget
/// crosses the limit.  This split keeps the validator
/// borrow-checkable under NLL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableExhaustionCandidate {
    /// Hat that emitted the rejection.
    pub hat: String,
    /// Topic the hat was emitting.
    pub topic: String,
    /// Reason class the rejection belongs to.
    pub reason_class: crate::event_policy::ReasonClass,
}

/// Unit 3 (2026-06-16-002 plan): strip `### HUMAN GUIDANCE` blocks
/// from a scratchpad snapshot.  Mirrors the state machine used by
/// `persist_guidance_to_scratchpad` to detect guidance blocks
/// (a `### HUMAN GUIDANCE` header followed by body lines, ending
/// at the next `### ` / `## ` section header or EOF) so a line in
/// `## NOTES` that happens to mention guidance is NOT stripped.
///
/// The filtered output preserves the surrounding section structure
/// — a guidance block is replaced with a single blank line so the
/// surrounding content keeps its line numbers in tools that index
/// by line.
fn filter_human_guidance_blocks(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_guidance = false;
    for line in content.lines() {
        if line.starts_with("### HUMAN GUIDANCE") {
            // Drop the entire guidance block (header + body).
            // Replace with a single blank line so subsequent
            // sections keep their line numbering stable.
            in_guidance = true;
            out.push('\n');
            continue;
        }
        if in_guidance && (line.starts_with("### ") || line.starts_with("## ")) {
            // New section starts after a guidance block — exit
            // the guidance state and emit the new section header
            // normally.
            in_guidance = false;
        }
        if !in_guidance {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Unit 2 (2026-06-16-002 plan R-B3): append a schema-aware fix hint to a
/// recoverable policy rejection payload when the source hat is authorised
/// to publish the topic and a schema is configured. Returns `payload`
/// unchanged if no hint can be generated.
fn append_fix_hint_if_recoverable(
    payload: &str,
    hat_id: Option<&str>,
    topic: &str,
    policy_config: &crate::config::EventPolicyConfig,
    registry: &crate::hat_registry::HatRegistry,
) -> String {
    let Some(hat_id) = hat_id else {
        return payload.to_string();
    };
    let Some(schema) = policy_config.schemas.get(topic) else {
        return payload.to_string();
    };
    let hat = match registry.get(&ralph_proto::HatId::new(hat_id)) {
        Some(h) => h,
        None => return payload.to_string(),
    };
    match crate::emit_schema_hint::fix_hint_for_hat_topic(hat, topic, schema) {
        Some(hint) => format!("{}\n\n{}", payload, hint),
        None => payload.to_string(),
    }
}

/// When a hat hits a topic-deny rule, list its declared `publishes`
/// topics so the agent can recover without guessing.
fn append_hat_publishes_hint(
    payload: &str,
    hat_id: Option<&str>,
    registry: &crate::hat_registry::HatRegistry,
) -> String {
    let Some(hat_id) = hat_id else {
        return payload.to_string();
    };
    let Some(hat) = registry.get(&ralph_proto::HatId::new(hat_id)) else {
        return payload.to_string();
    };
    if hat.publishes.is_empty() {
        return payload.to_string();
    }
    let topics: Vec<&str> = hat.publishes.iter().map(|t| t.as_str()).collect();
    format!(
        "{payload}\n\nAllowed publish topics for hat '{hat_id}': {}",
        topics.join(", ")
    )
}

fn apply_event_policy_validation(
    events: Vec<JsonlEvent>,
    policy_config: &crate::config::EventPolicyConfig,
    policy_state: &mut PolicyRuntimeState,
    review_step_tracker: &mut review_step_state::ReviewStepTracker,
    bus: &mut EventBus,
    write_diagnostic: bool,
    source_hats_by_topic: &std::collections::HashMap<String, Vec<String>>,
    target_hats_by_topic: &std::collections::HashMap<String, Vec<String>>,
    registry: &crate::hat_registry::HatRegistry,
) -> PolicyValidationResult {
    // Unit 2 (2026-06-16-002 plan) take-3: the validator does NOT
    // take `&mut LoopState` directly.  It returns a list of
    // `RecoverableExhaustionCandidate` and the **caller** is
    // responsible for calling `state.record_recoverable_rejection_key`
    // for each entry.  This split avoids the borrow-checker conflict
    // between `&mut LoopState` and `&mut ReviewStepTracker` (the
    // latter is a field of the former) at the call site.
    let mut payload_contract_violation: Option<crate::payload_contract::PayloadContractViolation> =
        None;
    // Unit 2 (2026-06-16-002 plan): `capture_violation` now ONLY fires
    // for **non-recoverable** findings.  The recoverable set
    // (`PayloadTypeMismatch`, `MissingRequiredField`, `TopicDenied`)
    // bypasses this closure entirely so the U6 fast-fail is not
    // triggered on a recoverable first attempt.
    //
    // U1 (2026-06-17-003 plan): `SemanticGateViolation` (e.g.
    // `review_passed_while_wave_open`) is in the same recoverable
    // set via `is_recoverable_policy_finding` returning
    // `Some(ReasonClass::SemanticGateViolation)`.  The bucket is
    // **independent** — semantic-gate rejections never compete
    // with `PayloadTypeMismatch` / `MissingRequiredField` /
    // `TopicDenied` for the same `(hat, topic)` retry budget.  The
    // dedicated `task.resume` payload (see the call sites below)
    // tells the source hat to wait for the mechanism-emitted
    // `plan.blocked` or to actually complete the missing dimensions
    // before retrying `review.passed`.
    let mut capture_violation = |finding: &PolicyFinding, payload: Option<&str>| {
        if payload_contract_violation.is_some() {
            return; // capture only the first
        }
        if is_recoverable_policy_finding(finding).is_some() {
            return; // recoverable: never feed into U6 fast-fail
        }
        let source_hats = source_hats_by_topic
            .get(&finding.topic)
            .cloned()
            .unwrap_or_default();
        let target_hats = target_hats_by_topic
            .get(&finding.topic)
            .cloned()
            .unwrap_or_default();
        let schema_defined_in = match policy_config.schemas.get(&finding.topic) {
            Some(_) => match &policy_config.schema_file {
                Some(f) => format!("inline + file:{}", f),
                None => "inline".to_string(),
            },
            None => "(none)".to_string(),
        };
        payload_contract_violation = finding_to_payload_contract_violation(
            finding,
            payload,
            &source_hats,
            &target_hats,
            &schema_defined_in,
        );
    };

    let mut validated_events = Vec::with_capacity(events.len());
    let mut hold_triggered = false;
    let mut hold_reason = None;
    let mut policy_rejections: Vec<crate::event_policy::PolicyRejection> = Vec::new();
    // Unit 2 (2026-06-16-002 plan): this function no longer takes
    // `&mut LoopState`.  It only collects **candidates** for
    // recoverable budget exhaustion.  The caller is responsible
    // for calling `state.record_recoverable_rejection_key(...)`
    // for each candidate and pushing the exhausted entries into
    // `state.recoverable_exhaustion_buffer`.  We emit the
    // `(hat, topic, reason_class)` triple plus a
    // `payload_excerpt` (for diagnostics) for the caller to
    // consume.
    let mut recoverable_candidates: Vec<RecoverableExhaustionCandidate> = Vec::new();

    for event in events {
        // Completion-honored guard takes precedence: after a completion promise
        // has been accepted, subsequent terminal/business events are filtered
        // according to completion_after_terminal config.
        if let Some(decision) = check_completion_honored(&event.topic, policy_config, policy_state)
        {
            match decision {
                PolicyDecision::Accept => {
                    validated_events.push(event);
                }
                PolicyDecision::Warn(findings) => {
                    for finding in findings {
                        let diagnostic = Event::new(
                            "event.policy_warning",
                            format!(
                                "Completion-guard warning for '{}': {}",
                                event.topic, finding.message
                            ),
                        );
                        bus.publish(diagnostic);
                    }
                    validated_events.push(event);
                }
                PolicyDecision::RejectWithResume(finding) => {
                    // Collect for unified rejection handler (Task #21)
                    policy_rejections.push(crate::event_policy::PolicyRejection {
                        topic: event.topic.clone(),
                        source_hat: event.hat.clone(),
                        finding: finding.clone(),
                        reason_class: None,
                    });
                    let recovery_payload = format!(
                        "EVENT_POLICY_REJECTED: event '{}' violates completion guard.\n{}\n\n\
                         Wait for the correct event schema before emitting this event. \
                         The loop will continue to allow recovery.",
                        event.topic, finding.message
                    );
                    publish_policy_rejection_resume(bus, &event, recovery_payload, Some(review_step_tracker));
                }
                PolicyDecision::Hold(finding) => {
                    hold_triggered = true;
                    hold_reason = Some(finding.message.clone());
                    // Collect for unified rejection handler (Task #21)
                    policy_rejections.push(crate::event_policy::PolicyRejection {
                        topic: event.topic.clone(),
                        source_hat: event.hat.clone(),
                        finding: finding.clone(),
                        reason_class: None,
                    });
                    let recovery_payload = format!(
                        "EVENT_POLICY_HOLD: event '{}' violates completion guard.\n{}\n\n\
                         Loop held due to completion guard violation. Use resume to continue.",
                        event.topic, finding.message
                    );
                    publish_policy_rejection_resume(bus, &event, recovery_payload, Some(review_step_tracker));
                }
                PolicyDecision::Block(finding) => {
                    if write_diagnostic {
                        let diagnostic = Event::new(
                            "event.completion.blocked",
                            format!(
                                "Completion guard blocked '{}': {}",
                                event.topic, finding.message
                            ),
                        );
                        bus.publish(diagnostic);
                    }
                }
                PolicyDecision::Ignore(finding) => {
                    if write_diagnostic {
                        let diagnostic = Event::new(
                            "event.completion.ignored",
                            format!(
                                "Completion guard ignored '{}': {}",
                                event.topic, finding.message
                            ),
                        );
                        bus.publish(diagnostic);
                    }
                }
            }
            continue;
        }

        // U3: Topic-deny rules — check BEFORE payload schema validation.
        // When a (hat_id, topic) pair matches a deny rule, the event is
        // rejected according to the policy mode (Block, Warn, etc.).
        if let Some(decision) =
            check_topic_deny_rules(event.hat.as_deref(), &event.topic, policy_config)
        {
            match decision {
                PolicyDecision::Accept => {
                    validated_events.push(event);
                }
                PolicyDecision::Warn(findings) => {
                    for finding in findings {
                        let diagnostic = Event::new(
                            "event.policy_warning",
                            format!(
                                "Topic-deny warning for '{}': {}",
                                event.topic, finding.message
                            ),
                        );
                        bus.publish(diagnostic);
                    }
                    validated_events.push(event);
                }
                PolicyDecision::RejectWithResume(finding) => {
                    let reason_class = is_recoverable_policy_finding(&finding);
                    policy_rejections.push(crate::event_policy::PolicyRejection {
                        topic: event.topic.clone(),
                        source_hat: event.hat.clone(),
                        finding: finding.clone(),
                        reason_class,
                    });
                    if let Some(rc) = reason_class {
                        let hat_for_counter =
                            event.hat.as_deref().unwrap_or("unknown");
                        recoverable_candidates.push(RecoverableExhaustionCandidate {
                            hat: hat_for_counter.to_string(),
                            topic: event.topic.clone(),
                            reason_class: rc,
                        });
                    }
                    let recovery_payload = format!(
                        "EVENT_POLICY_REJECTED: event '{}' matches topic-deny rule.\n{}\n\n\
                         This hat is not allowed to publish this topic. \
                         Emit one of the hat's declared `publishes` topics instead.",
                        event.topic, finding.message
                    );
                    let recovery_payload = append_hat_publishes_hint(
                        &recovery_payload,
                        event.hat.as_deref(),
                        registry,
                    );
                    publish_policy_rejection_resume(bus, &event, recovery_payload, Some(review_step_tracker));
                }
                PolicyDecision::Hold(finding) => {
                    hold_triggered = true;
                    hold_reason = Some(finding.message.clone());
                    policy_rejections.push(crate::event_policy::PolicyRejection {
                        topic: event.topic.clone(),
                        source_hat: event.hat.clone(),
                        finding: finding.clone(),
                        reason_class: None,
                    });
                    let recovery_payload = format!(
                        "EVENT_POLICY_HOLD: event '{}' matches topic-deny rule.\n{}\n\n\
                         Loop held due to topic-deny rule. Use resume to continue.",
                        event.topic, finding.message
                    );
                    publish_policy_rejection_resume(bus, &event, recovery_payload, Some(review_step_tracker));
                }
                PolicyDecision::Block(_finding) => {
                    // Silently drop the event
                }
                PolicyDecision::Ignore(_finding) => {
                    // Silently ignore the event
                }
            }
            continue;
        }

        let decision = validate_event(
            &event.topic,
            event.payload.as_deref(),
            policy_config,
            policy_state,
        );

        match decision {
            PolicyDecision::Accept => {
                if let Some(finding) = review_step_tracker.check_semantic_gates(&event) {
                    // U1 (2026-06-17-003 plan): `capture_violation`
                    // no-ops for `SemanticGateViolation` because the
                    // variant is in the recoverable set. We still
                    // emit a `task.resume` so the source hat sees
                    // the failure reason; the hint explicitly tells
                    // review-coordinator NOT to retry with
                    // `skip_reason=empty_diff` and to either
                    // complete the missing dimensions or wait for
                    // the mechanism to emit `plan.blocked` (U2).
                    capture_violation(&finding, event.payload.as_deref());
                    policy_rejections.push(crate::event_policy::PolicyRejection {
                        topic: event.topic.clone(),
                        source_hat: event.hat.clone(),
                        finding: finding.clone(),
                        reason_class: is_recoverable_policy_finding(&finding),
                    });
                    let recovery_payload = format!(
                        "EVENT_POLICY_REJECTED: event '{}' violates semantic gate (review step gate).\n{}\n\n\
                         Wave 未闭合，禁止 empty_diff；等待机制 plan.blocked 或补全维度后重发 review.passed。\
                         Wait for review-synthesizer terminal before plan-gate events.",
                        event.topic, finding.message
                    );
                    publish_policy_rejection_resume(bus, &event, recovery_payload, Some(review_step_tracker));
                } else {
                    review_step_tracker.observe_accepted(&event);
                    validated_events.push(event);
                }
            }
            PolicyDecision::Warn(findings) => {
                // In observe mode: log diagnostics but still pass the event through
                for finding in findings {
                    let diagnostic = Event::new(
                        "event.policy_warning",
                        format!("Policy warning for '{}': {}", event.topic, finding.message),
                    );
                    bus.publish(diagnostic);
                }
                if let Some(finding) = review_step_tracker.check_semantic_gates(&event) {
                    // U1 (2026-06-17-003 plan): same handling as
                    // the `Accept` arm — `SemanticGateViolation` is
                    // recoverable and the loop continues.
                    capture_violation(&finding, event.payload.as_deref());
                    policy_rejections.push(crate::event_policy::PolicyRejection {
                        topic: event.topic.clone(),
                        source_hat: event.hat.clone(),
                        finding: finding.clone(),
                        reason_class: is_recoverable_policy_finding(&finding),
                    });
                    let recovery_payload = format!(
                        "EVENT_POLICY_REJECTED: event '{}' violates semantic gate (review step gate).\n{}\n\n\
                         Wave 未闭合，禁止 empty_diff；等待机制 plan.blocked 或补全维度后重发 review.passed。\
                         Wait for review-synthesizer terminal before plan-gate events.",
                        event.topic, finding.message
                    );
                    publish_policy_rejection_resume(bus, &event, recovery_payload, Some(review_step_tracker));
                } else {
                    review_step_tracker.observe_accepted(&event);
                    validated_events.push(event);
                }
            }
            PolicyDecision::RejectWithResume(finding) => {
                let mut finding = finding;
                let reason_class = is_recoverable_policy_finding(&finding);
                // U4 (2026-06-17-003 plan): when the rejection is a
                // `DuplicateWorkDone` and the event carries a
                // `wave_id` (wave is still open), upgrade the hint
                // from `DuplicateSameStep` to `DuplicateStallBypass`
                // so the recovery message warns the agent against
                // bypassing a stalled review cycle. Also include
                // the wave_id in the message for diagnostics.
                if let ViolationType::DuplicateWorkDone { ref mut hint, ref key } =
                    finding.violation_type
                {
                    if event.wave_id.is_some() {
                        *hint = DuplicateWorkDoneHint::DuplicateStallBypass;
                        finding.message = format!(
                            "duplicate_stall_bypass: work.done for key '{key}' was already accepted \
                             but wave_id={:?} is still open. The agent is attempting to re-emit \
                             work.done to bypass the stalled review cycle. Wait for review-synthesizer \
                             terminal (review.passed or review.complete) or plan.blocked before \
                             re-sending work.done.",
                            event.wave_id
                        );
                    }
                }
                if reason_class.is_none() {
                    // Non-recoverable: capture the violation
                    // for the U6 fast-fail path.  We still
                    // publish a `task.resume` so the U1 R5
                    // routing (semantic-gate and
                    // review-step-gate violations targeted at
                    // the source hat) keeps working — the
                    // runner's "fast-fail" happens at the U6
                    // `PayloadContractViolation` branch, not
                    // here.  The U2 plan §3 "R-B2" semantic
                    // (non-recoverable → no resume) is reserved
                    // for the future `plan_name`/task-key
                    // mismatch path (U3), not the existing U1
                    // semantic-gate `InvalidFieldValue` path.
                    capture_violation(&finding, event.payload.as_deref());
                }
                policy_rejections.push(crate::event_policy::PolicyRejection {
                    topic: event.topic.clone(),
                    source_hat: event.hat.clone(),
                    finding: finding.clone(),
                    reason_class,
                });
                if let Some(rc) = reason_class {
                    let hat_for_counter =
                        event.hat.as_deref().unwrap_or("unknown");
                    // U1 (2026-06-17-003 plan): `SemanticGateViolation`
                    // is in the recoverable set so the event is not
                    // fatal, but it is intentionally **not** pushed
                    // into `recoverable_candidates` — semantic-gate
                    // rejections (e.g. `review_passed_while_wave_open`)
                    // never count toward `U2_REJECTION_RETRY_LIMIT`.
                    // Otherwise a misbehaving review-coordinator
                    // would exhaust the budget on empty-diff retries
                    // and the loop would still terminate via
                    // `RecoverablePayloadExhausted`.  The mechanism
                    // emits `plan.blocked` (U2) before the budget
                    // can run out for any meaningful case.
                    if !matches!(rc, ReasonClass::SemanticGateViolation) {
                        recoverable_candidates.push(RecoverableExhaustionCandidate {
                            hat: hat_for_counter.to_string(),
                            topic: event.topic.clone(),
                            reason_class: rc,
                        });
                    }
                    // NOTE: do not `continue` here — we still
                    // want to publish a `task.resume` (the
                    // recoverable path's contract).  The caller
                    // will check the post-call counter and, if
                    // exhausted, the runner will terminate the
                    // loop on the next iteration pass.
                }
                let recovery_payload = format!(
                    "EVENT_POLICY_REJECTED: event '{}' violates policy.\n{}\n\n\
                     Wait for the correct event schema before emitting this event. \
                     The loop will continue to allow recovery.",
                    event.topic, finding.message
                );
                let recovery_payload = append_fix_hint_if_recoverable(
                    &recovery_payload,
                    event.hat.as_deref(),
                    &event.topic,
                    policy_config,
                    registry,
                );
                publish_policy_rejection_resume(bus, &event, recovery_payload, Some(review_step_tracker));
                // Do NOT record the rejected event
            }
            PolicyDecision::Hold(finding) => {
                let reason_class = is_recoverable_policy_finding(&finding);
                if reason_class.is_none() {
                    capture_violation(&finding, event.payload.as_deref());
                }
                hold_triggered = true;
                hold_reason = Some(finding.message.clone());
                policy_rejections.push(crate::event_policy::PolicyRejection {
                    topic: event.topic.clone(),
                    source_hat: event.hat.clone(),
                    finding: finding.clone(),
                    reason_class,
                });
                let recovery_payload = format!(
                    "EVENT_POLICY_HOLD: event '{}' violates policy.\n{}\n\n\
                     Loop held due to policy violation. Use resume to continue.",
                    event.topic, finding.message
                );
                let recovery_payload = append_fix_hint_if_recoverable(
                    &recovery_payload,
                    event.hat.as_deref(),
                    &event.topic,
                    policy_config,
                    registry,
                );
                publish_policy_rejection_resume(bus, &event, recovery_payload, Some(review_step_tracker));
            }
            PolicyDecision::Block(_finding) => {
                // Silently drop the event without publishing recovery or hold artifacts
            }
            PolicyDecision::Ignore(_finding) => {
                // Silently ignore the event without publishing recovery or hold artifacts
            }
        }
    }

    PolicyValidationResult {
        events: validated_events,
        hold_triggered,
        hold_reason,
        payload_contract_violation,
        policy_rejections,
        // Unit 2 (2026-06-16-002 plan) take-3: the validator no
        // longer calls `record_recoverable_rejection_key` itself.
        // The `recoverable_exhausted` field is left empty; the
        // caller consumes `recoverable_candidates` and
        // post-increments the counter itself.  We keep the
        // field for back-compat with the
        // `PolicyValidationResult` shape (existing call sites
        // destructure it).
        recoverable_exhausted: Vec::new(),
        recoverable_candidates,
    }
}

/// U6: convert a `PolicyFinding` into a `PayloadContractViolation` if and only
/// if the finding is schema-derived (MissingRequiredField, PayloadTypeMismatch,
/// InvalidFieldValue). Terminal-monotonicity and completion-guard violations
/// are NOT payload contract violations and are passed through unchanged.
///
/// U1 (2026-06-17-003 plan): `SemanticGateViolation` is also passed
/// through unchanged — semantic-gate rejections are recoverable
/// (`task.resume` → review-coordinator) and **not** a payload contract
/// violation. Treating them as one would re-introduce the
/// `PayloadContractViolation` fatal termination that this unit removes.
fn finding_to_payload_contract_violation(
    finding: &PolicyFinding,
    payload: Option<&str>,
    source_hats: &[String],
    target_hats: &[String],
    schema_defined_in: &str,
) -> Option<crate::payload_contract::PayloadContractViolation> {
    use crate::event_policy::ViolationType;
    use crate::payload_contract::{PayloadContractViolation, PayloadContractViolationKind};
    let (kind, field) = match &finding.violation_type {
        ViolationType::MissingRequiredField { field } => (
            PayloadContractViolationKind::MissingRequiredField,
            Some(field.clone()),
        ),
        ViolationType::PayloadTypeMismatch { .. } => {
            (PayloadContractViolationKind::PayloadTypeMismatch, None)
        }
        ViolationType::InvalidFieldValue { field, .. } => (
            PayloadContractViolationKind::AllowedValueMismatch,
            Some(field.clone()),
        ),
        // Terminal / completion-guard / topic-format / topic-deny /
        // semantic-gate violations are NOT payload contract violations
        // and must not be reported as such. Semantic-gate violations
        // still write a diagnostic envelope via the unified recovery
        // pipeline (see runner.rs `record_recovery_envelope`), but
        // they do NOT trigger the U6 `PayloadContractViolation`
        // fatal termination — see the runner's branch below.
        ViolationType::TerminalMonotonicityViolation { .. }
        | ViolationType::DuplicateTerminalEvent { .. }
        | ViolationType::BusinessEventAfterCompletion { .. }
        | ViolationType::InvalidTopicFormat { .. }
        | ViolationType::TopicDenied { .. }
        | ViolationType::SemanticGateViolation { .. }
        | ViolationType::DuplicateWorkDone { .. } => return None,
    };
    let fix_hint = match kind {
        PayloadContractViolationKind::MissingRequiredField => format!(
            "Add the missing field to the payload of the '{}' event. \
             If the field is optional, remove it from the schema's required_fields.",
            finding.topic
        ),
        PayloadContractViolationKind::PayloadTypeMismatch => format!(
            "Ensure the payload of '{}' matches the schema's declared payload type.",
            finding.topic
        ),
        PayloadContractViolationKind::AllowedValueMismatch => format!(
            "Update the payload of '{}' to a value allowed by the schema.",
            finding.topic
        ),
        PayloadContractViolationKind::SchemaMissingForRequiredTopic => {
            "Add a schema for this topic.".to_string()
        }
    };
    Some(PayloadContractViolation {
        error_type: kind,
        timestamp: chrono::Utc::now().to_rfc3339(),
        topic: finding.topic.clone(),
        field,
        source_hat: source_hats.to_vec(),
        target_hat: target_hats.to_vec(),
        schema_defined_in: schema_defined_in.to_string(),
        downstream_reference: None,
        upstream_reference: None,
        fix_hint,
        payload_excerpt: payload.map(|p| {
            const MAX: usize = 240;
            if p.len() > MAX {
                format!("{}…", &p[..MAX])
            } else {
                p.to_string()
            }
        }),
    })
}

impl EventLoop {
    /// 2026-06-09: returns the union of `verdict_gate.topic` and
    /// its `additional_topics`, or `None` when no gate is
    /// configured.  Used at every record-verdict call site so the
    /// 4 call sites stay in lockstep.  Allocates only when a
    /// gate is present (the per-iteration cost is paid once, not
    /// per event).
    pub(crate) fn verdict_gate_topics(&self) -> Option<Vec<String>> {
        self.config.event_loop.verdict_gate.as_ref().map(|v| {
            let mut topics = Vec::with_capacity(1 + v.additional_topics.len());
            topics.push(v.topic.clone());
            topics.extend(v.additional_topics.iter().cloned());
            topics
        })
    }

    /// Test-only getter for the cached robot_guidance vec.
    /// Used by `guidance_dedup.rs` to assert KTD-7 in-memory dedup
    /// without going through the full `process_events_from_jsonl`
    /// pipeline. `pub(crate)` so the sibling test modules under
    /// `event_loop::tests` can see it; gated by `#[cfg(test)]` so
    /// the symbol does not leak into release builds.
    #[cfg(test)]
    pub(crate) fn robot_guidance_for_test(&self) -> Vec<String> {
        self.robot_guidance.clone()
    }

    /// Creates a new event loop from configuration.
    pub fn new(config: RalphConfig) -> Self {
        // Try to create diagnostics collector, but fall back to disabled if it fails
        // (e.g., in tests without proper directory setup)
        let diagnostics = crate::diagnostics::DiagnosticsCollector::new(std::path::Path::new("."))
            .unwrap_or_else(|e| {
                debug!(
                    "Failed to initialize diagnostics: {}, using disabled collector",
                    e
                );
                crate::diagnostics::DiagnosticsCollector::disabled()
            });

        Self::with_diagnostics(config, diagnostics)
    }

    /// Creates a new event loop with a loop context for path resolution.
    ///
    /// The loop context determines where events, tasks, and other state files
    /// are located. Use this for multi-loop scenarios where each loop runs
    /// in an isolated workspace (git worktree).
    ///
    /// **Diagnostics ownership (U0).** If `context.prebuilt_diagnostics()` is
    /// `Some`, that collector is reused as the authoritative session — the
    /// CLI builds it in `main.rs` and shares it with the tracing layer so
    /// the run produces a single timestamped session dir. Otherwise, a
    /// fresh `DiagnosticsCollector::new(workspace)` is created. Either way,
    /// init failure falls back to a disabled collector (with a `tracing::warn!`)
    /// — diagnostics never panic the loop.
    pub fn with_context(config: RalphConfig, context: LoopContext) -> Self {
        let diagnostics = match context.prebuilt_diagnostics() {
            Some(collector) => (**collector).clone(),
            None => crate::diagnostics::DiagnosticsCollector::new(context.workspace())
                .unwrap_or_else(|e| {
                    warn!(
                        "Failed to initialize diagnostics: {}, using disabled collector",
                        e
                    );
                    crate::diagnostics::DiagnosticsCollector::disabled()
                }),
        };

        Self::with_context_and_diagnostics(config, context, diagnostics)
    }

    /// Creates a new event loop with explicit loop context and diagnostics.
    pub fn with_context_and_diagnostics(
        mut config: RalphConfig,
        context: LoopContext,
        diagnostics: crate::diagnostics::DiagnosticsCollector,
    ) -> Self {
        // Solo mode safety guard: force scratchpad enabled when no hats defined
        if config.hats.is_empty() && !config.core.scratchpad.enabled {
            warn!(
                "core.scratchpad.enabled is false but no hats are defined. \
                 Scratchpad is the only continuity mechanism in solo mode — forcing enabled."
            );
            config.core.scratchpad.enabled = true;
        }

        let registry = HatRegistry::from_runtime_config(&config);
        let publish_schemas = config
            .event_loop
            .event_policy
            .as_ref()
            .map(|p| p.schemas.clone())
            .unwrap_or_default();
        let instruction_builder = InstructionBuilder::with_publish_schemas(
            config.core.clone(),
            config.events.clone(),
            publish_schemas,
        );

        let mut bus = EventBus::new();

        // Per spec: "Hatless Ralph is constant — Cannot be replaced, overwritten, or configured away"
        // Ralph is ALWAYS registered as the universal fallback for orphaned events.
        // Custom hats are registered first (higher priority), Ralph catches everything else.
        // The builtin "ralph" hat is already registered in the registry via `from_runtime_config`.
        for hat in registry.all() {
            bus.register(hat.clone());
        }

        if config.hats.is_empty() {
            debug!("Solo mode: Ralph is the only coordinator");
        } else {
            debug!(
                "Multi-hat mode: {} custom hats + Ralph as fallback",
                config.hats.len()
            );
        }

        // Build skill registry from config
        let mut skill_registry = if config.skills.enabled {
            SkillRegistry::from_config(
                &config.skills,
                context.workspace(),
                Some(config.cli.backend.as_str()),
            )
            .unwrap_or_else(|e| {
                warn!(
                    "Failed to build skill registry: {}, using empty registry",
                    e
                );
                SkillRegistry::new(Some(config.cli.backend.as_str()))
            })
        } else {
            SkillRegistry::new(Some(config.cli.backend.as_str()))
        };

        // Remove task/memory skills from the index when their config is disabled
        if !config.tasks.enabled {
            skill_registry.remove("ralph-tools-tasks");
        }
        if !config.memories.enabled {
            skill_registry.remove("ralph-tools-memories");
        }

        let skill_index = if config.skills.enabled {
            skill_registry.build_index(None)
        } else {
            String::new()
        };

        // When memories are enabled, add tasks CLI instructions alongside scratchpad
        let ralph = HatlessRalph::new(
            config.event_loop.completion_promise.clone(),
            config.core.clone(),
            &registry,
            config.event_loop.starting_event.clone(),
        )
        .with_memories_enabled(config.memories.enabled)
        .with_skill_index(skill_index);

        // Read timestamped events path from marker file, fall back to default
        // The marker file contains a relative path like ".ralph/events-20260127-123456.jsonl"
        // which we resolve relative to the workspace root
        let events_path = std::fs::read_to_string(context.current_events_marker())
            .map(|s| {
                let relative = s.trim();
                context.workspace().join(relative)
            })
            .unwrap_or_else(|_| context.events_path());
        let event_reader = EventReader::new(&events_path);

        Self {
            config: config.clone(),
            registry,
            bus,
            state: LoopState::new(),
            instruction_builder,
            ralph,
            robot_guidance: Vec::new(),
            event_reader,
            diagnostics,
            loop_context: Some(context),
            skill_registry,
            robot_service: None,
            handoff_index: crate::workflow_contract::HandoffIndex::from_config(&config),
            recovery_responder: RecoveryResponder::new(Arc::new(
                config.telemetry.runtime_diagnosis.clone(),
            )),
            hat_lifecycle_tracker: ActivationLifecycleTracker::new(),
            ephemeral_isolation: crate::ephemeral_isolation::EphemeralIsolation::new(),
        }
    }

    /// R4 (2026-06-14-003 plan): explicit accessor returning
    /// whether the preset's `event_loop.enforce_current_unit` is
    /// active.  The CLI uses this to surface the value in
    /// diagnostics; the actual contract is enforced inside
    /// `TaskStore::ensure` after `ralph-cli`'s `task_cli` enables
    /// the contract unconditionally (the contract is opt-in at the
    /// *key* level — only `uN-` slugs are gated — so legacy keys
    /// are unaffected).
    pub fn enforce_current_unit_active(&self) -> bool {
        self.config.event_loop.enforce_current_unit
    }

    /// Creates a new event loop with explicit diagnostics collector (for testing).
    pub fn with_diagnostics(
        mut config: RalphConfig,
        diagnostics: crate::diagnostics::DiagnosticsCollector,
    ) -> Self {
        // Solo mode safety guard: force scratchpad enabled when no hats defined
        if config.hats.is_empty() && !config.core.scratchpad.enabled {
            warn!(
                "core.scratchpad.enabled is false but no hats are defined. \
                 Scratchpad is the only continuity mechanism in solo mode — forcing enabled."
            );
            config.core.scratchpad.enabled = true;
        }

        let registry = HatRegistry::from_runtime_config(&config);
        let publish_schemas = config
            .event_loop
            .event_policy
            .as_ref()
            .map(|p| p.schemas.clone())
            .unwrap_or_default();
        let instruction_builder = InstructionBuilder::with_publish_schemas(
            config.core.clone(),
            config.events.clone(),
            publish_schemas,
        );

        let mut bus = EventBus::new();

        // Per spec: "Hatless Ralph is constant — Cannot be replaced, overwritten, or configured away"
        // Ralph is ALWAYS registered as the universal fallback for orphaned events.
        // Custom hats are registered first (higher priority), Ralph catches everything else.
        // The builtin "ralph" hat is already registered in the registry via `from_runtime_config`.
        for hat in registry.all() {
            bus.register(hat.clone());
        }

        if config.hats.is_empty() {
            debug!("Solo mode: Ralph is the only coordinator");
        } else {
            debug!(
                "Multi-hat mode: {} custom hats + Ralph as fallback",
                config.hats.len()
            );
        }

        // Build skill registry from config
        let workspace_root = std::path::Path::new(".");
        let mut skill_registry = if config.skills.enabled {
            SkillRegistry::from_config(
                &config.skills,
                workspace_root,
                Some(config.cli.backend.as_str()),
            )
            .unwrap_or_else(|e| {
                warn!(
                    "Failed to build skill registry: {}, using empty registry",
                    e
                );
                SkillRegistry::new(Some(config.cli.backend.as_str()))
            })
        } else {
            SkillRegistry::new(Some(config.cli.backend.as_str()))
        };

        // Remove task/memory skills from the index when their config is disabled
        if !config.tasks.enabled {
            skill_registry.remove("ralph-tools-tasks");
        }
        if !config.memories.enabled {
            skill_registry.remove("ralph-tools-memories");
        }

        let skill_index = if config.skills.enabled {
            skill_registry.build_index(None)
        } else {
            String::new()
        };

        // When memories are enabled, add tasks CLI instructions alongside scratchpad
        let ralph = HatlessRalph::new(
            config.event_loop.completion_promise.clone(),
            config.core.clone(),
            &registry,
            config.event_loop.starting_event.clone(),
        )
        .with_memories_enabled(config.memories.enabled)
        .with_skill_index(skill_index);

        // Read events path from marker file, fall back to default if not present
        // The marker file is written by run_loop_impl() at run startup
        let events_path = std::fs::read_to_string(".ralph/current-events")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| ".ralph/events.jsonl".to_string());
        let event_reader = EventReader::new(&events_path);

        Self {
            config: config.clone(),
            registry,
            bus,
            state: LoopState::new(),
            instruction_builder,
            ralph,
            robot_guidance: Vec::new(),
            event_reader,
            diagnostics,
            loop_context: None,
            skill_registry,
            robot_service: None,
            recovery_responder: RecoveryResponder::new(Arc::new(
                config.telemetry.runtime_diagnosis.clone(),
            )),
            hat_lifecycle_tracker: ActivationLifecycleTracker::new(),
            ephemeral_isolation: crate::ephemeral_isolation::EphemeralIsolation::new(),
            handoff_index: crate::workflow_contract::HandoffIndex::from_config(&config),
        }
    }

    /// Injects a robot service for human-in-the-loop communication.
    ///
    /// Call this after construction to enable `human.interact` event handling,
    /// periodic check-ins, and question/response flow. The service is typically
    /// created by the CLI layer (e.g., `TelegramService`) and injected here,
    /// keeping the core event loop decoupled from any specific communication
    /// platform.
    pub fn set_robot_service(&mut self, service: Box<dyn RobotService>) {
        self.robot_service = Some(service);
    }

    /// Returns the loop context, if one was provided.
    pub fn loop_context(&self) -> Option<&LoopContext> {
        self.loop_context.as_ref()
    }

    /// Returns the tasks path based on loop context or default.
    fn tasks_path(&self) -> PathBuf {
        self.loop_context
            .as_ref()
            .map(|ctx| ctx.tasks_path())
            .unwrap_or_else(|| PathBuf::from(".ralph/agent/tasks.jsonl"))
    }

    /// Returns the scratchpad path based on loop context and active scratchpad config.
    ///
    /// When a per-hat scratchpad override is active (path differs from global default),
    /// the custom path is resolved relative to the loop context workspace for worktree
    /// isolation. When using the default/global path, loop context's standard resolution
    /// applies.
    fn scratchpad_path(&self) -> PathBuf {
        let active_path = &self.ralph.active_scratchpad().path;

        match self.loop_context.as_ref() {
            Some(ctx) => ctx.workspace().join(active_path),
            None => PathBuf::from(active_path),
        }
    }

    /// Returns the global scratchpad path (ignoring per-hat overrides).
    /// Used for guidance persistence which is cross-hat state.
    fn global_scratchpad_path(&self) -> PathBuf {
        self.loop_context
            .as_ref()
            .map(|ctx| ctx.scratchpad_path())
            .unwrap_or_else(|| PathBuf::from(&self.config.core.scratchpad.path))
    }

    /// Returns the current loop state.
    pub fn state(&self) -> &LoopState {
        &self.state
    }

    /// Returns a mutable reference to the loop state.  Used by the U2
    /// targeted-retry machinery in the loop runner to record
    /// per-rejection-key retry counts against the bounded budget
    /// without having to take a `&mut self` on the whole `EventLoop`
    /// in every helper.
    pub fn state_mut(&mut self) -> &mut LoopState {
        &mut self.state
    }

    /// Test-only: set the current iteration directly. Production code
    /// should never call this; the iteration is normally advanced by
    /// the main loop. Exposed at the `pub` level so external
    /// integration tests (e.g. `ralph-cli/loop_runner/tests.rs`) can
    /// pin the iteration value the recovery / gate code reads.
    pub fn set_iteration_for_test(&mut self, n: u32) {
        self.state.iteration = n;
    }

    /// Returns the diagnostics collector used by this event loop.
    ///
    /// Callers outside the event loop (e.g. the CLI loop runner) can use
    /// this to log structured diagnostics events through the standard
    /// `DiagnosticsCollector` API rather than hand-rolling file writes.
    pub fn diagnostics(&self) -> &crate::diagnostics::DiagnosticsCollector {
        &self.diagnostics
    }

    /// Returns a reference to the activation lifecycle tracker.
    ///
    /// This is the **read API** consumed by the `ralph diagnose` reporter (U4).
    /// Event loop decision paths must NOT call this — they only use write APIs
    /// (`activate`, `observe_accepted_event`, `complete`) to avoid implicit
    /// feedback loops.
    pub fn hat_lifecycle_tracker(&self) -> &ActivationLifecycleTracker<SystemTimeClock> {
        &self.hat_lifecycle_tracker
    }

    /// Test-only: returns a mutable reference to the activation lifecycle
    /// tracker so external integration tests can drive `activate` /
    /// `complete` through the public API. Production code paths
    /// (`build_prompt`, `process_events_from_jsonl`) access the field
    /// directly — this helper exists so the test boundary does not
    /// require `pub(crate)` on the field.
    #[cfg(test)]
    pub fn hat_lifecycle_tracker_mut(
        &mut self,
    ) -> &mut ActivationLifecycleTracker<SystemTimeClock> {
        &mut self.hat_lifecycle_tracker
    }

    /// Resets the stale-loop topic counter.
    ///
    /// Call after processing wave results — multiple events with the same topic
    /// (e.g. `review.done` from parallel workers) are expected and should not
    /// trigger the stale loop detector.
    pub fn reset_stale_topic_counter(&mut self) {
        self.state.consecutive_same_signature = 0;
        self.state.last_emitted_signature = None;
    }

    /// Increment the hard-gate counter when an agent claims emit but writes no event.
    pub fn increment_hard_gate_count(&mut self) {
        self.state.consecutive_hard_gates += 1;
    }

    /// Unit 3 (2026-06-16-002 plan): `true` while the loop is
    /// still in the bootstrap window — i.e. between the
    /// `work.start` publication and the first legal
    /// `coordinator work.ready` (without `reviewed_task_id`).
    ///
    /// During this window the `build_prompt` paths skip
    /// injecting `human.guidance` into the coordinator's
    /// prompt so the coordinator's first action is not
    /// derailed by stale human input.  Once
    /// `bootstrap_complete` flips to `true`, the gate opens
    /// and guidance flows normally.
    pub fn in_bootstrap_phase(&self) -> bool {
        !self.state.bootstrap_complete && !self.state.bootstrap_failed
    }

    /// Unit 3 (2026-06-16-002 plan): `true` when `hat_id ==
    /// "coordinator"` AND the loop is still in the bootstrap
    /// window.  The gate only applies to the `coordinator`
    /// hat (not the `ralph` solo hat and not the
    /// `review-synthesizer` / `executor` / other downstream
    /// hats).  When the gate is closed, the build_prompt
    /// paths must skip:
    ///   - `update_robot_guidance` (no `human.guidance`
    ///     caching for the prompt)
    ///   - `apply_robot_guidance` (no `ralph.robot_guidance`
    ///     push)
    ///   - `collect_robot_guidance` (isolated-path
    ///     `## ROBOT GUIDANCE` block)
    ///   - scratchpad `### HUMAN GUIDANCE` block inclusion
    ///     (handled in `prepend_scratchpad`).
    pub fn coordinator_bootstrap_gate_closed(&self, hat_id: &HatId) -> bool {
        hat_id.as_str() == "coordinator" && self.in_bootstrap_phase()
    }

    /// Reset the hard-gate counter when an agent successfully emits an event.
    pub fn reset_hard_gate_count(&mut self) {
        self.state.consecutive_hard_gates = 0;
    }

    /// Records the git HEAD SHA at loop start so execution-contract validation
    /// can detect commits produced during this loop.
    ///
    /// `None` clears the recorded SHA and falls back to diff-only evidence.
    /// Pass the value returned by `ralph_core::get_head_sha` from the loop
    /// runner at startup; pass `None` when the workspace is not a git repo
    /// or the SHA could not be resolved.
    pub fn set_loop_start_sha(&mut self, sha: Option<String>) {
        self.state.loop_start_sha = sha;
    }

    /// Maximum consecutive hard-gate triggers before the loop terminates.
    pub const HARD_GATE_MAX: u32 = 3;

    /// Returns the configuration.
    pub fn config(&self) -> &RalphConfig {
        &self.config
    }

    /// Returns the hat registry.
    pub fn registry(&self) -> &HatRegistry {
        &self.registry
    }

    /// Returns a mutable reference to the hat registry.
    pub fn registry_mut(&mut self) -> &mut HatRegistry {
        &mut self.registry
    }

    /// Returns true when the given `hat` is permitted to publish the given
    /// `topic` under the registry's publish rules.
    ///
    /// This is the shared isolated-scope predicate used by both the
    /// regular event path (`process_parse_result`) and the wave partition
    /// path (`process_events_from_jsonl_with_waves`). Centralising the
    /// call here keeps the two paths in lock-step when scope rules change
    /// — see U4 plan §4 KTD-U4-1 / A2.
    pub fn isolated_publish_allowed(&self, hat: &HatId, topic: &str) -> bool {
        self.registry.can_publish(hat, topic)
    }

    /// Enforce isolated publish scope on a batch of wave events.
    ///
    /// Groups events by `wave_id` (preserving first-seen order), then:
    ///   * the first distinct `wave_id` is allowed only if every event
    ///     in the group is in the isolated hat's `publishes` list — if
    ///     not, the whole group is dropped as
    ///     `WaveRejection::IsolatedScopeViolation`;
    ///   * any subsequent distinct `wave_id` is dropped as
    ///     `WaveRejection::IsolatedMultipleBusinessEmissions`.
    ///
    /// Each rejection publishes a `*.scope_violation` event to the bus
    /// and constructs a `WaveRejection` value so that the caller's
    /// B2 responder path can wire it to `record_recovery_envelope`.
    ///
    /// See U4 plan §3 KTD-U4-1, §3 KTD-U4-2, §4 A3.
    fn enforce_wave_isolated_scope(
        &mut self,
        events: Vec<crate::event_reader::Event>,
        isolated_hat: &HatId,
    ) -> std::io::Result<Vec<crate::event_reader::Event>> {
        use crate::wave_detection::WaveRejection;
        use std::collections::HashMap;

        // DEBUG: 添加入口日志
        let input_event_count = events.len();
        tracing::debug!(
            isolated_hat = %isolated_hat.as_str(),
            input_event_count = input_event_count,
            "enforce_wave_isolated_scope entry"
        );

        // Group by wave_id, preserving first-seen order. Wave counts
        // per read batch are bounded by `max_wave_total` (default 64),
        // so a Vec is fine for the order book; HashMap gives O(1) lookup.
        let mut order: Vec<String> = Vec::with_capacity(events.len());
        let mut groups: HashMap<String, Vec<crate::event_reader::Event>> = HashMap::new();
        for event in events {
            let key = event.wave_id.clone().unwrap_or_default();
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(event);
        }

        // DEBUG: 记录分组结果
        tracing::debug!(
            wave_groups = order.len(),
            total_events = input_event_count,
            "wave grouping result"
        );

        let mut kept: Vec<crate::event_reader::Event> = Vec::new();
        // Tracks whether ANY distinct `wave_id` has been observed in
        // this read batch, regardless of whether that wave was kept or
        // dropped. KTD-U4-2: a single isolated activation allows at
        // most one distinct `wave_id`; any further distinct wave_id is
        // typed as `IsolatedMultipleBusinessEmissions`, even if the
        // first wave itself was rejected for scope.
        let mut wave_observed: bool = false;

        for wave_id in order {
            let group = groups.remove(&wave_id).unwrap_or_default();
            if group.is_empty() {
                continue;
            }

            if !wave_observed {
                // First distinct wave: check isolated scope on every
                // event. If any event is out of scope, the whole wave
                // is dropped (one business emission rule). The wave
                // is still considered "observed" so the next distinct
                // wave_id is typed as `IsolatedMultipleBusinessEmissions`
                // — a second wave is never silently absorbed by the
                // scope check.
                if let Some(out_of_scope_topic) = group.iter().find_map(|e| {
                    // DEBUG: 添加调试日志追踪每个事件的 scope 检查
                    let allowed = self.isolated_publish_allowed(isolated_hat, e.topic.as_str());
                    tracing::debug!(
                        wave_id = %wave_id,
                        event_hat = ?e.hat.as_deref(),
                        topic = %e.topic,
                        allowed = %allowed,
                        "isolated scope check for wave event"
                    );
                    if allowed {
                        None
                    } else {
                        Some(e.topic.to_string())
                    }
                }) {
                    let rejection = WaveRejection::IsolatedScopeViolation {
                        wave_id: wave_id.clone(),
                        topic: out_of_scope_topic,
                        isolated_hat: isolated_hat.to_string(),
                    };
                    self.publish_isolated_wave_violation(&rejection, isolated_hat, &group);
                    wave_observed = true;
                    continue;
                }
                wave_observed = true;
                kept.extend(group);
            } else {
                // Subsequent distinct wave_id in the same read batch:
                // typed as `IsolatedMultipleBusinessEmissions`.
                let rejection = WaveRejection::IsolatedMultipleBusinessEmissions {
                    wave_id: wave_id.clone(),
                    isolated_hat: isolated_hat.to_string(),
                };
                self.publish_isolated_wave_violation(&rejection, isolated_hat, &group);
            }
        }

        Ok(kept)
    }

    /// Publish a `.scope_violation` diagnostic event and log a warning
    /// for an isolated wave rejection. The typed `WaveRejection` is
    /// recorded as a recovery finding in B2; for now this method only
    /// handles the diagnostic side so that A1–A3 land atomically.
    fn publish_isolated_wave_violation(
        &mut self,
        rejection: &crate::wave_detection::WaveRejection,
        isolated_hat: &HatId,
        events: &[crate::event_reader::Event],
    ) {
        use crate::wave_detection::WaveRejection;
        let (reason_code, topic_label, wave_id) = match rejection {
            WaveRejection::IsolatedScopeViolation { wave_id, topic, .. } => (
                "wave_isolated_scope_violation",
                topic.as_str(),
                wave_id.as_str(),
            ),
            WaveRejection::IsolatedMultipleBusinessEmissions { wave_id, .. } => (
                "wave_isolated_multiple_business_emissions",
                "",
                wave_id.as_str(),
            ),
            _ => ("wave_isolated_unknown", "", ""),
        };
        warn!(
            hat = %isolated_hat.as_str(),
            reason = reason_code,
            wave = wave_id,
            dropped = events.len(),
            "Isolated wave rejection — dropping whole wave"
        );
        let violation_topic = format!("{}.scope_violation", isolated_hat.as_str());
        let violation_payload = format!(
            "Isolated mode wave rejection ({reason_code}): hat '{}' dropped {} wave event(s) {}",
            isolated_hat.as_str(),
            events.len(),
            if topic_label.is_empty() {
                String::new()
            } else {
                format!("(out-of-scope topic '{topic_label}')")
            }
        );
        self.bus
            .publish(Event::new(violation_topic, violation_payload));

        // B2 / KTD-U4-4: record a recovery envelope so the responder
        // can track the finding. Outcome is `NotRetriable` per plan §3
        // KTD-U4-5 table — cap/structure and isolated-scope rejections
        // do not enter automatic recovery escalation.
        let retry_key = crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(
            wave_id,
            reason_code,
        );
        let message = format!(
            "Isolated wave {} rejected: hat '{}' cannot publish '{}'; {} event(s) dropped",
            wave_id,
            isolated_hat.as_str(),
            if topic_label.is_empty() {
                "(multi-business)"
            } else {
                topic_label
            },
            events.len(),
        );
        let mut builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::WaveDispatcher)
            .severity(crate::diagnosis::DiagnosisSeverity::Error)
            .iteration(self.state.iteration)
            .reason_code(reason_code)
            .message(message)
            .retry_attempt(0)
            .safe_target(false)
            .outcome(crate::diagnosis::DiagnosisOutcome::NotRetriable)
            .retry_key(retry_key)
            .source_hat(isolated_hat.to_string());
        if !topic_label.is_empty() {
            builder = builder.topic(topic_label.to_string());
        }
        if let Some(session_id) = self.diagnostics.session_id() {
            builder = builder.session_id(session_id);
        }
        let envelope = builder.build();
        self.record_recovery_envelope(&envelope, Vec::new());
    }

    /// Records hook telemetry for diagnostics.
    pub fn log_hook_run_telemetry(&self, entry: crate::diagnostics::HookRunTelemetryEntry) {
        self.diagnostics.log_hook_run(entry);
    }

    /// Logs the full prompt for an iteration to the diagnostics session.
    pub fn log_prompt(&self, iteration: u32, hat: &str, prompt: &str) {
        self.diagnostics.log_prompt(iteration, hat, prompt);
    }

    /// Gets the backend configuration for a hat.
    ///
    /// If the hat has a backend configured, returns that.
    /// Otherwise, returns None (caller should use global backend).
    pub fn get_hat_backend(&self, hat_id: &HatId) -> Option<&HatBackend> {
        self.registry
            .get_config(hat_id)
            .and_then(|config| config.backend.as_ref())
    }

    /// Adds an observer that receives all published events.
    ///
    /// Multiple observers can be added (e.g., session recorder + TUI).
    /// Each observer is called before events are routed to subscribers.
    pub fn add_observer<F>(&mut self, observer: F)
    where
        F: Fn(&Event) + Send + 'static,
    {
        self.bus.add_observer(observer);
    }

    /// Sets a single observer, clearing any existing observers.
    ///
    /// Prefer `add_observer` when multiple observers are needed.
    #[deprecated(since = "2.0.0", note = "Use add_observer instead")]
    pub fn set_observer<F>(&mut self, observer: F)
    where
        F: Fn(&Event) + Send + 'static,
    {
        #[allow(deprecated)]
        self.bus.set_observer(observer);
    }

    /// Checks if any termination condition is met.
    pub fn check_termination(&mut self) -> Option<TerminationReason> {
        let cfg = &self.config.event_loop;

        if self.state.iteration >= cfg.max_iterations {
            return Some(TerminationReason::MaxIterations);
        }

        if self.state.elapsed().as_secs() >= cfg.max_runtime_seconds {
            return Some(TerminationReason::MaxRuntime);
        }

        if let Some(max_cost) = cfg.max_cost_usd
            && self.state.cumulative_cost >= max_cost
        {
            return Some(TerminationReason::MaxCost);
        }

        if self.state.consecutive_failures >= cfg.max_consecutive_failures {
            return Some(TerminationReason::ConsecutiveFailures);
        }

        // Check for loop thrashing: planner keeps dispatching abandoned tasks
        if self.state.abandoned_task_redispatches >= 3 {
            return Some(TerminationReason::LoopThrashing);
        }

        // Check for validation failures: too many consecutive malformed JSONL lines
        if self.state.consecutive_malformed_events >= 3 {
            return Some(TerminationReason::ValidationFailure);
        }

        // Check for hard-gate exhaustion: agent repeatedly claims emit but never writes
        if self.state.consecutive_hard_gates >= Self::HARD_GATE_MAX {
            warn!(
                count = self.state.consecutive_hard_gates,
                "Hard gate exhausted: agent repeatedly claimed to emit events but never wrote them"
            );
            return Some(TerminationReason::Stopped);
        }

        // Check for stale loop: same event signature emitted 3+ times in a row
        if self.state.consecutive_same_signature >= 3 {
            let topic = self
                .state
                .last_emitted_signature
                .as_ref()
                .map(|signature| signature.topic.as_str())
                .unwrap_or("?");
            warn!(
                topic,
                count = self.state.consecutive_same_signature,
                "Stale loop detected: same event signature emitted consecutively"
            );
            return Some(TerminationReason::LoopStale);
        }

        // P0-C (2026-06-10): fail-path auto-termination. When the
        // verdict gate is configured and a failing verdict has been
        // observed, AND that verdict has propagated to the LAST
        // configured topic in the gate's mirror chain, terminate with
        // `ReviewFailed`. Closes the "loop hangs after failing review"
        // gap where the gate forbids `LOOP_COMPLETE` on fail (correct)
        // but offered no other exit signal.
        //
        // Semantics: `gate.topic` is the upstream verdict (e.g.
        // `REVIEW_COMPLETE`); `gate.additional_topics` lists
        // downstream mirror events in propagation order (e.g.
        // `report.done`). The "last" mirror is whichever entry is
        // the final downstream — when the verdict is observed on
        // THAT topic, the workflow has reached its terminus.
        if let Some(gate) = self.config.event_loop.verdict_gate.as_ref()
            && let Some(topic) = self.state.last_verdict_topic.as_deref()
            && let Some(payload) = self.state.last_verdict_payload.as_deref()
            && Self::verdict_payload_is_fail(payload, gate)
        {
            // The "expected last" topic is the final mirror in the
            // gate's chain — `additional_topics.last()` if non-empty,
            // else `topic` itself.
            let expected_last = gate
                .additional_topics
                .last()
                .cloned()
                .unwrap_or_else(|| gate.topic.clone());
            if topic == expected_last.as_str() {
                info!(
                    verdict_topic = %topic,
                    fail_field = %gate.fail_field,
                    fail_value = %gate.fail_value,
                    "Verdict gate fail verdict fully propagated — auto-terminating with ReviewFailed"
                );
                return Some(TerminationReason::ReviewFailed {
                    topic: topic.to_string(),
                });
            }
        }

        // 2026-06-14-004 U2: isolated-scope circuit breaker check.
        // If the rejection branch tripped the breaker, the original
        // (non-normalized) termination reason is stored in LoopState.
        // This path does not depend on telemetry.runtime_diagnosis.
        if let Some(reason) = self.state.scope_violation_circuit_breaker_tripped.take() {
            if let TerminationReason::ScopeViolationCircuitBreakerTripped {
                ref hat,
                ref topic,
                violation_count,
                ..
            } = reason
            {
                warn!(
                    hat = %hat,
                    topic = %topic,
                    violation_count = violation_count,
                    "Scope violation circuit breaker tripped: terminating loop"
                );
            }
            return Some(reason);
        }

        // Check for stop signal from Telegram /stop or CLI stop-requested
        let stop_path =
            std::path::Path::new(&self.config.core.workspace_root).join(".ralph/stop-requested");
        if stop_path.exists() {
            let _ = std::fs::remove_file(&stop_path);
            return Some(TerminationReason::Stopped);
        }

        // Check for restart signal from Telegram /restart command
        let restart_path =
            std::path::Path::new(&self.config.core.workspace_root).join(".ralph/restart-requested");
        if restart_path.exists() {
            return Some(TerminationReason::RestartRequested);
        }

        // Check if workspace directory has been removed (zombie worktree detection)
        if !std::path::Path::new(&self.config.core.workspace_root).is_dir() {
            return Some(TerminationReason::WorkspaceGone);
        }

        None
    }

    /// Check if a loop.cancel event was detected.
    ///
    /// Unlike check_completion_event(), this does NOT validate required_events.
    /// Cancellation is an explicit abort — it doesn't need the workflow to be complete.
    pub fn check_cancellation_event(&mut self) -> Option<TerminationReason> {
        if !self.state.cancellation_requested {
            return None;
        }
        self.state.cancellation_requested = false;
        info!("Loop cancelled gracefully via loop.cancel event");

        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::LoopTerminated {
                reason: "cancelled".to_string(),
            },
        );

        Some(TerminationReason::Cancelled)
    }

    /// Request completion from the text fallback path.
    ///
    /// When a backend outputs a completion promise as plain text (without
    /// using `ralph emit`), this sets `completion_requested = true` so that
    /// `check_completion_event()` can apply all safety checks (persistent mode,
    /// required events, runtime tasks) before terminating.
    pub fn request_completion_from_text_fallback(&mut self) {
        if self.state.completion_honored {
            debug!("Completion already handled, ignoring text fallback request");
            return;
        }
        self.state.completion_requested = true;
        info!("Completion requested via text fallback (output contained completion promise)");
    }

    /// Checks if a completion event was received and returns termination reason.
    ///
    /// Completion is accepted via JSONL events (e.g., `ralph emit`) or via
    /// [`request_completion_from_text_fallback`].
    pub fn check_completion_event(&mut self) -> Option<TerminationReason> {
        // Idempotency: if we already handled completion, return the same conclusion
        if self.state.completion_honored {
            return Some(TerminationReason::CompletionPromise);
        }

        if !self.state.completion_requested {
            return None;
        }

        // Event chain validation: check required events were seen
        let required = self.config.event_loop.required_events.clone();
        if !required.is_empty() {
            let missing = self.state.missing_required_events(&required);
            if !missing.is_empty() {
                warn!(
                    missing = ?missing,
                    "Rejecting LOOP_COMPLETE: required events not seen during loop lifetime"
                );
                let sig = format!(
                    "missing_required:{}",
                    missing
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                );
                if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                    return Some(reason);
                }
                self.state.completion_requested = false;

                // Inject task.resume so the loop continues
                let resume_payload = format!(
                    "LOOP_COMPLETE rejected: missing required events: {:?}. \
                     The agent must complete all workflow phases before emitting LOOP_COMPLETE. \
                     Use loop.cancel to abort the workflow instead.",
                    missing
                );
                self.bus.publish(Event::new("task.resume", resume_payload));
                return None;
            }
        }

        let state_machine_enabled = self
            .config
            .event_loop
            .state_machine
            .as_ref()
            .is_some_and(|sm| sm.enabled);

        // Verdict gate: when configured, the most recent event matching the gate
        // topic must NOT carry fail_field == fail_value. This prevents a hat from
        // declaring success in its final review while bypassing the backstop check.
        if let Some(gate) = self.config.event_loop.verdict_gate.clone() {
            if let Some(payload) = self.state.last_verdict_payload.as_deref()
                && Self::verdict_payload_is_fail(payload, &gate)
            {
                warn!(
                    topic = %gate.topic,
                    field = %gate.fail_field,
                    value = %gate.fail_value,
                    "Rejecting LOOP_COMPLETE: verdict gate observed a failing verdict"
                );
                let sig = format!("verdict_fail:{}", gate.topic);
                if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                    return Some(reason);
                }
                self.state.completion_requested = false;

                let resume_payload = format!(
                    "LOOP_COMPLETE rejected: most recent {} event has {}={}. \
                     The workflow has not passed final review. Use loop.cancel to abort instead.",
                    gate.topic, gate.fail_field, gate.fail_value
                );
                self.bus.publish(Event::new("task.resume", resume_payload));
                return None;
            }
        }

        // Workflow guard completion validation: ensure all started guarded instances are terminal.
        // State-machine configs use their instance lifecycle as the completion source of truth.
        if !state_machine_enabled
            && let Some(guards) = &self.config.event_loop.workflow_guards
            && !guards.chains.is_empty()
            && let Some(rejection) = self.check_workflow_guard_completion(guards)
        {
            warn!(
                reason = %rejection.message,
                "Rejecting LOOP_COMPLETE: incomplete workflow guard instances"
            );
            // Build a stable signature from the rejection message to detect same-guard rejections
            let sig = format!("workflow_guard:{}", rejection.message);
            if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                return Some(reason);
            }
            self.state.completion_requested = false;

            let resume_payload = format!(
                "LOOP_COMPLETE rejected: {}. \
                 All workflow instances must reach a terminal phase before emitting LOOP_COMPLETE. \
                 Use loop.cancel to abort the workflow instead.",
                rejection.message
            );
            self.bus.publish(Event::new("task.resume", resume_payload));
            return None;
        }

        self.state.completion_requested = false;

        // In persistent mode, suppress completion and keep the loop alive
        if self.config.event_loop.persistent {
            info!("Completion event suppressed - persistent mode active, loop staying alive");

            self.diagnostics.log_orchestration(
                self.state.iteration,
                "loop",
                crate::diagnostics::OrchestrationEvent::LoopTerminated {
                    reason: "completion_event_suppressed_persistent".to_string(),
                },
            );

            // Inject a task.resume event so the loop continues with an idle prompt
            let resume_event = Event::new(
                "task.resume",
                "Persistent mode: loop staying alive after completion signal. \
                 Check for new tasks or await human guidance.",
            );
            self.bus.publish(resume_event);

            return None;
        }

        // Runtime tasks are the canonical queue when memories/tasks mode is enabled.
        if self.config.memories.enabled {
            if let Ok(false) = self.verify_tasks_complete() {
                let open_tasks = self.get_open_task_list();
                warn!(
                    open_tasks = ?open_tasks,
                    "Rejecting completion event with {} open task(s)",
                    open_tasks.len()
                );
                // Build a stable signature from sorted task IDs to detect same-set rejections
                let mut task_ids: Vec<&str> = open_tasks
                    .iter()
                    .filter_map(|t| t.split(':').next())
                    .collect();
                task_ids.sort();
                let task_ids_hash = {
                    use std::hash::{DefaultHasher, Hash, Hasher};
                    let mut h = DefaultHasher::new();
                    for id in &task_ids {
                        id.hash(&mut h);
                    }
                    h.finish()
                };
                let sig = format!("open_tasks:{}:{}", open_tasks.len(), task_ids_hash);
                if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                    return Some(reason);
                }
                self.state.completion_requested = false;
                self.bus.publish(Event::new(
                    "task.resume",
                    format!(
                        "Completion rejected: runtime tasks remain open: {:?}. Close, fail, or reopen outstanding tasks before emitting the completion promise.",
                        open_tasks
                    ),
                ));
                return None;
            }
        } else if let Ok(false) = self.verify_scratchpad_complete() {
            warn!("Completion event with pending scratchpad tasks - trusting agent decision");
        }

        // Completion accepted — reset stale-breaker state.
        self.state.completion_rejection_signature = None;
        self.state.consecutive_completion_rejections = 0;
        self.state.last_rejection_fingerprint = 0;

        info!("Completion event detected - terminating");

        // Log loop terminated
        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::LoopTerminated {
                reason: "completion_event".to_string(),
            },
        );

        self.state.completion_honored = true;

        if state_machine_enabled
            && let Some(ref mut sm_state) = self.state.state_machine_runtime_state
        {
            sm_state.mark_terminal_honored();
        }

        // Record completion honored in policy runtime state for downstream guarding
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            if let Some(ref mut policy_state) = self.state.policy_runtime_state {
                policy_state.completion_honored = true;
                policy_state.completion_topic =
                    Some(self.config.event_loop.completion_promise.clone());
                policy_state.completion_iteration = Some(self.state.iteration);
            }
        }

        Some(TerminationReason::CompletionPromise)
    }

    /// Tracks completion rejections for the stale-breaker mechanism.
    ///
    /// If the same rejection signature repeats 3+ times with no meaningful
    /// progress between rejections (business events, task state changes,
    /// workflow advancement, or state machine transitions), returns
    /// `TerminationReason::LoopStale` to prevent infinite API-burning loops.
    ///
    /// `task_snapshot` is `(open_count, closed_count)` from the task store.
    fn handle_completion_rejection(
        &mut self,
        signature: String,
        task_snapshot: (usize, usize),
    ) -> Option<TerminationReason> {
        let mut fingerprint = self.state.compute_progress_fingerprint();
        fingerprint.task_snapshot = task_snapshot;
        let current_fp = fingerprint.hash();

        let is_same = self.state.completion_rejection_signature.as_ref() == Some(&signature);
        let has_progress = current_fp != self.state.last_rejection_fingerprint;

        if is_same && !has_progress {
            self.state.consecutive_completion_rejections += 1;
            if self.state.consecutive_completion_rejections >= 3 {
                warn!(
                    signature = %signature,
                    count = self.state.consecutive_completion_rejections,
                    "Stale-breaker: same completion rejection repeated 3+ times with no progress"
                );
                return Some(TerminationReason::LoopStale);
            }
        } else if is_same && has_progress {
            // Same rejection reason but progress was made — reset counter
            self.state.consecutive_completion_rejections = 1;
        } else {
            // Different rejection reason — reset counter
            self.state.consecutive_completion_rejections = 1;
        }

        self.state.completion_rejection_signature = Some(signature);
        self.state.last_rejection_fingerprint = current_fp;
        None
    }

    /// Returns true if the verdict event payload contains `gate.fail_field == gate.fail_value`.
    ///
    /// Used by `check_completion_event` to enforce a verdict gate: the most recent
    /// event matching the configured verdict topic must not carry a failing verdict.
    /// Returns false when the payload is not valid JSON or the field is absent —
    /// absence is treated as "not failing" because the gate is opt-in and only
    /// trips on an explicit `fail` value.
    fn verdict_payload_is_fail(payload: &str, gate: &crate::config::VerdictGateConfig) -> bool {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
            return false;
        };
        value
            .get(&gate.fail_field)
            .and_then(|v| v.as_str())
            .is_some_and(|s| s == gate.fail_value)
    }

    /// Initializes the loop by publishing the start event.
    pub fn initialize(&mut self, prompt_content: &str) {
        // Use configured starting_event or default to task.start for backward compatibility
        let topic = self
            .config
            .event_loop
            .starting_event
            .clone()
            .unwrap_or_else(|| "task.start".to_string());
        self.initialize_with_topic(&topic, prompt_content);
    }

    /// Initializes the loop for resume mode by publishing task.resume.
    ///
    /// Per spec: "User can run `ralph resume` to restart reading existing scratchpad."
    /// The planner should read the existing scratchpad rather than doing fresh gap analysis.
    pub fn initialize_resume(&mut self, prompt_content: &str) {
        // Resume always uses task.resume regardless of starting_event config
        self.initialize_with_topic("task.resume", prompt_content);
        // Unit 3: rebuild bootstrap gate from recorded events so resume
        // does not re-open the guidance-suppression window mid-loop.
        self.rebuild_bootstrap_flags_from_recorded_events();
    }

    /// Common initialization logic with configurable topic.
    fn initialize_with_topic(&mut self, topic: &str, prompt_content: &str) {
        // Store the objective so it persists across all iterations.
        // After iteration 1, bus.take_pending() consumes the start event,
        // so without this the objective would be invisible to later hats.
        self.ralph.set_objective(prompt_content.to_string());

        // Unit 3 (2026-06-16-002 plan): reset the bootstrap gate only on
        // a fresh loop start — not on `task.resume` (resume rebuilds from
        // events.jsonl immediately after).
        if topic == "work.start" || topic == "task.start" {
            self.state.bootstrap_complete = false;
            self.state.bootstrap_failed = false;
        }

        let start_event = Event::new(topic, prompt_content);
        self.bus.publish(start_event);
        debug!(topic = topic, "Published {} event", topic);
    }

    /// Write a hold artifact when event policy triggers a hold.
    fn write_hold_artifact(&self, reason: Option<&str>) -> std::io::Result<()> {
        let workspace = self
            .loop_context
            .as_ref()
            .map(|ctx| ctx.workspace().to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let ralph_dir = workspace.join(".ralph");
        std::fs::create_dir_all(&ralph_dir)?;

        let hold_path = ralph_dir.join("hold-state.json");
        let hold_record = serde_json::json!({
            "schema_version": 1,
            "source": "event_policy",
            "reason": reason.unwrap_or("Policy violation"),
            "held_at": chrono::Utc::now().to_rfc3339(),
        });
        let bytes = serde_json::to_vec_pretty(&hold_record)?;

        // Atomic write
        let temp_path = ralph_dir.join(format!(
            ".hold-state.tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&temp_path, &bytes)?;
        std::fs::rename(&temp_path, &hold_path)?;

        info!(path = ?hold_path, "Wrote hold-state artifact");
        Ok(())
    }

    /// Gets the next hat to execute (if any have pending events).
    ///
    /// Per "Hatless Ralph" architecture: When custom hats are defined, Ralph is
    /// always the executor. Custom hats define topology (pub/sub contracts) that
    /// Ralph uses for coordination context, but Ralph handles all iterations.
    ///
    /// - Solo mode (no custom hats): Returns "ralph" if Ralph has pending events
    /// - Multi-hat mode (custom hats defined): Always returns "ralph" if ANY hat has pending events
    ///
    /// **Isolated mode** uses round-robin scheduling via
    /// `EventBus::select_next_hat_with_pending` to guarantee fair selection
    /// among all pending hats. The cursor is anchored in the full
    /// registered hat order, so a hat whose queue is drained or
    /// deregistered does not reset the cursor to the lexicographic first
    /// non-empty hat.
    ///
    /// **NOTE**: This method takes `&mut self` because isolated-mode round-robin
    /// advances the bus's internal cursor.
    pub fn next_hat(&mut self) -> Option<&HatId> {
        // U3 (2026-06-13-001 plan): hard-gate / wave-recovery hat pinning.
        //
        // When a `pending_recovery_hat` is recorded (set by the
        // runner's `inject_missing_event_hard_gate_guidance` or
        // `inject_wave_policy_rejection_guidance` helpers), the next
        // iteration MUST activate that hat, not whatever the
        // round-robin / coordinator topology would pick.  The default
        // round-robin would otherwise drift to `executor` after a
        // `review-coordinator` hard gate, breaking the loop.
        //
        // We use `take` semantics: the field is cleared on the
        // iteration that consumes it, so the loop never gets stuck on
        // a single hat past a single activation.  The `bus` already
        // publishes the recovery `human.guidance` event for that hat,
        // so the next prompt will see the schema-level / missing-
        // event message and the obligation should be satisfied on the
        // very next attempt.
        if let Some(pending_hat) = self.state.pending_recovery_hat.take() {
            // Only honor the pin when the hat is actually registered;
            // an obsolete or test-only hat id is treated as a no-op
            // and selection falls through to the normal algorithm.
            if self.bus.hat_ids().any(|id| *id == pending_hat) {
                return self.bus.hat_ids().find(|id| **id == pending_hat);
            }
            // Hat unknown (config drift, deregistration, or worktree
            // with a different hat set) — log so the operator can
            // see the recovery intent was lost instead of silently
            // routing to a different hat via round-robin.
            tracing::warn!(
                pending_hat = %pending_hat,
                "pending_recovery_hat references an unregistered hat id; falling through to default selection"
            );
        }

        match self.config.event_loop.execution_mode {
            HatExecutionMode::Isolated => {
                // Isolated mode: use round-robin to select the next hat.
                // This advances the cursor on the bus for fair scheduling.
                if self.bus.has_human_pending() && !self.bus.has_pending_non_human() {
                    // Only human events pending — route to ralph.
                    return self.bus.hat_ids().find(|id| id.as_str() == "ralph");
                }
                // WAC-U5 (2026-06-12-002): handoff priority pre-emption.
                // If the HandoffIndex has at least one priority-eligible
                // entry (unique consumer) and that hat currently has a
                // non-empty pending queue, the dispatcher selects it
                // immediately and the round-robin cursor advances. The
                // scan walks the index in BTreeMap (alphabetical topic)
                // order for determinism. If no priority hat has pending
                // events, we fall through to the normal round-robin
                // pass.
                let priority_hat: Option<HatId> =
                    self.handoff_index.entries.values().find_map(|entry| {
                        let consumer = entry.consumer.as_deref()?;
                        let has_pending = self
                            .bus
                            .peek_pending(&HatId::from(consumer))
                            .map(|q| !q.is_empty())
                            .unwrap_or(false);
                        if has_pending {
                            Some(HatId::from(consumer))
                        } else {
                            None
                        }
                    });
                // Select via round-robin. This updates last_selected.
                // We need to return a borrowed HatId, so we select and then look it up.
                let selected = self
                    .bus
                    .select_next_hat_with_pending(priority_hat.as_ref())?;
                // The selected hat must exist in the bus (it was found in pending).
                self.bus.hat_ids().find(|id| *id == &selected)
            }
            HatExecutionMode::Coordinator => {
                // Coordinator mode: peek for pending, then return ralph if any.
                let has_pending = self.bus.peek_next_hat_with_pending().is_some();

                // If no pending hat events but human interactions are pending, route to Ralph.
                if !has_pending && self.bus.has_human_pending() {
                    return self.bus.hat_ids().find(|id| id.as_str() == "ralph");
                }

                if !has_pending {
                    return None;
                }

                // Coordinator mode (default): In multi-hat mode, always route to Ralph
                // (custom hats define topology only). Ralph's prompt includes the ## HATS
                // section for coordination awareness.
                if self.config.hats.is_empty() {
                    // Solo mode - return the next hat (which is "ralph")
                    self.bus.hat_ids().find(|id| id.as_str() == "ralph")
                } else {
                    // Return "ralph" - the constant coordinator
                    self.bus.hat_ids().find(|id| id.as_str() == "ralph")
                }
            }
        }
    }

    /// Returns the hat that will be triggered by the next pending event, if any.
    pub fn triggered_hat(&mut self) -> Option<HatId> {
        self.next_hat().cloned()
    }

    /// Advances the event reader to the current end of the events file.
    ///
    /// Call this after writing observability records (e.g. start event) to the
    /// events JSONL file so they are not re-read by `process_events_from_jsonl`.
    /// The start event is already published to the bus via `initialize()`, so
    /// re-reading it from the file would cause double-delivery.
    pub fn sync_event_reader_to_file_end(&mut self) {
        let path = self.event_reader.path();
        if let Ok(metadata) = std::fs::metadata(path) {
            self.event_reader.set_position(metadata.len());
        }
    }

    /// Points the JSONL candidate reader at a different file and resets its
    /// offset. State-machine runs use this to keep raw candidate events
    /// separate from the accepted event history.
    pub fn set_event_reader_path(&mut self, path: impl Into<PathBuf>) {
        self.event_reader = EventReader::new(path);
    }

    /// Checks if any hats have pending events.
    ///
    /// Use this after `process_output` to detect if the LLM failed to publish an event.
    /// If false after processing, the loop will terminate on the next iteration.
    ///
    /// Uses peek (no side-effect) to avoid advancing the round-robin cursor.
    pub fn has_pending_events(&self) -> bool {
        self.bus.has_pending()
    }

    /// Checks if any pending events are human-related (human.response, human.guidance).
    ///
    /// Used to skip cooldown delays when a human event is next, since we don't
    /// want to artificially delay the response to a human interaction.
    pub fn has_pending_human_events(&self) -> bool {
        self.bus.has_human_pending()
    }

    /// Injects `human.guidance` events directly into the in-memory bus.
    ///
    /// This is used for local TUI/RPC guidance so the next prompt boundary
    /// sees the message immediately without waiting for a JSONL reread.
    pub fn inject_human_guidance<I, S>(&mut self, messages: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let verdict_topics = self.verdict_gate_topics();
        let verdict_topics_slice = verdict_topics.as_deref();
        for message in messages {
            let event = Event::new("human.guidance", message.into());
            self.state.record_event(&event);
            self.state
                .record_verdict_if_match(&event, verdict_topics_slice);
            self.bus.publish(event);
        }
    }

    /// Returns whether unread JSONL events include any semantic `plan.*` topics.
    ///
    /// This allows callers to dispatch `pre.plan.created` hooks before
    /// event publication handling without consuming unread events.
    pub fn has_pending_plan_events_in_jsonl(&self) -> std::io::Result<bool> {
        let result = self.event_reader.peek_new_events()?;
        Ok(result
            .events
            .iter()
            .any(|event| event.topic.starts_with("plan.")))
    }

    /// Returns structured context for the first unread `human.interact` event,
    /// if one is present in JSONL without consuming reader state.
    pub fn pending_human_interact_context_in_jsonl(&self) -> std::io::Result<Option<Value>> {
        let result = self.event_reader.peek_new_events()?;
        Ok(result
            .events
            .iter()
            .find(|event| event.topic == "human.interact")
            .map(|event| {
                Self::parse_human_interact_context(event.payload.as_deref().unwrap_or_default())
            }))
    }

    /// Gets the topics a hat is allowed to publish.
    ///
    /// Used to build retry prompts when the LLM forgets to publish an event.
    pub fn get_hat_publishes(&self, hat_id: &HatId) -> Vec<String> {
        self.registry
            .get(hat_id)
            .map(|hat| hat.publishes.iter().map(|t| t.to_string()).collect())
            .unwrap_or_default()
    }

    /// U2 (2026-06-17-003 plan): mechanism-emitted `plan.blocked`
    /// for a review wave that has stalled below `wave_total` past
    /// `0.8 * aggregate_timeout_secs` without further
    /// `dimension.done` progress.
    ///
    /// The hat provenance is `review-synthesizer` (so the event
    /// passes the isolated-scope publish allowlist check); the
    /// target is `shipper` per plan §Key Technical Decisions —
    /// `plan-gate.triggers` does NOT include `plan.blocked`, so
    /// routing through plan-gate would silently drop the event.
    /// The wave is then closed in the tracker (`open_wave_id =
    /// None`) so the gate does not re-fire on the next
    /// iteration.
    ///
    /// Called **before** [`Self::inject_review_aggregate_timeouts`]
    /// (per plan §U2 fixed order: incomplete-wave gate →
    /// handoff-expired → process JSONL → policy validation).
    /// When this method emits a `plan.blocked`, the U4 path is
    /// not consulted in the same iteration.
    ///
    /// Returns `true` if a `plan.blocked` was emitted.
    pub fn maybe_emit_incomplete_wave_blocked(&mut self) -> bool {
        use crate::flow_lifecycle::incomplete_wave_gate::{
            IncompleteWaveGate, IncompleteWaveGateConfig,
        };

        // Plan §U2: global default off, `ce-executor-isolated`
        // default on. The config key is
        // `workflow_contract.incomplete_wave_gate.enabled`. We
        // read it defensively — when the preset does not set it,
        // the helper returns None and the gate stays disabled.
        let enabled = self
            .config
            .event_loop
            .workflow_contract
            .as_ref()
            .map(|wc| wc.incomplete_wave_gate.enabled)
            .unwrap_or(false);
        if !enabled {
            return false;
        }

        // Compute staleness from the configured `review-synthesizer`
        // aggregate.timeout — matches what U4
        // `inject_review_aggregate_timeouts` reads.
        let aggregate_timeout_secs = self
            .registry
            .get_config(&HatId::new("review-synthesizer"))
            .and_then(|cfg| cfg.aggregate.as_ref())
            .map(|agg| u64::from(agg.timeout))
            .unwrap_or(300);
        let gate = IncompleteWaveGate::new(IncompleteWaveGateConfig {
            enabled: true,
            staleness_ratio: 0.8,
        });
        let staleness_secs = gate.staleness_secs(aggregate_timeout_secs);
        let candidates = self
            .state
            .review_step_tracker
            .open_waves_needing_intervention(staleness_secs);
        if candidates.is_empty() {
            return false;
        }
        // Use the first candidate per iteration — closing its
        // wave before the next call prevents re-emit. The next
        // iteration will pick up any remaining stalled waves.
        let info = candidates.into_iter().next().unwrap();

        let last_dim_secs_ago = info.last_dimension_at.map(|t| t.elapsed().as_secs());
        let payload = gate.evaluate(
            &self.state.flow_lifecycle,
            aggregate_timeout_secs,
            &info.wave_id,
            info.expected,
            info.received,
            last_dim_secs_ago,
        );
        let Some(mut payload) = payload else {
            return false;
        };
        // Fill in the per-step correlation fields from the
        // tracker observation.
        payload.plan_name = info.plan_name;
        payload.task_id = info.task_id;
        payload.step = info.step;

        // The plan requires the publish provenance be
        // `review-synthesizer` (which has `plan.blocked` in its
        // `publishes` allowlist per the preset validator). The
        // `Event::with_source(...)` helper stamps the producer
        // hat; `with_target(...)` routes to `shipper`.
        let json_payload = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let event = Event::new("plan.blocked", json_payload)
            .with_source(HatId::new("review-synthesizer"))
            .with_target(HatId::new("shipper"));
        debug!(
            wave_id = %info.wave_id,
            expected = info.expected,
            received = info.received,
            "U2: emitting mechanism-level plan.blocked (dimension_reviewers_failed_to_converge)"
        );
        self.bus.publish(event);

        // Close the wave in the tracker so the gate does not
        // re-fire on subsequent iterations. We do not change
        // `synth_terminal` / `synth_pass` here — the closed
        // wave has a `plan.blocked` outcome, not a `review.passed`
        // verdict, so plan-gate must not see it as terminal.
        let key = review_step_state::StepKey {
            plan_name: payload.plan_name.clone(),
            task_id: payload.task_id.clone(),
            step: payload.step.clone(),
        };
        self.state.review_step_tracker.close_wave(&key);
        true
    }

    /// U4: When a review wave is incomplete past the synthesizer aggregate window,
    /// route `review-synthesizer` via `task.resume` so the loop can emit
    /// `plan.blocked` instead of stalling indefinitely.
    pub fn inject_review_aggregate_timeouts(&mut self) -> bool {
        use std::time::Duration;

        let timeout_secs = self
            .registry
            .get_config(&HatId::new("review-synthesizer"))
            .and_then(|cfg| cfg.aggregate.as_ref())
            .map(|agg| u64::from(agg.timeout))
            .unwrap_or(300);
        let timeout = Duration::from_secs(timeout_secs);

        let actions = self
            .state
            .review_step_tracker
            .drain_expired_aggregate_timeouts(timeout);
        let Some(action) = actions.into_iter().next() else {
            return false;
        };

        let payload = format!(
            "RECOVERY (AGGREGATE TIMEOUT): review wave '{}' received {}/{} \
             review.dimension.done events within {}s. Activate review-synthesizer and emit \
             review.passed with skip_reason=aggregate_timeout (or review.failed if verdict \
             is fail). Do NOT emit plan.complete or queue.advance until synthesizer terminal.\n\
             plan_name={} task_id={} step={} wave_id={}",
            action.wave_id,
            action.received,
            action.expected,
            timeout_secs,
            action.plan_name,
            action.task_id,
            action.step,
            action.wave_id,
        );
        let target = HatId::new("review-synthesizer");
        debug!(
            wave_id = %action.wave_id,
            received = action.received,
            expected = action.expected,
            "Injecting aggregate timeout recovery to review-synthesizer"
        );
        // R1 (2026-06-14-003 plan): pin the wave_id so the next
        // `build_prompt` for `review-synthesizer` injects
        // `AGGREGATE_TIMEOUT: true` in the `## WAVE CONTEXT` block.
        // The pin is consumed (`.take()`) on first read — the
        // aggregate-timeout signal does not leak across waves.
        // See `LoopState::pending_synthesizer_timeout` for the
        // full rationale.
        self.state.pending_synthesizer_timeout = Some(action.wave_id.clone());

        // Unit 7 (2026-06-17-001): wave merge complete — register handoff
        // obligation for the synthesizer so HandoffTracker can detect if it
        // fails to activate within the configured aggregate timeout.
        let handoff_event_id = format!(
            "sla:review.dimension.done:{}",
            action.wave_id
        );
        self.state.handoff_tracker.on_handoff_accepted(
            "review.dimension.done",
            "review-synthesizer",
            handoff_event_id,
            std::time::Instant::now(),
        );

        self.bus
            .publish(Event::new("task.resume", payload).with_target(target));
        true
    }

    /// Unit 8 (2026-06-17-001) + U3 (2026-06-17-003): Returns true if `hat`
    /// is `review-synthesizer` — the only consumer routed through the
    /// 3-step stall escalation ladder. `review-coordinator` and
    /// `dimension-reviewer` use their own `stall:<name>` bucket (U8
    /// invariant pinned by `test_u3_ladder_inert_for_non_wave_hats`).
    fn is_wave_hat(hat: &HatId) -> bool {
        hat.as_str() == "review-synthesizer"
    }

    /// Injects a fallback event to recover from a stalled loop.
    ///
    /// When no hats have pending events (agent failed to publish), this method
    /// injects a `task.resume` event which Ralph will handle to attempt recovery.
    ///
    /// Returns true if a fallback event was injected, false if recovery is not possible.
    pub fn inject_fallback_event(&mut self) -> bool {
        if self.inject_review_aggregate_timeouts() {
            return true;
        }

        const STALL_HARD_THRESHOLD: u32 = 3;
        // Unit 8 (2026-06-17-001): use a per-last-hat stall key so wave hats
        // accumulate their own retry budget separate from ralph's global counter.
        let stall_key = if let Some(last_hat) = &self.state.last_hat {
            if Self::is_wave_hat(last_hat) {
                format!("flow:review-synthesizer")
            } else {
                format!("stall:{}", last_hat.as_str())
            }
        } else {
            "stall:ralph".to_string()
        };

        let stall_count_value = *self
            .state
            .stall_recovery_counts
            .entry(stall_key.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);

        let hard_escalation = stall_count_value >= STALL_HARD_THRESHOLD;
        // Unit 8: wave stall escalation — route to review-coordinator when
        // a wave hat is the last to execute and it has stalled.
        let hard_target = if let Some(last_hat) = &self.state.last_hat {
            if Self::is_wave_hat(last_hat) {
                HatId::new("review-coordinator")
            } else {
                HatId::new("review-synthesizer")
            }
        } else {
            HatId::new("review-synthesizer")
        };

        // U3 (2026-06-17-003 plan) — Stall/handoff routing ladder
        // (R-F3, SC-F1): for wave hats, the 3rd consecutive stall
        // (hard_escalation == true) MUST escalate to the mechanism
        // layer (`maybe_emit_incomplete_wave_blocked`) instead of
        // routing to review-coordinator. The coordinator path was
        // what activated the `work.done → empty_diff` bypass in
        // zippy-sparrow (review-coordinator fired while a wave was
        // still open and tried to terminate with `review.passed`).
        // The ladder is:
        //   - count 1, 2: existing `task.resume` → review-synthesizer
        //     (lets the synthesizer try to close the wave normally)
        //   - count 3+: mechanism emits `plan.blocked` via U2
        //     staleness; we return early so no `task.resume` is
        //     published and no extra work is routed to executor.
        // Shares the `flow:review-synthesizer` bucket with 001-U8
        // (no double counter) — the existing threshold (3) is the
        // single source of truth.
        if hard_escalation && stall_key.starts_with("flow:") {
            if self.maybe_emit_incomplete_wave_blocked() {
                debug!(
                    stall_count = stall_count_value,
                    stall_key = %stall_key,
                    "U3: stall ladder reached hard threshold — mechanism emitted plan.blocked; \
                     NOT routing executor to re-emit work.done (empty_diff bypass closed)"
                );
                return true;
            }
            // If U2 had nothing to emit (no open waves / no
            // candidates), fall through to the legacy hard path
            // so the loop does not get stuck — this preserves the
            // pre-U3 behaviour for the edge case where the
            // stall counter has drifted past threshold but the
            // tracker has no open wave (e.g. ralph itself is the
            // last hat and `last_hat` is a wave hat from a prior
            // session).
        }

        // If a custom hat was last executing, target the fallback back to it
        // This preserves hat context instead of always falling back to Ralph
        let fallback_event = if hard_escalation {
            let reason_str = if stall_key.starts_with("flow:") {
                "wave_stall_exhausted"
            } else {
                "stall_no_events"
            };
            let mut payload = format!(
                "RECOVERY (HARD): {} consecutive stall iterations (key=`{}`). \
                 Route to `{}` to emit review terminal or re-dispatch wave.",
                stall_count_value,
                stall_key,
                hard_target.as_str()
            );
            payload.push_str(&Self::format_recovery_diagnosis_block(
                reason_str,
                hard_target.as_str(),
                "emit review.wave.ready, review.passed, or review.failed",
                stall_count_value,
                &[],
            ));
            debug!(
                stall_count = stall_count_value,
                target = %hard_target.as_str(),
                "Injecting HARD stall recovery to review hat"
            );
            Event::new("task.resume", payload).with_target(hard_target)
        } else {
            match &self.state.last_hat {
                Some(hat_id) if hat_id.as_str() != "ralph" => {
                    let publishes = self.get_hat_publishes(hat_id);
                    let mut payload = if publishes.is_empty() {
                        format!(
                            "RECOVERY: Previous iteration by hat `{}` did not publish an event. \
                         Emit exactly one valid next event via `ralph emit`, or stop only after \
                         publishing the configured completion event.",
                            hat_id.as_str()
                        )
                    } else {
                        format!(
                            "RECOVERY: Previous iteration by hat `{}` did not publish an event. \
                         This failed because no event was emitted. Emit exactly ONE valid next \
                         event via `ralph emit`. Allowed topics: `{}`. Do not only write prose \
                         or update files. Stop immediately after emitting.\n\n\
                         If you attempted to emit an event in the previous turn but it was not \
                         recorded, you must use the bash tool to execute `ralph emit` — \
                         prose mentions are not sufficient.",
                            hat_id.as_str(),
                            publishes.join("`, `")
                        )
                    };

                    // U4: enrich the task.resume payload with a structured
                    // "## Recovery Diagnosis" block so the agent can act on
                    // the failure reason, not just the prose recovery hint.
                    payload.push_str(&Self::format_recovery_diagnosis_block(
                        "stall_no_events",
                        hat_id.as_str(),
                        "emit a regular event",
                        0,
                        &[],
                    ));

                    debug!(
                        hat = %hat_id.as_str(),
                        "Injecting fallback event to recover - targeting last hat with task.resume"
                    );
                    Event::new("task.resume", payload).with_target(hat_id.clone())
                }
                _ => {
                    let mut payload = String::from(
                        "RECOVERY: Previous iteration did not publish an event. \
                     Review the scratchpad and either dispatch the next task or complete the loop.",
                    );
                    // U4: enrich the Ralph fallback payload with a structured
                    // "## Recovery Diagnosis" block.
                    payload.push_str(&Self::format_recovery_diagnosis_block(
                        "stall_no_events",
                        "ralph",
                        "emit a regular event",
                        0,
                        &[],
                    ));
                    debug!(
                        "Injecting fallback event to recover - triggering Ralph with task.resume"
                    );
                    Event::new("task.resume", payload)
                }
            }
        };

        self.bus.publish(fallback_event);
        true
    }

    /// Build the "## Recovery Diagnosis" appendix used by U4-enriched
    /// `task.resume` payloads. The block is a short, machine-greppable
    /// list of `key: value` lines that downstream tooling (and the
    /// agent itself) can rely on.
    pub fn format_recovery_diagnosis_block(
        reason: &str,
        target: &str,
        expected_action: &str,
        retry_attempt: u32,
        evidence_paths: &[String],
    ) -> String {
        let evidence = if evidence_paths.is_empty() {
            "(none)".to_string()
        } else {
            evidence_paths.join(", ")
        };
        format!(
            "\n\n## Recovery Diagnosis\n- reason: {reason}\n- target: {target}\n- expected action: {expected_action}\n- retry attempt: {retry_attempt}\n- evidence: {evidence}\n"
        )
    }

    /// Write a U4 recovery envelope + audit event for a workflow guard
    /// rejection. The rejected event is NOT re-published — the helper
    /// only records the diagnosis. `safe_target` is `false` because
    /// workflow guard rejections do not have a registered retry target
    /// (the agent has to fix the phase order, not a specific hat).
    fn log_workflow_guard_rejection(
        event_loop: &mut EventLoop,
        rejection: &WorkflowGuardRejectionDetail,
    ) {
        let reason_code = if rejection.current_phase.is_none() {
            "workflow_correlation_extraction_failed"
        } else {
            "out_of_order_phase"
        };
        let target_hat = rejection
            .source_hat
            .clone()
            .or_else(|| Some(rejection.chain_name.clone()));
        let safe_target = false;
        let mut builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::WorkflowGuard)
            .severity(crate::diagnosis::DiagnosisSeverity::Warning)
            .iteration(event_loop.state().iteration)
            .topic(rejection.rejected_topic.clone());
        if let Some(hat) = rejection.source_hat.as_deref() {
            builder = builder.source_hat(hat);
        }
        builder = builder
            .reason_code(reason_code)
            .message(rejection.reason.clone())
            .expected_action(format!(
                "Wait for the correct phase before emitting '{}'. Next expected topic: {}",
                rejection.rejected_topic, rejection.next_expected
            ))
            .safe_target(safe_target)
            .outcome(crate::diagnosis::DiagnosisOutcome::Pending)
            .evidence(crate::diagnosis::EvidenceRef {
                kind: crate::diagnosis::EvidenceKind::Topic,
                ref_path: rejection.next_expected.clone(),
                snippet: None,
            })
            .retry_key(
                crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                    crate::diagnosis::DiagnosisSource::WorkflowGuard,
                    target_hat.as_deref(),
                    Some(rejection.rejected_topic.as_str()),
                    reason_code,
                    None,
                ),
            );
        if let Some(target) = target_hat.as_deref() {
            builder = builder.target_hat(target);
        }
        if let Some(session_id) = event_loop.diagnostics().session_id() {
            builder = builder.session_id(session_id);
        }
        let envelope = builder.build();
        // U6: workflow-guard rejections also flow through
        // `record_recovery_envelope` so the responder can surface
        // them in the next prompt. The original U3 journal + audit
        // logging is preserved by the helper.
        event_loop.record_recovery_envelope(
            &envelope,
            vec![format!(
                "chain={} instance={} next_expected={}",
                rejection.chain_name,
                rejection.instance_key.as_deref().unwrap_or("global"),
                rejection.next_expected
            )],
        );
    }

    /// Write a U5/R9 recovery envelope + audit event for a topic-format
    /// rejection. The rejected event is NOT re-published — the helper
    /// only records the diagnosis.
    ///
    /// `safe_target` is `false` because topic-format rejections are
    /// non-actionable by retry: the offending topic is fixed at the
    /// preset/agent-config level, not by re-emitting. The outcome is
    /// `NotRetriable` so the responder does not synthesize a fake
    /// `task.resume` and the journal entry sticks around for `ralph
    /// diagnose` to surface to operators.
    ///
    /// R10 plan commitment: "non-retryable, only write recovery signal".
    /// Before this helper, the topic-format rejection path published an
    /// `event.topic_format.rejected` diagnostic but never wrote the
    /// journal entry — i.e. silently dropped from the recovery stream.
    fn log_topic_format_rejection(
        event_loop: &mut EventLoop,
        rejected_topic: &str,
        source_hat: Option<&str>,
        allowed_topics: &[String],
    ) {
        const REASON_CODE: &str = "invalid_topic_format";
        let safe_target = false;
        let allowed_preview = if allowed_topics.is_empty() {
            "(none)".to_string()
        } else if allowed_topics.len() <= 8 {
            allowed_topics.join(", ")
        } else {
            format!(
                "{} (+{} more)",
                allowed_topics
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
                allowed_topics.len() - 8
            )
        };
        let message = format!(
            "Topic '{}' is not in the whitelist of known topics (allowed: {})",
            rejected_topic, allowed_preview
        );
        let expected_action = format!(
            "Update the preset/hat config so '{}' is declared as a hat publish \
             (or trigger) topic, or remove the source that emits it. \
             This rejection is non-retryable and will not re-fire task.resume.",
            rejected_topic
        );
        let mut builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::TopicFormat)
            .severity(crate::diagnosis::DiagnosisSeverity::Warning)
            .iteration(event_loop.state().iteration)
            .topic(rejected_topic.to_string())
            .reason_code(REASON_CODE)
            .message(message.clone())
            .expected_action(expected_action)
            .safe_target(safe_target)
            .outcome(crate::diagnosis::DiagnosisOutcome::NotRetriable)
            .evidence(crate::diagnosis::EvidenceRef {
                kind: crate::diagnosis::EvidenceKind::Topic,
                ref_path: rejected_topic.to_string(),
                snippet: Some(format!("allowed_count={}", allowed_topics.len())),
            })
            .retry_key(
                crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                    crate::diagnosis::DiagnosisSource::TopicFormat,
                    source_hat,
                    Some(rejected_topic),
                    REASON_CODE,
                    None,
                ),
            );
        if let Some(hat) = source_hat {
            builder = builder.source_hat(hat);
        }
        if let Some(session_id) = event_loop.diagnostics().session_id() {
            builder = builder.session_id(session_id);
        }
        let envelope = builder.build();
        // Recovery journal + orchestration audit go through the same
        // U3/U6 pipeline as every other rejection. We swallow I/O
        // errors here: `record_recovery_envelope` already logs a warn
        // on failure and updating the responder must never block the
        // main loop.
        event_loop.record_recovery_envelope(&envelope, vec![message]);
    }

    /// U1 (2026-06-13-001): log a recovery envelope when the event policy
    /// rejected every wave event in a single read batch. This is the
    /// "wave dispatch blocked" signal that lets the runner skip the
    /// `missing_event_gate` (the agent DID try to emit) and that gives
    /// `ralph diagnose` a concrete `payload_contract` reason instead of
    /// a silent zero-fan-out.
    ///
    /// - `source` is `DiagnosisSource::PayloadContract` (KTD-3) — the
    ///   preset payload contract already covers required-field gaps.
    /// - `reason_code` is `wave_dispatch_blocked` for a generic batch
    ///   rejection, or `missing_required_field` when the first
    ///   rejection's violation type is `MissingRequiredField { .. }`.
    /// - `evidence` carries the topic, the raw wave count, and the
    ///   source hat (if any).
    fn log_wave_policy_blocked_envelope(
        event_loop: &mut EventLoop,
        rejections: &[crate::event_policy::PolicyRejection],
        raw_count: usize,
    ) {
        use crate::diagnosis::{DiagnosisSeverity, DiagnosisSource};
        use crate::event_policy::ViolationType;

        // Drive reason_code / message off the first rejection when any
        // exist; otherwise fall back to a generic "wave_dispatch_blocked"
        // — this covers the Hold-only case where the policy validator
        // dropped events without producing a PolicyRejection row.
        let (reason_code, topic, source_hat, first_message): (&str, String, Option<String>, String) =
            match rejections.first() {
                Some(r) => {
                    let is_missing_field = matches!(
                        r.finding.violation_type,
                        ViolationType::MissingRequiredField { .. }
                    );
                    let code: &'static str = if is_missing_field {
                        "missing_required_field"
                    } else {
                        "wave_dispatch_blocked"
                    };
                    (
                        code,
                        r.topic.clone(),
                        r.source_hat.clone(),
                        r.finding.message.clone(),
                    )
                }
                None => (
                    "wave_dispatch_blocked",
                    "<unknown>".to_string(),
                    None,
                    "all wave events were dropped by event policy (no rejection row produced; likely Hold decisions)".to_string(),
                ),
            };
        let message = format!(
            "Wave dispatch blocked: all {} wave events were dropped by event policy. \
             First finding: {}",
            raw_count, first_message
        );
        let expected_action = format!(
            "Re-emit the wave with the corrected payload schema. The required fields for '{}' \
             are defined in the preset's event_policy.schemas block.",
            topic
        );
        let safe_target = true;

        let mut builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::PayloadContract)
            .severity(DiagnosisSeverity::Error)
            .iteration(event_loop.state().iteration)
            .topic(topic.clone())
            .reason_code(reason_code)
            .message(message.clone())
            .expected_action(expected_action)
            .safe_target(safe_target)
            .evidence(crate::diagnosis::EvidenceRef {
                kind: crate::diagnosis::EvidenceKind::Topic,
                ref_path: topic.clone(),
                snippet: Some(format!(
                    "raw_count={} rejected_count={}",
                    raw_count,
                    rejections.len()
                )),
            });

        // The `retry_key_from_parts` helper produces a stable
        // aggregation key based on (source, target_hat, topic,
        // reason_code, field). A follow-up emit with the same
        // corrected payload will dedupe against this envelope in
        // `ralph diagnose`.
        let wave_id_field: Option<&str> = None;
        if let Some(hat) = source_hat.as_deref() {
            builder = builder.source_hat(hat);
        }
        let retry_key = crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
            DiagnosisSource::PayloadContract,
            source_hat.as_deref(),
            Some(topic.as_str()),
            &reason_code,
            wave_id_field,
        );
        builder = builder.retry_key(retry_key);

        if let Some(session_id) = event_loop.diagnostics().session_id() {
            builder = builder.session_id(session_id);
        }
        let envelope = builder.build();

        warn!(
            topic = %topic,
            raw_count = raw_count,
            rejection_count = rejections.len(),
            reason_code = %reason_code,
            "Wave dispatch blocked by event policy: all wave events rejected"
        );

        event_loop.record_recovery_envelope(&envelope, vec![message]);
    }

    /// Builds the prompt for a hat's execution.
    ///
    /// Per "Hatless Ralph" architecture:
    /// - Solo mode: Ralph handles everything with his own prompt
    /// - Multi-hat mode: Ralph is the sole executor, custom hats define topology only
    ///
    /// When in multi-hat mode, this method collects ALL pending events across all hats
    /// and builds Ralph's prompt with that context. The `## HATS` section in Ralph's
    /// prompt documents the topology for coordination awareness.
    ///
    /// If memories are configured with `inject: auto`, this method also prepends
    /// primed memories to the prompt context. If a scratchpad file exists and is
    /// non-empty, its content is also prepended (before memories).
    pub fn build_prompt(&mut self, hat_id: &HatId) -> Option<String> {
        // 2026-06-13-004 U8 (P1-2): clear any pending handoff
        // deadlines for this hat. The hat is now actually
        // *building* a prompt — about to invoke the LLM — so
        // the deadline race that produced the 17m / 4m false
        // handoff timeouts in the 2026-06-13 incident is over.
        // KTD-6 explicitly forbids moving this clear to
        // `process_output` (L4223 `current_isolated_hat`): that
        // site records the *completed* hat, not the *about-to-
        // activate* hat. The build_prompt entry point is the
        // earliest moment the hat is unambiguously "live".
        // Safe in coordinator mode too — `on_hat_activated`
        // is a no-op when the tracker's `pending` map is empty
        // (and in coordinator mode the tracker is always empty).
        //
        // 2026-06-13 review fix (reliability F2): the "ralph"
        // hat is the constant coordinator sentinel, never a
        // handoff *consumer* — passing it through here would
        // spuriously clear real consumer pending entries whose
        // hat_id happens to match (or be a prefix of) "ralph".
        // Skip the clear for ralph; downstream ralph prompt
        // building still proceeds normally below.
        if hat_id.as_str() != "ralph" {
            self.state.handoff_tracker.on_hat_activated(hat_id.as_str());
        }
        // Handle "ralph" hat - the constant coordinator
        // Per spec: "Hatless Ralph is constant — Cannot be replaced, overwritten, or configured away"
        if hat_id.as_str() == "ralph" {
            if self.config.hats.is_empty() {
                // Solo mode - just Ralph's events, no hats to filter
                let mut events = self.bus.take_pending(&hat_id.clone());
                let mut human_events = self.bus.take_human_pending();
                events.append(&mut human_events);

                // Separate human.guidance events from regular events
                let (guidance_events, regular_events): (Vec<_>, Vec<_>) = events
                    .into_iter()
                    .partition(|e| e.topic.as_str() == "human.guidance");

                let events_context = regular_events
                    .iter()
                    .map(|e| Self::format_event(e))
                    .collect::<Vec<_>>()
                    .join("\n");

                // Solo mode: set scratchpad and iteration before guidance persistence
                self.ralph
                    .set_active_scratchpad(self.config.core.scratchpad.clone());
                self.ralph.set_iteration(self.state.iteration);

                // Unit 3 (2026-06-16-002 plan): during the
                // coordinator bootstrap window we MUST NOT inject
                // human guidance into the prompt — the agent's
                // first action should be the legal bootstrap
                // handoff, not a response to stale human input.
                // The gate fires for `hat_id == "coordinator"`
                // and `in_bootstrap_phase() == true`; in solo mode
                // `hat_id == "ralph"` so the guard is a no-op
                // (kept here for symmetry with the multi-hat /
                // isolated paths and as a safety net).
                if self.coordinator_bootstrap_gate_closed(hat_id) {
                    // Bootstrap window: drop pending guidance events
                    // (they are still on the bus and will be
                    // redelivered on the next iteration once
                    // `bootstrap_complete` flips to `true`).
                    drop(guidance_events);
                } else {
                    // Persist and inject human guidance into prompt if present
                    self.update_robot_guidance(guidance_events);
                    self.apply_robot_guidance();
                }

                // Build base prompt and prepend memories + scratchpad + ready tasks
                let base_prompt = self.ralph.build_prompt(&events_context, &[], &[]);
                self.ralph.clear_robot_guidance();
                let base_prompt = self.inject_phase_into_prompt(base_prompt);
                // U6: fold the soft runtime-diagnosis alert into the
                // prompt before skills prepending. The order
                // (phase → diagnosis alert → skills) is fixed by the
                // U6 plan so the skills index is never broken by the
                // alert text.
                let base_prompt = self.apply_runtime_diagnosis_prompt(base_prompt, hat_id);
                let with_skills = self.prepend_auto_inject_skills(base_prompt);
                let with_scratchpad = self.prepend_scratchpad(with_skills, Some(hat_id));
                let with_state_files = self.prepend_state_files(with_scratchpad);
                let final_prompt = self.prepend_ready_tasks(with_state_files);

                debug!("build_prompt: routing to HatlessRalph (solo mode)");
                return Some(final_prompt);
            } else {
                // Multi-hat mode: collect events and determine active hats
                let mut all_hat_ids: Vec<HatId> = self.bus.hat_ids().cloned().collect();
                // Deterministic ordering (avoid HashMap iteration order nondeterminism).
                all_hat_ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));

                let mut all_events = Vec::new();
                let mut system_events = Vec::new();

                for id in &all_hat_ids {
                    let pending = self.bus.take_pending(id);
                    if pending.is_empty() {
                        continue;
                    }

                    let (drop_pending, exhausted_event) = self.check_hat_exhaustion(id, &pending);
                    if drop_pending {
                        // Drop the pending events that would have activated the hat.
                        if let Some(exhausted_event) = exhausted_event {
                            all_events.push(exhausted_event.clone());
                            system_events.push(exhausted_event);
                        }
                        continue;
                    }

                    all_events.extend(pending);
                }

                let mut human_events = self.bus.take_human_pending();
                all_events.append(&mut human_events);

                // Publish orchestrator-generated system events after consuming pending events,
                // so they become visible in the event log and can be handled next iteration.
                for event in system_events {
                    self.bus.publish(event);
                }

                // Separate human.guidance events from regular events
                let (guidance_events, regular_events): (Vec<_>, Vec<_>) = all_events
                    .into_iter()
                    .partition(|e| e.topic.as_str() == "human.guidance");

                // Ignore kickoff/recovery noise when a real downstream event is pending.
                let effective_regular_events = self.effective_regular_events(&regular_events);

                // Determine which hats are active based on regular events
                let active_hat_ids = self.determine_active_hat_ids(&regular_events);
                self.record_hat_activations(&active_hat_ids);
                self.state.last_active_hat_ids = active_hat_ids.clone();

                // U3: Record activation lifecycle for each active hat.
                // For each hat activation, create an ActivationKey and activate the tracker.
                // The trigger topic is the first regular event whose topic matches
                // one of this hat's configured `triggers`. This must be derived from
                // the hat's subscription (NOT `can_publish` — trigger events are hat
                // *inputs*, not publishes; using `can_publish` caused the activate
                // side to fall through to the "unknown" fallback in production —
                // P0 code-review finding #1).
                for hat_id in &active_hat_ids {
                    let trigger_topic = self
                        .registry
                        .get_config(hat_id)
                        .map(|config| {
                            let trigger_topics = config.trigger_topics();
                            effective_regular_events
                                .iter()
                                .find(|e| {
                                    trigger_topics
                                        .iter()
                                        .any(|t| t.matches_str(e.topic.as_str()))
                                })
                                .map(|e| e.topic.to_string())
                                .unwrap_or_else(|| "unknown".to_string())
                        })
                        .unwrap_or_else(|| "unknown".to_string());
                    let key = ActivationKey {
                        loop_id: self
                            .loop_context
                            .as_ref()
                            .and_then(|ctx| ctx.loop_id())
                            .unwrap_or("primary")
                            .to_string(),
                        iteration: self.state.iteration,
                        hat_id: hat_id.as_str().to_string(),
                    };
                    self.hat_lifecycle_tracker.activate(
                        key,
                        trigger_topic,
                        None, // linked_task_id resolved later if available
                    );
                }
                self.state.last_activation_events =
                    effective_regular_events.iter().copied().cloned().collect();

                // Resolve scratchpad config for the active hat (or global default).
                // Must happen BEFORE guidance persistence so guidance is written
                // to the correct hat's scratchpad file.
                let resolved_scratchpad = if let Some(hat_id) = active_hat_ids.first() {
                    let hat_scratchpad = self
                        .registry
                        .get_config(hat_id)
                        .and_then(|c| c.scratchpad.as_ref());
                    ScratchpadConfig::resolve(hat_scratchpad, &self.config.core.scratchpad)
                } else {
                    // Ralph coordinating — use global
                    self.config.core.scratchpad.clone()
                };
                self.ralph.set_active_scratchpad(resolved_scratchpad);
                self.ralph.set_iteration(self.state.iteration);

                // Unit 3 (2026-06-16-002 plan): in multi-hat mode
                // `hat_id == "ralph"` (we are in this branch
                // because the ralph hat requested a prompt), so
                // the `coordinator_bootstrap_gate_closed` check
                // is a no-op.  Still, keep the guard for parity
                // with the isolated path — a future preset that
                // routes the multi-hat path through a hat named
                // "coordinator" will inherit the bootstrap
                // suppression automatically.
                if self.coordinator_bootstrap_gate_closed(hat_id) {
                    drop(guidance_events);
                } else {
                    // Persist and inject human guidance after scratchpad resolution
                    // (must also happen before immutable borrows from determine_active_hats)
                    self.update_robot_guidance(guidance_events);
                    self.apply_robot_guidance();
                }

                let active_hats = self.determine_active_hats(&regular_events);

                // FR-1: Hat-level event allowlist filtering.
                // If every active hat has an enabled allowlist, compute the union
                // of their configured events plus their triggers. Otherwise,
                // disable filtering for this iteration.
                let mut should_filter = true;
                let mut union_allowlist = std::collections::HashSet::new();
                for hat in &active_hats {
                    if let Some(config) = self.registry.get_config(&hat.id)
                        && let Some(ref filter) = config.event_filter
                        && filter.enabled
                    {
                        union_allowlist.extend(filter.events.iter().cloned());
                        union_allowlist.extend(config.triggers.iter().cloned());
                        continue;
                    }
                    // Fallback-only hats (e.g., builtin ralph with `*` subscription)
                    // have no config and should not disable filtering.
                    if hat.is_fallback_only() {
                        continue;
                    }
                    should_filter = false;
                    break;
                }

                let filtered_events: Vec<&Event> = if should_filter && !union_allowlist.is_empty() {
                    effective_regular_events
                        .into_iter()
                        .filter(|e| union_allowlist.contains(e.topic.as_str()))
                        .collect()
                } else {
                    effective_regular_events
                };

                // Extract trigger topic(s) for the active hats so they appear in the
                // prompt as `## ACTIVE TRIGGER`. Derive from `filtered_events` (the
                // FR-1-filtered subset) — not `regular_events` — so that the trigger
                // list stays consistent with what the prompt's PENDING EVENTS section
                // actually shows, avoiding re-injection of filtered-out events.
                let trigger_topics: Vec<String> = filtered_events
                    .iter()
                    .filter(|e| !Self::is_system_event(e.topic.as_str()))
                    .map(|e| e.topic.to_string())
                    .collect();

                // Format events for context
                let events_context = filtered_events
                    .iter()
                    .map(|e| Self::format_event(e))
                    .collect::<Vec<_>>()
                    .join("\n");

                // Build base prompt and prepend memories + scratchpad if available
                let base_prompt = self.ralph.build_prompt(
                    &events_context,
                    &active_hats,
                    &trigger_topics
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                );

                // Build prompt with active hats - filters instructions to only active hats
                debug!(
                    "build_prompt: routing to HatlessRalph (multi-hat coordinator mode), active_hats: {:?}",
                    active_hats
                        .iter()
                        .map(|h| h.id.as_str())
                        .collect::<Vec<_>>()
                );

                // Clear guidance after active_hats references are no longer needed
                self.ralph.clear_robot_guidance();
                let base_prompt = self.inject_phase_into_prompt(base_prompt);
                // U6: see solo-mode comment above. Coordinator
                // path passes `hat_id` (the ralph hat) so the
                // helper sees the full set of findings — the
                // coordinator sees every hat's alerts.
                let base_prompt = self.apply_runtime_diagnosis_prompt(base_prompt, hat_id);
                let with_skills = self.prepend_auto_inject_skills(base_prompt);
                let with_scratchpad = self.prepend_scratchpad(with_skills, Some(hat_id));
                let with_state_files = self.prepend_state_files(with_scratchpad);
                let final_prompt = self.prepend_ready_tasks(with_state_files);

                return Some(final_prompt);
            }
        }

        // Non-ralph hat requested
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated {
            // Isolated mode: build focused prompt for this hat only
            let mut events = self.bus.take_pending(&hat_id.clone());
            let mut human_events = self.bus.take_human_pending();
            events.append(&mut human_events);

            let (guidance_events, regular_events): (Vec<_>, Vec<_>) = events
                .into_iter()
                .partition(|e| e.topic.as_str() == "human.guidance");

            // Apply per-hat event filter if configured
            let hat_config = self.registry.get_config(hat_id);
            let mut allowlist = std::collections::HashSet::new();
            let should_filter = if let Some(config) = hat_config
                && let Some(ref filter) = config.event_filter
                && filter.enabled
            {
                allowlist.extend(filter.events.iter().cloned());
                allowlist.extend(config.triggers.iter().cloned());
                !allowlist.is_empty()
            } else {
                false
            };

            let filtered_events: Vec<&Event> = if should_filter {
                regular_events
                    .iter()
                    .filter(|e| allowlist.contains(e.topic.as_str()))
                    .collect()
            } else {
                regular_events.iter().collect()
            };

            let events_context = filtered_events
                .iter()
                .map(|e| Self::format_event(e))
                .collect::<Vec<_>>()
                .join("\n");

            // Resolve scratchpad for this hat
            let resolved_scratchpad = self
                .registry
                .get_config(hat_id)
                .and_then(|c| c.scratchpad.as_ref())
                .map(|s| ScratchpadConfig::resolve(Some(s), &self.config.core.scratchpad))
                .unwrap_or_else(|| self.config.core.scratchpad.clone());
            self.ralph.set_active_scratchpad(resolved_scratchpad);
            self.ralph.set_iteration(self.state.iteration);

            // Unit 3 (2026-06-16-002 plan): the isolated path is
            // the **only** path where the gate can actually fire
            // (the active `hat_id` is a real hat, not the
            // constant `ralph` sentinel).  When the active hat
            // is the `coordinator` and the loop is still in
            // bootstrap, drop the pending `human.guidance` events
            // and skip both `update_robot_guidance` /
            // `apply_robot_guidance` AND the
            // `collect_robot_guidance` block below — none of the
            // cached guidance should reach the coordinator's
            // first prompt.
            let skip_guidance = self.coordinator_bootstrap_gate_closed(hat_id);
            if !skip_guidance {
                // Handle guidance
                self.update_robot_guidance(guidance_events);
                self.apply_robot_guidance();
            } else {
                drop(guidance_events);
            }

            // Build base prompt
            let hat = self.registry.get(hat_id)?;

            // Debug logging to trace hat routing
            debug!(
                "build_prompt: hat_id='{}', instructions.is_empty()={}",
                hat_id.as_str(),
                hat.instructions.is_empty()
            );

            debug!(
                "build_prompt: routing to build_custom_hat() for '{}' (isolated mode)",
                hat_id.as_str()
            );

            let base_prompt = self
                .instruction_builder
                .build_custom_hat(hat, &events_context);

            // Inject the cached `human.guidance` text as a `## ROBOT GUIDANCE`
            // block so isolated hats (whose `build_custom_hat` template does
            // not read `ralph.robot_guidance` on its own) still see the
            // guidance that was just persisted to the scratchpad. We must
            // call this BEFORE `clear_robot_guidance()` below, otherwise the
            // in-memory copy is gone.
            //
            // Unit 3 (2026-06-16-002 plan): when the gate is
            // closed we did NOT call `update_robot_guidance` /
            // `apply_robot_guidance` above, so the in-memory
            // guidance cache is empty; `collect_robot_guidance`
            // returns an empty string and the conditional below
            // leaves `base_prompt` unchanged.  We still call
            // the helper for symmetry / future-proofing.
            let guidance_section = self.ralph.collect_robot_guidance();
            let base_prompt = if guidance_section.is_empty() {
                base_prompt
            } else {
                format!("{guidance_section}{base_prompt}")
            };

            // Apply prepend pipeline (SAME order as coordinator path)
            self.ralph.clear_robot_guidance();
            // R1: `## WAVE CONTEXT` block lives at the very top of the
            // prompt for `review-synthesizer` so the agent cannot miss
            // it.  The block is a no-op for any other hat.
            let base_prompt = self.prepend_wave_context(base_prompt, hat_id);
            // R3: surface ephemeral relocations so the agent stops
            // recreating runtime artefacts inside the source tree.
            let base_prompt = self.prepend_ephemeral_relocations(base_prompt);
            let base_prompt = self.inject_phase_into_prompt(base_prompt);
            // U6: in isolated mode the helper filters findings to
            // those whose target/source hat matches `hat_id`. The
            // plan's "isolated hat mode 下 alert 只注入目标 hat"
            // contract is enforced inside `apply_runtime_diagnosis_prompt`.
            let base_prompt = self.apply_runtime_diagnosis_prompt(base_prompt, hat_id);
            let with_skills = self.prepend_auto_inject_skills(base_prompt);
            let with_scratchpad = self.prepend_scratchpad(with_skills, Some(hat_id));
            let with_state_files = self.prepend_state_files(with_scratchpad);
            let final_prompt = self.prepend_ready_tasks(with_state_files);

            // Set active hat for downstream logic (default_publishes, enforce_hat_scope)
            self.state.last_active_hat_ids = vec![hat_id.clone()];

            return Some(final_prompt);
        }

        // Backward compatibility / non-isolated mode: simple custom hat prompt
        let events = self.bus.take_pending(&hat_id.clone());
        let events_context = events
            .iter()
            .map(|e| Self::format_event(e))
            .collect::<Vec<_>>()
            .join("\n");

        let hat = self.registry.get(hat_id)?;

        // Debug logging to trace hat routing
        debug!(
            "build_prompt: hat_id='{}', instructions.is_empty()={}",
            hat_id.as_str(),
            hat.instructions.is_empty()
        );

        // All hats use build_custom_hat with ghuntley-style prompts
        debug!(
            "build_prompt: routing to build_custom_hat() for '{}'",
            hat_id.as_str()
        );
        // U6: in the backward-compat custom-hat path there is no
        // isolated-mode filtering (the path is reached only when
        // execution_mode != Isolated), so we always pass the full
        // hat_id; the responder injects every finding whose hat
        // matches or has no hat binding.
        let base = self
            .instruction_builder
            .build_custom_hat(hat, &events_context);
        let with_phase = self.inject_phase_into_prompt(base);
        let with_diagnosis = self.apply_runtime_diagnosis_prompt(with_phase, hat_id);
        // We intentionally skip `prepend_auto_inject_skills` here
        // because the backward-compat custom-hat path predates
        // that pipeline and tests assert the absence of skill
        // injection for this branch.
        let _ = RUNTIME_DIAGNOSIS_ALERT_HEADER; // silence unused-import lint
        Some(with_diagnosis)
    }

    /// Inspect a batch of policy-accepted events and flip the
    /// `bootstrap_complete` / `bootstrap_failed` flags when the
    /// coordinator produces a terminal bootstrap handoff.
    ///
    /// Unit 3 (2026-06-16-002 plan) contract:
    /// - `coordinator` `work.ready` **without** a
    ///   `reviewed_task_id` field is the bootstrap handoff. It
    ///   marks `bootstrap_complete = true`.
    /// - `coordinator` `work.failed` is the explicit bootstrap
    ///   failure. It marks `bootstrap_failed = true` so the
    ///   runner can surface a precise reason rather than hang on
    ///   a missing `work.ready`.
    /// - Plan-gate `work.ready` (carrying `reviewed_task_id`) is
    ///   NOT a bootstrap event; the flag stays `false` so
    ///   step-advance handoffs from `review-synthesizer` keep
    ///   behaving as today.
    ///
    /// Both flags are reset to `false` in `initialize_with_topic`
    /// so a fresh `work.start` starts a new bootstrap window.
    /// Detection runs in the *accept* path so a rejected
    /// `work.ready` (e.g. payload contract violation) does NOT
    /// promote the flag — only events the runner actually
    /// processes count.
    fn update_bootstrap_flags_from_accepted(&mut self, accepted: &[JsonlEvent]) {
        self.apply_bootstrap_flags_from_events(accepted);
    }

    /// Derive bootstrap gate state from a chronological event batch
    /// (accepted events or full events.jsonl replay on resume).
    fn apply_bootstrap_flags_from_events(&mut self, events: &[JsonlEvent]) {
        for event in events {
            let hat = event.hat.as_deref().unwrap_or("");
            if hat != "coordinator" {
                continue;
            }
            if event.topic == "work.ready" && !self.state.bootstrap_complete {
                let is_bootstrap = event
                    .payload
                    .as_deref()
                    .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
                    .and_then(|v| v.get("reviewed_task_id").cloned())
                    .is_none();
                if is_bootstrap {
                    self.state.bootstrap_complete = true;
                }
            } else if event.topic == "work.failed" && !self.state.bootstrap_failed {
                self.state.bootstrap_failed = true;
            }
        }
    }

    /// Rebuild bootstrap flags after `task.resume` by scanning the loop's
    /// events file so guidance suppression does not leak across resume.
    fn rebuild_bootstrap_flags_from_recorded_events(&mut self) {
        let path = self
            .loop_context
            .as_ref()
            .map(|ctx| ctx.events_path())
            .unwrap_or_else(|| self.event_reader.path().to_path_buf());
        if !path.exists() {
            return;
        }
        let mut reader = EventReader::new(&path);
        reader.reset();
        if let Ok(result) = reader.read_new_events() {
            self.state.bootstrap_complete = false;
            self.state.bootstrap_failed = false;
            self.apply_bootstrap_flags_from_events(&result.events);
        }
    }

    /// Stores guidance payloads, persists them to scratchpad, and prepares them for prompt injection.
    ///
    /// Guidance events are ephemeral in the event bus (consumed by `take_pending`).
    /// This method both caches them in memory for prompt injection and appends
    /// them to the scratchpad file so they survive across process restarts.
    fn update_robot_guidance(&mut self, guidance_events: Vec<Event>) {
        if guidance_events.is_empty() {
            return;
        }

        // Persist new guidance to scratchpad before caching
        self.persist_guidance_to_scratchpad(&guidance_events);

        // 2026-06-13-004 review fix (correctness F2, KTD-7 two-layer
        // dedup): the in-memory `robot_guidance` vec is the source
        // for the next `apply_robot_guidance` → prompt injection.
        // A redelivered or duplicated `human.guidance` event would
        // otherwise add the same payload twice to the prompt.
        // Dedup against the existing vec and within the current
        // batch; persist layer has already dedup'd against disk.
        let mut seen_in_batch: std::collections::HashSet<String> = std::collections::HashSet::new();
        for event in guidance_events {
            // Move the payload out so we can dedup by owned String
            // without fighting the borrow checker. `payload` is
            // moved into `robot_guidance` when it survives the
            // dedup check; otherwise dropped.
            let payload = event.payload;
            if seen_in_batch.insert(payload.clone()) {
                let already = self.robot_guidance.iter().any(|p| p == &payload);
                if !already {
                    self.robot_guidance.push(payload);
                } else {
                    debug!(
                        payload_len = payload.len(),
                        "U9 (KTD-7 in-memory layer): skipping guidance payload already cached for prompt"
                    );
                }
            } else {
                debug!(
                    payload_len = payload.len(),
                    "U9 (KTD-7 in-memory layer): skipping duplicate guidance payload in current batch"
                );
            }
        }
    }

    /// Appends human guidance entries to the scratchpad file for durability.
    ///
    /// Each guidance message is written as a timestamped markdown entry so it
    /// appears alongside the agent's own thinking and survives process restarts.
    ///
    /// When scratchpad is disabled for the current hat, persists to the global
    /// scratchpad path (guidance is cross-hat state). If global is also disabled,
    /// skips persistence.
    fn persist_guidance_to_scratchpad(&self, guidance_events: &[Event]) {
        use std::io::Write;

        // When hat scratchpad is disabled, fall back to global scratchpad
        let scratchpad_path = if self.ralph.active_scratchpad().enabled {
            self.scratchpad_path()
        } else {
            if !self.config.core.scratchpad.enabled {
                debug!("Both hat and global scratchpad disabled, skipping guidance persistence");
                return;
            }
            self.global_scratchpad_path()
        };
        let resolved_path = if scratchpad_path.is_relative() {
            self.config.core.workspace_root.join(&scratchpad_path)
        } else {
            scratchpad_path
        };

        // Create parent directories if needed
        if let Some(parent) = resolved_path.parent()
            && !parent.exists()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            warn!("Failed to create scratchpad directory: {}", e);
            return;
        }

        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved_path)
        {
            Ok(f) => f,
            Err(e) => {
                warn!("Failed to open scratchpad for guidance persistence: {}", e);
                return;
            }
        };

        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        // 2026-06-13-004 U9 (P1-4): de-duplicate guidance payloads
        // against the on-disk scratchpad tail. The 2026-06-13
        // incident saw `Focus on error handling` written twice and
        // `Keep this in mind` written three times because
        // `persist_guidance_to_scratchpad` unconditionally appended.
        //
        // 2026-06-13 review fixes:
        //   - correctness F1: replaced `skip_while`+`filter` with a
        //     proper state machine so lines from sections AFTER
        //     the last `### HUMAN GUIDANCE` block are NOT
        //     collected as "existing payloads" (a line of text in
        //     `## NOTES` would otherwise be matched as a duplicate
        //     against a new guidance event with the same text).
        //   - reliability F5: extracted the 16 KB window size to
        //     a named constant with a comment explaining the
        //     capacity budget.
        //   - maintainability #20 (P2): the window is byte-bounded
        //     via `split_at` on bytes (UTF-8 safe via the byte
        //     check before the split). Char-based slicing would
        //     inflate to 64 KB worst-case for 4-byte CJK.
        const GUIDANCE_DEDUP_TAIL_BYTES: usize = 16 * 1024;
        let existing_payloads: std::collections::HashSet<String> = if resolved_path.exists() {
            std::fs::read_to_string(&resolved_path)
                .ok()
                .map(|content| {
                    // Byte-bounded tail; snap to a char boundary so
                    // the resulting &str is valid UTF-8 (no panic
                    // when the cut falls inside a multi-byte char).
                    let start = content.len().saturating_sub(GUIDANCE_DEDUP_TAIL_BYTES);
                    let tail_start = crate::text::floor_char_boundary(&content, start);
                    let tail = &content[tail_start..];
                    // State machine: collect body lines only while
                    // inside a `### HUMAN GUIDANCE` block. Stop at
                    // the next `### ` or `## ` header (any new
                    // section marker ends the current guidance
                    // block; `## NOTES` is the most common offender
                    // that would otherwise leak into the dedup
                    // HashSet). The block also ends at end-of-file.
                    let mut in_guidance = false;
                    let mut payloads = std::collections::HashSet::new();
                    for line in tail.lines() {
                        if line.starts_with("### HUMAN GUIDANCE") {
                            in_guidance = true;
                            continue;
                        }
                        if in_guidance && (line.starts_with("### ") || line.starts_with("## ")) {
                            in_guidance = false;
                            continue;
                        }
                        if in_guidance && !line.is_empty() {
                            payloads.insert(line.trim().to_string());
                        }
                    }
                    payloads
                })
                .unwrap_or_default()
        } else {
            std::collections::HashSet::new()
        };
        // 2026-06-13-004 review fix (F2 KTD-7): also dedup within
        // the current batch so a single persist call with two
        // identical payloads (e.g. a redelivered `human.guidance`
        // event) only writes the first one.
        let mut seen_in_batch: std::collections::HashSet<String> = std::collections::HashSet::new();
        for event in guidance_events {
            let payload = event.payload.as_str();
            if payload.is_empty() {
                continue;
            }
            if existing_payloads.contains(payload) || !seen_in_batch.insert(payload.to_string()) {
                debug!(
                    payload_len = payload.len(),
                    "U9: skipping duplicate guidance payload (already in scratchpad or in this batch)"
                );
                continue;
            }
            let entry = format!(
                "\n### HUMAN GUIDANCE ({})\n\n{}\n",
                timestamp, event.payload
            );
            if let Err(e) = file.write_all(entry.as_bytes()) {
                warn!("Failed to write guidance to scratchpad: {}", e);
            }
        }

        info!(
            count = guidance_events.len(),
            "Persisted human guidance to scratchpad"
        );
    }

    /// Injects cached guidance into the next prompt build.
    fn apply_robot_guidance(&mut self) {
        if self.robot_guidance.is_empty() {
            return;
        }

        self.ralph.set_robot_guidance(self.robot_guidance.clone());
        // P1 finding #4 (test isolation): clear the EventLoop-level
        // cache after the ralph copy has been set, so a subsequent
        // build_prompt call for a different hat does NOT re-inject
        // the same guidance. Without this, the guidance would leak
        // to any hat whose build_prompt is called in the same loop
        // iteration, breaking R9. The scratchpad persistence path
        // is independent (it writes to disk) and unaffected.
        self.robot_guidance.clear();
    }

    /// Prepends auto-injected skill content to the prompt.
    ///
    /// Injects current phase information into the prompt if phase support is enabled.
    ///
    /// When `event_loop.phase_config` is configured, this appends a "## Current Phase"
    /// section so the agent knows which phase (warmup / production) the loop is in.
    fn inject_phase_into_prompt(&self, prompt: String) -> String {
        if self.config.event_loop.phase_config.is_none() {
            return prompt;
        }
        let phase = self.registry.current_phase();
        format!("{}\n## Current Phase\n\n{}\n", prompt, phase)
    }

    /// U6: Append a `## Runtime Diagnosis Alert` block to the prompt
    /// when the recovery responder has findings that the next agent
    /// should see.
    ///
    /// This helper is the single chokepoint for prompt-level
    /// diagnosis injection and is called from every `build_prompt`
    /// path (solo ralph, multi-hat coordinator, isolated hat,
    /// backward-compat custom hat). The injection order is fixed by
    /// the U6 plan: `inject_phase_into_prompt` → diagnosis alert →
    /// `prepend_auto_inject_skills`, so the skills index never gets
    /// split by the alert.
    ///
    /// Returns `prompt` unchanged when the responder has nothing to
    /// surface (no pending findings, prompt injection disabled, or
    /// runtime-diagnosis entirely off).
    fn apply_runtime_diagnosis_prompt(&self, prompt: String, hat_id: &HatId) -> String {
        if !self.config.telemetry.runtime_diagnosis.enabled
            || !self
                .config
                .telemetry
                .runtime_diagnosis
                .prompt_injection_enabled
        {
            return prompt;
        }
        if !self.recovery_responder.has_pending_findings() {
            return prompt;
        }
        let current_iteration = self.state.iteration;
        let hat_filter = if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && hat_id.as_str() != "ralph"
        {
            Some(hat_id)
        } else {
            None
        };
        self.recovery_responder
            .inject_prompt_alert(&prompt, hat_filter, current_iteration)
    }

    /// U6: Record a recovery envelope that the recovery responder
    /// should respond to. This is the single entry point that U4
    /// write paths use to feed the responder. The function
    ///
    /// 1. Writes the journal entry to `recovery.jsonl` (U3 behavior).
    /// 2. Emits the high-level audit event to `orchestration.jsonl`.
    /// 3. Updates the responder's in-memory state and computes the
    ///    escalation level for this iteration.
    ///
    /// The function never fails: I/O errors are swallowed (matching
    /// the existing U3 logger contract) and the responder is updated
    /// regardless so the in-memory state stays consistent.
    pub fn record_recovery_envelope(
        &mut self,
        envelope: &RecoveryDiagnosisEnvelope,
        notes: Vec<String>,
    ) -> crate::diagnosis::EscalationDecision {
        let hat = envelope
            .source_hat
            .as_deref()
            .unwrap_or(envelope.target_hat.as_deref().unwrap_or("ralph"));
        self.diagnostics
            .log_recovery(RecoveryJournalEntry::from_envelope(envelope.clone(), notes));
        self.diagnostics.log_orchestration(
            envelope.iteration,
            hat,
            OrchestrationEvent::from_recovery_envelope(envelope),
        );
        let current_iteration = envelope.iteration.max(self.state.iteration);
        self.recovery_responder
            .record_finding(envelope, current_iteration)
    }

    /// U6: Mark the next iteration as fresh. Clears the responder's
    /// per-iteration caches (`pending_findings`, hard-escalation
    /// queue, termination hint) so the prompt builder does not
    /// re-inject stale alerts.
    pub fn begin_diagnosis_iteration(&mut self) {
        self.recovery_responder.begin_iteration();
    }

    /// U6: Read-only access to the recovery responder. Useful for
    /// the loop runner when checking the most recent hard
    /// escalation or termination hint.
    pub fn recovery_responder(&self) -> &RecoveryResponder {
        &self.recovery_responder
    }

    /// U6: Mutable access to the recovery responder. Used by the
    /// loop runner to mark findings as recovered after each
    /// iteration.
    pub fn recovery_responder_mut(&mut self) -> &mut RecoveryResponder {
        &mut self.recovery_responder
    }

    /// This generalizes the former `prepend_memories()` into a skill auto-injection
    /// pipeline that handles memories, tools, and any other auto-inject skills.
    ///
    /// Injection order:
    /// 1. Memory data + ralph-tools skill (special case: loads memory data from store, applies budget)
    /// 2. RObot interaction skill (gated by `robot.enabled`)
    /// 3. Other auto-inject skills from the registry (wrapped in XML tags)
    fn prepend_auto_inject_skills(&self, prompt: String) -> String {
        let mut prefix = String::new();

        // 1. Memory data + ralph-tools skill — special case with data loading
        self.inject_memories_and_tools_skill(&mut prefix);

        // 2. RObot interaction skill — gated by robot.enabled
        self.inject_robot_skill(&mut prefix);

        // 3. Other auto-inject skills from the registry
        self.inject_custom_auto_skills(&mut prefix);

        if prefix.is_empty() {
            return prompt;
        }

        prefix.push_str("\n\n");
        prefix.push_str(&prompt);
        prefix
    }

    /// Injects memory data and the ralph-tools skill into the prefix.
    ///
    /// Special case: loads memory entries from the store, applies budget
    /// truncation, then appends the ralph-tools skill content (which covers
    /// both tasks and memories CLI usage).
    /// Memory data is gated by `memories.enabled && memories.inject == Auto`.
    /// The ralph-tools skill is injected when either memories or tasks are enabled.
    fn inject_memories_and_tools_skill(&self, prefix: &mut String) {
        let memories_config = &self.config.memories;

        // Inject memory DATA if memories are enabled with auto-inject
        if memories_config.enabled && memories_config.inject == InjectMode::Auto {
            info!(
                "Memory injection check: enabled={}, inject={:?}, workspace_root={:?}",
                memories_config.enabled, memories_config.inject, self.config.core.workspace_root
            );

            let workspace_root = &self.config.core.workspace_root;
            let store = MarkdownMemoryStore::with_default_path(workspace_root);
            let memories_path = workspace_root.join(".ralph/agent/memories.md");

            info!(
                "Looking for memories at: {:?} (exists: {})",
                memories_path,
                memories_path.exists()
            );

            let memories = match store.load() {
                Ok(memories) => {
                    info!("Successfully loaded {} memories from store", memories.len());
                    memories
                }
                Err(e) => {
                    info!(
                        "Failed to load memories for injection: {} (path: {:?})",
                        e, memories_path
                    );
                    Vec::new()
                }
            };

            if memories.is_empty() {
                info!("Memory store is empty - no memories to inject");
            } else {
                let mut memories_content = format_memories_as_markdown(&memories);

                if memories_config.budget > 0 {
                    let original_len = memories_content.len();
                    memories_content =
                        truncate_to_budget(&memories_content, memories_config.budget);
                    debug!(
                        "Applied budget: {} chars -> {} chars (budget: {})",
                        original_len,
                        memories_content.len(),
                        memories_config.budget
                    );
                }

                info!(
                    "Injecting {} memories ({} chars) into prompt",
                    memories.len(),
                    memories_content.len()
                );

                prefix.push_str(&memories_content);
            }
        }

        // Inject ralph-tools skills conditionally based on config
        let tasks_enabled = self.config.tasks.enabled;

        // Base skill (shared commands) when either memories or tasks are enabled
        if (memories_config.enabled || tasks_enabled)
            && let Some(skill) = self.skill_registry.get("ralph-tools")
        {
            if !prefix.is_empty() {
                prefix.push_str("\n\n");
            }
            prefix.push_str(&format!(
                "<ralph-tools-skill>\n{}\n</ralph-tools-skill>",
                skill.content.trim()
            ));
            debug!("Injected ralph-tools skill from registry");
        }

        // Tasks skill — only when tasks are enabled
        if tasks_enabled && let Some(skill) = self.skill_registry.get("ralph-tools-tasks") {
            if !prefix.is_empty() {
                prefix.push_str("\n\n");
            }
            prefix.push_str(&format!(
                "<ralph-tools-tasks-skill>\n{}\n</ralph-tools-tasks-skill>",
                skill.content.trim()
            ));
            debug!("Injected ralph-tools-tasks skill from registry");
        }

        // Memories skill — only when memories are enabled
        if memories_config.enabled
            && let Some(skill) = self.skill_registry.get("ralph-tools-memories")
        {
            if !prefix.is_empty() {
                prefix.push_str("\n\n");
            }
            prefix.push_str(&format!(
                "<ralph-tools-memories-skill>\n{}\n</ralph-tools-memories-skill>",
                skill.content.trim()
            ));
            debug!("Injected ralph-tools-memories skill from registry");
        }
    }

    /// Injects the RObot interaction skill content into the prefix.
    ///
    /// Gated by `robot.enabled`. Teaches agents how and when to interact
    /// with humans via `human.interact` events.
    fn inject_robot_skill(&self, prefix: &mut String) {
        if !self.config.robot.enabled {
            return;
        }

        if let Some(skill) = self.skill_registry.get("robot-interaction") {
            if !prefix.is_empty() {
                prefix.push_str("\n\n");
            }
            prefix.push_str(&format!(
                "<robot-skill>\n{}\n</robot-skill>",
                skill.content.trim()
            ));
            debug!("Injected robot interaction skill from registry");
        }
    }

    /// Injects any user-configured auto-inject skills (excluding built-in skills handled separately).
    fn inject_custom_auto_skills(&self, prefix: &mut String) {
        for skill in self.skill_registry.auto_inject_skills(None) {
            // Skip built-in skills handled above
            if matches!(
                skill.name.as_str(),
                "ralph-tools" | "ralph-tools-tasks" | "ralph-tools-memories" | "robot-interaction"
            ) {
                continue;
            }

            if !prefix.is_empty() {
                prefix.push_str("\n\n");
            }
            prefix.push_str(&format!(
                "<{name}-skill>\n{content}\n</{name}-skill>",
                name = skill.name,
                content = skill.content.trim()
            ));
            debug!("Injected auto-inject skill: {}", skill.name);
        }
    }

    /// Prepends scratchpad content to the prompt if the file exists and is non-empty.
    ///
    /// The scratchpad is the agent's working memory for the current objective.
    /// Auto-injecting saves one tool call per iteration.
    /// When the file exceeds the budget, the TAIL is kept (most recent entries).
    fn prepend_scratchpad(
        &self,
        prompt: String,
        active_hat_id_for_filter: Option<&HatId>,
    ) -> String {
        // Skip injection when scratchpad is disabled for the current hat
        if !self.ralph.active_scratchpad().enabled {
            return prompt;
        }

        let scratchpad_path = self.scratchpad_path();

        let resolved_path = if scratchpad_path.is_relative() {
            self.config.core.workspace_root.join(&scratchpad_path)
        } else {
            scratchpad_path
        };

        if !resolved_path.exists() {
            debug!(
                "Scratchpad not found at {:?}, skipping injection",
                resolved_path
            );
            return prompt;
        }

        let content = match std::fs::read_to_string(&resolved_path) {
            Ok(c) => c,
            Err(e) => {
                info!("Failed to read scratchpad for injection: {}", e);
                return prompt;
            }
        };

        if content.trim().is_empty() {
            debug!("Scratchpad is empty, skipping injection");
            return prompt;
        }

        // Unit 3 (2026-06-16-002 plan): when the active hat is the
        // `coordinator` and the loop is still in the bootstrap
        // window, strip `### HUMAN GUIDANCE` blocks from the
        // scratchpad snapshot.  We use the same state-machine
        // header detection as `persist_guidance_to_scratchpad` so
        // a line in `## NOTES` that happens to look like a
        // guidance block is not falsely stripped.
        let gate_closed = active_hat_id_for_filter
            .map(|hat| self.coordinator_bootstrap_gate_closed(hat))
            .unwrap_or(false);
        let content = if gate_closed {
            filter_human_guidance_blocks(&content)
        } else {
            content
        };
        if content.trim().is_empty() {
            debug!("Scratchpad empty after bootstrap filter, skipping injection");
            return prompt;
        }

        // Budget: 4000 tokens ~16000 chars. Keep the TAIL (most recent content).
        let char_budget = 4000 * 4;
        let content = if content.len() > char_budget {
            // Find a line boundary near the start of the tail
            let start = content.len() - char_budget;
            // Ensure we start at a valid UTF-8 character boundary
            let start = floor_char_boundary(&content, start);
            let line_start = content[start..].find('\n').map_or(start, |n| start + n + 1);
            let discarded = &content[..line_start];

            // Summarize discarded content by extracting markdown headings
            let headings: Vec<&str> = discarded
                .lines()
                .filter(|line| line.starts_with('#'))
                .collect();
            let summary = if headings.is_empty() {
                format!(
                    "<!-- earlier content truncated ({} chars omitted) -->",
                    line_start
                )
            } else {
                format!(
                    "<!-- earlier content truncated ({} chars omitted) -->\n\
                     <!-- discarded sections: {} -->",
                    line_start,
                    headings.join(" | ")
                )
            };

            format!("{}\n\n{}", summary, &content[line_start..])
        } else {
            content
        };

        info!("Injecting scratchpad ({} chars) into prompt", content.len());

        let mut final_prompt = format!(
            "<scratchpad path=\"{}\">\n{}\n</scratchpad>\n\n",
            self.ralph.active_scratchpad().path,
            content
        );
        final_prompt.push_str(&prompt);
        final_prompt
    }

    /// Prepends ready tasks to the prompt if tasks are enabled and any exist.
    ///
    /// Loads the task store and formats ready (unblocked, open) tasks into
    /// a `<ready-tasks>` XML block. This saves the agent a tool call per
    /// iteration and puts tasks at the same prominence as the scratchpad.
    fn prepend_ready_tasks(&self, prompt: String) -> String {
        if !self.config.tasks.enabled {
            return prompt;
        }

        use crate::task::TaskStatus;
        use crate::task_store::TaskStore;

        let tasks_path = self.tasks_path();
        let resolved_path = if tasks_path.is_relative() {
            self.config.core.workspace_root.join(&tasks_path)
        } else {
            tasks_path
        };

        if !resolved_path.exists() {
            return prompt;
        }

        let store = match TaskStore::load(&resolved_path) {
            Ok(s) => s,
            Err(e) => {
                info!("Failed to load task store for injection: {}", e);
                return prompt;
            }
        };

        let current_loop_id = self.current_loop_id();

        let ready = Self::filter_tasks_by_loop(store.ready(), current_loop_id.as_deref());
        let open = Self::filter_tasks_by_loop(store.open(), current_loop_id.as_deref());
        let all_count =
            Self::filter_tasks_by_loop(store.all().iter().collect(), current_loop_id.as_deref())
                .len();
        let closed_count = all_count - open.len();

        if open.is_empty() && closed_count == 0 {
            return prompt;
        }

        let mut section = String::from("<ready-tasks>\n");
        if ready.is_empty() && open.is_empty() {
            section.push_str("No open tasks. Create tasks with `ralph tools task add`.\n");
        } else {
            section.push_str(&format!(
                "## Tasks: {} ready, {} open, {} closed\n\n",
                ready.len(),
                open.len(),
                closed_count
            ));
            for task in &ready {
                let status_icon = match task.status {
                    TaskStatus::Open => "[ ]",
                    TaskStatus::InProgress => "[~]",
                    _ => "[?]",
                };
                section.push_str(&format!(
                    "- {} [P{}] {} ({}){}\n",
                    status_icon,
                    task.priority,
                    task.title,
                    task.id,
                    task.key
                        .as_deref()
                        .map(|key| format!(" — key: {key}"))
                        .unwrap_or_default()
                ));
            }
            // Show blocked tasks separately so agent knows they exist
            let ready_ids: Vec<&str> = ready.iter().map(|t| t.id.as_str()).collect();
            let blocked: Vec<_> = open
                .iter()
                .filter(|t| !ready_ids.contains(&t.id.as_str()))
                .collect();
            if !blocked.is_empty() {
                section.push_str("\nBlocked:\n");
                for task in blocked {
                    section.push_str(&format!(
                        "- [blocked] [P{}] {} ({}){} — blocked by: {}\n",
                        task.priority,
                        task.title,
                        task.id,
                        task.key
                            .as_deref()
                            .map(|key| format!(" — key: {key}"))
                            .unwrap_or_default(),
                        task.blocked_by.join(", ")
                    ));
                }
            }
        }
        section.push_str("</ready-tasks>\n\n");

        info!(
            "Injecting ready tasks ({} ready, {} open, {} closed) into prompt",
            ready.len(),
            open.len(),
            closed_count
        );

        let mut final_prompt = section;
        final_prompt.push_str(&prompt);
        final_prompt
    }

    /// Prepends state file contents to the prompt if state files are configured.
    fn prepend_state_files(&self, prompt: String) -> String {
        let config = match &self.config.core.state_files {
            Some(c) if c.enabled => c,
            _ => return prompt,
        };
        crate::state_file_injector::inject_state_files(
            prompt,
            config,
            &self.config.core.workspace_root,
        )
    }

    /// Builds the Ralph prompt (coordination mode).
    pub fn build_ralph_prompt(&self, prompt_content: &str) -> String {
        self.ralph.build_prompt(prompt_content, &[], &[])
    }

    /// R1 (2026-06-14-003 plan): resolve the current wave context for
    /// the `review-synthesizer` aggregate hat.  Returns `None` when no
    /// relevant wave events are present so the caller can fall back to
    /// the pre-R1 behaviour (synthesizer activates without wave
    /// metadata — typical for non-wave presets).
    ///
    /// `pending_synthesizer_timeout` is `true` when the synthesizer was
    /// woken up by `inject_review_aggregate_timeouts`.  The field is
    /// **consumed (taken)** on this call so the AGGREGATE_TIMEOUT
    /// signal does not leak across waves: a wave-1 timeout must not
    /// mark wave-2's synthesizer activation as timed-out.  This
    /// matches the calm-oak failure mode the plan §5.1.4 calls out
    /// (the original loop saw stale wave context across waves).
    pub fn build_wave_context_for_synthesizer(
        &mut self,
    ) -> Option<crate::wave_context::WaveContext> {
        let events_path = self.events_path_for_wave_context()?;
        let aggregate_timeout = self.state.pending_synthesizer_timeout.take().is_some();
        crate::wave_context::resolve_wave_context_for_synthesizer_with_aggregate_timeout(
            &events_path,
            2000,
            aggregate_timeout,
        )
    }

    /// R1: best-effort events file path lookup for the wave context
    /// resolver.  Returns `None` when no loop context is attached
    /// (CLI helpers that build prompts out of band) — the resolver
    /// then no-ops and the caller falls back to the legacy prompt.
    fn events_path_for_wave_context(&self) -> Option<std::path::PathBuf> {
        self.loop_context.as_ref().map(|ctx| ctx.events_path())
    }

    /// R1: render the `## WAVE CONTEXT` block for the given hat and
    /// prepend it to the prompt.  For hats other than
    /// `review-synthesizer` this is a no-op — the wave context is only
    /// meaningful for the synthesizer aggregate.
    fn prepend_wave_context(&mut self, prompt: String, hat_id: &HatId) -> String {
        let Some(ctx) = self.build_wave_context_for_synthesizer_if_match(hat_id) else {
            return prompt;
        };
        format!("{}{prompt}", ctx.to_prompt_block())
    }

    /// R3 (2026-06-14-003 plan): invoke the ephemeral isolation engine
    /// when the preset opts in.  The records are stored on
    /// `LoopState.last_ephemeral_relocations` and consumed by the
    /// next `build_prompt` call.  Best-effort: a git failure or
    /// missing workspace never aborts the loop.
    pub(crate) fn run_ephemeral_isolation(&mut self) {
        if !self.config.event_loop.ephemeral_isolation {
            return;
        }
        if self.config.event_loop.execution_mode != crate::config::HatExecutionMode::Isolated {
            return;
        }
        let workspace: std::path::PathBuf =
            if self.config.core.workspace_root.as_os_str().is_empty() {
                self.loop_context
                    .as_ref()
                    .map(|c| c.workspace().to_path_buf())
                    .unwrap_or_default()
            } else {
                self.config.core.workspace_root.clone()
            };
        if workspace.as_os_str().is_empty() {
            return;
        }
        let loop_id = self
            .loop_context
            .as_ref()
            .and_then(|c| c.loop_id().map(str::to_string));
        let records = self
            .ephemeral_isolation
            .scan_and_relocate(&workspace, loop_id.as_deref());
        if records.is_empty() {
            return;
        }
        tracing::info!(
            count = records.len(),
            workspace = %workspace.display(),
            "ephemeral_isolation: relocated runtime artefacts to .ralph/agent/"
        );
        self.state.last_ephemeral_relocations = records;
    }

    /// R3: render the `## EPHEMERAL RELOCATED` block for the prompt
    /// when the most recent `process_output` produced relocation
    /// records.  Empty / missing records short-circuit to a no-op so
    /// the prepend pipeline stays cheap.  Records are consumed (taken)
    /// on read so the block does not re-appear in subsequent
    /// iterations.
    pub(crate) fn prepend_ephemeral_relocations(&mut self, prompt: String) -> String {
        if self.state.last_ephemeral_relocations.is_empty() {
            return prompt;
        }
        let records = std::mem::take(&mut self.state.last_ephemeral_relocations);
        let mut section = String::from(
            "## EPHEMERAL RELOCATED\n\
             The following runtime artefacts were moved out of the source tree by the runner. \
             Do NOT recreate these files inside the source tree; write runtime notes to \
             `.ralph/agent/` instead.\n\n",
        );
        for rec in &records {
            section.push_str(&format!(
                "- `{}` -> `{}` ({} bytes appended)\n",
                rec.from, rec.to, rec.size_bytes
            ));
        }
        section.push('\n');
        format!("{section}{prompt}")
    }

    /// R1: helper that consults the resolver only when the hat is the
    /// synthesizer.  Returning `Option<WaveContext>` keeps the prepend
    /// helper a one-liner.
    fn build_wave_context_for_synthesizer_if_match(
        &mut self,
        hat_id: &HatId,
    ) -> Option<crate::wave_context::WaveContext> {
        if hat_id.as_str() != "review-synthesizer" {
            return None;
        }
        self.build_wave_context_for_synthesizer()
    }

    /// Test-only accessor that mirrors
    /// [`Self::build_wave_context_for_synthesizer_if_match`].  Exposed
    /// at `pub(crate)` for the integration tests under
    /// `event_loop::tests` so they can assert the resolved context
    /// without wiring up the full multi-hat `build_prompt` machinery.
    /// Production code should call the prepend helper or
    /// `wave_context_json_for_hat`.
    #[cfg(test)]
    pub(crate) fn build_wave_context_for_synthesizer_if_match_for_test(
        &mut self,
        hat_id: &HatId,
    ) -> Option<crate::wave_context::WaveContext> {
        self.build_wave_context_for_synthesizer_if_match(hat_id)
    }

    /// R1: serialized wave context for the given hat, suitable for
    /// `RALPH_WAVE_CONTEXT` env var.  Returns `None` for hats other
    /// than `review-synthesizer` and when no wave events are present.
    pub fn wave_context_json_for_hat(&mut self, hat_id: &HatId) -> Option<String> {
        let ctx = self.build_wave_context_for_synthesizer_if_match(hat_id)?;
        serde_json::to_string(&ctx.to_json()).ok()
    }

    /// Determines which hats should be active based on pending events.
    /// Returns list of Hat references that are triggered by any pending event.
    fn determine_active_hats(&self, events: &[Event]) -> Vec<&Hat> {
        let mut active_hats = Vec::new();
        for id in self.determine_active_hat_ids(events) {
            if let Some(hat) = self.registry.get(&id) {
                active_hats.push(hat);
            }
        }
        active_hats
    }

    fn determine_active_hat_ids(&self, events: &[Event]) -> Vec<HatId> {
        let mut entrypoint_hat_ids = Vec::new();
        let mut progressed_hat_ids = Vec::new();
        for event in events {
            // Skip system/observability events (event.*) — they are not hat
            // progress signals, only diagnostic/audit trails. The Ralph
            // fallback hat subscribes to "*" and would otherwise activate
            // for `event.execution_contract.rejected` and similar topics,
            // shadowing the targeted recovery event for the source hat.
            // See docs/plans/2026-06-04-001-fix-contract-rejection-hat-retry-plan.md
            // (U3: Preserve Active Hat Selection Through Guidance Partitioning).
            if Self::is_system_event(event.topic.as_str()) {
                continue;
            }
            // Prefer direct event target over topic-based lookup
            let hat_id = if let Some(target) = &event.target
                && self.registry.get(target).is_some()
            {
                target.clone()
            } else if let Some(hat) = self.registry.get_for_topic(event.topic.as_str()) {
                hat.id.clone()
            } else {
                continue;
            };

            let list = if self.is_entrypoint_topic(event.topic.as_str()) {
                &mut entrypoint_hat_ids
            } else {
                &mut progressed_hat_ids
            };
            if !list.iter().any(|id| id == &hat_id) {
                list.push(hat_id);
            }
        }
        // Prefer progressed hats over entrypoint hats. Entrypoint events
        // (starting_event, task.start, task.resume) linger in the bus after
        // the first hat runs. Including them would re-activate the first hat
        // alongside downstream hats, confusing the agent with multiple hat
        // instructions when only the downstream hat should run.
        if progressed_hat_ids.is_empty() {
            entrypoint_hat_ids
        } else {
            progressed_hat_ids
        }
    }

    fn effective_regular_events<'a>(&self, events: &'a [Event]) -> Vec<&'a Event> {
        let has_downstream_event = events.iter().any(|event| {
            !Self::is_system_event(event.topic.as_str())
                && !Self::is_kickoff_or_recovery_event(event.topic.as_str())
        });
        events
            .iter()
            .filter(|event| {
                // Also drop system/observability events from prompt context —
                // they are diagnostic, not actionable hat progress.
                !Self::is_system_event(event.topic.as_str())
                    && (!has_downstream_event
                        || !Self::is_kickoff_or_recovery_event(event.topic.as_str()))
            })
            .collect()
    }

    fn is_kickoff_or_recovery_event(topic: &str) -> bool {
        topic == "task.start" || topic == "task.resume" || topic.strip_suffix(".start").is_some()
    }

    /// Returns true for system/observability event topics that should not
    /// influence active hat selection or appear as actionable progress in
    /// the prompt (e.g. `event.execution_contract.rejected`,
    /// `event.malformed`, `event.scope_violation`). These are audit/diagnostic
    /// events, not hat routing signals.
    fn is_system_event(topic: &str) -> bool {
        topic.starts_with("event.")
    }

    fn is_entrypoint_topic(&self, topic: &str) -> bool {
        topic == "task.start"
            || topic == "task.resume"
            || topic.strip_suffix(".start").is_some()
            || self.config.event_loop.starting_event.as_deref() == Some(topic)
    }

    fn peek_pending_regular_events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        for hat_id in self.bus.hat_ids() {
            if let Some(pending) = self.bus.peek_pending(hat_id) {
                events.extend(pending.iter().cloned());
            }
        }
        events
    }

    /// Formats an event for prompt context.
    ///
    /// For top-level prompts (task.start, task.resume), wraps the payload in
    /// `<top-level-prompt>` XML tags to clearly delineate the user's original request.
    fn format_event(event: &Event) -> String {
        let topic = &event.topic;
        let payload = &event.payload;

        if topic.as_str() == "task.start" || topic.as_str() == "task.resume" {
            format!(
                "Event: {} - <top-level-prompt>\n{}\n</top-level-prompt>",
                topic, payload
            )
        } else {
            format!("Event: {} - {}", topic, payload)
        }
    }

    fn check_hat_exhaustion(&mut self, hat_id: &HatId, dropped: &[Event]) -> (bool, Option<Event>) {
        let Some(config) = self.registry.get_config(hat_id) else {
            return (false, None);
        };
        let Some(max) = config.max_activations else {
            return (false, None);
        };

        let count = *self.state.hat_activation_counts.get(hat_id).unwrap_or(&0);
        if count < max {
            return (false, None);
        }

        // Emit only once per hat per run (avoid flooding).
        let should_emit = self.state.exhausted_hats.insert(hat_id.clone());

        if !should_emit {
            // Hat is already exhausted - drop pending events silently.
            return (true, None);
        }

        let mut dropped_topics: Vec<String> = dropped.iter().map(|e| e.topic.to_string()).collect();
        dropped_topics.sort();

        let payload = format!(
            "Hat '{hat}' exhausted.\n- max_activations: {max}\n- activations: {count}\n- dropped_topics:\n  - {topics}",
            hat = hat_id.as_str(),
            max = max,
            count = count,
            topics = dropped_topics.join("\n  - ")
        );

        warn!(
            hat = %hat_id.as_str(),
            max_activations = max,
            activations = count,
            "Hat exhausted (max_activations reached)"
        );

        (
            true,
            Some(Event::new(
                format!("{}.exhausted", hat_id.as_str()),
                payload,
            )),
        )
    }

    fn record_hat_activations(&mut self, active_hat_ids: &[HatId]) {
        for hat_id in active_hat_ids {
            *self
                .state
                .hat_activation_counts
                .entry(hat_id.clone())
                .or_insert(0) += 1;
        }
    }

    /// Returns the primary active hat ID for display purposes.
    /// Returns the first active hat, or "ralph" if no specific hat is active.
    /// BTreeMap iteration is already sorted by key.
    pub fn get_active_hat_id(&self) -> HatId {
        let pending_events = self.peek_pending_regular_events();
        if let Some(active_hat_id) = self
            .determine_active_hat_ids(&pending_events)
            .into_iter()
            .next()
        {
            return active_hat_id;
        }
        HatId::new("ralph")
    }

    /// Injects a default event for a hat when the agent wrote no events.
    ///
    /// Call this after `process_events_from_jsonl` returns `Ok(false)` (no events found).
    /// If the hat has `default_publishes` configured, this injects the default event.
    ///
    /// If the default topic matches the completion promise, `completion_requested` is set
    /// so the loop can terminate. Without this, completion events injected via
    /// `default_publishes` would only be published to the bus (triggering downstream hats)
    /// but never detected by `check_completion_event`, causing an infinite loop.
    ///
    /// **U3 P0 fix (post-review)**: in `execution_mode: isolated`, this path
    /// runs *outside* `process_events_from_jsonl`'s scope enforcement, so we
    /// must mirror the same two gates that path enforces for JSONL events:
    ///
    /// 1. **Publish scope gate** — `default_topic` must be in the hat's
    ///    `publishes` list. If not, drop the injection and emit
    ///    `{hat}.scope_violation` to keep `default_publishes` from being a
    ///    back door around the U3 can_publish check.
    /// 2. **Per-turn single-event budget** — the default_publishes injection
    ///    counts as a business event for the current turn. Set
    ///    `first_business_event_accepted` so a subsequent JSONL business
    ///    event in the same turn hits `event.isolation.boundary_violation`
    ///    (and vice versa: if a JSONL business event was already accepted
    ///    this turn, drop the default_publishes injection and emit
    ///    `event.isolation.boundary_violation`).
    ///
    /// Coordinator mode is unchanged: there is no per-turn budget, and the
    /// `ralph` pseudo-hat's `RALPH_CONTROL_TOPICS` allowlist (in
    /// `event_origin.rs`) still governs what the runtime fallback hat may
    /// publish.
    pub fn check_default_publishes(&mut self, hat_id: &HatId) {
        let Some(config) = self.registry.get_config(hat_id) else {
            return;
        };
        let Some(default_topic) = config.default_publishes.as_ref() else {
            return;
        };
        let default_topic = default_topic.clone();
        let default_topic_str = default_topic.as_str();

        // U3 P0 fix — Gate 1: publish scope.
        // In isolated mode, the current hat's `publishes` list is the
        // authoritative scope; `default_publishes` must be a subset of it.
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && !self.registry.can_publish(hat_id, default_topic_str)
        {
            warn!(
                hat = %hat_id.as_str(),
                topic = %default_topic_str,
                "Isolated mode: default_publishes not declared in hat scope — dropping injection"
            );
            let violation_topic = format!("{}.scope_violation", hat_id.as_str());
            let violation_payload = format!(
                "Isolated mode: hat '{}' cannot publish default topic '{}' (not in publishes)",
                hat_id.as_str(),
                default_topic_str
            );
            self.bus
                .publish(Event::new(violation_topic, violation_payload));
            return;
        }

        // U3 P0 fix — Gate 2: per-turn single-event budget coordination.
        // If a JSONL business event was already accepted in this turn
        // (isolated_turn_business_event_accepted is sticky across
        // process_events and check_default_publishes), dropping the default
        // injection prevents two business events from being accepted in one
        // turn.
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && self.state.isolated_turn_business_event_accepted
            && !crate::event_origin::is_orchestrator_control_topic(
                default_topic_str,
                self.config.event_loop.cancellation_promise.as_str(),
            )
        {
            warn!(
                hat = %hat_id.as_str(),
                topic = %default_topic_str,
                "Isolated mode: default_publishes would exceed per-turn business-event budget — dropping"
            );
            let diagnostic = Event::new(
                "event.isolation.boundary_violation",
                format!(
                    "Isolated mode: default_publishes '{}' on hat '{}' dropped — one business event already accepted this turn",
                    default_topic_str,
                    hat_id.as_str()
                ),
            );
            self.bus.publish(diagnostic);
            return;
        }

        let default_event = Event::new(default_topic_str, "").with_source(hat_id.clone());
        let verdict_topics = self.verdict_gate_topics();
        let verdict_topics_slice = verdict_topics.as_deref();
        self.state
            .record_verdict_if_match(&default_event, verdict_topics_slice);

        debug!(
            hat = %hat_id.as_str(),
            topic = %default_topic_str,
            "No events written by hat, injecting default_publishes event"
        );

        self.state.record_event(&default_event);

        // U3 P0 fix — claim the per-turn business-event budget slot when we
        // actually inject (so a subsequent JSONL business event in the same
        // turn will be rejected by the boundary check).
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && !crate::event_origin::is_orchestrator_control_topic(
                default_topic_str,
                self.config.event_loop.cancellation_promise.as_str(),
            )
        {
            self.state.isolated_turn_business_event_accepted = true;
        }

        // If the default topic is the completion promise, set the flag directly.
        // The normal path (process_events_from_jsonl) sets this when reading from
        // JSONL, but default_publishes bypasses JSONL entirely.
        if default_topic_str == self.config.event_loop.completion_promise
            && !self.state.completion_honored
        {
            info!(
                hat = %hat_id.as_str(),
                topic = %default_topic_str,
                "default_publishes matches completion_promise — requesting termination"
            );
            self.state.completion_requested = true;
        }

        self.bus.publish(default_event);
    }

    /// Returns a mutable reference to the event bus for direct event publishing.
    ///
    /// This is primarily used for planning sessions to inject user responses
    /// as events into the orchestration loop.
    pub fn bus(&mut self) -> &mut EventBus {
        &mut self.bus
    }

    /// Processes output from a hat execution.
    ///
    /// Returns the termination reason if the loop should stop.
    pub fn process_output(
        &mut self,
        hat_id: &HatId,
        output: &str,
        success: bool,
    ) -> Option<TerminationReason> {
        self.state.iteration += 1;
        self.state.last_hat = Some(hat_id.clone());

        // WRC-U4 (2026-06-12-003 / KTD-13 / hook 3): drain
        // handoff deadlines that exceeded their dispatch window
        // since the last iteration. Each escalation is converted
        // into a `task.resume` event routed to the safe target
        // (plan-gate or review-coordinator — see
        // `HandoffTracker::expired`). The recovery envelope is
        // written by the existing `RecoveryResponder` via the
        // `event.isolation.boundary_violation` path, which already
        // handles envelope writing and dedup. We do **not** log a
        // recovery envelope here directly to keep the tracker
        // side-effect-free: the runner's `process_events_from_jsonl`
        // sees the synthesized `task.resume` event on the next
        // pass and routes it through the normal recovery flow.
        //
        // Coordinator mode is a no-op because the HandoffIndex
        // returns `None` for every consumer lookup there; the
        // tracker's `pending` map stays empty.
        let escalations = self
            .state
            .handoff_tracker
            .expired(std::time::Instant::now());
        for esc in escalations {
            warn!(
                topic = %esc.topic,
                consumer = %esc.consumer,
                event_id = %esc.event_id,
                safe_target = %esc.safe_target,
                "handoff dispatch timeout: routing task.resume to {}",
                esc.safe_target,
            );
            // Synthesize the resume event into the bus so the
            // dispatcher can route it on the next iteration. The
            // event's `hat` is the safe_target so the
            // `EventOriginGuard` accepts the publish (it is in the
            // safe_target's `publishes` list per
            // `HatRegistry::from_runtime_config`). The payload
            // carries the full escalation metadata for the
            // downstream hat to act on.
            let payload = serde_json::json!({
                "reason": "handoff_dispatch_timeout",
                "topic": esc.topic,
                "consumer": esc.consumer,
                "event_id": esc.event_id,
                "safe_target": esc.safe_target,
                "details": esc.reason,
            });
            let resume_event = Event::new("task.resume", payload.to_string())
                .with_source(HatId::from(esc.safe_target.as_str()));
            self.bus.publish(resume_event);
            // 2026-06-13-004 U7 (P2-4): write a recovery envelope
            // for the handoff escalation so the responder can
            // surface this stall in the next prompt. The bus
            // `task.resume` event above is the visible-to-agent
            // signal; the envelope is the diagnose / journal
            // surface. KTD-5 locks the source to `StallRecovery`
            // and the outcome to `Escalated`. The two streams
            // (bus + journal) are kept in lockstep so operators
            // can correlate them in `ralph diagnose` and the
            // orchestration log.
            let reason_code = "handoff_dispatch_timeout";
            let env_source_hat = esc.consumer.clone();
            let env_target_hat = esc.safe_target.clone();
            let env_topic = esc.topic.clone();
            // Unit 5 / Unit 7 R-C2 (2026-06-17-001 plan): when the
            // escalation targets a wave-related hat, attach the
            // current flow record (wave_id / wave_total /
            // received_count / flow_phase) to the envelope so the
            // diagnose reporter can reconstruct the wave's
            // timeline.  This is informational only: the existing
            // handoff escalation path stays unchanged for non-wave
            // handoffs (R5, payload contract, etc.).
            let mut flow_context: Option<serde_json::Value> = None;
            if Self::is_wave_hat(&HatId::new(&esc.consumer)) {
                if let Some(record) = self.state.flow_lifecycle.get(&esc.event_id) {
                    flow_context = Some(serde_json::json!({
                        "wave_id": record.flow_unit_id,
                        "wave_total": record.wave_total,
                        "received_count": record.received_count,
                        "flow_phase": record.phase.as_str(),
                    }));
                } else {
                    // No record keyed by event_id — fall back to a
                    // record whose target_hat matches the consumer,
                    // picking the most recently transitioned one.
                    // This keeps the envelope useful when the
                    // event_id naming diverges (e.g. legacy `sla:*`
                    // keys) while remaining deterministic.
                    let candidates: Vec<&crate::flow_lifecycle::FlowLifecycleRecord> =
                        self.state.flow_lifecycle.active_records()
                            .filter(|r| r.target_hat == esc.consumer)
                            .collect();
                    if let Some(active) = candidates
                        .into_iter()
                        .max_by_key(|r| r.last_transition_at)
                    {
                        flow_context = Some(serde_json::json!({
                            "wave_id": active.flow_unit_id,
                            "wave_total": active.wave_total,
                            "received_count": active.received_count,
                            "flow_phase": active.phase.as_str(),
                        }));
                    }
                }
            }
            let mut env_builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
                .source(crate::diagnosis::DiagnosisSource::StallRecovery)
                .severity(crate::diagnosis::DiagnosisSeverity::Warning)
                .iteration(self.state.iteration)
                .topic(env_topic.clone())
                .source_hat(&env_source_hat)
                .target_hat(&env_target_hat)
                .reason_code(reason_code)
                .message(format!(
                    "handoff deadline exceeded: consumer '{}' did not activate within timeout",
                    env_source_hat
                ))
                .expected_action(format!(
                    "Consumer hat '{}' must activate before the next iteration. \
                     A `task.resume` has been routed to the safe target '{}' \
                     to keep the loop moving.",
                    env_source_hat, env_target_hat
                ))
                .safe_target(true)
                .outcome(crate::diagnosis::DiagnosisOutcome::Escalated)
                .evidence(crate::diagnosis::EvidenceRef {
                    kind: crate::diagnosis::EvidenceKind::Topic,
                    ref_path: env_topic.clone(),
                    snippet: Some(format!("event_id={} details={}", esc.event_id, esc.reason)),
                })
                .retry_key(
                    crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                        crate::diagnosis::DiagnosisSource::StallRecovery,
                        Some(&env_source_hat),
                        Some(&env_topic),
                        reason_code,
                        None,
                    ),
                );
            if let Some(ctx) = flow_context.as_ref() {
                env_builder = env_builder.evidence(crate::diagnosis::EvidenceRef {
                    kind: crate::diagnosis::EvidenceKind::Field,
                    ref_path: "flow.context".to_string(),
                    snippet: Some(ctx.to_string()),
                });
            }
            if let Some(session_id) = self.diagnostics().session_id() {
                env_builder = env_builder.session_id(session_id);
            }
            let envelope = env_builder.build();
            self.record_recovery_envelope(
                &envelope,
                vec![format!(
                    "handoff_escalation consumer={} topic={} event_id={} safe_target={}",
                    env_source_hat, env_topic, esc.event_id, env_target_hat
                )],
            );
        }

        // Track the isolated hat for scope enforcement in process_parse_result
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated {
            self.state.current_isolated_hat = Some(hat_id.clone());
        } else {
            self.state.current_isolated_hat = None;
        }
        // U3 P0 fix: reset the per-turn business-event budget at every turn
        // boundary so `check_default_publishes` and `process_parse_result`
        // see a consistent view of "what has been accepted this turn".
        self.state.isolated_turn_business_event_accepted = false;

        // Periodic robot check-in
        if let Some(interval_secs) = self.config.robot.checkin_interval_seconds
            && let Some(ref robot_service) = self.robot_service
        {
            let elapsed = self.state.elapsed();
            let interval = std::time::Duration::from_secs(interval_secs);
            let last = self
                .state
                .last_checkin_at
                .map(|t| t.elapsed())
                .unwrap_or(elapsed);

            if last >= interval {
                let context = self.build_checkin_context(hat_id);
                match robot_service.send_checkin(self.state.iteration, elapsed, Some(&context)) {
                    Ok(_) => {
                        self.state.last_checkin_at = Some(std::time::Instant::now());
                        debug!(iteration = self.state.iteration, "Sent robot check-in");
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to send robot check-in");
                    }
                }
            }
        }

        // Log iteration started
        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::IterationStarted,
        );

        // Log hat selected
        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::HatSelected {
                hat: hat_id.to_string(),
                reason: "process_output".to_string(),
            },
        );

        // Track failures
        if success {
            self.state.consecutive_failures = 0;
        } else {
            self.state.consecutive_failures += 1;
        }

        let _ = output;

        // File-modification audit: detect when a hat with disallowed Edit/Write tools
        // modified files. This is hard enforcement — emits a scope_violation event.
        self.audit_file_modifications(hat_id);

        // R3 (2026-06-14-003 plan): ephemeral file isolation.  When the
        // preset opts in via `event_loop.ephemeral_isolation: true` and
        // the loop is in isolated mode, scan the workspace for
        // runtime artefacts (`scratchpad.md`, `tmp*.md`, `*.bak`) that
        // landed in source trees and relocate them to
        // `.ralph/agent/scratchpad-{loop_id}.md`.  The records are
        // saved on `LoopState` so the next `build_prompt` can include
        // a `## EPHEMERAL RELOCATED` block.  The engine is best-
        // effort — a git failure, a read-only FS, or an unrecognised
        // layout does not interrupt the loop.
        self.run_ephemeral_isolation();

        // Events are ONLY read from the JSONL file written by `ralph emit`.
        // This enforces tool use and prevents confabulation (agent claiming to emit without actually doing so).
        // See process_events_from_jsonl() for event processing.

        // Check termination conditions
        self.check_termination()
    }

    /// Audits file modifications after a hat iteration.
    ///
    /// If the hat has `Edit` or `Write` in its `disallowed_tools`, checks whether
    /// files were modified (via `git diff --stat HEAD`). If so, emits a
    /// `<hat_id>.scope_violation` event.
    fn audit_file_modifications(&mut self, hat_id: &HatId) {
        let config = match self.registry.get_config(hat_id) {
            Some(c) => c,
            None => return,
        };

        let has_write_restriction = config
            .disallowed_tools
            .iter()
            .any(|t| t == "Edit" || t == "Write");

        if !has_write_restriction {
            return;
        }

        let workspace = &self.config.core.workspace_root;
        let diff_output = std::process::Command::new("git")
            .args(["diff", "--stat", "HEAD"])
            .current_dir(workspace)
            .output();

        match diff_output {
            Ok(output) if !output.stdout.is_empty() => {
                let diff_stat = String::from_utf8_lossy(&output.stdout).trim().to_string();
                warn!(
                    hat = %hat_id.as_str(),
                    diff = %diff_stat,
                    "Hat modified files despite tool restrictions (scope violation)"
                );

                let violation_topic = format!("{}.scope_violation", hat_id.as_str());
                let violation = Event::new(
                    violation_topic.as_str(),
                    format!(
                        "Hat '{}' modified files with Edit/Write disallowed:\n{}",
                        hat_id.as_str(),
                        diff_stat
                    ),
                );
                self.bus.publish(violation);
            }
            Err(e) => {
                debug!(error = %e, "Could not run git diff for file-modification audit");
            }
            _ => {} // No modifications — all good
        }
    }

    /// Extracts task identifier from build.blocked payload.
    /// Uses first line of payload as task ID.
    fn extract_task_id(payload: &str) -> String {
        payload
            .lines()
            .next()
            .unwrap_or("unknown")
            .trim()
            .to_string()
    }

    /// Adds cost to the cumulative total.
    pub fn add_cost(&mut self, cost: f64) {
        self.state.cumulative_cost += cost;
    }

    /// Verifies all tasks in scratchpad are complete or cancelled.
    ///
    /// Returns:
    /// - `Ok(true)` if all tasks are `[x]` or `[~]`, or if scratchpad is disabled
    /// - `Ok(false)` if any tasks are `[ ]` (pending)
    /// - `Err(...)` if scratchpad doesn't exist or can't be read
    fn verify_scratchpad_complete(&self) -> Result<bool, std::io::Error> {
        // Nothing to verify when scratchpad is disabled
        if !self.ralph.active_scratchpad().enabled {
            return Ok(true);
        }

        let scratchpad_path = self.scratchpad_path();

        if !scratchpad_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Scratchpad does not exist",
            ));
        }

        let content = std::fs::read_to_string(scratchpad_path)?;

        let has_pending = content
            .lines()
            .any(|line| line.trim_start().starts_with("- [ ]"));

        Ok(!has_pending)
    }

    /// Reads the current loop ID from the marker file.
    ///
    /// Returns `None` if no marker exists or is empty, which means
    /// task queries should be unfiltered (backwards compatible).
    fn current_loop_id(&self) -> Option<String> {
        self.loop_context
            .as_ref()
            .and_then(|ctx| {
                let marker_path = ctx.ralph_dir().join("current-loop-id");
                std::fs::read_to_string(&marker_path).ok()
            })
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
    }

    /// Returns the loop ID used for execution-contract task-loop checks.
    ///
    /// Primary loops keep `LoopContext::loop_id == None` and identify themselves
    /// via the `.ralph/current-loop-id` marker; worktree loops carry their id
    /// in the context. This helper funnels both shapes through the marker-based
    /// reader so the contract check never misclassifies primary-loop tasks as
    /// belonging to a non-existent "default" loop.
    fn current_loop_id_for_contract(&self) -> String {
        self.current_loop_id()
            .unwrap_or_else(|| "default".to_string())
    }

    /// Filters a task list by loop ID. When `loop_id` is `None`, returns all tasks.
    fn filter_tasks_by_loop<'a>(
        tasks: Vec<&'a crate::task::Task>,
        loop_id: Option<&str>,
    ) -> Vec<&'a crate::task::Task> {
        match loop_id {
            Some(id) => tasks
                .into_iter()
                .filter(|t| t.loop_id.as_deref() == Some(id))
                .collect(),
            None => tasks,
        }
    }

    fn verify_tasks_complete(&self) -> Result<bool, std::io::Error> {
        use crate::task_store::TaskStore;

        let tasks_path = self.tasks_path();

        // No tasks file = no pending tasks = complete
        if !tasks_path.exists() {
            return Ok(true);
        }

        let store = TaskStore::load(&tasks_path)?;
        let current_loop_id = self.current_loop_id();
        let open = Self::filter_tasks_by_loop(store.open(), current_loop_id.as_deref());
        Ok(open.is_empty())
    }

    /// Builds a [`CheckinContext`] with current loop state for robot check-ins.
    fn build_checkin_context(&self, hat_id: &HatId) -> CheckinContext {
        let (open_tasks, closed_tasks) = self.count_tasks();
        CheckinContext {
            current_hat: Some(hat_id.as_str().to_string()),
            open_tasks,
            closed_tasks,
            cumulative_cost: self.state.cumulative_cost,
        }
    }

    /// Counts open and closed tasks from the task store.
    ///
    /// Returns `(open_count, closed_count)`. "Open" means non-terminal tasks,
    /// "closed" means tasks with `TaskStatus::Closed`.
    fn count_tasks(&self) -> (usize, usize) {
        use crate::task_store::TaskStore;

        let tasks_path = self.tasks_path();
        if !tasks_path.exists() {
            return (0, 0);
        }

        match TaskStore::load(&tasks_path) {
            Ok(store) => {
                let current_loop_id = self.current_loop_id();
                let all = Self::filter_tasks_by_loop(
                    store.all().iter().collect(),
                    current_loop_id.as_deref(),
                );
                let open = Self::filter_tasks_by_loop(store.open(), current_loop_id.as_deref());
                let closed = all.len() - open.len();
                (open.len(), closed)
            }
            Err(_) => (0, 0),
        }
    }

    /// Returns a list of open task descriptions for logging purposes.
    fn get_open_task_list(&self) -> Vec<String> {
        use crate::task_store::TaskStore;

        let tasks_path = self.tasks_path();
        if let Ok(store) = TaskStore::load(&tasks_path) {
            let current_loop_id = self.current_loop_id();
            let open = Self::filter_tasks_by_loop(store.open(), current_loop_id.as_deref());
            return open
                .iter()
                .map(|t| format!("{}: {}", t.id, t.title))
                .collect();
        }
        vec![]
    }

    fn warn_on_mutation_evidence(&self, evidence: &crate::event_parser::BackpressureEvidence) {
        let threshold = self.config.event_loop.mutation_score_warn_threshold;

        match &evidence.mutants {
            Some(mutants) => {
                if let Some(reason) = Self::mutation_warning_reason(mutants, threshold) {
                    warn!(
                        reason = %reason,
                        mutants_status = ?mutants.status,
                        mutants_score = mutants.score_percent,
                        mutants_threshold = threshold,
                        "Mutation testing warning"
                    );
                }
            }
            None => {
                if let Some(threshold) = threshold {
                    warn!(
                        mutants_threshold = threshold,
                        "Mutation testing warning: missing mutation evidence in build.done payload"
                    );
                }
            }
        }
    }

    fn mutation_warning_reason(
        mutants: &MutationEvidence,
        threshold: Option<f64>,
    ) -> Option<String> {
        match mutants.status {
            MutationStatus::Fail => Some("mutation testing failed".to_string()),
            MutationStatus::Warn => Some(Self::format_mutation_message(
                "mutation score below threshold",
                mutants.score_percent,
            )),
            MutationStatus::Unknown => Some("mutation testing status unknown".to_string()),
            MutationStatus::Pass => {
                let threshold = threshold?;

                match mutants.score_percent {
                    Some(score) if score < threshold => Some(format!(
                        "mutation score {:.2}% below threshold {:.2}%",
                        score, threshold
                    )),
                    Some(_) => None,
                    None => Some(format!(
                        "mutation score missing (threshold {:.2}%)",
                        threshold
                    )),
                }
            }
        }
    }

    fn format_mutation_message(message: &str, score: Option<f64>) -> String {
        match score {
            Some(score) => format!("{message} ({score:.2}%)"),
            None => message.to_string(),
        }
    }

    fn parse_human_interact_context(payload: &str) -> Value {
        let mut context = match serde_json::from_str::<Value>(payload) {
            Ok(Value::Object(map)) => map,
            Ok(value) => {
                let mut map = Map::new();
                map.insert("question".to_string(), value);
                map
            }
            Err(_) => {
                let mut map = Map::new();
                map.insert("question".to_string(), Value::String(payload.to_string()));
                map
            }
        };

        if !context.contains_key("question") {
            context.insert("question".to_string(), Value::String(payload.to_string()));
        }

        Value::Object(context)
    }

    fn is_restart_request_payload(payload: &str) -> bool {
        let payload = payload.to_ascii_lowercase();
        payload.contains("restart yourself") || payload.contains("restart ralph")
    }

    fn is_restart_request_event(event: &Event) -> bool {
        matches!(event.topic.as_str(), "human.response" | "user.prompt")
            && Self::is_restart_request_payload(&event.payload)
    }

    /// Checks if all started guarded workflow instances have reached a terminal phase.
    ///
    /// Returns `Some(WorkflowGuardRejection)` if any instance is incomplete, `None` if all are terminal.
    ///
    /// Terminal phase is the last topic in the chain. An instance is considered "started"
    /// if it has any progress recorded (phase > 0, or any event in the chain has been seen).
    fn check_workflow_guard_completion(
        &self,
        guards: &crate::config::WorkflowGuardsConfig,
    ) -> Option<WorkflowGuardRejection> {
        for chain in &guards.chains {
            // Advisory chains are permissive and should not block LOOP_COMPLETE
            if matches!(chain.mode, crate::config::WorkflowChainMode::Advisory) {
                continue;
            }

            let terminal_phase = chain.topics.len().saturating_sub(1);

            // Check all instances for this chain
            for instance_key in self.state.workflow_progress.instance_keys(&chain.name) {
                let current_phase = self
                    .state
                    .workflow_progress
                    .get_phase(&chain.name, instance_key.as_deref());

                // Instance has no progress — not started, skip
                let current_phase = match current_phase {
                    Some(p) => p,
                    None => continue,
                };

                // If the instance hasn't reached terminal phase, it's incomplete
                if current_phase < terminal_phase {
                    let current_topic = chain
                        .topics
                        .get(current_phase)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    let next_topic = chain
                        .topics
                        .get(current_phase + 1)
                        .cloned()
                        .unwrap_or_else(|| "terminal".to_string());

                    return Some(WorkflowGuardRejection {
                        message: format!(
                            "workflow instance '{}' (chain '{}') is at phase {} ('{}') but not yet at terminal phase {} ('{}')",
                            instance_key.as_deref().unwrap_or("global"),
                            chain.name,
                            current_phase,
                            current_topic,
                            terminal_phase,
                            next_topic
                        ),
                    });
                }
            }
        }
        None
    }

    fn mark_restart_requested(&self, source: &str) {
        let restart_path =
            std::path::Path::new(&self.config.core.workspace_root).join(".ralph/restart-requested");

        if let Some(parent) = restart_path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            warn!(
                error = %err,
                path = %parent.display(),
                "Failed to create restart-requested parent directory"
            );
            return;
        }

        if let Err(err) = std::fs::write(&restart_path, source) {
            warn!(
                error = %err,
                path = %restart_path.display(),
                "Failed to write restart-requested signal"
            );
            return;
        }

        info!(
            source,
            path = %restart_path.display(),
            "Restart requested from human text"
        );
    }

    /// Processes events from JSONL and routes orphaned events to Ralph.
    ///
    /// Also handles backpressure for malformed JSONL lines by:
    /// 1. Emitting `event.malformed` system events for each parse failure
    /// 2. Tracking consecutive failures for termination check
    /// 3. Resetting counter when valid events are parsed
    ///
    /// Returns [`ProcessedEvents`] indicating whether events were found, whether
    /// semantic `plan.*` topics were published, structured `human.interact`
    /// context/outcome metadata, and whether any were orphans that Ralph should
    /// handle.
    pub fn process_events_from_jsonl(&mut self) -> std::io::Result<ProcessedEvents> {
        let result = self.event_reader.read_new_events()?;
        self.process_parse_result(result)
    }

    /// Inner event processing that operates on an already-parsed `ParseResult`.
    ///
    /// This is the single source of truth for event validation, backpressure,
    /// scope enforcement, and bus publishing. Both `process_events_from_jsonl`
    /// and `process_events_from_jsonl_with_waves` delegate to this method.
    fn process_parse_result(
        &mut self,
        result: crate::event_reader::ParseResult,
    ) -> std::io::Result<ProcessedEvents> {
        // DEBUG: 添加入口日志记录所有输入事件
        let event_count = result.events.len();
        let malformed_count = result.malformed.len();
        tracing::debug!(
            iteration = self.state.iteration,
            valid_events = event_count,
            malformed_events = malformed_count,
            "process_parse_result entry - events received"
        );
        // DEBUG: 记录前几个事件的详情用于调试
        for (i, evt) in result.events.iter().take(5).enumerate() {
            tracing::debug!(
                index = i,
                hat = ?evt.hat.as_deref(),
                topic = %evt.topic,
                ts = %evt.ts,
                "event detail"
            );
        }

        // U6: capture payload contract violation produced by event policy
        // validation. The loop runner will read this and pause with a
        // diagnostic.
        let mut payload_contract_violation: Option<
            crate::payload_contract::PayloadContractViolation,
        > = None;

        // Handle malformed lines with backpressure
        for malformed in &result.malformed {
            let payload = format!(
                "Line {}: {}\nContent: {}",
                malformed.line_number, malformed.error, &malformed.content
            );
            let event = Event::new("event.malformed", &payload);
            self.bus.publish(event);
            self.state.consecutive_malformed_events += 1;
            warn!(
                line = malformed.line_number,
                consecutive = self.state.consecutive_malformed_events,
                "Malformed event line detected"
            );
        }

        // Reset counter when valid events are parsed
        if !result.events.is_empty() {
            self.state.consecutive_malformed_events = 0;
        }

        if result.events.is_empty() && result.malformed.is_empty() {
            return Ok(ProcessedEvents {
                had_events: false,
                had_raw_events: false,
                had_rejected_events: false,
                had_plan_events: false,
                human_interact_context: None,
                has_orphans: false,
                accepted_events: Vec::new(),
                contract_rejections: Vec::new(),
                payload_contract_violation: None,
            });
        }

        // --- Scope enforcement ---
        // 2026-06-13-004 U7: copy out `current_isolated_hat` and the
        // `cancellation_promise` (as owned `String`) so the
        // immutable borrows of `self.state` / `self.config` end
        // before the loop body needs to take a mutable borrow of
        // `self` (e.g. via `record_recovery_envelope`). The
        // `&& let Some(ref …)` form would hold an immutable borrow
        // of `self.state` for the entire `if` block, blocking any
        // `&mut self` call inside it (E0502). Cloning is cheap
        // (a one-time allocation per turn) and lets the body freely
        // call `record_recovery_envelope` / `bus.publish` etc.
        let isolated_hat_owned: Option<ralph_proto::HatId> =
            self.state.current_isolated_hat.clone();
        let cancellation_owned: String = self.config.event_loop.cancellation_promise.clone();
        let events = if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && let Some(ref isolated_hat) = isolated_hat_owned
        {
            // Isolated mode: hard-enforce current hat scope + single business event boundary.
            // U3: orchestrator control topics and diagnostic topics bypass the budget
            // (they are loop-internal, not agent progress). Completion promises and
            // other agent terminal topics go through the normal `can_publish` +
            // single-event budget path so an isolated hat cannot bypass its
            // declared publish scope by emitting a completion-style event.
            let mut accepted = Vec::new();
            let mut first_business_event_accepted = false;
            // 2026-06-13-004 U3: track the first wave_id admitted in
            // this turn so subsequent events with the same wave_id
            // are exempt from the per-turn business-event budget.
            // The inner type is `Option<Option<String>>`:
            // outer `None` = no business event yet; inner `Some(None)`
            // = first business event had no wave_id (regular emit);
            // inner `Some(Some(wid))` = first business event was a
            // wave result with that wave_id.
            let mut first_wave_id_accepted: Option<Option<String>> = None;
            // 2026-06-13-004 P0 #4 review fix (U7 envelope disk
            // storm): per-turn dedup set for scope_drop retry_keys.
            // Multiple identical scope drops in the same
            // turn collapse to a single envelope write (the bus
            // `event.isolation.boundary_violation` event still
            // fires for each, preserving operator visibility —
            // only the recovery journal is dedup'd). This
            // protects `recovery.jsonl` from an 8x scope-drop
            // storm in long-running waves while still letting
            // ADV-1's retry_key namespace distinguish different
            // scope drops (different wave_id / scope_hat / topic
            // → different key → different envelope).
            let mut envelopes_written_this_turn: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let cancellation = cancellation_owned.as_str();

            for event in result.events {
                let topic = event.topic.as_str();
                let is_orchestrator_internal =
                    crate::event_origin::is_orchestrator_control_topic(topic, cancellation)
                        || crate::event_origin::is_orchestrator_diagnostic_topic(topic);

                if is_orchestrator_internal {
                    // Loop-internal event — always accepted, does not
                    // consume the per-turn business-event budget.
                    accepted.push(event);
                    continue;
                }

                // 2026-06-13-004 U2 (P0-1): prefer the event's own
                // `hat` field as the scope-anchor. The wave merge
                // layer (see `merge_wave_results_to_events_file`)
                // writes each record with `hat` set to the worker
                // provenance, so a re-published `review.dimension.done`
                // from `dimension-reviewer` is now attributed to
                // `dimension-reviewer`, not to the orchestrator
                // `current_isolated_hat` (e.g. `review-coordinator`).
                // When the event lacks `hat` (e.g. legacy hand-written
                // records, malformed agents), we fall back to
                // `isolated_hat` — the original behaviour.
                let scope_hat = event
                    .hat
                    .as_deref()
                    .map(|h| ralph_proto::HatId::new(h))
                    .unwrap_or_else(|| isolated_hat.clone());
                if !self.isolated_publish_allowed(&scope_hat, event.topic.as_str()) {
                    warn!(
                        hat = %isolated_hat.as_str(),
                        topic = %event.topic,
                        "Isolated mode: event out of hat scope — dropping"
                    );
                    // P1 finding #11: use the canonical orchestrator
                    // diagnostic topic from the allowlist, embedding the
                    // hat name in the payload. This keeps the bus surface
                    // uniform with the rest of the diagnostic taxonomy
                    // and ensures the entry survives the
                    // `is_orchestrator_diagnostic_topic` allowlist check
                    // on subsequent reads.
                    let violation = Event::new(
                        "event.isolation.boundary_violation",
                        format!(
                            "{{\"hat\":\"{}\",\"topic\":\"{}\",\"violation\":\"Isolated mode: hat '{}' cannot publish topic '{}'\"}}",
                            isolated_hat.as_str(),
                            event.topic,
                            isolated_hat.as_str(),
                            event.topic
                        ),
                    );
                    self.bus.publish(violation);
                    // 2026-06-13-004 U7 (P0-2 / P2-4): also write a
                    // recovery envelope to `recovery.jsonl` so the
                    // responder can surface this scope drop in the
                    // next prompt. Without this, the boundary
                    // violation is only visible in
                    // `orchestration.jsonl` (where bus events are
                    // recorded) — `recovery.jsonl` is the journal
                    // `ralph diagnose` reads, so a missing entry
                    // here means a missing signal. The bus event
                    // above is preserved for backward compatibility
                    // with existing log scrapers. KTD-5 locks the
                    // source to `WorkflowGuard` and the outcome to
                    // `Escalated` (not retryable — the agent has to
                    // fix its scope, not just retry).
                    let reason_code = "isolated_scope_violation";
                    // 2026-06-13-004 review fix (ce-code-review ADV-1):
                    // namespace the retry_key by `wave_id` AND
                    // `wave_index` when the event is part of a
                    // wave so 8 dimensions of the same wave
                    // produce 8 distinct journal entries
                    // (otherwise the responder's dedup collapses
                    // them into 1, re-creating the original
                    // "invisible failure" bug at the recovery
                    // layer). Non-wave events keep the original
                    // tuple-based key.
                    // 2026-06-13-004 P0 #2 + P0 #3 review fix
                    // (ADV-1 '?' fallback + ADV-3 normalize bypass):
                    // route wave events through
                    // `retry_key_from_parts` so `normalize_part`
                    // applies (lowercase + ASCII-only). Without
                    // this, `Reviewer` vs `reviewer` produced
                    // distinct retry_keys and bypassed the U5
                    // responder dedup. The wave_id + wave_index
                    // parts go through the normalizer together
                    // with `scope_hat` + `topic` + `reason_code`,
                    // keeping the format consistent with the
                    // non-wave branch and ensuring every
                    // collision case (case-difference, special
                    // chars, length) is normalized uniformly.
                    let scope_drop_retry_key = match event.wave_id.as_deref() {
                        Some(wid) => {
                            let widx = event
                                .wave_index
                                .map(|i| i.to_string())
                                .unwrap_or_else(|| format!("ts-{}", event.ts));
                            crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                                crate::diagnosis::DiagnosisSource::WorkflowGuard,
                                Some(scope_hat.as_str()),
                                Some(event.topic.as_str()),
                                // Embed wave_id + wave_index in the
                                // `reason_code` slot so the namespace
                                // is preserved end-to-end.
                                &format!("{reason_code}/{wid}/{widx}"),
                                None,
                            )
                        }
                        None => {
                            crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                                crate::diagnosis::DiagnosisSource::WorkflowGuard,
                                Some(scope_hat.as_str()),
                                Some(event.topic.as_str()),
                                reason_code,
                                None,
                            )
                        }
                    };
                    let mut env_builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
                        .source(crate::diagnosis::DiagnosisSource::WorkflowGuard)
                        .severity(crate::diagnosis::DiagnosisSeverity::Warning)
                        .iteration(self.state.iteration)
                        .topic(event.topic.to_string())
                        .source_hat(scope_hat.as_str())
                        .target_hat(scope_hat.as_str())
                        .reason_code(reason_code)
                        .message(format!(
                            "isolated mode: hat '{}' cannot publish topic '{}'",
                            scope_hat.as_str(),
                            event.topic
                        ))
                        .expected_action(format!(
                            "Hat '{}' must declare '{}' in its `publishes` list (or stop emitting it). \
                             This scope drop is not retryable — re-emit a topic the hat is allowed to publish.",
                            scope_hat.as_str(),
                            event.topic
                        ))
                        .safe_target(false)
                        .outcome(crate::diagnosis::DiagnosisOutcome::Escalated)
                        .evidence(crate::diagnosis::EvidenceRef {
                            kind: crate::diagnosis::EvidenceKind::Topic,
                            ref_path: event.topic.to_string(),
                            snippet: Some(format!(
                                "isolated_hat={} event_hat={}",
                                isolated_hat.as_str(),
                                scope_hat.as_str()
                            )),
                        })
                        // 2026-06-13-004 P0 #4: clone the retry_key
                        // here so we can both dedup it against
                        // `envelopes_written_this_turn` AND move
                        // it into `env_builder` below.
                        .retry_key(scope_drop_retry_key.clone());
                    if let Some(session_id) = self.diagnostics().session_id() {
                        env_builder = env_builder.session_id(session_id);
                    }
                    let envelope = env_builder.build();
                    // 2026-06-13-004 P0 #4 review fix (U7 envelope
                    // disk storm): per-turn dedup of the retry_key
                    // so multiple identical scope drops in the
                    // same `process_parse_result` call collapse to
                    // a single envelope write. The bus
                    // `event.isolation.boundary_violation` event
                    // (emitted earlier in this branch) still
                    // fires for each, so operators see every
                    // occurrence in `orchestration.jsonl`; the
                    // dedup only shields `recovery.jsonl` from
                    // the 8x write rate that wave-batches
                    // produce. Distinct scope drops (different
                    // wave_id / topic / scope_hat) produce
                    // distinct retry_keys and still write
                    // distinct envelopes, so ADV-1's
                    // namespace fix is preserved.
                    if !envelopes_written_this_turn.insert(scope_drop_retry_key.clone()) {
                        debug!(
                            retry_key = %scope_drop_retry_key,
                            topic = %event.topic,
                            "U7: per-turn dedup dropped identical scope-drop envelope"
                        );
                        continue;
                    }
                    // 2026-06-13-004 U7: copy out the immutable
                    // borrow of `isolated_hat` before we take a
                    // mutable borrow of `self` to record the
                    // envelope. E0502 would otherwise block the
                    // call (Rust cannot prove the immutable
                    // borrow ends before the mutable one starts
                    // when both go through `self`).
                    let isolated_hat_str = isolated_hat.as_str().to_string();
                    let scope_hat_str = scope_hat.as_str().to_string();
                    let topic_str = event.topic.to_string();
                    self.record_recovery_envelope(
                        &envelope,
                        vec![format!(
                            "scope_drop hat={} topic={} current_isolated_hat={}",
                            scope_hat_str, topic_str, isolated_hat_str
                        )],
                    );
                    // source hat so the next turn the rejected hat
                    // gets reactivated with explicit recovery context.
                    // Without this hook, an isolated hat that emits an
                    // out-of-scope terminal-style topic (e.g. an
                    // unauthorized `LOOP_COMPLETE`) would never see a
                    // recovery signal — the loop would simply drop the
                    // event and stay silent, breaking R8 / R11
                    // (targeted task.resume contract).  The recovery
                    // payload names the rejected topic and the allowed
                    // publishes so the agent can re-emit a legal one
                    // on its next turn.
                    let allowed: Vec<String> = self
                        .registry
                        .get_config(isolated_hat)
                        .map(|c| c.publishes.iter().map(|t| t.to_string()).collect())
                        .unwrap_or_default();
                    // P1 finding #6: dedup — if the target hat already
                    // has a pending `task.resume` (with the same
                    // `stage=isolated_scope` origin), skip injection.
                    // Each isolated violation turn would otherwise
                    // stack duplicate recovery events on the same
                    // queue, causing event-storm behaviour in loops
                    // that repeatedly re-attempt the same illegal
                    // publish (e.g. an agent that never learns). The
                    // dedup key is (target_hat, topic=task.resume) so
                    // multiple distinct source-hats can still each
                    // receive one recovery event per turn.
                    let already_pending_recovery = self
                        .bus
                        .peek_pending(isolated_hat)
                        .map(|events| events.iter().any(|e| e.topic.as_str() == "task.resume"))
                        .unwrap_or(false);
                    if !already_pending_recovery {
                        // 2026-06-14-004 U2: record the rejection key and check circuit breaker.
                        // We record BEFORE checking exhaustion so the count includes this attempt.
                        // The key includes wave_id/wave_index for wave events (distinguishes
                        // 8 different wave workers), so exhaustion means the SAME worker keeps
                        // hitting the same violation across iterations.
                        let count = self.state.record_rejection_key(&scope_drop_retry_key);
                        if self.state.rejection_key_is_exhausted(&scope_drop_retry_key) {
                            // Circuit breaker tripped: do NOT inject task.resume.
                            // The hat has exceeded U2_REJECTION_RETRY_LIMIT retries.
                            // Store the original termination reason in LoopState so
                            // `check_termination()` can return it with non-normalized
                            // hat/topic for clear diagnostics (R-C).
                            warn!(
                                key = %scope_drop_retry_key,
                                hat = %isolated_hat.as_str(),
                                topic = %event.topic,
                                count = count,
                                "Scope violation circuit breaker: no more task.resume injections for key '{}'",
                                scope_drop_retry_key
                            );
                            self.state.scope_violation_circuit_breaker_tripped =
                                Some(TerminationReason::ScopeViolationCircuitBreakerTripped {
                                    hat: isolated_hat.as_str().to_string(),
                                    topic: event.topic.to_string(),
                                    violation_count: count,
                                    allowed_topics: allowed.clone(),
                                });
                            // Publish a terminal diagnostic event so operators and
                            // `ralph diagnose` see what happened.
                            let breaker_event = Event::new(
                                "loop.terminate",
                                format!(
                                    "{{\"reason\":\"scope_violation_circuit_breaker_tripped\",\"hat\":\"{}\",\"topic\":\"{}\",\"violation_count\":{},\"allowed_topics\":{:?}}}",
                                    isolated_hat.as_str(),
                                    event.topic,
                                    count,
                                    allowed
                                ),
                            )
                            .with_target(isolated_hat.clone());
                            self.bus.publish(breaker_event);
                            continue;
                        }
                        // P1 finding #10: build the payload through the
                        // shared helper so the format matches the
                        // rejection pipeline and downstream consumers
                        // (U6 responder, U5 drift) can rely on a
                        // single schema.  The helper expects a
                        // `Rejection` — for the U2 isolated_scope path
                        // we construct one inline.
                        let rejection = crate::event_loop::rejection::Rejection {
                            stage: crate::event_loop::rejection::RejectionStage::Origin,
                            source_hat: Some(isolated_hat.to_string()),
                            business_hat: None,
                            topic: event.topic.to_string(),
                            violation: format!(
                                "hat '{}' cannot publish '{}' in isolated mode",
                                isolated_hat.as_str(),
                                event.topic
                            ),
                            retry_key: format!(
                                "{}:{}:isolated_scope",
                                isolated_hat.as_str(),
                                event.topic
                            ),
                            retry_eligible: true,
                            non_retryable_reason: None,
                            target_hat: Some(isolated_hat.to_string()),
                        };
                        // R5 (2026-06-14-003 plan): carry the wave
                        // metadata (when present) so the resumed hat
                        // can recover the wave context.  Plan AC7
                        // requires the resume payload to include
                        // `wave_id` / `wave_index` / `wave_total` for
                        // wave events; this branch was previously
                        // dropping them by passing `None`.
                        let wc =
                            crate::event_loop::rejection::WaveContextForResume::from_reader_event(
                                &event,
                            );
                        let resume_payload =
                            crate::event_loop::rejection::build_task_resume_payload(
                                &rejection,
                                &allowed,
                                &[],
                                None,
                                None,
                                wc.as_ref(),
                            );
                        let recovery = Event::new("task.resume", resume_payload)
                            .with_target(isolated_hat.clone());
                        let recovery_target = recovery.target.clone();
                        let recovery_payload = recovery.payload.clone();
                        self.bus.publish(recovery);
                        // P1 finding #1: also push the synthetic
                        // `task.resume` into the local `accepted` vector
                        // so the JSONL-derived `accepted_events` (used
                        // downstream to compute `had_events` for the
                        // turn) sees the recovery. Without this, a
                        // turn that contains only a rejected out-of-scope
                        // event would otherwise yield `had_events =
                        // false`, causing the loop runner to treat the
                        // turn as empty and not advance. The recovery
                        // stays targeted to the source hat via the
                        // bus.publish above — the `accepted` push only
                        // ensures the turn is reported as active.
                        //
                        // `accepted` here is `Vec<JsonlEvent>`
                        // (= `event_reader::Event`); we build one from
                        // the recovery's fields.
                        let resume_jsonl = crate::event_reader::Event {
                            topic: "task.resume".to_string(),
                            payload: Some(recovery_payload),
                            ts: chrono::Utc::now().to_rfc3339(),
                            hat: None,
                            triggered: recovery_target.map(|t| t.to_string()),
                            source: None,
                            wave_id: None,
                            wave_index: None,
                            wave_total: None,
                        };
                        accepted.push(resume_jsonl);
                    }
                    continue;
                }

                // 2026-06-13-004 U3: a `wave_id` group of result
                // events is ONE business emission, not N. The merge
                // layer (see `merge_wave_results_to_events_file`)
                // stamps every record with the originating `wave_id`,
                // so a batch of N `review.dimension.done` from
                // workers in the same wave must be admitted in full
                // even after the first business event was already
                // accepted in the same turn. Without this carve-out
                // the 8/8 wave result would be reduced to 1/8 by the
                // per-turn budget, which is the upstream cause of
                // the 2026-06-13 incident. We track the *set* of
                // wave_ids already accepted in this turn so a
                // distinct second wave still gets rejected (one
                // business emission per turn) but a continuation of
                // the same wave does not.
                let same_wave_continuation = event.wave_id.as_deref().is_some_and(|wid| {
                    first_wave_id_accepted
                        .as_ref()
                        .and_then(|inner| inner.as_deref())
                        == Some(wid)
                });

                // 2026-06-15-003 fix U1: `plan-gate` Path A dual-publish.
                // `queue.advance` followed by `work.ready` is the only
                // legitimate two-business-event sequence in isolated mode
                // — `queue.advance` advances the step counter, `work.ready`
                // carries the execution context that wakes the executor.
                // Without this carve-out the second event is dropped, the
                // executor is never scheduled, and the loop eventually
                // hits `consecutive_same_signature >= 3` → LoopStale.
                // See docs/plans/2026-06-15-003-...-plan.md and
                // docs/report/2026-06-15-plan-gate-dual-publish-...-diagnosis.md.
                // Scope is intentionally narrow: ordered pair, exact topics,
                // same hat, and only ONE extra event — the third business event in
                // the same turn is still dropped (sticky budget).
                // Hat check prevents cross-hat false positives: executor's
                // queue.advance does not豁免 coordinator's work.ready and vice versa.
                let is_dual_publish_step_handoff =
                    event.topic.as_str() == "work.ready"
                        && accepted.last().is_some_and(|prev| {
                            prev.topic.as_str() == "queue.advance"
                                && prev.hat.as_ref() == event.hat.as_ref()
                        });

                if first_business_event_accepted
                    && !same_wave_continuation
                    && !is_dual_publish_step_handoff
                {
                    warn!(
                        topic = %event.topic,
                        "Isolated mode: extra business event dropped — only one per turn"
                    );
                    let diagnostic = Event::new(
                        "event.isolation.boundary_violation",
                        format!(
                            "Isolated mode: dropped extra event '{}' — only one business event per turn allowed",
                            event.topic
                        ),
                    );
                    self.bus.publish(diagnostic);
                } else {
                    // 2026-06-13-004 U3: capture the wave_id *before*
                    // the event is moved into `accepted`, so we can
                    // remember the first wave's identity to
                    // discriminate continuations from distinct
                    // second waves.
                    let wave_id_to_record = event.wave_id.clone();
                    accepted.push(event);
                    if !first_business_event_accepted {
                        first_business_event_accepted = true;
                    }
                    if first_wave_id_accepted.is_none() {
                        first_wave_id_accepted = Some(wave_id_to_record);
                    }
                    // U3 P0 fix: write the sticky per-turn budget flag so
                    // `check_default_publishes` (which runs later in the same
                    // turn when JSONL had zero events, or earlier when JSONL
                    // had business events) sees a consistent view.
                    self.state.isolated_turn_business_event_accepted = true;
                }
            }
            accepted
        } else if self.config.event_loop.enforce_hat_scope {
            // Coordinator mode: scope enforcement with active_hats
            let active_hats = self.state.last_active_hat_ids.clone();
            let completion = &self.config.event_loop.completion_promise;
            let cancellation = &self.config.event_loop.cancellation_promise;
            let (in_scope, out_of_scope): (Vec<_>, Vec<_>) =
                result.events.into_iter().partition(|event| {
                    if active_hats.is_empty() {
                        // No active hat: only allow control topics and completion promise.
                        // This prevents arbitrary business events from entering the pipeline
                        // without hat provenance between orchestration cycles.
                        crate::event_origin::is_jsonl_control_topic(
                            event.topic.as_str(),
                            cancellation,
                        ) || event.topic.as_str() == completion.as_str()
                    } else {
                        active_hats
                            .iter()
                            .any(|hat_id| self.registry.can_publish(hat_id, event.topic.as_str()))
                    }
                });

            for event in &out_of_scope {
                let violation_hat = active_hats.first().map(|h| h.as_str()).unwrap_or("unknown");
                warn!(
                    active_hats = ?active_hats,
                    topic = %event.topic,
                    "Scope violation: active hat(s) cannot publish this topic — dropping event"
                );
                let violation_topic = format!("{}.scope_violation", violation_hat);
                let violation_payload = format!(
                    "Attempted to publish '{}': {}",
                    event.topic,
                    event.payload.clone().unwrap_or_default()
                );
                let violation = Event::new(violation_topic, violation_payload);
                self.bus.publish(violation);
            }

            in_scope
        } else {
            result.events
        };
        // --- End scope enforcement ---

        // --- Origin guard: validate JSONL event provenance before bus publication ---
        // Events from JSONL are untrusted until provenance and scope checks accept them.
        // This rejects no-hat business events, unknown-hat events, and out-of-scope topics.
        let (mut events, origin_rejections) = filter_events_by_origin(
            events,
            &self.registry,
            &self.config.event_loop.cancellation_promise,
            &self.config.event_loop.completion_promise,
        );
        let had_origin_rejections = !origin_rejections.is_empty();
        // --- End origin guard ---

        // --- Topic format check (U5 / R9): reject unknown topics before policy ---
        // Builds a whitelist from hat publishes + system/control topics.
        // Rejected topics produce a recovery signal but NO retry (R10).
        // Only active when event_policy is enabled AND hats are configured
        // (no hats = no whitelist to validate against, skip check).
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
            && !self.config.hats.is_empty()
        {
            use std::collections::HashSet;
            let allowed_topics: HashSet<String> = crate::event_policy::build_allowed_topics(
                &self.config.hats,
                &self.config.event_loop.completion_promise,
                self.config.event_loop.event_policy.as_ref(),
            );
            let (topic_format_ok, topic_format_rejections): (Vec<_>, Vec<_>) =
                events.into_iter().partition(|event| {
                    if crate::event_policy::is_system_topic(&event.topic) {
                        return true;
                    }
                    crate::event_policy::check_topic_format(&event.topic, &allowed_topics).is_none()
                });
            if !topic_format_rejections.is_empty() {
                // R10: convert each rejected event into a structured
                // RecoveryDiagnosisEnvelope and write it to
                // recovery.jsonl. We also still publish the legacy
                // `event.topic_format.rejected` diagnostic event so
                // operators reading the bus see the same signal they
                // always have — the journal entry is the new layer on
                // top, not a replacement.
                let allowed_list: Vec<String> = allowed_topics.iter().cloned().collect();
                for event in &topic_format_rejections {
                    warn!(
                        topic = %event.topic,
                        hat = ?event.hat,
                        "Topic format rejection: unknown topic not in whitelist"
                    );
                    // Backwards-compat diagnostic event (R10: no retry).
                    let diagnostic = Event::new(
                        "event.topic_format.rejected",
                        format!(
                            "TOPIC_FORMAT_REJECTED: '{}' is not in the whitelist of known topics. \
                             This event will not be retried.",
                            event.topic
                        ),
                    );
                    self.bus.publish(diagnostic);
                    // New: write the recovery journal entry. Without
                    // this, R10's "only write recovery signal"
                    // promise is silently dropped.
                    Self::log_topic_format_rejection(
                        self,
                        event.topic.as_str(),
                        event.hat.as_deref(),
                        &allowed_list,
                    );
                }
            }
            events = topic_format_ok;
        }
        // --- End topic format check ---

        // --- Event policy validation: check typed payload schema ---
        // Inserted after scope enforcement, before workflow guard validation
        let mut had_policy_rejections = false;
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            // Unit 2 (2026-06-16-002 plan): `apply_event_policy_validation`
            // now requires `&mut LoopState` (to drive the recoverable
            // budget counters and to surface exhaustions to the
            // runner).  To avoid a double `&mut self.state` borrow
            // (the `policy_runtime_state` slot is **inside**
            // `self.state`), we **take** the `Option<PolicyRuntimeState>`
            // out of `self.state` for the duration of the call, then
            // put it back.  This keeps the borrow checker happy and
            // also matches the original (pre-Unit-2) call site's
            // borrow pattern.
            let mut policy_state: PolicyRuntimeState = self
                .state
                .policy_runtime_state
                .take()
                .unwrap_or_default();
            // U6: build source/target hat indexes for payload contract
            // violation attribution.
            let mut source_hats_by_topic: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            let mut target_hats_by_topic: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for (hat_id, hat_config) in &self.config.hats {
                for t in &hat_config.publishes {
                    source_hats_by_topic
                        .entry(t.clone())
                        .or_default()
                        .push(hat_id.clone());
                }
                for t in &hat_config.triggers {
                    target_hats_by_topic
                        .entry(t.clone())
                        .or_default()
                        .push(hat_id.clone());
                }
            }
            // Unit 2 (2026-06-16-002 plan) take-3: the policy
            // validator no longer takes `&mut LoopState`; it
            // borrows `review_step_tracker` from `self.state` via
            // a `&mut self.state.review_step_tracker` field
            // reborrow that lives only for the call.  NLL
            // recognizes the disjoint field as borrowable because
            // the `&mut LoopState` parameter was removed (the
            // counter bookkeeping moved to the caller).
            let mut review_step_tracker =
                std::mem::take(&mut self.state.review_step_tracker);
            let mut policy_result = apply_event_policy_validation(
                events,
                policy_config,
                &mut policy_state,
                &mut review_step_tracker,
                &mut self.bus,
                policy_config
                    .completion_after_terminal
                    .write_diagnostic_event,
                &source_hats_by_topic,
                &target_hats_by_topic,
                &self.registry,
            );
            // Restore the `ReviewStepTracker` and put the
            // `PolicyRuntimeState` back so the next call sees the
            // same counters.
            self.state.review_step_tracker = review_step_tracker;
            self.state.policy_runtime_state = Some(policy_state);
            had_policy_rejections = !policy_result.policy_rejections.is_empty();
            // Unit 2 (2026-06-16-002 plan) take-3: process the
            // recoverable candidates ourselves.  The validator
            // did not call `record_recoverable_rejection_key`
            // because it does not own `&mut LoopState`.  For
            // each candidate we (a) bump the counter and (b)
            // push a `RecoverableExhaustion` into the buffer
            // when the post-increment count crosses the budget.
            for candidate in policy_result.recoverable_candidates.drain(..) {
                let (count, exhausted) = self
                    .state
                    .record_recoverable_rejection_key(
                        &candidate.hat,
                        &candidate.topic,
                        candidate.reason_class.as_str(),
                    );
                if exhausted {
                    self.state
                        .recoverable_exhaustion_buffer
                        .push(RecoverableExhaustion {
                            hat: candidate.hat,
                            topic: candidate.topic,
                            reason_class: candidate.reason_class,
                            count,
                        });
                }
            }

            // WRC-U4 (2026-06-12-003 / KTD-13 / F2): for every
            // accepted event whose topic has a unique consumer in
            // the HandoffIndex, record the handoff with the
            // configured dispatch deadline. The tracker is a no-op
            // in coordinator mode (`HandoffIndex::consumer_of`
            // returns None there) and for non-handoff topics. The
            // `Instant::now()` is captured at policy-accept time,
            // not at bus.publish time, so a slow downstream
            // validation step does not skew the deadline. Policy
            // rejections (anything that did not land in
            // `policy_result.events`) are intentionally NOT
            // recorded — those events are dropped or held, and
            // tracking them would create a phantom escalation.
            //
            // This loop runs **before** `events = policy_result.events`
            // because that line moves the field out of the
            // result; we still need to borrow the events vector
            // here. The cost is one extra field access (the
            // borrow ends at the end of this block).
            for accepted in &policy_result.events {
                if let Some(consumer) = self.handoff_index.consumer_of(&accepted.topic) {
                    // JsonlEvent has no stable `id` field; use
                    // `ts + topic` as the unique key for the
                    // tracker's pending map. Two events on the
                    // same topic with the same `ts` would
                    // collide, but the JSONL reader increments
                    // `ts` per line so the practical collision
                    // rate is zero in normal use.
                    let event_id = format!("{}:{}", accepted.ts, accepted.topic);
                    self.state.handoff_tracker.on_handoff_accepted(
                        accepted.topic.clone(),
                        consumer.to_string(),
                        event_id,
                        std::time::Instant::now(),
                    );
                }
            }

            // Unit 3 (2026-06-16-002 plan): flip the bootstrap
            // gate when the coordinator hands off a terminal
            // bootstrap event.  `policy_result.events` carries
            // every policy-accepted event, including those
            // routed through topic-deny and review-step gates
            // (the `Accept` arm of `PolicyDecision`).  Plan-gate
            // `work.ready` (with `reviewed_task_id`) is filtered
            // out by the helper, matching the
            // `ReviewStepTracker::check_semantic_gates` rule at
            // `review_step_state.rs:174-191`.
            self.update_bootstrap_flags_from_accepted(&policy_result.events);

            // U4 (2026-06-17-003 plan): maintain the per-loop
            // `work.done` dedup set. For each policy-accepted
            // event, either (a) record the dedup key (for
            // `work.done`) or (b) prune the step bucket (for
            // step-boundary events `queue.advance`, `review.failed`,
            // `fix.applied`). The set lives in
            // `LoopState::work_done_seen_tasks`; the in-batch
            // mirror lives in `PolicyRuntimeState::work_done_seen_keys`
            // and is consulted by `validate_event_with_hat` for
            // per-batch dedup.
            for accepted in &policy_result.events {
                match accepted.topic.as_str() {
                    "work.done" => {
                        if let Some(p) = accepted.payload.as_deref()
                            && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
                            && let (Some(pn), Some(st), Some(ti)) = (
                                obj.get("plan_name").and_then(|v| v.as_str()),
                                obj.get("step").and_then(|v| v.as_str()),
                                obj.get("task_id").and_then(|v| v.as_str()),
                            )
                        {
                            let key = LoopState::work_done_dedup_key(pn, st, ti);
                            self.state.work_done_seen_tasks.insert(key);
                        }
                    }
                    "queue.advance" | "review.failed" | "fix.applied" => {
                        if let Some(p) = accepted.payload.as_deref()
                            && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
                        {
                            let plan_name = obj
                                .get("plan_name")
                                .and_then(|v| v.as_str())
                                .map(String::from);
                            let step = obj
                                .get("completed_step")
                                .or_else(|| obj.get("step"))
                                .and_then(|v| v.as_str())
                                .map(String::from);
                            if let (Some(pn), Some(st)) = (plan_name, step) {
                                self.state.prune_work_done_bucket(&pn, &st);
                            }
                        }
                    }
                    _ => {}
                }
            }

            events = policy_result.events;

            // Write hold artifact if policy hold was triggered
            if policy_result.hold_triggered
                && let Err(e) = self.write_hold_artifact(policy_result.hold_reason.as_deref())
            {
                warn!(error = %e, "Failed to write hold artifact");
            }
            // U6: capture the first payload contract violation for the
            // loop runner to surface.
            if payload_contract_violation.is_none() {
                payload_contract_violation = policy_result.payload_contract_violation;
            }
        }
        // --- End event policy validation ---

        // --- State machine validation: enforce instance lifecycle rules ---
        // Inserted after policy validation, before workflow guards and record_event() + bus.publish()
        if let Some(ref sm_config) = self.config.event_loop.state_machine
            && sm_config.enabled
        {
            let sm_state = self
                .state
                .state_machine_runtime_state
                .get_or_insert_with(StateMachineRuntimeState::default);

            let (accepted, rejected): (Vec<_>, Vec<_>) = events.into_iter().partition(|event| {
                let topic = event.topic.as_str();
                let payload = event.payload.as_deref();
                let decision = sm_state.validate_event(topic, payload, sm_config);

                match decision {
                    StateMachineDecision::Accept { .. } => true,
                    StateMachineDecision::Reject { finding } => {
                        // Publish diagnostic event for rejection
                        let diagnostic = Event::new(
                            "event.state_machine.rejected",
                            serde_json::to_string(&finding)
                                .unwrap_or_else(|_| finding.reason.clone()),
                        );
                        self.bus.publish(diagnostic);
                        false
                    }
                    StateMachineDecision::Ignore { finding } => {
                        // Silently ignore (no bus publish, no record)
                        let diagnostic = Event::new(
                            "event.state_machine.ignored",
                            serde_json::to_string(&finding)
                                .unwrap_or_else(|_| finding.reason.clone()),
                        );
                        self.bus.publish(diagnostic);
                        false
                    }
                    StateMachineDecision::DiagnosticOnly { finding } => {
                        // Just log, event still passes through
                        let diagnostic = Event::new(
                            "event.state_machine.diagnostic",
                            serde_json::to_string(&finding)
                                .unwrap_or_else(|_| finding.reason.clone()),
                        );
                        self.bus.publish(diagnostic);
                        true
                    }
                }
            });

            // Log rejected count for metrics
            if !rejected.is_empty() {
                debug!(
                    rejected_count = rejected.len(),
                    "State machine rejected events"
                );
            }

            events = accepted;
        }
        // --- End state machine validation ---

        // --- Workflow guard validation: reject out-of-order events ---
        // Legacy linear guards run after the state machine. When the state machine
        // is enabled, branch-close topics that it accepts are lifecycle-complete
        // even if they are not part of a linear guard chain.
        let workflow_guards = self.config.event_loop.workflow_guards.as_ref();
        let state_machine_enabled = self
            .config
            .event_loop
            .state_machine
            .as_ref()
            .is_some_and(|sm| sm.enabled);
        // U4: workflow guard now returns `WorkflowGuardOutcome` so we
        // can write a recovery envelope per rejection. The accepted
        // events keep flowing exactly as before.
        let events: Vec<JsonlEvent> = match (workflow_guards, state_machine_enabled) {
            (Some(guards), false) if !guards.chains.is_empty() => {
                let outcome = apply_workflow_guard_validation(
                    events,
                    guards,
                    &mut self.state.workflow_progress,
                    &mut self.bus,
                    &self.state.review_step_tracker,
                );
                for rejection in &outcome.rejections {
                    Self::log_workflow_guard_rejection(&mut *self, rejection);
                }
                outcome.accepted_events
            }
            _ => events,
        };
        // --- End workflow guard validation ---

        // Update policy runtime state for events that survived all validation layers
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            let policy_state = self
                .state
                .policy_runtime_state
                .get_or_insert_with(PolicyRuntimeState::default);
            for event in &events {
                if policy_config.terminal_topics.contains(&event.topic) {
                    policy_state.terminal_observed = true;
                }
            }
        }

        // --- Execution contract validation (U5): validate work.done before publishing ---
        // This runs after all other validation layers, before record/publish.
        // Contract rejection publishes diagnostic + guidance but does NOT record/publish the original.
        let execution_contracts = self.config.event_loop.execution_contracts.as_ref();
        // Track raw event counts before contract filtering for missing-event gate logic
        let contract_validation_input_count = events.len();
        let mut contract_rejections: Vec<ExecutionContractFinding> = Vec::new();
        let contracts_enabled = execution_contracts.as_ref().is_some_and(|c| c.enabled);
        let events = if contracts_enabled {
            let contracts = execution_contracts.unwrap();
            let current_loop_id = self.current_loop_id_for_contract();
            let workspace_root = std::path::Path::new(&self.config.core.workspace_root);
            let tasks_path = self.tasks_path();

            let mut accepted: Vec<JsonlEvent> = Vec::with_capacity(events.len());
            for event in events {
                // Check if this topic has a contract rule
                if let Some(rule) = contracts.rules.get(event.topic.as_str()) {
                    let proto_event =
                        Event::new(event.topic.as_str(), event.payload.as_deref().unwrap_or(""));
                    // Provenance: prefer the hat the event declared on its
                    // own JSONL `hat` field (most accurate — it identifies
                    // the hat that *emitted* the event).  Fall back to the
                    // runner's last active hat when the JSONL line did not
                    // carry one (legacy fixtures / log-only emissions).
                    // The provenance is stamped onto every
                    // ExecutionContractFinding so the U2 recovery path can
                    // route `task.resume` to the actual source hat rather
                    // than the runner's current display hat.
                    let active_business_hat =
                        self.state.last_active_hat_ids.first().map(|h| h.as_str());
                    let event_provenance: Option<&str> = match event.hat.as_deref() {
                        Some("ralph") => active_business_hat.or(Some("ralph")),
                        Some(hat) => Some(hat),
                        None => active_business_hat,
                    };
                    let decision = validate_execution_contract(
                        &proto_event,
                        rule,
                        workspace_root,
                        current_loop_id.as_str(),
                        &tasks_path,
                        event_provenance,
                        &DefaultGitEvidenceProvider,
                        self.state.loop_start_sha.as_deref(),
                    );
                    match decision {
                        ExecutionContractDecision::Accept => {
                            accepted.push(event);
                        }
                        ExecutionContractDecision::Reject(findings) => {
                            // Publish rejection diagnostic and guidance, do NOT accept the event
                            let finding = &findings[0];
                            warn!(
                                topic = %event.topic,
                                violation = ?finding.kind,
                                "Execution contract rejected event"
                            );

                            // Targeted contract recovery (2026-06-04 plan U2):
                            // The rejected event must NOT advance downstream hats,
                            // but the source hat must be told to retry. Publish a
                            // `task.resume` with `target=source_hat` so the next
                            // prompt activates the responsible hat, not the Ralph
                            // fallback.
                            let source_hat_str = finding.source_hat.as_deref();
                            let mut retry_target: Option<HatId> = None;
                            let mut no_retry_reason: Option<String> = None;
                            if let Some(hat_id_str) = source_hat_str {
                                if hat_id_str != "ralph" {
                                    let hat_id = HatId::new(hat_id_str);
                                    match self.registry.get(&hat_id) {
                                        None => {
                                            no_retry_reason = Some(format!(
                                                "source hat '{}' not registered",
                                                hat_id_str
                                            ));
                                        }
                                        Some(_) => {
                                            let can_retry = self
                                                .registry
                                                .can_publish(&hat_id, event.topic.as_str());
                                            let can_fail =
                                                self.registry.can_publish(&hat_id, "work.failed");
                                            if !can_retry && !can_fail {
                                                no_retry_reason = Some(format!(
                                                    "source hat '{}' cannot publish '{}' or 'work.failed'",
                                                    hat_id_str,
                                                    event.topic.as_str()
                                                ));
                                            } else {
                                                retry_target = Some(hat_id);
                                            }
                                        }
                                    }
                                } else {
                                    no_retry_reason = Some(
                                        "no business hat available for fallback ralph".to_string(),
                                    );
                                }
                            } else {
                                no_retry_reason =
                                    Some("no source hat recorded on event or in state".to_string());
                            }

                            if let Some(hat_id) = &retry_target {
                                let original_trigger =
                                    self.state.last_activation_events.iter().rev().find(
                                        |trigger| {
                                            self.registry.get_config(hat_id).is_some_and(|config| {
                                                config.trigger_topics().iter().any(|topic| {
                                                    topic.matches_str(trigger.topic.as_str())
                                                })
                                            })
                                        },
                                    );
                                let retry_payload = serde_json::json!({
                                    "rejected_topic": event.topic.as_str(),
                                    "reason": finding.message,
                                    "finding_kind": format!("{:?}", finding.kind),
                                    "required_action": format!(
                                        "Fix the issue and emit '{}' again with correct payload, or emit 'work.failed' if unrecoverable.",
                                        event.topic.as_str()
                                    ),
                                    "original_payload": event.payload.as_deref().unwrap_or(""),
                                    "original_trigger_topic": original_trigger
                                        .map(|trigger| trigger.topic.as_str()),
                                    "original_trigger_payload": original_trigger
                                        .map(|trigger| {
                                            serde_json::from_str::<serde_json::Value>(
                                                trigger.payload.as_str(),
                                            )
                                            .unwrap_or_else(|_| {
                                                serde_json::Value::String(
                                                    trigger.payload.clone(),
                                                )
                                            })
                                        }),
                                    "retry_publish_topics": [event.topic.as_str(), "work.failed"],
                                    "contract_finding": finding,
                                });
                                let retry_event =
                                    Event::new("task.resume", retry_payload.to_string())
                                        .with_target(hat_id.clone());
                                debug!(
                                    target = %hat_id.as_str(),
                                    topic = %event.topic.as_str(),
                                    "Publishing targeted contract recovery event to source hat"
                                );
                                self.bus.publish(retry_event);
                            } else if let Some(reason) = &no_retry_reason {
                                warn!(
                                    topic = %event.topic.as_str(),
                                    reason = %reason,
                                    "No safe retry target for rejected event; recovery is human.guidance only"
                                );
                            }

                            // Publish structured diagnostic (now carries
                            // retry_target and no_retry_reason for observability).
                            let diagnostic_topic = rule.reject.diagnostic_topic.clone();
                            let diagnostic_payload = serde_json::json!({
                                "topic": event.topic.as_str(),
                                "finding": findings,
                                "rejected_at": chrono::Utc::now().to_rfc3339(),
                                "retry_target": retry_target.as_ref().map(|h| h.as_str()),
                                "no_retry_reason": no_retry_reason,
                            });
                            let diagnostic_event = Event::new(
                                diagnostic_topic.as_str(),
                                diagnostic_payload.to_string(),
                            );
                            self.bus.publish(diagnostic_event);

                            // Publish human-readable guidance
                            let guidance_payload = format!(
                                "Execution contract rejection for '{}': {}\n\n\
                                 To proceed, either:\n\
                                 1. Fix the issue and emit '{}' again with correct payload, OR\n\
                                 2. Emit 'work.failed' if the work cannot be completed.",
                                event.topic.as_str(),
                                finding.message,
                                event.topic.as_str(),
                            );
                            let guidance_event =
                                Event::new(rule.reject.guidance_topic.as_str(), guidance_payload);
                            self.bus.publish(guidance_event);

                            contract_rejections.extend(findings.iter().cloned());
                        }
                    }
                } else {
                    // No contract rule for this topic — pass through
                    accepted.push(event);
                }
            }
            accepted
        } else {
            events
        };
        // --- End execution contract validation ---

        // Calculate had_raw_events and had_rejected_events for missing-event gate logic
        // had_raw_events: events that passed through contract validation (accepted OR rejected)
        // had_rejected_events: events that were rejected by contract validation
        let had_rejected_events =
            had_origin_rejections || had_policy_rejections || !contract_rejections.is_empty();
        let had_raw_events = if contracts_enabled {
            // Events that went through contract validation: accepted + rejected
            // events.len() here is accepted.len() (passed or no-rule events)
            events.len() + contract_rejections.len() > 0
        } else {
            // Contracts disabled: all events passed through
            contract_validation_input_count > 0
        };

        let mut has_orphans = false;

        // Validate and transform events (apply backpressure for build.done)
        let mut validated_events = Vec::new();
        let completion_topic = self.config.event_loop.completion_promise.as_str();
        let cancellation_topic = self.config.event_loop.cancellation_promise.clone();
        let total_events = events.len();
        let mut completion_seen_in_batch = false;
        let policy_config_ref = self.config.event_loop.event_policy.as_ref();
        let write_diagnostic = policy_config_ref
            .map(|c| c.completion_after_terminal.write_diagnostic_event)
            .unwrap_or(false);
        let mut accepted_log_events = Vec::new();
        macro_rules! accept_event {
            ($accepted:expr) => {{
                let accepted = $accepted;
                accepted_log_events.push(accepted.clone());
                validated_events.push(accepted);
            }};
        }

        for (index, event) in events.into_iter().enumerate() {
            let payload = event.payload.clone().unwrap_or_default();

            // Detect loop.cancel — unconditional graceful termination
            if !cancellation_topic.is_empty() && event.topic.as_str() == cancellation_topic {
                info!(
                    payload = %payload,
                    "loop.cancel event detected — scheduling graceful termination"
                );
                self.state.cancellation_requested = true;
                accepted_log_events.push(Event::new(event.topic.as_str(), &payload));
                // Continue processing remaining events (they may contain cleanup info)
                continue;
            }

            if event.topic == completion_topic {
                if self.state.completion_honored {
                    debug!("Completion event already handled, ignoring duplicate");
                    continue;
                }
                // Completion event is accepted regardless of position in batch.
                // Events AFTER it in the same batch are protected by the completion guard.
                self.state.completion_requested = true;
                completion_seen_in_batch = true;
                accepted_log_events.push(Event::new(event.topic.as_str(), &payload));
                self.diagnostics.log_orchestration(
                    self.state.iteration,
                    "jsonl",
                    crate::diagnostics::OrchestrationEvent::EventPublished {
                        topic: event.topic.clone(),
                    },
                );
                info!(
                    topic = %event.topic,
                    position = index,
                    batch_size = total_events,
                    "Completion event detected in JSONL"
                );
                continue;
            }

            // Same-batch completion guard: events after a completion topic in the
            // same batch are subject to completion_after_terminal filtering.
            if completion_seen_in_batch {
                if let Some(ref policy_config) = policy_config_ref
                    && policy_config.enabled
                {
                    if let Some(decision) =
                        check_completion_guard(&event.topic, policy_config, true)
                    {
                        match &decision {
                            PolicyDecision::Block(finding) => {
                                if write_diagnostic {
                                    self.bus.publish(Event::new(
                                        "event.completion.blocked",
                                        format!(
                                            "Same-batch completion guard blocked '{}': {}",
                                            event.topic, finding.message
                                        ),
                                    ));
                                }
                            }
                            PolicyDecision::Ignore(finding) => {
                                if write_diagnostic {
                                    self.bus.publish(Event::new(
                                        "event.completion.ignored",
                                        format!(
                                            "Same-batch completion guard ignored '{}': {}",
                                            event.topic, finding.message
                                        ),
                                    ));
                                }
                            }
                            PolicyDecision::Warn(findings) => {
                                for finding in findings {
                                    self.bus.publish(Event::new(
                                        "event.policy_warning",
                                        format!(
                                            "Same-batch completion guard warning for '{}': {}",
                                            event.topic, finding.message
                                        ),
                                    ));
                                }
                                accept_event!(Event::new(event.topic.as_str(), &payload));
                            }
                            _ => {}
                        }
                        continue;
                    }
                }
            }

            if event.topic == "build.done" {
                // P4: structured JSON evidence is the preferred path. If
                // the payload parses as a JSON object we run the strict
                // schema check first; otherwise we fall back to the
                // legacy text "tests: pass" parsing.
                let trimmed = payload.trim();
                let json_status: Option<Result<BuildStatus, String>> = if trimmed.starts_with('{') {
                    Some(parse_backpressure_json(
                        trimmed,
                        &self.config.core.workspace_root,
                    ))
                } else {
                    None
                };
                if let Some(result) = json_status {
                    match result {
                        Ok(BuildStatus::Pass) => {
                            accept_event!(Event::new(event.topic.as_str(), &payload));
                        }
                        Ok(BuildStatus::Fail { reason, missing }) => {
                            warn!(
                                missing = ?missing,
                                "build.done rejected: structured backpressure failed"
                            );
                            self.diagnostics.log_orchestration(
                                self.state.iteration,
                                "jsonl",
                                crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                    reason: format!("structured build evidence failed: {reason}"),
                                },
                            );
                            accept_event!(Event::new(
                                "build.blocked",
                                crate::event_parser::build_blocked_payload(&reason),
                            ));
                        }
                        Ok(BuildStatus::Invalid { reason }) => {
                            warn!(reason = %reason, "build.done rejected: invalid JSON evidence");
                            self.diagnostics.log_orchestration(
                                self.state.iteration,
                                "jsonl",
                                crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                    reason: format!("invalid build evidence: {reason}"),
                                },
                            );
                            accept_event!(Event::new(
                                "build.blocked",
                                crate::event_parser::build_blocked_payload(&reason),
                            ));
                        }
                        Err(err) => {
                            warn!(error = %err, "build.done rejected: JSON parse error");
                            self.diagnostics.log_orchestration(
                                self.state.iteration,
                                "jsonl",
                                crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                    reason: format!("build evidence parse error: {err}"),
                                },
                            );
                            accept_event!(Event::new(
                                "build.blocked",
                                crate::event_parser::build_blocked_payload(&err),
                            ));
                        }
                    }
                } else if let Some(evidence) = EventParser::parse_backpressure_evidence(&payload) {
                    if evidence.all_passed() {
                        self.warn_on_mutation_evidence(&evidence);
                        accept_event!(Event::new(event.topic.as_str(), &payload));
                    } else {
                        // Evidence present but checks failed - synthesize build.blocked
                        warn!(
                            tests = evidence.tests_passed,
                            lint = evidence.lint_passed,
                            typecheck = evidence.typecheck_passed,
                            audit = evidence.audit_passed,
                            coverage = evidence.coverage_passed,
                            complexity = evidence.complexity_score,
                            duplication = evidence.duplication_passed,
                            performance = evidence.performance_regression,
                            specs = evidence.specs_verified,
                            "build.done rejected: backpressure checks failed"
                        );

                        let complexity = evidence
                            .complexity_score
                            .map(|value| format!("{value:.2}"))
                            .unwrap_or_else(|| "missing".to_string());
                        let performance = match evidence.performance_regression {
                            Some(true) => "regression".to_string(),
                            Some(false) => "pass".to_string(),
                            None => "missing".to_string(),
                        };
                        let specs = match evidence.specs_verified {
                            Some(true) => "pass".to_string(),
                            Some(false) => "fail".to_string(),
                            None => "not reported".to_string(),
                        };

                        self.diagnostics.log_orchestration(
                            self.state.iteration,
                            "jsonl",
                            crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                reason: format!(
                                    "backpressure checks failed: tests={}, lint={}, typecheck={}, audit={}, coverage={}, complexity={}, duplication={}, performance={}, specs={}",
                                    evidence.tests_passed,
                                    evidence.lint_passed,
                                    evidence.typecheck_passed,
                                    evidence.audit_passed,
                                    evidence.coverage_passed,
                                    complexity,
                                    evidence.duplication_passed,
                                    performance,
                                    specs
                                ),
                            },
                        );

                        accept_event!(Event::new(
                            "build.blocked",
                            "Backpressure checks failed. Fix tests/lint/typecheck/audit/coverage/complexity/duplication/specs before emitting build.done.",
                        ));
                    }
                } else {
                    // No evidence found - synthesize build.blocked
                    warn!("build.done rejected: missing backpressure evidence");

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "jsonl",
                        crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                            reason: "missing backpressure evidence".to_string(),
                        },
                    );

                    accept_event!(Event::new(
                        "build.blocked",
                        "Missing backpressure evidence. Include 'tests: pass', 'lint: pass', 'typecheck: pass', 'audit: pass', 'coverage: pass', 'complexity: <score>', 'duplication: pass', 'performance: pass' (optional), 'specs: pass' (optional) in build.done payload.",
                    ));
                }
            } else if event.topic == "review.done" && !event.is_wave_event() {
                // Validate review.done events have verification evidence.
                // Wave worker events skip this — wave reviews are read-only
                // and don't run tests/builds.
                let trimmed = payload.trim();
                let json_status: Option<Result<ReviewStatus, String>> = if trimmed.starts_with('{')
                {
                    Some(parse_review_json(trimmed, &self.config.core.workspace_root))
                } else {
                    None
                };
                if let Some(result) = json_status {
                    match result {
                        Ok(ReviewStatus::Pass) => {
                            accept_event!(Event::new(event.topic.as_str(), &payload));
                        }
                        Ok(ReviewStatus::Fail { reason, .. }) => {
                            warn!(reason = %reason, "review.done rejected: structured verification failed");
                            self.diagnostics.log_orchestration(
                                self.state.iteration,
                                "jsonl",
                                crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                    reason: format!("structured review evidence failed: {reason}"),
                                },
                            );
                            accept_event!(Event::new(
                                "review.blocked",
                                crate::event_parser::review_blocked_payload(&reason),
                            ));
                        }
                        Err(err) => {
                            warn!(error = %err, "review.done rejected: JSON parse error");
                            self.diagnostics.log_orchestration(
                                self.state.iteration,
                                "jsonl",
                                crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                    reason: format!("review evidence parse error: {err}"),
                                },
                            );
                            accept_event!(Event::new(
                                "review.blocked",
                                crate::event_parser::review_blocked_payload(&err),
                            ));
                        }
                    }
                } else if let Some(evidence) = EventParser::parse_review_evidence(&payload) {
                    if evidence.is_verified() {
                        accept_event!(Event::new(event.topic.as_str(), &payload));
                    } else {
                        // Evidence present but checks failed - synthesize review.blocked
                        warn!(
                            tests = evidence.tests_passed,
                            build = evidence.build_passed,
                            "review.done rejected: verification checks failed"
                        );

                        self.diagnostics.log_orchestration(
                            self.state.iteration,
                            "jsonl",
                            crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                reason: format!(
                                    "review verification failed: tests={}, build={}",
                                    evidence.tests_passed, evidence.build_passed
                                ),
                            },
                        );

                        accept_event!(Event::new(
                            "review.blocked",
                            "Review verification failed. Run tests and build before emitting review.done.",
                        ));
                    }
                } else {
                    // No evidence found - synthesize review.blocked
                    warn!("review.done rejected: missing verification evidence");

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "jsonl",
                        crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                            reason: "missing review verification evidence".to_string(),
                        },
                    );

                    accept_event!(Event::new(
                        "review.blocked",
                        "Missing verification evidence. Include 'tests: pass' and 'build: pass' in review.done payload.",
                    ));
                }
            } else if event.topic == "verify.passed" {
                if let Some(report) = EventParser::parse_quality_report(&payload) {
                    if report.meets_thresholds() {
                        accept_event!(Event::new(event.topic.as_str(), &payload));
                    } else {
                        let failed = report.failed_dimensions();
                        let reason = if failed.is_empty() {
                            "quality thresholds failed".to_string()
                        } else {
                            format!("quality thresholds failed: {}", failed.join(", "))
                        };

                        warn!(
                            failed_dimensions = ?failed,
                            "verify.passed rejected: quality thresholds failed"
                        );

                        self.diagnostics.log_orchestration(
                            self.state.iteration,
                            "jsonl",
                            crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                reason,
                            },
                        );

                        accept_event!(Event::new(
                            "verify.failed",
                            "Quality thresholds failed. Include quality.tests, quality.coverage, quality.lint, quality.audit, quality.mutation, quality.complexity with thresholds in verify.passed payload.",
                        ));
                    }
                } else {
                    // No quality report found - synthesize verify.failed
                    warn!("verify.passed rejected: missing quality report");

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "jsonl",
                        crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                            reason: "missing quality report".to_string(),
                        },
                    );

                    accept_event!(Event::new(
                        "verify.failed",
                        "Missing quality report. Include quality.tests, quality.coverage, quality.lint, quality.audit, quality.mutation, quality.complexity in verify.passed payload.",
                    ));
                }
            } else if event.topic == "verify.failed" {
                if EventParser::parse_quality_report(&payload).is_none() {
                    warn!("verify.failed missing quality report");
                }
                accept_event!(Event::new(event.topic.as_str(), &payload));
            } else {
                // Non-backpressure events pass through unchanged
                accept_event!(Event::new(event.topic.as_str(), &payload));
            }
        }

        // Track build.blocked events for thrashing detection
        let blocked_events: Vec<_> = validated_events
            .iter()
            .filter(|e| e.topic == "build.blocked".into())
            .collect();

        for blocked_event in &blocked_events {
            let task_id = Self::extract_task_id(&blocked_event.payload);

            let count = self
                .state
                .task_block_counts
                .entry(task_id.clone())
                .or_insert(0);
            *count += 1;

            debug!(
                task_id = %task_id,
                block_count = *count,
                "Task blocked"
            );

            // After 3 blocks on same task, emit build.task.abandoned
            if *count >= 3 && !self.state.abandoned_tasks.contains(&task_id) {
                warn!(
                    task_id = %task_id,
                    "Task abandoned after 3 consecutive blocks"
                );

                self.state.abandoned_tasks.push(task_id.clone());

                self.diagnostics.log_orchestration(
                    self.state.iteration,
                    "jsonl",
                    crate::diagnostics::OrchestrationEvent::TaskAbandoned {
                        reason: format!(
                            "3 consecutive build.blocked events for task '{}'",
                            task_id
                        ),
                    },
                );

                let abandoned_event = Event::new(
                    "build.task.abandoned",
                    format!(
                        "Task '{}' abandoned after 3 consecutive build.blocked events",
                        task_id
                    ),
                );

                self.bus.publish(abandoned_event);
            }
        }

        // Track hat-level blocking for legacy thrashing detection
        let has_blocked_event = !blocked_events.is_empty();

        if has_blocked_event {
            self.state.consecutive_blocked += 1;
        } else {
            self.state.consecutive_blocked = 0;
            self.state.last_blocked_hat = None;
        }

        // Handle human.interact blocking behavior:
        // When a human.interact event is detected and robot service is active,
        // send the question and block until human.response or timeout.
        let mut response_event = None;
        let mut human_interact_context = None;
        let ask_human_idx = validated_events
            .iter()
            .position(|e| e.topic == "human.interact".into());

        if let Some(idx) = ask_human_idx {
            let ask_event = &validated_events[idx];
            let payload = ask_event.payload.clone();

            // P5: validate the human.interact payload before blocking. An
            // empty/whitespace or malformed JSON payload is rejected up front
            // so the loop does not block on a question that would never
            // resolve. Inject a `human.timeout` so the agent sees a clear
            // error and continues.
            if let Err(reason) =
                crate::event_origin::validate_human_interact_payload(Some(&payload))
            {
                warn!(
                    payload = %payload,
                    reason = %reason,
                    "Rejecting human.interact with invalid payload before blocking"
                );
                self.diagnostics.log_error(
                    self.state.iteration,
                    "human.interact",
                    crate::diagnostics::DiagnosticError::ValidationFailure {
                        rule: "human_interact_payload".to_string(),
                        message: format!("invalid human.interact payload: {reason}"),
                        evidence: payload.clone(),
                    },
                );
                let mut err_context = Map::new();
                err_context.insert(
                    "outcome".to_string(),
                    Value::String("invalid_payload".to_string()),
                );
                err_context.insert("error".to_string(), Value::String(reason.clone()));
                human_interact_context = Some(Value::Object(err_context));
                response_event = Some(Event::new(
                    "human.timeout",
                    format!(
                        "Invalid human.interact payload: {reason}. Original payload: {payload}"
                    ),
                ));
            } else {
                let mut context = match Self::parse_human_interact_context(&payload) {
                    Value::Object(map) => map,
                    _ => Map::new(),
                };

                if let Some(ref robot_service) = self.robot_service {
                    info!(
                        payload = %payload,
                        "human.interact event detected — sending question via robot service"
                    );

                    // Send the question (includes retry with exponential backoff)
                    let send_ok = match robot_service.send_question(&payload) {
                        Ok(_message_id) => true,
                        Err(e) => {
                            warn!(
                                error = %e,
                                "Failed to send human.interact question after retries — treating as timeout"
                            );
                            // Log to diagnostics
                            self.diagnostics.log_error(
                                self.state.iteration,
                                "telegram",
                                crate::diagnostics::DiagnosticError::TelegramSendError {
                                    operation: "send_question".to_string(),
                                    error: e.to_string(),
                                    retry_count: 3,
                                },
                            );
                            context.insert(
                                "outcome".to_string(),
                                Value::String("send_failure".to_string()),
                            );
                            context.insert("error".to_string(), Value::String(e.to_string()));
                            false
                        }
                    };

                    // Block: poll events file for human.response
                    // Per spec, even on send failure we treat as timeout (continue without blocking)
                    if send_ok {
                        // Read the active events path from the current-events marker,
                        // falling back to the default events.jsonl if not available.
                        let events_path = self
                            .loop_context
                            .as_ref()
                            .and_then(|ctx| {
                                std::fs::read_to_string(ctx.current_events_marker())
                                    .ok()
                                    .map(|s| ctx.workspace().join(s.trim()))
                            })
                            .or_else(|| {
                                std::fs::read_to_string(".ralph/current-events")
                                    .ok()
                                    .map(|s| PathBuf::from(s.trim()))
                            })
                            .unwrap_or_else(|| {
                                self.loop_context
                                    .as_ref()
                                    .map(|ctx| ctx.events_path())
                                    .unwrap_or_else(|| PathBuf::from(".ralph/events.jsonl"))
                            });

                        match robot_service.wait_for_response(&events_path) {
                            Ok(Some(response)) => {
                                info!(
                                    response = %response,
                                    "Received human.response — continuing loop"
                                );
                                context.insert(
                                    "outcome".to_string(),
                                    Value::String("response".to_string()),
                                );
                                context.insert(
                                    "response".to_string(),
                                    Value::String(response.clone()),
                                );
                                // Create a human.response event to inject into the bus
                                response_event = Some(Event::new("human.response", &response));
                            }
                            Ok(None) => {
                                warn!(
                                    timeout_secs = robot_service.timeout_secs(),
                                    "Human response timeout — injecting human.timeout event"
                                );
                                context.insert(
                                    "outcome".to_string(),
                                    Value::String("timeout".to_string()),
                                );
                                context.insert(
                                    "timeout_seconds".to_string(),
                                    Value::from(robot_service.timeout_secs()),
                                );
                                let timeout_event = Event::new(
                                    "human.timeout",
                                    format!(
                                        "No response after {}s. Original question: {}",
                                        robot_service.timeout_secs(),
                                        payload
                                    ),
                                );
                                response_event = Some(timeout_event);
                            }
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    "Error waiting for human response — injecting human.timeout event"
                                );
                                context.insert(
                                    "outcome".to_string(),
                                    Value::String("wait_error".to_string()),
                                );
                                context.insert("error".to_string(), Value::String(e.to_string()));
                                let timeout_event = Event::new(
                                    "human.timeout",
                                    format!(
                                        "Error waiting for response: {}. Original question: {}",
                                        e, payload
                                    ),
                                );
                                response_event = Some(timeout_event);
                            }
                        }
                    }
                } else {
                    debug!(
                        "human.interact event detected but no robot service active — passing through"
                    );
                    context.insert(
                        "outcome".to_string(),
                        Value::String("no_robot_service".to_string()),
                    );
                }

                human_interact_context = Some(Value::Object(context));
            }
        }

        let restart_requested = validated_events.iter().any(Self::is_restart_request_event)
            || response_event
                .as_ref()
                .is_some_and(Self::is_restart_request_event);
        if restart_requested {
            self.mark_restart_requested("human_text");
        }

        // Track whether any events will be published (before the loop consumes them).
        let had_events = !validated_events.is_empty();
        let had_plan_events = validated_events
            .iter()
            .any(|event| event.topic.as_str().starts_with("plan."));
        // Record and diagnose validated events (before consuming them).
        let verdict_topics = self.verdict_gate_topics();
        let verdict_topics_slice = verdict_topics.as_deref();
        for event in &validated_events {
            // Record topic for event chain validation
            self.state.record_event(event);
            self.state
                .record_verdict_if_match(event, verdict_topics_slice);

            // U3: Update hat lifecycle tracker for accepted events.
            // Find the source hat for this event and update the tracker.
            // Terminal events call complete(); non-terminal call observe_accepted_event().
            //
            // P0 code-review finding #1: the key was previously (loop_id, iteration,
            // hat_id, trigger_identity) with trigger_identity reverse-derived via
            // `can_publish` on `last_activation_events`. Because trigger events are
            // hat inputs (not publishes), the reverse lookup always returned the
            // fallback ("unknown" on activate, topic_str on complete), so the keys
            // never matched and `complete` hit the `None` branch — every
            // activation leaked. The key is now the (loop_id, iteration, hat_id)
            // triple; trigger identity is a snapshot-only display field.
            let source_hat_id = event
                .source
                .as_ref()
                .or(self.state.last_active_hat_ids.first())
                .cloned();
            if let Some(source_hat_id) = source_hat_id {
                let hat_config = self.registry.get_config(&source_hat_id);
                let topic_str = event.topic.as_str();
                let is_terminal = hat_config
                    .is_some_and(|config| config.terminal_topic_set().contains(topic_str));
                let key = ActivationKey {
                    loop_id: self
                        .loop_context
                        .as_ref()
                        .and_then(|ctx| ctx.loop_id())
                        .unwrap_or("primary")
                        .to_string(),
                    iteration: self.state.iteration,
                    hat_id: source_hat_id.as_str().to_string(),
                };
                if is_terminal {
                    self.hat_lifecycle_tracker.complete(&key, topic_str);
                } else {
                    self.hat_lifecycle_tracker.observe_accepted_event(&key);
                }
                // WRC-U4 (2026-06-12-003): clear any pending handoff
                // deadlines for this consumer hat. The accept-time
                // deadline for the triggering handoff is irrelevant
                // once the hat has activated; the `on_hat_activated`
                // call also clears siblings (e.g. a `fix.plan.ready`
                // handoff queued behind the same `executor`).
                // `on_hat_activated` returns the number of cleared
                // entries which is informational here; we do not
                // surface it because the only consumer (the
                // diagnostic reporter) reads the pending count via
                // `pending_count()` at stall-check time.
                // 2026-06-13-004 P0 #5 review fix (F2 ralph
                // guard symmetry): mirror the build_prompt
                // guard. The "ralph" hat is the constant
                // coordinator sentinel, never a handoff
                // consumer — passing it through here would
                // spuriously clear real consumer pending
                // entries whose hat_id happens to match (or
                // be a prefix of) "ralph". Round 2 added
                // this guard at L2853 (build_prompt); this
                // closes the asymmetry at the process_output
                // handoff-clear site.
                if source_hat_id.as_str() != "ralph" {
                    self.state
                        .handoff_tracker
                        .on_hat_activated(source_hat_id.as_str());
                }
                // 2026-06-14-004 U2: when a hat successfully publishes a
                // legal event, clear its rejection retry counts so a prior
                // scope violation does not cause a premature fuse on a
                // later, unrelated violation.
                self.state
                    .clear_rejection_keys_for_hat(source_hat_id.as_str());
            }

            self.diagnostics.log_orchestration(
                self.state.iteration,
                "jsonl",
                crate::diagnostics::OrchestrationEvent::EventPublished {
                    topic: event.topic.to_string(),
                },
            );

            // Check for orphaned events: no specific hat (non-fallback-only) subscribes.
            // The builtin "ralph" fallback hat with `*` subscription is excluded so that
            // events only matching the universal fallback are still marked as orphans.
            if !self.registry.has_specific_subscriber(event.topic.as_str()) {
                has_orphans = true;
            }

            debug!(
                topic = %event.topic,
                "Publishing event from JSONL"
            );
        }

        // Apply event projections before publishing.
        for event in &validated_events {
            if let Some(ref projection_config) = self.config.core.event_projection
                && projection_config.enabled
            {
                crate::event_projection::apply_projection(
                    event,
                    &projection_config.rules,
                    &self.config.core.workspace_root,
                );
            }
        }

        // Publish validated events to the bus.
        // Ralph is always registered with subscribe("*"), so every event has at least
        // one subscriber. Events without a specific hat subscriber are "orphaned" —
        // Ralph handles them as the universal fallback.
        for event in validated_events {
            self.bus.publish(event);
        }

        // Publish human.response event if one was received during blocking
        if let Some(response) = response_event {
            let verdict_topics = self.verdict_gate_topics();
            let verdict_topics_slice = verdict_topics.as_deref();
            self.state.record_event(&response);
            self.state
                .record_verdict_if_match(&response, verdict_topics_slice);
            info!(
                topic = %response.topic,
                "Publishing human.response event from robot service"
            );
            if let Some(ref projection_config) = self.config.core.event_projection
                && projection_config.enabled
            {
                crate::event_projection::apply_projection(
                    &response,
                    &projection_config.rules,
                    &self.config.core.workspace_root,
                );
            }
            self.bus.publish(response);
        }

        // --- U3: Invariant assertion checks ---
        if self.config.core.invariant_assertions {
            let control_prefixes = ["event.", "human."];
            let control_exact = [
                "LOOP_COMPLETE",
                "REVIEW_COMPLETE",
                "loop.cancel",
                "task.resume",
                "build.task.abandoned",
                "event.isolation.boundary_violation",
            ];

            for event in &accepted_log_events {
                let topic = event.topic.as_str();
                let is_control = control_exact.contains(&topic)
                    || control_prefixes.iter().any(|p| topic.starts_with(p));

                // INV-1: Ralph must not publish business topics
                if !is_control && event.source.as_ref().map(|h| h.as_str()) == Some("ralph") {
                    self.state.invariant_violation_count += 1;
                    self.state.last_invariant_violation =
                        Some(format!("INV-1:hat=ralph,topic={}", topic));

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "ralph",
                        crate::diagnostics::OrchestrationEvent::InvariantViolation {
                            rule_id: "INV-1".to_string(),
                            description: format!("Ralph published business topic '{}'", topic),
                            topic: Some(topic.to_string()),
                            source: Some("ralph".to_string()),
                            iteration: self.state.iteration,
                        },
                    );

                    warn!(
                        topic = %topic,
                        invariant = "INV-1",
                        "Invariant violation: Ralph published business topic"
                    );
                }
            }
        }
        // --- End invariant checks ---

        Ok(ProcessedEvents {
            had_events,
            had_raw_events,
            had_rejected_events,
            had_plan_events,
            human_interact_context,
            has_orphans,
            accepted_events: accepted_log_events,
            contract_rejections,
            payload_contract_violation,
        })
    }

    /// Process events from JSONL, partitioning wave events from regular events.
    ///
    /// Wave events (those with `wave_id` set and targeting a concurrent hat) are
    /// extracted and returned separately. Regular events go through the full
    /// backpressure pipeline via `process_parse_result`.
    pub fn process_events_from_jsonl_with_waves(
        &mut self,
    ) -> std::io::Result<ProcessedEventsWithWaves> {
        let result = self.event_reader.read_new_events()?;

        // Partition: wave dispatch events vs regular events.
        // Only events that target a concurrent hat (concurrency > 1) are wave dispatches.
        // Wave *results* (e.g. review.done) have wave_id set but should be treated as
        // regular events so they reach the bus and trigger downstream hats (e.g. aggregator).
        //
        // Uses find_by_trigger + get_config — the same resolution path as
        // detect_wave_events — to ensure partition and detection agree.
        let (wave_events, regular_events): (Vec<_>, Vec<_>) =
            result.events.into_iter().partition(|e| {
                e.wave_id.is_some()
                    && self
                        .registry
                        .find_by_trigger(e.topic.as_str())
                        .and_then(|hat_id| self.registry.get_config(hat_id))
                        .is_some_and(|hat_config| hat_config.concurrency > 1)
            });

        // --- Origin guard: validate wave event provenance before policy validation ---
        // Wave dispatch events bypass process_parse_result, so origin validation must
        // run here to prevent forged wave events from reaching wave execution.
        let (wave_events, _origin_rejections) = filter_events_by_origin(
            wave_events,
            &self.registry,
            &self.config.event_loop.cancellation_promise,
            &self.config.event_loop.completion_promise,
        );

        // --- Topic format check (U5 / R9) for wave events ---
        // Only active when event_policy is enabled AND hats are configured.
        let wave_events = if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
            && !self.config.hats.is_empty()
        {
            let allowed_topics: std::collections::HashSet<String> =
                crate::event_policy::build_allowed_topics(
                    &self.config.hats,
                    &self.config.event_loop.completion_promise,
                    self.config.event_loop.event_policy.as_ref(),
                );
            let (wave_events_ok, wave_rejections): (Vec<_>, Vec<_>) =
                wave_events.into_iter().partition(|event| {
                    if crate::event_policy::is_system_topic(&event.topic) {
                        return true;
                    }
                    crate::event_policy::check_topic_format(&event.topic, &allowed_topics).is_none()
                });
            if !wave_rejections.is_empty() {
                // R10: same behavior as the regular-event path —
                // publish the legacy diagnostic AND write a recovery
                // journal entry so `ralph diagnose` can surface it.
                let allowed_list: Vec<String> = allowed_topics.iter().cloned().collect();
                for event in &wave_rejections {
                    warn!(
                        topic = %event.topic,
                        hat = ?event.hat,
                        "Topic format rejection (wave): unknown topic not in whitelist"
                    );
                    let diagnostic = Event::new(
                        "event.topic_format.rejected",
                        format!(
                            "TOPIC_FORMAT_REJECTED: '{}' is not in the whitelist of known topics. \
                             This event will not be retried.",
                            event.topic
                        ),
                    );
                    self.bus.publish(diagnostic);
                    Self::log_topic_format_rejection(
                        self,
                        event.topic.as_str(),
                        event.hat.as_deref(),
                        &allowed_list,
                    );
                }
            }
            wave_events_ok
        } else {
            wave_events
        };
        // --- End topic format check (wave) ---

        // --- Event policy validation for wave events ---
        // Wave dispatch events are partitioned before process_parse_result, so they
        // must undergo policy validation here to avoid bypassing schema checks.
        //
        // U1 (2026-06-13-001): capture the policy_rejections vector and the
        // raw count of events that entered this validation step. These two
        // pieces of evidence are surfaced on `ProcessedEventsWithWaves` so
        // the runner can:
        //   1. Avoid the false `missing_event_gate` (the agent DID try to
        //      emit; the wave fan-out was simply blocked by a missing
        //      required field such as `depth`).
        //   2. Emit a recovery envelope naming the failing topic / field /
        //      wave_id so `ralph diagnose` attributes the failure to
        //      `payload_contract` rather than a silent missing emission.
        let mut wave_raw_count: usize = 0;
        let mut wave_policy_rejections: Vec<crate::event_policy::PolicyRejection> = Vec::new();
        let wave_events = if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            wave_raw_count = wave_events.len();
            // Unit 2 (2026-06-16-002 plan): same `take()`/put-back
            // pattern as the regular partition — see the long
            // comment block above for the borrow-checker rationale.
            // The function now takes `&mut LoopState` and reborrows
            // `review_step_tracker` internally, so we only need to
            // move `policy_runtime_state` out of `self.state` for
            // the call.
            let mut policy_state: PolicyRuntimeState = self
                .state
                .policy_runtime_state
                .take()
                .unwrap_or_default();
            // Unit 2 (2026-06-16-002 plan) take-3: same pattern
            // as the regular partition.  Move the
            // `review_step_tracker` and `policy_runtime_state`
            // out of `self.state` for the call (the validator
            // does **not** take `&mut LoopState` anymore), then
            // restore them and post-process the recoverable
            // candidates.
            let mut review_step_tracker =
                std::mem::take(&mut self.state.review_step_tracker);
            let policy_result = apply_event_policy_validation(
                wave_events,
                policy_config,
                &mut policy_state,
                &mut review_step_tracker,
                &mut self.bus,
                policy_config
                    .completion_after_terminal
                    .write_diagnostic_event,
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new(),
                &self.registry,
            );
            self.state.review_step_tracker = review_step_tracker;
            self.state.policy_runtime_state = Some(policy_state);

            // Write hold artifact if policy hold was triggered
            if policy_result.hold_triggered
                && let Err(e) = self.write_hold_artifact(policy_result.hold_reason.as_deref())
            {
                warn!(error = %e, "Failed to write hold artifact");
            }

            wave_policy_rejections = policy_result.policy_rejections;
            // Unit 2 (2026-06-16-002 plan) take-3: post-process
            // the recoverable candidates — same loop as the
            // regular partition, just extending the same buffer.
            for candidate in policy_result.recoverable_candidates.into_iter() {
                let (count, exhausted) = self
                    .state
                    .record_recoverable_rejection_key(
                        &candidate.hat,
                        &candidate.topic,
                        candidate.reason_class.as_str(),
                    );
                if exhausted {
                    self.state
                        .recoverable_exhaustion_buffer
                        .push(RecoverableExhaustion {
                            hat: candidate.hat,
                            topic: candidate.topic,
                            reason_class: candidate.reason_class,
                            count,
                        });
                }
            }

            // U1: when policy rejected every wave event, write a recovery
            // envelope with `source = payload_contract` and
            // `reason_code = wave_dispatch_blocked` (or `missing_required_field`
            // if the first rejection's violation type is `MissingRequiredField`).
            // The envelope is what `ralph diagnose` and the runner's gate
            // logic use to distinguish "agent forgot to emit" from "agent
            // emitted a wave that policy blocked". Fired on any batch
            // where wave events entered the policy validator but none
            // survived — covers both Reject-with-Resume and Hold (which
            // does not produce PolicyRejection rows but still drops the
            // event from the dispatch set).
            if wave_raw_count > 0 && policy_result.events.is_empty() {
                Self::log_wave_policy_blocked_envelope(
                    self,
                    &wave_policy_rejections,
                    wave_raw_count,
                );
            }

            policy_result.events
        } else {
            wave_events
        };

        // Update policy runtime state for wave events that passed validation
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            let policy_state = self
                .state
                .policy_runtime_state
                .get_or_insert_with(PolicyRuntimeState::default);
            for event in &wave_events {
                if policy_config.terminal_topics.contains(&event.topic) {
                    policy_state.terminal_observed = true;
                }
            }
        }

        if !wave_events.is_empty() {
            debug!(
                wave_count = wave_events.len(),
                regular_count = regular_events.len(),
                "Partitioned wave events from regular events"
            );
        }

        // --- Isolated scope enforcement for wave events (U4 / A3) ---
        // Wave partition bypasses `process_parse_result`, so the regular
        // isolated-scope check does not run on wave events. We re-apply
        // it here post-partition. Per KTD-U4-1 the same
        // `isolated_publish_allowed` predicate is used; per KTD-U4-2 a
        // single isolated activation may emit at most one distinct
        // `wave_id` — additional distinct wave_ids in the same read
        // batch are typed as `IsolatedMultipleBusinessEmissions`.
        let wave_events = if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && let Some(isolated_hat) = self.state.current_isolated_hat.clone()
            && !wave_events.is_empty()
        {
            self.enforce_wave_isolated_scope(wave_events, &isolated_hat)?
        } else {
            wave_events
        };
        // --- End isolated scope enforcement for wave events ---

        // Delegate regular events to the full pipeline (backpressure, scope
        // enforcement, human.interact, plan detection, etc.)
        let regular_result = crate::event_reader::ParseResult {
            events: regular_events,
            malformed: result.malformed,
        };
        let processed = self.process_parse_result(regular_result)?;

        Ok(ProcessedEventsWithWaves {
            processed,
            wave_events,
            wave_policy_rejections,
            wave_raw_count,
        })
    }

    /// Checks if output contains a completion event from Ralph.
    ///
    /// Completion must be emitted as an `<event>` tag, not plain text.
    pub fn check_ralph_completion(&self, output: &str) -> bool {
        let events = EventParser::new().parse(output);
        events
            .iter()
            .any(|event| event.topic.as_str() == self.config.event_loop.completion_promise)
    }

    /// Publishes the loop.terminate system event to observers.
    ///
    /// Per spec: "Published by the orchestrator (not agents) when the loop exits."
    /// This is an observer-only event—hats cannot trigger on it.
    ///
    /// Returns the event for logging purposes.
    pub fn publish_terminate_event(&mut self, reason: &TerminationReason) -> Event {
        // Stop the robot service if it was running
        self.stop_robot_service();

        let elapsed = self.state.elapsed();
        let duration_str = format_duration(elapsed);

        let payload = format!(
            "## Reason\n{}\n\n## Status\n{}\n\n## Summary\n- Iterations: {}\n- Duration: {}\n- Exit code: {}",
            reason.as_str(),
            termination_status_text(reason),
            self.state.iteration,
            duration_str,
            reason.exit_code()
        );

        let event = Event::new("loop.terminate", &payload);

        // Publish to bus for observers (but no hat can trigger on this)
        self.bus.publish(event.clone());

        info!(
            reason = %reason.as_str(),
            iterations = self.state.iteration,
            duration = %duration_str,
            "Wrapping up: {}. {} iterations in {}.",
            reason.as_str(),
            self.state.iteration,
            duration_str
        );

        event
    }

    /// Publish an event to the event bus.
    pub fn publish_event(&mut self, event: Event) {
        self.bus.publish(event);
    }

    /// Returns the robot service's shutdown flag, if active.
    ///
    /// Signal handlers can set this flag to interrupt `wait_for_response()`
    /// without waiting for the full timeout.
    pub fn robot_shutdown_flag(&self) -> Option<Arc<AtomicBool>> {
        self.robot_service.as_ref().map(|s| s.shutdown_flag())
    }

    /// Stops the robot service if it's running.
    ///
    /// Called during loop termination to cleanly shut down the communication backend.
    fn stop_robot_service(&mut self) {
        if let Some(service) = self.robot_service.take() {
            service.stop();
        }
    }

    // -------------------------------------------------------------------------
    // Human-in-the-loop planning support
    // -------------------------------------------------------------------------

    /// Check if any event is a `user.prompt` event.
    ///
    /// Returns the first user prompt event found, or None.
    pub fn check_for_user_prompt(&self, events: &[Event]) -> Option<UserPrompt> {
        events
            .iter()
            .find(|e| e.topic.as_str() == "user.prompt")
            .map(|e| UserPrompt {
                id: Self::extract_prompt_id(&e.payload),
                text: e.payload.clone(),
            })
    }

    /// Extract a prompt ID from the event payload.
    ///
    /// Supports both XML attribute format: `<event topic="user.prompt" id="q1">...</event>`
    /// and JSON format in payload.
    fn extract_prompt_id(payload: &str) -> String {
        // Try to extract id attribute from XML-like format first
        if let Some(start) = payload.find("id=\"")
            && let Some(end) = payload[start + 4..].find('"')
        {
            return payload[start + 4..start + 4 + end].to_string();
        }

        // Fallback: generate a simple ID based on timestamp
        format!("q{}", Self::generate_prompt_id())
    }

    /// Generate a simple unique ID for prompts.
    /// Uses timestamp-based generation since uuid crate isn't available.
    fn generate_prompt_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{:x}", nanos % 0xFFFF_FFFF)
    }
}

/// A user prompt that requires human input.
///
/// Created when the agent emits a `user.prompt` event during planning.
#[derive(Debug, Clone)]
pub struct UserPrompt {
    /// Unique identifier for this prompt (e.g., "q1", "q2")
    pub id: String,
    /// The prompt/question text
    pub text: String,
}

/// Formats a duration as human-readable string.
fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

/// Returns a human-readable status based on termination reason.
fn termination_status_text(reason: &TerminationReason) -> &'static str {
    match reason {
        TerminationReason::CompletionPromise => "All tasks completed successfully.",
        TerminationReason::MaxIterations => "Stopped at iteration limit.",
        TerminationReason::MaxRuntime => "Stopped at runtime limit.",
        TerminationReason::MaxCost => "Stopped at cost limit.",
        TerminationReason::ConsecutiveFailures => "Too many consecutive failures.",
        TerminationReason::LoopThrashing => {
            "Loop thrashing detected - same hat repeatedly blocked."
        }
        TerminationReason::LoopStale => {
            "Stale loop detected - same topic emitted 3+ times consecutively."
        }
        TerminationReason::ValidationFailure => "Too many consecutive malformed JSONL events.",
        TerminationReason::Stopped => "Manually stopped.",
        TerminationReason::Interrupted => "Interrupted by signal.",
        TerminationReason::RestartRequested => "Restarting by human request.",
        TerminationReason::WorkspaceGone => "Workspace directory removed externally.",
        TerminationReason::Cancelled => "Cancelled gracefully (human rejection or timeout).",
        TerminationReason::PayloadContractViolation => "Payload contract violation - loop paused.",
        TerminationReason::RecoveryExhausted { .. } => {
            "Recovery responder exhausted retry window - loop paused."
        }
        TerminationReason::ReviewFailed { .. } => {
            "Review verdict failed and propagated to final mirror - loop terminated."
        }
        TerminationReason::ScopeViolationCircuitBreakerTripped { .. } => {
            "Isolated scope violation circuit breaker tripped - loop terminated."
        }
        TerminationReason::RecoverablePayloadExhausted { .. } => {
            "Recoverable-payload budget exhausted - loop terminated."
        }
    }
}
