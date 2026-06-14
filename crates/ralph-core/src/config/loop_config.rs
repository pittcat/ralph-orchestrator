//! Event loop configuration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::event_policy::EventPolicyConfig;
use super::execution_contracts::ExecutionContractsConfig;
use super::state_machine::StateMachineConfig;
use super::workflow_contract::WorkflowContractConfig;
use super::workflow_guards::{HatExecutionMode, WorkflowGuardsConfig};

/// Schema for validating events of a specific topic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSchema {
    /// Expected payload type.
    #[serde(default)]
    pub payload: Option<PayloadType>,
    /// Required fields in the JSON object payload.
    #[serde(default)]
    pub required_fields: Vec<String>,
    /// Allowed values for specific fields (dot-notation path -> allowed values).
    #[serde(default)]
    pub allowed_values: HashMap<String, Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PayloadType {
    JsonObject,
    String,
    Number,
    Bool,
    Array,
}

pub(super) fn default_prompt_file() -> String {
    "PROMPT.md".to_string()
}

fn default_completion_promise() -> String {
    "LOOP_COMPLETE".to_string()
}

fn default_max_iterations() -> u32 {
    100
}

fn default_max_runtime() -> u64 {
    14400 // 4 hours
}

fn default_max_wave_total() -> u32 {
    64
}

fn default_max_failures() -> u32 {
    5
}

fn default_cancellation_promise() -> String {
    "loop.cancel".to_string()
}

