//! GAP-01 knowledge observation wiring kept outside the large parser module.

use super::*;

impl EventLoop {
    /// Commit one bounded, fail-soft knowledge delta after event validation.
    pub(super) fn commit_knowledge_observations(
        &mut self,
        accepted_log_events: &[ralph_proto::Event],
    ) {
        if accepted_log_events.is_empty() {
            return;
        }
        let iteration = self.state.iteration;
        let loop_start_sha = self.state.loop_start_sha.clone();
        let plan_baseline_sha = self.state.plan_baseline_sha.clone();
        if let Some(ledger) = self.state.state_ledger.as_mut() {
            let scope = crate::state::KnowledgeCommitScope::new(
                ledger,
                iteration,
                loop_start_sha,
                plan_baseline_sha,
                crate::event_loop::disposition::classify,
            );
            match scope.commit(accepted_log_events) {
                crate::state::CommitObservationOutcome::Committed { count } => {
                    tracing::debug!(count, iteration, "knowledge observation committed");
                }
                crate::state::CommitObservationOutcome::PersistFailed { count, error } => {
                    tracing::warn!(
                        error = %error,
                        count,
                        iteration,
                        "knowledge commit failed; loop continues"
                    );
                }
                crate::state::CommitObservationOutcome::Empty => {}
            }
        }
    }
}
