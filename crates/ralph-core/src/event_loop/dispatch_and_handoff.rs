//! EventLoop implementation region 8.

use super::*;

impl EventLoop {
    /// Determines which hats should be active based on pending events.
    /// Returns list of Hat references that are triggered by any pending event.
    pub(super) fn determine_active_hats(&self, events: &[Event]) -> Vec<&Hat> {
        let mut active_hats = Vec::new();
        for id in self.determine_active_hat_ids(events) {
            if let Some(hat) = self.registry.get(&id) {
                active_hats.push(hat);
            }
        }
        active_hats
    }

    pub(super) fn determine_active_hat_ids(&self, events: &[Event]) -> Vec<HatId> {
        let mut entrypoint_hat_ids = Vec::new();
        let mut progressed_hat_ids = Vec::new();
        for event in events {
            // Skip system/observability events (event.*) — they are not hat
            // progress signals, only diagnostic/audit trails. The Ralph
            // fallback hat subscribes to "*" and would otherwise activate
            // for `event.execution_contract.rejected` and similar topics,
            // shadowing the targeted recovery event for the source hat.
            // See docs/plans/2026-06-04-001-fix-contract-rejection-hat-retry-plan.md
            // (U3: Preserve Active Hat Selection Through Guidance Partitioning).
            if Self::is_system_event(event.topic.as_str()) {
                continue;
            }
            // Prefer direct event target over topic-based lookup
            let hat_id = if let Some(target) = &event.target
                && self.registry.get(target).is_some()
            {
                target.clone()
            } else if let Some(hat) = self.registry.get_for_topic(event.topic.as_str()) {
                hat.id.clone()
            } else {
                continue;
            };

            let list = if self.is_entrypoint_topic(event.topic.as_str()) {
                &mut entrypoint_hat_ids
            } else {
                &mut progressed_hat_ids
            };
            if !list.iter().any(|id| id == &hat_id) {
                list.push(hat_id);
            }
        }
        // Prefer progressed hats over entrypoint hats. Entrypoint events
        // (starting_event, task.start, task.resume) linger in the bus after
        // the first hat runs. Including them would re-activate the first hat
        // alongside downstream hats, confusing the agent with multiple hat
        // instructions when only the downstream hat should run.
        if progressed_hat_ids.is_empty() {
            entrypoint_hat_ids
        } else {
            progressed_hat_ids
        }
    }