/// Event loop configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLoopConfig {
    /// Inline prompt text (mutually exclusive with prompt_file).
    pub prompt: Option<String>,

    /// Path to the prompt file.
    #[serde(default = "default_prompt_file")]
    pub prompt_file: String,

    /// Event topic that signals loop completion (must be emitted via `ralph emit`).
    #[serde(default = "default_completion_promise")]
    pub completion_promise: String,

    /// Maximum number of iterations before timeout.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,

    /// Maximum runtime in seconds.
    #[serde(default = "default_max_runtime")]
    pub max_runtime_seconds: u64,

    /// U2: cap on `wave_total` (the protocol-claimed fan-out size of a wave).
    /// Waves whose `wave_total` exceeds this value are rejected before any
    /// worker, TUI update, or backend call. Default: 64. The cap is an
    /// event-loop level setting (not a hat-level field) and is the
    /// primary defense against runaway fan-out (e.g. the 335-worker bug
    /// documented in `docs/report/ce-debug-report-2026-06-10-wave-335-fanout.md`).
    #[serde(default = "default_max_wave_total")]
    pub max_wave_total: u32,

    /// Maximum cost in USD before stopping.
    pub max_cost_usd: Option<f64>,

    /// Stop after this many consecutive failures.
    #[serde(default = "default_max_failures")]
    pub max_consecutive_failures: u32,

    /// Delay in seconds before starting the next iteration.
    /// Skipped when the next iteration is triggered by a human event.
    #[serde(default)]
    pub cooldown_delay_seconds: u64,

    /// Starting hat for multi-hat mode (deprecated, use starting_event instead).
    pub starting_hat: Option<String>,

    /// Event to publish after Ralph completes initial coordination.
    ///
    /// When custom hats are defined, Ralph handles `task.start` to do gap analysis
    /// and planning, then publishes this event to delegate to the first hat.
    ///
    /// Example: `starting_event: "tdd.start"` for TDD workflow.
    ///
    /// If not specified and hats are defined, Ralph will determine the appropriate
    /// event from the hat topology.
    pub starting_event: Option<String>,

    /// Warn when mutation testing score drops below this percentage (0-100).
    ///
    /// Warning-only: build.done is still accepted even if below threshold.
    #[serde(default)]
    pub mutation_score_warn_threshold: Option<f64>,

    /// When true, LOOP_COMPLETE does not terminate the loop.
    ///
    /// Instead of exiting, the loop injects a `task.resume` event and continues
    /// idling until new work arrives (human guidance, Telegram commands, etc.).
    /// The loop will only terminate on hard limits (max_iterations, max_runtime,
    /// max_cost), consecutive failures, or explicit interrupt/stop.
    #[serde(default)]
    pub persistent: bool,

    /// Event topics that must have been seen before LOOP_COMPLETE is accepted.
    /// If any required event has not been seen during the loop's lifetime,
    /// completion is rejected and a task.resume event is injected.
    #[serde(default)]
    pub required_events: Vec<String>,

    /// Event topic that triggers graceful early termination WITHOUT chain validation.
    /// Use this for human rejection, timeout escalation, or other abort paths.
    /// Defaults to "loop.cancel" (enabled). Set to "" (empty string) to disable.
    #[serde(default = "default_cancellation_promise")]
    pub cancellation_promise: String,

    /// When true, events emitted by a hat are validated against its declared
    /// `publishes` list. Out-of-scope events are dropped and replaced with
    /// `{hat_id}.scope_violation` diagnostic events. Defaults to false (permissive).
    #[serde(default)]
    pub enforce_hat_scope: bool,

    /// Opt-in workflow state guards for enforcing ordered event chains.
    ///
    /// When configured, events must follow the declared topic sequence before
    /// being published to the event bus. This prevents out-of-order events such
    /// as `experiment.evaluated` before `experiment.scored` from reaching
    /// downstream hats.
    #[serde(default)]
    pub workflow_guards: Option<WorkflowGuardsConfig>,

    /// Hat execution mode.
    ///
    /// Controls whether Ralph runs as a central coordinator (default) or
    /// dispatches each hat in an isolated backend process.
    #[serde(default)]
    pub execution_mode: HatExecutionMode,

    /// Opt-in event policy for typed payload validation.
    #[serde(default)]
    pub event_policy: Option<EventPolicyConfig>,

    /// Opt-in state machine for instance lifecycle validation.
    #[serde(default)]
    pub state_machine: Option<StateMachineConfig>,

    /// Phase configuration for two-phase loop (warmup + production).
    #[serde(default)]
    pub phase_config: Option<PhaseConfig>,

    /// Opt-in verdict gate: rejects LOOP_COMPLETE when the most recent event
    /// matching `topic` carries `fail_field == fail_value` in its payload.
    ///
    /// Use to enforce that a final-review verdict event (e.g. `REVIEW_COMPLETE`
    /// / `review.complete` published by a shipper/reporter hat) must indicate
    /// success before the loop can terminate. When `None` (default), no verdict
    /// check is performed — preserves backward compatibility.
    #[serde(default)]
    pub verdict_gate: Option<VerdictGateConfig>,

    /// Opt-in execution contracts for validating agent completion obligations.
    ///
    /// When configured, Ralph validates that `work.done` events carry the required
    /// payload fields, the referenced task is closed, and git state is consistent
    /// before the event can trigger downstream hats.
    #[serde(default)]
    pub execution_contracts: Option<ExecutionContractsConfig>,

    /// WAC-U3 (2026-06-12-002): Workflow Activation Contract
    /// runtime configuration. Optional; when absent the defaults
    /// (30s dispatch timeout, R7 seed handoff topics) apply.
    #[serde(default)]
    pub workflow_contract: Option<WorkflowContractConfig>,

    /// R3 (2026-06-14-003 plan): enable the ephemeral file isolation
    /// engine.  When `true` and `execution_mode == isolated`, the
    /// runtime scans the workspace for `scratchpad.md` /
    /// `tmp*.md` / `*.bak` artefacts that landed in source trees
    /// (`crates/`, `src/`, `backend/`, etc.) and relocates them to
    /// `.ralph/agent/scratchpad-{loop_id}.md`.  Defaults to `false`
    /// so non-isolated presets are unaffected; the
    /// `ce-executor-isolated` preset opts in.
    #[serde(default)]
    pub ephemeral_isolation: bool,

    /// R4 (2026-06-14-003 plan): enable the per-step single-U
    /// coordinator task contract.  When `true` and
    /// `execution_mode == isolated`, `TaskStore::ensure` rejects
    /// keys that mix units within the same `(loop_id, plan_name,
    /// step)`.  The contract is enforced only for keys whose last
    /// slug matches the `uN-` / `uNa-` shape; legacy or
    /// non-conforming keys fall through to the legacy behaviour.
    /// Defaults to `false`; `ce-executor-isolated` opts in.
    #[serde(default)]
    pub enforce_current_unit: bool,
}

