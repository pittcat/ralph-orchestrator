//! Fan-in module — supervisor fan-in (`run_supervisor_fan_in`,
//! `SupervisorFanInOutcome`, `TerminalFanInContext`) and the post-tick
//! compensation drain (`drain_pending_compensations`).
//!
//! Originally part of `wave/dispatcher.rs` (plan `2026-08-07-008`).
//! Public surface and behaviour preserved verbatim.

use std::path::Path;
use std::sync::Arc;
use tracing::warn;

use ralph_core::CompletedWave;

use super::coordination::{
    CoordCommitOutcome, build_wave_complete_payload, commit_complete_coord_event,
    commit_failed_coord_event, emit_injected_failed_coord, unix_now_secs,
};
use super::outcomes::{compute_slot_batch_fingerprint, merge_round_into};
use super::salvage::{
    build_wave_failed_slots_json, merge_completed_exec_fix_slots_to_main,
    merge_completed_review_slots_to_main, project_empty_salvage, workspace_root_from_events,
    write_wave_diagnostics_json,
};

/// U6: outcome of a production supervisor fan-in tick. The
/// dispatcher logs this and uses `injected` to decide whether a
/// fresh coordination event landed in the ledger this round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorFanInOutcome {
    /// A fresh `*.wave.complete` was merged + injected.
    InjectedComplete,
    /// A fresh `*.wave.failed` was injected.
    InjectedFailed,
    /// The wave was already merged on a prior tick (KTD-7
    /// idempotency); no new coordination event.
    AlreadyDone,
    /// The wave is still collecting slots; nothing injected.
    ContinueCollect,
    /// The merge sink rejected the batch; `merged_to_events`
    /// stayed false so the next tick retries exactly once
    /// (KTD-7). No coordination event injected.
    MergeFailed,
    /// The bridge/store errored; logged, treated as no-op so the
    /// next tick retries.
    StoreError,
}

impl SupervisorFanInOutcome {
    /// True when a fresh coordination event was injected this tick.
    #[allow(dead_code)] // consumed by diagnostics + follow-up units
    pub(crate) fn injected(self) -> bool {
        matches!(
            self,
            SupervisorFanInOutcome::InjectedComplete | SupervisorFanInOutcome::InjectedFailed
        )
    }
}

/// U6: production supervisor fan-in. Merges the per-slot worker
/// business events (sorted by slot index, de-duplicated) into the
/// loop's main ledger via the bridge's production merge sink, then
/// injects the unique `*.wave.complete` / `*.wave.failed`
/// coordination event (with the successful slots' `branch` /
/// `worktree_path` payload).
///
/// Contract (KTD-6 / KTD-7):
/// - The merge gate is the coordinator's `tick_with_slot_events`:
///   on `Integrate` it appends the sorted business events through
///   the sink and flips `merged_to_events`. If the sink fails, the
///   wave stays in `Collect` and NO coordination event is injected
///   — the next tick retries the merge exactly once.
/// - `merged_to_events` makes the injection idempotent: once merged,
///   subsequent ticks return `AlreadyDone` and never re-inject.
/// - The coordination event is appended to the SAME ledger the sink
///   wrote to, marked `system_injected: true`, WITHOUT advancing the
///   reader cursor; the caller's post-wave `process_events_from_jsonl`
///   re-read publishes the business + coordination events to the bus
///   exactly once.
///
/// This function does NOT perform any Git merge (the integrator path
/// owns that); it only merges the JSONL event fan-in.
/// U1: context for driving a terminal supervisor fan-in to convergence.
/// When present, the fan-in helper knows it must drive through
/// ContinueCollect (by recording never-started slots and re-ticking)
/// rather than returning ContinueCollect as a no-op with no owner.
/// Exhaustion returns `StoreError` (mapped to `fan_in_failure` by the
/// caller) — never silent `AlreadyDone` without a coordination event.
#[derive(Debug, Clone)]
pub struct TerminalFanInContext {
    /// True when cancel was requested (global_deadline or
    /// AggregateDeadlineExceeded fired).
    pub(crate) cancel_requested: bool,
    /// Real elapsed time since the wave started.
    pub(crate) elapsed: std::time::Duration,
}

