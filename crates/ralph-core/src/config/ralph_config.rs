use super::*;
use crate::diagnostics::DiagnosticsOptions;
use std::collections::HashMap;
use std::path::Path;
use tracing::debug;

// 2026-07-02-004 plan milestone A: explicit import for
// `build_gate_instructions` even though `use super::*`
// already brings it in. Keeps the dependency visible
// for code review.
use super::precheck::PrecheckRule;

impl RalphConfig {
    /// Loads configuration from a YAML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path_ref = path.as_ref();
        debug!(path = %path_ref.display(), "Loading configuration from file");
        let content = std::fs::read_to_string(path_ref)?;
        Self::parse_yaml(&content)
    }

    /// Parses configuration from a YAML string.
    pub fn parse_yaml(content: &str) -> Result<Self, ConfigError> {
        // Pre-flight check for deprecated/invalid keys to improve UX.
        let value: serde_yaml::Value = serde_yaml::from_str(content)?;
        if let Some(map) = value.as_mapping()
            && map.contains_key(serde_yaml::Value::String("project".to_string()))
        {
            return Err(ConfigError::DeprecatedProjectKey);
        }

        hooks::validate_hooks_phase_event_keys(&value)?;

        let config: Self = serde_yaml::from_value(value)?;
        debug!(
            backend = %config.cli.backend,
            has_v1_fields = config.agent.is_some(),
            custom_hats = config.hats.len(),
            "Configuration loaded"
        );
        Ok(config)
    }

    /// Normalizes v1 flat fields into v2 nested structure.
    ///
    /// V1 flat fields take precedence over v2 nested fields when both are present.
    /// This allows users to use either format or mix them.
    pub fn normalize(&mut self) {
        let mut normalized_count = 0;

        // Map v1 `agent` to v2 `cli.backend`
        if let Some(ref agent) = self.agent {
            debug!(from = "agent", to = "cli.backend", value = %agent, "Normalizing v1 field");
            self.cli.backend = agent.clone();
            normalized_count += 1;
        }

        // Map v1 `prompt_file` to v2 `event_loop.prompt_file`
        if let Some(ref pf) = self.prompt_file {
            debug!(from = "prompt_file", to = "event_loop.prompt_file", value = %pf, "Normalizing v1 field");
            self.event_loop.prompt_file = pf.clone();
            normalized_count += 1;
        }

        // Map v1 `completion_promise` to v2 `event_loop.completion_promise`
        if let Some(ref cp) = self.completion_promise {
            debug!(
                from = "completion_promise",
                to = "event_loop.completion_promise",
                "Normalizing v1 field"
            );
            self.event_loop.completion_promise = cp.clone();
            normalized_count += 1;
        }

        // Map v1 `max_iterations` to v2 `event_loop.max_iterations`
        if let Some(mi) = self.max_iterations {
            debug!(
                from = "max_iterations",
                to = "event_loop.max_iterations",
                value = mi,
                "Normalizing v1 field"
            );
            self.event_loop.max_iterations = mi;
            normalized_count += 1;
        }

        // Map v1 `max_runtime` to v2 `event_loop.max_runtime_seconds`
        if let Some(mr) = self.max_runtime {
            debug!(
                from = "max_runtime",
                to = "event_loop.max_runtime_seconds",
                value = mr,
                "Normalizing v1 field"
            );
            self.event_loop.max_runtime_seconds = mr;
            normalized_count += 1;
        }

        // Map v1 `max_cost` to v2 `event_loop.max_cost_usd`
        if self.max_cost.is_some() {
            debug!(
                from = "max_cost",
                to = "event_loop.max_cost_usd",
                "Normalizing v1 field"
            );
            self.event_loop.max_cost_usd = self.max_cost;
            normalized_count += 1;
        }

        // Merge extra_instructions into instructions for each hat
        for (hat_id, hat) in &mut self.hats {
            if !hat.extra_instructions.is_empty() {
                for fragment in hat.extra_instructions.drain(..) {
                    if !hat.instructions.ends_with('\n') {
                        hat.instructions.push('\n');
                    }
                    hat.instructions.push_str(&fragment);
                }
                debug!(hat = %hat_id, "Merged extra_instructions into hat instructions");
                normalized_count += 1;
            }
        }

        // 2026-07-02-004 plan milestone A (U2): precheck desugar.
        // Rewrites producers of each guarded topic to emit
        // `<topic>.proposed` and synthesizes a gate hat. Strict
        // no-op when `precheck.enabled` is false, when
        // `rules` is empty, or when `RALPH_PRECHECK_MODE=off`.
        self.apply_precheck_desugar();

        if normalized_count > 0 {
            debug!(
                fields_normalized = normalized_count,
                "V1 to V2 config normalization complete"
            );
        }
    }

    /// 2026-07-02-004 plan milestone A (U2/U3): precheck desugar.
    /// For each `event_loop.precheck.rules.<X>`:
    /// 1. Rewrite every hat whose `publishes` or
    ///    `terminal_events` contains `X` so those entries become
    ///    `X.proposed`. Consumer hats (which only reference `X` in
    ///    their `triggers`) are NOT touched.
    /// 1b. Rewrite `default_publishes: X` to `X.proposed` as well —
    ///    the runtime fallback injection must pass through the same
    ///    gate as a producer emit, otherwise a silent hat's fallback
    ///    event bypasses evidence audit / retry budget / escalation.
    /// 2. Synthesize a new hat `precheck-<X>` that:
    ///    - triggers on `X.proposed`,
    ///    - publishes `X` and `X.rejected`,
    ///    - has `terminal_events = [X, X.rejected]`,
    ///    - carries the rendered checklist in its `instructions`,
    ///    - has a `max_activations` cap of `retry_budget + 1`
    ///      (one initial + allowed retries).
    ///
    /// Strict no-op when:
    /// - `precheck` is `None` or `enabled = false`,
    /// - `rules` is empty,
    /// - `RALPH_PRECHECK_MODE=off` is set in the environment.
    fn apply_precheck_desugar(&mut self) {
        let precheck = match self.event_loop.precheck.as_ref() {
            Some(p) if p.enabled && !p.rules.is_empty() => p.clone(),
            _ => return,
        };
        if !super::precheck::precheck_runtime_enabled() {
            return;
        }

        for (topic, rule) in &precheck.rules {
            let proposed = format!("{topic}.proposed");
            let rejected = format!("{topic}.rejected");

            for (hat_id, hat) in &mut self.hats {
                let publishes_topic = hat.publishes.iter().any(|p| p == topic);
                let terminal_topic = hat.terminal_events.iter().any(|t| t == topic);
                let default_topic = hat.default_publishes.as_deref() == Some(topic.as_str());

                if !publishes_topic && !terminal_topic && !default_topic {
                    continue;
                }

                // `default_publishes` is a runtime fallback emit path
                // (`check_default_publishes` injects the topic directly onto
                // the bus when the hat wrote no events). Without this rewrite
                // the injected bare `<X>` bypasses the gate entirely: the gate
                // only triggers on `<X>.proposed`, so the fallback event would
                // reach downstream consumers with no evidence audit, no retry
                // budget, and no `plan.blocked` escalation. Route the fallback
                // through the same gate as a producer emit.
                if default_topic {
                    hat.default_publishes = Some(proposed.clone());
                }

                if publishes_topic {
                    hat.publishes.retain(|p| p != topic);
                    hat.publishes.push(proposed.clone());
                    hat.publishes.sort();
                    hat.publishes.dedup();
                }
                if terminal_topic {
                    hat.terminal_events.retain(|t| t != topic);
                    hat.terminal_events.push(proposed.clone());
                    hat.terminal_events.sort();
                    hat.terminal_events.dedup();
                }
                debug!(hat = %hat_id, topic = %topic, "Rewrote producer to emit proposed variant");
            }

            let instructions = build_gate_instructions(topic, rule);

            let gate_id = format!("precheck-{topic}");
            let max_activations = rule.on_fail.retry_budget.saturating_add(1);
            let gate = HatConfig {
                name: format!("Precheck Gate: {topic}"),
                description: Some(format!(
                    "LLM-as-judge gate for `{topic}`. Renders the declared checklist and \
                     passes or rejects the proposed event before it reaches downstream hats."
                )),
                triggers: vec![proposed.clone()],
                publishes: vec![topic.clone(), rejected.clone()],
                terminal_events: vec![topic.clone(), rejected.clone()],
                instructions,
                max_activations: Some(max_activations),
                ..Default::default()
            };
            self.hats.insert(gate_id.clone(), gate);
            super::precheck::inject_precheck_event_schemas(self, topic);
            debug!(gate = %gate_id, topic = %topic, "Synthesized precheck gate hat");
        }
    }

    /// Validates the configuration and returns warnings.
    ///
    /// This method checks for:
    /// - Deferred features that are enabled (archive_prompts, enable_metrics)
    /// - Dropped fields that are present (max_tokens, retry_delay, tool_permissions)
    /// - Ambiguous trigger routing across custom hats
    /// - Mutual exclusivity of prompt and prompt_file
    ///
    /// Returns a list of warnings that should be displayed to the user.
    pub fn validate(&self) -> Result<Vec<ConfigWarning>, ConfigError> {
        let mut warnings = Vec::new();

        // Skip all warnings if suppressed
        if self.suppress_warnings {
            return Ok(warnings);
        }

        // Check for mutual exclusivity of prompt and prompt_file in config
        // Only error if both are explicitly set (not defaults)
        if self.event_loop.prompt.is_some()
            && !self.event_loop.prompt_file.is_empty()
            && self.event_loop.prompt_file != loop_config::default_prompt_file()
        {
            return Err(ConfigError::MutuallyExclusive {
                field1: "event_loop.prompt".to_string(),
                field2: "event_loop.prompt_file".to_string(),
            });
        }
        if self.event_loop.completion_promise.trim().is_empty() {
            return Err(ConfigError::InvalidCompletionPromise);
        }

        for (idx, gate) in self.event_loop.path_required_events.iter().enumerate() {
            if gate.anchor.trim().is_empty() {
                return Err(ConfigError::PathRequiredValidation {
                    field: format!("event_loop.path_required_events[{idx}].anchor"),
                    message: "anchor topic cannot be empty".to_string(),
                });
            }
            if gate.require.is_empty() {
                return Err(ConfigError::PathRequiredValidation {
                    field: format!("event_loop.path_required_events[{idx}].require"),
                    message: "require list cannot be empty".to_string(),
                });
            }
            for (req_idx, topic) in gate.require.iter().enumerate() {
                if topic.trim().is_empty() {
                    return Err(ConfigError::PathRequiredValidation {
                        field: format!("event_loop.path_required_events[{idx}].require[{req_idx}]"),
                        message: "require topic cannot be empty".to_string(),
                    });
                }
            }
        }

        // Check custom backend has a command
        if self.cli.backend == "custom" && self.cli.command.as_ref().is_none_or(String::is_empty) {
            return Err(ConfigError::CustomBackendRequiresCommand);
        }

        // Check for deferred features
        if self.archive_prompts {
            warnings.push(ConfigWarning::DeferredFeature {
                field: "archive_prompts".to_string(),
                message: "Feature not yet available in v2".to_string(),
            });
        }

        if self.enable_metrics {
            warnings.push(ConfigWarning::DeferredFeature {
                field: "enable_metrics".to_string(),
                message: "Feature not yet available in v2".to_string(),
            });
        }

        // Check for dropped fields
        if self.max_tokens.is_some() {
            warnings.push(ConfigWarning::DroppedField {
                field: "max_tokens".to_string(),
                reason: "Token limits are controlled by the CLI tool".to_string(),
            });
        }

        if self.retry_delay.is_some() {
            warnings.push(ConfigWarning::DroppedField {
                field: "retry_delay".to_string(),
                reason: "Retry logic handled differently in v2".to_string(),
            });
        }

        if let Some(threshold) = self.event_loop.mutation_score_warn_threshold
            && !(0.0..=100.0).contains(&threshold)
        {
            warnings.push(ConfigWarning::InvalidValue {
                field: "event_loop.mutation_score_warn_threshold".to_string(),
                message: "Value must be between 0 and 100".to_string(),
            });
        }

        // Check adapter tool_permissions (dropped field)
        if self.adapters.claude.tool_permissions.is_some()
            || self.adapters.gemini.tool_permissions.is_some()
            || self.adapters.codex.tool_permissions.is_some()
        {
            warnings.push(ConfigWarning::DroppedField {
                field: "adapters.*.tool_permissions".to_string(),
                reason: "CLI tool manages its own permissions".to_string(),
            });
        }

        // Validate telemetry / runtime-diagnosis config (U1). Soft
        // warnings (e.g. enabled=false && write_artifacts=true) are
        // returned through the same channel as other warnings; hard
        // errors short-circuit validate.
        let telemetry_warnings = self.telemetry.validate()?;
        warnings.extend(telemetry_warnings);

        // Validate notifications config (U1 of plan 2026-07-25-001).
        // Hard errors short-circuit; no warnings emitted in this unit.
        self.notifications.validate()?;

        // Validate hooks config semantics (v1 guardrails)
        self.validate_hooks()?;

        // Check for required description field on all hats
        for (hat_id, hat_config) in &self.hats {
            if hat_config
                .description
                .as_ref()
                .is_none_or(|d| d.trim().is_empty())
            {
                return Err(ConfigError::MissingDescription {
                    hat: hat_id.clone(),
                });
            }
        }

        // Check wave config validity
        for (hat_id, hat_config) in &self.hats {
            if hat_config.concurrency == 0 {
                return Err(ConfigError::InvalidConcurrency {
                    hat: hat_id.clone(),
                    value: 0,
                });
            }
            if hat_config.aggregate.is_some() && hat_config.concurrency > 1 {
                return Err(ConfigError::AggregateOnConcurrentHat {
                    hat: hat_id.clone(),
                });
            }
        }

        // Check for reserved triggers: task.start and task.resume are reserved for Ralph
        // Per design: Ralph coordinates first, then delegates to custom hats via events.
        // Exception: the `coordinator` hat is Ralph's built-in coordinator and
        // legitimately subscribes to `task.resume` so the typed dispatch
        // (plan 2026-06-23-004 U4, CB-4 contract bug fix) can route
        // rejections back to the orchestrator's coordinator hat.
        const RESERVED_TRIGGERS: &[&str] = &["task.start", "task.resume"];
        for (hat_id, hat_config) in &self.hats {
            // 2026-06-23 fix (CB-4): the `coordinator` hat is
            // Ralph's own. It owns the `task.resume` /
            // `task.start` subscription rights.
            let is_coordinator = hat_id == "coordinator";
            for trigger in &hat_config.triggers {
                if RESERVED_TRIGGERS.contains(&trigger.as_str()) && !is_coordinator {
                    return Err(ConfigError::ReservedTrigger {
                        trigger: trigger.clone(),
                        hat: hat_id.clone(),
                    });
                }
            }
        }

        // Validate terminal_events: each terminal topic must exist in the hat's publishes.
        // Empty terminal_events is allowed (legacy hats) but emits a warning.
        for (hat_id, hat_config) in &self.hats {
            if hat_config.terminal_events.is_empty() {
                warnings.push(ConfigWarning::EmptyTerminalEvents {
                    hat: hat_id.clone(),
                });
            } else {
                let publishes: std::collections::HashSet<&str> =
                    hat_config.publishes.iter().map(String::as_str).collect();
                for topic in &hat_config.terminal_events {
                    if !publishes.contains(topic.as_str()) {
                        return Err(ConfigError::TerminalTopicNotInPublishes {
                            hat: hat_id.clone(),
                            topic: topic.clone(),
                        });
                    }
                }
            }
        }

        // Validate workflow guard config
        if let Some(workflow_guards) = &self.event_loop.workflow_guards {
            let mut seen_chain_names = std::collections::HashSet::new();
            for chain in &workflow_guards.chains {
                if chain.name.trim().is_empty() {
                    return Err(ConfigError::WorkflowGuardValidation {
                        field: "event_loop.workflow_guards.chains[].name".to_string(),
                        message: "Chain name cannot be empty".to_string(),
                    });
                }
                if !seen_chain_names.insert(&chain.name) {
                    return Err(ConfigError::WorkflowGuardValidation {
                        field: format!("event_loop.workflow_guards.chains.{}", chain.name),
                        message: format!("Duplicate workflow chain name '{}'", chain.name),
                    });
                }
                if chain.topics.is_empty() {
                    return Err(ConfigError::WorkflowGuardValidation {
                        field: format!("event_loop.workflow_guards.chains.{}.topics", chain.name),
                        message: "Workflow chain topics cannot be empty".to_string(),
                    });
                }
                let mut seen_topics = std::collections::HashSet::new();
                for topic in &chain.topics {
                    if !seen_topics.insert(topic) {
                        return Err(ConfigError::WorkflowGuardValidation {
                            field: format!(
                                "event_loop.workflow_guards.chains.{}.topics",
                                chain.name
                            ),
                            message: format!(
                                "Duplicate topic '{}' in workflow chain '{}'",
                                topic, chain.name
                            ),
                        });
                    }
                }
            }
        }

        // Validate state machine config
        if let Some(state_machine) = &self.event_loop.state_machine
            && state_machine.enabled
        {
            let mut seen_transition_topics = std::collections::HashSet::new();
            for transition in &state_machine.transitions {
                if transition.topic.trim().is_empty() {
                    return Err(ConfigError::StateMachineValidation {
                        field: "event_loop.state_machine.transitions[].topic".to_string(),
                        message: "Transition topic cannot be empty".to_string(),
                    });
                }
                if !seen_transition_topics.insert(&transition.topic) {
                    return Err(ConfigError::StateMachineValidation {
                        field: format!("event_loop.state_machine.transitions.{}", transition.topic),
                        message: format!("Duplicate transition topic '{}'", transition.topic),
                    });
                }
                if transition.from.is_empty() {
                    return Err(ConfigError::StateMachineValidation {
                        field: format!(
                            "event_loop.state_machine.transitions.{}.from",
                            transition.topic
                        ),
                        message: "Transition from states cannot be empty".to_string(),
                    });
                }
                if transition.to.trim().is_empty() {
                    return Err(ConfigError::StateMachineValidation {
                        field: format!(
                            "event_loop.state_machine.transitions.{}.to",
                            transition.topic
                        ),
                        message: "Transition target state cannot be empty".to_string(),
                    });
                }
                if transition.opens_instance && transition.closes_instance {
                    return Err(ConfigError::StateMachineValidation {
                        field: format!("event_loop.state_machine.transitions.{}", transition.topic),
                        message: "Transition cannot both open and close an instance".to_string(),
                    });
                }
            }
        }

        // Validate event policy config
        if let Some(event_policy) = &self.event_loop.event_policy
            && event_policy.enabled
        {
            for (topic, schema) in &event_policy.schemas {
                if topic.trim().is_empty() {
                    return Err(ConfigError::EventPolicyValidation {
                        field: "event_loop.event_policy.schemas".to_string(),
                        message: "Schema topic cannot be empty".to_string(),
                    });
                }
                // 2026-07-09-001 plan (U1): reject empty
                // `field_docs` keys. Non-required-field entries
                // are still allowed (documentation ≠ validation),
                // but a key like `""` would silently match no
                // real field and confuse the agent prompt
                // builder / policy-check error paths.
                for key in schema.field_docs.keys() {
                    if key.trim().is_empty() {
                        return Err(ConfigError::EventPolicyValidation {
                            field: format!("event_loop.event_policy.schemas.{}.field_docs", topic),
                            message: "Field doc key cannot be empty".to_string(),
                        });
                    }
                }
                for path in schema.allowed_values.keys() {
                    if path.trim().is_empty() {
                        return Err(ConfigError::EventPolicyValidation {
                            field: format!(
                                "event_loop.event_policy.schemas.{}.allowed_values",
                                topic
                            ),
                            message: "Field path cannot be empty".to_string(),
                        });
                    }
                    if path.contains("..") {
                        return Err(ConfigError::EventPolicyValidation {
                            field: format!(
                                "event_loop.event_policy.schemas.{}.allowed_values",
                                topic
                            ),
                            message: format!("Field path '{}' contains consecutive dots", path),
                        });
                    }
                    if path.starts_with('.') || path.ends_with('.') {
                        return Err(ConfigError::EventPolicyValidation {
                            field: format!(
                                "event_loop.event_policy.schemas.{}.allowed_values",
                                topic
                            ),
                            message: format!("Field path '{}' starts or ends with a dot", path),
                        });
                    }
                    if path.split('.').any(|s| s.is_empty()) {
                        return Err(ConfigError::EventPolicyValidation {
                            field: format!(
                                "event_loop.event_policy.schemas.{}.allowed_values",
                                topic
                            ),
                            message: format!("Field path '{}' contains empty segment", path),
                        });
                    }
                }
            }
        }

        // Check for ambiguous routing: each trigger topic must map to exactly one hat
        // Per spec: "Every trigger maps to exactly one hat | No ambiguous routing"
        //
        // Exception: when ALL hats subscribed to a given trigger explicitly list
        // that trigger in their `trigger_multi_consumer_topics`, the strict 1:1
        // check is bypassed. This is the documented escape hatch for design-level
        // multi-consumer topics. (Note: ce-executor-serial previously used this
        // for `fix.exhausted` / `debug.exhausted`, but removed it on 2026-06-24
        // due to a round-robin scheduling race; the mechanism remains for
        // future presets that need it.)
        if !self.hats.is_empty() {
            let mut trigger_to_hats: HashMap<&str, Vec<&str>> = HashMap::new();
            for (hat_id, hat_config) in &self.hats {
                for trigger in &hat_config.triggers {
                    trigger_to_hats
                        .entry(trigger.as_str())
                        .or_default()
                        .push(hat_id.as_str());
                }
            }
            for (trigger, hats) in trigger_to_hats {
                if hats.len() > 1 {
                    let all_allowed = hats.iter().all(|hat_id| {
                        self.hats
                            .get(*hat_id)
                            .map(|hc| hc.trigger_multi_consumer_topics.contains(trigger))
                            .unwrap_or(false)
                    });
                    if !all_allowed {
                        return Err(ConfigError::AmbiguousRouting {
                            trigger: trigger.to_string(),
                            hat1: hats[0].to_string(),
                            hat2: hats[1].to_string(),
                        });
                    }
                }
            }
        }

        Ok(warnings)
    }

    fn validate_hooks(&self) -> Result<(), ConfigError> {
        Self::validate_non_v1_hook_fields("hooks", &self.hooks.extra)?;

        if self.hooks.defaults.timeout_seconds == 0 {
            return Err(ConfigError::HookValidation {
                field: "hooks.defaults.timeout_seconds".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }

        if self.hooks.defaults.max_output_bytes == 0 {
            return Err(ConfigError::HookValidation {
                field: "hooks.defaults.max_output_bytes".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }

        for (phase_event, hook_specs) in &self.hooks.events {
            for (index, hook) in hook_specs.iter().enumerate() {
                let hook_field_base = format!("hooks.events.{phase_event}[{index}]");

                if hook.name.trim().is_empty() {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.name"),
                        message: "is required and must be non-empty".to_string(),
                    });
                }

                if hook
                    .command
                    .first()
                    .is_none_or(|command| command.trim().is_empty())
                {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.command"),
                        message: "is required and must include an executable at command[0]"
                            .to_string(),
                    });
                }

                if hook.on_error.is_none() {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.on_error"),
                        message: "is required in v1 (warn | block | suspend)".to_string(),
                    });
                }

                if let Some(timeout_seconds) = hook.timeout_seconds
                    && timeout_seconds == 0
                {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.timeout_seconds"),
                        message: "must be greater than 0 when specified".to_string(),
                    });
                }

                if let Some(max_output_bytes) = hook.max_output_bytes
                    && max_output_bytes == 0
                {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.max_output_bytes"),
                        message: "must be greater than 0 when specified".to_string(),
                    });
                }

                if hook.suspend_mode.is_some() && hook.on_error != Some(HookOnError::Suspend) {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.suspend_mode"),
                        message: "requires on_error: suspend".to_string(),
                    });
                }

                Self::validate_non_v1_hook_fields(&hook_field_base, &hook.extra)?;
                Self::validate_mutation_contract(&hook_field_base, &hook.mutate)?;
            }
        }

        Ok(())
    }

    fn validate_non_v1_hook_fields(
        path_prefix: &str,
        fields: &HashMap<String, serde_yaml::Value>,
    ) -> Result<(), ConfigError> {
        for key in fields.keys() {
            let field = format!("{path_prefix}.{key}");
            match key.as_str() {
                "global" | "globals" | "global_defaults" | "global_hooks" | "scope" => {
                    return Err(ConfigError::UnsupportedHookField {
                        field,
                        reason: "Use ~/.ralph/config.yml for user-level defaults; per-hook `global`/`scope` fields are not supported in v1"
                            .to_string(),
                    });
                }
                "parallel" | "parallelism" | "max_parallel" | "concurrency" | "run_in_parallel" => {
                    return Err(ConfigError::UnsupportedHookField {
                        field,
                        reason:
                            "Parallel hook execution is out of scope for v1; hooks must run sequentially"
                                .to_string(),
                    });
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn validate_mutation_contract(
        hook_field_base: &str,
        mutate: &HookMutationConfig,
    ) -> Result<(), ConfigError> {
        let mutate_field_base = format!("{hook_field_base}.mutate");

        if !mutate.enabled {
            if mutate.format.is_some() || !mutate.extra.is_empty() {
                return Err(ConfigError::HookValidation {
                    field: mutate_field_base,
                    message: "mutation settings require mutate.enabled: true".to_string(),
                });
            }
            return Ok(());
        }

        if let Some(format) = mutate.format.as_deref()
            && !format.eq_ignore_ascii_case("json")
        {
            return Err(ConfigError::HookValidation {
                field: format!("{mutate_field_base}.format"),
                message: "only 'json' is supported for v1 mutation payloads".to_string(),
            });
        }

        if let Some(key) = mutate.extra.keys().next() {
            let field = format!("{mutate_field_base}.{key}");
            let reason = match key.as_str() {
                "prompt" | "prompt_mutation" | "events" | "event" | "config" | "full_context" => {
                    "v1 allows metadata-only mutation; prompt/event/config mutation is unsupported"
                        .to_string()
                }
                "xml" => "v1 mutation payloads are JSON-only".to_string(),
                _ => "unsupported mutate field in v1 (supported keys: enabled, format)".to_string(),
            };

            return Err(ConfigError::UnsupportedHookField { field, reason });
        }

        Ok(())
    }

    /// Gets the effective backend name, resolving "auto" using the priority list.
    pub fn effective_backend(&self) -> &str {
        &self.cli.backend
    }

    /// Returns the agent priority list for auto-detection.
    /// If empty, returns the default priority order.
    pub fn get_agent_priority(&self) -> Vec<&str> {
        if self.agent_priority.is_empty() {
            vec!["claude", "gemini", "codex"]
        } else {
            self.agent_priority.iter().map(String::as_str).collect()
        }
    }

    /// Gets the adapter settings for a specific backend.
    #[allow(clippy::match_same_arms)] // Explicit match arms for each backend improves readability
    pub fn adapter_settings(&self, backend: &str) -> &AdapterSettings {
        match backend {
            "claude" => &self.adapters.claude,
            "gemini" => &self.adapters.gemini,
            "codex" => &self.adapters.codex,
            _ => &self.adapters.claude, // Default fallback
        }
    }

    /// Build a [`DiagnosticsOptions`] from this config + the
    /// `RALPH_DIAGNOSTICS` environment variable.
    ///
    /// The returned options drive the activation matrix in
    /// `crate::diagnostics::DiagnosticsCollector::with_options`. The CLI
    /// uses this to populate the authoritative collector U0 introduced,
    /// and U3 will read the same options to know whether to spin up the
    /// minimal diagnosis loggers (`recovery.jsonl`, `drift.jsonl`,
    /// `diagnosis-summary.json`).
    ///
    /// `workspace` is accepted for forward-compatibility with the
    /// session-dir-reuse path U3 may introduce. The U1 implementation
    /// ignores it; `session_dir` is always `None` here.
    #[must_use]
    pub fn diagnostics_options(&self, workspace: &Path) -> DiagnosticsOptions {
        self.telemetry.to_diagnostics_options(workspace)
    }

    /// Resolves the effective inactivity watchdog (seconds) for the
    /// autonomous / RPC / worktree path of `backend`.
    ///
    /// Resolution order (plan 2026-06-06-001, R5/R6):
    /// 1. `cli.autonomous_idle_timeout_secs` — explicit per-Ralph override.
    ///    `Some(0)` means *disabled*; `Some(N > 0)` means watchdog after N
    ///    seconds of silence. This is the documented escape hatch when a
    ///    specific operator wants to override the adapter-wide default.
    /// 2. `adapters.<backend>.timeout` — per-adapter inactivity timeout
    ///    (default 300s). This already carries the right "CLI execution
    ///    inactivity timeout" semantics, so the autonomous watchdog reuses
    ///    it instead of inventing a new field.
    ///
    /// Both sources use 0 to mean "no watchdog" (R8). The PTY executor and
    /// the CLI executor both honor that semantic.
    ///
    /// Callers typically pass this directly to `PtyConfig::idle_timeout_secs`
    /// or `CliExecutor::execute(... timeout ...)` so the watchdog fires
    /// consistently across PTY and non-PTY autonomous paths.
    pub fn autonomous_idle_timeout_secs(&self, backend: &str) -> u64 {
        self.cli
            .autonomous_idle_timeout_secs
            .unwrap_or_else(|| self.adapter_settings(backend).timeout)
    }

    /// Resolves and loads external schema files referenced in `event_policy.schema_file`.
    ///
    /// Relative paths are resolved against `base_path` (typically the preset file's directory).
    /// Inline schemas in `schemas` take priority over file schemas when both define the same topic.
    ///
    /// # Errors
    /// - Schema file does not exist
    /// - Schema file is not valid YAML
    /// - Schema file root is not a map (must be `topic: schema` pairs)
    pub fn resolve_schema_files(&mut self, base_path: &Path) -> Result<(), ConfigError> {
        let schema_file = match &self.event_loop.event_policy.as_ref() {
            Some(policy) => match &policy.schema_file {
                Some(f) => f,
                None => return Ok(()),
            },
            None => return Ok(()),
        };

        let file_path = Path::new(schema_file);
        let resolved_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            base_path.join(file_path)
        };

        if !resolved_path.exists() {
            return Err(ConfigError::SchemaFileNotFound {
                path: resolved_path.display().to_string(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"),
            });
        }

        let content = std::fs::read_to_string(&resolved_path)?;
        let value: serde_yaml::Value =
            serde_yaml::from_str(&content).map_err(|e| ConfigError::SchemaFileParseError {
                path: resolved_path.display().to_string(),
                source: e,
            })?;

        let map = value
            .as_mapping()
            .ok_or_else(|| ConfigError::SchemaFileNotMap {
                path: resolved_path.display().to_string(),
            })?;

        // Get existing inline schemas to merge with (inline takes priority)
        let inline_schemas = self
            .event_loop
            .event_policy
            .as_mut()
            .map(|p| std::mem::take(&mut p.schemas))
            .unwrap_or_default();

        // Build merged schemas: file schemas first, then inline overwrites
        let mut merged_schemas: HashMap<String, EventSchema> = HashMap::new();

        for (topic, schema_value) in map {
            let topic_str = topic.as_str().unwrap_or_default();
            let schema: EventSchema =
                serde_yaml::from_value(schema_value.clone()).map_err(|e| {
                    ConfigError::SchemaFileInvalidSchema {
                        path: resolved_path.display().to_string(),
                        topic: topic_str.to_string(),
                        source: e,
                    }
                })?;
            merged_schemas.insert(topic_str.to_string(), schema);
        }

        // Inline schemas overwrite file schemas for same topic (inline takes priority)
        for (topic, schema) in inline_schemas {
            merged_schemas.insert(topic, schema);
        }

        // Put merged schemas back
        if let Some(policy) = &mut self.event_loop.event_policy {
            policy.schemas = merged_schemas;
        }

        Ok(())
    }
}

/// Hooks configuration.
///
/// Controls per-project orchestrator lifecycle hooks. Hooks are disabled by
/// default and are inert until explicitly enabled.
///
/// Example configuration:
/// ```yaml
/// hooks:
///   enabled: true
///   defaults:
///     timeout_seconds: 30
///     max_output_bytes: 8192
///     suspend_mode: wait_for_resume
///   events:
///     pre.loop.start:
///       - name: env-guard
///         command: ["./scripts/hooks/env-guard.sh"]
///         on_error: block
/// ```
/// 2026-07-02-004 plan milestone A (U3): render the
/// declared checklist + hard-constraint instructions for a
/// synthesized precheck gate hat.
fn build_gate_instructions(topic: &str, rule: &PrecheckRule) -> String {
    let mut out = String::new();
    out.push_str(&format!("## PRECHECK GATE: {topic}\n\n"));
    out.push_str("You are an LLM-as-judge gate. A upstream hat just published `");
    out.push_str(&format!("{topic}.proposed"));
    out.push_str("`. You must decide whether the proposed event is acceptable.\n\n");
    out.push_str("### Checklist\n\n");
    for (i, item) in rule.prompt.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, item));
    }
    out.push_str("\n### Decision (hard constraint)\n\n");
    out.push_str(&format!(
        "You MUST emit exactly one of `{topic}` (pass) or `{topic}.rejected` (fail).\n"
    ));
    out.push_str(&format!(
        "If you reject, fill the `failed_checks` array (the 1-based checklist numbers that failed) \
         and the `reason` string in the `{topic}.rejected` payload.\n"
    ));
    out.push_str("\n### Scope boundary\n\n");
    out.push_str(
        "This gate is for subjective judgement only. Deterministic checks (schema, payload \
         fields, required events, git evidence, etc.) are handled by other gates. Do not try \
         to enforce those — just answer the checklist above.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-07-09-001 plan (U1): `EventSchema::field_docs` config
    // validation — empty keys are rejected with a stable
    // `EventPolicyValidation` error so the runtime and the
    // operator see the same failure surface.

    /// U1 error path: a `field_docs` entry with an empty key
    /// must be rejected at validation time and the error
    /// message must point at the topic's `field_docs` field
    /// path. Required: keeps the agent / preset author
    /// debugging flow consistent with the existing
    /// `allowed_values` validation pattern.
    #[test]
    fn u1_event_policy_field_docs_rejects_empty_key() {
        let yaml = r#"
event_loop:
  event_policy:
    mode: "enforce"
    enabled: true
    schemas:
      work.done:
        required_fields: ["task_id"]
        field_docs:
          "":
            meaning: "accidental empty key"
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let err = cfg
            .validate()
            .expect_err("empty field_docs key must fail validate()");
        match err {
            ConfigError::EventPolicyValidation { field, message } => {
                assert!(
                    field.contains("work.done.field_docs"),
                    "error field path must mention work.done.field_docs, got: {field}"
                );
                assert!(
                    message.to_lowercase().contains("empty"),
                    "error message must explain the empty-key problem, got: {message}"
                );
            }
            other => panic!("expected EventPolicyValidation, got: {other:?}"),
        }
    }

    /// U1 happy path: a non-empty `field_docs` block parses
    /// and passes validation unchanged. Required: documents
    /// the success path so the negative test above cannot
    /// silently pass because the whole event_policy block
    /// becomes ignored.
    #[test]
    fn u1_event_policy_field_docs_accepts_real_keys() {
        let yaml = r#"
event_loop:
  event_policy:
    mode: "enforce"
    enabled: true
    schemas:
      work.done:
        required_fields: ["task_id"]
        field_docs:
          task_id:
            meaning: "live task id"
            source: "ralph tools task list"
            fill_rule: "do NOT hand-write"
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let warnings = cfg.validate().expect("non-empty field_docs must validate");
        let _ = warnings;
    }

    /// U1 non-goal guard: a `field_docs` key that does not
    /// appear in `required_fields` is allowed. This keeps
    /// `field_docs` as documentation (advisory), not
    /// validation, and matches the `EventSchema`-level
    /// non-goal guard.
    #[test]
    fn u1_event_policy_field_docs_allows_non_required_field() {
        let yaml = r#"
event_loop:
  event_policy:
    mode: "enforce"
    enabled: true
    schemas:
      work.done:
        required_fields: ["task_id"]
        field_docs:
          optional_note:
            meaning: "operator annotation, free text"
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let warnings = cfg
            .validate()
            .expect("non-required field_docs entries must validate");
        let _ = warnings;
    }

    #[test]
    fn test_default_config() {
        let config = RalphConfig::default();
        // Default config has no custom hats (uses default planner+builder)
        assert!(config.hats.is_empty());
        assert_eq!(config.event_loop.max_iterations, 100);
        assert!(!config.verbose);
        assert!(!config.features.preflight.enabled);
        assert!(!config.features.preflight.strict);
        assert!(config.features.preflight.skip.is_empty());
    }

    #[test]
    fn test_parse_yaml_with_custom_hats() {
        let yaml = r#"
event_loop:
  prompt_file: "TASK.md"
  completion_promise: "DONE"
  max_iterations: 50
cli:
  backend: "claude"
hats:
  implementer:
    name: "Implementer"
    triggers: ["task.*", "review.done"]
    publishes: ["impl.done"]
    instructions: "You are the implementation agent."
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        // Custom hats are defined
        assert_eq!(config.hats.len(), 1);
        assert_eq!(config.event_loop.prompt_file, "TASK.md");

        let hat = config.hats.get("implementer").unwrap();
        assert_eq!(hat.triggers.len(), 2);
    }

    #[test]
    fn test_preflight_config_deserialize() {
        let yaml = r#"
features:
  preflight:
    enabled: true
    strict: true
    skip: ["git"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.features.preflight.enabled);
        assert!(config.features.preflight.strict);
        assert_eq!(config.features.preflight.skip, vec!["git".to_string()]);
    }

    #[test]
    fn test_parse_yaml_v1_format() {
        // V1 flat format - identical to Python v1.x config
        let yaml = r#"
agent: gemini
prompt_file: "TASK.md"
completion_promise: "RALPH_DONE"
max_iterations: 75
max_runtime: 7200
max_cost: 10.0
verbose: true
"#;
        let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

        // Before normalization, v2 fields have defaults
        assert_eq!(config.cli.backend, "claude"); // default
        assert_eq!(config.event_loop.max_iterations, 100); // default

        // Normalize v1 -> v2
        config.normalize();

        // After normalization, v2 fields have v1 values
        assert_eq!(config.cli.backend, "gemini");
        assert_eq!(config.event_loop.prompt_file, "TASK.md");
        assert_eq!(config.event_loop.completion_promise, "RALPH_DONE");
        assert_eq!(config.event_loop.max_iterations, 75);
        assert_eq!(config.event_loop.max_runtime_seconds, 7200);
        assert_eq!(config.event_loop.max_cost_usd, Some(10.0));
        assert!(config.verbose);
    }

    #[test]
    fn test_agent_priority() {
        let yaml = r"
agent: auto
agent_priority: [gemini, claude, codex]
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let priority = config.get_agent_priority();
        assert_eq!(priority, vec!["gemini", "claude", "codex"]);
    }

    #[test]
    fn test_default_agent_priority() {
        let config = RalphConfig::default();
        let priority = config.get_agent_priority();
        assert_eq!(priority, vec!["claude", "gemini", "codex"]);
    }

    #[test]
    fn test_validate_deferred_features() {
        let yaml = r"
archive_prompts: true
enable_metrics: true
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let warnings = config.validate().unwrap();

        assert_eq!(warnings.len(), 2);
        assert!(warnings
            .iter()
            .any(|w| matches!(w, ConfigWarning::DeferredFeature { field, .. } if field == "archive_prompts")));
        assert!(warnings
            .iter()
            .any(|w| matches!(w, ConfigWarning::DeferredFeature { field, .. } if field == "enable_metrics")));
    }

    #[test]
    fn test_validate_dropped_fields() {
        let yaml = r#"
max_tokens: 4096
retry_delay: 5
adapters:
  claude:
    tool_permissions: ["read", "write"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let warnings = config.validate().unwrap();

        assert_eq!(warnings.len(), 3);
        assert!(warnings.iter().any(
            |w| matches!(w, ConfigWarning::DroppedField { field, .. } if field == "max_tokens")
        ));
        assert!(warnings.iter().any(
            |w| matches!(w, ConfigWarning::DroppedField { field, .. } if field == "retry_delay")
        ));
        assert!(warnings
            .iter()
            .any(|w| matches!(w, ConfigWarning::DroppedField { field, .. } if field == "adapters.*.tool_permissions")));
    }

    #[test]
    fn test_suppress_warnings() {
        let yaml = r"
_suppress_warnings: true
archive_prompts: true
max_tokens: 4096
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let warnings = config.validate().unwrap();

        // All warnings should be suppressed
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_adapter_settings() {
        let yaml = r"
adapters:
  claude:
    timeout: 600
    enabled: true
  gemini:
    timeout: 300
    enabled: false
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

        let claude = config.adapter_settings("claude");
        assert_eq!(claude.timeout, 600);
        assert!(claude.enabled);

        let gemini = config.adapter_settings("gemini");
        assert_eq!(gemini.timeout, 300);
        assert!(!gemini.enabled);
    }

    // ── Unit 2 (plan 2026-06-06-001) — autonomous watchdog resolution ──

    /// Default behavior: with no `cli.autonomous_idle_timeout_secs` and no
    /// per-adapter override, the watchdog resolves to the adapter default
    /// (300s). This is the R5 / R6 baseline: never fall back to the
    /// interactive `cli.idle_timeout_secs` (30s) and never disable the
    /// watchdog silently.
    #[test]
    fn test_autonomous_idle_timeout_secs_falls_back_to_adapter_default() {
        let config = RalphConfig::default();
        // AdapterSettings::default() uses the 300s `default_timeout()`.
        assert_eq!(config.autonomous_idle_timeout_secs("claude"), 300);
        assert_eq!(config.autonomous_idle_timeout_secs("gemini"), 300);
    }

    /// Per-adapter override: `adapters.<backend>.timeout` is the second-tier
    /// source. Setting `adapters.claude.timeout: 600` makes the claude
    /// watchdog 600s while leaving gemini on 300s.
    #[test]
    fn test_autonomous_idle_timeout_secs_uses_adapter_override() {
        let yaml = r"
adapters:
  claude:
    timeout: 600
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.autonomous_idle_timeout_secs("claude"), 600);
        // Other backends still see their own adapter default.
        assert_eq!(config.autonomous_idle_timeout_secs("gemini"), 300);
    }

    /// Per-Ralph override: `cli.autonomous_idle_timeout_secs` wins over
    /// `adapters.<backend>.timeout` (the documented escape hatch). The
    /// `Some(0)` case must round-trip as `0` and explicitly disable the
    /// watchdog (R8), not be silently re-mapped to the adapter default.
    #[test]
    fn test_autonomous_idle_timeout_secs_cli_override_takes_precedence() {
        let yaml = r"
cli:
  autonomous_idle_timeout_secs: 120
adapters:
  claude:
    timeout: 600
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.autonomous_idle_timeout_secs("claude"),
            120,
            "cli.autonomous_idle_timeout_secs must win over adapters.claude.timeout"
        );

        // Explicit disable (Some(0)) must NOT be re-mapped to the adapter
        // default. R8: `0` means disabled in this field, period.
        let yaml = r"
cli:
  autonomous_idle_timeout_secs: 0
adapters:
  claude:
    timeout: 600
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.autonomous_idle_timeout_secs("claude"),
            0,
            "cli.autonomous_idle_timeout_secs=0 must remain 0 (explicit disable)"
        );
    }

    /// Unknown / non-`AdapterSettings` backend name falls back to the claude
    /// settings (per `adapter_settings()`), which itself defaults to 300s.
    /// This matches the existing `adapter_settings` fallback contract so the
    /// watchdog resolution stays consistent across the codebase.
    #[test]
    fn test_autonomous_idle_timeout_secs_unknown_backend_uses_claude_default() {
        let config = RalphConfig::default();
        assert_eq!(
            config.autonomous_idle_timeout_secs("some-future-backend"),
            300
        );
    }

    #[test]
    fn test_unknown_fields_ignored() {
        // Unknown fields should be silently ignored (forward compatibility)
        let yaml = r#"
agent: claude
unknown_field: "some value"
future_feature: true
"#;
        let result: Result<RalphConfig, _> = serde_yaml::from_str(yaml);
        // Should parse successfully, ignoring unknown fields
        assert!(result.is_ok());
    }

    #[test]
    fn test_custom_backend_args_shorthand() {
        let yaml = r#"
hats:
  opencode_builder:
    name: "Opencode"
    description: "Opencode hat"
    backend: "opencode"
    args: ["-m", "model"]
"#;
        let config = RalphConfig::parse_yaml(yaml).unwrap();
        let hat = config.hats.get("opencode_builder").unwrap();
        assert!(hat.backend_args.is_some());
        assert_eq!(
            hat.backend_args.as_ref().unwrap(),
            &vec!["-m".to_string(), "model".to_string()]
        );
    }

    #[test]
    fn test_custom_backend_args_explicit_key() {
        let yaml = r#"
hats:
  opencode_builder:
    name: "Opencode"
    description: "Opencode hat"
    backend: "opencode"
    backend_args: ["-m", "model"]
"#;
        let config = RalphConfig::parse_yaml(yaml).unwrap();
        let hat = config.hats.get("opencode_builder").unwrap();
        assert!(hat.backend_args.is_some());
        assert_eq!(
            hat.backend_args.as_ref().unwrap(),
            &vec!["-m".to_string(), "model".to_string()]
        );
    }

    #[test]
    fn test_project_key_rejected() {
        let yaml = r#"
project:
  specs_dir: "my_specs"
"#;
        let result = RalphConfig::parse_yaml(yaml);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::DeprecatedProjectKey
        ));
    }

    #[test]
    fn test_ambiguous_routing_rejected() {
        // Per spec: "Every trigger maps to exactly one hat | No ambiguous routing"
        // Note: using semantic events since task.start is reserved
        let yaml = r#"
hats:
  planner:
    name: "Planner"
    description: "Plans tasks"
    triggers: ["planning.start", "build.done"]
  builder:
    name: "Builder"
    description: "Builds code"
    triggers: ["build.task", "build.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::AmbiguousRouting { trigger, .. } if trigger == "build.done"),
            "Expected AmbiguousRouting error for 'build.done', got: {:?}",
            err
        );
    }

    #[test]
    fn test_unique_triggers_accepted() {
        // Valid config: each trigger maps to exactly one hat
        // Note: task.start is reserved for Ralph, so use semantic events
        let yaml = r#"
hats:
  planner:
    name: "Planner"
    description: "Plans tasks"
    triggers: ["planning.start", "build.done", "build.blocked"]
  builder:
    name: "Builder"
    description: "Builds code"
    triggers: ["build.task"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(
            result.is_ok(),
            "Expected valid config, got: {:?}",
            result.unwrap_err()
        );
    }

    /// U1: when ALL hats subscribed to a multi-consumer trigger list
    /// that trigger in their `trigger_multi_consumer_topics`, the strict
    /// 1:1 check is bypassed. This is the documented escape hatch for
    /// design-level multi-consumer topics. (ce-executor-serial removed
    /// its multi-consumer usage on 2026-06-24, but the mechanism remains.)
    #[test]
    fn test_validate_ambiguous_routing_allows_whitelisted_multi_consumer() {
        let yaml = r#"
hats:
  plan-gate:
    name: "Plan Gate"
    description: "Reconcile review verdict and decide queue.advance vs plan.complete"
    triggers: ["review.passed", "fix.exhausted", "debug.exhausted"]
    trigger_multi_consumer_topics: ["fix.exhausted", "debug.exhausted"]
  debug-resolver:
    name: "Debug Resolver"
    description: "Root-cause diagnosis after Fixer safe_auto exhaustion"
    triggers: ["fix.exhausted"]
    trigger_multi_consumer_topics: ["fix.exhausted"]
  shipper:
    name: "Shipper"
    description: "Finalize plan status and emit REVIEW_COMPLETE"
    triggers: ["plan.complete", "plan.blocked", "debug.exhausted"]
    trigger_multi_consumer_topics: ["debug.exhausted"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(
            result.is_ok(),
            "Expected whitelisted multi-consumer config to validate, got: {:?}",
            result.unwrap_err()
        );
    }

    /// U1: a single missing opt-in keeps the strict 1:1 check, so
    /// accidental multi-consumption is impossible. If one of the
    /// `fix.exhausted` consumers forgets to declare
    /// `trigger_multi_consumer_topics`, validate must fail with
    /// `AmbiguousRouting`.
    #[test]
    fn test_validate_ambiguous_routing_rejects_non_whitelisted_multi_consumer() {
        let yaml = r#"
hats:
  plan-gate:
    name: "Plan Gate"
    description: "Reconcile review verdict and decide queue.advance vs plan.complete"
    triggers: ["review.passed", "fix.exhausted"]
    trigger_multi_consumer_topics: ["fix.exhausted"]
  debug-resolver:
    name: "Debug Resolver"
    description: "Root-cause diagnosis after Fixer safe_auto exhaustion"
    triggers: ["fix.exhausted"]
    # NOTE: missing `trigger_multi_consumer_topics` for `fix.exhausted` — must still fail.
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err(), "Expected AmbiguousRouting error");
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::AmbiguousRouting { trigger, .. } if trigger == "fix.exhausted"),
            "Expected AmbiguousRouting for 'fix.exhausted', got: {:?}",
            err
        );
    }

    #[test]
    fn test_reserved_trigger_task_start_rejected() {
        // Per design: task.start is reserved for Ralph (the coordinator)
        let yaml = r#"
hats:
  my_hat:
    name: "My Hat"
    description: "Test hat"
    triggers: ["task.start"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::ReservedTrigger { trigger, hat }
                if trigger == "task.start" && hat == "my_hat"),
            "Expected ReservedTrigger error for 'task.start', got: {:?}",
            err
        );
    }

    #[test]
    fn test_reserved_trigger_task_resume_rejected() {
        // Per design: task.resume is reserved for Ralph (the coordinator)
        let yaml = r#"
hats:
  my_hat:
    name: "My Hat"
    description: "Test hat"
    triggers: ["task.resume", "other.event"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::ReservedTrigger { trigger, hat }
                if trigger == "task.resume" && hat == "my_hat"),
            "Expected ReservedTrigger error for 'task.resume', got: {:?}",
            err
        );
    }

    #[test]
    fn test_missing_description_rejected() {
        // Description is required for all hats
        let yaml = r#"
hats:
  my_hat:
    name: "My Hat"
    triggers: ["build.task"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::MissingDescription { hat } if hat == "my_hat"),
            "Expected MissingDescription error, got: {:?}",
            err
        );
    }

    #[test]
    fn test_empty_description_rejected() {
        // Empty description should also be rejected
        let yaml = r#"
hats:
  my_hat:
    name: "My Hat"
    description: "   "
    triggers: ["build.task"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::MissingDescription { hat } if hat == "my_hat"),
            "Expected MissingDescription error for empty description, got: {:?}",
            err
        );
    }

    #[test]
    fn test_core_config_defaults() {
        let config = RalphConfig::default();
        assert_eq!(config.core.scratchpad, ScratchpadConfig::default());
        assert_eq!(config.core.scratchpad.path, ".ralph/agent/scratchpad.md");
        assert!(config.core.scratchpad.enabled);
        assert_eq!(config.core.specs_dir, ".ralph/specs/");
        // Default guardrails per spec
        assert_eq!(config.core.guardrails.len(), 6);
        assert!(config.core.guardrails[0].contains("Fresh context"));
        assert!(config.core.guardrails[1].contains("search first"));
        assert!(config.core.guardrails[2].contains("Backpressure"));
        assert!(config.core.guardrails[3].contains("strongest available harness"));
        assert!(config.core.guardrails[4].contains("Confidence protocol"));
        assert!(config.core.guardrails[5].contains("Commit atomically"));
    }

    #[test]
    fn test_core_config_customizable() {
        let yaml = r#"
core:
  scratchpad: ".workspace/plan.md"
  specs_dir: "./specifications/"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.core.scratchpad.path, ".workspace/plan.md");
        assert!(config.core.scratchpad.enabled);
        assert_eq!(config.core.specs_dir, "./specifications/");
        // Guardrails should use defaults when not specified
        assert_eq!(config.core.guardrails.len(), 6);
    }

    #[test]
    fn test_core_config_custom_guardrails() {
        let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs/"
  guardrails:
    - "Custom rule one"
    - "Custom rule two"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.core.guardrails.len(), 2);
        assert_eq!(config.core.guardrails[0], "Custom rule one");
        assert_eq!(config.core.guardrails[1], "Custom rule two");
    }

    #[test]
    fn test_prompt_and_prompt_file_mutually_exclusive() {
        // Both prompt and prompt_file specified in config should error
        let yaml = r#"
event_loop:
  prompt: "inline text"
  prompt_file: "custom.md"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::MutuallyExclusive { field1, field2 }
                if field1 == "event_loop.prompt" && field2 == "event_loop.prompt_file"),
            "Expected MutuallyExclusive error, got: {:?}",
            err
        );
    }

    #[test]
    fn test_prompt_with_default_prompt_file_allowed() {
        // Having inline prompt with default prompt_file value should be OK
        let yaml = r#"
event_loop:
  prompt: "inline text"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(
            result.is_ok(),
            "Should allow inline prompt with default prompt_file"
        );
        assert_eq!(config.event_loop.prompt, Some("inline text".to_string()));
        assert_eq!(config.event_loop.prompt_file, "PROMPT.md");
    }

    #[test]
    fn test_custom_backend_requires_command() {
        // Custom backend without command should error
        let yaml = r#"
cli:
  backend: "custom"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::CustomBackendRequiresCommand),
            "Expected CustomBackendRequiresCommand error, got: {:?}",
            err
        );
    }

    #[test]
    fn test_empty_completion_promise_rejected() {
        let yaml = r#"
event_loop:
  completion_promise: "   "
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::InvalidCompletionPromise),
            "Expected InvalidCompletionPromise error, got: {:?}",
            err
        );
    }

    #[test]
    fn test_custom_backend_with_empty_command_errors() {
        // Custom backend with empty command should error
        let yaml = r#"
cli:
  backend: "custom"
  command: ""
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::CustomBackendRequiresCommand),
            "Expected CustomBackendRequiresCommand error, got: {:?}",
            err
        );
    }

    #[test]
    fn test_custom_backend_with_command_succeeds() {
        // Custom backend with valid command should pass validation
        let yaml = r#"
cli:
  backend: "custom"
  command: "my-agent"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(
            result.is_ok(),
            "Should allow custom backend with command: {:?}",
            result.unwrap_err()
        );
    }

    #[test]
    fn test_custom_backend_requires_command_message_actionable() {
        let err = ConfigError::CustomBackendRequiresCommand;
        let msg = err.to_string();
        assert!(msg.contains("cli.command"));
        assert!(msg.contains("ralph init --backend custom"));
        assert!(msg.contains("docs/reference/troubleshooting.md#custom-backend-command"));
    }

    #[test]
    fn test_reserved_trigger_message_actionable() {
        let err = ConfigError::ReservedTrigger {
            trigger: "task.start".to_string(),
            hat: "builder".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Reserved trigger"));
        assert!(msg.contains("docs/reference/troubleshooting.md#reserved-trigger"));
    }

    #[test]
    fn test_prompt_file_with_no_inline_allowed() {
        // Having only prompt_file specified should be OK
        let yaml = r#"
event_loop:
  prompt_file: "custom.md"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(
            result.is_ok(),
            "Should allow prompt_file without inline prompt"
        );
        assert_eq!(config.event_loop.prompt, None);
        assert_eq!(config.event_loop.prompt_file, "custom.md");
    }

    #[test]
    fn test_default_prompt_file_value() {
        let config = RalphConfig::default();
        assert_eq!(config.event_loop.prompt_file, "PROMPT.md");
        assert_eq!(config.event_loop.prompt, None);
    }

    #[test]
    fn test_tui_config_default() {
        let config = RalphConfig::default();
        assert_eq!(config.tui.prefix_key, "ctrl-a");
    }

    #[test]
    fn test_tui_config_parse_ctrl_b() {
        let yaml = r#"
tui:
  prefix_key: "ctrl-b"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let (key_code, key_modifiers) = config.tui.parse_prefix().unwrap();

        use crossterm::event::{KeyCode, KeyModifiers};
        assert_eq!(key_code, KeyCode::Char('b'));
        assert_eq!(key_modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn test_tui_config_parse_invalid_format() {
        let tui_config = TuiConfig {
            prefix_key: "invalid".to_string(),
        };
        let result = tui_config.parse_prefix();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid prefix_key format"));
    }

    #[test]
    fn test_execution_contract_config_disabled_by_default() {
        // U3: execution_contracts is disabled when not configured
        let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(
            config.event_loop.execution_contracts.is_none(),
            "execution_contracts should be None when not configured"
        );
    }

    #[test]
    fn test_execution_contract_config_full_rule() {
        // U3: full work.done contract config parses correctly
        let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
  execution_contracts:
    enabled: true
    rules:
      work.done:
        require_payload_fields: ["plan_name", "plan_path", "task_id", "task_key", "step"]
        require_task:
          id_field: "task_id"
          key_field: "task_key"
          loop_scoped: true
          allowed_terminal_statuses: ["closed"]
          auto_close_on_valid: false
        require_git_change:
          mode: diff_or_commit
          allow_empty_for_steps: ["trivial"]
        require_test_evidence:
          mode: optional
        reject:
          diagnostic_topic: "event.execution_contract.rejected"
          guidance_topic: "human.guidance"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let contracts = config
            .event_loop
            .execution_contracts
            .expect("should have contracts");
        assert!(contracts.enabled, "contracts should be enabled");

        let rule = contracts
            .rules
            .get("work.done")
            .expect("work.done rule should exist");
        assert_eq!(
            rule.require_payload_fields,
            vec!["plan_name", "plan_path", "task_id", "task_key", "step"],
            "payload fields should match"
        );
        assert!(rule.require_task.loop_scoped, "task should be loop-scoped");
        assert_eq!(
            rule.require_task.allowed_terminal_statuses,
            vec!["closed"],
            "allowed terminal statuses should be [closed]"
        );
        assert_eq!(
            rule.require_git_change.mode, "diff_or_commit",
            "git change mode should be diff_or_commit"
        );
    }

    #[test]
    fn test_execution_contract_config_minimal() {
        // U3: minimal execution contract with defaults
        let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
  execution_contracts:
    rules:
      work.done:
        require_payload_fields: ["task_id"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let contracts = config
            .event_loop
            .execution_contracts
            .expect("should have contracts");
        assert!(!contracts.enabled, "contracts should default to disabled");

        let rule = contracts
            .rules
            .get("work.done")
            .expect("work.done rule should exist");
        assert_eq!(rule.require_payload_fields, vec!["task_id"]);
        // Check defaults
        assert_eq!(rule.require_task.id_field, "task_id");
        assert!(rule.require_task.loop_scoped);
        assert_eq!(rule.require_git_change.mode, "diff_or_commit");
        assert_eq!(
            rule.reject.diagnostic_topic,
            "event.execution_contract.rejected"
        );
    }

    #[test]
    fn test_tui_config_parse_invalid_modifier() {
        let tui_config = TuiConfig {
            prefix_key: "alt-a".to_string(),
        };
        let result = tui_config.parse_prefix();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid modifier"));
    }

    #[test]
    fn test_tui_config_parse_invalid_key() {
        let tui_config = TuiConfig {
            prefix_key: "ctrl-abc".to_string(),
        };
        let result = tui_config.parse_prefix();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid key"));
    }

    #[test]
    fn test_hat_backend_named() {
        let yaml = r#""claude""#;
        let backend: HatBackend = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(backend.to_cli_backend(), "claude");
        match backend {
            HatBackend::Named(name) => assert_eq!(name, "claude"),
            _ => panic!("Expected Named variant"),
        }
    }

    #[test]
    fn test_hat_backend_named_with_args() {
        let yaml = r#"
type: "claude"
args: ["--model", "claude-sonnet-4"]
"#;
        let backend: HatBackend = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(backend.to_cli_backend(), "claude");
        match backend {
            HatBackend::NamedWithArgs { backend_type, args } => {
                assert_eq!(backend_type, "claude");
                assert_eq!(args, vec!["--model", "claude-sonnet-4"]);
            }
            _ => panic!("Expected NamedWithArgs variant"),
        }
    }

    #[test]
    fn test_hat_backend_named_with_args_empty() {
        // type: claude without args should still work (NamedWithArgs with empty args)
        let yaml = r#"
type: "gemini"
"#;
        let backend: HatBackend = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(backend.to_cli_backend(), "gemini");
        match backend {
            HatBackend::NamedWithArgs { backend_type, args } => {
                assert_eq!(backend_type, "gemini");
                assert!(args.is_empty());
            }
            _ => panic!("Expected NamedWithArgs variant"),
        }
    }

    #[test]
    fn test_hat_backend_custom() {
        let yaml = r#"
command: "/usr/bin/my-agent"
args: ["--flag", "value"]
"#;
        let backend: HatBackend = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(backend.to_cli_backend(), "custom");
        match backend {
            HatBackend::Custom { command, args } => {
                assert_eq!(command, "/usr/bin/my-agent");
                assert_eq!(args, vec!["--flag", "value"]);
            }
            _ => panic!("Expected Custom variant"),
        }
    }

    #[test]
    fn test_hat_config_with_backend() {
        let yaml = r#"
name: "Custom Builder"
triggers: ["build.task"]
publishes: ["build.done"]
instructions: "Build stuff"
backend: "gemini"
default_publishes: "task.done"
"#;
        let hat: HatConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(hat.name, "Custom Builder");
        assert!(hat.backend.is_some());
        match hat.backend.unwrap() {
            HatBackend::Named(name) => assert_eq!(name, "gemini"),
            _ => panic!("Expected Named backend"),
        }
        assert_eq!(hat.default_publishes, Some("task.done".to_string()));
    }

    #[test]
    fn test_hat_config_without_backend() {
        let yaml = r#"
name: "Default Hat"
triggers: ["task.start"]
publishes: ["task.done"]
instructions: "Do work"
"#;
        let hat: HatConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(hat.name, "Default Hat");
        assert!(hat.backend.is_none());
        assert!(hat.default_publishes.is_none());
    }

    #[test]
    fn test_mixed_backends_config() {
        let yaml = r#"
event_loop:
  prompt_file: "TASK.md"
  max_iterations: 50

cli:
  backend: "claude"

hats:
  planner:
    name: "Planner"
    triggers: ["task.start"]
    publishes: ["build.task"]
    instructions: "Plan the work"
    backend: "claude"
    
  builder:
    name: "Builder"
    triggers: ["build.task"]
    publishes: ["build.done"]
    instructions: "Build the thing"
    backend:
      type: "claude"
      args: ["--model", "haiku"]

  reviewer:
    name: "Reviewer"
    triggers: ["build.done"]
    publishes: ["review.complete"]
    instructions: "Review the work"
    backend:
      command: "/usr/local/bin/custom-agent"
      args: ["--mode", "review"]
    default_publishes: "review.complete"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.hats.len(), 3);

        // Check planner (Named backend)
        let planner = config.hats.get("planner").unwrap();
        assert!(planner.backend.is_some());
        match planner.backend.as_ref().unwrap() {
            HatBackend::Named(name) => assert_eq!(name, "claude"),
            _ => panic!("Expected Named backend for planner"),
        }

        // Check builder (NamedWithArgs backend)
        let builder = config.hats.get("builder").unwrap();
        assert!(builder.backend.is_some());
        match builder.backend.as_ref().unwrap() {
            HatBackend::NamedWithArgs { backend_type, args } => {
                assert_eq!(backend_type, "claude");
                assert_eq!(args, &vec!["--model".to_string(), "haiku".to_string()]);
            }
            _ => panic!("Expected NamedWithArgs backend for builder"),
        }

        // Check reviewer (Custom backend)
        let reviewer = config.hats.get("reviewer").unwrap();
        assert!(reviewer.backend.is_some());
        match reviewer.backend.as_ref().unwrap() {
            HatBackend::Custom { command, args } => {
                assert_eq!(command, "/usr/local/bin/custom-agent");
                assert_eq!(args, &vec!["--mode".to_string(), "review".to_string()]);
            }
            _ => panic!("Expected Custom backend for reviewer"),
        }
        assert_eq!(
            reviewer.default_publishes,
            Some("review.complete".to_string())
        );
    }

    #[test]
    fn test_features_config_auto_merge_defaults_to_false() {
        // Per spec: auto_merge should default to false for safety
        // This prevents automatic merging of parallel loop branches
        let config = RalphConfig::default();
        assert!(
            !config.features.auto_merge,
            "auto_merge should default to false"
        );
    }

    #[test]
    fn test_features_config_auto_merge_from_yaml() {
        // Users can opt into auto_merge via config
        let yaml = r"
features:
  auto_merge: true
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(
            config.features.auto_merge,
            "auto_merge should be true when configured"
        );
    }

    #[test]
    fn test_features_config_auto_merge_false_from_yaml() {
        // Explicit false should work too
        let yaml = r"
features:
  auto_merge: false
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(
            !config.features.auto_merge,
            "auto_merge should be false when explicitly configured"
        );
    }

    #[test]
    fn test_features_config_preserves_parallel_when_adding_auto_merge() {
        // Ensure adding auto_merge doesn't break existing parallel feature
        let yaml = r"
features:
  parallel: false
  auto_merge: true
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.features.parallel, "parallel should be false");
        assert!(config.features.auto_merge, "auto_merge should be true");
    }

    #[test]
    fn test_skills_config_defaults_when_absent() {
        // Configs without a skills: section should still parse (backwards compat)
        let yaml = r"
agent: claude
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.skills.enabled);
        assert!(config.skills.dirs.is_empty());
        assert!(config.skills.overrides.is_empty());
    }

    #[test]
    fn test_skills_config_deserializes_all_fields() {
        let yaml = r#"
skills:
  enabled: true
  dirs:
    - ".claude/skills"
    - "/shared/skills"
  overrides:
    pdd:
      enabled: false
    memories:
      auto_inject: true
      hats: ["ralph"]
      backends: ["claude"]
      tags: ["core"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.skills.enabled);
        assert_eq!(config.skills.dirs.len(), 2);
        assert_eq!(
            config.skills.dirs[0],
            std::path::PathBuf::from(".claude/skills")
        );
        assert_eq!(config.skills.overrides.len(), 2);

        let pdd = config.skills.overrides.get("pdd").unwrap();
        assert_eq!(pdd.enabled, Some(false));

        let memories = config.skills.overrides.get("memories").unwrap();
        assert_eq!(memories.auto_inject, Some(true));
        assert_eq!(memories.hats, vec!["ralph"]);
        assert_eq!(memories.backends, vec!["claude"]);
        assert_eq!(memories.tags, vec!["core"]);
    }

    #[test]
    fn test_skills_config_disabled() {
        let yaml = r"
skills:
  enabled: false
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.skills.enabled);
        assert!(config.skills.dirs.is_empty());
    }

    #[test]
    fn test_skill_override_partial_fields() {
        let yaml = r#"
skills:
  overrides:
    my-skill:
      hats: ["builder", "reviewer"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let override_ = config.skills.overrides.get("my-skill").unwrap();
        assert_eq!(override_.enabled, None);
        assert_eq!(override_.auto_inject, None);
        assert_eq!(override_.hats, vec!["builder", "reviewer"]);
        assert!(override_.backends.is_empty());
        assert!(override_.tags.is_empty());
    }

    #[test]
    fn test_hooks_config_valid_yaml_parses_and_validates() {
        let yaml = r#"
hooks:
  enabled: true
  defaults:
    timeout_seconds: 45
    max_output_bytes: 16384
    suspend_mode: wait_for_resume
  events:
    pre.loop.start:
      - name: env-guard
        command: ["./scripts/hooks/env-guard.sh", "--check"]
        on_error: block
    post.loop.complete:
      - name: notify
        command: ["./scripts/hooks/notify.sh"]
        on_error: warn
        mutate:
          enabled: true
          format: json
"#;
        let config = RalphConfig::parse_yaml(yaml).unwrap();

        assert!(config.hooks.enabled);
        assert_eq!(config.hooks.defaults.timeout_seconds, 45);
        assert_eq!(config.hooks.defaults.max_output_bytes, 16384);
        assert_eq!(config.hooks.events.len(), 2);

        let warnings = config.validate().unwrap();
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_hooks_parse_rejects_invalid_phase_event_key() {
        let yaml = r#"
hooks:
  enabled: true
  events:
    pre.loop.launch:
      - name: bad-phase
        command: ["./scripts/hooks/bad-phase.sh"]
        on_error: warn
"#;

        let result = RalphConfig::parse_yaml(yaml);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::InvalidHookPhaseEvent { phase_event }
            if phase_event == "pre.loop.launch"
        ));
    }

    #[test]
    fn test_hooks_parse_rejects_backpressure_phase_event_keys_in_v1() {
        let yaml = r#"
hooks:
  enabled: true
  events:
    pre.backpressure.triggered:
      - name: unsupported-backpressure
        command: ["./scripts/hooks/backpressure.sh"]
        on_error: warn
"#;

        let result = RalphConfig::parse_yaml(yaml);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::InvalidHookPhaseEvent { phase_event }
            if phase_event == "pre.backpressure.triggered"
        ));

        let message = err.to_string();
        assert!(message.contains("Supported v1 phase-events"));
        assert!(message.contains("pre.plan.created"));
        assert!(message.contains("post.loop.error"));
    }

    #[test]
    fn test_hooks_parse_rejects_invalid_on_error_enum_value() {
        let yaml = r#"
hooks:
  enabled: true
  events:
    pre.loop.start:
      - name: bad-on-error
        command: ["./scripts/hooks/bad-on-error.sh"]
        on_error: explode
"#;

        let result = RalphConfig::parse_yaml(yaml);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(&err, ConfigError::Yaml(_)));

        let message = err.to_string();
        assert!(message.contains("unknown variant `explode`"));
        assert!(message.contains("warn"));
        assert!(message.contains("block"));
        assert!(message.contains("suspend"));
    }

    #[test]
    fn test_hooks_validate_rejects_missing_name() {
        let yaml = r#"
hooks:
  enabled: true
  events:
    pre.loop.start:
      - command: ["./scripts/hooks/no-name.sh"]
        on_error: block
"#;
        let config = RalphConfig::parse_yaml(yaml).unwrap();

        let result = config.validate();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::HookValidation { field, .. }
            if field == "hooks.events.pre.loop.start[0].name"
        ));
    }

    #[test]
    fn test_hooks_validate_rejects_missing_command() {
        let yaml = r"
hooks:
  enabled: true
  events:
    pre.loop.start:
      - name: missing-command
        on_error: block
";
        let config = RalphConfig::parse_yaml(yaml).unwrap();

        let result = config.validate();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::HookValidation { field, .. }
            if field == "hooks.events.pre.loop.start[0].command"
        ));
    }

    #[test]
    fn test_hooks_validate_rejects_missing_on_error() {
        let yaml = r#"
hooks:
  enabled: true
  events:
    pre.loop.start:
      - name: missing-on-error
        command: ["./scripts/hooks/no-on-error.sh"]
"#;
        let config = RalphConfig::parse_yaml(yaml).unwrap();

        let result = config.validate();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::HookValidation { field, .. }
            if field == "hooks.events.pre.loop.start[0].on_error"
        ));
    }

    #[test]
    fn test_hooks_validate_rejects_zero_timeout_seconds() {
        let yaml = r"
hooks:
  enabled: true
  defaults:
    timeout_seconds: 0
";
        let config = RalphConfig::parse_yaml(yaml).unwrap();

        let result = config.validate();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::HookValidation { field, .. }
            if field == "hooks.defaults.timeout_seconds"
        ));
    }

    #[test]
    fn test_hooks_validate_rejects_zero_max_output_bytes() {
        let yaml = r"
hooks:
  enabled: true
  defaults:
    max_output_bytes: 0
";
        let config = RalphConfig::parse_yaml(yaml).unwrap();

        let result = config.validate();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::HookValidation { field, .. }
            if field == "hooks.defaults.max_output_bytes"
        ));
    }

    #[test]
    fn test_hooks_validate_rejects_parallel_non_v1_field() {
        let yaml = r"
hooks:
  enabled: true
  parallel: true
";
        let config = RalphConfig::parse_yaml(yaml).unwrap();

        let result = config.validate();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::UnsupportedHookField { field, .. }
            if field == "hooks.parallel"
        ));
    }

    #[test]
    fn test_hooks_validate_rejects_global_scope_non_v1_field() {
        let yaml = r#"
hooks:
  enabled: true
  events:
    pre.loop.start:
      - name: global-scope
        command: ["./scripts/hooks/global.sh"]
        on_error: warn
        scope: global
"#;
        let config = RalphConfig::parse_yaml(yaml).unwrap();

        let result = config.validate();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::UnsupportedHookField { field, .. }
            if field == "hooks.events.pre.loop.start[0].scope"
        ));
    }

    #[test]
    fn test_extra_instructions_merged_during_normalize() {
        let yaml = r#"
_fragments:
  shared_protocol: &shared_protocol |
    ### Shared Protocol
    Follow this protocol.

hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    instructions: |
      ## BUILDER MODE
      Build things.
    extra_instructions:
      - *shared_protocol
"#;
        let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let hat = config.hats.get("builder").unwrap();

        // Before normalize: extra_instructions has content, instructions does not include it
        assert_eq!(hat.extra_instructions.len(), 1);
        assert!(!hat.instructions.contains("Shared Protocol"));

        config.normalize();

        let hat = config.hats.get("builder").unwrap();
        // After normalize: extra_instructions drained, instructions includes the fragment
        assert!(hat.extra_instructions.is_empty());
        assert!(hat.instructions.contains("## BUILDER MODE"));
        assert!(hat.instructions.contains("### Shared Protocol"));
        assert!(hat.instructions.contains("Follow this protocol."));
    }

    #[test]
    fn test_extra_instructions_empty_by_default() {
        let yaml = r#"
hats:
  simple:
    name: "Simple"
    triggers: ["start"]
    instructions: "Do the thing."
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let hat = config.hats.get("simple").unwrap();
        assert!(hat.extra_instructions.is_empty());
    }

    // ─────────────────────────────────────────────────────────────────────────
    // TELEMETRY / RUNTIME-DIAGNOSIS CONFIG TESTS (U1)
    // ─────────────────────────────────────────────────────────────────────────

    /// AC: omitting `telemetry:` from `ralph.yml` yields the documented
    /// no-op defaults. This is the non-regression contract for existing
    /// preset files.
    #[test]
    fn test_telemetry_section_absent_uses_defaults() {
        let yaml = r"
agent: claude
event_loop:
  completion_promise: DONE
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.telemetry,
            super::telemetry::TelemetryConfig::default()
        );
        assert!(!config.telemetry.runtime_diagnosis.enabled);
        assert!(!config.telemetry.runtime_diagnosis.write_artifacts);
    }

    /// AC: `telemetry.runtime_diagnosis.drift.emit_cadence_sigma = -1.0`
    /// must be rejected by `RalphConfig::validate`.
    #[test]
    fn test_telemetry_validate_rejects_negative_emit_cadence_sigma() {
        let yaml = r"
telemetry:
  runtime_diagnosis:
    drift:
      emit_cadence_sigma: -1.0
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(
            result.is_err(),
            "negative emit_cadence_sigma must fail validate"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::TelemetryValidation { field, .. } if field.contains("emit_cadence_sigma")),
            "expected TelemetryValidation for emit_cadence_sigma, got {err:?}"
        );
    }

    /// AC: a default `telemetry` block must validate cleanly with zero
    /// errors. Soft warnings (e.g. `enabled=false && write_artifacts=true`)
    /// are surfaced separately and only when the caller opted in.
    #[test]
    fn test_telemetry_validate_default_is_clean() {
        let config = RalphConfig::default();
        let warnings = config.validate().expect("default telemetry must validate");
        // We only assert that no telemetry-derived warnings were emitted.
        // Other sections (e.g. deferred features) may also be quiet.
        assert!(
            !warnings.iter().any(|w| matches!(w,
                ConfigWarning::InvalidValue { field, .. }
                if field.starts_with("telemetry.")
            )),
            "default telemetry must not emit warnings, got {warnings:?}"
        );
    }

    /// AC: `telemetry.runtime_diagnosis.enabled: false` together with
    /// `write_artifacts: true` must surface a soft `ConfigWarning` from
    /// `validate` (not an error).
    #[test]
    fn test_telemetry_validate_warns_on_disabled_with_write_artifacts() {
        let yaml = r"
telemetry:
  runtime_diagnosis:
    enabled: false
    write_artifacts: true
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let warnings = config.validate().expect("soft warning must not be Err");
        assert!(
            warnings.iter().any(|w| matches!(w,
                ConfigWarning::InvalidValue { field, .. }
                if field == "telemetry.runtime_diagnosis.write_artifacts"
            )),
            "expected soft warning for write_artifacts, got {warnings:?}"
        );
    }

    /// AC: `RalphConfig::diagnostics_options` is a thin bridge to
    /// `TelemetryConfig::to_diagnostics_options`. We test the *config*
    /// contract (the inner, env-free path) by reading whatever the
    /// current `RALPH_DIAGNOSTICS` value is and asserting the bridge
    /// matches `to_diagnostics_options_with_full` with that value.
    /// `forbid(unsafe_code)` prevents mutating the env directly.
    #[test]
    fn test_diagnostics_options_default_workspace_is_noop() {
        let config = RalphConfig::default();
        let opts = config.diagnostics_options(std::path::Path::new("."));
        let env_value = std::env::var("RALPH_DIAGNOSTICS")
            .map(|v| v == "1")
            .unwrap_or(false);
        let inner_opts = config
            .telemetry
            .to_diagnostics_options_with_full(std::path::Path::new("."), env_value);
        assert_eq!(opts, inner_opts);

        // `session_dir` is always None at the U1 boundary.
        assert!(opts.session_dir.is_none());
    }

    // === Per-Hat Scratchpad Configuration Tests ===

    /// AC1: Legacy plain-string config
    #[test]
    fn test_scratchpad_legacy_plain_string() {
        let yaml = r#"
core:
  scratchpad: ".workspace/plan.md"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.core.scratchpad,
            ScratchpadConfig {
                enabled: true,
                path: ".workspace/plan.md".to_string()
            }
        );
    }

    /// AC2: Structured config with enabled/path
    #[test]
    fn test_scratchpad_structured_config() {
        let yaml = r#"
core:
  scratchpad:
    enabled: true
    path: ".ralph/agent/scratchpad.md"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.core.scratchpad,
            ScratchpadConfig {
                enabled: true,
                path: ".ralph/agent/scratchpad.md".to_string()
            }
        );
    }

    /// AC2 variant: Structured config with enabled: false
    #[test]
    fn test_scratchpad_structured_disabled() {
        let yaml = r"
core:
  scratchpad:
    enabled: false
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.core.scratchpad.enabled);
        assert_eq!(config.core.scratchpad.path, ".ralph/agent/scratchpad.md");
    }

    /// AC5/AC7: Default config unchanged
    #[test]
    fn test_scratchpad_default_config() {
        let config = RalphConfig::default();
        assert_eq!(
            config.core.scratchpad,
            ScratchpadConfig {
                enabled: true,
                path: ".ralph/agent/scratchpad.md".to_string()
            }
        );
    }

    /// AC8: Hat with plain-string scratchpad shorthand
    #[test]
    fn test_hat_scratchpad_plain_string() {
        let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["plan.start"]
    scratchpad: ".ralph/agent/planner.md"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let hat = config.hats.get("planner").unwrap();
        assert_eq!(
            hat.scratchpad,
            Some(ScratchpadConfig {
                enabled: true,
                path: ".ralph/agent/planner.md".to_string()
            })
        );
    }

    /// AC3 (config part): Hat disables scratchpad
    #[test]
    fn test_hat_scratchpad_disabled() {
        let yaml = r#"
hats:
  validator:
    name: "Validator"
    triggers: ["validate.start"]
    scratchpad:
      enabled: false
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let hat = config.hats.get("validator").unwrap();
        assert_eq!(
            hat.scratchpad,
            Some(ScratchpadConfig {
                enabled: false,
                path: ".ralph/agent/scratchpad.md".to_string()
            })
        );
    }

    /// AC4 (config part): Hat with custom path
    #[test]
    fn test_hat_scratchpad_custom_path() {
        let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    scratchpad:
      path: ".ralph/agent/builder.md"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let hat = config.hats.get("builder").unwrap();
        assert_eq!(
            hat.scratchpad,
            Some(ScratchpadConfig {
                enabled: true,
                path: ".ralph/agent/builder.md".to_string()
            })
        );
    }

    /// AC5 (config part): Hat inherits global (no scratchpad key)
    #[test]
    fn test_hat_scratchpad_inherits_global() {
        let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.start"]
    instructions: "Review the code."
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let hat = config.hats.get("reviewer").unwrap();
        assert!(
            hat.scratchpad.is_none(),
            "No scratchpad key means None (inherit global)"
        );
    }

    /// Resolution function test
    #[test]
    fn test_scratchpad_resolve_hat_override() {
        let global = ScratchpadConfig {
            enabled: true,
            path: ".ralph/agent/scratchpad.md".to_string(),
        };
        let hat_override = ScratchpadConfig {
            enabled: true,
            path: ".ralph/agent/planner.md".to_string(),
        };
        let resolved = ScratchpadConfig::resolve(Some(&hat_override), &global);
        assert_eq!(resolved, hat_override);
    }

    #[test]
    fn test_scratchpad_resolve_global_fallback() {
        let global = ScratchpadConfig {
            enabled: true,
            path: ".ralph/agent/scratchpad.md".to_string(),
        };
        let resolved = ScratchpadConfig::resolve(None, &global);
        assert_eq!(resolved, global);
    }

    /// AC9 (config part): Multiple hats with different configs
    #[test]
    fn test_multiple_hats_different_scratchpad_configs() {
        let yaml = r#"
core:
  scratchpad:
    enabled: true
    path: ".ralph/agent/scratchpad.md"
hats:
  planner:
    name: "Planner"
    triggers: ["plan.start"]
    scratchpad:
      path: ".ralph/agent/planner.md"
  builder:
    name: "Builder"
    triggers: ["build.start"]
  validator:
    name: "Validator"
    triggers: ["validate.start"]
    scratchpad:
      enabled: false
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

        // Planner has custom path
        let planner = config.hats.get("planner").unwrap();
        let planner_resolved =
            ScratchpadConfig::resolve(planner.scratchpad.as_ref(), &config.core.scratchpad);
        assert_eq!(planner_resolved.path, ".ralph/agent/planner.md");
        assert!(planner_resolved.enabled);

        // Builder inherits global
        let builder = config.hats.get("builder").unwrap();
        let builder_resolved =
            ScratchpadConfig::resolve(builder.scratchpad.as_ref(), &config.core.scratchpad);
        assert_eq!(builder_resolved.path, ".ralph/agent/scratchpad.md");
        assert!(builder_resolved.enabled);

        // Validator is disabled
        let validator = config.hats.get("validator").unwrap();
        let validator_resolved =
            ScratchpadConfig::resolve(validator.scratchpad.as_ref(), &config.core.scratchpad);
        assert!(!validator_resolved.enabled);
    }

    /// Edge case: core.scratchpad missing entirely
    #[test]
    fn test_scratchpad_missing_defaults() {
        let yaml = r#"
core:
  specs_dir: "./specs/"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.core.scratchpad, ScratchpadConfig::default());
    }

    /// Edge case: hat scratchpad with enabled but no path
    #[test]
    fn test_hat_scratchpad_enabled_no_path() {
        let yaml = r#"
hats:
  worker:
    name: "Worker"
    triggers: ["work.start"]
    scratchpad:
      enabled: true
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let hat = config.hats.get("worker").unwrap();
        let sc = hat.scratchpad.as_ref().unwrap();
        assert!(sc.enabled);
        assert_eq!(sc.path, ".ralph/agent/scratchpad.md");
    }

    /// Edge case: hat scratchpad with path but no enabled
    #[test]
    fn test_hat_scratchpad_path_no_enabled() {
        let yaml = r#"
hats:
  worker:
    name: "Worker"
    triggers: ["work.start"]
    scratchpad:
      path: ".ralph/agent/worker.md"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let hat = config.hats.get("worker").unwrap();
        let sc = hat.scratchpad.as_ref().unwrap();
        assert!(sc.enabled);
        assert_eq!(sc.path, ".ralph/agent/worker.md");
    }

    // ── Wave config tests (Step 2: HatConfig extensions) ──

    #[test]
    fn test_wave_config_concurrency_and_aggregate_parse() {
        let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    description: "Reviews files in parallel"
    triggers: ["review.file"]
    publishes: ["review.done"]
    instructions: "Review the file."
    concurrency: 3
  aggregator:
    name: "Aggregator"
    description: "Aggregates review results"
    triggers: ["review.done"]
    publishes: ["review.complete"]
    instructions: "Aggregate results."
    aggregate:
      mode: wait_for_all
      timeout: 600
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

        let reviewer = config.hats.get("reviewer").unwrap();
        assert_eq!(reviewer.concurrency, 3);
        assert!(reviewer.aggregate.is_none());

        let aggregator = config.hats.get("aggregator").unwrap();
        assert_eq!(aggregator.concurrency, 1); // default
        let agg = aggregator.aggregate.as_ref().unwrap();
        assert!(matches!(agg.mode, hat::AggregateMode::WaitForAll));
        assert_eq!(agg.timeout, 600);
    }

    #[test]
    fn test_wave_config_defaults_without_new_fields() {
        // Existing YAML without concurrency/aggregate should parse with defaults
        let yaml = r#"
hats:
  builder:
    name: "Builder"
    description: "Builds code"
    triggers: ["build.task"]
    publishes: ["build.done"]
    instructions: "Build stuff."
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let hat = config.hats.get("builder").unwrap();
        assert_eq!(hat.concurrency, 1);
        assert!(hat.aggregate.is_none());
    }

    #[test]
    fn test_wave_config_concurrency_zero_rejected() {
        let yaml = r#"
hats:
  worker:
    name: "Worker"
    description: "Parallel worker"
    triggers: ["work.item"]
    publishes: ["work.done"]
    instructions: "Do work."
    concurrency: 0
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::InvalidConcurrency { hat, .. } if hat == "worker"),
            "Expected InvalidConcurrency error, got: {:?}",
            err
        );
    }

    #[test]
    fn test_wave_config_aggregate_on_concurrent_hat_rejected() {
        // A hat cannot be both concurrent (concurrency > 1) and an aggregator
        let yaml = r#"
hats:
  hybrid:
    name: "Hybrid"
    description: "Invalid: both concurrent and aggregator"
    triggers: ["work.item"]
    publishes: ["work.done"]
    instructions: "Invalid config."
    concurrency: 3
    aggregate:
      mode: wait_for_all
      timeout: 300
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::AggregateOnConcurrentHat { hat, .. } if hat == "hybrid"),
            "Expected AggregateOnConcurrentHat error, got: {:?}",
            err
        );
    }

    #[test]
    fn test_wave_config_aggregate_on_non_concurrent_hat_valid() {
        // Aggregate on a hat with concurrency=1 (default) is valid
        let yaml = r#"
hats:
  aggregator:
    name: "Aggregator"
    description: "Collects results"
    triggers: ["work.done"]
    publishes: ["work.complete"]
    instructions: "Aggregate."
    aggregate:
      mode: wait_for_all
      timeout: 300
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();

        assert!(
            result.is_ok(),
            "Aggregate on non-concurrent hat should be valid: {:?}",
            result.unwrap_err()
        );
    }

    // ── Workflow Guard Configuration Tests (Unit 1) ──

    #[test]
    fn test_workflow_guards_absent_parses_as_disabled() {
        // YAML without workflow_guards section should parse with guard disabled
        let yaml = r"
event_loop:
  max_iterations: 50
cli:
  backend: claude
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(
            config.event_loop.workflow_guards.is_none(),
            "workflow_guards should be None when absent"
        );
        assert_eq!(config.event_loop.max_iterations, 50);
    }

    #[test]
    fn test_workflow_guards_with_single_chain_parses_correctly() {
        let yaml = r"
event_loop:
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let guards = config.event_loop.workflow_guards.as_ref().unwrap();
        assert_eq!(guards.chains.len(), 1);

        let chain = &guards.chains[0];
        assert_eq!(chain.name, "experiment");
        assert_eq!(chain.topics.len(), 5);
        assert_eq!(chain.topics[0], "experiment.planned");
        assert_eq!(chain.topics[4], "experiment.evaluated");
        assert!(matches!(chain.mode, WorkflowChainMode::Strict));
    }

    #[test]
    fn test_workflow_guards_empty_chain_list_accepted() {
        // Empty chain list should be accepted as disabled
        let yaml = r"
event_loop:
  workflow_guards:
    chains: []
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let guards = config.event_loop.workflow_guards.as_ref().unwrap();
        assert!(guards.chains.is_empty());
    }

    #[test]
    fn test_workflow_guards_chain_with_correlation_parses() {
        let yaml = r"
event_loop:
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.scored
        mode: strict
        correlation:
          from_payload: experiment_id
          from_topic: experiment.planned
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let chain = &config.event_loop.workflow_guards.as_ref().unwrap().chains[0];
        let corr = chain.correlation.as_ref().unwrap();
        assert_eq!(corr.from_payload, "experiment_id");
        assert_eq!(corr.from_topic, Some("experiment.planned".to_string()));
    }

    #[test]
    fn test_workflow_guards_chain_mode_advisory() {
        let yaml = r"
event_loop:
  workflow_guards:
    chains:
      - name: build
        topics:
          - build.started
          - build.completed
        mode: advisory
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let chain = &config.event_loop.workflow_guards.as_ref().unwrap().chains[0];
        assert!(matches!(chain.mode, WorkflowChainMode::Advisory));
    }

    #[test]
    fn test_workflow_guards_multiple_chains() {
        let yaml = r"
event_loop:
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.evaluated
      - name: build
        topics:
          - build.started
          - build.completed
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let guards = config.event_loop.workflow_guards.as_ref().unwrap();
        assert_eq!(guards.chains.len(), 2);
        assert_eq!(guards.chains[0].name, "experiment");
        assert_eq!(guards.chains[1].name, "build");
    }

    #[test]
    fn test_workflow_guards_validation_rejects_duplicate_topics() {
        let yaml = r"
event_loop:
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.planned
          - experiment.evaluated
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Duplicate topic"),
            "Expected duplicate topic error, got: {}",
            err
        );
    }

    #[test]
    fn test_workflow_guards_validation_rejects_empty_topics() {
        let yaml = r"
event_loop:
  workflow_guards:
    chains:
      - name: experiment
        topics: []
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("topics cannot be empty"),
            "Expected empty topics error, got: {}",
            err
        );
    }

    #[test]
    fn test_workflow_guards_validation_rejects_empty_chain_name() {
        let yaml = r#"
event_loop:
  workflow_guards:
    chains:
      - name: ""
        topics:
          - experiment.planned
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Chain name cannot be empty"),
            "Expected empty name error, got: {}",
            err
        );
    }

    #[test]
    fn test_workflow_guards_validation_rejects_duplicate_chain_name() {
        let yaml = r"
event_loop:
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
      - name: experiment
        topics:
          - build.started
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Duplicate workflow chain name"),
            "Expected duplicate chain name error, got: {}",
            err
        );
    }

    #[test]
    fn test_state_machine_validation_rejects_duplicate_transition_topic() {
        let yaml = r"
event_loop:
  state_machine:
    enabled: true
    transitions:
      - topic: experiment.planned
        from: [idle]
        to: planned
      - topic: experiment.planned
        from: [planned]
        to: planned_again
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Duplicate transition topic"),
            "Expected duplicate transition topic error, got: {}",
            err
        );
    }

    #[test]
    fn test_state_machine_validation_rejects_empty_from_state() {
        let yaml = r"
event_loop:
  state_machine:
    enabled: true
    transitions:
      - topic: experiment.planned
        from: []
        to: planned
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("from states cannot be empty"),
            "Expected empty from state error, got: {}",
            err
        );
    }

    #[test]
    fn test_state_machine_validation_rejects_open_and_close_transition() {
        let yaml = r"
event_loop:
  state_machine:
    enabled: true
    transitions:
      - topic: experiment.planned
        from: [idle]
        to: planned
        opens_instance: true
        closes_instance: true
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cannot both open and close"),
            "Expected open/close conflict error, got: {}",
            err
        );
    }

    // ── EventPolicyConfig tests ──

    #[test]
    fn test_event_policy_absent_parses_as_none() {
        let yaml = r"
event_loop:
  max_iterations: 50
cli:
  backend: claude
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(
            config.event_loop.event_policy.is_none(),
            "event_policy should be None when absent"
        );
        assert_eq!(config.event_loop.max_iterations, 50);
    }

    #[test]
    fn test_event_policy_observe_mode_parses() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    on_violation: warn
    schemas:
      experiment.planned:
        payload: json_object
        required_fields:
          - task_key
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let policy = config.event_loop.event_policy.as_ref().unwrap();
        assert!(policy.enabled);
        assert!(matches!(policy.mode, EventPolicyMode::Observe));
        assert!(matches!(policy.on_violation, ViolationAction::Warn));
        assert_eq!(policy.schemas.len(), 1);
        assert_eq!(policy.terminal_topics, vec!["LOOP_COMPLETE"]);
        assert_eq!(policy.business_topics, vec!["experiment.planned"]);

        let schema = policy.schemas.get("experiment.planned").unwrap();
        assert!(matches!(schema.payload, Some(PayloadType::JsonObject)));
        assert_eq!(schema.required_fields, vec!["task_key"]);
    }

    #[test]
    fn test_event_policy_enforce_mode_parses() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let policy = config.event_loop.event_policy.as_ref().unwrap();
        assert!(policy.enabled);
        assert!(matches!(policy.mode, EventPolicyMode::Enforce));
        assert!(matches!(
            policy.on_violation,
            ViolationAction::RejectWithResume
        ));
    }

    #[test]
    fn test_event_policy_invalid_mode_fails_parsing() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: invalid_mode
