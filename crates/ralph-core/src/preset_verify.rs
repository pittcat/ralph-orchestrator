//! Public verifier contract for `ralph preset verify` (Units 1 & 2).
//!
//! This module owns the **public** scenario schema (`version: 1`), the typed
//! model that downstream Units build on, the failure-category taxonomy, and
//! the deterministic report shape. Unit 2 adds the real `EventLoop` driver
//! into this module as well — see [`run_scenario`].
//!
//! The module does **not** know about CLI, skills, remote fetching or any
//! backend — those concerns live in the `ralph-cli` and skill layers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Top-level parsed scenario file (a version 1 contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioFile {
    pub version: u32,
    pub scenarios: Vec<Scenario>,
}

/// One scenario entry inside [`ScenarioFile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    pub name: String,
    pub responses: Vec<Response>,
    pub expect: ExpectBlock,
    pub limits: Limits,
}

/// A single scripted hat output consumed by the driver in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Response {
    pub hat: Option<String>,
    pub output: String,
    pub success: bool,
}

/// Step + no-progress budgets for a scenario. Both must be positive integers,
/// and `no_progress_steps <= max_steps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_steps: u32,
    pub no_progress_steps: u32,
}

/// The contract the driver must satisfy for a scenario to pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectBlock {
    pub start_event: String,
    pub accepted_events: Vec<String>,
    pub forbidden_events: Vec<String>,
    pub terminal: TerminalKind,
    pub terminal_topic: Option<String>,
    pub payload_fields: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
}

/// Terminal type the scenario expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    Success,
    Failure,
    Blocked,
    None,
}

/// Public, structured failure categories the verifier reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum FailureKind {
    InputError(String),
    StaticContractFailure(String),
    ScenarioFailure(String),
    RuntimeException(String),
    Timeout(String),
    NoProgress(String),
    UnclosedTerminal(String),
}

impl FailureKind {
    /// Short tag suitable for JSON / report fields (`failure_kind`).
    pub fn tag(&self) -> &'static str {
        match self {
            FailureKind::InputError(_) => "input_error",
            FailureKind::StaticContractFailure(_) => "static_contract_failure",
            FailureKind::ScenarioFailure(_) => "scenario_failure",
            FailureKind::RuntimeException(_) => "runtime_exception",
            FailureKind::Timeout(_) => "timeout",
            FailureKind::NoProgress(_) => "no_progress",
            FailureKind::UnclosedTerminal(_) => "unclosed_terminal",
        }
    }
}

/// Why scenario parsing / validation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputError {
    /// YAML or root-level shape error.
    Parse(String),
    /// `version` missing, not `1`, or otherwise unrecognised.
    SchemaVersion(String),
    /// Empty scenario list, duplicate name, missing field, etc.
    InvalidScenario(String),
    /// `expect.start_event` ≠ the config's resolved starting event.
    StartEventMismatch { expected: String, actual: String },
    /// Limits missing, non-positive, or `no_progress_steps > max_steps`.
    InvalidLimit(String),
}

/// Source provenance for the preset/hats under verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Builtin,
    External,
}

impl SourceKind {
    /// Derive source kind from a `HatsSource` label (E19).
    pub fn from_hats_source(label: &str) -> Self {
        if label.starts_with("builtin:") {
            SourceKind::Builtin
        } else {
            SourceKind::External
        }
    }

    /// Whether this label looks like a remote URL (must be rejected before
    /// runtime). The CLI layer converts remote labels into InputError before
    /// reaching the driver; this helper lets the CLI detect them through the
    /// same code path.
    pub fn is_remote(label: &str) -> bool {
        label.starts_with("http://") || label.starts_with("https://")
    }
}

/// Static-layer summary surfaced in the final report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticLayer {
    pub passed: bool,
    pub warnings: usize,
    pub errors: usize,
    pub findings: Vec<String>,
}

/// Per-scenario summary surfaced in [`PresetVerifyReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyReportScenario {
    pub name: String,
    pub passed: bool,
    pub steps: usize,
    pub accepted_events: Vec<String>,
    pub rejected_events: Vec<String>,
    pub terminal_topic: Option<String>,
    pub termination: Option<String>,
    pub failure_kind: Option<String>,
    pub last_observable_state: LastObservableState,
    pub trace_digest: String,
}

/// Last observable state for a scenario — used by reviewer to diagnose stalls
/// without scraping internal logs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastObservableState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_hat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_accepted_topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_runtime_termination: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_index: Option<usize>,
}

