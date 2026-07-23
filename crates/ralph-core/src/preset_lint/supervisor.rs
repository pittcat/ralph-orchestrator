//! 2026-07-03-001 plan U9: supervisor preset lint rules.
//!
//! Four rules, all `Error` severity:
//!
//! - `FINDING_SUPERVISOR_REQUIRES_ISOLATED` — `event_loop.supervisor.enabled: true`
//!   without `event_loop.execution_mode: isolated` (R4).
//! - `FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE` — an
//!   integrator hat (named `exec-integrator` / `fix-integrator`)
//!   declares `*.unit.done` in `triggers:`. The integrator's
//!   real handoff trigger is `*.wave.complete`; slot-level
//!   done events belong to the worker fan-out (KTD-6).
//! - `FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC` — a hat's
//!   `publishes:` claims one of the six supervisor coordination
//!   topics. Per R14 only the supervisor may publish those.
//! - `FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY` — 2026-07-22
//!   plan U3: a wave consumer hat (one whose `triggers:` includes
//!   a `*.unit.ready` topic) declares `concurrency <= 1` (the
//!   default). The wave detector in `wave_detection.rs`
//!   rejects such hats as `SequentialTarget`, silently dropping
//!   the entire wave batch. This lint forces the author to
//!   explicitly opt in to concurrency by setting `concurrency > 1`.
//!
//! Plain-YAML entry point (no `RalphConfig` parsing) so the
//! lint runs on partially-typed presets and stays stable
//! across `RalphConfig` refactors.

use crate::event_origin::SUPERVISOR_COORDINATION_TOPICS;
use crate::preset_lint::LintFinding;
pub use crate::preset_lint::finding_id::{
    FINDING_SUPERVISOR_ALIGNMENT_PUBLISHES_WAVE_READY, FINDING_SUPERVISOR_ALIGNMENT_TRIGGERS_WAVE_READY,
    FINDING_SUPERVISOR_DELETED_HAT_REFERENCED, FINDING_SUPERVISOR_DELETED_HAT_REINSTATED,
    FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC, FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE,
    FINDING_SUPERVISOR_REQUIRES_ISOLATED, FINDING_SUPERVISOR_TASK_PLANNER_PUBLISHES_EXEC_READY,
    FINDING_SUPERVISOR_TASK_PLANNER_TRIGGERS_EXEC_READY,
    FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY,
};
use serde_yaml::Value;

/// Patterns of integrator hat ids the lint matches against.
/// Future integrators are obvious from naming — `*-integrator`
/// or `*_integrator`.
const INTEGRATOR_HAT_NAMES: &[&str] = &["exec-integrator", "fix-integrator", "review-integrator"];

/// Slot-level done topics the lint rejects on integrators.
/// These are emitted by the worker-side per-slot hat; the
/// integrator's `triggers:` list must NOT contain them (KTD-6).
const SLOT_DONE_TOPICS: &[&str] = &[
    "exec.unit.done",
    "exec.unit.failed",
    "fix.unit.done",
    "fix.unit.failed",
    "review.unit.done",
    "review.unit.failed",
];

/// Run all supervisor preset rules against the raw preset
/// YAML. Returns findings in stable order (coordinator rule
/// first, then integrator rule, then publisher rule, then
/// wave consumer concurrency rule) so the lint output is
/// deterministic across re-runs.
pub fn check_supervisor_rules(raw_yaml: &str) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let value: Value = match serde_yaml::from_str(raw_yaml) {
        Ok(v) => v,
        Err(_) => {
            // YAML parse failure is reported by other lint
            // rules; the supervisor check is a no-op so we
            // don't double-report.
            return findings;
        }
    };

    // R-SW-1: supervisor.enabled requires isolated mode.
    if let Some(finding) = check_requires_isolated(&value) {
        findings.push(finding);
    }

    // R-SW-2: integrator hat must not subscribe to *.unit.done.
    findings.extend(check_integrator_triggers(&value));

    // R-COORD-4: hat publishes must not claim supervisor
    // coordination topics.
    findings.extend(check_hat_publishes_coord_topic(&value));

    // R-SW-3 (2026-07-22 plan U3): wave consumer hats must
    // declare `concurrency > 1`. Only fires when the
    // supervisor is actually enabled — the rule is a
    // capability gate, not a blanket check.
    if is_supervisor_enabled(&value) {
        findings.extend(check_wave_consumer_concurrency(&value));
        // 2026-07-23-005 plan U2: `task-planner` ownership
        // transfer. The hat is the dependency auditor, not the
        // wave dispatcher, so it must not claim or consume
        // `exec.unit.ready`. The two checks below fail
        // loudly at preset-load time if a future refactor
        // silently re-routes fan-out through `task-planner`.
        findings.extend(check_task_planner_publishes_exec_ready(&value));
        findings.extend(check_task_planner_triggers_exec_ready(&value));
        // 2026-07-23-005 plan U7: `alignment` is the
        // read-only verifier and must not emit / consume
        // per-unit fan-out topics. Two sibling findings.
        findings.extend(check_alignment_publishes_wave_ready(&value));
        findings.extend(check_alignment_triggers_wave_ready(&value));
        // 2026-07-23-005 plan U8: deleted hats (progress-steward,
        // shipper, fixer) must not be resurrected. The lint
        // walks the entire preset (hats + business_topics +
        // schema references + state projection + anywhere else
        // a hat id could leak through) and reports any match.
        findings.extend(check_deleted_hats_reinstated(&value));
        findings.extend(check_deleted_hats_referenced(&value));
    }

    findings
}

