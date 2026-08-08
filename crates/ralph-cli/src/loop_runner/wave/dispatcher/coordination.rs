//! Coordination module — coord payloads, coord-event commit helpers, shared emit_injected_failed_coord side-effects,
//! cross-source review hints, and the coord-event append + fingerprint helpers.
//! Originally part of `wave/dispatcher.rs` (plan `2026-08-07-008`).
//! Public surface and behaviour preserved verbatim.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::warn;

use ralph_core::CompletedWave;

use super::fan_in::SupervisorFanInOutcome;
use super::salvage::{
    build_wave_failed_slots_json, merge_completed_exec_fix_slots_to_main,
    merge_completed_review_slots_to_main, workspace_root_from_events, write_wave_diagnostics_json,
};

/// Shared InjectedFailed side-effects: never-started recording,
/// diagnostics, Completed-only salvage merge, and coord append.
pub(crate) fn emit_injected_failed_coord(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    wave_kind: ralph_core::supervisor::WaveKind,
    completed: &CompletedWave,
    store_wave_id: &str,
    main_events_file: &Path,
    topic: &str,
    reason: &str,
    blocking_slots: Vec<u32>,
    exec_fix_salvage_written: bool,
) -> SupervisorFanInOutcome {
    use ralph_core::supervisor::SupervisorBridge as _;

    if let Err(err) = bridge.record_never_started_failures(store_wave_id) {
        warn!(
            wave_id = %completed.wave_id,
            error = %err,
            "U1: record_never_started_failures failed during fan-in; \
             continuing anyway — the wave failure is already recorded"
        );
    }
    let snap_for_reasons = bridge.fan_in_status(store_wave_id);
    let mut reasons: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    if let Ok(snap) = snap_for_reasons.as_ref() {
        use ralph_core::supervisor::SlotStatus;
        for (idx, status) in &snap.slots {
            if matches!(status, SlotStatus::Failed | SlotStatus::Cancelled) {
                match bridge.slot_failure_reason(store_wave_id, *idx) {
                    Ok(Some(r)) => {
                        reasons.insert(*idx, r);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        warn!(
                            wave_id = %store_wave_id,
                            slot_index = *idx,
                            error = %err,
                            "U5: slot_failure_reason lookup failed; \
                             payload keeps reason=null for this slot"
                        );
                    }
                }
            }
        }
    }
    if let Ok(snap) = snap_for_reasons.as_ref() {
        let elapsed_secs = snap.started_at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        let payload =
            build_wave_failed_slots_json(&completed.wave_id, &snap.slots, &reasons, elapsed_secs);
        if let Err(err) = write_wave_diagnostics_json(
            &workspace_root_from_events(main_events_file),
            &completed.wave_id,
            &payload,
        ) {
            warn!(
                wave_id = %completed.wave_id,
                error = %err,
                "U5: write_wave_diagnostics_json failed (best-effort)"
            );
        }
    }
    if matches!(wave_kind, ralph_core::supervisor::WaveKind::Review) {
        if let Err(err) =
            merge_completed_review_slots_to_main(main_events_file, completed, bridge, store_wave_id)
        {
            warn!(
                wave_id = %completed.wave_id,
                store_wave_id = %store_wave_id,
                error = %err,
                "U5: review salvage merge failed during InjectedFailed path; refusing to append coord event"
            );
            return SupervisorFanInOutcome::StoreError;
        }
    } else if !exec_fix_salvage_written {
        // 2026-07-25-005 plan U1 (R3 / KTD6): exec/fix waves salvage
        // their Completed slots' business events before the coord
        // event, same ordering contract as the review arm. Skipped
        // when the settlement block above already wrote the salvage on
        // this tick (it re-ticked the coordinator to reach this arm),
        // so completed events are never double-appended.
        if let Err(err) = merge_completed_exec_fix_slots_to_main(
            main_events_file,
            completed,
            bridge,
            store_wave_id,
        ) {
            warn!(
                wave_id = %completed.wave_id,
                store_wave_id = %store_wave_id,
                error = %err,
                "U5: exec/fix salvage merge failed during InjectedFailed path; refusing to append coord event"
            );
            return SupervisorFanInOutcome::StoreError;
        }
    }
    let review_done_hints =
        build_review_done_hints(bridge, store_wave_id, completed, main_events_file);
    // 2026-07-27-003 plan U4 (KTD3 / R5 / R7): the only
    // authoritative source of completion is the supervisor store's
    // terminal evidence. Main ledger is treated as a projection
    // observation (orphan / conflict) and never as completion.
    // `build_review_done_hints` is preserved for the U3 bounded
    // backscan assertions; the failed-payload builder now uses the
    // reconciliation's `authoritative_completed` directly.
    let reconciliation =
        build_review_reconciliation(bridge, store_wave_id, completed, main_events_file);
    let payload = build_wave_failed_payload(
        wave_kind,
        completed,
        reason,
        blocking_slots,
        &reasons,
        Some(&review_done_hints),
        reconciliation.as_ref(),
    );
    match commit_failed_coord_event(bridge, main_events_file, store_wave_id, topic, &payload) {
        CoordCommitOutcome::Committed => SupervisorFanInOutcome::InjectedFailed,
        CoordCommitOutcome::StoreError => SupervisorFanInOutcome::StoreError,
    }
}

