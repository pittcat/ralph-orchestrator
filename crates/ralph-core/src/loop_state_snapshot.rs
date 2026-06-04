//! Loop state snapshot and trace replay.
//!
//! Provides a read-only snapshot of loop state derived from events JSONL,
//! supporting API/TUI/CLI queries without modifying the event log.

use crate::config::{EventPolicyConfig, WorkflowGuardsConfig};
use crate::event_reader::EventReader;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Snapshot of loop state derived from events JSONL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopStateSnapshot {
    /// Loop identifier.
    pub loop_id: String,
    /// Path to the events file.
    pub events_path: PathBuf,
    /// Last event index processed.
    pub last_index: usize,
    /// Whether a terminal topic has been observed.
    pub terminal: bool,
    /// Open workflow instances (not yet terminal).
    pub open_instances: Vec<WorkflowInstanceSnapshot>,
    /// Closed/completed workflow instances.
    pub closed_instances: Vec<WorkflowInstanceSnapshot>,
    /// Policy findings from replay.
    pub findings: Vec<PolicyFindingSnapshot>,
    /// Topics seen during replay.
    pub seen_topics: Vec<String>,
    /// Last topic observed.
    pub last_topic: Option<String>,
}

/// Snapshot of a single workflow instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInstanceSnapshot {
    /// Chain name.
    pub chain_name: String,
    /// Instance key (None for global instances).
    pub instance_key: Option<String>,
    /// Current phase index.
    pub current_phase: usize,
    /// Topics seen for this instance.
    pub seen_topics: Vec<String>,
}

/// Snapshot of a policy finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyFindingSnapshot {
    /// 1-based line number in the events JSONL file where the finding occurred.
    pub line_number: usize,
    /// Event topic.
    pub topic: String,
    /// Finding message.
    pub message: String,
}

/// Replays events from a JSONL file to produce a loop state snapshot.
pub fn replay_events_to_snapshot(
    events_path: impl Into<PathBuf>,
    loop_id: impl Into<String>,
    workflow_guards: Option<&WorkflowGuardsConfig>,
    event_policy: Option<&EventPolicyConfig>,
) -> std::io::Result<LoopStateSnapshot> {
    let path = events_path.into();
    let mut reader = EventReader::new(&path);

    // Read all events from the beginning
    let result = reader.read_new_events()?;

    let mut snapshot = LoopStateSnapshot {
        loop_id: loop_id.into(),
        events_path: path,
        last_index: result.events.len(),
        terminal: false,
        open_instances: Vec::new(),
        closed_instances: Vec::new(),
        findings: Vec::new(),
        seen_topics: Vec::new(),
        last_topic: None,
    };

    let mut seen_topics_set = HashSet::new();
    let mut terminal_observed = false;

    // Track workflow instances if guards are configured
    let mut instance_progress: HashMap<(String, Option<String>), Vec<String>> = HashMap::new();

    for (index, event) in result.events.iter().enumerate() {
        seen_topics_set.insert(event.topic.clone());
        snapshot.last_topic = Some(event.topic.clone());

        // Check terminal topics
        if let Some(policy) = event_policy {
            if policy.terminal_topics.contains(&event.topic) {
                terminal_observed = true;
            }
            // Check for business events after terminal
            if terminal_observed && policy.business_topics.contains(&event.topic) {
                snapshot.findings.push(PolicyFindingSnapshot {
                    line_number: index + 1,
                    topic: event.topic.clone(),
                    message: format!(
                        "Business event '{}' after terminal topic violates monotonicity",
                        event.topic
                    ),
                });
            }
        }

        // Track workflow progress
        if let Some(guards) = workflow_guards {
            for chain in &guards.chains {
                if chain.topics.contains(&event.topic) {
                    let instance_key = extract_correlation_key_from_event(event, chain);
                    let key = (chain.name.clone(), instance_key);
                    let topics = instance_progress.entry(key.clone()).or_default();
                    topics.push(event.topic.clone());
                }
            }
        }
    }

    snapshot.seen_topics = seen_topics_set.into_iter().collect();
    snapshot.terminal = terminal_observed;

    // Build instance snapshots: classify as open or closed based on whether
    // the instance has seen all topics in its chain.
    for ((chain_name, instance_key), topics) in instance_progress {
        let is_complete = workflow_guards
            .and_then(|g| g.chains.iter().find(|c| c.name == chain_name))
            .map(|chain| chain.topics.len() == topics.len())
            .unwrap_or(false);

        let snap = WorkflowInstanceSnapshot {
            chain_name,
            instance_key,
            current_phase: topics.len().saturating_sub(1),
            seen_topics: topics,
        };

        if is_complete {
            snapshot.closed_instances.push(snap);
        } else {
            snapshot.open_instances.push(snap);
        }
    }

    // Record malformed lines as findings
    for malformed in &result.malformed {
        snapshot.findings.push(PolicyFindingSnapshot {
            line_number: malformed.line_number as usize,
            topic: "event.malformed".to_string(),
            message: format!("Malformed line: {}", malformed.error),
        });
    }

    Ok(snapshot)
}

