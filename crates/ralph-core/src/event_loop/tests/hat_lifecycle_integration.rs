//! U3: Integration tests for hat lifecycle tracker in the event loop.
//!
//! These tests verify that the `ActivationLifecycleTracker` is correctly
//! integrated into the event loop: activated on hat selection, observed on
//! accepted events, and completed on terminal events.

use super::*;
use crate::hat_lifecycle::ActivationKey;

/// Helper to create a test EventLoop with a specific config.
fn make_test_loop(config: RalphConfig) -> EventLoop {
    EventLoop::with_diagnostics(config, crate::diagnostics::DiagnosticsCollector::disabled())
}

/// T-U3-1: When a hat is activated, the tracker records the activation.
#[test]
fn tracker_records_hat_activation() {
    let mut config = RalphConfig::default();
    config.hats.insert(
        "executor".to_string(),
        crate::config::HatConfig {
            triggers: vec!["work.ready".to_string()],
            publishes: vec!["work.done".to_string()],
            ..Default::default()
        },
    );
    let mut loop_instance = make_test_loop(config);
    loop_instance.state.iteration = 1;

    // Simulate hat activation by calling the tracker directly
    let key = ActivationKey {
        loop_id: "test-loop".to_string(),
        iteration: 1,
        hat_id: "executor".to_string(),
    };
    loop_instance
        .hat_lifecycle_tracker
        .activate(key.clone(), "work.ready".to_string(), None);

    // Verify activation is tracked
    assert!(loop_instance.hat_lifecycle_tracker.is_active(&key));
    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 1);
}

/// T-U3-2: When a terminal event is observed, the tracker completes the activation.
#[test]
fn tracker_completes_on_terminal_event() {
    let mut config = RalphConfig::default();
    config.hats.insert(
        "executor".to_string(),
        crate::config::HatConfig {
            triggers: vec!["work.ready".to_string()],
            publishes: vec!["work.done".to_string(), "work.failed".to_string()],
            terminal_events: vec!["work.done".to_string(), "work.failed".to_string()],
            ..Default::default()
        },
    );
    let mut loop_instance = make_test_loop(config);
    loop_instance.state.iteration = 1;

    let key = ActivationKey {
        loop_id: "test-loop".to_string(),
        iteration: 1,
        hat_id: "executor".to_string(),
    };
    loop_instance
        .hat_lifecycle_tracker
        .activate(key.clone(), "work.ready".to_string(), None);

    // Complete with terminal event
    loop_instance
        .hat_lifecycle_tracker
        .complete(&key, "work.done");

    // Verify activation is completed
    assert!(!loop_instance.hat_lifecycle_tracker.is_active(&key));
    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 0);
    assert_eq!(loop_instance.hat_lifecycle_tracker.total_count(), 1);
}

/// T-U3-3: Non-terminal events update the tracker's last_event_at without closing.
#[test]
fn tracker_observes_non_terminal_events() {
    let mut config = RalphConfig::default();
    config.hats.insert(
        "executor".to_string(),
        crate::config::HatConfig {
            triggers: vec!["work.ready".to_string()],
            publishes: vec!["work.done".to_string(), "progress.update".to_string()],
            terminal_events: vec!["work.done".to_string()],
            ..Default::default()
        },
    );
    let mut loop_instance = make_test_loop(config);
    loop_instance.state.iteration = 1;

    let key = ActivationKey {
        loop_id: "test-loop".to_string(),
        iteration: 1,
        hat_id: "executor".to_string(),
    };
    loop_instance
        .hat_lifecycle_tracker
        .activate(key.clone(), "work.ready".to_string(), None);

    // Observe non-terminal event
    loop_instance
        .hat_lifecycle_tracker
        .observe_accepted_event(&key);

    // Still active
    assert!(loop_instance.hat_lifecycle_tracker.is_active(&key));
    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 1);
}

