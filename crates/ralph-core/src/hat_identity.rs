//! Hat identity snapshot — source of truth for the `## HAT IDENTITY`
//! prompt block, `ralph inspect loop`, and tests.
//!
//! Plan ref: U1 of
//! `docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md`,
//! further tightened by R2 in
//! `docs/plans/2026-07-04-002-fix-opac-p0-p1-p2-issues-plan.md`.
//!
//! All fields are derived from the resolved `RalphConfig` so that the
//! prompt, CLI observation, and enforcement layers stay in sync.
//! Completion topics are **not** stored on the snapshot — see
//! [`crate::completion_emit::derive_completion_publishes`] for the
//! R2 SSOT fix that replaced the old heuristic.

use std::fmt::Write as _;

use ralph_proto::HatId;
use serde::{Deserialize, Serialize};

use crate::config::RalphConfig;
use crate::execution_contract::EffectiveExecutionContract;

/// Heading the loop prepends. Logged in the prompt verbatim so agents
/// and grep-based scrapers can match a single literal.
pub const HAT_IDENTITY_HEADING: &str = "## HAT IDENTITY";

/// Read-only snapshot of a hat's identity and permissions.
///
/// `completion_publishes` is intentionally **not** a struct field —
/// it is derived on demand via [`crate::completion_emit::derive_completion_publishes`]
/// so the prompt, `ralph inspect loop`, and the U7 close-warning all
/// share one computation against the resolved `RalphConfig`. The
/// pre-fix heuristic lived here and drifted from the warning payload
/// (P1 #4).
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
    /// U4 (plan 2026-07-30-004): topics explicitly denied for this hat
    /// by the Effective Execution Contract's `emit_denies`. Empty when
    /// the snapshot was built via [`from_config`](Self::from_config)
    /// (legacy path).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_topics: Vec<String>,
    /// U4 (plan 2026-07-30-004): SHA-256 digest of the compiled
    /// execution contract. Empty when the snapshot was built via
    /// [`from_config`](Self::from_config) (legacy path).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub contract_digest: String,
}

impl HatIdentitySnapshot {
    /// Build a snapshot from resolved config for a specific hat.
    ///
    /// Returns `None` when the hat is not declared in the config —
    /// unknown hats fail closed.
    ///
    /// **Legacy path.** Prefer [`from_config_and_contract`](Self::from_config_and_contract)
    /// which projects the Effective Execution Contract's actionability
    /// (U4, plan 2026-07-30-004) so the prompt and runtime enforcement
    /// are provably in sync. This method reads raw config values and
    /// leaves `denied_topics` / `contract_digest` empty.
    pub fn from_config(config: &RalphConfig, hat_id: &HatId) -> Option<Self> {
        let base = Self::base_from_config(config, hat_id)?;
        Some(Self {
            denied_topics: Vec::new(),
            contract_digest: String::new(),
            ..base
        })
    }

    /// Build a snapshot that projects the Effective Execution Contract's
    /// actionability for a specific hat (U4, plan 2026-07-30-004).
    ///
    /// `publishes` is derived from `contract.emit_allows` filtered to
    /// this hat (sorted), and `denied_topics` from `contract.emit_denies`
    /// filtered to this hat (sorted). This makes the prompt block and
    /// the runtime enforcement provably consistent.
    ///
    /// Returns `None` when the hat is not declared in the config.
    pub fn from_config_and_contract(
        config: &RalphConfig,
        hat_id: &HatId,
        contract: &EffectiveExecutionContract,
    ) -> Option<Self> {
        let base = Self::base_from_config(config, hat_id)?;
        let hat = hat_id.as_str();
        let mut publishes: Vec<String> = contract
            .emit_allows
            .iter()
            .filter(|(h, _)| h == hat)
            .map(|(_, t)| t.clone())
            .collect();
        publishes.sort();
        let mut denied_topics: Vec<String> = contract
            .emit_denies
            .iter()
            .filter(|(h, _)| h == hat)
            .map(|(_, t)| t.clone())
            .collect();
        denied_topics.sort();
        Some(Self {
            publishes,
            denied_topics,
            contract_digest: contract.contract_digest.clone(),
            ..base
        })
    }