/// Returns `true` when the preset declares
/// `event_loop.supervisor.enabled: true`. Used as the
/// capability gate for R-SW-3 (wave consumer concurrency).
fn is_supervisor_enabled(value: &Value) -> bool {
    value
        .get("event_loop")
        .and_then(|el| el.get("supervisor"))
        .and_then(|s| s.get("enabled"))
        .and_then(|e| e.as_bool())
        .unwrap_or(false)
}

/// Topics that mark a hat as a supervisor wave consumer.
/// The dispatcher publishes these in batches via
/// `ralph wave emit`; each consumer hat receives one
/// `*.unit.ready` event per slot and runs as a worker. If
/// the consumer hat's `concurrency <= 1`, the wave
/// detector (`wave_detection::try_build_wave`) rejects
/// the whole batch as `SequentialTarget` and silently
/// drops the N-1 extra slots. The lint catches this at
/// preset-load time.
const WAVE_CONSUMER_TRIGGER_TOPICS: &[&str] =
    &["exec.unit.ready", "review.unit.ready", "fix.unit.ready"];

/// R-SW-3 (2026-07-22 plan U3, R5 / R6): a hat whose
/// `triggers:` includes any `WAVE_CONSUMER_TRIGGER_TOPICS`
/// topic MUST declare `concurrency > 1`. The check runs
/// only when `event_loop.supervisor.enabled: true`. The
/// rule produces one finding per offending hat so the
/// operator can fix each worker independently.
fn check_wave_consumer_concurrency(value: &Value) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let hats = match value.get("hats").and_then(|h| h.as_mapping()) {
        Some(m) => m,
        None => return findings,
    };
    for (hat_id_value, hat_value) in hats {
        let hat_id = hat_id_value.as_str().unwrap_or("");
        let triggers: Vec<String> = hat_value
            .get("triggers")
            .and_then(|t| t.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let has_wave_trigger = triggers
            .iter()
            .any(|t| WAVE_CONSUMER_TRIGGER_TOPICS.contains(&t.as_str()));
        if !has_wave_trigger {
            continue;
        }
        // Read `concurrency` directly from the YAML map
        // rather than the typed `HatConfig` — this is a
        // raw-YAML lint, and missing `concurrency` is
        // semantically the same as `concurrency: 1` (the
        // runtime default). Both must fire.
        let concurrency = hat_value
            .get("concurrency")
            .and_then(|c| c.as_u64())
            .unwrap_or(1);
        if concurrency > 1 {
            continue;
        }
        let mut f = LintFinding::new(
            FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY,
            format!(
                "hat `{hat_id}` subscribes to a supervisor wave topic (`*.unit.ready`) but \
                 declares `concurrency: {concurrency}`. The wave detector in \
                 `crates/ralph-core/src/wave_detection.rs` rejects the entire wave batch as \
                 `SequentialTarget` when the consumer hat's concurrency is <= 1, silently \
                 dropping N-1 slots. The dispatcher publishes a complete batch; the runtime \
                 needs `concurrency > 1` on every consumer hat to dispatch the slots in parallel."
            ),
        );
        f.action_hint = Some(format!(
            "set `hats.{hat_id}.concurrency` to a value greater than 1 (builtin supervisor \
             hats use `concurrency: 4`); the effective per-wave concurrency is \
             `min(hat.concurrency, event_loop.supervisor.max_concurrent_workers)`"
        ));
        findings.push(f);
    }
    findings
}

/// R-SW-1: `event_loop.supervisor.enabled: true` AND
/// `event_loop.execution_mode != isolated` → Error.
fn check_requires_isolated(value: &Value) -> Option<LintFinding> {
    let event_loop = value.get("event_loop")?;
    let supervisor_enabled = event_loop
        .get("supervisor")
        .and_then(|s| s.get("enabled"))
        .and_then(|e| e.as_bool())
        .unwrap_or(false);
    if !supervisor_enabled {
        return None;
    }
    let mode = event_loop
        .get("execution_mode")
        .and_then(|m| m.as_str())
        .unwrap_or("");
    if mode == "isolated" {
        return None;
    }
    let mut f = LintFinding::new(
        FINDING_SUPERVISOR_REQUIRES_ISOLATED,
        "event_loop.supervisor.enabled: true requires event_loop.execution_mode: isolated; \
         the supervisor dispatcher branches on isolated mode and the legacy WaveTracker path is \
         not available in coordinator mode",
    );
    f.action_hint = Some(
        "set `event_loop.execution_mode: isolated` alongside \
         `event_loop.supervisor.enabled: true`, OR drop the supervisor block to fall back to the \
         legacy dispatcher"
            .to_string(),
    );
    Some(f)
}