pub(crate) fn commit_complete_coord_event(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    main_events_file: &Path,
    store_wave_id: &str,
    topic: &str,
    payload: &serde_json::Value,
) -> CoordCommitOutcome {
    let receipt = match append_supervisor_coord_event(main_events_file, topic, payload) {
        Ok(receipt) => receipt,
        Err(err) => {
            warn!(
                wave_id = %store_wave_id,
                topic = %topic,
                error = %err,
                "U5: append_supervisor_coord_event failed; refusing to commit"
            );
            return CoordCommitOutcome::StoreError;
        }
    };
    let summary = coordination_summary_from_receipt(&receipt);
    if let Err(err) = bridge.record_coordination_written(store_wave_id, &summary) {
        warn!(
            wave_id = %store_wave_id,
            error = %err,
            "U5: record_coordination_written failed; refusing to commit"
        );
        return CoordCommitOutcome::StoreError;
    }
    if let Err(err) = bridge.commit_coordination_event(
        store_wave_id,
        &summary,
        ralph_core::supervisor::WavePhase::Done,
    ) {
        warn!(
            wave_id = %store_wave_id,
            error = %err,
            "U5: commit_coordination_event(Done) failed; refusing to mark InjectedComplete"
        );
        return CoordCommitOutcome::StoreError;
    }
    CoordCommitOutcome::Committed
}

pub(crate) fn commit_failed_coord_event(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    main_events_file: &Path,
    store_wave_id: &str,
    topic: &str,
    payload: &serde_json::Value,
) -> CoordCommitOutcome {
    let receipt = match append_supervisor_coord_event(main_events_file, topic, payload) {
        Ok(receipt) => receipt,
        Err(err) => {
            warn!(
                wave_id = %store_wave_id,
                topic = %topic,
                error = %err,
                "U5: append_supervisor_coord_event failed on failed path; refusing to commit"
            );
            return CoordCommitOutcome::StoreError;
        }
    };
    let summary = coordination_summary_from_receipt(&receipt);
    if let Err(err) = bridge.record_coordination_written(store_wave_id, &summary) {
        warn!(
            wave_id = %store_wave_id,
            error = %err,
            "U5: record_coordination_written failed on failed path; refusing to commit"
        );
        return CoordCommitOutcome::StoreError;
    }
    if let Err(err) = bridge.commit_coordination_event(
        store_wave_id,
        &summary,
        ralph_core::supervisor::WavePhase::Failed,
    ) {
        warn!(
            wave_id = %store_wave_id,
            error = %err,
            "U5: commit_coordination_event(Failed) failed; refusing to mark InjectedFailed"
        );
        return CoordCommitOutcome::StoreError;
    }
    CoordCommitOutcome::Committed
}

pub(crate) fn coordination_summary_from_receipt(
    receipt: &ralph_core::supervisor::CoordinationReceipt,
) -> ralph_core::supervisor::CoordinationReceiptSummary {
    ralph_core::supervisor::CoordinationReceiptSummary {
        topic: receipt.topic.clone(),
        idempotency_key: receipt.idempotency_key.clone(),
        payload_fingerprint: receipt.payload_fingerprint.clone(),
        write_count: receipt.write_count,
        already_present_count: receipt.already_present_count,
        committed_at_unix_secs: receipt.committed_at_unix_secs,
    }
}

pub(crate) enum CoordCommitOutcome {
    Committed,
    StoreError,
}

