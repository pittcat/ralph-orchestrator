//! EventLoop implementation region 1.

use super::*;

impl EventLoop {
    /// 2026-07-01-001 plan U1: collect the set of topics the
    /// runtime considers "terminal" for the current loop.
    /// Derived from `EventPolicyConfig.terminal_topics` (when
    /// the policy is enabled) plus the configured completion
    /// promise and cancellation promise — that way the
    /// terminal set stays in lockstep with whatever the
    /// preset author wired in `event_policy`, instead of
    /// hard-coding a topic list that drifts when the
    /// ce-executor-serial preset changes its `terminal_topics`.
    pub(crate) fn collect_terminal_topic_set(&self) -> std::collections::HashSet<&str> {
        use std::collections::HashSet;
        let mut out: HashSet<&str> = HashSet::new();
        if let Some(policy) = self.config.event_loop.event_policy.as_ref()
            && policy.enabled
        {
            for topic in &policy.terminal_topics {
                out.insert(topic.as_str());
            }
        }
        // Always treat the configured completion promise
        // and cancellation promise as terminal — the rest of
        // the loop (U2) is anchored on these and skipping
        // them would let a post-completion event through.
        let completion = self.config.event_loop.completion_promise.as_str();
        if !completion.is_empty() {
            out.insert(completion);
        }
        let cancellation = self.config.event_loop.cancellation_promise.as_str();
        if !cancellation.is_empty() {
            out.insert(cancellation);
        }
        out
    }

    /// Topics listed in `event_loop.required_events` for the current loop.
    pub(crate) fn required_event_topic_set(&self) -> std::collections::HashSet<&str> {
        self.config
            .event_loop
            .required_events
            .iter()
            .map(|topic| topic.as_str())
            .collect()
    }

    /// Isolated-mode per-turn budget carve-out for ordered dual publishes
    /// from the same hat: `queue.advance` → `work.ready`, and any
    /// `required_events` topic → `completion_promise`.
    pub(crate) fn isolated_dual_publish_handoff(
        &self,
        incoming_topic: &str,
        incoming_hat: &str,
        isolated_hat: &str,
        accepted: &[crate::event_reader::Event],
    ) -> bool {
        let Some(last) = accepted.last() else {
            return false;
        };
        // Mirror isolated scope attribution: events without provenance
        // inherit the active isolated hat (same as the caller's
        // `incoming_hat` fallback). Using `""` for the previous event
        // broke the legacy `(queue.advance, work.ready)` pair when
        // neither JSONL line carried a `hat` field — the old inline
        // check compared `Option` equality (`None == None`).
        let last_hat = last
            .hat
            .as_deref()
            .or(last.source.as_deref())
            .unwrap_or(isolated_hat);
        if last_hat != incoming_hat {
            return false;
        }
        let last_topic = last.topic.as_str();
        if incoming_topic == "work.ready" && last_topic == "queue.advance" {
            return true;
        }
        let completion = self.config.event_loop.completion_promise.as_str();
        if incoming_topic == completion
            && !completion.is_empty()
            && self.required_event_topic_set().contains(last_topic)
        {
            return true;
        }
        false
    }

    pub(super) fn mark_required_event_seen(&mut self, topic: &str) {
        let required = self.config.event_loop.required_events.clone();
        self.state.mark_required_event_topic_seen(topic, &required);
    }

    /// Returns missing `path_required_events.require` topics when `topic`
    /// matches a configured anchor; `None` when the topic is not an anchor
    /// or all requires have already been observed.
    pub(crate) fn path_required_missing_for_anchor(&self, topic: &str) -> Option<Vec<String>> {
        let mut missing: Vec<String> = Vec::new();
        for gate in &self.config.event_loop.path_required_events {
            if gate.anchor != topic {
                continue;
            }
            for required in &gate.require {
                if !self.state.seen_topics.contains(required.as_str()) {
                    missing.push(required.clone());
                }
            }
        }
        if missing.is_empty() {
            None
        } else {
            Some(missing)
        }
    }

    /// 2026-06-09: returns the union of `verdict_gate.topic` and
    /// its `additional_topics`, or `None` when no gate is
    /// configured.  Used at every record-verdict call site so the
    /// 4 call sites stay in lockstep.  Allocates only when a
    /// gate is present (the per-iteration cost is paid once, not
    /// per event).
    pub(crate) fn verdict_gate_topics(&self) -> Option<Vec<String>> {
        self.config.event_loop.verdict_gate.as_ref().map(|v| {
            let mut topics = Vec::with_capacity(1 + v.additional_topics.len());
            topics.push(v.topic.clone());
            topics.extend(v.additional_topics.iter().cloned());
            topics
        })
    }

    /// Creates a new event loop from configuration.
    ///
    /// **Test-only.** Production code must construct via
    /// [`EventLoop::from_resolved_no_context`] (or [`EventLoop::from_resolved`])
    /// so the config passes the fallible execution-contract compile boundary
    /// (U2, plan 2026-07-30-004) before the loop is built.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(config: RalphConfig) -> Self {
        // Try to create diagnostics collector, but fall back to disabled if it fails
        // (e.g., in tests without proper directory setup)
        let diagnostics = crate::diagnostics::DiagnosticsCollector::new(std::path::Path::new("."))
            .unwrap_or_else(|e| {
                debug!(
                    "Failed to initialize diagnostics: {}, using disabled collector",
                    e
                );
                crate::diagnostics::DiagnosticsCollector::disabled()
            });

