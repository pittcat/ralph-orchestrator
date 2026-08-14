//! Tests for build_prompt.

use super::*;

fn head_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Return true if the ready-tasks block contains a line for `title`
/// (matched at the start of the line, after any leading `- [ ] [P1] `)
/// that ends with the `[read-only]` marker. Used to assert on the
/// rendered task line specifically, since the broader prompt may
/// legitimately mention `[read-only]` as a concept in injected skill
/// docs.
fn task_line_is_read_only(prompt: &str, title: &str) -> bool {
    let Some(start) = prompt.find("<ready-tasks>") else {
        return false;
    };
    let after_start = &prompt[start..];
    let Some(end) = after_start.find("</ready-tasks>") else {
        return false;
    };
    let block = &after_start[..end];
    for line in block.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("- ") {
            continue;
        }
        if trimmed.contains(title) && trimmed.contains("[read-only]") {
            return true;
        }
    }
    false
}

#[test]
fn terminal_hat_prompt_injects_deliverable_path_contract_from_completion_schema() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      LOOP_COMPLETE:
        payload: json_object
        required_fields: [reason, report_path]
        field_docs:
          report_path:
            source: ".ralph/reports/REPORT.md written by this activation"
            fill_rule: "use the readable repo-relative report path"
hats:
  worker:
    name: Worker
    triggers: [work.start]
    publishes: [work.done]
    instructions: "Do the work."
  reporter:
    name: Reporter
    triggers: [work.done]
    publishes: [LOOP_COMPLETE]
    terminal_events: [LOOP_COMPLETE]
    instructions: "Write the final report."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test terminal deliverable");

    let prompt = event_loop
        .build_prompt(&HatId::new("reporter"))
        .expect("terminal reporter prompt");

    assert!(prompt.contains("## TERMINAL DELIVERABLE CONTRACT"));
    assert!(prompt.contains(".ralph/reports/REPORT.md written by this activation"));
    assert!(prompt.contains("`report_path`"));
    assert!(prompt.contains("DELIVERABLE_PATH: <report_path>"));
}

#[test]
fn non_terminal_hat_prompt_does_not_inject_deliverable_path_contract() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      LOOP_COMPLETE:
        payload: json_object
        required_fields: [report_path]
hats:
  worker:
    name: Worker
    triggers: [work.start]
    publishes: [work.done]
    instructions: "Do the work."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test intermediate prompt");

    let prompt = event_loop
        .build_prompt(&HatId::new("worker"))
        .expect("worker prompt");

    assert!(!prompt.contains("## TERMINAL DELIVERABLE CONTRACT"));
}

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
        prompt.contains("allowed_topics") && prompt.contains("recorded=true"),
        "generic recovery directive content must be injected"
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

#[test]
fn ready_tasks_marks_non_mutable_tasks_read_only_for_non_coordinator_hat() {
    // Regression (2026-07-30 parallel-forge worktree hang): a non-coordinator
    // hat that can see tasks but cannot lifecycle-mutate any of them must
    // see a `[read-only]` marker per task plus a "none actionable" header,
    // so the agent does not call `ralph tools task start` on a task the
    // runtime ACL will reject (which historically parked the activation
    // until the no-progress watchdog killed the loop).
    use crate::task::Task;
    use crate::task_store::TaskStore;

    let temp = tempfile::tempdir().unwrap();
    let tasks_path = temp.path().join(".ralph/agent/tasks.jsonl");
    std::fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();

    let mut store = TaskStore::load(&tasks_path).unwrap();
    let task = Task::new("F1 impl".to_string(), 1).with_owner_hat(Some("executor".to_string()));
    store.add(task);
    store.save().unwrap();

    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  worktree:
    name: "Worktree"
    triggers: ["forge.concurrency.approved"]
    publishes: ["forge.worktrees.ready"]
    instructions: "Create worktrees."
  executor:
    name: "Executor"
    triggers: ["exec.unit.ready"]
    publishes: ["exec.unit.done"]
    instructions: "Implement units."
tasks:
  enabled: true
  coordinator_hats: []
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = temp.path().to_path_buf();

    let loop_context = crate::loop_context::LoopContext::primary(temp.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, loop_context);
    event_loop.initialize("read-only marker test");

    let prompt = event_loop
        .build_prompt(&HatId::new("worktree"))
        .expect("worktree prompt");

    assert!(
        task_line_is_read_only(&prompt, "F1 impl"),
        "non-coordinator hat must see [read-only] marker on the task line; ready block:\n{}",
        head_chars(&prompt, 4000)
    );
    assert!(
        prompt.contains("none actionable for this hat"),
        "non-coordinator hat with no mutable tasks must see 'none actionable' header; got prompt:\n{}",
        head_chars(&prompt, 4000)
    );
}