/// U7 (2026-07-23-002): build the `*.wave.complete` payload that
/// matches the **target topic's** schema — see the surviving
/// supervisor-enabled builtin `presets/en/parallel-forge.yml`
/// (plan 2026-08-09-001 removed `ce-executor-supervisor`).
///
/// - `exec.wave.complete` / `fix.wave.complete` require
///   `wave_id`, `completed_slots`, `merge_root_event_id`. The
///   payload also carries `success_slots` (per-slot branch +
///   worktree_path) so the integrator knows which branches to
///   merge.
/// - `review.wave.complete` requires `wave_id`,
///   `completed_dimensions`, `aggregate_timeout`. The
///   dimensions are derived from the per-slot `review.unit.done`
///   events (falling back to `assigned_dimensions` when the
///   events do not carry a `dimension` field).
pub(crate) fn build_wave_complete_payload(
    wave_kind: ralph_core::supervisor::WaveKind,
    completed: &ralph_core::CompletedWave,
    store_wave_id: &str,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    aggregate_timeout_secs: u64,
) -> serde_json::Value {
    use ralph_core::supervisor::{SupervisorBridge as _, WaveKind};

    match wave_kind {
        WaveKind::Review => {
            let completed_dimensions = collect_review_dimensions(completed);
            serde_json::json!({
                "wave_id": completed.wave_id,
                "completed_dimensions": completed_dimensions,
                "aggregate_timeout": aggregate_timeout_secs,
            })
        }
        WaveKind::Exec | WaveKind::Fix => {
            // Build the `success_slots` payload from the store's
            // per-slot resource bindings, filtered to the slots that
            // actually completed this wave. Each entry carries the
            // slot index + branch + worktree_path so the integrator
            // knows which branches to merge.
            let success_indices: std::collections::HashSet<u32> =
                completed.results.iter().map(|r| r.index).collect();
            let mut success_slots: Vec<serde_json::Value> = Vec::new();
            match bridge.slot_resources(store_wave_id) {
                Ok(resources) => {
                    let mut resources = resources;
                    resources.sort_by_key(|r| r.slot_index);
                    for res in resources {
                        if !success_indices.contains(&res.slot_index) {
                            continue;
                        }
                        success_slots.push(serde_json::json!({
                            "slot_index": res.slot_index,
                            "branch": res.branch,
                            "worktree_path": res.worktree_path,
                        }));
                    }
                }
                Err(err) => {
                    warn!(
                        wave_id = %completed.wave_id,
                        error = %err,
                        "U6: slot_resources failed; success_slots payload will be empty"
                    );
                }
            }
            let topic_prefix = match wave_kind {
                WaveKind::Exec => "exec",
                WaveKind::Fix => "fix",
                WaveKind::Review => "review",
            };
            serde_json::json!({
                "wave_id": completed.wave_id,
                "completed_slots": success_slots.len(),
                "success_slots": success_slots,
                "merge_root_event_id": format!("fan-in:{topic_prefix}.wave.complete:{}", completed.wave_id),
            })
        }
    }
}

