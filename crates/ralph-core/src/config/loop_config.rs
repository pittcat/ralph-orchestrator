//! Event loop configuration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::event_policy::EventPolicyConfig;
use super::execution_contracts::ExecutionContractsConfig;
use super::precheck::PrecheckConfig;
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    /// 2026-07-03-005 plan (P0 fix C7): per-element shape constraint for
    /// array fields in the payload. Key = array field name (e.g.
    /// `"dimensions"`); value = element-level field shape. When `None` or
    /// empty, no element-level validation runs and existing call sites
    /// (which build `EventSchema { ... }` without this field) remain
    /// valid thanks to `Default` + `#[serde(default)]` propagation.
    ///
    /// Today only one consumer: `review.dimensions.complete` validates
    /// that each element of the `dimensions` array has the
    /// `{dimension, status, findings_file}` triple. Status `done` requires
    /// a non-null `findings_file` (no silent-drop of fake "done" elements).
    #[serde(default)]
    pub element_constraints: HashMap<String, ElementConstraint>,
}

/// 2026-07-03-005 plan (P0 fix C7): per-array-field element shape
/// constraint. Applied to every element of the named array field.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ElementConstraint {
    /// Field name on each element (e.g. `"dimension"`).
    pub field: String,
    /// When `true`, this field must exist on every element.
    #[serde(default)]
    pub required: bool,
    /// Optional allowed-values list. When non-empty, the field value
    /// must be in this list (compared as JSON value, not string).
    #[serde(default)]
    pub allowed_values: Vec<serde_json::Value>,
    /// When this map is non-empty, the field is required only when the
    /// referenced field (key) equals the value (compared as JSON). E.g.
    /// `{"status": "done"}` means the `findings_file` field is required
    /// when `status == "done"`. The key/value are JSON values so the
    /// check is type-strict.
    #[serde(default)]
    pub required_when: HashMap<String, serde_json::Value>,
    /// Optional required-only-when-this-element-field-is-non-null
    /// check. When `true`, this field is required AND must not be `null`
    /// when the element's other field (named by `required_when` key)
    /// matches its value. Used to forbid `findings_file: null` for
    /// `status: done` elements.
    #[serde(default)]
    pub forbid_null_when_required: bool,
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

    /// 2026-07-02-004 plan milestone A (U1): opt-in precheck
    /// gate. When `enabled: true` and at least one rule is
    /// declared, `RalphConfig::normalize` rewrites producers of
    /// each guarded topic to emit `<topic>.proposed` and
    /// synthesizes a gate hat that emits either the original
    /// topic (pass) or `<topic>.rejected` (fail). Disabled by
    /// default; `RALPH_PRECHECK_MODE=off` forces no-op even
    /// when enabled.
    #[serde(default)]
    pub precheck: Option<PrecheckConfig>,

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

    /// 2026-07-03-001 plan U1: opt-in rusqlite-backed wave
    /// orchestrator. When `enabled == false` (default) the runtime
    /// preserves the legacy `WaveTracker` path (R1/R3). When
    /// `true` the dispatcher branches to `SupervisorCoordinator`
    /// (U8/U11/U12). The block is opt-in: presets opt in via
    /// `event_loop.supervisor.enabled: true` and the operator may
    /// override per-workspace in `ralph.yml`.
    #[serde(default)]
    pub supervisor: SupervisorConfig,

    /// U18 (P2): macro edge next hint — when `enabled` is true, the
    /// loop prepends a one-line `## NEXT ACTION` block derived from the
    /// most recent accepted business event payload's optional
    /// `next_hint` field (≤120 chars). Defaults to disabled so existing
    /// loops are unaffected.
    #[serde(default)]
    pub macro_edge_next_hint: MacroEdgeNextHintConfig,

    /// 2026-07-06-004 plan (U1): handoff envelope — when
    /// `enabled` is true the isolated prompt builder (U6),
    /// policy-check validator (U8), and `EmitResult` summary
    /// (U9) start honouring the typed `handoff_envelope` field
    /// in business event payloads. Defaults to disabled so
    /// non-serial presets and ad-hoc emits are unaffected
    /// (regression防线 #1).
    #[serde(default)]
    pub handoff_envelope: HandoffEnvelopeConfig,
}

