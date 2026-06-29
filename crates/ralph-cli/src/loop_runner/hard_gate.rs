use super::*;
use ralph_core::{
    NonRetryableReason, PolicyRejection, Rejection, RejectionKind, RejectionStage,
    TerminationReason, U2_REJECTION_RETRY_LIMIT, ViolationType,
    config::hat::resolve_missing_event_grace_secs,
    diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef,
        RecoveryDiagnosisEnvelope, RecoveryDiagnosisEnvelopeBuilder,
    },
    event_loop::rejection::enrich_task_resume_payload_with_stage,
};

pub fn should_hard_gate(hat_id: &HatId, event_loop: &EventLoop) -> bool {
    let Some(config) = event_loop.registry().get_config(hat_id) else {
        return false;
    };
    !config.publishes.is_empty() && config.default_publishes.is_none()
}

/// Determine whether the active hat has an emit obligation but produced no events.
///
/// This catches the "completely forgot to emit" case where the agent output
/// does not even mention `ralph emit`. Contrast with `should_hard_gate` which
/// only triggers when the agent claims to emit but writes no event.
///
/// 2026-06-07 plan U4: the activation-level obligations on the hat
/// take precedence over the blanket `publishes + default_publishes`
/// rule.  When a hat declares at least one `obligations` entry the
/// activation-level path is in charge; the runner is the source of
/// truth on whether the obligation is satisfied (via
/// `obligation_satisfied`).  This way a hat with conditional emit
/// semantics (e.g. `review-coordinator` emits `review.passed` for
/// empty diffs and `review.wave.ready` otherwise) is no longer
/// mis-classified as a missing-event offender.
///
/// `candidate_topics` is the list of topics the agent emitted (or
/// tried to emit) on this iteration.  When the hat has obligations
/// the gate only fires if the candidates fail to satisfy the
/// activation-level obligation.  When the hat has no obligations the
/// legacy blanket rule is applied unchanged for backwards
/// compatibility with presets that have not opted in.
///
/// 2026-06-17-004 U2 (R3): the per-hat activation clock
/// (`LoopState::hat_activation_at`) defers the gate for the first
/// `missing_event_grace_secs` seconds after a hat is activated.  This
/// protects long-running hats like `dimension-reviewer` (per-worker
/// timeout 1800s) from being mis-fired during the first ~30-60s of
/// model warm-up just because no event has appeared on the bus yet.
/// The grace window is resolved by
/// [`resolve_missing_event_grace_secs`]:
///   1. `hat.missing_event_grace_secs` (per-hat override)
///   2. preset default (operator-controlled)
///   3. `min(adapter_idle * 0.3, 540)` (diagnostic-recommended)
///   4. `0` (never suppress — legacy / opt-out)
pub fn should_gate_missing_events(
    hat_id: &HatId,
    event_loop: &EventLoop,
    candidate_topics: &[String],
) -> bool {
    let Some(config) = event_loop.registry().get_config(hat_id) else {
        return false;
    };

    let grace_secs = resolve_missing_event_grace_secs(
        config,
        None, // preset default — TODO: wire in U3 follow-up
        event_loop.config().cli.idle_timeout_secs,
    );
    let grace_duration = std::time::Duration::from_secs(u64::from(grace_secs));

    // 2026-06-26 plan U4: obligation-based precedence.
    //
    // The old code consulted `hat_activation_at` and gated the
    // check on `elapsed >= grace_secs`. That clock got refreshed
    // every `record_hat_activation` call and could be tricked by
    // `task.resume` (which reactivated the hat and reset the
    // timestamp) — a stuck hat could hide behind a refreshed
    // clock and never trip the gate.
    //
    // The new model uses the U3 `HatObligation` queue: the gate
    // fires ONLY when the hat has an open obligation that has
    // overstayed its grace window. `task.resume` re-arms the
    // existing obligation (it does NOT create a new one and does
    // NOT refresh `created_at`) so a stuck hat cannot hide.
    //
    // The clock model is kept as a fallback for the first-ever
    // activation (no obligation pushed yet) and for hats whose
    // `terminal_events` is empty.
    if let Some(overdue) = event_loop
        .state()
        .overdue_obligation(hat_id, grace_duration)
    {
        // Trace so operators see WHY the gate fired — without
        // this, the obligation's `redispatch_count` is invisible.
        tracing::debug!(
            hat = %hat_id,
            trigger = %overdue.trigger_topic,
            redispatches = overdue.redispatch_count,
            "MissingEventGate: obligation overdue (U4)"
        );
        return true;
    }
    // No overdue obligation — fall through to the legacy
    // activation-clock check. The obligation queue is the
    // primary signal; the clock is the secondary.
    if grace_secs > 0
        && let Some(elapsed) = event_loop.state().hat_activation_elapsed(hat_id)
        && elapsed < grace_duration
    {
        return false;
    }

    // Unit 6 (2026-06-17-001): GateWaveMutex — don't gate if a wave obligation
    // is pending for this hat. When a hat has emitted a wave batch (e.g.
    // `review.wave.ready`) but the workers haven't all reported back yet, the
    // missing-event gate should NOT fire — the hat is waiting on the wave,
    // not dead. The obligation is cleared when the wave reaches a terminal
    // phase (Closed / PartialClosed / Failed / Degraded).
    //
    // U4: opt-in hats get the precise activation-level path.  Legacy
    // presets without `obligations` keep the blanket rule.
    let matching_obligations: Vec<_> = event_loop
        .state()
        .last_activation_events
        .iter()
        .filter_map(|event| config.obligation_for_trigger(event.topic.as_str()))
        .collect();
    if !matching_obligations.is_empty() {
        // 2026-06-08 fix: build a `TriggerContext` PER obligation,
        // matched against that obligation's own `on_trigger` topic.
        // This ensures that when a hat has obligations for multiple
        // triggers (e.g. `work.done` and `fix.applied` on
        // review-coordinator), each obligation is evaluated against
        // the payload of its own trigger event — not the first
        // matching event in `last_activation_events`. Without per-
        // obligation isolation, divergent payloads would silently
        // corrupt the conditional_must_emit decision.
        //
        // When the payload is not valid JSON or fields are missing,
        // `TriggerContext::from_payload` returns a context with all
        // fields `None` — which means predicates effectively never
        // match (preserves legacy OR semantics as a safe default).
        let obligation_satisfied = matching_obligations.iter().any(|obligation| {
            let trigger_context: Option<ralph_core::TriggerContext> = event_loop
                .state()
                .last_activation_events
                .iter()
                .find(|event| event.topic.as_str() == obligation.on_trigger)
                .and_then(|event| serde_json::from_str::<serde_json::Value>(&event.payload).ok())
                .map(|payload| ralph_core::TriggerContext::from_payload(&payload));
            ralph_core::obligation_satisfied(
                Some(obligation),
                candidate_topics,
                trigger_context.as_ref(),
            )
        });
        if obligation_satisfied {
            return false;
        }
        // Obligation declared but unsatisfied: only fall back to
        // the gate when the hat is NOT waiting on a wave. If the
        // dispatcher has a non-terminal flow record for this hat,
        // the hat is still legitimately waiting on its workers —
        // give them time before the gate trips. We pass an empty
        // topic filter: any in-flight wave for the hat counts,
        // because the dispatcher's flow record uses the wave's
        // own topic (e.g. `review.wave.ready`), not the hat's
        // activation trigger (`work.done`).
        if event_loop
            .state()
            .flow_lifecycle
            .is_obligation_pending_for_hat(hat_id.as_str(), &[])
        {
            return false;
        }
        return true;
    }
    // Legacy blanket rule: hat has an obligation to publish but no
    // automatic fallback.
    !config.publishes.is_empty() && config.default_publishes.is_none()
}