/// R-SW-2: integrator hats declare `*.unit.done` in their
/// `triggers:` list. The lint matches hat names
/// `exec-integrator` / `fix-integrator` /
/// `review-integrator`. The lint ALSO matches a generic
/// `*-integrator` shape — future integrators fall under the
/// same rule.
fn check_integrator_triggers(value: &Value) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let hats = match value.get("hats").and_then(|h| h.as_mapping()) {
        Some(m) => m,
        None => return findings,
    };
    for (hat_id_value, hat_value) in hats {
        let hat_id = hat_id_value.as_str().unwrap_or("");
        let is_integrator = INTEGRATOR_HAT_NAMES.contains(&hat_id)
            || hat_id.ends_with("-integrator")
            || hat_id.ends_with("_integrator");
        if !is_integrator {
            continue;
        }
        let triggers: Vec<String> = hat_value
            .get("triggers")
            .and_then(|t| t.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for trigger in &triggers {
            if SLOT_DONE_TOPICS.contains(&trigger.as_str()) {
                let mut f = LintFinding::new(
                    FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE,
                    format!(
                        "integrator hat `{hat_id}` declares `{trigger}` in its `triggers:`; \
                         integrator must subscribe to `*.wave.complete`, NOT per-slot done topics. \
                         The merge gate is wave-level so the integrator receives the aggregate \
                         fan-in event, not the worker fan-out."
                    ),
                );
                f.action_hint = Some(format!(
                    "remove `{trigger}` from `{hat_id}.triggers` and add the matching `*.wave.complete` topic"
                ));
                findings.push(f);
            }
        }
    }
    findings
}

/// R-COORD-4: a hat's `publishes:` claim one of the six
/// supervisor coordination topics. Match
/// `*.wave.complete` / `*.wave.failed` patterns across all
/// WaveKind kinds; the supervisor is the only publisher.
fn check_hat_publishes_coord_topic(value: &Value) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let hats = match value.get("hats").and_then(|h| h.as_mapping()) {
        Some(m) => m,
        None => return findings,
    };
    for (hat_id_value, hat_value) in hats {
        let hat_id = hat_id_value.as_str().unwrap_or("");
        // The supervisor pseudo-hat is the only legitimate
        // publisher; we do not lint against the literal id
        // `supervisor` because the runtime injects via
        // `system_injected` and the hat registry does not own
        // a `supervisor` hat entry in U13 presets. We skip
        // any future `supervisor` hat id silently. The
        // `coordinator` pseudo-hat is also excluded: it
        // declares the system-visible end-points so the
        // fixture can show operators the full topology at a
        // glance, but the runtime blocks any agent-side emit
        // through the system_injected path alone. Wiring
        // coordinator emit gating lives in the dispatcher's
        // origin guard, not the lint.
        if hat_id == "supervisor" || hat_id == "coordinator" {
            continue;
        }
        // Synthesizers / integrators / orchestrator-side
        // hats commonly declare `*.wave.complete` /
        // `*.wave.failed` in their `publishes:` list because
        // that is the document surface; the actual emit is
        // gated through the origin guard (`system_injected`
        // only). The lint stays strict for **worker** hats
        // (anything outside the orchestrator whitelist).
        if hat_id.ends_with("-synthesizer")
            || hat_id.ends_with("_synthesizer")
            || hat_id.ends_with("-integrator")
            || hat_id.ends_with("_integrator")
            || hat_id.ends_with("-coordinator")
            || hat_id.ends_with("_coordinator")
        {
            continue;
        }
        let publishes: Vec<String> = hat_value
            .get("publishes")
            .and_then(|p| p.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for topic in &publishes {
            if SUPERVISOR_COORDINATION_TOPICS.contains(&topic.as_str()) {
                let mut f = LintFinding::new(
                    FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC,
                    format!(
                        "hat `{hat_id}` declares supervisor coordination topic `{topic}` in its \
                         `publishes:` list. Per R14 only the supervisor may inject \
                         `*.wave.complete` / `*.wave.failed` — agents publishing these are \
                         rejected at the origin guard (U7)."
                    ),
                );
                f.action_hint = Some(format!(
                    "remove `{topic}` from `{hat_id}.publishes`; supervisors own these topics"
                ));
                findings.push(f);
            }
        }
    }
    findings
}

/// 2026-07-23-005 plan U2: `task-planner` is the dependency
/// auditor and must NOT claim `exec.unit.ready` in `publishes:`.
/// The hat writes the static execution-plan artifact; per-unit
/// fan-out is the exec-wave dispatcher's job (U5). Re-introducing
/// `exec.unit.ready` here would silently restore the single-shot
/// broadcast that U2 just removed.
fn check_task_planner_publishes_exec_ready(value: &Value) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Some(hats) = value.get("hats").and_then(|h| h.as_mapping()) else {
        return findings;
    };
    let Some((_, hat_value)) = hats
        .iter()
        .find(|(id, _)| id.as_str() == Some("task-planner"))
    else {
        return findings;
    };
    let publishes: Vec<String> = hat_value
        .get("publishes")
        .and_then(|p| p.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if publishes.iter().any(|t| t == "exec.unit.ready") {
        let mut f = LintFinding::new(
            FINDING_SUPERVISOR_TASK_PLANNER_PUBLISHES_EXEC_READY,
            "hat `task-planner` declares `exec.unit.ready` in `publishes:`. Per 2026-07-23-005 \
             plan U2, task-planner is the dependency auditor and must not fan out per-unit \
             readiness events; that ownership belongs to the exec-wave dispatcher hat (U5)."
                .to_string(),
        );
        f.action_hint = Some(
            "remove `exec.unit.ready` from `task-planner.publishes:`; the exec-wave dispatcher \
             (U5) owns the per-unit fan-out."
                .to_string(),
        );
        findings.push(f);
    }
    findings
}

