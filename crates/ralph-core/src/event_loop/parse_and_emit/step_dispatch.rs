//! EventLoop implementation region 9 — step dispatch helpers.

use super::super::*;
use tracing::{debug, warn};

impl EventLoop {
    /// U12 wiring (P0-1, 2026-06-27 review): drive the
    /// `StepCloseObligationStage` progress registry
    /// after each `process_parse_result` batch.
    ///
    /// Strategy: count `work.done` emits in
    /// `seen_topics` as `done`, and look up `total` from
    /// `flow.steps[i].total_units`. If the current step
    /// does not declare `total_units`, the call is a
    /// no-op (the stage stays fail-open — the pre-U12
    /// behaviour for presets that did not opt in).
    ///
    /// Idempotent: the underlying
    /// `StepCloseObligationStage::update_progress` is
    /// itself idempotent and rejects counter regressions
    /// silently (see the stage rustdoc).
    pub(super) fn drive_step_close_progress(&mut self) {
        let step_id = self.state.flow_lifecycle.current_step_id().to_string();
        if step_id.is_empty() {
            return;
        }
        let total_units = match self.flow_step_total_units(&step_id) {
            Some(n) => n,
            None => return,
        };

        let done = self
            .state
            .seen_topics
            .iter()
            .filter(|t| t.as_str() == "work.done")
            .count() as u32;
        self.stage_pipeline
            .update_step_close_progress(&step_id, done, total_units);
    }

    /// 2026-07-28-001 plan U3: settle the staged over-emit
    /// recovery intent. If at least one real business event
    /// (not a scope-violation replay, not a boundary
    /// diagnostic, not a default publish) committed this
    /// turn, the recovery is purely diagnostic — drop the
    /// pending `task.resume` injection so the legitimate
    /// handoff is not pre-empted. If zero committed, inject
    /// the bounded `task.resume` (still behind the existing
    /// breaker).
    pub(super) fn resolve_over_emit_recovery(
        &mut self,
        accepted_log_events: &[ralph_proto::Event],
    ) {
        let pending = match self.state.pending_over_emit_recovery.take() {
            Some(recovery) => recovery,
            None => return,
        };
        let committed_business = accepted_log_events
            .iter()
            .any(|event| is_commit_first_business_topic(event.topic.as_str()));
        if committed_business {
            tracing::debug!(
                hat = %pending.hat.as_str(),
                dropped_topic = %pending.dropped_topic,
                "U3: over-emit recovery bypassed because a business event already committed"
            );
            return;
        }
        let key = format!(
            "isolated_budget:{}:per_turn",
            crate::diagnosis::normalize_part(pending.hat.as_str())
        );
        let count = self.state.record_rejection_key(&key);
        if self.state.rejection_key_is_exhausted(&key) {
            warn!(
                key = %key,
                hat = %pending.hat.as_str(),
                dropped_topic = %pending.dropped_topic,
                count = count,
                "U3: isolated over-emit recovery breaker tripped; no task.resume injected"
            );
            return;
        }
        let free_form = format!(
            "Isolated mode dropped an extra business event ('{}') and zero business events committed this turn — only the FIRST business event per activation is kept. Re-emit EXACTLY ONE business event (the one you actually intend, e.g. plan.complete) and nothing else.",
            pending.dropped_topic
        );
        let payload = enrich_task_resume_payload(
            &free_form,
            "isolated_extra_business_event_dropped",
            Some(pending.hat.as_str()),
            Some(RejectionKind::ContractViolation),
        );
        // Plan 2026-08-10-001 U1: route the isolated
        // over-emit recovery through the unified publisher.
        // The `retry_key` is signed by hat + per-turn so the
        // same hat's extra-event recovery collapses into one
        // resume per turn.
        let loop_id_for_resume = self.current_loop_id();
        let loop_id_str = loop_id_for_resume.as_deref().unwrap_or("default");
        let activation_id = format!("resume:{}:{}", loop_id_str, self.state.iteration);
        let _ = crate::event_loop::resume_routing::task_resume_ingress(
            &mut self.bus,
            &self.registry,
            self.state.state_ledger.as_ref(),
            loop_id_str,
            &activation_id,
            pending.hat.as_str(),
            None,
            &format!(
                "isolated_budget:{}:per_turn",
                crate::diagnosis::normalize_part(pending.hat.as_str())
            ),
            payload,
        );
    }

