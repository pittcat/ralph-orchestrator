//! 2026-07-02-006 plan U3: `mechanism.phase_authority` lint rules.
//!
//! Pure YAML-string entry point (no `RalphConfig` parsing):
//! `check_phase_authority_block(&str) -> Vec<LintFinding>`.
//!
//! The lint catches three classes of drift that U2's typed
//! parser does not see:
//!
//! 1. **Unknown primitives** — a transition references a
//!    `primitive: <name>` whose id is not on the engine's
//!    whitelist. U8/U9 register the canonical primitives
//!    (`on_event`, `on_test_passed_step`,
//!    `on_review_complete_verdict`, `on_loop_complete_honored`,
//!    `on_plan_terminal_accepted`); anything else is a future
//!    engine addition that has not landed.
//! 2. **Pipeline-style preset declares `phase_authority`** —
//!    per KTD1, the hat-only `ce-executor-pipeline` preset must
//!    NOT enable the engine. The lint surfaces a finding even
//!    when the rest of the YAML is otherwise valid.
//! 3. **Empty `phase_authority` block** — `enabled: true` with
//!    no `phases` and no `transitions` is meaningless: the
//!    engine never receives a phase id to track.
//!
//! The lint deliberately does **not** re-validate duplicate
//! phase ids or dangling transitions — U2's typed parser
//! already produces those errors and is the canonical
//! authority. The lint focuses on shapes the typed view cannot
//! see (primitive id whitelist, preset-name interlock).

use crate::event_loop::phase_authority::config::PhaseAuthorityConfig;
use crate::preset_lint::LintFinding;
use crate::preset_lint::finding_id::{
    FINDING_PHASE_AUTHORITY_EMPTY, FINDING_PHASE_AUTHORITY_PIPELINE_NOT_ALLOWED,
    FINDING_PHASE_AUTHORITY_UNKNOWN_PRIMITIVE,
};

/// Engine-known primitives (U6–U9 + the `on_plan_terminal_accepted`
/// placeholder for U13). New primitives must be added here **and**
/// to `event_loop::phase_authority::primitives` in lockstep, or
/// the lint will fire for the new primitive on every preset.
pub const KNOWN_PRIMITIVES: &[&str] = &[
    "on_event",
    "on_test_passed_step",
    "on_review_complete_verdict",
    "on_plan_terminal_accepted",
    "on_loop_complete_honored",
];

