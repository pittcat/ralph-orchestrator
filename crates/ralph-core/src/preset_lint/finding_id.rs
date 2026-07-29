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

/// U8 (plan 2026-07-04-004): `mechanism.flow.<step>.body` contains
/// `review.complete` on a `unit_loop`-shaped step. The unit_loop
/// is `foreach over plan units`; `review.complete` only fires
/// after all units are done via the `review_walk` step. Mixing
/// the two produces a state machine where the runtime tries to
/// route a single per-unit iteration through the per-plan review
/// pipeline — exactly the shape that produced the 2026-07-04
/// silent-success run. Severity: `Error` (structural
/// topology mismatch; not a stylistic warning).
pub const FINDING_FLOW_REVIEW_COMPLETE_IN_UNIT_LOOP_BODY: &str =
    "preset.flow_review_complete_in_unit_loop_body";

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

/// `event_loop.precheck.rules.<X>` is declared with `enabled: true` and the
/// desugar's `<X>.proposed` rewrite is already in circulation, but the
/// effective config has no `precheck-<X>` gate hat (half-desugared state).
/// Without the gate hat the producer's `<X>.proposed` has no consumer —
/// evidence audit + retry budget are silently bypassed.
///
/// `Warn` in default mode, `Error` in strict (mirrors `MissingTopicOwner`
/// semantics — fail-shaped contract drift should not pass `ralph run --strict`).
pub const FINDING_PRECHECK_RULE_WITHOUT_SYNTHESIZED_GATE_HAT: &str =
    "preset.precheck_rule_without_synthesized_gate_hat";

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

// 2026-07-16 cleanup U5 (KTD-5): removed deprecated `FINDING_REVIEW_TERMINAL_DUAL_SUBSCRIBE`
// + `FINDING_REVIEW_TERMINAL_PUBLISHER_INCOMPLETE` aliases. Both had
// rg = 0 callers after U4's compile error surface (the only
// references were the constants' own definition + the
// `FINDING_IDS` array entry below). Diagnostic tools that still
// grep the historical IDs should be migrated to the
// `*_TERMINAL_*` constants (which carry the same string value).

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

/// 2026-06-29 plan 2026-06-29-007 U2: a coordinator hat declares
/// `human.guidance` or `loop.stalled` in its `publishes` /
/// `default_publishes`. `human.guidance` has been removed from the
/// protocol; `loop.stalled` is owned by loop-level fallback hats
/// such as `progress-steward`. The coordinator must NOT publish
/// either topic.
///
/// Always `Error` severity.
pub const FINDING_HAT_SCOPE_COORDINATOR_FORBIDDEN_PUBLISH: &str =
    "preset.hat_scope_coordinator_forbidden_publish";

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

// ──────────────────────────────────────────────────────────────────────────
// 2026-06-28 plan U12: metadata-runtime drift
// ──────────────────────────────────────────────────────────────────────────

/// `mechanism.*` value disagrees with the runtime's accepted
/// set (e.g. `state_idempotency: maybe`,
/// `enforce_schema: soft`, `repair_budget: 0`). Surfaced as
/// `Error` so the preset fails to load — U7 makes the runtime
/// half of this contract a hard panic, U12 makes the
/// preset-half a hard lint.
pub const FINDING_METADATA_RUNTIME_DRIFT: &str = "preset.metadata_runtime_drift";

// ──────────────────────────────────────────────────────────────────────────
// 2026-07-03-002 plan U1: fix-unit task_id minting lint finding ID
// ──────────────────────────────────────────────────────────────────────────

/// 2026-07-03-002 plan U1: coordinator fix-unit dispatch 没有给出
/// `ralph tools task create` 调用模板或 `Task::fix_unit_task_id` shape 示范。
/// 093813 根因:preset 有 `MUST be freshly minted` 文案但无 CLI 参数模板,
/// agent 推不出参数导致手写 id 被 state_projector 拒。Always `Error`.
pub const FINDING_FIX_UNIT_TASK_ID_NOT_HELPER_DERIVED: &str =
    "preset.fix_unit_task_id_not_helper_derived";

