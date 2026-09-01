//! Outcomes module — outcome classification, deadline finalizers, recovery-envelope writers, record_outcome.
//! Originally part of `wave/dispatcher.rs` (plan `2026-08-07-008`).
//! Public surface and behaviour preserved verbatim.

use std::time::Duration;
use tracing::warn;

use ralph_core::CompletedWave;

use super::super::worker::WaveWorkerOutcome;
use super::dispatch::DispatchContext;
use super::dispatch::WORKER_TIMEOUT_ERR_PREFIX;
use crate::loop_runner::WaveDispatchOutcome;

pub(crate) fn record_outcome(
    tracker: &mut ralph_core::WaveTracker,
    wave_id: &str,
    index: u32,
    outcome: WaveWorkerOutcome,
) {
    match outcome {
        Ok((events, duration, success, _pid)) => {
            // PTY workers return Ok((_, _, false)) for non-zero exit
            // and for timeout-with-events (`run_wave_worker_pty`).
            // Distinguish:
            // - success + events present → result
            // - success + NO events → empty_worker_result (failure);
            //   a worker that exits 0 without accepted events is
            //   not a real success, it just failed silently. Without
            //   this rule, a false-green LOOP_COMPLETE could fire
            //   for a wave whose every slot is empty.
            // - success=false + events present → keep result visible
            //   (partial-timeout contract).
            // - success=false + empty → hard failure so a forced
            //   slot exit (exit 1, no events) cannot Integrate →
            //   false-green LOOP_COMPLETE.
            //
            // 2026-07-25-003 plan U5 (R3): align this branch with
            // the supervisor `classify_slot_result` truth table —
            // empty-success is `empty_worker_result` (Failed), not a
            // result.
            if success && events.is_empty() {
                tracker.record_failure(
                    wave_id,
                    index,
                    ralph_core::supervisor::worker_outcome::REASON_EMPTY_WORKER_RESULT.into(),
                    duration,
                );
            } else if success || !events.is_empty() {
                let proto_events: Vec<ralph_proto::Event> =
                    events.into_iter().map(ralph_proto::Event::from).collect();
                tracker.record_result(wave_id, index, proto_events);
            } else {
                tracker.record_failure(
                    wave_id,
                    index,
                    "worker exited unsuccessfully".into(),
                    duration,
                );
            }
        }
        Err((error, duration)) => {
            tracker.record_failure(wave_id, index, error, duration);
        }
    }
}

pub(crate) fn compute_slot_batch_fingerprint(events: &[ralph_core::Event]) -> (String, usize) {
    let mut buf = String::new();
    for event in events {
        if let Ok(line) = serde_json::to_string(event) {
            buf.push_str(&line);
            buf.push('\n');
        }
    }
    (
        ralph_core::agent_doc_sync::compute_sha256_hex(&buf),
        events.len(),
    )
}

#[derive(Debug)]
pub(crate) enum ClassifiedReason<'a> {
    Static(&'a str),
    Dynamic(&'a str),
}

#[derive(Debug)]
pub(crate) struct ClassifiedSlot<'a> {
    pub(crate) outcome: ralph_core::supervisor::worker_outcome::SlotOutcome,
    /// Reason string to forward to `record_slot_failure`. `None`
    /// when the outcome is `Completed` (the bridge takes the
    /// `record_slot_result` path, not the failure path).
    pub(crate) reason: Option<ClassifiedReason<'a>>,
}

