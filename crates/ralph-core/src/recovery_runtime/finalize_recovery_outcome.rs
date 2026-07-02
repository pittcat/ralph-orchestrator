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

/// 2026-06-29-007 plan U2: if a retry key has been stuck in a
/// non-terminal outcome for this many consecutive observations,
/// force `plan.blocked` even if the per-window flip counter resets.
/// This catches the `primary-20260629-120038` pattern where the
/// same key flipped 14 times without converging.
const LONG_NONTERMINAL_HISTORY_THRESHOLD: usize = 6;

const NONTERMINAL_OUTCOMES: &[&str] = &["Pending", "Recovered", "Repeated"];

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
            continue;
        }

        if handoff_timeout_pending(ctx, state) {
            actions.push(RecoveryAction::ForcePlanBlocked {
                reason: format!("handoff_timeout_recovery_finalized:{}", state.retry_key),
                retry_key: state.retry_key.clone(),
            });
            continue;
        }

        if is_long_nonterminal_history(&state.outcome_history) {
            actions.push(RecoveryAction::ForcePlanBlocked {
                reason: format!(
                    "outcome_history_exhausted:{}:len={}",
                    state.retry_key,
                    state.outcome_history.len()
                ),
                retry_key: state.retry_key.clone(),
            });
        }
    }
    actions
}

fn is_flapping(history: &[String]) -> bool {
    let recent: Vec<&str> = history
        .iter()
        .rev()
        .take(FLAP_WINDOW)
        .map(|s| s.as_str())
        .collect();
    if recent.len() < FLAP_THRESHOLD + 1 {
        return false;
    }
    count_flips(
        &recent
            .iter()
            .rev()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
    ) >= FLAP_THRESHOLD
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

/// True when a `handoff_dispatch_timeout` envelope from StallRecovery
/// is present for the same retry key and the key is still in a
/// non-terminal outcome. This prevents the 600s timeout from simply
/// injecting yet another `task.resume`; instead we escalate to
/// `plan.blocked`.
fn handoff_timeout_pending(ctx: &RuntimeContext, state: &super::RetryKeyState) -> bool {
    if !is_nonterminal_outcome(&state.last_outcome) {
        return false;
    }
    let state_key = normalize_retry_key(&state.retry_key);
    ctx.recovery_envelopes.iter().any(|e| {
        e.source == "StallRecovery"
            && e.retry_key.contains("handoff_dispatch_timeout")
            && normalize_retry_key(&e.retry_key) == state_key
    })
}

/// True when the last N outcomes are all non-terminal. Catches keys
/// that oscillate or stay Pending/Recovered/Repeated without ever
/// reaching Failed, even when the responder's window clear resets the
/// flip counter.
fn is_long_nonterminal_history(history: &[String]) -> bool {
    if history.len() < LONG_NONTERMINAL_HISTORY_THRESHOLD {
        return false;
    }
    history
        .iter()
        .rev()
        .take(LONG_NONTERMINAL_HISTORY_THRESHOLD)
        .all(|o| is_nonterminal_outcome(o))
}

fn is_nonterminal_outcome(outcome: &str) -> bool {
    NONTERMINAL_OUTCOMES.contains(&outcome)
}

/// Strip the trailing reason-code wildcard so two retry keys that differ
/// only in the last segment can be compared. Mirrors the normalisation
/// in `dedupe_stall_recovery.rs`.
fn normalize_retry_key(key: &str) -> String {
    let mut parts: Vec<&str> = key.split(':').collect();
    if parts.len() >= 4 {
        parts.pop();
        parts.push("*");
        parts.join(":")
    } else {
        key.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::RetryKeyState;
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

    #[test]
    fn handoff_dispatch_timeout_forces_plan_blocked_when_pending() {
        use super::super::EnvelopeSnapshot;
        let ctx = RuntimeContext {
            retry_key_states: vec![RetryKeyState {
                retry_key:
                    "stall_recovery:review-synthesizer:review_complete:handoff_dispatch_timeout:*"
                        .to_string(),
                last_outcome: "Pending".to_string(),
                outcome_history: vec!["Pending".to_string(); 3],
                attempt_count: 3,
            }],
            recovery_envelopes: vec![EnvelopeSnapshot {
                retry_key:
                    "stall_recovery:review-synthesizer:review_complete:handoff_dispatch_timeout:r1"
                        .to_string(),
                source: "StallRecovery".to_string(),
                outcome: "Pending".to_string(),
                iteration: 10,
                attempt: 1,
            }],
            ..Default::default()
        };
        let actions = finalize_recovery_outcome_on_flapping(&ctx);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], RecoveryAction::ForcePlanBlocked { reason, .. } if reason.contains("handoff_timeout_recovery_finalized"))
        );
    }

    #[test]
    fn handoff_dispatch_timeout_ignored_when_recovered_terminal() {
        use super::super::EnvelopeSnapshot;
        let ctx = RuntimeContext {
            retry_key_states: vec![RetryKeyState {
                retry_key:
                    "stall_recovery:review-synthesizer:review_complete:handoff_dispatch_timeout:*"
                        .to_string(),
                last_outcome: "Failed".to_string(),
                outcome_history: vec!["Failed".to_string()],
                attempt_count: 1,
            }],
            recovery_envelopes: vec![EnvelopeSnapshot {
                retry_key:
                    "stall_recovery:review-synthesizer:review_complete:handoff_dispatch_timeout:r1"
                        .to_string(),
                source: "StallRecovery".to_string(),
                outcome: "Failed".to_string(),
                iteration: 10,
                attempt: 1,
            }],
            ..Default::default()
        };
        assert!(finalize_recovery_outcome_on_flapping(&ctx).is_empty());
    }

    #[test]
    fn long_nonterminal_history_forces_plan_blocked() {
        // Six consecutive non-terminal outcomes with no flips — the
        // flapping rule does NOT fire, but the long-history rule does.
        let ctx = RuntimeContext {
            retry_key_states: vec![RetryKeyState {
                retry_key: "k".to_string(),
                last_outcome: "Recovered".to_string(),
                outcome_history: vec![
                    "Pending",
                    "Pending",
                    "Pending",
                    "Recovered",
                    "Recovered",
                    "Recovered",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
                attempt_count: 6,
            }],
            ..Default::default()
        };
        let actions = finalize_recovery_outcome_on_flapping(&ctx);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], RecoveryAction::ForcePlanBlocked { reason, .. } if reason.contains("outcome_history_exhausted"))
        );
    }

    #[test]
    fn short_nonterminal_history_does_not_force_plan_blocked() {
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
}
