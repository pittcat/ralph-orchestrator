//! Independent repair flow state machine + per-task budget (U2).
//!
//! Why this exists: prior to U2, `task.resume` events rode the main
//! `EventBus` and could be retried indefinitely through the
//! `stall_recovery_counts` map. The 2026-06-26 incident produced
//! 28 recovery messages per plan before eventually blocking — i.e.
//! the recovery loop itself became a stuck loop. U2 isolates the
//! repair lifecycle into its own state machine and bounds it with
//! a `RepairBudget`.
//!
//! Cross-platform / concurrency semantics: in-memory only. Not
//! thread-safe — callers serialise access. Persistence (so the
//! counter survives across processes) lives in
//! `state::idempotent_log` (U4) and is wired into the runtime in
//! U8. This module never touches the filesystem directly.
//!
//! # Example
//!
//! ```
//! use ralph_core::event_loop::repair_flow::{
//!     RepairAction, RepairBudget, RepairState, RepairStateMachine,
//! };
//!
//! let mut sm = RepairStateMachine::new(RepairBudget::default());
//! sm.try_transition(RepairAction::BeginDiagnosis).unwrap();
//! sm.try_transition(RepairAction::BeginFix).unwrap();
//! sm.try_transition(RepairAction::BeginVerify).unwrap();
//! sm.try_transition(RepairAction::Close).unwrap();
//! assert_eq!(sm.state(), RepairState::Closed);
//! ```

/// Discrete repair lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairState {
    /// Issue detected, no diagnostic yet.
    Detected,
    /// Diagnostic in progress.
    Diagnosing,
    /// Fix application in progress.
    Fixing,
    /// Verifying that the fix worked.
    Verifying,
    /// Repair closed successfully.
    Closed,
}

/// Action that drives a state transition. See `RepairStateMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairAction {
    BeginDiagnosis,
    BeginFix,
    BeginVerify,
    /// Increment the retry counter and re-enter `Diagnosing`. Used
    /// when a verification fails.
    Retry,
    Close,
}

/// Per-task retry budget. `max` is the inclusive upper bound on
/// the number of `Retry` actions permitted before the budget is
/// exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairBudget {
    pub max: u32,
}

impl Default for RepairBudget {
    /// Default budget is 3 — the value preset `ce-executor-serial`
    /// declares in its `mechanism.repair_budget` field (see
    /// appendix A of the plan).
    fn default() -> Self {
        Self { max: 3 }
    }
}

impl RepairBudget {
    pub fn new(max: u32) -> Self {
        Self { max }
    }
}

/// Raised when the budget is exhausted and a transition is still
/// attempted. The reason code is stable and consumed by the
/// `RepairDispatchStage` (U7) and by telemetry dashboards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetExhausted {
    pub reason_code: String,
    pub retries_consumed: u32,
    pub max: u32,
}

impl BudgetExhausted {
    fn new(consumed: u32, max: u32) -> Self {
        Self {
            reason_code: format!("repair_unrecoverable_after_{consumed}_retries"),
            retries_consumed: consumed,
            max,
        }
    }
}

/// Result of a `try_transition` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairTransitionResult {
    /// Transition accepted; caller may inspect `state()` to see
    /// the new state.
    Accepted,
    /// Transition rejected because the action is not legal from
    /// the current state. This is a programming error — repair
    /// topics are emitted by a finite, vetted set of hats.
    IllegalTransition { from: RepairState, action: RepairAction },
    /// Transition rejected because the retry budget is exhausted.
    BudgetExhausted(BudgetExhausted),
}

/// Repair state machine. Tracks per-task state and the number of
/// retries consumed; budget exhaustion is signalled explicitly so
/// the caller can escalate to `plan.blocked`.
#[derive(Debug, Clone)]
pub struct RepairStateMachine {
    state: RepairState,
    budget: RepairBudget,
    retries: u32,
    closed: bool,
}

impl Default for RepairStateMachine {
    fn default() -> Self {
        Self::new(RepairBudget::default())
    }
}

impl RepairStateMachine {
    pub fn new(budget: RepairBudget) -> Self {
        Self {
            state: RepairState::Detected,
            budget,
            retries: 0,
            closed: false,
        }
    }

    pub fn state(&self) -> RepairState {
        self.state
    }

    pub fn retries_consumed(&self) -> u32 {
        self.retries
    }

    pub fn budget(&self) -> RepairBudget {
        self.budget
    }

    /// Try to apply `action`. Returns:
    /// - `Accepted` on a valid transition;
    /// - `IllegalTransition` if `action` is not allowed from the
    ///   current state;
    /// - `BudgetExhausted` when a `Retry` would exceed the budget.
    ///
    /// After `Close` is accepted, the machine is sealed — any
    /// further action returns `IllegalTransition`. The internal
    /// `retries` counter is reset on a successful `Close` so the
    /// next repair cycle on the same task starts fresh.
    pub fn try_transition(
        &mut self,
        action: RepairAction,
    ) -> RepairTransitionResult {
        if self.closed {
            return RepairTransitionResult::IllegalTransition {
                from: self.state,
                action,
            };
        }

        let from = self.state;
        let next = match (self.state, action) {
            (RepairState::Detected, RepairAction::BeginDiagnosis) => RepairState::Diagnosing,
            (RepairState::Diagnosing, RepairAction::BeginFix) => RepairState::Fixing,
            (RepairState::Fixing, RepairAction::BeginVerify) => RepairState::Verifying,
            (
                RepairState::Verifying | RepairState::Diagnosing | RepairState::Fixing,
                RepairAction::Retry,
            ) => {
                if self.retries >= self.budget.max {
                    return RepairTransitionResult::BudgetExhausted(
                        BudgetExhausted::new(self.retries, self.budget.max),
                    );
                }
                self.retries += 1;
                RepairState::Diagnosing
            }
            (RepairState::Verifying, RepairAction::Close) => {
                self.closed = true;
                self.state = RepairState::Closed;
                self.retries = 0;
                return RepairTransitionResult::Accepted;
            }
            _ => {
                return RepairTransitionResult::IllegalTransition {
                    from,
                    action,
                };
            }
        };

        self.state = next;
        RepairTransitionResult::Accepted
    }
}

#[cfg(test)]
mod tests;