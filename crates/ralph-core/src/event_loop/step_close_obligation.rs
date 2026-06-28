//! U12 (2026-06-27 mechanism foundation completion):
//! step-close obligation — the pure-logic core that
//! tracks per-step partial progress and decides what
//! emit (if any) the next business event must satisfy
//! to close the step.
//!
//! Why this module exists: the 2026-06-26 diagnostic
//! (`iter=17`) observed a 4/8 partial state followed
//! by **silence** — the loop emitted no further events
//! and the runtime failed to flag the obligation. The
//! legacy code only enforced partial-state via the
//! `FlowStepScopeStage`'s `reason_pattern` check, which
//! fires **when** an emit arrives. It cannot catch the
//! "no emit at all" case.
//!
//! U12 introduces the obligation registry: a step in a
//! partial state carries an explicit obligation that
//! the next emit (or the loop-idle hook) must satisfy.
//! The registry is in-memory per loop and is
//! deliberately small — the SSoT for partial-state
//! semantics is still `FlowDeclaration::on_partial`.
//!
//! Cross-platform / concurrency semantics: pure CPU
//! only. No FS, no async.

use serde_json::Value;

/// The current partial progress on a step, expressed
/// as `done / total`. When `done < total` the step is
/// in a partial state and the next emit must satisfy
/// `on_partial` to close it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepProgress {
    /// Number of units finished (e.g. 4 in "4/8").
    pub done: u32,
    /// Total units the step is expected to drive (e.g. 8 in "4/8").
    pub total: u32,
}

impl StepProgress {
    /// `true` when the step has run every unit and
    /// therefore does not require a partial-state emit
    /// to close.
    pub fn is_complete(self) -> bool {
        self.total > 0 && self.done >= self.total
    }

    /// `true` when at least one unit ran and the
    /// remaining units could still be in flight. This
    /// is the canonical "partial" state that U12's
    /// obligation enforces.
    pub fn is_partial(self) -> bool {
        self.done > 0 && self.done < self.total
    }
}

/// One entry from a step's `on_partial` map. The
/// `reason_pattern` is the substring the partial emit's
/// `reason` field must contain (mirrors the appendix A
/// table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialObligation {
    /// Terminal-when key this obligation is keyed
    /// under (`all_done` / `any_failed` /
    /// `partial_units_done`).
    pub branch: String,
    /// Topic + reason pattern the next emit must match.
    /// `plan.blocked(reason="partial_units_done_continue_to_review")`
    /// is a typical example.
    pub expected_topic: String,
    pub expected_reason_pattern: String,
}

/// The current obligation on a step. `None` when no
/// enforcement is required (the step is complete, or
/// has not started, or has no `on_partial` map).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Obligation {
    /// No obligation — the next emit is unconstrained.
    None,
    /// The next emit must match one of the listed
    /// branches.
    Pending(Vec<PartialObligation>),
}

/// Compute the obligation for a step, given the
/// progress so far and the declared `on_partial` map.
///
/// Rules (locked by U12):
/// 1. `progress.is_complete()` → `Obligation::None`.
/// 2. `progress.is_partial()` AND `on_partial` is
///    non-empty → `Obligation::Pending(...)` with one
///    entry per branch.
/// 3. Otherwise → `Obligation::None`.
pub fn required_emit(
    progress: StepProgress,
    on_partial: &std::collections::BTreeMap<String, String>,
) -> Obligation {
    if progress.is_complete() || !progress.is_partial() {
        return Obligation::None;
    }
    if on_partial.is_empty() {
        return Obligation::None;
    }
    let obligations = on_partial
        .iter()
        .map(|(branch, directive)| parse_directive(branch, directive))
        .collect();
    Obligation::Pending(obligations)
}

/// Parse a `on_partial` directive string into a
/// `PartialObligation`. The directive grammar is
/// `<topic>(reason="<pattern>")` — the parenthesis
/// pattern is what `presets/schemas/ce-executor-serial.yml`
/// uses. Falls back to a plain topic without a pattern
/// when the directive does not match.
fn parse_directive(branch: &str, directive: &str) -> PartialObligation {
    // The directive shape is e.g.
    // `plan.blocked(reason="4_of_8_partial")`. Split
    // on `(` and trim.
    let (topic_part, reason_part) = match directive.split_once('(') {
        Some((topic, rest)) => (topic.trim().to_string(), rest.trim_end_matches(')').to_string()),
        None => (directive.trim().to_string(), String::new()),
    };
    // Extract `reason="<pattern>"` from the rest.
    let reason_pattern = extract_reason_pattern(&reason_part);
    PartialObligation {
        branch: branch.to_string(),
        expected_topic: topic_part,
        expected_reason_pattern: reason_pattern,
    }
}

fn extract_reason_pattern(rest: &str) -> String {
    // Look for `reason="..."` and return the inner
    // pattern. We deliberately use a hand-rolled
    // parser instead of serde_json so the directive
    // does not need to be a complete JSON document.
    if let Some(start) = rest.find("reason=\"") {
        let after = &rest[start + "reason=\"".len()..];
        if let Some(end) = after.find('"') {
            return after[..end].to_string();
        }
    }
    String::new()
}

/// Decide whether the given emit satisfies the
/// pending obligation. Returns `true` when:
/// - the obligation is `None` (any emit is fine), OR
/// - the emit's topic matches one of the obligation's
///   `expected_topic` values AND the payload's `reason`
///   contains the expected substring.
pub fn emit_satisfies_obligation(obligation: &Obligation, event_topic: &str, event_payload: &str) -> bool {
    match obligation {
        Obligation::None => true,
        Obligation::Pending(branches) => branches.iter().any(|b| {
            b.expected_topic == event_topic
                && reason_contains(event_payload, &b.expected_reason_pattern)
        }),
    }
}

fn reason_contains(payload: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        // An empty pattern matches any payload that has
        // a `reason` field at all (including `null`).
        let value: Option<Value> = serde_json::from_str(payload).ok();
        match value {
            Some(Value::Object(obj)) => obj.contains_key("reason"),
            _ => false,
        }
    } else {
        let value: Option<Value> = serde_json::from_str(payload).ok();
        match value.and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(String::from)) {
            Some(reason) => reason.contains(pattern),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests;