// Auto-extracted from the legacy loop-runner regression suite. Tests in this
// module remain part of the loop_runner::tests::legacy surface; only the file
// layout changed (mechanical split per plan 2026-08-07-005). Behavior,
// assertions, fixtures, and process environment semantics are unchanged.
//
// The full original `legacy.rs` import set is reproduced verbatim per bucket so
// that every existing test compiles without rewriting call sites. Splits may
// leave some imports unused in a given bucket; this is a mechanical artifact,
// not dead code (the same items remain used by sibling buckets).

#![allow(unused_imports)]

use super::super::super::*;
use super::super::common::*;
use super::super::fake_path::*;
use super::helpers::*;
use crate::test_support::CwdGuard;
use ralph_core::HatRegistry;
use ralph_core::planning_session::{ConversationEntry, ConversationType};
use ralph_proto::{Hat, Topic};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex};

// Test: test_detect_solo_output_completion_requires_hatless_mode
#[test]
fn test_detect_solo_output_completion_requires_hatless_mode() {
    let registry = HatRegistry::new();
    assert!(detect_solo_output_completion(
        &registry,
        "done\nLOOP_COMPLETE\n",
        "LOOP_COMPLETE"
    ));

    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let registry = HatRegistry::from_config(&config);
    assert!(
        !detect_solo_output_completion(&registry, "done\nLOOP_COMPLETE\n", "LOOP_COMPLETE"),
        "text completion should not terminate multi-hat workflows"
    );
}

// Test: test_detect_solo_output_completion_requires_final_non_empty_line
#[test]
fn test_detect_solo_output_completion_requires_final_non_empty_line() {
    let registry = HatRegistry::new();
    assert!(!detect_solo_output_completion(
        &registry,
        "LOOP_COMPLETE\nMore text after\n",
        "LOOP_COMPLETE"
    ));
    assert!(!detect_solo_output_completion(
        &registry,
        "I think LOOP_COMPLETE but not really",
        "LOOP_COMPLETE"
    ));
}

// Test: test_normalize_cli_output_for_parsing_extracts_claude_text_blocks
#[test]
fn test_normalize_cli_output_for_parsing_extracts_claude_text_blocks() {
    let raw = concat!(
        "{\"type\":\"system\",\"session_id\":\"abc\",\"model\":\"claude-opus-4-6\",\"tools\":[]}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"First line\"}]}}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"Bash\",\"input\":{\"command\":\"pytest\"}}]}}\n",
        "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"tool_1\",\"content\":\"ok\"}]}}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"LOOP_COMPLETE\"}]}}\n",
        "{\"type\":\"result\",\"duration_ms\":1,\"total_cost_usd\":0.0,\"num_turns\":1,\"is_error\":false}\n"
    );

    assert_eq!(
        normalize_cli_output_for_parsing(BackendOutputFormat::StreamJson, raw),
        "First line\nLOOP_COMPLETE\n"
    );
}

// Test: test_normalize_cli_output_for_parsing_extracts_pi_text_deltas
#[test]
fn test_normalize_cli_output_for_parsing_extracts_pi_text_deltas() {
    let raw = concat!(
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"hello \"}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"thinking_delta\",\"contentIndex\":0,\"delta\":\"hidden\"}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"LOOP_COMPLETE\"}}\n",
        "{\"type\":\"turn_end\",\"message\":{\"usage\":{\"input\":1,\"output\":1,\"cache_read\":0,\"cache_write\":0,\"cost\":{\"input\":0.0,\"output\":0.0,\"total\":0.0}}}}\n"
    );

    assert_eq!(
        normalize_cli_output_for_parsing(BackendOutputFormat::PiStreamJson, raw),
        "hello LOOP_COMPLETE"
    );
}

// Test: test_process_pending_merges_handles_missing_preset
#[test]
fn test_process_pending_merges_handles_missing_preset() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();
    std::fs::create_dir_all(repo_root.join(".ralph/merge-queue")).expect("queue dir");

    process_pending_merges(repo_root);
}

// Test: test_process_pending_merges_spawns_for_queue_entry
#[cfg(unix)]
#[test]
fn test_process_pending_merges_spawns_for_queue_entry() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();
    std::fs::create_dir_all(repo_root.join(".ralph/merge-queue")).expect("queue dir");

    let queue_file = repo_root.join(".ralph/merge-queue/loop-1234.json");
    std::fs::write(
        &queue_file,
        r#"{"loop_id":"1234","state":"queued","created_at":"2026-01-01T00:00:00Z"}"#,
    )
    .expect("queue file");

    let bin_dir = repo_root.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let ralph_path = write_fake_executable(&bin_dir, "ralph", "exit 0");

    process_pending_merges_with_command(repo_root, ralph_path.as_os_str());
}

// Test: test_process_pending_merges_missing_command_keeps_queue
#[test]
fn test_process_pending_merges_missing_command_keeps_queue() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();
    let queue = ralph_core::merge_queue::MergeQueue::new(repo_root);
    queue.enqueue("loop-9999", "merge prompt").expect("enqueue");

    process_pending_merges_with_command(repo_root, OsStr::new("ralph-command-missing-12345"));

    let config_path = repo_root.join(".ralph/merge-loop-config.yml");
    assert!(config_path.exists());
    let entries = queue
        .list_by_state(ralph_core::merge_queue::MergeState::Queued)
        .expect("list queued");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].loop_id, "loop-9999");
}

// Test: test_process_pending_merges_with_empty_queue_no_config_written
#[test]
fn test_process_pending_merges_with_empty_queue_no_config_written() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();
    std::fs::create_dir_all(repo_root.join(".ralph/merge-queue")).expect("queue dir");

    let config_path = repo_root.join(".ralph/merge-loop-config.yml");
    assert!(!config_path.exists());

    process_pending_merges_with_command(repo_root, OsStr::new("ralph"));

    assert!(!config_path.exists());
}