/// U7 (2026-07-23-002): build the `*.wave.failed` payload that
/// matches the **target topic's** schema. Exec/fix waves carry
/// `blocking_slots`; review waves carry `missing_dimensions`
/// (the dimensions that never produced a `review.unit.done`).
///
/// 2026-07-26-003 plan U4 (KTD5): the Review arm now subtracts
/// already-known-done dimensions from three sources:
/// 1. `completed.results` — the in-progress fan-in channel
/// 2. the supervisor store's `Completed` rows
/// 3. the main ledger — `review.unit.done` events that the merge
///    sink already wrote before this fan-in reached
///    `InjectedFailed`. Before this widening the function only
///    subtracted source (1), so main-merged dimensions were
///    double-counted as missing (the primary-20260726 incident).
///
/// The `review_done_hints` parameter carries sources (2) and (3);
/// callers in `run_supervisor_fan_in` build it from the bridge
/// snapshot + a brief main-ledger tail scan. When `None`, the
/// function still produces a missing_dimensions array but only
/// subtracts from `completed.results` — useful for unit tests and
/// for callers that do not need the cross-source reconciliation.
pub(crate) fn build_wave_failed_payload(
    wave_kind: ralph_core::supervisor::WaveKind,
    completed: &ralph_core::CompletedWave,
    reason: &str,
    blocking_slots: Vec<u32>,
    reasons: &std::collections::HashMap<u32, String>,
    review_done_hints: Option<&ReviewDoneHints>,
    reconciliation: Option<&ralph_core::supervisor::reconciliation::ReviewReconciliation>,
) -> serde_json::Value {
    use ralph_core::supervisor::WaveKind;

    match wave_kind {
        WaveKind::Review => {
            let assigned: std::collections::HashSet<String> =
                completed.assigned_dimensions.values().cloned().collect();
            // 2026-07-27-003 plan U4 (KTD3 / R5 / R7): the
            // union-of-hints path is kept for the U3 bounded
            // backscan test and the diagnostics writer, but the
            // public `missing_dimensions` field is now driven
            // exclusively by the store-backed reconciliation.
            // Main-ledger backscan is now an orphan/conflict
            // signal; it can no longer reduce `missing_dimensions`
            // (the implementation-review primary-20260727 accident).
            let missing_dimensions = match reconciliation {
                Some(recon) => {
                    let authoritative_dims: std::collections::HashSet<String> = recon
                        .authoritative_completed
                        .iter()
                        .filter_map(|idx| completed.assigned_dimensions.get(idx).cloned())
                        .collect();
                    ralph_core::supervisor::reconciliation::compute_review_missing_dimensions(
                        &assigned,
                        &authoritative_dims,
                    )
                }
                None => {
                    let completed_dims = collect_review_dimensions(completed);
                    let mut already_done: std::collections::HashSet<String> =
                        completed_dims.into_iter().collect();
                    if let Some(hints) = review_done_hints {
                        already_done.extend(hints.main_backscan.iter().cloned());
                        already_done.extend(hints.store_completed.iter().cloned());
                    }
                    compute_review_missing_dimensions(&assigned, &already_done)
                }
            };
            // 2026-07-27-003 plan U4 (KTD5 / R8): when the
            // reconciliation surfaces a store/main disagreement
            // (orphan or payload conflict), prefer the stable
            // `wave_evidence_conflict` reason so operators can
            // pin root cause without parsing per-slot fields. The
            // public payload keeps the existing 3-field contract
            // (`wave_id` / `missing_dimensions` / `reason`); the
            // detailed evidence-validation report is written to
            // the structured diagnostics writer, not the event
            // payload.
            let reason = if reconciliation
                .map(|r| !r.orphan_projections.is_empty() || !r.payload_conflicts.is_empty())
                .unwrap_or(false)
            {
                ralph_core::supervisor::reconciliation::REASON_WAVE_EVIDENCE_CONFLICT
            } else {
                reason
            };
            serde_json::json!({
                "wave_id": completed.wave_id,
                "missing_dimensions": missing_dimensions,
                "reason": reason,
            })
        }
        WaveKind::Exec | WaveKind::Fix => {
            // 2026-07-25-003 plan U6 (R5 / R4) + 2026-07-26-002
            // plan U5 (R5 / KTD6): per-slot `slot_failures` is
            // derived from the supervisor store's frozen
            // `failure_reason` codes (NOT from `completed.failures`
            // free-form text), restricted to `blocking_slots` so
            // the index set agrees exactly. This is the SSOT for
            // downstream consumers (integrator / alignment /
            // reporter) — they no longer parse worker-written
            // `error` strings to tell a `worker_timeout` apart from
            // an `empty_worker_result`.
            //
            // 2026-07-25-005 plan U1 (R4 / R7 / KTD7): each entry
            // additionally carries a stable consumer-facing
            // `failure_class` label from `map_failure_class`, and
            // the payload gains two top-level index sets:
            //   - `salvaged_slots`: the wave's Completed slot
            //     indices (from `completed.results`, ascending) —
            //     business events already kept for the main ledger;
            //   - `redrive_slots`: `blocking_slots` restricted to
            //     retryable frozen reasons (ascending) — the only
            //     slots an operator redrive may reopen.
            use ralph_core::supervisor::worker_outcome::{
                is_retryable_slot_reason, map_failure_class,
            };

            let mut slot_failures: Vec<serde_json::Value> = Vec::new();
            let mut redrive_slots: Vec<u32> = Vec::new();
            for idx in &blocking_slots {
                let stored_reason = reasons.get(idx).cloned();
                let fallback_reason = completed
                    .failures
                    .iter()
                    .find(|f| f.index == *idx)
                    .map(|f| f.error.clone());
                let duration_ms = completed
                    .failures
                    .iter()
                    .find(|f| f.index == *idx)
                    .map(|f| f.duration.as_millis())
                    .unwrap_or(0);
                let reason = stored_reason.or(fallback_reason);
                // `failure_class` is computed from the same reason
                // string recorded in the entry (store code or
                // fallback), so per-slot fields never disagree.
                // A missing reason fail-closes to `unknown` and is
                // never retryable, so it stays out of redrive_slots.
                let (reason_value, failure_class) = match &reason {
                    Some(r) => (serde_json::json!(r), map_failure_class(r)),
                    None => (serde_json::Value::Null, map_failure_class("")),
                };
                if reason.as_deref().is_some_and(is_retryable_slot_reason) {
                    redrive_slots.push(*idx);
                }
                slot_failures.push(serde_json::json!({
                    "slot_index": idx,
                    "reason": reason_value,
                    "duration_ms": duration_ms,
                    "failure_class": failure_class,
                }));
            }
            redrive_slots.sort_unstable();
            // Completed slot indices, ascending. `Completed` never
            // enters `blocking_slots` (R5), so this set is disjoint
            // from the failure sets above.
            let mut salvaged_slots: Vec<u32> = completed.results.iter().map(|r| r.index).collect();
            salvaged_slots.sort_unstable();
            serde_json::json!({
                "wave_id": completed.wave_id,
                "reason": reason,
                "blocking_slots": blocking_slots,
                "slot_failures": slot_failures,
                "salvaged_slots": salvaged_slots,
                "redrive_slots": redrive_slots,
            })
        }
    }
}

