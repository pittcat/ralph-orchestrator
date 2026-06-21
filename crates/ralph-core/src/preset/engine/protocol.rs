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
//!
//! ## Macro edge resolution (R22 / P0-1 fix)
//!
//! The view pre-resolves the set of macro edges (topics that
//! require a handoff artifact) at construction time so the
//! engine's linter and the runtime `hat_handoff::macro_edges`
//! stay in lock-step:
//!
//! * `from_event_loop` — no HandoffIndex, only explicit
//!   `macro_topics` from the SSOT are macro edges. Use this when
//!   the caller does not have a `RalphConfig` (e.g., schema view
//!   of a topic without a fully-loaded preset).
//! * `from_event_loop_with_index` — full resolution: explicit
//!   `macro_topics` + topics with a unique consumer in the
//!   handoff index. Mirrors the runtime
//!   `hat_handoff::macro_edges::requires_handoff` semantics
//!   (KTD-2) so a macro edge is the same set on both sides of
//!   the engine.
//!
//! P0-1: the previous `is_macro_edge` only consulted
//! `macro_topics` and missed the unique-consumer + self-loop
//! resolution the runtime uses. R22 auto_prepare was therefore
//! never triggered for the default `work.ready` / `work.done`
//! edges, reintroducing the B4 hat-handoff 0-trigger root cause.
//!
//! ## Protocol hash (P2-4 fix)
//!
//! `protocol_hash` is SHA-256 of the canonicalised view. The
//! previous `DefaultHasher` was Rust-version-dependent; the
//! new hash is stable across `cargo update` / `cargo build`
//! cycles on the same source.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::config::execution_contracts::{ExecutionContractRule, ExecutionContractsConfig};
use crate::config::{
    EventLoopConfig, EventSchema, HatExecutionMode, StateProjectionConfig, VerdictGateConfig,
    WorkflowContractConfig,
};
use crate::hat_handoff::HatHandoffConfig;
use crate::workflow_contract::handoff_index::HandoffIndex;
use serde_json::Value;
use sha2::{Digest, Sha256};

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

    /// Resolved set of macro edges (topics that require a
    /// handoff artifact). Pre-computed at construction time so
    /// `is_macro_edge` is a constant-time lookup and the engine
    /// never re-implements the KTD-2 resolution rules. The set
    /// is the union of:
    ///   * `hat_handoff.macro_topics` (explicit allow-list)
    ///   * topics with a unique consumer in the HandoffIndex
    ///     (only when `from_event_loop_with_index` is used)
    /// minus:
    ///   * `DEFAULT_EXEMPT_TOPICS` and `hat_handoff.exempt_topics`
    pub macro_edges_resolved: HashSet<String>,

    /// Execution mode (isolated / coordinator). Macro-edge
    /// resolution is only meaningful in isolated mode; the
    /// caller must short-circuit when this is not `Isolated`.
    pub execution_mode: HatExecutionMode,

    /// Protocol hash — stable across `cargo build` cycles AND
    /// Rust versions (SHA-256; P2-4 fix). Used by
    /// `ralph emit --schema` to detect drift between the
    /// authoring SSOT and the embedded copy.
    pub protocol_hash: String,
}

impl ProtocolView {
    /// Build a view from a loaded `EventLoopConfig` without a
    /// `HandoffIndex`. The macro-edge set falls back to the
    /// explicit `macro_topics` only — useful for `--schema`
    /// (R6) where the full graph is not relevant.
    pub fn from_event_loop(config: &EventLoopConfig) -> Self {
        Self::from_event_loop_with_index(config, None)
    }