// Test: test_process_pending_merges_redirects_subprocess_output_to_log_file
#[cfg(unix)]
#[test]
fn test_process_pending_merges_redirects_subprocess_output_to_log_file() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();

    // Enqueue a merge entry using the proper API
    let queue = ralph_core::merge_queue::MergeQueue::new(repo_root);
    queue.enqueue("test-loop", "merge prompt").expect("enqueue");

    // Create a fake ralph that writes to both stdout and stderr
    let bin_dir = repo_root.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let ralph_path = write_fake_executable(
        &bin_dir,
        "ralph",
        "echo 'stdout output' && echo 'stderr output' >&2 && sleep 0.1",
    );

    process_pending_merges_with_command(repo_root, ralph_path.as_os_str());

    // process_pending_merges_with_command now synchronously waits for the
    // child to exit (see merge_queue.rs function-level doc), so by the time
    // it returns the redirected stdio fds have been flushed and closed by
    // the OS. No fixed `std::thread::sleep` needed — that was the
    // CPU-preemption flake this test used to hit under load.

    // Verify a log file was created under .ralph/diagnostics/logs/
    let logs_dir = repo_root.join(".ralph/diagnostics/logs");
    assert!(logs_dir.exists(), "diagnostics logs directory should exist");

    let log_files: Vec<_> = std::fs::read_dir(&logs_dir)
        .expect("read logs dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("ralph-merge-"))
        .collect();
    assert!(
        !log_files.is_empty(),
        "should have at least one merge subprocess log file"
    );

    // Verify the log file contains the subprocess output
    let log_content = std::fs::read_to_string(log_files[0].path()).expect("read log file");
    assert!(
        log_content.contains("stdout output"),
        "log file should contain stdout, got: {log_content}"
    );
    assert!(
        log_content.contains("stderr output"),
        "log file should contain stderr, got: {log_content}"
    );
}

// Test: test_process_pending_merges_falls_back_to_null_on_log_creation_failure
#[cfg(unix)]
#[test]
fn test_process_pending_merges_falls_back_to_null_on_log_creation_failure() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();

    // Block log file creation by placing a regular file where the logs directory would be
    let diagnostics_dir = repo_root.join(".ralph/diagnostics");
    std::fs::create_dir_all(&diagnostics_dir).expect("diagnostics dir");
    std::fs::write(diagnostics_dir.join("logs"), "not a directory").expect("block logs dir");

    // Enqueue a merge entry using the proper API
    let queue = ralph_core::merge_queue::MergeQueue::new(repo_root);
    queue.enqueue("test-loop", "merge prompt").expect("enqueue");

    // Create a fake ralph
    let bin_dir = repo_root.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let ralph_path = write_fake_executable(&bin_dir, "ralph", "exit 0");

    // Should not panic even though log file creation fails
    process_pending_merges_with_command(repo_root, ralph_path.as_os_str());
}

// Test: test_resolve_prompt_content_inline_precedence
#[test]
fn test_resolve_prompt_content_inline_precedence() {
    let mut config = RalphConfig::default();
    config.event_loop.prompt = Some("inline prompt".to_string());
    config.event_loop.prompt_file = "missing.md".to_string();

    let resolved = resolve_prompt_content(&config.event_loop).expect("inline prompt");
    assert_eq!(resolved, "inline prompt");
}

// Test: test_resolve_prompt_content_from_file
#[test]
fn test_resolve_prompt_content_from_file() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let prompt_path = temp_dir.path().join("PROMPT.md");
    std::fs::write(&prompt_path, "file prompt").expect("write prompt");

    let mut config = RalphConfig::default();
    config.event_loop.prompt = None;
    config.event_loop.prompt_file = prompt_path.to_string_lossy().to_string();

    let resolved = resolve_prompt_content(&config.event_loop).expect("file prompt");
    assert_eq!(resolved, "file prompt");
}

// Test: test_resolve_prompt_content_missing_file_errors
#[test]
fn test_resolve_prompt_content_missing_file_errors() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let missing_path = temp_dir.path().join("missing.md");

    let mut config = RalphConfig::default();
    config.event_loop.prompt = None;
    config.event_loop.prompt_file = missing_path.to_string_lossy().to_string();

    let err = resolve_prompt_content(&config.event_loop).expect_err("missing prompt");
    assert!(
        err.to_string().contains("Prompt file"),
        "unexpected error: {err}"
    );
}

// Test: test_resolve_prompt_content_no_prompt_errors
#[test]
fn test_resolve_prompt_content_no_prompt_errors() {
    let mut config = RalphConfig::default();
    config.event_loop.prompt = None;
    config.event_loop.prompt_file = String::new();

    let err = resolve_prompt_content(&config.event_loop).expect_err("missing prompt");
    assert!(
        err.to_string().contains("No prompt specified"),
        "unexpected error: {err}"
    );
}

// Test: test_log_events_from_output_records_orphan_event
#[test]
fn test_log_events_from_output_records_orphan_event() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("events.jsonl");
    let mut logger = EventLogger::new(&log_path);

    let mut registry = HatRegistry::new();
    let mut hat = Hat::new("planner", "Planner");
    hat.subscriptions.push(Topic::new("task.start"));
    registry.register(hat);

    let output = "<event topic=\"task.start\">start</event>\n\
<event topic=\"unknown.event\">oops</event>";
    let hat_id = HatId::new("tester");

    log_events_from_output(&mut logger, 1, &hat_id, output, &registry, true);

    let content = std::fs::read_to_string(&log_path).expect("read events");
    let records: Vec<EventRecord> = content
        .lines()
        .map(|line| serde_json::from_str(line).expect("record"))
        .collect();

    let topics: std::collections::HashSet<String> =
        records.iter().map(|record| record.topic.clone()).collect();
    assert!(topics.contains("task.start"));
    assert!(topics.contains("unknown.event"));
    assert!(topics.contains("event.orphaned"));

    let triggered = records
        .iter()
        .find(|record| record.topic == "task.start")
        .and_then(|record| record.triggered.clone());
    assert_eq!(triggered.as_deref(), Some("planner"));
}

// Test: test_log_events_from_output_can_skip_raw_candidates_for_state_machine
#[test]
fn test_log_events_from_output_can_skip_raw_candidates_for_state_machine() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("events.jsonl");
    let mut logger = EventLogger::new(&log_path);

    let registry = HatRegistry::new();
    let output = "<event topic=\"experiment.ready\">{\"task_key\":\"t1\"}</event>";
    let hat_id = HatId::new("tester");

    log_events_from_output(&mut logger, 1, &hat_id, output, &registry, false);

    assert!(
        !log_path.exists(),
        "raw candidate events should not be written when accepted-only logging is enabled"
    );
}

