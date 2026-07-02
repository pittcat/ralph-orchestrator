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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RalphConfig;
    use std::collections::HashMap;

    /// Build a minimal config with the synthesized
    /// `precheck-<X>` gate hat and a single non-gate consumer
    /// so we can assert that:
    /// - the gate hat is recognized as the owner of `<X>` and
    ///   `<X>.rejected`,
    /// - publishing `<X>` from a non-owner is flagged (proves
    ///   the gate hat actually owns the topic in the lint's
    ///   eyes),
    /// - publishing `<X>` from the gate hat itself is
    ///   accepted (no spurious R3 finding).
    fn desugared_precheck_config() -> RalphConfig {
        let mut hats = HashMap::new();
        hats.insert(
            "executor".to_string(),
            crate::config::HatConfig {
                name: "Executor".to_string(),
                triggers: vec!["work.start".to_string()],
                publishes: vec!["review.complete.proposed".to_string()],
                ..Default::default()
            },
        );
        hats.insert(
            "precheck-review.complete".to_string(),
            crate::config::HatConfig {
                name: "Precheck Gate: review.complete".to_string(),
                triggers: vec!["review.complete.proposed".to_string()],
                publishes: vec![
                    "review.complete".to_string(),
                    "review.complete.rejected".to_string(),
                ],
                ..Default::default()
            },
        );
        hats.insert(
            "reviewer".to_string(),
            crate::config::HatConfig {
                name: "Reviewer".to_string(),
                triggers: vec!["review.complete".to_string()],
                publishes: vec!["work.done".to_string()],
                ..Default::default()
            },
        );
        let mut topic_owners = HashMap::new();
        topic_owners.insert(
            "review.complete".to_string(),
            vec!["precheck-review.complete".to_string()],
        );
        topic_owners.insert(
            "review.complete.rejected".to_string(),
            vec!["precheck-review.complete".to_string()],
        );
        RalphConfig {
            hats,
            topic_owners,
            ..Default::default()
        }
    }

    /// 2026-07-02-004 plan U8: ownership lint recognizes the
    /// synthesized gate hat as the sole owner of both `<X>`
    /// and `<X>.rejected`.  No R3 finding should fire — the
    /// gate is the owner and is allowed to publish.
    #[test]
    fn synthesized_gate_hat_is_recognized_as_owner_of_guarded_topic() {
        let config = desugared_precheck_config();
        let findings = check_ownership_rules(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "gate hat must own review.complete / .rejected; got: {findings:?}"
        );
        // And strict mode should not regress.
        let strict_findings = check_ownership_rules(&config, LintStrictness::Strict);
        assert!(
            strict_findings.is_empty(),
            "strict mode must also accept the synthesized owner, got: {strict_findings:?}"
        );
    }

    /// Counter-test: if the operator hand-writes a producer
    /// of `<X>` that is NOT the gate hat (so the gate hat is
    /// no longer the sole owner of `<X>`), the ownership lint
    /// MUST flag the cross-hat publish.  This proves the
    /// previous test is actually exercising the owner check
    /// rather than no-oping.
    #[test]
    fn non_owner_publishing_guarded_topic_is_flagged() {
        let mut config = desugared_precheck_config();
        // Add a stray hat that publishes the guarded topic.
        config.hats.insert(
            "intruder".to_string(),
            crate::config::HatConfig {
                name: "Intruder".to_string(),
                triggers: vec!["work.start".to_string()],
                publishes: vec!["review.complete".to_string()],
                ..Default::default()
            },
        );
        let findings = check_ownership_rules(&config, LintStrictness::Default);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH),
            "intruder publishing owned topic must be flagged, got: {findings:?}"
        );
    }
}
