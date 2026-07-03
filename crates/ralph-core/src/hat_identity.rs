//! Hat identity snapshot — source of truth for the `## HAT IDENTITY`
//! prompt block, `ralph inspect loop`, and tests.
//!
//! Plan ref: U1 of
//! `docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md`.
//!
//! All fields are derived from the resolved `RalphConfig` so that the
//! prompt, CLI observation, and enforcement layers stay in sync.

use std::fmt::Write as _;

use ralph_proto::HatId;
use serde::{Deserialize, Serialize};

use crate::config::RalphConfig;

/// Heading the loop prepends. Logged in the prompt verbatim so agents
/// and grep-based scrapers can match a single literal.
pub const HAT_IDENTITY_HEADING: &str = "## HAT IDENTITY";

/// Read-only snapshot of a hat's identity and permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HatIdentitySnapshot {
    /// Hat identifier.
    pub hat_id: String,
    /// Topics this hat is allowed to publish.
    pub publishes: Vec<String>,
    /// Topics that trigger this hat.
    pub triggers: Vec<String>,
    /// Whether this hat is in `tasks.coordinator_hats`.
    pub is_coordinator: bool,
    /// `ralph tools task` subcommands this hat may invoke.
    pub allowed_task_commands: Vec<String>,
    /// `ralph tools task` subcommands this hat must NOT invoke.
    pub denied_task_commands: Vec<String>,
    /// Completion-class topics from `publishes` that this hat should
    /// emit after closing work.
    pub completion_publishes: Vec<String>,
}

impl HatIdentitySnapshot {
    /// Build a snapshot from resolved config for a specific hat.
    ///
    /// Returns `None` when the hat is not declared in the config —
    /// unknown hats fail closed.
    pub fn from_config(config: &RalphConfig, hat_id: &HatId) -> Option<Self> {
        let hat_config = config.hats.get(hat_id.as_str())?;
        let is_coordinator = config
            .tasks
            .coordinator_hats
            .iter()
            .any(|h| h == hat_id.as_str());

        let (allowed_task_commands, denied_task_commands) = task_command_policy(is_coordinator);
        let publishes = hat_config.publishes.clone();
        let triggers = hat_config.all_trigger_topics();
        let triggers: Vec<String> = triggers.iter().map(|t| t.as_str().to_string()).collect();
        let completion_publishes = derive_completion_publishes(&publishes);

        Some(Self {
            hat_id: hat_id.as_str().to_string(),
            publishes,
            triggers,
            is_coordinator,
            allowed_task_commands,
            denied_task_commands,
            completion_publishes,
        })
    }

    /// Render the `## HAT IDENTITY` prompt block.
    pub fn to_prompt_block(&self) -> String {
        let mut buf = String::new();
        let _ = writeln!(buf, "{HAT_IDENTITY_HEADING}");
        let _ = writeln!(
            buf,
            "You are `{hat_id}`. This block is the single source of truth for your identity and permissions in this activation.",
            hat_id = self.hat_id
        );
        let _ = writeln!(buf);
        let _ = writeln!(buf, "- hat_id: {}", self.hat_id);
        let _ = writeln!(buf, "- is_coordinator: {}", self.is_coordinator);
        let _ = writeln!(
            buf,
            "- allowed_task_commands: {}",
            format_list(&self.allowed_task_commands)
        );
        let _ = writeln!(
            buf,
            "- denied_task_commands: {}",
            format_list(&self.denied_task_commands)
        );
        let _ = writeln!(buf, "- triggers: {}", format_list(&self.triggers));
        let _ = writeln!(buf, "- publishes: {}", format_list(&self.publishes));
        let _ = writeln!(
            buf,
            "- completion_publishes: {}",
            format_list(&self.completion_publishes)
        );
        if !self.denied_task_commands.is_empty() {
            let _ = writeln!(buf);
            let _ = writeln!(
                buf,
                "Do NOT invoke any command listed under `denied_task_commands`."
            );
        }
        let _ = writeln!(buf);
        buf
    }

    /// Serialize to a JSON value for machine-readable inspection.
    pub fn to_json(&self) -> serde_json::Value {
        // Unwrap is safe: the struct is composed of serializable fields.
        serde_json::to_value(self).expect("HatIdentitySnapshot serializes to JSON")
    }
}

/// Derive the task-command allow/deny lists from the coordinator flag.
///
/// * Coordinators may add/ensure tasks on behalf of the loop.
/// * Non-coordinators may only mutate tasks they own; `add` and `ensure`
///   are denied because they create cross-hat work items.
fn task_command_policy(is_coordinator: bool) -> (Vec<String>, Vec<String>) {
    let lifecycle = vec![
        "start".to_string(),
        "close".to_string(),
        "list".to_string(),
        "show".to_string(),
        "ready".to_string(),
        "fail".to_string(),
        "reopen".to_string(),
    ];
    if is_coordinator {
        let mut allowed = vec!["add".to_string(), "ensure".to_string()];
        allowed.extend(lifecycle);
        (allowed, Vec::new())
    } else {
        let denied = vec!["add".to_string(), "ensure".to_string()];
        (lifecycle, denied)
    }
}

/// Heuristic completion-class topic detector.
///
/// Filters a hat's declared publishes for topics that commonly signal
/// activation completion. This is intentionally simple and config-free
/// for the first iteration; future work can tighten it from
/// `WorkflowContract` / `execution_contract` terminal topics.
fn derive_completion_publishes(publishes: &[String]) -> Vec<String> {
    publishes
        .iter()
        .filter(|t| is_completion_topic(t))
        .cloned()
        .collect()
}

fn is_completion_topic(topic: &str) -> bool {
    let t = topic.to_lowercase();
    t.ends_with(".complete")
        || t.contains("done")
        || t.contains("finished")
        || t == "LOOP_COMPLETE".to_lowercase()
}

