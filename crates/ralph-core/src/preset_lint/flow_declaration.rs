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
    FINDING_FLOW_DECLARATION_MISSING, FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY,
    FINDING_FLOW_PARTIAL_BRANCH_EMPTY, FINDING_FLOW_PARTIAL_STATE_UNDECLARED,
    FINDING_FLOW_REVIEW_COMPLETE_IN_UNIT_LOOP_BODY, FINDING_FLOW_TERMINAL_EMIT_MISSING,
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

    // 4. U8 (plan 2026-07-04-004): `review.complete` MUST NOT
    //    appear in `unit_loop.body`. The unit_loop is
    //    `foreach over plan units`; `review.complete` only
    //    fires after all units are done via the `review_walk`
    //    step. Mixing the two produces a state machine where
    //    the runtime tries to route a single per-unit
    //    iteration through the per-plan review pipeline —
    //    exactly the shape that produced the 2026-07-04
    //    silent-success run. Severity is `Error` regardless of
    //    strictness because the rule is purely structural.
    findings.extend(check_review_complete_not_in_unit_loop_body(raw_yaml));

    // 5. U4 (plan 2026-07-28-001): `kind: linear` step has
    //    multiple allowed emits but no forward target names
    //    any of them. The runtime falls back to positional
    //    advance, which silently produces the
    //    `flow_drift_positional_fallback` class of bug
    //    (e.g. the 2026-07-27 parallel-forge primary run
    //    where `forge.plan.inspected` landed in `exec_wave`
    //    instead of `plan_authoring`). Surfacing this guard
    //    at preset-load time prevents authoring a flow that
    //    relies on positional fallback to wire up handoffs.
    findings.extend(check_flow_linear_positional_ambiguity(raw_yaml));

    Ok(findings)
}

/// 2026-07-28-001 plan U4 (R8 / S8): a non-final `kind: linear` step
/// declares at least two allowed emits but NO forward step has an
/// `on` / `on_any_of` that names any of those topics.
///
/// Trigger conditions (all must hold):
///   1. The step is NOT the last step in the flow.
///   2. The step has `kind: linear` (other kinds — `side_effect`,
///      `await`, `foreach`, `sequence`, `terminal` — are exempt
///      because their transition model is different).
///   3. `allowed_emits.len() >= 2` (single-topic steps have no
///      ambiguity since the runtime either has a forward target
///      or has to fall through linearly, which is the legacy
///      position-based contract).
///   4. The intersection of `{topics in this step's allowed_emits}`
///      and `{topics in any forward step's on ∪ on_any_of}` is
///      EMPTY **after removing non-transition topics**
///      (`work.failed` / `work.done` / `work.ready` / `exec.unit.*`
///      / `review.unit.*` / `fix.unit.*`).
///
/// Action hint: declare the next step's `on` (or the multi-source
/// branch's `on_any_of`) to name the transition explicitly.
///
/// Severity: `Error` in strict mode (so the lint surface stays quiet
/// for legacy presets; only `--strict` surfaces this). The default
/// mode is permissive by design — see plan §3.1 D5 for the
/// rationale.
pub fn check_flow_linear_positional_ambiguity(raw_yaml: &str) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Ok(value) = serde_yaml::from_str::<Value>(raw_yaml) else {
        return findings;
    };

    let Some(steps) = value
        .get("mechanism")
        .and_then(|m| m.get("flow"))
        .and_then(|f| f.get("steps"))
        .and_then(|s| s.as_sequence())
    else {
        return findings;
    };

    // Mirror `event_loop::advance_plan_step::NON_TRANSITION_TOPICS`
    // so the lint does not flag topics that the runtime will never
    // use as a transition signal. Keeping this list local avoids a
    // cross-crate dep on the runtime's `pub(crate)` constant.
    const NON_TRANSITION_TOPICS: &[&str] = &[
        "work.done",
        "work.failed",
        "work.ready",
        "exec.unit.ready",
        "exec.unit.done",
        "exec.unit.failed",
        "review.unit.ready",
        "review.unit.done",
        "fix.unit.ready",
        "fix.unit.done",
        "fix.unit.failed",
    ];

    let step_count = steps.len();
    for (idx, step) in steps.iter().enumerate() {
        let id = step.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let is_last = idx + 1 == step_count;

        // Exemption 1: not the last step.
        if is_last {
            continue;
        }
        // Exemption 2: must be kind == "linear".
        let kind = step.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if kind != "linear" {
            continue;
        }
        // Exemption 3: must have at least 2 allowed emits.
        let Some(allowed) = step.get("allowed_emits").and_then(|a| a.as_sequence()) else {
            continue;
        };
        if allowed.len() < 2 {
            continue;
        }
        let allowed_topics: Vec<String> = allowed
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        // Compute the forward target topic set: any topic named by
        // a forward step's `on` or `on_any_of`.
        let mut forward_targets: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for later in steps.iter().skip(idx + 1) {
            if let Some(on) = later.get("on").and_then(|v| v.as_str()).map(String::from) {
                forward_targets.insert(on);
            }
            if let Some(seq) = later.get("on_any_of").and_then(|a| a.as_sequence()) {
                for t in seq {
                    if let Some(s) = t.as_str() {
                        forward_targets.insert(s.to_string());
                    }
                }
            }
        }

        // Filter non-transition topics from the trigger: a topic
        // that the runtime never treats as a transition signal
        // (e.g. `work.failed`) does NOT need a forward target, and
        // surfacing it would be a false positive. Per plan §3.1 the
        // failure-capable step explicitly keeps `work.failed` in
        // its `allowed_emits` to satisfy the FlowStepScope gate
        // even though it never advances the step.
        let ambiguous: Vec<&String> = allowed_topics
            .iter()
            .filter(|t| {
                !NON_TRANSITION_TOPICS.contains(&t.as_str()) && !forward_targets.contains(*t)
            })
            .collect();
        if ambiguous.is_empty() {
            continue;
        }
        // The finding fires only when the step has at least one
        // topic that the runtime would fall back to advance
        // positionally for. We surface all such topics in the
        // message so the operator can wire each one.
        let topics_list: Vec<String> = allowed_topics.iter().map(|s| format!("`{s}`")).collect();
        let topics_str = topics_list.join(", ");
        let ambiguous_list: Vec<String> = ambiguous.iter().map(|s| format!("`{s}`")).collect();
        let ambiguous_str = ambiguous_list.join(", ");
        let mut f = LintFinding::new(
            FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY,
            format!(
                "step '{id}' (kind=linear, non-final) has multiple allowed emits ({topics_str}) \
                 but no forward step names any of them via `on` or `on_any_of`. The runtime will \
                 fall back to positional advance for {ambiguous_str}, which silently produces the \
                 `flow_drift_positional_fallback` class of bug. \
                 Declare a forward `on` (single target) or `on_any_of` (multi-source branch) for each."
            ),
        );
        f.hat = Some(id.to_string());
        f.action_hint = Some(format!(
            "add `on: <topic>` (single target) or `on_any_of: [<topics>]` (branch) to the next step, \
             and ensure every transition-capable topic in step '{id}'.allowed_emits appears in some \
             forward step's on/on_any_of. Topics needing forward targets: {ambiguous_str}"
        ));
        findings.push(f);
    }
    findings
}

