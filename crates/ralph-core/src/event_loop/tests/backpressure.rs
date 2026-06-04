//! Tests for backpressure.

use super::common::*;
use super::*;

#[test]
fn test_build_done_backpressure_accepts_mutants_warning() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: pass\nmutants: warn (65%)";
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"build.done".to_string()),
        "build.done with mutants warning should pass through. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"build.blocked".to_string()),
        "build.done should not be blocked by mutation warnings"
    );
}

#[test]
fn test_build_done_backpressure_rejects_high_complexity() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 12\nduplication: pass";
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"build.blocked".to_string()),
        "build.done with high complexity should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"build.done".to_string()),
        "build.done should not pass through when complexity is too high"
    );
}

#[test]
fn test_build_done_backpressure_rejects_duplication() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: fail";
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"build.blocked".to_string()),
        "build.done with duplication should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"build.done".to_string()),
        "build.done should not pass through when duplication fails"
    );
}

#[test]
fn test_build_done_backpressure_rejects_performance_regression() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: regression";
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"build.blocked".to_string()),
        "build.done with performance regression should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"build.done".to_string()),
        "build.done should not pass through when performance regresses"
    );
}

#[test]
fn test_review_done_backpressure_accepts_verified() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a review.done event WITH verification evidence
    write_event_to_jsonl(&events_path, "review.done", "tests: pass\nbuild: pass");
    let _ = event_loop.process_events_from_jsonl();

    // Should pass through as review.done (not blocked)
    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"review.done".to_string()),
        "Verified review.done should pass through. Got: {:?}",
        pending_topics
    );
}

#[test]
fn test_review_done_backpressure_rejects_unverified() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a review.done event WITHOUT verification evidence
    write_event_to_jsonl(&events_path, "review.done", "Looks good, approved!");
    let _ = event_loop.process_events_from_jsonl();

    // Should be transformed into review.blocked
    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"review.blocked".to_string()),
        "Unverified review.done should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"review.done".to_string()),
        "review.done should not pass through without evidence"
    );
}

#[test]
fn test_review_done_backpressure_rejects_failed_checks() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a review.done event with failed checks
    write_event_to_jsonl(&events_path, "review.done", "tests: fail\nbuild: pass");
    let _ = event_loop.process_events_from_jsonl();

    // Should be transformed into review.blocked
    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"review.blocked".to_string()),
        "review.done with failed tests should be blocked. Got: {:?}",
        pending_topics
    );
}

#[test]
fn test_verify_passed_backpressure_accepts_quality_report() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "quality.tests: pass\nquality.coverage: 82%\nquality.lint: pass\nquality.audit: pass\nquality.mutation: 72%\nquality.complexity: 7";
    write_event_to_jsonl(&events_path, "verify.passed", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"verify.passed".to_string()),
        "verify.passed with quality report should pass through. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"verify.failed".to_string()),
        "verify.passed should not be blocked by quality report"
    );
}

#[test]
fn test_verify_passed_backpressure_rejects_missing_quality_report() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "verify.passed", "All good");
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"verify.failed".to_string()),
        "verify.passed without quality report should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"verify.passed".to_string()),
        "verify.passed should not pass through without quality report"
    );
}

#[test]
fn test_verify_passed_backpressure_rejects_failed_thresholds() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "quality.tests: pass\nquality.coverage: 60%\nquality.lint: pass\nquality.audit: pass\nquality.mutation: 50%\nquality.complexity: 12";
    write_event_to_jsonl(&events_path, "verify.passed", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"verify.failed".to_string()),
        "verify.passed with failing thresholds should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"verify.passed".to_string()),
        "verify.passed should not pass through with failing thresholds"
    );
}