#[test]
fn ready_tasks_no_read_only_marker_for_owner_hat() {
    // The owner hat sees tasks without a [read-only] marker (it CAN mutate
    // its own tasks), preserving the legacy actionable rendering.
    use crate::task::Task;
    use crate::task_store::TaskStore;

    let temp = tempfile::tempdir().unwrap();
    let tasks_path = temp.path().join(".ralph/agent/tasks.jsonl");
    std::fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();

    let mut store = TaskStore::load(&tasks_path).unwrap();
    let task = Task::new("F1 impl".to_string(), 1).with_owner_hat(Some("executor".to_string()));
    store.add(task);
    store.save().unwrap();

    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  executor:
    name: "Executor"
    triggers: ["exec.unit.ready"]
    publishes: ["exec.unit.done"]
    instructions: "Implement units."
tasks:
  enabled: true
  coordinator_hats: []
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = temp.path().to_path_buf();

    let loop_context = crate::loop_context::LoopContext::primary(temp.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, loop_context);
    event_loop.initialize("owner actionable test");

    let prompt = event_loop
        .build_prompt(&HatId::new("executor"))
        .expect("executor prompt");

    assert!(
        prompt.contains("<ready-tasks>"),
        "owner hat must still see the ready-tasks block; got prompt head:\n{}",
        head_chars(&prompt, 2000)
    );
    // The `[read-only]` marker in the skill doc is a generic concept
    // description; assert against the rendered task line for F1 impl so
    // this test stays focused on the actionable-rendering contract for
    // the owner hat.
    assert!(
        !task_line_is_read_only(&prompt, "F1 impl"),
        "owner hat must NOT see [read-only] marker on its own task; ready block:\n{}",
        head_chars(&prompt, 4000)
    );
    assert!(
        !prompt.contains("none actionable for this hat"),
        "owner hat must NOT see the 'none actionable' header"
    );
}

