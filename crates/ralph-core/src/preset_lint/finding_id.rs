//! Finding ID constants for preset_lint rules.
//!
//! These constants are part of the public contract — callers (e.g.
//! `runtime_contract` tests, dashboard) reference them by string value.
//!
//! Implementation Plan Unit: shared across U1/U2/U3 of
//! `2026-06-08-003-feat-preset-static-lint-plan`.

// ──────────────────────────────────────────────────────────────────────────
// U1: Topic format finding IDs
// ──────────────────────────────────────────────────────────────────────────

/// Stable machine ID for a topic that violates the lowercase dot-case format
/// and is NOT in the whitelist.
pub const FINDING_INVALID_TOPIC_FORMAT: &str = "preset.invalid_topic_format";

/// Stable machine ID for a topic that matches the whitelist — reported as
/// `Pass` severity for informational purposes.
pub const FINDING_WHITELIST_EXEMPT_TOPIC: &str = "preset.whitelist_exempt_topic";

// ──────────────────────────────────────────────────────────────────────────
// U2: Ownership & coordinator finding IDs
// ──────────────────────────────────────────────────────────────────────────

/// `topic_owners` references a hat that does not exist in the config.
///
/// Always `Error` severity (regardless of strict mode).
pub const FINDING_OWNER_UNKNOWN_HAT: &str = "preset.owner_unknown_hat";

/// The owner hat of a topic does not declare that topic in its
/// `publishes` or `default_publishes`.
///
/// `Warn` in default mode, `Error` in strict.
pub const FINDING_OWNER_NOT_PUBLISHER: &str = "preset.owner_not_publisher";

/// A non-owner hat publishes a topic that has a declared owner.
///
/// `Warn` in default mode, `Error` in strict.
pub const FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH: &str = "preset.cross_hat_unauthorized_publish";

/// A topic is declared in `topic_owners` but no hat publishes it.
///
/// `Warn` in default mode, `Error` in strict.
pub const FINDING_MISSING_TOPIC_OWNER: &str = "preset.missing_topic_owner";

/// `tasks.enabled=true` but `tasks.coordinator_hats` is empty.
///
/// Always `Error` severity.
pub const FINDING_COORDINATOR_MISSING: &str = "preset.coordinator_missing";

/// A hat publishes a `task.*` topic but is not listed in
/// `tasks.coordinator_hats`.
///
/// Always `Error` severity.
pub const FINDING_TASK_PUBLISHER_NOT_COORDINATED: &str = "preset.task_publisher_not_coordinated";

// ──────────────────────────────────────────────────────────────────────────
// U1 of 2026-06-11-003: Multi-hat isolation policy
// ──────────────────────────────────────────────────────────────────────────

/// Preset declares more than [`crate::config::MULTI_HAT_ISOLATION_LIMIT`]
/// hats while `event_loop.execution_mode` is `coordinator` (explicit
/// or default). The policy requires `execution_mode: isolated` once
/// the threshold is crossed.
///
/// Always `Error` severity — the rule is never downgraded by
/// `LintStrictness` and admits no configuration, env var, test
/// switch, or hidden compat opt-out (R1-R5).
pub const FINDING_MULTI_HAT_REQUIRES_ISOLATED: &str = "preset.multi_hat_requires_isolated";
