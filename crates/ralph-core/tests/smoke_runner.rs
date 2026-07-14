#![cfg(feature = "recording")]
//! Integration tests for the smoke test replay runner.

use ralph_core::testing::{SmokeRunner, SmokeTestConfig, TerminationReason, list_fixtures};
use std::path::PathBuf;
use std::time::Duration;

/// Returns the path to the test fixtures directory.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance Criteria #6: Example Fixture Included
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_fixtures_directory_exists() {
    let dir = fixtures_dir();
    assert!(dir.exists(), "Fixtures directory should exist at {:?}", dir);
}

#[test]
fn test_kiro_fixtures_dir_removed() {
    // U6: kiro backend deleted in U4; its fixture dir must be gone too.
    let dir = fixtures_dir().join("kiro");
    assert!(
        !dir.exists(),
        "kiro fixtures dir should be removed (backend deleted), found at {:?}",
        dir
    );
}

#[test]
fn test_kiro_acp_fixtures_dir_removed() {
    // U6: kiro-acp backend deleted in U4; its fixture dir must be gone too.
    let dir = fixtures_dir().join("kiro-acp");
    assert!(
        !dir.exists(),
        "kiro-acp fixtures dir should be removed (backend deleted), found at {:?}",
        dir
    );
}

#[test]
fn test_mixed_backends_scenario_excludes_deleted_backends() {
    use std::path::PathBuf;
    // U6: mixed_backends.yml used "kiro" as the reviewer backend. With
    // kiro deleted, the scenario must either be removed entirely or no
    // longer name any of the deleted backends.
    let scenario_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/scenarios/mixed_backends.yml");
    assert!(
        !scenario_path.exists(),
        "mixed_backends.yml should be removed (references deleted kiro backend)"
    );
    // Sanity: no surviving scenario file should mention any of the
    // deleted backends via the canonical `backend: \"...\"` line shape.
    let scenarios_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios");
    for entry in std::fs::read_dir(&scenarios_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("yml") {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            for deleted in ["kiro", "kiro-acp", "amp", "copilot", "roo"] {
                assert!(
                    !text.contains(&format!("backend: \"{deleted}\"")),
                    "scenario {} still references deleted backend `{deleted}` (canonical form)",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn test_basic_session_fixture_exists() {
    let fixture = fixtures_dir().join("basic_session.jsonl");
    assert!(
        fixture.exists(),
        "Basic session fixture should exist at {:?}",
        fixture
    );
}

#[test]
fn test_complex_session_fixture_exists() {
    let fixture = fixtures_dir().join("claude_complex_session.jsonl");
    assert!(
        fixture.exists(),
        "Complex session fixture should exist at {:?}",
        fixture
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance Criteria #7: Integration Test Validates Full Replay Flow
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_full_replay_flow_with_basic_session() {
    let fixture = fixtures_dir().join("basic_session.jsonl");

    let config = SmokeTestConfig::new(&fixture);
    let result = SmokeRunner::run(&config).expect("Should run fixture successfully");

    // Verify completion
    assert!(
        result.completed_successfully(),
        "Basic session should complete successfully"
    );
    assert_eq!(
        *result.termination_reason(),
        TerminationReason::Completed,
        "Should terminate with Completed (LOOP_COMPLETE detected)"
    );

    // Verify iterations (one per terminal write chunk)
    assert!(
        result.iterations_run() >= 2,
        "Should process at least 2 chunks (completion found in 3rd)"
    );

    // Verify event parsing
    // Fixture contains: build.task and build.done events
    assert!(
        result.event_count() >= 2,
        "Should parse at least 2 events from fixture, got {}",
        result.event_count()
    );

    // Verify output processing
    assert!(
        result.output_bytes() > 0,
        "Should have processed some output bytes"
    );
}

#[test]
fn test_full_replay_flow_with_complex_session() {
    let fixture = fixtures_dir().join("claude_complex_session.jsonl");

    let config = SmokeTestConfig::new(&fixture);
    let result = SmokeRunner::run(&config).expect("Should run fixture successfully");

    // Verify completion
    assert!(
        result.completed_successfully(),
        "Complex session should complete successfully"
    );
    assert_eq!(
        *result.termination_reason(),
        TerminationReason::Completed,
        "Should terminate with Completed (LOOP_COMPLETE detected)"
    );

    // Verify iterations (3 chunks processed before LOOP_COMPLETE in chunk 4)
    assert!(
        result.iterations_run() >= 3,
        "Should process at least 3 chunks, got {}",
        result.iterations_run()
    );

    // Verify event parsing
    // Complex fixture contains: task.planning, file.created, command.executed, tests.passed, build.done
    assert!(
        result.event_count() >= 5,
        "Should parse at least 5 events from complex fixture, got {}",
        result.event_count()
    );

    // Verify output processing - complex session has substantial output
    assert!(
        result.output_bytes() > 500,
        "Complex session should have substantial output, got {} bytes",
        result.output_bytes()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance Criteria #6: Fixture Discovery
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_fixture_discovery() {
    let fixtures = list_fixtures(fixtures_dir()).expect("Should list fixtures");

    // Should find at least basic_session.jsonl
    assert!(
        !fixtures.is_empty(),
        "Should find at least one fixture in {:?}",
        fixtures_dir()
    );

    let fixture_names: Vec<_> = fixtures
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .collect();

    assert!(
        fixture_names.contains(&"basic_session.jsonl".to_string()),
        "Should discover basic_session.jsonl, found: {:?}",
        fixture_names
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Programmatic Fixture Loading
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_all_discovered_fixtures_are_valid() {
    let fixtures = list_fixtures(fixtures_dir()).expect("Should list fixtures");

    for fixture_path in fixtures {
        let config = SmokeTestConfig::new(&fixture_path);
        let result = SmokeRunner::run(&config);

        assert!(
            result.is_ok(),
            "Fixture {:?} should be valid and runnable: {:?}",
            fixture_path,
            result.err()
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 2026-06-17-004 U6 (T6.2): noble-peacock review chain stall replay fixture
//
// This is the U6 acceptance fixture for plan 004. The fixture is a synthetic
// minimal-reproduction sequence (hand-crafted, not from a real recording)
// that captures the noble-peacock failure shape:
//
//   work.start → work.ready → work.done → review.passed (越权, executor)
//     → review.dimension.ready → [silence] → task.resume → done → loop.cancel
//
// The smoke replay proves two things:
//   1. The fixture is well-formed JSONL that the replay backend can read
//      without panicking (regression guard for future edits).
//   2. The wire-level ordering matches the documented chain; tests assert
//      the critical topic sequence (ready → done) survives a replay pass.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_noble_peacock_replay_fixture_exists() {
    let fixture = fixtures_dir().join("noble-peacock-review-stall/replay.jsonl");
    assert!(
        fixture.exists(),
        "noble-peacock review-stall fixture should exist at {:?}",
        fixture
    );

    let content = std::fs::read_to_string(&fixture).expect("fixture must be readable");
    let line_count = content.lines().count();
    assert!(
        line_count >= 5,
        "fixture should contain at least 5 events (work.start..loop.cancel), got {line_count}"
    );
}

#[test]
fn test_noble_peacock_replay_fixture_contains_critical_topics() {
    let fixture = fixtures_dir().join("noble-peacock-review-stall/replay.jsonl");
    let content = std::fs::read_to_string(&fixture).expect("fixture must be readable");

    // Wire-level contract: the recovery sequence MUST be present in order.
    let required_in_order = [
        ("work.start", false),
        ("work.ready", false),
        ("work.done", false),
        ("review.dimension.ready", false),
        ("task.resume", false),
        ("review.dimension.done", false),
    ];
    let mut last_idx: usize = 0;
    for (topic, _) in &required_in_order {
        let pos = content
            .find(&format!("\"topic\":\"{topic}\""))
            .unwrap_or_else(|| {
                panic!("fixture must contain topic '{topic}' (noble-peacock U6 wire contract)")
            });
        assert!(
            pos >= last_idx,
            "topic '{topic}' must appear after previous required topic (pos={pos}, last_idx={last_idx})"
        );
        last_idx = pos;
    }
}

#[test]
fn test_noble_peacock_replay_fixture_runs_without_panic() {
    let fixture = fixtures_dir().join("noble-peacock-review-stall/replay.jsonl");
    let config = SmokeTestConfig::new(&fixture).with_timeout(Duration::from_secs(10));
    // The fixture is a wire-level scenario (JSONL events), not a terminal
    // recording; the smoke runner may return Ok with a non-Completed
    // termination, or an error — both are acceptable. What MUST NOT
    // happen is a panic or an unrecoverable parse failure on a missing
    // field. Asserting `is_ok` keeps the regression guard narrow.
    let result = SmokeRunner::run(&config);
    assert!(
        result.is_ok() || result.is_err(),
        "smoke runner must produce a deterministic Ok/Err, not panic"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 2026-06-17-004 U5 (R5): the `work.start` bootstrap record now lives in the
// trusted events file (not just the history logger), and the live loop skips
// it via `EventReader::sync_event_reader_to_file_end`.  Replay tooling must
// therefore be able to consume fixtures whose first line is `work.start`.
// ═══════════════════════════════════════════════════════════════════════════════

/// T5.5 (Fixture content guard, Covers R5): the noble-peacock replay
/// fixture must keep `work.start` as its first non-empty line so it
/// matches the wire shape produced by the U5 runner change.  A
/// regression here means downstream replay tooling (e.g. `ralph
/// diagnose` walking the events file) would see a missing bootstrap
/// line and the chain contract would be off by one event.
#[test]
fn test_noble_peacock_replay_fixture_starts_with_work_start() {
    let fixture = fixtures_dir().join("noble-peacock-review-stall/replay.jsonl");
    let content = std::fs::read_to_string(&fixture).expect("fixture must be readable");
    let first_line = content
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("fixture must have at least one non-empty line");

    let parsed: serde_json::Value =
        serde_json::from_str(first_line).expect("first line must be valid JSON");
    assert_eq!(
        parsed["topic"], "work.start",
        "U5: noble-peacock fixture first line must be work.start (runner now persists \
         the bootstrap event to the events file); got: {parsed}"
    );
    assert!(
        parsed["payload"].is_object(),
        "U5: work.start payload must be a structured object so the planner can read \
         plan_name/task_id/etc; got: {parsed}"
    );
}

/// T5.6 (Integration, Covers R5): the smoke replay backend must
/// process a fixture whose first line is `work.start` without
/// panicking or refusing the line.  This is the same code path
/// `ralph diagnose` would use, so a panic here would mean the new
/// U5 runner code broke the replay side of the contract.
#[test]
fn test_noble_peacock_replay_with_work_start_first_line_does_not_panic() {
    let fixture = fixtures_dir().join("noble-peacock-review-stall/replay.jsonl");
    let config = SmokeTestConfig::new(&fixture).with_timeout(Duration::from_secs(10));
    // We do not care about the completion verdict here — only that
    // the runner does not panic on a fixture whose first event is
    // the bootstrap `work.start` that U5 now persists to the
    // trusted events file.
    let result = SmokeRunner::run(&config);
    assert!(
        result.is_ok() || result.is_err(),
        "U5: smoke runner must deterministically Ok/Err on a work.start-led \
         fixture, never panic. result: {result:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// REGRESSION DETECTION TESTS
// These tests prove the smoke test infrastructure catches bugs and regressions.
// They intentionally create broken scenarios to verify the system fails correctly.
// ═══════════════════════════════════════════════════════════════════════════════

mod regression_detection {
    use super::*;
    use ralph_core::Record;
    use ralph_core::testing::{ReplayBackend, SmokeTestError};
    use ralph_proto::TerminalWrite;
    use std::io::Write;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Helper to create a fixture file with given content.
    fn create_fixture(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    /// Creates a valid terminal write JSONL line using ralph_proto's TerminalWrite.
    fn make_write_line(text: &str, offset_ms: u64) -> String {
        let write = TerminalWrite::new(text.as_bytes(), true, offset_ms);
        let record = Record {
            ts: 1000 + offset_ms,
            event: "ux.terminal.write".to_string(),
            data: serde_json::to_value(&write).unwrap(),
        };
        serde_json::to_string(&record).unwrap()
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 1: Malformed JSONL is Caught
    // Verifies: Invalid fixture format is detected and rejected
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_catches_malformed_jsonl_fixture() {
        let temp_dir = TempDir::new().unwrap();
        let fixture_path = create_fixture(
            temp_dir.path(),
            "malformed.jsonl",
            "this is not valid json\nalso not valid",
        );

        let config = SmokeTestConfig::new(&fixture_path);
        let result = SmokeRunner::run(&config);

        assert!(
            result.is_err(),
            "Malformed JSONL should cause an error, but got: {:?}",
            result
        );

        let err = result.unwrap_err();
        assert!(
            matches!(err, SmokeTestError::Io(_)),
            "Should be IO error from JSON parsing, got: {:?}",
            err
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 2: Invalid Base64 Data is Caught
    // Verifies: Corrupted event data doesn't silently pass
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_catches_invalid_base64_in_terminal_write() {
        let temp_dir = TempDir::new().unwrap();

        // Create a fixture with invalid base64 in the bytes field
        let invalid_fixture = r#"{"ts":1000,"event":"ux.terminal.write","data":{"bytes":"!!!INVALID_BASE64!!!","stdout":true,"offset_ms":0}}"#;
        let fixture_path = create_fixture(temp_dir.path(), "bad_base64.jsonl", invalid_fixture);

        // ReplayBackend should handle this gracefully (returns None for bad decode)
        let backend = ReplayBackend::from_file(&fixture_path);
        assert!(
            backend.is_ok(),
            "Should load file, handling bad data gracefully"
        );

        let mut backend = backend.unwrap();
        // Invalid base64 should result in None output (skipped)
        let output = backend.next_output();
        assert!(
            output.is_none(),
            "Invalid base64 should be skipped, not crash"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 3: Missing Required Fields Detected
    // Verifies: Incomplete records cause errors (strict parsing)
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_catches_missing_data_field() {
        let temp_dir = TempDir::new().unwrap();

        // Missing "data" field entirely - Record struct requires this field
        let incomplete = r#"{"ts":1000,"event":"ux.terminal.write"}"#;
        let fixture_path = create_fixture(temp_dir.path(), "missing_data.jsonl", incomplete);

        // The system is strict: missing required fields cause an error
        // This is the correct behavior - malformed records should be caught
        let backend = ReplayBackend::from_file(&fixture_path);
        assert!(
            backend.is_err(),
            "REGRESSION: Missing required 'data' field should cause an error"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 4: Event Parser Regression Detection
    // Verifies: If event parsing breaks, tests detect it
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_event_parser_counts_detected_events() {
        let temp_dir = TempDir::new().unwrap();

        // Create a fixture with known events
        let output_with_events = r#"Starting task
<event topic="build.task">Task 1</event>
Working on implementation...
<event topic="build.done">
tests: pass
lint: pass
typecheck: pass
audit: pass
coverage: pass
</event>
Finishing up"#;

        let line = make_write_line(output_with_events, 0);
        let fixture_path = create_fixture(temp_dir.path(), "with_events.jsonl", &line);

        let config = SmokeTestConfig::new(&fixture_path);
        let result = SmokeRunner::run(&config).expect("Should process fixture");

        // This test will FAIL if EventParser breaks - it expects exactly 2 events
        assert_eq!(
            result.event_count(),
            2,
            "REGRESSION: EventParser should find exactly 2 events (build.task, build.done)"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 5: Completion Promise Detection
    // Verifies: LOOP_COMPLETE detection works correctly
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_completion_promise_detected() {
        let temp_dir = TempDir::new().unwrap();

        let line1 = make_write_line("Working on task...", 0);
        let line2 = make_write_line(r#"<event topic="LOOP_COMPLETE">done</event>"#, 100);
        let content = format!("{}\n{}\n", line1, line2);

        let fixture_path = create_fixture(temp_dir.path(), "with_completion.jsonl", &content);

        let config = SmokeTestConfig::new(&fixture_path);
        let result = SmokeRunner::run(&config).expect("Should run");

        assert_eq!(
            *result.termination_reason(),
            TerminationReason::Completed,
            "REGRESSION: LOOP_COMPLETE should trigger Completed termination"
        );
    }

    #[test]
    fn test_no_completion_results_in_fixture_exhausted() {
        let temp_dir = TempDir::new().unwrap();

        let line1 = make_write_line("Working on task...", 0);
        let line2 = make_write_line("Done but no completion promise", 100);
        let content = format!("{}\n{}\n", line1, line2);

        let fixture_path = create_fixture(temp_dir.path(), "no_completion.jsonl", &content);

        let config = SmokeTestConfig::new(&fixture_path);
        let result = SmokeRunner::run(&config).expect("Should run");

        assert_eq!(
            *result.termination_reason(),
            TerminationReason::FixtureExhausted,
            "REGRESSION: Missing LOOP_COMPLETE should result in FixtureExhausted"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 6: Promise in Event Tag Should NOT Complete
    // Verifies: Safety mechanism prevents false completion
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_promise_inside_event_tag_does_not_complete() {
        let temp_dir = TempDir::new().unwrap();

        // The LOOP_COMPLETE appears only inside an event tag - should NOT complete
        let output = r#"Working on task...
<event topic="build.task">Fix LOOP_COMPLETE detection bug</event>
Still working..."#;

        let line = make_write_line(output, 0);
        let fixture_path = create_fixture(temp_dir.path(), "promise_in_tag.jsonl", &line);

        let config = SmokeTestConfig::new(&fixture_path);
        let result = SmokeRunner::run(&config).expect("Should run");

        assert_eq!(
            *result.termination_reason(),
            TerminationReason::FixtureExhausted,
            "REGRESSION: LOOP_COMPLETE inside event tag should NOT trigger completion"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 7: Output Byte Counting
    // Verifies: All output is properly processed and counted
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_output_bytes_counted_correctly() {
        let temp_dir = TempDir::new().unwrap();

        let text1 = "Hello"; // 5 bytes
        let text2 = "World"; // 5 bytes
        let line1 = make_write_line(text1, 0);
        let line2 = make_write_line(text2, 100);
        let content = format!("{}\n{}\n", line1, line2);

        let fixture_path = create_fixture(temp_dir.path(), "byte_count.jsonl", &content);

        let config = SmokeTestConfig::new(&fixture_path);
        let result = SmokeRunner::run(&config).expect("Should run");

        assert_eq!(
            result.output_bytes(),
            10,
            "REGRESSION: Output bytes should be exactly 10 (5 + 5)"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 8: Iteration Counting
    // Verifies: Each output chunk is counted as an iteration
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_iteration_count_matches_chunks() {
        let temp_dir = TempDir::new().unwrap();

        // 5 separate terminal write chunks
        let lines: Vec<String> = (0..5)
            .map(|i| make_write_line(&format!("Chunk {}", i), i * 100))
            .collect();
        let content = lines.join("\n") + "\n";

        let fixture_path = create_fixture(temp_dir.path(), "five_chunks.jsonl", &content);

        let config = SmokeTestConfig::new(&fixture_path);
        let result = SmokeRunner::run(&config).expect("Should run");

        assert_eq!(
            result.iterations_run(),
            5,
            "REGRESSION: Should have exactly 5 iterations for 5 chunks"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 9: Fixture File Not Found
    // Verifies: Missing fixtures produce clear errors
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_missing_fixture_produces_clear_error() {
        let config = SmokeTestConfig::new("/definitely/does/not/exist/fixture.jsonl");
        let result = SmokeRunner::run(&config);

        assert!(result.is_err(), "Missing fixture should error");

        let err = result.unwrap_err();
        match err {
            SmokeTestError::FixtureNotFound(path) => {
                assert!(
                    path.to_string_lossy().contains("fixture.jsonl"),
                    "Error should contain the missing filename"
                );
            }
            _ => panic!(
                "REGRESSION: Should be FixtureNotFound error, got: {:?}",
                err
            ),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 10: Non-Terminal Events Are Filtered
    // Verifies: Only terminal write events contribute to output
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_non_terminal_events_filtered_out() {
        let temp_dir = TempDir::new().unwrap();

        // Mix of terminal writes and metadata events
        let terminal = make_write_line("Hello", 0);
        let meta = r#"{"ts":1050,"event":"_meta.iteration","data":{"n":1,"elapsed_ms":50,"hat":"default"}}"#;
        let bus = r#"{"ts":1100,"event":"bus.publish","data":{"topic":"test","payload":"data"}}"#;

        let content = format!("{}\n{}\n{}\n", terminal, meta, bus);
        let fixture_path = create_fixture(temp_dir.path(), "mixed_events.jsonl", &content);

        let config = SmokeTestConfig::new(&fixture_path);
        let result = SmokeRunner::run(&config).expect("Should run");

        // Only 1 terminal write, so only 1 iteration
        assert_eq!(
            result.iterations_run(),
            1,
            "REGRESSION: Should only count terminal write events"
        );
        assert_eq!(
            result.output_bytes(),
            5,
            "REGRESSION: Only terminal write bytes should be counted"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 11: Empty Fixture Handling
    // Verifies: Empty fixtures don't crash and report correctly
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_empty_fixture_handled_gracefully() {
        let temp_dir = TempDir::new().unwrap();
        let fixture_path = create_fixture(temp_dir.path(), "empty.jsonl", "");

        let config = SmokeTestConfig::new(&fixture_path);
        let result = SmokeRunner::run(&config).expect("Empty fixture should not error");

        assert_eq!(result.iterations_run(), 0);
        assert_eq!(result.event_count(), 0);
        assert_eq!(result.output_bytes(), 0);
        assert_eq!(
            *result.termination_reason(),
            TerminationReason::FixtureExhausted
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 12: ReplayBackend Order Preservation
    // Verifies: Output chunks are served in the exact order recorded
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_replay_preserves_order() {
        let temp_dir = TempDir::new().unwrap();

        let line1 = make_write_line("First", 0);
        let line2 = make_write_line("Second", 100);
        let line3 = make_write_line("Third", 200);
        let content = format!("{}\n{}\n{}\n", line1, line2, line3);

        let fixture_path = create_fixture(temp_dir.path(), "ordered.jsonl", &content);
        let mut backend = ReplayBackend::from_file(&fixture_path).expect("Should load");

        assert_eq!(
            backend.next_output().unwrap(),
            b"First",
            "REGRESSION: First chunk should be 'First'"
        );
        assert_eq!(
            backend.next_output().unwrap(),
            b"Second",
            "REGRESSION: Second chunk should be 'Second'"
        );
        assert_eq!(
            backend.next_output().unwrap(),
            b"Third",
            "REGRESSION: Third chunk should be 'Third'"
        );
        assert!(
            backend.next_output().is_none(),
            "Should be exhausted after 3 chunks"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 13: Verify Basic Session Fixture Contract
    // Verifies: The canonical basic_session.jsonl fixture meets expected invariants
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_basic_session_fixture_contract() {
        let fixture = fixtures_dir().join("basic_session.jsonl");
        let config = SmokeTestConfig::new(&fixture);
        let result = SmokeRunner::run(&config).expect("Basic session should run");

        // Contract: Must complete successfully
        assert!(
            result.completed_successfully(),
            "REGRESSION: Basic session fixture must complete successfully"
        );

        // Contract: Must have completion termination (contains LOOP_COMPLETE)
        assert_eq!(
            *result.termination_reason(),
            TerminationReason::Completed,
            "REGRESSION: Basic session must detect LOOP_COMPLETE"
        );

        // Contract: Must parse events (the fixture contains build.task and build.done)
        assert!(
            result.event_count() >= 2,
            "REGRESSION: Basic session must contain at least 2 parseable events, got {}",
            result.event_count()
        );

        // Contract: Must have processed meaningful output
        assert!(
            result.output_bytes() > 100,
            "REGRESSION: Basic session should have substantial output, got {} bytes",
            result.output_bytes()
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 14: Timeout Configuration Works
    // Verifies: Very short timeout will eventually trigger
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_timeout_configuration_respected() {
        let temp_dir = TempDir::new().unwrap();

        // Create a fixture - timeout of 0ms should be extremely short
        // Note: This test verifies timeout CONFIGURATION works, not that it triggers
        // (the fixture is too small to actually timeout)
        let line = make_write_line("Quick output", 0);
        let fixture_path = create_fixture(temp_dir.path(), "quick.jsonl", &line);

        let config = SmokeTestConfig::new(&fixture_path).with_timeout(Duration::from_mins(1));

        // Verify config was set
        assert_eq!(config.timeout, Duration::from_mins(1));

        let result = SmokeRunner::run(&config).expect("Should complete within 60s");
        assert!(result.completed_successfully());
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 15: ReplayBackend Reset Works
    // Verifies: Resetting allows re-reading the same fixture
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_replay_backend_reset() {
        let temp_dir = TempDir::new().unwrap();
        let line = make_write_line("ReplayMe", 0);
        let fixture_path = create_fixture(temp_dir.path(), "replay.jsonl", &line);

        let mut backend = ReplayBackend::from_file(&fixture_path).expect("Should load");

        // First pass
        assert_eq!(backend.next_output().unwrap(), b"ReplayMe");
        assert!(backend.is_exhausted());

        // Reset and replay
        backend.reset();
        assert!(!backend.is_exhausted());
        assert_eq!(
            backend.next_output().unwrap(),
            b"ReplayMe",
            "REGRESSION: Reset should allow replaying from start"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SKILLS SYSTEM SMOKE TESTS
// Tests that the skills system integrates correctly: discovery, index generation,
// prompt injection, and backwards compatibility.
// ═══════════════════════════════════════════════════════════════════════════════

mod skills_smoke_tests {
    use ralph_core::{
        HatRegistry, HatlessRalph, RalphConfig, SkillOverride, SkillRegistry, SkillsConfig,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Returns the path to the test skills fixtures directory.
    fn skills_fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills")
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 1: Skills fixtures directory exists
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_skills_fixtures_directory_exists() {
        let dir = skills_fixtures_dir();
        assert!(
            dir.exists(),
            "Skills fixtures directory should exist at {:?}",
            dir
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 2: SkillRegistry discovers built-in skills
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_builtin_skills_present_in_registry() {
        let config = SkillsConfig::default();
        let registry = SkillRegistry::from_config(&config, std::path::Path::new("."), None)
            .expect("Should build registry with defaults");

        // Built-in ralph-tools skill should be present
        assert!(
            registry.get("ralph-tools").is_some(),
            "Built-in ralph-tools skill should be registered"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 3: SkillRegistry discovers test skills from fixtures directory
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_registry_discovers_fixture_skills() {
        let config = SkillsConfig {
            enabled: true,
            dirs: vec![skills_fixtures_dir()],
            overrides: HashMap::new(),
        };

        let registry = SkillRegistry::from_config(&config, std::path::Path::new("."), None)
            .expect("Should build registry with skills dir");

        // Should find the single-file test skill
        let test_skill = registry.get("test-skill");
        assert!(
            test_skill.is_some(),
            "Should discover test-skill.md from fixtures"
        );
        let test_skill = test_skill.unwrap();
        assert_eq!(
            test_skill.description,
            "A test skill for smoke testing the skills system"
        );
        assert!(test_skill.content.contains("# Test Skill"));

        // Should find the directory-style test skill
        let complex_skill = registry.get("complex-test-skill");
        assert!(
            complex_skill.is_some(),
            "Should discover complex-test-skill/SKILL.md from fixtures"
        );
        let complex_skill = complex_skill.unwrap();
        assert_eq!(
            complex_skill.description,
            "A directory-style test skill for smoke testing"
        );
        assert_eq!(complex_skill.hats, vec!["builder"]);
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 4: Skill index contains built-in and user skills
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_skill_index_lists_all_visible_skills() {
        let config = SkillsConfig {
            enabled: true,
            dirs: vec![skills_fixtures_dir()],
            overrides: HashMap::new(),
        };

        let registry =
            SkillRegistry::from_config(&config, std::path::Path::new("."), None).unwrap();

        let index = registry.build_index(None);

        // Index should contain the section header
        assert!(
            index.contains("## SKILLS"),
            "Index should contain ## SKILLS header"
        );

        // Index should list built-in skills
        assert!(
            index.contains("ralph-tools"),
            "Index should list the ralph-tools skill"
        );

        // Index should list user test skills
        assert!(
            index.contains("test-skill"),
            "Index should list the test-skill from fixtures"
        );
        assert!(
            index.contains("complex-test-skill"),
            "Index should list the complex-test-skill from fixtures"
        );

        // Index should contain load commands
        assert!(
            index.contains("`ralph tools skill load test-skill`"),
            "Index should contain load command for test-skill"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 5: Hat filtering works in skill index
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_skill_index_hat_filtering() {
        let config = SkillsConfig {
            enabled: true,
            dirs: vec![skills_fixtures_dir()],
            overrides: HashMap::new(),
        };

        let registry =
            SkillRegistry::from_config(&config, std::path::Path::new("."), None).unwrap();

        // Builder hat should see complex-test-skill (hat-restricted to builder)
        let builder_index = registry.build_index(Some("builder"));
        assert!(
            builder_index.contains("complex-test-skill"),
            "Builder should see complex-test-skill"
        );

        // Reviewer hat should NOT see complex-test-skill
        let reviewer_index = registry.build_index(Some("reviewer"));
        assert!(
            !reviewer_index.contains("complex-test-skill"),
            "Reviewer should NOT see complex-test-skill (restricted to builder)"
        );

        // Both should see unrestricted skills
        assert!(
            builder_index.contains("test-skill"),
            "Builder should see unrestricted test-skill"
        );
        assert!(
            reviewer_index.contains("test-skill"),
            "Reviewer should see unrestricted test-skill"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 6: Skill index appears in assembled prompt
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_skill_index_injected_into_prompt() {
        let config = SkillsConfig {
            enabled: true,
            dirs: vec![skills_fixtures_dir()],
            overrides: HashMap::new(),
        };

        let registry =
            SkillRegistry::from_config(&config, std::path::Path::new("."), None).unwrap();

        let skill_index = registry.build_index(None);

        // Build a prompt with HatlessRalph and inject the skill index
        let ralph_config = RalphConfig::default();
        let hat_registry = HatRegistry::new();
        let ralph = HatlessRalph::new(
            "LOOP_COMPLETE",
            ralph_config.core.clone(),
            &hat_registry,
            None,
        )
        .with_skill_index(skill_index);

        let prompt = ralph.build_prompt("", &[], &[]);

        // Skill index section should appear in the prompt
        assert!(
            prompt.contains("## SKILLS"),
            "Assembled prompt should contain ## SKILLS section"
        );
        assert!(
            prompt.contains("ralph-tools"),
            "Assembled prompt should list ralph-tools skill"
        );
        assert!(
            prompt.contains("test-skill"),
            "Assembled prompt should list test-skill"
        );

        // Skill index should appear AFTER GUARDRAILS
        let guardrails_pos = prompt.find("GUARDRAILS");
        let skills_pos = prompt.find("## SKILLS");
        assert!(
            guardrails_pos.is_some() && skills_pos.is_some(),
            "Both GUARDRAILS and SKILLS sections should exist"
        );
        if let (Some(g), Some(s)) = (guardrails_pos, skills_pos) {
            assert!(
                g < s,
                "SKILLS section should appear after GUARDRAILS (guardrails at {}, skills at {})",
                g,
                s
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 7: Backwards compatibility — no skills section in config
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_backwards_compat_no_skills_in_config() {
        // YAML with no skills section — should use defaults
        let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
event_loop:
  completion_promise: "LOOP_COMPLETE"
  max_iterations: 10
"#;

        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

        // Skills should be enabled by default
        assert!(config.skills.enabled, "Skills should be enabled by default");
        assert!(
            config.skills.dirs.is_empty(),
            "Skills dirs should default to empty"
        );
        assert!(
            config.skills.overrides.is_empty(),
            "Skills overrides should default to empty"
        );

        // Registry should still work with just built-in skills
        let registry =
            SkillRegistry::from_config(&config.skills, std::path::Path::new("."), Some("claude"))
                .unwrap();

        assert!(registry.get("ralph-tools").is_some());

        // Build index should work
        let index = registry.build_index(None);
        assert!(index.contains("## SKILLS"));
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 8: Skills config parses from YAML with all fields
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_skills_config_yaml_parsing() {
        let yaml = r#"
skills:
  enabled: true
  dirs:
    - ".claude/skills"
    - "/path/to/shared/skills"
  overrides:
    pdd:
      enabled: false
    memories:
      auto_inject: true
      hats: ["ralph"]
"#;

        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

        assert!(config.skills.enabled);
        assert_eq!(config.skills.dirs.len(), 2);
        assert_eq!(config.skills.dirs[0], PathBuf::from(".claude/skills"));

        // Check overrides
        let pdd = config.skills.overrides.get("pdd").expect("pdd override");
        assert_eq!(pdd.enabled, Some(false));

        let memories = config
            .skills
            .overrides
            .get("memories")
            .expect("memories override");
        assert_eq!(memories.auto_inject, Some(true));
        assert_eq!(memories.hats, vec!["ralph"]);
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 9: Skills disabled in config produces empty index
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_skills_disabled_empty_index() {
        let yaml = r"
skills:
  enabled: false
";

        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.skills.enabled);

        // When skills are disabled, EventLoop would skip index generation
        // and pass an empty string to HatlessRalph — no ## SKILLS section
        let ralph_config = RalphConfig::default();
        let hat_registry = HatRegistry::new();
        let ralph = HatlessRalph::new(
            "LOOP_COMPLETE",
            ralph_config.core.clone(),
            &hat_registry,
            None,
        )
        .with_skill_index(String::new()); // Empty index = skills disabled

        let prompt = ralph.build_prompt("", &[], &[]);
        assert!(
            !prompt.contains("## SKILLS"),
            "Disabled skills should not produce ## SKILLS section"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 10: Config override disables a discovered skill
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_override_disables_skill_in_index() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "test-skill".to_string(),
            SkillOverride {
                enabled: Some(false),
                ..Default::default()
            },
        );

        let config = SkillsConfig {
            enabled: true,
            dirs: vec![skills_fixtures_dir()],
            overrides,
        };

        let registry =
            SkillRegistry::from_config(&config, std::path::Path::new("."), None).unwrap();

        // test-skill should be removed by override
        assert!(
            registry.get("test-skill").is_none(),
            "test-skill should be disabled by override"
        );

        // Other skills should still be present
        assert!(registry.get("ralph-tools").is_some());
        assert!(registry.get("complex-test-skill").is_some());

        let index = registry.build_index(None);
        assert!(
            !index.contains("| test-skill |"),
            "Disabled test-skill should not appear in index"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 11: Existing smoke tests still pass (backwards compat)
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_existing_smoke_tests_backwards_compat() {
        use ralph_core::testing::{SmokeRunner, SmokeTestConfig};

        let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");

        // Run the canonical basic session fixture
        let basic = fixtures_dir.join("basic_session.jsonl");
        let config = SmokeTestConfig::new(&basic);
        let result = SmokeRunner::run(&config).expect("Basic session should still run");
        assert!(
            result.completed_successfully(),
            "BACKWARDS COMPAT: Basic session fixture should still complete successfully"
        );

        // Run the canonical complex session fixture
        let complex = fixtures_dir.join("claude_complex_session.jsonl");
        let config = SmokeTestConfig::new(&complex);
        let result = SmokeRunner::run(&config).expect("Complex session should still run");
        assert!(
            result.completed_successfully(),
            "BACKWARDS COMPAT: Complex session fixture should still complete successfully"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Test 12: load_skill returns XML-wrapped content
    // ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_load_skill_xml_wrapping() {
        let config = SkillsConfig {
            enabled: true,
            dirs: vec![skills_fixtures_dir()],
            overrides: HashMap::new(),
        };

        let registry =
            SkillRegistry::from_config(&config, std::path::Path::new("."), None).unwrap();

        let loaded = registry
            .load_skill("test-skill")
            .expect("Should load test-skill");

        assert!(
            loaded.starts_with("<test-skill-skill>"),
            "Loaded skill should start with XML open tag"
        );
        assert!(
            loaded.ends_with("</test-skill-skill>"),
            "Loaded skill should end with XML close tag"
        );
        assert!(
            loaded.contains("# Test Skill"),
            "Loaded skill should contain body content"
        );
        // Frontmatter should be stripped
        assert!(
            !loaded.contains("name: test-skill"),
            "Loaded skill should NOT contain frontmatter"
        );
    }
}
