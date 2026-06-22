//! Mutable validation context (U11).
//!
//! [`ValidationContext`] wraps the [`crate::state::LedgerSnapshot`] and
//! gives rules mutable access to the runtime state they need to
//! update while validating an event.  Rules that only need read-only
//! access can use [`ValidationContext::snapshot`]; rules that need to
//! mutate per-event policy state (e.g. event-policy dedup keys) can
//! use [`ValidationContext::snapshot_mut`] or the typed helpers
//! [`ValidationContext::policy_runtime_state`] and
//! [`ValidationContext::review_step_tracker`].

use std::collections::BTreeMap;

use crate::event_loop::review_step_state::ReviewStepTracker;
use crate::event_policy::{PolicyRejection, PolicyRuntimeState};
use crate::payload_contract::PayloadContractViolation;
use crate::state::LedgerSnapshot;

/// Mutable context passed to every [`crate::validation::ValidationRule`].
pub struct ValidationContext<'a> {
    snapshot: &'a mut LedgerSnapshot,
    /// Optional override for the policy runtime state. When set, it
    /// shadows the snapshot's `policy_runtime` field so callers with
    /// a separate `PolicyRuntimeState` instance (e.g. the event loop)
    /// do not have to copy state into the snapshot before validating.
    policy_runtime_state: Option<&'a mut PolicyRuntimeState>,
    /// Optional override for the review-step tracker. When set, it
    /// shadows the snapshot's `review_step_tracker` field.
    review_step_tracker: Option<&'a mut ReviewStepTracker>,
    /// Optional accumulator for the first non-recoverable payload
    /// contract violation detected by the event-policy rule. The
    /// event loop uses this to decide whether to pause the loop.
    payload_contract_violation: Option<&'a mut Option<PayloadContractViolation>>,
    /// Optional accumulator for policy rejections. The wave partition
    /// uses this to surface wave-specific rejections to the runner.
    policy_rejections: Option<&'a mut Vec<PolicyRejection>>,
    /// Optional source-hat attribution per topic, used when building
    /// a structured `PayloadContractViolation`.
    source_hats_by_topic: Option<&'a BTreeMap<String, Vec<String>>>,
    /// Optional target-hat attribution per topic, used when building
    /// a structured `PayloadContractViolation`.
    target_hats_by_topic: Option<&'a BTreeMap<String, Vec<String>>>,
}

impl<'a> ValidationContext<'a> {
    /// Build a context around a mutable snapshot reference.
    pub fn new(snapshot: &'a mut LedgerSnapshot) -> Self {
        Self {
            snapshot,
            policy_runtime_state: None,
            review_step_tracker: None,
            payload_contract_violation: None,
            policy_rejections: None,
            source_hats_by_topic: None,
            target_hats_by_topic: None,
        }
    }

    /// Use a caller-supplied `PolicyRuntimeState` instead of the
    /// snapshot's embedded one.
    pub fn with_policy_runtime_state(mut self, state: &'a mut PolicyRuntimeState) -> Self {
        self.policy_runtime_state = Some(state);
        self
    }

    /// Use a caller-supplied `ReviewStepTracker` instead of the
    /// snapshot's embedded one.
    pub fn with_review_step_tracker(mut self, tracker: &'a mut ReviewStepTracker) -> Self {
        self.review_step_tracker = Some(tracker);
        self
    }

    /// Accumulate the first non-recoverable payload contract violation
    /// into a caller-supplied slot.
    pub fn with_payload_contract_violation(
        mut self,
        slot: &'a mut Option<PayloadContractViolation>,
    ) -> Self {
        self.payload_contract_violation = Some(slot);
        self
    }

    /// Accumulate policy rejections into a caller-supplied vector.
    pub fn with_policy_rejections(mut self, vec: &'a mut Vec<PolicyRejection>) -> Self {
        self.policy_rejections = Some(vec);
        self
    }

    /// Provide source-hat attribution for `PayloadContractViolation`
    /// construction.
    pub fn with_source_hats_by_topic(mut self, map: &'a BTreeMap<String, Vec<String>>) -> Self {
        self.source_hats_by_topic = Some(map);
        self
    }

    /// Provide target-hat attribution for `PayloadContractViolation`
    /// construction.
    pub fn with_target_hats_by_topic(mut self, map: &'a BTreeMap<String, Vec<String>>) -> Self {
        self.target_hats_by_topic = Some(map);
        self
    }

    /// Borrow the snapshot immutably.
    pub fn snapshot(&self) -> &LedgerSnapshot {
        self.snapshot
    }

    /// Borrow the snapshot mutably.
    pub fn snapshot_mut(&mut self) -> &mut LedgerSnapshot {
        self.snapshot
    }

    /// Mutable access to the `PolicyRuntimeState` that this validation
    /// should mutate. Falls back to the snapshot's field, lazily
    /// inserting a default when absent.
    pub fn policy_runtime_state(&mut self) -> &mut PolicyRuntimeState {
        if let Some(ref mut state) = self.policy_runtime_state {
            return &mut **state;
        }
        self.snapshot
            .policy_runtime
            .get_or_insert_with(PolicyRuntimeState::default)
    }

    /// Mutable access to the `ReviewStepTracker` that this validation
    /// should mutate. Falls back to the snapshot's field.
    pub fn review_step_tracker(&mut self) -> &mut ReviewStepTracker {
        if let Some(ref mut tracker) = self.review_step_tracker {
            return &mut **tracker;
        }
        &mut self.snapshot.review_step_tracker
    }

    /// Record the first non-recoverable payload contract violation.
    /// Later violations are ignored so the loop surfaces only the
    /// first one.
    pub fn record_payload_contract_violation(&mut self, violation: PayloadContractViolation) {
        if let Some(slot) = self.payload_contract_violation.as_deref_mut() {
            if slot.is_none() {
                *slot = Some(violation);
            }
        }
    }

    /// Record a policy rejection for downstream attribution (e.g. wave
    /// partition diagnostics).
    pub fn record_policy_rejection(&mut self, rejection: PolicyRejection) {
        if let Some(vec) = self.policy_rejections.as_deref_mut() {
            vec.push(rejection);
        }
    }

    /// Source hats for the topic, when attribution maps were supplied.
    pub fn source_hats_for(&self, topic: &str) -> Vec<String> {
        self.source_hats_by_topic
            .and_then(|m| m.get(topic).cloned())
            .unwrap_or_default()
    }

    /// Target hats for the topic, when attribution maps were supplied.
    pub fn target_hats_for(&self, topic: &str) -> Vec<String> {
        self.target_hats_by_topic
            .and_then(|m| m.get(topic).cloned())
            .unwrap_or_default()
    }
}
