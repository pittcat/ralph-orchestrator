//! U2 (plan 2026-07-30-004): the fallible startup boundary that compiles the
//! final resolved [`RalphConfig`] into a frozen, fingerprinted
//! [`ResolvedRuntimeConfig`].
//!
//! Every **production** `EventLoop` construction must go through
//! [`compile`] first and only proceed on `Ok`. A config with a contract gap
//! therefore fails *before* loop initialization, with a readable
//! [`ContractFindings`] error, instead of starting a loop on an inconsistent
//! declaration.
//!
//! This unit is a pure compile-time + startup boundary: it performs **no**
//! runtime action (no activation persistence, no Prompt/CLI enforcement).
//! It only:
//! - folds profile / CLI / schema / desugar inputs into a stable,
//!   deterministic `contract_digest`,
//! - applies deny-wins capability resolution,
//! - runs consumer-completeness checks (a declared execution-contract topic
//!   with no runtime consumer is a finding).

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::config::RalphConfig;

/// The effective emit capability for a `(hat, topic)` pair after deny-wins
/// resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitDecision {
    /// The hat may emit the topic.
    Allow,
    /// The hat may not emit the topic (explicit deny, or fail-closed unknown).
    Deny,
}

/// A single compile-time contract finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractCompileFinding {
    /// Classification of the gap.
    pub kind: ContractCompileFindingKind,
    /// Human-readable, actionable description.
    pub message: String,
}

/// Classification of a compile-time contract gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractCompileFindingKind {
    /// A topic carries a declared execution-contract rule but no runtime hat
    /// consumes it (and it is not a terminal / completion topic). The contract
    /// declaration would resolve to nothing at runtime (R20: a contract
    /// declaration must have a production consumer).
    MissingConsumer { topic: String },
}

/// The error type returned by [`compile`]: one or more compile-time findings.
///
/// Display renders every finding on its own line so a CLI startup failure is
/// directly readable by the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractFindings(pub Vec<ContractCompileFinding>);

impl ContractFindings {
    /// Borrow the underlying findings.
    #[must_use]
    pub fn findings(&self) -> &[ContractCompileFinding] {
        &self.0
    }
}

impl std::fmt::Display for ContractFindings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "execution contract compilation failed with {} finding(s):",
            self.0.len()
        )?;
        for (i, finding) in self.0.iter().enumerate() {
            writeln!(f, "  {}. {}", i + 1, finding.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ContractFindings {}

/// Contract-derived capability for one task and caller activation.
///
/// Lifecycle administration, execution ownership, and immediate actionability
/// are deliberately separate: coordinator rights may administer another hat's
/// task but never grant permission to execute it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCapability {
    /// Caller may perform lifecycle administration (`close`, `fail`, `reopen`).
    pub lifecycle_administration: bool,
    /// Caller is the task's execution owner in the same loop.
    pub execution_ownership: bool,
    /// Caller owns the task and it is open with no unresolved blockers.
    pub actionable_now: bool,
    /// Stable explanation for the first denied execution constraint.
    pub deny_reason: Option<&'static str>,
}

/// Evaluate task capabilities from the same task/loop/hat inputs used by the
/// prompt and agent CLI.
#[must_use]
pub fn evaluate_task_capability(
    task: &crate::task::Task,
    caller_hat: Option<&str>,
    current_loop_id: Option<&str>,
    coordinator_hats: &[String],
) -> TaskCapability {
    let loop_matches = match (current_loop_id, task.loop_id.as_deref()) {
        (Some(current), Some(target)) => current == target,
        (None, None) => true,
        _ => false,
    };
    let caller_hat = caller_hat.unwrap_or("");
    let owns_task = !caller_hat.is_empty() && task.owner_hat_id.as_deref() == Some(caller_hat);
    let is_coordinator = !caller_hat.is_empty()
        && coordinator_hats.iter().any(|hat| hat == caller_hat);
    let lifecycle_administration = loop_matches && (owns_task || is_coordinator);
    let execution_ownership = loop_matches && owns_task;
    let actionable_now = execution_ownership
        && task.status == crate::task::TaskStatus::Open
        && task.blocked_by.is_empty();
    let deny_reason = if !loop_matches {
        Some("task_wrong_loop")
    } else if !owns_task {
        Some("not_execution_owner")
    } else if task.status != crate::task::TaskStatus::Open {
        Some("task_not_open")
    } else if !task.blocked_by.is_empty() {
        Some("task_blocked")
    } else {
        None
    };

    TaskCapability {
        lifecycle_administration,
        execution_ownership,
        actionable_now,
        deny_reason,
    }
}

/// The compiled, frozen Effective Execution Contract: the deterministic
/// identity plus the resolved capability/consumer view derived from the final
/// config. Pure data — no runtime behaviour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveExecutionContract {
    /// Stable SHA-256 hex digest over the canonicalized contract inputs
    /// (profile / CLI / schema / desugar / hats / deny rules / contracts).
    /// Same input always yields the same digest.
    pub contract_digest: String,
    /// `(hat_id, topic)` pairs explicitly denied via `topic_deny_rules`.
    pub emit_denies: BTreeSet<(String, String)>,
    /// Glob-bearing deny patterns keyed by hat. Resolved lazily by
    /// [`EffectiveExecutionContract::emit_decision`] to share the same
    /// matcher the runtime uses for `check_topic_deny_rules`.
    pub glob_denies: BTreeMap<String, Vec<String>>,
    /// `(hat_id, topic)` pairs the hat may emit (publish/terminal/default),
    /// after deny-wins removal of `emit_denies`.
    pub emit_allows: BTreeSet<(String, String)>,
    /// Topics that have at least one runtime consumer (a hat triggering on
    /// them).
    pub consumed_topics: BTreeSet<String>,
    /// Topics that carry a declared execution-contract rule (only populated
    /// when `execution_contracts.enabled`).
    pub declared_contract_topics: BTreeSet<String>,
}

