//! 2026-09-03-0959 plan U8 (S11; D11, D12; E2, E9, E11):
//! bounded correction state machine.
//!
//! Contract (plan §7 U8 #6):
//! - **Max 3 correction rounds** per `(unit_key, stage)` pair.
//!   A "round" is one full dispatch of the *resume stage*
//!   following a review/test rejection.
//! - **CAS-pinned transitions**: `advance_correction` requires
//!   the caller to assert the expected `(unit_key, round)`
//!   pair. A stale or stolen pair yields `CorrectionError::Stale`.
//! - **Single typed `Blocked`** on exhaust: once `round ==
//!   MAX_CORRECTION_ROUNDS` and the next rejection arrives, the
//!   state machine emits `CorrectionDecision::ExhaustedSingleBlocked`
//!   — no further recovery, no looping. The runtime is then
//!   obligated to surface this as a single
//!   `RuntimeJobError::Blocked`.
//! - **Fix resumes from the failing stage**: the correction's
//!   origin carries `Stage` (the stage whose review rejected).
//!   The state machine stamps `resume_stage` onto the next
//!   `DispatchRound` so the runner knows exactly where to
//!   restart the unit — `Execute` after an `Execute`-stage
//!   rejection, `Review` after a `Review`-stage rejection.
//!
//! The module is pure CPU: no `thread::sleep`, no real I/O.
//! Tests use the typed `Stage` enum re-exported from
//! `runtime_job` via the supervisor root, but to keep this
//! module self-contained (and to avoid the `ralph-cli` cfg(test)
//! gate) we accept a tiny `CorrectionStage` enum that mirrors
//! the runtime kernel's `Stage` values 1:1. The conversion is
//! the caller's responsibility — keeps this module a leaf.

#![allow(dead_code)] // U8 wires these into U9+; surface kept stable now.

use std::collections::HashMap;
use std::fmt;

/// Maximum number of correction rounds per unit. Plan §7 U8 #6
/// pins this at 3 — "3 rounds correction + fail-confidence-
/// rubric 90/75 confidence gate to allow `work.failed`".
pub const MAX_CORRECTION_ROUNDS: u64 = 3;

/// Stages a correction can resume from. Mirrors
/// `runtime_job::Stage` 1:1; the conversion is the caller's
/// responsibility so this module stays a leaf (no `ralph-cli`
/// dep).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CorrectionStage {
    /// Resume at the Execute stage — the original execution
    /// re-runs with the fix applied.
    Execute,
    /// Resume at the Review stage — only the review re-runs
    /// against the previous execution's already-applied change.
    Review,
    /// Resume at the Verify stage — only the verification re-runs.
    Verify,
}

impl fmt::Display for CorrectionStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CorrectionStage::Execute => f.write_str("execute"),
            CorrectionStage::Review => f.write_str("review"),
            CorrectionStage::Verify => f.write_str("verify"),
        }
    }
}

/// Per-unit mutable correction state. The runtime holds one of
/// these per active unit (keyed by `unit_key`).
///
/// `round` is "rounds *completed*". A freshly-started unit
/// has `round == 0`. After the first review rejection,
/// `round == 1` and the next dispatch is `Execute` (or
/// whatever the rejection's failing stage says).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionState {
    pub unit_key: String,
    pub round: u64,
    /// Stage the next dispatch must restart from. Set by the
    /// origin of the rejection; immutable until the next
    /// rejection overwrites it.
    pub resume_stage: CorrectionStage,
    /// Optional human-readable reason the rejection was emitted
    /// (kept short — used only for the `Blocked` payload on
    /// exhaust).
    pub last_reason: Option<String>,
}

impl CorrectionState {
    /// Fresh state at `round = 0`, `resume_stage = Execute`.
    pub fn fresh(unit_key: impl Into<String>) -> Self {
        Self {
            unit_key: unit_key.into(),
            round: 0,
            resume_stage: CorrectionStage::Execute,
            last_reason: None,
        }
    }
}

