//! U4b: `WorkflowGuardRule` — wraps the workflow-guard stage.
//!
//! Post-commit phase. The legacy implementation still lives in
//! `event_loop::apply_workflow_guard_validation` (a free function
//! that mutates `WorkflowProgress`, the `EventBus`, and a
//! `ReviewStepTracker`) because the event loop does not yet run
//! post-commit rules. U4b exposes the **decision** surface behind a
//! pure `ValidationRule` so the unified pipeline can compose it once
//! post-commit execution is wired.
//!
//! The rule mirrors the strict-chain check: an event topic must
//! appear in a configured chain's topic list and the chain's progress
//! (read from the snapshot's `workflow_phases` map) must mark the
//! event's phase as valid. The rule does **not** mutate the
//! `EventBus`, call `RecoveryResponder`, or advance workflow progress
//! — those side effects belong to the orchestrator layer (U6 will
//! wire them). The rule produces the
//! `ReasonCode::WORKFLOW_GUARD_OUT_OF_ORDER` reason code on
//! rejection, matching the legacy diagnostic.

use crate::config::WorkflowChain;
use crate::config::workflow_guards::{CorrelationConfig, WorkflowChainMode};
use crate::event_reader::Event;
use crate::preset::engine::protocol::ProtocolView;

use super::context::ValidationContext;
use super::pipeline::{RulePhase, ValidationRule};
use super::result::{
    ReasonCode, RejectionHint, ValidationResult, ValidationStage, WorkflowGuardRejectionDetail,
};

/// `WorkflowGuardRule` — post-commit workflow-guard check.
pub struct WorkflowGuardRule;

/// Result of extracting a correlation key from an event payload.
#[derive(Debug, Clone)]
enum CorrelationKeyResult {
    /// Chain has no correlation config — use global instance tracking.
    Global,
    /// Successfully extracted instance key from payload.
    Instance(String),
    /// Correlation config exists but extraction failed.
    ExtractFailed,
}

impl ValidationRule for WorkflowGuardRule {
    fn name(&self) -> &'static str {
        ValidationStage::WorkflowGuard.as_str()
    }

    fn applies_to(&self) -> RulePhase {
        RulePhase::PostCommit
    }

    fn validate(
        &self,
        protocol_view: &ProtocolView,
        ctx: &mut ValidationContext<'_>,
        event: &Event,
    ) -> ValidationResult {
        let guards = match protocol_view.workflow_guards.as_ref() {
            Some(g) if !g.chains.is_empty() => g,
            _ => return ValidationResult::accept_with(ValidationStage::WorkflowGuard),
        };

        let snapshot = ctx.snapshot();

        // Collect every chain whose topic list contains this event.
        let matching_chains: Vec<&WorkflowChain> = guards
            .chains
            .iter()
            .filter(|chain| chain.topics.contains(&event.topic))
            .collect();

        if matching_chains.is_empty() {
            // Side-channel event: not part of any guarded chain.
            return ValidationResult::accept_with(ValidationStage::WorkflowGuard);
        }

        let mut rejections: Vec<WorkflowGuardRejectionDetail> = Vec::new();

        for chain in matching_chains {
            let instance_key = match extract_correlation_key(event, chain) {
                CorrelationKeyResult::Global => None,
                CorrelationKeyResult::Instance(key) => Some(key),
                CorrelationKeyResult::ExtractFailed => {
                    rejections.push(WorkflowGuardRejectionDetail {
                        chain_name: chain.name.clone(),
                        instance_key: None,
                        current_phase: None,
                        current_topic: "none".to_string(),
                        next_expected: "unknown (correlation extraction failed)".to_string(),
                    });
                    continue;
                }
            };

            let phase = chain
                .topics
                .iter()
                .position(|t| *t == event.topic)
                .expect("topic is in chain.topics by filter above");

            // Advisory chains never reject; they only track progress in
            // the orchestrator layer.
            if !matches!(chain.mode, WorkflowChainMode::Strict) {
                continue;
            }

            let key = workflow_phase_key(&chain.name, instance_key.as_deref());
            let current_phase = snapshot
                .workflow_phases
                .get(&key)
                .map(|p| *p as usize);

            let valid = match current_phase {
                None => phase == 0,
                Some(highest) => phase == highest || phase == highest + 1,
            };

            if !valid {
                let current_topic = current_phase
                    .and_then(|p| chain.topics.get(p).cloned())
                    .unwrap_or_else(|| "none".to_string());
                let next_expected = current_phase
                    .and_then(|p| chain.topics.get(p + 1).cloned())
                    .unwrap_or_else(|| "terminal".to_string());

                rejections.push(WorkflowGuardRejectionDetail {
                    chain_name: chain.name.clone(),
                    instance_key: instance_key.clone(),
                    current_phase,
                    current_topic,
                    next_expected,
                });
            }
        }

        if rejections.is_empty() {
            return ValidationResult::accept_with(ValidationStage::WorkflowGuard);
        }

        let first = &rejections[0];
        ValidationResult::reject(
            ValidationStage::WorkflowGuard,
            ReasonCode::WORKFLOW_GUARD_OUT_OF_ORDER,
            Some(RejectionHint::workflow_guard_out_of_order(
                &event.topic,
                &rejections,
            )),
            true,
        )
    }
}