impl EffectiveExecutionContract {
    /// Resolve the effective emit capability for `(hat, topic)` with
    /// deny-wins + fail-closed semantics:
    /// 1. an explicit deny rule → [`EmitDecision::Deny`] (deny wins over any
    ///    publish-side allow),
    /// 2. otherwise a declared publish/terminal/default → [`EmitDecision::Allow`],
    /// 3. otherwise → [`EmitDecision::Deny`] (fail-closed on unknown
    ///    capability, R4).
    #[must_use]
    pub fn emit_decision(&self, hat: &str, topic: &str) -> EmitDecision {
        let key = (hat.to_string(), topic.to_string());
        if self.emit_denies.contains(&key) {
            return EmitDecision::Deny;
        }
        if let Some(patterns) = self.glob_denies.get(hat) {
            if patterns
                .iter()
                .any(|pattern| crate::event_policy::matches_topic_rule(pattern, topic))
            {
                return EmitDecision::Deny;
            }
        }
        if self.emit_allows.contains(&key) {
            return EmitDecision::Allow;
        }
        EmitDecision::Deny
    }
}

/// The frozen runtime config handed to a production `EventLoop` constructor:
/// the final [`RalphConfig`] plus its compiled [`EffectiveExecutionContract`].
///
/// After [`compile`] returns, the resolved config must not mutate for the
/// lifetime of the loop (R25).
#[derive(Debug, Clone)]
pub struct ResolvedRuntimeConfig {
    config: RalphConfig,
    contract: EffectiveExecutionContract,
}

impl ResolvedRuntimeConfig {
    /// Borrow the frozen config.
    #[must_use]
    pub fn config(&self) -> &RalphConfig {
        &self.config
    }

    /// Borrow the compiled contract.
    #[must_use]
    pub fn contract(&self) -> &EffectiveExecutionContract {
        &self.contract
    }

    /// The deterministic contract digest.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.contract.contract_digest
    }

    /// Consume the wrapper and return the frozen config (used by the
    /// `EventLoop` production constructors).
    #[must_use]
    pub fn into_inner(self) -> RalphConfig {
        self.config
    }
}

