//! U3 (plan 2026-08-16-1015): terminal target routing static lint.
//!
//! Validates `EventSchema.required_target_hat` contract declarations
//! at preset-load time:
//!
//! - [`FINDING_TERMINAL_TARGET_CONTRACT_EMPTY_STRING`] — `required_target_hat = ""`
//!   defensive fallback is a no-op that bypasses the guard; Warn.
//! - [`FINDING_TERMINAL_TARGET_NOT_REGISTERED`] — topic declares
//!   `required_target_hat` but no hat subscribes; the contract can never
//!   be enforced; Error.
//! - [`FINDING_TERMINAL_TARGET_CONSUMER_MISMATCH`] — declared hat
//!   doesn't match the unique consumer registered in the handoff index;
//!   the contract would silently pass for the wrong target; Error.

use crate::config::RalphConfig;
use crate::preset_lint::finding_id::{
    FINDING_TERMINAL_TARGET_CONSUMER_MISMATCH, FINDING_TERMINAL_TARGET_CONTRACT_EMPTY_STRING,
    FINDING_TERMINAL_TARGET_NOT_REGISTERED,
};
use crate::preset_lint::{LintFinding, LintSeverity};
use crate::workflow_contract::HandoffIndex;
use crate::preset_lint::workflow_activation::HandoffGraph;

/// Check all `event_policy.schemas` entries that declare a
/// `required_target_hat` for the three routing contract violations.
/// Returns an empty list when no schemas declare `required_target_hat`
/// (the check is a no-op for presets that do not use the feature).
pub fn check_target_routing(config: &RalphConfig) -> Vec<LintFinding> {
    let Some(policy) = config.event_loop.event_policy.as_ref() else {
        return Vec::new();
    };

    let index = HandoffIndex::from_config(config);
    let graph = HandoffGraph::from_config(config);
    let mut findings = Vec::new();

    for (topic, schema) in &policy.schemas {
        let Some(ref required) = schema.required_target_hat else {
            continue;
        };

        // Rule 1: empty-string is a no-op defensive fallback.
        if required.is_empty() {
            findings.push(LintFinding {
                id: FINDING_TERMINAL_TARGET_CONTRACT_EMPTY_STRING,
                severity: LintSeverity::Warn,
                message: format!(
                    "event_policy.schemas[\"{topic}\"] declares required_target_hat = \"\"; \
                     an empty string is a no-op that bypasses the terminal target guard entirely"
                ),
                topic: Some(topic.clone()),
                hat: None,
                owner: None,
                action_hint: Some(
                    "set required_target_hat to the exact hat that must receive this topic, \
                     or remove the field entirely if the topic does not require a mandatory target"
                        .to_string(),
                ),
            });
            continue;
        }

        // Look up the unique consumer from the handoff index.
        // Only topics in the handoff index (seeded or derived from unique triggers)
        // can be validated for routing. Topics not in the index are either
        // control topics, diagnostic topics, or multi-consumer topics — none
        // of which have a single routing target to validate against.
        let Some(consumer) = index.consumer_of(topic) else {
            // Topic not in the index: check if it has zero subscribers
            // (not registered anywhere) or is a multi-consumer topic.
            // Zero subscribers → fire NOT_REGISTERED.
            // Multi-consumer (len > 1) → skip (no unique target to validate).
            let subscriber_count = graph.topic_subscribers.get(topic).map(|h| h.len()).unwrap_or(0);
            if subscriber_count == 0 {
                findings.push(LintFinding {
                    id: FINDING_TERMINAL_TARGET_NOT_REGISTERED,
                    severity: LintSeverity::Error,
                    message: format!(
                        "event_policy.schemas[\"{topic}\"] declares required_target_hat = \"{required}\" \
                         but no hat subscribes to this topic; the contract can never be enforced"
                    ),
                    topic: Some(topic.clone()),
                    hat: None,
                    owner: None,
                    action_hint: Some(
                        "ensure the topic is published by at least one hat and that a hat \
                         triggers it, or remove required_target_hat if no mandatory target is needed"
                            .to_string(),
                    ),
                });
            }
            continue;
        };

        // Rule 3: consumer mismatch.
        if consumer != required {
            findings.push(LintFinding {
                id: FINDING_TERMINAL_TARGET_CONSUMER_MISMATCH,
                severity: LintSeverity::Error,
                message: format!(
                    "event_policy.schemas[\"{topic}\"] declares required_target_hat = \"{required}\" \
                     but the registered unique consumer is \"{consumer}\"; the contract would \
                     silently pass for the wrong target hat"
                ),
                topic: Some(topic.clone()),
                hat: None,
                owner: None,
                action_hint: Some(format!(
                    "update required_target_hat to \"{consumer}\" to match the registered consumer, \
                     or remove required_target_hat if no mandatory target is needed"
                )),
            });
        }
    }

    findings
}
