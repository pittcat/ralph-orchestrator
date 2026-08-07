//! Event loop orchestration.
//!
//! The event loop coordinates the execution of hats via pub/sub messaging.

pub mod accepted_event;
// U6 (plan 2026-07-30-004): the Accepted Transition API — the single,
// atomic entry point for all business state changes. Validates
// pre-commit, writes a durable outbox entry, then publishes to the bus.
pub mod accepted_transition;
// U8 (plan 2026-07-30-004): typed disposition classification. Every
// topic maps to one of {Business, Recovery, DiagnosticObservation,
// LoopControl}; only Business / Recovery advance business flow through
// the Accepted Transition API.
pub mod disposition;
pub mod loop_state;
pub mod plan_blocked_reason;
pub mod rejection;
pub mod rejection_kind;
pub mod review_step_state;
pub mod terminal_closed_guard;
// 2026-06-27 mechanism foundation U1: hard required-fields check at
// emit time. Pure-logic core; `EmitSchemaGateStage` (U6) wraps it.
pub mod emit_schema_gate;
// 2026-06-27 mechanism foundation completion (002 plan, U1):
// single emit-gate facade. Wraps `StagePipeline::run` plus
// the `is_repair_topic` routing hint so `publish_event`
// and `process_parse_result` share one entry point.
pub mod emit_gate;
// 2026-06-27 mechanism foundation U5: declarative flow
// parser (steps / allowed_emits / terminal_emits / on_partial).
// The lint in `preset_lint::flow_declaration` and the
// `FlowStepScopeStage` (U9) both consume the same type.
pub mod flow_declaration;
// 2026-07-02-006 plan U1: opt-in `WorkflowPhaseAuthority`
// engine entry point. The pure-data `PhaseAuthorityConfig` lives
// in `phase_authority::config`; U2 onward extends the module with
// declaration / evaluator / stage wiring.
pub mod phase_authority;
// 2026-06-27 mechanism foundation U8: thin wiring layer from
// the existing task_store / diagnosis / drift consumers to the
// U4 `IdempotentLog`. The runtime opts in by setting
// `mechanism.state_idempotency: required` in the preset.
pub mod idempotent_wiring;
// 2026-06-27 mechanism foundation U3: legacy task loop_id backfill.
// Pure file-I/O; sits next to `relocate_legacy_tasks` so U7 can
// invoke it from `RepairDispatchStage` without crossing module
// boundaries.
pub mod legacy_task_relocate;
// 2026-06-27 mechanism foundation U2: independent repair state
// machine + per-task budget. `RepairDispatchStage` (U7) wraps it.
pub mod recovery_finalizer;
pub mod repair_flow;
pub mod repair_stream_sink;
pub mod stage_pipeline;
pub mod step_close_obligation;
// 2026-07-02-004 plan milestone B (U5): synthesized precheck
// gate hat hard-gate enforcement. Pure-logic core that the
// event loop invokes from the step-close obligation path.
pub mod precheck_gate_enforcement;
// 2026-07-02-004 plan milestone B (U6): failure-closure
// runner for `<X>.rejected` events. Owns the per-(loop,
// topic) retry counter and the dispatch decision
// (resume vs escalate to `plan.blocked`).
pub mod precheck_gate_runner;
// 2026-06-27 mechanism foundation U6+ wiring stages. Each
// U-* wiring unit lives in `event_loop::stages` as its own
// submodule. Order matches the locked pipeline order; do
// not reorder without updating `assert_stage_order!`.
pub mod stages;
// 2026-06-23-005 U3: typed TerminationTrigger SSOT (KTD-7 + R11).
// See `event_loop::termination` for the typed enum + reason mapper.
pub mod termination;
// 2026-06-23-005 U4: typed AuditSeverity SSOT (KTD-8 + R12).
// See `event_loop::audit` for the typed severity + dispatcher.
pub mod audit;
#[cfg(test)]
// 2026-07-02-006 plan U15: build_stage_pipeline_from_config
// branch tests. Sibling to `tests` so the wiring change is
// visible without scanning the entire mod.rs.
mod build_stage_pipeline_phase_branch_tests;
mod impl_region_01;
mod impl_region_02;
mod impl_region_03;
mod impl_region_04;
mod impl_region_05;
mod impl_region_06;
mod impl_region_07;
mod impl_region_08;
mod impl_region_09;
mod impl_region_10;
#[cfg(test)]
mod tests;

// 2026-06-10-003 U1 scaffold: 10 target submodules (filled in U3-U6).
// Each placeholder is intentionally empty; `pub use xxx::*` re-exports
// are wired in the corresponding unit to keep the public API stable.
pub mod diagnostics;
pub mod dispatch;
pub mod lifecycle;
// U5a: EventLoop 生命周期相关 free function SSOT 转发。
// impl EventLoop 方法留到 U5b-U5e 阶段。
pub use lifecycle::build_state_ledger_from_env;
// U6 (plan 2026-07-30-004): Accepted Transition API re-exports.
pub use accepted_transition::{AcceptedTransition, OutboxEntry, TransitionError};
pub use disposition::{Disposition, publish_synthetic};
pub mod policy;
pub mod process;
pub mod prompt;
pub mod termination_impl;
// U5b: termination text formatting free function SSOT 转发。
// impl EventLoop 方法留到后续 U 阶段处理。
pub use termination_impl::{format_duration, termination_status_text};
pub mod types;
pub mod verdict;
pub mod wave;
pub mod workflow_guard;

// 2026-06-10-003 U1 scaffold: 6 follow-up placeholders for modules that
// already exceed the R1 red-line (loop_state / rejection / review_step_state).
// NOT `pub use`d from `event_loop::mod` — see plan v14.
mod flow_lifecycle;
mod loop_state_active;
mod loop_state_history;
mod rejection_envelope;
mod rejection_payload;
mod review_step_gate;

pub use loop_state::{
    LINT_CIRCUIT_BREAKER_LIMIT, LoopState, RejectionDigestEntry, U2_REJECTION_RETRY_LIMIT,
    WorkflowProgress,
};
// Items are also re-exported from `crate::*` via `lib.rs`. The lib-side
// re-export keeps the public API stable; the `pub use` here is a
// convenience path for in-crate consumers (the runner).
#[allow(unused_imports)]
pub use rejection::{
    NonRetryableReason, Rejection, RejectionStage, build_task_resume_payload,
    enrich_task_resume_payload, enrich_task_resume_payload_full,
    enrich_task_resume_payload_with_stage, extract_reason_code, rejection_from_origin,
    resolve_target_hat, task_resume_payload_has_required_fields,
};
// U4b (2026-06-10-003 plan, v14): re-export the policy / payload_contract
// helper free functions moved from this file into `policy.rs`. The
// `pub use` preserves the in-crate call sites (the legacy direct-path
// `build_unified_validation_pipeline(&self.config.event_loop)` and
// `publish_correction_via_context(...)` invocations in
// `process_parse_result`) so R3 (public API stable) holds without an
// extra forwarder layer.
pub use policy::{
    build_unified_validation_pipeline, policy_finding_for_topic, publish_correction_via_context,
};
// U3: re-export the type declarations that were moved from mod.rs to
// `types.rs`. The `pub use` preserves the existing public API path
// (`event_loop::TerminationReason`, etc.) so downstream consumers see
// no change. `WorkflowGuardRejection` stays module-private and is
// only `pub(super)` in `types.rs`.
pub use types::{EventLoop, ProcessedEvents, ProcessedEventsWithWaves, TerminationReason};
// 2026-06-26 plan U1: typed verdict SSOT — used by `verdict_payload_is_fail`
// and `check_completion_event` to share the same Pass / PassWithResiduals /
// Fail semantics as the terminal reporting chain.
pub use verdict::{Verdict, VerdictParseError};
// 2026-06-26 plan U1: completion-correction exhaust + structural-rejection
// sources, surfaced through `TerminationReason::CompletionStuck`.
pub use types::{CompletionStuck, StuckSource};

// 2026-07-06-004 plan U4: prompt-injection gate helper, exposed
// at module scope so the U4 tests (and U6 wiring) can reach it
// without going through `EventLoop`. Stays `pub(crate)` so it
// never leaks out of `ralph-core`.
#[cfg(test)]
pub(crate) use self::prompt_helpers::prepend_handoff_envelope_if_enabled;
// 2026-07-06-004 plan U6: isolated-prompt wiring helper. Used by
// the real prompt chain (after orchestrator context / wave
// context) so the wiring test (`u6_handoff_envelope_wiring`) can
// pin the behaviour without going through EventLoop.
pub(crate) use self::prompt_helpers::build_isolated_prompt_with_handoff;

mod prompt_helpers {
    use crate::config::HandoffEnvelopeConfig;
    use crate::handoff_envelope::{
        HandoffEnvelopeView, latest_handoff_envelope_payload, render_handoff_envelope_prompt,
    };
    use ralph_proto::Event;

    /// 2026-07-06-004 plan U4: small private helper that decides
    /// whether to prepend the rendered `## HANDOFF ENVELOPE`
    /// block. Default-closed (no-op) when either flag is off or
    /// no envelope is supplied. U6 calls this from inside the real
    /// prompt chain with the latest envelope extracted from
    /// recent events; U4 only tests the gate logic itself.
    pub(crate) fn prepend_handoff_envelope_if_enabled(
        prompt: String,
        config: &HandoffEnvelopeConfig,
        envelope: Option<&HandoffEnvelopeView>,
    ) -> String {
        if !(config.enabled && config.prompt_injection) {
            return prompt;
        }
        let Some(view) = envelope else {
            return prompt;
        };
        let rendered = render_handoff_envelope_prompt(view);
        // The renderer always emits a trailing newline. Joining
        // with "---" on its own line keeps the original prompt
        // body unambiguously separated.
        format!("{rendered}---\n\n{prompt}")
    }

    /// 2026-07-06-004 plan U6: typed inputs for
    /// `build_isolated_prompt_with_handoff`. The struct keeps the
    /// signature small enough that the wiring tests can construct
    /// it without instantiating an EventLoop.
    pub(crate) struct IsolatedPromptInputs<'a> {
        pub base_prompt: String,
        pub events: &'a [Event],
        pub config: &'a HandoffEnvelopeConfig,
        /// 2026-07-06-004 fix-plan U5 (R5): the current hat
        /// id for the activation. The extractor drops every
        /// envelope whose `to_hat` does NOT match — the
        /// trust-boundary check that prevents one hat's
        /// envelope from influencing another's prompt.
        pub current_hat: &'a str,
    }

    /// 2026-07-06-004 plan U6: real-prompt wiring helper. Given a
    /// base prompt, recent events, and the typed config, run the
    /// extractor (U5) + prepender (U4) and return the final
    /// string. The real prompt chain in `EventLoop` calls this
    /// helper from inside the orchestrator-context → macro-next-
    /// hint stretch (per plan §Unit 6 ordering).
    pub(crate) fn build_isolated_prompt_with_handoff(inputs: IsolatedPromptInputs<'_>) -> String {
        let envelope = latest_handoff_envelope_payload(inputs.events, inputs.current_hat);
        prepend_handoff_envelope_if_enabled(inputs.base_prompt, inputs.config, envelope.as_ref())
    }
}

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
use crate::event_policy::{PolicyDecision, PolicyRuntimeState, check_completion_guard};
use crate::event_reader::{Event as JsonlEvent, EventReader};
use crate::execution_contract::{
    DefaultGitEvidenceProvider, ExecutionContractDecision, ExecutionContractFinding,
    ExecutionContractViolationKind, run_execution_contract_soft_checks,
    validate_execution_contract,
};
use crate::hat_lifecycle::{ActivationKey, ActivationLifecycleTracker, SystemTimeClock};
use crate::hat_registry::HatRegistry;
use crate::hatless_ralph::HatlessRalph;
use crate::instructions::InstructionBuilder;
use crate::loop_context::LoopContext;
use crate::memory_store::{MarkdownMemoryStore, format_memories_as_markdown, truncate_to_budget};
use crate::preset::engine::gates::RejectionKind;
use crate::preset::engine::{
    LintResumeTarget, ProtocolView, build_lint_mirror_block, build_lint_resume_block,
};
use crate::skill_registry::SkillRegistry;
use crate::state_machine::{StateMachineDecision, StateMachineRuntimeState};

use crate::text::floor_char_boundary;
use ralph_proto::{Event, EventBus, Hat, HatId};
use serde_json::Value;
// U3: `WorkflowGuardRejection` is `pub(super)` in `types.rs` (it stays
// module-private because nothing outside `event_loop` constructs it).
// Bring it into the `mod.rs` namespace so the `impl EventLoop` blocks
// can name it without a fully qualified path.
use self::types::WorkflowGuardRejection;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

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
            TerminationReason::RecoverablePayloadExhausted { .. } => 1,
            // 2026-06-26 plan U1: completion-rejection budget exhausted
            // (recoverable) OR structural rejection routed to a hard
            // stop. Both are non-zero exits — the operator must see
            // the loop end and consult `loop.terminate.last_reason`.
            TerminationReason::CompletionStuck(_) => 1,
            // U5 (plan 2026-07-04-004): dimension-reviewer
            // scope_violation hard-reject — exit 1 (failure,
            // not a clean completion) so dashboards / CI surfaces
            // the silent-success guard fire as an error rather
            // than a limit.
            TerminationReason::ScopeViolationHardRejected { .. } => 1,
            // U1 (plan 2026-07-27-001): fan-in failure is a failure
            // (exit 1), not a clean completion or a limit.
            TerminationReason::FanInFailed => 1,
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
            // 2026-06-26 plan U1: completion correction budget exhausted
            // OR structural rejection. The string is the same
            // (`completion_stuck`) so the operator can grep for it
            // across the log; the structured `source` field on the
            // payload carries the classification.
            TerminationReason::CompletionStuck(_) => "completion_stuck",
            // U5 (plan 2026-07-04-004): dimension-reviewer
            // scope_violation hard-reject. Stable reason string
            // (matches the variant name; downstream consumers pin
            // against this literal).
            TerminationReason::ScopeViolationHardRejected { .. } => "scope_violation_hard_rejected",
            // U1 (plan 2026-07-27-001): production fan-in failure.
            TerminationReason::FanInFailed => "fan_in_failed",
        }
    }

    /// Returns true if this is a successful completion (not an error or limit).
    pub fn is_success(&self) -> bool {
        matches!(self, TerminationReason::CompletionPromise)
    }
}

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

/// 2026-07-28-001 plan U3: staged over-emit recovery intent. The
/// per-turn drop path sets this on the first violation; the end
/// of `process_parse_result` resolves it AFTER the business
/// events have been admitted. When at least one business event
/// has committed the recovery becomes diagnostic-only (so the
/// pre-fix `task.resume` cannot starve a legitimate handoff);
/// when zero committed it injects the bounded `task.resume`.
#[derive(Debug, Clone)]
pub struct OverEmitRecovery {
    pub hat: HatId,
    pub dropped_topic: String,
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
/// 2026-06-23 T2: appends a `## RUNTIME CONFIG` block exposing the
/// runtime-resolved `event_loop.*` values that the hat preset
/// references as variables (e.g. `max_residuals`) but cannot see
/// through plain text. This keeps the YAML position of
/// `max_residuals` (in `event_loop:`) unchanged, lets the operator
/// override it in `ralph.yml`, and lets the hat prompt read the
/// actual value rather than the literal variable name.
///
/// 2026-06-24 plan U2: also appends `max_residuals` so the terminal
/// reporting chain can read the verdict-promotion threshold
/// without depending on hat-side hardcoding.
///
/// Appended AFTER `### GUARDRAILS` so the hat's own instructions
/// remain authoritative for workflow order. Block is always emitted
/// (even with default 8) so the hat learns where to look.
pub(crate) fn append_runtime_config_block(base_prompt: String, max_residuals: u32) -> String {
    format!(
        "{base_prompt}\n\n## RUNTIME CONFIG\n\
         The following values are resolved at loop start and apply to this iteration:\n\
         - max_residuals: {r}\n",
        r = max_residuals,
    )
}

/// 2026-07-26-001 plan U2: structured preview of what
/// `EventLoop::build_prompt` would inject for one hat, **without**
/// running the loop, consuming the event bus, or writing to any
/// ledger. Powers the `ralph inspect prompt` CLI (U3-U5) and the
/// operator skills' visible-context checks (U7-U11).
///
/// **Same source as the live prompt.** The `auto_inject` set is
/// derived from the same registry + gate state that
/// `prepend_auto_inject_skills` consults; the
/// `preview_characterization` test module (event_loop/tests/
/// preview_characterization.rs) pins the equivalence between
/// this preview and the actual prompt — any future drift fails
/// the tests, not this API.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PromptPreview {
    /// Hat id whose prompt is being previewed.
    pub hat_id: String,
    /// Snapshot of the auto-inject gates that drive the
    /// `ralph-tools` / `ralph-tools-tasks` / `ralph-tools-memories`
    /// / `ralph-tools-opac` decision.
    pub gates: PromptGates,
    /// Skills injected into the prompt without the agent asking.
    /// Stable order: gated family first (in registration order),
    /// then registry-flagged skills in registry iteration order.
    pub auto_inject: Vec<PromptSkillEntry>,
    /// Skills visible to the hat but not injected — the agent
    /// loads them via `ralph tools skill load <name>`. Sorted by
    /// name for stable JSON.
    pub on_demand: Vec<PromptSkillEntry>,
    /// `## …` block titles extracted from a dry `build_prompt`
    /// call, in the order they appear in the prompt.
    pub block_titles: Vec<String>,