/// 2026-07-06-004 plan U1: typed view of the
/// `event_loop.handoff_envelope:` block in `presets/en/<name>.yml`.
///
/// All four fields default to `false`. The master `enabled` flag
/// exists so non-serial presets, plain `ralph emit` calls, and
/// the policy-check dry-run path keep their pre-004 behaviour
/// with zero changes (regression防线 #1 / #8). The four flags are
/// orthogonal: U7 only opens `prompt_injection`, U10 is the first
/// unit that opens `validate_payload` / `emit_result_summary`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HandoffEnvelopeConfig {
    /// Master switch. When `false` the handoff envelope is dormant
    /// at every layer: payload validator is skipped, prompt
    /// renderer is skipped, `EmitResult` summary is omitted.
    /// `presets/en/ce-executor-serial.yml` is the only preset that
    /// flips this on (U7), and even there `validate_payload` /
    /// `emit_result_summary` stay off until U10.
    #[serde(default)]
    pub enabled: bool,

    /// When true and `enabled` is also true, the isolated prompt
    /// builder prepends the rendered `## HANDOFF ENVELOPE` block
    /// derived from the most recent accepted business event
    /// payload's `handoff_envelope` field (U6).
    #[serde(default)]
    pub prompt_injection: bool,

    /// When true and `enabled` is also true, the policy-check
    /// validation gate rejects payloads that lack a valid
    /// `handoff_envelope` (U8). Off by default so non-serial
    /// presets and ad-hoc emits are not affected.
    #[serde(default)]
    pub validate_payload: bool,

    /// When true and `enabled` is also true, `EmitResult` includes
    /// an optional `handoff_envelope` summary so the agent can see
    /// the envelope it just emitted was recognised (U9).
    #[serde(default)]
    pub emit_result_summary: bool,
}

impl Default for HandoffEnvelopeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            prompt_injection: false,
            validate_payload: false,
            emit_result_summary: false,
        }
    }
}

