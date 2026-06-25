//! `LintResumeHint` — in-memory lint failure feedback (KTD-8).
//!
//! Lint failures do NOT persist `task.resume` (R9). They write
//! `LoopState.pending_lint_resume` so the next `build_prompt`
//! can inject `## LINT RESUME REQUIRED` for the right target hat.
//! Operators who want to bypass can pass `--bypass-lint`; the
//! linter still records an audit event but does not gate.
//!
//! Plan ref: R8–R13, plan 2026-06-20-001.
//!
//! ## P1-1: typed rejection classification
//!
//! The preferred constructor is
//! [`LintResumeHint::from_typed_rejection`], which takes a
//! [`crate::preset::engine::gates::RejectionKind`] and maps it
//! to the failure class. The legacy
//! [`LintResumeHint::from_reason`] is still available for
//! callers that do not yet have a typed kind (it degrades to
//! the previous string-substring matching); new code MUST use
//! the typed constructor.

use serde::Serialize;

use super::gates::RejectionKind;

/// Failure classification used to pick the right target hat
/// (KTD-4). The linter carries the classification forward to
/// the resume hint so the prompt can route the agent to the
/// correct owner.
///
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LintFailureClass {
    /// Payload-level mistake (missing required field, wrong type).
    /// Routes back to the source hat so the agent can retry.
    PayloadError,
    /// Upstream state mismatch (progress.md / step / state projection).
    /// Routes to `plan-gate` which owns the orchestration state.
    UpstreamStateMissing,
    /// Topic emitted by a hat that does not own it. Routes back
    /// to the source hat to discourage cross-hat publishing.
    TopicOwnership,
}

impl LintFailureClass {
    /// Infer the failure class from a reason string. Legacy
    /// path — new code should construct the hint from a typed
    /// [`RejectionKind`] via
    /// [`LintResumeHint::from_typed_rejection`]. Kept for
    /// callers that only have a reason string (notably the
    /// runtime gate path, which appends to a JSONL log and
    /// then re-reads it).
    pub fn from_reason(reason: &str) -> Self {
        let lower = reason.to_ascii_lowercase();
        if lower.contains("topic")
            && (lower.contains("ownership")
                || lower.contains("deny")
                || lower.contains("unauthorized"))
        {
            Self::TopicOwnership
        } else if lower.contains("progress") && lower.contains("stale") {
            // Only the very specific "progress stale" phrase maps to
            // upstream state. Generic "missing fields" stays as a
            // payload error so the source hat retries.
            Self::UpstreamStateMissing
        } else {
            Self::PayloadError
        }
    }
}

/// Target hat the resume prompt should route to.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LintResumeTarget {
    /// Source hat that emitted the rejected event.
    SourceHat,
    /// Plan-gate hat that owns orchestration state.
    PlanGate,
}

impl LintResumeTarget {
    /// Compute the target hat from the failure class. Mirrors
    /// KTD-4 mapping.
    pub fn from_class(class: &LintFailureClass) -> Self {
        match class {
            LintFailureClass::PayloadError => Self::SourceHat,
            LintFailureClass::UpstreamStateMissing => Self::PlanGate,
            LintFailureClass::TopicOwnership => Self::SourceHat,
        }
    }
}

/// In-memory lint failure hint. Cleared on the next successful
/// `build_prompt` consumption (event_loop/mod.rs owns the
/// lifecycle; the engine only builds the value).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LintResumeHint {
    pub class: LintFailureClass,
    pub target: LintResumeTarget,
    pub topic: String,
    pub reason: String,
}

impl LintResumeHint {
    /// Construct from a typed [`RejectionKind`] (P1-1).
    /// Preferred entry point for callers that already have a
    /// structured rejection — the routing target is decided by
    /// the kind, not by string matching on the message.
    pub fn from_typed_rejection(topic: &str, kind: RejectionKind, message: &str) -> Self {
        let class = kind.to_lint_class();
        let target = LintResumeTarget::from_class(&class);
        Self {
            class,
            target,
            topic: topic.to_string(),
            reason: message.to_string(),
        }
    }

    /// Legacy constructor that infers the class from the reason
    /// string. Use [`Self::from_typed_rejection`] for new code
    /// — string matching is fragile and bypassable by any
    /// reason containing the keyword.
    pub fn from_reason(topic: &str, reason: &str) -> Self {
        let class = LintFailureClass::from_reason(reason);
        let target = LintResumeTarget::from_class(&class);
        Self {
            class,
            target,
            topic: topic.to_string(),
            reason: reason.to_string(),
        }
    }
}

/// Public alias used by `linter.rs` / `lint_mirror.rs` to keep
/// the same import path.
pub fn classify_lint_failure(reason: &str) -> LintFailureClass {
    LintFailureClass::from_reason(reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::engine::gates::RejectionKind;

    #[test]
    fn payload_error_routes_to_source() {
        let hint = LintResumeHint::from_reason("work.done", "missing required fields: step");
        assert_eq!(hint.class, LintFailureClass::PayloadError);
        assert_eq!(hint.target, LintResumeTarget::SourceHat);
    }

    #[test]
    fn upstream_routes_to_plan_gate() {
        let hint = LintResumeHint::from_reason("queue.advance", "upstream progress.md stale");
        assert_eq!(hint.class, LintFailureClass::UpstreamStateMissing);
        assert_eq!(hint.target, LintResumeTarget::PlanGate);
    }

    /// P1-1: the typed constructor picks the right class
    /// *regardless* of the message text. A missing-field
    /// rejection whose message accidentally contains the
    /// word "artifact" is still classified as a payload error.
    /// This is the regression that the old `from_reason` could
    /// not catch.
    #[test]
    fn p1_1_typed_rejection_ignores_message_keywords() {
        let hint = LintResumeHint::from_typed_rejection(
            "work.done",
            RejectionKind::MissingField,
            "missing required fields: plan_name, commit_sha (an artifact of the run)",
        );
        assert_eq!(
            hint.class,
            LintFailureClass::PayloadError,
            "missing-field rejection must stay a payload error even when the message mentions 'artifact'"
        );
        assert_eq!(hint.target, LintResumeTarget::SourceHat);
    }

    /// P1-1: TopicOwnership routes to source-hat even when the
    /// message does NOT contain the words "topic" / "ownership".
    #[test]
    fn p1_1_topic_ownership_routes_to_source_hat_unconditionally() {
        let hint = LintResumeHint::from_typed_rejection(
            "review.complete",
            RejectionKind::TopicOwnership,
            "hat executor is not authorized to publish review.complete",
        );
        assert_eq!(hint.class, LintFailureClass::TopicOwnership);
        assert_eq!(hint.target, LintResumeTarget::SourceHat);
    }

    /// P1-1: PreCheck routes to source-hat (same as MissingField).
    #[test]
    fn p1_1_pre_check_routes_to_source_hat() {
        let hint = LintResumeHint::from_typed_rejection(
            "work.done",
            RejectionKind::PreCheck,
            "runtime TTL exceeded for this event",
        );
        assert_eq!(hint.class, LintFailureClass::PayloadError);
        assert_eq!(hint.target, LintResumeTarget::SourceHat);
    }
}