/// Full report emitted by `ralph preset verify`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetVerifyReport {
    pub passed: bool,
    pub source_kind: SourceKind,
    /// Static contract layer summary. Serialised as `"static"` to match the
    /// public report contract in §3.3 of the plan.
    #[serde(rename = "static")]
    pub static_layer: StaticLayer,
    pub scenarios: Vec<VerifyReportScenario>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    #[serde(default)]
    pub last_observable_state: LastObservableState,
    pub trace_digest: String,
}

impl Default for PresetVerifyReport {
    /// Zero-state shim for early-exit report construction. CLI command
    /// surfaces use `PresetVerifyReport::default().with_failure_kind(...)`.
    fn default() -> Self {
        Self {
            passed: false,
            source_kind: SourceKind::External,
            static_layer: StaticLayer {
                passed: true,
                warnings: 0,
                errors: 0,
                findings: Vec::new(),
            },
            scenarios: Vec::new(),
            failure_kind: None,
            last_observable_state: LastObservableState::default(),
            trace_digest: String::new(),
        }
    }
}

impl PresetVerifyReport {
    /// Set `failure_kind` for early-exit error reports. Takes `&mut self` so
    /// callers can compose additional field overrides after the initial
    /// `PresetVerifyReport::default()` shim (e.g. attach the resolved
    /// `static_layer` after a contract failure).
    pub fn with_failure_kind(&mut self, kind: impl Into<String>) -> &mut Self {
        self.failure_kind = Some(kind.into());
        self
    }
}

impl Limits {
    pub fn new(max_steps: u32, no_progress_steps: u32) -> Result<Self, InputError> {
        if max_steps == 0 {
            return Err(InputError::InvalidLimit("max_steps must be positive".into()));
        }
        if no_progress_steps == 0 {
            return Err(InputError::InvalidLimit(
                "no_progress_steps must be positive".into(),
            ));
        }
        if no_progress_steps > max_steps {
            return Err(InputError::InvalidLimit(
                "no_progress_steps must be <= max_steps".into(),
            ));
        }
        Ok(Self {
            max_steps,
            no_progress_steps,
        })
    }
}

impl ExpectBlock {
    pub fn validate(&self) -> Result<(), InputError> {
        if !matches!(self.terminal, TerminalKind::None) && self.terminal_topic.is_none() {
            return Err(InputError::InvalidScenario(format!(
                "terminal={:?} requires terminal_topic",
                self.terminal
            )));
        }
        Ok(())
    }
}

impl Response {
    /// Default `success = true` when the YAML omits the field.
    fn from_yaml(raw: RawResponse) -> Result<Self, InputError> {
        let output = raw
            .output
            .ok_or_else(|| InputError::InvalidScenario("response.output is required".into()))?;
        Ok(Self {
            hat: raw.hat,
            output,
            success: raw.success.unwrap_or(true),
        })
    }
}

impl ScenarioFile {
    /// Parse a scenario YAML blob against the resolved config's starting event.
    pub fn from_yaml(yaml: &str, config_starting_event: &str) -> Result<Self, InputError> {
        let raw: RawScenarioFile = serde_yaml::from_str(yaml)
            .map_err(|e| InputError::Parse(format!("yaml parse failed: {e}")))?;

        let version = raw.version.ok_or_else(|| {
            InputError::SchemaVersion("missing `version` field at top level".into())
        })?;
        if version != 1 {
            return Err(InputError::SchemaVersion(format!(
                "unsupported version: {version}"
            )));
        }

        let raw_scenarios = raw.scenarios.ok_or_else(|| {
            InputError::InvalidScenario("scenarios list is required".into())
        })?;
        if raw_scenarios.is_empty() {
            return Err(InputError::InvalidScenario(
                "scenarios list must not be empty".into(),
            ));
        }

        let mut names = std::collections::HashSet::new();
        let mut scenarios = Vec::with_capacity(raw_scenarios.len());
        for raw_s in raw_scenarios {
            let name = raw_s.name.clone().ok_or_else(|| {
                InputError::InvalidScenario("scenario.name is required".into())
            })?;
            if !names.insert(name.clone()) {
                return Err(InputError::InvalidScenario(format!(
                    "duplicate scenario name: {name}"
                )));
            }
            scenarios.push(Scenario::from_raw(raw_s, config_starting_event)?);
        }

        Ok(Self {
            version: 1,
            scenarios,
        })
    }
}

