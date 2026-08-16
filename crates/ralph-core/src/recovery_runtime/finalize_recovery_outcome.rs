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
/// is present for the same retry key, the key is still in a
/// non-terminal outcome, AND the attempt count has reached the
/// configured cap (`ctx.handoff_retry_cap`, sourced from
/// `TelemetryConfig::max_repeated_recoveries`). This used to be
/// a single-shot force-block on every pending timeout (plan
/// 2026-08-16-1015 Unit 3): the first timeout must now be allowed
/// to traverse the targeted `task.resume` route and only escalate
/// once the bounded retry budget is exhausted.
fn handoff_timeout_pending(ctx: &RuntimeContext, state: &super::RetryKeyState) -> bool {
    if !is_nonterminal_outcome(&state.last_outcome) {
        return false;
    }
    // Saturate to at least 1 so a misconfigured 0 cannot silently
    // bypass the bounded-retry intent. Production context always
    // passes >= 1 (config validation rejects 0); manual Default
    // for tests uses 3.
    let cap = ctx.handoff_retry_cap.max(1);
    if state.attempt_count < cap {
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
                outcome_history: [
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

    // ----------------------------------------------------------------
    // Plan 2026-08-16-1015 Unit 3 acceptance tests
    // ----------------------------------------------------------------

    /// Unit 3 S5: a handoff timeout with attempt_count below the
    /// cap must NOT produce `ForcePlanBlocked`; the targeted
    /// `task.resume` route is allowed to run.
    #[test]
    fn handoff_dispatch_timeout_does_not_block_before_retry_cap() {
        use super::super::EnvelopeSnapshot;
        for attempt in 1..3 {
            let ctx = RuntimeContext {
                retry_key_states: vec![RetryKeyState {
                    retry_key:
                        "stall_recovery:review-synthesizer:review_complete:handoff_dispatch_timeout:*"
                            .to_string(),
                    last_outcome: "Pending".to_string(),
                    outcome_history: vec!["Pending".to_string(); 3],
                    attempt_count: attempt,
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
                // Default cap (3) is greater than every attempt.
                ..Default::default()
            };
            let actions = finalize_recovery_outcome_on_flapping(&ctx);
            assert!(
                !actions.iter().any(|a| matches!(
                    a,
                    RecoveryAction::ForcePlanBlocked { reason, .. } if reason.contains("handoff_timeout_recovery_finalized")
                )),
                "attempt={attempt} under default cap=3 must not produce handoff finalizer block; got {actions:?}"
            );
        }
    }

    /// Unit 3 S6: handoff timeout at attempt == cap must produce
    /// exactly one `ForcePlanBlocked` action; attempt == cap + 1 must
    /// not produce a second one (caller-side idempotence on the
    /// retry key).
    #[test]
    fn handoff_dispatch_timeout_blocks_at_configured_retry_cap() {
        use super::super::EnvelopeSnapshot;
        let key =
            "stall_recovery:review-synthesizer:review_complete:handoff_dispatch_timeout:*";
        let envelope = EnvelopeSnapshot {
            retry_key: "stall_recovery:review-synthesizer:review_complete:handoff_dispatch_timeout:r1"
                .to_string(),
            source: "StallRecovery".to_string(),
            outcome: "Pending".to_string(),
            iteration: 10,
            attempt: 1,
        };
        // Exact cap: 3 attempts vs cap=3 → block exactly once.
        let ctx_exact = RuntimeContext {
            retry_key_states: vec![RetryKeyState {
                retry_key: key.to_string(),
                last_outcome: "Pending".to_string(),
                outcome_history: vec!["Pending".to_string(); 3],
                attempt_count: 3,
            }],
            recovery_envelopes: vec![envelope.clone()],
            handoff_retry_cap: 3,
            ..Default::default()
        };
        let actions = finalize_recovery_outcome_on_flapping(&ctx_exact);
        let blocks: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, RecoveryAction::ForcePlanBlocked { reason, .. } if reason.contains("handoff_timeout_recovery_finalized")))
            .collect();
        assert_eq!(blocks.len(), 1, "exact cap must produce one block");
        // Over cap: 4 attempts (caller retries) — already blocked once,
        // finalizer still produces exactly one block action (caller is
        // responsible for dedup).
        let ctx_over = RuntimeContext {
            retry_key_states: vec![RetryKeyState {
                retry_key: key.to_string(),
                last_outcome: "Pending".to_string(),
                outcome_history: vec!["Pending".to_string(); 4],
                attempt_count: 4,
            }],
            recovery_envelopes: vec![envelope],
            handoff_retry_cap: 3,
            ..Default::default()
        };
        let actions_over = finalize_recovery_outcome_on_flapping(&ctx_over);
        let blocks_over: Vec<_> = actions_over
            .iter()
            .filter(|a| matches!(a, RecoveryAction::ForcePlanBlocked { reason, .. } if reason.contains("handoff_timeout_recovery_finalized")))
            .collect();
        assert_eq!(blocks_over.len(), 1, "over cap still produces one block");
    }

    /// Unit 3 S7: terminal outcome (`Failed`) makes
    /// `handoff_timeout_pending` short-circuit to false regardless of
    /// attempt count.
    #[test]
    fn handoff_dispatch_timeout_ignores_terminal_outcome() {
        use super::super::EnvelopeSnapshot;
        let ctx = RuntimeContext {
            retry_key_states: vec![RetryKeyState {
                retry_key:
                    "stall_recovery:review-synthesizer:review_complete:handoff_dispatch_timeout:*"
                        .to_string(),
                last_outcome: "Failed".to_string(),
                outcome_history: vec!["Failed".to_string(); 5],
                attempt_count: 5,
            }],
            recovery_envelopes: vec![EnvelopeSnapshot {
                retry_key:
                    "stall_recovery:review-synthesizer:review_complete:handoff_dispatch_timeout:r1"
                        .to_string(),
                source: "StallRecovery".to_string(),
                outcome: "Failed".to_string(),
                iteration: 10,
                attempt: 5,
            }],
            ..Default::default()
        };
        let actions = finalize_recovery_outcome_on_flapping(&ctx);
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, RecoveryAction::ForcePlanBlocked { reason, .. }
                    if reason.contains("handoff_timeout_recovery_finalized"))),
            "terminal outcome must never trigger handoff finalizer block; got {actions:?}"
        );
    }

    /// Unit 3: cap=0 must saturate to 1 (config validation already
    /// rejects cap=0; this guards hand-rolled test contexts).
    #[test]
    fn handoff_retry_cap_saturates_to_one_when_zero() {
        use super::super::EnvelopeSnapshot;
        let ctx = RuntimeContext {
            retry_key_states: vec![RetryKeyState {
                retry_key:
                    "stall_recovery:review-synthesizer:review_complete:handoff_dispatch_timeout:*"
                        .to_string(),
                last_outcome: "Pending".to_string(),
                outcome_history: vec!["Pending".to_string(); 2],
                attempt_count: 1,
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
            handoff_retry_cap: 0,
            ..Default::default()
        };
        let actions = finalize_recovery_outcome_on_flapping(&ctx);
        // attempt_count (1) >= saturated cap (1) → block.
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, RecoveryAction::ForcePlanBlocked { reason, .. }
                    if reason.contains("handoff_timeout_recovery_finalized")))
                .count(),
            1,
            "cap=0 must saturate to 1; got {actions:?}"
        );
    }
}