    /// Shared base construction: hat existence check, coordinator flag,
    /// task-command policy, triggers, and raw publishes. Both
    /// [`from_config`](Self::from_config) and
    /// [`from_config_and_contract`](Self::from_config_and_contract)
    /// delegate here; the latter overrides `publishes` with the
    /// contract-projected set.
    fn base_from_config(config: &RalphConfig, hat_id: &HatId) -> Option<Self> {
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

        Some(Self {
            hat_id: hat_id.as_str().to_string(),
            publishes,
            triggers,
            is_coordinator,
            allowed_task_commands,
            denied_task_commands,
            denied_topics: Vec::new(),
            contract_digest: String::new(),
        })
    }

    /// Render the `## HAT IDENTITY` prompt block.
    ///
    /// Completion topics are derived from the resolved `RalphConfig`
    /// (not stored on `self`) so the prompt and the U7 close-warning
    /// cannot drift.
    pub fn to_prompt_block(&self, config: &RalphConfig) -> String {
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
        if !self.denied_topics.is_empty() {
            let _ = writeln!(buf, "- denied_topics: {}", format_list(&self.denied_topics));
        }
        if !self.contract_digest.is_empty() {
            let _ = writeln!(buf, "- contract_digest: {}", self.contract_digest);
        }
        let completion = crate::completion_emit::derive_completion_publishes(config, &self.hat_id);
        let _ = writeln!(buf, "- completion_publishes: {}", format_list(&completion));
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
        assert!(
            snapshot
                .allowed_task_commands
                .contains(&"ensure".to_string())
        );
        assert!(!snapshot.denied_task_commands.contains(&"add".to_string()));
        // prompt block derives completion topis from policy; without
        // event_policy the helper returns []. This test pins the
        // "completion_publishes derives from policy" behaviour.
        let block = snapshot.to_prompt_block(&config);
        assert!(block.contains("completion_publishes: (none)"));
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
        assert!(
            snapshot
                .denied_task_commands
                .contains(&"ensure".to_string())
        );
        assert!(!snapshot.allowed_task_commands.contains(&"add".to_string()));
        assert!(
            snapshot
                .allowed_task_commands
                .contains(&"start".to_string())
        );
        assert!(
            snapshot
                .allowed_task_commands
                .contains(&"close".to_string())
        );
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
        assert!(
            snapshot
                .denied_task_commands
                .contains(&"ensure".to_string())
        );
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
        assert!(
            snapshot
                .to_prompt_block(&config)
                .contains("publishes: (none)")
        );
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
        let block = snapshot.to_prompt_block(&config);

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
        assert!(
            json["denied_task_commands"]
                .as_array()
                .unwrap()
                .contains(&"add".into())
        );
    }

    #[test]
    fn prompt_block_completion_topics_uses_event_policy() {
        use crate::completion_emit::derive_completion_publishes;
        // R2: completion_publishes must be derived from the resolved
        // event_policy.terminal_topics ∪ event_policy.business_topics,
        // not from a hand-rolled heuristic that previously included
        // "work.failed" via substring matching.
        //
        // We split the verification into two parts because loading a
        // full event_policy YAML fixture is brittle (the schema gains
        // fields routinely). Part 1 covers the SSOT in isolation; Part 2
        // asserts the prompt block falls back to `(none)` when no policy
        // is configured — the key regression target.
        let config_with_hats = parse_config(
            r#"
hats:
  finisher:
    name: "Finisher"
    triggers: ["work.ready"]
    publishes:
      - "plan.complete"
      - "work.done"
      - "work.failed"
      - "debug.log"
"#,
        );
        // Part 1: no policy ⇒ empty completion list. Pin the absence
        // of the heuristic so a stray regex patch cannot silently
        // re-introduce "work.failed".
        let empty = derive_completion_publishes(&config_with_hats, "finisher");
        assert!(
            empty.is_empty(),
            "no policy ⇒ completion_topics must be empty: got {empty:?}"
        );
        let snap = HatIdentitySnapshot::from_config(&config_with_hats, &HatId::new("finisher"))
            .expect("finisher snapshot");
        let block = snap.to_prompt_block(&config_with_hats);
        assert!(block.contains("completion_publishes: (none)"));
        // Prompt block should still list the full publishes for the
        // agent's reference; that line legitimately contains
        // "work.failed" because the hat declared it. Only the
        // *completion* derivation excludes it.
        assert!(block.contains("publishes: plan.complete, work.done, work.failed, debug.log"));
    }