/// Compile the final resolved config into a frozen [`ResolvedRuntimeConfig`].
///
/// Deterministic: the same input always yields the same `contract_digest`.
/// Fails with [`ContractFindings`] when a contract gap is detected (currently:
/// a declared execution-contract topic with no runtime consumer).
pub fn compile(final_config: RalphConfig) -> Result<ResolvedRuntimeConfig, ContractFindings> {
    // ── deny-wins capability resolution ──────────────────────────────────
    // Explicit `topic_deny_rules` are collected first; a deny always wins
    // over any publish-side allow for the same `(hat, topic)`.
    let mut emit_denies: BTreeSet<(String, String)> = BTreeSet::new();
    let mut glob_denies: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(policy) = &final_config.event_loop.event_policy {
        for rule in &policy.topic_deny_rules {
            if rule.topic.contains('*') {
                glob_denies
                    .entry(rule.hat_id.clone())
                    .or_default()
                    .push(rule.topic.clone());
            } else {
                emit_denies.insert((rule.hat_id.clone(), rule.topic.clone()));
            }
        }
    }

    let mut emit_allows: BTreeSet<(String, String)> = BTreeSet::new();
    let mut consumed_topics: BTreeSet<String> = BTreeSet::new();
    let mut terminal_topics: BTreeSet<String> = BTreeSet::new();
    for (hat_id, hat) in &final_config.hats {
        for trigger in &hat.triggers {
            consumed_topics.insert(trigger.clone());
        }
        for topic in &hat.terminal_events {
            terminal_topics.insert(topic.clone());
        }
        // Emit-side topics: publishes + terminal_events + default_publishes.
        let mut emit_side: Vec<&String> = Vec::new();
        emit_side.extend(hat.publishes.iter());
        emit_side.extend(hat.terminal_events.iter());
        if let Some(default_publishes) = &hat.default_publishes {
            emit_side.push(default_publishes);
        }
        for topic in emit_side {
            let key = (hat_id.clone(), topic.clone());
            // deny-wins: a denied (hat, topic) never lands in the allow set.
            if !emit_denies.contains(&key) {
                emit_allows.insert(key);
            }
        }
    }
    if let Some(policy) = &final_config.event_loop.event_policy {
        for topic in &policy.terminal_topics {
            terminal_topics.insert(topic.clone());
        }
    }

    // ── consumer-completeness (R20) ──────────────────────────────────────
    // Only enforced when execution contracts are enabled. A declared contract
    // topic must have a production consumer (a hat triggering on it) unless it
    // is a terminal / completion / starting topic.
    let mut declared_contract_topics: BTreeSet<String> = BTreeSet::new();
    let mut findings: Vec<ContractCompileFinding> = Vec::new();
    if let Some(contracts) = &final_config.event_loop.execution_contracts
        && contracts.enabled
    {
        let mut topics: Vec<&String> = contracts.rules.keys().collect();
        topics.sort();
        for topic in topics {
            declared_contract_topics.insert((*topic).clone());
            let is_consumed = consumed_topics.contains(topic.as_str());
            let is_terminal = terminal_topics.contains(topic.as_str())
                || topic.as_str() == final_config.event_loop.completion_promise
                || final_config.event_loop.starting_event.as_deref() == Some(topic.as_str());
            if !is_consumed && !is_terminal {
                findings.push(ContractCompileFinding {
                    kind: ContractCompileFindingKind::MissingConsumer {
                        topic: (*topic).clone(),
                    },
                    message: format!(
                        "execution contract declares topic '{topic}' but no hat triggers on it \
                         and it is not a terminal/completion topic, so the contract has no \
                         production consumer. Add a consumer hat trigger for '{topic}' or remove \
                         the contract rule."
                    ),
                });
            }
        }
    }

    if !findings.is_empty() {
        return Err(ContractFindings(findings));
    }

    let contract = EffectiveExecutionContract {
        contract_digest: sha256_hex(&canonical_contract_bytes(&final_config)),
        emit_denies,
        glob_denies,
        emit_allows,
        consumed_topics,
        declared_contract_topics,
    };
    Ok(ResolvedRuntimeConfig {
        config: final_config,
        contract,
    })
}

