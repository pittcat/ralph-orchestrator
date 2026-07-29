//! 2026-07-29-002 plan U: precheck gate hat coverage check.
//!
//! Invariant asserted on the caller's config AS-IS: when
//! `event_loop.precheck.rules.<X>` is declared with `enabled: true`
//! and the config shows evidence that `normalize()`'s
//! `apply_precheck_desugar` already rewrote producers to emit
//! `<X>.proposed`, the hat map must contain the synthesized
//! `precheck-<X>` gate hat. A missing gate hat in that state means
//! `<X>.proposed` events have no consumer — the fail-shaped evidence
//! audit and retry budget silently vanish.
//!
//! This lint deliberately does NOT re-run `normalize()` on a clone:
//! `apply_precheck_desugar` synthesizes the gate hat unconditionally
//! for every declared rule, so a lint that normalizes first can never
//! observe the gate hat missing (tautology). Checking the caller's
//! hat map directly keeps the finding reachable for genuinely
//! half-desugared configs (hand-built configs, or a future desugar
//! regression that drops the synthesized hat).
//!
//! Equally deliberate: a config whose producers still publish the
//! bare `<X>` (raw preset YAML before `normalize`, e.g. the embedded
//! presets fed to `test_all_embedded_presets_pass_strict_lint` via
//! `RalphConfig::parse_yaml`, which does not normalize) is silent —
//! `normalize()` will synthesize the gate hat at load time.
//!
//! Boundary this lint does NOT cover: when the merge layer
//! (`ralph-cli` `merge_hats_overlay`) strips the whole
//! `event_loop.precheck` block, no declared rules survive and this
//! lint is silent by construction. That regression class is pinned by
//! the `merge_hats_overlay_preserves_precheck_when_operator_omits_it`
//! integration test in `ralph-cli/src/preflight.rs`.
//!
//! The kill switch `precheck_runtime_enabled()` is honored so users
//! running with `RALPH_PRECHECK_MODE=off` (or tests that inject the
//! test kill switch) are not falsely flagged.
//!
//! Severity contract:
//! - Default lint mode: `Warn` (non-blocking).
//! - Strict lint mode:  `Error` (block `ralph run --strict` startup).

use crate::config::RalphConfig;
use crate::config::precheck_runtime_enabled;
use crate::preset_lint::finding_id::FINDING_PRECHECK_RULE_WITHOUT_SYNTHESIZED_GATE_HAT;
use crate::preset_lint::{LintFinding, LintSeverity, LintStrictness};

/// Verify that every `event_loop.precheck.rules.<X>` declared in the
/// caller's config is paired with a `precheck-<X>` gate hat whenever
/// the desugar's `<X>.proposed` rewrite is already in circulation.
///
/// The caller's `&RalphConfig` is inspected as-is and never mutated.
pub fn check_precheck_rule_without_synthesized_gate_hat(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    // Honor the runtime kill switch (plan 2026-07-02-004 milestone A U2):
    // when precheck is forced off, the rules table is intentionally
    // inert; this lint must not flag the contract drift.
    if !precheck_runtime_enabled() {
        return Vec::new();
    }

    let declared_topics: Vec<String> = match config.event_loop.precheck.as_ref() {
        Some(p) if p.enabled && !p.rules.is_empty() => p.rules.keys().cloned().collect(),
        // `precheck: None`, `enabled: false`, or empty rules table —
        // legitimate framework-default states; the lint is silent.
        _ => return Vec::new(),
    };

    let severity = match strictness {
        LintStrictness::Default => LintSeverity::Warn,
        LintStrictness::Strict => LintSeverity::Error,
    };

    let mut findings: Vec<LintFinding> = declared_topics
        .into_iter()
        .filter(|topic| !topic.trim().is_empty())
        .filter(|topic| {
            let gate_hat_id = format!("precheck-{topic}");
            if config.hats.contains_key(&gate_hat_id) {
                // Gate hat present: desugar completed (or the preset
                // hand-wrote the gate). Healthy.
                return false;
            }
            // Fire only on the half-desugared shape: `<X>.proposed`
            // is already in circulation (evidence the producer rewrite
            // ran) but the gate hat is missing. A config whose
            // producers still reference only the bare `<X>` is simply
            // pre-normalize; `normalize()` will synthesize the gate.
            config_references_proposed(config, topic)
        })
        .map(|topic| {
            let gate_hat_id = format!("precheck-{topic}");
            let mut finding = LintFinding::new(
                FINDING_PRECHECK_RULE_WITHOUT_SYNTHESIZED_GATE_HAT,
                format!(
                    "event_loop.precheck.rules[\"{topic}\"] is declared with enabled=true \
                     and producers already emit \"{topic}.proposed\", but the effective \
                     config has no gate hat \"{gate_hat_id}\". Without it, the proposed \
                     event has no consumer: the fail-shaped evidence audit and retry \
                     budget silently vanish. Run `normalize()` (standard load paths do \
                     this) or restore the synthesized gate hat; if the whole \
                     `event_loop.precheck` block was stripped by the merge layer, see \
                     `merge_hats_overlay_preserves_precheck_when_operator_omits_it` in \
                     ralph-cli/src/preflight.rs.",
                ),
            )
            .with_topic(&topic)
            .with_hat(&gate_hat_id);
            finding.severity = severity;
            finding
        })
        .collect();

    findings.sort_by(|a, b| {
        a.id.cmp(b.id)
            .then(a.topic.cmp(&b.topic))
            .then(a.hat.cmp(&b.hat))
    });

    findings
}

/// Whether any hat in the config references `<topic>.proposed` in its
/// `publishes`, `terminal_events`, `triggers`, or `default_publishes`
/// — evidence that `apply_precheck_desugar`'s producer rewrite has
/// already run for this topic.
fn config_references_proposed(config: &RalphConfig, topic: &str) -> bool {
    let proposed = format!("{topic}.proposed");
    config.hats.values().any(|hat| {
        hat.publishes.iter().any(|p| p == &proposed)
            || hat.terminal_events.iter().any(|t| t == &proposed)
            || hat.triggers.iter().any(|t| t == &proposed)
            || hat.default_publishes.as_deref() == Some(proposed.as_str())
    })
}
