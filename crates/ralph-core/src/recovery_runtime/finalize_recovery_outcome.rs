//! Detect outcome flapping for a single retry key and force convergence to
//! `plan.blocked` instead of allowing the loop to burn iterations forever.
//!
//! See plan 2026-06-28-003 §Defense 2, function 2.

use super::{RecoveryAction, RuntimeContext};

/// Thresholds: if a retry key's outcome flips among Pending/Recovered/Repeated
/// at least this many times within the recent history, force a terminal
/// `plan.blocked`. These are intentionally hard-coded for the first iteration;
/// parameterization is deferred to follow-up work.
const FLAP_WINDOW: usize = 8;
const FLAP_THRESHOLD: usize = 3;

pub fn finalize_recovery_outcome_on_flapping(ctx: &RuntimeContext) -> Vec<RecoveryAction> {
    let mut actions = Vec::new();
    for state in &ctx.retry_key_states {
        if is_flapping(&state.outcome_history) {
            actions.push(RecoveryAction::ForcePlanBlocked {
                reason: format!(
                    "outcome_flapping:{}:flips={}",
                    state.retry_key,
                    count_flips(&state.outcome_history)
                ),
                retry_key: state.retry_key.clone(),
            });
        }
    }
    actions
}

fn is_flapping(history: &[String]) -> bool {
    let recent: Vec<&str> = history.iter().rev().take(FLAP_WINDOW).map(|s| s.as_str()).collect();
    if recent.len() < FLAP_THRESHOLD + 1 {
        return false;
    }
    count_flips(&recent.iter().rev().map(|s| s.to_string()).collect::<Vec<_>>()) >= FLAP_THRESHOLD
}

fn count_flips(history: &[String]) -> usize {
    if history.len() < 2 {
        return 0;
    }
    let mut flips = 0;
    for w in history.windows(2) {
        if w[0] != w[1] {
            flips += 1;
        }
    }
    flips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_flap_when_history_stable() {
        let ctx = RuntimeContext {
            retry_key_states: vec![RetryKeyState {
                retry_key: "k".to_string(),
                last_outcome: "Pending".to_string(),
                outcome_history: vec!["Pending".to_string(); 5],
                attempt_count: 5,
            }],
            ..Default::default()
        };
        assert!(finalize_recovery_outcome_on_flapping(&ctx).is_empty());
    }

    #[test]
    fn flapping_triggers_force_plan_blocked() {
        let history: Vec<String> = ["Pending", "Recovered", "Repeated", "Recovered", "Pending"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let ctx = RuntimeContext {
            retry_key_states: vec![RetryKeyState {
                retry_key: "k".to_string(),
                last_outcome: "Pending".to_string(),
                outcome_history: history,
                attempt_count: 5,
            }],
            ..Default::default()
        };
        let actions = finalize_recovery_outcome_on_flapping(&ctx);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], RecoveryAction::ForcePlanBlocked { reason, .. } if reason.contains("outcome_flapping"))
        );
    }
}
