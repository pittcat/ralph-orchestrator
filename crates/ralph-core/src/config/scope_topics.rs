//! U4 (2026-08-10-002 plan, R10 / M3): canonical scope topic list.
//!
//! Single source of truth for the four `event_policy` topics whose
//! handoff contract is structurally a scope handoff. Both
//! `ralph-core::preset_lint::payload_consistency` (the polarity
//! walker) and `ralph-cli::policy_check::gates` (the `check_scope_handoff_guard`
//! topic switch) consume this list; any future scope topic must be
//! added here so both consumers stay in lockstep.
//!
//! The four topics are also the legal set of `event_policy.schemas`
//! keys the polarity walker is allowed to enumerate when validating
//! `payload_consistency.rules[].when` field references — when a
//! rule's topic is not in this list, the polarity walker is a no-op
//! for that rule (the topic is pipeline-shaped, not scope-shaped).
//!
//! Adding a 5th topic requires:
//! 1. Appending the topic name to [`SCOPE_TOPICS`].
//! 2. Updating the `const _: () = assert!(SCOPE_TOPICS.len() == ...);`
//!    line below to the new count.
//! 3. Adding a `check_<topic>_scope_fields` dispatch arm in
//!    `ralph-cli::policy_check::gates::check_scope_handoff_guard`
//!    plus the corresponding fixture in
//!    `crates/ralph-core/tests/scenarios/`.
//! 4. Extending the polarity walker helper if the new topic has a
//!    different structural-field / threshold-field set.

/// Canonical list of scope handoff topics. Order matches the legacy
/// `SCOPE_TOPICS` literal in
/// `crates/ralph-cli/src/policy_check/gates.rs:869-874` and the
/// `PROTECTED_SCOPE_TOPICS` literal in
/// `crates/ralph-core/src/preset_lint/payload_consistency.rs:63-68`.
/// Any drift between the two legacy constants and this list is a bug.
pub const SCOPE_TOPICS: &[&str] = &[
    "merge.integrated",
    "merge.stabilized",
    "postmerge.changemap.ready",
    "redteam.plan.resolved",
];

/// Compile-time length guard: adding a 5th topic without updating
/// the assertion fails the build. Pair this with an inline test
/// (see `scope_topics::tests`) so runtime behaviour is also pinned.
pub const SCOPE_TOPICS_LEN: usize = SCOPE_TOPICS.len();
const _: () = assert!(SCOPE_TOPICS_LEN == 4);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_topics_contains_four_canonical_names() {
        assert_eq!(SCOPE_TOPICS.len(), 4);
        assert!(SCOPE_TOPICS.contains(&"merge.integrated"));
        assert!(SCOPE_TOPICS.contains(&"merge.stabilized"));
        assert!(SCOPE_TOPICS.contains(&"postmerge.changemap.ready"));
        assert!(SCOPE_TOPICS.contains(&"redteam.plan.resolved"));
    }

    #[test]
    fn scope_topics_has_no_duplicates() {
        let mut sorted: Vec<&str> = SCOPE_TOPICS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SCOPE_TOPICS.len(), "duplicate scope topic");
    }
}