pub(crate) fn run_supervisor_fan_in(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    completed: &CompletedWave,
    detected: &ralph_core::DetectedWave,
    main_events_file: &Path,
    aggregate_timeout_secs: u64,
    terminal_ctx: Option<TerminalFanInContext>,
) -> SupervisorFanInOutcome {
    use ralph_core::supervisor::{SupervisorBridge as _, WaveKind};

    // `CompletedWave` is produced by the local tracker, while the
    // supervisor store uses an internal `w-*` row id.  Keep store access
    // on the latter, but make every business coordination payload use the
    // public id from the detected trigger wave.
    let mut coordination_completed = completed.clone();
    coordination_completed.wave_id = detected.wave_id.clone();

    // Worker execution may overlap across independent waves, but fan-in
    // appends to one event stream and commits one delivery state at a time.
    let fan_in_lock = bridge.fan_in_lock();
    let _fan_in_guard = match fan_in_lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            warn!(
                wave_id = %completed.wave_id,
                "supervisor fan-in lock was poisoned; continuing with recovered guard"
            );
            poisoned.into_inner()
        }
    };

    // Infer the wave kind from the trigger topic (mirrors
    // `execute_wave_via_supervisor_with_executor`).
    let trigger_topic = detected
        .events
        .first()
        .map(|e| e.topic.as_str())
        .unwrap_or("");
    // 2026-07-23-001 plan U9: widened `review.wave.` → `review.`
    // to keep the kind inference consistent between the spawn
    // path and this fan-in path. See the matching note on
    // `execute_wave_via_supervisor_with_executor` for why the
    // builtin preset's `review.unit.ready` trigger needs to be
    // classified Review.
    let wave_kind = if trigger_topic.starts_with("review.") {
        WaveKind::Review
    } else if trigger_topic.starts_with("fix.") {
        WaveKind::Fix
    } else {
        WaveKind::Exec
    };

    // Re-derive the store-assigned wave id idempotently. The
    // dispatcher's supervisor spawn path already registered the wave
    // under `completed.wave_id`; `register_wave_if_absent` returns
    // the existing store id on re-entry so the coordinator reads the
    // same row the slot results were recorded against.
    //
    // 2026-07-28-003 plan U4 (R14 / S13): mirror of the spawn
    // path's registration call; reads the budget from the bridge
    // so the two `register_wave_if_absent` calls always agree
    // on the same value.
    let store_wave_id = match bridge.register_wave_if_absent(
        wave_kind,
        &completed.wave_id,
        completed.wave_total,
        bridge.slot_retry_budget(),
    ) {
        Ok(id) => id,
        Err(err) => {
            warn!(
                wave_id = %completed.wave_id,
                error = %err,
                "U6: supervisor register_wave_if_absent failed during fan-in"
            );
            return SupervisorFanInOutcome::StoreError;
        }
    };

    // U1 (Green 1 / Green 3): when cancel_requested is true (AggregateDeadlineExceeded
    // path), mark the store wave as cancelled so evaluate_phase sees the flag
    // and returns Failed immediately on the first tick.
    if let Some(ctx) = terminal_ctx.as_ref()
        && ctx.cancel_requested
        && let Err(err) = bridge.cancel_wave(&store_wave_id)
    {
        warn!(
            wave_id = %completed.wave_id,
            error = %err,
            "U1: cancel_wave failed during terminal fan-in"
        );
    }

    // Gather the per-slot business events, ordered by slot index and
    // de-duplicated by (topic, payload). Sorting by `WaveResult.index`
    // gives the deterministic slot-index order the plan requires; the
    // dedup keeps the main ledger free of repeated business events
    // when two slots emit an identical record.
    let mut results_by_index: Vec<&ralph_core::WaveResult> = completed.results.iter().collect();
    results_by_index.sort_by_key(|r| r.index);
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut slot_events: Vec<ralph_proto::Event> = Vec::new();
    for result in results_by_index {
        for event in &result.events {
            let key = (event.topic.as_str().to_string(), event.payload.clone());
            if seen.insert(key) {
                slot_events.push(event.clone());
            }
        }
    }

    let inputs = ralph_core::supervisor::PhaseInputs {
        aggregate_timeout_secs,
        elapsed_secs: terminal_ctx
            .as_ref()
            .map(|ctx| ctx.elapsed.as_secs())
            .unwrap_or(0),
        cancel_requested: terminal_ctx
            .as_ref()
            .is_some_and(|ctx| ctx.cancel_requested),
    };

    // The coordinator is the merge gate: on `Integrate` it appends
    // `slot_events` through the production sink and flips
    // `merged_to_events`. Sink failure → `MergeFailed` (no injection,
    // retry next tick).
    let slot_events_for_retry = slot_events.clone();
    let action = match bridge.tick_with_slot_events(&store_wave_id, inputs.clone(), slot_events) {
        Ok(action) => action,
        Err(err) => {
            warn!(
                wave_id = %completed.wave_id,
                store_wave_id = %store_wave_id,
                error = %err,
                "U6: supervisor tick_with_slot_events failed during fan-in"
            );
            return SupervisorFanInOutcome::StoreError;
        }
    };

    // ── 2026-07-25-005 plan U1 (R3 / R4 / KTD2 / KTD6) ────────────────
    // Exec/Fix partial-failure settlement. The coordinator's pure
    // phase function only reaches `Failed` once EVERY slot is
    // terminal; a wave whose worker batch has finished but which
    // still carries (a) a permanently Failed slot plus (b) slots
    // that never reported anything would otherwise sit in
    // `ContinueCollect` forever (the coordinator keeps waiting for
    // workers that will never report). Fan-in runs after the wave's
    // worker batch completes, so those silent slots can be settled
    // forward-only as `slot_never_started` (KTD5: no visible
    // rollback) and the coordinator then owns the Failed verdict,
    // the wave-phase flip and the coord-injection latch as usual.
    //
    // The `SalvageNotMerged` half: production exec/fix waves never
    // pre-mark the salvage, so `fail_wave` refuses the first tick.
    // We perform the completed-only salvage merge here (KTD6 order:
    // append completed events, THEN fail), commit the mark, and
    // re-tick exactly once so the coordinator latches the failure.
    //
    // Review waves keep their existing flow untouched.
    let mut exec_fix_salvage_written = false;
    let action = if matches!(wave_kind, WaveKind::Exec | WaveKind::Fix)
        && matches!(
            action,
            ralph_core::supervisor::CoordinatorAction::ContinueCollect
                | ralph_core::supervisor::CoordinatorAction::SalvageNotMerged
        ) {
        use ralph_core::supervisor::SlotStatus;
        let settle_snapshot = bridge.fan_in_status(&store_wave_id).ok();
        let has_blocking = settle_snapshot.as_ref().is_some_and(|snap| {
            snap.slots
                .iter()
                .any(|(_, status)| matches!(status, SlotStatus::Failed | SlotStatus::Cancelled))
        });
        if has_blocking {
            // (a) Salvage completed-only business events into the
            // main ledger and commit the salvage mark (also covers
            // the zero-completed case: nothing to append, mark
            // still commits so fail_wave's gate can open).
            if let Err(err) = merge_completed_exec_fix_slots_to_main(
                main_events_file,
                &coordination_completed,
                bridge,
                &store_wave_id,
            ) {
                warn!(
                    wave_id = %completed.wave_id,
                    store_wave_id = %store_wave_id,
                    error = %err,
                    "U5: exec/fix salvage merge failed; refusing to advance delivery state"
                );
                return SupervisorFanInOutcome::StoreError;
            }
            exec_fix_salvage_written = true;
            // (b) Settle slots the finished batch left non-terminal:
            // they will never report. First-terminal-wins makes this
            // idempotent for slots that raced into a terminal state
            // between the snapshot read and this record.
            if let Some(snap) = settle_snapshot.as_ref() {
                for (slot_index, status) in &snap.slots {
                    if matches!(
                        status,
                        SlotStatus::Completed | SlotStatus::Failed | SlotStatus::Cancelled
                    ) {
                        continue;
                    }
                    if let Err(err) = bridge.record_slot_failure(
                        &store_wave_id,
                        *slot_index,
                        ralph_core::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED,
                    ) {
                        warn!(
                            wave_id = %completed.wave_id,
                            slot_index = *slot_index,
                            error = %err,
                            "U1: record_slot_failure(slot_never_started) failed during \
                             exec/fix partial-failure settlement"
                        );
                    }
                }
            }
            // (c) Re-tick: the coordinator now sees every slot
            // terminal with at least one failure and returns
            // `InjectedFailed` (or `AlreadyDone` on a racing latch).
            match bridge.tick_with_slot_events(&store_wave_id, inputs, Vec::new()) {
                Ok(retried) => retried,
                Err(err) => {
                    warn!(
                        wave_id = %completed.wave_id,
                        store_wave_id = %store_wave_id,
                        error = %err,
                        "U1: re-tick after exec/fix partial-failure settlement failed"
                    );
                    return SupervisorFanInOutcome::StoreError;
                }
            }
        } else {
            action
        }
    } else {
        action
    };

    let action_outcome = match action {
        ralph_core::supervisor::CoordinatorAction::InjectedComplete { topic, .. } => {
            // U7 (2026-07-23-002): build the wave-coordination payload
            // shape that matches the **target topic's** schema. The
            // earlier implementation hard-coded the exec-style payload
            // (`completed_slots` / `success_slots` / `merge_root_event_id`)
            // for every wave kind, but `review.wave.complete` and
            // `fix.wave.complete` have different required_fields — see
            // the surviving supervisor-enabled builtin
            // `presets/en/parallel-forge.yml` (plan 2026-08-09-001
            // removed `ce-executor-supervisor`). A mismatched
            // payload was rejected by the engine gate's required_fields
            // check, the event was demoted to `MalformedLine`, and the
            // downstream integrator hat (e.g. `review-synthesizer`)
            // never woke up. The hard-gate counter then terminated the
            // loop after three iterations with no events emitted.
            let payload = build_wave_complete_payload(
                wave_kind,
                &coordination_completed,
                &store_wave_id,
                bridge,
                aggregate_timeout_secs,
            );
            match commit_complete_coord_event(
                bridge,
                main_events_file,
                &store_wave_id,
                &topic,
                &payload,
            ) {
                CoordCommitOutcome::Committed => SupervisorFanInOutcome::InjectedComplete,
                CoordCommitOutcome::StoreError => SupervisorFanInOutcome::StoreError,
            }
        }
        ralph_core::supervisor::CoordinatorAction::InjectedFailed {
            topic,
            reason,
            blocking_slots,
        } => emit_injected_failed_coord(
            bridge,
            wave_kind,
            &coordination_completed,
            &store_wave_id,
            main_events_file,
            &topic,
            reason,
            blocking_slots,
            exec_fix_salvage_written,
        ),
        ralph_core::supervisor::CoordinatorAction::AlreadyDone => {
            SupervisorFanInOutcome::AlreadyDone
        }
        ralph_core::supervisor::CoordinatorAction::SalvageNotMerged => {
            // 2026-07-27-003 plan U5: when the coordinator refuses
            // because salvage isn't committed, we still need to
            // surface a salvage receipt. The runtime cannot
            // fabricate one without a real write — delegate to
            // `project_empty_salvage` for the all-failed case,
            // otherwise call `merge_completed_*_slots_to_main` so
            // the merge seam produces a real receipt.
            let snap = bridge.fan_in_status(&store_wave_id).ok();
            let salvage_outcome = if matches!(wave_kind, ralph_core::supervisor::WaveKind::Review) {
                merge_completed_review_slots_to_main(
                    main_events_file,
                    &coordination_completed,
                    bridge,
                    &store_wave_id,
                )
            } else {
                merge_completed_exec_fix_slots_to_main(
                    main_events_file,
                    &coordination_completed,
                    bridge,
                    &store_wave_id,
                )
            };
            if let Err(err) = salvage_outcome {
                warn!(
                    wave_id = %completed.wave_id,
                    error = %err,
                    "U5: salvage projection failed during SalvageNotMerged recovery"
                );
                if let Some(snap) = snap.as_ref() {
                    let _ = project_empty_salvage(snap, &store_wave_id);
                }
                return SupervisorFanInOutcome::StoreError;
            }
            let retry_inputs = ralph_core::supervisor::PhaseInputs {
                aggregate_timeout_secs,
                elapsed_secs: terminal_ctx
                    .as_ref()
                    .map(|ctx| ctx.elapsed.as_secs())
                    .unwrap_or(0),
                cancel_requested: terminal_ctx
                    .as_ref()
                    .is_some_and(|ctx| ctx.cancel_requested),
            };
            let retry_action = match bridge.tick_with_slot_events(
                &store_wave_id,
                retry_inputs,
                slot_events_for_retry.clone(),
            ) {
                Ok(a) => a,
                Err(err) => {
                    warn!(wave_id = %completed.wave_id, error = %err, "U1: retry tick after SalvageNotMerged failed");
                    return SupervisorFanInOutcome::StoreError;
                }
            };
            match retry_action {
                ralph_core::supervisor::CoordinatorAction::InjectedFailed {
                    topic,
                    reason,
                    blocking_slots,
                } => emit_injected_failed_coord(
                    bridge,
                    wave_kind,
                    &coordination_completed,
                    &store_wave_id,
                    main_events_file,
                    &topic,
                    reason,
                    blocking_slots,
                    exec_fix_salvage_written,
                ),
                ralph_core::supervisor::CoordinatorAction::InjectedComplete { topic, .. } => {
                    let payload = build_wave_complete_payload(
                        wave_kind,
                        &coordination_completed,
                        &store_wave_id,
                        bridge,
                        aggregate_timeout_secs,
                    );
                    match commit_complete_coord_event(
                        bridge,
                        main_events_file,
                        &store_wave_id,
                        &topic,
                        &payload,
                    ) {
                        CoordCommitOutcome::Committed => SupervisorFanInOutcome::InjectedComplete,
                        CoordCommitOutcome::StoreError => SupervisorFanInOutcome::StoreError,
                    }
                }
                ralph_core::supervisor::CoordinatorAction::AlreadyDone => {
                    SupervisorFanInOutcome::AlreadyDone
                }
                ralph_core::supervisor::CoordinatorAction::ContinueCollect
                | ralph_core::supervisor::CoordinatorAction::SalvageNotMerged
                | ralph_core::supervisor::CoordinatorAction::MergeFailed { .. } => {
                    // Terminal salvage retry exhausted without a coordination
                    // event. Fail-close — never mark AlreadyDone without inject
                    // (that recreates the orphan ContinueCollect hang).
                    warn!(
                        wave_id = %completed.wave_id,
                        "U1: terminal SalvageNotMerged retry exhausted without \
                         InjectedFailed/Complete; returning StoreError"
                    );
                    SupervisorFanInOutcome::StoreError
                }
            }
        }
        ralph_core::supervisor::CoordinatorAction::ContinueCollect => {
            // U1 (Green 6): terminal_ctx is set and first tick returned ContinueCollect.
            // Record never-started failures, then drive the four-phase
            // commit (salvage → coord → commit) by calling the merge
            // seam directly so the next tick observes the receipt.
            if terminal_ctx.is_some() {
                if let Err(err) = bridge.record_never_started_failures(&store_wave_id) {
                    warn!(wave_id = %completed.wave_id, error = %err, "U1: record_never_started_failures failed");
                }
                // 2026-07-27-003 plan U5: invoke the merge seam
                // (which commits the salvage receipt as a
                // side-effect) before the retry tick so the
                // coordinator's gate opens.
                let salvage = if matches!(wave_kind, ralph_core::supervisor::WaveKind::Review) {
                    merge_completed_review_slots_to_main(
                        main_events_file,
                        &coordination_completed,
                        bridge,
                        &store_wave_id,
                    )
                } else {
                    merge_completed_exec_fix_slots_to_main(
                        main_events_file,
                        &coordination_completed,
                        bridge,
                        &store_wave_id,
                    )
                };
                if let Err(err) = salvage {
                    warn!(
                        wave_id = %completed.wave_id,
                        error = %err,
                        "U5: salvage projection failed during terminal ContinueCollect recovery"
                    );
                    return SupervisorFanInOutcome::StoreError;
                }
                let retry_inputs = ralph_core::supervisor::PhaseInputs {
                    aggregate_timeout_secs,
                    elapsed_secs: terminal_ctx
                        .as_ref()
                        .map(|ctx| ctx.elapsed.as_secs())
                        .unwrap_or(0),
                    cancel_requested: terminal_ctx
                        .as_ref()
                        .is_some_and(|ctx| ctx.cancel_requested),
                };
                let retry_action = match bridge.tick_with_slot_events(
                    &store_wave_id,
                    retry_inputs,
                    slot_events_for_retry.clone(),
                ) {
                    Ok(a) => a,
                    Err(err) => {
                        warn!(wave_id = %completed.wave_id, error = %err, "U1: second tick failed");
                        return SupervisorFanInOutcome::StoreError;
                    }
                };
                match retry_action {
                    ralph_core::supervisor::CoordinatorAction::InjectedFailed {
                        topic,
                        reason,
                        blocking_slots,
                    } => emit_injected_failed_coord(
                        bridge,
                        wave_kind,
                        &coordination_completed,
                        &store_wave_id,
                        main_events_file,
                        &topic,
                        reason,
                        blocking_slots,
                        exec_fix_salvage_written,
                    ),
                    ralph_core::supervisor::CoordinatorAction::InjectedComplete {
                        topic, ..
                    } => {
                        let payload = build_wave_complete_payload(
                            wave_kind,
                            &coordination_completed,
                            &store_wave_id,
                            bridge,
                            aggregate_timeout_secs,
                        );
                        match commit_complete_coord_event(
                            bridge,
                            main_events_file,
                            &store_wave_id,
                            &topic,
                            &payload,
                        ) {
                            CoordCommitOutcome::Committed => {
                                SupervisorFanInOutcome::InjectedComplete
                            }
                            CoordCommitOutcome::StoreError => SupervisorFanInOutcome::StoreError,
                        }
                    }
                    ralph_core::supervisor::CoordinatorAction::AlreadyDone => {
                        SupervisorFanInOutcome::AlreadyDone
                    }
                    _ => {
                        warn!(
                            wave_id = %completed.wave_id,
                            "U1: terminal ContinueCollect retry exhausted without \
                             InjectedFailed/Complete; returning StoreError"
                        );
                        SupervisorFanInOutcome::StoreError
                    }
                }
            } else {
                SupervisorFanInOutcome::ContinueCollect
            }
        }
        ralph_core::supervisor::CoordinatorAction::MergeFailed { topic, error } => {
            // U1 (Green 8): bounded retry on the same merge seam when this
            // call is the final terminal fan-in (no next tick owner).
            if terminal_ctx.is_some() {
                warn!(
                    wave_id = %completed.wave_id,
                    topic = %topic,
                    error = %error,
                    "U1: terminal merge sink rejected; retrying once"
                );
                let retry_inputs = ralph_core::supervisor::PhaseInputs {
                    aggregate_timeout_secs,
                    elapsed_secs: terminal_ctx
                        .as_ref()
                        .map(|ctx| ctx.elapsed.as_secs())
                        .unwrap_or(0),
                    cancel_requested: terminal_ctx
                        .as_ref()
                        .is_some_and(|ctx| ctx.cancel_requested),
                };
                match bridge.tick_with_slot_events(
                    &store_wave_id,
                    retry_inputs,
                    slot_events_for_retry,
                ) {
                    Ok(ralph_core::supervisor::CoordinatorAction::InjectedComplete {
                        topic,
                        ..
                    }) => {
                        let payload = build_wave_complete_payload(
                            wave_kind,
                            &coordination_completed,
                            &store_wave_id,
                            bridge,
                            aggregate_timeout_secs,
                        );
                        match commit_complete_coord_event(
                            bridge,
                            main_events_file,
                            &store_wave_id,
                            &topic,
                            &payload,
                        ) {
                            CoordCommitOutcome::Committed => {
                                SupervisorFanInOutcome::InjectedComplete
                            }
                            CoordCommitOutcome::StoreError => SupervisorFanInOutcome::StoreError,
                        }
                    }
                    Ok(ralph_core::supervisor::CoordinatorAction::InjectedFailed {
                        topic,
                        reason,
                        blocking_slots,
                    }) => emit_injected_failed_coord(
                        bridge,
                        wave_kind,
                        &coordination_completed,
                        &store_wave_id,
                        main_events_file,
                        &topic,
                        reason,
                        blocking_slots,
                        exec_fix_salvage_written,
                    ),
                    Ok(ralph_core::supervisor::CoordinatorAction::AlreadyDone) => {
                        SupervisorFanInOutcome::AlreadyDone
                    }
                    Ok(_) | Err(_) => {
                        warn!(
                            wave_id = %completed.wave_id,
                            "U1: terminal MergeFailed retry exhausted; returning StoreError"
                        );
                        SupervisorFanInOutcome::StoreError
                    }
                }
            } else {
                warn!(
                    wave_id = %completed.wave_id,
                    topic = %topic,
                    error = %error,
                    "U6: supervisor merge sink rejected the batch; \
                     merged_to_events stays false, retrying on next tick (KTD-7)"
                );
                SupervisorFanInOutcome::MergeFailed
            }
        }
    };

    // 2026-07-22-001 plan U6: every successful fan-in tick drains
    // any pending compensation jobs (OnTimeout / OnCancel /
    // OnPartial) and marks them executed. We do this after the
    // coordinator action has been processed so a wave that just
    // got marked cancelled observes the new phase before its
    // compensation hook runs. Failures only warn — the wave's
    // terminal phase still succeeds.
    drain_pending_compensations(bridge);

    action_outcome
}

