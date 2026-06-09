//! Activation lifecycle tracker for hat activations.
//!
//! This module implements a pure Rust state machine that tracks the lifecycle
//! of each hat activation — from `activate` through `observe_accepted_event`
//! to `complete`. The tracker exposes a read-only query API consumed by
//! `ralph diagnose` reporter; event loop decision paths only call write APIs.
//!
//! # Design Principles
//!
//! - **No I/O**: the tracker only records state; it does not write to disk,
//!   emit events, or interact with the EventBus.
//! - **Idempotent**: duplicate `complete` calls on the same key do not panic
//!   and are logged as warnings. Late events for completed activations are
//!   silently ignored.
//! - **Clock injection**: time is abstracted via the [`Clock`] trait so tests
//!   can use a fake clock while production uses `SystemTime::now()`.
//!
//! # Integration Boundary
//!
//! The tracker has **one explicit read consumer**: the `ralph diagnose`
//! reporter (U4). Event loop decision paths (hat selection, policy apply,
//! execution contract) only call write APIs. Any future read consumer must
//! be approved in a new plan to avoid implicit feedback loops.

use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, SystemTime};
use tracing::warn;

// ---------------------------------------------------------------------------
// Clock trait
// ---------------------------------------------------------------------------

/// Abstraction over time source for testability.
///
/// Production uses [`SystemTimeClock`] which wraps `SystemTime::now()`.
/// Tests inject a [`FakeClock`] that advances manually.
pub trait Clock {
    /// Returns the current point in time.
    fn now(&self) -> SystemTime;
}

/// Production clock using `SystemTime::now()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemTimeClock;

impl Clock for SystemTimeClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Fake clock for testing — advances only when explicitly told to.
///
/// Uses `Rc<Cell<>>` internally so that cloned instances share the same
/// time state. This is critical: when the test advances the clock, the
/// tracker's internal clock must also see the advance.
pub struct FakeClock {
    now: Rc<Cell<SystemTime>>,
}

impl FakeClock {
    /// Creates a fake clock starting at the given time.
    pub fn at(time: SystemTime) -> Self {
        Self {
            now: Rc::new(Cell::new(time)),
        }
    }

    /// Creates a fake clock starting at a fixed epoch (2026-01-01 00:00:00 UTC).
    #[allow(clippy::duration_subsec)] // 1_735_689_600 seconds is ~55 years, days not more readable
    pub fn fixed() -> Self {
        Self {
            now: Rc::new(Cell::new(
                std::time::UNIX_EPOCH + Duration::from_secs(1_735_689_600),
            )),
        }
    }

    /// Advances the clock by the given duration.
    pub fn advance(&mut self, duration: Duration) {
        self.now.set(self.now.get() + duration);
    }
}

impl Clone for FakeClock {
    fn clone(&self) -> Self {
        Self {
            now: Rc::clone(&self.now),
        }
    }
}

impl fmt::Debug for FakeClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeClock")
            .field("now", &self.now.get())
            .finish()
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        self.now.get()
    }
}

// ---------------------------------------------------------------------------
// ActivationKey
// ---------------------------------------------------------------------------

/// Unique identifier for a hat activation.
///
/// Composed of loop id, iteration, hat id, and trigger event identity to
/// guarantee parallel activations never collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActivationKey {
    /// The loop id this activation belongs to.
    pub loop_id: String,
    /// The iteration number when this activation was triggered.
    pub iteration: u32,
    /// The hat id being activated.
    pub hat_id: String,
    /// Identity of the trigger event (topic + optional instance key).
    pub trigger_identity: String,
}

impl fmt::Display for ActivationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}:{}",
            self.loop_id, self.iteration, self.hat_id, self.trigger_identity
        )
    }
}

// ---------------------------------------------------------------------------
// ActivationSnapshot (read-only query result)
// ---------------------------------------------------------------------------

/// Read-only snapshot of an active hat activation.
///
/// Returned by [`ActivationLifecycleTracker::active_activations`].
/// Defined here (not in diagnosis/) to avoid reverse dependencies — the
/// diagnosis reporter consumes this type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivationSnapshot {
    /// The hat id being activated.
    pub hat_id: String,
    /// Topic of the event that triggered this activation.
    pub trigger_topic: String,
    /// Identity of the trigger event.
    pub trigger_identity: String,
    /// When this activation started.
    pub activated_at: SystemTime,
    /// When the last accepted event was observed.
    pub last_event_at: SystemTime,
    /// Duration since activation started (real-time calculated).
    pub duration: Duration,
    /// Associated task id, if any.
    pub linked_task_id: Option<String>,
    /// The activation key.
    pub key: ActivationKey,
}

