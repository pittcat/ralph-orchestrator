//! Event loop configuration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::event_policy::EventPolicyConfig;
use super::execution_contracts::ExecutionContractsConfig;
use super::state_machine::StateMachineConfig;
use super::state_projection::StateProjectionConfig;
use super::workflow_contract::WorkflowContractConfig;
use super::workflow_guards::{HatExecutionMode, WorkflowGuardsConfig};

/// Hat-specific allowed values for a field within an event schema.
///
/// When a field has hat-aware restrictions, only the hats listed here may
/// use the associated values. Values that are legal for one hat but illegal
/// for another are rejected at CLI emit time instead of killing the loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HatAllowedValues {
    pub hat_id: String,
    pub values: Vec<serde_json::Value>,
}

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
    /// These apply regardless of which hat emits the event.
    #[serde(default)]
    pub allowed_values: HashMap<String, Vec<serde_json::Value>>,
    /// Hat-aware allowed values. Keys are dot-notation field paths; values are
    /// per-hat allowed-value lists. When the emitting hat matches a rule, the
    /// field value must be in that rule's list. This lets the policy express
    /// e.g. "review-coordinator may only use skip_reason='empty_diff', while
    /// review-synthesizer may use 'aggregate_timeout'".
    #[serde(default)]
    pub hat_allowed_values: HashMap<String, Vec<HatAllowedValues>>,
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

/// 2026-06-24 plan U2: threshold below which
/// `review.complete.verdict == "pass_with_residuals"` is upgraded
/// to `pass` by the shipper when translating to `REVIEW_COMPLETE`.
/// Default: 8 (matches the ralph-e2e `primary-20260624-032505`
/// case where 8 residual findings, 2 P0 + 6 P1 + 1 P2, was
/// structurally pass-with-residuals rather than fail). Operators
/// can lower it (tighter) or raise it (more lenient) per
/// workspace; presets can declare a different value.
fn default_max_residuals() -> u32 {
    8
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
    /// idling until new work arrives (human guidance, recovery commands, etc.).
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
    /// `ce-executor-serial` preset opts in.
    #[serde(default)]
    pub ephemeral_isolation: bool,

    /// R4 (2026-06-14-003 plan): enable the per-step single-U
    /// coordinator task contract.  When `true` and
    /// `execution_mode == isolated`, `TaskStore::ensure` rejects
    /// keys that mix units within the same `(loop_id, plan_name,
    /// step)`.  The contract is enforced only for keys whose last
    /// slug matches the `uN-` / `uNa-` shape; legacy or
    /// non-conforming keys fall through to the legacy behaviour.
    /// Defaults to `false`; `ce-executor-serial` opts in.
    #[serde(default)]
    pub enforce_current_unit: bool,

    /// P0-3 (2026-06-27 adversarial review): mechanism
    /// foundation (U5) — declared flow declaration. The
    /// `EventLoop::build_stage_pipeline_from_config`
    /// constructor (U6) reads this field to build the
    /// `FlowDeclaration` that `FlowStepScopeStage`
    /// (U9) and the lint rule both consume. Mirrors
    /// the `mechanism:` block at the bottom of
    /// `presets/en/<name>.yml`; the
    /// `presets/schemas/<name>.yml` SSOT is the
    /// authoritative schema. The legacy
    /// `serde_yaml::to_string(&RalphConfig)` path
    /// could not surface this field because
    /// `RalphConfig` had no `mechanism:` block —
    /// the parser therefore always fell back to the
    /// minimal-flow YAML, which made
    /// `FlowStepScopeStage` accept anything (the
    /// 2026-06-27 review flagged this as a P0).
    #[serde(default)]
    pub mechanism: Option<MechanismConfig>,

    /// 2026-06-16-001 U5: progress-steward fallback configuration.
    /// When the loop detects that no accepted business event has
    /// advanced for `max_steward_iterations` consecutive turns, it
    /// wakes the `steward_hat_id` hat to summarise the state and
    /// emit a single recovery event. The steward is itself exempt
    /// from re-routing (a steward emit that fails the origin guard
    /// will not recursively re-trigger the steward).
    ///
    /// Defaults are conservative: enabled with `progress-steward`
    /// as the target and 3 iterations as the stall threshold. Set
    /// `enabled: false` to disable the steward entirely.
    #[serde(default)]
    pub progress_steward: ProgressStewardConfig,

    /// 2026-06-16-001 U3: TTL for `task.resume` injection. Rejections
    /// whose source event is older than this TTL are silently
    /// dropped — the rejection would otherwise re-activate a task
    /// that has since been closed or whose context has drifted past
    /// the recovery window. The default is 300s; operators can
    /// override per-preset or in `ralph.yml`. A value of `0`
    /// disables the freshness filter (always admit). U5 also reads
    /// this TTL when routing rejections to the `progress-steward`
    /// hat (the steward itself is exempt from re-routing into
    /// itself).
    #[serde(default)]
    pub task_resume_ttl_seconds: Option<u64>,

    /// State projection (Phase 1 of the north-star plan). When
    /// `enabled` is `true`, the event loop projects the canonical
    /// `.ralph/agent/tasks.jsonl` and `.ralph/agent/progress.md`
    /// ledgers from the inbound event batch **before** the
    /// `progress_task_gate` runs (SP-R8). Defaults to `disabled` so
    /// legacy presets are unaffected; `ce-executor-serial` and
    /// `ce-executor-serial` opt in via
    /// `event_loop.state_projection.enabled: true` (SP-R18).
    #[serde(default)]
    pub state_projection: StateProjectionConfig,

    /// 2026-06-24 plan U2: residual-finding threshold for verdict
    /// promotion. When the shipper hat translates
    /// `plan.complete.verdict == "pass_with_residuals"` to
    /// `REVIEW_COMPLETE`, it reads `final_findings_count` (or
    /// `residual_findings_count`) and promotes the verdict to
    /// `pass` if the count is at or below this threshold. Above
    /// the threshold, the original `pass_with_residuals` verdict
    /// (and `pass_or_fail: fail`) is preserved so the manager
    /// intervenes. Default: 8 (see `default_max_residuals`).
    #[serde(default = "default_max_residuals")]
    pub max_residuals: u32,
}

