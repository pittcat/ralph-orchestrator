use super::*;
use ralph_core::{
    NonRetryableReason, PolicyRejection, Rejection, TerminationReason, U2_REJECTION_RETRY_LIMIT,
    ViolationType,
    diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef,
        RecoveryDiagnosisEnvelope, RecoveryDiagnosisEnvelopeBuilder,
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
pub fn should_gate_missing_events(
    hat_id: &HatId,
    event_loop: &EventLoop,
    candidate_topics: &[String],
) -> bool {
    let Some(config) = event_loop.registry().get_config(hat_id) else {
        return false;
    };

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
pub fn compute_recovery_status(event_loop: &mut EventLoop, topic: &str) -> Option<String> {
    let bus = event_loop.bus();
    for hat_id in bus.hat_ids() {
        if let Some(pending) = bus.peek_pending(hat_id) {
            for event in pending {
                if event.topic.as_str() == "task.resume"
                    && event.payload.contains(topic)
                    && let Some(target) = event.target.as_ref()
                {
                    return Some(target.as_str().to_string());
                }
            }
        }
    }
    None
}

/// Inject a human.guidance event directly into the events file so the agent
/// sees it on the next iteration. Used when the agent claimed to emit but no
/// event was actually written.
pub fn inject_hard_gate_guidance(ctx: &LoopContext, hat_id: &HatId, expected_topics: &[String]) {
    let events_path = resolve_current_events_path(ctx);
    let topics_str = if expected_topics.is_empty() {
        "(check hat configuration)".to_string()
    } else {
        expected_topics.join("`, `")
    };

    let payload = format!(
        "⚠️ HARD GATE TRIGGERED: Previous iteration by hat `{hat}` claimed to emit an event, \
         but NO EVENT WAS WRITTEN to the events file.\n\n\
         You MUST use the bash tool to execute: ralph emit <topic>\n\
         Allowed topics: `{topics}`\n\n\
         Writing `ralph emit` in prose or comments is NOT sufficient. \
         The turn is incomplete until the command succeeds and the event appears in the events file.",
        hat = hat_id.as_str(),
        topics = topics_str
    );

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
}

/// Inject a human.guidance event when the agent completely forgot to emit.
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
pub fn inject_missing_event_hard_gate_guidance(
    ctx: &LoopContext,
    event_loop: Option<&mut EventLoop>,
    hat_id: &HatId,
    expected_topics: &[String],
) {
    let events_path = resolve_current_events_path(ctx);
    let topics_str = if expected_topics.is_empty() {
        "(check hat configuration)".to_string()
    } else {
        expected_topics.join("`, `")
    };

    let payload = format!(
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
        // U3 (2026-06-13-001 plan): pin the next iteration to the
        // gated hat so the round-robin / coordinator selection
        // cannot drift to `executor` (or any other hat) right after
        // we surface the missing-event guidance.  Cleared by
        // `EventLoop::next_hat` on the next activation.
        event_loop.state_mut().pending_recovery_hat = Some(hat_id.clone());
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