#[test]
fn ready_tasks_marks_non_self_owner_read_only_for_coordinator_hat() {
    // A coordinator hat (i.e. one listed in `tasks.coordinator_hats`) sees
    // ready tasks it does NOT own rendered with a `[read-only]` marker plus
    // a "none actionable for this hat" header. The coordinator's lifecycle
    // mutation rights are preserved (CLI `start` / `close` / `fail` / `reopen`
    // / `add` / `ensure` are still authorized), but the prompt must not
    // invite the coordinator to *execute* a unit task it does not own.
    // This test pins the prompt-injection behaviour; ACL behaviour is
    // covered by the task_cli auth tests.
    use crate::task::Task;
    use crate::task_store::TaskStore;

    let temp = tempfile::tempdir().unwrap();
    let tasks_path = temp.path().join(".ralph/agent/tasks.jsonl");
    std::fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();

    let mut store = TaskStore::load(&tasks_path).unwrap();
    let task = Task::new("F1 impl".to_string(), 1).with_owner_hat(Some("executor".to_string()));
    store.add(task);
    store.save().unwrap();

    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  forge-dispatcher:
    name: "Forge Dispatcher"
    triggers: ["forge.worktrees.ready"]
    publishes: ["exec.unit.ready"]
    instructions: "Dispatch waves."
  executor:
    name: "Executor"
    triggers: ["exec.unit.ready"]
    publishes: ["exec.unit.done"]
    instructions: "Implement units."
tasks:
  enabled: true
  coordinator_hats: ["forge-dispatcher"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = temp.path().to_path_buf();

    let loop_context = crate::loop_context::LoopContext::primary(temp.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, loop_context);
    event_loop.initialize("coordinator non-self-owner read-only test");

    let prompt = event_loop
        .build_prompt(&HatId::new("forge-dispatcher"))
        .expect("dispatcher prompt");

    assert!(
        prompt.contains("<ready-tasks>"),
        "coordinator must still see the ready-tasks block; got prompt head:\n{}",
        head_chars(&prompt, 2000)
    );
    assert!(
        task_line_is_read_only(&prompt, "F1 impl"),
        "coordinator hat must see [read-only] marker on a non-self-owner task; ready block:\n{}",
        head_chars(&prompt, 4000)
    );
    assert!(
        prompt.contains("none actionable for this hat"),
        "coordinator hat with no actionable ready task must see 'none actionable' header; got prompt:\n{}",
        head_chars(&prompt, 4000)
    );
}

#[test]
fn ready_tasks_no_read_only_marker_for_coordinator_on_self_owner_task() {
    // Forward edge: a coordinator hat still sees its own ready task WITHOUT
    // a `[read-only]` marker and WITHOUT the "none actionable" header. This
    // proves the new owner-only check does not over-degrade coordinators to
    // read-only across the board — they remain actionable for tasks they
    // own.
    use crate::task::Task;
    use crate::task_store::TaskStore;

    let temp = tempfile::tempdir().unwrap();
    let tasks_path = temp.path().join(".ralph/agent/tasks.jsonl");
    std::fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();

    let mut store = TaskStore::load(&tasks_path).unwrap();
    let task = Task::new("dispatch the next wave".to_string(), 1)
        .with_owner_hat(Some("forge-dispatcher".to_string()));
    store.add(task);
    store.save().unwrap();

    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  forge-dispatcher:
    name: "Forge Dispatcher"
    triggers: ["forge.worktrees.ready"]
    publishes: ["exec.unit.ready"]
    instructions: "Dispatch waves."
  executor:
    name: "Executor"
    triggers: ["exec.unit.ready"]
    publishes: ["exec.unit.done"]
    instructions: "Implement units."
tasks:
  enabled: true
  coordinator_hats: ["forge-dispatcher"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = temp.path().to_path_buf();

    let loop_context = crate::loop_context::LoopContext::primary(temp.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, loop_context);
    event_loop.initialize("coordinator self-owner actionable test");

    let prompt = event_loop
        .build_prompt(&HatId::new("forge-dispatcher"))
        .expect("dispatcher prompt");

    assert!(
        prompt.contains("<ready-tasks>"),
        "coordinator must see the ready-tasks block; got prompt head:\n{}",
        head_chars(&prompt, 2000)
    );
    assert!(
        !task_line_is_read_only(&prompt, "dispatch the next wave"),
        "coordinator must NOT see [read-only] marker on its own task; ready block:\n{}",
        head_chars(&prompt, 4000)
    );
    assert!(
        !prompt.contains("none actionable for this hat"),
        "coordinator must NOT see the 'none actionable' header when it owns a ready task; got prompt:\n{}",
        head_chars(&prompt, 4000)
    );
}

#[test]
fn ready_tasks_mixed_owner_marks_only_non_self_read_only_for_coordinator() {
    // Mixed case: a coordinator hat has one self-owned task and one task
    // owned by another hat. The self-owned task must be unmarked; the
    // non-self-owned task must be marked `[read-only]`. Because at least
    // one ready task is actionable, the header must NOT be the
    // "none actionable" variant — it is the normal `## Tasks: N ready...`
    // header.
    use crate::task::Task;
    use crate::task_store::TaskStore;

    let temp = tempfile::tempdir().unwrap();
    let tasks_path = temp.path().join(".ralph/agent/tasks.jsonl");
    std::fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();

    let mut store = TaskStore::load(&tasks_path).unwrap();
    let self_task = Task::new("dispatch the next wave".to_string(), 1)
        .with_owner_hat(Some("forge-dispatcher".to_string()));
    let other_task =
        Task::new("F1 impl".to_string(), 1).with_owner_hat(Some("executor".to_string()));
    store.add(self_task);
    store.add(other_task);
    store.save().unwrap();

    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  forge-dispatcher:
    name: "Forge Dispatcher"
    triggers: ["forge.worktrees.ready"]
    publishes: ["exec.unit.ready"]
    instructions: "Dispatch waves."
  executor:
    name: "Executor"
    triggers: ["exec.unit.ready"]
    publishes: ["exec.unit.done"]
    instructions: "Implement units."
tasks:
  enabled: true
  coordinator_hats: ["forge-dispatcher"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = temp.path().to_path_buf();

    let loop_context = crate::loop_context::LoopContext::primary(temp.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, loop_context);
    event_loop.initialize("coordinator mixed owner test");

    let prompt = event_loop
        .build_prompt(&HatId::new("forge-dispatcher"))
        .expect("dispatcher prompt");

    assert!(
        task_line_is_read_only(&prompt, "F1 impl"),
        "non-self-owner task must carry the [read-only] marker; ready block:\n{}",
        head_chars(&prompt, 4000)
    );
    assert!(
        !task_line_is_read_only(&prompt, "dispatch the next wave"),
        "self-owner task must NOT carry the [read-only] marker; ready block:\n{}",
        head_chars(&prompt, 4000)
    );
    assert!(
        !prompt.contains("none actionable for this hat"),
        "header must be the normal variant when at least one ready task is actionable; got prompt:\n{}",
        head_chars(&prompt, 4000)
    );
}