// ──────────────────────────────────────────────────────────────────────────
// 2026-07-03-001 plan U9: supervisor preset lint finding IDs
// ──────────────────────────────────────────────────────────────────────────

/// `event_loop.supervisor.enabled: true` was declared without
/// `event_loop.execution_mode: isolated`. Per R4 the supervisor
/// path requires the isolated mode contract (3-hat coordinator
/// ceiling does not apply, but isolation is mandatory).
/// Always `Error` so the preset fails to load with a clear
/// remediation hint.
pub const FINDING_SUPERVISOR_REQUIRES_ISOLATED: &str = "preset.supervisor_requires_isolated";

/// An integrator-style hat (`exec-integrator` /
/// `fix-integrator`) declares `*.unit.done` (the per-slot
/// `exec.unit.done` / `fix.unit.done` topic) in its
/// `triggers:` list. Per KTD-6 the integrator's handoff
/// trigger is the wave-complete coord event, NOT the
/// per-slot done topic. `*.unit.done` belongs to the
/// worker fan-out side; including it in the integrator's
/// triggers leaks slot-level semantics into the merge path.
/// Always `Error`.
pub const FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE: &str =
    "preset.supervisor_integrator_triggers_slot_done";

/// A hat's `publishes:` list contains one of the six
/// supervisor coordination topics (`*.wave.complete` /
/// `*.wave.failed`). Per R14 only the supervisor itself may
/// inject those; agents that declare them as `publishes`
/// will silently lose their emits to the origin guard. The
/// lint surfaces the misconfiguration at preset-load time.
/// Always `Error`.
pub const FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC: &str =
    "preset.supervisor_hat_publishes_coord_topic";

/// 2026-07-22 plan U3 (R5): a wave consumer hat (one whose
/// `triggers:` includes a `*.unit.ready` topic) declares
/// `concurrency <= 1` (the default). The runtime wave
/// detector in `wave_detection.rs` rejects such hats as
/// `SequentialTarget`, silently dropping the entire wave
/// batch the dispatcher published. This lint forces the
/// author to explicitly opt in to wave concurrency by
/// setting `concurrency > 1` on every `*.unit.ready`
/// consumer. Always `Error` severity so the preset fails
/// to load with a clear remediation hint before runtime
/// discovers the same gap.
pub const FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY: &str =
    "preset.supervisor_wave_consumer_low_concurrency";

/// 2026-07-23-005 plan U2: `task-planner` is the dependency auditor
/// and must NOT claim the wave coordination topic `exec.unit.ready`
/// in its `publishes:` list. That ownership transfers to the
/// exec-wave dispatcher hat in U5; emitting `exec.unit.ready` from
/// `task-planner` would re-introduce the single-shot fan-out that
/// U2 just removed. Always `Error` severity so the preset fails to
/// load with a clear remediation hint.
pub const FINDING_SUPERVISOR_TASK_PLANNER_PUBLISHES_EXEC_READY: &str =
    "preset.supervisor_task_planner_publishes_exec_unit_ready";

/// 2026-07-23-005 plan U2: `task-planner` is the dependency
/// auditor and must NOT have `exec.unit.ready` in `triggers:` —
/// the hat does not consume per-unit readiness events (that is
/// the wave dispatcher's job). Allowing it to consume the topic
/// would silently re-route fan-out through `task-planner`.
pub const FINDING_SUPERVISOR_TASK_PLANNER_TRIGGERS_EXEC_READY: &str =
    "preset.supervisor_task_planner_triggers_exec_unit_ready";

/// 2026-07-23-005 plan U7: `alignment` is a read-only verifier
/// and must NOT emit any wave dispatch topic. If it lists
/// `*.unit.ready` in `publishes:`, it has become a second
/// dispatcher and bypasses the formal fix chain (U7 hard rule).
pub const FINDING_SUPERVISOR_ALIGNMENT_PUBLISHES_WAVE_READY: &str =
    "preset.supervisor_alignment_publishes_wave_ready";

/// 2026-07-23-005 plan U7: `alignment` must NOT consume
/// per-unit fan-out topics either. Same rationale as the
/// publishes-side sibling finding.
pub const FINDING_SUPERVISOR_ALIGNMENT_TRIGGERS_WAVE_READY: &str =
    "preset.supervisor_alignment_triggers_wave_ready";