/// Canonicalize the contract-relevant inputs of `config` into a stable byte
/// buffer suitable for hashing. All maps/sets are iterated in sorted order so
/// the output is independent of insertion order (deterministic digest). The
/// buffer folds every input the fingerprint must be sensitive to: profile
/// overlay, CLI overlay, event schemas, the precheck desugar input, the hat
/// topology, deny rules, execution contracts, and terminal identity.
fn canonical_contract_bytes(config: &RalphConfig) -> Vec<u8> {
    let mut out = String::new();

    // Profile overlay.
    out.push_str("[profile]\n");
    let mut specs: Vec<String> = config.profiles.default.iter().map(|s| s.to_string()).collect();
    specs.sort();
    for spec in specs {
        out.push_str(&spec);
        out.push('\n');
    }

    // CLI overlay.
    out.push_str("[cli]\n");
    out.push_str(&format!("backend={}\n", config.cli.backend));
    out.push_str(&format!(
        "command={}\n",
        config.cli.command.as_deref().unwrap_or("")
    ));
    out.push_str(&format!("prompt_mode={}\n", config.cli.prompt_mode));
    out.push_str(&format!("default_mode={}\n", config.cli.default_mode));
    out.push_str(&format!("idle_timeout_secs={}\n", config.cli.idle_timeout_secs));
    out.push_str(&format!(
        "autonomous_idle_timeout_secs={}\n",
        config
            .cli
            .autonomous_idle_timeout_secs
            .map(|v| v.to_string())
            .unwrap_or_default()
    ));
    out.push_str(&format!(
        "prompt_flag={}\n",
        config.cli.prompt_flag.as_deref().unwrap_or("")
    ));
    out.push_str("args=");
    for arg in &config.cli.args {
        out.push_str(arg);
        out.push(',');
    }
    out.push('\n');

    // Event schemas.
    out.push_str("[schema]\n");
    if let Some(policy) = &config.event_loop.event_policy {
        let mut topics: Vec<&String> = policy.schemas.keys().collect();
        topics.sort();
        for topic in topics {
            let schema = &policy.schemas[topic];
            out.push_str(&format!("topic={topic}\n"));
            let mut required = schema.required_fields.clone();
            required.sort();
            out.push_str(&format!("  required={}\n", required.join(",")));
            let mut allowed_keys: Vec<&str> =
                schema.allowed_values.keys().map(String::as_str).collect();
            allowed_keys.sort();
            out.push_str(&format!("  allowed_values_keys={}\n", allowed_keys.join(",")));
        }
    }

    // Precheck desugar input (`rules` is a BTreeMap — already sorted).
    out.push_str("[desugar]\n");
    if let Some(precheck) = &config.event_loop.precheck {
        out.push_str(&format!("enabled={}\n", precheck.enabled));
        for (topic, rule) in &precheck.rules {
            out.push_str(&format!(
                "rule={topic}|prompt={}|retry={}\n",
                rule.prompt.join(";"),
                rule.on_fail.retry_budget
            ));
        }
    }

    // Hat topology.
    out.push_str("[hats]\n");
    let mut hat_ids: Vec<&String> = config.hats.keys().collect();
    hat_ids.sort();
    for hat_id in hat_ids {
        let hat = &config.hats[hat_id];
        let mut triggers = hat.triggers.clone();
        triggers.sort();
        let mut publishes = hat.publishes.clone();
        publishes.sort();
        let mut terminal = hat.terminal_events.clone();
        terminal.sort();
        let mut exempt = hat.exempt_topics.clone();
        exempt.sort();
        out.push_str(&format!("hat={hat_id}\n"));
        out.push_str(&format!("  triggers={}\n", triggers.join(",")));
        out.push_str(&format!("  publishes={}\n", publishes.join(",")));
        out.push_str(&format!("  terminal={}\n", terminal.join(",")));
        out.push_str(&format!(
            "  default_publishes={}\n",
            hat.default_publishes.as_deref().unwrap_or("")
        ));
        out.push_str(&format!(
            "  max_activations={}\n",
            hat.max_activations.map(|v| v.to_string()).unwrap_or_default()
        ));
        out.push_str(&format!("  exempt={}\n", exempt.join(",")));
        out.push_str(&format!(
            "  backend={}\n",
            hat.backend
                .as_ref()
                .map(|b| b.to_cli_backend().to_string())
                .unwrap_or_default()
        ));
    }

    // Topic deny rules.
    out.push_str("[deny_rules]\n");
    if let Some(policy) = &config.event_loop.event_policy {
        let mut rules: Vec<(String, String)> = policy
            .topic_deny_rules
            .iter()
            .map(|r| (r.hat_id.clone(), r.topic.clone()))
            .collect();
        rules.sort();
        for (hat_id, topic) in rules {
            out.push_str(&format!("{hat_id}>{topic}\n"));
        }
    }

    // Execution contracts.
    out.push_str("[execution_contracts]\n");
    if let Some(contracts) = &config.event_loop.execution_contracts {
        out.push_str(&format!("enabled={}\n", contracts.enabled));
        let mut topics: Vec<&String> = contracts.rules.keys().collect();
        topics.sort();
        for topic in topics {
            let rule = &contracts.rules[topic];
            let mut fields = rule.require_payload_fields.clone();
            fields.sort();
            out.push_str(&format!("contract={topic}|fields={}\n", fields.join(",")));
        }
    }

    // Terminal / completion identity.
    out.push_str("[terminal]\n");
    out.push_str(&format!(
        "completion_promise={}\n",
        config.event_loop.completion_promise
    ));
    out.push_str(&format!(
        "starting_event={}\n",
        config.event_loop.starting_event.as_deref().unwrap_or("")
    ));
    if let Some(policy) = &config.event_loop.event_policy {
        let mut terminal_topics = policy.terminal_topics.clone();
        terminal_topics.sort();
        out.push_str(&format!("terminal_topics={}\n", terminal_topics.join(",")));
    }

    out.into_bytes()
}