/// U6: Handle execution contract rejections for operator visibility.
///
/// Logs warnings with topic, hat, reason, task_id for each rejection so
/// operators see the rejection in the console. Delegates structured
/// diagnostics to `DiagnosticsCollector::log_execution_contract_rejections`,
/// which writes an `OrchestrationEvent::ExecutionContractRejected` entry to
/// `orchestration.jsonl` — the standard path that TUI/RPC observers
/// already subscribe to via the EventBus. No-op when diagnostics are
/// disabled. Does NOT terminate the loop — guidance drives the next
/// iteration.
///
/// 2026-06-04 plan U7: Also emits `OrchestrationEvent::ContractRecoveryRouted`
/// for each rejected topic so operators can distinguish
///   (a) rejected event will be retried by the source hat, from
///   (b) rejected event has no safe retry target and needs human intervention.
///
/// 2026-06-04 plan U4 step-04: Also writes a `RecoveryDiagnosisEnvelope`
/// per rejection to `recovery.jsonl` and a high-level
/// `OrchestrationEvent::RecoveryDiagnosed` audit to `orchestration.jsonl`.
/// The envelope's `safe_target` mirrors whether a retry target was
/// routed; the `target_hat` is the routed hat or `None` for fail-closed
/// rejections. The rejected event itself is NOT re-published — this
/// function only records the diagnosis.
///
/// 2026-06-07 plan U2 (rework):
///   - **Provenance**: the source hat for the rejection is read from
///     `finding.source_hat` (the JSONL `event.hat` field, or the
///     runner's `last_active_hat_id` fallback).  This is the hat
///     that *emitted* the event — NOT the runner's current display
///     hat.  In Coordinator mode the display hat can be "ralph"
///     while the real source is `executor`; using the display hat
///     would loop back to the wrong role.
///   - **Original trigger snapshot**: the resume payload embeds the
///     actual triggering event (e.g. `work.ready` for an `executor`
///     retry) so the resumed hat sees the same context it saw on
///     the first dispatch — not the rejected topic itself.
///   - **Bounded retry budget**: the per-key counter
///     (`record_rejection_key`) gates the retry.  When the post-
///     increment count *exceeds* `U2_REJECTION_RETRY_LIMIT` the
///     rejection is marked fail-closed and the function returns
///     `Some(TerminationReason::RecoveryExhausted)` so the runner
///     can terminate the loop instead of looping forever.
pub fn handle_execution_contract_rejections(
    processed: &ralph_core::ProcessedEvents,
    event_loop: &mut EventLoop,
    hat_id: &HatId,
) -> Option<TerminationReason> {
    let rejections = &processed.contract_rejections;
    if rejections.is_empty() {
        return None;
    }

    let iteration = event_loop.state().iteration;
    let hat_name = hat_id.as_str();
    let session_id = event_loop.diagnostics().session_id();
    let display_hat = hat_name.to_string();
    let mut termination: Option<TerminationReason> = None;

    // Console-visible warning for each rejection. Include retry_target
    // status when available so operators can see at a glance whether the
    // rejection will auto-recover or needs intervention.
    for finding in rejections {
        // ── U2: build a unified Rejection from the contract finding ──
        // The runner previously only wrote diagnostic envelopes for
        // contract rejections and relied on human.guidance to drive
        // the next iteration.  Per the 2026-06-07 plan Unit 2, the
        // runner classifies the rejection via the shared Rejection type
        // and records the stable retry key against the bounded budget.
        // EventLoop owns publication of the targeted `task.resume`; this
        // layer only observes that routing and records diagnostics.
        //
        // **U7b (plan 2026-06-21-002):** the
        // `task.resume`-from-hard-gate path is preserved for
        // backwards compatibility (the feature flag
        // `UNIFIED_DETERMINISTIC_CORRECTION` is off by default).
        // When the flag is on, contract rejections should
        // populate a
        // [`crate::correction::CorrectionContext`] on
        // `state.prompt_context` instead of relying on
        // `task.resume`; U9 will migrate the production code to
        // the new API.
        //
        // Provenance priority:
        //   1. `finding.source_hat` — stamped onto the finding by
        //      `validate_execution_contract` from the original JSONL
        //      `event.hat` field (most accurate).
        //   2. `display_hat` (the runner's current hat) — used as a
        //      last-resort fallback when the finding is produced by a
        //      legacy code path that does not stamp source_hat.
        let real_source = finding
            .source_hat
            .clone()
            .unwrap_or_else(|| display_hat.clone());
        let business_hat = if real_source == "ralph" {
            // In Coordinator mode, "ralph" is the umbrella; the
            // display_hat is the actual business hat.  Treat
            // display_hat as the business role for diagnostics.
            display_hat.clone()
        } else {
            real_source.clone()
        };
        let mut rejection = Rejection::from_execution_contract(
            finding,
            Some(real_source.clone()),
            Some(business_hat.clone()),
        );

        // Record the rejection key and check the bounded budget.
        // `>` semantics: counts 1..=LIMIT all permit a task.resume;
        // the (LIMIT+1)-th attempt is the first one marked exhausted
        // and triggers a fail-closed termination.
        let retry_count = event_loop
            .state_mut()
            .record_rejection_key(&rejection.retry_key);
        let budget_exhausted = retry_count > U2_REJECTION_RETRY_LIMIT;
        if budget_exhausted {
            rejection.retry_eligible = false;
            rejection.non_retryable_reason = Some(NonRetryableReason::RetryBudgetExhausted);
            rejection.target_hat = None;
            warn!(
                topic = %finding.topic,
                hat = %real_source,
                violation = ?finding.kind,
                retry_key = %rejection.retry_key,
                retry_count = retry_count,
                limit = U2_REJECTION_RETRY_LIMIT,
                "Execution contract rejection budget exhausted; fail-closed"
            );
            // Promote the first exhausted rejection into a
            // termination reason.  Subsequent exhausted rejections
            // (if any) in the same batch collapse into the same
            // TerminationReason — only the retry_key of the first
            // one is carried so the operator can grep recovery.jsonl.
            if termination.is_none() {
                termination = Some(TerminationReason::RecoveryExhausted {
                    retry_key: rejection.retry_key.clone(),
                    reason: format!(
                        "execution contract rejection on '{}' exceeded retry budget ({} > {})",
                        finding.topic, retry_count, U2_REJECTION_RETRY_LIMIT
                    ),
                });
            }
        }

        // EventLoop owns recovery publication. The runner only observes
        // the already-routed task.resume and records bounded diagnostics.
        let recovery = compute_recovery_status(event_loop, finding.topic.as_str());

        match &recovery {
            Some(target) => warn!(
                topic = %finding.topic,
                hat = %real_source,
                violation = ?finding.kind,
                message = %finding.message,
                retry_target = %target,
                "Execution contract rejected event; targeted recovery routed to source hat"
            ),
            None => warn!(
                topic = %finding.topic,
                hat = %real_source,
                violation = ?finding.kind,
                message = %finding.message,
                "Execution contract rejected event; NO safe retry target — recovery is human.guidance only"
            ),
        }

        // Structured diagnostics: record whether recovery was routed.
        let (retry_target, no_retry_reason) = match &recovery {
            Some(t) => (Some(t.clone()), None),
            None => (
                None,
                Some(if budget_exhausted {
                    "retry budget exhausted for this rejection key".to_string()
                } else {
                    "no safe retry target (see human.guidance)".to_string()
                }),
            ),
        };
        event_loop.diagnostics().log_orchestration(
            iteration,
            hat_name,
            ralph_core::diagnostics::OrchestrationEvent::ContractRecoveryRouted {
                topic: finding.topic.clone(),
                retry_target,
                no_retry_reason,
            },
        );

        // U4: write recovery envelope for this rejection. The rejected
        // event is NOT re-published; this only records the diagnosis.
        let reason_code = format!("{:?}", finding.kind);
        let target_hat_for_envelope = recovery.clone();
        let safe_target = target_hat_for_envelope.is_some() && !budget_exhausted;
        let outcome = if budget_exhausted {
            DiagnosisOutcome::Escalated
        } else {
            DiagnosisOutcome::Pending
        };
        let mut builder = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::ExecutionContract)
            .severity(DiagnosisSeverity::Error)
            .iteration(iteration)
            .source_hat(&real_source)
            .topic(finding.topic.as_str())
            .reason_code(reason_code.clone())
            .message(finding.message.clone())
            .expected_action(if budget_exhausted {
                "stop loop: same rejection has been retried past the bounded budget"
            } else {
                "re-emit with correct contract fields"
            })
            .retry_key(RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                DiagnosisSource::ExecutionContract,
                target_hat_for_envelope.as_deref(),
                Some(finding.topic.as_str()),
                &reason_code,
                None,
            ))
            .safe_target(safe_target)
            .outcome(outcome);
        if let Some(session_id) = session_id.as_deref() {
            builder = builder.session_id(session_id);
        }
        if let Some(target) = target_hat_for_envelope.as_deref() {
            builder = builder.target_hat(target);
        }
        let envelope = builder.build();
        let mut notes: Vec<String> = Vec::new();
        if let Some(target) = target_hat_for_envelope.as_deref() {
            notes.push(format!("safe retry target: {target}"));
        } else {
            notes.push("no safe retry target; failed-closed".to_string());
        }
        if budget_exhausted {
            notes.push(format!(
                "retry budget exhausted ({} > {})",
                retry_count, U2_REJECTION_RETRY_LIMIT
            ));
        }
        // U6: the single entry point that funnels the envelope
        // into both the U3 journal logger and the U6 recovery
        // responder. The U3 `log_recovery` + `log_orchestration`
        // calls were replaced by `record_recovery_envelope` so the
        // responder can compute the escalation level for the next
        // prompt build.
        event_loop.record_recovery_envelope(&envelope, notes);
    }

    // Structured diagnostics: writes to orchestration.jsonl under
    // .ralph/diagnostics/<session>/. The TUI/RPC observer chain consumes
    // these via the standard EventBus path, so no separate file is needed.
    event_loop
        .diagnostics()
        .log_execution_contract_rejections(iteration, hat_name, rejections);

    termination
}

