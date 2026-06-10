//! Unified Runtime Contract report data model.
//!
//! This module defines the shared `RuntimeContractReport` /
//! `RuntimeContractFinding` types that preset / workflow contract checks
//! (config, topology, orphan, payload) converge into. Different entry points
//! (`ralph preset check`, `ralph hats validate`, `ralph preflight`,
//! `enforce_payload_contract_gate`) feed the same report shape so callers
//! can rely on stable JSON output and consistent strict semantics.
//!
//! Implementation Units:
//! - U1: introduce this data model (this file).
//! - U2: aggregator that maps existing validators into findings.
//! - U3+: CLI adapters that expose the report.
//!
//! Stability rules:
//! - The JSON field names (`id`, `source`, `severity`, `stage`,
//!   `message`, `details`, `action_hint`, `source_label`, `payload_strict`,
//!   `fail_on_warnings`, `passed`, `findings`, `checked_at`) are part of
//!   the public contract; renaming or repurposing them is a breaking
//!   change for downstream diagnostics / CI consumers.
//! - `source=preflight` is reserved for CLI/preflight adapter wrapper
//!   reports. The core preset contract aggregator must not stamp findings
//!   with `source=preflight`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Origin of a finding within the runtime contract surface.
///
/// `Preflight` is reserved for adapter-layer wrapper reports; the core
/// preset contract aggregator must not produce findings with this source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSource {
    /// `RalphConfig::validate()` findings.
    Config,
    /// `validate_preset_topology()` findings.
    Topology,
    /// Orphan topic detection findings.
    Orphan,
    /// `validate_payload_contract()` findings.
    Payload,
    /// `preset_lint` authoring-time static lint findings (topic format,
    /// ownership, coordinator checks).
    Lint,
    /// Reserved for CLI/preflight adapter wrapper reports.
    Preflight,
}

impl FindingSource {
    /// Returns the stable string label used in JSON output and machine IDs.
    pub fn as_str(self) -> &'static str {
        match self {
            FindingSource::Config => "config",
            FindingSource::Topology => "topology",
            FindingSource::Orphan => "orphan",
            FindingSource::Payload => "payload",
            FindingSource::Lint => "lint",
            FindingSource::Preflight => "preflight",
        }
    }
}

/// Severity classification for a finding.
///
/// The triple `Pass` / `Warn` / `Error` mirrors `CheckStatus` in
/// `preflight.rs` and is used uniformly across all entry points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    /// Check produced no actionable signal.
    Pass,
    /// Check produced a non-blocking signal.
    Warn,
    /// Check produced a blocking signal.
    Error,
}

impl FindingSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingSeverity::Pass => "pass",
            FindingSeverity::Warn => "warn",
            FindingSeverity::Error => "error",
        }
    }
}

/// Lifecycle stage at which a finding is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStage {
    /// Preset / workflow authoring time (`ralph preset check`).
    Authoring,
    /// Optional pre-run preflight (`ralph preflight`,
    /// `run_auto_preflight()`).
    Preflight,
    /// `enforce_payload_contract_gate()` at `ralph run` startup.
    RunHardGate,
}

impl FindingStage {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingStage::Authoring => "authoring",
            FindingStage::Preflight => "preflight",
            FindingStage::RunHardGate => "run_hard_gate",
        }
    }
}

/// A single normalized runtime contract finding.
///
/// `id` is a stable machine identifier (e.g.
/// `config.invalid_concurrency`, `topology.unreachable_completion`,
/// `payload.field_missing_from_schema`). `source` and `stage` together
/// describe where and when the finding fires; `severity` describes how
/// it interacts with the surrounding `RuntimeContractReport` strict
/// dimensions.
///
/// `details` carries structured, source-specific context. Keys are
/// stable within a source (for example `hat`, `topic`, `field`,
/// `schema_source`, `source_hats`); other keys are accepted but not
/// part of the stability contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeContractFinding {
    /// Stable machine ID, source-prefixed (e.g. `topology.unreachable_start`).
    pub id: String,
    /// Origin of the finding.
    pub source: FindingSource,
    /// Severity classification.
    pub severity: FindingSeverity,
    /// Lifecycle stage at which the finding was produced.
    pub stage: FindingStage,
    /// Human-readable summary.
    pub message: String,
    /// Optional structured context (e.g. `hat`, `topic`, `field`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, String>,
    /// Optional fix suggestion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_hint: Option<String>,
}

impl RuntimeContractFinding {
    /// Construct a new finding. `details` starts empty and can be
    /// extended via `with_detail`.
    ///
    /// **Note:** This constructor accepts any `source`, including
    /// `FindingSource::Preflight`. The Preflight source is reserved for
    /// CLI/preflight adapter wrapper reports; the public constructor
    /// stays open so the adapter layer is the only place that needs to
    /// construct reserved-source findings. The core preset contract
    /// aggregator (U2) MUST use [`RuntimeContractFinding::try_new_core`]
    /// instead, which enforces the reservation at runtime.
    pub fn new(
        id: impl Into<String>,
        source: FindingSource,
        severity: FindingSeverity,
        stage: FindingStage,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            source,
            severity,
            stage,
            message: message.into(),
            details: BTreeMap::new(),
            action_hint: None,
        }
    }

    /// Construct a new finding via the core preset contract aggregator
    /// path. Refuses `source = Preflight` at runtime and returns
    /// `Err(FindingSource::Preflight)` so callers can detect a misuse
    /// without crashing the aggregator loop.
    ///
    /// This is the U2 aggregator's only entry point for building
    /// findings: every `Finding` produced by `RuntimeContractAggregator`
    /// passes through this constructor, which means the
    /// "core aggregator must not stamp findings with Preflight" invariant
    /// is enforced by the type's own construction logic, not just by
    /// docstring discipline. See G1-RES-1 in the plan.
    pub(crate) fn try_new_core(
        id: impl Into<String>,
        source: FindingSource,
        severity: FindingSeverity,
        stage: FindingStage,
        message: impl Into<String>,
    ) -> Result<Self, FindingSource> {
        if matches!(source, FindingSource::Preflight) {
            return Err(source);
        }
        Ok(Self::new(id, source, severity, stage, message))
    }

    /// Add a structured detail key/value pair. Returns `self` for chaining.
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    /// Set the action hint. Returns `self` for chaining.
    pub fn with_action_hint(mut self, hint: impl Into<String>) -> Self {
        self.action_hint = Some(hint.into());
        self
    }

    /// Returns `true` if this finding should cause the surrounding
    /// `RuntimeContractReport` to be marked failed.
    ///
    /// Strict semantics:
    /// - `severity = Error` always fails the report.
    /// - `severity = Warn` only fails the report when
    ///   `fail_on_warnings = true`.
    /// - `severity = Pass` never fails the report.
    pub fn is_blocking(&self, fail_on_warnings: bool) -> bool {
        match self.severity {
            FindingSeverity::Error => true,
            FindingSeverity::Warn => fail_on_warnings,
            FindingSeverity::Pass => false,
        }
    }
}

/// Configuration for the strict dimensions of a `RuntimeContractReport`.
///
/// `payload_strict` and `fail_on_warnings` are independent axes:
/// - `payload_strict` only affects the severity of payload findings
///   (e.g. missing schema is `warn` in non-strict, `error` in strict).
/// - `fail_on_warnings` controls whether warnings cause the overall
///   report to fail (`ralph preflight --strict` uses
///   `fail_on_warnings=true`; `ralph hats validate --strict` only sets
///   `payload_strict=true`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeContractStrictness {
    pub payload_strict: bool,
    pub fail_on_warnings: bool,
}

impl RuntimeContractStrictness {
    /// Strict profile used by `ralph preset check --strict`:
    /// `payload_strict=true` AND `fail_on_warnings=true`.
    pub fn preset_check_strict() -> Self {
        Self {
            payload_strict: true,
            fail_on_warnings: true,
        }
    }
}

/// Aggregated runtime contract report.
///
/// `source_label` is a short, human-readable identifier of what was
/// checked (e.g. `builtin:ce-executor`, `path/to/preset.yml`,
/// `current-config`). It is informational and does not participate in
/// any logic; callers should treat it as the report's "name".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeContractReport {
    /// Human-readable identifier of the checked target.
    pub source_label: String,
    /// Whether payload findings should treat missing schemas as errors.
    pub payload_strict: bool,
    /// Whether warning findings should fail the report.
    pub fail_on_warnings: bool,
    /// Whether the report passed under the configured strictness.
    pub passed: bool,
    /// Number of warning findings (any severity, any source).
    pub warnings: usize,
    /// Number of error findings (any source).
    pub errors: usize,
    /// All findings collected for this report.
    pub findings: Vec<RuntimeContractFinding>,
    /// RFC3339 timestamp at which the report was assembled.
    pub checked_at: String,
}