/// 2026-07-23-005 plan U2: `task-planner` must NOT consume
/// `exec.unit.ready` either. The hat is activated by
/// `work.ready`; if it also lists `exec.unit.ready` in
/// `triggers:` the runtime could re-route per-unit fan-out
/// through `task-planner` and bypass the exec-wave
/// dispatcher entirely. This is a sibling guard to
/// [`check_task_planner_publishes_exec_ready`].
fn check_task_planner_triggers_exec_ready(value: &Value) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Some(hats) = value.get("hats").and_then(|h| h.as_mapping()) else {
        return findings;
    };
    let Some((_, hat_value)) = hats
        .iter()
        .find(|(id, _)| id.as_str() == Some("task-planner"))
    else {
        return findings;
    };
    let triggers: Vec<String> = hat_value
        .get("triggers")
        .and_then(|t| t.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if triggers.iter().any(|t| t == "exec.unit.ready") {
        let mut f = LintFinding::new(
            FINDING_SUPERVISOR_TASK_PLANNER_TRIGGERS_EXEC_READY,
            "hat `task-planner` declares `exec.unit.ready` in `triggers:`. Per 2026-07-23-005 \
             plan U2, task-planner is the dependency auditor and must not consume per-unit \
             readiness events; the exec-wave dispatcher hat (U5) consumes them."
                .to_string(),
        );
        f.action_hint = Some(
            "remove `exec.unit.ready` from `task-planner.triggers:`; the exec-wave dispatcher \
             (U5) consumes the per-unit fan-out."
                .to_string(),
        );
        findings.push(f);
    }
    findings
}

/// Topics that mark a hat as a wave dispatcher. If `alignment`
/// claims or consumes any of them, it has silently become a
/// second fixer or dispatcher and bypasses the formal fix
/// chain. U7 hard rule: alignment is read-only.
const WAVE_DISPATCHER_TOPICS: &[&str] = &[
    "exec.unit.ready",
    "fix.unit.ready",
    "review.unit.ready",
];

/// 2026-07-23-005 plan U7: `alignment` must NOT publish
/// `*.unit.ready`. If it does, it has become a second wave
/// dispatcher. The fix chain (fix-task-planner → fix-worker
/// → fix-integrator) is the single source of code-change
/// fan-out.
fn check_alignment_publishes_wave_ready(value: &Value) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Some(hats) = value.get("hats").and_then(|h| h.as_mapping()) else {
        return findings;
    };
    let Some((_, hat_value)) = hats
        .iter()
        .find(|(id, _)| id.as_str() == Some("alignment"))
    else {
        return findings;
    };
    let publishes: Vec<String> = hat_value
        .get("publishes")
        .and_then(|p| p.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    for topic in &publishes {
        if WAVE_DISPATCHER_TOPICS.contains(&topic.as_str()) {
            let mut f = LintFinding::new(
                FINDING_SUPERVISOR_ALIGNMENT_PUBLISHES_WAVE_READY,
                format!(
                    "hat `alignment` declares wave dispatcher topic `{topic}` in `publishes:`. \
                     Per 2026-07-23-005 plan U7, alignment is a read-only verifier; emitting \
                     per-unit readiness events turns it into a second dispatcher and bypasses \
                     the formal fix chain."
                ),
            );
            f.action_hint = Some(
                "remove the wave dispatcher topic from `alignment.publishes:`; only \
                 `plan.complete` and `plan.blocked` are allowed."
                    .to_string(),
            );
            findings.push(f);
        }
    }
    findings
}

/// 2026-07-23-005 plan U7 (sibling of the publishes-side
/// check): `alignment` must NOT consume `*.unit.ready`
/// either — it is activated by `fix.done` and the formal
/// review / fix chain.
/// 2026-07-23-005 plan U8: list of hat ids that the
/// supervisor preset explicitly deleted. Each name MUST NOT
/// appear in `hats:`; if it does, the lint surfaces a hard
/// finding. The list is intentionally narrow: only
/// hats whose deletion is part of U8.
const DELETED_SUPERVISOR_HATS: &[&str] = &["progress-steward", "shipper", "fixer"];

