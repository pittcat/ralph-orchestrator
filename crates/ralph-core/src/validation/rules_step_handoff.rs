//! U4c: `StepHandoffRule` — wraps `step_handoff::progress_task_gate`.
//!
//! Pre-commit phase. The rule inspects `queue.advance` and
//! `plan.complete` events against the snapshot's
//! `progress` + `tasks` views (lifted from the legacy disk-read
//! `check_progress_task_alignment` into a pure-function shape
//! that takes `&ProgressSnapshot + &[Task]` directly).
//!
//! The rule preserves the legacy `reason` strings from
//! `ProgressTaskMismatch` so existing tests can match the
//! stable surface.

use crate::event_reader::Event;
use crate::preset::engine::protocol::ProtocolView;
use crate::state::LedgerSnapshot;
use crate::step_handoff::progress_task_gate::{
    check_alignment_with_snapshot, GateDecision, GATED_TOPICS,
};
use crate::task::Task;
use ralph_proto::HatId;

use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ReasonCode, ValidationResult, ValidationStage};

/// `StepHandoffRule` — pre-commit step-handoff gate.
pub struct StepHandoffRule;

impl ValidationRule for StepHandoffRule {
    fn name(&self) -> &'static str {
        ValidationStage::StepHandoff.as_str()
    }

    fn applies_to(&self) -> RulePhase {
        RulePhase::PreCommit
    }

    fn validate(
        &self,
        _protocol_view: &ProtocolView,
        ledger_snapshot: &LedgerSnapshot,
        event: &Event,
    ) -> ValidationResult {
        if !GATED_TOPICS.contains(&event.topic.as_str()) {
            return ValidationResult::accept();
        }
        let (step, task_id) = extract_step_task(event);
        let progress = ledger_snapshot.progress.clone();
        let tasks: Vec<Task> = ledger_snapshot.tasks.clone();
        let decision = check_alignment_with_snapshot(
            &progress,
            &tasks,
            event.topic.as_str(),
            step.as_deref(),
            task_id.as_deref(),
        );
        match decision {
            GateDecision::Inert | GateDecision::Aligned => ValidationResult::accept(),
            GateDecision::Mismatch(m) => {
                let code = format!("{}:{}", ReasonCode::STEP_HANDOFF_MISMATCH_PREFIX, m.reason);
                let hint = m.detail.clone();
                ValidationResult::reject(
                    ValidationStage::StepHandoff,
                    code,
                    Some(hint),
                    true,
                )
            }
        }
    }
}

/// Extract `(step, task_id)` from the event payload. The
/// `progress_task_gate` legacy function accepts `Option<&str>`
/// values; this helper returns them as owned strings so the
/// borrow checker stays happy.
fn extract_step_task(event: &Event) -> (Option<String>, Option<String>) {
    let payload = event.payload.as_deref().unwrap_or("");
    let parsed: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let step = parsed
        .get("step")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let task_id = parsed
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (step, task_id)
}

// Keep the unused HatId import out of warnings.
#[allow(dead_code)]
fn _hat_id_marker(_: HatId) {}