// ---------------------------------------------------------------------------
// ActivationState (internal)
// ---------------------------------------------------------------------------

/// Internal state of a single activation.
#[derive(Debug, Clone)]
enum ActivationState {
    Active {
        /// When the activation started.
        activated_at: SystemTime,
        /// Topic of the trigger event.
        trigger_topic: String,
        /// Identity of the trigger event.
        trigger_identity: String,
        /// When the last accepted event was observed.
        last_event_at: SystemTime,
        /// Associated task id, if any.
        linked_task_id: Option<String>,
        /// The hat id.
        hat_id: String,
    },
    Completed {
        /// When the activation completed.
        #[allow(dead_code)] // stored for potential diagnose reporter use
        completed_at: SystemTime,
        /// The terminal topic that closed the activation.
        terminal_topic: String,
    },
}

// ---------------------------------------------------------------------------
// ActivationLifecycleTracker
// ---------------------------------------------------------------------------

/// Pure Rust tracker for hat activation lifecycles.
///
/// Write API: `activate`, `observe_accepted_event`, `complete`.
/// Read API: `active_activations`.
#[derive(Debug, Clone)]
pub struct ActivationLifecycleTracker<C: Clock = SystemTimeClock> {
    clock: C,
    activations: HashMap<ActivationKey, ActivationState>,
}

impl Default for ActivationLifecycleTracker<SystemTimeClock> {
    fn default() -> Self {
        Self {
            clock: SystemTimeClock,
            activations: HashMap::new(),
        }
    }
}

impl ActivationLifecycleTracker<SystemTimeClock> {
    /// Creates a new tracker with the system clock.
    pub fn new() -> Self {
        Self::default()
    }
}

impl<C: Clock> ActivationLifecycleTracker<C> {
    /// Creates a new tracker with a custom clock (for testing).
    pub fn with_clock(clock: C) -> Self {
        Self {
            clock,
            activations: HashMap::new(),
        }
    }

    /// Records the start of a hat activation.
    ///
    /// If an activation with the same key already exists and is still active,
    /// this is a no-op (the earlier activation is preserved).
    pub fn activate(
        &mut self,
        key: ActivationKey,
        trigger_topic: String,
        linked_task_id: Option<String>,
    ) {
        if self.activations.contains_key(&key) {
            // Duplicate activation — preserve existing state.
            return;
        }
        let now = self.clock.now();
        self.activations.insert(
            key.clone(),
            ActivationState::Active {
                activated_at: now,
                trigger_topic,
                trigger_identity: key.trigger_identity.clone(),
                last_event_at: now,
                linked_task_id,
                hat_id: key.hat_id.clone(),
            },
        );
    }

    /// Records an accepted (non-terminal) event for an active activation.
    ///
    /// Updates `last_event_at` to the current time. If the activation is not
    /// found or is already completed, this is a no-op.
    pub fn observe_accepted_event(&mut self, key: &ActivationKey) {
        if let Some(ActivationState::Active {
            last_event_at,
            ..
        }) = self.activations.get_mut(key)
        {
            *last_event_at = self.clock.now();
        }
        // Late event for completed activation — silently ignored.
    }

    /// Marks an activation as completed by a terminal event.
    ///
    /// Idempotent: calling `complete` on an already-completed activation
    /// logs a warning and does not panic.
    pub fn complete(&mut self, key: &ActivationKey, terminal_topic: &str) {
        match self.activations.get_mut(key) {
            Some(ActivationState::Active { .. }) => {
                let now = self.clock.now();
                *self.activations.get_mut(key).unwrap() = ActivationState::Completed {
                    completed_at: now,
                    terminal_topic: terminal_topic.to_string(),
                };
            }
            Some(ActivationState::Completed {
                terminal_topic: prev,
                ..
            }) => {
                warn!(
                    key = %key,
                    previous_terminal = %prev,
                    new_terminal = %terminal_topic,
                    "Duplicate complete call on already-completed activation"
                );
            }
            None => {
                warn!(
                    key = %key,
                    terminal_topic = %terminal_topic,
                    "Complete called for unknown activation key"
                );
            }
        }
    }