    pub(super) fn effective_regular_events<'a>(&self, events: &'a [Event]) -> Vec<&'a Event> {
        let has_downstream_event = events.iter().any(|event| {
            !Self::is_system_event(event.topic.as_str())
                && !Self::is_kickoff_or_recovery_event(event.topic.as_str())
        });
        events
            .iter()
            .filter(|event| {
                // Also drop system/observability events from prompt context —
                // they are diagnostic, not actionable hat progress.
                !Self::is_system_event(event.topic.as_str())
                    && (!has_downstream_event
                        || !Self::is_kickoff_or_recovery_event(event.topic.as_str()))
            })
            .collect()
    }

    pub(super) fn is_kickoff_or_recovery_event(topic: &str) -> bool {
        topic == "task.start" || topic == "task.resume" || topic.strip_suffix(".start").is_some()
    }

    /// U3 (plan 2026-08-03-004): system-injected recovery-channel events
    /// (`task.resume` / `loop.resume`) must survive a hat's
    /// `event_filter` allowlist. The parallel-forge manifest-resume
    /// bootstrap (and every targeted recovery injection) re-binds the
    /// pending hat through this channel with the original trigger
    /// embedded in the payload; the allowlist only declares the hat's
    /// business trigger topics, so filtering the recovery payload would
    /// leave the resumed hat without its original trigger and the chain
    /// could not continue. The exemption is narrow: only
    /// runtime-injected (`system_injected == true`) recovery topics —
    /// hat-emitted events on these topics stay filtered as before.
    pub(super) fn is_recovery_channel_event(event: &Event) -> bool {
        event.system_injected == Some(true)
            && (event.topic.as_str() == ralph_proto::TASK_RESUME
                || event.topic.as_str() == ralph_proto::LOOP_RESUME)
    }

    /// Returns true for system/observability event topics that should not
    /// influence active hat selection or appear as actionable progress in
    /// the prompt (e.g. `event.execution_contract.rejected`,
    /// `event.malformed`, `event.scope_violation`). These are audit/diagnostic
    /// events, not hat routing signals.
    pub(super) fn is_system_event(topic: &str) -> bool {
        // 2026-06-28-005 plan U3: the previous addition of
        // `topic == "plan.blocked"` here was reverted because
        // it broke the legitimate hat-routing path in
        // `test_ce_executor_plan_blocked_routes_to_shipper_not_reporter`:
        // the ce-executor-serial preset has a `shipper` hat
        // with `triggers: ["plan.blocked"]`, and that test
        // expects the shipper to be the next active hat after
        // a real `plan.blocked` event. Marking the topic as a
        // system event short-circuits that routing.
        //
        // The original KTD-3 contract-reject concern was that
        // `plan.blocked` would shadow the targeted retry on
        // the source hat. That is handled separately by
        // publishing the targeted retry *before* the guidance
        // publish (see event_loop/mod.rs around the contract
        // reject site) and by keeping the publish `with_target`
        // on the guidance event itself. The system-event guard
        // is not required.
        topic.starts_with("event.")
    }

    pub(super) fn is_entrypoint_topic(&self, topic: &str) -> bool {
        topic == "task.start"
            || topic == "task.resume"
            || topic.strip_suffix(".start").is_some()
            || self.config.event_loop.starting_event.as_deref() == Some(topic)
    }

    pub(super) fn peek_pending_regular_events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        for hat_id in self.bus.hat_ids() {
            if let Some(pending) = self.bus.peek_pending(hat_id) {
                events.extend(pending.iter().cloned());
            }
        }
        events
    }

    /// Formats an event for prompt context.
    ///
    /// For top-level prompts (task.start, task.resume), wraps the payload in
    /// `<top-level-prompt>` XML tags to clearly delineate the user's original request.
    pub(super) fn format_event(event: &Event) -> String {
        let topic = &event.topic;
        let payload = &event.payload;

        if topic.as_str() == "task.start" || topic.as_str() == "task.resume" {
            format!(
                "Event: {} - <top-level-prompt>\n{}\n</top-level-prompt>",
                topic, payload
            )
        } else {
            format!("Event: {} - {}", topic, payload)
        }
    }

    pub(super) fn check_hat_exhaustion(
        &mut self,
        hat_id: &HatId,
        dropped: &[Event],
    ) -> (bool, Option<Event>) {
        let Some(config) = self.registry.get_config(hat_id) else {
            return (false, None);
        };
        let Some(max) = config.max_activations else {
            return (false, None);
        };

        let count = *self.state.hat_activation_counts.get(hat_id).unwrap_or(&0);
        if count < max {
            return (false, None);
        }

        // Emit only once per hat per run (avoid flooding).
        let should_emit = self.state.exhausted_hats.insert(hat_id.clone());

        if !should_emit {
            // Hat is already exhausted - drop pending events silently.
            return (true, None);
        }

        let mut dropped_topics: Vec<String> = dropped.iter().map(|e| e.topic.to_string()).collect();
        dropped_topics.sort();

        let payload = format!(
            "Hat '{hat}' exhausted.\n- max_activations: {max}\n- activations: {count}\n- dropped_topics:\n  - {topics}",
            hat = hat_id.as_str(),
            max = max,
            count = count,
            topics = dropped_topics.join("\n  - ")
        );

        warn!(
            hat = %hat_id.as_str(),
            max_activations = max,
            activations = count,
            "Hat exhausted (max_activations reached)"
        );

        (
            true,
            Some(Event::new(
                format!("{}.exhausted", hat_id.as_str()),
                payload,
            )),
        )
    }

    pub(super) fn record_hat_activations(&mut self, active_hat_ids: &[HatId]) {
        for hat_id in active_hat_ids {
            *self
                .state
                .hat_activation_counts
                .entry(hat_id.clone())
                .or_insert(0) += 1;
        }
    }

    /// Returns the primary active hat ID for display purposes.
    /// Returns the first active hat, or "ralph" if no specific hat is active.
    /// BTreeMap iteration is already sorted by key.
    pub fn get_active_hat_id(&self) -> HatId {
        let pending_events = self.peek_pending_regular_events();
        if let Some(active_hat_id) = self
            .determine_active_hat_ids(&pending_events)
            .into_iter()
            .next()
        {
            return active_hat_id;
        }
        HatId::new("ralph")
    }

    /// Injects a default event for a hat when the agent wrote no events.
    ///
    /// Call this after `process_events_from_jsonl` returns `Ok(false)` (no events found).
    /// If the hat has `default_publishes` configured, this injects the default event.
    ///
    /// If the default topic matches the completion promise, `completion_requested` is set
    /// so the loop can terminate. Without this, completion events injected via
    /// `default_publishes` would only be published to the bus (triggering downstream hats)
    /// but never detected by `check_completion_event`, causing an infinite loop.
    ///
    /// **U3 P0 fix (post-review)**: in `execution_mode: isolated`, this path
    /// runs *outside* `process_events_from_jsonl`'s scope enforcement, so we
    /// must mirror the same two gates that path enforces for JSONL events:
    ///
    /// 1. **Publish scope gate** — `default_topic` must be in the hat's
    ///    `publishes` list. If not, drop the injection and emit
    ///    `{hat}.scope_violation` to keep `default_publishes` from being a
    ///    back door around the U3 can_publish check.
    /// 2. **Per-turn single-event budget** — the default_publishes injection
    ///    counts as a business event for the current turn. Set
    ///    `first_business_event_accepted` so a subsequent JSONL business
    ///    event in the same turn hits `event.isolation.boundary_violation`
    ///    (and vice versa: if a JSONL business event was already accepted
    ///    this turn, drop the default_publishes injection and emit
    ///    `event.isolation.boundary_violation`).
    ///
    /// Coordinator mode is unchanged: there is no per-turn budget, and the
    /// `ralph` pseudo-hat's `RALPH_CONTROL_TOPICS` allowlist (in
    /// `event_origin.rs`) still governs what the runtime fallback hat may
    /// publish.
    pub fn check_default_publishes(&mut self, hat_id: &HatId) {
        let Some(config) = self.registry.get_config(hat_id) else {
            return;
        };
        let Some(default_topic) = config.default_publishes.as_ref() else {
            return;
        };
        let default_topic = default_topic.clone();
        let default_topic_str = default_topic.as_str();

        // U3 P0 fix — Gate 1: publish scope.
        // In isolated mode, the current hat's `publishes` list is the
        // authoritative scope; `default_publishes` must be a subset of it.
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && !self.registry.can_publish(hat_id, default_topic_str)
        {
            warn!(
                hat = %hat_id.as_str(),
                topic = %default_topic_str,
                "Isolated mode: default_publishes not declared in hat scope — dropping injection"
            );
            let violation_topic = format!("{}.scope_violation", hat_id.as_str());
            let violation_payload = format!(
                "Isolated mode: hat '{}' cannot publish default topic '{}' (not in publishes)",
                hat_id.as_str(),
                default_topic_str
            );
            self.bus
                .publish(Event::new(violation_topic, violation_payload));
            return;
        }

        // U3 P0 fix — Gate 2: per-turn single-event budget coordination.
        // If a JSONL business event was already accepted in this turn
        // (isolated_turn_business_event_accepted is sticky across
        // process_events and check_default_publishes), dropping the default
        // injection prevents two business events from being accepted in one
        // turn.
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && self.state.isolated_turn_business_event_accepted
            && !crate::event_origin::is_orchestrator_control_topic(
                default_topic_str,
                self.config.event_loop.cancellation_promise.as_str(),
            )
        {
            warn!(
                hat = %hat_id.as_str(),
                topic = %default_topic_str,
                "Isolated mode: default_publishes would exceed per-turn business-event budget — dropping"
            );
            let diagnostic = Event::new(
                "event.isolation.boundary_violation",
                format!(
                    "Isolated mode: default_publishes '{}' on hat '{}' dropped — one business event already accepted this turn",
                    default_topic_str,
                    hat_id.as_str()
                ),
            );
            self.bus.publish(diagnostic);
            return;
        }

        let payload = serde_json::json!({
            "reason": "default_publishes",
            "message": format!(
                "Hat '{}' emitted no events; orchestrator injected default topic '{}'",
                hat_id.as_str(),
                default_topic_str
            ),
            "hat": hat_id.as_str(),
            "topic": default_topic_str,
        });
        let default_event = Event::new(default_topic_str, payload.to_string())
            .with_source(hat_id.clone())
            .with_system_injected();
        let verdict_topics = self.verdict_gate_topics();
        let verdict_topics_slice = verdict_topics.as_deref();
        self.state
            .record_verdict_if_match(&default_event, verdict_topics_slice);
        self.state.record_completion_predecessor_if_match(
            &default_event,
            self.config.event_loop.completion_payload_match.as_ref(),
        );

        debug!(
            hat = %hat_id.as_str(),
            topic = %default_topic_str,
            "No events written by hat, injecting default_publishes event"
        );

        self.state.record_event(&default_event);

        // U3 P0 fix — claim the per-turn business-event budget slot when we
        // actually inject (so a subsequent JSONL business event in the same
        // turn will be rejected by the boundary check).
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && !crate::event_origin::is_orchestrator_control_topic(
                default_topic_str,
                self.config.event_loop.cancellation_promise.as_str(),
            )
        {
            self.state.isolated_turn_business_event_accepted = true;
        }

        // If the default topic is the completion promise, set the flag directly.
        // The normal path (process_events_from_jsonl) sets this when reading from
        // JSONL, but default_publishes bypasses JSONL entirely.
        if default_topic_str == self.config.event_loop.completion_promise
            && !self.state.completion_honored
        {
            info!(
                hat = %hat_id.as_str(),
                topic = %default_topic_str,
                "default_publishes matches completion_promise — requesting termination"
            );
            // P0-5: gate default_publishes' terminal signal on
            // `required_events`.
            if let Err(reason) = self.state.mark_completion_requested(
                &self.config.event_loop.required_events,
                &self.config.event_loop.completion_promise,
            ) {
                tracing::warn!(
                    reason = %reason,
                    hat = %hat_id.as_str(),
                    topic = %default_topic_str,
                    iteration = self.state.iteration,
                    "P0-5: default_publishes completion rejected; \
                     required events not yet observed; \
                     hat's default emit will not transition loop to terminal"
                );
                // Fall through: still publish the default
                // event so the agent can continue running;
                // the terminal transition just does not fire.
            } else {
                // P1-2: per-event commit (see `commit_terminal_delta`).
                Self::commit_terminal_delta(
                    &mut self.state.state_ledger,
                    crate::state::CommitDelta::CompletionRequested,
                );
            }
        }

        self.persist_system_injected_jsonl_event(hat_id, default_topic_str, &payload);

        let reason_code = "default_publishes_injected";
        let hat_str = hat_id.as_str();
        let mut env_builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::MissingEventGate)
            .severity(crate::diagnosis::DiagnosisSeverity::Warning)
            .iteration(self.state.iteration)
            .topic(default_topic_str)
            .source_hat(hat_str)
            .reason_code(reason_code)
            .message(format!(
                "Hat '{hat_str}' emitted no events; orchestrator injected default_publishes topic '{default_topic_str}'"
            ))
            .expected_action(format!(
                "Hat '{hat_str}' should emit '{default_topic_str}' before the turn ends; this injection is a synthetic fallback"
            ))
            .outcome(crate::diagnosis::DiagnosisOutcome::Pending)
            .retry_key(
                crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                    crate::diagnosis::DiagnosisSource::MissingEventGate,
                    Some(hat_str),
                    Some(default_topic_str),
                    reason_code,
                    None,
                ),
            );
        if let Some(session_id) = self.diagnostics.session_id() {
            env_builder = env_builder.session_id(session_id);
        }
        let envelope = env_builder.build();
        self.record_recovery_envelope(
            &envelope,
            vec![format!("default_publishes:{default_topic_str}")],
        );

        self.bus.publish(default_event);
    }

    /// P0-3 (2026-07-02-005): persist orchestrator-injected
    /// `default_publishes` events to the trusted events JSONL so
    /// operators can audit why a downstream hat was activated.
    ///
    /// The event is also published on the bus for immediate routing.
    /// The JSONL copy is marked `system_injected: true` and the reader
    /// position is advanced past it so the next
    /// `process_events_from_jsonl` pass does not double-publish.
    ///
    /// 2026-07-03-001 supervisor real-wiring: this method is `pub`
    /// because `ralph-cli`'s dispatcher calls it after a supervisor
    /// `tick` returns `InjectedComplete` / `InjectedFailed` to write
    /// the `*.wave.complete` / `*.wave.failed` coordination event and
    /// advance the reader cursor. The BDD scenarios in
    /// `ralph-core/tests/scenarios.rs` also call it from
    /// `run_bdd_supervisor_fan_in`.
    pub fn persist_system_injected_jsonl_event(
        &mut self,
        hat_id: &HatId,
        topic: &str,
        payload: &serde_json::Value,
    ) {
        let events_path = self.event_reader.path().to_path_buf();
        if let Some(parent) = events_path.parent()
            && !parent.as_os_str().is_empty()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(
                path = %events_path.display(),
                error = %err,
                "P0-3: failed to create events directory for default_publishes audit write"
            );
            return;
        }

        let ts = chrono::Utc::now().to_rfc3339();
        let record = serde_json::json!({
            "topic": topic,
            "payload": payload,
            "ts": ts,
            "hat": hat_id.as_str(),
            "source": hat_id.as_str(),
            "system_injected": true,
        });

        let append_result = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)?;
            let line = serde_json::to_string(&record)?;
            writeln!(file, "{line}")?;
            file.flush()?;
            Ok(())
        })();

        match append_result {
            Ok(()) => {
                if let Ok(metadata) = std::fs::metadata(&events_path) {
                    self.event_reader.set_position(metadata.len());
                }
                debug!(
                    hat = %hat_id.as_str(),
                    topic = %topic,
                    path = %events_path.display(),
                    "P0-3: persisted default_publishes event to JSONL for audit"
                );
            }
            Err(err) => {
                tracing::warn!(
                    hat = %hat_id.as_str(),
                    topic = %topic,
                    path = %events_path.display(),
                    error = %err,
                    "P0-3: failed to persist default_publishes event to JSONL; continuing with bus publish only"
                );
            }
        }
    }

    /// Returns a mutable reference to the event bus for direct event publishing.
    ///
    /// This is primarily used for planning sessions to inject user responses
    /// as events into the orchestration loop.
    pub fn bus(&mut self) -> &mut EventBus {
        &mut self.bus
    }

    /// Processes output from a hat execution.
    ///
    /// Returns the termination reason if the loop should stop.
    ///
    /// 2026-06-23-005 F4 (P0-2 重定位): `process_output` still
    /// consumes the legacy `consecutive_failures >= 5` termination
    /// path. The plan (`2026-06-23-005` U3 / KTD-7) envisioned a
    /// single-match `TerminationTrigger` dispatch, but the
    /// prerequisite (`pending_dead_letter` field + `LoopState`
    /// persistence) does not exist in the current codebase. F4
    /// therefore leaves `process_output` untouched and only
    /// documents the boundary. See
    /// `event_loop::termination` module-level docs for the
    /// full reasoning. The `LoopState::push_termination_trigger` /
    /// `pop_termination_trigger` APIs added in F4 are
    /// infrastructure-only — no caller enqueues triggers yet.
    pub fn process_output(
        &mut self,
        hat_id: &HatId,
        output: &str,
        success: bool,
    ) -> Option<TerminationReason> {
        self.state.iteration += 1;
        self.state.last_hat = Some(hat_id.clone());

        // WRC-U4 (2026-06-12-003 / KTD-13 / hook 3): drain
        // handoff deadlines that exceeded their dispatch window
        // since the last iteration. Each escalation is converted
        // into a `task.resume` event routed to the safe target
        // (plan-gate or review-coordinator — see
        // `HandoffTracker::expired`). The recovery envelope is
        // written by the existing `RecoveryResponder` via the
        // `event.isolation.boundary_violation` path, which already
        // handles envelope writing and dedup. We do **not** log a
        // recovery envelope here directly to keep the tracker
        // side-effect-free: the runner's `process_events_from_jsonl`
        // sees the synthesized `task.resume` event on the next
        // pass and routes it through the normal recovery flow.
        //
        // Coordinator mode is a no-op because the HandoffIndex
        // returns `None` for every consumer lookup there; the
        // tracker's `pending` map stays empty.
        let escalations = self
            .state
            .handoff_tracker
            .expired(std::time::Instant::now());
        for esc in escalations {
            warn!(
                topic = %esc.topic,
                consumer = %esc.consumer,
                event_id = %esc.event_id,
                safe_target = %esc.safe_target,
                "handoff dispatch timeout: routing task.resume to {}",
                esc.safe_target,
            );
            // Synthesize the resume event into the bus so the
            // dispatcher can route it on the next iteration. The
            // event's `target` is the safe_target so it bypasses
            // normal subscription matching and is delivered directly
            // to that hat; `source` is the orchestrator (`ralph`) so
            // the `EventOriginGuard` accepts the publish. The payload
            // carries the full escalation metadata for the downstream
            // hat to act on.
            // U2 (2026-06-17-003 plan): the JSON payload already
            // includes `reason`; add `target_hat` so the drift
            // detector counts it as schema-compliant.
            // P1-2 (plan 2026-06-29-006): route the synthesis
            // through `enrich_task_resume_payload_full` so the
            // `kind` field is populated explicitly. Previously this
            // path built an inline JSON payload that missed the
            // `kind` field, which the drift detector saw as 0/N
            // (`task.resume.kind` 1/5 in primary-172725).
            let message = format!(
                "handoff deadline exceeded: consumer '{}' did not activate within timeout",
                esc.consumer
            );
            let payload_str = crate::event_loop::rejection::enrich_task_resume_payload_full(
                &message,
                "handoff_dispatch_timeout",
                Some(esc.safe_target.as_str()),
                // `MissingEvent` is the closest existing
                // RejectionStage variant for a "consumer did not
                // emit within window" handoff stall; the drift
                // detector already special-cases it (see
                // `rejection::RejectionStage::MissingEvent`).
                Some(crate::event_loop::rejection::RejectionStage::MissingEvent),
                // Pass `None` so `enrich_task_resume_payload_full`
                // falls back to `reason_hint` ("handoff_dispatch_timeout")
                // for the `kind` field. The typed
                // `RejectionKind::StallNoEvents` would also work
                // but it would force drift to bucket these
                // escalations as `stall_no_events`, which is a
                // different (loop-wide) class. Keeping the kind
                // = reason preserves the original drift semantics
                // for the handoff path while still satisfying
                // the `kind` field presence requirement.
                None,
                // `allowed_topics` is reserved for the rejection
                // pipeline that knows the target hat's published
                // topic set; the handoff escalation path doesn't
                // carry that context, so we leave the list empty
                // (the enrich helper skips the field entirely in
                // that case).
                &[],
            );
            // The legacy inline JSON also carried
            // `topic` / `consumer` / `event_id` / `safe_target` /
            // `details` so downstream hats can correlate the
            // envelope. The enrich helper only knows the common
            // schema, so we re-parse and merge those fields back in
            // before publishing.
            let mut payload: serde_json::Value = serde_json::from_str(&payload_str)
                .unwrap_or_else(|e| panic!("enrich payload must be valid JSON: {e}"));
            if let serde_json::Value::Object(ref mut map) = payload {
                // Override `kind` with the literal `reason_hint`.
                // `enrich_task_resume_payload_full` falls back to
                // the violation_class of `reason_hint`, which is
                // "other" for `handoff_dispatch_timeout`. The
                // drift detector's `task.resume.kind` field
                // presence check only requires the field to be
                // non-empty; using the literal reason here
                // matches the value the downstream hat / drift
                // detector will see in the `reason` field.
                map.insert(
                    "kind".into(),
                    serde_json::Value::String("handoff_dispatch_timeout".into()),
                );
                map.insert("topic".into(), serde_json::Value::String(esc.topic.clone()));
                map.insert(
                    "consumer".into(),
                    serde_json::Value::String(esc.consumer.clone()),
                );
                map.insert(
                    "event_id".into(),
                    serde_json::Value::String(esc.event_id.clone()),
                );
                map.insert(
                    "safe_target".into(),
                    serde_json::Value::String(esc.safe_target.clone()),
                );
                map.insert(
                    "details".into(),
                    serde_json::Value::String(esc.reason.clone()),
                );
            }
            // Plan 2026-08-10-001 U1: route the handoff-resume
            // through the unified publisher so the registry /
            // dedup / fail-close checks fire. The caller-side
            // `retry_key` is signed by consumer + event_id so
            // multiple handoff timeouts collapse into one
            // resume per consumer/event.
            let loop_id_for_resume = self.current_loop_id();
            let decision = crate::event_loop::resume_routing::publish_targeted_resume_for_hat(
                &mut self.bus,
                &self.registry,
                None,
                loop_id_for_resume.as_deref(),
                esc.safe_target.as_str(),
                None,
                None,
                &format!("handoff:{}:{}", esc.consumer, esc.event_id),
                payload.to_string(),
            );
            if let crate::event_loop::resume_routing::ResumeDecision::Block { reason } = &decision {
                tracing::warn!(
                    target = %esc.safe_target.as_str(),
                    consumer = %esc.consumer,
                    event_id = %esc.event_id,
                    ?reason,
                    "handoff resume blocked (no safe target)"
                );
            }
            // P2-1 (plan 2026-06-29-006): bump the
            // consumer's cumulative stall count. When the
            // post-bump value reaches 2, publish a
            // `loop.stalled` business event so the
            // `progress-steward` hat (which subscribes to
            // `loop.stalled` in the ce-executor-serial preset)
            // can step in and rescue the loop. Without this
            // signal, the loop just keeps routing `task.resume`
            // to the stalled hat indefinitely.
            let stall_count = self
                .state
                .handoff_tracker
                .bump_consumer_stall_count(&esc.consumer);
            if stall_count >= 2 && self.config.event_loop.progress_steward.enabled {
                // 2026-07-06 plan U12: when `progress_steward.enabled`
                // is `false`, the runtime MUST NOT publish
                // `loop.stalled` wake events. The ce-executor-serial
                // preset (U10/U11) removed the `progress-steward`
                // hat and set this flag to `false`; publishing
                // `loop.stalled` here would target a non-existent
                // hat (the bus would silently drop it) and surface
                // as a phantom-recovery drift. The fail-close
                // contract is: `enabled==false` ⇒ no
                // `loop.stalled` wake from any code path.
                let stalled_payload = serde_json::json!({
                    "reason": "consumer_stall_repeat",
                    "consumer": esc.consumer,
                    "topic": esc.topic,
                    "stall_count": stall_count,
                    "retry_key": format!(
                        "stall_recovery:{}:{}:handoff_dispatch_timeout:*",
                        esc.consumer, esc.topic
                    ),
                });
                let stalled_event = Event::new("loop.stalled", stalled_payload.to_string())
                    .with_source(HatId::from("ralph"));
                self.bus.publish(stalled_event);
            }
            // 2026-06-13-004 U7 (P2-4): write a recovery envelope
            // for the handoff escalation so the responder can
            // surface this stall in the next prompt. The bus
            // `task.resume` event above is the visible-to-agent
            // signal; the envelope is the diagnose / journal
            // surface. KTD-5 locks the source to `StallRecovery`
            // and the outcome to `Escalated`. The two streams
            // (bus + journal) are kept in lockstep so operators
            // can correlate them in `ralph diagnose` and the
            // orchestration log.
            let reason_code = "handoff_dispatch_timeout";
            let env_source_hat = esc.consumer.clone();
            let env_target_hat = esc.safe_target.clone();
            let env_topic = esc.topic.clone();
            // Unit 5 / Unit 7 R-C2 (2026-06-17-001 plan): when the
            // escalation targets a wave-related hat, attach the
            // current flow record (wave_id / wave_total /
            // received_count / flow_phase) to the envelope so the
            // diagnose reporter can reconstruct the wave's
            // timeline.  This is informational only: the existing
            // handoff escalation path stays unchanged for non-wave
            // handoffs (R5, payload contract, etc.).
            let mut flow_context: Option<serde_json::Value> = None;
            if Self::is_wave_hat(&HatId::new(&esc.consumer)) {
                if let Some(record) = self.state.flow_lifecycle.get(&esc.event_id) {
                    flow_context = Some(serde_json::json!({
                        "wave_id": record.flow_unit_id,
                        "wave_total": record.wave_total,
                        "received_count": record.received_count,
                        "flow_phase": record.phase.as_str(),
                    }));
                } else {
                    // No record keyed by event_id — fall back to a
                    // record whose target_hat matches the consumer,
                    // picking the most recently transitioned one.
                    // This keeps the envelope useful when the
                    // event_id naming diverges (e.g. legacy `sla:*`
                    // keys) while remaining deterministic.
                    let candidates: Vec<&crate::flow_lifecycle::FlowLifecycleRecord> = self
                        .state
                        .flow_lifecycle
                        .active_records()
                        .filter(|r| r.target_hat == esc.consumer)
                        .collect();
                    if let Some(active) =
                        candidates.into_iter().max_by_key(|r| r.last_transition_at)
                    {
                        flow_context = Some(serde_json::json!({
                            "wave_id": active.flow_unit_id,
                            "wave_total": active.wave_total,
                            "received_count": active.received_count,
                            "flow_phase": active.phase.as_str(),
                        }));
                    }
                }
            }
            let mut env_builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
                .source(crate::diagnosis::DiagnosisSource::StallRecovery)
                .severity(crate::diagnosis::DiagnosisSeverity::Warning)
                .iteration(self.state.iteration)
                .topic(env_topic.clone())
                .source_hat(&env_source_hat)
                .target_hat(&env_target_hat)
                .reason_code(reason_code)
                .message(format!(
                    "handoff deadline exceeded: consumer '{}' did not activate within timeout",
                    env_source_hat
                ))
                .expected_action(format!(
                    "Consumer hat '{}' must activate before the next iteration. \
                     A `task.resume` has been routed to the safe target '{}' \
                     to keep the loop moving.",
                    env_source_hat, env_target_hat
                ))
                .safe_target(true)
                .outcome(crate::diagnosis::DiagnosisOutcome::Escalated)
                .evidence(crate::diagnosis::EvidenceRef {
                    kind: crate::diagnosis::EvidenceKind::Topic,
                    ref_path: env_topic.clone(),
                    snippet: Some(format!("event_id={} details={}", esc.event_id, esc.reason)),
                })
                .retry_key(
                    crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                        crate::diagnosis::DiagnosisSource::StallRecovery,
                        Some(&env_source_hat),
                        Some(&env_topic),
                        reason_code,
                        None,
                    ),
                );
            if let Some(ctx) = flow_context.as_ref() {
                env_builder = env_builder.evidence(crate::diagnosis::EvidenceRef {
                    kind: crate::diagnosis::EvidenceKind::Field,
                    ref_path: "flow.context".to_string(),
                    snippet: Some(ctx.to_string()),
                });
            }
            if let Some(session_id) = self.diagnostics().session_id() {
                env_builder = env_builder.session_id(session_id);
            }
            let envelope = env_builder.build();
            self.record_recovery_envelope(
                &envelope,
                vec![format!(
                    "handoff_escalation consumer={} topic={} event_id={} safe_target={}",
                    env_source_hat, env_topic, esc.event_id, env_target_hat
                )],
            );
        }

        // 2026-06-28-003: run runtime-recovery detectors after recording
        // any StallRecovery envelopes. This publishes loop.stalled when
        // the stall path forgot to do so and forces flapping keys to
        // plan.blocked.
        let ctx = self.runtime_recovery_context(&[]);
        self.apply_runtime_recovery_actions(&ctx);

        // U2 (2026-06-17-003 plan): per-iteration incomplete-wave scan.
        // Run after handoff escalations and before processing new JSONL events
        // so a stalled wave can be closed by the mechanism before the active hat
        // tries to bypass with empty_diff. When the gate is disabled (default)
        // this is a cheap no-op.
        let _ = self.maybe_emit_incomplete_wave_blocked();

        // Track the isolated hat for scope enforcement in process_parse_result
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated {
            self.state.current_isolated_hat = Some(hat_id.clone());
        } else {
            self.state.current_isolated_hat = None;
        }
        // U3 P0 fix: reset the per-turn business-event budget at every turn
        // boundary so `check_default_publishes` and `process_parse_result`
        // see a consistent view of "what has been accepted this turn".
        self.state.isolated_turn_business_event_accepted = false;

        // Log iteration started
        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::IterationStarted,
        );

        // Log hat selected
        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::HatSelected {
                hat: hat_id.to_string(),
                reason: "process_output".to_string(),
            },
        );

        // Track failures
        if success {
            self.state.consecutive_failures = 0;
        } else {
            self.state.consecutive_failures += 1;
        }

        let _ = output;

        // File-modification audit: detect when a hat with disallowed Edit/Write tools
        // modified files. This is hard enforcement — emits a scope_violation event.
        self.audit_file_modifications(hat_id);

        // R3 (2026-06-14-003 plan): ephemeral file isolation.  When the
        // preset opts in via `event_loop.ephemeral_isolation: true` and
        // the loop is in isolated mode, scan the workspace for
        // runtime artefacts (`scratchpad.md`, `tmp*.md`, `*.bak`) that
        // landed in source trees and relocate them to
        // `.ralph/agent/scratchpad-{loop_id}.md`.  The records are
        // saved on `LoopState` so the next `build_prompt` can include
        // a `## EPHEMERAL RELOCATED` block.  The engine is best-
        // effort — a git failure, a read-only FS, or an unrecognised
        // layout does not interrupt the loop.
        self.run_ephemeral_isolation();

        // Events are ONLY read from the JSONL file written by `ralph emit`.
        // This enforces tool use and prevents confabulation (agent claiming to emit without actually doing so).
        // See process_events_from_jsonl() for event processing.

        // Check termination conditions
        self.check_termination()
    }

    /// Audits file modifications after a hat iteration.
    ///
    /// If the hat has `Edit` or `Write` in its `disallowed_tools`, checks whether
    /// files were modified (via `git diff --stat HEAD`). If so, emits a
    /// `<hat_id>.scope_violation` event AND promotes the finding to
    /// `AuditSeverity::Fail { add_failures: 1 }` per
    /// `2026-06-23-005` U4 (R5+KTD-8). This is the first audit class
    /// promoted from Warn to Fail — drift_monitor's 3 alert classes
    /// stay at Warn (U9 follow-up).
    pub(super) fn audit_file_modifications(&mut self, hat_id: &HatId) {
        let config = match self.registry.get_config(hat_id) {
            Some(c) => c,
            None => return,
        };

        let has_write_restriction = config
            .disallowed_tools
            .iter()
            .any(|t| t == "Edit" || t == "Write");

        if !has_write_restriction {
            return;
        }

        let workspace = &self.config.core.workspace_root;
        let diff_output = std::process::Command::new("git")
            .args(["diff", "--stat", "HEAD"])
            .current_dir(workspace)
            .output();

        match diff_output {
            Ok(output) if !output.stdout.is_empty() => {
                let diff_stat = String::from_utf8_lossy(&output.stdout).trim().to_string();
                warn!(
                    hat = %hat_id.as_str(),
                    diff = %diff_stat,
                    "Hat modified files despite tool restrictions (scope violation)"
                );

                let violation_topic = format!("{}.scope_violation", hat_id.as_str());
                let violation = Event::new(
                    violation_topic.as_str(),
                    format!(
                        "Hat '{}' modified files with Edit/Write disallowed:\n{}",
                        hat_id.as_str(),
                        diff_stat
                    ),
                );
                self.bus.publish(violation);

                // Scope violations from read-only dimension reviewers are
                // promoted from the legacy `add_failures: 1` counting path to
                // a typed hard reject. This covers both the historical
                // `dimension-reviewer` hat and split dimension hats (`dim:*`)
                // that explicitly disallow Edit/Write.
                //
                // Other hats still route through `Fail { add_failures: 1 }`
                // because their scope_violation can be a legitimate fix
                // attempt (coordinator writing plan files, executor
                // committing code).
                //
                // The BlockLoop arm does NOT increment
                // `consecutive_failures` (orthogonal termination
                // mechanism); instead it pushes a typed
                // `TerminationTrigger::DeadLetter` which
                // `check_termination` converts to
                // `TerminationReason::ScopeViolationHardRejected`
                // on the next call.
                let is_read_only_dimension_reviewer = hat_id.as_str() == "dimension-reviewer"
                    || (hat_id.as_str().starts_with("dim:")
                        && config
                            .disallowed_tools
                            .iter()
                            .any(|tool| matches!(tool.as_str(), "Edit" | "Write")));
                let severity = if is_read_only_dimension_reviewer {
                    crate::event_loop::audit::AuditSeverity::BlockLoop {
                        reason: "scope_violation".to_string(),
                    }
                } else {
                    crate::event_loop::audit::AuditSeverity::Fail { add_failures: 1 }
                };
                let kind = if is_read_only_dimension_reviewer {
                    crate::preset::engine::gates::RejectionKind::ScopeViolation
                } else {
                    // Pre-U5 placeholder retained for non-read-only-reviewer
                    // hats so the audit chain stays backwards-compatible.
                    crate::preset::engine::gates::RejectionKind::MissingField
                };
                crate::event_loop::audit::AuditDispatcher::dispatch(
                    severity,
                    crate::event_loop::audit::AuditContext {
                        hat: hat_id.as_str().to_string(),
                        kind,
                        details: diff_stat.clone(),
                    },
                    &mut self.state.consecutive_failures,
                );

                // Push the typed termination trigger so
                // `check_termination` produces the matching
                // `TerminationReason::ScopeViolationHardRejected`.
                // Only for read-only dimension reviewers (the BlockLoop arm).
                // The trigger carries the hat + diff stat so
                // `trigger_to_reason` produces a fully-populated
                // `TerminationReason` without further enrichment.
                if is_read_only_dimension_reviewer
                    && let Err(e) = self.state.push_termination_trigger(
                        crate::event_loop::termination::TerminationTrigger::ScopeViolation {
                            hat: hat_id.as_str().to_string(),
                            diff_stat: diff_stat.clone(),
                        },
                    )
                {
                    warn!(
                        error = %e,
                        "scope_violation_hard_rejected: failed to push termination trigger"
                    );
                }
            }
            Err(e) => {
                debug!(error = %e, "Could not run git diff for file-modification audit");
            }
            _ => {} // No modifications — all good
        }
    }

    /// Extracts task identifier from build.blocked payload.
    /// Uses first line of payload as task ID.
    pub(super) fn extract_task_id(payload: &str) -> String {
        payload
            .lines()
            .next()
            .unwrap_or("unknown")
            .trim()
            .to_string()
    }

    /// Adds cost to the cumulative total.
    pub fn add_cost(&mut self, cost: f64) {
        self.state.cumulative_cost += cost;
    }

    /// Verifies all tasks in scratchpad are complete or cancelled.
    ///
    /// Returns:
    /// - `Ok(true)` if all tasks are `[x]` or `[~]`, or if scratchpad is disabled
    /// - `Ok(false)` if any tasks are `[ ]` (pending)
    /// - `Err(...)` if scratchpad doesn't exist or can't be read
    pub(super) fn verify_scratchpad_complete(&self) -> Result<bool, std::io::Error> {
        // Nothing to verify when scratchpad is disabled
        if !self.ralph.active_scratchpad().enabled {
            return Ok(true);
        }

        let scratchpad_path = self.scratchpad_path();

        if !scratchpad_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Scratchpad does not exist",
            ));
        }

        let content = std::fs::read_to_string(scratchpad_path)?;

        let has_pending = content
            .lines()
            .any(|line| line.trim_start().starts_with("- [ ]"));

        Ok(!has_pending)
    }

    /// Reads the current loop ID from the marker file.
    ///
    /// Returns `None` if no marker exists or is empty, which means
    /// task queries should be unfiltered (backwards compatible).
    pub(super) fn current_loop_id(&self) -> Option<String> {
        self.loop_context
            .as_ref()
            .and_then(|ctx| {
                let marker_path = ctx.ralph_dir().join("current-loop-id");
                std::fs::read_to_string(&marker_path).ok()
            })
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
    }
}
