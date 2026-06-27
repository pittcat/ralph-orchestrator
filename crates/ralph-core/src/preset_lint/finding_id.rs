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

// ──────────────────────────────────────────────────────────────────────────
// WAC-U1 (2026-06-12-002): Workflow Activation Contract finding IDs
// ──────────────────────────────────────────────────────────────────────────

/// R2: A hat triggers on a topic published by another hat but does
/// not declare the topic in its own `publishes` — a re-emit hazard.
///
/// `Warn` in default mode, `Error` in strict. Builtin embedded
/// presets force `Error` regardless of CLI strictness (WAC-U2 R6).
pub const FINDING_RE_EMIT_TRAP: &str = "preset.re_emit_trap";

/// R3: A hat has no activation egress — none of its publishes reach
/// a downstream hat's trigger or a terminal/completion topic within
/// the bounded hop count (≤2 by default).
///
/// `Warn` in default mode, `Error` in strict. Builtin embedded
/// presets force `Error` (WAC-U2 R6).
pub const FINDING_ACTIVATION_EGRESS_MISSING: &str = "preset.activation_egress_missing";

/// R4: A topic with exactly one consumer (a handoff) is consumed by
/// a hat whose own publishes lead to a dead end (no downstream hat
/// trigger or terminal reachable within 2 hops).
///
/// `Warn` in default mode, `Error` in strict. Builtin embedded
/// presets force `Error` (WAC-U2 R6).
pub const FINDING_HANDOFF_PAIRING_BROKEN: &str = "preset.handoff_pairing_broken";

/// R5: A hat triggers on a topic that has no publisher and no
/// subscriber — the trigger can never be satisfied and the workflow
/// stage it represents cannot close.
///
/// `Warn` in default mode, `Error` in strict. Builtin embedded
/// presets force `Error` (WAC-U2 R6).
pub const FINDING_TRIGGER_PUBLISH_ASYMMETRY: &str = "preset.trigger_publish_asymmetry";

/// KTD-6: The handoff topic seed list and the auto-derived unique
/// consumer topics conflict (a seed resolves to a multi-consumer
/// topic in the graph, or vice versa). The derived side wins per
/// KTD-6, and the finding surfaces the conflict for the operator.
///
/// `Warn` in default mode, `Error` in strict. Builtin embedded
/// presets force `Error` (WAC-U2 R6).
pub const FINDING_HANDOFF_SEED_DERIVED_CONFLICT: &str = "preset.handoff_seed_derived_conflict";

// ──────────────────────────────────────────────────────────────────────────
// Plan 001 §4.5 (2026-06-15-001): Schema parity finding IDs
// ──────────────────────────────────────────────────────────────────────────

/// A hat declares a topic in `publishes` but `event_policy.schemas` has no
/// entry for that topic. The CLI pre-publish check would have nothing to
/// validate against; the agent's payload contract is undefined for this
/// topic. Always `Error` in strict mode.
pub const FINDING_PUBLISHES_MISSING_SCHEMA: &str = "preset.publishes_missing_schema";

/// `presets/schemas/<preset>.yml` does not structurally match the inline
/// `event_policy.schemas` block. Editing only one of the two would
/// silently leave the runtime and the reference in disagreement. Always
/// `Error` — checked by the `ce_executor_*_reference_schema_matches_inline`
/// tests in `crates/ralph-cli/src/presets.rs`.
pub const FINDING_SCHEMA_REFERENCE_PARITY: &str = "preset.schema_reference_parity";

// ──────────────────────────────────────────────────────────────────────────
// Plan 2026-06-20-001 U1 KTD-3: state_projection actions_chain order
// ──────────────────────────────────────────────────────────────────────────