/// 2026-07-22-001 plan U6 (KTD-7): drain any pending
/// compensation jobs and mark them executed. The
/// compensation-hook command itself is a no-op for now — we
/// record stderr diagnostics so an operator scanning loop
/// output sees exactly which waves triggered which
/// compensation kind. Failures only warn; they do not block
/// the wave's terminal phase (KTD-7).
pub(crate) fn drain_pending_compensations(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
) {
    use ralph_core::supervisor::SupervisorBridge as _;
    let pending = match bridge.take_pending_compensations() {
        Ok(p) => p,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "supervisor take_pending_compensations returned an error; \
                 treating as empty queue"
            );
            return;
        }
    };
    for (wave_id, kind) in pending {
        // The "hook" itself is a stderr diagnostic record.
        // Hook command execution (e.g. cleaning up the
        // wave's worktree branch) lands in a follow-up
        // release; today we mark the job executed so a
        // subsequent inspect surfaces its terminal status.
        let kind_str = match kind {
            ralph_core::supervisor::CompensationKind::OnTimeout => "timeout",
            ralph_core::supervisor::CompensationKind::OnCancel => "cancel",
            ralph_core::supervisor::CompensationKind::OnPartial => "partial",
        };
        tracing::info!(
            wave_id = %wave_id,
            kind = kind_str,
            "supervisor compensation hook executed (2026-07-22-001 plan U6)"
        );
        if let Err(err) = bridge.complete_compensation(&wave_id, kind, true) {
            tracing::warn!(
                wave_id = %wave_id,
                kind = kind_str,
                error = %err,
                "supervisor complete_compensation failed; \
                 the job will be retried on the next drain"
            );
        }
    }
}