/// Build the `workflow_phases` map key used by [`LedgerSnapshot`].
fn workflow_phase_key(chain_name: &str, instance_key: Option<&str>) -> String {
    match instance_key {
        Some(k) => format!("{chain_name}::{k}"),
        None => format!("{chain_name}::"),
    }
}

/// Extract the correlation key from an event payload based on chain config.
fn extract_correlation_key(event: &Event, chain: &WorkflowChain) -> CorrelationKeyResult {
    let Some(correlation) = chain.correlation.as_ref() else {
        return CorrelationKeyResult::Global;
    };

    let Some(payload) = event.payload.as_ref() else {
        return CorrelationKeyResult::ExtractFailed;
    };

    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return CorrelationKeyResult::ExtractFailed;
    };

    let parts: Vec<&str> = correlation.from_payload.split('.').collect();
    let mut current = &value;
    for part in parts {
        let Some(next) = current.get(part) else {
            return CorrelationKeyResult::ExtractFailed;
        };
        current = next;
    }

    match current.as_str() {
        Some(s) => CorrelationKeyResult::Instance(s.to_string()),
        None => CorrelationKeyResult::ExtractFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CorrelationConfig, WorkflowChain, WorkflowChainMode, WorkflowGuardsConfig};
    use crate::event_reader::Event;
    use crate::state::LedgerSnapshot;
    use crate::validation::context::ValidationContext;

    fn event(topic: &str, payload: Option<&str>) -> Event {
        Event {
            topic: topic.to_string(),
            payload: payload.map(|s| s.to_string()),
            ts: "2026-06-22T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        }
    }

    fn strict_chain() -> WorkflowChain {
        WorkflowChain {
            name: "experiment".to_string(),
            topics: vec![
                "experiment.planned".to_string(),
                "experiment.run".to_string(),
                "experiment.evaluated".to_string(),
            ],
            mode: WorkflowChainMode::Strict,
            correlation: None,
        }
    }

    fn advisory_chain() -> WorkflowChain {
        WorkflowChain {
            name: "build".to_string(),
            topics: vec![
                "build.started".to_string(),
                "build.finished".to_string(),
            ],
            mode: WorkflowChainMode::Advisory,
            correlation: None,
        }
    }

    fn view_with_chains(chains: Vec<WorkflowChain>) -> ProtocolView {
        let mut config = crate::config::EventLoopConfig::default();
        config.workflow_guards = Some(WorkflowGuardsConfig { chains });
        ProtocolView::from_event_loop(&config)
    }

    #[test]
    fn accepts_side_channel_event() {
        let view = view_with_chains(vec![strict_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        let mut ctx = ValidationContext::new(&mut snap);
        let result = WorkflowGuardRule.validate(&view, &mut ctx, &event("periodic.review", None));
        assert!(result.accepted);
        assert_eq!(result.stage, ValidationStage::WorkflowGuard);
    }

    #[test]
    fn accepts_chain_start() {
        let view = view_with_chains(vec![strict_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        let mut ctx = ValidationContext::new(&mut snap);
        let result =
            WorkflowGuardRule.validate(&view, &mut ctx, &event("experiment.planned", None));
        assert!(result.accepted);
    }

    #[test]
    fn rejects_out_of_order_strict_chain() {
        let view = view_with_chains(vec![strict_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        snap.workflow_phases
            .insert("experiment::".to_string(), 0);
        let mut ctx = ValidationContext::new(&mut snap);
        let result =
            WorkflowGuardRule.validate(&view, &mut ctx, &event("experiment.evaluated", None));
        assert!(!result.accepted);
        assert_eq!(
            result.reason_code.as_deref(),
            Some(ReasonCode::WORKFLOW_GUARD_OUT_OF_ORDER)
        );
        assert!(result.retry_eligible);
        assert!(result
            .correction_hint
            .as_deref()
            .unwrap_or("")
            .contains("experiment.run"));
    }

    #[test]
    fn accepts_advisory_out_of_order() {
        let view = view_with_chains(vec![advisory_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        let mut ctx = ValidationContext::new(&mut snap);
        let result =
            WorkflowGuardRule.validate(&view, &mut ctx, &event("build.finished", None));
        assert!(result.accepted);
    }

    #[test]
    fn accepts_idempotent_re_emission() {
        let view = view_with_chains(vec![strict_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        snap.workflow_phases
            .insert("experiment::".to_string(), 1);
        let mut ctx = ValidationContext::new(&mut snap);
        let result = WorkflowGuardRule.validate(&view, &mut ctx, &event("experiment.run", None));
        assert!(result.accepted);
    }

    #[test]
    fn accepts_in_order_advance() {
        let view = view_with_chains(vec![strict_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        snap.workflow_phases
            .insert("experiment::".to_string(), 1);
        let mut ctx = ValidationContext::new(&mut snap);
        let result =
            WorkflowGuardRule.validate(&view, &mut ctx, &event("experiment.evaluated", None));
        assert!(result.accepted);
    }

    #[test]
    fn rejects_correlation_extraction_failure() {
        let mut chain = strict_chain();
        chain.correlation = Some(CorrelationConfig {
            from_payload: "experiment_id".to_string(),
            from_topic: None,
        });
        let view = view_with_chains(vec![chain]);
        let mut snap = LedgerSnapshot::cold_start();
        let mut ctx = ValidationContext::new(&mut snap);
        let result = WorkflowGuardRule.validate(
            &view,
            &mut ctx,
            &event("experiment.planned", Some(r#"{}"#)),
        );
        assert!(!result.accepted);
        assert_eq!(
            result.reason_code.as_deref(),
            Some(ReasonCode::WORKFLOW_GUARD_OUT_OF_ORDER)
        );
    }

    #[test]
    fn accepts_correlated_instance_start() {
        let mut chain = strict_chain();
        chain.correlation = Some(CorrelationConfig {
            from_payload: "experiment_id".to_string(),
            from_topic: None,
        });
        let view = view_with_chains(vec![chain]);
        let mut snap = LedgerSnapshot::cold_start();
        // Another instance is further ahead; this must not affect the new instance.
        snap.workflow_phases
            .insert("experiment::other".to_string(), 2);
        let mut ctx = ValidationContext::new(&mut snap);
        let result = WorkflowGuardRule.validate(
            &view,
            &mut ctx,
            &event(
                "experiment.planned",
                Some(r#"{"experiment_id":"exp-1"}"#),
            ),
        );
        assert!(result.accepted);
    }
}
