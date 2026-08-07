//! EventLoop implementation region 2.

use super::*;

impl EventLoop {
    /// 2026-07-07-002 U4 + 2026-07-07-003 fix: terminal-closed guard using
    /// Unit 3 pure decision. The post-completion *business* freeze is now
    /// policy-aware: only `Reject` freezes at the guard. `Warn` / `Ignore`
    /// fall through to the downstream `check_completion_guard` so the
    /// existing policy path publishes the configured warning or
    /// ignore-with-diagnostic. Without an enabled `event_policy`, the
    /// guard keeps the conservative 2026-07-01 freeze (default `Reject`).
    pub(super) fn evaluate_terminal_closed_for_event(
        &mut self,
        topic: &str,
        payload: &str,
        completion_topic: &str,
    ) -> crate::event_loop::terminal_closed_guard::TerminalClosedDecision {
        use crate::config::CompletionAfterTerminalAction;
        use crate::event_loop::terminal_closed_guard::{
            TerminalClosedDecision, TerminalClosedInput, classify_topic, evaluate_terminal_closed,
        };
        if !self.state.completion_honored {
            return TerminalClosedDecision::Allow;
        }
        let proto = Event::new(topic, payload);
        let is_byte_duplicate = self.state.is_review_complete_duplicate(&proto);
        let business_action = self
            .config
            .event_loop
            .event_policy
            .as_ref()
            .filter(|p| p.enabled)
            .map(|p| {
                p.completion_after_terminal
                    .business_after_completion
                    .clone()
            })
            .unwrap_or(CompletionAfterTerminalAction::Reject);
        let input = TerminalClosedInput {
            completion_honored: true,
            topic,
            topic_class: classify_topic(topic),
            is_completion_promise: topic == completion_topic,
            is_byte_duplicate,
            business_after_completion: business_action,
        };
        evaluate_terminal_closed(&input)
    }

    pub(super) fn publish_post_terminal_rejection(&mut self, topic: &str, reason: &str) {
        self.bus.publish(Event::new(
            "event.post_terminal.rejected",
            format!(
                "{{\"rejected_topic\":\"{topic}\",\"reason\":\"{reason}\",\"completion_honored\":true}}"
            ),
        ));
    }

    /// Returns the scratchpad path based on loop context and active scratchpad config.
    ///
    /// When a per-hat scratchpad override is active (path differs from global default),
    /// the custom path is resolved relative to the loop context workspace for worktree
    /// isolation. When using the default/global path, loop context's standard resolution
    /// applies.
    pub(super) fn scratchpad_path(&self) -> PathBuf {
        let active_path = &self.ralph.active_scratchpad().path;

        match self.loop_context.as_ref() {
            Some(ctx) => ctx.workspace().join(active_path),
            None => PathBuf::from(active_path),
        }
    }

    /// Returns the global scratchpad path (ignoring per-hat overrides).
    /// Used for guidance persistence which is cross-hat state.
    pub(super) fn global_scratchpad_path(&self) -> PathBuf {
        self.loop_context
            .as_ref()
            .map(|ctx| ctx.scratchpad_path())
            .unwrap_or_else(|| PathBuf::from(&self.config.core.scratchpad.path))
    }

    /// Returns the current loop state.
    pub fn state(&self) -> &LoopState {
        &self.state
    }

    /// Returns a mutable reference to the loop state.  Used by the U2
    /// targeted-retry machinery in the loop runner to record
    /// per-rejection-key retry counts against the bounded budget
    /// without having to take a `&mut self` on the whole `EventLoop`
    /// in every helper.
    pub fn state_mut(&mut self) -> &mut LoopState {
        &mut self.state
    }

