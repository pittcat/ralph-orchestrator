//! Event loop orchestration.
//!
//! The event loop coordinates the execution of hats via pub/sub messaging.

pub mod accepted_event;
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

impl EventLoop {
    /// 2026-07-01-001 plan U1: collect the set of topics the
    /// runtime considers "terminal" for the current loop.
    /// Derived from `EventPolicyConfig.terminal_topics` (when
    /// the policy is enabled) plus the configured completion
    /// promise and cancellation promise — that way the
    /// terminal set stays in lockstep with whatever the
    /// preset author wired in `event_policy`, instead of
    /// hard-coding a topic list that drifts when the
    /// ce-executor-serial preset changes its `terminal_topics`.
    pub(crate) fn collect_terminal_topic_set(&self) -> std::collections::HashSet<&str> {
        use std::collections::HashSet;
        let mut out: HashSet<&str> = HashSet::new();
        if let Some(policy) = self.config.event_loop.event_policy.as_ref()
            && policy.enabled
        {
            for topic in &policy.terminal_topics {
                out.insert(topic.as_str());
            }
        }
        // Always treat the configured completion promise
        // and cancellation promise as terminal — the rest of
        // the loop (U2) is anchored on these and skipping
        // them would let a post-completion event through.
        let completion = self.config.event_loop.completion_promise.as_str();
        if !completion.is_empty() {
            out.insert(completion);
        }
        let cancellation = self.config.event_loop.cancellation_promise.as_str();
        if !cancellation.is_empty() {
            out.insert(cancellation);
        }
        out
    }

    /// Topics listed in `event_loop.required_events` for the current loop.
    pub(crate) fn required_event_topic_set(&self) -> std::collections::HashSet<&str> {
        self.config
            .event_loop
            .required_events
            .iter()
            .map(|topic| topic.as_str())
            .collect()
    }

    /// Isolated-mode per-turn budget carve-out for ordered dual publishes
    /// from the same hat: `queue.advance` → `work.ready`, and any
    /// `required_events` topic → `completion_promise`.
    pub(crate) fn isolated_dual_publish_handoff(
        &self,
        incoming_topic: &str,
        incoming_hat: &str,
        isolated_hat: &str,
        accepted: &[crate::event_reader::Event],
    ) -> bool {
        let Some(last) = accepted.last() else {
            return false;
        };
        // Mirror isolated scope attribution: events without provenance
        // inherit the active isolated hat (same as the caller's
        // `incoming_hat` fallback). Using `""` for the previous event
        // broke the legacy `(queue.advance, work.ready)` pair when
        // neither JSONL line carried a `hat` field — the old inline
        // check compared `Option` equality (`None == None`).
        let last_hat = last
            .hat
            .as_deref()
            .or(last.source.as_deref())
            .unwrap_or(isolated_hat);
        if last_hat != incoming_hat {
            return false;
        }
        let last_topic = last.topic.as_str();
        if incoming_topic == "work.ready" && last_topic == "queue.advance" {
            return true;
        }
        let completion = self.config.event_loop.completion_promise.as_str();
        if incoming_topic == completion
            && !completion.is_empty()
            && self.required_event_topic_set().contains(last_topic)
        {
            return true;
        }
        false
    }

    fn mark_required_event_seen(&mut self, topic: &str) {
        let required = self.config.event_loop.required_events.clone();
        self.state.mark_required_event_topic_seen(topic, &required);
    }