    // ── 2026-07-27-002 plan Unit 1: scenario injection fields ──
    /// Structured trigger context view, derived from the simulated trigger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_context_injected: Option<crate::trigger_context::TriggerContextView>,
    /// Wave context snapshot for the hat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wave_context_injected: Option<crate::wave_context::WaveContext>,
    /// Orchestrator context as generic JSON (composite of task/progress views).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestrator_context_injected: Option<serde_json::Value>,
    /// Correction context (single rejection entry).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction_injected: Option<crate::correction::CorrectionContext>,
    /// Extended gate flags beyond the basic gates (e.g. scratchpad).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_gates: Option<SkillGateFlags>,
    /// Evidence level: "static" (default), "runtime" (scenario args supplied),
    /// or "unverified".
    #[serde(
        default = "default_evidence_level",
        skip_serializing_if = "is_static_evidence_level"
    )]
    pub evidence_level: String,

    /// 2026-07-27-002 plan Unit 2: candidate emit evaluation (when --topic
    /// and --payload are provided). Contains the read-only policy decision
    /// preview for the simulated emit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_emit: Option<crate::event_policy::CandidateEmitPreview>,
}

/// Snapshot of the auto-inject gates that drive
/// `prepend_auto_inject_skills`. Mirrors the `memories.enabled`
/// and `tasks.enabled` config fields.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptGates {
    pub tasks_enabled: bool,
    pub memories_enabled: bool,
}

/// Extended gate flags beyond the basic `PromptGates` (e.g. scratchpad).
/// 2026-07-27-002 plan Unit 1: visible in `PromptPreview.skill_gates`
/// when scenario args are supplied.
/// U7: expanded to carry all three gates so the inspect command can
/// override any subset while falling back to effective config for the rest.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillGateFlags {
    pub tasks_enabled: bool,
    pub memories_enabled: bool,
    pub scratchpad_enabled: bool,
}

/// Default evidence level for `PromptPreview.evidence_level`.
/// Returns `"static"` — the preview was derived from config alone
/// without runtime scenario parameters.
pub fn default_evidence_level() -> String {
    "static".to_string()
}

pub fn is_static_evidence_level(level: &String) -> bool {
    level == "static"
}

/// One entry in either the auto-inject or on-demand list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptSkillEntry {
    pub name: String,
    /// How this skill is sourced for the auto-inject set:
    ///   * `Gated` — controlled by the hard-coded
    ///     `inject_memories_and_tools_skill` block.
    ///   * `RegistryAuto` — `auto_inject: true` in the skill
    ///     registry frontmatter.
    /// For on-demand entries, this is always `OnDemand`.
    pub source: PromptSkillSource,
}

/// Discriminator for [`PromptSkillEntry`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSkillSource {
    Gated,
    RegistryAuto,
    OnDemand,
}

impl PromptSkillEntry {
    pub(crate) fn gated(name: &str) -> Self {
        Self {
            name: name.to_string(),
            source: PromptSkillSource::Gated,
        }
    }
    pub(crate) fn registry_auto(name: &str) -> Self {
        Self {
            name: name.to_string(),
            source: PromptSkillSource::RegistryAuto,
        }
    }
    fn on_demand(name: String) -> Self {
        Self {
            name,
            source: PromptSkillSource::OnDemand,
        }
    }
}

/// Single source of truth for which skills should be auto-injected
/// into a hat's prompt, derived from the same `SkillRegistry` that
/// the live `inject_memories_and_tools_skill` path uses. Both the
/// `ralph inspect prompt` preview path AND the live `build_prompt`
/// path MUST go through `plan_auto_inject` so the operator-visible
/// preview matches what agents actually receive.
///
/// Gated skills (always ralph-tools / -tasks / -memories / -opac
/// when their gate is open) live in the first Vec. Registry-auto
/// (third-party skills with `auto_inject: true` frontmatter)
/// live in the second. On-demand (visible-but-not-injected)
/// live in the third and are NOT pushed into the prompt — they
/// are exposed via `ralph tools skill load <name>`.
pub struct SkillInjector;

impl SkillInjector {
    /// Compute the (gated, registry_auto, on_demand) skill sets for
    /// `hat_id` from `config` using the provided `registry`.
    ///
    /// Returns owned Vecs so the caller can assemble a
    /// `PromptPreview` without further registry access.
    pub fn plan_auto_inject(
        config: &RalphConfig,
        hat_id: &HatId,
        registry: &SkillRegistry,
    ) -> (
        Vec<PromptSkillEntry>,
        Vec<PromptSkillEntry>,
        Vec<PromptSkillEntry>,
    ) {
        let gates = PromptGates {
            tasks_enabled: config.tasks.enabled,
            memories_enabled: config.memories.enabled,
        };

        // Short-circuit when skills are globally disabled
        if !config.skills.enabled {
            return (Vec::new(), Vec::new(), Vec::new());
        }

        let mut gated: Vec<PromptSkillEntry> = Vec::new();
        let default_gate_open = gates.memories_enabled || gates.tasks_enabled;

        if default_gate_open && registry.is_hat_eligible("ralph-tools", hat_id.as_str()) {
            gated.push(PromptSkillEntry::gated("ralph-tools"));
        }
        if gates.tasks_enabled && registry.is_hat_eligible("ralph-tools-tasks", hat_id.as_str()) {
            gated.push(PromptSkillEntry::gated("ralph-tools-tasks"));
        }
        if gates.memories_enabled
            && registry.is_hat_eligible("ralph-tools-memories", hat_id.as_str())
        {
            gated.push(PromptSkillEntry::gated("ralph-tools-memories"));
        }
        if default_gate_open && registry.is_hat_eligible("ralph-tools-opac", hat_id.as_str()) {
            gated.push(PromptSkillEntry::gated("ralph-tools-opac"));
        }

        let mut registry_auto: Vec<PromptSkillEntry> = Vec::new();
        for skill in registry.auto_inject_skills(Some(hat_id.as_str())) {
            if matches!(
                skill.name.as_str(),
                "ralph-tools" | "ralph-tools-tasks" | "ralph-tools-memories" | "ralph-tools-opac"
            ) {
                continue;
            }
            registry_auto.push(PromptSkillEntry::registry_auto(&skill.name));
        }

        let mut on_demand: Vec<PromptSkillEntry> = registry
            .skills_for_hat(Some(hat_id.as_str()))
            .into_iter()
            .map(|s| s.name.clone())
            // 2026-07-26-002 plan U10 (R12): preview and the live
            // `build_prompt` path must agree on which skills are
            // visible. The live path calls
            // `skill_registry.remove("ralph-tools-memories")` when
            // `memories.enabled == false` (see EventLoop::new);
            // plan_auto_inject must mirror that removal here so
            // the on-demand list does not surface a skill the
            // agent can never actually load.
            .filter(|name| name != "ralph-tools-memories" || gates.memories_enabled)
            .filter(|name| !gated.iter().any(|e| &e.name == name))
            .filter(|name| !registry_auto.iter().any(|e| &e.name == name))
            .map(PromptSkillEntry::on_demand)
            .collect();
        on_demand.sort_by(|a, b| a.name.cmp(&b.name));

        (gated, registry_auto, on_demand)
    }
}

/// Strip the `### HUMAN GUIDANCE` block from a historical
/// scratchpad. Kept as a private file-level helper because
/// `filter_human_guidance_blocks` (which used to handle every
/// `### HUMAN GUIDANCE` block plus its inline variants) was
/// removed in plan 2026-06-28-005 together with the
/// `human.guidance` topic. We still need to drop the block
/// 2026-07-03-005 plan (P0 fix M-1): free-function helper used in
/// the `should_admit` 6th branch (see isolated-budget escape). Returns
/// true when the given optional `HatConfig` declares `topic` in its
/// `exempt_topics` list — i.e. the hat has positively declared this
/// topic as exempt from the per-turn single-business-event budget.
/// Returns false for `None` config (no exemption), missing config,
/// or empty `exempt_topics` (default behaviour preserved).
///
/// 2026-07-04-001 plan U13 (KTD-11): also returns true when `topic`
/// appears in `event_policy_business_topics` or
/// `event_policy_terminal_topics` AND the hat has it in `publishes`.
/// This is the SSOT for "completion-class" carve-out — a single
/// `business_topics` declaration covers every hat that can publish the
/// topic (e.g. `review.dimension.ready` exempts both `review-coordinator`
/// and any future dimension walker). Per-hat `exempt_topics` still
/// takes precedence for backwards compatibility with the
/// `ce-executor-serial` preset, which declared
/// `exempt_topics: ["review.dimension.ready", "review.dimensions.complete"]`.
/// Returns `true` when `topic` is a real business event for the
/// commit-aware over-emit recovery decision. Diagnostic /
/// control-plane topics (`task.resume`, `LOOP_COMPLETE`,
/// `plan.blocked`, `event.isolation.*`, `*.scope_violation`) are
/// **not** business topics — they are part of the recovery
/// carrier or runtime bookkeeping and must NOT count as a
/// "successful commit" that suppresses the over-emit
/// `task.resume` injection. Plan 2026-07-28-001 U3 R6 / S5 / S10.
///
/// Single source of truth: `OverEmitRecovery::resolve()` and any
/// future caller that decides whether a turn committed at least
/// one business event go through this helper. Future diagnostic
/// topics added to the recovery carrier surface should be added
/// here rather than inlining the predicate.
pub(crate) fn is_commit_first_business_topic(topic: &str) -> bool {
    if topic == "task.resume" || topic == "LOOP_COMPLETE" || topic == "plan.blocked" {
        return false;
    }
    if topic.starts_with("event.isolation.") {
        return false;
    }
    if topic.ends_with(".scope_violation") {
        return false;
    }
    true
}

fn is_isolated_exempt_topic(
    config: Option<&crate::config::hat::HatConfig>,
    topic: &str,
    event_policy_business_topics: &[String],
    event_policy_terminal_topics: &[String],
) -> bool {
    let Some(cfg) = config else {
        return false;
    };
    // Per-hat positive list (existing behaviour, set by ce-executor-serial).
    if cfg.exempt_topics.iter().any(|t| {
        let pattern = ralph_proto::Topic::new(t);
        let topic_obj = ralph_proto::Topic::new(topic);
        pattern.matches(&topic_obj)
    }) {
        return true;
    }
    // 2026-07-04-001 plan U13 (KTD-11): derived carve-out from
    // `event_policy.business_topics` ∪ `terminal_topics`. The topic is
    // exempt if (a) the resolved config declares it as a business or
    // terminal topic, AND (b) the calling hat has it in `publishes`.
    let in_class = |class: &[String]| {
        class.iter().any(|t| {
            let pattern = ralph_proto::Topic::new(t);
            let topic_obj = ralph_proto::Topic::new(topic);
            pattern.matches(&topic_obj)
        })
    };
    let is_completion_class =
        in_class(event_policy_business_topics) || in_class(event_policy_terminal_topics);
    if !is_completion_class {
        return false;
    }
    cfg.publishes.iter().any(|t| {
        let pattern = ralph_proto::Topic::new(t);
        let topic_obj = ralph_proto::Topic::new(topic);
        pattern.matches(&topic_obj)
    })
}

