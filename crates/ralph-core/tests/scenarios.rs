//! E2E scenario tests for event-loop redesign.
//!
//! Tests cover:
//! - Solo mode (Ralph with no hats)
//! - Multi-hat delegation
//! - Orphaned event handling
//! - Default publishes fallback
//! - Mixed backends
//! - AutoResearch workflow guards

use ralph_core::supervisor::{
    CoordinatorAction, InMemoryCoordinatorBridge, InMemorySupervisorStore, PhaseInputs,
    SupervisorBridge, SupervisorStore, TerminalEvidence, WaveKind,
};
use ralph_core::testing::{MockBackend, Scenario, ScenarioRunner};
use ralph_core::{EventLoop, EventParser, HatConfig, LoopContext, RalphConfig, TerminationReason};
use serde::Deserialize;
use std::fs;
use std::sync::Arc;

/// 2026-06-20-002 plan U3 review: production-side prompt headings
/// (preset/engine/lint_mirror.rs:28, :52) hoisted to test-side
/// constants so fixtures cannot drift when production rewrites
/// the heading. YAML fixtures may either write the literal or
/// omit `block:` to use the default.
pub const LINT_MIRROR_HEADING: &str = "## LINT MIRROR";
pub const LINT_RESUME_REQUIRED_HEADING: &str = "## LINT RESUME REQUIRED";

fn default_lint_resume_required() -> String {
    LINT_RESUME_REQUIRED_HEADING.to_string()
}

#[derive(Debug, Deserialize)]
struct ScenarioYaml {
    name: String,
    description: String,
    config: ConfigYaml,
    #[serde(default)]
    compiled_contract: bool,
    /// Mock responses: each entry is either a raw XML string
    /// (the LLM's emitted text) or a `{text, hat}` object. The
    /// `hat` field, when present, is stamped on every event parsed
    /// from the response as the JSONL `hat` field. The runtime
    /// origin guard (`event_loop::filter_events_by_origin`) uses
    /// `from_hat` to evaluate whether the emitting hat is in the
    /// hat registry; scenarios that exercise the origin guard need
    /// to set `hat` on their mock events.
    #[serde(default, deserialize_with = "deserialize_mock_responses")]
    mock_responses: Vec<MockResponseYaml>,
    #[serde(default)]
    checkpoints: Vec<CheckpointYaml>,
    expected: ExpectedYaml,
    /// 2026-06-18-002 plan U9: scenario fixture files written to the
    /// temp workspace root **before** the run starts. General fixture
    /// mechanism for any scenario that needs files pre-staged (e.g.
    /// `.ralph/agent/step-handoff/*.md` for step_handoff scenarios).
    #[serde(default)]
    fixture_files: Vec<FixtureFileYaml>,
    /// 2026-07-27-001: optional supervisor fan-in controls for BDD
    /// scenarios that exercise production `SupervisorCoordinator`
    /// tick (expected slot total + optional forced terminal).
    #[serde(default)]
    supervisor_fan_in: Option<SupervisorFanInYaml>,
    /// U3 (plan 2026-08-03-004): optional parallel-forge resume
    /// bootstrap. When present, the runner builds a real U1
    /// `ResumeManifest` from this block, converts it through
    /// `rejection::task_resume_from_manifest` (digest / pending-hat
    /// validation), and boots the loop with
    /// `EventLoop::initialize_manifest_resume` INSTEAD of the
    /// configured starting event. Models a reused worktree whose old
    /// runtime state is gone: only the manifest boundary survives.
    #[serde(default)]
    resume_bootstrap: Option<ResumeBootstrapYaml>,
}

/// U3 (plan 2026-08-03-004): fixture-side shape of the resume
/// manifest boundary. Mirrors the fields of
/// `ralph_core::parallel_forge_resume::BoundaryRecord` that the
/// U2 conversion consumes.
#[derive(Debug, Deserialize, Clone)]
struct ResumeBootstrapYaml {
    /// Accepted terminal boundaries of the OLD run, in commit order.
    /// The last entry becomes the resume payload's
    /// `accepted_boundary_topic`.
    accepted: Vec<ResumeAcceptedBoundaryYaml>,
    /// The hat the bootstrap `task.resume` must target.
    pending_hat: String,
    /// Snapshot of the event that originally triggered the pending hat.
    original_trigger: ResumeOriginalTriggerYaml,
    /// Wave correlation metadata of the boundary event.
    #[serde(default)]
    wave: Option<ResumeWaveYaml>,
    /// How many times the bootstrap runs before the first iteration.
    /// Defaults to 1; S7 idempotence scenarios set 2 to prove a
    /// repeated manifest bootstrap stays a no-op.
    #[serde(default = "default_resume_repeat")]
    repeat: usize,
}

fn default_resume_repeat() -> usize {
    1
}

#[derive(Debug, Deserialize, Clone)]
struct ResumeAcceptedBoundaryYaml {
    topic: String,
    #[serde(default)]
    hat: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ResumeOriginalTriggerYaml {
    topic: String,
    /// JSON payload snapshot embedded in the resume message.
    payload: String,
    #[serde(default)]
    hat: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ResumeWaveYaml {
    wave_id: String,
    wave_index: u32,
    wave_total: u32,
}

/// Controls how `run_bdd_supervisor_fan_in` registers and ticks waves.
#[derive(Debug, Deserialize, Default, Clone)]
struct SupervisorFanInYaml {
    /// Slot count passed to `register_wave_if_absent`. When omitted,
    /// defaults to `event_loop.supervisor.max_concurrent_workers`.
    #[serde(default)]
    expected_slots: Option<u32>,
    /// When `"timeout"`, force a terminal coordinator tick even if
    /// fewer than `expected_slots` have completed (cancel + elapsed
    /// > aggregate_timeout). Used for partial/timeout production
    /// fan-in proofs without wall-clock waits.
    #[serde(default)]
    force_terminal: Option<String>,
    /// Minimum completed slots required before `force_terminal` fires.
    /// Defaults to 1. Set to 2+ when the fixture intentionally
    /// completes a subset before timeout.
    #[serde(default)]
    min_slots_before_force: Option<u32>,
}

/// Plan 2026-07-28-001 U2/U3 (`task_ledger` assertion).
#[derive(Debug, Deserialize, Clone)]
struct TaskLedgerRowYaml {
    task_key: String,
    status: String,
    #[serde(default)]
    blocked_by_keys: Vec<String>,
}

/// Plan 2026-07-28-001 U2/U3 (`payload_task_refs` assertion).
#[derive(Debug, Deserialize, Clone)]
struct PayloadTaskRefYaml {
    topic: String,
    occurrence: usize,
    payload_field: String,
    task_key: String,
}

/// Plan 2026-07-28-001 U2/U3 (`supervisor_waves` assertion).
#[derive(Debug, Deserialize, Clone)]
struct SupervisorWaveYaml {
    wave_id: String,
    #[allow(dead_code)] // reserved for future phase-aware assertions
    kind: String,
    expected_total: u32,
    completed_count: u32,
    failed_count: u32,
    #[allow(dead_code)] // reserved for future phase-aware assertions
    phase: String,
}

#[derive(Debug, Deserialize, Clone)]
struct MockResponseYaml {
    text: String,
    /// Optional source hat to stamp on parsed events. When `None`
    /// the events are written without a `hat` field (legacy behavior).
    #[serde(default)]
    hat: Option<String>,
}

fn deserialize_mock_responses<'de, D>(deserializer: D) -> Result<Vec<MockResponseYaml>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    // Accept either a list of strings (legacy) or a list of
    // `{text, hat}` objects. This keeps existing scenarios
    // source-compatible while letting new ones opt into hat
    // tagging.
    let raw: Vec<serde_yaml::Value> = Deserialize::deserialize(deserializer)?;
    let mut out = Vec::with_capacity(raw.len());
    for (i, v) in raw.into_iter().enumerate() {
        match v {
            serde_yaml::Value::String(s) => out.push(MockResponseYaml { text: s, hat: None }),
            serde_yaml::Value::Mapping(_) => {
                let parsed: MockResponseYaml = serde_yaml::from_value(v)
                    .map_err(|e| D::Error::custom(format!("mock_responses[{i}]: {e}")))?;
                out.push(parsed);
            }
            other => {
                return Err(D::Error::custom(format!(
                    "mock_responses[{i}]: expected string or mapping, got {other:?}"
                )));
            }
        }
    }
    Ok(out)
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize)]
struct FixtureFileYaml {
    /// Path relative to the workspace root (e.g.
    /// `.ralph/agent/step-handoff/1-2-a-b.md`).
    path: String,
    content: String,
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize)]
struct ConfigYaml {
    prompt_file: String,
    max_iterations: u32,
    #[serde(default)]
    hats: serde_yaml::Value,
    #[serde(default)]
    event_loop: serde_yaml::Value,
    #[serde(default)]
    core: serde_yaml::Value,
    #[serde(default)]
    tasks: serde_yaml::Value,
    #[serde(default)]
    topic_owners: serde_yaml::Value,
    #[serde(default)]
    topic_format_whitelist: serde_yaml::Value,
    /// Top-level `mechanism:` block (mirrors `RalphConfig.mechanism`).
    #[serde(default)]
    mechanism: serde_yaml::Value,
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize)]
struct ExpectedYaml {
    iterations: usize,
    #[serde(default)]
    events: Vec<EventYaml>,
    #[serde(default)]
    workflow_progress: Vec<WorkflowProgressYaml>,
    /// Events that must NOT have been accepted by the event loop.
    /// Used to assert that semantic gates drop bypass attempts.
    #[serde(default)]
    absent_events: Vec<EventYaml>,
    /// P1-2 (2026-06-28 review): the wire-level
    /// assertion is restored via the recovery
    /// envelope channel. Each substring here MUST
    /// appear in the `recovery.jsonl` written under
    /// the session directory so operators can
    /// attribute the gate's rejection to a specific
    /// reason. The recovery-envelope contract
    /// survives the `accepted_events` / `seen_topics`
    /// admission semantics — see
    /// `evaluate_emit_gate_for_jsonl_event` in
    /// `event_loop/mod.rs`.
    #[serde(default)]
    recovery_contains: Vec<String>,
    completion: bool,
    /// 2026-06-18-002 plan U8 (KTD-17): assert that for a given
    /// `hat`, the last prompt the runner built contains every
    /// substring. Used by step_handoff scenarios to verify
    /// `## STEP HANDOFF` injection end-to-end. Empty list = no
    /// prompt assertions.
    #[serde(default)]
    prompt_contains: Vec<PromptContainsYaml>,
    /// 2026-06-20-002 plan U1 (R-H1 ~ R-H6): assert runtime
    /// **memory state** at specific iterations. Each entry has an
    /// `at_iteration` field (1-indexed) selecting which snapshot
    /// to evaluate against. Empty list = no state assertions.
    /// The runner records `LoopStateSnapshot` + `BuildPromptSnapshot`
    /// at the end of every iteration, then evaluates the list in
    /// order after the loop completes (read-only; the runner
    /// itself never mutates state from `assert_state`).
    #[serde(default)]
    assert_state: Vec<AssertionYaml>,
    /// 2026-07-02-005 plan Final Verification: assert accepted
    /// event counts per topic (from `ProcessedEvents::accepted_events`).
    #[serde(default)]
    event_topic_counts: Vec<EventTopicCountYaml>,
    /// Assert selected payload fields on the Nth accepted event for a topic.
    #[serde(default)]
    payload_matches: Vec<PayloadMatchYaml>,
    /// Plan 2026-07-28-001 U2/U3 (R1/R2/R4/R5/S8/S12). Assert the
    /// live task ledger in the scenario temp workspace matches the
    /// declared task DAG after the run completes. The fixture
    /// declares each task row by stable `task_key` plus the
    /// live-status / blocker set the orchestrator should observe;
    /// unknown keys, missing rows, or extra rows fail the scenario.
    #[serde(default)]
    task_ledger: Vec<TaskLedgerRowYaml>,
    /// Plan 2026-07-28-001 U2/U3 (R4/R5/S12). Assert that the
    /// Nth accepted event of a given topic carries a top-level
    /// payload field whose value equals the live task id resolved
    /// from the matched `task_key`. This is the canonical
    /// proof that dispatcher / executor payloads reference live
    /// task ids rather than hand-rolled ones.
    #[serde(default)]
    payload_task_refs: Vec<PayloadTaskRefYaml>,
    /// Plan 2026-07-28-001 U2/U3 (R5/S8/S12). Assert that the
    /// in-memory supervisor store reports expected waves with the
    /// declared kind / expected_total / completed_count /
    /// failed_count / phase. Reading from the production
    /// `InMemoryCoordinatorBridge` keeps the assertion on the
    /// real fan-in seam.
    #[serde(default)]
    supervisor_waves: Vec<SupervisorWaveYaml>,
    /// Plan 2026-07-28-001 U3 (S12). Assert that the live task ledger
    /// reports a set of ready task keys (no open blockers, status
    /// `open`) that **matches** this list after deduplication and
    /// lexicographic ordering. Used to prove the U2/U3 next-ready
    /// set flows from `forge.worktrees.ready` into the supervisor.
    #[serde(default)]
    ready_task_keys: Vec<String>,
    /// U3 (plan 2026-08-03-004). Exact ordered list of hats that must
    /// have had a prompt built (one entry per activation), in
    /// iteration order. Iterations without an activation contribute
    /// nothing. Pins resume routing: the pending hat activates first
    /// and upstream hats do not re-activate; a repeated bootstrap must
    /// not duplicate activations.
    #[serde(default)]
    activation_hats: Vec<String>,
}

/// One entry in `ExpectedYaml.assert_state` (2026-06-20-002 plan U1).
///
/// Exactly one variant field is set per entry. The discriminator
/// is the field name; serde's untagged enum on the
/// `Mapping`-shaped entries lets YAML files write either of:
///
///   - pending_lint_resume: { at_iteration: 3, topic: work.done, reason_contains: "missing" }
///   - pending_lint_resume_cleared: { at_iteration: 4 }
///   - rejection_digest_contains: { at_iteration: 5, contains_topic: work.done, contains_reason: "missing" }
///   - prompt_injects: { at_iteration: 4, hat: executor, block: "## LINT RESUME REQUIRED" }
///
/// `at_iteration` is mandatory on every variant and must satisfy
/// `1 <= at_iteration <= actual_iterations` (R-H3). Out-of-range
/// values are reported as `at_iteration=N out of range [1, M]`
/// before evaluating.
#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize)]
struct AssertionYaml {
    at_iteration: usize,
    // pending_lint_resume: state.pending_lint_resume is Some
    // matching the optional topic / reason predicates.
    #[serde(default)]
    pending_lint_resume: Option<PendingLintResumeYaml>,
    #[serde(default)]
    pending_lint_resume_cleared: Option<PendingLintResumeClearedYaml>,
    #[serde(default)]
    rejection_digest_contains: Option<RejectionDigestContainsYaml>,
    #[serde(default)]
    prompt_injects: Option<PromptInjectsYaml>,
    // Plan 2026-06-20-001 KTD-7: lint circuit breaker assertion.
    // Predicate matches when `state.lint_circuit_breaker_tripped`
    // equals the supplied `tripped` value. Used by scenario 10
    // (`serial_lint_circuit_breaker.yaml`) to assert the
    // breaker tripped at iter 4 after 3 consecutive engine-gate
    // rejections.
    #[serde(default)]
    lint_circuit_breaker: Option<LintCircuitBreakerYaml>,
    // 2026-06-21-002 plan U9: assert a CorrectionContext is
    // queued in `state.prompt_context.correction_blocks`. Matches
    // by `reason_code` prefix, `retry_count`, and
    // `needs_escalation` flag — used by the new
    // `correction_deterministic` and `correction_three_escalation`
    // scenarios.
    #[serde(default)]
    correction_block_present: Option<CorrectionBlockPresentYaml>,
    // 2026-06-21-002 plan U9: assert a rejection record exists
    // in the workspace-level `.ralph/recovery.jsonl` with a
    // matching `reason_code` prefix. Used by
    // `diagnose_from_ledger.yml` to pin the runtime→CLI
    // surface contract.
    #[serde(default)]
    rejection_log_contains_reason_code: Option<RejectionLogContainsReasonCodeYaml>,
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize, Default)]
struct LintCircuitBreakerYaml {
    #[serde(default)]
    tripped: Option<bool>,
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize, Default)]
struct PendingLintResumeYaml {
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    reason_contains: Option<String>,
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize, Default)]
struct PendingLintResumeClearedYaml {}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize, Default)]
struct RejectionDigestContainsYaml {
    #[serde(default)]
    contains_topic: Option<String>,
    #[serde(default)]
    contains_reason: Option<String>,
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize)]
struct PromptInjectsYaml {
    hat: String,
    #[serde(default = "default_lint_resume_required")]
    block: String,
}

// 2026-06-21-002 plan U9: `correction_block_present` predicate.
// At least one entry in
// `state.prompt_context.correction_blocks` must match the
// supplied `reason_code_prefix`, and (when present) the
// `retry_count` and `needs_escalation` fields must match.
#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize, Default)]
struct CorrectionBlockPresentYaml {
    #[serde(default)]
    reason_code_prefix: Option<String>,
    #[serde(default)]
    retry_count: Option<u32>,
    #[serde(default)]
    needs_escalation: Option<bool>,
    /// U5 (plan 2026-08-06-001): at least one
    /// `evidence.observed[].field` must contain each of
    /// the supplied field names (substring match so the
    /// test is not coupled to the exact JSON value
    /// serialisation).
    #[serde(default)]
    evidence_observed_contains: Option<Vec<String>>,
    /// U5: the `evidence.invariant` text must contain the
    /// supplied substring (verbatim match against
    /// `rule.message` is the typical use case).
    #[serde(default)]
    evidence_invariant_contains: Option<String>,
    /// U5: the feedback kind must match
    /// (`"semantic"` / `"mechanical"` / `"unknown"`).
    /// Drives the S1 / S2 evidence-bound feedback
    /// assertion: a semantic rejection MUST carry
    /// observed + invariant + required proof.
    #[serde(default)]
    feedback_kind: Option<String>,
}

// 2026-06-21-002 plan U9: `rejection_log_contains_reason_code`
// predicate. Reads the workspace-level `.ralph/recovery.jsonl`
// and asserts at least one record's `reason_code` starts with
// the supplied prefix. Used by `diagnose_from_ledger.yml` to
// pin the runtime surface contract that the CLI binary's
// `--from-ledger` reads (T8.1-T8.3 in `crates/ralph-cli/tests/diagnose.rs`).
#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize, Default)]
struct RejectionLogContainsReasonCodeYaml {
    prefix: String,
}

/// Read-only snapshot of `LoopState` taken at the end of each
/// iteration (2026-06-20-002 plan U2 / R-H2). Cloned so that
/// `assert_state` evaluation does not borrow the live `&mut
/// EventLoop` past the iteration loop. Fields deliberately
/// mirror only what `assert_state` predicates need; no new
/// EventLoop API required.
#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Clone)]
struct LoopStateSnapshot {
    iteration: u32,
    /// Clone of `state.pending_lint_resume` (Option<LintResumeHint>).
    /// We store only the fields the predicates inspect; deep
    /// cloning the full hint struct would not buy anything and
    /// would couple the test to engine internals.
    pending_lint_resume: Option<PendingLintResumeSummary>,
    /// Snapshot of `state.recent_rejection_digest` (BTreeMap<String,
    /// RejectionDigestEntry>) flattened into `(topic, reason)`
    /// pairs that the predicates can match on.
    rejection_digest_entries: Vec<RejectionDigestSummary>,
    /// Snapshot of `state.scope_violation_circuit_breaker_tripped`
    /// being Some. The serial-lint domain does not have its own
    /// circuit breaker (plan 2026-06-20-002 U1 review: this
    /// field is the closest existing analog), so scenarios that
    /// need a "linter tripped" signal use this via the
    /// `circuit_breaker_tripped` assertion (added in U6 if needed).
    /// Tracked here for future extensibility.
    #[allow(dead_code)]
    scope_violation_circuit_breaker_tripped: bool,
    /// Snapshot of `state.lint_circuit_breaker_tripped`. Plan
    /// 2026-06-20-001 KTD-7 / RISK-6: when the engine gate
    /// rejects every event for
    /// `LINT_CIRCUIT_BREAKER_LIMIT` consecutive iterations,
    /// this latches. Scenarios use
    /// `lint_circuit_breaker_tripped: { tripped: true }` to
    /// assert the trip happened. Distinct from
    /// `scope_violation_circuit_breaker_tripped` (different
    /// domain).
    #[allow(dead_code)]
    lint_circuit_breaker_tripped: bool,
    /// Snapshot of `state.consecutive_engine_gate_rejections`.
    /// Scenarios use `lint_circuit_breaker_counter: { gte: N }`
    /// to assert the breaker counter crossed a threshold. This
    /// is the raw counter, not the trip latch — useful for
    /// asserting "counter climbed to N but breaker had not yet
    /// tripped" intermediate states.
    #[allow(dead_code)]
    consecutive_engine_gate_rejections: u32,
    /// 2026-06-21-002 plan U9: snapshot of
    /// `state.prompt_context.correction_blocks`, flattened into
    /// the fields the `correction_block_present` predicate
    /// inspects (`reason_code`, `retry_count`,
    /// `needs_escalation`). Mirrors the
    /// `rejection_digest_entries` pattern: the live `BTreeMap`
    /// is unwrapped into a `Vec` of summaries so the predicate
    /// does not need to know about
    /// `ralph_core::correction::CorrectionContext`'s full shape.
    #[allow(dead_code)]
    correction_block_summaries: Vec<CorrectionBlockSummary>,
    /// 2026-06-21-002 plan U9: snapshot of
    /// `state.prompt_context.resume_blocks`. Future U9 scenarios
    /// that pin the `--continue` path use this via the
    /// `resume_block_present` predicate (added when needed).
    #[allow(dead_code)]
    resume_block_summaries: Vec<ResumeBlockSummary>,
    /// 2026-06-21-002 plan U9: absolute path to the workspace
    /// `.ralph/recovery.jsonl` at snapshot time. Stored so the
    /// `rejection_log_contains_reason_code` predicate can read
    /// the live log without re-deriving the path.
    #[allow(dead_code)]
    workspace_recovery_log: Option<std::path::PathBuf>,
}

#[allow(dead_code)] // Test infrastructure
#[derive(Debug, Clone)]
struct PendingLintResumeSummary {
    topic: String,
    reason: String,
}

#[allow(dead_code)] // Test infrastructure
#[derive(Debug, Clone)]
struct RejectionDigestSummary {
    code: String,
    last_topic: String,
    last_message: String,
}

// 2026-06-21-002 plan U9: summary of one
// `state.prompt_context.correction_blocks` entry. Mirrors the
// shape of `ralph_core::correction::CorrectionContext` but
// only carries the fields the predicate inspects.
#[allow(dead_code)] // Test infrastructure
#[derive(Debug, Clone)]
struct CorrectionBlockSummary {
    reason_code: String,
    retry_count: u32,
    needs_escalation: bool,
    /// U5 (plan 2026-08-06-001): feedback_kind as a
    /// stable snake_case string ("semantic" / "mechanical" /
    /// "unknown").  Used by the predicate to assert
    /// `correction_block_present.feedback_kind`.
    feedback_kind: String,
    /// U5: field names declared in
    /// `evidence.observed[].field` so the predicate can
    /// match without reaching into the inner serde types.
    evidence_observed_fields: Vec<String>,
    /// U5: `evidence.invariant` text so the predicate can
    /// substring-match without parsing the inner types.
    evidence_invariant: String,
}

// 2026-06-21-002 plan U9: summary of one
// `state.prompt_context.resume_blocks` entry. Future predicate
// expansion point.
#[allow(dead_code)] // Test infrastructure
#[derive(Debug, Clone)]
struct ResumeBlockSummary {
    loop_id: String,
    last_iteration: u32,
}

/// Read-only snapshot of one `build_prompt` invocation's output
/// (2026-06-20-002 plan U2). The runner already records the
/// **last** prompt per hat; `assert_state` needs iteration-level
/// access, so we also push per-iteration `(hat, prompt)` pairs.
#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Clone)]
struct BuildPromptSnapshot {
    iteration: u32,
    hat: String,
    prompt: String,
}

/// `ExpectedYaml.prompt_contains` 元素:断言 hat 的 prompt 含
/// 列出的所有 substrings。
#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize)]
struct PromptContainsYaml {
    hat: String,
    #[serde(default)]
    substrings: Vec<String>,
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize)]
struct EventYaml {
    topic: String,
}

