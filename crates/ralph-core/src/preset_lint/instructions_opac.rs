// OPAC instructions lint rule family (2026-07-04-001 plan U11).
//
// Five rules over the raw preset YAML that catch anti-patterns which cause
// agent misbehavior in isolated mode. The lint is pure-YAML so it survives
// `RalphConfig` refactors; callers (`ralph preset check`, `ralph run -H
// builtin:…`) wire the raw text into `run_preset_lint` via the `raw_yaml`
// parameter.
//
// Rule IDs (all `Error` by default):
//   - `INSTRUCTIONS_TASK_CREATE_LITERAL` — `ralph tools task create` literal
//   - `INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING` — coordinator hat talks
//     about fix-units without citing `task ensure --for-fix-unit`
//   - `INSTRUCTIONS_OPAC_SKILL_REFERENCE_MISSING` — hat publishes non-empty
//     topics but instructions do not cite `ralph-tools-opac` or
//     `ralph-tools-emit` §5 precheck
//   - `INSTRUCTIONS_READ_INTERNAL_LEDGER` — instructions direct the agent
//     to read runtime-private files (`.ralph/events.jsonl`,
//     `supervisor.db`, etc.) or to call `ralph diagnose --supervisor`
//   - `INSTRUCTIONS_SUPERVISOR_COORDINATION_TOPIC` — instructions direct
//     the agent to emit `*.wave.complete` / `*.unit.ready` from agent
//     context (F-019 root cause)

use serde_yaml::Value;

use super::finding_id::{
    FINDING_INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING,
    FINDING_INSTRUCTIONS_OPAC_SKILL_REFERENCE_MISSING,
    FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER,
    FINDING_INSTRUCTIONS_SUPERVISOR_COORDINATION_TOPIC,
    FINDING_INSTRUCTIONS_TASK_CREATE_LITERAL,
};
use super::LintFinding;

/// Supervisor-only coordination topics. Agents emitting these will be
/// silently dropped by `event_origin::is_supervisor_coordination_topic`.
/// **Only** used by `check_supervisor_coordination_emit` below — the
/// emitter判定 (which hat needs to cite `ralph-tools-opac`) is no
/// longer driven by a fixed topic whitelist. Per 2026-07-04-002 plan
/// KTD-10, every hat whose `publishes:` is non-empty is treated as an
/// emitter for OPAC purposes (any JSONL write counts), so the rule is
/// derived from `hat.publishes` rather than a hard-coded list.
const SUPERVISOR_COORD_TOPICS: &[&str] = &[
    "exec.wave.complete",
    "exec.wave.failed",
    "review.wave.complete",
    "review.wave.failed",
    "fix.wave.complete",
    "fix.wave.failed",
];

/// Public entry point. Walks every hat in the preset and emits findings
/// for the OPAC anti-patterns described in this module's header.
pub fn check_instructions_opac(raw_yaml: &str) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let parsed: Value = match serde_yaml::from_str(raw_yaml) {
        Ok(v) => v,
        Err(_) => return findings, // parsing failures are surfaced by other lint modules
    };

    let Some(hats) = parsed.get("hats").and_then(Value::as_mapping) else {
        return findings;
    };

    for (hat_id, hat_value) in hats {
        let Some(hat_id_str) = hat_id.as_str() else { continue };
        let hat_value = match hat_value.as_mapping() {
            Some(m) => m,
            None => continue,
        };

        let instructions = match hat_value
            .get("instructions")
            .and_then(Value::as_str)
            .or_else(|| {
                hat_value
                    .get("instructions")
                    .and_then(Value::as_sequence)
                    .and_then(|s| {
                        s.iter().next().and_then(|v| v.as_str())
                    })
            }) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue, // hat with no instructions is not subject to this lint
        };

        let publishes = hat_publishes(hat_value);

        check_task_create_literal(hat_id_str, &instructions, &mut findings);
        check_internal_ledger_read(hat_id_str, &instructions, &mut findings);
        check_supervisor_coordination_emit(hat_id_str, &instructions, &mut findings);

        // 2026-07-04-002 plan KTD-10: derive emitter判定 from the hat's own
        // `publishes` list. Any non-empty publishes list means the hat can
        // write a JSONL event during its activation, which is what makes it
        // an emitter for OPAC purposes. The previous fixed
        // `EMITTER_TOPICS` whitelist was too narrow — builtin presets
        // ship business topics that were not on that list
        // (e.g. `experiment.measured`, `merge.reviewed`,
        // `hypothesis.confirmed`), so the OPAC skill reference rule
        // silently failed to fire for those emitters.
        if !publishes.is_empty() {
            check_opac_skill_reference(hat_id_str, &instructions, &mut findings);
            let talks_fix_unit = mentions_fix_unit(&instructions);
            if talks_fix_unit {
                check_fix_unit_mint_template(hat_id_str, &instructions, &mut findings);
            }
        }
    }

    findings
}