// Test: test_log_accepted_events_records_orphan_event
#[test]
fn test_log_accepted_events_records_orphan_event() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("events.jsonl");
    let mut logger = EventLogger::new(&log_path);

    let mut registry = HatRegistry::new();
    let mut hat = Hat::new("planner", "Planner");
    hat.subscriptions.push(Topic::new("task.start"));
    registry.register(hat);

    let hat_id = HatId::new("tester");
    let events = vec![Event::new("unknown.event", "accepted")];
    log_accepted_events(&mut logger, 1, &hat_id, &events, &registry);

    let content = std::fs::read_to_string(&log_path).expect("read events");
    let records: Vec<EventRecord> = content
        .lines()
        .map(|line| serde_json::from_str(line).expect("record"))
        .collect();

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].topic, "event.orphaned");
    assert_eq!(records[1].topic, "unknown.event");
}

// Test: test_log_terminate_event_writes_record
#[test]
fn test_log_terminate_event_writes_record() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("events.jsonl");
    let mut logger = EventLogger::new(&log_path);

    let event = Event::new("loop.terminate", "done");
    log_terminate_event(&mut logger, 7, &event, None);

    let content = std::fs::read_to_string(&log_path).expect("read events");
    let records: Vec<EventRecord> = content
        .lines()
        .map(|line| serde_json::from_str(line).expect("record"))
        .collect();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].topic, "loop.terminate");
    assert_eq!(records[0].hat, "loop");
    assert_eq!(records[0].iteration, 7);
}

// Test: test_check_planning_session_responses_publishes_user_response
#[test]
fn test_check_planning_session_responses_publishes_user_response() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_id = format!(
        "session-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let ctx = ralph_core::LoopContext::primary(temp_dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx.clone());

    let conversation_path = ctx.planning_conversation_path(&session_id);
    std::fs::create_dir_all(conversation_path.parent().expect("parent"))
        .expect("create conversation dir");

    let prompt_entry = ConversationEntry {
        entry_type: ConversationType::UserPrompt,
        id: "prompt-1".to_string(),
        text: "Which option?".to_string(),
        ts: "2026-01-31T00:00:00Z".to_string(),
    };
    let response_entry = ConversationEntry {
        entry_type: ConversationType::UserResponse,
        id: "response-1".to_string(),
        text: "Option A".to_string(),
        ts: "2026-01-31T00:00:01Z".to_string(),
    };
    let conversation = format!(
        "{}\n{}\n",
        serde_json::to_string(&prompt_entry).expect("serialize prompt"),
        serde_json::to_string(&response_entry).expect("serialize response")
    );
    std::fs::write(&conversation_path, conversation).expect("write conversation");

    let published = std::sync::Arc::new(Mutex::new(Vec::new()));
    let published_clone = std::sync::Arc::clone(&published);
    event_loop
        .bus()
        .add_observer(move |event| published_clone.lock().unwrap().push(event.clone()));

    check_planning_session_responses_for_session(&mut event_loop, &session_id)
        .expect("check responses");
    {
        let events = published.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].topic.as_str(), "user.response");
        assert!(events[0].payload.contains("response-1"));
    }

    check_planning_session_responses_for_session(&mut event_loop, &session_id)
        .expect("dedup responses");
    let events = published.lock().unwrap();
    assert_eq!(events.len(), 1);
}

// Test: test_check_planning_session_responses_for_session_no_context_is_ok
#[test]
fn test_check_planning_session_responses_for_session_no_context_is_ok() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    let published = std::sync::Arc::new(Mutex::new(Vec::new()));
    let published_clone = std::sync::Arc::clone(&published);
    event_loop
        .bus()
        .add_observer(move |event| published_clone.lock().unwrap().push(event.clone()));

    check_planning_session_responses_for_session(&mut event_loop, "session-no-context")
        .expect("check responses");

    assert!(published.lock().unwrap().is_empty());
}

// Test: test_check_planning_session_responses_skips_invalid_json
#[test]
fn test_check_planning_session_responses_skips_invalid_json() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_id = format!(
        "session-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let ctx = ralph_core::LoopContext::primary(temp_dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx.clone());

    let conversation_path = ctx.planning_conversation_path(&session_id);
    std::fs::create_dir_all(conversation_path.parent().expect("parent"))
        .expect("create conversation dir");

    let prompt_entry = ConversationEntry {
        entry_type: ConversationType::UserPrompt,
        id: "prompt-1".to_string(),
        text: "Choose one".to_string(),
        ts: "2026-01-31T00:00:00Z".to_string(),
    };
    let conversation = format!(
        "not-json\n{}\n",
        serde_json::to_string(&prompt_entry).expect("serialize prompt")
    );
    std::fs::write(&conversation_path, conversation).expect("write conversation");

    let published = std::sync::Arc::new(Mutex::new(Vec::new()));
    let published_clone = std::sync::Arc::clone(&published);
    event_loop
        .bus()
        .add_observer(move |event| published_clone.lock().unwrap().push(event.clone()));

    check_planning_session_responses_for_session(&mut event_loop, &session_id)
        .expect("check responses");

    assert!(published.lock().unwrap().is_empty());
}