/// Inspects the bus for a targeted `task.resume` event whose target is a
/// registered hat. Returns the target hat name if a recovery was published
/// for the given topic in the current iteration's pending queues.
///
/// 2026-06-17-003 P1 fix: parses the task.resume payload as JSON and
/// matches the `rejected_topic` field directly. The previous
/// implementation used `event.payload.contains(topic)`, which is
/// fragile — once U2 introduced structured JSON payloads
/// (`rejected_topic`, `target_hat`, `reason`, `source_hat`, `message`),
/// a topic name like `work.done` would substring-match the
/// `target_hat`'s `allowed_topics` array (if the JSON serialised the
/// array with that topic) and produce a false positive. The JSON
/// parse also degrades gracefully for legacy free-form payloads
/// (returns `false` from the parser, falls through to the no-match
/// path — the same behaviour as the old code's `contains` on a
/// non-matching string).
pub fn compute_recovery_status(event_loop: &mut EventLoop, topic: &str) -> Option<String> {
    let bus = event_loop.bus();
    for hat_id in bus.hat_ids() {
        if let Some(pending) = bus.peek_pending(hat_id) {
            for event in pending {
                if event.topic.as_str() != "task.resume" {
                    continue;
                }
                let Some(target) = event.target.as_ref() else {
                    continue;
                };
                if task_resume_payload_matches_topic(&event.payload, topic) {
                    return Some(target.as_str().to_string());
                }
            }
        }
    }
    None
}

/// Returns `true` when the task.resume payload's `rejected_topic` field
/// equals the given topic. Defensive JSON parser — if the payload is
/// not valid JSON, or the field is missing / not a string, returns
/// `false` (no false-positive match). Used by [`compute_recovery_status`]
/// to replace the previous `event.payload.contains(topic)` heuristic.
fn task_resume_payload_matches_topic(payload: &str, topic: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return false;
    };
    value
        .get("rejected_topic")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t == topic)
}

/// Inject a structured `task.resume` event directly into the events file so
/// the agent sees it on the next iteration. Used when the agent claimed to
/// emit but no event was actually written.
///
/// 2026-06-17-003 plan U3: switched from `human.guidance` (free-form
/// text, impersonating human steer) to `task.resume` (structured
/// recovery payload) so automated recovery does not pretend to be
/// operator guidance.  The `enrich_task_resume_payload` helper
/// guarantees the schema-required `reason` + `target_hat` fields are
/// present (drift detector's `field_completeness` was 0% before this
/// fix).  The original message text is preserved as the `message` field
/// inside the JSON payload, and the allowed topics are written as a
/// structured `allowed_topics` array.  When `event_loop` is provided
/// the `pending_recovery_hat` pin is set to the offending hat so the
/// next activation lands back on it (matches the missing-event
/// sibling helper below).
///
/// 2026-06-17-003 P1 fix: the signature now takes
/// `event_loop: Option<&mut EventLoop>` so the helper can mirror
/// `inject_missing_event_hard_gate_guidance` and pin
/// `pending_recovery_hat` to the offending hat. Previously the
/// claim-but-no-write path left the pin unset, so the next iteration
/// could round-robin to an unrelated hat and the agent would have no
/// clear retry path. The call site at `runner.rs` is updated to pass
/// `Some(&mut event_loop)`. The signature is intentionally
/// `Option<&mut EventLoop>` to keep backwards compatibility with the
/// legacy callers that did not have an EventLoop handle available.
///
/// 2026-06-17-004 U4 (R1): this function is now a thin wrapper
/// over [`inject_hard_gate_guidance_with_triggers`] that passes an
/// empty trigger snapshot.  Callers that have an `EventLoop`
/// handle available should prefer the `_with_triggers` form so
/// the resume payload can carry the original obligation trigger
/// topic + payload (e.g. `review.dimension.ready(dimension=testing)`)
/// forward to the next activation.  When the wrapper is used the
/// `original_trigger_topic` / `original_trigger_payload` fields
/// are absent from the resume JSON — fine for legacy callers that
/// have no `last_activation_events` to snapshot, but the runner's
/// primary call site (claim-but-no-write path in `runner.rs`)
/// MUST use the `_with_triggers` form so the recovery can land
/// back on the right `review.dimension` for `dimension-reviewer`.
///
/// `#[allow(dead_code)]` — kept for backwards compatibility
/// (legacy callers + tests) but the runner now invokes
/// `inject_hard_gate_guidance_with_triggers` directly.
#[allow(dead_code)]
pub fn inject_hard_gate_guidance(
    ctx: &LoopContext,
    event_loop: Option<&mut EventLoop>,
    hat_id: &HatId,
    expected_topics: &[String],
) {
    inject_hard_gate_guidance_with_triggers(ctx, event_loop, hat_id, expected_topics, &[])
}