    /// 2026-06-29-007 plan U1b: drive the `current_step`
    /// field transition after the unit_loop `total_units`
    /// have been reached. When `current_step ==
    /// "unit_loop"` and `work.done` count meets
    /// `total_units`, advance to `review_walk`. The
    /// helper is idempotent: re-entry while already on
    /// `review_walk` (or any non-`unit_loop` step) is a
    /// no-op.
    pub(super) fn drive_step_transition(&mut self) {
        let step_id = self.state.flow_lifecycle.current_step_id().to_string();
        if step_id != "unit_loop" {
            return;
        }
        let total_units = match self.flow_step_total_units(&step_id) {
            Some(n) => n,
            None => return,
        };
        let done = self
            .state
            .seen_topics
            .iter()
            .filter(|t| t.as_str() == "work.done")
            .count() as u32;
        if done < total_units {
            return;
        }
        if let Err(e) = self.state.flow_lifecycle.advance_to("review_walk") {
            tracing::warn!(
                error = %e,
                "flow_lifecycle.advance_to(review_walk) failed; staying on unit_loop"
            );
        }
    }

    /// 2026-07-02-004 plan milestone B (U5/U6): enforce precheck
    /// gate hard-gate semantics and dispatch rejections (resume vs.
    /// exhaustion).  U5 synthesizes `<X>.rejected` when the gate
    /// hat is silent or ambiguous; U6 routes failures through the
    /// correction + `task.resume` pipeline (R5 / AE3).
    pub(super) fn drive_precheck_gate_obligation(&mut self, accepted: &[ralph_proto::Event]) {
        use crate::event_loop::precheck_gate_enforcement as gate;
        use ralph_proto::HatId;
        use std::collections::HashSet;

        let precheck_cfg = match self.config.event_loop.precheck.as_ref() {
            Some(p) if p.enabled && !p.rules.is_empty() => p.clone(),
            _ => return,
        };
        if !crate::config::precheck_runtime_enabled() {
            return;
        }

        let loop_id = self
            .loop_context
            .as_ref()
            .and_then(|c| c.loop_id())
            .unwrap_or("default")
            .to_string();

        // U5: silent / ambiguous gate → synthetic `<X>.rejected`.
        let synthetics = gate::collect_synthetic_precheck_rejections(
            &self.state.hat_obligations,
            accepted,
            |topic| precheck_cfg.rules.get(topic).map(|r| r.prompt.len()),
        );
        let mut synthesized_gates: HashSet<String> = HashSet::new();
        for synthetic in synthetics {
            synthesized_gates.insert(synthetic.gate_hat_id.clone());
            let gate_hat = HatId::new(&synthetic.gate_hat_id);
            self.state
                .discharge_hat_obligation(&gate_hat, &synthetic.rejected_topic);
            self.dispatch_precheck_rejection(
                &loop_id,
                &precheck_cfg,
                &synthetic.gate_hat_id,
                &synthetic.guarded_topic,
                &synthetic.payload_json,
            );
        }

        for event in accepted {
            let source_hat = match gate::resolve_gate_hat_for_emit(event, &precheck_cfg.rules) {
                Some(id) => HatId::new(id),
                None => continue,
            };
            if !gate::is_gate_hat(source_hat.as_str()) {
                continue;
            }
            if synthesized_gates.contains(source_hat.as_str()) {
                continue;
            }
            let topic_str = event.topic.as_str();

            if let Some(guarded) = gate::gate_topic(source_hat.as_str())
                && topic_str == guarded
            {
                self.precheck_retries.record_pass(&loop_id, guarded);
                self.state.discharge_hat_obligation(&source_hat, topic_str);
                continue;
            }

            let guarded = match topic_str.strip_suffix(".rejected") {
                Some(s) => s,
                None => continue,
            };
            let hat_guarded = match gate::gate_topic(source_hat.as_str()) {
                Some(g) => g,
                None => continue,
            };
            if hat_guarded != guarded {
                continue;
            }

            let Some(_rule) = precheck_cfg.rules.get(guarded) else {
                continue;
            };

            self.state.discharge_hat_obligation(&source_hat, topic_str);
            self.dispatch_precheck_rejection(
                &loop_id,
                &precheck_cfg,
                source_hat.as_str(),
                guarded,
                event.payload.as_str(),
            );
        }
    }

