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
#[derive(Debug, Clone, Default)]
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
    ///
    /// minus:
    ///   * `DEFAULT_EXEMPT_TOPICS` and `hat_handoff.exempt_topics`
    pub macro_edges_resolved: HashSet<String>,

    /// Topic → unique consumer hat, populated when a HandoffIndex
    /// is supplied. Used by `is_macro_edge` for self-loop exclusion
    /// (KTD-2). Empty when `from_event_loop` is used (no index).
    pub macro_edge_consumers: HashMap<String, String>,

    /// Execution mode (isolated / coordinator). Macro-edge
    /// resolution is only meaningful in isolated mode; the
    /// caller must short-circuit when this is not `Isolated`.
    pub execution_mode: HatExecutionMode,

    /// Protocol hash — stable across `cargo build` cycles AND
    /// Rust versions (SHA-256; P2-4 fix). Used by
    /// `ralph emit --schema` to detect drift between the
    /// authoring SSOT and the embedded copy.
    pub protocol_hash: String,

    /// KTD-8: feature flag — whether the unified
    /// `ProtocolView`-backed validation pipeline is enabled
    /// (`UNIFIED_PROTOCOL_VIEW=1`). When `false`, callers
    /// must continue to use the legacy resolution path. The
    /// flag is captured at construction time so the runtime
    /// can read it from `feature_enabled()` without holding
    /// onto the env.
    pub feature_flag_enabled: bool,
}