pub(crate) fn classify_slot_result<'a>(result: &'a WaveWorkerOutcome) -> ClassifiedSlot<'a> {
    use ralph_core::supervisor::worker_outcome::{
        SlotOutcome, TerminalMarker, WorkerExit, classify_worker_outcome,
    };
    match result {
        Ok((events, _duration, success, _pid)) => {
            let exit = if *success {
                WorkerExit::Exit0
            } else {
                WorkerExit::ExitNonZero
            };
            let mut markers: Vec<TerminalMarker> = Vec::new();
            let mut accepted: usize = 0;
            for ev in events {
                accepted += 1;
                if ev.topic.ends_with(".unit.done") || ev.topic.ends_with(".wave.done") {
                    markers.push(TerminalMarker::Done);
                } else if ev.topic.ends_with(".unit.failed") || ev.topic.ends_with(".wave.failed") {
                    markers.push(TerminalMarker::Failed);
                }
            }
            let outcome = classify_worker_outcome(exit, accepted, &markers);
            let reason = match &outcome {
                SlotOutcome::Failed { reason } => Some(ClassifiedReason::Static(reason)),
                SlotOutcome::Completed(_) => None,
            };
            ClassifiedSlot { outcome, reason }
        }
        Err((reason, _duration)) => {
            // KTD8 / AE3: timeout-prefix detection — the worker.rs stable
            // prefix "Worker timed out after" identifies a genuine timeout
            // (empty event batch, no terminal). In that case we synthesise a
            // typed Timeout exit and let classify_worker_outcome resolve it
            // to the frozen reason code so the operator sees the stable
            // `worker_timeout` string instead of a raw Dynamic message.
            //
            // 2026-07-25-006 plan U9: idle heartbeat kill is the second
            // member of the `worker_timeout` family. The worker emits
            // messages beginning with `"idle heartbeat exceeded"`; we
            // route them through `WorkerExit::IdleTimeout` so the
            // classifier resolves to `worker_timeout` (the operator
            // sees the original idle string verbatim, the family
            // collapses into the same `worker_timeout` reason).
            //
            // Non-timeout Err (any other message) is preserved verbatim with
            // the legacy `worker_cancelled` shell — fixing that broader
            // mis-classification is out of scope for this plan (plan KTD8
            // explicitly says "非超时 Err 仍保留 Dynamic 原文字案").
            // 2026-07-26-002 plan U8 (R10): use the shared
            // constant so worker.rs and this classifier stay
            // compile-linked.
            if reason.starts_with(WORKER_TIMEOUT_ERR_PREFIX) {
                // Empty event batch + empty terminal markers — classify as Timeout.
                let outcome = classify_worker_outcome(WorkerExit::Timeout, 0, &[]);
                let reason = match &outcome {
                    SlotOutcome::Failed { reason } => Some(ClassifiedReason::Static(reason)),
                    SlotOutcome::Completed(_) => None,
                };
                ClassifiedSlot { outcome, reason }
            } else if reason.starts_with("idle heartbeat exceeded") {
                // 2026-07-25-006 U9: idle kill still maps to the
                // `worker_timeout` family; the reason string carries
                // the operator-visible detail (`"idle heartbeat
                // exceeded: 120s since last activity, weak_count=8"`).
                // The outcome is `Failed { reason: "worker_timeout" }`
                // but the dynamic reason surfaced to the operator is
                // the original idle string verbatim.
                let outcome = classify_worker_outcome(WorkerExit::IdleTimeout, 0, &[]);
                ClassifiedSlot {
                    outcome,
                    reason: Some(ClassifiedReason::Dynamic(reason)),
                }
            } else {
                ClassifiedSlot {
                    outcome: SlotOutcome::Failed {
                        reason: ralph_core::supervisor::worker_outcome::REASON_WORKER_CANCELLED,
                    },
                    reason: Some(ClassifiedReason::Dynamic(reason)),
                }
            }
        }
    }
}

pub(crate) fn classify_slot_attempt<'a>(
    result: &'a WaveWorkerOutcome,
    wave_kind: Option<ralph_core::supervisor::WaveKind>,
) -> ClassifiedSlot<'a> {
    use ralph_core::supervisor::WaveKind;
    use ralph_core::supervisor::worker_outcome::{
        REASON_EXECUTOR_REPORTED_FAILURE, SlotOutcome, WorkerTerminalKind,
    };

    if !matches!(wave_kind, Some(WaveKind::Exec)) {
        return classify_slot_result(result);
    }
    if let Err((reason, _duration)) = result
        && reason == REASON_EXECUTOR_REPORTED_FAILURE
    {
        return ClassifiedSlot {
            outcome: SlotOutcome::Failed {
                reason: REASON_EXECUTOR_REPORTED_FAILURE,
            },
            reason: Some(ClassifiedReason::Static(REASON_EXECUTOR_REPORTED_FAILURE)),
        };
    }
    let classified = classify_slot_result(result);
    if matches!(
        classified.outcome,
        SlotOutcome::Completed(WorkerTerminalKind::Failed)
    ) {
        return ClassifiedSlot {
            outcome: SlotOutcome::Failed {
                reason: REASON_EXECUTOR_REPORTED_FAILURE,
            },
            reason: Some(ClassifiedReason::Static(REASON_EXECUTOR_REPORTED_FAILURE)),
        };
    }
    classified
}

pub(crate) fn reported_failure_detail(result: &WaveWorkerOutcome) -> Option<String> {
    let (events, _duration, _success, _pid) = result.as_ref().ok()?;
    let failed = events
        .iter()
        .find(|event| event.topic.as_str().ends_with(".unit.failed"))?;
    let payload = failed.payload.as_deref()?;
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let reason = value.get("reason")?.as_str()?.trim();
    (!reason.is_empty()).then(|| reason.to_string())
}