fn hat_publishes(hat_mapping: &serde_yaml::Mapping) -> Vec<String> {
    hat_mapping
        .get("publishes")
        .and_then(Value::as_sequence)
        .map(|s| {
            s.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn check_task_create_literal(hat_id: &str, instructions: &str, findings: &mut Vec<LintFinding>) {
    // Match the fictional `ralph tools task create` or `ralph task create` shape.
    // We use a small word-boundary regex that catches both literal spellings.
    for needle in ["ralph tools task create", "ralph task create"] {
        if contains_word_boundary(instructions, needle) {
            findings.push(
                LintFinding::new(
                    FINDING_INSTRUCTIONS_TASK_CREATE_LITERAL,
                    format!(
                        "hat `{hat_id}` instructions reference the non-existent command `{needle}`; use `ralph tools task add` or `ralph tools task ensure`"
                    ),
                )
                .with_hat(hat_id),
            );
            return;
        }
    }
}

fn check_internal_ledger_read(
    hat_id: &str,
    instructions: &str,
    findings: &mut Vec<LintFinding>,
) {
    // Patterns cover all three categories described in U11:
    //  1. read/tail events.jsonl
    //  2. read supervisor.db / loops.json
    //  3. call `ralph diagnose --supervisor`
    let needles: &[&str] = &[
        ".ralph/events.jsonl",
        ".ralph/loops.json",
        ".ralph/supervisor.db",
        "ralph diagnose --supervisor",
    ];
    for needle in needles {
        if instructions.contains(needle) {
            findings.push(
                LintFinding::new(
                    FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER,
                    format!(
                        "hat `{hat_id}` instructions reference runtime-private ledger `{needle}`; use `ralph tools task list` or `ralph inspect loop` instead (HARD RULE 4)"
                    ),
                )
                .with_hat(hat_id),
            );
            return;
        }
    }
}

fn check_supervisor_coordination_emit(
    hat_id: &str,
    instructions: &str,
    findings: &mut Vec<LintFinding>,
) {
    // `ralph emit` / `ralph wave emit` followed by a supervisor-only coord
    // topic. We allow mentions of these topics in prose / docs because
    // agents should be aware they exist; the rule fires only when the
    // instruction tells the agent to publish one.
    let emit_patterns = ["ralph emit", "ralph wave emit"];
    for emit in emit_patterns {
        for topic in SUPERVISOR_COORD_TOPICS {
            if instructions.contains(emit) && instructions.contains(topic) {
                // Heuristic: the emit and topic must be close enough to count
                // as a directed instruction. We accept anything within ~80
                // chars on the same line — sufficient to catch `<TOPIC>`
                // placeholders and bare topic names near the emit verb.
                let lower = instructions.to_ascii_lowercase();
                if let Some(emit_pos) = lower.find(emit) {
                    if let Some(topic_pos) = lower.find(topic) {
                        let distance = (emit_pos as isize - topic_pos as isize).abs();
                        if distance <= 80 {
                            findings.push(
                                LintFinding::new(
                                    FINDING_INSTRUCTIONS_SUPERVISOR_COORDINATION_TOPIC,
                                    format!(
                                        "hat `{hat_id}` instructions tell the agent to `{emit} {topic}`; only the supervisor runtime may inject this topic (F-019 root cause)"
                                    ),
                                )
                                .with_hat(hat_id),
                            );
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn check_opac_skill_reference(
    hat_id: &str,
    instructions: &str,
    findings: &mut Vec<LintFinding>,
) {
    let cites_opac = instructions.contains("ralph-tools-opac");
    let cites_emit_precheck = instructions.contains("ralph-tools-emit")
        && (instructions.contains("§5") || instructions.contains("section 5"));
    if !cites_opac && !cites_emit_precheck {
        findings.push(
            LintFinding::new(
                FINDING_INSTRUCTIONS_OPAC_SKILL_REFERENCE_MISSING,
                format!(
                    "hat `{hat_id}` publishes business events but instructions do not cite `ralph-tools-opac` or `ralph-tools-emit §5` precheck; agent cannot reliably reach Observe / Precheck / Confirm"
                ),
            )
            .with_hat(hat_id),
        );
    }
}

fn check_fix_unit_mint_template(
    hat_id: &str,
    instructions: &str,
    findings: &mut Vec<LintFinding>,
) {
    let cites_template = instructions.contains("--for-fix-unit")
        || instructions.contains("for-fix-unit")
        || instructions.contains("task ensure");
    if !cites_template {
        findings.push(
            LintFinding::new(
                FINDING_INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING,
                format!(
                    "hat `{hat_id}` references fix-units but instructions do not cite the `ralph tools task ensure --for-fix-unit` mint template; the canonical template is required for stable step handoff"
                ),
            )
            .with_hat(hat_id),
        );
    }
}

fn mentions_fix_unit(instructions: &str) -> bool {
    let lower = instructions.to_ascii_lowercase();
    lower.contains("fix-unit") || lower.contains("fix unit") || lower.contains("fresh mint")
}

/// Tiny helper: does `haystack` contain `needle` with word/space boundaries
/// on both sides? Avoids the `regex` crate dependency for a single rule.
fn contains_word_boundary(haystack: &str, needle: &str) -> bool {
    if let Some(idx) = haystack.find(needle) {
        let before_ok = idx == 0
            || haystack
                .as_bytes()
                .get(idx - 1)
                .map(|b| b.is_ascii_whitespace() || *b == b'`')
                .unwrap_or(true);
        let after = idx + needle.len();
        let after_ok = after >= haystack.len()
            || haystack
                .as_bytes()
                .get(after)
                .map(|b| b.is_ascii_whitespace() || *b == b'`' || *b == b'\'')
                .unwrap_or(true);
        before_ok && after_ok
    } else {
        false
    }
}

// Extension trait for ergonomics — mirrors what other preset_lint modules do.
trait WithHat {
    fn with_hat(self, hat: &str) -> Self;
}

impl WithHat for LintFinding {
    fn with_hat(mut self, hat: &str) -> Self {
        self.hat = Some(hat.to_string());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset_lint::finding_id::{
        FINDING_INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING,
        FINDING_INSTRUCTIONS_OPAC_SKILL_REFERENCE_MISSING,
        FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER,
        FINDING_INSTRUCTIONS_SUPERVISOR_COORDINATION_TOPIC,
        FINDING_INSTRUCTIONS_TASK_CREATE_LITERAL,
    };

    fn make_preset(extra_hats: &str) -> String {
        format!(
            r#"
hats:
  coordinator:
    subscribes_to: [task.start]
    publishes:
      - work.ready
      - plan.complete
    instructions: |
      Cite ralph-tools-opac for OPAC discipline and ralph-tools-emit §5 precheck.
      {extra_hats}
"#
        )
    }

    #[test]
    fn clean_preset_emits_no_findings() {
        let yaml = make_preset("");
        let findings = check_instructions_opac(&yaml);
        assert!(
            findings.is_empty(),
            "expected no findings, got {findings:?}"
        );
    }

    #[test]
    fn task_create_literal_is_caught() {
        let yaml = make_preset(
            "Use `ralph tools task create` to mint work items.",
        );
        let findings = check_instructions_opac(&yaml);
        assert!(findings.iter().any(|f| f.id
            == FINDING_INSTRUCTIONS_TASK_CREATE_LITERAL));
    }

    #[test]
    fn ralph_task_create_literal_is_caught() {
        let yaml = make_preset("Run `ralph task create` first.");
        let findings = check_instructions_opac(&yaml);
        assert!(findings.iter().any(|f| f.id
            == FINDING_INSTRUCTIONS_TASK_CREATE_LITERAL));
    }

    #[test]
    fn events_jsonl_read_is_caught() {
        let yaml = make_preset("Read tail of .ralph/events.jsonl for audit.");
        let findings = check_instructions_opac(&yaml);
        assert!(findings.iter().any(|f| f.id
            == FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER));
    }

    #[test]
    fn supervisor_db_read_is_caught() {
        let yaml = make_preset("Open .ralph/supervisor.db to inspect waves.");
        let findings = check_instructions_opac(&yaml);
        assert!(findings.iter().any(|f| f.id
            == FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER));
    }

    #[test]
    fn diagnose_supervisor_is_caught() {
        let yaml = make_preset("Run `ralph diagnose --supervisor` for state.");
        let findings = check_instructions_opac(&yaml);
        assert!(findings.iter().any(|f| f.id
            == FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER));
    }

    #[test]
    fn supervisor_coord_emit_is_caught() {
        let yaml = make_preset(
            "Use `ralph emit review.wave.complete --json '{...}'` to close.",
        );
        let findings = check_instructions_opac(&yaml);
        assert!(findings.iter().any(|f| f.id
            == FINDING_INSTRUCTIONS_SUPERVISOR_COORDINATION_TOPIC));
    }

    #[test]
    fn opac_skill_reference_missing_is_caught() {
        // No `ralph-tools-opac` reference; only generic text.
        let yaml = r#"
hats:
  implementer:
    subscribes_to: [work.ready]
    publishes:
      - work.done
    instructions: |
      Do the work and emit work.done at the end.
"#;
        let findings = check_instructions_opac(yaml);
        assert!(findings.iter().any(|f| f.id
            == FINDING_INSTRUCTIONS_OPAC_SKILL_REFERENCE_MISSING));
    }

    #[test]
    fn fix_unit_mint_template_missing_is_caught() {
        let yaml = r#"
hats:
  fixer:
    subscribes_to: [fix.unit.ready]
    publishes:
      - fix.applied
    instructions: |
      When a fix-unit lands, run the verification suite.
      Cite ralph-tools-opac and ralph-tools-emit §5.
"#;
        let findings = check_instructions_opac(yaml);
        assert!(findings.iter().any(|f| f.id
            == FINDING_INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING));
    }

    #[test]
    fn fix_unit_mint_template_present_passes() {
        let yaml = r#"
hats:
  fixer:
    subscribes_to: [fix.unit.ready]
    publishes:
      - fix.applied
    instructions: |
      When a fix-unit lands, call `ralph tools task ensure --for-fix-unit --key ...`.
      Cite ralph-tools-opac and ralph-tools-emit §5.
"#;
        let findings = check_instructions_opac(yaml);
        assert!(
            !findings.iter().any(|f| f.id == FINDING_INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING),
            "fix-unit mint template was cited; should not fire"
        );
    }

    #[test]
    fn hat_without_instructions_is_skipped() {
        let yaml = r#"
hats:
  silent:
    subscribes_to: [work.ready]
    publishes:
      - work.done
"#;
        let findings = check_instructions_opac(yaml);
        assert!(findings.is_empty(), "hat with no instructions: skip silently");
    }
}