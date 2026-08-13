//! EventLoop implementation region 3.

use super::*;

impl EventLoop {
    /// Returns the configuration.
    pub fn config(&self) -> &RalphConfig {
        &self.config
    }

    /// Returns the hat registry.
    pub fn registry(&self) -> &HatRegistry {
        &self.registry
    }

    /// Returns a mutable reference to the hat registry.
    pub fn registry_mut(&mut self) -> &mut HatRegistry {
        &mut self.registry
    }

    /// Returns true when the given `hat` is permitted to publish the given
    /// `topic` under the registry's publish rules.
    ///
    /// This is the shared isolated-scope predicate used by both the
    /// regular event path (`process_parse_result`) and the wave partition
    /// path (`process_events_from_jsonl_with_waves`). Centralising the
    /// call here keeps the two paths in lock-step when scope rules change
    /// — see U4 plan §4 KTD-U4-1 / A2.
    pub fn isolated_publish_allowed(&self, hat: &HatId, topic: &str) -> bool {
        self.registry.can_publish(hat, topic)
    }

    /// 2026-07-03-005 plan (P0 fix M-1): check whether the given
    /// isolated-mode hat has declared the given topic as exempt from
    /// the per-turn single-business-event budget. Returns false when
    /// the hat is not registered, has no `HatConfig`, or its
    /// `exempt_topics` list does not contain `topic`. The caller uses
    /// this to admit declared serial walks (e.g. review-coordinator
    /// walking 6 `review.dimension.ready` events) without consuming
    /// the `non_wave_business_event_accepted` slot.
    pub fn isolated_exempt_topic(&self, hat: &HatId, topic: &str) -> bool {
        let (business, terminal) = self
            .config
            .event_loop
            .event_policy
            .as_ref()
            .map(|ep| (ep.business_topics.as_slice(), ep.terminal_topics.as_slice()))
            .unwrap_or((&[], &[]));
        is_isolated_exempt_topic(self.registry.get_config(hat), topic, business, terminal)
    }

    /// 2026-07-04-001 plan U16 (KTD-13): validate that a `task.resume`
    /// injection's consumer hat actually subscribes to the original
    /// topic via `HandoffIndex::consumer_of`. If the resolved consumer
    /// exists but its `triggers` does not include `original_topic`,
    /// the resume would never have a chance of being consumed —
    /// injecting it would silently stall for the full stall
    /// Validate that a `task.resume` event is being routed to the hat
    /// that will actually pick it up. The single argument form returns
    /// an [`EventLoopResumeDecision`]; callers in the recovery /
    /// diagnostic loops should branch on `Block` so the resume is not
    /// silently published to a hat that will ignore it.
    ///
    /// Plan ref: U16 of
    /// `docs/plans/2026-07-04-002-fix-opac-p0-p1-p2-issues-plan.md`
    /// (P0 #3 fix). The previous implementation returned
    /// `Option<String>` which the call sites collapsed into a `warn!`
    /// — `task.resume` events therefore still flowed to hats that did
    /// not subscribe, leading to silent stall. This decision variant
    /// gives the call sites a hard "block / allow" signal that feeds
    /// the same diagnostic pipeline as other recovery blocks.
    ///
    /// The fallback no-events branch (when `original_topic` is `None`)
    /// is preserved as `Allow` so we don't regress the no-events
    /// inject path that operators rely on during partial outages.
    pub fn validate_resume_routing(
        &self,
        target_hat: &HatId,
        original_topic: Option<&str>,
    ) -> EventLoopResumeDecision {
        let Some(topic) = original_topic else {
            // Fallback no-events inject path — we have no original
            // topic, so route by the registered consumer-of
            // `task.resume` (the `HandoffIndex` consumer fallback).
            return EventLoopResumeDecision::Allow;
        };
        let Some(consumer) = self.handoff_index.consumer_of(topic) else {
            // No registered consumer: this is the existing
            // "no upstream subscription" warning shape — we keep it
            // as a Block so callers can opt-out, but the message is
            // deliberately generic to avoid leaking preset topology
            // into a diagnostic event.
            return EventLoopResumeDecision::Block(format!(
                "U16: no HandoffIndex consumer found for original trigger topic `{}`; task.resume would not be picked up",
                topic
            ));
        };
        if consumer != target_hat.as_str() {
            return EventLoopResumeDecision::Block(format!(
                "U16: resume target hat `{}` is not the HandoffIndex consumer of `{}` (consumer is `{}`); resume will not be picked up",
                target_hat.as_str(),
                topic,
                consumer
            ));
        }
        // Confirm the consumer's `triggers` declares the topic. The
        // registry's `get_config(...).triggers` is the SSOT for what
        // a hat subscribes to (alias of `subscribes_to`); if the
        // topic is missing the hat's prompt will never see the
        // upstream event, so a resume is also wasted.
        //
        // U8 / U6 of plan 2026-07-05-005: the inline
        // `triggers.iter().any(...)` loop is replaced with a call
        // to the shared `check_hat_triggers` helper. **Only this
        // path** uses the helper today; `next_hat` filters by
        // `event.target == Some(id)` (a different predicate — the
        // publisher named a specific hat, not a topic), and
        // `process_output` handoff escalation at line 4406 uses
        // literal `t == e.topic.as_str()` matching (Topic::matches
        // is glob-aware; mixing the two would silently change
        // routing for any hat whose `triggers` contains a glob).
        // See fix-plan §U6 option (a): keep the divergence
        // documented rather than wiring `process_output` through
        // the helper.
        if let Some(cfg) = self.registry.get_config(&HatId::from(consumer))
            && let Err(_err) =
                crate::workflow_contract::handoff_index::check_hat_triggers(&cfg.triggers, topic)
        {
            return EventLoopResumeDecision::Block(format!(
                "U16: resume target hat `{}` does not declare `{}` in its `triggers` list; resume will not be picked up",
                consumer, topic
            ));
        }
        EventLoopResumeDecision::Allow
    }

