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
//! The unified state ledger is always enabled. The legacy no-op
//! fallback has been removed.
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
/// 2026-06-27 mechanism foundation U4: idempotent JSONL log
/// writer (atomic rename + OS file lock). Wired into
/// task_store / diagnosis / drift consumers in U8.
pub mod idempotent_log;
mod knowledge;
mod ledger;
/// U7a persistent rejection log — `.ralph/recovery.jsonl` writer
/// for the deterministic-correction path.  Mirrors the
/// diagnostics `recovery.jsonl` line shape but lives at the
/// workspace root so it survives `RALPH_DIAGNOSTICS=0`.
pub mod recovery_log;
mod snapshot;

#[cfg(test)]
// 2026-07-02-006 plan U12: pin the new `workflow_phase`
// field. Lives next to `state::snapshot` so the field's
// ownership stays obvious to anyone reading the field
// declaration.
mod snapshot_workflow_phase_tests;

pub use commit::{Commit, CommitDelta, CounterKind, TaskTransition};
pub use knowledge::{
    DISPLAY_RECORDS_MAX, EVIDENCE_REFS_MAX, CommitObservationOutcome, EvidenceFreshness,
    EvidenceRef, InputFingerprint, KnowledgeAuthority, KnowledgeBuildError, KnowledgeKind,
    KnowledgeRecord, KnowledgeRecordBuilder, KnowledgeView, OrchestrationKnowledgeState,
    PROMPT_FIELD_MAX_BYTES, PROMPT_HEADING, PROMPT_RECORDS_VISIBLE, SEMANTIC_FIELD_MAX_BYTES,
    VerificationStatus, accepted_source_ref, commit_accepted_observations, observation_id,
    observations_from_accepted_events, payload_digest_hex, render_prompt_block,
};
pub use ledger::{LEDGER_RELATIVE_PATH, LedgerError, StateLedger, read_commit_log, truncate_after};
pub use recovery_log::{
    RECOVERY_LOG_RELATIVE_PATH, RejectionRecord, append_rejection, read_rejection_log,
    recovery_log_path, retry_count_for,
};
pub use snapshot::{LedgerSnapshot, ObligationTriggerRecord, SerializedLintResumeHint};