    /// U6 closure for one `<X>.rejected` (LLM or synthetic).
    pub(super) fn dispatch_precheck_rejection(
        &mut self,
        loop_id: &str,
        precheck_cfg: &crate::config::PrecheckConfig,
        gate_hat_id: &str,
        guarded: &str,
        rejected_payload_json: &str,
    ) {
        use crate::event_loop::precheck_gate_runner as runner;
        use crate::event_loop::rejection::enrich_task_resume_payload_full;
        use crate::preset::engine::gates::RejectionKind;
        use ralph_proto::HatId;

        let rule = match precheck_cfg.rules.get(guarded) {
            Some(r) => r,
            None => return,
        };
        let rejection_count = self.precheck_retries.record_rejection(loop_id, guarded);

        let params = runner::DispatchParams {
            loop_id,
            topic: guarded,
            target_hat: rule.on_fail.target.as_str(),
            retry_budget: rule.on_fail.retry_budget,
            on_exhausted: rule.on_fail.on_exhausted.as_str(),
            rejection_count,
            rejected_payload_json,
        };
        let outcome = runner::dispatch_rejection(&params);
        match outcome {
            runner::DispatchOutcome::Resume {
                target_hat,
                new_count,
                ..
            } => {
                let message =
                    runner::format_precheck_failure_message(guarded, rejected_payload_json);
                let mut rejection = Rejection {
                    stage: RejectionStage::Policy,
                    source_hat: Some(gate_hat_id.to_string()),
                    business_hat: None,
                    topic: guarded.to_string(),
                    violation: message.clone(),
                    retry_key: String::new(),
                    retry_eligible: true,
                    non_retryable_reason: None,
                    target_hat: Some(target_hat.clone()),
                    original_event_id: None,
                    original_ts: None,
                    kind: Some(RejectionKind::ContractViolation),
                    duplicate_work_done_hint: None,
                    seen_count: None,
                };
                rejection.retry_key = rejection.compute_retry_key();
                let mut ctx = crate::correction::emit_correction_context(
                    self.state.state_ledger.as_mut(),
                    &rejection,
                    new_count,
                    Some(self.config.core.workspace_root.as_path()),
                    &mut self.state.prompt_context,
                );
                // U2 (plan 2026-08-06-001, R1/R5): enrich the
                // precheck correction with structured evidence.
                // Synthetic rejections get an explicit
                // `gate_silent_or_ambiguous` marker; LLM
                // rejections get per-check `unchecked`
                // observations so the hat cannot mistake them
                // for a clean "the check failed" verification.
                // U3 (plan 2026-08-17-1841, R2/R3/D2/D3): thread
                // the precheck rule's optional `recovery_guidance`
                // into the evidence so the U2 correction renderer
                // surfaces the preset-supplied common / by_check
                // items at the target hat's prompt.  When no rule
                // is registered (e.g. legacy / desugared presets)
                // the function returns the same evidence shape
                // with `guidance = None` and `failed_check_keys`
                // still populated.
                let rule_ref = self
                    .config
                    .event_loop
                    .precheck
                    .as_ref()
                    .and_then(|pc| pc.rules.get(guarded));
                if let Some(evidence) =
                    runner::build_precheck_evidence(guarded, rejected_payload_json, rule_ref)
                {
                    ctx = ctx.with_feedback_kind(crate::correction::FeedbackKind::Semantic);
                    ctx = ctx.with_evidence(evidence);
                    // Replace the entry emit_correction_context
                    // just pushed (legacy mechanical place-holder)
                    // with the upgraded semantic + evidence one.
                    // U2 (AC4): use rfind so we always upgrade the
                    // freshly-pushed entry even when multiple entries
                    // share the same (retry_key, topic).
                    if let Some(last) = self
                        .state
                        .prompt_context
                        .correction_blocks
                        .iter_mut()
                        .rfind(|c| c.retry_key == ctx.retry_key && c.topic == ctx.topic)
                    {
                        *last = ctx.clone();
                    }
                }

                let allowed_topics = self
                    .registry
                    .get_config(&HatId::new(&target_hat))
                    .map(|cfg| cfg.publishes.clone())
                    .unwrap_or_default();
                let resume_payload = enrich_task_resume_payload_full(
                    &message,
                    "precheck_rejected",
                    Some(&target_hat),
                    Some(RejectionStage::Policy),
                    Some(RejectionKind::ContractViolation),
                    &allowed_topics,
                );
                tracing::info!(
                    loop_id = %loop_id,
                    gate = %gate_hat_id,
                    topic = %guarded,
                    target_hat = %target_hat,
                    count = new_count,
                    "U6: precheck rejection within budget; injecting correction + task.resume"
                );
                // Plan 2026-08-10-001 U1: route the precheck
                // correction through the unified publisher. The
                // `retry_key` is signed by `gate` + `guarded`
                // so duplicate precheck rejections for the same
                // gate/topic collapse into a single resume.
                let loop_id_for_resume = self.current_loop_id();
                let loop_id_str = loop_id_for_resume.as_deref().unwrap_or("default");
                let activation_id = format!("resume:{}:{}", loop_id_str, self.state.iteration);
                let decision = crate::event_loop::resume_routing::task_resume_ingress(
                    &mut self.bus,
                    &self.registry,
                    self.state.state_ledger.as_ref(),
                    loop_id_str,
                    &activation_id,
                    &target_hat,
                    None,
                    &format!("precheck:{}:{}", gate_hat_id, guarded),
                    resume_payload,
                );
                if let crate::event_loop::resume_routing::ResumeDecision::Block { reason } =
                    &decision
                {
                    tracing::warn!(
                        loop_id = %loop_id,
                        gate = %gate_hat_id,
                        topic = %guarded,
                        target_hat = %target_hat,
                        ?reason,
                        "precheck correction blocked (no safe target)"
                    );
                }
                self.state
                    .redispatch_hat_obligation(&HatId::new(target_hat));
            }
            runner::DispatchOutcome::Exhausted { topic, reason } => {
                tracing::warn!(
                    loop_id = %loop_id,
                    gate = %gate_hat_id,
                    topic = %guarded,
                    on_exhausted = %topic,
                    reason = %reason,
                    "U6: precheck retry budget exhausted; escalating to on_exhausted"
                );
                let payload = runner::build_exhausted_payload(&topic, &reason);
                let blocked = ralph_proto::Event::new(topic.clone(), payload)
                    .with_source(HatId::new(gate_hat_id));
                self.state.record_event(&blocked);
                self.bus.publish(blocked);
                self.terminal_event_emitted = true;
            }
            runner::DispatchOutcome::Pass => {}
        }
    }

