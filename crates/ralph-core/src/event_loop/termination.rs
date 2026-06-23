//! 2026-06-23-005 F4 (P0-2 重定位): typed `TerminationTrigger` SSOT.
//!
//! ## Status (F4)
//!
//! This module is **typed-enum infrastructure only**. It defines
//! the `TerminationTrigger` and `DeadLetterSource` enums, plus
//! the `trigger_to_reason` mapper that converts a typed trigger
//! into the existing `TerminationReason`. The plan
//! (`2026-06-23-005-fix-ce-executor-serial-hard-gate-half-edge-recovery-plan.md`
//! U3 / R11 / R14 / KTD-7) described a fuller refactor that:
//!
//! 1. Removes the `pending_dead_letter` field from `LoopState`.
//! 2. Refactors `process_output` to a single `match
//!    TerminationTrigger` branch instead of three independent `if`s.
//! 3. Adds a `LOOP_STATE_SCHEMA_VERSION` constant + a
//!    `deserialize_v1` migration path for `.ralph/state.json`.
//!
//! **None of items 1-3 are implemented in F4.** The plan's
//! `## Problem Frame` section asserted these as the current
//! state, but the codebase (verified at F4 review time) does not
//! match that assertion:
//!
//! - **`pending_dead_letter` does not exist** in the current
//!   `LoopState` (verified via `rg "pending_dead_letter"
//!   crates/ralph-core/src/event_loop/loop_state.rs` = 0 hits).
//!   The plan's KTD-7 ("process_output 多触发器散落") was based
//!   on a stale read of the codebase.
//! - **`LoopState` has no persistence path** — it carries
//!   `Instant` fields, `HashMap<HatId, _>`, and other
//!   non-`Serialize` types, and is never written to disk. The
//!   only persistence touch-point is
//!   `.ralph/loop-termination-reason.json` which holds a
//!   `TerminationReason` (not a `LoopState`).
//! - **`process_output` already uses a single termination
//!   branch** for the `consecutive_failures >= 5` check
//!   (mod.rs:1369-1373); there is no "3 散落 if" to collapse.
//!
//! F4 therefore establishes the **shape** of the typed trigger
//! SSOT (so future R15 follow-up plans can wire
//! `process_output` to consume it without re-architecting the
//! enum) and exposes typed `push_termination_trigger` /
//! `pop_termination_trigger` APIs on `LoopState` (F4 only adds
//! the field + methods; no caller enqueues triggers yet).
//!
//! ## Future work (deferred)
//!
//! The plan's `Deferred to Follow-Up Work` section lists
//! `process_output` refactor + `LoopState` persistence as
//! out-of-scope for the original 005 plan. F4 follows the
//! same boundary.
//!
//! Reference: `docs/plans/2026-06-23-005-fix-ce-executor-serial-hard-gate-half-edge-recovery-plan.md`
//! U3 / R11 / R14 / KTD-7.

use crate::preset::engine::gates::RejectionKind;

/// Source attribution for a `DeadLetter` trigger (R14: typed enum
/// serialization, no string concatenation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeadLetterSource {
    /// Triggered by the missing-event hard gate
    /// (`hard_gate::inject_missing_event_hard_gate_guidance`).
    HardGate,
    /// Triggered by orchestrator stall_recovery paths
    /// (e.g. `mod.rs:2680` `enrich_task_resume_payload(... "stall_no_events")`).
    StallRecovery,
    /// Triggered by the payload contract rejection.
    PayloadContract,
}

/// Typed termination trigger (KTD-7 SSOT). New triggers extend this
/// enum; `process_output` consumes them via a single match arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationTrigger {
    /// Consecutive failure count reached the threshold (currently 5).
    Failure { consecutive_count: u32 },
    /// A typed dead-letter from `CoordinatorDispatcher::dispatch`
    /// returned `PlanBlocked`.
    DeadLetter {
        kind: RejectionKind,
        source: DeadLetterSource,
    },
    /// The plan completed (e.g. all tasks closed + completion promise).
    PlanComplete { plan_id: String },
}

/// Convert a `TerminationTrigger` into the existing typed
/// `TerminationReason` (which already has all the variants the runtime
/// supports). New triggers only need a new arm here.
pub fn trigger_to_reason(trigger: TerminationTrigger) -> crate::TerminationReason {
    use crate::TerminationReason as R;
    match trigger {
        TerminationTrigger::Failure { .. } => R::ConsecutiveFailures,
        TerminationTrigger::DeadLetter { kind, source } => {
            // Surface as the typed `PayloadContractViolation` variant —
            // the human-readable reason field carries the typed kind
            // for log / `loop-termination-reason.json` aggregation.
            // Source attribute is logged but not yet serialized into
            // the variant (TerminationReason does not carry source
            // attribution today); this is a known follow-up tracked in
            // the plan's U8 deferred-work section.
            tracing::warn!(
                kind = kind.reason_code(),
                ?source,
                "typed dead-letter trigger surfaced as PayloadContractViolation"
            );
            R::PayloadContractViolation
        }
        TerminationTrigger::PlanComplete { .. } => R::CompletionPromise,
    }
}

/// Default queue capacity (P1-6 fix: prevent OOM from runaway triggers).
pub const TRIGGER_QUEUE_CAPACITY: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerminationReason;

    #[test]
    fn failure_trigger_maps_to_consecutive_failures_reason() {
        let reason = trigger_to_reason(TerminationTrigger::Failure {
            consecutive_count: 5,
        });
        assert_eq!(reason, TerminationReason::ConsecutiveFailures);
    }

    #[test]
    fn dead_letter_trigger_maps_to_payload_contract_violation_reason() {
        let reason = trigger_to_reason(TerminationTrigger::DeadLetter {
            kind: RejectionKind::MissingEventGate,
            source: DeadLetterSource::HardGate,
        });
        assert_eq!(reason, TerminationReason::PayloadContractViolation);
    }

    #[test]
    fn plan_complete_trigger_maps_to_completion_promise_reason() {
        let reason = trigger_to_reason(TerminationTrigger::PlanComplete {
            plan_id: "primary-20260623-095708".to_string(),
        });
        assert_eq!(reason, TerminationReason::CompletionPromise);
    }

    #[test]
    fn trigger_queue_capacity_constant_is_stable() {
        // P1-6: documented capacity. Changing this constant requires a
        // corresponding migration of any persistent trigger queue.
        assert_eq!(TRIGGER_QUEUE_CAPACITY, 16);
    }
}