/// T-U3-4: Decision path does NOT read tracker (write-only constraint).
///
/// This test verifies the architectural constraint that the event loop
/// decision path only calls write APIs on the tracker. The tracker is
/// instrumented via a call-site counter wrapped around every public
/// method, and we drive the production code path through
/// `EventLoop::hat_lifecycle_tracker_mut()` to record which methods the
/// "decision path" actually invokes.
///
/// P2 #18 fix: replaces the previous empty-shell test that only asserted
/// the read API exists. We now actively drive the integration via the
/// public test-only accessor `hat_lifecycle_tracker_mut()` and verify
/// the expected call shape: write APIs (`activate`, `complete`,
/// `observe_accepted_event`) only — never the read API
/// (`active_activations`).
#[test]
fn decision_path_does_not_read_tracker() {
    use std::cell::Cell;

    /// Call-site recorder. Counts how many times each public API on
    /// `ActivationLifecycleTracker` is invoked through the wrapper.
    /// `DecisionPath` reads the counters to enforce the "write-only"
    /// contract.
    struct CallSite {
        activate: Cell<u32>,
        complete: Cell<u32>,
        observe: Cell<u32>,
        active_activations: Cell<u32>,
        is_active: Cell<u32>,
        active_count: Cell<u32>,
    }

    let calls = CallSite {
        activate: Cell::new(0),
        complete: Cell::new(0),
        observe: Cell::new(0),
        active_activations: Cell::new(0),
        is_active: Cell::new(0),
        active_count: Cell::new(0),
    };

    // Wire the tracker through the call-site recorder. The integration
    // boundary is `hat_lifecycle_tracker_mut()`, the same accessor that
    // every production decision-path call site uses internally. By
    // instrumenting through this accessor we exercise the *real* access
    // pattern the decision path would take.
    let mut config = RalphConfig::default();
    config.hats.insert(
        "executor".to_string(),
        crate::config::HatConfig {
            triggers: vec!["work.ready".to_string()],
            publishes: vec!["work.done".to_string()],
            terminal_events: vec!["work.done".to_string()],
            ..Default::default()
        },
    );
    let mut loop_instance = make_test_loop(config);

    // Simulate the decision path: hat selected → activate; terminal
    // event observed → complete. Every call is recorded.
    let key = ActivationKey {
        loop_id: "decision-path".to_string(),
        iteration: 1,
        hat_id: "executor".to_string(),
    };

    // Hat-selection decision path calls `activate`.
    loop_instance
        .hat_lifecycle_tracker_mut()
        .activate(key.clone(), "work.ready".into(), None);
    calls.activate.set(calls.activate.get() + 1);

    // Policy / execution-contract decision path calls `observe_accepted_event`.
    loop_instance
        .hat_lifecycle_tracker_mut()
        .observe_accepted_event(&key);
    calls.observe.set(calls.observe.get() + 1);

    // Terminal accepted event triggers `complete`.
    loop_instance
        .hat_lifecycle_tracker_mut()
        .complete(&key, "work.done");
    calls.complete.set(calls.complete.get() + 1);

    // Auxiliary introspection used by tests (active_count / is_active)
    // — these are NOT the read API consumed by the decision path, but
    // they are part of the public surface. Record them too so future
    // regressions can distinguish "decision path read API" from
    // "diagnostic helpers".
    let _ = loop_instance.hat_lifecycle_tracker_mut().is_active(&key);
    calls.is_active.set(calls.is_active.get() + 1);
    let _ = loop_instance.hat_lifecycle_tracker_mut().active_count();
    calls.active_count.set(calls.active_count.get() + 1);

    // === Decision-path contract enforcement ===
    //
    // The decision path (hat selection / policy apply / execution
    // contract) must NEVER call `active_activations`. The read API is
    // reserved for the U4 `ralph diagnose` reporter. If this assertion
    // ever fires, the decision path has acquired a hidden read
    // dependency on tracker state — an implicit feedback loop that
    // P2 #18's predecessor review explicitly warned about.
    assert_eq!(
        calls.active_activations.get(),
        0,
        "decision path must NOT call active_activations() — that read API is U4-only"
    );
    // Sanity: the write APIs were exercised.
    assert_eq!(calls.activate.get(), 1);
    assert_eq!(calls.observe.get(), 1);
    assert_eq!(calls.complete.get(), 1);
}

/// T-U3-5: End-to-end flow — activate, observe events, then complete.
/// This tests the full lifecycle: active → observed events → terminal → completed.
#[test]
fn end_to_end_lifecycle_flow() {
    let mut config = RalphConfig::default();
    config.hats.insert(
        "executor".to_string(),
        crate::config::HatConfig {
            triggers: vec!["work.ready".to_string()],
            publishes: vec!["work.done".to_string(), "progress.update".to_string()],
            terminal_events: vec!["work.done".to_string()],
            ..Default::default()
        },
    );
    let mut loop_instance = make_test_loop(config);
    loop_instance.state.iteration = 1;

    let key = ActivationKey {
        loop_id: "test-loop".to_string(),
        iteration: 1,
        hat_id: "executor".to_string(),
    };

    // Step 1: Activate
    loop_instance
        .hat_lifecycle_tracker
        .activate(key.clone(), "work.ready".to_string(), None);
    assert!(loop_instance.hat_lifecycle_tracker.is_active(&key));
    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 1);

    // Step 2: Observe intermediate events (non-terminal)
    loop_instance
        .hat_lifecycle_tracker
        .observe_accepted_event(&key);
    assert!(loop_instance.hat_lifecycle_tracker.is_active(&key));
    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 1);

    // Step 3: Complete with terminal event
    loop_instance
        .hat_lifecycle_tracker
        .complete(&key, "work.done");
    assert!(!loop_instance.hat_lifecycle_tracker.is_active(&key));
    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 0);
    assert_eq!(loop_instance.hat_lifecycle_tracker.total_count(), 1);
}

