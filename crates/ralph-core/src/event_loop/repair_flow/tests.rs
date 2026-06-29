use super::{RepairAction, RepairBudget, RepairState, RepairStateMachine, RepairTransitionResult};

#[test]
fn repair_flow_happy_path_full_progression() {
    let mut sm = RepairStateMachine::new(RepairBudget::default());
    assert_eq!(sm.state(), RepairState::Detected);

    assert_eq!(
        sm.try_transition(RepairAction::BeginDiagnosis),
        RepairTransitionResult::Accepted
    );
    assert_eq!(sm.state(), RepairState::Diagnosing);

    assert_eq!(
        sm.try_transition(RepairAction::BeginFix),
        RepairTransitionResult::Accepted
    );
    assert_eq!(sm.state(), RepairState::Fixing);

    assert_eq!(
        sm.try_transition(RepairAction::BeginVerify),
        RepairTransitionResult::Accepted
    );
    assert_eq!(sm.state(), RepairState::Verifying);

    assert_eq!(
        sm.try_transition(RepairAction::Close),
        RepairTransitionResult::Accepted
    );
    assert_eq!(sm.state(), RepairState::Closed);
}

#[test]
fn repair_flow_default_budget_is_three() {
    let sm = RepairStateMachine::default();
    assert_eq!(sm.budget(), RepairBudget { max: 3 });
    assert_eq!(sm.budget().max, 3);
}

#[test]
fn repair_flow_budget_can_be_overridden() {
    let sm = RepairStateMachine::new(RepairBudget::new(5));
    assert_eq!(sm.budget().max, 5);
}

#[test]
fn repair_flow_budget_exhausted_after_three_retries() {
    let mut sm = RepairStateMachine::new(RepairBudget::default());

    // First attempt: drive into Verifying.
    sm.try_transition(RepairAction::BeginDiagnosis);
    sm.try_transition(RepairAction::BeginFix);
    sm.try_transition(RepairAction::BeginVerify);

    // Three retries should be accepted…
    for _ in 0..3 {
        // Each Retry returns to Diagnosing; re-drive to Verifying.
        assert_eq!(
            sm.try_transition(RepairAction::Retry),
            RepairTransitionResult::Accepted
        );
        sm.try_transition(RepairAction::BeginFix);
        sm.try_transition(RepairAction::BeginVerify);
    }

    assert_eq!(sm.retries_consumed(), 3);

    // Fourth Retry must reject with BudgetExhausted.
    let err = sm.try_transition(RepairAction::Retry);
    let RepairTransitionResult::BudgetExhausted(be) = err else {
        panic!("expected BudgetExhausted, got {err:?}");
    };
    assert_eq!(be.reason_code, "repair_unrecoverable_after_3_retries");
    assert_eq!(be.retries_consumed, 3);
    assert_eq!(be.max, 3);
}

#[test]
fn repair_flow_budget_exhausted_then_repeated_calls_still_reject() {
    let mut sm = RepairStateMachine::new(RepairBudget::new(1));

    sm.try_transition(RepairAction::BeginDiagnosis);
    sm.try_transition(RepairAction::BeginFix);
    sm.try_transition(RepairAction::BeginVerify);

    // Use the one permitted retry.
    sm.try_transition(RepairAction::Retry);
    sm.try_transition(RepairAction::BeginFix);
    sm.try_transition(RepairAction::BeginVerify);

    // Now the next Retry is rejected, and subsequent calls keep
    // returning BudgetExhausted (no panic).
    for _ in 0..3 {
        let result = sm.try_transition(RepairAction::Retry);
        assert!(matches!(result, RepairTransitionResult::BudgetExhausted(_)));
    }
}

#[test]
fn repair_flow_close_resets_internal_retry_counter() {
    let mut sm = RepairStateMachine::new(RepairBudget::new(5));

    // Drive through one full cycle that consumed two retries.
    sm.try_transition(RepairAction::BeginDiagnosis);
    sm.try_transition(RepairAction::BeginFix);
    sm.try_transition(RepairAction::BeginVerify);
    sm.try_transition(RepairAction::Retry);
    sm.try_transition(RepairAction::BeginFix);
    sm.try_transition(RepairAction::BeginVerify);
    sm.try_transition(RepairAction::Retry);
    sm.try_transition(RepairAction::BeginFix);
    sm.try_transition(RepairAction::BeginVerify);
    assert_eq!(sm.retries_consumed(), 2);

    // Close resets.
    sm.try_transition(RepairAction::Close);
    assert_eq!(sm.state(), RepairState::Closed);
    assert_eq!(sm.retries_consumed(), 0);

    // After Close, the machine is sealed.
    let result = sm.try_transition(RepairAction::BeginDiagnosis);
    assert!(matches!(
        result,
        RepairTransitionResult::IllegalTransition { .. }
    ));
}

#[test]
fn repair_flow_illegal_transition_returns_caller_info() {
    let mut sm = RepairStateMachine::new(RepairBudget::default());
    // Cannot BeginFix directly from Detected.
    let result = sm.try_transition(RepairAction::BeginFix);
    let RepairTransitionResult::IllegalTransition { from, action } = result else {
        panic!("expected IllegalTransition, got {result:?}");
    };
    assert_eq!(from, RepairState::Detected);
    assert_eq!(action, RepairAction::BeginFix);
}