/// 2026-07-23-005 plan U8: the deleted hats
/// (`progress-steward`, `shipper`, `fixer`) MUST NOT
/// appear in any supervisor preset. The lint is a
/// hard error because each of these hats was deleted
/// for a specific architectural reason (single owner of
/// reporting, no fallback fixer, no progress rescue) and
/// resurrecting them silently regresses the topology.
pub const FINDING_SUPERVISOR_DELETED_HAT_REINSTATED: &str =
    "preset.supervisor_deleted_hat_reinstated";

/// 2026-07-23-005 plan U8: the deleted hats must not
/// appear anywhere in the preset (not just `hats:` —
/// also no orphan trigger publishes / deny rules /
/// state-projection references). The lint walks every
/// string-typed value in the preset and reports any
/// match.
pub const FINDING_SUPERVISOR_DELETED_HAT_REFERENCED: &str =
    "preset.supervisor_deleted_hat_referenced";

// ──────────────────────────────────────────────────────────────────────────
// OPAC instructions lint finding IDs (2026-07-04-001 plan U11)
// ──────────────────────────────────────────────────────────────────────────

/// Hat instructions require task creation/mutation although the hat is not
/// authorized to write tasks, or a projection-owned task writer conflicts with
/// an agent-side mutation instruction.
pub const FINDING_INSTRUCTIONS_TASK_MUTATION_AUTHORITY_CONFLICT: &str =
    "preset.instructions_task_mutation_authority_conflict";

/// (or `ralph task create`) command. The real CLI exposes only `add` and
/// `ensure`. Agents copy-pasting this string will hit a hard CLI error at
/// runtime; surface the anti-pattern at preset-load time.
/// Default `Error`; `--strict` makes it fail preset startup.
pub const FINDING_INSTRUCTIONS_TASK_CREATE_LITERAL: &str =
    "preset.instructions_task_create_literal";

/// A coordinator hat (or hat whose `publishes` contains `work.ready`)
/// references `fix-unit` / `fix unit` / `fresh mint` in its instructions
/// but does NOT cite `task ensure --for-fix-unit` (or
/// `ensure.*--for-fix-unit`). Per 002 plan U14a, the canonical fix-unit
/// mint path is `ralph tools task ensure --for-fix-unit` — any other shape
/// will produce stale task ids that break the step handoff chain.
/// Always `Error`.
pub const FINDING_INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING: &str =
    "preset.instructions_fix_unit_mint_template_missing";

/// Hat `instructions:` include non-empty `publishes` but neither
/// `ralph-tools-opac` nor `ralph-tools-emit` §5 precheck is cited. Per
/// 2026-07-04-001 plan R12 agents should *reference* the OPAC skill rather
/// than copy command strings — without the reference the agent cannot
/// reliably reach the Observe / Precheck / Confirm stages.
/// Always `Error`.
pub const FINDING_INSTRUCTIONS_OPAC_SKILL_REFERENCE_MISSING: &str =
    "preset.instructions_opac_skill_reference_missing";

/// Hat `instructions:` reference reading or tailing internal Ralph state
/// files that are runtime-private (`.ralph/events.jsonl`,
/// `.ralph/supervisor.db`, `.ralph/loops.json`) or call
/// `ralph diagnose --supervisor`. Per HARD RULE 4, agents must NOT reach
/// into runtime ledgers directly; they go through `ralph tools task …`
/// or `ralph inspect loop` instead.
/// Always `Error`.
pub const FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER: &str =
    "preset.instructions_read_internal_ledger";

/// Hat `instructions:` direct the agent to `ralph emit` (or
/// `ralph wave emit`) a supervisor-only coordination topic
/// (`*.wave.complete` / `*.unit.ready` etc.). Per `event_origin`
/// these are denied for agent origin, so the agent's emit will silently
/// drop — surfaces as the F-019 incident in
/// `presets/en/ce-executor-supervisor.yml`.
/// Always `Error`.
pub const FINDING_INSTRUCTIONS_SUPERVISOR_COORDINATION_TOPIC: &str =
    "preset.instructions_supervisor_coordination_topic";

