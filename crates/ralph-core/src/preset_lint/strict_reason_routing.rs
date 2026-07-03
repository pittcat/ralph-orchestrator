//! 2026-06-30-001 P0-2 (primary-20260630-032648 diagnosis):
//! `strict_reason_routing` lint.
//!
//! The shipper's `plan.blocked` reason routing MUST be a
//! STRICT EXACT MATCH on the canonical reason literal in the
//! whitelist (`loop_stalled_max_iterations`,
//! `steward_escalation`, `review_terminal_drift`). The pre-fix
//! prompt wording — "for recoverable reasons ... with
//! recoverable reason X" — was being interpreted by the agent
//! as a substring / narrative prefix match, which promoted
//! recovery-bucket reasons (e.g. `stall_no_events recovery: ...`)
//! to `pass` even though the underlying recovery reason was
//! outside the whitelist. This is the exact P0-2 failure the
//! diagnosis flags.
//!
//! The lint is a drift-guard: it scans the shipper's prompt
//! body and refuses to load the preset when the marker
//! phrases ("STRICT-MATCH" or "STRICT EXACT MATCH") are
//! missing. The marker is intentionally in CAPS so a casual
//! refactor that re-writes the prompt in lowercase is caught
//! by the lint and surfaces a clear finding before the next
//! run ever sees a `plan.blocked` event.
//!
//! Severity: Error (structural, not stylistic). The lint
//! fires during `run_preset_lint` so the failure surfaces at
//! preset-load time rather than mid-run.

use super::{LintFinding, LintStrictness};
use crate::config::RalphConfig;

/// 2026-06-30-001 P0-2: stable finding ID. Exposed in
/// `finding_id.rs` as `FINDING_STRICT_REASON_ROUTING_MISSING`.
const FINDING_ID: &str = super::finding_id::FINDING_STRICT_REASON_ROUTING_MISSING;

/// The lint targets the `shipper` hat's prompt body. The
/// shipper is the only hat in `ce-executor-serial` (and
/// derivatives) that owns `plan.blocked` reason routing.
const TARGET_HAT: &str = "shipper";

/// Marker phrases the shipper prompt must contain at least
/// one of. CAPS is intentional so a casual lowercase rewrite
/// is caught. Both markers are accepted as equivalent.
const REQUIRED_MARKERS: &[&str] = &["STRICT-MATCH", "STRICT EXACT MATCH"];

/// Run the P0-2 lint against the shipper hat's prompt body.
///
/// P1-2 (after code-review): the lint prefers the raw YAML
/// text the caller supplied so it can scan the shipper
/// prompt in its YAML-original form. The `RalphConfig`
/// round-trip drops fields the typed config does not model
/// (e.g. `system_prompt_template`, anchor-based fragments
/// injected at render time, prompts that span multiple
/// YAML fields). When the raw text is not available
/// (synthesised configs, unit-test harnesses), the lint
/// falls back to scanning `instructions + extra_instructions`
/// so the existing test coverage keeps working.
pub fn check_strict_reason_routing(
    config: &RalphConfig,
    _strictness: LintStrictness,
    raw_yaml: Option<&str>,
) -> Vec<LintFinding> {
    // Prefer scanning the raw YAML so we cover any prompt
    // shape the typed config might have dropped.
    if let Some(yaml) = raw_yaml {
        if let Some(shipper_text) = extract_shipper_text_from_yaml(yaml) {
            if REQUIRED_MARKERS.iter().any(|m| shipper_text.contains(m)) {
                return Vec::new();
            }
            return vec![missing_marker_finding(shipper_text.is_empty())];
        }
        // No shipper node in the YAML at all → the preset
        // does not have a shipper. Nothing to check.
        return Vec::new();
    }
    // Fallback: scan the typed `instructions +
    // extra_instructions` of the shipper hat.
    let Some(hat_cfg) = config.hats.get(TARGET_HAT) else {
        return Vec::new();
    };
    let mut body = hat_cfg.instructions.clone();
    for extra in &hat_cfg.extra_instructions {
        body.push('\n');
        body.push_str(extra);
    }
    if body.is_empty() {
        return Vec::new();
    }
    if REQUIRED_MARKERS.iter().any(|m| body.contains(m)) {
        return Vec::new();
    }
    vec![missing_marker_finding(false)]
}

