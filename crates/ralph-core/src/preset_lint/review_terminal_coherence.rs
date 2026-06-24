//! 2026-06-23-004 plan U1 KTD-RTC: review terminal coherence lint rules.
//!
//! These rules detect the structural mismatches that allowed the
//! `ce-executor-serial-primary-20260623-152241` loop to drift from
//! `review.passed` to `review.complete`:
//!
//! - **`check_reviewer_dual_subscribe`**: a downstream consumer hat that
//!   triggers on **both** topics of a mutually exclusive terminal pair
//!   violates the **single-canonical-trigger** invariant.
//!
//! - **`check_publisher_terminal_completeness`**: a hat claiming the
//!   terminal ownership of a pair MUST declare BOTH topics in its
//!   `publishes` set; otherwise the runtime will reject the missing
//!   publish, silently dropping the terminal signal.
//!
//! Both rules are **structural** — they look at declared triggers and
//! publishes only, no runtime events required. This is intentional: the
//! drift already happened in the field, and we want the gate to fire
//! BEFORE the loop starts, not after the fact.
//!
//! 2026-06-24 plan U3: the `REVIEW_PAIR` list is now empty. The new
//! 10-hat architecture has `review-synthesizer` emit only
//! `review.complete` (no `review.passed` / `review.failed` pair), so
//! there is no mutually exclusive terminal pair to police. The lint
//! functions and tests are retained for KTD-TTC-2 follow-up work that
//! may add new pairs (`fix.applied` / `fix.exhausted`, etc.) with their
//! own narrow exemption rules.

use crate::config::RalphConfig;

use super::finding_id::{FINDING_TERMINAL_DUAL_SUBSCRIBE, FINDING_TERMINAL_PUBLISHER_INCOMPLETE};
use super::LintFinding;

/// Mutually exclusive terminal topic pairs covered by this lint.
///
/// A "mutually exclusive terminal pair" is two topics that share a
/// single decision point — exactly one of them fires on any given
/// transition. A downstream hat that subscribes to both will accept
/// whichever arrives first, bypassing the branch decision the publisher
/// was making. A publisher that declares one of the two but not the
/// other will have the omitted publish rejected at runtime (the runtime
/// only allows declared publishes), silently dropping the terminal.
///
/// **2026-06-24 plan U3: empty list.** The new 10-hat architecture
/// removed the `review.passed` / `review.complete` pair —
/// `review-synthesizer` now emits only `review.complete`. There is
/// no mutually exclusive terminal pair in the current preset.
///
/// Future work (KTD-TTC-2): extend the SSOT to cover the other
/// "branch-decision" pairs (`fix.applied` / `fix.exhausted`, etc.)
/// with their own narrow exemption rules. The `pub` accessor below
/// is the API surface for that follow-up.
const REVIEW_PAIR: &[(&str, &str)] = &[];

/// Return the list of mutually exclusive terminal pairs covered by
/// this lint. Currently empty — see the comment on `REVIEW_PAIR` for
/// the rationale and the KTD-TTC-2 follow-up.
pub fn mutually_exclusive_terminal_pairs() -> &'static [(&'static str, &'static str)] {
    REVIEW_PAIR
}

/// Default consumer hat ids that legitimately consume a terminal
/// pair as a unit. The set is intentionally empty.
const DEFAULT_EXEMPT_CONSUMERS: &[String] = &[];

/// Detect a downstream hat that triggers on both topics of any
/// mutually exclusive terminal pair. The two are sibling terminal
/// events; a single hat that consumes both will be confused by the
/// publisher's branch decision and may accept the cheaper alternative
/// while bypassing the `pass` / `pass_with_residuals` (or equivalent)
/// distinction the publisher was enforcing.
///
/// Detected across the pairs in `mutually_exclusive_terminal_pairs()`,
/// not just `review.passed` / `review.complete`. The lint is structural
/// and runs at preset load time, so adding a pair to the SSOT list
/// (above) is enough to make the lint cover it.
pub fn check_reviewer_dual_subscribe(config: &RalphConfig) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let exempt: std::collections::BTreeSet<String> = config
        .event_loop
        .review_terminal_coherence_exempt_consumers
        .as_deref()
        .unwrap_or(DEFAULT_EXEMPT_CONSUMERS)
        .iter()
        .cloned()
        .collect();

    for (hat_id, hat) in &config.hats {
        if exempt.contains(hat_id) {
            continue;
        }
        for (a, b) in REVIEW_PAIR {
            let has_a = hat.triggers.iter().any(|t| t == a);
            let has_b = hat.triggers.iter().any(|t| t == b);
            if !(has_a && has_b) {
                continue;
            }
            findings.push(
                LintFinding::error(
                    FINDING_TERMINAL_DUAL_SUBSCRIBE,
                    format!(
                        "hat \"{hat_id}\" triggers on both \'{a}\' and \'{b}\'; \
                         these are mutually exclusive branch events. \
                         Pick one canonical trigger (the other is the branch you will not see)."
                    ),
                )
                .with_hat(hat_id.clone())
                .with_topic(*a)
                .with_action_hint(format!(
                    "Remove one of the two triggers from hat \"{hat_id}\". \
                     If dual subscription is intentional (e.g. the hat \
                     needs to read the `verdict` payload regardless of \
                     which terminal carries it), add \"{hat_id}\" to \
                     event_loop.review_terminal_coherence_exempt_consumers."
                )),
            );
        }
    }

    findings
}

