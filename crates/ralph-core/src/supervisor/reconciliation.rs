//! 2026-07-27-003 plan U4: Supervisor 终态证据唯一权威的 reconciliation.
//!
//! `build_wave_failed_payload` 之前在 `Review` arm 同时采信 main ledger
//! backscan 和 Supervisor store `Completed` 状态——主账本里被孤立的
//! `review.unit.done` 行（slot 在 store 侧 Failed）会被错误地从
//! `missing_dimensions` 中扣掉，从而把 6 个失败 slot 报告成 1 个 missing
//! （implementation-review primary-20260727 事故）。
//!
//! 本模块把 reconciliation 拆成两个纯数据步骤：
//!
//! 1. **`scan_review_projection_observations`** — 只读 main ledger，把
//!    每条同 wave 的 `review.unit.done` 行转成 `ProjectionObservation`，
//!    不做任何权威判断。供调用方独立使用（事件级诊断）。
//! 2. **`reconcile_review_wave`** — 接收 store snapshot + 投影观测，
//!    决定：
//!      - `authoritative_completed`（仅来自 store `Completed` + 校验
//!        通过的 terminal evidence；main projection 一律不算）；
//!      - `missing_dimensions`（assigned 减去 authoritative_completed）；
//!      - `blocking_slots`（已 terminal 但 evidence 校验失败的 slot）；
//!      - `orphan_projections`（main 有 done，store 对应 slot 失败/未
//!        完成）；
//!      - `missing_projections`（store 权威完成，但 main 没有对应行，
//!        留给 U5 投影阶段使用）；
//!      - `payload_conflicts`（main 行有 `dimension` 字段但跟 slot
//!        assignment 不一致）；
//!      - `evidence_validations`（每个被采纳 evidence 的逐项校验结果）。
//!
//! `validate_terminal_evidence` 是 `reconcile_review_wave` 的内部原子：
//! 证明一个 slot 既是 `Completed`，又有匹配的 topic / dimension /
//! fingerprint。`Result<(), ValidationError>` 模型把任何不匹配收敛到
//! fail-close，绝不允许半 valid 的 evidence 计入 `authoritative_completed`。
//!
//! 该模块是**纯函数 + 纯数据**：不读文件、不写文件、不持有任何
//! Supervisor store / bridge。IO 由 dispatcher 完成，reconciliation
//! 本身必须可由相同输入重现，以满足 plan 验收中的「DB 重启后
//! reconciliation 一致」要求。
//!
//! # 关键不变量
//!
//! - `authoritative_completed ∩ orphan_projections = ∅`（一条 main done
//!   行不可能既被 store 认账又被 store 拒掉）。
//! - `authoritative_completed ⊆ {slots ∈ snapshot.slots : status ==
//!   Completed ∧ evidence validation passes}`。
//! - `missing_dimensions = expected_dimensions − authoritative_completed`。
//! - `compute_review_missing_dimensions`（dispatcher 现有纯函数）只接收
//!   authoritative_completed，**不**接收 main backscan。

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::supervisor::WaveDeliveryState;
use crate::supervisor::{SlotStatus, TerminalEvidence, WaveSnapshot};

/// Stable, machine-readable conflict category for the `reason` field of
/// `*.wave.failed`. Surfaced as the public reason in dispatcher
/// payloads; per-slot detail goes to the structured diagnostics writer.
pub const REASON_WAVE_EVIDENCE_CONFLICT: &str = "wave_evidence_conflict";

/// One raw main-ledger row that the dispatcher collected for the wave.
/// `scan_review_projection_observations` produces one of these per
/// same-wave `review.unit.done` line; `reconcile_review_wave` then
/// classifies it as authoritative / orphan / conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionObservation {
    /// Slot index carried on the event envelope, when present. `None`
    /// for legacy / pre-fix rows that did not stamp the index — these
    /// are tracked as orphan observations (no slot to reconcile
    /// against) rather than silently consumed.
    pub slot_index: Option<u32>,
    /// Topic of the row (always `review.unit.done` after the
    /// scan-time filter, but kept on the type so callers can audit
    /// malformed rows separately).
    pub topic: String,
    /// Wave id stamped on the envelope, if any.
    pub wave_id: Option<String>,
    /// Decoded `dimension` payload field, if any.
    pub dimension: Option<String>,
    /// SHA-256 fingerprint of the original payload string (so conflict
    /// detection does not have to re-parse the same JSON twice).
    pub payload_fingerprint: String,
    /// Line number inside the main file (0-based) — diagnostics only.
    pub line_no: usize,
}