    /// Resolve `FlowDeclaration.steps[i].total_units` for
    /// the step whose id matches `step_id`. Returns
    /// `None` when the step is not declared or did not
    /// opt into `total_units`.
    ///
    /// 2026-06-28-002 U6: fix-unit steps (`fix-{NN}`) that
    /// did not declare `total_units` fall back to the
    /// `tasks.jsonl` record count for matching fix-units.
    /// Without this, `StepCloseObligationStage` stays
    /// fail-open for fix-unit flows because the registry
    /// never knows the total. Non-fix steps retain the
    /// pre-U6 strict `None` semantics so other presets are
    /// not affected.
    pub(super) fn flow_step_total_units(&self, step_id: &str) -> Option<u32> {
        if let Some(n) = self.flow_step_totals.get(step_id).copied() {
            return Some(n);
        }
        if step_id.starts_with("fix-") {
            return self.count_fix_unit_tasks(step_id);
        }
        None
    }

    /// 2026-06-28-002 U6: count `tasks.jsonl` records whose
    /// task_key matches the fix-unit shape
    /// `ce-executor:*:{step_id}:*` so the step-close progress
    /// stage can satisfy its total even when the preset omits
    /// `total_units` in `FlowDeclaration.steps[i]`.
    pub(super) fn count_fix_unit_tasks(&self, step_id: &str) -> Option<u32> {
        use crate::task_store::TaskStore;
        let path = self.tasks_path();
        let store = match TaskStore::load(&path) {
            Ok(s) => s,
            Err(_) => return None,
        };
        let prefix = "ce-executor:".to_string();
        let needle = format!(":{step_id}:");
        let count = store
            .all()
            .iter()
            .filter(|t| {
                t.key
                    .as_deref()
                    .map(|k| k.starts_with(&prefix) && k.contains(&needle))
                    .unwrap_or(false)
            })
            .count() as u32;
        if count == 0 { None } else { Some(count) }
    }