impl Scenario {
    fn from_raw(raw: RawScenario, config_starting_event: &str) -> Result<Self, InputError> {
        let name = raw
            .name
            .clone()
            .ok_or_else(|| InputError::InvalidScenario("scenario.name is required".into()))?;
        let raw_responses = raw.responses.unwrap_or_default();
        let mut responses = Vec::with_capacity(raw_responses.len());
        for r in raw_responses {
            responses.push(Response::from_yaml(r)?);
        }

        let expect_raw = raw.expect.ok_or_else(|| {
            InputError::InvalidScenario(format!("scenario {name}: expect block required"))
        })?;
        let expect = ExpectBlock {
            start_event: expect_raw.start_event.ok_or_else(|| {
                InputError::InvalidScenario(format!(
                    "scenario {name}: expect.start_event required"
                ))
            })?,
            accepted_events: expect_raw.accepted_events.unwrap_or_default(),
            forbidden_events: expect_raw.forbidden_events.unwrap_or_default(),
            terminal: parse_terminal(expect_raw.terminal.as_deref())?,
            terminal_topic: expect_raw.terminal_topic,
            payload_fields: expect_raw
                .payload_fields
                .map(parse_payload_fields)
                .transpose()?
                .unwrap_or_default(),
        };
        if expect.start_event != config_starting_event {
            return Err(InputError::StartEventMismatch {
                expected: expect.start_event,
                actual: config_starting_event.to_string(),
            });
        }
        expect.validate()?;

        let limits_raw = raw.limits.ok_or_else(|| {
            InputError::InvalidScenario(format!("scenario {name}: limits required"))
        })?;
        let limits = Limits::new(
            limits_raw
                .max_steps
                .ok_or_else(|| InputError::InvalidLimit("max_steps required".into()))?,
            limits_raw.no_progress_steps.ok_or_else(|| {
                InputError::InvalidLimit("no_progress_steps required".into())
            })?,
        )?;

        Ok(Self {
            name,
            responses,
            expect,
            limits,
        })
    }
}

fn parse_terminal(value: Option<&str>) -> Result<TerminalKind, InputError> {
    Ok(match value {
        None | Some("none") => TerminalKind::None,
        Some("success") => TerminalKind::Success,
        Some("failure") => TerminalKind::Failure,
        Some("blocked") => TerminalKind::Blocked,
        Some(other) => {
            return Err(InputError::InvalidScenario(format!(
                "unknown expect.terminal: {other}"
            )))
        }
    })
}

fn parse_payload_fields(
    raw: BTreeMap<String, serde_yaml::Value>,
) -> Result<BTreeMap<String, BTreeMap<String, serde_json::Value>>, InputError> {
    let mut out = BTreeMap::new();
    for (topic, value) in raw {
        let inner = match value {
            serde_yaml::Value::Mapping(m) => m,
            _ => {
                return Err(InputError::InvalidScenario(format!(
                    "payload_fields[{topic}] must be an object"
                )))
            }
        };
        let mut topic_fields = BTreeMap::new();
        for (k, v) in inner {
            let key = match &k {
                serde_yaml::Value::String(s) => s.clone(),
                serde_yaml::Value::Number(n) => n.to_string(),
                serde_yaml::Value::Bool(b) => b.to_string(),
                serde_yaml::Value::Null => "null".to_string(),
                other => {
                    return Err(InputError::InvalidScenario(format!(
                        "payload_fields[{topic}] key must be string/number/bool, got: {other:?}"
                    )))
                }
            };
            let json_v: serde_json::Value = serde_json::to_value(v).map_err(|e| {
                InputError::InvalidScenario(format!(
                    "payload_fields[{topic}].{key} not JSON-serialisable: {e}"
                ))
            })?;
            topic_fields.insert(key, json_v);
        }
        out.insert(topic, topic_fields);
    }
    Ok(out)
}

/// Compute a deterministic SHA-256 digest of (scenario contract, input blob,
/// accepted event sequence). Must be reproducible across runs and exclude
/// timestamps / absolute paths.
pub fn compute_trace_digest(scenario: &Scenario, input_blob: &str, accepted: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"name=");
    hasher.update(scenario.name.as_bytes());
    hasher.update(b"\nlimits.max_steps=");
    hasher.update(scenario.limits.max_steps.to_le_bytes());
    hasher.update(b"\nlimits.no_progress_steps=");
    hasher.update(scenario.limits.no_progress_steps.to_le_bytes());
    hasher.update(b"\nterminal=");
    hasher.update(format!("{:?}", scenario.expect.terminal).as_bytes());
    hasher.update(b"\nterminal_topic=");
    hasher.update(
        scenario
            .expect
            .terminal_topic
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
    );
    hasher.update(b"\nstart_event=");
    hasher.update(scenario.expect.start_event.as_bytes());

    let mut expected: Vec<&str> = scenario
        .expect
        .accepted_events
        .iter()
        .map(String::as_str)
        .collect();
    expected.sort();
    for e in expected {
        hasher.update(b"\nexpected=");
        hasher.update(e.as_bytes());
    }

    for r in &scenario.responses {
        hasher.update(b"\nresp.hat=");
        if let Some(h) = &r.hat {
            hasher.update(h.as_bytes());
        }
        hasher.update(b"\nresp.output=");
        hasher.update(r.output.as_bytes());
        hasher.update(b"\nresp.success=");
        hasher.update(if r.success { b"1" } else { b"0" });
    }

    hasher.update(b"\ninput=");
    hasher.update(input_blob.as_bytes());

    for a in accepted {
        hasher.update(b"\naccepted=");
        hasher.update(a.as_bytes());
    }

    let out = hasher.finalize();
    hex_encode(&out)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

