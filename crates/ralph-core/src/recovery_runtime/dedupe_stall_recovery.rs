//! Detect when a `missing_event_gate` envelope duplicates a
//! `stall_recovery` envelope for the same `(hat, topic)` in the same
//! iteration. The two retry keys should share an attempt window so they do
//! not race to `EscalationLevel::Final` independently.
//!
//! See plan 2026-06-28-003 §Defense 2, function 1.

use super::{RecoveryAction, RuntimeContext};

/// If the current iteration already has a `stall_recovery` envelope for the
/// same hat/topic, drop the secondary `missing_event_gate` envelope.
///
/// This is the runtime half of the dedup guard that already lives in
/// `ralph-cli/src/loop_runner/hard_gate.rs`; keeping a detector here lets
/// other callers (unit tests, drift engine) reason about the rule directly.
pub fn dedupe_stall_recovery_with_missing_event_gate(ctx: &RuntimeContext) -> Vec<RecoveryAction> {
    // We need at least two envelopes on the same iteration to compare.
    if ctx.recovery_envelopes.len() < 2 {
        return Vec::new();
    }

    let stall_keys: Vec<String> = ctx
        .recovery_envelopes
        .iter()
        .filter(|e| e.source == "StallRecovery" || e.retry_key.starts_with("stall_recovery:"))
        .map(|e| normalize_retry_key(&e.retry_key))
        .collect();

    if stall_keys.is_empty() {
        return Vec::new();
    }

    ctx.recovery_envelopes
        .iter()
        .filter(|e| {
            (e.source == "MissingEventGate" || e.retry_key.starts_with("missing_event_gate:"))
                && stall_keys
                    .iter()
                    .any(|stall| same_hat_topic(stall, &e.retry_key))
        })
        .map(|e| RecoveryAction::DedupeEnvelope {
            drop_retry_key: e.retry_key.clone(),
        })
        .collect()
}

/// Strip the trailing reason-code wildcard so two retry keys that differ
/// only in the last segment can be compared on hat+topic.
fn normalize_retry_key(key: &str) -> String {
    let mut parts: Vec<&str> = key.split(':').collect();
    if parts.len() >= 4 {
        // Replace the last field (reason/wildcard) with "*" so the seed key
        // and the envelope key line up.
        parts.pop();
        parts.push("*");
        parts.join(":")
    } else {
        key.to_string()
    }
}

/// True when two retry keys share the same hat and topic segments.
fn same_hat_topic(a: &str, b: &str) -> bool {
    let a_parts: Vec<&str> = a.split(':').collect();
    let b_parts: Vec<&str> = b.split(':').collect();
    a_parts.len() >= 3
        && b_parts.len() >= 3
        && a_parts[1] == b_parts[1]
        && a_parts[2] == b_parts[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::EnvelopeSnapshot;

    #[test]
    fn empty_context_yields_no_action() {
        assert!(dedupe_stall_recovery_with_missing_event_gate(&RuntimeContext::default()).is_empty());
    }

    #[test]
    fn dedups_when_stall_and_missing_event_share_hat_topic() {
        let ctx = RuntimeContext {
            recovery_envelopes: vec![
                EnvelopeSnapshot {
                    retry_key: "stall_recovery:executor:work_done:handoff_dispatch_timeout:*".to_string(),
                    source: "StallRecovery".to_string(),
                    outcome: "Pending".to_string(),
                    iteration: 5,
                    attempt: 1,
                },
                EnvelopeSnapshot {
                    retry_key: "missing_event_gate:executor:work_done:missing_event:*".to_string(),
                    source: "MissingEventGate".to_string(),
                    outcome: "Pending".to_string(),
                    iteration: 5,
                    attempt: 1,
                },
            ],
            ..Default::default()
        };
        let actions = dedupe_stall_recovery_with_missing_event_gate(&ctx);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], RecoveryAction::DedupeEnvelope { drop_retry_key } if drop_retry_key.contains("missing_event_gate"))
        );
    }

    #[test]
    fn no_action_when_only_one_envelope() {
        let ctx = RuntimeContext {
            recovery_envelopes: vec![EnvelopeSnapshot {
                retry_key: "missing_event_gate:executor:work_done:missing_event:*".to_string(),
                source: "MissingEventGate".to_string(),
                outcome: "Pending".to_string(),
                iteration: 5,
                attempt: 1,
            }],
            ..Default::default()
        };
        assert!(dedupe_stall_recovery_with_missing_event_gate(&ctx).is_empty());
    }
}
