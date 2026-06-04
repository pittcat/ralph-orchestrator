//! Tests for scratchpad.

use super::common::*;
use super::*;

#[test]
fn test_consecutive_failures_increments_on_failed_output() {
    // Kills: line 928 `+= 1` → `-=` / `*=`
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let ralph = HatId::new("ralph");

    event_loop.process_output(&ralph, "output", false);
    assert_eq!(event_loop.state.consecutive_failures, 1);

    event_loop.process_output(&ralph, "output", false);
    assert_eq!(event_loop.state.consecutive_failures, 2);
}

#[test]
fn test_consecutive_failures_resets_on_success() {
    // Kills: line 926 reset branch
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let ralph = HatId::new("ralph");

    event_loop.process_output(&ralph, "output", false);
    assert_eq!(event_loop.state.consecutive_failures, 1);

    event_loop.process_output(&ralph, "output", true);
    assert_eq!(event_loop.state.consecutive_failures, 0);
}

#[test]
fn test_cost_based_termination() {
    // Kills: line 383 `>=` → `<`, lines 987 `add_cost` noop / `-=` / `*=`
    let yaml = r"
event_loop:
  max_cost_usd: 10.0
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.add_cost(9.99);
    assert_eq!(
        event_loop.check_termination(),
        None,
        "Should NOT terminate below max cost"
    );

    event_loop.add_cost(0.01);
    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::MaxCost),
        "Should terminate at exactly max cost"
    );
}

#[test]
fn test_malformed_events_increment_counter() {
    // Kills: line 1063 `+= 1` → `-=` / `*=`
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    // Write invalid JSONL
    std::fs::write(&events_path, "not valid json\n").unwrap();
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop.state.consecutive_malformed_events, 1,
        "First malformed line should set counter to 1"
    );

    // Write another invalid line (append)
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&events_path)
        .unwrap();
    writeln!(file, "also not json").unwrap();
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop.state.consecutive_malformed_events, 2,
        "Second malformed line should set counter to 2"
    );
}

#[test]
fn test_malformed_counter_resets_on_valid_event() {
    // Kills: line 1072 `!` deletion
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    // Write invalid JSONL
    std::fs::write(&events_path, "not valid json\n").unwrap();
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.consecutive_malformed_events, 1);

    // Write a valid event
    write_event_to_jsonl(&events_path, "build.done", "success");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop.state.consecutive_malformed_events, 0,
        "Counter should reset when valid events are parsed"
    );
}

#[test]
fn test_validation_failure_termination_at_threshold() {
    // Kills: line 1165 `>=` → `<` and `&&` → `||`
    // (Note: line 1165 refers to validation threshold at line 398)
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    event_loop.state.consecutive_malformed_events = 2;
    assert_eq!(
        event_loop.check_termination(),
        None,
        "Should NOT terminate at 2 malformed events (threshold is 3)"
    );

    event_loop.state.consecutive_malformed_events = 3;
    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::ValidationFailure),
        "Should terminate at 3 malformed events"
    );
}

#[test]
fn test_stop_requested_termination_clears_signal() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let event_loop = EventLoop::new(config);

    let stop_path = temp_dir.path().join(".ralph/stop-requested");
    std::fs::create_dir_all(stop_path.parent().unwrap()).unwrap();
    std::fs::write(&stop_path, "").unwrap();

    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::Stopped),
        "Should terminate when stop requested signal exists"
    );
    assert!(
        !stop_path.exists(),
        "Stop signal should be removed after detection"
    );
}

#[test]
fn test_format_event_wraps_top_level_prompts() {
    // Kills: line 761 `==` → `!=` and `||` → `&&`
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Build a web server");

    let ralph = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph).unwrap();

    // task.start event should be wrapped in <top-level-prompt>
    assert!(
        prompt.contains("<top-level-prompt>"),
        "task.start events should be wrapped in <top-level-prompt> tags"
    );

    // Consume the start event, publish a non-top-level event
    event_loop
        .bus
        .publish(Event::new("build.done", "completed"));
    let prompt2 = event_loop.build_prompt(&ralph).unwrap();

    // build.done is NOT a top-level prompt, should NOT have the tag
    assert!(
        !prompt2.contains("<top-level-prompt>"),
        "Non-top-level events should NOT be wrapped in <top-level-prompt> tags"
    );
}