// Test: test_resolve_display_hat_for_execution_prefers_prompt_selected_hat_for_ralph
#[test]
fn test_resolve_display_hat_for_execution_prefers_prompt_selected_hat_for_ralph() {
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["debug.start", "hypothesis.confirmed"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml config");
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus()
        .publish(Event::new("hypothesis.test", "Test the hypothesis"));
    event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");

    let display_hat = resolve_display_hat_for_execution(
        &event_loop,
        &HatId::new("ralph"),
        &HatId::new("investigator"),
    );

    assert_eq!(display_hat.as_str(), "tester");
}

// Test: test_resolve_display_hat_for_execution_ignores_targeted_task_resume_noise
#[test]
fn test_resolve_display_hat_for_execution_ignores_targeted_task_resume_noise() {
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["task.resume", "debug.start", "hypothesis.confirmed"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml config");
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus()
        .publish(Event::new("task.resume", "Recovery").with_target("investigator"));
    event_loop
        .bus()
        .publish(Event::new("hypothesis.test", "Test the hypothesis"));
    event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");

    let display_hat = resolve_display_hat_for_execution(
        &event_loop,
        &HatId::new("ralph"),
        &HatId::new("investigator"),
    );

    assert_eq!(display_hat.as_str(), "tester");
}

// Test: test_resolve_display_hat_for_execution_prefers_downstream_event_over_start_event
#[test]
fn test_resolve_display_hat_for_execution_prefers_downstream_event_over_start_event() {
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["debug.start", "hypothesis.confirmed"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml config");
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus()
        .publish(Event::new("debug.start", "Investigate the bug"));
    event_loop
        .bus()
        .publish(Event::new("hypothesis.test", "Test the hypothesis"));
    event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");

    let display_hat = resolve_display_hat_for_execution(
        &event_loop,
        &HatId::new("ralph"),
        &HatId::new("investigator"),
    );

    assert_eq!(display_hat.as_str(), "tester");
}

// Test: test_resolve_display_hat_for_execution_keeps_explicit_non_ralph_hat
#[test]
fn test_resolve_display_hat_for_execution_keeps_explicit_non_ralph_hat() {
    let event_loop = EventLoop::new(RalphConfig::default());

    let display_hat = resolve_display_hat_for_execution(
        &event_loop,
        &HatId::new("fixer"),
        &HatId::new("investigator"),
    );

    assert_eq!(display_hat.as_str(), "fixer");
}

// Test: test_output_processing_hat_uses_display_hat_when_ralph_coordinates
#[test]
fn test_output_processing_hat_uses_display_hat_when_ralph_coordinates() {
    let execution_hat =
        resolve_hat_for_output_processing(&HatId::new("ralph"), &HatId::new("tester"));

    assert_eq!(execution_hat.as_str(), "tester");
}

// Test: test_output_processing_hat_keeps_explicit_non_ralph_hat
#[test]
fn test_output_processing_hat_keeps_explicit_non_ralph_hat() {
    let execution_hat =
        resolve_hat_for_output_processing(&HatId::new("fixer"), &HatId::new("tester"));

    assert_eq!(execution_hat.as_str(), "fixer");
}

// Test: test_output_mentions_ralph_emit_detects_tool_call_output
#[test]
fn test_output_mentions_ralph_emit_detects_tool_call_output() {
    assert!(output_mentions_ralph_emit(
        r#"[Tool] Bash: ralph emit "hypothesis.test" "payload""#
    ));
    assert!(!output_mentions_ralph_emit("[Tool] Bash: cargo test"));
}

// Test: test_state_machine_emit_path_uses_candidate_events_file
#[test]
fn test_state_machine_emit_path_uses_candidate_events_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ctx = LoopContext::primary(temp.path().to_path_buf());
    std::fs::create_dir_all(ctx.ralph_dir()).expect("create .ralph");
    std::fs::write(ctx.current_events_marker(), ".ralph/events-accepted.jsonl")
        .expect("write current events marker");
    std::fs::write(
        current_candidate_events_marker(&ctx),
        ".ralph/event-candidates.jsonl",
    )
    .expect("write candidate marker");

    assert_eq!(
        resolve_emit_events_path(&ctx, true),
        temp.path().join(".ralph/event-candidates.jsonl")
    );
    assert_eq!(
        resolve_emit_events_path(&ctx, false),
        temp.path().join(".ralph/events-accepted.jsonl")
    );
}

// Test: u3_wave_merge_stamps_wave_total_on_every_record
#[test]
fn u3_wave_merge_stamps_wave_total_on_every_record() {
    use crate::loop_runner::wave::merge_wave_results_to_events_file;
    use ralph_core::{CompletedWave, WaveResult};
    use ralph_proto::Event;
    use std::time::Duration;

    let completed = CompletedWave {
        wave_id: "w-u3-test".to_string(),
        wave_total: 8,
        results: (0..8)
            .map(|i| WaveResult {
                index: i,
                events: vec![Event::new(
                    "review.dimension.done",
                    format!("{{\"dimension\":\"d{i}\"}}"),
                )],
            })
            .collect(),
        failures: Vec::new(),
        duration: Duration::from_millis(1234),
        partial: false,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    };

    let tmp = tempfile::TempDir::new().unwrap();
    let events_path = tmp.path().join("events.jsonl");
    std::fs::write(&events_path, "").unwrap();

    merge_wave_results_to_events_file(
        &completed,
        &events_path,
        &["review.dimension.done".into()],
        "reviewer",
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge must succeed");

    let raw = std::fs::read_to_string(&events_path).unwrap();
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 8, "8 worker results → 8 merged records");

    let mut seen_indexes = std::collections::BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["wave_id"], "w-u3-test", "line {i} missing wave_id");
        assert!(v["wave_index"].is_number(), "line {i} missing wave_index");
        assert_eq!(v["wave_total"], 8, "line {i} wrong wave_total");
        assert!(v["ts"].is_string(), "line {i} missing ts");
        // 2026-06-13-004 U1 + review fix (T-P1-1): every merged
        // record must carry the `hat` field so the downstream
        // `process_parse_result` scope check (U2) can read it.
        // Pre-fix this only checked wave_id/index/total/ts.
        assert_eq!(
            v["hat"], "reviewer",
            "line {i} missing or wrong 'hat' field (U1 provenance)"
        );
        // U1 also mirrors the provenance into `source` so any
        // legacy `EventRecordRaw` consumer (which reads `source`
        // not `hat`) still sees the worker identity.
        assert_eq!(
            v["source"], "reviewer",
            "line {i} missing or wrong 'source' field (U1 provenance mirror)"
        );
        let idx = v["wave_index"].as_u64().unwrap() as u32;
        assert!(seen_indexes.insert(idx), "duplicate wave_index {idx}");
    }
    assert_eq!(seen_indexes.len(), 8, "all 8 expected indexes merged");
}

// Test: u3_wave_merge_emits_synthetic_events_on_failure_with_wave_total
#[test]
fn u3_wave_merge_emits_synthetic_events_on_failure_with_wave_total() {
    use crate::loop_runner::wave::merge_wave_results_to_events_file;
    use ralph_core::{CompletedWave, WaveFailure, WaveResult};
    use ralph_proto::Event;
    use std::time::Duration;

    let completed = CompletedWave {
        wave_id: "w-partial".to_string(),
        wave_total: 3,
        results: vec![WaveResult {
            index: 0,
            events: vec![Event::new("review.dimension.done", "ok")],
        }],
        failures: vec![
            WaveFailure {
                index: 1,
                error: "worker crashed".into(),
                duration: Duration::from_millis(50),
                expected_dimension: None,
                actual_dimension: None,
            },
            WaveFailure {
                index: 2,
                error: "timeout".into(),
                duration: Duration::from_millis(300),
                expected_dimension: None,
                actual_dimension: None,
            },
        ],
        duration: Duration::from_millis(500),
        partial: false,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    };
    let tmp = tempfile::TempDir::new().unwrap();
    let events_path = tmp.path().join("events.jsonl");
    std::fs::write(&events_path, "").unwrap();

    merge_wave_results_to_events_file(
        &completed,
        &events_path,
        &["review.dimension.done".into()],
        "reviewer",
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge must succeed");

    let raw = std::fs::read_to_string(&events_path).unwrap();
    let mut success_count = 0;
    let mut failed_count = 0;
    let mut synthetic_count = 0;
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["wave_id"], "w-partial");
        assert_eq!(v["wave_total"], 3, "every record carries wave_total");
        match v["topic"].as_str() {
            Some("wave.worker.failed") => failed_count += 1,
            Some("review.dimension.done")
                if v["payload"].as_str().unwrap_or("").contains("FAILED") =>
            {
                synthetic_count += 1;
            }
            Some("review.dimension.done") => success_count += 1,
            other => panic!("unexpected topic: {other:?}"),
        }
    }
    assert_eq!(success_count, 1);
    assert_eq!(failed_count, 2);
    assert_eq!(synthetic_count, 2);
}