/// Configuration for the macro-edge next hint (U18 P2).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MacroEdgeNextHintConfig {
    /// Whether to inject the `## NEXT ACTION` block.
    #[serde(default)]
    pub enabled: bool,
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
            // 2026-07-02-004 plan milestone A (U1):
            // precheck is opt-in. None by default so
            // existing presets keep their zero-regression
            // contract.
            precheck: None,
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
            // 2026-07-03-001 plan U1: supervisor is opt-in.
            // Default is `enabled == false` so R3's
            // zero-regression contract holds even when older
            // RalphConfig default serialisations flow through.
            supervisor: SupervisorConfig::default(),
            // U18: macro edge next hint defaults to disabled.
            macro_edge_next_hint: MacroEdgeNextHintConfig::default(),
            // 2026-07-06-004 plan U1: handoff envelope defaults to
            // disabled (every flag false). U7 is the first unit
            // that flips any flag on.
            handoff_envelope: HandoffEnvelopeConfig::default(),
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

    /// 2026-07-03-001 plan U1: a complete `event_loop.supervisor`
    /// block round-trips through serde_yaml with the documented
    /// fields (db_path, max_concurrent_workers,
    /// aggregate_timeout_secs) populated to the exact YAML-declared
    /// values. Required: this is the configuration SSOT used by the
    /// runtime to decide whether to spin up the rusqlite-backed
    /// SupervisorCoordinator.
    #[test]
    fn u1_supervisor_config_parses_full_block() {
        let yaml = r#"
enabled: true
db_path: ".ralph/supervisor.db"
max_concurrent_workers: 8
aggregate_timeout_secs: 900
"#;
        let cfg: SupervisorConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.db_path, ".ralph/supervisor.db");
        assert_eq!(cfg.max_concurrent_workers, 8);
        assert_eq!(cfg.aggregate_timeout_secs, 900);
    }

    /// U1 default shape: when YAML omits the block entirely, the
    /// default is fully populated with `enabled == false` and the
    /// documented defaults for the rest. Required: presets and the
    /// legacy event loop must silently match `EventLoopConfig::default()`
    /// (the disabled branch) so R3 keeps its zero-regression contract.
    #[test]
    fn u1_supervisor_config_defaults_to_disabled() {
        let cfg = SupervisorConfig::default();
        assert!(!cfg.enabled, "default must be disabled to preserve R3");
        assert_eq!(cfg.db_path, ".ralph/supervisor.db");
        assert_eq!(cfg.max_concurrent_workers, 4);
        assert_eq!(cfg.aggregate_timeout_secs, 600);
    }

    /// U1 negative parsing: an unknown key is rejected by
    /// serde_yaml so the operator immediately learns the field is
    /// not honoured. Required: prevents silent typos like
    /// `database_path` from silently no-op'ing.
    #[test]
    fn u1_supervisor_config_rejects_unknown_field() {
        let yaml = r#"
enabled: true
bogus_field: 1
"#;
        let result: Result<SupervisorConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_err(),
            "unknown supervisor field must produce a deserialization error"
        );
    }

    /// U1 nesting contract: omitting `event_loop.supervisor`
    /// entirely from a complete YAML still yields the EventLoopConfig
    /// defaults (`enabled == false`) so R3's "no regression" path is
    /// intact.
    #[test]
    fn u1_event_loop_config_omits_supervisor_block() {
        let yaml = r#"
prompt_file: "PROMPT.md"
"#;
        let cfg: EventLoopConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(
            !cfg.supervisor.enabled,
            "event_loop config without supervisor block must default to disabled"
        );
        assert_eq!(cfg.supervisor.max_concurrent_workers, 4);
        assert_eq!(cfg.supervisor.aggregate_timeout_secs, 600);
        assert_eq!(cfg.supervisor.db_path, ".ralph/supervisor.db");
    }

    /// U1 nesting with explicit supervisor.enabled = true survives
    /// a full EventLoopConfig parse. Required: this is the scenario
    /// U12/U13 will exercise when wiring the dispatcher branch.
    #[test]
    fn u1_event_loop_config_supervises_when_enabled_true() {
        let yaml = r#"
supervisor:
  enabled: true
  max_concurrent_workers: 16
"#;
        let cfg: EventLoopConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.supervisor.enabled);
        assert_eq!(cfg.supervisor.max_concurrent_workers, 16);
        // Fields left at framework defaults must round-trip too.
        assert_eq!(cfg.supervisor.aggregate_timeout_secs, 600);
    }

    /// 2026-07-06-004 plan U1 RED: default-disabled contract. The
    /// typed config must exist and every field must default to
    /// `false` so existing loops, presets, and the policy-check
    /// pipeline keep zero regression (regression防线 #1 / #8).
    #[test]
    fn handoff_envelope_defaults_to_disabled() {
        let cfg = HandoffEnvelopeConfig::default();
        assert!(
            !cfg.enabled,
            "default must be disabled so non-serial presets and ad-hoc emits are unaffected"
        );
        assert!(
            !cfg.prompt_injection,
            "default prompt_injection must be false"
        );
        assert!(
            !cfg.validate_payload,
            "default validate_payload must be false"
        );
        assert!(
            !cfg.emit_result_summary,
            "default emit_result_summary must be false"
        );
    }

    /// 2026-07-06-004 plan U1 RED: explicit flags round-trip
    /// through serde_yaml. The plan defines four orthogonal flags;
    /// each must independently honour an explicit `true` /
    /// `false`. Also asserts `EventLoopConfig` carries the block
    /// with the same defaults when omitted at the top level.
    #[test]
    fn handoff_envelope_deserializes_explicit_flags() {
        let yaml = r#"
enabled: true
prompt_injection: true
validate_payload: false
emit_result_summary: false
"#;
        let cfg: HandoffEnvelopeConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.enabled);
        assert!(cfg.prompt_injection);
        assert!(!cfg.validate_payload);
        assert!(!cfg.emit_result_summary);

        // U1 negative path: unknown fields must surface a parse
        // error so silent typos like `validates_payload` cannot
        // no-op. Mirrors `SupervisorConfig`'s deny_unknown_fields
        // contract (same regression防线 family).
        let bad_yaml = r#"
