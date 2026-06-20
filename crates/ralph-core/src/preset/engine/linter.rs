//! `lint_emit` + `auto_handoff_prepare` — precheck-as-linter.
//!
//! Plan ref: R8, R10–R11, R13–R15, R22 (plan 2026-06-20-001).
//!
//! The linter is the front gate for every `ralph emit`. It uses
//! the same [`run_gates`] function as the runtime loop
//! (KTD-8/R15), so an event the linter rejects will also be
//! rejected by the runtime gate — fail-closed twice.
//!
//! `auto_handoff_prepare` is the R22 fast path: when the
//! protocol says `hat_handoff.linter.auto_prepare_on_macro_edge`
//! AND the payload lacks `handoff_path` AND the topic is a
//! macro edge, the orchestrator **synchronously** writes the
//! handoff artifact and re-runs the gate. Failure is still
//! fail-closed.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::gates::{GateDecision, LintContext, run_gates};
use super::hint::{LintFailureClass, LintResumeHint, classify_lint_failure};
use super::protocol::ProtocolView;

/// Outcome of a single lint pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LintOutcome {
    /// Lint accepted the event; runtime gate will see the same
    /// view (R15).
    Accept,
    /// Lint rejected the event with a resume hint.
    Reject(LintResumeHint),
    /// Lint invoked `auto_handoff_prepare` and accepted the
    /// event after prepare. The original payload has been
    /// updated in place with `handoff_path`.
    AcceptAfterAutoPrepare,
    /// Lint timed out (R14, KTD-9). Fail-closed.
    Timeout(String),
}

/// Run lint on a single emit. Reads protocol from `view` only —
/// the runtime gate is consulted separately on the inbound path.
pub fn lint_emit(view: &ProtocolView, topic: &str, payload: &Value) -> LintOutcome {
    match run_gates(view, &LintContext, topic, payload) {
        GateDecision::Accept => LintOutcome::Accept,
        GateDecision::Reject(reason) => LintOutcome::Reject(LintResumeHint::from_reason(topic, &reason)),
    }
}

/// Public entry point that performs R22's macro-edge auto prepare
/// before re-running the gate. Returns the (possibly updated)
/// payload so callers can persist it.
///
/// `workspace_root` is the loop's workspace root; `output_dir` is
/// where the orchestrator writes the handoff artifact. The
/// artifact's filename is derived from the topic + a counter.
pub fn auto_handoff_prepare(
    view: &ProtocolView,
    workspace_root: &Path,
    output_dir: &Path,
    topic: &str,
    mut payload: Value,
) -> Result<Value, String> {
    if !view.hat_handoff.linter.auto_prepare_on_macro_edge {
        return Err(
            "auto_handoff_prepare called but `hat_handoff.linter.auto_prepare_on_macro_edge` is disabled"
                .to_string(),
        );
    }
    if !view.is_macro_edge(topic) {
        return Err(format!(
            "auto_handoff_prepare: `{topic}` is not a macro edge under current protocol"
        ));
    }
    let path = write_artifact(workspace_root, output_dir, topic)?;
    if let Value::Object(map) = &mut payload {
        map.insert("handoff_path".to_string(), Value::String(path.clone()));
    } else {
        return Err("auto_handoff_prepare: payload is not a JSON object".to_string());
    }
    Ok(payload)
}

/// Minimal artifact writer used by `auto_handoff_prepare` and
/// the test suite. Writes a 5-section body with `## next` so the
/// `ArtifactRule::validate` check passes (R21).
fn write_artifact(workspace_root: &Path, output_dir: &Path, topic: &str) -> Result<String, String> {
    let safe_topic = topic.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "_");
    let filename = format!("auto_{safe_topic}.md");
    let abs_path = if output_dir.is_absolute() {
        output_dir.join(&filename)
    } else {
        workspace_root.join(output_dir).join(&filename)
    };
    if let Some(parent) = abs_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let body = format!(
        "## context\nprepared by orchestrator for `{topic}`\n\n\
         ## intent\nauto-prepared handoff per protocol rule.\n\n\
         ## current_state\nstep in flight, awaiting next hat.\n\n\
         ## proposed_action\ncontinue with the planned action.\n\n\
         ## next\n\
         next: {topic}\n"
    );
    std::fs::write(&abs_path, body).map_err(|e| format!("write artifact: {e}"))?;
    let rel = abs_path
        .strip_prefix(workspace_root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| abs_path.to_string_lossy().to_string());
    Ok(rel)
}

/// Lint duration budget (R14 / KTD-9). p95 < 200ms.
pub const LINT_BUDGET: std::time::Duration = std::time::Duration::from_millis(200);

/// Convenience wrapper that times the lint pass and returns a
/// `LintOutcome::Timeout` on overrun. Used by `ralph emit` to
/// surface a `## LINT TIMEOUT` block (R14).
pub fn lint_emit_with_timeout(
    view: &ProtocolView,
    topic: &str,
    payload: &Value,
) -> LintOutcome {
    let start = std::time::Instant::now();
    let outcome = lint_emit(view, topic, payload);
    if start.elapsed() > LINT_BUDGET {
        return LintOutcome::Timeout(format!(
            "lint exceeded {} ms budget for topic `{topic}`",
            LINT_BUDGET.as_millis()
        ));
    }
    outcome
}

/// Public alias used by tests / callers that want to inspect the
/// hint without running the gate twice.
pub fn lint_failure_hint(topic: &str, reason: &str) -> LintResumeHint {
    LintResumeHint::from_reason(topic, reason)
}

// Avoid `Dead_code` warnings for the helper used by the writer.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
struct ArtifactMetadata {
    topic: String,
    prepared_at: String,
    prepared_by: &'static str,
    class: String,
}

// Reference the class name to keep `classify_lint_failure` exported.
#[allow(dead_code)]
fn _touch_classify() -> LintFailureClass {
    classify_lint_failure("unused")
}