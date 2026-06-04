//! Tests for objective.

use super::*;

#[test]
fn test_initialize_stores_objective_in_ralph() {
    // initialize() should store the prompt as the objective in HatlessRalph
    // so that subsequent iterations always see it, even after bus.take_pending() consumes the start event.
    let yaml = r#"
hats:
  test_writer:
    name: "Test Writer"
    triggers: ["tdd.start"]
    publishes: ["test.written"]
    instructions: "Write failing tests."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.initialize("Implement a binary search tree with insert and search");

    // Consume the start event (simulates iteration 1 completing)
    let ralph_id = HatId::new("ralph");
    let prompt1 = event_loop.build_prompt(&ralph_id).unwrap();
    assert!(
        prompt1.contains("## OBJECTIVE"),
        "Iteration 1 should have OBJECTIVE section"
    );
    assert!(
        prompt1.contains("Implement a binary search tree"),
        "Iteration 1 should show the objective"
    );

    // Simulate iteration 2: hat publishes an event, start event is gone
    event_loop
        .bus
        .publish(Event::new("test.written", "tests/bst_test.rs"));

    let prompt2 = event_loop.build_prompt(&ralph_id).unwrap();

    // Objective should STILL be present even though task.start was consumed
    assert!(
        prompt2.contains("## OBJECTIVE"),
        "Iteration 2+ should still have OBJECTIVE section"
    );
    assert!(
        prompt2.contains("Implement a binary search tree"),
        "Objective should persist across iterations"
    );
}