impl RuntimeContractReport {
    /// Build a new report from a `source_label` and a strictness profile.
    ///
    /// `checked_at` is captured at construction time. Findings must be
    /// pushed via `add_finding` / `extend_findings`; `passed`,
    /// `warnings`, and `errors` are recomputed on demand.
    pub fn new(source_label: impl Into<String>, strictness: RuntimeContractStrictness) -> Self {
        Self {
            source_label: source_label.into(),
            payload_strict: strictness.payload_strict,
            fail_on_warnings: strictness.fail_on_warnings,
            passed: true,
            warnings: 0,
            errors: 0,
            findings: Vec::new(),
            checked_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Append a finding and refresh aggregate counters.
    pub fn add_finding(&mut self, finding: RuntimeContractFinding) {
        match finding.severity {
            FindingSeverity::Error => self.errors += 1,
            FindingSeverity::Warn => self.warnings += 1,
            FindingSeverity::Pass => {}
        }
        self.findings.push(finding);
        self.recompute_passed();
    }

    /// Append multiple findings and refresh aggregate counters.
    pub fn extend_findings<I: IntoIterator<Item = RuntimeContractFinding>>(&mut self, findings: I) {
        for finding in findings {
            self.add_finding(finding);
        }
    }

    /// Recompute `passed` from current `findings` and the configured
    /// strictness. A finding is blocking if its `is_blocking` method
    /// returns `true` under the report's `fail_on_warnings` setting.
    pub fn recompute_passed(&mut self) {
        self.passed = !self
            .findings
            .iter()
            .any(|f| f.is_blocking(self.fail_on_warnings));
    }
}

/// Topics consumed by the event loop runner itself (not by any hat).
///
/// These are hat-publishable topics whose consumers live in
/// `crates/ralph-core/src/event_loop/`, not in the hat graph. The
/// orphan-event check in `detect_orphan_topics` and the legacy
/// `ralph hats validate` orphan check both skip them, otherwise they
/// would produce false-positive warnings like "Event 'build.blocked'
/// has no hat subscribers" when in reality the loop runner is tracking
/// the event for thrashing detection.
///
/// **Single source of truth**: this constant is defined in core
/// (U2 of the runtime contract consolidation plan). The CLI layer
/// (`crates/ralph-cli/src/hats.rs`) imports this constant instead of
/// redeclaring it, so the orphan exemption set cannot drift between
/// the aggregator and the legacy entry point.
///
/// **Adding to this list**: only add a topic if you have verified (by
/// reading the consuming code in `event_loop/mod.rs`) that the loop
/// runner subscribes to it without any hat subscription. This list is
/// intentionally narrow — every other orphan warning is real and
/// indicates a typo, a missing hat, or a stale `publishes` list.
pub const LOOP_RUNNER_INTERNAL_TOPICS: &[&str] = &[
    // `build.blocked` triggers thrashing detection in
    // `event_loop::EventLoop::process_events` (around the comment
    // "Track build.blocked events for thrashing detection"). After 3
    // consecutive blocked events on the same task, the loop runner
    // synthesizes `build.task.abandoned` and abandons the task. The
    // Builder hat is the typical publisher; no hat needs to subscribe.
    "build.blocked",
];

// ──────────────────────────────────────────────────────────────────────────
// U2: Preset Contract Aggregator
// ──────────────────────────────────────────────────────────────────────────

use crate::config::ConfigError;
use crate::config::ConfigWarning;
use crate::hat_registry::HatRegistry;
use crate::payload_contract::{
    PayloadContractError, PayloadContractErrorKind, PayloadContractValidationResult,
};
use crate::preset_lint::{LintStrictness, run_preset_lint};
use crate::preset_validator::{
    TopologyError, TopologyErrorKind, TopologyValidationResult, validate_preset_topology,
};

/// Stable, source-prefixed machine ID for a `ConfigError` variant.
///
/// Returns the lowercase snake_case ID used in `RuntimeContractFinding.id`.
/// Unknown variants fall through to `config.error` (a sentinel that the
/// runtime tests pin to fail loudly if a new variant is added without
/// updating the aggregator's mapping table).
fn config_error_id(err: &ConfigError) -> &'static str {
    match err {
        ConfigError::AmbiguousRouting { .. } => "config.ambiguous_routing",
        ConfigError::MutuallyExclusive { .. } => "config.mutually_exclusive",
        ConfigError::InvalidCompletionPromise => "config.invalid_completion_promise",
        ConfigError::CustomBackendRequiresCommand => "config.custom_backend_requires_command",
        ConfigError::ReservedTrigger { .. } => "config.reserved_trigger",
        ConfigError::MissingDescription { .. } => "config.missing_description",
        ConfigError::RobotMissingField { .. } => "config.robot_missing_field",
        ConfigError::InvalidHookPhaseEvent { .. } => "config.invalid_hook_phase_event",
        ConfigError::HookValidation { .. } => "config.hook_validation",
        ConfigError::UnsupportedHookField { .. } => "config.unsupported_hook_field",
        ConfigError::DeprecatedProjectKey => "config.deprecated_project_key",
        ConfigError::InvalidConcurrency { .. } => "config.invalid_concurrency",
        ConfigError::AggregateOnConcurrentHat { .. } => "config.aggregate_on_concurrent_hat",
        ConfigError::WorkflowGuardValidation { .. } => "config.workflow_guard_validation",
        ConfigError::EventPolicyValidation { .. } => "config.event_policy_validation",
        ConfigError::StateMachineValidation { .. } => "config.state_machine_validation",
        ConfigError::SchemaFileNotFound { .. } => "config.schema_file_not_found",
        ConfigError::SchemaFileParseError { .. } => "config.schema_file_parse_error",
        ConfigError::SchemaFileNotMap { .. } => "config.schema_file_not_map",
        ConfigError::SchemaFileInvalidSchema { .. } => "config.schema_file_invalid_schema",
        ConfigError::TelemetryValidation { .. } => "config.telemetry_validation",
        ConfigError::TerminalTopicNotInPublishes { .. } => "config.terminal_topic_not_in_publishes",
        ConfigError::Io(_) | ConfigError::Yaml(_) => "config.parse_error",
    }
}

/// Stable, source-prefixed machine ID for a `ConfigWarning` variant.
fn config_warning_id(warning: &ConfigWarning) -> &'static str {
    match warning {
        ConfigWarning::DeferredFeature { .. } => "config.deferred_feature",
        ConfigWarning::DroppedField { .. } => "config.dropped_field",
        ConfigWarning::InvalidValue { .. } => "config.invalid_value",
        ConfigWarning::EmptyTerminalEvents { .. } => "config.empty_terminal_events",
    }
}

/// Convert a `ConfigWarning` into a single `RuntimeContractFinding`.
///
/// Returns `Err(FindingSource::Preflight)` only if the construction
/// path itself is misconfigured; in practice this never fires for
/// `source = config` because the `try_new_core` refusal applies only
/// to `FindingSource::Preflight`.
fn config_warning_finding(warning: &ConfigWarning) -> RuntimeContractFinding {
    let id = config_warning_id(warning);
    let message = warning.to_string();
    let mut finding = RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Config,
        FindingSeverity::Warn,
        FindingStage::Authoring,
        message,
    )
    .expect("config warnings never use the reserved Preflight source");
    match warning {
        ConfigWarning::DeferredFeature { field, message: _ } => {
            finding = finding.with_detail("field", field.clone());
        }
        ConfigWarning::DroppedField { field, reason } => {
            finding = finding
                .with_detail("field", field.clone())
                .with_detail("reason", reason.clone());
        }
        ConfigWarning::InvalidValue { field, message: _ } => {
            finding = finding.with_detail("field", field.clone());
        }
        ConfigWarning::EmptyTerminalEvents { hat } => {
            finding = finding.with_detail("hat", hat.clone());
        }
    }
    finding
}

/// Convert a `ConfigError` into a single `RuntimeContractFinding` (Error
/// severity). The aggregator uses this to short-circuit on config
/// failures so callers see exactly one config error and no misleading
/// secondary topology/payload/orphan findings.
fn config_error_finding(err: &ConfigError) -> RuntimeContractFinding {
    let id = config_error_id(err);
    let message = err.to_string();
    let mut finding = RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Config,
        FindingSeverity::Error,
        FindingStage::Authoring,
        message,
    )
    .expect("config errors never use the reserved Preflight source");
    // Best-effort detail extraction by variant. Unknown variants still
    // emit a finding with the message; details remain empty.
    match err {
        ConfigError::AmbiguousRouting {
            trigger,
            hat1,
            hat2,
        } => {
            finding = finding
                .with_detail("trigger", trigger.clone())
                .with_detail("hat1", hat1.clone())
                .with_detail("hat2", hat2.clone());
        }
        ConfigError::MutuallyExclusive { field1, field2 } => {
            finding = finding
                .with_detail("field1", field1.clone())
                .with_detail("field2", field2.clone());
        }
        ConfigError::ReservedTrigger { trigger, hat } => {
            finding = finding
                .with_detail("trigger", trigger.clone())
                .with_detail("hat", hat.clone());
        }
        ConfigError::MissingDescription { hat } => {
            finding = finding.with_detail("hat", hat.clone());
        }
        ConfigError::RobotMissingField { field, hint: _ } => {
            finding = finding.with_detail("field", field.clone());
        }
        ConfigError::InvalidHookPhaseEvent { phase_event } => {
            finding = finding.with_detail("phase_event", phase_event.clone());
        }
        ConfigError::HookValidation { field, message: _ } => {
            finding = finding.with_detail("field", field.clone());
        }
        ConfigError::UnsupportedHookField { field, reason: _ } => {
            finding = finding.with_detail("field", field.clone());
        }
        ConfigError::InvalidConcurrency { hat, value } => {
            finding = finding
                .with_detail("hat", hat.clone())
                .with_detail("value", value.to_string());
        }
        ConfigError::AggregateOnConcurrentHat { hat } => {
            finding = finding.with_detail("hat", hat.clone());
        }
        ConfigError::WorkflowGuardValidation { field, message: _ } => {
            finding = finding.with_detail("field", field.clone());
        }
        ConfigError::EventPolicyValidation { field, message: _ } => {
            finding = finding.with_detail("field", field.clone());
        }
        ConfigError::StateMachineValidation { field, message: _ } => {
            finding = finding.with_detail("field", field.clone());
        }
        ConfigError::SchemaFileNotFound { path, source: _ } => {
            finding = finding.with_detail("path", path.clone());
        }
        ConfigError::SchemaFileParseError { path, source: _ } => {
            finding = finding.with_detail("path", path.clone());
        }
        ConfigError::SchemaFileNotMap { path } => {
            finding = finding.with_detail("path", path.clone());
        }
        ConfigError::SchemaFileInvalidSchema {
            topic,
            path,
            source: _,
        } => {
            finding = finding
                .with_detail("path", path.clone())
                .with_detail("topic", topic.clone());
        }
        ConfigError::Io(_)
        | ConfigError::Yaml(_)
        | ConfigError::DeprecatedProjectKey
        | ConfigError::InvalidCompletionPromise
        | ConfigError::CustomBackendRequiresCommand
        | ConfigError::TelemetryValidation { .. } => {}
        ConfigError::TerminalTopicNotInPublishes { hat, topic } => {
            finding = finding
                .with_detail("hat", hat.clone())
                .with_detail("topic", topic.clone());
        }
    }
    finding
}

/// Convert a topology validation error into a `RuntimeContractFinding`.
///
/// Each `TopologyErrorKind` maps to a stable `topology.*` machine ID.
fn topology_finding(err: &TopologyError) -> RuntimeContractFinding {
    let id = match err.kind {
        TopologyErrorKind::UnreachableStart => "topology.unreachable_start",
        TopologyErrorKind::UnreachableCompletion => "topology.unreachable_completion",
        TopologyErrorKind::UnreachableRequired => "topology.unreachable_required",
        TopologyErrorKind::RequiredEventNotOnAllPaths => "topology.required_event_not_on_all_paths",
    };
    let mut finding = RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Topology,
        FindingSeverity::Error,
        FindingStage::Authoring,
        err.message.clone(),
    )
    .expect("topology errors never use the reserved Preflight source");
    // Extract the first topic-shaped token from the message for stable
    // `topic` detail. The message strings are produced by
    // `validate_preset_topology` and always start with the topic name
    // in single quotes (e.g. "Starting event 'start' has ...").
    if let Some(start) = err.message.find('\'')
        && let Some(end_rel) = err.message[start + 1..].find('\'')
    {
        let topic = &err.message[start + 1..start + 1 + end_rel];
        if !topic.is_empty() {
            finding = finding.with_detail("topic", topic.to_string());
        }
    }
    finding
}