/// Stable SHA-256 fingerprint helper. Mirrors the algorithm used by
/// `TerminalEvidence::from_event` so the reconciliation can compare
/// `observation.payload_fingerprint` against the saved `evidence.
/// payload_fingerprint` byte-for-byte.
pub fn fingerprint_payload(payload: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Result of validating one slot's `TerminalEvidence` against the
/// expected assignment and topic. Failures are typed so the
/// structured diagnostics writer can categorise them without
/// inspecting error strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result", content = "reason")]
pub enum EvidenceValidation {
    /// Slot is `Completed`, topic matches, dimension matches the
    /// assignment, payload decodes consistently, and fingerprint
    /// matches the saved evidence fingerprint.
    Valid,
    /// Slot is not in a `Completed` state. Slot index is provided
    /// for diagnostics; the slot's current status is in
    /// `current_status` for visibility.
    SlotNotCompleted {
        slot_index: u32,
        current_status: SlotStatus,
    },
    /// No terminal evidence was recorded for the slot. A bare
    /// `Completed` status bit with no evidence is fail-closed
    /// (2026-07-26-004 KTD3).
    MissingEvidence { slot_index: u32 },
    /// Evidence topic does not match the expected terminal topic
    /// for this wave kind. Review waves require `review.unit.done`.
    TopicMismatch {
        slot_index: u32,
        expected: String,
        actual: String,
    },
    /// Evidence dimension is missing (e.g. exec/fix style terminal
    /// row), but the wave kind requires a dimension.
    MissingDimension { slot_index: u32 },
    /// Evidence dimension disagrees with the slot's assignment.
    DimensionMismatch {
        slot_index: u32,
        expected: String,
        actual: String,
    },
    /// Evidence fingerprint disagrees between the saved evidence
    /// and the saved accepted payload (the supervisor store keeps
    /// these in lockstep; divergence is fail-closed).
    FingerprintMismatch { slot_index: u32 },
    /// Slot is `Completed` with valid evidence but the slot has no
    /// assignment at all. The dispatcher refuses to invent one
    /// (no `evidence.dimension.or(assigned)` fallback).
    UnassignedSlot { slot_index: u32 },
}

/// Per-slot record of how its evidence was judged. Stored on
/// `ReviewReconciliation.evidence_validations` keyed by slot index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceValidationRecord {
    pub slot_index: u32,
    pub validation: EvidenceValidation,
}

/// One orphan projection: a `review.unit.done` row in main whose slot
/// is not `Completed` in the store (Failed / Pending / Running /
/// Cancelled / Dispatched / missing). Reconciled as a diagnostic
/// signal, NOT as authoritative completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanProjection {
    pub slot_index: Option<u32>,
    pub dimension: Option<String>,
    pub payload_fingerprint: String,
    pub line_no: usize,
    /// Slot status the store reported for the same slot, when one
    /// was found. `None` if the row had no slot index and no slot
    /// in the store matched.
    pub store_status: Option<SlotStatus>,
}

/// One main projection that disagrees with the slot assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadConflict {
    pub slot_index: Option<u32>,
    pub expected_dimension: Option<String>,
    pub actual_dimension: String,
    pub payload_fingerprint: String,
    pub line_no: usize,
}

/// Final reconciliation output of `reconcile_review_wave`. All
/// collections are sorted (BTreeSet) so equality comparisons are
/// deterministic and diagnostics payloads are reproducible across
/// runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewReconciliation {
    /// Stable, sorted (slot index ascending) list of slots whose
    /// evidence passed all checks. The ONLY source of completion
    /// authority used by `build_wave_failed_payload` to compute
    /// `missing_dimensions`.
    pub authoritative_completed: Vec<u32>,
    /// Sorted, deduplicated list of dimensions that never reached
    /// `authoritative_completed`. Mirrors `compute_review_missing_dimensions`
    /// output but derived from the store-backed authoritative
    /// completed set, not from a union with main backscan.
    pub missing_dimensions: Vec<String>,
    /// Slots that reached a terminal state but whose evidence
    /// failed validation. Surfaced as `blocking_slots` in the
    /// diagnostics writer; the public `*.wave.failed` payload
    /// keeps `missing_dimensions` only.
    pub blocking_slots: Vec<u32>,
    /// Same-wave main rows whose slot is not `Completed` in the
    /// store. Fail-closed: they are NOT counted as completion.
    pub orphan_projections: Vec<OrphanProjection>,
    /// Slots that are `Completed` with valid evidence but have no
    /// matching main projection. Surfaced for U5's projection
    /// write to fill in; the reconciliation itself does not
    /// touch the main ledger.
    pub missing_projections: Vec<u32>,
    /// Main rows whose `dimension` disagrees with the slot's
    /// assignment (or whose slot cannot be identified). Fail-closed.
    pub payload_conflicts: Vec<PayloadConflict>,
    /// Per-slot evidence validation. Always populated for slots
    /// that appear in `snapshot.slots`; entries appear for orphan
    /// and conflicting slots as well, with the matching failure
    /// variant.
    pub evidence_validations: Vec<EvidenceValidationRecord>,
}

