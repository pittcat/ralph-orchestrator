//! Detect an executor `work.done` resend storm: the same task key emits
//! `work.done` repeatedly without the commit count advancing. When the count
//! reaches the threshold, inject a hard stop directive.
//!
//! See plan 2026-06-28-003 §Defense 2, function 4.

use super::{EventSnapshot, RecoveryAction, RuntimeContext};
use serde_json::Value;

const RESEND_WINDOW: usize = 8;
const RESEND_THRESHOLD: usize = 3;

pub fn block_executor_resend_storm(ctx: &RuntimeContext) -> Vec<RecoveryAction> {
    // Resend-storm detection requires the runtime to identify the
    // currently active executor-class hat structurally (by which hat
    // publishes `work.done`) rather than by a fragile string
    // comparison against the literal name "executor". When
    // `executor_hat_ids` is empty (e.g. unit tests construct a bare
    // context) fall back to the legacy "executor" string so existing
    // tests keep passing.
    let current_hat_is_executor = match ctx.current_hat.as_deref() {
        Some(hat) => {
            if ctx.executor_hat_ids.iter().any(|id| id == hat) {
                true
            } else {
                ctx.executor_hat_ids.is_empty() && hat == "executor"
            }
        }
        None => false,
    };
    if !current_hat_is_executor {
        return Vec::new();
    }

    let recent_events: Vec<&EventSnapshot> = ctx
        .events
        .iter()
        .rev()
        .take(RESEND_WINDOW)
        .collect();

    let mut work_done_by_task: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
    for event in recent_events.iter().rev() {
        if event.topic != "work.done" {
            continue;
        }
        let Some(task_key) = task_key_from_payload(&event.payload) else {
            continue;
        };
        let commit_count = commit_count_from_payload(&event.payload);
        work_done_by_task
            .entry(task_key)
            .or_default()
            .push(commit_count);
    }

    for (_task_key, counts) in work_done_by_task {
        if counts.len() >= RESEND_THRESHOLD && all_same(&counts) {
            return vec![RecoveryAction::InjectDirective {
                text: "ralph stop".to_string(),
            }];
        }
    }

    Vec::new()
}

fn task_key_from_payload(payload: &str) -> Option<String> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let obj = value.as_object()?;
    obj.get("task_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            obj.get("plan_path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

fn commit_count_from_payload(payload: &str) -> u32 {
    let value: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    value
        .get("commit_count")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .unwrap_or(0)
}

fn all_same(values: &[u32]) -> bool {
    values.iter().all(|v| *v == values[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_action_for_non_executor() {
        let ctx = RuntimeContext {
            current_hat: Some("reviewer".to_string()),
            events: vec![EventSnapshot {
                topic: "work.done".to_string(),
                payload: r#"{"task_id":"t1","commit_count":1}"#.to_string(),
                iteration: 5,
            }],
            ..Default::default()
        };
        assert!(block_executor_resend_storm(&ctx).is_empty());
    }

    #[test]
    fn triggers_when_commit_count_stalls() {
        let ctx = RuntimeContext {
            current_hat: Some("executor".to_string()),
            events: (0..3)
                .map(|i| EventSnapshot {
                    topic: "work.done".to_string(),
                    payload: r#"{"task_id":"t1","commit_count":1}"#.to_string(),
                    iteration: 5 + i as u32,
                })
                .collect(),
            ..Default::default()
        };
        let actions = block_executor_resend_storm(&ctx);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], RecoveryAction::InjectDirective { text } if text == "ralph stop")
        );
    }

    #[test]
    fn no_trigger_when_commit_count_advances() {
        let ctx = RuntimeContext {
            current_hat: Some("executor".to_string()),
            events: (0..3)
                .map(|i| EventSnapshot {
                    topic: "work.done".to_string(),
                    payload: format!(r#"{{"task_id":"t1","commit_count":{}}}"#, i + 1),
                    iteration: 5 + i as u32,
                })
                .collect(),
            ..Default::default()
        };
        assert!(block_executor_resend_storm(&ctx).is_empty());
    }

    #[test]
    fn triggers_for_executor_class_hat_by_registry_list() {
        // P1 fix: the detector must recognise any hat whose config
        // publishes `work.done`, not just the literal "executor"
        // name. The live registry populates `executor_hat_ids` from
        // `HatConfig.publishes`, so a renamed executor hat (e.g.
        // `executor-fix-unit`) still triggers the block.
        let ctx = RuntimeContext {
            current_hat: Some("executor-fix-unit".to_string()),
            executor_hat_ids: vec!["executor-fix-unit".to_string()],
            events: (0..3)
                .map(|i| EventSnapshot {
                    topic: "work.done".to_string(),
                    payload: r#"{"task_id":"t1","commit_count":2}"#.to_string(),
                    iteration: 5 + i as u32,
                })
                .collect(),
            ..Default::default()
        };
        let actions = block_executor_resend_storm(&ctx);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], RecoveryAction::InjectDirective { text } if text == "ralph stop"));
    }

    #[test]
    fn ignores_non_executor_hat_even_with_empty_registry_list() {
        // Empty registry list = the live registry has not yet been
        // populated. The detector must NOT silently accept any hat
        // when the structural list is empty: it must fall back to
        // the legacy "executor" string match so a renamed hat does
        // not accidentally fire (e.g. `planner` hat misconfigured).
        let ctx = RuntimeContext {
            current_hat: Some("planner".to_string()),
            executor_hat_ids: vec![],
            events: (0..3)
                .map(|i| EventSnapshot {
                    topic: "work.done".to_string(),
                    payload: r#"{"task_id":"t1","commit_count":1}"#.to_string(),
                    iteration: 5 + i as u32,
                })
                .collect(),
            ..Default::default()
        };
        assert!(block_executor_resend_storm(&ctx).is_empty());
    }
}
