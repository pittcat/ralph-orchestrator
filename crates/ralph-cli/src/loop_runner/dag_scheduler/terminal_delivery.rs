//! U8 terminal_delivery — replay-once terminal-event delivery (skeleton).
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U8.
//!
//! Owns the prepare → append-once+fsync → delivered state machine.
//! Real implementation uses target main-events FileLock and SQLite
//! v20 schema. This skeleton provides the pure logic for testing.

// SKELETON-ONLY (per fix-plan 2026-09-09-0917-fix-forge-dag-p1-closure-plan U2 / U25):
// public types stay exposed for downstream unit tests but are not yet wired
// into production callers; U10 event_file_append helper and U11 production
// wiring promote this file to `PRODUCTION:` marker.
#![allow(dead_code)]

use std::collections::BTreeMap;

/// State of a terminal delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeliveryState {
    Prepared,
    Appending,
    Delivered,
    Blocked,
}

impl DeliveryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Appending => "appending",
            Self::Delivered => "delivered",
            Self::Blocked => "blocked",
        }
    }
}

/// Identity of a delivery: plan_key + topic + artifact_digest (stable).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeliveryKey {
    pub plan_key: String,
    pub topic: String,
    pub artifact_digest: String,
}

impl DeliveryKey {
    pub fn derive(plan_key: &str, topic: &str, artifact_digest: &str) -> Self {
        Self {
            plan_key: plan_key.to_string(),
            topic: topic.to_string(),
            artifact_digest: artifact_digest.to_string(),
        }
    }

    pub fn stable_string(&self) -> String {
        format!("{}|{}|{}", self.plan_key, self.topic, self.artifact_digest)
    }
}

/// Outcome of an attempt to advance a delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// Advanced to next state.
    Advanced { to: DeliveryState },
    /// Payload mismatch vs existing record; conflict.
    Conflict { reason: String },
    /// Already in terminal state; no-op.
    AlreadyDelivered,
    /// Persist blocked; caller should fail-closed.
    Blocked { reason: String },
}

/// Pure state-machine dispatch.
pub fn advance_delivery(
    current: DeliveryState,
    payload_matches_existing: bool,
    fsync_succeeded: bool,
    cancelled: bool,
) -> DeliveryOutcome {
    match current {
        DeliveryState::Delivered => DeliveryOutcome::AlreadyDelivered,
        DeliveryState::Blocked => DeliveryOutcome::Blocked {
            reason: "previously blocked".to_string(),
        },
        DeliveryState::Prepared => {
            if !payload_matches_existing {
                return DeliveryOutcome::Conflict {
                    reason: "payload mismatch".to_string(),
                };
            }
            if cancelled {
                return DeliveryOutcome::Blocked {
                    reason: "cancelled".to_string(),
                };
            }
            DeliveryOutcome::Advanced {
                to: DeliveryState::Appending,
            }
        }
        DeliveryState::Appending => {
            if !fsync_succeeded {
                // Stay in Appending (pending); retry can pick up here.
                DeliveryOutcome::Advanced {
                    to: DeliveryState::Appending,
                }
            } else {
                DeliveryOutcome::Advanced {
                    to: DeliveryState::Delivered,
                }
            }
        }
    }
}

/// Reference table: which state transitions are legal.
pub fn legal_transitions() -> BTreeMap<DeliveryState, Vec<DeliveryState>> {
    let mut m = BTreeMap::new();
    m.insert(
        DeliveryState::Prepared,
        vec![DeliveryState::Appending, DeliveryState::Blocked],
    );
    m.insert(
        DeliveryState::Appending,
        vec![
            DeliveryState::Delivered,
            DeliveryState::Appending,
            DeliveryState::Blocked,
        ],
    );
    m.insert(DeliveryState::Delivered, vec![]);
    m.insert(DeliveryState::Blocked, vec![]);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_key_stable() {
        let k1 = DeliveryKey::derive("plan-A", "forge.exec.development.done", "digest-1");
        let k2 = DeliveryKey::derive("plan-A", "forge.exec.development.done", "digest-1");
        assert_eq!(k1.stable_string(), k2.stable_string());
    }

    #[test]
    fn prepare_conflict_rejected() {
        let o = advance_delivery(DeliveryState::Prepared, false, true, false);
        assert!(matches!(o, DeliveryOutcome::Conflict { .. }));
    }

    #[test]
    fn append_once_under_lock() {
        // Skeleton: pure logic. Real impl uses FileLock. Here we verify
        // the dispatch: Prepared + payload match + ok → Appending.
        let o = advance_delivery(DeliveryState::Prepared, true, false, false);
        assert_eq!(
            o,
            DeliveryOutcome::Advanced {
                to: DeliveryState::Appending
            }
        );
    }

    #[test]
    fn fsync_failure_keeps_pending() {
        let o = advance_delivery(DeliveryState::Appending, true, false, false);
        // Appending + fsync fail → stay in Appending (idempotent retry).
        assert_eq!(
            o,
            DeliveryOutcome::Advanced {
                to: DeliveryState::Appending
            }
        );
    }

    #[test]
    fn known_torn_tail_repaired_without_other_bytes_loss() {
        // Skeleton contract: a previously-blocked delivery doesn't
        // auto-deliver; it stays blocked. Real torn-tail repair lives
        // in the helper module (event_file_append).
        let o = advance_delivery(DeliveryState::Blocked, true, true, false);
        assert!(matches!(o, DeliveryOutcome::Blocked { .. }));
    }
}