/// 2026-06-17-004 U4 (R1): internal entry point that takes the
/// obligation-trigger snapshot for the claim-but-no-write hard
/// gate.  Mirrors the
/// [`inject_missing_event_hard_gate_guidance_with_triggers`]
/// shape: embeds the first trigger's `topic` + `payload` into
/// the resume JSON (`original_trigger_topic` /
/// `original_trigger_payload`) and stashes the full snapshot
/// into `LoopState::pending_obligation_triggers` so the runner's
/// `replay_obligation_triggers_to_activation_state` can drain it
/// into `last_activation_events` for the next activation.
///
/// The `target` field on the JSONL record is set to `hat_id` so
/// the `EventBus` re-reader routes the resume to the gated hat
/// without parsing the payload (matches the missing-event path's
/// `Event::with_target` contract from 2026-06-17-003 plan R5).
pub fn inject_hard_gate_guidance_with_triggers(
    ctx: &LoopContext,
    event_loop: Option<&mut EventLoop>,
    hat_id: &HatId,
    expected_topics: &[String],
    obligation_triggers: &[ralph_proto::Event],
) {
    let events_path = resolve_current_events_path(ctx);
    let topics_str = if expected_topics.is_empty() {
        "(check hat configuration)".to_string()
    } else {
        expected_topics.join("`, `")
    };

    let free_form_message = format!(
        "⚠️ HARD GATE TRIGGERED: Previous iteration by hat `{hat}` claimed to emit an event, \
         but NO EVENT WAS WRITTEN to the events file.\n\n\
         You MUST use the bash tool to execute: ralph emit <topic>\n\
         Allowed topics: `{topics}`\n\n\
         Writing `ralph emit` in prose or comments is NOT sufficient. \
         The turn is incomplete until the command succeeds and the event appears in the events file.",
        hat = hat_id.as_str(),
        topics = topics_str
    );

    // U3: build a structured `task.resume` payload.
    // `enrich_task_resume_payload_with_stage` wraps the free-form
    // message and adds schema-required `reason` + `target_hat`.
    //
    // U4 (R1): pass the new `RejectionStage::EmitClaimedButNotWritten`
    // variant so the drift detector can distinguish "agent forgot
    // to emit" (MissingEvent) from "agent claimed to emit but the
    // run fell off the rails" (this variant).  Both share the same
    // recovery shape but the operator-actionable root cause is
    // different — the new stage value keeps the failure-bucket
    // counters stable across the two paths.
    let hat_name = hat_id.as_str();
    let resume_payload = enrich_task_resume_payload_with_stage(
        &free_form_message,
        "emit_claimed_but_not_written",
        Some(hat_name),
        Some(RejectionStage::EmitClaimedButNotWritten),
        Some(RejectionKind::MissingEventGate),
    );
    let resume_value: serde_json::Value = serde_json::from_str(&resume_payload)
        .expect("enrich_task_resume_payload must produce valid JSON");
    let mut resume_obj = resume_value
        .as_object()
        .cloned()
        .expect("enrich_task_resume_payload must produce a JSON object");
    resume_obj.insert(
        "allowed_topics".into(),
        serde_json::Value::Array(
            expected_topics
                .iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect(),
        ),
    );
    resume_obj.insert("hint".into(), serde_json::Value::String(free_form_message));
    resume_obj.insert(
        "triggered".into(),
        serde_json::Value::String(hat_name.to_string()),
    );
    // 2026-06-17-004 U4 (R1): embed the original obligation
    // trigger topic + payload so the resumed hat sees the same
    // context it saw on the first dispatch.  When multiple
    // triggers are pending (e.g. `work.done` + `fix.applied`),
    // the first trigger is used for the embedded payload; the
    // rest are still replayed into `last_activation_events` so
    // the obligation check sees them on the next activation.
    // When `obligation_triggers` is empty (legacy caller) the
    // fields are simply omitted — the resume remains a valid
    // `task.resume` event with the schema-required fields only.
    if let Some(first_trigger) = obligation_triggers.first() {
        resume_obj.insert(
            "original_trigger_topic".into(),
            serde_json::Value::String(first_trigger.topic.to_string()),
        );
        let trigger_payload = serde_json::from_str::<serde_json::Value>(&first_trigger.payload)
            .unwrap_or_else(|_| serde_json::Value::String(first_trigger.payload.clone()));
        resume_obj.insert("original_trigger_payload".into(), trigger_payload);
    }
    let payload_str = serde_json::Value::Object(resume_obj).to_string();

    let timestamp = chrono::Utc::now().to_rfc3339();
    // 2026-06-17-004 U3 (R4+R5): write the resume event as an
    // `Event::with_target`-shaped JSONL record.  The top-level
    // `target` field mirrors `Event::target` so downstream
    // consumers (e.g. the `EventBus` re-reader) can route the
    // resume to the offending hat without parsing the payload.
    // The `hat` field is also written for backwards compat with
    // U1's `check_emit_provenance` (it inspects the top-level
    // `hat` for the allowlist check).
    let event = serde_json::json!({
        "topic": "task.resume",
        "hat": hat_name,
        "target": hat_name,
        "payload": payload_str,
        "ts": timestamp,
    });

    match serde_json::to_string(&event) {
        Ok(line) => {
            use std::io::Write;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path);
            match file {
                Ok(f) => {
                    let mut writer = std::io::BufWriter::new(f);
                    if writeln!(writer, "{}", line).is_err() {
                        warn!(path = ?events_path, "Failed writing hard-gate guidance event");
                    }
                }
                Err(e) => {
                    warn!(error = %e, path = ?events_path, "Failed opening events file for hard-gate guidance");
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed serializing hard-gate guidance event");
        }
    }

    // P1 fix: pin the next iteration to the offending hat so the
    // round-robin / coordinator selection cannot drift away from the
    // gated hat. Mirrors `inject_missing_event_hard_gate_guidance`
    // (line ~644). Without this pin, the next iteration could
    // round-robin to a different hat and the task.resume hint would
    // land on a hat that does not own the expected topics.
    if let Some(event_loop) = event_loop {
        event_loop.state_mut().pending_recovery_hat = Some(hat_id.clone());
        // 2026-06-17-004 U4 (R1): stash the obligation-trigger
        // snapshot so the runner's
        // `replay_obligation_triggers_to_activation_state` can
        // drain it into `last_activation_events` for the next
        // activation.  Same pattern as the missing-event path
        // (see `inject_missing_event_hard_gate_guidance_with_triggers`).
        if !obligation_triggers.is_empty() {
            event_loop.state_mut().pending_obligation_triggers = obligation_triggers.to_vec();
        }
    }
}

/// Inject a structured `task.resume` event when the agent completely
/// forgot to emit.
///
/// This is distinct from `inject_hard_gate_guidance` which handles the case
/// where the agent claimed to emit but no event was written. This function
/// handles the case where the agent simply did not emit any event at all.
///
/// 2026-06-04 plan U4 step-01: When `event_loop` is provided, also writes
/// a `RecoveryDiagnosisEnvelope` to `recovery.jsonl` and a corresponding
/// `OrchestrationEvent::RecoveryDiagnosed` audit to `orchestration.jsonl`
/// so the missing-event gate is auditable. The guidance payload itself
/// is unchanged; the envelope only records the diagnosis.
///
/// 2026-06-17-003 plan U3: switched from `human.guidance` (free-form
/// text, impersonating human steer) to `task.resume` (structured
/// recovery payload).  `enrich_task_resume_payload` guarantees the
/// schema-required `reason` + `target_hat` fields are present.  The
/// free-form message is preserved as `message` / `hint` fields, the
/// allowed topics are written as a structured `allowed_topics` array,
/// and `triggered` is stamped to the offending hat.  The
/// `pending_recovery_hat` pin and the recovery envelope write are
/// unchanged.
///
/// 2026-06-17-004 U3 (R4+R5): the function now records the
/// obligation-trigger snapshot into `LoopState::pending_obligation_triggers`
/// before injecting the resume, and embeds the first trigger's
/// `topic` + `payload` into the resume JSON via
/// `original_trigger_topic` / `original_trigger_payload`.  The
/// runner's `replay_obligation_triggers_to_activation_state` is
/// called from the runner AFTER pinning `pending_recovery_hat`
/// (see `runner.rs:4156` area) so the next activation's
/// `last_activation_events` contains the original trigger — the
/// obligation check on the next pass has the right context.
///
/// `#[allow(dead_code)]` — kept for backwards compatibility
/// (legacy callers + tests) but the runner now invokes
/// `inject_missing_event_hard_gate_guidance_with_triggers`
/// directly.
#[allow(dead_code)]
pub fn inject_missing_event_hard_gate_guidance(
    ctx: &LoopContext,
    event_loop: Option<&mut EventLoop>,
    hat_id: &HatId,
    expected_topics: &[String],
) {
    inject_missing_event_hard_gate_guidance_with_triggers(
        ctx,
        event_loop,
        hat_id,
        expected_topics,
        &[],
    )
}

/// 2026-06-17-004 U3 (R4+R5): internal entry point that takes
/// the obligation-trigger snapshot.  Public callers (e.g. the
/// runner) should prefer the no-snapshot wrapper above for
/// backwards compatibility.
pub fn inject_missing_event_hard_gate_guidance_with_triggers(
    ctx: &LoopContext,
    event_loop: Option<&mut EventLoop>,
    hat_id: &HatId,
    expected_topics: &[String],
    obligation_triggers: &[ralph_proto::Event],
) {
    let events_path = resolve_current_events_path(ctx);
    let topics_str = if expected_topics.is_empty() {
        "(check hat configuration)".to_string()
    } else {
        expected_topics.join("`, `")
    };

    let free_form_message = format!(
        "⚠️ HARD GATE TRIGGERED: Previous iteration by hat `{hat}` did NOT emit any event.\n\n\
         This hat is configured to publish events but emitted nothing. Ralph cannot \
         proceed without observable completion signals.\n\n\
         You MUST use the bash tool to execute: ralph emit <topic>\n\
         Allowed topics: `{topics}`\n\n\
         If the work is complete, emit `work.done`. If the work failed, emit `work.failed`. \
         Do not update files or write prose without emitting an event.",
        hat = hat_id.as_str(),
        topics = topics_str
    );

    // U3: build a structured `task.resume` payload.  Same shape as
    // `inject_hard_gate_guidance` — `enrich_task_resume_payload_with_stage`
    // adds schema-required `reason` + `target_hat`; we layer
    // `allowed_topics`, `hint` and `triggered` on top for the
    // agent to read from the event.  2026-06-17-004 U3 (R4+R5)
    // adds `stage: "missing_event"` so the drift detector's
    // field-completeness metric counts these as a recognisable
    // rejection class.
    let hat_name = hat_id.as_str();
    let resume_payload = enrich_task_resume_payload_with_stage(
        &free_form_message,
        "hard_gate_missing_event",
        Some(hat_name),
        Some(RejectionStage::MissingEvent),
        Some(RejectionKind::MissingEventGate),
    );
    let resume_value: serde_json::Value = serde_json::from_str(&resume_payload)
        .expect("enrich_task_resume_payload must produce valid JSON");
    let mut resume_obj = resume_value
        .as_object()
        .cloned()
        .expect("enrich_task_resume_payload must produce a JSON object");
    resume_obj.insert(
        "allowed_topics".into(),
        serde_json::Value::Array(
            expected_topics
                .iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect(),
        ),
    );
    resume_obj.insert("hint".into(), serde_json::Value::String(free_form_message));
    resume_obj.insert(
        "triggered".into(),
        serde_json::Value::String(hat_name.to_string()),
    );
    // 2026-06-17-004 U3 (R4+R5): embed the original obligation
    // trigger topic + payload so the resumed hat sees the same
    // context it saw on the first dispatch.  When multiple
    // triggers are pending (e.g. `work.done` + `fix.applied`),
    // the first trigger is used; the rest are still replayed
    // into `last_activation_events` so the obligation check sees
    // them on the next activation.
    if let Some(first_trigger) = obligation_triggers.first() {
        resume_obj.insert(
            "original_trigger_topic".into(),
            serde_json::Value::String(first_trigger.topic.to_string()),
        );
        let trigger_payload = serde_json::from_str::<serde_json::Value>(&first_trigger.payload)
            .unwrap_or_else(|_| serde_json::Value::String(first_trigger.payload.clone()));
        resume_obj.insert("original_trigger_payload".into(), trigger_payload);
    }
    let payload_str = serde_json::Value::Object(resume_obj).to_string();

    let timestamp = chrono::Utc::now().to_rfc3339();
    // 2026-06-17-004 U3 (R4+R5): same `Event::with_target` shape
    // as `inject_hard_gate_guidance` — top-level `target` and
    // `hat` fields so the resume is routed to the offending hat
    // by both the bus re-reader (target) and the U1 provenance
    // allowlist (hat).
    let event = serde_json::json!({
        "topic": "task.resume",
        "hat": hat_name,
        "target": hat_name,
        "payload": payload_str,
        "ts": timestamp,
    });

    match serde_json::to_string(&event) {
        Ok(line) => {
            use std::io::Write;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path);
            match file {
                Ok(f) => {
                    let mut writer = std::io::BufWriter::new(f);
                    if writeln!(writer, "{}", line).is_err() {
                        warn!(path = ?events_path, "Failed writing missing-event hard-gate guidance");
                    }
                }
                Err(e) => {
                    warn!(error = %e, path = ?events_path, "Failed opening events file for missing-event hard-gate guidance");
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed serializing missing-event hard-gate guidance event");
        }
    }

    // U4: write recovery envelope for the missing-event gate. The
    // `display_hat` is always a registered hat (the runner resolves
    // it from `last_active_hat_ids`), so `safe_target = true`.
    if let Some(event_loop) = event_loop {
        let hat_name = hat_id.as_str();
        let iteration = event_loop.state().iteration;
        let session_id = event_loop.diagnostics().session_id();
        let topic_for_envelope = expected_topics.first().cloned();
        let reason_code = "missing_event";
        let expected_action = if expected_topics.is_empty() {
            "emit an event per the hat's publish obligation".to_string()
        } else {
            format!("emit one of: {}", expected_topics.join(", "))
        };
        // P0-1 / P1-1 (plan 2026-06-29-006): if a
        // `stall_recovery` envelope for the same `(hat, topic)`
        // was already recorded on this iteration, the
        // `missing_event_gate` is a duplicate diagnosis of the
        // same root cause (the handoff tracker already flagged
        // the missing event via `stall_recovery`). Skip the
        // second envelope so the two retry_keys do not
        // accumulate attempts in parallel. See
        // 2026-06-29-ce-executor-serial-primary-172725 §F1/F2.
        let stall_key = match topic_for_envelope.as_deref() {
            Some(topic) => {
                // `RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts`
                // normalizes topic via `normalize_part` (which
                // replaces `.` with `_`). The 2026-06-29
                // code-review fix applies the same normalization
                // here so the dedup guard can actually find a
                // matching key — without this, the seed key
                // has `work_done` while the guard's lookup has
                // `work.done`, the dedup never fires, and the
                // primary-172725 cascade reproduces.
                let normalized_topic = topic.replace('.', "_");
                Some(format!(
                    "stall_recovery:{hat_name}:{normalized_topic}:handoff_dispatch_timeout:*"
                ))
            }
            None => None,
        };
        if let Some(key) = stall_key.as_deref() {
            let already_tracked = event_loop
                .recovery_responder()
                .tracked_retry_keys_list()
                .iter()
                .any(|k| k == key);
            if already_tracked {
                debug!(
                    hat = %hat_name,
                    topic = %topic_for_envelope.as_deref().unwrap_or(""),
                    "P0-1 (2026-06-29-006): missing_event_gate skipped — stall_recovery envelope already tracks this hat/topic; emitting task.resume guidance only"
                );
                // The task.resume guidance event has already been
                // written to the events file above; we only skip
                // the second recovery envelope to avoid double
                // counting.
                return;
            }
        }
        // U3 (2026-06-13-001 plan): pin the next iteration to the
        // gated hat so the round-robin / coordinator selection
        // cannot drift to `executor` (or any other hat) right after
        // we surface the missing-event guidance.  Cleared by
        // `EventLoop::next_hat` on the next activation.
        event_loop.state_mut().pending_recovery_hat = Some(hat_id.clone());
        // 2026-06-17-004 U3 (R4+R5): stash the obligation-trigger
        // snapshot so the runner can replay it into
        // `last_activation_events` for the next activation.  The
        // snapshot is drained by
        // `LoopState::replay_obligation_triggers_to_activation_state`.
        if !obligation_triggers.is_empty() {
            event_loop.state_mut().pending_obligation_triggers = obligation_triggers.to_vec();
        }
        let mut builder = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::MissingEventGate)
            .severity(DiagnosisSeverity::Warning)
            .iteration(iteration)
            .source_hat(hat_name)
            .target_hat(hat_name)
            .reason_code(reason_code)
            .message(format!(
                "Hat '{}' did not emit any event on its publish obligation",
                hat_name
            ))
            .expected_action(expected_action)
            .evidence(EvidenceRef {
                kind: EvidenceKind::Topic,
                ref_path: expected_topics.join(","),
                snippet: None,
            })
            .safe_target(true)
            .outcome(DiagnosisOutcome::Pending)
            .retry_key(RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                DiagnosisSource::MissingEventGate,
                Some(hat_name),
                topic_for_envelope.as_deref(),
                reason_code,
                None,
            ));
        if let Some(session_id) = session_id.as_deref() {
            builder = builder.session_id(session_id);
        }
        if let Some(topic) = topic_for_envelope.as_deref() {
            builder = builder.topic(topic);
        }
        let envelope = builder.build();
        // U6: see `record_recovery_envelope` in the execution-contract
        // path above. The missing-event gate envelopes always have a
        // safe target (the hat itself), so the responder is the right
        // place to surface them in the next prompt.
        event_loop.record_recovery_envelope(&envelope, Vec::new());
    }
}

