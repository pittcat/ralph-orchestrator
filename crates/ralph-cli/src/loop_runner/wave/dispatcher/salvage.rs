//! Salvage module — completed-slot merge, diagnostics JSON writer, workspace-root derivation helper.
//! Originally part of `wave/dispatcher.rs` (plan `2026-08-07-008`).
//! Public surface and behaviour preserved verbatim.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::warn;

use super::coordination::unix_now_secs;
use ralph_core::CompletedWave;

pub(crate) fn merge_completed_review_slots_to_main(
    main_events_file: &Path,
    completed: &ralph_core::CompletedWave,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    store_wave_id: &str,
) -> Result<ralph_core::supervisor::ProjectionReceipt, ralph_core::supervisor::ProjectionError> {
    use ralph_core::supervisor::ProjectionKey;
    let mut lines: Vec<String> = Vec::new();
    let mut keys: Vec<ProjectionKey> = Vec::new();
    for result in &completed.results {
        // Slots that show up in `completed.failures` are skipped:
        // their `results` entry is a stale artifact of the failed
        // tick and must not be merged (silent-success anti-pattern).
        if completed.failures.iter().any(|f| f.index == result.index) {
            continue;
        }
        for event in &result.events {
            if event.topic.as_str() != "review.unit.done" {
                continue;
            }
            // Pre-render the JSONL row with the `review-worker`
            // attribution so `compute_missing_dimensions` (U4)
            // sees the dimension in main as already done.
            //
            // 2026-07-26-004 plan U3 (R1 / bounded backscan): preserve
            // the event's envelope `wave_id` / `wave_index` so the
            // fan-in main-ledger backscan can filter to THIS wave and
            // never eat another wave's `review.unit.done`. Dropping the
            // wave id here was what made cross-source reconciliation
            // unsafe before U3.
            // 2026-07-31-002 plan U0: salvage fingerprint stability — drop
            // the `ts` field so re-tick salvages produce byte-identical lines.
            // `fingerprint_lines` (line 4542) hashes the serialized row bytes;
            // a per-call `Utc::now()` made the fingerprint drift each tick,
            // tripping the strict rusqlite `commit_salvage_projection` gate
            // (rusqlite.rs:1240-1248). `Event` does not carry a `ts` field
            // (see crates/ralph-proto/src/event.rs:8), so we cannot backfill
            // a stable timestamp; omitting `ts` is the fingerprint-stable
            // choice and does not break downstream consumers
            // (compute_missing_dimensions reads topic/payload/wave_id/
            // wave_index, never `ts`).
            let record = serde_json::json!({
                "topic": event.topic.as_str(),
                "payload": event.payload.as_str(),
                "hat": "review-worker",
                "source": "review-worker",
                "wave_id": event.wave_id,
                "wave_index": event.wave_index,
            });
            let line = serde_json::to_string(&record)
                .map_err(|err| ralph_core::supervisor::ProjectionError::Io(err.to_string()))?;
            keys.push(ProjectionKey {
                slot_index: result.index,
                payload_fingerprint: ralph_core::supervisor::fingerprint_payload(&event.payload),
            });
            lines.push(line);
        }
    }
    commit_salvage_batch(
        main_events_file,
        &lines,
        keys,
        bridge,
        store_wave_id,
        "merge_completed_review_slots_to_main",
    )
}

/// 2026-07-25-005 plan U1 (R3 / R4 / KTD2 / KTD6): the exec/fix
/// counterpart of [`merge_completed_review_slots_to_main`]. When an
/// exec/fix wave must fail, the Completed slots' business events are
/// appended to the main ledger FIRST (salvage) and only then does the
/// dispatcher inject `*.wave.failed` — KTD2 forbids a silent partial
/// complete, but the completed work must not be dropped on the floor
/// either.
///
/// Slots that also show up in `completed.failures` are skipped: their
/// `results` entry is a stale artifact of the failed tick and merging
/// it would be a silent-success anti-pattern. Each salvaged row keeps
/// the worker's own `source` attribution and the wave envelope
/// (`wave_id` / `wave_index`) so the post-wave re-read publishes the
/// event exactly as the worker produced it.
///
/// Like the review helper, the salvage phases are committed by
/// [`commit_salvage_batch`] even when ZERO rows were appended: an
/// all-failed wave has nothing to salvage, but the coordinator's
/// `fail_wave` gate still requires `SalvageCommitted` before it
pub(crate) fn merge_completed_exec_fix_slots_to_main(
    main_events_file: &Path,
    completed: &ralph_core::CompletedWave,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    store_wave_id: &str,
) -> Result<ralph_core::supervisor::ProjectionReceipt, ralph_core::supervisor::ProjectionError> {
    use ralph_core::supervisor::ProjectionKey;
    let mut lines: Vec<String> = Vec::new();
    let mut keys: Vec<ProjectionKey> = Vec::new();
    for result in &completed.results {
        if completed.failures.iter().any(|f| f.index == result.index) {
            continue;
        }
        for event in &result.events {
            let attribution = event
                .source
                .as_ref()
                .map(|h| h.as_str())
                .unwrap_or("worker");
            // 2026-07-31-002 plan U0: same rationale as the review arm —
            // keep the exec/fix salvage fingerprint stable across retry
            // ticks so the strict rusqlite `commit_salvage_projection`
            // gate accepts the re-tick salvage (rusqlite.rs:1240-1248).
            let record = serde_json::json!({
                "topic": event.topic.as_str(),
                "payload": event.payload.as_str(),
                "hat": attribution,
                "source": attribution,
                "wave_id": event.wave_id,
                "wave_index": event.wave_index,
            });
            let line = serde_json::to_string(&record)
                .map_err(|err| ralph_core::supervisor::ProjectionError::Io(err.to_string()))?;
            keys.push(ProjectionKey {
                slot_index: result.index,
                payload_fingerprint: ralph_core::supervisor::fingerprint_payload(&event.payload),
            });
            lines.push(line);
        }
    }
    commit_salvage_batch(
        main_events_file,
        &lines,
        keys,
        bridge,
        store_wave_id,
        "merge_completed_exec_fix_slots_to_main",
    )
}