/// 2026-07-23-005 plan U7: `alignment` must NOT consume
/// per-unit fan-out topics either. Same rationale as the
/// publishes-side sibling finding.
fn check_alignment_triggers_wave_ready(value: &Value) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Some(hats) = value.get("hats").and_then(|h| h.as_mapping()) else {
        return findings;
    };
    let Some((_, hat_value)) = hats
        .iter()
        .find(|(id, _)| id.as_str() == Some("alignment"))
    else {
        return findings;
    };
    let triggers: Vec<String> = hat_value
        .get("triggers")
        .and_then(|t| t.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    for topic in &triggers {
        if WAVE_DISPATCHER_TOPICS.contains(&topic.as_str()) {
            let mut f = LintFinding::new(
                FINDING_SUPERVISOR_ALIGNMENT_TRIGGERS_WAVE_READY,
                format!(
                    "hat `alignment` consumes wave dispatcher topic `{topic}` in `triggers:`. \
                     Per 2026-07-23-005 plan U7, alignment must not consume per-unit readiness \
                     events; it is triggered by `fix.done`."
                ),
            );
            f.action_hint = Some(
                "remove the wave dispatcher topic from `alignment.triggers:`; alignment is \
                 triggered by `fix.done`."
                    .to_string(),
            );
            findings.push(f);
        }
    }
    findings
}

/// 2026-07-23-005 plan U8: detect any deleted hat re-instated
/// in `hats:`. One finding per offender so the operator can
/// see which resurrection regressed the topology.
fn check_deleted_hats_reinstated(value: &Value) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Some(hats) = value.get("hats").and_then(|h| h.as_mapping()) else {
        return findings;
    };
    for deleted in DELETED_SUPERVISOR_HATS {
        if hats.contains_key(*deleted) {
            let mut f = LintFinding::new(
                FINDING_SUPERVISOR_DELETED_HAT_REINSTATED,
                format!(
                    "hat `{deleted}` was deleted by 2026-07-23-005 plan U8 and must not be \
                     reinstated. Each deleted hat had a specific architectural reason: \
                     `progress-steward` (no loop-level rescue), `shipper` (single reporter \
                     owner), `fixer` (no fallback fix chain)."
                ),
            );
            f.action_hint = Some(format!(
                "remove `{deleted}` from `hats:`; reporter is the single owner of \
                 plan.complete / plan.blocked (U8)."
            ));
            findings.push(f);
        }
    }
    findings
}

/// 2026-07-23-005 plan U8: detect any residual reference to
/// the deleted hats anywhere else in the preset
/// (state-projection entries, deny rules, trigger lists,
/// coordinator_hats lists, etc.). Walks every string-typed
/// scalar value and reports a single finding per deleted
/// hat. The walk is shallow on purpose — we want a clear
/// "you mentioned `progress-steward` somewhere" signal so
/// the operator can grep for it.
fn check_deleted_hats_referenced(value: &Value) -> Vec<LintFinding> {
    let mut findings: Vec<LintFinding> = Vec::new();
    let mut reported: std::collections::HashSet<&'static str> = std::collections::HashSet::new();
    walk_strings(value, &mut |s| {
        for deleted in DELETED_SUPERVISOR_HATS {
            if s == *deleted && !reported.contains(deleted) {
                let mut f = LintFinding::new(
                    FINDING_SUPERVISOR_DELETED_HAT_REFERENCED,
                    format!(
                        "preset contains a residual reference to deleted hat `{deleted}` \
                         outside `hats:`. 2026-07-23-005 plan U8 deleted this hat; remove \
                         the reference (state-projection / deny rule / coordinator_hats / \
                         etc.)."
                    ),
                );
                f.action_hint = Some(format!(
                    "grep for `{deleted}` and remove the stale reference"
                ));
                findings.push(f);
                reported.insert(deleted);
            }
        }
    });
    findings
}

/// Depth-first walk that visits every string-typed scalar
/// in a serde_yaml::Value tree (including mapping keys,
/// because state-projection entries use the hat id as the
/// key). The closure receives each string by reference.
fn walk_strings<F: FnMut(&str)>(value: &Value, visit: &mut F) {
    match value {
        Value::String(s) => visit(s),
        Value::Sequence(seq) => {
            for v in seq {
                walk_strings(v, visit);
            }
        }
        Value::Mapping(map) => {
            for (k, v) in map {
                walk_strings(k, visit);
                walk_strings(v, visit);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    //! U9 closed-circuit tests. Each finding has at least one
    //! positive and one negative YAML fixture; the negative
    //! fixtures are intentionally valid so the test pins the
    //! rule's intent (don't flag innocuous presets).

    use super::*;

    fn run(yaml: &str) -> Vec<LintFinding> {
        check_supervisor_rules(yaml)
    }

    #[test]
    fn enabled_supervisor_without_isolated_is_error() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: coordinator
hats:
  executor:
    publishes: [work.done]
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_REQUIRES_ISOLATED),
            "expected supervisor_requires_isolated, got {findings:?}"
        );
    }

    #[test]
    fn enabled_supervisor_with_isolated_is_silent() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
";
        let findings = run(yaml);
        assert!(
            findings.is_empty(),
            "isolated supervisor must pass, got {findings:?}"
        );
    }

    #[test]
    fn disabled_supervisor_is_silent() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: false
  execution_mode: coordinator