    /// 2026-06-26 plan U4: discharge hat obligations for any accepted
    /// business event. Centralised here so the obligation queue is
    /// kept in lock-step with the bus — every accepted event
    /// immediately removes the obligation for the hat that emitted
    /// it (if the topic was one the hat owed).
    ///
    /// Returns the number of obligations discharged, mostly useful
    /// for the diagnostics collector. The discharge is idempotent:
    /// if no obligation is open, `discharge_hat_obligation` is a
    /// silent no-op (the emit is a side-effect, not the expected
    /// business event).
    pub fn discharge_obligations_for_accepted(&mut self, events: &[Event]) -> usize {
        let mut discharged = 0;
        for event in events {
            let Some(hat_id) = event.source.as_ref() else {
                continue;
            };
            if self
                .state
                .discharge_hat_obligation(hat_id, event.topic.as_str())
            {
                discharged += 1;
            }
        }
        discharged
    }

    /// Process events from JSONL, partitioning wave events from regular events.
    ///
    /// Wave events (those with `wave_id` set and targeting a concurrent hat) are
    /// extracted and returned separately. Regular events go through the full
    /// backpressure pipeline via `process_parse_result`.
    pub fn process_events_from_jsonl_with_waves(
        &mut self,
    ) -> std::io::Result<ProcessedEventsWithWaves> {
        let result = self.event_reader.read_new_events()?;
        // 2026-06-16-001 U1: reset the per-turn stall-detector
        // flag at the start of each read so the helper can
        // observe whether THIS turn admitted a business event.
        // Mirror of process_events_from_jsonl() line 6349.
        self.state.stall_detector_had_events = false;

        // Partition: wave dispatch events vs regular events.
        // Only events that target a concurrent hat (concurrency > 1) are wave dispatches.
        // Wave *results* (e.g. review.done) have wave_id set but should be treated as
        // regular events so they reach the bus and trigger downstream hats (e.g. aggregator).
        //
        // Uses find_by_trigger + get_config — the same resolution path as
        // detect_wave_events — to ensure partition and detection agree.
        let (wave_events, regular_events): (Vec<_>, Vec<_>) =
            result.events.into_iter().partition(|e| {
                e.wave_id.is_some()
                    && self
                        .registry
                        .find_by_trigger(e.topic.as_str())
                        .and_then(|hat_id| self.registry.get_config(hat_id))
                        .is_some_and(|hat_config| hat_config.concurrency > 1)
            });

        // --- Origin guard: validate wave event provenance before policy validation ---
        // Wave dispatch events bypass process_parse_result, so origin validation must
        // run here to prevent forged wave events from reaching wave execution.
        let (wave_events, _origin_rejections) = filter_events_by_origin(
            wave_events,
            &self.registry,
            &self.config.event_loop.cancellation_promise,
            &self.config.event_loop.completion_promise,
        );

        // --- Topic format check (U5 / R9) for wave events ---
        // Only active when event_policy is enabled AND hats are configured.
        let wave_events = if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
            && !self.config.hats.is_empty()
        {
            let allowed_topics: std::collections::HashSet<String> =
                crate::event_policy::build_allowed_topics(
                    &self.config.hats,
                    &self.config.event_loop.completion_promise,
                    self.config.event_loop.event_policy.as_ref(),
                );
            let (wave_events_ok, wave_rejections): (Vec<_>, Vec<_>) =
                wave_events.into_iter().partition(|event| {
                    if crate::event_policy::is_system_topic(&event.topic) {
                        return true;
                    }
                    crate::event_policy::check_topic_format(&event.topic, &allowed_topics).is_none()
                });
            if !wave_rejections.is_empty() {
                // R10: same behavior as the regular-event path —
                // publish the legacy diagnostic AND write a recovery
                // journal entry so `ralph diagnose` can surface it.
                let allowed_list: Vec<String> = allowed_topics.iter().cloned().collect();
                for event in &wave_rejections {
                    warn!(
                        topic = %event.topic,
                        hat = ?event.hat,
                        "Topic format rejection (wave): unknown topic not in whitelist"
                    );
                    let diagnostic = Event::new(
                        "event.topic_format.rejected",
                        format!(
                            "TOPIC_FORMAT_REJECTED: '{}' is not in the whitelist of known topics. \
                             This event will not be retried.",
                            event.topic
                        ),
                    );
                    self.bus.publish(diagnostic);
                    Self::log_topic_format_rejection(
                        self,
                        event.topic.as_str(),
                        event.hat.as_deref(),
                        &allowed_list,
                    );
                }
            }
            wave_events_ok
        } else {
            wave_events
        };
        // --- End topic format check (wave) ---

        // --- Event policy validation for wave events ---
        // Wave dispatch events are partitioned before process_parse_result, so they
        // must undergo policy validation here to avoid bypassing schema checks.
        //
        // U1 (2026-06-13-001): capture the policy_rejections vector and the
        // raw count of events that entered this validation step. These two
        // pieces of evidence are surfaced on `ProcessedEventsWithWaves` so
        // the runner can:
        //   1. Avoid the false `missing_event_gate` (the agent DID try to
        //      emit; the wave fan-out was simply blocked by a missing
        //      required field such as `depth`).
        //   2. Emit a recovery envelope naming the failing topic / field /
        //      wave_id so `ralph diagnose` attributes the failure to
        //      `payload_contract` rather than a silent missing emission.
        let mut wave_raw_count: usize = 0;
        let mut wave_policy_rejections: Vec<crate::event_policy::PolicyRejection> = Vec::new();
        let wave_events = if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            wave_raw_count = wave_events.len();
            let mut policy_state: PolicyRuntimeState =
                self.state.policy_runtime_state.take().unwrap_or_default();
            let mut review_step_tracker = std::mem::take(&mut self.state.review_step_tracker);
            let mut state_ledger = std::mem::take(&mut self.state.state_ledger);
            let mut wave_violation: Option<crate::payload_contract::PayloadContractViolation> =
                None;
            let mut wave_rejections: Vec<crate::event_policy::PolicyRejection> = Vec::new();
            let mut hold_reason: Option<String> = None;

            let view = crate::preset::engine::protocol::ProtocolView::from_event_loop(
                &self.config.event_loop,
            );
            use crate::validation::{EventPolicyRule, ValidationContext, ValidationRule};
            let rule = EventPolicyRule;

            let mut accepted_wave_events: Vec<JsonlEvent> = Vec::with_capacity(wave_events.len());
            for evt in &wave_events {
                let mut snapshot = crate::state::LedgerSnapshot::cold_start();
                let r = {
                    let mut ctx = ValidationContext::new(&mut snapshot)
                        .with_policy_runtime_state(&mut policy_state)
                        .with_review_step_tracker(&mut review_step_tracker)
                        .with_payload_contract_violation(&mut wave_violation)
                        .with_policy_rejections(&mut wave_rejections);
                    rule.validate(&view, &mut ctx, evt)
                };
                if r.accepted {
                    if r.stage == crate::validation::ValidationStage::EventPolicy
                        && r.reason_code.as_deref()
                            == Some(crate::validation::ReasonCode::EVENT_POLICY_WARNING)
                    {
                        let msg = format!(
                            "Policy warning for '{}': {}",
                            evt.topic,
                            r.correction_hint.as_deref().unwrap_or("")
                        );
                        self.bus.publish(Event::new("event.policy_warning", msg));
                    }
                    accepted_wave_events.push(evt.clone());
                    continue;
                }
                match r.reason_code.as_deref() {
                    Some(crate::validation::ReasonCode::EVENT_POLICY_COMPLETION_BLOCKED) => {
                        let msg = r
                            .correction_hint
                            .clone()
                            .unwrap_or_else(|| format!("Completion guard blocked '{}'", evt.topic));
                        self.bus
                            .publish(Event::new("event.completion.blocked", msg));
                    }
                    Some(crate::validation::ReasonCode::EVENT_POLICY_COMPLETION_IGNORED) => {
                        let msg = r
                            .correction_hint
                            .clone()
                            .unwrap_or_else(|| format!("Completion guard ignored '{}'", evt.topic));
                        self.bus
                            .publish(Event::new("event.completion.ignored", msg));
                    }
                    Some(
                        crate::validation::ReasonCode::EVENT_POLICY_BLOCKED
                        | crate::validation::ReasonCode::EVENT_POLICY_IGNORED,
                    ) => {}
                    Some(crate::validation::ReasonCode::EVENT_POLICY_HOLD) => {
                        hold_reason = r
                            .correction_hint
                            .clone()
                            .or_else(|| Some(format!("Event '{}' violates policy", evt.topic)));
                        let reason = format!(
                            "{}:{}",
                            r.stage.as_str(),
                            r.reason_code.as_deref().unwrap_or("rejected"),
                        );
                        publish_correction_via_context(
                            &mut self.bus,
                            &mut self.state,
                            state_ledger.as_mut(),
                            evt,
                            &reason,
                            policy_finding_for_topic(&wave_rejections, evt.topic.as_str()),
                        );
                    }
                    _ => {
                        let reason = format!(
                            "{}:{}",
                            r.stage.as_str(),
                            r.reason_code.as_deref().unwrap_or("rejected"),
                        );
                        publish_correction_via_context(
                            &mut self.bus,
                            &mut self.state,
                            state_ledger.as_mut(),
                            evt,
                            &reason,
                            policy_finding_for_topic(&wave_rejections, evt.topic.as_str()),
                        );
                    }
                }
            }

            self.state.state_ledger = state_ledger;
            self.state.review_step_tracker = review_step_tracker;
            self.state.policy_runtime_state = Some(policy_state);

            wave_policy_rejections = wave_rejections;

            // Write hold artifact if policy hold was triggered.
            if let Some(ref reason) = hold_reason
                && let Err(e) = self.write_hold_artifact(Some(reason))
            {
                warn!(error = %e, "Failed to write hold artifact");
            }

            // Post-process recoverable rejection budget.
            use crate::event_policy::ReasonClass;
            for rejection in &wave_policy_rejections {
                if let Some(ref class) = rejection.reason_class {
                    if matches!(class, ReasonClass::SemanticGateViolation) {
                        continue;
                    }
                    let hat = rejection.source_hat.as_deref().unwrap_or("unknown");
                    let (count, exhausted) = self.state.record_recoverable_rejection_key(
                        hat,
                        &rejection.topic,
                        class.as_str(),
                    );
                    if exhausted {
                        self.state
                            .recoverable_exhaustion_buffer
                            .push(RecoverableExhaustion {
                                hat: hat.to_string(),
                                topic: rejection.topic.clone(),
                                reason_class: *class,
                                count,
                            });
                    }
                }
            }

            // U1: when every wave event was rejected, write a recovery envelope.
            if wave_raw_count > 0 && accepted_wave_events.is_empty() {
                Self::log_wave_policy_blocked_envelope(
                    self,
                    &wave_policy_rejections,
                    wave_raw_count,
                );
            }

            accepted_wave_events
        } else {
            wave_events
        };

