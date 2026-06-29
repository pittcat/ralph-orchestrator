//! `mechanism.flow` declaration lint rules (U5).
//!
//! Why this exists: the 2026-06-26 incident produced a 4/8
//! partial-completion state that the runtime had no way to
//! classify — the preset described its workflow as a free-form
//! `body:` list, so when `unit_loop` finished with 4 of 8
//! units done there was no `on_partial` branch for the
//! coordinator to fall through. The runtime had to invent one
//! from the `verdict_gate` topic, which silently routed the
//! event to the shipper and broke review.
//!
//! The four rules here make that class of bug catchable at
//! preset-load time rather than at the worst possible moment:
//!
//! - `flow_declaration_missing` — preset has no `mechanism.flow`
//!   at all. This is the umbrella rule; the other rules only
//!   fire once the declaration exists.
//! - `flow_partial_state_undeclared` — a step with
//!   `terminal_when` in `{all_done, any_failed, partial_units_done}`
//!   is missing `on_partial`.
//! - `flow_partial_branch_empty` — `on_partial.<key>` is empty.
//! - `flow_terminal_emit_missing` — `terminal_emits` does not
//!   contain `LOOP_COMPLETE`.
//! - `flow_unknown_emit_rejected` — `allowed_emits` references a
//!   topic the runtime doesn't know how to validate against.

use crate::config::RalphConfig;
use crate::event_loop::flow_declaration::{FlowDeclaration, FlowParseError, is_partial_state};
use crate::preset_lint::LintFinding;
use crate::preset_lint::finding_id::{
    FINDING_FLOW_DECLARATION_MISSING, FINDING_FLOW_PARTIAL_BRANCH_EMPTY,
    FINDING_FLOW_PARTIAL_STATE_UNDECLARED, FINDING_FLOW_TERMINAL_EMIT_MISSING,
    FINDING_FLOW_UNKNOWN_EMIT_REJECTED,
};
use serde_yaml::Value;

/// Run every flow declaration rule and return the findings.
///
/// `raw_yaml` is the raw preset YAML the lint was loaded
/// with. It is forwarded as-is to `FlowDeclaration::from_yaml`,
/// so the lint sees exactly what the operator wrote (rather
/// than a synthesised re-serialisation that may lose fields
/// the typed config struct doesn't model — e.g. `mechanism:`).
pub fn check_flow_declaration(raw_yaml: &str) -> Result<Vec<LintFinding>, FlowParseError> {
    let decl = match FlowDeclaration::from_yaml(raw_yaml) {
        Ok(d) => d,
        Err(FlowParseError::MissingMechanismFlow) => {
            return Ok(vec![missing_flow_finding(
                "preset YAML has no `mechanism.flow` block",
            )]);
        }
        Err(other) => return Err(other),
    };

    let mut findings = Vec::new();

    // 1. Partial-state branches must be declared.
    for step in &decl.steps {
        let Some(terminal_when) = step.terminal_when.as_deref() else {
            continue;
        };
        if !is_partial_state(terminal_when) {
            continue;
        }
        if step.on_partial.is_empty() {
            let mut f = LintFinding::new(
                FINDING_FLOW_PARTIAL_STATE_UNDECLARED,
                format!(
                    "step '{}' declares terminal_when='{}' which is a partial state but on_partial is missing or empty",
                    step.id, terminal_when
                ),
            );
            f.action_hint = Some(format!(
                "add `on_partial: {{ all_done: '<emit>', any_failed: '<emit>', partial_units_done: '<emit>' }}` to step '{}'",
                step.id
            ));
            findings.push(f);
            continue;
        }
        for (key, value) in &step.on_partial {
            if value.trim().is_empty() {
                let mut f = LintFinding::new(
                    FINDING_FLOW_PARTIAL_BRANCH_EMPTY,
                    format!(
                        "step '{}' on_partial.{} is empty; an empty branch silently swallows the partial state",
                        step.id, key
                    ),
                );
                f.action_hint = Some(format!(
                    "set `on_partial.{key}` to a non-empty emit expression on step '{}'",
                    step.id
                ));
                findings.push(f);
            }
        }
    }

    // 2. terminal_emits must include the runtime's
    //    `event_loop.completion_promise` so the verdict gate
    //    and the flow declaration agree on what terminates
    //    the loop. We accept either an explicit
    //    `LOOP_COMPLETE` (the default completion promise)
    //    or the per-preset `completion_promise` value.
    let raw_value: serde_yaml::Value =
        serde_yaml::from_str(raw_yaml).unwrap_or(serde_yaml::Value::Null);
    let completion_promise = raw_value
        .get("event_loop")
        .and_then(|el| el.get("completion_promise"))
        .and_then(|v| v.as_str())
        .unwrap_or("LOOP_COMPLETE");
    if !decl.terminal_emits.iter().any(|t| t == completion_promise) {
        let mut f = LintFinding::new(
            FINDING_FLOW_TERMINAL_EMIT_MISSING,
            format!(
                "flow declaration `terminal_emits` ({:?}) does not contain `{}` (the preset's completion_promise)",
                decl.terminal_emits, completion_promise
            ),
        );
        f.action_hint = Some(format!(
            "add `{}` to `terminal_emits` (the runtime verdict gate locks this set)",
            completion_promise
        ));
        findings.push(f);
    }

    // 3. allowed_emits should not contain topics that are
    //    obviously broken (e.g. uppercase identifiers that
    //    look like control topics not in the whitelist). The
    //    full schema check is wired in U10 once presets are
    //    updated; here we only catch the common typos.
    let known_topics = collect_known_topics(raw_yaml);
    for step in &decl.steps {
        for topic in &step.allowed_emits {
            if !known_topics.contains(topic) {
                let mut f = LintFinding::new(
                    FINDING_FLOW_UNKNOWN_EMIT_REJECTED,
                    format!(
                        "step '{}' allows topic `{}` but no schema entry or whitelist match exists",
                        step.id, topic
                    ),
                );
                f.action_hint = Some(format!(
                    "add `{topic}` to `event_policy.schemas` or to `topic_format_whitelist`, or remove it from `allowed_emits`"
                ));
                findings.push(f);
            }
        }
    }

    Ok(findings)
}

