//! EventLoop implementation region 7.

use super::*;

impl EventLoop {
    /// Builds the Ralph prompt (coordination mode).
    pub fn build_ralph_prompt(&self, prompt_content: &str) -> String {
        self.ralph.build_prompt(prompt_content, &[], &[])
    }

    /// R1 (2026-06-14-003 plan): resolve the current wave context for
    /// the `review-synthesizer` aggregate hat.  Returns `None` when no
    /// relevant wave events are present so the caller can fall back to
    /// the pre-R1 behaviour (synthesizer activates without wave
    /// metadata — typical for non-wave presets).
    ///
    /// `pending_synthesizer_timeout` is `true` when the synthesizer was
    /// woken up by `inject_review_aggregate_timeouts`.  The field is
    /// **consumed (taken)** on this call so the AGGREGATE_TIMEOUT
    /// signal does not leak across waves: a wave-1 timeout must not
    /// mark wave-2's synthesizer activation as timed-out.  This
    /// matches the calm-oak failure mode the plan §5.1.4 calls out
    /// (the original loop saw stale wave context across waves).
    pub fn build_wave_context_for_synthesizer(
        &mut self,
    ) -> Option<crate::wave_context::WaveContext> {
        let events_path = self.events_path_for_wave_context()?;
        let aggregate_timeout = self.state.pending_synthesizer_timeout.take().is_some();
        crate::wave_context::resolve_wave_context_for_synthesizer_with_aggregate_timeout(
            &events_path,
            2000,
            aggregate_timeout,
        )
    }

    /// R1: best-effort events file path lookup for the wave context
    /// resolver.  Returns `None` when no loop context is attached
    /// (CLI helpers that build prompts out of band) — the resolver
    /// then no-ops and the caller falls back to the legacy prompt.
    pub(super) fn events_path_for_wave_context(&self) -> Option<std::path::PathBuf> {
        self.loop_context.as_ref().map(|ctx| ctx.events_path())
    }

    /// R1: render the `## WAVE CONTEXT` block for the given hat and
    /// prepend it to the prompt.  For hats other than
    /// `review-synthesizer` this is a no-op — the wave context is only
    /// meaningful for the synthesizer aggregate.
    pub(super) fn prepend_wave_context(&mut self, prompt: String, hat_id: &HatId) -> String {
        let Some(ctx) = self.build_wave_context_for_synthesizer_if_match(hat_id) else {
            return prompt;
        };
        format!("{}{prompt}", ctx.to_prompt_block())
    }

    /// 2026-06-18-001 plan U6: prepend `## RECENT REJECTIONS` 块。
    /// 复用 `LoopState::format_rejection_digest_block`,空 digest
    /// 时返回空字符串,no-op 行为。
    pub(super) fn prepend_rejection_digest(&self, prompt: String) -> String {
        let block = self.state.format_rejection_digest_block();
        if block.is_empty() {
            prompt
        } else {
            format!("{block}\n{prompt}")
        }
    }