/// `state_projection.actions_chain.work.done` does not place
/// `close_task` *before* `mark_step_completed`. The progress gate
/// (`progress_task_gate`) would then see the step **after** the
/// task close and reject the next emit, reintroducing the
/// `ce-executor-serial-primary-20260619` 死循环.
///
/// Always `Error` severity — order is semantic; the engine
/// typestate in `state_projector/mod.rs` is the secondary
/// defence (catches Rust-side dispatch bugs only). Plan
/// 2026-06-20-001 R3 / KTD-3 "主 (primary)" line.
pub const FINDING_WORK_DONE_ACTION_CHAIN_ORDER: &str = "preset.state_projection_work_done_order";

// ──────────────────────────────────────────────────────────────────────────
// 2026-06-23-004 plan U1 KTD-RTC: review terminal coherence
// (renamed to KTD-TTC "terminal coherence" in the 2026-06-24 dual-review
// chain, see docs/solutions/.../ce-executor-serial-mechanism-close-loop-2026-06-23.md).
// The finding IDs are kept stable for back-compat: the OLD review.*-only
// constants are kept as deprecated aliases, and the new TTC family covers
// all pairs in `MUTUALLY_EXCLUSIVE_TERMINAL_PAIRS`.
// ──────────────────────────────────────────────────────────────────────────

/// A downstream hat that triggers on BOTH topics of any mutually
/// exclusive terminal pair (`review.passed` / `review.complete`,
/// `plan.complete` / `plan.blocked`, etc.) will accept whichever
/// arrives first and bypass the publisher's branch decision — most
/// famously the `verdict` distinction between `pass`,
/// `pass_with_residuals`, and `fail`.
///
/// Always `Error` severity. The dual-review chain (2026-06-24)
/// generalized this from `review.*` only to all pairs in
/// `MUTUALLY_EXCLUSIVE_TERMINAL_PAIRS` (see
/// `crates/ralph-core/src/preset_lint/mod.rs`).
pub const FINDING_TERMINAL_DUAL_SUBSCRIBE: &str = "preset.terminal_dual_subscribe";

/// A hat that emits ONE topic of a mutually exclusive terminal pair
/// MUST declare the sibling in its `publishes` set. The publisher
/// branches between the two based on runtime data (residual findings,
/// fix exhaustion, etc.); declaring only one means the runtime will
/// reject the other publish as an unknown topic, silently dropping
/// the terminal.
///
/// Always `Error` severity.
pub const FINDING_TERMINAL_PUBLISHER_INCOMPLETE: &str = "preset.terminal_publisher_incomplete";

/// Back-compat alias for the original (2026-06-23-004 plan U1 KTD-RTC)
/// review-only finding ID. New code should use
/// `FINDING_TERMINAL_DUAL_SUBSCRIBE`; this alias is kept so older
/// diagnostic tools that grep for the historical ID continue to
/// find the finding.
#[deprecated(note = "use FINDING_TERMINAL_DUAL_SUBSCRIBE")]
pub const FINDING_REVIEW_TERMINAL_DUAL_SUBSCRIBE: &str = "preset.terminal_dual_subscribe";

/// Back-compat alias for the original review-only publisher finding.
/// New code should use `FINDING_TERMINAL_PUBLISHER_INCOMPLETE`.
#[deprecated(note = "use FINDING_TERMINAL_PUBLISHER_INCOMPLETE")]
pub const FINDING_REVIEW_TERMINAL_PUBLISHER_INCOMPLETE: &str =
    "preset.terminal_publisher_incomplete";

// ──────────────────────────────────────────────────────────────────────────
// 2026-06-26 plan U2: hat scope invariant finding IDs
// ──────────────────────────────────────────────────────────────────────────

/// 2026-06-26 plan U2: in isolated mode, a hat declared with
/// `event_filter.enabled = false` (or with the field omitted) loses
/// the prompt-side scope enforcement — the agent can see topics the
/// hat is not allowed to react to. Always `Error` severity in
/// isolated mode; the rule does NOT fire in coordinator mode where
/// `event_filter` is intentionally a soft hint.
///
/// Refs: docs/plans/2026-06-26-001-fix-ce-executor-serial-four-recurrences-plan.md
/// R1 (Hat 作用域不变量).
pub const FINDING_HAT_SCOPE_EVENT_FILTER_DISABLED: &str = "preset.hat_scope_event_filter_disabled";