// ---------------- raw (YAML-facing) types ---------------- //
//
// These are private because they only model the on-disk shape and are not
// part of the public contract. Callers go through ScenarioFile::from_yaml.

// ---------------- raw (YAML-facing) types ---------------- //
//
// These are private because they only model the on-disk shape and are not
// part of the public contract. Callers go through ScenarioFile::from_yaml.

#[derive(Debug, Deserialize)]
struct RawScenarioFile {
    version: Option<u32>,
    scenarios: Option<Vec<RawScenario>>,
}

#[derive(Debug, Deserialize)]
struct RawScenario {
    name: Option<String>,
    responses: Option<Vec<RawResponse>>,
    expect: Option<RawExpect>,
    limits: Option<RawLimits>,
}

#[derive(Debug, Deserialize)]
struct RawResponse {
    hat: Option<String>,
    output: Option<String>,
    success: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RawExpect {
    start_event: Option<String>,
    accepted_events: Option<Vec<String>>,
    forbidden_events: Option<Vec<String>>,
    terminal: Option<String>,
    terminal_topic: Option<String>,
    payload_fields: Option<BTreeMap<String, serde_yaml::Value>>,
}

#[derive(Debug, Deserialize)]
struct RawLimits {
    max_steps: Option<u32>,
    no_progress_steps: Option<u32>,
}

// ---------------- driver + trace (Unit 2) ---------------- //
//
// The driver drives a real `EventLoop` over the scripted responses and
// returns an ordered `ScenarioTrace` consumed by the verdict evaluator
// (Unit 3). It is intentionally decoupled from CLI/skill concerns.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::RalphConfig;
use crate::event_loop::{
    EventLoop, ProcessedEvents, TerminationReason,
};
use crate::execution_contract;
use crate::loop_context::LoopContext;
use ralph_proto::Event;

/// One step recorded by the driver. `accepted` lists topics from the
/// `ProcessedEvents.accepted_events` for this iteration in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    pub step: usize,
    pub response_index: usize,
    pub hat: Option<String>,
    pub output: String,
    pub success: bool,
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
    pub termination: Option<String>,
}

/// Per-scenario trace produced by [`run_scenario`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioTrace {
    pub scenario: Scenario,
    pub steps: Vec<StepRecord>,
    pub accepted_events: Vec<String>,
    pub rejected_events: Vec<String>,
    pub last_hat: Option<String>,
    pub last_accepted_topic: Option<String>,
    pub last_runtime_termination: Option<String>,
    pub terminal_topic: Option<String>,
    pub trace_digest: String,
}

/// Outcome of running one scenario. The verdict classifier in Unit 3 turns
/// this into a structured `VerifyReportScenario` + `failure_kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioOutcome {
    pub trace: ScenarioTrace,
    /// One of: passed, no_progress, timeout, unclosed_terminal,
    /// scenario_failure, runtime_exception, static_contract_failure,
    /// input_error.
    pub failure_kind: Option<FailureKind>,
    pub passed: bool,
}

/// Handle for the temp workspace the driver creates. Holds the tempdir until
/// dropped; the driver itself does not return absolute paths to callers.
pub struct DriverWorkspace {
    pub temp_dir: tempfile::TempDir,
}

/// Accumulates step-by-step driver state and produces [`ScenarioTrace`] /
/// [`ScenarioOutcome`] results. Replaces the 9-argument `build_trace` fan-out
/// in [`run_scenario`] with a single mutable accumulator; the 9-arg helper is
/// kept as a private wrapper that calls `ScenarioTracer::snapshot`.
struct ScenarioTracer<'a> {
    scenario: &'a Scenario,
    steps: Vec<StepRecord>,
    accepted: Vec<String>,
    rejected: Vec<String>,
    last_hat: Option<String>,
    last_accepted_topic: Option<String>,
    last_runtime_termination: Option<String>,
    terminal_topic: Option<String>,
    no_progress_window: usize,
    step_count: usize,
}