    /// Enforce isolated publish scope on a batch of wave events.
    ///
    /// Groups events by `wave_id` (preserving first-seen order), then:
    ///   * the first distinct `wave_id` is allowed only if every event
    ///     in the group is in the isolated hat's `publishes` list — if
    ///     not, the whole group is dropped as
    ///     `WaveRejection::IsolatedScopeViolation`;
    ///   * any subsequent distinct `wave_id` is dropped as
    ///     `WaveRejection::IsolatedMultipleBusinessEmissions`.
    ///
    /// Each rejection publishes a `*.scope_violation` event to the bus
    /// and constructs a `WaveRejection` value so that the caller's
    /// B2 responder path can wire it to `record_recovery_envelope`.
    ///
    /// See U4 plan §3 KTD-U4-1, §3 KTD-U4-2, §4 A3.
    pub(super) fn enforce_wave_isolated_scope(
        &mut self,
        events: Vec<crate::event_reader::Event>,
        isolated_hat: &HatId,
    ) -> std::io::Result<Vec<crate::event_reader::Event>> {
        use crate::wave_detection::WaveRejection;
        use std::collections::HashMap;

        // DEBUG: 添加入口日志
        let input_event_count = events.len();
        tracing::debug!(
            isolated_hat = %isolated_hat.as_str(),
            input_event_count = input_event_count,
            "enforce_wave_isolated_scope entry"
        );

        // Group by wave_id, preserving first-seen order. Wave counts
        // per read batch are bounded by `max_wave_total` (default 64),
        // so a Vec is fine for the order book; HashMap gives O(1) lookup.
        let mut order: Vec<String> = Vec::with_capacity(events.len());
        let mut groups: HashMap<String, Vec<crate::event_reader::Event>> = HashMap::new();
        for event in events {
            let key = event.wave_id.clone().unwrap_or_default();
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(event);
        }

        // DEBUG: 记录分组结果
        tracing::debug!(
            wave_groups = order.len(),
            total_events = input_event_count,
            "wave grouping result"
        );

        let mut kept: Vec<crate::event_reader::Event> = Vec::new();
        // Tracks whether ANY distinct `wave_id` has been observed in
        // this read batch, regardless of whether that wave was kept or
        // dropped. KTD-U4-2: a single isolated activation allows at
        // most one distinct `wave_id`; any further distinct wave_id is
        // typed as `IsolatedMultipleBusinessEmissions`, even if the
        // first wave itself was rejected for scope.
        let mut wave_observed: bool = false;

        for wave_id in order {
            let group = groups.remove(&wave_id).unwrap_or_default();
            if group.is_empty() {
                continue;
            }

            if wave_observed {
                // Subsequent distinct wave_id in the same read batch:
                // typed as `IsolatedMultipleBusinessEmissions`.
                let rejection = WaveRejection::IsolatedMultipleBusinessEmissions {
                    wave_id: wave_id.clone(),
                    isolated_hat: isolated_hat.to_string(),
                };
                self.publish_isolated_wave_violation(&rejection, isolated_hat, &group);
            } else {
                // First distinct wave: check isolated scope on every
                // event. If any event is out of scope, the whole wave
                // is dropped (one business emission rule). The wave
                // is still considered "observed" so the next distinct
                // wave_id is typed as `IsolatedMultipleBusinessEmissions`
                // — a second wave is never silently absorbed by the
                // scope check.
                if let Some(out_of_scope_topic) = group.iter().find_map(|e| {
                    // DEBUG: 添加调试日志追踪每个事件的 scope 检查
                    let allowed = self.isolated_publish_allowed(isolated_hat, e.topic.as_str());
                    tracing::debug!(
                        wave_id = %wave_id,
                        event_hat = ?e.hat.as_deref(),
                        topic = %e.topic,
                        allowed = %allowed,
                        "isolated scope check for wave event"
                    );
                    if allowed { None } else { Some(e.topic.clone()) }
                }) {
                    let rejection = WaveRejection::IsolatedScopeViolation {
                        wave_id: wave_id.clone(),
                        topic: out_of_scope_topic,
                        isolated_hat: isolated_hat.to_string(),
                    };
                    self.publish_isolated_wave_violation(&rejection, isolated_hat, &group);
                    wave_observed = true;
                    continue;
                }
                wave_observed = true;
                kept.extend(group);
            }
        }

        Ok(kept)
    }