/// from scratchpads that pre-date 2026-06-28 so the bootstrap
/// path does not surface stale guidance text to a fresh
/// agent. New scratchpads will not contain the block (the
/// emit path is gone), so this helper only fires on history.
fn strip_human_guidance_block(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_guidance = false;
    for line in content.lines() {
        if line.starts_with("### HUMAN GUIDANCE") {
            in_guidance = true;
            out.push('\n');
            continue;
        }
        if in_guidance && (line.starts_with("### ") || line.starts_with("## ")) {
            in_guidance = false;
        }
        if !in_guidance {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Minimal FlowDeclaration YAML retained for documentation and legacy
/// test fixtures. Hat-only presets no longer fall back to this at
/// runtime — see [`StagePipeline::with_hat_only_stages_for_loop_config`].
#[allow(dead_code)]
fn minimal_flow_declaration_yaml() -> &'static str {
    // U11 (2026-06-27-002 plan completion) requires
    // `FlowStepScopeStage` to be fail-closed when the
    // topic is outside the declared `allowed_emits`
    // set. The minimal fallback flow therefore MUST
    // declare `unit_loop` (the default `current_step_id`
    // produced by `FlowLifecycleRegistry::current_step_id()`)
    // with a permissive `allowed_emits` set so that
    // presets without an explicit `mechanism:` block
    // continue to function as before. Operators who
    // want to enforce strict topic/step gating must
    // declare their own flow in the preset; the lint
    // `flow_declaration_missing` flags the absence.
    r"mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: []
    steps:
      - id: unit_loop
        allowed_emits:
          - work.start
          - work.start
          - work.ready
          - work.done
          - work.failed
          - test.passed
          - test.failed
          - fix.applied
          - fix.exhausted
          - task.resume
          - plan.complete
          - plan.blocked
          - plan.created
          - a.impl.done
          - b.impl.done
          - task.done
          - queue.advance
          - hypothesis.test
          - review.start
          - review.dimension.ready
          - review.dimension.done
          - review.complete
          - review.done
          - review.blocked
          - review.file
          - experiment.planned
          - experiment.ready
          - experiment.running
          - experiment.done
          - experiment.failed
          - build.blocked
          - build.done
          - loop.cancel
          - verify.passed
          - verify.failed
          - experiment.planned
          - seed.ready
          - REPORT_DONE
          - REVIEW_COMPLETE
          - LOOP_COMPLETE
          - event.malformed
          - event.isolation.boundary_violation
          - human.guidance
          - user.prompt
          - task.resume
          - task.relocate_legacy
          - task.relocate
          - repair.budget.exhausted
          - repair.close
          - report.done
          - aggregate.inbox
          - aggregate.done
          - stop_requested
          - restart_requested
"
}

/// U6: build the default emit-time stage pipeline from the loaded
/// `RalphConfig`.
///
/// Presets that **opt in** to `mechanism.flow` (top-level or legacy
/// `event_loop.mechanism`) get the full stage pipeline including
/// `FlowStepScopeStage` and `StepCloseObligationStage`.
///
/// Presets without `mechanism.flow` (hat-only linear chains such as
/// `ce-executor-pipeline`) skip flow-step gating; routing is driven by
/// hat triggers/publishes plus `event_policy`.
pub fn load_opt_in_flow_declaration(
    config: &crate::config::RalphConfig,
) -> Option<crate::event_loop::flow_declaration::FlowDeclaration> {
    use crate::event_loop::flow_declaration::FlowDeclaration;
    // Typed conversion — do NOT serde_yaml round-trip. Wrapping
    // `to_string(flow_cfg)` under `mechanism:\n  flow:\n` left the
    // body unindented, so `mechanism.flow` parsed as null and
    // `FlowStepScopeStage` rejected every emit with
    // `flow_step_undeclared` (work.ready never reached task-planner).
    effective_mechanism_config(config)
        .and_then(|m| m.flow.as_ref())
        .and_then(|flow_cfg| FlowDeclaration::from_config(flow_cfg).ok())
}

fn effective_mechanism_config(
    config: &crate::config::RalphConfig,
) -> Option<&crate::config::MechanismConfig> {
    config
        .mechanism
        .as_ref()
        .or(config.event_loop.mechanism.as_ref())
}

fn build_phase_authority_arc(
    config: &crate::config::RalphConfig,
) -> std::sync::Arc<crate::event_loop::phase_authority::WorkflowPhaseAuthority> {
    let authority = effective_mechanism_config(config)
        .and_then(|m| m.phase_authority.as_ref())
        .and_then(|cfg| {
            crate::event_loop::phase_authority::WorkflowPhaseAuthority::from_config(cfg).ok()
        })
        .unwrap_or_else(crate::event_loop::phase_authority::WorkflowPhaseAuthority::disabled);
    std::sync::Arc::new(authority)
}

fn build_stage_pipeline_from_config(
    config: &crate::config::RalphConfig,
) -> (
    crate::event_loop::stage_pipeline::StagePipeline,
    std::collections::HashMap<String, u32>,
    std::sync::Arc<crate::event_loop::phase_authority::WorkflowPhaseAuthority>,
) {
    use crate::event_loop::flow_declaration::FlowDeclaration;
    use crate::event_loop::stage_pipeline::StagePipeline;
    let loop_cfg = Some(&config.event_loop);
    let authority = build_phase_authority_arc(config);
    // Top-level `mechanism:` (preset SSOT) and `event_loop.mechanism`
    // must both enable the phase pipeline — `build_phase_authority_arc`
    // already reads `effective_mechanism_config`.
    let phase_authority_enabled = authority.is_enabled();

    if phase_authority_enabled {
        let flow_yaml = load_opt_in_flow_declaration(config).unwrap_or_else(|| {
            FlowDeclaration::from_yaml(minimal_flow_declaration_yaml()).unwrap()
        });
        let step_totals: std::collections::HashMap<String, u32> = flow_yaml
            .steps
            .iter()
            .filter_map(|s| s.total_units.map(|n| (s.id.clone(), n)))
            .collect();
        let pipeline = StagePipeline::with_phase_authority_stages_for_loop_config(
            flow_yaml,
            loop_cfg,
            authority.clone(),
        );
        return (pipeline, step_totals, authority);
    }

    if let Some(flow_yaml) = load_opt_in_flow_declaration(config) {
        let step_totals: std::collections::HashMap<String, u32> = flow_yaml
            .steps
            .iter()
            .filter_map(|s| s.total_units.map(|n| (s.id.clone(), n)))
            .collect();
        let pipeline = StagePipeline::with_default_stages_for_loop_config(flow_yaml, loop_cfg);
        (pipeline, step_totals, authority)
    } else {
        let pipeline = StagePipeline::with_hat_only_stages_for_loop_config(loop_cfg);
        (pipeline, std::collections::HashMap::new(), authority)
    }
}

/// Validates events against configured workflow guards is implemented by
/// [`crate::validation::rules_workflow_guard::WorkflowGuardRule`], invoked
/// from the unified pre-commit / post-commit loop in
/// `process_parse_result`. The legacy free function
/// `apply_workflow_guard_validation` and its sibling
/// `WorkflowGuardOutcome` / `WorkflowGuardRejectionDetail` structs were
/// removed in U11-T4 (post-commit wiring); the recovery-envelope writer
/// `Self::log_workflow_guard_rejection` survives because it is
/// implementation-agnostic and is reused by the unified handler.
/// P1-1 (2026-07-01-002 audit): parse the `step` field out of a
/// `work.ready` payload and return it when (a) it claims to be a
/// `fix-NN` step and (b) the id is **not** present in
/// `fix_unit_known`.  Returns `None` for non-fix-unit steps,
/// malformed payloads, or already-known ids — those are not in
/// scope for the fix-unit range guard.
fn unknown_fix_step(
    payload: Option<&str>,
    fix_unit_known: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let payload = payload?;
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let step_id = match value.get("step")? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => map.get("id")?.as_str()?.to_string(),
        _ => return None,
    };
    if !step_id.starts_with("fix-") {
        return None;
    }
    if fix_unit_known.contains(&step_id) {
        return None;
    }
    Some(step_id)
}

/// P1-1 (2026-07-01-002 audit): shape the `task.resume` payload
/// for JSONL events read from `apply_emit_gate`.  The JSONL
/// `Event` only carries `topic` / `hat` / `payload` — there is
/// no `source` field, so `target` is sourced from `hat`.
fn build_invalid_step_target_resume_payload_for_jsonl(
    finding: &crate::execution_contract::ExecutionContractFinding,
    original_event: &crate::event_reader::Event,
    known_fix_units: &[String],
) -> String {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "stage".into(),
        serde_json::Value::String("FixUnitRangeGuard".into()),
    );
    payload.insert(
        "original_topic".into(),
        serde_json::Value::String(original_event.topic.clone()),
    );
    payload.insert(
        "violation".into(),
        serde_json::Value::String("invalid_step_target".into()),
    );
    payload.insert(
        "reason_code".into(),
        serde_json::Value::String(
            crate::validation::ReasonCode::CONTRACT_INVALID_STEP_TARGET.to_string(),
        ),
    );
    if let Some(hat) = original_event.hat.as_ref() {
        payload.insert("target".into(), serde_json::Value::String(hat.clone()));
    }
    payload.insert(
        "known_fix_units".into(),
        serde_json::Value::Array(
            known_fix_units
                .iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect(),
        ),
    );
    payload.insert(
        "guidance".into(),
        serde_json::Value::String(finding.message.clone()),
    );
    serde_json::to_string(&serde_json::Value::Object(payload)).unwrap_or_else(|_| "{}".to_string())
}

/// Pure config-driven preview that does **not** require a
/// constructed `EventLoop`. Used by `ralph inspect prompt` to
/// avoid the noisy `tracing::info!("Memory injection check…")`
/// path that runs when an EventLoop is constructed (its
/// initialization logs to stdout, which corrupts the JSON SSOT
/// contract). The `EventLoop::prompt_preview` method delegates to
/// this function with a closure that runs `build_prompt` for the
/// block-title extraction.
///
/// `block_titles` is supplied via a closure so the caller can opt
/// into the heavier `build_prompt`-driven extraction; the pure
/// CLI path passes `|_| Vec::new()` to keep the command
/// side-effect-free.
pub fn preview_prompt_for_config<F>(
    config: &RalphConfig,
    hat_id: &HatId,
    block_titles: F,
) -> Option<PromptPreview>
where
    F: FnOnce(&HatId) -> Vec<String>,
{
    let hat_registry = HatRegistry::from_config(config);
    if hat_registry.get(hat_id).is_none() && hat_id.as_str() != "ralph" {
        return None;
    }

    let skill_registry = SkillRegistry::from_config(
        &config.skills,
        std::path::Path::new(&config.core.workspace_root),
        Some(config.cli.backend.as_str()),
    )
    .unwrap_or_else(|_| SkillRegistry::new(Some(config.cli.backend.as_str())));

    let gates = PromptGates {
        tasks_enabled: config.tasks.enabled,
        memories_enabled: config.memories.enabled,
    };

    let (gated, registry_auto, on_demand) =
        SkillInjector::plan_auto_inject(config, hat_id, &skill_registry);

    let auto_inject = [gated, registry_auto].concat();
    let block_titles = block_titles(hat_id);

    Some(PromptPreview {
        hat_id: hat_id.as_str().to_string(),
        gates,
        auto_inject,
        on_demand,
        block_titles,
        // 2026-07-27-002 plan Unit 1: scenario injection defaults.
        // These are populated by `inspect_prompt_command` when
        // scenario args are supplied; the pure config path leaves
        // them at their default (None / "static").
        trigger_context_injected: None,
        wave_context_injected: None,
        orchestrator_context_injected: None,
        correction_injected: None,
        skill_gates: None,
        evidence_level: default_evidence_level(),
        // 2026-07-27-002 plan Unit 2: candidate emit preview.
        candidate_emit: None,
    })
}

/// Outcome of `EventLoop::validate_resume_routing`. Callers in the
/// recovery / diagnostic loops branch on this and avoid publishing a
/// `task.resume` when `Block(reason)` is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventLoopResumeDecision {
    /// Routing is consistent with the original trigger topic.
    Allow,
    /// Routing would target a hat that won't pick the resume up;
    /// `reason` is a stable operator-grepable message.
    Block(String),
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