    /// Returns snapshots of all currently active activations.
    ///
    /// Completed activations are excluded. Results are sorted by duration
    /// descending (longest active first).
    pub fn active_activations(&self) -> Vec<ActivationSnapshot> {
        let now = self.clock.now();
        let mut snapshots: Vec<ActivationSnapshot> = self
            .activations
            .iter()
            .filter_map(|(key, state)| match state {
                ActivationState::Active {
                    activated_at,
                    trigger_topic,
                    trigger_identity,
                    last_event_at,
                    linked_task_id,
                    hat_id,
                } => {
                    let duration = now.duration_since(*activated_at).unwrap_or_default();
                    Some(ActivationSnapshot {
                        hat_id: hat_id.clone(),
                        trigger_topic: trigger_topic.clone(),
                        trigger_identity: trigger_identity.clone(),
                        activated_at: *activated_at,
                        last_event_at: *last_event_at,
                        duration,
                        linked_task_id: linked_task_id.clone(),
                        key: key.clone(),
                    })
                }
                ActivationState::Completed { .. } => None,
            })
            .collect();
        // Sort by duration descending (longest active first).
        snapshots.sort_by_key(|s| std::cmp::Reverse(s.duration));
        snapshots
    }

    /// Returns the total count of activations (active + completed).
    pub fn total_count(&self) -> usize {
        self.activations.len()
    }

    /// Returns the count of currently active activations.
    pub fn active_count(&self) -> usize {
        self.activations
            .values()
            .filter(|s| matches!(s, ActivationState::Active { .. }))
            .count()
    }

    /// Returns whether a specific key is currently active.
    pub fn is_active(&self, key: &ActivationKey) -> bool {
        matches!(
            self.activations.get(key),
            Some(ActivationState::Active { .. })
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_key(loop_id: &str, hat_id: &str, trigger: &str) -> ActivationKey {
        ActivationKey {
            loop_id: loop_id.to_string(),
            iteration: 1,
            hat_id: hat_id.to_string(),
            trigger_identity: trigger.to_string(),
        }
    }

    fn test_key_with_iter(loop_id: &str, hat_id: &str, trigger: &str, iter: u32) -> ActivationKey {
        ActivationKey {
            loop_id: loop_id.to_string(),
            iteration: iter,
            hat_id: hat_id.to_string(),
            trigger_identity: trigger.to_string(),
        }
    }

    // T-U2-1: active 后收到中间 event 记录时间戳，不关闭
    #[test]
    fn observe_event_updates_timestamp_without_closing() {
        let mut clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);
        assert!(tracker.is_active(&key));
        assert_eq!(tracker.active_count(), 1);

        // Advance time and observe an event.
        clock.advance(Duration::from_secs(60));
        tracker.observe_accepted_event(&key);

        // Still active, duration updated.
        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots[0].duration >= Duration::from_secs(60));
        assert!(tracker.is_active(&key));
    }