pub(crate) fn take_results(
    tracker: &mut ralph_core::WaveTracker,
    wave_id: &str,
    assigned_dimensions: &std::collections::HashMap<u32, String>,
) -> CompletedWave {
    let mut completed = tracker
        .take_wave_results(wave_id)
        .expect("wave must exist in tracker after registration");
    // U4/R4 (2026-06-17-002): stamp the per-index dimension map
    // onto the returned CompletedWave so the merge layer can
    // drop mismatched review.dimension.done events.
    completed.assigned_dimensions = assigned_dimensions.clone();
    completed
}

pub(crate) fn merge_round_into(
    base: &mut Option<ralph_core::CompletedWave>,
    round: ralph_core::CompletedWave,
) {
    match base {
        None => *base = Some(round),
        Some(base) => {
            base.results.extend(round.results);
            base.failures.extend(round.failures);
            base.worker_events.extend(round.worker_events);
            base.duration += round.duration;
            base.partial = base.partial || round.partial;
        }
    }
}

pub(crate) fn outcome_for_completion(completed: CompletedWave) -> WaveDispatchOutcome {
    if completed.partial {
        WaveDispatchOutcome::Partial(completed)
    } else {
        WaveDispatchOutcome::Completed(completed)
    }
}

pub(crate) async fn finalize_timeout(
    join_set: &mut tokio::task::JoinSet<(u32, WaveWorkerOutcome)>,
    tracker: &mut ralph_core::WaveTracker,
    ctx: &DispatchContext,
    label: &'static str,
    threshold: tokio::time::Instant,
) -> CompletedWave {
    warn!(
        wave_id = %ctx.wave_id,
        label,
        "Wave deadline reached, aborting remaining workers"
    );
    inject_synthetic_failures(tracker, ctx, label, threshold);
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}
    let mut completed = tracker
        .force_take_wave_results(&ctx.wave_id)
        .expect("wave must exist in tracker after registration");
    // U4/R4 (2026-06-17-002): stamp the per-index dimension map
    // onto the returned CompletedWave even on the timeout path
    // so the merge layer can record dimension_missing failures.
    completed.assigned_dimensions = ctx.assigned_dimensions.clone();
    completed
}

pub(crate) async fn finalize_global_exceeded(
    join_set: &mut tokio::task::JoinSet<(u32, WaveWorkerOutcome)>,
    _ctx: &DispatchContext,
    progress_handle: tokio::task::JoinHandle<()>,
) {
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}
    // Reuse the same 5s defensive guard as the other exit paths
    // so a leaked sender cannot hang the dispatcher when the
    // global deadline fires.
    wait_for_progress_reporter(progress_handle).await;
}

pub(crate) fn inject_synthetic_failures(
    tracker: &mut ralph_core::WaveTracker,
    ctx: &DispatchContext,
    label: &'static str,
    threshold: tokio::time::Instant,
) {
    for i in 0..ctx.expected_total {
        if !tracker.has_reported(&ctx.wave_id, i) {
            warn!(
                wave_id = %ctx.wave_id,
                worker = i,
                label,
                "Worker did not report — recording synthetic failure"
            );
            // U4/R4 (2026-06-17-002): when the un-reported slot
            // had a dimension assignment, record a
            // `dimension_missing` failure so the merge layer
            // emits `wave.worker.failed(reason=worker_failed:dimension_missing)`
            // with the expected dimension. Plain
            // `record_failure` would lose the dimension context.
            let expected = ctx.assigned_dimensions.get(&i).cloned();
            if let Some(expected_dim) = expected {
                tracker.record_failure_with_dimensions(
                    &ctx.wave_id,
                    i,
                    format!("dimension_missing: expected={expected_dim}"),
                    threshold.saturating_duration_since(ctx.started_at),
                    Some(expected_dim),
                    None,
                );
            } else {
                tracker.record_failure(
                    &ctx.wave_id,
                    i,
                    format!("worker did not report before {label}"),
                    threshold.saturating_duration_since(ctx.started_at),
                );
            }
        }
    }
}

pub(crate) async fn wait_for_progress_reporter(progress_handle: tokio::task::JoinHandle<()>) {
    // Defensive upper bound: a leaked sender must not hang the
    // dispatcher forever. The normal path is "all senders dropped
    // → channel closed → reporter task finishes almost
    // immediately".
    match tokio::time::timeout(Duration::from_secs(5), progress_handle).await {
        Ok(Ok(())) => {}
        Ok(Err(join_err)) => {
            warn!(error = %join_err, "Progress reporter task panicked");
        }
        Err(_) => {
            warn!("Progress reporter did not exit within 5s after worker drain");
        }
    }
}