/// 2026-07-09-001 plan (U7): hat `instructions:` direct the
/// agent to build / shape / fix a `ralph emit` payload but
/// do **not** cite the new `ralph-tools-emit` policy-check
/// feedback section. The agent ends up re-deriving field
/// shapes from stale inline text instead of the U3
/// enrichment layer (`field_description`,
/// `suggested_payload_shape`, `suggested_command`).
///
/// The rule only fires when:
/// - the hat publishes a non-empty topics list, AND
/// - the hat's `instructions` mention `payload` /
///   `ralph emit` / `ralph wave emit` /
///   `field shape` / `required fields`, AND
/// - the hat's `instructions` do not cite the U3
///   `ralph-tools-emit` feedback section (e.g.
///   "enrichment fields" or "policy-check feedback").
///
/// Always `Error` for builtin / high-risk presets
/// (Phase 1); the lint scope is tightened in
/// `check_emit_feedback_skill_reference`.
pub const FINDING_INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING: &str =
    "preset.instructions_emit_feedback_skill_reference_missing";

// ──────────────────────────────────────────────────────────────────────────
// U3 + U4 of plan 2026-07-04-004: review-synthesizer + coordinator
// routing drift guards. The two rules below are *drift* guards
// rather than correctness checks: they fire when the agent-facing
// text drifts away from the "全 6 维度 + findings_count==0"
// invariants that the runtime depends on for silent-success
// detection. Future edits that loosen the wording are caught
// here before they ship.
// ──────────────────────────────────────────────────────────────────────────

/// U3 (plan 2026-07-04-004): review-synthesizer's
/// `all_dimensions_failed` hard-gate text drifted away from the
/// "全 6 维度 status == failed" explicit invariant. The runtime
/// reads the synthesized text to decide whether the agent
/// published `plan.blocked` vs `plan.complete`; loose wording
/// (e.g. "All dimensions failed", "if any dimension failed") is
/// exactly what produced the 2026-07-04 silent-success run.
pub const FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD: &str = "preset.review_synthesizer_block_guard";

// ──────────────────────────────────────────────────────────────────────────
// 2026-07-09-003 plan (U4): schema-backed trigger context lint
// finding IDs. R2 / R8 / R9 / R11 / R19 / R20 / SC5.
// ──────────────────────────────────────────────────────────────────────────

/// U4 (plan 2026-07-09-003): `trigger_context.summary_fields` (or
/// a hint condition's `field`) references a field that is not in
/// `required_fields ∪ known_fields ∪ field_docs.keys() ∪
/// allowed_values.keys()`. R2 / R19. Always `Error`.
pub const FINDING_TRIGGER_CONTEXT_UNKNOWN_FIELD: &str = "preset.trigger_context_unknown_field";

/// U4 (plan 2026-07-09-003): hint condition uses an
/// `op` that is not in the v1 allowlist
/// (`eq` / `ne` / `gt` / `gte` / `lt` / `lte` / `exists` /
/// `missing`). The `HintOp::Unknown` variant preserves the
/// original string at parse time so this finding can pin the
/// exact unsupported predicate. R8 / R20. Always `Error`.
pub const FINDING_TRIGGER_CONTEXT_UNSUPPORTED_PREDICATE: &str =
    "preset.trigger_context_unsupported_predicate";

/// U4 (plan 2026-07-09-003): hint condition has a `value` field
/// shape that does not match the `op` — comparison ops
/// (`gt` / `gte` / `lt` / `lte`) require a JSON number, and
/// `exists` / `missing` must not carry a `value`. R8 / R9.
/// Always `Error`.
pub const FINDING_TRIGGER_CONTEXT_VALUE_SHAPE: &str = "preset.trigger_context_value_shape";

/// U4 (plan 2026-07-09-003): two routing hints inside the same
/// `trigger_context` declare the same `label`. The label is the
/// stable identifier agent skill docs and BDD scenarios refer
/// to; duplicates silently scramble the matched-hint sequence
/// agents see. R11 / SC5. Always `Error`.
pub const FINDING_TRIGGER_CONTEXT_DUPLICATE_LABEL: &str = "preset.trigger_context_duplicate_label";

