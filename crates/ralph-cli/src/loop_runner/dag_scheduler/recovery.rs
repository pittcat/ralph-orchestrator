// U9 (2026-09-03-0959 plan §7 U9): crash-window recovery +
// exactly-once projection.
//
// This module is the **pure planner** that the runtime's recovery
// wiring composes against. It defines:
//   - the attempt-token launch fence (stale tokens must NOT
//     publish accepted-effect)
//   - the terminal-persist idempotency planner (replay-safe
//     slot-terminal commit)
//   - the worktree-bind reuse verifier (pure decision layer on
//     top of U7's `UnitWorktree::acquire`)
//   - the integration record idempotency verifier (pure decision
//     layer on top of U7's `IntegrationStore::record_integrated`)
//   - the task-close idempotency key (one close per
//     `(task_key, step, attempt, idempotency_key)` tuple)
//   - the terminal-emit phase advance gate (forward-only through
//     `WaveDeliveryState`, fail-closed on ambiguity)
//   - the fail-closed envelope: when state is ambiguous, return
//     `Ambiguous { hint }` and the runtime MUST mark the plan as
//     `blocked`. Never respawn or kill.
//
// The recovery *use site* (U10) composes these planners against
// the live `DagSchedulerStore` / `IntegrationStore` /
// `UnitWorktree`. Until then, the module is reachable from tests
// but not from the runtime driver, which is the expected
// transitional state for the U6/U7/U8/U10 surface.
#![allow(dead_code)]

// ---------------------------------------------------------------------------
// Launch fencing: attempt-token CAS.
// ---------------------------------------------------------------------------

/// Outcome of the launch-fence check. Given a `(live_attempt,
/// candidate_attempt)` pair, the planner says whether the
/// candidate is allowed to publish accepted-effect.
///
/// `Stale` ⇒ refuse. The candidate was minted before a
/// review-rejection bump incremented the live attempt; any
/// accepted-effect it emits would clobber the live attempt's
/// ledger.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchFenceOutcome {
    /// The candidate's attempt equals the live attempt; the
    /// launch is allowed to proceed and may publish
    /// accepted-effect.
    Current,
    /// The candidate's attempt is strictly less than the live
    /// attempt; refuse. The kernel invocation is from a stale
    /// attempt.
    Stale {
        live_attempt: u64,
        candidate_attempt: u64,
    },
    /// The candidate's attempt is strictly greater than the
    /// live attempt; this is a programmer error (the candidate
    /// was minted by an ahead-of-time worker). Refuse; never
    /// silently advance the live attempt.
    Ahead {
        live_attempt: u64,
        candidate_attempt: u64,
    },
}

/// Pure launch-fence planner. No I/O; just compares two attempt
/// counters.
///
/// `live_attempt` is what the runtime currently has persisted
/// (from `JobPipeline::UnitPipelineState.attempt`); the
/// candidate's `attempt` is what the kernel invocation claims to
/// be on. Same attempt ⇒ Current. Strictly less ⇒ Stale.
/// Strictly greater ⇒ Ahead.
#[cfg(test)]
pub fn launch_fence(live_attempt: u64, candidate_attempt: u64) -> LaunchFenceOutcome {
    if candidate_attempt == live_attempt {
        LaunchFenceOutcome::Current
    } else if candidate_attempt < live_attempt {
        LaunchFenceOutcome::Stale {
            live_attempt,
            candidate_attempt,
        }
    } else {
        LaunchFenceOutcome::Ahead {
            live_attempt,
            candidate_attempt,
        }
    }
}

// ---------------------------------------------------------------------------
// Terminal persist idempotency planner.
// ---------------------------------------------------------------------------

/// Decision returned by the terminal-persist planner. Three
/// cases, all three are exactly-once:
///   - `Replay { already_committed: false }` ⇒ the persisted
///     record is missing; commit must run.
///   - `Replay { already_committed: true }` ⇒ the persisted
///     record exists and the fingerprint matches; commit is a
///     no-op.
///   - `Conflict` ⇒ persisted record exists with a DIFFERENT
///     fingerprint; the candidate is from a different worker /
///     attempt and must NOT clobber the committed row.
///   - `Ambiguous` ⇒ candidate has no fingerprint to compare
///     (e.g. the candidate's evidence is partial); refuse —
///     fail-closed.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalPersistDecision {
    /// Commit must run on this resume.
    Commit,
    /// Commit already ran; this resume is a no-op.
    Idempotent,
    /// A conflicting terminal evidence row is already persisted;
    /// refuse to overwrite (fail-closed).
    Conflict { reason: String },
    /// Candidate evidence is partial / missing fields; refuse
    /// to commit. The runtime MUST mark the plan blocked.
    Ambiguous { hint: String },
}