pub(crate) fn record_loop_max_runtime_envelope(
    event_loop: &mut ralph_core::EventLoop,
    loop_id: &str,
    wave: &ralph_core::DetectedWave,
) {
    use ralph_core::diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
    };

    let retry_key = format!("loop_runner:{}:max_runtime", loop_id);
    if event_loop.recovery_responder().attempt_count(&retry_key) > 0 {
        // Already recorded this loop's max_runtime abort in an
        // earlier wave/iteration. Skipping prevents responder
        // attempt_count inflation toward escalation. The
        // DiagnosticsCollector still gets the original envelope
        // from the first call, which is what the audit log
        // contract requires (one envelope per abort).
        return;
    }

    let topic = wave
        .hat_config
        .publishes
        .first()
        .cloned()
        .or_else(|| wave.events.first().map(|e| e.topic.clone()))
        .unwrap_or_default();

    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::WaveDispatcher)
        .severity(DiagnosisSeverity::Error)
        .iteration(event_loop.state().iteration)
        .source_hat(wave.target_hat.to_string())
        .topic(topic)
        .reason_code("loop_max_runtime_exceeded")
        .message(format!(
            "Loop {} max_runtime exceeded during wave {} on hat {}",
            loop_id, wave.wave_id, wave.target_hat
        ))
        .expected_action(
            "Loop will terminate with TerminationReason::MaxRuntime. Investigate long-running wave workers."
                .to_string(),
        )
        .retry_attempt(0)
        .safe_target(false)
        .outcome(DiagnosisOutcome::NotRetriable)
        .retry_key(retry_key)
        .build();
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
}

pub(crate) fn record_wave_timeout_envelope(
    event_loop: &mut ralph_core::EventLoop,
    wave: &ralph_core::DetectedWave,
    completed: &ralph_core::CompletedWave,
    reason_code: &'static str,
) {
    use ralph_core::diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
        RecoveryDiagnosisEnvelopeBuilder,
    };

    // Plan §5 B3: RecoveryDiagnosisEnvelope has no independent
    // `wave_id` / `expected` / `completed` fields. We encode the
    // wave identity via the wave-scoped `retry_key` and stash the
    // counts in the human-readable `message` (truncated to
    // `MAX_ENVELOPE_MESSAGE_CHARS` by `build`).
    let topic = wave
        .hat_config
        .publishes
        .first()
        .cloned()
        .or_else(|| wave.events.first().map(|e| e.topic.clone()))
        .unwrap_or_default();

    let expected = completed.wave_total as usize;
    let actual = completed.results.len() + completed.failures.len();
    let duration_ms = completed.duration.as_millis() as u64;

    let retry_key = RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(&wave.wave_id, reason_code);
    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::WaveDispatcher)
        .severity(DiagnosisSeverity::Warning)
        .source_hat(wave.target_hat.to_string())
        .topic(topic)
        .reason_code(reason_code)
        .message(format!(
            "Wave {} timeout: {}/{} workers reported in {}ms (reason={})",
            wave.wave_id, actual, expected, duration_ms, reason_code
        ))
        .expected_action(
            "Investigate the slow wave workers; a subsequent complete wave on this target topic will mark this finding Recovered."
                .to_string(),
        )
        .retry_attempt(0)
        .safe_target(false)
        .outcome(DiagnosisOutcome::Pending)
        .retry_key(retry_key)
        .build();
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
}

pub(crate) fn record_wave_spawn_failed_envelope(
    event_loop: &mut ralph_core::EventLoop,
    loop_id: &str,
    wave: &ralph_core::DetectedWave,
    spawned_count: u32,
    expected_count: u32,
) {
    use ralph_core::diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
        RecoveryDiagnosisEnvelopeBuilder,
    };

    let topic = wave
        .hat_config
        .publishes
        .first()
        .cloned()
        .unwrap_or_default();

    let retry_key =
        RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(&wave.wave_id, "wave_spawn_failed");
    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::WaveDispatcher)
        .severity(DiagnosisSeverity::Error)
        .source_hat(wave.target_hat.to_string())
        .topic(topic)
        .reason_code("wave_spawn_failed")
        .message(format!(
            "Wave {} spawn guarantee violated: only {}/{} workers spawned (loop={})",
            wave.wave_id, spawned_count, expected_count, loop_id
        ))
        .expected_action(
            "Investigate why workers failed to spawn. This may indicate a system resource issue or dispatcher bug."
                .to_string(),
        )
        .retry_attempt(0)
        .safe_target(false)
        .outcome(DiagnosisOutcome::NotRetriable)
        .retry_key(retry_key)
        .build();
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
}