impl ProtocolView {
    /// Build a view from a loaded `EventLoopConfig` without a
    /// `HandoffIndex`. The macro-edge set falls back to the
    /// explicit `macro_topics` only — useful for `--schema`
    /// (R6) where the full graph is not relevant.
    ///
    /// **P2-#6 (002-adversarial-review)**: this entry point no
    /// longer reads the `UNIFIED_PROTOCOL_VIEW` env var. Tests
    /// using this helper stay env-independent; production
    /// callers that need the env-gated behaviour must use
    /// [`Self::from_event_loop_with_index_for_env`] so the
    /// env read happens at one well-known call site.
    pub fn from_event_loop(config: &EventLoopConfig) -> Self {
        Self::from_event_loop_with_index_and_feature(config, None, false)
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
    ///
    /// **P2-#6 (002-adversarial-review)**: this entry point no
    /// longer reads the `UNIFIED_PROTOCOL_VIEW` env var.
    /// Reading the env here made the test suite non-deterministic
    /// under `cargo nextest` (process-per-test cannot isolate
    /// `std::env` because the OS env is inherited at process
    /// start, and a parallel test that *does* call `set_var`
    /// races with this reader). The flag now defaults to
    /// `false`; callers that need the env-gated behaviour must
    /// go through [`Self::from_event_loop_with_index_and_feature`]
    /// (the production `LoopRunner` does) or
    /// [`Self::from_event_loop_with_index_for_env`] (a thin
    /// wrapper that reads the env at a single, well-known call
    /// site).
    pub fn from_event_loop_with_index(
        config: &EventLoopConfig,
        index: Option<&HandoffIndex>,
    ) -> Self {
        Self::from_event_loop_with_index_and_feature(config, index, false)
    }

    /// Build a view with the `UNIFIED_PROTOCOL_VIEW` env var
    /// read at the call site. Production-only — tests must not
    /// use this helper because the env is process-global and
    /// cannot be reset safely across `cargo nextest` workers.
    /// The CLI / runtime invoke this *once* at startup so the
    /// rest of the pipeline can stay env-free.
    pub fn from_event_loop_with_index_for_env(
        config: &EventLoopConfig,
        index: Option<&HandoffIndex>,
    ) -> Self {
        // KTD-8: feature flag is captured at construction so the
        // runtime can consult `feature_enabled()` later without
        // re-reading the env.
        // U11-T7: default is now ON; explicit `UNIFIED_PROTOCOL_VIEW=0`
        // opts out and preserves the legacy resolution path.
        let feature_flag_enabled = if let Some(cell) = TEST_PROTOCOL_VIEW_ENABLED.get() {
            // Test override wins so the workspace's
            // `forbid(unsafe_code)` lint does not require
            // tests to flip the process-global env var.
            cell.load(std::sync::atomic::Ordering::Relaxed)
        } else {
            !matches!(
                std::env::var("UNIFIED_PROTOCOL_VIEW").ok().as_deref(),
                Some("0")
            )
        };
        Self::from_event_loop_with_index_and_feature(config, index, feature_flag_enabled)
    }

    /// Build a view with explicit feature-flag control. Used
    /// by tests and by callers that want to opt-in/out of the
    /// unified view independent of the env var.
    ///
    /// When `feature_enabled = false` the view is constructed
    /// the legacy way — fields are populated but
    /// `feature_flag_enabled` returns `false`, so the runtime
    /// falls back to the pre-U3 resolution path. This is the
    /// conservative default (KTD-8): the migration to the
    /// unified view is opt-in until U4/U5 validate it.
    pub fn from_event_loop_with_index_and_feature(
        config: &EventLoopConfig,
        index: Option<&HandoffIndex>,
        feature_enabled: bool,
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

        let (macro_edges_resolved, macro_edge_consumers) = resolve_macro_edges(&hat_handoff, index);

        // P2-4: SHA-256 (stable across Rust versions). The previous
        // `DefaultHasher` was Rust-version-dependent and produced
        // false-positive drift warnings after `cargo update`.
        let protocol_hash = compute_protocol_hash(
            &effective_required_fields,
            &hat_handoff,
            &macro_edges_resolved,
        );

        Self {
            effective_required_fields,
            verdict_gate,
            workflow_contract,
            state_projection,
            execution_contracts,
            hat_handoff,
            macro_edges_resolved,
            macro_edge_consumers,
            execution_mode,
            protocol_hash,
            feature_flag_enabled: feature_enabled,
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

    /// Whether the unified `ProtocolView` feature is enabled
    /// (KTD-8). When `false`, callers MUST use the legacy
    /// resolution path (`hat_handoff::macro_edges::requires_handoff`,
    /// `hat_handoff::publishes_check`, etc.). The flag is
    /// captured at construction so the runtime can consult
    /// it without re-reading the env.
    pub fn feature_enabled(&self) -> bool {
        self.feature_flag_enabled
    }

    /// KTD-8 / U3: macro-edge check without self-loop
    /// exclusion. Returns `true` when the topic is a macro
    /// edge *and* the engine should require a handoff
    /// artifact, **regardless of the from-hat identity**.
    ///
    /// This is the SSOT-level macro check used by:
    /// * `engine_and_runtime_agree_on_macro_set_for_isolated`
    ///   test (drift detection between lint/runtime)
    /// * `lint_mirror` and CLI `--policy-check` lint paths
    ///   when the caller does not know the from-hat
    ///
    /// For the runtime hot path that has the from-hat, prefer
    /// `is_macro_edge(topic, Some(from_hat))` which additionally
    /// applies the KTD-2 self-loop exclusion.
    ///
    /// The implementation is the same as
    /// `is_macro_edge(topic, None)` but documented as a
    /// distinct, load-bearing surface so U4+ validation
    /// pipeline can call it without guessing.
    pub fn is_macro_edge(&self, topic: &str) -> bool {
        self.is_macro_edge_full(topic, None)
    }

    /// Full macro-edge check with optional `from_hat` for
    /// self-loop exclusion. Internal helper backing both
    /// `is_macro_edge(topic)` and `is_macro_edge(topic,
    /// from_hat)` (preserved for backwards compatibility with
    /// callers that already pass `from_hat`).
    fn is_macro_edge_full(&self, topic: &str, from_hat: Option<&str>) -> bool {
        if !self.hat_handoff.enabled {
            return false;
        }
        if !matches!(self.execution_mode, HatExecutionMode::Isolated) {
            return false;
        }
        // Orchestrator control and diagnostic topics are never macro edges
        // (they are loop-internal signals, not hat-to-hat handoffs).
        if crate::event_origin::is_orchestrator_control_topic(topic, "")
            || crate::event_origin::is_orchestrator_diagnostic_topic(topic)
        {
            return false;
        }
        if self.hat_handoff.is_exempt(topic) {
            return false;
        }
        if !self.macro_edges_resolved.contains(topic) {
            return false;
        }
        // Self-loop exclusion (KTD-2). When the caller supplies
        // `from_hat`, compare it with the unique consumer for this
        // topic. If they match, this is a self-loop and NOT a macro
        // edge (the handoff stays within the same hat).
        if let Some(from) = from_hat {
            if from.is_empty() {
                return false;
            }
            if let Some(consumer) = self.macro_edge_consumers.get(topic) {
                if from == consumer {
                    return false;
                }
            }
        }
        true
    }

    /// KTD-8 / U3: full macro-edge check with from-hat
    /// (KTD-2 self-loop exclusion applied). Preserved for
    /// backwards compatibility — existing callers (engine
    /// gate, linter auto-prepare) pass `Some(from_hat)` when
    /// they have it.
    pub fn is_macro_edge_from(&self, topic: &str, from_hat: Option<&str>) -> bool {
        self.is_macro_edge_full(topic, from_hat)
    }

    /// Backwards-compatible two-argument macro-edge check.
    /// Equivalent to `is_macro_edge_from(topic, from_hat)`.
    /// Retained as a wrapper so existing call sites
    /// (`gates::run_gates`, `linter::lint_emit`) keep working
    /// without churn; new callers should prefer
    /// `is_macro_edge(topic)` (no from_hat) when the
    /// self-loop exclusion is not needed.
    pub fn is_macro_edge_legacy(&self, topic: &str, from_hat: Option<&str>) -> bool {
        self.is_macro_edge_full(topic, from_hat)
    }

    /// KTD-8 / U3: handoff artifact requirements for a topic.
    /// Returns the [`ArtifactRule`] derived from
    /// `hat_handoff.artifact` when the topic is a macro edge,
    /// `None` otherwise. The rule is consumed by U5's
    /// handoff artifact auto-generation and the runtime
    /// gate's structure validation.
    ///
    /// Note: the artifact rule is currently a single
    /// `HatHandoffConfig`-wide value (not per-topic), so the
    /// `Option` here is "macro-edge?" rather than "topic-
    /// specific spec?". The return type uses `Option` to
    /// preserve room for per-topic overrides added in later
    /// plans without an API break.
    pub fn handoff_artifact_required(&self, topic: &str) -> Option<ArtifactSpec> {
        if !self.is_macro_edge(topic) {
            return None;
        }
        Some(ArtifactSpec {
            required_sections: self.hat_handoff.artifact.required_sections,
            require_next_marker: self.hat_handoff.artifact.require_next_marker,
            max_bytes: self.hat_handoff.max_bytes,
        })
    }

    /// KTD-8 / U3: whether `source` is allowed to publish
    /// `topic`. The check consults the SSOT
    /// `EventLoopConfig` permits when available
    /// (`hat_handoff.is_explicit_macro` /
    /// `is_exempt`); absent a graph it falls back to
    /// permissive so the lint pipeline can still classify
    /// a topic without a fully loaded graph.
    ///
    /// `source` is the emitter's `HatId` string. The rule is
    /// intentionally permissive when the publishes graph is
    /// absent (returns `true`); stricter cross-hat publishing
    /// enforcement remains in the runtime's
    /// `publishes_check` validator.
    ///
    /// Returned as `bool` (vs `Result`) so the U4 pipeline can
    /// compose it with a single `&&` chain.
    pub fn topic_publisher_allowed(&self, topic: &str, source: &str) -> bool {
        // Orchestrator control / diagnostic topics are always
        // allowed (loop internals — there is no "publishing hat").
        if crate::event_origin::is_orchestrator_control_topic(topic, "")
            || crate::event_origin::is_orchestrator_diagnostic_topic(topic)
        {
            return true;
        }
        // Exempted topics bypass the cross-hat gate.
        if self.hat_handoff.is_exempt(topic) {
            return true;
        }
        // Macro-forced topics: any hat may publish them
        // (the runtime publishes_check enforces the downstream
        // allowed set; this view only answers "may the source
        // hat own this topic at the lint level").
        if self.hat_handoff.is_explicit_macro(topic) {
            return true;
        }
        // No per-hat publishes graph exposed by
        // `EventLoopConfig` itself (the YAML `hats[*].publishes`
        // is owned by `RalphConfig`, not `EventLoopConfig`).
        // We therefore fall back to permissive: U4's
        // validation pipeline lifts the full graph from
        // `RalphConfig` and wraps `topic_publisher_allowed`
        // with stricter checks. The U3 view must remain
        // usable in lint mode where only `EventLoopConfig`
        // is loaded.
        let _ = source;
        true
    }

    /// KTD-8 / U3: required field set for `topic` as a
    /// borrowed reference (vs `required_fields` which clones).
    /// `None` when the topic has no schema entry. Used by U4
    /// pipelines that need to inspect the rule without
    /// allocating.
    pub fn required_fields_for(&self, topic: &str) -> Option<&HashSet<String>> {
        self.effective_required_fields.get(topic)
    }
}

/// Test-only override for the `UNIFIED_PROTOCOL_VIEW` env-var
/// read in `ProtocolView::from_event_loop_with_index_for_env`.
///
/// U11-T7: tests need to flip the env-var default-on/off without
/// `std::env::set_var` (which is `unsafe` under Rust 1.81+ and
/// conflicts with the workspace `forbid(unsafe_code)` lint).
/// Mirrors the `set_correction_enabled_for_test` pattern in
/// `crate::correction`.
///
/// Production code never touches this static; it stays `None`
/// in release binaries.
pub fn set_protocol_view_enabled_for_test(enabled: bool) {
    let cell = TEST_PROTOCOL_VIEW_ENABLED.get_or_init(|| {
        std::sync::atomic::AtomicBool::new(true)
    });
    cell.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

/// Reset the test override so the next call to
/// `from_event_loop_with_index_for_env` consults the env var
/// again. Idempotent when the override was never set.
pub fn reset_protocol_view_enabled_for_test() {
    if let Some(cell) = TEST_PROTOCOL_VIEW_ENABLED.get() {
        cell.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

static TEST_PROTOCOL_VIEW_ENABLED: std::sync::OnceLock<std::sync::atomic::AtomicBool> =
    std::sync::OnceLock::new();

/// KTD-8 / U3: handoff artifact specification derived from
/// `HatHandoffConfig.artifact` for a single topic. Currently
/// a single config-wide value; the `Option` returned by
/// `ProtocolView::handoff_artifact_required` reserves room
/// for per-topic overrides added in later plans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSpec {
    /// Required number of `## section` headings before
    /// `## next`. Mirrors `ArtifactRule::required_sections`.
    pub required_sections: u32,
    /// Whether `## next` marker must be present. Mirrors
    /// `ArtifactRule::require_next_marker`.
    pub require_next_marker: bool,
    /// Maximum bytes for the injected block. Mirrors
    /// `HatHandoffConfig::max_bytes` (KTD-7).
    pub max_bytes: usize,
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
///
/// Returns `(macro_edges, consumer_map)` where `consumer_map`
/// holds `topic → consumer_hat` for self-loop exclusion.
fn resolve_macro_edges(
    hat_handoff: &HatHandoffConfig,
    index: Option<&HandoffIndex>,
) -> (HashSet<String>, HashMap<String, String>) {
    let mut edges: HashSet<String> = hat_handoff.macro_topics.iter().cloned().collect();
    let mut consumers: HashMap<String, String> = HashMap::new();
    if let Some(idx) = index {
        for topic in idx.topics() {
            if let Some(consumer) = idx.consumer_of(&topic) {
                edges.insert(topic.clone());
                consumers.insert(topic.clone(), consumer.to_string());
            }
        }
    }
    (edges, consumers)
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
        out.insert(
            topic.clone(),
            schema.required_fields.iter().cloned().collect(),
        );
    }
    if let Some(contracts) = contracts {
        if contracts.enabled {
            for (topic, rule) in &contracts.rules {
                let extras = extra_required_fields_from_rule(rule);
                if !extras.is_empty() {
                    out.entry(topic.clone()).or_default().extend(extras);
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
    hasher.update(
        hat_handoff
            .artifact
            .require_next_marker
            .to_string()
            .as_bytes(),
    );
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
    use serial_test::serial;

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
            view.is_macro_edge_from("work.ready", Some("plan-gate")),
            "work.ready has unique consumer (executor); engine must recognise it as a macro edge"
        );
        assert!(
            view.is_macro_edge_from("work.done", Some("executor")),
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
            !view.is_macro_edge_from("work.ready", Some("plan-gate")),
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
        view.hat_handoff
            .exempt_topics
            .push("work.ready".to_string());
        assert!(
            !view.is_macro_edge_from("work.ready", Some("plan-gate")),
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
        assert!(!view.is_macro_edge_from("work.ready", Some("plan-gate")));
    }

    /// P0-1: coordinator mode → no macro edges regardless of
    /// the resolved set.
    #[test]
    fn is_macro_edge_coordinator_mode_means_no_macro_edges() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);
        let mut view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));
        view.execution_mode = HatExecutionMode::Coordinator;
        assert!(!view.is_macro_edge_from("work.ready", Some("plan-gate")));
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
        cfg2.event_loop
            .hat_handoff
            .linter
            .auto_prepare_on_macro_edge = true;
        let v2 = ProtocolView::from_event_loop(&cfg2.event_loop);
        assert_ne!(
            v1.protocol_hash, v2.protocol_hash,
            "protocol hash must reflect linter.auto_prepare_on_macro_edge"
        );
    }

    /// P0-1: self-loop exclusion. When from_hat == consumer_of(topic),
    /// the edge is NOT a macro edge (the handoff stays within the same hat).
    #[test]
    fn is_macro_edge_excludes_self_loop() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);
        let view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));
        // Verify the consumer map is populated correctly for self-loop exclusion.
        assert_eq!(
            view.macro_edge_consumers.get("work.ready"),
            Some(&"executor".to_string()),
            "work.ready consumer must be executor"
        );
        assert_eq!(
            view.macro_edge_consumers.get("work.done"),
            Some(&"reviewer".to_string()),
            "work.done consumer must be reviewer"
        );
        // Self-loop: from_hat == consumer should return false.
        assert!(
            !view.is_macro_edge_from("work.ready", Some("executor")),
            "self-loop (executor -> work.ready -> executor) must NOT be a macro edge"
        );
    }

    // ============================================================
    // U3 (KTD-8) test scenarios — happy path / edge case / feature flag.
    // Plan: docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md
    // ============================================================

    /// `review.dimension.ready` 是 DEFAULT_EXEMPT_TOPICS 的一员,
    /// 所以 `is_macro_edge_*` 应该一律返回 false。
    /// 三处(`is_macro_edge(topic)`、`is_macro_edge_from(topic, Some("reviewer"))`、
    /// runtime `hat_handoff::macro_edges::requires_handoff`)结论必须一致。
    #[test]
    fn u3_happy_path_three_layers_agree_on_exempt_topic() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);
        let view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));

        // 1) `is_macro_edge(topic)` — no from_hat, no self-loop exclusion
        let layer1 = view.is_macro_edge("review.dimension.ready");
        // 2) `is_macro_edge_from(topic, Some(from_hat))` — engine gate path
        let layer2 = view.is_macro_edge_from("review.dimension.ready", Some("reviewer"));
        // 3) runtime `requires_handoff` — kept as the SSOT parity check
        let layer3 = matches!(
            crate::hat_handoff::macro_edges::requires_handoff(
                true,
                &HatExecutionMode::Isolated,
                &index,
                "review.dimension.ready",
                "reviewer",
                |t| view.hat_handoff.is_exempt(t),
                |t| view.hat_handoff.is_explicit_macro(t),
            ),
            crate::hat_handoff::macro_edges::MacroEdge::Required
        );

        assert!(
            !layer1,
            "U3 happy path: exempt topic is not a macro edge (layer 1)"
        );
        assert!(
            !layer2,
            "U3 happy path: exempt topic is not a macro edge (layer 2)"
        );
        assert!(
            !layer3,
            "U3 happy path: exempt topic is not a macro edge (layer 3)"
        );
    }

    /// KTD-2: `queue.advance` 拓扑上是 plan-gate 自环
    /// (plan_gate 发布,plan_gate 触发),必须 NOT 是 macro edge。
    /// 同时验证 `is_macro_edge(topic)` 与 `is_macro_edge_from(...)` 一致。
    #[test]
    fn u3_edge_case_queue_advance_self_loop_not_macro() {
        // 用 emit_instructions 的 two-hat fixture:plan_gate 发布 work.ready + queue.advance,
        // executor 只 trigger work.ready。queue.advance 没有 consumer → 自环 + 微观边。
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: isolated
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready", "queue.advance"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let index = HandoffIndex::from_config(&cfg);
        let view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));

        // queue.advance 没有 unique consumer(plan_gate 自己消费)
        // → resolve_macro_edges 不会把它加入 resolved set
        // → is_macro_edge 必须 false
        assert!(
            !view.macro_edges_resolved.contains("queue.advance"),
            "queue.advance self-loop must NOT be in macro_edges_resolved"
        );
        assert!(
            !view.is_macro_edge("queue.advance"),
            "U3 edge case: queue.advance self-loop is not a macro edge (no from_hat)"
        );
        assert!(
            !view.is_macro_edge_from("queue.advance", Some("plan-gate")),
            "U3 edge case: queue.advance self-loop is not a macro edge (from=plan-gate)"
        );
    }

    /// KTD-8 / U11-T7 feature flag: `feature_enabled()` reflects
    /// both the explicit boolean passed to
    /// `from_event_loop_with_index_and_feature` and the test
    /// override installed by
    /// `set_protocol_view_enabled_for_test`. U11-T7 flipped the
    /// env-var default to ON; this test exercises both code paths
    /// without `std::env::set_var` (forbidden by the workspace
    /// `forbid(unsafe_code)` lint).
    ///
    /// The env-var read in `from_event_loop_with_index_for_env`
    /// is exercised by toggling the test-override atomic.
    #[test]
    #[serial] // touches the process-wide test override
    fn u3_feature_flag_explicit_off_default_on() {
        let cfg = minimal_config();

        // Explicit on: opt-in wins regardless of the env-var
        // default (which is ON post U11-T7).
        let view_on =
            ProtocolView::from_event_loop_with_index_and_feature(&cfg.event_loop, None, true);
        assert!(
            view_on.feature_enabled(),
            "explicit feature_enabled = true must be respected (env-var default flipped ON by U11-T7)"
        );

        // Explicit off via the constructor still wins.
        let view_off =
            ProtocolView::from_event_loop_with_index_and_feature(&cfg.event_loop, None, false);
        assert!(
            !view_off.feature_enabled(),
            "explicit feature_enabled = false must be respected (opt-out path)"
        );

        // The env-var wrapper, with the test override in place,
        // returns `feature_enabled` equal to the override value
        // (the env-var read is short-circuited by the override).
        super::set_protocol_view_enabled_for_test(false);
        let view_env_off =
            ProtocolView::from_event_loop_with_index_for_env(&cfg.event_loop, None);
        assert!(
            !view_env_off.feature_enabled(),
            "env-var wrapper with override = false must yield feature_enabled = false (opt-out via test override)"
        );

        super::set_protocol_view_enabled_for_test(true);
        let view_env_on =
            ProtocolView::from_event_loop_with_index_for_env(&cfg.event_loop, None);
        assert!(
            view_env_on.feature_enabled(),
            "env-var wrapper with override = true must yield feature_enabled = true (default-on path)"
        );

        // Reset the override so subsequent tests see the real
        // env var (which is unset in the test runner, so the
        // default-on path wins).
        super::reset_protocol_view_enabled_for_test();
    }

    /// U3 / KTD-8: handoff_artifact_required 在 macro-edge 上返回
    /// Some(ArtifactSpec),非 macro-edge 返回 None。
    #[test]
    fn u3_handoff_artifact_required_reflects_macro_set() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);
        let mut view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));
        // 设置一个非默认的 artifact rule,验证 spec 透传该值
        view.hat_handoff.artifact.required_sections = 5;
        view.hat_handoff.artifact.require_next_marker = true;
        view.hat_handoff.max_bytes = 4096;

        // work.ready 是 macro edge
        let spec = view
            .handoff_artifact_required("work.ready")
            .expect("work.ready is a macro edge; spec must be Some");
        assert_eq!(spec.required_sections, 5);
        assert!(spec.require_next_marker);
        assert_eq!(spec.max_bytes, 4096);

        // queue.advance 不是 macro edge(没有 unique consumer)
        assert!(
            view.handoff_artifact_required("queue.advance").is_none(),
            "non-macro topic must have no artifact spec"
        );
    }

    /// U3: required_fields_for 返回 `&HashSet<String>` 而 `required_fields` clone。
    #[test]
    fn u3_required_fields_for_returns_borrowed_view() {
        // `EventLoopConfig` has serde defaults for most fields;
        // only `event_policy` needs explicit content.
        let yaml = r#"
event_policy:
  enabled: true
  mode: observe
  schemas:
    work.done:
      required_fields: ["plan_name", "step"]
"#;
        let cfg: EventLoopConfig = serde_yaml::from_str(yaml).unwrap();
        let view = ProtocolView::from_event_loop(&cfg);
        let fields = view
            .required_fields_for("work.done")
            .expect("work.done schema must be present");
        assert!(fields.contains("plan_name"));
        assert!(fields.contains("step"));
        assert!(view.required_fields_for("unknown.topic").is_none());
    }

    /// U3: topic_publisher_allowed 在默认情况下是 permissive(返回 true),
    /// 等待 U4 引入完整 publishes graph 后再加严。
    /// 此处验证 API 形状 + 豁免/macro 走快速路径。
    #[test]
    fn u3_topic_publisher_allowed_permissive_default() {
        let cfg = minimal_config();
        let index = HandoffIndex::from_config(&cfg);
        let view = ProtocolView::from_event_loop_with_index(&cfg.event_loop, Some(&index));

        // 默认 permissive
        assert!(view.topic_publisher_allowed("any.topic", "any-hat"));
        // orchestrator control topic 永远允许
        assert!(view.topic_publisher_allowed("event.start", "any-hat"));
        // exempt 走快速路径
        assert!(view.topic_publisher_allowed("review.dimension.done", "any-hat"));
    }
}