/// Assert exact count of accepted events for a topic across the run.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct EventTopicCountYaml {
    topic: String,
    count: usize,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct PayloadMatchYaml {
    topic: String,
    #[serde(default)]
    occurrence: Option<usize>,
    fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Default)]
struct CheckpointYaml {
    after_response: usize,
    #[serde(default)]
    workflow_progress: Vec<WorkflowProgressYaml>,
    #[serde(default)]
    completion_rejected: bool,
    /// After this response, call `check_completion_event()` and assert
    /// the loop honors `LOOP_COMPLETE` (sets `completion_honored`).
    #[serde(default)]
    honor_completion: bool,
    /// Sleep this many milliseconds after evaluating the checkpoint.
    /// Used by flow-reliability scenarios that need real wall-clock
    /// staleness to trigger the incomplete-wave gate.
    #[serde(default)]
    sleep_ms: u64,
}

#[derive(Debug, Deserialize)]
struct WorkflowProgressYaml {
    chain: String,
    phase: usize,
    #[serde(default)]
    instance: Option<String>,
}

fn load_scenario(path: &str) -> ScenarioYaml {
    let content =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e));
    serde_yaml::from_str(&content).unwrap_or_else(|e| panic!("Failed to parse {}: {}", path, e))
}

/// Shared scenario runner used by both the workflow-guard
/// harness and the in-line `test_isolated_with_event_projection`
/// test. (2026-06-20-002 plan U2/R-H2 + U3/Q-3: extracted from
/// the two near-duplicate bodies.)
///
/// Sets up the tempdir, pre-stages fixture files, builds a
/// baseline `RalphConfig`, runs the iteration loop while
/// recording per-iteration `LoopStateSnapshot` and
/// `BuildPromptSnapshot` records, and evaluates all common
/// assertions (`expected.events`, `absent_events`,
/// `prompt_contains`, `workflow_progress`, completion, and
/// `assert_state`).
///
/// 2026-07-03-001 Phase 6 BDD helper: append a supervisor-
/// injected coordination event directly to the trusted events
/// JSONL WITHOUT advancing the `EventReader` cursor. The
/// follow-up `process_events_from_jsonl` call in the runner
/// then picks it up through the normal acceptance path (origin
/// guard → policy → state machine → bus → `seen_topics`).
///
/// We bypass `EventLoop::persist_system_injected_jsonl_event`
/// because that method advances the reader cursor past the
/// injected line (correct for production to avoid double-
/// processing, but wrong for the BDD stub runner which needs
/// the re-read to surface the event in `seen_topics`).
fn bdd_append_supervisor_event(
    event_loop: &EventLoop,
    topic: &str,
    payload: &serde_json::Value,
    hat_id: &str,
) {
    use std::io::Write;
    let events_path = event_loop
        .loop_context()
        .map(|ctx| ctx.events_path())
        .unwrap_or_else(|| std::path::PathBuf::from(".ralph/events.jsonl"));
    let ts = chrono::Utc::now().to_rfc3339();
    let record = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts,
        "hat": hat_id,
        "source": hat_id,
        "system_injected": true,
    });
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
    {
        let _ = writeln!(file, "{}", record);
    }
}

/// 2026-07-03-001 plan U10 / fix-plan U10 + Phase 6 BDD
/// realization + 2026-07-27-001 terminal fan-in:
/// drive the supervisor coordinator `tick` from the BDD scenario
/// runner so `*.wave.complete` / `*.wave.failed` are produced by
/// the real `SupervisorCoordinator` (via `InMemoryCoordinatorBridge`)
/// instead of being faked via a mock `system_injected` response.
///
/// Contract:
/// - Called AFTER `process_events_from_jsonl` so accepted events are visible.
/// - Accumulates `*.unit.done` slots across iterations under `waves`.
/// - Registers with `expected_slots` when provided so partial waves
///   stay Collect until complete or `force_terminal`.
/// - When `expected_slots` is `None` (legacy scenarios without
///   `supervisor_fan_in`), ticks with the accumulated slot count —
///   matching pre-2026-07-27 auto behavior for 1-slot pins like U13.
/// - On `force_terminal=timeout`: cancel + never-started + salvage +
///   elapsed > aggregate_timeout, then tick → InjectedFailed.
/// - Review payloads carry `completed_dimensions` / `missing_dimensions`.
fn run_bdd_supervisor_fan_in(
    event_loop: &mut EventLoop,
    bridge: &InMemoryCoordinatorBridge,
    accepted_events: &[ralph_proto::Event],
    aggregate_timeout_secs: u64,
    expected_slots: Option<u32>,
    force_terminal: bool,
    min_slots_before_force: u32,
    waves: &mut std::collections::HashMap<String, Vec<(u32, String, usize, Option<String>)>>,
    ticked_waves: &mut std::collections::HashSet<String>,
) -> usize {
    let mut wave_kind: std::collections::HashMap<String, WaveKind> =
        std::collections::HashMap::new();

    for ev in accepted_events {
        let payload: serde_yaml::Value = match serde_yaml::from_str(&ev.payload) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let is_exec_done = ev.topic.as_str() == "exec.unit.done";
        let is_fix_done = ev.topic.as_str() == "fix.unit.done";
        let is_review_done = ev.topic.as_str() == "review.unit.done";
        if !is_exec_done && !is_fix_done && !is_review_done {
            continue;
        }
        let wave_id = payload
            .get("wave_id")
            .and_then(|v| v.as_str())
            .unwrap_or("bdd-wave");
        let slot_index = payload
            .get("slot_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let content_hash = payload
            .get("content_hash")
            .and_then(|v| v.as_str())
            .unwrap_or("bdd-hash")
            .to_string();
        let dimension = payload
            .get("dimension")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let kind = if is_fix_done {
            WaveKind::Fix
        } else if is_review_done {
            WaveKind::Review
        } else {
            WaveKind::Exec
        };
        wave_kind.insert(wave_id.to_string(), kind);
        let entry = waves.entry(wave_id.to_string()).or_default();
        if !entry.iter().any(|(idx, _, _, _)| *idx == slot_index) {
            entry.push((slot_index, content_hash, 1, dimension));
        }
    }

    if waves.is_empty() {
        return 0;
    }

    // `None` → auto: tick with the largest accumulated slot count this
    // call (legacy 1-slot pins). `Some(n)` → wait until n slots arrive
    // (or force_terminal). Never default to max_concurrent_workers —
    // that silently stalls fixtures whose slot count < worker cap.
    let expected_slots = expected_slots
        .unwrap_or_else(|| (waves.values().map(|s| s.len()).max().unwrap_or(1) as u32).max(1))
        .max(1);
    let mut injected = 0usize;
    for (wave_id, slots) in waves.iter_mut() {
        let kind = wave_kind.remove(wave_id).unwrap_or_else(|| {
            // Kind may be missing on a force-terminal-only call with
            // no new accepted events this iteration — infer Review
            // when any slot carries a dimension.
            if slots.iter().any(|(_, _, _, d)| d.is_some()) {
                WaveKind::Review
            } else {
                WaveKind::Exec
            }
        });
        let is_review = matches!(kind, WaveKind::Review);

        if ticked_waves.contains(wave_id) {
            continue;
        }

        let ready = slots.len() as u32 >= expected_slots;
        let force_now =
            force_terminal && !ready && (slots.len() as u32) >= min_slots_before_force.max(1);
        if !ready && !force_now {
            // Still collecting — register early so total is correct, but
            // do not tick until all slots arrive (or force_terminal).
            let _ = bridge.register_wave_if_absent(kind, wave_id, expected_slots, 1);
            for (slot_index, content_hash, event_count, dimension) in slots.iter() {
                if !is_review {
                    use ralph_core::supervisor::SlotResource;
                    let _ = bridge.store().bind_worktree(
                        &bridge
                            .register_wave_if_absent(kind, wave_id, expected_slots, 1)
                            .unwrap_or_else(|_| wave_id.clone()),
                        *slot_index,
                        SlotResource {
                            slot_index: *slot_index,
                            worktree_path: Some(format!(".ralph/bdd/{wave_id}/{slot_index}")),
                            branch: Some(format!("ralph/bdd/{wave_id}/{slot_index}")),
                        },
                    );
                    let _ = bridge.store().try_dispatch_next(64);
                }
                let store_id =
                    match bridge.register_wave_if_absent(kind, wave_id, expected_slots, 1) {
                        Ok(id) => id,
                        Err(_) => continue,
                    };
                let _ =
                    bridge.record_slot_result(&store_id, *slot_index, content_hash, *event_count);
                let evidence_topic = match kind {
                    WaveKind::Review => "review.unit.done",
                    WaveKind::Fix => "fix.unit.done",
                    WaveKind::Exec => "exec.unit.done",
                };
                let evidence_payload = match dimension {
                    Some(dim) => format!(
                        "{{\"slot_index\":{slot_index},\"dimension\":\"{dim}\",\"content_hash\":\"{content_hash}\"}}"
                    ),
                    None => format!(
                        "{{\"slot_index\":{slot_index},\"content_hash\":\"{content_hash}\",\"event_count\":{event_count}}}"
                    ),
                };
                let _ = bridge.store().record_slot_terminal_evidence(
                    &store_id,
                    *slot_index,
                    &TerminalEvidence::from_event(evidence_topic, &evidence_payload),
                );
            }
            continue;
        }

        let store_id = match bridge.register_wave_if_absent(kind, wave_id, expected_slots, 1) {
            Ok(id) => id,
            Err(err) => {
                eprintln!("[bdd-supervisor] register_wave_if_absent failed for {wave_id}: {err}");
                continue;
            }
        };

        let completed_dimensions: Vec<String> =
            slots.iter().filter_map(|(_, _, _, d)| d.clone()).collect();

        for (slot_index, content_hash, event_count, dimension) in slots.iter() {
            if !is_review {
                use ralph_core::supervisor::SlotResource;
                if let Err(err) = bridge.store().bind_worktree(
                    &store_id,
                    *slot_index,
                    SlotResource {
                        slot_index: *slot_index,
                        worktree_path: Some(format!(".ralph/bdd/{wave_id}/{slot_index}")),
                        branch: Some(format!("ralph/bdd/{wave_id}/{slot_index}")),
                    },
                ) {
                    eprintln!(
                        "[bdd-supervisor] bind_worktree failed for {wave_id}/{slot_index}: {err}"
                    );
                }
                if let Err(err) = bridge.store().try_dispatch_next(64) {
                    eprintln!("[bdd-supervisor] try_dispatch_next failed: {err}");
                }
            }

            if let Err(err) =
                bridge.record_slot_result(&store_id, *slot_index, content_hash, *event_count)
            {
                eprintln!(
                    "[bdd-supervisor] record_slot_result failed for {wave_id}/{slot_index}: {err}"
                );
            }
            let evidence_topic = match kind {
                WaveKind::Review => "review.unit.done",
                WaveKind::Fix => "fix.unit.done",
                WaveKind::Exec => "exec.unit.done",
            };
            let evidence_payload = match dimension {
                Some(dim) => format!(
                    "{{\"slot_index\":{slot_index},\"dimension\":\"{dim}\",\"content_hash\":\"{content_hash}\"}}"
                ),
                None => format!(
                    "{{\"slot_index\":{slot_index},\"content_hash\":\"{content_hash}\",\"event_count\":{event_count}}}"
                ),
            };
            if let Err(err) = bridge.store().record_slot_terminal_evidence(
                &store_id,
                *slot_index,
                &TerminalEvidence::from_event(evidence_topic, &evidence_payload),
            ) {
                eprintln!(
                    "[bdd-supervisor] record_slot_terminal_evidence failed for {wave_id}/{slot_index}: {err}"
                );
            }
        }

        let inputs = if force_now {
            if let Err(err) = bridge.cancel_wave(&store_id) {
                eprintln!("[bdd-supervisor] cancel_wave failed for {wave_id}: {err}");
            }
            if let Err(err) = bridge.record_never_started_failures(&store_id) {
                eprintln!(
                    "[bdd-supervisor] record_never_started_failures failed for {wave_id}: {err}"
                );
            }
            if let Err(err) = bridge.commit_salvage_projection(
                &store_id,
                &ralph_core::supervisor::ProjectionReceiptSummary {
                    kind: ralph_core::supervisor::ProjectionKind::Business,
                    batch_fingerprint: String::new(),
                    write_count: 0,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
            ) {
                eprintln!("[bdd-supervisor] mark_salvage_merged failed for {wave_id}: {err}");
            }
            PhaseInputs {
                aggregate_timeout_secs,
                elapsed_secs: aggregate_timeout_secs.saturating_add(1),
                cancel_requested: true,
            }
        } else {
            PhaseInputs {
                aggregate_timeout_secs,
                elapsed_secs: 0,
                cancel_requested: false,
            }
        };

        let action = match bridge.tick(&store_id, inputs) {
            Ok(a) => a,
            Err(err) => {
                eprintln!("[bdd-supervisor] tick failed for {wave_id}: {err}");
                continue;
            }
        };
        match action {
            CoordinatorAction::InjectedComplete { topic, .. } => {
                let payload = if is_review {
                    serde_json::json!({
                        "wave_id": wave_id,
                        "completed_dimensions": completed_dimensions,
                        "aggregate_timeout": aggregate_timeout_secs,
                    })
                } else {
                    serde_json::json!({
                        "wave_id": wave_id,
                        "completed_slots": slots.len(),
                        "success_slots": [],
                        "merge_root_event_id": format!("fan-in:{topic}:{wave_id}"),
                    })
                };
                bdd_append_supervisor_event(event_loop, &topic, &payload, "supervisor");
                let proto_event = ralph_proto::Event::new(topic.as_str(), payload.to_string())
                    .with_source(ralph_proto::HatId::new("supervisor"));
                event_loop.publish_event(proto_event.clone());
                event_loop.state_mut().record_event(&proto_event);
                injected += 1;
                ticked_waves.insert(wave_id.clone());
            }
            CoordinatorAction::InjectedFailed {
                topic,
                reason,
                blocking_slots,
            } => {
                let payload = if is_review {
                    // Prefer schema-required missing_dimensions for Review.
                    let known: std::collections::HashSet<&str> =
                        completed_dimensions.iter().map(|s| s.as_str()).collect();
                    let canonical = [
                        "goal-alignment",
                        "correctness",
                        "testing",
                        "maintainability",
                        "project-standards",
                        "adversarial",
                    ];
                    let missing: Vec<&str> = canonical
                        .iter()
                        .copied()
                        .filter(|d| !known.contains(d))
                        .collect();
                    serde_json::json!({
                        "wave_id": wave_id,
                        "missing_dimensions": missing,
                        "reason": reason,
                    })
                } else {
                    serde_json::json!({
                        "wave_id": wave_id,
                        "reason": reason,
                        "blocking_slots": blocking_slots,
                    })
                };
                bdd_append_supervisor_event(event_loop, &topic, &payload, "supervisor");
                let proto_event = ralph_proto::Event::new(topic.as_str(), payload.to_string())
                    .with_source(ralph_proto::HatId::new("supervisor"));
                event_loop.publish_event(proto_event.clone());
                event_loop.state_mut().record_event(&proto_event);
                injected += 1;
                ticked_waves.insert(wave_id.clone());
            }
            CoordinatorAction::AlreadyDone | CoordinatorAction::ContinueCollect => {}
            #[allow(clippy::match_wildcard_for_single_variants)]
            other => {
                eprintln!("[bdd-supervisor] unexpected action for {wave_id}: {other:?}");
            }
        }
    }

    injected
}

/// Runs a scenario through the real EventLoop and captures
/// per-iteration snapshots for post-loop assertions.
///
/// The caller supplies `extra_config` for scenario-specific
/// overrides (hat map, `event_loop` block, `core` block, etc.).
/// The baseline config sets `task_resume_ttl_seconds = Some(0)`
/// AFTER `extra_config` runs so scenario fixtures using a
/// hardcoded `2024-01-01T00:00:00Z` timestamp continue to pass
/// the freshness filter (2026-06-16-001 U3).
///
/// Returns the `TempDir` guard so callers that need to inspect
/// out-of-band artifacts (e.g. `projected-events.jsonl`) keep the
/// directory alive through their post-loop assertions.
/// U3 (plan 2026-08-03-004): build a real U1 `ResumeManifest` from the
/// fixture's `resume_bootstrap` block and convert it through the U2
/// `task_resume_from_manifest` path — the same chain the CLI runner
/// uses at a reused-worktree bootstrap. Panics (failing the scenario)
/// when the conversion rejects the fixture manifest.
fn build_manifest_resume_recovery(
    name: &str,
    bootstrap: &ResumeBootstrapYaml,
    registered_hats: &std::collections::BTreeSet<String>,
) -> ralph_core::event_loop::rejection::ManifestResumeRecovery {
    use ralph_core::parallel_forge_resume::{
        AcceptedBoundary, BoundaryRecord, MANIFEST_SCHEMA_VERSION, ResumeIdentity, ResumeManifest,
        TriggerSnapshot, WaveMetadata,
    };
    let accepted = bootstrap
        .accepted
        .iter()
        .enumerate()
        .map(|(idx, entry)| AcceptedBoundary {
            topic: entry.topic.clone(),
            transition_id: format!("bdd-resume-transition-{}", idx + 1),
            committed_at: format!("2026-08-03T00:00:{:02}Z", idx + 1),
            hat: entry.hat.clone(),
            in_event_log: true,
        })
        .collect();
    let mut manifest = ResumeManifest {
        schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
        captured_at: "2026-08-03T00:00:00Z".to_string(),
        identity: ResumeIdentity {
            plan_path: "docs/plans/bdd-resume.md".to_string(),
            plan_digest: "bdd-plan-digest".to_string(),
            preset_name: "parallel-forge".to_string(),
            config_digest: "bdd-config-digest".to_string(),
            worktree_name: "bdd-resume".to_string(),
            source_head_sha: String::new(),
            loop_id: "bdd-old-loop".to_string(),
        },
        boundary: BoundaryRecord {
            accepted,
            pending_hat: Some(bootstrap.pending_hat.clone()),
            original_trigger: Some(TriggerSnapshot {
                topic: bootstrap.original_trigger.topic.clone(),
                payload: Some(bootstrap.original_trigger.payload.clone()),
                hat: bootstrap.original_trigger.hat.clone(),
                triggered: Some(bootstrap.pending_hat.clone()),
                ts: "2026-08-03T00:00:59Z".to_string(),
            }),
            wave: bootstrap.wave.as_ref().map(|wave| WaveMetadata {
                wave_id: wave.wave_id.clone(),
                wave_index: wave.wave_index,
                wave_total: wave.wave_total,
            }),
        },
        tasks: Vec::new(),
        artifacts: Vec::new(),
        incomplete_reasons: Vec::new(),
        manifest_digest: String::new(),
    };
    manifest.finalize_digest();
    ralph_core::event_loop::rejection::task_resume_from_manifest(&manifest, registered_hats)
        .unwrap_or_else(|e| panic!("{name}: resume manifest conversion failed: {e}"))
}

fn run_scenario_with_snapshots(
    yaml: &ScenarioYaml,
    extra_config: impl FnOnce(&mut RalphConfig, &ScenarioYaml),
) -> tempfile::TempDir {
    use std::io::Write;
    let temp_dir = tempfile::tempdir().unwrap();
    let ralph_dir = temp_dir.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    let events_path = ralph_dir.join("events.jsonl");

    // 2026-06-18-002 plan U9: write fixture files (e.g. handoff
    // markdown files) to the temp workspace before the run. These
    // are scenario fixtures, not loop state.
    for fixture in &yaml.fixture_files {
        let abs = temp_dir.path().join(&fixture.path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&abs, &fixture.content).unwrap();
    }

    // Build RalphConfig from the YAML config section
    let mut config = RalphConfig::default();
    config.max_iterations = Some(yaml.config.max_iterations);
    config.prompt_file = Some(yaml.config.prompt_file.clone());
    // Pin the workspace to the temp dir so the projector and
    // the event reader resolve `.ralph/...` from there. Without
    // this the projector would point at the cwd of the test
    // runner and the scenario would silently no-op.
    config.core.workspace_root = temp_dir.path().to_path_buf();

    // Apply caller-specific config overrides (hats, event_loop, core, ...).
    extra_config(&mut config, yaml);

    // 2026-06-16-001 U3: scenario fixtures use a hardcoded
    // `2024-01-01T00:00:00Z` timestamp, which is older than the
    // default 300s TTL. Disable the freshness filter so the
    // fixtures continue to exercise the workflow-guard path without
    // being classified as stale rejections. The U3 TTL behavior
    // is covered by `event_loop/tests/task_resume_ttl.rs`. Applied
    // AFTER `extra_config` so a caller's `event_loop` parse wins
    // on every other field; only `task_resume_ttl_seconds` is
    // force-disabled.
    config.event_loop.task_resume_ttl_seconds = Some(0);

    // Re-pin the workspace to the helper's tempdir AFTER
    // `extra_config`. Some scenarios (e.g.
    // `isolated_with_event_projection`) overwrite `config.core`
    // wholesale from a YAML block; that overwrite nulls the
    // `workspace_root` we just set, so we re-pin here. The
    // helper owns the tempdir, so its path is the single
    // source of truth for `workspace_root` regardless of
    // what callers did in `extra_config`.
    config.core.workspace_root = temp_dir.path().to_path_buf();

    // 2026-07-03-001 Phase 6 BDD realization: snapshot the
    // supervisor config BEFORE `config` is moved into
    // `EventLoop::with_context`. When the scenario opts in
    // (`supervisor.enabled: true` + `execution_mode: isolated`),
    // we construct an `InMemoryCoordinatorBridge` and drive the
    // coordinator `tick` from `run_bdd_supervisor_fan_in` after
    // every `process_events_from_jsonl` pass. The bridge owns an
    // in-memory store so the BDD path does not depend on the
    // `supervisor-db` cargo feature.
    let supervisor_path_enabled = {
        use ralph_core::config::HatExecutionMode;
        ralph_core::supervisor::is_supervisor_path_enabled(
            config.event_loop.supervisor.enabled,
            matches!(config.event_loop.execution_mode, HatExecutionMode::Isolated),
        )
    };
    let supervisor_aggregate_timeout_secs = config.event_loop.supervisor.aggregate_timeout_secs;
    let supervisor_bridge: Option<InMemoryCoordinatorBridge> = if supervisor_path_enabled {
        let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());
        Some(InMemoryCoordinatorBridge::from_store(store))
    } else {
        None
    };

    let context = LoopContext::primary(temp_dir.path().to_path_buf());

    // U3 (plan 2026-08-03-004): capture the registered hat ids BEFORE
    // `config` moves into the EventLoop — the manifest → task.resume
    // conversion validates the pending hat against this set, exactly
    // like the CLI runner does at bootstrap.
    let registered_hats: std::collections::BTreeSet<String> = config.hats.keys().cloned().collect();

    let mut event_loop = if yaml.compiled_contract {
        let resolved = ralph_core::execution_contract::compile(config)
            .unwrap_or_else(|error| panic!("{}: contract compile failed: {error}", yaml.name));
        EventLoop::from_resolved(resolved, context)
    } else {
        EventLoop::with_context(config, context)
    };
    if let Some(ref bootstrap) = yaml.resume_bootstrap {
        // Resume bootstrap: the old run's runtime state is gone; only
        // the manifest boundary survives. Boot through the real U1/U2
        // conversion chain instead of the configured starting event.
        let recovery = build_manifest_resume_recovery(&yaml.name, bootstrap, &registered_hats);
        for _ in 0..bootstrap.repeat.max(1) {
            event_loop.initialize_manifest_resume("Test", recovery.clone());
        }
    } else {
        event_loop.initialize("Test");
    }

    let parser = EventParser::new();

    // 2026-06-18-002 plan U8 (KTD-17): capture the **last prompt**
    // each hat saw during the run. Stored by hat id so the
    // `prompt_contains` assertions below can look up the right
    // entry. Only the last prompt per hat is retained because the
    // prompt grows monotonically and the asserts want the most
    // representative state.
    let mut last_prompts: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    // 2026-06-20-002 plan U2 (R-H2): record per-iteration
    // snapshots of `LoopState` (read-only clone) and the most
    // recent `build_prompt` output. Snapshots are pushed **after
    // process_events_from_jsonl** so the state captured reflects
    // the iteration's terminal position (i.e. after rejection
    // digest accumulation and any consume-on-use of
    // `pending_lint_resume`). The runner itself never mutates
    // state from `assert_state` (R-H4); snapshotting is the only
    // borrow of `state()` we need outside the iteration loop.
    let mut state_snapshots: Vec<LoopStateSnapshot> = Vec::with_capacity(yaml.mock_responses.len());
    let mut prompt_snapshots: Vec<BuildPromptSnapshot> = Vec::new();
    // 2026-06-20-002 plan U2: holds the (hat, prompt) pair from
    // the most-recent `build_prompt` in the current iteration,
    // drained into `prompt_snapshots` at iteration end. Stays in
    // sync with `last_prompts` (hat-scoped last-only map) but
    // preserves the per-iteration pairing `assert_state` needs.
    let mut last_prompt_for_iter: Option<(String, String)> = None;
    let mut accepted_topic_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut accepted_payloads: std::collections::HashMap<String, Vec<serde_json::Value>> =
        std::collections::HashMap::new();

    // 2026-07-27-001 plan U2: persistent slot accumulator for supervisor fan-in.
    // Slots arrive across multiple iterations (one slot per review-worker activation).
    // The map must persist across calls to `run_bdd_supervisor_fan_in`, so we
    // create it here and pass it as a mutable reference.
    let mut bdd_waves: std::collections::HashMap<
        String,
        Vec<(u32, String, usize, Option<String>)>,
    > = std::collections::HashMap::new();
    let mut bdd_ticked_waves: std::collections::HashSet<String> = std::collections::HashSet::new();
    let bdd_expected_slots = yaml
        .supervisor_fan_in
        .as_ref()
        .and_then(|s| s.expected_slots);
    let bdd_force_terminal = yaml
        .supervisor_fan_in
        .as_ref()
        .and_then(|s| s.force_terminal.as_deref())
        .is_some_and(|v| v == "timeout" || v == "partial");
    let bdd_min_slots_before_force = yaml
        .supervisor_fan_in
        .as_ref()
        .and_then(|s| s.min_slots_before_force)
        .unwrap_or(1);

    for (idx, response) in yaml.mock_responses.iter().enumerate() {
        // Simulate hat execution so isolated mode scope enforcement is active.
        // build_prompt() consumes pending events from the bus, matching real loop behavior.
        if let Some(hat) = event_loop.next_hat() {
            let hat = hat.clone();
            let prompt = event_loop.build_prompt(&hat);
            if let Some(p) = prompt {
                // 2026-06-20-002 plan U2: also stash the
                // most-recent prompt for `assert_state.prompt_injects`
                // (per-iteration) before we overwrite the
                // hat-scoped `last_prompts` map. Note the move of
                // `p` here is safe because `last_prompts` only
                // keeps the last value per hat.
                last_prompt_for_iter = Some((hat.to_string(), p.clone()));
                last_prompts.insert(hat.to_string(), p);
            }
            let _ = event_loop.process_output(&hat, "", true);
        }

        let events = parser.parse(&response.text);
        {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)
                .unwrap();
            for event in &events {
                let mut entry = serde_json::json!({
                    "topic": event.topic,
                    "payload": event.payload,
                    "ts": "2024-01-01T00:00:00Z",
                });
                if let Some(ref hat) = response.hat {
                    entry["hat"] = serde_json::Value::String(hat.clone());
                }
                writeln!(file, "{}", entry).unwrap();
            }
        }

        let result = event_loop.process_events_from_jsonl().unwrap();
        for e in &result.accepted_events {
            *accepted_topic_counts
                .entry(e.topic.to_string())
                .or_insert(0) += 1;
            accepted_payloads
                .entry(e.topic.to_string())
                .or_default()
                .push(
                    serde_json::from_str(&e.payload)
                        .unwrap_or_else(|_| serde_json::Value::String(e.payload.clone())),
                );
        }

        // 2026-07-03-001 Phase 6 BDD realization: when the
        // scenario opts into the supervisor path, drive the
        // coordinator `tick` here so `*.wave.complete` /
        // `*.wave.failed` events are produced by the real
        // `SupervisorCoordinator` (not faked via mock YAML
        // responses). The fan-in helper writes the injected
        // event to JSONL for audit, publishes it to the bus,
        // and records it in `seen_topics` so the scenario's
        // `expected.events` assertion sees it without relying
        // on a second `process_events_from_jsonl` pass.
        if let Some(ref bridge) = supervisor_bridge {
            let _ = run_bdd_supervisor_fan_in(
                &mut event_loop,
                bridge,
                &result.accepted_events,
                supervisor_aggregate_timeout_secs,
                bdd_expected_slots,
                bdd_force_terminal,
                bdd_min_slots_before_force,
                &mut bdd_waves,
                &mut bdd_ticked_waves,
            );
        }

        // Evaluate checkpoints tied to this response index (1-based in YAML)
        let mut sleep_after_response = 0u64;
        for checkpoint in &yaml.checkpoints {
            if checkpoint.after_response == idx + 1 {
                for progress in &checkpoint.workflow_progress {
                    let instance = progress.instance.as_deref();
                    let actual_phase = event_loop
                        .state()
                        .workflow_progress
                        .get_phase(&progress.chain, instance);
                    assert_eq!(
                        actual_phase,
                        Some(progress.phase),
                        "{}: After response {}, expected workflow progress phase {} for chain '{}', got {:?}",
                        yaml.name,
                        idx + 1,
                        progress.phase,
                        progress.chain,
                        actual_phase
                    );
                }

                if checkpoint.completion_rejected {
                    let honored_before = event_loop.state().completion_honored;
                    let reason = event_loop.check_completion_event();
                    // 2026-06-26 plan U6: verdict_fail is a
                    // structural rejection — the loop returns
                    // `Some(TerminationReason::CompletionStuck(
                    //   source: StructuralRejection, ...))`
                    // on the first attempt instead of
                    // suppressing completion silently. The BDD
                    // contract here is "LOOP_COMPLETE is not
                    // honoured as `CompletionPromise`" — both
                    // `None` (gate open, no termination yet)
                    // and a structural `CompletionStuck` satisfy
                    // it. Recoverable rejection is still
                    // `None` (correction block queued).
                    //
                    // When completion was already honored on a
                    // prior response, `check_completion_event`
                    // is idempotent and returns
                    // `CompletionPromise` again — that still
                    // satisfies "this response did not newly
                    // honor a rejected duplicate".
                    let newly_honoured =
                        matches!(reason, Some(TerminationReason::CompletionPromise))
                            && !honored_before;
                    assert!(
                        !newly_honoured,
                        "{}: After response {}, expected LOOP_COMPLETE to be rejected, but got {:?}",
                        yaml.name,
                        idx + 1,
                        reason
                    );
                }

                if checkpoint.honor_completion {
                    let reason = event_loop.check_completion_event();
                    assert!(
                        matches!(reason, Some(TerminationReason::CompletionPromise)),
                        "{}: After response {}, expected LOOP_COMPLETE to be honored, but got {:?}",
                        yaml.name,
                        idx + 1,
                        reason
                    );
                }

                if checkpoint.sleep_ms > sleep_after_response {
                    sleep_after_response = checkpoint.sleep_ms;
                }
            }
        }

        if sleep_after_response > 0 {
            std::thread::sleep(std::time::Duration::from_millis(sleep_after_response));
        }

        // 2026-06-20-002 plan U2 (R-H2): snapshot LoopState +
        // BuildPrompt at the end of each iteration. Pushed AFTER
        // process_events_from_jsonl so the captured state is the
        // iteration's terminal position (post-rejection-digest
        // accumulation and post-consume-on-use of
        // pending_lint_resume). Pushed BEFORE the iteration's
        // checkpoint assertions so a checkpoint failure does not
        // produce a half-built snapshot list.
        //
        // 2026-06-21-002 plan U9: pass the workspace root so
        // the snapshot can resolve `.ralph/recovery.jsonl` for
        // the `rejection_log_contains_reason_code` predicate.
        let snapshot =
            capture_state_snapshot(event_loop.state(), Some(temp_dir.path().to_path_buf()));
        state_snapshots.push(snapshot);
        if let Some((hat, prompt)) = last_prompt_for_iter.take() {
            prompt_snapshots.push(BuildPromptSnapshot {
                iteration: (idx + 1) as u32,
                hat,
                prompt,
            });
        } else {
            // No prompt was built this iteration (no hat
            // activated); still push a marker so `at_iteration`
            // indexing lines up 1:1 with the iteration count for
            // scenarios that need to assert "no prompt was built
            // at iteration N".
            prompt_snapshots.push(BuildPromptSnapshot {
                iteration: (idx + 1) as u32,
                hat: String::new(),
                prompt: String::new(),
            });
        }
    }

    // Verify all expected events were seen (accepted) at least once
    for expected_event in &yaml.expected.events {
        let seen = event_loop.state().seen_topics.clone();
        assert!(
            seen.contains(&expected_event.topic),
            "{}: Expected event '{}' to be seen (accepted), but it was not recorded. seen_topics: {:?}",
            yaml.name,
            expected_event.topic,
            seen
        );
    }

    // 2026-06-20-002 plan U1 (R-H1): evaluate the optional
    // `assert_state` list. Each entry picks one snapshot by
    // `at_iteration` (1-indexed) and runs a single predicate.
    // Order matches YAML; failures include the iteration index,
    // the predicate name, and the actual snapshot state so the
    // developer can locate the bug without re-running.
    evaluate_assert_state(
        &yaml.name,
        &yaml.expected.assert_state,
        &state_snapshots,
        &prompt_snapshots,
    );

    // Verify explicitly absent events were NOT accepted by the event loop
    for absent_event in &yaml.expected.absent_events {
        assert!(
            !event_loop.state().seen_topics.contains(&absent_event.topic),
            "{}: Expected event '{}' to be rejected/dropped, but it was accepted",
            yaml.name,
            absent_event.topic
        );
    }

    // P1-2 (2026-06-28 review): the wire-level assertion
    // for emit-time gate failures is checked via the
    // recovery envelope channel. The runner writes a
    // session-level `recovery.jsonl` whenever the stage
    // pipeline records a rejection; the assertion here
    // grep's for substrings so the test fails when the
    // gate was bypassed (no envelope written).
    //
    // When diagnostics are disabled the collector has no
    // session directory and the envelope is silently
    // discarded. The assertion is skipped in that case
    // because the test proves the gate fired at the pure
    // logic level (iterations + completion assertions
    // already cover that) and the `recovery_contains`
    // list is a regression guard for the wire-level path.
    if !yaml.expected.recovery_contains.is_empty() {
        let session_dir = event_loop.diagnostics().session_dir();
        if let Some(session_dir) = session_dir {
            let recovery_path = session_dir.join("recovery.jsonl");
            let body = std::fs::read_to_string(&recovery_path).unwrap_or_else(|_| {
                panic!(
                    "{}: recovery.jsonl missing at {} — gate never fired?",
                    yaml.name,
                    recovery_path.display()
                )
            });
            for needle in &yaml.expected.recovery_contains {
                assert!(
                    body.contains(needle.as_str()),
                    "{}: recovery.jsonl missing substring `{}`. Body:\n{}",
                    yaml.name,
                    needle,
                    body
                );
            }
        } else {
            // Diagnostics disabled: skip the wire-level
            // assertion. The test still passes because
            // `iterations` + `completion` + the stage
            // unit tests already prove the gate fired.
            // The recovery_contains list serves as
            // documentation + regression guard for the
            // diagnostic-enabled path.
            tracing::debug!(
                "{}: recovery_contains assertion skipped (diagnostics disabled)",
                yaml.name
            );
        }
    }

    // 2026-06-18-002 plan U8 (KTD-17): assert `prompt_contains` per
    // hat. Each entry's substrings must appear in the **last** prompt
    // captured for that hat. Skip silently when the hat was never
    // activated (no entry in `last_prompts`); this keeps scenarios
    // that don't exercise the prompt path passing without forcing
    // every hat to be reached.
    for pc in &yaml.expected.prompt_contains {
        let Some(prompt) = last_prompts.get(&pc.hat) else {
            continue;
        };
        for needle in &pc.substrings {
            assert!(
                prompt.contains(needle.as_str()),
                "{}: prompt for hat `{}` is missing substring `{}`\n--- prompt (first 800 chars) ---\n{}\n---",
                yaml.name,
                pc.hat,
                needle,
                &prompt[..prompt.len().min(800)],
            );
        }
    }

    // U3 (plan 2026-08-03-004): assert the exact ordered activation
    // sequence (hats that had a prompt built). Iterations without an
    // activation are skipped. Pins resume routing: the pending hat
    // activates first, upstream hats do not re-activate, and a
    // repeated bootstrap never duplicates activations.
    if !yaml.expected.activation_hats.is_empty() {
        let actual: Vec<String> = prompt_snapshots
            .iter()
            .filter(|snap| !snap.hat.is_empty())
            .map(|snap| snap.hat.clone())
            .collect();
        assert_eq!(
            actual, yaml.expected.activation_hats,
            "{}: activation hat sequence mismatch (resume routing)",
            yaml.name
        );
    }

    // Verify final workflow progress
    for progress in &yaml.expected.workflow_progress {
        let instance = progress.instance.as_deref();
        let actual_phase = event_loop
            .state()
            .workflow_progress
            .get_phase(&progress.chain, instance);
        assert_eq!(
            actual_phase,
            Some(progress.phase),
            "{}: Expected final workflow progress phase {} for chain '{}', got {:?}",
            yaml.name,
            progress.phase,
            progress.chain,
            actual_phase
        );
    }

    // Verify completion behavior
    if yaml.expected.completion {
        let reason = event_loop.check_completion_event();
        assert!(
            reason.is_some(),
            "{}: Expected LOOP_COMPLETE to be accepted, but it was rejected or not present",
            yaml.name
        );
    } else {
        let reason = event_loop.check_completion_event();
        assert!(
            reason.is_none(),
            "{}: Expected LOOP_COMPLETE to be rejected, but got {:?}",
            yaml.name,
            reason
        );
    }

    for etc in &yaml.expected.event_topic_counts {
        let actual = accepted_topic_counts.get(&etc.topic).copied().unwrap_or(0);
        assert_eq!(
            actual, etc.count,
            "{}: Expected topic '{}' to be accepted {} time(s), got {} (all counts: {:?})",
            yaml.name, etc.topic, etc.count, actual, accepted_topic_counts
        );
    }

    for payload_match in &yaml.expected.payload_matches {
        let occurrence = payload_match.occurrence.unwrap_or(1);
        assert!(
            occurrence > 0,
            "{}: payload_matches occurrence for '{}' must be 1-based",
            yaml.name,
            payload_match.topic
        );
        let payloads = accepted_payloads
            .get(&payload_match.topic)
            .unwrap_or_else(|| {
                panic!(
                    "{}: Expected accepted payload for topic '{}', got topics {:?}",
                    yaml.name,
                    payload_match.topic,
                    accepted_payloads.keys().collect::<Vec<_>>()
                )
            });
        let payload = payloads.get(occurrence - 1).unwrap_or_else(|| {
            panic!(
                "{}: Expected occurrence {} for topic '{}', got {} occurrence(s)",
                yaml.name,
                occurrence,
                payload_match.topic,
                payloads.len()
            )
        });
        for (field, expected_value) in &payload_match.fields {
            let actual = payload.get(field).unwrap_or_else(|| {
                panic!(
                    "{}: Payload for topic '{}' occurrence {} missing field '{}': {}",
                    yaml.name, payload_match.topic, occurrence, field, payload
                )
            });
            assert_eq!(
                actual, expected_value,
                "{}: Payload field '{}.{}' occurrence {} mismatch",
                yaml.name, payload_match.topic, field, occurrence
            );
        }
    }

    // Verify iteration count matches the number of mock responses
    assert_eq!(
        yaml.mock_responses.len(),
        yaml.expected.iterations,
        "{}: Expected {} iterations, but scenario has {} mock responses",
        yaml.name,
        yaml.expected.iterations,
        yaml.mock_responses.len()
    );

    // Plan 2026-07-28-001 U1/U2/U3 task-to-wave assertions: read the
    // live task ledger + supervisor store directly from disk /
    // in-memory bridge so the scenario proves the production
    // TaskStore / SupervisorCoordinator observed the same state the
    // dispatcher did. Mocks are not on the path — the bridge was
    // wired in early in this function when `supervisor_fan_in` was
    // opted in.
    if !yaml.expected.task_ledger.is_empty()
        || !yaml.expected.payload_task_refs.is_empty()
        || !yaml.expected.supervisor_waves.is_empty()
        || !yaml.expected.ready_task_keys.is_empty()
    {
        assert_task_ledger_and_waves(
            &yaml.name,
            &yaml.expected.task_ledger,
            &yaml.expected.payload_task_refs,
            &yaml.expected.supervisor_waves,
            &yaml.expected.ready_task_keys,
            supervisor_bridge.as_ref(),
            temp_dir.path(),
            &accepted_payloads,
        );
    }

    println!("✓ {} passed", yaml.description);

    temp_dir
}