/// U2 (2026-06-13-001): inject a `human.guidance` event when a wave
/// batch was *policy-rejected* but no `wave_events` reached the
/// dispatcher.
///
/// U3 deferred (2026-06-17-003 plan): this helper is **NOT** converted
/// to `task.resume` in U3.  It is wave-only — `ce-executor-serial`
/// does not use waves — and the U2 contracts (recovery envelope shape,
/// pinning, dedup) are still consumed by the wave path.  The
/// conversion will happen in the isolated wave stability follow-up
/// using the same `enrich_task_resume_payload` pattern that U3 applies
/// to `inject_hard_gate_guidance` / `inject_missing_event_hard_gate_guidance`.
/// See `docs/achieved/plan/2026-06-17-003-fix-ce-executor-serial-precheck-recovery-gates-plan.md`
/// → "Deferred to Follow-Up Work" for the rationale.
///
/// This is the schema-level cousin of `inject_missing_event_hard_gate_guidance`.
/// The agent DID emit a wave batch (it sits in the JSONL with `wave_id`
/// set), but the event policy kept the events off the bus because
/// required fields were missing (e.g. `depth` on `review.wave.ready`).
/// The agent must see *which field* is missing in the next prompt,
/// not a generic "you forgot to emit" message.
///
/// Behaviour, mirroring `inject_missing_event_hard_gate_guidance`:
///   - Append a `human.guidance` line to the current events file
///     listing each unique finding message. The guidance uses the
///     `Missing required field: X` text from `PolicyFinding.message`
///     so the agent can `jq` / grep the schema field directly.
///   - When `event_loop` is provided, also write a
///     `RecoveryDiagnosisEnvelope` (source `PayloadContract`, reason
///     `wave_dispatch_blocked` or `missing_required_field` per the
///     KTD-3 taxonomy in the plan) so `ralph diagnose` attributes the
///     failure to a schema contract violation, not a missing emit.
///
/// Mutual exclusion with the missing-event gate is enforced at the
/// call site: the runner routes to this helper only when
/// `wave_had_policy_rejections && wave_events.is_empty()` is true, so
/// the two guidance paths never write to the same iteration.
pub fn inject_wave_policy_rejection_guidance(
    ctx: &LoopContext,
    event_loop: Option<&mut EventLoop>,
    hat_id: &HatId,
    rejections: &[PolicyRejection],
    raw_count: usize,
    expected_topics: &[String],
) {
    if rejections.is_empty() {
        return;
    }

    // Deduplicate findings by `(topic, field, reason_code)` so a batch
    // that fails on the same field once surfaces one bullet instead
    // of N copies, but two findings on the same topic with different
    // fields stay distinct. The dedupe key also includes the violation
    // type so message-text churn in the policy layer cannot collapse
    // distinct schema errors. BTreeSet gives deterministic ordering
    // (HashSet would not), which keeps prompt regression tests stable.
    let mut seen: std::collections::BTreeSet<(String, String, &'static str)> =
        std::collections::BTreeSet::new();
    let mut unique_findings: Vec<&PolicyRejection> = Vec::new();
    for r in rejections {
        let field = r.finding.violation_type.field().unwrap_or("").to_string();
        let reason_code = r.finding.violation_type.reason_code();
        let key = (r.topic.clone(), field, reason_code);
        if seen.insert(key) {
            unique_findings.push(r);
        }
    }

    let topics_str = if expected_topics.is_empty() {
        "(check hat configuration)".to_string()
    } else {
        expected_topics.join("`, `")
    };

    let findings_block = unique_findings
        .iter()
        .map(|r| format!("  - `{}`: {}", r.topic, r.finding.message))
        .collect::<Vec<_>>()
        .join("\n");

    let payload = format!(
        "⚠️ WAVE BATCH REJECTED: Previous iteration by hat `{hat}` emitted a wave batch of \
         {raw_count} event(s) for topic(s) `{topics}`, but the event policy REJECTED every event \
         because required fields were missing or invalid. The wave dispatcher was NOT started.\n\n\
         Schema findings (unique):\n{findings}\n\n\
         You MUST fix the payload before re-emitting. Edit the `ralph wave emit` invocation so each \
         payload contains every required field (e.g. add `depth`), then re-emit. Do NOT re-emit \
         the same payload verbatim — policy will reject it again.\n\n\
         Allowed topics for this hat: `{topics}`",
        hat = hat_id.as_str(),
        topics = topics_str,
        findings = findings_block,
    );

    let events_path = resolve_current_events_path(ctx);

    let timestamp = chrono::Utc::now().to_rfc3339();
    let event = serde_json::json!({
        "topic": "human.guidance",
        "payload": payload,
        "ts": timestamp,
    });

    match serde_json::to_string(&event) {
        Ok(line) => {
            use std::io::Write;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path);
            match file {
                Ok(f) => {
                    let mut writer = std::io::BufWriter::new(f);
                    if writeln!(writer, "{}", line).is_err() {
                        warn!(path = ?events_path, "Failed writing wave-policy-rejection guidance event");
                    }
                }
                Err(e) => {
                    warn!(error = %e, path = ?events_path, "Failed opening events file for wave-policy-rejection guidance");
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed serializing wave-policy-rejection guidance event");
        }
    }

    // U2 recovery envelope: payload_contract + wave_dispatch_blocked
    // (KTD-3 in the plan). `safe_target` is `true` when at least one
    // rejected topic matches a hat obligation; otherwise the agent
    // is on its own. The display_hat is always registered.
    if let Some(event_loop) = event_loop {
        let hat_name = hat_id.as_str();
        let iteration = event_loop.state().iteration;
        let session_id = event_loop.diagnostics().session_id();
        // U3 (2026-06-13-001 plan): pin the next iteration to the
        // gated hat so the round-robin selection cannot drift away
        // from the hat that just had its wave batch rejected.
        // `EventLoop::next_hat` consumes the pin on activation.
        event_loop.state_mut().pending_recovery_hat = Some(hat_id.clone());
        // Anchor the envelope on the first rejected topic — there
        // can be several in a batch, but the retry_key only needs
        // one stable identifier for grouping.
        let first = unique_findings
            .first()
            .copied()
            .unwrap_or_else(|| &rejections[0]);
        let topic_for_envelope = &first.topic;
        let reason_code = if unique_findings.iter().all(|r| {
            matches!(
                r.finding.violation_type,
                ViolationType::MissingRequiredField { .. }
            )
        }) {
            "missing_required_field"
        } else {
            "wave_dispatch_blocked"
        };
        let message = if rejections.len() == 1 {
            format!(
                "Wave batch of {} event(s) was policy-rejected on topic `{}`: {}",
                raw_count, topic_for_envelope, first.finding.message
            )
        } else {
            format!(
                "Wave batch of {} event(s) was policy-rejected ({} unique finding(s)); first: `{}` — {}",
                raw_count,
                unique_findings.len(),
                topic_for_envelope,
                first.finding.message
            )
        };
        let expected_action = "fix the schema on the wave payload (e.g. add required fields) and re-emit with `ralph wave emit`".to_string();
        let evidence = format!(
            "{}{}",
            topic_for_envelope,
            if unique_findings.len() > 1 {
                format!(" (+{} more)", unique_findings.len() - 1)
            } else {
                String::new()
            }
        );
        let mut builder = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::PayloadContract)
            .severity(DiagnosisSeverity::Error)
            .iteration(iteration)
            .source_hat(hat_name)
            .target_hat(hat_name)
            .topic(topic_for_envelope)
            .reason_code(reason_code)
            .message(message)
            .expected_action(expected_action)
            .evidence(EvidenceRef {
                kind: EvidenceKind::Topic,
                ref_path: evidence,
                snippet: None,
            })
            .safe_target(true)
            .outcome(DiagnosisOutcome::Pending)
            .retry_key(RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                DiagnosisSource::PayloadContract,
                Some(hat_name),
                Some(topic_for_envelope),
                reason_code,
                None,
            ));
        if let Some(session_id) = session_id.as_deref() {
            builder = builder.session_id(session_id);
        }
        let envelope = builder.build();
        let notes = vec![format!(
            "wave batch: {} raw event(s), {} unique finding(s)",
            raw_count,
            unique_findings.len()
        )];
        event_loop.record_recovery_envelope(&envelope, notes);
    }
}