/// Detect a hat that publishes one topic of any mutually exclusive
/// terminal pair but omits its sibling from the `publishes`
/// declaration. The publisher branches between the two based on
/// runtime data (e.g. residual findings for the synthesizer, fix
/// exhaustion for the fixer); declaring only one means the runtime
/// will reject the other publish (event_policy unknown publish),
/// silently dropping the terminal.
///
/// **Mechanism review 2026-06-24 P1 narrowing**: only the
/// `topic_owners`-registered owner hat of one of the pair's topics
/// is checked. A hat that publishes one terminal as a *hint*
/// (e.g. shipper emits `review.complete` as a status readback but
/// never owns the branch decision) is NOT required to declare the
/// sibling. When `topic_owners` does not register an owner for
/// either topic in the pair, the rule is silent — there is no
/// declared branch owner to be "incomplete" against, and any hat
/// publishing one terminal is exercising a non-owner surface that
/// the lint cannot meaningfully police without the ownership map.
pub fn check_publisher_terminal_completeness(config: &RalphConfig) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    for (hat_id, hat) in &config.hats {
        for (a, b) in REVIEW_PAIR {
            // P1 narrowing: only enforce on owner hats. A non-owner
            // hat that publishes one terminal (e.g. shipper mirroring
            // review.complete) is a legitimate readback, not a
            // branch owner. If the operator has not declared any
            // owner in `topic_owners`, the rule is silent for the
            // entire pair — there is no branch owner to police.
            let is_owner_of_a = config
                .topic_owners
                .get(*a)
                .map(|owners| owners.iter().any(|o| o == hat_id))
                .unwrap_or(false);
            let is_owner_of_b = config
                .topic_owners
                .get(*b)
                .map(|owners| owners.iter().any(|o| o == hat_id))
                .unwrap_or(false);
            if !is_owner_of_a && !is_owner_of_b {
                continue;
            }
            let publishes_a = hat.publishes.iter().any(|t| t == a);
            let publishes_b = hat.publishes.iter().any(|t| t == b);
            // Exactly one declared → publisher is incomplete. Both or
            // neither is fine (the hat simply does not own this branch
            // decision; "neither" is allowed because some hats are
            // consumers only).
            if !(publishes_a ^ publishes_b) {
                continue;
            }
            let (present, missing) = if publishes_a { (*a, *b) } else { (*b, *a) };
            findings.push(
                LintFinding::error(
                    FINDING_TERMINAL_PUBLISHER_INCOMPLETE,
                    format!(
                        "hat \"{hat_id}\" publishes \'{present}\' but not \'{missing}\'; \
                         these are mutually exclusive branch events, and \"{hat_id}\" is \
                         the registered owner of one of them, so the runtime will reject \
                         the missing publish as an unknown topic."
                    ),
                )
                .with_hat(hat_id.clone())
                .with_topic(missing)
                .with_action_hint(format!(
                    "Add \'{missing}\' to hat \"{hat_id}\" publishes list, or remove \
                     \'{present}\' if the hat does not own this branch decision."
                )),
            );
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RalphConfig;

    fn parse(yaml: &str) -> RalphConfig {
        serde_yaml::from_str(yaml).expect("test yaml must parse")
    }

    /// 2026-06-24 plan U3: REVIEW_PAIR is now empty (10-hat architecture
    /// has no `review.passed` / `review.complete` pair). The lint must
    /// be silent on every preset because there is no pair to police.
    /// These tests pin the empty-pair semantics: regardless of what a
    /// hat declares, no finding is produced until KTD-TTC-2 adds a new
    /// pair to `REVIEW_PAIR`.

    #[test]
    fn empty_pair_silent_on_dual_subscribe() {
        let yaml = r#"
hats:
  observer:
    name: Observer
    description: observes both
    triggers:
      - review.passed
      - review.complete
    publishes:
      - log.entry
event_loop:
  starting_event: work.start
  completion_promise: LOOP_COMPLETE
tasks:
  enabled: false
"#;
        let cfg = parse(yaml);
        let findings = check_reviewer_dual_subscribe(&cfg);
        assert!(
            findings.is_empty(),
            "REVIEW_PAIR is empty; no dual subscribe can be flagged, got {findings:?}"
        );
    }

    #[test]
    fn empty_pair_silent_on_publisher_completeness() {
        let yaml = r#"
hats:
  review-synthesizer:
    name: Synth
    description: synth
    triggers:
      - review.dimensions.complete
    publishes:
      - review.complete
topic_owners:
  review.complete:
    - review-synthesizer
event_loop:
  starting_event: work.start
  completion_promise: LOOP_COMPLETE
tasks:
  enabled: false
"#;
        let cfg = parse(yaml);
        let findings = check_publisher_terminal_completeness(&cfg);
        assert!(
            findings.is_empty(),
            "REVIEW_PAIR is empty; no publisher completeness check can fire, got {findings:?}"
        );
    }

    /// P1-D regression: exemption referencing unknown hat_id does not panic.
    #[test]
    fn exemption_referencing_unknown_hat_does_not_panic() {
        let yaml = r#"
hats:
  a:
    name: A
    description: producer
    triggers: [work.start]
    publishes: [work.ready]
  b:
    name: B
    description: consumer
    triggers: [work.ready]
    publishes: [loop.complete]
event_loop:
  starting_event: work.start
  completion_promise: loop.complete
  review_terminal_coherence_exempt_consumers:
    - nonexistent_hat_typo
tasks:
  enabled: false
"#;
        let cfg = parse(yaml);
        let findings = check_reviewer_dual_subscribe(&cfg);
        assert!(
            findings.is_empty(),
            "no dual subscribe → no findings; orphan exemption must not panic"
        );
    }

    /// KTD-TTC-2 follow-up pins: these pairs are NOT in REVIEW_PAIR
    /// today, so the lint must stay silent. When KTD-TTC-2 adds them,
    /// these tests will need updating.
    #[test]
    fn plan_complete_blocked_dual_subscribe_is_NOT_flagged_in_rtc_scope() {
        let yaml = r#"
hats:
  bad-gate:
    name: Bad Gate
    description: branches on both
    triggers:
      - plan.complete
      - plan.blocked
    publishes:
      - queue.advance
event_loop:
  starting_event: work.start
  completion_promise: LOOP_COMPLETE
tasks:
  enabled: false
"#;
        let cfg = parse(yaml);
        let findings = check_reviewer_dual_subscribe(&cfg);
        assert!(
            findings.is_empty(),
            "plan.* pair is deferred to KTD-TTC-2; must not fire, got {findings:?}"
        );
    }

    #[test]
    fn fix_applied_exhausted_dual_subscribe_is_NOT_flagged_in_rtc_scope() {
        let yaml = r#"
hats:
  bad-fixer:
    name: Bad Fixer
    description: branches on both
    triggers:
      - fix.applied
      - fix.exhausted
    publishes:
      - log.entry
event_loop:
  starting_event: work.start
  completion_promise: LOOP_COMPLETE
tasks:
  enabled: false
"#;
        let cfg = parse(yaml);
        let findings = check_reviewer_dual_subscribe(&cfg);
        assert!(
            findings.is_empty(),
            "fix.* pair is deferred to KTD-TTC-2; must not fire, got {findings:?}"
        );
    }

    #[test]
    fn fixer_missing_fix_exhausted_is_NOT_flagged_in_rtc_scope() {
        let yaml = r#"
hats:
  fixer:
    name: Fixer
    description: applies safe_auto
    triggers:
      - test.failed
    publishes:
      - fix.applied
event_loop:
  starting_event: work.start
  completion_promise: LOOP_COMPLETE
tasks:
  enabled: false
"#;
        let cfg = parse(yaml);
        let findings = check_publisher_terminal_completeness(&cfg);
        assert!(
            findings.is_empty(),
            "fix.* pair is deferred to KTD-TTC-2; must not fire, got {findings:?}"
        );
    }
}