/// 2026-06-16-001 U5: per-preset configuration for the loop-level
/// `progress-steward` fallback hat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressStewardConfig {
    /// Master switch. When false, the loop never auto-wakes the
    /// steward; the human must intervene manually.
    #[serde(default = "default_progress_steward_enabled")]
    pub enabled: bool,

    /// The hat id to wake when a stall is detected. The hat must
    /// exist in the preset's `hats:` mapping; otherwise the
    /// runtime logs a warning and skips the wake.
    #[serde(default = "default_progress_steward_hat_id")]
    pub steward_hat_id: String,

    /// Number of consecutive turns with no accepted business event
    /// before the loop auto-emits `loop.stalled` and wakes the
    /// steward. After this many consecutive steward activations
    /// without a forwarded business event, the loop emits
    /// `plan.blocked(reason=loop_stalled_max_iterations)` and
    /// terminates cleanly through shipper → reporter.
    #[serde(default = "default_progress_steward_max_iterations")]
    pub max_steward_iterations: u32,
}

fn default_progress_steward_enabled() -> bool {
    false
}

fn default_progress_steward_hat_id() -> String {
    "progress-steward".to_string()
}

fn default_progress_steward_max_iterations() -> u32 {
    3
}

impl Default for ProgressStewardConfig {
    fn default() -> Self {
        Self {
            enabled: default_progress_steward_enabled(),
            steward_hat_id: default_progress_steward_hat_id(),
            max_steward_iterations: default_progress_steward_max_iterations(),
        }
    }
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
            // 2026-06-16-001 U3: 300s default TTL for `task.resume`
            // freshness. Rejections older than this are dropped to
            // prevent stale recovery signals from re-activating a
            // task that has since been closed. Operators can
            // override per-preset or in `ralph.yml`.
            task_resume_ttl_seconds: Some(300),
            // 2026-06-16-001 U5: default progress-steward
            // configuration. Enabled, target = `progress-steward`,
            // 3 iterations before auto-emit of `plan.blocked`.
            progress_steward: ProgressStewardConfig::default(),
            // 2026-06-17-003 U1: state projection opt-in. Disabled
            // by default; presets opt in via YAML.
            state_projection: StateProjectionConfig::default(),
            // 2026-06-24 plan U2: max_residuals default 8.
            // Presets (e.g. ce-executor-serial → 8) and operators
            // may override via YAML.
            max_residuals: default_max_residuals(),
            // P0-3 (2026-06-27 adversarial review): the
            // mechanism foundation opt-in. None by
            // default so the runtime falls back to the
            // minimal flow declaration (see
            // `event_loop::mod::minimal_flow_declaration_yaml`).
            // Presets that declare a `mechanism:` block
            // override this via YAML.
            mechanism: None,
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