/// 2026-06-30-001 P1-2: scan the raw YAML text for the
/// `shipper` hat node and concatenate the relevant prompt
/// fields (`instructions` + `extra_instructions` list).
/// The extraction is intentionally a light-weight line
/// scanner — not a full YAML parser — so we do not pull a
/// new dependency. The scan walks lines, tracks indentation
/// depth, and assembles the shipper's prompt body. The
/// returned text is the rendered concatenation, mirroring
/// what the runtime will eventually feed to the hat.
///
/// `None` when the YAML does not carry a `shipper` hat
/// node (custom presets without shipper) — those are
/// exempt from the lint.
fn extract_shipper_text_from_yaml(yaml: &str) -> Option<String> {
    // Find the `shipper:` line. The hat can appear at any
    // indentation depth under `hats:`. We track depth
    // relative to the `shipper:` line to know when the
    // block ends.
    //
    // P1-2b: reject false positives such as
    // `shipper: [REVIEW_COMPLETE]` under `mechanism.flow`
    // `allowed_emits`. A real hat definition is followed by
    // child keys (name, triggers, publishes, instructions, ...)
    // at greater indentation; an inline map/list value is
    // followed by a sibling at the same indentation.
    let mut lines = yaml.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("shipper:") {
            continue;
        }
        // Compute the indentation of the `shipper:` line.
        let indent = line.len() - trimmed.len();

        // Skip false-positive `shipper:` map values that are
        // not hat definitions.
        let mut is_hat_block = false;
        while let Some(next) = lines.peek() {
            if next.is_empty() {
                let _ = lines.next();
                continue;
            }
            let next_indent = next.len() - next.trim_start().len();
            is_hat_block = next_indent > indent;
            break;
        }
        if !is_hat_block {
            continue;
        }

        let mut body = String::new();
        // Track the most recent key we saw so multi-line
        // `instructions: |` block bodies are folded into
        // the right field.
        let mut last_key: Option<&'static str> = None;
        for next in lines {
            if next.is_empty() {
                // Blank line: still part of the current
                // block (multi-line YAML literal). Skip
                // without resetting `last_key`.
                continue;
            }
            let next_indent = next.len() - next.trim_start().len();
            if next_indent <= indent {
                break;
            }
            let t = next.trim_start();
            // A sibling key resets the block tracker.
            if !t.starts_with("- ") {
                if t.starts_with("instructions:") {
                    last_key = Some("instructions");
                    // Inline value (rare in production
                    // presets but supported): take the
                    // remainder of the line.
                    let rest = t["instructions:".len()..].trim();
                    if !rest.is_empty() {
                        body.push_str(rest);
                        body.push('\n');
                    }
                    continue;
                } else if t.starts_with("extra_instructions:") {
                    last_key = Some("extra_instructions");
                    let v = t["extra_instructions:".len()..].trim();
                    body.push_str(v);
                    body.push('\n');
                    continue;
                } else if last_key == Some("instructions") {
                    // Continuation of a multi-line literal
                    // block (YAML `|` / `>` folded). The
                    // next-indent check already filtered
                    // siblings; lines that are deeper than
                    // `shipper:` and not a list item are
                    // part of the `instructions` block.
                    body.push_str(t);
                    body.push('\n');
                    continue;
                }
            } else if t.starts_with("- ") {
                // List item under shipper (could be an
                // extra_instructions list, an alias, a
                // tag). Concatenate the value.
                body.push_str(&t[2..]);
                body.push('\n');
            }
        }
        return Some(body);
    }
    None
}