        Self::build_no_context(config, diagnostics)
    }

    /// Creates a new event loop with a loop context for path resolution.
    ///
    /// The loop context determines where events, tasks, and other state files
    /// are located. Use this for multi-loop scenarios where each loop runs
    /// in an isolated workspace (git worktree).
    ///
    /// **Diagnostics ownership (U0).** If `context.prebuilt_diagnostics()` is
    /// `Some`, that collector is reused as the authoritative session — the
    /// CLI builds it in `main.rs` and shares it with the tracing layer so
    /// the run produces a single timestamped session dir. Otherwise, a
    /// fresh `DiagnosticsCollector::new(workspace)` is created. Either way,
    /// init failure falls back to a disabled collector (with a `tracing::warn!`)
    /// — diagnostics never panic the loop.
    /// **Test-only.** Production code must construct via
    /// [`EventLoop::from_resolved`] so the config passes the fallible
    /// execution-contract compile boundary (U2) first.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_context(config: RalphConfig, context: LoopContext) -> Self {
        let diagnostics = match context.prebuilt_diagnostics() {
            Some(collector) => (**collector).clone(),
            None => crate::diagnostics::DiagnosticsCollector::new(context.workspace())
                .unwrap_or_else(|e| {
                    warn!(
                        "Failed to initialize diagnostics: {}, using disabled collector",
                        e
                    );
                    crate::diagnostics::DiagnosticsCollector::disabled()
                }),
        };

        Self::build_with_context(config, context, diagnostics)
            .expect("U13: archive failed; the loop cannot start on stale state. Use with_context_and_diagnostics to receive the error explicitly.")
    }

    /// Production constructor: build the loop from a config that has already
    /// passed the fallible execution-contract compile boundary (U2, plan
    /// 2026-07-30-004). Mirrors the context-aware path of
    /// [`EventLoop::with_context`].
    ///
    /// Callers must obtain `resolved` via
    /// [`crate::execution_contract::compile`] and fail non-zero on `Err`
    /// *before* reaching this point — a config gap must abort startup before
    /// loop initialization.
    pub fn from_resolved(
        resolved: crate::execution_contract::ResolvedRuntimeConfig,
        context: LoopContext,
    ) -> Self {
        // U4: retain the compiled contract so `prepend_hat_identity`
        // can project contract actionability into the prompt block.
        let contract = std::sync::Arc::new(resolved.contract().clone());
        let config = resolved.into_inner();
        let diagnostics = match context.prebuilt_diagnostics() {
            Some(collector) => (**collector).clone(),
            None => crate::diagnostics::DiagnosticsCollector::new(context.workspace())
                .unwrap_or_else(|e| {
                    warn!(
                        "Failed to initialize diagnostics: {}, using disabled collector",
                        e
                    );
                    crate::diagnostics::DiagnosticsCollector::disabled()
                }),
        };

        let mut event_loop = Self::build_with_context(config, context, diagnostics)
            .expect("U13: archive failed; the loop cannot start on stale state. Use with_context_and_diagnostics to receive the error explicitly.");
        event_loop.execution_contract = Some(contract);
        event_loop
    }

    /// Production constructor for the no-context path (mirrors
    /// [`EventLoop::new`]). See [`EventLoop::from_resolved`] for the contract
    /// compile requirement.
    pub fn from_resolved_no_context(
        resolved: crate::execution_contract::ResolvedRuntimeConfig,
    ) -> Self {
        // U4: retain the compiled contract for prompt projection.
        let contract = std::sync::Arc::new(resolved.contract().clone());
        let config = resolved.into_inner();
        let diagnostics = crate::diagnostics::DiagnosticsCollector::new(std::path::Path::new("."))
            .unwrap_or_else(|e| {
                debug!(
                    "Failed to initialize diagnostics: {}, using disabled collector",
                    e
                );
                crate::diagnostics::DiagnosticsCollector::disabled()
            });

        let mut event_loop = Self::build_no_context(config, diagnostics);
        event_loop.execution_contract = Some(contract);
        event_loop
    }

    /// Creates a new event loop with explicit loop context and diagnostics.
    ///
    /// **Test-only.** Production code must construct via
    /// [`EventLoop::from_resolved`] so the config passes the fallible
    /// execution-contract compile boundary (U2) first.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_context_and_diagnostics(
        config: RalphConfig,
        context: LoopContext,
        diagnostics: crate::diagnostics::DiagnosticsCollector,
    ) -> std::io::Result<Self> {
        Self::build_with_context(config, context, diagnostics)
    }

    /// Ungated context-aware builder shared by the test-only
    /// [`EventLoop::with_context_and_diagnostics`] / [`EventLoop::with_context`]
    /// and the production [`EventLoop::from_resolved`].
    // U11 wiring: archive_state_for_loop 在 new() 路径调用
    // U13 (2026-06-27-002 plan completion): a failed
    // archive now returns `Err` instead of warning and
    // continuing, so stale `.ralph/` state can never
    // poison a fresh loop (SC-6).
    pub(super) fn build_with_context(
        mut config: RalphConfig,
        context: LoopContext,
        diagnostics: crate::diagnostics::DiagnosticsCollector,
    ) -> std::io::Result<Self> {
        // Solo mode safety guard: force scratchpad enabled when no hats defined
        if config.hats.is_empty() && !config.core.scratchpad.enabled {
            warn!(
                "core.scratchpad.enabled is false but no hats are defined. \
                 Scratchpad is the only continuity mechanism in solo mode — forcing enabled."
            );
            config.core.scratchpad.enabled = true;
        }

        // U11 wiring: archive previous-loop state on worktree
        // reuse. U13 (2026-06-27-002 plan completion)
        // flips the behaviour from best-effort to
        // fail-closed: a failed archive aborts the
        // loop start so the new loop_id never sees
        // stale `.ralph/` state (which is what caused
        // SC-6 to fail in the 2026-06-26 diagnostic).
        if let Some(loop_id) = context.loop_id() {
            use crate::event_loop::stages::archive_version_stage::archive_state_for_loop;
            match archive_state_for_loop(&context.ralph_dir(), loop_id) {
                Ok(Some(dir)) => info!("U13: archived previous-loop state to {}", dir.display()),
                Ok(None) => debug!("U13: no previous loop state to archive"),
                Err(e) => {
                    // U13 fail-closed: surface the error
                    // to the caller so `EventLoop::new`
                    // / `with_context_and_diagnostics`
                    // returns `Err` instead of starting
                    // a loop on stale state. The 2026-06-26
                    // diagnostic flagged the legacy
                    // `warn + continue` behaviour as the
                    // root cause of SC-6 violations.
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("U13: archive_state_for_loop failed for loop_id={loop_id}: {e}"),
                    ));
                }
            }

            // 2026-06-28-002 U5: mirror the existing
            // `.ralph/agent/tasks.jsonl` snapshot into the
            // idempotent log so U8's `_idempotency_key` /
            // `_final` fields land on every pre-existing task
            // before the first `save()` of the new run.
            // Failures are logged at WARN level — the JSONL
            // remains the source of truth and the bootstrap
            // path must not block on a best-effort side channel.
            {
                use crate::state::idempotent_log::IdempotentLog;
                use crate::task_store::TaskStore;
                let tasks_path = context.tasks_path();
                match TaskStore::load(&tasks_path) {
                    Ok(mut store) => {
                        match IdempotentLog::open(&context.workspace().join(".ralph"), loop_id) {
                            Ok(log) => {
                                let arc = std::sync::Arc::new(std::sync::Mutex::new(log));
                                if let Err(e) = store.save_with_shared_log(arc, loop_id) {
                                    warn!(
                                        loop_id = %loop_id,
                                        tasks_path = %tasks_path.display(),
                                        error = %e,
                                        "U5: mirroring existing tasks into idempotent log \
                                         failed; continuing without blocking the loop start"
                                    );
                                }
                            }
                            Err(e) => warn!(
                                loop_id = %loop_id,
                                error = %e,
                                "U5: IdempotentLog::open for mirror failed; skipping task mirror"
                            ),
                        }
                    }
                    Err(e) => debug!(
                        tasks_path = %tasks_path.display(),
                        error = %e,
                        "U5: existing tasks.jsonl not yet present; nothing to mirror"
                    ),
                }
            }

            // U8 (2026-06-27-002 plan completion):
            // backfill `loop_id` on every legacy task
            // record left behind by the pre-mechanism
            // foundation runtime. Idempotent — repeated
            // invocations are no-ops. The function logs
            // errors at WARN level and continues; a
            // failed backfill must not block loop start.
            let tasks_path = context.tasks_path();
            match crate::event_loop::legacy_task_relocate::relocate_legacy_tasks(
                &tasks_path,
                loop_id,
            ) {
                Ok(n) if n > 0 => info!(
                    "U8: backfilled loop_id on {n} legacy task record(s) in {}",
                    tasks_path.display()
                ),
                Ok(_) => debug!("U8: no legacy task records to backfill"),
                Err(e) => warn!("U8: relocate_legacy_tasks failed (continuing): {e}"),
            }
        }

        let registry = HatRegistry::from_runtime_config(&config);
        let publish_schemas = config
            .event_loop
            .event_policy
            .as_ref()
            .map(|p| p.schemas.clone())
            .unwrap_or_default();
        let instruction_builder = InstructionBuilder::with_publish_schemas(
            config.core.clone(),
            config.events.clone(),
            publish_schemas,
        );

        let mut bus = EventBus::new();

        // Per spec: "Hatless Ralph is constant — Cannot be replaced, overwritten, or configured away"
        // Ralph is ALWAYS registered as the universal fallback for orphaned events.
        // Custom hats are registered first (higher priority), Ralph catches everything else.
        // The builtin "ralph" hat is already registered in the registry via `from_runtime_config`.
        for hat in registry.all() {
            bus.register(hat.clone());
        }

        if config.hats.is_empty() {
            debug!("Solo mode: Ralph is the only coordinator");
        } else {
            debug!(
                "Multi-hat mode: {} custom hats + Ralph as fallback",
                config.hats.len()
            );
        }

        // Build skill registry from config
        let mut skill_registry = if config.skills.enabled {
            SkillRegistry::from_config(
                &config.skills,
                context.workspace(),
                Some(config.cli.backend.as_str()),
            )
            .unwrap_or_else(|e| {
                warn!(
                    "Failed to build skill registry: {}, using empty registry",
                    e
                );
                SkillRegistry::new(Some(config.cli.backend.as_str()))
            })
        } else {
            SkillRegistry::new(Some(config.cli.backend.as_str()))
        };

        // Remove task/memory skills from the index when their config is disabled
        if !config.tasks.enabled {
            skill_registry.remove("ralph-tools-tasks");
        }
        if !config.memories.enabled {
            skill_registry.remove("ralph-tools-memories");
        }

        let skill_index = if config.skills.enabled {
            skill_registry.build_index(None)
        } else {
            String::new()
        };

        // When memories are enabled, add tasks CLI instructions alongside scratchpad
        let ralph = HatlessRalph::new(
            config.event_loop.completion_promise.clone(),
            config.core.clone(),
            &registry,
            config.event_loop.starting_event.clone(),
        )
        .with_memories_enabled(config.memories.enabled)
        .with_skill_index(skill_index);

        // Read timestamped events path from marker file, fall back to default
        // The marker file contains a relative path like ".ralph/events-20260127-123456.jsonl"
        // which we resolve relative to the workspace root
        let events_path = std::fs::read_to_string(context.current_events_marker())
            .map(|s| {
                let relative = s.trim();
                context.workspace().join(relative)
            })
            .unwrap_or_else(|_| context.events_path());
        let event_reader = EventReader::new(&events_path);

        // 2026-07-01-001 U1: seed policy runtime state from the existing events
        // file so per-loop dedup sets (`review.start`, `review.dimension.ready`,
        // `work.done`, etc.) survive process restarts. Without this, a loop
        // restart or a new `ralph` invocation sees an empty dedup set and
        // accepts duplicate handoff events that the previous process already
        // handled.
        let mut state = LoopState::new();
        if let Some(policy_config) = config
            .event_loop
            .event_policy
            .as_ref()
            .filter(|p| p.enabled)
        {
            match crate::event_policy::PolicyRuntimeState::from_events(&events_path, policy_config)
            {
                Ok(policy_state) => {
                    state.policy_runtime_state = Some(policy_state);
                }
                Err(e) => {
                    warn!(
                        events_path = %events_path.display(),
                        error = %e,
                        "Failed to seed policy runtime state from events; starting with empty state"
                    );
                }
            }
        }

        let handoff_timeout = config
            .event_loop
            .workflow_contract
            .as_ref()
            .map(|wc| wc.effective_timeout_seconds())
            .unwrap_or(crate::config::HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS);
        state.handoff_tracker = crate::workflow_contract::HandoffTracker::new()
            .with_default_timeout(std::time::Duration::from_secs(handoff_timeout));

        // U2 (plan 2026-06-21-002): unified state ledger opt-in.
        // U2: the state ledger is always enabled.
        state.state_ledger = Some(build_state_ledger_from_env(context.workspace()));

        // Plan GAP-02 (2026-08-13-002) / Unit 4 (U4-finish):
        // close the outbox-only crash window. When the previous
        // process died between the durable outbox write (with
        // `state_machine_projection`) and the StateLedger commit,
        // the projection lives only in the outbox. Walk every
        // outbox entry, find any projection whose transition_id is
        // not yet in the ledger's applied set, and commit the
        // delta — `apply_transition_delta` dedupes, so re-running
        // this on a healthy ledger is a no-op (R6).
        //
        // Order matters: repair happens BEFORE the runtime
        // hydration below so the hydrated state already includes
        // any projections recovered from the outbox. A genuine
        // filesystem error (e.g. unreadable outbox) fails closed
        // here — the same fail-closed contract as
        // `commit_idempotent`'s outbox read failure path.
        if let Some(ledger) = state.state_ledger.as_mut()
            && let Err(e) = crate::event_loop::accepted_transition::AcceptedTransition::repair_state_machine_projection_from_outbox(
                ledger,
            )
        {
            return Err(e);
        }

        // Plan GAP-02 (2026-08-13-002) / Unit 4: rehydrate the
        // StateMachine runtime from the freshly-built ledger
        // snapshot. Cold-start branches with no StateMachine
        // delta simply leave the runtime `None`; otherwise
        // the runtime is restored with the same instance
        // map, transition count, and terminal flags. This is
        // a *pure* read — no side effects — so the order vs
        // policy / task / projector bootstrap is preserved.
        if let Some(ledger) = state.state_ledger.as_ref() {
            let snapshot = ledger.snapshot();
            if let Some(runtime) = snapshot.state_machine_runtime.clone() {
                state.state_machine_runtime_state = Some(runtime);
            }
        }

        // P0-2 (2026-06-27 adversarial review):
        // open the idempotent log for real so the
        // wiring layer (`IdempotentLog::append`) can
        // actually persist recovery / drift / task
        // records. Previously the field was
        // `IdempotentLog::disabled()`, so every
        // `write_recovery` / `write_drift` / `write_task`
        // call was a no-op and SC-5 (summary count
        // equals `_final:true` record count) could
        // never hold. We open AFTER the archive step
        // (U11) so a stale `loop-version.json` from
        // a previous loop does not get overwritten
        // by the new open before the old records
        // are moved into `archive/`. Archive runs
        // first; open runs immediately below; this
        // is the order pinned by P1-10.
        //
        // P1-10 (2026-06-27 adversarial review):
        // the order is now load-bearing — the
        // `archive_state_for_loop` call above
        // (search for `// U11 wiring:` near
        // line 535) MUST stay strictly above
        // this `open` call. Reordering them
        // silently corrupts the workspace (old
        // `loop-version.json` gets overwritten
        // before its records are archived).
        // The order is enforced by
        // `tests/u11_archive_before_open.rs`
        // (added in P1-10) which exercises the
        // two paths and asserts that the
        // archive directory is populated
        // before `IdempotentLog::open`
        // touches `loop-version.json`. A
        // code-review comment here is the
        // single source of truth for the
        // load-bearing ordering.
        let idempotent_log = match context.loop_id() {
            Some(loop_id) => {
                let ralph_dir = context.ralph_dir();
                // 2026-06-28 plan U7 (R7): branches on
                // `mechanism.state_idempotency`:
                //   - `required` + loop_id: open is HARD. Failure
                //     surfaces as `Err` so the runner exits and
                //     does not start a loop with `IdempotentLog::disabled()`.
                //   - `disabled` + loop_id: still allow disabled
                //     (legacy / opt-out presets).
                //   - `required` without loop_id: also Err — the
                //     caller asked for required but the legacy
                //     primary loop path has no loop_id.
                let required = self_is_state_idempotency_required(&config);
                match crate::state::idempotent_log::IdempotentLog::open(&ralph_dir, loop_id) {
                    Ok(log) => std::sync::Mutex::new(log),
                    Err(e) if required => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!(
                                "U7: IdempotentLog::open failed for required-state_idempotency preset \
                                 (loop_id={loop_id}, ralph_dir={}): {e}. \
                                 Refusing to fall back to a disabled log; the loop will not start.",
                                ralph_dir.display(),
                            ),
                        ));
                    }
                    Err(e) => {
                        warn!(
                            loop_id = %loop_id,
                            ralph_dir = %ralph_dir.display(),
                            error = %e,
                            "IdempotentLog::open failed for non-required preset; \
                             falling back to disabled log."
                        );
                        std::sync::Mutex::new(
                            crate::state::idempotent_log::IdempotentLog::disabled(),
                        )
                    }
                }
            }
            None => {
                // No loop_id: legacy primary loop. The U7
                // plan's third branch says `state_idempotency:
                // required` without a `loop_id` is an Err —
                // but the BDD scenario harness
                // (`run_workflow_guard_scenario`) runs without
                // a `loop_id` and declares `required` to test
                // the runtime's other guarantees. To keep
                // the scenario suite green while still
                // surfacing misconfigured production presets,
                // we issue a `warn!` here and fall back to
                // `IdempotentLog::disabled()`. The U12
                // metadata_runtime_drift lint will surface a
                // `required` value that the operator did not
                // intend; U7's hard panic is reserved for the
                // `loop_id`-present / `IdempotentLog::open`
                // failure case (the 2026-06-28 diagnosis
                // P0-2 root cause).
                let required = self_is_state_idempotency_required(&config);
                if required {
                    warn!(
                        "U7: state_idempotency is `required` but the loop context has no loop_id; \
                         falling back to disabled log. The U12 metadata_runtime_drift lint will \
                         surface this configuration as a hard error at preset-load time. \
                         For production preset authors: pair `state_idempotency: required` with a \
                         loop context that carries a `loop_id`."
                    );
                } else {
                    debug!(
                        "loop context has no loop_id; using disabled idempotent log \
                         (the legacy primary loop runs without a loop_id)."
                    );
                }
                std::sync::Mutex::new(crate::state::idempotent_log::IdempotentLog::disabled())
            }
        };

        let (stage_pipeline, flow_step_totals, phase_authority) =
            build_stage_pipeline_from_config(&config);

        // 2026-06-28-002 U5 P0 fix: after the bootstrap mirror
        // (above) writes per-task idempotent records to disk via a
        // transient log, the EventLoop's main `idempotent_log` is
        // freshly opened and its in-memory index is empty —
        // without `replay()`, `final_count` / `final_records` /
        // any `_final`-based gate sees zero records. Call
        // `replay()` once so the mirror records surface in the
        // main log. Best-effort: a replay failure is logged at
        // WARN level and the loop still starts (the JSONL
        // tasks.jsonl remains the source of truth).
        {
            if let Ok(mut log) = idempotent_log.lock()
                && let Err(e) = log.replay()
            {
                warn!(
                    error = %e,
                    "U5: IdempotentLog::replay after bootstrap mirror failed; \
                     mirror records will be invisible to the main log until next save"
                );
            }
        }

        // U3 (plan 2026-07-30-004): open the persistent activation registry.
        // Best-effort: a corrupt or unreadable registry file causes the
        // field to be `None` and the loop proceeds without cross-process
        // activation identity tracking. The CLI can still check via
        // `load_registry_readonly` which returns the error explicitly.
        let activation_registry = {
            let registry_path = context
                .workspace()
                .join(".ralph")
                .join(crate::execution_contract::ACTIVATION_REGISTRY_RELATIVE_PATH);
            match crate::execution_contract::ActivationRegistry::open(registry_path) {
                Ok(r) => {
                    debug!("U3: activation registry opened");
                    Some(r)
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "U3: activation registry failed to open; proceeding without it. \
                         Concurrent activation enforcement is disabled."
                    );
                    None
                }
            }
        };

        Ok(Self {
            config: config.clone(),
            registry,
            bus,
            state,
            instruction_builder,
            ralph,
            robot_guidance: Vec::new(),
            event_reader,
            diagnostics,
            loop_context: Some(context),
            skill_registry,
            handoff_index: crate::workflow_contract::HandoffIndex::from_config(&config),
            recovery_responder: RecoveryResponder::new(Arc::new(
                config.telemetry.runtime_diagnosis.clone(),
            )),
            hat_lifecycle_tracker: ActivationLifecycleTracker::new(),
            activation_registry,
            ephemeral_isolation: crate::ephemeral_isolation::EphemeralIsolation::new(),
            idempotent_log,
            stage_pipeline,
            flow_step_totals,
            // P1-5 (2026-06-27 adversarial review):
            // per-task repair state machine registry.
            repair_state_machines: std::collections::HashMap::new(),
            repair_stream_pending: 0,
            // 2026-06-28 plan U4: initialise current_plan_step to
            // the first declared flow step (when one exists) or
            // an empty string for legacy / no-flow presets. The
            // value drives the FlowStepScopeStage `current_step`
            // lookup so review-chain events can land in the
            // right scope without relying solely on the U3
            // defensive bypass.
            current_plan_step: initial_current_plan_step(&config),
            terminal_event_emitted: false,
            // 2026-07-02-004 plan U6: per-loop
            // precheck gate retry registry. In-memory
            // only; rebuilt on process restart (same
            // cold-start semantics as
            // stall_recovery_counts).
            precheck_retries: crate::event_loop::precheck_gate_runner::PrecheckRetryRegistry::new(),
            phase_authority,
            // U4: set post-construction by `from_resolved`; the shared
            // builder always starts with `None` (legacy / test paths).
            execution_contract: None,
            activation_worktree_baselines: std::collections::HashMap::new(),
            // Plan GAP-02 / Unit 2: per-loop stash of StateMachine
            // candidate decisions captured at the candidate stage.
            // Reset every batch by `process_parse_result`; cleared
            // on `process_parse_result` exit.
            pending_state_machine_candidates: Vec::new(),
            state_machine_apply_snapshot: None,
        })
    }

    /// R4 (2026-06-14-003 plan): explicit accessor returning
    /// whether the preset's `event_loop.enforce_current_unit` is
    /// active.  The CLI uses this to surface the value in
    /// diagnostics; the actual contract is enforced inside
    /// `TaskStore::ensure` after `ralph-cli`'s `task_cli` enables
    /// the contract unconditionally (the contract is opt-in at the
    /// *key* level — only `uN-` slugs are gated — so legacy keys
    /// are unaffected).
    pub fn enforce_current_unit_active(&self) -> bool {
        self.config.event_loop.enforce_current_unit
    }

    /// Creates a new event loop with explicit diagnostics collector (for testing).
    ///
    /// **Test-only.** Production code must construct via
    /// [`EventLoop::from_resolved_no_context`] so the config passes the
    /// fallible execution-contract compile boundary (U2) first.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_diagnostics(
        config: RalphConfig,
        diagnostics: crate::diagnostics::DiagnosticsCollector,
    ) -> Self {
        Self::build_no_context(config, diagnostics)
    }

    /// Ungated no-context builder shared by the test-only
    /// [`EventLoop::with_diagnostics`] / [`EventLoop::new`] and the production
    /// [`EventLoop::from_resolved_no_context`].
    pub(super) fn build_no_context(
        mut config: RalphConfig,
        diagnostics: crate::diagnostics::DiagnosticsCollector,
    ) -> Self {
        // Solo mode safety guard: force scratchpad enabled when no hats defined
        if config.hats.is_empty() && !config.core.scratchpad.enabled {
            warn!(
                "core.scratchpad.enabled is false but no hats are defined. \
                 Scratchpad is the only continuity mechanism in solo mode — forcing enabled."
            );
            config.core.scratchpad.enabled = true;
        }

        let registry = HatRegistry::from_runtime_config(&config);
        let publish_schemas = config
            .event_loop
            .event_policy
            .as_ref()
            .map(|p| p.schemas.clone())
            .unwrap_or_default();
        let instruction_builder = InstructionBuilder::with_publish_schemas(
            config.core.clone(),
            config.events.clone(),
            publish_schemas,
        );

        let mut bus = EventBus::new();

        // Per spec: "Hatless Ralph is constant — Cannot be replaced, overwritten, or configured away"
        // Ralph is ALWAYS registered as the universal fallback for orphaned events.
        // Custom hats are registered first (higher priority), Ralph catches everything else.
        // The builtin "ralph" hat is already registered in the registry via `from_runtime_config`.
        for hat in registry.all() {
            bus.register(hat.clone());
        }

        if config.hats.is_empty() {
            debug!("Solo mode: Ralph is the only coordinator");
        } else {
            debug!(
                "Multi-hat mode: {} custom hats + Ralph as fallback",
                config.hats.len()
            );
        }

        // Build skill registry from config
        let workspace_root = std::path::Path::new(".");
        let mut skill_registry = if config.skills.enabled {
            SkillRegistry::from_config(
                &config.skills,
                workspace_root,
                Some(config.cli.backend.as_str()),
            )
            .unwrap_or_else(|e| {
                warn!(
                    "Failed to build skill registry: {}, using empty registry",
                    e
                );
                SkillRegistry::new(Some(config.cli.backend.as_str()))
            })
        } else {
            SkillRegistry::new(Some(config.cli.backend.as_str()))
        };

        // Remove task/memory skills from the index when their config is disabled
        if !config.tasks.enabled {
            skill_registry.remove("ralph-tools-tasks");
        }
        if !config.memories.enabled {
            skill_registry.remove("ralph-tools-memories");
        }

        let skill_index = if config.skills.enabled {
            skill_registry.build_index(None)
        } else {
            String::new()
        };

        // When memories are enabled, add tasks CLI instructions alongside scratchpad
        let ralph = HatlessRalph::new(
            config.event_loop.completion_promise.clone(),
            config.core.clone(),
            &registry,
            config.event_loop.starting_event.clone(),
        )
        .with_memories_enabled(config.memories.enabled)
        .with_skill_index(skill_index);

        // Read events path from marker file, fall back to default if not present
        // The marker file is written by run_loop_impl() at run startup
        let events_path = std::fs::read_to_string(".ralph/current-events")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| ".ralph/events.jsonl".to_string());
        let event_reader = EventReader::new(&events_path);

        let mut state = LoopState::new();
        let handoff_timeout = config
            .event_loop
            .workflow_contract
            .as_ref()
            .map(|wc| wc.effective_timeout_seconds())
            .unwrap_or(crate::config::HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS);
        state.handoff_tracker = crate::workflow_contract::HandoffTracker::new()
            .with_default_timeout(std::time::Duration::from_secs(handoff_timeout));

        let (stage_pipeline, flow_step_totals, phase_authority) =
            build_stage_pipeline_from_config(&config);

        Self {
            config: config.clone(),
            registry,
            bus,
            state,
            instruction_builder,
            ralph,
            robot_guidance: Vec::new(),
            event_reader,
            diagnostics,
            loop_context: None,
            skill_registry,
            handoff_index: crate::workflow_contract::HandoffIndex::from_config(&config),
            recovery_responder: RecoveryResponder::new(Arc::new(
                config.telemetry.runtime_diagnosis.clone(),
            )),
            hat_lifecycle_tracker: ActivationLifecycleTracker::new(),
            // U3: no-context path (e.g. `ralph inspect prompt`) has no
            // workspace, so the registry cannot be opened.
            activation_registry: None,
            ephemeral_isolation: crate::ephemeral_isolation::EphemeralIsolation::new(),
            idempotent_log: std::sync::Mutex::new(
                crate::state::idempotent_log::IdempotentLog::disabled(),
            ),
            stage_pipeline,
            flow_step_totals,
            // P1-5 (2026-06-27 adversarial review):
            // per-task repair state machine registry.
            // The map is empty on construction; the
            // `RepairDispatchStage` lazily inserts a
            // fresh machine for each new `task_key`.
            repair_state_machines: std::collections::HashMap::new(),
            repair_stream_pending: 0,
            current_plan_step: initial_current_plan_step(&config),
            terminal_event_emitted: false,
            // 2026-07-02-004 plan U6: per-loop
            // precheck gate retry registry (see
            // matching initialiser in the first
            // `with_context_and_diagnostics` body).
            precheck_retries: crate::event_loop::precheck_gate_runner::PrecheckRetryRegistry::new(),
            phase_authority,
            // U4: set post-construction by `from_resolved_no_context`.
            execution_contract: None,
            activation_worktree_baselines: std::collections::HashMap::new(),
            // Plan GAP-02 / Unit 2: per-loop stash of StateMachine
            // candidate decisions captured at the candidate stage.
            pending_state_machine_candidates: Vec::new(),
            // Plan GAP-02 / Unit 3: pre-apply live-runtime snapshot
            // for rollback on projection commit failure.
            state_machine_apply_snapshot: None,
        }
    }

    /// Returns the loop context, if one was provided.
    pub fn loop_context(&self) -> Option<&LoopContext> {
        self.loop_context.as_ref()
    }

    /// Returns the tasks path based on loop context or default.
    pub(super) fn tasks_path(&self) -> PathBuf {
        self.loop_context
            .as_ref()
            .map(|ctx| ctx.tasks_path())
            .unwrap_or_else(|| PathBuf::from(".ralph/agent/tasks.jsonl"))
    }

    /// Plan 2026-08-13-003 U4: loop history path (used by
    /// `build_resume_context_from_sources`). Returns
    /// `None` when the loop context does not expose a
    /// history path so the caller falls back to a safe
    /// zero.
    pub(super) fn loop_history_path(&self) -> Option<PathBuf> {
        self.loop_context.as_ref().map(|ctx| ctx.history_path())
    }

    /// Plan 2026-08-13-003 U4: progress.md path (used by
    /// `build_resume_context_from_sources`). Derived from
    /// the loop context workspace.
    pub(super) fn progress_path(&self) -> Option<PathBuf> {
        self.loop_context.as_ref().map(|ctx| {
            ctx.workspace()
                .join(".ralph")
                .join("agent")
                .join("progress.md")
        })
    }

    /// Plan 2026-08-13-003 U4 + 2026-08-13-003 fix-plan U5 R10:
    /// scratchpad path. Honours `core.scratchpad.path` /
    /// `core.scratchpad.enabled` from the RalphConfig. When the
    /// config points at a custom path we resolve it under the
    /// loop workspace; when `enabled` is `false` we return
    /// `None` so callers (e.g. `build_resume_context_from_sources`)
    /// can fall back to an explicit unavailable marker instead of
    /// silently reading the default path.
    pub(super) fn resume_scratchpad_path(&self) -> Option<PathBuf> {
        let scratchpad_cfg = &self.config.core.scratchpad;
        if !scratchpad_cfg.enabled {
            return None;
        }
        self.loop_context.as_ref().map(|ctx| {
            let configured = &scratchpad_cfg.path;
            // Absolute paths are honoured as-is (operator
            // override); relative paths resolve under the
            // workspace's `.ralph/agent/` directory.
            if configured.starts_with('/') {
                return PathBuf::from(configured);
            }
            ctx.workspace()
                .join(".ralph")
                .join("agent")
                .join(configured)
        })
    }

    /// 2026-07-07-002 plan U2: side effects that must run only after execution
    /// contract (and other commit gates) accept an event for the main ledger.
    pub(super) fn apply_contract_committed_side_effects(&mut self, events: &[JsonlEvent]) {
        self.update_bootstrap_flags_from_accepted(events);
        for accepted in events {
            if let Some(consumer) = self.handoff_index.consumer_of(&accepted.topic) {
                // Virtual fan-in consumers (`supervisor` and `wave_runtime`)
                // are runtime components, not `HatRegistry` agent hats. They
                // legitimately consume slot-level `*.unit.done` /
                // `*.unit.failed` topics but have no registry entry and
                // therefore no `triggers` list, so the U16 check below would
                // misread the missing entry as "triggers do not declare the
                // topic" and emit a spurious `task.resume.misrouted`.
                // Skip both the misrouted check and the 600s pending-handoff
                // registration for virtual consumers; they are dispatched by
                // their runtime, never via handoff/`task.resume`.
                // Ordinary hats fall through to the unchanged U16 logic.
                if !crate::event_origin::is_virtual_runtime_consumer(consumer) {
                    let consumer_triggers_ok = self
                        .registry
                        .get_config(&HatId::from(consumer))
                        .map(|cfg| {
                            crate::workflow_contract::handoff_index::check_hat_triggers(
                                &cfg.triggers,
                                accepted.topic.as_str(),
                            )
                            .is_ok()
                        })
                        .unwrap_or(false);
                    if !consumer_triggers_ok {
                        warn!(
                            topic = %accepted.topic,
                            consumer = %consumer,
                            "U16 handoff: consumer hat's `triggers` does not declare \
                             this topic — emitting task.resume.misrouted diagnostic, \
                             skipping 600s pending registration"
                        );
                        let diagnostic = Event::new(
                            "task.resume.misrouted",
                            format!(
                                "U16: consumer hat `{}` does not declare `{}` in its \
                                 `triggers` list; handoff skipped to avoid 600s stall \
                                 escalation. Fix: add `{}` to the hat's `triggers:` or \
                                 remove the producer from this hat's emission scope.",
                                consumer, accepted.topic, accepted.topic
                            ),
                        )
                        .with_source(HatId::from("ralph"));
                        self.state.record_event(&diagnostic);
                        self.bus.publish(diagnostic);
                        continue;
                    }
                    let event_id = format!("{}:{}", accepted.ts, accepted.topic);
                    self.state.handoff_tracker.on_handoff_accepted(
                        accepted.topic.clone(),
                        consumer.to_string(),
                        event_id.clone(),
                        std::time::Instant::now(),
                    );
                }
            }

            match accepted.topic.as_str() {
                "work.done" => {
                    if let Some(p) = accepted.payload.as_deref()
                        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
                        && let (Some(pn), Some(st), Some(ti)) = (
                            obj.get("plan_name").and_then(|v| v.as_str()),
                            obj.get("step").and_then(|v| v.as_str()),
                            obj.get("task_id").and_then(|v| v.as_str()),
                        )
                    {
                        let key = LoopState::work_done_dedup_key(pn, st, ti);
                        self.state.work_done_seen_tasks.insert(key);
                    }
                    let ctx = self.runtime_recovery_context(std::slice::from_ref(accepted));
                    self.apply_runtime_recovery_actions(&ctx);
                }
                "fix.applied" => {
                    if let Some(p) = accepted.payload.as_deref()
                        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
                    {
                        let plan_name = obj
                            .get("plan_name")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        let step = obj
                            .get("completed_step")
                            .or_else(|| obj.get("step"))
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        if let (Some(pn), Some(st)) = (&plan_name, &step) {
                            self.state.prune_work_done_bucket(pn, st);
                            let task_id = obj
                                .get("task_id")
                                .and_then(|v| v.as_str())
                                .map(String::from);
                            if let Some(ti) = task_id.as_deref() {
                                let new_count = self.state.increment_fix_round(pn, st, ti);
                                if new_count >= LoopState::FIX_ROUND_HARD_CAP {
                                    warn!(
                                        plan = %pn,
                                        step = %st,
                                        task = %ti,
                                        count = new_count,
                                        "fix-round hard cap reached; emitting fix.exhausted"
                                    );
                                    let exhausted_payload = serde_json::json!({
                                        "plan_name": pn,
                                        "fix_round": new_count,
                                        "task_id": ti,
                                        "task_key": obj.get("task_key").and_then(|v| v.as_str()).unwrap_or(""),
                                        "step": st,
                                        "reason": format!(
                                            "fix budget exhausted (max {} rounds)",
                                            LoopState::FIX_ROUND_HARD_CAP
                                        ),
                                    });
                                    self.bus.publish(Event::new(
                                        "fix.exhausted",
                                        exhausted_payload.to_string(),
                                    ));
                                }
                                if let Some(ref mut policy_state) = self.state.policy_runtime_state
                                {
                                    policy_state.prune_review_dimension_ready_bucket(pn, st, ti);
                                    policy_state
                                        .prune_review_dimensions_complete_bucket(pn, st, ti);
                                    policy_state.prune_work_done_bucket(pn, st);
                                    policy_state.prune_work_ready_bucket(pn, st);
                                    policy_state.prune_test_result_buckets(pn, st, ti);
                                    policy_state.prune_review_start_bucket(pn, ti);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

// Plan 2026-08-16-1015 U5: acceptance lifecycle tests for the
// `TerminalTargetGuardStage` + `HandoffTracker` integration.
//
// Covers three scenarios:
// 1. `report.done{triggered=reporter}` with `required_target_hat=reporter`
//    schema contract → handoff tracker registers ONLY `reporter`.
// 2. `report.done{triggered=executor}` with the same contract → pipeline
//    rejects with `terminal_target_mismatch`, handoff tracker registers
//    NOTHING.
// 3. (regression) `work.done{triggered=executor}` without a
//    `required_target_hat` contract → existing acceptance semantics
//    preserved, no regression.
//
// Test entry: `cargo nextest run -p ralph-core -- accepted_report_done_with_reporter_target`

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EventPolicyConfig, EventSchema, HatConfig, HatExecutionMode, RalphConfig};
    use crate::event_loop::stage_pipeline::{FlowStep, StageContext};
    use crate::event_reader::Event as JsonlEvent;
    use ralph_proto::Event;
    use std::collections::HashMap;

    /// Build a `RalphConfig` with `event_policy.schemas` declaring
    /// `required_target_hat` for one topic AND a hat definition that
    /// publishes that topic (so the handoff index finds a consumer).
    fn config_with_required_target_hat(topic: &str, required_target_hat: Option<&str>) -> RalphConfig {
        let mut cfg = RalphConfig::default();
        let mut schemas = HashMap::new();
        schemas.insert(
            topic.to_string(),
            EventSchema {
                required_target_hat: required_target_hat.map(String::from),
                ..Default::default()
            },
        );
        cfg.event_loop.event_policy = Some(EventPolicyConfig {
            enabled: true,
            schemas,
            ..EventPolicyConfig::default()
        });
        cfg.event_loop.execution_mode = HatExecutionMode::Isolated;
        // Add a hat that triggers on `topic` so `HandoffGraph::from_config`
        // populates `topic_subscribers` and the handoff index resolves a consumer.
        let mut hats = HashMap::new();
        hats.insert(
            "reporter".to_string(),
            HatConfig {
                name: "Reporter".to_string(),
                triggers: vec![topic.to_string()],
                publishes: vec![],
                ..Default::default()
            },
        );
        cfg.hats = hats;
        cfg
    }

    /// Build a minimal `EventLoop` from a `RalphConfig`.
    fn make_loop(config: RalphConfig) -> EventLoop {
        EventLoop::new(config)
    }

    /// Run an event through the loop's stage pipeline and return the result.
    fn drive_event_through_pipeline(loop_: &mut EventLoop, event: Event) -> Result<(), crate::event_loop::stage_pipeline::StageReject> {
        let mut repair_states = std::collections::HashMap::new();
        let mut ctx = StageContext::new(
            FlowStep::new("unit_loop"),
            "loop-test",
            1,
            &mut repair_states,
        );
        loop_.stage_pipeline.run(&mut ctx, &event)
    }

    // Test 1: `report.done{triggered=reporter}` with `required_target_hat=reporter`
    // schema contract → handoff tracker registers ONLY `reporter`.
    #[test]
    fn accepted_report_done_with_reporter_target_registers_only_reporter_in_handoff_tracker() {
        let config = config_with_required_target_hat("report.done", Some("reporter"));
        let mut loop_ = make_loop(config);

        // Build a `report.done` event with `triggered=reporter`.
        let event = Event::new("report.done", r#"{"triggered":"reporter"}"#);

        // Pipeline must accept it.
        let result = drive_event_through_pipeline(&mut loop_, event.clone());
        assert!(
            result.is_ok(),
            "report.done{{triggered=reporter}} should pass TerminalTargetGuard: {result:?}"
        );

        // Simulate handoff acceptance by calling `apply_contract_committed_side_effects`
        // with the accepted event.  This is the production path that registers the
        // pending handoff on the consumer from the handoff index.
        let jsonl_event = JsonlEvent {
            topic: "report.done".into(),
            ts: "2026-08-16T10:00:00Z".into(),
            source: Some("reporter".into()),
            hat: Some("reporter".into()),
            triggered: Some("reporter".into()),
            payload: Some(r#"{"triggered":"reporter"}"#.into()),
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        };
        loop_.apply_contract_committed_side_effects(&[jsonl_event]);

        // The handoff tracker must have exactly 1 pending entry (for `reporter`).
        assert_eq!(
            loop_.state.handoff_tracker.pending_count(),
            1,
            "handoff tracker should have 1 pending entry for reporter"
        );

        // Simulate `reporter` hat activation — pending count should drop to 0.
        loop_.state.handoff_tracker.on_hat_activated("reporter");
        assert_eq!(
            loop_.state.handoff_tracker.pending_count(),
            0,
            "after reporter activation, handoff tracker should be empty"
        );
    }

    // Test 2: `report.done{triggered=executor}` with the same contract
    // → pipeline rejects with `terminal_target_mismatch`, handoff tracker
    // registers NOTHING.
    #[test]
    fn rejected_report_done_with_executor_target_does_not_register_any_handoff_tracker_entry() {
        let config = config_with_required_target_hat("report.done", Some("reporter"));
        let mut loop_ = make_loop(config);

        // Build a `report.done` event with `triggered=executor` (wrong target).
        let event = Event::new("report.done", r#"{"triggered":"executor"}"#);

        // Pipeline must reject it.
        let result = drive_event_through_pipeline(&mut loop_, event.clone());
        let reject = result.expect_err("report.done{{triggered=executor}} should be rejected");
        assert_eq!(
            reject.reason_code, "terminal_target_mismatch",
            "rejection reason should be terminal_target_mismatch, got: {}",
            reject.reason_code
        );

        // Handoff tracker must still be empty — the event never reached
        // `apply_contract_committed_side_effects`.
        assert_eq!(
            loop_.state.handoff_tracker.pending_count(),
            0,
            "rejected event should not register any handoff tracker entry"
        );
    }

    // Test 3 (regression): `work.done{triggered=executor}` without a
    // `required_target_hat` contract → existing acceptance semantics
    // preserved, no regression.
    #[test]
    fn non_contract_topic_work_done_with_executor_target_passes_through_unchanged() {
        // `work.done` has NO entry in schemas → no terminal-target contract.
        let config = config_with_required_target_hat("report.done", Some("reporter"));
        let mut loop_ = make_loop(config);

        // Build a `work.done` event (topic not in the schema map).
        let event = Event::new("work.done", r#"{"triggered":"executor","plan_name":"p","step":"S1","task_id":"t1"}"#);

        // Pipeline must accept it (no contract applies).
        let result = drive_event_through_pipeline(&mut loop_, event.clone());
        assert!(
            result.is_ok(),
            "work.done{{triggered=executor}} should pass through unchanged: {result:?}"
        );

        // Handoff tracker must be empty (work.done is not a terminal event
        // that goes through handoff registration in the same way — or at
        // minimum, no pending entry should appear for executor here).
        assert_eq!(
            loop_.state.handoff_tracker.pending_count(),
            0,
            "work.done without contract should not register handoff tracker entry"
        );
    }

}