/// 2026-07-26-003 plan U4: cross-source reconciliation hints for
/// the Review arm of `build_wave_failed_payload`. Filled by
/// `run_supervisor_fan_in` from the supervisor bridge snapshot
/// (store `Completed` rows) and a tight main-ledger tail scan.
/// These hint dimensions are subtracted from `missing_dimensions`
/// in addition to the in-progress `completed.results`.
pub struct ReviewDoneHints {
    /// Dimensions whose `review.unit.done` already lives in the
    /// main ledger from a previous fan-in tick (or any
    /// non-wave path that wrote directly into main). Computed
    /// by tail-scanning the main events file for the wave_id +
    /// `review.unit.done`.
    pub main_backscan: std::collections::HashSet<String>,
    /// Dimensions whose slots are `Completed` in the supervisor
    /// store with an associated `review.unit.done` event the
    /// dispatcher already absorbed into a sibling wave's
    /// `completed.results` blob.
    pub store_completed: std::collections::HashSet<String>,
}

pub(crate) fn compute_review_missing_dimensions(
    assigned: &std::collections::HashSet<String>,
    already_done: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut missing: Vec<String> = assigned
        .iter()
        .filter(|d| !already_done.contains(*d))
        .cloned()
        .collect();
    missing.sort();
    missing
}

pub(crate) fn collect_review_dimensions(completed: &ralph_core::CompletedWave) -> Vec<String> {
    let mut by_index: std::collections::BTreeMap<u32, String> = std::collections::BTreeMap::new();
    for result in &completed.results {
        for event in &result.events {
            if event.topic.as_str() != "review.unit.done" {
                continue;
            }
            let payload_str = event.payload.as_str();
            if !payload_str.is_empty()
                && let Ok(serde_json::Value::Object(map)) =
                    serde_json::from_str::<serde_json::Value>(payload_str)
                && let Some(serde_json::Value::String(dim)) = map.get("dimension")
            {
                by_index.insert(result.index, dim.clone());
                break;
            }
        }
        if !by_index.contains_key(&result.index)
            && let Some(dim) = completed.assigned_dimensions.get(&result.index)
        {
            by_index.insert(result.index, dim.clone());
        }
    }
    by_index.into_values().collect()
}

pub(crate) fn payload_object(
    payload: Option<&serde_json::Value>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let p = payload?;
    match p {
        serde_json::Value::Object(map) => Some(map.clone()),
        serde_json::Value::String(s) => {
            serde_json::from_str::<serde_json::Value>(s)
                .ok()
                .and_then(|v| match v {
                    serde_json::Value::Object(map) => Some(map),
                    _ => None,
                })
        }
        _ => None,
    }
}