#[test]
fn test_check_ralph_completion_detection() {
    // Kills: line 1241 return `true` / `false`
    let config = RalphConfig::default();
    let event_loop = EventLoop::new(config);

    assert!(
        event_loop.check_ralph_completion(r#"<event topic="LOOP_COMPLETE">done</event>"#),
        "Should detect completion event"
    );
    assert!(
        !event_loop.check_ralph_completion("LOOP_COMPLETE\nMore text"),
        "Completion requires emitted event, not plain text"
    );
    assert!(
        !event_loop.check_ralph_completion("no match here"),
        "Should not detect completion in unrelated text"
    );
}

#[test]
fn test_scratchpad_injection_with_content() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let scratchpad_path = temp_dir.path().join(".ralph/agent/scratchpad.md");
    std::fs::create_dir_all(scratchpad_path.parent().unwrap()).unwrap();
    std::fs::write(
        &scratchpad_path,
        "## Progress\n- [x] Step 1\n- [ ] Step 2\n",
    )
    .unwrap();

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        prompt.contains("<scratchpad"),
        "Prompt should contain scratchpad header"
    );
    assert!(
        prompt.contains("Step 1"),
        "Prompt should contain scratchpad content"
    );
    assert!(
        prompt.contains("Step 2"),
        "Prompt should contain scratchpad content"
    );
}

#[test]
fn test_scratchpad_injection_no_file() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    // Do NOT create scratchpad file

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        !prompt.contains("<scratchpad path="),
        "Prompt should NOT contain scratchpad injection when file doesn't exist"
    );
}

#[test]
fn test_scratchpad_injection_empty_file() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let scratchpad_path = temp_dir.path().join(".ralph/agent/scratchpad.md");
    std::fs::create_dir_all(scratchpad_path.parent().unwrap()).unwrap();
    std::fs::write(&scratchpad_path, "   \n\n  ").unwrap();

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        !prompt.contains("<scratchpad path="),
        "Prompt should NOT contain scratchpad injection when file is empty/whitespace"
    );
}

#[test]
fn test_scratchpad_injection_ordering() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let scratchpad_path = temp_dir.path().join(".ralph/agent/scratchpad.md");
    std::fs::create_dir_all(scratchpad_path.parent().unwrap()).unwrap();
    std::fs::write(&scratchpad_path, "scratchpad marker content").unwrap();

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    let scratchpad_pos = prompt
        .find("<scratchpad")
        .expect("Should contain scratchpad");
    let orientation_pos = prompt
        .find("### 0a. ORIENTATION")
        .expect("Should contain orientation");

    assert!(
        scratchpad_pos < orientation_pos,
        "Scratchpad should appear before ORIENTATION in the prompt"
    );
}

#[test]
fn test_scratchpad_injection_tail_truncation() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let scratchpad_path = temp_dir.path().join(".ralph/agent/scratchpad.md");
    std::fs::create_dir_all(scratchpad_path.parent().unwrap()).unwrap();

    // Create content exceeding 16000 chars (4000 tokens * 4 chars/token)
    // Include markdown headings so truncation summary captures them
    let mut large_content = String::new();
    large_content.push_str("### Initial Analysis\n\n");
    for i in 0..500 {
        large_content.push_str(&format!("Line {}: some padding content here\n", i));
    }
    large_content.push_str("### Research Phase\n\n");
    for i in 500..1000 {
        large_content.push_str(&format!("Line {}: some padding content here\n", i));
    }
    large_content.push_str("### Implementation Notes\n\n");
    for i in 1000..2000 {
        large_content.push_str(&format!("Line {}: some padding content here\n", i));
    }
    assert!(
        large_content.len() > 16000,
        "Test content should exceed budget"
    );
    std::fs::write(&scratchpad_path, &large_content).unwrap();

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        prompt.contains("<scratchpad"),
        "Prompt should contain scratchpad header even when truncated"
    );
    assert!(
        prompt.contains("earlier content truncated"),
        "Prompt should indicate truncation occurred"
    );
    // Discarded headings should be summarized
    assert!(
        prompt.contains("discarded sections:"),
        "Prompt should summarize discarded section headings"
    );
    assert!(
        prompt.contains("### Initial Analysis"),
        "Prompt should list the discarded heading"
    );
    // The tail (most recent lines) should be kept
    assert!(
        prompt.contains("Line 1999"),
        "Last line should be preserved (tail kept)"
    );
    // Early lines should be truncated
    assert!(
        !prompt.contains("Line 0:"),
        "First line should be truncated (head removed)"
    );
}