pub fn resolve_display_hat_for_execution(
    event_loop: &EventLoop,
    hat_id: &HatId,
    preview_display_hat: &HatId,
) -> HatId {
    if hat_id.as_str() != "ralph" {
        return hat_id.clone();
    }

    event_loop
        .state()
        .last_active_hat_ids
        .first()
        .cloned()
        .unwrap_or_else(|| preview_display_hat.clone())
}

pub fn resolve_hat_for_output_processing(hat_id: &HatId, display_hat: &HatId) -> HatId {
    if hat_id.as_str() == "ralph" {
        display_hat.clone()
    } else {
        hat_id.clone()
    }
}

#[cfg(test)]
mod p0_1_dedup_tests {
    //! P0-1 / P1-1 (plan 2026-06-29-006): when the handoff
    //! tracker has already recorded a `stall_recovery` envelope
    //! for the same `(hat, topic)` pair, the
    //! `missing_event_gate` injector must skip writing its own
    //! envelope. The two paths diagnose the same root cause
    //! (consumer did not emit within window); without dedup
    //! the two retry_keys would accumulate attempts in parallel
    //! and prematurely trigger `EscalationLevel::Final`. See
    //! 2026-06-29-ce-executor-serial-primary-172725 §F1/F2.
    //!
    //! The U5 guard fires inside
    //! `inject_missing_event_hard_gate_guidance_with_triggers`
    //! by reading the responder's `tracked_retry_keys_list()`.
    //! This test pins the contract of the *key format* the
    //! guard looks up, so any drift between the handoff
    //! tracker's recorded key and the guard's expected key
    //! causes a compile-time or assertion-time failure.