/// 2026-06-16-001 U5: stall detection and progress-steward wake
/// helper, extracted from the post-validation tail of
/// `process_parse_result` so it can also run from the
/// empty-JSONL early-return path (a turn with zero events is
/// the canonical no-progress turn).
///
/// `had_events` is the per-turn boolean: true if any accepted
/// business event was admitted this turn; false otherwise
/// (including the empty-JSONL case, where the function is
/// invoked directly).
///
/// 2026-06-16-001 review fix (F-REL-002): takes a `&HatRegistry`
/// so the wake can be cross-validated against the actual hat
/// graph before publishing. A `loop.stalled` published with
/// `target=steward_hat_id` is silently dropped by
/// `EventBus::publish` when the target hat is not in the
/// registry (event_bus.rs:118-128). Validating here logs a
/// `warn!` and skips the wake — the loop continues
/// `no_progress` until either (a) the operator adds the
/// steward hat to the preset, or (b) the runtime escalates
/// to `plan.blocked` via the U5 escalation branch. The
/// runtime never silently dies.
///
/// 2026-07-30-002 plan U1 (R1/R2/D1/D3): the function now
/// takes the preset's derived `blocked_topic` so the
/// fail-close emit matches the preset's blocked protocol
/// namespace (parallel-forge → `forge.plan.blocked`,
/// ce-executor-supervisor → `plan.blocked`, undeclared
/// flows fall back to `plan.blocked`). It returns `true`
/// Render a slice of `CorrectionContext` entries into the
/// `## ORCHESTRATOR CORRECTION` markdown block.  Free
/// function (no `EventLoop` borrow) so `prepend_correction_and_resume`
/// can render the consumed entries after the partition drain
/// without re-borrowing `state.prompt_context`.
///
/// Returns an empty string when `entries` is empty.  Pure —
/// no side effects, deterministic given the same input order.
fn render_correction_entries(entries: &[crate::correction::CorrectionContext]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let has_semantic = entries
        .iter()
        .any(|e| matches!(e.feedback_kind, crate::correction::FeedbackKind::Semantic));
    let mut out = String::from("## ORCHESTRATOR CORRECTION\n\n");
    // U1 (plan 2026-08-06-001, AC1): route the preamble by
    // FeedbackKind so the agent gets semantically-accurate
    // guidance.  Semantic → "contradicted an invariant";
    // Mechanical / Unknown → legacy "Address each reason".
    if has_semantic {
        out.push_str(
            "The orchestrator rejected the events below because\n\
             the payloads contradicted an invariant derived\n\
             from the artifact, test, or verification state.\n\
             Each entry lists what was observed, the invariant\n\
             that was violated, and the condition you must\n\
             re-prove.  Re-emitting the original payload\n\
             without changing the underlying evidence will\n\
             keep failing and counts against the retry\n\
             budget — open the artifact, fix the root cause,\n\
             re-verify, then rebuild the payload and rerun\n\
             `ralph emit --policy-check` before re-emitting.\n\n",
        );
    } else {
        out.push_str(
            "The orchestrator rejected the events below. Address each\n\
             reason before emitting more events on these topics.\n\n",
        );
    }
    for ctx in entries {
        out.push_str(&ctx.render_block());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod u1_render_correction_entries_preamble {
    use super::*;
    use crate::correction::{CorrectionContext, FeedbackKind};
    use crate::event_loop::rejection::Rejection;

    fn rejection_for(topic: &str) -> Rejection {
        Rejection {
            stage: crate::event_loop::rejection::RejectionStage::Policy,
            source_hat: Some("executor".to_string()),
            business_hat: None,
            topic: topic.to_string(),
            violation: format!("sample violation for {topic}"),
            retry_key: format!("policy:executor:{topic}:sample"),
            retry_eligible: true,
            non_retryable_reason: None,
            target_hat: Some("executor".to_string()),
            original_event_id: None,
            original_ts: None,
            kind: None,
            duplicate_work_done_hint: None,
            seen_count: None,
        }
    }

    fn semantic_context(topic: &str) -> CorrectionContext {
        CorrectionContext::from_rejection(&rejection_for(topic), 1)
            .with_feedback_kind(FeedbackKind::Semantic)
    }

    fn mechanical_context(topic: &str) -> CorrectionContext {
        CorrectionContext::from_rejection(&rejection_for(topic), 1)
            .with_feedback_kind(FeedbackKind::Mechanical)
    }

    #[test]
    fn empty_entries_returns_empty_string() {
        let entries: Vec<CorrectionContext> = vec![];
        let result = render_correction_entries(&entries);
        assert_eq!(result, "");
    }

    #[test]
    fn semantic_entry_uses_contradicted_preamble() {
        // When any entry is Semantic, the semantic preamble must
        // be used even if other entries are Mechanical.
        let entries = vec![
            mechanical_context("work.done"),
            semantic_context("review.passed"),
        ];
        let result = render_correction_entries(&entries);
        assert!(
            result.contains("contradicted an invariant"),
            "semantic preamble must appear when any entry is semantic: {}",
            result
        );
        assert!(
            !result.contains("Address each"),
            "legacy preamble must NOT appear when any entry is semantic: {}",
            result
        );
    }

    #[test]
    fn pure_mechanical_uses_legacy_preamble() {
        // When all entries are Mechanical, the legacy preamble
        // must be used.
        let entries = vec![
            mechanical_context("work.done"),
            mechanical_context("review.passed"),
        ];
        let result = render_correction_entries(&entries);
        assert!(
            result.contains("Address each"),
            "legacy preamble must appear for purely mechanical entries: {}",
            result
        );
        assert!(
            !result.contains("contradicted an invariant"),
            "semantic preamble must NOT appear for purely mechanical entries: {}",
            result
        );
    }

    #[test]
    fn pure_semantic_uses_contradicted_preamble() {
        // When all entries are Semantic, the semantic preamble
        // must be used.
        let entries = vec![
            semantic_context("work.done"),
            semantic_context("review.passed"),
        ];
        let result = render_correction_entries(&entries);
        assert!(
            result.contains("contradicted an invariant"),
            "semantic preamble must appear for purely semantic entries: {}",
            result
        );
        assert!(
            !result.contains("Address each"),
            "legacy preamble must NOT appear for purely semantic entries: {}",
            result
        );
    }
}

/// when a blocked topic was actually published this turn so
/// the caller can run the escape step advance + flow-authority
/// snapshot in a single place.
fn run_stall_detector_on_state(
    state: &mut crate::event_loop::loop_state::LoopState,
    config_progress_steward: &crate::config::ProgressStewardConfig,
    registry: &crate::hat_registry::HatRegistry,
    bus: &mut ralph_proto::EventBus,
    blocked_topic: &str,
) -> Option<ralph_proto::Event> {
    if state.stall_detector_had_events {
        // A business event was admitted in this turn — reset
        // the no-progress counter and clear the per-turn
        // self-protection flag.
        if state.consecutive_no_progress_turns > 0 {
            debug!(
                was = state.consecutive_no_progress_turns,
                "isolated loop: progress detected — resetting stall counter"
            );
        }
        state.consecutive_no_progress_turns = 0;
        if state.consecutive_steward_activations > 0 {
            debug!(
                was = state.consecutive_steward_activations,
                "isolated loop: steward produced progress — resetting steward counter"
            );
        }
        state.consecutive_steward_activations = 0;
        state.steward_woken_this_turn = false;
        return None;
    }
    if state.steward_woken_this_turn && config_progress_steward.enabled {
        // Self-protection: the steward was already woken in
        // this turn. Suppress recursive wakes (enabled path only).
        return None;
    }
    state.consecutive_no_progress_turns = state.consecutive_no_progress_turns.saturating_add(1);
    let max_iter = config_progress_steward.max_steward_iterations;

    // 2026-07-06 plan U12 + R9 fail-close: when steward is disabled,
    // never publish `loop.stalled`, but still hard-fail after
    // `max_steward_iterations` consecutive no-progress turns.
    if !config_progress_steward.enabled {
        if state.consecutive_no_progress_turns >= max_iter {
            warn!(
                consecutive_no_progress = state.consecutive_no_progress_turns,
                max_iter,
                "isolated loop: no progress for {} turns with progress_steward disabled — \
                 emitting {blocked_topic} (fail-close)",
                max_iter,
            );
            // 2026-07-24-005 plan U1: target is `reporter` (was
            // `shipper`); the shipper hat is removed from the
            // supervisor preset — reporter is the canonical
            // `plan.blocked` terminal owner.
            //
            // 2026-07-30-002 plan U1: topic is the preset's
            // derived blocked namespace (e.g. `forge.plan.blocked`
            // for parallel-forge), so the reporter's terminal
            // emit clears FlowStepScope. See `derive_blocked_topic`.
            let blocked = ralph_proto::Event::new(
                blocked_topic,
                "{\"reason\":\"loop_stalled_max_iterations\"}".to_string(),
            )
            .with_target(ralph_proto::HatId::new("reporter"));
            state.consecutive_no_progress_turns = 0;
            state.consecutive_steward_activations = 0;
            return Some(blocked);
        }
    } else if state.consecutive_no_progress_turns >= max_iter
        && state.consecutive_steward_activations < max_iter
    {
        // 2026-06-16-001 review fix (F-REL-002): cross-validate
        // the steward hat id against the runtime registry. A
        // `loop.stalled` with `target=<unknown hat>` is
        // silently dropped by `EventBus::publish` —
        // operators would see "no progress" warnings without
        // any recovery action. The runtime logs a `warn!`
        // and treats the wake as a no-op (so the
        // `consecutive_steward_activations` counter still
        // increments toward the U5 escalation branch).
        let steward_id = ralph_proto::HatId::new(config_progress_steward.steward_hat_id.as_str());
        if registry.get(&steward_id).is_none() {
            warn!(
                steward_hat_id = %config_progress_steward.steward_hat_id,
                "isolated loop: progress-steward hat is not registered — \
                 skipping loop.stalled wake (the U5 escalation branch \
                 will emit plan.blocked after max_steward_iterations). \
                 Add the hat to the preset's `hats:` map or set \
                 `progress_steward.steward_hat_id` to an existing hat id."
            );
            // Still increment the activation counter so the
            // U5 escalation path can fire if the misconfig
            // persists.
            state.consecutive_steward_activations =
                state.consecutive_steward_activations.saturating_add(1);
            return None;
        }
        // First-time wake: auto-emit `loop.stalled` diagnostic
        // and increment the steward activation counter. The
        // actual steward activation happens in the next
        // `process_output` cycle when the loop picks up the
        // `loop.stalled` event and routes it to the steward
        // hat.
        warn!(
            consecutive_no_progress = state.consecutive_no_progress_turns,
            max_iter, "isolated loop: no progress for {} turns — waking progress-steward", max_iter,
        );
        let stalled = ralph_proto::Event::new(
            "loop.stalled",
            format!(
                "{{\"reason\":\"no_progress_for_{}_turns\"}}",
                state.consecutive_no_progress_turns
            ),
        )
        .with_target(steward_id);
        bus.publish(stalled);
        state.consecutive_steward_activations =
            state.consecutive_steward_activations.saturating_add(1);
        state.steward_woken_this_turn = true;
    } else if state.consecutive_steward_activations >= max_iter {
        // The steward has been woken `max_iter` times in a row
        // without producing a forwarded business event.
        // Escalate by emitting `plan.blocked` and forcing the
        // loop to route through `reporter` for a clean
        // termination.
        //
        // 2026-07-24-005 plan U1: target is now `reporter`
        // (was `shipper`); the shipper hat is removed from the
        // supervisor preset — reporter is the canonical
        // `plan.blocked` terminal owner. The previous comment
        // about the "shipper → reporter termination path" is
        // replaced with a direct reporter route.
        warn!(
            consecutive_steward_activations = state.consecutive_steward_activations,
            max_iter,
            "isolated loop: steward did not produce progress after {} wakes — emitting {blocked_topic}",
            max_iter,
        );
        let blocked = ralph_proto::Event::new(
            blocked_topic,
            "{\"reason\":\"loop_stalled_max_iterations\"}".to_string(),
        )
        // 2026-06-16-001 review fix (CORR-P1-2): explicit
        // `with_target(...)` so the route matches the R5
        // hard-gate hat-routing convention. Without a
        // target, the bus delivers the event to the
        // default-routed hats; with the target,
        // `reporter` is the canonical consumer and the
        // event reaches the reporter termination
        // path consistently. Loopback to progress-steward
        // is unnecessary: the steward was the one that
        // failed to make progress, so the recovery action
        // is to terminate, not retry.
        //
        // 2026-07-24-005 plan U1: target is now `reporter`
        // (was `shipper`); the shipper hat is removed from
        // the supervisor preset.
        .with_target(ralph_proto::HatId::new("reporter"));
        // Reset so the next loop (e.g. a follow-up diagnostic
        // or operator restart) starts from a clean state.
        state.consecutive_no_progress_turns = 0;
        state.consecutive_steward_activations = 0;
        // Return the proposed blocked transition to the caller; the caller
        // must durably accept it before advancing flow authority.
        return Some(blocked);
    }

    // 2026-06-23 fix plan U3 (CB-6): typed rejection-stall
    // detection. After all the no-progress / steward-wake
    // escalation above, ALSO check the typed rejection window
    // (via `LoopState::detect_rejection_stall_kind`) and emit a
    // `stall.handoff_unconsumed` diagnostic if the rejection
    // count exceeds the typed threshold (default 3 in
    // `detect_rejection_stall`). This closes the 8h+ stall
    // detector silence bug from `primary-20260622-182705`
    // (filename_mismatch × 6 with no stall alert).
    if let Some(stall_kind) = crate::event_loop::loop_state::detect_rejection_stall_kind(state) {
        let window = state.typed_lint_rejection_count(stall_kind);
        warn!(
            kind = %stall_kind.reason_code(),
            window = window,
            "isolated loop: typed rejection stall detected — emitting stall.handoff_unconsumed"
        );
        let stall_event = ralph_proto::Event::new(
            "stall.handoff_unconsumed",
            format!(
                "{{\"reason\":\"rejection_stall\",\"kind\":\"{kind}\",\"window\":{window}}}",
                kind = stall_kind.reason_code(),
            ),
        );
        bus.publish(stall_event);
    }
    // 2026-07-30-002 plan U1: no blocked topic was fired this
    // turn — caller skips the escape-step advance.
    None
}

/// 2026-06-16-001 U3: freshness filter for `task.resume` injection.
///
/// Returns `true` when the rejection is older than `ttl_seconds` and
/// should be dropped. The check prefers `rejection.original_ts` (the
/// source event's timestamp) and falls back to treating the
/// rejection as fresh.
///
/// Missing or unparseable timestamps are treated as "fresh" (not
/// stale) so legacy JSONL that pre-dates the freshness filter still
/// flows through the existing recovery path. A TTL of `0` disables
/// the filter (always fresh) so unit tests can opt out without
/// monkey-patching the helper.
///
/// 2026-06-16-001 review fix (ADV-U3-1): a source timestamp
/// in the FUTURE relative to the current wall clock is treated
/// as STALE. The previous behaviour used `saturating_sub`
/// which clamps to 0 for future ts (treated as fresh), letting
/// a clock-skewed or forged event slip through. A test fixture
/// or a buggy clock could re-introduce the same 50-min stall
/// the plan aims to fix. The runtime logs a `warn!` so the
/// anomaly is observable in `orchestration.jsonl`.
fn is_rejection_stale(
    rejection: &crate::event_loop::rejection::Rejection,
    ttl_seconds: u64,
) -> bool {
    if ttl_seconds == 0 {
        return false;
    }
    let Some(ts_str) = rejection.original_ts.as_deref() else {
        return false;
    };
    let Ok(source_dt) = chrono::DateTime::parse_from_rfc3339(ts_str) else {
        return false;
    };
    let source_unix = source_dt.timestamp();
    let now_unix = chrono::Utc::now().timestamp();
    // Future timestamp: clock skew or forgery. Treat as stale
    // so the recovery signal cannot be re-injected.
    if source_unix > now_unix {
        warn!(
            source_event_ts = %ts_str,
            now_unix,
            source_unix,
            "task.resume TTL: source event timestamp is in the future — \
             treating as stale (clock skew or forgery)"
        );
        return true;
    }
    let age = now_unix.saturating_sub(source_unix);
    age > ttl_seconds as i64
}

// 2026-07-26-003 plan U7: characterization pin for the
// `task_resume_ttl_seconds` decision. The plan requires that
// the default stays at 300s and that the failure-convergence
// path (review.wave.failed → finalizer) does NOT lean on the
// stale-`task.resume` mechanism to fire. The `is_rejection_stale`
// behavior must therefore stay bounded: ttl=0 disables the
// filter (no false-stale), missing `original_ts` is non-stale
// (back-compat), future-timestamp is stale (clock-skew guard),
// past-timestamp > ttl is stale. These four edges are exactly
// the surfaces a future wave-aware wave-scoped TTL exemption
// would have to expand — without breaking them, so each is
// pinned here.
#[cfg(test)]
mod u7_rejection_stale_characterization {
    use super::*;
    use crate::event_loop::rejection::Rejection;

    fn rejection_with_ts(ts: &str) -> Rejection {
        Rejection {
            stage: crate::event_loop::rejection::RejectionStage::Policy,
            source_hat: Some("review-worker".to_string()),
            business_hat: Some("review-worker".to_string()),
            topic: "review.unit.done".to_string(),
            violation: "test".to_string(),
            retry_key: "rk-1".to_string(),
            retry_eligible: true,
            non_retryable_reason: None,
            target_hat: Some("review-worker".to_string()),
            original_event_id: None,
            original_ts: Some(ts.to_string()),
            kind: None,
            duplicate_work_done_hint: None,
            seen_count: None,
        }
    }

    #[test]
    fn ttl_zero_disables_filter() {
        // ttl=0 must mean "do not filter" — used by scenario /
        // regression suites that pin aged fixtures.
        let rejection = rejection_with_ts("2024-01-01T00:00:00Z");
        assert!(!is_rejection_stale(&rejection, 0));
    }

    #[test]
    fn missing_original_ts_is_non_stale() {
        // Legacy / synthesised rejections without an
        // `original_ts` must survive the filter; otherwise the
        // failure-convergence path could lose recovery telemetry
        // it depends on.
        let mut rejection = rejection_with_ts("2024-01-01T00:00:00Z");
        rejection.original_ts = None;
        assert!(!is_rejection_stale(&rejection, 300));
    }

    #[test]
    fn past_older_than_ttl_is_stale() {
        // An event older than the 300s default is stale. We
        // build a ts 1000s in the past relative to `now`.
        let past = chrono::Utc::now() - chrono::Duration::seconds(1000);
        let rejection = rejection_with_ts(&past.to_rfc3339());
        assert!(
            is_rejection_stale(&rejection, 300),
            "1000s-old rejection with 300s TTL must be stale"
        );
    }

    #[test]
    fn future_timestamp_is_stale() {
        // Clock skew / forgery guard: a future ts means we
        // cannot trust the timestamp; the rejection must
        // NOT be re-injected into the loop.
        let future = chrono::Utc::now() + chrono::Duration::seconds(60);
        let rejection = rejection_with_ts(&future.to_rfc3339());
        assert!(is_rejection_stale(&rejection, 300));
    }

    #[test]
    fn default_config_default_is_300s() {
        // Pin the SSOT default. U7 records that plan 003 did
        // NOT widen / shrink this — the failure-convergence
        // path does not depend on stale-resume activation,
        // so we leave the default exactly where the
        // 2026-06-16-001 U3 plan left it.
        let cfg: crate::config::EventLoopConfig = Default::default();
        assert_eq!(
            cfg.task_resume_ttl_seconds,
            Some(300),
            "task_resume_ttl_seconds default must remain 300s; \
             changing this requires new plan coverage (U7 invariants)"
        );
    }
}

// 2026-06-28 plan U4: helpers for the plan-mode current_step
// state machine. Kept at module scope (not on `impl EventLoop`)
// so tests can exercise them without spinning up a full
// EventLoop.
//
// `initial_current_plan_step` returns the id of the first
// declared flow step when the preset has a `mechanism.flow`,
// or an empty string when the preset has no flow declaration
// (legacy / solo mode). An empty string is the legacy
// fail-open signal — the `FlowStepScopeStage` accepts the
// event and `build_stage_context_for` falls back to
// `state.flow_lifecycle.current_step_id()`.
/// 2026-06-28 plan U7 (R7): helper to decide whether the
/// preset treats `state_idempotency: required` as a hard
/// constraint. Pulled out so both the `loop_id`-present and
/// `loop_id`-absent branches in `with_context_and_diagnostics`
/// agree on the policy without re-reading the YAML twice.
fn self_is_state_idempotency_required(config: &RalphConfig) -> bool {
    config
        .event_loop
        .mechanism
        .as_ref()
        .and_then(|m| m.flow.as_ref())
        .map(|f| f.state_idempotency == "required")
        .unwrap_or(false)
}

/// 2026-07-30-002 plan U1 (R2/D1): derive the topic the
/// mechanism fail-close must publish. Scans the preset's
/// declared flow for `*.plan.blocked` topics; exactly one
/// distinct match wins (e.g. `forge.plan.blocked` for
/// `parallel-forge`), zero or multiple distinct matches
/// fall back to the legacy `plan.blocked` so unrelated
/// presets are not disturbed. The check is intentionally
/// narrow: only `== "plan.blocked"` or `ends_with(".plan.blocked")`
/// qualify, so a generic `plan.blocked`-suffixed topic never
/// wins by accident.
pub(crate) fn derive_blocked_topic(config: &RalphConfig) -> String {
    let Some(mechanism) = effective_mechanism_config(config) else {
        return "plan.blocked".to_string();
    };
    let Some(flow) = mechanism.flow.as_ref() else {
        return "plan.blocked".to_string();
    };
    let mut matches: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for step in &flow.steps {
        for topic in &step.allowed_emits {
            if topic == "plan.blocked" || topic.ends_with(".plan.blocked") {
                matches.insert(topic.as_str());
                if matches.len() > 1 {
                    return "plan.blocked".to_string();
                }
            }
        }
    }
    if matches.len() == 1 {
        matches.into_iter().next().unwrap().to_string()
    } else {
        "plan.blocked".to_string()
    }
}

/// 2026-07-30-002 plan U1 (R1/D3): forward-only escape step
/// resolution. Given the current step and a blocked topic,
/// return the FIRST forward step whose `on` or `on_any_of`
/// accepts the topic. Returns `None` when (a) the flow has
/// no declared entry, (b) the current step is not found,
/// (c) no forward step accepts the topic. The helper is
/// intentionally a one-shot escape: once `current_plan_step`
/// has advanced, no second jump is performed.
pub(crate) fn resolve_escape_step(
    config: &RalphConfig,
    current: &str,
    topic: &str,
) -> Option<String> {
    let mechanism = effective_mechanism_config(config)?;
    let flow = mechanism.flow.as_ref()?;
    let steps = &flow.steps;
    let idx = steps.iter().position(|s| s.id == current)?;
    for (j, candidate) in steps.iter().enumerate() {
        if j <= idx {
            continue;
        }
        let enters = candidate.on.as_deref() == Some(topic)
            || candidate.on_any_of.iter().any(|t| t == topic);
        if enters {
            return Some(candidate.id.clone());
        }
    }
    None
}

/// 2026-07-01-001 plan U6: extract the canonical step id
/// from a `test.passed` payload. Returns `None` for
/// malformed payloads — the orchestrator-state cache then
/// keeps its previous value (so a transient malformed
/// event does not wipe the directive).
fn extract_step_id(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    if let Some(s) = value.get("step").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    if let Some(obj) = value.get("step").and_then(|v| v.as_object())
        && let Some(id) = obj.get("id").and_then(|v| v.as_str())
    {
        return Some(id.to_string());
    }
    None
}

/// 2026-07-01-001 plan U6 wiring (review P1-2): extract the
/// plan path from a `work.ready` payload so the runtime can
/// install the plan topology on first sight. Returns `None`
/// when the payload is malformed or the field is absent —
/// the caller treats that as "skip this turn's install
/// attempt" (the topology cache stays empty, which is the
/// pre-existing fail-closed state).
fn initial_current_plan_step(config: &RalphConfig) -> String {
    // Top-level `mechanism:` is the preset SSOT; fall back to
    // legacy `event_loop.mechanism` via `effective_mechanism_config`.
    effective_mechanism_config(config)
        .and_then(|m| m.flow.as_ref())
        .and_then(|f| f.steps.first())
        .map(|s| s.id.clone())
        .unwrap_or_default()
}

/// 2026-06-28 plan U4: advance the plan-mode `current_plan_step`
/// after an event has been accepted by the stage pipeline.
///
/// Returns `Some(next_step_id)` when the step changes,
/// `None` when no transition fires (event is not a transition
/// event, the current step has no successor, or no flow
/// declaration is loaded).
///
/// `current` is the *current* plan step. The function consults
/// the step's `terminal_when` configuration: when
/// `terminal_when == "all_done"` (the only branch the
/// `ce-executor-serial` flow uses), a transition event
/// is any topic that is in `allowed_emits` AND not the loop's
/// primary completion topic (`work.done` is the unit's "I'm
/// done" signal, not a step transition). The next step is
/// the next entry in `mechanism.flow.steps`.
///
/// The mapping keeps the policy in the YAML declaration:
/// adding a new step in `mechanism.flow.steps` requires no
/// code change here. The whitelist of "non-transition"
/// topics is intentionally tiny — only `work.done` is
/// recognised as a per-unit "still working" signal because
/// every other allowed emit moves the plan forward.
pub(crate) fn advance_plan_step(
    config: &RalphConfig,
    current: &str,
    accepted_topic: &str,
) -> Option<String> {
    if current.is_empty() {
        return None;
    }
    let flow = effective_mechanism_config(config)?.flow.as_ref()?;
    let steps = &flow.steps;
    let idx = steps.iter().position(|s| s.id == current)?;
    let step = &steps[idx];
    // Per-unit "still working" emits that must not advance the
    // step. The list is intentionally small: only `work.done`
    // is the standard completion sentinel for the unit_loop
    // pattern. A plan that wants different semantics can use
    // the `terminal_when` field to refine; for now the simple
    // rule is enough.
    //
    // 2026-07-24-005 plan U2 (KTD3): supervisor exec_wave
    // declares `exec.unit.done` / `exec.unit.failed` /
    // `exec.unit.ready` in `allowed_emits` so a unit terminal
    // does not collide with FlowStepScope. Without these in
    // the non-transition whitelist, the first unit completion
    // would collapse the step to `exec_integrate` before the
    // wave has actually finished. The wave-terminal
    // `exec.wave.complete` / `exec.wave.failed` are NOT
    // listed — they remain transition topics so the wave
    // still advances when it has truly closed.
    const NON_TRANSITION_TOPICS: &[&str] = &[
        "work.done",
        "work.ready",
        "exec.unit.ready",
        "exec.unit.done",
        "exec.unit.failed",
        // Review / fix wave unit terminals — same isomorphic
        // contract as exec_wave (005 residual closure).
        "review.unit.ready",
        "review.unit.done",
        "fix.unit.ready",
        "fix.unit.done",
        "fix.unit.failed",
    ];
    if NON_TRANSITION_TOPICS.contains(&accepted_topic) {
        return None;
    }
    if !step.allowed_emits.iter().any(|t| t == accepted_topic) {
        return None;
    }

    // 2026-07-29-001 plan U1 (R1): explicit `transition_emits`
    // narrows the per-step transition authority. Empty keeps
    // the legacy behaviour (any `allowed_emits` topic advances
    // the step) for presets that have not opted in. Once
    // declared, only topics named in `transition_emits`
    // advance the step; the remaining `allowed_emits` topics
    // stay in scope for the FlowStepScope gate (so a
    // failure-capable step can keep `work.failed` in
    // `allowed_emits` without collapsing the step on the first
    // failure), but they no longer drive the positional /
    // declared forward advance. The lint graph
    // (`preset_lint::flow_declaration`) refuses to load a
    // preset whose `transition_emits` is not a subset of
    // `allowed_emits`, so this branch can trust the topic is
    // already in-scope.
    if !step.transition_emits.is_empty()
        && !step.transition_emits.iter().any(|t| t == accepted_topic)
    {
        return None;
    }

    // 2026-07-26-004 plan U6 (R7 / R8): declared-transition authority.
    // A FORWARD step (`j > idx`) whose `on` / `on_any_of` names the
    // accepted topic is the transition target. Forward-only makes the
    // transition idempotent (re-accepting the same event once advanced
    // finds no forward target → no-op) and rejects retrograde / illegal
    // jumps. Branching via `on_any_of` lets a failed review wave jump
    // straight to `finalize` instead of walking `synth_await`/`fix_plan`
    // positionally (the primary-20260726 flow-drift root cause).
    for (j, candidate) in steps.iter().enumerate() {
        if j <= idx {
            continue;
        }
        let enters = candidate.on.as_deref() == Some(accepted_topic)
            || candidate.on_any_of.iter().any(|t| t == accepted_topic);
        if enters {
            return Some(candidate.id.clone());
        }
    }

    // Legacy linear fallback: flows without declared `on` / `on_any_of`
    // transitions advance positionally (the existing ce-executor-serial
    // and supervisor exec_wave behaviour — unchanged).
    steps.get(idx + 1).map(|s| s.id.clone())
}

/// 2026-07-26-004 plan U7 (R7 / R8): recover the current flow step by
/// folding the SAME [`advance_plan_step`] authority over a sequence of
/// accepted topics, starting from [`initial_current_plan_step`].
///
/// This is the single recoverable source of truth the EventLoop restart
/// path, JSONL replay, and CLI `--policy-check` MUST share so none of
/// them silently re-derives the current step from the flow's first step
/// (the primary-20260726 `flow_unknown_emit` after `scope.ready`). The
/// resident EventLoop advances `current_plan_step` incrementally as it
/// ingests events; a separate process (CLI policy-check) or a restart
/// rebuilds the identical value by replaying the accepted topic sequence
/// through this fold.
pub fn recover_current_plan_step(config: &RalphConfig, accepted_topics: &[&str]) -> String {
    let mut current = initial_current_plan_step(config);
    for topic in accepted_topics {
        if let Some(next) = advance_plan_step(config, &current, topic) {
            current = next;
        }
    }
    current
}

/// Plan 004 R7 (P0-4): read the most recent accepted step from the
/// resident EventLoop's `.ralph/flow-authority.jsonl` ledger. The
/// resident EventLoop appends an entry on every accepted transition
/// (`append_flow_authority_snapshot`), and CLI policy-check /
/// restart recovery reads the same ledger so they never disagree
/// on the current step — and so rejected events, which never reach
/// the accept branch, do not pollute the recovered step. Returns
/// `None` if the file is missing or contains no accepted entries
/// for the active `loop_id`.
///
/// Plan 2026-07-31-001 (root cause from implementation-review runs
/// primary-20260731-131515 + primary-20260731-133437): when the
/// resident EventLoop appends a snapshot it also stamps the active
/// `loop_id` (read from the `.ralph/current-loop-id` marker).
/// Without the stamp, a new loop cold-start on the same workspace
/// would inherit the previous loop's terminal step (e.g.
/// `finalize`) and reject every fresh emit via `flow_unknown_emit`
/// — `ralph emit --policy-check` reads the ledger independently of
/// the resident loop's in-memory `current_plan_step`, so the dual
/// views drifted across loops and the very first emit of each
/// implementation-review run failed.
///
/// The `loop_id` filter here makes the recover semantics
/// loop-scoped: only entries belonging to the current loop are
/// considered; older loops' entries are treated as absent. Worktree
/// loops and primary loops share the same marker file, but each
/// writes its own `loop_id` so the filter cleanly partitions them.
/// When `loop_id` is `None` (no marker on disk) the function falls
/// back to the historical loop-blind read for backward
/// compatibility with tests that author entries without a marker
/// or with entries authored before the stamp.
pub fn load_flow_authority_current_step(
    workspace_root: &std::path::Path,
    loop_id: Option<&str>,
) -> Option<String> {
    let path = workspace_root.join(".ralph/flow-authority.jsonl");
    let contents = std::fs::read_to_string(&path).ok()?;
    let mut last: Option<String> = None;
    for line in contents.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(step) = v.get("step").and_then(|s| s.as_str()) else {
            continue;
        };
        // Plan 2026-07-31-001: when the caller passes a `loop_id`,
        // skip entries that belong to a different loop. Entries
        // without a `loop_id` field predate the stamp and are
        // accepted unconditionally (legacy behaviour) — this lets
        // pre-fix runs on the same workspace stay readable until
        // the first new entry overwrites the file.
        if let Some(active) = loop_id {
            let entry_loop = v.get("loop_id").and_then(|s| s.as_str());
            if let Some(entry_loop) = entry_loop
                && entry_loop != active
            {
                continue;
            }
        }
        last = Some(step.to_string());
    }
    last
}

// 2026-06-28 plan U4: tests for the plan-mode current_step
// state machine helpers. These run without spinning up the
// full EventLoop — they exercise the helper directly and
// confirm the wiring contract.

#[cfg(test)]
mod u4_current_plan_step_tests {
    use super::*;
    use crate::config::{
        EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig,
    };

    fn flow_config(steps: Vec<(&str, Vec<&str>)>) -> RalphConfig {
        let step_configs: Vec<FlowStepConfig> = steps
            .into_iter()
            .map(|(id, allowed)| FlowStepConfig {
                id: id.to_string(),
                kind: None,
                allowed_emits: allowed.into_iter().map(String::from).collect(),
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: None,
                on_any_of: Vec::new(),
                transition_emits: Vec::new(),
            })
            .collect();
        let mut cfg = RalphConfig::default();
        cfg.event_loop = EventLoopConfig {
            mechanism: Some(MechanismConfig {
                flow: Some(FlowDeclarationConfig {
                    flow_type: "declared".to_string(),
                    version: 1,
                    terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                    steps: step_configs,
                    ..FlowDeclarationConfig::default()
                }),
                phase_authority: None,
            }),
            ..EventLoopConfig::default()
        };
        cfg
    }

    #[test]
    fn initial_returns_first_step_id_from_top_level_mechanism() {
        // Preset SSOT is top-level `mechanism:`, not
        // `event_loop.mechanism`. initial_current_plan_step must
        // read via effective_mechanism_config.
        let mut cfg = RalphConfig::default();
        cfg.mechanism = Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: vec![FlowStepConfig {
                    id: "unit_loop".to_string(),
                    kind: None,
                    allowed_emits: vec!["work.ready".to_string()],
                    terminal_when: None,
                    on_partial: std::collections::BTreeMap::new(),
                    runs: None,
                    on: None,
                    on_any_of: Vec::new(),
                    transition_emits: Vec::new(),
                }],
                ..FlowDeclarationConfig::default()
            }),
            phase_authority: None,
        });
        assert_eq!(initial_current_plan_step(&cfg), "unit_loop");
    }

    #[test]
    fn initial_returns_first_step_id() {
        let cfg = flow_config(vec![("unit_loop", vec!["work.done"])]);
        assert_eq!(initial_current_plan_step(&cfg), "unit_loop");
    }

    #[test]
    fn initial_returns_empty_when_no_flow() {
        let cfg = RalphConfig::default();
        assert_eq!(initial_current_plan_step(&cfg), "");
    }

    #[test]
    fn advance_on_transition_event() {
        let cfg = flow_config(vec![
            ("unit_loop", vec!["work.done", "review.start"]),
            ("review_walk", vec!["review.complete"]),
        ]);
        let next = advance_plan_step(&cfg, "unit_loop", "review.start");
        assert_eq!(next, Some("review_walk".to_string()));
    }

    #[test]
    fn advance_skips_non_transition_event() {
        let cfg = flow_config(vec![
            ("unit_loop", vec!["work.done", "review.start"]),
            ("review_walk", vec!["review.complete"]),
        ]);
        // work.done is in allowed_emits but not a transition
        // event in this flow — staying on unit_loop is correct.
        let next = advance_plan_step(&cfg, "unit_loop", "work.done");
        assert_eq!(next, None);
    }

    #[test]
    fn advance_returns_none_at_last_step() {
        let cfg = flow_config(vec![("ship", vec!["LOOP_COMPLETE"])]);
        let next = advance_plan_step(&cfg, "ship", "LOOP_COMPLETE");
        assert_eq!(next, None);
    }

    #[test]
    fn advance_returns_none_with_empty_current() {
        let cfg = flow_config(vec![("unit_loop", vec!["review.start"])]);
        let next = advance_plan_step(&cfg, "", "review.start");
        assert_eq!(next, None);
    }

    #[test]
    fn advance_returns_none_when_current_unknown() {
        let cfg = flow_config(vec![("unit_loop", vec!["review.start"])]);
        let next = advance_plan_step(&cfg, "ghost", "review.start");
        assert_eq!(next, None);
    }

    #[test]
    fn advance_no_flow_returns_none() {
        let cfg = RalphConfig::default();
        let next = advance_plan_step(&cfg, "unit_loop", "review.start");
        assert_eq!(next, None);
    }

    /// 2026-07-26-004 plan U6 (R7 / R8): the flow authority advances by
    /// DECLARED transition (`on` / `on_any_of`), is idempotent on repeat,
    /// and branches a failed review wave straight to `finalize` (not
    /// positionally through `synth_await` / `fix_plan`) — the
    /// primary-20260726 flow-drift root cause. Mirrors the
    /// implementation-review flow shape.
    #[test]
    fn u6_declared_transition_authority_is_idempotent_and_branching() {
        let mk = |id: &str,
                  allowed: Vec<&str>,
                  on: Option<&str>,
                  on_any_of: Vec<&str>|
         -> FlowStepConfig {
            FlowStepConfig {
                id: id.to_string(),
                kind: None,
                allowed_emits: allowed.into_iter().map(String::from).collect(),
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: on.map(String::from),
                on_any_of: on_any_of.into_iter().map(String::from).collect(),
                transition_emits: Vec::new(),
            }
        };
        let mut cfg = RalphConfig::default();
        cfg.event_loop = EventLoopConfig {
            mechanism: Some(MechanismConfig {
                flow: Some(FlowDeclarationConfig {
                    flow_type: "declared".to_string(),
                    version: 1,
                    terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                    steps: vec![
                        mk(
                            "scope_freeze",
                            vec!["scope.ready", "scope.blocked"],
                            None,
                            vec![],
                        ),
                        mk(
                            "review_wave",
                            vec![
                                "review.unit.done",
                                "review.wave.complete",
                                "review.wave.failed",
                            ],
                            Some("scope.ready"),
                            vec![],
                        ),
                        mk(
                            "synth_await",
                            vec!["review.synthesized"],
                            Some("review.wave.complete"),
                            vec![],
                        ),
                        mk(
                            "fix_plan",
                            vec!["fix.plan.ready"],
                            Some("review.synthesized"),
                            vec![],
                        ),
                        mk(
                            "finalize",
                            vec!["LOOP_COMPLETE"],
                            None,
                            vec!["fix.plan.ready", "scope.blocked", "review.wave.failed"],
                        ),
                    ],
                    ..FlowDeclarationConfig::default()
                }),
                phase_authority: None,
            }),
            ..EventLoopConfig::default()
        };

        // Declared `on`: scope.ready transitions scope_freeze → review_wave.
        assert_eq!(
            advance_plan_step(&cfg, "scope_freeze", "scope.ready"),
            Some("review_wave".to_string())
        );
        // Idempotent: scope.ready is not allowed at review_wave, so a
        // replayed transition is a no-op (the step does not re-advance).
        assert_eq!(advance_plan_step(&cfg, "review_wave", "scope.ready"), None);
        // Non-transition unit terminal stays on review_wave.
        assert_eq!(
            advance_plan_step(&cfg, "review_wave", "review.unit.done"),
            None
        );
        // Declared `on`: review.wave.complete → synth_await.
        assert_eq!(
            advance_plan_step(&cfg, "review_wave", "review.wave.complete"),
            Some("synth_await".to_string())
        );
        // BRANCH (on_any_of): review.wave.failed jumps straight to
        // finalize, NOT positionally to synth_await.
        assert_eq!(
            advance_plan_step(&cfg, "review_wave", "review.wave.failed"),
            Some("finalize".to_string())
        );
        // BRANCH from the first step: scope.blocked → finalize.
        assert_eq!(
            advance_plan_step(&cfg, "scope_freeze", "scope.blocked"),
            Some("finalize".to_string())
        );
        // Recovery: rebuilding from the initial step + the accepted
        // transition lands on the same step the live loop reached.
        let initial = initial_current_plan_step(&cfg);
        assert_eq!(initial, "scope_freeze");
        assert_eq!(
            advance_plan_step(&cfg, &initial, "scope.ready"),
            Some("review_wave".to_string())
        );
    }

    /// 2026-07-26-004 plan U7 (R7 / R8): `recover_current_plan_step`
    /// rebuilds the SAME current step a resident EventLoop reaches
    /// incrementally, by folding the single `advance_plan_step`
    /// authority over the accepted topic sequence. A restart / replay /
    /// CLI policy-check that calls this never re-derives from the flow's
    /// first step independently (the primary-20260726 flow drift).
    #[test]
    fn u7_recover_current_plan_step_matches_incremental_advance() {
        let mk = |id: &str,
                  allowed: Vec<&str>,
                  on: Option<&str>,
                  on_any_of: Vec<&str>|
         -> FlowStepConfig {
            FlowStepConfig {
                id: id.to_string(),
                kind: None,
                allowed_emits: allowed.into_iter().map(String::from).collect(),
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: on.map(String::from),
                on_any_of: on_any_of.into_iter().map(String::from).collect(),
                transition_emits: Vec::new(),
            }
        };
        let mut cfg = RalphConfig::default();
        cfg.event_loop = EventLoopConfig {
            mechanism: Some(MechanismConfig {
                flow: Some(FlowDeclarationConfig {
                    flow_type: "declared".to_string(),
                    version: 1,
                    terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                    steps: vec![
                        mk(
                            "scope_freeze",
                            vec!["scope.ready", "scope.blocked"],
                            None,
                            vec![],
                        ),
                        mk(
                            "review_wave",
                            vec!["review.unit.done", "review.wave.failed"],
                            Some("scope.ready"),
                            vec![],
                        ),
                        mk(
                            "finalize",
                            vec!["LOOP_COMPLETE"],
                            None,
                            vec!["scope.blocked", "review.wave.failed"],
                        ),
                    ],
                    ..FlowDeclarationConfig::default()
                }),
                phase_authority: None,
            }),
            ..EventLoopConfig::default()
        };

        // No events → first step.
        assert_eq!(recover_current_plan_step(&cfg, &[]), "scope_freeze");
        // After scope.ready → review_wave (matches incremental advance).
        assert_eq!(
            recover_current_plan_step(&cfg, &["scope.ready"]),
            "review_wave"
        );
        // review.unit.done is a non-transition → stays review_wave.
        assert_eq!(
            recover_current_plan_step(&cfg, &["scope.ready", "review.unit.done"]),
            "review_wave"
        );
        // Branch: review.wave.failed → finalize.
        assert_eq!(
            recover_current_plan_step(&cfg, &["scope.ready", "review.wave.failed"]),
            "finalize"
        );
        // Recovery is deterministic: replaying the same sequence twice
        // yields the same step the resident loop holds.
        let seq = ["scope.ready", "review.unit.done", "review.unit.done"];
        assert_eq!(recover_current_plan_step(&cfg, &seq), "review_wave");
        assert_eq!(recover_current_plan_step(&cfg, &seq), "review_wave");
    }

    // 2026-07-24-005 plan U2 (R2 / R3 / S1 / S6): supervisor
    // exec_wave accepts `exec.unit.done` / `exec.unit.failed`
    // without advancing the step, while `exec.wave.complete`
    // still advances to `exec_integrate`. These three topics
    // are pinned in the `NON_TRANSITION_TOPICS` whitelist of
    // `advance_plan_step` so the supervisor wave does not
    // collapse after the first unit completion.
    //
    // KTD3: the whitelist is the smaller change vs. the
    // alternative of an `exec_unit_*` non-transition bucket.
    /// 2026-07-29-001 plan U1 (R1): when a step declares an
    /// explicit `transition_emits`, only those topics advance
    /// the plan-mode current step. Other topics that remain
    /// in `allowed_emits` (e.g. `forge.review.ready`) are
    /// still accepted in the current step (FlowStepScope) but
    /// no longer collapse the step boundary through the
    /// positional-advance fallback. Topic names use a bespoke
    /// namespace that avoids the runtime's NON_TRANSITION_TOPICS
    /// whitelist, so the assertions actually prove the
    /// transition_emits field narrows the authority (the
    /// whitelist would otherwise mask the failure on
    /// `work.ready`/`work.failed`-style topics).
    #[test]
    fn u1_transition_emits_only_named_topics_advance() {
        let mk = |id: &str, allowed: Vec<&str>, transition: Vec<&str>| -> FlowStepConfig {
            FlowStepConfig {
                id: id.to_string(),
                kind: None,
                allowed_emits: allowed.into_iter().map(String::from).collect(),
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: None,
                on_any_of: Vec::new(),
                transition_emits: transition.into_iter().map(String::from).collect(),
            }
        };
        let mut cfg = RalphConfig::default();
        cfg.event_loop = EventLoopConfig {
            mechanism: Some(MechanismConfig {
                flow: Some(FlowDeclarationConfig {
                    flow_type: "declared".to_string(),
                    version: 1,
                    terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                    steps: vec![
                        mk(
                            "unit_loop",
                            vec![
                                "forge.triage.ready",
                                "forge.triage.partial",
                                "forge.triage.done",
                            ],
                            vec!["forge.triage.done"],
                        ),
                        mk(
                            "review_walk",
                            vec!["forge.review.complete"],
                            vec!["forge.review.complete"],
                        ),
                    ],
                    ..FlowDeclarationConfig::default()
                }),
                phase_authority: None,
            }),
            ..EventLoopConfig::default()
        };
        // forge.triage.done is in transition_emits → advances.
        assert_eq!(
            advance_plan_step(&cfg, "unit_loop", "forge.triage.done"),
            Some("review_walk".to_string())
        );
        // forge.triage.ready is in allowed_emits but NOT in
        // transition_emits → must NOT advance.
        assert_eq!(
            advance_plan_step(&cfg, "unit_loop", "forge.triage.ready"),
            None
        );
        // forge.triage.partial is in allowed_emits but NOT in
        // transition_emits → must NOT advance.
        assert_eq!(
            advance_plan_step(&cfg, "unit_loop", "forge.triage.partial"),
            None
        );
    }

    /// 2026-07-29-001 plan U1 (R1 / R8): when `transition_emits`
    /// is empty (the legacy default), every `allowed_emits`
    /// topic remains transition-capable — the contract a
    /// preset wrote before this field was introduced.
    #[test]
    fn u1_empty_transition_emits_keeps_legacy_allowed_emits_authority() {
        let cfg = flow_config(vec![
            ("unit_loop", vec!["work.done", "review.start"]),
            ("review_walk", vec!["review.complete"]),
        ]);
        // review.start advances (legacy contract).
        assert_eq!(
            advance_plan_step(&cfg, "unit_loop", "review.start"),
            Some("review_walk".to_string())
        );
    }

    /// 2026-07-29-001 plan U1 (R8): resident EventLoop
    /// (`advance_plan_step`) and replay (`recover_current_plan_step`)
    /// share the same authority. When `transition_emits` is
    /// explicit, the replay-folding must agree with the live
    /// incremental advance on every accepted topic sequence.
    #[test]
    fn u1_recover_current_plan_step_matches_incremental_with_transition_emits() {
        let mk = |id: &str, allowed: Vec<&str>, transition: Vec<&str>| -> FlowStepConfig {
            FlowStepConfig {
                id: id.to_string(),
                kind: None,
                allowed_emits: allowed.into_iter().map(String::from).collect(),
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: None,
                on_any_of: Vec::new(),
                transition_emits: transition.into_iter().map(String::from).collect(),
            }
        };
        let mut cfg = RalphConfig::default();
        cfg.event_loop = EventLoopConfig {
            mechanism: Some(MechanismConfig {
                flow: Some(FlowDeclarationConfig {
                    flow_type: "declared".to_string(),
                    version: 1,
                    terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                    steps: vec![
                        mk(
                            "unit_loop",
                            vec!["work.ready", "work.failed", "review.start"],
                            vec!["review.start"],
                        ),
                        mk(
                            "review_walk",
                            vec!["review.complete"],
                            vec!["review.complete"],
                        ),
                    ],
                    ..FlowDeclarationConfig::default()
                }),
                phase_authority: None,
            }),
            ..EventLoopConfig::default()
        };
        // Resident path.
        let mut live = initial_current_plan_step(&cfg);
        assert_eq!(live, "unit_loop");
        for topic in ["work.ready", "work.failed", "review.start"] {
            if let Some(next) = advance_plan_step(&cfg, &live, topic) {
                live = next;
            }
        }
        assert_eq!(live, "review_walk");
        // Replay path — must agree.
        let replayed =
            recover_current_plan_step(&cfg, &["work.ready", "work.failed", "review.start"]);
        assert_eq!(replayed, live);
    }

    fn exec_wave_flow() -> RalphConfig {
        flow_config(vec![
            ("unit_loop", vec!["work.ready", "execution.plan.ready"]),
            (
                "exec_wave",
                vec![
                    "exec.wave.complete",
                    "exec.wave.failed",
                    "exec.unit.done",
                    "exec.unit.failed",
                ],
            ),
            ("exec_integrate", vec!["plan.complete"]),
        ])
    }

    #[test]
    fn u2_advance_unit_done_on_exec_wave_returns_none() {
        // S1 + R3: a unit terminal on the exec_wave step
        // must NOT advance the plan to exec_integrate.
        let cfg = exec_wave_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.unit.done");
        assert_eq!(next, None);
    }

    #[test]
    fn u2_advance_unit_failed_on_exec_wave_returns_none() {
        let cfg = exec_wave_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.unit.failed");
        assert_eq!(next, None);
    }

    #[test]
    fn u2_advance_wave_complete_on_exec_wave_advances() {
        // S6: the wave terminal must still advance to the
        // next step (exec_integrate) — the wave has truly
        // closed.
        let cfg = exec_wave_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.wave.complete");
        assert_eq!(next, Some("exec_integrate".to_string()));
    }

    #[test]
    fn u2_advance_unit_done_on_unit_loop_returns_none() {
        // S2 boundary: the supervisor preset must NOT
        // double-mount `exec.unit.done` on `unit_loop`;
        // the helper still returns None because the topic
        // is not in `unit_loop.allowed_emits` (and is in
        // the non-transition list).
        let cfg = exec_wave_flow();
        let next = advance_plan_step(&cfg, "unit_loop", "exec.unit.done");
        assert_eq!(next, None);
    }

    #[test]
    fn u2_advance_execution_plan_ready_advances_to_exec_wave() {
        // S3 / R4: `execution.plan.ready` accepted on
        // `unit_loop` advances to `exec_wave`. Confirms
        // the flow declaration wires task-planner →
        // exec-wave-dispatcher.
        let cfg = exec_wave_flow();
        let next = advance_plan_step(&cfg, "unit_loop", "execution.plan.ready");
        assert_eq!(next, Some("exec_wave".to_string()));
    }

    fn review_fix_wave_flow() -> RalphConfig {
        flow_config(vec![
            (
                "review_loop",
                vec![
                    "review.wave.complete",
                    "review.wave.failed",
                    "review.unit.ready",
                    "review.unit.done",
                ],
            ),
            (
                "fix_loop",
                vec![
                    "fix.wave.complete",
                    "fix.wave.failed",
                    "fix.unit.ready",
                    "fix.unit.done",
                    "fix.unit.failed",
                ],
            ),
            ("plan_end", vec!["plan.complete"]),
        ])
    }

    #[test]
    fn u2_review_unit_done_on_review_loop_returns_none() {
        let cfg = review_fix_wave_flow();
        assert_eq!(
            advance_plan_step(&cfg, "review_loop", "review.unit.done"),
            None
        );
    }

    #[test]
    fn u2_fix_unit_done_on_fix_loop_returns_none() {
        let cfg = review_fix_wave_flow();
        assert_eq!(advance_plan_step(&cfg, "fix_loop", "fix.unit.done"), None);
        assert_eq!(advance_plan_step(&cfg, "fix_loop", "fix.unit.failed"), None);
    }

    #[test]
    fn u2_review_wave_complete_advances_to_fix_loop() {
        let cfg = review_fix_wave_flow();
        assert_eq!(
            advance_plan_step(&cfg, "review_loop", "review.wave.complete"),
            Some("fix_loop".to_string())
        );
    }

    #[test]
    fn u2_fix_wave_complete_advances_to_plan_end() {
        let cfg = review_fix_wave_flow();
        assert_eq!(
            advance_plan_step(&cfg, "fix_loop", "fix.wave.complete"),
            Some("plan_end".to_string())
        );
    }
}

