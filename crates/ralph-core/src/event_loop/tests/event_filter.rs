//! Tests for event_filter.

use super::*;

#[test]
fn test_event_filter_no_filter_sees_full_history() {
    // Regression: hat without event_filter sees all events in prompt.
    let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.trigger"]
    publishes: ["review.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("review.trigger", "trigger payload"));
    event_loop
        .bus
        .publish(Event::new("other.event", "other payload"));

    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id).unwrap();

    assert!(
        prompt.contains("review.trigger"),
        "Prompt should contain review.trigger"
    );
    assert!(
        prompt.contains("other.event"),
        "Prompt should contain other.event when no filter is set"
    );
}

#[test]
fn test_event_filter_allowlist_filters_events() {
    // Only allowlisted events appear in the prompt.
    let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.trigger"]
    publishes: ["review.done"]
    event_filter:
      enabled: true
      events: ["review.trigger", "review.file"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("review.trigger", "trigger payload"));
    event_loop
        .bus
        .publish(Event::new("other.event", "other payload"));

    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id).unwrap();

    assert!(
        prompt.contains("review.trigger"),
        "Prompt should contain allowlisted review.trigger"
    );
    assert!(
        !prompt.contains("other.event"),
        "Prompt should NOT contain non-allowlisted other.event"
    );
}

#[test]
fn test_event_filter_trigger_auto_included() {
    // Trigger events are automatically added to the allowlist.
    let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.trigger"]
    publishes: ["review.done"]
    event_filter:
      enabled: true
      events: ["review.file"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("review.trigger", "trigger payload"));
    event_loop
        .bus
        .publish(Event::new("review.file", "file payload"));
    event_loop
        .bus
        .publish(Event::new("other.event", "other payload"));

    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id).unwrap();

    assert!(
        prompt.contains("review.trigger"),
        "Trigger event should be auto-included in prompt"
    );
    assert!(
        prompt.contains("review.file"),
        "Explicitly allowlisted event should be included"
    );
    assert!(
        !prompt.contains("other.event"),
        "Non-allowlisted event should be excluded"
    );
}

#[test]
fn test_event_filter_multi_hat_union_allowlist() {
    // When multiple active hats have filters, the allowlist is the union.
    let yaml = r#"
hats:
  hat_a:
    name: "Hat A"
    triggers: ["event.a"]
    publishes: ["done.a"]
    event_filter:
      enabled: true
      events: ["event.a"]
  hat_b:
    name: "Hat B"
    triggers: ["event.b"]
    publishes: ["done.b"]
    event_filter:
      enabled: true
      events: ["event.b"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.bus.publish(Event::new("event.a", "payload a"));
    event_loop.bus.publish(Event::new("event.b", "payload b"));
    event_loop.bus.publish(Event::new("event.c", "payload c"));

    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id).unwrap();

    assert!(
        prompt.contains("event.a"),
        "Union allowlist should include event.a"
    );
    assert!(
        prompt.contains("event.b"),
        "Union allowlist should include event.b"
    );
    assert!(
        !prompt.contains("event.c"),
        "Union allowlist should exclude event.c"
    );
}