";
        let result: Result<RalphConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_err(),
            "Invalid event_policy mode must fail parsing"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown variant `invalid_mode`"),
            "Error should mention unknown variant, got: {}",
            err
        );
    }

    #[test]
    fn test_event_policy_empty_schema_topic_rejected() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      '':
        payload: json_object
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Schema topic cannot be empty"),
            "Expected empty schema topic error, got: {}",
            err
        );
    }

    #[test]
    fn test_event_policy_invalid_field_path_rejected() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      experiment.planned:
        payload: json_object
        allowed_values:
          data..field:
            - keep
            - discard
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("consecutive dots"),
            "Expected consecutive dots error, got: {}",
            err
        );
    }

    // ── HatExecutionMode config tests ──

    #[test]
    fn test_hat_execution_mode_defaults_to_coordinator() {
        let config = RalphConfig::default();
        assert_eq!(
            config.event_loop.execution_mode,
            HatExecutionMode::Coordinator,
            "Default execution_mode must be Coordinator"
        );
    }

    #[test]
    fn test_hat_execution_mode_explicit_isolated() {
        let yaml = r"
event_loop:
  execution_mode: isolated
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.event_loop.execution_mode, HatExecutionMode::Isolated);
    }

    #[test]
    fn test_hat_execution_mode_explicit_coordinator() {
        let yaml = r"
event_loop:
  execution_mode: coordinator
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.event_loop.execution_mode,
            HatExecutionMode::Coordinator
        );
    }

    #[test]
    fn test_hat_execution_mode_missing_field_defaults_to_coordinator() {
        // Existing configs without execution_mode should parse successfully
        let yaml = r"
event_loop:
  max_iterations: 50
cli:
  backend: claude
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.event_loop.execution_mode,
            HatExecutionMode::Coordinator,
            "Missing execution_mode must default to Coordinator"
        );
    }

    #[test]
    fn test_hat_execution_mode_invalid_value_fails_parsing() {
        let yaml = r"
event_loop:
  execution_mode: sandbox
";
        let result: Result<RalphConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_err(),
            "Invalid execution_mode value must fail parsing"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown variant `sandbox`"),
            "Error should mention unknown variant, got: {}",
            err
        );
    }

    #[test]
    fn test_hat_execution_mode_case_sensitive_rejected() {
        // Pascal-case 'Isolated' should be rejected (serde rename_all = snake_case)
        let yaml = r"
event_loop:
  execution_mode: Isolated
";
        let result: Result<RalphConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_err(),
            "Case-sensitive mode value 'Isolated' must be rejected"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown variant `Isolated`"),
            "Error should mention unknown variant `Isolated`, got: {}",
            err
        );
    }

    #[test]
    fn test_hat_execution_mode_uppercase_rejected() {
        // ALL CAPS 'ISOLATED' should be rejected
        let yaml = r"
event_loop:
  execution_mode: ISOLATED
";
        let result: Result<RalphConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_err(),
            "Uppercase mode value 'ISOLATED' must be rejected"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown variant `ISOLATED`"),
            "Error should mention unknown variant `ISOLATED`, got: {}",
            err
        );
    }

    #[test]
    fn test_hat_execution_mode_empty_string_fails() {
        let yaml = r#"
event_loop:
  execution_mode: ""
"#;
        let result: Result<RalphConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_err(),
            "Empty string execution_mode must fail parsing"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown variant `"),
            "Error should mention unknown variant for empty string, got: {}",
            err
        );
    }

    // ── EventPolicyConfig new fields tests ──

    #[test]
    fn test_event_policy_old_config_without_new_fields_parses() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      experiment.planned:
        payload: json_object
        required_fields:
          - task_key
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let policy = config.event_loop.event_policy.as_ref().unwrap();
        assert!(!policy.require_policy_check_for_cli_emit);
        assert!(policy.allow_unsafe_cli_emit);
        assert!(!policy.require_emit_provenance);
        assert_eq!(
            policy.completion_after_terminal.duplicate_terminal,
            CompletionAfterTerminalAction::Warn
        );
        assert_eq!(
            policy.completion_after_terminal.business_after_completion,
            CompletionAfterTerminalAction::Warn
        );
        assert!(!policy.completion_after_terminal.write_diagnostic_event);
    }

    #[test]
    fn test_event_policy_strict_config_parses() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: false
    require_emit_provenance: true
    completion_after_terminal:
      duplicate_terminal: reject
      business_after_completion: ignore
      write_diagnostic_event: true
    schemas:
      experiment.planned:
        payload: json_object
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let policy = config.event_loop.event_policy.as_ref().unwrap();
        assert!(policy.require_policy_check_for_cli_emit);
        assert!(!policy.allow_unsafe_cli_emit);
        assert!(policy.require_emit_provenance);
        assert_eq!(
            policy.completion_after_terminal.duplicate_terminal,
            CompletionAfterTerminalAction::Reject
        );
        assert_eq!(
            policy.completion_after_terminal.business_after_completion,
            CompletionAfterTerminalAction::Ignore
        );
        assert!(policy.completion_after_terminal.write_diagnostic_event);
    }

    #[test]
    fn test_event_policy_invalid_completion_action_fails_parsing() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    completion_after_terminal:
      duplicate_terminal: invalid_action
";
        let result: Result<RalphConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_err(),
            "Invalid completion_after_terminal action must fail parsing"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown variant `invalid_action`"),
            "Error should mention unknown variant, got: {}",
            err
        );
    }

    // ── Schema file tests ──

    #[test]
    fn test_schema_file_field_parses() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    schema_file: schemas.yml
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let policy = config.event_loop.event_policy.as_ref().unwrap();
        assert_eq!(policy.schema_file.as_deref(), Some("schemas.yml"));
    }

    #[test]
    fn test_schema_file_field_absent_is_none() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: observe
";
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let policy = config.event_loop.event_policy.as_ref().unwrap();
        assert!(policy.schema_file.is_none());
    }

    #[test]
    fn test_resolve_schema_files_no_schema_file() {
        let mut config = RalphConfig::default();
        config.event_loop.event_policy = Some(EventPolicyConfig::default());
        // Should succeed when no schema_file is set
        let result = config.resolve_schema_files(std::path::Path::new("/tmp"));
        assert!(result.is_ok());
        // Schemas should remain empty
        assert!(
            config
                .event_loop
                .event_policy
                .as_ref()
                .unwrap()
                .schemas
                .is_empty()
        );
    }

    #[test]
    fn test_resolve_schema_files_file_not_found() {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    schema_file: nonexistent.yml
";
        let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.resolve_schema_files(std::path::Path::new("/tmp"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Schema file not found"));
        assert!(err.contains("nonexistent.yml"));
    }

    #[test]
    fn test_resolve_schema_files_invalid_yaml() {
        let temp_dir = std::env::temp_dir();
        let schema_path = temp_dir.join("invalid_schema.yml");
        std::fs::write(&schema_path, "not: [valid: yaml: broken").unwrap();

        let yaml = format!(
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    schema_file: "{}"
"#,
            schema_path.file_name().unwrap().to_string_lossy()
        );

        let mut config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let result = config.resolve_schema_files(&temp_dir);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Schema file parse error"));

        std::fs::remove_file(&schema_path).ok();
    }

    #[test]
    fn test_resolve_schema_files_root_not_map() {
        let temp_dir = std::env::temp_dir();
        let schema_path = temp_dir.join("array_schema.yml");
        // YAML with array at root instead of map
        std::fs::write(&schema_path, "- topic1\n  - topic2").unwrap();

        let yaml = format!(
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    schema_file: "{}"
"#,
            schema_path.file_name().unwrap().to_string_lossy()
        );

        let mut config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let result = config.resolve_schema_files(&temp_dir);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("must be a map"));

        std::fs::remove_file(&schema_path).ok();
    }

    #[test]
    fn test_resolve_schema_files_invalid_schema_for_topic() {
        let temp_dir = std::env::temp_dir();
        let schema_path = temp_dir.join("bad_topic_schema.yml");
        // Valid YAML but invalid schema structure for the topic
        std::fs::write(
            &schema_path,
            r"
experiment.planned:
  payload: json_object
  required_fields: not_an_array
",
        )
        .unwrap();

        let yaml = format!(
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    schema_file: "{}"
"#,
            schema_path.file_name().unwrap().to_string_lossy()
        );

        let mut config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let result = config.resolve_schema_files(&temp_dir);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid schema for topic"));

        std::fs::remove_file(&schema_path).ok();
    }

    #[test]
    fn test_resolve_schema_files_loads_and_merges() {
        let temp_dir = std::env::temp_dir();
        let schema_path = temp_dir.join("merged_schema.yml");
        std::fs::write(
            &schema_path,
            r"
experiment.planned:
  payload: json_object
  required_fields:
    - task_key
    - plan_name
work.done:
  payload: json_object
  required_fields:
    - task_id
",
        )
        .unwrap();

        let yaml = format!(
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    schema_file: "{}"
    schemas:
      experiment.planned:
        payload: json_object
        required_fields:
          - task_key
          - dimension
"#,
            schema_path.file_name().unwrap().to_string_lossy()
        );

        let mut config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let result = config.resolve_schema_files(&temp_dir);
        assert!(result.is_ok());

        let schemas = &config.event_loop.event_policy.as_ref().unwrap().schemas;
        // experiment.planned: inline schema takes priority, replaces file schema
        let exp_planned = schemas.get("experiment.planned").unwrap();
        assert!(
            exp_planned
                .required_fields
                .contains(&"task_key".to_string())
        );
        assert!(
            exp_planned
                .required_fields
                .contains(&"dimension".to_string())
        );
        // plan_name from file is NOT present because inline replaces entire topic schema
        assert!(
            !exp_planned
                .required_fields
                .contains(&"plan_name".to_string())
        );
        // work.done should be loaded from file only (not in inline)
        let work_done = schemas.get("work.done").unwrap();
        assert!(work_done.required_fields.contains(&"task_id".to_string()));

        std::fs::remove_file(&schema_path).ok();
    }

    #[test]
    fn test_resolve_schema_files_inline_takes_priority() {
        let temp_dir = std::env::temp_dir();
        let schema_path = temp_dir.join("priority_schema.yml");
        std::fs::write(
            &schema_path,
            r"
work.ready:
  payload: json_object
  required_fields:
    - file_source
",
        )
        .unwrap();

        let yaml = format!(
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    schema_file: "{}"
    schemas:
      work.ready:
        payload: json_object
        required_fields:
          - task_key
"#,
            schema_path.file_name().unwrap().to_string_lossy()
        );

        let mut config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let result = config.resolve_schema_files(&temp_dir);
        assert!(result.is_ok());

        // Inline schema should completely override file schema for same topic
        let schemas = &config.event_loop.event_policy.as_ref().unwrap().schemas;
        let work_ready = schemas.get("work.ready").unwrap();
        assert_eq!(work_ready.required_fields, vec!["task_key"]);
        // file_source from file should NOT be present since inline overrides
        assert!(
            !work_ready
                .required_fields
                .contains(&"file_source".to_string())
        );

        std::fs::remove_file(&schema_path).ok();
    }

    #[test]
    fn test_resolve_schema_files_absolute_path() {
        let temp_dir = std::env::temp_dir();
        let schema_path = temp_dir.join("absolute_schema.yml");
        std::fs::write(
            &schema_path,
            r"
test.topic:
  payload: json_object
",
        )
        .unwrap();

        let yaml = format!(
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    schema_file: "{}"
"#,
            schema_path.display()
        );

        let mut config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let result = config.resolve_schema_files(&temp_dir);
        assert!(result.is_ok());

        let schemas = &config.event_loop.event_policy.as_ref().unwrap().schemas;
        assert!(schemas.contains_key("test.topic"));

        std::fs::remove_file(&schema_path).ok();
    }

    // ─── U1: terminal_events validation tests ───

    /// T-U1-V1: terminal topic 不在 publishes 中时返回 error。
    #[test]
    fn validate_terminal_topic_not_in_publishes_returns_error() {
        let yaml = r#"
hats:
  executor:
    name: "Executor"
    description: "Executes tasks"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    terminal_events: ["work.failed"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  starting_event: "work.start"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        let result = config.validate();
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::TerminalTopicNotInPublishes { hat, topic } => {
                assert_eq!(hat, "executor");
                assert_eq!(topic, "work.failed");
            }
            other => panic!("expected TerminalTopicNotInPublishes, got: {other:?}"),
        }
    }

    /// T-U1-V2: terminal topic 在 publishes 中时验证通过（无 error）。
    #[test]
    fn validate_terminal_topic_in_publishes_passes() {
        let yaml = r#"
hats:
  executor:
    name: "Executor"
    description: "Executes tasks"
    triggers: ["work.ready"]
    publishes: ["work.done", "work.failed"]
    terminal_events: ["work.done", "work.failed"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  starting_event: "work.start"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        let result = config.validate();
        assert!(result.is_ok(), "validation should pass: {:?}", result.err());
    }

    /// T-U1-V3: 空 terminal_events 产生 EmptyTerminalEvents warning。
    #[test]
    fn validate_empty_terminal_events_emits_warning() {
        let yaml = r#"
hats:
  legacy:
    name: "Legacy"
    description: "Legacy hat"
    triggers: ["work.start"]
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  starting_event: "work.start"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        let result = config.validate().expect("should not error");
        let has_empty_warning = result.iter().any(|w| {
            matches!(
                w,
                ConfigWarning::EmptyTerminalEvents { hat } if hat == "legacy"
            )
        });
        assert!(
            has_empty_warning,
            "expected EmptyTerminalEvents warning for legacy hat: {result:?}"
        );
    }

    /// T-U1-V4: 旧 preset（无 terminal_events 字段）产生 warning，不阻塞。
    #[test]
    fn validate_old_preset_no_terminal_events_produces_warning_not_error() {
        let yaml = r#"
hats:
  a:
    name: "A"
    description: "Hat A"
    triggers: ["work.start"]
    publishes: ["work.done"]
  b:
    name: "B"
    description: "Hat B"
    triggers: ["work.done"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  starting_event: "work.start"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        let result = config.validate().expect("should not error");
        // Both hats should produce EmptyTerminalEvents warnings
        let empty_warnings: Vec<_> = result
            .iter()
            .filter(|w| matches!(w, ConfigWarning::EmptyTerminalEvents { .. }))
            .collect();
        assert_eq!(empty_warnings.len(), 2, "both hats should warn: {result:?}");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // PROFILES CONFIG TESTS (U1 of plan 2026-06-25-002)
    // ─────────────────────────────────────────────────────────────────────────

    /// AC: `profiles.default` accepts a comma-separated string and is
    /// deserialized into a list of [`ProfileSpec`] with the `scope:`
    /// `name` shape preserved. Whitespace around each spec is trimmed.
    #[test]
    fn test_profiles_default_comma_separated_string() {
        let yaml = r#"
profiles:
  default: "repo:strict, user:my-style"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        assert_eq!(config.profiles.default.len(), 2);
        assert_eq!(config.profiles.default[0].scope, ProfileScope::Repo);
        assert_eq!(config.profiles.default[0].name, "strict");
        assert_eq!(config.profiles.default[1].scope, ProfileScope::User);
        assert_eq!(config.profiles.default[1].name, "my-style");
    }

    /// AC: `profiles.default` also accepts a YAML sequence form so users
    /// can use either style. Both must yield the same parsed list.
    #[test]
    fn test_profiles_default_yaml_sequence() {
        let yaml = r"
profiles:
  default:
    - repo:strict
    - user:my-style
";
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        assert_eq!(config.profiles.default.len(), 2);
        assert_eq!(config.profiles.default[0].scope, ProfileScope::Repo);
        assert_eq!(config.profiles.default[0].name, "strict");
        assert_eq!(config.profiles.default[1].scope, ProfileScope::User);
        assert_eq!(config.profiles.default[1].name, "my-style");
    }

    /// AC: extra whitespace around each spec in the comma-separated form
    /// must be trimmed away silently so users can format for readability
    /// without errors.
    #[test]
    fn test_profiles_default_comma_trims_whitespace() {
        let yaml = r#"
profiles:
  default: "  repo:strict  ,   user:my-style  "
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        assert_eq!(config.profiles.default.len(), 2);
        assert_eq!(config.profiles.default[0].name, "strict");
        assert_eq!(config.profiles.default[1].name, "my-style");
    }

    /// AC: omitting the `profiles:` section entirely is the backwards-
    /// compat path for existing `ralph.yml` files. The default value must
    /// be an empty list with no parse error.
    #[test]
    fn test_profiles_section_absent_uses_defaults() {
        let yaml = r"
agent: claude
event_loop:
  completion_promise: DONE
";
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        assert!(config.profiles.default.is_empty());
        assert_eq!(config.profiles, ProfilesConfig::default());
    }

    /// AC: explicitly empty `profiles.default` (empty string or empty
    /// sequence) must parse to an empty list — same shape as omitting the
    /// section. Both YAML forms (`""` and `[]`) are accepted.
    #[test]
    fn test_profiles_default_empty_string_and_empty_sequence() {
        let yaml_string = r#"
profiles:
  default: ""
"#;
        let cfg_string: RalphConfig =
            serde_yaml::from_str(yaml_string).expect("parse empty string");
        assert!(cfg_string.profiles.default.is_empty());

        let yaml_seq = r"
profiles:
  default: []
";
        let cfg_seq: RalphConfig = serde_yaml::from_str(yaml_seq).expect("parse empty seq");
        assert!(cfg_seq.profiles.default.is_empty());
    }

    /// AC: round-trip — `serde_yaml::to_value` then `from_value` must
    /// produce an identical [`ProfilesConfig`] value. This guards against
    /// custom deserializer drift in either direction.
    #[test]
    fn test_profiles_config_roundtrip() {
        let original = ProfilesConfig {
            default: vec![
                ProfileSpec {
                    scope: ProfileScope::Repo,
                    name: "strict".to_string(),
                },
                ProfileSpec {
                    scope: ProfileScope::User,
                    name: "my-style".to_string(),
                },
            ],
        };
        let value = serde_yaml::to_value(&original).expect("serialize");
        let restored: ProfilesConfig = serde_yaml::from_value(value).expect("deserialize");
        assert_eq!(restored, original);
    }

    /// AC: legacy `ralph.yml` (no `profiles` block, no other unusual
    /// fields) continues to parse after U1 — the new field must be
    /// additive and never break backwards compatibility.
    #[test]
    fn test_profiles_backward_compat_no_profiles_block() {
        let yaml = r#"
cli:
  backend: claude
hats:
  planner:
    name: "Planner"
    description: "Plans tasks"
    triggers: ["plan.start"]
"#;
        let result: Result<RalphConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_ok(),
            "legacy config without `profiles:` must still parse, got: {:?}",
            result.err()
        );
        let cfg = result.unwrap();
        assert!(cfg.profiles.default.is_empty());
    }

    /// AC: invalid scope values (anything other than `repo` or `user`)
    /// are rejected at parse time — they must surface as a serde YAML
    /// error rather than silently mapping to a wrong scope.
    #[test]
    fn test_profiles_default_invalid_scope_rejected() {
        let yaml = r#"
profiles:
  default: "team:strict"
"#;
        let result: Result<RalphConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_err(),
            "invalid scope 'team' must be rejected at parse time"
        );
    }

    /// AC: spec entries missing the `<name>` portion (e.g. `repo:` with
    /// no name) are rejected at parse time so misformatted configs
    /// surface early instead of producing empty-name specs.
    #[test]
    fn test_profiles_default_missing_name_rejected() {
        let yaml = r#"
profiles:
  default: "repo:"
"#;
        let result: Result<RalphConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_err(),
            "spec 'repo:' with empty name must be rejected"
        );
    }

    // =====================================================================
    // 2026-07-02-004 plan milestone A regression tests (U1-U4).
    // =====================================================================

    fn minimal_yaml() -> &'static str {
        r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
hats:
  planner:
    name: "Planner"
    description: "Plans tasks"
    triggers: ["task.start"]
    publishes: ["build.task"]
  builder:
    name: "Builder"
    description: "Builds things"
    triggers: ["build.task"]
    publishes: ["build.done"]
"#
    }

    /// U1 happy path: a minimal `precheck` block with one rule
    /// parses cleanly and round-trips through serde.
    #[test]
    fn precheck_config_round_trip() {
        let yaml = r#"
event_loop:
  precheck:
    enabled: true
    rules:
      review.complete:
        prompt:
          - "Findings are concrete and actionable"
          - "Each finding cites a file path"
        on_fail:
          target: "reviewer"
          retry_budget: 2
          on_exhausted: "plan.blocked(reason=precheck_failed)"
          reason: "review findings inadequate"
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).expect("parse");
        let precheck = cfg.event_loop.precheck.as_ref().expect("precheck set");
        assert!(precheck.enabled);
        assert_eq!(precheck.rules.len(), 1);
        let rule = precheck.rules.get("review.complete").expect("rule");
        assert_eq!(rule.prompt.len(), 2);
        assert_eq!(rule.on_fail.target, "reviewer");
        assert_eq!(rule.on_fail.retry_budget, 2);
        assert_eq!(
            rule.on_fail.on_exhausted,
            "plan.blocked(reason=precheck_failed)"
        );
        assert_eq!(rule.on_fail.reason, "review findings inadequate");

        // Round-trip via serialize
        let serialized = serde_yaml::to_string(&cfg).expect("serialize");
        let reparsed: RalphConfig = serde_yaml::from_str(&serialized).expect("re-parse");
        assert_eq!(
            cfg.event_loop.precheck.as_ref().unwrap().rules,
            reparsed.event_loop.precheck.as_ref().unwrap().rules
        );
    }

    /// U1 disabled-by-default: omitting `precheck` yields `None`.
    #[test]
    fn precheck_disabled_is_noop() {
        let cfg = RalphConfig::parse_yaml(minimal_yaml()).expect("parse");
        assert!(cfg.event_loop.precheck.is_none());
    }

    /// U2 desugar: when `precheck.enabled = true` and a rule
    /// guards `build.done`, a `precheck-build.done` hat appears,
    /// and the builder's `publishes` is rewritten to
    /// `build.done.proposed`.
    #[test]
    fn precheck_desugar_synthesizes_gate_hat() {
        let yaml = r#"
event_loop:
  precheck:
    enabled: true
    rules:
      build.done:
        prompt:
          - "Tests were run"
        on_fail:
          target: "builder"
hats:
  builder:
    name: "Builder"
    description: "build"
    triggers: ["task.start"]
    publishes: ["build.done"]
"#;
        let mut cfg = RalphConfig::parse_yaml(yaml).expect("parse");
        cfg.normalize();

        let gate_id = "precheck-build.done";
        let gate = cfg.hats.get(gate_id).expect("gate hat present");
        assert_eq!(gate.triggers, vec!["build.done.proposed".to_string()]);
        assert_eq!(
            gate.publishes,
            vec!["build.done".to_string(), "build.done.rejected".to_string()]
        );
        assert_eq!(
            gate.terminal_events,
            vec!["build.done".to_string(), "build.done.rejected".to_string()]
        );
        assert!(gate.description.is_some(), "gate must have a description");
        assert_eq!(gate.max_activations, Some(4), "retry_budget=3 default + 1");

        // Producer rewrite
        let builder = cfg.hats.get("builder").expect("builder hat");
        assert!(
            builder
                .publishes
                .contains(&"build.done.proposed".to_string()),
            "builder must publish proposed variant; got {:?}",
            builder.publishes
        );
        assert!(
            !builder.publishes.contains(&"build.done".to_string()),
            "builder must no longer publish raw topic; got {:?}",
            builder.publishes
        );
    }

    /// `default_publishes` fallback must route through the gate too:
    /// a silent hat's runtime-injected event lands directly on the bus,
    /// so a bare `<X>` default would bypass evidence audit, retry budget,
    /// and `plan.blocked` escalation. The desugar rewrites it to
    /// `<X>.proposed` so the injection triggers the gate hat.
    #[test]
    fn precheck_desugar_rewrites_default_publishes() {
        let yaml = r#"
event_loop:
  precheck:
    enabled: true
    rules:
      work.failed:
        prompt:
          - "Evidence file exists"
        on_fail:
          target: "executor"
hats:
  executor:
    name: "Executor"
    description: "exec"
    triggers: ["work.start"]
    publishes: ["work.done", "work.failed"]
    default_publishes: "work.failed"
"#;
        let mut cfg = RalphConfig::parse_yaml(yaml).expect("parse");
        cfg.normalize();

        let executor = cfg.hats.get("executor").expect("executor hat");
        assert_eq!(
            executor.default_publishes.as_deref(),
            Some("work.failed.proposed"),
            "default_publishes must be rewritten to the proposed variant"
        );
        // Gate-1 scope invariant in `check_default_publishes`: the default
        // topic must remain a member of the hat's publishes after desugar.
        assert!(
            executor
                .publishes
                .contains(&"work.failed.proposed".to_string()),
            "publishes must contain the proposed variant; got {:?}",
            executor.publishes
        );
        assert!(
            cfg.hats.contains_key("precheck-work.failed"),
            "gate hat must be synthesized"
        );
    }

    /// A hat whose `default_publishes` is NOT guarded by any precheck
    /// rule keeps its raw default topic (no spurious rewrite).
    #[test]
    fn precheck_desugar_leaves_unguarded_default_publishes_untouched() {
        let yaml = r#"
event_loop:
  precheck:
    enabled: true
    rules:
      work.failed:
        prompt:
          - "Evidence file exists"
        on_fail:
          target: "executor"
hats:
  executor:
    name: "Executor"
    description: "exec"
    triggers: ["work.start"]
    publishes: ["work.done", "work.failed"]
    default_publishes: "work.done"
"#;
        let mut cfg = RalphConfig::parse_yaml(yaml).expect("parse");
        cfg.normalize();

        let executor = cfg.hats.get("executor").expect("executor hat");
        assert_eq!(
            executor.default_publishes.as_deref(),
            Some("work.done"),
            "unguarded default topic must stay raw"
        );
    }

    /// U3 instructions: the gate hat's `instructions` field must
    /// include every checklist point, the hard-constraint
    /// directive, and the scope boundary.
    #[test]
    fn precheck_gate_instructions_contain_checklist() {
        let yaml = r#"
event_loop:
  precheck:
    enabled: true
    rules:
      review.complete:
        prompt:
          - "All findings are concrete"
          - "Each finding has a file path"
        on_fail:
          target: "reviewer"
"#;
        let mut cfg = RalphConfig::parse_yaml(yaml).expect("parse");
        cfg.normalize();

        let gate = cfg.hats.get("precheck-review.complete").expect("gate");
        let instructions = &gate.instructions;
        assert!(
            instructions.contains("All findings are concrete"),
            "checklist item 1 missing"
        );
        assert!(
            instructions.contains("Each finding has a file path"),
            "checklist item 2 missing"
        );
        assert!(
            instructions.contains("`review.complete`"),
            "must reference target topic in hard constraint"
        );
        assert!(
            instructions.contains("`review.complete.rejected`"),
            "must reference rejected variant"
        );
        assert!(
            instructions.contains("subjective judgement only"),
            "must include scope boundary"
        );
    }

    /// U4 zero-regression: a config without `precheck` must
    /// parse and normalize without producing any precheck-derived
    /// hats or rewriting any existing topic.
    #[test]
    fn precheck_absent_is_strict_noop() {
        let mut cfg = RalphConfig::parse_yaml(minimal_yaml()).expect("parse");
        let hats_before: Vec<String> = {
            let mut keys: Vec<String> = cfg.hats.keys().cloned().collect();
            keys.sort();
            keys
        };
        let builder_publishes_before = cfg.hats.get("builder").unwrap().publishes.clone();

        cfg.normalize();

        let hats_after: Vec<String> = {
            let mut keys: Vec<String> = cfg.hats.keys().cloned().collect();
            keys.sort();
            keys
        };
        assert_eq!(
            hats_before, hats_after,
            "no hats should be added when precheck is absent"
        );
        assert!(
            !hats_after.iter().any(|k| k.starts_with("precheck-")),
            "no precheck-* hats should exist; got {:?}",
            hats_after
        );
        assert_eq!(
            cfg.hats.get("builder").unwrap().publishes,
            builder_publishes_before,
            "builder.publishes must be unchanged"
        );
    }

    /// U4 zero-regression: `precheck.enabled = false` is a no-op
    /// even when rules are declared.
    #[test]
    fn precheck_disabled_block_is_noop() {
        let yaml = r#"
event_loop:
  precheck:
    enabled: false
    rules:
      build.done:
        prompt: ["check something"]
        on_fail:
          target: "builder"
hats:
  builder:
    name: "Builder"
    description: "build"
    triggers: ["task.start"]
    publishes: ["build.done"]
"#;
        let mut cfg = RalphConfig::parse_yaml(yaml).expect("parse");
        let builder_publishes_before = cfg.hats.get("builder").unwrap().publishes.clone();

        cfg.normalize();

        assert!(
            !cfg.hats.keys().any(|k| k.starts_with("precheck-")),
            "no gate hats when enabled=false"
        );
        assert_eq!(
            cfg.hats.get("builder").unwrap().publishes,
            builder_publishes_before,
            "builder.publishes must be unchanged when enabled=false"
        );
    }

    /// U4 kill switch: test override skips desugar when enabled.
    #[test]
    fn precheck_kill_switch_skips_desugar() {
        use super::precheck::{
            reset_precheck_kill_switch_for_test, set_precheck_kill_switch_for_test,
        };
        set_precheck_kill_switch_for_test(true);

        let yaml = r#"
event_loop:
  precheck:
    enabled: true
    rules:
      build.done:
        prompt: ["ok"]
        on_fail:
          target: builder
hats:
  builder:
    name: "Builder"
    triggers: ["task.start"]
    publishes: ["build.done"]
"#;
        let mut cfg = RalphConfig::parse_yaml(yaml).expect("parse");
        let before = cfg.hats.get("builder").unwrap().publishes.clone();
        cfg.normalize();
        assert!(
            !cfg.hats.keys().any(|k| k.starts_with("precheck-")),
            "kill switch must skip desugar"
        );
        assert_eq!(cfg.hats.get("builder").unwrap().publishes, before);

        reset_precheck_kill_switch_for_test();
    }

    /// U2 multi-producer: when multiple hats publish the
    /// guarded topic, all of them are rewritten to `.proposed`.
    #[test]
    fn precheck_desugar_handles_multiple_producers() {
        let yaml = r#"
event_loop:
  precheck:
    enabled: true
    rules:
      ship.done:
        prompt: ["safe to ship"]
        on_fail:
          target: "executor"
hats:
  shipper:
    name: "Shipper"
    description: "ship stuff"
    triggers: ["plan.complete"]
    publishes: ["ship.done"]
  alt_shipper:
    name: "Alt Shipper"
    description: "alt ship"
    triggers: ["plan.complete"]
    publishes: ["ship.done", "log.done"]
"#;
        let mut cfg = RalphConfig::parse_yaml(yaml).expect("parse");
        cfg.normalize();

        for hat_id in &["shipper", "alt_shipper"] {
            let hat = cfg.hats.get(*hat_id).expect(hat_id);
            assert!(
                hat.publishes.contains(&"ship.done.proposed".to_string()),
                "{} must publish proposed variant; got {:?}",
                hat_id,
                hat.publishes
            );
            assert!(
                !hat.publishes.contains(&"ship.done".to_string()),
                "{} must not publish raw ship.done; got {:?}",
                hat_id,
                hat.publishes
            );
        }

        // alt_shipper's other topic (log.done) must be untouched.
        let alt = cfg.hats.get("alt_shipper").unwrap();
        assert!(
            alt.publishes.contains(&"log.done".to_string()),
            "log.done must be untouched; got {:?}",
            alt.publishes
        );

        // Gate hat exists
        let gate = cfg.hats.get("precheck-ship.done").expect("gate");
        assert_eq!(gate.triggers, vec!["ship.done.proposed".to_string()]);
    }

    /// U2 consumer isolation: hats that only subscribe to the
    /// guarded topic via `triggers` must NOT be touched.
    #[test]
    fn precheck_desugar_preserves_consumers() {
        let yaml = r#"
event_loop:
  precheck:
    enabled: true
    rules:
      build.done:
        prompt: ["ok"]
        on_fail:
          target: "builder"
hats:
  builder:
    name: "Builder"
    description: "build"
    triggers: ["task.start"]
    publishes: ["build.done"]
  downstream:
    name: "Downstream"
    description: "consume build.done"
    triggers: ["build.done"]
    publishes: ["next.event"]
"#;
        let mut cfg = RalphConfig::parse_yaml(yaml).expect("parse");
        cfg.normalize();

        let downstream = cfg.hats.get("downstream").expect("downstream");
        assert_eq!(
            downstream.triggers,
            vec!["build.done".to_string()],
            "consumer's trigger on raw topic must be untouched; got {:?}",
            downstream.triggers
        );
        assert_eq!(
            downstream.publishes,
            vec!["next.event".to_string()],
            "consumer's publishes must be untouched"
        );
    }
}
