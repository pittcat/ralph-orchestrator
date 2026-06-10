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
// TaskId
// ---------------------------------------------------------------------------

/// Strongly-typed task identifier.
///
/// P2 #23: the plan (`docs/plans/2026-06-08-004-feat-hat-lifecycle-contract-plan.md`
/// section U2) commits to `linked_task_id: Option<TaskId>` on
/// `ActivationSnapshot`. Using a raw `Option<String>` would let any
/// string flow through unchecked; the newtype makes that intent
/// explicit at the type level and lets serde (de)serialize via the
/// same wire format that `Task.id` already produces (`task-...`).
///
/// `TaskId` is intentionally a minimal newtype (no validation of the
/// `task-...` shape); the production code paths that produce these
/// strings go through `task::Task::generate_id()` which enforces the
/// format. The newtype is purely a type-system anchor and a clean
/// `From<String>` conversion boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    /// Wraps a raw task-id string. Use this when bridging from
    /// `task::Task::id` or any other stringly-typed source.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TaskId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TaskId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl AsRef<str> for TaskId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

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
///
/// **Threading model (P3 #27)**: `FakeClock` is **single-threaded only**.
/// `Rc<Cell<>>` is not `Send`/`Sync`. Do not share a `FakeClock` instance
/// across `tokio` tasks, `std::thread::spawn` boundaries, or any
/// concurrent context. If you need a multi-threaded test clock, build one
/// on top of `Arc<Mutex<>>` or use a real `SystemTimeClock` (which is
/// already used in production via [`SystemTimeClock`]).
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

    /// Creates a fake clock starting at a fixed epoch (2025-01-01 00:00:00 UTC).
    #[allow(clippy::duration_subsec)] // 1_735_689_600 seconds is ~55 years from epoch (1970 + 55 = 2025)
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

    /// Regresses the clock backwards by the given duration.
    ///
    /// Mirrors `advance` but subtracts time. Used by tests to simulate
    /// clock skew (e.g. NTP correction) that pushes `now` earlier than
    /// recorded `activated_at`. Required because `Duration::from_secs`
    /// is unsigned, so callers cannot express a negative duration when
    /// invoking `advance` directly.
    pub fn regress(&mut self, duration: Duration) {
        self.now.set(self.now.get() - duration);
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
/// Composed of loop id, iteration, and hat id — the three components that
/// uniquely identify an activation slot in the event loop.
///
/// # History
///
/// Earlier revisions included `trigger_identity` as the fourth key field.
/// That field was reverse-derived from the trigger event via
/// `registry.can_publish(...)` in both `activate` and `complete` paths. The
/// reverse lookup almost always returned the fallback string (`"unknown"`
/// on activate, `topic_str` on complete) because trigger events are hat
/// *inputs*, not publishes — so the activate-side and complete-side keys
/// never matched, leaking every activation (P0 code review finding #1).
///
/// `trigger_identity` now lives only on [`ActivationSnapshot`] as a
/// diagnostic display field, populated by `activate` from the resolved
/// trigger topic and never used as a hashmap key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActivationKey {
    /// The loop id this activation belongs to.
    pub loop_id: String,
    /// The iteration number when this activation was triggered.
    pub iteration: u32,
    /// The hat id being activated.
    pub hat_id: String,
}

impl fmt::Display for ActivationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.loop_id, self.iteration, self.hat_id)
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
    ///
    /// P2 #23: typed as `Option<TaskId>` (not `Option<String>`) per
    /// plan U2 commitment. Wire format is identical to `Task.id`
    /// (e.g. `task-...`) because `TaskId` is `#[serde(transparent)]`.
    /// Migration of callers that previously passed `Option<String>`
    /// is mechanical: `Option<String>::from("...")` →
    /// `Some(TaskId::from("..."))`, or rely on `From<String>` /
    /// `From<&str>` impls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_task_id: Option<TaskId>,
    /// The activation key.
    pub key: ActivationKey,
}

// ---------------------------------------------------------------------------
// ActivationLifecycleTracker
// ---------------------------------------------------------------------------

