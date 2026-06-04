//! Tests for wave_results.

use super::*;

#[test]
fn test_wave_results_activate_synthesizer() {
    // Simulates the wave review scenario:
    // 1. Coordinator dispatches review.perspective (wave events)
    // 2. After wave, review.done events are published to bus
    // 3. On next iteration, synthesizer should be the active hat
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["review.start"]
    publishes: ["review.perspective"]
    instructions: "Dispatch reviewers as a wave."
  reviewer:
    name: "Reviewer"
    triggers: ["review.perspective"]
    publishes: ["review.done"]
    concurrency: 3
    instructions: "Review code from your specialty."
  synthesizer:
    name: "Synthesizer"
    triggers: ["review.done"]
    publishes: ["review.complete"]
    instructions: "SYNTHESIZER MODE - Aggregate all review.done findings into a report."
    aggregate:
      mode: wait_for_all
      timeout: 300
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Step 1: Initialize with review.start — coordinator activates
    event_loop.initialize("Review the code");
    assert_eq!(event_loop.next_hat().unwrap().as_str(), "ralph");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("Coordinator"),
        "Should activate coordinator for review.start"
    );

    // Step 2: Simulate wave results — publish review.done events directly to bus
    // (this is what loop_runner does after merge_wave_results_to_events_file + re-read)
    event_loop
        .bus
        .publish(Event::new("review.done", "Rust review findings"));
    event_loop
        .bus
        .publish(Event::new("review.done", "Frontend review findings"));
    event_loop
        .bus
        .publish(Event::new("review.done", "Docs review findings"));

    // Step 3: next_hat should find pending events and return ralph
    assert!(
        event_loop.next_hat().is_some(),
        "Should have pending events for next hat"
    );

    // Step 4: build_prompt should activate synthesizer (not coordinator)
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        prompt.contains("SYNTHESIZER MODE"),
        "Should activate synthesizer for review.done events"
    );
    assert!(
        !prompt.contains("Dispatch reviewers"),
        "Should NOT have coordinator instructions"
    );
    assert!(
        prompt.contains("review.done"),
        "Should contain review.done events in context"
    );
}