/// Shared tail for both salvage merge seams: append `lines` to the
/// main ledger, then stamp the two delivery phases the coordinator's
/// `fail_wave` gate requires (`BusinessProjected` →
/// `SalvageCommitted`).
///
/// An EMPTY batch is a legitimate salvage outcome — an all-failed
/// wave has no Completed slot to keep — and must still commit both
/// phases. Returning an uncommitted empty receipt strands the wave
/// below `SalvageCommitted`, so `fail_wave` answers
/// `SalvageNotMerged` on every tick and the fan-in degrades to
/// `StoreError` / `fan_in_failed` instead of injecting
/// `*.wave.failed`. Both seams funnel through here so the empty and
/// non-empty paths cannot drift apart again.
///
/// A write failure leaves both phases unstamped so the next tick
/// re-runs the seam (idempotent on slot status).
pub(crate) fn commit_salvage_batch(
    main_events_file: &Path,
    lines: &[String],
    keys: Vec<ralph_core::supervisor::ProjectionKey>,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    store_wave_id: &str,
    seam: &'static str,
) -> Result<ralph_core::supervisor::ProjectionReceipt, ralph_core::supervisor::ProjectionError> {
    use ralph_core::supervisor::{ProjectionKind, ProjectionReceipt, ProjectionReceiptSummary};
    use std::io::Write;
    if !lines.is_empty() {
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
            for line in lines {
                writeln!(file, "{}", line)?;
            }
            file.flush()?;
            Ok(())
        })();
        if let Err(err) = write_result {
            warn!(
                wave_id = %store_wave_id,
                path = %main_events_file.display(),
                seam,
                error = %err,
                "U5: failed to merge Completed slots to main ledger; the salvage \
                 phases stay uncommitted so the coordinator will refuse the \
                 coord-event injection until the next tick retries"
            );
            return Err(ralph_core::supervisor::ProjectionError::Io(err.to_string()));
        }
    }
    // An empty batch keeps the stable `empty-<wave>` fingerprint that
    // `build_empty_projection_receipt` and the in-memory store share.
    let batch_fingerprint = if lines.is_empty() {
        format!("empty-{store_wave_id}")
    } else {
        fingerprint_lines(lines)
    };
    let write_count = lines.len() as u32;
    let committed_at_unix_secs = unix_now_secs();
    let summary = |batch_fingerprint: String| ProjectionReceiptSummary {
        kind: ProjectionKind::Business,
        batch_fingerprint,
        write_count,
        already_present_count: 0,
        committed_at_unix_secs,
    };
    // 2026-07-27-004 plan U5 (R17 / P0): phase one (`Pending` →
    // `BusinessProjected`) is stamped only after the rows physically
    // landed — the strict rusqlite `commit_salvage_projection` gate
    // refuses a `Pending` wave.
    if let Err(err) =
        bridge.record_business_projection(store_wave_id, &summary(batch_fingerprint.clone()))
    {
        warn!(
            wave_id = %store_wave_id,
            seam,
            error = %err,
            "record_business_projection failed; next tick will retry"
        );
        return Err(ralph_core::supervisor::ProjectionError::from(err));
    }
    // Phase two: commit the salvage mark AFTER the rows landed.
    if let Err(err) =
        bridge.commit_salvage_projection(store_wave_id, &summary(batch_fingerprint.clone()))
    {
        warn!(
            wave_id = %store_wave_id,
            seam,
            error = %err,
            "commit_salvage_projection failed; next tick will retry"
        );
        return Err(ralph_core::supervisor::ProjectionError::from(err));
    }
    Ok(ProjectionReceipt {
        wave_id: store_wave_id.to_string(),
        kind: ProjectionKind::Business,
        idempotency_keys: keys,
        write_count,
        already_present_count: 0,
        batch_fingerprint,
        committed_at_unix_secs,
    })
}

