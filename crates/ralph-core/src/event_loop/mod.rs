//! Event loop orchestration.
//!
//! The event loop coordinates the execution of hats via pub/sub messaging.

pub mod loop_state;
pub mod rejection;
pub mod review_step_state;
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
pub use policy::{build_unified_validation_pipeline, publish_correction_via_context};
// U3: re-export the type declarations that were moved from mod.rs to
// `types.rs`. The `pub use` preserves the existing public API path
// (`event_loop::TerminationReason`, etc.) so downstream consumers see
// no change. `WorkflowGuardRejection` stays module-private and is
// only `pub(super)` in `types.rs`.
pub use types::{EventLoop, ProcessedEvents, ProcessedEventsWithWaves, TerminationReason};
// 2026-06-26 plan U1: typed verdict SSOT — used by `verdict_payload_is_fail`
// and `check_completion_event` to share the same Pass / PassWithResiduals /
// Fail semantics as the shipper and reporter prompts.
pub use verdict::{Verdict, VerdictParseError};
// 2026-06-26 plan U1: completion-correction exhaust + structural-rejection
// sources, surfaced through `TerminationReason::CompletionStuck`.
pub use types::{CompletionStuck, StuckSource};

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

impl Default for ProcessedEvents {
    fn default() -> Self {
        Self {
            had_events: false,
            had_raw_events: false,
            had_rejected_events: false,
            had_plan_events: false,
            has_orphans: false,
            accepted_events: Vec::new(),
            contract_rejections: Vec::new(),
            payload_contract_violation: None,
        }
    }
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
            TerminationReason::RecoverablePayloadExhausted { .. } => 1,
            // 2026-06-26 plan U1: completion-rejection budget exhausted
            // (recoverable) OR structural rejection routed to a hard
            // stop. Both are non-zero exits — the operator must see
            // the loop end and consult `loop.terminate.last_reason`.
            TerminationReason::CompletionStuck(_) => 1,
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
/// 2026-06-24 plan U2: also appends `max_residuals` so the shipper
/// hat can read the verdict-promotion threshold without depending
/// on hat-side hardcoding.
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

/// Minimal FlowDeclaration YAML used as the fallback when
/// the preset has no mechanism: block.
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
    r#"mechanism:
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
"#
}

