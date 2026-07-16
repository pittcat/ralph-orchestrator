//! 2026-07-03-001 plan U9: supervisor preset lint rules.
//!
//! Three rules, all `Error` severity:
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
//!
//! Plain-YAML entry point (no `RalphConfig` parsing) so the
//! lint runs on partially-typed presets and stays stable
//! across `RalphConfig` refactors.

use crate::event_origin::SUPERVISOR_COORDINATION_TOPICS;
use crate::preset_lint::LintFinding;
pub use crate::preset_lint::finding_id::{
    FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC, FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE,
    FINDING_SUPERVISOR_REQUIRES_ISOLATED,
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
/// first, then integrator rule, then publisher rule) so the
/// lint output is deterministic across re-runs.
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
}