/// U5 (plan 2026-07-09-003): `trigger_context` is declared for
/// a topic but no hat subscribes to that topic. The block
/// would never reach a downstream hat's prompt, so the
/// declaration is dead. R21 / R22. Always `Error`.
pub const FINDING_TRIGGER_CONTEXT_NO_CONSUMER: &str = "preset.trigger_context_no_consumer";

/// U5 (plan 2026-07-22-003): the `trigger_context` block
/// references a source topic, but a hat that does subscribe to
/// that topic is misconfigured in a way that would still let
/// the block leak. Reserved for future use; current U5 work
/// is no-op here.
pub const FINDING_TRIGGER_CONTEXT_TOPOLOGY_LEAK: &str = "preset.trigger_context_topology_leak";

// ──────────────────────────────────────────────────────────────────────────
// plan 2026-07-22-004 (U5): payload_consistency rule sanity lint
// finding IDs. R6 / R3 / S6 / S3. The `payload_consistency` prefix is
// deliberately distinct from the `trigger_context_*` family above so a
// payload-consistency rule finding can never collide with a
// trigger-context finding (both validate a different `event_policy`
// block and both enumerate predicate field references).
// ──────────────────────────────────────────────────────────────────────────

/// U5 (plan 2026-07-22-004): two `event_policy.payload_consistency`
/// rules in the same preset share an `id`. The id is embedded in the
/// runtime `payload_consistency:<id>` gate, so duplicates scramble the
/// agent-facing rejection reason. `Warn` in default mode, `Error` in
/// strict.
pub const FINDING_PAYLOAD_CONSISTENCY_DUPLICATE_ID: &str =
    "preset.payload_consistency_duplicate_id";

/// U5 (plan 2026-07-22-004): a `payload_consistency` rule targets a
/// `topic` with no entry in `event_policy.schemas`. Almost always a
/// typo; the rule references a topic the policy does not validate.
/// `Warn` in default mode, `Error` in strict.
pub const FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_TOPIC: &str =
    "preset.payload_consistency_unknown_topic";

/// U5 (plan 2026-07-22-004): a `field` referenced anywhere in a
/// `payload_consistency` rule's `when` (recursively through `all` /
/// `any`) is not declared on the topic's schema (`required_fields ∪
/// known_fields ∪ field_docs ∪ allowed_values ∪ element_constraints`).
/// The runtime evaluator treats the missing field as a miss, so the
/// predicate can never fire as authored — a silent correctness bug.
/// `Warn` in default mode, `Error` in strict.
pub const FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_FIELD: &str =
    "preset.payload_consistency_unknown_field";

/// U3 (fix-plan 2026-07-22-004 adversarial:A1): a `payload_consistency`
/// rule's `when` references an op that is not in the runtime whitelist
/// (`eq` / `ne` / `gt` / `gte` / `exists` / `non_empty`). The runtime
/// evaluator treats unknown ops as fail-close `Hit`, which silently
/// turns the gated topic into a hard reject. The lint surfaces this
/// at preset-load time instead. `Warn` in default mode, `Error` in
/// strict.
pub const FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_OP: &str = "preset.payload_consistency_unknown_op";

/// U3 (fix-plan 2026-07-22-004 adversarial:A1): a `payload_consistency`
/// rule's `when` is not a JSON object (it is a scalar, array, or null).
/// The runtime evaluator treats non-object `when` as fail-close `Hit`.
/// The lint surfaces this at preset-load time so the rule author can
/// rewrite the `when` as `{all:[...]}` / `{any:[...]}` or a single
/// predicate object. `Warn` in default mode, `Error` in strict.
pub const FINDING_PAYLOAD_CONSISTENCY_NON_OBJECT_WHEN: &str =
    "preset.payload_consistency_non_object_when";

/// U3 (2026-07-23-002 plan, KTD3): a `payload_consistency` rule's
/// `message` exceeds the maximum byte length (1024 UTF-8 bytes) or
/// contains unsafe characters (ANSI escapes, C0/C1 control chars
/// except `\n`/`\t`, zero-width characters). The runtime
/// `safe_display` API strips these at render time, but the lint
/// surfaces the misconfiguration at preset-load time so the rule
/// author fixes the message rather than relying on runtime
/// truncation/stripping. `Warn` in default mode, `Error` in strict.
pub const FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE: &str =
    "preset.payload_consistency_unsafe_message";

