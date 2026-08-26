//! EventLoop implementation region 9.

use super::super::*;
use tracing::{debug, warn};

impl EventLoop {
    fn runtime_precheck_rejection_for_event(
        &self,
        event: &crate::event_reader::Event,
        payload: &str,
    ) -> Option<String> {
        let guarded = event.topic.as_str();
        let gate_hat = format!("precheck-{guarded}");
        if event.hat.as_deref() != Some(gate_hat.as_str()) {
            return None;
        }
        let precheck = self.config.event_loop.precheck.as_ref()?;
        if !precheck.enabled || !precheck.rules.contains_key(guarded) {
            return None;
        }

        let validation = match guarded {
            "work.done"
                if self
                    .registry
                    .get_config(&ralph_proto::HatId::new("executor"))
                    .is_some()
                    && self
                        .registry
                        .get_config(&ralph_proto::HatId::new("test-stabilizer"))
                        .is_some() =>
            {
                crate::event_loop::worktree_handoff::validate_work_done_handoff(
                    &self.config.core.workspace_root,
                    self.activation_worktree_baselines.get("executor"),
                    payload,
                )
            }
            "stabilization.done" => {
                self.registry
                    .get_config(&ralph_proto::HatId::new("test-stabilizer"))?;
                crate::event_loop::worktree_handoff::validate_stabilization_handoff(
                    &self.config.core.workspace_root,
                    self.activation_worktree_baselines.get("test-stabilizer"),
                    payload,
                )
            }
            _ => return None,
        };
        let Err(reason) = validation else {
            return None;
        };

        tracing::warn!(
            gate = %gate_hat,
            topic = %guarded,
            reason = %reason,
            "runtime handoff precheck rejected terminal event"
        );
        Some(
            serde_json::json!({
                "failed_checks": ["worktree_handoff_inconsistent"],
                "reason": reason,
                "synthetic": true,
            })
            .to_string(),
        )
    }

    /// Returns the loop ID used for execution-contract task-loop checks.
    ///
    /// Primary loops keep `LoopContext::loop_id == None` and identify themselves
    /// via the `.ralph/current-loop-id` marker; worktree loops carry their id
    /// in the context. This helper funnels both shapes through the marker-based
    /// reader so the contract check never misclassifies primary-loop tasks as
    /// belonging to a non-existent "default" loop.
    pub(crate) fn current_loop_id_for_contract(&self) -> String {
        self.current_loop_id()
            .unwrap_or_else(|| "default".to_string())
    }