        // Update policy runtime state for wave events that passed validation
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            let policy_state = self
                .state
                .policy_runtime_state
                .get_or_insert_with(PolicyRuntimeState::default);
            for event in &wave_events {
                if policy_config.terminal_topics.contains(&event.topic) {
                    policy_state.terminal_observed = true;
                }
            }
        }

        if !wave_events.is_empty() {
            debug!(
                wave_count = wave_events.len(),
                regular_count = regular_events.len(),
                "Partitioned wave events from regular events"
            );
        }

        // --- Isolated scope enforcement for wave events (U4 / A3) ---
        // Wave partition bypasses `process_parse_result`, so the regular
        // isolated-scope check does not run on wave events. We re-apply
        // it here post-partition. Per KTD-U4-1 the same
        // `isolated_publish_allowed` predicate is used; per KTD-U4-2 a
        // single isolated activation may emit at most one distinct
        // `wave_id` — additional distinct wave_ids in the same read
        // batch are typed as `IsolatedMultipleBusinessEmissions`.
        let wave_events = if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && let Some(isolated_hat) = self.state.current_isolated_hat.clone()
            && !wave_events.is_empty()
        {
            self.enforce_wave_isolated_scope(wave_events, &isolated_hat)?
        } else {
            wave_events
        };
        // --- End isolated scope enforcement for wave events ---

        // Delegate regular events to the full pipeline (backpressure, scope
        // enforcement, plan detection, etc.)
        let regular_result = crate::event_reader::ParseResult {
            events: regular_events,
            malformed: result.malformed,
        };
        let processed = self.process_parse_result(regular_result)?;

        Ok(ProcessedEventsWithWaves {
            processed,
            wave_events,
            wave_policy_rejections,
            wave_raw_count,
        })
    }
}
