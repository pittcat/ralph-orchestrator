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
mod acceptance_and_lifecycle;
pub mod audit;
#[cfg(test)]
// 2026-07-02-006 plan U15: build_stage_pipeline_from_config
// branch tests. Sibling to `tests` so the wiring change is
// visible without scanning the entire mod.rs.
mod build_stage_pipeline_phase_branch_tests;
mod completion_and_termination;
mod dispatch_and_handoff;
mod event_processing;
mod flow_authority;
mod flow_wiring;
mod prompt_types;
pub use flow_wiring::*;
pub use prompt_types::*;
mod parse_and_emit;
mod prompt_injection;
/// Plan 2026-08-10-001 U2: unified `task.resume` target resolver
/// and publisher boundary.
pub mod resume_routing;
mod state_recovery;
mod terminal_routing;
#[cfg(test)]
mod tests;
mod wave_scope;

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
/// undeclared flows fall back to `plan.blocked`). It returns `true`
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
            // Route fail-close to reporter only when the preset explicitly
            // subscribes reporter to this blocked topic. Presets such as
            // merge-batch require stabilization before reporting; an
            // unconditional target would bypass that topology.
            //
            // 2026-07-30-002 plan U1: topic is the preset's
            // derived blocked namespace (e.g. `forge.plan.blocked`
            // for parallel-forge), so the reporter's terminal
            // emit clears FlowStepScope. See `derive_blocked_topic`.
            let blocked = ralph_proto::Event::new(
                blocked_topic,
                "{\"reason\":\"loop_stalled_max_iterations\"}".to_string(),
            );
            let blocked = if registry
                .get_config(&ralph_proto::HatId::new("reporter"))
                .is_some_and(|config| config.triggers.iter().any(|t| t == blocked.topic.as_str()))
            {
                blocked.with_target(ralph_proto::HatId::new("reporter"))
            } else {
                blocked
            };
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
        );
        let blocked = if registry
            .get_config(&ralph_proto::HatId::new("reporter"))
            .is_some_and(|config| config.triggers.iter().any(|t| t == blocked.topic.as_str()))
        {
            blocked.with_target(ralph_proto::HatId::new("reporter"))
        } else {
            blocked
        };
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

// 2026-07-28-001 plan U2: typed exec_wave branch tests.
// Separate file keeps wave transition / non-transition coverage
// isolated from the longer flow_authority_pf_recovery_tests block.
#[cfg(test)]
pub mod wave_branch_tests;