/// SHA-256 hex digest of the canonical contract bytes.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::execution_contracts::ExecutionContractsConfig;
    use crate::config::{
        EventPolicyConfig, EventSchema, ExecutionContractRule, PrecheckConfig, PrecheckOnFail,
        PrecheckRule, TopicDenyRule,
    };

    /// Build a small but valid multi-hat config that compiles cleanly: every
    /// declared contract topic has a consumer, no orphan declarations.
    fn valid_config() -> RalphConfig {
        let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
cli:
  backend: "claude"
hats:
  executor:
    name: "Executor"
    description: "Does the work"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]
    instructions: "do work"
  coordinator:
    name: "Coordinator"
    description: "Consumes completion"
    triggers: ["work.done"]
    publishes: ["LOOP_COMPLETE"]
    terminal_events: ["LOOP_COMPLETE"]
    instructions: "coordinate"
"#;
        RalphConfig::parse_yaml(yaml).expect("valid config must parse")
    }

    #[test]
    fn task_capability_separates_coordinator_admin_from_execution() {
        let task = crate::task::Task::new("unit".to_string(), 1)
            .with_loop_id(Some("loop-1".to_string()))
            .with_owner_hat(Some("executor".to_string()));
        let coordinators = vec!["dispatcher".to_string()];

        let capability = evaluate_task_capability(
            &task,
            Some("dispatcher"),
            Some("loop-1"),
            &coordinators,
        );
        assert!(capability.lifecycle_administration);
        assert!(!capability.execution_ownership);
        assert!(!capability.actionable_now);
        assert_eq!(capability.deny_reason, Some("not_execution_owner"));
    }

    #[test]
    fn task_capability_owner_is_actionable_only_when_ready() {
        let mut task = crate::task::Task::new("unit".to_string(), 1)
            .with_loop_id(Some("loop-1".to_string()))
            .with_owner_hat(Some("executor".to_string()));
        let capability =
            evaluate_task_capability(&task, Some("executor"), Some("loop-1"), &[]);
        assert!(capability.lifecycle_administration);
        assert!(capability.execution_ownership);
        assert!(capability.actionable_now);

        task.blocked_by.push("task-blocker".to_string());
        let blocked = evaluate_task_capability(&task, Some("executor"), Some("loop-1"), &[]);
        assert!(blocked.execution_ownership);
        assert!(!blocked.actionable_now);
        assert_eq!(blocked.deny_reason, Some("task_blocked"));
    }

    #[test]
    fn glob_deny_pattern_denies_matching_topic() {
        let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    topic_deny_rules:
      - {hat_id: debug, topic: "debug.*"}
cli:
  backend: "claude"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]
    instructions: "do work"
  debug:
    name: "Debug"
    triggers: ["work.done"]
    publishes: ["debug.step"]
    instructions: "debug"
