//! Tests for hat_exhaustion.

use super::*;

#[test]
fn test_hat_max_activations_emits_exhausted_event() {
    // Repro for issue #66: per-hat max_activations should prevent infinite reviewer loops.
    // Events are now published directly to the bus (simulating what ralph emit writes to JSONL
    // and process_events_from_jsonl publishes).
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    description: "Implements requested changes"
    triggers: ["work.start", "review.changes_requested"]
    publishes: ["implementation.done"]
  code_reviewer:
    name: "Code Reviewer"
    description: "Reviews changes and requests fixes"
    triggers: ["implementation.done"]
    publishes: ["review.changes_requested"]
    max_activations: 3
  escalator:
    name: "Escalator"
    description: "Handles exhausted hats"
    triggers: ["code_reviewer.exhausted"]
    publishes: []
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let ralph = HatId::new("ralph");

    // Seed the loop with an executor event.
    event_loop
        .bus
        .publish(Event::new("work.start", "begin").with_source(ralph.clone()));

    // Cycle: executor -> implementation.done; reviewer -> review.changes_requested.
    for _ in 0..3 {
        // Executor active.
        let _ = event_loop.build_prompt(&ralph).unwrap();
        // Simulate event from JSONL (ralph emit writes to file, process_events_from_jsonl publishes)
        event_loop
            .bus
            .publish(Event::new("implementation.done", "done"));

        // Reviewer active (up to max_activations=3).
        let prompt = event_loop.build_prompt(&ralph).unwrap();
        assert!(
            !prompt.contains("Event: code_reviewer.exhausted"),
            "Reviewer should not be exhausted yet"
        );
        event_loop
            .bus
            .publish(Event::new("review.changes_requested", "fix"));
    }

    // One more implementation.done should attempt a 4th reviewer activation.
    let _ = event_loop.build_prompt(&ralph).unwrap();
    event_loop
        .bus
        .publish(Event::new("implementation.done", "done"));

    let prompt = event_loop.build_prompt(&ralph).unwrap();
    assert!(
        prompt.contains("Event: code_reviewer.exhausted"),
        "Expected code_reviewer.exhausted to be emitted when max_activations is exceeded"
    );
    let escalator_id = HatId::new("escalator");
    assert!(
        event_loop
            .bus
            .peek_pending(&escalator_id)
            .is_some_and(|events| {
                events
                    .iter()
                    .any(|e| e.topic.as_str() == "code_reviewer.exhausted")
            }),
        "Expected code_reviewer.exhausted to be published for escalator"
    );

    // Further would-trigger events are dropped (no re-activation beyond max).
    let reviewer_id = HatId::new("code_reviewer");
    assert_eq!(
        *event_loop
            .state
            .hat_activation_counts
            .get(&reviewer_id)
            .unwrap_or(&0),
        3,
        "Reviewer should have exactly max activations recorded"
    );

    event_loop
        .bus
        .publish(Event::new("implementation.done", "done again").with_source(ralph.clone()));
    let prompt = event_loop.build_prompt(&ralph).unwrap();
    assert!(
        !prompt.contains("Event: implementation.done"),
        "Pending events for an exhausted hat should be dropped"
    );
    assert_eq!(
        *event_loop
            .state
            .hat_activation_counts
            .get(&reviewer_id)
            .unwrap_or(&0),
        3,
        "Reviewer should not be activated after exhaustion"
    );
}

#[test]
fn test_record_hat_activations_increments_counts() {
    let mut event_loop = EventLoop::new(RalphConfig::default());
    let planner = HatId::new("planner");
    let reviewer = HatId::new("reviewer");

    event_loop.record_hat_activations(&[planner.clone(), reviewer.clone()]);
    event_loop.record_hat_activations(std::slice::from_ref(&planner));

    assert_eq!(
        event_loop.state.hat_activation_counts.get(&planner),
        Some(&2)
    );
    assert_eq!(
        event_loop.state.hat_activation_counts.get(&reviewer),
        Some(&1)
    );
}

#[test]
fn test_check_hat_exhaustion_emits_once_at_limit() {
    let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.done"]
    publishes: ["review.blocked"]
    max_activations: 2
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let hat_id = HatId::new("reviewer");
    let dropped = vec![
        Event::new("review.done", "ok"),
        Event::new("build.done", "ok"),
    ];

    event_loop
        .state
        .hat_activation_counts
        .insert(hat_id.clone(), 1);
    let (drop, event) = event_loop.check_hat_exhaustion(&hat_id, &dropped);
    assert!(!drop);
    assert!(event.is_none());

    event_loop
        .state
        .hat_activation_counts
        .insert(hat_id.clone(), 2);
    let (drop, event) = event_loop.check_hat_exhaustion(&hat_id, &dropped);
    assert!(drop);
    let exhausted = event.expect("exhausted event");
    assert_eq!(exhausted.topic.as_str(), "reviewer.exhausted");
    assert!(exhausted.payload.contains("max_activations: 2"));
    assert!(exhausted.payload.contains("activations: 2"));

    let (drop_again, event_again) = event_loop.check_hat_exhaustion(&hat_id, &dropped);
    assert!(drop_again);
    assert!(event_again.is_none());
}
