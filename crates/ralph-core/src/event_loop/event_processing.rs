//! EventLoop implementation region 5.

use super::*;

impl EventLoop {
    /// Unit 8 (2026-06-17-001) + U3 (2026-06-17-003): Returns true if `hat`
    /// is `review-synthesizer` — the only consumer routed through the
    /// 3-step stall escalation ladder. `review-coordinator` and
    /// `dimension-reviewer` use their own `stall:<name>` bucket (U8
    /// invariant pinned by `test_u3_ladder_inert_for_non_wave_hats`).
    pub(super) fn is_wave_hat(hat: &HatId) -> bool {
        hat.as_str() == "review-synthesizer"
    }

    /// 2026-06-28 plan U6 (R6): drive the per-task
    /// `RepairStateMachine` from the stall hot path.
    ///
    /// The first escalation for a `task_key` lazily creates a
    /// machine with the preset's `mechanism.repair_budget`
    /// (defaulting to 3 when no flow is declared). Subsequent
    /// escalations call `Retry` and consume one unit of the
    /// budget. When the budget is exhausted, we emit a
    /// `plan.blocked` envelope with
    /// `reason="repair_unrecoverable_after_N_retries"` and
    /// return `true` so the caller skips the `task.resume`
    /// path.
    ///
    /// The machine is keyed by `task_key` (= `stall_key` from
    /// the caller); different keys have independent budgets.
    pub(super) fn drive_repair_state_machine(&mut self, task_key: &str, stall_count: u32) -> bool {
        use crate::event_loop::repair_flow::{
            RepairAction, RepairBudget, RepairStateMachine, RepairTransitionResult,
        };
        // Read the budget from the preset (U12 will lint this).
        // When no flow is declared we fall back to the
        // repository-wide default of 3.
        let max = self
            .config
            .event_loop
            .mechanism
            .as_ref()
            .and_then(|m| m.flow.as_ref())
            .map(|f| f.repair_budget)
            .unwrap_or(3);
        let budget = RepairBudget { max };
        let machine = self
            .repair_state_machines
            .entry(task_key.to_string())
            .or_insert_with(|| RepairStateMachine::new(budget));
        // First escalation: Detected -> Diagnosing. We use
        // the budget to gate the upgrade so a preset that
        // declared `repair_budget: 0` immediately exhausts.
        let result = if stall_count == 1 {
            machine.try_transition(RepairAction::BeginDiagnosis)
        } else {
            machine.try_transition(RepairAction::Retry)
        };
        match result {
            RepairTransitionResult::BudgetExhausted(exhausted) => {
                let payload = format!(
                    r#"{{"reason":"{}","task_key":"{}","retries_consumed":{},"budget":{}}}"#,
                    exhausted.reason_code, task_key, exhausted.retries_consumed, exhausted.max,
                );
                let blocked =
                    Event::new("plan.blocked", payload.clone()).with_target(HatId::new("ralph"));
                self.record_repair_event(&blocked);
                // 2026-06-29 code-review fix: set the
                // `terminal_event_emitted` flag so U8's
                // final-threshold path (also emitting
                // `plan.blocked`) does not fire a second
                // time for the same `stall_key`. Mirrors
                // U8's behaviour at line 2983.
                self.terminal_event_emitted = true;
                true
            }
            // Illegal transitions are expected when a previous
            // stall cycle Closed the machine — treat them as
            // no-ops, NOT as a budget-exhausted stop.
            RepairTransitionResult::IllegalTransition { .. } => false,
            RepairTransitionResult::Accepted => false,
        }
    }

    /// True when the last hat consumed a multi-consumer pass-through trigger
    /// and another registered consumer still has that topic pending — stall
    /// recovery must not inject targeted `task.resume` to the pass-through hat.
    pub(super) fn should_skip_stall_recovery_for_multi_consumer_peers(&self) -> bool {
        let Some(last_hat) = self.state.last_hat.as_ref() else {
            return false;
        };
        let Some(config) = self.registry.get_config(last_hat) else {
            return false;
        };
        let Some(pass_through_trigger) =
            self.state.last_activation_events.iter().find_map(|event| {
                let topic = event.topic.as_str();
                if config.triggers.iter().any(|t| t == topic)
                    && config.trigger_multi_consumer_topics.contains(topic)
                    && config.publishes.len() == 1
                    && config.publishes.iter().any(|p| p == topic)
                {
                    Some(topic.to_string())
                } else {
                    None
                }
            })
        else {
            return false;
        };
        self.bus.hat_ids().any(|id| {
            if id == last_hat {
                return false;
            }
            self.bus
                .peek_pending(id)
                .is_some_and(|q| q.iter().any(|e| e.topic.as_str() == pass_through_trigger))
        })
    }

