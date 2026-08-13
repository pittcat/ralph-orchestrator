//! EventLoop implementation region 10.

use super::*;

impl EventLoop {
    /// Checks if output contains a completion event from Ralph.
    ///
    /// Completion must be emitted as an `<event>` tag, not plain text.
    pub fn check_ralph_completion(&self, output: &str) -> bool {
        let events = EventParser::new().parse(output);
        events
            .iter()
            .any(|event| event.topic.as_str() == self.config.event_loop.completion_promise)
    }

    /// Publishes the loop.terminate system event to observers.
    ///
    /// Per spec: "Published by the orchestrator (not agents) when the loop exits."
    /// This is an observer-only event—hats cannot trigger on it.
    ///
    /// Returns the event for logging purposes.
    pub fn publish_terminate_event(&mut self, reason: &TerminationReason) -> Event {
        let elapsed = self.state.elapsed();
        let duration_str = format_duration(elapsed);

        let payload = format!(
            "## Reason\n{}\n\n## Status\n{}\n\n## Summary\n- Iterations: {}\n- Duration: {}\n- Exit code: {}",
            reason.as_str(),
            termination_status_text(reason),
            self.state.iteration,
            duration_str,
            reason.exit_code()
        );

        let event = Event::new("loop.terminate", &payload);

        // Publish to bus for observers (but no hat can trigger on this)
        self.bus.publish(event.clone());

        info!(
            reason = %reason.as_str(),
            iterations = self.state.iteration,
            duration = %duration_str,
            "Wrapping up: {}. {} iterations in {}.",
            reason.as_str(),
            self.state.iteration,
            duration_str
        );

        event
    }

    /// Publish an event to the event bus.
    ///
    /// R6/U2: ralph pseudo-hat may only publish control topics. This
    /// gate mirrors the `process_events_from_jsonl` check so that
    /// orchestrator-internal publish paths (e.g. `inject_fallback_event`)
    /// and external callers (`runner.rs`) share the same boundary.
    pub fn publish_event(&mut self, event: Event) {
        if let Some(ref hat) = event.source
            && hat.as_str() == "ralph"
        {
            let topic = event.topic.as_str();
            // P1-12: uses prefix match so future `ralph.*` topics are
            // recognized without updating the constant list.
            if !crate::event_origin::is_ralph_control_topic(topic) {
                warn!(
                    topic = %topic,
                    "ralph hat business topic rejected in publish_event: ralph may only publish control topics"
                );
                let violation = Event::new(
                    "event.isolation.boundary_violation",
                    format!(
                        "{{\"hat\":\"ralph\",\"topic\":\"{}\",\"violation\":\"ralph_business_topic_rejected: ralph hat may only publish control topics\"}}",
                        topic
                    ),
                );
                self.bus.publish(violation);
                return;
            }
        }

        // 2026-07-02 P0: `review.dimension.ready` idempotency
        // dedup must run BEFORE the emit-gate facade so a
        // resume-replayed duplicate (e.g. review-coordinator
        // re-sending `adversarial` after a stall_recovery
        // `task.resume` — observed in the 2026-07-01
        // ralph-e2e run, recovery.jsonl iter 24) is rejected
        // as `DuplicateWorkDone` and the original retry_key
        // path is preserved. The dedup lives in
        // `event_policy::validate_event_with_hat`
        // (event_policy.rs:1115-1169) but the policy module
        // is only invoked from unit tests today — hat-channel
        // output bypasses it. This call wires the dedup into
        // the production emit path with no schema-side
        // change; on RejectWithResume the event is routed to
        // the repair stream (the same sink the stage pipeline
        // uses for `AcceptRepairStream`) and never reaches
        // the bus, so a `task.resume` retry does not
        // re-introduce a duplicate.
        if event.topic.as_str() == "review.dimension.ready"
            && let Some(ref mut policy_state) = self.state.policy_runtime_state
            && let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            use crate::event_policy::{PolicyDecision, validate_event_with_hat};
            let payload_str = event.payload.as_str();
            let decision = validate_event_with_hat(
                event.topic.as_str(),
                Some(payload_str),
                policy_config,
                policy_state,
                event.source.as_ref().map(|h| h.as_str()),
            );
            if let PolicyDecision::RejectWithResume(_) | PolicyDecision::Hold(_) = decision {
                tracing::info!(
                    topic = %event.topic,
                    plan = %event.source.as_ref().map(|s| s.as_str()).unwrap_or(""),
                    "P0: review.dimension.ready rejected by idempotency dedup; routing to repair stream"
                );
                self.record_repair_event(&event);
                return;
            }
        }

