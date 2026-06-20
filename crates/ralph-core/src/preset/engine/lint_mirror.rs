//! `build_lint_mirror_block` + `build_lint_resume_block`
//! (U4b, R12, R13).
//!
//! Renders the two prompt blocks the linter injects into the
//! next iteration's `build_prompt`:
//!
//!   * `## LINT MIRROR` — always on (when linter is enabled and
//!     a hint is in `pending_lint_resume`). Shows the failing
//!     topic, reason, and the protocol hash so the agent sees the
//!     protocol that produced the rule.
//!   * `## LINT RESUME REQUIRED` — only when the hint is fresh
//!     (i.e. the loop has not yet consumed it). Renders the
//!     target hat name and the failing reason.
//!
//! Both blocks are derived from `ProtocolView` + `LintResumeHint`
//! — no hand-written prose, so the agent always sees the
//! canonical contract.

use super::hint::{LintResumeHint, LintResumeTarget};
use super::protocol::ProtocolView;

/// `## LINT MIRROR` block. Returns the rendered markdown. The
/// caller is responsible for skipping the block when there is
/// no `pending_lint_resume` (the engine's `LintResumeHint` is
/// `Option`-wrapped at the loop-state layer).
pub fn build_lint_mirror_block(view: &ProtocolView, hint: &LintResumeHint) -> String {
    format!(
        "## LINT MIRROR\n\
         protocol_hash: {protocol_hash}\n\
         topic: `{topic}`\n\
         class: `{class:?}`\n\
         target: `{target:?}`\n\
         reason: {reason}\n\
         \n\
         Fix the payload so the next `ralph emit` clears the lint.\n",
        protocol_hash = view.protocol_hash,
        topic = hint.topic,
        class = hint.class,
        target = hint.target,
        reason = hint.reason,
    )
}

/// `## LINT RESUME REQUIRED` block. Always paired with the mirror
/// block on the iteration that follows a lint failure.
pub fn build_lint_resume_block(hint: &LintResumeHint) -> String {
    let target_hat = match hint.target {
        LintResumeTarget::SourceHat => "the source hat",
        LintResumeTarget::PlanGate => "plan-gate",
    };
    format!(
        "## LINT RESUME REQUIRED\n\
         route_to: `{target_hat}`\n\
         topic: `{topic}`\n\
         reason: {reason}\n",
        topic = hint.topic,
        reason = hint.reason,
    )
}