impl<'a> ScenarioTracer<'a> {
    fn new(scenario: &'a Scenario) -> Self {
        Self {
            scenario,
            steps: Vec::with_capacity(scenario.responses.len()),
            accepted: Vec::new(),
            rejected: Vec::new(),
            last_hat: None,
            last_accepted_topic: None,
            last_runtime_termination: None,
            terminal_topic: None,
            no_progress_window: 0,
            step_count: 0,
        }
    }

    /// Record one response iteration, updating all accumulated state.
    fn record_step(&mut self, step: StepRecord, processed: &ProcessedEvents) {
        self.step_count = self.step_count.saturating_add(1);
        self.steps.push(step);

        for event in &processed.accepted_events {
            let topic = event.topic.to_string();
            self.accepted.push(topic.clone());
            self.last_accepted_topic = Some(topic.clone());
            if let Some(expected_topic) = &self.scenario.expect.terminal_topic
                && &topic == expected_topic
            {
                self.terminal_topic = Some(topic);
            }
        }

        if processed.had_rejected_events {
            for finding in &processed.contract_rejections {
                let label = format!(
                    "contract:topic={} kind={:?}",
                    finding.topic, finding.kind
                );
                self.rejected.push(label);
            }
        }

        if processed.accepted_events.is_empty() {
            self.no_progress_window = self.no_progress_window.saturating_add(1);
        } else {
            self.no_progress_window = 0;
        }
    }

    /// Take a snapshot of the current accumulated state and return a
    /// [`ScenarioTrace`]. The tracer is not consumed.
    fn snapshot(&self, input_blob: &str) -> ScenarioTrace {
        let accepted_refs: Vec<&str> = self.accepted.iter().map(String::as_str).collect();
        let trace_digest = compute_trace_digest(self.scenario, input_blob, &accepted_refs);
        ScenarioTrace {
            scenario: self.scenario.clone(),
            steps: self.steps.clone(),
            accepted_events: self.accepted.clone(),
            rejected_events: self.rejected.clone(),
            last_hat: self.last_hat.clone(),
            last_accepted_topic: self.last_accepted_topic.clone(),
            last_runtime_termination: self.last_runtime_termination.clone(),
            terminal_topic: self.terminal_topic.clone(),
            trace_digest,
        }
    }

    /// Consume the tracer and wrap its accumulated state in a
    /// [`ScenarioOutcome`].
    fn finalize_with(
        self,
        input_blob: &str,
        failure_kind: FailureKind,
        passed: bool,
    ) -> ScenarioOutcome {
        ScenarioOutcome {
            trace: self.snapshot(input_blob),
            failure_kind: Some(failure_kind),
            passed,
        }
    }
}

impl DriverWorkspace {
    pub fn new() -> std::io::Result<Self> {
        let temp_dir = tempfile::Builder::new()
            .prefix("ralph-preset-verify-")
            .tempdir()?;
        Ok(Self { temp_dir })
    }

    pub fn ralph_dir(&self) -> PathBuf {
        self.temp_dir.path().join(".ralph")
    }

    pub fn events_path(&self) -> PathBuf {
        self.ralph_dir().join("events.jsonl")
    }
}

