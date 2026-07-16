//! U4b: `WorkflowGuardRule` — wraps the workflow-guard stage.
//!
//! Post-commit phase. The unified event loop calls
//! [`ValidationPipeline::validate_post_commit`] after every pre-commit
//! accept; this rule is the single source of truth for
//! out-of-order / correlation-extraction rejections. Side effects
//! (recovery-envelope writing, escalation) are owned by the
//! orchestrator layer; the rule only (a) checks validity against
//! the current progress and (b) advances progress on accept.
//!
//! The rule produces the `ReasonCode::WORKFLOW_GUARD_OUT_OF_ORDER`
//! reason code on rejection, matching the legacy diagnostic.
//!
//! The rule reads & writes the loop's `WorkflowProgress` through
//! the [`ValidationContext`] override. Side effects (recovery
//! envelope writing, escalation) are owned by the orchestrator
//! layer; the rule only (a) checks validity against the current
//! progress and (b) advances progress on accept.

use crate::config::WorkflowChain;
use crate::config::workflow_guards::WorkflowChainMode;
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

        // `WorkflowGuardRule` requires mutable access to a
        // `WorkflowProgress`. The event loop supplies the live
        // `LoopState::workflow_progress` via
        // [`ValidationContext::with_workflow_progress`]; in
        // tests the override is wired manually. When the
        // override is missing we cannot make a decision, so
        // we accept (matches the legacy pre-override default
        // of "no guard installed").
        let Some(progress) = ctx.workflow_progress_mut() else {
            return ValidationResult::accept_with(ValidationStage::WorkflowGuard);
        };

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
        let mut advances: Vec<(String, Option<String>, usize)> = Vec::new();

        for chain in &matching_chains {
            let instance_key = match extract_correlation_key(event, chain) {
                CorrelationKeyResult::Global => None,
                CorrelationKeyResult::Instance(key) => Some(key),
                CorrelationKeyResult::ExtractFailed => {
                    rejections.push(WorkflowGuardRejectionDetail {
                        chain_name: chain.name.clone(),
                        instance_key: None,
                        rejected_topic: event.topic.clone(),
                        current_phase: None,
                        current_topic: "none".to_string(),
                        next_expected: "unknown (correlation extraction failed)".to_string(),
                        source_hat: event.hat.clone(),
                        reason: "unknown (correlation extraction failed)".to_string(),
                    });
                    continue;
                }
            };

            let phase = chain
                .topics
                .iter()
                .position(|t| *t == event.topic)
                .expect("topic is in chain.topics by filter above");

            // Strict mode rejects out-of-order; advisory mode
            // tracks progress only.
            if matches!(chain.mode, WorkflowChainMode::Strict)
                && !progress.is_phase_valid(&chain.name, instance_key.as_deref(), phase)
            {
                let current_phase = progress.get_phase(&chain.name, instance_key.as_deref());
                let current_topic = current_phase
                    .and_then(|p| chain.topics.get(p).cloned())
                    .unwrap_or_else(|| "none".to_string());
                let next_expected = current_phase
                    .and_then(|p| chain.topics.get(p + 1).cloned())
                    .unwrap_or_else(|| "terminal".to_string());

                rejections.push(WorkflowGuardRejectionDetail {
                    chain_name: chain.name.clone(),
                    instance_key: instance_key.clone(),
                    rejected_topic: event.topic.clone(),
                    current_phase,
                    current_topic,
                    next_expected,
                    source_hat: event.hat.clone(),
                    reason: String::new(),
                });
                continue;
            }

            // Both strict and advisory chains advance progress for
            // in-order events (advisory never rejects; strict
            // accepts in-order).
            advances.push((chain.name.clone(), instance_key, phase));
        }

        if !rejections.is_empty() {
            // Build the consolidated reason string and let the
            // first rejection carry the structured detail. The
            // event loop's recovery-envelope writer reads the
            // detail; later rejections on the same event are
            // surfaced through the validation `correction_hint`.
            let reason = format!(
                "Workflow guard rejected '{}': {}",
                event.topic,
                rejections
                    .iter()
                    .map(|d| {
                        format!(
                            "chain '{}' (instance '{}'): current='{}' (phase {}), next expected='{}'",
                            d.chain_name,
                            d.instance_key.as_deref().unwrap_or("global"),
                            d.current_topic,
                            d.current_phase
                                .map(|p| p.to_string())
                                .unwrap_or_else(|| "none".to_string()),
                            d.next_expected
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            for d in &mut rejections {
                if d.reason.is_empty() {
                    d.reason = reason.clone();
                }
            }
            // Record the first rejection structurally so the
            // orchestrator can write a recovery envelope; later
            // rejections are summarised in the `correction_hint`
            // (the rule returns a single ValidationResult).
            let first = rejections.into_iter().next().expect("non-empty");
            ctx.record_workflow_guard_detail(first.clone());
            return ValidationResult::reject(
                ValidationStage::WorkflowGuard,
                ReasonCode::WORKFLOW_GUARD_OUT_OF_ORDER,
                Some(RejectionHint::workflow_guard_out_of_order(
                    &event.topic,
                    std::slice::from_ref(&first),
                )),
                true,
            );
        }

        // All chains accepted — advance progress for every
        // matched chain. The advance is idempotent: re-emitting
        // the same phase does not change the recorded highest.
        for (chain_name, instance_key, phase) in advances {
            progress.advance(&chain_name, instance_key.as_deref(), phase);
        }

        ValidationResult::accept_with(ValidationStage::WorkflowGuard)
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
    use crate::config::workflow_guards::CorrelationConfig;
    use crate::config::{WorkflowChain, WorkflowChainMode, WorkflowGuardsConfig};
    use crate::event_loop::WorkflowProgress;
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
            topics: vec!["build.started".to_string(), "build.finished".to_string()],
            mode: WorkflowChainMode::Advisory,
            correlation: None,
        }
    }

    fn view_with_chains(chains: Vec<WorkflowChain>) -> ProtocolView {
        let mut config = crate::config::EventLoopConfig::default();
        config.workflow_guards = Some(WorkflowGuardsConfig { chains });
        ProtocolView::from_event_loop(&config)
    }

    fn ctx_with_progress<'a>(
        snap: &'a mut LedgerSnapshot,
        progress: &'a mut WorkflowProgress,
    ) -> ValidationContext<'a> {
        ValidationContext::new(snap).with_workflow_progress(progress)
    }

    #[test]
    fn accepts_side_channel_event() {
        let view = view_with_chains(vec![strict_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        let mut progress = WorkflowProgress::new();
        let mut ctx = ctx_with_progress(&mut snap, &mut progress);
        let result = WorkflowGuardRule.validate(&view, &mut ctx, &event("periodic.review", None));
        assert!(result.accepted);
        assert_eq!(result.stage, ValidationStage::WorkflowGuard);
    }

    #[test]
    fn accepts_chain_start() {
        let view = view_with_chains(vec![strict_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        let mut progress = WorkflowProgress::new();
        let mut ctx = ctx_with_progress(&mut snap, &mut progress);
        let result =
            WorkflowGuardRule.validate(&view, &mut ctx, &event("experiment.planned", None));
        assert!(result.accepted);
        assert_eq!(progress.get_phase("experiment", None), Some(0));
    }

    #[test]
    fn rejects_out_of_order_strict_chain() {
        let view = view_with_chains(vec![strict_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        let mut progress = WorkflowProgress::new();
        progress.advance("experiment", None, 0);
        let mut details: Vec<WorkflowGuardRejectionDetail> = Vec::new();
        let mut ctx =
            ctx_with_progress(&mut snap, &mut progress).with_workflow_guard_details(&mut details);
        let result =
            WorkflowGuardRule.validate(&view, &mut ctx, &event("experiment.evaluated", None));
        assert!(!result.accepted);
        assert_eq!(
            result.reason_code.as_deref(),
            Some(ReasonCode::WORKFLOW_GUARD_OUT_OF_ORDER)
        );
        assert!(result.retry_eligible);
        assert!(
            result
                .correction_hint
                .as_deref()
                .unwrap_or("")
                .contains("experiment.run")
        );
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].current_phase, Some(0));
        assert_eq!(details[0].next_expected, "experiment.run");
    }

    #[test]
    fn accepts_advisory_out_of_order() {
        let view = view_with_chains(vec![advisory_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        let mut progress = WorkflowProgress::new();
        let mut ctx = ctx_with_progress(&mut snap, &mut progress);
        let result = WorkflowGuardRule.validate(&view, &mut ctx, &event("build.finished", None));
        assert!(result.accepted);
        // Advisory chains do not reject out-of-order events. The
        // `WorkflowProgress::advance` helper short-circuits when
        // the phase is not sequentially valid, so the first
        // emission of a higher phase does not record progress
        // (this matches the legacy behaviour).
        assert_eq!(progress.get_phase("build", None), None);
    }

    #[test]
    fn accepts_idempotent_re_emission() {
        let view = view_with_chains(vec![strict_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        let mut progress = WorkflowProgress::new();
        progress.advance("experiment", None, 0);
        progress.advance("experiment", None, 1);
        let mut ctx = ctx_with_progress(&mut snap, &mut progress);
        let result = WorkflowGuardRule.validate(&view, &mut ctx, &event("experiment.run", None));
        assert!(result.accepted);
    }

    #[test]
    fn accepts_in_order_advance() {
        let view = view_with_chains(vec![strict_chain()]);
        let mut snap = LedgerSnapshot::cold_start();
        let mut progress = WorkflowProgress::new();
        progress.advance("experiment", None, 0);
        progress.advance("experiment", None, 1);
        let mut ctx = ctx_with_progress(&mut snap, &mut progress);
        let result =
            WorkflowGuardRule.validate(&view, &mut ctx, &event("experiment.evaluated", None));
        assert!(result.accepted);
        assert_eq!(progress.get_phase("experiment", None), Some(2));
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
        let mut progress = WorkflowProgress::new();
        let mut details: Vec<WorkflowGuardRejectionDetail> = Vec::new();
        let mut ctx =
            ctx_with_progress(&mut snap, &mut progress).with_workflow_guard_details(&mut details);
        let result = WorkflowGuardRule.validate(
            &view,
            &mut ctx,
            &event("experiment.planned", Some(r"{}")),
        );
        assert!(!result.accepted);
        assert_eq!(
            result.reason_code.as_deref(),
            Some(ReasonCode::WORKFLOW_GUARD_OUT_OF_ORDER)
        );
        assert_eq!(details.len(), 1);
        assert_eq!(
            details[0].next_expected,
            "unknown (correlation extraction failed)"
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
        let mut progress = WorkflowProgress::new();
        // Another instance is further ahead; this must not affect the new instance.
        progress.advance("experiment", Some("other"), 0);
        progress.advance("experiment", Some("other"), 1);
        progress.advance("experiment", Some("other"), 2);
        let mut ctx = ctx_with_progress(&mut snap, &mut progress);
        let result = WorkflowGuardRule.validate(
            &view,
            &mut ctx,
            &event("experiment.planned", Some(r#"{"experiment_id":"exp-1"}"#)),
        );
        assert!(result.accepted);
    }
}
