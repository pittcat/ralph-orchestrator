//! Tests for structured_evidence.

use super::common::*;
use super::*;
use crate::config::CoreConfig;

#[test]
fn test_structured_build_done_json_pass_accepted() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = r#"{"checks":{"tests":"pass","lint":"pass","typecheck":"pass"}}"#;
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"build.done".to_string()),
        "structured pass should propagate build.done. Got: {topics:?}"
    );
    assert!(
        !topics.contains(&"build.blocked".to_string()),
        "structured pass must not emit build.blocked. Got: {topics:?}"
    );
}

#[test]
fn test_structured_build_done_json_missing_lint_blocks() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = r#"{"checks":{"tests":"pass","typecheck":"pass"}}"#;
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"build.blocked".to_string()),
        "missing lint should emit build.blocked. Got: {topics:?}"
    );
    assert!(
        !topics.contains(&"build.done".to_string()),
        "missing lint must not propagate build.done. Got: {topics:?}"
    );
}

#[test]
fn test_structured_build_done_json_missing_evidence_file_blocks() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = r#"{"checks":{"tests":"pass","lint":"pass","typecheck":"pass"},"evidence_files":["missing/never-created.log"]}"#;
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"build.blocked".to_string()),
        "missing evidence file should emit build.blocked. Got: {topics:?}"
    );
}

#[test]
fn test_legacy_text_build_done_still_passes() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Legacy text evidence uses the same evidence format that the JSON
    // path also requires: every required check + duplication. The
    // legacy parser returns duplication_passed=false when the field is
    // missing, so we must include it explicitly.
    write_event_to_jsonl(
        &events_path,
        "build.done",
        "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 5\nduplication: pass",
    );
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"build.done".to_string()),
        "legacy text evidence should still pass. Got: {topics:?}"
    );
}

#[test]
fn test_structured_review_done_json_pass_accepted() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = r#"{"checks":{"tests":"pass","build":"pass"}}"#;
    write_event_to_jsonl(&events_path, "review.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"review.done".to_string()),
        "structured review pass should propagate. Got: {topics:?}"
    );
    assert!(
        !topics.contains(&"review.blocked".to_string()),
        "structured review pass must not block. Got: {topics:?}"
    );
}

#[test]
fn test_structured_review_done_json_missing_build_blocks() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = r#"{"checks":{"tests":"pass"}}"#;
    write_event_to_jsonl(&events_path, "review.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"review.blocked".to_string()),
        "missing build check should emit review.blocked. Got: {topics:?}"
    );
}

#[test]
fn test_structured_wave_review_done_exempt_from_blocking() {
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Wave metadata must be at the event top-level (not in payload) so
    // the EventRecord picks it up and the event loop treats it as a
    // wave event.
    let wave_event = serde_json::json!({
        "topic": "review.done",
        "payload": r#"{"checks":{"tests":"pass","build":"pass"}}"#,
        "ts": chrono::Utc::now().to_rfc3339(),
        "wave_id": "w-1",
        "wave_index": 0,
        "wave_total": 2,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap();
    writeln!(file, "{wave_event}").unwrap();

    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    // Wave result events are exempt; the loop checks `!event.is_wave_event()`
    // before applying the structured JSON path, so we should NOT see
    // review.blocked even though the payload itself is structured.
    assert!(
        !topics.contains(&"review.blocked".to_string()),
        "wave review.done must not be blocked. Got: {topics:?}"
    );
}