impl ReviewReconciliation {
    /// Convenience accessor: sorted (slot index ascending) list of
    /// slot indices the dispatcher should treat as `Completed`.
    pub fn completed_slot_indices(&self) -> &[u32] {
        &self.authoritative_completed
    }

    /// Convenience accessor: missing dimensions in the same sorted
    /// order used by the public `*.wave.failed` payload.
    pub fn missing_dimension_list(&self) -> &[String] {
        &self.missing_dimensions
    }
}

/// Validate one slot's evidence against the expected topic and
/// assignment. Returns `EvidenceValidation::Valid` only when ALL
/// checks pass; any failure returns the typed variant.
///
/// Inputs:
/// - `slot_index`: slot id (always known when called from
///   `reconcile_review_wave`; provided for diagnostics).
/// - `slot_status`: status the store reported for the slot.
/// - `evidence`: terminal evidence saved alongside the slot, if any.
/// - `expected_topic`: terminal topic this wave kind requires (e.g.
///   `review.unit.done`).
/// - `expected_assignment`: dimension assigned to the slot. `None`
///   when the wave is not dimension-scoped.
/// - `accepted_payload_fingerprint`: fingerprint of the saved
///   accepted payload (kept in lockstep with the evidence row;
///   `None` for stores that do not track it).
pub fn validate_terminal_evidence(
    slot_index: u32,
    slot_status: SlotStatus,
    evidence: Option<&TerminalEvidence>,
    expected_topic: &str,
    expected_assignment: Option<&str>,
    accepted_payload_fingerprint: Option<&str>,
) -> EvidenceValidation {
    if !matches!(slot_status, SlotStatus::Completed) {
        return EvidenceValidation::SlotNotCompleted {
            slot_index,
            current_status: slot_status,
        };
    }
    let evidence = match evidence {
        Some(ev) => ev,
        None => return EvidenceValidation::MissingEvidence { slot_index },
    };
    if evidence.topic != expected_topic {
        return EvidenceValidation::TopicMismatch {
            slot_index,
            expected: expected_topic.to_string(),
            actual: evidence.topic.clone(),
        };
    }
    if evidence.dimension.is_none() {
        return EvidenceValidation::MissingDimension { slot_index };
    }
    if expected_assignment.is_none() {
        return EvidenceValidation::UnassignedSlot { slot_index };
    }
    let actual_dim = evidence.dimension.as_deref().unwrap_or("");
    let expected_dim = expected_assignment.unwrap_or("");
    if actual_dim != expected_dim {
        return EvidenceValidation::DimensionMismatch {
            slot_index,
            expected: expected_dim.to_string(),
            actual: actual_dim.to_string(),
        };
    }
    if let Some(accepted_fp) = accepted_payload_fingerprint
        && accepted_fp != evidence.payload_fingerprint
    {
        return EvidenceValidation::FingerprintMismatch { slot_index };
    }
    EvidenceValidation::Valid
}