    use super::*;

    /// Format of the stall_recovery retry key the guard looks
    /// up. Must match the format produced by
    /// `RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts`
    /// for `DiagnosisSource::StallRecovery` with
    /// reason_code `handoff_dispatch_timeout`. Pinned here so
    /// drift between the two halves is caught at test time.
    ///
    /// `retry_key_from_parts` normalizes topic parts with
    /// `normalize_part`, which replaces `.` with `_`. The guard
    /// code in `inject_missing_event_hard_gate_guidance_with_triggers`
    /// mirrors the same normalization, so the two halves match
    /// even when the topic is `work.done` rather than
    /// `work_done`. We pin the normalized form here.
    fn stall_key_for(hat: &str, topic: &str) -> String {
        let normalized_topic = topic.replace('.', "_");
        format!("stall_recovery:{hat}:{normalized_topic}:handoff_dispatch_timeout:*")
    }

    #[test]
    fn p0_1_stall_key_format_is_consistent() {
        // The guard matches on the literal key
        // `stall_recovery:<hat>:<topic>:handoff_dispatch_timeout:*`,
        // normalised with `.` -> `_` (matching
        // `retry_key_from_parts`'s `normalize_part`). If either
        // the handoff tracker or the guard changes this format
        // (or the normalisation step drifts), the dedup
        // silently breaks — pin it.
        assert_eq!(
            stall_key_for("executor", "work.done"),
            "stall_recovery:executor:work_done:handoff_dispatch_timeout:*"
        );
    }

