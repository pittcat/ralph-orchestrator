//! Wiring layer for U4 `IdempotentLog` to existing
//! task / diagnosis / drift consumers (U8).
//!
//! Why this is a separate module: the in-memory data
//! structures (`TaskStore`, the recovery envelope writer, the
//! drift engine) are deeply integrated with the runtime. A
//! wholesale migration would break hundreds of unit tests
//! that exercise the in-memory path. U8 therefore adds a
//! thin, opt-in wiring layer that the runtime can route
//! writes through **when** the operator enables the
//! `mechanism.state_idempotency: required` flag in the
//! preset's `mechanism:` block.
//!
//! Cross-platform / concurrency semantics: delegates to
//! `state::idempotent_log::IdempotentLog`, which already
//! covers the OS-lock story.
//!
//! # Idempotency key conventions
//!
//! - `task:{task_id}:loop:{loop_id}` — one record per task
//!   per loop. The record is marked `_final=true` when the
//!   task reaches a terminal state.
//! - `recovery:{retry_key}:loop:{loop_id}` — one record per
//!   recovery retry. `_final=true` when the repair closes.
//! - `drift:{finding_id}:loop:{loop_id}` — one record per
//!   drift finding. Drift findings are advisory; the record
//!   is `_final=true` from creation.

use crate::state::idempotent_log::{IdempotentError, IdempotentLog, IdempotentRecord};
use serde_json::Value;

/// Build the canonical idempotency key for a task.
pub fn task_key(task_id: &str, loop_id: &str) -> String {
    format!("task:{task_id}:loop:{loop_id}")
}

/// Build the canonical idempotency key for a recovery record.
pub fn recovery_key(retry_key: &str, loop_id: &str) -> String {
    format!("recovery:{retry_key}:loop:{loop_id}")
}

/// Build the canonical idempotency key for a drift finding.
pub fn drift_key(finding_id: &str, loop_id: &str) -> String {
    format!("drift:{finding_id}:loop:{loop_id}")
}

/// Errors specific to the wiring layer (wraps IdempotentError
/// and adds `MissingLoopId` for the task-store contract).
#[derive(Debug, thiserror::Error)]
pub enum WiringError {
    #[error("task_id `{0}` cannot be written without a non-empty loop_id")]
    MissingLoopId(String),
    #[error("idempotent log: {0}")]
    Idempotent(#[from] IdempotentError),
}

/// Write a task to the idempotent log. Returns
/// `WiringError::MissingLoopId` if the loop_id is empty
/// (matches the plan's "no loop_id → no write" contract).
pub fn write_task(
    log: &mut IdempotentLog,
    task_id: &str,
    loop_id: &str,
    payload: Value,
    is_final: bool,
) -> Result<(), WiringError> {
    if loop_id.trim().is_empty() {
        return Err(WiringError::MissingLoopId(task_id.to_string()));
    }
    let key = task_key(task_id, loop_id);
    let mut record = IdempotentRecord::new(key)
        .with_payload(payload)
        .with_final(is_final);
    if is_final {
        record = record.with_transition(None, "closed");
    }
    log.append(record)?;
    Ok(())
}

/// Write a recovery record. `retry_key` is the same id the
/// pre-U2 `stall_recovery_counts` map used so on-disk data
/// lines up with what the loop_state would have indexed.
pub fn write_recovery(
    log: &mut IdempotentLog,
    retry_key: &str,
    loop_id: &str,
    payload: Value,
    is_final: bool,
) -> Result<(), WiringError> {
    if loop_id.trim().is_empty() {
        return Err(WiringError::MissingLoopId(retry_key.to_string()));
    }
    let key = recovery_key(retry_key, loop_id);
    let record = IdempotentRecord::new(key)
        .with_payload(payload)
        .with_final(is_final);
    log.append(record)?;
    Ok(())
}

/// Write a drift finding. Drift records are `_final=true` from
/// creation — the engine only writes after the finding has
/// been resolved, never partway through.
pub fn write_drift(
    log: &mut IdempotentLog,
    finding_id: &str,
    loop_id: &str,
    payload: Value,
) -> Result<(), WiringError> {
    if loop_id.trim().is_empty() {
        return Err(WiringError::MissingLoopId(finding_id.to_string()));
    }
    let key = drift_key(finding_id, loop_id);
    let record = IdempotentRecord::new(key)
        .with_payload(payload)
        .with_final(true);
    log.append(record)?;
    Ok(())
}

/// Build a `DiagnosisSummary`-shaped count from the
/// idempotent log's final records. Plan SC-5: this count
/// must equal the number of `_final=true` records on disk
/// so the on-disk JSONL and the in-memory summary never
/// disagree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosisSummary {
    pub recovery_count: usize,
    pub drift_finding_count: usize,
    pub task_count: usize,
}

impl DiagnosisSummary {
    pub fn from_final_records(
        records: &[IdempotentRecord],
    ) -> Self {
        let mut summary = Self::default();
        for r in records {
            if !r._final {
                continue;
            }
            if r._idempotency_key.starts_with("recovery:") {
                summary.recovery_count += 1;
            } else if r._idempotency_key.starts_with("drift:") {
                summary.drift_finding_count += 1;
            } else if r._idempotency_key.starts_with("task:") {
                summary.task_count += 1;
            }
        }
        summary
    }
}

#[cfg(test)]
mod tests;