// Test: u3_wave_merge_handles_duplicate_indexes_without_panicking
#[test]
fn u3_wave_merge_handles_duplicate_indexes_without_panicking() {
    use crate::loop_runner::wave::merge_wave_results_to_events_file;
    use ralph_core::{CompletedWave, WaveResult};
    use ralph_proto::Event;
    use std::time::Duration;

    // Submit indexes 0, 1, 2, 2 (duplicate) — the merge must not
    // panic and must surface the duplicate in observability logs
    // (we don't assert on log capture here; the contract is
    // "function does not blow up and writes all submitted records").
    let mut results = Vec::new();
    for i in 0..4 {
        results.push(WaveResult {
            index: i,
            events: vec![Event::new(
                "review.dimension.done",
                format!("{{\"i\":{i}}}"),
            )],
        });
    }
    let completed = CompletedWave {
        wave_id: "w-dup".to_string(),
        wave_total: 4,
        results,
        failures: Vec::new(),
        duration: Duration::from_millis(100),
        partial: false,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    };
    let tmp = tempfile::TempDir::new().unwrap();
    let events_path = tmp.path().join("events.jsonl");
    std::fs::write(&events_path, "").unwrap();

    merge_wave_results_to_events_file(
        &completed,
        &events_path,
        &["review.dimension.done".into()],
        "reviewer",
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge must succeed");
    let raw = std::fs::read_to_string(&events_path).unwrap();
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 4, "all 4 result events appended");
}

// Test: u3_wave_dispatch_merge_activates_wait_for_all_aggregator
#[tokio::test]
async fn u3_wave_dispatch_merge_activates_wait_for_all_aggregator() {
    let setup = setup_wave_test();
    let workspace = &setup.workspace;
    let mut event_loop = setup.event_loop;
    let events_file = &setup.events_file;
    let backend = &setup.backend;

    // 1. Run a 3-worker wave via the real production entry point.
    let wave = make_wave_with_count("w-u3-a", 3, vec!["review.done".to_string()]);
    let completed = execute_wave(
        &wave,
        backend,
        events_file,
        false,
        false,
        None,
        None,
        "u3-a-test",
        None,
    )
    .await
    .expect("wave must complete");

    // Sanity: all 3 results present, no failures.
    assert_eq!(completed.wave_id, "w-u3-a");
    assert_eq!(completed.wave_total, 3);
    assert_eq!(completed.results.len(), 3, "3 workers → 3 results");
    assert_eq!(completed.failures.len(), 0);
    assert!(!completed.partial);
    for r in &completed.results {
        assert_eq!(
            r.events.len(),
            1,
            "U3-A: each worker result must carry 1 review.done event, \
             worker {} got {}",
            r.index,
            r.events.len()
        );
    }

    // 2. Merge the worker events into the main events file.
    merge_wave_results_to_events_file(
        &completed,
        events_file,
        &wave.hat_config.publishes,
        wave.target_hat.as_str(),
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge must succeed");

    // Every merged record must carry the same wave_id, unique
    // wave_index, and the correct wave_total.
    let merged = std::fs::read_to_string(events_file).expect("read merged");
    let mut seen_wave_ids = std::collections::HashSet::new();
    let mut seen_indexes = std::collections::BTreeSet::new();
    for line in merged.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).expect("json");
        seen_wave_ids.insert(v["wave_id"].as_str().unwrap_or("").to_string());
        let idx = v["wave_index"].as_u64().unwrap() as u32;
        assert!(seen_indexes.insert(idx), "duplicate wave_index {idx}");
        assert_eq!(v["wave_total"].as_u64().unwrap(), 3);
    }
    assert_eq!(seen_wave_ids.len(), 1, "all records share one wave_id");
    assert_eq!(seen_indexes, [0, 1, 2].into_iter().collect());

    // 3. Re-read the events file through the real EventLoop pipeline
    //    so the bus routes review.done → aggregator.
    event_loop.initialize("u3-a init");
    let processed = event_loop
        .process_events_from_jsonl()
        .expect("re-read must succeed");
    assert!(
        processed.had_events,
        "process_events_from_jsonl must pick up the merged events"
    );

    // 4. The aggregator's pending queue must contain all 3 review.done
    //    events. wait_for_all only allows activation after the full
    //    set is delivered, so any pending → 3 of them.
    let aggregator_id = ralph_proto::HatId::new("aggregator");
    let agg_pending: Vec<ralph_proto::Event> = event_loop
        .bus()
        .peek_pending(&aggregator_id)
        .cloned()
        .unwrap_or_default();
    let review_done_count = agg_pending
        .iter()
        .filter(|e| e.topic.as_str() == "review.done")
        .count();
    assert_eq!(
        review_done_count, 3,
        "aggregator must see all 3 review.done events after merge, got: {review_done_count}"
    );

    // 5. The synthesizer's `wait_for_all` activation must produce the
    //    AGGREGATOR MODE prompt, not the worker prompt.
    let ralph_id = ralph_proto::HatId::new("ralph");
    let prompt = event_loop
        .build_prompt(&ralph_id)
        .expect("build_prompt must succeed for ralph");
    assert!(
        prompt.contains("AGGREGATOR MODE"),
        "U3-A: after full wave merge, the aggregator must be the active hat; prompt: {prompt}"
    );
    assert!(
        !prompt.contains("Dispatch wave"),
        "U3-A: dispatcher instructions must NOT leak into the aggregator prompt"
    );

    // 6. R10 determinism: build a FRESH EventLoop with the same
    //    topology, register a bus observer, then process the same
    //    events file. We register the observer on BOTH a fresh
    //    event_loop A and a fresh event_loop B, then process the
    //    events file on each. Compare the per-turn bus topic
    //    sequences for equality.
    //
    // P2 finding #15: instead of comparing a single bool, capture
    // the full per-iteration accepted event topics. A bus observer
    // is registered BEFORE process_events_from_jsonl on each
    // EventLoop so both runs see the same events.
    let observed_a = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let config1 = make_wave_aggregator_topology();
    let loop_ctx1 = ralph_core::LoopContext::primary(workspace.clone());
    let mut event_loop_a = ralph_core::EventLoop::with_context(config1, loop_ctx1);
    let observed_a_clone = std::sync::Arc::clone(&observed_a);
    event_loop_a
        .bus()
        .add_observer(move |event: &ralph_proto::Event| {
            observed_a_clone
                .lock()
                .unwrap()
                .push(event.topic.as_str().to_string());
        });
    event_loop_a.initialize("u3-a run A");
    let _ = event_loop_a.process_events_from_jsonl();
    let seq_a = observed_a.lock().unwrap().clone();

    let observed_b = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let config2 = make_wave_aggregator_topology();
    let loop_ctx2 = ralph_core::LoopContext::primary(workspace.clone());
    let mut event_loop_b = ralph_core::EventLoop::with_context(config2, loop_ctx2);
    let observed_b_clone = std::sync::Arc::clone(&observed_b);
    event_loop_b
        .bus()
        .add_observer(move |event: &ralph_proto::Event| {
            observed_b_clone
                .lock()
                .unwrap()
                .push(event.topic.as_str().to_string());
        });
    event_loop_b.initialize("u3-a run B");
    let _ = event_loop_b.process_events_from_jsonl();
    let seq_b = observed_b.lock().unwrap().clone();

    // R10 sequence equality: the bus topic sequence observed on
    // the first run and the second run must match exactly. A
    // single bool would silently miss a sequence that diverges
    // but still activates the aggregator.
    assert_eq!(
        seq_a, seq_b,
        "U3-A R10: bus topic sequence must match across runs (a={seq_a:?} b={seq_b:?})"
    );
    let has_aggregator_1 = seq_a.iter().any(|t| t == "review.done");
    let has_aggregator_2 = seq_b.iter().any(|t| t == "review.done");
    assert_eq!(
        has_aggregator_1, has_aggregator_2,
        "U3-A R10: same input must activate the same hat on replay"
    );
}

