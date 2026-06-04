//! Tests for active_hat.

use super::*;

#[test]
fn test_hatless_mode_registers_ralph_catch_all() {
    // When no hats are configured, "ralph" should be registered as catch-all
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    // Registry should contain the builtin "ralph" hat (from_runtime_config)
    assert!(
        !event_loop.registry().is_empty(),
        "Registry should contain builtin ralph"
    );
    assert!(
        event_loop.registry().get(&HatId::new("ralph")).is_some(),
        "Registry should have ralph registered"
    );

    // No user-defined hats (EventLoop::default creates RalphConfig with empty hats)
    assert_eq!(
        event_loop.config().hats.len(),
        0,
        "No custom hats configured"
    );

    // When we initialize, task.start should route to "ralph"
    event_loop.initialize("Test prompt");

    // "ralph" should have pending events
    let next_hat = event_loop.next_hat();
    assert!(next_hat.is_some(), "Should have pending events for ralph");
    assert_eq!(next_hat.unwrap().as_str(), "ralph");
}

#[test]
fn test_hatless_mode_builds_ralph_prompt() {
    // In hatless mode, build_prompt for "ralph" should return HatlessRalph prompt
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id);

    assert!(prompt.is_some(), "Should build prompt for ralph");
    let prompt = prompt.unwrap();

    // Should contain RFC2119-style Ralph identity (uses "You are Ralph")
    assert!(
        prompt.contains("You are Ralph"),
        "Should identify as Ralph with RFC2119 style"
    );
    assert!(
        prompt.contains("## WORKFLOW"),
        "Should have workflow section"
    );
    assert!(
        prompt.contains("## EVENT WRITING"),
        "Should have event writing section"
    );
    assert!(
        prompt.contains("LOOP_COMPLETE"),
        "Should reference completion event"
    );
}

#[test]
fn test_always_hatless_ralph_executes_all_iterations() {
    // Per acceptance criteria #1: Ralph executes all iterations with custom hats
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start", "build.done"]
    publishes: ["build.task"]
  builder:
    name: "Builder"
    triggers: ["build.task"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Simulate the workflow: task.start → planner (conceptually)
    event_loop.initialize("Implement feature X");
    assert_eq!(event_loop.next_hat().unwrap().as_str(), "ralph");

    // Simulate build.task → builder (conceptually)
    event_loop.build_prompt(&HatId::new("ralph")); // Consume task.start
    event_loop
        .bus
        .publish(Event::new("build.task", "Build feature X"));
    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "build.task should route to Ralph"
    );

    // Simulate build.done → planner (conceptually)
    event_loop.build_prompt(&HatId::new("ralph")); // Consume build.task
    event_loop
        .bus
        .publish(Event::new("build.done", "Feature X complete"));
    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "build.done should route to Ralph"
    );
}

#[test]
fn test_always_hatless_solo_mode_unchanged() {
    // Per acceptance criteria #3: Solo mode (no hats) operates as before
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    // Registry should contain builtin ralph, but no custom hats
    assert!(
        !event_loop.registry().is_empty(),
        "Registry should contain builtin ralph"
    );
    assert!(
        event_loop.registry().get(&HatId::new("ralph")).is_some(),
        "Registry should have ralph registered"
    );
    assert!(
        event_loop.config().hats.is_empty(),
        "No custom hats in config"
    );

    event_loop.initialize("Do something");
    assert_eq!(event_loop.next_hat().unwrap().as_str(), "ralph");

    // Solo mode prompt should NOT have ## HATS section
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        !prompt.contains("## HATS"),
        "Solo mode should not have HATS section"
    );
}

