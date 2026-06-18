//! WAC-U6 (2026-06-12-002): HandoffTracker — per-handoff dispatch
//! deadline tracking.
//!
//! Responsibilities:
//! - Record an accepted handoff event's `(topic, consumer, event_id,
//!   deadline)` so the dispatcher can detect "unique-consumer
//!   handoff accepted but consumer never activated within the
//!   configured timeout".
//! - On hat activation, clear the matching pending entries.
//! - On deadline expiry, expose the escalation payload that the
//!   main loop will route to `task.resume` for a safe target hat
//!   (plan-gate or review-coordinator) — never to a null terminal
//!   (KTD-13).
//!
//! The tracker is purely in-memory; persistence of escalation
//! envelopes to `.ralph/recovery.jsonl` is handled by the existing
//! `RecoveryResponder` and the diagnosis subsystem. The tracker
//! only decides *what* to escalate, not *how to write* the
//! recovery record.
//!
//! Construction is cheap (default state is empty). All public
//! methods are `&mut self` because the tracker mutates its
//! pending map on every call.
//!
//! ## Escalation semantics (U2, 2026-06-17 plan)
//!
//! The tracker implements **single Hard escalation** semantics:
//! each timed-out handoff produces exactly one
//! [`HandoffEscalation`] whose `safe_target` is either the
//! original consumer (the canonical path) or, when the original
//! consumer is itself `plan-gate` (i.e. plan-gate is the
//! bottleneck), `review-coordinator` as the audit fallback. There
//! is **no** repeated-counter or multi-step escalation ladder
//! (e.g. "3 Hard → 1 Final / plan.blocked"): if a subsequent
//! handoff on the same topic also times out, that handoff is
//! tracked and escalated independently by the loop's next
//! iteration. Operators see one escalation per stuck handoff;
//! the `reason` field includes the configured timeout so
//! diagnose reports can correlate timing.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A pending handoff awaiting consumer activation.
///
/// `deadline` is absolute (monotonic clock) so the main loop
/// can compare it against `Instant::now()` without
/// reconstructing a duration from a relative offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingHandoff {
    /// The handoff topic (e.g. `work.ready`).
    pub topic: String,
    /// The unique non-wildcard consumer of the handoff.
    pub consumer: String,
    /// Source event id (for traceability in the recovery envelope).
    pub event_id: String,
    /// Absolute deadline (monotonic clock) by which the consumer
    /// must activate.
    pub deadline: Instant,
}

impl PendingHandoff {
    /// `true` if the monotonic clock has passed `deadline`.
    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    /// Time remaining until deadline. Negative if expired.
    pub fn remaining(&self, now: Instant) -> Duration {
        self.deadline.saturating_duration_since(now)
    }
}

/// KTD-13 escalation payload built by `HandoffTracker::expired`.
///
/// The main loop wraps this in a recovery envelope (source:
/// `stall_recovery`, outcome: `escalated`) and routes
/// `task.resume` to `safe_target`. The escalation payload
/// includes the original handoff metadata so the diagnostic
/// report can correlate the cause with the loop's intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffEscalation {
    pub topic: String,
    pub consumer: String,
    pub event_id: String,
    /// Hat the loop should route the resume event to. Defaults
    /// to the plan-gate; if plan-gate is the original consumer
    /// that timed out, the loop should fall back to the
    /// review-coordinator.
    pub safe_target: String,
    /// Human-readable reason for the operator-facing report.
    pub reason: String,
}

/// WAC-U6 (2026-06-12-002): per-loop handoff deadline tracker.
#[derive(Debug, Default, Clone)]
pub struct HandoffTracker {
    /// Keyed by `event_id` so duplicate topics (consecutive
    /// handoffs on the same topic) do not collide. FIFO order
    /// is preserved by insertion (HashMap iteration order is
    /// unspecified; the consumer pick is driven by `min_by_key`
    /// on the deadline below).
    pending: HashMap<String, PendingHandoff>,
    /// Default safe target when the original consumer is the
    /// plan-gate (i.e. plan-gate is itself the bottleneck and
    /// cannot be the resume target).
    fallback_safe_target: String,
    /// Per-loop default timeout (set from `WorkflowContractConfig`).
    default_timeout: Duration,
}