enabled: true
bogus_field: 1
"#;
        let result: Result<HandoffEnvelopeConfig, _> = serde_yaml::from_str(bad_yaml);
        assert!(
            result.is_err(),
            "unknown handoff_envelope field must produce a deserialization error"
        );

        // U1 nesting: omitting `event_loop.handoff_envelope` from
        // a top-level config still yields the disabled defaults.
        let top_yaml = r#"
prompt_file: "PROMPT.md"
"#;
        let cfg: EventLoopConfig = serde_yaml::from_str(top_yaml).unwrap();
        assert!(
            !cfg.handoff_envelope.enabled,
            "event_loop config without handoff_envelope block must default to disabled"
        );
        assert!(!cfg.handoff_envelope.prompt_injection);
        assert!(!cfg.handoff_envelope.validate_payload);
        assert!(!cfg.handoff_envelope.emit_result_summary);

        // U1 nesting with explicit enabled = true survives a full
        // EventLoopConfig parse.
        let top_yaml2 = r#"
handoff_envelope:
  enabled: true
  validate_payload: true
"#;
        let cfg: EventLoopConfig = serde_yaml::from_str(top_yaml2).unwrap();
        assert!(cfg.handoff_envelope.enabled);
        assert!(cfg.handoff_envelope.validate_payload);
        assert!(!cfg.handoff_envelope.prompt_injection);
    }
}

/// 2026-07-03-001 plan U1: `event_loop.supervisor` block SSOT.
///
/// `SupervisorConfig` toggles the rusqlite-backed wave orchestrator.
/// When `enabled == false` (default) the runtime never instantiates
/// the supervisor store: no `.ralph/supervisor.db` is created and
/// the legacy `WaveTracker` path takes over unchanged (R3).
///
/// All fields are documented in
/// `docs/brainstorms/2026-07-03-supervisor-rusqlite-parallel-preset-requirements.md`
/// and exist for the preset (U13) and runtime wiring (U8/U11/U12)
/// to read. The default values must stay small enough that an
/// accidental `enabled: true` does not consume excessive resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorConfig {
    /// Master switch. When `false` the supervisor runtime path is
    /// dormant: `SupervisorStore::open` is never called, the DB file
    /// is not created, and the dispatcher falls back to the legacy
    /// `WaveTracker` (R1/R3).
    #[serde(default = "default_supervisor_enabled")]
    pub enabled: bool,

    /// SQLite database file path. Relative paths resolve against the
    /// loop workspace (`<workspace>/.ralph/`); absolute paths are
    /// honoured as-is. The runtime MUST refuse to start the
    /// supervisor when the file is not openable (R-C4 / fail-closed).
    #[serde(default = "default_supervisor_db_path")]
    pub db_path: String,

    /// Maximum number of worker slots active concurrently across all
    /// waves. Acts as a soft backpressure ceiling: when active
    /// workers reach this number, additional waves sit in the FIFO
    /// `wave_queue` table until a slot frees (R-A2 / R6).
    #[serde(default = "default_supervisor_max_concurrent_workers")]
    pub max_concurrent_workers: u32,

    /// Wall-clock budget for one wave's collect phase. When the
    /// budget expires, partial waves are marked `timeout` and the
    /// supervisor injects `*.wave.failed(reason=timeout)` rather
    /// than running compensation silently (R-C3 / KTD-8).
    #[serde(default = "default_supervisor_aggregate_timeout_secs")]
    pub aggregate_timeout_secs: u64,
}

fn default_supervisor_enabled() -> bool {
    false
}

fn default_supervisor_db_path() -> String {
    ".ralph/supervisor.db".to_string()
}