";
        let findings = run(yaml);
        assert!(
            findings.is_empty(),
            "disabled supervisor must not trigger R-SW-1, got {findings:?}"
        );
    }

    #[test]
    fn abscent_supervisor_block_is_silent() {
        let yaml = r"
event_loop:
  execution_mode: coordinator
";
        let findings = run(yaml);
        assert!(findings.is_empty());
    }

    #[test]
    fn integrator_triggers_slot_done_is_error() {
        let yaml = r"
hats:
  exec-integrator:
    triggers:
      - exec.wave.complete
      - exec.unit.done
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE),
            "expected integrator_slot_done finding, got {findings:?}"
        );
    }

    #[test]
    fn integrator_triggers_wave_complete_only_is_silent() {
        let yaml = r"
hats:
  fix-integrator:
    triggers:
      - fix.wave.complete
";
        let findings = run(yaml);
        assert!(
            findings.is_empty(),
            "integrator subscribing only to fix.wave.complete is valid, got {findings:?}"
        );
    }

    #[test]
    fn hat_publishes_coord_topic_is_error() {
        let yaml = r"
hats:
  rogue-integration:
    publishes:
      - exec.wave.complete
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC),
            "expected hat_publishes_coord_topic finding, got {findings:?}"
        );
    }

    #[test]
    fn hat_publishes_normal_topic_is_silent() {
        let yaml = r"
hats:
  executor:
    publishes:
      - work.done
";
        let findings = run(yaml);
        assert!(findings.is_empty());
    }

    #[test]
    fn review_integrator_matches_generic_suffix_too() {
        // The lint accepts both `review-integrator` (kebab)
        // and any `*-integrator` suffix for forward
        // compatibility.
        let yaml = r"
hats:
  custom-review-integrator:
    triggers:
      - review.unit.done
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE),
            "generic -integrator suffix must trigger R-SW-2, got {findings:?}"
        );
    }

    #[test]
    fn finding_ids_remain_stable_across_renames() {
        // Pin the string constants so dashboards / runtime
        // contracts that match by id never silently miss a
        // rename.
        assert_eq!(
            FINDING_SUPERVISOR_REQUIRES_ISOLATED,
            "preset.supervisor_requires_isolated"
        );
        assert_eq!(
            FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE,
            "preset.supervisor_integrator_triggers_slot_done"
        );
        assert_eq!(
            FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC,
            "preset.supervisor_hat_publishes_coord_topic"
        );
        assert_eq!(
            FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY,
            "preset.supervisor_wave_consumer_low_concurrency"
        );
    }

    // ──────────────────────────────────────────────────────────────────
    // 2026-07-22 plan U3 (R5): wave consumer concurrency lint tests.
    //
    // The rule: when `event_loop.supervisor.enabled: true`,
    // any hat whose `triggers:` includes a `*.unit.ready`
    // topic (the wave batch topic family) MUST declare
    // `concurrency > 1`. Otherwise the wave detector silently
    // rejects the entire batch as `SequentialTarget` — the
    // user sees N-1 dropped events without any explicit
    // diagnostic. The lint surfaces this at preset-load time.
    // ──────────────────────────────────────────────────────────────────

    /// The dispatcher hat publishes `exec.unit.ready`; the
    /// worker hat consumes it. When the worker hat's
    /// `concurrency` defaults to 1, the wave is silently
    /// dropped at runtime. The lint MUST fire.
    #[test]
    fn wave_consumer_default_concurrency_is_error() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  worker:
    triggers:
      - exec.unit.ready
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY),
            "expected wave_consumer_low_concurrency finding, got {:?}",
            findings.iter().map(|f| f.id).collect::<Vec<_>>()
        );
    }

    /// Same topology, but the worker hat declares
    /// `concurrency: 1` explicitly. The lint MUST still fire
    /// (concurrency <= 1 is the violation threshold; explicit
    /// 1 is no better than the default).
    #[test]
    fn wave_consumer_concurrency_one_is_error() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  worker:
    concurrency: 1
    triggers:
      - exec.unit.ready
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY),
            "concurrency=1 must trigger the finding (SequentialTarget at runtime), got {:?}",
            findings.iter().map(|f| f.id).collect::<Vec<_>>()
        );
    }

    /// The lint is conservative: boundary `concurrency: 2`
    /// MUST pass — that's the minimum that makes the wave
    /// detector accept the batch.
    #[test]
    fn wave_consumer_concurrency_two_is_silent() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  worker:
    concurrency: 2
    triggers:
      - exec.unit.ready
";
        let findings = run(yaml);
        assert!(
            !findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY),
            "concurrency=2 must not trigger the finding, got {:?}",
            findings.iter().map(|f| f.id).collect::<Vec<_>>()
        );
    }

    /// `concurrency: 4` (the value builtin supervisor
    /// workers now use) MUST pass.
    #[test]
    fn wave_consumer_concurrency_four_is_silent() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  worker:
    concurrency: 4
    triggers:
      - exec.unit.ready