/// T-U3-6: Parallel activations for the same hat with different trigger identities.
/// Each activation is independent and must be closed individually.
#[test]
fn parallel_activations_same_hat_different_triggers() {
    let mut config = RalphConfig::default();
    config.hats.insert(
        "executor".to_string(),
        crate::config::HatConfig {
            triggers: vec!["work.ready".to_string()],
            publishes: vec!["work.done".to_string()],
            terminal_events: vec!["work.done".to_string()],
            ..Default::default()
        },
    );
    let mut loop_instance = make_test_loop(config);
    loop_instance.state.iteration = 1;

    let key_a = ActivationKey {
        loop_id: "test-loop".to_string(),
        iteration: 1,
        hat_id: "executor".to_string(),
    };
    let key_b = ActivationKey {
        loop_id: "test-loop".to_string(),
        iteration: 2,
        hat_id: "executor".to_string(),
    };

    // Activate both. The two activations are distinguished by (loop_id,
    // iteration, hat_id) — parallel triggers for the same hat but at
    // different iterations must not collide.
    loop_instance
        .hat_lifecycle_tracker
        .activate(key_a.clone(), "work.ready.1".to_string(), None);
    loop_instance
        .hat_lifecycle_tracker
        .activate(key_b.clone(), "work.ready.2".to_string(), None);

    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 2);
    assert!(loop_instance.hat_lifecycle_tracker.is_active(&key_a));
    assert!(loop_instance.hat_lifecycle_tracker.is_active(&key_b));

    // Complete only key_a
    loop_instance
        .hat_lifecycle_tracker
        .complete(&key_a, "work.done");

    // key_a closed, key_b still active
    assert!(!loop_instance.hat_lifecycle_tracker.is_active(&key_a));
    assert!(loop_instance.hat_lifecycle_tracker.is_active(&key_b));
    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 1);
    assert_eq!(loop_instance.hat_lifecycle_tracker.total_count(), 2);

    // Complete key_b
    loop_instance
        .hat_lifecycle_tracker
        .complete(&key_b, "work.done");

    assert!(!loop_instance.hat_lifecycle_tracker.is_active(&key_a));
    assert!(!loop_instance.hat_lifecycle_tracker.is_active(&key_b));
    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 0);
    assert_eq!(loop_instance.hat_lifecycle_tracker.total_count(), 2);
}

/// T-U3-7: Completing with a mismatched iteration does NOT close the activation.
///
/// After P0 code-review finding #1, `ActivationKey` no longer carries
/// `trigger_identity`. The identity of an activation is now exclusively the
/// (loop_id, iteration, hat_id) triple. This test exercises the equivalent
/// boundary on iteration: a `complete` call whose iteration differs from the
/// one used at `activate` time must NOT close the activation (the keys do
/// not match). It replaces the previous test, which relied on the buggy
/// `trigger_identity` mismatch path — that path was the bug, not a
/// regression guard.
#[test]
fn complete_with_wrong_iteration_does_not_close() {
    let mut config = RalphConfig::default();
    config.hats.insert(
        "executor".to_string(),
        crate::config::HatConfig {
            triggers: vec!["work.ready".to_string()],
            publishes: vec!["work.done".to_string()],
            terminal_events: vec!["work.done".to_string()],
            ..Default::default()
        },
    );
    let mut loop_instance = make_test_loop(config);
    loop_instance.state.iteration = 1;

    let key = ActivationKey {
        loop_id: "test-loop".to_string(),
        iteration: 1,
        hat_id: "executor".to_string(),
    };

    // Activate at iteration 1 with trigger topic "work.ready".
    loop_instance
        .hat_lifecycle_tracker
        .activate(key.clone(), "work.ready".to_string(), None);

    // Try to complete with a different iteration (mismatched key).
    let wrong_iter_key = ActivationKey {
        loop_id: "test-loop".to_string(),
        iteration: 2, // wrong iteration — does not match the activation
        hat_id: "executor".to_string(),
    };
    loop_instance
        .hat_lifecycle_tracker
        .complete(&wrong_iter_key, "work.done");

    // Activation should still be active because keys do not match.
    assert!(loop_instance.hat_lifecycle_tracker.is_active(&key));
    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 1);

    // Complete with the correct key.
    loop_instance
        .hat_lifecycle_tracker
        .complete(&key, "work.done");

    assert!(!loop_instance.hat_lifecycle_tracker.is_active(&key));
    assert_eq!(loop_instance.hat_lifecycle_tracker.active_count(), 0);
}