    /// Publish a `.scope_violation` diagnostic event and log a warning
    /// for an isolated wave rejection. The typed `WaveRejection` is
    /// recorded as a recovery finding in B2; for now this method only
    /// handles the diagnostic side so that A1–A3 land atomically.
    pub(super) fn publish_isolated_wave_violation(
        &mut self,
        rejection: &crate::wave_detection::WaveRejection,
        isolated_hat: &HatId,
        events: &[crate::event_reader::Event],
    ) {
        use crate::wave_detection::WaveRejection;
        let (reason_code, topic_label, wave_id) = match rejection {
            WaveRejection::IsolatedScopeViolation { wave_id, topic, .. } => (
                "wave_isolated_scope_violation",
                topic.as_str(),
                wave_id.as_str(),
            ),
            WaveRejection::IsolatedMultipleBusinessEmissions { wave_id, .. } => (
                "wave_isolated_multiple_business_emissions",
                "",
                wave_id.as_str(),
            ),
            _ => ("wave_isolated_unknown", "", ""),
        };
        warn!(
            hat = %isolated_hat.as_str(),
            reason = reason_code,
            wave = wave_id,
            dropped = events.len(),
            "Isolated wave rejection — dropping whole wave"
        );
        let violation_topic = format!("{}.scope_violation", isolated_hat.as_str());
        let violation_payload = format!(
            "Isolated mode wave rejection ({reason_code}): hat '{}' dropped {} wave event(s) {}",
            isolated_hat.as_str(),
            events.len(),
            if topic_label.is_empty() {
                String::new()
            } else {
                format!("(out-of-scope topic '{topic_label}')")
            }
        );
        self.bus
            .publish(Event::new(violation_topic, violation_payload));

        // B2 / KTD-U4-4: record a recovery envelope so the responder
        // can track the finding. Outcome is `NotRetriable` per plan §3
        // KTD-U4-5 table — cap/structure and isolated-scope rejections
        // do not enter automatic recovery escalation.
        let retry_key = crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(
            wave_id,
            reason_code,
        );
        let message = format!(
            "Isolated wave {} rejected: hat '{}' cannot publish '{}'; {} event(s) dropped",
            wave_id,
            isolated_hat.as_str(),
            if topic_label.is_empty() {
                "(multi-business)"
            } else {
                topic_label
            },
            events.len(),
        );
        let mut builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::WaveDispatcher)
            .severity(crate::diagnosis::DiagnosisSeverity::Error)
            .iteration(self.state.iteration)
            .reason_code(reason_code)
            .message(message)
            .retry_attempt(0)
            .safe_target(false)
            .outcome(crate::diagnosis::DiagnosisOutcome::NotRetriable)
            .retry_key(retry_key)
            .source_hat(isolated_hat.to_string());
        if !topic_label.is_empty() {
            builder = builder.topic(topic_label.to_string());
        }
        if let Some(session_id) = self.diagnostics.session_id() {
            builder = builder.session_id(session_id);
        }
        let envelope = builder.build();
        self.record_recovery_envelope(&envelope, Vec::new());
    }

    /// Records hook telemetry for diagnostics.
    pub fn log_hook_run_telemetry(&self, entry: crate::diagnostics::HookRunTelemetryEntry) {
        self.diagnostics.log_hook_run(entry);
    }

    /// Logs the full prompt for an iteration to the diagnostics session.
    pub fn log_prompt(&self, iteration: u32, hat: &str, prompt: &str) {
        self.diagnostics.log_prompt(iteration, hat, prompt);
    }

    /// Gets the backend configuration for a hat.
    ///
    /// If the hat has a backend configured, returns that.
    /// Otherwise, returns None (caller should use global backend).
    pub fn get_hat_backend(&self, hat_id: &HatId) -> Option<&HatBackend> {
        self.registry
            .get_config(hat_id)
            .and_then(|config| config.backend.as_ref())
    }

    /// Adds an observer that receives all published events.
    ///
    /// Multiple observers can be added (e.g., session recorder + TUI).
    /// Each observer is called before events are routed to subscribers.
    pub fn add_observer<F>(&mut self, observer: F)
    where
        F: Fn(&Event) + Send + 'static,
    {
        self.bus.add_observer(observer);
    }

    /// Sets a single observer, clearing any existing observers.
    ///
    /// Prefer `add_observer` when multiple observers are needed.
    #[deprecated(since = "2.0.0", note = "Use add_observer instead")]
    pub fn set_observer<F>(&mut self, observer: F)
    where
        F: Fn(&Event) + Send + 'static,
    {
        #[allow(deprecated)]
        self.bus.set_observer(observer);
    }

    /// Checks if any termination condition is met.
    pub fn check_termination(&mut self) -> Option<TerminationReason> {
        let cfg = &self.config.event_loop;

        if self.state.iteration >= cfg.max_iterations {
            return Some(TerminationReason::MaxIterations);
        }

        if self.state.elapsed().as_secs() >= cfg.max_runtime_seconds {
            return Some(TerminationReason::MaxRuntime);
        }

        if let Some(max_cost) = cfg.max_cost_usd
            && self.state.cumulative_cost >= max_cost
        {
            return Some(TerminationReason::MaxCost);
        }

        if self.state.consecutive_failures >= cfg.max_consecutive_failures {
            return Some(TerminationReason::ConsecutiveFailures);
        }

        // Check for loop thrashing: planner keeps dispatching abandoned tasks
        if self.state.abandoned_task_redispatches >= 3 {
            return Some(TerminationReason::LoopThrashing);
        }

        // Check for validation failures: too many consecutive malformed JSONL lines
        if self.state.consecutive_malformed_events >= 3 {
            return Some(TerminationReason::ValidationFailure);
        }

        // Check for hard-gate exhaustion: agent repeatedly claims emit but never writes
        if self.state.consecutive_hard_gates >= Self::HARD_GATE_MAX {
            warn!(
                count = self.state.consecutive_hard_gates,
                "Hard gate exhausted: agent repeatedly claimed to emit events but never wrote them"
            );
            return Some(TerminationReason::Stopped);
        }

        // Check for stale loop: same event signature emitted 3+ times in a row
        if self.state.consecutive_same_signature >= 3 {
            let topic = self
                .state
                .last_emitted_signature
                .as_ref()
                .map(|signature| signature.topic.as_str())
                .unwrap_or("?");
            warn!(
                topic,
                count = self.state.consecutive_same_signature,
                "Stale loop detected: same event signature emitted consecutively"
            );
            return Some(TerminationReason::LoopStale);
        }

        // P0-C (2026-06-10): fail-path auto-termination via the
        // `verdict_gate.fail` chain — REMOVED in U9
        // (2026-06-27-002 plan completion). The legacy
        // `additional_topics: ["report.done"]` mirror is
        // retired; only `LOOP_COMPLETE` terminates the
        // dispatcher (see U10). A failing verdict is
        // still recorded in `last_verdict_topic` /
        // `last_verdict_payload` and surfaced via the
        // `verdict_failed` recovery envelope, but the
        // loop does NOT auto-terminate on its own.

        // 2026-06-14-004 U2: isolated-scope circuit breaker check.
        // If the rejection branch tripped the breaker, the original
        // (non-normalized) termination reason is stored in LoopState.
        // This path does not depend on telemetry.runtime_diagnosis.
        if let Some(reason) = self.state.scope_violation_circuit_breaker_tripped.take() {
            if let TerminationReason::ScopeViolationCircuitBreakerTripped {
                ref hat,
                ref topic,
                violation_count,
                ..
            } = reason
            {
                warn!(
                    hat = %hat,
                    topic = %topic,
                    violation_count = violation_count,
                    "Scope violation circuit breaker tripped: terminating loop"
                );
            }
            return Some(reason);
        }

        // U5 (plan 2026-07-04-004): drain the typed termination
        // trigger queue for hard-reject triggers pushed by the
        // audit chain (e.g. dimension-reviewer scope_violation).
        // The legacy `process_output` consumer is still TODO per
        // F4 docs; we read the queue here so the U5 hard-reject
        // shape is observable without waiting for the F4 single-
        // match dispatch migration. The trigger converts to a
        // typed `TerminationReason::ScopeViolationHardRejected`
        // (or `PayloadContractViolation` for non-ScopeViolation
        // kinds) via `trigger_to_reason`.
        if let Some(trigger) = self.state.pop_termination_trigger() {
            let reason = crate::event_loop::termination::trigger_to_reason(trigger);
            return Some(reason);
        }

        // Check for stop signal from .ralph/stop-requested (written by `ralph loops stop`
        // or external tooling — the Telegram /stop producer was removed with `ralph-telegram`
        // in the 2026-06-25 refactor; the signal-file mechanism survives)
        let stop_path =
            std::path::Path::new(&self.config.core.workspace_root).join(".ralph/stop-requested");
        if stop_path.exists() {
            let _ = std::fs::remove_file(&stop_path);
            return Some(TerminationReason::Stopped);
        }

        // Check for restart signal from external tooling (e.g. `ralph loops stop`)
        let restart_path =
            std::path::Path::new(&self.config.core.workspace_root).join(".ralph/restart-requested");
        if restart_path.exists() {
            return Some(TerminationReason::RestartRequested);
        }

        // Check if workspace directory has been removed (zombie worktree detection)
        if !std::path::Path::new(&self.config.core.workspace_root).is_dir() {
            return Some(TerminationReason::WorkspaceGone);
        }

        None
    }

    /// Check if a loop.cancel event was detected.
    ///
    /// Unlike check_completion_event(), this does NOT validate required_events.
    /// Cancellation is an explicit abort — it doesn't need the workflow to be complete.
    pub fn check_cancellation_event(&mut self) -> Option<TerminationReason> {
        if !self.state.cancellation_requested {
            return None;
        }
        self.state.cancellation_requested = false;
        info!("Loop cancelled gracefully via loop.cancel event");

        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::LoopTerminated {
                reason: "cancelled".to_string(),
            },
        );

        Some(TerminationReason::Cancelled)
    }

    /// Request completion from the text fallback path.
    ///
    /// When a backend outputs a completion promise as plain text (without
    /// using `ralph emit`), this sets `completion_requested = true` so that
    /// `check_completion_event()` can apply all safety checks (persistent mode,
    /// required events, runtime tasks) before terminating.
    ///
    /// 2026-06-30-001 P0-5: gated by `report_done_seen`. A
    /// text-fallback completion promise that arrives before
    /// `report.done` is logged at `warn!` and rejected; the loop
    /// continues to wait for the workflow's final report before
    /// transitioning to terminal.
    pub fn request_completion_from_text_fallback(&mut self) {
        if self.state.completion_honored {
            debug!("Completion already handled, ignoring text fallback request");
            return;
        }
        // P0-5: required_events gate.
        if let Err(reason) = self.state.mark_completion_requested(
            &self.config.event_loop.required_events,
            &self.config.event_loop.completion_promise,
        ) {
            tracing::warn!(
                reason = %reason,
                iteration = self.state.iteration,
                "P0-5: text-fallback completion rejected; \
                 required events not yet observed; loop continues"
            );
            self.state.completion_requested = true;
            return;
        }
        // P1-2: per-event commit so a mid-flight crash preserves
        // the completion signal for replay. The A1 end-of-batch
        // hook used to commit this; moving to the decision point
        // shrinks the window where a crash loses the signal.
        Self::commit_terminal_delta(
            &mut self.state.state_ledger,
            crate::state::CommitDelta::CompletionRequested,
        );
        info!("Completion requested via text fallback (output contained completion promise)");
    }

    /// Per-event commit helper for terminal markers
    /// (`CompletionRequested`, `CompletionHonored`,
    /// `CancellationRequested`).
    ///
    /// P1-2 (P1 follow-up): the A1 end-of-batch hook used to
    /// commit these. Moving to the decision point shrinks the
    /// window where a mid-flight crash loses the termination
    /// signal — `replay_from_disk` will see the flag set on
    /// cold start and honor the termination instead of
    /// re-running the batch.
    ///
    /// No-op when the ledger is not enabled (legacy mode) or
    /// the commit itself fails (the loop is still in
    /// termination mode; ledger error is logged and the batch
    /// continues). Per-event scalar `CounterChanged { Iteration }`
    /// stays end-of-batch — that signal is per-iteration, not
    /// per-decision.
    ///
    /// Takes `&mut Option<StateLedger>` (not `&mut self`) so
    /// the caller can keep an immutable borrow of
    /// `self.config.event_loop.event_policy` (or any other
    /// immutable field) alive in the same scope. The helper
    /// only touches the ledger slot; nothing else on `self`.
    pub(super) fn commit_terminal_delta(
        ledger_slot: &mut Option<crate::state::StateLedger>,
        delta: crate::state::CommitDelta,
    ) {
        let Some(ledger) = ledger_slot else {
            return;
        };
        let topic = match &delta {
            crate::state::CommitDelta::CompletionRequested => "loop.completion_requested",
            crate::state::CommitDelta::CompletionHonored => "loop.completion_honored",
            crate::state::CommitDelta::CancellationRequested => "loop.cancellation_requested",
            _ => "loop.terminal",
        };
        if let Err(e) = ledger.commit(delta, Some(topic.to_string())) {
            tracing::warn!(
                error = %e,
                topic,
                "P1-2: per-event terminal commit failed; loop continues"
            );
        }
    }

    /// Checks if a completion event was received and returns termination reason.
    ///
    /// Completion is accepted via JSONL events (e.g., `ralph emit`) or via
    /// [`request_completion_from_text_fallback`].
    pub fn check_completion_event(&mut self) -> Option<TerminationReason> {
        // Idempotency: if we already handled completion, return the same conclusion
        if self.state.completion_honored {
            return Some(TerminationReason::CompletionPromise);
        }

        if !self.state.completion_requested {
            return None;
        }

        // Event chain validation: check required events were seen
        let required = self.config.event_loop.required_events.clone();
        if !required.is_empty() {
            let missing = self.state.missing_required_events(&required);
            if !missing.is_empty() {
                warn!(
                    missing = ?missing,
                    "Rejecting LOOP_COMPLETE: required events not seen during loop lifetime"
                );
                let sig = format!(
                    "missing_required:{}",
                    missing
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                );
                if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                    return Some(reason);
                }
                self.state.completion_requested = false;

                // U11-T8 / P0-2 (2026-06-23-003 plan): deterministic
                // correction.  Replaces the legacy `task.resume`
                // injection so the rejection signal flows through
                // `PromptContext` (single source for the next prompt)
                // instead of the EventBus back-channel.
                let free_form = format!(
                    "LOOP_COMPLETE rejected: missing required events: {:?}. \
                     The agent must complete all workflow phases before emitting LOOP_COMPLETE. \
                     Use loop.cancel to abort the workflow instead.",
                    missing
                );
                if let Some(stuck) = Self::inject_completion_correction(
                    &mut self.state,
                    "missing_required_events",
                    &free_form,
                ) {
                    return Some(stuck);
                }
                return None;
            }
        }

        let state_machine_enabled = self
            .config
            .event_loop
            .state_machine
            .as_ref()
            .is_some_and(|sm| sm.enabled);

        // Verdict gate: when configured, the most recent event matching the gate
        // topic must NOT carry fail_field == fail_value. This prevents a hat from
        // declaring success in its final review while bypassing the backstop check.
        //
        // 2026-06-17-002 U6: also check the upstream verdict payload
        // (`gate.topic` itself, e.g. `REVIEW_COMPLETE`) independently of
        // downstream mirrors. A fake pass on `report.done` must not hide
        // an upstream fail.
        if let Some(gate) = self.config.event_loop.verdict_gate.clone() {
            let upstream_fail = self
                .state
                .last_upstream_verdict_payload
                .as_deref()
                .is_some_and(|p| Self::verdict_payload_is_fail(p, &gate));
            let mirror_fail = self
                .state
                .last_verdict_payload
                .as_deref()
                .is_some_and(|p| Self::verdict_payload_is_fail(p, &gate));
            if upstream_fail || mirror_fail {
                warn!(
                    topic = %gate.topic,
                    field = %gate.fail_field,
                    value = %gate.fail_value,
                    upstream_fail,
                    mirror_fail,
                    "Rejecting LOOP_COMPLETE: verdict gate observed a failing verdict"
                );
                let sig = format!("verdict_fail:{}", gate.topic);
                if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                    return Some(reason);
                }
                self.state.completion_requested = false;

                // U11-T8 / P0-2 (2026-06-23-003 plan): deterministic
                // 2026-06-26 plan U6: structural rejection — do
                // NOT inject a correction block. The agent cannot
                // change the verdict (it is already published) and
                // injecting a correction would just spend the
                // recoverable budget on a failure mode that is not
                // recoverable. Surface the stuck signal so the
                // operator sees the loop end with a clear reason.
                return Some(TerminationReason::CompletionStuck(Box::new(
                    crate::event_loop::types::CompletionStuck {
                        source: crate::event_loop::types::StuckSource::StructuralRejection,
                        retry_key: format!("verdict_fail:{}", gate.topic),
                        attempts: 1,
                        last_reason: format!(
                            "verdict fail on {topic} ({field}={value})",
                            topic = gate.topic,
                            field = gate.fail_field,
                            value = gate.fail_value,
                        ),
                    },
                )));
            }
        }

        // Completion payload match gate: when configured, the completion
        // payload must carry the same top-level field values as the most
        // recent accepted predecessor event on the configured topic.
        if let Some(match_cfg) = self.config.event_loop.completion_payload_match.clone()
            && let Some((predecessor_topic, predecessor_payload)) =
                self.state.last_completion_predecessor.clone()
        {
            let completion_payload = self
                .state
                .last_completion_payload
                .as_deref()
                .unwrap_or("{}");
            let mismatch = Self::completion_payload_mismatch(
                &match_cfg,
                &predecessor_payload,
                completion_payload,
            );
            if let Some(reason) = mismatch {
                warn!(
                    topic = %predecessor_topic,
                    reason = %reason,
                    "Rejecting LOOP_COMPLETE: completion payload mismatch"
                );
                let sig = format!("completion_payload_mismatch:{predecessor_topic}");
                if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                    return Some(reason);
                }
                self.state.completion_requested = false;

                let free_form = format!(
                    "LOOP_COMPLETE rejected: payload mismatch on {topic} ({reason}). \
                     The completion payload must carry the same field values as the \
                     most recent accepted {topic} event. Re-emit with matching values \
                     or use loop.cancel to abort.",
                    topic = predecessor_topic,
                    reason = reason,
                );
                if let Some(stuck) = Self::inject_completion_correction(
                    &mut self.state,
                    "completion_payload_mismatch",
                    &free_form,
                ) {
                    return Some(stuck);
                }
                return None;
            }
        }

        // Workflow guard completion validation: ensure all started guarded instances are terminal.
        // State-machine configs use their instance lifecycle as the completion source of truth.
        if !state_machine_enabled
            && let Some(guards) = &self.config.event_loop.workflow_guards
            && !guards.chains.is_empty()
            && let Some(rejection) = self.check_workflow_guard_completion(guards)
        {
            warn!(
                reason = %rejection.message,
                "Rejecting LOOP_COMPLETE: incomplete workflow guard instances"
            );
            // Build a stable signature from the rejection message to detect same-guard rejections
            let sig = format!("workflow_guard:{}", rejection.message);
            if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                return Some(reason);
            }
            self.state.completion_requested = false;

            let free_form = format!(
                "LOOP_COMPLETE rejected: {}. \
                 All workflow instances must reach a terminal phase before emitting LOOP_COMPLETE. \
                 Use loop.cancel to abort the workflow instead.",
                rejection.message
            );
            // U11-T8 / P0-2 (2026-06-23-003 plan): deterministic
            // correction.  Replaces the legacy `task.resume`
            // injection.
            if let Some(stuck) = Self::inject_completion_correction(
                &mut self.state,
                "workflow_guard_incomplete",
                &free_form,
            ) {
                return Some(stuck);
            }
            return None;
        }

        self.state.completion_requested = false;

        // In persistent mode, suppress completion and keep the loop alive
        if self.config.event_loop.persistent {
            info!("Completion event suppressed - persistent mode active, loop staying alive");

            self.diagnostics.log_orchestration(
                self.state.iteration,
                "loop",
                crate::diagnostics::OrchestrationEvent::LoopTerminated {
                    reason: "completion_event_suppressed_persistent".to_string(),
                },
            );

            // Inject a task.resume event so the loop continues with an idle prompt
            // U2 (2026-06-17-003 plan): wrap the free-form message in
            // a JSON object carrying the schema-required
            // `reason` and `target_hat` fields.
            // 2026-06-23-005 F2: carry the typed `PersistentLoopActive`
            // kind so the schema validator / recovery aggregator
            // see the typed completion-suppression signal.
            //
            // Plan 2026-08-10-001 U1: route through the unified
            // publisher. The persistent-mode idle-continuation
            // targets `state.pending_recovery_hat` (set by the
            // caller before this point) when present, else
            // falls back to `ralph` (the dispatcher hat
            // subscribed to `.completed.*`). The `retry_key`
            // distinguishes persistent-idle resumes from any
            // other targeted recovery.
            let persistent_target = self
                .state
                .pending_recovery_hat
                .clone()
                .unwrap_or_else(|| HatId::new("ralph"));
            let persistent_payload = enrich_task_resume_payload(
                "Persistent mode: loop staying alive after completion signal. \
                 Check for new tasks or await human guidance.",
                "persistent mode",
                Some(persistent_target.as_str()),
                Some(RejectionKind::PersistentLoopActive),
            );
            let loop_id_for_resume = self.current_loop_id();
            crate::event_loop::resume_routing::publish_targeted_resume_for_hat(
                &mut self.bus,
                &self.registry,
                None,
                loop_id_for_resume.as_deref(),
                persistent_target.as_str(),
                None,
                None,
                "persistent_idle",
                persistent_payload,
            );

            return None;
        }

        // Runtime tasks are the canonical queue when memories/tasks mode is enabled.
        if self.config.memories.enabled {
            if let Ok(false) = self.verify_tasks_complete() {
                let open_tasks = self.get_open_task_list();
                warn!(
                    open_tasks = ?open_tasks,
                    "Rejecting completion event with {} open task(s)",
                    open_tasks.len()
                );
                // Build a stable signature from sorted task IDs to detect same-set rejections
                let mut task_ids: Vec<&str> = open_tasks
                    .iter()
                    .filter_map(|t| t.split(':').next())
                    .collect();
                task_ids.sort_unstable();
                let task_ids_hash = {
                    use std::hash::{DefaultHasher, Hash, Hasher};
                    let mut h = DefaultHasher::new();
                    for id in &task_ids {
                        id.hash(&mut h);
                    }
                    h.finish()
                };
                let sig = format!("open_tasks:{}:{}", open_tasks.len(), task_ids_hash);
                if let Some(reason) = self.handle_completion_rejection(sig, self.count_tasks()) {
                    return Some(reason);
                }
                self.state.completion_requested = false;
                // U2 (2026-06-17-003 plan): wrap the free-form
                // message in a JSON object carrying the
                // schema-required `reason` and `target_hat` fields.
                // 2026-06-23-005 F2: carry the typed
                // `OpenTasksBlocking` kind so the schema validator
                // sees the completion-rejection signal.
                //
                // Plan 2026-08-10-001 U1: route through the
                // unified publisher. The completion-blocking site
                // has no hat context — only the orchestrator
                // (`ralph`) can dispatch the next unit, so target
                // it explicitly. The `retry_key` is signed by the
                // sorted task-id set so the same set of open
                // tasks collapses into a single resume; a
                // different set collapses into a different
                // resume.
                let open_tasks_payload = enrich_task_resume_payload(
                    &format!(
                        "Completion rejected: runtime tasks remain open: {:?}. \
                         Close, fail, or reopen outstanding tasks before \
                         emitting the completion promise.",
                        open_tasks
                    ),
                    "open tasks remain",
                    Some("ralph"),
                    Some(RejectionKind::OpenTasksBlocking),
                );
                let loop_id_for_resume = self.current_loop_id();
                crate::event_loop::resume_routing::publish_targeted_resume_for_hat(
                    &mut self.bus,
                    &self.registry,
                    None,
                    loop_id_for_resume.as_deref(),
                    "ralph",
                    None,
                    None,
                    &format!("open_tasks:{}:{}", open_tasks.len(), task_ids_hash),
                    open_tasks_payload,
                );
                return None;
            }
        } else if let Ok(false) = self.verify_scratchpad_complete() {
            warn!("Completion event with pending scratchpad tasks - trusting agent decision");
        }

        // Completion accepted — reset stale-breaker state.
        self.state.completion_rejection_signature = None;
        self.state.consecutive_completion_rejections = 0;
        self.state.last_rejection_fingerprint = 0;

        info!("Completion event detected - terminating");

        // Log loop terminated
        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::LoopTerminated {
                reason: "completion_event".to_string(),
            },
        );

        // P1-2: per-event commit (see `commit_terminal_delta`).
        if !self.state.completion_honored {
            Self::commit_terminal_delta(
                &mut self.state.state_ledger,
                crate::state::CommitDelta::CompletionHonored,
            );
        }
        self.state.completion_honored = true;

        if state_machine_enabled
            && let Some(ref mut sm_state) = self.state.state_machine_runtime_state
        {
            sm_state.mark_terminal_honored();
            // Plan GAP-02 / Unit 4: persist terminal-honored
            // delta so restart hydration can rebuild
            // `terminal_honored` on the next process. The
            // legacy completion-honored delta still ships so
            // non-Unit-4 callers keep behaving as before. The
            // semantic-delta-dedup path in Unit 1's
            // `apply_transition_delta` ensures this delta is
            // idempotent across replays of the same honored
            // state — we always carry a fresh
            // `transition_id` derived from `loop_id +
            // contract_id + topic = "state_machine.terminal"
            // + terminal_iteration`.
            let next_delta = crate::state::CommitDelta::StateMachineTransition {
                delta: crate::state_machine::StateMachineTransitionDelta {
                    transition_id: crate::state_machine::StateMachineTransitionId::build(
                        &self.current_loop_id_for_contract(),
                        Some("terminal-honored"),
                        "wave-scope",
                        "state_machine.terminal_honored",
                        None,
                        self.state.iteration as u64,
                    ),
                    topic: "state_machine.terminal_honored".to_string(),
                    instance_key: None,
                    new_state: "terminal_honored".to_string(),
                    opens_instance: false,
                    closes_instance: false,
                    terminal_observed: true,
                    terminal_honored: true,
                },
            };
            let _ = Self::commit_terminal_delta(
                &mut self.state.state_ledger,
                next_delta,
            );
        }

        // Record completion honored in policy runtime state for downstream guarding
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
            && let Some(ref mut policy_state) = self.state.policy_runtime_state
        {
            policy_state.completion_honored = true;
            // 2026-06-29-007 P0 fix: terminal_observed is set only when the
            // completion promise is actually honored, not when it is merely
            // seen and later rejected by required_events / verdict gate.
            policy_state.terminal_observed = true;
            policy_state.completion_topic = Some(self.config.event_loop.completion_promise.clone());
            policy_state.completion_iteration = Some(self.state.iteration);
        }

        Some(TerminationReason::CompletionPromise)
    }

    /// Tracks completion rejections for the stale-breaker mechanism.
    ///
    /// If the same rejection signature repeats 3+ times with no meaningful
    /// progress between rejections (business events, task state changes,
    /// workflow advancement, or state machine transitions), returns
    /// `TerminationReason::LoopStale` to prevent infinite API-burning loops.
    ///
    /// `task_snapshot` is `(open_count, closed_count)` from the task store.
    pub(super) fn handle_completion_rejection(
        &mut self,
        signature: String,
        task_snapshot: (usize, usize),
    ) -> Option<TerminationReason> {
        let mut fingerprint = self.state.compute_progress_fingerprint();
        fingerprint.task_snapshot = task_snapshot;
        let current_fp = fingerprint.hash();

        let is_same = self.state.completion_rejection_signature.as_ref() == Some(&signature);
        let has_progress = current_fp != self.state.last_rejection_fingerprint;

        if is_same && !has_progress {
            self.state.consecutive_completion_rejections += 1;
            if self.state.consecutive_completion_rejections >= 3 {
                warn!(
                    signature = %signature,
                    count = self.state.consecutive_completion_rejections,
                    "Stale-breaker: same completion rejection repeated 3+ times with no progress"
                );
                return Some(TerminationReason::LoopStale);
            }
        } else if is_same && has_progress {
            // Same rejection reason but progress was made — reset counter
            self.state.consecutive_completion_rejections = 1;
        } else {
            // Different rejection reason — reset counter
            self.state.consecutive_completion_rejections = 1;
        }

        self.state.completion_rejection_signature = Some(signature);
        self.state.last_rejection_fingerprint = current_fp;
        None
    }

    /// P0-2 (2026-06-23-003 plan): completion rejection no longer
    /// publishes a `task.resume` event.  Instead, we route the
    /// rejection through the deterministic-correction path so the
    /// next prompt builder renders a `## ORCHESTRATOR CORRECTION`
    /// block sourced from `state.prompt_context` (the U7a single
    /// source of truth for prompt-side rejection signals).
    ///
    /// The synthesised `Rejection` uses the `Policy` stage as the
    /// closest existing bucket.  The `reason_hint` is fed into the
    /// correction block verbatim so the next prompt keeps the same
    /// free-form text the legacy `task.resume` payload used to
    /// carry.  The per-key retry counter is read from the unified
    /// ledger so escalation (R11) tracks the same number the
    /// legacy wire-format path used.
    ///
    /// 2026-06-26 plan U6: returns
    /// `Some(TerminationReason::CompletionStuck)` when the retry
    /// budget for this `retry_key` is exhausted (>= 3). The caller
    /// must surface the stuck signal instead of looping again. The
    /// structural-rejection path
    /// (e.g. `verdict_fail` in `check_completion_event`) does NOT
    /// call this helper — it goes straight to
    /// `CompletionStuck(StructuralRejection)` so a structural
    /// failure never silently burns the recoverable budget.
    pub(super) fn inject_completion_correction(
        state: &mut LoopState,
        reason_hint: &str,
        free_form: &str,
    ) -> Option<TerminationReason> {
        let topic = ralph_proto::LOOP_COMPLETE.to_string();
        let mut rejection = crate::event_loop::rejection::Rejection {
            stage: crate::event_loop::rejection::RejectionStage::Policy,
            source_hat: None,
            business_hat: None,
            topic: topic.clone(),
            violation: free_form.to_string(),
            retry_key: String::new(),
            retry_eligible: true,
            non_retryable_reason: None,
            target_hat: None,
            original_event_id: None,
            original_ts: None,
            // 2026-06-23 fix plan U5 (CB-2): completion-correction
            // path predates typed-kind plumbing — keep None.
            kind: None,
            duplicate_work_done_hint: None,
            seen_count: None,
        };
        let retry_key = rejection.compute_retry_key();
        rejection.retry_key = retry_key.clone();

        // Read the per-key retry count from the unified ledger so
        // R11 escalation tracks the same number the legacy
        // `task.resume` payload used to ship on the wire.  Fall
        // back to 1 on cold start (no prior rejection recorded).
        let retry_count = state
            .state_ledger
            .as_ref()
            .and_then(|l| l.snapshot().rejection_digest().get(&retry_key))
            .map(|entry| entry.count)
            .unwrap_or(1u32);

        // 2026-06-26 plan U6: bounded recovery. After 3 attempts
        // for the same retry key, stop injecting corrections and
        // surface a `CompletionStuck(RejectionDigestExhausted)`
        // termination so the operator sees the loop end. The
        // budget matches `U2_REJECTION_RETRY_LIMIT` (3) so the
        // gate, the runner, and the summary report all use the
        // same number.
        if retry_count > U2_REJECTION_RETRY_LIMIT {
            return Some(TerminationReason::CompletionStuck(Box::new(
                crate::event_loop::types::CompletionStuck {
                    source: crate::event_loop::types::StuckSource::RejectionDigestExhausted,
                    retry_key: retry_key.clone(),
                    attempts: retry_count,
                    last_reason: format!("{reason_hint}: {free_form}"),
                },
            )));
        }

        // `emit_correction_context` is the U7a entry point: it
        // commits a `RejectionRecorded` delta to the unified ledger
        // (when wired up) and pushes the `CorrectionContext` into
        // `state.prompt_context` so the next `build_prompt` call
        // prepends the `## ORCHESTRATOR CORRECTION` block.  No
        // event is published on the bus — the prompt builder is
        // the single source of truth.
        let _ctx = crate::correction::emit_correction_context(
            state.state_ledger.as_mut(),
            &rejection,
            retry_count,
            None,
            &mut state.prompt_context,
        );

        // Surface the reason hint in tracing so operators can
        // correlate a `LOOP_COMPLETE` rejection with the
        // correction block queued in the next prompt.
        tracing::info!(
            retry_key = %retry_key,
            reason_hint = %reason_hint,
            topic = %topic,
            "P0-2: injected completion rejection into state.prompt_context (replaces task.resume)"
        );
        // 2026-06-26 plan U6: correction queued; budget not
        // exhausted yet — caller should keep the loop alive.
        None
    }
}
