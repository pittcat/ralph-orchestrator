//! Plan 2026-08-05-001 U10 split: types and free functions moved verbatim
//! from `event_loop/mod.rs` to keep the public API stable while shrinking
//! the root file. See the inline comments for plan / unit provenance.
//!
//! This module only owns prompt surface types (preview / gates / skill
//! entries / skill injector), the over-emit recovery bookkeeping structs,
//! the orphan `TerminationReason` impl, and the isolated-prompt single-
//! business-event gating helpers. The runtime data path stays in
//! `event_loop::mod` — these are the SSoT shapes that
//! `ralph inspect prompt` and the isolated-budget escape branch consume.

use crate::config::RalphConfig;
use crate::skill_registry::SkillRegistry;
use ralph_proto::HatId;

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
/// from scratchpads that pre-date 2026-06-28 so the bootstrap
/// path does not surface stale guidance text to a fresh
/// agent. New scratchpads will not contain the block (the
/// emit path is gone), so this helper only fires on history.
pub(super) fn strip_human_guidance_block(content: &str) -> String {
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
pub(super) fn is_isolated_exempt_topic(
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

use super::types::TerminationReason;
