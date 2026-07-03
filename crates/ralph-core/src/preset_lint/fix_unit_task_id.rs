//! 2026-07-03-002 plan U1: `fix_unit_task_id_helper_derived` lint.
//!
//! Static scan of coordinator hats' instructions: when a coordinator
//! hat **publishes `work.ready`** (the dispatch signal) AND its
//! instructions mention fix-unit dispatch (`fix-NN` / `fix_unit` /
//! `fix-01` etc.), it MUST include a `ralph tools task create` CLI
//! invocation template AND reference the canonical
//! `task-{plan_slug}-fix{NN}u{NN}-{ts_hex}` shape (the output of
//! `Task::fix_unit_task_id` in `crates/ralph-core/src/task.rs:143-158`).
//!
//! 093813 root cause: `presets/en/ce-executor-serial.yml:988-994` had
//! the `MUST be freshly minted` HARD RULE but no CLI parameter template.
//! The agent could not infer `--plan-name` / `--step` / `--task-key`
//! arguments, so it hand-composed a task_id reusing a prior step's id,
//! which `state_projector/task.rs:253-260` correctly rejected with
//! `task_id_reused_across_keys`. The rejection stalled fix-01 dispatch
//! and the entire fix-unit chain (fix-02/03, plan.complete,
//! REVIEW_COMPLETE, LOOP_COMPLETE) never fired.
//!
//! Scope guard: only hats that **publish `work.ready`** are checked.
//! `coordinator_hats` typically lists `coordinator` (dispatch),
//! `executor` (task lifecycle participant), and `validator` — but only
//! the dispatch hat mints the task_id. Checking the executor/validator
//! would false-positive: they consume `work.ready` and close/test the
//! task, they do not mint the id.
//!
//! This lint is the **primary** (事前) defence; the runtime projector
//! rejection at `state_projector/task.rs:253-260` is the **secondary**
//! (事后) defence. The lint surfaces the gap at preset-load time rather
//! than mid-run.
//!
//! Severity: `Error` (always). The rule is structural — a dispatch
//! hat that emits fix-unit `work.ready` without a minting template is
//! broken by construction.

use super::{LintFinding, LintStrictness};
use crate::config::RalphConfig;

/// 2026-07-03-002 plan U1: stable finding ID.
pub const FINDING_FIX_UNIT_TASK_ID_NOT_HELPER_DERIVED: &str =
    "preset.fix_unit_task_id_not_helper_derived";

/// Markers that indicate a coordinator hat handles fix-unit dispatch.
/// If NONE of these appear in the instructions, the hat is skipped
/// (it does not dispatch fix-units, so the rule is out of scope).
///
/// `fix-NN` / `fix_unit` are the dispatch-grammar tokens; `fix-01` is
/// the concrete example. The generic `fix-unit` word is intentionally
/// excluded — it appears in prose like "no fix-unit handling here" and
/// would false-positive on hats that merely mention the concept.
const FIX_UNIT_DISPATCH_MARKERS: &[&str] = &["fix-NN", "fix_unit", "fix-01", "fix-02", "fix-03"];

/// The dispatch signal topic. Only hats that publish `work.ready` mint
/// task_ids; the executor/validator consume it and act on the id they
/// receive, so they are out of scope for this lint.
const WORK_READY_TOPIC: &str = "work.ready";

/// The review-complete trigger topic. The fix-unit dispatch hat
/// triggers on `review.complete` (it wakes to dispatch fix-units after
/// review fails). Progress-steward also publishes `work.ready` as a
/// stall fallback, but it triggers on `loop.stalled` only — it does
/// not mint task_ids from a fix-plan, it re-emits the prior step's
/// work.ready. Scope the lint to hats that trigger on `review.complete`
/// to avoid false-positiving on progress-steward.
const REVIEW_COMPLETE_TRIGGER: &str = "review.complete";

/// Marker that the instructions reference the canonical task_id shape
/// produced by `Task::fix_unit_task_id`. Either the literal shape
/// pattern or the Rust helper name satisfies this.
const CANONICAL_SHAPE_MARKERS: &[&str] = &[
    "task-{plan_slug}-fix{NN}u{NN}-{ts_hex}",
    "Task::fix_unit_task_id",
];

/// Marker that the instructions give the agent a concrete CLI template
/// to call `ralph tools task create`. The literal command name is the
/// minimum; a full parameter template is preferred but the lint only
/// enforces presence, not parameter completeness (the latter is a
/// documentation concern, not a structural invariant).
const CLI_TEMPLATE_MARKER: &str = "ralph tools task create";

