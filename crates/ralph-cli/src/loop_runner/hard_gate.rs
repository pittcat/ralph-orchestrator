use super::*;
use ralph_core::{
    NonRetryableReason, Rejection, TerminationReason, U2_REJECTION_RETRY_LIMIT,
    config::hat::resolve_missing_event_grace_secs,
    diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
        RecoveryDiagnosisEnvelopeBuilder,
    },
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

    // Multi-consumer pass-through hats (e.g. shipper on `plan.complete`)
    // receive the trigger via pending and intentionally do not re-emit —
    // downstream consumers still hold the same event on the bus. Skip the
    // legacy blanket gate when the activation consumed a declared trigger
    // that is whitelisted in `trigger_multi_consumer_topics`.
    if regular_events_declare_multi_consumer_pass_through(config, event_loop) {
        return false;
    }

    // Legacy blanket rule: hat has an obligation to publish but no
    // automatic fallback.
    !config.publishes.is_empty() && config.default_publishes.is_none()
}

/// True when this hat was activated by a multi-consumer trigger it is
/// allowed to observe without re-emitting (pass-through forwarder).
fn regular_events_declare_multi_consumer_pass_through(
    config: &ralph_core::HatConfig,
    event_loop: &EventLoop,
) -> bool {
    event_loop
        .state()
        .last_activation_events
        .iter()
        .any(|event| {
            let topic = event.topic.as_str();
            config.triggers.iter().any(|t| t == topic)
                && config.trigger_multi_consumer_topics.contains(topic)
                // Pass-through only: the hat's sole publish is the same
                // topic (e.g. shipper forwards `plan.complete`). Hats like
                // reporter that publish a different terminal topic must
                // still emit.
                && config.publishes.len() == 1
                && config.publishes.iter().any(|p| p == topic)
        })
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