"#;
        let resolved = compile(RalphConfig::parse_yaml(yaml).expect("parse"))
            .expect("compile must succeed");
        let glob = resolved
            .contract()
            .glob_denies
            .get("debug")
            .expect("glob must be stored for hat=debug");
        assert_eq!(glob, &vec!["debug.*".to_string()]);
        assert_eq!(
            resolved.contract().emit_decision("debug", "debug.step"),
            EmitDecision::Deny,
            "glob deny must reject matching topic"
        );
        assert_eq!(
            resolved.contract().emit_decision("debug", "debug.done"),
            EmitDecision::Deny,
            "glob deny must cover every concrete matching topic"
        );
        assert_eq!(
            resolved.contract().emit_decision("executor", "work.done"),
            EmitDecision::Allow,
            "non-denied (hat, topic) stays allow"
        );
    }

    #[test]
    fn task_capability_fails_closed_across_loops() {
        let task = crate::task::Task::new("unit".to_string(), 1)
            .with_loop_id(Some("loop-2".to_string()))
            .with_owner_hat(Some("executor".to_string()));
        let capability =
            evaluate_task_capability(&task, Some("executor"), Some("loop-1"), &[]);
        assert!(!capability.lifecycle_administration);
        assert!(!capability.execution_ownership);
        assert!(!capability.actionable_now);
        assert_eq!(capability.deny_reason, Some("task_wrong_loop"));
    }

    #[test]
    fn compile_accepts_a_valid_config() {
        let resolved = compile(valid_config()).expect("valid config must compile Ok");
        assert!(!resolved.digest().is_empty(), "digest must be non-empty");
    }

    #[test]
    fn compile_is_deterministic_same_input_same_digest() {
        let a = compile(valid_config()).expect("compile a");
        let b = compile(valid_config()).expect("compile b");
        assert_eq!(
            a.digest(),
            b.digest(),
            "same config must yield the same contract digest"
        );
    }

    #[test]
    fn digest_is_sensitive_to_profile_overlay() {
        let base = compile(valid_config()).expect("base");
        let mut changed = valid_config();
        changed.profiles.default = vec![crate::config::ProfileSpec::parse_str("repo:strict")
            .expect("profile spec parses")];
        let changed = compile(changed).expect("changed");
        assert_ne!(
            base.digest(),
            changed.digest(),
            "activating a profile must change the digest"
        );
    }

    #[test]
    fn digest_is_sensitive_to_cli_overlay() {
        let base = compile(valid_config()).expect("base");
        let mut changed = valid_config();
        changed.cli.backend = "gemini".to_string();
        let changed = compile(changed).expect("changed");
        assert_ne!(
            base.digest(),
            changed.digest(),
            "a CLI backend overlay must change the digest"
        );
    }

    #[test]
    fn digest_is_sensitive_to_schema() {
        let base = compile(valid_config()).expect("base");
        let mut changed = valid_config();
        let mut policy = EventPolicyConfig::default();
        policy.enabled = true;
        let mut schema = EventSchema::default();
        schema.required_fields = vec!["task_id".to_string()];
        policy.schemas.insert("work.done".to_string(), schema);
        changed.event_loop.event_policy = Some(policy);
        let changed = compile(changed).expect("changed");
        assert_ne!(
            base.digest(),
            changed.digest(),
            "adding an event schema must change the digest"
        );
    }

    #[test]
    fn digest_is_sensitive_to_desugar_input() {
        let base = compile(valid_config()).expect("base");
        let mut changed = valid_config();
        let mut precheck = PrecheckConfig::default();
        precheck.enabled = true;
        precheck.rules.insert(
            "work.done".to_string(),
            PrecheckRule {
                prompt: vec!["did you run the tests?".to_string()],
                on_fail: PrecheckOnFail {
                    target: "executor".to_string(),
                    retry_budget: 3,
                    on_exhausted: "plan.blocked".to_string(),
                    reason: "precheck failed".to_string(),
                },
            },
        );
        changed.event_loop.precheck = Some(precheck);
        let changed = compile(changed).expect("changed");
        assert_ne!(
            base.digest(),
            changed.digest(),
            "changing the precheck desugar input must change the digest"
        );
    }

    #[test]
    fn deny_wins_over_publish_allow() {
        let mut config = valid_config();
        let mut policy = EventPolicyConfig::default();
        policy.enabled = true;
        policy.topic_deny_rules.push(TopicDenyRule {
            hat_id: "executor".to_string(),
            topic: "work.done".to_string(),
        });
        config.event_loop.event_policy = Some(policy);

        let resolved = compile(config).expect("config with deny rule must still compile");
        let contract = resolved.contract();
        // `executor` publishes `work.done` (an allow), but the explicit deny
        // rule must win.
        assert!(
            contract
                .emit_denies
                .contains(&("executor".to_string(), "work.done".to_string())),
            "the deny rule must be recorded"
        );
        assert_eq!(
            contract.emit_decision("executor", "work.done"),
            EmitDecision::Deny,
            "deny must override the publish-side allow"
        );
    }

    #[test]
    fn publish_without_deny_is_allowed() {
        let resolved = compile(valid_config()).expect("valid config");
        assert_eq!(
            resolved.contract().emit_decision("executor", "work.done"),
            EmitDecision::Allow,
            "a published topic with no deny rule is allowed"
        );
    }

    #[test]
    fn unknown_capability_is_fail_closed() {
        let resolved = compile(valid_config()).expect("valid config");
        // `executor` does not publish `LOOP_COMPLETE` and there is no rule for
        // it → fail-closed deny (R4 unknown capability).
        assert_eq!(
            resolved.contract().emit_decision("executor", "LOOP_COMPLETE"),
            EmitDecision::Deny,
            "unknown emit capability must fail closed"
        );
    }

    #[test]
    fn missing_consumer_for_declared_contract_is_a_finding() {
        let mut config = valid_config();
        // Declare an execution contract for a topic that NO hat consumes and
        // that is not terminal/completion. This is a genuine gap.
        let mut contracts = ExecutionContractsConfig::default();
        contracts.enabled = true;
        contracts.rules.insert(
            "orphan.topic".to_string(),
            ExecutionContractRule {
                require_payload_fields: vec!["task_id".to_string()],
                ..Default::default()
            },
        );
        config.event_loop.execution_contracts = Some(contracts);

        let err = compile(config).expect_err("orphan contract topic must fail compile");
        assert!(
            err.findings().iter().any(|f| matches!(
                &f.kind,
                ContractCompileFindingKind::MissingConsumer { topic } if topic == "orphan.topic"
            )),
            "expected a MissingConsumer finding for orphan.topic, got: {err:?}"
        );
        // The rendered message must be operator-readable.
        assert!(err.to_string().contains("orphan.topic"));
    }

    #[test]
    fn declared_contract_with_consumer_compiles_ok() {
        let mut config = valid_config();
        // `work.done` is consumed by the `coordinator` hat, so a contract on it
        // is complete.
        let mut contracts = ExecutionContractsConfig::default();
        contracts.enabled = true;
        contracts.rules.insert(
            "work.done".to_string(),
            ExecutionContractRule {
                require_payload_fields: vec!["task_id".to_string()],
                ..Default::default()
            },
        );
        config.event_loop.execution_contracts = Some(contracts);

        let resolved = compile(config).expect("contract topic with a consumer must compile");
        assert!(resolved
            .contract()
            .declared_contract_topics
            .contains("work.done"));
    }

    #[test]
    fn disabled_contracts_skip_consumer_check() {
        let mut config = valid_config();
        // enabled = false (default): rules are parsed but not applied, so no
        // consumer check runs even for an orphan topic.
        let mut contracts = ExecutionContractsConfig::default();
        contracts.enabled = false;
        contracts.rules.insert(
            "orphan.topic".to_string(),
            ExecutionContractRule::default(),
        );
        config.event_loop.execution_contracts = Some(contracts);

        compile(config).expect("disabled contracts must not produce findings");
    }

    #[test]
    fn terminal_contract_topic_needs_no_consumer() {
        let mut config = valid_config();
        // `LOOP_COMPLETE` is the completion promise / terminal — a contract on
        // it needs no downstream consumer.
        let mut contracts = ExecutionContractsConfig::default();
        contracts.enabled = true;
        contracts
            .rules
            .insert("LOOP_COMPLETE".to_string(), ExecutionContractRule::default());
        config.event_loop.execution_contracts = Some(contracts);

        compile(config).expect("terminal contract topic must compile without a consumer");
    }
}