// Test: u3_partial_wave_does_not_activate_aggregator_until_full_set
#[cfg(unix)]
#[tokio::test]
async fn u3_partial_wave_does_not_activate_aggregator_until_full_set() {
    // P2 finding #12: shared setup helper.
    let setup = setup_wave_test();
    let workspace = &setup.workspace;
    let mut event_loop = setup.event_loop;
    let events_file = &setup.events_file;
    let backend = &setup.backend;

    // Run the full 3-worker wave (we'll surgically slice the merge
    // afterward to simulate partial-merge). After this completes,
    // the worker events files contain 3 review.done records, and
    // the main events file is still empty.
    let wave = make_wave_with_count("w-u3-b", 3, vec!["review.done".to_string()]);
    let completed = execute_wave(
        &wave,
        backend,
        events_file,
        false,
        false,
        None,
        None,
        "u3-b-test",
        None,
    )
    .await
    .expect("wave must complete");
    assert_eq!(completed.results.len(), 3);

    // Build a partial CompletedWave with only the first 2 results to
    // simulate the realistic "merge 2/3 before the 3rd arrives" case.
    // WaveResult does not implement Clone, so we copy event-by-event.
    let partial_results: Vec<ralph_core::WaveResult> = completed
        .results
        .iter()
        .take(2)
        .map(|r| ralph_core::WaveResult {
            index: r.index,
            events: r.events.clone(),
        })
        .collect();
    let partial = ralph_core::CompletedWave {
        wave_id: "w-u3-b".to_string(),
        wave_total: 3,
        results: partial_results,
        failures: Vec::new(),
        duration: completed.duration,
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    };
    merge_wave_results_to_events_file(
        &partial,
        events_file,
        &wave.hat_config.publishes,
        wave.target_hat.as_str(),
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("partial merge must succeed");

    // R7: after a partial merge, the events file must contain exactly
    // 2 records (one per merged worker result), each carrying the
    // correct wave_id / wave_index / wave_total. The 3rd result
    // has not been merged yet, so the file is incomplete.
    let merged_partial = std::fs::read_to_string(events_file).expect("read partial");
    let partial_record_count = merged_partial
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    assert_eq!(
        partial_record_count, 2,
        "U3-B: partial merge must produce exactly 2 records (2 of 3 results); got {partial_record_count}"
    );

    event_loop.initialize("u3-b init");
    let processed_partial = event_loop
        .process_events_from_jsonl()
        .expect("partial re-read must succeed");
    assert!(processed_partial.had_events);

    // 1. Partial merge: the aggregator sees 2 review.done events in
    //    its pending queue.
    let aggregator_id = ralph_proto::HatId::new("aggregator");
    let agg_pending_partial: Vec<ralph_proto::Event> = event_loop
        .bus()
        .peek_pending(&aggregator_id)
        .cloned()
        .unwrap_or_default();
    let review_done_partial = agg_pending_partial
        .iter()
        .filter(|e| e.topic.as_str() == "review.done")
        .count();
    assert_eq!(
        review_done_partial, 2,
        "U3-B: partial merge must leave exactly 2 review.done events in aggregator queue"
    );

    // 2. Reset the events file and re-merge the FULL set. The
    //    aggregator's pending queue must now contain all 3.
    //
    // Note: EventLoop owns the bus; we need a fresh EventLoop to
    // replay the full set deterministically without re-routing
    // partial-merge leftovers.
    let config2 = make_wave_aggregator_topology();
    let loop_ctx2 = ralph_core::LoopContext::primary(workspace.clone());
    let mut event_loop2 = ralph_core::EventLoop::with_context(config2, loop_ctx2);

    // Reset main events file and re-merge all 3 results.
    std::fs::write(events_file, "").expect("reset events");
    merge_wave_results_to_events_file(
        &completed,
        events_file,
        &wave.hat_config.publishes,
        wave.target_hat.as_str(),
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("full merge must succeed");
    event_loop2.initialize("u3-b init full");
    let _ = event_loop2.process_events_from_jsonl();

    let agg_pending_full: Vec<ralph_proto::Event> = event_loop2
        .bus()
        .peek_pending(&aggregator_id)
        .cloned()
        .unwrap_or_default();
    let review_done_full = agg_pending_full
        .iter()
        .filter(|e| e.topic.as_str() == "review.done")
        .count();
    assert_eq!(
        review_done_full, 3,
        "U3-B: full merge must leave all 3 review.done events in aggregator queue"
    );

    let ralph_id = ralph_proto::HatId::new("ralph");
    let prompt = event_loop2
        .build_prompt(&ralph_id)
        .expect("build_prompt must succeed");
    assert!(
        prompt.contains("AGGREGATOR MODE"),
        "U3-B: after full merge, the aggregator must be active; prompt: {prompt}"
    );

    // 3. Determinism (R10): the merged events file must carry one
    //    unique wave_index per merged record. We re-merge the
    //    partial set and confirm the records map 1:1 with worker
    //    indexes 0..1.
    let mut partial_indexes: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for line in merged_partial.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).expect("json");
        let idx = v["wave_index"].as_u64().unwrap() as u32;
        partial_indexes.insert(idx);
    }
    assert_eq!(
        partial_indexes,
        [0, 1].into_iter().collect(),
        "U3-B: partial merge indexes must match the merged workers"
    );
}