/// 2026-07-28-001 plan U4 (R8): a non-final `kind: linear` step
/// declares at least two allowed emits but NO forward step has an
/// `on` / `on_any_of` that names any of those topics. The runtime
/// falls back to positional advance, which silently produces the
/// `flow_drift_positional_fallback` class of bug (e.g. the
/// 2026-07-27 parallel-forge primary run where `forge.plan.inspected`
/// landed in `exec_wave` instead of `plan_authoring`).
///
/// `Error` in strict mode (the only mode that surfaces this — the
/// default mode is permissive for legacy presets). The rule is
/// local to a single non-final `kind: linear` step; non-linear
/// (`side_effect` / `await` / `foreach` / `sequence` / `terminal`)
/// steps are exempt because their transition model is different.
pub const FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY: &str =
    "preset.flow_linear_positional_ambiguity";

/// 2026-07-29-001 plan U1 (R5): a step declares a topic in
/// `transition_emits` that is NOT in its own `allowed_emits`.
/// Such a topic can never be accepted by FlowStepScopeStage,
/// so declaring it as a transition signal is a dead contract.
/// Surfaces the "transition emits a topic the step does not allow"
/// anti-pattern at preset-load time instead of silently dropping
/// the transition at runtime.
pub const FINDING_FLOW_TRANSITION_EMIT_NOT_IN_ALLOWED: &str =
    "preset.flow_transition_emit_not_in_allowed";

/// 2026-07-29-001 plan U1 (R5): a step declares a transition
/// topic that has no forward step in the flow with an
/// `on` / `on_any_of` naming that topic. Authoring a transition
/// without a forward target is the same class of bug as a
/// linear step with no `on` — the runtime falls through to
/// legacy positional advance, which can silently misroute
/// the flow.
pub const FINDING_FLOW_TRANSITION_EMIT_NO_FORWARD_TARGET: &str =
    "preset.flow_transition_emit_no_forward_target";

// ──────────────────────────────────────────────────────────────────────────
// 2026-07-29-003 plan U1: strict read-only hat lint finding IDs
// ──────────────────────────────────────────────────────────────────────────

/// 2026-07-29-003 plan U1: a strict read-only hat (denies both Edit and
/// Write) declares no `allowed_write_paths` contract. Without the contract
/// the workspace mutation guard cannot filter expected deltas, so every
/// delta is a violation. Always `Error`.
pub const FINDING_STRICT_READONLY_MISSING_WRITE_CONTRACT: &str =
    "preset.strict_readonly_missing_write_contract";

/// 2026-07-29-003 plan U1: an `allowed_write_paths` entry fails
/// `workspace_mutation_guard::validate_allowed_path`. Always `Error`.
pub const FINDING_STRICT_READONLY_INVALID_WRITE_PATH: &str =
    "preset.strict_readonly_invalid_write_path";

