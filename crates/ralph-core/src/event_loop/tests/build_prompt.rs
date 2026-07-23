//! Tests for build_prompt.

use super::*;

#[test]
fn test_build_prompt_uses_ghuntley_style_for_all_hats() {
    // Per Hatless Ralph spec: All hats use build_custom_hat with ghuntley-style prompts
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start", "build.done", "build.blocked"]
    publishes: ["build.task"]
  builder:
    name: "Builder"
    triggers: ["build.task"]
    publishes: ["build.done", "build.blocked"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test task");

    // Planner hat should get ghuntley-style prompt via build_custom_hat
    let planner_id = HatId::new("planner");
    let planner_prompt = event_loop.build_prompt(&planner_id).unwrap();

    // Verify ghuntley-style structure (numbered phases, guardrails)
    assert!(
        planner_prompt.contains("### 0. ORIENTATION"),
        "Planner should use ghuntley-style orientation phase"
    );
    assert!(
        planner_prompt.contains("### GUARDRAILS"),
        "Planner prompt should have guardrails section"
    );
    assert!(
        planner_prompt.contains("You have fresh context each iteration"),
        "Planner prompt should have RFC2119 identity"
    );

    // Now trigger builder hat by publishing build.task event
    let hat_id = HatId::new("builder");
    event_loop
        .bus
        .publish(Event::new("build.task", "Build something"));

    let builder_prompt = event_loop.build_prompt(&hat_id).unwrap();

    // Verify RFC2119-style structure for builder too
    assert!(
        builder_prompt.contains("### 0. ORIENTATION"),
        "Builder should use RFC2119-style orientation phase"
    );
    assert!(
        builder_prompt.contains("You MUST NOT use more than 1 subagent for build/tests"),
        "Builder prompt should have subagent limit with MUST NOT"
    );
}

#[test]
fn test_build_prompt_uses_custom_hat_for_non_defaults() {
    // Per spec: Custom hats use build_custom_hat with their instructions
    let yaml = r#"
mode: "multi"
hats:
  reviewer:
    name: "Code Reviewer"
    triggers: ["review.request"]
    instructions: "Review code quality."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Publish event to trigger reviewer
    event_loop
        .bus
        .publish(Event::new("review.request", "Review PR #123"));

    let reviewer_id = HatId::new("reviewer");
    let prompt = event_loop.build_prompt(&reviewer_id).unwrap();

    // Should be custom hat prompt (contains custom instructions)
    assert!(
        prompt.contains("Code Reviewer"),
        "Custom hat should use its name"
    );
    assert!(
        prompt.contains("Review code quality"),
        "Custom hat should include its instructions"
    );
    // Should NOT be planner or builder prompt
    assert!(
        !prompt.contains("PLANNER MODE"),
        "Custom hat should not use planner prompt"
    );
    assert!(
        !prompt.contains("BUILDER MODE"),
        "Custom hat should not use builder prompt"
    );
}

#[test]
fn test_custom_hat_with_instructions_uses_build_custom_hat() {
    // Per spec: Custom hats with instructions should use build_custom_hat() method
    let yaml = r#"
hats:
  reviewer:
    name: "Code Reviewer"
    triggers: ["review.request"]
    instructions: "Review code for quality and security issues."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Trigger the custom hat
    event_loop
        .bus
        .publish(Event::new("review.request", "Review PR #123"));

    let reviewer_id = HatId::new("reviewer");
    let prompt = event_loop.build_prompt(&reviewer_id).unwrap();

    // Should use build_custom_hat() - verify by checking for ghuntley-style structure
    assert!(
        prompt.contains("Code Reviewer"),
        "Should include custom hat name"
    );
    assert!(
        prompt.contains("Review code for quality and security issues"),
        "Should include custom instructions"
    );
    assert!(
        prompt.contains("### 0. ORIENTATION"),
        "Should include ghuntley-style orientation"
    );
    assert!(
        prompt.contains("### 1. EXECUTE"),
        "Should use ghuntley-style execute phase"
    );
    assert!(
        prompt.contains("### GUARDRAILS"),
        "Should include guardrails section"
    );

    // Should include event context
    assert!(
        prompt.contains("Review PR #123"),
        "Should include event context"
    );
}

#[test]
fn test_custom_hat_instructions_included_in_prompt() {
    // Test that custom instructions are properly included in the generated prompt
    let yaml = r#"
hats:
  tester:
    name: "Test Engineer"
    triggers: ["test.request"]
    instructions: |
      Run comprehensive tests including:
      - Unit tests
      - Integration tests
      - Security scans
      Report results with detailed coverage metrics.
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Trigger the custom hat
    event_loop
        .bus
        .publish(Event::new("test.request", "Test the auth module"));

    let tester_id = HatId::new("tester");
    let prompt = event_loop.build_prompt(&tester_id).unwrap();

    // Verify all custom instructions are included
    assert!(prompt.contains("Run comprehensive tests including"));
    assert!(prompt.contains("Unit tests"));
    assert!(prompt.contains("Integration tests"));
    assert!(prompt.contains("Security scans"));
    assert!(prompt.contains("detailed coverage metrics"));

    // Verify event context is included
    assert!(prompt.contains("Test the auth module"));
}

#[test]
fn test_active_hat_with_instructions_and_publishing_guide() {
    // When a hat is triggered by an event, show ACTIVE HAT section with
    // instructions and Event Publishing Guide instead of full topology.
    let yaml = r#"
hats:
  deployer:
    name: "Deployment Manager"
    triggers: ["deploy.request", "deploy.rollback"]
    publishes: ["deploy.done", "deploy.failed"]
    instructions: "Handle deployment operations safely."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Publish an event that triggers the deployer hat
    event_loop
        .bus
        .publish(Event::new("deploy.request", "Deploy to staging"));

    // In multi-hat mode, next_hat always returns "ralph"
    let next_hat = event_loop.next_hat();
    assert_eq!(
        next_hat.unwrap().as_str(),
        "ralph",
        "Multi-hat mode routes to Ralph"
    );

    // Build Ralph's prompt - should include active hat info
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    // The event topic should be in PENDING EVENTS
    assert!(
        prompt.contains("deploy.request"),
        "Should include the event topic in pending events"
    );

    // Should have ACTIVE HAT section (not HATS topology table)
    assert!(
        prompt.contains("## ACTIVE HAT"),
        "Should have ACTIVE HAT section when hat is triggered"
    );
    assert!(
        !prompt.contains("| Hat | Triggers On | Publishes |"),
        "Should NOT have topology table when hat is active"
    );

    // Should include the hat's instructions
    assert!(
        prompt.contains("Handle deployment operations safely"),
        "Should include active hat's instructions"
    );

    // Should have Event Publishing Guide
    assert!(
        prompt.contains("### Event Publishing Guide"),
        "Should have Event Publishing Guide"
    );
    assert!(
        prompt.contains("`deploy.done`"),
        "Guide should list deploy.done"
    );
    assert!(
        prompt.contains("`deploy.failed`"),
        "Guide should list deploy.failed"
    );
}

#[test]
fn test_default_hat_with_custom_instructions_uses_build_custom_hat() {
    // Test that even default hats (planner/builder) use build_custom_hat when they have custom instructions
    let yaml = r#"
hats:
  planner:
    name: "Custom Planner"
    triggers: ["task.start", "build.done"]
    instructions: "Custom planning instructions with special focus on security."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.initialize("Test task");

    let planner_id = HatId::new("planner");
    let prompt = event_loop.build_prompt(&planner_id).unwrap();

    // Should use build_custom_hat with ghuntley-style structure
    assert!(prompt.contains("Custom Planner"), "Should use custom name");
    assert!(
        prompt.contains("Custom planning instructions with special focus on security"),
        "Should include custom instructions"
    );
    assert!(
        prompt.contains("### 1. EXECUTE"),
        "Should use ghuntley-style execute phase"
    );
    assert!(
        prompt.contains("### GUARDRAILS"),
        "Should include guardrails section"
    );
}

#[test]
fn test_custom_hat_without_instructions_gets_default_behavior() {
    // Test that custom hats without instructions still work with build_custom_hat
    let yaml = r#"
hats:
  monitor:
    name: "System Monitor"
    triggers: ["monitor.request"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("monitor.request", "Check system health"));

    let monitor_id = HatId::new("monitor");
    let prompt = event_loop.build_prompt(&monitor_id).unwrap();

    // Should still use build_custom_hat with ghuntley-style structure
    assert!(
        prompt.contains("System Monitor"),
        "Should include custom hat name"
    );
    assert!(
        prompt.contains("Follow the incoming event instructions"),
        "Should have default instructions when none provided"
    );
    assert!(
        prompt.contains("### 0. ORIENTATION"),
        "Should include ghuntley-style orientation"
    );
    assert!(
        prompt.contains("### GUARDRAILS"),
        "Should include guardrails section"
    );
    assert!(
        prompt.contains("Check system health"),
        "Should include event context"
    );
}

#[test]
fn test_done_section_suppressed_for_active_hat_via_event_loop() {
    // When a hat is active (triggered by an event), the DONE section should NOT appear.
    // This prevents intermediate hats from seeing completion instructions.
    let yaml = r#"
hats:
  implementer:
    name: "Implementer"
    triggers: ["test.written"]
    publishes: ["test.passing"]
    instructions: "Make the failing test pass."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Build a calculator");

    // Consume the start event
    let ralph_id = HatId::new("ralph");
    let _ = event_loop.build_prompt(&ralph_id);

    // Simulate implementer being triggered
    event_loop
        .bus
        .publish(Event::new("test.written", "tests/calc_test.rs"));

    let prompt = event_loop.build_prompt(&ralph_id).unwrap();

    // Implementer hat is active — DONE section should be suppressed
    assert!(
        !prompt.contains("## DONE"),
        "DONE section should be suppressed when a hat is active"
    );
    assert!(
        !prompt.contains("You MUST emit a completion event"),
        "Completion instruction should not appear for active hat"
    );

    // But the objective should still be visible
    assert!(
        prompt.contains("## OBJECTIVE"),
        "OBJECTIVE should still be visible to active hat"
    );
    assert!(
        prompt.contains("Build a calculator"),
        "Objective content should be visible"
    );
}

// --- plan 004 U6 Tier 2: ralph-tools skill R0 anchors must be auto-injected ---

#[test]
fn test_build_prompt_injects_ralph_tools_skill_r0_block() {
    // Given: a minimal isolated-mode config with memories.enabled = true
    // When: build_prompt for the isolated hat (so the isolated branch
    //   in `EventLoop::build_prompt` runs `prepend_auto_inject_skills` —
    //   the backward-compat custom-hat path explicitly skips skill
    //   injection per `mod.rs:4371-4374`)
    // Then: the prompt contains <ralph-tools-skill> with the R0
    //   "收到 task.resume 时" anchor (plan 004 U6 Tier 2)
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  builder:
    name: "Builder"
    triggers: ["work.start"]
    publishes: ["work.done"]
memories:
  enabled: true
  inject: auto
tasks:
  enabled: true
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Inject anchor test");

    let hat_id = HatId::new("builder");
    let prompt = event_loop.build_prompt(&hat_id).unwrap();

    // R0 anchor: the section title "收到 task.resume 时" must appear in the
    // auto-injected <ralph-tools-skill> block. This guarantees the
    // Tier 2 contract: file content == injected content == visible to agent.
    assert!(
        prompt.contains("<ralph-tools-skill>"),
        "build_prompt must wrap ralph-tools.md in <ralph-tools-skill>; got prompt:\n{}",
        &prompt[..prompt.len().min(3000)]
    );
    assert!(
        prompt.contains("收到 `task.resume` 时"),
        "R0 anchor '收到 task.resume 时' must be in the auto-injected ralph-tools block"
    );
    assert!(
        prompt.contains("required_fields"),
        "R0 anchor 'required_fields' must be in the auto-injected ralph-tools block"
    );
    assert!(
        prompt.contains("--policy-check"),
        "R0 anchor '--policy-check' must be in the auto-injected ralph-tools block"
    );

    // Negative anchor: the legacy unsafe-bypass suggestion must NOT appear as a
    // recommended path. The phrasing in §通用错误恢复 was rewritten to
    // explicitly steer agents away from `--unsafe-no-policy-check` as a
    // first choice.
    assert!(
        !prompt.contains("确认配置允许 `--unsafe-no-policy-check`"),
        "R0b: ralph-tools.md must NOT recommend `--unsafe-no-policy-check` as a default fix"
    );
}

#[test]
fn test_build_prompt_injects_ralph_tools_via_tasks_only() {
    // Branch coverage: ralph-tools is injected when EITHER memories.enabled
    // OR tasks.enabled is true (event_loop/mod.rs:4862-4873).
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  builder:
    name: "Builder"
    triggers: ["work.start"]
    publishes: ["work.done"]
memories:
  enabled: false
tasks:
  enabled: true
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Tasks-only injection test");

    let prompt = event_loop.build_prompt(&HatId::new("builder")).unwrap();
    assert!(
        prompt.contains("<ralph-tools-skill>") && prompt.contains("收到 `task.resume` 时"),
        "ralph-tools must be injected when tasks.enabled = true (even with memories off)"
    );
}

#[test]
fn test_build_prompt_injects_recovery_directives_from_task_resume() {
    // 2026-06-28-003: a pending `task.resume` event with
    // `recovery_directives` must cause the runner to prepend a
    // `## RECOVERY DIRECTIVES` block to the prompt.
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready", "task.resume"]
    publishes: ["work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Recovery directives injection test");

    let payload = serde_json::json!({
        "reason": "missing_event_gate",
        "target_hat": "executor",
        "kind": "missing_event_gate",
        "recovery_directives": ["RD-EXECUTOR-RESEND-LIMIT"],
    });
    event_loop.bus.publish(
        Event::new("task.resume", payload.to_string()).with_target(HatId::new("executor")),
    );

    // In coordinator mode the ralph hat consumes pending events and
    // builds the coordinator prompt; recovery directives must be
    // prepended there.
    let prompt = event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");
    assert!(
        prompt.contains("## RECOVERY DIRECTIVES"),
        "prompt must contain RECOVERY DIRECTIVES section; got:\n{prompt}"
    );
    assert!(
        prompt.contains("RD-EXECUTOR-RESEND-LIMIT"),
        "prompt must contain the directive section body"
    );
    assert!(
        prompt.contains("at most 2 times"),
        "directive content must be injected"
    );
}

#[test]
fn test_build_prompt_skips_recovery_directives_when_empty() {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready", "task.resume"]
    publishes: ["work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("No recovery directives test");

    event_loop
        .bus
        .publish(Event::new("work.ready", "{}").with_target(HatId::new("executor")));

    let prompt = event_loop
        .build_prompt(&HatId::new("executor"))
        .expect("prompt should build");
    assert!(
        !prompt.contains("## RECOVERY DIRECTIVES"),
        "RECOVERY DIRECTIVES must not appear when no task.resume carries directives"
    );
}

#[test]
fn isolated_ralph_build_prompt_does_not_drain_multi_consumer_peer_pending() {
    // Regression: coordinator-style ralph build_prompt drained every hat's
    // pending queue even in isolated mode, stealing `plan.complete` from
    // reporter/shipper when ralph was selected with a stray task.resume.
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
hats:
  shipper:
    name: "Shipper"
    triggers: ["plan.complete"]
    publishes: ["plan.complete"]
    trigger_multi_consumer_topics: ["plan.complete"]
  reporter:
    name: "Reporter"
    triggers: ["plan.complete"]
    publishes: ["LOOP_COMPLETE"]
    trigger_multi_consumer_topics: ["plan.complete"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Multi-consumer terminal handoff");

    event_loop
        .bus
        .publish(Event::new("plan.complete", r#"{"plan_name":"p"}"#));
    event_loop
        .bus
        .publish(Event::new("task.resume", r#"{"reason":"stall_no_events"}"#));

    let ralph_id = HatId::new("ralph");
    let reporter_id = HatId::new("reporter");
    let shipper_id = HatId::new("shipper");

    assert!(
        event_loop.build_prompt(&ralph_id).is_some(),
        "isolated ralph must use per-hat pending, not coordinator drain"
    );

    assert!(
        event_loop
            .bus
            .peek_pending(&reporter_id)
            .is_some_and(|q| q.iter().any(|e| e.topic.as_str() == "plan.complete")),
        "reporter must still hold plan.complete after isolated ralph activation"
    );
    assert!(
        event_loop
            .bus
            .peek_pending(&shipper_id)
            .is_some_and(|q| q.iter().any(|e| e.topic.as_str() == "plan.complete")),
        "shipper must still hold plan.complete after isolated ralph activation"
    );
}
