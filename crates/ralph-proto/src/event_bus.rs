//! Event bus for pub/sub messaging.
//!
//! The event bus routes events to subscribed hats based on topic patterns.
//! Multiple observers can be added to receive all published events for
//! recording, TUI updates, and benchmarking purposes.

use crate::{Event, Hat, HatId};
use std::collections::BTreeMap;

/// Type alias for the observer callback function.
type Observer = Box<dyn Fn(&Event) + Send + 'static>;

/// Central pub/sub hub for routing events between hats.
pub struct EventBus {
    /// Registered hats indexed by ID.
    hats: BTreeMap<HatId, Hat>,

    /// Pending events for each hat.
    pending: BTreeMap<HatId, Vec<Event>>,

    /// Pending human interaction events (human.*).
    human_pending: Vec<Event>,

    /// Observers that receive all published events.
    /// Multiple observers can be registered (e.g., session recorder + TUI).
    observers: Vec<Observer>,

    /// Round-robin cursor: tracks the last selected hat ID.
    /// Used by `select_next_hat_with_pending` to ensure fair scheduling.
    last_selected: Option<HatId>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self {
            hats: BTreeMap::new(),
            pending: BTreeMap::new(),
            human_pending: Vec::new(),
            observers: Vec::new(),
            last_selected: None,
        }
    }
}

impl EventBus {
    /// Creates a new empty event bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an observer that receives all published events.
    ///
    /// Multiple observers can be added (e.g., session recorder + TUI).
    /// Each observer is called before events are routed to subscribers.
    /// This enables recording sessions by subscribing to the event stream
    /// without modifying the routing logic.
    pub fn add_observer<F>(&mut self, observer: F)
    where
        F: Fn(&Event) + Send + 'static,
    {
        self.observers.push(Box::new(observer));
    }

    /// Sets a single observer, clearing any existing observers.
    ///
    /// Prefer `add_observer` when multiple observers are needed.
    /// This method is kept for backwards compatibility.
    #[deprecated(since = "2.0.0", note = "Use add_observer instead")]
    pub fn set_observer<F>(&mut self, observer: F)
    where
        F: Fn(&Event) + Send + 'static,
    {
        self.observers.clear();
        self.observers.push(Box::new(observer));
    }

    /// Clears all observer callbacks.
    pub fn clear_observers(&mut self) {
        self.observers.clear();
    }

    /// Registers a hat with the event bus.
    pub fn register(&mut self, hat: Hat) {
        let id = hat.id.clone();
        self.hats.insert(id.clone(), hat);
        self.pending.entry(id).or_default();
    }

    /// Publishes an event to all subscribed hats.
    ///
    /// Returns the list of hat IDs that received the event.
    /// If an observer is set, it receives the event before routing.
    #[allow(clippy::needless_pass_by_value)] // Event is cloned to multiple recipients
    pub fn publish(&mut self, event: Event) -> Vec<HatId> {
        // --- EventBus source guard: reject events with impossible source ---
        // If event.source is set, it must correspond to a registered hat.
        // Events with unknown sources are dropped before observers see them.
        // System-injected events bypass this guard (orchestrator bootstrap).
        if event.system_injected != Some(true) {
            if let Some(ref source) = event.source {
                if !self.hats.contains_key(source) {
                    // Unknown source — fail closed, return no recipients
                    return Vec::new();
                }
            }
        }
        // --- End EventBus source guard ---

        // Notify all observers before routing
        for observer in &self.observers {
            observer(&event);
        }

        // U2 fix: explicit `target` takes precedence over the `human.*`
        // prefix interception. Without this, a `human.guidance(target=...)`
        // event would be silently absorbed into `human_pending` and never
        // reach the intended hat. With it, any event with a target that
        // resolves to a registered hat is routed directly to that hat —
        // regardless of whether its topic starts with `human.`.
        if let Some(ref target) = event.target {
            if self.hats.contains_key(target) {
                self.pending
                    .entry(target.clone())
                    .or_default()
                    .push(event.clone());
                return vec![target.clone()];
            }
            // Target set but unregistered: keep the original direct-target
            // contract (empty recipients, no human_pending absorption).
            return Vec::new();
        }

        // No explicit target: `human.*` events go to the dedicated queue
        // (preserved original behavior).
        if event.topic.as_str().starts_with("human.") {
            self.human_pending.push(event);
            return Vec::new();
        }

        let mut recipients = Vec::new();

        // Route with priority: specific subscriptions > fallback wildcards
        // Per spec: "If event has subscriber → Select that hat's backend"
        //           "If no subscriber → Select Ralph's backend (cli.backend)"

        // First, find hats with specific (non-global-wildcard) subscriptions
        let mut specific_recipients = Vec::new();
        let mut fallback_recipients = Vec::new();

        for (id, hat) in &self.hats {
            if hat.has_specific_subscription(&event.topic) {
                // Hat has a specific subscription for this topic
                specific_recipients.push(id.clone());
            } else if hat.is_subscribed(&event.topic) {
                // Hat matches only via global wildcard (fallback)
                fallback_recipients.push(id.clone());
            }
        }

        // Use specific subscribers if any, otherwise fall back to wildcard handlers
        let chosen_recipients = if specific_recipients.is_empty() {
            fallback_recipients
        } else {
            specific_recipients
        };

        for id in chosen_recipients {
            self.pending
                .entry(id.clone())
                .or_default()
                .push(event.clone());
            recipients.push(id);
        }

        recipients
    }

