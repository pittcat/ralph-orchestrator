//! Unified orchestrator state (`StateLedger` + commit log).
//!
//! Plan ref: U1 of
//! `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md`.
//!
//! The state module defines:
//! - [`LedgerSnapshot`] — the unified runtime state projection.
//!   Replaces the in-memory trackers spread across `LoopState`,
//!   `StateProjector::ProjectionContext`, the policy / state
//!   machine / flow / handoff registries, and the legacy
//!   `tasks.jsonl` / `progress.md` ledgers.
//! - [`Commit`] / [`CommitDelta`] — the append-only state
//!   mutation log, persisted as `.ralph/ledger.jsonl`.
//! - [`StateLedger`] — the runtime container that owns the
//!   snapshot + commit log and provides commit / replay /
//!   feature-flag opt-in.
//!
//! ## Feature flag
//!
//! The `UNIFIED_STATE_LEDGER=1` env var is the U1 opt-in switch.
//! The state module does not consult the env var itself — the
//! loop constructor resolves the boolean and passes it via
//! [`StateLedger::new`]. When the flag is off, every `commit()` is
//! a no-op and `replay_from_disk` returns an empty snapshot, so
//! the legacy code path is fully preserved.
//!
//! ## U1 scope
//!
//! U1 implements the structural model and persistence. U2
//! onwards migrate the runtime read/write sites. The U1 test
//! suite (`tests.rs`) covers the snapshot/commit/ledger
//! behaviour in isolation; the integration tests live in
//! U9.

#[cfg(test)]
mod tests;

mod commit;
mod ledger;
/// U7a persistent rejection log — `.ralph/recovery.jsonl` writer
/// for the deterministic-correction path.  Mirrors the
/// diagnostics `recovery.jsonl` line shape but lives at the
/// workspace root so it survives `RALPH_DIAGNOSTICS=0`.
pub mod recovery_log;
mod snapshot;

pub use commit::{Commit, CommitDelta, CounterKind, TaskTransition};
pub use ledger::{
    HandoffAcceptedInputs, HandoffCommitOutcome, LEDGER_RELATIVE_PATH, LedgerError, StateLedger,
    read_commit_log, truncate_after,
};
pub use recovery_log::{
    RECOVERY_LOG_RELATIVE_PATH, RejectionRecord, append_rejection, read_rejection_log,
    recovery_log_path, retry_count_for,
};
pub use snapshot::{LedgerSnapshot, ObligationTriggerRecord, SerializedLintResumeHint};