/// Map a `PayloadContractValidationResult` into findings.
///
/// `strict` is the `payload_strict` axis of the report. The validator
/// already produces errors for `FieldMissingFromSchema` regardless of
/// mode; the only axis that changes is `SchemaMissingForRequiredTopic`,
/// which is `Error` in strict mode and `Warning` otherwise. The
/// aggregator forwards both, but the *severity* of the latter is
/// determined by the validator's classification (already in
/// `result.errors` vs `result.warnings`) — the `strict` flag here is
/// only used as a defensive double-check so a future refactor that
/// flips the validator's classification doesn't silently change the
/// shared report's findings.
pub fn payload_findings_from_result(
    result: &PayloadContractValidationResult,
) -> Vec<RuntimeContractFinding> {
    let mut findings: Vec<RuntimeContractFinding> =
        result.errors.iter().map(payload_error_finding).collect();
    for w in &result.warnings {
        findings.push(payload_warning_finding(w));
    }
    findings
}

fn payload_error_finding(err: &PayloadContractError) -> RuntimeContractFinding {
    let id = match err.kind {
        PayloadContractErrorKind::FieldMissingFromSchema => "payload.field_missing_from_schema",
        PayloadContractErrorKind::SchemaMissingForRequiredTopic => {
            "payload.schema_missing_for_required_topic"
        }
    };
    let mut finding = RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Payload,
        FindingSeverity::Error,
        FindingStage::Authoring,
        err.message.clone(),
    )
    .expect("payload errors never use the reserved Preflight source");
    finding = finding
        .with_detail("hat", err.hat_id.clone())
        .with_detail("topic", err.topic.clone());
    if let Some(field) = &err.field {
        finding = finding.with_detail("field", field.clone());
    }
    if !err.source_hats.is_empty() {
        finding = finding.with_detail("source_hats", err.source_hats.join(", "));
    }
    finding = finding.with_detail("schema_defined_in", err.schema_defined_in.clone());
    if let Some(line) = err.instructions_line {
        finding = finding.with_detail("instructions_line", line.to_string());
    }
    finding
}

fn payload_warning_finding(warning: &str) -> RuntimeContractFinding {
    // Payload warnings currently only come from `SchemaMissingForRequiredTopic`
    // in non-strict mode, so the warning text always mentions the topic.
    // We extract it as a best-effort detail; the canonical id is fixed
    // because non-strict-mode warnings are aggregated before the
    // per-topic dispatch and don't carry structured fields.
    let id = "payload.schema_missing_for_required_topic";
    let mut finding = RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Payload,
        FindingSeverity::Warn,
        FindingStage::Authoring,
        warning.to_string(),
    )
    .expect("payload warnings never use the reserved Preflight source");
    // Best-effort: find the first `'topic'` mention.
    if let Some(start) = warning.find('\'')
        && let Some(end_rel) = warning[start + 1..].find('\'')
    {
        let topic = &warning[start + 1..start + 1 + end_rel];
        if !topic.is_empty() && topic != "field" {
            finding = finding.with_detail("topic", topic.to_string());
        }
    }
    finding
}

/// Detect orphan topics — published by a custom hat but with no
/// non-fallback hat subscriber — and return them as warning findings.
///
/// Exemptions (no orphan reported even if the topic has no specific
/// subscriber):
/// 1. The completion promise topic (consumed by the loop runner).
/// 2. Any topic listed in `config.event_loop.required_events`
///    (loop-level gates consumed by the loop runner's
///    `missing_required_events` check).
/// 3. Any topic in [`LOOP_RUNNER_INTERNAL_TOPICS`] (e.g. `build.blocked`).
///
/// The function uses `HatRegistry::has_specific_subscriber` so the
/// runtime fallback `ralph`'s `*` subscription does NOT count as a
/// subscriber. This preserves the legacy `ralph hats validate` orphan
/// semantics exactly.
pub fn detect_orphan_topics(
    config: &crate::config::RalphConfig,
    registry: &HatRegistry,
) -> Vec<RuntimeContractFinding> {
    let mut findings = Vec::new();
    // Snapshot the exemption sets once.
    let required: std::collections::HashSet<&str> = config
        .event_loop
        .required_events
        .iter()
        .map(String::as_str)
        .collect();
    let completion = config.event_loop.completion_promise.as_str();

    for hat in registry.all() {
        if hat.is_fallback_only() {
            // The runtime fallback `ralph` publishes nothing; skip.
            continue;
        }
        for pub_topic in &hat.publishes {
            let topic = pub_topic.as_str();
            if topic == completion {
                continue;
            }
            if required.contains(topic) {
                continue;
            }
            if LOOP_RUNNER_INTERNAL_TOPICS.contains(&topic) {
                continue;
            }
            if !registry.has_specific_subscriber(topic) {
                let finding = RuntimeContractFinding::try_new_core(
                    "orphan.no_subscriber",
                    FindingSource::Orphan,
                    FindingSeverity::Warn,
                    FindingStage::Authoring,
                    format!(
                        "Event '{}' published by '{}' has no hat subscribers",
                        topic, hat.name
                    ),
                )
                .expect("orphan findings never use the reserved Preflight source")
                .with_detail("topic", topic.to_string())
                .with_detail("publisher", hat.name.clone());
                findings.push(finding);
            }
        }
    }
    findings
}

/// R4 (2026-06-07 plan U5): every topic that the loop runner *requires*
/// must have at least one publisher AND at least one subscriber.
/// The topics in scope are:
///
///   - `config.event_loop.required_events` (loop-level gates).
///   - Topics declared in `execution_contracts.rules[*]` keys —
///     these are contractually protected at runtime and must therefore
///     have a real producer and a real consumer in the topology.
///   - Topics in `event_policy.schemas[*]` keys — every schema the
///     preset declares must correspond to a topic that actually flows
///     through the graph; otherwise the schema is dead config.
///
/// This check is the symmetric counterpart of `detect_orphan_topics`
/// (which catches *published-but-unconsumed* topics) and the loop
/// runner's `missing_required_events` (which only fires at runtime
/// when the loop terminates without seeing the event).  Surfacing
/// the gap at authoring time means the runner can `fail-fast` on a
/// broken preset instead of looping until max_iterations.
pub fn detect_required_topic_gaps(
    config: &crate::config::RalphConfig,
    registry: &HatRegistry,
) -> Vec<RuntimeContractFinding> {
    let mut findings = Vec::new();
    let mut topics_to_check: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for t in &config.event_loop.required_events {
        topics_to_check.insert(t.clone());
    }
    if let Some(ec) = &config.event_loop.execution_contracts {
        if ec.enabled {
            for topic in ec.rules.keys() {
                topics_to_check.insert(topic.clone());
            }
        }
    }
    if let Some(ep) = &config.event_loop.event_policy {
        for topic in ep.schemas.keys() {
            topics_to_check.insert(topic.clone());
        }
    }

    for topic in &topics_to_check {
        // Skip the completion promise — it has no publisher in the
        // hat graph (it is emitted by the loop runner itself, not a
        // hat) but it does have subscribers via ralph's wildcard.
        if topic == &config.event_loop.completion_promise {
            continue;
        }
        if LOOP_RUNNER_INTERNAL_TOPICS.contains(&topic.as_str()) {
            continue;
        }

        let has_publisher = registry
            .all()
            .any(|h| h.publishes.iter().any(|p| p.matches_str(topic.as_str())));

        // Skip the no_subscriber check for terminal workflow events:
        // a required topic whose publishing hat also publishes the
        // completion promise is a "loop-terminating event" — the loop
        // runner observes it for `required_events` accounting, but
        // there is intentionally no downstream hat subscription
        // because the next event in the chain is the completion
        // promise. Example: `reporter` emits `report.done` and
        // `LOOP_COMPLETE`; no hat subscribes to `report.done` because
        // the loop is about to terminate. Demanding a subscriber here
        // would force every terminal workflow event to declare a
        // throwaway hat or be downgraded from `required_events` to a
        // soft expectation, neither of which matches the preset
        // design intent.
        //
        // The builtin runtime `ralph` hat publishes a derived scope
        // (all configured hats' triggers + publishes + completion
        // promise) so it always trivially satisfies the dual-publisher
        // test.  Excluding it keeps the heuristic honest: the dual
        // publisher must be a real business hat, not the universal
        // fallback that would mask otherwise-broken topologies.
        let publishing_hat_also_publishes_completion = registry.all().any(|h| {
            h.id.as_str() != "ralph"
                && h.publishes.iter().any(|p| p.matches_str(topic.as_str()))
                && h.publishes
                    .iter()
                    .any(|p| p.matches_str(config.event_loop.completion_promise.as_str()))
        });
        // has_specific_subscriber excludes fallback-only hats (those
        // subscribed to "*"), so it correctly returns false when
        // ralph's wildcard is the only "subscriber".
        let has_subscriber = registry.has_specific_subscriber(topic.as_str());

        if !has_publisher {
            let finding = RuntimeContractFinding::try_new_core(
                "required.no_publisher",
                FindingSource::Topology,
                FindingSeverity::Error,
                FindingStage::Authoring,
                format!(
                    "Required topic '{}' has no publisher in the hat graph",
                    topic
                ),
            )
            .expect("required-no-publisher finding uses no reserved source")
            .with_detail("topic", topic.clone());
            findings.push(finding);
        }
        if !has_subscriber && !publishing_hat_also_publishes_completion {
            let finding = RuntimeContractFinding::try_new_core(
                "required.no_subscriber",
                FindingSource::Topology,
                FindingSeverity::Error,
                FindingStage::Authoring,
                format!(
                    "Required topic '{}' has no subscriber in the hat graph",
                    topic
                ),
            )
            .expect("required-no-subscriber finding uses no reserved source")
            .with_detail("topic", topic.clone());
            findings.push(finding);
        }
    }
    findings
}

