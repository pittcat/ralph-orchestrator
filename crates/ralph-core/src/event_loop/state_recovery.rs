//! EventLoop implementation region 4.

use super::*;

impl EventLoop {
    /// Returns true if the verdict event payload resolves to a
    /// `Fail` verdict, either via the typed `Verdict::from_payload`
    /// path (when `gate.verdict_field` is configured) or via the
    /// legacy binary `fail_field == fail_value` match (when
    /// `verdict_field` is `None`).
    ///
    /// The typed path is the 2026-06-26 plan U5 contract: it
    /// recognises `pass` / `pass_with_residuals` / `fail` as three
    /// distinct terminal states and applies `max_residuals` to
    /// promote or downgrade `pass_with_residuals`. The legacy
    /// path is preserved so presets that have not yet opted into
    /// the new field keep working unchanged.
    ///
    /// Returns false on:
    /// - payload not valid JSON,
    /// - verdict field missing (legacy path: absence == not failing
    ///   because the gate is opt-in and only trips on an explicit
    ///   `fail` value),
    /// - payload that fails to parse as a typed `Verdict` (treated
    ///   as "not failing" so a transient shape mismatch does not
    ///   silently kill the loop; the operator can grep
    ///   `verdict_parse_error` in the diagnostics if the
    ///   mismatch persists).
    pub(super) fn verdict_payload_is_fail(
        payload: &str,
        gate: &crate::config::VerdictGateConfig,
    ) -> bool {
        if let Some(verdict_field) = gate.verdict_field.as_deref() {
            // Typed Verdict path. Threshold defaults to 8 to
            // match the ralph-e2e `primary-20260624-032505`
            // case (see `default_max_residuals` in
            // `crate::config::loop_config`).
            const DEFAULT_MAX_RESIDUALS: u32 = 8;
            let max_residuals = Some(DEFAULT_MAX_RESIDUALS);
            let verdict =
                Verdict::from_payload(payload, verdict_field, gate.residual_count_field.as_deref());
            match verdict {
                Ok(v) => v.resolve(max_residuals).is_fail(),
                Err(_) => {
                    tracing::debug!(
                        verdict_field,
                        "verdict payload did not parse as typed Verdict; \
                         treating as not failing"
                    );
                    false
                }
            }
        } else {
            // Legacy binary match: `fail_field == fail_value`.
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                return false;
            };
            value
                .get(&gate.fail_field)
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == gate.fail_value)
        }
    }

    /// Compare the top-level fields declared in `match_cfg` between
    /// the predecessor payload and the completion payload. Returns
    /// `Some(reason)` on mismatch, missing field, or non-object
    /// payload; `None` when all declared fields match.
    pub(super) fn completion_payload_mismatch(
        match_cfg: &crate::config::CompletionPayloadMatchConfig,
        predecessor_payload: &str,
        completion_payload: &str,
    ) -> Option<String> {
        let pred: serde_json::Value = match serde_json::from_str(predecessor_payload) {
            Ok(v) => v,
            Err(_) => return Some("predecessor payload is not valid JSON".to_string()),
        };
        let comp: serde_json::Value = match serde_json::from_str(completion_payload) {
            Ok(v) => v,
            Err(_) => return Some("completion payload is not valid JSON".to_string()),
        };
        let pred_obj = pred.as_object()?;
        let comp_obj = comp.as_object()?;
        for field in &match_cfg.fields {
            let pred_val = pred_obj.get(field);
            let comp_val = comp_obj.get(field);
            match (pred_val, comp_val) {
                (Some(p), Some(c)) if p == c => continue,
                (Some(p), Some(c)) => {
                    return Some(format!(
                        "field '{field}' mismatch: predecessor={p}, completion={c}"
                    ));
                }
                (Some(_), None) => {
                    return Some(format!("field '{field}' missing in completion payload"));
                }
                (None, Some(_)) => {
                    return Some(format!("field '{field}' missing in predecessor payload"));
                }
                (None, None) => {
                    return Some(format!("field '{field}' missing in both payloads"));
                }
            }
        }
        None
    }

    /// Initializes the loop by publishing the start event.
    pub fn initialize(&mut self, prompt_content: &str) {
        // Use configured starting_event or default to task.start for backward compatibility
        let topic = self
            .config
            .event_loop
            .starting_event
            .clone()
            .unwrap_or_else(|| "task.start".to_string());
        self.initialize_with_topic(&topic, prompt_content);
    }

    /// Initializes the loop for resume mode by publishing task.resume.
    ///
    /// Per spec: "User can run `ralph resume` to restart reading existing scratchpad."
    /// The planner should read the existing scratchpad rather than doing fresh gap analysis.
    ///
    /// **U7b (plan 2026-06-21-002):** when the
    /// `UNIFIED_DETERMINISTIC_CORRECTION=1` env var is set,
    /// this function delegates to
    /// [`Self::initialize_resume_with_context`] which emits the
    /// new `loop.resume` control event (see
    /// [`ralph_proto::LOOP_RESUME`]) and seeds a
    /// [`crate::correction::ResumeContext`] block in the next
    /// prompt.  The legacy `task.resume` path is preserved for
    /// callers that have not opted in.
    ///
    /// Plan 2026-08-13-003 U4: the production default context
    /// is no longer `ResumeContext::default()` — it is built
    /// from the live `LoopHistory::last_iteration`, the
    /// current-loop `TaskStore` closed/open counts, the
    /// `.ralph/agent/progress.md` `ProgressSnapshot`, and the
    /// scratchpad headline. Read failures fall back to safe
    /// empty values (per D5 + plan §20) so the loop.resume
    /// event still fires for legacy workspaces that lack the
    /// auxiliary files.
    pub fn initialize_resume(&mut self, prompt_content: &str) {
        if crate::correction::is_correction_enabled() {
            let context = self.build_resume_context_from_sources();
            self.initialize_resume_with_context(prompt_content, context);
            return;
        }
        // Legacy path: emit `task.resume` regardless of starting_event
        // config.  Preserved so the U1-U6 test suite keeps
        // passing without the feature flag.
        self.initialize_with_topic("task.resume", prompt_content);
        // Unit 3: rebuild bootstrap gate from recorded events so resume
        // does not re-open the guidance-suppression window mid-loop.
        self.rebuild_bootstrap_flags_from_recorded_events();
    }

    /// Plan 2026-08-13-003 U4: build a production
    /// [`crate::correction::ResumeContext`] from the live
    /// sources required by D5 (LoopHistory / current-loop
    /// TaskStore / progress.md / scratchpad). Each source is
    /// read independently; failure of any single read leaves
    /// that field empty/zero (no fabricated values).
    fn build_resume_context_from_sources(&self) -> crate::correction::ResumeContext {
        use crate::correction::ResumeContext;
        use crate::step_handoff::progress_task_gate::ProgressSnapshot;

        let loop_id = self
            .loop_id_label()
            .trim()
            .to_string();

        // Last iteration from LoopHistory (if available).
        let history_path = self.loop_history_path();
        let last_iteration = history_path
            .as_ref()
            .and_then(|p| crate::loop_history::LoopHistory::new(p.clone()).last_iteration().ok())
            .flatten()
            .unwrap_or(0);

        // Current-loop TaskStore closed count.
        let tasks_path = self.tasks_path();
        let closed_tasks_count = read_closed_tasks_count(&tasks_path);

        // ProgressSnapshot summary (current_step + completed).
        let progress_path = self.progress_path();
        let progress_summary = progress_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|content| {
                let snap = ProgressSnapshot::parse(&content);
                let mut out = String::new();
                if let Some(step) = snap.current_step() {
                    out.push_str(&format!("current_step={step}; "));
                }
                if !snap.completed_steps.is_empty() {
                    out.push_str(&format!(
                        "completed={}; ",
                        snap.completed_steps.join(",")
                    ));
                }
                out
            })
            .unwrap_or_default();

        // Scratchpad headline (first non-empty heading).
        let scratchpad_path = self.resume_scratchpad_path();
        let scratchpad_headline = scratchpad_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|content| first_meaningful_heading(&content))
            .unwrap_or_default();

        ResumeContext::new(
            loop_id,
            closed_tasks_count,
            progress_summary,
            last_iteration,
            scratchpad_headline,
        )
    }

    /// U7b (plan 2026-06-21-002): initialize resume with an
    /// explicit [`crate::correction::ResumeContext`].  Emits a
    /// `loop.resume` control event (see [`ralph_proto::LOOP_RESUME`])
    /// instead of `task.resume`, and seeds the resume block in
    /// [`crate::correction::PromptContext`] so the next prompt
    /// contains `## LOOP RESUME CONTEXT`.
    ///
    /// Callers should construct the resume context from the
    /// scratchpad / progress.md / closed-tasks state at the
    /// resume boundary; this function only routes the event and
    /// stores the block.
    pub fn initialize_resume_with_context(
        &mut self,
        prompt_content: &str,
        resume_context: crate::correction::ResumeContext,
    ) {
        // Always push the resume block to `state.prompt_context`
        // regardless of the legacy `task.resume` topic.  This
        // is the U7b contract: the next prompt always carries
        // `## LOOP RESUME CONTEXT` when the user runs
        // `--continue`, even when the feature flag is off.
        self.state.prompt_context.resume_blocks.push(resume_context);

        // Emit the boot topic.  Prefer the new `loop.resume`
        // event when the feature flag is on; fall back to
        // `task.resume` for the legacy test paths.
        let topic = if crate::correction::is_correction_enabled() {
            ralph_proto::LOOP_RESUME
        } else {
            "task.resume"
        };
        self.initialize_with_topic(topic, prompt_content);
        // Unit 3: rebuild bootstrap gate from recorded events so resume
        // does not re-open the guidance-suppression window mid-loop.
        self.rebuild_bootstrap_flags_from_recorded_events();
    }

    /// U2 (plan 2026-08-03-004): initialize the loop from a validated
    /// parallel-forge resume manifest recovery. Publishes the
    /// recovery's TARGETED `task.resume` — the existing recovery
    /// contract, no second resume message type — instead of the
    /// configured starting event, and pins `pending_recovery_hat` so
    /// the next activation lands on the manifest's pending hat with
    /// its original trigger embedded in the payload.
    ///
    /// Idempotent: when the target hat already holds a
    /// system-injected `task.resume` bootstrap event, the publish is
    /// skipped so a repeated bootstrap never inserts a second
    /// recovery obligation. The pin is (re)applied either way.
    ///
    /// Callers must run the manifest through
    /// [`rejection::task_resume_from_manifest`] first (digest /
    /// target-hat validation) — this method publishes the result.
    pub fn initialize_manifest_resume(
        &mut self,
        prompt_content: &str,
        recovery: rejection::ManifestResumeRecovery,
    ) {
        // The objective must survive iterations exactly like
        // `initialize_with_topic` does for fresh starts.
        self.ralph.set_objective(prompt_content.to_string());

        let already_bootstrapped =
            self.bus
                .peek_pending(&recovery.target_hat)
                .is_some_and(|pending| {
                    pending.iter().any(|event| {
                        event.topic.as_str() == "task.resume" && event.system_injected == Some(true)
                    })
                });
        if already_bootstrapped {
            debug!(
                target_hat = %recovery.target_hat.as_str(),
                "U2: manifest resume bootstrap repeated; skipping duplicate task.resume publish"
            );
        } else {
            // Plan 2026-08-10-001 U1: route the manifest bootstrap
            // through the unified publisher. The `system_injected`
            // and `with_source("orchestrator")` metadata are
            // preserved as the helper's resolved publisher —
            // `source` is carried by `recovery.payload` already;
            // `system_injected` is a `Ralph` lifecycle flag
            // routed by the original event, so the targeted
            // resume here keeps the legacy metadata via the
            // publish path. The live `peek_pending` adapter
            // covers the dedup check.
            let loop_id_for_resume = self.current_loop_id();
            crate::event_loop::resume_routing::publish_targeted_resume_for_hat(
                &mut self.bus,
                &self.registry,
                None,
                loop_id_for_resume.as_deref(),
                recovery.target_hat.as_str(),
                None,
                None,
                None,
                "manifest_resume",
                recovery.payload,
            );
            debug!(
                target_hat = %recovery.target_hat.as_str(),
                original_trigger_topic = %recovery.original_trigger_topic,
                "U2: manifest resume bootstrap published targeted task.resume"
            );
        }
        self.state.pending_recovery_hat = Some(recovery.target_hat);
    }

    /// Common initialization logic with configurable topic.
    pub(super) fn initialize_with_topic(&mut self, topic: &str, prompt_content: &str) {
        // Store the objective so it persists across all iterations.
        // After iteration 1, bus.take_pending() consumes the start event,
        // so without this the objective would be invisible to later hats.
        self.ralph.set_objective(prompt_content.to_string());

        // Unit 3 (2026-06-16-002 plan): reset the bootstrap gate only on
        // a fresh loop start — not on `task.resume` (resume rebuilds from
        // events.jsonl immediately after).
        if topic == "work.start" || topic == "task.start" {
            self.state.bootstrap_complete = false;
            self.state.bootstrap_failed = false;
        }

        let start_event = Event::new(topic, prompt_content)
            .with_source("orchestrator")
            .with_system_injected();
        self.bus.publish(start_event);
        debug!(topic = topic, "Published {} event", topic);
    }

    /// Write a hold artifact when event policy triggers a hold.
    pub(super) fn write_hold_artifact(&self, reason: Option<&str>) -> std::io::Result<()> {
        let workspace = self
            .loop_context
            .as_ref()
            .map(|ctx| ctx.workspace().to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let ralph_dir = workspace.join(".ralph");
        std::fs::create_dir_all(&ralph_dir)?;

        let hold_path = ralph_dir.join("hold-state.json");
        let hold_record = serde_json::json!({
            "schema_version": 1,
            "source": "event_policy",
            "reason": reason.unwrap_or("Policy violation"),
            "held_at": chrono::Utc::now().to_rfc3339(),
        });
        let bytes = serde_json::to_vec_pretty(&hold_record)?;

        // Atomic write
        let temp_path = ralph_dir.join(format!(
            ".hold-state.tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&temp_path, &bytes)?;
        std::fs::rename(&temp_path, &hold_path)?;

        info!(path = ?hold_path, "Wrote hold-state artifact");
        Ok(())
    }

    /// Gets the next hat to execute (if any have pending events).
    ///
    /// Per "Hatless Ralph" architecture: When custom hats are defined, Ralph is
    /// always the executor. Custom hats define topology (pub/sub contracts) that
    /// Ralph uses for coordination context, but Ralph handles all iterations.
    ///
    /// - Solo mode (no custom hats): Returns "ralph" if Ralph has pending events
    /// - Multi-hat mode (custom hats defined): Always returns "ralph" if ANY hat has pending events
    ///
    /// **Isolated mode** uses round-robin scheduling via
    /// `EventBus::select_next_hat_with_pending` to guarantee fair selection
    /// among all pending hats. The cursor is anchored in the full
    /// registered hat order, so a hat whose queue is drained or
    /// deregistered does not reset the cursor to the lexicographic first
    /// non-empty hat.
    ///
    /// **NOTE**: This method takes `&mut self` because isolated-mode round-robin
    /// advances the bus's internal cursor.
    pub fn next_hat(&mut self) -> Option<&HatId> {
        // U3 (2026-06-13-001 plan): hard-gate / wave-recovery hat pinning.
        //
        // When a `pending_recovery_hat` is recorded (set by the
        // runner's `inject_missing_event_hard_gate_guidance` or
        // `inject_wave_policy_rejection_guidance` helpers), the next
        // iteration MUST activate that hat, not whatever the
        // round-robin / coordinator topology would pick.  The default
        // round-robin would otherwise drift to `executor` after a
        // `review-coordinator` hard gate, breaking the loop.
        //
        // We use `take` semantics: the field is cleared on the
        // iteration that consumes it, so the loop never gets stuck on
        // a single hat past a single activation.  The `bus` already
        // publishes the recovery `human.guidance` event for that hat,
        // so the next prompt will see the schema-level / missing-
        // event message and the obligation should be satisfied on the
        // very next attempt.
        if let Some(pending_hat) = self.state.pending_recovery_hat.take() {
            // Only honor the pin when the hat is actually registered;
            // an obsolete or test-only hat id is treated as a no-op
            // and selection falls through to the normal algorithm.
            if self.bus.hat_ids().any(|id| *id == pending_hat) {
                return self.bus.hat_ids().find(|id| **id == pending_hat);
            }
            // Hat unknown (config drift, deregistration, or worktree
            // with a different hat set) — log so the operator can
            // see the recovery intent was lost instead of silently
            // routing to a different hat via round-robin.
            tracing::warn!(
                pending_hat = %pending_hat,
                "pending_recovery_hat references an unregistered hat id; falling through to default selection"
            );
        }

        // 2026-07-02-001 review P0 fix (code-review #1): when a hat
        // has a **targeted** event in its pending queue (i.e. an
        // event with `event.target == Some(hat_id)`), the next
        // activation MUST be that hat. The pre-existing
        // `event_bus::publish` direct-target contract already routes
        // targeted events to the named hat's queue; the dispatcher's
        // only remaining job is to ensure the dispatcher picks that
        // hat up next. Without this fast path, a targeted
        // `task.resume` from the 62a40b41
        // `isolated_extra_business_event_dropped` backpressure (or
        // any other targeted recovery signal) could be deferred by
        // the round-robin scan, leaving the over-emitting hat dormant
        // for a full cycle. The hat is selected deterministically
        // (BTreeMap dict order) when multiple hats have targeted
        // events, mirroring the round-robin cursor's tie-breaking.
        //
        // This is a **targeted-event fast path**, separate from the
        // handoff priority pre-emption below. Targeted events are
        // unambiguous by construction (the publisher named a specific
        // hat), so they don't need a "topic-eligibility" filter; the
        // handoff priority path's strict topic-exact predicate is
        // preserved for the broad (untargeted) handoff case.
        let targeted_hat: Option<HatId> = {
            let mut found: Option<HatId> = None;
            for id in self.bus.hat_ids() {
                let has_targeted = self
                    .bus
                    .peek_pending(id)
                    .map(|q| q.iter().any(|event| event.target.as_ref() == Some(id)))
                    .unwrap_or(false);
                if has_targeted {
                    // BTreeMap order → first targeted wins.
                    found = Some(id.clone());
                    break;
                }
            }
            found
        };
        if let Some(ref id) = targeted_hat {
            tracing::debug!(
                target = "ralph_core::event_loop",
                hat = %id,
                "next_hat: targeted event in consumer queue — fast-pathing to that hat"
            );
            // Advance the round-robin cursor to mirror a normal
            // selection (so the next non-targeted selection resumes
            // fairly from the registered successor).
            self.bus.select_next_hat_with_pending(Some(id))?;
            return self.bus.hat_ids().find(|hat_id| hat_id == &id);
        }

        match self.config.event_loop.execution_mode {
            HatExecutionMode::Isolated => {
                // Isolated mode: use round-robin to select the next hat.
                // This advances the cursor on the bus for fair scheduling.
                //
                // 2026-06-28-005: the `has_human_pending` guard that
                // routed to ralph when only human events were pending
                // is gone — the `human_pending` queue was removed
                // together with the `human.guidance` topic.
                // WAC-U5 (2026-06-12-002): handoff priority pre-emption.
                // If the HandoffIndex has at least one priority-eligible
                // entry (unique consumer) and that hat currently has a
                // non-empty pending queue, the dispatcher selects it
                // immediately and the round-robin cursor advances. The
                // scan walks the index in BTreeMap (alphabetical topic)
                // order for determinism. If no priority hat has pending
                // events, we fall through to the normal round-robin
                // pass.
                // 2026-07-02-001 plan U1 (Fix A): handoff priority pre-emption
                // must require **topic-exact pending**, not just a non-empty
                // consumer queue. The pre-fix predicate (consumer queue
                // non-empty → eligible for priority) was susceptible to
                // misleading routing whenever a hat's queue held an event
                // whose topic was *not* the handoff entry's topic (e.g. an
                // untargeted `task.resume` left behind by an earlier round).
                // Such residue would short-circuit the round-robin scan and
                // pre-empt a different hat's legitimate handoff dispatch.
                //
                // "Topic-exact" means `event.topic.as_str() == entry.topic`
                // — string equality on the topic name. Topic *pattern*
                // matching (e.g. `work.*`) is the `EventBus::publish`
                // concern; the dispatcher's priority pre-empt requires the
                // consumer to have a pending event whose topic is the
                // handoff entry's topic verbatim, not a pattern. This is
                // the same contract the HandoffIndex uses for `consumer_of`
                // (see `workflow_contract/handoff_index.rs:228`).
                //
                // The post-fix predicate walks the priority-dispatchable
                // entries in BTreeMap (alphabetical topic) order, and for
                // each `(topic T, consumer C)` checks whether C's pending
                // queue contains an event with `event.topic == T`. Only
                // that case is treated as eligible for priority pre-emption.
                // If no entry yields a topic-exact pending, `priority_hat`
                // stays `None` and the dispatcher falls through to the
                // normal round-robin scan.
                //
                // 2026-07-02-001 review P0 fix (code-review #1): the
                // targeted-event fast path above (`targeted_hat`) handles
                // the 62a40b41 `isolated_extra_business_event_dropped`
                // targeted-`task.resume` reactivation. The
                // priority-predicate additionally filters out topics
                // classified as **orchestrator control / system
                // backpressure** by `ralph_proto::is_orchestrator_control`
                // (`task.resume`, `loop.resume`, `LOOP_COMPLETE`,
                // `LOOP_CANCEL`). These topics *do* appear in
                // `HandoffIndex::entries` when a hat subscribes to them
                // (e.g. `executor` subscribes to `task.resume`), and the
                // strict topic-exact predicate alone is not enough to
                // reject the priority pre-empt — an untargeted
                // `task.resume` residue in such a consumer's queue would
                // still win the priority pre-empt. Filtering them here
                // restores the 62a40b41 contract: system backpressure
                // events never pre-empt a handoff dispatch, and the
                // targeted-event fast path above is the only place such
                // events can re-activate a hat.
                let priority_hat: Option<HatId> =
                    self.handoff_index
                        .entries
                        .iter()
                        .find_map(|(topic, entry)| {
                            let consumer = entry.consumer.as_deref()?;
                            let hat_id = HatId::from(consumer);
                            if ralph_proto::topics::is_orchestrator_control(topic.as_str()) {
                                return None;
                            }
                            let topic_matches = self
                                .bus
                                .peek_pending(&hat_id)
                                .map(|q| {
                                    q.iter().any(|event| event.topic.as_str() == topic.as_str())
                                })
                                .unwrap_or(false);
                            if topic_matches {
                                // KTD-9 / R1: pre-emption hits are observable
                                // so future drift has a forensic trail.
                                tracing::debug!(
                                    target = "ralph_core::event_loop",
                                    topic = %topic,
                                    consumer = %hat_id,
                                    "priority pre-empt: topic-exact pending in consumer queue"
                                );
                                Some(hat_id)
                            } else {
                                None
                            }
                        });
                // Select via round-robin. This updates last_selected.
                // We need to return a borrowed HatId, so we select and then look it up.
                let selected = self
                    .bus
                    .select_next_hat_with_pending(priority_hat.as_ref())?;
                // The selected hat must exist in the bus (it was found in pending).
                self.bus.hat_ids().find(|id| *id == &selected)
            }
            HatExecutionMode::Coordinator => {
                // Coordinator mode: peek for pending, then return ralph if any.
                let has_pending = self.bus.peek_next_hat_with_pending().is_some();

                // 2026-06-28-005: the `has_human_pending` fallback
                // path that routed to ralph when only human events
                // were pending is gone — the `human_pending` queue
                // was removed together with the topic.

                if !has_pending {
                    return None;
                }

                // Coordinator mode (default): In multi-hat mode, always route to Ralph
                // (custom hats define topology only). Ralph's prompt includes the ## HATS
                // section for coordination awareness.
                if self.config.hats.is_empty() {
                    // Solo mode - return the next hat (which is "ralph")
                    self.bus.hat_ids().find(|id| id.as_str() == "ralph")
                } else {
                    // Return "ralph" - the constant coordinator
                    self.bus.hat_ids().find(|id| id.as_str() == "ralph")
                }
            }
        }
    }

    /// Returns the hat that will be triggered by the next pending event, if any.
    pub fn triggered_hat(&mut self) -> Option<HatId> {
        self.next_hat().cloned()
    }

    /// Advances the event reader to the current end of the events file.
    ///
    /// Call this after writing observability records (e.g. start event) to the
    /// events JSONL file so they are not re-read by `process_events_from_jsonl`.
    /// The start event is already published to the bus via `initialize()`, so
    /// re-reading it from the file would cause double-delivery.
    pub fn sync_event_reader_to_file_end(&mut self) {
        let path = self.event_reader.path();
        if let Ok(metadata) = std::fs::metadata(path) {
            self.event_reader.set_position(metadata.len());
        }
    }

    /// Returns the current byte offset of the embedded `EventReader`.
    ///
    /// Primarily for tests that need to assert the cursor was pushed
    /// to the end of the file (e.g. after
    /// [`Self::sync_event_reader_to_file_end`]) so a freshly
    /// appended bootstrap record is not re-delivered to the bus.
    pub fn event_reader_position(&self) -> u64 {
        self.event_reader.position()
    }

    /// Reads the events file from the current reader offset without
    /// advancing the cursor.
    ///
    /// Convenience wrapper for tests so they can assert that a
    /// freshly persisted bootstrap line is no longer "new" after
    /// `sync_event_reader_to_file_end()` is called.  The wrapper
    /// deliberately exposes the same `ParseResult` shape returned by
    /// `EventReader::read_new_events` so test assertions stay
    /// uniform.
    pub fn peek_event_reader_for_test(&self) -> std::io::Result<crate::event_reader::ParseResult> {
        self.event_reader.peek_new_events()
    }

    /// Points the JSONL candidate reader at a different file and resets its
    /// offset. State-machine runs use this to keep raw candidate events
    /// separate from the accepted event history.
    pub fn set_event_reader_path(&mut self, path: impl Into<PathBuf>) {
        self.event_reader = EventReader::new(path);
    }

    /// Checks if any hats have pending events.
    ///
    /// Use this after `process_output` to detect if the LLM failed to publish an event.
    /// If false after processing, the loop will terminate on the next iteration.
    ///
    /// Uses peek (no side-effect) to avoid advancing the round-robin cursor.
    pub fn has_pending_events(&self) -> bool {
        self.bus.has_pending()
    }

    /// Checks if any pending events are human guidance events.
    ///
    /// 2026-06-28-005: stub kept so callers that previously
    /// consulted `bus.has_human_pending()` still compile while the
    /// `human.guidance` topic and its dedicated `human_pending`
    /// queue are removed together. Always returns `false` now —
    /// the queue is gone, so the question is no longer meaningful.
    pub fn has_pending_human_events(&self) -> bool {
        false
    }

    /// Returns whether unread JSONL events include any semantic `plan.*` topics.
    ///
    /// This allows callers to dispatch `pre.plan.created` hooks before
    /// event publication handling without consuming unread events.
    pub fn has_pending_plan_events_in_jsonl(&self) -> std::io::Result<bool> {
        let result = self.event_reader.peek_new_events()?;
        Ok(result
            .events
            .iter()
            .any(|event| event.topic.starts_with("plan.")))
    }

    /// Gets the topics a hat is allowed to publish.
    ///
    /// Used to build retry prompts when the LLM forgets to publish an event.
    pub fn get_hat_publishes(&self, hat_id: &HatId) -> Vec<String> {
        self.registry
            .get(hat_id)
            .map(|hat| hat.publishes.iter().map(|t| t.to_string()).collect())
            .unwrap_or_default()
    }

    /// U2 (2026-06-17-003 plan): mechanism-emitted `plan.blocked`
    /// for a review wave that has stalled below `wave_total` past
    /// `0.8 * aggregate_timeout_secs` without further
    /// `dimension.done` progress.
    ///
    /// The hat provenance is `review-synthesizer` (so the event
    /// passes the isolated-scope publish allowlist check); the
    /// target is `reporter` (was `shipper` per plan 2026-07-24-005 U1
    /// — `plan-gate.triggers` does NOT include `plan.blocked`, so
    /// routing through plan-gate would silently drop the event.
    /// The wave is then closed in the tracker (`open_wave_id =
    /// None`) so the gate does not re-fire on the next
    /// iteration.
    ///
    /// Called once per iteration inside [`Self::process_output`],
    /// after handoff escalations and before new JSONL events are
    /// processed. This matches the plan §U2 fixed order:
    /// incomplete-wave gate → handoff-expired → process JSONL →
    /// policy validation. It is also invoked from the stall ladder
    /// in [`Self::inject_fallback_event`] as a hard-escalation
    /// fallback. When this method emits a `plan.blocked`, the U4
    /// aggregate-timeout path is not consulted in the same iteration.
    ///
    /// Returns `true` if a `plan.blocked` was emitted.
    pub fn maybe_emit_incomplete_wave_blocked(&mut self) -> bool {
        use crate::flow_lifecycle::incomplete_wave_gate::{
            IncompleteWaveGate, IncompleteWaveGateConfig,
        };

        // Plan §U2: global default off, `ce-executor-serial`
        // default on. The config key is
        // `workflow_contract.incomplete_wave_gate.enabled`. We
        // read it defensively — when the preset does not set it,
        // the helper returns None and the gate stays disabled.
        let enabled = self
            .config
            .event_loop
            .workflow_contract
            .as_ref()
            .map(|wc| wc.incomplete_wave_gate.enabled)
            .unwrap_or(false);
        if !enabled {
            return false;
        }

        // Compute staleness from the configured `review-synthesizer`
        // aggregate.timeout — matches what U4
        // `inject_review_aggregate_timeouts` reads.
        let aggregate_timeout_secs = self
            .registry
            .get_config(&HatId::new("review-synthesizer"))
            .and_then(|cfg| cfg.aggregate.as_ref())
            .map(|agg| u64::from(agg.timeout))
            .unwrap_or(300);
        let gate = IncompleteWaveGate::new(IncompleteWaveGateConfig {
            enabled: true,
            staleness_ratio: 0.8,
        });
        let staleness_secs = gate.staleness_secs(aggregate_timeout_secs);
        let candidates = self
            .state
            .review_step_tracker
            .open_waves_needing_intervention(staleness_secs);
        if candidates.is_empty() {
            return false;
        }
        // Use the first candidate per iteration — closing its
        // wave before the next call prevents re-emit. The next
        // iteration will pick up any remaining stalled waves.
        let info = candidates.into_iter().next().unwrap();

        let last_dim_secs_ago = info.last_dimension_at.map(|t| t.elapsed().as_secs());
        let payload = gate.evaluate(
            &self.state.flow_lifecycle,
            aggregate_timeout_secs,
            &info.wave_id,
            info.expected,
            info.received,
            last_dim_secs_ago,
        );
        let Some(mut payload) = payload else {
            return false;
        };
        // Fill in the per-step correlation fields from the
        // tracker observation.
        payload.plan_name = info.plan_name;
        payload.task_id = info.task_id;
        payload.step = info.step;

        // The plan requires the publish provenance be
        // `review-synthesizer` (which has `plan.blocked` in its
        // `publishes` allowlist per the preset validator). The
        // `Event::with_source(...)` helper stamps the producer
        // hat; `with_target(...)` routes to `reporter`.
        //
        // 2026-07-24-005 plan U1: target was `shipper`; the
        // shipper hat is removed from the supervisor preset —
        // `reporter` is the canonical `plan.blocked` terminal
        // owner.
        let json_payload = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let event = Event::new("plan.blocked", json_payload)
            .with_source(HatId::new("review-synthesizer"))
            .with_target(HatId::new("reporter"));
        debug!(
            wave_id = %info.wave_id,
            expected = info.expected,
            received = info.received,
            "U2: emitting mechanism-level plan.blocked (dimension_reviewers_failed_to_converge)"
        );
        self.bus.publish(event);

        // Close the wave in the tracker so the gate does not
        // re-fire on subsequent iterations. We do not change
        // `synth_terminal` / `synth_pass` here — the closed
        // wave has a `plan.blocked` outcome, not a `review.passed`
        // verdict, so plan-gate must not see it as terminal.
        let key = review_step_state::StepKey {
            plan_name: payload.plan_name.clone(),
            task_id: payload.task_id.clone(),
            step: payload.step.clone(),
        };
        self.state.review_step_tracker.close_wave(&key);
        true
    }

    /// U4: When a review wave is incomplete past the synthesizer aggregate window,
    /// route `review-synthesizer` via `task.resume` so the loop can emit
    /// `plan.blocked` instead of stalling indefinitely.
    pub fn inject_review_aggregate_timeouts(&mut self) -> bool {
        use std::time::Duration;

        let timeout_secs = self
            .registry
            .get_config(&HatId::new("review-synthesizer"))
            .and_then(|cfg| cfg.aggregate.as_ref())
            .map(|agg| u64::from(agg.timeout))
            .unwrap_or(300);
        let timeout = Duration::from_secs(timeout_secs);

        let actions = self
            .state
            .review_step_tracker
            .drain_expired_aggregate_timeouts(timeout);
        let Some(action) = actions.into_iter().next() else {
            return false;
        };

        let free_form = format!(
            "RECOVERY (AGGREGATE TIMEOUT): review wave '{}' received {}/{} \
             review.dimension.done events within {}s. Activate review-synthesizer and emit \
             review.passed with skip_reason=aggregate_timeout (or review.failed if verdict \
             is fail). Do NOT emit plan.complete or queue.advance until synthesizer terminal.\n\
             plan_name={} task_id={} step={} wave_id={}",
            action.wave_id,
            action.received,
            action.expected,
            timeout_secs,
            action.plan_name,
            action.task_id,
            action.step,
            action.wave_id,
        );
        let target = HatId::new("review-synthesizer");
        // U2 (2026-06-17-003 plan): wrap the free-form message in
        // a JSON object carrying the schema-required `reason` and
        // `target_hat` fields.
        let payload = enrich_task_resume_payload(
            &free_form,
            "aggregate_timeout",
            Some(target.as_str()),
            Some(RejectionKind::ContractViolation),
        );
        debug!(
            wave_id = %action.wave_id,
            received = action.received,
            expected = action.expected,
            "Injecting aggregate timeout recovery to review-synthesizer"
        );
        // R1 (2026-06-14-003 plan): pin the wave_id so the next
        // `build_prompt` for `review-synthesizer` injects
        // `AGGREGATE_TIMEOUT: true` in the `## WAVE CONTEXT` block.
        // The pin is consumed (`.take()`) on first read — the
        // aggregate-timeout signal does not leak across waves.
        // See `LoopState::pending_synthesizer_timeout` for the
        // full rationale.
        self.state.pending_synthesizer_timeout = Some(action.wave_id.clone());

        // Unit 7 (2026-06-17-001): wave merge complete — register handoff
        // obligation for the synthesizer so HandoffTracker can detect if it
        // fails to activate within the configured aggregate timeout.
        let handoff_event_id = format!("sla:review.dimension.done:{}", action.wave_id);
        self.state.handoff_tracker.on_handoff_accepted(
            "review.dimension.done",
            "review-synthesizer",
            handoff_event_id,
            std::time::Instant::now(),
        );

        // Plan 2026-08-10-001 U1: route the aggregate-timeout
        // recovery through the unified publisher so the dedup /
        // fail-close checks fire. The target is hard-coded to
        // `review-synthesizer` per the pre-existing ladder; the
        // `retry_key` is signed by the wave_id so multiple
        // aggregate-timeout recoveries collapse into one resume
        // per wave.
        let loop_id_for_resume = self.current_loop_id();
        let decision = crate::event_loop::resume_routing::publish_targeted_resume_for_hat(
            &mut self.bus,
            &self.registry,
            None,
            loop_id_for_resume.as_deref(),
            target.as_str(),
            None,
            None,
            None,
            &format!("aggregate_timeout:{}", action.wave_id),
            payload,
        );
        if let crate::event_loop::resume_routing::ResumeDecision::Block { reason } = &decision {
            tracing::warn!(
                target = %target.as_str(),
                wave_id = %action.wave_id,
                ?reason,
                "aggregate-timeout recovery blocked (no safe target)"
            );
        }
        true
    }
}

/// Plan 2026-08-13-003 U4: helpers for `build_resume_context_from_sources`.
/// All helpers are read-only; missing files or unreadable paths return
/// `None` / `0` so the loop.resume event still fires for legacy
/// workspaces that lack the auxiliary files.

fn read_closed_tasks_count(tasks_path: &std::path::Path) -> u32 {
    use crate::task_store::TaskStore;
    TaskStore::load(tasks_path)
        .map(|store| {
            store
                .tasks()
                .iter()
                .filter(|t| t.status.is_terminal())
                .count() as u32
        })
        .unwrap_or(0)
}

fn first_meaningful_heading(content: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            return rest.trim().to_string();
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            return rest.trim().to_string();
        }
        if !trimmed.is_empty() {
            // First non-heading non-empty line — use it as
            // the headline for very loose scratchpads.
            return trimmed.chars().take(80).collect();
        }
    }
    String::new()
}