/// Pure reconciliation: store snapshot + per-slot evidence + main
/// projection observations + expected dimension assignment →
/// `ReviewReconciliation`.
///
/// This function:
/// - never reads or writes IO;
/// - never inspects the bridge or store directly;
/// - returns identical output for identical inputs (DB-restart
///   invariant);
/// - never produces `authoritative_completed` for orphan main rows
///   (the implementation-review primary-20260727 accident);
/// - never produces `*.wave.complete` signals — caller is responsible
///   for the success/failure decision based on `missing_dimensions`.
///
/// `expected_dimensions` is the `CompletedWave.assigned_dimensions`
/// map (slot index → assigned dimension). Slots that are not in this
/// map have no assignment, and any evidence they carry fails closed
/// (`UnassignedSlot`).
///
/// `evidence_by_slot` is the per-slot terminal evidence the store
/// persists; the dispatcher reads it via
/// `SupervisorBridge::slot_terminal_evidence`. `accepted_payload_fingerprint`
/// is reserved for stores that track a separate accepted-payload
/// hash; the default is `None` (no extra check).
pub fn reconcile_review_wave(
    snapshot: &WaveSnapshot,
    expected_dimensions: &HashMap<u32, String>,
    projection_observations: &[ProjectionObservation],
    expected_terminal_topic: &str,
    evidence_by_slot: &HashMap<u32, TerminalEvidence>,
    accepted_payload_fingerprint: Option<&dyn Fn(u32) -> Option<String>>,
) -> ReviewReconciliation {
    let mut authoritative: BTreeSet<u32> = BTreeSet::new();
    let mut blocking: BTreeSet<u32> = BTreeSet::new();
    let mut authoritative_dimensions: BTreeSet<String> = BTreeSet::new();
    let mut evidence_validations: Vec<EvidenceValidationRecord> = Vec::new();

    for (slot_index, slot_status) in &snapshot.slots {
        let assignment = expected_dimensions.get(slot_index).map(String::as_str);
        let evidence = evidence_by_slot.get(slot_index);
        let accepted_fp_string;
        let accepted_fp = match accepted_payload_fingerprint {
            Some(cb) => match cb(*slot_index) {
                Some(s) => {
                    accepted_fp_string = s;
                    Some(accepted_fp_string.as_str())
                }
                None => None,
            },
            None => None,
        };
        let validation = validate_terminal_evidence(
            *slot_index,
            *slot_status,
            evidence,
            expected_terminal_topic,
            assignment,
            accepted_fp,
        );
        if matches!(validation, EvidenceValidation::Valid) {
            authoritative.insert(*slot_index);
            if let Some(assigned) = assignment {
                authoritative_dimensions.insert(assigned.to_string());
            }
        } else if matches!(slot_status, SlotStatus::Completed) {
            // Terminal but invalid → blocking (rejected for
            // completion; downstream reclassifies as
            // required-slot-failure).
            blocking.insert(*slot_index);
        }
        evidence_validations.push(EvidenceValidationRecord {
            slot_index: *slot_index,
            validation,
        });
    }

    let (orphan_projections, missing_projections, payload_conflicts) = classify_projections(
        snapshot,
        expected_dimensions,
        projection_observations,
        &authoritative,
    );

    let mut missing_dimensions: Vec<String> = expected_dimensions
        .values()
        .filter(|dim| !authoritative_dimensions.contains(*dim))
        .cloned()
        .collect();
    missing_dimensions.sort();

    ReviewReconciliation {
        authoritative_completed: authoritative.iter().copied().collect(),
        missing_dimensions,
        blocking_slots: blocking.iter().copied().collect(),
        orphan_projections,
        missing_projections,
        payload_conflicts,
        evidence_validations,
    }
}