    /// 2026-06-30-001 P0-3 (U3 runtime guard): returns
    /// `true` when the event payload is a `work.done`
    /// whose `task_key` is a fix-unit shape
    /// (`<plan>:step:fix-NN:u{N}`). The check tolerates
    /// the legacy "YAML-style" payload format used by
    /// BDD harness mocks (e.g.
    /// `task_key: "ce-executor:p:fix-01:u1"`) by
    /// looking for the marker in both structured JSON
    /// and the raw text. Production payloads are
    /// structured JSON; BDD mocks and ad-hoc emit
    /// patterns are loose text.
    pub(super) fn is_fix_unit_completion_event(&self, event: &Event) -> bool {
        if event.topic.as_str() != "work.done" {
            return false;
        }
        if event.payload.is_empty() {
            return false;
        }
        // Try structured JSON first (production path).
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&event.payload)
            && let Some(key) = value.get("task_key").and_then(|v| v.as_str())
            && crate::task_store::is_fix_unit_key(key)
        {
            return true;
        }
        // Fallback: scan the raw text for the fix-unit
        // marker. The marker is `task_key:` followed
        // by a quoted string containing `fix-` and
        // digits — distinctive enough that a substring
        // match is safe.
        let lower = event.payload.to_ascii_lowercase();
        lower.contains("task_key:") && lower.contains("fix-")
    }

    /// 2026-06-30-001 P0-3 (U3 runtime guard): returns
    /// `true` when every fix-unit task in the current
    /// plan's `tasks.jsonl` is `Closed` (or `Failed`).
    /// This is the structural signal that the fix-unit
    /// ladder is exhausted and the next event from
    /// coordinator must be `plan.complete`, NOT
    /// `review.start`.
    pub(super) fn is_fix_unit_chain_exhausted(&self) -> bool {
        use crate::task_store::TaskStore;
        // Resolve the tasks path through the loop
        // context (the only place the workspace
        // configuration is held on `EventLoop`).
        let Some(loop_ctx) = self.loop_context.as_ref() else {
            return false;
        };
        let tasks_path = loop_ctx.tasks_path();
        let Ok(store) = TaskStore::load(&tasks_path) else {
            return false;
        };
        let mut has_any_fix_unit = false;
        for task in store.all() {
            // The store's stable key encodes the
            // step prefix; only fix-unit tasks
            // participate in the chain-exhausted check.
            let Some(key) = task.key.as_deref() else {
                continue;
            };
            if !crate::task_store::is_fix_unit_key(key) {
                continue;
            }
            has_any_fix_unit = true;
            if !task.status.is_terminal() {
                return false;
            }
        }
        // No fix-unit tasks at all → chain is trivially
        // exhausted (the loop has no fix-units, so
        // "review.start after every fix-NN is closed"
        // is vacuously true). The runtime guard is
        // still safe: it only rejects `review.start`
        // that arrives AFTER a fix-unit chain was
        // expected to be done.
        has_any_fix_unit
    }

    /// Test-only: set the current iteration directly. Production code
    /// should never call this; the iteration is normally advanced by
    /// the main loop. Exposed at the `pub` level so external
    /// integration tests (e.g. `ralph-cli/loop_runner/tests.rs`) can
    /// pin the iteration value the recovery / gate code reads.
    pub fn set_iteration_for_test(&mut self, n: u32) {
        self.state.iteration = n;
    }

    /// Returns the diagnostics collector used by this event loop.
    ///
    /// Callers outside the event loop (e.g. the CLI loop runner) can use
    /// this to log structured diagnostics events through the standard
    /// `DiagnosticsCollector` API rather than hand-rolling file writes.
    pub fn diagnostics(&self) -> &crate::diagnostics::DiagnosticsCollector {
        &self.diagnostics
    }

    /// U8 (2026-06-27 mechanism foundation): accessor for the
    /// loop-scoped idempotent log. Wiring paths in
    /// `task_store::save_with_idempotent_log`,
    /// `drift::engine::drain_observer`,
    /// `drift::engine::check_recovery_for_iteration`, and
    /// `DiagnosticsCollector::log_*_via_idempotent` lock this
    /// mutex before calling `IdempotentLog::append`. A disabled
    /// log (constructed when the workspace was not writable at
    /// startup) makes every write path short-circuit, so the
    /// caller's expected type is always `&Mutex<IdempotentLog>`
    /// regardless of whether the operator opted into
    /// `mechanism.state_idempotency: required`.
    pub fn idempotent_log(&self) -> &std::sync::Mutex<crate::state::idempotent_log::IdempotentLog> {
        &self.idempotent_log
    }

    /// Returns a reference to the activation lifecycle tracker.
    ///
    /// This is the **read API** consumed by the `ralph diagnose` reporter (U4).
    /// Event loop decision paths must NOT call this — they only use write APIs
    /// (`activate`, `observe_accepted_event`, `complete`) to avoid implicit
    /// feedback loops.
    pub fn hat_lifecycle_tracker(&self) -> &ActivationLifecycleTracker<SystemTimeClock> {
        &self.hat_lifecycle_tracker
    }

    /// Test-only: returns a mutable reference to the activation lifecycle
    /// tracker so external integration tests can drive `activate` /
    /// `complete` through the public API. Production code paths
    /// (`build_prompt`, `process_events_from_jsonl`) access the field
    /// directly — this helper exists so the test boundary does not
    /// require `pub(crate)` on the field.
    #[cfg(test)]
    pub fn hat_lifecycle_tracker_mut(
        &mut self,
    ) -> &mut ActivationLifecycleTracker<SystemTimeClock> {
        &mut self.hat_lifecycle_tracker
    }

    /// Resets the stale-loop topic counter.
    ///
    /// Call after processing wave results — multiple events with the same topic
    /// (e.g. `review.done` from parallel workers) are expected and should not
    /// trigger the stale loop detector.
    pub fn reset_stale_topic_counter(&mut self) {
        self.state.consecutive_same_signature = 0;
        self.state.last_emitted_signature = None;
    }

    /// Increment the hard-gate counter when an agent claims emit but writes no event.
    pub fn increment_hard_gate_count(&mut self) {
        self.state.consecutive_hard_gates += 1;
    }

    /// Unit 3 (2026-06-16-002 plan): `true` while the loop is
    /// still in the bootstrap window — i.e. between the
    /// `work.start` publication and the first legal
    /// `coordinator work.ready` (without `reviewed_task_id`).
    ///
    /// During this window the `build_prompt` paths skip
    /// injecting `human.guidance` into the coordinator's
    /// prompt so the coordinator's first action is not
    /// derailed by stale human input.  Once
    /// `bootstrap_complete` flips to `true`, the gate opens
    /// and guidance flows normally.
    pub fn in_bootstrap_phase(&self) -> bool {
        !self.state.bootstrap_complete && !self.state.bootstrap_failed
    }

    /// 2026-06-28-005: stub kept so the three call sites
    /// inside `update_robot_guidance` / `apply_robot_guidance` /
    /// `prepend_scratchpad` still compile while those
    /// robot-guidance helpers are scheduled for deletion in a
    /// follow-up phase. The `suppress_human_guidance` config
    /// field was removed in this same phase (it gates nothing
    /// now that the `human.guidance` topic is gone), so this
    /// helper always returns `false`.
    pub fn human_guidance_suppressed(&self) -> bool {
        false
    }

    /// Unit 3 (2026-06-16-002 plan): `true` when `hat_id ==
    /// "coordinator"` AND the loop is still in the bootstrap
    /// window.  The gate only applies to the `coordinator`
    /// hat (not the `ralph` solo hat and not the
    /// `review-synthesizer` / `executor` / other downstream
    /// hats).  When the gate is closed, the build_prompt
    /// paths must skip:
    ///   - `update_robot_guidance` (no `human.guidance`
    ///     caching for the prompt)
    ///   - `apply_robot_guidance` (no `ralph.robot_guidance`
    ///     push)
    ///   - `collect_robot_guidance` (isolated-path
    ///     `## ROBOT GUIDANCE` block)
    ///   - scratchpad `### HUMAN GUIDANCE` block inclusion
    ///     (handled in `prepend_scratchpad`).
    pub fn coordinator_bootstrap_gate_closed(&self, hat_id: &HatId) -> bool {
        hat_id.as_str() == "coordinator" && self.in_bootstrap_phase()
    }

    /// Reset the hard-gate counter when an agent successfully emits an event.
    pub fn reset_hard_gate_count(&mut self) {
        self.state.consecutive_hard_gates = 0;
    }

    /// Records the git HEAD SHA at loop start so execution-contract validation
    /// can detect commits produced during this loop.
    ///
    /// `None` clears the recorded SHA and falls back to diff-only evidence.
    /// Pass the value returned by `ralph_core::get_head_sha` from the loop
    /// runner at startup; pass `None` when the workspace is not a git repo
    /// or the SHA could not be resolved.
    pub fn set_loop_start_sha(&mut self, sha: Option<String>) {
        self.state.loop_start_sha = sha;
    }

    /// Set the persisted plan baseline SHA.
    ///
    /// This is the git HEAD at plan start. It is injected into the
    /// `## ORCHESTRATOR CONTEXT` block so plan-driven presets can scope
    /// review diffs from the plan's origin rather than from an arbitrary
    /// rerun.
    pub fn set_plan_baseline_sha(&mut self, sha: Option<String>) {
        self.state.plan_baseline_sha = sha;
    }

    /// Maximum consecutive hard-gate triggers before the loop terminates.
    pub const HARD_GATE_MAX: u32 = 3;
}