#[test]
fn test_active_hat_gets_publishing_guide_not_topology() {
    // When a hat is triggered, show its instructions + Event Publishing Guide
    // Skip the topology table/Mermaid to reduce token usage
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start", "build.done", "build.blocked"]
    publishes: ["build.task"]
  builder:
    name: "Builder"
    description: "Builds code"
    triggers: ["build.task"]
    publishes: ["build.done", "build.blocked"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test"); // Publishes task.start which triggers Planner

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    // Planner is active (triggered by task.start), so we get ACTIVE HAT section
    assert!(
        prompt.contains("## ACTIVE HAT"),
        "Should have ACTIVE HAT section when hat is triggered"
    );

    // Should NOT have topology table when a hat is active
    assert!(
        !prompt.contains("| Hat | Triggers On | Publishes |"),
        "Should NOT have topology table when hat is active"
    );
    assert!(
        !prompt.contains("```mermaid"),
        "Should NOT have Mermaid diagram when hat is active"
    );

    // Should have Event Publishing Guide showing who receives build.task
    assert!(
        prompt.contains("### Event Publishing Guide"),
        "Should have Event Publishing Guide for active hat"
    );
    assert!(
        prompt.contains("`build.task` → Received by: Builder"),
        "Should show Builder receives build.task"
    );
}

#[test]
fn test_always_hatless_no_backend_delegation() {
    // Per acceptance criteria #5: Custom hat backends are NOT used
    // This is architectural - the EventLoop.next_hat() always returns "ralph"
    // so per-hat backends (if configured) are never invoked
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    backend: "gemini"  # This backend should NEVER be used
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.bus.publish(Event::new("build.task", "Test"));

    // Despite builder having a specific backend, Ralph handles the iteration
    let next = event_loop.next_hat();
    assert_eq!(
        next.unwrap().as_str(),
        "ralph",
        "Ralph handles all iterations"
    );

    // The backend delegation would happen in main.rs, but since we always
    // return "ralph" from next_hat(), the gemini backend is never selected
}

#[test]
fn test_always_hatless_collects_all_pending_events() {
    // Verify Ralph's prompt includes downstream events from all hats when in multi-hat mode.
    // Kickoff events like task.start should drop out once a more specific downstream event exists.
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start"]
  builder:
    name: "Builder"
    triggers: ["build.task"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Publish events that would go to different hats
    event_loop
        .bus
        .publish(Event::new("task.start", "Start task"));
    event_loop
        .bus
        .publish(Event::new("build.task", "Build something"));

    // Ralph should collect ALL pending events
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    // The downstream event should be in Ralph's context, and the kickoff event
    // should not dominate once downstream work is pending.
    assert!(
        !prompt.contains("task.start"),
        "task.start should be filtered once a downstream event is pending"
    );
    assert!(
        prompt.contains("build.task"),
        "Should include build.task event"
    );
}

#[test]
fn test_determine_active_hats() {
    // Create EventLoop with 3 hats (security_reviewer, architecture_reviewer, correctness_reviewer)
    let yaml = r#"
hats:
  security_reviewer:
    name: "Security Reviewer"
    triggers: ["review.security"]
  architecture_reviewer:
    name: "Architecture Reviewer"
    triggers: ["review.architecture"]
  correctness_reviewer:
    name: "Correctness Reviewer"
    triggers: ["review.correctness"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    // Create events: [Event("review.security", "..."), Event("review.architecture", "...")]
    let events = vec![
        Event::new("review.security", "Check for vulnerabilities"),
        Event::new("review.architecture", "Review design patterns"),
    ];

    // Call determine_active_hats(&events)
    let active_hats = event_loop.determine_active_hats(&events);

    // Assert: Returns Vec with exactly security_reviewer and architecture_reviewer Hats
    assert_eq!(active_hats.len(), 2, "Should return exactly 2 active hats");

    let hat_ids: Vec<&str> = active_hats.iter().map(|h| h.id.as_str()).collect();
    assert!(
        hat_ids.contains(&"security_reviewer"),
        "Should include security_reviewer"
    );
    assert!(
        hat_ids.contains(&"architecture_reviewer"),
        "Should include architecture_reviewer"
    );
    assert!(
        !hat_ids.contains(&"correctness_reviewer"),
        "Should NOT include correctness_reviewer"
    );
}

#[test]
fn test_get_active_hat_id_with_pending_event() {
    // Create EventLoop with security_reviewer hat
    let yaml = r#"
hats:
  security_reviewer:
    name: "Security Reviewer"
    triggers: ["review.security"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Publish Event("review.security", "...")
    event_loop
        .bus
        .publish(Event::new("review.security", "Check authentication"));

    // Call get_active_hat_id()
    let active_hat_id = event_loop.get_active_hat_id();

    // Assert: Returns HatId("security_reviewer"), NOT "ralph"
    assert_eq!(
        active_hat_id.as_str(),
        "security_reviewer",
        "Should return security_reviewer, not ralph"
    );
}

#[test]
fn test_get_active_hat_id_no_pending_returns_ralph() {
    // Create EventLoop with hats but NO pending events
    let yaml = r#"
hats:
  security_reviewer:
    name: "Security Reviewer"
    triggers: ["review.security"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    // Call get_active_hat_id() - no pending events
    let active_hat_id = event_loop.get_active_hat_id();

    // Assert: Returns HatId("ralph")
    assert_eq!(
        active_hat_id.as_str(),
        "ralph",
        "Should return ralph when no pending events"
    );
}

#[test]
fn test_get_active_hat_id_deterministic_with_multiple_pending() {
    // Two hats with pending events → get_active_hat_id returns alphabetically first matching hat
    let yaml = r#"
hats:
  zebra_hat:
    name: "Zebra"
    triggers: ["work.*"]
  alpha_hat:
    name: "Alpha"
    triggers: ["work.*"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Publish event that both hats subscribe to
    event_loop
        .bus
        .publish(Event::new("work.start", "Begin work"));

    // Should deterministically return "alpha_hat" (alphabetically first)
    let active = event_loop.get_active_hat_id();
    assert_eq!(
        active.as_str(),
        "alpha_hat",
        "get_active_hat_id should return alphabetically first matching hat"
    );

    // Run multiple times to confirm determinism
    for _ in 0..100 {
        let active = event_loop.get_active_hat_id();
        assert_eq!(active.as_str(), "alpha_hat");
    }
}

#[test]
fn test_get_active_hat_id_matches_prompt_active_hat_selection() {
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["debug.start", "hypothesis.confirmed"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("debug.start", "Investigate a bug"));
    event_loop
        .bus
        .publish(Event::new("hypothesis.test", "Test the hypothesis"));

    let preview_active_hat = event_loop.get_active_hat_id();

    event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");

    let built_active_hat = event_loop
        .state
        .last_active_hat_ids
        .first()
        .expect("build_prompt should set active hats")
        .clone();

    assert_eq!(
        preview_active_hat.as_str(),
        "tester",
        "downstream hypothesis.test should outrank kickoff debug.start in preview selection"
    );
    assert_eq!(
        built_active_hat.as_str(),
        "tester",
        "build_prompt should select tester when debug.start and hypothesis.test are both pending"
    );
    assert_eq!(
        preview_active_hat, built_active_hat,
        "display hat preview should match prompt-selected active hat"
    );
}

#[test]
fn test_get_active_hat_id_prefers_semantic_event_over_targeted_task_resume() {
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["task.resume", "debug.start", "hypothesis.confirmed"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("task.resume", "Recovery").with_target("investigator"));
    event_loop
        .bus
        .publish(Event::new("hypothesis.test", "Test the hypothesis"));

    let preview_active_hat = event_loop.get_active_hat_id();
    assert_eq!(
        preview_active_hat.as_str(),
        "tester",
        "semantic downstream work should outrank fallback task.resume for display selection"
    );

    event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");

    let built_active_hat = event_loop
        .state
        .last_active_hat_ids
        .first()
        .expect("build_prompt should set active hats")
        .clone();
    assert_eq!(
        built_active_hat.as_str(),
        "tester",
        "prompt-selected active hat should ignore fallback task.resume when real work is pending"
    );
}

#[test]
fn test_get_active_hat_id_honors_direct_target_before_topic_lookup() {
    let yaml = r#"
hats:
  alpha_hat:
    name: "Alpha"
    triggers: ["task.resume"]
  zebra_hat:
    name: "Zebra"
    triggers: ["task.resume"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("task.resume", "Recovery").with_target("zebra_hat"));

    let active_hat_id = event_loop.get_active_hat_id();
    assert_eq!(
        active_hat_id.as_str(),
        "zebra_hat",
        "direct event targets should override generic topic subscriber ordering"
    );
}

#[test]
fn test_determine_active_hat_ids_excludes_entrypoint_hats_when_progressed_events_exist() {
    // When both a stale entrypoint event (review.start) and a progressed event
    // (review.done) are pending, only the downstream hat should be activated —
    // not the entrypoint hat. This prevents the coordinator from being
    // re-included alongside the synthesizer after wave workers complete.
    let yaml = r#"
event_loop:
  starting_event: "review.start"
  completion_promise: "review.complete"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["review.start"]
    publishes: ["review.perspective"]
  synthesizer:
    name: "Synthesizer"
    triggers: ["review.done"]
    publishes: ["review.complete"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Review the auth module");

    // Simulate state after wave workers complete: stale review.start +
    // new review.done events are both pending.
    let events = vec![
        Event::new("review.start", "Review the auth module"),
        Event::new("review.done", "## Rust Review\n..."),
        Event::new("review.done", "## Frontend Review\n..."),
    ];

    let active = event_loop.determine_active_hat_ids(&events);
    assert_eq!(
        active.len(),
        1,
        "Only the synthesizer should be active, not coordinator + synthesizer"
    );
    assert_eq!(
        active[0].as_str(),
        "synthesizer",
        "The synthesizer (triggered by review.done) should be selected over the coordinator (triggered by stale review.start)"
    );
}

#[test]
fn test_determine_active_hat_ids_falls_back_to_entrypoint_when_no_progressed_events() {
    // When only entrypoint events are pending, the entrypoint hat should be activated.
    let yaml = r#"
event_loop:
  starting_event: "review.start"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["review.start"]
    publishes: ["review.perspective"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Review the auth module");

    let events = vec![Event::new("review.start", "Review the auth module")];

    let active = event_loop.determine_active_hat_ids(&events);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].as_str(), "coordinator");
}

#[test]
fn test_check_for_user_prompt_detects_user_prompt_event() {
    // Create EventLoop
    let config: RalphConfig = serde_yaml::from_str("hats: {}").unwrap();
    let event_loop = EventLoop::new(config);

    // Create events with a user.prompt event
    // The id is embedded in the XML payload
    let events = vec![
        Event::new("build.task", "Some task"),
        Event::new(
            "user.prompt",
            r#"<event topic="user.prompt" id="q1">What is the feature name?</event>"#,
        ),
        Event::new("other.event", "Other"),
    ];

    // Check for user prompt
    let user_prompt = event_loop.check_for_user_prompt(&events);

    assert!(user_prompt.is_some(), "Should detect user.prompt event");
    assert_eq!(user_prompt.unwrap().id, "q1");
}

#[test]
fn test_check_for_user_prompt_returns_none_when_no_user_prompt() {
    // Create EventLoop
    let config: RalphConfig = serde_yaml::from_str("hats: {}").unwrap();
    let event_loop = EventLoop::new(config);

    // Create events WITHOUT a user.prompt event
    let events = vec![
        Event::new("build.task", "Some task"),
        Event::new("build.done", "Task completed"),
    ];

    // Check for user prompt
    let user_prompt = event_loop.check_for_user_prompt(&events);

    assert!(
        user_prompt.is_none(),
        "Should not detect user.prompt when not present"
    );
}

#[test]
fn test_extract_prompt_id_from_xml_format() {
    // Create EventLoop
    let config: RalphConfig = serde_yaml::from_str("hats: {}").unwrap();
    let event_loop = EventLoop::new(config);

    // Create event with XML attribute format
    let event = Event::new(
        "user.prompt",
        r#"<event topic="user.prompt" id="q42">What's the deadline?</event>"#,
    );
    let events = vec![event];

    let user_prompt = event_loop.check_for_user_prompt(&events).unwrap();
    assert_eq!(user_prompt.id, "q42");
}

#[test]
fn test_extract_prompt_id_prefers_xml_id() {
    let payload = r#"<event topic="user.prompt" id="q42">Question?</event>"#;
    assert_eq!(EventLoop::extract_prompt_id(payload), "q42");
}

#[test]
fn test_extract_prompt_id_fallback_prefix() {
    let id = EventLoop::extract_prompt_id("Plain question");
    assert!(id.starts_with('q'));
    assert!(id.len() > 1);
}

#[test]
fn test_check_for_user_prompt_extracts_id_and_text() {
    let event_loop = EventLoop::new(RalphConfig::default());
    let payload = r#"<event topic="user.prompt" id="q7">Need input</event>"#;
    let events = vec![
        Event::new("build.done", "ok"),
        Event::new("user.prompt", payload),
    ];

    let prompt = event_loop.check_for_user_prompt(&events).expect("prompt");
    assert_eq!(prompt.id, "q7");
    assert_eq!(prompt.text, payload);
}