pub(crate) fn build_review_done_hints(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    store_wave_id: &str,
    completed: &ralph_core::CompletedWave,
    main_events_file: &Path,
) -> ReviewDoneHints {
    use ralph_core::supervisor::SlotStatus;
    use std::io::BufRead;

    // --- main_backscan: same-wave `review.unit.done` already in main ---
    //
    // 2026-07-27-003 plan U4 (KTD3 / R5 / R7): pre-U4, this set
    // was a *raw* tail scan — every same-wave `review.unit.done`
    // row in main counted. The implementation-review
    // primary-20260727 incident turned that into the orphan trap:
    // store had 6 Failed slots, but main still carried 5
    // `review.unit.done` rows the dispatcher had previously
    // merged before scope-drop. `main_backscan` then claimed 5
    // dimensions were done, and `build_wave_failed_payload` only
    // reported 1 missing dimension.
    //
    // Post-U4: this helper still computes the raw scan (it is
    // the surface the U3 bounded-backscan test pins), but the
    // `main_backscan` set is *post-filtered* against the
    // store's authoritative set — a main row only counts when
    // the slot it references is `Completed` in the store with
    // valid terminal evidence. Rows whose slot is Failed /
    // Pending / unknown fall out and become orphan / conflict
    // observations in the new `ReviewReconciliation` instead.
    let mut main_backscan_raw = std::collections::HashSet::new();
    // Slot-indexed bookkeeping for the post-filter below.
    let mut main_backscan_dim_by_slot: std::collections::HashMap<u32, String> =
        std::collections::HashMap::new();
    let mut main_backscan_no_slot: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    if let Ok(file) = std::fs::File::open(main_events_file) {
        for line in std::io::BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if record.get("topic").and_then(|t| t.as_str()) != Some("review.unit.done") {
                continue;
            }
            // Bounded wave match: the envelope wave_id must equal this
            // wave. Rows without a wave_id (legacy / malformed) are NOT
            // counted — fail-closed.
            if record.get("wave_id").and_then(|w| w.as_str()) != Some(completed.wave_id.as_str()) {
                continue;
            }
            // Plan 004 P1-7: the main-ledger payload may arrive in two
            // shapes — string-encoded JSON (the legacy / agent
            // emit path) OR an inline JSON object (the
            // supervisor merge sink path, which writes object
            // payloads directly). The pre-fix code only
            // accepted the string form, so an object payload
            // was silently ignored and the dimension was
            // re-counted as missing. The fix: read whichever
            // shape is present via a unified accessor that
            // returns the inner payload object, then index
            // `dimension` directly.
            let map = match payload_object(record.get("payload")) {
                Some(m) => m,
                None => continue,
            };
            let dim = match map.get("dimension") {
                Some(serde_json::Value::String(d)) => d.clone(),
                _ => continue,
            };
            main_backscan_raw.insert(dim.clone());
            let slot_index = record
                .get("slot_index")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32);
            match slot_index {
                Some(idx) => {
                    main_backscan_dim_by_slot.insert(idx, dim);
                }
                None => {
                    main_backscan_no_slot.insert(dim);
                }
            }
        }
    }

    // --- store_completed: Completed slots WITH valid terminal evidence ---
    //
    // Plan 004 P1-6 / KTD3 fail-closed: terminal evidence is
    // bound to (topic, dimension, slot_index) — it MUST match
    // the wave kind's terminal topic AND carry a dimension AND
    // that dimension must equal the slot's assigned dimension.
    // Any mismatch (wrong topic, missing dimension, dimension
    // mismatch) drops the slot from `done` so the dispatcher
    // cannot under-report `missing_dimensions` by smuggling in
    // unrelated events as terminal evidence.
    let mut store_completed = std::collections::HashSet::new();
    let mut authoritative_slot_indices: std::collections::HashSet<u32> =
        std::collections::HashSet::new();
    if let Ok(snap) = bridge.fan_in_status(store_wave_id) {
        for (slot_index, status) in &snap.slots {
            if !matches!(status, SlotStatus::Completed) {
                continue;
            }
            let evidence = match bridge.slot_terminal_evidence(store_wave_id, *slot_index) {
                Ok(Some(ev)) => ev,
                Ok(None) => continue,
                Err(err) => {
                    tracing::warn!(
                        wave_id = %store_wave_id,
                        slot_index = slot_index,
                        error = %err,
                        "store_completed: evidence lookup failed; failing closed",
                    );
                    continue;
                }
            };
            // P1-6: topic must be the wave-kind terminal
            // topic. We pin Review for now; Exec/Fix
            // reconciliation is a separate fan-in path and does
            // not enter this helper.
            if evidence.topic != "review.unit.done" {
                tracing::warn!(
                    wave_id = %store_wave_id,
                    slot_index = slot_index,
                    evidence_topic = %evidence.topic,
                    "store_completed: evidence topic is not the review terminal topic; failing closed",
                );
                continue;
            }
            // P1-6: dimension must be present AND equal the
            // slot's assigned dimension. We refuse the
            // pre-fix `evidence.dimension.or(assigned)` fallback
            // because it would let an evidence row with a
            // missing dimension silently mark the assigned
            // dimension done — exactly the wrong-topic /
            // missing-dimension inflation the review demanded
            // close.
            let evidence_dim = match &evidence.dimension {
                Some(d) => d,
                None => {
                    tracing::warn!(
                        wave_id = %store_wave_id,
                        slot_index = slot_index,
                        "store_completed: evidence missing dimension; failing closed",
                    );
                    continue;
                }
            };
            let assigned = completed.assigned_dimensions.get(slot_index);
            match assigned {
                Some(a) if a == evidence_dim => {
                    store_completed.insert(a.clone());
                    authoritative_slot_indices.insert(*slot_index);
                }
                Some(a) => {
                    tracing::warn!(
                        wave_id = %store_wave_id,
                        slot_index = slot_index,
                        assigned = %a,
                        evidence_dimension = %evidence_dim,
                        "store_completed: dimension mismatch; failing closed",
                    );
                    continue;
                }
                None => {
                    // No assigned dimension at all — refuse to
                    // invent one.
                    tracing::warn!(
                        wave_id = %store_wave_id,
                        slot_index = slot_index,
                        "store_completed: slot has no assigned dimension; failing closed",
                    );
                    continue;
                }
            }
        }
    }

    // Post-filter `main_backscan` against the store's authoritative
    // slot set (2026-07-27-003 plan U4 / R7). A main row only
    // counts when the slot it references is in
    // `authoritative_slot_indices`. Rows with no slot index and
    // rows whose slot is not authoritative drop out of
    // `main_backscan` entirely; they are visible to the
    // diagnostics writer through `ReviewReconciliation` instead.
    let mut main_backscan = std::collections::HashSet::new();
    for (slot_index, dim) in &main_backscan_dim_by_slot {
        if authoritative_slot_indices.contains(slot_index) {
            main_backscan.insert(dim.clone());
        }
    }
    for dim in &main_backscan_no_slot {
        // No-slot main rows cannot be tied to a specific
        // authoritative completion. U4 fail-closes them: they
        // are NOT counted as completion. The pre-fix code kept
        // them in `main_backscan`, which is exactly the orphan
        // trap the incident exposed.
        tracing::warn!(
            wave_id = %store_wave_id,
            dimension = %dim,
            "main_backscan: same-wave row with no slot_index; \
             dropped from completion set (treated as orphan)"
        );
    }
    let _ = main_backscan_raw; // raw set is reserved for the
    // structured diagnostics writer
    // (U6 follow-up); the U3 test only
    // pins the post-filtered result.

    ReviewDoneHints {
        main_backscan,
        store_completed,
    }
}