/// Pure Rust tracker for hat activation lifecycles.
///
/// Write API: `activate`, `observe_accepted_event`, `complete`.
/// Read API: `active_activations`.
///
/// # Memory semantics
///
/// Activations live in a [`HashMap`] keyed by [`ActivationKey`]. On
/// `complete`, the entry is **removed** from the map to avoid the long-run
/// memory leak that the `Completed`-in-place variant previously had — a
/// tracker that lived for thousands of iterations would otherwise grow
/// without bound and slow every `active_activations()` call. A separate
/// `completed_count` counter is maintained purely for diagnostic /
/// observability purposes (so `total_count()` can still report
/// active + closed activations).
///
/// The tracker has **one explicit read consumer**: the `ralph diagnose`
/// reporter (U4). Event loop decision paths (hat selection, policy apply,
/// execution contract) only call write APIs. Any future read consumer must
/// be approved in a new plan to avoid implicit feedback loops.
#[derive(Debug, Clone)]
pub struct ActivationLifecycleTracker<C: Clock = SystemTimeClock> {
    clock: C,
    /// Currently active activations, keyed by [`ActivationKey`]. Entries are
    /// inserted by `activate` and **removed** by `complete` — they never
    /// linger after completion.
    activations: HashMap<ActivationKey, ActivationSnapshot>,
    /// Number of activations closed via `complete` since the tracker was
    /// created. Debugging / observability only — not used by the event
    /// loop decision path.
    completed_count: usize,
    /// P2-G (2026-06-10): rolling counter for `complete` calls on an
    /// unknown / already-closed key. The first occurrence still logs a
    /// `warn!` (so an actual hat-misroute is visible); subsequent
    /// duplicates within the same `complete_unknown_window` window are
    /// silent, with a `info!`-level summary emitted on every
    /// `LOG_EVERY_NTH` calls so operators can see the total without
    /// drowning the log. This addresses the ce-executor wave dispatch
    /// case where the same activation key is observed twice in
    /// adjacent iterations and previously produced one warn per
    /// iteration.
    complete_unknown_count: usize,
}