fn missing_marker_finding(_is_empty: bool) -> LintFinding {
    LintFinding::error(
        FINDING_ID,
        format!(
            "{TARGET_HAT} prompt is missing STRICT-MATCH marker; \
             plan.blocked reason routing must use STRICT EXACT MATCH on the canonical \
             whitelist literal (loop_stalled_max_iterations, steward_escalation, \
             review_terminal_drift). The pre-fix wording let recovery-bucket reasons \
             (e.g. 'stall_no_events recovery: ...') promote to pass via substring match. \
             See plan 2026-06-30-001 U4."
        ),
    )
    .with_hat(TARGET_HAT.to_string())
    .with_action_hint(format!(
        "Add a STRICT-MATCH (or 'STRICT EXACT MATCH') marker to the {TARGET_HAT} \
         'plan.blocked' reason-routing paragraph; document that the comparison is \
         'reason == whitelist_entry' after trim+lowercase, with no substring / \
         'starts with' fallback."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HatConfig, RalphConfig};
    use std::collections::HashMap;

    fn cfg_with_shipper_prompt(prompt: Option<&str>, extra: Vec<&str>) -> RalphConfig {
        let mut hats = HashMap::new();
        hats.insert(
            TARGET_HAT.to_string(),
            HatConfig {
                instructions: prompt.unwrap_or("").to_string(),
                extra_instructions: extra.into_iter().map(String::from).collect(),
                ..HatConfig::default()
            },
        );
        RalphConfig {
            hats,
            ..RalphConfig::default()
        }
    }

    #[test]
    fn no_shipper_passes() {
        let cfg = RalphConfig {
            hats: HashMap::new(),
            ..RalphConfig::default()
        };
        assert!(check_strict_reason_routing(&cfg, LintStrictness::Default, None).is_empty());
    }

    #[test]
    fn no_prompt_passes() {
        let cfg = cfg_with_shipper_prompt(None, vec![]);
        assert!(check_strict_reason_routing(&cfg, LintStrictness::Default, None).is_empty());
    }

    #[test]
    fn prompt_with_strict_match_passes() {
        let cfg = cfg_with_shipper_prompt(
            Some("On plan.blocked: use STRICT-MATCH on the canonical whitelist."),
            vec![],
        );
        assert!(check_strict_reason_routing(&cfg, LintStrictness::Default, None).is_empty());
    }

    #[test]
    fn prompt_with_strict_match_in_extra_passes() {
        let cfg = cfg_with_shipper_prompt(
            Some("Some preamble."),
            vec!["On plan.blocked: STRICT EXACT MATCH after trim+lowercase."],
        );
        assert!(check_strict_reason_routing(&cfg, LintStrictness::Default, None).is_empty());
    }

    #[test]
    fn prompt_without_marker_fails() {
        let cfg = cfg_with_shipper_prompt(
            Some("On plan.blocked: for recoverable reasons, run checks 1-2."),
            vec![],
        );
        let findings = check_strict_reason_routing(&cfg, LintStrictness::Default, None);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_ID);
        assert!(findings[0].message.contains("STRICT"));
    }

    #[test]
    fn lowercase_marker_does_not_satisfy_lint() {
        // CAPS is intentional; lowercase rewrites must be
        // caught so a casual refactor does not silently
        // remove the strict-match contract.
        let cfg = cfg_with_shipper_prompt(Some("strict-match on whitelist"), vec![]);
        let findings = check_strict_reason_routing(&cfg, LintStrictness::Default, None);
        assert_eq!(findings.len(), 1);
    }

    // P1-2: raw_yaml path — the lint prefers raw text and
    // covers preset shapes the typed config cannot model.
    #[test]
    fn raw_yaml_with_strict_match_passes() {
        let yaml = r#"
hats:
  shipper:
    instructions: |
      On plan.blocked: use STRICT-MATCH on the canonical whitelist.
"#;
        let cfg = cfg_with_shipper_prompt(None, vec![]);
        assert!(check_strict_reason_routing(&cfg, LintStrictness::Default, Some(yaml)).is_empty());
    }

    #[test]
    fn raw_yaml_without_marker_fails_even_if_typed_config_has_marker() {
        // Operator edited the YAML but forgot to add the
        // marker; the typed config the lint is given
        // through `instructions` does not yet have the
        // pre-edit prompt. The raw_yaml path catches the
        // drift.
        let yaml = r#"
hats:
  shipper:
    instructions: |
      On plan.blocked: for recoverable reasons, run checks.
"#;
        let cfg = cfg_with_shipper_prompt(
            Some("STRICT-MATCH on whitelist"), // typed config has marker
            vec![],
        );
        let findings = check_strict_reason_routing(&cfg, LintStrictness::Default, Some(yaml));
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn raw_yaml_extra_instructions_list_picked_up() {
        let yaml = r#"
hats:
  shipper:
    instructions: |
      Some preamble.
    extra_instructions:
      - "On plan.blocked: STRICT EXACT MATCH after trim+lowercase."
"#;
        let cfg = cfg_with_shipper_prompt(None, vec![]);
        assert!(check_strict_reason_routing(&cfg, LintStrictness::Default, Some(yaml)).is_empty());
    }

    #[test]
    fn raw_yaml_skips_inline_shipper_map_value_before_real_hat() {
        // ce-executor-serial has `shipper: [REVIEW_COMPLETE]` under
        // mechanism.flow.allowed_emits before the actual `shipper:` hat
        // definition. The linter must not stop at the inline map value and
        // must still find the marker in the real shipper instructions.
        let yaml = r#"
mechanism:
  flow:
    steps:
      - id: ship
        allowed_emits:
          shipper: [REVIEW_COMPLETE]
          reporter: [report.done, LOOP_COMPLETE]
hats:
  shipper:
    instructions: |
      On plan.blocked: use STRICT-MATCH on the canonical whitelist.
"#;
        let cfg = cfg_with_shipper_prompt(None, vec![]);
        assert!(check_strict_reason_routing(&cfg, LintStrictness::Default, Some(yaml)).is_empty());
    }
}