impl Default for EventLoopConfig {
    fn default() -> Self {
        Self {
            prompt: None,
            prompt_file: default_prompt_file(),
            completion_promise: default_completion_promise(),
            max_iterations: default_max_iterations(),
            max_runtime_seconds: default_max_runtime(),
            max_wave_total: default_max_wave_total(),
            max_cost_usd: None,
            max_consecutive_failures: default_max_failures(),
            cooldown_delay_seconds: 0,
            starting_hat: None,
            starting_event: None,
            mutation_score_warn_threshold: None,
            persistent: false,
            required_events: Vec::new(),
            cancellation_promise: default_cancellation_promise(),
            enforce_hat_scope: false,
            workflow_guards: None,
            execution_mode: HatExecutionMode::default(),
            event_policy: None,
            state_machine: None,
            phase_config: None,
            verdict_gate: None,
            execution_contracts: None,
            workflow_contract: None,
            ephemeral_isolation: false,
            enforce_current_unit: false,
        }
    }
}

/// Verdict gate: when the most recent event matching any of
/// `topic` (or `additional_topics`) carries `fail_field == fail_value`
/// in its payload, LOOP_COMPLETE is rejected.
///
/// 2026-06-09 fix: added `additional_topics` so the gate can
/// cover the case where the verdict payload is mirrored onto
/// multiple topics — e.g. the ce-executor preset records
/// `pass_or_fail` on the upstream `REVIEW_COMPLETE` *and* on the
/// `report.done` summary event.  When `report.done` carries
/// `pass_or_fail: "fail"`, the gate must fire and reject any
/// follow-up `LOOP_COMPLETE` (closing the "rogue LOOP_COMPLETE
/// masks a failing review" gap documented in the 2026-06-09
/// ce-executor mechanism-vs-orchestration diagnosis).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerdictGateConfig {
    /// Event topic carrying the verdict payload (e.g. "review.complete").
    pub topic: String,

    /// JSON field name within the event payload (e.g. "pass_or_fail").
    pub fail_field: String,

    /// Value that triggers rejection of LOOP_COMPLETE (e.g. "fail").
    pub fail_value: String,

    /// 2026-06-09: additional topics that should also feed the
    /// verdict gate.  When ANY of `topic` or `additional_topics`
    /// receives an event whose payload has `fail_field == fail_value`,
    /// the gate fires.  Empty (the default) preserves the legacy
    /// single-topic behavior, so existing presets keep working
    /// unchanged.
    #[serde(default)]
    pub additional_topics: Vec<String>,
}

/// Orchestration phase enum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Warmup/calibration phase for harness tuning.
    #[default]
    Warmup,
    /// Production phase for正式 experiments.
    Production,
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Phase::Warmup => write!(f, "warmup"),
            Phase::Production => write!(f, "production"),
        }
    }
}

/// Phase configuration for two-phase orchestration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseConfig {
    /// Initial phase when loop starts.
    pub initial: Phase,

    /// Event topic that triggers phase transition.
    #[serde(default = "default_transition_event")]
    pub transition_event: String,

    /// Warmup-specific configuration for two-phase loops.
    #[serde(default)]
    pub warmup_config: Option<WarmupConfig>,
}

fn default_transition_event() -> String {
    "phase.transition".to_string()
}

/// Warmup configuration for two-phase loops.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupConfig {
    /// Minimum iterations before warmup can exit.
    #[serde(default)]
    pub min_iterations: u32,

    /// Maximum iterations before forcing transition.
    #[serde(default)]
    pub max_iterations: u32,

    /// Number of quiet rounds (no new findings) before exiting warmup.
    #[serde(default)]
    pub exit_quiet_rounds: u32,

    /// If true, loop stops after warmup completes (instead of transitioning to production).
    #[serde(default)]
    pub stop_on_exit: bool,
}

impl Default for WarmupConfig {
    fn default() -> Self {
        Self {
            min_iterations: 10,
            max_iterations: 30,
            exit_quiet_rounds: 3,
            stop_on_exit: false,
        }
    }
}