    /// Injects a fallback event to recover from a stalled loop.
    ///
    /// When no hats have pending events (agent failed to publish), this method
    /// injects a `task.resume` event which Ralph will handle to attempt recovery.
    ///
    /// Returns true if a fallback event was injected, false if recovery is not possible.
    pub fn inject_fallback_event(&mut self) -> bool {
        if self.inject_review_aggregate_timeouts() {
            return true;
        }

        // Do not stall-recover after the loop has already reached terminal.
        if self.state.completion_honored {
            return false;
        }
        if self.state.completion_requested && self.check_completion_event().is_some() {
            return false;
        }

        // Pass-through multi-consumer hats (e.g. shipper on `plan.complete`) may
        // intentionally not re-emit; peer consumers still hold the same trigger.
        // Injecting targeted `task.resume` to the pass-through hat would pre-empt
        // round-robin and starve peers (reporter never sees `plan.complete`).
        if self.should_skip_stall_recovery_for_multi_consumer_peers() {
            return false;
        }

        const STALL_HARD_THRESHOLD: u32 = 3;
        // Unit 8 (2026-06-17-001): use a per-last-hat stall key so wave hats
        // accumulate their own retry budget separate from ralph's global counter.
        let stall_key = if let Some(last_hat) = &self.state.last_hat {
            if Self::is_wave_hat(last_hat) {
                "flow:review-synthesizer".to_string()
            } else {
                format!("stall:{}", last_hat.as_str())
            }
        } else {
            "stall:ralph".to_string()
        };

        let stall_count_value = *self
            .state
            .stall_recovery_counts
            .entry(stall_key.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);

        // 2026-06-28 plan U8 (R5): final-threshold self-stop.
        // Even when U6's `RepairStateMachine` did not consume
        // the budget (e.g. the budget was set high, or the
        // machine was reset by a Close), the per-key stall
        // counter must still emit a terminal `plan.blocked`
        // when it crosses `STALL_FINAL_THRESHOLD`. This is a
        // safety net: the loop's self-stop is the only
        // contract that survives a misconfigured preset.
        const STALL_FINAL_THRESHOLD: u32 = 10;
        if stall_count_value >= STALL_FINAL_THRESHOLD {
            if !self.terminal_event_emitted {
                let payload = format!(
                    r#"{{"reason":"stall_recovery_exhausted","task_key":"{}","stall_count":{}}}"#,
                    stall_key, stall_count_value,
                );
                let blocked =
                    Event::new("plan.blocked", payload.clone()).with_target(HatId::new("ralph"));
                self.record_repair_event(&blocked);
                self.terminal_event_emitted = true;
            }
            debug!(
                stall_count = stall_count_value,
                stall_key = %stall_key,
                "U8: stall_recovery final threshold reached — loop self-stops"
            );
            return true;
        }

        // 2026-06-28 plan U6 (R6): drive the per-task
        // `RepairStateMachine` from the stall hot path so
        // `repair_budget` becomes a real hard cap rather than
        // a metadata decoration. The first escalation
        // transitions the machine into `Diagnosing`; each
        // subsequent escalation calls `Retry`. When
        // `RepairStateMachine` reports `BudgetExhausted`, we
        // emit a `plan.blocked` envelope and short-circuit so
        // no `task.resume` is published — the loop's self-stop
        // path takes over.
        let budget_exhausted = self.drive_repair_state_machine(&stall_key, stall_count_value);
        if budget_exhausted {
            debug!(
                stall_count = stall_count_value,
                stall_key = %stall_key,
                "U6: repair_budget exhausted — emitting plan.blocked and skipping task.resume"
            );
            return true;
        }

        let hard_escalation = stall_count_value >= STALL_HARD_THRESHOLD;
        // Unit 8: wave stall escalation — route to review-coordinator when
        // a wave hat is the last to execute and it has stalled.
        let hard_target = if let Some(last_hat) = &self.state.last_hat {
            if Self::is_wave_hat(last_hat) {
                HatId::new("review-coordinator")
            } else {
                HatId::new("review-synthesizer")
            }
        } else {
            HatId::new("review-synthesizer")
        };

        // U3 (2026-06-17-003 plan) — Stall/handoff routing ladder
        // (R-F3, SC-F1): for wave hats, the 3rd consecutive stall
        // (hard_escalation == true) MUST escalate to the mechanism
        // layer (`maybe_emit_incomplete_wave_blocked`) instead of
        // routing to review-coordinator. The coordinator path was
        // what activated the `work.done → empty_diff` bypass in
        // zippy-sparrow (review-coordinator fired while a wave was
        // still open and tried to terminate with `review.passed`).
        // The ladder is:
        //   - count 1, 2: existing `task.resume` → review-synthesizer
        //     (lets the synthesizer try to close the wave normally)
        //   - count 3+: mechanism emits `plan.blocked` via U2
        //     staleness; we return early so no `task.resume` is
        //     published and no extra work is routed to executor.
        // Shares the `flow:review-synthesizer` bucket with 001-U8
        // (no double counter) — the existing threshold (3) is the
        // single source of truth.
        if hard_escalation
            && stall_key.starts_with("flow:")
            && self.maybe_emit_incomplete_wave_blocked()
        {
            debug!(
                stall_count = stall_count_value,
                stall_key = %stall_key,
                "U3: stall ladder reached hard threshold — mechanism emitted plan.blocked; \
                 NOT routing executor to re-emit work.done (empty_diff bypass closed)"
            );
            return true;
        }
        // If U2 had nothing to emit (no open waves / no
        // candidates), fall through to the legacy hard path
        // so the loop does not get stuck — this preserves the
        // pre-U3 behaviour for the edge case where the
        // stall counter has drifted past threshold but the
        // tracker has no open wave (e.g. ralph itself is the
        // last hat and `last_hat` is a wave hat from a prior
        // session).

        // If a custom hat was last executing, target the fallback back to it
        // This preserves hat context instead of always falling back to Ralph.
        //
        // Plan 2026-08-10-001 U1: the hard-escalation branch uses
        // the unified `publish_targeted_resume_for_hat` helper
        // which already publishes through the bus. The other two
        // branches (last-hat fallback, Ralph fallback) keep the
        // pre-U1 direct `Event::new(...)` shape because they need
        // subsequent metadata stamping / enrich payload that would
        // require an additional surface on the helper; they use
        // `Some(Event::new(...))` so the trailing publish line
        // applies them.
        let fallback_event: Option<Event> = if hard_escalation {
            let reason_str = if stall_key.starts_with("flow:") {
                "wave_stall_exhausted"
            } else {
                "stall_no_events"
            };
            let mut payload = format!(
                "RECOVERY (HARD): {} consecutive stall iterations (key=`{}`). \
                 Route to `{}` to emit review terminal or re-dispatch wave.",
                stall_count_value,
                stall_key,
                hard_target.as_str()
            );
            payload.push_str(&Self::format_recovery_diagnosis_block(
                reason_str,
                hard_target.as_str(),
                "emit review.wave.ready, review.passed, or review.failed",
                stall_count_value,
                &[],
            ));
            // U2 (2026-06-17-003 plan): wrap the free-form message
            // in a JSON object carrying the schema-required
            // `reason` and `target_hat` fields.
            // 2026-06-28-002 U3: stamp the hard_target's allowed
            // publish topics so the resumed agent sees the legal
            // emit surface and the isolated scope check sees the
            // same list.
            let hard_target_publishes = self.get_hat_publishes(&hard_target);
            let structured_payload = enrich_task_resume_payload_full(
                &payload,
                reason_str,
                Some(hard_target.as_str()),
                None,
                Some(RejectionKind::StallNoEvents),
                &hard_target_publishes,
            );
            debug!(
                stall_count = stall_count_value,
                target = %hard_target.as_str(),
                "Injecting HARD stall recovery to review hat"
            );
            // 2026-07-04-001 plan U16: validate that the hard_target
            // matches the original trigger topic's consumer. The
            // 2026-07-04-002 plan upgraded this from a `warn!` (which
            // silently dropped into the recovery envelope) to a hard
            // Block so a mismatch no longer publishes a `task.resume`
            // to a hat that won't pick it up.
            //
            // The hard_escalation path does not currently carry the
            // original trigger topic, so we pass `None` and rely on
            // the no-op fallback inside `validate_resume_routing`
            // (returns `Allow` when no `original_topic` is supplied).
            // This intentionally preserves the pre-fix behaviour for
            // the long-running stall ladder while still exposing the
            // new `EventLoopResumeDecision` API to future caller
            // upgrades. Routing-mismatch warnings for the hard ladder
            // surface in `recovery.jsonl` rather than blocking the
            // resume.
            if let EventLoopResumeDecision::Block(reason) =
                self.validate_resume_routing(&hard_target, None)
            {
                let diagnostic = Event::new(
                    "event.recovery.routing_blocked",
                    format!(
                        "{{\"target\":\"{}\",\"reason\":\"{}\"}}",
                        hard_target.as_str(),
                        reason
                    ),
                );
                self.bus.publish(diagnostic);
                warn!(target = %hard_target.as_str(), "{reason}");
            }
            // Plan 2026-08-10-001 U1: route hard-target recovery
            // through the unified publisher so the registry /
            // dedup / fail-close checks actually fire. The
            // caller-side `retry_key` is derived from the stall
            // context; this site never re-queues the same stall
            // twice in one activation. The unified helper already
            // publishes through the bus, so the trailing
            // `self.bus.publish(fallback_event)` line is skipped
            // for this branch by returning `None`.
            //
            // `current_loop_id()` borrows `&self`; capture it
            // before the mutable bus borrow.
            let loop_id_for_resume = self.current_loop_id();
            let loop_id_str = loop_id_for_resume.as_deref().unwrap_or("default");
            let activation_id = format!("resume:{}:{}", loop_id_str, self.state.iteration);
            let decision = crate::event_loop::resume_routing::task_resume_ingress(
                &mut self.bus,
                &self.registry,
                self.state.state_ledger.as_ref(),
                loop_id_str,
                &activation_id,
                hard_target.as_str(),
                None,
                &format!("hard_stall:{}", stall_count_value),
                structured_payload,
            );
            if let crate::event_loop::resume_routing::ResumeDecision::Block { reason } = &decision {
                tracing::warn!(
                    target = %hard_target.as_str(),
                    ?reason,
                    "hard-stall recovery blocked (no safe target)"
                );
            }
            None
        } else {
            match &self.state.last_hat {
                Some(hat_id) if hat_id.as_str() != "ralph" => {
                    let publishes = self.get_hat_publishes(hat_id);
                    let mut payload = if publishes.is_empty() {
                        format!(
                            "RECOVERY: Previous iteration by hat `{}` did not publish an event. \
                         Emit exactly one valid next event via `ralph emit`, or stop only after \
                         publishing the configured completion event.",
                            hat_id.as_str()
                        )
                    } else {
                        format!(
                            "RECOVERY: Previous iteration by hat `{}` did not publish an event. \
                         This failed because no event was emitted. Emit exactly ONE valid next \
                         event via `ralph emit`. Allowed topics: `{}`. Do not only write prose \
                         or update files. Stop immediately after emitting.\n\n\
                         If you attempted to emit an event in the previous turn but it was not \
                         recorded, you must use the bash tool to execute `ralph emit` — \
                         prose mentions are not sufficient.",
                            hat_id.as_str(),
                            publishes.join("`, `")
                        )
                    };

                    // U4: enrich the task.resume payload with a structured
                    // "## Recovery Diagnosis" block so the agent can act on
                    // the failure reason, not just the prose recovery hint.
                    payload.push_str(&Self::format_recovery_diagnosis_block(
                        "stall_no_events",
                        hat_id.as_str(),
                        "emit a regular event",
                        0,
                        &[],
                    ));

                    // U2 (2026-06-17-003 plan): wrap the free-form
                    // message in a JSON object carrying the
                    // schema-required `reason` and `target_hat` fields.
                    // 2026-06-28-002 U3: stamp `allowed_topics` so
                    // the agent's resumed emit is constrained to
                    // its own publishes (e.g. coordinator gets
                    // `work.ready` but NOT `work.start`).
                    let structured_payload = enrich_task_resume_payload_full(
                        &payload,
                        "stall_no_events",
                        Some(hat_id.as_str()),
                        None,
                        Some(RejectionKind::StallNoEvents),
                        &publishes,
                    );

                    debug!(
                        hat = %hat_id.as_str(),
                        "Injecting fallback event to recover - targeting last hat with task.resume"
                    );
                    // 2026-07-04-001 plan U16: validate that the resume
                    // target hat actually subscribes to the original
                    // topic. The fallback site does not carry the
                    // original trigger topic (it fires on "no events
                    // emitted"), so we pass `None` — the check is a
                    // no-op here (returns `Allow` per the new API
                    // contract). Routing-mismatch warnings surface
                    // at the upstream rejection site instead.
                    if let EventLoopResumeDecision::Block(reason) =
                        self.validate_resume_routing(hat_id, None)
                    {
                        warn!(hat = %hat_id.as_str(), "{reason}");
                    }
                    // Plan 2026-08-10-001 U1: route the last-hat
                    // fallback through the unified publisher so
                    // the dedup / fail-close checks fire. The
                    // caller-side `retry_key = "stall_no_events"`
                    // distinguishes this from the hard-escalation
                    // branch. The helper publishes directly into
                    // the bus; return `None` so the trailing
                    // publish line is skipped.
                    let loop_id_for_resume = self.current_loop_id();
                    let loop_id_str = loop_id_for_resume.as_deref().unwrap_or("default");
                    let activation_id = format!("resume:{}:{}", loop_id_str, self.state.iteration);
                    let decision = crate::event_loop::resume_routing::task_resume_ingress(
                        &mut self.bus,
                        &self.registry,
                        self.state.state_ledger.as_ref(),
                        loop_id_str,
                        &activation_id,
                        hat_id.as_str(),
                        None,
                        "stall_no_events",
                        structured_payload,
                    );
                    if let crate::event_loop::resume_routing::ResumeDecision::Block { reason } =
                        &decision
                    {
                        tracing::warn!(
                            target = %hat_id.as_str(),
                            ?reason,
                            "last-hat stall recovery blocked (no safe target)"
                        );
                    }
                    None
                }
                _ => {
                    let mut payload = String::from(
                        "RECOVERY: Previous iteration did not publish an event. \
                     Review the scratchpad and either dispatch the next task or complete the loop.",
                    );
                    // U4: enrich the Ralph fallback payload with a structured
                    // "## Recovery Diagnosis" block.
                    payload.push_str(&Self::format_recovery_diagnosis_block(
                        "stall_no_events",
                        "ralph",
                        "emit a regular event",
                        0,
                        &[],
                    ));
                    // U2 (2026-06-17-003 plan): wrap the free-form
                    // message in a JSON object carrying the
                    // schema-required `reason` and `target_hat` fields.
                    // 2026-06-28-002 U3: stamp `allowed_topics` for
                    // the ralph hat so the resumed iteration
                    // honours its own publishes.
                    let ralph_publishes = self.get_hat_publishes(&HatId::new("ralph"));
                    let _structured_payload = enrich_task_resume_payload_full(
                        &payload,
                        "stall_no_events",
                        Some("ralph"),
                        None,
                        Some(RejectionKind::StallNoEvents),
                        &ralph_publishes,
                    );
                    debug!("Ralph untargeted fallback dropped; recovered via stall pathway");
                    // Plan 2026-08-10-001 U1 R1: drop the
                    // untargeted fallback. The previous shape
                    // `Event::new("task.resume", structured_payload)`
                    // leaves `target = None`, which would fall
                    // through to subscription routing and reach
                    // any hat subscribed to `task.resume` —
                    // violating D4 (round-robin / unsourced
                    // resumes are forbidden). Surface the stall
                    // through the existing `loop.stalled` pathway
                    // when the steward is enabled; otherwise
                    // emit nothing here and let the up-stream
                    // `plan.blocked` ladder take over.
                    if self.config.event_loop.progress_steward.enabled {
                        let stall_event = Event::new(
                            "loop.stalled",
                            "{\"reason\":\"stall_no_events\",\"target\":\"ralph\"}".to_string(),
                        );
                        self.bus.publish(stall_event);
                    } else {
                        tracing::warn!(
                            "Ralph untargeted fallback dropped; no progress_steward to wake. \
                             Up-stream plan.blocked ladder handles the loop exit."
                        );
                    }
                    None
                }
            }
        };

        if let Some(fallback_event) = fallback_event {
            self.bus.publish(fallback_event);
        }
        true
    }

    /// Recover an isolated activation that completed successfully without
    /// publishing any of its declared terminal events.
    ///
    /// This is intentionally separate from `inject_fallback_event`: the
    /// latter is a generic no-events stall path and may target Ralph or a
    /// reviewer fallback.  A closed, empty terminal-obligation channel has
    /// a known owner and must retry that owner directly.
    pub fn inject_missing_terminal_emit_recovery(
        &mut self,
        hat_id: &HatId,
        terminal_topics: &[String],
    ) -> bool {
        self.inject_missing_terminal_emit_recovery_with_limit(
            hat_id,
            terminal_topics,
            U2_REJECTION_RETRY_LIMIT,
        )
    }

    /// Recover one empty isolated activation at most once. The next empty
    /// activation is a distinct failure and must fail closed instead of
    /// consuming the generic missing-event retry budget.
    pub fn inject_missing_terminal_emit_recovery_once(
        &mut self,
        hat_id: &HatId,
        terminal_topics: &[String],
    ) -> bool {
        self.inject_missing_terminal_emit_recovery_with_limit(hat_id, terminal_topics, 1)
    }

