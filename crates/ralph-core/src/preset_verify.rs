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