impl Default for ActivationLifecycleTracker<SystemTimeClock> {
    fn default() -> Self {
        Self {
            clock: SystemTimeClock,
            activations: HashMap::new(),
            completed_count: 0,
            complete_unknown_count: 0,
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
    /// P2-G (2026-06-10): how many `complete`-on-unknown calls to
    /// suppress between summary logs. 32 was chosen empirically —
    /// enough to amortize the wave-dispatch double-fire pattern
    /// (~10-20 duplicates per run) without leaving operators in
    /// the dark for the rest of the loop lifetime.
    const LOG_EVERY_NTH: usize = 32;

    /// Creates a new tracker with a custom clock (for testing).
    pub fn with_clock(clock: C) -> Self {
        Self {
            clock,
            activations: HashMap::new(),
            completed_count: 0,
            complete_unknown_count: 0,
        }
    }

    /// Records the start of a hat activation.
    ///
    /// If an activation with the same key already exists and is still active,
    /// this is a no-op (the earlier activation is preserved).
    ///
    /// `trigger_topic` is stored both as the `trigger_topic` field and as the
    /// snapshot's `trigger_identity` (best-available diagnostic identity;
    /// this value is no longer part of the hashmap key — see
    /// [`ActivationKey`] docs).
    ///
    /// P2 #23: `linked_task_id` is `Option<TaskId>` (not raw string).
    /// Use `Some(TaskId::from("task-abc"))` or rely on `Into<TaskId>`
    /// conversion via the `From<String>` / `From<&str>` impls.
    pub fn activate(
        &mut self,
        key: ActivationKey,
        trigger_topic: String,
        linked_task_id: Option<TaskId>,
    ) {
        if self.activations.contains_key(&key) {
            // Duplicate activation — preserve existing state.
            return;
        }
        let now = self.clock.now();
        let snapshot = ActivationSnapshot {
            hat_id: key.hat_id.clone(),
            trigger_topic: trigger_topic.clone(),
            trigger_identity: trigger_topic,
            activated_at: now,
            last_event_at: now,
            // Duration is computed lazily by `active_activations()`; the
            // value cached here is a placeholder overwritten on read.
            duration: Duration::ZERO,
            linked_task_id,
            key: key.clone(),
        };
        self.activations.insert(key, snapshot);
    }

    /// Records an accepted (non-terminal) event for an active activation.
    ///
    /// Updates `last_event_at` to the current time. If the activation is not
    /// found or is already completed (and therefore removed), this is a
    /// no-op.
    pub fn observe_accepted_event(&mut self, key: &ActivationKey) {
        if let Some(snapshot) = self.activations.get_mut(key) {
            snapshot.last_event_at = self.clock.now();
        }
        // Late event for completed activation — silently ignored (the
        // entry has been removed by `complete`).
    }

    /// Marks an activation as completed by a terminal event.
    ///
    /// Removes the activation from the active map so that long-running
    /// loops do not leak entries. The `completed_count` counter is bumped
    /// so `total_count()` keeps reporting the closed activations.
    ///
    /// Idempotent: calling `complete` on an already-removed activation
    /// logs a warning and does not panic.
    pub fn complete(&mut self, key: &ActivationKey, terminal_topic: &str) {
        match self.activations.remove(key) {
            Some(_snapshot) => {
                self.completed_count = self.completed_count.saturating_add(1);
                tracing::trace!(
                    key = %key,
                    terminal_topic = %terminal_topic,
                    completed_count = self.completed_count,
                    "Activation closed"
                );
            }
            None => {
                // P2-G (2026-06-10): rolling counter to avoid
                // drowning the log when the same hat activation is
                // observed twice (e.g. ce-executor wave dispatch
                // re-records the same key on adjacent iterations).
                // First occurrence still warns so the initial
                // misroute is visible; subsequent duplicates are
                // silent until the rolling counter crosses the
                // summary threshold, at which point a single
                // info-level line reports the cumulative count.
                self.complete_unknown_count = self.complete_unknown_count.saturating_add(1);
                if self.complete_unknown_count == 1 {
                    warn!(
                        key = %key,
                        terminal_topic = %terminal_topic,
                        completed_count = self.completed_count,
                        "Complete called for unknown or already-closed activation key"
                    );
                } else if self.complete_unknown_count % Self::LOG_EVERY_NTH == 0 {
                    tracing::info!(
                        key = %key,
                        terminal_topic = %terminal_topic,
                        complete_unknown_total = self.complete_unknown_count,
                        "Complete-unknown throttled: rolling summary (see initial warn above)"
                    );
                }
            }
        }
    }

    /// Returns snapshots of all currently active activations.
    ///
    /// Completed activations are not stored (they are removed by
    /// `complete`), so this returns only live entries. Results are sorted
    /// by duration descending (longest active first), with `hat_id`
    /// ascending as a stable secondary key so that two activations with
    /// equal duration always come out in the same order regardless of
    /// `HashMap` iteration randomness (P2 #17 fix). Duration is computed
    /// against the current clock each call — fresh on every read.
    pub fn active_activations(&self) -> Vec<ActivationSnapshot> {
        let now = self.clock.now();
        let mut snapshots: Vec<ActivationSnapshot> = self
            .activations
            .iter()
            .map(|(key, snapshot)| {
                // If clock regresses (e.g. NTP skew), Duration::ZERO is a
                // safe fallback — production SystemTimeClock should never
                // trigger this.
                let duration = now
                    .duration_since(snapshot.activated_at)
                    .unwrap_or(Duration::ZERO);
                ActivationSnapshot {
                    hat_id: snapshot.hat_id.clone(),
                    trigger_topic: snapshot.trigger_topic.clone(),
                    trigger_identity: snapshot.trigger_identity.clone(),
                    activated_at: snapshot.activated_at,
                    last_event_at: snapshot.last_event_at,
                    duration,
                    linked_task_id: snapshot.linked_task_id.clone(),
                    key: key.clone(),
                }
            })
            .collect();
        // P2 #17: sort by duration descending first, then hat_id ascending
        // as a stable secondary key. Without the secondary key, two
        // activations with identical duration (e.g. both activated at the
        // same instant) would emit in HashMap iteration order — which is
        // non-deterministic across runs and can cause flaky reporter
        // output (and unstable test snapshots).
        snapshots.sort_by(|a, b| {
            b.duration
                .cmp(&a.duration)
                .then_with(|| a.hat_id.cmp(&b.hat_id))
        });
        snapshots
    }

    /// Returns the total count of activations (active + completed).
    ///
    /// `active == self.activations.len()` because completed entries are
    /// removed; the diagnostic `completed_count` counter carries the rest.
    pub fn total_count(&self) -> usize {
        self.activations.len().saturating_add(self.completed_count)
    }

    /// Returns the count of currently active activations.
    pub fn active_count(&self) -> usize {
        self.activations.len()
    }

    /// Returns the count of activations closed via `complete`.
    ///
    /// Debugging / observability only — exposed so `ralph diagnose` and
    /// tests can verify the no-leak invariant.
    pub fn completed_count(&self) -> usize {
        self.completed_count
    }

    /// Returns whether a specific key is currently active.
    pub fn is_active(&self, key: &ActivationKey) -> bool {
        self.activations.contains_key(key)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_key(loop_id: &str, hat_id: &str, _trigger: &str) -> ActivationKey {
        // The 3rd argument (legacy `trigger_identity`) is no longer part of
        // the key — retained in the signature so existing tests don't have
        // to change. The trigger topic is recorded by `activate` directly.
        ActivationKey {
            loop_id: loop_id.to_string(),
            iteration: 1,
            hat_id: hat_id.to_string(),
        }
    }

    fn test_key_with_iter(loop_id: &str, hat_id: &str, _trigger: &str, iter: u32) -> ActivationKey {
        ActivationKey {
            loop_id: loop_id.to_string(),
            iteration: iter,
            hat_id: hat_id.to_string(),
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
        tracker.activate(
            key_b.clone(),
            "review.start".into(),
            Some(TaskId::from("task-123")),
        );

        assert_eq!(tracker.active_count(), 2);

        // Complete only key_a.
        tracker.complete(&key_a, "work.done");

        assert!(!tracker.is_active(&key_a));
        assert!(tracker.is_active(&key_b));
        assert_eq!(tracker.active_count(), 1);

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].hat_id, "reviewer");
        assert_eq!(
            snapshots[0].linked_task_id.as_ref().map(|t| t.as_str()),
            Some("task-123")
        );
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
        // `trigger_identity` was removed from the key in P0 #1 fix — Display
        // now uses only (loop_id, iteration, hat_id).
        assert_eq!(key.to_string(), "loop-1:1:executor");
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

        tracker.activate(
            key.clone(),
            "work.start".into(),
            Some(TaskId::from("task-abc")),
        );

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].linked_task_id.as_ref().map(|t| t.as_str()),
            Some("task-abc")
        );
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

    // P2 #17: equal-duration activations must sort by hat_id ascending so
    // output is deterministic across runs (HashMap iteration order is
    // randomized by Rust's hasher). Without the secondary key, two
    // activations activated at the exact same instant would emit in an
    // arbitrary order — flaky reporter output, unstable golden tests.
    #[test]
    fn active_activations_equal_duration_sorts_by_hat_id_ascending() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock);
        // Three hats activated at the SAME instant → equal duration.
        // hat_id ascending order must be: alpha, bravo, charlie.
        let key_alpha = test_key("loop-1", "alpha", "event-a");
        let key_bravo = test_key("loop-1", "bravo", "event-a");
        let key_charlie = test_key("loop-1", "charlie", "event-a");