    /// 2026-06-26 plan U5: optional override of the typed
    /// `Verdict::from_payload` field name. When `None`, the gate
    /// keeps the legacy binary match (only `fail_field ==
    /// fail_value` trips the gate). When `Some(name)`, the gate
    /// parses the payload as a typed `Verdict` and applies
    /// [`Verdict::resolve`] with `max_residuals` to decide pass vs
    /// fail — `pass_with_residuals` becomes a real intermediate
    /// state instead of an alias for fail.
    ///
    /// The default in newer presets is `"verdict"`; older presets
    /// that still use `pass_or_fail` keep `None` and the binary
    /// match is preserved.
    #[serde(default)]
    pub verdict_field: Option<String>,

    /// 2026-06-26 plan U5: optional override of the residual-count
    /// field name read by `Verdict::from_payload` when the verdict
    /// is `pass_with_residuals`. The default is
    /// `"final_findings_count"`. Ignored when `verdict_field` is
    /// `None` (legacy binary match path).
    #[serde(default)]
    pub residual_count_field: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u7_progress_steward_config_serializes_with_exempt_field() {
        let yaml = r#"
enabled: true
steward_hat_id: "progress-steward"
max_steward_iterations: 3
"#;
        let cfg: ProgressStewardConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.max_steward_iterations, 3);
    }

    // 2026-06-23: T1 — `max_fix_rounds` field removed in 2026-06-24
    // (fixer hardcodes max 10 in instructions; the config field was
    // never enforced by Rust code and contradicted the instructions).
}

/// P0-3 (2026-06-27 adversarial review): typed
/// mirror of the `mechanism:` block at the bottom
/// of `presets/en/<name>.yml`. The runtime reads
/// this field directly instead of round-tripping
/// `RalphConfig` through YAML (which silently
/// dropped the block because the previous
/// `RalphConfig` had no `mechanism:` field). The
/// field is optional — presets that have not opted
/// into the mechanism foundation fall back to the
/// minimal `FlowDeclaration` (see
/// `event_loop::mod::minimal_flow_declaration_yaml`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MechanismConfig {
    /// The flow declaration. Mirrors
    /// `presets/schemas/<name>.yml`'s `mechanism.flow`
    /// block. When `None`, the runtime uses the
    /// minimal flow declaration so legacy presets
    /// continue to function without changes.
    pub flow: Option<FlowDeclarationConfig>,
}

/// P0-3: typed view of a `mechanism.flow`
/// declaration. Mirrors the SSOT in
/// `presets/schemas/<name>.yml` and the typed
/// `FlowDeclaration` in
/// `event_loop::flow_declaration`. The runtime
/// converts this into the in-memory
/// `FlowDeclaration` via
/// `FlowDeclaration::from_yaml` after wrapping it
/// in the `mechanism:` key the parser expects.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FlowDeclarationConfig {
    #[serde(default = "default_flow_type")]
    pub flow_type: String,
    #[serde(default = "default_flow_version")]
    pub version: u32,
    #[serde(default)]
    pub terminal_emits: Vec<String>,
    #[serde(default)]
    pub steps: Vec<FlowStepConfig>,
    #[serde(default = "default_repair_budget")]
    pub repair_budget: u32,
    #[serde(default = "default_enforce_schema")]
    pub enforce_schema: String,
    #[serde(default = "default_state_idempotency")]
    pub state_idempotency: String,
}

/// P0-3: typed view of one step in a
/// `mechanism.flow.steps` list. Mirrors
/// `event_loop::flow_declaration::FlowStepDecl`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FlowStepConfig {
    pub id: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub allowed_emits: Vec<String>,
    #[serde(default)]
    pub terminal_when: Option<String>,
    #[serde(default)]
    pub on_partial: std::collections::BTreeMap<String, String>,
}

fn default_flow_type() -> String {
    "declared".to_string()
}

fn default_flow_version() -> u32 {
    1
}

fn default_repair_budget() -> u32 {
    3
}

fn default_enforce_schema() -> String {
    "hard".to_string()
}

fn default_state_idempotency() -> String {
    "required".to_string()
}