fn format_list(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_config(yaml: &str) -> RalphConfig {
        serde_yaml::from_str(yaml).expect("test fixture YAML parses")
    }

    #[test]
    fn coordinator_hat_allows_add_and_ensure() {
        let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.*"]
    publishes: ["plan.complete", "work.ready"]
  worker:
    name: "Worker"
    triggers: ["work.ready"]
    publishes: ["work.done"]
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
"#;
        let config = parse_config(yaml);
        let snapshot = HatIdentitySnapshot::from_config(&config, &HatId::new("coordinator"))
            .expect("coordinator hat exists");

        assert!(snapshot.is_coordinator);
        assert!(snapshot.allowed_task_commands.contains(&"add".to_string()));
        assert!(snapshot.allowed_task_commands.contains(&"ensure".to_string()));
        assert!(!snapshot.denied_task_commands.contains(&"add".to_string()));
        assert!(snapshot.completion_publishes.contains(&"plan.complete".to_string()));
    }

    #[test]
    fn non_coordinator_denies_add_and_ensure() {
        let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.*"]
    publishes: ["plan.complete"]
  worker:
    name: "Worker"
    triggers: ["work.ready"]
    publishes: ["work.done"]
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
"#;
        let config = parse_config(yaml);
        let snapshot = HatIdentitySnapshot::from_config(&config, &HatId::new("worker"))
            .expect("worker hat exists");

        assert!(!snapshot.is_coordinator);
        assert!(snapshot.denied_task_commands.contains(&"add".to_string()));
        assert!(snapshot.denied_task_commands.contains(&"ensure".to_string()));
        assert!(!snapshot.allowed_task_commands.contains(&"add".to_string()));
        assert!(snapshot.allowed_task_commands.contains(&"start".to_string()));
        assert!(snapshot.allowed_task_commands.contains(&"close".to_string()));
        assert!(snapshot.allowed_task_commands.contains(&"list".to_string()));
    }

    #[test]
    fn unknown_hat_id_returns_none() {
        let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.*"]
tasks:
  coordinator_hats:
    - coordinator
"#;
        let config = parse_config(yaml);
        assert!(HatIdentitySnapshot::from_config(&config, &HatId::new("ghost")).is_none());
    }

    #[test]
    fn empty_coordinator_hats_fail_closed_for_agent_owner() {
        let yaml = r#"
hats:
  owner:
    name: "Owner"
    triggers: ["task.*"]
    publishes: ["work.done"]
tasks:
  enabled: true
  coordinator_hats: []
"#;
        let config = parse_config(yaml);
        let snapshot = HatIdentitySnapshot::from_config(&config, &HatId::new("owner"))
            .expect("owner hat exists");

        assert!(!snapshot.is_coordinator);
        assert!(snapshot.denied_task_commands.contains(&"add".to_string()));
        assert!(snapshot.denied_task_commands.contains(&"ensure".to_string()));
    }

    #[test]
    fn hat_without_publishes_yields_empty_arrays() {
        let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["work.done"]
tasks:
  coordinator_hats:
    - reviewer
"#;
        let config = parse_config(yaml);
        let snapshot = HatIdentitySnapshot::from_config(&config, &HatId::new("reviewer"))
            .expect("reviewer hat exists");

        assert!(snapshot.publishes.is_empty());
        assert!(snapshot.completion_publishes.is_empty());
        assert!(snapshot.to_prompt_block().contains("publishes: (none)"));
    }

    #[test]
    fn prompt_block_contains_heading_and_fields() {
        let yaml = r#"
hats:
  worker:
    name: "Worker"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
        let config = parse_config(yaml);
        let snapshot = HatIdentitySnapshot::from_config(&config, &HatId::new("worker"))
            .expect("worker hat exists");
        let block = snapshot.to_prompt_block();

        assert!(block.starts_with(HAT_IDENTITY_HEADING));
        assert!(block.contains("hat_id: worker"));
        assert!(block.contains("is_coordinator: false"));
        assert!(block.contains("denied_task_commands: add, ensure"));
        assert!(block.contains("Do NOT invoke any command listed under `denied_task_commands`."));
    }

    #[test]
    fn to_json_round_trips() {
        let yaml = r#"
hats:
  worker:
    name: "Worker"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
        let config = parse_config(yaml);
        let snapshot = HatIdentitySnapshot::from_config(&config, &HatId::new("worker"))
            .expect("worker hat exists");
        let json = snapshot.to_json();

        assert_eq!(json["hat_id"], "worker");
        assert_eq!(json["is_coordinator"], false);
        assert!(json["denied_task_commands"].as_array().unwrap().contains(&"add".into()));
    }

    #[test]
    fn completion_publishes_filters_by_common_patterns() {
        let yaml = r#"
hats:
  finisher:
    name: "Finisher"
    triggers: ["work.ready"]
    publishes:
      - "plan.complete"
      - "work.done"
      - "task.finished"
      - "work.failed"
      - "debug.log"
"#;
        let config = parse_config(yaml);
        let snapshot = HatIdentitySnapshot::from_config(&config, &HatId::new("finisher"))
            .expect("finisher hat exists");

        assert!(snapshot.completion_publishes.contains(&"plan.complete".to_string()));
        assert!(snapshot.completion_publishes.contains(&"work.done".to_string()));
        assert!(snapshot.completion_publishes.contains(&"task.finished".to_string()));
        assert!(!snapshot.completion_publishes.contains(&"work.failed".to_string()));
        assert!(!snapshot.completion_publishes.contains(&"debug.log".to_string()));
    }

    #[test]
    fn heading_constant_is_stable() {
        assert_eq!(HAT_IDENTITY_HEADING, "## HAT IDENTITY");
    }
}