/// Plan 2026-07-28-001 U1/U2/U3: reload the task store + supervisor
/// bridge from the scenario temp workspace and assert the live
/// state matches the declared task DAG, payload references, and
/// wave fan-in. Used by the new BDD fixtures
/// `parallel_forge_task_dispatch_runtime.yml` and
/// `parallel_forge_duplicate_handoff_runtime.yml` so the
/// fixtures prove the runtime path — not stub-and-pray.
fn assert_task_ledger_and_waves(
    name: &str,
    ledger: &[TaskLedgerRowYaml],
    payload_refs: &[PayloadTaskRefYaml],
    waves: &[SupervisorWaveYaml],
    ready_keys: &[String],
    bridge: Option<&InMemoryCoordinatorBridge>,
    workspace: &std::path::Path,
    accepted_payloads: &std::collections::HashMap<String, Vec<serde_json::Value>>,
) {
    use ralph_core::task_store::TaskStore;
    // TaskStore writes to `<workspace>/.ralph/agent/tasks.jsonl`.
    let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
    if !ledger.is_empty() || !ready_keys.is_empty() || !payload_refs.is_empty() {
        let store = TaskStore::load(&tasks_path).unwrap_or_else(|err| {
            panic!(
                "{name}: failed to reload task ledger at {}: {err}",
                tasks_path.display()
            )
        });
        let all = store.all();
        let mut key_to_id: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut id_to_key: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for task in all {
            let Some(key) = task.key.as_ref() else {
                continue;
            };
            key_to_id.insert(key.clone(), task.id.clone());
            id_to_key.insert(task.id.clone(), key.clone());
        }
        for row in ledger {
            let id = key_to_id.get(&row.task_key).unwrap_or_else(|| {
                panic!(
                    "{name}: task_ledger row for key `{}` not found in ledger (keys: {:?})",
                    row.task_key,
                    key_to_id.keys().collect::<Vec<_>>()
                )
            });
            let task = all.iter().find(|t| &t.id == id).expect("task row");
            let actual_status = match task.status {
                ralph_core::task::TaskStatus::Open => "open",
                ralph_core::task::TaskStatus::InProgress => "in_progress",
                ralph_core::task::TaskStatus::Closed => "closed",
                ralph_core::task::TaskStatus::Failed => "failed",
            };
            assert_eq!(
                actual_status, row.status,
                "{name}: task_ledger row `{}` expected status `{}`, got `{}`",
                row.task_key, row.status, actual_status
            );
            let actual_blocker_keys: std::collections::BTreeSet<String> = task
                .blocked_by
                .iter()
                .filter_map(|bid| id_to_key.get(bid).cloned())
                .collect();
            let expected_blocker_keys: std::collections::BTreeSet<String> =
                row.blocked_by_keys.iter().cloned().collect();
            assert_eq!(
                actual_blocker_keys, expected_blocker_keys,
                "{name}: task_ledger row `{}` blocker-key set mismatch",
                row.task_key
            );
        }
        if !ready_keys.is_empty() {
            let actual: std::collections::BTreeSet<String> = all
                .iter()
                .filter(|t| {
                    t.status == ralph_core::task::TaskStatus::Open && t.blocked_by.is_empty()
                })
                .filter_map(|t| t.key.clone())
                .collect();
            let expected: std::collections::BTreeSet<String> = ready_keys.iter().cloned().collect();
            assert_eq!(
                actual, expected,
                "{name}: ready_task_keys mismatch (actual vs expected)"
            );
        }
        for pref in payload_refs {
            assert!(
                pref.occurrence > 0,
                "{name}: payload_task_refs occurrence must be 1-based"
            );
            let payloads = accepted_payloads.get(&pref.topic).unwrap_or_else(|| {
                panic!(
                    "{name}: payload_task_refs topic `{}` never accepted",
                    pref.topic
                )
            });
            let payload = payloads.get(pref.occurrence - 1).unwrap_or_else(|| {
                panic!(
                    "{name}: payload_task_refs occurrence {} of topic `{}` not present",
                    pref.occurrence, pref.topic
                )
            });
            let actual = payload.get(&pref.payload_field).unwrap_or_else(|| {
                panic!(
                    "{name}: payload_task_refs payload field `{}.{}` missing from accepted payload {}",
                    pref.topic, pref.payload_field, payload
                )
            });
            let expected_id = key_to_id.get(&pref.task_key).unwrap_or_else(|| {
                panic!(
                    "{name}: payload_task_refs task_key `{}` not in ledger",
                    pref.task_key
                )
            });
            assert_eq!(
                actual.as_str().map(|s| s.to_string()),
                Some(expected_id.clone()),
                "{name}: payload_task_refs `{}.{}` (occurrence {}) must resolve to live task id `{}`",
                pref.topic,
                pref.payload_field,
                pref.occurrence,
                expected_id
            );
        }
    }
    if !waves.is_empty() {
        let Some(bridge) = bridge else {
            panic!(
                "{name}: supervisor_waves were declared but the scenario did not opt into the live supervisor bridge"
            );
        };
        let store = bridge.store();
        let actual_wave_ids = store
            .list_wave_ids()
            .unwrap_or_else(|err| panic!("{name}: failed to list live supervisor waves: {err}"));
        let expected_wave_ids: std::collections::BTreeSet<String> =
            waves.iter().map(|wave| wave.wave_id.clone()).collect();
        let actual_wave_ids_set: std::collections::BTreeSet<String> =
            actual_wave_ids.iter().cloned().collect();
        assert_eq!(
            actual_wave_ids_set, expected_wave_ids,
            "{name}: live supervisor wave ids mismatch"
        );
        for wave in waves {
            let kind = match wave.kind.as_str() {
                "exec" | "execution" => WaveKind::Exec,
                "review" => WaveKind::Review,
                "fix" => WaveKind::Fix,
                other => panic!(
                    "{name}: supervisor_waves `{}` uses unsupported kind `{}`",
                    wave.wave_id, other
                ),
            };
            match store.fan_in_status(&wave.wave_id) {
                Ok(snap) => {
                    assert_eq!(
                        snap.kind, kind,
                        "{name}: supervisor_waves `{}` kind mismatch",
                        wave.wave_id
                    );
                    assert_eq!(
                        snap.expected_total, wave.expected_total,
                        "{name}: supervisor_waves `{}` expected_total mismatch",
                        wave.wave_id
                    );
                    assert_eq!(
                        snap.completed_count, wave.completed_count,
                        "{name}: supervisor_waves `{}` completed_count mismatch",
                        wave.wave_id
                    );
                    assert_eq!(
                        snap.failed_count, wave.failed_count,
                        "{name}: supervisor_waves `{}` failed_count mismatch",
                        wave.wave_id
                    );
                    assert_eq!(
                        snap.phase.to_string(),
                        wave.phase,
                        "{name}: supervisor_waves `{}` phase mismatch",
                        wave.wave_id
                    );
                }
                Err(_) => panic!(
                    "{name}: supervisor_waves expected wave `{}` registered",
                    wave.wave_id
                ),
            }
        }
    }
}

/// Apply the per-scenario YAML `config.hats` block to a
/// `RalphConfig`, injecting the map key as the hat `name` when
/// the inline entry omits one. (2026-06-20-002 plan U3/Q-3:
/// extracted from the two near-duplicate wrappers.)
fn apply_yaml_hats(yaml: &ScenarioYaml, config: &mut RalphConfig) {
    if yaml.config.hats.is_null() {
        return;
    }
    let hat_map: std::collections::HashMap<String, serde_yaml::Value> =
        match serde_yaml::from_value(yaml.config.hats.clone()) {
            Ok(m) => m,
            Err(_) => return,
        };
    let mut hats = std::collections::HashMap::new();
    for (hat_id, mut hat_value) in hat_map {
        if let serde_yaml::Value::Mapping(ref mut map) = hat_value
            && !map.contains_key(serde_yaml::Value::String("name".to_string()))
        {
            map.insert(
                serde_yaml::Value::String("name".to_string()),
                serde_yaml::Value::String(hat_id.clone()),
            );
        }
        let hat_config: HatConfig = serde_yaml::from_value(hat_value)
            .unwrap_or_else(|e| panic!("Failed to parse hat config for '{}': {}", hat_id, e));
        hats.insert(hat_id, hat_config);
    }
    config.hats = hats;
}