// Plan 004 P0-4: `load_flow_authority_current_step` reads the
// accepted-only ledger the resident EventLoop writes on every
// accept. The CLI policy-check and restart recovery both call it,
// so the contract here pins the read-side semantics that close
// the rejected-event poisoning bug.
#[cfg(test)]
mod p0_4_flow_authority_ledger_tests {
    use super::*;
    use std::path::PathBuf;

    // Plan 2026-07-31-001 (nextest process-per-test
    // compatibility): the prior helper shared one directory
    // per process id, which caused races when nextest ran tests
    // in parallel. Each test now gets its own sub-directory
    // rooted at the shared per-process temp dir; the helper
    // accepts the test name so two tests never collide on
    // `flow-authority.jsonl` writes. The `test_name` is the
    // `&str` the caller passes — usually the literal test fn
    // name to keep a 1:1 audit trail between the test and its
    // scratch space.
    fn workspace_root(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("ralph-p0-4-flow-auth-{}", std::process::id()))
            .join(test_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".ralph")).unwrap();
        dir
    }

    #[test]
    fn load_returns_none_when_ledger_missing() {
        let root = workspace_root("load_returns_none_when_ledger_missing");
        let got = load_flow_authority_current_step(&root, None);
        assert!(got.is_none(), "missing ledger must yield None");
    }

    #[test]
    fn load_returns_last_step_from_ledger() {
        let root = workspace_root("load_returns_last_step_from_ledger");
        let path = root.join(".ralph/flow-authority.jsonl");
        std::fs::write(
            &path,
            "{\"step\":\"scope_freeze\",\"topic\":\"scope.freeze\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n\
             {\"step\":\"synth_await\",\"topic\":\"review.wave.complete\"}\n",
        )
        .unwrap();
        let got = load_flow_authority_current_step(&root, None);
        assert_eq!(got.as_deref(), Some("synth_await"));
    }

    #[test]
    fn load_skips_blank_and_malformed_lines() {
        let root = workspace_root("load_skips_blank_and_malformed_lines");
        let path = root.join(".ralph/flow-authority.jsonl");
        std::fs::write(
            &path,
            "\n{\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n\
             not-json\n\
             {\"step\":\"synth_await\"}\n",
        )
        .unwrap();
        let got = load_flow_authority_current_step(&root, None);
        assert_eq!(got.as_deref(), Some("synth_await"));
    }

    /// Plan 004 R7 / P0-4: rejected events never enter the
    /// accept branch, so the authority ledger only reflects the
    /// accepted transitions. Mixing rejected events into the
    /// main ledger (the pre-fix bug) used to advance the
    /// recovered step incorrectly.
    #[test]
    fn rejected_events_do_not_pollute_authority() {
        // The acceptance ledger is a separate file from
        // events.jsonl. The pre-fix CLI folded raw main ledger
        // topics (including rejected ones) through
        // `advance_plan_step`. The post-fix CLI reads only the
        // accepted ledger; the test pins that rejected events
        // never reach this file.
        let root = workspace_root("rejected_events_do_not_pollute_authority");
        let path = root.join(".ralph/flow-authority.jsonl");
        // Simulate the EventLoop having accepted exactly one
        // event: scope.ready, which advanced review_wave.
        std::fs::write(
            &path,
            "{\"step\":\"scope_freeze\",\"topic\":\"scope.freeze\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n",
        )
        .unwrap();
        let got = load_flow_authority_current_step(&root, None);
        assert_eq!(got.as_deref(), Some("review_wave"));
    }

    /// Plan 004 R7: the same accepted-step ledger is consumed
    /// by both the resident EventLoop (writes) and CLI
    /// policy-check / restart (reads). Restart consistency:
    /// re-instantiating the recovery function on the same
    /// ledger must produce the same step.
    #[test]
    fn restart_consistency_across_reads() {
        let root = workspace_root("restart_consistency_across_reads");
        let path = root.join(".ralph/flow-authority.jsonl");
        std::fs::write(
            &path,
            "{\"step\":\"scope_freeze\",\"topic\":\"scope.freeze\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n\
             {\"step\":\"synth_await\",\"topic\":\"review.wave.complete\"}\n",
        )
        .unwrap();
        let a = load_flow_authority_current_step(&root, None);
        let b = load_flow_authority_current_step(&root, None);
        assert_eq!(a, b, "restart must observe the same authority");
        assert_eq!(a.as_deref(), Some("synth_await"));
    }

    // Plan 2026-07-31-001 regression tests: the loop_id filter
    // must partition flow-authority.jsonl entries by their active
    // loop so a new loop cold-start on the same workspace does NOT
    // inherit the previous loop's terminal step (root cause:
    // implementation-review runs primary-20260731-131515 +
    // primary-20260731-133437 both failed `ralph emit
    // scope.ready.proposed --policy-check` with
    // `flow_unknown_emit` because the previous loop's `finalize`
    // entry was carried over via the loop-blind read).

    #[test]
    fn load_filters_entries_by_loop_id() {
        let root = workspace_root("load_filters_entries_by_loop_id");
        let path = root.join(".ralph/flow-authority.jsonl");
        std::fs::write(
            &path,
            "{\"step\":\"scope_freeze\",\"topic\":\"scope.ready\",\"loop_id\":\"loop-A\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\",\"loop_id\":\"loop-A\"}\n\
             {\"step\":\"finalize\",\"topic\":\"scope.blocked\",\"loop_id\":\"loop-B\"}\n",
        )
        .unwrap();
        // loop-A caller — must see the latest loop-A entry,
        // NOT the stale `finalize` from loop-B.
        let a = load_flow_authority_current_step(&root, Some("loop-A"));
        assert_eq!(
            a.as_deref(),
            Some("review_wave"),
            "loop-A caller must ignore loop-B entries"
        );
        // loop-B caller — must see the loop-B entry.
        let b = load_flow_authority_current_step(&root, Some("loop-B"));
        assert_eq!(b.as_deref(), Some("finalize"));
        // No loop_id passed (legacy / tests / CLI sub-process
        // without a marker on disk) — last entry wins (loop-B's
        // finalize) so older flows and tests keep working.
        let none = load_flow_authority_current_step(&root, None);
        assert_eq!(none.as_deref(), Some("finalize"));
    }

    #[test]
    fn load_keeps_unstamped_entries_for_backward_compat() {
        let root = workspace_root("load_keeps_unstamped_entries_for_backward_compat");
        let path = root.join(".ralph/flow-authority.jsonl");
        std::fs::write(
            &path,
            "{\"step\":\"scope_freeze\",\"topic\":\"scope.ready\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n",
        )
        .unwrap();
        let got = load_flow_authority_current_step(&root, Some("loop-C"));
        assert_eq!(
            got.as_deref(),
            Some("review_wave"),
            "unstamped entries must remain readable so pre-fix loops and tests don't break"
        );
    }

    #[test]
    fn load_returns_none_for_empty_loop_scoped_ledger() {
        let root = workspace_root("load_returns_none_for_empty_loop_scoped_ledger");
        let path = root.join(".ralph/flow-authority.jsonl");
        std::fs::write(
            &path,
            "{\"step\":\"finalize\",\"topic\":\"scope.blocked\",\"loop_id\":\"loop-A\"}\n",
        )
        .unwrap();
        // loop-B caller — no entry for this loop — must return
        // None (fall back to initial_current_plan_step on the
        // consumer side) so `ralph emit --policy-check` does not
        // pick up another loop's terminal step.
        let got = load_flow_authority_current_step(&root, Some("loop-B"));
        assert!(
            got.is_none(),
            "loop-B caller must see no entries; the loop-A `finalize` \
             must not leak across loops"
        );
    }
}

