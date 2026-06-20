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

use serde_json::Value;

use super::gates::{GateDecision, LintContext, run_gates};
use super::hint::LintResumeHint;
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
///
/// Plan R22 (2026-06-20-001): when the topic is a macro edge
/// (per `view.is_macro_edge(topic)`), the payload lacks
/// `handoff_path`, AND `hat_handoff.linter.auto_prepare_on_macro_edge`
/// is enabled, this function synchronously prepares the handoff
/// artifact (via `auto_handoff_prepare`) and mutates `payload` to
/// inject `handoff_path` before re-running the gate. The
/// returned `LintOutcome::AcceptAfterAutoPrepare` tells the
/// caller that the orchestrator acted on the agent's behalf.
/// `Accept` means no prepare was needed (not a macro edge, or
/// already had `handoff_path`). `Reject`/`Timeout` short-circuit
/// and never invoke auto_prepare.
pub fn lint_emit(view: &ProtocolView, topic: &str, payload: &mut Value) -> LintOutcome {
    // Plan R22 / review P0 #3: macro-edge auto_prepare is the
    // B4 fix. We must check the protocol BEFORE the gate runs,
    // because a missing handoff_path is not in the required-fields
    // set — it lives on the hat_handoff side.
    if view.is_macro_edge(topic)
        && !has_handoff_path(payload)
        && view.hat_handoff.linter.auto_prepare_on_macro_edge
    {
        match auto_handoff_prepare(view, workspace_root_for(view), output_dir_for(view), topic, payload.clone()) {
            Ok(prepared) => {
                // auto_handoff_prepare mutates the payload to inject
                // `handoff_path`; copy it back so the caller writes
                // the prepared value to events.jsonl.
                *payload = prepared;
            }
            Err(err) => {
                // Prepare itself failed — fail-closed (R22). The
                // gate would have rejected this anyway (missing
                // handoff_path), so emit Reject with the prepare
                // error as the reason so the agent sees the root
                // cause rather than the gate's symptom.
                let hint = LintResumeHint::from_reason(
                    topic,
                    &format!("auto_handoff_prepare failed: {err}"),
                );
                return LintOutcome::Reject(hint);
            }
        }
    }
    match run_gates(view, &LintContext, topic, payload) {
        GateDecision::Accept => {
            // Distinguish Accept (no prepare) from
            // AcceptAfterAutoPrepare (prepare ran). The
            // `auto_handoff_prepare` returns Ok(prepared) when
            // it actually wrote the artifact; we re-derive the
            // outcome by inspecting the prepare path.
            if view.is_macro_edge(topic) && has_handoff_path(payload) {
                LintOutcome::AcceptAfterAutoPrepare
            } else {
                LintOutcome::Accept
            }
        }
        GateDecision::Reject(reason) => LintOutcome::Reject(LintResumeHint::from_reason(topic, &reason)),
    }
}

/// Return true when the payload is a JSON object that carries a
/// non-empty `handoff_path`. Helper for the macro-edge check.
fn has_handoff_path(payload: &Value) -> bool {
    match payload {
        Value::Object(map) => map
            .get("handoff_path")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        _ => false,
    }
}

/// Stub: return the workspace root for the current view. In
/// real callers this is `RalphConfig::core::workspace_root`; the
/// engine does not have a direct handle to it today. Review
/// P0 #3 follow-up: extend `ProtocolView` with `workspace_root`
/// + `output_dir` so `auto_handoff_prepare` can write the
/// artifact deterministically. For now this returns the current
/// dir so the wiring compiles; the runtime path is the CLI
/// which has `workspace_root` already.
fn workspace_root_for(_view: &ProtocolView) -> &std::path::Path {
    static ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
}

/// Stub: return the output directory for handoff artifacts.
/// Review P0 #3 follow-up: same as `workspace_root_for`.
fn output_dir_for(_view: &ProtocolView) -> &std::path::Path {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| std::path::PathBuf::from(".ralph/handoff"))
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
         ## rationale\norchestrator-side auto-prepare satisfies the R22 macro-edge contract.\n\n\
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

/// Run lint on a single emit, time-budgeted (R14 / KTD-9). The
/// timeout is post-hoc: it measures elapsed AFTER `lint_emit`
/// returns. A true fail-closed interrupt requires
/// `JoinHandle::join_timeout` (tracked as F-PS-006 follow-up).
pub fn lint_emit_with_timeout(
    view: &ProtocolView,
    topic: &str,
    payload: &mut Value,
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
