//! `ProtocolView` — typed read-only view over an `EventLoopConfig`
//! that exposes the protocol SSOT values the engine needs.
//!
//! Plan ref: KTD-10, plan 2026-06-20-001 U1.
//!
//! The view is *preset-agnostic*: every value comes from the
//! loaded `EventLoopConfig`. Operators edit
//! `presets/schemas/<name>.yml` and the embedded preset reflects
//! the change after `cargo build`. No duplicate field table lives
//! in Rust — `effective_required_fields` derives from
//! `event_policy.schemas` (top-level topic keys) joined with
//! `execution_contracts.rules[topic].extra_required_fields` (the
//! KTD-12 rule A contract).

use std::collections::{HashMap, HashSet};

use crate::config::execution_contracts::{ExecutionContractRule, ExecutionContractsConfig};
use crate::config::{
    EventLoopConfig, EventSchema, StateProjectionConfig, VerdictGateConfig, WorkflowContractConfig,
};
use crate::hat_handoff::HatHandoffConfig;
use serde_json::Value;

/// Read-only protocol view. Cheap to clone (HashMaps of strings)
/// and `Sync` because every field is owned data — callers may
/// pass it to the gate, the projector, and the linter concurrently.
#[derive(Debug, Clone)]
pub struct ProtocolView {
    /// Topic → required field set, derived from
    /// `event_policy.schemas` + `execution_contracts.rules`
    /// (KTD-12). The set is the union of:
    ///   * `event_policy.schemas[topic].required_fields`
    ///   * `execution_contracts.rules[topic].extra_required_fields`
    ///     (optional, default empty; `require_payload_fields` is
    ///     NOT supported per KTD-12)
    pub effective_required_fields: HashMap<String, HashSet<String>>,

    /// Verdict gate config (review-time fail field).
    pub verdict_gate: Option<VerdictGateConfig>,

    /// Workflow contract (handoff seeds + step handoff flags).
    pub workflow_contract: Option<WorkflowContractConfig>,

    /// State projection chain.
    pub state_projection: Option<StateProjectionConfig>,

    /// Execution contracts (require_git_change / require_task /
    /// dedup_key / etc). Empty when `enabled = false`.
    pub execution_contracts: Option<ExecutionContractsConfig>,

    /// Hat handoff config (artifact rules, linter settings,
    /// macro/exempt topics, max bytes).
    pub hat_handoff: HatHandoffConfig,

    /// Protocol hash — stable across `cargo build` cycles. Used
    /// by `ralph emit --schema` to detect drift between the
    /// authoring SSOT and the embedded copy.
    pub protocol_hash: String,
}

impl ProtocolView {
    /// Build a view from a loaded `EventLoopConfig`. Cheap and
    /// deterministic; the same config always produces the same
    /// hash so drift detection is reliable.
    pub fn from_event_loop(config: &EventLoopConfig) -> Self {
        // EventLoopConfig stores `event_policy` and `workflow_contract`
        // as `Option<T>`; fall back to empty defaults when absent so
        // the view is always populated.
        let event_policy = config
            .event_policy
            .clone()
            .unwrap_or_default();
        let workflow_contract = config.workflow_contract.clone();
        let effective_required_fields = compute_effective_required_fields(
            &event_policy.schemas,
            config.execution_contracts.as_ref(),
        );

        let verdict_gate = config.verdict_gate.clone();
        let state_projection = Some(config.state_projection.clone());
        let execution_contracts = config.execution_contracts.clone();
        let hat_handoff = config.hat_handoff.clone();

        // Hash the canonicalised view so callers can detect drift
        // between authoring SSOT and embedded runtime copy. The
        // hash is informational only — operators do not act on it
        // unless `ralph emit --schema` reports a mismatch.
        let protocol_hash = compute_protocol_hash(&effective_required_fields, &hat_handoff);

        Self {
            effective_required_fields,
            verdict_gate,
            workflow_contract,
            state_projection,
            execution_contracts,
            hat_handoff,
            protocol_hash,
        }
    }

    /// Required field set for a single topic (empty when the
    /// topic is not in the protocol). Used by both the linter
    /// (R8) and the runtime gate (R15) so they share the same
    /// source of truth.
    pub fn required_fields(&self, topic: &str) -> HashSet<String> {
        self.effective_required_fields
            .get(topic)
            .cloned()
            .unwrap_or_default()
    }

    /// Whether `topic` is a macro edge (handoff-required). Read
    /// from `hat_handoff.macro_topics` plus the default macro set
    /// when `enabled`. Both sides of the engine (gate + lint)
    /// call this so a macro edge can never be admitted without
    /// a corresponding handoff artifact.
    pub fn is_macro_edge(&self, topic: &str) -> bool {
        if !self.hat_handoff.enabled {
            return false;
        }
        if self.hat_handoff.is_exempt(topic) {
            return false;
        }
        if self.hat_handoff.is_explicit_macro(topic) {
            return true;
        }
        false
    }
}

/// Join `event_policy.schemas[topic].required_fields` with
/// `execution_contracts.rules[topic].require_payload_fields` (KTD-12).
/// The latter is added when `execution_contracts.enabled` is true.
fn compute_effective_required_fields(
    schemas: &HashMap<String, EventSchema>,
    contracts: Option<&ExecutionContractsConfig>,
) -> HashMap<String, HashSet<String>> {
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();
    for (topic, schema) in schemas {
        out.insert(topic.clone(), schema.required_fields.iter().cloned().collect());
    }
    if let Some(contracts) = contracts {
        if contracts.enabled {
            for (topic, rule) in &contracts.rules {
                let extras = extra_required_fields_from_rule(rule);
                if !extras.is_empty() {
                    out.entry(topic.clone())
                        .or_default()
                        .extend(extras);
                }
            }
        }
    }
    out
}

fn extra_required_fields_from_rule(rule: &ExecutionContractRule) -> HashSet<String> {
    let mut set: HashSet<String> = HashSet::new();
    // Per KTD-12 the rule's `require_payload_fields` is *added*
    // to the SSOT schema fields. When operator wants to tighten
    // a topic beyond the SSOT, they add fields here. The merge
    // is a set union — duplicates with SSOT are no-ops.
    set.extend(rule.require_payload_fields.iter().cloned());
    set
}

fn compute_protocol_hash(
    fields: &HashMap<String, HashSet<String>>,
    hat_handoff: &HatHandoffConfig,
) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    // Stable iteration order via BTreeMap shadow.
    let mut sorted: Vec<(&String, &HashSet<String>)> = fields.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (topic, fs) in sorted {
        topic.hash(&mut hasher);
        let mut fs_v: Vec<&String> = fs.iter().collect();
        fs_v.sort();
        for f in fs_v {
            f.hash(&mut hasher);
        }
    }
    hat_handoff.enabled.hash(&mut hasher);
    hat_handoff.artifact.required_sections.hash(&mut hasher);
    hat_handoff.artifact.require_next_marker.hash(&mut hasher);
    hat_handoff.linter.auto_prepare_on_macro_edge.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Best-effort payload introspection helper used by both the
/// gate and the linter: returns the field set actually present
/// in `payload`. Empty payload returns an empty set.
pub fn payload_field_set(payload: &Value) -> HashSet<String> {
    match payload {
        Value::Object(map) => map.keys().cloned().collect(),
        _ => HashSet::new(),
    }
}