/// Inventory of every finding id in this module. Use this in tests
/// that assert the lint surface does not silently re-introduce a
/// serial-only or coordinator-loop finding. Plan 2026-07-07-006
/// Unit 4 Step 4.6.
pub const ALL_FINDING_IDS: &[&str] = &[
    FINDING_INVALID_TOPIC_FORMAT,
    FINDING_WHITELIST_EXEMPT_TOPIC,
    FINDING_FLOW_REVIEW_COMPLETE_IN_UNIT_LOOP_BODY,
    FINDING_OWNER_UNKNOWN_HAT,
    FINDING_OWNER_NOT_PUBLISHER,
    FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH,
    FINDING_MISSING_TOPIC_OWNER,
    FINDING_COORDINATOR_MISSING,
    FINDING_TASK_PUBLISHER_NOT_COORDINATED,
    FINDING_MULTI_HAT_REQUIRES_ISOLATED,
    FINDING_RE_EMIT_TRAP,
    FINDING_ACTIVATION_EGRESS_MISSING,
    FINDING_HANDOFF_PAIRING_BROKEN,
    FINDING_TRIGGER_PUBLISH_ASYMMETRY,
    FINDING_HANDOFF_SEED_DERIVED_CONFLICT,
    FINDING_PUBLISHES_MISSING_SCHEMA,
    FINDING_SCHEMA_REFERENCE_PARITY,
    FINDING_WORK_DONE_ACTION_CHAIN_ORDER,
    FINDING_TERMINAL_DUAL_SUBSCRIBE,
    FINDING_TERMINAL_PUBLISHER_INCOMPLETE,
    FINDING_HAT_SCOPE_EVENT_FILTER_DISABLED,
    FINDING_HAT_SCOPE_TOPIC_DENY_INCOMPLETE,
    FINDING_HAT_SCOPE_COORDINATOR_REVIEW_LEAK,
    FINDING_HAT_SCOPE_COORDINATOR_FORBIDDEN_PUBLISH,
    FINDING_HAT_SCOPE_VERDICT_FIELD_UNKNOWN,
    FINDING_FLOW_DECLARATION_MISSING,
    FINDING_FLOW_PARTIAL_STATE_UNDECLARED,
    FINDING_FLOW_PARTIAL_BRANCH_EMPTY,
    FINDING_FLOW_TERMINAL_EMIT_MISSING,
    FINDING_FLOW_UNKNOWN_EMIT_REJECTED,
    FINDING_METADATA_RUNTIME_DRIFT,
    FINDING_FIX_UNIT_TASK_ID_NOT_HELPER_DERIVED,
    FINDING_SUPERVISOR_REQUIRES_ISOLATED,
    FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE,
    FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC,
    FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY,
    FINDING_SUPERVISOR_TASK_PLANNER_PUBLISHES_EXEC_READY,
    FINDING_SUPERVISOR_TASK_PLANNER_TRIGGERS_EXEC_READY,
    FINDING_SUPERVISOR_ALIGNMENT_PUBLISHES_WAVE_READY,
    FINDING_SUPERVISOR_ALIGNMENT_TRIGGERS_WAVE_READY,
    FINDING_SUPERVISOR_DELETED_HAT_REINSTATED,
    FINDING_SUPERVISOR_DELETED_HAT_REFERENCED,
    FINDING_INSTRUCTIONS_TASK_CREATE_LITERAL,
    FINDING_INSTRUCTIONS_FIX_UNIT_MINT_TEMPLATE_MISSING,
    FINDING_INSTRUCTIONS_OPAC_SKILL_REFERENCE_MISSING,
    FINDING_INSTRUCTIONS_READ_INTERNAL_LEDGER,
    FINDING_INSTRUCTIONS_SUPERVISOR_COORDINATION_TOPIC,
    FINDING_INSTRUCTIONS_EMIT_FEEDBACK_SKILL_REFERENCE_MISSING,
    FINDING_INSTRUCTIONS_TASK_MUTATION_AUTHORITY_CONFLICT,
    FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD,
    FINDING_TRIGGER_CONTEXT_UNKNOWN_FIELD,
    FINDING_TRIGGER_CONTEXT_UNSUPPORTED_PREDICATE,
    FINDING_TRIGGER_CONTEXT_VALUE_SHAPE,
    FINDING_TRIGGER_CONTEXT_DUPLICATE_LABEL,
    FINDING_TRIGGER_CONTEXT_NO_CONSUMER,
    FINDING_TRIGGER_CONTEXT_TOPOLOGY_LEAK,
    FINDING_PAYLOAD_CONSISTENCY_DUPLICATE_ID,
    FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_TOPIC,
    FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_FIELD,
    FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_OP,
    FINDING_PAYLOAD_CONSISTENCY_NON_OBJECT_WHEN,
    FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE,
    FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY,
    FINDING_PRECHECK_RULE_WITHOUT_SYNTHESIZED_GATE_HAT,
    FINDING_FLOW_TRANSITION_EMIT_NOT_IN_ALLOWED,
    FINDING_FLOW_TRANSITION_EMIT_NO_FORWARD_TARGET,
    FINDING_STRICT_READONLY_MISSING_WRITE_CONTRACT,
    FINDING_STRICT_READONLY_INVALID_WRITE_PATH,
];