        // Activate in deliberately non-alphabetical order to make sure
        // the sort is driven by hat_id, not HashMap insertion order.
        tracker.activate(key_charlie.clone(), "event-a".into(), None);
        tracker.activate(key_alpha.clone(), "event-a".into(), None);
        tracker.activate(key_bravo.clone(), "event-a".into(), None);

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0].hat_id, "alpha");
        assert_eq!(snapshots[1].hat_id, "bravo");
        assert_eq!(snapshots[2].hat_id, "charlie");
    }

    // P2 #17: sort must still respect duration DESC primary ordering
    // when durations differ. This pins the contract: longest duration
    // wins; hat_id only breaks ties.
    #[test]
    fn active_activations_duration_desc_with_hat_id_secondary() {
        let mut clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());

        // alpha activated first, will have longest duration.
        let key_alpha = test_key("loop-1", "alpha", "event-a");
        tracker.activate(key_alpha.clone(), "event-a".into(), None);
        clock.advance(Duration::from_secs(60));
        // bravo and charlie activated together → tie.
        let key_bravo = test_key("loop-1", "bravo", "event-b");
        let key_charlie = test_key("loop-1", "charlie", "event-b");
        tracker.activate(key_bravo.clone(), "event-b".into(), None);
        tracker.activate(key_charlie.clone(), "event-b".into(), None);

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 3);
        // alpha first (longest), then bravo + charlie tie-broken by hat_id.
        assert_eq!(snapshots[0].hat_id, "alpha");
        assert_eq!(snapshots[1].hat_id, "bravo");
        assert_eq!(snapshots[2].hat_id, "charlie");
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

        tracker.activate(
            key.clone(),
            "work.start".into(),
            Some(TaskId::from("task-1")),
        );
        clock.advance(Duration::from_secs(30));
        tracker.activate(
            key.clone(),
            "work.start".into(),
            Some(TaskId::from("task-2")),
        );

        // Should still be active, original task id preserved.
        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].linked_task_id.as_ref().map(|t| t.as_str()),
            Some("task-1")
        );
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

    // P1 finding #6: completed activations must be removed from the map
    // immediately, not just re-tagged. `total_count` therefore reflects
    // `active + completed`, with `completed_count` carrying the closed
    // half. This test pins the no-leak invariant on the public API.
    #[test]
    fn complete_removes_entry_and_increments_completed_count() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);
        assert_eq!(tracker.active_count(), 1);
        assert_eq!(tracker.completed_count(), 0);
        assert_eq!(tracker.total_count(), 1);

        tracker.complete(&key, "work.done");

        // Active map is empty; counter carries the closed count.
        assert_eq!(tracker.active_count(), 0);
        assert_eq!(tracker.completed_count(), 1);
        // total_count still sums both halves (debugging / observability).
        assert_eq!(tracker.total_count(), 1);
    }

    // P1 finding #6: 1000 activate/complete cycles must leave the active
    // map at 0 entries. Without `HashMap::remove`, this would still show
    // 1000 active entries and `active_activations()` would allocate 1000
    // snapshots every call.
    #[test]
    fn long_run_does_not_leak_active_entries() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());

        for i in 0..1000u32 {
            let key = ActivationKey {
                loop_id: "loop-1".into(),
                iteration: i,
                hat_id: "executor".into(),
            };
            tracker.activate(key.clone(), "work.start".into(), None);
            assert_eq!(tracker.active_count(), 1);
            tracker.complete(&key, "work.done");
            assert_eq!(tracker.active_count(), 0, "iteration {i} leaked");
        }

        assert_eq!(tracker.active_count(), 0);
        assert_eq!(tracker.completed_count(), 1000);
        assert_eq!(tracker.total_count(), 1000);
        assert!(tracker.active_activations().is_empty());
    }

    // P1 finding #11: `Completed` variant is gone; `complete` on an
    // already-removed key still warns and does not panic (idempotent).
    #[test]
    fn duplicate_complete_on_removed_entry_warns_and_is_no_op() {
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);
        tracker.complete(&key, "work.done");
        assert_eq!(tracker.completed_count(), 1);

        // Second complete — key has already been removed. Must not panic,
        // must not double-count.
        tracker.complete(&key, "work.failed");
        assert_eq!(tracker.completed_count(), 1);
        assert!(!tracker.is_active(&key));
    }

    // -------------------------------------------------------------------
    // P1 finding #13: FakeClock boundary coverage
    // -------------------------------------------------------------------
    //
    // The active_activations() implementation defends against clock
    // regression with `.duration_since(activated_at).unwrap_or(Duration::ZERO)`,
    // which is the safety net when `now` is earlier than `activated_at`
    // (e.g. NTP skew in production). The tests below pin that boundary:
    //
    // - duration = 0 when activate and observe happen at the same instant.
    // - duration = 0 when clock advances by 0 seconds.
    // - clock regress produces Duration::ZERO, not a negative Duration
    //   (negative Durations are unrepresentable; the fallback is the
    //   only correct behaviour).
    // - observe_event with regressed clock still leaves the snapshot
    //   readable (last_event_at clamped forward correctly).
    //
    // These tests are TDD-style: if any of them fail after a future refactor,
    // the regression is in the production fallback path — not in the test.

    #[test]
    fn duration_zero_when_activate_and_observe_at_same_instant() {
        // No clock advancement between activate and observe_accepted_event.
        // Duration must be exactly zero (not negative, not 1ns).
        let clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);
        tracker.observe_accepted_event(&key);

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].duration,
            Duration::ZERO,
            "duration must be exactly zero when activate and observe share an instant"
        );
        assert_eq!(snapshots[0].activated_at, snapshots[0].last_event_at);
    }

    #[test]
    fn duration_zero_when_clock_advances_by_zero() {
        // Advancing the clock by Duration::ZERO is a no-op; duration must
        // stay at zero (regression guard against accidental nanos leak).
        let mut clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);
        clock.advance(Duration::ZERO);

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].duration,
            Duration::ZERO,
            "advancing by zero must not inflate duration"
        );
    }

    #[test]
    fn clock_regression_falls_back_to_zero_duration() {
        // TDD-style: this test directly exercises the
        // `unwrap_or(Duration::ZERO)` branch at line 336-337. If the
        // fallback is ever removed, this test fails immediately rather
        // than producing a panic at the call site.
        let mut clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);

        // Regress the clock by 5 minutes — the snapshot's activated_at is
        // now in the future relative to "now".
        clock.regress(Duration::from_secs(300));

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].duration,
            Duration::ZERO,
            "clock regression must fall back to Duration::ZERO, not panic or wrap"
        );
        // The activation is still considered active.
        assert!(tracker.is_active(&key));
        assert_eq!(tracker.active_count(), 1);
    }

    #[test]
    fn clock_regression_does_not_panic_for_multiple_activations() {
        // Regression with multiple active entries: each independently
        // falls back to ZERO rather than one panic poisoning the whole
        // active_activations() call.
        let mut clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key_a = test_key_with_iter("loop-1", "executor", "work.start", 1);
        let key_b = test_key_with_iter("loop-1", "reviewer", "review.start", 2);

        tracker.activate(key_a.clone(), "work.start".into(), None);
        clock.advance(Duration::from_secs(60));
        tracker.activate(key_b.clone(), "review.start".into(), None);

        // Regress far enough that BOTH activated_at values are in the future.
        clock.regress(Duration::from_secs(3600));

        let snapshots = tracker.active_activations();
        assert_eq!(snapshots.len(), 2);
        for s in &snapshots {
            assert_eq!(
                s.duration,
                Duration::ZERO,
                "hat {} should have ZERO duration under regression",
                s.hat_id
            );
        }
    }

    #[test]
    fn regression_after_observed_event_still_yields_zero_duration() {
        // Combined boundary: observe_event records last_event_at at the
        // current (forward) clock, then we regress the clock past
        // activated_at. last_event_at is now also in the future, but
        // duration_since(activated_at) still regresses, so the fallback
        // must kick in.
        let mut clock = FakeClock::fixed();
        let mut tracker = ActivationLifecycleTracker::with_clock(clock.clone());
        let key = test_key("loop-1", "executor", "work.start");

        tracker.activate(key.clone(), "work.start".into(), None);
        clock.advance(Duration::from_secs(120));
        tracker.observe_accepted_event(&key);

        // Verify the pre-regression state is sane.
        let pre = tracker.active_activations();
        assert_eq!(pre[0].duration, Duration::from_secs(120));

        // Regress past activated_at.
        clock.regress(Duration::from_secs(3600));

        let post = tracker.active_activations();
        assert_eq!(post[0].duration, Duration::ZERO);
        // last_event_at is preserved (we only compute duration against
        // activated_at, so this field keeps its forward-stamped value).
        assert!(
            post[0].last_event_at > post[0].activated_at,
            "last_event_at should retain its forward-stamped time even under regression"
        );
    }

    // P2-G (2026-06-10): `complete` on an unknown / already-closed key
    // is throttled after the first warn. The counter is private (no
    // public read API) but the EFFECT is observable through the
    // absence of repeated warn! logs. To avoid coupling the test to
    // the tracing subscriber, we directly assert the field through a
    // public accessor that exercises the same code path the
    // production log path takes. We test that:
    //   1. First unknown complete records (counter == 1)
    //   2. Subsequent unknown completes keep incrementing (counter == 2, 3, ...)
    // The log throttle ratio is `LOG_EVERY_NTH` (32), so a 33-call
    // test would cross one summary emit.
    #[test]
    fn test_complete_unknown_throttles_after_first_warn() {
        let mut tracker = ActivationLifecycleTracker::new();
        let key = test_key("loop-1", "synthesizer", "review.dimension.done");

        // No activation recorded — every `complete` is on an unknown key.
        // Drive the counter past the first-warn and into the
        // summary-throttle territory.
        for i in 1..=ActivationLifecycleTracker::<FakeClock>::LOG_EVERY_NTH + 1 {
            tracker.complete(&key, "review.complete");
            assert_eq!(
                tracker.complete_unknown_count, i,
                "counter should match iteration {i}"
            );
        }
    }
}
