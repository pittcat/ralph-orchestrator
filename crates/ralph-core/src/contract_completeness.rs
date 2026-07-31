//! U12 (plan 2026-07-30-004): contract completeness lint + machine-readable
//! contract inspect.
//!
//! Two capabilities live here:
//!
//! 1. [`check_contract_completeness`] — a static lint that verifies every
//!    *emitting* hat in a preset is covered by at least one explicit
//!    execution-contract rule. A hat that can emit one or more topics but whose
//!    emitted topics carry **no** contract rule is a *passthrough* activation:
//!    its output flows downstream without any backpressure gate (Tenet #2).
//!    This is the lint that guards the Parallel Forge preset against silent
//!    regressions to a passthrough adapter.
//!
//! 2. [`inspect_contract_json`] — a machine-readable JSON view of the compiled
//!    [`EffectiveExecutionContract`] scoped to a single hat, surfacing the
//!    deterministic `contract_digest` plus the hat's resolved emit
//!    allow/deny sets. Backs `ralph inspect contract`.
//!
//! The completeness lint is intentionally *vacuous* when execution contracts
//! are not enabled (mirroring [`crate::execution_contract::compile`], which
//! skips consumer-completeness when `enabled == false`): when nothing is
//! enforced, nothing is "passthrough" relative to an enforced contract, so
//! flagging every hat would be noise.

use crate::config::{HatConfig, RalphConfig};
use crate::execution_contract::ResolvedRuntimeConfig;

/// A completeness finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletenessFinding {
    /// A hat emits at least one topic but no execution-contract rule covers any
    /// of its emitted topics — its output passes through unvalidated.
    PassthroughHat {
        /// The uncovered hat's id.
        hat_id: String,
    },
}

impl CompletenessFinding {
    /// A human-readable, actionable message for the finding.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            CompletenessFinding::PassthroughHat { hat_id } => format!(
                "hat '{hat_id}' emits at least one topic but no execution-contract rule \
                 covers any of its emitted topics; its output passes through unvalidated. \
                 Add an `execution_contracts.rules` entry for a topic this hat publishes \
                 (or a terminal event it owns)."
            ),
        }
    }
}

/// Collect a hat's emit-side topics: `publishes` ∪ `terminal_events` ∪
/// `default_publishes`. Deduplicated and sorted for stable, testable output.
fn emit_side_topics(hat: &HatConfig) -> Vec<String> {
    let mut topics: Vec<String> = Vec::new();
    topics.extend(hat.publishes.iter().cloned());
    topics.extend(hat.terminal_events.iter().cloned());
    if let Some(default) = &hat.default_publishes {
        topics.push(default.clone());
    }
    topics.sort();
    topics.dedup();
    topics
}

/// Check that every emitting hat in the config is covered by at least one
/// explicit execution-contract rule.
///
/// Returns an empty `Vec` when:
/// - `execution_contracts` is absent or `enabled == false` (vacuous — nothing
///   enforced), or
/// - every emitting hat has at least one emitted topic carrying a contract
///   rule.
///
/// A hat with no emit-side topics (a pure consumer) is never flagged: it has
/// nothing that could pass through unvalidated.
#[must_use]
pub fn check_contract_completeness(config: &RalphConfig) -> Vec<CompletenessFinding> {
    let mut findings = Vec::new();

    // Vacuous when contracts are not enabled.
    let Some(contracts) = &config.event_loop.execution_contracts else {
        return findings;
    };
    if !contracts.enabled {
        return findings;
    }

    // Deterministic ordering: HashMap iteration order is unspecified.
    let mut hat_ids: Vec<&String> = config.hats.keys().collect();
    hat_ids.sort();
    for hat_id in hat_ids {
        let hat = &config.hats[hat_id];
        let emit_topics = emit_side_topics(hat);
        if emit_topics.is_empty() {
            // Pure consumer hat — nothing to gate.
            continue;
        }
        let covered = emit_topics
            .iter()
            .any(|topic| contracts.rules.contains_key(topic));
        if !covered {
            findings.push(CompletenessFinding::PassthroughHat {
                hat_id: (*hat_id).clone(),
            });
        }
    }
    findings
}

