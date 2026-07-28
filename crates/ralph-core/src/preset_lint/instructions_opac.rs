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

use super::LintFinding;
use super::finding_id::{
    FINDING_INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING,
    FINDING_INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING,
    FINDING_INSTRUCTIONS_OPAC_SKILL_REFERENCE_MISSING, FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER,
    FINDING_INSTRUCTIONS_SUPERVISOR_COORDINATION_TOPIC, FINDING_INSTRUCTIONS_TASK_CREATE_LITERAL,
    FINDING_INSTRUCTIONS_TASK_MUTATION_AUTHORITY_CONFLICT,
};

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
    check_instructions_opac_with_preset(raw_yaml, "")
}

/// Preset-aware variant: pass the resolved preset name (e.g.
/// `ce-executor-pipeline-loop`) so the U7 emit-feedback rule
/// can gate on `U7_EMIT_FEEDBACK_LINT_PRESET_WHITELIST`.
/// When the name is empty, the rule stays silent (matches
/// the pre-U7 behaviour).
pub fn check_instructions_opac_with_preset(raw_yaml: &str, preset_name: &str) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let parsed: Value = match serde_yaml::from_str(raw_yaml) {
        Ok(v) => v,
        Err(_) => return findings, // parsing failures are surfaced by other lint modules
    };

    let Some(hats) = parsed.get("hats").and_then(Value::as_mapping) else {
        return findings;
    };

    let projection_owned = parsed
        .get("event_loop")
        .and_then(|value| value.get("state_projection"))
        .and_then(|value| value.get("actions"))
        .is_some_and(|actions| actions.as_mapping().is_some_and(|map| !map.is_empty()));
    let coordinator_hats = parsed
        .get("tasks")
        .and_then(|value| value.get("coordinator_hats"))
        .and_then(Value::as_sequence)
        .map(|hats| {
            hats.iter()
                .filter_map(Value::as_str)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    let lint_active = U7_EMIT_FEEDBACK_LINT_PRESET_WHITELIST.contains(&preset_name);

    for (hat_id, hat_value) in hats {
        let Some(hat_id_str) = hat_id.as_str() else {
            continue;
        };
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
                    .and_then(|s| s.iter().next().and_then(|v| v.as_str()))
            }) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => String::new(),
        };
        let extra_instructions = hat_value
            .get("extra_instructions")
            .and_then(Value::as_sequence)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let instructions = if extra_instructions.is_empty() {
            instructions
        } else {
            format!("{instructions}\n{extra_instructions}")
        };
        if instructions.is_empty() {
            continue;
        }

        let publishes = hat_publishes(hat_value);

        check_task_create_literal(hat_id_str, &instructions, &mut findings);
        check_task_mutation_authority(
            hat_id_str,
            &instructions,
            projection_owned,
            coordinator_hats.contains(hat_id_str),
            &mut findings,
        );
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
            // 2026-07-09-001 plan (U7): gate the new
            // emit-feedback-skill-reference rule on the same
            // "talks about payload shape" heuristic the rule
            // checks. Hats that just emit a single
            // `work.done`-style "publish at the end" without
            // shaping the payload (e.g. some builtin observers)
            // are exempt — the lint targets emitter hats that
            // *describe* the payload to the agent. The
            // additional preset-name gate scopes the lint to
            // the U7 whitelist so we do not bootstrap a
            // failing-everywhere cliff.
            if lint_active && mentions_payload_construction(&instructions) {
                check_emit_feedback_skill_reference(
                    hat_id_str,
                    &instructions,
                    &mut findings,
                    preset_name,
                );
            }
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