/// Walk observations, classifying each as orphan / conflict / missing
/// based on the already-computed authoritative set.
fn classify_projections(
    snapshot: &WaveSnapshot,
    expected_dimensions: &HashMap<u32, String>,
    observations: &[ProjectionObservation],
    authoritative: &BTreeSet<u32>,
) -> (Vec<OrphanProjection>, Vec<u32>, Vec<PayloadConflict>) {
    let mut orphans: Vec<OrphanProjection> = Vec::new();
    let mut conflicts: Vec<PayloadConflict> = Vec::new();
    let mut projected_slots: BTreeSet<u32> = BTreeSet::new();

    for obs in observations {
        // Non-terminal topics are pre-filtered by
        // `scan_review_projection_observations`. Defensive guard
        // here in case a caller passes mixed observations.
        if obs.topic != "review.unit.done" {
            continue;
        }
        if let Some(slot_idx) = obs.slot_index {
            projected_slots.insert(slot_idx);
        }
        let store_status = obs
            .slot_index
            .and_then(|idx| snapshot.slots.iter().find(|(i, _)| *i == idx))
            .map(|(_, status)| *status);
        let slot_evidence_completed = obs
            .slot_index
            .map(|idx| authoritative.contains(&idx))
            .unwrap_or(false);

        match (obs.slot_index, store_status) {
            (Some(idx), Some(status))
                if status == SlotStatus::Completed && slot_evidence_completed =>
            {
                // Authoritative slot, but the main row's
                // `dimension` is the secondary signal: it must
                // agree with the assignment. A disagreement
                // here is a payload conflict (the projection
                // exists for an authoritative slot but with the
                // wrong dimension) — never silent acceptance.
                if let Some(assigned) = expected_dimensions.get(&idx)
                    && obs.dimension.as_deref() != Some(assigned.as_str())
                {
                    conflicts.push(PayloadConflict {
                        slot_index: Some(idx),
                        expected_dimension: Some(assigned.clone()),
                        actual_dimension: obs.dimension.clone().unwrap_or_default(),
                        payload_fingerprint: obs.payload_fingerprint.clone(),
                        line_no: obs.line_no,
                    });
                }
            }
            (Some(idx), Some(_)) => {
                // Slot exists in the store but is NOT
                // authoritatively Completed. The main row is an
                // orphan projection regardless of dimension match:
                // a Failed slot cannot validate ANY main row.
                orphans.push(OrphanProjection {
                    slot_index: Some(idx),
                    dimension: obs.dimension.clone(),
                    payload_fingerprint: obs.payload_fingerprint.clone(),
                    line_no: obs.line_no,
                    store_status,
                });
            }
            (Some(_), None) => {
                // Slot index present in main but no slot in the
                // store matches. Capture as orphan.
                orphans.push(OrphanProjection {
                    slot_index: obs.slot_index,
                    dimension: obs.dimension.clone(),
                    payload_fingerprint: obs.payload_fingerprint.clone(),
                    line_no: obs.line_no,
                    store_status: None,
                });
            }
            (None, _) => {
                // No slot index on the main row. Always an
                // orphan: we cannot tie it to any authoritative
                // completion, and the dispatcher cannot tell a
                // same-wave row from a stale one.
                orphans.push(OrphanProjection {
                    slot_index: None,
                    dimension: obs.dimension.clone(),
                    payload_fingerprint: obs.payload_fingerprint.clone(),
                    line_no: obs.line_no,
                    store_status: None,
                });
            }
        }
    }

    // Missing projection: authoritative slot has no corresponding
    // main observation. Iterate the authoritative set; anything
    // without a matching slot index in the observations is missing.
    let mut missing_projections: Vec<u32> = Vec::new();
    for slot in authoritative {
        if !projected_slots.contains(slot) {
            missing_projections.push(*slot);
        }
    }
    missing_projections.sort();

    (orphans, missing_projections, conflicts)
}

/// Convenience pure helper: read a main-ledger file (one JSONL row
/// per line) and produce the same-wave `review.unit.done` rows as
/// `ProjectionObservation`s. The scan is bounded by the wave id
/// (rows without a matching `wave_id` are dropped, fail-closed) and
/// by topic (rows whose `topic` is not `review.unit.done` are
/// dropped). Malformed rows are skipped silently; per-line
/// diagnostics are out of scope for the reconciliation module.
pub fn scan_review_projection_observations(
    main_events_jsonl: &str,
    wave_id: &str,
) -> Vec<ProjectionObservation> {
    use std::io::BufRead;
    let mut out: Vec<ProjectionObservation> = Vec::new();
    let cursor = std::io::Cursor::new(main_events_jsonl.as_bytes());
    for (line_no, line) in cursor.lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let record: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if record.get("topic").and_then(|t| t.as_str()) != Some("review.unit.done") {
            continue;
        }
        if record.get("wave_id").and_then(|w| w.as_str()) != Some(wave_id) {
            continue;
        }
        let slot_index = record
            .get("slot_index")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        let payload = record
            .get("payload")
            .and_then(|p| p.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                record
                    .get("payload")
                    .and_then(|p| p.as_object())
                    .map(|obj| serde_json::to_string(obj).unwrap_or_default())
            });
        let (dimension, payload_str) = match payload.as_deref() {
            Some(s) => {
                let dim = serde_json::from_str::<serde_json::Value>(s)
                    .ok()
                    .and_then(|v| {
                        v.get("dimension")
                            .and_then(|d| d.as_str())
                            .map(String::from)
                    });
                (dim, s.to_string())
            }
            None => (None, String::new()),
        };
        out.push(ProjectionObservation {
            slot_index,
            topic: "review.unit.done".to_string(),
            wave_id: Some(wave_id.to_string()),
            dimension,
            payload_fingerprint: fingerprint_payload(&payload_str),
            line_no,
        });
    }
    out
}