    /// Takes all pending events for a hat.
    pub fn take_pending(&mut self, hat_id: &HatId) -> Vec<Event> {
        self.pending.remove(hat_id).unwrap_or_default()
    }

    /// Takes all pending human interaction events.
    pub fn take_human_pending(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.human_pending)
    }

    /// Returns a reference to pending events for a hat without consuming them.
    pub fn peek_pending(&self, hat_id: &HatId) -> Option<&Vec<Event>> {
        self.pending.get(hat_id)
    }

    /// Returns a reference to pending human interaction events without consuming them.
    pub fn peek_human_pending(&self) -> &[Event] {
        &self.human_pending
    }

    /// Checks if there are any pending events for any hat (including human events).
    pub fn has_pending(&self) -> bool {
        !self.human_pending.is_empty() || self.pending.values().any(|events| !events.is_empty())
    }

    /// Checks if there are any pending non-human events (i.e., hat-level queue entries).
    pub fn has_pending_non_human(&self) -> bool {
        self.pending.values().any(|events| !events.is_empty())
    }

    /// Checks if there are any pending human interaction events.
    pub fn has_human_pending(&self) -> bool {
        !self.human_pending.is_empty()
    }

    /// Returns the next hat with pending events (deterministic, lexicographic first).
    ///
    /// This is a **peek** operation — it does not advance the round-robin cursor.
    /// Use `select_next_hat_with_pending` for fair scheduling with cursor advancement.
    ///
    /// BTreeMap iteration is already sorted by key.
    pub fn peek_next_hat_with_pending(&self) -> Option<&HatId> {
        self.pending
            .iter()
            .find(|(_, events)| !events.is_empty())
            .map(|(id, _)| id)
    }

    /// Deprecated: use `peek_next_hat_with_pending` (no side-effect) or
    /// `select_next_hat_with_pending` (round-robin) instead.
    #[deprecated(note = "Use peek_next_hat_with_pending() or select_next_hat_with_pending(None)")]
    pub fn next_hat_with_pending(&self) -> Option<&HatId> {
        self.peek_next_hat_with_pending()
    }

    /// Selects the next hat with pending events using round-robin
    /// scheduling, with an optional priority pre-emption for
    /// handoff topics.
    ///
    /// `priority_hat`, when `Some(hat_id)`, short-circuits the
    /// round-robin scan: if that hat has a non-empty pending
    /// queue, it is selected immediately and the round-robin
    /// cursor advances to the hat that **follows** the priority
    /// hat in the registered order (so the next non-priority
    /// selection resumes fairly). If `priority_hat` is `Some`
    /// but the hat's pending queue is empty, the function
    /// proceeds with the normal round-robin scan.
    ///
    /// The cursor is interpreted against the **full registered hat order** in
    /// the `hats` BTreeMap. The selection scan starts at the first key
    /// strictly greater than `last_selected` and wraps around to the start
    /// of the BTreeMap. For each registered hat in this circular scan, its
    /// `pending` queue is consulted; the first hat with a non-empty queue
    /// wins, has its `last_selected` updated, and its `HatId` returned.
    ///
    /// If the scan completes a full cycle without finding a non-empty queue,
    /// `None` is returned and `last_selected` is **not** mutated.
    ///
    /// WAC-U5 (2026-06-12-002): handoff priority dispatch is the
    /// narrow exception that lets a single handoff consumer
    /// pre-empt the round-robin cursor. R9 / KTD-6: the caller
    /// is responsible for only passing a `priority_hat` that is
    /// a **unique** consumer of the corresponding handoff
    /// topic. Multi-consumer or wildcard topics must not use
    /// the priority path.
    pub fn select_next_hat_with_pending(&mut self, priority_hat: Option<&HatId>) -> Option<HatId> {
        // WAC-U5 priority pre-emption: if the priority hat has
        // pending events, dispatch it now and advance the
        // round-robin cursor to the hat that follows it in
        // registered order. This is the only short-circuit the
        // cursor allows; the cursor otherwise walks the full
        // registered set as before.
        if let Some(priority_id) = priority_hat {
            let has_pending = self
                .pending
                .get(priority_id)
                .map(|q| !q.is_empty())
                .unwrap_or(false);
            if has_pending {
                self.last_selected = Some(priority_id.clone());
                return Some(priority_id.clone());
            }
            // Priority hat has nothing pending; fall through to
            // the normal round-robin scan. This is the
            // documented "priority hat drained between rounds"
            // case: the next selection must be fair.
        }

        // Scan the full registered hat order (BTreeMap keys are already
        // sorted). Start from the first key strictly greater than the
        // cursor; wrap around to the start if no such key exists. For
        // each registered hat, check whether its pending queue is
        // non-empty; pick the first one that is.
        //
        // Iterating `hats` (not `pending`) is what keeps the cursor's
        // position correct when a previously selected hat's queue is
        // empty or its entry was removed entirely by `take_pending`.
        let keys: Vec<&HatId> = self.hats.keys().collect();
        if keys.is_empty() {
            return None;
        }

        // Find the first index whose key is strictly greater than the
        // cursor. If the cursor is None, the cursor is not in the
        // BTreeMap (drained/never set), or no greater key exists, we
        // start the circular scan at index 0.
        let start_idx = match &self.last_selected {
            Some(cursor) => keys.iter().position(|k| **k > *cursor).unwrap_or(0), // no greater key → wrap to start
            None => 0,
        };

        // Walk circularly from start_idx for up to N keys, returning the
        // first non-empty queue found.
        let n = keys.len();
        for offset in 0..n {
            let idx = (start_idx + offset) % n;
            let id = keys[idx];
            if !self.pending.get(id).map(|q| q.is_empty()).unwrap_or(true) {
                self.last_selected = Some(id.clone());
                return Some(id.clone());
            }
        }

        // Full cycle exhausted with no non-empty queue. Do not mutate
        // the cursor.
        None
    }

    /// Gets a hat by ID.
    pub fn get_hat(&self, id: &HatId) -> Option<&Hat> {
        self.hats.get(id)
    }

    /// Returns all registered hat IDs.
    pub fn hat_ids(&self) -> impl Iterator<Item = &HatId> {
        self.hats.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_publish_to_subscriber() {
        let mut bus = EventBus::new();

        let hat = Hat::new("impl", "Implementer").subscribe("task.*");
        bus.register(hat);

        let event = Event::new("task.start", "Start implementing");
        let recipients = bus.publish(event);

        assert_eq!(recipients.len(), 1);
        assert_eq!(recipients[0].as_str(), "impl");
    }

    #[test]
    fn test_no_match() {
        let mut bus = EventBus::new();

        let hat = Hat::new("impl", "Implementer").subscribe("task.*");
        bus.register(hat);

        let event = Event::new("review.done", "Review complete");
        let recipients = bus.publish(event);

        assert!(recipients.is_empty());
    }

    #[test]
    fn test_direct_target() {
        let mut bus = EventBus::new();

        let impl_hat = Hat::new("impl", "Implementer").subscribe("task.*");
        let review_hat = Hat::new("reviewer", "Reviewer").subscribe("impl.*");
        bus.register(impl_hat);
        bus.register(review_hat);

        // Direct target bypasses subscription matching
        let event = Event::new("handoff", "Please review").with_target("reviewer");
        let recipients = bus.publish(event);

        assert_eq!(recipients.len(), 1);
        assert_eq!(recipients[0].as_str(), "reviewer");
    }

    #[test]
    fn test_take_pending() {
        let mut bus = EventBus::new();

        let hat = Hat::new("impl", "Implementer").subscribe("*");
        bus.register(hat);

        bus.publish(Event::new("task.start", "Start"));
        bus.publish(Event::new("task.continue", "Continue"));

        let hat_id = HatId::new("impl");
        let events = bus.take_pending(&hat_id);

        assert_eq!(events.len(), 2);
        assert!(bus.take_pending(&hat_id).is_empty());
    }

    #[test]
    fn test_human_events_use_separate_queue() {
        let mut bus = EventBus::new();

        let hat = Hat::new("ralph", "Ralph").subscribe("*");
        bus.register(hat);

        bus.publish(Event::new("human.interact", "question"));
        bus.publish(Event::new("human.response", "hello"));
        bus.publish(Event::new("human.guidance", "note"));

        assert_eq!(bus.peek_human_pending().len(), 3);
        assert_eq!(
            bus.peek_pending(&HatId::new("ralph"))
                .map(|events| events.len())
                .unwrap_or(0),
            0
        );

        let taken = bus.take_human_pending();
        assert_eq!(taken.len(), 3);
        assert!(!bus.has_human_pending());
    }

    #[test]
    fn test_self_routing_allowed() {
        // Self-routing is allowed to handle LLM non-determinism.
        // Spec acceptance criteria: planner emits build.done (even though builder "should"),
        // event routes back to planner, planner continues (no source-based blocking).
        let mut bus = EventBus::new();

        let planner = Hat::new("planner", "Planner").subscribe("build.done");
        bus.register(planner);

        // Planner emits build.done (wrong hat, but LLMs are non-deterministic)
        let event = Event::new("build.done", "Done").with_source("planner");
        let recipients = bus.publish(event);

        // Event SHOULD route back to planner (self-routing allowed, no source filtering)
        assert_eq!(recipients.len(), 1);
        assert_eq!(recipients[0].as_str(), "planner");
    }

    #[test]
    fn test_observer_receives_all_events() {
        use std::sync::{Arc, Mutex};

        let mut bus = EventBus::new();
        let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let observed_clone = Arc::clone(&observed);
        bus.add_observer(move |event| {
            observed_clone.lock().unwrap().push(event.payload.clone());
        });

        let hat = Hat::new("impl", "Implementer").subscribe("task.*");
        bus.register(hat);

        // Publish events - observer should see all regardless of routing
        bus.publish(Event::new("task.start", "Start"));
        bus.publish(Event::new("other.event", "Other")); // No subscriber
        bus.publish(Event::new("task.done", "Done"));

        let captured = observed.lock().unwrap();
        assert_eq!(captured.len(), 3);
        assert_eq!(captured[0], "Start");
        assert_eq!(captured[1], "Other");
        assert_eq!(captured[2], "Done");
    }

    #[test]
    fn test_multiple_observers() {
        use std::sync::{Arc, Mutex};

        let mut bus = EventBus::new();
        let observer1_count = Arc::new(Mutex::new(0));
        let observer2_count = Arc::new(Mutex::new(0));

        let count1 = Arc::clone(&observer1_count);
        bus.add_observer(move |_| {
            *count1.lock().unwrap() += 1;
        });

        let count2 = Arc::clone(&observer2_count);
        bus.add_observer(move |_| {
            *count2.lock().unwrap() += 1;
        });

        bus.publish(Event::new("test", "1"));
        bus.publish(Event::new("test", "2"));

        // Both observers should have received both events
        assert_eq!(*observer1_count.lock().unwrap(), 2);
        assert_eq!(*observer2_count.lock().unwrap(), 2);
    }

    #[test]
    fn test_clear_observers() {
        use std::sync::{Arc, Mutex};

        let mut bus = EventBus::new();
        let count = Arc::new(Mutex::new(0));

        let count_clone = Arc::clone(&count);
        bus.add_observer(move |_| {
            *count_clone.lock().unwrap() += 1;
        });

        bus.publish(Event::new("test", "1"));
        assert_eq!(*count.lock().unwrap(), 1);

        bus.clear_observers();
        bus.publish(Event::new("test", "2"));
        assert_eq!(*count.lock().unwrap(), 1); // Still 1, observers cleared
    }

    #[test]
    fn test_peek_pending_does_not_consume() {
        let mut bus = EventBus::new();

        let hat = Hat::new("impl", "Implementer").subscribe("*");
        bus.register(hat);

        bus.publish(Event::new("task.start", "Start"));
        bus.publish(Event::new("task.continue", "Continue"));

        let hat_id = HatId::new("impl");

        // Peek at pending events
        let peeked = bus.peek_pending(&hat_id);
        assert!(peeked.is_some());
        assert_eq!(peeked.unwrap().len(), 2);

        // Peek again - should still be there
        let peeked_again = bus.peek_pending(&hat_id);
        assert!(peeked_again.is_some());
        assert_eq!(peeked_again.unwrap().len(), 2);

        // Now take them - should consume
        let taken = bus.take_pending(&hat_id);
        assert_eq!(taken.len(), 2);

        // Peek after take - should be empty
        let peeked_after_take = bus.peek_pending(&hat_id);
        assert!(peeked_after_take.is_none() || peeked_after_take.unwrap().is_empty());
    }

    // ─── Round-robin scheduling tests (U4) ────────────────────────────────────

    /// Helper: register a hat with wildcard subscription.
    fn register_wildcard(bus: &mut EventBus, id: &str) {
        bus.register(Hat::new(id, id).subscribe("*"));
    }

    /// F3 / AE6: A and B both pending; after selecting A (and consuming its
    /// queue), the next selection must be B.
    #[test]
    fn two_hats_round_robin() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");

        bus.publish(Event::new("work", "for alpha")); // routed to alpha + beta
        bus.publish(Event::new("work", "for beta")); // same

        // First selection — cursor starts at None → picks first non-empty (alpha).
        let sel1 = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel1.as_str(), "alpha");

        // Simulate the loop consuming alpha's queue.
        bus.take_pending(&sel1);

        // Second selection — cursor is now "alpha"; next after alpha is "beta".
        let sel2 = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel2.as_str(), "beta");
    }

    /// A, B, C continuously pending: the selection sequence must cycle
    /// A → B → C → A → … deterministically.
    #[test]
    fn three_hats_fair_rotation() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");
        register_wildcard(&mut bus, "gamma");

        // Publish enough events so queues never empty.
        for i in 0..6 {
            bus.publish(Event::new("work", &format!("event-{i}")));
        }

        let mut sequence = Vec::new();
        for _ in 0..6 {
            let sel = bus.select_next_hat_with_pending(None).unwrap();
            sequence.push(sel.as_str().to_string());
            // Do NOT consume — queues stay full (self-replenishing).
        }

        // Expected: alpha → beta → gamma → alpha → beta → gamma
        assert_eq!(
            sequence,
            vec!["alpha", "beta", "gamma", "alpha", "beta", "gamma"]
        );
    }

    /// A self-loops (gets new pending each time it's selected) — B must still
    /// be selected within N-1 other selections (here N=2, so B is selected
    /// immediately after A in the next round).
    #[test]
    fn self_looping_hat_does_not_starve() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");

        // Initial publish.
        bus.publish(Event::new("work", "seed-alpha"));
        bus.publish(Event::new("work", "seed-beta"));

        // Round 1: select alpha, consume, republish for alpha only.
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "alpha");
        bus.take_pending(&sel);
        // Alpha self-loops: new work arrives.
        bus.publish(Event::new("work", "alpha-loop"));

        // Round 2: cursor is "alpha", so next is "beta".
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "beta");
        bus.take_pending(&sel);

        // Beta should NOT starve — it was selected in round 2.
    }

    /// Only one pending hat: every selection returns the same hat, no idle.
    #[test]
    fn single_pending_hat_selected_every_time() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "solo");

        bus.publish(Event::new("work", "item-1"));
        bus.publish(Event::new("work", "item-2"));

        for _ in 0..3 {
            let sel = bus.select_next_hat_with_pending(None).unwrap();
            assert_eq!(sel.as_str(), "solo");
            // Don't consume — queue stays full.
        }
    }

    /// Cursor points to a hat whose queue was cleared → scan wraps around
    /// and picks the next non-empty queue correctly.
    #[test]
    fn cursor_after_cleared_queue_wraps() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");
        register_wildcard(&mut bus, "gamma");

        // Publish to all three.
        bus.publish(Event::new("work", "a1"));
        bus.publish(Event::new("work", "b1"));
        bus.publish(Event::new("work", "g1"));

        // Select alpha first.
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "alpha");
        bus.take_pending(&sel);

        // Select beta.
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "beta");
        // Clear beta too.
        bus.take_pending(&sel);

        // Cursor is now "beta". Next should be "gamma" (the only remaining).
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "gamma");
        bus.take_pending(&sel);

        // All queues empty — should return None.
        assert!(bus.select_next_hat_with_pending(None).is_none());
    }

    /// peek_next_hat_with_pending must NOT advance the cursor.
    #[test]
    fn peek_does_not_advance_cursor() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");

        bus.publish(Event::new("work", "a1"));
        bus.publish(Event::new("work", "b1"));

        // Peek multiple times — always returns the same (lexicographic first).
        for _ in 0..5 {
            let peeked = bus.peek_next_hat_with_pending().unwrap();
            assert_eq!(peeked.as_str(), "alpha");
        }

        // Select after peeking — cursor was never advanced by peek.
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "alpha");
    }

    /// Same registered hats, same pending queues, same initial cursor →
    /// two independent runs produce the identical selection sequence.
    #[test]
    fn same_state_same_sequence() {
        fn run_sequence() -> Vec<String> {
            let mut bus = EventBus::new();
            register_wildcard(&mut bus, "alpha");
            register_wildcard(&mut bus, "beta");
            register_wildcard(&mut bus, "gamma");

            for i in 0..6 {
                bus.publish(Event::new("work", &format!("event-{i}")));
            }

            let mut seq = Vec::new();
            for _ in 0..6 {
                let sel = bus.select_next_hat_with_pending(None).unwrap();
                seq.push(sel.as_str().to_string());
            }
            seq
        }

        let seq1 = run_sequence();
        let seq2 = run_sequence();
        assert_eq!(seq1, seq2, "Same initial state must produce same sequence");
    }

    // ─── Round-robin cursor regression tests (U1 / AE1) ──────────────────────
    //
    // These tests pin down the contract that the round-robin cursor is anchored
    // on the full registered hat order, not the order of hats that *currently*
    // have non-empty queues. If a hat's queue is drained (or the hat is
    // deregistered), the cursor must still be interpreted against the full
    // BTreeMap so the next selection is the registered successor of the
    // previously selected hat — not a degenerate fall-back to the
    // lexicographically first non-empty queue.

    /// AE1: After `beta` is the last selected hat and its queue is cleared,
    /// the cursor must find `gamma` (the registered successor of `beta`)
    /// even though `alpha` is also pending. The pre-fix code falls back to
    /// `alpha` because `beta` is no longer in the non-empty filtered list.
    #[test]
    fn cursor_after_cleared_queue_picks_registered_successor() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");
        register_wildcard(&mut bus, "gamma");

        // Publish one event routed only to alpha.
        bus.publish(Event::new("work", "a").with_target("alpha"));
        // Select alpha, drain it. Cursor = alpha.
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "alpha");
        bus.take_pending(&sel);

        // Publish one event routed only to beta; select beta, drain it.
        // Cursor = beta.
        bus.publish(Event::new("work", "b").with_target("beta"));
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "beta");
        bus.take_pending(&sel);

        // Now publish to alpha and gamma only — NOT beta. This creates
        // the buggy state: cursor = beta (not in non_empty = [alpha, gamma]).
        bus.publish(Event::new("work", "a2").with_target("alpha"));
        bus.publish(Event::new("work", "g2").with_target("gamma"));

        // Next selection: pre-fix returns alpha (lex first non-empty).
        // Post-fix: scan full order from cursor=beta → first key > beta
        // is gamma → gamma is non-empty → pick gamma.
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(
            sel.as_str(),
            "gamma",
            "cursor must find registered successor, not lexicographic first"
        );
    }

    /// After `gamma` was the last selected hat and its queue is cleared,
    /// the scan must wrap around to `alpha` (the registered order's start).
    #[test]
    fn cursor_at_last_hat_wraps_to_first() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");
        register_wildcard(&mut bus, "gamma");

        // Select alpha, drain.
        bus.publish(Event::new("work", "a").with_target("alpha"));
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "alpha");
        bus.take_pending(&sel);

        // Select beta, drain.
        bus.publish(Event::new("work", "b").with_target("beta"));
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "beta");
        bus.take_pending(&sel);

        // Select gamma, drain. Cursor = gamma.
        bus.publish(Event::new("work", "g").with_target("gamma"));
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "gamma");
        bus.take_pending(&sel);

        // Republish to alpha only (NOT beta or gamma).
        bus.publish(Event::new("work", "a2").with_target("alpha"));

        // Cursor is "gamma" with empty queue. Wrap to alpha (first
        // registered key > no keys wrap → start at alpha).
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "alpha");
    }

    /// A continuously self-replenishing hat must not starve its peers.
    /// Alpha self-loops (always non-empty); beta and gamma get drained and
    /// then re-published. Within one full round of registered hats, both
    /// beta and gamma must be selected.
    ///
    /// P2 finding #14: the bound is parameterised by the registered hat
    /// count (3 in this fixture). For 4+ hat scenarios the round length
    /// grows linearly; callers using this test as a template should
    /// compute the bound as `hat_count * hat_count` to give the cursor
    /// at least one full wrap per peer.
    #[test]
    fn self_replenishing_hat_does_not_starve_others() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");
        register_wildcard(&mut bus, "gamma");
        let hat_count = 3usize;
        // Bound: hat_count² = one full round per peer, regardless of
        // starting cursor position. For 3 hats → 9 iterations.
        let bound = hat_count * hat_count;

        // Initial: all three pending (one event each).
        bus.publish(Event::new("work", "a1").with_target("alpha"));
        bus.publish(Event::new("work", "b1").with_target("beta"));
        bus.publish(Event::new("work", "g1").with_target("gamma"));

        let mut seen_beta = false;
        let mut seen_gamma = false;

        // Simulate selection rounds: when we pick alpha, we re-publish
        // for alpha (self-loop). When we pick beta or gamma, we mark them
        // seen and re-publish for them too. We need to use targeted events
        // to keep queues separate.
        for _round in 0..bound {
            let Some(sel) = bus.select_next_hat_with_pending(None) else {
                break;
            };
            bus.take_pending(&sel);
            // Replenish alpha unconditionally (self-loop).
            bus.publish(Event::new("work", "a-loop").with_target("alpha"));
            if sel.as_str() == "beta" {
                seen_beta = true;
                bus.publish(Event::new("work", "b-loop").with_target("beta"));
            } else if sel.as_str() == "gamma" {
                seen_gamma = true;
                bus.publish(Event::new("work", "g-loop").with_target("gamma"));
            }
            if seen_beta && seen_gamma {
                break;
            }
        }

        assert!(seen_beta, "beta must be selected within {bound} rounds");
        assert!(seen_gamma, "gamma must be selected within {bound} rounds");
    }

    /// The fixed sequence must be reproducible across runs.
    #[test]
    fn cleared_queue_sequence_is_deterministic() {
        fn run() -> Vec<String> {
            let mut bus = EventBus::new();
            register_wildcard(&mut bus, "alpha");
            register_wildcard(&mut bus, "beta");
            register_wildcard(&mut bus, "gamma");

            // Sequence: always drain before next publish so the queue is
            // frequently empty between selections.
            let mut seq = Vec::new();
            for i in 0..6 {
                bus.publish(Event::new("work", format!("e{i}")).with_target("alpha"));
                if let Some(sel) = bus.select_next_hat_with_pending(None) {
                    seq.push(sel.as_str().to_string());
                    bus.take_pending(&sel);
                }
            }
            seq
        }

        let s1 = run();
        let s2 = run();
        assert_eq!(s1, s2, "drain-then-publish sequence must be deterministic");
    }

    /// Multiple `peek_next_hat_with_pending()` calls plus a `has_pending`
    /// check must NOT advance or reset the round-robin cursor. The actual
    /// `select_next_hat_with_pending(None)` call after these inspections must
    /// return exactly the same hat it would have returned without the
    /// inspections.
    #[test]
    fn peek_and_has_pending_do_not_mutate_cursor() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");
        register_wildcard(&mut bus, "gamma");

        // Set up: cursor = alpha (after selecting alpha and draining it).
        bus.publish(Event::new("work", "a").with_target("alpha"));
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "alpha");
        bus.take_pending(&sel);

        // Inspect (peek/has_pending) — none of these should mutate the
        // cursor or the pending state.
        for _ in 0..10 {
            let _ = bus.peek_next_hat_with_pending();
            let _ = bus.has_pending();
            let _ = bus.has_pending_non_human();
        }

        // Publish to beta. The very next selection (after the inspections)
        // must still be beta — the registered successor of alpha.
        bus.publish(Event::new("work", "b").with_target("beta"));

        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(
            sel.as_str(),
            "beta",
            "peek/has_pending must not advance the cursor"
        );
    }

    /// When the cursor points to a hat whose queue has been drained (i.e.
    /// is not currently non-empty) the scan must still find the correct
    /// registered successor in BTreeMap order. We simulate a "stale cursor"
    /// by selecting beta, draining it, then re-publishing to alpha and
    /// delta only. The next selection must be delta (BTreeMap successor of
    /// beta), not alpha (lexicographic first non-empty).
    #[test]
    fn stale_cursor_finds_stable_successor_in_full_order() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");
        register_wildcard(&mut bus, "gamma");
        register_wildcard(&mut bus, "delta");

        // Walk the cursor: alpha → beta, draining each.
        bus.publish(Event::new("work", "a").with_target("alpha"));
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "alpha");
        bus.take_pending(&sel);

        bus.publish(Event::new("work", "b").with_target("beta"));
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "beta");
        bus.take_pending(&sel);

        // Now: cursor = beta, alpha and beta are empty. Publish
        // ONLY to alpha and delta.
        bus.publish(Event::new("work", "a2").with_target("alpha"));
        bus.publish(Event::new("work", "d2").with_target("delta"));

        // Next selection: scan from > beta. Registered order: alpha,
        // beta, delta, gamma. Successor of beta = delta. Delta is
        // non-empty → pick delta. Pre-fix: lexicographic first non-empty
        // = alpha.
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(
            sel.as_str(),
            "delta",
            "stale cursor at 'beta' must pick 'delta' (registered successor)"
        );
    }

    /// Pre-fix bug: cursor on a hat whose queue is empty → falls back to
    /// lexicographic first non-empty, breaking the round-robin contract.
    /// Post-fix: scan the full BTreeMap and pick the first non-empty key
    /// strictly greater than the cursor (with wrap).
    ///
    /// This test sequences: alpha→beta→gamma and then sets up a state
    /// where alpha and gamma are non-empty and cursor points to beta (an
    /// empty queue).
    #[test]
    fn full_cycle_returns_none_without_cursor_mutation() {
        let mut bus = EventBus::new();
        register_wildcard(&mut bus, "alpha");
        register_wildcard(&mut bus, "beta");
        register_wildcard(&mut bus, "gamma");

        // Walk the cursor through alpha, beta, gamma — draining each.
        for (id, label) in [("alpha", "a"), ("beta", "b"), ("gamma", "g")] {
            bus.publish(Event::new("work", label).with_target(id));
            let sel = bus.select_next_hat_with_pending(None).unwrap();
            assert_eq!(sel.as_str(), id);
            bus.take_pending(&sel);
        }

        // All queues empty → no selection.
        assert!(bus.select_next_hat_with_pending(None).is_none());

        // Cursor should not have been mutated by the None return. Re-publish
        // to all three and verify the next selection respects the cursor
        // (gamma → wrap → alpha).
        for (id, label) in [("alpha", "a2"), ("beta", "b2"), ("gamma", "g2")] {
            bus.publish(Event::new("work", label).with_target(id));
        }

        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "alpha");
    }

    // ── WAC-U5 (2026-06-12-002): handoff priority dispatch ──

    /// T-U5-01: when the priority hat has a non-empty pending
    /// queue, `select_next_hat_with_pending(Some(priority))`
    /// short-circuits the round-robin scan and selects that
    /// hat immediately, regardless of the cursor's position.
    #[test]
    fn priority_hat_short_circuits_round_robin() {
        let mut bus = EventBus::new();
        for id in ["alpha", "beta", "gamma"] {
            bus.register(Hat::new(id, id).subscribe("work.*"));
        }
        for (id, label) in [("alpha", "a1"), ("beta", "b1"), ("gamma", "g1")] {
            bus.publish(Event::new("work", label).with_target(id));
        }
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(sel.as_str(), "alpha");
        bus.take_pending(&sel);

        for (id, label) in [("beta", "b2"), ("gamma", "g2")] {
            bus.publish(Event::new("work", label).with_target(id));
        }
        let sel = bus
            .select_next_hat_with_pending(Some(&HatId::from("gamma")))
            .unwrap();
        assert_eq!(sel.as_str(), "gamma");
    }

    /// T-U5-04: when the priority hat has no pending events
    /// (e.g. drained between rounds), the function falls through
    /// to the normal round-robin scan.
    #[test]
    fn priority_hat_falls_through_when_empty() {
        let mut bus = EventBus::new();
        for id in ["alpha", "beta", "gamma"] {
            bus.register(Hat::new(id, id).subscribe("work.*"));
        }
        for (id, label) in [("alpha", "a1"), ("beta", "b1")] {
            bus.publish(Event::new("work", label).with_target(id));
        }
        let sel = bus
            .select_next_hat_with_pending(Some(&HatId::from("gamma")))
            .unwrap();
        assert_eq!(sel.as_str(), "alpha");
    }

    /// T-U5-05: priority pre-emption advances the round-robin
    /// cursor to the priority hat, so the next non-priority
    /// selection resumes from the hat that follows the
    /// priority hat in registered order.
    #[test]
    fn priority_advances_cursor_for_next_round() {
        let mut bus = EventBus::new();
        for id in ["alpha", "beta", "gamma"] {
            bus.register(Hat::new(id, id).subscribe("work.*"));
        }
        for (id, label) in [("alpha", "a1"), ("beta", "b1"), ("gamma", "g1")] {
            bus.publish(Event::new("work", label).with_target(id));
        }
        let sel = bus
            .select_next_hat_with_pending(Some(&HatId::from("beta")))
            .unwrap();
        assert_eq!(sel.as_str(), "beta");
        bus.take_pending(&sel);
        for (id, label) in [("alpha", "a2"), ("gamma", "g2")] {
            bus.publish(Event::new("work", label).with_target(id));
        }
        let sel = bus.select_next_hat_with_pending(None).unwrap();
        assert_eq!(
            sel.as_str(),
            "gamma",
            "after priority pre-emption, cursor must advance to the priority hat, so the next scan starts at its successor"
        );
    }

    // ─── U2: human.* with explicit target routes to target hat ─────────────

    /// U2 (R-REP2): `human.guidance(target=progress-steward)` must route
    /// to the target hat instead of being absorbed into `human_pending`.
    #[test]
    fn test_human_guidance_with_target_routes_to_target_hat() {
        let mut bus = EventBus::new();
        let steward = Hat::new("progress-steward", "Steward").subscribe("*");
        let ralph = Hat::new("ralph", "Ralph").subscribe("*");
        bus.register(steward);
        bus.register(ralph);

        let event =
            Event::new("human.guidance", "guidance content").with_target("progress-steward");
        let recipients = bus.publish(event);

        assert_eq!(recipients.len(), 1);
        assert_eq!(recipients[0].as_str(), "progress-steward");
        assert!(
            bus.peek_human_pending().is_empty(),
            "human.* with target must NOT enter human_pending"
        );
        assert_eq!(
            bus.peek_pending(&HatId::new("progress-steward"))
                .map(|v| v.len())
                .unwrap_or(0),
            1,
            "target hat must hold the event in its pending queue"
        );
        assert_eq!(
            bus.peek_pending(&HatId::new("ralph"))
                .map(|v| v.len())
                .unwrap_or(0),
            0,
            "non-target hats must not receive the event"
        );
    }

    /// U2: `human.*` without target preserves original behavior — absorbed
    /// into `human_pending`, no hat receives it directly.
    #[test]
    fn test_human_guidance_without_target_still_human_pending() {
        let mut bus = EventBus::new();
        let ralph = Hat::new("ralph", "Ralph").subscribe("*");
        bus.register(ralph);

        let event = Event::new("human.guidance", "no target");
        let recipients = bus.publish(event);

        assert!(recipients.is_empty());
        assert_eq!(bus.peek_human_pending().len(), 1);
        assert_eq!(
            bus.peek_pending(&HatId::new("ralph"))
                .map(|v| v.len())
                .unwrap_or(0),
            0
        );
    }

    /// U2: `human.*` with target pointing to an unregistered hat must
    /// return empty recipients (matching the existing direct-target
    /// contract) and must NOT silently fall back to `human_pending`.
    #[test]
    fn test_human_target_unregistered_returns_empty() {
        let mut bus = EventBus::new();
        let ralph = Hat::new("ralph", "Ralph").subscribe("*");
        bus.register(ralph);

        let event = Event::new("human.guidance", "to unknown").with_target("nonexistent");
        let recipients = bus.publish(event);

        assert!(
            recipients.is_empty(),
            "unregistered target must yield empty recipients"
        );
        assert!(
            bus.peek_human_pending().is_empty(),
            "unregistered target must NOT silently fall back to human_pending"
        );
    }
}