fn check_task_mutation_authority(
    hat_id: &str,
    instructions: &str,
    projection_owned: bool,
    is_coordinator: bool,
    findings: &mut Vec<LintFinding>,
) {
    let lower = instructions.to_ascii_lowercase();
    let command = [
        "ralph tools task add",
        "ralph task add",
        "ralph tools task ensure",
        "ralph task ensure",
    ]
    .iter()
    .find(|needle| lower.contains(**needle));
    let Some(command) = command else {
        return;
    };
    // Read-only task subcommands and the fix-unit mint template never
    // mutate the ledger through this code path — exempt them by command
    // shape (not by negation heuristic). A hat that pairs the mutation
    // command with a separate "do not"-style disclaimer still fires the
    // lint; the agent runtime's authority model does not care what the
    // instructions author intended to say.
    let read_only_call = ["ralph tools task list", "ralph tools task show", "ralph tools task verify"]
        .iter()
        .any(|needle| lower.contains(needle));
    if read_only_call {
        return;
    }
    let legal_fix_unit_template =
        lower.contains("--for-fix-unit") && lower.contains("ralph tools task ensure");
    if legal_fix_unit_template {
        return;
    }
    let reason = if projection_owned {
        "projector_single_writer_conflict"
    } else if !is_coordinator {
        "non_coordinator_task_mutation"
    } else {
        return;
    };
    findings.push(
        LintFinding::new(
            FINDING_INSTRUCTIONS_TASK_MUTATION_AUTHORITY_CONFLICT,
            format!(
                "hat `{hat_id}` requires `{command}` ({reason}); task creation must use the configured projector or an authorized coordinator"
            ),
        )
        .with_hat(hat_id)
        .with_action_hint("Remove agent-side task mutation and emit the declarative task handoff; read live task IDs through the task API."),
    );
}