/// U8 (plan 2026-07-04-004) helper: scan the raw YAML's
/// `mechanism.flow.steps[]` list; for any step whose `id` is
/// `unit_loop`, ensure `body` does NOT include `review.complete`.
///
/// `review.complete` is the terminal topic of the per-plan
/// `review_walk` step (after all units are done). It belongs in
/// `review_walk.body` (or the review step's `terminal_emits`),
/// NOT in `unit_loop.body`. Surfacing this guard at preset-load
/// time prevents the silent-success anti-pattern where the
/// runtime tries to fire per-plan review inside a per-unit loop.
///
/// Returns zero findings when no `unit_loop` step is declared
/// (presets without the foreach semantics — e.g.
/// `ce-executor-pipeline` — are out of scope).
fn check_review_complete_not_in_unit_loop_body(raw_yaml: &str) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Ok(value) = serde_yaml::from_str::<Value>(raw_yaml) else {
        return findings;
    };

    let Some(steps) = value
        .get("mechanism")
        .and_then(|m| m.get("flow"))
        .and_then(|f| f.get("steps"))
        .and_then(|s| s.as_sequence())
    else {
        return findings;
    };

    for step in steps {
        let id = step.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id != "unit_loop" {
            continue;
        }
        let Some(body) = step.get("body").and_then(|b| b.as_sequence()) else {
            continue;
        };
        for topic in body {
            let topic_name = topic.as_str().unwrap_or("");
            // Match `review.complete` and any topic that
            // starts with `review.complete` (e.g.
            // `review.complete.foo`) so a future topic
            // family cannot smuggle in the anti-pattern.
            if topic_name == "review.complete" || topic_name.starts_with("review.complete.") {
                let mut f = LintFinding::new(
                    FINDING_FLOW_REVIEW_COMPLETE_IN_UNIT_LOOP_BODY,
                    format!(
                        "step 'unit_loop' body contains `{}`; review.complete is the \
                         per-plan review_walk terminal topic (after all units are done) \
                         and MUST NOT appear in unit_loop.body. The unit_loop is \
                         `foreach over plan units`; review.complete belongs in \
                         review_walk.body or the review step's terminal_emits. \
                         Mixing the two produces the silent-success shape where the \
                         runtime tries to fire per-plan review inside a per-unit loop.",
                        topic_name
                    ),
                );
                // `unit_loop` step id for the dashboard filter.
                f.hat = Some("unit_loop".to_string());
                f.action_hint = Some(format!(
                    "Move `{}` from unit_loop.body to review_walk.body (or to the \
                     review step's terminal_emits). The unit_loop only emits per-unit \
                     events: work.ready, work.done, work.failed, test.passed, \
                     test.failed, fix.applied, fix.exhausted.",
                    topic_name
                ));
                // Severity stays Error — structural mismatch.
                findings.push(f);
            }
        }
    }

    findings
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
