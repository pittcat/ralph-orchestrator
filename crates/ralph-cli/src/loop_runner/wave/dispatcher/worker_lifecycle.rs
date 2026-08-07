//! Worker lifecycle module — supervisor slot release Drop guard.
//!
//! Originally part of `wave/dispatcher.rs` (plan `2026-08-07-008`).
//! Public surface and behaviour preserved verbatim.

use std::sync::Arc;
use tracing::warn;

/// Synchronously returns a supervisor slot to terminal state when a
/// worker task exits, including JoinSet cancellation/abort. A Drop
/// guard is required because an aborted async task never executes code
/// after its awaited executor future.
///
/// 2026-07-23-007 plan U6 (A2 / A5): the drop guard NEVER
/// overwrites a slot the worker task already drove to a terminal
/// state. The supervisor store's `release_slot_dispatch` is
/// idempotent (no-op when the slot is already `Completed` /
/// `Failed` / `Cancelled`), so a panic between
/// `record_slot_result` and `guard.outcome = Completed` cannot
/// downgrade a terminal write — the existing
/// `release_slot_dispatch(Completed | Failed)` call is a safe
/// no-op. The `outcome` field is kept so the guard preserves the
/// explicit `Completed` signal for the dispatch_records
/// transition; the store's `IN ('dispatched','running')` predicate
/// is the actual safety gate.
pub(crate) struct SupervisorSlotRelease {
    pub(crate) bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    pub(crate) wave_id: String,
    pub(crate) slot_index: u32,
    pub(crate) outcome: ralph_core::supervisor::DispatchOutcome,
}

impl Drop for SupervisorSlotRelease {
    fn drop(&mut self) {
        if let Err(error) =
            self.bridge
                .release_slot_dispatch(&self.wave_id, self.slot_index, self.outcome)
        {
            warn!(
                wave_id = %self.wave_id,
                slot_index = self.slot_index,
                %error,
                "supervisor terminal permit release failed"
            );
        }
    }
}