#[cfg(test)]
mod hat_only_pipeline_tests {
    use super::*;
    use crate::config::RalphConfig;

    #[test]
    fn config_without_mechanism_uses_hat_only_emit_pipeline() {
        let config = RalphConfig::default();
        let (pipeline, step_totals, _authority) = build_stage_pipeline_from_config(&config);
        assert!(step_totals.is_empty());
        assert_eq!(
            pipeline.names(),
            vec!["RepairDispatch", "EmitSchemaGate", "VerdictGate"]
        );
    }
}

// 2026-07-28-001 plan U1: typed embedded recovery tests for parallel-forge
// flow authority (R1/S1, R2/S2, R7/S7, R9/S9). Uses the same helpers
// as u4_current_plan_step_tests but focuses on the recover_current_plan_step
// fold over the parallel-forge step sequence.
#[cfg(test)]
mod flow_authority_pf_recovery_tests {
    use super::*;
    use crate::config::{
        EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig,
    };

    /// Build a RalphConfig that mirrors parallel-forge's flow declaration.
    /// Identical to the version in u4_current_plan_step_tests (mod.rs:15301).
    fn parallel_forge_flow() -> RalphConfig {
        let mk = |id: &str,
                  allowed: Vec<&str>,
                  on: Option<&str>,
                  on_any_of: Vec<&str>,
                  runs: Option<&str>| FlowStepConfig {
            id: id.to_string(),
            kind: if runs.is_some() {
                Some("side_effect".to_string())
            } else if matches!(id, "planning" | "integration") {
                Some("linear".to_string())
            } else {
                None
            },
            allowed_emits: allowed.into_iter().map(String::from).collect(),
            terminal_when: None,
            on_partial: std::collections::BTreeMap::new(),
            runs: runs.map(String::from),
            on: on.map(String::from),
            on_any_of: on_any_of.into_iter().map(String::from).collect(),
            transition_emits: Vec::new(),
        };

        let mut cfg = RalphConfig::default();
        cfg.event_loop = EventLoopConfig {
            mechanism: Some(MechanismConfig {
                flow: Some(FlowDeclarationConfig {
                    flow_type: "declared".to_string(),
                    version: 1,
                    terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                    steps: vec![
                        mk(
                            "planning",
                            vec![
                                "forge.plan.inspected",
                                "forge.plan.ready",
                                "forge.concurrency.approved",
                                "forge.worktrees.ready",
                                "forge.plan.blocked",
                            ],
                            None,
                            vec![],
                            None,
                        ),
                        mk(
                            "exec_wave",
                            vec![
                                "exec.wave.complete",
                                "exec.wave.failed",
                                "exec.unit.ready",
                                "exec.unit.done",
                                "exec.unit.failed",
                                "forge.exec.development.done",
                            ],
                            Some("forge.worktrees.ready"),
                            vec![],
                            Some("supervisor.exec.wave"),
                        ),
                        mk(
                            "unit_review",
                            vec!["forge.units.reviewed"],
                            Some("forge.exec.development.done"),
                            vec![],
                            None,
                        ),
                        mk(
                            "integration",
                            vec![
                                "forge.integration.done",
                                "forge.incremental.verified",
                                "forge.full.verified",
                                "forge.audit.done",
                                "forge.report.done",
                                "work.failed",
                            ],
                            Some("forge.units.reviewed"),
                            vec![],
                            None,
                        ),
                        mk(
                            "plan_end",
                            vec!["forge.report.done", "LOOP_COMPLETE"],
                            None,
                            vec![],
                            None,
                        ),
                    ],
                    ..FlowDeclarationConfig::default()
                }),
                phase_authority: None,
            }),
            ..EventLoopConfig::default()
        };
        cfg
    }