/// Extract a correlation key from an event payload using dot notation.
fn extract_correlation_key_from_event(
    event: &crate::event_reader::Event,
    chain: &crate::config::WorkflowChain,
) -> Option<String> {
    let correlation = chain.correlation.as_ref()?;
    let payload_str = event.payload.as_deref()?;
    let value: serde_json::Value = serde_json::from_str(payload_str).ok()?;

    let mut current = &value;
    for part in correlation.from_payload.split('.') {
        match current {
            serde_json::Value::Object(obj) => {
                current = obj.get(part)?;
            }
            _ => return None,
        }
    }

    current.as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WorkflowChain, WorkflowChainMode};
    use std::io::Write;

    fn write_test_events(path: &std::path::Path, events: &[(&str, &str)]) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .unwrap();
        for (topic, payload) in events {
            let ts = chrono::Utc::now().to_rfc3339();
            let line = serde_json::json!({"topic": topic, "payload": payload, "ts": ts});
            writeln!(file, "{}", line).unwrap();
        }
    }

    #[test]
    fn test_replay_no_policy_gives_basic_snapshot() {
        let temp_dir = tempfile::tempdir().unwrap();
        let events_path = temp_dir.path().join("events.jsonl");
        write_test_events(
            &events_path,
            &[
                ("task.start", "start"),
                ("build.done", "done"),
                ("LOOP_COMPLETE", "finished"),
            ],
        );

        let snapshot = replay_events_to_snapshot(&events_path, "test-loop", None, None).unwrap();
        assert_eq!(snapshot.loop_id, "test-loop");
        assert_eq!(snapshot.seen_topics.len(), 3);
        // Without event_policy, terminal detection is disabled
        assert!(!snapshot.terminal);
        assert_eq!(snapshot.last_topic, Some("LOOP_COMPLETE".to_string()));
    }

    #[test]
    fn test_replay_with_terminal_topics() {
        let temp_dir = tempfile::tempdir().unwrap();
        let events_path = temp_dir.path().join("events.jsonl");
        write_test_events(
            &events_path,
            &[
                ("experiment.planned", r#"{"task_key": "t1"}"#),
                ("LOOP_COMPLETE", "finished"),
                ("experiment.evaluated", r#"{"task_key": "t1"}"#),
            ],
        );

        let policy = EventPolicyConfig {
            enabled: true,
            mode: crate::config::EventPolicyMode::Observe,
            on_violation: crate::config::ViolationAction::Warn,
            schemas: std::collections::HashMap::new(),
            terminal_topics: vec!["LOOP_COMPLETE".to_string()],
            business_topics: vec![
                "experiment.planned".to_string(),
                "experiment.evaluated".to_string(),
            ],
            ..Default::default()
        };

        let snapshot =
            replay_events_to_snapshot(&events_path, "test-loop", None, Some(&policy)).unwrap();
        assert!(snapshot.terminal);
        assert_eq!(snapshot.findings.len(), 1);
        assert!(snapshot.findings[0].message.contains("monotonicity"));
    }

    #[test]
    fn test_replay_malformed_events_in_findings() {
        let temp_dir = tempfile::tempdir().unwrap();
        let events_path = temp_dir.path().join("events.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&events_path)
            .unwrap();
        writeln!(
            file,
            r#"{{"topic": "ok", "payload": "x", "ts": "2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(file, "not valid json").unwrap();

        let snapshot = replay_events_to_snapshot(&events_path, "test-loop", None, None).unwrap();
        assert_eq!(snapshot.findings.len(), 1);
        assert!(snapshot.findings[0].message.contains("Malformed"));
    }

    #[test]
    fn test_replay_classifies_instances_as_open_or_closed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let events_path = temp_dir.path().join("events.jsonl");

        // Mix of complete and partial chains with correlation by id
        write_test_events(
            &events_path,
            &[
                ("task.start", r#"{"id": "1"}"#),
                ("build.done", r#"{"id": "1"}"#),
                ("LOOP_COMPLETE", r#"{"id": "1"}"#),
                ("task.start", r#"{"id": "2"}"#),
            ],
        );

        let guards = WorkflowGuardsConfig {
            chains: vec![WorkflowChain {
                name: "build".to_string(),
                topics: vec![
                    "task.start".to_string(),
                    "build.done".to_string(),
                    "LOOP_COMPLETE".to_string(),
                ],
                mode: WorkflowChainMode::Strict,
                correlation: Some(crate::config::workflow_guards::CorrelationConfig {
                    from_payload: "id".to_string(),
                    from_topic: None,
                }),
            }],
        };

        let snapshot =
            replay_events_to_snapshot(&events_path, "test-loop", Some(&guards), None).unwrap();

        assert_eq!(
            snapshot.closed_instances.len(),
            1,
            "Complete chain should be closed"
        );
        assert_eq!(
            snapshot.open_instances.len(),
            1,
            "Partial chain should be open"
        );
        assert_eq!(snapshot.closed_instances[0].chain_name, "build");
        assert_eq!(snapshot.closed_instances[0].seen_topics.len(), 3);
        assert_eq!(snapshot.open_instances[0].chain_name, "build");
        assert_eq!(snapshot.open_instances[0].seen_topics.len(), 1);
    }
}