/// Convenience: read the per-slot terminal evidence from a
/// `SupervisorBridge`-like source. The trait object is `dyn
/// SupervisorBridge`; the dispatcher passes the bridge and this
/// helper drains the evidence into a `HashMap` for the pure
/// reconciliation function. Stays in this module so the
/// `reconcile_review_wave` signature can stay closure-free.
pub fn collect_evidence(
    bridge: &dyn crate::supervisor::SupervisorBridge,
    snapshot: &WaveSnapshot,
) -> HashMap<u32, TerminalEvidence> {
    let mut out: HashMap<u32, TerminalEvidence> = HashMap::new();
    for (slot_index, status) in &snapshot.slots {
        if !matches!(status, SlotStatus::Completed) {
            continue;
        }
        match bridge.slot_terminal_evidence(&snapshot.wave_id, *slot_index) {
            Ok(Some(ev)) => {
                out.insert(*slot_index, ev);
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    wave_id = %snapshot.wave_id,
                    slot_index = slot_index,
                    error = %err,
                    "reconciliation: slot_terminal_evidence lookup failed; \
                     treating as missing evidence (fail-closed)"
                );
            }
        }
    }
    out
}

/// Free function the dispatcher calls instead of computing
/// `missing_dimensions` from the union of main backscan + store
/// completed. Only the store-side `authoritative_completed` flows
/// in; main backscan no longer participates in completion
/// calculation. The result is sorted and deduplicated to match the
/// public payload shape.
pub fn compute_review_missing_dimensions(
    assigned: &HashSet<String>,
    authoritative_completed_dimensions: &HashSet<String>,
) -> Vec<String> {
    let mut missing: Vec<String> = assigned
        .iter()
        .filter(|d| !authoritative_completed_dimensions.contains(*d))
        .cloned()
        .collect();
    missing.sort();
    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::WaveKind;

    fn snap(slots: Vec<(u32, SlotStatus)>) -> WaveSnapshot {
        WaveSnapshot {
            wave_id: "w".to_string(),
            kind: WaveKind::Review,
            phase: crate::supervisor::WavePhase::Collect,
            expected_total: slots.len() as u32,
            completed_count: slots
                .iter()
                .filter(|(_, s)| matches!(s, SlotStatus::Completed))
                .count() as u32,
            failed_count: slots
                .iter()
                .filter(|(_, s)| matches!(s, SlotStatus::Failed))
                .count() as u32,
            pending_count: 0,
            in_flight_count: 0,
            cancel_requested: false,
            // U5: `salvage_merged` / `merged_to_events` booleans
            // removed; superseded by `WaveDeliveryState` +
            // receipt summaries on the persistence side.
            // Reconciliation observes slot status only.
            delivery_state: WaveDeliveryState::Pending,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            slots,
        }
    }

    fn ev(dim: &str) -> TerminalEvidence {
        TerminalEvidence::from_event(
            "review.unit.done",
            &serde_json::json!({ "dimension": dim }).to_string(),
        )
    }

    #[test]
    fn completed_with_valid_evidence_is_authoritative() {
        let mut dims = HashMap::new();
        dims.insert(0, "correctness".to_string());
        dims.insert(1, "testing".to_string());
        let snap = snap(vec![(0, SlotStatus::Completed), (1, SlotStatus::Failed)]);
        let mut ev_map = HashMap::new();
        ev_map.insert(0, ev("correctness"));
        // Plant a main observation for slot 0 so the missing-projection
        // path stays empty for this test.
        let observations = vec![ProjectionObservation {
            slot_index: Some(0),
            topic: "review.unit.done".to_string(),
            wave_id: Some("w".to_string()),
            dimension: Some("correctness".to_string()),
            payload_fingerprint: fingerprint_payload("{\"dimension\":\"correctness\"}"),
            line_no: 0,
        }];
        let r = reconcile_review_wave(
            &snap,
            &dims,
            &observations,
            "review.unit.done",
            &ev_map,
            None,
        );
        assert_eq!(r.authoritative_completed, vec![0]);
        assert_eq!(r.missing_dimensions, vec!["testing".to_string()]);
        assert!(r.orphan_projections.is_empty());
        assert!(r.missing_projections.is_empty());
        assert!(r.payload_conflicts.is_empty());
    }

    #[test]
    fn six_failed_with_five_orphan_main_does_not_complete() {
        // Implementation-review primary-20260727 accident.
        let mut dims = HashMap::new();
        for (i, d) in [
            "goal-alignment",
            "correctness",
            "maintainability",
            "adversarial",
            "testing",
            "project-standards",
        ]
        .iter()
        .enumerate()
        {
            dims.insert(i as u32, d.to_string());
        }
        let snap = snap(
            (0u32..6)
                .map(|i| (i, SlotStatus::Failed))
                .collect::<Vec<_>>(),
        );
        let observations: Vec<ProjectionObservation> = [
            "goal-alignment",
            "correctness",
            "maintainability",
            "adversarial",
            "project-standards",
        ]
        .iter()
        .enumerate()
        .map(|(i, d)| ProjectionObservation {
            slot_index: Some(i as u32),
            topic: "review.unit.done".to_string(),
            wave_id: Some("w".to_string()),
            dimension: Some((*d).to_string()),
            payload_fingerprint: fingerprint_payload(
                &serde_json::json!({"dimension": d}).to_string(),
            ),
            line_no: i,
        })
        .collect();
        let r = reconcile_review_wave(
            &snap,
            &dims,
            &observations,
            "review.unit.done",
            &HashMap::new(),
            None,
        );
        assert!(r.authoritative_completed.is_empty());
        assert_eq!(
            r.missing_dimensions,
            vec![
                "adversarial".to_string(),
                "correctness".to_string(),
                "goal-alignment".to_string(),
                "maintainability".to_string(),
                "project-standards".to_string(),
                "testing".to_string(),
            ]
        );
        // 5 main rows on 6 Failed slots: all 5 are orphans
        // (Failed slot → orphan regardless of dimension). The
        // slot 4 row uses `project-standards` but slot 4 is
        // assigned `testing`; that mismatch is second-order when
        // the slot is Failed and is folded into the same
        // orphan-projection bucket.
        assert_eq!(r.orphan_projections.len(), 5);
        assert_eq!(r.payload_conflicts.len(), 0);
    }

    #[test]
    fn dimension_mismatch_on_failed_slot_marks_orphan() {
        // When the store says Failed, dimension disagreement is
        // second-order: the slot is not authoritative, so the row
        // is an orphan (not a payload conflict). Conflicts only
        // arise when the slot IS Completed with valid evidence but
        // the row's dimension disagrees (covered separately).
        let mut dims = HashMap::new();
        dims.insert(0, "correctness".to_string());
        let snap = snap(vec![(0, SlotStatus::Failed)]);
        let observations = vec![ProjectionObservation {
            slot_index: Some(0),
            topic: "review.unit.done".to_string(),
            wave_id: Some("w".to_string()),
            dimension: Some("performance".to_string()),
            payload_fingerprint: fingerprint_payload("{\"dimension\":\"performance\"}"),
            line_no: 0,
        }];
        let r = reconcile_review_wave(
            &snap,
            &dims,
            &observations,
            "review.unit.done",
            &HashMap::new(),
            None,
        );
        assert_eq!(r.orphan_projections.len(), 1);
        assert_eq!(r.payload_conflicts.len(), 0);
        assert_eq!(r.missing_dimensions, vec!["correctness".to_string()]);
    }

    #[test]
    fn dimension_mismatch_on_completed_slot_marks_conflict() {
        // A Completed slot with valid evidence carries an
        // authoritative dimension. If the main row disagrees, it
        // is a payload conflict (the row exists for an
        // authoritative slot, but with the wrong dimension).
        let mut dims = HashMap::new();
        dims.insert(0, "correctness".to_string());
        let snap = snap(vec![(0, SlotStatus::Completed)]);
        let mut ev_map = HashMap::new();
        ev_map.insert(0, ev("correctness"));
        let observations = vec![ProjectionObservation {
            slot_index: Some(0),
            topic: "review.unit.done".to_string(),
            wave_id: Some("w".to_string()),
            dimension: Some("performance".to_string()),
            payload_fingerprint: fingerprint_payload("{\"dimension\":\"performance\"}"),
            line_no: 0,
        }];
        let r = reconcile_review_wave(
            &snap,
            &dims,
            &observations,
            "review.unit.done",
            &ev_map,
            None,
        );
        assert_eq!(r.authoritative_completed, vec![0]);
        assert_eq!(r.payload_conflicts.len(), 1);
        assert_eq!(
            r.payload_conflicts[0].expected_dimension,
            Some("correctness".to_string())
        );
        assert_eq!(
            r.payload_conflicts[0].actual_dimension,
            "performance".to_string()
        );
    }

    #[test]
    fn fingerprint_mismatch_is_fail_closed() {
        let mut dims = HashMap::new();
        dims.insert(0, "correctness".to_string());
        let snap = snap(vec![(0, SlotStatus::Completed)]);
        let mut ev_map = HashMap::new();
        ev_map.insert(0, ev("correctness"));
        let accepted = |_: u32| Some("different-fingerprint".to_string());
        let r = reconcile_review_wave(
            &snap,
            &dims,
            &[],
            "review.unit.done",
            &ev_map,
            Some(&accepted),
        );
        assert_eq!(r.authoritative_completed, Vec::<u32>::new());
        assert!(matches!(
            r.evidence_validations[0].validation,
            EvidenceValidation::FingerprintMismatch { .. }
        ));
    }

    #[test]
    fn completed_evidence_but_no_main_is_missing_projection() {
        let mut dims = HashMap::new();
        dims.insert(0, "correctness".to_string());
        let snap = snap(vec![(0, SlotStatus::Completed)]);
        let mut ev_map = HashMap::new();
        ev_map.insert(0, ev("correctness"));
        let r = reconcile_review_wave(&snap, &dims, &[], "review.unit.done", &ev_map, None);
        assert_eq!(r.authoritative_completed, vec![0]);
        assert_eq!(r.missing_projections, vec![0]);
        assert!(r.missing_dimensions.is_empty());
    }

    #[test]
    fn output_is_deterministically_ordered() {
        let mut dims = HashMap::new();
        dims.insert(0, "zeta".to_string());
        dims.insert(1, "alpha".to_string());
        dims.insert(2, "mu".to_string());
        let snap = snap(vec![
            (0, SlotStatus::Failed),
            (1, SlotStatus::Failed),
            (2, SlotStatus::Failed),
        ]);
        let r = reconcile_review_wave(&snap, &dims, &[], "review.unit.done", &HashMap::new(), None);
        assert_eq!(
            r.missing_dimensions,
            vec!["alpha".to_string(), "mu".to_string(), "zeta".to_string()]
        );
        assert!(r.authoritative_completed.is_empty());
        assert!(r.blocking_slots.is_empty());
    }

    #[test]
    fn db_restart_reconciliation_is_deterministic() {
        // Same inputs → same outputs (DB-restart invariant).
        let mut dims = HashMap::new();
        dims.insert(0, "correctness".to_string());
        dims.insert(1, "testing".to_string());
        let snap = snap(vec![(0, SlotStatus::Completed), (1, SlotStatus::Completed)]);
        let mut ev_map = HashMap::new();
        ev_map.insert(0, ev("correctness"));
        ev_map.insert(1, ev("testing"));
        let r1 = reconcile_review_wave(&snap, &dims, &[], "review.unit.done", &ev_map, None);
        let r2 = reconcile_review_wave(&snap, &dims, &[], "review.unit.done", &ev_map, None);
        assert_eq!(r1, r2);
    }

    #[test]
    fn validate_terminal_evidence_completed_no_evidence() {
        let v = validate_terminal_evidence(
            0,
            SlotStatus::Completed,
            None,
            "review.unit.done",
            Some("correctness"),
            None,
        );
        assert!(matches!(v, EvidenceValidation::MissingEvidence { .. }));
    }

    #[test]
    fn validate_terminal_evidence_wrong_topic() {
        let v = validate_terminal_evidence(
            0,
            SlotStatus::Completed,
            Some(&ev("correctness")),
            "review.unit.done",
            Some("correctness"),
            None,
        );
        assert!(matches!(v, EvidenceValidation::Valid));
        let wrong_topic = TerminalEvidence::from_event(
            "review.dimension.done",
            "{\"dimension\":\"correctness\"}",
        );
        let v = validate_terminal_evidence(
            0,
            SlotStatus::Completed,
            Some(&wrong_topic),
            "review.unit.done",
            Some("correctness"),
            None,
        );
        assert!(matches!(v, EvidenceValidation::TopicMismatch { .. }));
    }

    #[test]
    fn scan_review_projection_observations_filters_other_waves() {
        let jsonl = r#"{"topic":"review.unit.done","payload":{"dimension":"correctness"},"wave_id":"W-main","slot_index":0}
{"topic":"review.unit.done","payload":{"dimension":"security"},"wave_id":"W-other","slot_index":0}
{"topic":"review.unit.done","payload":{"dimension":"testing"},"slot_index":1}
{"topic":"other.topic","payload":{},"wave_id":"W-main"}
not json
{"topic":"review.unit.done","payload":"{\"dimension\":\"perf\"}","wave_id":"W-main","slot_index":2}
"#;
        let obs = scan_review_projection_observations(jsonl, "W-main");
        let dims: Vec<_> = obs.iter().map(|o| o.dimension.clone()).collect();
        assert_eq!(
            dims,
            vec![Some("correctness".to_string()), Some("perf".to_string()),]
        );
    }
}
