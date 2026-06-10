//! U2: Ownership static rules (R2/R3/R4).
//!
//! This module owns the `check_owner_references` and `check_ownership_rules`
//! functions that enforce:
//!
//! - R2: every owner hat referenced in `topic_owners` must exist, and
//!   must actually publish the topic in its `publishes`/`default_publishes`.
//! - R3: non-owner hats must not publish owner topics.
//! - R4: a topic declared in `topic_owners` must have at least one
//!   publisher hat.
//!
//! Coordinator rules (R5) live in the sibling `coordinator` module.
//!
//! Implementation Plan Unit: U2 of `2026-06-08-003-feat-preset-static-lint-plan`.

use crate::config::RalphConfig;
use crate::preset_lint::finding_id::{
    FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH, FINDING_MISSING_TOPIC_OWNER,
    FINDING_OWNER_NOT_PUBLISHER, FINDING_OWNER_UNKNOWN_HAT,
};
use crate::preset_lint::{LintFinding, LintStrictness};

/// Check R2: Every owner hat referenced in `topic_owners` must exist
/// in the config's hat map.
///
/// Returns `Error` findings for unknown hats.
pub fn check_owner_references(config: &RalphConfig) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    for (topic, owners) in &config.topic_owners {
        for owner in owners {
            if !config.hats.contains_key(owner) {
                findings.push(
                    LintFinding::error(
                        FINDING_OWNER_UNKNOWN_HAT,
                        format!(
                            "topic_owners[\"{topic}\"] references unknown hat \"{owner}\"; \
                             add a hat definition or remove the owner entry"
                        ),
                    )
                    .with_topic(topic)
                    .with_owner(owner)
                    .with_action_hint(format!("Add hat \"{owner}\" to the hats section")),
                );
            }
        }
    }

    findings
}

/// Iterate over all topics a hat explicitly publishes (via `publishes` or
/// `default_publishes`) without allocating.
pub(super) fn hat_publishes_refs(
    hat_config: &crate::config::HatConfig,
) -> impl Iterator<Item = &str> {
    hat_config
        .publishes
        .iter()
        .map(String::as_str)
        .chain(hat_config.default_publishes.as_deref())
}

/// Check R2 + R3: Owner hats must publish their owned topic, and
/// non-owner hats must not publish owner topics.
///
/// In strict mode, all warnings become errors.
pub fn check_ownership_rules(config: &RalphConfig, strictness: LintStrictness) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    for (topic, owners) in &config.topic_owners {
        // Build set of hats that publish this topic.
        let publishers: Vec<&str> = config
            .hats
            .iter()
            .filter(|(_, hat)| hat_publishes_refs(hat).any(|p| p == topic))
            .map(|(hat_id, _)| hat_id.as_str())
            .collect();

        // R4: If a topic is declared in topic_owners, at least one hat must publish it.
        //
        // P2 #21 fix: when R4 fires (no publisher at all), we ALSO skip
        // the per-owner R2 loop below. Otherwise the operator would get:
        //   - 1 × FINDING_MISSING_TOPIC_OWNER ("no publisher at all")
        //   - N × FINDING_OWNER_NOT_PUBLISHER ("each owner doesn't publish")
        // which is 1 + N redundant findings for the same root cause —
        // confusing for humans reading the report and for CI tools that
        // count finding IDs. The R4 finding's message + action_hint
        // already names every owner and tells the operator what to do,
        // so emitting N more FINDING_OWNER_NOT_PUBLISHER entries would
        // be pure noise.
        //
        // R2 still fires for the *partial* case (some owners publish,
        // some don't) where the per-owner findings are genuinely
        // distinct and actionable.
        // P3 #25: removed the `!owners.is_empty()` defensive guard
        // because the construction site in
        // `RalphConfig::topic_owners` never inserts empty `Vec<String>`
        // values — the deserializer would reject them and the merge
        // code drops empty entries. The condition is therefore
        // equivalent to `publishers.is_empty()`. Kept the explicit
        // check out of an abundance of caution only if a future
        // construction site changes the invariant; if that happens
        // also audit the `owners.join(", ")` call below.
        let r4_fired = if publishers.is_empty() {
            let severity = strictness.ownership_severity();
            let owner_list = owners.join(", ");
            findings.push(LintFinding {
                id: FINDING_MISSING_TOPIC_OWNER,
                severity,
                message: format!(
                    "topic \"{topic}\" is declared in topic_owners with owners [{owner_list}] \
                         but no hat publishes it; at least one owner must publish this topic \
                         (covers R2 owner-publishes check for each owner)"
                ),
                topic: Some(topic.clone()),
                hat: None,
                owner: Some(owner_list.clone()),
                action_hint: Some(format!(
                    "Add \"{topic}\" to the publishes list of one of the owners: [{owner_list}]"
                )),
            });
            true
        } else {
            false
        };

        // R2: Each owner must publish the topic.
        //
        // P2 #21: skip when R4 already fired (the per-owner message is
        // redundant with the umbrella R4 finding).
        if !r4_fired {
            for owner in owners {
                if !publishers.iter().any(|p| *p == owner) {
                    let severity = strictness.ownership_severity();
                    findings.push(LintFinding {
                        id: FINDING_OWNER_NOT_PUBLISHER,
                        severity,
                        message: format!(
                            "hat \"{owner}\" is the declared owner of topic \"{topic}\" \
                                 but does not publish it; add \"{topic}\" to its publishes \
                                 or default_publishes"
                        ),
                        topic: Some(topic.clone()),
                        hat: Some(owner.clone()),
                        owner: Some(owner.clone()),
                        action_hint: Some(format!(
                            "Add \"{topic}\" to hat \"{owner}\" publishes list"
                        )),
                    });
                }
            }
        }

        // R3: Non-owner hats publishing owner topic produce unauthorized publish.
        let owner_set: std::collections::HashSet<&str> =
            owners.iter().map(|s| s.as_str()).collect();
        for publisher in &publishers {
            if !owner_set.contains(*publisher) {
                let severity = strictness.ownership_severity();
                findings.push(LintFinding {
                    id: FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH,
                    severity,
                    message: format!(
                        "hat \"{publisher}\" publishes topic \"{topic}\" which is \
                             owned by [{}]; non-owner publishing is not allowed",
                        owners.join(", ")
                    ),
                    topic: Some(topic.clone()),
                    hat: Some(publisher.to_string()),
                    owner: Some(owners.join(", ")),
                    action_hint: Some(format!(
                        "Remove \"{topic}\" from hat \"{publisher}\" publishes, \
                             or add \"{publisher}\" as an owner"
                    )),
                });
            }
        }
    }

    findings
}