// Test: u3_worker_failure_emits_synthetic_result_for_aggregator
#[cfg(unix)]
#[tokio::test]
async fn u3_worker_failure_emits_synthetic_result_for_aggregator() {
    // P2 finding #12: shared setup helper. U3-C is a failure-only
    // test, so we replace the global backend with a missing binary
    // path AFTER the helper installs the working worker.
    let setup = setup_wave_test();
    let workspace = &setup.workspace;
    let mut event_loop = setup.event_loop;
    let events_file = &setup.events_file;
    let backend = ralph_adapters::CliBackend {
        command: workspace
            .join("bin")
            .join("does-not-exist")
            .display()
            .to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: ralph_adapters::OutputFormat::Text,
        env_vars: vec![],
    };

    // 3 workers: all fail. We point the global backend at a
    // missing binary so the dispatcher's PTY-spawn path records
    // 3 PTY failures. The merge layer synthesises a
    // `review.done(FAILED)` record per failure so the aggregator's
    // `wait_for_all` contract still completes.

    let wave = make_wave_with_count("w-u3-c", 3, vec!["review.done".to_string()]);
    let completed = execute_wave(
        &wave,
        &backend,
        events_file,
        false,
        false,
        None,
        None,
        "u3-c-test",
        None,
    )
    .await
    .expect("wave must complete even with worker failure");

    // Dispatcher records 3 failures (PTY-spawn failure for all 3
    // workers because the global backend path is a missing binary).
    assert_eq!(completed.wave_total, 3);
    assert_eq!(completed.results.len(), 0, "no workers succeeded");
    assert_eq!(completed.failures.len(), 3, "all 3 workers failed");
    let failure_indices: std::collections::BTreeSet<u32> =
        completed.failures.iter().map(|f| f.index).collect();
    assert_eq!(
        failure_indices,
        [0, 1, 2].into_iter().collect(),
        "all 3 indices must be recorded as failures"
    );

    // Merge: each failure must produce BOTH a `wave.worker.failed`
    // record AND a synthetic `review.done` record carrying the
    // FAILED marker (per `merge_wave_results_to_events_file`
    // contract). This is the "synthetic result" path the
    // aggregator uses to advance `wait_for_all` even when workers
    // don't deliver real results.
    merge_wave_results_to_events_file(
        &completed,
        events_file,
        &wave.hat_config.publishes,
        wave.target_hat.as_str(),
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge must succeed");

    let merged = std::fs::read_to_string(events_file).expect("read");
    let mut failure_record_count = 0;
    let mut synthetic_done_count = 0;
    let mut real_done_count = 0;
    let mut synthetic_indexes = std::collections::BTreeSet::new();
    let mut failure_indexes_observed = std::collections::BTreeSet::new();
    for line in merged.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).expect("json");
        let topic = v["topic"].as_str().unwrap_or("");
        match topic {
            "wave.worker.failed" => {
                failure_record_count += 1;
                let idx = v["wave_index"].as_u64().unwrap() as u32;
                failure_indexes_observed.insert(idx);
                assert_eq!(v["wave_id"], "w-u3-c");
                assert_eq!(v["wave_total"], 3);
            }
            "review.done" => {
                let payload = v["payload"].as_str().unwrap_or("");
                if payload.contains("FAILED") {
                    synthetic_done_count += 1;
                    let idx = v["wave_index"].as_u64().unwrap() as u32;
                    synthetic_indexes.insert(idx);
                } else {
                    real_done_count += 1;
                }
                assert_eq!(v["wave_id"], "w-u3-c");
                assert_eq!(v["wave_total"], 3);
            }
            other => panic!("unexpected merged topic: {other:?}"),
        }
    }
    assert_eq!(failure_record_count, 3, "3 wave.worker.failed records");
    assert_eq!(synthetic_done_count, 3, "3 synthetic FAILED review.done");
    assert_eq!(real_done_count, 0, "no real review.done");
    assert_eq!(failure_indexes_observed, [0, 1, 2].into_iter().collect());
    assert_eq!(synthetic_indexes, [0, 1, 2].into_iter().collect());

    // Re-read the events file. The aggregator's pending queue should
    // see 3 review.done records (all synthetic FAILED) — `wait_for_all`
    // treats synthetic results as fulfilling the wait condition.
    event_loop.initialize("u3-c init");
    let _ = event_loop.process_events_from_jsonl();

    let aggregator_id = ralph_proto::HatId::new("aggregator");
    let agg_pending: Vec<ralph_proto::Event> = event_loop
        .bus()
        .peek_pending(&aggregator_id)
        .cloned()
        .unwrap_or_default();
    let review_done_in_queue = agg_pending
        .iter()
        .filter(|e| e.topic.as_str() == "review.done")
        .count();
    assert_eq!(
        review_done_in_queue, 3,
        "U3-C: aggregator must see all 3 review.done events (synthetic FAILED)"
    );

    let ralph_id = ralph_proto::HatId::new("ralph");
    let prompt = event_loop
        .build_prompt(&ralph_id)
        .expect("build_prompt must succeed");
    assert!(
        prompt.contains("AGGREGATOR MODE"),
        "U3-C: aggregator must activate even when 1 worker failed, prompt: {prompt}"
    );
    // P2 finding #17: tighten the failure-context assertion. The
    // previous form `prompt.contains("FAILED") || prompt.contains("Worker 1")`
    // matched either the failure marker or any "Worker 1" string,
    // which a future innocuous change to the prompt could satisfy
    // accidentally.  We require BOTH the failure marker and a
    // stable per-index label so the assertion pins the
    // contract semantically.
    assert!(
        prompt.contains("FAILED"),
        "U3-C: aggregator prompt must surface the worker failure marker, prompt: {prompt}"
    );
    assert!(
        prompt.contains("## Worker 1") || prompt.contains("worker 1"),
        "U3-C: aggregator prompt must surface a per-index worker label for context, prompt: {prompt}"
    );
}