/// Decisions the correction state machine emits. The runner
/// branches on these — `DispatchRound` means "launch the next
/// attempt at `stage`"; `ExhaustedSingleBlocked` means "stop
/// looping, surface a single `Blocked` typed error with the
/// carried `reason`".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorrectionDecision {
    DispatchRound {
        unit_key: String,
        round: u64,
        stage: CorrectionStage,
    },
    ExhaustedSingleBlocked {
        unit_key: String,
        rounds_used: u64,
        reason: String,
    },
}

/// Failure modes from `advance_correction`. The runner surfaces
/// these as `RuntimeJobError` (the typed error set is the
/// contract — see `runtime_job::RuntimeJobError::Blocked`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorrectionError {
    /// CAS mismatch: the caller's expected `(unit_key, round)`
    /// pair does not match the state held by the machine. Either
    /// a stale caller or a stolen unit_key — both fail-closed.
    Stale {
        expected_unit: String,
        expected_round: u64,
        actual_unit: String,
        actual_round: u64,
    },
}

/// Bounded correction state machine. One per supervisor; keyed
/// by `unit_key`. The state lives in `states: HashMap`. Tests
/// drive it directly; production wiring is U9+.
///
/// `states` is public so the inspect view (U5) can read the
/// correction snapshot without going through a getter. Tests
/// pin its initial size + invariants.
#[derive(Debug, Default, Clone)]
pub struct CorrectionMachine {
    pub states: HashMap<String, CorrectionState>,
}