pub(crate) fn build_review_reconciliation(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    store_wave_id: &str,
    completed: &ralph_core::CompletedWave,
    main_events_file: &Path,
) -> Option<ralph_core::supervisor::reconciliation::ReviewReconciliation> {
    let snap = match bridge.fan_in_status(store_wave_id) {
        Ok(s) => s,
        Err(err) => {
            warn!(
                wave_id = %store_wave_id,
                error = %err,
                "U4: fan_in_status failed; reconciliation skipped, \
                 caller falls back to legacy union-of-hints path"
            );
            return None;
        }
    };
    let main_contents = match std::fs::read_to_string(main_events_file) {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            warn!(
                wave_id = %store_wave_id,
                error = %err,
                "U4: main ledger read failed; reconciliation continues \
                 with empty observations"
            );
            String::new()
        }
    };
    let observations = ralph_core::supervisor::reconciliation::scan_review_projection_observations(
        &main_contents,
        store_wave_id,
    );
    let evidence_by_slot =
        ralph_core::supervisor::reconciliation::collect_evidence(bridge.as_ref(), &snap);
    Some(
        ralph_core::supervisor::reconciliation::reconcile_review_wave(
            &snap,
            &completed.assigned_dimensions,
            &observations,
            "review.unit.done",
            &evidence_by_slot,
            None,
        ),
    )
}

