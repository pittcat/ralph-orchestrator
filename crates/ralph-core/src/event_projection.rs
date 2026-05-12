//! Event projection to JSONL files.
//!
//! When configured, matching events are automatically projected to target
//! JSONL files with extracted fields. This enables downstream consumers
//! to tail specific event streams without parsing the full events file.

use crate::config::ProjectionRule;
use ralph_proto::Event;
use std::io::Write;
use std::path::Path;

/// Apply projection rules to an event, appending matching projections to target files.
///
/// For each rule whose `trigger_events` contains the event's topic, extracts
/// the requested fields and appends a JSON object as a single JSONL line.
pub fn apply_projection(event: &Event, rules: &[ProjectionRule], workspace_root: &Path) {
    for rule in rules {
        if rule.trigger_events.iter().any(|t| t == event.topic.as_str()) {
            let projected = extract_fields(event, &rule.fields);
            let target_path = workspace_root.join(&rule.target_file);
            if let Err(e) = append_jsonl(&target_path, &projected) {
                eprintln!("[event_projection] warning: {}", e);
            }
        }
    }
}

fn extract_fields(event: &Event, fields: &[String]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for field in fields {
        let value = match field.as_str() {
            "topic" => serde_json::Value::String(event.topic.to_string()),
            "payload" => serde_json::Value::String(event.payload.clone()),
            "timestamp" => serde_json::Value::Null,
            "wave_id" => event
                .wave_id
                .clone()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
            _ => {
                // Try to extract from payload as JSON
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                    json.get(field).cloned().unwrap_or(serde_json::Value::Null)
                } else {
                    serde_json::Value::Null
                }
            }
        };
        map.insert(field.clone(), value);
    }
    serde_json::Value::Object(map)
}

fn append_jsonl(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", serde_json::to_string(value)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_event(topic: &str, payload: &str) -> Event {
        Event::new(topic, payload)
    }

    #[test]
    fn happy_path_matching_event_gets_projected() {
        let tmp = tempfile::tempdir().unwrap();
        let event = make_event("build.done", r#"{"status":"ok"}"#);
        let rules = vec![ProjectionRule {
            name: "build-log".to_string(),
            trigger_events: vec!["build.done".to_string()],
            fields: vec!["topic".to_string(), "payload".to_string()],
            target_file: "projections/build.jsonl".to_string(),
            mode: crate::config::ProjectionMode::Append,
        }];

        apply_projection(&event, &rules, tmp.path());

        let path = tmp.path().join("projections/build.jsonl");
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        let line: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(line["topic"], "build.done");
        assert_eq!(line["payload"], r#"{"status":"ok"}"#);
    }

    #[test]
    fn append_mode_does_not_overwrite_existing_content() {
        let tmp = tempfile::tempdir().unwrap();
        let rules = vec![ProjectionRule {
            name: "all-events".to_string(),
            trigger_events: vec!["task.start".to_string()],
            fields: vec!["topic".to_string()],
            target_file: "events.jsonl".to_string(),
            mode: crate::config::ProjectionMode::Append,
        }];

        apply_projection(&make_event("task.start", ""), &rules, tmp.path());
        apply_projection(&make_event("task.start", ""), &rules, tmp.path());

        let path = tmp.path().join("events.jsonl");
        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn missing_field_returns_null() {
        let tmp = tempfile::tempdir().unwrap();
        let event = make_event("build.done", r#"{}"#);
        let rules = vec![ProjectionRule {
            name: "extract".to_string(),
            trigger_events: vec!["build.done".to_string()],
            fields: vec!["nonexistent".to_string(), "timestamp".to_string()],
            target_file: "out.jsonl".to_string(),
            mode: crate::config::ProjectionMode::Append,
        }];

        apply_projection(&event, &rules, tmp.path());

        let path = tmp.path().join("out.jsonl");
        let content = fs::read_to_string(&path).unwrap();
        let line: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(line["nonexistent"], serde_json::Value::Null);
        assert_eq!(line["timestamp"], serde_json::Value::Null);
    }

    #[test]
    fn auto_creates_parent_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let event = make_event("x", "y");
        let rules = vec![ProjectionRule {
            name: "deep".to_string(),
            trigger_events: vec!["x".to_string()],
            fields: vec!["topic".to_string()],
            target_file: "a/b/c/d.jsonl".to_string(),
            mode: crate::config::ProjectionMode::Append,
        }];

        apply_projection(&event, &rules, tmp.path());

        let path = tmp.path().join("a/b/c/d.jsonl");
        assert!(path.exists());
    }

    #[test]
    fn write_failure_prints_warning_without_panic() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a directory at the target path so that opening it as a file fails.
        let target = tmp.path().join("is_a_dir.jsonl");
        fs::create_dir(&target).unwrap();

        let event = make_event("x", "y");
        let rules = vec![ProjectionRule {
            name: "fail".to_string(),
            trigger_events: vec!["x".to_string()],
            fields: vec!["topic".to_string()],
            target_file: "is_a_dir.jsonl".to_string(),
            mode: crate::config::ProjectionMode::Append,
        }];

        // Should not panic — just print a warning to stderr.
        apply_projection(&event, &rules, tmp.path());
    }

    #[test]
    fn extracts_nested_field_from_json_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let event = make_event("build.done", r#"{"status":"ok","count":42}"#);
        let rules = vec![ProjectionRule {
            name: "nested".to_string(),
            trigger_events: vec!["build.done".to_string()],
            fields: vec!["status".to_string(), "count".to_string(), "missing".to_string()],
            target_file: "nested.jsonl".to_string(),
            mode: crate::config::ProjectionMode::Append,
        }];

        apply_projection(&event, &rules, tmp.path());

        let path = tmp.path().join("nested.jsonl");
        let content = fs::read_to_string(&path).unwrap();
        let line: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(line["status"], "ok");
        assert_eq!(line["count"], 42);
        assert_eq!(line["missing"], serde_json::Value::Null);
    }

    #[test]
    fn non_matching_event_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let event = make_event("other.event", "");
        let rules = vec![ProjectionRule {
            name: "only-build".to_string(),
            trigger_events: vec!["build.done".to_string()],
            fields: vec!["topic".to_string()],
            target_file: "out.jsonl".to_string(),
            mode: crate::config::ProjectionMode::Append,
        }];

        apply_projection(&event, &rules, tmp.path());

        let path = tmp.path().join("out.jsonl");
        assert!(!path.exists());
    }
}