impl CorrectionMachine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Idempotent insert. If `unit_key` is already present,
    /// returns the existing state. New units are initialised at
    /// `round = 0`.
    pub fn ensure(&mut self, unit_key: impl Into<String>) -> CorrectionState {
        let key = unit_key.into();
        self.states
            .entry(key.clone())
            .or_insert_with(|| CorrectionState::fresh(key.clone()))
            .clone()
    }

    /// Pure decision: given a rejection at `stage` with `reason`,
    /// would the next correction be dispatchable or exhausted?
    /// Does NOT mutate state. Tests use this to drive boundary
    /// conditions without committing to an advance.
    pub fn peek_decision(&self, unit_key: &str, stage: CorrectionStage, reason: &str) -> CorrectionDecision {
        let state = match self.states.get(unit_key) {
            Some(s) => s,
            None => {
                // No prior state → first correction at round 1.
                return CorrectionDecision::DispatchRound {
                    unit_key: unit_key.to_string(),
                    round: 1,
                    stage,
                };
            }
        };
        if state.round >= MAX_CORRECTION_ROUNDS {
            return CorrectionDecision::ExhaustedSingleBlocked {
                unit_key: state.unit_key.clone(),
                rounds_used: state.round,
                reason: reason.to_string(),
            };
        }
        CorrectionDecision::DispatchRound {
            unit_key: state.unit_key.clone(),
            round: state.round + 1,
            stage,
        }
    }

    /// Compare-and-swap advance. The caller MUST pass the
    /// `(expected_unit, expected_round)` it observed just
    /// before issuing the rejection — usually the round it
    /// is *closing* (the state machine will then move to
    /// `expected_round + 1`).
    ///
    /// On `Ok(decision)`:
    /// - `DispatchRound { round: n+1, stage, ... }` — the
    ///   caller's next step is to mint a fresh `JobToken` at
    ///   `round = n+1` and launch the resume stage.
    /// - `ExhaustedSingleBlocked { rounds_used: MAX, reason, ... }`
    ///   — the caller must surface a single typed `Blocked`
    ///   with the carried reason. **No** further dispatch.
    ///
    /// On `Err(CorrectionError::Stale)`:
    /// - The caller's expected `(unit, round)` did not match
    ///   the machine's state. The runner treats this as a
    ///   hard failure — no state change, no second attempt.
    pub fn advance_correction(
        &mut self,
        expected_unit: &str,
        expected_round: u64,
        stage: CorrectionStage,
        reason: &str,
    ) -> Result<CorrectionDecision, CorrectionError> {
        // (1) Locate or initialise state. Initialisation is
        // only legal if `expected_round == 0` — anything else
        // is stale.
        let actual_round = self.states.get(expected_unit).map(|s| s.round);
        match actual_round {
            None => {
                if expected_round != 0 {
                    return Err(CorrectionError::Stale {
                        expected_unit: expected_unit.to_string(),
                        expected_round,
                        actual_unit: expected_unit.to_string(),
                        actual_round: 0,
                    });
                }
                // Initialise at round 0; the caller's rejection
                // is "the first one" so the next round = 1.
            }
            Some(ar) if ar != expected_round => {
                return Err(CorrectionError::Stale {
                    expected_unit: expected_unit.to_string(),
                    expected_round,
                    actual_unit: expected_unit.to_string(),
                    actual_round: ar,
                });
            }
            Some(_) => {}
        }

        // (2) Compute next round.
        let next_round = expected_round.saturating_add(1);
        if next_round > MAX_CORRECTION_ROUNDS {
            // Already at the cap; surface exhausted blocked
            // WITHOUT mutating state (the cap is sticky).
            let state = self
                .states
                .get(expected_unit)
                .cloned()
                .unwrap_or_else(|| CorrectionState::fresh(expected_unit));
            return Ok(CorrectionDecision::ExhaustedSingleBlocked {
                unit_key: state.unit_key,
                rounds_used: state.round,
                reason: reason.to_string(),
            });
        }

        // (3) Commit: write the next round + resume stage.
        let entry = self
            .states
            .entry(expected_unit.to_string())
            .or_insert_with(|| CorrectionState::fresh(expected_unit));
        entry.round = next_round;
        entry.resume_stage = stage;
        entry.last_reason = Some(reason.to_string());
        Ok(CorrectionDecision::DispatchRound {
            unit_key: entry.unit_key.clone(),
            round: next_round,
            stage,
        })
    }

    /// Read-only snapshot for inspect / tests.
    pub fn snapshot(&self, unit_key: &str) -> Option<&CorrectionState> {
        self.states.get(unit_key)
    }

    /// Number of tracked units.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> CorrectionMachine {
        CorrectionMachine::new()
    }

    // ----- freshness + CAS ----------------------------------------------

    #[test]
    fn fresh_state_starts_at_round_zero() {
        let s = CorrectionState::fresh("U8-A");
        assert_eq!(s.unit_key, "U8-A");
        assert_eq!(s.round, 0);
        assert_eq!(s.resume_stage, CorrectionStage::Execute);
        assert!(s.last_reason.is_none());
    }

    #[test]
    fn advance_first_rejection_dispatches_round_1() {
        let mut m = machine();
        let d = m
            .advance_correction("U8-A", 0, CorrectionStage::Execute, "tests failing")
            .expect("ok");
        match d {
            CorrectionDecision::DispatchRound { unit_key, round, stage } => {
                assert_eq!(unit_key, "U8-A");
                assert_eq!(round, 1);
                assert_eq!(stage, CorrectionStage::Execute);
            }
            other => panic!("expected DispatchRound, got {other:?}"),
        }
        assert_eq!(m.snapshot("U8-A").unwrap().round, 1);
    }

    // ----- CAS guard -----------------------------------------------------

    #[test]
    fn advance_stale_expected_round_returns_stale_error() {
        let mut m = machine();
        m.advance_correction("U8-A", 0, CorrectionStage::Execute, "r1")
            .expect("ok");
        // Caller observes round=1 and issues a second rejection
        // with expected_round=2 (i.e. assumes the previous
        // round closed cleanly). But state is still round=1 →
        // CAS mismatch.
        let err = m
            .advance_correction("U8-A", 2, CorrectionStage::Execute, "stale")
            .expect_err("stale");
        match err {
            CorrectionError::Stale {
                expected_unit,
                expected_round,
                actual_unit,
                actual_round,
            } => {
                assert_eq!(expected_unit, "U8-A");
                assert_eq!(expected_round, 2);
                assert_eq!(actual_unit, "U8-A");
                assert_eq!(actual_round, 1);
            }
        }
        // State is unchanged: still at round=1.
        assert_eq!(m.snapshot("U8-A").unwrap().round, 1);
    }

    #[test]
    fn advance_stale_expected_unit_does_not_cross_contaminate() {
        let mut m = machine();
        m.advance_correction("U8-A", 0, CorrectionStage::Execute, "r1")
            .expect("ok");
        // Different unit_key with expected_round=1 should not
        // match U8-A's state.
        let err = m
            .advance_correction("U8-B", 1, CorrectionStage::Execute, "stale")
            .expect_err("stale");
        assert!(matches!(err, CorrectionError::Stale { .. }));
    }

    #[test]
    fn advance_correct_expected_round_proceeds() {
        let mut m = machine();
        m.advance_correction("U8-A", 0, CorrectionStage::Execute, "r1")
            .expect("ok");
        // Caller observes round=1, issues second rejection with
        // expected_round=1 → CAS passes, round becomes 2.
        let d = m
            .advance_correction("U8-A", 1, CorrectionStage::Review, "review found issue")
            .expect("ok");
        match d {
            CorrectionDecision::DispatchRound { round, stage, .. } => {
                assert_eq!(round, 2);
                assert_eq!(stage, CorrectionStage::Review);
            }
            other => panic!("expected DispatchRound, got {other:?}"),
        }
        assert_eq!(m.snapshot("U8-A").unwrap().round, 2);
        assert_eq!(
            m.snapshot("U8-A").unwrap().resume_stage,
            CorrectionStage::Review,
            "resume_stage MUST follow the rejection's origin"
        );
    }

    // ----- Fix resumes from failing stage --------------------------------

    #[test]
    fn resume_stage_follows_rejection_origin() {
        let mut m = machine();
        // First rejection at Execute.
        m.advance_correction("U8-A", 0, CorrectionStage::Execute, "tests failing")
            .expect("ok");
        assert_eq!(
            m.snapshot("U8-A").unwrap().resume_stage,
            CorrectionStage::Execute
        );
        // Second rejection at Review — next resume must be at
        // Review, not Execute.
        m.advance_correction("U8-A", 1, CorrectionStage::Review, "lint failed")
            .expect("ok");
        assert_eq!(
            m.snapshot("U8-A").unwrap().resume_stage,
            CorrectionStage::Review
        );
    }

    // ----- Bounded rounds: 3, then exhausted -----------------------------

    #[test]
    fn exhaust_after_three_rounds_emits_single_blocked() {
        let mut m = machine();
        // Round 1: caller expects 0, dispatches 1.
        m.advance_correction("U8-A", 0, CorrectionStage::Execute, "r1")
            .expect("ok");
        // Round 2: caller expects 1, dispatches 2.
        m.advance_correction("U8-A", 1, CorrectionStage::Execute, "r2")
            .expect("ok");
        // Round 3: caller expects 2, dispatches 3 (the last legal round).
        let d = m
            .advance_correction("U8-A", 2, CorrectionStage::Execute, "r3")
            .expect("ok");
        match d {
            CorrectionDecision::DispatchRound { round, .. } => {
                assert_eq!(round, MAX_CORRECTION_ROUNDS);
            }
            other => panic!("expected DispatchRound for round 3, got {other:?}"),
        }
        // Fourth rejection with expected_round=3 → exhausted.
        let d = m
            .advance_correction("U8-A", 3, CorrectionStage::Execute, "still failing")
            .expect("must be OK (decision), not error");
        match d {
            CorrectionDecision::ExhaustedSingleBlocked {
                unit_key,
                rounds_used,
                reason,
            } => {
                assert_eq!(unit_key, "U8-A");
                assert_eq!(rounds_used, MAX_CORRECTION_ROUNDS);
                assert_eq!(reason, "still failing");
            }
            other => panic!("expected ExhaustedSingleBlocked, got {other:?}"),
        }
        // State must remain pinned at the cap (NOT advance to 4).
        assert_eq!(
            m.snapshot("U8-A").unwrap().round,
            MAX_CORRECTION_ROUNDS,
            "exhausted state must NOT advance past MAX_CORRECTION_ROUNDS"
        );
    }

    #[test]
    fn exhaust_does_not_emit_second_blocked() {
        let mut m = machine();
        // Burn the 3 rounds.
        m.advance_correction("U8-A", 0, CorrectionStage::Execute, "r1")
            .expect("ok");
        m.advance_correction("U8-A", 1, CorrectionStage::Execute, "r2")
            .expect("ok");
        m.advance_correction("U8-A", 2, CorrectionStage::Execute, "r3")
            .expect("ok");
        // First exhausted blocked.
        m.advance_correction("U8-A", 3, CorrectionStage::Execute, "still failing")
            .expect("ok");
        // A second rejection after exhaust with the same
        // expected_round=3 — state is unchanged at round=3 so
        // CAS still matches, but the next_round would be 4 > 3
        // → still ExhaustedSingleBlocked, NOT a fresh
        // DispatchRound.
        let d = m
            .advance_correction("U8-A", 3, CorrectionStage::Execute, "again")
            .expect("ok");
        match d {
            CorrectionDecision::ExhaustedSingleBlocked { rounds_used, .. } => {
                assert_eq!(
                    rounds_used, MAX_CORRECTION_ROUNDS,
                    "exhaust must be sticky at MAX_CORRECTION_ROUNDS"
                );
            }
            other => panic!("expected ExhaustedSingleBlocked (sticky), got {other:?}"),
        }
        // State remains at MAX_CORRECTION_ROUNDS.
        assert_eq!(m.snapshot("U8-A").unwrap().round, MAX_CORRECTION_ROUNDS);
    }

    // ----- Multiple units ------------------------------------------------

    #[test]
    fn multiple_units_are_independent() {
        let mut m = machine();
        m.advance_correction("U8-A", 0, CorrectionStage::Execute, "A1")
            .expect("ok");
        m.advance_correction("U8-A", 1, CorrectionStage::Execute, "A2")
            .expect("ok");
        m.advance_correction("U8-B", 0, CorrectionStage::Review, "B1")
            .expect("ok");
        assert_eq!(m.snapshot("U8-A").unwrap().round, 2);
        assert_eq!(m.snapshot("U8-B").unwrap().round, 1);
        assert_eq!(m.snapshot("U8-C"), None);
        assert_eq!(m.len(), 2);
        assert!(!m.is_empty());
    }

    // ----- peek_decision -------------------------------------------------

    #[test]
    fn peek_does_not_mutate_state() {
        let mut m = machine();
        m.advance_correction("U8-A", 0, CorrectionStage::Execute, "r1")
            .expect("ok");
        let before = m.snapshot("U8-A").unwrap().clone();
        let _ = m.peek_decision("U8-A", CorrectionStage::Execute, "preview");
        let after = m.snapshot("U8-A").unwrap().clone();
        assert_eq!(before, after, "peek_decision MUST NOT mutate state");
    }

    #[test]
    fn peek_first_rejection_returns_round_1() {
        let m = machine();
        let d = m.peek_decision("U8-A", CorrectionStage::Execute, "first");
        match d {
            CorrectionDecision::DispatchRound { round, .. } => {
                assert_eq!(round, 1);
            }
            other => panic!("expected DispatchRound round 1, got {other:?}"),
        }
    }

    #[test]
    fn peek_after_exhaust_returns_exhausted() {
        let mut m = machine();
        m.advance_correction("U8-A", 0, CorrectionStage::Execute, "r1")
            .expect("ok");
        m.advance_correction("U8-A", 1, CorrectionStage::Execute, "r2")
            .expect("ok");
        m.advance_correction("U8-A", 2, CorrectionStage::Execute, "r3")
            .expect("ok");
        m.advance_correction("U8-A", 3, CorrectionStage::Execute, "still failing")
            .expect("ok");
        let d = m.peek_decision("U8-A", CorrectionStage::Execute, "preview");
        assert!(matches!(d, CorrectionDecision::ExhaustedSingleBlocked { .. }));
    }

    // ----- ensure --------------------------------------------------------

    #[test]
    fn ensure_is_idempotent() {
        let mut m = machine();
        let a = m.ensure("U8-A");
        assert_eq!(a.round, 0);
        m.advance_correction("U8-A", 0, CorrectionStage::Execute, "r1")
            .expect("ok");
        // ensure on existing unit returns the existing state
        // (round 1), NOT a fresh round 0.
        let a2 = m.ensure("U8-A");
        assert_eq!(a2.round, 1);
    }

    // ----- Display -------------------------------------------------------

    #[test]
    fn correction_stage_display_strings_are_stable() {
        assert_eq!(CorrectionStage::Execute.to_string(), "execute");
        assert_eq!(CorrectionStage::Review.to_string(), "review");
        assert_eq!(CorrectionStage::Verify.to_string(), "verify");
    }
}