";
        let findings = run(yaml);
        assert!(
            !findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY),
            "concurrency=4 must not trigger the finding, got {:?}",
            findings.iter().map(|f| f.id).collect::<Vec<_>>()
        );
    }

    /// The rule targets all three wave batch topic families
    /// (`exec.unit.ready` / `review.unit.ready` /
    /// `fix.unit.ready`). Each family has its own consumer
    /// hat and each MUST declare `concurrency > 1`.
    #[test]
    fn all_three_wave_consumer_families_are_linted() {
        for wave_topic in &["exec.unit.ready", "review.unit.ready", "fix.unit.ready"] {
            let yaml = format!(
                "event_loop:\n  supervisor:\n    enabled: true\n  execution_mode: isolated\nhats:\n  worker:\n    triggers:\n      - {}\n",
                wave_topic
            );
            let findings = run(&yaml);
            assert!(
                findings
                    .iter()
                    .any(|f| f.id == FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY),
                "wave topic `{}` must trigger the finding, got {:?}",
                wave_topic,
                findings.iter().map(|f| f.id).collect::<Vec<_>>()
            );
        }
    }

    /// Non-supervisor pipelines MUST NOT be touched by the
    /// new rule. The preset disables supervisor; the
    /// worker hat with default concurrency must pass
    /// silently — the lint is capability-gated.
    #[test]
    fn non_supervisor_preset_is_unaffected() {
        let yaml = r"
event_loop:
  execution_mode: isolated
hats:
  worker:
    triggers:
      - exec.unit.ready
";
        let findings = run(yaml);
        assert!(
            !findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY),
            "non-supervisor pipeline must not trigger the new rule, got {:?}",
            findings.iter().map(|f| f.id).collect::<Vec<_>>()
        );
    }

    /// Hat declares `*.unit.ready` but also a non-wave
    /// trigger. As long as ONE trigger is a wave topic AND
    /// concurrency is low, the lint fires — the wave path
    /// still drops at runtime.
    #[test]
    fn mixed_triggers_wave_topic_is_linted() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  worker:
    triggers:
      - exec.unit.ready
      - work.ready
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY),
            "mixed triggers with a wave topic must still trigger the finding, got {:?}",
            findings.iter().map(|f| f.id).collect::<Vec<_>>()
        );
    }

    /// Multiple wave consumer hats: each one with low
    /// concurrency MUST produce its own finding. The
    /// finding is per-hat so the operator can fix each
    /// worker independently.
    #[test]
    fn multiple_wave_consumers_each_produce_finding() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  worker:
    triggers:
      - exec.unit.ready
  review-batch-worker:
    triggers:
      - review.unit.ready
  fix-worker:
    triggers:
      - fix.unit.ready
";
        let findings = run(yaml);
        let count = findings
            .iter()
            .filter(|f| f.id == FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY)
            .count();
        assert_eq!(
            count,
            3,
            "three wave consumers with low concurrency must produce three findings, got {:?}",
            findings.iter().map(|f| f.id).collect::<Vec<_>>()
        );
    }

    /// A hat that does NOT consume a wave topic (its
    /// trigger is a normal business topic) MUST NOT be
    /// linted — the rule is topic-gated, not "every hat
    /// in a supervisor preset".
    #[test]
    fn non_wave_consumer_hat_is_silent() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  exec-integrator:
    triggers:
      - exec.wave.complete
";
        let findings = run(yaml);
        assert!(
            !findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY),
            "exec-integrator (not a wave consumer) must not trigger the rule, got {:?}",
            findings.iter().map(|f| f.id).collect::<Vec<_>>()
        );
    }

    /// The action_hint on the finding must tell the
    /// operator exactly what to fix (set `concurrency > 1`
    /// on the named hat).
    #[test]
    fn wave_consumer_finding_carries_action_hint() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  worker:
    triggers:
      - exec.unit.ready