/// 2026-07-26-004 plan U5 (S5 / AE3 / KTD4): the producer identity
/// stamped on runtime coordination events (`*.wave.complete` /
/// `*.wave.failed`). The orchestrator — not the consumer hat — produces
/// these; `ralph` is the builtin runtime pseudo-hat the origin guard
/// already recognises as a control producer. The consumer hat is carried
/// separately in the event's `hat` field (routing / topic subscription).
pub(crate) const COORD_SYSTEM_PRODUCER: &str = "ralph";

pub(crate) fn append_supervisor_coord_event(
    main_events_file: &Path,
    topic: &str,
    payload: &serde_json::Value,
) -> Result<ralph_core::supervisor::CoordinationReceipt, ralph_core::supervisor::ProjectionError> {
    use std::io::Write;
    // Derive the hat attribution from the coordination topic.
    // `exec.wave.complete` → `exec-integrator`, `fix.wave.complete` →
    // `fix-integrator`, `review.wave.complete` → `review-synthesizer`.
    // Failed waves route to the matching failure-handler hat
    // (`exec-failure-handler`); for review, the `implementation-review`
    // preset's `event_filter` subscribes `finalizer` to
    // `review.wave.failed` (so the failure triggers
    // `wave-blocked.md` + `LOOP_COMPLETE` via finalizer, never
    // `review-synthesizer`). For fix waves, the failure also routes
    // to `exec-failure-handler` (the preset has no dedicated
    // `fix-failure-handler` hat).
    let hat_attribution = if topic.starts_with("exec.wave.") {
        if topic.ends_with(".failed") {
            "exec-failure-handler"
        } else {
            "exec-integrator"
        }
    } else if topic.starts_with("fix.wave.") {
        if topic.ends_with(".failed") {
            "exec-failure-handler"
        } else {
            "fix-integrator"
        }
    } else if topic.starts_with("review.wave.") {
        // 2026-07-26-003 plan (KTD4): split the review band by
        // success vs failure. Success keeps routing to
        // `review-synthesizer` (the integrator that reads
        // `completed_dimensions`); failure now routes to
        // `finalizer` (the only hat subscribed to
        // `review.wave.failed` in the `implementation-review`
        // preset). Routing the failure to `review-synthesizer`
        // caused the primary-20260726 incident: the synthesizer
        // was woken for the failure path, attempted to CLI-emit
        // a coordination topic it did not own, and got rejected;
        // meanwhile `finalizer` never received the trigger.
        if topic.ends_with(".failed") {
            "finalizer"
        } else {
            "review-synthesizer"
        }
    } else {
        "ralph"
    };
    // 2026-07-26-004 plan U5 (S5 / AE3 / KTD4): separate the PRODUCER
    // from the CONSUMER. A runtime coordination event is produced by
    // the orchestrator (system producer `ralph`), NOT by the consumer
    // hat. The consumer (finalizer / integrator / synthesizer) is
    // expressed by `hat` (which the 2026-07-26-003 routing fix and the
    // preset's topic subscription rely on) — keeping `hat` unchanged
    // preserves that routing while `source` now truthfully names the
    // runtime as producer. The two answers no longer reuse one field.
    let record = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": chrono::Utc::now().to_rfc3339(),
        "hat": hat_attribution,
        "source": COORD_SYSTEM_PRODUCER,
        "system_injected": true,
    });
    let serialised = serde_json::to_string(&record)
        .map_err(|err| ralph_core::supervisor::ProjectionError::Io(err.to_string()))?;
    let write_result = (|| -> std::io::Result<()> {
        if let Some(parent) = main_events_file.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(main_events_file)?;
        writeln!(file, "{}", serialised)?;
        file.flush()?;
        Ok(())
    })();
    if let Err(err) = write_result {
        warn!(
            topic = %topic,
            path = %main_events_file.display(),
            error = %err,
            "U5: failed to append supervisor coordination event to ledger"
        );
        return Err(ralph_core::supervisor::ProjectionError::Io(err.to_string()));
    }
    let payload_fingerprint = fingerprint_coord_payload(topic, payload);
    let idempotency_key = format!("coord:{}:{}", topic, payload_fingerprint);
    Ok(ralph_core::supervisor::CoordinationReceipt {
        wave_id: String::new(),
        topic: topic.to_string(),
        idempotency_key,
        payload_fingerprint,
        write_count: 1,
        already_present_count: 0,
        committed_at_unix_secs: unix_now_secs(),
    })
}

pub(crate) fn fingerprint_coord_payload(topic: &str, payload: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(topic.as_bytes());
    hasher.update(b"\0");
    hasher.update(payload.to_string().as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub(crate) fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