    /// U7a (plan 2026-06-21-002): prepend the
    /// `## ORCHESTRATOR CORRECTION` block (when
    /// `state.prompt_context.correction_blocks` is non-empty)
    /// and the `## LOOP RESUME CONTEXT` block (when
    /// `state.prompt_context.resume_blocks` is non-empty).  The
    /// resume block is also consumed here (`Option::take`-style
    /// via [`std::mem::take`]) so it appears in exactly one
    /// prompt — the first prompt after `--continue`.
    ///
    /// U1 (2026-08-06-001 D9): correction entries are now
    /// partitioned by `target_hat`.  Only entries with
    /// `target_hat ∈ {None, current_hat_id}` are rendered AND
    /// consumed; entries targeted at another hat stay queued so
    /// the unrelated hat cannot accidentally swallow them
    /// (F-A fix).  Entries with `target_hat = None` retain the
    /// legacy "visible to every hat" fallback (used for
    /// diagnosis-fallback corrections).
    pub(super) fn prepend_correction_and_resume(
        &mut self,
        prompt: String,
        current_hat_id: &HatId,
    ) -> String {
        // Take the resume block out — the first prompt after
        // resume must carry `## LOOP RESUME CONTEXT`, but a
        // subsequent prompt must not (the user already saw the
        // block; showing it again would be confusing).
        let resume_blocks = std::mem::take(&mut self.state.prompt_context.resume_blocks);
        let mut pc = std::mem::take(&mut self.state.prompt_context);
        pc.resume_blocks = resume_blocks;
        // U1 D9 partition: drain only the entries visible to
        // `current_hat_id`.  Other entries (different target hat)
        // stay in `state.prompt_context` and will be rendered
        // when their target hat next builds a prompt.  The
        // consumed entries are rendered into the prompt here so
        // we do not double-render after re-installing `pc`.
        let consumed = pc.take_visible_corrections(current_hat_id.as_str());
        let resume_block = pc.render_resume_block();
        let correction_block = render_correction_entries(&consumed);
        let block = {
            let mut s = String::new();
            if !correction_block.is_empty() {
                s.push_str(&correction_block);
                s.push('\n');
            }
            if !resume_block.is_empty() {
                s.push_str(&resume_block);
                s.push('\n');
            }
            s
        };
        // Re-install the remaining prompt_context (resume_blocks
        // preserved; correction_blocks already partitioned).
        self.state.prompt_context = pc;
        if block.is_empty() {
            prompt
        } else {
            format!("{block}{prompt}")
        }
    }

    /// 2026-06-17-003 U4: prepend the `## ORCHESTRATOR CONTEXT`
    /// block. Reads the projector's in-memory cache when state
    /// projection is enabled; falls back to a disabled-stub
    /// explanation otherwise (so the agent still sees the
    /// heading and knows the orchestrator owns the ledgers).
    ///
    /// Phase 1 scope (R5 in 2026-06-17-005 fix plan): only the
    /// `isolated` build_prompt path calls this helper. The
    /// `HatlessRalph` (solo / multi-hat coordinator) and the
    /// backward-compat custom-hat paths skip injection — they
    /// build their prompts through a different pipeline that
    /// does not own a `StateProjector`. Widening the scope to
    /// those paths is deferred to Phase 2.
    /// OPAC U2: prepend the `## HAT IDENTITY` block to the prompt so
    /// the agent sees its authoritative identity and permission list
    /// (derived from the resolved `RalphConfig`) before any other
    /// injected context. Mirrors [`HatIdentitySnapshot::to_prompt_block`].
    ///
    /// The block is rendered only for hats that exist in the resolved
    /// config (so a stale `ralph run` against an outdated preset does
    /// not crash on an unknown hat id) and is skipped for the `ralph`
    /// orchestrator sentinel — the prompt there is framework-driven
    /// and never needs an explicit identity header. The placement is
    /// deliberately *above* `## ORCHESTRATOR CONTEXT` so the agent
    /// sees "who you are" before "what the loop is doing" (KTD-5).
    pub fn prepend_hat_identity(&self, prompt: String, hat_id: &HatId) -> String {
        if hat_id.as_str() == "ralph" {
            return prompt;
        }
        // U4 (plan 2026-07-30-004): prefer the contract-projected
        // snapshot so the prompt and runtime enforcement are provably
        // in sync. Fall back to raw config when the contract is not
        // available (legacy / test constructors).
        let snapshot = match &self.execution_contract {
            Some(contract) => crate::hat_identity::HatIdentitySnapshot::from_config_and_contract(
                &self.config,
                hat_id,
                contract,
            ),
            None => crate::hat_identity::HatIdentitySnapshot::from_config(&self.config, hat_id),
        };
        let Some(snapshot) = snapshot else {
            tracing::debug!(
                hat_id = %hat_id.as_str(),
                "OPAC U2: skipping ## HAT IDENTITY injection for unknown hat"
            );
            return prompt;
        };
        let hat_block = snapshot.to_prompt_block(&self.config);
        format!("{}{prompt}", hat_block)
    }

