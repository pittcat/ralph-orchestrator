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
    FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC, FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE,
    FINDING_SUPERVISOR_REQUIRES_ISOLATED, FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY,
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
}