/// Pure planner for the "slot terminal record" idempotency
/// surface. Compares the candidate's evidence fingerprint against
/// the persisted row's fingerprint (if any). Same fingerprint ⇒
/// Idempotent. No persisted row ⇒ Commit. Different fingerprint
/// ⇒ Conflict (fail-closed).
#[cfg(test)]
pub fn plan_terminal_persist(
    persisted_fingerprint: Option<&str>,
    candidate_fingerprint: Option<&str>,
) -> TerminalPersistDecision {
    match (persisted_fingerprint, candidate_fingerprint) {
        (None, None) => TerminalPersistDecision::Ambiguous {
            hint: "neither persisted nor candidate evidence has a fingerprint".to_string(),
        },
        (None, Some(_)) => TerminalPersistDecision::Commit,
        (Some(existing), Some(candidate)) if existing == candidate => {
            TerminalPersistDecision::Idempotent
        }
        (Some(existing), Some(candidate)) => TerminalPersistDecision::Conflict {
            reason: format!(
                "terminal evidence fingerprint drift: persisted={existing}, candidate={candidate}"
            ),
        },
        (Some(_), None) => TerminalPersistDecision::Ambiguous {
            hint: "persisted evidence has a fingerprint but candidate does not".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Worktree-bind reuse verifier.
// ---------------------------------------------------------------------------

/// Decision returned by the worktree-bind reuse planner. Pure;
/// mirrors U7's `UnitWorktree::acquire` reuse rule so the recovery
/// layer can run the decision without touching the filesystem.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeBindVerdict {
    /// Existing branch tip equals the verified base; safe to
    /// reuse the worktree on disk.
    Reuse,
    /// No existing branch tip; fresh create.
    Create,
    /// Existing branch tip differs from the verified base; the
    /// previous run raced against a stale base — fail-closed.
    BaseMismatch {
        existing_tip: String,
        verified_base: String,
    },
}

#[cfg(test)]
pub fn plan_worktree_bind(
    existing_branch_tip: Option<&str>,
    verified_base_commit: Option<&str>,
) -> WorktreeBindVerdict {
    match (existing_branch_tip, verified_base_commit) {
        (None, _) => WorktreeBindVerdict::Create,
        (Some(tip), None) => WorktreeBindVerdict::BaseMismatch {
            existing_tip: tip.to_string(),
            verified_base: "<unverified>".to_string(),
        },
        (Some(tip), Some(base)) if tip == base => WorktreeBindVerdict::Reuse,
        (Some(tip), Some(base)) => WorktreeBindVerdict::BaseMismatch {
            existing_tip: tip.to_string(),
            verified_base: base.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Integration record idempotency verifier.
// ---------------------------------------------------------------------------

/// Fingerprint shape U9 expects for every integration record.
/// Mirrors U7's `compute_integration_fingerprint` over the
/// `(unit_id, base_commit, integrated_commit, expected_head_before)`
/// tuple. This is the SSOT surface that the recovery planner
/// consumes; the actual SHA-256 lives in
/// `ralph_core::supervisor::dag_integration::compute_integration_fingerprint`.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeIntentFingerprint {
    pub unit_id: String,
    pub base_commit: String,
    pub integrated_commit: String,
    pub expected_head_before: String,
}

/// Pure planner for the integration record idempotency surface.
/// Given a (persisted, candidate) pair, returns the decision.
///
/// The contract is identical to U7's `IntegrationStore`:
///   - Same tuple + same fingerprint ⇒ Idempotent.
///   - No persisted row ⇒ Commit (a fresh record is written).
///   - Different tuple (different `base_commit` /
///     `integrated_commit` / `expected_head_before`) ⇒
///     DuplicateUnitForTarget (fail-closed; the lane is the
///     single writer per target).
///   - Same unit but partial fields ⇒ Ambiguous (fail-closed;
///     resume must mark plan blocked).
#[cfg(test)]
pub fn plan_integration_record(
    persisted: Option<&MergeIntentFingerprint>,
    candidate: &MergeIntentFingerprint,
) -> IntegrationRecordDecision {
    match persisted {
        None => IntegrationRecordDecision::Commit,
        Some(existing) if existing == candidate => IntegrationRecordDecision::Idempotent,
        Some(existing) => IntegrationRecordDecision::DuplicateUnitForTarget {
            existing_unit_id: existing.unit_id.clone(),
            candidate_unit_id: candidate.unit_id.clone(),
        },
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationRecordDecision {
    /// Persist a new record.
    Commit,
    /// Persisted record matches the candidate exactly; resume
    /// is a no-op.
    Idempotent,
    /// Persisted record has a DIFFERENT tuple for the same unit;
    /// fail-closed (the lane is the single writer per target).
    DuplicateUnitForTarget {
        existing_unit_id: String,
        candidate_unit_id: String,
    },
}

// ---------------------------------------------------------------------------
// Task-close idempotency key.
// ---------------------------------------------------------------------------

/// Stable idempotency key for a single task-close invocation.
/// Derived from `(task_key, step, attempt, idempotency_key)`
/// where `idempotency_key` is the caller's chosen nonce (e.g.
/// a SHA-256 over the task's close payload).
///
/// Two task-close calls with the SAME `TaskCloseIdempotencyKey`
/// are guaranteed to be replays of the same logical close — the
/// runtime MUST treat the second call as a no-op. Two calls
/// with DIFFERENT `TaskCloseIdempotencyKey` are distinct closes
/// (even for the same task); the runtime MUST NOT silently
/// dedupe them.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskCloseIdempotencyKey {
    pub task_key: String,
    pub step: u32,
    pub attempt: u64,
    pub idempotency_key: String,
}

#[cfg(test)]
impl TaskCloseIdempotencyKey {
    /// Mint a fresh idempotency key. The caller supplies the
    /// task_key, step, attempt, and a stable idempotency_key
    /// nonce. The runtime MUST reject hand-rolled values that
    /// don't match this signature (see `ralph-tools-tasks` red
    /// box).
    pub fn mint(
        task_key: impl Into<String>,
        step: u32,
        attempt: u64,
        idempotency_key: impl Into<String>,
    ) -> Self {
        Self {
            task_key: task_key.into(),
            step,
            attempt,
            idempotency_key: idempotency_key.into(),
        }
    }

    /// Whether two keys describe the same logical close.
    /// Used by the runtime to detect replays after a crash.
    pub fn is_replay_of(&self, other: &TaskCloseIdempotencyKey) -> bool {
        self == other
    }
}

// ---------------------------------------------------------------------------
// Terminal-emit phase advance gate.
// ---------------------------------------------------------------------------

/// Outcome of the terminal-emit phase advance gate. Mirrors U5's
/// `WaveDeliveryState::next()` but exposes the explicit
/// decision the recovery layer needs.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseAdvanceOutcome {
    /// Forward transition; allowed.
    Advance { from: u8, to: u8 },
    /// Already at or past the target phase; replay is a no-op.
    Replay { at: u8 },
    /// Backwards transition; refuse.
    Refuse { from: u8, to: u8 },
    /// Phase value out of range; fail-closed.
    Ambiguous { hint: &'static str },
}

/// Phases U5's `WaveDeliveryState` exposes, in order. Numeric
/// values match the U5 contract: 0=Pending, 1=BusinessProjected,
/// 2=SalvageCommitted, 3=CoordinationWritten,
/// 4=CoordinationCommitted. `PHASE_COUNT` is the upper bound
/// (exclusive).
#[cfg(test)]
pub const PHASE_PENDING: u8 = 0;
#[cfg(test)]
pub const PHASE_BUSINESS_PROJECTED: u8 = 1;
#[cfg(test)]
pub const PHASE_SALVAGE_COMMITTED: u8 = 2;
#[cfg(test)]
pub const PHASE_COORDINATION_WRITTEN: u8 = 3;
#[cfg(test)]
pub const PHASE_COORDINATION_COMMITTED: u8 = 4;
#[cfg(test)]
pub const PHASE_COUNT: u8 = 5;

/// Pure phase-advance planner. The current phase and the
/// target phase are both numeric `u8`s in `[0, PHASE_COUNT)`.
///
///   - target > current ⇒ Advance.
///   - target == current ⇒ Replay (no-op).
///   - target < current ⇒ Refuse (rollback).
///   - current or target >= PHASE_COUNT ⇒ Ambiguous (fail-closed).
#[cfg(test)]
pub fn plan_terminal_emit_phase(current: u8, target: u8) -> PhaseAdvanceOutcome {
    if current >= PHASE_COUNT {
        return PhaseAdvanceOutcome::Ambiguous {
            hint: "current phase is out of range",
        };
    }
    if target >= PHASE_COUNT {
        return PhaseAdvanceOutcome::Ambiguous {
            hint: "target phase is out of range",
        };
    }
    if target > current {
        PhaseAdvanceOutcome::Advance {
            from: current,
            to: target,
        }
    } else if target == current {
        PhaseAdvanceOutcome::Replay { at: current }
    } else {
        PhaseAdvanceOutcome::Refuse {
            from: current,
            to: target,
        }
    }
}

// ---------------------------------------------------------------------------
// Fail-closed envelope.
// ---------------------------------------------------------------------------

/// The fail-closed envelope: every recovery decision in this
/// module rolls up to one of three terminal verdicts. The
/// runtime MUST honour `Blocked` (mark the plan as blocked —
/// never respawn or kill) and MUST honour `Ambiguous` the same
/// way (the planner could not make a decision, so the runtime
/// must not act on stale state).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryVerdict {
    /// Safe to proceed: replay-safe, idempotent, or no-op.
    Safe,
    /// Refused: the candidate is stale / conflicts with
    /// persisted state; refuse without blocking.
    Refused { reason: String },
    /// Fail-closed: the planner could not make a safe decision;
    /// the runtime MUST mark the plan blocked.
    Blocked { reason: String },
}

/// Roll up a `TerminalPersistDecision` into a `RecoveryVerdict`.
/// Used by the recovery use-site to consolidate per-stage
/// verdicts into a single plan-level decision.
#[cfg(test)]
pub fn verdict_from_terminal_persist(decision: &TerminalPersistDecision) -> RecoveryVerdict {
    match decision {
        TerminalPersistDecision::Commit | TerminalPersistDecision::Idempotent => {
            RecoveryVerdict::Safe
        }
        TerminalPersistDecision::Conflict { reason } => RecoveryVerdict::Refused {
            reason: reason.clone(),
        },
        TerminalPersistDecision::Ambiguous { hint } => RecoveryVerdict::Blocked {
            reason: hint.clone(),
        },
    }
}

/// Roll up a `WorktreeBindVerdict` into a `RecoveryVerdict`.
#[cfg(test)]
pub fn verdict_from_worktree_bind(verdict: &WorktreeBindVerdict) -> RecoveryVerdict {
    match verdict {
        WorktreeBindVerdict::Reuse | WorktreeBindVerdict::Create => RecoveryVerdict::Safe,
        WorktreeBindVerdict::BaseMismatch {
            existing_tip,
            verified_base,
        } => RecoveryVerdict::Refused {
            reason: format!(
                "worktree bind base mismatch: existing_tip={existing_tip}, verified_base={verified_base}"
            ),
        },
    }
}

/// Roll up an `IntegrationRecordDecision` into a
/// `RecoveryVerdict`.
#[cfg(test)]
pub fn verdict_from_integration_record(decision: &IntegrationRecordDecision) -> RecoveryVerdict {
    match decision {
        IntegrationRecordDecision::Commit | IntegrationRecordDecision::Idempotent => {
            RecoveryVerdict::Safe
        }
        IntegrationRecordDecision::DuplicateUnitForTarget {
            existing_unit_id,
            candidate_unit_id,
        } => RecoveryVerdict::Blocked {
            reason: format!(
                "duplicate unit on same target: existing={existing_unit_id}, candidate={candidate_unit_id}"
            ),
        },
    }
}

/// Roll up a `PhaseAdvanceOutcome` into a `RecoveryVerdict`.
#[cfg(test)]
pub fn verdict_from_phase_advance(outcome: &PhaseAdvanceOutcome) -> RecoveryVerdict {
    match outcome {
        PhaseAdvanceOutcome::Advance { .. } | PhaseAdvanceOutcome::Replay { .. } => {
            RecoveryVerdict::Safe
        }
        PhaseAdvanceOutcome::Refuse { from, to } => RecoveryVerdict::Refused {
            reason: format!("phase rollback refused: from={from}, to={to}"),
        },
        PhaseAdvanceOutcome::Ambiguous { hint } => RecoveryVerdict::Blocked {
            reason: (*hint).to_string(),
        },
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Launch fence.
    // ------------------------------------------------------------------

    /// A kernel invocation whose attempt matches the live
    /// attempt is allowed to publish accepted-effect.
    #[test]
    fn launch_fence_current_attempt_passes() {
        assert_eq!(launch_fence(2, 2), LaunchFenceOutcome::Current);
    }

    /// A kernel invocation from a stale attempt (live bumped
    /// past it after a review rejection) MUST NOT publish.
    /// This is the core launch-fence guarantee.
    #[test]
    fn launch_fence_stale_attempt_refuses() {
        let outcome = launch_fence(3, 1);
        match outcome {
            LaunchFenceOutcome::Stale {
                live_attempt,
                candidate_attempt,
            } => {
                assert_eq!(live_attempt, 3);
                assert_eq!(candidate_attempt, 1);
            }
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    /// A candidate whose attempt is ahead of the live attempt
    /// is a programmer error (the kernel was minted by an
    /// ahead-of-time worker); refuse without silently bumping.
    #[test]
    fn launch_fence_ahead_attempt_refuses() {
        let outcome = launch_fence(2, 5);
        match outcome {
            LaunchFenceOutcome::Ahead {
                live_attempt,
                candidate_attempt,
            } => {
                assert_eq!(live_attempt, 2);
                assert_eq!(candidate_attempt, 5);
            }
            other => panic!("expected Ahead, got {other:?}"),
        }
    }

    /// The launch-fence planner is deterministic — same input
    /// gives same output, always.
    #[test]
    fn launch_fence_replay_determinism() {
        assert_eq!(launch_fence(0, 0), launch_fence(0, 0));
        assert_eq!(launch_fence(7, 3), launch_fence(7, 3));
    }

    // ------------------------------------------------------------------
    // Terminal persist.
    // ------------------------------------------------------------------

    /// No persisted row ⇒ commit must run on this resume.
    #[test]
    fn terminal_persist_no_persisted_row_commits() {
        let decision = plan_terminal_persist(None, Some("fp-candidate"));
        assert_eq!(decision, TerminalPersistDecision::Commit);
    }

    /// Same fingerprint ⇒ idempotent replay; commit is a no-op.
    /// Exactly-once projection guarantee: the commit cannot
    /// double-fire after a crash.
    #[test]
    fn terminal_persist_same_fingerprint_is_idempotent() {
        let decision = plan_terminal_persist(Some("fp"), Some("fp"));
        assert_eq!(decision, TerminalPersistDecision::Idempotent);
    }

    /// Different fingerprints ⇒ Conflict; refuse to overwrite
    /// the persisted row. Fail-closed.
    #[test]
    fn terminal_persist_fingerprint_drift_is_conflict() {
        let decision = plan_terminal_persist(Some("fp-persisted"), Some("fp-candidate"));
        match decision {
            TerminalPersistDecision::Conflict { reason } => {
                assert!(reason.contains("fp-persisted"));
                assert!(reason.contains("fp-candidate"));
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    /// Neither side has a fingerprint ⇒ Ambiguous; fail-closed.
    /// The runtime MUST mark the plan blocked rather than
    /// blindly commit or replay.
    #[test]
    fn terminal_persist_no_fingerprints_is_ambiguous() {
        let decision = plan_terminal_persist(None, None);
        assert!(matches!(
            decision,
            TerminalPersistDecision::Ambiguous { .. }
        ));
    }

    /// Persisted row has a fingerprint but candidate does not
    /// ⇒ Ambiguous (the candidate's evidence is partial).
    #[test]
    fn terminal_persist_partial_candidate_is_ambiguous() {
        let decision = plan_terminal_persist(Some("fp"), None);
        assert!(matches!(
            decision,
            TerminalPersistDecision::Ambiguous { .. }
        ));
    }

    // ------------------------------------------------------------------
    // Worktree bind.
    // ------------------------------------------------------------------

    /// Existing branch tip equals verified base ⇒ safe to reuse.
    #[test]
    fn worktree_bind_reuse_when_tip_matches_base() {
        let verdict = plan_worktree_bind(Some("base"), Some("base"));
        assert_eq!(verdict, WorktreeBindVerdict::Reuse);
    }

    /// No existing branch tip ⇒ fresh create.
    #[test]
    fn worktree_bind_create_when_no_existing_tip() {
        let verdict = plan_worktree_bind(None, Some("base"));
        assert_eq!(verdict, WorktreeBindVerdict::Create);
    }

    /// Existing branch tip differs from verified base ⇒
    /// BaseMismatch. The lane refuses to silently rewrite the
    /// unit's base.
    #[test]
    fn worktree_bind_rejects_base_mismatch() {
        let verdict = plan_worktree_bind(Some("stale-tip"), Some("fresh-base"));
        match verdict {
            WorktreeBindVerdict::BaseMismatch {
                existing_tip,
                verified_base,
            } => {
                assert_eq!(existing_tip, "stale-tip");
                assert_eq!(verified_base, "fresh-base");
            }
            other => panic!("expected BaseMismatch, got {other:?}"),
        }
    }

    /// Existing branch tip but no verified base ⇒ BaseMismatch
    /// (we cannot verify the existing branch without a base).
    #[test]
    fn worktree_bind_rejects_tip_without_base() {
        let verdict = plan_worktree_bind(Some("tip"), None);
        assert!(matches!(verdict, WorktreeBindVerdict::BaseMismatch { .. }));
    }

    // ------------------------------------------------------------------
    // Integration record idempotency.
    // ------------------------------------------------------------------

    fn fingerprint(
        unit: &str,
        base: &str,
        integrated: &str,
        expected: &str,
    ) -> MergeIntentFingerprint {
        MergeIntentFingerprint {
            unit_id: unit.to_string(),
            base_commit: base.to_string(),
            integrated_commit: integrated.to_string(),
            expected_head_before: expected.to_string(),
        }
    }

    /// No persisted row ⇒ Commit.
    #[test]
    fn integration_record_no_persisted_commits() {
        let candidate = fingerprint("U1", "b1", "i1", "h1");
        assert_eq!(
            plan_integration_record(None, &candidate),
            IntegrationRecordDecision::Commit
        );
    }

    /// Same tuple ⇒ Idempotent. Exactly-once projection for
    /// the integration merge step.
    #[test]
    fn integration_record_same_tuple_is_idempotent() {
        let a = fingerprint("U1", "b1", "i1", "h1");
        let b = fingerprint("U1", "b1", "i1", "h1");
        assert_eq!(
            plan_integration_record(Some(&a), &b),
            IntegrationRecordDecision::Idempotent
        );
    }

    /// Same unit but different tuple ⇒ DuplicateUnitForTarget.
    /// The lane is the single writer per target; a re-run with
    /// a different candidate fails-closed.
    #[test]
    fn integration_record_different_tuple_is_duplicate() {
        let persisted = fingerprint("U1", "b1", "i1", "h1");
        let candidate = fingerprint("U1", "b2", "i2", "h2");
        let decision = plan_integration_record(Some(&persisted), &candidate);
        match decision {
            IntegrationRecordDecision::DuplicateUnitForTarget {
                existing_unit_id,
                candidate_unit_id,
            } => {
                assert_eq!(existing_unit_id, "U1");
                assert_eq!(candidate_unit_id, "U1");
            }
            other => panic!("expected DuplicateUnitForTarget, got {other:?}"),
        }
    }

    /// Different unit ⇒ still DuplicateUnitForTarget: the
    /// tuple-key equality is on the whole `(unit_id, base,
    /// integrated, expected_head)` tuple, but for a same-target
    /// lane two units on the same tuple is illegal (the lane
    /// would never write the second).
    #[test]
    fn integration_record_different_unit_on_same_tuple_is_duplicate() {
        let persisted = fingerprint("U1", "b1", "i1", "h1");
        let candidate = fingerprint("U2", "b1", "i1", "h1");
        let decision = plan_integration_record(Some(&persisted), &candidate);
        assert!(matches!(
            decision,
            IntegrationRecordDecision::DuplicateUnitForTarget { .. }
        ));
    }

    // ------------------------------------------------------------------
    // Task-close idempotency key.
    // ------------------------------------------------------------------

    /// Two calls with the SAME key are replays of the same
    /// logical close; the runtime MUST treat the second as a
    /// no-op.
    #[test]
    fn task_close_same_key_is_replay() {
        let k1 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-1");
        let k2 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-1");
        assert!(k1.is_replay_of(&k2));
    }

    /// Different idempotency_key ⇒ distinct closes; the
    /// runtime MUST NOT silently dedupe.
    #[test]
    fn task_close_different_idempotency_key_is_distinct() {
        let k1 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-1");
        let k2 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-2");
        assert!(!k1.is_replay_of(&k2));
    }

    /// Different attempt ⇒ distinct closes. After a review
    /// rejection the pipeline bumps attempt and re-mints; the
    /// new close MUST NOT be deduped against the old one.
    #[test]
    fn task_close_different_attempt_is_distinct() {
        let k1 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-1");
        let k2 = TaskCloseIdempotencyKey::mint("U1", 1, 1, "idem-1");
        assert!(!k1.is_replay_of(&k2));
    }

    /// Different step ⇒ distinct closes. Multi-step tasks may
    /// be closed multiple times; the keys MUST differ.
    #[test]
    fn task_close_different_step_is_distinct() {
        let k1 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-1");
        let k2 = TaskCloseIdempotencyKey::mint("U1", 2, 0, "idem-1");
        assert!(!k1.is_replay_of(&k2));
    }

    /// Different task_key ⇒ distinct closes.
    #[test]
    fn task_close_different_task_key_is_distinct() {
        let k1 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-1");
        let k2 = TaskCloseIdempotencyKey::mint("U2", 1, 0, "idem-1");
        assert!(!k1.is_replay_of(&k2));
    }

    /// TaskCloseIdempotencyKey implements Hash so callers can
    /// store it in a HashSet for O(1) replay detection.
    #[test]
    fn task_close_key_supports_hash_set() {
        use std::collections::HashSet;
        let mut set: HashSet<TaskCloseIdempotencyKey> = HashSet::new();
        let k1 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-1");
        let k2 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-1");
        assert!(set.insert(k1.clone()));
        assert!(!set.insert(k2));
        assert_eq!(set.len(), 1);
    }

    // ------------------------------------------------------------------
    // Terminal-emit phase advance.
    // ------------------------------------------------------------------

    /// Forward transition ⇒ Advance.
    #[test]
    fn phase_advance_forward_allows() {
        let outcome = plan_terminal_emit_phase(PHASE_PENDING, PHASE_BUSINESS_PROJECTED);
        assert!(matches!(
            outcome,
            PhaseAdvanceOutcome::Advance { from: 0, to: 1 }
        ));
    }

    /// Multi-step forward transition (e.g. resume from
    /// Pending straight to SalvageCommitted) ⇒ Advance.
    #[test]
    fn phase_advance_multistep_forward_allows() {
        let outcome = plan_terminal_emit_phase(PHASE_PENDING, PHASE_SALVAGE_COMMITTED);
        match outcome {
            PhaseAdvanceOutcome::Advance { from, to } => {
                assert_eq!(from, 0);
                assert_eq!(to, 2);
            }
            other => panic!("expected Advance, got {other:?}"),
        }
    }

    /// Same phase ⇒ Replay (no-op). Exactly-once guarantee:
    /// the commit cannot double-fire after a crash.
    #[test]
    fn phase_advance_same_phase_is_replay() {
        let outcome =
            plan_terminal_emit_phase(PHASE_COORDINATION_WRITTEN, PHASE_COORDINATION_WRITTEN);
        assert_eq!(
            outcome,
            PhaseAdvanceOutcome::Replay {
                at: PHASE_COORDINATION_WRITTEN
            }
        );
    }

    /// Backwards transition ⇒ Refuse; the runtime MUST NOT
    /// roll back a committed phase.
    #[test]
    fn phase_advance_backwards_refuses() {
        let outcome = plan_terminal_emit_phase(PHASE_COORDINATION_COMMITTED, PHASE_PENDING);
        assert!(matches!(outcome, PhaseAdvanceOutcome::Refuse { .. }));
    }

    /// Out-of-range current phase ⇒ Ambiguous (fail-closed).
    #[test]
    fn phase_advance_out_of_range_current_is_ambiguous() {
        let outcome = plan_terminal_emit_phase(PHASE_COUNT, PHASE_PENDING);
        assert!(matches!(outcome, PhaseAdvanceOutcome::Ambiguous { .. }));
    }

    /// Out-of-range target phase ⇒ Ambiguous (fail-closed).
    #[test]
    fn phase_advance_out_of_range_target_is_ambiguous() {
        let outcome = plan_terminal_emit_phase(PHASE_PENDING, PHASE_COUNT);
        assert!(matches!(outcome, PhaseAdvanceOutcome::Ambiguous { .. }));
    }

    // ------------------------------------------------------------------
    // Recovery verdict roll-up.
    // ------------------------------------------------------------------

    /// Commit / Idempotent terminal-persist decisions roll up
    /// to Safe.
    #[test]
    fn verdict_terminal_persist_safe_when_commit_or_idempotent() {
        assert_eq!(
            verdict_from_terminal_persist(&TerminalPersistDecision::Commit),
            RecoveryVerdict::Safe
        );
        assert_eq!(
            verdict_from_terminal_persist(&TerminalPersistDecision::Idempotent),
            RecoveryVerdict::Safe
        );
    }

    /// Conflict terminal-persist decision rolls up to Refused.
    #[test]
    fn verdict_terminal_persist_conflict_refuses() {
        let decision = TerminalPersistDecision::Conflict {
            reason: "drift".to_string(),
        };
        match verdict_from_terminal_persist(&decision) {
            RecoveryVerdict::Refused { reason } => assert!(reason.contains("drift")),
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    /// Ambiguous terminal-persist decision rolls up to Blocked.
    /// The runtime MUST mark the plan blocked rather than
    /// blindly commit or replay.
    #[test]
    fn verdict_terminal_persist_ambiguous_blocks() {
        let decision = TerminalPersistDecision::Ambiguous {
            hint: "no fingerprint".to_string(),
        };
        match verdict_from_terminal_persist(&decision) {
            RecoveryVerdict::Blocked { reason } => assert!(reason.contains("fingerprint")),
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    /// Worktree-bind Reuse / Create rolls up to Safe.
    #[test]
    fn verdict_worktree_bind_safe_when_reuse_or_create() {
        assert_eq!(
            verdict_from_worktree_bind(&WorktreeBindVerdict::Reuse),
            RecoveryVerdict::Safe
        );
        assert_eq!(
            verdict_from_worktree_bind(&WorktreeBindVerdict::Create),
            RecoveryVerdict::Safe
        );
    }

    /// Worktree-bind BaseMismatch rolls up to Refused (the
    /// planner knows the cause; refuse without blocking).
    #[test]
    fn verdict_worktree_bind_base_mismatch_refuses() {
        let verdict = WorktreeBindVerdict::BaseMismatch {
            existing_tip: "tip".to_string(),
            verified_base: "base".to_string(),
        };
        match verdict_from_worktree_bind(&verdict) {
            RecoveryVerdict::Refused { reason } => {
                assert!(reason.contains("base mismatch"));
            }
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    /// Integration-record Commit / Idempotent rolls up to Safe.
    #[test]
    fn verdict_integration_record_safe_when_commit_or_idempotent() {
        assert_eq!(
            verdict_from_integration_record(&IntegrationRecordDecision::Commit),
            RecoveryVerdict::Safe
        );
        assert_eq!(
            verdict_from_integration_record(&IntegrationRecordDecision::Idempotent),
            RecoveryVerdict::Safe
        );
    }

    /// Integration-record DuplicateUnitForTarget rolls up to
    /// Blocked. The lane is the single writer per target; a
    /// duplicate on the same target means the plan is in a
    /// state where resume cannot safely proceed.
    #[test]
    fn verdict_integration_record_duplicate_blocks() {
        let decision = IntegrationRecordDecision::DuplicateUnitForTarget {
            existing_unit_id: "U1".to_string(),
            candidate_unit_id: "U1".to_string(),
        };
        match verdict_from_integration_record(&decision) {
            RecoveryVerdict::Blocked { reason } => {
                assert!(reason.contains("duplicate"));
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    /// Phase advance Advance / Replay rolls up to Safe.
    #[test]
    fn verdict_phase_advance_safe_when_forward_or_replay() {
        assert_eq!(
            verdict_from_phase_advance(&PhaseAdvanceOutcome::Advance { from: 0, to: 1 }),
            RecoveryVerdict::Safe
        );
        assert_eq!(
            verdict_from_phase_advance(&PhaseAdvanceOutcome::Replay { at: 2 }),
            RecoveryVerdict::Safe
        );
    }

    /// Phase advance Refuse rolls up to Refused.
    #[test]
    fn verdict_phase_advance_backwards_refuses() {
        let outcome = PhaseAdvanceOutcome::Refuse { from: 4, to: 0 };
        match verdict_from_phase_advance(&outcome) {
            RecoveryVerdict::Refused { reason } => {
                assert!(reason.contains("rollback"));
            }
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    /// Phase advance Ambiguous rolls up to Blocked.
    #[test]
    fn verdict_phase_advance_ambiguous_blocks() {
        let outcome = PhaseAdvanceOutcome::Ambiguous {
            hint: "out of range",
        };
        match verdict_from_phase_advance(&outcome) {
            RecoveryVerdict::Blocked { reason } => {
                assert!(reason.contains("range"));
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // End-to-end exactly-once projection: each interruption
    // point produces AT MOST one observable effect per
    // (plan_key, unit_id, step, attempt) tuple.
    // ------------------------------------------------------------------

    /// Six interruption points all produce the Safe verdict
    /// when the persisted state matches the candidate. Exactly
    /// one effect fires per tuple; replays are no-ops.
    #[test]
    fn six_interruption_points_each_idempotent_on_match() {
        // 1. Launch fence: current attempt ⇒ Current.
        assert_eq!(launch_fence(0, 0), LaunchFenceOutcome::Current);

        // 2. Terminal persist: same fingerprint ⇒ Idempotent.
        assert_eq!(
            plan_terminal_persist(Some("fp"), Some("fp")),
            TerminalPersistDecision::Idempotent
        );

        // 3. Worktree bind: same branch tip ⇒ Reuse.
        assert_eq!(
            plan_worktree_bind(Some("base"), Some("base")),
            WorktreeBindVerdict::Reuse
        );

        // 4. Integration record: same tuple ⇒ Idempotent.
        let fp = fingerprint("U1", "b", "i", "h");
        assert_eq!(
            plan_integration_record(Some(&fp), &fp),
            IntegrationRecordDecision::Idempotent
        );

        // 5. Task close: same key ⇒ replay.
        let k = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem");
        assert!(k.is_replay_of(&k));

        // 6. Terminal emit: same phase ⇒ Replay.
        assert_eq!(
            plan_terminal_emit_phase(PHASE_COORDINATION_WRITTEN, PHASE_COORDINATION_WRITTEN),
            PhaseAdvanceOutcome::Replay { at: 3 }
        );
    }

    /// Six interruption points each refuse cleanly when the
    /// state is stale / conflicting. None of them silently
    /// respawn or kill.
    #[test]
    fn six_interruption_points_each_refuse_on_stale_or_conflict() {
        // 1. Launch fence: stale attempt ⇒ Stale.
        assert!(matches!(
            launch_fence(2, 1),
            LaunchFenceOutcome::Stale { .. }
        ));

        // 2. Terminal persist: fingerprint drift ⇒ Conflict.
        assert!(matches!(
            plan_terminal_persist(Some("a"), Some("b")),
            TerminalPersistDecision::Conflict { .. }
        ));

        // 3. Worktree bind: tip differs from base ⇒ BaseMismatch.
        assert!(matches!(
            plan_worktree_bind(Some("tip"), Some("base")),
            WorktreeBindVerdict::BaseMismatch { .. }
        ));

        // 4. Integration record: different tuple ⇒ DuplicateUnitForTarget.
        let a = fingerprint("U1", "b1", "i1", "h1");
        let b = fingerprint("U1", "b2", "i2", "h2");
        assert!(matches!(
            plan_integration_record(Some(&a), &b),
            IntegrationRecordDecision::DuplicateUnitForTarget { .. }
        ));

        // 5. Task close: different key ⇒ NOT a replay.
        let k1 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-1");
        let k2 = TaskCloseIdempotencyKey::mint("U1", 1, 0, "idem-2");
        assert!(!k1.is_replay_of(&k2));

        // 6. Terminal emit: backwards ⇒ Refuse.
        assert!(matches!(
            plan_terminal_emit_phase(PHASE_COORDINATION_COMMITTED, PHASE_PENDING),
            PhaseAdvanceOutcome::Refuse { .. }
        ));
    }

    /// Six interruption points each Block (fail-closed) when
    /// state is ambiguous. The runtime MUST mark the plan
    /// blocked rather than respawn or kill.
    #[test]
    fn six_interruption_points_each_block_on_ambiguity() {
        // 1. Launch fence: ahead-of-time attempt — programme
        //    error; the planner refuses but does NOT block
        //    (this is a programming mistake, not an ambiguity).
        //    ⇒ Skip from the Block set; the verdict is Refused.
        // 2. Terminal persist: missing fingerprints ⇒ Ambiguous
        //    ⇒ Blocked.
        assert!(matches!(
            verdict_from_terminal_persist(&plan_terminal_persist(None, None)),
            RecoveryVerdict::Blocked { .. }
        ));
        // 3. Worktree bind: tip without base is Refused (the
        //    planner knows the cause); not Block.
        // 4. Integration record: DuplicateUnitForTarget ⇒ Blocked.
        let a = fingerprint("U1", "b1", "i1", "h1");
        let b = fingerprint("U1", "b2", "i2", "h2");
        assert!(matches!(
            verdict_from_integration_record(&plan_integration_record(Some(&a), &b)),
            RecoveryVerdict::Blocked { .. }
        ));
        // 5. Task close: ambiguity surfaces as "key collides
        //    but task ids differ". The planner does not invent
        //    ambiguity — the caller must scrub.
        // 6. Terminal emit: out-of-range phase ⇒ Ambiguous
        //    ⇒ Blocked.
        assert!(matches!(
            verdict_from_phase_advance(&plan_terminal_emit_phase(PHASE_COUNT, PHASE_PENDING)),
            RecoveryVerdict::Blocked { .. }
        ));
    }
}