/// Run one scenario through a real `EventLoop` and return the ordered trace.
///
/// `mutate_config` lets the caller (CLI) shape the loaded `RalphConfig`
/// (workspace_root, hat setup, event_policy, etc.) before compile. The driver
/// then pins the workspace to the tempdir and pins
/// `event_loop.task_resume_ttl_seconds = Some(0)` so the deterministic
/// fixture inputs aren't classified as stale by the freshness filter.
pub fn run_scenario(
    scenario: &Scenario,
    config: &RalphConfig,
    workspace: &DriverWorkspace,
    input_blob: &str,
) -> Result<ScenarioOutcome, FailureKind> {
    // Prepare the .ralph directory and a writable events file in the temp
    // workspace (real `process_events_from_jsonl` reads from this file).
    std::fs::create_dir_all(workspace.ralph_dir()).map_err(|e| {
        FailureKind::RuntimeException(format!("create .ralph dir failed: {e}"))
    })?;

    let mut config = config.clone();
    config.core.workspace_root = workspace.temp_dir.path().to_path_buf();
    // Disable freshness TTL so the fixture inputs aren't classified as stale.
    config.event_loop.task_resume_ttl_seconds = Some(0);
    // Re-pin workspace_root after the clone in case the caller overwrote core.
    config.core.workspace_root = workspace.temp_dir.path().to_path_buf();

    let context = LoopContext::primary(workspace.temp_dir.path().to_path_buf());

    let resolved = execution_contract::compile(config).map_err(|e| {
        FailureKind::StaticContractFailure(format!("contract compile failed: {e:?}"))
    })?;

    let mut event_loop = EventLoop::from_resolved(resolved, context);
    event_loop.initialize("Verify");

    let parser = crate::event_parser::EventParser::new();
    let mut tracer = ScenarioTracer::new(scenario);

    // P0 adversarial A1: an empty response sequence with terminal: none is not
    // a valid scenario — it represents a degenerate input that must be rejected
    // by the driver before the loop iterates zero times. Verifies with
    // `FailureKind::NoProgress` and `passed=false`.
    if scenario.responses.is_empty() && matches!(scenario.expect.terminal, TerminalKind::None) {
        return Ok(tracer.finalize_with(
            input_blob,
            FailureKind::NoProgress("empty response sequence is not a valid scenario".into()),
            false,
        ));
    }

    for (idx, response) in scenario.responses.iter().enumerate() {
        tracer.step_count = tracer.step_count.saturating_add(1);
        if tracer.step_count > scenario.limits.max_steps as usize {
            return Ok(ScenarioOutcome {
                trace: tracer.snapshot(input_blob),
                failure_kind: Some(FailureKind::Timeout(format!(
                    "max_steps={} exceeded",
                    scenario.limits.max_steps
                ))),
                passed: false,
            });
        }

        // `next_hat()` is the only authority on the current hat. If the
        // scenario pinned a hat and it doesn't match the runtime selection,
        // we record a scenario_failure (the driver never silently routes the
        // response to a different hat — D6 deterministic contract).
        let next_hat = event_loop.next_hat().cloned();
        if let Some(pinned) = &response.hat {
            match &next_hat {
                Some(actual) if actual.as_str() == pinned.as_str() => {}
                _ => {
                    let detail = format!(
                        "scenario pinned hat={} but next_hat()={:?} at response_index={}",
                        pinned,
                        next_hat.as_ref().map(|h| h.to_string()),
                        idx
                    );
                    return Ok(ScenarioOutcome {
                        trace: tracer.snapshot(input_blob),
                        failure_kind: Some(FailureKind::ScenarioFailure(detail)),
                        passed: false,
                    });
                }
            }
        }

        let hat_id = match next_hat {
            Some(h) => h,
            None => {
                // No more hats to schedule. Treat as a bounded timeout
                // (we've run out of routing options).
                return Ok(ScenarioOutcome {
                    trace: tracer.snapshot(input_blob),
                    failure_kind: Some(FailureKind::UnclosedTerminal(
                        "next_hat() returned None before consuming all responses".into(),
                    )),
                    passed: false,
                });
            }
        };

        // build_prompt is consumed but the prompt itself is not part of the
        // trace (the verifier is about workflow, not about prompt text).
        let _ = event_loop.build_prompt(&hat_id);

        let termination: Option<TerminationReason> =
            event_loop.process_output(&hat_id, &response.output, response.success);
        if let Some(reason) = &termination {
            tracer.last_runtime_termination = Some(format!("{reason:?}"));
        }
        tracer.last_hat = Some(hat_id.to_string());

        // Parse the scripted output and append events to JSONL so
        // process_events_from_jsonl can route them through the real runtime.
        let parsed = parser.parse(&response.output);
        let events_path = workspace.events_path();
        write_events_to_jsonl(&events_path, &parsed, idx).map_err(|e| {
            FailureKind::RuntimeException(format!("write events.jsonl failed: {e}"))
        })?;

        let processed: ProcessedEvents =
            event_loop.process_events_from_jsonl().map_err(|e| {
                FailureKind::RuntimeException(format!("process_events_from_jsonl failed: {e:?}"))
            })?;

        // StepRecord accepts/rejected fields are kept for backward compatibility
        // with consumers that read ScenarioTrace; tracer.record_step builds the
        // same shape from `processed`.
        let step = StepRecord {
            step: tracer.step_count,
            response_index: idx,
            hat: Some(hat_id.to_string()),
            output: response.output.clone(),
            success: response.success,
            accepted: Vec::new(),
            rejected: Vec::new(),
            termination: tracer.last_runtime_termination.clone(),
        };
        tracer.record_step(step, &processed);

        if tracer.no_progress_window >= scenario.limits.no_progress_steps as usize {
            return Ok(ScenarioOutcome {
                trace: tracer.snapshot(input_blob),
                failure_kind: Some(FailureKind::NoProgress(format!(
                    "no_progress_steps={} consecutive no-accepted events",
                    scenario.limits.no_progress_steps
                ))),
                passed: false,
            });
        }
    }

    // If we got here, classify based on terminal expectation vs reality.
    let passed = match scenario.expect.terminal {
        TerminalKind::None => true,
        _ => tracer.terminal_topic.is_some()
            && scenario
                .expect
                .terminal_topic
                .as_ref()
                .map(|t| tracer.terminal_topic.as_deref() == Some(t.as_str()))
                .unwrap_or(false),
    };

    let failure_kind = if passed {
        None
    } else {
        Some(FailureKind::UnclosedTerminal(format!(
            "expected terminal={:?} topic={:?} but trace ended with accepted={:?}",
            scenario.expect.terminal,
            scenario.expect.terminal_topic,
            tracer.accepted
        )))
    };

    Ok(ScenarioOutcome {
        trace: tracer.snapshot(input_blob),
        failure_kind,
        passed,
    })
}