pub(crate) fn project_empty_salvage(
    snapshot: &ralph_core::supervisor::WaveSnapshot,
    store_wave_id: &str,
) -> Result<ralph_core::supervisor::ProjectionReceipt, ralph_core::supervisor::ProjectionError> {
    use ralph_core::supervisor::{ProjectionKind, SlotStatus};
    for (slot_index, status) in &snapshot.slots {
        if matches!(status, SlotStatus::Completed) {
            return Err(ralph_core::supervisor::ProjectionError::InvalidTransition(
                format!(
                    "project_empty_salvage: wave {store_wave_id} has Completed slot {slot_index}; \
                     refusing to mark empty salvage"
                ),
            ));
        }
    }
    Ok(build_empty_projection_receipt(
        store_wave_id,
        ProjectionKind::Business,
    ))
}

pub(crate) fn build_empty_projection_receipt(
    wave_id: &str,
    kind: ralph_core::supervisor::ProjectionKind,
) -> ralph_core::supervisor::ProjectionReceipt {
    ralph_core::supervisor::ProjectionReceipt {
        wave_id: wave_id.to_string(),
        kind,
        idempotency_keys: Vec::new(),
        write_count: 0,
        already_present_count: 0,
        batch_fingerprint: format!("empty-{wave_id}"),
        committed_at_unix_secs: unix_now_secs(),
    }
}

pub(crate) fn fingerprint_lines(lines: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for line in lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub(crate) fn build_wave_failed_slots_json(
    wave_id: &str,
    slots: &[(u32, ralph_core::supervisor::SlotStatus)],
    reasons: &std::collections::HashMap<u32, String>,
    elapsed_secs: u64,
) -> serde_json::Value {
    let slot_entries: Vec<serde_json::Value> = slots
        .iter()
        .map(|(idx, status)| {
            let reason = reasons.get(idx);
            serde_json::json!({
                "slot_index": *idx,
                "status": status_to_str(status),
                "reason": reason,
            })
        })
        .collect();
    serde_json::json!({
        "wave_id": wave_id,
        "generated_at_kind": "injected_failed",
        "elapsed_secs": elapsed_secs,
        "slots": slot_entries,
    })
}

pub(crate) fn status_to_str(status: &ralph_core::supervisor::SlotStatus) -> &'static str {
    match status {
        ralph_core::supervisor::SlotStatus::Pending => "pending",
        ralph_core::supervisor::SlotStatus::Dispatched => "dispatched",
        ralph_core::supervisor::SlotStatus::Running => "running",
        ralph_core::supervisor::SlotStatus::Completed => "completed",
        ralph_core::supervisor::SlotStatus::Failed => "failed",
        ralph_core::supervisor::SlotStatus::Cancelled => "cancelled",
    }
}

pub(crate) fn write_wave_diagnostics_json(
    root: &Path,
    wave_id: &str,
    payload: &serde_json::Value,
) -> std::io::Result<PathBuf> {
    let dir = root.join(".ralph").join("diagnostics");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("wave-{wave_id}-slots.json"));
    let bytes = serde_json::to_vec_pretty(payload).expect("payload is always a valid JSON Value");
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// 2026-07-26-002 plan U6 (R6 / KTD2 / KTD7): append the
/// dispatcher's signed absolute channel path to
/// `<workspace>/.ralph/current-wave-channels`. The marker is one
/// line per path, exact-matched at the consumer (no prefix
/// wildcards). Concurrent waves can append freely because each
/// line is independently accepted or rejected by the canonicalize
/// equality check in `paths_equivalent`.
///
/// Failure modes:
/// - `.ralph/` not writable: caller logs warn and the worker
///   falls back to the legacy shape-only allowlist.
/// - `events_file` does not have a `.ralph/` ancestor: caller
///   surfaces an error and the worker is spawned without a marker.
#[allow(dead_code)]
pub(crate) fn append_wave_channel_to_marker(
    main_events_file: &Path,
    worker_events_file: &Path,
) -> std::io::Result<()> {
    let workspace_root = workspace_root_from_events(main_events_file);
    let ralph_dir = workspace_root.join(".ralph");
    let marker = ralph_dir.join("current-wave-channels");
    std::fs::create_dir_all(&ralph_dir)?;
    let absolute = if worker_events_file.is_absolute() {
        worker_events_file.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(worker_events_file))
            .unwrap_or_else(|_| worker_events_file.to_path_buf())
    };
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&marker)?;
    writeln!(file, "{}", absolute.display())?;
    Ok(())
}

pub(crate) fn workspace_root_from_events(events_file: &Path) -> PathBuf {
    let mut current = events_file;
    for _ in 0..2 {
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => break,
        }
    }
    if current.is_absolute() {
        return current.to_path_buf();
    }
    std::env::current_dir()
        .map(|c| c.join(current))
        .unwrap_or_else(|_| current.to_path_buf())
}