fn default_supervisor_max_concurrent_workers() -> u32 {
    4
}

fn default_supervisor_aggregate_timeout_secs() -> u64 {
    600
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            enabled: default_supervisor_enabled(),
            db_path: default_supervisor_db_path(),
            max_concurrent_workers: default_supervisor_max_concurrent_workers(),
            aggregate_timeout_secs: default_supervisor_aggregate_timeout_secs(),
        }
    }
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

    /// 2026-07-02-006 plan (U1): opt-in
    /// `WorkflowPhaseAuthority` engine config. Mirrors
    /// `mechanism.phase_authority` in
    /// `presets/en/<name>.yml`. When `None` or
    /// `enabled == false`, the runtime's phase
    /// authority is a no-op and behaviour matches the
    /// pre-006 baseline (serial runs through the
    /// existing `FlowStepScopeStage` flow guard). The
    /// typed view lives in
    /// `event_loop::phase_authority::config::PhaseAuthorityConfig`
    /// and is re-exported from there.
    #[serde(default)]
    pub phase_authority:
        Option<crate::event_loop::phase_authority::config::PhaseAuthorityConfig>,
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
    // 2026-07-02-001 plan U4 (Fix D): the `type` rename mirrors
    // `event_loop::flow_declaration::FlowDeclaration::flow_type` so a
    // non-default value declared in a preset's `mechanism.flow` block
    // (e.g. `type: declared` written by `ce-executor-serial.yml`)
    // is no longer silently dropped to the framework default at the
    // config-typed view. Pre-fix: `flow_type` carried no rename, so
    // serde_yaml saw the YAML key `type` and fell through to
    // `default_flow_type()`. The runtime's downstream
    // `FlowDeclaration::from_yaml` reads the same `type` key correctly
    // (it has its own `#[serde(rename = "type")]`), so the discrepancy
    // was invisible until a future guard started inspecting
    // `config.mechanism.flow.flow_type` directly.
    #[serde(rename = "type", default = "default_flow_type")]
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

#[cfg(test)]
mod flow_declaration_config_tests {
    //! 2026-07-02-001 plan U4 (Fix D) round-trip pins: the
    //! `type` rename on `FlowDeclarationConfig::flow_type` must
    //! behave like its sibling
    //! `event_loop::flow_declaration::FlowDeclaration::flow_type`
    //! (which already had `#[serde(rename = "type")]` since the
    //! 2026-06-27 mechanism foundation). The pre-fix
    //! `FlowDeclarationConfig::flow_type` had no rename, so any
    //! preset declaring `type: <non-default>` in its
    //! `mechanism.flow` block would silently fall back to
    //! `default_flow_type() == "declared"` at this layer.
    //!
    //! See `docs/plans/2026-07-02-001-fix-hat-routing-next-hop-plan.md` U4 / R5.
    use super::*;

    /// U4 happy path: `type: <non-default-sentinel>` survives
    /// the rename. Pre-fix: this assertion would have observed
    /// the framework default `"declared"`, hiding the bug.
    #[test]
    fn flow_type_renames_from_yaml_type_key() {
        let yaml = r#"
type: "u4-sentinel"
version: 2
steps:
  - id: "step-01"
"#;
        let cfg: FlowDeclarationConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            cfg.flow_type, "u4-sentinel",
            "`type:` in YAML must land on `flow_type` via the `rename` attribute; \
             pre-fix the rename was missing and this would silently be the framework default"
        );
        assert_eq!(cfg.version, 2);
    }

    /// U4 default path: omitting `type:` must still produce the
    /// framework default `"declared"` so backward compatibility is
    /// preserved for presets that don't declare a flow type.
    #[test]
    fn flow_type_defaults_when_omitted() {
        let yaml = r#"
version: 1
steps:
  - id: "step-01"
"#;
        let cfg: FlowDeclarationConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            cfg.flow_type, "declared",
            "missing `type:` must fall back to default_flow_type() to preserve \
             presets that omit the field"
        );
    }
}