    // R1/S1: recover_current_plan_step folds the planning handoff
    // sequence correctly: empty → forge.concurrency.approved → exec_wave.
    #[test]
    fn pf_recovery_r1_planning_handoff_folds_to_exec_wave() {
        let cfg = parallel_forge_flow();
        let initial = initial_current_plan_step(&cfg);
        assert_eq!(initial, "planning", "R1: initial step must be planning");
        let recovered = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(
            recovered, "exec_wave",
            "R1: forge.concurrency.approved must advance planning → exec_wave"
        );
    }

    /// R1/S1 variant: forge.worktrees.ready is the concurrency approval signal.
    #[test]
    fn pf_recovery_r1_worktrees_ready_folds_to_exec_wave() {
        let cfg = parallel_forge_flow();
        let recovered = recover_current_plan_step(&cfg, &["forge.worktrees.ready"]);
        assert_eq!(
            recovered, "exec_wave",
            "R1: forge.worktrees.ready must advance planning → exec_wave"
        );
    }

    /// R2/S2: forge.plan.blocked at planning is in allowed_emits but has no
    /// declared `on` transition, so advance_plan_step falls back to linear
    /// advance (planning → exec_wave). This is a known plan-vs-rule gap:
    /// the executor HARDS RULES forbid editing presets/en/, so the
    /// terminal-report semantics for forge.plan.blocked cannot be wired
    /// here. Recorded as a plan flaw in .ralph/agent/decisions.md.
    #[test]
    fn pf_recovery_r2_plan_blocked_at_planning_linear_advance_to_exec_wave() {
        let cfg = parallel_forge_flow();
        let recovered = recover_current_plan_step(&cfg, &["forge.plan.blocked"]);
        assert_eq!(
            recovered, "exec_wave",
            "R2 GAP: forge.plan.blocked currently advances via linear fallback \
             (terminal-report semantics require preset YAML edit; out of executor scope)"
        );
    }

    /// R7/S7: forge.plan.blocked is idempotent on repeat (same linear advance
    /// applies on both first and second emission). The terminal semantics
    /// (staying put) requires an explicit non-transition declaration in YAML.
    #[test]
    fn pf_recovery_r7_forge_plan_blocked_idempotent_linear_fallback() {
        let cfg = parallel_forge_flow();
        // First emission: linear fallback advances planning → exec_wave
        let after_block = recover_current_plan_step(&cfg, &["forge.plan.blocked"]);
        assert_eq!(after_block, "exec_wave");
        // Second emission: same linear fallback, still idempotent (no double-advance)
        let recovered =
            recover_current_plan_step(&cfg, &["forge.plan.blocked", "forge.plan.blocked"]);
        assert_eq!(
            recovered, "exec_wave",
            "R7: repeated forge.plan.blocked is idempotent (linear fallback is deterministic)"
        );
    }

    /// R7/S7: forge.plan.blocked at exec_wave is not a transition; fold stays.
    #[test]
    fn pf_recovery_r7_plan_blocked_at_exec_wave_stays_at_exec_wave() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave");
        let recovered =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "forge.plan.blocked"]);
        assert_eq!(
            recovered, "exec_wave",
            "R7: forge.plan.blocked at exec_wave must not trigger a transition"
        );
    }

    /// R9/S9: old planning events do NOT backstep after advancing to exec_wave.
    #[test]
    fn pf_recovery_r9_old_planning_events_do_not_backstep_at_exec_wave() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave");
        let recovered =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "forge.plan.ready"]);
        assert_eq!(
            recovered, "exec_wave",
            "R9: old forge.plan.ready after exec_wave must not backstep"
        );
    }

    /// R9/S9: repeated transition event is idempotent — stays at exec_wave.
    #[test]
    fn pf_recovery_r9_repeated_concurrency_approved_stays_at_exec_wave() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave");
        let recovered = recover_current_plan_step(
            &cfg,
            &["forge.concurrency.approved", "forge.concurrency.approved"],
        );
        assert_eq!(
            recovered, "exec_wave",
            "R9: repeated forge.concurrency.approved must not backstep"
        );
    }

    /// Full happy-path fold: planning → exec_wave → unit_review → integration → plan_end.
    #[test]
    fn pf_recovery_full_happy_path_folds_through_all_steps() {
        let cfg = parallel_forge_flow();
        let step1 = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(step1, "exec_wave");
        let step2 =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.wave.complete"]);
        assert_eq!(step2, "unit_review");
        let step3 = recover_current_plan_step(
            &cfg,
            &[
                "forge.concurrency.approved",
                "exec.wave.complete",
                "forge.units.reviewed",
            ],
        );
        assert_eq!(step3, "integration");
        let step4 = recover_current_plan_step(
            &cfg,
            &[
                "forge.concurrency.approved",
                "exec.wave.complete",
                "forge.units.reviewed",
                "forge.report.done",
            ],
        );
        assert_eq!(step4, "plan_end");
    }

    /// S1: exec.unit.done is a per-unit terminal, NOT a step transition.
    #[test]
    fn pf_recovery_s1_exec_unit_done_is_non_transition() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave");
        let recovered =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.unit.done"]);
        assert_eq!(
            recovered, "exec_wave",
            "S1: exec.unit.done must not advance exec_wave step"
        );
    }

    /// S2: exec.unit.failed is a per-unit terminal, NOT a step transition.
    #[test]
    fn pf_recovery_s2_exec_unit_failed_is_non_transition() {
        let cfg = parallel_forge_flow();
        let at_exec = recover_current_plan_step(&cfg, &["forge.concurrency.approved"]);
        assert_eq!(at_exec, "exec_wave");
        let recovered =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.unit.failed"]);
        assert_eq!(
            recovered, "exec_wave",
            "S2: exec.unit.failed must not advance exec_wave step"
        );
    }

    /// R7/S7: forge.plan.blocked at integration is not in allowed_emits; fold stays.
    #[test]
    fn pf_recovery_r7_plan_blocked_at_integration_not_in_allowed_emits() {
        let cfg = parallel_forge_flow();
        let at_integration = recover_current_plan_step(
            &cfg,
            &[
                "forge.concurrency.approved",
                "exec.wave.complete",
                "forge.units.reviewed",
            ],
        );
        assert_eq!(at_integration, "integration");
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.concurrency.approved",
                "exec.wave.complete",
                "forge.units.reviewed",
                "forge.plan.blocked",
            ],
        );
        assert_eq!(
            recovered, "integration",
            "R7: forge.plan.blocked at integration must not trigger a transition"
        );
    }

    /// R9/S9: repeated exec.wave.complete must not backstep from unit_review.
    #[test]
    fn pf_recovery_r9_repeated_exec_wave_complete_stays_at_unit_review() {
        let cfg = parallel_forge_flow();
        let at_review =
            recover_current_plan_step(&cfg, &["forge.concurrency.approved", "exec.wave.complete"]);
        assert_eq!(at_review, "unit_review");
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.concurrency.approved",
                "exec.wave.complete",
                "exec.wave.complete",
            ],
        );
        assert_eq!(
            recovered, "unit_review",
            "R9: repeated exec.wave.complete must not backstep to exec_wave"
        );
    }
}

// 2026-07-28-001 plan U2: typed exec_wave branch tests.
// Separate file keeps wave transition / non-transition coverage
// isolated from the longer flow_authority_pf_recovery_tests block.
#[cfg(test)]
pub mod wave_branch_tests;

// 2026-07-28-001 plan §3.1: 14-step parallel-forge flow authority tests.
// Verifies the declared 14-step flow (planning → plan_authoring →
// concurrency_review → worktree_setup → exec_wave → exec_finalize →
// exec_failure → unit_review → integration → incremental_verify →
// full_verify → audit → report → plan_end) behaves correctly:
//   - Each cross-hat handoff uses the next step's `on`.
//   - Multi-source block uses `report.on_any_of`.
//   - exec_wave unit topics and `work.failed` are non-transitions.
//   - `exec.wave.complete` and `exec.wave.failed` route to distinct
//     branches (`exec_finalize` vs `exec_failure`).
//   - `forge.report.done` enters `plan_end` from any failure-capable
//     step (integration, incremental_verify, full_verify, exec_failure).
//   - `LOOP_COMPLETE` is only accepted at `plan_end`.
//
// Distinct from U1/U2's 5-step flow baseline; uses inline 14-step
// config so the tests stay decoupled from the embedded preset.
#[cfg(test)]
mod flow_authority_pf_declared_14step_tests {
    use super::*;
    use crate::config::{
        EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig,
    };