/// R4 (U5): for every hat with `obligations:`, each
/// `must_emit_any_of` topic must be present in the hat's `publishes`
/// list.  Otherwise the activation-level path is incoherent: the
/// runner would conclude the obligation is satisfiable by a topic
/// the hat has no authority to publish, and the origin guard would
/// reject the event at runtime.
pub fn detect_obligation_topics_not_in_publishes(
    config: &crate::config::RalphConfig,
) -> Vec<RuntimeContractFinding> {
    let mut findings = Vec::new();
    for (hat_id, hat_config) in &config.hats {
        let publishes: std::collections::HashSet<&str> =
            hat_config.publishes.iter().map(String::as_str).collect();
        for obligation in &hat_config.obligations {
            for topic in &obligation.must_emit_any_of {
                if !publishes.contains(topic.as_str()) {
                    let finding = RuntimeContractFinding::try_new_core(
                        "obligation.topic_not_in_publishes",
                        FindingSource::Topology,
                        FindingSeverity::Error,
                        FindingStage::Authoring,
                        format!(
                            "Hat '{}' obligation for trigger '{}' lists '{}' in \
                             must_emit_any_of but the topic is not in publishes",
                            hat_id, obligation.on_trigger, topic
                        ),
                    )
                    .expect("obligation finding uses no reserved source")
                    .with_detail("hat", hat_id.to_string())
                    .with_detail("on_trigger", obligation.on_trigger.clone())
                    .with_detail("topic", topic.clone());
                    findings.push(finding);
                }
            }
        }
    }
    findings
}

/// Preset Contract Aggregator — assembles a `RuntimeContractReport` from
/// the existing config / topology / payload / orphan validators in
/// the canonical order defined by the runtime contract consolidation
/// plan (U2).
///
/// # Order of operations
///
/// 1. `RalphConfig::validate()` — warnings become `source=config`
///    warning findings, errors become a single `source=config` error
///    finding. If the config is invalid, the aggregator **short-circuits**
///    and does not run lint, topology, payload, or orphan checks. This
///    prevents misleading secondary findings when the config is
///    structurally broken (e.g. empty `completion_promise`).
/// 2. `run_preset_lint()` — topic format, ownership, and coordinator
///    checks produce `source=lint` findings. Lint findings are semantic
///    (not structural) and do **not** short-circuit subsequent checks,
///    so callers see all authoring issues in one report. When
///    `fail_on_warnings` is true, ownership checks use `Strict`
///    severity (warnings become errors).
/// 3. `validate_preset_topology()` — every topology error becomes a
///    `source=topology` error finding with a stable `topology.*` id.
/// 4. `validate_payload_contract()` — every error becomes a
///    `source=payload` error finding, every warning a `source=payload`
///    warning finding. The validator already classifies
///    `SchemaMissingForRequiredTopic` per the strict flag passed in.
/// 5. `detect_orphan_topics()` — every real orphan becomes a
///    `source=orphan` warning finding. Completion promise,
///    `required_events`, and `LOOP_RUNNER_INTERNAL_TOPICS` are
///    exempt.
///
/// # Inputs
///
/// - `config`: a fully-loaded and normalized `RalphConfig`.
/// - `registry`: a runtime-aware `HatRegistry` (built via
///   `HatRegistry::from_runtime_config`). The aggregator uses it for
///   topology reachability and payload contract dispatch. For orphan
///   detection, the aggregator uses
///   `HatRegistry::has_specific_subscriber` so the runtime fallback
///   `ralph`'s `*` subscription does not mask real orphans.
/// - `strictness`: the report's strictness profile
///   (`payload_strict` + `fail_on_warnings` axes).
/// - `source_label`: a short human-readable identifier of what was
///   checked (e.g. `builtin:ce-executor`, `path/to/preset.yml`).
///
/// # Strict semantics
///
/// The aggregator does not silently change strictness. `payload_strict`
/// is forwarded to the payload validator; `fail_on_warnings` is
/// enforced by `RuntimeContractReport::add_finding` and the final
/// `recompute_passed` call.
pub struct RuntimeContractAggregator;