fn check_internal_ledger_read(hat_id: &str, instructions: &str, findings: &mut Vec<LintFinding>) {
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
                if let Some(emit_pos) = lower.find(emit)
                    && let Some(topic_pos) = lower.find(topic)
                {
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

fn check_opac_skill_reference(hat_id: &str, instructions: &str, findings: &mut Vec<LintFinding>) {
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

fn check_fix_unit_mint_template(hat_id: &str, instructions: &str, findings: &mut Vec<LintFinding>) {
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

/// 2026-07-09-001 plan (U7): returns true when the hat
/// `instructions` describe payload construction (so the
/// emit-feedback lint should run). The heuristic looks for
/// common payload-shaping verbs / nouns. We deliberately
/// match the same words the agent sees in the schema-aware
/// prompt section, so the rule's gate is *visible* to
/// preset authors.
fn mentions_payload_construction(instructions: &str) -> bool {
    let lower = instructions.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "payload",
        "ralph emit",
        "ralph wave emit",
        "field shape",
        "required fields",
        "field_docs",
        "field description",
        "schema-aware",
        "policy-check",
    ];
    NEEDLES.iter().any(|n| lower.contains(n))
}

/// 2026-07-09-001 plan (U7): the rule is intentionally
/// scoped to a short whitelist of "high-risk" preset names.
/// The lint's first iteration only enforces on presets that
/// the U8 pilot covers (`ce-executor-pipeline-loop`). Adding
/// a new preset to the whitelist is the documented way to
/// widen the rule; adding a new finding to a preset that is
/// not on the list is a no-op. This matches the plan's
/// "Lint 第一版只针对 builtin/high-risk" intent and avoids a
/// bootstrap cliff where every existing preset would fail
/// the new check at once.
const U7_EMIT_FEEDBACK_LINT_PRESET_WHITELIST: &[&str] = &["ce-executor-pipeline-loop"];

fn check_emit_feedback_skill_reference(
    hat_id: &str,
    instructions: &str,
    findings: &mut Vec<LintFinding>,
    _preset_name: &str,
) {
    // 2026-07-09-001 plan (U7): the rule accepts any of the
    // following references as proof the agent is pointed at
    // the new policy-check feedback section:
    //   - `ralph-tools-emit` + `policy-check feedback`
    //   - `ralph-tools-emit` + `enrichment fields`
    //   - `ralph-tools-emit` + `suggested_payload_shape`
    //   - `ralph-tools-emit` + `field_description`
    //   - `ralph-tools-emit` + `suggested_command`
    //
    // The mention does NOT need a §N anchor — the U7 plan
    // scopes the rule to builtin / high-risk presets, and
    // builtin preset authors are expected to read the
    // ralph-tools-emit skill end-to-end.
    let cites_emit_feedback = instructions.contains("ralph-tools-emit")
        && [
            "policy-check feedback",
            "enrichment fields",
            "suggested_payload_shape",
            "field_description",
            "suggested_command",
        ]
        .iter()
        .any(|n| instructions.contains(n));
    if !cites_emit_feedback {
        findings.push(
            LintFinding::new(
                FINDING_INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING,
                format!(
                    "hat `{hat_id}` instructions describe payload construction but do not cite the U3 `ralph-tools-emit` policy-check feedback section (field_description / suggested_payload_shape / suggested_command); the agent will re-derive field shapes from stale inline text instead of the schema-aware layer"
                ),
            )
            .with_hat(hat_id),
        );
    }
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
// (Removed in U3: an inherent `LintFinding::with_hat` already exists in
// `preset_lint/mod.rs`, so this trait+impl was shadowed and never imported
// by any caller. U3 deletes it as dead code.)

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
            r"
hats:
  coordinator:
    subscribes_to: [task.start]
    publishes:
      - work.ready
      - plan.complete
    instructions: |
      Cite ralph-tools-opac for OPAC discipline and ralph-tools-emit §5 precheck.
      {extra_hats}
"
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
        let yaml = make_preset("Use `ralph tools task create` to mint work items.");
        let findings = check_instructions_opac(&yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_TASK_CREATE_LITERAL)
        );
    }

    #[test]
    fn ralph_task_create_literal_is_caught() {
        let yaml = make_preset("Run `ralph task create` first.");
        let findings = check_instructions_opac(&yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_TASK_CREATE_LITERAL)
        );
    }

    #[test]
    fn events_jsonl_read_is_caught() {
        let yaml = make_preset("Read tail of .ralph/events.jsonl for audit.");
        let findings = check_instructions_opac(&yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER)
        );
    }

    #[test]
    fn supervisor_db_read_is_caught() {
        let yaml = make_preset("Open .ralph/supervisor.db to inspect waves.");
        let findings = check_instructions_opac(&yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER)
        );
    }

    #[test]
    fn diagnose_supervisor_is_caught() {
        let yaml = make_preset("Run `ralph diagnose --supervisor` for state.");
        let findings = check_instructions_opac(&yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER)
        );
    }

    #[test]
    fn supervisor_coord_emit_is_caught() {
        let yaml = make_preset("Use `ralph emit review.wave.complete --json '{...}'` to close.");
        let findings = check_instructions_opac(&yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_SUPERVISOR_COORDINATION_TOPIC)
        );
    }

    #[test]
    fn opac_skill_reference_missing_is_caught() {
        // No `ralph-tools-opac` reference; only generic text.
        let yaml = r"
hats:
  implementer:
    subscribes_to: [work.ready]
    publishes:
      - work.done
    instructions: |
      Do the work and emit work.done at the end.
";
        let findings = check_instructions_opac_with_preset(yaml, "ce-executor-pipeline-loop");
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_OPAC_SKILL_REFERENCE_MISSING)
        );
    }

    #[test]
    fn fix_unit_mint_template_missing_is_caught() {
        let yaml = r"
hats:
  fixer:
    subscribes_to: [fix.unit.ready]
    publishes:
      - fix.applied
    instructions: |
      When a fix-unit lands, run the verification suite.
      Cite ralph-tools-opac and ralph-tools-emit §5.
";
        let findings = check_instructions_opac_with_preset(yaml, "ce-executor-pipeline-loop");
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING)
        );
    }

    #[test]
    fn fix_unit_mint_template_present_passes() {
        let yaml = r"
hats:
  fixer:
    subscribes_to: [fix.unit.ready]
    publishes:
      - fix.applied
    instructions: |
      When a fix-unit lands, call `ralph tools task ensure --for-fix-unit --key ...`.
      Cite ralph-tools-opac and ralph-tools-emit §5.
";
        let findings = check_instructions_opac_with_preset(yaml, "ce-executor-pipeline-loop");
        assert!(
            !findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING),
            "fix-unit mint template was cited; should not fire"
        );
    }

    #[test]
    fn hat_without_instructions_is_skipped() {
        let yaml = r"
hats:
  silent:
    subscribes_to: [work.ready]
    publishes:
      - work.done
";
        let findings = check_instructions_opac_with_preset(yaml, "ce-executor-pipeline-loop");
        assert!(
            findings.is_empty(),
            "hat with no instructions: skip silently"
        );
    }

    #[test]
    fn task_mutation_authority_matrix() {
        let cases = [
            (
                "non-coordinator",
                "ralph tools task add \"Unit\" --key k",
                false,
                true,
            ),
            (
                "projection-owner",
                "ralph tools task ensure --key k",
                true,
                true,
            ),
        ];
        for (name, command, projection_owned, is_coordinator) in cases {
            let yaml = format!(
                "event_loop:\n  state_projection:\n    actions:\n      custom.ready:\n        kind: ensure_task\n        key: task_key\ntasks:\n  coordinator_hats: [{}]\nhats:\n  worker:\n    instructions: |\n      {command}\n",
                if is_coordinator { "worker" } else { "other" }
            );
            let findings = check_instructions_opac_with_preset(&yaml, "");
            assert!(
                findings.iter().any(|finding| {
                    finding.id == FINDING_INSTRUCTIONS_TASK_MUTATION_AUTHORITY_CONFLICT
                        && finding.hat.as_deref() == Some("worker")
                }),
                "{name} must be rejected: {findings:?}"
            );
            let _ = projection_owned;
        }
    }

    #[test]
    fn task_mutation_negative_read_only_and_fix_unit_cases_are_allowed() {
        let yaml = r#"
event_loop:
  state_projection:
    actions:
      custom.ready:
        kind: ensure_task
        key: task_key
tasks:
  coordinator_hats: [coordinator]
hats:
  coordinator:
    instructions: |
      Use `ralph tools task list` and `ralph tools task show` for status.
  fixer:
    instructions: |
      Call `ralph tools task ensure --for-fix-unit --key ...`.
"#;
        let findings = check_instructions_opac_with_preset(yaml, "");
        assert!(
            !findings.iter().any(|finding| {
                finding.id == FINDING_INSTRUCTIONS_TASK_MUTATION_AUTHORITY_CONFLICT
            }),
            "allowed cases must not report: {findings:?}"
        );
    }

    /// The lint must fire even when the instructions author writes a
    /// top-level "do not"-style disclaimer. Authority comes from the
    /// configured projector / `tasks.coordinator_hats`, not from text
    /// the agent may or may not parse; a disclaimer halfway down the
    /// file must not exempt a real mutation mention earlier.
    #[test]
    fn task_mutation_does_not_exempt_via_disclaimer_text() {
        let yaml = r#"
event_loop:
  state_projection:
    actions:
      custom.ready:
        kind: ensure_task
        key: task_key
tasks:
  coordinator_hats: [coordinator]
hats:
  worker:
    instructions: |
      Do not call `ralph tools task add`; rely on the projector.
      Continue with `ralph tools task add --key urgent-fix --title "..."`.
"#;
        let findings = check_instructions_opac_with_preset(yaml, "");
        assert!(
            findings.iter().any(|f| f.id
                == FINDING_INSTRUCTIONS_TASK_MUTATION_AUTHORITY_CONFLICT
                && f.hat.as_deref() == Some("worker")),
            "disclaimer must not exempt real mutation mention elsewhere: {findings:?}"
        );
    }

    /// U7 error path: hat publishes a business event, the
    /// `instructions` text talks about `payload` / `ralph
    /// emit` / `field shape`, but the new
    /// `ralph-tools-emit` policy-check feedback section is
    /// not cited. The lint fires with
    /// `INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING`.
    #[test]
    fn u7_emit_feedback_skill_reference_missing_is_caught() {
        let yaml = r"
hats:
  emitter:
    subscribes_to: [work.start]
    publishes:
      - work.done
    instructions: |
      Build the JSON payload for work.done and call `ralph emit work.done --json '<payload>'`.
";
        let findings = check_instructions_opac_with_preset(yaml, "ce-executor-pipeline-loop");
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING),
            "expected emit-feedback-skill-reference finding, got: {findings:?}"
        );
    }

    /// U7 happy path: instructions cite `ralph-tools-emit`
    /// AND mention one of the U3 enrichment fields
    /// (e.g. `field_description`, `suggested_payload_shape`,
    /// `suggested_command`). The lint passes.
    #[test]
    fn u7_emit_feedback_skill_reference_present_passes() {
        let yaml = r"
hats:
  emitter:
    subscribes_to: [work.start]
    publishes:
      - work.done
    instructions: |
      Build the JSON payload for work.done. Cite ralph-tools-emit and read the policy-check feedback section for `field_description` / `suggested_payload_shape` / `suggested_command` after a rejection.
";
        let findings = check_instructions_opac_with_preset(yaml, "ce-executor-pipeline-loop");
        assert!(
            !findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING),
            "ralph-tools-emit + enrichment-field mention must satisfy the lint"
        );
    }

    /// U7 non-goal: hat publishes business event but
    /// `instructions` does NOT mention payload construction
    /// — the lint stays silent. This is the gate the
    /// `mentions_payload_construction` heuristic enforces:
    /// hats that just say "publish at the end" without
    /// describing the payload are exempt (they already rely
    /// on the schema-aware prompt section, not the inline
    /// text).
    #[test]
    fn u7_emit_feedback_skill_reference_no_payload_mention_is_skipped() {
        let yaml = r"
hats:
  observer:
    subscribes_to: [work.start]
    publishes:
      - work.done
    instructions: |
      Do the work and publish work.done at the end.
";
        let findings = check_instructions_opac_with_preset(yaml, "ce-executor-pipeline-loop");
        assert!(
            !findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING),
            "hat without payload-shaping text must be exempt, got: {findings:?}"
        );
    }

    /// U7 whitelist gate: the lint stays silent when the
    /// preset is not on the
    /// `U7_EMIT_FEEDBACK_LINT_PRESET_WHITELIST`. A future
    /// preset author can opt in by adding the preset to the
    /// list — this test pins the gate so a wholesale widening
    /// of the rule is a deliberate code change.
    #[test]
    fn u7_emit_feedback_skill_reference_preset_whitelist_gate() {
        let yaml = r"
hats:
  emitter:
    subscribes_to: [work.start]
    publishes:
      - work.done
    instructions: |
      Build the JSON payload for work.done and call `ralph emit work.done --json '<payload>'`.
";
        // Whitelisted preset → finding expected.
        let findings = check_instructions_opac_with_preset(yaml, "ce-executor-pipeline-loop");
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING),
            "whitelisted preset must fire the rule, got: {findings:?}"
        );
        // Non-whitelisted preset → finding must NOT fire,
        // even though the same yaml would have triggered it.
        let findings = check_instructions_opac_with_preset(yaml, "merge-loop");
        assert!(
            !findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING),
            "non-whitelisted preset must stay silent, got: {findings:?}"
        );
        // Empty preset name → same as the non-whitelisted
        // branch (preserves the pre-U7 behaviour for
        // `check_instructions_opac` callers that don't have
        // a preset name handy).
        let findings = check_instructions_opac_with_preset(yaml, "");
        assert!(
            !findings
                .iter()
                .any(|f| f.id == FINDING_INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING),
            "empty preset name must stay silent, got: {findings:?}"
        );
    }
}