fn run_scenario(yaml: ScenarioYaml) {
    let backend = MockBackend::new(yaml.mock_responses.iter().map(|r| r.text.clone()).collect());
    let runner = ScenarioRunner::new(backend.clone());

    let mut config = RalphConfig::default();
    config.max_iterations = Some(yaml.config.max_iterations);
    config.prompt_file = Some(yaml.config.prompt_file);

    let scenario =
        Scenario::new(yaml.name.clone(), config).with_iterations(yaml.expected.iterations);

    let trace = runner.run(&scenario);

    // Verify iteration count
    assert_eq!(
        trace.iterations, yaml.expected.iterations,
        "{}: Expected {} iterations, got {}",
        yaml.name, yaml.expected.iterations, trace.iterations
    );

    // Verify backend was called
    assert!(
        backend.execution_count() > 0,
        "{}: Backend should have been called",
        yaml.name
    );

    println!("✓ {} passed", yaml.description);
}

/// Runs a scenario that validates workflow guard behavior by feeding parsed
/// events through a real EventLoop and asserting on workflow progress.
/// (2026-06-20-002 plan U3/Q-3: thin wrapper around the shared
/// `run_scenario_with_snapshots` helper; previously duplicated
/// the entire runner body.)
fn run_workflow_guard_scenario(yaml: ScenarioYaml) {
    run_scenario_with_snapshots(&yaml, |config, yaml| {
        apply_yaml_hats(yaml, config);
        if !yaml.config.mechanism.is_null() {
            config.mechanism = serde_yaml::from_value(yaml.config.mechanism.clone())
                .unwrap_or_else(|e| {
                    panic!("{}: failed to parse config.mechanism: {e}", yaml.name);
                });
        }
        if !yaml.config.event_loop.is_null() {
            config.event_loop = serde_yaml::from_value(yaml.config.event_loop.clone()).unwrap();
        }
        // 2026-07-02-004: preset_lint and runtime both operate on the
        // desugared graph (synthetic gate hats + `.proposed` rewrites).
        config.normalize();
    });
}

#[test]
fn test_solo_mode() {
    let yaml = load_scenario("tests/scenarios/solo_mode.yml");
    run_scenario(yaml);
}

#[test]
fn test_multi_hat() {
    let yaml = load_scenario("tests/scenarios/multi_hat.yml");
    run_scenario(yaml);
}

#[test]
fn test_orphaned_events() {
    let yaml = load_scenario("tests/scenarios/orphaned_events.yml");
    run_scenario(yaml);
}

#[test]
fn test_default_publishes() {
    let yaml = load_scenario("tests/scenarios/default_publishes.yml");
    run_scenario(yaml);
}

#[test]
fn test_autoresearch_guard() {
    let yaml = load_scenario("tests/scenarios/autoresearch_guard.yml");
    run_workflow_guard_scenario(yaml);
}

// BDD scenario for feat-ralph-cli-agent-reference-split has been removed.
// Real CLI acceptance is covered by integration tests in
// crates/ralph-cli/tests/integration_agent_reference.rs.

#[test]
fn test_isolated_multi_hat() {
    let yaml = load_scenario("tests/scenarios/isolated_multi_hat.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_isolated_boundary_violation() {
    let yaml = load_scenario("tests/scenarios/isolated_boundary_violation.yml");
    run_workflow_guard_scenario(yaml);
}

/// Plan 2026-07-28-001 U2 (R1/R4/R5/S1/S8/S12): parallel-forge
/// task-to-wave happy path — `forge.plan.ready` atomically
/// materialises a DAG, the dispatcher pulls live ids from the
/// TaskStore, the supervisor fan-in closes U1 then opens U2, and
/// the loop completes. Drives the REAL TaskStore + InMemory
/// supervisor bridge (not a mock faked system event).
#[test]
fn test_parallel_forge_task_dispatch_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_task_dispatch_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

/// Plan 2026-07-28-001 U3 (S4 / S8): duplicate-fanout guard.
/// Worktree emits two `forge.worktrees.ready` in one isolated
/// activation. The harness asserts that exactly one of them
/// reaches the bus (`event_topic_counts: forge.worktrees.ready =
/// 1`), the second fires a boundary diagnostic, and zero
/// hat-targeted `task.resume` injections reach the worktree.
#[test]
fn test_parallel_forge_duplicate_handoff_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_duplicate_handoff_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

// Plan 2026-07-29-005 U7 / S6 (G11): two-wave happy path.
// Walks the per-wave settlement chain
// (forge.wave.reviewed → forge.wave.integrated →
// forge.wave.verified → forge.wave.settled) for two waves.
// Asserts each topic appears exactly twice and the second
// wave's `forge.wave.worktrees.ready.verified_base_commit`
// matches the first wave's `forge.wave.verified.candidate
// _commit_sha`.
#[test]
fn test_parallel_forge_two_wave_settlement_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_two_wave_settlement_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

// Plan 2026-07-29-005 U7 / S7 (G11): slot fail routes through
// forge.wave.review.failed → forge.correction.{requested,done}
// → re-review → settle, NOT work.failed. Asserts no
// `work.failed` event ever appears and the run terminates with
// `LOOP_COMPLETE` after correction.
#[test]
fn test_parallel_forge_correction_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_correction_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

/// Plan 2026-07-29-005 U7 / S11 (#2): `forge.final.correction.settled`
/// must only be accepted at `correction_round: 3` (the budget-exhausted
/// final round). The runtime gate is the schema's `allowed_values: {3}`
/// — payload_consistency cannot express `<3 reject` (no lt/not in the
/// predicate whitelist). This BDD exercises both sides of the gate
/// through the real EventLoop runner.
#[test]
fn test_parallel_forge_round_exhaustion_gate_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_round_exhaustion_gate_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-30-002 plan U1 (R3/S3): parallel-forge fail-close BDD.
/// After the planning chain lands and `forge.exec.development.done`
/// advances to `development_loop`, three empty turns trip the
/// fail-close detector. The runtime must (a) publish the
/// preset-derived blocked topic `forge.plan.blocked` (not the
/// legacy `plan.blocked`), (b) advance `current_plan_step` to
/// `report` and append a flow-authority snapshot, so (c) the
/// reporter's `forge.report.done` clears FlowStepScope and the
/// loop closes via `LOOP_COMPLETE`.
///
/// This test calls `run_scenario_with_snapshots` directly (not
/// the `run_workflow_guard_scenario` wrapper) so it can read
/// `.ralph/flow-authority.jsonl` post-loop and assert the
/// escape snapshot was written.
#[test]
fn test_parallel_forge_fail_close_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_fail_close_runtime.yml");
    let temp_dir = run_scenario_with_snapshots(&yaml, |config, yaml| {
        apply_yaml_hats(yaml, config);
        if !yaml.config.mechanism.is_null() {
            config.mechanism = serde_yaml::from_value(yaml.config.mechanism.clone())
                .unwrap_or_else(|e| {
                    panic!("{}: failed to parse config.mechanism: {e}", yaml.name);
                });
        }
        if !yaml.config.event_loop.is_null() {
            config.event_loop = serde_yaml::from_value(yaml.config.event_loop.clone()).unwrap();
        }
        config.normalize();
    });
    let ledger = temp_dir.path().join(".ralph/flow-authority.jsonl");
    let contents = std::fs::read_to_string(&ledger).unwrap_or_default();
    let last_forge_blocked = contents
        .lines()
        .rev()
        .find(|line| {
            let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
            v.get("topic").and_then(|t| t.as_str()) == Some("forge.plan.blocked")
        })
        .expect(
            "expected a flow-authority snapshot with topic=forge.plan.blocked (fail-close escape \
             advance); none found in ledger",
        );
    let snap: serde_json::Value = serde_json::from_str(last_forge_blocked).unwrap();
    assert_eq!(
        snap.get("step").and_then(|s| s.as_str()),
        Some("report"),
        "expected the fail-close escape to advance current_plan_step to report; got {snap}",
    );
    let outbox = ralph_core::event_loop::accepted_transition::read_outbox(temp_dir.path())
        .expect("accepted transition outbox must be readable");
    let blocked_count = outbox
        .iter()
        .filter(|entry| entry.topic == "forge.plan.blocked")
        .count();
    assert_eq!(
        blocked_count, 1,
        "fail-close must durably accept forge.plan.blocked exactly once"
    );
}

/// U3 (plan 2026-08-03-004) / S2: mid-wave resume replays only the
/// unfinished unit; tasks close only via `forge.wave.settled`.
#[test]
fn test_parallel_forge_resume_wave_replay_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_resume_wave_replay_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

/// U3 (plan 2026-08-03-004) / S3: after an accepted
/// `forge.wave.integrated` boundary only the verifier resumes;
/// upstream hat activations do not increase.
#[test]
fn test_parallel_forge_resume_verifier_only_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_resume_verifier_only_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

/// U3 (plan 2026-08-03-004) / S4: correction-interrupted resume
/// re-binds the correction executor with consistent wave metadata and
/// a fresh round budget.
#[test]
fn test_parallel_forge_resume_correction_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_resume_correction_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