/// Produce a machine-readable JSON view of the compiled contract for a hat.
///
/// Surfaces the deterministic `contract_digest` (shared identity across the
/// resident loop and the independent CLI) plus the hat's resolved emit
/// allow/deny topic sets after deny-wins resolution. The `hat_id` is echoed so
/// the output is self-describing.
#[must_use]
pub fn inspect_contract_json(resolved: &ResolvedRuntimeConfig, hat_id: &str) -> serde_json::Value {
    let contract = resolved.contract();
    let allows: Vec<&str> = contract
        .emit_allows
        .iter()
        .filter(|pair| pair.0 == hat_id)
        .map(|pair| pair.1.as_str())
        .collect();
    let denies: Vec<&str> = contract
        .emit_denies
        .iter()
        .filter(|pair| pair.0 == hat_id)
        .map(|pair| pair.1.as_str())
        .collect();
    serde_json::json!({
        "hat_id": hat_id,
        "contract_digest": contract.contract_digest,
        "emit_allows": allows,
        "emit_denies": denies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::execution_contracts::ExecutionContractsConfig;
    use crate::config::ExecutionContractRule;
    use crate::execution_contract::compile;

    /// A two-hat config: `worker` emits `work.done`, `coordinator` consumes it
    /// and emits the terminal `LOOP_COMPLETE`.
    fn base_yaml() -> &'static str {
        r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
cli:
  backend: "claude"
hats:
  worker:
    name: "Worker"
    description: "Does the work"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]
    instructions: "do work"
  coordinator:
    name: "Coordinator"
    description: "Consumes completion"
    triggers: ["work.done"]
    publishes: ["LOOP_COMPLETE"]
    terminal_events: ["LOOP_COMPLETE"]
    instructions: "coordinate"
"#
    }

    /// Enable contracts and cover `topic` with a (minimal) rule.
    fn enable_covering(config: &mut RalphConfig, topics: &[&str]) {
        let mut contracts = ExecutionContractsConfig::default();
        contracts.enabled = true;
        for topic in topics {
            contracts
                .rules
                .insert((*topic).to_string(), ExecutionContractRule::default());
        }
        config.event_loop.execution_contracts = Some(contracts);
    }

    #[test]
    fn u12_completeness_lint_passes_for_explicit_contract() {
        // Every emitting hat has at least one emitted topic carrying a rule.
        let mut config = RalphConfig::parse_yaml(base_yaml()).expect("base parses");
        enable_covering(&mut config, &["work.done", "LOOP_COMPLETE"]);

        let findings = check_contract_completeness(&config);
        assert!(
            findings.is_empty(),
            "fully-covered preset must have no findings, got: {findings:?}"
        );
    }

    #[test]
    fn u12_completeness_lint_flags_passthrough_hat() {
        // Cover `worker`'s `work.done` but leave `coordinator`'s `LOOP_COMPLETE`
        // uncovered — the coordinator is a passthrough activation.
        let mut config = RalphConfig::parse_yaml(base_yaml()).expect("base parses");
        enable_covering(&mut config, &["work.done"]);

        let findings = check_contract_completeness(&config);
        assert_eq!(
            findings,
            vec![CompletenessFinding::PassthroughHat {
                hat_id: "coordinator".to_string(),
            }],
            "only the uncovered coordinator hat must be flagged"
        );
        // The message must be actionable and name the hat.
        assert!(findings[0].message().contains("coordinator"));
    }

    #[test]
    fn u12_completeness_lint_vacuous_when_contracts_disabled() {
        // enabled == false: nothing enforced, no findings even though the hats
        // would otherwise be passthrough.
        let mut config = RalphConfig::parse_yaml(base_yaml()).expect("base parses");
        let mut contracts = ExecutionContractsConfig::default();
        contracts.enabled = false;
        config.event_loop.execution_contracts = Some(contracts);

        assert!(check_contract_completeness(&config).is_empty());
    }

    #[test]
    fn u12_inspect_contract_json_surfaces_digest() {
        // Contracts disabled so compile() succeeds without needing consumers.
        let config = RalphConfig::parse_yaml(base_yaml()).expect("base parses");
        let resolved = compile(config).expect("base config compiles");

        let json = inspect_contract_json(&resolved, "worker");
        assert_eq!(json["hat_id"], "worker");
        // The deterministic digest must be surfaced and non-empty.
        assert!(
            json["contract_digest"].is_string(),
            "contract_digest must be a string: {json}"
        );
        assert!(
            !json["contract_digest"].as_str().unwrap().is_empty(),
            "contract_digest must be non-empty"
        );
        // The emit allow/deny arrays must be present. `worker` publishes
        // `work.done`, so it must appear in emit_allows.
        assert!(json["emit_allows"].is_array(), "emit_allows must be an array");
        assert!(json["emit_denies"].is_array(), "emit_denies must be an array");
        let allows: Vec<&str> = json["emit_allows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            allows.contains(&"work.done"),
            "worker's publish topic must be in emit_allows: {allows:?}"
        );
    }
}