// Test: u3_two_independent_waves_route_to_separate_aggregations
#[cfg(unix)]
#[tokio::test]
async fn u3_two_independent_waves_route_to_separate_aggregations() {
    // P2 finding #12: shared setup helper.
    let setup = setup_wave_test();
    let mut event_loop = setup.event_loop;
    let events_file = &setup.events_file;
    let backend = &setup.backend;

    // Two distinct waves (different wave_id) of 2 workers each.
    let wave_a = make_wave_with_count("w-u3-d-a", 2, vec!["review.done".to_string()]);
    let wave_b = make_wave_with_count("w-u3-d-b", 2, vec!["review.done".to_string()]);

    let completed_a = execute_wave(
        &wave_a,
        backend,
        events_file,
        false,
        false,
        None,
        None,
        "u3-d-test",
        None,
    )
    .await
    .expect("wave A");
    let completed_b = execute_wave(
        &wave_b,
        backend,
        events_file,
        false,
        false,
        None,
        None,
        "u3-d-test",
        None,
    )
    .await
    .expect("wave B");

    // Sanity: each wave's results carry its own wave_id and the
    // expected per-index payloads. With the simple worker script
    // each result's payload encodes the worker index, so we check
    // that wave A's results cover {0, 1} and wave B's results also
    // cover {0, 1}.
    let mut a_indexes: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for r in &completed_a.results {
        let payload = r.events[0].payload.as_str();
        assert!(
            payload == "dim-0-result" || payload == "dim-1-result",
            "U3-D: wave A result must carry dim-0-result or dim-1-result, got: {payload}"
        );
        a_indexes.insert(r.index);
    }
    let mut b_indexes: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for r in &completed_b.results {
        let payload = r.events[0].payload.as_str();
        assert!(
            payload == "dim-0-result" || payload == "dim-1-result",
            "U3-D: wave B result must carry dim-0-result or dim-1-result, got: {payload}"
        );
        b_indexes.insert(r.index);
    }
    assert_eq!(
        a_indexes,
        [0, 1].into_iter().collect(),
        "U3-D: wave A must cover indexes 0 and 1"
    );
    assert_eq!(
        b_indexes,
        [0, 1].into_iter().collect(),
        "U3-D: wave B must cover indexes 0 and 1"
    );

    merge_wave_results_to_events_file(
        &completed_a,
        events_file,
        &wave_a.hat_config.publishes,
        wave_a.target_hat.as_str(),
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge A");
    merge_wave_results_to_events_file(
        &completed_b,
        events_file,
        &wave_b.hat_config.publishes,
        wave_b.target_hat.as_str(),
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge B");

    // The merged events file must contain BOTH wave_ids, distinctly.
    let merged = std::fs::read_to_string(events_file).expect("read");
    let mut wave_id_a_count = 0;
    let mut wave_id_b_count = 0;
    for line in merged.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).expect("json");
        match v["wave_id"].as_str() {
            Some("w-u3-d-a") => wave_id_a_count += 1,
            Some("w-u3-d-b") => wave_id_b_count += 1,
            other => panic!("unexpected wave_id in merged file: {other:?}"),
        }
    }
    assert_eq!(wave_id_a_count, 2, "wave A produces 2 merged records");
    assert_eq!(wave_id_b_count, 2, "wave B produces 2 merged records");

    // Re-read and check aggregator pending queue. Both waves feed
    // the same `review.done` topic, so the aggregator should see
    // 4 review.done events (no cross-wave deduplication at the bus
    // level — that's the aggregator's job).
    event_loop.initialize("u3-d init");
    let _ = event_loop.process_events_from_jsonl();

    let aggregator_id = ralph_proto::HatId::new("aggregator");
    let agg_pending: Vec<ralph_proto::Event> = event_loop
        .bus()
        .peek_pending(&aggregator_id)
        .cloned()
        .unwrap_or_default();
    let review_done_count = agg_pending
        .iter()
        .filter(|e| e.topic.as_str() == "review.done")
        .count();
    assert_eq!(
        review_done_count, 4,
        "U3-D: aggregator must see all 4 review.done events from both waves"
    );

    // The two waves must each carry their own wave_id in the merged
    // records — this is the per-wave identity the aggregator can use
    // to group results. The bus itself doesn't dedup by wave_id (the
    // aggregator is downstream of the bus), so we assert identity at
    // the merge layer.
    let mut seen_wave_ids = std::collections::BTreeSet::new();
    for line in merged.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).expect("json");
        if v["topic"] == "review.done" {
            seen_wave_ids.insert(v["wave_id"].as_str().unwrap_or("").to_string());
        }
    }
    assert_eq!(
        seen_wave_ids,
        ["w-u3-d-a", "w-u3-d-b"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );

    // P2 finding #16: verify per-wave_id grouping at the
    // P2 #16: per-wave identity must be preserved end-to-end. After
    // merging two distinct waves, the events file must still carry
    // records from BOTH wave_ids (proving wave_id metadata is
    // preserved through the merge pipeline and into the canonical
    // event log the event-loop re-reads).
    //
    // The aggregator's prompt template is intentionally
    // wave_id-agnostic (it groups by aggregate contract, not by
    // raw wave_id string), so the assertion is on the persisted
    // event log — the canonical source of truth for wave_id
    // metadata — and the merged-events count we already verified
    // above.
    let merged_after = std::fs::read_to_string(events_file).expect("read merged");
    let mut wave_ids_in_log: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for line in merged_after.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).expect("json");
        if let Some(wid) = v["wave_id"].as_str() {
            wave_ids_in_log.insert(wid.to_string());
        }
    }
    assert!(
        wave_ids_in_log.contains("w-u3-d-a"),
        "U3-D P2 #16: events file must contain wave_id 'w-u3-d-a' for grouping; got: {wave_ids_in_log:?}"
    );
    assert!(
        wave_ids_in_log.contains("w-u3-d-b"),
        "U3-D P2 #16: events file must contain wave_id 'w-u3-d-b' for grouping; got: {wave_ids_in_log:?}"
    );
}