/// Run the U1 lint against `config`. Returns zero or more findings.
///
/// A finding fires when a coordinator hat:
/// 1. is listed in `tasks.coordinator_hats`, AND
/// 2. publishes `work.ready` (the dispatch signal — only dispatch hats
///    mint task_ids), AND
/// 3. its instructions mention any `FIX_UNIT_DISPATCH_MARKERS` (the hat
///    handles fix-unit dispatch), AND
/// 4. the instructions do NOT contain `CLI_TEMPLATE_MARKER`, OR
/// 5. the instructions do NOT contain any `CANONICAL_SHAPE_MARKERS`.
///
/// Hats not in `coordinator_hats`, hats that do not publish `work.ready`,
/// or hats whose instructions do not mention fix-unit dispatch are skipped.
pub fn check_fix_unit_task_id_helper_derived(
    config: &RalphConfig,
    _strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    // Build the set of coordinator hats from `tasks.coordinator_hats`.
    // Empty list → no coordinator hats are gated, so the lint is a no-op
    // (consistent with `FINDING_COORDINATOR_MISSING` in `ownership.rs`).
    let coordinator_hats: std::collections::HashSet<&str> = config
        .tasks
        .coordinator_hats
        .iter()
        .map(String::as_str)
        .collect();

    if coordinator_hats.is_empty() {
        return findings;
    }

    for (hat_id, hat_cfg) in &config.hats {
        if !coordinator_hats.contains(hat_id.as_str()) {
            continue;
        }

        // Scope guard: only dispatch hats (those that publish work.ready)
        // mint task_ids. Executor/validator consume work.ready and act on
        // the id they receive — they do not mint, so they are out of scope.
        if !hat_cfg.publishes.iter().any(|t| t == WORK_READY_TOPIC) {
            continue;
        }

        // Scope guard: progress-steward also publishes `work.ready` as a
        // stall fallback, but it triggers on `loop.stalled` and re-emits
        // the prior step's work.ready — it does not mint task_ids from a
        // fix-plan. The fix-unit dispatch hat triggers on `review.complete`
        // (it wakes to dispatch fix-units after review fails). Only check
        // hats that trigger on review.complete.
        if !hat_cfg.triggers.iter().any(|t| t == REVIEW_COMPLETE_TRIGGER) {
            continue;
        }

        // Combine `instructions` + `extra_instructions` so YAML-anchor
        // fragments are scanned too.
        let mut body = hat_cfg.instructions.clone();
        for fragment in &hat_cfg.extra_instructions {
            body.push('\n');
            body.push_str(fragment);
        }

        // Skip hats that do not handle fix-unit dispatch.
        let dispatches_fix_units = FIX_UNIT_DISPATCH_MARKERS
            .iter()
            .any(|marker| body.contains(marker));
        if !dispatches_fix_units {
            continue;
        }

        let has_cli_template = body.contains(CLI_TEMPLATE_MARKER);
        let has_canonical_shape = CANONICAL_SHAPE_MARKERS
            .iter()
            .any(|marker| body.contains(marker));

        if has_cli_template && has_canonical_shape {
            continue;
        }

        let missing: Vec<&str> = match (!has_cli_template, !has_canonical_shape) {
            (true, true) => vec![
                "a `ralph tools task create` CLI invocation template",
                "the canonical `task-{plan_slug}-fix{NN}u{NN}-{ts_hex}` shape \
                 (or `Task::fix_unit_task_id` reference)",
            ],
            (true, false) => vec!["a `ralph tools task create` CLI invocation template"],
            (false, true) => vec![
                "the canonical `task-{plan_slug}-fix{NN}u{NN}-{ts_hex}` shape \
                 (or `Task::fix_unit_task_id` reference)",
            ],
            (false, false) => unreachable!("both present ⇒ skip earlier"),
        };

        let finding = LintFinding::error(
            FINDING_FIX_UNIT_TASK_ID_NOT_HELPER_DERIVED,
            format!(
                "coordinator hat '{}' publishes `work.ready` and handles \
                 fix-unit dispatch but its instructions lack {}. The 093813 \
                 run stalled because the coordinator hand-composed a task_id \
                 reusing a prior step's id, which \
                 `state_projector/task.rs:253-260` rejected with \
                 `task_id_reused_across_keys`. Add a `ralph tools task create` \
                 CLI template and reference the canonical shape so the agent \
                 mints a fresh id per fix-unit.",
                hat_id,
                missing.join(" and ")
            ),
        )
        .with_hat(hat_id.clone())
        .with_topic(WORK_READY_TOPIC)
        .with_action_hint(format!(
            "In `presets/en/<preset>.yml` under `hats.{}.instructions`, add a \
             `### Fix-Unit Task ID Minting` section that (1) shows the \
             `ralph tools task create --plan-name <plan> --fix-unit <round> \
             <index>` CLI template and (2) names the canonical \
             `task-{{plan_slug}}-fix{{NN}}u{{NN}}-{{ts_hex}}` shape produced \
             by `Task::fix_unit_task_id`.",
            hat_id
        ));
        findings.push(finding);
    }

    findings.sort_by(|a, b| a.hat.cmp(&b.hat));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HatConfig, RalphConfig};
    use std::collections::HashMap;

    fn cfg_with_coordinator(instructions: &str) -> RalphConfig {
        let mut hats = HashMap::new();
        hats.insert(
            "coordinator".to_string(),
            HatConfig {
                instructions: instructions.to_string(),
                // Mirror the production ce-executor-serial coordinator:
                // publishes `work.ready` (dispatch signal) AND triggers
                // `review.complete` (wakes to dispatch fix-units after
                // review fails). The lint's second scope guard requires
                // both signals to identify a fix-unit dispatch hat.
                publishes: vec!["work.ready".to_string()],
                triggers: vec!["review.complete".to_string()],
                ..HatConfig::default()
            },
        );
        RalphConfig {
            hats,
            tasks: crate::config::TasksConfig {
                enabled: true,
                coordinator_hats: vec!["coordinator".to_string()],
            },
            ..RalphConfig::default()
        }
    }

    #[test]
    fn coordinator_with_fix_unit_and_full_template_no_finding() {
        let instructions = r#"
      ### Fix-Unit Task ID Minting
      Call `ralph tools task create` to mint a fresh task_id.
      The CLI derives `task-{plan_slug}-fix{NN}u{NN}-{ts_hex}` shape.
      fix-01 dispatch uses this template.
      "#;
        let config = cfg_with_coordinator(instructions);
        let findings = check_fix_unit_task_id_helper_derived(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "coordinator with full template must produce no finding, got {findings:?}"
        );
    }

    #[test]
    fn coordinator_with_fix_unit_but_no_cli_template_emits_finding() {
        let instructions = r#"
      Dispatch fix-01 with a fresh task_id. DO NOT reuse prior ids.
      The canonical shape is task-{plan_slug}-fix{NN}u{NN}-{ts_hex}.
      "#;
        let config = cfg_with_coordinator(instructions);
        let findings = check_fix_unit_task_id_helper_derived(&config, LintStrictness::Default);
        assert_eq!(
            findings.len(),
            1,
            "missing CLI template must produce 1 finding, got {findings:?}"
        );
        let f = &findings[0];
        assert_eq!(f.id, FINDING_FIX_UNIT_TASK_ID_NOT_HELPER_DERIVED);
        assert_eq!(f.severity, crate::preset_lint::LintSeverity::Error);
        assert_eq!(f.hat.as_deref(), Some("coordinator"));
        assert!(
            f.action_hint
                .as_deref()
                .unwrap()
                .contains("ralph tools task create"),
            "action_hint must mention CLI template, got {:?}",
            f.action_hint
        );
    }

    #[test]
    fn coordinator_with_fix_unit_but_no_shape_marker_emits_finding() {
        let instructions = r#"
      Dispatch fix-01. Call `ralph tools task create` to mint.
      No shape reference here.
      "#;
        let config = cfg_with_coordinator(instructions);
        let findings = check_fix_unit_task_id_helper_derived(&config, LintStrictness::Default);
        assert_eq!(
            findings.len(),
            1,
            "missing shape marker must produce 1 finding, got {findings:?}"
        );
        let f = &findings[0];
        assert!(
            f.message
                .contains("task-{plan_slug}-fix{NN}u{NN}-{ts_hex}"),
            "finding message must mention missing shape, got {}",
            f.message
        );
    }

    #[test]
    fn coordinator_without_fix_unit_dispatch_no_finding() {
        let instructions = r#"
      Dispatch step-01 with ralph tools task create.
      No fix-unit handling here.
      "#;
        let config = cfg_with_coordinator(instructions);
        let findings = check_fix_unit_task_id_helper_derived(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "non-fix-unit coordinator must be skipped, got {findings:?}"
        );
    }

    #[test]
    fn hat_not_in_coordinator_hats_no_finding() {
        let mut hats = HashMap::new();
        hats.insert(
            "executor".to_string(),
            HatConfig {
                instructions: "fix-01 dispatch without template".to_string(),
                publishes: vec!["work.ready".to_string()],
                ..HatConfig::default()
            },
        );
        let config = RalphConfig {
            hats,
            tasks: crate::config::TasksConfig {
                enabled: true,
                coordinator_hats: vec!["coordinator".to_string()],
            },
            ..RalphConfig::default()
        };
        let findings = check_fix_unit_task_id_helper_derived(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "non-coordinator hat must be skipped even with fix-unit markers, got {findings:?}"
        );
    }

    #[test]
    fn empty_coordinator_hats_list_no_finding() {
        let mut hats = HashMap::new();
        hats.insert(
            "coordinator".to_string(),
            HatConfig {
                instructions: "fix-01 without template".to_string(),
                publishes: vec!["work.ready".to_string()],
                ..HatConfig::default()
            },
        );
        let config = RalphConfig {
            hats,
            tasks: crate::config::TasksConfig {
                enabled: true,
                coordinator_hats: vec![],
            },
            ..RalphConfig::default()
        };
        let findings = check_fix_unit_task_id_helper_derived(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "empty coordinator_hats list ⇒ no-op, got {findings:?}"
        );
    }

    #[test]
    fn executor_publishes_no_work_ready_no_finding() {
        // Scope guard: executor consumes work.ready (triggers on it) but
        // does not publish it — it publishes work.done. Even if its
        // instructions mention fix-01 and it is in coordinator_hats,
        // it must NOT be flagged (it does not mint task_ids).
        let mut hats = HashMap::new();
        hats.insert(
            "executor".to_string(),
            HatConfig {
                instructions: "fix-01 mentioned in prose".to_string(),
                publishes: vec!["work.done".to_string()],
                triggers: vec!["work.ready".to_string()],
                ..HatConfig::default()
            },
        );
        let config = RalphConfig {
            hats,
            tasks: crate::config::TasksConfig {
                enabled: true,
                coordinator_hats: vec!["executor".to_string()],
            },
            ..RalphConfig::default()
        };
        let findings = check_fix_unit_task_id_helper_derived(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "executor (publishes work.done, not work.ready) must be skipped, got {findings:?}"
        );
    }

    #[test]
    fn extra_instructions_are_scanned() {
        let mut hats = HashMap::new();
        hats.insert(
            "coordinator".to_string(),
            HatConfig {
                instructions: "Main instructions mention fix-01.".to_string(),
                publishes: vec!["work.ready".to_string()],
                triggers: vec!["review.complete".to_string()],
                extra_instructions: vec![
                    "Call `ralph tools task create` to mint.".to_string(),
                    "Shape: task-{plan_slug}-fix{NN}u{NN}-{ts_hex}".to_string(),
                ],
                ..HatConfig::default()
            },
        );
        let config = RalphConfig {
            hats,
            tasks: crate::config::TasksConfig {
                enabled: true,
                coordinator_hats: vec!["coordinator".to_string()],
            },
            ..RalphConfig::default()
        };
        let findings = check_fix_unit_task_id_helper_derived(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "extra_instructions markers must satisfy the lint, got {findings:?}"
        );
    }

    #[test]
    fn task_helper_rust_name_satisfies_shape_marker() {
        let instructions = r#"
      Dispatch fix-01. Call `ralph tools task create` to mint.
      The id matches Task::fix_unit_task_id output shape.
      "#;
        let config = cfg_with_coordinator(instructions);
        let findings = check_fix_unit_task_id_helper_derived(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "Task::fix_unit_task_id Rust name must satisfy shape marker, got {findings:?}"
        );
    }

    #[test]
    fn progress_steward_publishes_work_ready_but_triggers_loop_stalled_no_finding() {
        // Regression for the 093813 fix's second scope guard:
        // progress-steward publishes `work.ready` as a stall fallback
        // AND is listed in coordinator_hats (it participates in task
        // lifecycle), but it triggers on `loop.stalled` — NOT
        // `review.complete`. It re-emits the prior step's work.ready
        // rather than minting a fresh task_id from a fix-plan, so it
        // must NOT be flagged even if its instructions happen to
        // mention fix-unit markers and lack the CLI template.
        let mut hats = HashMap::new();
        hats.insert(
            "progress-steward".to_string(),
            HatConfig {
                instructions: "Re-dispatch fix-01 on stall. No template here."
                    .to_string(),
                publishes: vec!["work.ready".to_string()],
                triggers: vec!["loop.stalled".to_string()],
                ..HatConfig::default()
            },
        );
        let config = RalphConfig {
            hats,
            tasks: crate::config::TasksConfig {
                enabled: true,
                coordinator_hats: vec!["progress-steward".to_string()],
            },
            ..RalphConfig::default()
        };
        let findings = check_fix_unit_task_id_helper_derived(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "progress-steward (triggers loop.stalled, not review.complete) \
             must be skipped even with fix-unit markers and no template, \
             got {findings:?}"
        );
    }
}