    pub(super) fn prepend_orchestrator_context(&self, prompt: String, hat_id: &HatId) -> String {
        // The `ralph` / orchestrator itself and short-lived
        // control hats do not need the context; the prompt is
        // already covered by the framework's own message.
        if hat_id.as_str() == "ralph" {
            return prompt;
        }
        let mut snap = if let Some(p) = self.state.state_projection.as_ref() {
            crate::runtime_state::RuntimeStateSnapshot::build(p)
        } else {
            crate::runtime_state::RuntimeStateSnapshot::disabled_stub()
        };
        // Inject git baseline SHAs from loop state. These are recorded by
        // the runner at loop start and are not part of the state projector's
        // ledgers.
        snap.loop_start_sha = self.state.loop_start_sha.clone();
        // B-layer reconciliation (2026-07-05 plan): prefer the SHA on disk
        // (`.ralph/agent/plan-baseline-{key}.sha`) over the in-memory
        // LoopState copy, which is stale when plan-reviewer's
        // §Step 2.5b reconciliation rewrites the file mid-run. Falls
        // back to LoopState on missing/unreadable files.
        snap.plan_baseline_sha = self.resolve_reconciled_plan_baseline_sha();
        format!("{}{prompt}", snap.to_prompt_block())
    }

    /// Read the latest plan baseline SHA from disk on every hat prompt.
    /// The reader is intentionally read-only and ignores errors: the
    /// caller keeps the LoopState fallback when disk is unavailable,
    /// the derivation key cannot be computed, or `loop_context` was
    /// not provided (e.g. unit tests using `EventLoop::new` directly).
    pub(super) fn resolve_reconciled_plan_baseline_sha(&self) -> Option<String> {
        use crate::plan_baseline::{derive_baseline_key, read_plan_baseline};
        if let Some(ctx) = self.loop_context.as_ref() {
            let plan_key = derive_baseline_key(
                &self.config.event_loop.prompt_file,
                None,
                self.config.event_loop.prompt.as_deref(),
                Some(ctx.workspace()),
            );
            if let Some(key) = plan_key.as_deref()
                && let Some(sha) = read_plan_baseline(ctx.workspace(), Some(key))
            {
                return Some(sha);
            }
        }
        self.state.plan_baseline_sha.clone()
    }

    /// R3 (2026-06-14-003 plan): invoke the ephemeral isolation engine
    /// when the preset opts in.  The records are stored on
    /// `LoopState.last_ephemeral_relocations` and consumed by the
    /// next `build_prompt` call.  Best-effort: a git failure or
    /// missing workspace never aborts the loop.
    pub(crate) fn run_ephemeral_isolation(&mut self) {
        if !self.config.event_loop.ephemeral_isolation {
            return;
        }
        if self.config.event_loop.execution_mode != crate::config::HatExecutionMode::Isolated {
            return;
        }
        let workspace: std::path::PathBuf =
            if self.config.core.workspace_root.as_os_str().is_empty() {
                self.loop_context
                    .as_ref()
                    .map(|c| c.workspace().to_path_buf())
                    .unwrap_or_default()
            } else {
                self.config.core.workspace_root.clone()
            };
        if workspace.as_os_str().is_empty() {
            return;
        }
        let loop_id = self
            .loop_context
            .as_ref()
            .and_then(|c| c.loop_id().map(str::to_string));
        let records = self
            .ephemeral_isolation
            .scan_and_relocate(&workspace, loop_id.as_deref());
        if records.is_empty() {
            return;
        }
        tracing::info!(
            count = records.len(),
            workspace = %workspace.display(),
            "ephemeral_isolation: relocated runtime artefacts to .ralph/agent/"
        );
        self.state.last_ephemeral_relocations = records;
    }

