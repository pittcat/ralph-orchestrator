//! `LintResumeHint` — in-memory lint failure feedback (KTD-8).
//!
//! Lint failures do NOT persist `task.resume` (R9). They write
//! `LoopState.pending_lint_resume` so the next `build_prompt`
//! can inject `## LINT RESUME REQUIRED` for the right target hat.
//! Operators who want to bypass can pass `--bypass-lint`; the
//! linter still records an audit event but does not gate.
//!
//! Plan ref: R8–R13, plan 2026-06-20-001.

use serde::Serialize;

/// Failure classification used to pick the right target hat
/// (KTD-4). The linter carries the classification forward to
/// the resume hint so the prompt can route the agent to the
/// correct owner.
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
    /// Handoff artifact missing required sections / `## next` marker.
    /// Routes back to the source hat so the agent can regenerate.
    HandoffArtifact,
}

impl LintFailureClass {
    /// Infer the failure class from a reason string. Mirrors the
    /// reasons produced by `run_gates` / `lint_emit` so callers
    /// can pipe the rejection reason through without an extra
    /// classification layer.
    pub fn from_reason(reason: &str) -> Self {
        let lower = reason.to_ascii_lowercase();
        // Check artifact first — its `## next` token is unique.
        if lower.contains("artifact") || lower.contains("## next") {
            Self::HandoffArtifact
        } else if lower.contains("topic") && (lower.contains("ownership") || lower.contains("deny") || lower.contains("unauthorized")) {
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
            LintFailureClass::HandoffArtifact => Self::SourceHat,
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

    #[test]
    fn artifact_routes_to_source() {
        let hint = LintResumeHint::from_reason("review.passed", "## next marker missing in artifact");
        assert_eq!(hint.class, LintFailureClass::HandoffArtifact);
        assert_eq!(hint.target, LintResumeTarget::SourceHat);
    }
}