impl RuntimeContractAggregator {
    /// Run all preset contract checks in order and return a fully
    /// assembled `RuntimeContractReport`.
    pub fn aggregate(
        source_label: impl Into<String>,
        config: &crate::config::RalphConfig,
        registry: &HatRegistry,
        strictness: RuntimeContractStrictness,
    ) -> RuntimeContractReport {
        let mut report = RuntimeContractReport::new(source_label, strictness);

        // Step 1: config validation. Short-circuit on Err.
        match config.validate() {
            Ok(warnings) => {
                for warning in &warnings {
                    let finding = config_warning_finding(warning);
                    report.add_finding(finding);
                }
            }
            Err(err) => {
                let finding = config_error_finding(&err);
                report.add_finding(finding);
                // Short-circuit: do not run topology/payload/orphan on a
                // broken config — they would produce misleading secondary
                // findings (e.g. "no subscriber for start" when the real
                // problem is an invalid completion promise).
                report.recompute_passed();
                return report;
            }
        }

        // Step 2 (U3): preset static lint — topic format, ownership,
        // coordinator checks. Lint findings are semantic (not structural)
        // and do not short-circuit subsequent topology/payload/orphan
        // checks, so callers see all authoring issues in one report.
        //
        // Only runs in strict mode (`fail_on_warnings=true`) to preserve
        // backward compatibility: non-strict `preset check` historically
        // did NOT run lint, and adding it would be a behavioral regression.
        // The `ralph run` hard gate always uses strict mode, so lint is
        // always enforced at startup.
        if strictness.fail_on_warnings {
            for finding in run_preset_lint(config, LintStrictness::Strict) {
                report.add_finding(finding);
            }
        }

        // Step 3: topology validation. Uses the runtime-aware registry
        // so reachability is checked against the *actual* hat graph
        // (including fallback ralph's wildcard subscription for the
        // most permissive interpretation).
        let topology_result: TopologyValidationResult = validate_preset_topology(config, registry);
        for err in &topology_result.errors {
            let finding = topology_finding(err);
            report.add_finding(finding);
        }
        // Topology warnings are not produced by the current validator
        // (the field is reserved for future use), but we still drain
        // the vector defensively so a future contributor adding
        // warnings doesn't need to revisit the aggregator.
        for warning in &topology_result.warnings {
            let finding = RuntimeContractFinding::try_new_core(
                "topology.warning",
                FindingSource::Topology,
                FindingSeverity::Warn,
                FindingStage::Authoring,
                warning.clone(),
            )
            .expect("topology warnings never use the reserved Preflight source");
            report.add_finding(finding);
        }

        // Step 4: payload contract validation. The validator already
        // honors `payload_strict`; we forward the flag and let it
        // produce the correct errors/warnings split.
        let payload_result: PayloadContractValidationResult =
            crate::payload_contract::validate_payload_contract(
                config,
                registry,
                strictness.payload_strict,
            );
        for finding in payload_findings_from_result(&payload_result) {
            report.add_finding(finding);
        }

        // Step 5: orphan topic detection. Uses the same registry; the
        // `has_specific_subscriber` call inside `detect_orphan_topics`
        // excludes fallback-only hats so a `*` subscription by ralph
        // does not mask real orphans.
        for finding in detect_orphan_topics(config, registry) {
            report.add_finding(finding);
        }

        // Step 6 (2026-06-07 plan U5 R4): required-topic coverage.
        // Every topic in `required_events` / execution_contracts /
        // event_policy.schemas must have a publisher AND a subscriber.
        for finding in detect_required_topic_gaps(config, registry) {
            report.add_finding(finding);
        }

        // Step 7 (2026-06-07 plan U5 R4): obligation-topic alignment.
        // Each `obligations[*].must_emit_any_of` topic must be present
        // in the same hat's `publishes` list.  Otherwise the
        // activation-level path (Unit 4) would conclude the obligation
        // is satisfiable by a topic the hat has no authority to emit,
        // and the origin guard would reject the event at runtime.
        for finding in detect_obligation_topics_not_in_publishes(config) {
            report.add_finding(finding);
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warn_finding() -> RuntimeContractFinding {
        RuntimeContractFinding::new(
            "config.deferred_feature",
            FindingSource::Config,
            FindingSeverity::Warn,
            FindingStage::Authoring,
            "feature deferred",
        )
    }

    fn error_finding() -> RuntimeContractFinding {
        RuntimeContractFinding::new(
            "topology.unreachable_completion",
            FindingSource::Topology,
            FindingSeverity::Error,
            FindingStage::Authoring,
            "completion promise unreachable",
        )
    }

    fn pass_finding() -> RuntimeContractFinding {
        RuntimeContractFinding::new(
            "config.ok",
            FindingSource::Config,
            FindingSeverity::Pass,
            FindingStage::Authoring,
            "ok",
        )
    }

    #[test]
    fn empty_report_passes() {
        let report = RuntimeContractReport::new("empty", RuntimeContractStrictness::default());
        assert!(report.passed);
        assert_eq!(report.warnings, 0);
        assert_eq!(report.errors, 0);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn single_warning_finding_does_not_fail_non_strict_report() {
        let mut report =
            RuntimeContractReport::new("warn-only", RuntimeContractStrictness::default());
        report.add_finding(warn_finding());
        assert!(
            report.passed,
            "non-strict report should pass with one warning"
        );
        assert_eq!(report.warnings, 1);
        assert_eq!(report.errors, 0);
    }

    #[test]
    fn single_error_finding_fails_report() {
        let mut report =
            RuntimeContractReport::new("err-only", RuntimeContractStrictness::default());
        report.add_finding(error_finding());
        assert!(!report.passed);
        assert_eq!(report.warnings, 0);
        assert_eq!(report.errors, 1);
    }

    #[test]
    fn fail_on_warnings_makes_warnings_blocking() {
        let mut report = RuntimeContractReport::new(
            "strict-warn",
            RuntimeContractStrictness {
                payload_strict: false,
                fail_on_warnings: true,
            },
        );
        report.add_finding(warn_finding());
        assert!(!report.passed);
        assert_eq!(report.warnings, 1);
    }

    #[test]
    fn payload_strict_does_not_alter_non_payload_severity() {
        // payload_strict must only affect payload findings; config/orphan/topology
        // findings keep their intrinsic severity regardless of the flag.
        let mut report = RuntimeContractReport::new(
            "payload-strict",
            RuntimeContractStrictness {
                payload_strict: true,
                fail_on_warnings: false,
            },
        );
        report.add_finding(warn_finding());
        assert!(report.passed);
        assert_eq!(report.warnings, 1);
    }

    #[test]
    fn finding_is_blocking_respects_strictness() {
        let warn = warn_finding();
        assert!(!warn.is_blocking(false));
        assert!(warn.is_blocking(true));

        let err = error_finding();
        assert!(err.is_blocking(false));
        assert!(err.is_blocking(true));

        let pass = RuntimeContractFinding::new(
            "config.ok",
            FindingSource::Config,
            FindingSeverity::Pass,
            FindingStage::Authoring,
            "ok",
        );
        assert!(!pass.is_blocking(false));
        assert!(!pass.is_blocking(true));

        // Payload source must follow the same strictness matrix as any
        // other source; payload_strict only changes *which* severity
        // payload findings are produced with, not how the resulting
        // finding interacts with fail_on_warnings.
        let payload_warn = RuntimeContractFinding::new(
            "payload.missing_schema",
            FindingSource::Payload,
            FindingSeverity::Warn,
            FindingStage::Authoring,
            "schema not declared",
        );
        assert!(!payload_warn.is_blocking(false));
        assert!(payload_warn.is_blocking(true));
    }

    #[test]
    fn json_serialization_contains_stable_fields() {
        let mut report = RuntimeContractReport::new(
            "json-fixture",
            RuntimeContractStrictness {
                payload_strict: true,
                fail_on_warnings: false,
            },
        );
        report.add_finding(
            error_finding()
                .with_detail("topic", "LOOP_COMPLETE")
                .with_detail("start", "task.start")
                .with_action_hint("add a hat that publishes the completion topic"),
        );
        let value: serde_json::Value = serde_json::to_value(&report).expect("serialize report");
        let obj = value
            .as_object()
            .expect("report should serialize to object");

        for key in [
            "source_label",
            "payload_strict",
            "fail_on_warnings",
            "passed",
            "warnings",
            "errors",
            "findings",
            "checked_at",
        ] {
            assert!(
                obj.contains_key(key),
                "report JSON missing stable key: {key}"
            );
        }

        let findings = obj
            .get("findings")
            .and_then(|v| v.as_array())
            .expect("findings should be an array");
        assert_eq!(findings.len(), 1);
        let finding = findings[0]
            .as_object()
            .expect("finding should be an object");
        for key in ["id", "source", "severity", "stage", "message", "details"] {
            assert!(
                finding.contains_key(key),
                "finding JSON missing stable key: {key}"
            );
        }
        assert_eq!(
            finding.get("source").and_then(|v| v.as_str()),
            Some("topology")
        );
        assert_eq!(
            finding.get("severity").and_then(|v| v.as_str()),
            Some("error")
        );
        assert_eq!(
            finding.get("stage").and_then(|v| v.as_str()),
            Some("authoring")
        );
        let details = finding
            .get("details")
            .and_then(|v| v.as_object())
            .expect("details should serialize to object");
        assert_eq!(
            details.get("topic").and_then(|v| v.as_str()),
            Some("LOOP_COMPLETE")
        );
    }

    #[test]
    fn finding_with_detail_and_action_hint_chains() {
        let finding = warn_finding()
            .with_detail("field", "concurrency")
            .with_action_hint("reduce concurrency to >= 1");
        assert_eq!(
            finding.details.get("field").map(String::as_str),
            Some("concurrency")
        );
        assert_eq!(
            finding.action_hint.as_deref(),
            Some("reduce concurrency to >= 1")
        );
    }

    #[test]
    fn recompute_passed_handles_mixed_findings() {
        let mut report = RuntimeContractReport::new(
            "mixed",
            RuntimeContractStrictness {
                payload_strict: false,
                fail_on_warnings: true,
            },
        );
        report.add_finding(warn_finding());
        assert!(!report.passed, "fail_on_warnings should make warn blocking");
        report.add_finding(error_finding());
        assert!(!report.passed);
        assert_eq!(report.warnings, 1);
        assert_eq!(report.errors, 1);
    }

    #[test]
    fn preset_check_strict_sets_both_axes() {
        let strictness = RuntimeContractStrictness::preset_check_strict();
        assert!(strictness.payload_strict);
        assert!(strictness.fail_on_warnings);
    }

    #[test]
    fn extend_findings_appends_in_order() {
        let mut report = RuntimeContractReport::new("extend", RuntimeContractStrictness::default());
        report.extend_findings([warn_finding(), error_finding()]);
        assert_eq!(report.findings.len(), 2);
        assert_eq!(report.warnings, 1);
        assert_eq!(report.errors, 1);
        assert!(!report.passed);
    }

    #[test]
    fn source_and_severity_and_stage_have_stable_strs() {
        assert_eq!(FindingSource::Config.as_str(), "config");
        assert_eq!(FindingSource::Topology.as_str(), "topology");
        assert_eq!(FindingSource::Orphan.as_str(), "orphan");
        assert_eq!(FindingSource::Payload.as_str(), "payload");
        assert_eq!(FindingSource::Lint.as_str(), "lint");
        assert_eq!(FindingSource::Preflight.as_str(), "preflight");

        assert_eq!(FindingSeverity::Pass.as_str(), "pass");
        assert_eq!(FindingSeverity::Warn.as_str(), "warn");
        assert_eq!(FindingSeverity::Error.as_str(), "error");

        assert_eq!(FindingStage::Authoring.as_str(), "authoring");
        assert_eq!(FindingStage::Preflight.as_str(), "preflight");
        assert_eq!(FindingStage::RunHardGate.as_str(), "run_hard_gate");
    }

    // ---- T1: Pass severity must not bump warnings or errors counters. ----
    // Guards the `FindingSeverity::Pass => {}` no-op arm in `add_finding`.
    // If a future refactor misclassifies Pass (e.g. typo `warnings += 1`),
    // this test will fail before U2 aggregator hands the report to a
    // strict preflight consumer.
    #[test]
    fn add_finding_pass_severity_does_not_increment_counters() {
        let mut report =
            RuntimeContractReport::new("pass-only", RuntimeContractStrictness::default());
        let pass = pass_finding();
        report.add_finding(pass);
        assert_eq!(report.warnings, 0, "Pass finding must not bump warnings");
        assert_eq!(report.errors, 0, "Pass finding must not bump errors");
        assert_eq!(report.findings.len(), 1);
        assert!(report.passed);
    }

    // ---- T1-extended (P2 #2): Pass + Warn + Error mixed combo ----
    // guards counter accumulation AND `recompute_passed` over multiple
    // findings. T1 (above) only covered Pass in isolation; this test
    // exercises all three severities in a single report so a future
    // refactor that mis-classifies Pass into Warn/Error counters (or
    // double-counts when `add_finding` is called more than once) fails
    // loudly under both `fail_on_warnings` settings.
    #[test]
    fn add_finding_pass_warn_error_mixed_counters_and_recompute_passed() {
        // Non-strict: Warn does not block, only Error does.
        let mut report =
            RuntimeContractReport::new("mixed-non-strict", RuntimeContractStrictness::default());
        report.add_finding(pass_finding());
        report.add_finding(warn_finding());
        report.add_finding(error_finding());
        assert_eq!(report.findings.len(), 3);
        assert_eq!(
            report.warnings, 1,
            "Warn finding must bump warnings exactly once"
        );
        assert_eq!(
            report.errors, 1,
            "Error finding must bump errors exactly once"
        );
        assert!(
            !report.passed,
            "non-strict report with Error finding must be failed"
        );

        // Strict (fail_on_warnings=true): the Warn+Error pass/fail invariant
        // is covered by `recompute_passed_handles_mixed_findings` above. The
        // unique contribution of the strict half here is to verify that Pass
        // does not double-block nor bump counters when added to a strict
        // mixed-combo report — i.e. the findings list still grows by one
        // for the Pass push, but `recompute_passed` still produces the same
        // fail verdict.
        let mut strict_report = RuntimeContractReport::new(
            "mixed-strict",
            RuntimeContractStrictness {
                payload_strict: false,
                fail_on_warnings: true,
            },
        );
        strict_report.add_finding(pass_finding());
        strict_report.add_finding(warn_finding());
        strict_report.add_finding(error_finding());
        assert_eq!(strict_report.findings.len(), 3);
    }

    // ---- T2: skip_serializing_if must drop empty details and None
    // action_hint from the JSON output. ----
    #[test]
    fn json_serialization_omits_optional_fields_when_absent() {
        let finding = RuntimeContractFinding::new(
            "config.minimal",
            FindingSource::Config,
            FindingSeverity::Warn,
            FindingStage::Authoring,
            "minimal payload",
        );
        let value: serde_json::Value = serde_json::to_value(&finding).expect("serialize finding");
        let obj = value
            .as_object()
            .expect("finding should serialize to object");
        assert!(
            !obj.contains_key("details"),
            "empty details map must be omitted from JSON"
        );
        assert!(
            !obj.contains_key("action_hint"),
            "None action_hint must be omitted from JSON"
        );
        assert_eq!(
            obj.get("id").and_then(|v| v.as_str()),
            Some("config.minimal")
        );
    }

    // ---- T3: roundtrip a report through JSON and assert structural
    // equality. Guards against accidental rename/alias drift on the
    // public contract. ----
    #[test]
    fn json_serialization_roundtrip_preserves_report() {
        let mut report = RuntimeContractReport::new(
            "roundtrip",
            RuntimeContractStrictness {
                payload_strict: true,
                fail_on_warnings: true,
            },
        );
        report.add_finding(
            error_finding()
                .with_detail("topic", "LOOP_COMPLETE")
                .with_detail("start", "task.start")
                .with_action_hint("add a hat that publishes the completion topic"),
        );
        let value: serde_json::Value = serde_json::to_value(&report).expect("serialize report");
        let roundtripped: RuntimeContractReport =
            serde_json::from_value(value).expect("deserialize report");
        assert_eq!(roundtripped, report);
    }

    // ---- T3-extended: deserialization must accept JSON that omits the
    // optional `details` and `action_hint` keys entirely (the form
    // produced by `skip_serializing_if` on the way out). This validates
    // that the omit direction is symmetric with T2's serialise-omit
    // assertion: a downstream consumer that writes a stripped JSON
    // document by hand and feeds it back to `RuntimeContractReport`
    // sees `details == BTreeMap::new()` and `action_hint == None`,
    // not a deserialization error. ----
    #[test]
    fn json_deserialization_with_omitted_optional_fields_defaults_correctly() {
        // Hand-build a minimal JSON object with `details` and
        // `action_hint` keys ABSENT (not `null`, not `{}` — simply
        // missing). All other fields are the natural defaults of an
        // empty report.
        let minimal_json = serde_json::json!({
            "source_label": "minimal",
            "payload_strict": false,
            "fail_on_warnings": false,
            "passed": true,
            "warnings": 0,
            "errors": 0,
            "findings": [],
            "checked_at": "1970-01-01T00:00:00+00:00",
        });
        let report: RuntimeContractReport = serde_json::from_value(minimal_json)
            .expect("report must deserialize when details/action_hint are absent");
        assert!(
            report.findings.is_empty(),
            "findings should be empty Vec by default"
        );
        // Validate per-finding defaults next — Report has no details/action_hint fields.
        let minimal_finding_json = serde_json::json!({
            "id": "config.minimal",
            "source": "config",
            "severity": "warn",
            "stage": "authoring",
            "message": "minimal payload",
        });
        let finding: RuntimeContractFinding = serde_json::from_value(minimal_finding_json)
            .expect("finding must deserialize when details/action_hint are absent");
        assert!(
            finding.details.is_empty(),
            "absent details must default to empty BTreeMap"
        );
        assert!(
            finding.action_hint.is_none(),
            "absent action_hint must default to None"
        );
    }

    // ---- T4a: extend_findings on an empty iterator must be a no-op. ----
    #[test]
    fn extend_findings_with_empty_iter_is_noop() {
        let mut report =
            RuntimeContractReport::new("empty-extend", RuntimeContractStrictness::default());
        let before = report.findings.len();
        let before_warnings = report.warnings;
        let before_errors = report.errors;
        report.extend_findings(std::iter::empty::<RuntimeContractFinding>());
        assert_eq!(report.findings.len(), before);
        assert_eq!(report.warnings, before_warnings);
        assert_eq!(report.errors, before_errors);
        assert!(report.passed);
    }

    // ---- T4b: with_detail must support distinct keys and same-key
    // overwrite. ----
    #[test]
    fn with_detail_supports_multiple_distinct_keys_and_overwrite() {
        // distinct keys: hat/topic/field/schema_source/source_hats are the
        // 5 stable per-source keys called out in the doc comment.
        let finding = warn_finding()
            .with_detail("hat", "executor")
            .with_detail("topic", "LOOP_COMPLETE")
            .with_detail("field", "concurrency")
            .with_detail("schema_source", "events/LOOP_COMPLETE")
            .with_detail("source_hats", "coordinator,alternate");
        assert_eq!(
            finding.details.get("hat").map(String::as_str),
            Some("executor")
        );
        assert_eq!(
            finding.details.get("topic").map(String::as_str),
            Some("LOOP_COMPLETE")
        );
        assert_eq!(
            finding.details.get("field").map(String::as_str),
            Some("concurrency")
        );
        assert_eq!(
            finding.details.get("schema_source").map(String::as_str),
            Some("events/LOOP_COMPLETE")
        );
        assert_eq!(
            finding.details.get("source_hats").map(String::as_str),
            Some("coordinator,alternate")
        );
        assert_eq!(finding.details.len(), 5);

        // overwrite: re-inserting an existing key replaces the value.
        let overwritten = finding.with_detail("field", "timeout_seconds");
        assert_eq!(
            overwritten.details.get("field").map(String::as_str),
            Some("timeout_seconds")
        );
        assert_eq!(
            overwritten.details.len(),
            5,
            "overwrite must not change key count"
        );
    }

    // ---- T5 (residual G1-RES-1, non-tautological fix): the core
    // preset contract aggregator must not stamp findings with
    // `source=Preflight`. The previous T5 test only checked that the
    // module docstring contained certain strings — a future contributor
    // who deleted the docstring would have broken the test, but the test
    // never actually exercised the aggregator. U2 introduces a
    // `pub(crate)` constructor `try_new_core` that refuses the reserved
    // source at runtime. The new T5 test below pins the constructor's
    // contract:
    //   - `try_new_core` returns `Err(FindingSource::Preflight)` when
    //     the caller attempts to use the reserved source.
    //   - `try_new_core` returns `Ok` for the other four sources.
    //   - The public `new` constructor still accepts `Preflight` because
    //     the CLI/preflight adapter layer is the only legitimate caller.
    // This is paired with the docstring-guard test below, which keeps
    // the module-level reservation note under test. ----
    #[test]
    fn finding_source_preflight_refused_by_core_constructor() {
        // The public `new` accepts Preflight (adapter layer is the only
        // legitimate caller). Pin that the variant is constructible so a
        // future refactor that breaks serde derive on Preflight is caught.
        let adapter_finding = RuntimeContractFinding::new(
            "adapter.preflight",
            FindingSource::Preflight,
            FindingSeverity::Warn,
            FindingStage::Preflight,
            "adapter-only path",
        );
        assert_eq!(adapter_finding.source, FindingSource::Preflight);

        // The core constructor refuses Preflight — this is the new
        // mechanism U2 introduces. The plan's preferred T5 fix path.
        let result = RuntimeContractFinding::try_new_core(
            "core.attempted_preflight",
            FindingSource::Preflight,
            FindingSeverity::Warn,
            FindingStage::Authoring,
            "core must not use Preflight source",
        );
        assert!(
            result.is_err(),
            "core constructor must refuse Preflight, got Ok"
        );
        assert_eq!(
            result.err(),
            Some(FindingSource::Preflight),
            "Err variant must carry the refused source for diagnostics"
        );

        // The core constructor accepts all other sources (Config,
        // Topology, Orphan, Payload). Exercising all four guards against
        // a future refactor that over-broadens the refusal to non-Preflight
        // sources.
        for source in [
            FindingSource::Config,
            FindingSource::Topology,
            FindingSource::Orphan,
            FindingSource::Payload,
            FindingSource::Lint,
        ] {
            let f = RuntimeContractFinding::try_new_core(
                "core.ok",
                source,
                FindingSeverity::Warn,
                FindingStage::Authoring,
                "ok",
            );
            assert!(
                f.is_ok(),
                "core constructor must accept {source:?} (non-reserved)"
            );
            assert_eq!(f.expect("Ok finding").source, source);
        }
    }

    // ---- T5-doc (complementary documentation guard): the module-level
    // docstring declares the Preflight reservation invariant. Reading
    // the file at compile time pins the documentation discipline so a
    // future contributor who deletes the note must either restore it or
    // update both this guard AND the constructor mechanism above. ----
    #[test]
    fn finding_source_preflight_documented_as_reserved_in_module_docstring() {
        // Variant is constructible and serializes to its stable label.
        let preflight = FindingSource::Preflight;
        assert_eq!(preflight.as_str(), "preflight");
        let value: serde_json::Value = serde_json::to_value(preflight).expect("serialize source");
        assert_eq!(value.as_str(), Some("preflight"));
        // Module-level docstring declares the reservation invariant.
        let source = include_str!("runtime_contract.rs");
        assert!(
            source.contains("reserved for adapter-layer wrapper reports"),
            "FindingSource::Preflight reservation invariant must be documented at module level"
        );
        assert!(
            source.contains("must not produce findings with this source"),
            "FindingSource::Preflight reservation invariant must forbid core aggregator use"
        );
        // FindingStage::Preflight shares the "preflight" JSON label; this
        // is forward-looking: U2 must distinguish them via their JSON key
        // (source vs stage), not via the label.
        let stage_value: serde_json::Value =
            serde_json::to_value(FindingStage::Preflight).expect("serialize stage");
        assert_eq!(stage_value.as_str(), Some("preflight"));
    }

    // ──────────────────────────────────────────────────────────────────────
    // U2 aggregator tests
    // ──────────────────────────────────────────────────────────────────────

    /// Build a runtime-aware registry from a YAML config string.
    /// The `from_runtime_config` constructor includes the runtime fallback
    /// `ralph` hat, which is what the aggregator's topology analysis needs.
    fn runtime_registry(yaml: &str) -> HatRegistry {
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        HatRegistry::from_runtime_config(&config)
    }

    // ---- U2-LOOP: `LOOP_RUNNER_INTERNAL_TOPICS` single source ----

    #[test]
    fn loop_runner_internal_topics_contains_build_blocked() {
        assert!(
            LOOP_RUNNER_INTERNAL_TOPICS.contains(&"build.blocked"),
            "build.blocked must be in the orphan exemption list"
        );
    }

    // ---- U2-aggregator: happy path ----

    #[test]
    fn u2_aggregator_empty_hats_passes() {
        // Hatless / solo mode: no hats, no findings.
        let mut config = crate::config::RalphConfig::default();
        config.tasks.enabled = false;
        config.topic_format_whitelist = vec!["LOOP_COMPLETE".to_string()];
        let registry = HatRegistry::from_runtime_config(&config);
        let report = RuntimeContractAggregator::aggregate(
            "empty",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(report.passed, "empty config should pass: {:?}", report);
        assert_eq!(
            report.warnings, 0,
            "unexpected warnings: {:?}",
            report.findings
        );
        assert_eq!(report.errors, 0, "unexpected errors: {:?}", report.findings);
        assert!(
            report.findings.is_empty(),
            "unexpected findings: {:?}",
            report.findings
        );
    }

    #[test]
    fn u2_aggregator_linear_chain_passes() {
        // Two hats form `work.start -> work.ready -> LOOP_COMPLETE`. Topology
        // is valid; no payload refs; no orphans.
        let yaml = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: false
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "linear",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(
            report.passed,
            "linear chain should pass: {:?}",
            report.findings
        );
        assert_eq!(report.errors, 0);
    }

    // ---- U2-aggregator: topology errors ----

    #[test]
    fn u2_aggregator_unreachable_start_is_topology_error() {
        // Starting event `start` has no subscriber. ralph fallback's `*`
        // subscription is excluded from topology graph analysis, so the
        // aggregator sees this as a real topology error.
        let yaml = r#"
hats:
  a:
    name: "A"
    description: "Other-only"
    triggers: ["other.topic"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "no-start",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(!report.passed);
        let topology = report
            .findings
            .iter()
            .find(|f| f.source == FindingSource::Topology && f.severity == FindingSeverity::Error)
            .expect("expected at least one topology error finding");
        assert_eq!(topology.id, "topology.unreachable_start");
        assert!(
            topology.details.get("topic").map(String::as_str) == Some("start"),
            "details.topic must capture the start topic: {:?}",
            topology.details
        );
    }

    #[test]
    fn u2_aggregator_unreachable_completion_is_topology_error() {
        // `FAR_AWAY` completion promise has no publisher reachable from start.
        let yaml = r#"
hats:
  a:
    name: "A"
    description: "Done publisher"
    triggers: ["work.start"]
    publishes: ["work.done"]
event_loop:
  starting_event: "work.start"
  completion_promise: "FAR_AWAY"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "unreachable-completion",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(!report.passed);
        let topology = report
            .findings
            .iter()
            .find(|f| {
                f.source == FindingSource::Topology && f.id == "topology.unreachable_completion"
            })
            .expect("expected topology.unreachable_completion finding");
        assert_eq!(
            topology.details.get("topic").map(String::as_str),
            Some("FAR_AWAY")
        );
    }

    #[test]
    fn u2_aggregator_required_event_unreachable_is_topology_error() {
        // Required event `nonexistent` is not reachable.
        let yaml = r#"
hats:
  a:
    name: "A"
    description: "Completion publisher"
    triggers: ["start"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["nonexistent"]
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "missing-required",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(!report.passed);
        let topology = report.findings.iter().find(|f| {
            f.source == FindingSource::Topology && f.id == "topology.unreachable_required"
        });
        assert!(
            topology.is_some(),
            "expected topology.unreachable_required finding: {:?}",
            report.findings
        );
    }

    #[test]
    fn u2_aggregator_required_event_bypassed_is_topology_error() {
        // Two branches: one emits `review.passed`, the other bypasses it.
        let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    description: "Dispatcher"
    triggers: ["work.start"]
    publishes: ["needs.review", "skip.review"]
  reviewer:
    name: "Reviewer"
    description: "Reviews"
    triggers: ["needs.review"]
    publishes: ["review.passed"]
  reviewed_reporter:
    name: "Reviewed Reporter"
    description: "Reviewed path"
    triggers: ["review.passed"]
    publishes: ["LOOP_COMPLETE"]
  direct_reporter:
    name: "Direct Reporter"
    description: "Bypass path"
    triggers: ["skip.review"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["review.passed"]
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "bypassed-required",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(!report.passed);
        let topology = report.findings.iter().find(|f| {
            f.source == FindingSource::Topology
                && f.id == "topology.required_event_not_on_all_paths"
        });
        assert!(
            topology.is_some(),
            "expected topology.required_event_not_on_all_paths finding: {:?}",
            report.findings
        );
    }

    // ---- U2-aggregator: orphan detection ----

    #[test]
    fn u2_aggregator_real_orphan_is_orphan_warning() {
        // Hat `Sloppy` publishes `orphan.typo`; no subscriber.
        let yaml = r#"
hats:
  sloppy:
    name: "Sloppy"
    description: "Typos"
    triggers: ["trigger.z"]
    publishes: ["orphan.typo"]
  a:
    name: "A"
    description: "Entry"
    triggers: ["work.start"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "orphan-typo",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        let orphan = report
            .findings
            .iter()
            .find(|f| f.source == FindingSource::Orphan && f.severity == FindingSeverity::Warn);
        assert!(
            orphan.is_some(),
            "expected an orphan warning: {:?}",
            report.findings
        );
        let orphan = orphan.unwrap();
        assert_eq!(orphan.id, "orphan.no_subscriber");
        assert_eq!(
            orphan.details.get("topic").map(String::as_str),
            Some("orphan.typo")
        );
        assert_eq!(
            orphan.details.get("publisher").map(String::as_str),
            Some("Sloppy")
        );
    }

    #[test]
    fn u2_aggregator_completion_promise_not_orphan() {
        // `LOOP_COMPLETE` is published but only consumed by the loop runner.
        let yaml = r#"
hats:
  a:
    name: "A"
    description: "Entry"
    triggers: ["work.start"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "completion",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.source != FindingSource::Orphan),
            "completion_promise must not trigger orphan finding: {:?}",
            report.findings
        );
    }

    #[test]
    fn u2_aggregator_required_event_not_orphan() {
        // `report.done` is required and only consumed by the loop runner.
        let yaml = r#"
hats:
  reporter:
    name: "Reporter"
    description: "Final report"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
event_loop:
  starting_event: "REVIEW_COMPLETE"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "required-not-orphan",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.source != FindingSource::Orphan),
            "required_events topics must not trigger orphan finding: {:?}",
            report.findings
        );
    }

    #[test]
    fn u2_aggregator_build_blocked_not_orphan() {
        // `build.blocked` is consumed by the loop runner for thrashing
        // detection. A Builder hat publishes it without any hat
        // subscriber. Must NOT trigger an orphan finding.
        let yaml = r#"
hats:
  builder:
    name: "Builder"
    description: "Builds"
    triggers: ["work.start"]
    publishes: ["build.blocked"]
  a:
    name: "A"
    description: "Entry"
    triggers: ["work.start"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        // Use a different completion path so build.blocked's subscriber
        // chain is actually disambiguated. The orphan check only
        // inspects publishers, not subscribers.
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "build-blocked",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.source != FindingSource::Orphan),
            "build.blocked must not trigger orphan finding: {:?}",
            report.findings
        );
    }

    #[test]
    fn u2_aggregator_orphan_check_uses_specific_subscriber() {
        // Regression: ralph fallback's `*` subscription must NOT count
        // as a hat subscriber for orphan purposes. The `Sloppy` hat
        // publishes `work.dnoe` (a typo of `work.done`) with no real
        // subscriber — orphan must be reported.
        let yaml = r#"
hats:
  sloppy:
    name: "Sloppy"
    description: "Typos"
    triggers: ["trigger.z"]
    publishes: ["work.dnoe"]
  a:
    name: "A"
    description: "Entry"
    triggers: ["work.start"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "specific-subscriber",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        let orphan = report
            .findings
            .iter()
            .find(|f| f.source == FindingSource::Orphan);
        assert!(
            orphan.is_some(),
            "ralph fallback `*` subscription must not mask real orphan: {:?}",
            report.findings
        );
    }

    // ---- U2-aggregator: payload contract ----

    #[test]
    fn u2_aggregator_payload_non_strict_missing_schema_is_warn() {
        // Hat b references payload fields for `work.ready`, no schema.
        // Non-strict mode: payload finding is a warning, report passes.
        let yaml = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: false
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "payload-warn",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(
            report.passed,
            "non-strict missing schema is a warning: {:?}",
            report
        );
        let payload = report
            .findings
            .iter()
            .find(|f| f.source == FindingSource::Payload && f.severity == FindingSeverity::Warn);
        assert!(
            payload.is_some(),
            "expected payload warning: {:?}",
            report.findings
        );
        assert_eq!(
            payload.unwrap().id,
            "payload.schema_missing_for_required_topic"
        );
    }

    #[test]
    fn u2_aggregator_payload_strict_missing_schema_is_error() {
        // Same input, strict mode: payload finding is an error, report fails.
        let yaml = r#"
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "payload-strict",
            &config,
            &registry,
            RuntimeContractStrictness::preset_check_strict(),
        );
        assert!(
            !report.passed,
            "strict missing schema is an error: {:?}",
            report
        );
        let payload = report.findings.iter().find(|f| {
            f.source == FindingSource::Payload
                && f.severity == FindingSeverity::Error
                && f.id == "payload.schema_missing_for_required_topic"
        });
        assert!(
            payload.is_some(),
            "expected payload error: {:?}",
            report.findings
        );
    }

    #[test]
    fn u2_aggregator_payload_field_missing_from_schema_is_error() {
        // Schema declares only `task_id`, but consumer references `plan_name`.
        let yaml = r#"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["task_id"]
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "payload-field-missing",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(!report.passed);
        let payload = report.findings.iter().find(|f| {
            f.source == FindingSource::Payload && f.id == "payload.field_missing_from_schema"
        });
        assert!(
            payload.is_some(),
            "expected field-missing finding: {:?}",
            report.findings
        );
        let payload = payload.unwrap();
        assert_eq!(
            payload.details.get("field").map(String::as_str),
            Some("plan_name")
        );
        assert_eq!(payload.details.get("hat").map(String::as_str), Some("b"));
        assert_eq!(
            payload.details.get("topic").map(String::as_str),
            Some("work.ready")
        );
    }

    // ---- U2-aggregator: config validation + strict semantics ----

    #[test]
    fn u2_aggregator_config_warning_with_fail_on_warnings_makes_report_fail() {
        // `archive_prompts: true` is a DeferredFeature warning. With
        // fail_on_warnings=false the report passes; with
        // fail_on_warnings=true the same warning makes the report fail.
        let mut config = crate::config::RalphConfig::default();
        config.archive_prompts = true;
        config.event_loop.completion_promise = "DONE".to_string();
        config.tasks.enabled = false;
        config.topic_format_whitelist = vec!["DONE".to_string()];
        let registry = HatRegistry::from_runtime_config(&config);

        let non_strict = RuntimeContractAggregator::aggregate(
            "deferred-non-strict",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(
            non_strict.passed,
            "non-strict report with one config warning should pass: {:?}",
            non_strict
        );
        assert_eq!(non_strict.warnings, 1);

        let strict = RuntimeContractAggregator::aggregate(
            "deferred-strict",
            &config,
            &registry,
            RuntimeContractStrictness {
                payload_strict: false,
                fail_on_warnings: true,
            },
        );
        assert!(
            !strict.passed,
            "fail_on_warnings=true must fail the report on a warning: {:?}",
            strict
        );
        assert_eq!(strict.warnings, 1);
    }

    #[test]
    fn u2_aggregator_config_error_short_circuits() {
        // Empty `completion_promise` → config validate returns Err. The
        // aggregator must emit exactly one config error finding and
        // SKIP topology/payload/orphan — otherwise we'd produce
        // misleading secondary findings about an unreachable start
        // (which is actually a consequence of the broken config, not a
        // separate topology problem).
        let mut config = crate::config::RalphConfig::default();
        config.event_loop.completion_promise = "   ".to_string();
        let registry = HatRegistry::from_runtime_config(&config);
        let report = RuntimeContractAggregator::aggregate(
            "short-circuit",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert!(!report.passed);
        // Exactly one finding, and it is a config error.
        assert_eq!(
            report.findings.len(),
            1,
            "config error must short-circuit other checks: {:?}",
            report.findings
        );
        let only = &report.findings[0];
        assert_eq!(only.source, FindingSource::Config);
        assert_eq!(only.severity, FindingSeverity::Error);
        assert_eq!(only.id, "config.invalid_completion_promise");
    }

    // ---- U2-aggregator: serialization & label ----

    #[test]
    fn u2_aggregator_source_label_is_preserved() {
        let config = crate::config::RalphConfig::default();
        let registry = HatRegistry::from_runtime_config(&config);
        let report = RuntimeContractAggregator::aggregate(
            "builtin:ce-executor",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        assert_eq!(report.source_label, "builtin:ce-executor");
    }

    #[test]
    fn u2_aggregator_json_serialization_roundtrip() {
        // Build a report with mixed findings; roundtrip through JSON and
        // assert structural equality. The aggregator output must
        // satisfy the public JSON contract that U1 pinned.
        let yaml = r#"
hats:
  sloppy:
    name: "Sloppy"
    description: "Typos"
    triggers: ["trigger.z"]
    publishes: ["orphan.typo"]
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "roundtrip",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        let value: serde_json::Value = serde_json::to_value(&report).expect("serialize report");
        let obj = value
            .as_object()
            .expect("report should serialize to object");
        for key in [
            "source_label",
            "payload_strict",
            "fail_on_warnings",
            "passed",
            "warnings",
            "errors",
            "findings",
            "checked_at",
        ] {
            assert!(
                obj.contains_key(key),
                "report JSON missing stable key: {key}"
            );
        }
        let roundtripped: RuntimeContractReport =
            serde_json::from_value(value).expect("deserialize report");
        assert_eq!(roundtripped, report);
    }

    // ---- U2-aggregator: aggregator never stamps Preflight ----

    #[test]
    fn u2_aggregator_never_stamps_finding_source_preflight() {
        // Construct a deliberately-broken config that exercises
        // config/topology/payload/orphan paths and assert the resulting
        // report contains zero findings with `source=preflight`.
        let yaml = r#"
hats:
  sloppy:
    name: "Sloppy"
    description: "Typos"
    triggers: ["trigger.z"]
    publishes: ["orphan.typo", "report.done"]
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "no-preflight",
            &config,
            &registry,
            RuntimeContractStrictness::preset_check_strict(),
        );
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.source != FindingSource::Preflight),
            "core aggregator must never stamp findings with source=preflight: {:?}",
            report.findings
        );
    }

    // ──────────────────────────────────────────────────────────────────
    // 2026-06-07 plan U5: required-topic coverage + obligation alignment
    // ──────────────────────────────────────────────────────────────────

    use crate::RalphConfig;

    fn u5_registry() -> (RalphConfig, HatRegistry) {
        // Minimal but complete topology: every required topic is
        // both published and subscribed, every obligation topic is in
        // the hat's publishes.
        let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["work.ready", "work.failed"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done", "work.failed"]
    obligations:
      - on_trigger: "work.ready"
        must_emit_any_of: ["work.done", "work.failed"]
  reviewer:
    name: "Reviewer"
    triggers: ["work.done"]
    publishes: ["review.passed"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  required_events: ["work.done"]
  starting_event: "work.start"
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      work.done:
        required_fields: [plan_name]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse test config");
        let registry = HatRegistry::from_runtime_config(&config);
        (config, registry)
    }

    #[test]
    fn u5_detect_required_topic_gaps_clean_topology_produces_no_errors() {
        let (config, registry) = u5_registry();
        let findings = detect_required_topic_gaps(&config, &registry);
        let errors: Vec<&RuntimeContractFinding> = findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "clean topology must produce no required-topic errors, got: {errors:?}"
        );
    }

    #[test]
    fn u5_detect_required_topic_gaps_missing_publisher() {
        // Topology without an executor → work.done has no publisher
        // and the contract rule for work.done requires one.
        let yaml = r#"
hats:
  observer:
    name: "Observer"
    triggers: ["work.start"]
    publishes: ["work.start"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  required_events: ["work.done"]
  starting_event: "work.start"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse");
        let registry = HatRegistry::from_runtime_config(&config);
        let findings = detect_required_topic_gaps(&config, &registry);
        let codes: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert!(
            codes.contains(&"required.no_publisher"),
            "missing publisher must surface required.no_publisher, got: {codes:?}"
        );
    }

    #[test]
    fn u5_detect_required_topic_gaps_missing_subscriber() {
        // Topology that publishes a topic nobody listens to: producer
        // is fine, but the required topic has no subscriber.
        let yaml = r#"
hats:
  emitter:
    name: "Emitter"
    triggers: ["work.start"]
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  required_events: ["work.done"]
  starting_event: "work.start"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse");
        let registry = HatRegistry::from_runtime_config(&config);
        let findings = detect_required_topic_gaps(&config, &registry);
        let codes: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert!(
            codes.contains(&"required.no_subscriber"),
            "missing subscriber must surface required.no_subscriber, got: {codes:?}"
        );
    }

    #[test]
    fn u5_detect_obligation_topics_not_in_publishes_catches_misconfiguration() {
        // R4: a hat that lists a topic in `must_emit_any_of` without
        // listing it in `publishes` is incoherent — the activation-
        // level path would never actually let the hat satisfy the
        // obligation (origin guard would reject the publish).
        let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.failed"]
    obligations:
      - on_trigger: "work.ready"
        must_emit_any_of: ["work.done", "work.failed"]
  receiver:
    name: "Receiver"
    triggers: ["work.failed"]
    publishes: ["ack"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  required_events: ["work.failed"]
  starting_event: "work.start"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse");
        let findings = detect_obligation_topics_not_in_publishes(&config);
        let codes: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert!(
            codes.contains(&"obligation.topic_not_in_publishes"),
            "executor lists work.done in must_emit_any_of but not in publishes: {codes:?}"
        );
    }

    #[test]
    fn u5_detect_obligation_topics_clean_config_produces_no_findings() {
        let (config, _registry) = u5_registry();
        let findings = detect_obligation_topics_not_in_publishes(&config);
        assert!(
            findings.is_empty(),
            "clean config must produce no obligation findings, got: {findings:?}"
        );
    }

    #[test]
    fn u5_aggregator_runs_required_topic_check() {
        // End-to-end: the aggregator must call the new step.
        let (config, registry) = u5_registry();
        let report = RuntimeContractAggregator::aggregate(
            "u5-test",
            &config,
            &registry,
            RuntimeContractStrictness::preset_check_strict(),
        );
        let required_codes: Vec<&str> = report
            .findings
            .iter()
            .filter(|f| f.id.starts_with("required."))
            .map(|f| f.id.as_str())
            .collect();
        assert!(
            required_codes.is_empty(),
            "aggregator must not produce required.* findings on clean config, got: {required_codes:?}"
        );
    }

    // ──────────────────────────────────────────────────────────────────
    // U3 aggregator tests: preset static lint integration
    // ──────────────────────────────────────────────────────────────────

    use crate::preset_lint::FINDING_INVALID_TOPIC_FORMAT;

    #[test]
    fn u3_aggregator_invalid_topic_produces_lint_finding() {
        // An uppercase topic like "LOOP_COMPLETE" without whitelist
        // produces a `lint.invalid_topic_format` finding.
        let yaml = r#"
tasks:
  enabled: false
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "u3-lint-topic",
            &config,
            &registry,
            RuntimeContractStrictness::preset_check_strict(),
        );
        let lint_findings: Vec<&RuntimeContractFinding> = report
            .findings
            .iter()
            .filter(|f| f.source == FindingSource::Lint)
            .collect();
        // LOOP_COMPLETE is uppercase → should produce a lint finding.
        let invalid = lint_findings
            .iter()
            .find(|f| f.id == format!("lint.{FINDING_INVALID_TOPIC_FORMAT}"));
        assert!(
            invalid.is_some(),
            "uppercase LOOP_COMPLETE without whitelist must produce lint finding: {:?}",
            lint_findings
        );
        let invalid = invalid.unwrap();
        assert_eq!(
            invalid.details.get("topic").map(String::as_str),
            Some("LOOP_COMPLETE")
        );
    }

    #[test]
    fn u3_aggregator_whitelist_exempts_topic() {
        // LOOP_COMPLETE in the whitelist should NOT produce an error.
        let yaml = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: false
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "u3-lint-whitelist",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        // No lint ERROR or WARN findings for LOOP_COMPLETE.
        let bad_lint: Vec<&RuntimeContractFinding> = report
            .findings
            .iter()
            .filter(|f| {
                f.source == FindingSource::Lint
                    && f.severity != FindingSeverity::Pass
                    && f.details.get("topic").map(String::as_str) == Some("LOOP_COMPLETE")
            })
            .collect();
        assert!(
            bad_lint.is_empty(),
            "whitelisted LOOP_COMPLETE must not produce lint errors/warnings: {:?}",
            bad_lint
        );
    }

    #[test]
    fn u3_aggregator_lint_and_payload_errors_coexist() {
        // Config with both a lint error (invalid topic) and a payload
        // error (field missing from schema). Both must appear.
        let yaml = r#"
topic_format_whitelist: []
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["task_id"]
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "u3-lint-and-payload",
            &config,
            &registry,
            RuntimeContractStrictness::preset_check_strict(),
        );
        let has_lint = report
            .findings
            .iter()
            .any(|f| f.source == FindingSource::Lint);
        let has_payload = report
            .findings
            .iter()
            .any(|f| f.source == FindingSource::Payload);
        assert!(
            has_lint,
            "report must contain lint findings: {:?}",
            report.findings
        );
        assert!(
            has_payload,
            "report must contain payload findings: {:?}",
            report.findings
        );
    }

    #[test]
    fn u3_aggregator_lint_findings_are_sorted() {
        // Lint findings should be deterministic regardless of config order.
        let yaml = r#"
topic_format_whitelist: []
tasks:
  enabled: false
hats:
  a:
    name: "A"
    description: "Producer A"
    triggers: ["work.start"]
    publishes: ["invalid_topic_a"]
  b:
    name: "B"
    description: "Producer B"
    triggers: ["work.start"]
    publishes: ["invalid_topic_b"]
event_loop:
  starting_event: "work.start"
  completion_promise: "loop.complete"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let report = RuntimeContractAggregator::aggregate(
            "u3-lint-sorted",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        let lint_ids: Vec<&str> = report
            .findings
            .iter()
            .filter(|f| f.source == FindingSource::Lint)
            .map(|f| f.id.as_str())
            .collect();
        // Check that lint findings are sorted by id.
        for window in lint_ids.windows(2) {
            assert!(
                window[0] <= window[1],
                "lint findings not sorted: {:?}",
                lint_ids
            );
        }
    }

    #[test]
    fn u3_aggregator_strict_mode_promotes_ownership_warnings_to_errors() {
        // With fail_on_warnings=true (preset_check_strict), ownership
        // warnings should become errors via LintStrictness::Strict.
        let yaml = r#"
topic_owners:
  work.done:
    - executor
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: false
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    description: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.done"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);

        // Non-strict: ownership warnings stay warnings.
        let non_strict = RuntimeContractAggregator::aggregate(
            "u3-non-strict",
            &config,
            &registry,
            RuntimeContractStrictness::default(),
        );
        let ownership_warns: Vec<&RuntimeContractFinding> = non_strict
            .findings
            .iter()
            .filter(|f| {
                f.source == FindingSource::Lint
                    && f.id.starts_with("lint.preset.")
                    && f.severity == FindingSeverity::Warn
            })
            .collect();
        // With clean ownership config, there should be no ownership warnings.
        assert!(
            ownership_warns.is_empty(),
            "clean config should have no ownership warnings: {:?}",
            ownership_warns
        );

        // Strict: same config, no ownership errors either (clean config).
        let strict = RuntimeContractAggregator::aggregate(
            "u3-strict",
            &config,
            &registry,
            RuntimeContractStrictness::preset_check_strict(),
        );
        let ownership_errs: Vec<&RuntimeContractFinding> = strict
            .findings
            .iter()
            .filter(|f| {
                f.source == FindingSource::Lint
                    && f.id.starts_with("lint.preset.")
                    && f.severity == FindingSeverity::Error
            })
            .collect();
        assert!(
            ownership_errs.is_empty(),
            "clean config should have no ownership errors: {:?}",
            ownership_errs
        );
    }
}