    /// Filters a task list by loop ID. When `loop_id` is `None`, returns all tasks.
    pub(crate) fn filter_tasks_by_loop<'a>(
        tasks: Vec<&'a crate::task::Task>,
        loop_id: Option<&str>,
    ) -> Vec<&'a crate::task::Task> {
        match loop_id {
            Some(id) => tasks
                .into_iter()
                .filter(|t| t.loop_id.as_deref() == Some(id))
                .collect(),
            None => tasks,
        }
    }

    pub(crate) fn verify_tasks_complete(&self) -> Result<bool, std::io::Error> {
        use crate::task_store::TaskStore;

        let tasks_path = self.tasks_path();

        // No tasks file = no pending tasks = complete
        if !tasks_path.exists() {
            return Ok(true);
        }

        let store = TaskStore::load(&tasks_path)?;
        let current_loop_id = self.current_loop_id();
        let open = Self::filter_tasks_by_loop(store.open(), current_loop_id.as_deref());
        Ok(open.is_empty())
    }

    /// Counts open and closed tasks from the task store.
    ///
    /// Returns `(open_count, closed_count)`. "Open" means non-terminal tasks,
    /// "closed" means tasks with `TaskStatus::Closed`.
    pub(crate) fn count_tasks(&self) -> (usize, usize) {
        use crate::task_store::TaskStore;

        let tasks_path = self.tasks_path();
        if !tasks_path.exists() {
            return (0, 0);
        }

        match TaskStore::load(&tasks_path) {
            Ok(store) => {
                let current_loop_id = self.current_loop_id();
                let all = Self::filter_tasks_by_loop(
                    store.all().iter().collect(),
                    current_loop_id.as_deref(),
                );
                let open = Self::filter_tasks_by_loop(store.open(), current_loop_id.as_deref());
                let closed = all.len() - open.len();
                (open.len(), closed)
            }
            Err(_) => (0, 0),
        }
    }

    /// Returns a list of open task descriptions for logging purposes.
    pub(crate) fn get_open_task_list(&self) -> Vec<String> {
        use crate::task_store::TaskStore;

        let tasks_path = self.tasks_path();
        if let Ok(store) = TaskStore::load(&tasks_path) {
            let current_loop_id = self.current_loop_id();
            let open = Self::filter_tasks_by_loop(store.open(), current_loop_id.as_deref());
            return open
                .iter()
                .map(|t| format!("{}: {}", t.id, t.title))
                .collect();
        }
        vec![]
    }

    pub(crate) fn warn_on_mutation_evidence(
        &self,
        evidence: &crate::event_parser::BackpressureEvidence,
    ) {
        let threshold = self.config.event_loop.mutation_score_warn_threshold;

        match &evidence.mutants {
            Some(mutants) => {
                if let Some(reason) = Self::mutation_warning_reason(mutants, threshold) {
                    warn!(
                        reason = %reason,
                        mutants_status = ?mutants.status,
                        mutants_score = mutants.score_percent,
                        mutants_threshold = threshold,
                        "Mutation testing warning"
                    );
                }
            }
            None => {
                if let Some(threshold) = threshold {
                    warn!(
                        mutants_threshold = threshold,
                        "Mutation testing warning: missing mutation evidence in build.done payload"
                    );
                }
            }
        }
    }

    pub(crate) fn mutation_warning_reason(
        mutants: &MutationEvidence,
        threshold: Option<f64>,
    ) -> Option<String> {
        match mutants.status {
            MutationStatus::Fail => Some("mutation testing failed".to_string()),
            MutationStatus::Warn => Some(Self::format_mutation_message(
                "mutation score below threshold",
                mutants.score_percent,
            )),
            MutationStatus::Unknown => Some("mutation testing status unknown".to_string()),
            MutationStatus::Pass => {
                let threshold = threshold?;

                match mutants.score_percent {
                    Some(score) if score < threshold => Some(format!(
                        "mutation score {:.2}% below threshold {:.2}%",
                        score, threshold
                    )),
                    Some(_) => None,
                    None => Some(format!(
                        "mutation score missing (threshold {:.2}%)",
                        threshold
                    )),
                }
            }
        }
    }

    pub(crate) fn format_mutation_message(message: &str, score: Option<f64>) -> String {
        match score {
            Some(score) => format!("{message} ({score:.2}%)"),
            None => message.to_string(),
        }
    }

    /// Checks if all started guarded workflow instances have reached a terminal phase.
    ///
    /// Returns `Some(WorkflowGuardRejection)` if any instance is incomplete, `None` if all are terminal.
    ///
    /// Terminal phase is the last topic in the chain. An instance is considered "started"
    /// if it has any progress recorded (phase > 0, or any event in the chain has been seen).
    pub(crate) fn check_workflow_guard_completion(
        &self,
        guards: &crate::config::WorkflowGuardsConfig,
    ) -> Option<WorkflowGuardRejection> {
        for chain in &guards.chains {
            // Advisory chains are permissive and should not block LOOP_COMPLETE
            if matches!(chain.mode, crate::config::WorkflowChainMode::Advisory) {
                continue;
            }

            let terminal_phase = chain.topics.len().saturating_sub(1);

            // Check all instances for this chain
            for instance_key in self.state.workflow_progress.instance_keys(&chain.name) {
                let current_phase = self
                    .state
                    .workflow_progress
                    .get_phase(&chain.name, instance_key.as_deref());

                // Instance has no progress — not started, skip
                let current_phase = match current_phase {
                    Some(p) => p,
                    None => continue,
                };

                // If the instance hasn't reached terminal phase, it's incomplete
                if current_phase < terminal_phase {
                    let current_topic = chain
                        .topics
                        .get(current_phase)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    let next_topic = chain
                        .topics
                        .get(current_phase + 1)
                        .cloned()
                        .unwrap_or_else(|| "terminal".to_string());

                    return Some(WorkflowGuardRejection {
                        message: format!(
                            "workflow instance '{}' (chain '{}') is at phase {} ('{}') but not yet at terminal phase {} ('{}')",
                            instance_key.as_deref().unwrap_or("global"),
                            chain.name,
                            current_phase,
                            current_topic,
                            terminal_phase,
                            next_topic
                        ),
                    });
                }
            }
        }
        None
    }

    /// Processes events from JSONL and routes orphaned events to Ralph.
    ///
    /// Also handles backpressure for malformed JSONL lines by:
    /// 1. Emitting `event.malformed` system events for each parse failure
    /// 2. Tracking consecutive failures for termination check
    /// 3. Resetting counter when valid events are parsed
    ///
    /// Returns [`ProcessedEvents`] indicating whether events were found, whether
    /// semantic `plan.*` topics were published, and whether any were orphans that Ralph should
    /// handle.
    pub fn process_events_from_jsonl(&mut self) -> std::io::Result<ProcessedEvents> {
        let result = self.event_reader.read_new_events()?;
        // 2026-06-16-001 U5: reset the per-turn stall-detector
        // flag at the start of each read so the helper can
        // observe whether THIS turn admitted a business event.
        self.state.stall_detector_had_events = false;
        self.process_parse_result(result)
    }

    /// Inner event processing that operates on an already-parsed `ParseResult`.
    ///
    /// This is the single source of truth for event validation, backpressure,
    /// scope enforcement, and bus publishing. Both `process_events_from_jsonl`
    /// and `process_events_from_jsonl_with_waves` delegate to this method.
    pub(crate) fn process_parse_result(
        &mut self,
        result: crate::event_reader::ParseResult,
    ) -> std::io::Result<ProcessedEvents> {
        // Plan GAP-02 / Unit 2: reset the per-loop StateMachine
        // candidate stash so a stale list from a prior batch
        // cannot leak into this batch's apply stage.
        self.pending_state_machine_candidates.clear();
        // DEBUG: 添加入口日志记录所有输入事件
        let event_count = result.events.len();
        let malformed_count = result.malformed.len();
        tracing::debug!(
            iteration = self.state.iteration,
            valid_events = event_count,
            malformed_events = malformed_count,
            "process_parse_result entry - events received"
        );
        self.diagnostics.log_runtime_trace(
            crate::diagnostics::RuntimeTraceEntry::new(
                self.state.iteration as u64,
                0,
                crate::diagnostics::RuntimeTracePhase::Batch,
            )
            .with_kind("event_batch")
            .with_status("received")
            .with_fields(serde_json::json!({
                "valid_events": event_count,
                "malformed_events": malformed_count,
            })),
        );
        // DEBUG: 记录前几个事件的详情用于调试
        for (i, evt) in result.events.iter().take(5).enumerate() {
            tracing::debug!(
                index = i,
                hat = ?evt.hat.as_deref(),
                topic = %evt.topic,
                ts = %evt.ts,
                "event detail"
            );
        }

        // A2 (002-adversarial-review / 003-adversarial-review
        // P0-2): build the unified `ValidationPipeline` once
        // per batch so the runtime can consult it instead of
        // the legacy per-rule gate stack. The build is opt-in
        // via the `UNIFIED_VALIDATION=1` env var (mirrors the
        // `protocol_view.feature_enabled()` surface); when the
        // flag is off the pipeline is dropped and the legacy
        // gate stack continues to gate events as before. The
        // pipeline is **built** here so the per-batch wiring
        // is exercised; the actual call sites inside the
        // per-event gate stack are migrated in follow-up
        // commits (the full migration requires lifting the
        // workspace path and HatRegistry into the pipeline,
        // which is a non-trivial signature change).
        let unified_pipeline = build_unified_validation_pipeline(&self.config);
        tracing::debug!(
            pre_commit_rules = unified_pipeline.pre_commit_rules.len(),
            post_commit_rules = unified_pipeline.post_commit_rules.len(),
            "A2: unified validation pipeline built for this batch"
        );

        // U6: capture payload contract violation produced by event policy
        // validation. The loop runner will read this and pause with a
        // diagnostic.
        let mut payload_contract_violation: Option<
            crate::payload_contract::PayloadContractViolation,
        > = None;
        let mut had_policy_rejections = false;

        // U2 (plan 2026-06-20-001, R15 / KTD-10): engine-backed
        // fail-fast gate. Runs *before* d623c09's policy / scope
        // gates so the loop and the CLI emit share the SAME
        // required-field check (no duplicate field tables in
        // Rust, per KTD-10). The engine uses the same
        // `ProtocolView` the linter reads, so the two layers
        // cannot drift.
        //
        // 2026-06-20-001 review P0 #1: the engine filter MUST
        // run *before* the malformed-handling loop below, so
        // engine-rejected events are converted into
        // `MalformedLine` entries that the existing
        // bookkeeping loop (publish event.malformed + increment
        // consecutive_malformed_events) actually observes. The
        // previous placement ran the filter AFTER the
        // bookkeeping loop, so engine rejections were silently
        // dropped without any bus signal.
        //
        // 2026-06-20-001 review P0 #4: the filter also seeds
        // `state.pending_lint_resume` (via the helper
        // `engine_required_field_filter`) so the agent's next
        // `build_prompt` sees `## LINT RESUME REQUIRED`. The
        // `state.pending_lint_resume` slot is the single source
        // of truth for the lint resume path; the CLI's
        // `pending_lint_resume.json` write was a no-op stub as
        // of the same review.
        //
        // Scope of U2 phase 1 (this commit): the engine gate
        // ONLY short-circuits on `required_fields` missing —
        // the heavier d623c09 checks (terminal monotonicity,
        // semantic gate, recovery) keep running afterwards. The
        // fail-fast is opt-in: the same gate is skipped when the
        // execution_mode is `Coordinator`, and when the engine
        // budget env `RALPH_SERIAL_LINT_MODE=off` is set.
        // Disabling the engine gate does NOT disable the d623c09
        // gates — the engine is a fail-fast addition, not a
        // replacement.
        //
        // Phase 2 (U11-T2) moved event-policy validation into the
        // unified `ValidationPipeline` (`rules_event_policy::EventPolicyRule`).
        // The per-event loop below runs that pipeline and applies the same
        // d623c09 semantics (terminal monotonicity, semantic gate, recovery)
        // through the pipeline's `ValidationResult`s.
        let result = if self.should_run_engine_gate() {
            self.apply_engine_required_field_gate(result)
        } else {
            result
        };

        // Handle malformed lines with backpressure. The engine
        // gate above (review P0 #1) appends `MalformedLine`
        // entries with `line_number=0` for engine rejections;
        // this loop publishes them as `event.malformed` and
        // increments `consecutive_malformed_events` so the
        // existing termination backstop still fires.
        for malformed in &result.malformed {
            let payload = format!(
                "Line {}: {}\nContent: {}",
                malformed.line_number, malformed.error, malformed.content
            );
            let event = Event::new("event.malformed", &payload);
            self.bus.publish(event);
            self.state.consecutive_malformed_events += 1;
            warn!(
                line = malformed.line_number,
                consecutive = self.state.consecutive_malformed_events,
                "Malformed event line detected"
            );
        }

        // Reset counter when valid events are parsed
        if !result.events.is_empty() {
            self.state.consecutive_malformed_events = 0;
        }

        if result.events.is_empty() && result.malformed.is_empty() {
            // 2026-06-16-001 U5: a turn with no events is the
            // canonical "no progress" turn. Run the stall
            // detector before returning so the loop does not
            // silently starve when the JSONL is empty.
            // 2026-07-28-001 plan U3: an empty-activation
            // turn never has a staged over-emit recovery,
            // so no settlement is needed here.
            // 2026-07-30-002 plan U1 (R1/D4): route through
            // the wrapper so the fail-close emit also advances
            // the flow step + appends the snapshot.
            self.run_stall_detector_with_authority_advance()?;
            self.diagnostics.log_runtime_trace(
                crate::diagnostics::RuntimeTraceEntry::new(
                    self.state.iteration as u64,
                    0,
                    crate::diagnostics::RuntimeTracePhase::Commit,
                )
                .with_kind("empty_batch_commit")
                .with_status("no_progress"),
            );
            return Ok(ProcessedEvents {
                had_events: false,
                had_raw_events: false,
                had_rejected_events: false,
                had_plan_events: false,
                has_orphans: false,
                accepted_events: Vec::new(),
                contract_rejections: Vec::new(),
                payload_contract_violation: None,
            });
        }

        // --- Scope enforcement ---
        // 2026-06-13-004 U7: copy out `current_isolated_hat` and the
        // `cancellation_promise` (as owned `String`) so the
        // immutable borrows of `self.state` / `self.config` end
        // before the loop body needs to take a mutable borrow of
        // `self` (e.g. via `record_recovery_envelope`). The
        // `&& let Some(ref …)` form would hold an immutable borrow
        // of `self.state` for the entire `if` block, blocking any
        // `&mut self` call inside it (E0502). Cloning is cheap
        // (a one-time allocation per turn) and lets the body freely
        // call `record_recovery_envelope` / `bus.publish` etc.
        let isolated_hat_owned: Option<ralph_proto::HatId> =
            self.state.current_isolated_hat.clone();
        let cancellation_owned: String = self.config.event_loop.cancellation_promise.clone();
        let events = if self.config.event_loop.execution_mode == HatExecutionMode::Isolated
            && let Some(ref isolated_hat) = isolated_hat_owned
        {
            // Isolated mode: hard-enforce current hat scope + single business event boundary.
            // U3: orchestrator control topics and diagnostic topics bypass the budget
            // (they are loop-internal, not agent progress). Completion promises and
            // other agent terminal topics go through the normal `can_publish` +
            // single-event budget path so an isolated hat cannot bypass its
            // declared publish scope by emitting a completion-style event.
            let mut accepted = Vec::new();
            // 2026-06-16-001 U1: replace `first_wave_id_accepted: Option<Option<String>>`
            // with two independent slots so a wave group is not
            // poisoned by a preceding no-wave_id business event (or
            // vice versa).
            //
            // Invariants:
            // - `non_wave_business_event_accepted` records whether
            //   the single non-wave business slot in this turn has
            //   been consumed.
            // - `accepted_wave_id` records the wave_id of the wave
            //   group (if any) admitted in this turn. A new wave_id
            //   still gets rejected, but a continuation of the same
            //   wave does not.
            // - `is_dual_publish_step_handoff` carves out the
            //   `queue.advance` + `work.ready` handoff pair (see
            //   2026-06-15-003 U1) — the second event in the pair
            //   does not consume a fresh slot.
            let mut non_wave_business_event_accepted = false;
            // 2026-06-30 per-turn-budget backpressure: emit at most ONE
            // hat-targeted `task.resume` per turn when extra business
            // events are dropped by the single-business-event budget.
            // The real incident dropped 30 `plan.complete` events behind
            // a stray `work.ready`; without this guard each drop would
            // inject a duplicate resume (event storm).
            let mut per_turn_budget_feedback_injected = false;
            // 2026-07-28-001 plan U3: the over-emit recovery
            // intent is staged on `self.state.pending_over_emit_recovery`
            // from the drop branch so it survives block exit.
            // 2026-07-04-002 plan U13 carve-out enforcement: the carve-out
            // admits at most ONE exempt topic per activation, regardless
            // of how many `exempt_topics` the preset declared. A second
            // exempt topic in the same activation still hits the default
            // budget (drop + diagnostic), preserving the plan's
            // "serial walk at most once per turn" invariant.
            let mut exempt_topic_carveout_used = false;
            // 2026-07-06 U2 (DEV-001): track when an event was admitted
            // via the exempt_topics carve-out so the slot-bump at
            // line 9191-9208 can be skipped, preserving the
            // non_wave_business_event_accepted=false slot for the
            // rest of the serial walk within the same turn.
            let mut admitted_via_carveout = false;
            let mut accepted_wave_id: Option<String> = None;
            // 2026-06-13-004 P0 #4 review fix (U7 envelope disk
            // storm): per-turn dedup set for scope_drop retry_keys.
            // Multiple identical scope drops in the same
            // turn collapse to a single envelope write (the bus
            // `event.isolation.boundary_violation` event still
            // fires for each, preserving operator visibility —
            // only the recovery journal is dedup'd). This
            // protects `recovery.jsonl` from an 8x scope-drop
            // storm in long-running waves while still letting
            // ADV-1's retry_key namespace distinguish different
            // scope drops (different wave_id / scope_hat / topic
            // → different key → different envelope).
            let mut envelopes_written_this_turn: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let cancellation = cancellation_owned.as_str();

            for event in result.events {
                let topic = event.topic.as_str();
                let is_orchestrator_internal =
                    crate::event_origin::is_orchestrator_control_topic(topic, cancellation)
                        || crate::event_origin::is_orchestrator_diagnostic_topic(topic);

                if is_orchestrator_internal {
                    // Loop-internal event — always accepted, does not
                    // consume the per-turn business-event budget.
                    accepted.push(event);
                    continue;
                }

                // U7 (2026-07-23-002): supervisor-injected coordination
                // events (`*.wave.complete` / `*.wave.failed`, marked
                // `system_injected: true` by `append_supervisor_coord_event`)
                // bypass the per-hat scope check. They are
                // orchestrator-produced, not agent output, and their
                // `hat` field is attribution metadata for the
                // downstream consumer hat, not a publish-scope claim.
                // This aligns with the existing bypasses in
                // `event_origin::validate_event_origin` (P0-1) and
                // `EventBus::publish` (source guard). Without this
                // bypass, isolated scope enforcement drops the
                // coordination event before it reaches the EventBus,
                // leaving the integrator hat's pending queue empty.
                if event.system_injected == Some(true) {
                    // Plan 2026-07-31-001 fix: also count system-injected
                    // events as progress so the post-turn stall detector
                    // does not false-positive fail-close when
                    // `check_default_publishes` injects `<X>.proposed`
                    // into the JSONL for a downstream `precheck-<X>` gate
                    // hat to consume. Without this, the gate hat's PTY
                    // session races the fail-close: while it runs its
                    // LLM-as-judge evaluation, the next
                    // `process_events_from_jsonl` re-reads the injected
                    // event via the bypass branch above, and the
                    // subsequent `run_stall_detector_*` sees
                    // `stall_detector_had_events == false`, incrementing
                    // `consecutive_no_progress_turns` until it hits
                    // `max_steward_iterations` and emits `plan.blocked`
                    // before the gate has a chance to emit `work.failed`
                    // or `work.failed.rejected`.
                    self.state.stall_detector_had_events = true;
                    accepted.push(event);
                    continue;
                }

                // R6/U2: ralph pseudo-hat may only publish control topics.
                // Business topics from ralph are rejected here (fail-closed)
                // so they do NOT count as progress toward the stall detector.
                // P1-12: use prefix match so future `ralph.*` topics are recognised.
                if event.hat.as_deref() == Some("ralph")
                    && !crate::event_origin::is_ralph_control_topic(topic)
                {
                    warn!(
                        topic = %topic,
                        "ralph hat business topic rejected: ralph may only publish control topics"
                    );
                    self.state.record_rejection_digest(
                        "ralph_business_topic_rejected",
                        "ralph hat may only publish control topics",
                        &event.topic,
                        &event.ts,
                    );
                    let violation = Event::new(
                        "event.isolation.boundary_violation",
                        format!(
                            "{{\"hat\":\"ralph\",\"topic\":\"{}\",\"violation\":\"ralph_business_topic_rejected: ralph hat may only publish control topics\"}}",
                            event.topic
                        ),
                    );
                    self.bus.publish(violation);
                    continue;
                }

                // 2026-06-18-001 plan U5: 对**完全没有 provenance**的
                // business topic fail-closed,reason=`isolated_anonymous_business_topic`。
                // 这是 CLI gate(U1) + EventBus source guard 的 runtime
                // 侧封堵——直接文件 append 或 loop-runner 内部 publish
                // 绕过 CLI 的路径在这里拦截。
                if crate::event_origin::is_anonymous_business_topic(
                    &event,
                    &self.registry,
                    cancellation,
                    Some(isolated_hat.as_str()),
                ) {
                    warn!(
                        topic = %event.topic,
                        ts = event.ts,
                        "U5: isolated anonymous business topic rejected (no hat/source/triggered provenance)"
                    );
                    // 2026-06-18-001 plan U6: 累加到 digest
                    self.state.record_rejection_digest(
                        "isolated_anonymous_business_topic",
                        "no hat/source/triggered provenance; supply --hat or use a registered hat backend",
                        &event.topic,
                        &event.ts,
                    );
                    let violation = Event::new(
                        "event.isolation.boundary_violation",
                        format!(
                            "{{\"hat\":\"<anonymous>\",\"topic\":\"{}\",\"violation\":\"isolated_anonymous_business_topic: no hat/source/triggered provenance\"}}",
                            event.topic
                        ),
                    );
                    self.bus.publish(violation);
                    // Plan 2026-08-13-003 U1: route the
                    // anonymous-business recovery through the
                    // unified publisher so target/recipient
                    // fail-close (D4) and dedup fire. The
                    // resolved target is the current
                    // `isolated_hat` — the only hat that owns
                    // the anonymous business event in this
                    // scope. If the registry has unmounted
                    // the hat between resolve and publish,
                    // the publisher returns Block with no
                    // bus side effect.
                    let loop_id_for_resume = self.current_loop_id();
                    let loop_id_str = loop_id_for_resume.as_deref().unwrap_or("default");
                    let activation_id = format!("resume:{}:{}", loop_id_str, self.state.iteration);
                    let resume_payload = format!(
                        "{{\"target_hat\":\"{}\",\"reason\":\"isolated_anonymous_business_topic\",\"topic\":\"{}\"}}",
                        isolated_hat.as_str(),
                        event.topic
                    );
                    let decision = crate::event_loop::resume_routing::task_resume_ingress(
                        &mut self.bus,
                        &self.registry,
                        self.state.state_ledger.as_ref(),
                        loop_id_str,
                        &activation_id,
                        isolated_hat.as_str(),
                        None,
                        &format!("anonymous_business:{}", event.topic),
                        resume_payload,
                    );
                    if let crate::event_loop::resume_routing::ResumeDecision::Block { reason } =
                        &decision
                    {
                        tracing::warn!(
                            target = %isolated_hat.as_str(),
                            topic = %event.topic,
                            ?reason,
                            "isolated-anonymous-business recovery blocked (no safe target)"
                        );
                    }
                    continue;
                }

                // 2026-06-13-004 U2 (P0-1): prefer the event's own
                // `hat` field as the scope-anchor. The wave merge
                // layer (see `merge_wave_results_to_events_file`)
                // writes each record with `hat` set to the worker
                // provenance, so a re-published `review.dimension.done`
                // from `dimension-reviewer` is now attributed to
                // `dimension-reviewer`, not to the orchestrator
                // `current_isolated_hat` (e.g. `review-coordinator`).
                // When the event lacks `hat` (e.g. legacy hand-written
                // records, malformed agents), we fall back to
                // `isolated_hat` — the original behaviour.
                let scope_hat = event
                    .hat
                    .as_deref()
                    .map(|h| ralph_proto::HatId::new(h))
                    .unwrap_or_else(|| isolated_hat.clone());
                if !self.isolated_publish_allowed(&scope_hat, event.topic.as_str()) {
                    warn!(
                        hat = %isolated_hat.as_str(),
                        topic = %event.topic,
                        "Isolated mode: event out of hat scope — dropping"
                    );
                    // P1 finding #11: use the canonical orchestrator
                    // diagnostic topic from the allowlist, embedding the
                    // hat name in the payload. This keeps the bus surface
                    // uniform with the rest of the diagnostic taxonomy
                    // and ensures the entry survives the
                    // `is_orchestrator_diagnostic_topic` allowlist check
                    // on subsequent reads.
                    let violation = Event::new(
                        "event.isolation.boundary_violation",
                        format!(
                            "{{\"hat\":\"{}\",\"topic\":\"{}\",\"violation\":\"Isolated mode: hat '{}' cannot publish topic '{}'\"}}",
                            isolated_hat.as_str(),
                            event.topic,
                            isolated_hat.as_str(),
                            event.topic
                        ),
                    );
                    self.bus.publish(violation);
                    // 2026-06-13-004 U7 (P0-2 / P2-4): also write a
                    // recovery envelope to `recovery.jsonl` so the
                    // responder can surface this scope drop in the
                    // next prompt. Without this, the boundary
                    // violation is only visible in
                    // `orchestration.jsonl` (where bus events are
                    // recorded) — `recovery.jsonl` is the journal
                    // `ralph diagnose` reads, so a missing entry
                    // here means a missing signal. The bus event
                    // above is preserved for backward compatibility
                    // with existing log scrapers. KTD-5 locks the
                    // source to `WorkflowGuard` and the outcome to
                    // `Escalated` (not retryable — the agent has to
                    // fix its scope, not just retry).
                    let reason_code = "isolated_scope_violation";
                    // 2026-06-13-004 review fix (ce-code-review ADV-1):
                    // namespace the retry_key by `wave_id` AND
                    // `wave_index` when the event is part of a
                    // wave so 8 dimensions of the same wave
                    // produce 8 distinct journal entries
                    // (otherwise the responder's dedup collapses
                    // them into 1, re-creating the original
                    // "invisible failure" bug at the recovery
                    // layer). Non-wave events keep the original
                    // tuple-based key.
                    // 2026-06-13-004 P0 #2 + P0 #3 review fix
                    // (ADV-1 '?' fallback + ADV-3 normalize bypass):
                    // route wave events through
                    // `retry_key_from_parts` so `normalize_part`
                    // applies (lowercase + ASCII-only). Without
                    // this, `Reviewer` vs `reviewer` produced
                    // distinct retry_keys and bypassed the U5
                    // responder dedup. The wave_id + wave_index
                    // parts go through the normalizer together
                    // with `scope_hat` + `topic` + `reason_code`,
                    // keeping the format consistent with the
                    // non-wave branch and ensuring every
                    // collision case (case-difference, special
                    // chars, length) is normalized uniformly.
                    let scope_drop_retry_key = match event.wave_id.as_deref() {
                        Some(wid) => {
                            let widx = event
                                .wave_index
                                .map(|i| i.to_string())
                                .unwrap_or_else(|| format!("ts-{}", event.ts));
                            crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                                crate::diagnosis::DiagnosisSource::WorkflowGuard,
                                Some(scope_hat.as_str()),
                                Some(event.topic.as_str()),
                                // Embed wave_id + wave_index in the
                                // `reason_code` slot so the namespace
                                // is preserved end-to-end.
                                &format!("{reason_code}/{wid}/{widx}"),
                                None,
                            )
                        }
                        None => {
                            crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                                crate::diagnosis::DiagnosisSource::WorkflowGuard,
                                Some(scope_hat.as_str()),
                                Some(event.topic.as_str()),
                                reason_code,
                                None,
                            )
                        }
                    };
                    let mut env_builder = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
                        .source(crate::diagnosis::DiagnosisSource::WorkflowGuard)
                        .severity(crate::diagnosis::DiagnosisSeverity::Warning)
                        .iteration(self.state.iteration)
                        .topic(event.topic.clone())
                        .source_hat(scope_hat.as_str())
                        .target_hat(scope_hat.as_str())
                        .reason_code(reason_code)
                        .message(format!(
                            "isolated mode: hat '{}' cannot publish topic '{}'",
                            scope_hat.as_str(),
                            event.topic
                        ))
                        .expected_action(format!(
                            "Hat '{}' must declare '{}' in its `publishes` list (or stop emitting it). \
                             This scope drop is not retryable — re-emit a topic the hat is allowed to publish.",
                            scope_hat.as_str(),
                            event.topic
                        ))
                        .safe_target(false)
                        .outcome(crate::diagnosis::DiagnosisOutcome::Escalated)
                        .evidence(crate::diagnosis::EvidenceRef {
                            kind: crate::diagnosis::EvidenceKind::Topic,
                            ref_path: event.topic.clone(),
                            snippet: Some(format!(
                                "isolated_hat={} event_hat={}",
                                isolated_hat.as_str(),
                                scope_hat.as_str()
                            )),
                        })
                        // 2026-06-13-004 P0 #4: clone the retry_key
                        // here so we can both dedup it against
                        // `envelopes_written_this_turn` AND move
                        // it into `env_builder` below.
                        .retry_key(scope_drop_retry_key.clone());
                    if let Some(session_id) = self.diagnostics().session_id() {
                        env_builder = env_builder.session_id(session_id);
                    }
                    let envelope = env_builder.build();
                    // 2026-06-13-004 P0 #4 review fix (U7 envelope
                    // disk storm): per-turn dedup of the retry_key
                    // so multiple identical scope drops in the
                    // same `process_parse_result` call collapse to
                    // a single envelope write. The bus
                    // `event.isolation.boundary_violation` event
                    // (emitted earlier in this branch) still
                    // fires for each, so operators see every
                    // occurrence in `orchestration.jsonl`; the
                    // dedup only shields `recovery.jsonl` from
                    // the 8x write rate that wave-batches
                    // produce. Distinct scope drops (different
                    // wave_id / topic / scope_hat) produce
                    // distinct retry_keys and still write
                    // distinct envelopes, so ADV-1's
                    // namespace fix is preserved.
                    if !envelopes_written_this_turn.insert(scope_drop_retry_key.clone()) {
                        debug!(
                            retry_key = %scope_drop_retry_key,
                            topic = %event.topic,
                            "U7: per-turn dedup dropped identical scope-drop envelope"
                        );
                        continue;
                    }
                    // 2026-06-13-004 U7: copy out the immutable
                    // borrow of `isolated_hat` before we take a
                    // mutable borrow of `self` to record the
                    // envelope. E0502 would otherwise block the
                    // call (Rust cannot prove the immutable
                    // borrow ends before the mutable one starts
                    // when both go through `self`).
                    let isolated_hat_str = isolated_hat.as_str().to_string();
                    let scope_hat_str = scope_hat.as_str().to_string();
                    let topic_str = event.topic.clone();
                    self.record_recovery_envelope(
                        &envelope,
                        vec![format!(
                            "scope_drop hat={} topic={} current_isolated_hat={}",
                            scope_hat_str, topic_str, isolated_hat_str
                        )],
                    );
                    // source hat so the next turn the rejected hat
                    // gets reactivated with explicit recovery context.
                    // Without this hook, an isolated hat that emits an
                    // out-of-scope terminal-style topic (e.g. an
                    // unauthorized `LOOP_COMPLETE`) would never see a
                    // recovery signal — the loop would simply drop the
                    // event and stay silent, breaking R8 / R11
                    // (targeted task.resume contract).  The recovery
                    // payload names the rejected topic and the allowed
                    // publishes so the agent can re-emit a legal one
                    // on its next turn.
                    let allowed: Vec<String> = self
                        .registry
                        .get_config(isolated_hat)
                        .map(|c| c.publishes.clone())
                        .unwrap_or_default();
                    // P1 finding #6: dedup — if the target hat already
                    // has a pending `task.resume` (with the same
                    // `stage=isolated_scope` origin), skip injection.
                    // Each isolated violation turn would otherwise
                    // stack duplicate recovery events on the same
                    // queue, causing event-storm behaviour in loops
                    // that repeatedly re-attempt the same illegal
                    // publish (e.g. an agent that never learns). The
                    // dedup key is (target_hat, topic=task.resume) so
                    // multiple distinct source-hats can still each
                    // receive one recovery event per turn.
                    let already_pending_recovery = self
                        .bus
                        .peek_pending(isolated_hat)
                        .map(|events| events.iter().any(|e| e.topic.as_str() == "task.resume"))
                        .unwrap_or(false);
                    if !already_pending_recovery {
                        // 2026-06-14-004 U2: record the rejection key and check circuit breaker.
                        // We record BEFORE checking exhaustion so the count includes this attempt.
                        // The key includes wave_id/wave_index for wave events (distinguishes
                        // 8 different wave workers), so exhaustion means the SAME worker keeps
                        // hitting the same violation across iterations.
                        let count = self.state.record_rejection_key(&scope_drop_retry_key);
                        if self.state.rejection_key_is_exhausted(&scope_drop_retry_key) {
                            // Circuit breaker tripped: do NOT inject task.resume.
                            // The hat has exceeded U2_REJECTION_RETRY_LIMIT retries.
                            // Store the original termination reason in LoopState so
                            // `check_termination()` can return it with non-normalized
                            // hat/topic for clear diagnostics (R-C).
                            warn!(
                                key = %scope_drop_retry_key,
                                hat = %isolated_hat.as_str(),
                                topic = %event.topic,
                                count = count,
                                "Scope violation circuit breaker: no more task.resume injections for key '{}'",
                                scope_drop_retry_key
                            );
                            self.state.scope_violation_circuit_breaker_tripped =
                                Some(TerminationReason::ScopeViolationCircuitBreakerTripped {
                                    hat: isolated_hat.as_str().to_string(),
                                    topic: event.topic.clone(),
                                    violation_count: count,
                                    allowed_topics: allowed.clone(),
                                });
                            // Publish a terminal diagnostic event so operators and
                            // `ralph diagnose` see what happened.
                            let breaker_event = Event::new(
                                "loop.terminate",
                                format!(
                                    "{{\"reason\":\"scope_violation_circuit_breaker_tripped\",\"hat\":\"{}\",\"topic\":\"{}\",\"violation_count\":{},\"allowed_topics\":{:?}}}",
                                    isolated_hat.as_str(),
                                    event.topic,
                                    count,
                                    allowed
                                ),
                            )
                            .with_target(isolated_hat.clone());
                            self.bus.publish(breaker_event);
                            continue;
                        }
                        // P1 finding #10: build the payload through the
                        // shared helper so the format matches the
                        // rejection pipeline and downstream consumers
                        // (U6 responder, U5 drift) can rely on a
                        // single schema.  The helper expects a
                        // `Rejection` — for the U2 isolated_scope path
                        // we construct one inline.
                        let rejection = crate::event_loop::rejection::Rejection {
                            stage: crate::event_loop::rejection::RejectionStage::Origin,
                            source_hat: Some(isolated_hat.to_string()),
                            business_hat: None,
                            topic: event.topic.clone(),
                            violation: format!(
                                "hat '{}' cannot publish '{}' in isolated mode",
                                isolated_hat.as_str(),
                                event.topic
                            ),
                            retry_key: format!(
                                "{}:{}:isolated_scope",
                                isolated_hat.as_str(),
                                event.topic
                            ),
                            retry_eligible: true,
                            non_retryable_reason: None,
                            target_hat: Some(isolated_hat.to_string()),
                            // 2026-06-16-001 U3: capture the source
                            // event's timestamp so the freshness
                            // filter (U3 TTL) can drop stale
                            // rejections on the next call. The
                            // `event_reader::Event` struct does not
                            // carry a stable `id` field — the JSONL
                            // line offset or `ts` is the closest
                            // available correlation key, so
                            // `original_event_id` stays None and
                            // `original_ts` carries the event
                            // timestamp.
                            original_event_id: None,
                            original_ts: Some(event.ts.clone()),
                            // 2026-06-23 fix plan U5 (CB-2): isolated_scope
                            // path predates the typed-kind plumbing;
                            // pass None so the resume payload falls
                            // back to `violation`-derived reason.
                            kind: None,
                            duplicate_work_done_hint: None,
                            seen_count: None,
                        };
                        // 2026-06-16-001 U3: freshness filter — drop
                        // the rejection (and the synthetic
                        // `task.resume` it would produce) if the
                        // source event's timestamp is older than
                        // `task_resume_ttl_seconds`. The default is
                        // 300s; operators can override per-preset.
                        // We treat missing/unparseable timestamps
                        // as "fresh" so legacy JSONL that lacks a
                        // recoverable ts still flows through the
                        // existing recovery path.
                        let ttl_seconds = self
                            .config
                            .event_loop
                            .task_resume_ttl_seconds
                            .unwrap_or(300);
                        if is_rejection_stale(&rejection, ttl_seconds) {
                            warn!(
                                source_event_ts = ?rejection.original_ts,
                                ttl_seconds,
                                hat = %isolated_hat.as_str(),
                                topic = %event.topic,
                                "isolated mode: stale rejection — dropping task.resume"
                            );
                            self.bus.publish(Event::new(
                                "event.isolation.boundary_violation",
                                format!(
                                    "{{\"hat\":\"{}\",\"topic\":\"{}\",\"violation\":\"Isolated mode: stale rejection for '{}' (TTL={}s) — dropping task.resume\"}}",
                                    isolated_hat.as_str(),
                                    event.topic,
                                    event.topic,
                                    ttl_seconds
                                ),
                            ));
                            continue;
                        }
                        // R5 (2026-06-14-003 plan): carry the wave
                        // metadata (when present) so the resumed hat
                        // can recover the wave context.  Plan AC7
                        // requires the resume payload to include
                        // `wave_id` / `wave_index` / `wave_total` for
                        // wave events; this branch was previously
                        // dropping them by passing `None`.
                        let wc =
                            crate::event_loop::rejection::WaveContextForResume::from_reader_event(
                                &event,
                            );
                        let resume_payload =
                            crate::event_loop::rejection::build_task_resume_payload(
                                &rejection,
                                &allowed,
                                &[],
                                None,
                                None,
                                wc.as_ref(),
                            );
                        // Plan 2026-08-10-001 U1: route the
                        // scope-drop recovery through the
                        // unified publisher. The
                        // post-scope `accepted` push (below)
                        // stays as-is so `had_events` for the
                        // turn is correctly true; the
                        // `triggered: None` JsonlEvent shape
                        // from the U1 bugfix continues to
                        // avoid double-targeting. The helper
                        // publishes the targeted
                        // `task.resume` directly. We capture
                        // the payload string and loop_id
                        // before the bus borrow so the
                        // post-scope `accepted` push still
                        // has access to them.
                        let loop_id_for_resume = self.current_loop_id();
                        let loop_id_str = loop_id_for_resume.as_deref().unwrap_or("default");
                        let activation_id =
                            format!("resume:{}:{}", loop_id_str, self.state.iteration);
                        let recovery_payload_for_accepted = resume_payload;
                        let _ = crate::event_loop::resume_routing::task_resume_ingress(
                            &mut self.bus,
                            &self.registry,
                            self.state.state_ledger.as_ref(),
                            loop_id_str,
                            &activation_id,
                            isolated_hat.as_str(),
                            None,
                            &format!("scope_drop:{}", isolated_hat.as_str()),
                            recovery_payload_for_accepted.clone(),
                        );
                        let recovery_payload = recovery_payload_for_accepted;
                        let _ = loop_id_for_resume;
                        // P1 finding #1: also push the synthetic
                        // `task.resume` into the local `accepted` vector
                        // so the JSONL-derived `accepted_events` (used
                        // downstream to compute `had_events` for the
                        // turn) sees the recovery. Without this, a
                        // turn that contains only a rejected out-of-scope
                        // event would otherwise yield `had_events =
                        // false`, causing the loop runner to treat the
                        // turn as empty and not advance. The recovery
                        // stays targeted to the source hat via the
                        // bus.publish above — the `accepted` push only
                        // ensures the turn is reported as active.
                        //
                        // `accepted` here is `Vec<JsonlEvent>`
                        // (= `event_reader::Event`); we build one from
                        // the recovery's fields. **Do not** set
                        // `triggered` here: this is a fresh runtime
                        // synthesised event, not a JSONL rebuild.
                        // The accepted-branch rebuild path
                        // (`jsonl_event_to_proto` / `accept_event!(&event, &payload)`)
                        // would otherwise re-apply `with_target` to
                        // the rebuild, producing a *second*
                        // `task.resume` on the bus (one from the
                        // immediate `self.bus.publish(recovery)` here,
                        // one from the rebuild in the post-scope
                        // accepted-events loop) and double-counting
                        // the recovery on the target hat's pending
                        // queue. Setting `triggered: None` keeps the
                        // rebuild target-less, which matches the
                        // pre-U1 behaviour (the pre-U1 catch-all
                        // used `Event::new(topic, &payload)`, which
                        // did not propagate `target`).
                        let resume_jsonl = crate::event_reader::Event {
                            topic: "task.resume".to_string(),
                            payload: Some(recovery_payload),
                            ts: chrono::Utc::now().to_rfc3339(),
                            hat: None,
                            triggered: None,
                            source: None,
                            wave_id: None,
                            wave_index: None,
                            wave_total: None,
                            system_injected: None,
                        };
                        accepted.push(resume_jsonl);
                    }
                    continue;
                }

                // 2026-06-16-001 U1: wave group admission logic.
                // A `wave_id` group of result events is ONE business
                // emission, not N. The merge layer (see
                // `merge_wave_results_to_events_file`) stamps every
                // record with the originating `wave_id`, so a batch
                // of N `review.dimension.done` from workers in the
                // same wave must be admitted in full even after a
                // non-wave business event was already accepted in the
                // same turn.
                //
                // Rules (evaluated in order):
                // 1. event.wave_id == accepted_wave_id → admit
                //    (continuation of the admitted wave group).
                // 2. event.wave_id.is_some() && accepted_wave_id.is_none()
                //    → admit, set accepted_wave_id (new wave group).
                // 3. event.wave_id.is_some() && accepted_wave_id is
                //    some other id → reject (a distinct second wave).
                // 4. event.wave_id.is_none() && !non_wave_business_event_accepted
                //    → admit (consume the non-wave slot).
                // 5. event.wave_id.is_none() && non_wave_business_event_accepted
                //    but event is `work.ready` and the last accepted
                //    event is `queue.advance` from the same hat
                //    (is_dual_publish_step_handoff) → admit (handoff
                //    carve-out, see 2026-06-15-003 U1).
                // 6. otherwise → reject.
                let event_wave_id = event.wave_id.clone();
                let admitted_under_wave = match event_wave_id.as_deref() {
                    Some(wid) => match accepted_wave_id.as_deref() {
                        Some(current) => current == wid,
                        None => true,
                    },
                    None => false,
                };
                let wave_collision = match event_wave_id.as_deref() {
                    Some(wid) => {
                        matches!(accepted_wave_id.as_deref(), Some(current) if current != wid)
                    }
                    None => false,
                };

                let incoming_hat = event
                    .hat
                    .as_deref()
                    .or(event.source.as_deref())
                    .unwrap_or(isolated_hat.as_str());
                let is_dual_publish_step_handoff = self.isolated_dual_publish_handoff(
                    event.topic.as_str(),
                    incoming_hat,
                    isolated_hat.as_str(),
                    &accepted,
                );
                let required_event_topics = self.required_event_topic_set();

                // 2026-07-01-001 plan U1: terminal-priority
                // budget. When the non-wave slot has already
                // been consumed by a non-terminal event (e.g.
                // a stray `work.ready` that the agent emitted
                // before the terminal), the runtime must NOT
                // drop a terminal event (LOOP_COMPLETE /
                // plan.complete / plan.blocked / report.done /
                // REVIEW_COMPLETE). The terminal topic list is
                // derived from `EventPolicyConfig.terminal_topics`
                // + the configured completion / cancellation
                // promises, so non-ce-executor presets stay
                // untouched.
                //
                // Mechanics: when the current event is a
                // terminal topic and the slot is already
                // taken, we publish a `event.isolation.terminal_priority`
                // diagnostic, evict the non-terminal
                // business event from `accepted`, and admit
                // the terminal event instead. The eviction
                // is safe because the agent already had a
                // chance to act on the non-terminal event in
                // earlier turns; dropping it here is the
                // lesser evil vs. stalling the loop.
                let terminal_topics = self.collect_terminal_topic_set();
                let event_is_terminal = terminal_topics.contains(event.topic.as_str());
                let mut evicted_non_terminal: Option<usize> = None;
                if event_is_terminal && non_wave_business_event_accepted {
                    for (idx, prev) in accepted.iter().enumerate().rev() {
                        let prev_topic = prev.topic.as_str();
                        if prev_topic == "task.resume" {
                            // Don't touch recovery envelopes.
                            continue;
                        }
                        if required_event_topics.contains(prev_topic) {
                            // P0-5: required pre-completion events must
                            // never be displaced by U1 terminal-priority.
                            break;
                        }
                        if terminal_topics.contains(prev_topic) {
                            // Already admitted a terminal event
                            // — keep the new one out so the
                            // budget stays sane.
                            break;
                        }
                        if prev.wave_id.is_none() {
                            evicted_non_terminal = Some(idx);
                            break;
                        }
                    }
                }

                let mut should_admit = if admitted_under_wave {
                    true
                } else if wave_collision {
                    false
                } else if !non_wave_business_event_accepted {
                    true
                } else if event_is_terminal && evicted_non_terminal.is_some() {
                    // U1: terminal-priority override — the
                    // terminal event displaces the earlier
                    // non-terminal business event.
                    true
                } else if !exempt_topic_carveout_used && {
                    let (business, terminal) = self
                        .config
                        .event_loop
                        .event_policy
                        .as_ref()
                        .map(|ep| (ep.business_topics.as_slice(), ep.terminal_topics.as_slice()))
                        .unwrap_or((&[], &[]));
                    is_isolated_exempt_topic(
                        self.registry
                            .get_config(isolated_hat_owned.as_ref().unwrap_or(&HatId::from(""))),
                        &event.topic,
                        business,
                        terminal,
                    )
                } {
                    // 2026-07-03-005 plan (P0 fix M-1): declared
                    // serial walk exemption. The isolated hat has
                    // listed this topic in its `exempt_topics` (a
                    // preset-declared positive list of topics that
                    // are exempt from the per-turn business-event
                    // budget), so we admit the event without
                    // consuming the `non_wave_business_event_accepted`
                    // slot. Critical for hats that walk N events
                    // one-per-turn (e.g. review-coordinator walking
                    // 6 review.dimension.ready events in
                    // ce-executor-serial — see preset's
                    // `exempt_topics: ["review.dimension.ready",
                    // "review.dimensions.complete"]`). Empty
                    // exempt_topics = no exemption (default
                    // behaviour preserved).
                    //
                    // 2026-07-04-002 plan (P0 #2 fix): the
                    // `!non_wave_business_event_accepted` guard in
                    // the previous revision was structurally dead —
                    // the earlier `else if !non_wave_business_event_accepted`
                    // branch always returned `true` first. Removing
                    // it makes this branch reachable when the
                    // per-turn slot is already occupied. A second
                    // exempt topic in the *same* turn still falls
                    // through to the default budget (drop + bound),
                    // because we do not consume the slot here.
                    //
                    // 2026-07-06 U2 (DEV-001): record that this
                    // admission was via the carve-out so the
                    // slot-bump path below can be skipped, letting
                    // the serial walk continue within the same turn.
                    admitted_via_carveout = true;
                    true
                } else {
                    is_dual_publish_step_handoff
                };

                if should_admit && let Some(idx) = evicted_non_terminal {
                    let evicted = accepted.remove(idx);
                    warn!(
                        evicted_topic = %evicted.topic,
                        admitted_topic = %event.topic,
                        hat = %isolated_hat.as_str(),
                        "U1 terminal-priority: displaced earlier non-terminal business event to admit terminal event"
                    );
                    let diagnostic = Event::new(
                        "event.isolation.terminal_priority",
                        format!(
                            "{{\"hat\":\"{}\",\"evicted_topic\":\"{}\",\"admitted_topic\":\"{}\",\"reason\":\"isolated mode: terminal topics have priority over non-terminal business events in the per-turn budget\"}}",
                            isolated_hat.as_str(),
                            evicted.topic,
                            event.topic
                        ),
                    );
                    self.bus.publish(diagnostic);
                    // The eviction freed the non-wave slot;
                    // the per-turn sticky flag must be reset
                    // so subsequent admits (this turn) see
                    // the slot as open.
                    non_wave_business_event_accepted = false;
                }

                // 2026-07-04-002 plan (P0 #2): record the carve-out
                // usage so a SECOND exempt topic in the same
                // activation falls through to the default budget
                // (drop + diagnostic). We only flip the flag when
                // the carve-out actually admitted — admits via
                // other branches (wave, terminal-priority, fresh
                // slot) keep the carve-out unused for this turn.
                if should_admit && !non_wave_business_event_accepted && !admitted_under_wave {
                    let (business, terminal) = self
                        .config
                        .event_loop
                        .event_policy
                        .as_ref()
                        .map(|ep| (ep.business_topics.as_slice(), ep.terminal_topics.as_slice()))
                        .unwrap_or((&[], &[]));
                    if is_isolated_exempt_topic(
                        self.registry
                            .get_config(isolated_hat_owned.as_ref().unwrap_or(&HatId::from(""))),
                        &event.topic,
                        business,
                        terminal,
                    ) {
                        exempt_topic_carveout_used = true;
                    }
                }

                if should_admit
                    && let Some(missing) =
                        self.path_required_missing_for_anchor(event.topic.as_str())
                {
                    tracing::warn!(
                        topic = %event.topic,
                        missing = ?missing,
                        hat = %isolated_hat.as_str(),
                        "Isolated admit rejected: path_required_events require topics not yet observed"
                    );
                    should_admit = false;
                }

                if should_admit {
                    self.mark_required_event_seen(event.topic.as_str());
                    accepted.push(event);
                    match event_wave_id.as_deref() {
                        Some(wid) => {
                            if accepted_wave_id.is_none() {
                                accepted_wave_id = Some(wid.to_string());
                            }
                        }
                        None => {
                            // 2026-07-06 U2 (DEV-001): exempt_topics
                            // carve-out admissions must NOT consume
                            // the per-turn non_wave_business_event_accepted
                            // slot, otherwise the serial walk (e.g.
                            // review-coordinator walking 6
                            // review.dimension.ready) drops N-1 events
                            // and review-synthesizer receives incomplete
                            // data. The pre-existing carve-out branch
                            // already sets admitted_via_carveout = true
                            // above.
                            if !admitted_via_carveout {
                                non_wave_business_event_accepted = true;
                            }
                        }
                    }
                    // U3 P0 fix: write the sticky per-turn budget flag so
                    // `check_default_publishes` (which runs later in the same
                    // turn when JSONL had zero events, or earlier when JSONL
                    // had business events) sees a consistent view.
                    //
                    // 2026-07-06 U2 (DEV-001): carve-out admissions must
                    // also keep isolated_turn_business_event_accepted
                    // false so the default_publishes guard does not
                    // see the slot as occupied and refuse the next
                    // exempt topic in the serial walk.
                    if !admitted_via_carveout {
                        self.state.isolated_turn_business_event_accepted = true;
                    }
                    // 2026-06-16-001 U5: mark the per-turn
                    // stall-detector flag so the post-validation
                    // stall detector resets the counters.
                    self.state.stall_detector_had_events = true;
                } else {
                    warn!(
                        topic = %event.topic,
                        "Isolated mode: extra business event dropped — only one per turn"
                    );
                    let diagnostic = Event::new(
                        "event.isolation.boundary_violation",
                        format!(
                            "Isolated mode: dropped extra event '{}' — only one business event per turn allowed",
                            event.topic
                        ),
                    )
                    .with_target(isolated_hat.clone());
                    self.bus.publish(diagnostic);

                    // 2026-07-28-001 plan U3 (commit-aware
                    // over-emit recovery): the previous path
                    // injected a hat-targeted `task.resume`
                    // immediately, which let a co-emitted
                    // first business event (already admitted in
                    // the same turn) be silently displaced by
                    // `next_hat` priority. Instead, stage the
                    // intent here and resolve it AFTER the loop
                    // has determined whether any business event
                    // actually committed. The recovery is only
                    // useful when zero business events landed;
                    // otherwise the over-emit is a pure
                    // cosmetic extra and the agent already
                    // succeeded on its primary emit.
                    if !per_turn_budget_feedback_injected {
                        per_turn_budget_feedback_injected = true;
                        self.state.pending_over_emit_recovery = Some(OverEmitRecovery {
                            hat: isolated_hat.clone(),
                            dropped_topic: event.topic.clone(),
                        });
                    }
                }
            }
            accepted
        } else if self.config.event_loop.enforce_hat_scope {
            // Coordinator mode: scope enforcement with active_hats
            let active_hats = self.state.last_active_hat_ids.clone();
            let completion = &self.config.event_loop.completion_promise;
            let cancellation = &self.config.event_loop.cancellation_promise;
            let (in_scope, out_of_scope): (Vec<_>, Vec<_>) =
                result.events.into_iter().partition(|event| {
                    if active_hats.is_empty() {
                        // No active hat: only allow control topics and completion promise.
                        // This prevents arbitrary business events from entering the pipeline
                        // without hat provenance between orchestration cycles.
                        crate::event_origin::is_jsonl_control_topic(
                            event.topic.as_str(),
                            cancellation,
                        ) || event.topic.as_str() == completion.as_str()
                    } else {
                        active_hats
                            .iter()
                            .any(|hat_id| self.registry.can_publish(hat_id, event.topic.as_str()))
                    }
                });

            for event in &out_of_scope {
                let violation_hat = active_hats.first().map(|h| h.as_str()).unwrap_or("unknown");
                warn!(
                    active_hats = ?active_hats,
                    topic = %event.topic,
                    "Scope violation: active hat(s) cannot publish this topic — dropping event"
                );
                let violation_topic = format!("{}.scope_violation", violation_hat);
                let violation_payload = format!(
                    "Attempted to publish '{}': {}",
                    event.topic,
                    event.payload.clone().unwrap_or_default()
                );
                let violation = Event::new(violation_topic, violation_payload);
                self.bus.publish(violation);
            }

            in_scope
        } else {
            result.events
        };

        // --- Origin guard: validate JSONL event provenance before bus publication ---
        // Events from JSONL are untrusted until provenance and scope checks accept them.
        // This rejects no-hat business events, unknown-hat events, and out-of-scope topics.
        let (mut events, origin_rejections) = filter_events_by_origin(
            events,
            &self.registry,
            &self.config.event_loop.cancellation_promise,
            &self.config.event_loop.completion_promise,
        );
        let had_origin_rejections = !origin_rejections.is_empty();
        // 2026-06-18-001 plan U6: 把 origin guard 拒收累加到 digest,
        // 让 agent 在下一轮 prompt 中看到 `## RECENT REJECTIONS`。
        for rej in &origin_rejections {
            self.state.record_rejection_digest(
                rej.reason,
                &format!(
                    "origin guard rejected topic `{}` from hat {:?}",
                    rej.topic, rej.source_hat
                ),
                &rej.topic,
                "",
            );
            // Plan 2026-08-26-1104 U3 (S3.3): persist a
            // `kind=policy_receipt` row per origin-guard rejection.
            // Today this gate has no recovery.jsonl path — the
            // receipt is the new evidence layer. `rule_refs`
            // contains `origin_guard` so downstream dashboards
            // can group origin vs policy rejections. `reason_code`
            // is the stable machine-readable string
            // (`origin:{reason}`) and `retry_key` matches
            // `RejectionRecord::retry_key` shape so the
            // attribution engine (U8) can reconcile.
            let reason_code = format!("origin:{}", rej.reason);
            self.diagnostics.emit_policy_receipt(
                crate::diagnostics::PolicyReceiptDecision::Reject,
                rej.topic.clone(),
                rej.source_hat.as_deref(),
                &["origin_guard"],
                Some(&reason_code),
                None,
            );
        }
        // --- End origin guard ---

        // --- Topic format check (U5 / R9): reject unknown topics before policy ---
        // Builds a whitelist from hat publishes + system/control topics.
        // Rejected topics produce a recovery signal but NO retry (R10).
        // Only active when event_policy is enabled AND hats are configured
        // (no hats = no whitelist to validate against, skip check).
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
            && !self.config.hats.is_empty()
        {
            use std::collections::HashSet;
            let allowed_topics: HashSet<String> = crate::event_policy::build_allowed_topics(
                &self.config.hats,
                &self.config.event_loop.completion_promise,
                self.config.event_loop.event_policy.as_ref(),
            );
            let (topic_format_ok, topic_format_rejections): (Vec<_>, Vec<_>) =
                events.into_iter().partition(|event| {
                    if crate::event_policy::is_system_topic(&event.topic) {
                        return true;
                    }
                    crate::event_policy::check_topic_format(&event.topic, &allowed_topics).is_none()
                });
            if !topic_format_rejections.is_empty() {
                // R10: convert each rejected event into a structured
                // RecoveryDiagnosisEnvelope and write it to
                // recovery.jsonl. We also still publish the legacy
                // `event.topic_format.rejected` diagnostic event so
                // operators reading the bus see the same signal they
                // always have — the journal entry is the new layer on
                // top, not a replacement.
                let allowed_list: Vec<String> = allowed_topics.iter().cloned().collect();
                for event in &topic_format_rejections {
                    warn!(
                        topic = %event.topic,
                        hat = ?event.hat,
                        "Topic format rejection: unknown topic not in whitelist"
                    );
                    // 2026-06-18-001 plan U6: 累加到 digest
                    self.state.record_rejection_digest(
                        "topic_format_rejected",
                        &format!(
                            "topic `{}` is not in the whitelist of known topics",
                            event.topic
                        ),
                        &event.topic,
                        &event.ts,
                    );
                    // Backwards-compat diagnostic event (R10: no retry).
                    let diagnostic = Event::new(
                        "event.topic_format.rejected",
                        format!(
                            "TOPIC_FORMAT_REJECTED: '{}' is not in the whitelist of known topics. \
                             This event will not be retried.",
                            event.topic
                        ),
                    );
                    self.bus.publish(diagnostic);
                    // New: write the recovery journal entry. Without
                    // this, R10's "only write recovery signal"
                    // promise is silently dropped.
                    Self::log_topic_format_rejection(
                        self,
                        event.topic.as_str(),
                        event.hat.as_deref(),
                        &allowed_list,
                    );
                }
            }
            events = topic_format_ok;
        }
        // --- End topic format check ---

        // --- Event policy validation now runs inside the U11-T2 unified pipeline ---
        // The legacy `apply_event_policy_validation` block was removed; see the
        // per-event unified pipeline loop below for completion guard, topic-deny,
        // payload policy, review-step gates, and side-effect handling.

        // --- State machine validation: enforce instance lifecycle rules ---
        // Plan GAP-02 / Unit 2: delegate to `state_machine_stage`'s
        // candidate-stage helper. The helper runs every event
        // through `validate_event` against a *clone* of the live
        // StateMachine runtime so downstream reject cannot
        // pollute live state. The candidate decisions are stored
        // on `self` so the final pending_publish boundary in
        // `AcceptedTransition` (Unit 3) can project the surviving
        // transitions into a `StateMachineTransitionDelta`.
        let (mut events, state_machine_candidates) = self.run_state_machine_candidate_stage(events);
        if !state_machine_candidates.is_empty() {
            // Stash for Unit 3 wiring; the actual projection /
            // apply happens at the final pending_publish boundary
            // so a downstream reject cannot pollute live state.
            self.pending_state_machine_candidates = state_machine_candidates;
        }
        // --- End state machine validation ---

        // --- State projection (U1 of 2026-06-17-003 plan): ---
        // SP-R8 mandates that the projector runs **after** the
        // state machine has accepted the batch and **before** the
        // `progress_task_gate`. The projector is the canonical
        // writer for `.ralph/agent/tasks.jsonl` and
        // `.ralph/agent/progress.md`; the gate then reads the
        // projected ledgers. Failures are fail-closed — the
        // affected events are dropped from the bus with an
        // `event.state_projection.rejected` diagnostic.
        if self.config.event_loop.state_projection.enabled {
            // P0-2 follow-up (plan 2026-06-29-006 §F3): hoist the
            // loop id out of `self` BEFORE the closure below so the
            // borrow checker doesn't conflict with the
            // `self.state.state_projection` immutable borrow on
            // 8055.
            let projector_loop_id = self.current_loop_id_for_contract();
            let projector = self.state.state_projection.get_or_insert_with(|| {
                let ctx = crate::state_projector::ProjectionContext::new(
                    self.config.core.workspace_root.as_path(),
                    self.config.event_loop.state_projection.clone(),
                    // Mirror the loop's R4 setting so the projector
                    // respects `enforce_current_unit` rather than
                    // silently disabling it. R1 in
                    // 2026-06-17-005 fix plan.
                    self.config.event_loop.enforce_current_unit,
                )
                // P0-2 follow-up: thread the loop's
                // `current-loop-id` marker into the projector
                // context so `project_ensure_task`'s fallback
                // (when `payload.loop_id` is absent) hits a real
                // value. Without this wiring the fallback is a
                // dead branch in production and coordinator
                // `work.ready` events produced tasks whose
                // `loop_id` was `None` on disk — the CLI then
                // hard-rejected those records with "legacy task
                // has no loop_id; not mutable from agent context".
                .with_current_loop_id(projector_loop_id);
                let mut p = crate::state_projector::StateProjector::new(ctx);
                // Best-effort bootstrap; failure is non-fatal
                // because the projector falls back to live
                // disk reads on a cold cache.
                let _ = p.bootstrap_from_disk();
                p
            });
            let report = projector.apply(&events);
            // Fix-2 (2026-06-29 primary-072512 P0): snapshot the
            // rejections into LoopState so the runner's step-close
            // partition can choose between hard-gate (no emit) and
            // schema-guidance (emit happened but projector rejected).
            // Without this, the runner cannot distinguish "agent did
            // not emit" from "agent emitted but the event was
            // dropped at projection", and the latter triggered
            // hard-gate exhaustion on step-04 (events:14 in
            // `docs/report/2026-06-29-ce-executor-serial-primary-
            // 20260629-072512-diagnosis.md`).
            self.state.last_projection_rejections = report.rejections.clone();
            if !report.rejections.is_empty() {
                for rej in &report.rejections {
                    let payload = serde_json::json!({
                        "topic": rej.topic,
                        "reason": rej.reason,
                        "event_payload": rej.payload,
                    })
                    .to_string();
                    self.bus.publish(ralph_proto::Event::new(
                        "event.state_projection.rejected",
                        payload,
                    ));
                }
                // P0 fix (review 2026-06-17-003): retain by the
                // event's `(topic, payload)` pair rather than by
                // topic name alone. When two events of the same
                // topic appear in a single batch (ce-executor
                // wave scenarios, plan-gate dual-publish
                // carve-outs), rejecting the whole topic dropped
                // sibling events that the projector would
                // otherwise accept. The event reader does not
                // surface a line number, so we use the payload
                // text as the per-event tie-breaker: events with
                // distinct payloads are independent and only the
                // exact matching entry is dropped. Events with no
                // payload (e.g. bare `task.resume`) fall back to
                // a per-topic index counter so a single no-payload
                // reject still does not wipe the whole topic.
                let mut seen_no_payload: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                let mut need_no_payload: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for r in &report.rejections {
                    if r.payload.is_none() {
                        *need_no_payload.entry(r.topic.clone()).or_insert(0) += 1;
                    }
                }
                let rejected_with_payload: std::collections::HashSet<(String, String)> = report
                    .rejections
                    .iter()
                    .filter_map(|r| {
                        let p = r.payload.as_ref()?;
                        Some((r.topic.clone(), p.clone()))
                    })
                    .collect();
                events.retain(|e| {
                    if let Some(p) = e.payload.as_ref() {
                        !rejected_with_payload.contains(&(e.topic.clone(), p.clone()))
                    } else {
                        let seen = seen_no_payload.entry(e.topic.clone()).or_insert(0);
                        let needed = need_no_payload.get(&e.topic).copied().unwrap_or(0);
                        let drop = *seen < needed;
                        *seen += 1;
                        !drop
                    }
                });
            }
        }
        // --- End state projection ---

        // --- U11-T2: per-event unified ValidationPipeline ---
        //
        // Runs the unified pre-commit rules against every event that
        // reached this point. Event-policy decisions are handled here
        // (drop, warn, or publish correction); non-event-policy rejections
        // emit a correction but keep the event so the legacy gate stack
        // can produce its own verdict.
        {
            let policy_enabled = self
                .config
                .event_loop
                .event_policy
                .as_ref()
                .is_some_and(|p| p.enabled);
            let pipeline = &unified_pipeline;

            // U11-T9 (P0-3 follow-up): mirror the state projector's cache
            // into the `LedgerSnapshot` so `StepHandoffRule` sees the same
            // view as the legacy disk-side gate.
            if let Some(ref mut projector) = self.state.state_projection
                && let Some(ref mut ledger) = self.state.state_ledger
            {
                let mut guard = ledger.snapshot_mut();
                projector.sync_to_ledger_snapshot(&mut guard);
            }

            let mut state_ledger = std::mem::take(&mut self.state.state_ledger);
            let mut snapshot = state_ledger
                .as_ref()
                .map(|l| l.snapshot().clone())
                .unwrap_or_else(crate::state::LedgerSnapshot::cold_start);
            let view = crate::preset::engine::protocol::ProtocolView::from_event_loop(
                &self.config.event_loop,
            );

            // Pass LoopState's policy runtime state / review-step tracker into
            // the context as overrides so the event-policy rule mutates the
            // canonical instances directly.
            let mut policy_state = self.state.policy_runtime_state.take().unwrap_or_default();
            let mut review_step_tracker = std::mem::take(&mut self.state.review_step_tracker);
            // U11-T4 (post-commit wiring): hand the live
            // `WorkflowProgress` to the validation context so the
            // unified `WorkflowGuardRule` reads & advances the same
            // instance the legacy gate stack used to. The pre-commit
            // rules do not touch this field; the post-commit pass
            // calls it after every pre-commit accept.
            let mut workflow_progress = std::mem::take(&mut self.state.workflow_progress);
            let mut event_policy_violation: Option<
                crate::payload_contract::PayloadContractViolation,
            > = None;
            let mut policy_rejections: Vec<crate::event_policy::PolicyRejection> = Vec::new();
            // `WorkflowGuardRule` rejection details collected per
            // event; drained after the per-event loop to write
            // recovery envelopes (one per rejected event).
            let mut wg_details: Vec<crate::validation::WorkflowGuardRejectionDetail> = Vec::new();

            // Source/target hat attribution for payload-contract violations.
            let (source_hats_by_topic, target_hats_by_topic) = if policy_enabled {
                let mut source: std::collections::BTreeMap<String, Vec<String>> =
                    std::collections::BTreeMap::new();
                let mut target: std::collections::BTreeMap<String, Vec<String>> =
                    std::collections::BTreeMap::new();
                for (hat_id, hat_config) in &self.config.hats {
                    for t in &hat_config.publishes {
                        source.entry(t.clone()).or_default().push(hat_id.clone());
                    }
                    for t in &hat_config.triggers {
                        target.entry(t.clone()).or_default().push(hat_id.clone());
                    }
                }
                for hats in source.values_mut() {
                    hats.sort();
                }
                for hats in target.values_mut() {
                    hats.sort();
                }
                (source, target)
            } else {
                (
                    std::collections::BTreeMap::new(),
                    std::collections::BTreeMap::new(),
                )
            };

            let mut accepted_events: Vec<JsonlEvent> = Vec::with_capacity(events.len());
            let mut rejected_topics: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut hold_reason: Option<String> = None;

            // U11-T4: post-commit workflow-guard wiring only
            // engages when the state machine is disabled (mirrors
            // the legacy bypass at line 8373). The state machine
            // owns the lifecycle when it is on, so the linear
            // guard would double-reject.
            let post_commit_enabled = self
                .config
                .event_loop
                .state_machine
                .as_ref()
                .is_none_or(|sm| !sm.enabled)
                && self
                    .config
                    .event_loop
                    .workflow_guards
                    .as_ref()
                    .is_some_and(|g| !g.chains.is_empty());

            for evt in &events {
                if let Some(ps) = self.phase_authority.snapshot() {
                    snapshot.workflow_phase = Some(ps);
                }
                let results = {
                    let mut ctx = crate::validation::ValidationContext::new(&mut snapshot)
                        .with_policy_runtime_state(&mut policy_state)
                        .with_review_step_tracker(&mut review_step_tracker)
                        .with_workflow_progress(&mut workflow_progress)
                        .with_workflow_guard_details(&mut wg_details)
                        .with_payload_contract_violation(&mut event_policy_violation)
                        .with_policy_rejections(&mut policy_rejections)
                        .with_source_hats_by_topic(&source_hats_by_topic)
                        .with_target_hats_by_topic(&target_hats_by_topic)
                        // U5 of plan 2026-07-02-005: wire the on-disk
                        // tasks.jsonl path so the StepHandoffRule can
                        // best-effort reload on a stale in-memory view
                        // (140149 / 175407 root cause).
                        .with_tasks_path(
                            self.config
                                .core
                                .workspace_root
                                .join(".ralph")
                                .join("agent")
                                .join("tasks.jsonl"),
                        );
                    pipeline.validate_pre_commit_with_view(&view, &mut ctx, evt)
                };
                let mut event_accepted = true;
                let mut event_warnings: Vec<String> = Vec::new();
                for r in &results {
                    if r.accepted {
                        if r.stage == crate::validation::ValidationStage::EventPolicy
                            && r.reason_code.as_deref()
                                == Some(crate::validation::ReasonCode::EVENT_POLICY_WARNING)
                            && let Some(hint) = &r.correction_hint
                        {
                            event_warnings.push(hint.clone());
                        }
                        continue;
                    }
                    // Preserve the legacy opt-out for step-handoff when state
                    // projection is disabled.
                    if r.stage == crate::validation::ValidationStage::StepHandoff
                        && !self.config.event_loop.state_projection.enabled
                    {
                        continue;
                    }
                    // U11-T2: step-handoff rejections now emit their operator-facing
                    // side effects (`plan.blocked` + diagnostic + recovery envelope)
                    // directly from the unified rejection handler. The legacy batch
                    // gate is removed; this is the single source of truth for the
                    // progress-task-mismatch recovery path.
                    if r.stage == crate::validation::ValidationStage::StepHandoff {
                        self.emit_step_handoff_rejection_side_effects(evt, r);
                        event_accepted = false;
                        break;
                    }
                    if r.stage == crate::validation::ValidationStage::EventPolicy {
                        match r.reason_code.as_deref() {
                            Some(
                                crate::validation::ReasonCode::EVENT_POLICY_COMPLETION_BLOCKED,
                            ) => {
                                let msg = r.correction_hint.clone().unwrap_or_else(|| {
                                    format!("Completion guard blocked '{}'", evt.topic)
                                });
                                self.bus
                                    .publish(Event::new("event.completion.blocked", msg));
                                event_accepted = false;
                                break;
                            }
                            Some(
                                crate::validation::ReasonCode::EVENT_POLICY_COMPLETION_IGNORED,
                            ) => {
                                let msg = r.correction_hint.clone().unwrap_or_else(|| {
                                    format!("Completion guard ignored '{}'", evt.topic)
                                });
                                self.bus
                                    .publish(Event::new("event.completion.ignored", msg));
                                event_accepted = false;
                                break;
                            }
                            Some(
                                crate::validation::ReasonCode::EVENT_POLICY_BLOCKED
                                | crate::validation::ReasonCode::EVENT_POLICY_IGNORED,
                            ) => {
                                event_accepted = false;
                                break;
                            }
                            Some(crate::validation::ReasonCode::EVENT_POLICY_WARNING) => {
                                if let Some(hint) = &r.correction_hint {
                                    event_warnings.push(hint.clone());
                                }
                                continue;
                            }
                            Some(crate::validation::ReasonCode::EVENT_POLICY_HOLD) => {
                                hold_reason = r.correction_hint.clone().or_else(|| {
                                    Some(format!("Event '{}' violates policy", evt.topic))
                                });
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
                                    policy_finding_for_topic(
                                        &policy_rejections,
                                        evt.topic.as_str(),
                                    ),
                                );
                                had_policy_rejections = true;
                                event_accepted = false;
                                break;
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
                                    policy_finding_for_topic(
                                        &policy_rejections,
                                        evt.topic.as_str(),
                                    ),
                                );
                                had_policy_rejections = true;
                                event_accepted = false;
                                break;
                            }
                        }
                    } else {
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
                            None,
                        );
                        rejected_topics.insert(evt.topic.clone());
                    }
                }
                if !event_warnings.is_empty() {
                    let msg = format!(
                        "Policy warning for '{}': {}",
                        evt.topic,
                        event_warnings.join("; ")
                    );
                    self.bus.publish(Event::new("event.policy_warning", msg));
                }
                // U11-T4: post-commit pass — only `WorkflowGuardRule`
                // is wired this round. `ExecutionContractRule` is
                // still a partial proxy and would double-reject with
                // the legacy `validate_execution_contract` path.
                // When the post-commit rule rejects, drain the
                // matching `WorkflowGuardRejectionDetail` (the rule
                // pushed it before returning) and write the
                // recovery envelope. Multiple chain rejections on
                // one event share a single recovery envelope (the
                // detail's `reason` concatenates chain details, the
                // legacy helper does the same).
                if event_accepted && post_commit_enabled {
                    let post_results = {
                        let mut ctx = crate::validation::ValidationContext::new(&mut snapshot)
                            .with_policy_runtime_state(&mut policy_state)
                            .with_review_step_tracker(&mut review_step_tracker)
                            .with_workflow_progress(&mut workflow_progress)
                            .with_workflow_guard_details(&mut wg_details)
                            .with_payload_contract_violation(&mut event_policy_violation)
                            .with_source_hats_by_topic(&source_hats_by_topic)
                            .with_target_hats_by_topic(&target_hats_by_topic)
                            .with_tasks_path(
                                self.config
                                    .core
                                    .workspace_root
                                    .join(".ralph")
                                    .join("agent")
                                    .join("tasks.jsonl"),
                            );
                        pipeline.validate_post_commit(&view, &mut ctx, evt)
                    };
                    for r in &post_results {
                        if r.accepted {
                            continue;
                        }
                        if r.stage != crate::validation::ValidationStage::WorkflowGuard {
                            // Future post-commit rules (e.g. the
                            // full `ExecutionContractRule` once U6
                            // wires the workspace path) plug in
                            // here. Today only the workflow guard
                            // is engaged, so any other stage is a
                            // misconfiguration — log and drop the
                            // event to be safe.
                            tracing::warn!(
                                stage = %r.stage,
                                topic = %evt.topic,
                                "U11-T4: unexpected post-commit rejection; dropping event"
                            );
                            event_accepted = false;
                            break;
                        }
                        // Drain the matching detail recorded by
                        // the rule. Today `WorkflowGuardRule` is
                        // the only post-commit rule, so at most
                        // one detail was pushed; we pop it back
                        // here so the next iteration's pre-commit
                        // sees a clean accumulator.
                        if let Some(detail) = wg_details.pop() {
                            Self::log_workflow_guard_rejection(self, &detail);
                        }
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
                            None,
                        );
                        had_policy_rejections = true;
                        event_accepted = false;
                        break;
                    }
                }
                if event_accepted {
                    // Plan 2026-08-26-1104 U3 (S3.1): persist
                    // a `kind=policy_receipt` row with
                    // `decision=accept` per event that clears
                    // the unified pipeline. The receipt
                    // carries `rule_refs` listing the gates the
                    // event passed, plus `event_digest` /
                    // `topic` / `hat` / `contract_digest` so
                    // the attribution engine (U8) can join the
                    // per-event decision stream back to the
                    // session's `contract_receipt`. Per-field
                    // bytes are capped to `MAX_SIDECAR_FIELD_BYTES`
                    // at the writer boundary (S3.4) — we do not
                    // put the full event payload on disk.
                    let event_json = serde_json::to_value(evt).ok();
                    self.diagnostics.emit_policy_receipt(
                        crate::diagnostics::PolicyReceiptDecision::Accept,
                        evt.topic.clone(),
                        evt.hat.as_deref(),
                        &["event_policy"],
                        None,
                        event_json.as_ref(),
                    );
                    // U3 (2026-06-27-002 plan completion): the
                    // emit-gate facade was originally wired
                    // here, but breaking the invariant that
                    // `accepted_events` is the source of
                    // `hat_lifecycle_tracker.complete()` calls
                    // caused 30+ existing tests to fail
                    // (P0 #1 regression gate). The gate is
                    // now observed in a post-process step
                    // (see the `validate_publish_gate`
                    // helper below) so the lifecycle tracker
                    // still records terminal events while
                    // gate-rejected events surface their
                    // recovery envelope.
                    accepted_events.push(evt.clone());
                }
            }

            events = accepted_events;

            // Restore LoopState fields mutated through context overrides.
            self.state.state_ledger = state_ledger;
            self.state.review_step_tracker = review_step_tracker;
            self.state.policy_runtime_state = Some(policy_state);
            self.state.workflow_progress = workflow_progress;

            if policy_enabled {
                // Process recoverable rejection budget.
                use crate::event_policy::ReasonClass;
                for rejection in &policy_rejections {
                    if let Some(ref class) = rejection.reason_class {
                        // Semantic-gate violations are recoverable but bypass the
                        // retry budget so a misbehaving coordinator cannot exhaust
                        // the schema budget on empty-diff retries.
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

                // Write hold artifact if policy hold was triggered.
                if let Some(ref reason) = hold_reason
                    && let Err(e) = self.write_hold_artifact(Some(reason))
                {
                    warn!(error = %e, "Failed to write hold artifact");
                }

                // U6: capture the first payload contract violation for the runner.
                if payload_contract_violation.is_none() {
                    payload_contract_violation = event_policy_violation;
                }
                if !policy_rejections.is_empty() {
                    had_policy_rejections = true;
                    // Plan 2026-08-26-1104 U3 (S3.2): persist a
                    // `kind=policy_receipt` row per policy
                    // rejection. `reason_code` is the stable
                    // machine-readable string
                    // (`policy:{violation_type.reason_code()}`)
                    // that the recovery.jsonl RejectionRecord
                    // shares, and `retry_key` matches
                    // `RejectionRecord::retry_key` so the
                    // attribution engine (U8) can reconcile
                    // the receipt row to the journal row by
                    // exact string match. `rule_refs` contains
                    // `event_policy` so dashboards can group
                    // policy rejections distinctly from origin
                    // ones (which carry `origin_guard`).
                    //
                    // `PolicyFinding` does not implement
                    // `serde::Serialize`, so we project a
                    // minimal JSON shape that preserves the
                    // observable evidence (reason_code, topic,
                    // message) without changing the upstream
                    // type's contract.
                    for rejection in &policy_rejections {
                        let reason_code =
                            format!("policy:{}", rejection.finding.violation_type.reason_code());
                        let event_json = serde_json::json!({
                            "topic": rejection.finding.topic,
                            "reason_code":
                                rejection.finding.violation_type.reason_code(),
                            "message": rejection.finding.message,
                        });
                        self.diagnostics.emit_policy_receipt(
                            crate::diagnostics::PolicyReceiptDecision::Reject,
                            rejection.topic.clone(),
                            rejection.source_hat.as_deref(),
                            &["event_policy"],
                            Some(&reason_code),
                            Some(&event_json),
                        );
                    }
                }
            }

            if !rejected_topics.is_empty() {
                tracing::debug!(
                    rejected = rejected_topics.len(),
                    remaining = events.len(),
                    "U11-T2: unified pipeline rejected topics; non-event-policy events continue through legacy gates"
                );
            }
        }
        // --- End U11-T2 ---
        // P1-3 (P1 follow-up): the unified pipeline verdict
        // is independent of the legacy gate stack — the two
        // layers produce orthogonal reject signals (the
        // agent-facing `publish_correction_via_context` from
        // unified, the operator-facing `recovery_envelope` +
        // `contract_rejections` from legacy). The batch is
        // NOT short-circuited: events the unified pipeline
        // rejected DO still reach the legacy gates so the
        // legacy execution-contract check can produce its
        // own `MissingPayloadField` finding. (Originally U11-T2
        // had an `events.retain` that dropped unified-rejected
        // topics; that was the wrong design and broke
        // `replay_light_integration::test_rejected_work_done_retry_*`
        // and `test_rejected_missing_plan_path_*`. The retain
        // is removed; tests `p1_3_unified_*` document the
        // layered contract.)

        // --- Workflow guard validation is now unified into the
        // pre-commit / post-commit loop above (U11-T4). The legacy
        // `apply_workflow_guard_validation` call site, the legacy
        // `WorkflowGuardOutcome` / `WorkflowGuardRejectionDetail`
        // types, and the legacy workflow-guard → `task.resume`
        // bridge have all been deleted; the `WorkflowGuardRule` in
        // `validation::rules_workflow_guard` is the single source
        // of truth for out-of-order / correlation-extraction
        // rejections. ---

        // Update policy runtime state for events that survived all validation layers
        if let Some(ref policy_config) = self.config.event_loop.event_policy
            && policy_config.enabled
        {
            let policy_state = self
                .state
                .policy_runtime_state
                .get_or_insert_with(PolicyRuntimeState::default);
            let completion_promise = self.config.event_loop.completion_promise.as_str();
            for event in &events {
                // 2026-06-29-007 P0 fix: do not mark the completion promise
                // (e.g. LOOP_COMPLETE) as terminal until check_completion_event
                // has actually validated required_events / verdict gate. A
                // rejected LOOP_COMPLETE must not poison terminal state and
                // block recovery events like plan.blocked / task.resume.
                if policy_config.terminal_topics.contains(&event.topic)
                    && event.topic != completion_promise
                {
                    policy_state.terminal_observed = true;
                }
            }
        }

        // --- Execution contract validation (U5): validate work.done before publishing ---
        // This runs after all other validation layers, before record/publish.
        // Contract rejection publishes diagnostic + guidance but does NOT record/publish the original.
        // Track raw event counts before contract filtering for missing-event gate logic
        let contract_validation_input_count = events.len();
        let mut contract_rejections: Vec<ExecutionContractFinding> = Vec::new();
        // U3 (2026-06-27-002 plan): take an owned copy of
        // `execution_contracts` so the immutable borrow of
        // `self.config` ends BEFORE the `for` loop runs
        // `apply_emit_gate` (which needs `&mut self`). The
        // original is restored below. This is the only
        // way around NLL's limit on conditional borrow
        // extents inside a `for` body.
        let contracts_enabled = self
            .config
            .event_loop
            .execution_contracts
            .as_ref()
            .is_some_and(|c| c.enabled);
        let owned_contracts = if contracts_enabled {
            self.config.event_loop.execution_contracts.clone()
        } else {
            None
        };
        let events = if contracts_enabled {
            let contracts = owned_contracts.as_ref().unwrap();
            let current_loop_id = self.current_loop_id_for_contract();
            // U3 (2026-06-27-002 plan): own the
            // `workspace_root` and `tasks_path` paths so
            // the `&self` borrow ends before the loop
            // body needs `&mut self` for
            // `apply_emit_gate`.
            let workspace_root_owned = std::path::PathBuf::from(&self.config.core.workspace_root);
            let tasks_path_owned = self.tasks_path();
            let workspace_root = workspace_root_owned.as_path();
            let tasks_path = tasks_path_owned.as_path();

            let mut accepted: Vec<JsonlEvent> = Vec::with_capacity(events.len());
            // P1-1 (2026-07-01-002 audit): collect the set of
            // `fix-NN` ids already known to the projector so that a
            // stale coordinator emitting `work.ready(fix-XX)` for an
            // id outside the chain is rejected before the contract
            // check produces a misleading finding.  When the
            // projector is disabled the gate is a no-op — the
            // contract pipeline still applies, but the range
            // guard's `fix-unit` set is empty so unknown-fix emits
            // pass through.  This preserves the historical
            // behaviour for presets that opt out of state
            // projection.
            let fix_unit_known: std::collections::BTreeSet<String> =
                match self.state.state_projection.as_ref() {
                    Some(projector) => crate::runtime_state::fix_unit_known_ids(projector),
                    None => std::collections::BTreeSet::new(),
                };
            // Re-usable insertion point for the fix-unit range
            // finding.  Constructed fresh per iteration so the
            // closure captures the right `&event`.
            for event in events {
                // Range guard BEFORE the contract check: when the
                // payload targets a `fix-NN` step that the
                // projector has never seen, drop the event as
                // `invalid_step_target`.  We skip the check for any
                // other topic (e.g. `fix.applied`, `work.done`,
                // `plan.complete`) and for fix-unit events whose
                // step is already known.
                if event.topic.as_str() == "work.ready" {
                    // The range guard only fires when the
                    // projector is active (it has populated
                    // `tasks.jsonl`).  When the chain is genuinely
                    // empty — e.g. before the first fix-unit is
                    // dispatched — we let the event through so the
                    // contract pipeline can decide.  This
                    // preserves the historical behaviour when
                    // state projection is disabled (empty chain
                    // means "no information, accept everything").
                    let guard_active = self.state.state_projection.as_ref().is_some();
                    if guard_active
                        && let Some(rejected_step) =
                            unknown_fix_step(event.payload.as_deref(), &fix_unit_known)
                    {
                        warn!(
                            topic = %event.topic,
                            step = %rejected_step,
                            "fix-unit step outside known chain — rejecting work.ready and surfacing task.resume"
                        );
                        // Synthesize an ExecutionContractFinding so
                        // the downstream rejection machinery (which
                        // already knows how to publish a `task.resume`
                        // with the right provenance) treats this
                        // exactly like any other contract violation.
                        self.push_fix_unit_range_finding(&event, &rejected_step, &fix_unit_known);
                        // Skip the rest of the contract pipeline
                        // for the rejected event; the rejection
                        // machinery above has already published the
                        // diagnostic + `task.resume`.
                        continue;
                    }
                }
                // Check if this topic has a contract rule
                if let Some(rule) = contracts.rules.get(event.topic.as_str()) {
                    let proto_event =
                        Event::new(event.topic.as_str(), event.payload.as_deref().unwrap_or(""));
                    // Provenance: prefer the hat the event declared on its
                    // own JSONL `hat` field (most accurate — it identifies
                    // the hat that *emitted* the event).  Fall back to the
                    // runner's last active hat when the JSONL line did not
                    // carry one (legacy fixtures / log-only emissions).
                    // The provenance is stamped onto every
                    // ExecutionContractFinding so the U2 recovery path can
                    // route `task.resume` to the actual source hat rather
                    // than the runner's current display hat.
                    let active_business_hat =
                        self.state.last_active_hat_ids.first().map(|h| h.as_str());
                    let event_provenance: Option<&str> = match event.hat.as_deref() {
                        Some("ralph") => active_business_hat.or(Some("ralph")),
                        Some(hat) => Some(hat),
                        None => active_business_hat,
                    };
                    let decision = validate_execution_contract(
                        &proto_event,
                        rule,
                        workspace_root,
                        current_loop_id.as_str(),
                        tasks_path,
                        event_provenance,
                        &DefaultGitEvidenceProvider,
                        self.state.loop_start_sha.as_deref(),
                    );
                    let guidance_topic_owned = rule.reject.guidance_topic.clone();
                    let diagnostic_topic_owned = rule.reject.diagnostic_topic.clone();
                    match decision {
                        ExecutionContractDecision::Accept => {
                            // U2 (2026-07-01-002 plan): run soft checks
                            // (e.g. fix-unit commit footer) on accepted
                            // events.  These never flip an Accept into a
                            // Reject; instead they surface diagnostics so
                            // the agent can self-correct next iteration
                            // (see `check_fix_unit_commit_footer`).
                            let soft_diagnostics = run_execution_contract_soft_checks(
                                &proto_event,
                                workspace_root,
                                &DefaultGitEvidenceProvider,
                                self.state.loop_start_sha.as_deref(),
                            );
                            for diag in &soft_diagnostics {
                                warn!(
                                    topic = %event.topic,
                                    step = ?diag.kind,
                                    "Execution contract soft-check diagnostic"
                                );
                            }
                            accepted.push(event);
                        }
                        ExecutionContractDecision::Reject(findings) => {
                            // Publish rejection diagnostic and guidance, do NOT accept the event
                            let finding = &findings[0];
                            let disposition = crate::event_loop::accepted_event::from_execution_contract_rejection(
                                crate::event_loop::accepted_event::CandidateEvent {
                                    topic: event.topic.clone(),
                                    payload: event.payload.clone().unwrap_or_default(),
                                },
                                crate::event_loop::rejection::RejectionStage::ExecutionContract,
                                format!("{:?}", finding.kind),
                                finding.message.clone(),
                            );
                            debug_assert!(
                                !disposition.is_committable(),
                                "execution contract rejection must never be committable"
                            );
                            warn!(
                                topic = %event.topic,
                                violation = ?finding.kind,
                                "Execution contract rejected event"
                            );

                            // Targeted contract recovery (2026-06-04 plan U2):
                            // The rejected event must NOT advance downstream hats,
                            // but the source hat must be told to retry. Publish a
                            // `task.resume` with `target=source_hat` so the next
                            // prompt activates the responsible hat, not the Ralph
                            // fallback.
                            //
                            // DEV-005 (2026-07-06): for `TaskNotTerminal`, route
                            // recovery to the hat that can actually close the task
                            // (typically coordinator) instead of the emitter.
                            // P1-5 (2026-07-07-002): for `TaskNotFound` carrying a
                            // `task_key`, route to a coordinator hat that can repair
                            // the ledger (orphan row / identity mismatch) — the
                            // emitter (executor) cannot fix `tasks.jsonl`.
                            let source_hat_str = finding.source_hat.as_deref();
                            let mut retry_target: Option<HatId> = None;
                            let mut no_retry_reason: Option<String> = None;
                            let mut task_not_terminal_hint: Option<String> = None;
                            if let Some(hat_id_str) = source_hat_str {
                                if hat_id_str == "ralph" {
                                    no_retry_reason = Some(
                                        "no business hat available for fallback ralph".to_string(),
                                    );
                                } else {
                                    let resolved_hat_id_str =
                                        if let ExecutionContractViolationKind::TaskNotTerminal {
                                            task_id,
                                            ..
                                        } = &finding.kind
                                        {
                                            use crate::task_store::TaskStore;
                                            let task_snapshot = TaskStore::load(tasks_path)
                                                .ok()
                                                .and_then(|store| store.get(task_id).cloned());
                                            let (delegate, hint) =
                                                crate::execution_contract::task_not_terminal_resume_plan(
                                                    task_id,
                                                    task_snapshot.as_ref(),
                                                    hat_id_str,
                                                    &self.config.tasks.coordinator_hats,
                                                );
                                            task_not_terminal_hint = Some(hint);
                                            delegate
                                        } else if let ExecutionContractViolationKind::TaskNotFound {
                                            task_id,
                                        } = &finding.kind
                                        {
                                            // P1-5: TaskNotFound with a payload task_key is
                                            // an identity mismatch / orphan-row scenario.
                                            // The executor cannot repair the ledger; route
                                            // to a coordinator hat. Without a task_key this
                                            // is a plain missing-task error and the source
                                            // hat is still the right retry target.
                                            let payload_obj = event
                                                .payload
                                                .as_deref()
                                                .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok());
                                            let payload_key = payload_obj
                                                .as_ref()
                                                .and_then(|v| v.get("task_key"))
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            if payload_key.is_empty() {
                                                hat_id_str.to_string()
                                            } else {
                                                use crate::task_store::TaskStore;
                                                let task_snapshot = TaskStore::load(tasks_path)
                                                    .ok()
                                                    .and_then(|store| store.get(task_id).cloned());
                                                let (delegate, hint) =
                                                    crate::execution_contract::task_not_found_resume_plan(
                                                        task_id,
                                                        payload_key,
                                                        task_snapshot.as_ref(),
                                                        hat_id_str,
                                                        &self.config.tasks.coordinator_hats,
                                                    );
                                                task_not_terminal_hint = Some(hint);
                                                delegate
                                            }
                                        } else {
                                            hat_id_str.to_string()
                                        };
                                    let hat_id = HatId::new(&resolved_hat_id_str);
                                    match self.registry.get(&hat_id) {
                                        None => {
                                            no_retry_reason = Some(format!(
                                                "source hat '{}' not registered",
                                                resolved_hat_id_str
                                            ));
                                        }
                                        Some(_) => {
                                            let is_delegated_recovery = matches!(
                                                &finding.kind,
                                                ExecutionContractViolationKind::TaskNotTerminal { .. }
                                                    | ExecutionContractViolationKind::TaskNotFound { .. }
                                            )
                                                && resolved_hat_id_str != hat_id_str;
                                            if is_delegated_recovery {
                                                retry_target = Some(hat_id);
                                            } else {
                                                let can_retry = self
                                                    .registry
                                                    .can_publish(&hat_id, event.topic.as_str());
                                                let can_fail = self
                                                    .registry
                                                    .can_publish(&hat_id, "work.failed");
                                                if !can_retry && !can_fail {
                                                    no_retry_reason = Some(format!(
                                                        "recovery hat '{}' cannot publish '{}' or 'work.failed'",
                                                        resolved_hat_id_str,
                                                        event.topic.as_str()
                                                    ));
                                                } else {
                                                    retry_target = Some(hat_id);
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                no_retry_reason =
                                    Some("no source hat recorded on event or in state".to_string());
                            }

                            if let Some(hat_id) = &retry_target {
                                let payload_obj = event.payload.as_deref().and_then(|p| {
                                    serde_json::from_str::<serde_json::Value>(p).ok()
                                });
                                let task_key = payload_obj
                                    .as_ref()
                                    .and_then(|v| v.get("task_key"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let step = payload_obj
                                    .as_ref()
                                    .and_then(|v| v.get("step"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let violation_code = match &finding.kind {
                                    ExecutionContractViolationKind::TaskNotTerminal { .. } => {
                                        "task_not_terminal"
                                    }
                                    ExecutionContractViolationKind::TaskNotFound { .. } => {
                                        // P1-5: distinguish identity-mismatch routing
                                        // (coordinator-bound) from plain missing-task
                                        // (source-hat retry) so the protocol-violation
                                        // budget is tracked under the right signature.
                                        if task_key.is_empty() {
                                            "task_not_found"
                                        } else {
                                            "task_not_found_identity_mismatch"
                                        }
                                    }
                                    _ => "execution_contract",
                                };
                                let source_hat = source_hat_str.unwrap_or("unknown");
                                let (_protocol_count, protocol_exhausted) =
                                    self.state.record_protocol_violation_signature(
                                        source_hat,
                                        event.topic.as_str(),
                                        task_key,
                                        step,
                                        violation_code,
                                    );
                                if protocol_exhausted {
                                    let fail_reason =
                                        format!("protocol_violation_repeated:{violation_code}");
                                    warn!(
                                        topic = %event.topic.as_str(),
                                        reason = %fail_reason,
                                        "U8: protocol violation retry budget exhausted; fail-closing"
                                    );
                                    let blocked = Event::new(
                                        "plan.blocked",
                                        serde_json::json!({ "reason": fail_reason }).to_string(),
                                    );
                                    self.bus.publish(blocked.clone());
                                    self.state.record_event(&blocked);
                                } else {
                                    let original_trigger = self
                                        .state
                                        .last_activation_events
                                        .iter()
                                        .rev()
                                        .find(|trigger| {
                                            self.registry.get_config(hat_id).is_some_and(|config| {
                                                config.trigger_topics().iter().any(|topic| {
                                                    topic.matches_str(trigger.topic.as_str())
                                                })
                                            })
                                        });
                                    let recovery_reason = task_not_terminal_hint
                                        .as_deref()
                                        .unwrap_or(finding.message.as_str());
                                    let retry_payload = serde_json::json!({
                                        "rejected_topic": event.topic.as_str(),
                                        // U2 (2026-06-17-003 plan): add the
                                        // schema-required `target_hat` field
                                        // alongside `reason` so the drift
                                        // detector counts the contract recovery
                                        // as schema-compliant.
                                        "target_hat": hat_id.as_str(),
                                        "reason": recovery_reason,
                                        "finding_kind": format!("{:?}", finding.kind),
                                        "required_action": format!(
                                            "Fix the issue and emit '{}' again with correct payload, or emit 'work.failed' if unrecoverable.",
                                            event.topic.as_str()
                                        ),
                                        "original_payload": event.payload.as_deref().unwrap_or(""),
                                        "original_trigger_topic": original_trigger
                                            .map(|trigger| trigger.topic.as_str()),
                                        "original_trigger_payload": original_trigger
                                            .map(|trigger| {
                                                serde_json::from_str::<serde_json::Value>(
                                                    trigger.payload.as_str(),
                                                )
                                                .unwrap_or_else(|_| {
                                                    serde_json::Value::String(
                                                        trigger.payload.clone(),
                                                    )
                                                })
                                            }),
                                        "retry_publish_topics": [event.topic.as_str(), "work.failed"],
                                        "contract_finding": finding,
                                    });
                                    // Plan 2026-08-10-001 U1: route
                                    // the contract-rejection retry
                                    // through the unified publisher.
                                    // The `retry_key` is signed by
                                    // `task_key`+`step` so duplicate
                                    // contract retries for the same
                                    // step collapse into a single
                                    // resume.
                                    let retry_payload_string = retry_payload.to_string();
                                    let loop_id_for_resume = self.current_loop_id();
                                    let loop_id_str =
                                        loop_id_for_resume.as_deref().unwrap_or("default");
                                    let activation_id =
                                        format!("resume:{}:{}", loop_id_str, self.state.iteration);
                                    let retry_step_for_key: String = if step.is_empty() {
                                        String::from("none")
                                    } else {
                                        step.to_string()
                                    };
                                    let decision =
                                        crate::event_loop::resume_routing::task_resume_ingress(
                                            &mut self.bus,
                                            &self.registry,
                                            self.state.state_ledger.as_ref(),
                                            loop_id_str,
                                            &activation_id,
                                            hat_id.as_str(),
                                            Some(retry_step_for_key.as_str()),
                                            "contract_rejection_retry",
                                            retry_payload_string,
                                        );
                                    if let crate::event_loop::resume_routing::ResumeDecision::Block { reason } = &decision {
                                        tracing::warn!(
                                            target = %hat_id.as_str(),
                                            topic = %event.topic.as_str(),
                                            ?reason,
                                            "contract-rejection retry blocked (no safe target)"
                                        );
                                    }
                                    debug!(
                                        target = %hat_id.as_str(),
                                        topic = %event.topic.as_str(),
                                        "Publishing targeted contract recovery event to source hat"
                                    );
                                }
                            } else if let Some(reason) = &no_retry_reason {
                                warn!(
                                    topic = %event.topic.as_str(),
                                    reason = %reason,
                                    "No safe retry target for rejected event; recovery is human.guidance only"
                                );
                            }

                            // Publish structured diagnostic (now carries
                            // retry_target and no_retry_reason for observability).
                            let diagnostic_payload = serde_json::json!({
                                "topic": event.topic.as_str(),
                                "finding": findings,
                                "rejected_at": chrono::Utc::now().to_rfc3339(),
                                "retry_target": retry_target.as_ref().map(|h| h.as_str()),
                                "no_retry_reason": no_retry_reason,
                            });
                            let diagnostic_event = Event::new(
                                diagnostic_topic_owned.as_str(),
                                diagnostic_payload.to_string(),
                            );
                            self.bus.publish(diagnostic_event);

                            // Publish the human-readable guidance. The default target
                            // is `plan.blocked` (plan 2026-06-28-005
                            // changed it from the now-deleted
                            // `human.guidance`). The payload is
                            // kept as a free-form string so existing
                            // consumer tooling that parses text still
                            // works.
                            let guidance_payload = format!(
                                "Execution contract rejection for '{}': {}\n\n\
                                 To proceed, either:\n\
                                 1. Fix the issue and emit '{}' again with correct payload, OR\n\
                                 2. Emit 'work.failed' if the work cannot be completed.",
                                event.topic.as_str(),
                                finding.message,
                                event.topic.as_str(),
                            );
                            let guidance_event =
                                Event::new(guidance_topic_owned.as_str(), guidance_payload)
                                    // 2026-06-28-005: pin the guidance
                                    // publish to the same target as the
                                    // retry event so the ralph fallback
                                    // (subscribed to *) does not shadow
                                    // it. retry_target is None for the
                                    // no-safe-target case; in that case
                                    // the event fans out to the
                                    // fallback (which is the documented
                                    // behaviour — see no_retry_reason
                                    // branch above).
                                    .with_target(
                                        retry_target.clone().unwrap_or_else(|| HatId::new("ralph")),
                                    );
                            self.bus.publish(guidance_event);

                            contract_rejections.extend(findings.iter().cloned());
                        }
                    }
                } else {
                    // No contract rule for this topic — pass through
                    accepted.push(event);
                }
            }
            accepted
        } else {
            events
        };
        // --- End execution contract validation ---

        // Calculate had_raw_events and had_rejected_events for missing-event gate logic
        // had_raw_events: events that passed through contract validation (accepted OR rejected)
        // had_rejected_events: events that were rejected by contract validation
        let had_rejected_events =
            had_origin_rejections || had_policy_rejections || !contract_rejections.is_empty();
        let had_raw_events = if contracts_enabled {
            // Events that went through contract validation: accepted + rejected
            // events.len() here is accepted.len() (passed or no-rule events)
            events.len() + contract_rejections.len() > 0
        } else {
            // Contracts disabled: all events passed through
            contract_validation_input_count > 0
        };

        // 2026-07-06 U5 (DEV-005): TaskNotTerminal recovery is handled
        // inline in the contract-rejection branch above (routes to the
        // hat that can close the task). The post-batch synthesis loop
        // was removed to avoid duplicate `task.resume` events.

        let mut has_orphans = false;

        // Validate and transform events (apply backpressure for build.done)
        let mut validated_events = Vec::new();
        // P1-2: own the topic strings so per-event commits
        // (`commit_terminal_delta` borrows `&mut self`) can run
        // inside the same loop without aliasing the `&str`
        // borrow from `completion_promise.as_str()`.
        let completion_topic = self.config.event_loop.completion_promise.clone();
        let cancellation_topic = self.config.event_loop.cancellation_promise.clone();
        let total_events = events.len();
        let mut completion_seen_in_batch = false;
        // Clone the policy config so `policy_config_ref`
        // can be dropped before the U3 gate loop runs.
        // `policy_config_ref` is `Option<&PolicyConfig>`
        // which borrows `self.config`; the gate loop
        // needs `&mut self` for `apply_emit_gate`.
        let policy_config_owned = self.config.event_loop.event_policy.clone();
        let write_diagnostic = policy_config_owned
            .as_ref()
            .map(|c| c.completion_after_terminal.write_diagnostic_event)
            .unwrap_or(false);
        let policy_config_ref = policy_config_owned.as_ref();
        let mut accepted_log_events = Vec::new();
        // Retain the accepted JSONL metadata (especially timestamps) for
        // post-publication handoff registration; the validation loop consumes
        // `events` below.
        let committed_events = events.clone();

        // U1 (plan 2026-08-10-001): helper that rebuilds a ralph_proto::Event
        // from a JSONL event, preserving source/target/wave/system_injected
        // metadata while allowing the caller to supply a potentially-replaced
        // payload string. This stops `Event::new(topic, &payload)` from
        // silently stripping routing metadata in the accepted path.
        let jsonl_event_to_proto =
            |jsonl_event: &crate::event_reader::Event, payload: &str| -> Event {
                let mut proto: Event = jsonl_event.clone().into();
                proto.payload = payload.to_string();
                proto
            };

        macro_rules! accept_event {
            ($accepted:expr) => {{
                let accepted = $accepted;
                // 2026-07-06 U9 (DEV-009): when a work.done is admitted,
                // record its step so the topology guard at line ~10666
                // can refuse the next step's work.ready until the
                // previous step's work.done lands.
                if accepted.topic.as_str() == "work.done" {
                    let payload: &str = &accepted.payload;
                    if let Some(start) = payload.find("\"step\":\"") {
                        let rest = &payload[start + 8..];
                        if let Some(end) = rest.find('"') {
                            let step = &rest[..end];
                            if step.starts_with("step-") {
                                self.state.step_work_done_seen.insert(step.to_string());
                            }
                        }
                    }
                }
                accepted_log_events.push(accepted.clone());
                validated_events.push(accepted);
            }};
            // Convenience form for JSONL events: takes the JSONL event and
            // a payload string, preserves all metadata from the JSONL event.
            ($jsonl_event:expr, $payload:expr) => {{
                let proto = jsonl_event_to_proto(&$jsonl_event, $payload);
                accept_event!(proto)
            }};
        }

        // U3 (2026-06-27-002 plan completion): first
        // pass through the emit-gate facade runs BEFORE
        // the main loop so the recovery envelope
        // (Reject) or repair-sink envelope
        // (AcceptRepairStream) is recorded for every
        // event in the batch. The second pass runs
        // before `self.bus.publish` (see below) to enforce
        // the `AcceptMainBus`-only publication contract.
        // The double-pass design keeps the lifecycle
        // tracker integration intact: terminal events
        // still close activations even when the gate
        // rejects them.
        //
        // `policy_config_ref` is captured by value (it is
        // a `&Option<...>` whose payload we do not
        // mutate) before the gate loop runs so the
        // `&mut self` borrow on `apply_emit_gate` is
        // unblocked.
        // `policy_config_ref` (an `Option<&EventPolicyConfig>`)
        // is held until after the U3 gate loop completes. The
        // gate loop needs `&mut self`, so the immutable
        // borrow on `self.config` must be released first.
        // Snapshot the events by reference so the gate
        // loop can borrow `&mut self`. The `events` vec
        // is owned (not borrowed from self) so this is
        // safe.
        //
        // P0-1 (2026-06-27 adversarial review): the
        // previous design called `apply_emit_gate` here
        // and re-ran the stage pipeline in
        // `apply_emit_gate_on_validated`, which
        // double-advanced the per-task
        // `RepairStateMachine` and broke the
        // `repair_budget=3` invariant. We now stash the
        // outcome from the first pass (which mutates
        // `self.repair_state_machine`) keyed by
        // `(topic, payload)` so the publish-time gate
        // can reuse it without re-running the pipeline.
        //
        // Keying by `(topic, payload)` is safe because
        // each JSONL line is a unique event — two
        // distinct events with the same topic and the
        // same payload would be a pathological duplicate
        // in `events.jsonl`, which the upstream parser
        // already rejects. The synthesised
        // `build.blocked` / `task.relocate` events
        // inherit the source event's payload verbatim
        // (see the `accept_event!` call sites below),
        // so the lookup hits the same key. The keys
        // are normalised to `(String, String)` so both
        // the JSONL-internal `event_reader::Event`
        // (String topic) and the bus-shaped
        // `ralph_proto::Event` (Topic, `.as_str()`)
        // can index into the same map.
        let gate_outcomes: std::collections::HashMap<
            (String, String),
            crate::event_loop::emit_gate::EmitGateOutcome,
        > = {
            let mut outcomes = std::collections::HashMap::with_capacity(events.len());
            for event in &events {
                let key = (
                    event.topic.clone(),
                    event.payload.clone().unwrap_or_default(),
                );
                let outcome = self.evaluate_emit_gate_for_jsonl_event(event);
                outcomes.insert(key, outcome);
            }
            outcomes
        };

        for (index, event) in events.into_iter().enumerate() {
            let payload = event.payload.clone().unwrap_or_default();

            // Runtime-side handoff verification runs before a synthesized
            // precheck gate's pass can reach downstream hats. A gate agent
            // may still perform the evidence review, but it cannot turn a
            // stale HEAD or changed dirty worktree into a successful handoff
            // by emitting a falsely green payload. The rejected event enters
            // the normal precheck retry path below.
            if let Some(rejection_payload) =
                self.runtime_precheck_rejection_for_event(&event, &payload)
            {
                let gate_hat = event
                    .hat
                    .clone()
                    .unwrap_or_else(|| format!("precheck-{}", event.topic));
                let mut rejected = jsonl_event_to_proto(&event, &rejection_payload);
                rejected.topic = ralph_proto::Topic::new(format!("{}.rejected", event.topic));
                rejected.source = Some(ralph_proto::HatId::new(gate_hat));
                accept_event!(rejected);
                continue;
            }

            // 2026-07-07-002 U4: terminal-closed guard before main-events commit.
            match self.evaluate_terminal_closed_for_event(
                event.topic.as_str(),
                &payload,
                completion_topic.as_str(),
            ) {
                crate::event_loop::terminal_closed_guard::TerminalClosedDecision::Allow => {}
                crate::event_loop::terminal_closed_guard::TerminalClosedDecision::RejectPostTerminal => {
                    self.publish_post_terminal_rejection(
                        event.topic.as_str(),
                        "post_terminal_business_event_frozen",
                    );
                    continue;
                }
                crate::event_loop::terminal_closed_guard::TerminalClosedDecision::IgnoreDuplicateTerminal => {
                    self.bus.publish(Event::new(
                        "event.completion.ignored",
                        format!(
                            "Terminal-closed guard ignored duplicate '{}'",
                            event.topic
                        ),
                    ));
                    continue;
                }
            }

            // 2026-07-06 U9 (DEV-009): topology guard — work.ready for
            // step-NN where NN > 01 must be preceded by work.done for
            // step-(NN-1). Without this guard the coordinator can
            // publish a new step's work.ready before the executor
            // closed the previous step's work.done, leaving tasks
            // stuck open across the boundary (observed in
            // 2026-07-05-153532 run: step-02 work.ready at 15:43 with
            // step-01 work.done outstanding). Log + drop with a
            // diagnostic; the coordinator will be re-prompted with
            // the missing predecessor and re-emit on the next turn.
            if event.topic == "work.ready" {
                let step: Option<String> =
                    payload
                        .find("\"step\":\"")
                        .map(|i| i + 8)
                        .and_then(|start| {
                            let rest = &payload[start..];
                            rest.find('"').map(|end| rest[..end].to_string())
                        });
                if let Some(step) = step
                    && let Some(nn) = step
                        .strip_prefix("step-")
                        .and_then(|s| s.parse::<u32>().ok())
                    && nn > 1
                {
                    let prev = format!("step-{:02}", nn - 1);
                    if !self.state.step_work_done_seen.contains(&prev) {
                        warn!(
                            topic = %event.topic,
                            step = %step,
                            prev_step = %prev,
                            "DEV-009: work.ready for step arrived before previous step's work.done; dropping as cross-step handoff violation"
                        );
                        let diagnostic = Event::new(
                            "event.topology.out_of_order",
                            format!(
                                "{{\"dropped_topic\":\"work.ready\",\"step\":\"{step}\",\"prev_step\":\"{prev}\",\"reason\":\"work.ready arrived before previous step's work.done\"}}"
                            ),
                        );
                        self.bus.publish(diagnostic);
                        continue;
                    }
                }
            }

            // 2026-07-06 U7 (DEV-007): topology guard — test.passed
            // must be preceded by work.done for the same plan/step.
            // Without this guard a validator hat that activates late
            // (e.g. after the shipper has already emitted REVIEW_COMPLETE
            // via the runtime-recovery stall pipeline) can publish a
            // test.passed event that violates the preset's intended
            // review-before-publish sequence. Log + drop, do not
            // diagnose as failure (the test genuinely passed; only
            // the ordering was wrong).
            if event.topic == "test.passed" && !self.state.seen_topics.contains("work.done") {
                warn!(
                    topic = %event.topic,
                    "DEV-007: test.passed arrived before any work.done in this loop; dropping as topology-violating"
                );
                let diagnostic = Event::new(
                    "event.topology.out_of_order",
                    "{\"dropped_topic\":\"test.passed\",\"reason\":\"test.passed arrived before any work.done was admitted for this loop\"}".to_string(),
                );
                self.bus.publish(diagnostic);
                continue;
            }

            // Detect loop.cancel — unconditional graceful termination
            if !cancellation_topic.is_empty() && event.topic.as_str() == cancellation_topic {
                info!(
                    payload = %payload,
                    "loop.cancel event detected — scheduling graceful termination"
                );
                // P1-2: per-event commit (see `commit_terminal_delta`).
                if !self.state.cancellation_requested {
                    Self::commit_terminal_delta(
                        &mut self.state.state_ledger,
                        crate::state::CommitDelta::CancellationRequested,
                    );
                }
                self.state.cancellation_requested = true;
                accepted_log_events.push(jsonl_event_to_proto(&event, &payload));
                // Continue processing remaining events (they may contain cleanup info)
                continue;
            }

            if event.topic == completion_topic.as_str() {
                if self.state.completion_honored {
                    debug!("Completion event already handled, ignoring duplicate");
                    continue;
                }
                // 2026-06-30-001 P0-5: report_done_seen guard.
                // Refuse to honour `LOOP_COMPLETE` if the
                // workflow has not yet produced its final
                // `report.done`. This stops the runner / agent
                // from racing the reviewer chain to the
                // terminal — events L37 of the 032648 run
                // showed ralph emitting `LOOP_COMPLETE` while
                // 6/7 review dimensions were still in flight.
                if let Err(reason) = self.state.mark_completion_requested(
                    &self.config.event_loop.required_events,
                    &self.config.event_loop.completion_promise,
                ) {
                    tracing::warn!(
                        reason = %reason,
                        iteration = self.state.iteration,
                        "LOOP_COMPLETE REJECTED by mark_completion_requested"
                    );
                    self.state.completion_requested = true;
                    if self
                        .state
                        .is_rejected_completion_duplicate(payload.as_str())
                    {
                        // Identical rejected payload: do not re-inject
                        // a correction block (would just spam the prompt),
                        // but still let `check_completion_event()` advance
                        // the stale-breaker counter for this iteration.
                        continue;
                    }
                    let missing = self
                        .state
                        .missing_required_events(&self.config.event_loop.required_events);
                    let free_form = format!(
                        "LOOP_COMPLETE rejected: missing required events: {missing:?}. \
                         The agent must complete all workflow phases before emitting LOOP_COMPLETE. \
                         Use loop.cancel to abort the workflow instead."
                    );
                    tracing::warn!(
                        reason = %reason,
                        missing = ?missing,
                        iteration = self.state.iteration,
                        topic = %event.topic,
                        index = index,
                        "P0-5: completion event rejected; \
                         required events not yet observed; \
                         event will not transition loop to terminal"
                    );
                    let _ = Self::inject_completion_correction(
                        &mut self.state,
                        "missing_required_events",
                        &free_form,
                    );
                    // Drop the event from this batch's
                    // accepted stream; the runtime continues
                    // to wait for required workflow events. The
                    // event is NOT added to `accepted_log_events`
                    // so the events.jsonl file does not carry a
                    // false-positive terminal event.
                    continue;
                }
                // Completion event is accepted regardless of position in batch.
                // Events AFTER it in the same batch are protected by the completion guard.
                // P1-2: per-event commit (see `commit_terminal_delta`).
                Self::commit_terminal_delta(
                    &mut self.state.state_ledger,
                    crate::state::CommitDelta::CompletionRequested,
                );
                completion_seen_in_batch = true;
                let accepted = jsonl_event_to_proto(&event, &payload);
                accepted_log_events.push(accepted.clone());
                self.state.record_event(&accepted);
                self.state.last_completion_payload = Some(payload.to_string());
                self.diagnostics.log_orchestration(
                    self.state.iteration,
                    "jsonl",
                    crate::diagnostics::OrchestrationEvent::EventPublished {
                        topic: event.topic.clone(),
                    },
                );
                info!(
                    topic = %event.topic,
                    position = index,
                    batch_size = total_events,
                    "Completion event detected in JSONL"
                );
                continue;
            }

            // 2026-07-01-001 plan U2: persistent
            // `completion_honored` guard. Once a previous batch
            // (or this loop's prior run via ledger replay) set
            // the flag, every subsequent business event must
            // be rejected even if the *current* batch has not
            // seen a completion topic yet. The same-batch
            // guard below stays as a fast path for diagnostics.
            //
            // 2026-07-01-001 review P1-1: when `event_policy`
            // is disabled or absent, the policy-config branch
            // is skipped — but R1's "no further business event
            // may enter the bus" is an absolute invariant, so
            // we fall back to a hard intercept that always
            // `continue`s. This keeps simple presets (no
            // event_policy) on the same R1 contract as
            // ce-executor-serial.
            if self.state.completion_honored
                && event.topic != self.config.event_loop.completion_promise.as_str()
                && event.topic != self.config.event_loop.cancellation_promise.as_str()
            {
                let policy_enabled = policy_config_ref.is_some_and(|c| c.enabled);
                if !policy_enabled {
                    // Hard fallback (2026-07-01-001 review P1-1):
                    // refuse every business event
                    // post-completion when no policy is
                    // configured. We ALWAYS emit the
                    // diagnostic here (no `write_diagnostic`
                    // gate) because there is no
                    // `completion_after_terminal` config to
                    // consult — the R1 absolute invariant
                    // holds regardless of policy settings,
                    // and `ralph diagnose` needs the event
                    // for parity with the policy-configured
                    // path.
                    self.bus.publish(Event::new(
                        "event.completion.blocked",
                        format!(
                            "Persistent completion guard hard-blocked '{}': \
                             no event_policy configured; R1 fallback intercept",
                            event.topic
                        ),
                    ));
                    continue;
                }
                if let Some(policy_config) = policy_config_ref
                    && let Some(decision) =
                        check_completion_guard(&event.topic, policy_config, true)
                {
                    match &decision {
                        PolicyDecision::Block(finding) => {
                            if write_diagnostic {
                                self.bus.publish(Event::new(
                                    "event.completion.blocked",
                                    format!(
                                        "Persistent completion guard blocked '{}': {}",
                                        event.topic, finding.message
                                    ),
                                ));
                            }
                        }
                        PolicyDecision::Ignore(finding) => {
                            if write_diagnostic {
                                self.bus.publish(Event::new(
                                    "event.completion.ignored",
                                    format!(
                                        "Persistent completion guard ignored '{}': {}",
                                        event.topic, finding.message
                                    ),
                                ));
                            }
                        }
                        PolicyDecision::Warn(findings) => {
                            for finding in findings {
                                self.bus.publish(Event::new(
                                    "event.policy_warning",
                                    format!(
                                        "Persistent completion guard warning for '{}': {}",
                                        event.topic, finding.message
                                    ),
                                ));
                            }
                            accept_event!(event, &payload);
                        }
                        _ => {}
                    }
                    continue;
                }
            }

            // Same-batch completion guard: events after a completion topic in the
            // same batch are subject to completion_after_terminal filtering.
            if completion_seen_in_batch
                && let Some(policy_config) = policy_config_ref
                && policy_config.enabled
                && let Some(decision) = check_completion_guard(&event.topic, policy_config, true)
            {
                match &decision {
                    PolicyDecision::Block(finding) => {
                        if write_diagnostic {
                            self.bus.publish(Event::new(
                                "event.completion.blocked",
                                format!(
                                    "Same-batch completion guard blocked '{}': {}",
                                    event.topic, finding.message
                                ),
                            ));
                        }
                    }
                    PolicyDecision::Ignore(finding) => {
                        if write_diagnostic {
                            self.bus.publish(Event::new(
                                "event.completion.ignored",
                                format!(
                                    "Same-batch completion guard ignored '{}': {}",
                                    event.topic, finding.message
                                ),
                            ));
                        }
                    }
                    PolicyDecision::Warn(findings) => {
                        for finding in findings {
                            self.bus.publish(Event::new(
                                "event.policy_warning",
                                format!(
                                    "Same-batch completion guard warning for '{}': {}",
                                    event.topic, finding.message
                                ),
                            ));
                        }
                        accept_event!(event, &payload);
                    }
                    _ => {}
                }
                continue;
            }

            if event.topic == "build.done" {
                // P4: structured JSON evidence is the preferred path. If
                // the payload parses as a JSON object we run the strict
                // schema check first; otherwise we fall back to the
                // legacy text "tests: pass" parsing.
                let trimmed = payload.trim();
                let json_status: Option<Result<BuildStatus, String>> = if trimmed.starts_with('{') {
                    Some(parse_backpressure_json(
                        trimmed,
                        &self.config.core.workspace_root,
                    ))
                } else {
                    None
                };
                if let Some(result) = json_status {
                    match result {
                        Ok(BuildStatus::Pass) => {
                            accept_event!(event, &payload);
                        }
                        Ok(BuildStatus::Fail { reason, missing }) => {
                            warn!(
                                missing = ?missing,
                                "build.done rejected: structured backpressure failed"
                            );
                            self.diagnostics.log_orchestration(
                                self.state.iteration,
                                "jsonl",
                                crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                    reason: format!("structured build evidence failed: {reason}"),
                                },
                            );
                            accept_event!(Event::new(
                                "build.blocked",
                                crate::event_parser::build_blocked_payload(&reason),
                            ));
                        }
                        Ok(BuildStatus::Invalid { reason }) => {
                            warn!(reason = %reason, "build.done rejected: invalid JSON evidence");
                            self.diagnostics.log_orchestration(
                                self.state.iteration,
                                "jsonl",
                                crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                    reason: format!("invalid build evidence: {reason}"),
                                },
                            );
                            accept_event!(Event::new(
                                "build.blocked",
                                crate::event_parser::build_blocked_payload(&reason),
                            ));
                        }
                        Err(err) => {
                            warn!(error = %err, "build.done rejected: JSON parse error");
                            self.diagnostics.log_orchestration(
                                self.state.iteration,
                                "jsonl",
                                crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                    reason: format!("build evidence parse error: {err}"),
                                },
                            );
                            accept_event!(Event::new(
                                "build.blocked",
                                crate::event_parser::build_blocked_payload(&err),
                            ));
                        }
                    }
                } else if let Some(evidence) = EventParser::parse_backpressure_evidence(&payload) {
                    if evidence.all_passed() {
                        self.warn_on_mutation_evidence(&evidence);
                        accept_event!(event, &payload);
                    } else {
                        // Evidence present but checks failed - synthesize build.blocked
                        warn!(
                            tests = evidence.tests_passed,
                            lint = evidence.lint_passed,
                            typecheck = evidence.typecheck_passed,
                            audit = evidence.audit_passed,
                            coverage = evidence.coverage_passed,
                            complexity = evidence.complexity_score,
                            duplication = evidence.duplication_passed,
                            performance = evidence.performance_regression,
                            specs = evidence.specs_verified,
                            "build.done rejected: backpressure checks failed"
                        );

                        let complexity = evidence
                            .complexity_score
                            .map(|value| format!("{value:.2}"))
                            .unwrap_or_else(|| "missing".to_string());
                        let performance = match evidence.performance_regression {
                            Some(true) => "regression".to_string(),
                            Some(false) => "pass".to_string(),
                            None => "missing".to_string(),
                        };
                        let specs = match evidence.specs_verified {
                            Some(true) => "pass".to_string(),
                            Some(false) => "fail".to_string(),
                            None => "not reported".to_string(),
                        };

                        self.diagnostics.log_orchestration(
                            self.state.iteration,
                            "jsonl",
                            crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                reason: format!(
                                    "backpressure checks failed: tests={}, lint={}, typecheck={}, audit={}, coverage={}, complexity={}, duplication={}, performance={}, specs={}",
                                    evidence.tests_passed,
                                    evidence.lint_passed,
                                    evidence.typecheck_passed,
                                    evidence.audit_passed,
                                    evidence.coverage_passed,
                                    complexity,
                                    evidence.duplication_passed,
                                    performance,
                                    specs
                                ),
                            },
                        );

                        accept_event!(Event::new(
                            "build.blocked",
                            "Backpressure checks failed. Fix tests/lint/typecheck/audit/coverage/complexity/duplication/specs before emitting build.done.",
                        ));
                    }
                } else {
                    // No evidence found - synthesize build.blocked
                    warn!("build.done rejected: missing backpressure evidence");

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "jsonl",
                        crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                            reason: "missing backpressure evidence".to_string(),
                        },
                    );

                    accept_event!(Event::new(
                        "build.blocked",
                        "Missing backpressure evidence. Include 'tests: pass', 'lint: pass', 'typecheck: pass', 'audit: pass', 'coverage: pass', 'complexity: <score>', 'duplication: pass', 'performance: pass' (optional), 'specs: pass' (optional) in build.done payload.",
                    ));
                }
            } else if event.topic == "review.done" && !event.is_wave_event() {
                // Validate review.done events have verification evidence.
                // Wave worker events skip this — wave reviews are read-only
                // and don't run tests/builds.
                let trimmed = payload.trim();
                let json_status: Option<Result<ReviewStatus, String>> = if trimmed.starts_with('{')
                {
                    Some(parse_review_json(trimmed, &self.config.core.workspace_root))
                } else {
                    None
                };
                if let Some(result) = json_status {
                    match result {
                        Ok(ReviewStatus::Pass) => {
                            accept_event!(event, &payload);
                        }
                        Ok(ReviewStatus::Fail { reason, .. }) => {
                            warn!(reason = %reason, "review.done rejected: structured verification failed");
                            self.diagnostics.log_orchestration(
                                self.state.iteration,
                                "jsonl",
                                crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                    reason: format!("structured review evidence failed: {reason}"),
                                },
                            );
                            accept_event!(Event::new(
                                "review.blocked",
                                crate::event_parser::review_blocked_payload(&reason),
                            ));
                        }
                        Err(err) => {
                            warn!(error = %err, "review.done rejected: JSON parse error");
                            self.diagnostics.log_orchestration(
                                self.state.iteration,
                                "jsonl",
                                crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                    reason: format!("review evidence parse error: {err}"),
                                },
                            );
                            accept_event!(Event::new(
                                "review.blocked",
                                crate::event_parser::review_blocked_payload(&err),
                            ));
                        }
                    }
                } else if let Some(evidence) = EventParser::parse_review_evidence(&payload) {
                    if evidence.is_verified() {
                        accept_event!(event, &payload);
                    } else {
                        // Evidence present but checks failed - synthesize review.blocked
                        warn!(
                            tests = evidence.tests_passed,
                            build = evidence.build_passed,
                            "review.done rejected: verification checks failed"
                        );

                        self.diagnostics.log_orchestration(
                            self.state.iteration,
                            "jsonl",
                            crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                reason: format!(
                                    "review verification failed: tests={}, build={}",
                                    evidence.tests_passed, evidence.build_passed
                                ),
                            },
                        );

                        accept_event!(Event::new(
                            "review.blocked",
                            "Review verification failed. Run tests and build before emitting review.done.",
                        ));
                    }
                } else {
                    // No evidence found - synthesize review.blocked
                    warn!("review.done rejected: missing verification evidence");

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "jsonl",
                        crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                            reason: "missing review verification evidence".to_string(),
                        },
                    );

                    accept_event!(Event::new(
                        "review.blocked",
                        "Missing verification evidence. Include 'tests: pass' and 'build: pass' in review.done payload.",
                    ));
                }
            } else if event.topic == "verify.passed" {
                if let Some(report) = EventParser::parse_quality_report(&payload) {
                    if report.meets_thresholds() {
                        accept_event!(event, &payload);
                    } else {
                        let failed = report.failed_dimensions();
                        let reason = if failed.is_empty() {
                            "quality thresholds failed".to_string()
                        } else {
                            format!("quality thresholds failed: {}", failed.join(", "))
                        };

                        warn!(
                            failed_dimensions = ?failed,
                            "verify.passed rejected: quality thresholds failed"
                        );

                        self.diagnostics.log_orchestration(
                            self.state.iteration,
                            "jsonl",
                            crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                reason,
                            },
                        );

                        accept_event!(Event::new(
                            "verify.failed",
                            "Quality thresholds failed. Include quality.tests, quality.coverage, quality.lint, quality.audit, quality.mutation, quality.complexity with thresholds in verify.passed payload.",
                        ));
                    }
                } else {
                    // No quality report found - synthesize verify.failed
                    warn!("verify.passed rejected: missing quality report");

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "jsonl",
                        crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                            reason: "missing quality report".to_string(),
                        },
                    );

                    accept_event!(Event::new(
                        "verify.failed",
                        "Missing quality report. Include quality.tests, quality.coverage, quality.lint, quality.audit, quality.mutation, quality.complexity in verify.passed payload.",
                    ));
                }
            } else if event.topic == "verify.failed" {
                if EventParser::parse_quality_report(&payload).is_none() {
                    warn!("verify.failed missing quality report");
                }
                accept_event!(&event, &payload);
            } else {
                // Non-backpressure events pass through unchanged
                accept_event!(&event, &payload);
            }
        }

        // Track build.blocked events for thrashing detection
        let blocked_events: Vec<_> = validated_events
            .iter()
            .filter(|e| e.topic == "build.blocked".into())
            .collect();

        for blocked_event in &blocked_events {
            let task_id = Self::extract_task_id(&blocked_event.payload);

            let count = self
                .state
                .task_block_counts
                .entry(task_id.clone())
                .or_insert(0);
            *count += 1;

            debug!(
                task_id = %task_id,
                block_count = *count,
                "Task blocked"
            );

            // After 3 blocks on same task, emit build.task.abandoned
            if *count >= 3 && !self.state.abandoned_tasks.contains(&task_id) {
                warn!(
                    task_id = %task_id,
                    "Task abandoned after 3 consecutive blocks"
                );

                self.state.abandoned_tasks.push(task_id.clone());

                self.diagnostics.log_orchestration(
                    self.state.iteration,
                    "jsonl",
                    crate::diagnostics::OrchestrationEvent::TaskAbandoned {
                        reason: format!(
                            "3 consecutive build.blocked events for task '{}'",
                            task_id
                        ),
                    },
                );

                let abandoned_event = Event::new(
                    "build.task.abandoned",
                    format!(
                        "Task '{}' abandoned after 3 consecutive build.blocked events",
                        task_id
                    ),
                );

                self.bus.publish(abandoned_event);
            }
        }

        // Track hat-level blocking for legacy thrashing detection
        let has_blocked_event = !blocked_events.is_empty();

        if has_blocked_event {
            self.state.consecutive_blocked += 1;
        } else {
            self.state.consecutive_blocked = 0;
            self.state.last_blocked_hat = None;
        }

        // Track whether any events will be published (before the loop consumes them).
        let had_events = !validated_events.is_empty();
        let had_plan_events = validated_events
            .iter()
            .any(|event| event.topic.as_str().starts_with("plan."));
        // Record and diagnose validated events (before consuming them).
        let verdict_topics = self.verdict_gate_topics();
        let verdict_topics_slice = verdict_topics.as_deref();
        // U2: collect predecessor deltas for post-loop ledger commit.
        let mut completion_predecessor_deltas: Vec<crate::state::CommitDelta> = Vec::new();
        for event in &validated_events {
            // 2026-06-30-001 P0-3 (primary-20260630-032648
            // diagnosis): the runtime rejects
            // `review.start` emits that arrive after the
            // fix-unit chain is exhausted. The pre-fix
            // behaviour let coordinator emit
            // `review.start` a second time after the
            // `fix-NN` chain was complete, triggering an
            // unwanted second review walk that confused
            // the progress-steward state machine and
            // pushed the loop off the normal
            // `plan.complete → shipper → reporter →
            // LOOP_COMPLETE` ladder. The fix is
            // structural: the runtime enforces "no
            // `review.start` after every fix-NN is
            // closed", regardless of what the agent's
            // prompt says. The pre-fix prompt comment is
            // still kept (defence in depth), but it is
            // no longer the sole guard.
            //
            // Detection: when the admitted event is a
            // `work.done` whose `task_key` is a fix-unit
            // shape, we re-check the task store. If every
            // fix-NN step in the current plan is now
            // closed, flip `fix_unit_chain_exhausted` to
            // `true`. The next admit loop iteration that
            // sees a `review.start` while the flag is
            // `true` rejects it before it lands in
            // `accepted_log_events`.
            if event.topic.as_str() == "work.done" && self.is_fix_unit_completion_event(event) {
                self.state.seen_fix_unit_completions =
                    self.state.seen_fix_unit_completions.saturating_add(1);
                if self.is_fix_unit_chain_exhausted() {
                    self.state.fix_unit_chain_exhausted = true;
                }
            }
            if event.topic.as_str() == "review.start"
                && (self.state.fix_unit_chain_exhausted
                    || self.state.seen_fix_unit_completions >= 2)
            {
                tracing::warn!(
                    iteration = self.state.iteration,
                    "P0-3: rejected review.start after fix-unit chain exhausted; \
                     coordinator must emit plan.complete, NOT a second review walk"
                );
                // Drop the event from the accepted stream;
                // the runtime continues to wait for
                // `plan.complete`.
                continue;
            }
            if event.topic.as_str() == "REVIEW_COMPLETE"
                && self.phase_authority_rejects_shipper_emit(event)
            {
                tracing::warn!(
                    iteration = self.state.iteration,
                    topic = %event.topic,
                    "phase authority: shipper routing denied REVIEW_COMPLETE"
                );
                continue;
            }

            // path_required_events: reject anchor topics until every
            // require topic has been observed on this loop lifetime.
            if let Some(missing) = self.path_required_missing_for_anchor(event.topic.as_str()) {
                tracing::warn!(
                    iteration = self.state.iteration,
                    topic = %event.topic,
                    missing = ?missing,
                    "Rejected anchor event: path_required_events require topics not yet observed"
                );
                continue;
            }

            let gate_key = (
                event.topic.as_str().to_string(),
                event.payload.as_str().to_string(),
            );
            if matches!(
                gate_outcomes.get(&gate_key),
                Some(crate::event_loop::emit_gate::EmitGateOutcome::Reject(reject))
                    if reject.reason_code == "phase_violation"
            ) {
                continue;
            }
            if matches!(
                gate_outcomes.get(&gate_key),
                Some(crate::event_loop::emit_gate::EmitGateOutcome::AcceptRepairStream)
            ) {
                continue;
            }

            // Record topic for event chain validation
            self.state.record_event(event);
            self.mark_required_event_seen(event.topic.as_str());
            self.state
                .record_verdict_if_match(event, verdict_topics_slice);
            self.state.record_completion_predecessor_if_match(
                event,
                self.config.event_loop.completion_payload_match.as_ref(),
            );
            // U2: collect predecessor delta for post-loop ledger commit.
            if let Some(cfg) = self.config.event_loop.completion_payload_match.as_ref()
                && event.topic.as_str() == cfg.topic
            {
                completion_predecessor_deltas.push(
                    crate::state::CommitDelta::CompletionPredecessorRecorded {
                        topic: event.topic.to_string(),
                        payload: event.payload.to_string(),
                    },
                );
            }

            // U3: Update hat lifecycle tracker for accepted events.
            // Find the source hat for this event and update the tracker.
            // Terminal events call complete(); non-terminal call observe_accepted_event().
            //
            // P0 code-review finding #1: the key was previously (loop_id, iteration,
            // hat_id, trigger_identity) with trigger_identity reverse-derived via
            // `can_publish` on `last_activation_events`. Because trigger events are
            // hat inputs (not publishes), the reverse lookup always returned the
            // fallback ("unknown" on activate, topic_str on complete), so the keys
            // never matched and `complete` hit the `None` branch — every
            // activation leaked. The key is now the (loop_id, iteration, hat_id)
            // triple; trigger identity is a snapshot-only display field.
            let source_hat_id = event
                .source
                .as_ref()
                .or(self.state.last_active_hat_ids.first())
                .cloned();
            if let Some(source_hat_id) = source_hat_id {
                let hat_config = self.registry.get_config(&source_hat_id);
                let topic_str = event.topic.as_str();
                let is_terminal = hat_config
                    .is_some_and(|config| config.terminal_topic_set().contains(topic_str));
                let key = ActivationKey {
                    loop_id: self
                        .loop_context
                        .as_ref()
                        .and_then(|ctx| ctx.loop_id())
                        .unwrap_or("primary")
                        .to_string(),
                    iteration: self.state.iteration,
                    hat_id: source_hat_id.as_str().to_string(),
                };
                if is_terminal {
                    self.hat_lifecycle_tracker.complete(&key, topic_str);
                } else {
                    self.hat_lifecycle_tracker.observe_accepted_event(&key);
                }
                // WRC-U4 (2026-06-12-003): clear any pending handoff
                // deadlines for this consumer hat. The accept-time
                // deadline for the triggering handoff is irrelevant
                // once the hat has activated; the `on_hat_activated`
                // call also clears siblings (e.g. a `fix.plan.ready`
                // handoff queued behind the same `executor`).
                // `on_hat_activated` returns the number of cleared
                // entries which is informational here; we do not
                // surface it because the only consumer (the
                // diagnostic reporter) reads the pending count via
                // `pending_count()` at stall-check time.
                // 2026-06-13-004 P0 #5 review fix (F2 ralph
                // guard symmetry): mirror the build_prompt
                // guard. The "ralph" hat is the constant
                // coordinator sentinel, never a handoff
                // consumer — passing it through here would
                // spuriously clear real consumer pending
                // entries whose hat_id happens to match (or
                // be a prefix of) "ralph". Round 2 added
                // this guard at L2853 (build_prompt); this
                // closes the asymmetry at the process_output
                // handoff-clear site.
                if source_hat_id.as_str() != "ralph" {
                    self.state
                        .handoff_tracker
                        .on_hat_activated(source_hat_id.as_str());
                }
                // 2026-06-14-004 U2: when a hat successfully publishes a
                // legal event, clear its rejection retry counts so a prior
                // scope violation does not cause a premature fuse on a
                // later, unrelated violation.
                self.state
                    .clear_rejection_keys_for_hat(source_hat_id.as_str());
            }

            self.diagnostics.log_orchestration(
                self.state.iteration,
                "jsonl",
                crate::diagnostics::OrchestrationEvent::EventPublished {
                    topic: event.topic.to_string(),
                },
            );

            // Check for orphaned events: no specific hat (non-fallback-only) subscribes.
            // The builtin "ralph" fallback hat with `*` subscription is excluded so that
            // events only matching the universal fallback are still marked as orphans.
            if !self.registry.has_specific_subscriber(event.topic.as_str()) {
                has_orphans = true;
            }

            debug!(
                topic = %event.topic,
                "Publishing event from JSONL"
            );
        }

        // Apply event projections before publishing.
        for event in &validated_events {
            if let Some(ref projection_config) = self.config.core.event_projection
                && projection_config.enabled
            {
                crate::event_projection::apply_projection(
                    event,
                    &projection_config.rules,
                    &self.config.core.workspace_root,
                );
            }
        }

        // Publish validated events to the bus.
        // Ralph is always registered with subscribe("*"), so every event has at least
        // one subscriber. Events without a specific hat subscriber are "orphaned" —
        // Ralph handles them as the universal fallback.
        //
        // U3 (2026-06-27-002 plan completion): route each
        // validated event through the emit-gate facade one
        // more time before publishing to the bus. Events that
        // the gate rejects are still recorded in the
        // lifecycle tracker (so terminal events close
        // activations), but they do NOT reach `self.bus`.
        // The `take_pending` is required because
        // `apply_emit_gate_on_validated` borrows `&mut self`
        // while the iterator borrows `validated_events`.
        //
        // P0-1: we look up the stashed outcome from the
        // first gate pass (keyed by `(topic, payload)`)
        // so the stage pipeline — and especially the
        // `RepairStateMachine.try_transition` call inside
        // `RepairDispatchStage` — runs exactly once per
        // event. The synthesised events (e.g.
        // `build.blocked`) inherit the source event's
        // payload verbatim, so the lookup hits the
        // same key.
        let pending_publish: Vec<Event> = {
            let mut pending = Vec::new();
            for event in &validated_events {
                let payload = event.payload.as_str().to_string();
                let key = (event.topic.as_str().to_string(), payload.clone());
                let stashed = gate_outcomes.get(&key).cloned();
                let accepted = self.apply_emit_gate_on_validated(event, stashed);
                if accepted {
                    pending.push(event.clone());
                }
            }
            pending
        };

        // Plan GAP-02 / Unit 2: apply the StateMachine
        // candidate decisions to the live runtime only at the
        // final pending_publish boundary. Events that survived
        // the candidate stage but were dropped by downstream
        // gates never see their decisions applied — so a
        // downstream reject cannot pollute live
        // `state_machine_runtime_state`. Unit 3 binds the
        // projection list to the durable outbox receipt at the
        // AcceptedTransition call below.
        //
        // Plan GAP-02 / Unit 3 (U3-finish): collect the projected
        // deltas into a (topic, payload) lookup so the per-event
        // publish loop below can forward each event's projection
        // into the projection-aware `AcceptedTransition` helper.
        // The map only contains projections for events that
        // actually passed every downstream gate — disabled-path
        // / no-candidate batches produce an empty map and the
        // existing non-projection `publish_synthetic` path is
        // taken, preserving all U6/U7/U8 contracts.
        let mut projection_lookup: std::collections::HashMap<
            (String, String),
            crate::state_machine::StateMachineTransitionDelta,
        > = std::collections::HashMap::new();
        if !self.pending_state_machine_candidates.is_empty() {
            let loop_id = self.current_loop_id_for_contract();
            let survivor_events: Vec<_> = self
                .pending_state_machine_candidates
                .iter()
                .filter_map(|cand| {
                    if pending_publish.iter().any(|e| {
                        e.topic.as_str() == cand.event.topic.as_str()
                            && e.payload.as_str() == cand.event.payload.as_deref().unwrap_or("")
                    }) {
                        Some(cand.event.clone())
                    } else {
                        None
                    }
                })
                .collect();
            self.pending_state_machine_candidates.clear();
            // U1 fix: final survivors are re-validated against the LIVE runtime
            // snapshot (not the cumulative candidate clone) so a downstream-rejected
            // predecessor cannot influence a later survivor's decision.
            let revalidated = self.revalidate_state_machine_candidates_in_order(&survivor_events);
            let projected = self.apply_state_machine_decisions(&revalidated, &loop_id);
            for (delta, cand) in projected.iter().zip(revalidated.iter()) {
                let key = (
                    cand.event.topic.as_str().to_string(),
                    cand.event.payload.as_deref().unwrap_or("").to_string(),
                );
                projection_lookup.insert(key, delta.clone());
            }
        }

        // U7/U8: pre-compute idempotent-transition context once per
        // batch so the per-event loop only needs field-level borrows.
        let u7_contract_digest = self
            .execution_contract
            .as_ref()
            .map(|c| c.contract_digest.clone());
        let u7_loop_id = self.current_loop_id_for_contract();
        let u7_iteration = self.state.iteration;

        for event in pending_publish {
            // U8 (plan 2026-07-30-004): route by typed disposition.
            // Business / Recovery events go through the idempotent
            // Accepted Transition API (durable outbox + publish) when
            // the execution contract is compiled; DiagnosticObservation
            // / LoopControl events use the explicit direct channel and
            // never advance phase authority. Without a compiled
            // contract (legacy / test paths) every event falls back to
            // a direct publish.
            let u8_disposition = crate::event_loop::disposition::classify(event.topic.as_str());

            if u8_disposition.advances_flow() && u7_contract_digest.is_some() {
                let digest = u7_contract_digest.as_deref().expect("checked above");
                let activation_id = format!(
                    "{}:{u7_iteration}",
                    event
                        .source
                        .as_ref()
                        .map(|hat| hat.as_str())
                        .unwrap_or("unknown")
                );
                // Plan GAP-02 / Unit 3 (U3-finish): look up the
                // projection emitted by `apply_state_machine_decisions`
                // and forward it into the projection-aware
                // AcceptedTransition helper. Disabled path / no
                // candidate → `None` and the helper falls through to
                // the legacy `commit_idempotent` path (U6/U7/U8
                // contract preserved byte-for-byte).
                let projection_key = (
                    event.topic.as_str().to_string(),
                    event.payload.as_str().to_string(),
                );
                let projection = projection_lookup.remove(&projection_key);

                // `&mut StateLedger` is required for the
                // projection-aware helper; the non-projection
                // branch auto-reborrows to `&StateLedger` inside
                // `commit_idempotent`.
                // Plan GAP-02 / Unit 3: commit the projection through the
                // EventLoop helper that wires the pre-apply live-runtime
                // snapshot into the rollback closure. On
                // `StateLedger::commit` failure the live runtime is
                // restored to the pre-apply snapshot; on success the
                // snapshot is dropped. The helper accepts an
                // `Option<Delta>` so it can also serve the no-projection
                // legacy path (disabled state machine / no candidate).
                self.commit_state_machine_projection(
                    &event,
                    u8_disposition,
                    &u7_loop_id,
                    &activation_id,
                    digest,
                    projection,
                )
                .map_err(|error| {
                    std::io::Error::other(format!(
                        "accepted transition commit failed for topic '{}': {error}",
                        event.topic
                    ))
                })?;
            } else {
                // Legacy loops without a compiled contract retain their
                // existing direct channel. Diagnostic/loop-control events also
                // use this explicit non-outbox route and never advance flow.
                self.bus.publish(event.clone());
            }
            self.diagnose_plan_complete_channel(
                &event,
                crate::event_loop::phase_authority::diagnosis::Channel::Main,
            );
            // U8: only Business / Recovery dispositions advance flow.
            // Diagnostic / loop-control events are observations about
            // the loop, not transitions of business state, so phase
            // authority MUST NOT run for them.
            if u8_disposition.advances_flow() {
                self.apply_phase_authority_on_accepted(&event);
            }
        }

        // Register handoff deadlines only after every accepted transition has
        // been durably committed and delivery-checked. This prevents a
        // silently undelivered event from creating a false 600s timeout.
        self.apply_contract_committed_side_effects(&committed_events);

        // --- U3: Invariant assertion checks ---
        if self.config.core.invariant_assertions {
            let control_prefixes = ["event.", "human."];
            let control_exact = [
                "LOOP_COMPLETE",
                "REVIEW_COMPLETE",
                "loop.cancel",
                "task.resume",
                "build.task.abandoned",
                "event.isolation.boundary_violation",
            ];

            for event in &accepted_log_events {
                let topic = event.topic.as_str();
                let is_control = control_exact.contains(&topic)
                    || control_prefixes.iter().any(|p| topic.starts_with(p));

                // INV-1: Ralph must not publish business topics
                if !is_control && event.source.as_ref().map(|h| h.as_str()) == Some("ralph") {
                    self.state.invariant_violation_count += 1;
                    self.state.last_invariant_violation =
                        Some(format!("INV-1:hat=ralph,topic={}", topic));

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "ralph",
                        crate::diagnostics::OrchestrationEvent::InvariantViolation {
                            rule_id: "INV-1".to_string(),
                            description: format!("Ralph published business topic '{}'", topic),
                            topic: Some(topic.to_string()),
                            source: Some("ralph".to_string()),
                            iteration: self.state.iteration,
                        },
                    );

                    warn!(
                        topic = %topic,
                        invariant = "INV-1",
                        "Invariant violation: Ralph published business topic"
                    );
                }
            }
        }
        // --- End invariant checks ---

        // 2026-06-16-001 U5: stall detection and progress-steward
        // wake. The counter is updated after all validation layers
        // have run so it reflects the *post-validation* state
        // (a turn that only produced rejections is a
        // no-progress turn, not a turn that advanced).
        // 2026-07-30-002 plan U1 (R1/D4): route through the
        // wrapper so the fail-close emit also advances the
        // flow step + appends the snapshot.
        self.run_stall_detector_with_authority_advance()?;
        // --- End U5 stall detection ---

        // 2026-07-01-001 plan U6: capture the most recent
        // `test.passed` step into the orchestrator-state cache
        // so the next coordinator prompt can render a
        // directive. We scan `accepted_log_events` (the
        // post-validation stream) so a rejected test.passed
        // is intentionally ignored — the engine only feeds
        // the directive for admitted passes.
        for event in &accepted_log_events {
            if event.topic.as_str() == "test.passed"
                && let Some(step) = extract_step_id(&event.payload)
            {
                let was_fix = step.starts_with("fix-");
                self.state.record_test_passed(step, was_fix);
            }
            if event.topic.as_str() == "test.failed"
                && let Some(step) = extract_step_id(&event.payload)
            {
                self.state.record_validator_terminal(step, "failed");
            }
            if event.topic.as_str() == "plan.complete"
                && let Some(step) = extract_step_id(&event.payload)
            {
                self.state.last_plan_complete_step = Some(step);
            }
        }

        // 2026-07-01-001 plan U6 wiring was removed: plan
        // topology scanning is no longer a base concern. The
        // coordinator hat now derives plan structure from the
        // plan file via prompt context instead of engine-side
        // regex parsing.

        // U2: commit collected predecessor deltas now that
        // `state_ledger` is restored.
        if let Some(ref mut ledger) = self.state.state_ledger {
            for delta in completion_predecessor_deltas {
                if let Err(e) =
                    ledger.commit(delta, Some("loop.completion_predecessor".to_string()))
                {
                    tracing::warn!(
                        error = %e,
                        "U2: completion predecessor commit failed; loop continues"
                    );
                }
            }
        }

        // A1 (002-adversarial-review / 003-adversarial-review
        // P0-1): when the unified ledger is wired in, mirror
        // the per-batch counters into the commit log so the
        // `StateLedger` actually participates in the production
        // event loop. P1-2 (P1 follow-up): terminal markers
        // (`CompletionRequested` / `CompletionHonored` /
        // `CancellationRequested`) are committed per-event at
        // the decision point (see `commit_terminal_delta`) so
        // a mid-flight crash preserves the termination signal.
        // This hook keeps the per-iteration `CounterChanged`
        // and the loop-`StewardWoken` scalars that don't need
        // per-event latency.
        if let Some(ref mut ledger) = self.state.state_ledger {
            use crate::state::{CommitDelta, CounterKind};
            // 2026-06-23 fix plan U7 (CB-5): only advance the
            // iter counter when this iteration actually accepted
            // at least one event. A no-progress turn (all
            // rejected) must NOT bump the iter counter — that
            // would create a divergent ledger where iter N points at
            // `events.jsonl` lines from a different iteration.
            //
            // The `loop.batch_sync` source tag distinguishes the
            // happy path from the no-progress path so operators
            // inspecting `ledger.jsonl` can see when the loop
            // chose not to advance.
            //
            // 2026-06-30-001 P0-6 (primary-20260630-032648
            // diagnosis): the pre-fix code emitted
            // `loop.batch_sync.no_progress` for no-progress
            // turns, which produced two diverging iter
            // sequences in the ledger — `loop.batch_sync`
            // and `loop.batch_sync.no_progress` were
            // committed with independent `seq` numbers, and
            // `summary.md` ended up showing 41 iter while
            // the no-progress sub-stream was at 28. We now
            // commit a single `loop.batch_sync` entry per
            // turn and carry the no-progress signal in the
            // `delta.kind` (via `kind: "no_progress"`),
            // keeping the iter sequence monotonic.
            let batch_sync_source = "loop.batch_sync";
            let iter_counter = CommitDelta::CounterChanged {
                counter: CounterKind::Iteration,
                new_value: i64::from(self.state.iteration),
            };
            // 2026-06-30-001 P1-4: when the turn is a
            // no-progress turn, ALSO commit a
            // `NoProgressTurnObserved` delta so the
            // no-progress dimension is preserved on disk
            // even though we now use a single
            // `loop.batch_sync` source string. Operators
            // can still query "no-progress turns" via
            // `grep kind no_progress_turn_observed
            // .ralph/ledger.jsonl`. The source string on
            // this companion entry is the same
            // `loop.batch_sync` so any source-string
            // filter keeps working unchanged.
            let is_no_progress_turn = !had_events && accepted_log_events.is_empty();
            if is_no_progress_turn {
                let no_progress = CommitDelta::NoProgressTurnObserved {
                    iteration: self.state.iteration,
                };
                if let Err(e) = ledger.commit(no_progress, Some(batch_sync_source.to_string())) {
                    tracing::warn!(
                        error = %e,
                        iteration = self.state.iteration,
                        source = %batch_sync_source,
                        "P1-4: no-progress companion commit failed; loop continues"
                    );
                }
            }
            if let Err(e) = ledger.commit(iter_counter, Some(batch_sync_source.to_string())) {
                tracing::warn!(
                    error = %e,
                    iteration = self.state.iteration,
                    source = %batch_sync_source,
                    "A1: end-of-batch ledger commit failed; loop continues"
                );
            }
            // Terminal marker commits moved to per-event
            // decision points (see `commit_terminal_delta`).
        }

        self.commit_knowledge_observations(&accepted_log_events);

        // U12 wiring (P0-1, 2026-06-27 review): refresh the
        // step-close progress registry after every parsed
        // batch so the next emit is checked against the
        // latest `done`/`total`. Idempotent and a no-op
        // when the step did not opt into `total_units`.
        self.drive_step_close_progress();

        // 2026-06-29-007 plan U1b: advance `current_step`
        // when unit_loop `total_units` reached. Runs after
        // `drive_step_close_progress` so the step-close
        // counter is up to date.
        self.drive_step_transition();

        // 2026-07-02-004 plan U5/U6 wiring: enforce the
        // synthesized precheck gate hat hard-gate and
        // dispatch rejections (resume vs. exhaustion).
        // Runs after `drive_step_transition` so the
        // step-close stage fires first when both apply.
        self.drive_precheck_gate_obligation(&accepted_log_events);

        // 2026-07-28-001 plan U3: stage the over-emit
        // recovery intent and resolve it AFTER we know
        // whether the turn committed a business event. The
        // recovery is stored in `state.pending_over_emit_recovery`
        // by the drop branch above and settled here so a
        // legitimate handoff emitted in the same
        // activation can never be pre-empted by an extra
        // event's `task.resume` injection.
        self.resolve_over_emit_recovery(&accepted_log_events);

        if !accepted_log_events.is_empty() {
            self.diagnostics.log_runtime_trace(
                crate::diagnostics::RuntimeTraceEntry::new(
                    self.state.iteration as u64,
                    0,
                    crate::diagnostics::RuntimeTracePhase::Accepted,
                )
                .with_kind("event_batch_accepted")
                .with_status("accepted")
                .with_fields(serde_json::json!({
                    "count": accepted_log_events.len(),
                    "topics": accepted_log_events
                        .iter()
                        .map(|event| event.topic.to_string())
                        .collect::<Vec<_>>(),
                })),
            );
        }
        if had_rejected_events {
            self.diagnostics.log_runtime_trace(
                crate::diagnostics::RuntimeTraceEntry::new(
                    self.state.iteration as u64,
                    0,
                    crate::diagnostics::RuntimeTracePhase::Rejected,
                )
                .with_kind("event_batch_rejected")
                .with_status("rejected")
                .with_fields(serde_json::json!({
                    "accepted_count": accepted_log_events.len(),
                    "contract_rejection_count": contract_rejections.len(),
                })),
            );
        }
        self.diagnostics.log_runtime_trace(
            crate::diagnostics::RuntimeTraceEntry::new(
                self.state.iteration as u64,
                0,
                crate::diagnostics::RuntimeTracePhase::Commit,
            )
            .with_kind("event_batch_commit")
            .with_status(if accepted_log_events.is_empty() {
                "no_progress"
            } else {
                "committed"
            })
            .with_fields(serde_json::json!({
                "accepted_count": accepted_log_events.len(),
                "rejected": had_rejected_events,
            })),
        );

        Ok(ProcessedEvents {
            had_events,
            had_raw_events,
            had_rejected_events,
            had_plan_events,
            has_orphans,
            accepted_events: accepted_log_events,
            contract_rejections,
            payload_contract_violation,
        })
    }
}