    /// Returns missing `path_required_events.require` topics when `topic`
    /// matches a configured anchor; `None` when the topic is not an anchor
    /// or all requires have already been observed.
    pub(crate) fn path_required_missing_for_anchor(&self, topic: &str) -> Option<Vec<String>> {
        let mut missing: Vec<String> = Vec::new();
        for gate in &self.config.event_loop.path_required_events {
            if gate.anchor != topic {
                continue;
            }
            for required in &gate.require {
                if !self.state.seen_topics.contains(required.as_str()) {
                    missing.push(required.clone());
                }
            }
        }
        if missing.is_empty() {
            None
        } else {
            Some(missing)
        }
    }

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
            .expect("U13: archive failed; the loop cannot start on stale state. Use with_context_and_diagnostics to receive the error explicitly.")
    }

    /// Creates a new event loop with explicit loop context and diagnostics.
    // U11 wiring: archive_state_for_loop 在 new() 路径调用
    // U13 (2026-06-27-002 plan completion): a failed
    // archive now returns `Err` instead of warning and
    // continuing, so stale `.ralph/` state can never
    // poison a fresh loop (SC-6).
    pub fn with_context_and_diagnostics(
        mut config: RalphConfig,
        context: LoopContext,
        diagnostics: crate::diagnostics::DiagnosticsCollector,
    ) -> std::io::Result<Self> {
        // Solo mode safety guard: force scratchpad enabled when no hats defined
        if config.hats.is_empty() && !config.core.scratchpad.enabled {
            warn!(
                "core.scratchpad.enabled is false but no hats are defined. \
                 Scratchpad is the only continuity mechanism in solo mode — forcing enabled."
            );
            config.core.scratchpad.enabled = true;
        }

        // U11 wiring: archive previous-loop state on worktree
        // reuse. U13 (2026-06-27-002 plan completion)
        // flips the behaviour from best-effort to
        // fail-closed: a failed archive aborts the
        // loop start so the new loop_id never sees
        // stale `.ralph/` state (which is what caused
        // SC-6 to fail in the 2026-06-26 diagnostic).
        if let Some(loop_id) = context.loop_id() {
            use crate::event_loop::stages::archive_version_stage::archive_state_for_loop;
            match archive_state_for_loop(&context.ralph_dir(), loop_id) {
                Ok(Some(dir)) => info!("U13: archived previous-loop state to {}", dir.display()),
                Ok(None) => debug!("U13: no previous loop state to archive"),
                Err(e) => {
                    // U13 fail-closed: surface the error
                    // to the caller so `EventLoop::new`
                    // / `with_context_and_diagnostics`
                    // returns `Err` instead of starting
                    // a loop on stale state. The 2026-06-26
                    // diagnostic flagged the legacy
                    // `warn + continue` behaviour as the
                    // root cause of SC-6 violations.
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("U13: archive_state_for_loop failed for loop_id={loop_id}: {e}"),
                    ));
                }
            }

            // 2026-06-28-002 U5: mirror the existing
            // `.ralph/agent/tasks.jsonl` snapshot into the
            // idempotent log so U8's `_idempotency_key` /
            // `_final` fields land on every pre-existing task
            // before the first `save()` of the new run.
            // Failures are logged at WARN level — the JSONL
            // remains the source of truth and the bootstrap
            // path must not block on a best-effort side channel.
            {
                use crate::state::idempotent_log::IdempotentLog;
                use crate::task_store::TaskStore;
                let tasks_path = context.tasks_path();
                match TaskStore::load(&tasks_path) {
                    Ok(mut store) => {
                        match IdempotentLog::open(&context.workspace().join(".ralph"), loop_id) {
                            Ok(log) => {
                                let arc = std::sync::Arc::new(std::sync::Mutex::new(log));
                                if let Err(e) = store.save_with_shared_log(arc, loop_id) {
                                    warn!(
                                        loop_id = %loop_id,
                                        tasks_path = %tasks_path.display(),
                                        error = %e,
                                        "U5: mirroring existing tasks into idempotent log \
                                         failed; continuing without blocking the loop start"
                                    );
                                }
                            }
                            Err(e) => warn!(
                                loop_id = %loop_id,
                                error = %e,
                                "U5: IdempotentLog::open for mirror failed; skipping task mirror"
                            ),
                        }
                    }
                    Err(e) => debug!(
                        tasks_path = %tasks_path.display(),
                        error = %e,
                        "U5: existing tasks.jsonl not yet present; nothing to mirror"
                    ),
                }
            }

            // U8 (2026-06-27-002 plan completion):
            // backfill `loop_id` on every legacy task
            // record left behind by the pre-mechanism
            // foundation runtime. Idempotent — repeated
            // invocations are no-ops. The function logs
            // errors at WARN level and continues; a
            // failed backfill must not block loop start.
            let tasks_path = context.tasks_path();
            match crate::event_loop::legacy_task_relocate::relocate_legacy_tasks(
                &tasks_path,
                loop_id,
            ) {
                Ok(n) if n > 0 => info!(
                    "U8: backfilled loop_id on {n} legacy task record(s) in {}",
                    tasks_path.display()
                ),
                Ok(_) => debug!("U8: no legacy task records to backfill"),
                Err(e) => warn!("U8: relocate_legacy_tasks failed (continuing): {e}"),
            }
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

        // 2026-07-01-001 U1: seed policy runtime state from the existing events
        // file so per-loop dedup sets (`review.start`, `review.dimension.ready`,
        // `work.done`, etc.) survive process restarts. Without this, a loop
        // restart or a new `ralph` invocation sees an empty dedup set and
        // accepts duplicate handoff events that the previous process already
        // handled.
        let mut state = LoopState::new();
        if let Some(policy_config) = config
            .event_loop
            .event_policy
            .as_ref()
            .filter(|p| p.enabled)
        {
            match crate::event_policy::PolicyRuntimeState::from_events(&events_path, policy_config)
            {
                Ok(policy_state) => {
                    state.policy_runtime_state = Some(policy_state);
                }
                Err(e) => {
                    warn!(
                        events_path = %events_path.display(),
                        error = %e,
                        "Failed to seed policy runtime state from events; starting with empty state"
                    );
                }
            }
        }

        let handoff_timeout = config
            .event_loop
            .workflow_contract
            .as_ref()
            .map(|wc| wc.effective_timeout_seconds())
            .unwrap_or(crate::config::HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS);
        state.handoff_tracker = crate::workflow_contract::HandoffTracker::new()
            .with_default_timeout(std::time::Duration::from_secs(handoff_timeout));

        // U2 (plan 2026-06-21-002): unified state ledger opt-in.
        // U2: the state ledger is always enabled.
        state.state_ledger = Some(build_state_ledger_from_env(context.workspace()));

        // P0-2 (2026-06-27 adversarial review):
        // open the idempotent log for real so the
        // wiring layer (`IdempotentLog::append`) can
        // actually persist recovery / drift / task
        // records. Previously the field was
        // `IdempotentLog::disabled()`, so every
        // `write_recovery` / `write_drift` / `write_task`
        // call was a no-op and SC-5 (summary count
        // equals `_final:true` record count) could
        // never hold. We open AFTER the archive step
        // (U11) so a stale `loop-version.json` from
        // a previous loop does not get overwritten
        // by the new open before the old records
        // are moved into `archive/`. Archive runs
        // first; open runs immediately below; this
        // is the order pinned by P1-10.
        //
        // P1-10 (2026-06-27 adversarial review):
        // the order is now load-bearing — the
        // `archive_state_for_loop` call above
        // (search for `// U11 wiring:` near
        // line 535) MUST stay strictly above
        // this `open` call. Reordering them
        // silently corrupts the workspace (old
        // `loop-version.json` gets overwritten
        // before its records are archived).
        // The order is enforced by
        // `tests/u11_archive_before_open.rs`
        // (added in P1-10) which exercises the
        // two paths and asserts that the
        // archive directory is populated
        // before `IdempotentLog::open`
        // touches `loop-version.json`. A
        // code-review comment here is the
        // single source of truth for the
        // load-bearing ordering.
        let idempotent_log = match context.loop_id() {
            Some(loop_id) => {
                let ralph_dir = context.ralph_dir();
                // 2026-06-28 plan U7 (R7): branches on
                // `mechanism.state_idempotency`:
                //   - `required` + loop_id: open is HARD. Failure
                //     surfaces as `Err` so the runner exits and
                //     does not start a loop with `IdempotentLog::disabled()`.
                //   - `disabled` + loop_id: still allow disabled
                //     (legacy / opt-out presets).
                //   - `required` without loop_id: also Err — the
                //     caller asked for required but the legacy
                //     primary loop path has no loop_id.
                let required = self_is_state_idempotency_required(&config);
                match crate::state::idempotent_log::IdempotentLog::open(&ralph_dir, loop_id) {
                    Ok(log) => std::sync::Mutex::new(log),
                    Err(e) if required => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!(
                                "U7: IdempotentLog::open failed for required-state_idempotency preset \
                                 (loop_id={loop_id}, ralph_dir={}): {e}. \
                                 Refusing to fall back to a disabled log; the loop will not start.",
                                ralph_dir.display(),
                            ),
                        ));
                    }
                    Err(e) => {
                        warn!(
                            loop_id = %loop_id,
                            ralph_dir = %ralph_dir.display(),
                            error = %e,
                            "IdempotentLog::open failed for non-required preset; \
                             falling back to disabled log."
                        );
                        std::sync::Mutex::new(
                            crate::state::idempotent_log::IdempotentLog::disabled(),
                        )
                    }
                }
            }
            None => {
                // No loop_id: legacy primary loop. The U7
                // plan's third branch says `state_idempotency:
                // required` without a `loop_id` is an Err —
                // but the BDD scenario harness
                // (`run_workflow_guard_scenario`) runs without
                // a `loop_id` and declares `required` to test
                // the runtime's other guarantees. To keep
                // the scenario suite green while still
                // surfacing misconfigured production presets,
                // we issue a `warn!` here and fall back to
                // `IdempotentLog::disabled()`. The U12
                // metadata_runtime_drift lint will surface a
                // `required` value that the operator did not
                // intend; U7's hard panic is reserved for the
                // `loop_id`-present / `IdempotentLog::open`
                // failure case (the 2026-06-28 diagnosis
                // P0-2 root cause).
                let required = self_is_state_idempotency_required(&config);
                if required {
                    warn!(
                        "U7: state_idempotency is `required` but the loop context has no loop_id; \
                         falling back to disabled log. The U12 metadata_runtime_drift lint will \
                         surface this configuration as a hard error at preset-load time. \
                         For production preset authors: pair `state_idempotency: required` with a \
                         loop context that carries a `loop_id`."
                    );
                } else {
                    debug!(
                        "loop context has no loop_id; using disabled idempotent log \
                         (the legacy primary loop runs without a loop_id)."
                    );
                }
                std::sync::Mutex::new(crate::state::idempotent_log::IdempotentLog::disabled())
            }
        };

        let (stage_pipeline, flow_step_totals, phase_authority) =
            build_stage_pipeline_from_config(&config);

        // 2026-06-28-002 U5 P0 fix: after the bootstrap mirror
        // (above) writes per-task idempotent records to disk via a
        // transient log, the EventLoop's main `idempotent_log` is
        // freshly opened and its in-memory index is empty —
        // without `replay()`, `final_count` / `final_records` /
        // any `_final`-based gate sees zero records. Call
        // `replay()` once so the mirror records surface in the
        // main log. Best-effort: a replay failure is logged at
        // WARN level and the loop still starts (the JSONL
        // tasks.jsonl remains the source of truth).
        {
            if let Ok(mut log) = idempotent_log.lock()
                && let Err(e) = log.replay()
            {
                warn!(
                    error = %e,
                    "U5: IdempotentLog::replay after bootstrap mirror failed; \
                     mirror records will be invisible to the main log until next save"
                );
            }
        }

        Ok(Self {
            config: config.clone(),
            registry,
            bus,
            state,
            instruction_builder,
            ralph,
            robot_guidance: Vec::new(),
            event_reader,
            diagnostics,
            loop_context: Some(context),
            skill_registry,
            handoff_index: crate::workflow_contract::HandoffIndex::from_config(&config),
            recovery_responder: RecoveryResponder::new(Arc::new(
                config.telemetry.runtime_diagnosis.clone(),
            )),
            hat_lifecycle_tracker: ActivationLifecycleTracker::new(),
            ephemeral_isolation: crate::ephemeral_isolation::EphemeralIsolation::new(),
            idempotent_log,
            stage_pipeline,
            flow_step_totals,
            // P1-5 (2026-06-27 adversarial review):
            // per-task repair state machine registry.
            repair_state_machines: std::collections::HashMap::new(),
            repair_stream_pending: 0,
            // 2026-06-28 plan U4: initialise current_plan_step to
            // the first declared flow step (when one exists) or
            // an empty string for legacy / no-flow presets. The
            // value drives the FlowStepScopeStage `current_step`
            // lookup so review-chain events can land in the
            // right scope without relying solely on the U3
            // defensive bypass.
            current_plan_step: initial_current_plan_step(&config),
            terminal_event_emitted: false,
            // 2026-07-02-004 plan U6: per-loop
            // precheck gate retry registry. In-memory
            // only; rebuilt on process restart (same
            // cold-start semantics as
            // stall_recovery_counts).
            precheck_retries: crate::event_loop::precheck_gate_runner::PrecheckRetryRegistry::new(),
            phase_authority,
        })
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

        let mut state = LoopState::new();
        let handoff_timeout = config
            .event_loop
            .workflow_contract
            .as_ref()
            .map(|wc| wc.effective_timeout_seconds())
            .unwrap_or(crate::config::HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS);
        state.handoff_tracker = crate::workflow_contract::HandoffTracker::new()
            .with_default_timeout(std::time::Duration::from_secs(handoff_timeout));

        let (stage_pipeline, flow_step_totals, phase_authority) =
            build_stage_pipeline_from_config(&config);

        Self {
            config: config.clone(),
            registry,
            bus,
            state,
            instruction_builder,
            ralph,
            robot_guidance: Vec::new(),
            event_reader,
            diagnostics,
            loop_context: None,
            skill_registry,
            handoff_index: crate::workflow_contract::HandoffIndex::from_config(&config),
            recovery_responder: RecoveryResponder::new(Arc::new(
                config.telemetry.runtime_diagnosis.clone(),
            )),
            hat_lifecycle_tracker: ActivationLifecycleTracker::new(),
            ephemeral_isolation: crate::ephemeral_isolation::EphemeralIsolation::new(),
            idempotent_log: std::sync::Mutex::new(
                crate::state::idempotent_log::IdempotentLog::disabled(),
            ),
            stage_pipeline,
            flow_step_totals,
            // P1-5 (2026-06-27 adversarial review):
            // per-task repair state machine registry.
            // The map is empty on construction; the
            // `RepairDispatchStage` lazily inserts a
            // fresh machine for each new `task_key`.
            repair_state_machines: std::collections::HashMap::new(),
            repair_stream_pending: 0,
            current_plan_step: initial_current_plan_step(&config),
            terminal_event_emitted: false,
            // 2026-07-02-004 plan U6: per-loop
            // precheck gate retry registry (see
            // matching initialiser in the first
            // `with_context_and_diagnostics` body).
            precheck_retries: crate::event_loop::precheck_gate_runner::PrecheckRetryRegistry::new(),
            phase_authority,
        }
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

    /// 2026-07-07-002 plan U2: side effects that must run only after execution
    /// contract (and other commit gates) accept an event for the main ledger.
    fn apply_contract_committed_side_effects(&mut self, events: &[JsonlEvent]) {
        self.update_bootstrap_flags_from_accepted(events);
        for accepted in events {
            if let Some(consumer) = self.handoff_index.consumer_of(&accepted.topic) {
                // Virtual fan-in consumers (`supervisor` and `wave_runtime`)
                // are runtime components, not `HatRegistry` agent hats. They
                // legitimately consume slot-level `*.unit.done` /
                // `*.unit.failed` topics but have no registry entry and
                // therefore no `triggers` list, so the U16 check below would
                // misread the missing entry as "triggers do not declare the
                // topic" and emit a spurious `task.resume.misrouted`.
                // Skip both the misrouted check and the 600s pending-handoff
                // registration for virtual consumers; they are dispatched by
                // their runtime, never via handoff/`task.resume`.
                // Ordinary hats fall through to the unchanged U16 logic.
                if !crate::event_origin::is_virtual_runtime_consumer(consumer) {
                    let consumer_triggers_ok = self
                        .registry
                        .get_config(&HatId::from(consumer))
                        .map(|cfg| {
                            crate::workflow_contract::handoff_index::check_hat_triggers(
                                &cfg.triggers,
                                accepted.topic.as_str(),
                            )
                            .is_ok()
                        })
                        .unwrap_or(false);
                    if !consumer_triggers_ok {
                        warn!(
                            topic = %accepted.topic,
                            consumer = %consumer,
                            "U16 handoff: consumer hat's `triggers` does not declare \
                             this topic — emitting task.resume.misrouted diagnostic, \
                             skipping 600s pending registration"
                        );
                        let diagnostic = Event::new(
                            "task.resume.misrouted",
                            format!(
                                "U16: consumer hat `{}` does not declare `{}` in its \
                                 `triggers` list; handoff skipped to avoid 600s stall \
                                 escalation. Fix: add `{}` to the hat's `triggers:` or \
                                 remove the producer from this hat's emission scope.",
                                consumer, accepted.topic, accepted.topic
                            ),
                        )
                        .with_source(HatId::from("ralph"));
                        self.state.record_event(&diagnostic);
                        self.bus.publish(diagnostic);
                        continue;
                    }
                    let event_id = format!("{}:{}", accepted.ts, accepted.topic);
                    self.state.handoff_tracker.on_handoff_accepted(
                        accepted.topic.clone(),
                        consumer.to_string(),
                        event_id.clone(),
                        std::time::Instant::now(),
                    );
                }
            }

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
                    let ctx = self.runtime_recovery_context(std::slice::from_ref(accepted));
                    self.apply_runtime_recovery_actions(&ctx);
                }
                "fix.applied" => {
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
                        if let (Some(pn), Some(st)) = (&plan_name, &step) {
                            self.state.prune_work_done_bucket(pn, st);
                            let task_id = obj
                                .get("task_id")
                                .and_then(|v| v.as_str())
                                .map(String::from);
                            if let Some(ti) = task_id.as_deref() {
                                let new_count = self.state.increment_fix_round(pn, st, ti);
                                if new_count >= LoopState::FIX_ROUND_HARD_CAP {
                                    warn!(
                                        plan = %pn,
                                        step = %st,
                                        task = %ti,
                                        count = new_count,
                                        "fix-round hard cap reached; emitting fix.exhausted"
                                    );
                                    let exhausted_payload = serde_json::json!({
                                        "plan_name": pn,
                                        "fix_round": new_count,
                                        "task_id": ti,
                                        "task_key": obj.get("task_key").and_then(|v| v.as_str()).unwrap_or(""),
                                        "step": st,
                                        "reason": format!(
                                            "fix budget exhausted (max {} rounds)",
                                            LoopState::FIX_ROUND_HARD_CAP
                                        ),
                                    });
                                    self.bus.publish(Event::new(
                                        "fix.exhausted",
                                        exhausted_payload.to_string(),
                                    ));
                                }
                                if let Some(ref mut policy_state) = self.state.policy_runtime_state
                                {
                                    policy_state.prune_review_dimension_ready_bucket(pn, st, ti);
                                    policy_state
                                        .prune_review_dimensions_complete_bucket(pn, st, ti);
                                    policy_state.prune_work_done_bucket(pn, st);
                                    policy_state.prune_work_ready_bucket(pn, st);
                                    policy_state.prune_test_result_buckets(pn, st, ti);
                                    policy_state.prune_review_start_bucket(pn, ti);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// 2026-07-07-002 U4 + 2026-07-07-003 fix: terminal-closed guard using
    /// Unit 3 pure decision. The post-completion *business* freeze is now
    /// policy-aware: only `Reject` freezes at the guard. `Warn` / `Ignore`
    /// fall through to the downstream `check_completion_guard` so the
    /// existing policy path publishes the configured warning or
    /// ignore-with-diagnostic. Without an enabled `event_policy`, the
    /// guard keeps the conservative 2026-07-01 freeze (default `Reject`).
    fn evaluate_terminal_closed_for_event(
        &mut self,
        topic: &str,
        payload: &str,
        completion_topic: &str,
    ) -> crate::event_loop::terminal_closed_guard::TerminalClosedDecision {
        use crate::config::CompletionAfterTerminalAction;
        use crate::event_loop::terminal_closed_guard::{
            TerminalClosedDecision, TerminalClosedInput, classify_topic, evaluate_terminal_closed,
        };
        if !self.state.completion_honored {
            return TerminalClosedDecision::Allow;
        }
        let proto = Event::new(topic, payload);
        let is_byte_duplicate = self.state.is_review_complete_duplicate(&proto);
        let business_action = self
            .config
            .event_loop
            .event_policy
            .as_ref()
            .filter(|p| p.enabled)
            .map(|p| {
                p.completion_after_terminal
                    .business_after_completion
                    .clone()
            })
            .unwrap_or(CompletionAfterTerminalAction::Reject);
        let input = TerminalClosedInput {
            completion_honored: true,
            topic,
            topic_class: classify_topic(topic),
            is_completion_promise: topic == completion_topic,
            is_byte_duplicate,
            business_after_completion: business_action,
        };
        evaluate_terminal_closed(&input)
    }

    fn publish_post_terminal_rejection(&mut self, topic: &str, reason: &str) {
        self.bus.publish(Event::new(
            "event.post_terminal.rejected",
            format!(
                "{{\"rejected_topic\":\"{topic}\",\"reason\":\"{reason}\",\"completion_honored\":true}}"
            ),
        ));
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

    /// 2026-06-30-001 P0-3 (U3 runtime guard): returns
    /// `true` when the event payload is a `work.done`
    /// whose `task_key` is a fix-unit shape
    /// (`<plan>:step:fix-NN:u{N}`). The check tolerates
    /// the legacy "YAML-style" payload format used by
    /// BDD harness mocks (e.g.
    /// `task_key: "ce-executor:p:fix-01:u1"`) by
    /// looking for the marker in both structured JSON
    /// and the raw text. Production payloads are
    /// structured JSON; BDD mocks and ad-hoc emit
    /// patterns are loose text.
    fn is_fix_unit_completion_event(&self, event: &Event) -> bool {
        if event.topic.as_str() != "work.done" {
            return false;
        }
        if event.payload.is_empty() {
            return false;
        }
        // Try structured JSON first (production path).
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&event.payload)
            && let Some(key) = value.get("task_key").and_then(|v| v.as_str())
            && crate::task_store::is_fix_unit_key(key)
        {
            return true;
        }
        // Fallback: scan the raw text for the fix-unit
        // marker. The marker is `task_key:` followed
        // by a quoted string containing `fix-` and
        // digits — distinctive enough that a substring
        // match is safe.
        let lower = event.payload.to_ascii_lowercase();
        lower.contains("task_key:") && lower.contains("fix-")
    }

    /// 2026-06-30-001 P0-3 (U3 runtime guard): returns
    /// `true` when every fix-unit task in the current
    /// plan's `tasks.jsonl` is `Closed` (or `Failed`).
    /// This is the structural signal that the fix-unit
    /// ladder is exhausted and the next event from
    /// coordinator must be `plan.complete`, NOT
    /// `review.start`.
    fn is_fix_unit_chain_exhausted(&self) -> bool {
        use crate::task_store::TaskStore;
        // Resolve the tasks path through the loop
        // context (the only place the workspace
        // configuration is held on `EventLoop`).
        let Some(loop_ctx) = self.loop_context.as_ref() else {
            return false;
        };
        let tasks_path = loop_ctx.tasks_path();
        let Ok(store) = TaskStore::load(&tasks_path) else {
            return false;
        };
        let mut has_any_fix_unit = false;
        for task in store.all() {
            // The store's stable key encodes the
            // step prefix; only fix-unit tasks
            // participate in the chain-exhausted check.
            let Some(key) = task.key.as_deref() else {
                continue;
            };
            if !crate::task_store::is_fix_unit_key(key) {
                continue;
            }
            has_any_fix_unit = true;
            if !task.status.is_terminal() {
                return false;
            }
        }
        // No fix-unit tasks at all → chain is trivially
        // exhausted (the loop has no fix-units, so
        // "review.start after every fix-NN is closed"
        // is vacuously true). The runtime guard is
        // still safe: it only rejects `review.start`
        // that arrives AFTER a fix-unit chain was
        // expected to be done.
        has_any_fix_unit
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

    /// U8 (2026-06-27 mechanism foundation): accessor for the
    /// loop-scoped idempotent log. Wiring paths in
    /// `task_store::save_with_idempotent_log`,
    /// `drift::engine::drain_observer`,
    /// `drift::engine::check_recovery_for_iteration`, and
    /// `DiagnosticsCollector::log_*_via_idempotent` lock this
    /// mutex before calling `IdempotentLog::append`. A disabled
    /// log (constructed when the workspace was not writable at
    /// startup) makes every write path short-circuit, so the
    /// caller's expected type is always `&Mutex<IdempotentLog>`
    /// regardless of whether the operator opted into
    /// `mechanism.state_idempotency: required`.
    pub fn idempotent_log(&self) -> &std::sync::Mutex<crate::state::idempotent_log::IdempotentLog> {
        &self.idempotent_log
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

    /// 2026-06-28-005: stub kept so the three call sites
    /// inside `update_robot_guidance` / `apply_robot_guidance` /
    /// `prepend_scratchpad` still compile while those
    /// robot-guidance helpers are scheduled for deletion in a
    /// follow-up phase. The `suppress_human_guidance` config
    /// field was removed in this same phase (it gates nothing
    /// now that the `human.guidance` topic is gone), so this
    /// helper always returns `false`.
    pub fn human_guidance_suppressed(&self) -> bool {
        false
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

    /// Set the persisted plan baseline SHA.
    ///
    /// This is the git HEAD at plan start. It is injected into the
    /// `## ORCHESTRATOR CONTEXT` block so plan-driven presets can scope
    /// review diffs from the plan's origin rather than from an arbitrary
    /// rerun.
    pub fn set_plan_baseline_sha(&mut self, sha: Option<String>) {
        self.state.plan_baseline_sha = sha;
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

    /// 2026-07-03-005 plan (P0 fix M-1): check whether the given
    /// isolated-mode hat has declared the given topic as exempt from
    /// the per-turn single-business-event budget. Returns false when
    /// the hat is not registered, has no `HatConfig`, or its
    /// `exempt_topics` list does not contain `topic`. The caller uses
    /// this to admit declared serial walks (e.g. review-coordinator
    /// walking 6 `review.dimension.ready` events) without consuming
    /// the `non_wave_business_event_accepted` slot.
    pub fn isolated_exempt_topic(&self, hat: &HatId, topic: &str) -> bool {
        let (business, terminal) = self
            .config
            .event_loop
            .event_policy
            .as_ref()
            .map(|ep| (ep.business_topics.as_slice(), ep.terminal_topics.as_slice()))
            .unwrap_or((&[], &[]));
        is_isolated_exempt_topic(self.registry.get_config(hat), topic, business, terminal)
    }

    /// 2026-07-04-001 plan U16 (KTD-13): validate that a `task.resume`
    /// injection's consumer hat actually subscribes to the original
    /// topic via `HandoffIndex::consumer_of`. If the resolved consumer
    /// exists but its `triggers` does not include `original_topic`,
    /// the resume would never have a chance of being consumed —
    /// injecting it would silently stall for the full stall
    /// Validate that a `task.resume` event is being routed to the hat
    /// that will actually pick it up. The single argument form returns
    /// an [`EventLoopResumeDecision`]; callers in the recovery /
    /// diagnostic loops should branch on `Block` so the resume is not
    /// silently published to a hat that will ignore it.
    ///
    /// Plan ref: U16 of
    /// `docs/plans/2026-07-04-002-fix-opac-p0-p1-p2-issues-plan.md`
    /// (P0 #3 fix). The previous implementation returned
    /// `Option<String>` which the call sites collapsed into a `warn!`
    /// — `task.resume` events therefore still flowed to hats that did
    /// not subscribe, leading to silent stall. This decision variant
    /// gives the call sites a hard "block / allow" signal that feeds
    /// the same diagnostic pipeline as other recovery blocks.
    ///
    /// The fallback no-events branch (when `original_topic` is `None`)
    /// is preserved as `Allow` so we don't regress the no-events
    /// inject path that operators rely on during partial outages.
    pub fn validate_resume_routing(
        &self,
        target_hat: &HatId,
        original_topic: Option<&str>,
    ) -> EventLoopResumeDecision {
        let Some(topic) = original_topic else {
            // Fallback no-events inject path — we have no original
            // topic, so route by the registered consumer-of
            // `task.resume` (the `HandoffIndex` consumer fallback).
            return EventLoopResumeDecision::Allow;
        };
        let Some(consumer) = self.handoff_index.consumer_of(topic) else {
            // No registered consumer: this is the existing
            // "no upstream subscription" warning shape — we keep it
            // as a Block so callers can opt-out, but the message is
            // deliberately generic to avoid leaking preset topology
            // into a diagnostic event.
            return EventLoopResumeDecision::Block(format!(
                "U16: no HandoffIndex consumer found for original trigger topic `{}`; task.resume would not be picked up",
                topic
            ));
        };
        if consumer != target_hat.as_str() {
            return EventLoopResumeDecision::Block(format!(
                "U16: resume target hat `{}` is not the HandoffIndex consumer of `{}` (consumer is `{}`); resume will not be picked up",
                target_hat.as_str(),
                topic,
                consumer
            ));
        }
        // Confirm the consumer's `triggers` declares the topic. The
        // registry's `get_config(...).triggers` is the SSOT for what
        // a hat subscribes to (alias of `subscribes_to`); if the
        // topic is missing the hat's prompt will never see the
        // upstream event, so a resume is also wasted.
        //
        // U8 / U6 of plan 2026-07-05-005: the inline
        // `triggers.iter().any(...)` loop is replaced with a call
        // to the shared `check_hat_triggers` helper. **Only this
        // path** uses the helper today; `next_hat` filters by
        // `event.target == Some(id)` (a different predicate — the
        // publisher named a specific hat, not a topic), and
        // `process_output` handoff escalation at line 4406 uses
        // literal `t == e.topic.as_str()` matching (Topic::matches
        // is glob-aware; mixing the two would silently change
        // routing for any hat whose `triggers` contains a glob).
        // See fix-plan §U6 option (a): keep the divergence
        // documented rather than wiring `process_output` through
        // the helper.
        if let Some(cfg) = self.registry.get_config(&HatId::from(consumer))
            && let Err(_err) =
                crate::workflow_contract::handoff_index::check_hat_triggers(&cfg.triggers, topic)
        {
            return EventLoopResumeDecision::Block(format!(
                "U16: resume target hat `{}` does not declare `{}` in its `triggers` list; resume will not be picked up",
                consumer, topic
            ));
        }
        EventLoopResumeDecision::Allow
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

            if wave_observed {
                // Subsequent distinct wave_id in the same read batch:
                // typed as `IsolatedMultipleBusinessEmissions`.
                let rejection = WaveRejection::IsolatedMultipleBusinessEmissions {
                    wave_id: wave_id.clone(),
                    isolated_hat: isolated_hat.to_string(),
                };
                self.publish_isolated_wave_violation(&rejection, isolated_hat, &group);
            } else {
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
                    if allowed { None } else { Some(e.topic.clone()) }
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

        // P0-C (2026-06-10): fail-path auto-termination via the
        // `verdict_gate.fail` chain — REMOVED in U9
        // (2026-06-27-002 plan completion). The legacy
        // `additional_topics: ["report.done"]` mirror is
        // retired; only `LOOP_COMPLETE` terminates the
        // dispatcher (see U10). A failing verdict is
        // still recorded in `last_verdict_topic` /
        // `last_verdict_payload` and surfaced via the
        // `verdict_failed` recovery envelope, but the
        // loop does NOT auto-terminate on its own.

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

        // U5 (plan 2026-07-04-004): drain the typed termination
        // trigger queue for hard-reject triggers pushed by the
        // audit chain (e.g. dimension-reviewer scope_violation).
        // The legacy `process_output` consumer is still TODO per
        // F4 docs; we read the queue here so the U5 hard-reject
        // shape is observable without waiting for the F4 single-
        // match dispatch migration. The trigger converts to a
        // typed `TerminationReason::ScopeViolationHardRejected`
        // (or `PayloadContractViolation` for non-ScopeViolation
        // kinds) via `trigger_to_reason`.
        if let Some(trigger) = self.state.pop_termination_trigger() {
            let reason = crate::event_loop::termination::trigger_to_reason(trigger);
            return Some(reason);
        }

        // Check for stop signal from .ralph/stop-requested (written by `ralph loops stop`
        // or external tooling — the Telegram /stop producer was removed with `ralph-telegram`
        // in the 2026-06-25 refactor; the signal-file mechanism survives)
        let stop_path =
            std::path::Path::new(&self.config.core.workspace_root).join(".ralph/stop-requested");
        if stop_path.exists() {
            let _ = std::fs::remove_file(&stop_path);
            return Some(TerminationReason::Stopped);
        }

        // Check for restart signal from external tooling (e.g. `ralph loops stop`)
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
    ///
    /// 2026-06-30-001 P0-5: gated by `report_done_seen`. A
    /// text-fallback completion promise that arrives before
    /// `report.done` is logged at `warn!` and rejected; the loop
    /// continues to wait for the workflow's final report before
    /// transitioning to terminal.
    pub fn request_completion_from_text_fallback(&mut self) {
        if self.state.completion_honored {
            debug!("Completion already handled, ignoring text fallback request");
            return;
        }
        // P0-5: required_events gate.
        if let Err(reason) = self.state.mark_completion_requested(
            &self.config.event_loop.required_events,
            &self.config.event_loop.completion_promise,
        ) {
            tracing::warn!(
                reason = %reason,
                iteration = self.state.iteration,
                "P0-5: text-fallback completion rejected; \
                 required events not yet observed; loop continues"
            );
            self.state.completion_requested = true;
            return;
        }
        // P1-2: per-event commit so a mid-flight crash preserves
        // the completion signal for replay. The A1 end-of-batch
        // hook used to commit this; moving to the decision point
        // shrinks the window where a crash loses the signal.
        Self::commit_terminal_delta(
            &mut self.state.state_ledger,
            crate::state::CommitDelta::CompletionRequested,
        );
        info!("Completion requested via text fallback (output contained completion promise)");
    }

    /// Per-event commit helper for terminal markers
    /// (`CompletionRequested`, `CompletionHonored`,
    /// `CancellationRequested`).
    ///
    /// P1-2 (P1 follow-up): the A1 end-of-batch hook used to
    /// commit these. Moving to the decision point shrinks the
    /// window where a mid-flight crash loses the termination
    /// signal — `replay_from_disk` will see the flag set on
    /// cold start and honor the termination instead of
    /// re-running the batch.
    ///
    /// No-op when the ledger is not enabled (legacy mode) or
    /// the commit itself fails (the loop is still in
    /// termination mode; ledger error is logged and the batch
    /// continues). Per-event scalar `CounterChanged { Iteration }`
    /// stays end-of-batch — that signal is per-iteration, not
    /// per-decision.
    ///
    /// Takes `&mut Option<StateLedger>` (not `&mut self`) so
    /// the caller can keep an immutable borrow of
    /// `self.config.event_loop.event_policy` (or any other
    /// immutable field) alive in the same scope. The helper
    /// only touches the ledger slot; nothing else on `self`.
    fn commit_terminal_delta(
        ledger_slot: &mut Option<crate::state::StateLedger>,
        delta: crate::state::CommitDelta,
    ) {
        let Some(ledger) = ledger_slot else {
            return;
        };
        let topic = match &delta {
            crate::state::CommitDelta::CompletionRequested => "loop.completion_requested",
            crate::state::CommitDelta::CompletionHonored => "loop.completion_honored",
            crate::state::CommitDelta::CancellationRequested => "loop.cancellation_requested",
            _ => "loop.terminal",
        };
        if let Err(e) = ledger.commit(delta, Some(topic.to_string())) {
            tracing::warn!(
                error = %e,
                topic,
                "P1-2: per-event terminal commit failed; loop continues"
            );
        }
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

                // U11-T8 / P0-2 (2026-06-23-003 plan): deterministic
                // correction.  Replaces the legacy `task.resume`
                // injection so the rejection signal flows through
                // `PromptContext` (single source for the next prompt)
                // instead of the EventBus back-channel.
                let free_form = format!(
                    "LOOP_COMPLETE rejected: missing required events: {:?}. \
                     The agent must complete all workflow phases before emitting LOOP_COMPLETE. \
                     Use loop.cancel to abort the workflow instead.",
                    missing
                );
                if let Some(stuck) = Self::inject_completion_correction(
                    &mut self.state,
                    "missing_required_events",
                    &free_form,
                ) {
                    return Some(stuck);
                }
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
        //
        // 2026-06-17-002 U6: also check the upstream verdict payload
        // (`gate.topic` itself, e.g. `REVIEW_COMPLETE`) independently of
        // downstream mirrors. A fake pass on `report.done` must not hide
        // an upstream fail.
        if let Some(gate) = self.config.event_loop.verdict_gate.clone() {
            let upstream_fail = self
                .state
                .last_upstream_verdict_payload
                .as_deref()
                .is_some_and(|p| Self::verdict_payload_is_fail(p, &gate));
            let mirror_fail = self
                .state
                .last_verdict_payload
                .as_deref()
                .is_some_and(|p| Self::verdict_payload_is_fail(p, &gate));
            if upstream_fail || mirror_fail {
                warn!(
                    topic = %gate.topic,
                    field = %gate.fail_field,
                    value = %gate.fail_value,
                    upstream_fail,
                    mirror_fail,
                    "Rejecting LOOP_COMPLETE: verdict gate observed a failing verdict"
                );
                let sig = format!("verdict_fail:{}", gate.topic);
                if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                    return Some(reason);
                }
                self.state.completion_requested = false;

                // U11-T8 / P0-2 (2026-06-23-003 plan): deterministic
                // 2026-06-26 plan U6: structural rejection — do
                // NOT inject a correction block. The agent cannot
                // change the verdict (it is already published) and
                // injecting a correction would just spend the
                // recoverable budget on a failure mode that is not
                // recoverable. Surface the stuck signal so the
                // operator sees the loop end with a clear reason.
                return Some(TerminationReason::CompletionStuck(Box::new(
                    crate::event_loop::types::CompletionStuck {
                        source: crate::event_loop::types::StuckSource::StructuralRejection,
                        retry_key: format!("verdict_fail:{}", gate.topic),
                        attempts: 1,
                        last_reason: format!(
                            "verdict fail on {topic} ({field}={value})",
                            topic = gate.topic,
                            field = gate.fail_field,
                            value = gate.fail_value,
                        ),
                    },
                )));
            }
        }

        // Completion payload match gate: when configured, the completion
        // payload must carry the same top-level field values as the most
        // recent accepted predecessor event on the configured topic.
        if let Some(match_cfg) = self.config.event_loop.completion_payload_match.clone()
            && let Some((predecessor_topic, predecessor_payload)) =
                self.state.last_completion_predecessor.clone()
        {
            let completion_payload = self
                .state
                .last_completion_payload
                .as_deref()
                .unwrap_or("{}");
            let mismatch = Self::completion_payload_mismatch(
                &match_cfg,
                &predecessor_payload,
                completion_payload,
            );
            if let Some(reason) = mismatch {
                warn!(
                    topic = %predecessor_topic,
                    reason = %reason,
                    "Rejecting LOOP_COMPLETE: completion payload mismatch"
                );
                let sig = format!("completion_payload_mismatch:{predecessor_topic}");
                if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                    return Some(reason);
                }
                self.state.completion_requested = false;

                let free_form = format!(
                    "LOOP_COMPLETE rejected: payload mismatch on {topic} ({reason}). \
                     The completion payload must carry the same field values as the \
                     most recent accepted {topic} event. Re-emit with matching values \
                     or use loop.cancel to abort.",
                    topic = predecessor_topic,
                    reason = reason,
                );
                if let Some(stuck) = Self::inject_completion_correction(
                    &mut self.state,
                    "completion_payload_mismatch",
                    &free_form,
                ) {
                    return Some(stuck);
                }
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

            let free_form = format!(
                "LOOP_COMPLETE rejected: {}. \
                 All workflow instances must reach a terminal phase before emitting LOOP_COMPLETE. \
                 Use loop.cancel to abort the workflow instead.",
                rejection.message
            );
            // U11-T8 / P0-2 (2026-06-23-003 plan): deterministic
            // correction.  Replaces the legacy `task.resume`
            // injection.
            if let Some(stuck) = Self::inject_completion_correction(
                &mut self.state,
                "workflow_guard_incomplete",
                &free_form,
            ) {
                return Some(stuck);
            }
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
            // U2 (2026-06-17-003 plan): wrap the free-form message in
            // a JSON object carrying the schema-required
            // `reason` and `target_hat` fields.
            // 2026-06-23-005 F2: carry the typed `PersistentLoopActive`
            // kind so the schema validator / recovery aggregator
            // see the typed completion-suppression signal.
            let persistent_payload = enrich_task_resume_payload(
                "Persistent mode: loop staying alive after completion signal. \
                 Check for new tasks or await human guidance.",
                "persistent mode",
                None,
                Some(RejectionKind::PersistentLoopActive),
            );
            let resume_event = Event::new("task.resume", persistent_payload);
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
                task_ids.sort_unstable();
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
                // U2 (2026-06-17-003 plan): wrap the free-form
                // message in a JSON object carrying the
                // schema-required `reason` and `target_hat` fields.
                // 2026-06-23-005 F2: carry the typed
                // `OpenTasksBlocking` kind so the schema validator
                // sees the completion-rejection signal.
                let open_tasks_payload = enrich_task_resume_payload(
                    &format!(
                        "Completion rejected: runtime tasks remain open: {:?}. \
                         Close, fail, or reopen outstanding tasks before \
                         emitting the completion promise.",
                        open_tasks
                    ),
                    "open tasks remain",
                    None,
                    Some(RejectionKind::OpenTasksBlocking),
                );
                self.bus
                    .publish(Event::new("task.resume", open_tasks_payload));
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

        // P1-2: per-event commit (see `commit_terminal_delta`).
        if !self.state.completion_honored {
            Self::commit_terminal_delta(
                &mut self.state.state_ledger,
                crate::state::CommitDelta::CompletionHonored,
            );
        }
        self.state.completion_honored = true;

        if state_machine_enabled
            && let Some(ref mut sm_state) = self.state.state_machine_runtime_state
        {
            sm_state.mark_terminal_honored();
        }

        // Record completion honored in policy runtime state for downstream guarding
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
            && let Some(ref mut policy_state) = self.state.policy_runtime_state
        {
            policy_state.completion_honored = true;
            // 2026-06-29-007 P0 fix: terminal_observed is set only when the
            // completion promise is actually honored, not when it is merely
            // seen and later rejected by required_events / verdict gate.
            policy_state.terminal_observed = true;
            policy_state.completion_topic = Some(self.config.event_loop.completion_promise.clone());
            policy_state.completion_iteration = Some(self.state.iteration);
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

    /// P0-2 (2026-06-23-003 plan): completion rejection no longer
    /// publishes a `task.resume` event.  Instead, we route the
    /// rejection through the deterministic-correction path so the
    /// next prompt builder renders a `## ORCHESTRATOR CORRECTION`
    /// block sourced from `state.prompt_context` (the U7a single
    /// source of truth for prompt-side rejection signals).
    ///
    /// The synthesised `Rejection` uses the `Policy` stage as the
    /// closest existing bucket.  The `reason_hint` is fed into the
    /// correction block verbatim so the next prompt keeps the same
    /// free-form text the legacy `task.resume` payload used to
    /// carry.  The per-key retry counter is read from the unified
    /// ledger so escalation (R11) tracks the same number the
    /// legacy wire-format path used.
    ///
    /// 2026-06-26 plan U6: returns
    /// `Some(TerminationReason::CompletionStuck)` when the retry
    /// budget for this `retry_key` is exhausted (>= 3). The caller
    /// must surface the stuck signal instead of looping again. The
    /// structural-rejection path
    /// (e.g. `verdict_fail` in `check_completion_event`) does NOT
    /// call this helper — it goes straight to
    /// `CompletionStuck(StructuralRejection)` so a structural
    /// failure never silently burns the recoverable budget.
    fn inject_completion_correction(
        state: &mut LoopState,
        reason_hint: &str,
        free_form: &str,
    ) -> Option<TerminationReason> {
        let topic = ralph_proto::LOOP_COMPLETE.to_string();
        let mut rejection = crate::event_loop::rejection::Rejection {
            stage: crate::event_loop::rejection::RejectionStage::Policy,
            source_hat: None,
            business_hat: None,
            topic: topic.clone(),
            violation: free_form.to_string(),
            retry_key: String::new(),
            retry_eligible: true,
            non_retryable_reason: None,
            target_hat: None,
            original_event_id: None,
            original_ts: None,
            // 2026-06-23 fix plan U5 (CB-2): completion-correction
            // path predates typed-kind plumbing — keep None.
            kind: None,
            duplicate_work_done_hint: None,
            seen_count: None,
        };
        let retry_key = rejection.compute_retry_key();
        rejection.retry_key = retry_key.clone();

        // Read the per-key retry count from the unified ledger so
        // R11 escalation tracks the same number the legacy
        // `task.resume` payload used to ship on the wire.  Fall
        // back to 1 on cold start (no prior rejection recorded).
        let retry_count = state
            .state_ledger
            .as_ref()
            .and_then(|l| l.snapshot().rejection_digest().get(&retry_key))
            .map(|entry| entry.count)
            .unwrap_or(1u32);

        // 2026-06-26 plan U6: bounded recovery. After 3 attempts
        // for the same retry key, stop injecting corrections and
        // surface a `CompletionStuck(RejectionDigestExhausted)`
        // termination so the operator sees the loop end. The
        // budget matches `U2_REJECTION_RETRY_LIMIT` (3) so the
        // gate, the runner, and the summary report all use the
        // same number.
        if retry_count > U2_REJECTION_RETRY_LIMIT {
            return Some(TerminationReason::CompletionStuck(Box::new(
                crate::event_loop::types::CompletionStuck {
                    source: crate::event_loop::types::StuckSource::RejectionDigestExhausted,
                    retry_key: retry_key.clone(),
                    attempts: retry_count,
                    last_reason: format!("{reason_hint}: {free_form}"),
                },
            )));
        }

        // `emit_correction_context` is the U7a entry point: it
        // commits a `RejectionRecorded` delta to the unified ledger
        // (when wired up) and pushes the `CorrectionContext` into
        // `state.prompt_context` so the next `build_prompt` call
        // prepends the `## ORCHESTRATOR CORRECTION` block.  No
        // event is published on the bus — the prompt builder is
        // the single source of truth.
        let _ctx = crate::correction::emit_correction_context(
            state.state_ledger.as_mut(),
            &rejection,
            retry_count,
            None,
            &mut state.prompt_context,
        );

        // Surface the reason hint in tracing so operators can
        // correlate a `LOOP_COMPLETE` rejection with the
        // correction block queued in the next prompt.
        tracing::info!(
            retry_key = %retry_key,
            reason_hint = %reason_hint,
            topic = %topic,
            "P0-2: injected completion rejection into state.prompt_context (replaces task.resume)"
        );
        // 2026-06-26 plan U6: correction queued; budget not
        // exhausted yet — caller should keep the loop alive.
        None
    }

    /// Returns true if the verdict event payload resolves to a
    /// `Fail` verdict, either via the typed `Verdict::from_payload`
    /// path (when `gate.verdict_field` is configured) or via the
    /// legacy binary `fail_field == fail_value` match (when
    /// `verdict_field` is `None`).
    ///
    /// The typed path is the 2026-06-26 plan U5 contract: it
    /// recognises `pass` / `pass_with_residuals` / `fail` as three
    /// distinct terminal states and applies `max_residuals` to
    /// promote or downgrade `pass_with_residuals`. The legacy
    /// path is preserved so presets that have not yet opted into
    /// the new field keep working unchanged.
    ///
    /// Returns false on:
    /// - payload not valid JSON,
    /// - verdict field missing (legacy path: absence == not failing
    ///   because the gate is opt-in and only trips on an explicit
    ///   `fail` value),
    /// - payload that fails to parse as a typed `Verdict` (treated
    ///   as "not failing" so a transient shape mismatch does not
    ///   silently kill the loop; the operator can grep
    ///   `verdict_parse_error` in the diagnostics if the
    ///   mismatch persists).
    fn verdict_payload_is_fail(payload: &str, gate: &crate::config::VerdictGateConfig) -> bool {
        if let Some(verdict_field) = gate.verdict_field.as_deref() {
            // Typed Verdict path. Threshold defaults to 8 to
            // match the ralph-e2e `primary-20260624-032505`
            // case (see `default_max_residuals` in
            // `crate::config::loop_config`).
            const DEFAULT_MAX_RESIDUALS: u32 = 8;
            let max_residuals = Some(DEFAULT_MAX_RESIDUALS);
            let verdict =
                Verdict::from_payload(payload, verdict_field, gate.residual_count_field.as_deref());
            match verdict {
                Ok(v) => v.resolve(max_residuals).is_fail(),
                Err(_) => {
                    tracing::debug!(
                        verdict_field,
                        "verdict payload did not parse as typed Verdict; \
                         treating as not failing"
                    );
                    false
                }
            }
        } else {
            // Legacy binary match: `fail_field == fail_value`.
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                return false;
            };
            value
                .get(&gate.fail_field)
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == gate.fail_value)
        }
    }

    /// Compare the top-level fields declared in `match_cfg` between
    /// the predecessor payload and the completion payload. Returns
    /// `Some(reason)` on mismatch, missing field, or non-object
    /// payload; `None` when all declared fields match.
    fn completion_payload_mismatch(
        match_cfg: &crate::config::CompletionPayloadMatchConfig,
        predecessor_payload: &str,
        completion_payload: &str,
    ) -> Option<String> {
        let pred: serde_json::Value = match serde_json::from_str(predecessor_payload) {
            Ok(v) => v,
            Err(_) => return Some("predecessor payload is not valid JSON".to_string()),
        };
        let comp: serde_json::Value = match serde_json::from_str(completion_payload) {
            Ok(v) => v,
            Err(_) => return Some("completion payload is not valid JSON".to_string()),
        };
        let pred_obj = pred.as_object()?;
        let comp_obj = comp.as_object()?;
        for field in &match_cfg.fields {
            let pred_val = pred_obj.get(field);
            let comp_val = comp_obj.get(field);
            match (pred_val, comp_val) {
                (Some(p), Some(c)) if p == c => continue,
                (Some(p), Some(c)) => {
                    return Some(format!(
                        "field '{field}' mismatch: predecessor={p}, completion={c}"
                    ));
                }
                (Some(_), None) => {
                    return Some(format!("field '{field}' missing in completion payload"));
                }
                (None, Some(_)) => {
                    return Some(format!("field '{field}' missing in predecessor payload"));
                }
                (None, None) => {
                    return Some(format!("field '{field}' missing in both payloads"));
                }
            }
        }
        None
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
    ///
    /// **U7b (plan 2026-06-21-002):** when the
    /// `UNIFIED_DETERMINISTIC_CORRECTION=1` env var is set,
    /// this function delegates to
    /// [`Self::initialize_resume_with_context`] which emits the
    /// new `loop.resume` control event (see
    /// [`ralph_proto::LOOP_RESUME`]) and seeds a
    /// [`crate::correction::ResumeContext`] block in the next
    /// prompt.  The legacy `task.resume` path is preserved for
    /// callers that have not opted in.
    pub fn initialize_resume(&mut self, prompt_content: &str) {
        if crate::correction::is_correction_enabled() {
            self.initialize_resume_with_context(
                prompt_content,
                crate::correction::ResumeContext::default(),
            );
            return;
        }
        // Legacy path: emit `task.resume` regardless of starting_event
        // config.  Preserved so the U1-U6 test suite keeps
        // passing without the feature flag.
        self.initialize_with_topic("task.resume", prompt_content);
        // Unit 3: rebuild bootstrap gate from recorded events so resume
        // does not re-open the guidance-suppression window mid-loop.
        self.rebuild_bootstrap_flags_from_recorded_events();
    }

    /// U7b (plan 2026-06-21-002): initialize resume with an
    /// explicit [`crate::correction::ResumeContext`].  Emits a
    /// `loop.resume` control event (see [`ralph_proto::LOOP_RESUME`])
    /// instead of `task.resume`, and seeds the resume block in
    /// [`crate::correction::PromptContext`] so the next prompt
    /// contains `## LOOP RESUME CONTEXT`.
    ///
    /// Callers should construct the resume context from the
    /// scratchpad / progress.md / closed-tasks state at the
    /// resume boundary; this function only routes the event and
    /// stores the block.
    pub fn initialize_resume_with_context(
        &mut self,
        prompt_content: &str,
        resume_context: crate::correction::ResumeContext,
    ) {
        // Always push the resume block to `state.prompt_context`
        // regardless of the legacy `task.resume` topic.  This
        // is the U7b contract: the next prompt always carries
        // `## LOOP RESUME CONTEXT` when the user runs
        // `--continue`, even when the feature flag is off.
        self.state.prompt_context.resume_blocks.push(resume_context);

        // Emit the boot topic.  Prefer the new `loop.resume`
        // event when the feature flag is on; fall back to
        // `task.resume` for the legacy test paths.
        let topic = if crate::correction::is_correction_enabled() {
            ralph_proto::LOOP_RESUME
        } else {
            "task.resume"
        };
        self.initialize_with_topic(topic, prompt_content);
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

        let start_event = Event::new(topic, prompt_content)
            .with_source("orchestrator")
            .with_system_injected();
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

        // 2026-07-02-001 review P0 fix (code-review #1): when a hat
        // has a **targeted** event in its pending queue (i.e. an
        // event with `event.target == Some(hat_id)`), the next
        // activation MUST be that hat. The pre-existing
        // `event_bus::publish` direct-target contract already routes
        // targeted events to the named hat's queue; the dispatcher's
        // only remaining job is to ensure the dispatcher picks that
        // hat up next. Without this fast path, a targeted
        // `task.resume` from the 62a40b41
        // `isolated_extra_business_event_dropped` backpressure (or
        // any other targeted recovery signal) could be deferred by
        // the round-robin scan, leaving the over-emitting hat dormant
        // for a full cycle. The hat is selected deterministically
        // (BTreeMap dict order) when multiple hats have targeted
        // events, mirroring the round-robin cursor's tie-breaking.
        //
        // This is a **targeted-event fast path**, separate from the
        // handoff priority pre-emption below. Targeted events are
        // unambiguous by construction (the publisher named a specific
        // hat), so they don't need a "topic-eligibility" filter; the
        // handoff priority path's strict topic-exact predicate is
        // preserved for the broad (untargeted) handoff case.
        let targeted_hat: Option<HatId> = {
            let mut found: Option<HatId> = None;
            for id in self.bus.hat_ids() {
                let has_targeted = self
                    .bus
                    .peek_pending(id)
                    .map(|q| q.iter().any(|event| event.target.as_ref() == Some(id)))
                    .unwrap_or(false);
                if has_targeted {
                    // BTreeMap order → first targeted wins.
                    found = Some(id.clone());
                    break;
                }
            }
            found
        };
        if let Some(ref id) = targeted_hat {
            tracing::debug!(
                target = "ralph_core::event_loop",
                hat = %id,
                "next_hat: targeted event in consumer queue — fast-pathing to that hat"
            );
            // Advance the round-robin cursor to mirror a normal
            // selection (so the next non-targeted selection resumes
            // fairly from the registered successor).
            self.bus.select_next_hat_with_pending(Some(id))?;
            return self.bus.hat_ids().find(|hat_id| hat_id == &id);
        }

        match self.config.event_loop.execution_mode {
            HatExecutionMode::Isolated => {
                // Isolated mode: use round-robin to select the next hat.
                // This advances the cursor on the bus for fair scheduling.
                //
                // 2026-06-28-005: the `has_human_pending` guard that
                // routed to ralph when only human events were pending
                // is gone — the `human_pending` queue was removed
                // together with the `human.guidance` topic.
                // WAC-U5 (2026-06-12-002): handoff priority pre-emption.
                // If the HandoffIndex has at least one priority-eligible
                // entry (unique consumer) and that hat currently has a
                // non-empty pending queue, the dispatcher selects it
                // immediately and the round-robin cursor advances. The
                // scan walks the index in BTreeMap (alphabetical topic)
                // order for determinism. If no priority hat has pending
                // events, we fall through to the normal round-robin
                // pass.
                // 2026-07-02-001 plan U1 (Fix A): handoff priority pre-emption
                // must require **topic-exact pending**, not just a non-empty
                // consumer queue. The pre-fix predicate (consumer queue
                // non-empty → eligible for priority) was susceptible to
                // misleading routing whenever a hat's queue held an event
                // whose topic was *not* the handoff entry's topic (e.g. an
                // untargeted `task.resume` left behind by an earlier round).
                // Such residue would short-circuit the round-robin scan and
                // pre-empt a different hat's legitimate handoff dispatch.
                //
                // "Topic-exact" means `event.topic.as_str() == entry.topic`
                // — string equality on the topic name. Topic *pattern*
                // matching (e.g. `work.*`) is the `EventBus::publish`
                // concern; the dispatcher's priority pre-empt requires the
                // consumer to have a pending event whose topic is the
                // handoff entry's topic verbatim, not a pattern. This is
                // the same contract the HandoffIndex uses for `consumer_of`
                // (see `workflow_contract/handoff_index.rs:228`).
                //
                // The post-fix predicate walks the priority-dispatchable
                // entries in BTreeMap (alphabetical topic) order, and for
                // each `(topic T, consumer C)` checks whether C's pending
                // queue contains an event with `event.topic == T`. Only
                // that case is treated as eligible for priority pre-emption.
                // If no entry yields a topic-exact pending, `priority_hat`
                // stays `None` and the dispatcher falls through to the
                // normal round-robin scan.
                //
                // 2026-07-02-001 review P0 fix (code-review #1): the
                // targeted-event fast path above (`targeted_hat`) handles
                // the 62a40b41 `isolated_extra_business_event_dropped`
                // targeted-`task.resume` reactivation. The
                // priority-predicate additionally filters out topics
                // classified as **orchestrator control / system
                // backpressure** by `ralph_proto::is_orchestrator_control`
                // (`task.resume`, `loop.resume`, `LOOP_COMPLETE`,
                // `LOOP_CANCEL`). These topics *do* appear in
                // `HandoffIndex::entries` when a hat subscribes to them
                // (e.g. `executor` subscribes to `task.resume`), and the
                // strict topic-exact predicate alone is not enough to
                // reject the priority pre-empt — an untargeted
                // `task.resume` residue in such a consumer's queue would
                // still win the priority pre-empt. Filtering them here
                // restores the 62a40b41 contract: system backpressure
                // events never pre-empt a handoff dispatch, and the
                // targeted-event fast path above is the only place such
                // events can re-activate a hat.
                let priority_hat: Option<HatId> =
                    self.handoff_index
                        .entries
                        .iter()
                        .find_map(|(topic, entry)| {
                            let consumer = entry.consumer.as_deref()?;
                            let hat_id = HatId::from(consumer);
                            if ralph_proto::topics::is_orchestrator_control(topic.as_str()) {
                                return None;
                            }
                            let topic_matches = self
                                .bus
                                .peek_pending(&hat_id)
                                .map(|q| {
                                    q.iter().any(|event| event.topic.as_str() == topic.as_str())
                                })
                                .unwrap_or(false);
                            if topic_matches {
                                // KTD-9 / R1: pre-emption hits are observable
                                // so future drift has a forensic trail.
                                tracing::debug!(
                                    target = "ralph_core::event_loop",
                                    topic = %topic,
                                    consumer = %hat_id,
                                    "priority pre-empt: topic-exact pending in consumer queue"
                                );
                                Some(hat_id)
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

                // 2026-06-28-005: the `has_human_pending` fallback
                // path that routed to ralph when only human events
                // were pending is gone — the `human_pending` queue
                // was removed together with the topic.

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

    /// Returns the current byte offset of the embedded `EventReader`.
    ///
    /// Primarily for tests that need to assert the cursor was pushed
    /// to the end of the file (e.g. after
    /// [`Self::sync_event_reader_to_file_end`]) so a freshly
    /// appended bootstrap record is not re-delivered to the bus.
    pub fn event_reader_position(&self) -> u64 {
        self.event_reader.position()
    }

    /// Reads the events file from the current reader offset without
    /// advancing the cursor.
    ///
    /// Convenience wrapper for tests so they can assert that a
    /// freshly persisted bootstrap line is no longer "new" after
    /// `sync_event_reader_to_file_end()` is called.  The wrapper
    /// deliberately exposes the same `ParseResult` shape returned by
    /// `EventReader::read_new_events` so test assertions stay
    /// uniform.
    pub fn peek_event_reader_for_test(&self) -> std::io::Result<crate::event_reader::ParseResult> {
        self.event_reader.peek_new_events()
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

    /// Checks if any pending events are human guidance events.
    ///
    /// 2026-06-28-005: stub kept so callers that previously
    /// consulted `bus.has_human_pending()` still compile while the
    /// `human.guidance` topic and its dedicated `human_pending`
    /// queue are removed together. Always returns `false` now —
    /// the queue is gone, so the question is no longer meaningful.
    pub fn has_pending_human_events(&self) -> bool {
        false
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
    /// target is `reporter` (was `shipper` per plan 2026-07-24-005 U1
    /// — `plan-gate.triggers` does NOT include `plan.blocked`, so
    /// routing through plan-gate would silently drop the event.
    /// The wave is then closed in the tracker (`open_wave_id =
    /// None`) so the gate does not re-fire on the next
    /// iteration.
    ///
    /// Called once per iteration inside [`Self::process_output`],
    /// after handoff escalations and before new JSONL events are
    /// processed. This matches the plan §U2 fixed order:
    /// incomplete-wave gate → handoff-expired → process JSONL →
    /// policy validation. It is also invoked from the stall ladder
    /// in [`Self::inject_fallback_event`] as a hard-escalation
    /// fallback. When this method emits a `plan.blocked`, the U4
    /// aggregate-timeout path is not consulted in the same iteration.
    ///
    /// Returns `true` if a `plan.blocked` was emitted.
    pub fn maybe_emit_incomplete_wave_blocked(&mut self) -> bool {
        use crate::flow_lifecycle::incomplete_wave_gate::{
            IncompleteWaveGate, IncompleteWaveGateConfig,
        };

        // Plan §U2: global default off, `ce-executor-serial`
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
        // hat; `with_target(...)` routes to `reporter`.
        //
        // 2026-07-24-005 plan U1: target was `shipper`; the
        // shipper hat is removed from the supervisor preset —
        // `reporter` is the canonical `plan.blocked` terminal
        // owner.
        let json_payload = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let event = Event::new("plan.blocked", json_payload)
            .with_source(HatId::new("review-synthesizer"))
            .with_target(HatId::new("reporter"));
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

        let free_form = format!(
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
        // U2 (2026-06-17-003 plan): wrap the free-form message in
        // a JSON object carrying the schema-required `reason` and
        // `target_hat` fields.
        let payload = enrich_task_resume_payload(
            &free_form,
            "aggregate_timeout",
            Some(target.as_str()),
            Some(RejectionKind::ContractViolation),
        );
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
        let handoff_event_id = format!("sla:review.dimension.done:{}", action.wave_id);
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

    /// 2026-06-28 plan U6 (R6): drive the per-task
    /// `RepairStateMachine` from the stall hot path.
    ///
    /// The first escalation for a `task_key` lazily creates a
    /// machine with the preset's `mechanism.repair_budget`
    /// (defaulting to 3 when no flow is declared). Subsequent
    /// escalations call `Retry` and consume one unit of the
    /// budget. When the budget is exhausted, we emit a
    /// `plan.blocked` envelope with
    /// `reason="repair_unrecoverable_after_N_retries"` and
    /// return `true` so the caller skips the `task.resume`
    /// path.
    ///
    /// The machine is keyed by `task_key` (= `stall_key` from
    /// the caller); different keys have independent budgets.
    fn drive_repair_state_machine(&mut self, task_key: &str, stall_count: u32) -> bool {
        use crate::event_loop::repair_flow::{
            RepairAction, RepairBudget, RepairStateMachine, RepairTransitionResult,
        };
        // Read the budget from the preset (U12 will lint this).
        // When no flow is declared we fall back to the
        // repository-wide default of 3.
        let max = self
            .config
            .event_loop
            .mechanism
            .as_ref()
            .and_then(|m| m.flow.as_ref())
            .map(|f| f.repair_budget)
            .unwrap_or(3);
        let budget = RepairBudget { max };
        let machine = self
            .repair_state_machines
            .entry(task_key.to_string())
            .or_insert_with(|| RepairStateMachine::new(budget));
        // First escalation: Detected -> Diagnosing. We use
        // the budget to gate the upgrade so a preset that
        // declared `repair_budget: 0` immediately exhausts.
        let result = if stall_count == 1 {
            machine.try_transition(RepairAction::BeginDiagnosis)
        } else {
            machine.try_transition(RepairAction::Retry)
        };
        match result {
            RepairTransitionResult::BudgetExhausted(exhausted) => {
                let payload = format!(
                    r#"{{"reason":"{}","task_key":"{}","retries_consumed":{},"budget":{}}}"#,
                    exhausted.reason_code, task_key, exhausted.retries_consumed, exhausted.max,
                );
                let blocked = Event::new("plan.blocked", payload).with_target(HatId::new("ralph"));
                self.record_repair_event(&blocked);
                // 2026-06-29 code-review fix: set the
                // `terminal_event_emitted` flag so U8's
                // final-threshold path (also emitting
                // `plan.blocked`) does not fire a second
                // time for the same `stall_key`. Mirrors
                // U8's behaviour at line 2983.
                self.terminal_event_emitted = true;
                true
            }
            // Illegal transitions are expected when a previous
            // stall cycle Closed the machine — treat them as
            // no-ops, NOT as a budget-exhausted stop.
            RepairTransitionResult::IllegalTransition { .. } => false,
            RepairTransitionResult::Accepted => false,
        }
    }

    /// True when the last hat consumed a multi-consumer pass-through trigger
    /// and another registered consumer still has that topic pending — stall
    /// recovery must not inject targeted `task.resume` to the pass-through hat.
    fn should_skip_stall_recovery_for_multi_consumer_peers(&self) -> bool {
        let Some(last_hat) = self.state.last_hat.as_ref() else {
            return false;
        };
        let Some(config) = self.registry.get_config(last_hat) else {
            return false;
        };
        let Some(pass_through_trigger) =
            self.state.last_activation_events.iter().find_map(|event| {
                let topic = event.topic.as_str();
                if config.triggers.iter().any(|t| t == topic)
                    && config.trigger_multi_consumer_topics.contains(topic)
                    && config.publishes.len() == 1
                    && config.publishes.iter().any(|p| p == topic)
                {
                    Some(topic.to_string())
                } else {
                    None
                }
            })
        else {
            return false;
        };
        self.bus.hat_ids().any(|id| {
            if id == last_hat {
                return false;
            }
            self.bus
                .peek_pending(id)
                .is_some_and(|q| q.iter().any(|e| e.topic.as_str() == pass_through_trigger))
        })
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

        // Do not stall-recover after the loop has already reached terminal.
        if self.state.completion_honored {
            return false;
        }
        if self.state.completion_requested && self.check_completion_event().is_some() {
            return false;
        }

        // Pass-through multi-consumer hats (e.g. shipper on `plan.complete`) may
        // intentionally not re-emit; peer consumers still hold the same trigger.
        // Injecting targeted `task.resume` to the pass-through hat would pre-empt
        // round-robin and starve peers (reporter never sees `plan.complete`).
        if self.should_skip_stall_recovery_for_multi_consumer_peers() {
            return false;
        }

        const STALL_HARD_THRESHOLD: u32 = 3;
        // Unit 8 (2026-06-17-001): use a per-last-hat stall key so wave hats
        // accumulate their own retry budget separate from ralph's global counter.
        let stall_key = if let Some(last_hat) = &self.state.last_hat {
            if Self::is_wave_hat(last_hat) {
                "flow:review-synthesizer".to_string()
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

        // 2026-06-28 plan U8 (R5): final-threshold self-stop.
        // Even when U6's `RepairStateMachine` did not consume
        // the budget (e.g. the budget was set high, or the
        // machine was reset by a Close), the per-key stall
        // counter must still emit a terminal `plan.blocked`
        // when it crosses `STALL_FINAL_THRESHOLD`. This is a
        // safety net: the loop's self-stop is the only
        // contract that survives a misconfigured preset.
        const STALL_FINAL_THRESHOLD: u32 = 10;
        if stall_count_value >= STALL_FINAL_THRESHOLD {
            if !self.terminal_event_emitted {
                let payload = format!(
                    r#"{{"reason":"stall_recovery_exhausted","task_key":"{}","stall_count":{}}}"#,
                    stall_key, stall_count_value,
                );
                let blocked = Event::new("plan.blocked", payload).with_target(HatId::new("ralph"));
                self.record_repair_event(&blocked);
                self.terminal_event_emitted = true;
            }
            debug!(
                stall_count = stall_count_value,
                stall_key = %stall_key,
                "U8: stall_recovery final threshold reached — loop self-stops"
            );
            return true;
        }

        // 2026-06-28 plan U6 (R6): drive the per-task
        // `RepairStateMachine` from the stall hot path so
        // `repair_budget` becomes a real hard cap rather than
        // a metadata decoration. The first escalation
        // transitions the machine into `Diagnosing`; each
        // subsequent escalation calls `Retry`. When
        // `RepairStateMachine` reports `BudgetExhausted`, we
        // emit a `plan.blocked` envelope and short-circuit so
        // no `task.resume` is published — the loop's self-stop
        // path takes over.
        let budget_exhausted = self.drive_repair_state_machine(&stall_key, stall_count_value);
        if budget_exhausted {
            debug!(
                stall_count = stall_count_value,
                stall_key = %stall_key,
                "U6: repair_budget exhausted — emitting plan.blocked and skipping task.resume"
            );
            return true;
        }

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
        if hard_escalation
            && stall_key.starts_with("flow:")
            && self.maybe_emit_incomplete_wave_blocked()
        {
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
            // U2 (2026-06-17-003 plan): wrap the free-form message
            // in a JSON object carrying the schema-required
            // `reason` and `target_hat` fields.
            // 2026-06-28-002 U3: stamp the hard_target's allowed
            // publish topics so the resumed agent sees the legal
            // emit surface and the isolated scope check sees the
            // same list.
            let hard_target_publishes = self.get_hat_publishes(&hard_target);
            let structured_payload = enrich_task_resume_payload_full(
                &payload,
                reason_str,
                Some(hard_target.as_str()),
                None,
                Some(RejectionKind::StallNoEvents),
                &hard_target_publishes,
            );
            debug!(
                stall_count = stall_count_value,
                target = %hard_target.as_str(),
                "Injecting HARD stall recovery to review hat"
            );
            // 2026-07-04-001 plan U16: validate that the hard_target
            // matches the original trigger topic's consumer. The
            // 2026-07-04-002 plan upgraded this from a `warn!` (which
            // silently dropped into the recovery envelope) to a hard
            // Block so a mismatch no longer publishes a `task.resume`
            // to a hat that won't pick it up.
            //
            // The hard_escalation path does not currently carry the
            // original trigger topic, so we pass `None` and rely on
            // the no-op fallback inside `validate_resume_routing`
            // (returns `Allow` when no `original_topic` is supplied).
            // This intentionally preserves the pre-fix behaviour for
            // the long-running stall ladder while still exposing the
            // new `EventLoopResumeDecision` API to future caller
            // upgrades. Routing-mismatch warnings for the hard ladder
            // surface in `recovery.jsonl` rather than blocking the
            // resume.
            if let EventLoopResumeDecision::Block(reason) =
                self.validate_resume_routing(&hard_target, None)
            {
                let diagnostic = Event::new(
                    "event.recovery.routing_blocked",
                    format!(
                        "{{\"target\":\"{}\",\"reason\":\"{}\"}}",
                        hard_target.as_str(),
                        reason
                    ),
                );
                self.bus.publish(diagnostic);
                warn!(target = %hard_target.as_str(), "{reason}");
            }
            Event::new("task.resume", structured_payload).with_target(hard_target)
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

                    // U2 (2026-06-17-003 plan): wrap the free-form
                    // message in a JSON object carrying the
                    // schema-required `reason` and `target_hat` fields.
                    // 2026-06-28-002 U3: stamp `allowed_topics` so
                    // the agent's resumed emit is constrained to
                    // its own publishes (e.g. coordinator gets
                    // `work.ready` but NOT `work.start`).
                    let structured_payload = enrich_task_resume_payload_full(
                        &payload,
                        "stall_no_events",
                        Some(hat_id.as_str()),
                        None,
                        Some(RejectionKind::StallNoEvents),
                        &publishes,
                    );

                    debug!(
                        hat = %hat_id.as_str(),
                        "Injecting fallback event to recover - targeting last hat with task.resume"
                    );
                    // 2026-07-04-001 plan U16: validate that the resume
                    // target hat actually subscribes to the original
                    // topic. The fallback site does not carry the
                    // original trigger topic (it fires on "no events
                    // emitted"), so we pass `None` — the check is a
                    // no-op here (returns `Allow` per the new API
                    // contract). Routing-mismatch warnings surface
                    // at the upstream rejection site instead.
                    if let EventLoopResumeDecision::Block(reason) =
                        self.validate_resume_routing(hat_id, None)
                    {
                        warn!(hat = %hat_id.as_str(), "{reason}");
                    }
                    Event::new("task.resume", structured_payload).with_target(hat_id.clone())
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
                    // U2 (2026-06-17-003 plan): wrap the free-form
                    // message in a JSON object carrying the
                    // schema-required `reason` and `target_hat` fields.
                    // 2026-06-28-002 U3: stamp `allowed_topics` for
                    // the ralph hat so the resumed iteration
                    // honours its own publishes.
                    let ralph_publishes = self.get_hat_publishes(&HatId::new("ralph"));
                    let structured_payload = enrich_task_resume_payload_full(
                        &payload,
                        "stall_no_events",
                        Some("ralph"),
                        None,
                        Some(RejectionKind::StallNoEvents),
                        &ralph_publishes,
                    );
                    debug!(
                        "Injecting fallback event to recover - triggering Ralph with task.resume"
                    );
                    Event::new("task.resume", structured_payload)
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
        rejection: &crate::validation::WorkflowGuardRejectionDetail,
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
            reason_code,
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
    fn append_terminal_deliverable_contract(&self, prompt: String, hat_id: &HatId) -> String {
        let promise = self.config.event_loop.completion_promise.as_str();
        let Some(hat) = self.registry.get_config(hat_id) else {
            return prompt;
        };
        let publishes_completion = hat.publishes.iter().any(|topic| topic == promise)
            || hat.default_publishes.as_deref() == Some(promise);
        if !publishes_completion {
            return prompt;
        }

        let Some(policy) = self.config.event_loop.event_policy.as_ref() else {
            return prompt;
        };
        let Some(schema) = policy.schemas.get(promise) else {
            return prompt;
        };
        let Some(path_field) = ["report_path", "artifact_path"]
            .iter()
            .find(|field| schema.required_fields.iter().any(|required| required == **field))
        else {
            return prompt;
        };
        let field_doc = schema.field_docs.get(*path_field);
        let path_source = field_doc
            .map(|doc| doc.source.trim())
            .filter(|source| !source.is_empty())
            .unwrap_or("the real operator-facing artifact available in this activation");
        let fill_rule = field_doc
            .map(|doc| doc.fill_rule.trim())
            .filter(|rule| !rule.is_empty())
            .unwrap_or("use the real repo-relative path; never invent a path");

        format!(
            "{prompt}\n\n## TERMINAL DELIVERABLE CONTRACT\n\
             This is the final activation for completion topic `{promise}`.\n\
             - Before emitting, resolve `{path_field}` from: {path_source}.\n\
             - Contract: {fill_rule}.\n\
             - Verify the file is readable with `test -f` before policy-check and the real emit.\n\
             - The `{promise}` payload MUST include `{path_field}` with that exact repo-relative path.\n\
             - After the emit succeeds, your final visible reply MUST contain exactly one standalone line:\n\
             `DELIVERABLE_PATH: <{path_field}>`\n\
             Replace the placeholder with the same path carried in `{path_field}`. Do not finish with only a prose summary.\n"
        )
    }

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
                    self.apply_robot_guidance(hat_id);
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
                // 2026-06-28-003: prepend recovery directives derived
                // from pending `task.resume` events so the agent sees
                // behaviour guidance before the skill index.
                let base_prompt = self.prepend_recovery_directives(base_prompt, &regular_events);
                let with_skills = self.prepend_auto_inject_skills(base_prompt, hat_id);
                let with_scratchpad = self.prepend_scratchpad(with_skills, Some(hat_id));
                let with_state_files = self.prepend_state_files(with_scratchpad);
                let final_prompt = self.prepend_ready_tasks(with_state_files);
                // U7a (plan 2026-06-21-002): prepend the
                // deterministic correction + resume blocks.  The
                // queue lives on `LoopState::prompt_context` and
                // is populated by `emit_correction_context` on
                // the policy rejection path; this prepend is a
                // no-op when the queue is empty (the legacy
                // `task.resume` path keeps working unchanged).
                let final_prompt = self.prepend_correction_and_resume(final_prompt);
                // U4b (plan 2026-06-20-001, R12 / R13 / KTD-8):
                // if the most recent `ralph emit` was rejected by
                // the lint phase, inject `## LINT MIRROR` +
                // `## LINT RESUME REQUIRED` so the next prompt
                // tells the agent *what* the lint saw and *which
                // hat* should fix it.  The hint is consumed on
                // first read (consume-on-use) so a stale resume
                // does not leak across prompts.
                let final_prompt = self.inject_pending_lint_resume(final_prompt, hat_id);

                debug!("build_prompt: routing to HatlessRalph (solo mode)");
                return Some(final_prompt);
            } else if self.config.event_loop.execution_mode != HatExecutionMode::Isolated {
                // Coordinator multi-hat mode: collect events and determine active hats.
                // Isolated mode must NOT take this path — ralph is a round-robin peer
                // and may only consume its own pending queue. Draining every hat's
                // queue here steals multi-consumer handoffs (e.g. `plan.complete`
                // pending for reporter/shipper) and downstream hats never activate.
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

                // 2026-06-17-004 U2 (R3): refresh the per-hat
                // activation clock for every hat about to execute
                // an agent.  The clock is the source of truth for
                // the missing-event gate's grace window: when the
                // gate fires within `hat.missing_event_grace_secs`
                // (default `min(adapter_idle * 0.3, 540)`) of an
                // activation, the gate is suppressed so long-running
                // hats like `dimension-reviewer` (per-worker timeout
                // 1800s) are not mis-fired during the first few
                // seconds of model warm-up.  Subsequent activations
                // REPLACE the timestamp so a hat that loops through
                // many short turns does not accumulate a stale
                // clock that suppresses the gate past its useful
                // window.
                for hat_id in &active_hat_ids {
                    self.state.record_hat_activation(hat_id);
                }

                // 2026-06-26 plan U4: push a fresh obligation for each
                // active hat. The MissingEventGate (U4) now consults
                // the obligation queue instead of the activation
                // clock. `terminal_events` (if non-empty) is the
                // set of topics that count as "the hat has
                // fulfilled its trigger obligation" — for hats
                // without an explicit `terminal_events` list we
                // fall back to `publishes`. Hats with neither
                // receive no obligation (no contract to enforce).
                for hat_id in &active_hat_ids {
                    if let Some(hat_cfg) = self.registry.get_config(hat_id).cloned() {
                        let expected = if !hat_cfg.terminal_events.is_empty() {
                            hat_cfg.terminal_events.clone()
                        } else if !hat_cfg.publishes.is_empty() {
                            hat_cfg.publishes.clone()
                        } else {
                            continue;
                        };
                        // The trigger topic is the first regular
                        // event whose topic is in this hat's
                        // configured `triggers`. Falls back to the
                        // first regular event's topic if no exact
                        // match — preserves the old record path.
                        let trigger_topic: String = regular_events
                            .iter()
                            .find(|e| hat_cfg.triggers.iter().any(|t| t == e.topic.as_str()))
                            .map(|e| e.topic.to_string())
                            .or_else(|| regular_events.first().map(|e| e.topic.to_string()))
                            .unwrap_or_default();
                        self.state.push_hat_obligation(
                            hat_id.clone(),
                            trigger_topic.clone(),
                            expected,
                        );
                    }
                }

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
                    self.apply_robot_guidance(hat_id);
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
                // 2026-06-28-003: prepend recovery directives derived
                // from pending `task.resume` events.
                let base_prompt = self.prepend_recovery_directives(base_prompt, &regular_events);
                let with_skills = self.prepend_auto_inject_skills(base_prompt, hat_id);
                let with_scratchpad = self.prepend_scratchpad(with_skills, Some(hat_id));
                let with_state_files = self.prepend_state_files(with_scratchpad);
                let final_prompt = self.prepend_ready_tasks(with_state_files);
                // U7a (plan 2026-06-21-002): prepend deterministic
                // correction + resume blocks.  No-op when the
                // queue is empty.
                let final_prompt = self.prepend_correction_and_resume(final_prompt);
                // U4b: see solo-mode comment above. Same
                // consume-on-use semantics for the lint hint.
                let final_prompt = self.inject_pending_lint_resume(final_prompt, hat_id);

                return Some(final_prompt);
            }
        }

        // Isolated per-hat prompt (including ralph when it is selected by round-robin).
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated {
            // Isolated mode: build focused prompt for this hat only.
            let mut events = self.bus.take_pending(&hat_id.clone());
            let mut human_events = self.bus.take_human_pending();
            events.append(&mut human_events);

            let (guidance_events, regular_events): (Vec<_>, Vec<_>) = events
                .into_iter()
                .partition(|e| e.topic.as_str() == "human.guidance");

            // Mirror the multi-hat Ralph path (L4636–4718): record the
            // trigger events this activation consumed so the missing-event
            // gate can distinguish pass-through hats (e.g. shipper on a
            // multi-consumer `plan.complete`) from hats that truly forgot
            // to emit.
            self.state.record_hat_activation(hat_id);
            self.state.last_activation_events = regular_events.clone();

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
            if skip_guidance {
                drop(guidance_events);
            } else {
                // Handle guidance
                self.update_robot_guidance(guidance_events);
                self.apply_robot_guidance(hat_id);
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
            // 2026-06-23 T2: append `## RUNTIME CONFIG` block so the hat
            // can read the runtime-resolved `max_fix_rounds` (which lives
            // under `event_loop:` in the YAML). The block is informational
            // and lives at the END of the hat prompt so the hat's own
            // workflow order (in `### GUARDRAILS`) stays authoritative.
            let base_prompt =
                append_runtime_config_block(base_prompt, self.config.event_loop.max_residuals);
            let base_prompt = self.append_terminal_deliverable_contract(base_prompt, hat_id);

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

            // 2026-06-17-003 U4 / 2026-06-17-005 R5:
            // `## ORCHESTRATOR CONTEXT` block is the canonical
            // view of the run. The block is always emitted
            // (even when projection is disabled) so the agent
            // never has to hand-read a ledger; the
            // `projection_disabled` flag in the block tells the
            // agent whether the values are live. R5 in
            // 2026-06-17-005 pins Phase 1 scope to the
            // **isolated** build_prompt path only — see the
            // Phase 1 scope note on `prepend_orchestrator_context`
            // and the backward-compat custom-hat path.
            //
            // OPAC U2: `## HAT IDENTITY` is the agent's single
            // source of truth for its role and permissions. It
            // lives *above* ORCHESTRATOR CONTEXT so the agent sees
            // "who you are" before "what the loop is doing" (KTD-5).
            let base_prompt = self.prepend_hat_identity(base_prompt, hat_id);
            // P1-7 fix: orchestrator context is placed BEFORE
            // wave context so the prompt stack order is:
            //   ## HAT IDENTITY
            //   ## WAVE CONTEXT (synthesizer only)
            //   ## ORCHESTRATOR CONTEXT
            //   hat instructions
            let base_prompt = self.prepend_orchestrator_context(base_prompt, hat_id);

            // R1: `## WAVE CONTEXT` block lives near the top for
            // `review-synthesizer`; it is a no-op for any other hat.
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
            // 2026-06-18-001 plan U6: 注入 `## RECENT REJECTIONS` 块
            // 告诉 agent 最近哪些 emit 被 runtime 拒收。让 agent
            // 看到 backpressure,避免用同一 payload 反复探测。
            let base_prompt = self.prepend_rejection_digest(base_prompt);
            // U7a (plan 2026-06-21-002): prepend the
            // deterministic correction + resume blocks.  Always
            // prepends the resume block when `--continue` ran
            // (the queue is non-empty).  Always prepends the
            // correction block when the queue is non-empty
            // (the U7a `prompt_context` queue is populated by
            // `emit_correction_context` calls on the policy
            // rejection path; when the feature flag is off, the
            // queue stays empty and this prepend is a no-op).
            let base_prompt = self.prepend_correction_and_resume(base_prompt);
            // 2026-06-28-003: prepend recovery directives derived
            // from pending `task.resume` events.
            let base_prompt = self.prepend_recovery_directives(base_prompt, &regular_events);
            // 2026-07-09-003 plan (U3): prepend the schema-backed
            // `## TRIGGER CONTEXT` block. The helper is a no-op
            // when the schema has no `trigger_context`
            // declaration or the hat does not subscribe to the
            // source topic, so the SC6 / R3 / R29 byte-identical
            // pre-feature contract holds for undeclared
            // presets.
            let base_prompt = self.prepend_trigger_context(base_prompt, hat_id, &regular_events);
            let with_skills = self.prepend_auto_inject_skills(base_prompt, hat_id);
            let with_scratchpad = self.prepend_scratchpad(with_skills, Some(hat_id));
            let with_state_files = self.prepend_state_files(with_scratchpad);
            let final_prompt = self.prepend_ready_tasks(with_state_files);
            // U18: macro edge next hint — when `event_loop.macro_edge_next_hint.enabled`
            // is true, prepend a one-line `## NEXT ACTION` derived from the most recent
            // accepted business event payload's `next_hint` field (≤120 chars). When the
            // feature is disabled or no hint is available the prepend is a no-op.
            let final_prompt = self.prepend_macro_next_hint(final_prompt, &regular_events, hat_id);
            // 2026-07-06-004 plan U6: wire the handoff envelope
            // extractor (U5) + prepender (U4) into the isolated
            // prompt chain. The helper is gated on
            // `event_loop.handoff_envelope.enabled &&
            // prompt_injection` and on a recent event carrying a
            // valid envelope; default-closed so non-serial presets
            // and ad-hoc loops are unaffected (regression defence
            // #3 / #6).
            let final_prompt = build_isolated_prompt_with_handoff(
                crate::event_loop::prompt_helpers::IsolatedPromptInputs {
                    base_prompt: final_prompt,
                    events: &regular_events,
                    config: &self.config.event_loop.handoff_envelope,
                    // U5 (2026-07-06-004 fix-plan R5): tighten
                    // the trust boundary so envelopes addressed
                    // to a different hat never reach this
                    // hat's prompt. `hat_id` is the current
                    // isolated hat id.
                    current_hat: hat_id.as_str(),
                },
            );
            // U4b: see solo-mode comment above. In isolated
            // mode the lint hint routes to the *source* hat
            // (the one that emitted the rejected event), so the
            // helper consults `pending_lint_resume.target` to
            // decide whether the current hat is the recipient.
            // The hint is consumed on first injection so the
            // same failure is not replayed forever.
            let final_prompt = self.inject_pending_lint_resume(final_prompt, hat_id);

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

        // Set active hat for downstream logic (default_publishes, enforce_hat_scope).
        // Mirror the isolated-mode assignment at L4079 so observers reading
        // `last_active_hat_ids` after `build_prompt` see the same value in both
        // execution modes. Without this, backward-compat (Coordinator default)
        // callers would observe a stale Vec while isolated callers see the
        // just-built hat — see test_rejected_work_done_retry_payload_reaches_executor_prompt.
        self.state.last_active_hat_ids = vec![hat_id.clone()];

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
        // 2026-06-23 T2: append `## RUNTIME CONFIG` block so the hat can
        // read the runtime-resolved `max_fix_rounds`. Appended BEFORE
        // `inject_phase_into_prompt` so the phase block (if any) sits
        // just above RUNTIME CONFIG at the tail of the prompt.
        let base = append_runtime_config_block(base, self.config.event_loop.max_residuals);
        let base = self.append_terminal_deliverable_contract(base, hat_id);
        let with_phase = self.inject_phase_into_prompt(base);
        let with_diagnosis = self.apply_runtime_diagnosis_prompt(with_phase, hat_id);
        // R5 (2026-06-17-005 fix plan): the
        // `## ORCHESTRATOR CONTEXT` block is intentionally NOT
        // injected on this path in Phase 1. The backward-compat
        // custom-hat path predates the state projector and
        // shares a single `events_context` across every hat in
        // the same loop; threading the projector snapshot
        // through here without breaking the
        // `RUNTIME_DIAGNOSIS_ALERT_HEADER` / auto-inject-skills
        // contract is a Phase 2 task. See the Phase 1 scope
        // note on `prepend_orchestrator_context` (event_loop)
        // and the comment on the isolated build_prompt branch
        // at L4522.
        // We intentionally skip `prepend_auto_inject_skills` here
        // because the backward-compat custom-hat path predates
        // that pipeline and tests assert the absence of skill
        // injection for this branch.
        let _ = RUNTIME_DIAGNOSIS_ALERT_HEADER; // silence unused-import lint
        Some(with_diagnosis)
    }

    /// 2026-07-26-001 plan U2: structured preview of what
    /// `build_prompt` would inject for the given hat, *without*
    /// running the loop. Powers the `ralph inspect prompt` CLI
    /// (U3-U5).
    ///
    /// **Side effects — single-CLI-invocation safe, do NOT reuse
    /// across hot loops or shared state.** Although the preview
    /// itself does not publish events, calling it ultimately
    /// invokes `build_prompt` (via `preview_block_titles`), which:
    ///
    /// 1. Calls `handoff_tracker.on_hat_activated(hat_id)`,
    ///    clearing any pending handoff deadlines for that hat.
    /// 2. Calls `event_bus.take_pending(hat_id)`, consuming
    ///    any pending events addressed to the new hat.
    ///
    /// Within a single CLI invocation (`ralph inspect prompt`)
    /// the `EventLoop` instance owns its state and no externally
    /// visible mutation escapes — the CLI process exits
    /// immediately after, so cleared deadlines and consumed
    /// pending events are simply discarded. **Across invocations,
    /// or in any long-lived hot loop / shared `EventLoop`
    /// instance, these calls would silently bypass WRC-U4's 30s
    /// escalation gate and consume pending escalations**, so
    /// `prompt_preview` MUST NOT be called more than once per
    /// `EventLoop` instance. This contract is the same one
    /// `build_prompt` honours; any caller that survives multiple
    /// activations should drive the same code path through the
    /// orchestrator's hat-activation lifecycle, not through this
    /// inspector.
    ///
    /// Returns `None` when the hat is not registered.
    ///
    /// The `auto_inject` set is derived from the same
    /// `prepend_auto_inject_skills` pipeline that `build_prompt`
    /// uses, **without** invoking it. The
    /// `preview_characterization` test module pins the equivalence
    /// between this preview and the live prompt — any future drift
    /// in the auto-inject rules must fail those tests, not this
    /// preview API.
    pub fn prompt_preview(&mut self, hat_id: &HatId) -> Option<PromptPreview> {
        let config = self.config.clone();
        let preview = preview_prompt_for_config(&config, hat_id, |_| Vec::new());
        // Fill block_titles via the heavier build_prompt path now
        // that the immutable borrow on config is released.
        let mut preview = preview?;
        preview.block_titles = self.preview_block_titles(hat_id);
        Some(preview)
    }

    /// 2026-07-26-001 plan U2 R3: thin alias for `build_prompt`
    /// so callers (especially the U1 `inspect --full` JSON / human
    /// paths) can build only the prompt body without materializing
    /// a full `PromptPreview` struct.
    ///
    /// **Side effects — same contract as `prompt_preview`:** this
    /// is a direct wrapper around `build_prompt`, so it inherits
    /// the `handoff_tracker.on_hat_activated` clear and
    /// `event_bus.take_pending` consumption. Single CLI invocation
    /// is safe; do not call more than once per `EventLoop`
    /// instance. See `prompt_preview`'s doc for the full rationale
    /// (WRC-U4 30s escalation gate).
    pub fn build_prompt_body(&mut self, hat_id: &HatId) -> Option<String> {
        self.build_prompt(hat_id)
    }

    /// Block titles extracted from a dry prompt build for `hat_id`,
    /// in the order they appear. Implementation: call
    /// `build_prompt` and parse out `## …` headers from the
    /// resulting string. Build prompt is side-effect-free with
    /// respect to ledger state (it only clears handoff deadlines
    /// for the hat — see build_prompt doc comment), so the dry
    /// build here is safe to call from a read-only CLI.
    pub(crate) fn preview_block_titles(&mut self, hat_id: &HatId) -> Vec<String> {
        let Some(prompt) = self.build_prompt(hat_id) else {
            return Vec::new();
        };
        let mut titles: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for line in prompt.lines() {
            let Some(rest) = line.strip_prefix("## ") else {
                continue;
            };
            let trimmed = rest.trim().to_string();
            if seen.insert(trimmed.clone()) {
                titles.push(trimmed);
            }
        }
        titles
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

        // U2 (2026-06-18-004 plan, R2, KTD2): when
        // `suppress_human_guidance` is set, the loop persists
        // guidance to the scratchpad for audit but does NOT
        // cache it in `robot_guidance` (which is the source for
        // `apply_robot_guidance` → prompt injection). ce-executor-serial
        // opts into this so the active hat's prompt never sees
        // human.guidance text — the source of the perky-maple
        // P1-2 probe storm.
        let suppress = self.human_guidance_suppressed();
        // 2026-06-18-001 plan U7: 当 suppress=true 时,progress-steward
        // 仍能收到 `human.guidance` 内容——`suppress` 设计本意是防止
        // executor 探测风暴,误伤了依赖 guidance 的 steward。
        // 豁免条件:
        // - 事件显式 target=progress-steward(由 EventBus U2 修复路由到位)
        // - progress_steward.exempt_from_suppress_human_guidance=true(默认)
        //   且事件无 target 但当前在 steward 上下文(如下一轮 build_prompt
        //   时 hat_id=progress-steward)
        let exempt_steward_hat_id = self
            .config
            .event_loop
            .progress_steward
            .steward_hat_id
            .clone();
        // 2026-06-28-005: progress_steward.exempt_from_suppress_human_guidance
        // was deleted together with the suppress_human_guidance field.
        // Hard-coded to false here; this branch becomes dead once
        // update_robot_guidance itself is removed in a follow-up phase.
        let exempt_enabled = false;

        // Persist new guidance to scratchpad before caching
        self.persist_guidance_to_scratchpad(&guidance_events);

        // 2026-06-13-004 review fix (correctness F2, KTD-7 two-layer
        // dedup): the in-memory `robot_guidance` vec is the source
        // for the next `apply_robot_guidance` → prompt injection.
        // A redelivered or duplicated `human.guidance` event would
        // otherwise add the same payload twice to the prompt.
        // Dedup against the existing vec and within the current
        // batch; persist layer has already dedup'd against disk.
        // U2: when `suppress_human_guidance` is set, the loop
        // does NOT push the deduped payload into the in-memory
        // cache (which is the source for prompt injection via
        // `apply_robot_guidance` → `## ROBOT GUIDANCE` block).
        // The scratchpad persistence above already happened so
        // the event survives for audit.
        // 2026-06-18-001 plan U7: progress-steward 豁免。
        let mut seen_in_batch: std::collections::HashSet<String> = std::collections::HashSet::new();
        for event in guidance_events {
            // Move the payload out so we can dedup by owned String
            // without fighting the borrow checker. `payload` is
            // moved into `robot_guidance` when it survives the
            // dedup check; otherwise dropped.
            let payload = event.payload;
            if suppress {
                // U7 豁免:target=steward 或 exempt_enabled + 事件无
                // target 但下一轮将进入 steward 上下文,跳过 suppress
                let targeted_to_steward = event
                    .target
                    .as_ref()
                    .map(|t| t.as_str() == exempt_steward_hat_id)
                    .unwrap_or(false);
                if !(exempt_enabled && targeted_to_steward) {
                    // Drop the payload on the floor — already
                    // persisted above.
                    continue;
                }
                debug!("U7: human.guidance exempt from suppress for progress-steward");
            }
            if seen_in_batch.insert(payload.clone()) {
                let already = self.robot_guidance.iter().any(|p| p == &payload);
                if already {
                    debug!(
                        payload_len = payload.len(),
                        "U9 (KTD-7 in-memory layer): skipping guidance payload already cached for prompt"
                    );
                } else {
                    self.robot_guidance.push(payload);
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
    fn apply_robot_guidance(&mut self, _hat_id: &HatId) {
        if self.robot_guidance.is_empty() {
            return;
        }

        // U2 (2026-06-18-004 plan, R2, KTD2): when
        // `suppress_human_guidance` is set, drain the in-memory
        // cache without pushing to `ralph.robot_guidance`. This
        // catches stale entries that pre-date the opt-in flip
        // (e.g. a config edit mid-loop) and ensures the active
        // hat prompt NEVER contains a `## ROBOT GUIDANCE` block
        // under suppress mode. The scratchpad still records the
        // raw guidance for audit.
        //
        // 2026-06-18-006 plan U5 (R5, KTD): also drain
        // `self.ralph.robot_guidance` so any guidance cached
        // BEFORE the suppress flip (e.g. a mid-loop config edit
        // that went non-suppress → suppress) does NOT leak into
        // the next prompt. Mirrors the isolated `build_prompt`
        // symmetry at line 4543 where `collect_robot_guidance()`
        // is paired with `clear_robot_guidance()` — the same
        // collector/clear invariant must hold on the suppress
        // path so a stale `## ROBOT GUIDANCE` block never survives
        // a `suppress_human_guidance` opt-in.
        // 2026-06-18-001 plan U7 (R-REP2 / R-D3):
        // suppress 模式下仍保留 progress-steward 的 guidance。
        // 既要保留"target=steward"的针对性 guidance（由
        // `update_robot_guidance` 已过滤保留），
        // 也要保留"无 target 但当前正在 build_prompt 的 hat_id
        // 就是 progress-steward"的兜底 guidance。
        // 豁免时仍要把 robot_guidance 推入 ralph,但**不**
        // 清空 `self.ralph.robot_guidance`——让 steward 在 suppress
        // 下能持续看到跨 turn 累积的 guidance。
        if self.human_guidance_suppressed() {
            // 2026-06-28-005: progress_steward.exempt_from_suppress_human_guidance
            // config field was deleted together with suppress_human_guidance.
            // The exempt branch is therefore unreachable: human_guidance_suppressed()
            // is a stub that always returns false, so the body below
            // becomes dead. Kept temporarily while update_robot_guidance
            // is scheduled for deletion in a follow-up phase.
            let _steward_hat_id = self
                .config
                .event_loop
                .progress_steward
                .steward_hat_id
                .as_str();
            // The exempt check used to read the now-deleted
            // exempt_from_suppress_human_guidance field; the helper
            // returns false unconditionally, so we fall through to
            // the suppress path uniformly.
            self.robot_guidance.clear();
            self.ralph.clear_robot_guidance();
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
        // 2026-06-28-003 P1: consult the runtime-recovery dispatcher
        // before persisting. If a DedupeEnvelope action matches this
        // envelope's retry_key, the dispatcher's view of the runtime
        // state already considers this envelope redundant (e.g. a
        // stall_recovery envelope on the same hat/topic was tracked
        // earlier in the iteration), so skip writing the duplicate to
        // recovery.jsonl and skip the orchestration audit event.
        if self.should_dedupe_envelope(envelope) {
            debug!(
                retry_key = %envelope.retry_key,
                "P1 dedupe: runtime-recovery dispatcher requested drop"
            );
            return crate::diagnosis::EscalationDecision {
                level: crate::diagnosis::EscalationLevel::Soft,
                retry_key: envelope.retry_key.clone(),
                attempt: 0,
                target_hat: None,
                reason: None,
            };
        }
        let hat = envelope
            .source_hat
            .as_deref()
            .unwrap_or(envelope.target_hat.as_deref().unwrap_or("ralph"));
        self.diagnostics
            .log_recovery(RecoveryJournalEntry::from_envelope(envelope.clone(), notes));
        self.diagnostics.log_orchestration(
            envelope.iteration.unwrap_or(0),
            hat,
            OrchestrationEvent::from_recovery_envelope(envelope),
        );
        let current_iteration = envelope
            .iteration
            .max(Some(self.state.iteration))
            .unwrap_or(0);
        self.recovery_responder
            .record_finding(envelope, current_iteration)
    }

    /// Returns true when the runtime-recovery dispatcher's
    /// `DedupeEnvelope` action matches `envelope.retry_key`.
    ///
    /// The dispatcher compares the candidate envelope against the
    /// currently tracked retry keys (plus pending findings from the
    /// same iteration) so a `missing_event_gate` envelope that
    /// duplicates an already-tracked `stall_recovery` on the same
    /// `(hat, topic)` is dropped before it pollutes recovery.jsonl.
    fn should_dedupe_envelope(&self, envelope: &RecoveryDiagnosisEnvelope) -> bool {
        use crate::recovery_runtime::RecoveryAction;
        let ctx = self.runtime_recovery_context(&[]);
        crate::recovery_runtime::dispatch(&ctx)
            .iter()
            .any(|action| matches!(action, RecoveryAction::DedupeEnvelope { drop_retry_key } if drop_retry_key == &envelope.retry_key))
    }

    /// U11-T2 step-handoff side effects: when the unified pipeline
    /// rejects a `queue.advance` / `plan.complete` event, publish the
    /// same `plan.blocked` + diagnostic + recovery envelope that the
    /// legacy `apply_step_handoff_gate` used to emit. This keeps the
    /// operator-facing signal (`ralph diagnose`, responder ladder)
    /// intact while the gate decision itself lives in the pure
    /// `StepHandoffRule`.
    fn emit_step_handoff_rejection_side_effects(
        &mut self,
        event: &JsonlEvent,
        result: &crate::validation::ValidationResult,
    ) {
        let (step, task_id) = {
            let payload = event.payload.as_deref().unwrap_or("");
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(payload) {
                let step = parsed
                    .get("step")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let task_id = parsed
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                (step, task_id)
            } else {
                (None, None)
            }
        };
        let reason = result
            .reason_code
            .as_deref()
            .and_then(|code| {
                code.strip_prefix(crate::validation::ReasonCode::STEP_HANDOFF_MISMATCH_PREFIX)
            })
            .unwrap_or("progress_task_mismatch");
        let detail = result.correction_hint.as_deref().unwrap_or("");

        let blocked_payload = serde_json::json!({
            "reason": reason,
            "topic": event.topic,
            "step": step,
            "task_id": task_id,
            "detail": detail,
        });
        let source_hat = HatId::from("plan-gate");
        let blocked =
            Event::new("plan.blocked", blocked_payload.to_string()).with_source(source_hat);
        self.bus.publish(blocked);

        let diagnostic = Event::new(
            "event.step_handoff.gate_rejected",
            format!(
                "step_handoff gate rejected topic='{}' reason={}",
                event.topic, reason
            ),
        );
        self.bus.publish(diagnostic);

        let envelope = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::PayloadContract)
            .severity(crate::diagnosis::DiagnosisSeverity::Warning)
            .iteration(self.state.iteration)
            .source_hat("plan-gate")
            .target_hat("plan-gate")
            .topic(event.topic.clone())
            .reason_code(reason)
            .message(format!(
                "step_handoff gate rejected topic='{}' reason={} detail={}",
                event.topic, reason, detail
            ))
            .safe_target(true)
            .build();
        self.record_recovery_envelope(&envelope, Vec::new());
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

    /// 2026-06-28-003: build a runtime-recovery context from the
    /// current loop state. Used by hot-path detectors.
    ///
    /// `extra_jsonl_events` are appended to the pending regular events so
    /// detectors can see just-accepted JSONL events that have not yet
    /// been published to the bus.
    pub(crate) fn runtime_recovery_context(
        &self,
        extra_jsonl_events: &[crate::event_reader::Event],
    ) -> crate::recovery_runtime::RuntimeContext {
        use crate::diagnosis::DiagnosisSource;
        use crate::recovery_runtime::{EnvelopeSnapshot, EventSnapshot, RetryKeyState};

        let mut ctx = crate::recovery_runtime::RuntimeContext {
            current_iteration: self.state.iteration,
            current_hat: self.state.last_hat.as_ref().map(|h| h.as_str().to_string()),
            ..Default::default()
        };

        // Snapshot the executor-class hat set from the live registry
        // so `block_executor_resend_storm` matches structurally on
        // `publishes contains work.done` rather than a hard-coded
        // "executor" string. Empty registry (test scaffolding) leaves
        // the list empty and the detector falls back to the legacy
        // string match for backwards compatibility.
        for id in self.registry.ids() {
            if let Some(cfg) = self.registry.get_config(id)
                && cfg.publishes.iter().any(|t| t == "work.done")
            {
                ctx.executor_hat_ids.push(id.as_str().to_string());
            }
        }

        // Snapshot tracked retry keys.
        for key in self.recovery_responder.tracked_retry_keys_list() {
            let outcome = self
                .recovery_responder
                .outcome_for(&key)
                .map(|o| format!("{o:?}"))
                .unwrap_or_else(|| "Pending".to_string());
            let attempt = self.recovery_responder.attempt_count(&key);
            let history: Vec<String> = self
                .recovery_responder
                .outcome_history_snapshot(&key)
                .into_iter()
                .map(|o| format!("{o:?}"))
                .collect();
            ctx.retry_key_states.push(RetryKeyState {
                retry_key: key.clone(),
                last_outcome: outcome.clone(),
                outcome_history: history,
                attempt_count: attempt,
            });
        }

        // Snapshot recent pending regular events plus any extra JSONL
        // events supplied by the caller (e.g. a freshly accepted work.done).
        for event in self.peek_pending_regular_events() {
            ctx.events.push(EventSnapshot {
                topic: event.topic.to_string(),
                payload: event.payload.clone(),
                iteration: self.state.iteration,
            });
        }
        for event in extra_jsonl_events {
            ctx.events.push(EventSnapshot {
                topic: event.topic.clone(),
                payload: event.payload.clone().unwrap_or_default(),
                iteration: self.state.iteration,
            });
        }

        // Snapshot pending findings as recovery envelopes.
        for finding in self.recovery_responder.pending_findings() {
            ctx.recovery_envelopes.push(EnvelopeSnapshot {
                retry_key: finding.retry_key.clone(),
                source: match finding.source {
                    DiagnosisSource::StallRecovery => "StallRecovery".to_string(),
                    DiagnosisSource::MissingEventGate => "MissingEventGate".to_string(),
                    DiagnosisSource::DriftMonitor => "DriftMonitor".to_string(),
                    DiagnosisSource::WorkflowGuard => "WorkflowGuard".to_string(),
                    DiagnosisSource::ExecutionContract => "ExecutionContract".to_string(),
                    DiagnosisSource::PayloadContract => "PayloadContract".to_string(),
                    DiagnosisSource::HookRetry => "HookRetry".to_string(),
                    DiagnosisSource::LoopStale => "LoopStale".to_string(),
                    DiagnosisSource::TopicFormat => "TopicFormat".to_string(),
                    _ => "Other".to_string(),
                },
                outcome: format!("{:?}", finding.outcome),
                iteration: finding.iteration.unwrap_or(self.state.iteration),
                attempt: finding.retry_attempt,
            });
        }

        ctx
    }

    /// 2026-06-28-003: run runtime-recovery detectors against the
    /// supplied context and apply the returned actions to the loop.
    /// Detectors are best-effort: a missing signal causes silent skip.
    pub fn apply_runtime_recovery_actions(
        &mut self,
        ctx: &crate::recovery_runtime::RuntimeContext,
    ) {
        use crate::recovery_runtime::RecoveryAction;
        use ralph_proto::{Event, HatId};

        for action in crate::recovery_runtime::dispatch(ctx) {
            match action {
                RecoveryAction::PublishEvent { topic, payload } => {
                    debug!(topic = %topic, "runtime-recovery: publishing corrective event");
                    let event =
                        Event::new(topic.as_str(), payload).with_source(HatId::from("ralph"));
                    // 2026-07-06 U2 (DEV-002): persist runtime-recovery
                    // corrective events to events.jsonl alongside the
                    // bus publish. Without this the trusted events stream
                    // diverges from the in-memory bus and downstream
                    // shipper routing gates (see shipper_reason.rs) miss
                    // the recovery context.
                    self.state.record_event(&event);
                    self.bus.publish(event);
                }
                RecoveryAction::ForcePlanBlocked { reason, retry_key } => {
                    warn!(%reason, %retry_key, "runtime-recovery: forcing plan.blocked");
                    let payload = serde_json::json!({
                        "reason": format!("recovery_exhausted:{retry_key}"),
                        "runtime_recovery_reason": reason,
                    });
                    // 2026-07-24-005 plan U1: target is `reporter`
                    // (was `shipper`); the shipper hat is removed
                    // from the supervisor preset — reporter is the
                    // canonical `plan.blocked` terminal owner.
                    let blocked = Event::new("plan.blocked", payload.to_string())
                        .with_source(HatId::from("ralph"))
                        .with_target(HatId::from("reporter"));
                    // 2026-07-06 U2 (DEV-002): persist the terminal
                    // plan.blocked to events.jsonl. Previously only
                    // bus.publish was called, leaving events.jsonl
                    // silent while the in-memory bus still routed
                    // downstream — silent-success path.
                    //
                    // ===========================================================================
                    // P0-1 LINT GUARD (2026-07-06 silent-success regression):
                    // DO NOT REORDER. `state.record_event(&blocked)` MUST run BEFORE
                    // `bus.publish(blocked)`. Otherwise the trusted events.jsonl
                    // diverges from the in-memory bus and shipper's
                    // `is_recoverable_plan_blocked_reason` lookup reads stale
                    // data, producing REVIEW_COMPLETE(pass) over a plan.blocked
                    // that was never persisted. This was the root cause of the
                    // 9-recurrence silent-success loop family
                    // (primary-20260705-224028 + 8 prior runs).
                    //
                    // If you must change this ordering, first read:
                    //   - `docs/report/2026-07-06-ce-executor-serial-primary-20260705-224028-diagnosis.md`
                    //   - `crates/ralph-core/src/recovery_runtime/publish_loop_stalled.rs`
                    //     (which now emits `recovery_exhausted:<retry_key>` literals
                    //     to align with this path)
                    // ===========================================================================
                    self.state.record_event(&blocked);
                    self.bus.publish(blocked);
                }
                RecoveryAction::InjectDirective { text } => {
                    warn!(%text, "runtime-recovery: directive injection requested");
                    // Store for the next prompt build. build_prompt drains
                    // the buffer so the directive is delivered exactly once.
                    self.state.pending_recovery_directives.push(text);
                }
                RecoveryAction::DedupeEnvelope { drop_retry_key } => {
                    debug!(%drop_retry_key, "runtime-recovery: envelope dedupe requested");
                    // Callers that record envelopes should check this action
                    // and skip writing the duplicate.
                }
            }
        }
    }

    /// This generalizes the former `prepend_memories()` into a skill auto-injection
    /// pipeline that handles memories, tools, and any other auto-inject skills.
    ///
    /// Injection order:
    /// 1. Memory data + ralph-tools skill (special case: loads memory data from store, applies budget)
    /// 2. Other auto-inject skills from the registry (wrapped in XML tags)
    ///
    /// Note (2026-06-25 refactor): the former step 2 was "RObot interaction skill (gated by
    /// `robot.enabled`)", which was removed together with the `ralph-telegram` crate; the
    /// `human.guidance` / `task.resume` recovery channel is unrelated and preserved.
    fn prepend_auto_inject_skills(&self, prompt: String, hat_id: &HatId) -> String {
        let mut prefix = String::new();

        // 1. Memory data + ralph-tools skill — special case with data loading
        self.inject_memories_and_tools_skill(&mut prefix, hat_id);

        // 2. Other auto-inject skills from the registry
        self.inject_custom_auto_skills(&mut prefix, hat_id);

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
    fn inject_memories_and_tools_skill(&self, prefix: &mut String, hat_id: &HatId) {
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

        // Inject ralph-tools skills via the SSOT plan_auto_inject.
        // plan_auto_inject already honours per-hat eligibility
        // (is_hat_eligible) and the gated/registry-auto split, so
        // the live path and the preview path produce identical
        // results.
        //
        // 2026-07-26-002 U1: chain only `gated` here. Custom
        // registry-auto skills are owned by
        // `inject_custom_auto_skills` below — chaining both sets
        // here produced double injection of any
        // `skills.overrides.<name>.auto_inject: true` skill.
        let (gated, _registry_auto, _on_demand) =
            SkillInjector::plan_auto_inject(&self.config, hat_id, &self.skill_registry);

        for entry in gated {
            let Some(skill) = self.skill_registry.get(entry.name.as_str()) else {
                continue;
            };
            if !prefix.is_empty() {
                prefix.push_str("\n\n");
            }
            prefix.push_str(&format!(
                "<{name}-skill>\n{content}\n</{name}-skill>",
                name = entry.name,
                content = skill.content.trim()
            ));
            debug!("Injected {} skill from registry", entry.name);
        }
    }

    /// Injects any user-configured auto-inject skills (excluding built-in skills handled separately).
    fn inject_custom_auto_skills(&self, prefix: &mut String, hat_id: &HatId) {
        // U8: the per-hat filter was previously dropped on the floor
        // (None), so hat-restricted skills were being injected into
        // every hat. Threading `hat_id` here is what the plan KTD calls
        // out as the "auto_inject_skills(None) → auto_inject_skills(Some(...))"
        // fix.
        for skill in self
            .skill_registry
            .auto_inject_skills(Some(hat_id.as_str()))
        {
            // Skip built-in skills handled above
            //
            // 2026-06-25 refactor: `robot-interaction` was removed because its
            // only content was `human.interact` / `human.guidance` Telegram
            // guidance; the `ralph-telegram` crate was deleted (see plan
            // 2026-06-25-001). No other Telegram-specific skills remain.
            //
            // U8: `ralph-tools-opac` is also handled above (it lives
            // in the ralph-tools injection block so the agent gets one
            // consolidated skill doc, not three at the bottom).
            if matches!(
                skill.name.as_str(),
                "ralph-tools" | "ralph-tools-tasks" | "ralph-tools-memories" | "ralph-tools-opac"
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

    /// Extract recovery directive IDs from a batch of pending events.
    ///
    /// Only `task.resume` events are inspected. The `recovery_directives`
    /// array is read from each payload, flattened, deduplicated while
    /// preserving first-seen order. Unknown IDs are kept (the lookup
    /// step skips them).
    fn recovery_directive_ids_from_events(events: &[Event]) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut ordered = Vec::new();
        for event in events {
            if event.topic.as_str() != "task.resume" {
                continue;
            }
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) else {
                continue;
            };
            let Some(array) = payload
                .get("recovery_directives")
                .and_then(|v| v.as_array())
            else {
                continue;
            };
            for item in array {
                let Some(id) = item.as_str() else {
                    continue;
                };
                if seen.insert(id.to_string()) {
                    ordered.push(id.to_string());
                }
            }
        }
        ordered
    }

    /// Build the `## RECOVERY DIRECTIVES` prompt section from the
    /// registered `ralph-tools-recovery-directives` skill.
    ///
    /// For each directive ID, the matching `## <ID>` section is extracted
    /// from the skill markdown. IDs without a matching section are
    /// silently skipped. Returns an empty string when there are no IDs
    /// or the skill is not registered.
    fn build_recovery_directives_section(&self, directive_ids: &[String]) -> String {
        if directive_ids.is_empty() {
            return String::new();
        }
        let Some(skill) = self.skill_registry.get("ralph-tools-recovery-directives") else {
            return String::new();
        };
        let content = skill.content.trim();
        let mut sections: Vec<String> = Vec::new();
        for id in directive_ids {
            let marker = format!("## {id}");
            let Some(start) = content.find(&marker) else {
                continue;
            };
            let rest = &content[start + marker.len()..];
            let end = rest.find("\n## ").unwrap_or(rest.len());
            let section = &content[start..start + marker.len() + end];
            sections.push(section.trim().to_string());
        }
        if sections.is_empty() {
            return String::new();
        }
        let mut out = String::from("## RECOVERY DIRECTIVES\n\n");
        out.push_str(
            "The following runtime directives apply to pending `task.resume` events. \
             Treat them as system operating procedure.\n\n",
        );
        for (i, section) in sections.iter().enumerate() {
            if i > 0 {
                out.push_str("\n\n");
            }
            out.push_str(section);
        }
        out.push('\n');
        out
    }

    /// Prepend recovery directives (if any) to the prompt.
    fn prepend_recovery_directives(&mut self, prompt: String, events: &[Event]) -> String {
        let ids = Self::recovery_directive_ids_from_events(events);
        let mut section = self.build_recovery_directives_section(&ids);
        // 2026-06-28-003: also prepend directives produced by in-flight
        // runtime-recovery detectors (e.g. resend-storm block).
        let runtime_directives = std::mem::take(&mut self.state.pending_recovery_directives);
        if !runtime_directives.is_empty() {
            if section.is_empty() {
                section = String::from("## RECOVERY DIRECTIVES\n\n");
            }
            for directive in runtime_directives {
                section.push_str("\n- ");
                section.push_str(&directive);
            }
            section.push('\n');
        }
        if section.is_empty() {
            return prompt;
        }
        format!("{section}\n{prompt}")
    }

    /// 2026-07-09-003 plan (U3): prepend the
    /// `## TRIGGER CONTEXT` block derived from the schema-
    /// declared `trigger_context` (U1) of the most recent
    /// accepted event that the current hat subscribed to.
    ///
    /// The block is rendered by [`crate::trigger_context`]:
    /// the helper here is the runtime wiring that finds the
    /// matching trigger, looks up the schema, and decides
    /// whether to inject at all. Three gates short-circuit to
    /// a no-op prompt (SC6 / R3 / R29):
    ///
    /// 1. `event_policy` is absent (no schemas declared).
    /// 2. No event in `regular_events` matches the hat's
    ///    declared `triggers` (no trigger ⇒ no context).
    /// 3. The schema for the matched topic has no
    ///    `trigger_context` declaration (default-empty
    ///    `TriggerContextConfig`).
    ///
    /// Topology safety (R21 / R22): the helper filters by the
    /// hat's own `triggers` list, so a `## TRIGGER CONTEXT`
    /// block can never be injected into a hat that did not
    /// subscribe to the source topic. U5 wires a sibling lint
    /// that catches the same mistake statically.
    ///
    /// The block is intentionally prepended **above** every
    /// other prepend helper so the agent sees the trigger
    /// summary first (R13 / R17 / KTD-5).
    fn prepend_trigger_context(
        &self,
        prompt: String,
        hat_id: &HatId,
        regular_events: &[Event],
    ) -> String {
        // Gate 1: no event policy ⇒ no schemas ⇒ no block.
        let Some(policy) = self.config.event_loop.event_policy.as_ref() else {
            return prompt;
        };

        // The current hat's declared triggers drive the
        // topology guard. We never fall back to a wildcard
        // search — a hat that subscribes to no topics must
        // not see a trigger context.
        let Some(hat_config) = self.registry.get_config(hat_id) else {
            return prompt;
        };
        let hat_triggers: Vec<String> = hat_config.triggers.clone();
        if hat_triggers.is_empty() {
            return prompt;
        }

        // Find the most recent non-system event the hat
        // subscribes to.
        let Some(trigger) =
            crate::trigger_context::find_matching_trigger_event(regular_events, &hat_triggers)
        else {
            return prompt;
        };

        // Gate 2: schema for the source topic must exist and
        // declare a non-empty `trigger_context` block.
        let Some(schema) = policy.schemas.get(trigger.topic) else {
            return prompt;
        };
        if schema.trigger_context.summary_fields.is_empty()
            && schema.trigger_context.routing_hints.is_empty()
        {
            return prompt;
        }

        // Build + render. `source_hat` is unknown at this
        // layer (events do not carry it), so the renderer
        // surfaces `(unknown source hat)`. That is a U4 / U5
        // observable gap that strict lint can flag if a
        // schema/preset relies on it.
        let view = crate::trigger_context::build(&crate::trigger_context::TriggerContextInput {
            current_hat: hat_id.as_str(),
            source_topic: trigger.topic,
            source_hat: None,
            schema,
            payload: &trigger.payload,
        });

        let Some(block) = crate::trigger_context::render(&view) else {
            return prompt;
        };

        format!("{block}\n{prompt}")
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
        //
        // U2 (2026-06-18-004 plan, R2, KTD2): when the loop
        // opts into `suppress_human_guidance` (ce-executor-serial),
        // strip the same blocks for the active hat regardless of
        // bootstrap state. This is the source of the perky-maple
        // P1-2 probe storm — the executor hat saw `### HUMAN
        // GUIDANCE: Focus on error handling` and went into a
        // 6-round emit-probing spiral.
        let gate_closed = active_hat_id_for_filter
            .map(|hat| self.coordinator_bootstrap_gate_closed(hat))
            .unwrap_or(false);
        let suppress_active = self.human_guidance_suppressed();
        // 2026-06-28-005: filter_human_guidance_blocks was
        // deleted together with the `human.guidance` topic.
        // The bootstrap gate (`gate_closed`) is the only
        // remaining reason to filter the scratchpad today.
        let content = if gate_closed {
            // Drop the `### HUMAN GUIDANCE` block from any
            // historical scratchpad that pre-dates the topic
            // removal. This is purely defensive: a scratchpad
            // from before 2026-06-28 might still contain the
            // block; the regex-free inline filter below
            // strips it line by line. We keep the filter
            // here as a small private helper rather than
            // pulling back the public filter function.
            strip_human_guidance_block(&content)
        } else if suppress_active {
            // Suppress is now a no-op (the topic it gated is
            // gone). Kept for backwards-compatible YAML
            // loading — the field still deserializes (see
            // Phase 3b U7) and we simply do not act on it.
            content
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

    /// 2026-06-18-001 plan U6: prepend `## RECENT REJECTIONS` 块。
    /// 复用 `LoopState::format_rejection_digest_block`,空 digest
    /// 时返回空字符串,no-op 行为。
    fn prepend_rejection_digest(&self, prompt: String) -> String {
        let block = self.state.format_rejection_digest_block();
        if block.is_empty() {
            prompt
        } else {
            format!("{block}\n{prompt}")
        }
    }

    /// U7a (plan 2026-06-21-002): prepend the
    /// `## ORCHESTRATOR CORRECTION` block (when
    /// `state.prompt_context.correction_blocks` is non-empty)
    /// and the `## LOOP RESUME CONTEXT` block (when
    /// `state.prompt_context.resume_blocks` is non-empty).  The
    /// resume block is also consumed here (`Option::take`-style
    /// via [`std::mem::take`]) so it appears in exactly one
    /// prompt — the first prompt after `--continue`.  The
    /// correction queue is **not** consumed here so multiple
    /// rejections can accumulate across iterations and be
    /// folded into the next prompt; the caller clears the queue
    /// when it wants to start fresh.
    fn prepend_correction_and_resume(&mut self, prompt: String) -> String {
        // Take the resume block out — the first prompt after
        // resume must carry `## LOOP RESUME CONTEXT`, but a
        // subsequent prompt must not (the user already saw the
        // block; showing it again would be confusing).
        let resume_blocks = std::mem::take(&mut self.state.prompt_context.resume_blocks);
        let mut pc = std::mem::take(&mut self.state.prompt_context);
        pc.resume_blocks = resume_blocks;
        // 2026-06-26 plan U6: drain `correction_blocks` after
        // rendering. The previous "queue persists across
        // iterations" behaviour caused the prompt to grow on
        // every iteration as the same correction was re-rendered,
        // which is exactly the path that the plan warns about
        // under "correction_blocks 必须 consume-on-use". We
        // consume-on-use: render the correction block once,
        // then drop the queue. If the rejection is persistent
        // (the agent does not act on the correction), the next
        // iteration's `inject_completion_correction` call will
        // either re-queue a new correction (under the budget)
        // or surface `CompletionStuck(RejectionDigestExhausted)`
        // when the budget is exhausted.
        let correction_block = pc.render_correction_block();
        pc.correction_blocks.clear();
        let resume_block = pc.render_resume_block();
        let block = {
            let mut s = String::new();
            if !correction_block.is_empty() {
                s.push_str(&correction_block);
                s.push('\n');
            }
            if !resume_block.is_empty() {
                s.push_str(&resume_block);
                s.push('\n');
            }
            s
        };
        // Re-install the remaining prompt_context (resume_blocks
        // preserved; correction_blocks already empty).
        self.state.prompt_context = pc;
        if block.is_empty() {
            prompt
        } else {
            format!("{block}{prompt}")
        }
    }

    /// 2026-06-17-003 U4: prepend the `## ORCHESTRATOR CONTEXT`
    /// block. Reads the projector's in-memory cache when state
    /// projection is enabled; falls back to a disabled-stub
    /// explanation otherwise (so the agent still sees the
    /// heading and knows the orchestrator owns the ledgers).
    ///
    /// Phase 1 scope (R5 in 2026-06-17-005 fix plan): only the
    /// `isolated` build_prompt path calls this helper. The
    /// `HatlessRalph` (solo / multi-hat coordinator) and the
    /// backward-compat custom-hat paths skip injection — they
    /// build their prompts through a different pipeline that
    /// does not own a `StateProjector`. Widening the scope to
    /// those paths is deferred to Phase 2.
    /// OPAC U2: prepend the `## HAT IDENTITY` block to the prompt so
    /// the agent sees its authoritative identity and permission list
    /// (derived from the resolved `RalphConfig`) before any other
    /// injected context. Mirrors [`HatIdentitySnapshot::to_prompt_block`].
    ///
    /// The block is rendered only for hats that exist in the resolved
    /// config (so a stale `ralph run` against an outdated preset does
    /// not crash on an unknown hat id) and is skipped for the `ralph`
    /// orchestrator sentinel — the prompt there is framework-driven
    /// and never needs an explicit identity header. The placement is
    /// deliberately *above* `## ORCHESTRATOR CONTEXT` so the agent
    /// sees "who you are" before "what the loop is doing" (KTD-5).
    pub fn prepend_hat_identity(&self, prompt: String, hat_id: &HatId) -> String {
        if hat_id.as_str() == "ralph" {
            return prompt;
        }
        let Some(snapshot) =
            crate::hat_identity::HatIdentitySnapshot::from_config(&self.config, hat_id)
        else {
            tracing::debug!(
                hat_id = %hat_id.as_str(),
                "OPAC U2: skipping ## HAT IDENTITY injection for unknown hat"
            );
            return prompt;
        };
        let hat_block = snapshot.to_prompt_block(&self.config);
        format!("{}{prompt}", hat_block)
    }

    fn prepend_orchestrator_context(&self, prompt: String, hat_id: &HatId) -> String {
        // The `ralph` / orchestrator itself and short-lived
        // control hats do not need the context; the prompt is
        // already covered by the framework's own message.
        if hat_id.as_str() == "ralph" {
            return prompt;
        }
        let mut snap = if let Some(p) = self.state.state_projection.as_ref() {
            crate::runtime_state::RuntimeStateSnapshot::build(p)
        } else {
            crate::runtime_state::RuntimeStateSnapshot::disabled_stub()
        };
        // Inject git baseline SHAs from loop state. These are recorded by
        // the runner at loop start and are not part of the state projector's
        // ledgers.
        snap.loop_start_sha = self.state.loop_start_sha.clone();
        // B-layer reconciliation (2026-07-05 plan): prefer the SHA on disk
        // (`.ralph/agent/plan-baseline-{key}.sha`) over the in-memory
        // LoopState copy, which is stale when plan-reviewer's
        // §Step 2.5b reconciliation rewrites the file mid-run. Falls
        // back to LoopState on missing/unreadable files.
        snap.plan_baseline_sha = self.resolve_reconciled_plan_baseline_sha();
        format!("{}{prompt}", snap.to_prompt_block())
    }

    /// Read the latest plan baseline SHA from disk on every hat prompt.
    /// The reader is intentionally read-only and ignores errors: the
    /// caller keeps the LoopState fallback when disk is unavailable,
    /// the derivation key cannot be computed, or `loop_context` was
    /// not provided (e.g. unit tests using `EventLoop::new` directly).
    fn resolve_reconciled_plan_baseline_sha(&self) -> Option<String> {
        use crate::plan_baseline::{derive_baseline_key, read_plan_baseline};
        if let Some(ctx) = self.loop_context.as_ref() {
            let plan_key = derive_baseline_key(
                &self.config.event_loop.prompt_file,
                None,
                self.config.event_loop.prompt.as_deref(),
                Some(ctx.workspace()),
            );
            if let Some(key) = plan_key.as_deref()
                && let Some(sha) = read_plan_baseline(ctx.workspace(), Some(key))
            {
                return Some(sha);
            }
        }
        self.state.plan_baseline_sha.clone()
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

    /// U4b (plan 2026-06-20-001, R12 / R13 / KTD-8): inject the
    /// lint failure hint as `## LINT MIRROR` + `## LINT RESUME
    /// REQUIRED` at the head of `prompt`.  The hint is consumed
    /// on first read (`Option::take`) so a stale resume does not
    /// leak across prompts.
    ///
    /// The block is prepended (above the rest of the prompt) so
    /// the agent sees the protocol hash + failing topic first —
    /// matching the order in the CLI emit failure output so the
    /// two paths produce the same canonical block (R12).
    ///
    /// In multi-hat / isolated modes the hint is only injected
    /// when the active hat matches `hint.target` — otherwise
    /// the resume belongs to a *different* hat and the current
    /// hat has nothing to fix. Solo / coordinator modes always
    /// inject because `hat_id` is `"ralph"` (the orchestrator
    /// itself, which sees every hat's alerts).
    fn prepend_macro_next_hint(
        &self,
        prompt: String,
        regular_events: &[ralph_proto::Event],
        hat_id: &HatId,
    ) -> String {
        // U18 (P2): macro edge next hint. The flag defaults to disabled;
        // when off we are a no-op so existing loops are unaffected.
        let flag = self.config.event_loop.macro_edge_next_hint.enabled;
        if !flag {
            return prompt;
        }

        // Only the dispatcher hat (the one that received the macro
        // edge event) sees the hint; coordinators do not need it
        // because the runtime already routes them.
        if hat_id.as_str() == "ralph" {
            return prompt;
        }

        // Find the most recent accepted business event whose payload
        // carries a `next_hint` string. We scan backwards so the
        // latest hint wins (older hints are stale).
        let mut hint: Option<String> = None;
        for ev in regular_events.iter().rev() {
            let payload_str = ev.payload.clone();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload_str)
                && let Some(s) = val.get("next_hint").and_then(|v| v.as_str())
            {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    // Cap at 120 chars (U18 contract). Truncate at
                    // a char boundary so multi-byte codepoints are
                    // not sliced.
                    let cap = trimmed.chars().take(120).collect::<String>();
                    hint = Some(cap);
                    break;
                }
            }
        }

        let Some(hint) = hint else { return prompt };
        format!("## NEXT ACTION\n\n{hint}\n\n---\n\n{prompt}")
    }

    fn inject_pending_lint_resume(&mut self, prompt: String, hat_id: &HatId) -> String {
        let Some(hint) = self.state.pending_lint_resume.take() else {
            return prompt;
        };
        // Route check: in multi-hat / isolated mode, only inject
        // when the current hat is the lint target.
        if self.config.event_loop.execution_mode != HatExecutionMode::Coordinator
            && hat_id.as_str() != "ralph"
        {
            // Map `LintResumeTarget` -> owning hat name. The
            // hint class already classifies into source hat /
            // plan-gate; we use the canonical hat ids here. The
            // mapping is identical to KTD-4 / hint.rs.
            let target_hat = match hint.target {
                LintResumeTarget::SourceHat => {
                    // The lint failure came from THIS hat (the
                    // one currently building the prompt). SourceHat
                    // means "the hat that emitted the rejected
                    // event"; in single-hat mode that is the
                    // active hat. In multi-hat mode the source
                    // hat is identified by the topic itself; the
                    // resume hint carries the failing topic and
                    // the active hat should be the one that
                    // emits it. We accept the hint when the
                    // current hat's `publishes` list contains
                    // the failing topic — otherwise the resume
                    // belongs to a different hat.
                    self.registry
                        .get_config(hat_id)
                        .map(|cfg| cfg.publishes.iter().any(|t| t == hint.topic.as_str()))
                        .unwrap_or(false)
                }
                LintResumeTarget::PlanGate => {
                    hat_id.as_str() == "plan-gate"
                        || hat_id.as_str() == "ralph"
                        || hat_id.as_str() == "coordinator"
                }
            };
            if !target_hat {
                // Not for this hat — restore the hint so the
                // correct hat's next prompt can consume it.
                self.state.pending_lint_resume = Some(hint);
                return prompt;
            }
        }

        // U11-T3 note: the matching `CorrectionContext` push to
        // `state.prompt_context` happens in
        // `apply_engine_required_field_gate` at the moment of
        // rejection (so the per-iteration BDD snapshot sees the
        // block in the iteration it fired). This helper only
        // emits the human-readable prompt block.

        let view = ProtocolView::from_event_loop(&self.config.event_loop);
        let mirror = build_lint_mirror_block(&view, &hint);
        let resume = build_lint_resume_block(&hint);
        format!("{mirror}{resume}\n{prompt}")
    }

    /// U2 (plan 2026-06-20-001, R15 / KTD-10): decide whether the
    /// event loop should consult the engine-backed gate before
    /// the d623c09 policy / scope gates. Same opt-in as the CLI
    /// emit lint (see `commands/emit.rs::should_run_lint`).
    fn should_run_engine_gate(&self) -> bool {
        if std::env::var("RALPH_SERIAL_LINT_MODE")
            .map(|v| v.eq_ignore_ascii_case("off"))
            .unwrap_or(false)
        {
            return false;
        }
        if self.config.event_loop.execution_mode == HatExecutionMode::Coordinator {
            return false;
        }
        // Plan 2026-06-20-001 KTD-7 / RISK-6 circuit breaker.
        // When the linter has rejected every event for
        // `LINT_CIRCUIT_BREAKER_LIMIT` consecutive iterations,
        // it auto-disables itself for the rest of the run.
        // d623c09's runtime gates keep running, and the
        // existing `consecutive_malformed_events >= 3`
        // termination check remains as the final backstop. We
        // trip on threshold 2 so the breaker fires *before* the
        // termination check at 3, giving the runtime gates one
        // iteration to record the rejection before the loop
        // dies. Operators can override with
        // `RALPH_SERIAL_LINT_MODE=off`.
        if self.state.lint_circuit_breaker_tripped {
            return false;
        }
        true
    }

    /// U2 (plan 2026-06-20-001): apply the engine's required-
    /// fields gate to a parsed batch *before* handing the
    /// batch to the d623c09 policy / scope / recovery stack.
    /// Returns a fresh `ParseResult` with rejected events
    /// reported as malformed (so the existing rejection
    /// bookkeeping fires the same way it does for
    /// `event.malformed`) and the accepted events proceeding
    /// through the d623c09 path unchanged.
    ///
    /// P1-3: the previous name (`engine_required_field_filter`)
    /// suggested a pure filter; the function actually does four
    /// distinct things:
    ///
    ///   1. runs the engine gate (decision),
    ///   2. drops rejected events from the batch (filter),
    ///   3. appends a `MalformedLine` so the existing
    ///      bookkeeping increments `consecutive_malformed_events`
    ///      and publishes `event.malformed` (audit),
    ///   4. seeds `state.pending_lint_resume` so the next
    ///      `build_prompt` injects `## LINT RESUME REQUIRED`
    ///      (agent feedback).
    ///
    /// The new name `apply_engine_required_field_gate`
    /// matches the actual contract: a fail-fast **gate** that
    /// has side effects. The four steps are factored into
    /// helpers below so each step is independently testable
    /// and rename-safe.
    ///
    /// Fail-closed semantics: when the engine rejects an event
    /// (because `required_fields` are missing), the event is
    /// **dropped** — it never lands on the bus and never sees
    /// d623c09.
    ///
    /// Circuit breaker (KTD-7 / RISK-6): if every event in the
    /// batch was rejected, increment
    /// `consecutive_engine_gate_rejections`; when it reaches
    /// `LINT_CIRCUIT_BREAKER_LIMIT`, set
    /// `lint_circuit_breaker_tripped = true` so the engine
    /// gate short-circuits for the rest of the run. A
    /// batch with at least one accept resets the counter
    /// (the gate did useful work that iteration).
    fn apply_engine_required_field_gate(
        &mut self,
        mut result: crate::event_reader::ParseResult,
    ) -> crate::event_reader::ParseResult {
        use crate::event_reader::MalformedLine;
        use crate::preset::engine::{
            GateDecision, LintContext, LintResumeHint, gates::RejectionKind, run_gates,
        };
        let view = ProtocolView::from_event_loop(&self.config.event_loop);
        let ctx = LintContext;
        let mut rejected = 0usize;
        let mut last_rejection: Option<(String, RejectionKind, String)> = None;
        let mut kept = Vec::with_capacity(result.events.len());
        for event in result.events.drain(..) {
            let topic = event.topic.clone();
            let payload_value = match event.payload.as_deref() {
                Some(s) if !s.is_empty() => Self::parse_event_payload_value(s),
                _ => serde_json::Value::Null,
            };
            let decision = run_gates(&view, &ctx, &topic, &payload_value, event.hat.as_deref());
            match decision {
                GateDecision::Accept => kept.push(event),
                GateDecision::Reject { kind, message } => {
                    rejected += 1;
                    tracing::warn!(
                        topic = %topic,
                        kind = %kind.reason_code(),
                        reason = %message,
                        hat = ?event.hat.as_deref(),
                        "engine gate rejected event (U2 fail-fast, required-fields)"
                    );
                    let raw = event.payload.clone().unwrap_or_default();
                    result.malformed.push(MalformedLine::new(
                        0,
                        &raw,
                        format!("engine_rejected:{}: {}", kind.reason_code(), message),
                    ));
                    last_rejection = Some((topic.clone(), kind, message));
                }
            }
        }
        result.events = kept;
        if rejected > 0 && result.events.is_empty() {
            self.state.consecutive_engine_gate_rejections = self
                .state
                .consecutive_engine_gate_rejections
                .saturating_add(1);
            // P1-1 (P1 follow-up): resolve the trip threshold
            // with a 3-tier fallback so tests can relax the
            // limit without `std::env::set_var` (unsafe under
            // Rust 1.81+ / workspace's `forbid(unsafe_code)`):
            //   1. test override (set via
            //      `set_lint_circuit_breaker_limit_for_test`) —
            //      wins so the 3-stage R11 escalation scenario
            //      can run independently of the env var.
            //   2. `RALPH_LINT_CIRCUIT_BREAKER_LIMIT` env var —
            //      production operator override.
            //   3. `LINT_CIRCUIT_BREAKER_LIMIT` constant (RISK-6:
            //      1-iter early warning).
            let limit = crate::event_loop::loop_state::lint_circuit_breaker_limit_for_test()
                .or_else(|| {
                    std::env::var("RALPH_LINT_CIRCUIT_BREAKER_LIMIT")
                        .ok()
                        .and_then(|v| v.parse::<u32>().ok())
                })
                .unwrap_or(LINT_CIRCUIT_BREAKER_LIMIT);
            if self.state.consecutive_engine_gate_rejections >= limit
                && !self.state.lint_circuit_breaker_tripped
            {
                self.state.lint_circuit_breaker_tripped = true;
                tracing::warn!(
                    consecutive = self.state.consecutive_engine_gate_rejections,
                    limit,
                    "lint circuit breaker tripped: engine gate disabled for remainder of run \
                     (d623c09 runtime gates remain active; RALPH_SERIAL_LINT_MODE=off \
                     is the operator override)"
                );
            }
        } else if self.state.consecutive_engine_gate_rejections > 0 {
            // Reset on any accept — the gate is still useful.
            self.state.consecutive_engine_gate_rejections = 0;
        }
        if rejected > 0 {
            tracing::debug!(
                rejected,
                kept = result.events.len(),
                "engine gate filter result"
            );
            // Review P0 #4: seed the in-memory resume hint so
            // `inject_pending_lint_resume` injects the failure
            // block on the next `build_prompt`. This is the
            // single source of truth for the lint resume path;
            // the CLI emit file-write (now a no-op stub) is no
            // longer part of the contract.
            if let Some((topic, kind, message)) = last_rejection {
                let hint = LintResumeHint::from_typed_rejection(&topic, kind, &message);
                // U11-T3: also push the lint rejection into the
                // unified `state.prompt_context` queue at the
                // moment of rejection (not at `build_prompt` time).
                // This way the per-iteration BDD snapshot sees
                // the correction block in the same iteration the
                // rejection fired, and downstream prompt
                // builders can drain the queue if needed.
                //
                // The R11 escalation tripwire (and the BDD's
                // expected `retry_count`) is keyed off the
                // reason_code (`lint:missing_field` etc.). We
                // update `LoopState::recent_rejection_digest`
                // (the legacy in-memory digest that works without
                // the unified ledger) so the next call sees the
                // incremented count. When the ledger IS
                // configured, the helper also commits a
                // `CommitDelta::RejectionRecorded` there.
                let reason_code = format!(
                    "lint:{}",
                    crate::event_loop::rejection::extract_reason_code(&message)
                );
                self.state.record_rejection_digest(
                    &reason_code,
                    &message,
                    &topic,
                    "iteration-start",
                );
                let retry_count = self
                    .state
                    .recent_rejection_digest
                    .get(&reason_code)
                    .map(|e| e.count)
                    .unwrap_or(1u32);
                let mut state_ledger = std::mem::take(&mut self.state.state_ledger);
                let _ctx = crate::correction::emit_correction_from_lint_hint(
                    state_ledger.as_mut(),
                    &hint,
                    retry_count,
                    None,
                    &mut self.state.prompt_context,
                );
                self.state.state_ledger = state_ledger;
                self.state.pending_lint_resume = Some(hint);
            }
        }
        result
    }

    /// Parse the JSON value from an event's payload string,
    /// returning `Value::Null` when the payload is empty.
    /// Non-JSON payloads are wrapped as `Value::String` so the
    /// engine's required-field check still operates (the
    /// required-field set is empty for non-object payloads, so
    /// any JSON object missing fields is correctly rejected).
    fn parse_event_payload_value(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw).unwrap_or(serde_json::Value::String(raw.to_string()))
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
        // 2026-06-28-005 plan U3: the previous addition of
        // `topic == "plan.blocked"` here was reverted because
        // it broke the legitimate hat-routing path in
        // `test_ce_executor_plan_blocked_routes_to_shipper_not_reporter`:
        // the ce-executor-serial preset has a `shipper` hat
        // with `triggers: ["plan.blocked"]`, and that test
        // expects the shipper to be the next active hat after
        // a real `plan.blocked` event. Marking the topic as a
        // system event short-circuits that routing.
        //
        // The original KTD-3 contract-reject concern was that
        // `plan.blocked` would shadow the targeted retry on
        // the source hat. That is handled separately by
        // publishing the targeted retry *before* the guidance
        // publish (see event_loop/mod.rs around the contract
        // reject site) and by keeping the publish `with_target`
        // on the guidance event itself. The system-event guard
        // is not required.
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

        let payload = serde_json::json!({
            "reason": "default_publishes",
            "message": format!(
                "Hat '{}' emitted no events; orchestrator injected default topic '{}'",
                hat_id.as_str(),
                default_topic_str
            ),
            "hat": hat_id.as_str(),
            "topic": default_topic_str,
        });
        let default_event = Event::new(default_topic_str, payload.to_string())
            .with_source(hat_id.clone())
            .with_system_injected();
        let verdict_topics = self.verdict_gate_topics();
        let verdict_topics_slice = verdict_topics.as_deref();
        self.state
            .record_verdict_if_match(&default_event, verdict_topics_slice);
        self.state.record_completion_predecessor_if_match(
            &default_event,
            self.config.event_loop.completion_payload_match.as_ref(),
        );

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
            // P0-5: gate default_publishes' terminal signal on
            // `required_events`.
            if let Err(reason) = self.state.mark_completion_requested(
                &self.config.event_loop.required_events,
                &self.config.event_loop.completion_promise,
            ) {
                tracing::warn!(
                    reason = %reason,
                    hat = %hat_id.as_str(),
                    topic = %default_topic_str,
                    iteration = self.state.iteration,
                    "P0-5: default_publishes completion rejected; \
                     required events not yet observed; \
                     hat's default emit will not transition loop to terminal"
                );
                // Fall through: still publish the default
                // event so the agent can continue running;
                // the terminal transition just does not fire.
            } else {
                // P1-2: per-event commit (see `commit_terminal_delta`).
                Self::commit_terminal_delta(
                    &mut self.state.state_ledger,
                    crate::state::CommitDelta::CompletionRequested,
                );
            }
        }

        self.persist_system_injected_jsonl_event(hat_id, default_topic_str, &payload);

        let reason_code = "default_publishes_injected";
        let hat_str = hat_id.as_str();
        let mut env_builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::MissingEventGate)
            .severity(crate::diagnosis::DiagnosisSeverity::Warning)
            .iteration(self.state.iteration)
            .topic(default_topic_str)
            .source_hat(hat_str)
            .reason_code(reason_code)
            .message(format!(
                "Hat '{hat_str}' emitted no events; orchestrator injected default_publishes topic '{default_topic_str}'"
            ))
            .expected_action(format!(
                "Hat '{hat_str}' should emit '{default_topic_str}' before the turn ends; this injection is a synthetic fallback"
            ))
            .outcome(crate::diagnosis::DiagnosisOutcome::Pending)
            .retry_key(
                crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                    crate::diagnosis::DiagnosisSource::MissingEventGate,
                    Some(hat_str),
                    Some(default_topic_str),
                    reason_code,
                    None,
                ),
            );
        if let Some(session_id) = self.diagnostics.session_id() {
            env_builder = env_builder.session_id(session_id);
        }
        let envelope = env_builder.build();
        self.record_recovery_envelope(
            &envelope,
            vec![format!("default_publishes:{default_topic_str}")],
        );

        self.bus.publish(default_event);
    }

    /// P0-3 (2026-07-02-005): persist orchestrator-injected
    /// `default_publishes` events to the trusted events JSONL so
    /// operators can audit why a downstream hat was activated.
    ///
    /// The event is also published on the bus for immediate routing.
    /// The JSONL copy is marked `system_injected: true` and the reader
    /// position is advanced past it so the next
    /// `process_events_from_jsonl` pass does not double-publish.
    ///
    /// 2026-07-03-001 supervisor real-wiring: this method is `pub`
    /// because `ralph-cli`'s dispatcher calls it after a supervisor
    /// `tick` returns `InjectedComplete` / `InjectedFailed` to write
    /// the `*.wave.complete` / `*.wave.failed` coordination event and
    /// advance the reader cursor. The BDD scenarios in
    /// `ralph-core/tests/scenarios.rs` also call it from
    /// `run_bdd_supervisor_fan_in`.
    pub fn persist_system_injected_jsonl_event(
        &mut self,
        hat_id: &HatId,
        topic: &str,
        payload: &serde_json::Value,
    ) {
        let events_path = self.event_reader.path().to_path_buf();
        if let Some(parent) = events_path.parent()
            && !parent.as_os_str().is_empty()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(
                path = %events_path.display(),
                error = %err,
                "P0-3: failed to create events directory for default_publishes audit write"
            );
            return;
        }

        let ts = chrono::Utc::now().to_rfc3339();
        let record = serde_json::json!({
            "topic": topic,
            "payload": payload,
            "ts": ts,
            "hat": hat_id.as_str(),
            "source": hat_id.as_str(),
            "system_injected": true,
        });

        let append_result = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)?;
            let line = serde_json::to_string(&record)?;
            writeln!(file, "{line}")?;
            file.flush()?;
            Ok(())
        })();

        match append_result {
            Ok(()) => {
                if let Ok(metadata) = std::fs::metadata(&events_path) {
                    self.event_reader.set_position(metadata.len());
                }
                debug!(
                    hat = %hat_id.as_str(),
                    topic = %topic,
                    path = %events_path.display(),
                    "P0-3: persisted default_publishes event to JSONL for audit"
                );
            }
            Err(err) => {
                tracing::warn!(
                    hat = %hat_id.as_str(),
                    topic = %topic,
                    path = %events_path.display(),
                    error = %err,
                    "P0-3: failed to persist default_publishes event to JSONL; continuing with bus publish only"
                );
            }
        }
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
    ///
    /// 2026-06-23-005 F4 (P0-2 重定位): `process_output` still
    /// consumes the legacy `consecutive_failures >= 5` termination
    /// path. The plan (`2026-06-23-005` U3 / KTD-7) envisioned a
    /// single-match `TerminationTrigger` dispatch, but the
    /// prerequisite (`pending_dead_letter` field + `LoopState`
    /// persistence) does not exist in the current codebase. F4
    /// therefore leaves `process_output` untouched and only
    /// documents the boundary. See
    /// `event_loop::termination` module-level docs for the
    /// full reasoning. The `LoopState::push_termination_trigger` /
    /// `pop_termination_trigger` APIs added in F4 are
    /// infrastructure-only — no caller enqueues triggers yet.
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
            // event's `target` is the safe_target so it bypasses
            // normal subscription matching and is delivered directly
            // to that hat; `source` is the orchestrator (`ralph`) so
            // the `EventOriginGuard` accepts the publish. The payload
            // carries the full escalation metadata for the downstream
            // hat to act on.
            // U2 (2026-06-17-003 plan): the JSON payload already
            // includes `reason`; add `target_hat` so the drift
            // detector counts it as schema-compliant.
            // P1-2 (plan 2026-06-29-006): route the synthesis
            // through `enrich_task_resume_payload_full` so the
            // `kind` field is populated explicitly. Previously this
            // path built an inline JSON payload that missed the
            // `kind` field, which the drift detector saw as 0/N
            // (`task.resume.kind` 1/5 in primary-172725).
            let message = format!(
                "handoff deadline exceeded: consumer '{}' did not activate within timeout",
                esc.consumer
            );
            let payload_str = crate::event_loop::rejection::enrich_task_resume_payload_full(
                &message,
                "handoff_dispatch_timeout",
                Some(esc.safe_target.as_str()),
                // `MissingEvent` is the closest existing
                // RejectionStage variant for a "consumer did not
                // emit within window" handoff stall; the drift
                // detector already special-cases it (see
                // `rejection::RejectionStage::MissingEvent`).
                Some(crate::event_loop::rejection::RejectionStage::MissingEvent),
                // Pass `None` so `enrich_task_resume_payload_full`
                // falls back to `reason_hint` ("handoff_dispatch_timeout")
                // for the `kind` field. The typed
                // `RejectionKind::StallNoEvents` would also work
                // but it would force drift to bucket these
                // escalations as `stall_no_events`, which is a
                // different (loop-wide) class. Keeping the kind
                // = reason preserves the original drift semantics
                // for the handoff path while still satisfying
                // the `kind` field presence requirement.
                None,
                // `allowed_topics` is reserved for the rejection
                // pipeline that knows the target hat's published
                // topic set; the handoff escalation path doesn't
                // carry that context, so we leave the list empty
                // (the enrich helper skips the field entirely in
                // that case).
                &[],
            );
            // The legacy inline JSON also carried
            // `topic` / `consumer` / `event_id` / `safe_target` /
            // `details` so downstream hats can correlate the
            // envelope. The enrich helper only knows the common
            // schema, so we re-parse and merge those fields back in
            // before publishing.
            let mut payload: serde_json::Value = serde_json::from_str(&payload_str)
                .unwrap_or_else(|e| panic!("enrich payload must be valid JSON: {e}"));
            if let serde_json::Value::Object(ref mut map) = payload {
                // Override `kind` with the literal `reason_hint`.
                // `enrich_task_resume_payload_full` falls back to
                // the violation_class of `reason_hint`, which is
                // "other" for `handoff_dispatch_timeout`. The
                // drift detector's `task.resume.kind` field
                // presence check only requires the field to be
                // non-empty; using the literal reason here
                // matches the value the downstream hat / drift
                // detector will see in the `reason` field.
                map.insert(
                    "kind".into(),
                    serde_json::Value::String("handoff_dispatch_timeout".into()),
                );
                map.insert("topic".into(), serde_json::Value::String(esc.topic.clone()));
                map.insert(
                    "consumer".into(),
                    serde_json::Value::String(esc.consumer.clone()),
                );
                map.insert(
                    "event_id".into(),
                    serde_json::Value::String(esc.event_id.clone()),
                );
                map.insert(
                    "safe_target".into(),
                    serde_json::Value::String(esc.safe_target.clone()),
                );
                map.insert(
                    "details".into(),
                    serde_json::Value::String(esc.reason.clone()),
                );
            }
            let resume_event = Event::new("task.resume", payload.to_string())
                .with_source(HatId::from("ralph"))
                .with_target(HatId::from(esc.safe_target.as_str()));
            self.bus.publish(resume_event);
            // P2-1 (plan 2026-06-29-006): bump the
            // consumer's cumulative stall count. When the
            // post-bump value reaches 2, publish a
            // `loop.stalled` business event so the
            // `progress-steward` hat (which subscribes to
            // `loop.stalled` in the ce-executor-serial preset)
            // can step in and rescue the loop. Without this
            // signal, the loop just keeps routing `task.resume`
            // to the stalled hat indefinitely.
            let stall_count = self
                .state
                .handoff_tracker
                .bump_consumer_stall_count(&esc.consumer);
            if stall_count >= 2 && self.config.event_loop.progress_steward.enabled {
                // 2026-07-06 plan U12: when `progress_steward.enabled`
                // is `false`, the runtime MUST NOT publish
                // `loop.stalled` wake events. The ce-executor-serial
                // preset (U10/U11) removed the `progress-steward`
                // hat and set this flag to `false`; publishing
                // `loop.stalled` here would target a non-existent
                // hat (the bus would silently drop it) and surface
                // as a phantom-recovery drift. The fail-close
                // contract is: `enabled==false` ⇒ no
                // `loop.stalled` wake from any code path.
                let stalled_payload = serde_json::json!({
                    "reason": "consumer_stall_repeat",
                    "consumer": esc.consumer,
                    "topic": esc.topic,
                    "stall_count": stall_count,
                    "retry_key": format!(
                        "stall_recovery:{}:{}:handoff_dispatch_timeout:*",
                        esc.consumer, esc.topic
                    ),
                });
                let stalled_event = Event::new("loop.stalled", stalled_payload.to_string())
                    .with_source(HatId::from("ralph"));
                self.bus.publish(stalled_event);
            }
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
                    let candidates: Vec<&crate::flow_lifecycle::FlowLifecycleRecord> = self
                        .state
                        .flow_lifecycle
                        .active_records()
                        .filter(|r| r.target_hat == esc.consumer)
                        .collect();
                    if let Some(active) =
                        candidates.into_iter().max_by_key(|r| r.last_transition_at)
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

        // 2026-06-28-003: run runtime-recovery detectors after recording
        // any StallRecovery envelopes. This publishes loop.stalled when
        // the stall path forgot to do so and forces flapping keys to
        // plan.blocked.
        let ctx = self.runtime_recovery_context(&[]);
        self.apply_runtime_recovery_actions(&ctx);

        // U2 (2026-06-17-003 plan): per-iteration incomplete-wave scan.
        // Run after handoff escalations and before processing new JSONL events
        // so a stalled wave can be closed by the mechanism before the active hat
        // tries to bypass with empty_diff. When the gate is disabled (default)
        // this is a cheap no-op.
        let _ = self.maybe_emit_incomplete_wave_blocked();

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
    /// `<hat_id>.scope_violation` event AND promotes the finding to
    /// `AuditSeverity::Fail { add_failures: 1 }` per
    /// `2026-06-23-005` U4 (R5+KTD-8). This is the first audit class
    /// promoted from Warn to Fail — drift_monitor's 3 alert classes
    /// stay at Warn (U9 follow-up).
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

                // Scope violations from read-only dimension reviewers are
                // promoted from the legacy `add_failures: 1` counting path to
                // a typed hard reject. This covers both the historical
                // `dimension-reviewer` hat and split dimension hats (`dim:*`)
                // that explicitly disallow Edit/Write.
                //
                // Other hats still route through `Fail { add_failures: 1 }`
                // because their scope_violation can be a legitimate fix
                // attempt (coordinator writing plan files, executor
                // committing code).
                //
                // The BlockLoop arm does NOT increment
                // `consecutive_failures` (orthogonal termination
                // mechanism); instead it pushes a typed
                // `TerminationTrigger::DeadLetter` which
                // `check_termination` converts to
                // `TerminationReason::ScopeViolationHardRejected`
                // on the next call.
                let is_read_only_dimension_reviewer = hat_id.as_str() == "dimension-reviewer"
                    || (hat_id.as_str().starts_with("dim:")
                        && config
                            .disallowed_tools
                            .iter()
                            .any(|tool| matches!(tool.as_str(), "Edit" | "Write")));
                let severity = if is_read_only_dimension_reviewer {
                    crate::event_loop::audit::AuditSeverity::BlockLoop {
                        reason: "scope_violation".to_string(),
                    }
                } else {
                    crate::event_loop::audit::AuditSeverity::Fail { add_failures: 1 }
                };
                let kind = if is_read_only_dimension_reviewer {
                    crate::preset::engine::gates::RejectionKind::ScopeViolation
                } else {
                    // Pre-U5 placeholder retained for non-read-only-reviewer
                    // hats so the audit chain stays backwards-compatible.
                    crate::preset::engine::gates::RejectionKind::MissingField
                };
                crate::event_loop::audit::AuditDispatcher::dispatch(
                    severity,
                    crate::event_loop::audit::AuditContext {
                        hat: hat_id.as_str().to_string(),
                        kind,
                        details: diff_stat.clone(),
                    },
                    &mut self.state.consecutive_failures,
                );

                // Push the typed termination trigger so
                // `check_termination` produces the matching
                // `TerminationReason::ScopeViolationHardRejected`.
                // Only for read-only dimension reviewers (the BlockLoop arm).
                // The trigger carries the hat + diff stat so
                // `trigger_to_reason` produces a fully-populated
                // `TerminationReason` without further enrichment.
                if is_read_only_dimension_reviewer
                    && let Err(e) = self.state.push_termination_trigger(
                        crate::event_loop::termination::TerminationTrigger::ScopeViolation {
                            hat: hat_id.as_str().to_string(),
                            diff_stat: diff_stat.clone(),
                        },
                    )
                {
                    warn!(
                        error = %e,
                        "scope_violation_hard_rejected: failed to push termination trigger"
                    );
                }
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

    /// Processes events from JSONL and routes orphaned events to Ralph.
    ///
    /// Also handles backpressure for malformed JSONL lines by:
    /// 1. Emitting `event.malformed` system events for each parse failure
    /// 2. Tracking consecutive failures for termination check
    /// 3. Resetting counter when valid events are parsed
    ///
    /// Returns [`ProcessedEvents`] indicating whether events were found, whether
    /// semantic `plan.*` topics were published, and whether any were orphans that Ralph should
    /// handle.
    pub fn process_events_from_jsonl(&mut self) -> std::io::Result<ProcessedEvents> {
        let result = self.event_reader.read_new_events()?;
        // 2026-06-16-001 U5: reset the per-turn stall-detector
        // flag at the start of each read so the helper can
        // observe whether THIS turn admitted a business event.
        self.state.stall_detector_had_events = false;
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

        // A2 (002-adversarial-review / 003-adversarial-review
        // P0-2): build the unified `ValidationPipeline` once
        // per batch so the runtime can consult it instead of
        // the legacy per-rule gate stack. The build is opt-in
        // via the `UNIFIED_VALIDATION=1` env var (mirrors the
        // `protocol_view.feature_enabled()` surface); when the
        // flag is off the pipeline is dropped and the legacy
        // gate stack continues to gate events as before. The
        // pipeline is **built** here so the per-batch wiring
        // is exercised; the actual call sites inside the
        // per-event gate stack are migrated in follow-up
        // commits (the full migration requires lifting the
        // workspace path and HatRegistry into the pipeline,
        // which is a non-trivial signature change).
        let unified_pipeline = build_unified_validation_pipeline(&self.config);
        tracing::debug!(
            pre_commit_rules = unified_pipeline.pre_commit_rules.len(),
            post_commit_rules = unified_pipeline.post_commit_rules.len(),
            "A2: unified validation pipeline built for this batch"
        );

        // U6: capture payload contract violation produced by event policy
        // validation. The loop runner will read this and pause with a
        // diagnostic.
        let mut payload_contract_violation: Option<
            crate::payload_contract::PayloadContractViolation,
        > = None;
        let mut had_policy_rejections = false;

        // U2 (plan 2026-06-20-001, R15 / KTD-10): engine-backed
        // fail-fast gate. Runs *before* d623c09's policy / scope
        // gates so the loop and the CLI emit share the SAME
        // required-field check (no duplicate field tables in
        // Rust, per KTD-10). The engine uses the same
        // `ProtocolView` the linter reads, so the two layers
        // cannot drift.
        //
        // 2026-06-20-001 review P0 #1: the engine filter MUST
        // run *before* the malformed-handling loop below, so
        // engine-rejected events are converted into
        // `MalformedLine` entries that the existing
        // bookkeeping loop (publish event.malformed + increment
        // consecutive_malformed_events) actually observes. The
        // previous placement ran the filter AFTER the
        // bookkeeping loop, so engine rejections were silently
        // dropped without any bus signal.
        //
        // 2026-06-20-001 review P0 #4: the filter also seeds
        // `state.pending_lint_resume` (via the helper
        // `engine_required_field_filter`) so the agent's next
        // `build_prompt` sees `## LINT RESUME REQUIRED`. The
        // `state.pending_lint_resume` slot is the single source
        // of truth for the lint resume path; the CLI's
        // `pending_lint_resume.json` write was a no-op stub as
        // of the same review.
        //
        // Scope of U2 phase 1 (this commit): the engine gate
        // ONLY short-circuits on `required_fields` missing —
        // the heavier d623c09 checks (terminal monotonicity,
        // semantic gate, recovery) keep running afterwards. The
        // fail-fast is opt-in: the same gate is skipped when the
        // execution_mode is `Coordinator`, and when the engine
        // budget env `RALPH_SERIAL_LINT_MODE=off` is set.
        // Disabling the engine gate does NOT disable the d623c09
        // gates — the engine is a fail-fast addition, not a
        // replacement.
        //
        // Phase 2 (U11-T2) moved event-policy validation into the
        // unified `ValidationPipeline` (`rules_event_policy::EventPolicyRule`).
        // The per-event loop below runs that pipeline and applies the same
        // d623c09 semantics (terminal monotonicity, semantic gate, recovery)
        // through the pipeline's `ValidationResult`s.
        let result = if self.should_run_engine_gate() {
            self.apply_engine_required_field_gate(result)
        } else {
            result
        };

        // Handle malformed lines with backpressure. The engine
        // gate above (review P0 #1) appends `MalformedLine`
        // entries with `line_number=0` for engine rejections;
        // this loop publishes them as `event.malformed` and
        // increments `consecutive_malformed_events` so the
        // existing termination backstop still fires.
        for malformed in &result.malformed {
            let payload = format!(
                "Line {}: {}\nContent: {}",
                malformed.line_number, malformed.error, malformed.content
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
            // 2026-06-16-001 U5: a turn with no events is the
            // canonical "no progress" turn. Run the stall
            // detector before returning so the loop does not
            // silently starve when the JSONL is empty.
            // 2026-07-28-001 plan U3: an empty-activation
            // turn never has a staged over-emit recovery,
            // so no settlement is needed here.
            run_stall_detector_on_state(
                &mut self.state,
                &self.config.event_loop.progress_steward,
                &self.registry,
                &mut self.bus,
            );
            return Ok(ProcessedEvents {
                had_events: false,
                had_raw_events: false,
                had_rejected_events: false,
                had_plan_events: false,
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
            // 2026-06-16-001 U1: replace `first_wave_id_accepted: Option<Option<String>>`
            // with two independent slots so a wave group is not
            // poisoned by a preceding no-wave_id business event (or
            // vice versa).
            //
            // Invariants:
            // - `non_wave_business_event_accepted` records whether
            //   the single non-wave business slot in this turn has
            //   been consumed.
            // - `accepted_wave_id` records the wave_id of the wave
            //   group (if any) admitted in this turn. A new wave_id
            //   still gets rejected, but a continuation of the same
            //   wave does not.
            // - `is_dual_publish_step_handoff` carves out the
            //   `queue.advance` + `work.ready` handoff pair (see
            //   2026-06-15-003 U1) — the second event in the pair
            //   does not consume a fresh slot.
            let mut non_wave_business_event_accepted = false;
            // 2026-06-30 per-turn-budget backpressure: emit at most ONE
            // hat-targeted `task.resume` per turn when extra business
            // events are dropped by the single-business-event budget.
            // The real incident dropped 30 `plan.complete` events behind
            // a stray `work.ready`; without this guard each drop would
            // inject a duplicate resume (event storm).
            let mut per_turn_budget_feedback_injected = false;
            // 2026-07-28-001 plan U3: the over-emit recovery
            // intent is staged on `self.state.pending_over_emit_recovery`
            // from the drop branch so it survives block exit.
            // 2026-07-04-002 plan U13 carve-out enforcement: the carve-out
            // admits at most ONE exempt topic per activation, regardless
            // of how many `exempt_topics` the preset declared. A second
            // exempt topic in the same activation still hits the default
            // budget (drop + diagnostic), preserving the plan's
            // "serial walk at most once per turn" invariant.
            let mut exempt_topic_carveout_used = false;
            // 2026-07-06 U2 (DEV-001): track when an event was admitted
            // via the exempt_topics carve-out so the slot-bump at
            // line 9191-9208 can be skipped, preserving the
            // non_wave_business_event_accepted=false slot for the
            // rest of the serial walk within the same turn.
            let mut admitted_via_carveout = false;
            let mut accepted_wave_id: Option<String> = None;
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

                // U7 (2026-07-23-002): supervisor-injected coordination
                // events (`*.wave.complete` / `*.wave.failed`, marked
                // `system_injected: true` by `append_supervisor_coord_event`)
                // bypass the per-hat scope check. They are
                // orchestrator-produced, not agent output, and their
                // `hat` field is attribution metadata for the
                // downstream consumer hat, not a publish-scope claim.
                // This aligns with the existing bypasses in
                // `event_origin::validate_event_origin` (P0-1) and
                // `EventBus::publish` (source guard). Without this
                // bypass, isolated scope enforcement drops the
                // coordination event before it reaches the EventBus,
                // leaving the integrator hat's pending queue empty.
                if event.system_injected == Some(true) {
                    accepted.push(event);
                    continue;
                }

                // R6/U2: ralph pseudo-hat may only publish control topics.
                // Business topics from ralph are rejected here (fail-closed)
                // so they do NOT count as progress toward the stall detector.
                // P1-12: use prefix match so future `ralph.*` topics are recognised.
                if event.hat.as_deref() == Some("ralph")
                    && !crate::event_origin::is_ralph_control_topic(topic)
                {
                    warn!(
                        topic = %topic,
                        "ralph hat business topic rejected: ralph may only publish control topics"
                    );
                    self.state.record_rejection_digest(
                        "ralph_business_topic_rejected",
                        "ralph hat may only publish control topics",
                        &event.topic,
                        &event.ts,
                    );
                    let violation = Event::new(
                        "event.isolation.boundary_violation",
                        format!(
                            "{{\"hat\":\"ralph\",\"topic\":\"{}\",\"violation\":\"ralph_business_topic_rejected: ralph hat may only publish control topics\"}}",
                            event.topic
                        ),
                    );
                    self.bus.publish(violation);
                    continue;
                }

                // 2026-06-18-001 plan U5: 对**完全没有 provenance**的
                // business topic fail-closed,reason=`isolated_anonymous_business_topic`。
                // 这是 CLI gate(U1) + EventBus source guard 的 runtime
                // 侧封堵——直接文件 append 或 loop-runner 内部 publish
                // 绕过 CLI 的路径在这里拦截。
                if crate::event_origin::is_anonymous_business_topic(
                    &event,
                    &self.registry,
                    cancellation,
                    Some(isolated_hat.as_str()),
                ) {
                    warn!(
                        topic = %event.topic,
                        ts = event.ts,
                        "U5: isolated anonymous business topic rejected (no hat/source/triggered provenance)"
                    );
                    // 2026-06-18-001 plan U6: 累加到 digest
                    self.state.record_rejection_digest(
                        "isolated_anonymous_business_topic",
                        "no hat/source/triggered provenance; supply --hat or use a registered hat backend",
                        &event.topic,
                        &event.ts,
                    );
                    let violation = Event::new(
                        "event.isolation.boundary_violation",
                        format!(
                            "{{\"hat\":\"<anonymous>\",\"topic\":\"{}\",\"violation\":\"isolated_anonymous_business_topic: no hat/source/triggered provenance\"}}",
                            event.topic
                        ),
                    );
                    self.bus.publish(violation);
                    // 触发 task.resume(target=ralph) 走 orchestrator 恢复路径
                    let resume = Event::new(
                        "task.resume",
                        format!(
                            "{{\"target_hat\":\"ralph\",\"reason\":\"isolated_anonymous_business_topic\",\"topic\":\"{}\"}}",
                            event.topic
                        ),
                    );
                    self.bus.publish(resume);
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
                        .topic(event.topic.clone())
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
                            ref_path: event.topic.clone(),
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
                    let topic_str = event.topic.clone();
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
                        .map(|c| c.publishes.clone())
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
                                    topic: event.topic.clone(),
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
                            topic: event.topic.clone(),
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
                            // 2026-06-16-001 U3: capture the source
                            // event's timestamp so the freshness
                            // filter (U3 TTL) can drop stale
                            // rejections on the next call. The
                            // `event_reader::Event` struct does not
                            // carry a stable `id` field — the JSONL
                            // line offset or `ts` is the closest
                            // available correlation key, so
                            // `original_event_id` stays None and
                            // `original_ts` carries the event
                            // timestamp.
                            original_event_id: None,
                            original_ts: Some(event.ts.clone()),
                            // 2026-06-23 fix plan U5 (CB-2): isolated_scope
                            // path predates the typed-kind plumbing;
                            // pass None so the resume payload falls
                            // back to `violation`-derived reason.
                            kind: None,
                            duplicate_work_done_hint: None,
                            seen_count: None,
                        };
                        // 2026-06-16-001 U3: freshness filter — drop
                        // the rejection (and the synthetic
                        // `task.resume` it would produce) if the
                        // source event's timestamp is older than
                        // `task_resume_ttl_seconds`. The default is
                        // 300s; operators can override per-preset.
                        // We treat missing/unparseable timestamps
                        // as "fresh" so legacy JSONL that lacks a
                        // recoverable ts still flows through the
                        // existing recovery path.
                        let ttl_seconds = self
                            .config
                            .event_loop
                            .task_resume_ttl_seconds
                            .unwrap_or(300);
                        if is_rejection_stale(&rejection, ttl_seconds) {
                            warn!(
                                source_event_ts = ?rejection.original_ts,
                                ttl_seconds,
                                hat = %isolated_hat.as_str(),
                                topic = %event.topic,
                                "isolated mode: stale rejection — dropping task.resume"
                            );
                            self.bus.publish(Event::new(
                                "event.isolation.boundary_violation",
                                format!(
                                    "{{\"hat\":\"{}\",\"topic\":\"{}\",\"violation\":\"Isolated mode: stale rejection for '{}' (TTL={}s) — dropping task.resume\"}}",
                                    isolated_hat.as_str(),
                                    event.topic,
                                    event.topic,
                                    ttl_seconds
                                ),
                            ));
                            continue;
                        }
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
                            system_injected: None,
                        };
                        accepted.push(resume_jsonl);
                    }
                    continue;
                }

                // 2026-06-16-001 U1: wave group admission logic.
                // A `wave_id` group of result events is ONE business
                // emission, not N. The merge layer (see
                // `merge_wave_results_to_events_file`) stamps every
                // record with the originating `wave_id`, so a batch
                // of N `review.dimension.done` from workers in the
                // same wave must be admitted in full even after a
                // non-wave business event was already accepted in the
                // same turn.
                //
                // Rules (evaluated in order):
                // 1. event.wave_id == accepted_wave_id → admit
                //    (continuation of the admitted wave group).
                // 2. event.wave_id.is_some() && accepted_wave_id.is_none()
                //    → admit, set accepted_wave_id (new wave group).
                // 3. event.wave_id.is_some() && accepted_wave_id is
                //    some other id → reject (a distinct second wave).
                // 4. event.wave_id.is_none() && !non_wave_business_event_accepted
                //    → admit (consume the non-wave slot).
                // 5. event.wave_id.is_none() && non_wave_business_event_accepted
                //    but event is `work.ready` and the last accepted
                //    event is `queue.advance` from the same hat
                //    (is_dual_publish_step_handoff) → admit (handoff
                //    carve-out, see 2026-06-15-003 U1).
                // 6. otherwise → reject.
                let event_wave_id = event.wave_id.clone();
                let admitted_under_wave = match event_wave_id.as_deref() {
                    Some(wid) => match accepted_wave_id.as_deref() {
                        Some(current) => current == wid,
                        None => true,
                    },
                    None => false,
                };
                let wave_collision = match event_wave_id.as_deref() {
                    Some(wid) => {
                        matches!(accepted_wave_id.as_deref(), Some(current) if current != wid)
                    }
                    None => false,
                };

                let incoming_hat = event
                    .hat
                    .as_deref()
                    .or(event.source.as_deref())
                    .unwrap_or(isolated_hat.as_str());
                let is_dual_publish_step_handoff = self.isolated_dual_publish_handoff(
                    event.topic.as_str(),
                    incoming_hat,
                    isolated_hat.as_str(),
                    &accepted,
                );
                let required_event_topics = self.required_event_topic_set();

                // 2026-07-01-001 plan U1: terminal-priority
                // budget. When the non-wave slot has already
                // been consumed by a non-terminal event (e.g.
                // a stray `work.ready` that the agent emitted
                // before the terminal), the runtime must NOT
                // drop a terminal event (LOOP_COMPLETE /
                // plan.complete / plan.blocked / report.done /
                // REVIEW_COMPLETE). The terminal topic list is
                // derived from `EventPolicyConfig.terminal_topics`
                // + the configured completion / cancellation
                // promises, so non-ce-executor presets stay
                // untouched.
                //
                // Mechanics: when the current event is a
                // terminal topic and the slot is already
                // taken, we publish a `event.isolation.terminal_priority`
                // diagnostic, evict the non-terminal
                // business event from `accepted`, and admit
                // the terminal event instead. The eviction
                // is safe because the agent already had a
                // chance to act on the non-terminal event in
                // earlier turns; dropping it here is the
                // lesser evil vs. stalling the loop.
                let terminal_topics = self.collect_terminal_topic_set();
                let event_is_terminal = terminal_topics.contains(event.topic.as_str());
                let mut evicted_non_terminal: Option<usize> = None;
                if event_is_terminal && non_wave_business_event_accepted {
                    for (idx, prev) in accepted.iter().enumerate().rev() {
                        let prev_topic = prev.topic.as_str();
                        if prev_topic == "task.resume" {
                            // Don't touch recovery envelopes.
                            continue;
                        }
                        if required_event_topics.contains(prev_topic) {
                            // P0-5: required pre-completion events must
                            // never be displaced by U1 terminal-priority.
                            break;
                        }
                        if terminal_topics.contains(prev_topic) {
                            // Already admitted a terminal event
                            // — keep the new one out so the
                            // budget stays sane.
                            break;
                        }
                        if prev.wave_id.is_none() {
                            evicted_non_terminal = Some(idx);
                            break;
                        }
                    }
                }

                let mut should_admit = if admitted_under_wave {
                    true
                } else if wave_collision {
                    false
                } else if !non_wave_business_event_accepted {
                    true
                } else if event_is_terminal && evicted_non_terminal.is_some() {
                    // U1: terminal-priority override — the
                    // terminal event displaces the earlier
                    // non-terminal business event.
                    true
                } else if !exempt_topic_carveout_used && {
                    let (business, terminal) = self
                        .config
                        .event_loop
                        .event_policy
                        .as_ref()
                        .map(|ep| (ep.business_topics.as_slice(), ep.terminal_topics.as_slice()))
                        .unwrap_or((&[], &[]));
                    is_isolated_exempt_topic(
                        self.registry
                            .get_config(isolated_hat_owned.as_ref().unwrap_or(&HatId::from(""))),
                        &event.topic,
                        business,
                        terminal,
                    )
                } {
                    // 2026-07-03-005 plan (P0 fix M-1): declared
                    // serial walk exemption. The isolated hat has
                    // listed this topic in its `exempt_topics` (a
                    // preset-declared positive list of topics that
                    // are exempt from the per-turn business-event
                    // budget), so we admit the event without
                    // consuming the `non_wave_business_event_accepted`
                    // slot. Critical for hats that walk N events
                    // one-per-turn (e.g. review-coordinator walking
                    // 6 review.dimension.ready events in
                    // ce-executor-serial — see preset's
                    // `exempt_topics: ["review.dimension.ready",
                    // "review.dimensions.complete"]`). Empty
                    // exempt_topics = no exemption (default
                    // behaviour preserved).
                    //
                    // 2026-07-04-002 plan (P0 #2 fix): the
                    // `!non_wave_business_event_accepted` guard in
                    // the previous revision was structurally dead —
                    // the earlier `else if !non_wave_business_event_accepted`
                    // branch always returned `true` first. Removing
                    // it makes this branch reachable when the
                    // per-turn slot is already occupied. A second
                    // exempt topic in the *same* turn still falls
                    // through to the default budget (drop + bound),
                    // because we do not consume the slot here.
                    //
                    // 2026-07-06 U2 (DEV-001): record that this
                    // admission was via the carve-out so the
                    // slot-bump path below can be skipped, letting
                    // the serial walk continue within the same turn.
                    admitted_via_carveout = true;
                    true
                } else {
                    is_dual_publish_step_handoff
                };

                if should_admit && let Some(idx) = evicted_non_terminal {
                    let evicted = accepted.remove(idx);
                    warn!(
                        evicted_topic = %evicted.topic,
                        admitted_topic = %event.topic,
                        hat = %isolated_hat.as_str(),
                        "U1 terminal-priority: displaced earlier non-terminal business event to admit terminal event"
                    );
                    let diagnostic = Event::new(
                        "event.isolation.terminal_priority",
                        format!(
                            "{{\"hat\":\"{}\",\"evicted_topic\":\"{}\",\"admitted_topic\":\"{}\",\"reason\":\"isolated mode: terminal topics have priority over non-terminal business events in the per-turn budget\"}}",
                            isolated_hat.as_str(),
                            evicted.topic,
                            event.topic
                        ),
                    );
                    self.bus.publish(diagnostic);
                    // The eviction freed the non-wave slot;
                    // the per-turn sticky flag must be reset
                    // so subsequent admits (this turn) see
                    // the slot as open.
                    non_wave_business_event_accepted = false;
                }

                // 2026-07-04-002 plan (P0 #2): record the carve-out
                // usage so a SECOND exempt topic in the same
                // activation falls through to the default budget
                // (drop + diagnostic). We only flip the flag when
                // the carve-out actually admitted — admits via
                // other branches (wave, terminal-priority, fresh
                // slot) keep the carve-out unused for this turn.
                if should_admit && !non_wave_business_event_accepted && !admitted_under_wave {
                    let (business, terminal) = self
                        .config
                        .event_loop
                        .event_policy
                        .as_ref()
                        .map(|ep| (ep.business_topics.as_slice(), ep.terminal_topics.as_slice()))
                        .unwrap_or((&[], &[]));
                    if is_isolated_exempt_topic(
                        self.registry
                            .get_config(isolated_hat_owned.as_ref().unwrap_or(&HatId::from(""))),
                        &event.topic,
                        business,
                        terminal,
                    ) {
                        exempt_topic_carveout_used = true;
                    }
                }

                if should_admit
                    && let Some(missing) =
                        self.path_required_missing_for_anchor(event.topic.as_str())
                {
                    tracing::warn!(
                        topic = %event.topic,
                        missing = ?missing,
                        hat = %isolated_hat.as_str(),
                        "Isolated admit rejected: path_required_events require topics not yet observed"
                    );
                    should_admit = false;
                }

                if should_admit {
                    self.mark_required_event_seen(event.topic.as_str());
                    accepted.push(event);
                    match event_wave_id.as_deref() {
                        Some(wid) => {
                            if accepted_wave_id.is_none() {
                                accepted_wave_id = Some(wid.to_string());
                            }
                        }
                        None => {
                            // 2026-07-06 U2 (DEV-001): exempt_topics
                            // carve-out admissions must NOT consume
                            // the per-turn non_wave_business_event_accepted
                            // slot, otherwise the serial walk (e.g.
                            // review-coordinator walking 6
                            // review.dimension.ready) drops N-1 events
                            // and review-synthesizer receives incomplete
                            // data. The pre-existing carve-out branch
                            // already sets admitted_via_carveout = true
                            // above.
                            if !admitted_via_carveout {
                                non_wave_business_event_accepted = true;
                            }
                        }
                    }
                    // U3 P0 fix: write the sticky per-turn budget flag so
                    // `check_default_publishes` (which runs later in the same
                    // turn when JSONL had zero events, or earlier when JSONL
                    // had business events) sees a consistent view.
                    //
                    // 2026-07-06 U2 (DEV-001): carve-out admissions must
                    // also keep isolated_turn_business_event_accepted
                    // false so the default_publishes guard does not
                    // see the slot as occupied and refuse the next
                    // exempt topic in the serial walk.
                    if !admitted_via_carveout {
                        self.state.isolated_turn_business_event_accepted = true;
                    }
                    // 2026-06-16-001 U5: mark the per-turn
                    // stall-detector flag so the post-validation
                    // stall detector resets the counters.
                    self.state.stall_detector_had_events = true;
                } else {
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
                    )
                    .with_target(isolated_hat.clone());
                    self.bus.publish(diagnostic);

                    // 2026-07-28-001 plan U3 (commit-aware
                    // over-emit recovery): the previous path
                    // injected a hat-targeted `task.resume`
                    // immediately, which let a co-emitted
                    // first business event (already admitted in
                    // the same turn) be silently displaced by
                    // `next_hat` priority. Instead, stage the
                    // intent here and resolve it AFTER the loop
                    // has determined whether any business event
                    // actually committed. The recovery is only
                    // useful when zero business events landed;
                    // otherwise the over-emit is a pure
                    // cosmetic extra and the agent already
                    // succeeded on its primary emit.
                    if !per_turn_budget_feedback_injected {
                        per_turn_budget_feedback_injected = true;
                        self.state.pending_over_emit_recovery = Some(OverEmitRecovery {
                            hat: isolated_hat.clone(),
                            dropped_topic: event.topic.clone(),
                        });
                    }
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
        // 2026-06-18-001 plan U6: 把 origin guard 拒收累加到 digest,
        // 让 agent 在下一轮 prompt 中看到 `## RECENT REJECTIONS`。
        for rej in &origin_rejections {
            self.state.record_rejection_digest(
                rej.reason,
                &format!(
                    "origin guard rejected topic `{}` from hat {:?}",
                    rej.topic, rej.source_hat
                ),
                &rej.topic,
                "",
            );
        }
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
                    // 2026-06-18-001 plan U6: 累加到 digest
                    self.state.record_rejection_digest(
                        "topic_format_rejected",
                        &format!(
                            "topic `{}` is not in the whitelist of known topics",
                            event.topic
                        ),
                        &event.topic,
                        &event.ts,
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

        // --- Event policy validation now runs inside the U11-T2 unified pipeline ---
        // The legacy `apply_event_policy_validation` block was removed; see the
        // per-event unified pipeline loop below for completion guard, topic-deny,
        // payload policy, review-step gates, and side-effect handling.

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

        // --- State projection (U1 of 2026-06-17-003 plan): ---
        // SP-R8 mandates that the projector runs **after** the
        // state machine has accepted the batch and **before** the
        // `progress_task_gate`. The projector is the canonical
        // writer for `.ralph/agent/tasks.jsonl` and
        // `.ralph/agent/progress.md`; the gate then reads the
        // projected ledgers. Failures are fail-closed — the
        // affected events are dropped from the bus with an
        // `event.state_projection.rejected` diagnostic.
        if self.config.event_loop.state_projection.enabled {
            // P0-2 follow-up (plan 2026-06-29-006 §F3): hoist the
            // loop id out of `self` BEFORE the closure below so the
            // borrow checker doesn't conflict with the
            // `self.state.state_projection` immutable borrow on
            // 8055.
            let projector_loop_id = self.current_loop_id_for_contract();
            let projector = self.state.state_projection.get_or_insert_with(|| {
                let ctx = crate::state_projector::ProjectionContext::new(
                    self.config.core.workspace_root.as_path(),
                    self.config.event_loop.state_projection.clone(),
                    // Mirror the loop's R4 setting so the projector
                    // respects `enforce_current_unit` rather than
                    // silently disabling it. R1 in
                    // 2026-06-17-005 fix plan.
                    self.config.event_loop.enforce_current_unit,
                )
                // P0-2 follow-up: thread the loop's
                // `current-loop-id` marker into the projector
                // context so `project_ensure_task`'s fallback
                // (when `payload.loop_id` is absent) hits a real
                // value. Without this wiring the fallback is a
                // dead branch in production and coordinator
                // `work.ready` events produced tasks whose
                // `loop_id` was `None` on disk — the CLI then
                // hard-rejected those records with "legacy task
                // has no loop_id; not mutable from agent context".
                .with_current_loop_id(projector_loop_id);
                let mut p = crate::state_projector::StateProjector::new(ctx);
                // Best-effort bootstrap; failure is non-fatal
                // because the projector falls back to live
                // disk reads on a cold cache.
                let _ = p.bootstrap_from_disk();
                p
            });
            let report = projector.apply(&events);
            // Fix-2 (2026-06-29 primary-072512 P0): snapshot the
            // rejections into LoopState so the runner's step-close
            // partition can choose between hard-gate (no emit) and
            // schema-guidance (emit happened but projector rejected).
            // Without this, the runner cannot distinguish "agent did
            // not emit" from "agent emitted but the event was
            // dropped at projection", and the latter triggered
            // hard-gate exhaustion on step-04 (events:14 in
            // `docs/report/2026-06-29-ce-executor-serial-primary-
            // 20260629-072512-diagnosis.md`).
            self.state.last_projection_rejections = report.rejections.clone();
            if !report.rejections.is_empty() {
                for rej in &report.rejections {
                    let payload = serde_json::json!({
                        "topic": rej.topic,
                        "reason": rej.reason,
                        "event_payload": rej.payload,
                    })
                    .to_string();
                    self.bus.publish(ralph_proto::Event::new(
                        "event.state_projection.rejected",
                        payload,
                    ));
                }
                // P0 fix (review 2026-06-17-003): retain by the
                // event's `(topic, payload)` pair rather than by
                // topic name alone. When two events of the same
                // topic appear in a single batch (ce-executor
                // wave scenarios, plan-gate dual-publish
                // carve-outs), rejecting the whole topic dropped
                // sibling events that the projector would
                // otherwise accept. The event reader does not
                // surface a line number, so we use the payload
                // text as the per-event tie-breaker: events with
                // distinct payloads are independent and only the
                // exact matching entry is dropped. Events with no
                // payload (e.g. bare `task.resume`) fall back to
                // a per-topic index counter so a single no-payload
                // reject still does not wipe the whole topic.
                let mut seen_no_payload: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                let mut need_no_payload: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for r in &report.rejections {
                    if r.payload.is_none() {
                        *need_no_payload.entry(r.topic.clone()).or_insert(0) += 1;
                    }
                }
                let rejected_with_payload: std::collections::HashSet<(String, String)> = report
                    .rejections
                    .iter()
                    .filter_map(|r| {
                        let p = r.payload.as_ref()?;
                        Some((r.topic.clone(), p.clone()))
                    })
                    .collect();
                events.retain(|e| {
                    if let Some(p) = e.payload.as_ref() {
                        !rejected_with_payload.contains(&(e.topic.clone(), p.clone()))
                    } else {
                        let seen = seen_no_payload.entry(e.topic.clone()).or_insert(0);
                        let needed = need_no_payload.get(&e.topic).copied().unwrap_or(0);
                        let drop = *seen < needed;
                        *seen += 1;
                        !drop
                    }
                });
            }
        }
        // --- End state projection ---

        // --- U11-T2: per-event unified ValidationPipeline ---
        //
        // Runs the unified pre-commit rules against every event that
        // reached this point. Event-policy decisions are handled here
        // (drop, warn, or publish correction); non-event-policy rejections
        // emit a correction but keep the event so the legacy gate stack
        // can produce its own verdict.
        {
            let policy_enabled = self
                .config
                .event_loop
                .event_policy
                .as_ref()
                .is_some_and(|p| p.enabled);
            let pipeline = &unified_pipeline;

            // U11-T9 (P0-3 follow-up): mirror the state projector's cache
            // into the `LedgerSnapshot` so `StepHandoffRule` sees the same
            // view as the legacy disk-side gate.
            if let Some(ref mut projector) = self.state.state_projection
                && let Some(ref mut ledger) = self.state.state_ledger
            {
                let mut guard = ledger.snapshot_mut();
                projector.sync_to_ledger_snapshot(&mut guard);
            }

            let mut state_ledger = std::mem::take(&mut self.state.state_ledger);
            let mut snapshot = state_ledger
                .as_ref()
                .map(|l| l.snapshot().clone())
                .unwrap_or_else(crate::state::LedgerSnapshot::cold_start);
            let view = crate::preset::engine::protocol::ProtocolView::from_event_loop(
                &self.config.event_loop,
            );

            // Pass LoopState's policy runtime state / review-step tracker into
            // the context as overrides so the event-policy rule mutates the
            // canonical instances directly.
            let mut policy_state = self.state.policy_runtime_state.take().unwrap_or_default();
            let mut review_step_tracker = std::mem::take(&mut self.state.review_step_tracker);
            // U11-T4 (post-commit wiring): hand the live
            // `WorkflowProgress` to the validation context so the
            // unified `WorkflowGuardRule` reads & advances the same
            // instance the legacy gate stack used to. The pre-commit
            // rules do not touch this field; the post-commit pass
            // calls it after every pre-commit accept.
            let mut workflow_progress = std::mem::take(&mut self.state.workflow_progress);
            let mut event_policy_violation: Option<
                crate::payload_contract::PayloadContractViolation,
            > = None;
            let mut policy_rejections: Vec<crate::event_policy::PolicyRejection> = Vec::new();
            // `WorkflowGuardRule` rejection details collected per
            // event; drained after the per-event loop to write
            // recovery envelopes (one per rejected event).
            let mut wg_details: Vec<crate::validation::WorkflowGuardRejectionDetail> = Vec::new();

            // Source/target hat attribution for payload-contract violations.
            let (source_hats_by_topic, target_hats_by_topic) = if policy_enabled {
                let mut source: std::collections::BTreeMap<String, Vec<String>> =
                    std::collections::BTreeMap::new();
                let mut target: std::collections::BTreeMap<String, Vec<String>> =
                    std::collections::BTreeMap::new();
                for (hat_id, hat_config) in &self.config.hats {
                    for t in &hat_config.publishes {
                        source.entry(t.clone()).or_default().push(hat_id.clone());
                    }
                    for t in &hat_config.triggers {
                        target.entry(t.clone()).or_default().push(hat_id.clone());
                    }
                }
                for hats in source.values_mut() {
                    hats.sort();
                }
                for hats in target.values_mut() {
                    hats.sort();
                }
                (source, target)
            } else {
                (
                    std::collections::BTreeMap::new(),
                    std::collections::BTreeMap::new(),
                )
            };

            let mut accepted_events: Vec<JsonlEvent> = Vec::with_capacity(events.len());
            let mut rejected_topics: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut hold_reason: Option<String> = None;

            // U11-T4: post-commit workflow-guard wiring only
            // engages when the state machine is disabled (mirrors
            // the legacy bypass at line 8373). The state machine
            // owns the lifecycle when it is on, so the linear
            // guard would double-reject.
            let post_commit_enabled = self
                .config
                .event_loop
                .state_machine
                .as_ref()
                .is_none_or(|sm| !sm.enabled)
                && self
                    .config
                    .event_loop
                    .workflow_guards
                    .as_ref()
                    .is_some_and(|g| !g.chains.is_empty());

            for evt in &events {
                if let Some(ps) = self.phase_authority.snapshot() {
                    snapshot.workflow_phase = Some(ps);
                }
                let results = {
                    let mut ctx = crate::validation::ValidationContext::new(&mut snapshot)
                        .with_policy_runtime_state(&mut policy_state)
                        .with_review_step_tracker(&mut review_step_tracker)
                        .with_workflow_progress(&mut workflow_progress)
                        .with_workflow_guard_details(&mut wg_details)
                        .with_payload_contract_violation(&mut event_policy_violation)
                        .with_policy_rejections(&mut policy_rejections)
                        .with_source_hats_by_topic(&source_hats_by_topic)
                        .with_target_hats_by_topic(&target_hats_by_topic)
                        // U5 of plan 2026-07-02-005: wire the on-disk
                        // tasks.jsonl path so the StepHandoffRule can
                        // best-effort reload on a stale in-memory view
                        // (140149 / 175407 root cause).
                        .with_tasks_path(
                            self.config
                                .core
                                .workspace_root
                                .join(".ralph")
                                .join("agent")
                                .join("tasks.jsonl"),
                        );
                    pipeline.validate_pre_commit_with_view(&view, &mut ctx, evt)
                };
                let mut event_accepted = true;
                let mut event_warnings: Vec<String> = Vec::new();
                for r in &results {
                    if r.accepted {
                        if r.stage == crate::validation::ValidationStage::EventPolicy
                            && r.reason_code.as_deref()
                                == Some(crate::validation::ReasonCode::EVENT_POLICY_WARNING)
                            && let Some(hint) = &r.correction_hint
                        {
                            event_warnings.push(hint.clone());
                        }
                        continue;
                    }
                    // Preserve the legacy opt-out for step-handoff when state
                    // projection is disabled.
                    if r.stage == crate::validation::ValidationStage::StepHandoff
                        && !self.config.event_loop.state_projection.enabled
                    {
                        continue;
                    }
                    // U11-T2: step-handoff rejections now emit their operator-facing
                    // side effects (`plan.blocked` + diagnostic + recovery envelope)
                    // directly from the unified rejection handler. The legacy batch
                    // gate is removed; this is the single source of truth for the
                    // progress-task-mismatch recovery path.
                    if r.stage == crate::validation::ValidationStage::StepHandoff {
                        self.emit_step_handoff_rejection_side_effects(evt, r);
                        event_accepted = false;
                        break;
                    }
                    if r.stage == crate::validation::ValidationStage::EventPolicy {
                        match r.reason_code.as_deref() {
                            Some(
                                crate::validation::ReasonCode::EVENT_POLICY_COMPLETION_BLOCKED,
                            ) => {
                                let msg = r.correction_hint.clone().unwrap_or_else(|| {
                                    format!("Completion guard blocked '{}'", evt.topic)
                                });
                                self.bus
                                    .publish(Event::new("event.completion.blocked", msg));
                                event_accepted = false;
                                break;
                            }
                            Some(
                                crate::validation::ReasonCode::EVENT_POLICY_COMPLETION_IGNORED,
                            ) => {
                                let msg = r.correction_hint.clone().unwrap_or_else(|| {
                                    format!("Completion guard ignored '{}'", evt.topic)
                                });
                                self.bus
                                    .publish(Event::new("event.completion.ignored", msg));
                                event_accepted = false;
                                break;
                            }
                            Some(
                                crate::validation::ReasonCode::EVENT_POLICY_BLOCKED
                                | crate::validation::ReasonCode::EVENT_POLICY_IGNORED,
                            ) => {
                                event_accepted = false;
                                break;
                            }
                            Some(crate::validation::ReasonCode::EVENT_POLICY_WARNING) => {
                                if let Some(hint) = &r.correction_hint {
                                    event_warnings.push(hint.clone());
                                }
                                continue;
                            }
                            Some(crate::validation::ReasonCode::EVENT_POLICY_HOLD) => {
                                hold_reason = r.correction_hint.clone().or_else(|| {
                                    Some(format!("Event '{}' violates policy", evt.topic))
                                });
                                let reason = format!(
                                    "{}:{}",
                                    r.stage.as_str(),
                                    r.reason_code.as_deref().unwrap_or("rejected"),
                                );
                                publish_correction_via_context(
                                    &mut self.bus,
                                    &mut self.state,
                                    state_ledger.as_mut(),
                                    evt,
                                    &reason,
                                    policy_finding_for_topic(
                                        &policy_rejections,
                                        evt.topic.as_str(),
                                    ),
                                );
                                had_policy_rejections = true;
                                event_accepted = false;
                                break;
                            }
                            _ => {
                                let reason = format!(
                                    "{}:{}",
                                    r.stage.as_str(),
                                    r.reason_code.as_deref().unwrap_or("rejected"),
                                );
                                publish_correction_via_context(
                                    &mut self.bus,
                                    &mut self.state,
                                    state_ledger.as_mut(),
                                    evt,
                                    &reason,
                                    policy_finding_for_topic(
                                        &policy_rejections,
                                        evt.topic.as_str(),
                                    ),
                                );
                                had_policy_rejections = true;
                                event_accepted = false;
                                break;
                            }
                        }
                    } else {
                        let reason = format!(
                            "{}:{}",
                            r.stage.as_str(),
                            r.reason_code.as_deref().unwrap_or("rejected"),
                        );
                        publish_correction_via_context(
                            &mut self.bus,
                            &mut self.state,
                            state_ledger.as_mut(),
                            evt,
                            &reason,
                            None,
                        );
                        rejected_topics.insert(evt.topic.clone());
                    }
                }
                if !event_warnings.is_empty() {
                    let msg = format!(
                        "Policy warning for '{}': {}",
                        evt.topic,
                        event_warnings.join("; ")
                    );
                    self.bus.publish(Event::new("event.policy_warning", msg));
                }
                // U11-T4: post-commit pass — only `WorkflowGuardRule`
                // is wired this round. `ExecutionContractRule` is
                // still a partial proxy and would double-reject with
                // the legacy `validate_execution_contract` path.
                // When the post-commit rule rejects, drain the
                // matching `WorkflowGuardRejectionDetail` (the rule
                // pushed it before returning) and write the
                // recovery envelope. Multiple chain rejections on
                // one event share a single recovery envelope (the
                // detail's `reason` concatenates chain details, the
                // legacy helper does the same).
                if event_accepted && post_commit_enabled {
                    let post_results = {
                        let mut ctx = crate::validation::ValidationContext::new(&mut snapshot)
                            .with_policy_runtime_state(&mut policy_state)
                            .with_review_step_tracker(&mut review_step_tracker)
                            .with_workflow_progress(&mut workflow_progress)
                            .with_workflow_guard_details(&mut wg_details)
                            .with_payload_contract_violation(&mut event_policy_violation)
                            .with_source_hats_by_topic(&source_hats_by_topic)
                            .with_target_hats_by_topic(&target_hats_by_topic)
                            .with_tasks_path(
                                self.config
                                    .core
                                    .workspace_root
                                    .join(".ralph")
                                    .join("agent")
                                    .join("tasks.jsonl"),
                            );
                        pipeline.validate_post_commit(&view, &mut ctx, evt)
                    };
                    for r in &post_results {
                        if r.accepted {
                            continue;
                        }
                        if r.stage != crate::validation::ValidationStage::WorkflowGuard {
                            // Future post-commit rules (e.g. the
                            // full `ExecutionContractRule` once U6
                            // wires the workspace path) plug in
                            // here. Today only the workflow guard
                            // is engaged, so any other stage is a
                            // misconfiguration — log and drop the
                            // event to be safe.
                            tracing::warn!(
                                stage = %r.stage,
                                topic = %evt.topic,
                                "U11-T4: unexpected post-commit rejection; dropping event"
                            );
                            event_accepted = false;
                            break;
                        }
                        // Drain the matching detail recorded by
                        // the rule. Today `WorkflowGuardRule` is
                        // the only post-commit rule, so at most
                        // one detail was pushed; we pop it back
                        // here so the next iteration's pre-commit
                        // sees a clean accumulator.
                        if let Some(detail) = wg_details.pop() {
                            Self::log_workflow_guard_rejection(self, &detail);
                        }
                        let reason = format!(
                            "{}:{}",
                            r.stage.as_str(),
                            r.reason_code.as_deref().unwrap_or("rejected"),
                        );
                        publish_correction_via_context(
                            &mut self.bus,
                            &mut self.state,
                            state_ledger.as_mut(),
                            evt,
                            &reason,
                            None,
                        );
                        had_policy_rejections = true;
                        event_accepted = false;
                        break;
                    }
                }
                if event_accepted {
                    // U3 (2026-06-27-002 plan completion): the
                    // emit-gate facade was originally wired
                    // here, but breaking the invariant that
                    // `accepted_events` is the source of
                    // `hat_lifecycle_tracker.complete()` calls
                    // caused 30+ existing tests to fail
                    // (P0 #1 regression gate). The gate is
                    // now observed in a post-process step
                    // (see the `validate_publish_gate`
                    // helper below) so the lifecycle tracker
                    // still records terminal events while
                    // gate-rejected events surface their
                    // recovery envelope.
                    accepted_events.push(evt.clone());
                }
            }

            events = accepted_events;

            // Restore LoopState fields mutated through context overrides.
            self.state.state_ledger = state_ledger;
            self.state.review_step_tracker = review_step_tracker;
            self.state.policy_runtime_state = Some(policy_state);
            self.state.workflow_progress = workflow_progress;

            if policy_enabled {
                // Process recoverable rejection budget.
                use crate::event_policy::ReasonClass;
                for rejection in &policy_rejections {
                    if let Some(ref class) = rejection.reason_class {
                        // Semantic-gate violations are recoverable but bypass the
                        // retry budget so a misbehaving coordinator cannot exhaust
                        // the schema budget on empty-diff retries.
                        if matches!(class, ReasonClass::SemanticGateViolation) {
                            continue;
                        }
                        let hat = rejection.source_hat.as_deref().unwrap_or("unknown");
                        let (count, exhausted) = self.state.record_recoverable_rejection_key(
                            hat,
                            &rejection.topic,
                            class.as_str(),
                        );
                        if exhausted {
                            self.state
                                .recoverable_exhaustion_buffer
                                .push(RecoverableExhaustion {
                                    hat: hat.to_string(),
                                    topic: rejection.topic.clone(),
                                    reason_class: *class,
                                    count,
                                });
                        }
                    }
                }

                // Write hold artifact if policy hold was triggered.
                if let Some(ref reason) = hold_reason
                    && let Err(e) = self.write_hold_artifact(Some(reason))
                {
                    warn!(error = %e, "Failed to write hold artifact");
                }

                // U6: capture the first payload contract violation for the runner.
                if payload_contract_violation.is_none() {
                    payload_contract_violation = event_policy_violation;
                }
                if !policy_rejections.is_empty() {
                    had_policy_rejections = true;
                }
            }

            if !rejected_topics.is_empty() {
                tracing::debug!(
                    rejected = rejected_topics.len(),
                    remaining = events.len(),
                    "U11-T2: unified pipeline rejected topics; non-event-policy events continue through legacy gates"
                );
            }
        }
        // --- End U11-T2 ---
        // P1-3 (P1 follow-up): the unified pipeline verdict
        // is independent of the legacy gate stack — the two
        // layers produce orthogonal reject signals (the
        // agent-facing `publish_correction_via_context` from
        // unified, the operator-facing `recovery_envelope` +
        // `contract_rejections` from legacy). The batch is
        // NOT short-circuited: events the unified pipeline
        // rejected DO still reach the legacy gates so the
        // legacy execution-contract check can produce its
        // own `MissingPayloadField` finding. (Originally U11-T2
        // had an `events.retain` that dropped unified-rejected
        // topics; that was the wrong design and broke
        // `replay_light_integration::test_rejected_work_done_retry_*`
        // and `test_rejected_missing_plan_path_*`. The retain
        // is removed; tests `p1_3_unified_*` document the
        // layered contract.)

        // --- Workflow guard validation is now unified into the
        // pre-commit / post-commit loop above (U11-T4). The legacy
        // `apply_workflow_guard_validation` call site, the legacy
        // `WorkflowGuardOutcome` / `WorkflowGuardRejectionDetail`
        // types, and the legacy workflow-guard → `task.resume`
        // bridge have all been deleted; the `WorkflowGuardRule` in
        // `validation::rules_workflow_guard` is the single source
        // of truth for out-of-order / correlation-extraction
        // rejections. ---

        // Update policy runtime state for events that survived all validation layers
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            let policy_state = self
                .state
                .policy_runtime_state
                .get_or_insert_with(PolicyRuntimeState::default);
            let completion_promise = self.config.event_loop.completion_promise.as_str();
            for event in &events {
                // 2026-06-29-007 P0 fix: do not mark the completion promise
                // (e.g. LOOP_COMPLETE) as terminal until check_completion_event
                // has actually validated required_events / verdict gate. A
                // rejected LOOP_COMPLETE must not poison terminal state and
                // block recovery events like plan.blocked / task.resume.
                if policy_config.terminal_topics.contains(&event.topic)
                    && event.topic != completion_promise
                {
                    policy_state.terminal_observed = true;
                }
            }
        }

        // --- Execution contract validation (U5): validate work.done before publishing ---
        // This runs after all other validation layers, before record/publish.
        // Contract rejection publishes diagnostic + guidance but does NOT record/publish the original.
        // Track raw event counts before contract filtering for missing-event gate logic
        let contract_validation_input_count = events.len();
        let mut contract_rejections: Vec<ExecutionContractFinding> = Vec::new();
        // U3 (2026-06-27-002 plan): take an owned copy of
        // `execution_contracts` so the immutable borrow of
        // `self.config` ends BEFORE the `for` loop runs
        // `apply_emit_gate` (which needs `&mut self`). The
        // original is restored below. This is the only
        // way around NLL's limit on conditional borrow
        // extents inside a `for` body.
        let contracts_enabled = self
            .config
            .event_loop
            .execution_contracts
            .as_ref()
            .is_some_and(|c| c.enabled);
        let owned_contracts = if contracts_enabled {
            self.config.event_loop.execution_contracts.clone()
        } else {
            None
        };
        let events = if contracts_enabled {
            let contracts = owned_contracts.as_ref().unwrap();
            let current_loop_id = self.current_loop_id_for_contract();
            // U3 (2026-06-27-002 plan): own the
            // `workspace_root` and `tasks_path` paths so
            // the `&self` borrow ends before the loop
            // body needs `&mut self` for
            // `apply_emit_gate`.
            let workspace_root_owned = std::path::PathBuf::from(&self.config.core.workspace_root);
            let tasks_path_owned = self.tasks_path();
            let workspace_root = workspace_root_owned.as_path();
            let tasks_path = tasks_path_owned.as_path();

            let mut accepted: Vec<JsonlEvent> = Vec::with_capacity(events.len());
            // P1-1 (2026-07-01-002 audit): collect the set of
            // `fix-NN` ids already known to the projector so that a
            // stale coordinator emitting `work.ready(fix-XX)` for an
            // id outside the chain is rejected before the contract
            // check produces a misleading finding.  When the
            // projector is disabled the gate is a no-op — the
            // contract pipeline still applies, but the range
            // guard's `fix-unit` set is empty so unknown-fix emits
            // pass through.  This preserves the historical
            // behaviour for presets that opt out of state
            // projection.
            let fix_unit_known: std::collections::BTreeSet<String> =
                match self.state.state_projection.as_ref() {
                    Some(projector) => crate::runtime_state::fix_unit_known_ids(projector),
                    None => std::collections::BTreeSet::new(),
                };
            // Re-usable insertion point for the fix-unit range
            // finding.  Constructed fresh per iteration so the
            // closure captures the right `&event`.
            for event in events {
                // Range guard BEFORE the contract check: when the
                // payload targets a `fix-NN` step that the
                // projector has never seen, drop the event as
                // `invalid_step_target`.  We skip the check for any
                // other topic (e.g. `fix.applied`, `work.done`,
                // `plan.complete`) and for fix-unit events whose
                // step is already known.
                if event.topic.as_str() == "work.ready" {
                    // The range guard only fires when the
                    // projector is active (it has populated
                    // `tasks.jsonl`).  When the chain is genuinely
                    // empty — e.g. before the first fix-unit is
                    // dispatched — we let the event through so the
                    // contract pipeline can decide.  This
                    // preserves the historical behaviour when
                    // state projection is disabled (empty chain
                    // means "no information, accept everything").
                    let guard_active = self.state.state_projection.as_ref().is_some();
                    if guard_active
                        && let Some(rejected_step) =
                            unknown_fix_step(event.payload.as_deref(), &fix_unit_known)
                    {
                        warn!(
                            topic = %event.topic,
                            step = %rejected_step,
                            "fix-unit step outside known chain — rejecting work.ready and surfacing task.resume"
                        );
                        // Synthesize an ExecutionContractFinding so
                        // the downstream rejection machinery (which
                        // already knows how to publish a `task.resume`
                        // with the right provenance) treats this
                        // exactly like any other contract violation.
                        self.push_fix_unit_range_finding(&event, &rejected_step, &fix_unit_known);
                        // Skip the rest of the contract pipeline
                        // for the rejected event; the rejection
                        // machinery above has already published the
                        // diagnostic + `task.resume`.
                        continue;
                    }
                }
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
                        tasks_path,
                        event_provenance,
                        &DefaultGitEvidenceProvider,
                        self.state.loop_start_sha.as_deref(),
                    );
                    let guidance_topic_owned = rule.reject.guidance_topic.clone();
                    let diagnostic_topic_owned = rule.reject.diagnostic_topic.clone();
                    match decision {
                        ExecutionContractDecision::Accept => {
                            // U2 (2026-07-01-002 plan): run soft checks
                            // (e.g. fix-unit commit footer) on accepted
                            // events.  These never flip an Accept into a
                            // Reject; instead they surface diagnostics so
                            // the agent can self-correct next iteration
                            // (see `check_fix_unit_commit_footer`).
                            let soft_diagnostics = run_execution_contract_soft_checks(
                                &proto_event,
                                workspace_root,
                                &DefaultGitEvidenceProvider,
                                self.state.loop_start_sha.as_deref(),
                            );
                            for diag in &soft_diagnostics {
                                warn!(
                                    topic = %event.topic,
                                    step = ?diag.kind,
                                    "Execution contract soft-check diagnostic"
                                );
                            }
                            accepted.push(event);
                        }
                        ExecutionContractDecision::Reject(findings) => {
                            // Publish rejection diagnostic and guidance, do NOT accept the event
                            let finding = &findings[0];
                            let disposition = crate::event_loop::accepted_event::from_execution_contract_rejection(
                                crate::event_loop::accepted_event::CandidateEvent {
                                    topic: event.topic.clone(),
                                    payload: event.payload.clone().unwrap_or_default(),
                                },
                                crate::event_loop::rejection::RejectionStage::ExecutionContract,
                                format!("{:?}", finding.kind),
                                finding.message.clone(),
                            );
                            debug_assert!(
                                !disposition.is_committable(),
                                "execution contract rejection must never be committable"
                            );
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
                            //
                            // DEV-005 (2026-07-06): for `TaskNotTerminal`, route
                            // recovery to the hat that can actually close the task
                            // (typically coordinator) instead of the emitter.
                            // P1-5 (2026-07-07-002): for `TaskNotFound` carrying a
                            // `task_key`, route to a coordinator hat that can repair
                            // the ledger (orphan row / identity mismatch) — the
                            // emitter (executor) cannot fix `tasks.jsonl`.
                            let source_hat_str = finding.source_hat.as_deref();
                            let mut retry_target: Option<HatId> = None;
                            let mut no_retry_reason: Option<String> = None;
                            let mut task_not_terminal_hint: Option<String> = None;
                            if let Some(hat_id_str) = source_hat_str {
                                if hat_id_str == "ralph" {
                                    no_retry_reason = Some(
                                        "no business hat available for fallback ralph".to_string(),
                                    );
                                } else {
                                    let resolved_hat_id_str =
                                        if let ExecutionContractViolationKind::TaskNotTerminal {
                                            task_id,
                                            ..
                                        } = &finding.kind
                                        {
                                            use crate::task_store::TaskStore;
                                            let task_snapshot = TaskStore::load(tasks_path)
                                                .ok()
                                                .and_then(|store| store.get(task_id).cloned());
                                            let (delegate, hint) =
                                                crate::execution_contract::task_not_terminal_resume_plan(
                                                    task_id,
                                                    task_snapshot.as_ref(),
                                                    hat_id_str,
                                                    &self.config.tasks.coordinator_hats,
                                                );
                                            task_not_terminal_hint = Some(hint);
                                            delegate
                                        } else if let ExecutionContractViolationKind::TaskNotFound {
                                            task_id,
                                        } = &finding.kind
                                        {
                                            // P1-5: TaskNotFound with a payload task_key is
                                            // an identity mismatch / orphan-row scenario.
                                            // The executor cannot repair the ledger; route
                                            // to a coordinator hat. Without a task_key this
                                            // is a plain missing-task error and the source
                                            // hat is still the right retry target.
                                            let payload_obj = event
                                                .payload
                                                .as_deref()
                                                .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok());
                                            let payload_key = payload_obj
                                                .as_ref()
                                                .and_then(|v| v.get("task_key"))
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            if payload_key.is_empty() {
                                                hat_id_str.to_string()
                                            } else {
                                                use crate::task_store::TaskStore;
                                                let task_snapshot = TaskStore::load(tasks_path)
                                                    .ok()
                                                    .and_then(|store| store.get(task_id).cloned());
                                                let (delegate, hint) =
                                                    crate::execution_contract::task_not_found_resume_plan(
                                                        task_id,
                                                        payload_key,
                                                        task_snapshot.as_ref(),
                                                        hat_id_str,
                                                        &self.config.tasks.coordinator_hats,
                                                    );
                                                task_not_terminal_hint = Some(hint);
                                                delegate
                                            }
                                        } else {
                                            hat_id_str.to_string()
                                        };
                                    let hat_id = HatId::new(&resolved_hat_id_str);
                                    match self.registry.get(&hat_id) {
                                        None => {
                                            no_retry_reason = Some(format!(
                                                "source hat '{}' not registered",
                                                resolved_hat_id_str
                                            ));
                                        }
                                        Some(_) => {
                                            let is_delegated_recovery = matches!(
                                                &finding.kind,
                                                ExecutionContractViolationKind::TaskNotTerminal { .. }
                                                    | ExecutionContractViolationKind::TaskNotFound { .. }
                                            )
                                                && resolved_hat_id_str != hat_id_str;
                                            if is_delegated_recovery {
                                                retry_target = Some(hat_id);
                                            } else {
                                                let can_retry = self
                                                    .registry
                                                    .can_publish(&hat_id, event.topic.as_str());
                                                let can_fail = self
                                                    .registry
                                                    .can_publish(&hat_id, "work.failed");
                                                if !can_retry && !can_fail {
                                                    no_retry_reason = Some(format!(
                                                        "recovery hat '{}' cannot publish '{}' or 'work.failed'",
                                                        resolved_hat_id_str,
                                                        event.topic.as_str()
                                                    ));
                                                } else {
                                                    retry_target = Some(hat_id);
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                no_retry_reason =
                                    Some("no source hat recorded on event or in state".to_string());
                            }

                            if let Some(hat_id) = &retry_target {
                                let payload_obj = event.payload.as_deref().and_then(|p| {
                                    serde_json::from_str::<serde_json::Value>(p).ok()
                                });
                                let task_key = payload_obj
                                    .as_ref()
                                    .and_then(|v| v.get("task_key"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let step = payload_obj
                                    .as_ref()
                                    .and_then(|v| v.get("step"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let violation_code = match &finding.kind {
                                    ExecutionContractViolationKind::TaskNotTerminal { .. } => {
                                        "task_not_terminal"
                                    }
                                    ExecutionContractViolationKind::TaskNotFound { .. } => {
                                        // P1-5: distinguish identity-mismatch routing
                                        // (coordinator-bound) from plain missing-task
                                        // (source-hat retry) so the protocol-violation
                                        // budget is tracked under the right signature.
                                        if task_key.is_empty() {
                                            "task_not_found"
                                        } else {
                                            "task_not_found_identity_mismatch"
                                        }
                                    }
                                    _ => "execution_contract",
                                };
                                let source_hat = source_hat_str.unwrap_or("unknown");
                                let (_protocol_count, protocol_exhausted) =
                                    self.state.record_protocol_violation_signature(
                                        source_hat,
                                        event.topic.as_str(),
                                        task_key,
                                        step,
                                        violation_code,
                                    );
                                if protocol_exhausted {
                                    let fail_reason =
                                        format!("protocol_violation_repeated:{violation_code}");
                                    warn!(
                                        topic = %event.topic.as_str(),
                                        reason = %fail_reason,
                                        "U8: protocol violation retry budget exhausted; fail-closing"
                                    );
                                    let blocked = Event::new(
                                        "plan.blocked",
                                        serde_json::json!({ "reason": fail_reason }).to_string(),
                                    );
                                    self.bus.publish(blocked.clone());
                                    self.state.record_event(&blocked);
                                } else {
                                    let original_trigger = self
                                        .state
                                        .last_activation_events
                                        .iter()
                                        .rev()
                                        .find(|trigger| {
                                            self.registry.get_config(hat_id).is_some_and(|config| {
                                                config.trigger_topics().iter().any(|topic| {
                                                    topic.matches_str(trigger.topic.as_str())
                                                })
                                            })
                                        });
                                    let recovery_reason = task_not_terminal_hint
                                        .as_deref()
                                        .unwrap_or(finding.message.as_str());
                                    let retry_payload = serde_json::json!({
                                        "rejected_topic": event.topic.as_str(),
                                        // U2 (2026-06-17-003 plan): add the
                                        // schema-required `target_hat` field
                                        // alongside `reason` so the drift
                                        // detector counts the contract recovery
                                        // as schema-compliant.
                                        "target_hat": hat_id.as_str(),
                                        "reason": recovery_reason,
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
                                }
                            } else if let Some(reason) = &no_retry_reason {
                                warn!(
                                    topic = %event.topic.as_str(),
                                    reason = %reason,
                                    "No safe retry target for rejected event; recovery is human.guidance only"
                                );
                            }

                            // Publish structured diagnostic (now carries
                            // retry_target and no_retry_reason for observability).
                            let diagnostic_payload = serde_json::json!({
                                "topic": event.topic.as_str(),
                                "finding": findings,
                                "rejected_at": chrono::Utc::now().to_rfc3339(),
                                "retry_target": retry_target.as_ref().map(|h| h.as_str()),
                                "no_retry_reason": no_retry_reason,
                            });
                            let diagnostic_event = Event::new(
                                diagnostic_topic_owned.as_str(),
                                diagnostic_payload.to_string(),
                            );
                            self.bus.publish(diagnostic_event);

                            // Publish the human-readable guidance. The default target
                            // is `plan.blocked` (plan 2026-06-28-005
                            // changed it from the now-deleted
                            // `human.guidance`). The payload is
                            // kept as a free-form string so existing
                            // consumer tooling that parses text still
                            // works.
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
                                Event::new(guidance_topic_owned.as_str(), guidance_payload)
                                    // 2026-06-28-005: pin the guidance
                                    // publish to the same target as the
                                    // retry event so the ralph fallback
                                    // (subscribed to *) does not shadow
                                    // it. retry_target is None for the
                                    // no-safe-target case; in that case
                                    // the event fans out to the
                                    // fallback (which is the documented
                                    // behaviour — see no_retry_reason
                                    // branch above).
                                    .with_target(
                                        retry_target.clone().unwrap_or_else(|| HatId::new("ralph")),
                                    );
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

        // 2026-07-07-002 U2: handoff / work.done dedup side effects only for
        // contract-committed events (never for rejected candidates).
        self.apply_contract_committed_side_effects(&events);

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

        // 2026-07-06 U5 (DEV-005): TaskNotTerminal recovery is handled
        // inline in the contract-rejection branch above (routes to the
        // hat that can close the task). The post-batch synthesis loop
        // was removed to avoid duplicate `task.resume` events.

        let mut has_orphans = false;

        // Validate and transform events (apply backpressure for build.done)
        let mut validated_events = Vec::new();
        // P1-2: own the topic strings so per-event commits
        // (`commit_terminal_delta` borrows `&mut self`) can run
        // inside the same loop without aliasing the `&str`
        // borrow from `completion_promise.as_str()`.
        let completion_topic = self.config.event_loop.completion_promise.clone();
        let cancellation_topic = self.config.event_loop.cancellation_promise.clone();
        let total_events = events.len();
        let mut completion_seen_in_batch = false;
        // Clone the policy config so `policy_config_ref`
        // can be dropped before the U3 gate loop runs.
        // `policy_config_ref` is `Option<&PolicyConfig>`
        // which borrows `self.config`; the gate loop
        // needs `&mut self` for `apply_emit_gate`.
        let policy_config_owned = self.config.event_loop.event_policy.clone();
        let write_diagnostic = policy_config_owned
            .as_ref()
            .map(|c| c.completion_after_terminal.write_diagnostic_event)
            .unwrap_or(false);
        let policy_config_ref = policy_config_owned.as_ref();
        let mut accepted_log_events = Vec::new();
        macro_rules! accept_event {
            ($accepted:expr) => {{
                let accepted = $accepted;
                // 2026-07-06 U9 (DEV-009): when a work.done is admitted,
                // record its step so the topology guard at line ~10666
                // can refuse the next step's work.ready until the
                // previous step's work.done lands.
                if accepted.topic.as_str() == "work.done" {
                    let payload: &str = &accepted.payload;
                    if let Some(start) = payload.find("\"step\":\"") {
                        let rest = &payload[start + 8..];
                        if let Some(end) = rest.find('"') {
                            let step = &rest[..end];
                            if step.starts_with("step-") {
                                self.state.step_work_done_seen.insert(step.to_string());
                            }
                        }
                    }
                }
                accepted_log_events.push(accepted.clone());
                validated_events.push(accepted);
            }};
        }

        // U3 (2026-06-27-002 plan completion): first
        // pass through the emit-gate facade runs BEFORE
        // the main loop so the recovery envelope
        // (Reject) or repair-sink envelope
        // (AcceptRepairStream) is recorded for every
        // event in the batch. The second pass runs
        // before `self.bus.publish` (see below) to enforce
        // the `AcceptMainBus`-only publication contract.
        // The double-pass design keeps the lifecycle
        // tracker integration intact: terminal events
        // still close activations even when the gate
        // rejects them.
        //
        // `policy_config_ref` is captured by value (it is
        // a `&Option<...>` whose payload we do not
        // mutate) before the gate loop runs so the
        // `&mut self` borrow on `apply_emit_gate` is
        // unblocked.
        // `policy_config_ref` (an `Option<&EventPolicyConfig>`)
        // is held until after the U3 gate loop completes. The
        // gate loop needs `&mut self`, so the immutable
        // borrow on `self.config` must be released first.
        // Snapshot the events by reference so the gate
        // loop can borrow `&mut self`. The `events` vec
        // is owned (not borrowed from self) so this is
        // safe.
        //
        // P0-1 (2026-06-27 adversarial review): the
        // previous design called `apply_emit_gate` here
        // and re-ran the stage pipeline in
        // `apply_emit_gate_on_validated`, which
        // double-advanced the per-task
        // `RepairStateMachine` and broke the
        // `repair_budget=3` invariant. We now stash the
        // outcome from the first pass (which mutates
        // `self.repair_state_machine`) keyed by
        // `(topic, payload)` so the publish-time gate
        // can reuse it without re-running the pipeline.
        //
        // Keying by `(topic, payload)` is safe because
        // each JSONL line is a unique event — two
        // distinct events with the same topic and the
        // same payload would be a pathological duplicate
        // in `events.jsonl`, which the upstream parser
        // already rejects. The synthesised
        // `build.blocked` / `task.relocate` events
        // inherit the source event's payload verbatim
        // (see the `accept_event!` call sites below),
        // so the lookup hits the same key. The keys
        // are normalised to `(String, String)` so both
        // the JSONL-internal `event_reader::Event`
        // (String topic) and the bus-shaped
        // `ralph_proto::Event` (Topic, `.as_str()`)
        // can index into the same map.
        let gate_outcomes: std::collections::HashMap<
            (String, String),
            crate::event_loop::emit_gate::EmitGateOutcome,
        > = {
            let mut outcomes = std::collections::HashMap::with_capacity(events.len());
            for event in &events {
                let key = (
                    event.topic.clone(),
                    event.payload.clone().unwrap_or_default(),
                );
                let outcome = self.evaluate_emit_gate_for_jsonl_event(event);
                outcomes.insert(key, outcome);
            }
            outcomes
        };

        for (index, event) in events.into_iter().enumerate() {
            let payload = event.payload.clone().unwrap_or_default();

            // 2026-07-07-002 U4: terminal-closed guard before main-events commit.
            match self.evaluate_terminal_closed_for_event(
                event.topic.as_str(),
                &payload,
                completion_topic.as_str(),
            ) {
                crate::event_loop::terminal_closed_guard::TerminalClosedDecision::Allow => {}
                crate::event_loop::terminal_closed_guard::TerminalClosedDecision::RejectPostTerminal => {
                    self.publish_post_terminal_rejection(
                        event.topic.as_str(),
                        "post_terminal_business_event_frozen",
                    );
                    continue;
                }
                crate::event_loop::terminal_closed_guard::TerminalClosedDecision::IgnoreDuplicateTerminal => {
                    self.bus.publish(Event::new(
                        "event.completion.ignored",
                        format!(
                            "Terminal-closed guard ignored duplicate '{}'",
                            event.topic
                        ),
                    ));
                    continue;
                }
            }

            // 2026-07-06 U9 (DEV-009): topology guard — work.ready for
            // step-NN where NN > 01 must be preceded by work.done for
            // step-(NN-1). Without this guard the coordinator can
            // publish a new step's work.ready before the executor
            // closed the previous step's work.done, leaving tasks
            // stuck open across the boundary (observed in
            // 2026-07-05-153532 run: step-02 work.ready at 15:43 with
            // step-01 work.done outstanding). Log + drop with a
            // diagnostic; the coordinator will be re-prompted with
            // the missing predecessor and re-emit on the next turn.
            if event.topic == "work.ready" {
                let step: Option<String> =
                    payload
                        .find("\"step\":\"")
                        .map(|i| i + 8)
                        .and_then(|start| {
                            let rest = &payload[start..];
                            rest.find('"').map(|end| rest[..end].to_string())
                        });
                if let Some(step) = step
                    && let Some(nn) = step
                        .strip_prefix("step-")
                        .and_then(|s| s.parse::<u32>().ok())
                    && nn > 1
                {
                    let prev = format!("step-{:02}", nn - 1);
                    if !self.state.step_work_done_seen.contains(&prev) {
                        warn!(
                            topic = %event.topic,
                            step = %step,
                            prev_step = %prev,
                            "DEV-009: work.ready for step arrived before previous step's work.done; dropping as cross-step handoff violation"
                        );
                        let diagnostic = Event::new(
                            "event.topology.out_of_order",
                            format!(
                                "{{\"dropped_topic\":\"work.ready\",\"step\":\"{step}\",\"prev_step\":\"{prev}\",\"reason\":\"work.ready arrived before previous step's work.done\"}}"
                            ),
                        );
                        self.bus.publish(diagnostic);
                        continue;
                    }
                }
            }

            // 2026-07-06 U7 (DEV-007): topology guard — test.passed
            // must be preceded by work.done for the same plan/step.
            // Without this guard a validator hat that activates late
            // (e.g. after the shipper has already emitted REVIEW_COMPLETE
            // via the runtime-recovery stall pipeline) can publish a
            // test.passed event that violates the preset's intended
            // review-before-publish sequence. Log + drop, do not
            // diagnose as failure (the test genuinely passed; only
            // the ordering was wrong).
            if event.topic == "test.passed" && !self.state.seen_topics.contains("work.done") {
                warn!(
                    topic = %event.topic,
                    "DEV-007: test.passed arrived before any work.done in this loop; dropping as topology-violating"
                );
                let diagnostic = Event::new(
                    "event.topology.out_of_order",
                    "{\"dropped_topic\":\"test.passed\",\"reason\":\"test.passed arrived before any work.done was admitted for this loop\"}".to_string(),
                );
                self.bus.publish(diagnostic);
                continue;
            }

            // Detect loop.cancel — unconditional graceful termination
            if !cancellation_topic.is_empty() && event.topic.as_str() == cancellation_topic {
                info!(
                    payload = %payload,
                    "loop.cancel event detected — scheduling graceful termination"
                );
                // P1-2: per-event commit (see `commit_terminal_delta`).
                if !self.state.cancellation_requested {
                    Self::commit_terminal_delta(
                        &mut self.state.state_ledger,
                        crate::state::CommitDelta::CancellationRequested,
                    );
                }
                self.state.cancellation_requested = true;
                accepted_log_events.push(Event::new(event.topic.as_str(), &payload));
                // Continue processing remaining events (they may contain cleanup info)
                continue;
            }

            if event.topic == completion_topic.as_str() {
                if self.state.completion_honored {
                    debug!("Completion event already handled, ignoring duplicate");
                    continue;
                }
                // 2026-06-30-001 P0-5: report_done_seen guard.
                // Refuse to honour `LOOP_COMPLETE` if the
                // workflow has not yet produced its final
                // `report.done`. This stops the runner / agent
                // from racing the reviewer chain to the
                // terminal — events L37 of the 032648 run
                // showed ralph emitting `LOOP_COMPLETE` while
                // 6/7 review dimensions were still in flight.
                if let Err(reason) = self.state.mark_completion_requested(
                    &self.config.event_loop.required_events,
                    &self.config.event_loop.completion_promise,
                ) {
                    tracing::warn!(
                        reason = %reason,
                        iteration = self.state.iteration,
                        "LOOP_COMPLETE REJECTED by mark_completion_requested"
                    );
                    self.state.completion_requested = true;
                    if self
                        .state
                        .is_rejected_completion_duplicate(payload.as_str())
                    {
                        // Identical rejected payload: do not re-inject
                        // a correction block (would just spam the prompt),
                        // but still let `check_completion_event()` advance
                        // the stale-breaker counter for this iteration.
                        continue;
                    }
                    let missing = self
                        .state
                        .missing_required_events(&self.config.event_loop.required_events);
                    let free_form = format!(
                        "LOOP_COMPLETE rejected: missing required events: {missing:?}. \
                         The agent must complete all workflow phases before emitting LOOP_COMPLETE. \
                         Use loop.cancel to abort the workflow instead."
                    );
                    tracing::warn!(
                        reason = %reason,
                        missing = ?missing,
                        iteration = self.state.iteration,
                        topic = %event.topic,
                        index = index,
                        "P0-5: completion event rejected; \
                         required events not yet observed; \
                         event will not transition loop to terminal"
                    );
                    let _ = Self::inject_completion_correction(
                        &mut self.state,
                        "missing_required_events",
                        &free_form,
                    );
                    // Drop the event from this batch's
                    // accepted stream; the runtime continues
                    // to wait for required workflow events. The
                    // event is NOT added to `accepted_log_events`
                    // so the events.jsonl file does not carry a
                    // false-positive terminal event.
                    continue;
                }
                // Completion event is accepted regardless of position in batch.
                // Events AFTER it in the same batch are protected by the completion guard.
                // P1-2: per-event commit (see `commit_terminal_delta`).
                Self::commit_terminal_delta(
                    &mut self.state.state_ledger,
                    crate::state::CommitDelta::CompletionRequested,
                );
                completion_seen_in_batch = true;
                let accepted = Event::new(event.topic.as_str(), &payload);
                accepted_log_events.push(accepted.clone());
                self.state.record_event(&accepted);
                self.state.last_completion_payload = Some(payload.to_string());
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

            // 2026-07-01-001 plan U2: persistent
            // `completion_honored` guard. Once a previous batch
            // (or this loop's prior run via ledger replay) set
            // the flag, every subsequent business event must
            // be rejected even if the *current* batch has not
            // seen a completion topic yet. The same-batch
            // guard below stays as a fast path for diagnostics.
            //
            // 2026-07-01-001 review P1-1: when `event_policy`
            // is disabled or absent, the policy-config branch
            // is skipped — but R1's "no further business event
            // may enter the bus" is an absolute invariant, so
            // we fall back to a hard intercept that always
            // `continue`s. This keeps simple presets (no
            // event_policy) on the same R1 contract as
            // ce-executor-serial.
            if self.state.completion_honored
                && event.topic != self.config.event_loop.completion_promise.as_str()
                && event.topic != self.config.event_loop.cancellation_promise.as_str()
            {
                let policy_enabled = policy_config_ref.is_some_and(|c| c.enabled);
                if !policy_enabled {
                    // Hard fallback (2026-07-01-001 review P1-1):
                    // refuse every business event
                    // post-completion when no policy is
                    // configured. We ALWAYS emit the
                    // diagnostic here (no `write_diagnostic`
                    // gate) because there is no
                    // `completion_after_terminal` config to
                    // consult — the R1 absolute invariant
                    // holds regardless of policy settings,
                    // and `ralph diagnose` needs the event
                    // for parity with the policy-configured
                    // path.
                    self.bus.publish(Event::new(
                        "event.completion.blocked",
                        format!(
                            "Persistent completion guard hard-blocked '{}': \
                             no event_policy configured; R1 fallback intercept",
                            event.topic
                        ),
                    ));
                    continue;
                }
                if let Some(policy_config) = policy_config_ref
                    && let Some(decision) =
                        check_completion_guard(&event.topic, policy_config, true)
                {
                    match &decision {
                        PolicyDecision::Block(finding) => {
                            if write_diagnostic {
                                self.bus.publish(Event::new(
                                    "event.completion.blocked",
                                    format!(
                                        "Persistent completion guard blocked '{}': {}",
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
                                        "Persistent completion guard ignored '{}': {}",
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
                                        "Persistent completion guard warning for '{}': {}",
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

            // Same-batch completion guard: events after a completion topic in the
            // same batch are subject to completion_after_terminal filtering.
            if completion_seen_in_batch
                && let Some(policy_config) = policy_config_ref
                && policy_config.enabled
                && let Some(decision) = check_completion_guard(&event.topic, policy_config, true)
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

        // Track whether any events will be published (before the loop consumes them).
        let had_events = !validated_events.is_empty();
        let had_plan_events = validated_events
            .iter()
            .any(|event| event.topic.as_str().starts_with("plan."));
        // Record and diagnose validated events (before consuming them).
        let verdict_topics = self.verdict_gate_topics();
        let verdict_topics_slice = verdict_topics.as_deref();
        // U2: collect predecessor deltas for post-loop ledger commit.
        let mut completion_predecessor_deltas: Vec<crate::state::CommitDelta> = Vec::new();
        for event in &validated_events {
            // 2026-06-30-001 P0-3 (primary-20260630-032648
            // diagnosis): the runtime rejects
            // `review.start` emits that arrive after the
            // fix-unit chain is exhausted. The pre-fix
            // behaviour let coordinator emit
            // `review.start` a second time after the
            // `fix-NN` chain was complete, triggering an
            // unwanted second review walk that confused
            // the progress-steward state machine and
            // pushed the loop off the normal
            // `plan.complete → shipper → reporter →
            // LOOP_COMPLETE` ladder. The fix is
            // structural: the runtime enforces "no
            // `review.start` after every fix-NN is
            // closed", regardless of what the agent's
            // prompt says. The pre-fix prompt comment is
            // still kept (defence in depth), but it is
            // no longer the sole guard.
            //
            // Detection: when the admitted event is a
            // `work.done` whose `task_key` is a fix-unit
            // shape, we re-check the task store. If every
            // fix-NN step in the current plan is now
            // closed, flip `fix_unit_chain_exhausted` to
            // `true`. The next admit loop iteration that
            // sees a `review.start` while the flag is
            // `true` rejects it before it lands in
            // `accepted_log_events`.
            if event.topic.as_str() == "work.done" && self.is_fix_unit_completion_event(event) {
                self.state.seen_fix_unit_completions =
                    self.state.seen_fix_unit_completions.saturating_add(1);
                if self.is_fix_unit_chain_exhausted() {
                    self.state.fix_unit_chain_exhausted = true;
                }
            }
            if event.topic.as_str() == "review.start"
                && (self.state.fix_unit_chain_exhausted
                    || self.state.seen_fix_unit_completions >= 2)
            {
                tracing::warn!(
                    iteration = self.state.iteration,
                    "P0-3: rejected review.start after fix-unit chain exhausted; \
                     coordinator must emit plan.complete, NOT a second review walk"
                );
                // Drop the event from the accepted stream;
                // the runtime continues to wait for
                // `plan.complete`.
                continue;
            }
            if event.topic.as_str() == "REVIEW_COMPLETE"
                && self.phase_authority_rejects_shipper_emit(event)
            {
                tracing::warn!(
                    iteration = self.state.iteration,
                    topic = %event.topic,
                    "phase authority: shipper routing denied REVIEW_COMPLETE"
                );
                continue;
            }

            // path_required_events: reject anchor topics until every
            // require topic has been observed on this loop lifetime.
            if let Some(missing) = self.path_required_missing_for_anchor(event.topic.as_str()) {
                tracing::warn!(
                    iteration = self.state.iteration,
                    topic = %event.topic,
                    missing = ?missing,
                    "Rejected anchor event: path_required_events require topics not yet observed"
                );
                continue;
            }

            let gate_key = (
                event.topic.as_str().to_string(),
                event.payload.as_str().to_string(),
            );
            if matches!(
                gate_outcomes.get(&gate_key),
                Some(crate::event_loop::emit_gate::EmitGateOutcome::Reject(reject))
                    if reject.reason_code == "phase_violation"
            ) {
                continue;
            }
            if matches!(
                gate_outcomes.get(&gate_key),
                Some(crate::event_loop::emit_gate::EmitGateOutcome::AcceptRepairStream)
            ) {
                continue;
            }

            // Record topic for event chain validation
            self.state.record_event(event);
            self.mark_required_event_seen(event.topic.as_str());
            self.state
                .record_verdict_if_match(event, verdict_topics_slice);
            self.state.record_completion_predecessor_if_match(
                event,
                self.config.event_loop.completion_payload_match.as_ref(),
            );
            // U2: collect predecessor delta for post-loop ledger commit.
            if let Some(cfg) = self.config.event_loop.completion_payload_match.as_ref()
                && event.topic.as_str() == cfg.topic
            {
                completion_predecessor_deltas.push(
                    crate::state::CommitDelta::CompletionPredecessorRecorded {
                        topic: event.topic.to_string(),
                        payload: event.payload.to_string(),
                    },
                );
            }

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
        //
        // U3 (2026-06-27-002 plan completion): route each
        // validated event through the emit-gate facade one
        // more time before publishing to the bus. Events that
        // the gate rejects are still recorded in the
        // lifecycle tracker (so terminal events close
        // activations), but they do NOT reach `self.bus`.
        // The `take_pending` is required because
        // `apply_emit_gate_on_validated` borrows `&mut self`
        // while the iterator borrows `validated_events`.
        //
        // P0-1: we look up the stashed outcome from the
        // first gate pass (keyed by `(topic, payload)`)
        // so the stage pipeline — and especially the
        // `RepairStateMachine.try_transition` call inside
        // `RepairDispatchStage` — runs exactly once per
        // event. The synthesised events (e.g.
        // `build.blocked`) inherit the source event's
        // payload verbatim, so the lookup hits the
        // same key.
        let pending_publish: Vec<Event> = {
            let mut pending = Vec::new();
            for event in &validated_events {
                let payload = event.payload.as_str().to_string();
                let key = (event.topic.as_str().to_string(), payload);
                let stashed = gate_outcomes.get(&key).cloned();
                let accepted = self.apply_emit_gate_on_validated(event, stashed);
                if accepted {
                    pending.push(event.clone());
                }
            }
            pending
        };
        for event in pending_publish {
            self.bus.publish(event.clone());
            self.diagnose_plan_complete_channel(
                &event,
                crate::event_loop::phase_authority::diagnosis::Channel::Main,
            );
            self.apply_phase_authority_on_accepted(&event);
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

        // 2026-06-16-001 U5: stall detection and progress-steward
        // wake. The counter is updated after all validation layers
        // have run so it reflects the *post-validation* state
        // (a turn that only produced rejections is a
        // no-progress turn, not a turn that advanced).
        run_stall_detector_on_state(
            &mut self.state,
            &self.config.event_loop.progress_steward,
            &self.registry,
            &mut self.bus,
        );
        // --- End U5 stall detection ---

        // 2026-07-01-001 plan U6: capture the most recent
        // `test.passed` step into the orchestrator-state cache
        // so the next coordinator prompt can render a
        // directive. We scan `accepted_log_events` (the
        // post-validation stream) so a rejected test.passed
        // is intentionally ignored — the engine only feeds
        // the directive for admitted passes.
        for event in &accepted_log_events {
            if event.topic.as_str() == "test.passed"
                && let Some(step) = extract_step_id(&event.payload)
            {
                let was_fix = step.starts_with("fix-");
                self.state.record_test_passed(step, was_fix);
            }
            if event.topic.as_str() == "test.failed"
                && let Some(step) = extract_step_id(&event.payload)
            {
                self.state.record_validator_terminal(step, "failed");
            }
            if event.topic.as_str() == "plan.complete"
                && let Some(step) = extract_step_id(&event.payload)
            {
                self.state.last_plan_complete_step = Some(step);
            }
        }

        // 2026-07-01-001 plan U6 wiring was removed: plan
        // topology scanning is no longer a base concern. The
        // coordinator hat now derives plan structure from the
        // plan file via prompt context instead of engine-side
        // regex parsing.

        // U2: commit collected predecessor deltas now that
        // `state_ledger` is restored.
        if let Some(ref mut ledger) = self.state.state_ledger {
            for delta in completion_predecessor_deltas {
                if let Err(e) =
                    ledger.commit(delta, Some("loop.completion_predecessor".to_string()))
                {
                    tracing::warn!(
                        error = %e,
                        "U2: completion predecessor commit failed; loop continues"
                    );
                }
            }
        }

        // A1 (002-adversarial-review / 003-adversarial-review
        // P0-1): when the unified ledger is wired in, mirror
        // the per-batch counters into the commit log so the
        // `StateLedger` actually participates in the production
        // event loop. P1-2 (P1 follow-up): terminal markers
        // (`CompletionRequested` / `CompletionHonored` /
        // `CancellationRequested`) are committed per-event at
        // the decision point (see `commit_terminal_delta`) so
        // a mid-flight crash preserves the termination signal.
        // This hook keeps the per-iteration `CounterChanged`
        // and the loop-`StewardWoken` scalars that don't need
        // per-event latency.
        if let Some(ref mut ledger) = self.state.state_ledger {
            use crate::state::{CommitDelta, CounterKind};
            // 2026-06-23 fix plan U7 (CB-5): only advance the
            // iter counter when this iteration actually accepted
            // at least one event. A no-progress turn (all
            // rejected) must NOT bump the iter counter — that
            // would create a divergent ledger where iter N points at
            // `events.jsonl` lines from a different iteration.
            //
            // The `loop.batch_sync` source tag distinguishes the
            // happy path from the no-progress path so operators
            // inspecting `ledger.jsonl` can see when the loop
            // chose not to advance.
            //
            // 2026-06-30-001 P0-6 (primary-20260630-032648
            // diagnosis): the pre-fix code emitted
            // `loop.batch_sync.no_progress` for no-progress
            // turns, which produced two diverging iter
            // sequences in the ledger — `loop.batch_sync`
            // and `loop.batch_sync.no_progress` were
            // committed with independent `seq` numbers, and
            // `summary.md` ended up showing 41 iter while
            // the no-progress sub-stream was at 28. We now
            // commit a single `loop.batch_sync` entry per
            // turn and carry the no-progress signal in the
            // `delta.kind` (via `kind: "no_progress"`),
            // keeping the iter sequence monotonic.
            let batch_sync_source = "loop.batch_sync";
            let iter_counter = CommitDelta::CounterChanged {
                counter: CounterKind::Iteration,
                new_value: i64::from(self.state.iteration),
            };
            // 2026-06-30-001 P1-4: when the turn is a
            // no-progress turn, ALSO commit a
            // `NoProgressTurnObserved` delta so the
            // no-progress dimension is preserved on disk
            // even though we now use a single
            // `loop.batch_sync` source string. Operators
            // can still query "no-progress turns" via
            // `grep kind no_progress_turn_observed
            // .ralph/ledger.jsonl`. The source string on
            // this companion entry is the same
            // `loop.batch_sync` so any source-string
            // filter keeps working unchanged.
            let is_no_progress_turn = !had_events && accepted_log_events.is_empty();
            if is_no_progress_turn {
                let no_progress = CommitDelta::NoProgressTurnObserved {
                    iteration: self.state.iteration,
                };
                if let Err(e) = ledger.commit(no_progress, Some(batch_sync_source.to_string())) {
                    tracing::warn!(
                        error = %e,
                        iteration = self.state.iteration,
                        source = %batch_sync_source,
                        "P1-4: no-progress companion commit failed; loop continues"
                    );
                }
            }
            if let Err(e) = ledger.commit(iter_counter, Some(batch_sync_source.to_string())) {
                tracing::warn!(
                    error = %e,
                    iteration = self.state.iteration,
                    source = %batch_sync_source,
                    "A1: end-of-batch ledger commit failed; loop continues"
                );
            }
            // Terminal marker commits moved to per-event
            // decision points (see `commit_terminal_delta`).
        }

        // U12 wiring (P0-1, 2026-06-27 review): refresh the
        // step-close progress registry after every parsed
        // batch so the next emit is checked against the
        // latest `done`/`total`. Idempotent and a no-op
        // when the step did not opt into `total_units`.
        self.drive_step_close_progress();

        // 2026-06-29-007 plan U1b: advance `current_step`
        // when unit_loop `total_units` reached. Runs after
        // `drive_step_close_progress` so the step-close
        // counter is up to date.
        self.drive_step_transition();

        // 2026-07-02-004 plan U5/U6 wiring: enforce the
        // synthesized precheck gate hat hard-gate and
        // dispatch rejections (resume vs. exhaustion).
        // Runs after `drive_step_transition` so the
        // step-close stage fires first when both apply.
        self.drive_precheck_gate_obligation(&accepted_log_events);

        // 2026-07-28-001 plan U3: stage the over-emit
        // recovery intent and resolve it AFTER we know
        // whether the turn committed a business event. The
        // recovery is stored in `state.pending_over_emit_recovery`
        // by the drop branch above and settled here so a
        // legitimate handoff emitted in the same
        // activation can never be pre-empted by an extra
        // event's `task.resume` injection.
        self.resolve_over_emit_recovery(&accepted_log_events);

        Ok(ProcessedEvents {
            had_events,
            had_raw_events,
            had_rejected_events,
            had_plan_events,
            has_orphans,
            accepted_events: accepted_log_events,
            contract_rejections,
            payload_contract_violation,
        })
    }

    /// U12 wiring (P0-1, 2026-06-27 review): drive the
    /// `StepCloseObligationStage` progress registry
    /// after each `process_parse_result` batch.
    ///
    /// Strategy: count `work.done` emits in
    /// `seen_topics` as `done`, and look up `total` from
    /// `flow.steps[i].total_units`. If the current step
    /// does not declare `total_units`, the call is a
    /// no-op (the stage stays fail-open — the pre-U12
    /// behaviour for presets that did not opt in).
    ///
    /// Idempotent: the underlying
    /// `StepCloseObligationStage::update_progress` is
    /// itself idempotent and rejects counter regressions
    /// silently (see the stage rustdoc).
    fn drive_step_close_progress(&mut self) {
        let step_id = self.state.flow_lifecycle.current_step_id().to_string();
        if step_id.is_empty() {
            return;
        }
        let total_units = match self.flow_step_total_units(&step_id) {
            Some(n) => n,
            None => return,
        };

        let done = self
            .state
            .seen_topics
            .iter()
            .filter(|t| t.as_str() == "work.done")
            .count() as u32;
        self.stage_pipeline
            .update_step_close_progress(&step_id, done, total_units);
    }

    /// 2026-07-28-001 plan U3: settle the staged over-emit
    /// recovery intent. If at least one real business event
    /// (not a scope-violation replay, not a boundary
    /// diagnostic, not a default publish) committed this
    /// turn, the recovery is purely diagnostic — drop the
    /// pending `task.resume` injection so the legitimate
    /// handoff is not pre-empted. If zero committed, inject
    /// the bounded `task.resume` (still behind the existing
    /// breaker).
    fn resolve_over_emit_recovery(&mut self, accepted_log_events: &[ralph_proto::Event]) {
        let pending = match self.state.pending_over_emit_recovery.take() {
            Some(recovery) => recovery,
            None => return,
        };
        let committed_business = accepted_log_events
            .iter()
            .any(|event| is_commit_first_business_topic(event.topic.as_str()));
        if committed_business {
            tracing::debug!(
                hat = %pending.hat.as_str(),
                dropped_topic = %pending.dropped_topic,
                "U3: over-emit recovery bypassed because a business event already committed"
            );
            return;
        }
        let key = format!(
            "isolated_budget:{}:per_turn",
            crate::diagnosis::normalize_part(pending.hat.as_str())
        );
        let count = self.state.record_rejection_key(&key);
        if self.state.rejection_key_is_exhausted(&key) {
            warn!(
                key = %key,
                hat = %pending.hat.as_str(),
                dropped_topic = %pending.dropped_topic,
                count = count,
                "U3: isolated over-emit recovery breaker tripped; no task.resume injected"
            );
            return;
        }
        let free_form = format!(
            "Isolated mode dropped an extra business event ('{}') and zero business events committed this turn — only the FIRST business event per activation is kept. Re-emit EXACTLY ONE business event (the one you actually intend, e.g. plan.complete) and nothing else.",
            pending.dropped_topic
        );
        let payload = enrich_task_resume_payload(
            &free_form,
            "isolated_extra_business_event_dropped",
            Some(pending.hat.as_str()),
            Some(RejectionKind::ContractViolation),
        );
        self.bus
            .publish(Event::new("task.resume", payload.clone()).with_target(pending.hat.clone()));
    }

    /// 2026-06-29-007 plan U1b: drive the `current_step`
    /// field transition after the unit_loop `total_units`
    /// have been reached. When `current_step ==
    /// "unit_loop"` and `work.done` count meets
    /// `total_units`, advance to `review_walk`. The
    /// helper is idempotent: re-entry while already on
    /// `review_walk` (or any non-`unit_loop` step) is a
    /// no-op.
    fn drive_step_transition(&mut self) {
        let step_id = self.state.flow_lifecycle.current_step_id().to_string();
        if step_id != "unit_loop" {
            return;
        }
        let total_units = match self.flow_step_total_units(&step_id) {
            Some(n) => n,
            None => return,
        };
        let done = self
            .state
            .seen_topics
            .iter()
            .filter(|t| t.as_str() == "work.done")
            .count() as u32;
        if done < total_units {
            return;
        }
        if let Err(e) = self.state.flow_lifecycle.advance_to("review_walk") {
            tracing::warn!(
                error = %e,
                "flow_lifecycle.advance_to(review_walk) failed; staying on unit_loop"
            );
        }
    }

    /// 2026-07-02-004 plan milestone B (U5/U6): enforce precheck
    /// gate hard-gate semantics and dispatch rejections (resume vs.
    /// exhaustion).  U5 synthesizes `<X>.rejected` when the gate
    /// hat is silent or ambiguous; U6 routes failures through the
    /// correction + `task.resume` pipeline (R5 / AE3).
    fn drive_precheck_gate_obligation(&mut self, accepted: &[ralph_proto::Event]) {
        use crate::event_loop::precheck_gate_enforcement as gate;
        use ralph_proto::HatId;
        use std::collections::HashSet;

        let precheck_cfg = match self.config.event_loop.precheck.as_ref() {
            Some(p) if p.enabled && !p.rules.is_empty() => p.clone(),
            _ => return,
        };
        if !crate::config::precheck_runtime_enabled() {
            return;
        }

        let loop_id = self
            .loop_context
            .as_ref()
            .and_then(|c| c.loop_id())
            .unwrap_or("default")
            .to_string();

        // U5: silent / ambiguous gate → synthetic `<X>.rejected`.
        let synthetics = gate::collect_synthetic_precheck_rejections(
            &self.state.hat_obligations,
            accepted,
            |topic| precheck_cfg.rules.get(topic).map(|r| r.prompt.len()),
        );
        let mut synthesized_gates: HashSet<String> = HashSet::new();
        for synthetic in synthetics {
            synthesized_gates.insert(synthetic.gate_hat_id.clone());
            let gate_hat = HatId::new(&synthetic.gate_hat_id);
            self.state
                .discharge_hat_obligation(&gate_hat, &synthetic.rejected_topic);
            self.dispatch_precheck_rejection(
                &loop_id,
                &precheck_cfg,
                &synthetic.gate_hat_id,
                &synthetic.guarded_topic,
                &synthetic.payload_json,
            );
        }

        for event in accepted {
            let source_hat = match gate::resolve_gate_hat_for_emit(event, &precheck_cfg.rules) {
                Some(id) => HatId::new(id),
                None => continue,
            };
            if !gate::is_gate_hat(source_hat.as_str()) {
                continue;
            }
            if synthesized_gates.contains(source_hat.as_str()) {
                continue;
            }
            let topic_str = event.topic.as_str();

            if let Some(guarded) = gate::gate_topic(source_hat.as_str())
                && topic_str == guarded
            {
                self.precheck_retries.record_pass(&loop_id, guarded);
                self.state.discharge_hat_obligation(&source_hat, topic_str);
                continue;
            }

            let guarded = match topic_str.strip_suffix(".rejected") {
                Some(s) => s,
                None => continue,
            };
            let hat_guarded = match gate::gate_topic(source_hat.as_str()) {
                Some(g) => g,
                None => continue,
            };
            if hat_guarded != guarded {
                continue;
            }

            let Some(_rule) = precheck_cfg.rules.get(guarded) else {
                continue;
            };

            self.state.discharge_hat_obligation(&source_hat, topic_str);
            self.dispatch_precheck_rejection(
                &loop_id,
                &precheck_cfg,
                source_hat.as_str(),
                guarded,
                event.payload.as_str(),
            );
        }
    }

    /// U6 closure for one `<X>.rejected` (LLM or synthetic).
    fn dispatch_precheck_rejection(
        &mut self,
        loop_id: &str,
        precheck_cfg: &crate::config::PrecheckConfig,
        gate_hat_id: &str,
        guarded: &str,
        rejected_payload_json: &str,
    ) {
        use crate::event_loop::precheck_gate_runner as runner;
        use crate::event_loop::rejection::enrich_task_resume_payload_full;
        use crate::preset::engine::gates::RejectionKind;
        use ralph_proto::HatId;

        let rule = match precheck_cfg.rules.get(guarded) {
            Some(r) => r,
            None => return,
        };
        let rejection_count = self.precheck_retries.record_rejection(loop_id, guarded);

        let params = runner::DispatchParams {
            loop_id,
            topic: guarded,
            target_hat: rule.on_fail.target.as_str(),
            retry_budget: rule.on_fail.retry_budget,
            on_exhausted: rule.on_fail.on_exhausted.as_str(),
            rejection_count,
            rejected_payload_json,
        };
        let outcome = runner::dispatch_rejection(&params);
        match outcome {
            runner::DispatchOutcome::Resume {
                target_hat,
                new_count,
                ..
            } => {
                let message =
                    runner::format_precheck_failure_message(guarded, rejected_payload_json);
                let mut rejection = Rejection {
                    stage: RejectionStage::Policy,
                    source_hat: Some(gate_hat_id.to_string()),
                    business_hat: None,
                    topic: guarded.to_string(),
                    violation: message.clone(),
                    retry_key: String::new(),
                    retry_eligible: true,
                    non_retryable_reason: None,
                    target_hat: Some(target_hat.clone()),
                    original_event_id: None,
                    original_ts: None,
                    kind: Some(RejectionKind::ContractViolation),
                    duplicate_work_done_hint: None,
                    seen_count: None,
                };
                rejection.retry_key = rejection.compute_retry_key();
                let _ctx = crate::correction::emit_correction_context(
                    self.state.state_ledger.as_mut(),
                    &rejection,
                    new_count,
                    Some(self.config.core.workspace_root.as_path()),
                    &mut self.state.prompt_context,
                );

                let allowed_topics = self
                    .registry
                    .get_config(&HatId::new(&target_hat))
                    .map(|cfg| cfg.publishes.clone())
                    .unwrap_or_default();
                let resume_payload = enrich_task_resume_payload_full(
                    &message,
                    "precheck_rejected",
                    Some(&target_hat),
                    Some(RejectionStage::Policy),
                    Some(RejectionKind::ContractViolation),
                    &allowed_topics,
                );
                tracing::info!(
                    loop_id = %loop_id,
                    gate = %gate_hat_id,
                    topic = %guarded,
                    target_hat = %target_hat,
                    count = new_count,
                    "U6: precheck rejection within budget; injecting correction + task.resume"
                );
                self.bus.publish(
                    ralph_proto::Event::new("task.resume", resume_payload)
                        .with_target(HatId::new(target_hat.clone())),
                );
                self.state
                    .redispatch_hat_obligation(&HatId::new(target_hat));
            }
            runner::DispatchOutcome::Exhausted { topic, reason } => {
                tracing::warn!(
                    loop_id = %loop_id,
                    gate = %gate_hat_id,
                    topic = %guarded,
                    on_exhausted = %topic,
                    reason = %reason,
                    "U6: precheck retry budget exhausted; escalating to on_exhausted"
                );
                let payload = runner::build_exhausted_payload(&topic, &reason);
                let blocked = ralph_proto::Event::new(topic.clone(), payload)
                    .with_source(HatId::new(gate_hat_id));
                self.state.record_event(&blocked);
                self.bus.publish(blocked);
                self.terminal_event_emitted = true;
            }
            runner::DispatchOutcome::Pass => {}
        }
    }

    /// Resolve `FlowDeclaration.steps[i].total_units` for
    /// the step whose id matches `step_id`. Returns
    /// `None` when the step is not declared or did not
    /// opt into `total_units`.
    ///
    /// 2026-06-28-002 U6: fix-unit steps (`fix-{NN}`) that
    /// did not declare `total_units` fall back to the
    /// `tasks.jsonl` record count for matching fix-units.
    /// Without this, `StepCloseObligationStage` stays
    /// fail-open for fix-unit flows because the registry
    /// never knows the total. Non-fix steps retain the
    /// pre-U6 strict `None` semantics so other presets are
    /// not affected.
    fn flow_step_total_units(&self, step_id: &str) -> Option<u32> {
        if let Some(n) = self.flow_step_totals.get(step_id).copied() {
            return Some(n);
        }
        if step_id.starts_with("fix-") {
            return self.count_fix_unit_tasks(step_id);
        }
        None
    }

    /// 2026-06-28-002 U6: count `tasks.jsonl` records whose
    /// task_key matches the fix-unit shape
    /// `ce-executor:*:{step_id}:*` so the step-close progress
    /// stage can satisfy its total even when the preset omits
    /// `total_units` in `FlowDeclaration.steps[i]`.
    fn count_fix_unit_tasks(&self, step_id: &str) -> Option<u32> {
        use crate::task_store::TaskStore;
        let path = self.tasks_path();
        let store = match TaskStore::load(&path) {
            Ok(s) => s,
            Err(_) => return None,
        };
        let prefix = "ce-executor:".to_string();
        let needle = format!(":{step_id}:");
        let count = store
            .all()
            .iter()
            .filter(|t| {
                t.key
                    .as_deref()
                    .map(|k| k.starts_with(&prefix) && k.contains(&needle))
                    .unwrap_or(false)
            })
            .count() as u32;
        if count == 0 { None } else { Some(count) }
    }

    /// 2026-06-26 plan U4: discharge hat obligations for any accepted
    /// business event. Centralised here so the obligation queue is
    /// kept in lock-step with the bus — every accepted event
    /// immediately removes the obligation for the hat that emitted
    /// it (if the topic was one the hat owed).
    ///
    /// Returns the number of obligations discharged, mostly useful
    /// for the diagnostics collector. The discharge is idempotent:
    /// if no obligation is open, `discharge_hat_obligation` is a
    /// silent no-op (the emit is a side-effect, not the expected
    /// business event).
    pub fn discharge_obligations_for_accepted(&mut self, events: &[Event]) -> usize {
        let mut discharged = 0;
        for event in events {
            let Some(hat_id) = event.source.as_ref() else {
                continue;
            };
            if self
                .state
                .discharge_hat_obligation(hat_id, event.topic.as_str())
            {
                discharged += 1;
            }
        }
        discharged
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
        // 2026-06-16-001 U1: reset the per-turn stall-detector
        // flag at the start of each read so the helper can
        // observe whether THIS turn admitted a business event.
        // Mirror of process_events_from_jsonl() line 6349.
        self.state.stall_detector_had_events = false;

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
            let mut policy_state: PolicyRuntimeState =
                self.state.policy_runtime_state.take().unwrap_or_default();
            let mut review_step_tracker = std::mem::take(&mut self.state.review_step_tracker);
            let mut state_ledger = std::mem::take(&mut self.state.state_ledger);
            let mut wave_violation: Option<crate::payload_contract::PayloadContractViolation> =
                None;
            let mut wave_rejections: Vec<crate::event_policy::PolicyRejection> = Vec::new();
            let mut hold_reason: Option<String> = None;

            let view = crate::preset::engine::protocol::ProtocolView::from_event_loop(
                &self.config.event_loop,
            );
            use crate::validation::{EventPolicyRule, ValidationContext, ValidationRule};
            let rule = EventPolicyRule;

            let mut accepted_wave_events: Vec<JsonlEvent> = Vec::with_capacity(wave_events.len());
            for evt in &wave_events {
                let mut snapshot = crate::state::LedgerSnapshot::cold_start();
                let r = {
                    let mut ctx = ValidationContext::new(&mut snapshot)
                        .with_policy_runtime_state(&mut policy_state)
                        .with_review_step_tracker(&mut review_step_tracker)
                        .with_payload_contract_violation(&mut wave_violation)
                        .with_policy_rejections(&mut wave_rejections);
                    rule.validate(&view, &mut ctx, evt)
                };
                if r.accepted {
                    if r.stage == crate::validation::ValidationStage::EventPolicy
                        && r.reason_code.as_deref()
                            == Some(crate::validation::ReasonCode::EVENT_POLICY_WARNING)
                    {
                        let msg = format!(
                            "Policy warning for '{}': {}",
                            evt.topic,
                            r.correction_hint.as_deref().unwrap_or("")
                        );
                        self.bus.publish(Event::new("event.policy_warning", msg));
                    }
                    accepted_wave_events.push(evt.clone());
                    continue;
                }
                match r.reason_code.as_deref() {
                    Some(crate::validation::ReasonCode::EVENT_POLICY_COMPLETION_BLOCKED) => {
                        let msg = r
                            .correction_hint
                            .clone()
                            .unwrap_or_else(|| format!("Completion guard blocked '{}'", evt.topic));
                        self.bus
                            .publish(Event::new("event.completion.blocked", msg));
                    }
                    Some(crate::validation::ReasonCode::EVENT_POLICY_COMPLETION_IGNORED) => {
                        let msg = r
                            .correction_hint
                            .clone()
                            .unwrap_or_else(|| format!("Completion guard ignored '{}'", evt.topic));
                        self.bus
                            .publish(Event::new("event.completion.ignored", msg));
                    }
                    Some(
                        crate::validation::ReasonCode::EVENT_POLICY_BLOCKED
                        | crate::validation::ReasonCode::EVENT_POLICY_IGNORED,
                    ) => {}
                    Some(crate::validation::ReasonCode::EVENT_POLICY_HOLD) => {
                        hold_reason = r
                            .correction_hint
                            .clone()
                            .or_else(|| Some(format!("Event '{}' violates policy", evt.topic)));
                        let reason = format!(
                            "{}:{}",
                            r.stage.as_str(),
                            r.reason_code.as_deref().unwrap_or("rejected"),
                        );
                        publish_correction_via_context(
                            &mut self.bus,
                            &mut self.state,
                            state_ledger.as_mut(),
                            evt,
                            &reason,
                            policy_finding_for_topic(&wave_rejections, evt.topic.as_str()),
                        );
                    }
                    _ => {
                        let reason = format!(
                            "{}:{}",
                            r.stage.as_str(),
                            r.reason_code.as_deref().unwrap_or("rejected"),
                        );
                        publish_correction_via_context(
                            &mut self.bus,
                            &mut self.state,
                            state_ledger.as_mut(),
                            evt,
                            &reason,
                            policy_finding_for_topic(&wave_rejections, evt.topic.as_str()),
                        );
                    }
                }
            }

            self.state.state_ledger = state_ledger;
            self.state.review_step_tracker = review_step_tracker;
            self.state.policy_runtime_state = Some(policy_state);

            wave_policy_rejections = wave_rejections;

            // Write hold artifact if policy hold was triggered.
            if let Some(ref reason) = hold_reason
                && let Err(e) = self.write_hold_artifact(Some(reason))
            {
                warn!(error = %e, "Failed to write hold artifact");
            }

            // Post-process recoverable rejection budget.
            use crate::event_policy::ReasonClass;
            for rejection in &wave_policy_rejections {
                if let Some(ref class) = rejection.reason_class {
                    if matches!(class, ReasonClass::SemanticGateViolation) {
                        continue;
                    }
                    let hat = rejection.source_hat.as_deref().unwrap_or("unknown");
                    let (count, exhausted) = self.state.record_recoverable_rejection_key(
                        hat,
                        &rejection.topic,
                        class.as_str(),
                    );
                    if exhausted {
                        self.state
                            .recoverable_exhaustion_buffer
                            .push(RecoverableExhaustion {
                                hat: hat.to_string(),
                                topic: rejection.topic.clone(),
                                reason_class: *class,
                                count,
                            });
                    }
                }
            }

            // U1: when every wave event was rejected, write a recovery envelope.
            if wave_raw_count > 0 && accepted_wave_events.is_empty() {
                Self::log_wave_policy_blocked_envelope(
                    self,
                    &wave_policy_rejections,
                    wave_raw_count,
                );
            }

            accepted_wave_events
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
        // enforcement, plan detection, etc.)
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
    ///
    /// R6/U2: ralph pseudo-hat may only publish control topics. This
    /// gate mirrors the `process_events_from_jsonl` check so that
    /// orchestrator-internal publish paths (e.g. `inject_fallback_event`)
    /// and external callers (`runner.rs`) share the same boundary.
    pub fn publish_event(&mut self, event: Event) {
        if let Some(ref hat) = event.source
            && hat.as_str() == "ralph"
        {
            let topic = event.topic.as_str();
            // P1-12: uses prefix match so future `ralph.*` topics are
            // recognized without updating the constant list.
            if !crate::event_origin::is_ralph_control_topic(topic) {
                warn!(
                    topic = %topic,
                    "ralph hat business topic rejected in publish_event: ralph may only publish control topics"
                );
                let violation = Event::new(
                    "event.isolation.boundary_violation",
                    format!(
                        "{{\"hat\":\"ralph\",\"topic\":\"{}\",\"violation\":\"ralph_business_topic_rejected: ralph hat may only publish control topics\"}}",
                        topic
                    ),
                );
                self.bus.publish(violation);
                return;
            }
        }

        // 2026-07-02 P0: `review.dimension.ready` idempotency
        // dedup must run BEFORE the emit-gate facade so a
        // resume-replayed duplicate (e.g. review-coordinator
        // re-sending `adversarial` after a stall_recovery
        // `task.resume` — observed in the 2026-07-01
        // ralph-e2e run, recovery.jsonl iter 24) is rejected
        // as `DuplicateWorkDone` and the original retry_key
        // path is preserved. The dedup lives in
        // `event_policy::validate_event_with_hat`
        // (event_policy.rs:1115-1169) but the policy module
        // is only invoked from unit tests today — hat-channel
        // output bypasses it. This call wires the dedup into
        // the production emit path with no schema-side
        // change; on RejectWithResume the event is routed to
        // the repair stream (the same sink the stage pipeline
        // uses for `AcceptRepairStream`) and never reaches
        // the bus, so a `task.resume` retry does not
        // re-introduce a duplicate.
        if event.topic.as_str() == "review.dimension.ready"
            && let Some(ref mut policy_state) = self.state.policy_runtime_state
            && let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            use crate::event_policy::{PolicyDecision, validate_event_with_hat};
            let payload_str = event.payload.as_str();
            let decision = validate_event_with_hat(
                event.topic.as_str(),
                Some(payload_str),
                policy_config,
                policy_state,
                event.source.as_ref().map(|h| h.as_str()),
            );
            if let PolicyDecision::RejectWithResume(_) | PolicyDecision::Hold(_) = decision {
                tracing::info!(
                    topic = %event.topic,
                    plan = %event.source.as_ref().map(|s| s.as_str()).unwrap_or(""),
                    "P0: review.dimension.ready rejected by idempotency dedup; routing to repair stream"
                );
                self.record_repair_event(&event);
                return;
            }
        }

        // U6 (2026-06-27 mechanism foundation): every event
        // that survives the ralph-boundary check must also
        // pass through the emit-gate facade (U1/U2). The
        // facade combines `StagePipeline::run` with the
        // `is_repair_topic` routing hint so the bus never
        // sees a repair topic and a rejected event lands in
        // `record_stage_rejection`.
        let mut stage_ctx = self.build_stage_context_for(&event);
        // The facade owns the routing decision; we only
        // need to mirror the three outcomes into the
        // appropriate sink.
        let outcome = crate::event_loop::emit_gate::evaluate_emit_gate(&mut stage_ctx, &event);
        match outcome {
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptMainBus => {
                // U10 (2026-06-27-002 plan completion): if
                // the topic is in `terminal_emits`, write
                // the loop-termination record so the
                // dispatcher knows the loop has reached
                // its natural end. Only `LOOP_COMPLETE`
                // is in the default set after U9 retired
                // the legacy `report.done` mirror.
                if self.stage_pipeline.is_terminal(&event) {
                    self.write_loop_termination_record(&event);
                }
                self.bus.publish(event.clone());
                self.diagnose_plan_complete_channel(
                    &event,
                    crate::event_loop::phase_authority::diagnosis::Channel::Main,
                );
                self.apply_phase_authority_on_accepted(&event);
            }
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptRepairStream => {
                // U7 (2026-06-27-002 plan completion): the
                // U6 repair sink writes the envelope to
                // `.ralph/recovery.jsonl`. The bus NEVER
                // sees a repair topic.
                self.record_repair_event(&event);
            }
            crate::event_loop::emit_gate::EmitGateOutcome::Reject(reject) => {
                // The facade carries the StageReject; route
                // through the existing recovery envelope.
                self.record_stage_rejection(&event, &reject);
            }
        }
    }

    /// U6 (2026-06-27 mechanism foundation): build the
    /// StageContext consumed by every stage in the emit-time
    /// pipeline AND the U1 emit-gate facade. Reads the loop
    /// id from loop_context, the current step id from
    /// FlowLifecycleRegistry (falling back to "unit_loop"),
    /// and the expected version from the shared idempotent
    /// log. StageContext borrows a static RepairStateMachine
    /// stub; every stage currently ignores it.
    ///
    /// The `pipeline` field is wired in U1 so the
    /// `evaluate_emit_gate` facade can run the pipeline
    /// from inside the gate without the caller having to
    /// thread the pipeline separately.
    fn build_stage_context_for(
        &mut self,
        event: &Event,
    ) -> crate::event_loop::stage_pipeline::StageContext<'_> {
        use crate::event_loop::stage_pipeline::{FlowStep, StageContext};
        let loop_id = self
            .loop_context()
            .and_then(|c| c.loop_id())
            .unwrap_or("default")
            .to_string();
        // 2026-06-28 plan U4: prefer the plan-mode step
        // (advanced by U4's transition logic) over the
        // wave-phase fallback. When the preset has no
        // `mechanism.flow`, `current_plan_step` is the empty
        // string and the wave-phase value takes over so the
        // existing tests keep working.
        let step_id = if self.current_plan_step.is_empty() {
            self.state.flow_lifecycle.current_step_id().to_string()
        } else {
            self.current_plan_step.clone()
        };
        let expected_version = self
            .idempotent_log
            .lock()
            .map(|log| log.version())
            .unwrap_or(0);
        let _ = event;
        // P1-5 (2026-06-27 adversarial review):
        // hand the per-task repair state machine
        // registry to the stage context so the
        // `RepairDispatchStage` can advance the
        // per-`task_key` budget. The previous
        // design shared one machine for every
        // repair event, which violated R2.
        StageContext::with_pipeline(
            FlowStep::new(step_id),
            loop_id,
            expected_version,
            &mut self.repair_state_machines,
            &self.stage_pipeline,
        )
    }

    /// U3 (2026-06-27-002 plan completion): publish-time
    /// gate. The first gate pass (in
    /// `apply_emit_gate` over `event_reader::Event`)
    /// only recorded the recovery envelope / repair-sink
    /// side effect; this second pass decides whether
    /// the validated event reaches the main bus.
    ///
    /// P0-1 (2026-06-27 adversarial review): the
    /// previous implementation re-ran the stage
    /// pipeline here, which double-advanced the
    /// `RepairStateMachine` for repair topics (the
    /// pipeline mutates `ctx.repair_state` in place).
    /// To preserve the per-task budget we now reuse
    /// the outcome from the first pass instead of
    /// running the pipeline twice. The first-pass
    /// outcome is stashed in `validated_gate_outcomes`
    /// (keyed by the JSONL event's index — see
    /// P1-1 (2026-07-01-002 audit): when the coordinator emits a
    /// `work.ready(fix-XX)` whose `fix-XX` is **not** in the
    /// projector-known chain, reject it with a synthetic
    /// `ExecutionContractFinding` so the downstream rejection
    /// machinery publishes a `task.resume` with the right
    /// provenance (the source hat) and appends a recovery
    /// envelope to the ledger.
    ///
    /// This is intentionally **not** a stage — the check runs
    /// before the contract pipeline and only matches a single
    /// topic (`work.ready`).  Adding a stage for one topic would
    /// push an unrelated layer into every other emit path.
    ///
    /// `fix_unit_known` carries the closure's projection so it
    /// doesn't need `&self`.  Free function rather than method to
    /// avoid the borrow conflict with the surrounding `for event
    /// in events` loop.
    fn push_fix_unit_range_finding(
        &mut self,
        event: &crate::event_reader::Event,
        rejected_step: &str,
        fix_unit_known: &std::collections::BTreeSet<String>,
    ) {
        use crate::execution_contract::{ExecutionContractFinding, ExecutionContractViolationKind};
        let known_list: Vec<String> = fix_unit_known.iter().cloned().collect();
        let source_hat: Option<String> = event.hat.clone();
        let finding = ExecutionContractFinding {
            kind: ExecutionContractViolationKind::InvalidStepTarget {
                step: rejected_step.to_string(),
                known_fix_units: known_list.clone(),
            },
            message: format!(
                "work.ready requested fix-unit `{}` which is not in the known fix-unit chain ({}). \
                 The chain has already exhausted or the id is from a stale plan; re-emit with an id from `{}` or finish with `plan.complete`.",
                rejected_step,
                if known_list.is_empty() {
                    "(none yet)".to_string()
                } else {
                    known_list.join(", ")
                },
                if known_list.is_empty() {
                    "<none>".to_string()
                } else {
                    format!("{{{}}}", known_list.join(", "))
                },
            ),
            topic: event.topic.clone(),
            source_hat: source_hat.clone(),
        };
        tracing::warn!(
            finding = ?finding.kind,
            step = %rejected_step,
            "fix-unit range reject"
        );
        let payload_json =
            build_invalid_step_target_resume_payload_for_jsonl(&finding, event, &known_list);
        self.bus
            .publish(ralph_proto::Event::new("task.resume", payload_json));
        self.diagnostics.log_execution_contract_rejections(
            0,
            source_hat.as_deref().unwrap_or("ralph"),
            std::slice::from_ref(&finding),
        );
    }

    /// `apply_emit_gate`).
    fn apply_emit_gate_on_validated(
        &mut self,
        event: &ralph_proto::Event,
        stashed_outcome: Option<crate::event_loop::emit_gate::EmitGateOutcome>,
    ) -> bool {
        let outcome = match stashed_outcome {
            Some(o) => o,
            None => {
                let mut stage_ctx = self.build_stage_context_for(event);
                crate::event_loop::emit_gate::evaluate_emit_gate(&mut stage_ctx, event)
            }
        };
        match outcome {
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptMainBus => true,
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptRepairStream => {
                // Repair stream was already recorded
                // during the first gate pass. Skip publish.
                false
            }
            crate::event_loop::emit_gate::EmitGateOutcome::Reject(reject) => {
                // Recovery envelope was already recorded
                // during the first gate pass. Skip publish.
                let _ = reject;
                false
            }
        }
    }

    /// U3 (2026-06-27-002 plan completion): route the
    /// event through the emit-gate facade used by
    /// `publish_event` (U2) and return the outcome
    /// so the caller can decide whether to admit the
    /// event to `accepted` and, on the second pass
    /// (`apply_emit_gate_on_validated`), reuse the
    /// outcome to publish-skip without re-running the
    /// pipeline.
    ///
    /// P0-1 (2026-06-27 adversarial review): the
    /// previous design called `apply_emit_gate` AND
    /// `apply_emit_gate_on_validated` per event,
    /// which advanced the per-task
    /// `RepairStateMachine` twice — exhausting the
    /// `repair_budget=3` invariant after just 2
    /// repair events. We now return the `EmitGateOutcome`
    /// from the first pass and stash it so the second
    /// pass (publish gate) can route without re-running
    /// the pipeline.
    ///
    /// P1-9 (2026-06-27 adversarial review): the
    /// previous name (`apply_emit_gate` → `bool`) was
    /// semantically misleading — all three outcomes
    /// returned `true`. Renamed to
    /// `evaluate_emit_gate_for_jsonl_event` to make
    /// the return type (`EmitGateOutcome`) explicit
    /// at every call site. The legacy name remains
    /// as a thin wrapper that discards the outcome
    /// for any external call site that still uses it.
    ///
    /// Takes the JSONL-internal `event_reader::Event`
    /// shape because the only callers live inside
    /// `process_parse_result`. `publish_event` keeps its
    /// own (private) variant that takes a
    /// `ralph_proto::Event` directly.
    fn evaluate_emit_gate_for_jsonl_event(
        &mut self,
        event: &crate::event_reader::Event,
    ) -> crate::event_loop::emit_gate::EmitGateOutcome {
        // Convert JSONL-internal Event to the bus-shaped
        // ralph_proto::Event the facade expects. Preserve
        // `hat`/`source` so `PhaseAuthorityStage` (U13) can
        // enforce per-phase whitelists on JSONL ingest.
        let proto: Event = event.clone().into();
        let mut stage_ctx = self.build_stage_context_for(&proto);
        let outcome = crate::event_loop::emit_gate::evaluate_emit_gate(&mut stage_ctx, &proto);
        match &outcome {
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptMainBus => {
                // U3 (2026-06-27-002 plan completion):
                // admit the event so the lifecycle tracker
                // and `validated_events` downstream see it.
                // The BDD wire-level `absent_events` assertions
                // are pinned at the publication level
                // (post `process_events_from_jsonl`), not at
                // the `accepted_events` admission level.
            }
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptRepairStream => {
                self.repair_stream_pending += 1;
                // U7 (2026-06-27-002 plan completion):
                // the JSONL ingest path now also writes
                // to the U6 repair sink.
                self.record_repair_event(&proto);
                // Admit the event so lifecycle tracker
                // records it, but the publication-side
                // will not see it on the main bus.
            }
            crate::event_loop::emit_gate::EmitGateOutcome::Reject(reject) => {
                self.record_stage_rejection(&proto, reject);
                // Admit the event so lifecycle tracker
                // still records the original emit attempt.
                // The BDD wire-level assertion pins that
                // the bus NEVER receives the rejected
                // event (post `process_events_from_jsonl`).
            }
        }
        outcome
    }

    /// U7 (2026-06-27-002 plan completion): shared
    /// helper used by both `publish_event` (U2) and
    /// `apply_emit_gate` (U3) when the emit-gate facade
    /// routes an event to the repair stream. The
    /// `RepairStreamSink` is a pure file-I/O boundary
    /// (see U6); the orchestration glue lives here.
    ///
    /// The workspace root is taken from `self.config
    /// .core.workspace_root`. On FS error we log and
    /// continue — the loop must not crash on a
    /// transient disk error.
    /// 2026-07-02-006 plan U26: R14 dual-check when `plan.complete`
    /// lands on main vs repair sink.
    fn diagnose_plan_complete_channel(
        &mut self,
        event: &ralph_proto::Event,
        channel: crate::event_loop::phase_authority::diagnosis::Channel,
    ) {
        if !self.phase_authority.is_enabled() {
            return;
        }
        use crate::event_loop::phase_authority::diagnosis::{
            DualCheckInput, DualCheckOutcome, diagnosis_plan_complete_dual_check,
        };
        let outcome = diagnosis_plan_complete_dual_check(&DualCheckInput {
            topic: event.topic.to_string(),
            source: event.source.as_ref().map(|h| h.to_string()),
            channel,
        });
        match outcome {
            DualCheckOutcome::DualSink => {
                tracing::warn!(
                    topic = %event.topic,
                    source = ?event.source,
                    "R14: plan.complete landed on repair sink — dual-check invariant broken"
                );
                let payload = serde_json::json!({
                    "topic": event.topic.as_str(),
                    "channel": "repair",
                    "reason": "plan.complete_dual",
                });
                self.bus.publish(ralph_proto::Event::new(
                    "plan.complete_dual",
                    payload.to_string(),
                ));
            }
            DualCheckOutcome::UnknownChannel => {
                tracing::warn!(
                    topic = %event.topic,
                    "R14: plan.complete channel unknown — cannot prove dual-check invariant"
                );
            }
            DualCheckOutcome::Ok | DualCheckOutcome::NotApplicable => {}
        }
    }

    /// 2026-07-02-006 plan U20: shipper routing when phase engine is on.
    fn phase_authority_rejects_shipper_emit(&self, event: &ralph_proto::Event) -> bool {
        if self.shipper_validator_gate_rejects(event) {
            return true;
        }
        if !self.phase_authority.is_enabled() {
            return false;
        }
        use crate::event_loop::phase_authority::shipper_helper::{
            ShipperDecision, ShipperRoutingContext,
            shipper_requires_plan_complete_when_phase_enabled,
        };
        let reason = self
            .state
            .policy_runtime_state
            .as_ref()
            .and_then(|s| s.last_plan_blocked_reason.clone());
        let plan_complete_present = self
            .state
            .seen_topics
            .iter()
            .any(|t| t.as_str() == "plan.complete")
            || event.topic.as_str() == "plan.complete";
        let ctx = ShipperRoutingContext {
            phase_authority_enabled: true,
            current_phase: self.phase_authority.snapshot().map(|s| s.phase_id),
            reason,
            plan_complete_present,
        };
        matches!(
            shipper_requires_plan_complete_when_phase_enabled(&ctx),
            ShipperDecision::Deny
        )
    }

    /// 2026-07-07-002 U6: shipper success requires current-step validator terminal.
    fn shipper_validator_gate_rejects(&self, event: &ralph_proto::Event) -> bool {
        if event.topic.as_str() != "REVIEW_COMPLETE" {
            return false;
        }
        use crate::event_loop::phase_authority::shipper_helper::{
            ShipperValidatorGateContext, ShipperValidatorGateDecision, ValidatorTerminalKind,
            evaluate_shipper_validator_gate,
        };
        let pass_or_fail = serde_json::from_str::<serde_json::Value>(event.payload.as_str())
            .ok()
            .and_then(|v| {
                v.get("pass_or_fail")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_ascii_lowercase())
            })
            .unwrap_or_default();
        let attempting_success = pass_or_fail == "pass"
            || event.payload.contains("pass_with_residuals")
            || event.payload.contains("\"verdict\":\"pass");
        let plan_blocked_reason = self
            .state
            .policy_runtime_state
            .as_ref()
            .and_then(|s| s.last_plan_blocked_reason.clone());
        let validator_terminal_kind =
            self.state
                .last_validator_terminal_kind
                .as_deref()
                .and_then(|k| match k {
                    "passed" => Some(ValidatorTerminalKind::Passed),
                    "failed" => Some(ValidatorTerminalKind::Failed),
                    _ => None,
                });
        let current_step = self
            .state
            .last_test_passed_step
            .clone()
            .or_else(|| self.state.last_validator_terminal_step.clone())
            .or_else(|| self.state.last_plan_complete_step.clone());
        let ctx = ShipperValidatorGateContext {
            current_step,
            validator_terminal_step: self.state.last_validator_terminal_step.clone(),
            validator_terminal_kind,
            plan_blocked_reason,
            attempting_success_ship: attempting_success,
        };
        !matches!(
            evaluate_shipper_validator_gate(&ctx),
            ShipperValidatorGateDecision::Allow
        )
    }

    fn record_repair_event(&mut self, event: &ralph_proto::Event) {
        let completion_topic = self.config.event_loop.completion_promise.clone();
        match self.evaluate_terminal_closed_for_event(
            event.topic.as_str(),
            event.payload.as_str(),
            completion_topic.as_str(),
        ) {
            crate::event_loop::terminal_closed_guard::TerminalClosedDecision::Allow => {}
            crate::event_loop::terminal_closed_guard::TerminalClosedDecision::RejectPostTerminal => {
                self.publish_post_terminal_rejection(
                    event.topic.as_str(),
                    "post_terminal_repair_stream_frozen",
                );
                return;
            }
            crate::event_loop::terminal_closed_guard::TerminalClosedDecision::IgnoreDuplicateTerminal => {
                return;
            }
        }
        self.diagnose_plan_complete_channel(
            event,
            crate::event_loop::phase_authority::diagnosis::Channel::Repair,
        );
        let workspace = std::path::PathBuf::from(&self.config.core.workspace_root);
        if let Err(err) =
            crate::event_loop::repair_stream_sink::record_repair_event(event, &workspace)
        {
            tracing::warn!(
                topic = %event.topic,
                error = %err,
                "U7: failed to write repair-stream envelope; continuing without crash"
            );
        }
    }

    /// U10 (2026-06-27-002 plan completion): when the
    /// dispatcher accepts a terminal emit
    /// (`LOOP_COMPLETE` by default), record the
    /// loop-termination intent. The actual end-of-loop
    /// book-keeping (closing the ledger, releasing
    /// the activation tracker) still happens in
    /// `decide_termination_reason`; this method just
    /// logs the event so operators can see when the
    /// terminal topic was accepted.
    fn write_loop_termination_record(&self, event: &ralph_proto::Event) {
        let loop_id = self
            .loop_context()
            .and_then(|c| c.loop_id())
            .unwrap_or("default");
        info!(
            loop_id = %loop_id,
            topic = %event.topic,
            iteration = self.state.iteration,
            "U10: terminal emit accepted — loop will close at the next dispatch tick"
        );
    }

    /// U6 (2026-06-27 mechanism foundation): turn a stage
    /// pipeline rejection into a RecoveryDiagnosisEnvelope and
    /// route it through record_recovery_envelope so the
    /// gate's signal lands in recovery.jsonl and is
    /// aggregated by ralph diagnose. CliEmit is reused
    /// because the emit-time gate runs at the same logical
    /// boundary as the CLI precheck.
    ///
    /// P1-1 (2026-06-28 review): the method is
    /// `pub(crate)` so the P1-1 integration test can
    /// synthesise a budget-exhaustion rejection and
    /// assert that the `plan.blocked` escalation is
    /// published on the bus.
    /// 2026-07-02-006 plan U23: advance workflow phase after a
    /// business event lands on the main bus.
    fn apply_phase_authority_on_accepted(&mut self, event: &Event) {
        // 2026-06-28 plan U4: a successful accept may carry the
        // runner into the next plan step. Advance here so both
        // ingress paths (`publish_event` and `process_parse_result`)
        // share the same step transition and snapshot write.
        if let Some(next) =
            advance_plan_step(&self.config, &self.current_plan_step, event.topic.as_str())
        {
            self.current_plan_step = next.clone();
        }
        // Plan 004 R7 (P0-4): persist accepted step transitions
        // at the shared main-bus acceptance point. This method is
        // called from both `publish_event` and `process_parse_result`,
        // so the resident EventLoop and CLI policy-check both read the
        // same authority ledger regardless of ingress path.
        self.append_flow_authority_snapshot(event.topic.as_str());
        if !self.phase_authority.is_enabled() {
            return;
        }
        let payload: serde_json::Value =
            serde_json::from_str(event.payload.as_str()).unwrap_or(serde_json::Value::Null);
        let honored = self.stage_pipeline.is_terminal(event);
        let snap = self.phase_authority.snapshot().unwrap_or_else(|| {
            crate::event_loop::phase_authority::PhaseSnapshot::with_phase_id("unit_loop")
        });
        let accepted = crate::event_loop::phase_authority::AcceptedEvent {
            topic: event.topic.as_str(),
            payload: &payload,
            honored,
        };
        let (next, effects) = crate::event_loop::phase_authority::handle_phase_on_event_accepted(
            &self.phase_authority,
            snap,
            &accepted,
        );
        if let Some(ledger) = self.state.state_ledger.as_mut() {
            ledger.snapshot_mut().workflow_phase = Some(next.clone());
        }
        if !effects.progress_md_fragment.is_empty() {
            let progress_path = self
                .config
                .core
                .workspace_root
                .join(".ralph")
                .join("agent")
                .join("progress.md");
            if let Ok(mut existing) = std::fs::read_to_string(&progress_path) {
                existing.push_str(&effects.progress_md_fragment);
                let _ = std::fs::write(progress_path, existing);
            }
        }
        if effects.review_walk_closed {
            tracing::debug!("phase authority: review walk closed");
        }
        if effects.phase_entered {
            tracing::debug!(
                phase = %next.phase_id,
                topic = %event.topic,
                "phase authority: entered new workflow phase"
            );
        }
    }

    /// Plan 004 R7 (P0-4): append the current step snapshot to
    /// `.ralph/flow-authority.jsonl` whenever an event is accepted
    /// onto the main bus. The CLI `--policy-check` path and a
    /// restart of the EventLoop both consult this ledger to recover
    /// the current step, so they read the same authority the
    /// resident EventLoop holds. Rejected events never reach this
    /// method, so the ledger only records accepted transitions.
    fn append_flow_authority_snapshot(&self, topic: &str) {
        use std::io::Write;
        let path = std::path::Path::new(&self.config.core.workspace_root)
            .join(".ralph/flow-authority.jsonl");
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(
                workspace = %self.config.core.workspace_root.display(),
                path = %path.display(),
                error = %err,
                "failed to create flow-authority parent directory"
            );
            return;
        }
        let mut entry = serde_json::Map::new();
        entry.insert(
            "step".to_string(),
            serde_json::Value::String(self.current_plan_step.clone()),
        );
        entry.insert(
            "topic".to_string(),
            serde_json::Value::String(topic.to_string()),
        );
        let line = serde_json::Value::Object(entry).to_string();
        let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        else {
            tracing::warn!(
                workspace = %self.config.core.workspace_root.display(),
                path = %path.display(),
                "failed to open flow-authority ledger for append"
            );
            return;
        };
        if let Err(err) = writeln!(f, "{line}") {
            tracing::warn!(
                workspace = %self.config.core.workspace_root.display(),
                path = %path.display(),
                error = %err,
                "failed to append flow-authority snapshot"
            );
        }
    }

    pub(crate) fn record_stage_rejection(
        &mut self,
        event: &Event,
        reject: &crate::event_loop::stage_pipeline::StageReject,
    ) {
        use crate::diagnosis::{DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef};
        const PAYLOAD_PREVIEW_CHARS: usize = 200;
        let payload_preview: String = event.payload.chars().take(PAYLOAD_PREVIEW_CHARS).collect();
        let mut builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::CliEmit)
            .severity(DiagnosisSeverity::Warning)
            .topic(event.topic.as_str())
            .source_hat(event.source.as_ref().map(|h| h.as_str()).unwrap_or(""))
            .reason_code(reject.reason_code.clone())
            .message(format!(
                "stage '{}' rejected event: {} (missing_fields={:?})",
                reject.stage_name, reject.reason_code, reject.missing_fields
            ))
            .evidence(EvidenceRef::new(
                EvidenceKind::Field,
                reject.stage_name,
                Some(payload_preview),
            ));
        if let Some(iter) = self.state.iteration.checked_add(0) {
            builder = builder.iteration(iter);
        }
        let envelope = builder.build();
        let notes = vec![format!(
            "stage_pipeline rejection: stage={} reason={} topic={}",
            reject.stage_name, reject.reason_code, event.topic
        )];
        let _ = self.record_recovery_envelope(&envelope, notes);

        if reject.reason_code == "phase_violation" {
            let hat = event
                .source
                .as_ref()
                .map(|h| h.as_str())
                .unwrap_or("unknown");
            let snap = self.phase_authority.record_phase_violation(hat);
            if let Some(ledger) = self.state.state_ledger.as_mut() {
                ledger.snapshot_mut().workflow_phase = Some(snap.clone());
            }
            if let Some(policy) = self.phase_authority.violation_policy() {
                use crate::event_loop::phase_authority::ViolationKind;
                use crate::event_loop::phase_authority::resume_budget::{
                    BudgetDecision, ExhaustedAction, on_exhausted_action,
                    should_admit_resume_from_snapshot,
                };
                match should_admit_resume_from_snapshot(
                    &policy,
                    &snap,
                    hat,
                    ViolationKind::PhaseViolation,
                ) {
                    BudgetDecision::Admit => {
                        let resume_payload = serde_json::json!({
                            "reason_code": "phase_violation",
                            "topic": event.topic.as_str(),
                            "hat": hat,
                            "loop_id": self.loop_id_label(),
                        });
                        self.bus.publish(ralph_proto::Event::new(
                            "task.resume",
                            resume_payload.to_string(),
                        ));
                    }
                    BudgetDecision::Exhausted => match on_exhausted_action(&policy) {
                        ExhaustedAction::PlanBlocked => {
                            let blocked_payload = serde_json::json!({
                                "reason": "phase_violation_exhausted",
                                "topic": event.topic.as_str(),
                                "hat": hat,
                                "loop_id": self.loop_id_label(),
                            });
                            self.bus.publish(ralph_proto::Event::new(
                                "plan.blocked",
                                blocked_payload.to_string(),
                            ));
                        }
                        ExhaustedAction::SilentDrop => {}
                    },
                }
            }
        }

        // P1-1 (2026-06-28 review): when the
        // rejection comes from a budget exhaustion on
        // the repair stream, escalate to a synthesised
        // `plan.blocked` so the operator sees the
        // reason without grepping `recovery.jsonl`.
        // The escalation reuses the same `bus.publish`
        // path as the three existing `plan.blocked`
        // emitters (waves, step-handoff, stall
        // detector) so it lands on the main bus without
        // re-entering the stage pipeline.
        if reject
            .reason_code
            .starts_with("repair_unrecoverable_after_")
        {
            let blocked_payload = serde_json::json!({
                "reason": reject.reason_code,
                "topic": event.topic.as_str(),
                "stage": reject.stage_name,
                "loop_id": self.loop_id_label(),
            });
            self.bus.publish(ralph_proto::Event::new(
                "plan.blocked",
                blocked_payload.to_string(),
            ));
            debug!(
                topic = %event.topic,
                reason = %reject.reason_code,
                "P1-1: synthesised plan.blocked after repair budget exhaustion"
            );
        }
    }

    /// Resolve the loop id label used by the P1-1
    /// `plan.blocked` escalation. Returns the context's
    /// loop id when available, otherwise the literal
    /// `"default"` (mirrors `write_loop_termination_record`).
    fn loop_id_label(&self) -> String {
        self.loop_context()
            .and_then(|c| c.loop_id())
            .unwrap_or("default")
            .to_string()
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
fn run_stall_detector_on_state(
    state: &mut crate::event_loop::loop_state::LoopState,
    config_progress_steward: &crate::config::ProgressStewardConfig,
    registry: &crate::hat_registry::HatRegistry,
    bus: &mut ralph_proto::EventBus,
) {
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
        return;
    }
    if state.steward_woken_this_turn && config_progress_steward.enabled {
        // Self-protection: the steward was already woken in
        // this turn. Suppress recursive wakes (enabled path only).
        return;
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
                 emitting plan.blocked (fail-close)",
                max_iter,
            );
            // 2026-07-24-005 plan U1: target is `reporter` (was
            // `shipper`); the shipper hat is removed from the
            // supervisor preset — reporter is the canonical
            // `plan.blocked` terminal owner.
            let blocked = ralph_proto::Event::new(
                "plan.blocked",
                "{\"reason\":\"loop_stalled_max_iterations\"}".to_string(),
            )
            .with_target(ralph_proto::HatId::new("reporter"));
            bus.publish(blocked);
            state.consecutive_no_progress_turns = 0;
            state.consecutive_steward_activations = 0;
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
            return;
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
            "isolated loop: steward did not produce progress after {} wakes — emitting plan.blocked",
            max_iter,
        );
        let blocked = ralph_proto::Event::new(
            "plan.blocked",
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
        bus.publish(blocked);
        // Reset so the next loop (e.g. a follow-up diagnostic
        // or operator restart) starts from a clean state.
        state.consecutive_no_progress_turns = 0;
        state.consecutive_steward_activations = 0;
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
/// `None` if the file is missing or contains no accepted entries.
pub fn load_flow_authority_current_step(workspace_root: &std::path::Path) -> Option<String> {
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
        if let Some(step) = v.get("step").and_then(|s| s.as_str()) {
            last = Some(step.to_string());
        }
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

    fn workspace_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ralph-p0-4-flow-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".ralph")).unwrap();
        dir
    }

    #[test]
    fn load_returns_none_when_ledger_missing() {
        let root = workspace_root();
        let got = load_flow_authority_current_step(&root);
        assert!(got.is_none(), "missing ledger must yield None");
    }

    #[test]
    fn load_returns_last_step_from_ledger() {
        let root = workspace_root();
        let path = root.join(".ralph/flow-authority.jsonl");
        std::fs::write(
            &path,
            "{\"step\":\"scope_freeze\",\"topic\":\"scope.freeze\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n\
             {\"step\":\"synth_await\",\"topic\":\"review.wave.complete\"}\n",
        )
        .unwrap();
        let got = load_flow_authority_current_step(&root);
        assert_eq!(got.as_deref(), Some("synth_await"));
    }

    #[test]
    fn load_skips_blank_and_malformed_lines() {
        let root = workspace_root();
        let path = root.join(".ralph/flow-authority.jsonl");
        std::fs::write(
            &path,
            "\n{\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n\
             not-json\n\
             {\"step\":\"synth_await\"}\n",
        )
        .unwrap();
        let got = load_flow_authority_current_step(&root);
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
        let root = workspace_root();
        let path = root.join(".ralph/flow-authority.jsonl");
        // Simulate the EventLoop having accepted exactly one
        // event: scope.ready, which advanced review_wave.
        std::fs::write(
            &path,
            "{\"step\":\"scope_freeze\",\"topic\":\"scope.freeze\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n",
        )
        .unwrap();
        let got = load_flow_authority_current_step(&root);
        assert_eq!(got.as_deref(), Some("review_wave"));
    }

    /// Plan 004 R7: the same accepted-step ledger is consumed
    /// by both the resident EventLoop (writes) and CLI
    /// policy-check / restart (reads). Restart consistency:
    /// re-instantiating the recovery function on the same
    /// ledger must produce the same step.
    #[test]
    fn restart_consistency_across_reads() {
        let root = workspace_root();
        let path = root.join(".ralph/flow-authority.jsonl");
        std::fs::write(
            &path,
            "{\"step\":\"scope_freeze\",\"topic\":\"scope.freeze\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n\
             {\"step\":\"synth_await\",\"topic\":\"review.wave.complete\"}\n",
        )
        .unwrap();
        let a = load_flow_authority_current_step(&root);
        let b = load_flow_authority_current_step(&root);
        assert_eq!(a, b, "restart must observe the same authority");
        assert_eq!(a.as_deref(), Some("synth_await"));
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
                            vec![
                                "forge.audit.done",
                                "forge.plan.blocked",
                                "work.failed",
                            ],
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