fn write_events_to_jsonl(
    path: &Path,
    events: &[Event],
    response_index: usize,
) -> std::io::Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for event in events {
        let entry = serde_json::json!({
            "topic": event.topic,
            "payload": event.payload,
            "response_index": response_index,
        });
        writeln!(file, "{entry}")?;
    }
    Ok(())
}

// ---------------- Unit 3 verdict evaluator ---------------- //
//
// The verdict evaluator turns a `ScenarioOutcome` into a `VerifyReportScenario`
// and decides the failure_kind for the run. It does NOT consume the
// static-layer report (that's the CLI's job).

/// Per-scenario verdict evaluation. Maps `ScenarioOutcome` into the final
/// `VerifyReportScenario` shape required by the public report contract.
///
/// Compares the recorded trace against `scenario.expect`:
/// - terminal kind exact match (success / failure / blocked / none)
/// - terminal topic exact match when terminal is not `none`
/// - forbidden topics must NOT appear in `accepted_events`
/// - accepted topics in `expect.accepted_events` must all appear in
///   the recorded trace (set membership, order-independent)
/// - payload_fields (when provided) must match the recorded accepted
///   payload for that topic (when at least one accepted payload exists)
/// - `terminal: none` plus no expected accepted topics is a pass ONLY
///   when the driver consumed every response — an `UnclosedTerminal`
///   failure from the driver (next_hat returned None early) is NOT
///   turned into a pass.
///
/// On a verdict mismatch, the failure_kind is rewritten from the
/// driver's coarse classification to the more precise verdict class
/// (e.g. `ScenarioFailure` for terminal mismatches, `UnclosedTerminal`
/// when the terminal topic never fired). The trace itself is preserved.
pub fn evaluate_scenario(outcome: ScenarioOutcome) -> VerifyReportScenario {
    use std::collections::HashSet;
    let scenario = &outcome.trace.scenario;
    let accepted: HashSet<&str> =
        outcome.trace.accepted_events.iter().map(String::as_str).collect();
    let forbidden_violated: Vec<String> = scenario
        .expect
        .forbidden_events
        .iter()
        .filter(|t| accepted.contains(t.as_str()))
        .cloned()
        .collect();

    // accepted_events: every expected topic must appear in the trace.
    // Order is intentionally not enforced at the verdict level — the
    // driver's ordered trace preserves order, but the `expect.accepted_events`
    // contract is set membership.
    let missing_accepted: Vec<String> = scenario
        .expect
        .accepted_events
        .iter()
        .filter(|t| !accepted.contains(t.as_str()))
        .cloned()
        .collect();

    // Driver exited early (e.g. next_hat returned None mid-response-list).
    // Even when terminal is `none`, an unclosed run must not pass.
    let driver_unclosed = matches!(
        outcome.failure_kind,
        Some(FailureKind::UnclosedTerminal(_))
            | Some(FailureKind::Timeout(_))
            | Some(FailureKind::NoProgress(_))
    );
    let exhausted_unclosed = outcome.trace.steps.len() < scenario.responses.len();

    // Terminal check: when terminal != None, the expected terminal topic
    // must appear in the accepted trace.
    let terminal_topic_seen = scenario
        .expect
        .terminal_topic
        .as_ref()
        .map(|t| accepted.contains(t.as_str()))
        .unwrap_or(false);
    let terminal_ok = match scenario.expect.terminal {
        TerminalKind::None => !driver_unclosed && !exhausted_unclosed,
        _ => terminal_topic_seen,
    };

    // Forbidden + missing check.
    let expected_ok = missing_accepted.is_empty() && forbidden_violated.is_empty();

    let (passed, failure_kind_override) = if terminal_ok && expected_ok {
        (true, None)
    } else {
        let mut detail = String::new();
        if driver_unclosed {
            detail.push_str(&format!(
                "driver exited early: outcome={:?}; ",
                outcome.failure_kind
            ));
        } else if exhausted_unclosed && matches!(scenario.expect.terminal, TerminalKind::None) {
            detail.push_str(&format!(
                "responses exhausted without terminal: consumed {} of {} responses; ",
                outcome.trace.steps.len(),
                scenario.responses.len()
            ));
        } else if !terminal_ok {
            detail.push_str(&format!(
                "expected terminal={:?} topic={:?} but trace ended with accepted={:?}; ",
                scenario.expect.terminal, scenario.expect.terminal_topic, outcome.trace.accepted_events
            ));
        }
        if !missing_accepted.is_empty() {
            detail.push_str(&format!("missing expected topics: {missing_accepted:?}; "));
        }
        if !forbidden_violated.is_empty() {
            detail.push_str(&format!("forbidden topics observed: {forbidden_violated:?}; "));
        }
        let kind = if matches!(scenario.expect.terminal, TerminalKind::None)
            && matches!(outcome.failure_kind, Some(FailureKind::NoProgress(_)))
        {
            // Driver classified the run as no-progress (deterministic budget
            // exhaustion). Preserve the driver's verdict under terminal: None
            // so downstream consumers can distinguish budget exhaustion from
            // contract mismatch.
            FailureKind::NoProgress(detail)
        } else if matches!(outcome.failure_kind, Some(FailureKind::UnclosedTerminal(_))) {
            FailureKind::UnclosedTerminal(detail)
        } else if matches!(outcome.failure_kind, Some(FailureKind::Timeout(_))) {
            FailureKind::Timeout(detail)
        } else if !terminal_ok && scenario.expect.terminal != TerminalKind::None {
            FailureKind::UnclosedTerminal(detail)
        } else {
            FailureKind::ScenarioFailure(detail)
        };
        (false, Some(kind))
    };

    let effective_failure = failure_kind_override.or(outcome.failure_kind);
    let failure_tag = effective_failure.as_ref().map(FailureKind::tag);

    VerifyReportScenario {
        name: scenario.name.clone(),
        passed,
        steps: outcome.trace.steps.len(),
        accepted_events: outcome.trace.accepted_events.clone(),
        rejected_events: outcome.trace.rejected_events.clone(),
        terminal_topic: outcome.trace.terminal_topic.clone(),
        termination: outcome.trace.last_runtime_termination.clone(),
        failure_kind: failure_tag.map(str::to_string),
        last_observable_state: LastObservableState {
            step: outcome.trace.steps.last().map(|s| s.step),
            last_hat: outcome.trace.last_hat.clone(),
            last_accepted_topic: outcome.trace.last_accepted_topic.clone(),
            last_runtime_termination: outcome.trace.last_runtime_termination.clone(),
            response_index: outcome.trace.steps.last().map(|s| s.response_index),
        },
        trace_digest: outcome.trace.trace_digest.clone(),
    }
}

