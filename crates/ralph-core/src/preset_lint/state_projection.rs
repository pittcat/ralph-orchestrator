//! Plan 2026-06-20-001 U1 KTD-3: build-time assertion that
//! `state_projection.actions_chain.work.done` orders
//! `close_task` *before* `mark_step_completed`.
//!
//! This is the **primary** (主) defence per KTD-3: the YAML
//! author must put the action that closes the task first, so the
//! subsequent `mark_step_completed` step sees a coherent
//! `tasks.jsonl` state. The engine typestate in
//! `state_projector/mod.rs` is the **secondary** (辅) check and
//! only catches Rust-side dispatch bugs.
//!
//! Without this check, a reversed chain reproduces the
//! `ce-executor-serial-primary-20260619` 死循环 where
//! `progress_task_gate` rejects the next emit because
//! `## Completed Steps` lags the task close.

use crate::config::RalphConfig;
use crate::config::StateProjectionAction;
use crate::preset_lint::finding_id::FINDING_WORK_DONE_ACTION_CHAIN_ORDER;
use crate::preset_lint::{LintFinding, LintSeverity};

const WORK_DONE_TOPIC: &str = "work.done";

/// Return an error finding when `work.done`'s `actions_chain`
/// puts `mark_step_completed` ahead of `close_task`. Returns
/// `Vec::new()` when the chain is absent, the projector is
/// disabled, or the chain is not in scope of this rule
/// (legacy single-action `actions` map, or chain missing one
/// of the two actions — those are flagged by other rules
/// such as `check_publishes_have_schema`).
///
/// Severity: `Error` (always). Order is semantic.
pub fn check_work_done_action_chain_order(config: &RalphConfig) -> Vec<LintFinding> {
    let sp = &config.event_loop.state_projection;
    if !sp.enabled {
        return Vec::new();
    }
    // The legacy `actions` map holds one action per topic, so
    // order is undefined and not the SSOT form (F-PS-005
    // removed it from `presets/en/ce-executor-serial.yml`).
    // Skip — this rule is scoped to `actions_chain`.
    let Some(chain) = sp.actions_chain.get(WORK_DONE_TOPIC) else {
        return Vec::new();
    };
    let close_idx = chain
        .iter()
        .position(|a| matches!(a, StateProjectionAction::CloseTask { .. }));
    let mark_idx = chain
        .iter()
        .position(|a| matches!(a, StateProjectionAction::MarkStepCompleted { .. }));
    match (close_idx, mark_idx) {
        (Some(c), Some(m)) if c < m => Vec::new(),
        (Some(_), Some(_)) => vec![order_finding()],
        // Chain missing one or both: not in scope of this rule.
        _ => Vec::new(),
    }
}

fn order_finding() -> LintFinding {
    LintFinding {
        id: FINDING_WORK_DONE_ACTION_CHAIN_ORDER,
        severity: LintSeverity::Error,
        message: format!(
            "`state_projection.actions_chain.work.done` must place `close_task` \
             before `mark_step_completed` (R3 / KTD-3). The current order would \
             let `progress_task_gate` see the step AFTER the task close and \
             reject the next emit, reintroducing the \
             `ce-executor-serial-primary-20260619` 死循环."
        ),
        topic: Some(WORK_DONE_TOPIC.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(
            "Swap the order: `close_task` (task_id, step) MUST come first, \
             then `mark_step_completed` (step). Both stay in the same \
             `actions_chain` list under the SSOT at \
             `presets/schemas/ce-executor-serial.yml`."
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> RalphConfig {
        serde_yaml::from_str(yaml).expect("config parses")
    }

    #[test]
    fn work_done_close_before_mark_passes() {
        let yaml = r#"
prompt_file: PROMPT.md
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  state_projection:
    enabled: true
    actions_chain:
      work.done:
        - kind: close_task
          task_id: "task_id"
          step: "step"
        - kind: mark_step_completed
          step: "step"
"#;
        let config = parse(yaml);
        let findings = check_work_done_action_chain_order(&config);
        assert!(
            findings.is_empty(),
            "correct order must produce no finding, got {findings:?}"
        );
    }

    #[test]
    fn work_done_mark_before_close_emits_finding() {
        let yaml = r#"
prompt_file: PROMPT.md
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  state_projection:
    enabled: true
    actions_chain:
      work.done:
        - kind: mark_step_completed
          step: "step"
        - kind: close_task
          task_id: "task_id"
          step: "step"
"#;
        let config = parse(yaml);
        let findings = check_work_done_action_chain_order(&config);
        assert_eq!(findings.len(), 1, "expected 1 finding, got {findings:?}");
        let f = &findings[0];
        assert_eq!(f.id, FINDING_WORK_DONE_ACTION_CHAIN_ORDER);
        assert_eq!(f.severity, LintSeverity::Error);
        assert_eq!(f.topic.as_deref(), Some("work.done"));
        assert!(
            f.action_hint.as_deref().unwrap().contains("close_task"),
            "action_hint must mention close_task, got {:?}",
            f.action_hint
        );
    }

    #[test]
    fn work_done_missing_chain_no_finding() {
        // Legacy `actions` map (single action) is out of scope for this rule.
        let yaml = r#"
prompt_file: PROMPT.md
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  state_projection:
    enabled: true
    actions:
      work.done:
        kind: close_task
        task_id: "task_id"
        step: "step"
"#;
        let config = parse(yaml);
        let findings = check_work_done_action_chain_order(&config);
        assert!(
            findings.is_empty(),
            "legacy single-action form is out of scope, got {findings:?}"
        );
    }

    #[test]
    fn work_done_disabled_projector_no_finding() {
        let yaml = r#"
prompt_file: PROMPT.md
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  state_projection:
    enabled: false
    actions_chain:
      work.done:
        - kind: mark_step_completed
          step: "step"
        - kind: close_task
          task_id: "task_id"
          step: "step"
"#;
        let config = parse(yaml);
        let findings = check_work_done_action_chain_order(&config);
        assert!(
            findings.is_empty(),
            "disabled projector must not flag, got {findings:?}"
        );
    }
}
