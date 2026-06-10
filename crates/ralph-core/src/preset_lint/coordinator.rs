//! U2: Coordinator static rules (R5).
//!
//! This module owns `check_coordinator_rules` — the R5 checks that
//! enforce `tasks.coordinator_hats` invariants when `tasks.enabled` is
//! true.
//!
//! Implementation Plan Unit: U2 of `2026-06-08-003-feat-preset-static-lint-plan`.

use crate::config::RalphConfig;
use crate::preset_lint::LintFinding;
use crate::preset_lint::finding_id::{
    FINDING_COORDINATOR_MISSING, FINDING_TASK_PUBLISHER_NOT_COORDINATED,
};
use crate::preset_lint::ownership::hat_publishes_refs;

/// Check R5: When `tasks.enabled=true`, `coordinator_hats` must be
/// non-empty, and every hat that publishes a `task.*` topic must be
/// listed in `coordinator_hats`.
///
/// Always returns `Error` severity findings.
pub fn check_coordinator_rules(config: &RalphConfig) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    if !config.tasks.enabled {
        return findings;
    }

    // R5a: coordinator_hats must be non-empty.
    if config.tasks.coordinator_hats.is_empty() {
        // Collect candidate hats that publish task.* topics.
        let candidates: Vec<&str> = config
            .hats
            .iter()
            .filter(|(_, hat)| hat_publishes_refs(hat).any(|p| p.starts_with("task.")))
            .map(|(hat_id, _)| hat_id.as_str())
            .collect();

        let hint = if candidates.is_empty() {
            "Add coordinator_hats to the tasks section".to_string()
        } else {
            format!(
                "Add coordinator_hats: [{}] to the tasks section",
                candidates.join(", ")
            )
        };

        findings.push(
            LintFinding::error(
                FINDING_COORDINATOR_MISSING,
                "tasks.enabled is true but tasks.coordinator_hats is empty; \
                 at least one coordinator hat is required",
            )
            .with_action_hint(hint),
        );

        // Don't check task publishers if coordinator is empty —
        // the error above is sufficient and more actionable.
        return findings;
    }

    let coordinator_set: std::collections::HashSet<&str> = config
        .tasks
        .coordinator_hats
        .iter()
        .map(|s| s.as_str())
        .collect();

    // R5b: Every hat publishing task.* must be in coordinator_hats.
    for (hat_id, hat_config) in &config.hats {
        let task_topics: Vec<&str> = hat_publishes_refs(hat_config)
            .filter(|p| p.starts_with("task."))
            .collect();
        if !task_topics.is_empty() && !coordinator_set.contains(hat_id.as_str()) {
            findings.push(
                LintFinding::error(
                    FINDING_TASK_PUBLISHER_NOT_COORDINATED,
                    format!(
                        "hat \"{hat_id}\" publishes task topics [{}] but is not \
                         listed in tasks.coordinator_hats",
                        task_topics.join(", ")
                    ),
                )
                .with_hat(hat_id)
                .with_action_hint(format!("Add \"{hat_id}\" to tasks.coordinator_hats")),
            );
        }
    }

    findings
}