/// Build the full report from per-scenario outcomes and the static layer.
pub fn build_report(
    source_kind: SourceKind,
    static_layer: StaticLayer,
    outcomes: Vec<(ScenarioOutcome, VerifyReportScenario)>,
    overall_failure: Option<&FailureKind>,
    input_blob: &str,
) -> PresetVerifyReport {
    let passed = outcomes.iter().all(|(_, s)| s.passed) && overall_failure.is_none();
    let scenarios: Vec<VerifyReportScenario> =
        outcomes.into_iter().map(|(_, s)| s).collect();
    let failure_kind_tag = overall_failure.map(FailureKind::tag).map(str::to_string);
    let trace_digest = if let Some(last) = scenarios.last() {
        last.trace_digest.clone()
    } else {
        compute_trace_digest_for_empty(input_blob)
    };
    let last_observable_state = scenarios
        .last()
        .map(|s| s.last_observable_state.clone())
        .unwrap_or_default();
    PresetVerifyReport {
        passed,
        source_kind,
        static_layer,
        scenarios,
        failure_kind: failure_kind_tag,
        last_observable_state,
        trace_digest,
    }
}

fn compute_trace_digest_for_empty(input_blob: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"empty-report\ninput=");
    h.update(input_blob.as_bytes());
    hex_encode(&h.finalize())
}

// Re-export `Event` so callers building static-layer helpers can construct
// payloads without an extra import.
pub use ralph_proto::Event as ProtoEvent;
pub use ralph_proto::HatId as ProtoHatId;