/// Run every `mechanism.phase_authority` rule against the raw
/// preset YAML.
///
/// `raw_yaml` is the full preset body. The lint reads
/// `mechanism.phase_authority` directly so unknown top-level
/// fields (e.g. `hats`, `event_loop`, `event_policy`) survive
/// the round-trip.
pub fn check_phase_authority_block(raw_yaml: &str) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    let value: serde_yaml::Value = match serde_yaml::from_str(raw_yaml) {
        Ok(v) => v,
        Err(_) => {
            // The preset is broken at the YAML level; other lint
            // rules will report the parse error. The phase
            // authority check is a no-op in that case.
            return findings;
        }
    };

    // Pipeline-style presets must NOT declare phase_authority.
    // We detect them by the presence of a `hats:` top-level
    // list AND the absence of a coordinator role — i.e. the
    // hat-only topology. The check is best-effort: a preset
    // that intentionally mixes both hats and a coordinator will
    // fail this guard, which is the desired behaviour (the
    // operator must then split it into two presets).
    let is_pipeline_style = is_hat_only_pipeline(&value);
    if is_pipeline_style && has_phase_authority_block(&value) {
        let mut f = LintFinding::new(
            FINDING_PHASE_AUTHORITY_PIPELINE_NOT_ALLOWED,
            "preset declares `mechanism.phase_authority` but appears to be a hat-only pipeline \
             (no coordinator role). The phase engine is for presets that opt in to multi-phase \
             coordination; hat-only pipelines must keep `phase_authority` absent or `enabled: false`",
        );
        f.action_hint = Some(
            "remove the `phase_authority` block, set `phase_authority.enabled: false`, or \
             add a coordinator role to declare this preset as phase-driven"
                .to_string(),
        );
        findings.push(f);
        return findings;
    }

    if !has_phase_authority_block(&value) {
        return findings;
    }

    let Some(phase_block) = value
        .get("mechanism")
        .and_then(|m| m.get("phase_authority"))
    else {
        return findings;
    };

    let cfg: PhaseAuthorityConfig = match serde_yaml::from_value(phase_block.clone()) {
        Ok(c) => c,
        Err(err) => {
            // U2 will reject the same shape at runtime; the
            // lint records the typed error so the operator
            // gets a unified report at preset-load time.
            let mut f = LintFinding::new(
                FINDING_PHASE_AUTHORITY_EMPTY,
                format!(
                    "phase_authority block does not match the typed PhaseAuthorityConfig: {err}"
                ),
            );
            f.action_hint = Some(
                "compare the YAML against `crates/ralph-core/src/event_loop/phase_authority/config.rs` \
                 (U1) — `enabled`, `initial_phase`, `phases[]`, `transitions[]`, \
                 `violation_policy`, `progress_projection`"
                    .to_string(),
            );
            findings.push(f);
            return findings;
        }
    };

    if !cfg.enabled {
        return findings;
    }

    if cfg.phases.is_empty() {
        let mut f = LintFinding::new(
            FINDING_PHASE_AUTHORITY_EMPTY,
            "phase_authority is enabled but declares no phases; the engine cannot track a phase id",
        );
        f.action_hint =
            Some("add at least one phase under `mechanism.phase_authority.phases[]`".to_string());
        findings.push(f);
    }

    if cfg.transitions.is_empty() {
        let mut f = LintFinding::new(
            FINDING_PHASE_AUTHORITY_EMPTY,
            "phase_authority is enabled but declares no transitions; the loop cannot advance",
        );
        f.action_hint = Some(
            "add at least one transition under `mechanism.phase_authority.transitions[]`"
                .to_string(),
        );
        findings.push(f);
    }

    for tr in &cfg.transitions {
        let Some(primitive) = tr.on.0.as_mapping().and_then(|m| {
            m.get(&serde_yaml::Value::String("primitive".to_string()))
                .and_then(|v| v.as_str())
        }) else {
            continue;
        };
        if !KNOWN_PRIMITIVES.contains(&primitive) {
            let mut f = LintFinding::new(
                FINDING_PHASE_AUTHORITY_UNKNOWN_PRIMITIVE,
                format!(
                    "transition `{} -> {}` references unknown primitive `{}`",
                    tr.from, tr.to, primitive
                ),
            );
            f.action_hint = Some(format!(
                "use one of the engine-known primitives: {:?}; new primitives require an engine change in `event_loop::phase_authority::primitives`",
                KNOWN_PRIMITIVES
            ));
            findings.push(f);
        }
    }

    findings
}

fn has_phase_authority_block(value: &serde_yaml::Value) -> bool {
    value
        .get("mechanism")
        .and_then(|m| m.get("phase_authority"))
        .is_some()
}

/// Best-effort detection of a hat-only pipeline preset. The
/// signal is the absence of any `tasks.coordinator_hats` block
/// plus the presence of a flat `hats:` list. The function does
/// not inspect the hat ids — the lint is structural only.
fn is_hat_only_pipeline(value: &serde_yaml::Value) -> bool {
    let has_hats = value.get("hats").map(|h| h.is_mapping()).unwrap_or(false);
    if !has_hats {
        return false;
    }
    // Coordinator-driven presets expose `tasks.coordinator_hats`
    // (or `event_loop.tasks.coordinator_hats`). A pipeline
    // preset does not.
    let has_coordinator = value
        .get("tasks")
        .and_then(|t| t.get("coordinator_hats"))
        .is_some()
        || value
            .get("event_loop")
            .and_then(|el| el.get("tasks"))
            .and_then(|t| t.get("coordinator_hats"))
            .is_some();
    !has_coordinator
}