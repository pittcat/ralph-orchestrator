//! EventLoop implementation region 6.

use super::*;

impl EventLoop {
    /// U6: Append a `## Runtime Diagnosis Alert` block to the prompt
    /// when the recovery responder has findings that the next agent
    /// should see.
    ///
    /// This helper is the single chokepoint for prompt-level
    /// diagnosis injection and is called from every `build_prompt`
    /// path (solo ralph, multi-hat coordinator, isolated hat,
    /// backward-compat custom hat). The injection order is fixed by
    /// the U6 plan: `inject_phase_into_prompt` → diagnosis alert →
    /// `prepend_auto_inject_skills`, so the skills index never gets
    /// split by the alert.
    ///
    /// Returns `prompt` unchanged when the responder has nothing to
    /// surface (no pending findings, prompt injection disabled, or
    /// runtime-diagnosis entirely off).
    pub(super) fn apply_runtime_diagnosis_prompt(&self, prompt: String, hat_id: &HatId) -> String {
        if !self.config.telemetry.runtime_diagnosis.enabled
            || !self
                .config
                .telemetry
                .runtime_diagnosis
                .prompt_injection_enabled
        {
            return prompt;
        }
        if !self.recovery_responder.has_pending_findings() {
            return prompt;
        }
        let current_iteration = self.state.iteration;
        let hat_filter = if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && hat_id.as_str() != "ralph"
        {
            Some(hat_id)
        } else {
            None
        };
        self.recovery_responder
            .inject_prompt_alert(&prompt, hat_filter, current_iteration)
    }

    /// U6: Record a recovery envelope that the recovery responder
    /// should respond to. This is the single entry point that U4
    /// write paths use to feed the responder. The function
    ///
    /// 1. Writes the journal entry to `recovery.jsonl` (U3 behavior).
    /// 2. Emits the high-level audit event to `orchestration.jsonl`.
    /// 3. Updates the responder's in-memory state and computes the
    ///    escalation level for this iteration.
    ///
    /// The function never fails: I/O errors are swallowed (matching
    /// the existing U3 logger contract) and the responder is updated
    /// regardless so the in-memory state stays consistent.
    pub fn record_recovery_envelope(
        &mut self,
        envelope: &RecoveryDiagnosisEnvelope,
        notes: Vec<String>,
    ) -> crate::diagnosis::EscalationDecision {
        // 2026-06-28-003 P1: consult the runtime-recovery dispatcher
        // before persisting. If a DedupeEnvelope action matches this
        // envelope's retry_key, the dispatcher's view of the runtime
        // state already considers this envelope redundant (e.g. a
        // stall_recovery envelope on the same hat/topic was tracked
        // earlier in the iteration), so skip writing the duplicate to
        // recovery.jsonl and skip the orchestration audit event.
        if self.should_dedupe_envelope(envelope) {
            debug!(
                retry_key = %envelope.retry_key,
                "P1 dedupe: runtime-recovery dispatcher requested drop"
            );
            return crate::diagnosis::EscalationDecision {
                level: crate::diagnosis::EscalationLevel::Soft,
                retry_key: envelope.retry_key.clone(),
                attempt: 0,
                target_hat: None,
                reason: None,
            };
        }
        let hat = envelope
            .source_hat
            .as_deref()
            .unwrap_or(envelope.target_hat.as_deref().unwrap_or("ralph"));
        self.diagnostics
            .log_recovery(RecoveryJournalEntry::from_envelope(envelope.clone(), notes));
        self.diagnostics.log_orchestration(
            envelope.iteration.unwrap_or(0),
            hat,
            OrchestrationEvent::from_recovery_envelope(envelope),
        );
        let current_iteration = envelope
            .iteration
            .max(Some(self.state.iteration))
            .unwrap_or(0);
        self.recovery_responder
            .record_finding(envelope, current_iteration)
    }

    /// Returns true when the runtime-recovery dispatcher's
    /// `DedupeEnvelope` action matches `envelope.retry_key`.
    ///
    /// The dispatcher compares the candidate envelope against the
    /// currently tracked retry keys (plus pending findings from the
    /// same iteration) so a `missing_event_gate` envelope that
    /// duplicates an already-tracked `stall_recovery` on the same
    /// `(hat, topic)` is dropped before it pollutes recovery.jsonl.
    pub(super) fn should_dedupe_envelope(&self, envelope: &RecoveryDiagnosisEnvelope) -> bool {
        use crate::recovery_runtime::RecoveryAction;
        let ctx = self.runtime_recovery_context(&[]);
        crate::recovery_runtime::dispatch(&ctx)
            .iter()
            .any(|action| matches!(action, RecoveryAction::DedupeEnvelope { drop_retry_key } if drop_retry_key == &envelope.retry_key))
    }