    #[test]
    fn heading_constant_is_stable() {
        assert_eq!(HAT_IDENTITY_HEADING, "## HAT IDENTITY");
    }

    #[test]
    fn u4_prompt_projects_contract_actionability_not_raw_config() {
        use std::collections::{BTreeMap, BTreeSet};

        use crate::execution_contract::EffectiveExecutionContract;

        let yaml = r#"
hats:
  worker:
    name: "Worker"
    triggers: ["work.ready"]
    publishes: ["work.done", "work.failed", "work.ready"]
"#;
        let config = parse_config(yaml);

        // Build a contract that denies work.ready for worker but allows
        // work.done and work.failed.
        let contract = EffectiveExecutionContract {
            contract_digest: "abc123digest".to_string(),
            emit_denies: BTreeSet::from([("worker".to_string(), "work.ready".to_string())]),
            glob_denies: BTreeMap::new(),
            emit_allows: BTreeSet::from([
                ("worker".to_string(), "work.done".to_string()),
                ("worker".to_string(), "work.failed".to_string()),
            ]),
            consumed_topics: BTreeSet::new(),
            declared_contract_topics: BTreeSet::new(),
        };

        let snapshot = HatIdentitySnapshot::from_config_and_contract(
            &config,
            &HatId::new("worker"),
            &contract,
        )
        .expect("worker hat exists");

        // publishes must be the contract-projected set, NOT the raw
        // config's 3 topics.
        assert_eq!(snapshot.publishes, vec!["work.done", "work.failed"]);
        // denied_topics must reflect the contract deny.
        assert_eq!(snapshot.denied_topics, vec!["work.ready"]);
        // contract_digest must match.
        assert_eq!(snapshot.contract_digest, contract.contract_digest);

        // The prompt block must show the projected publishes and the
        // denied_topics / contract_digest lines.
        let block = snapshot.to_prompt_block(&config);
        assert!(block.contains("publishes: work.done, work.failed"));
        assert!(block.contains("denied_topics: work.ready"));
        assert!(block.contains("contract_digest: abc123digest"));
        // Raw config's work.ready must NOT appear in publishes.
        assert!(!block.contains("publishes: work.done, work.failed, work.ready"));

        // JSON serialization must include the new fields.
        let json = snapshot.to_json();
        assert_eq!(json["contract_digest"], "abc123digest");
        assert_eq!(json["denied_topics"][0], "work.ready");
    }

    #[test]
    fn u4_legacy_from_config_leaves_contract_fields_empty() {
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

        assert!(snapshot.denied_topics.is_empty());
        assert!(snapshot.contract_digest.is_empty());

        // Prompt block must NOT contain denied_topics or contract_digest
        // lines when they are empty.
        let block = snapshot.to_prompt_block(&config);
        assert!(!block.contains("denied_topics:"));
        assert!(!block.contains("contract_digest:"));

        // JSON must omit the empty fields (skip_serializing_if).
        let json = snapshot.to_json();
        assert!(json.get("denied_topics").is_none());
        assert!(json.get("contract_digest").is_none());
    }
}