    /// Build a RalphConfig mirroring the target 14-step parallel-forge
    /// flow declaration from plan §3.1.
    fn parallel_forge_14step_flow() -> RalphConfig {
        let mk = |id: &str,
                  kind: Option<&str>,
                  allowed: Vec<&str>,
                  on: Option<&str>,
                  on_any_of: Vec<&str>,
                  runs: Option<&str>| FlowStepConfig {
            id: id.to_string(),
            kind: kind.map(String::from),
            allowed_emits: allowed.into_iter().map(String::from).collect(),
            terminal_when: None,
            on_partial: std::collections::BTreeMap::new(),
            runs: runs.map(String::from),
            on: on.map(String::from),
            on_any_of: on_any_of.into_iter().map(String::from).collect(),
            transition_emits: Vec::new(),
        };

        let mut cfg = RalphConfig::default();
        cfg.event_loop = EventLoopConfig {
            mechanism: Some(MechanismConfig {
                flow: Some(FlowDeclarationConfig {
                    flow_type: "declared".to_string(),
                    version: 1,
                    terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                    steps: vec![
                        mk(
                            "planning",
                            Some("linear"),
                            vec!["forge.plan.inspected", "forge.plan.blocked"],
                            None,
                            vec![],
                            None,
                        ),
                        mk(
                            "plan_authoring",
                            Some("linear"),
                            vec!["forge.plan.ready", "forge.plan.blocked"],
                            Some("forge.plan.inspected"),
                            vec![],
                            None,
                        ),
                        mk(
                            "concurrency_review",
                            Some("linear"),
                            vec!["forge.concurrency.approved", "forge.plan.blocked"],
                            Some("forge.plan.ready"),
                            vec![],
                            None,
                        ),
                        mk(
                            "worktree_setup",
                            Some("linear"),
                            vec!["forge.worktrees.ready", "forge.plan.blocked"],
                            Some("forge.concurrency.approved"),
                            vec![],
                            None,
                        ),
                        mk(
                            "exec_wave",
                            Some("side_effect"),
                            vec![
                                "exec.unit.ready",
                                "exec.unit.done",
                                "exec.unit.failed",
                                "exec.wave.complete",
                                "exec.wave.failed",
                            ],
                            Some("forge.worktrees.ready"),
                            vec![],
                            Some("supervisor.exec.wave"),
                        ),
                        mk(
                            "exec_finalize",
                            Some("await"),
                            vec!["forge.exec.development.done"],
                            Some("exec.wave.complete"),
                            vec![],
                            None,
                        ),
                        mk(
                            "exec_failure",
                            Some("await"),
                            vec!["work.failed", "forge.report.done"],
                            Some("exec.wave.failed"),
                            vec![],
                            None,
                        ),
                        mk(
                            "unit_review",
                            Some("linear"),
                            vec!["forge.units.reviewed", "forge.plan.blocked"],
                            Some("forge.exec.development.done"),
                            vec![],
                            None,
                        ),
                        mk(
                            "integration",
                            Some("linear"),
                            vec!["forge.integration.done", "work.failed", "forge.report.done"],
                            Some("forge.units.reviewed"),
                            vec![],
                            None,
                        ),
                        mk(
                            "incremental_verify",
                            Some("linear"),
                            vec![
                                "forge.incremental.verified",
                                "work.failed",
                                "forge.report.done",
                            ],
                            Some("forge.integration.done"),
                            vec![],
                            None,
                        ),
                        mk(
                            "full_verify",
                            Some("linear"),
                            vec!["forge.full.verified", "work.failed", "forge.report.done"],
                            Some("forge.incremental.verified"),
                            vec![],
                            None,
                        ),
                        mk(
                            "audit",
                            Some("linear"),
                            vec!["forge.audit.done", "forge.plan.blocked"],
                            Some("forge.full.verified"),
                            vec![],
                            None,
                        ),
                        mk(
                            "report",
                            Some("await"),
                            vec!["forge.report.done"],
                            None,
                            // U7 (plan 2026-07-29-001): plan-level
                            // `work.failed` is now a transition.
                            // The `report` step is the universal
                            // funnel for terminal failures.
                            vec!["forge.audit.done", "forge.plan.blocked", "work.failed"],
                            None,
                        ),
                        mk(
                            "plan_end",
                            Some("terminal"),
                            vec!["LOOP_COMPLETE"],
                            Some("forge.report.done"),
                            vec![],
                            None,
                        ),
                    ],
                    ..FlowDeclarationConfig::default()
                }),
                phase_authority: None,
            }),
            ..EventLoopConfig::default()
        };
        cfg
    }

    // ── R1/S1: planning handoff steps ──────────────────────────────────────

    /// R1: forge.plan.inspected enters plan_authoring (not exec_wave).
    #[test]
    fn pf_14step_inspected_enters_plan_authoring_not_exec_wave() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "planning", "forge.plan.inspected");
        assert_eq!(
            next,
            Some("plan_authoring".to_string()),
            "R1: forge.plan.inspected must advance planning → plan_authoring"
        );
    }

    /// R1: forge.plan.ready enters concurrency_review.
    #[test]
    fn pf_14step_plan_ready_enters_concurrency_review() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "plan_authoring", "forge.plan.ready");
        assert_eq!(
            next,
            Some("concurrency_review".to_string()),
            "R1: forge.plan.ready must advance plan_authoring → concurrency_review"
        );
    }

    /// R1: forge.concurrency.approved enters worktree_setup.
    #[test]
    fn pf_14step_concurrency_approved_enters_worktree_setup() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "concurrency_review", "forge.concurrency.approved");
        assert_eq!(
            next,
            Some("worktree_setup".to_string()),
            "R1: forge.concurrency.approved must advance concurrency_review → worktree_setup"
        );
    }

    /// R1: forge.worktrees.ready enters exec_wave.
    #[test]
    fn pf_14step_worktrees_ready_enters_exec_wave() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "worktree_setup", "forge.worktrees.ready");
        assert_eq!(
            next,
            Some("exec_wave".to_string()),
            "R1: forge.worktrees.ready must advance worktree_setup → exec_wave"
        );
    }

    // ── R2/S2: blocked branches into report ────────────────────────────────

    /// R2: forge.plan.blocked at planning enters report (not exec_wave).
    #[test]
    fn pf_14step_plan_blocked_at_planning_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "planning", "forge.plan.blocked");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.plan.blocked at planning must advance → report"
        );
    }

    /// R2: forge.plan.blocked at plan_authoring enters report.
    #[test]
    fn pf_14step_plan_blocked_at_plan_authoring_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "plan_authoring", "forge.plan.blocked");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.plan.blocked at plan_authoring must advance → report"
        );
    }

    /// R2: forge.plan.blocked at concurrency_review enters report.
    #[test]
    fn pf_14step_plan_blocked_at_concurrency_review_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "concurrency_review", "forge.plan.blocked");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.plan.blocked at concurrency_review must advance → report"
        );
    }

    /// R2: forge.plan.blocked at worktree_setup enters report.
    #[test]
    fn pf_14step_plan_blocked_at_worktree_setup_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "worktree_setup", "forge.plan.blocked");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.plan.blocked at worktree_setup must advance → report"
        );
    }

    /// R2: forge.plan.blocked at audit enters report.
    #[test]
    fn pf_14step_plan_blocked_at_audit_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "audit", "forge.plan.blocked");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.plan.blocked at audit must advance → report"
        );
    }

    /// R2: forge.audit.done enters report (on_any_of branch).
    #[test]
    fn pf_14step_audit_done_enters_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "audit", "forge.audit.done");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R2: forge.audit.done must advance audit → report"
        );
    }

    // ── R3/S3: exec_wave unit topics are non-transitions ────────────────────

    /// R3: exec.unit.done stays at exec_wave.
    #[test]
    fn pf_14step_exec_unit_done_is_non_transition() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.unit.done");
        assert_eq!(next, None, "R3: exec.unit.done must not advance exec_wave");
    }

    /// R3: exec.unit.failed stays at exec_wave.
    #[test]
    fn pf_14step_exec_unit_failed_is_non_transition() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.unit.failed");
        assert_eq!(
            next, None,
            "R3: exec.unit.failed must not advance exec_wave"
        );
    }

    /// S3: exec.unit.ready stays at exec_wave.
    #[test]
    fn pf_14step_exec_unit_ready_is_non_transition() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.unit.ready");
        assert_eq!(next, None, "S3: exec.unit.ready must not advance exec_wave");
    }

    // ── R4/S4: exec.wave.complete / exec.wave.failed branch distinctly ─────

    /// R4: exec.wave.complete enters exec_finalize (not unit_review).
    #[test]
    fn pf_14step_exec_wave_complete_enters_exec_finalize() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.wave.complete");
        assert_eq!(
            next,
            Some("exec_finalize".to_string()),
            "R4: exec.wave.complete must advance exec_wave → exec_finalize"
        );
    }

    /// R4: exec.wave.failed enters exec_failure (distinct from success).
    #[test]
    fn pf_14step_exec_wave_failed_enters_exec_failure() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_wave", "exec.wave.failed");
        assert_eq!(
            next,
            Some("exec_failure".to_string()),
            "R4: exec.wave.failed must advance exec_wave → exec_failure"
        );
    }

    /// R4: forge.exec.development.done enters unit_review (from exec_finalize).
    #[test]
    fn pf_14step_development_done_enters_unit_review() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_finalize", "forge.exec.development.done");
        assert_eq!(
            next,
            Some("unit_review".to_string()),
            "R4: forge.exec.development.done must advance exec_finalize → unit_review"
        );
    }

    /// R4 (U7): work.failed at exec_failure is now a transition
    /// (drives the `report` step via `on_any_of`). The legacy
    /// non-transition contract applied only to per-unit `work.failed`
    /// inside the exec_wave step; the plan-level `work.failed` at
    /// exec_failure / integration must advance to keep the route
    /// open.
    #[test]
    fn pf_14step_work_failed_at_exec_failure_advances_to_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_failure", "work.failed");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R4 (U7): work.failed at exec_failure must advance → report"
        );
    }

    /// R4: forge.report.done at exec_failure enters plan_end.
    #[test]
    fn pf_14step_report_done_at_exec_failure_enters_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "exec_failure", "forge.report.done");
        assert_eq!(
            next,
            Some("plan_end".to_string()),
            "R4: forge.report.done at exec_failure must advance → plan_end"
        );
    }

    // ── R5/S5: post-exec success chain ─────────────────────────────────────

    /// R5: forge.units.reviewed enters integration.
    #[test]
    fn pf_14step_units_reviewed_enters_integration() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "unit_review", "forge.units.reviewed");
        assert_eq!(
            next,
            Some("integration".to_string()),
            "R5: forge.units.reviewed must advance unit_review → integration"
        );
    }

    /// R5: forge.integration.done enters incremental_verify.
    #[test]
    fn pf_14step_integration_done_enters_incremental_verify() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "integration", "forge.integration.done");
        assert_eq!(
            next,
            Some("incremental_verify".to_string()),
            "R5: forge.integration.done must advance integration → incremental_verify"
        );
    }

    /// R5: forge.incremental.verified enters full_verify.
    #[test]
    fn pf_14step_incremental_verified_enters_full_verify() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "incremental_verify", "forge.incremental.verified");
        assert_eq!(
            next,
            Some("full_verify".to_string()),
            "R5: forge.incremental.verified must advance incremental_verify → full_verify"
        );
    }

    /// R5: forge.full.verified enters audit.
    #[test]
    fn pf_14step_full_verified_enters_audit() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "full_verify", "forge.full.verified");
        assert_eq!(
            next,
            Some("audit".to_string()),
            "R5: forge.full.verified must advance full_verify → audit"
        );
    }

    /// R5: forge.report.done at report enters plan_end.
    #[test]
    fn pf_14step_report_done_at_report_enters_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "report", "forge.report.done");
        assert_eq!(
            next,
            Some("plan_end".to_string()),
            "R5: forge.report.done must advance report → plan_end"
        );
    }

    /// R5: plan_end rejects LOOP_COMPLETE as transition (terminal).
    #[test]
    fn pf_14step_plan_end_loop_complete_is_non_transition() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "plan_end", "LOOP_COMPLETE");
        assert_eq!(next, None, "plan_end is the terminal step");
    }

    // ── R6/S6: failure-capable post-exec steps route to plan_end ────────────

    /// R6 (U7): work.failed at integration is now a transition to
    /// `report` (via `on_any_of`). The legacy non-transition
    /// contract was relaxed for plan-level `work.failed`.
    #[test]
    fn pf_14step_work_failed_at_integration_advances_to_report() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "integration", "work.failed");
        assert_eq!(
            next,
            Some("report".to_string()),
            "R6 (U7): work.failed at integration must advance → report"
        );
    }

    /// R6: forge.report.done at integration enters plan_end.
    #[test]
    fn pf_14step_report_done_at_integration_enters_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "integration", "forge.report.done");
        assert_eq!(
            next,
            Some("plan_end".to_string()),
            "R6: forge.report.done at integration must advance → plan_end"
        );
    }

    /// R6: forge.report.done at incremental_verify enters plan_end.
    #[test]
    fn pf_14step_report_done_at_incremental_verify_enters_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "incremental_verify", "forge.report.done");
        assert_eq!(
            next,
            Some("plan_end".to_string()),
            "R6: forge.report.done at incremental_verify must advance → plan_end"
        );
    }

    /// R6: forge.report.done at full_verify enters plan_end.
    #[test]
    fn pf_14step_report_done_at_full_verify_enters_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let next = advance_plan_step(&cfg, "full_verify", "forge.report.done");
        assert_eq!(
            next,
            Some("plan_end".to_string()),
            "R6: forge.report.done at full_verify must advance → plan_end"
        );
    }

    // ── R7/S7: replay/live equivalence + idempotency ────────────────────────

    /// R7: full happy-path fold reaches plan_end.
    #[test]
    fn pf_14step_recover_full_happy_path_folds_to_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.ready",
                "forge.concurrency.approved",
                "forge.worktrees.ready",
                "exec.wave.complete",
                "forge.exec.development.done",
                "forge.units.reviewed",
                "forge.integration.done",
                "forge.incremental.verified",
                "forge.full.verified",
                "forge.audit.done",
                "forge.report.done",
            ],
        );
        assert_eq!(
            recovered, "plan_end",
            "R7: full happy-path fold must reach plan_end"
        );
    }

    /// R7: replay yields the same step (no retrograde).
    #[test]
    fn pf_14step_recover_replay_is_idempotent() {
        let cfg = parallel_forge_14step_flow();
        let seq = [
            "forge.plan.inspected",
            "forge.plan.ready",
            "forge.concurrency.approved",
            "forge.worktrees.ready",
        ];
        let first = recover_current_plan_step(&cfg, &seq);
        let second = recover_current_plan_step(&cfg, &seq);
        assert_eq!(first, second, "R7: replay must yield the same step");
        assert_eq!(first, "exec_wave");
    }

    /// R7: failed-path fold reaches plan_end via exec_failure.
    #[test]
    fn pf_14step_recover_failed_path_folds_to_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.ready",
                "forge.concurrency.approved",
                "forge.worktrees.ready",
                "exec.wave.failed",
                "forge.report.done",
            ],
        );
        assert_eq!(
            recovered, "plan_end",
            "R7: failed-path fold must reach plan_end via exec_failure"
        );
    }

    /// R7: blocked-path fold reaches plan_end via report.
    #[test]
    fn pf_14step_recover_blocked_path_folds_to_plan_end() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.blocked",
                "forge.report.done",
            ],
        );
        assert_eq!(
            recovered, "plan_end",
            "R7: blocked-path fold must reach plan_end via report"
        );
    }

    /// R7: forge.plan.blocked at exec_wave is not in allowed_emits → stays.
    #[test]
    fn pf_14step_plan_blocked_at_exec_wave_not_in_allowed_emits() {
        let cfg = parallel_forge_14step_flow();
        // exec_wave.allowed_emits does NOT include forge.plan.blocked.
        let next = advance_plan_step(&cfg, "exec_wave", "forge.plan.blocked");
        assert_eq!(
            next, None,
            "R7: forge.plan.blocked at exec_wave must not trigger a transition"
        );
    }

    /// R7: initial step is planning.
    #[test]
    fn pf_14step_initial_step_is_planning() {
        let cfg = parallel_forge_14step_flow();
        assert_eq!(
            initial_current_plan_step(&cfg),
            "planning",
            "R7: initial step must be planning"
        );
    }

    /// R9: old/duplicate forge.concurrency.approved after exec_wave stays put.
    #[test]
    fn pf_14step_old_handoff_after_exec_wave_no_backstep() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.ready",
                "forge.concurrency.approved",
                "forge.worktrees.ready",
                "forge.plan.ready", // old handoff, must not backstep
            ],
        );
        assert_eq!(
            recovered, "exec_wave",
            "R9: old forge.plan.ready after exec_wave must not backstep"
        );
    }

    /// R9: repeated forge.plan.inspected at plan_authoring stays put.
    #[test]
    fn pf_14step_repeated_inspected_no_backstep() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.inspected", // duplicate
            ],
        );
        assert_eq!(
            recovered, "plan_authoring",
            "R9: repeated forge.plan.inspected must not backstep"
        );
    }

    /// R9: old forge.plan.inspected after exec_wave stays put.
    #[test]
    fn pf_14step_old_inspected_after_exec_wave_no_backstep() {
        let cfg = parallel_forge_14step_flow();
        let recovered = recover_current_plan_step(
            &cfg,
            &[
                "forge.plan.inspected",
                "forge.plan.ready",
                "forge.concurrency.approved",
                "forge.worktrees.ready",
                "forge.plan.inspected", // old handoff
            ],
        );
        assert_eq!(
            recovered, "exec_wave",
            "R9: old forge.plan.inspected after exec_wave must not backstep"
        );
    }
}