/// U6: build the default emit-time stage pipeline from the
/// loaded RalphConfig. Falls back to a minimal declared flow
/// when the preset has no mechanism: block.
///
/// P0-3 (2026-06-27 adversarial review): the
/// previous implementation round-tripped the entire
/// `RalphConfig` through `serde_yaml::to_string` and
/// fed it to `FlowDeclaration::from_yaml`, but
/// `RalphConfig` had no `mechanism:` field — the
/// parser therefore always saw a missing
/// `mechanism:` block and silently fell back to the
/// minimal flow declaration (empty `steps`),
/// rendering `FlowStepScopeStage` no-op for
/// operator-declared flows. We now read the typed
/// `config.event_loop.mechanism.flow` field
/// (added in P0-3) and serialise ONLY the
/// `mechanism:` subtree the parser expects. The
/// fallback to the minimal flow declaration
/// remains for presets that have not opted in.
fn build_stage_pipeline_from_config(
    config: &crate::config::RalphConfig,
) -> (
    crate::event_loop::stage_pipeline::StagePipeline,
    std::collections::HashMap<String, u32>,
) {
    use crate::event_loop::flow_declaration::FlowDeclaration;
    use crate::event_loop::stage_pipeline::StagePipeline;
    let flow_yaml = config
        .mechanism
        .as_ref()
        .and_then(|m| m.flow.as_ref())
        .or_else(|| {
            // Legacy `event_loop.mechanism` (P0-3
            // v1 placement) — accepted as a
            // backward-compat shim for presets
            // that nested the block under
            // `event_loop:`. New presets should
            // use the top-level `mechanism:`
            // key (mirroring the
            // `presets/schemas/<name>.yml` SSOT).
            config.event_loop.mechanism.as_ref().and_then(|m| m.flow.as_ref())
        })
        .and_then(|flow_cfg| {
            // Wrap the typed flow in the `mechanism:` block
            // the parser expects. `serde_yaml::to_string` on
            // the typed config produces the inner map;
            // we wrap it into the `mechanism.flow` key
            // pair the parser looks up.
            serde_yaml::to_string(flow_cfg).ok().map(|flow| {
                format!("mechanism:\n  flow:\n{flow}")
            })
        })
        .and_then(|yaml| FlowDeclaration::from_yaml(&yaml).ok())
        .unwrap_or_else(|| {
            FlowDeclaration::from_yaml(minimal_flow_declaration_yaml())
                .expect("minimal flow declaration YAML is always valid")
        });
    // U12 wiring (P0-1, 2026-06-27 review): mirror
    // `total_units` per step so `drive_step_close_progress`
    // can resolve the total without walking the
    // trait-object pipeline. Steps without `total_units`
    // are absent from the map → stage stays fail-open.
    let step_totals: std::collections::HashMap<String, u32> = flow_yaml
        .steps
        .iter()
        .filter_map(|s| s.total_units.map(|n| (s.id.clone(), n)))
        .collect();
    let pipeline = StagePipeline::with_default_stages_for_loop_config(flow_yaml, Some(&config.event_loop));
    (pipeline, step_totals)
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
                Ok(Some(dir)) => info!(
                    "U13: archived previous-loop state to {}",
                    dir.display()
                ),
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
                        format!(
                            "U13: archive_state_for_loop failed for loop_id={loop_id}: {e}"
                        ),
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
                        match IdempotentLog::open(
                            &context.workspace().join(".ralph"),
                            loop_id,
                        ) {
                            Ok(log) => {
                                let arc =
                                    std::sync::Arc::new(std::sync::Mutex::new(log));
                                if let Err(e) =
                                    store.save_with_shared_log(arc, loop_id)
                                {
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
                Err(e) => warn!(
                    "U8: relocate_legacy_tasks failed (continuing): {e}"
                ),
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

        let mut state = LoopState::new();
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
                        std::sync::Mutex::new(crate::state::idempotent_log::IdempotentLog::disabled())
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

        let (stage_pipeline, flow_step_totals) = build_stage_pipeline_from_config(&config);

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
            if let Ok(mut log) = idempotent_log.lock() {
                if let Err(e) = log.replay() {
                    warn!(
                        error = %e,
                        "U5: IdempotentLog::replay after bootstrap mirror failed; \
                         mirror records will be invisible to the main log until next save"
                    );
                }
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

        let (stage_pipeline, flow_step_totals) = build_stage_pipeline_from_config(&config);

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
            idempotent_log: std::sync::Mutex::new(crate::state::idempotent_log::IdempotentLog::disabled()),
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

    /// U2 (2026-06-18-004 plan, R2, KTD2): returns `true` when
    /// `human.guidance` injection MUST be suppressed for the
    /// current loop. Driven by the `event_loop.suppress_human_guidance`
    /// config flag — used by `ce-executor-serial` to prevent the
    /// perky-maple P1-2 probe storm. Mirrors the
    /// `coordinator_bootstrap_gate_closed` access pattern so the
    /// four guidance injection sites
    /// (`update_robot_guidance` / `apply_robot_guidance` /
    /// `collect_robot_guidance` / `prepend_scratchpad`) can each
    /// short-circuit through a single helper.
    pub fn human_guidance_suppressed(&self) -> bool {
        self.config.event_loop.suppress_human_guidance
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
    pub fn request_completion_from_text_fallback(&mut self) {
        if self.state.completion_honored {
            debug!("Completion already handled, ignoring text fallback request");
            return;
        }
        // P1-2: per-event commit so a mid-flight crash preserves
        // the completion signal for replay. The A1 end-of-batch
        // hook used to commit this; moving to the decision point
        // shrinks the window where a crash loses the signal.
        if !self.state.completion_requested {
            Self::commit_terminal_delta(
                &mut self.state.state_ledger,
                crate::state::CommitDelta::CompletionRequested,
            );
        }
        self.state.completion_requested = true;
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
            .map(|entry| entry.count as u32)
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
    /// Used to skip cooldown delays when a human guidance event is next, since
    /// we don't want to artificially delay the response to a human interaction.
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
    fn drive_repair_state_machine(
        &mut self,
        task_key: &str,
        stall_count: u32,
    ) -> bool {
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
                    exhausted.reason_code,
                    task_key,
                    exhausted.retries_consumed,
                    exhausted.max,
                );
                let blocked =
                    Event::new("plan.blocked", payload).with_target(HatId::new("ralph"));
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
                let blocked =
                    Event::new("plan.blocked", payload).with_target(HatId::new("ralph"));
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
        let budget_exhausted = self.drive_repair_state_machine(
            &stall_key,
            stall_count_value,
        );
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
                let with_skills = self.prepend_auto_inject_skills(base_prompt);
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
                            trigger_topic.to_string(),
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
                let with_skills = self.prepend_auto_inject_skills(base_prompt);
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

        // Non-ralph hat requested
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated {
            // Isolated mode: build focused prompt for this hat only.
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
                self.apply_robot_guidance(hat_id);
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
            // 2026-06-23 T2: append `## RUNTIME CONFIG` block so the hat
            // can read the runtime-resolved `max_fix_rounds` (which lives
            // under `event_loop:` in the YAML). The block is informational
            // and lives at the END of the hat prompt so the hat's own
            // workflow order (in `### GUARDRAILS`) stays authoritative.
            let base_prompt =
                append_runtime_config_block(base_prompt, self.config.event_loop.max_residuals);

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
            // P1-7 fix: orchestrator context is placed BEFORE
            // wave context so the prompt stack order is:
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
            let with_skills = self.prepend_auto_inject_skills(base_prompt);
            let with_scratchpad = self.prepend_scratchpad(with_skills, Some(hat_id));
            let with_state_files = self.prepend_state_files(with_scratchpad);
            let final_prompt = self.prepend_ready_tasks(with_state_files);
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
        let exempt_enabled = self
            .config
            .event_loop
            .progress_steward
            .exempt_from_suppress_human_guidance;

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
    fn apply_robot_guidance(&mut self, hat_id: &HatId) {
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
            let steward_hat_id = self
                .config
                .event_loop
                .progress_steward
                .steward_hat_id
                .as_str();
            let exempt = self
                .config
                .event_loop
                .progress_steward
                .exempt_from_suppress_human_guidance
                && hat_id.as_str() == steward_hat_id;
            if exempt {
                tracing::debug!(
                    target: "ralph::human_guidance",
                    hat_id = %hat_id.as_str(),
                    "U7: progress-steward exempt from suppress — pushing guidance to ralph"
                );
                self.ralph.set_robot_guidance(self.robot_guidance.clone());
                // 与非 suppress 路径一致:推入后清空本层 cache
                self.robot_guidance.clear();
                return;
            }
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
    fn prepend_auto_inject_skills(&self, prompt: String) -> String {
        let mut prefix = String::new();

        // 1. Memory data + ralph-tools skill — special case with data loading
        self.inject_memories_and_tools_skill(&mut prefix);

        // 2. Other auto-inject skills from the registry
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

    /// Injects any user-configured auto-inject skills (excluding built-in skills handled separately).
    fn inject_custom_auto_skills(&self, prefix: &mut String) {
        for skill in self.skill_registry.auto_inject_skills(None) {
            // Skip built-in skills handled above
            //
            // 2026-06-25 refactor: `robot-interaction` was removed because its
            // only content was `human.interact` / `human.guidance` Telegram
            // guidance; the `ralph-telegram` crate was deleted (see plan
            // 2026-06-25-001). No other Telegram-specific skills remain.
            if matches!(
                skill.name.as_str(),
                "ralph-tools" | "ralph-tools-tasks" | "ralph-tools-memories"
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
        let content = if gate_closed || suppress_active {
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
        snap.plan_baseline_sha = self.state.plan_baseline_sha.clone();
        format!("{}{prompt}", snap.to_prompt_block())
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
            let topic = event.topic.to_string();
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
                    .map(|e| e.count as u32)
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
            // P1-2: per-event commit (see `commit_terminal_delta`).
            if !self.state.completion_requested {
                Self::commit_terminal_delta(
                    &mut self.state.state_ledger,
                    crate::state::CommitDelta::CompletionRequested,
                );
            }
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
            let payload = serde_json::json!({
                "reason": "handoff_dispatch_timeout",
                "target_hat": esc.safe_target,
                "topic": esc.topic,
                "consumer": esc.consumer,
                "event_id": esc.event_id,
                "safe_target": esc.safe_target,
                "details": esc.reason,
            });
            let resume_event = Event::new("task.resume", payload.to_string())
                .with_source(HatId::from("ralph"))
                .with_target(HatId::from(esc.safe_target.as_str()));
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

                // 2026-06-23-005 U4 (R5+KTD-8): scope_violation is
                // promoted from Warn to Fail. Use the typed
                // AuditSeverity SSOT + AuditDispatcher so the
                // consecutive_failures increment goes through the
                // single audit dispatch path. `MissingField` is used
                // as the typed kind placeholder; scope_violation does
                // not yet have a dedicated RejectionKind variant — the
                // next plan can add `ScopeViolation` if drift_monitor
                // classification wants it. The consecutive_failures
                // increment is the contract that matters for the U4
                // kill-switch behaviour (KTD-4 + KTD-8).
                crate::event_loop::audit::AuditDispatcher::dispatch(
                    crate::event_loop::audit::AuditSeverity::Fail { add_failures: 1 },
                    crate::event_loop::audit::AuditContext {
                        hat: hat_id.as_str().to_string(),
                        kind: crate::preset::engine::gates::RejectionKind::MissingField,
                        details: diff_stat.clone(),
                    },
                    &mut self.state.consecutive_failures,
                );
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
        let unified_pipeline = build_unified_validation_pipeline(&self.config.event_loop);
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
            // 2026-06-16-001 U5: a turn with no events is the
            // canonical "no progress" turn. Run the stall
            // detector before returning so the loop does not
            // silently starve when the JSONL is empty.
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

                // R6/U2: ralph pseudo-hat may only publish control topics.
                // Business topics from ralph are rejected here (fail-closed)
                // so they do NOT count as progress toward the stall detector.
                // P1-12: use prefix match so future `ralph.*` topics are recognised.
                if event.hat.as_deref() == Some("ralph") {
                    if !crate::event_origin::is_ralph_control_topic(topic) {
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

                let is_dual_publish_step_handoff = event.topic.as_str() == "work.ready"
                    && accepted.last().is_some_and(|prev| {
                        prev.topic.as_str() == "queue.advance"
                            && prev.hat.as_ref() == event.hat.as_ref()
                    });

                let should_admit = if admitted_under_wave {
                    true
                } else if wave_collision {
                    false
                } else if !non_wave_business_event_accepted {
                    true
                } else {
                    is_dual_publish_step_handoff
                };

                if !should_admit {
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
                    accepted.push(event);
                    match event_wave_id.as_deref() {
                        Some(wid) => {
                            if accepted_wave_id.is_none() {
                                accepted_wave_id = Some(wid.to_string());
                            }
                        }
                        None => {
                            non_wave_business_event_accepted = true;
                        }
                    }
                    // U3 P0 fix: write the sticky per-turn budget flag so
                    // `check_default_publishes` (which runs later in the same
                    // turn when JSONL had zero events, or earlier when JSONL
                    // had business events) sees a consistent view.
                    self.state.isolated_turn_business_event_accepted = true;
                    // 2026-06-16-001 U5: mark the per-turn
                    // stall-detector flag so the post-validation
                    // stall detector resets the counters.
                    self.state.stall_detector_had_events = true;
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
            let projector = self.state.state_projection.get_or_insert_with(|| {
                let ctx = crate::state_projector::ProjectionContext::new(
                    self.config.core.workspace_root.as_path(),
                    self.config.event_loop.state_projection.clone(),
                    // Mirror the loop's R4 setting so the projector
                    // respects `enforce_current_unit` rather than
                    // silently disabling it. R1 in
                    // 2026-06-17-005 fix plan.
                    self.config.event_loop.enforce_current_unit,
                );
                let mut p = crate::state_projector::StateProjector::new(ctx);
                // Best-effort bootstrap; failure is non-fatal
                // because the projector falls back to live
                // disk reads on a cold cache.
                let _ = p.bootstrap_from_disk();
                p
            });
            let report = projector.apply(&events);
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
            if let Some(ref mut projector) = self.state.state_projection {
                if let Some(ref mut ledger) = self.state.state_ledger {
                    let mut guard = ledger.snapshot_mut();
                    projector.sync_to_ledger_snapshot(&mut guard);
                }
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
                let mut ctx = crate::validation::ValidationContext::new(&mut snapshot)
                    .with_policy_runtime_state(&mut policy_state)
                    .with_review_step_tracker(&mut review_step_tracker)
                    .with_workflow_progress(&mut workflow_progress)
                    .with_workflow_guard_details(&mut wg_details)
                    .with_payload_contract_violation(&mut event_policy_violation)
                    .with_policy_rejections(&mut policy_rejections)
                    .with_source_hats_by_topic(&source_hats_by_topic)
                    .with_target_hats_by_topic(&target_hats_by_topic);
                let results = pipeline.validate_pre_commit_with_view(&view, &mut ctx, evt);
                let mut event_accepted = true;
                let mut event_warnings: Vec<String> = Vec::new();
                for r in &results {
                    if r.accepted {
                        if r.stage == crate::validation::ValidationStage::EventPolicy
                            && r.reason_code.as_deref()
                                == Some(crate::validation::ReasonCode::EVENT_POLICY_WARNING)
                        {
                            if let Some(hint) = &r.correction_hint {
                                event_warnings.push(hint.clone());
                            }
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
                            Some(crate::validation::ReasonCode::EVENT_POLICY_BLOCKED)
                            | Some(crate::validation::ReasonCode::EVENT_POLICY_IGNORED) => {
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
                    let post_results = pipeline.validate_post_commit(&view, &mut ctx, evt);
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

                // WRC-U4: handoff tracking for accepted events whose topic has a
                // unique consumer in the HandoffIndex.
                self.update_bootstrap_flags_from_accepted(&events);
                for accepted in &events {
                    if let Some(consumer) = self.handoff_index.consumer_of(&accepted.topic) {
                        let event_id = format!("{}:{}", accepted.ts, accepted.topic);
                        self.state.handoff_tracker.on_handoff_accepted(
                            accepted.topic.clone(),
                            consumer.to_string(),
                            event_id.clone(),
                            std::time::Instant::now(),
                        );
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
                                        // 2026-06-24 P1-2: increment the
                                        // fix-round counter. When the hard
                                        // cap is reached, emit fix.exhausted
                                        // so the executor gets a rewrite
                                        // chance and the shipper can route
                                        // to terminal.
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
                                        if let Some(ref mut policy_state) =
                                            self.state.policy_runtime_state
                                        {
                                            policy_state
                                                .prune_review_dimension_ready_bucket(pn, st, ti);
                                            policy_state.prune_review_dimensions_complete_bucket(
                                                pn, st, ti,
                                            );
                                            policy_state.prune_work_done_bucket(pn, st);
                                            // 2026-06-24 P1-3: prune the new
                                            // `work.ready` / `test.passed` /
                                            // `test.failed` buckets so the
                                            // next round's emits land without
                                            // colliding with the prior round's
                                            // entries.
                                            policy_state.prune_work_ready_bucket(pn, st);
                                            policy_state.prune_test_result_buckets(pn, st, ti);
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }

                // Write hold artifact if policy hold was triggered.
                if let Some(ref reason) = hold_reason {
                    if let Err(e) = self.write_hold_artifact(Some(reason)) {
                        warn!(error = %e, "Failed to write hold artifact");
                    }
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
            for event in &events {
                if policy_config.terminal_topics.contains(&event.topic) {
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
            let workspace_root_owned =
                std::path::PathBuf::from(&self.config.core.workspace_root);
            let tasks_path_owned = self.tasks_path();
            let workspace_root = workspace_root_owned.as_path();
            let tasks_path = tasks_path_owned.as_path();

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
                    let guidance_topic_owned = rule.reject.guidance_topic.clone();
                    let diagnostic_topic_owned = rule.reject.diagnostic_topic.clone();
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
                                    // U2 (2026-06-17-003 plan): add the
                                    // schema-required `target_hat` field
                                    // alongside `reason` so the drift
                                    // detector counts the contract recovery
                                    // as schema-compliant.
                                    "target_hat": hat_id.as_str(),
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
                                Event::new(guidance_topic_owned.as_str(), guidance_payload);
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
        let policy_enabled_for_gate = policy_config_owned
            .as_ref()
            .map(|c| c.enabled)
            .unwrap_or(false);
        let write_diagnostic = policy_config_owned
            .as_ref()
            .map(|c| c.completion_after_terminal.write_diagnostic_event)
            .unwrap_or(false);
        let policy_config_ref = policy_config_owned.as_ref();
        let mut accepted_log_events = Vec::new();
        macro_rules! accept_event {
            ($accepted:expr) => {{
                let accepted = $accepted;
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
        let policy_enabled_for_gate = policy_config_ref
            .map(|c| c.enabled)
            .unwrap_or(false);
        let completion_after_terminal_for_gate = policy_config_ref
            .map(|c| c.completion_after_terminal.write_diagnostic_event)
            .unwrap_or(false);
        // `policy_config_ref` (an `Option<&EventPolicyConfig>`)
        // is held until after the U3 gate loop completes. The
        // gate loop needs `&mut self`, so the immutable
        // borrow on `self.config` must be released first.
        let _ = (policy_enabled_for_gate, completion_after_terminal_for_gate);
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
                // Completion event is accepted regardless of position in batch.
                // Events AFTER it in the same batch are protected by the completion guard.
                // P1-2: per-event commit (see `commit_terminal_delta`).
                if !self.state.completion_requested {
                    Self::commit_terminal_delta(
                        &mut self.state.state_ledger,
                        crate::state::CommitDelta::CompletionRequested,
                    );
                }
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
                if self.apply_emit_gate_on_validated(event, stashed) {
                    pending.push(event.clone());
                }
            }
            pending
        };
        for event in pending_publish {
            self.bus.publish(event);
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
            let batch_sync_source = if had_events || !accepted_log_events.is_empty() {
                "loop.batch_sync"
            } else {
                "loop.batch_sync.no_progress"
            };
            let iter_counter = CommitDelta::CounterChanged {
                counter: CounterKind::Iteration,
                new_value: self.state.iteration as i64,
            };
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
        let prefix = format!("ce-executor:");
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
        if count == 0 {
            None
        } else {
            Some(count)
        }
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
                let mut ctx = ValidationContext::new(&mut snapshot)
                    .with_policy_runtime_state(&mut policy_state)
                    .with_review_step_tracker(&mut review_step_tracker)
                    .with_payload_contract_violation(&mut wave_violation)
                    .with_policy_rejections(&mut wave_rejections);
                let r = rule.validate(&view, &mut ctx, evt);
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
                    Some(crate::validation::ReasonCode::EVENT_POLICY_BLOCKED)
                    | Some(crate::validation::ReasonCode::EVENT_POLICY_IGNORED) => {}
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
                        );
                    }
                }
            }

            self.state.state_ledger = state_ledger;
            self.state.review_step_tracker = review_step_tracker;
            self.state.policy_runtime_state = Some(policy_state);

            wave_policy_rejections = wave_rejections;

            // Write hold artifact if policy hold was triggered.
            if let Some(ref reason) = hold_reason {
                if let Err(e) = self.write_hold_artifact(Some(reason)) {
                    warn!(error = %e, "Failed to write hold artifact");
                }
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
        if let Some(ref hat) = event.source {
            if hat.as_str() == "ralph" {
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
        let outcome = crate::event_loop::emit_gate::evaluate_emit_gate(
            &mut stage_ctx, &event,
        );
        match outcome {
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptMainBus => {
                // 2026-06-28 plan U4: a successful accept may
                // carry the runner into the next plan step.
                // We advance AFTER publish so the stage_ctx
                // that just succeeded does not see its own
                // topic through the new step's scope.
                if let Some(next) = advance_plan_step(
                    &self.config,
                    &self.current_plan_step,
                    event.topic.as_str(),
                ) {
                    self.current_plan_step = next;
                }
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
                self.bus.publish(event);
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
        let step_id = if !self.current_plan_step.is_empty() {
            self.current_plan_step.clone()
        } else {
            self.state.flow_lifecycle.current_step_id().to_string()
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
        // ralph_proto::Event the facade expects. We
        // discard `hat`/`source` metadata that the gate
        // does not need; the source attribution lands in
        // the recovery envelope via `record_stage_rejection`.
        let payload = event.payload.clone().unwrap_or_default();
        let proto = Event::new(event.topic.as_str(), payload.as_str());
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
    fn record_repair_event(&mut self, event: &ralph_proto::Event) {
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
    pub(crate) fn record_stage_rejection(
        &mut self,
        event: &Event,
        reject: &crate::event_loop::stage_pipeline::StageReject,
    ) {
        use crate::diagnosis::{DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef};
        const PAYLOAD_PREVIEW_CHARS: usize = 200;
        let payload_preview: String = event
            .payload
            .chars()
            .take(PAYLOAD_PREVIEW_CHARS)
            .collect();
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
        if reject.reason_code.starts_with("repair_unrecoverable_after_") {
            let blocked_payload = serde_json::json!({
                "reason": reject.reason_code,
                "topic": event.topic.as_str(),
                "stage": reject.stage_name,
                "loop_id": self.loop_id_label(),
            });
            self.bus
                .publish(ralph_proto::Event::new("plan.blocked", blocked_payload.to_string()));
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
    if !config_progress_steward.enabled {
        return;
    }
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
    if state.steward_woken_this_turn {
        // Self-protection: the steward was already woken in
        // this turn. Suppress recursive wakes.
        return;
    }
    state.consecutive_no_progress_turns = state.consecutive_no_progress_turns.saturating_add(1);
    let max_iter = config_progress_steward.max_steward_iterations;
    if state.consecutive_no_progress_turns >= max_iter
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
        // loop to route through shipper → reporter for a
        // clean termination.
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
        // `with_target(shipper)` so the route matches the R5
        // hard-gate hat-routing convention. Without a
        // target, the bus delivers the event to the
        // default-routed hats; with the target, the
        // shipper is the canonical consumer and the
        // event reaches the shipper → reporter termination
        // path consistently. Loopback to progress-steward
        // is unnecessary: the steward was the one that
        // failed to make progress, so the recovery action
        // is to terminate, not retry.
        .with_target(ralph_proto::HatId::new("shipper"));
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

fn initial_current_plan_step(config: &RalphConfig) -> String {
    config
        .event_loop
        .mechanism
        .as_ref()
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
    let flow = config.event_loop.mechanism.as_ref()?.flow.as_ref()?;
    let steps = &flow.steps;
    let idx = steps.iter().position(|s| s.id == current)?;
    let step = &steps[idx];
    // Per-unit "still working" emits that must not advance the
    // step. The list is intentionally small: only `work.done`
    // is the standard completion sentinel for the unit_loop
    // pattern. A plan that wants different semantics can use
    // the `terminal_when` field to refine; for now the simple
    // rule is enough.
    const NON_TRANSITION_TOPICS: &[&str] = &["work.done", "work.failed", "work.ready"];
    if NON_TRANSITION_TOPICS.contains(&accepted_topic) {
        return None;
    }
    if !step.allowed_emits.iter().any(|t| t == accepted_topic) {
        return None;
    }
    steps.get(idx + 1).map(|s| s.id.clone())
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
            }),
            ..EventLoopConfig::default()
        };
        cfg
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
}