        // U6 (2026-06-27 mechanism foundation): every event
        // that survives the ralph-boundary check must also
        // pass through the emit-gate facade (U1/U2). The
        // facade combines `StagePipeline::run` with the
        // `is_repair_topic` routing hint so the bus never
        // sees a repair topic and a rejected event lands in
        // `record_stage_rejection`.
        let mut stage_ctx = self.build_stage_context_for(&event);
        // The facade owns the routing decision; we only
        // need to mirror the three outcomes into the
        // appropriate sink.
        let outcome = crate::event_loop::emit_gate::evaluate_emit_gate(&mut stage_ctx, &event);
        match outcome {
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptMainBus => {
                // U10 (2026-06-27-002 plan completion): if
                // the topic is in `terminal_emits`, write
                // the loop-termination record so the
                // dispatcher knows the loop has reached
                // its natural end. Only `LOOP_COMPLETE`
                // is in the default set after U9 retired
                // the legacy `report.done` mirror.
                if self.stage_pipeline.is_terminal(&event) {
                    self.write_loop_termination_record(&event);
                }
                self.bus.publish(event.clone());
                self.diagnose_plan_complete_channel(
                    &event,
                    crate::event_loop::phase_authority::diagnosis::Channel::Main,
                );
                // U8 (plan 2026-07-30-004): only Business / Recovery
                // dispositions advance flow; diagnostic / loop-control
                // topics never reach phase authority.
                if crate::event_loop::disposition::classify(event.topic.as_str()).advances_flow() {
                    self.apply_phase_authority_on_accepted(&event);
                }
            }
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptRepairStream => {
                // U7 (2026-06-27-002 plan completion): the
                // U6 repair sink writes the envelope to
                // `.ralph/recovery.jsonl`. The bus NEVER
                // sees a repair topic.
                self.record_repair_event(&event);
            }
            crate::event_loop::emit_gate::EmitGateOutcome::Reject(reject) => {
                // The facade carries the StageReject; route
                // through the existing recovery envelope.
                self.record_stage_rejection(&event, &reject);
            }
        }
    }

    /// U6 (2026-06-27 mechanism foundation): build the
    /// StageContext consumed by every stage in the emit-time
    /// pipeline AND the U1 emit-gate facade. Reads the loop
    /// id from loop_context, the current step id from
    /// FlowLifecycleRegistry (falling back to "unit_loop"),
    /// and the expected version from the shared idempotent
    /// log. StageContext borrows a static RepairStateMachine
    /// stub; every stage currently ignores it.
    ///
    /// The `pipeline` field is wired in U1 so the
    /// `evaluate_emit_gate` facade can run the pipeline
    /// from inside the gate without the caller having to
    /// thread the pipeline separately.
    pub(super) fn build_stage_context_for(
        &mut self,
        event: &Event,
    ) -> crate::event_loop::stage_pipeline::StageContext<'_> {
        use crate::event_loop::stage_pipeline::{FlowStep, StageContext};
        let loop_id = self
            .loop_context()
            .and_then(|c| c.loop_id())
            .unwrap_or("default")
            .to_string();
        // 2026-06-28 plan U4: prefer the plan-mode step
        // (advanced by U4's transition logic) over the
        // wave-phase fallback. When the preset has no
        // `mechanism.flow`, `current_plan_step` is the empty
        // string and the wave-phase value takes over so the
        // existing tests keep working.
        let step_id = if self.current_plan_step.is_empty() {
            self.state.flow_lifecycle.current_step_id().to_string()
        } else {
            self.current_plan_step.clone()
        };
        let expected_version = self
            .idempotent_log
            .lock()
            .map(|log| log.version())
            .unwrap_or(0);
        let _ = event;
        // P1-5 (2026-06-27 adversarial review):
        // hand the per-task repair state machine
        // registry to the stage context so the
        // `RepairDispatchStage` can advance the
        // per-`task_key` budget. The previous
        // design shared one machine for every
        // repair event, which violated R2.
        StageContext::with_pipeline(
            FlowStep::new(step_id),
            loop_id,
            expected_version,
            &mut self.repair_state_machines,
            &self.stage_pipeline,
        )
    }

    /// U3 (2026-06-27-002 plan completion): publish-time
    /// gate. The first gate pass (in
    /// `apply_emit_gate` over `event_reader::Event`)
    /// only recorded the recovery envelope / repair-sink
    /// side effect; this second pass decides whether
    /// the validated event reaches the main bus.
    ///
    /// P0-1 (2026-06-27 adversarial review): the
    /// previous implementation re-ran the stage
    /// pipeline here, which double-advanced the
    /// `RepairStateMachine` for repair topics (the
    /// pipeline mutates `ctx.repair_state` in place).
    /// To preserve the per-task budget we now reuse
    /// the outcome from the first pass instead of
    /// running the pipeline twice. The first-pass
    /// outcome is stashed in `validated_gate_outcomes`
    /// (keyed by the JSONL event's index — see
    /// P1-1 (2026-07-01-002 audit): when the coordinator emits a
    /// `work.ready(fix-XX)` whose `fix-XX` is **not** in the
    /// projector-known chain, reject it with a synthetic
    /// `ExecutionContractFinding` so the downstream rejection
    /// machinery publishes a `task.resume` with the right
    /// provenance (the source hat) and appends a recovery
    /// envelope to the ledger.
    ///
    /// This is intentionally **not** a stage — the check runs
    /// before the contract pipeline and only matches a single
    /// topic (`work.ready`).  Adding a stage for one topic would
    /// push an unrelated layer into every other emit path.
    ///
    /// `fix_unit_known` carries the closure's projection so it
    /// doesn't need `&self`.  Free function rather than method to
    /// avoid the borrow conflict with the surrounding `for event
    /// in events` loop.
    pub(super) fn push_fix_unit_range_finding(
        &mut self,
        event: &crate::event_reader::Event,
        rejected_step: &str,
        fix_unit_known: &std::collections::BTreeSet<String>,
    ) {
        use crate::execution_contract::{ExecutionContractFinding, ExecutionContractViolationKind};
        let known_list: Vec<String> = fix_unit_known.iter().cloned().collect();
        let source_hat: Option<String> = event.hat.clone();
        let finding = ExecutionContractFinding {
            kind: ExecutionContractViolationKind::InvalidStepTarget {
                step: rejected_step.to_string(),
                known_fix_units: known_list.clone(),
            },
            message: format!(
                "work.ready requested fix-unit `{}` which is not in the known fix-unit chain ({}). \
                 The chain has already exhausted or the id is from a stale plan; re-emit with an id from `{}` or finish with `plan.complete`.",
                rejected_step,
                if known_list.is_empty() {
                    "(none yet)".to_string()
                } else {
                    known_list.join(", ")
                },
                if known_list.is_empty() {
                    "<none>".to_string()
                } else {
                    format!("{{{}}}", known_list.join(", "))
                },
            ),
            topic: event.topic.clone(),
            source_hat: source_hat.clone(),
        };
        tracing::warn!(
            finding = ?finding.kind,
            step = %rejected_step,
            "fix-unit range reject"
        );
        let payload_json =
            build_invalid_step_target_resume_payload_for_jsonl(&finding, event, &known_list);
        // Plan 2026-08-10-001 U3: route through the unified
        // resume publisher so the source hat's pending queue
        // receives the targeted resume; bare Event::new would
        // leave `target=None` and the resume would fall through
        // to subscription routing (R4 / E3). Fail-closed when
        // the source hat is unknown / unregistered.
        let source_hat_id: Option<HatId> = source_hat.as_deref().map(|h| HatId::new(h.to_string()));
        // TaskStore is loaded on demand via the loop-context
        // SSOT accessor; the open-task owner fallback is
        // optional and degrades gracefully when the ledger is
        // unavailable. Plan 2026-08-10-001 U1 (R3): replace
        // the hand-rolled `.ralph/agent/tasks.jsonl` join with
        // `self.tasks_path()` so future loop-context overrides
        // (worktree rewrites, alternate ralph dirs) propagate
        // uniformly.
        let task_store = crate::task_store::TaskStore::load(&self.tasks_path()).ok();
        let loop_id_owned = self.current_loop_id();
        // The unified publisher takes a `&ResumeRoutingInputs`.
        // We need owned strings for the lifetime; clone them into
        // a small owned struct passed to a one-shot closure that
        // builds the inputs by reference into a thread-local is
        // overkill for one publish. Instead, build the inputs
        // locally and pass them in. The borrow checker requires
        // `loop_id_owned` to outlive the inputs, which we ensure
        // by stacking the decision call inside this scope.
        let (decision, _loop_id_keepalive) = {
            let mut resume_inputs =
                crate::event_loop::resume_routing::ResumeRoutingInputs::default();
            if let Some(hat) = &source_hat_id {
                resume_inputs.event_target = Some(hat.as_str());
            }
            if let Some(loop_id) = loop_id_owned.as_deref() {
                resume_inputs.loop_id = Some(loop_id);
            }
            let retry_key = format!("invalid_step:{rejected_step}");
            resume_inputs.retry_key = Some(&retry_key);
            let decision = crate::event_loop::resume_routing::publish_targeted_resume(
                &mut self.bus,
                &resume_inputs,
                &self.registry,
                task_store.as_ref(),
                &[],
                payload_json,
            );
            (decision, loop_id_owned)
        };
        if let crate::event_loop::resume_routing::ResumeDecision::Block { reason } = &decision {
            tracing::warn!(
                step = %rejected_step,
                ?reason,
                "fix-unit range reject: resume blocked (no safe target)"
            );
        }
        self.diagnostics.log_execution_contract_rejections(
            0,
            source_hat.as_deref().unwrap_or("ralph"),
            std::slice::from_ref(&finding),
        );
    }

    /// `apply_emit_gate`).
    pub(super) fn apply_emit_gate_on_validated(
        &mut self,
        event: &ralph_proto::Event,
        stashed_outcome: Option<crate::event_loop::emit_gate::EmitGateOutcome>,
    ) -> bool {
        let outcome = match stashed_outcome {
            Some(o) => o,
            None => {
                let mut stage_ctx = self.build_stage_context_for(event);
                crate::event_loop::emit_gate::evaluate_emit_gate(&mut stage_ctx, event)
            }
        };
        match outcome {
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptMainBus => true,
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptRepairStream => {
                // Repair stream was already recorded
                // during the first gate pass. Skip publish.
                false
            }
            crate::event_loop::emit_gate::EmitGateOutcome::Reject(reject) => {
                // Recovery envelope was already recorded
                // during the first gate pass. Skip publish.
                let _ = reject;
                false
            }
        }
    }

    /// U3 (2026-06-27-002 plan completion): route the
    /// event through the emit-gate facade used by
    /// `publish_event` (U2) and return the outcome
    /// so the caller can decide whether to admit the
    /// event to `accepted` and, on the second pass
    /// (`apply_emit_gate_on_validated`), reuse the
    /// outcome to publish-skip without re-running the
    /// pipeline.
    ///
    /// P0-1 (2026-06-27 adversarial review): the
    /// previous design called `apply_emit_gate` AND
    /// `apply_emit_gate_on_validated` per event,
    /// which advanced the per-task
    /// `RepairStateMachine` twice — exhausting the
    /// `repair_budget=3` invariant after just 2
    /// repair events. We now return the `EmitGateOutcome`
    /// from the first pass and stash it so the second
    /// pass (publish gate) can route without re-running
    /// the pipeline.
    ///
    /// P1-9 (2026-06-27 adversarial review): the
    /// previous name (`apply_emit_gate` → `bool`) was
    /// semantically misleading — all three outcomes
    /// returned `true`. Renamed to
    /// `evaluate_emit_gate_for_jsonl_event` to make
    /// the return type (`EmitGateOutcome`) explicit
    /// at every call site. The legacy name remains
    /// as a thin wrapper that discards the outcome
    /// for any external call site that still uses it.
    ///
    /// Takes the JSONL-internal `event_reader::Event`
    /// shape because the only callers live inside
    /// `process_parse_result`. `publish_event` keeps its
    /// own (private) variant that takes a
    /// `ralph_proto::Event` directly.
    pub(super) fn evaluate_emit_gate_for_jsonl_event(
        &mut self,
        event: &crate::event_reader::Event,
    ) -> crate::event_loop::emit_gate::EmitGateOutcome {
        // Convert JSONL-internal Event to the bus-shaped
        // ralph_proto::Event the facade expects. Preserve
        // `hat`/`source` so `PhaseAuthorityStage` (U13) can
        // enforce per-phase whitelists on JSONL ingest.
        let proto: Event = event.clone().into();
        let mut stage_ctx = self.build_stage_context_for(&proto);
        let outcome = crate::event_loop::emit_gate::evaluate_emit_gate(&mut stage_ctx, &proto);
        match &outcome {
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptMainBus => {
                // U3 (2026-06-27-002 plan completion):
                // admit the event so the lifecycle tracker
                // and `validated_events` downstream see it.
                // The BDD wire-level `absent_events` assertions
                // are pinned at the publication level
                // (post `process_events_from_jsonl`), not at
                // the `accepted_events` admission level.
            }
            crate::event_loop::emit_gate::EmitGateOutcome::AcceptRepairStream => {
                self.repair_stream_pending += 1;
                // U7 (2026-06-27-002 plan completion):
                // the JSONL ingest path now also writes
                // to the U6 repair sink.
                self.record_repair_event(&proto);
                // Admit the event so lifecycle tracker
                // records it, but the publication-side
                // will not see it on the main bus.
            }
            crate::event_loop::emit_gate::EmitGateOutcome::Reject(reject) => {
                self.record_stage_rejection(&proto, reject);
                // Admit the event so lifecycle tracker
                // still records the original emit attempt.
                // The BDD wire-level assertion pins that
                // the bus NEVER receives the rejected
                // event (post `process_events_from_jsonl`).
            }
        }
        outcome
    }

    /// U7 (2026-06-27-002 plan completion): shared
    /// helper used by both `publish_event` (U2) and
    /// `apply_emit_gate` (U3) when the emit-gate facade
    /// routes an event to the repair stream. The
    /// `RepairStreamSink` is a pure file-I/O boundary
    /// (see U6); the orchestration glue lives here.
    ///
    /// The workspace root is taken from `self.config
    /// .core.workspace_root`. On FS error we log and
    /// continue — the loop must not crash on a
    /// transient disk error.
    /// 2026-07-02-006 plan U26: R14 dual-check when `plan.complete`
    /// lands on main vs repair sink.
    pub(super) fn diagnose_plan_complete_channel(
        &mut self,
        event: &ralph_proto::Event,
        channel: crate::event_loop::phase_authority::diagnosis::Channel,
    ) {
        if !self.phase_authority.is_enabled() {
            return;
        }
        use crate::event_loop::phase_authority::diagnosis::{
            DualCheckInput, DualCheckOutcome, diagnosis_plan_complete_dual_check,
        };
        let outcome = diagnosis_plan_complete_dual_check(&DualCheckInput {
            topic: event.topic.to_string(),
            source: event.source.as_ref().map(|h| h.to_string()),
            channel,
        });
        match outcome {
            DualCheckOutcome::DualSink => {
                tracing::warn!(
                    topic = %event.topic,
                    source = ?event.source,
                    "R14: plan.complete landed on repair sink — dual-check invariant broken"
                );
                let payload = serde_json::json!({
                    "topic": event.topic.as_str(),
                    "channel": "repair",
                    "reason": "plan.complete_dual",
                });
                self.bus.publish(ralph_proto::Event::new(
                    "plan.complete_dual",
                    payload.to_string(),
                ));
            }
            DualCheckOutcome::UnknownChannel => {
                tracing::warn!(
                    topic = %event.topic,
                    "R14: plan.complete channel unknown — cannot prove dual-check invariant"
                );
            }
            DualCheckOutcome::Ok | DualCheckOutcome::NotApplicable => {}
        }
    }

    /// 2026-07-02-006 plan U20: shipper routing when phase engine is on.
    pub(super) fn phase_authority_rejects_shipper_emit(&self, event: &ralph_proto::Event) -> bool {
        if self.shipper_validator_gate_rejects(event) {
            return true;
        }
        if !self.phase_authority.is_enabled() {
            return false;
        }
        use crate::event_loop::phase_authority::shipper_helper::{
            ShipperDecision, ShipperRoutingContext,
            shipper_requires_plan_complete_when_phase_enabled,
        };
        let reason = self
            .state
            .policy_runtime_state
            .as_ref()
            .and_then(|s| s.last_plan_blocked_reason.clone());
        let plan_complete_present = self
            .state
            .seen_topics
            .iter()
            .any(|t| t.as_str() == "plan.complete")
            || event.topic.as_str() == "plan.complete";
        let ctx = ShipperRoutingContext {
            phase_authority_enabled: true,
            current_phase: self.phase_authority.snapshot().map(|s| s.phase_id),
            reason,
            plan_complete_present,
        };
        matches!(
            shipper_requires_plan_complete_when_phase_enabled(&ctx),
            ShipperDecision::Deny
        )
    }

    /// 2026-07-07-002 U6: shipper success requires current-step validator terminal.
    pub(super) fn shipper_validator_gate_rejects(&self, event: &ralph_proto::Event) -> bool {
        if event.topic.as_str() != "REVIEW_COMPLETE" {
            return false;
        }
        use crate::event_loop::phase_authority::shipper_helper::{
            ShipperValidatorGateContext, ShipperValidatorGateDecision, ValidatorTerminalKind,
            evaluate_shipper_validator_gate,
        };
        let pass_or_fail = serde_json::from_str::<serde_json::Value>(event.payload.as_str())
            .ok()
            .and_then(|v| {
                v.get("pass_or_fail")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_ascii_lowercase())
            })
            .unwrap_or_default();
        let attempting_success = pass_or_fail == "pass"
            || event.payload.contains("pass_with_residuals")
            || event.payload.contains("\"verdict\":\"pass");
        let plan_blocked_reason = self
            .state
            .policy_runtime_state
            .as_ref()
            .and_then(|s| s.last_plan_blocked_reason.clone());
        let validator_terminal_kind =
            self.state
                .last_validator_terminal_kind
                .as_deref()
                .and_then(|k| match k {
                    "passed" => Some(ValidatorTerminalKind::Passed),
                    "failed" => Some(ValidatorTerminalKind::Failed),
                    _ => None,
                });
        let current_step = self
            .state
            .last_test_passed_step
            .clone()
            .or_else(|| self.state.last_validator_terminal_step.clone())
            .or_else(|| self.state.last_plan_complete_step.clone());
        let ctx = ShipperValidatorGateContext {
            current_step,
            validator_terminal_step: self.state.last_validator_terminal_step.clone(),
            validator_terminal_kind,
            plan_blocked_reason,
            attempting_success_ship: attempting_success,
        };
        !matches!(
            evaluate_shipper_validator_gate(&ctx),
            ShipperValidatorGateDecision::Allow
        )
    }

    pub(super) fn record_repair_event(&mut self, event: &ralph_proto::Event) {
        let completion_topic = self.config.event_loop.completion_promise.clone();
        match self.evaluate_terminal_closed_for_event(
            event.topic.as_str(),
            event.payload.as_str(),
            completion_topic.as_str(),
        ) {
            crate::event_loop::terminal_closed_guard::TerminalClosedDecision::Allow => {}
            crate::event_loop::terminal_closed_guard::TerminalClosedDecision::RejectPostTerminal => {
                self.publish_post_terminal_rejection(
                    event.topic.as_str(),
                    "post_terminal_repair_stream_frozen",
                );
                return;
            }
            crate::event_loop::terminal_closed_guard::TerminalClosedDecision::IgnoreDuplicateTerminal => {
                return;
            }
        }
        self.diagnose_plan_complete_channel(
            event,
            crate::event_loop::phase_authority::diagnosis::Channel::Repair,
        );
        let workspace = std::path::PathBuf::from(&self.config.core.workspace_root);
        if let Err(err) =
            crate::event_loop::repair_stream_sink::record_repair_event(event, &workspace)
        {
            tracing::warn!(
                topic = %event.topic,
                error = %err,
                "U7: failed to write repair-stream envelope; continuing without crash"
            );
        }
    }

    /// U10 (2026-06-27-002 plan completion): when the
    /// dispatcher accepts a terminal emit
    /// (`LOOP_COMPLETE` by default), record the
    /// loop-termination intent. The actual end-of-loop
    /// book-keeping (closing the ledger, releasing
    /// the activation tracker) still happens in
    /// `decide_termination_reason`; this method just
    /// logs the event so operators can see when the
    /// terminal topic was accepted.
    pub(super) fn write_loop_termination_record(&self, event: &ralph_proto::Event) {
        let loop_id = self
            .loop_context()
            .and_then(|c| c.loop_id())
            .unwrap_or("default");
        info!(
            loop_id = %loop_id,
            topic = %event.topic,
            iteration = self.state.iteration,
            "U10: terminal emit accepted — loop will close at the next dispatch tick"
        );
    }

    /// U6 (2026-06-27 mechanism foundation): turn a stage
    /// pipeline rejection into a RecoveryDiagnosisEnvelope and
    /// route it through record_recovery_envelope so the
    /// gate's signal lands in recovery.jsonl and is
    /// aggregated by ralph diagnose. CliEmit is reused
    /// because the emit-time gate runs at the same logical
    /// boundary as the CLI precheck.
    ///
    /// P1-1 (2026-06-28 review): the method is
    /// `pub(crate)` so the P1-1 integration test can
    /// synthesise a budget-exhaustion rejection and
    /// assert that the `plan.blocked` escalation is
    /// published on the bus.
    /// 2026-07-02-006 plan U23: advance workflow phase after a
    /// business event lands on the main bus.
    pub(super) fn apply_phase_authority_on_accepted(&mut self, event: &Event) {
        // 2026-06-28 plan U4: a successful accept may carry the
        // runner into the next plan step. Advance here so both
        // ingress paths (`publish_event` and `process_parse_result`)
        // share the same step transition and snapshot write.
        if let Some(next) =
            advance_plan_step(&self.config, &self.current_plan_step, event.topic.as_str())
        {
            self.current_plan_step = next.clone();
        }
        // Plan 004 R7 (P0-4): persist accepted step transitions
        // at the shared main-bus acceptance point. This method is
        // called from both `publish_event` and `process_parse_result`,
        // so the resident EventLoop and CLI policy-check both read the
        // same authority ledger regardless of ingress path.
        self.append_flow_authority_snapshot(event.topic.as_str());
        if !self.phase_authority.is_enabled() {
            return;
        }
        let payload: serde_json::Value =
            serde_json::from_str(event.payload.as_str()).unwrap_or(serde_json::Value::Null);
        let honored = self.stage_pipeline.is_terminal(event);
        let snap = self.phase_authority.snapshot().unwrap_or_else(|| {
            crate::event_loop::phase_authority::PhaseSnapshot::with_phase_id("unit_loop")
        });
        let accepted = crate::event_loop::phase_authority::AcceptedEvent {
            topic: event.topic.as_str(),
            payload: &payload,
            honored,
        };
        let (next, effects) = crate::event_loop::phase_authority::handle_phase_on_event_accepted(
            &self.phase_authority,
            snap,
            &accepted,
        );
        if let Some(ledger) = self.state.state_ledger.as_mut() {
            ledger.snapshot_mut().workflow_phase = Some(next.clone());
        }
        if !effects.progress_md_fragment.is_empty() {
            let progress_path = self
                .config
                .core
                .workspace_root
                .join(".ralph")
                .join("agent")
                .join("progress.md");
            if let Ok(mut existing) = std::fs::read_to_string(&progress_path) {
                existing.push_str(&effects.progress_md_fragment);
                let _ = std::fs::write(progress_path, existing);
            }
        }
        if effects.review_walk_closed {
            tracing::debug!("phase authority: review walk closed");
        }
        if effects.phase_entered {
            tracing::debug!(
                phase = %next.phase_id,
                topic = %event.topic,
                "phase authority: entered new workflow phase"
            );
        }
    }

    /// Plan 004 R7 (P0-4): append the current step snapshot to
    /// `.ralph/flow-authority.jsonl` whenever an event is accepted
    /// onto the main bus. The CLI `--policy-check` path and a
    /// restart of the EventLoop both consult this ledger to recover
    /// the current step, so they read the same authority the
    /// resident EventLoop holds. Rejected events never reach this
    /// method, so the ledger only records accepted transitions.
    /// 2026-07-30-002 plan U1 (R1/D4): wrapper that calls
    /// the free `run_stall_detector_on_state` with the
    /// preset-derived blocked topic and, on a real fail-close
    /// emit, advances `current_plan_step` to the first
    /// forward step that accepts the topic and persists a
    /// flow-authority snapshot. Both call sites in
    /// `process_parse_result` (empty-turn early return and
    /// post-validation tail) route through here so the
    /// escape advance cannot diverge.
    ///
    /// 2026-08-01 plan P0-3 (U3 third-state): when a compiled
    /// execution contract is attached but the state ledger is
    /// uninitialised, the function fails closed via `io::Error`
    /// and emits a `BackpressureTriggered` orchestration event
    /// so operators can distinguish this from a normal commit
    /// failure. See `docs/report/2026-08-01-ce-executor-pipeline-
    /// 2026-08-01-001-fix-unified-execution-contract-p0-p1-plan-diagnosis.md`
    /// §7 for the original observation.
    pub(super) fn run_stall_detector_with_authority_advance(&mut self) -> std::io::Result<()> {
        let blocked_topic = derive_blocked_topic(&self.config);
        let Some(blocked) = run_stall_detector_on_state(
            &mut self.state,
            &self.config.event_loop.progress_steward,
            &self.registry,
            &mut self.bus,
            &blocked_topic,
        ) else {
            return Ok(());
        };

        if let Some(contract) = self.execution_contract.as_ref() {
            let ledger = self.state.state_ledger.as_ref().ok_or_else(|| {
                self.diagnostics.log_orchestration(
                    self.state.iteration,
                    "stall-detector",
                    crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                        reason:
                            "fail-close blocked transition requires an initialized state ledger"
                                .to_string(),
                    },
                );
                std::io::Error::other(
                    "fail-close blocked transition requires an initialized state ledger",
                )
            })?;
            let loop_id = self.current_loop_id_for_contract();
            let activation_id = format!("stall-detector:{}", self.state.iteration);
            crate::event_loop::disposition::publish_synthetic(
                &blocked,
                crate::event_loop::disposition::Disposition::Recovery,
                &loop_id,
                &activation_id,
                &contract.contract_digest,
                ledger,
                &mut self.bus,
            )
            .map_err(|error| {
                std::io::Error::other(format!(
                    "fail-close blocked transition commit failed: {error}"
                ))
            })?;
        } else {
            // Test/legacy loops that did not compile a contract retain the
            // historical direct route.
            self.bus.publish(blocked);
        }

        if let Some(next) =
            resolve_escape_step(&self.config, &self.current_plan_step, &blocked_topic)
        {
            self.current_plan_step = next;
        }
        // Mirror `apply_phase_authority_on_accepted`: the
        // ledger must always record the snapshot even when
        // no forward step accepts the topic, so restart /
        // CLI `--policy-check` see the same view the
        // resident EventLoop holds (the accept path writes
        // the same shape for accepted business topics).
        self.append_flow_authority_snapshot(&blocked_topic);
        Ok(())
    }

    pub(super) fn append_flow_authority_snapshot(&self, topic: &str) {
        use std::io::Write;
        let path = std::path::Path::new(&self.config.core.workspace_root)
            .join(".ralph/flow-authority.jsonl");
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(
                workspace = %self.config.core.workspace_root.display(),
                path = %path.display(),
                error = %err,
                "failed to create flow-authority parent directory"
            );
            return;
        }
        let mut entry = serde_json::Map::new();
        entry.insert(
            "step".to_string(),
            serde_json::Value::String(self.current_plan_step.clone()),
        );
        entry.insert(
            "topic".to_string(),
            serde_json::Value::String(topic.to_string()),
        );
        // Plan 2026-07-31-001 (root cause from implementation-review
        // runs primary-20260731-131515 + primary-20260731-133437):
        // stamp each accepted-step entry with the active loop_id so a
        // new loop cold-start on the same workspace does not inherit
        // the previous loop's terminal step. Without the stamp,
        // `load_flow_authority_current_step` (consumed by
        // `ralph emit --policy-check` and by R7 restart recovery)
        // returns the last entry of the previous loop — e.g.
        // `finalize` — and the very first emit of the new loop
        // (`scope.ready.proposed`) hits `flow_unknown_emit` because
        // `finalize.allowed_emits = [LOOP_COMPLETE]`. The CLI uses
        // the ledger value for `FlowStepScopeStage::current_step`
        // even on the resident EventLoop's own `--policy-check`
        // path; the resident loop's in-memory `current_plan_step`
        // is initialised to `initial_current_plan_step` and is not
        // synchronised with the ledger — the stamp makes the
        // ledger partitionable so each loop sees only its own
        // authoritative step transitions.
        if let Some(loop_id) = self.current_loop_id() {
            entry.insert("loop_id".to_string(), serde_json::Value::String(loop_id));
        }
        let line = serde_json::Value::Object(entry).to_string();
        let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        else {
            tracing::warn!(
                workspace = %self.config.core.workspace_root.display(),
                path = %path.display(),
                "failed to open flow-authority ledger for append"
            );
            return;
        };
        if let Err(err) = writeln!(f, "{line}") {
            tracing::warn!(
                workspace = %self.config.core.workspace_root.display(),
                path = %path.display(),
                error = %err,
                "failed to append flow-authority snapshot"
            );
        }
    }

    pub(crate) fn record_stage_rejection(
        &mut self,
        event: &Event,
        reject: &crate::event_loop::stage_pipeline::StageReject,
    ) {
        use crate::diagnosis::{DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef};
        const PAYLOAD_PREVIEW_CHARS: usize = 200;
        let payload_preview: String = event.payload.chars().take(PAYLOAD_PREVIEW_CHARS).collect();
        let mut builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::CliEmit)
            .severity(DiagnosisSeverity::Warning)
            .topic(event.topic.as_str())
            .source_hat(event.source.as_ref().map(|h| h.as_str()).unwrap_or(""))
            .reason_code(reject.reason_code.clone())
            .message(format!(
                "stage '{}' rejected event: {} (missing_fields={:?})",
                reject.stage_name, reject.reason_code, reject.missing_fields
            ))
            .evidence(EvidenceRef::new(
                EvidenceKind::Field,
                reject.stage_name,
                Some(payload_preview),
            ));
        if let Some(iter) = self.state.iteration.checked_add(0) {
            builder = builder.iteration(iter);
        }
        let envelope = builder.build();
        let notes = vec![format!(
            "stage_pipeline rejection: stage={} reason={} topic={}",
            reject.stage_name, reject.reason_code, event.topic
        )];
        let _ = self.record_recovery_envelope(&envelope, notes);

        if reject.reason_code == "phase_violation" {
            let hat = event
                .source
                .as_ref()
                .map(|h| h.as_str())
                .unwrap_or("unknown");
            let snap = self.phase_authority.record_phase_violation(hat);
            if let Some(ledger) = self.state.state_ledger.as_mut() {
                ledger.snapshot_mut().workflow_phase = Some(snap.clone());
            }
            if let Some(policy) = self.phase_authority.violation_policy() {
                use crate::event_loop::phase_authority::ViolationKind;
                use crate::event_loop::phase_authority::resume_budget::{
                    BudgetDecision, ExhaustedAction, on_exhausted_action,
                    should_admit_resume_from_snapshot,
                };
                match should_admit_resume_from_snapshot(
                    &policy,
                    &snap,
                    hat,
                    ViolationKind::PhaseViolation,
                ) {
                    BudgetDecision::Admit => {
                        let resume_payload = serde_json::json!({
                            "reason_code": "phase_violation",
                            "topic": event.topic.as_str(),
                            "hat": hat,
                            "loop_id": self.loop_id_label(),
                        });
                        // Plan 2026-08-13-003 U1: route the
                        // phase-violation recovery through
                        // the unified publisher so
                        // target/recipient fail-close (D4)
                        // and dedup fire. The resolved
                        // target is the offending hat —
                        // the only hat that owns the
                        // event in this scope. If the
                        // registry has unmounted the hat
                        // between resolve and publish, the
                        // publisher returns Block with no
                        // bus side effect.
                        let loop_id_for_resume = self.loop_id_label();
                        let decision = crate::event_loop::resume_routing::publish_targeted_resume_for_hat(
                            &mut self.bus,
                            &self.registry,
                            None,
                            Some(loop_id_for_resume.as_str()),
                            hat,
                            None,
                            None,
                            None,
                            &format!("phase_violation:{}:{}", event.topic.as_str(), hat),
                            resume_payload.to_string(),
                        );
                        if let crate::event_loop::resume_routing::ResumeDecision::Block { reason } =
                            &decision
                        {
                            tracing::warn!(
                                target = %hat,
                                topic = %event.topic.as_str(),
                                ?reason,
                                "phase-violation recovery blocked (no safe target)"
                            );
                        }
                    }
                    BudgetDecision::Exhausted => match on_exhausted_action(&policy) {
                        ExhaustedAction::PlanBlocked => {
                            let blocked_payload = serde_json::json!({
                                "reason": "phase_violation_exhausted",
                                "topic": event.topic.as_str(),
                                "hat": hat,
                                "loop_id": self.loop_id_label(),
                            });
                            self.bus.publish(ralph_proto::Event::new(
                                "plan.blocked",
                                blocked_payload.to_string(),
                            ));
                        }
                        ExhaustedAction::SilentDrop => {}
                    },
                }
            }
        }

        // P1-1 (2026-06-28 review): when the
        // rejection comes from a budget exhaustion on
        // the repair stream, escalate to a synthesised
        // `plan.blocked` so the operator sees the
        // reason without grepping `recovery.jsonl`.
        // The escalation reuses the same `bus.publish`
        // path as the three existing `plan.blocked`
        // emitters (waves, step-handoff, stall
        // detector) so it lands on the main bus without
        // re-entering the stage pipeline.
        if reject
            .reason_code
            .starts_with("repair_unrecoverable_after_")
        {
            let blocked_payload = serde_json::json!({
                "reason": reject.reason_code,
                "topic": event.topic.as_str(),
                "stage": reject.stage_name,
                "loop_id": self.loop_id_label(),
            });
            self.bus.publish(ralph_proto::Event::new(
                "plan.blocked",
                blocked_payload.to_string(),
            ));
            debug!(
                topic = %event.topic,
                reason = %reject.reason_code,
                "P1-1: synthesised plan.blocked after repair budget exhaustion"
            );
        }
    }

    /// Resolve the loop id label used by the P1-1
    /// `plan.blocked` escalation. Returns the context's
    /// loop id when available, otherwise the literal
    /// `"default"` (mirrors `write_loop_termination_record`).
    pub(super) fn loop_id_label(&self) -> String {
        self.loop_context()
            .and_then(|c| c.loop_id())
            .unwrap_or("default")
            .to_string()
    }

    // -------------------------------------------------------------------------
    // Human-in-the-loop planning support
    // -------------------------------------------------------------------------

    /// Check if any event is a `user.prompt` event.
    ///
    /// Returns the first user prompt event found, or None.
    pub fn check_for_user_prompt(&self, events: &[Event]) -> Option<UserPrompt> {
        events
            .iter()
            .find(|e| e.topic.as_str() == "user.prompt")
            .map(|e| UserPrompt {
                id: Self::extract_prompt_id(&e.payload),
                text: e.payload.clone(),
            })
    }

    /// Extract a prompt ID from the event payload.
    ///
    /// Supports both XML attribute format: `<event topic="user.prompt" id="q1">...</event>`
    /// and JSON format in payload.
    pub(super) fn extract_prompt_id(payload: &str) -> String {
        // Try to extract id attribute from XML-like format first
        if let Some(start) = payload.find("id=\"")
            && let Some(end) = payload[start + 4..].find('"')
        {
            return payload[start + 4..start + 4 + end].to_string();
        }

        // Fallback: generate a simple ID based on timestamp
        format!("q{}", Self::generate_prompt_id())
    }

    /// Generate a simple unique ID for prompts.
    /// Uses timestamp-based generation since uuid crate isn't available.
    pub(super) fn generate_prompt_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{:x}", nanos % 0xFFFF_FFFF)
    }
}