    /// U11-T2 step-handoff side effects: when the unified pipeline
    /// rejects a `queue.advance` / `plan.complete` event, publish the
    /// same `plan.blocked` + diagnostic + recovery envelope that the
    /// legacy `apply_step_handoff_gate` used to emit. This keeps the
    /// operator-facing signal (`ralph diagnose`, responder ladder)
    /// intact while the gate decision itself lives in the pure
    /// `StepHandoffRule`.
    pub(super) fn emit_step_handoff_rejection_side_effects(
        &mut self,
        event: &JsonlEvent,
        result: &crate::validation::ValidationResult,
    ) {
        let (step, task_id) = {
            let payload = event.payload.as_deref().unwrap_or("");
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(payload) {
                let step = parsed
                    .get("step")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let task_id = parsed
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                (step, task_id)
            } else {
                (None, None)
            }
        };
        let reason = result
            .reason_code
            .as_deref()
            .and_then(|code| {
                code.strip_prefix(crate::validation::ReasonCode::STEP_HANDOFF_MISMATCH_PREFIX)
            })
            .unwrap_or("progress_task_mismatch");
        let detail = result.correction_hint.as_deref().unwrap_or("");

        let blocked_payload = serde_json::json!({
            "reason": reason,
            "topic": event.topic,
            "step": step,
            "task_id": task_id,
            "detail": detail,
        });
        let source_hat = HatId::from("plan-gate");
        let blocked =
            Event::new("plan.blocked", blocked_payload.to_string()).with_source(source_hat);
        self.bus.publish(blocked);

        let diagnostic = Event::new(
            "event.step_handoff.gate_rejected",
            format!(
                "step_handoff gate rejected topic='{}' reason={}",
                event.topic, reason
            ),
        );
        self.bus.publish(diagnostic);

        let envelope = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::PayloadContract)
            .severity(crate::diagnosis::DiagnosisSeverity::Warning)
            .iteration(self.state.iteration)
            .source_hat("plan-gate")
            .target_hat("plan-gate")
            .topic(event.topic.clone())
            .reason_code(reason)
            .message(format!(
                "step_handoff gate rejected topic='{}' reason={} detail={}",
                event.topic, reason, detail
            ))
            .safe_target(true)
            .build();
        self.record_recovery_envelope(&envelope, Vec::new());
    }

    /// U6: Mark the next iteration as fresh. Clears the responder's
    /// per-iteration caches (`pending_findings`, hard-escalation
    /// queue, termination hint) so the prompt builder does not
    /// re-inject stale alerts.
    pub fn begin_diagnosis_iteration(&mut self) {
        self.recovery_responder.begin_iteration();
    }

    /// U6: Read-only access to the recovery responder. Useful for
    /// the loop runner when checking the most recent hard
    /// escalation or termination hint.
    pub fn recovery_responder(&self) -> &RecoveryResponder {
        &self.recovery_responder
    }

    /// U6: Mutable access to the recovery responder. Used by the
    /// loop runner to mark findings as recovered after each
    /// iteration.
    pub fn recovery_responder_mut(&mut self) -> &mut RecoveryResponder {
        &mut self.recovery_responder
    }

    /// 2026-06-28-003: build a runtime-recovery context from the
    /// current loop state. Used by hot-path detectors.
    ///
    /// `extra_jsonl_events` are appended to the pending regular events so
    /// detectors can see just-accepted JSONL events that have not yet
    /// been published to the bus.
    pub(crate) fn runtime_recovery_context(
        &self,
        extra_jsonl_events: &[crate::event_reader::Event],
    ) -> crate::recovery_runtime::RuntimeContext {
        use crate::diagnosis::DiagnosisSource;
        use crate::recovery_runtime::{EnvelopeSnapshot, EventSnapshot, RetryKeyState};

        let mut ctx = crate::recovery_runtime::RuntimeContext {
            current_iteration: self.state.iteration,
            current_hat: self.state.last_hat.as_ref().map(|h| h.as_str().to_string()),
            ..Default::default()
        };

        // Snapshot the executor-class hat set from the live registry
        // so `block_executor_resend_storm` matches structurally on
        // `publishes contains work.done` rather than a hard-coded
        // "executor" string. Empty registry (test scaffolding) leaves
        // the list empty and the detector falls back to the legacy
        // string match for backwards compatibility.
        for id in self.registry.ids() {
            if let Some(cfg) = self.registry.get_config(id)
                && cfg.publishes.iter().any(|t| t == "work.done")
            {
                ctx.executor_hat_ids.push(id.as_str().to_string());
            }
        }

        // Snapshot tracked retry keys.
        for key in self.recovery_responder.tracked_retry_keys_list() {
            let outcome = self
                .recovery_responder
                .outcome_for(&key)
                .map(|o| format!("{o:?}"))
                .unwrap_or_else(|| "Pending".to_string());
            let attempt = self.recovery_responder.attempt_count(&key);
            let history: Vec<String> = self
                .recovery_responder
                .outcome_history_snapshot(&key)
                .into_iter()
                .map(|o| format!("{o:?}"))
                .collect();
            ctx.retry_key_states.push(RetryKeyState {
                retry_key: key.clone(),
                last_outcome: outcome.clone(),
                outcome_history: history,
                attempt_count: attempt,
            });
        }

        // Snapshot recent pending regular events plus any extra JSONL
        // events supplied by the caller (e.g. a freshly accepted work.done).
        for event in self.peek_pending_regular_events() {
            ctx.events.push(EventSnapshot {
                topic: event.topic.to_string(),
                payload: event.payload.clone(),
                iteration: self.state.iteration,
            });
        }
        for event in extra_jsonl_events {
            ctx.events.push(EventSnapshot {
                topic: event.topic.clone(),
                payload: event.payload.clone().unwrap_or_default(),
                iteration: self.state.iteration,
            });
        }

        // Snapshot pending findings as recovery envelopes.
        for finding in self.recovery_responder.pending_findings() {
            ctx.recovery_envelopes.push(EnvelopeSnapshot {
                retry_key: finding.retry_key.clone(),
                source: match finding.source {
                    DiagnosisSource::StallRecovery => "StallRecovery".to_string(),
                    DiagnosisSource::MissingEventGate => "MissingEventGate".to_string(),
                    DiagnosisSource::DriftMonitor => "DriftMonitor".to_string(),
                    DiagnosisSource::WorkflowGuard => "WorkflowGuard".to_string(),
                    DiagnosisSource::ExecutionContract => "ExecutionContract".to_string(),
                    DiagnosisSource::PayloadContract => "PayloadContract".to_string(),
                    DiagnosisSource::HookRetry => "HookRetry".to_string(),
                    DiagnosisSource::LoopStale => "LoopStale".to_string(),
                    DiagnosisSource::TopicFormat => "TopicFormat".to_string(),
                    _ => "Other".to_string(),
                },
                outcome: format!("{:?}", finding.outcome),
                iteration: finding.iteration.unwrap_or(self.state.iteration),
                attempt: finding.retry_attempt,
            });
        }

        ctx
    }

    /// 2026-06-28-003: run runtime-recovery detectors against the
    /// supplied context and apply the returned actions to the loop.
    /// Detectors are best-effort: a missing signal causes silent skip.
    pub fn apply_runtime_recovery_actions(
        &mut self,
        ctx: &crate::recovery_runtime::RuntimeContext,
    ) {
        use crate::recovery_runtime::RecoveryAction;
        use ralph_proto::{Event, HatId};

        for action in crate::recovery_runtime::dispatch(ctx) {
            match action {
                RecoveryAction::PublishEvent { topic, payload } => {
                    debug!(topic = %topic, "runtime-recovery: publishing corrective event");
                    let event =
                        Event::new(topic.as_str(), payload).with_source(HatId::from("ralph"));
                    // 2026-07-06 U2 (DEV-002): persist runtime-recovery
                    // corrective events to events.jsonl alongside the
                    // bus publish. Without this the trusted events stream
                    // diverges from the in-memory bus and downstream
                    // shipper routing gates (see shipper_reason.rs) miss
                    // the recovery context.
                    self.state.record_event(&event);
                    self.bus.publish(event);
                }
                RecoveryAction::ForcePlanBlocked { reason, retry_key } => {
                    warn!(%reason, %retry_key, "runtime-recovery: forcing plan.blocked");
                    let payload = serde_json::json!({
                        "reason": format!("recovery_exhausted:{retry_key}"),
                        "runtime_recovery_reason": reason,
                    });
                    // 2026-07-24-005 plan U1: target is `reporter`
                    // (was `shipper`); the shipper hat is removed
                    // from the supervisor preset — reporter is the
                    // canonical `plan.blocked` terminal owner.
                    let blocked = Event::new("plan.blocked", payload.to_string())
                        .with_source(HatId::from("ralph"))
                        .with_target(HatId::from("reporter"));
                    // 2026-07-06 U2 (DEV-002): persist the terminal
                    // plan.blocked to events.jsonl. Previously only
                    // bus.publish was called, leaving events.jsonl
                    // silent while the in-memory bus still routed
                    // downstream — silent-success path.
                    //
                    // ===========================================================================
                    // P0-1 LINT GUARD (2026-07-06 silent-success regression):
                    // DO NOT REORDER. `state.record_event(&blocked)` MUST run BEFORE
                    // `bus.publish(blocked)`. Otherwise the trusted events.jsonl
                    // diverges from the in-memory bus and shipper's
                    // `is_recoverable_plan_blocked_reason` lookup reads stale
                    // data, producing REVIEW_COMPLETE(pass) over a plan.blocked
                    // that was never persisted. This was the root cause of the
                    // 9-recurrence silent-success loop family
                    // (primary-20260705-224028 + 8 prior runs).
                    //
                    // If you must change this ordering, first read:
                    //   - `docs/report/2026-07-06-ce-executor-serial-primary-20260705-224028-diagnosis.md`
                    //   - `crates/ralph-core/src/recovery_runtime/publish_loop_stalled.rs`
                    //     (which now emits `recovery_exhausted:<retry_key>` literals
                    //     to align with this path)
                    // ===========================================================================
                    self.state.record_event(&blocked);
                    self.bus.publish(blocked);
                }
                RecoveryAction::InjectDirective { text } => {
                    warn!(%text, "runtime-recovery: directive injection requested");
                    // Store for the next prompt build. build_prompt drains
                    // the buffer so the directive is delivered exactly once.
                    self.state.pending_recovery_directives.push(text);
                }
                RecoveryAction::DedupeEnvelope { drop_retry_key } => {
                    debug!(%drop_retry_key, "runtime-recovery: envelope dedupe requested");
                    // Callers that record envelopes should check this action
                    // and skip writing the duplicate.
                }
            }
        }
    }

    /// This generalizes the former `prepend_memories()` into a skill auto-injection
    /// pipeline that handles memories, tools, and any other auto-inject skills.
    ///
    /// Injection order:
    /// 1. Memory data + ralph-tools skill (special case: loads memory data from store, applies budget)
    /// 2. Other auto-inject skills from the registry (wrapped in XML tags)
    ///
    /// Note (2026-06-25 refactor): the former step 2 was "RObot interaction skill (gated by
    /// `robot.enabled`)", which was removed together with the `ralph-telegram` crate; the
    /// `human.guidance` / `task.resume` recovery channel is unrelated and preserved.
    pub(super) fn prepend_auto_inject_skills(&self, prompt: String, hat_id: &HatId) -> String {
        let mut prefix = String::new();

        // 1. Memory data + ralph-tools skill — special case with data loading
        self.inject_memories_and_tools_skill(&mut prefix, hat_id);

        // 2. Other auto-inject skills from the registry
        self.inject_custom_auto_skills(&mut prefix, hat_id);

        if prefix.is_empty() {
            return prompt;
        }

        prefix.push_str("\n\n");
        prefix.push_str(&prompt);
        prefix
    }

    /// Injects memory data and the ralph-tools skill into the prefix.
    ///
    /// Special case: loads memory entries from the store, applies budget
    /// truncation, then appends the ralph-tools skill content (which covers
    /// both tasks and memories CLI usage).
    /// Memory data is gated by `memories.enabled && memories.inject == Auto`.
    /// The ralph-tools skill is injected when either memories or tasks are enabled.
    pub(super) fn inject_memories_and_tools_skill(&self, prefix: &mut String, hat_id: &HatId) {
        let memories_config = &self.config.memories;

        // Inject memory DATA if memories are enabled with auto-inject
        if memories_config.enabled && memories_config.inject == InjectMode::Auto {
            info!(
                "Memory injection check: enabled={}, inject={:?}, workspace_root={:?}",
                memories_config.enabled, memories_config.inject, self.config.core.workspace_root
            );

            let workspace_root = &self.config.core.workspace_root;
            let store = MarkdownMemoryStore::with_default_path(workspace_root);
            let memories_path = workspace_root.join(".ralph/agent/memories.md");

            info!(
                "Looking for memories at: {:?} (exists: {})",
                memories_path,
                memories_path.exists()
            );

            let memories = match store.load() {
                Ok(memories) => {
                    info!("Successfully loaded {} memories from store", memories.len());
                    memories
                }
                Err(e) => {
                    info!(
                        "Failed to load memories for injection: {} (path: {:?})",
                        e, memories_path
                    );
                    Vec::new()
                }
            };

            if memories.is_empty() {
                info!("Memory store is empty - no memories to inject");
            } else {
                let mut memories_content = format_memories_as_markdown(&memories);

                if memories_config.budget > 0 {
                    let original_len = memories_content.len();
                    memories_content =
                        truncate_to_budget(&memories_content, memories_config.budget);
                    debug!(
                        "Applied budget: {} chars -> {} chars (budget: {})",
                        original_len,
                        memories_content.len(),
                        memories_config.budget
                    );
                }

                info!(
                    "Injecting {} memories ({} chars) into prompt",
                    memories.len(),
                    memories_content.len()
                );

                prefix.push_str(&memories_content);
            }
        }

        // Inject ralph-tools skills via the SSOT plan_auto_inject.
        // plan_auto_inject already honours per-hat eligibility
        // (is_hat_eligible) and the gated/registry-auto split, so
        // the live path and the preview path produce identical
        // results.
        //
        // 2026-07-26-002 U1: chain only `gated` here. Custom
        // registry-auto skills are owned by
        // `inject_custom_auto_skills` below — chaining both sets
        // here produced double injection of any
        // `skills.overrides.<name>.auto_inject: true` skill.
        let (gated, _registry_auto, _on_demand) =
            SkillInjector::plan_auto_inject(&self.config, hat_id, &self.skill_registry);

        for entry in gated {
            let Some(skill) = self.skill_registry.get(entry.name.as_str()) else {
                continue;
            };
            if !prefix.is_empty() {
                prefix.push_str("\n\n");
            }
            prefix.push_str(&format!(
                "<{name}-skill>\n{content}\n</{name}-skill>",
                name = entry.name,
                content = skill.content.trim()
            ));
            debug!("Injected {} skill from registry", entry.name);
        }
    }

    /// Injects any user-configured auto-inject skills (excluding built-in skills handled separately).
    pub(super) fn inject_custom_auto_skills(&self, prefix: &mut String, hat_id: &HatId) {
        // U8: the per-hat filter was previously dropped on the floor
        // (None), so hat-restricted skills were being injected into
        // every hat. Threading `hat_id` here is what the plan KTD calls
        // out as the "auto_inject_skills(None) → auto_inject_skills(Some(...))"
        // fix.
        for skill in self
            .skill_registry
            .auto_inject_skills(Some(hat_id.as_str()))
        {
            // Skip built-in skills handled above
            //
            // 2026-06-25 refactor: `robot-interaction` was removed because its
            // only content was `human.interact` / `human.guidance` Telegram
            // guidance; the `ralph-telegram` crate was deleted (see plan
            // 2026-06-25-001). No other Telegram-specific skills remain.
            //
            // U8: `ralph-tools-opac` is also handled above (it lives
            // in the ralph-tools injection block so the agent gets one
            // consolidated skill doc, not three at the bottom).
            if matches!(
                skill.name.as_str(),
                "ralph-tools" | "ralph-tools-tasks" | "ralph-tools-memories" | "ralph-tools-opac"
            ) {
                continue;
            }

            if !prefix.is_empty() {
                prefix.push_str("\n\n");
            }
            prefix.push_str(&format!(
                "<{name}-skill>\n{content}\n</{name}-skill>",
                name = skill.name,
                content = skill.content.trim()
            ));
            debug!("Injected auto-inject skill: {}", skill.name);
        }
    }

    /// Extract recovery directive IDs from a batch of pending events.
    ///
    /// Only `task.resume` events are inspected. The `recovery_directives`
    /// array is read from each payload, flattened, deduplicated while
    /// preserving first-seen order. Unknown IDs are kept (the lookup
    /// step skips them).
    pub(super) fn recovery_directive_ids_from_events(events: &[Event]) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut ordered = Vec::new();
        for event in events {
            if event.topic.as_str() != "task.resume" {
                continue;
            }
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) else {
                continue;
            };
            let Some(array) = payload
                .get("recovery_directives")
                .and_then(|v| v.as_array())
            else {
                continue;
            };
            for item in array {
                let Some(id) = item.as_str() else {
                    continue;
                };
                if seen.insert(id.to_string()) {
                    ordered.push(id.to_string());
                }
            }
        }
        ordered
    }

    /// Build the `## RECOVERY DIRECTIVES` prompt section from the
    /// registered `ralph-tools-recovery-directives` skill.
    ///
    /// For each directive ID, the matching `## <ID>` section is extracted
    /// from the skill markdown. IDs without a matching section are
    /// silently skipped. Returns an empty string when there are no IDs
    /// or the skill is not registered.
    pub(super) fn build_recovery_directives_section(&self, directive_ids: &[String]) -> String {
        if directive_ids.is_empty() {
            return String::new();
        }
        let Some(skill) = self.skill_registry.get("ralph-tools-recovery-directives") else {
            return String::new();
        };
        let content = skill.content.trim();
        let mut sections: Vec<String> = Vec::new();
        for id in directive_ids {
            let marker = format!("## {id}");
            let Some(start) = content.find(&marker) else {
                continue;
            };
            let rest = &content[start + marker.len()..];
            let end = rest.find("\n## ").unwrap_or(rest.len());
            let section = &content[start..start + marker.len() + end];
            sections.push(section.trim().to_string());
        }
        if sections.is_empty() {
            return String::new();
        }
        let mut out = String::from("## RECOVERY DIRECTIVES\n\n");
        out.push_str(
            "The following runtime directives apply to pending `task.resume` events. \
             Treat them as system operating procedure.\n\n",
        );
        for (i, section) in sections.iter().enumerate() {
            if i > 0 {
                out.push_str("\n\n");
            }
            out.push_str(section);
        }
        out.push('\n');
        out
    }

    /// Prepend recovery directives (if any) to the prompt.
    pub(super) fn prepend_recovery_directives(
        &mut self,
        prompt: String,
        events: &[Event],
    ) -> String {
        let ids = Self::recovery_directive_ids_from_events(events);
        let mut section = self.build_recovery_directives_section(&ids);
        // 2026-06-28-003: also prepend directives produced by in-flight
        // runtime-recovery detectors (e.g. resend-storm block).
        let runtime_directives = std::mem::take(&mut self.state.pending_recovery_directives);
        if !runtime_directives.is_empty() {
            if section.is_empty() {
                section = String::from("## RECOVERY DIRECTIVES\n\n");
            }
            for directive in runtime_directives {
                section.push_str("\n- ");
                section.push_str(&directive);
            }
            section.push('\n');
        }
        if section.is_empty() {
            return prompt;
        }
        format!("{section}\n{prompt}")
    }

    /// 2026-07-09-003 plan (U3): prepend the
    /// `## TRIGGER CONTEXT` block derived from the schema-
    /// declared `trigger_context` (U1) of the most recent
    /// accepted event that the current hat subscribed to.
    ///
    /// The block is rendered by [`crate::trigger_context`]:
    /// the helper here is the runtime wiring that finds the
    /// matching trigger, looks up the schema, and decides
    /// whether to inject at all. Three gates short-circuit to
    /// a no-op prompt (SC6 / R3 / R29):
    ///
    /// 1. `event_policy` is absent (no schemas declared).
    /// 2. No event in `regular_events` matches the hat's
    ///    declared `triggers` (no trigger ⇒ no context).
    /// 3. The schema for the matched topic has no
    ///    `trigger_context` declaration (default-empty
    ///    `TriggerContextConfig`).
    ///
    /// Topology safety (R21 / R22): the helper filters by the
    /// hat's own `triggers` list, so a `## TRIGGER CONTEXT`
    /// block can never be injected into a hat that did not
    /// subscribe to the source topic. U5 wires a sibling lint
    /// that catches the same mistake statically.
    ///
    /// The block is intentionally prepended **above** every
    /// other prepend helper so the agent sees the trigger
    /// summary first (R13 / R17 / KTD-5).
    pub(super) fn prepend_trigger_context(
        &self,
        prompt: String,
        hat_id: &HatId,
        regular_events: &[Event],
    ) -> String {
        // Gate 1: no event policy ⇒ no schemas ⇒ no block.
        let Some(policy) = self.config.event_loop.event_policy.as_ref() else {
            return prompt;
        };

        // The current hat's declared triggers drive the
        // topology guard. We never fall back to a wildcard
        // search — a hat that subscribes to no topics must
        // not see a trigger context.
        let Some(hat_config) = self.registry.get_config(hat_id) else {
            return prompt;
        };
        let hat_triggers: Vec<String> = hat_config.triggers.clone();
        if hat_triggers.is_empty() {
            return prompt;
        }

        // Find the most recent non-system event the hat
        // subscribes to.
        let Some(trigger) =
            crate::trigger_context::find_matching_trigger_event(regular_events, &hat_triggers)
        else {
            return prompt;
        };

        // Gate 2: schema for the source topic must exist and
        // declare a non-empty `trigger_context` block.
        let Some(schema) = policy.schemas.get(trigger.topic) else {
            return prompt;
        };
        if schema.trigger_context.summary_fields.is_empty()
            && schema.trigger_context.routing_hints.is_empty()
        {
            return prompt;
        }

        // Build + render. `source_hat` is unknown at this
        // layer (events do not carry it), so the renderer
        // surfaces `(unknown source hat)`. That is a U4 / U5
        // observable gap that strict lint can flag if a
        // schema/preset relies on it.
        let view = crate::trigger_context::build(&crate::trigger_context::TriggerContextInput {
            current_hat: hat_id.as_str(),
            source_topic: trigger.topic,
            source_hat: None,
            schema,
            payload: &trigger.payload,
        });

        let Some(block) = crate::trigger_context::render(&view) else {
            return prompt;
        };

        format!("{block}\n{prompt}")
    }

    /// Prepends scratchpad content to the prompt if the file exists and is non-empty.
    ///
    /// The scratchpad is the agent's working memory for the current objective.
    /// Auto-injecting saves one tool call per iteration.
    /// When the file exceeds the budget, the TAIL is kept (most recent entries).
    pub(super) fn prepend_scratchpad(
        &self,
        prompt: String,
        active_hat_id_for_filter: Option<&HatId>,
    ) -> String {
        // Skip injection when scratchpad is disabled for the current hat
        if !self.ralph.active_scratchpad().enabled {
            return prompt;
        }

        let scratchpad_path = self.scratchpad_path();

        let resolved_path = if scratchpad_path.is_relative() {
            self.config.core.workspace_root.join(&scratchpad_path)
        } else {
            scratchpad_path
        };

        if !resolved_path.exists() {
            debug!(
                "Scratchpad not found at {:?}, skipping injection",
                resolved_path
            );
            return prompt;
        }

        let content = match std::fs::read_to_string(&resolved_path) {
            Ok(c) => c,
            Err(e) => {
                info!("Failed to read scratchpad for injection: {}", e);
                return prompt;
            }
        };

        if content.trim().is_empty() {
            debug!("Scratchpad is empty, skipping injection");
            return prompt;
        }

        // Unit 3 (2026-06-16-002 plan): when the active hat is the
        // `coordinator` and the loop is still in the bootstrap
        // window, strip `### HUMAN GUIDANCE` blocks from the
        // scratchpad snapshot.  We use the same state-machine
        // header detection as `persist_guidance_to_scratchpad` so
        // a line in `## NOTES` that happens to look like a
        // guidance block is not falsely stripped.
        //
        // U2 (2026-06-18-004 plan, R2, KTD2): when the loop
        // opts into `suppress_human_guidance` (ce-executor-serial),
        // strip the same blocks for the active hat regardless of
        // bootstrap state. This is the source of the perky-maple
        // P1-2 probe storm — the executor hat saw `### HUMAN
        // GUIDANCE: Focus on error handling` and went into a
        // 6-round emit-probing spiral.
        let gate_closed = active_hat_id_for_filter
            .map(|hat| self.coordinator_bootstrap_gate_closed(hat))
            .unwrap_or(false);
        let suppress_active = self.human_guidance_suppressed();
        // 2026-06-28-005: filter_human_guidance_blocks was
        // deleted together with the `human.guidance` topic.
        // The bootstrap gate (`gate_closed`) is the only
        // remaining reason to filter the scratchpad today.
        let content = if gate_closed {
            // Drop the `### HUMAN GUIDANCE` block from any
            // historical scratchpad that pre-dates the topic
            // removal. This is purely defensive: a scratchpad
            // from before 2026-06-28 might still contain the
            // block; the regex-free inline filter below
            // strips it line by line. We keep the filter
            // here as a small private helper rather than
            // pulling back the public filter function.
            strip_human_guidance_block(&content)
        } else if suppress_active {
            // Suppress is now a no-op (the topic it gated is
            // gone). Kept for backwards-compatible YAML
            // loading — the field still deserializes (see
            // Phase 3b U7) and we simply do not act on it.
            content
        } else {
            content
        };
        if content.trim().is_empty() {
            debug!("Scratchpad empty after bootstrap filter, skipping injection");
            return prompt;
        }

        // Budget: 4000 tokens ~16000 chars. Keep the TAIL (most recent content).
        let char_budget = 4000 * 4;
        let content = if content.len() > char_budget {
            // Find a line boundary near the start of the tail
            let start = content.len() - char_budget;
            // Ensure we start at a valid UTF-8 character boundary
            let start = floor_char_boundary(&content, start);
            let line_start = content[start..].find('\n').map_or(start, |n| start + n + 1);
            let discarded = &content[..line_start];

            // Summarize discarded content by extracting markdown headings
            let headings: Vec<&str> = discarded
                .lines()
                .filter(|line| line.starts_with('#'))
                .collect();
            let summary = if headings.is_empty() {
                format!(
                    "<!-- earlier content truncated ({} chars omitted) -->",
                    line_start
                )
            } else {
                format!(
                    "<!-- earlier content truncated ({} chars omitted) -->\n\
                     <!-- discarded sections: {} -->",
                    line_start,
                    headings.join(" | ")
                )
            };

            format!("{}\n\n{}", summary, &content[line_start..])
        } else {
            content
        };

        info!("Injecting scratchpad ({} chars) into prompt", content.len());

        let mut final_prompt = format!(
            "<scratchpad path=\"{}\">\n{}\n</scratchpad>\n\n",
            self.ralph.active_scratchpad().path,
            content
        );
        final_prompt.push_str(&prompt);
        final_prompt
    }

    /// Prepends ready tasks to the prompt if tasks are enabled and any exist.
    ///
    /// Loads the task store and formats ready (unblocked, open) tasks into
    /// a `<ready-tasks>` XML block. This saves the agent a tool call per
    /// iteration and puts tasks at the same prominence as the scratchpad.
    ///
    /// The `is_actionable` check below intentionally decouples
    /// **execution-ability** (whether this hat should *run* the task —
    /// owner match only) from **lifecycle mutation rights** (whether
    /// the hat can `start` / `close` / `fail` / `reopen` — owner OR
    /// membership in `tasks.coordinator_hats`, see
    /// `task::can_hat_mutate_task_lifecycle` and the `task_cli` auth
    /// paths). A coordinator hat may still mutate any task for
    /// coordination purposes, but the prompt must not invite it to
    /// *execute* a unit task it does not own: non-self-owner ready
    /// tasks are rendered with a `[read-only]` marker so the
    /// coordinator does not call `task start` on someone else's unit.
    /// When no ready task is actionable for the caller, a one-line
    /// "no actionable tasks" header replaces the actionable-looking
    /// list (which historically parked the activation until the
    /// no-progress watchdog killed the loop).
    pub(super) fn prepend_ready_tasks(&self, prompt: String, hat_id: Option<&HatId>) -> String {
        if !self.config.tasks.enabled {
            return prompt;
        }

        use crate::task::TaskStatus;
        use crate::task_store::TaskStore;

        let tasks_path = self.tasks_path();
        let resolved_path = if tasks_path.is_relative() {
            self.config.core.workspace_root.join(&tasks_path)
        } else {
            tasks_path
        };

        if !resolved_path.exists() {
            return prompt;
        }

        let store = match TaskStore::load(&resolved_path) {
            Ok(s) => s,
            Err(e) => {
                info!("Failed to load task store for injection: {}", e);
                return prompt;
            }
        };

        let current_loop_id = self.current_loop_id();

        let ready = Self::filter_tasks_by_loop(store.ready(), current_loop_id.as_deref());
        let open = Self::filter_tasks_by_loop(store.open(), current_loop_id.as_deref());
        let all_count =
            Self::filter_tasks_by_loop(store.all().iter().collect(), current_loop_id.as_deref())
                .len();
        let closed_count = all_count - open.len();

        if open.is_empty() && closed_count == 0 {
            return prompt;
        }

        let mut section = String::from("<ready-tasks>\n");
        if ready.is_empty() && open.is_empty() {
            section.push_str("No open tasks. Create tasks with `ralph tools task add`.\n");
        } else {
            // Use the shared execution-contract evaluator so prompt
            // actionability and agent CLI authorization cannot drift.
            let caller_hat_str: Option<&str> = hat_id.map(|hat| hat.as_str());
            let task_capability = |task: &crate::task::Task| {
                crate::execution_contract::evaluate_task_capability(
                    task,
                    caller_hat_str,
                    current_loop_id.as_deref(),
                    &self.config.tasks.coordinator_hats,
                )
            };
            let is_actionable = |task: &crate::task::Task| -> bool {
                caller_hat_str.is_none() || task_capability(task).actionable_now
            };
            let any_actionable_ready = ready.iter().any(|t| is_actionable(t));
            let header = if caller_hat_str.is_some() && !any_actionable_ready {
                format!(
                    "## Tasks: {} ready, {} open, {} closed — none actionable for this hat (all read-only)\n\n",
                    ready.len(),
                    open.len(),
                    closed_count
                )
            } else {
                format!(
                    "## Tasks: {} ready, {} open, {} closed\n\n",
                    ready.len(),
                    open.len(),
                    closed_count
                )
            };
            section.push_str(&header);
            for task in &ready {
                let status_icon = match task.status {
                    TaskStatus::Open => "[ ]",
                    TaskStatus::InProgress => "[~]",
                    _ => "[?]",
                };
                let ro_marker = if is_actionable(task) {
                    ""
                } else {
                    " [read-only]"
                };
                section.push_str(&format!(
                    "- {} [P{}] {} ({}){}{}\n",
                    status_icon,
                    task.priority,
                    task.title,
                    task.id,
                    task.key
                        .as_deref()
                        .map(|key| format!(" — key: {key}"))
                        .unwrap_or_default(),
                    ro_marker
                ));
            }
            // Show blocked tasks separately so agent knows they exist
            let ready_ids: Vec<&str> = ready.iter().map(|t| t.id.as_str()).collect();
            let blocked: Vec<_> = open
                .iter()
                .filter(|t| !ready_ids.contains(&t.id.as_str()))
                .collect();
            if !blocked.is_empty() {
                section.push_str("\nBlocked:\n");
                for task in blocked {
                    let ro_marker =
                        if caller_hat_str.is_none() || task_capability(task).execution_ownership {
                            ""
                        } else {
                            " [read-only]"
                        };
                    section.push_str(&format!(
                        "- [blocked] [P{}] {} ({}){} — blocked by: {}{}\n",
                        task.priority,
                        task.title,
                        task.id,
                        task.key
                            .as_deref()
                            .map(|key| format!(" — key: {key}"))
                            .unwrap_or_default(),
                        task.blocked_by.join(", "),
                        ro_marker
                    ));
                }
            }
        }
        section.push_str("</ready-tasks>\n\n");

        info!(
            "Injecting ready tasks ({} ready, {} open, {} closed) into prompt",
            ready.len(),
            open.len(),
            closed_count
        );

        let mut final_prompt = section;
        final_prompt.push_str(&prompt);
        final_prompt
    }

    /// Prepends state file contents to the prompt if state files are configured.
    pub(super) fn prepend_state_files(&self, prompt: String) -> String {
        let config = match &self.config.core.state_files {
            Some(c) if c.enabled => c,
            _ => return prompt,
        };
        crate::state_file_injector::inject_state_files(
            prompt,
            config,
            &self.config.core.workspace_root,
        )
    }
}