    /// R3: render the `## EPHEMERAL RELOCATED` block for the prompt
    /// when the most recent `process_output` produced relocation
    /// records.  Empty / missing records short-circuit to a no-op so
    /// the prepend pipeline stays cheap.  Records are consumed (taken)
    /// on read so the block does not re-appear in subsequent
    /// iterations.
    pub(crate) fn prepend_ephemeral_relocations(&mut self, prompt: String) -> String {
        if self.state.last_ephemeral_relocations.is_empty() {
            return prompt;
        }
        let records = std::mem::take(&mut self.state.last_ephemeral_relocations);
        let mut section = String::from(
            "## EPHEMERAL RELOCATED\n\
             The following runtime artefacts were moved out of the source tree by the runner. \
             Do NOT recreate these files inside the source tree; write runtime notes to \
             `.ralph/agent/` instead.\n\n",
        );
        for rec in &records {
            section.push_str(&format!(
                "- `{}` -> `{}` ({} bytes appended)\n",
                rec.from, rec.to, rec.size_bytes
            ));
        }
        section.push('\n');
        format!("{section}{prompt}")
    }

    /// U4b (plan 2026-06-20-001, R12 / R13 / KTD-8): inject the
    /// lint failure hint as `## LINT MIRROR` + `## LINT RESUME
    /// REQUIRED` at the head of `prompt`.  The hint is consumed
    /// on first read (`Option::take`) so a stale resume does not
    /// leak across prompts.
    ///
    /// The block is prepended (above the rest of the prompt) so
    /// the agent sees the protocol hash + failing topic first —
    /// matching the order in the CLI emit failure output so the
    /// two paths produce the same canonical block (R12).
    ///
    /// In multi-hat / isolated modes the hint is only injected
    /// when the active hat matches `hint.target` — otherwise
    /// the resume belongs to a *different* hat and the current
    /// hat has nothing to fix. Solo / coordinator modes always
    /// inject because `hat_id` is `"ralph"` (the orchestrator
    /// itself, which sees every hat's alerts).
    pub(super) fn prepend_macro_next_hint(
        &self,
        prompt: String,
        regular_events: &[ralph_proto::Event],
        hat_id: &HatId,
    ) -> String {
        // U18 (P2): macro edge next hint. The flag defaults to disabled;
        // when off we are a no-op so existing loops are unaffected.
        let flag = self.config.event_loop.macro_edge_next_hint.enabled;
        if !flag {
            return prompt;
        }

        // Only the dispatcher hat (the one that received the macro
        // edge event) sees the hint; coordinators do not need it
        // because the runtime already routes them.
        if hat_id.as_str() == "ralph" {
            return prompt;
        }

        // Find the most recent accepted business event whose payload
        // carries a `next_hint` string. We scan backwards so the
        // latest hint wins (older hints are stale).
        let mut hint: Option<String> = None;
        for ev in regular_events.iter().rev() {
            let payload_str = ev.payload.clone();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload_str)
                && let Some(s) = val.get("next_hint").and_then(|v| v.as_str())
            {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    // Cap at 120 chars (U18 contract). Truncate at
                    // a char boundary so multi-byte codepoints are
                    // not sliced.
                    let cap = trimmed.chars().take(120).collect::<String>();
                    hint = Some(cap);
                    break;
                }
            }
        }

        let Some(hint) = hint else { return prompt };
        format!("## NEXT ACTION\n\n{hint}\n\n---\n\n{prompt}")
    }

    pub(super) fn inject_pending_lint_resume(&mut self, prompt: String, hat_id: &HatId) -> String {
        let Some(hint) = self.state.pending_lint_resume.take() else {
            return prompt;
        };
        // Route check: in multi-hat / isolated mode, only inject
        // when the current hat is the lint target.
        if self.config.event_loop.execution_mode != HatExecutionMode::Coordinator
            && hat_id.as_str() != "ralph"
        {
            // Map `LintResumeTarget` -> owning hat name. The
            // hint class already classifies into source hat /
            // plan-gate; we use the canonical hat ids here. The
            // mapping is identical to KTD-4 / hint.rs.
            let target_hat = match hint.target {
                LintResumeTarget::SourceHat => {
                    // The lint failure came from THIS hat (the
                    // one currently building the prompt). SourceHat
                    // means "the hat that emitted the rejected
                    // event"; in single-hat mode that is the
                    // active hat. In multi-hat mode the source
                    // hat is identified by the topic itself; the
                    // resume hint carries the failing topic and
                    // the active hat should be the one that
                    // emits it. We accept the hint when the
                    // current hat's `publishes` list contains
                    // the failing topic — otherwise the resume
                    // belongs to a different hat.
                    self.registry
                        .get_config(hat_id)
                        .map(|cfg| cfg.publishes.iter().any(|t| t == hint.topic.as_str()))
                        .unwrap_or(false)
                }
                LintResumeTarget::PlanGate => {
                    hat_id.as_str() == "plan-gate"
                        || hat_id.as_str() == "ralph"
                        || hat_id.as_str() == "coordinator"
                }
            };
            if !target_hat {
                // Not for this hat — restore the hint so the
                // correct hat's next prompt can consume it.
                self.state.pending_lint_resume = Some(hint);
                return prompt;
            }
        }

        // U11-T3 note: the matching `CorrectionContext` push to
        // `state.prompt_context` happens in
        // `apply_engine_required_field_gate` at the moment of
        // rejection (so the per-iteration BDD snapshot sees the
        // block in the iteration it fired). This helper only
        // emits the human-readable prompt block.

        let view = ProtocolView::from_event_loop(&self.config.event_loop);
        let mirror = build_lint_mirror_block(&view, &hint);
        let resume = build_lint_resume_block(&hint);
        format!("{mirror}{resume}\n{prompt}")
    }

    /// U2 (plan 2026-06-20-001, R15 / KTD-10): decide whether the
    /// event loop should consult the engine-backed gate before
    /// the d623c09 policy / scope gates. Same opt-in as the CLI
    /// emit lint (see `commands/emit.rs::should_run_lint`).
    pub(super) fn should_run_engine_gate(&self) -> bool {
        if std::env::var("RALPH_SERIAL_LINT_MODE")
            .map(|v| v.eq_ignore_ascii_case("off"))
            .unwrap_or(false)
        {
            return false;
        }
        if self.config.event_loop.execution_mode == HatExecutionMode::Coordinator {
            return false;
        }
        // Plan 2026-06-20-001 KTD-7 / RISK-6 circuit breaker.
        // When the linter has rejected every event for
        // `LINT_CIRCUIT_BREAKER_LIMIT` consecutive iterations,
        // it auto-disables itself for the rest of the run.
        // d623c09's runtime gates keep running, and the
        // existing `consecutive_malformed_events >= 3`
        // termination check remains as the final backstop. We
        // trip on threshold 2 so the breaker fires *before* the
        // termination check at 3, giving the runtime gates one
        // iteration to record the rejection before the loop
        // dies. Operators can override with
        // `RALPH_SERIAL_LINT_MODE=off`.
        if self.state.lint_circuit_breaker_tripped {
            return false;
        }
        true
    }

    /// U2 (plan 2026-06-20-001): apply the engine's required-
    /// fields gate to a parsed batch *before* handing the
    /// batch to the d623c09 policy / scope / recovery stack.
    /// Returns a fresh `ParseResult` with rejected events
    /// reported as malformed (so the existing rejection
    /// bookkeeping fires the same way it does for
    /// `event.malformed`) and the accepted events proceeding
    /// through the d623c09 path unchanged.
    ///
    /// P1-3: the previous name (`engine_required_field_filter`)
    /// suggested a pure filter; the function actually does four
    /// distinct things:
    ///
    ///   1. runs the engine gate (decision),
    ///   2. drops rejected events from the batch (filter),
    ///   3. appends a `MalformedLine` so the existing
    ///      bookkeeping increments `consecutive_malformed_events`
    ///      and publishes `event.malformed` (audit),
    ///   4. seeds `state.pending_lint_resume` so the next
    ///      `build_prompt` injects `## LINT RESUME REQUIRED`
    ///      (agent feedback).
    ///
    /// The new name `apply_engine_required_field_gate`
    /// matches the actual contract: a fail-fast **gate** that
    /// has side effects. The four steps are factored into
    /// helpers below so each step is independently testable
    /// and rename-safe.
    ///
    /// Fail-closed semantics: when the engine rejects an event
    /// (because `required_fields` are missing), the event is
    /// **dropped** — it never lands on the bus and never sees
    /// d623c09.
    ///
    /// Circuit breaker (KTD-7 / RISK-6): if every event in the
    /// batch was rejected, increment
    /// `consecutive_engine_gate_rejections`; when it reaches
    /// `LINT_CIRCUIT_BREAKER_LIMIT`, set
    /// `lint_circuit_breaker_tripped = true` so the engine
    /// gate short-circuits for the rest of the run. A
    /// batch with at least one accept resets the counter
    /// (the gate did useful work that iteration).
    pub(super) fn apply_engine_required_field_gate(
        &mut self,
        mut result: crate::event_reader::ParseResult,
    ) -> crate::event_reader::ParseResult {
        use crate::event_reader::MalformedLine;
        use crate::preset::engine::{
            GateDecision, LintContext, LintResumeHint, gates::RejectionKind, run_gates,
        };
        let view = ProtocolView::from_event_loop(&self.config.event_loop);
        let ctx = LintContext;
        let mut rejected = 0usize;
        let mut last_rejection: Option<(String, RejectionKind, String)> = None;
        let mut kept = Vec::with_capacity(result.events.len());
        for event in result.events.drain(..) {
            let topic = event.topic.clone();
            let payload_value = match event.payload.as_deref() {
                Some(s) if !s.is_empty() => Self::parse_event_payload_value(s),
                _ => serde_json::Value::Null,
            };
            let decision = run_gates(&view, &ctx, &topic, &payload_value, event.hat.as_deref());
            match decision {
                GateDecision::Accept => kept.push(event),
                GateDecision::Reject { kind, message } => {
                    rejected += 1;
                    tracing::warn!(
                        topic = %topic,
                        kind = %kind.reason_code(),
                        reason = %message,
                        hat = ?event.hat.as_deref(),
                        "engine gate rejected event (U2 fail-fast, required-fields)"
                    );
                    let raw = event.payload.clone().unwrap_or_default();
                    result.malformed.push(MalformedLine::new(
                        0,
                        &raw,
                        format!("engine_rejected:{}: {}", kind.reason_code(), message),
                    ));
                    last_rejection = Some((topic.clone(), kind, message));
                }
            }
        }
        result.events = kept;
        if rejected > 0 && result.events.is_empty() {
            self.state.consecutive_engine_gate_rejections = self
                .state
                .consecutive_engine_gate_rejections
                .saturating_add(1);
            // P1-1 (P1 follow-up): resolve the trip threshold
            // with a 3-tier fallback so tests can relax the
            // limit without `std::env::set_var` (unsafe under
            // Rust 1.81+ / workspace's `forbid(unsafe_code)`):
            //   1. test override (set via
            //      `set_lint_circuit_breaker_limit_for_test`) —
            //      wins so the 3-stage R11 escalation scenario
            //      can run independently of the env var.
            //   2. `RALPH_LINT_CIRCUIT_BREAKER_LIMIT` env var —
            //      production operator override.
            //   3. `LINT_CIRCUIT_BREAKER_LIMIT` constant (RISK-6:
            //      1-iter early warning).
            let limit = crate::event_loop::loop_state::lint_circuit_breaker_limit_for_test()
                .or_else(|| {
                    std::env::var("RALPH_LINT_CIRCUIT_BREAKER_LIMIT")
                        .ok()
                        .and_then(|v| v.parse::<u32>().ok())
                })
                .unwrap_or(LINT_CIRCUIT_BREAKER_LIMIT);
            if self.state.consecutive_engine_gate_rejections >= limit
                && !self.state.lint_circuit_breaker_tripped
            {
                self.state.lint_circuit_breaker_tripped = true;
                tracing::warn!(
                    consecutive = self.state.consecutive_engine_gate_rejections,
                    limit,
                    "lint circuit breaker tripped: engine gate disabled for remainder of run \
                     (d623c09 runtime gates remain active; RALPH_SERIAL_LINT_MODE=off \
                     is the operator override)"
                );
            }
        } else if self.state.consecutive_engine_gate_rejections > 0 {
            // Reset on any accept — the gate is still useful.
            self.state.consecutive_engine_gate_rejections = 0;
        }
        if rejected > 0 {
            tracing::debug!(
                rejected,
                kept = result.events.len(),
                "engine gate filter result"
            );
            // Review P0 #4: seed the in-memory resume hint so
            // `inject_pending_lint_resume` injects the failure
            // block on the next `build_prompt`. This is the
            // single source of truth for the lint resume path;
            // the CLI emit file-write (now a no-op stub) is no
            // longer part of the contract.
            if let Some((topic, kind, message)) = last_rejection {
                let hint = LintResumeHint::from_typed_rejection(&topic, kind, &message);
                // U11-T3: also push the lint rejection into the
                // unified `state.prompt_context` queue at the
                // moment of rejection (not at `build_prompt` time).
                // This way the per-iteration BDD snapshot sees
                // the correction block in the same iteration the
                // rejection fired, and downstream prompt
                // builders can drain the queue if needed.
                //
                // The R11 escalation tripwire (and the BDD's
                // expected `retry_count`) is keyed off the
                // reason_code (`lint:missing_field` etc.). We
                // update `LoopState::recent_rejection_digest`
                // (the legacy in-memory digest that works without
                // the unified ledger) so the next call sees the
                // incremented count. When the ledger IS
                // configured, the helper also commits a
                // `CommitDelta::RejectionRecorded` there.
                let reason_code = format!(
                    "lint:{}",
                    crate::event_loop::rejection::extract_reason_code(&message)
                );
                self.state.record_rejection_digest(
                    &reason_code,
                    &message,
                    &topic,
                    "iteration-start",
                );
                let retry_count = self
                    .state
                    .recent_rejection_digest
                    .get(&reason_code)
                    .map(|e| e.count)
                    .unwrap_or(1u32);
                let mut state_ledger = std::mem::take(&mut self.state.state_ledger);
                let _ctx = crate::correction::emit_correction_from_lint_hint(
                    state_ledger.as_mut(),
                    &hint,
                    retry_count,
                    None,
                    &mut self.state.prompt_context,
                );
                self.state.state_ledger = state_ledger;
                self.state.pending_lint_resume = Some(hint);
            }
        }
        result
    }

    /// Parse the JSON value from an event's payload string,
    /// returning `Value::Null` when the payload is empty.
    /// Non-JSON payloads are wrapped as `Value::String` so the
    /// engine's required-field check still operates (the
    /// required-field set is empty for non-object payloads, so
    /// any JSON object missing fields is correctly rejected).
    pub(super) fn parse_event_payload_value(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw).unwrap_or(serde_json::Value::String(raw.to_string()))
    }

    /// R1: helper that consults the resolver only when the hat is the
    /// synthesizer.  Returning `Option<WaveContext>` keeps the prepend
    /// helper a one-liner.
    pub(super) fn build_wave_context_for_synthesizer_if_match(
        &mut self,
        hat_id: &HatId,
    ) -> Option<crate::wave_context::WaveContext> {
        if hat_id.as_str() != "review-synthesizer" {
            return None;
        }
        self.build_wave_context_for_synthesizer()
    }

    /// Test-only accessor that mirrors
    /// [`Self::build_wave_context_for_synthesizer_if_match`].  Exposed
    /// at `pub(crate)` for the integration tests under
    /// `event_loop::tests` so they can assert the resolved context
    /// without wiring up the full multi-hat `build_prompt` machinery.
    /// Production code should call the prepend helper or
    /// `wave_context_json_for_hat`.
    #[cfg(test)]
    pub(crate) fn build_wave_context_for_synthesizer_if_match_for_test(
        &mut self,
        hat_id: &HatId,
    ) -> Option<crate::wave_context::WaveContext> {
        self.build_wave_context_for_synthesizer_if_match(hat_id)
    }

    /// R1: serialized wave context for the given hat, suitable for
    /// `RALPH_WAVE_CONTEXT` env var.  Returns `None` for hats other
    /// than `review-synthesizer` and when no wave events are present.
    pub fn wave_context_json_for_hat(&mut self, hat_id: &HatId) -> Option<String> {
        let ctx = self.build_wave_context_for_synthesizer_if_match(hat_id)?;
        serde_json::to_string(&ctx.to_json()).ok()
    }
}