impl HandoffTracker {
    /// Build a new tracker with default safe target `plan-gate`
    /// and default timeout `HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS`.
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
            fallback_safe_target: "plan-gate".to_string(),
            default_timeout: Duration::from_secs(
                crate::config::HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS,
            ),
        }
    }

    /// Override the default timeout (set from the loop's
    /// `WorkflowContractConfig` block at construction time).
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Override the fallback safe target (e.g. the loop
    /// can install `review-coordinator` when `plan-gate` is
    /// itself the bottleneck).
    pub fn with_fallback_safe_target(mut self, target: impl Into<String>) -> Self {
        self.fallback_safe_target = target.into();
        self
    }

    /// Record a newly accepted handoff. Returns the entry's
    /// `event_id` so the caller can correlate later activation
    /// or escalation.
    ///
    /// `now` is the monotonic clock at the moment the handoff
    /// was accepted (not the consumer's clock).
    pub fn on_handoff_accepted(
        &mut self,
        topic: impl Into<String>,
        consumer: impl Into<String>,
        event_id: impl Into<String>,
        now: Instant,
    ) {
        let deadline = now + self.default_timeout;
        let entry = PendingHandoff {
            topic: topic.into(),
            consumer: consumer.into(),
            event_id: event_id.into(),
            deadline,
        };
        self.pending.insert(entry.event_id.clone(), entry);
    }

    /// 2026-06-18-002 plan U5 (KTD-5): cancel a pending handoff
    /// by its `event_id`. Used by the hat_handoff gate to roll
    /// back the policy-accept `on_handoff_accepted` record when
    /// the gate rejects the same event for missing/invalid
    /// handoff content (phantom pending protection).
    ///
    /// Returns `true` if a pending entry was removed.
    pub fn cancel_pending(&mut self, event_id: &str) -> bool {
        self.pending.remove(event_id).is_some()
    }

    /// Remove all pending entries that point to `consumer` —
    /// called when a hat is activated and its pending queue is
    /// drained. If multiple handoffs were queued for the same
    /// consumer (e.g. consecutive `work.ready` events), all
    /// are cleared atomically.
    ///
    /// Returns the number of entries cleared.
    pub fn on_hat_activated(&mut self, consumer: &str) -> usize {
        let before = self.pending.len();
        self.pending.retain(|_, p| p.consumer != consumer);
        before - self.pending.len()
    }

    /// Number of pending entries (mostly for tests / diagnostics).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Collect all expired entries as escalations, removing
    /// them from the pending map. The caller is expected to
    /// route each escalation to a recovery envelope and inject
    /// a `task.resume` for `safe_target`.
    ///
    /// Determinism: escalations are returned sorted by
    /// `(topic, event_id)` so the loop applies them in a stable
    /// order across runs.
    pub fn expired(&mut self, now: Instant) -> Vec<HandoffEscalation> {
        let expired_keys: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| p.is_expired(now))
            .map(|(k, _)| k.clone())
            .collect();
        let mut escalations: Vec<HandoffEscalation> = expired_keys
            .into_iter()
            .filter_map(|k| self.pending.remove(&k))
            .map(|p| {
                let safe_target = if p.consumer == self.fallback_safe_target {
                    // Plan-gate is the bottleneck — fall back to
                    // review-coordinator. The latter is the
                    // canonical audit hat for ce-executor and
                    // has no overlap with plan-gate's input
                    // surface.
                    "review-coordinator".to_string()
                } else {
                    p.consumer.clone()
                };
                let reason = format!(
                    "handoff `{}` for consumer `{}` was accepted but no activation occurred within {}s",
                    p.event_id,
                    p.consumer,
                    self.default_timeout.as_secs()
                );
                HandoffEscalation {
                    topic: p.topic,
                    consumer: p.consumer,
                    event_id: p.event_id,
                    safe_target,
                    reason,
                }
            })
            .collect();
        escalations.sort_by(|a, b| a.topic.cmp(&b.topic).then(a.event_id.cmp(&b.event_id)));
        escalations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(seconds: u64) -> Instant {
        Instant::now() + Duration::from_secs(seconds)
    }

    #[test]
    fn pending_handoff_is_expired_after_deadline() {
        let p = PendingHandoff {
            topic: "work.ready".into(),
            consumer: "executor".into(),
            event_id: "evt-1".into(),
            deadline: t(10),
        };
        assert!(!p.is_expired(t(9)));
        assert!(p.is_expired(t(10)));
        assert!(p.is_expired(t(11)));
    }

    #[test]
    fn on_hat_activated_clears_matching_entries() {
        let mut tracker = HandoffTracker::new();
        tracker.on_handoff_accepted("work.ready", "executor", "evt-1", t(0));
        tracker.on_handoff_accepted("fix.plan.ready", "executor", "evt-2", t(0));
        tracker.on_handoff_accepted("work.ready", "executor", "evt-3", t(0));
        assert_eq!(tracker.pending_count(), 3);
        let cleared = tracker.on_hat_activated("executor");
        assert_eq!(cleared, 3);
        assert_eq!(tracker.pending_count(), 0);
    }

    #[test]
    fn on_hat_activated_keeps_other_consumers() {
        let mut tracker = HandoffTracker::new();
        tracker.on_handoff_accepted("work.ready", "executor", "evt-1", t(0));
        tracker.on_handoff_accepted("work.failed", "fixer", "evt-2", t(0));
        let cleared = tracker.on_hat_activated("executor");
        assert_eq!(cleared, 1);
        assert_eq!(tracker.pending_count(), 1);
    }

    #[test]
    fn expired_returns_escalations_and_clears_them() {
        let mut tracker = HandoffTracker::new();
        // t(0) is "now"; default timeout is 30s, so deadlines
        // land at t(30) and t(35).
        tracker.on_handoff_accepted("work.ready", "executor", "evt-1", t(0));
        tracker.on_handoff_accepted("work.failed", "fixer", "evt-2", t(5));
        // At t(40) both entries are past their deadlines.
        let escalations = tracker.expired(t(40));
        assert_eq!(escalations.len(), 2);
        // Each escalation names the original consumer as the
        // safe_target (executor / fixer are not the fallback).
        let by_topic: std::collections::HashMap<_, _> =
            escalations.iter().map(|e| (e.topic.clone(), e)).collect();
        assert_eq!(by_topic["work.ready"].safe_target, "executor");
        assert_eq!(by_topic["work.failed"].safe_target, "fixer");
        // Pending is empty after escalations are taken.
        assert_eq!(tracker.pending_count(), 0);
    }

    #[test]
    fn expired_does_not_touch_unexpired_entries() {
        let mut tracker = HandoffTracker::new();
        tracker.on_handoff_accepted("work.ready", "executor", "evt-1", t(0));
        // At t=5 the entry is not yet expired (deadline = t(30)).
        let escalations = tracker.expired(t(5));
        assert!(escalations.is_empty());
        assert_eq!(tracker.pending_count(), 1);
    }

    #[test]
    fn fallback_safe_target_used_when_consumer_matches() {
        let mut tracker = HandoffTracker::new();
        tracker.on_handoff_accepted("work.ready", "plan-gate", "evt-1", t(0));
        let escalations = tracker.expired(t(100));
        assert_eq!(escalations.len(), 1);
        // Plan-gate is the bottleneck → fall back to
        // review-coordinator.
        assert_eq!(escalations[0].safe_target, "review-coordinator");
    }

    #[test]
    fn custom_timeout_and_fallback_are_honored() {
        let mut tracker = HandoffTracker::new()
            .with_default_timeout(Duration::from_secs(60))
            .with_fallback_safe_target("custom-hat");
        tracker.on_handoff_accepted("work.ready", "custom-hat", "evt-1", t(0));
        let escalations = tracker.expired(t(120));
        assert_eq!(escalations.len(), 1);
        assert_eq!(escalations[0].safe_target, "review-coordinator");
    }

    #[test]
    fn escalations_are_sorted_by_topic_then_event_id() {
        let mut tracker = HandoffTracker::new();
        tracker.on_handoff_accepted("zeta", "a", "evt-2", t(0));
        tracker.on_handoff_accepted("alpha", "b", "evt-1", t(0));
        tracker.on_handoff_accepted("alpha", "c", "evt-3", t(0));
        let escalations = tracker.expired(t(100));
        assert_eq!(
            escalations
                .iter()
                .map(|e| (e.topic.clone(), e.event_id.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("alpha".to_string(), "evt-1".to_string()),
                ("alpha".to_string(), "evt-3".to_string()),
                ("zeta".to_string(), "evt-2".to_string()),
            ]
        );
    }

    #[test]
    fn cancel_pending_removes_entry() {
        let mut tracker = HandoffTracker::new();
        tracker.on_handoff_accepted("work.ready", "executor", "evt-1", t(0));
        assert_eq!(tracker.pending_count(), 1);
        assert!(tracker.cancel_pending("evt-1"));
        assert_eq!(tracker.pending_count(), 0);
        // 二次 cancel 不报错,只返回 false。
        assert!(!tracker.cancel_pending("evt-1"));
    }

    #[test]
    fn cancel_pending_does_not_touch_other_entries() {
        let mut tracker = HandoffTracker::new();
        tracker.on_handoff_accepted("work.ready", "executor", "evt-1", t(0));
        tracker.on_handoff_accepted("work.failed", "fixer", "evt-2", t(0));
        assert!(tracker.cancel_pending("evt-1"));
        assert_eq!(tracker.pending_count(), 1);
        assert!(!tracker.cancel_pending("evt-1"));
        // evt-2 仍然在,直到 expired() 或 on_hat_activated。
        let escalations = tracker.expired(t(100));
        assert_eq!(escalations.len(), 1);
        assert_eq!(escalations[0].event_id, "evt-2");
    }
}