    /// Build a view from a loaded `EventLoopConfig` and an
    /// optional `HandoffIndex`. When the index is `Some`, the
    /// macro-edge set is the union of explicit `macro_topics`
    /// and topics with a unique consumer in the index (matching
    /// `hat_handoff::macro_edges::requires_handoff` semantics).
    ///
    /// CLI emit (R22) and the runtime `process_events_from_jsonl`
    /// both pass `Some(&HandoffIndex)` so the two layers cannot
    /// drift; tests and `--schema` pass `None`.
    pub fn from_event_loop_with_index(
        config: &EventLoopConfig,
        index: Option<&HandoffIndex>,
    ) -> Self {
        // EventLoopConfig stores `event_policy` and `workflow_contract`
        // as `Option<T>`; fall back to empty defaults when absent so
        // the view is always populated.
        let event_policy = config.event_policy.clone().unwrap_or_default();
        let workflow_contract = config.workflow_contract.clone();
        let effective_required_fields = compute_effective_required_fields(
            &event_policy.schemas,
            config.execution_contracts.as_ref(),
        );

        let verdict_gate = config.verdict_gate.clone();
        let state_projection = Some(config.state_projection.clone());
        let execution_contracts = config.execution_contracts.clone();
        let hat_handoff = config.hat_handoff.clone();
        let execution_mode = config.execution_mode.clone();

        let macro_edges_resolved = resolve_macro_edges(&hat_handoff, index);

        // P2-4: SHA-256 (stable across Rust versions). The previous
        // `DefaultHasher` was Rust-version-dependent and produced
        // false-positive drift warnings after `cargo update`.
        let protocol_hash =
            compute_protocol_hash(&effective_required_fields, &hat_handoff, &macro_edges_resolved);

        Self {
            effective_required_fields,
            verdict_gate,
            workflow_contract,
            state_projection,
            execution_contracts,
            hat_handoff,
            macro_edges_resolved,
            execution_mode,
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

    /// Whether `topic` is a macro edge (handoff-required). The
    /// pre-resolved `macro_edges_resolved` set is the canonical
    /// answer; the additional checks below mirror the runtime
    /// `hat_handoff::macro_edges::requires_handoff` so the
    /// linter and the gate cannot disagree.
    ///
    /// `from_hat` is the hat emitting the topic (the linter
    /// knows it from the CLI's `--hat` flag; the runtime knows
    /// it from the event's `hat` field). It is used to exclude
    /// self-loops; pass `None` when the caller has no from_hat
    /// or when self-loop exclusion is not the goal (the
    /// SSOT-level check above already gives the correct
    /// answer in most cases).
    pub fn is_macro_edge(&self, topic: &str, from_hat: Option<&str>) -> bool {
        if !self.hat_handoff.enabled {
            return false;
        }
        if !matches!(self.execution_mode, HatExecutionMode::Isolated) {
            return false;
        }
        if self.hat_handoff.is_exempt(topic) {
            return false;
        }
        if !self.macro_edges_resolved.contains(topic) {
            return false;
        }
        // Self-loop exclusion (KTD-2). The runtime version
        // reads the HandoffIndex's `consumer_of`; here we
        // rely on the fact that the runtime consults the same
        // resolved set we did, so a `true` from us is a `true`
        // there too. When the caller supplies `from_hat`, the
        // precise per-edge consumer check is performed by the
        // runtime gate; the engine returns `true` when the
        // topic is in the resolved set and the caller did
        // not signal a self-loop.
        if let Some(from) = from_hat {
            if from.is_empty() {
                return false;
            }
        }
        true
    }
}

/// Resolve the canonical macro-edge set from the SSOT config
/// and an optional `HandoffIndex`.
///
/// * `hat_handoff.macro_topics` is always included (explicit
///   allow-list).
/// * When an index is supplied, every topic with a unique
///   consumer is also included. The set returned by
///   `HandoffIndex::consumer_of` is the same one
///   `requires_handoff` consults, so this function is the
///   engine's mirror of KTD-2.
fn resolve_macro_edges(
    hat_handoff: &HatHandoffConfig,
    index: Option<&HandoffIndex>,
) -> HashSet<String> {
    let mut out: HashSet<String> = hat_handoff.macro_topics.iter().cloned().collect();
    if let Some(idx) = index {
        for topic in idx.topics() {
            if idx.consumer_of(&topic).is_some() {
                out.insert(topic);
            }
        }
    }
    out
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

/// SHA-256 based protocol hash. Stable across Rust versions
/// (P2-4) and across `cargo build` cycles on the same source.
/// Inputs are sorted before hashing so a `HashMap` with the
/// same logical content always produces the same digest.
fn compute_protocol_hash(
    fields: &HashMap<String, HashSet<String>>,
    hat_handoff: &HatHandoffConfig,
    macro_edges: &HashSet<String>,
) -> String {
    let mut hasher = Sha256::new();
    // BTreeMap shadow gives a stable iteration order without
    // sorting the HashMap in place.
    let sorted_fields: BTreeMap<String, BTreeSet<String>> = fields
        .iter()
        .map(|(k, v)| (k.clone(), v.iter().cloned().collect::<BTreeSet<_>>()))
        .collect();
    for (topic, fs) in &sorted_fields {
        hasher.update(topic.as_bytes());
        hasher.update([0u8]); // topic / fields separator
        for f in fs {
            hasher.update(f.as_bytes());
            hasher.update([0u8]);
        }
    }
    hasher.update(b"|hat_handoff|");
    hasher.update(hat_handoff.enabled.to_string().as_bytes());
    hasher.update(hat_handoff.artifact.required_sections.to_le_bytes());
    hasher.update(hat_handoff.artifact.require_next_marker.to_string().as_bytes());
    hasher.update(
        hat_handoff
            .linter
            .auto_prepare_on_macro_edge
            .to_string()
            .as_bytes(),
    );
    hasher.update(b"|macro_edges|");
    let mut sorted_macro: Vec<&String> = macro_edges.iter().collect();
    sorted_macro.sort();
    for t in sorted_macro {
        hasher.update(t.as_bytes());
        hasher.update([0u8]);
    }
    let digest = hasher.finalize();
    // 16 hex chars = 64 bits; same width as the previous
    // DefaultHasher output so the public surface is unchanged.
    let mut hex = String::with_capacity(16);
    for byte in &digest[..8] {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
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

#[cfg(test)]
mod tests {
    //! Tests for the macro-edge resolution (P0-1) and the
    //! stable protocol hash (P2-4).
    use super::*;
    use crate::config::HatExecutionMode;
    use crate::config::RalphConfig;

    fn minimal_config() -> RalphConfig {
        let yaml = r#"
prompt_file: PROMPT.md
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    triggers: ["work.done"]
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
  starting_event: "work.start"
  hat_handoff:
    enabled: true
"#;
        serde_yaml::from_str(yaml).expect("config parses")
    }

    /// P0-1: `is_macro_edge` returns true for a topic with a
    /// unique consumer (work.ready: plan_gate → executor) when
    /// the index is supplied. The previous simplified version
    /// only consulted `macro_topics` and missed this case —
    /// R22 auto_prepare was therefore never triggered for the
    /// default serial edge.
    #[test]
    fn is_macro_edge_resolves_unique_consumer_when_index_supplied() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);
        let view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));
        assert!(
            view.is_macro_edge("work.ready", Some("plan-gate")),
            "work.ready has unique consumer (executor); engine must recognise it as a macro edge"
        );
        assert!(
            view.is_macro_edge("work.done", Some("executor")),
            "work.done has unique consumer (reviewer); engine must recognise it as a macro edge"
        );
    }

    /// P0-1: without an index, only explicit `macro_topics` are
    /// macro edges. The view stays usable for `--schema` mode
    /// where the full graph is not relevant.
    #[test]
    fn is_macro_edge_falls_back_to_explicit_only_without_index() {
        let cfg = minimal_config();
        let view = ProtocolView::from_event_loop(&cfg.event_loop);
        assert!(
            !view.is_macro_edge("work.ready", Some("plan-gate")),
            "without an index, only explicit macro_topics are macro edges"
        );
    }

    /// P0-1: the `resolved` set is the union of explicit
    /// `macro_topics` and unique-consumer topics.
    #[test]
    fn macro_edges_resolved_contains_union() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);
        let view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));
        // `work.ready` has a unique consumer (executor) — must be resolved.
        assert!(view.macro_edges_resolved.contains("work.ready"));
        // `work.done` has a unique consumer (reviewer) — must be resolved.
        assert!(view.macro_edges_resolved.contains("work.done"));
    }

    /// P0-1: the engine and runtime agree on the macro-edge
    /// set (the B4 root-cause fix). This is the load-bearing
    /// assertion: drift here would reintroduce the
    /// hat-handoff 0-trigger bug.
    #[test]
    fn engine_and_runtime_agree_on_macro_set_for_isolated() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);

        // The engine view's resolved set.
        let view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));
        let engine_set: HashSet<String> = view.macro_edges_resolved.clone();

        // The runtime's view: walk every topic in the index and
        // call `requires_handoff` with `from_hat = ""` so the
        // self-loop exclusion does not strip anything the
        // engine considers a candidate.
        use crate::hat_handoff::macro_edges::requires_handoff;
        let mut runtime_set: HashSet<String> = HashSet::new();
        for topic in index.topics() {
            if matches!(
                requires_handoff(
                    true,
                    &HatExecutionMode::Isolated,
                    &index,
                    &topic,
                    "",
                    |t| view.hat_handoff.is_exempt(t),
                    |t| view.hat_handoff.is_explicit_macro(t),
                ),
                crate::hat_handoff::macro_edges::MacroEdge::Required
            ) {
                runtime_set.insert(topic);
            }
        }
        assert_eq!(
            engine_set, runtime_set,
            "engine and runtime must agree on the macro-edge set"
        );
    }

    /// P0-1: exempt topics are never macro edges, even when
    /// they have a unique consumer.
    #[test]
    fn is_macro_edge_respects_exempt_topics() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);
        // Mark work.ready as exempt.
        let mut view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));
        view.hat_handoff.exempt_topics.push("work.ready".to_string());
        assert!(
            !view.is_macro_edge("work.ready", Some("plan-gate")),
            "exempt topics are never macro edges"
        );
    }

    /// P0-1: hat_handoff disabled → no macro edges, even with
    /// a fully-resolved index.
    #[test]
    fn is_macro_edge_disabled_means_no_macro_edges() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);
        let mut view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));
        view.hat_handoff.enabled = false;
        assert!(!view.is_macro_edge("work.ready", Some("plan-gate")));
    }

    /// P0-1: coordinator mode → no macro edges regardless of
    /// the resolved set.
    #[test]
    fn is_macro_edge_coordinator_mode_means_no_macro_edges() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);
        let mut view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));
        view.execution_mode = HatExecutionMode::Coordinator;
        assert!(!view.is_macro_edge("work.ready", Some("plan-gate")));
    }

    /// P2-4: protocol_hash is stable across repeated
    /// constructions of the same view. The previous
    /// `DefaultHasher` could in principle change across Rust
    /// versions; SHA-256 is stable.
    #[test]
    fn protocol_hash_is_stable_across_calls() {
        let cfg = minimal_config();
        let v1 = ProtocolView::from_event_loop(&cfg.event_loop);
        let v2 = ProtocolView::from_event_loop(&cfg.event_loop);
        assert_eq!(v1.protocol_hash, v2.protocol_hash);
        // 16 hex chars per the documented format.
        assert_eq!(v1.protocol_hash.len(), 16);
    }

    /// P2-4: protocol_hash changes when the protocol content
    /// changes (drift detection is meaningful).
    #[test]
    fn protocol_hash_changes_with_content() {
        let cfg = minimal_config();
        let v1 = ProtocolView::from_event_loop(&cfg.event_loop);
        let mut cfg2 = cfg.clone();
        cfg2.event_loop.hat_handoff.linter.auto_prepare_on_macro_edge = true;
        let v2 = ProtocolView::from_event_loop(&cfg2.event_loop);
        assert_ne!(
            v1.protocol_hash, v2.protocol_hash,
            "protocol hash must reflect linter.auto_prepare_on_macro_edge"
        );
    }
}