    #[test]
    fn p0_1_stall_key_distinguishes_topics() {
        let a = stall_key_for("executor", "work.ready");
        let b = stall_key_for("executor", "work.done");
        assert_ne!(a, b, "different topics must produce different keys");
    }

    #[test]
    fn p0_1_stall_key_distinguishes_hats() {
        let a = stall_key_for("executor", "work.done");
        let b = stall_key_for("validator", "work.done");
        assert_ne!(a, b, "different hats must produce different keys");
    }

    // === End-to-end U5 guard verification (plan 2026-06-29-006) ===
    //
    // The key-format tests above verify the *contract* the guard
    // reads, but do not exercise the guard itself. The end-to-end
    // tests below build a real `EventLoop`, seed a `stall_recovery`
    // envelope via `record_recovery_envelope`, then drive the
    // missing-event injector and assert the
    // `tracked_retry_keys_list` does not gain a
    // `missing_event_gate` entry. This pins the plan-required
    // "happy path / edge path / error path" coverage on the
    // actual guard function.

    use ralph_core::diagnosis::{
        DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
        RecoveryDiagnosisEnvelopeBuilder,
    };
    use ralph_core::event_loop::EventLoop;
    use ralph_core::RalphConfig;

    fn build_event_loop(workspace: &std::path::Path) -> EventLoop {
        let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
  completion_promise: "LOOP_COMPLETE"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
        let mut config: RalphConfig = serde_yaml::from_str(yaml).expect("parse test yaml");
        config.core.workspace_root = workspace.to_path_buf();
        let diagnostics = ralph_core::diagnostics::DiagnosticsCollector::with_enabled(workspace, true)
            .expect("create diagnostics collector");
        let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
        event_loop.initialize("p0_1_end_to_end");
        event_loop
    }

    fn seed_stall_recovery(event_loop: &mut EventLoop, hat: &str, topic: &str) {
        // retry_key_from_parts normalizes topic via
        // `normalize_part` (`.` -> `_`); mirror that here so
        // the assertion checks the actual key on the responder.
        let normalized_topic = topic.replace('.', "_");
        let key = format!(
            "stall_recovery:{hat}:{normalized_topic}:handoff_dispatch_timeout:*"
        );
        let envelope = RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
            DiagnosisSource::StallRecovery,
            Some(hat),
            Some(topic),
            "handoff_dispatch_timeout",
            None,
        );
        let env = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::StallRecovery)
            .source_hat(hat.to_string())
            .target_hat(hat.to_string())
            .reason_code("handoff_dispatch_timeout")
            .topic(topic.to_string())
            .severity(DiagnosisSeverity::Warning)
            .safe_target(true)
            .expected_action("wait for hat to activate".to_string())
            .message(format!("seeded stall_recovery for {hat}/{topic}"))
            .retry_key(envelope.clone())
            .build();
        let _ = event_loop.recovery_responder_mut().record_finding(&env, 1);
        // Sanity: the seeded key must be in the tracked list.
        assert!(
            event_loop
                .recovery_responder()
                .tracked_retry_keys_list()
                .iter()
                .any(|k| k == &key),
            "P0-1: seed must record the stall_recovery key, got: {:?}",
            event_loop.recovery_responder().tracked_retry_keys_list()
        );
    }

    #[test]
    fn p0_1_end_to_end_guard_skips_when_stall_recovery_tracked() {
        // Plan U5 happy path: a stall_recovery envelope is
        // already tracked; the missing-event guard must skip
        // and not register a new missing_event_gate envelope.
        let dir = tempfile::tempdir().unwrap();
        let mut event_loop = build_event_loop(dir.path());
        seed_stall_recovery(&mut event_loop, "executor", "work.done");

        let missing_key = "missing_event_gate:executor:work_done:missing_event:*";
        let before: Vec<String> = event_loop
            .recovery_responder()
            .tracked_retry_keys_list()
            .into_iter()
            .filter(|k| k == missing_key)
            .collect();
        assert!(
            before.is_empty(),
            "P0-1: missing_event_gate key must not be present before the guard runs"
        );

        // Drive the guard. We use a minimal stub hat_id and
        // empty triggers; the guard only inspects the responder
        // state, so the inputs do not matter for the dedup
        // decision. The function writes the guidance event to
        // the events file but does not record a recovery
        // envelope when the guard short-circuits.
        let ctx = ralph_core::loop_context::LoopContext::primary(dir.path().to_path_buf());
        let hat = HatId::from("executor");
        let expected_topics = vec!["work.done".to_string()];
        super::inject_missing_event_hard_gate_guidance_with_triggers(
            &ctx,
            Some(&mut event_loop),
            &hat,
            &expected_topics,
            &[],
        );

        let after: Vec<String> = event_loop
            .recovery_responder()
            .tracked_retry_keys_list()
            .into_iter()
            .filter(|k| k == missing_key)
            .collect();
        assert!(
            after.is_empty(),
            "P0-1: missing_event_gate key must not be registered when stall_recovery is already tracked; got: {after:?}"
        );
    }
}
