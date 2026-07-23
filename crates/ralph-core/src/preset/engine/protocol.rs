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
//! ## Protocol hash (P2-4 fix)
//!
//! `protocol_hash` is SHA-256 of the canonicalised view. The
//! previous `DefaultHasher` was Rust-version-dependent; the
//! new hash is stable across `cargo update` / `cargo build`
//! cycles on the same source.
//!
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::config::execution_contracts::{ExecutionContractRule, ExecutionContractsConfig};
use crate::config::{
    EventLoopConfig, EventPolicyConfig, EventSchema, HandoffEnvelopeConfig, StateProjectionConfig,
    VerdictGateConfig, WorkflowChain, WorkflowContractConfig, WorkflowGuardsConfig,
};
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

    /// Workflow guards (ordered event chains).
    pub workflow_guards: Option<WorkflowGuardsConfig>,

    /// State projection chain.
    pub state_projection: Option<StateProjectionConfig>,

    /// Execution contracts (require_git_change / require_task /
    /// dedup_key / etc). Empty when `enabled = false`.
    pub execution_contracts: Option<ExecutionContractsConfig>,

    /// Event policy config (schemas, topic deny rules, terminal /
    /// business topics, completion-after-terminal actions). `None`
    /// when the preset does not declare an `event_policy` block.
    pub event_policy: Option<EventPolicyConfig>,

    /// 2026-07-06-004 fix-plan U1: typed handoff envelope config.
    /// `EventPolicyRule` reads this field to build the
    /// `EventLoopHandoffConfig<'_>` adapter that
    /// `validate_event_with_options` requires for the nested
    /// `check_handoff_envelope` gate. Defaults to a disabled
    /// config so non-serial presets and ad-hoc emits see the
    /// same pre-fix behaviour.
    pub handoff_envelope: HandoffEnvelopeConfig,

    /// 2026-07-06-004 fix-plan U6 (R6): topology topics — the
    /// union of every hat's `triggers` ∪ `publishes` in the
    /// loaded preset. `EventPolicyRule` reads this when
    /// running the topology-aware
    /// `validate_handoff_envelope_payload_with_topology` pass
    /// so a `success_signal` / `failure_signal` outside the
    /// declared topology is rejected as
    /// `handoff_envelope_signal_outside_topology`. Empty when
    /// the preset has no `hats:` block (CLI dry-runs / plain
    /// loops); the topology check is then skipped for
    /// parity.
    pub topology_topics: HashSet<String>,

    /// Protocol hash — stable across `cargo build` cycles AND
    /// Rust versions (SHA-256; P2-4 fix). Used by
    /// `ralph emit --schema` to detect drift between the
    /// authoring SSOT and the embedded copy.
    pub protocol_hash: String,

    /// 2026-07-23-002 plan U7: the loop's `completion_promise`
    /// topic (e.g. `LOOP_COMPLETE`). Populated from
    /// `EventLoopConfig::completion_promise` at view-construction
    /// time so `EventPolicyRule` can exempt it from the
    /// duplicate-terminal check. Without this, a pre-terminal
    /// topic like `plan.complete` sets `terminal_observed`, and
    /// the subsequent `LOOP_COMPLETE` (also in `terminal_topics`)
    /// is hard-rejected as a duplicate terminal — blocking loop
    /// completion. The completion_promise is the authoritative
    /// end signal and must always pass the duplicate-terminal
    /// gate (it is validated separately by
    /// `check_completion_event`).
    pub completion_promise: String,

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
    /// Build a view from a loaded `EventLoopConfig`. Test /
    /// `--schema` callers go through this; production callers
    /// that need feature-flag control use
    /// [`Self::from_event_loop_with_feature`].
    ///
    /// **P2-#6 (002-adversarial-review)**: this entry point no
    /// longer reads the `UNIFIED_PROTOCOL_VIEW` env var. Tests
    /// using this helper stay env-independent; production
    /// callers that need the env-gated behaviour must use
    /// [`Self::from_event_loop_with_feature_for_env`] so the
    /// env read happens at one well-known call site.
    pub fn from_event_loop(config: &EventLoopConfig) -> Self {
        Self::from_event_loop_with_feature(config, false)
    }

    /// Build a view with the unified protocol view always enabled.
    /// The env var read has been removed; the test override remains
    /// so suites can still exercise the legacy path when needed.
    pub fn from_event_loop_with_feature_for_env(config: &EventLoopConfig) -> Self {
        let feature_flag_enabled = TEST_PROTOCOL_VIEW_ENABLED
            .get()
            .map(|cell| cell.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(true);
        Self::from_event_loop_with_feature(config, feature_flag_enabled)
    }

    /// 2026-07-06-004 fix-plan U6 (R6): production entry
    /// point that threads the loaded `RalphConfig`'s `hats`
    /// map into the topology computation. Same env semantics
    /// as `from_event_loop_with_feature_for_env`.
    pub fn from_event_loop_with_feature_for_env_and_hats(
        config: &EventLoopConfig,
        hats: &HashMap<String, crate::config::HatConfig>,
    ) -> Self {
        let feature_flag_enabled = TEST_PROTOCOL_VIEW_ENABLED
            .get()
            .map(|cell| cell.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(true);
        Self::from_event_loop_with_feature_hats(config, hats, feature_flag_enabled)
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
    pub fn from_event_loop_with_feature(config: &EventLoopConfig, feature_enabled: bool) -> Self {
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
        let workflow_guards = config.workflow_guards.clone();
        let state_projection = Some(config.state_projection.clone());
        let execution_contracts = config.execution_contracts.clone();

        // 2026-07-06-004 fix-plan U1: relay the typed handoff
        // envelope config so `EventPolicyRule::validate` can
        // build the `EventLoopHandoffConfig<'_>` adapter without
        // threading the `EventLoopConfig` through every rule.
        let handoff_envelope = config.handoff_envelope.clone();

        // 2026-07-06-004 fix-plan U6 (R6): compute the topology
        // topic set (every hat's `triggers` ∪ `publishes`).
        // `EventLoopConfig` does not carry the `hats` map (that
        // lives on `RalphConfig`), so the view populates an
        // empty set here and `from_event_loop_with_feature_hats`
        // is the production entry point that threads the hats
        // map in.
        let topology_topics: HashSet<String> = HashSet::new();

        // P2-4: SHA-256 (stable across Rust versions). The previous
        // `DefaultHasher` was Rust-version-dependent and produced
        // false-positive drift warnings after `cargo update`.
        let protocol_hash =
            compute_protocol_hash(&effective_required_fields, workflow_guards.as_ref());

        Self {
            effective_required_fields,
            verdict_gate,
            workflow_contract,
            workflow_guards,
            state_projection,
            execution_contracts,
            event_policy: config.event_policy.clone(),
            handoff_envelope,
            topology_topics,
            protocol_hash,
            completion_promise: config.completion_promise.clone(),
            feature_flag_enabled: feature_enabled,
        }
    }

    /// 2026-07-06-004 fix-plan U6 (R6): production entry
    /// point that threads the loaded `RalphConfig`'s `hats`
    /// map into the topology computation. The view is then
    /// able to enforce `success_signal` /
    /// `failure_signal` against the union of every hat's
    /// `triggers` ∪ `publishes`.
    pub fn from_event_loop_with_feature_hats(
        config: &EventLoopConfig,
        hats: &HashMap<String, crate::config::HatConfig>,
        feature_enabled: bool,
    ) -> Self {
        let mut view = Self::from_event_loop_with_feature(config, feature_enabled);
        let mut topology: HashSet<String> = HashSet::new();
        for hat in hats.values() {
            for t in &hat.triggers {
                topology.insert(t.clone());
            }
            for p in &hat.publishes {
                topology.insert(p.clone());
            }
        }
        view.topology_topics = topology;
        view
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
    /// resolution path. The flag is captured at construction
    /// so the runtime can consult it without re-reading the env.
    pub fn feature_enabled(&self) -> bool {
        self.feature_flag_enabled
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
    let cell = TEST_PROTOCOL_VIEW_ENABLED.get_or_init(|| std::sync::atomic::AtomicBool::new(true));
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
    if let Some(contracts) = contracts
        && contracts.enabled
    {
        for (topic, rule) in &contracts.rules {
            let extras = extra_required_fields_from_rule(rule);
            if !extras.is_empty() {
                out.entry(topic.clone()).or_default().extend(extras);
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
    workflow_guards: Option<&WorkflowGuardsConfig>,
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
    hasher.update(b"|workflow_guards|");
    if let Some(guards) = workflow_guards {
        let mut chains: Vec<&WorkflowChain> = guards.chains.iter().collect();
        chains.sort_by(|a, b| a.name.cmp(&b.name));
        for chain in chains {
            hasher.update(chain.name.as_bytes());
            hasher.update([0u8]);
            for topic in &chain.topics {
                hasher.update(topic.as_bytes());
                hasher.update([0u8]);
            }
            hasher.update([1u8]); // chain separator
        }
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
    //! Tests for the stable protocol hash (P2-4) and the KTD-8 surface.
    use super::*;
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
"#;
        serde_yaml::from_str(yaml).expect("config parses")
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
        use crate::config::workflow_guards::{
            WorkflowChain, WorkflowChainMode, WorkflowGuardsConfig,
        };
        let cfg = minimal_config();
        let v1 = ProtocolView::from_event_loop(&cfg.event_loop);
        let mut cfg2 = cfg.clone();
        cfg2.event_loop.workflow_guards = Some(WorkflowGuardsConfig {
            chains: vec![WorkflowChain {
                name: "main".to_string(),
                topics: vec!["work.start".to_string(), "work.ready".to_string()],
                mode: WorkflowChainMode::Strict,
                correlation: None,
            }],
        });
        let v2 = ProtocolView::from_event_loop(&cfg2.event_loop);
        assert_ne!(
            v1.protocol_hash, v2.protocol_hash,
            "protocol hash must reflect workflow_guards changes"
        );
    }

    /// KTD-8 / U11-T7 feature flag: `feature_enabled()` reflects
    /// both the explicit boolean passed to
    /// `from_event_loop_with_feature` and the test override
    /// installed by `set_protocol_view_enabled_for_test`.
    #[test]
    #[serial] // touches the process-wide test override
    fn u3_feature_flag_explicit_off_default_on() {
        let cfg = minimal_config();

        // Explicit on: opt-in wins regardless of the env-var
        // default (which is ON post U11-T7).
        let view_on = ProtocolView::from_event_loop_with_feature(&cfg.event_loop, true);
        assert!(
            view_on.feature_enabled(),
            "explicit feature_enabled = true must be respected (env-var default flipped ON by U11-T7)"
        );

        // Explicit off via the constructor still wins.
        let view_off = ProtocolView::from_event_loop_with_feature(&cfg.event_loop, false);
        assert!(
            !view_off.feature_enabled(),
            "explicit feature_enabled = false must be respected (opt-out path)"
        );

        // The env-var wrapper, with the test override in place,
        // returns `feature_enabled` equal to the override value
        // (the env-var read is short-circuited by the override).
        super::set_protocol_view_enabled_for_test(false);
        let view_env_off = ProtocolView::from_event_loop_with_feature_for_env(&cfg.event_loop);
        assert!(
            !view_env_off.feature_enabled(),
            "env-var wrapper with override = false must yield feature_enabled = false (opt-out via test override)"
        );

        super::set_protocol_view_enabled_for_test(true);
        let view_env_on = ProtocolView::from_event_loop_with_feature_for_env(&cfg.event_loop);
        assert!(
            view_env_on.feature_enabled(),
            "env-var wrapper with override = true must yield feature_enabled = true (default-on path)"
        );

        // Reset the override so subsequent tests see the real
        // env var (which is unset in the test runner, so the
        // default-on path wins).
        super::reset_protocol_view_enabled_for_test();
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
}