fn missing_flow_finding(detail: &str) -> LintFinding {
    let mut f = LintFinding::new(
        FINDING_FLOW_DECLARATION_MISSING,
        format!(
            "{detail}; without it the runtime cannot enforce step scope, partial-state recovery, or terminal alignment"
        ),
    );
    // P0-3 (2026-06-27 adversarial review): the
    // `mechanism:` block is opt-in. The
    // `flow_declaration_missing` finding is a
    // **warning** by default (operators can
    // declare the block when they want strict
    // step-scope enforcement) so legacy
    // presets continue to pass lint until
    // they opt in. Strict lint upgrades it
    // to `Error` via the existing strict
    // promotion path (see
    // `preset_lint::LintFinding::promote_to_error_if_strict`).
    f.severity = crate::preset_lint::LintSeverity::Warn;
    f.action_hint = Some(
        "add a `mechanism:` top-level key with `flow:` sub-key to the preset YAML; see \
         `docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md` \
         appendix A for the field reference"
            .to_string(),
    );
    f
}

/// Collect the set of topic names the runtime already knows
/// about: declared in `event_policy.schemas` plus the
/// well-known whitelist (LOOP_COMPLETE, REPORT_DONE, etc.).
///
/// Reads directly from the raw YAML so unknown fields like
/// `mechanism:` are preserved by the parser.
fn collect_known_topics(raw_yaml: &str) -> std::collections::HashSet<String> {
    let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Whitelist of well-known control topics that the
    // topic_format rule already exempts from format checks.
    for topic in ["LOOP_COMPLETE", "REPORT_DONE", "REVIEW_COMPLETE"] {
        set.insert(topic.to_string());
    }

    if let Ok(value) = serde_yaml::from_str::<Value>(raw_yaml) {
        // 1. Event-policy schemas declare payload contracts.
        if let Some(schemas) = value
            .get("event_loop")
            .and_then(|el| el.get("event_policy"))
            .and_then(|ep| ep.get("schemas"))
            .and_then(|s| s.as_mapping())
        {
            for (topic, _rule) in schemas {
                if let Some(topic) = topic.as_str() {
                    set.insert(topic.to_string());
                }
            }
        }

        // 2. Top-level `topic_format_whitelist` declares
        //    operator-approved topics (e.g. legacy
        //    `task.relocate_legacy` in `ce-executor-serial`).
        //    Including them here lets the flow declaration
        //    lint allow the same set the topic-format lint
        //    allows, so the two stay in lockstep.
        if let Some(whitelist) = value
            .get("topic_format_whitelist")
            .and_then(|w| w.as_sequence())
        {
            for topic in whitelist {
                if let Some(t) = topic.as_str() {
                    set.insert(t.to_string());
                }
            }
        }

        // 3. Hat `publishes:` declarations — a topic that
        //    some hat is contractually allowed to emit is
        //    by definition a known topic. This keeps
        //    `flow.allowed_emits` in lockstep with the hat
        //    topology without forcing the operator to add
        //    the topic to a schema or whitelist just to
        //    silence the lint.
        if let Some(hats) = value.get("hats").and_then(|h| h.as_mapping()) {
            for (_hat_id, hat) in hats {
                if let Some(publishes) = hat.get("publishes").and_then(|p| p.as_sequence()) {
                    for topic in publishes {
                        if let Some(t) = topic.as_str() {
                            set.insert(t.to_string());
                        }
                    }
                }
            }
        }
    }

    set
}

// Back-compat overload — kept so existing callers that pass a
// typed config (rather than raw YAML) still compile. Used by
// the U5 wiring step in run_preset_lint.
pub fn check_flow_declaration_with_config(
    config: &RalphConfig,
) -> Result<Vec<LintFinding>, FlowParseError> {
    let raw = serde_yaml::to_string(config).unwrap_or_default();
    check_flow_declaration(&raw)
}

#[cfg(test)]
mod tests;