    // T-U2-2: 任一 terminal event 关闭 activation
    #[test]
    fn terminal_event_closes_activation() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);
        assert!(tracker.is_active(&key));

        tracker.complete(&key, "work.done");

        // No longer active.
        assert!(!tracker.is_active(&key));
        assert_eq!(tracker.active_count(), 0);
        assert_eq!(tracker.active_activations().len(), 0);
        // Total count still includes completed.
        assert_eq!(tracker.total_count(), 1);
    }

    // T-U2-3: 并行 activation（不同 key）的状态互不污染
    #[test]
    fn parallel_activations_are_independent() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key_a = test_key("loop-1", "executor", "work.start");
        let key_b = test_key("loop-1", "reviewer", "review.start");

        tracker.activate(key_a.clone(), "work.start".into(), None);
        tracker.activate(key_b.clone(), "review.start".into(), Some("task-123".into()));

        assert_eq!(tracker.active_count(), 2);

        // Complete only key_a.
        tracker.complete(&key_a, "work.done");

        assert!(!tracker.is_active(&key_a));
        assert!(tracker.is_active(&key_b));
        assert_eq!(tracker.active_count(), 1);

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].hat_id, "reviewer");
        assert_eq!(snapshots[0].linked_task_id.as_deref(), Some("task-123"));
    }

    // T-U2-4: 被拒 event 不调用 observe API，不进入 tracker
    #[test]
    fn rejected_event_does_not_enter_tracker() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        // Only activate, don't observe any rejected event.
        tracker.activate(key.clone(), "work.start".into(), None);

        // The tracker only knows about the activation we explicitly created.
        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].hat_id, "executor");
    }

    // T-U2-5: active_activations() 返回当前 active 的快照，不含 completed
    #[test]
    fn active_activations_excludes_completed() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key_a = test_key("loop-1", "executor", "work.start");
        let key_b = test_key("loop-1", "reviewer", "review.start");

        tracker.activate(key_a.clone(), "work.start".into(), None);
        tracker.activate(key_b.clone(), "review.start".into(), None);
        tracker.complete(&key_a, "work.done");

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].hat_id, "reviewer");
    }

    // T-U2-6: completed activation 立即从 active 集合移除
    #[test]
    fn completed_activation_removed_immediately() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);
        assert_eq!(tracker.active_count(), 1);

        tracker.complete(&key, "work.done");
        assert_eq!(tracker.active_count(), 0);
        assert!(tracker.active_activations().is_empty());
    }

    // T-U2-7: 重复 complete 同 key 幂等不 panic
    #[test]
    fn duplicate_complete_is_idempotent() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);
        tracker.complete(&key, "work.done");

        // Second complete should not panic.
        tracker.complete(&key, "work.failed");
        assert!(!tracker.is_active(&key));
    }

    // T-U2-8: late event 对已 complete 的 activation 不修改状态
    #[test]
    fn late_event_ignored_for_completed_activation() {
        let mut clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);
        tracker.complete(&key, "work.done");

        // Late event — should be silently ignored.
        clock.advance(Duration::from_secs(120));
        tracker.observe_accepted_event(&key);

        // Still not active.
        assert!(!tracker.is_active(&key));
        assert_eq!(tracker.active_count(), 0);
    }

    // Additional: ActivationKey Display
    #[test]
    fn activation_key_display() {
        let key = test_key("loop-1", "executor", "work.start");
        assert_eq!(key.to_string(), "loop-1:1:executor:work.start");
    }

    // Additional: empty tracker returns empty snapshots
    #[test]
    fn empty_tracker_returns_empty() {
        let tracker = ActivationLifecycleTracker::<FakeClock>::with_clock(FakeClock::fixed());
        assert!(tracker.active_activations().is_empty());
        assert_eq!(tracker.active_count(), 0);
        assert_eq!(tracker.total_count(), 0);
    }

    // Additional: activate with linked_task_id
    #[test]
    fn activate_stores_linked_task_id() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), Some("task-abc".into()));

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].linked_task_id.as_deref(), Some("task-abc"));
    }

    // Additional: activate without linked_task_id
    #[test]
    fn activate_without_linked_task_id() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots[0].linked_task_id.is_none());
    }

    // Additional: complete on unknown key logs warning (no panic)
    #[test]
    fn complete_on_unknown_key_does_not_panic() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "unknown", "event");

        // Should not panic.
        tracker.complete(&key, "work.done");
    }

    // Additional: observe event on unknown key does not panic
    #[test]
    fn observe_event_on_unknown_key_does_not_panic() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "unknown", "event");

        // Should not panic.
        tracker.observe_accepted_event(&key);
    }

    // Additional: activation ordering by duration
    #[test]
    fn active_activations_sorted_by_duration_descending() {
        let mut clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key_a = test_key("loop-1", "fast-hat", "event-a");
        let key_b = test_key("loop-1", "slow-hat", "event-b");

        tracker.activate(key_a.clone(), "event-a".into(), None);
        clock.advance(Duration::from_secs(10));
        tracker.activate(key_b.clone(), "event-b".into(), None);

        // key_a has been active longer.
        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].hat_id, "fast-hat");
        assert_eq!(snapshots[1].hat_id, "slow-hat");
    }

    // Additional: duplicate activate is no-op (preserves existing)
    #[test]
    fn duplicate_activate_preserves_existing() {
        let mut clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), Some("task-1".into()));
        clock.advance(Duration::from_secs(30));
        tracker.activate(key.clone(), "work.start".into(), Some("task-2".into()));

        // Should still be active, original task id preserved.
        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].linked_task_id.as_deref(), Some("task-1"));
    }

    // Additional: multiple activations same hat different iterations
    #[test]
    fn same_hat_different_iterations_are_independent() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key1 = test_key_with_iter("loop-1", "executor", "work.start", 1);
        let key2 = test_key_with_iter("loop-1", "executor", "work.start", 2);

        tracker.activate(key1.clone(), "work.start".into(), None);
        tracker.activate(key2.clone(), "work.start".into(), None);

        assert_eq!(tracker.active_count(), 2);

        tracker.complete(&key1, "work.done");
        assert_eq!(tracker.active_count(), 1);
        assert!(tracker.is_active(&key2));
    }
}