    fn inject_missing_terminal_emit_recovery_with_limit(
        &mut self,
        hat_id: &HatId,
        terminal_topics: &[String],
        retry_limit: u32,
    ) -> bool {
        if self.state.completion_honored || terminal_topics.is_empty() {
            return false;
        }

        let trigger = self
            .state
            .last_activation_events
            .iter()
            .rev()
            .find(|event| {
                self.registry
                    .get_config(hat_id)
                    .is_some_and(|config| config.triggers.iter().any(|t| t == event.topic.as_str()))
            })
            .or_else(|| self.state.last_activation_events.first());
        let Some(trigger) = trigger else {
            return false;
        };
        let trigger_topic = trigger.topic.to_string();
        let trigger_payload = trigger.payload.clone();
        // A supervisor wave-completion signal can legitimately wake a
        // dispatcher whose ready set is empty. It is a coordination tick,
        // not an agent-owned terminal obligation, so preserve the existing
        // no-op path for this class of trigger.
        let is_supervisor_wave_tick = self.config.event_loop.supervisor.enabled
            && (trigger_topic.ends_with(".wave.complete")
                || trigger_topic.ends_with(".wave.failed"));
        if crate::runtime_contract::is_wave_coordination_topic_trigger(&self.config, &trigger_topic)
            || is_supervisor_wave_tick
        {
            return false;
        }
        let primary_terminal_topic = terminal_topics
            .first()
            .map(String::as_str)
            .unwrap_or("terminal_event");
        let retry_key = format!(
            "missing_event_gate:{}:{}:{}:missing_terminal_emit",
            hat_id.as_str(),
            trigger_topic,
            terminal_topics.join("|")
        );
        let retry_count = self.state.record_rejection_key(&retry_key);

        if retry_count > retry_limit {
            if !self.terminal_event_emitted {
                let payload = serde_json::json!({
                    "reason": "missing_terminal_emit",
                    "target_hat": hat_id.as_str(),
                    "trigger_topic": trigger_topic,
                    "expected_terminal_events": terminal_topics,
                    "retry_key": retry_key,
                    "retry_count": retry_count,
                })
                .to_string();
                let blocked =
                    Event::new("plan.blocked", payload.clone()).with_target(HatId::new("ralph"));
                self.record_repair_event(&blocked);
                self.record_recovery_envelope(
                    &crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
                        .source(crate::diagnosis::DiagnosisSource::MissingEventGate)
                        .severity(crate::diagnosis::DiagnosisSeverity::Error)
                        .iteration(self.state.iteration)
                        .source_hat(hat_id.as_str())
                        .topic(primary_terminal_topic)
                        .reason_code("missing_terminal_emit_exhausted")
                        .message(format!(
                            "Hat '{}' exhausted recovery after {} missing terminal emits",
                            hat_id, retry_count
                        ))
                        .expected_action(
                            "stop the loop and inspect the responsible hat's emit path",
                        )
                        .safe_target(false)
                        .outcome(crate::diagnosis::DiagnosisOutcome::Failed)
                        .retry_key(retry_key.clone())
                        .build(),
                    vec![format!("retry limit {retry_limit}")],
                );
                if retry_limit == 1 {
                    let _ = self.publish_missing_terminal_fail_close(&payload);
                }
                self.terminal_event_emitted = true;
            }
            return false;
        }

        let allowed_topics = self.get_hat_publishes(hat_id);
        let rejection = crate::event_loop::rejection::rejection_with_key(
            RejectionStage::MissingEvent,
            Some(hat_id.as_str().to_string()),
            primary_terminal_topic.to_string(),
            "missing terminal emit after isolated activation",
            retry_key.clone(),
        );
        let mut rejection = rejection;
        rejection.kind = Some(RejectionKind::MissingEventGate);
        // Plan 2026-08-16-1015 Unit 2: compute the per-topic
        // required fields from the unified ProtocolView so the
        // resumed hat sees real schema field names (not topic
        // names) in both the legacy `required_fields` array and
        // the new `terminal_required_fields` map.
        let protocol_view =
            crate::preset::engine::protocol::ProtocolView::from_event_loop(&self.config.event_loop);
        let mut terminal_required_fields: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for topic in terminal_topics {
            let mut fields: Vec<String> = protocol_view
                .required_fields_for(topic)
                .map(|set| set.iter().cloned().collect())
                .unwrap_or_default();
            fields.sort();
            terminal_required_fields.insert(topic.clone(), fields);
        }
        let mut payload =
            crate::event_loop::rejection::build_task_resume_payload_with_terminal_contract(
                &rejection,
                &allowed_topics,
                terminal_topics,
                primary_terminal_topic,
                &terminal_required_fields,
                Some(trigger_topic.as_str()),
                Some(trigger_payload.as_str()),
                None,
            );
        // Keep retry attempts distinct in the live queue. The stable
        // rejection key groups the budget, while this attempt field lets
        // the resume dedup layer collapse only an exact duplicate of the
        // same attempt instead of suppressing the bounded retry sequence.
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&payload)
            && let Some(object) = value.as_object_mut()
        {
            object.insert(
                "retry_attempt".to_string(),
                serde_json::Value::from(retry_count),
            );
            payload = value.to_string();
        }
        // Plan 2026-08-10-001 U1: route the missing-terminal-emit
        // recovery through the unified publisher so the dedup /
        // fail-close checks fire. The `retry_key` carries the
        // bounded retry budget signature so duplicate retries
        // collapse into a single resume.
        let loop_id_for_resume = self.current_loop_id();
        let loop_id_str = loop_id_for_resume.as_deref().unwrap_or("default");
        let activation_id = format!("resume:{}:{}", loop_id_str, self.state.iteration);
        let decision = crate::event_loop::resume_routing::task_resume_ingress(
            &mut self.bus,
            &self.registry,
            self.state.state_ledger.as_ref(),
            loop_id_str,
            &activation_id,
            hat_id.as_str(),
            None,
            &format!("missing_terminal_emit:{}:{}", retry_key, retry_count),
            payload,
        );
        if let crate::event_loop::resume_routing::ResumeDecision::Block { reason } = &decision {
            tracing::warn!(
                target = %hat_id.as_str(),
                ?reason,
                "missing-terminal-emit recovery blocked (no safe target)"
            );
        }
        self.record_recovery_envelope(
            &crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
                .source(crate::diagnosis::DiagnosisSource::MissingEventGate)
                .severity(crate::diagnosis::DiagnosisSeverity::Error)
                .iteration(self.state.iteration)
                .source_hat(hat_id.as_str())
                .topic(primary_terminal_topic)
                .reason_code("missing_terminal_emit")
                .message(format!(
                    "Hat '{}' completed with an empty channel and did not emit one of {:?}",
                    hat_id, terminal_topics
                ))
                .expected_action("re-run the same hat and emit exactly one declared terminal event")
                .safe_target(true)
                .outcome(crate::diagnosis::DiagnosisOutcome::Pending)
                .retry_key(retry_key)
                .build(),
            vec![format!("retry attempt {retry_count}")],
        );
        true
    }

    /// Build the "## Recovery Diagnosis" appendix used by U4-enriched
    /// `task.resume` payloads. The block is a short, machine-greppable
    /// list of `key: value` lines that downstream tooling (and the
    /// agent itself) can rely on.
    pub fn format_recovery_diagnosis_block(
        reason: &str,
        target: &str,
        expected_action: &str,
        retry_attempt: u32,
        evidence_paths: &[String],
    ) -> String {
        let evidence = if evidence_paths.is_empty() {
            "(none)".to_string()
        } else {
            evidence_paths.join(", ")
        };
        format!(
            "\n\n## Recovery Diagnosis\n- reason: {reason}\n- target: {target}\n- expected action: {expected_action}\n- retry attempt: {retry_attempt}\n- evidence: {evidence}\n"
        )
    }

    /// Write a U4 recovery envelope + audit event for a workflow guard
    /// rejection. The rejected event is NOT re-published — the helper
    /// only records the diagnosis. `safe_target` is `false` because
    /// workflow guard rejections do not have a registered retry target
    /// (the agent has to fix the phase order, not a specific hat).
    pub(super) fn log_workflow_guard_rejection(
        event_loop: &mut EventLoop,
        rejection: &crate::validation::WorkflowGuardRejectionDetail,
    ) {
        let reason_code = if rejection.current_phase.is_none() {
            "workflow_correlation_extraction_failed"
        } else {
            "out_of_order_phase"
        };
        let target_hat = rejection
            .source_hat
            .clone()
            .or_else(|| Some(rejection.chain_name.clone()));
        let safe_target = false;
        let mut builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::WorkflowGuard)
            .severity(crate::diagnosis::DiagnosisSeverity::Warning)
            .iteration(event_loop.state().iteration)
            .topic(rejection.rejected_topic.clone());
        if let Some(hat) = rejection.source_hat.as_deref() {
            builder = builder.source_hat(hat);
        }
        builder = builder
            .reason_code(reason_code)
            .message(rejection.reason.clone())
            .expected_action(format!(
                "Wait for the correct phase before emitting '{}'. Next expected topic: {}",
                rejection.rejected_topic, rejection.next_expected
            ))
            .safe_target(safe_target)
            .outcome(crate::diagnosis::DiagnosisOutcome::Pending)
            .evidence(crate::diagnosis::EvidenceRef {
                kind: crate::diagnosis::EvidenceKind::Topic,
                ref_path: rejection.next_expected.clone(),
                snippet: None,
            })
            .retry_key(
                crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                    crate::diagnosis::DiagnosisSource::WorkflowGuard,
                    target_hat.as_deref(),
                    Some(rejection.rejected_topic.as_str()),
                    reason_code,
                    None,
                ),
            );
        if let Some(target) = target_hat.as_deref() {
            builder = builder.target_hat(target);
        }
        if let Some(session_id) = event_loop.diagnostics().session_id() {
            builder = builder.session_id(session_id);
        }
        let envelope = builder.build();
        // U6: workflow-guard rejections also flow through
        // `record_recovery_envelope` so the responder can surface
        // them in the next prompt. The original U3 journal + audit
        // logging is preserved by the helper.
        event_loop.record_recovery_envelope(
            &envelope,
            vec![format!(
                "chain={} instance={} next_expected={}",
                rejection.chain_name,
                rejection.instance_key.as_deref().unwrap_or("global"),
                rejection.next_expected
            )],
        );
    }

    /// Write a U5/R9 recovery envelope + audit event for a topic-format
    /// rejection. The rejected event is NOT re-published — the helper
    /// only records the diagnosis.
    ///
    /// `safe_target` is `false` because topic-format rejections are
    /// non-actionable by retry: the offending topic is fixed at the
    /// preset/agent-config level, not by re-emitting. The outcome is
    /// `NotRetriable` so the responder does not synthesize a fake
    /// `task.resume` and the journal entry sticks around for `ralph
    /// diagnose` to surface to operators.
    ///
    /// R10 plan commitment: "non-retryable, only write recovery signal".
    /// Before this helper, the topic-format rejection path published an
    /// `event.topic_format.rejected` diagnostic but never wrote the
    /// journal entry — i.e. silently dropped from the recovery stream.
    pub(super) fn log_topic_format_rejection(
        event_loop: &mut EventLoop,
        rejected_topic: &str,
        source_hat: Option<&str>,
        allowed_topics: &[String],
    ) {
        const REASON_CODE: &str = "invalid_topic_format";
        let safe_target = false;
        let allowed_preview = if allowed_topics.is_empty() {
            "(none)".to_string()
        } else if allowed_topics.len() <= 8 {
            allowed_topics.join(", ")
        } else {
            format!(
                "{} (+{} more)",
                allowed_topics
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
                allowed_topics.len() - 8
            )
        };
        let message = format!(
            "Topic '{}' is not in the whitelist of known topics (allowed: {})",
            rejected_topic, allowed_preview
        );
        let expected_action = format!(
            "Update the preset/hat config so '{}' is declared as a hat publish \
             (or trigger) topic, or remove the source that emits it. \
             This rejection is non-retryable and will not re-fire task.resume.",
            rejected_topic
        );
        let mut builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::TopicFormat)
            .severity(crate::diagnosis::DiagnosisSeverity::Warning)
            .iteration(event_loop.state().iteration)
            .topic(rejected_topic.to_string())
            .reason_code(REASON_CODE)
            .message(message.clone())
            .expected_action(expected_action)
            .safe_target(safe_target)
            .outcome(crate::diagnosis::DiagnosisOutcome::NotRetriable)
            .evidence(crate::diagnosis::EvidenceRef {
                kind: crate::diagnosis::EvidenceKind::Topic,
                ref_path: rejected_topic.to_string(),
                snippet: Some(format!("allowed_count={}", allowed_topics.len())),
            })
            .retry_key(
                crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                    crate::diagnosis::DiagnosisSource::TopicFormat,
                    source_hat,
                    Some(rejected_topic),
                    REASON_CODE,
                    None,
                ),
            );
        if let Some(hat) = source_hat {
            builder = builder.source_hat(hat);
        }
        if let Some(session_id) = event_loop.diagnostics().session_id() {
            builder = builder.session_id(session_id);
        }
        let envelope = builder.build();
        // Recovery journal + orchestration audit go through the same
        // U3/U6 pipeline as every other rejection. We swallow I/O
        // errors here: `record_recovery_envelope` already logs a warn
        // on failure and updating the responder must never block the
        // main loop.
        event_loop.record_recovery_envelope(&envelope, vec![message]);
    }

    /// U1 (2026-06-13-001): log a recovery envelope when the event policy
    /// rejected every wave event in a single read batch. This is the
    /// "wave dispatch blocked" signal that lets the runner skip the
    /// `missing_event_gate` (the agent DID try to emit) and that gives
    /// `ralph diagnose` a concrete `payload_contract` reason instead of
    /// a silent zero-fan-out.
    ///
    /// - `source` is `DiagnosisSource::PayloadContract` (KTD-3) — the
    ///   preset payload contract already covers required-field gaps.
    /// - `reason_code` is `wave_dispatch_blocked` for a generic batch
    ///   rejection, or `missing_required_field` when the first
    ///   rejection's violation type is `MissingRequiredField { .. }`.
    /// - `evidence` carries the topic, the raw wave count, and the
    ///   source hat (if any).
    pub(super) fn log_wave_policy_blocked_envelope(
        event_loop: &mut EventLoop,
        rejections: &[crate::event_policy::PolicyRejection],
        raw_count: usize,
    ) {
        use crate::diagnosis::{DiagnosisSeverity, DiagnosisSource};
        use crate::event_policy::ViolationType;

        // Drive reason_code / message off the first rejection when any
        // exist; otherwise fall back to a generic "wave_dispatch_blocked"
        // — this covers the Hold-only case where the policy validator
        // dropped events without producing a PolicyRejection row.
        let (reason_code, topic, source_hat, first_message): (&str, String, Option<String>, String) =
            match rejections.first() {
                Some(r) => {
                    let is_missing_field = matches!(
                        r.finding.violation_type,
                        ViolationType::MissingRequiredField { .. }
                    );
                    let code: &'static str = if is_missing_field {
                        "missing_required_field"
                    } else {
                        "wave_dispatch_blocked"
                    };
                    (
                        code,
                        r.topic.clone(),
                        r.source_hat.clone(),
                        r.finding.message.clone(),
                    )
                }
                None => (
                    "wave_dispatch_blocked",
                    "<unknown>".to_string(),
                    None,
                    "all wave events were dropped by event policy (no rejection row produced; likely Hold decisions)".to_string(),
                ),
            };
        let message = format!(
            "Wave dispatch blocked: all {} wave events were dropped by event policy. \
             First finding: {}",
            raw_count, first_message
        );
        let expected_action = format!(
            "Re-emit the wave with the corrected payload schema. The required fields for '{}' \
             are defined in the preset's event_policy.schemas block.",
            topic
        );
        let safe_target = true;

        let mut builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::PayloadContract)
            .severity(DiagnosisSeverity::Error)
            .iteration(event_loop.state().iteration)
            .topic(topic.clone())
            .reason_code(reason_code)
            .message(message.clone())
            .expected_action(expected_action)
            .safe_target(safe_target)
            .evidence(crate::diagnosis::EvidenceRef {
                kind: crate::diagnosis::EvidenceKind::Topic,
                ref_path: topic.clone(),
                snippet: Some(format!(
                    "raw_count={} rejected_count={}",
                    raw_count,
                    rejections.len()
                )),
            });

        // The `retry_key_from_parts` helper produces a stable
        // aggregation key based on (source, target_hat, topic,
        // reason_code, field). A follow-up emit with the same
        // corrected payload will dedupe against this envelope in
        // `ralph diagnose`.
        let wave_id_field: Option<&str> = None;
        if let Some(hat) = source_hat.as_deref() {
            builder = builder.source_hat(hat);
        }
        let retry_key = crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
            DiagnosisSource::PayloadContract,
            source_hat.as_deref(),
            Some(topic.as_str()),
            reason_code,
            wave_id_field,
        );
        builder = builder.retry_key(retry_key);

        if let Some(session_id) = event_loop.diagnostics().session_id() {
            builder = builder.session_id(session_id);
        }
        let envelope = builder.build();

        warn!(
            topic = %topic,
            raw_count = raw_count,
            rejection_count = rejections.len(),
            reason_code = %reason_code,
            "Wave dispatch blocked by event policy: all wave events rejected"
        );

        event_loop.record_recovery_envelope(&envelope, vec![message]);
    }

    /// Builds the prompt for a hat's execution.
    ///
    /// Per "Hatless Ralph" architecture:
    /// - Solo mode: Ralph handles everything with his own prompt
    /// - Multi-hat mode: Ralph is the sole executor, custom hats define topology only
    ///
    /// When in multi-hat mode, this method collects ALL pending events across all hats
    /// and builds Ralph's prompt with that context. The `## HATS` section in Ralph's
    /// prompt documents the topology for coordination awareness.
    ///
    /// If memories are configured with `inject: auto`, this method also prepends
    /// primed memories to the prompt context. If a scratchpad file exists and is
    /// non-empty, its content is also prepended (before memories).
    pub(super) fn append_terminal_deliverable_contract(
        &self,
        prompt: String,
        hat_id: &HatId,
    ) -> String {
        let promise = self.config.event_loop.completion_promise.as_str();
        let Some(hat) = self.registry.get_config(hat_id) else {
            return prompt;
        };
        let publishes_completion = hat.publishes.iter().any(|topic| topic == promise)
            || hat.default_publishes.as_deref() == Some(promise);
        if !publishes_completion {
            return prompt;
        }

        let Some(policy) = self.config.event_loop.event_policy.as_ref() else {
            return prompt;
        };
        let Some(schema) = policy.schemas.get(promise) else {
            return prompt;
        };
        let Some(path_field) = ["report_path", "artifact_path"].iter().find(|field| {
            schema
                .required_fields
                .iter()
                .any(|required| required == **field)
        }) else {
            return prompt;
        };
        let field_doc = schema.field_docs.get(*path_field);
        let path_source = field_doc
            .map(|doc| doc.source.trim())
            .filter(|source| !source.is_empty())
            .unwrap_or("the real operator-facing artifact available in this activation");
        let fill_rule = field_doc
            .map(|doc| doc.fill_rule.trim())
            .filter(|rule| !rule.is_empty())
            .unwrap_or("use the real repo-relative path; never invent a path");

        format!(
            "{prompt}\n\n## TERMINAL DELIVERABLE CONTRACT\n\
             This is the final activation for completion topic `{promise}`.\n\
             - Before emitting, resolve `{path_field}` from: {path_source}.\n\
             - Contract: {fill_rule}.\n\
             - Verify the file is readable with `test -f` before policy-check and the real emit.\n\
             - The `{promise}` payload MUST include `{path_field}` with that exact repo-relative path.\n\
             - After the emit succeeds, your final visible reply MUST contain exactly one standalone line:\n\
             `DELIVERABLE_PATH: <{path_field}>`\n\
             Replace the placeholder with the same path carried in `{path_field}`. Do not finish with only a prose summary.\n"
        )
    }

    pub fn build_prompt(&mut self, hat_id: &HatId) -> Option<String> {
        if let Some(workspace) = self
            .loop_context
            .as_ref()
            .map(|context| context.workspace())
        {
            match crate::event_loop::worktree_handoff::WorktreeSnapshot::capture(workspace) {
                Ok(snapshot) => {
                    self.activation_worktree_baselines
                        .insert(hat_id.as_str().to_string(), snapshot);
                }
                Err(error) => warn!(
                    hat = %hat_id,
                    error = %error,
                    "failed to capture activation worktree baseline; audit will fail closed"
                ),
            }
        }
        // 2026-06-13-004 U8 (P1-2): clear any pending handoff
        // deadlines for this hat. The hat is now actually
        // *building* a prompt — about to invoke the LLM — so
        // the deadline race that produced the 17m / 4m false
        // handoff timeouts in the 2026-06-13 incident is over.
        // KTD-6 explicitly forbids moving this clear to
        // `process_output` (L4223 `current_isolated_hat`): that
        // site records the *completed* hat, not the *about-to-
        // activate* hat. The build_prompt entry point is the
        // earliest moment the hat is unambiguously "live".
        // Safe in coordinator mode too — `on_hat_activated`
        // is a no-op when the tracker's `pending` map is empty
        // (and in coordinator mode the tracker is always empty).
        //
        // 2026-06-13 review fix (reliability F2): the "ralph"
        // hat is the constant coordinator sentinel, never a
        // handoff *consumer* — passing it through here would
        // spuriously clear real consumer pending entries whose
        // hat_id happens to match (or be a prefix of) "ralph".
        // Skip the clear for ralph; downstream ralph prompt
        // building still proceeds normally below.
        if hat_id.as_str() != "ralph" {
            self.state.handoff_tracker.on_hat_activated(hat_id.as_str());
        }
        // Handle "ralph" hat - the constant coordinator
        // Per spec: "Hatless Ralph is constant — Cannot be replaced, overwritten, or configured away"
        if hat_id.as_str() == "ralph" {
            if self.config.hats.is_empty() {
                // Solo mode - just Ralph's events, no hats to filter
                let mut events = self.bus.take_pending(&hat_id.clone());
                let mut human_events = self.bus.take_human_pending();
                events.append(&mut human_events);

                // Separate human.guidance events from regular events
                let (guidance_events, regular_events): (Vec<_>, Vec<_>) = events
                    .into_iter()
                    .partition(|e| e.topic.as_str() == "human.guidance");

                let events_context = regular_events
                    .iter()
                    .map(|e| Self::format_event(e))
                    .collect::<Vec<_>>()
                    .join("\n");

                // Solo mode: set scratchpad and iteration before guidance persistence
                self.ralph
                    .set_active_scratchpad(self.config.core.scratchpad.clone());
                self.ralph.set_iteration(self.state.iteration);

                // Unit 3 (2026-06-16-002 plan): during the
                // coordinator bootstrap window we MUST NOT inject
                // human guidance into the prompt — the agent's
                // first action should be the legal bootstrap
                // handoff, not a response to stale human input.
                // The gate fires for `hat_id == "coordinator"`
                // and `in_bootstrap_phase() == true`; in solo mode
                // `hat_id == "ralph"` so the guard is a no-op
                // (kept here for symmetry with the multi-hat /
                // isolated paths and as a safety net).
                if self.coordinator_bootstrap_gate_closed(hat_id) {
                    // Bootstrap window: drop pending guidance events
                    // (they are still on the bus and will be
                    // redelivered on the next iteration once
                    // `bootstrap_complete` flips to `true`).
                    drop(guidance_events);
                } else {
                    // Persist and inject human guidance into prompt if present
                    self.update_robot_guidance(guidance_events);
                    self.apply_robot_guidance(hat_id);
                }

                // Build base prompt and prepend memories + scratchpad + ready tasks
                let base_prompt = self.ralph.build_prompt(&events_context, &[], &[]);
                self.ralph.clear_robot_guidance();
                let base_prompt = self.inject_phase_into_prompt(base_prompt);
                // U6: fold the soft runtime-diagnosis alert into the
                // prompt before skills prepending. The order
                // (phase → diagnosis alert → skills) is fixed by the
                // U6 plan so the skills index is never broken by the
                // alert text.
                let base_prompt = self.apply_runtime_diagnosis_prompt(base_prompt, hat_id);
                // 2026-06-28-003: prepend recovery directives derived
                // from pending `task.resume` events so the agent sees
                // behaviour guidance before the skill index.
                let base_prompt = self.prepend_recovery_directives(base_prompt, &regular_events);
                let with_skills = self.prepend_auto_inject_skills(base_prompt, hat_id);
                let with_scratchpad = self.prepend_scratchpad(with_skills, Some(hat_id));
                let with_state_files = self.prepend_state_files(with_scratchpad);
                let final_prompt = self.prepend_ready_tasks(with_state_files, Some(hat_id));
                // U7a (plan 2026-06-21-002): prepend the
                // deterministic correction + resume blocks.  The
                // queue lives on `LoopState::prompt_context` and
                // is populated by `emit_correction_context` on
                // the policy rejection path; this prepend is a
                // no-op when the queue is empty (the legacy
                // `task.resume` path keeps working unchanged).
                let final_prompt = self.prepend_correction_and_resume(final_prompt, hat_id);
                // U4b (plan 2026-06-20-001, R12 / R13 / KTD-8):
                // if the most recent `ralph emit` was rejected by
                // the lint phase, inject `## LINT MIRROR` +
                // `## LINT RESUME REQUIRED` so the next prompt
                // tells the agent *what* the lint saw and *which
                // hat* should fix it.  The hint is consumed on
                // first read (consume-on-use) so a stale resume
                // does not leak across prompts.
                let final_prompt = self.inject_pending_lint_resume(final_prompt, hat_id);

                debug!("build_prompt: routing to HatlessRalph (solo mode)");
                return Some(final_prompt);
            } else if self.config.event_loop.execution_mode != HatExecutionMode::Isolated {
                // Coordinator multi-hat mode: collect events and determine active hats.
                // Isolated mode must NOT take this path — ralph is a round-robin peer
                // and may only consume its own pending queue. Draining every hat's
                // queue here steals multi-consumer handoffs (e.g. `plan.complete`
                // pending for reporter/shipper) and downstream hats never activate.
                let mut all_hat_ids: Vec<HatId> = self.bus.hat_ids().cloned().collect();
                // Deterministic ordering (avoid HashMap iteration order nondeterminism).
                all_hat_ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));

                let mut all_events = Vec::new();
                let mut system_events = Vec::new();

                for id in &all_hat_ids {
                    let pending = self.bus.take_pending(id);
                    if pending.is_empty() {
                        continue;
                    }

                    let (drop_pending, exhausted_event) = self.check_hat_exhaustion(id, &pending);
                    if drop_pending {
                        // Drop the pending events that would have activated the hat.
                        if let Some(exhausted_event) = exhausted_event {
                            all_events.push(exhausted_event.clone());
                            system_events.push(exhausted_event);
                        }
                        continue;
                    }

                    all_events.extend(pending);
                }

                let mut human_events = self.bus.take_human_pending();
                all_events.append(&mut human_events);

                // Publish orchestrator-generated system events after consuming pending events,
                // so they become visible in the event log and can be handled next iteration.
                for event in system_events {
                    self.bus.publish(event);
                }

                // Separate human.guidance events from regular events
                let (guidance_events, regular_events): (Vec<_>, Vec<_>) = all_events
                    .into_iter()
                    .partition(|e| e.topic.as_str() == "human.guidance");

                // Ignore kickoff/recovery noise when a real downstream event is pending.
                let effective_regular_events = self.effective_regular_events(&regular_events);

                // Determine which hats are active based on regular events
                let active_hat_ids = self.determine_active_hat_ids(&regular_events);
                self.record_hat_activations(&active_hat_ids);
                self.state.last_active_hat_ids = active_hat_ids.clone();

                // 2026-06-17-004 U2 (R3): refresh the per-hat
                // activation clock for every hat about to execute
                // an agent.  The clock is the source of truth for
                // the missing-event gate's grace window: when the
                // gate fires within `hat.missing_event_grace_secs`
                // (default `min(adapter_idle * 0.3, 540)`) of an
                // activation, the gate is suppressed so long-running
                // hats like `dimension-reviewer` (per-worker timeout
                // 1800s) are not mis-fired during the first few
                // seconds of model warm-up.  Subsequent activations
                // REPLACE the timestamp so a hat that loops through
                // many short turns does not accumulate a stale
                // clock that suppresses the gate past its useful
                // window.
                for hat_id in &active_hat_ids {
                    self.state.record_hat_activation(hat_id);
                }

                // 2026-06-26 plan U4: push a fresh obligation for each
                // active hat. The MissingEventGate (U4) now consults
                // the obligation queue instead of the activation
                // clock. `terminal_events` (if non-empty) is the
                // set of topics that count as "the hat has
                // fulfilled its trigger obligation" — for hats
                // without an explicit `terminal_events` list we
                // fall back to `publishes`. Hats with neither
                // receive no obligation (no contract to enforce).
                for hat_id in &active_hat_ids {
                    if let Some(hat_cfg) = self.registry.get_config(hat_id).cloned() {
                        let expected = if !hat_cfg.terminal_events.is_empty() {
                            hat_cfg.terminal_events.clone()
                        } else if !hat_cfg.publishes.is_empty() {
                            hat_cfg.publishes.clone()
                        } else {
                            continue;
                        };
                        // The trigger topic is the first regular
                        // event whose topic is in this hat's
                        // configured `triggers`. Falls back to the
                        // first regular event's topic if no exact
                        // match — preserves the old record path.
                        let trigger_topic: String = regular_events
                            .iter()
                            .find(|e| hat_cfg.triggers.iter().any(|t| t == e.topic.as_str()))
                            .map(|e| e.topic.to_string())
                            .or_else(|| regular_events.first().map(|e| e.topic.to_string()))
                            .unwrap_or_default();
                        self.state.push_hat_obligation(
                            hat_id.clone(),
                            trigger_topic.clone(),
                            expected,
                        );
                    }
                }

                // U3: Record activation lifecycle for each active hat.
                // For each hat activation, create an ActivationKey and activate the tracker.
                // The trigger topic is the first regular event whose topic matches
                // one of this hat's configured `triggers`. This must be derived from
                // the hat's subscription (NOT `can_publish` — trigger events are hat
                // *inputs*, not publishes; using `can_publish` caused the activate
                // side to fall through to the "unknown" fallback in production —
                // P0 code-review finding #1).
                for hat_id in &active_hat_ids {
                    let trigger_topic = self
                        .registry
                        .get_config(hat_id)
                        .map(|config| {
                            let trigger_topics = config.trigger_topics();
                            effective_regular_events
                                .iter()
                                .find(|e| {
                                    trigger_topics
                                        .iter()
                                        .any(|t| t.matches_str(e.topic.as_str()))
                                })
                                .map(|e| e.topic.to_string())
                                .unwrap_or_else(|| "unknown".to_string())
                        })
                        .unwrap_or_else(|| "unknown".to_string());
                    let key = ActivationKey {
                        loop_id: self
                            .loop_context
                            .as_ref()
                            .and_then(|ctx| ctx.loop_id())
                            .unwrap_or("primary")
                            .to_string(),
                        iteration: self.state.iteration,
                        hat_id: hat_id.as_str().to_string(),
                    };
                    self.hat_lifecycle_tracker.activate(
                        key,
                        trigger_topic,
                        None, // linked_task_id resolved later if available
                    );
                }
                self.state.last_activation_events =
                    effective_regular_events.iter().copied().cloned().collect();

                // Resolve scratchpad config for the active hat (or global default).
                // Must happen BEFORE guidance persistence so guidance is written
                // to the correct hat's scratchpad file.
                let resolved_scratchpad = if let Some(hat_id) = active_hat_ids.first() {
                    let hat_scratchpad = self
                        .registry
                        .get_config(hat_id)
                        .and_then(|c| c.scratchpad.as_ref());
                    ScratchpadConfig::resolve(hat_scratchpad, &self.config.core.scratchpad)
                } else {
                    // Ralph coordinating — use global
                    self.config.core.scratchpad.clone()
                };
                self.ralph.set_active_scratchpad(resolved_scratchpad);
                self.ralph.set_iteration(self.state.iteration);

                // Unit 3 (2026-06-16-002 plan): in multi-hat mode
                // `hat_id == "ralph"` (we are in this branch
                // because the ralph hat requested a prompt), so
                // the `coordinator_bootstrap_gate_closed` check
                // is a no-op.  Still, keep the guard for parity
                // with the isolated path — a future preset that
                // routes the multi-hat path through a hat named
                // "coordinator" will inherit the bootstrap
                // suppression automatically.
                if self.coordinator_bootstrap_gate_closed(hat_id) {
                    drop(guidance_events);
                } else {
                    // Persist and inject human guidance after scratchpad resolution
                    // (must also happen before immutable borrows from determine_active_hats)
                    self.update_robot_guidance(guidance_events);
                    self.apply_robot_guidance(hat_id);
                }

                let active_hats = self.determine_active_hats(&regular_events);

                // FR-1: Hat-level event allowlist filtering.
                // If every active hat has an enabled allowlist, compute the union
                // of their configured events plus their triggers. Otherwise,
                // disable filtering for this iteration.
                let mut should_filter = true;
                let mut union_allowlist = std::collections::HashSet::new();
                for hat in &active_hats {
                    if let Some(config) = self.registry.get_config(&hat.id)
                        && let Some(ref filter) = config.event_filter
                        && filter.enabled
                    {
                        union_allowlist.extend(filter.events.iter().cloned());
                        union_allowlist.extend(config.triggers.iter().cloned());
                        continue;
                    }
                    // Fallback-only hats (e.g., builtin ralph with `*` subscription)
                    // have no config and should not disable filtering.
                    if hat.is_fallback_only() {
                        continue;
                    }
                    should_filter = false;
                    break;
                }

                let filtered_events: Vec<&Event> = if should_filter && !union_allowlist.is_empty() {
                    effective_regular_events
                        .into_iter()
                        .filter(|e| union_allowlist.contains(e.topic.as_str()))
                        .collect()
                } else {
                    effective_regular_events
                };

                // Extract trigger topic(s) for the active hats so they appear in the
                // prompt as `## ACTIVE TRIGGER`. Derive from `filtered_events` (the
                // FR-1-filtered subset) — not `regular_events` — so that the trigger
                // list stays consistent with what the prompt's PENDING EVENTS section
                // actually shows, avoiding re-injection of filtered-out events.
                let trigger_topics: Vec<String> = filtered_events
                    .iter()
                    .filter(|e| !Self::is_system_event(e.topic.as_str()))
                    .map(|e| e.topic.to_string())
                    .collect();

                // Format events for context
                let events_context = filtered_events
                    .iter()
                    .map(|e| Self::format_event(e))
                    .collect::<Vec<_>>()
                    .join("\n");

                // Build base prompt and prepend memories + scratchpad if available
                let base_prompt = self.ralph.build_prompt(
                    &events_context,
                    &active_hats,
                    &trigger_topics
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                );

                // Build prompt with active hats - filters instructions to only active hats
                debug!(
                    "build_prompt: routing to HatlessRalph (multi-hat coordinator mode), active_hats: {:?}",
                    active_hats
                        .iter()
                        .map(|h| h.id.as_str())
                        .collect::<Vec<_>>()
                );

                // Clear guidance after active_hats references are no longer needed
                self.ralph.clear_robot_guidance();
                let base_prompt = self.inject_phase_into_prompt(base_prompt);
                // U6: see solo-mode comment above. Coordinator
                // path passes `hat_id` (the ralph hat) so the
                // helper sees the full set of findings — the
                // coordinator sees every hat's alerts.
                let base_prompt = self.apply_runtime_diagnosis_prompt(base_prompt, hat_id);
                // 2026-06-28-003: prepend recovery directives derived
                // from pending `task.resume` events.
                let base_prompt = self.prepend_recovery_directives(base_prompt, &regular_events);
                let with_skills = self.prepend_auto_inject_skills(base_prompt, hat_id);
                let with_scratchpad = self.prepend_scratchpad(with_skills, Some(hat_id));
                let with_state_files = self.prepend_state_files(with_scratchpad);
                let final_prompt = self.prepend_ready_tasks(with_state_files, Some(hat_id));
                // U7a (plan 2026-06-21-002): prepend deterministic
                // correction + resume blocks.  No-op when the
                // queue is empty.
                let final_prompt = self.prepend_correction_and_resume(final_prompt, hat_id);
                // U4b: see solo-mode comment above. Same
                // consume-on-use semantics for the lint hint.
                let final_prompt = self.inject_pending_lint_resume(final_prompt, hat_id);

                return Some(final_prompt);
            }
        }

        // Isolated per-hat prompt (including ralph when it is selected by round-robin).
        if self.config.event_loop.execution_mode == HatExecutionMode::Isolated {
            // Isolated mode: build focused prompt for this hat only.
            let mut events = self.bus.take_pending(&hat_id.clone());
            let mut human_events = self.bus.take_human_pending();
            events.append(&mut human_events);

            let (guidance_events, regular_events): (Vec<_>, Vec<_>) = events
                .into_iter()
                .partition(|e| e.topic.as_str() == "human.guidance");

            // Mirror the multi-hat Ralph path (L4636–4718): record the
            // trigger events this activation consumed so the missing-event
            // gate can distinguish pass-through hats (e.g. shipper on a
            // multi-consumer `plan.complete`) from hats that truly forgot
            // to emit.
            self.state.record_hat_activation(hat_id);
            self.state.last_activation_events = regular_events.clone();

            // Apply per-hat event filter if configured
            let hat_config = self.registry.get_config(hat_id);
            let mut allowlist = std::collections::HashSet::new();
            let should_filter = if let Some(config) = hat_config
                && let Some(ref filter) = config.event_filter
                && filter.enabled
            {
                allowlist.extend(filter.events.iter().cloned());
                allowlist.extend(config.triggers.iter().cloned());
                !allowlist.is_empty()
            } else {
                false
            };

            let filtered_events: Vec<&Event> = if should_filter {
                regular_events
                    .iter()
                    .filter(|e| {
                        allowlist.contains(e.topic.as_str()) || Self::is_recovery_channel_event(e)
                    })
                    .collect()
            } else {
                regular_events.iter().collect()
            };

            let events_context = filtered_events
                .iter()
                .map(|e| Self::format_event(e))
                .collect::<Vec<_>>()
                .join("\n");

            // Resolve scratchpad for this hat
            let resolved_scratchpad = self
                .registry
                .get_config(hat_id)
                .and_then(|c| c.scratchpad.as_ref())
                .map(|s| ScratchpadConfig::resolve(Some(s), &self.config.core.scratchpad))
                .unwrap_or_else(|| self.config.core.scratchpad.clone());
            self.ralph.set_active_scratchpad(resolved_scratchpad);
            self.ralph.set_iteration(self.state.iteration);

            // Unit 3 (2026-06-16-002 plan): the isolated path is
            // the **only** path where the gate can actually fire
            // (the active `hat_id` is a real hat, not the
            // constant `ralph` sentinel).  When the active hat
            // is the `coordinator` and the loop is still in
            // bootstrap, drop the pending `human.guidance` events
            // and skip both `update_robot_guidance` /
            // `apply_robot_guidance` AND the
            // `collect_robot_guidance` block below — none of the
            // cached guidance should reach the coordinator's
            // first prompt.
            let skip_guidance = self.coordinator_bootstrap_gate_closed(hat_id);
            if skip_guidance {
                drop(guidance_events);
            } else {
                // Handle guidance
                self.update_robot_guidance(guidance_events);
                self.apply_robot_guidance(hat_id);
            }

            // Build base prompt
            let hat = self.registry.get(hat_id)?;

            // Debug logging to trace hat routing
            debug!(
                "build_prompt: hat_id='{}', instructions.is_empty()={}",
                hat_id.as_str(),
                hat.instructions.is_empty()
            );

            debug!(
                "build_prompt: routing to build_custom_hat() for '{}' (isolated mode)",
                hat_id.as_str()
            );

            let base_prompt = self
                .instruction_builder
                .build_custom_hat(hat, &events_context);
            // 2026-06-23 T2: append `## RUNTIME CONFIG` block so the hat
            // can read the runtime-resolved `max_fix_rounds` (which lives
            // under `event_loop:` in the YAML). The block is informational
            // and lives at the END of the hat prompt so the hat's own
            // workflow order (in `### GUARDRAILS`) stays authoritative.
            let base_prompt =
                append_runtime_config_block(base_prompt, self.config.event_loop.max_residuals);
            let base_prompt = self.append_terminal_deliverable_contract(base_prompt, hat_id);

            // Inject the cached `human.guidance` text as a `## ROBOT GUIDANCE`
            // block so isolated hats (whose `build_custom_hat` template does
            // not read `ralph.robot_guidance` on its own) still see the
            // guidance that was just persisted to the scratchpad. We must
            // call this BEFORE `clear_robot_guidance()` below, otherwise the
            // in-memory copy is gone.
            //
            // Unit 3 (2026-06-16-002 plan): when the gate is
            // closed we did NOT call `update_robot_guidance` /
            // `apply_robot_guidance` above, so the in-memory
            // guidance cache is empty; `collect_robot_guidance`
            // returns an empty string and the conditional below
            // leaves `base_prompt` unchanged.  We still call
            // the helper for symmetry / future-proofing.
            let guidance_section = self.ralph.collect_robot_guidance();
            let base_prompt = if guidance_section.is_empty() {
                base_prompt
            } else {
                format!("{guidance_section}{base_prompt}")
            };

            // Apply prepend pipeline (SAME order as coordinator path)
            self.ralph.clear_robot_guidance();

            // 2026-06-17-003 U4 / 2026-06-17-005 R5:
            // `## ORCHESTRATOR CONTEXT` block is the canonical
            // view of the run. The block is always emitted
            // (even when projection is disabled) so the agent
            // never has to hand-read a ledger; the
            // `projection_disabled` flag in the block tells the
            // agent whether the values are live. R5 in
            // 2026-06-17-005 pins Phase 1 scope to the
            // **isolated** build_prompt path only — see the
            // Phase 1 scope note on `prepend_orchestrator_context`
            // and the backward-compat custom-hat path.
            //
            // OPAC U2: `## HAT IDENTITY` is the agent's single
            // source of truth for its role and permissions. It
            // lives *above* ORCHESTRATOR CONTEXT so the agent sees
            // "who you are" before "what the loop is doing" (KTD-5).
            let base_prompt = self.prepend_hat_identity(base_prompt, hat_id);
            // P1-7 fix: orchestrator context is placed BEFORE
            // wave context so the prompt stack order is:
            //   ## HAT IDENTITY
            //   ## WAVE CONTEXT (synthesizer only)
            //   ## ORCHESTRATOR CONTEXT
            //   hat instructions
            let base_prompt = self.prepend_orchestrator_context(base_prompt, hat_id);

            // R1: `## WAVE CONTEXT` block lives near the top for
            // `review-synthesizer`; it is a no-op for any other hat.
            let base_prompt = self.prepend_wave_context(base_prompt, hat_id);

            // R3: surface ephemeral relocations so the agent stops
            // recreating runtime artefacts inside the source tree.
            let base_prompt = self.prepend_ephemeral_relocations(base_prompt);
            let base_prompt = self.inject_phase_into_prompt(base_prompt);
            // U6: in isolated mode the helper filters findings to
            // those whose target/source hat matches `hat_id`. The
            // plan's "isolated hat mode 下 alert 只注入目标 hat"
            // contract is enforced inside `apply_runtime_diagnosis_prompt`.
            let base_prompt = self.apply_runtime_diagnosis_prompt(base_prompt, hat_id);
            // 2026-06-18-001 plan U6: 注入 `## RECENT REJECTIONS` 块
            // 告诉 agent 最近哪些 emit 被 runtime 拒收。让 agent
            // 看到 backpressure,避免用同一 payload 反复探测。
            let base_prompt = self.prepend_rejection_digest(base_prompt);
            // U7a (plan 2026-06-21-002): prepend the
            // deterministic correction + resume blocks.  Always
            // prepends the resume block when `--continue` ran
            // (the queue is non-empty).  Always prepends the
            // correction block when the queue is non-empty
            // (the U7a `prompt_context` queue is populated by
            // `emit_correction_context` calls on the policy
            // rejection path; when the feature flag is off, the
            // queue stays empty and this prepend is a no-op).
            let base_prompt = self.prepend_correction_and_resume(base_prompt, hat_id);
            // 2026-06-28-003: prepend recovery directives derived
            // from pending `task.resume` events.
            let base_prompt = self.prepend_recovery_directives(base_prompt, &regular_events);
            // 2026-07-09-003 plan (U3): prepend the schema-backed
            // `## TRIGGER CONTEXT` block. The helper is a no-op
            // when the schema has no `trigger_context`
            // declaration or the hat does not subscribe to the
            // source topic, so the SC6 / R3 / R29 byte-identical
            // pre-feature contract holds for undeclared
            // presets.
            let base_prompt = self.prepend_trigger_context(base_prompt, hat_id, &regular_events);
            let with_skills = self.prepend_auto_inject_skills(base_prompt, hat_id);
            let with_scratchpad = self.prepend_scratchpad(with_skills, Some(hat_id));
            let with_state_files = self.prepend_state_files(with_scratchpad);
            let final_prompt = self.prepend_ready_tasks(with_state_files, Some(hat_id));
            // U18: macro edge next hint — when `event_loop.macro_edge_next_hint.enabled`
            // is true, prepend a one-line `## NEXT ACTION` derived from the most recent
            // accepted business event payload's `next_hint` field (≤120 chars). When the
            // feature is disabled or no hint is available the prepend is a no-op.
            let final_prompt = self.prepend_macro_next_hint(final_prompt, &regular_events, hat_id);
            // 2026-07-06-004 plan U6: wire the handoff envelope
            // extractor (U5) + prepender (U4) into the isolated
            // prompt chain. The helper is gated on
            // `event_loop.handoff_envelope.enabled &&
            // prompt_injection` and on a recent event carrying a
            // valid envelope; default-closed so non-serial presets
            // and ad-hoc loops are unaffected (regression defence
            // #3 / #6).
            let final_prompt = build_isolated_prompt_with_handoff(
                crate::event_loop::prompt_helpers::IsolatedPromptInputs {
                    base_prompt: final_prompt,
                    events: &regular_events,
                    config: &self.config.event_loop.handoff_envelope,
                    // U5 (2026-07-06-004 fix-plan R5): tighten
                    // the trust boundary so envelopes addressed
                    // to a different hat never reach this
                    // hat's prompt. `hat_id` is the current
                    // isolated hat id.
                    current_hat: hat_id.as_str(),
                },
            );
            // U4b: see solo-mode comment above. In isolated
            // mode the lint hint routes to the *source* hat
            // (the one that emitted the rejected event), so the
            // helper consults `pending_lint_resume.target` to
            // decide whether the current hat is the recipient.
            // The hint is consumed on first injection so the
            // same failure is not replayed forever.
            let final_prompt = self.inject_pending_lint_resume(final_prompt, hat_id);

            // Set active hat for downstream logic (default_publishes, enforce_hat_scope)
            self.state.last_active_hat_ids = vec![hat_id.clone()];

            return Some(final_prompt);
        }

        // Backward compatibility / non-isolated mode: simple custom hat prompt
        let events = self.bus.take_pending(&hat_id.clone());
        let events_context = events
            .iter()
            .map(|e| Self::format_event(e))
            .collect::<Vec<_>>()
            .join("\n");

        let hat = self.registry.get(hat_id)?;

        // Set active hat for downstream logic (default_publishes, enforce_hat_scope).
        // Mirror the isolated-mode assignment at L4079 so observers reading
        // `last_active_hat_ids` after `build_prompt` see the same value in both
        // execution modes. Without this, backward-compat (Coordinator default)
        // callers would observe a stale Vec while isolated callers see the
        // just-built hat — see test_rejected_work_done_retry_payload_reaches_executor_prompt.
        self.state.last_active_hat_ids = vec![hat_id.clone()];

        // Debug logging to trace hat routing
        debug!(
            "build_prompt: hat_id='{}', instructions.is_empty()={}",
            hat_id.as_str(),
            hat.instructions.is_empty()
        );

        // All hats use build_custom_hat with ghuntley-style prompts
        debug!(
            "build_prompt: routing to build_custom_hat() for '{}'",
            hat_id.as_str()
        );
        // U6: in the backward-compat custom-hat path there is no
        // isolated-mode filtering (the path is reached only when
        // execution_mode != Isolated), so we always pass the full
        // hat_id; the responder injects every finding whose hat
        // matches or has no hat binding.
        let base = self
            .instruction_builder
            .build_custom_hat(hat, &events_context);
        // 2026-06-23 T2: append `## RUNTIME CONFIG` block so the hat can
        // read the runtime-resolved `max_fix_rounds`. Appended BEFORE
        // `inject_phase_into_prompt` so the phase block (if any) sits
        // just above RUNTIME CONFIG at the tail of the prompt.
        let base = append_runtime_config_block(base, self.config.event_loop.max_residuals);
        let base = self.append_terminal_deliverable_contract(base, hat_id);
        let with_phase = self.inject_phase_into_prompt(base);
        let with_diagnosis = self.apply_runtime_diagnosis_prompt(with_phase, hat_id);
        // R5 (2026-06-17-005 fix plan): the
        // `## ORCHESTRATOR CONTEXT` block is intentionally NOT
        // injected on this path in Phase 1. The backward-compat
        // custom-hat path predates the state projector and
        // shares a single `events_context` across every hat in
        // the same loop; threading the projector snapshot
        // through here without breaking the
        // `RUNTIME_DIAGNOSIS_ALERT_HEADER` / auto-inject-skills
        // contract is a Phase 2 task. See the Phase 1 scope
        // note on `prepend_orchestrator_context` (event_loop)
        // and the comment on the isolated build_prompt branch
        // at L4522.
        // We intentionally skip `prepend_auto_inject_skills` here
        // because the backward-compat custom-hat path predates
        // that pipeline and tests assert the absence of skill
        // injection for this branch.
        let _ = RUNTIME_DIAGNOSIS_ALERT_HEADER; // silence unused-import lint
        Some(with_diagnosis)
    }

    /// 2026-07-26-001 plan U2: structured preview of what
    /// `build_prompt` would inject for the given hat, *without*
    /// running the loop. Powers the `ralph inspect prompt` CLI
    /// (U3-U5).
    ///
    /// **Side effects — single-CLI-invocation safe, do NOT reuse
    /// across hot loops or shared state.** Although the preview
    /// itself does not publish events, calling it ultimately
    /// invokes `build_prompt` (via `preview_block_titles`), which:
    ///
    /// 1. Calls `handoff_tracker.on_hat_activated(hat_id)`,
    ///    clearing any pending handoff deadlines for that hat.
    /// 2. Calls `event_bus.take_pending(hat_id)`, consuming
    ///    any pending events addressed to the new hat.
    ///
    /// Within a single CLI invocation (`ralph inspect prompt`)
    /// the `EventLoop` instance owns its state and no externally
    /// visible mutation escapes — the CLI process exits
    /// immediately after, so cleared deadlines and consumed
    /// pending events are simply discarded. **Across invocations,
    /// or in any long-lived hot loop / shared `EventLoop`
    /// instance, these calls would silently bypass WRC-U4's 30s
    /// escalation gate and consume pending escalations**, so
    /// `prompt_preview` MUST NOT be called more than once per
    /// `EventLoop` instance. This contract is the same one
    /// `build_prompt` honours; any caller that survives multiple
    /// activations should drive the same code path through the
    /// orchestrator's hat-activation lifecycle, not through this
    /// inspector.
    ///
    /// Returns `None` when the hat is not registered.
    ///
    /// The `auto_inject` set is derived from the same
    /// `prepend_auto_inject_skills` pipeline that `build_prompt`
    /// uses, **without** invoking it. The
    /// `preview_characterization` test module pins the equivalence
    /// between this preview and the live prompt — any future drift
    /// in the auto-inject rules must fail those tests, not this
    /// preview API.
    pub fn prompt_preview(&mut self, hat_id: &HatId) -> Option<PromptPreview> {
        let config = self.config.clone();
        let preview = preview_prompt_for_config(&config, hat_id, |_| Vec::new());
        // Fill block_titles via the heavier build_prompt path now
        // that the immutable borrow on config is released.
        let mut preview = preview?;
        preview.block_titles = self.preview_block_titles(hat_id);
        Some(preview)
    }

    /// 2026-07-26-001 plan U2 R3: thin alias for `build_prompt`
    /// so callers (especially the U1 `inspect --full` JSON / human
    /// paths) can build only the prompt body without materializing
    /// a full `PromptPreview` struct.
    ///
    /// **Side effects — same contract as `prompt_preview`:** this
    /// is a direct wrapper around `build_prompt`, so it inherits
    /// the `handoff_tracker.on_hat_activated` clear and
    /// `event_bus.take_pending` consumption. Single CLI invocation
    /// is safe; do not call more than once per `EventLoop`
    /// instance. See `prompt_preview`'s doc for the full rationale
    /// (WRC-U4 30s escalation gate).
    pub fn build_prompt_body(&mut self, hat_id: &HatId) -> Option<String> {
        self.build_prompt(hat_id)
    }

    /// Block titles extracted from a dry prompt build for `hat_id`,
    /// in the order they appear. Implementation: call
    /// `build_prompt` and parse out `## …` headers from the
    /// resulting string. Build prompt is side-effect-free with
    /// respect to ledger state (it only clears handoff deadlines
    /// for the hat — see build_prompt doc comment), so the dry
    /// build here is safe to call from a read-only CLI.
    pub(crate) fn preview_block_titles(&mut self, hat_id: &HatId) -> Vec<String> {
        let Some(prompt) = self.build_prompt(hat_id) else {
            return Vec::new();
        };
        let mut titles: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for line in prompt.lines() {
            let Some(rest) = line.strip_prefix("## ") else {
                continue;
            };
            let trimmed = rest.trim().to_string();
            if seen.insert(trimmed.clone()) {
                titles.push(trimmed);
            }
        }
        titles
    }

    /// Inspect a batch of policy-accepted events and flip the
    /// `bootstrap_complete` / `bootstrap_failed` flags when the
    /// coordinator produces a terminal bootstrap handoff.
    ///
    /// Unit 3 (2026-06-16-002 plan) contract:
    /// - `coordinator` `work.ready` **without** a
    ///   `reviewed_task_id` field is the bootstrap handoff. It
    ///   marks `bootstrap_complete = true`.
    /// - `coordinator` `work.failed` is the explicit bootstrap
    ///   failure. It marks `bootstrap_failed = true` so the
    ///   runner can surface a precise reason rather than hang on
    ///   a missing `work.ready`.
    /// - Plan-gate `work.ready` (carrying `reviewed_task_id`) is
    ///   NOT a bootstrap event; the flag stays `false` so
    ///   step-advance handoffs from `review-synthesizer` keep
    ///   behaving as today.
    ///
    /// Both flags are reset to `false` in `initialize_with_topic`
    /// so a fresh `work.start` starts a new bootstrap window.
    /// Detection runs in the *accept* path so a rejected
    /// `work.ready` (e.g. payload contract violation) does NOT
    /// promote the flag — only events the runner actually
    /// processes count.
    pub(super) fn update_bootstrap_flags_from_accepted(&mut self, accepted: &[JsonlEvent]) {
        self.apply_bootstrap_flags_from_events(accepted);
    }

    /// Derive bootstrap gate state from a chronological event batch
    /// (accepted events or full events.jsonl replay on resume).
    pub(super) fn apply_bootstrap_flags_from_events(&mut self, events: &[JsonlEvent]) {
        for event in events {
            let hat = event.hat.as_deref().unwrap_or("");
            if hat != "coordinator" {
                continue;
            }
            if event.topic == "work.ready" && !self.state.bootstrap_complete {
                let is_bootstrap = event
                    .payload
                    .as_deref()
                    .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
                    .and_then(|v| v.get("reviewed_task_id").cloned())
                    .is_none();
                if is_bootstrap {
                    self.state.bootstrap_complete = true;
                }
            } else if event.topic == "work.failed" && !self.state.bootstrap_failed {
                self.state.bootstrap_failed = true;
            }
        }
    }

    /// Rebuild bootstrap flags after `task.resume` by scanning the loop's
    /// events file so guidance suppression does not leak across resume.
    pub(super) fn rebuild_bootstrap_flags_from_recorded_events(&mut self) {
        let path = self
            .loop_context
            .as_ref()
            .map(|ctx| ctx.events_path())
            .unwrap_or_else(|| self.event_reader.path().to_path_buf());
        if !path.exists() {
            return;
        }
        let mut reader = EventReader::new(&path);
        reader.reset();
        if let Ok(result) = reader.read_new_events() {
            self.state.bootstrap_complete = false;
            self.state.bootstrap_failed = false;
            self.apply_bootstrap_flags_from_events(&result.events);
        }
    }

    /// Stores guidance payloads, persists them to scratchpad, and prepares them for prompt injection.
    ///
    /// Guidance events are ephemeral in the event bus (consumed by `take_pending`).
    /// This method both caches them in memory for prompt injection and appends
    /// them to the scratchpad file so they survive across process restarts.
    pub(super) fn update_robot_guidance(&mut self, guidance_events: Vec<Event>) {
        if guidance_events.is_empty() {
            return;
        }

        // U2 (2026-06-18-004 plan, R2, KTD2): when
        // `suppress_human_guidance` is set, the loop persists
        // guidance to the scratchpad for audit but does NOT
        // cache it in `robot_guidance` (which is the source for
        // `apply_robot_guidance` → prompt injection). ce-executor-serial
        // opts into this so the active hat's prompt never sees
        // human.guidance text — the source of the perky-maple
        // P1-2 probe storm.
        let suppress = self.human_guidance_suppressed();
        // 2026-06-18-001 plan U7: 当 suppress=true 时,progress-steward
        // 仍能收到 `human.guidance` 内容——`suppress` 设计本意是防止
        // executor 探测风暴,误伤了依赖 guidance 的 steward。
        // 豁免条件:
        // - 事件显式 target=progress-steward(由 EventBus U2 修复路由到位)
        // - progress_steward.exempt_from_suppress_human_guidance=true(默认)
        //   且事件无 target 但当前在 steward 上下文(如下一轮 build_prompt
        //   时 hat_id=progress-steward)
        let exempt_steward_hat_id = self
            .config
            .event_loop
            .progress_steward
            .steward_hat_id
            .clone();
        // 2026-06-28-005: progress_steward.exempt_from_suppress_human_guidance
        // was deleted together with the suppress_human_guidance field.
        // Hard-coded to false here; this branch becomes dead once
        // update_robot_guidance itself is removed in a follow-up phase.
        let exempt_enabled = false;

        // Persist new guidance to scratchpad before caching
        self.persist_guidance_to_scratchpad(&guidance_events);

        // 2026-06-13-004 review fix (correctness F2, KTD-7 two-layer
        // dedup): the in-memory `robot_guidance` vec is the source
        // for the next `apply_robot_guidance` → prompt injection.
        // A redelivered or duplicated `human.guidance` event would
        // otherwise add the same payload twice to the prompt.
        // Dedup against the existing vec and within the current
        // batch; persist layer has already dedup'd against disk.
        // U2: when `suppress_human_guidance` is set, the loop
        // does NOT push the deduped payload into the in-memory
        // cache (which is the source for prompt injection via
        // `apply_robot_guidance` → `## ROBOT GUIDANCE` block).
        // The scratchpad persistence above already happened so
        // the event survives for audit.
        // 2026-06-18-001 plan U7: progress-steward 豁免。
        let mut seen_in_batch: std::collections::HashSet<String> = std::collections::HashSet::new();
        for event in guidance_events {
            // Move the payload out so we can dedup by owned String
            // without fighting the borrow checker. `payload` is
            // moved into `robot_guidance` when it survives the
            // dedup check; otherwise dropped.
            let payload = event.payload;
            if suppress {
                // U7 豁免:target=steward 或 exempt_enabled + 事件无
                // target 但下一轮将进入 steward 上下文,跳过 suppress
                let targeted_to_steward = event
                    .target
                    .as_ref()
                    .map(|t| t.as_str() == exempt_steward_hat_id)
                    .unwrap_or(false);
                if !(exempt_enabled && targeted_to_steward) {
                    // Drop the payload on the floor — already
                    // persisted above.
                    continue;
                }
                debug!("U7: human.guidance exempt from suppress for progress-steward");
            }
            if seen_in_batch.insert(payload.clone()) {
                let already = self.robot_guidance.iter().any(|p| p == &payload);
                if already {
                    debug!(
                        payload_len = payload.len(),
                        "U9 (KTD-7 in-memory layer): skipping guidance payload already cached for prompt"
                    );
                } else {
                    self.robot_guidance.push(payload);
                }
            } else {
                debug!(
                    payload_len = payload.len(),
                    "U9 (KTD-7 in-memory layer): skipping duplicate guidance payload in current batch"
                );
            }
        }
    }

    /// Appends human guidance entries to the scratchpad file for durability.
    ///
    /// Each guidance message is written as a timestamped markdown entry so it
    /// appears alongside the agent's own thinking and survives process restarts.
    ///
    /// When scratchpad is disabled for the current hat, persists to the global
    /// scratchpad path (guidance is cross-hat state). If global is also disabled,
    /// skips persistence.
    pub(super) fn persist_guidance_to_scratchpad(&self, guidance_events: &[Event]) {
        use std::io::Write;

        // When hat scratchpad is disabled, fall back to global scratchpad
        let scratchpad_path = if self.ralph.active_scratchpad().enabled {
            self.scratchpad_path()
        } else {
            if !self.config.core.scratchpad.enabled {
                debug!("Both hat and global scratchpad disabled, skipping guidance persistence");
                return;
            }
            self.global_scratchpad_path()
        };
        let resolved_path = if scratchpad_path.is_relative() {
            self.config.core.workspace_root.join(&scratchpad_path)
        } else {
            scratchpad_path
        };

        // Create parent directories if needed
        if let Some(parent) = resolved_path.parent()
            && !parent.exists()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            warn!("Failed to create scratchpad directory: {}", e);
            return;
        }

        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved_path)
        {
            Ok(f) => f,
            Err(e) => {
                warn!("Failed to open scratchpad for guidance persistence: {}", e);
                return;
            }
        };

        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        // 2026-06-13-004 U9 (P1-4): de-duplicate guidance payloads
        // against the on-disk scratchpad tail. The 2026-06-13
        // incident saw `Focus on error handling` written twice and
        // `Keep this in mind` written three times because
        // `persist_guidance_to_scratchpad` unconditionally appended.
        //
        // 2026-06-13 review fixes:
        //   - correctness F1: replaced `skip_while`+`filter` with a
        //     proper state machine so lines from sections AFTER
        //     the last `### HUMAN GUIDANCE` block are NOT
        //     collected as "existing payloads" (a line of text in
        //     `## NOTES` would otherwise be matched as a duplicate
        //     against a new guidance event with the same text).
        //   - reliability F5: extracted the 16 KB window size to
        //     a named constant with a comment explaining the
        //     capacity budget.
        //   - maintainability #20 (P2): the window is byte-bounded
        //     via `split_at` on bytes (UTF-8 safe via the byte
        //     check before the split). Char-based slicing would
        //     inflate to 64 KB worst-case for 4-byte CJK.
        const GUIDANCE_DEDUP_TAIL_BYTES: usize = 16 * 1024;
        let existing_payloads: std::collections::HashSet<String> = if resolved_path.exists() {
            std::fs::read_to_string(&resolved_path)
                .ok()
                .map(|content| {
                    // Byte-bounded tail; snap to a char boundary so
                    // the resulting &str is valid UTF-8 (no panic
                    // when the cut falls inside a multi-byte char).
                    let start = content.len().saturating_sub(GUIDANCE_DEDUP_TAIL_BYTES);
                    let tail_start = crate::text::floor_char_boundary(&content, start);
                    let tail = &content[tail_start..];
                    // State machine: collect body lines only while
                    // inside a `### HUMAN GUIDANCE` block. Stop at
                    // the next `### ` or `## ` header (any new
                    // section marker ends the current guidance
                    // block; `## NOTES` is the most common offender
                    // that would otherwise leak into the dedup
                    // HashSet). The block also ends at end-of-file.
                    let mut in_guidance = false;
                    let mut payloads = std::collections::HashSet::new();
                    for line in tail.lines() {
                        if line.starts_with("### HUMAN GUIDANCE") {
                            in_guidance = true;
                            continue;
                        }
                        if in_guidance && (line.starts_with("### ") || line.starts_with("## ")) {
                            in_guidance = false;
                            continue;
                        }
                        if in_guidance && !line.is_empty() {
                            payloads.insert(line.trim().to_string());
                        }
                    }
                    payloads
                })
                .unwrap_or_default()
        } else {
            std::collections::HashSet::new()
        };
        // 2026-06-13-004 review fix (F2 KTD-7): also dedup within
        // the current batch so a single persist call with two
        // identical payloads (e.g. a redelivered `human.guidance`
        // event) only writes the first one.
        let mut seen_in_batch: std::collections::HashSet<String> = std::collections::HashSet::new();
        for event in guidance_events {
            let payload = event.payload.as_str();
            if payload.is_empty() {
                continue;
            }
            if existing_payloads.contains(payload) || !seen_in_batch.insert(payload.to_string()) {
                debug!(
                    payload_len = payload.len(),
                    "U9: skipping duplicate guidance payload (already in scratchpad or in this batch)"
                );
                continue;
            }
            let entry = format!(
                "\n### HUMAN GUIDANCE ({})\n\n{}\n",
                timestamp, event.payload
            );
            if let Err(e) = file.write_all(entry.as_bytes()) {
                warn!("Failed to write guidance to scratchpad: {}", e);
            }
        }

        info!(
            count = guidance_events.len(),
            "Persisted human guidance to scratchpad"
        );
    }

    /// Injects cached guidance into the next prompt build.
    pub(super) fn apply_robot_guidance(&mut self, _hat_id: &HatId) {
        if self.robot_guidance.is_empty() {
            return;
        }

        // U2 (2026-06-18-004 plan, R2, KTD2): when
        // `suppress_human_guidance` is set, drain the in-memory
        // cache without pushing to `ralph.robot_guidance`. This
        // catches stale entries that pre-date the opt-in flip
        // (e.g. a config edit mid-loop) and ensures the active
        // hat prompt NEVER contains a `## ROBOT GUIDANCE` block
        // under suppress mode. The scratchpad still records the
        // raw guidance for audit.
        //
        // 2026-06-18-006 plan U5 (R5, KTD): also drain
        // `self.ralph.robot_guidance` so any guidance cached
        // BEFORE the suppress flip (e.g. a mid-loop config edit
        // that went non-suppress → suppress) does NOT leak into
        // the next prompt. Mirrors the isolated `build_prompt`
        // symmetry at line 4543 where `collect_robot_guidance()`
        // is paired with `clear_robot_guidance()` — the same
        // collector/clear invariant must hold on the suppress
        // path so a stale `## ROBOT GUIDANCE` block never survives
        // a `suppress_human_guidance` opt-in.
        // 2026-06-18-001 plan U7 (R-REP2 / R-D3):
        // suppress 模式下仍保留 progress-steward 的 guidance。
        // 既要保留"target=steward"的针对性 guidance（由
        // `update_robot_guidance` 已过滤保留），
        // 也要保留"无 target 但当前正在 build_prompt 的 hat_id
        // 就是 progress-steward"的兜底 guidance。
        // 豁免时仍要把 robot_guidance 推入 ralph,但**不**
        // 清空 `self.ralph.robot_guidance`——让 steward 在 suppress
        // 下能持续看到跨 turn 累积的 guidance。
        if self.human_guidance_suppressed() {
            // 2026-06-28-005: progress_steward.exempt_from_suppress_human_guidance
            // config field was deleted together with suppress_human_guidance.
            // The exempt branch is therefore unreachable: human_guidance_suppressed()
            // is a stub that always returns false, so the body below
            // becomes dead. Kept temporarily while update_robot_guidance
            // is scheduled for deletion in a follow-up phase.
            let _steward_hat_id = self
                .config
                .event_loop
                .progress_steward
                .steward_hat_id
                .as_str();
            // The exempt check used to read the now-deleted
            // exempt_from_suppress_human_guidance field; the helper
            // returns false unconditionally, so we fall through to
            // the suppress path uniformly.
            self.robot_guidance.clear();
            self.ralph.clear_robot_guidance();
            return;
        }

        self.ralph.set_robot_guidance(self.robot_guidance.clone());
        // P1 finding #4 (test isolation): clear the EventLoop-level
        // cache after the ralph copy has been set, so a subsequent
        // build_prompt call for a different hat does NOT re-inject
        // the same guidance. Without this, the guidance would leak
        // to any hat whose build_prompt is called in the same loop
        // iteration, breaking R9. The scratchpad persistence path
        // is independent (it writes to disk) and unaffected.
        self.robot_guidance.clear();
    }

    /// Prepends auto-injected skill content to the prompt.
    ///
    /// Injects current phase information into the prompt if phase support is enabled.
    ///
    /// When `event_loop.phase_config` is configured, this appends a "## Current Phase"
    /// section so the agent knows which phase (warmup / production) the loop is in.
    pub(super) fn inject_phase_into_prompt(&self, prompt: String) -> String {
        if self.config.event_loop.phase_config.is_none() {
            return prompt;
        }
        let phase = self.registry.current_phase();
        format!("{}\n## Current Phase\n\n{}\n", prompt, phase)
    }
}