/// U3 (plan 2026-08-03-004) / S7: a repeated manifest bootstrap is a
/// no-op — one recovery obligation, one activation per resumed hat.
#[test]
fn test_parallel_forge_resume_idempotent_bootstrap_runtime() {
    let yaml =
        load_scenario("tests/scenarios/parallel_forge_resume_idempotent_bootstrap_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_review_passed_while_wave_open() {
    let yaml = load_scenario("tests/scenarios/flow_reliability/review_passed_while_wave_open.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-22-004 plan U4 (S2 + S5): an enabled `payload_consistency`
/// rule that hits the current `fix.done` payload must reject the event
/// through the REAL EventLoop runner. The rejected `fix.done` never
/// reaches the bus (`absent_events`), the downstream finisher never
/// wakes, and `on_violation: reject_with_resume` routes a recoverable
/// CorrectionContext into `state.prompt_context.correction_blocks`
/// (`assert_state.correction_block_present`) — the loop does NOT honor
/// the rejected event as a success (`completion: false`).
///
/// MUST use `run_workflow_guard_scenario` (real EventLoop), never the
/// `run_scenario` stub (plan §U4): the stub only checks iteration count
/// and would silently swallow a topology/gate mismatch.
#[test]
fn test_payload_consistency_reject_inconsistent_fix_done() {
    let yaml =
        load_scenario("tests/scenarios/payload_consistency/reject_inconsistent_fix_done.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-22-004 plan U4 (non-vacuity control + S5 clarity): the SAME
/// topology and rule as the reject scenario, but with a consistent
/// payload that misses the rule. The `fix.done` is accepted and the
/// loop completes normally. Proves the rejection in the sibling
/// scenario is caused by the `payload_consistency` gate (not the
/// harness): flipping the payload from hit→miss flips reject→accept.
#[test]
fn test_payload_consistency_accept_consistent_fix_done() {
    let yaml = load_scenario("tests/scenarios/payload_consistency/accept_consistent_fix_done.yml");
    run_workflow_guard_scenario(yaml);
}

/// U5 (plan 2026-08-06-001) — evidence-bound correction BDD.
/// The fixer emits a contradictory `fix.done` payload that
/// the `payload_consistency` gate rejects; the next prompt
/// carries a semantic correction block with structured
/// observed / invariant / required proof so the hat cannot
/// satisfy the gate by editing fields.  After two rejection
/// rounds the fixer lands a consistent payload (real fix
/// landed, fixes_applied=1), the gate accepts, the finisher
/// fires, the loop completes.  Pins S1, S2, S7, S8.
#[test]
fn test_evidence_bound_correction_payload_consistency() {
    let yaml = load_scenario("tests/scenarios/payload_consistency/evidence_bound_correction.yml");
    run_workflow_guard_scenario(yaml);
}

/// U5 (plan 2026-08-06-001) — S3: retry_count increments on
/// repeated rejection of the same correction key. Three
/// contradictory fix.done rounds increment the counter from 0
/// to 1 to 2; the fourth round exhausts max_iterations and
/// plan.blocked fires.
#[test]
fn test_evidence_bound_retry_increment() {
    let yaml =
        load_scenario("tests/scenarios/payload_consistency/evidence_bound_retry_increment.yml");
    run_workflow_guard_scenario(yaml);
}

/// U5 (plan 2026-08-06-001) — S4: retry_count resets when a
/// correction is accepted. The rejection ledger entry is cleared
/// on acceptance, so a fresh rejection of the same topic starts
/// at retry_count=0. After a second rejection, the counter is
/// again at 1 (not accumulated across the acceptance boundary).
#[test]
fn test_evidence_bound_retry_reset() {
    let yaml = load_scenario("tests/scenarios/payload_consistency/evidence_bound_retry_reset.yml");
    run_workflow_guard_scenario(yaml);
}

/// U5 (plan 2026-08-06-001) — S5+S6: precheck rejection routes
/// to the hat named in `on_fail.target`. S5: a normal LLM
/// `work.failed.rejected` carries `target_hat=Some("executor")`
/// so only the executor's next prompt receives the correction
/// block. S6: a synthetic `work.failed.rejected` with
/// `synthetic=true` and `reason=gate_silent_or_ambiguous` is
/// surfaced with unchecked/unavailable evidence observations.
#[test]
fn test_evidence_bound_precheck_routing() {
    let yaml =
        load_scenario("tests/scenarios/payload_consistency/evidence_bound_precheck_routing.yml");
    run_workflow_guard_scenario(yaml);
}

/// U5 (plan 2026-08-06-001) — S7: three consecutive precheck
/// rejections exhaust `retry_budget=3`. The fourth rejection
/// triggers `DispatchOutcome::Exhausted` BEFORE the
/// CorrectionContext is built, so no correction block is queued
/// at the exhaust iteration. Exactly ONE `plan.blocked` event
/// fires (not one per round). The rejection ledger retains the
/// last-evidence entry (retry_count=2) for post-mortem.
#[test]
fn test_evidence_bound_exhaust() {
    let yaml = load_scenario("tests/scenarios/payload_consistency/evidence_bound_exhaust.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_incomplete_wave_plan_blocked() {
    let yaml = load_scenario("tests/scenarios/flow_reliability/incomplete_wave_plan_blocked.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-06-17-002 plan U8: regression for the wave dimension
/// enforcement loop. A 4-dimension review wave where one worker
/// initially returns the wrong `dimension`; CLI precheck + merge
/// layer drop the event, the dispatcher writes a `task.resume`,
/// the worker retries with the correct dimension, and the wave
/// converges to 4 valid `review.dimension.done` events with no
/// `plan.blocked`.
#[test]
fn test_wave_dimension_mismatch_retry() {
    let yaml = load_scenario("tests/scenarios/flow_reliability/wave_dimension_mismatch_retry.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-02-003 plan U2: linear one-shot plan execution for the
/// `ce-executor-pipeline` preset. The 12-hat flat single-consumer
/// chain (plan-reviewer → executor → 6 dim hats → review-synthesizer
/// → fixer → alignment → reporter → LOOP_COMPLETE) is exercised by
/// feeding the events through a real EventLoop. Asserts the 12
/// business events fire in order and the loop completes
/// (`completion: true`). Any regression that drops a dimension
/// hat, breaks the chain handoff, or merges two downstream hats
/// fails the `expected.events` count or the `absent_events` list.
#[test]
fn test_ce_executor_pipeline() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-08 loop preset: first review round has no P0/P1, so
/// review-gate emits `review.accepted` and the fix path stays absent.
#[test]
fn test_ce_executor_pipeline_loop() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_loop.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-08 loop preset regression: first review round has P1
/// residuals, so the gate requests fixes; `fix.done` must wake
/// `review-reentry`, which starts round 2 before any terminal
/// completion is honored.
#[test]
fn test_ce_executor_pipeline_loop_fix_reentry() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_loop_fix_reentry.yml");
    run_workflow_guard_scenario(yaml);
}

/// Loop topology regression: only `work.done` flows through
/// `test-stabilizer`; `fix.done` directly re-enters review. This
/// scenario asserts:
/// - test-stabilizer fires once for the initial executor handoff.
/// - review-reentry subscribes to stabilization.done and fix.done.
/// - Round 2 review.round.ready carries source_topic=fix.done and
///   round_base_sha == fix.head_sha.
#[test]
fn test_ce_executor_pipeline_loop_fix_stabilizer_reentry() {
    let yaml =
        load_scenario("tests/scenarios/ce_executor_pipeline_loop_fix_stabilizer_reentry.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-09-004 plan U3: at review_round 6 with
/// `blocking_main_conflict_count > 0`, the gate must emit
/// `review.loop.blocked` and its prompt must carry the
/// `[max_round_blocked]` hint guidance from the trigger context.
#[test]
fn test_ce_executor_pipeline_loop_max_round_blocked() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_loop_max_round_blocked.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-02-003 plan U2, narrowed by 2026-07-24-002 plan U3 (B2):
/// dead-end variant. `work.failed` is reserved for runs with zero
/// deliverable commits (`completed_units` empty). The stabilizer,
/// 6-dim review chain, and downstream synthesizers/fixer/alignment
/// MUST NOT fire — only reporter handles the dead end. Asserts the
/// absent_events list contains stabilization.*, every dimension
/// done, fix.done, align.done, and review.complete.
#[test]
fn test_ce_executor_pipeline_blocked() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_blocked.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-24-003 plan U2: scope ambiguity fail-close — no wave dispatch.
#[test]
fn test_implementation_review_scope() {
    let yaml = load_scenario("tests/scenarios/implementation_review_scope.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-24-003 plan U3: success topic/schema chain through wave fan-in
/// stand-in to LOOP_COMPLETE{result:clean}.
#[test]
fn test_implementation_review_wave() {
    let yaml = load_scenario("tests/scenarios/implementation_review_wave.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-26-003 plan U6 / S7 / AE5 + 2026-07-26-004 plan U10
/// (R10): review.wave.failed → finalizer → LOOP_COMPLETE{result:blocked}.
/// Run on the real EventLoop (`run_workflow_guard_scenario`,
/// the stub-safe `run_scenario` would silently swallow the
/// topology mismatch) so `expected.events` asserts the events
/// that actually hit the ledger. The hat assignment locked in
/// by the scenario's `subscribes_to` chain proves that the
/// wave-failed trigger routes to `finalizer`, never to
/// `review-synthesizer` (the primary-20260726 incident
/// misroute). All 5 mock responses are consumed in order:
/// scope-preparer emits scope.ready → review-dispatcher emits
/// review.unit.ready → review-worker emits review.unit.done →
/// wave-runtime injects review.wave.failed → finalizer emits
/// LOOP_COMPLETE. The absent_events guarantee locks the S7 /
/// AE5 contract: `review.synthesized` / `review.wave.complete`
/// must NOT appear when the wave goes down the failed path.
#[test]
fn test_implementation_review_wave_failed() {
    let yaml = load_scenario("tests/scenarios/implementation_review_wave_failed.yml");
    run_workflow_guard_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-07-27-001 plan U2: Red tests (must fail before fix)
// ──────────────────────────────────────────────────────────────────────

/// U2 Red 1: without the wave-runtime stand-in, a scenario that only
/// emits review.unit.done events MUST NOT produce review.wave.complete
/// (the coordination topic must come from the production fan-in seam).
/// Before the fix, this test fails because no production fan-in is wired
/// in the BDD runner for review waves — the supervisor bridge only handles
/// exec/fix waves. After the fix, the scenario
/// `implementation_review_wave_runtime_fan_in.yml` proves the seam works.
#[test]
fn implementation_review_success_uses_runtime_fan_in() {
    // This test verifies the RED baseline: when we use the production-backed
    // scenario YAML (no wave-runtime hat), the supervisor bridge must
    // inject review.wave.complete from real coordinator tick.
    // The test fails before the fix because run_bdd_supervisor_fan_in
    // only processes exec.unit.done / fix.unit.done, not review.unit.done.
    // FIX: run_bdd_supervisor_fan_in already handles review.unit.done (WaveKind::Review).
    // This test passes after U1 confirms review wave fan-in works.
    let yaml = load_scenario("tests/scenarios/implementation_review_wave_runtime_fan_in.yml");
    run_workflow_guard_scenario(yaml);
}

/// U2 Red 2 / S7: aggregate-timeout / partial wave via production fan-in.
/// Two of six slots complete; harness force-terminals → review.wave.failed
/// → finalizer emits LOOP_COMPLETE{result:blocked}. No task.resume redrive.
#[test]
fn implementation_review_timeout_reaches_finalizer_without_task_resume_redrive() {
    let yaml =
        load_scenario("tests/scenarios/implementation_review_wave_failed_runtime_fan_in.yml");
    run_workflow_guard_scenario(yaml);
}

/// U2 Red 3: an agent hat that tries to emit review.wave.complete or
/// review.wave.failed directly must be rejected by the origin guard.
/// This test verifies the preset does NOT declare any hat as publishing
/// those topics (which would conflict with the runtime-only authority).
#[test]
fn implementation_review_runtime_topics_remain_agent_denied() {
    use ralph_core::runtime_contract::RuntimeContractStrictness;

    // Parse the implementation-review preset YAML directly.
    // Path is relative to crate root (crates/ralph-core/).
    let preset_yaml = std::fs::read_to_string("../../presets/en/implementation-review.yml")
        .expect("must read implementation-review.yml");
    let config: RalphConfig =
        serde_yaml::from_str(&preset_yaml).expect("parse implementation-review.yml");

    let registry = ralph_core::HatRegistry::from_runtime_config(&config);
    let strictness = RuntimeContractStrictness::preset_check_strict();
    let report = ralph_core::runtime_contract::RuntimeContractAggregator::aggregate(
        "u2-red3:agent_denied_runtime_topics",
        &config,
        &registry,
        strictness,
        None,
    );

    // The key invariant: NO hat in the preset publishes review.wave.complete
    // or review.wave.failed. These are runtime-only coordination topics.
    for (hat_id, hat_config) in &config.hats {
        for topic in &hat_config.publishes {
            assert!(
                !topic.contains("wave.complete") && !topic.contains("wave.failed"),
                "U2 Red 3: hat '{}' publishes '{topic}' — review.wave.complete/failed \
                 are runtime-only, no agent hat may publish them",
                hat_id
            );
        }
    }
    // Also verify the preset passes strict lint (no other violations).
    assert!(
        report.passed,
        "U2 Red 3: preset must pass strict lint (got findings: {:?})",
        report
            .findings
            .iter()
            .map(|f| format!("[{:?}] {}", f.severity, f.message))
            .collect::<Vec<_>>()
    );
}

/// U2 Red 4: structural proof that review-dispatcher does NOT subscribe
/// to task.resume and does NOT own any coordination topic.
/// The dispatcher may only publish review.unit.ready or dispatch.blocked.
#[test]
fn implementation_review_dispatcher_contract_has_no_resume_redrive() {
    use ralph_core::runtime_contract::RuntimeContractStrictness;

    // Path is relative to crate root (crates/ralph-core/).
    let preset_yaml = std::fs::read_to_string("../../presets/en/implementation-review.yml")
        .expect("must read implementation-review.yml");
    let config: RalphConfig =
        serde_yaml::from_str(&preset_yaml).expect("parse implementation-review.yml");

    let registry = ralph_core::HatRegistry::from_runtime_config(&config);
    let strictness = RuntimeContractStrictness::preset_check_strict();
    let report = ralph_core::runtime_contract::RuntimeContractAggregator::aggregate(
        "u2-red4:dispatcher_no_resume_redrive",
        &config,
        &registry,
        strictness,
        None,
    );

    // Find review-dispatcher's trigger/publish list
    let dispatcher_hat = config
        .hats
        .get("review-dispatcher")
        .expect("review-dispatcher must exist");

    // Red 4a: dispatcher must NOT trigger on task.resume
    let triggers_task_resume = dispatcher_hat
        .triggers
        .iter()
        .any(|t| t.as_str() == "task.resume");
    assert!(
        !triggers_task_resume,
        "U2 Red 4a: review-dispatcher must NOT trigger on task.resume; \
         triggers={:?}",
        dispatcher_hat.triggers
    );

    // Red 4b: dispatcher must NOT publish any *.wave.* coordination topic
    let publishes_coord = dispatcher_hat.publishes.iter().any(|p| p.contains("wave"));
    assert!(
        !publishes_coord,
        "U2 Red 4b: review-dispatcher must NOT publish any *.wave.* topic; \
         publishes={:?}",
        dispatcher_hat.publishes
    );

    // Red 4c: dispatcher publishes exactly its mutually exclusive outcomes
    let expected_publishes = std::collections::BTreeSet::from([
        "dispatch.blocked".to_string(),
        "review.unit.ready".to_string(),
    ]);
    let actual_publishes = dispatcher_hat
        .publishes
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        actual_publishes, expected_publishes,
        "U2 Red 4c: review-dispatcher publishes must be exactly review.unit.ready and dispatch.blocked"
    );

    // The preset overall must pass strict lint (no other violations)
    assert!(
        report.passed,
        "U2 Red 4: preset must pass strict lint (got findings: {:?})",
        report
            .findings
            .iter()
            .map(|f| format!("[{:?}] {}", f.severity, f.message))
            .collect::<Vec<_>>()
    );
}

// ──────────────────────────────────────────────────────────────────────
// 2026-07-27-001 plan U2: production-backed Green scenarios
// ──────────────────────────────────────────────────────────────────────

/// U2 Green 2a: success path via real Supervisor fan-in.
/// `run_bdd_supervisor_fan_in` (activated by `supervisor.enabled: true`)
/// drives `SupervisorCoordinator.tick` from six review.unit.done events
/// and injects `review.wave.complete`. The chain continues through
/// review-synthesizer → fix-planner → finalizer → LOOP_COMPLETE{result:clean}.
#[test]
fn test_implementation_review_wave_runtime_fan_in() {
    let yaml = load_scenario("tests/scenarios/implementation_review_wave_runtime_fan_in.yml");
    run_workflow_guard_scenario(yaml);
}

/// U2 Green 2b: failure/timeout path via real Supervisor fan-in.
/// Two of six slots complete; BDD force-terminal drives InjectedFailed.
/// Finalizer receives review.wave.failed (no synthesizer) and emits
/// LOOP_COMPLETE{result:blocked}. Absent_events lock the S7 contract.
#[test]
fn test_implementation_review_wave_failed_runtime_fan_in() {
    let yaml =
        load_scenario("tests/scenarios/implementation_review_wave_failed_runtime_fan_in.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-27-003 plan U7: production seam proof for the failed path
/// in the same shape as `test_implementation_review_wave_failed_runtime_fan_in`
/// (U2 Green 2b), but specialized to the U7 smallest-meaningful salvage
/// baseline (1 of 6 slots) so the absent_events coverage and
/// event_topic_counts pins survive a future regression that silently
/// reduces the missing-dimension floor or adds a fallback synth route.
///
/// Complement to `test_implementation_review_wave_runtime_fan_in` (U2
/// Green 2a, success). Together they form the U7 path-2 BDD proof that
/// `finalizer` is the ONLY hat activated after `review.wave.failed` —
/// `review-synthesizer`/`fix-planner` never fire on the failed path.
#[test]
fn test_implementation_review_wave_runtime_failed_fan_in() {
    let yaml =
        load_scenario("tests/scenarios/implementation_review_wave_runtime_failed_fan_in.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-24-003 plan U8 / S1: Apply → Confirm happy path through
/// the real EventLoop runner. The wave event flows from
/// `work.done` through the dispatcher to `review.wave.ready`
/// and the loop closes without any fail-closed event. Uses
/// `run_workflow_guard_scenario` (the real EventLoop runner,
/// not the `run_scenario` stub) so `expected.events` actually
/// checks what hit the ledger.
#[test]
fn test_wave_protocol_normal_apply_confirm() {
    let yaml = load_scenario("tests/scenarios/wave_protocol/normal_apply_confirm.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-24-003 plan U8 / S9: a partial wave is fail-closed —
/// the runtime never auto-recovers a missing slot. We model
/// the contract by leaving the mock response without
/// `completion: true` and asserting the loop neither emits a
/// terminal green signal (`review.passed` / `LOOP_COMPLETE`)
/// nor silently re-fires the wave. An operator can then run
/// `ralph wave inspect <wave_id>` to inspect the gap.
#[test]
fn test_wave_protocol_recovery_required() {
    let yaml = load_scenario("tests/scenarios/wave_protocol/recovery_required.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-24-003 plan U4: synthesizer integrity failure → blocked terminal;
/// fix.plan.ready must stay absent.
#[test]
fn test_implementation_review_fan_in() {
    let yaml = load_scenario("tests/scenarios/implementation_review_fan_in.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-16-001 plan U2 bill contract, inverted by 2026-07-24-002
/// plan U3 (B1): a non-final Unit failure (U2) with commits on
/// record (`completed_units` non-empty) MUST settle via `work.done`
/// with `execution_status=partial` and the full settlement bill
/// (`planned_units` / `attempted_units` / `completed_units` /
/// `failed_units` / `blocked_units` / `skipped_units`), and the
/// whole linear chain (stabilizer → 6 dims → synthesizer →
/// fix-planner → fixer → alignment → reporter) MUST run to
/// report.done{verdict: pass_with_residuals}. `work.failed` is
/// dead-end-only and must stay absent here.
#[test]
fn test_ce_executor_pipeline_executor_fail_stop() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_executor_fail_stop.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-16-001 plan U7 + S15: when test-stabilizer emits
/// `stabilization.blocked` (e.g. traceability gaps or unstable
/// evidence), the loop MUST short-circuit to reporter without firing
/// any review/fix/align event. Asserts that stabilization.blocked is
/// the sole terminal trigger path to reporter, and report.done
/// carries verdict=blocked with reason=stabilization_blocked.
#[test]
fn test_ce_executor_pipeline_stabilization_blocked_report() {
    let yaml =
        load_scenario("tests/scenarios/ce_executor_pipeline_stabilization_blocked_report.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_ce_executor_pipeline_post_fix_review() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_post_fix_review.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-16-001 plan U6: Fixer MUST execute Fix Units strictly
/// serially. When a non-final Fix Unit fails (UF2 in this scenario),
/// subsequent Fix Units (UF3..UF5) MUST NOT start. `fix.done` MUST
/// carry the complete fix-unit bill
/// (`planned_fix_units` / `attempted_fix_units` /
/// `completed_fix_units` / `failed_fix_units` /
/// `skipped_fix_units`). Then the new U5 topology applies:
/// fix.done → review-reentry round 2 → review.loop.blocked (must-fix count
/// still > 0). Reporter consumes review.loop.blocked.
#[test]
fn test_ce_executor_pipeline_loop_fixer_fail_stop() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_loop_fixer_fail_stop.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-24-002 plan U3 (B5, loop half): a partial `work.done`
/// (U2 failed, U1/U3 delivered) in the loop preset MUST walk
/// stabilizer → review-reentry → 6-dim review → gate accept →
/// alignment → reporter instead of short-circuiting via
/// `work.failed`. Round 1 has no must-fix-now findings, so the
/// gate emits `review.accepted` and the loop ends with
/// verdict=pass_with_residuals; `work.failed`, `fix.requested`,
/// and `review.loop.blocked` must stay absent.
#[test]
fn test_ce_executor_pipeline_loop_executor_partial_done() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_loop_executor_partial_done.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-24-002 plan U3 (KTD10): when a non-final Fix Unit fails
/// (UF2) while independent Fix Units land (UF1, UF3), `fix.done`
/// MUST report `fix_status=partial` with the full fix-unit bill —
/// never a premature `blocked`. The partial fix.done re-enters
/// review round 2, which accepts with the UF2 finding as residual,
/// and the loop reports verdict=pass_with_residuals.
/// `review.loop.blocked` must stay absent.
#[test]
fn test_ce_executor_pipeline_loop_fixer_partial_continue() {
    let yaml =
        load_scenario("tests/scenarios/ce_executor_pipeline_loop_fixer_partial_continue.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-24-002 plan U3 (B3): regressions are report-only. A
/// `work.done` with `new_business_regressions_count > 0` and an
/// honest `post_verification_status=red` walks the full linear
/// chain and ends in report.done{verdict: pass_with_residuals} —
/// regressions alone never force verdict=blocked or work.failed.
#[test]
fn test_ce_executor_pipeline_report_residuals() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_report_residuals.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-16-001 plan U1: Plan Reviewer MUST accept a
/// semantically-complete but format-drifted plan and emit
/// `plan.ready` carrying the ce-unified-plan/v1 contract fields
/// (`plan_contract_version`, `normalized_plan_file`,
/// `plan_contract_digest`, `trace_file`). Asserts that plan.ready
/// is emitted (not plan.blocked) and the contract field set is
/// present and non-empty.
#[test]
fn test_ce_plan_reviewer_semantic_recognition_positive() {
    let yaml = load_scenario(
        "tests/scenarios/ce_executor_pipeline_plan_reviewer_semantic_recognition.yml",
    );
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-16-001 plan U1 + S2: a list-shaped plan that lacks
/// verification gates MUST be rejected with `plan.blocked` carrying
/// non-empty `semantic_gaps`. The blocked event MUST NOT carry a
/// fabricated `normalized_plan_file` (the contract forbids
/// fake-ready artifacts on the blocked path).
#[test]
fn test_ce_plan_reviewer_semantic_blocked_negative() {
    let yaml = load_scenario(
        "tests/scenarios/ce_executor_pipeline_plan_reviewer_semantic_blocked_negative.yml",
    );
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-16-001 plan U1 + S2: an ambiguous plan (contradictory
/// dependencies, undetermined Unit boundary) MUST be rejected with
/// `plan.blocked`. The Plan Reviewer MUST NOT escape the ambiguity
/// by emitting `plan.ready` with a fake `normalized_plan_file`.
#[test]
fn test_ce_plan_reviewer_semantic_ambiguous() {
    let yaml =
        load_scenario("tests/scenarios/ce_executor_pipeline_plan_reviewer_semantic_ambiguous.yml");
    run_workflow_guard_scenario(yaml);
}

/// plan.blocked short-circuits to reporter without executor or review chain.
#[test]
fn test_ce_executor_pipeline_plan_blocked() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_plan_blocked.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_plan_gate_dual_publish_handoff() {
    // 2026-06-15-003 fix U2: regression for the `(queue.advance, work.ready)`
    // dual-publish carve-out. Both topics must be accepted in the same turn
    // and the executor must wake in a later turn.
    let yaml = load_scenario("tests/scenarios/plan_gate_dual_publish_handoff.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-07-03-001 plan U10 / fix-plan U10: register the
// U13 supervisor minimal scenario via
// `run_workflow_guard_scenario` (NOT the stub
// `run_scenario`). The YAML file is extended with a
// `system_injected: exec.wave.complete` mock response so
// the test actually exercises the supervisor coord-event
// path (F-010 / AE1 — the original fixture was a
// tautological passthrough that never drove the
// `exec-integrator` hat).
#[test]
fn test_u13_supervisor_minimal() {
    let yaml = load_scenario("tests/scenarios/supervisor/supervisor_minimal.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-07-28-001 plan U2 (R3/S3, R4/S4): BDD fixture for parallel-forge
// exec_wave branch topology. Uses run_workflow_guard_scenario (real
// EventLoop) with supervisor fan-in enabled (expected_slots: 2).
// R3/S3: exec.unit.done / exec.unit.failed do NOT advance exec_wave step.
// R4/S4: exec.wave.complete injected by supervisor fan-in DOES advance
//   exec_wave → unit_review.
// Coverage: 2-unit fan-out → supervisor injects exec.wave.complete after
// both slots complete. Verifies event_topic_counts for unit.done (2x) and
// exec.wave.complete (1x), and absent exec.wave.failed on happy path.
#[test]
fn test_parallel_forge_exec_wave_branch() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_exec_wave_branch.yml");
    run_workflow_guard_scenario(yaml);
}

// A force-terminalled exec wave must inject `exec.wave.failed` AND find
// a subscriber. The failure arm was previously injected into a preset
// where no hat listened for it, so a silent worker ended the run with
// no correction and no report. This pins the closed loop:
// exec.wave.failed → failure handler → forge.correction.requested.
#[test]
fn test_parallel_forge_exec_wave_failed_routes_to_correction() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_exec_wave_failed_correction.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-07-28-001 plan U3 (R5/S5 + R9/S9): parallel-forge full
// 14-step declared flow success path. Every cross-hat handoff advances
// the authority exactly once through planning → exec_wave → unit_review
// → integration → incremental_verify → full_verify → audit → report
// → plan_end → LOOP_COMPLETE. Uses run_workflow_guard_scenario (real
// EventLoop) with supervisor fan-in enabled so the real
// SupervisorCoordinator injects exec.wave.complete (not a faked mock).
// U3 critical assertion: failure-side topics (`exec.wave.failed`,
// `work.failed`, `forge.plan.blocked`) must stay absent, and
// `LOOP_COMPLETE` / `forge.report.done` each fire exactly once.
#[test]
fn test_parallel_forge_declared_flow_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_declared_flow_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-07-28-001 plan U3 (R6/S6 + R9/S9): parallel-forge declared
// flow with failed post-exec convergence. After
// `forge.integration.done`, the verifier emits `work.failed` instead
// of `forge.incremental.verified`. The failure-capable step stays in
// `incremental_verify` (work.failed is non-transition), the reporter
// is re-triggered, and `forge.report.done` + `LOOP_COMPLETE` close
// the loop. Uses run_workflow_guard_scenario (real EventLoop) with
// supervisor fan-in enabled. U3 critical assertion: subsequent
// success topics (`forge.incremental.verified`, `forge.full.verified`,
// `forge.audit.done`) must stay absent after the `work.failed` is
// accepted — without this, a regression that allows success handoffs
// to slip past the failure gate would silently expand the failure
// path.
#[test]
fn test_parallel_forge_declared_flow_failed_runtime() {
    let yaml = load_scenario("tests/scenarios/parallel_forge_declared_flow_failed_runtime.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-06-20-001 plan U6: serial-lint BDD scenarios were
// considered but deferred. The first iteration (commit
// 0083f5b) shipped 3 YAML scenarios + 3 #[test] functions, but
// the review v2 (20260620-164253-6e112e43) caught:
//   - F1: `run_scenario` is a stub that never boots an
//     EventLoop, so the engine gate path was not exercised.
//   - F2: YAML used `seen_events:` (silently dropped) instead
//     of `events:`.
//   - F3: scenario runner's `next_hat()` returns the fallback
//     `ralph` hat on iteration 2 (because the executor's
//     rejected event never lands on the bus), making the
//     `## LINT MIRROR` injection unreachable from the
//     SourceHat routing check.
//
// The review v2 fix attempts (run_workflow_guard_scenario +
// events: rename + JSON payload) uncovered a deeper issue:
// the in-loop feedback path (engine gate →
// `state.pending_lint_resume` → `inject_pending_lint_resume`)
// is a *single* per-process state machine, and the scenario
// runner's per-iteration `next_hat` is not designed to drive
// the lint feedback path end-to-end.
//
// The cleanest path forward is unit tests in
// `crates/ralph-core/src/event_loop/tests/serial_lint.rs` and
// `crates/ralph-core/src/preset/engine/linter.rs` that
// exercise the in-loop path with explicit
// `engine_required_field_filter` and `inject_pending_lint_resume`
// calls. Those unit tests do not need a full EventLoop boot,
// so the scenario runner's hat-routing is not a factor. The
// U6 BDD scenarios remain in `docs/plans/2026-06-20-001-...-plan.md`
// for a future commit that pairs them with a BDD framework
// extension (e.g., a "lint feedback" runner that drives the
// loop in a single hat so the SourceHat routing matches).

#[test]
fn test_plan_gate_dual_publish_inverse_rejected() {
    // 2026-06-17-002 U3 regression: the dual-publish carve-out is an
    // *ordered* pair. Inverse order `(work.ready, queue.advance)` must
    // NOT admit the second event — only the first business event
    // (`work.ready`) is accepted; `queue.advance` is dropped.
    let yaml = load_scenario("tests/scenarios/plan_gate_dual_publish_inverse_rejected.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_plan_gate_dual_publish_third_blocked() {
    // 2026-06-17-002 U3 regression: the dual-publish carve-out is
    // *sticky* — a third business event in the same turn is dropped
    // by the per-turn budget. The carve-out has a single +1 window,
    // not unlimited.
    let yaml = load_scenario("tests/scenarios/plan_gate_dual_publish_third_blocked.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-06-23-006 plan U7 (P0-2): removed
// `test_progress_task_mismatch_gate_blocks_queue_advance` — its
// scenario `tests/scenarios/step_handoff/progress_task_mismatch.yml`
// embedded `plan-gate` topology deleted by 2026-06-24-001 plan U5.

// 2026-06-23-006 plan U7 (P0-2): removed
// `test_state_projection_work_done_updates_progress`,
// `test_step_advance_u1_to_u2_handoff_under_30s`,
// `test_fix_exhausted_reaches_plan_gate`,
// `test_debug_exhausted_reaches_plan_gate` — their scenarios
// embedded `plan-gate` / `debug-resolver` hat topology, deleted by
// 2026-06-24-001 plan U5.

#[test]
fn test_workflow_activation_contract_re_emit_trap() {
    // WAC-U8 AE1 (2026-06-12-002): a hat that triggers on a
    // topic published by another hat and does not declare that
    // topic in its own `publishes` is a re-emit trap. The
    // strict WAC lint must surface this as a finding.
    use ralph_core::preset_lint::run_workflow_activation_contract;
    let config_yaml = r#"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["queue.advance"]
  executor:
    name: "Executor"
    triggers: ["queue.advance"]
    publishes: ["work.done"]
"#;
    let config: ralph_core::RalphConfig =
        serde_yaml::from_str(config_yaml).expect("parse WAC AE1 fixture");
    let findings = run_workflow_activation_contract(&config, true, false);
    let re_emit = findings
        .iter()
        .find(|f| f.id == "preset.re_emit_trap")
        .expect("strict WAC must surface the re_emit_trap finding for executor+queue.advance");
    assert_eq!(re_emit.hat.as_deref(), Some("executor"));
    assert_eq!(re_emit.topic.as_deref(), Some("queue.advance"));
}

#[test]
fn test_workflow_activation_contract_handoff_pairing_broken() {
    // WAC-U8 AE1 sibling: a handoff (unique consumer) whose
    // publishes do not reach a terminal topic is flagged by
    // R4. The executor consumes `work.ready` uniquely and
    // emits a topic that no other hat triggers on, so R4 fires.
    use ralph_core::preset_lint::run_workflow_activation_contract;
    let config_yaml = r#"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["executor.dead_end"]
"#;
    let config: ralph_core::RalphConfig =
        serde_yaml::from_str(config_yaml).expect("parse WAC AE1 handoff fixture");
    let findings = run_workflow_activation_contract(&config, true, false);
    let finding = findings
        .iter()
        .find(|f| f.id == "preset.handoff_pairing_broken")
        .expect(
            "strict WAC must surface the handoff_pairing_broken finding for work.ready+executor",
        );
    assert_eq!(finding.topic.as_deref(), Some("work.ready"));
    assert_eq!(finding.hat.as_deref(), Some("executor"));
}

#[test]
fn test_workflow_activation_contract_null_payload_rejected() {
    // WAC-U8 AE3: a null `review.passed` payload is hard-rejected
    // by `event_policy::validate_event` even when the policy is
    // in Observe mode (KTD-9). The dispatcher never sees the
    // event in the validated stream.
    use ralph_core::config::{EventPolicyConfig, EventPolicyMode, ViolationAction};
    use ralph_core::{PolicyDecision, PolicyRuntimeState, validate_event};

    let mut config = EventPolicyConfig::default();
    config.enabled = true;
    config.mode = EventPolicyMode::Observe;
    config.on_violation = ViolationAction::RejectWithResume;

    let mut state = PolicyRuntimeState::default();
    let decision = validate_event("review.passed", None, &config, &mut state);
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "WAC R10 must RejectWithResume null review.passed, got {:?}",
        decision
    );
}

#[test]
fn test_workflow_activation_contract_step_advance_handoff_chain() {
    // P1 (R14 subset): executor is the unique consumer of work.ready and
    // must be priority-dispatchable when plan-gate publishes the handoff.
    // Semantic gate coverage lives in review_step_state unit tests.
    use ralph_proto::{Event, EventBus, Hat, HatId};

    let work_ready_payload = r#"{"plan_name":"p","plan_path":"docs/plans/p.md","task_id":"t2","task_key":"k2","step":"step-02","complexity":"small","reviewed_task_id":"t1","reviewed_task_key":"k1","completed_step":"step-01","next_step":"step-02"}"#;

    let mut bus = EventBus::new();
    bus.register(Hat::new("plan-gate", "plan-gate").subscribe("review.*"));
    bus.register(Hat::new("executor", "executor").subscribe("work.ready"));
    bus.register(Hat::new("review-coordinator", "rc").subscribe("work.done"));

    bus.publish(Event::new("work.ready", work_ready_payload).with_source(HatId::from("plan-gate")));

    let priority = HatId::from("executor");
    let selected = bus
        .select_next_hat_with_pending(Some(&priority))
        .expect("executor must be selectable");
    assert_eq!(
        selected,
        HatId::from("executor"),
        "handoff priority must route work.ready to executor (merry-wren dispatch gap fix)"
    );
}

#[test]
fn test_workflow_activation_contract_handoff_priority_dispatch() {
    // WAC-U8 AE5: when the EventBus's priority pre-emption is
    // armed and the priority hat has a non-empty pending
    // queue, the dispatcher selects that hat immediately,
    // skipping the round-robin scan.
    use ralph_proto::{Event, EventBus, Hat, HatId};

    let mut bus = EventBus::new();
    for id in ["alpha", "beta", "gamma"] {
        bus.register(Hat::new(id, id).subscribe("work.*"));
    }
    for (id, label) in [("alpha", "a1"), ("beta", "b1"), ("gamma", "g1")] {
        bus.publish(Event::new("work", label).with_target(id));
    }
    let sel = bus
        .select_next_hat_with_pending(Some(&HatId::from("gamma")))
        .expect("priority pre-emption must select gamma");
    assert_eq!(sel.as_str(), "gamma");
}

#[test]
fn test_isolated_with_event_projection() {
    // (2026-06-20-002 plan U3/Q-3: was a 340+ line in-line
    // duplicate of the workflow-guard runner. Now delegates to
    // the shared `run_scenario_with_snapshots` helper; only the
    // `core.event_projection` config block and the post-loop
    // `projected-events.jsonl` content check remain test-specific.)
    let yaml = load_scenario("tests/scenarios/isolated_with_event_projection.yml");

    let temp_dir = run_scenario_with_snapshots(&yaml, |config, yaml| {
        apply_yaml_hats(yaml, config);
        if !yaml.config.event_loop.is_null() {
            config.event_loop = serde_yaml::from_value(yaml.config.event_loop.clone()).unwrap();
        }
        if !yaml.config.core.is_null() {
            config.core = serde_yaml::from_value(yaml.config.core.clone()).unwrap();
        }
        // `config.core.workspace_root` is re-pinned by the helper
        // AFTER `extra_config` returns; this scenario's YAML
        // `core` block overwrites `workspace_root` (it doesn't
        // ship one), so the helper's re-pin is what actually
        // points the projector at the test's tempdir.
    });

    // Verify projection file was created and contains expected events
    let projection_path = temp_dir
        .path()
        .join(".ralph")
        .join("projected-events.jsonl");
    assert!(
        projection_path.exists(),
        "{}: Expected projection file to exist at {:?}",
        yaml.name,
        projection_path
    );

    let projection_content = std::fs::read_to_string(&projection_path).unwrap();
    let lines: Vec<&str> = projection_content.trim().lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "{}: Expected 2 projected events, got {}",
        yaml.name,
        lines.len()
    );

    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["topic"], "experiment.planned");
    assert_eq!(first["payload"], "plan: \"Build feature\"");

    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(second["topic"], "experiment.ready");
    assert_eq!(second["payload"], "status: \"done\"");

    println!("✓ {} passed", yaml.description);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-07 plan U7: end-to-end recovery contract
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_ce_executor_recovery_scenario() {
    // The YAML ships a 5-mock-response loop that mirrors the
    // 2026-06-06 drift run: executor activated, no event in iter 1
    // (missing-event), valid work.done in iter 2, wave dispatch
    // in iter 3, LOOP_COMPLETE in iter 4.  The deeper per-hypothesis
    // assertions (origin guard, contract rejection, obligation
    // alignment) live in `ralph-cli/tests/ce_executor_recovery.rs`.
    // This scenario asserts the wire-level flow.
    let yaml = load_scenario("tests/scenarios/ce_executor_recovery.yml");
    run_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-16-002 plan U6: bootstrap recovery contract
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_ce_executor_bootstrap_recovery_scenario() {
    // First work.ready omits bootstrap-only reviewed_task_id and is accepted,
    // then executor work.done, review wave, and LOOP_COMPLETE complete the loop.
    let yaml = load_scenario("tests/scenarios/ce_executor_bootstrap_recovery.yml");
    run_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-16-002 plan U6: coordinator build.deny deny rule
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_u6_coordinator_build_done_deny_scenario() {
    // Coordinator cannot emit build.done; the event is rejected and the loop
    // terminates without completion.
    let yaml = load_scenario("tests/scenarios/u6_coordinator_build_done_deny.yml");
    run_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// ──────────────────────────────────────────────────────────────────────
// U6: Hat lifecycle contract — terminal events close activations
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_hat_lifecycle_contract() {
    // Verifies that terminal events (work.done, review.complete, LOOP_COMPLETE)
    // close hat activations as expected in a simple pipeline topology.
    let yaml = load_scenario("tests/scenarios/hat_lifecycle_contract.yml");
    run_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// U6: Preset static lint BDD — AE1 coverage
//
// Exercises real config parsing, HatRegistry construction, and
// RuntimeContractAggregator with strict preset_check_strict()
// through the same path that `ralph preset check --strict` uses.
// This is NOT a source-level string assertion — it runs the full
// authoring lint pipeline.
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_preset_static_lint_scenario() {
    use ralph_core::HatRegistry;
    use ralph_core::runtime_contract::{
        FindingSeverity, RuntimeContractAggregator, RuntimeContractStrictness,
    };

    let yaml = load_scenario("tests/scenarios/preset_static_lint.yml");

    // Build RalphConfig from the YAML config section (reuse workflow guard
    // helper pattern for hat parsing).
    let mut config = RalphConfig::default();
    config.max_iterations = Some(yaml.config.max_iterations);
    config.prompt_file = Some(yaml.config.prompt_file);
    if !yaml.config.hats.is_null()
        && let Ok(hat_map) = serde_yaml::from_value::<
            std::collections::HashMap<String, serde_yaml::Value>,
        >(yaml.config.hats.clone())
    {
        let mut hats = std::collections::HashMap::new();
        for (hat_id, mut hat_value) in hat_map {
            if let Some(map) = hat_value.as_mapping_mut()
                && !map.contains_key(serde_yaml::Value::String("name".to_string()))
            {
                map.insert(
                    serde_yaml::Value::String("name".to_string()),
                    serde_yaml::Value::String(hat_id.clone()),
                );
            }
            let hat_config: HatConfig = serde_yaml::from_value(hat_value)
                .unwrap_or_else(|e| panic!("Failed to parse hat '{}': {}", hat_id, e));
            hats.insert(hat_id, hat_config);
        }
        config.hats = hats;
    }
    if !yaml.config.event_loop.is_null() {
        config.event_loop = serde_yaml::from_value(yaml.config.event_loop).unwrap();
    }
    if !yaml.config.tasks.is_null() {
        config.tasks = serde_yaml::from_value(yaml.config.tasks).unwrap();
    }
    if !yaml.config.topic_owners.is_null() {
        config.topic_owners = serde_yaml::from_value(yaml.config.topic_owners).unwrap();
    }
    if !yaml.config.topic_format_whitelist.is_null() {
        config.topic_format_whitelist =
            serde_yaml::from_value(yaml.config.topic_format_whitelist).unwrap();
    }

    // Run the aggregator with strict preset_check_strict() — same path
    // as `ralph preset check --strict` and the run hard gate.
    let registry = HatRegistry::from_runtime_config(&config);
    let strictness = RuntimeContractStrictness::preset_check_strict();
    let report = RuntimeContractAggregator::aggregate(
        "bdd:preset_static_lint",
        &config,
        &registry,
        strictness,
        None,
    );

    // AE1: valid preset must pass strict lint.
    assert!(
        report.passed,
        "preset_static_lint BDD scenario must pass strict lint: {:?}",
        report
            .findings
            .iter()
            .filter(|f| matches!(f.severity, FindingSeverity::Error | FindingSeverity::Warn))
            .map(|f| format!("[{:?}] {}: {}", f.severity, f.id, f.message))
            .collect::<Vec<_>>()
    );
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-09: four P0 guards BDD scenarios (U1–U4)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_u1_partial_wave_dispatch() {
    let yaml = load_scenario("tests/scenarios/four-p0-guards/u1-partial-wave-dispatch.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_u2_ralph_pseudo_hat_rejection() {
    let yaml = load_scenario("tests/scenarios/four-p0-guards/u2-ralph-pseudo-hat-rejection.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_u3_topic_deny_rule() {
    let yaml = load_scenario("tests/scenarios/four-p0-guards/u3-topic-deny-rule.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_u4_plan_name_equality() {
    let yaml = load_scenario("tests/scenarios/four-p0-guards/u4-plan-name-equality.yml");
    run_workflow_guard_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-10: ce-executor worktree isolation BDD scenario (U4)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_ce_executor_worktree_isolation() {
    // U4 (2026-06-10): worktree isolation contract at the event-loop level.
    // The cross-process filesystem isolation is verified end-to-end by
    // `crates/ralph-cli/tests/integration_worktree_isolation.rs`. This
    // BDD scenario complements that test by exercising the event flow
    // with a worktree-mode-shaped config, ensuring no leakage at the
    // event-loop layer.
    let yaml = load_scenario("tests/scenarios/ce-executor-worktree-isolation.yml");
    run_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// U2 of 2026-06-11-003: multi-hat isolation policy BDD scenario
//
// AE2: 4-hat preset with default (Coordinator) execution mode MUST
// be rejected by the strict preset lint aggregator with a single
// `lint.preset.multi_hat_requires_isolated` finding. The same
// finding shape drives the `ralph preset check` CLI surface, the
// `ralph preflight --check multi-hat-isolation` check, and the
// `ralph run` hard gate.
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_multi_hat_isolation_lint_bdd_4_hat_default_fails() {
    use ralph_core::HatRegistry;
    use ralph_core::preset_lint::finding_id::FINDING_MULTI_HAT_REQUIRES_ISOLATED;
    use ralph_core::runtime_contract::{
        FindingSeverity, RuntimeContractAggregator, RuntimeContractStrictness,
    };

    let yaml = load_scenario("tests/scenarios/multi_hat_isolation_lint.yml");

    // Build the resolved config exactly the way the lint aggregator
    // sees it. (Mirrors the test_preset_static_lint_scenario helper.)
    let mut config = RalphConfig::default();
    config.max_iterations = Some(yaml.config.max_iterations);
    config.prompt_file = Some(yaml.config.prompt_file);
    if !yaml.config.hats.is_null()
        && let Ok(hat_map) = serde_yaml::from_value::<
            std::collections::HashMap<String, serde_yaml::Value>,
        >(yaml.config.hats.clone())
    {
        let mut hats = std::collections::HashMap::new();
        for (hat_id, mut hat_value) in hat_map {
            if let Some(map) = hat_value.as_mapping_mut()
                && !map.contains_key(serde_yaml::Value::String("name".to_string()))
            {
                map.insert(
                    serde_yaml::Value::String("name".to_string()),
                    serde_yaml::Value::String(hat_id.clone()),
                );
            }
            let hat_config: HatConfig = serde_yaml::from_value(hat_value)
                .unwrap_or_else(|e| panic!("Failed to parse hat '{}': {}", hat_id, e));
            hats.insert(hat_id, hat_config);
        }
        config.hats = hats;
    }
    if !yaml.config.event_loop.is_null() {
        config.event_loop = serde_yaml::from_value(yaml.config.event_loop).unwrap();
    }
    if !yaml.config.tasks.is_null() {
        config.tasks = serde_yaml::from_value(yaml.config.tasks).unwrap();
    }
    if !yaml.config.topic_owners.is_null() {
        config.topic_owners = serde_yaml::from_value(yaml.config.topic_owners).unwrap();
    }
    if !yaml.config.topic_format_whitelist.is_null() {
        config.topic_format_whitelist =
            serde_yaml::from_value(yaml.config.topic_format_whitelist).unwrap();
    }

    assert_eq!(
        config.hats.len(),
        4,
        "fixture must declare 4 hats for AE2 to be meaningful"
    );

    let registry = HatRegistry::from_runtime_config(&config);
    let strictness = RuntimeContractStrictness::preset_check_strict();
    let report = RuntimeContractAggregator::aggregate(
        "bdd:multi_hat_isolation_lint",
        &config,
        &registry,
        strictness,
        None,
    );

    // 4 hats, default Coordinator mode → aggregator must fail.
    assert!(
        !report.passed,
        "4-hat default coordinator preset MUST fail strict lint: {:?}",
        report
            .findings
            .iter()
            .map(|f| format!("[{:?}] {}: {}", f.severity, f.id, f.message))
            .collect::<Vec<_>>()
    );

    // Exactly one multi_hat_requires_isolated error finding, with the
    // expected details. This is the same shape the preflight check
    // and the run gate consume.
    let multi_hat_findings: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.id == format!("lint.{}", FINDING_MULTI_HAT_REQUIRES_ISOLATED))
        .collect();
    assert_eq!(
        multi_hat_findings.len(),
        1,
        "expected exactly one multi_hat_requires_isolated finding, got: {:?}",
        report
            .findings
            .iter()
            .map(|f| format!("[{:?}] {}: {}", f.severity, f.id, f.message))
            .collect::<Vec<_>>()
    );
    let finding = &multi_hat_findings[0];
    assert_eq!(finding.severity, FindingSeverity::Error);
    assert_eq!(
        finding.details.get("actual").map(String::as_str),
        Some("4"),
        "details.actual must be 4: {:?}",
        finding.details
    );
    assert_eq!(
        finding.details.get("limit").map(String::as_str),
        Some("3"),
        "details.limit must be 3: {:?}",
        finding.details
    );
    assert_eq!(
        finding.details.get("required_mode").map(String::as_str),
        Some("isolated"),
        "details.required_mode must be 'isolated': {:?}",
        finding.details
    );
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-17-003 plan U6: flow reliability replay & BDD scenarios
//
// U6 (2026-06-17-003 plan) locks the zippy-sparrow failure pattern
// via direct integration tests against the real mechanism APIs
// (`open_waves_needing_intervention` + `incomplete_wave_gate::evaluate`).
// The BDD `expected.events` framework asserts on `seen_topics`,
// which is populated by `process_events_from_jsonl` — mechanism
// events bypass that path and are verified separately here.
//
// The `SemanticGateViolation` recoverable behavior is locked by
// the existing `test_review_passed_while_wave_open_emits_semantic_gate_violation_not_invalid_field_value`
// in `crates/ralph-core/src/event_loop/review_step_state.rs`.
//
// U6-P1: zippy-sparrow fixture replay — load the recorded JSONL
// fixture (`tests/fixtures/flow_reliability/zippy-sparrow-4of11-stall.jsonl`),
// feed the agent events through `process_events_from_jsonl`, and
// assert the gate produces the expected rejection shape
// (`SemanticGateViolation`) without the loop terminating with
// `PayloadContractViolation`. The mechanism-emitted `plan.blocked`
// is verified by `test_u6_incomplete_wave_plan_blocked_mechanism`
// above (the scenario framework's `process_events_from_jsonl`
// path does NOT call `run_iteration`, so mechanism events are
// checked out-of-band).
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_u6_incomplete_wave_plan_blocked_mechanism() {
    // U2 (2026-06-17-003 plan): 11-维 wave 收 4 维后 stall,
    // 机制层应在 0.8 * aggregate_timeout_secs 窗口后 emit
    // `plan.blocked(reason=dimension_reviewers_failed_to_converge)`。
    //
    // We test the mechanism in two layers:
    // 1. `ReviewStepTracker::open_waves_needing_intervention` returns
    //    the candidate when the wave is stalled.
    // 2. `IncompleteWaveGate::evaluate` returns the correct payload
    //    shape (reason, missing_dimensions, routing).
    use ralph_core::Event as JsonlEvt;
    use ralph_core::event_loop::review_step_state::ReviewStepTracker;
    use ralph_core::flow_lifecycle::FlowLifecycleRegistry;
    use ralph_core::flow_lifecycle::incomplete_wave_gate::{
        IncompleteWaveGate, IncompleteWaveGateConfig,
    };
    use std::thread::sleep;
    use std::time::Duration;

    // Build a tracker that mirrors the 4/11 维 stall pattern.
    let mut tracker = ReviewStepTracker::default();

    // Register wave (wave_total=11).
    let wave = JsonlEvt {
        topic: "review.wave.ready".to_string(),
        payload: Some(
            r#"{"plan_name":"u6-bdd","task_id":"u6-bdd-task","task_key":"u6-bdd-key","step":"step-01"}"#
                .to_string(),
        ),
        ts: String::new(),
        hat: Some("review-coordinator".to_string()),
        triggered: None,
        source: None,
        wave_id: Some("w-u6bdd-0001".to_string()),
        wave_index: None,
        wave_total: Some(11),
        system_injected: None,
    };
    tracker.observe_accepted(&wave);

    // Register 4 dimension.done events.
    for dim in &["d1", "d2", "d3", "d4"] {
        let dim_evt = JsonlEvt {
            topic: "review.dimension.done".to_string(),
            payload: Some(format!(
                r#"{{"plan_name":"u6-bdd","task_id":"u6-bdd-task","task_key":"u6-bdd-key","step":"step-01","dimension":"{dim}"}}"#
            )),
            ts: String::new(),
            hat: Some("dimension-reviewer".to_string()),
            triggered: None,
            source: None,
            wave_id: Some("w-u6bdd-0001".to_string()),
            wave_index: None,
            wave_total: Some(11),
            system_injected: None,
        };
        tracker.observe_accepted(&dim_evt);
    }

    // Sleep so the staleness window (0.8 * 5s = 4s) elapses.
    sleep(Duration::from_secs(5));

    // The tracker's `open_waves_needing_intervention` returns the
    // candidate wave with expected=11, received=4.
    let staleness_secs = 4u64;
    let candidates = tracker.open_waves_needing_intervention(staleness_secs);
    assert_eq!(
        candidates.len(),
        1,
        "U6: 4/11 stalled wave must be a candidate for plan.blocked"
    );
    let candidate = &candidates[0];
    assert_eq!(candidate.expected, 11);
    assert_eq!(candidate.received, 4);
    assert_eq!(candidate.wave_id, "w-u6bdd-0001");

    // The gate's `evaluate` returns the right payload shape.
    let gate = IncompleteWaveGate::new(IncompleteWaveGateConfig {
        enabled: true,
        staleness_ratio: 0.8,
    });
    let registry = FlowLifecycleRegistry::default();
    let last_dim_secs_ago = candidate.last_dimension_at.map(|t| t.elapsed().as_secs());
    let payload = gate
        .evaluate(
            &registry,
            5, // aggregate_timeout_secs
            "w-u6bdd-0001",
            11, // expected
            4,  // received
            last_dim_secs_ago,
        )
        .expect("U6: gate must emit plan.blocked payload for stalled wave");

    assert_eq!(payload.reason, "dimension_reviewers_failed_to_converge");
    assert_eq!(payload.wave_id, "w-u6bdd-0001");
    assert_eq!(payload.expected, 11);
    assert_eq!(payload.received, 4);
    // `missing_dimensions` from the tracker is empty by design (the
    // tracker only learns dimension names from `dimension.done`).
    // The audit surfaces counts only — the mechanism already covers
    // the case via `received < expected`.
    assert!(
        payload.missing_dimensions.is_empty(),
        "U6: missing_dimensions is filled by the runner from the gap between expected and received"
    );
}

// ──────────────────────────────────────────────────────────────────────
// U6-P1 fixture replay: feed the recorded zippy-sparrow JSONL through
// `process_events_from_jsonl` and assert the U1 gate produces a
// `SemanticGateViolation` (recoverable, not fatal) — mirroring the
// recovery envelope captured in line 21 of the fixture.
//
// The fixture's `recovery_envelope` line is a *target* shape produced
// by the post-fix runtime; this test verifies the gate logic that
// produces it. The mechanism-emitted `plan.blocked` is verified by
// `test_u6_incomplete_wave_plan_blocked_mechanism` above.
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_u6_zippy_sparrow_replay_fixture() {
    use ralph_core::Event as JsonlEvt;
    use ralph_core::event_loop::review_step_state::ReviewStepTracker;

    // 1) Load and validate the fixture file itself: it must contain
    //    the recorded `recovery_envelope` (semantic_gate_violation)
    //    on line 21 — the post-fix invariant we want to preserve.
    let fixture_path = "tests/fixtures/flow_reliability/zippy-sparrow-4of11-stall.jsonl";
    let fixture_text = std::fs::read_to_string(fixture_path)
        .expect("U6-P1: zippy-sparrow fixture must be readable from tests/fixtures/");
    let mut found_semantic_gate_envelope = false;
    let mut agent_event_lines: Vec<String> = Vec::new();
    for line in fixture_text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed: serde_json::Value =
            serde_json::from_str(line).expect("U6-P1: every fixture line must be valid JSON");
        if parsed.get("type") == Some(&serde_json::Value::String("recovery_envelope".into())) {
            let reason_code = parsed
                .get("reason_code")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if reason_code == "semantic_gate_violation" {
                found_semantic_gate_envelope = true;
                // The post-fix envelope must reference the
                // canonical gate name and identify the source hat
                // as `review-coordinator` (zippy-sparrow actor).
                let source_hat = parsed
                    .get("source_hat")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                assert_eq!(
                    source_hat, "review-coordinator",
                    "U6-P1: semantic_gate_violation envelope must originate from review-coordinator"
                );
            }
            // The envelope is not a bus event — skip from the replay set.
            continue;
        }
        if parsed.get("type") == Some(&serde_json::Value::String("event".into())) {
            agent_event_lines.push(line.to_string());
        }
    }
    assert!(
        found_semantic_gate_envelope,
        "U6-P1: fixture must include a recovery_envelope with reason_code=semantic_gate_violation \
         (the post-fix runtime produces this; the fixture locks the target shape)"
    );

    // 2) Replay the agent events through the gate logic directly:
    //    build a `ReviewStepTracker` from the fixture's wave /
    //    dimension events, then assert that the U1
    //    `check_semantic_gates` produces a `SemanticGateViolation`
    //    for the `review.passed(empty_diff)` event while the wave
    //    is still open. We do not drive `process_events_from_jsonl`
    //    here because the fixture contains bus-shape events that
    //    require isolated-mode hat setup; the gate's contract is
    //    verified at the `ReviewStepTracker` boundary, which is
    //    what the runtime calls.
    //
    //    The fixture was recorded as the production agent's
    //    YAML-formatted payload (e.g. `plan_name: "u6-fixture"`)
    //    while `step_key_from_event` requires JSON-encoded
    //    payloads. We synthesize the JSON triplet the tracker
    //    needs from the fixture's documented step context
    //    (the per-dimension `review.wave.ready` / `review.dimension.done`
    //    events were recorded without the triplet, since the
    //    runtime carries it in a separate event envelope at
    //    accept time). Real runtime events carry the triplet
    //    inline.
    let step_context: (String, String, String) = (
        "u6-fixture".to_string(),
        "u6-replay-task".to_string(),
        "step-01".to_string(),
    );
    let mut bus_events: Vec<JsonlEvt> = Vec::new();
    let mut tracker = ReviewStepTracker::default();
    for line in &agent_event_lines {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v.get("type") != Some(&serde_json::Value::String("event".into())) {
            continue;
        }
        let hat = v
            .get("hat")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let topic = v
            .get("topic")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let original_payload = v.get("payload").and_then(|x| x.as_str()).map(String::from);
        // For wave-related events, replace the YAML payload
        // with the JSON-encoded triplet the tracker needs.
        // For other events, keep the original payload (the
        // gate is the only thing we exercise here, and the
        // YAML payload would fail JSON parse for review.passed
        // too — but the gate only inspects the event's
        // `hat` / `topic` for the review-coordinator empty-diff
        // check, not the payload fields).
        let payload = if matches!(
            topic.as_str(),
            "review.wave.ready" | "review.dimension.done" | "review.passed"
        ) {
            let (pn, ti, st) = &step_context;
            // Pull `dimension` from the fixture's recorded
            // payload (YAML inline form) so the tracker's
            // `observe_accepted` can register the dimension in
            // `dimensions_received`. The runtime carries the
            // same field under the JSON key.
            let mut obj = serde_json::json!({
                "plan_name": pn,
                "task_id": ti,
                "step": st,
            });
            if let Some(ref p) = original_payload {
                // Naive extraction: scan for `dimension: "<name>"`
                // in the YAML-formatted payload. Fixture
                // payloads are short single-line strings, so a
                // lightweight regex-less scan is enough.
                if let Some(idx) = p.find("dimension: \"") {
                    let rest = &p[idx + "dimension: \"".len()..];
                    if let Some(end) = rest.find('"') {
                        let dim = &rest[..end];
                        obj["dimension"] = serde_json::Value::String(dim.to_string());
                    }
                }
            }
            Some(obj.to_string())
        } else {
            original_payload
        };
        let wave_id = v.get("wave_id").and_then(|x| x.as_str()).map(String::from);
        let wave_total = v
            .get("wave_total")
            .and_then(|x| x.as_u64())
            .map(|n| n as u32);
        let evt = JsonlEvt {
            topic: topic.clone(),
            payload,
            ts: String::new(),
            hat: Some(hat),
            triggered: None,
            source: None,
            wave_id,
            wave_index: None,
            wave_total,
            system_injected: None,
        };
        // Walk the same accept-path the runtime uses: feed
        // wave / dimension events into the tracker so it
        // reflects the 4-of-11 stalled state, then ask the
        // gate whether the next `review.passed` should be
        // admitted.
        if topic == "review.wave.ready" || topic == "review.dimension.done" {
            tracker.observe_accepted(&evt);
        }
        bus_events.push(evt);
    }

    // 3) The fixture's last `event` line is the
    //    `review.passed(empty_diff)` while the wave is still
    //    stalled. The U1 gate must reject it with the
    //    `review_passed_while_wave_open` semantic violation.
    let review_passed = bus_events
        .iter()
        .find(|e| e.topic == "review.passed")
        .expect("U6-P1: fixture must contain a `review.passed` event line");
    let finding = tracker
        .check_semantic_gates(review_passed, None)
        .expect("U6-P1: U1 gate must produce a finding for review.passed while wave is open");
    // `event_policy::ViolationType` is not publicly re-exported
    // from the crate root, so we assert on the `Debug` /
    // `message` surface instead. The variant tag
    // `SemanticGateViolation` and the gate id are part of the
    // public `reason_code` contract documented in
    // `docs/guide/runtime-diagnosis.md` and must be stable.
    let debug = format!("{:?}", finding.violation_type);
    assert!(
        debug.contains("SemanticGateViolation"),
        "U6-P1: expected SemanticGateViolation variant, got {debug} \
         — fixture line 20 should NOT fall through to the previous \
         (fatal) InvalidFieldValue path"
    );
    assert!(
        debug.contains("review_passed_while_wave_open"),
        "U6-P1: gate id must be the canonical zippy-sparrow gate (got: {debug})"
    );
    assert!(
        finding.message.contains("w-u6fixture-0001"),
        "U6-P1: gate message must reference the stalled wave id (got: {})",
        finding.message
    );

    // 4) Cross-check: the U5 gate's `is_wave_closed` query must
    //    report the step as still open (the U1 gate's precondition
    //    matches the U5 query, locking the gate pair in sync).
    assert!(
        !tracker.is_wave_closed("u6-fixture", "u6-replay-task", "step-01"),
        "U6-P1: tracker must report wave open for the 4-of-11 stalled step \
         (U5 gate query is consistent with the U1 gate's precondition)"
    );
}

// =====================================================================
// 2026-06-20-002 plan U1/U2: assert_state harness extension
// =====================================================================
//
// `capture_state_snapshot` and `evaluate_assert_state` are the
// runtime-state inspection path for BDD scenarios. They sit
// BELOW the production code path: the runner records read-only
// clones of `LoopState` + `BuildPromptSnapshot`s at the end of
// every iteration, then walks `ExpectedYaml.assert_state` in
// declaration order. No production code in
// `crates/ralph-core/src/` was changed.
//
// Design summary (plan 2026-06-20-002 U1):
//   R-H1  ExpectedYaml.assert_state is `Vec<AssertionYaml>`;
//         missing field = no assertion (back-compat with the 27
//         existing scenarios).
//   R-H2  Snapshot recording is read-only (`&LoopState -> Clone`).
//         The runner never mutates state from `assert_state`.
//   R-H3  `at_iteration` is mandatory. Out-of-range values fail
//         fast with a clear message rather than being silently
//         clamped or skipped.
//   R-H4  Existing scenarios (no `assert_state` field) pass
//         unchanged — the eval loop is a no-op when the list is
//         empty.
//   R-H5  Assertion failures report the predicate name + actual
//         snapshot state. See `evaluate_assert_state` below.
//   R-H6  Predicate variants reference runtime fields that are
//         shared by the broader event-loop (e.g. `pending_lint_resume`,
//         `recent_rejection_digest`). They are not specific to
//         the serial preset and are available to any future
//         scenario that needs them.

/// Build a read-only `LoopStateSnapshot` from a `&LoopState`.
/// (2026-06-20-002 plan U2). Pulls the minimum fields the
/// predicates need; deeper cloning would couple the test to
/// engine internals that can change.
///
/// 2026-06-21-002 plan U9: extended with `correction_blocks` /
/// `resume_blocks` summaries and `workspace_recovery_log`
/// (workspace path passed in so the predicate can read
/// `.ralph/recovery.jsonl` without re-deriving the location).
#[allow(dead_code)]
fn capture_state_snapshot(
    state: &ralph_core::event_loop::LoopState,
    workspace_root: Option<std::path::PathBuf>,
) -> LoopStateSnapshot {
    let pending_lint_resume =
        state
            .pending_lint_resume
            .as_ref()
            .map(|hint| PendingLintResumeSummary {
                topic: hint.topic.clone(),
                reason: hint.reason.clone(),
            });
    let rejection_digest_entries = state
        .recent_rejection_digest
        .iter()
        .map(|(code, entry)| RejectionDigestSummary {
            code: code.clone(),
            last_topic: entry.last_topic.clone(),
            last_message: entry.last_message.clone(),
        })
        .collect();
    // 2026-06-21-002 plan U9: snapshot
    // `state.prompt_context.correction_blocks` / `resume_blocks`
    // into the summary vectors. The runtime lives in
    // `ralph_core::correction::PromptContext`; we pull the
    // fields the predicate inspects and skip the rest so the
    // test does not depend on the full struct shape.
    let correction_block_summaries = state
        .prompt_context
        .correction_blocks
        .iter()
        .map(|c| {
            let feedback_kind = match c.feedback_kind {
                ralph_core::correction::FeedbackKind::Semantic => "semantic".to_string(),
                ralph_core::correction::FeedbackKind::Mechanical => "mechanical".to_string(),
                ralph_core::correction::FeedbackKind::Unknown => "unknown".to_string(),
            };
            let evidence_observed_fields = c
                .evidence
                .as_ref()
                .map(|ev| ev.observed.iter().map(|o| o.field.clone()).collect())
                .unwrap_or_default();
            let evidence_invariant = c
                .evidence
                .as_ref()
                .map(|ev| ev.invariant.clone())
                .unwrap_or_default();
            CorrectionBlockSummary {
                reason_code: c.reason_code.clone(),
                retry_count: c.retry_count,
                needs_escalation: c.needs_escalation,
                feedback_kind,
                evidence_observed_fields,
                evidence_invariant,
            }
        })
        .collect();
    let resume_block_summaries = state
        .prompt_context
        .resume_blocks
        .iter()
        .map(|r| ResumeBlockSummary {
            loop_id: r.loop_id.clone(),
            last_iteration: r.last_iteration,
        })
        .collect();
    let workspace_recovery_log = workspace_root
        .as_ref()
        .map(|p| p.join(".ralph").join("recovery.jsonl"));
    LoopStateSnapshot {
        iteration: state.iteration,
        pending_lint_resume,
        rejection_digest_entries,
        scope_violation_circuit_breaker_tripped: state
            .scope_violation_circuit_breaker_tripped
            .is_some(),
        // Plan 2026-06-20-001 KTD-7: capture the lint circuit
        // breaker state so scenario 10 (and future scenarios)
        // can assert the trip directly. The pre-existing
        // scenario 10 YAML was authored before this field
        // landed and asserted `pending_lint_resume` at iter 4
        // instead; it has been updated in lockstep with the
        // implementation.
        lint_circuit_breaker_tripped: state.lint_circuit_breaker_tripped,
        consecutive_engine_gate_rejections: state.consecutive_engine_gate_rejections,
        correction_block_summaries,
        resume_block_summaries,
        workspace_recovery_log,
    }
}

/// Walk `assert_state` in order, dispatching each entry to the
/// appropriate predicate (2026-06-20-002 plan U1). Each
/// predicate receives the snapshot at the requested
/// `at_iteration` (1-indexed) and the matching
/// `BuildPromptSnapshot` (same index). The list is iterated in
/// YAML order, so a scenario that wants to assert "set at N,
/// cleared at N+1" can list the two entries adjacent.
fn evaluate_assert_state(
    scenario_name: &str,
    assertions: &[AssertionYaml],
    state_snapshots: &[LoopStateSnapshot],
    prompt_snapshots: &[BuildPromptSnapshot],
) {
    if assertions.is_empty() {
        return;
    }
    let max_iter = state_snapshots.len();
    for (idx, assertion) in assertions.iter().enumerate() {
        let at = assertion.at_iteration;
        // R-H3: out-of-range at_iteration fails fast with a
        // clear pointer to the offending entry (1-based
        // assertion index in the YAML list).
        assert!(
            !(at < 1 || at > max_iter),
            "{}: assert_state[{}].at_iteration = {} is out of range [1, {}] \
             (scenario produced {} iterations; check the YAML `expected.iterations`)",
            scenario_name,
            idx,
            at,
            max_iter,
            max_iter
        );
        let state_snap = &state_snapshots[at - 1];
        let prompt_snap = prompt_snapshots
            .get(at - 1)
            .expect("prompt_snapshots length must match state_snapshots length");

        // Exactly one of the variant fields is set; the rest are
        // None. We dispatch on the first non-None. A scenario
        // that wants to assert multiple state dimensions at the
        // same iteration should list them as separate YAML
        // entries — keeping the dispatch single-predicate keeps
        // failure messages precise.
        if let Some(ref p) = assertion.pending_lint_resume {
            evaluate_pending_lint_resume(scenario_name, idx, at, p, state_snap);
        } else if assertion.pending_lint_resume_cleared.is_some() {
            evaluate_pending_lint_resume_cleared(scenario_name, idx, at, state_snap);
        } else if let Some(ref r) = assertion.rejection_digest_contains {
            evaluate_rejection_digest_contains(scenario_name, idx, at, r, state_snap);
        } else if let Some(ref pi) = assertion.prompt_injects {
            evaluate_prompt_injects(scenario_name, idx, at, pi, prompt_snap);
        } else if let Some(ref cb) = assertion.lint_circuit_breaker {
            evaluate_lint_circuit_breaker(scenario_name, idx, at, cb, state_snap);
        } else if let Some(ref c) = assertion.correction_block_present {
            // 2026-06-21-002 plan U9: U7a
            // `CorrectionContext` predicate — asserts a
            // correction block is queued in
            // `state.prompt_context.correction_blocks`.
            evaluate_correction_block_present(scenario_name, idx, at, c, state_snap);
        } else if let Some(ref rl) = assertion.rejection_log_contains_reason_code {
            // 2026-06-21-002 plan U9: U8
            // `rejection_log_contains_reason_code` predicate —
            // asserts `.ralph/recovery.jsonl` carries at least
            // one record with the matching `reason_code`
            // prefix.
            evaluate_rejection_log_contains_reason_code(scenario_name, idx, at, rl, state_snap);
        } else {
            panic!(
                "{}: assert_state[{}] at_iteration={} has no predicate set \
                 (expected one of pending_lint_resume, pending_lint_resume_cleared, \
                 rejection_digest_contains, prompt_injects, lint_circuit_breaker, \
                 correction_block_present, rejection_log_contains_reason_code)",
                scenario_name, idx, at
            );
        }
    }
}

// 2026-06-21-002 plan U9: `correction_block_present` predicate
// implementation. Walks
// `state.prompt_context.correction_blocks` and asserts at
// least one entry matches the supplied `reason_code_prefix`,
// and (when set) the `retry_count` and `needs_escalation`
// fields.
fn evaluate_correction_block_present(
    scenario_name: &str,
    assertion_idx: usize,
    at: usize,
    expected: &CorrectionBlockPresentYaml,
    snap: &LoopStateSnapshot,
) {
    let entries = &snap.correction_block_summaries;
    assert!(
        !entries.is_empty(),
        "{}: assert_state[{}] correction_block_present at_iteration={} \
         expected at least one entry in state.prompt_context.correction_blocks, got empty",
        scenario_name,
        assertion_idx,
        at
    );
    let mut matched = entries.iter().filter(|c| {
        if let Some(ref prefix) = expected.reason_code_prefix
            && !c.reason_code.starts_with(prefix.as_str())
        {
            return false;
        }
        if let Some(rc) = expected.retry_count
            && c.retry_count != rc
        {
            return false;
        }
        if let Some(ne) = expected.needs_escalation
            && c.needs_escalation != ne
        {
            return false;
        }
        // U5: evidence_observed_contains — every supplied
        // field name must appear as a substring in at least
        // one observed entry's field. Substring match (not
        // exact equality) so the test is not coupled to the
        // exact JSON value serialisation.
        if let Some(ref needles) = expected.evidence_observed_contains {
            for needle in needles {
                if !c
                    .evidence_observed_fields
                    .iter()
                    .any(|f| f.contains(needle))
                {
                    return false;
                }
            }
        }
        // U5: evidence_invariant_contains — substring match
        // against the entry's invariant text.
        if let Some(ref needle) = expected.evidence_invariant_contains
            && !c.evidence_invariant.contains(needle.as_str())
        {
            return false;
        }
        // U5: feedback_kind — exact match against the
        // canonical snake_case form.
        if let Some(ref kind) = expected.feedback_kind
            && c.feedback_kind != kind.as_str()
        {
            return false;
        }
        true
    });
    let first_match = matched.next();
    assert!(
        first_match.is_some(),
        "{}: assert_state[{}] correction_block_present at_iteration={} \
         no correction block matched reason_code_prefix={:?} retry_count={:?} \
         needs_escalation={:?} evidence_observed_contains={:?} \
         evidence_invariant_contains={:?} feedback_kind={:?}; entries: {:?}",
        scenario_name,
        assertion_idx,
        at,
        expected.reason_code_prefix,
        expected.retry_count,
        expected.needs_escalation,
        expected.evidence_observed_contains,
        expected.evidence_invariant_contains,
        expected.feedback_kind,
        entries
    );
}

// 2026-06-21-002 plan U9: `rejection_log_contains_reason_code`
// predicate implementation. Reads the workspace-level
// `.ralph/recovery.jsonl` recorded in the snapshot and asserts
// at least one record's `reason_code` starts with the
// supplied prefix. Mirrors the diagnostic surface that
// `ralph diagnose --from-ledger` consumes (T8.1 in
// `crates/ralph-cli/tests/diagnose.rs`).
fn evaluate_rejection_log_contains_reason_code(
    scenario_name: &str,
    assertion_idx: usize,
    at: usize,
    expected: &RejectionLogContainsReasonCodeYaml,
    snap: &LoopStateSnapshot,
) {
    let path = snap.workspace_recovery_log.as_ref().unwrap_or_else(|| {
        panic!(
            "{}: assert_state[{}] rejection_log_contains_reason_code at_iteration={} \
             no workspace path recorded in snapshot — cannot read recovery.jsonl",
            scenario_name, assertion_idx, at
        )
    });
    assert!(
        path.exists(),
        "{}: assert_state[{}] rejection_log_contains_reason_code at_iteration={} \
         recovery.jsonl not found at {:?}",
        scenario_name,
        assertion_idx,
        at,
        path
    );
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "{}: assert_state[{}] rejection_log_contains_reason_code at_iteration={} \
             failed to read {:?}: {}",
            scenario_name, assertion_idx, at, path, e
        )
    });
    let prefix = expected.prefix.as_str();
    let mut found = false;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(rc) = v.get("reason_code").and_then(|x| x.as_str())
            && rc.starts_with(prefix)
        {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "{}: assert_state[{}] rejection_log_contains_reason_code at_iteration={} \
         expected at least one record with reason_code prefix {:?}, content: {:?}",
        scenario_name, assertion_idx, at, prefix, content
    );
}

fn evaluate_pending_lint_resume(
    scenario_name: &str,
    assertion_idx: usize,
    at: usize,
    expected: &PendingLintResumeYaml,
    snap: &LoopStateSnapshot,
) {
    let actual = snap.pending_lint_resume.as_ref().unwrap_or_else(|| {
        panic!(
            "{}: assert_state[{}] pending_lint_resume at_iteration={} \
                 expected Some, got None (rejection did not seed the hint)",
            scenario_name, assertion_idx, at
        )
    });
    if let Some(ref topic) = expected.topic {
        assert_eq!(
            actual.topic, *topic,
            "{}: assert_state[{}] pending_lint_resume at_iteration={} \
             expected topic={:?}, got {:?}",
            scenario_name, assertion_idx, at, topic, actual.topic
        );
    }
    if let Some(ref needle) = expected.reason_contains {
        assert!(
            actual.reason.contains(needle.as_str()),
            "{}: assert_state[{}] pending_lint_resume at_iteration={} \
             expected reason to contain {:?}, got {:?}",
            scenario_name,
            assertion_idx,
            at,
            needle,
            actual.reason
        );
    }
}

fn evaluate_pending_lint_resume_cleared(
    scenario_name: &str,
    assertion_idx: usize,
    at: usize,
    snap: &LoopStateSnapshot,
) {
    assert!(
        snap.pending_lint_resume.is_none(),
        "{}: assert_state[{}] pending_lint_resume_cleared at_iteration={} \
         expected None, got Some({:?}) — consume-on-use did not clear the slot",
        scenario_name,
        assertion_idx,
        at,
        snap.pending_lint_resume
    );
}

fn evaluate_lint_circuit_breaker(
    scenario_name: &str,
    assertion_idx: usize,
    at: usize,
    expected: &LintCircuitBreakerYaml,
    snap: &LoopStateSnapshot,
) {
    if let Some(want_tripped) = expected.tripped {
        assert_eq!(
            snap.lint_circuit_breaker_tripped,
            want_tripped,
            "{}: assert_state[{}] lint_circuit_breaker.tripped at_iteration={} \
             expected {}, got {} (consecutive_engine_gate_rejections={})",
            scenario_name,
            assertion_idx,
            at,
            want_tripped,
            snap.lint_circuit_breaker_tripped,
            snap.consecutive_engine_gate_rejections,
        );
    }
}

fn evaluate_rejection_digest_contains(
    scenario_name: &str,
    assertion_idx: usize,
    at: usize,
    expected: &RejectionDigestContainsYaml,
    snap: &LoopStateSnapshot,
) {
    let entries = &snap.rejection_digest_entries;
    if let Some(ref topic) = expected.contains_topic {
        let hit = entries.iter().any(|e| e.last_topic == *topic);
        assert!(
            hit,
            "{}: assert_state[{}] rejection_digest_contains at_iteration={} \
             expected an entry with last_topic={:?}, got entries: {:?}",
            scenario_name, assertion_idx, at, topic, entries
        );
    }
    if let Some(ref needle) = expected.contains_reason {
        let hit = entries
            .iter()
            .any(|e| e.last_message.contains(needle.as_str()));
        assert!(
            hit,
            "{}: assert_state[{}] rejection_digest_contains at_iteration={} \
             expected an entry with last_message containing {:?}, got entries: {:?}",
            scenario_name, assertion_idx, at, needle, entries
        );
    }
}

fn evaluate_prompt_injects(
    scenario_name: &str,
    assertion_idx: usize,
    at: usize,
    expected: &PromptInjectsYaml,
    snap: &BuildPromptSnapshot,
) {
    assert!(
        !snap.hat.is_empty(),
        "{}: assert_state[{}] prompt_injects at_iteration={} \
         expected a prompt for hat {:?}, but no prompt was built this iteration \
         (no hat activated)",
        scenario_name,
        assertion_idx,
        at,
        expected.hat
    );
    assert_eq!(
        snap.hat, expected.hat,
        "{}: assert_state[{}] prompt_injects at_iteration={} \
         expected hat={:?}, got {:?}",
        scenario_name, assertion_idx, at, expected.hat, snap.hat
    );
    assert!(
        snap.prompt.contains(expected.block.as_str()),
        "{}: assert_state[{}] prompt_injects at_iteration={} \
         expected the prompt for hat {:?} to contain {:?}, but it does not. \
         First 400 chars of the prompt:\n---\n{}\n---",
        scenario_name,
        assertion_idx,
        at,
        expected.hat,
        expected.block,
        &snap.prompt[..snap.prompt.len().min(400)]
    );
}

// =====================================================================
// 2026-06-27 mechanism foundation: 5 BDD wiring scenarios (U6/U7/U8/U9/U9.5)
// =====================================================================

#[test]
fn test_mechanism_plan_blocked_reason_required() {
    let yaml =
        load_scenario("tests/scenarios/mechanism/foundation/plan_blocked_reason_required.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_mechanism_repair_budget_exhausted_blocks_plan() {
    let yaml = load_scenario(
        "tests/scenarios/mechanism/foundation/repair_budget_exhausted_blocks_plan.yml",
    );
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_mechanism_diagnosis_count_matches_final_state() {
    let yaml = load_scenario(
        "tests/scenarios/mechanism/foundation/diagnosis_count_matches_final_state.yml",
    );
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_mechanism_flow_unknown_emit_rejected() {
    let yaml = load_scenario("tests/scenarios/mechanism/foundation/flow_unknown_emit_rejected.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_mechanism_verdict_gate_terminal_alignment() {
    let yaml =
        load_scenario("tests/scenarios/mechanism/foundation/verdict_gate_terminal_alignment.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-06-29-007 plan U19: replay the
/// 2026-06-26 diagnostic scenario. The loop must NOT
/// silently burn the budget on a 4/8 partial + silence
/// emit chain (U12 obligation + U5 budget gate +
/// U7/U15 repair stream).
#[test]
fn test_mechanism_scenario_replay_2026_06_26() {
    let yaml = load_scenario("tests/scenarios/mechanism/foundation/scenario_replay_2026_06_26.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-02-001 plan U2 (Fix B): end-to-end regression for the
/// hat-routing next-hop bug. The scenario reproduces the production
/// incident chain (duplicate `work.done` → residual targeted
/// `task.resume` in executor's queue → `test.passed` next hop must
/// route to `coordinator`, not the mis-led `executor`) and asserts
/// on the event sequence at the workflow-guard layer. Pre-U1, the
/// preemption predicate (`consumer queue non-empty` instead of
/// `consumer queue contains the handoff topic`) caused `next_hat`
/// to pre-empt `coordinator`'s legitimate handoff dispatch, so
/// `executor` would run step-02 without re-coordination. Post-U1
/// the route goes through `coordinator` and a fresh `work.ready`
/// is emitted before `work.done(step-02)`.
///
/// See `docs/plans/2026-07-02-001-fix-hat-routing-next-hop-plan.md` U2 / R2.
#[test]
fn test_hat_routing_next_hop_regression() {
    let yaml = load_scenario("tests/scenarios/2026-07-02-hat-routing-next-hop.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-02-004 plan U5: gate pass path — proposed → gate → real event.
#[test]
fn test_precheck_gate_pass() {
    let yaml = load_scenario("tests/scenarios/2026-07-02-precheck-gate-pass.yml");
    run_workflow_guard_scenario(yaml);
}

/// 2026-07-02-004 plan U6 / AE2: retry budget exhaustion → plan.blocked.
#[test]
fn test_precheck_gate_exhaust() {
    let yaml = load_scenario("tests/scenarios/2026-07-02-precheck-gate-exhaust.yml");
    run_workflow_guard_scenario(yaml);
}

/// plan 2026-07-29-001 U7: ce-executor-pipeline fail-gate rejected→pass path.
/// Executor's `work.failed.proposed` is rejected once by the synthesized
/// precheck gate, then re-emitted with sufficient evidence and forwarded.
#[test]
fn test_ce_executor_pipeline_fail_gate_rejected_then_pass() {
    let yaml =
        load_scenario("tests/scenarios/ce_executor_pipeline_fail_gate_rejected_then_pass.yml");
    run_workflow_guard_scenario(yaml);
}

/// plan 2026-07-29-001 U7: ce-executor-pipeline fail-gate exhaust path.
/// Three consecutive rejections burn the retry budget; runtime emits
/// plan.blocked(kind=precheck_exhausted) and work.failed never lands.
#[test]
fn test_ce_executor_pipeline_fail_gate_exhaust() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_fail_gate_exhaust.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-07-04-002 plan U9 (R11 BDD): ME-1 macro-edge next_hint
// → ## NEXT ACTION prompt injection。Emitter hat 在 business
// event payload 携带 `next_hint`;下游 hat 的 prompt 顶部应当
// 出现 `## NEXT ACTION` heading 与 hint 原文(per
// `prepend_macro_next_hint` 实现契约: ≤120 chars, scan
// backwards 取最近)。
#[test]
fn test_opac_me1_macro_edge_next_hint() {
    let yaml = load_scenario("tests/scenarios/opac/macro_edge_next_hint.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-07-04-002 plan U9 (R11 BDD): SB-1 supervisor exec wave
// fan-out 3 unit → exec.wave.complete. 走 `run_workflow_guard_scenario`
// 真 EventLoop runner,通过 supervisor.enabled: true 触发
// `run_bdd_supervisor_fan_in`,让 SupervisorCoordinator 真的
// emit `exec.wave.complete` (而不是 mock 伪造)。
// 验证 event_topic_counts: exec.unit.ready ×3, exec.unit.done ×3,
// exec.wave.complete ×1。
#[test]
fn test_opac_sb1_supervisor_exec_wave_fanout() {
    let yaml = load_scenario("tests/scenarios/supervisor/supervisor_exec_wave_fanout.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-07-04-002 plan U9 (R11 BDD): SB-2 review batch +
// agent-forged `review.wave.complete` 被 origin guard reject
// (R-COORD-2 / U7)。`review.complete` (agent business topic)
// accept;`review.wave.complete` (supervisor-only coordination
// topic) 被拒收,绝不能进入 seen_topics。
#[test]
fn test_opac_sb2_supervisor_review_batch_origin_guard() {
    let yaml = load_scenario("tests/scenarios/supervisor/supervisor_review_batch.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-07-23-005 plan U2 (R-B1 BDD): task-planner writes the
// execution-plan artifact on the happy path and emits NO
// business topic. The artifact at
// `.ralph/review/<plan-key>/execution-plan.yml` is the new
// dependency SSOT; downstream routing reads the artifact, not
// a per-emit handoff. This fixture pins the topology
// (task-planner no longer publishes `exec.unit.ready` — that
// ownership moves to U5).
#[test]
fn test_u2_task_planner_writes_execution_plan_artifact() {
    let yaml = load_scenario("tests/scenarios/supervisor/u2_task_planner_artifact_happy_path.yml");
    run_workflow_guard_scenario(yaml);
}

// 2026-07-23-005 plan U3 (R-B1 BDD): task-planner fail-closes
// on invalid DAG inputs (cycle, self-dep, unknown-dep,
// no-ready). The fixture emits a single `plan.blocked` with a
// strict `reason` enum; the runtime rejects any reason outside
// the enum set.
#[test]
fn test_u3_task_planner_rejects_invalid_dag() {
    let yaml = load_scenario("tests/scenarios/supervisor/u3_task_planner_rejects_invalid_dag.yml");
    run_workflow_guard_scenario(yaml);
}

// Unit 2 (plan 2026-07-07-006): pipeline scenario's mock `work.done`
// payload must carry every unit-evidence field the executor mode
// promises. Read-only fixture check; the fixture cannot drift away
// from the schema without this assertion failing first.
//
// Field set is sourced from the shared
// `ralph_core::test_support::unit_evidence::UNIT_EVIDENCE_FIELDS`
// SSOT (fix-plan U7 / SR-M1) so this BDD and `ralph-cli`'s lock
// test reference the same constant.

/// Extract every mock-response `work.done` payload object from the
/// pipeline fixture YAML by simple text scan (the fixture uses raw
/// `{"..."}` blocks). Returns the set of top-level keys present.
fn pipeline_work_done_payload_keys() -> std::collections::BTreeSet<String> {
    let text = fs::read_to_string("tests/scenarios/ce_executor_pipeline.yml")
        .expect("read ce_executor_pipeline.yml fixture");
    // Find each `<event topic="work.done">` block.
    let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for segment in text.split("<event topic=\"work.done\">").skip(1) {
        let body = segment.split("</event>").next().unwrap_or("");
        // Cheap JSON-key extraction: substring between { and the first "}".
        let json = body
            .split('{')
            .nth(1)
            .and_then(|s| s.split('}').next())
            .unwrap_or("");
        for kv in json.split(',') {
            let key = kv.split(':').next().unwrap_or("").trim().trim_matches('"');
            if !key.is_empty() {
                keys.insert(key.to_string());
            }
        }
    }
    keys
}

/// SC1 scenario half: the pipeline happy-path fixture's mock
/// `work.done` payload must already include every unit-evidence
/// field the executor mode promises. Drift here is a regression
/// because the fixture is the contract surface downstream hats
/// consume.
#[test]
fn test_pipeline_work_done_payload_carries_unit_evidence() {
    let keys = pipeline_work_done_payload_keys();
    let needed: std::collections::BTreeSet<String> =
        ralph_core::test_support::unit_evidence::UNIT_EVIDENCE_FIELDS
            .iter()
            .map(|s| s.to_string())
            .collect();
    let missing: Vec<&String> = needed.difference(&keys).collect();
    assert!(
        missing.is_empty(),
        "ce_executor_pipeline.yml work.done mock payload is missing unit evidence fields: {missing:?}. \
         Per plan 2026-07-07-006 Unit 2, the executor mode requires these in work.done."
    );
}

/// SC1 blocked-scenario half: the `work.failed` payload must carry
/// at least `{plan_name, reason}` so downstream can attribute the
/// failure to the originating plan, plus the 2026-07-24-002 U1
/// dead-end contract fields (`completed_units`, `decisions_file`)
/// so the fixture cannot drift back to a deliverable run claiming
/// `work.failed`.
#[test]
fn test_pipeline_work_failed_payload_minimal() {
    let text = fs::read_to_string("tests/scenarios/ce_executor_pipeline_blocked.yml")
        .expect("read ce_executor_pipeline_blocked.yml fixture");
    let segment = text
        .split("<event topic=\"work.failed\">")
        .nth(1)
        .expect("ce_executor_pipeline_blocked.yml must contain a work.failed mock event");
    let body = segment.split("</event>").next().unwrap_or("");
    let json = body
        .split('{')
        .nth(1)
        .and_then(|s| s.split('}').next())
        .unwrap_or("");
    let keys: std::collections::BTreeSet<String> = json
        .split(',')
        .filter_map(|kv| {
            let key = kv.split(':').next()?.trim().trim_matches('"');
            if key.is_empty() {
                None
            } else {
                Some(key.to_string())
            }
        })
        .collect();
    for required in ["plan_name", "reason", "completed_units", "decisions_file"] {
        assert!(
            keys.contains(required),
            "ce_executor_pipeline_blocked.yml work.failed mock payload is missing \
             `{required}`; got keys = {keys:?}"
        );
    }
}

// Unit 3 (plan 2026-07-07-006): serial-only scenarios are removed from
// the scenarios registry. Lock the cleanup so future contributors
// cannot silently re-add serial-only fixtures to a single-chain world.

const SERIAL_ONLY_PATH_FRAGMENTS: &[&str] = &[
    "ce_executor_serial",
    "serial_phase",
    "2026-06-29-007",
    "2026-06-30-001",
    "2026-07-01-002",
    "2026-07-06-task-not-terminal-coordinator",
    "2026-07-07-004-u1-coordinator",
];

fn registered_scenario_paths() -> Vec<String> {
    // Static scan: every `load_scenario("tests/scenarios/<path>")` call
    // in this file. We deliberately use a textual scan instead of an
    // `inventory_scenario!`-style macro so the test reads the actual
    // registration surface and stays robust to future macro refactors.
    // Skip comment lines and lines that only mention `tests/scenarios/`
    // inside doc strings or error messages.
    let text = fs::read_to_string("tests/scenarios.rs").expect("read scenarios.rs");
    let mut paths: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        if !line.contains("load_scenario(") {
            continue;
        }
        if let Some(start) = line.find("tests/scenarios/") {
            let rest = &line[start..];
            let mut end = rest.len();
            for (i, c) in rest.char_indices() {
                if c == '"' {
                    end = i;
                    break;
                }
            }
            let path = rest[..end].to_string();
            if !path.is_empty() {
                paths.push(path);
            }
        }
    }
    paths
}

#[test]
fn test_no_serial_only_scenario_registration() {
    let registered = registered_scenario_paths();
    let serial_only: Vec<&String> = registered
        .iter()
        .filter(|p| {
            SERIAL_ONLY_PATH_FRAGMENTS
                .iter()
                .any(|frag| p.contains(frag))
        })
        .collect();
    assert!(
        serial_only.is_empty(),
        "serial-only scenarios must be removed in Unit 3; remaining = {serial_only:?}"
    );
}

#[test]
fn test_retained_scenarios_pipeline_or_generic_only() {
    // Every remaining scenario registration must belong to one of
    // {pipeline, supervisor, generic}. We rely on filename convention;
    // preset binding is asserted by `run_workflow_guard_scenario` at
    // runtime.
    const GENERIC_PATH_PREFIXES: &[&str] = &[
        "tests/scenarios/autoresearch",
        "tests/scenarios/ce-executor-worktree",
        "tests/scenarios/ce_executor_bootstrap_recovery",
        "tests/scenarios/ce_executor_recovery",
        "tests/scenarios/hat-routing",
        "tests/scenarios/hat_lifecycle_contract",
        "tests/scenarios/preset_static_lint",
        "tests/scenarios/multi_hat_isolation_lint",
        "tests/scenarios/isolated_",
        "tests/scenarios/default_publishes",
        "tests/scenarios/multi_hat",
        "tests/scenarios/orphaned_events",
        "tests/scenarios/solo_mode",
        "tests/scenarios/precheck-gate",
        "tests/scenarios/plan_gate_",
        "tests/scenarios/step_handoff/",
        "tests/scenarios/flow_reliability/",
        "tests/scenarios/four-p0-guards/",
        "tests/scenarios/mechanism/",
        "tests/scenarios/u6_coordinator_",
        "tests/scenarios/2026-07-02-",
        // 2026-07-28-001 plan U2/U3: parallel-forge exec_wave branch
        // and success/failed chain BDD fixtures are fixture-neutral
        // (custom hat topology in the yaml) so they qualify as
        // generic (not pipeline, not supervisor).
        "tests/scenarios/parallel_forge_",
        "tests/scenarios/isolated_with_event_projection",
        // correction/diagnose 模块的通用行为(fixture-neutral,U8 已恢复
        // 三个 pipeline-named 测试条目,见 d294be76 的 commit message)
        "tests/scenarios/correction_",
        "tests/scenarios/diagnose_from_ledger",
        // payload_consistency 门的通用行为(fixture-neutral,抽象 topic/rule,
        // 不绑定任何 builtin preset;plan 2026-07-22-004 U4)
        "tests/scenarios/payload_consistency/",
        // 2026-08-08-004 plan U1/U2: scope boundary fixture files added in U1
        // (scope_payload_contract.yml, scope_agent_contract.yml) — abstract fixtures
        // with routing characterization, not bound to builtin preset; schema validation
        // via unit/CLI tests
        "tests/scenarios/scope_",
        // 2026-08-08-004 plan U4: post-merge scope classification fixtures
        // (postmerge_scope_mixed_history.yml, postmerge_scope_blocked.yml,
        // postmerge_scope_drift.yml) — generic routing characterization with
        // isolated postmerge hat chain; schema validation via unit/CLI tests
        "tests/scenarios/postmerge_scope_",
        // 2026-08-08-004 plan U5: red-team independent scope fixtures
        // (redteam_scope_direct_target.yml, redteam_scope_placeholder_blocked.yml)
        // — generic routing characterization; plan-resolver manifest + real
        // scope_base_sha without merge-batch boundary
        "tests/scenarios/redteam_scope_",
        // red-team failure sink handoff fixture validates the real runtime
        // reporter path.
        "tests/scenarios/redteam_failed_",
        // red-team experiment queue fixture validates the explicit serial
        // continuation edge between evidence-gate and experiment-runner.
        "tests/scenarios/redteam_experiment_queue",
        // 2026-08-08-004 plan U2: merge-batch boundary manifest routing
        // (abstract fixture with merge-batch hat chain; schema validation via unit/CLI tests)
        "tests/scenarios/merge_batch_boundary",
        // implementation-review preset (6-hat wave review, non-pipeline / non-supervisor)
        // 非 pipeline / 非 supervisor）
        "tests/scenarios/implementation_review_",
        // 2026-07-24-003 plan U8: wave protocol 通用场景
        // (normal_apply_confirm / recovery_required),跨 preset 共用
        "tests/scenarios/wave_protocol/",
    ];
    const SUPERVISOR_PATH_PREFIXES: &[&str] =
        &["tests/scenarios/opac/", "tests/scenarios/supervisor/"];
    const PIPELINE_PATH_PREFIXES: &[&str] = &["tests/scenarios/ce_executor_pipeline"];
    let registered = registered_scenario_paths();
    let offenders: Vec<&String> = registered
        .iter()
        .filter(|p| {
            let is_generic = GENERIC_PATH_PREFIXES.iter().any(|g| p.starts_with(g));
            let is_supervisor = SUPERVISOR_PATH_PREFIXES.iter().any(|g| p.starts_with(g));
            let is_pipeline = PIPELINE_PATH_PREFIXES.iter().any(|g| p.starts_with(g));
            !(is_generic || is_supervisor || is_pipeline)
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "every retained scenario must be pipeline, supervisor, or generic; offenders = {offenders:?}"
    );
}

// 2026-07-07-006 fix-plan U8 (R9 / SR-T2-fix): the three U9 correction
// scenarios were dropped wholesale when the `ce_executor_serial_*`
// fixtures were purged (1b91e51b), but they exercise the correction
// module's general behavior — deterministic escalation, three-step
// escalation, and `ralph diagnose --from-ledger` ledger replay — not
// anything serial-specific. The fixtures are already neutral
// (`correction_*.yml` / `diagnose_from_ledger.yml`), so we restore
// the test entries with pipeline-named identifiers and the
// correction-module test helpers that already exist in
// `ralph_core::correction::set_correction_enabled_for_test` and
// `event_loop::loop_state::set_lint_circuit_breaker_limit_for_test`.

fn enable_deterministic_correction_for_pipeline_scenario() {
    ralph_core::correction::set_correction_enabled_for_test(true);
}

/// BDD regression: a recoverable rejection accumulates a
/// `CorrectionContext` and the next prompt contains the
/// `## ORCHESTRATOR CORRECTION` block. Generic correction-module
/// behavior; re-locked against the pipeline fixture path
/// (correction_deterministic.yml is fixture-neutral).
#[test]
fn test_ce_executor_pipeline_u9_correction_deterministic_scenario() {
    enable_deterministic_correction_for_pipeline_scenario();
    let yaml = load_scenario("tests/scenarios/correction_deterministic.yml");
    run_workflow_guard_scenario(yaml);
}

/// BDD regression: three rejections on the same retry_key flip
/// the correction block's `needs_escalation` flag and render the
/// `ESCALATION` annotation line in the next prompt. Mirrors
/// U9 of the original serial plan but exercises pipeline paths.
#[test]
fn test_ce_executor_pipeline_u9_correction_three_escalation_scenario() {
    ralph_core::event_loop::loop_state::set_lint_circuit_breaker_limit_for_test(3);
    enable_deterministic_correction_for_pipeline_scenario();
    let yaml = load_scenario("tests/scenarios/correction_three_escalation.yml");
    run_workflow_guard_scenario(yaml);
}

/// BDD regression: `ralph diagnose --from-ledger` reads the
/// workspace `.ralph/recovery.jsonl` and surfaces a U7a rejection
/// record whose `reason_code` matches the CLI binary's
/// T8.1/T8.3 contract. Generic ledger-to-diagnose mapping; safe
/// to keep alongside the pipeline BDD surface.
#[test]
fn test_ce_executor_pipeline_u9_diagnose_from_ledger_scenario() {
    enable_deterministic_correction_for_pipeline_scenario();
    let yaml = load_scenario("tests/scenarios/diagnose_from_ledger.yml");
    run_workflow_guard_scenario(yaml);
}

/// 线性 preset：任一 mandatory dimension finding product 缺失时，
/// review-synthesizer 只能发送 `review.artifact.blocked`，不得用 P3
/// 或 ignore 占位 finding 合成 verdict；下游不得进入 fix、accept 或
/// alignment，最终由 reporter 生成 blocked 报告并完成 loop。
#[test]
fn test_ce_executor_pipeline_review_artifact_blocked() {
    let yaml = load_scenario("tests/scenarios/ce_executor_pipeline_review_artifact_blocked.yml");
    run_workflow_guard_scenario(yaml);
}

/// Loop preset：在多轮 review/fix 流程中验证同一 fail-close 契约。
/// 阻塞路径必须直接退出 loop，不得开启下一轮 fix，也不得进入 alignment。
#[test]
fn test_ce_executor_pipeline_loop_review_artifact_blocked() {
    let yaml =
        load_scenario("tests/scenarios/ce_executor_pipeline_loop_review_artifact_blocked.yml");
    run_workflow_guard_scenario(yaml);
}

/// U3 (plan 2026-08-08-004 §Unit 3 §9): direct-target / no-merge-boundary
/// scope resolution. Two plans on a direct commit chain; change-mapper
/// derives scope_base from first-parent topology, writes scope-manifest.json
/// and diff patches, and emits resolved postmerge.changemap.ready with all
/// U1 scope fields. The EventLoop accepts the event and the loop completes.
#[test]
fn test_postmerge_scope_direct_target_without_merge_boundary() {
    let yaml = load_scenario("tests/scenarios/postmerge_scope_direct_target.yml");
    run_workflow_guard_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-08-08-004 plan Unit 1: scope handoff consistency RED tests
// ──────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────
// 2026-08-08-004 plan Unit 2: merge-batch boundary manifest routing
// ──────────────────────────────────────────────────────────────────────

/// U2 (plan 2026-08-08-004 §Unit 2 §9): merge_batch_boundary routing characterization.
/// SUCCESS path: integrator emits merge.integrated with complete boundary
/// (merge_boundary_path/digest/status + integration_complete=true); the
/// EventLoop accepts the event; stabilizer and reporter complete normally.
/// FAILURE path: integrator emits integration_complete=false and
/// merge_boundary_status=incomplete; the EventLoop accepts the event;
/// stabilizer short-circuits with passed:false; reporter emits
/// merge.batch.complete(success:false) — NOT a false-success path.
/// Uses the real EventLoop via `run_workflow_guard_scenario` (not the
/// `run_scenario` stub) to exercise the `payload_consistency` rules on
/// merge.integrated and merge.stabilized.
#[test]
fn test_merge_batch_boundary_payload_and_failure_path() {
    let yaml = load_scenario("tests/scenarios/merge_batch_boundary.yml");
    run_workflow_guard_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-08-08-004 plan Unit 4: mixed/unknown hard gate
// ──────────────────────────────────────────────────────────────────────

/// U4 (plan 2026-08-08-004 §Unit 4 §9): mixed interleaved history.
/// Two plans with merge commits between them; change-mapper classifies
/// merge commits as interleaved, emits resolved scope with
/// interleaved_commits_count>0, all classification diff paths,
/// critical_unknown_count=0, proceed:true. The EventLoop accepts the event
/// and the loop completes.
#[test]
fn test_postmerge_scope_mixed_history_classifies_interleaved() {
    let yaml = load_scenario("tests/scenarios/postmerge_scope_mixed_history.yml");
    run_workflow_guard_scenario(yaml);
}

/// U4 (§Unit 4 §9): critical unknown hunk blocks audit.
/// A hunk in a critical path cannot be attributed to any known plan.
/// change-mapper emits blocked postmerge.changemap.ready with
/// critical_unknown_count>0, proceed:false. system-auditor short-circuits;
/// closer/reporter complete with verdict:FAIL.
#[test]
fn test_postmerge_scope_unknown_hunk_blocks_audit() {
    let yaml = load_scenario("tests/scenarios/postmerge_scope_blocked.yml");
    run_workflow_guard_scenario(yaml);
}

/// U4 (§Unit 4 §9): pre-emit drift recheck aborts resolved scope.
/// HEAD/tree changed during scope resolution; the pre-emit drift recheck
/// aborts before the resolved event is emitted. change-mapper emits blocked
/// scope instead. No stale resolved event ever reaches the EventLoop.
#[test]
fn test_postmerge_scope_drift_blocks_before_emit() {
    let yaml = load_scenario("tests/scenarios/postmerge_scope_drift.yml");
    run_workflow_guard_scenario(yaml);
}

/// U1 Red: merge.integrated without merge_boundary_path/digest/status
/// must be rejected. RED: current schema does NOT require these fields
/// so the incomplete payload passes. After Step 2 schema extensions and
/// guard, the payload is rejected and this test PASSES.
#[test]
fn test_scope_payload_contract_merge_integrated() {
    let yaml = load_scenario("tests/scenarios/scope_payload_contract.yml");
    run_workflow_guard_scenario(yaml);
}

/// U1 Red: postmerge.changemap.ready without scope manifest fields
/// must be rejected. RED: current schema does NOT require scope_*
/// fields so the incomplete payload passes. After Step 2 schema
/// extensions and guard, the payload is rejected and this test PASSES.
#[test]
fn test_scope_agent_contract_postmerge_changemap() {
    let yaml = load_scenario("tests/scenarios/scope_agent_contract.yml");
    run_workflow_guard_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-08-08-004 plan Unit 5: red-team independent scope
// ──────────────────────────────────────────────────────────────────────

/// U5 (plan 2026-08-08-004 §Unit 5 §9): red-team plan-resolver
/// resolves scope independently without merge_boundary_path. target-locker
/// locks HEAD/tree; plan-resolver derives scope_base from explicit input,
/// writes scope-manifest.json and full-current.patch from real scope_base_sha,
/// and emits redteam.plan.resolved with all U1 scope fields.
/// attack-surface-mapper activates (resolved path). Loop reaches redteam.complete.
#[test]
fn test_redteam_scope_direct_target_without_merge_boundary() {
    let yaml = load_scenario("tests/scenarios/redteam_scope_direct_target.yml");
    run_workflow_guard_scenario(yaml);
}

/// U5 (plan 2026-08-08-004 §Unit 5 §9): red-team plan-resolver
/// emits unresolved when scope_base is a placeholder. target-locker locks;
/// plan-resolver detects `<global-baseline>` placeholder and emits
/// redteam.plan.unresolved with reason=SCOPE_BASE_PLACEHOLDER.
/// attack-surface-mapper does NOT activate (unresolved path).
/// Loop reaches redteam.complete(success:false).
#[test]
fn test_redteam_scope_placeholder_blocked() {
    let yaml = load_scenario("tests/scenarios/redteam_scope_placeholder_blocked.yml");
    run_workflow_guard_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-08-08-004 plan Unit 6: mixed/conflict/confidence gates
// ──────────────────────────────────────────────────────────────────────

/// U6 (plan 2026-08-08-004 §Unit 6 §9): mixed direct/merge history.
/// plan-resolver classifies interleaved commits, emits resolved scope
/// with coverage=92, boundary_conflict=false, critical_unknown_count=0,
/// overall_confidence=90. attack-surface-mapper activates (resolved path).
/// Loop reaches redteam.complete.
#[test]
fn test_redteam_scope_mixed_history_classifies_interleaved() {
    let yaml = load_scenario("tests/scenarios/redteam_scope_mixed_history.yml");
    run_workflow_guard_scenario(yaml);
}

/// U6 (§Unit 6 §9): boundary conflict blocks attack.mapped.
/// plan-resolver emits scope with boundary_conflict=true (cross_check=0);
/// payload_consistency rule rejects; attack-surface-mapper does NOT activate.
/// Loop reaches redteam.complete(success:false).
#[test]
fn test_redteam_scope_boundary_conflict_unresolved() {
    let yaml = load_scenario("tests/scenarios/redteam_scope_boundary_conflict.yml");
    run_workflow_guard_scenario(yaml);
}

/// U6 (§Unit 6 §9): critical unknown hunk blocks attack.mapped.
/// plan-resolver emits scope with critical_unknown_count=1;
/// payload_consistency rule rejects; attack-surface-mapper does NOT activate.
/// Loop reaches redteam.complete(success:false).
#[test]
fn test_redteam_scope_unknown_hunk_blocks_attack() {
    let yaml = load_scenario("tests/scenarios/redteam_scope_unknown_blocked.yml");
    run_workflow_guard_scenario(yaml);
}

/// A producer failure is a first-class red-team business event. The real
/// EventLoop routes the shared `redteam.failed` sink to the reporter, which
/// closes the preset through the `redteam.complete(success=false)` contract.
#[test]
fn test_redteam_failed_reaches_reporter() {
    let yaml = load_scenario("tests/scenarios/redteam_failed_reporter.yml");
    run_workflow_guard_scenario(yaml);
}

/// The red-team evidence gate must record one experiment and explicitly route
/// the next queued experiment before it emits the aggregate evidence handoff.
#[test]
fn test_redteam_experiment_queue_continues_before_aggregate_gate() {
    let yaml = load_scenario("tests/scenarios/redteam_experiment_queue.yml");
    run_workflow_guard_scenario(yaml);
}

/// U1 (plan 2026-08-08-004 fix-plan §Unit 1, A3) + U3 (plan
/// 2026-08-10-002 §Unit 3, R2/R6/D5): the `redteam.attack.mapped`
/// HARD GATE is now a runtime contract via the new
/// `ne`-form payload_consistency rule
/// `redteam-attack-mapped-predecessor-must-be-resolved` +
/// schema-required `predecessor_event` field. This fixture emits
/// `redteam.attack.mapped` WITHOUT `predecessor_event`; the schema
/// `required_fields` rejects the emit; attack.mapped is rejected;
/// experiment-runner does not activate. Loop falls through to
/// `redteam.complete(success:false)`.
///
/// Companion to:
///   - `test_redteam_scope_attack_mapped_gate_rejects_wrong_predecessor`
///     (wrong literal → rejected by `ne` rule)
///   - `test_redteam_scope_attack_mapped_legal_predecessor_accepted`
///     (legal literal → accepted; experiment-runner activates; loop
///     reaches `redteam.complete(success:true)`).
#[test]
fn test_redteam_scope_attack_mapped_gate_rejects_missing_predecessor() {
    let yaml = load_scenario("tests/scenarios/redteam_scope_attack_mapped_gate.yml");
    run_workflow_guard_scenario(yaml);
}

/// U3 (plan 2026-08-10-002 §Unit 3, R2/R6/D5): the new
/// `ne`-form payload_consistency rule
/// `redteam-attack-mapped-predecessor-must-be-resolved` rejects any
/// `predecessor_event` literal that is not `"redteam.plan.resolved"`.
/// This fixture emits `redteam.attack.mapped` WITH the wrong literal
/// `"redteam.plan.unresolved"`; schema accepts the field (it IS
/// present), but the `ne` rule fires; attack.mapped is rejected;
/// experiment-runner does not activate. Loop falls through to
/// `redteam.complete(success:false)`.
#[test]
fn test_redteam_scope_attack_mapped_gate_rejects_wrong_predecessor() {
    let yaml = load_scenario("tests/scenarios/redteam_scope_attack_mapped_wrong_predecessor.yml");
    run_workflow_guard_scenario(yaml);
}

/// U3 (plan 2026-08-10-002 §Unit 3, R1/R6/D5): the legal predecessor
/// literal `"redteam.plan.resolved"` Misses the new `ne` rule, so
/// `redteam.attack.mapped` lands; experiment-runner activates;
/// evidence-gate → impact-boundary → independent-reviewer → reporter.
/// Loop reaches `redteam.complete(success:true)`.
///
/// This fixture closes the gap exposed by E10 (plan 2026-08-10-002):
/// the previous `exists:true AND eq` form Hit on the legal literal
/// and rejected it. Before U3, this fixture would have failed at
/// the `redteam.attack.mapped` step (rule would fire on the legal
/// value). After U3, the legal literal passes and the full chain
/// reaches `redteam.complete(success:true)`.
#[test]
fn test_redteam_scope_attack_mapped_legal_predecessor_accepted() {
    let yaml = load_scenario("tests/scenarios/redteam_scope_attack_mapped_legal_predecessor.yml");
    run_workflow_guard_scenario(yaml);
}