/// 2026-06-26 plan U2: a hat publishes a topic that has no entry in
/// the preset's `topic_deny_rules` and is not on the hat's exempt
/// list. Without an explicit deny rule the topic can be emitted
/// from any context, bypassing the scope invariant that the
/// `publishes` set is meant to enforce. Always `Error` severity.
pub const FINDING_HAT_SCOPE_TOPIC_DENY_INCOMPLETE: &str = "preset.hat_scope_topic_deny_incomplete";

/// 2026-06-26 plan U2: a coordinator hat (one listed in
/// `tasks.coordinator_hats` or the implicit `coordinator` role)
/// declares `event_filter.events` containing any of the
/// `review.*` / `plan.complete` / `plan.blocked` chain topics.
/// The coordinator must NOT see the review chain (its job is to
/// dispatch the workflow, not to react to the verdict) — leaking
/// these topics into its prompt has historically caused the
/// `ce-executor-serial` "fix.applied" / re-review loop where the
/// coordinator pre-empts the reviewer.
///
/// Always `Error` severity.
pub const FINDING_HAT_SCOPE_COORDINATOR_REVIEW_LEAK: &str =
    "preset.hat_scope_coordinator_review_leak";

/// 2026-06-26 Root-Cause Review P1 #2: warn the operator when
/// `verdict_gate.verdict_field` is set to a name that is not
/// the well-known `verdict` / `pass_or_fail` alias. The gate
/// silently treats any payload that does not carry the
/// configured field as "not failing", so a typo here masks
/// `verdict_fail` upstream.
///
/// `Warn` severity — the operator may legitimately use a custom
/// name (the lint cannot verify upstream payload consistency);
/// the warning forces the operator to acknowledge the footgun.
pub const FINDING_HAT_SCOPE_VERDICT_FIELD_UNKNOWN: &str = "preset.hat_scope_verdict_field_unknown";

// ──────────────────────────────────────────────────────────────────────────
// 2026-06-27 plan: mechanism foundation U5 flow declaration finding IDs
// ──────────────────────────────────────────────────────────────────────────

/// Preset has no `mechanism.flow` block. Without a flow
/// declaration the runtime cannot enforce step scope or
/// terminal alignment, so the entire mechanism foundation is
/// inert for this preset. Always `Error` severity.
pub const FINDING_FLOW_DECLARATION_MISSING: &str = "preset.flow_declaration_missing";

/// A step whose `terminal_when` is in `{all_done, any_failed,
/// partial_units_done}` does not declare an `on_partial` map.
/// Without the partial branch the runtime cannot recover from
/// the 4/8 partial-completion case that triggered the
/// 2026-06-26 incident. Always `Error` severity.
pub const FINDING_FLOW_PARTIAL_STATE_UNDECLARED: &str = "preset.flow_partial_state_undeclared";

/// `on_partial.<key>` maps to an empty string. The lint
/// rejects this because an empty emit expression is a silent
/// no-op that swallows the partial state. Always `Error`.
pub const FINDING_FLOW_PARTIAL_BRANCH_EMPTY: &str = "preset.flow_partial_branch_empty";

/// A `terminal_emits` value is missing the well-known
/// `LOOP_COMPLETE` topic. The verdict gate locks this set;
/// the lint surfaces drifts to operators before runtime
/// rejects them. Always `Error`.
pub const FINDING_FLOW_TERMINAL_EMIT_MISSING: &str = "preset.flow_terminal_emit_missing";

/// An `allowed_emits` set in a flow step contains a topic
/// that is not on the well-known topic-format whitelist AND
/// not declared in `event_policy.schemas`. The runtime
/// `FlowStepScopeStage` will reject emits of this topic at
/// runtime; the lint catches it at preset-load time.
pub const FINDING_FLOW_UNKNOWN_EMIT_REJECTED: &str = "preset.flow_unknown_emit_rejected";