";
        let findings = run(yaml);
        let finding = findings
            .iter()
            .find(|f| f.id == FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY)
            .expect("expected the finding");
        let hint = finding
            .action_hint
            .as_deref()
            .expect("action_hint must be set on wave consumer finding");
        assert!(
            hint.contains("concurrency") && hint.contains("worker"),
            "action_hint must name the field and the hat, got `{hint}`"
        );
    }

    /// Supervisor disabled → the rule's trigger never
    /// activates, so a hat that looks like a wave consumer
    /// must pass silently.
    #[test]
    fn supervisor_disabled_skips_wave_concurrency_rule() {
        let yaml = r"
event_loop:
  execution_mode: isolated
  supervisor:
    enabled: false
hats:
  worker:
    triggers:
      - exec.unit.ready
";
        let findings = run(yaml);
        assert!(
            findings.is_empty(),
            "disabled supervisor must short-circuit every supervisor rule, got {:?}",
            findings.iter().map(|f| f.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn coordinator_pseudo_hat_is_exempt_from_publishes_check() {
        // The `coordinator` pseudo-hat commonly declares the
        // system-visible end-points so a fixture can show
        // operators the full topology at a glance. The
        // dispatcher's origin guard (not the lint) enforces
        // the actual gate: agent emits of coord topics are
        // rejected regardless of the hat name.
        let yaml = r"
hats:
  coordinator:
    publishes:
      - exec.wave.complete
";
        let findings = run(yaml);
        assert!(
            !findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC),
            "coordinator is exempt from R-COORD-4, got {findings:?}"
        );
    }

    #[test]
    fn legal_minimum_supervisor_preset_has_no_finding() {
        // The minimum legal fixture: supervisor-enabled,
        // isolated, integrators trigger wave-complete, hats
        // publish normal topics only. Must pass clean.
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  exec-integrator:
    triggers:
      - exec.wave.complete
  fix-integrator:
    triggers:
      - fix.wave.complete
  worker:
    publishes:
      - work.done
";
        let findings = run(yaml);
        assert!(
            findings.is_empty(),
            "legal supervisor preset must pass, got {findings:?}"
        );
    }

    // 2026-07-23-005 plan U2: `task-planner` ownership
    // transfer guards. The lint must flag any preset that
    // re-introduces `exec.unit.ready` in `task-planner`'s
    // `publishes:` or `triggers:` lists (U5 owns that
    // ownership now).
    #[test]
    fn task_planner_publishing_exec_unit_ready_is_error() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  task-planner:
    triggers:
      - work.ready
    publishes:
      - exec.unit.ready
";
        let findings = run(yaml);
        assert!(
            findings.iter().any(|f| f.id == FINDING_SUPERVISOR_TASK_PLANNER_PUBLISHES_EXEC_READY),
            "expected task_planner_publishes_exec_unit_ready finding, got {findings:?}"
        );
    }

    #[test]
    fn task_planner_triggering_exec_unit_ready_is_error() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  task-planner:
    triggers:
      - work.ready
      - exec.unit.ready
    publishes:
      - plan.blocked
";
        let findings = run(yaml);
        assert!(
            findings.iter().any(|f| f.id == FINDING_SUPERVISOR_TASK_PLANNER_TRIGGERS_EXEC_READY),
            "expected task_planner_triggers_exec_unit_ready finding, got {findings:?}"
        );
    }

    #[test]
    fn task_planner_without_exec_unit_ready_is_silent() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  task-planner:
    triggers:
      - work.ready
    publishes:
      - plan.blocked
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .all(|f| f.id != FINDING_SUPERVISOR_TASK_PLANNER_PUBLISHES_EXEC_READY
                    && f.id != FINDING_SUPERVISOR_TASK_PLANNER_TRIGGERS_EXEC_READY),
            "U2 ownership transfer guards must stay silent for a valid task-planner preset; \
             got {findings:?}"
        );
    }

    // 2026-07-23-005 plan U7: alignment is read-only and must
    // not become a second wave dispatcher. The two lint rules
    // catch any preset that re-introduces per-unit readiness
    // topics in alignment's publishes / triggers.
    #[test]
    fn alignment_publishing_unit_ready_is_error() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  alignment:
    triggers:
      - fix.done
    publishes:
      - fix.unit.ready
      - plan.complete
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_ALIGNMENT_PUBLISHES_WAVE_READY),
            "expected alignment_publishes_wave_ready finding, got {findings:?}"
        );
    }

    #[test]
    fn alignment_triggering_unit_ready_is_error() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  alignment:
    triggers:
      - fix.unit.ready
    publishes:
      - plan.complete
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_ALIGNMENT_TRIGGERS_WAVE_READY),
            "expected alignment_triggers_wave_ready finding, got {findings:?}"
        );
    }

    #[test]
    fn alignment_pure_read_only_is_silent() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  alignment:
    triggers:
      - fix.done
    publishes:
      - plan.complete
      - plan.blocked
";
        let findings = run(yaml);
        assert!(
            findings.iter().all(|f| f.id
                != FINDING_SUPERVISOR_ALIGNMENT_PUBLISHES_WAVE_READY
                && f.id != FINDING_SUPERVISOR_ALIGNMENT_TRIGGERS_WAVE_READY),
            "U7 alignment must stay read-only; got {findings:?}"
        );
    }

    // 2026-07-23-005 plan U8: deleted hats must not be
    // reinstated in `hats:` or referenced anywhere else
    // (state-projection / deny rule / etc.).
    #[test]
    fn deleted_progress_steward_in_hats_is_error() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  progress-steward:
    triggers:
      - loop.stalled
    publishes:
      - work.ready
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_DELETED_HAT_REINSTATED),
            "deleted progress-steward must trigger deleted_hat_reinstated; got {findings:?}"
        );
    }

    #[test]
    fn deleted_shipper_referenced_in_state_projection_is_error() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats: {}
state_projection:
  shipper: 'present'
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_DELETED_HAT_REFERENCED),
            "deleted shipper in state_projection must trigger deleted_hat_referenced; \
             got {findings:?}"
        );
    }

    #[test]
    fn deleted_fixer_referenced_in_deny_rules_is_error() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats: {}
topic_deny_rules:
  fixer:
    deny:
      - work.failed
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_SUPERVISOR_DELETED_HAT_REFERENCED),
            "deleted fixer in topic_deny_rules must trigger deleted_hat_referenced; \
             got {findings:?}"
        );
    }

    #[test]
    fn no_deleted_hat_references_is_silent() {
        let yaml = r"
event_loop:
  supervisor:
    enabled: true
  execution_mode: isolated
hats:
  coordinator:
    triggers:
      - plan.ready
    publishes:
      - work.ready
  reporter:
    triggers:
      - plan.complete
      - plan.blocked
    publishes:
      - LOOP_COMPLETE
";
        let findings = run(yaml);
        assert!(
            findings
                .iter()
                .all(|f| f.id != FINDING_SUPERVISOR_DELETED_HAT_REINSTATED
                    && f.id != FINDING_SUPERVISOR_DELETED_HAT_REFERENCED),
            "clean U8 topology must not trigger deleted-hats lints; got {findings:?}"
        );
    }
}
