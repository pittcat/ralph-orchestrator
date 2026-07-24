//! Preset Static Lint — entry point and shared types.
//!
//! This module is the public face of the preset-lint subsystem. It hosts
//! shared types (`LintSeverity`, `LintFinding`, `LintStrictness`), the
//! U3 orchestrator (`run_preset_lint`), and the lint→contract adapter.
//! The four rule families live in sibling modules:
//!
//! - [`finding_id`] — stable finding ID constants.
//! - [`topic_format`] — U1 topic format rules + surface enumeration.
//! - [`ownership`] — U2 R2/R3/R4 ownership rules.
//! - [`coordinator`] — U2 R5 coordinator rules.
//!
//! Implementation Plan Unit: U1/U2/U3 of `2026-06-08-003-feat-preset-static-lint-plan`.
//!
//! Stability rules:
//! - The `finding_id` constants are part of the public contract.
//! - The `TopicSurface` enum variants are source of truth for which
//!   config locations are linted.
//! - `TopicOccurrence` fields (`topic`, `surface`, `hat`) are stable.

use crate::config::RalphConfig;
use crate::runtime_contract::{
    FindingSeverity, FindingSource, FindingStage, RuntimeContractFinding,
};

pub mod coordinator;
pub mod dimension_reviewer_write_paths;
pub mod finding_id;
pub mod fix_unit_task_id;
pub mod flow_declaration;
pub mod hat_scope_invariant;
// 2026-07-04-001 plan U11: instructions OPAC lint rule family.
// Catches five anti-patterns in hat `instructions:` text that cause
// agent misbehavior in isolated mode (fictional `task create`,
// reading runtime-private ledgers, emit of supervisor coord topics,
// missing OPAC skill reference, missing fix-unit mint template).
pub mod instructions_opac;
pub mod metadata_runtime_drift;
pub mod multi_hat;
pub mod ownership;
// plan 2026-07-22-004 (U5): payload_consistency rule sanity lint.
// Validates every `event_policy.payload_consistency.rules[]` entry
// (unique id, known topic, referenced fields declared on the topic
// schema) so a misconfigured gate fails close at preset-load time
// instead of silently rejecting every gated event at runtime.
pub mod payload_consistency;
// 2026-07-04-004 plan U3: review-synthesizer block-guard drift
// lint. Catches presets whose `review-synthesizer` `instructions:`
// drift away from the explicit "all 6 dimensions status == failed"
// invariant that the runtime relies on for silent-success detection.
pub mod review_synthesizer_block_guard;
pub mod schema_parity;
// 2026-07-03-001 plan U9: supervisor preset lint rule family.
// Three rules (R-SW-1, R-SW-2, R-COORD-4) over the raw preset
// YAML. Pure-YAML entry point so the lint is stable across
// `RalphConfig` refactors. Wired into `run_preset_lint` near
// the phase_authority block so the operator sees a single
// coordinated report.
pub mod state_projection;
pub mod supervisor;
pub mod topic_format;
/// 2026-07-09-003 plan (U4): schema-backed trigger context
/// static lint. Catches unknown `summary_fields` / condition
/// field references, unsupported predicate ops, value-shape
/// mismatches, and duplicate hint labels. Strict-only by
/// design (R3 / R29) — default mode skips the check entirely
/// so undeclared presets see no behaviour change.
pub mod trigger_context;
pub mod workflow_activation;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod supervisor_preset_test;

pub use coordinator::check_coordinator_rules;
pub use finding_id::{
    FINDING_ACTIVATION_EGRESS_MISSING, FINDING_COORDINATOR_MISSING,
    FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH, FINDING_FLOW_DECLARATION_MISSING,
    FINDING_FLOW_PARTIAL_BRANCH_EMPTY, FINDING_FLOW_PARTIAL_STATE_UNDECLARED,
    FINDING_FLOW_TERMINAL_EMIT_MISSING, FINDING_FLOW_UNKNOWN_EMIT_REJECTED,
    FINDING_HANDOFF_PAIRING_BROKEN, FINDING_HANDOFF_SEED_DERIVED_CONFLICT,
    FINDING_INVALID_TOPIC_FORMAT, FINDING_MISSING_TOPIC_OWNER, FINDING_MULTI_HAT_REQUIRES_ISOLATED,
    FINDING_OWNER_NOT_PUBLISHER, FINDING_OWNER_UNKNOWN_HAT, FINDING_RE_EMIT_TRAP,
    FINDING_TASK_PUBLISHER_NOT_COORDINATED, FINDING_TERMINAL_DUAL_SUBSCRIBE,
    FINDING_TERMINAL_PUBLISHER_INCOMPLETE, FINDING_TRIGGER_PUBLISH_ASYMMETRY,
    FINDING_WHITELIST_EXEMPT_TOPIC, FINDING_WORK_DONE_ACTION_CHAIN_ORDER,
};
pub use flow_declaration::check_flow_declaration;

// Re-export the WAC top-level entry point so callers (and the
// WAC-U8 BDD scenarios) can invoke the rule family without
// reaching into the module directly. As of WRC-U1 (2026-06-12-003),
// the function is wired into `run_preset_lint`: WAC findings are
// always produced (severity graded by `LintStrictness`), and the
// full report surfaces through `ralph preset check` and the
// `enforce_preset_lint_gate` startup hard gate. See plan
// `2026-06-12-003-feat-wac-rollout-completion-plan.md` (WRC-U1) and
// `2026-06-12-002-feat-workflow-activation-contract-plan.md`
// (KTD-2: WAC always-on, severity by strictness).
pub use dimension_reviewer_write_paths::{
    FINDING_DIMENSION_REVIEWER_WRITE_PLAN, check_dimension_reviewer_write_paths,
};
pub use fix_unit_task_id::{
    FINDING_FIX_UNIT_TASK_ID_NOT_HELPER_DERIVED, check_fix_unit_task_id_helper_derived,
};
pub use hat_scope_invariant::check_hat_scope_invariant;
pub use instructions_opac::{check_instructions_opac, check_instructions_opac_with_preset};
pub use metadata_runtime_drift::check_metadata_runtime_drift;
pub use multi_hat::check_multi_hat_isolation;
// 2026-07-04-004 plan U3: review-synthesizer block-guard drift
// lint exported here so `run_preset_lint` (further down) and BDD
// scenarios can call it without reaching into the submodule.
pub use review_synthesizer_block_guard::{
    FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD, check_review_synthesizer_block_guard,
};
// 2026-07-04-004 plan U4: review-complete misrouting drift lint
// exported alongside U3 so callers have a single import surface.
pub use ownership::{check_owner_references, check_ownership_rules};
pub use payload_consistency::check_payload_consistency;
pub use state_projection::check_work_done_action_chain_order;
// 2026-07-03-001 plan U9: export the supervisor lint entry
// point so `ralph preset check` / `run_preset_lint` callers
// can wire it (next line: into the unified orchestrator).
pub use supervisor::{
    FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC, FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE,
    FINDING_SUPERVISOR_REQUIRES_ISOLATED, FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY,
    check_supervisor_rules,
};
pub use topic_format::{
    TopicFormatResult, TopicOccurrence, TopicSurface, enumerate_topics, suggest_topic_fix,
    validate_all_topics, validate_topic_format,
};
pub use trigger_context::{check_trigger_context, check_trigger_context_topology};
pub use workflow_activation::{
    HandoffGraph, source_label_is_builtin_embedded, run_workflow_activation_contract,
    wave_coord_check_v2 as is_wave_coordination_trigger,
};

// ──────────────────────────────────────────────────────────────────────────
// U2: Shared types — strictness, severity, finding
// ──────────────────────────────────────────────────────────────────────────

/// Severity override for strict mode (U2 checks that are warn-by-default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintStrictness {
    /// Default mode: ownership warnings remain warnings.
    Default,
    /// Strict mode: ownership warnings become errors.
    Strict,
}

impl LintStrictness {
    /// Returns the severity to use for checks that are warn-by-default.
    pub fn ownership_severity(self) -> LintSeverity {
        match self {
            Self::Default => LintSeverity::Warn,
            Self::Strict => LintSeverity::Error,
        }
    }
}

/// Severity level for a lint finding.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum LintSeverity {
    /// Hard error — must be fixed before proceeding.
    Error,
    /// Warning — should be fixed, but non-blocking in default mode.
    Warn,
    /// Informational pass — the check succeeded.
    Pass,
}

impl std::fmt::Display for LintSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error => write!(f, "error"),
            Self::Warn => write!(f, "warn"),
            Self::Pass => write!(f, "pass"),
        }
    }
}

impl LintSeverity {
    /// Parse a severity string, returning `None` for unknown values.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "error" => Some(Self::Error),
            "warn" => Some(Self::Warn),
            "pass" => Some(Self::Pass),
            _ => None,
        }
    }
}

/// Result of a single U2 ownership / coordinator check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintFinding {
    /// Stable machine finding ID (e.g. `preset.owner_unknown_hat`).
    pub id: &'static str,
    /// Severity level — type-safe enum preventing invalid values.
    pub severity: LintSeverity,
    /// Human-readable summary.
    pub message: String,
    /// Optional topic involved.
    pub topic: Option<String>,
    /// Optional hat involved.
    pub hat: Option<String>,
    /// Optional owner hat.
    pub owner: Option<String>,
    /// Optional fix hint.
    pub action_hint: Option<String>,
}

impl LintFinding {
    /// Build a new `LintFinding` from a public caller — used
    /// by the U5 `flow_declaration` lint module, which lives
    /// in a sibling directory and needs to construct findings
    /// without going through the `error()` shorthand.
    pub fn new(id: &'static str, message: impl Into<String>) -> Self {
        Self {
            id,
            severity: LintSeverity::Error,
            message: message.into(),
            topic: None,
            hat: None,
            owner: None,
            action_hint: None,
        }
    }

    pub(crate) fn error(id: &'static str, message: impl Into<String>) -> Self {
        Self {
            id,
            severity: LintSeverity::Error,
            message: message.into(),
            topic: None,
            hat: None,
            owner: None,
            action_hint: None,
        }
    }

    pub(crate) fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    pub(crate) fn with_hat(mut self, hat: impl Into<String>) -> Self {
        self.hat = Some(hat.into());
        self
    }

    pub(crate) fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub(crate) fn with_action_hint(mut self, hint: impl Into<String>) -> Self {
        self.action_hint = Some(hint.into());
        self
    }
}

/// Run all U2 ownership and coordinator checks.
///
/// Returns a sorted, deterministic list of findings.
pub fn validate_ownership_and_coordinator(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    findings.extend(check_owner_references(config));
    findings.extend(check_ownership_rules(config, strictness));
    findings.extend(check_coordinator_rules(config));

    // Sort by (id, topic, hat) for deterministic output.
    findings.sort_by(|a, b| {
        a.id.cmp(b.id)
            .then(a.topic.cmp(&b.topic))
            .then(a.hat.cmp(&b.hat))
    });

    findings
}

// ──────────────────────────────────────────────────────────────────────────
// U3: Convert preset_lint findings → RuntimeContractFinding
// ──────────────────────────────────────────────────────────────────────────

/// Convert a `LintSeverity` to a `RuntimeContractFinding` severity.
fn lint_severity_to_contract(severity: LintSeverity) -> FindingSeverity {
    match severity {
        LintSeverity::Error => FindingSeverity::Error,
        LintSeverity::Warn => FindingSeverity::Warn,
        LintSeverity::Pass => FindingSeverity::Pass,
    }
}

/// Convert a single `LintFinding` into a `RuntimeContractFinding`.
///
/// The `source` is always `FindingSource::Lint` and the `stage` is
/// `FindingStage::Authoring`. The `id` is prefixed with `lint.` to
/// distinguish it from config/topology/payload/orphan findings.
fn lint_finding_to_contract(finding: &LintFinding) -> RuntimeContractFinding {
    // Prefix the id with "lint." for machine-readable source separation.
    let id = format!("lint.{}", finding.id);
    let severity = lint_severity_to_contract(finding.severity);

    let mut contract_finding = RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Lint,
        severity,
        FindingStage::Authoring,
        finding.message.clone(),
    )
    .expect("lint findings never use the reserved Preflight source");

    if let Some(topic) = &finding.topic {
        contract_finding = contract_finding.with_detail("topic", topic.clone());
    }
    if let Some(hat) = &finding.hat {
        contract_finding = contract_finding.with_detail("hat", hat.clone());
    }
    if let Some(owner) = &finding.owner {
        contract_finding = contract_finding.with_detail("owner", owner.clone());
    }
    if let Some(hint) = &finding.action_hint {
        contract_finding = contract_finding.with_action_hint(hint.clone());
    }

    contract_finding
}

/// Convert a batch of `LintFinding` entries into `RuntimeContractFinding`
/// entries suitable for inclusion in a `RuntimeContractReport`.
///
/// Returns findings in deterministic order (sorted by id, topic, hat).
pub fn lint_findings_to_contract_findings(findings: &[LintFinding]) -> Vec<RuntimeContractFinding> {
    findings.iter().map(lint_finding_to_contract).collect()
}

/// Run all U3 lint checks (topic format + ownership + coordinator)
/// and return findings as `RuntimeContractFinding` entries.
///
/// This is the single entry point called by `RuntimeContractAggregator`.
/// Findings are deterministic and sorted.
///
/// WRC-U1 (2026-06-12-003): WAC findings are appended at the end of the
/// pipeline. WAC runs **always-on** (KTD-2) — independent of
/// `strictness` — so callers that use the default path (`ralph preset check`
/// without `--strict`) still see WAC findings as warnings; the
/// aggregator's `fail_on_warnings` flag is what escalates them to
/// blocking. The `strictness == Strict` arm here promotes WAC findings
/// to errors directly, so callers that opt into strict mode (e.g.
/// `enforce_preset_lint_gate`, `--strict` CLI flag) get the
/// `lint.preset.*` blocking semantics without an extra hop.
///
/// WRC-U3 / KTD-7: `source_is_builtin_embedded` escalates every WAC
/// finding to `Error` regardless of `strict`. The CLI runner passes
/// `true` when the caller invoked `ralph run -H builtin:<name>`; the
/// aggregator derives the same flag from the report's `source_label`.
pub fn run_preset_lint(
    config: &RalphConfig,
    strictness: LintStrictness,
    source_is_builtin_embedded: bool,
    raw_yaml: Option<&str>,
) -> Vec<RuntimeContractFinding> {
    run_preset_lint_with_preset_name(config, strictness, source_is_builtin_embedded, raw_yaml, "")
}

/// 2026-07-09-001 plan (U7): preset-aware variant of
/// `run_preset_lint`. The `preset_name` is forwarded to the
/// instructions-OPAC rule so its whitelist gate can fire on
/// the high-risk presets only. Pass an empty string when the
/// caller does not have a preset name handy (matches the
/// pre-U7 behaviour).
pub fn run_preset_lint_with_preset_name(
    config: &RalphConfig,
    strictness: LintStrictness,
    source_is_builtin_embedded: bool,
    raw_yaml: Option<&str>,
    preset_name: &str,
) -> Vec<RuntimeContractFinding> {
    let mut findings: Vec<RuntimeContractFinding> = Vec::new();

    // Topic format validation (U1)
    let format_results = validate_all_topics(config);
    for result in format_results {
        if result.is_valid {
            if result.is_whitelisted {
                // Whitelisted topics are informational passes.
                let id = format!("lint.{}", FINDING_WHITELIST_EXEMPT_TOPIC);
                let finding = RuntimeContractFinding::try_new_core(
                    id,
                    FindingSource::Lint,
                    FindingSeverity::Pass,
                    FindingStage::Authoring,
                    format!(
                        "topic \"{}\" is in the whitelist and exempt from format checks",
                        result.token
                    ),
                )
                .expect("lint findings never use the reserved Preflight source")
                .with_detail("topic", result.token.clone());
                findings.push(finding);
            }
            // Valid non-whitelisted topics produce no finding.
        } else {
            // Invalid topic format.
            let id = format!("lint.{}", FINDING_INVALID_TOPIC_FORMAT);
            let mut finding = RuntimeContractFinding::try_new_core(
                id,
                FindingSource::Lint,
                FindingSeverity::Warn,
                FindingStage::Authoring,
                format!(
                    "topic \"{}\" violates the lowercase dot-case format",
                    result.token
                ),
            )
            .expect("lint findings never use the reserved Preflight source")
            .with_detail("topic", result.token.clone());

            if let Some(suggestion) = &result.suggestion {
                finding = finding.with_action_hint(format!(
                    "Rename to \"{}\" or add to topic_format_whitelist",
                    suggestion
                ));
            }
            findings.push(finding);
        }
    }

    // Ownership & coordinator checks (U2)
    let ownership_findings = validate_ownership_and_coordinator(config, strictness);
    findings.extend(lint_findings_to_contract_findings(&ownership_findings));

    // Multi-hat isolation policy (U1 of 2026-06-11-003): always
    // Error, never downgraded by `LintStrictness`. Produces
    // `RuntimeContractFinding` directly because the structured
    // details `actual` / `limit` / `required_mode` must flow
    // through to the runtime contract aggregator's `details` map.
    findings.extend(check_multi_hat_isolation(config));

    // WRC-U1: WAC (Workflow Activation Contract) rule family —
    // R2 re-emit trap, R3 activation egress, R4 handoff pairing,
    // R5 trigger/publish asymmetry. Always-on per KTD-2:
    // strictness only changes the severity of each finding.
    // `source_is_builtin_embedded` is forwarded to the WAC
    // severity rule (KTD-7) so the CLI gate and the aggregator
    // can both upgrade builtin WAC findings to Error.
    let wac_strict = matches!(strictness, LintStrictness::Strict);
    let wac_findings =
        run_workflow_activation_contract(config, wac_strict, source_is_builtin_embedded);
    findings.extend(lint_findings_to_contract_findings(&wac_findings));

    // 2026-06-26 plan U2: hat scope invariant — three rules
    // (event_filter enabled / topic_deny_rules coverage /
    // coordinator review-chain leak). Fires only in isolated
    // mode; severity is `Error` for all three rules (structural
    // invariants, not style warnings).
    findings.extend(check_hat_scope_invariant(config));

    // 2026-07-03-002 plan U1: fix-unit task_id minting lint. A
    // coordinator hat that handles fix-unit dispatch MUST include
    // a `ralph tools task create` CLI template AND reference the
    // canonical `task-{plan_slug}-fix{NN}u{NN}-{ts_hex}` shape.
    // 093813 root cause: preset had the `MUST be freshly minted`
    // HARD RULE but no CLI parameter template, so the agent
    // hand-composed a task_id reusing a prior step's id, which
    // `state_projector/task.rs:253-260` rejected. This lint
    // surfaces the gap at preset-load time rather than mid-run.
    // Always-on `Error` severity — the rule is structural.
    findings.extend(lint_findings_to_contract_findings(
        &check_fix_unit_task_id_helper_derived(config, strictness),
    ));

    // Plan 2026-06-20-001 U1 KTD-3: state_projection work.done action
    // chain order assertion. Always-on — order is semantic; the
    // engine typestate in `state_projector/mod.rs` is the
    // secondary check (catches Rust-side dispatch bugs only).
    // Reversed chains reintroduce the
    // `ce-executor-serial-primary-20260619` 死循环 by letting
    // `progress_task_gate` reject the next emit after a successful
    // task close. Findings are `Error` severity regardless of
    // `strictness` because the rule is purely structural.
    findings.extend(lint_findings_to_contract_findings(
        &check_work_done_action_chain_order(config),
    ));

    // 2026-06-28 plan U12 (R12): metadata-runtime drift. Validates
    // that the preset's `mechanism.*` block
    // (`state_idempotency`, `enforce_schema`, `repair_budget`) uses
    // values the runtime actually supports. U7 makes the runtime
    // half of the contract a hard panic; this lint closes the
    // preset half so the failure surfaces at preset-load time
    // rather than mid-run.
    findings.extend(check_metadata_runtime_drift(config));

    // 2026-07-03-001 plan U9: supervisor preset rules — R-SW-1
    // (supervisor requires isolated), R-SW-2 (integrator must
    // not subscribe to *.unit.done), R-COORD-4 (hat publishes
    // must not claim supervisor coord topics). The lint reads
    // the raw preset text when available; otherwise it falls
    // back to the typed-config dump (which lacks the `hats:`
    // map's per-hat details — the supervisor rules therefore
    // require `raw_yaml` to be useful, so `ralph preset check`
    // must pass it). U13's preset loader wires raw text
    // explicitly; unit tests below exercise both paths.
    if let Some(text) = raw_yaml {
        let sup_findings = check_supervisor_rules(text);
        findings.extend(lint_findings_to_contract_findings(&sup_findings));
    }

    // 2026-07-04-001 plan U11: instructions OPAC lint rule family.
    // Five rules over hat `instructions:` text — task_create literal,
    // fix-unit mint template, OPAC skill reference, internal-ledger
    // reads, supervisor coord-topic emits. Always Error; raw_yaml
    // is required so the lint can scan the YAML-original text.
    if let Some(text) = raw_yaml {
        let opac_findings = check_instructions_opac_with_preset(text, preset_name);
        findings.extend(lint_findings_to_contract_findings(&opac_findings));
    }

    // Plan 001 §4.5 R1: every hat `publishes` topic must have a schema
    // entry under `event_policy.schemas`. Without this gate, the CLI
    // pre-publish check has nothing to validate against for the topic.
    //
    // Note: `check_schema_reference_parity` is intentionally NOT wired
    // here. It requires a sibling `presets/schemas/<name>.yml` reference
    // file whose path is only known at compile time. The byte-equality
    // tests inside `crates/ralph-cli/src/presets.rs` (the
    // `test_ce_executor_serial_*` family that calls `merge_root_with_ssot`)
    // are the authoritative CI gate; `ralph preset check` relies on
    // `check_publishes_have_schema` for runtime surfacing.
    let schema_parity_findings = schema_parity::check_publishes_have_schema(config, strictness);
    findings.extend(lint_findings_to_contract_findings(&schema_parity_findings));

    // 2026-06-29-007 plan U5a: dimension-reviewer write-path lint.
    // dimension-reviewer is a code-only reviewer; granting it
    // write access to docs/plans/ lets a bad review rewrite
    // the runbook mid-loop. The lint fires `Error` so the
    // preset-load hard gate rejects the offending preset.
    findings.extend(lint_findings_to_contract_findings(
        &check_dimension_reviewer_write_paths(config, strictness),
    ));

    // 2026-07-04-004 plan U3: review-synthesizer block-guard drift
    // lint. Catches presets whose `review-synthesizer`
    // `instructions:` drift away from the explicit "全 6 维度
    // status == failed" invariant. The runtime relies on the
    // explicit phrasing to decide between plan.blocked and the
    // residual-risks path; loose wording is exactly what produced
    // the 2026-07-04 silent-success run. Severity graded by
    // strictness (Warn in default, Error in strict).
    findings.extend(lint_findings_to_contract_findings(
        &check_review_synthesizer_block_guard(config, strictness),
    ));

    // 2026-07-09-003 plan (U4): schema-backed trigger context
    // static lint. Catches unknown `summary_fields` /
    // condition field references, unsupported predicate ops,
    // value-shape mismatches, and duplicate hint labels. The
    // lint is strict-only by design (R3 / R29): default mode
    // skips the check entirely so undeclared presets see no
    // behaviour change. The helper reads
    // `event_policy.schemas` directly so it can also run for
    // presets whose `event_policy` is otherwise inactive.
    if matches!(strictness, LintStrictness::Strict)
        && let Some(policy) = config.event_loop.event_policy.as_ref()
    {
        let trigger_ctx_findings = check_trigger_context(&policy.schemas, strictness);
        findings.extend(lint_findings_to_contract_findings(&trigger_ctx_findings));
    }

    // 2026-07-09-003 plan (U5): trigger context topology
    // lint. Catches `trigger_context` blocks declared on a
    // topic that no hat subscribes to. The block would never
    // reach a downstream hat's prompt, so the declaration is
    // dead. R21 / R22 / SC5. Strict-only by design; the
    // default mode skip is the R3 / R29 invariant.
    if matches!(strictness, LintStrictness::Strict) {
        let topology_findings = check_trigger_context_topology(config, strictness);
        findings.extend(lint_findings_to_contract_findings(&topology_findings));
    }

    // plan 2026-07-22-004 (U5): payload_consistency rule sanity lint.
    // Validates every `event_policy.payload_consistency.rules[]` entry
    // (unique id, topic exists in `event_policy.schemas`, and every
    // `field` referenced in `when` is declared on that topic's schema).
    // The runtime evaluator is fail-close, so a misconfigured rule
    // would silently reject every gated event; surfacing it here means
    // the preset fails to load (strict) instead of failing mid-run.
    // Severity graded by strictness (Warn default, Error strict).
    findings.extend(lint_findings_to_contract_findings(
        &check_payload_consistency(config, strictness),
    ));

    // 2026-06-27 mechanism foundation U5: flow declaration lint.
    // Only presets that declare a `mechanism:` block are checked.
    // Hat-only linear presets (e.g. `ce-executor-pipeline`) intentionally
    // omit `mechanism.flow` and rely on hat triggers + event_policy.
    //
    // 2026-06-27 wiring follow-up: prefer the raw_yaml the caller
    // supplied (when running strict_lint against embedded presets
    // or freshly parsed operator files). The typed-config round
    // trip drops fields `RalphConfig` does not model — the
    // `mechanism:` block is one of those, so the flow-declaration
    // check needs the unaltered text. Fall back to
    // `serde_yaml::to_string(config)` when no raw text is
    // available (e.g. callers that synthesise `RalphConfig`
    // programmatically).
    // P0-3 (2026-06-27 adversarial review): when
    // the caller does not supply `raw_yaml`
    // (tests / programmatic config builders),
    // synthesise a `mechanism:` block from
    // the typed `config.mechanism` field so
    // the flow-declaration lint still fires
    // when the preset opted in. When the
    // caller does supply `raw_yaml`, that
    // text is used verbatim (it may carry
    // keys the typed `RalphConfig` does not
    // model). The synthesised case avoids
    // double-`mechanism:` blocks: the
    // typed-config dump already carries the
    // `mechanism:` key when the field is set,
    // so the synthesised append is a no-op
    // in that case.
    let raw_yaml_owned = match raw_yaml {
        Some(text) => text.to_string(),
        None => {
            let config_yaml = serde_yaml::to_string(config).unwrap_or_default();

            if let Some(mechanism) = config.mechanism.as_ref() {
                if config_yaml
                    .lines()
                    .any(|line| line.trim_start().starts_with("mechanism:"))
                {
                    // The typed-config dump already
                    // carries the `mechanism:` key;
                    // do not double-append.
                    config_yaml
                } else {
                    let mechanism_yaml = serde_yaml::to_string(mechanism).unwrap_or_default();
                    format!("{config_yaml}mechanism:\n{mechanism_yaml}\n")
                }
            } else {
                // Strip the `mechanism: null` line
                // that the typed config dump
                // emits for the default-`None`
                // field. The synthesised raw
                // yaml must NOT carry a
                // `mechanism:` key when the
                // preset has not opted in,
                // otherwise the lint flags a
                // duplicate / missing-field
                // inconsistency.
                config_yaml
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("mechanism:"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
    };
    let has_mechanism_block = raw_yaml_owned
        .lines()
        .any(|line| line.trim_start().starts_with("mechanism:"));
    if has_mechanism_block {
        match flow_declaration::check_flow_declaration(&raw_yaml_owned) {
            Ok(flow_findings) => {
                findings.extend(lint_findings_to_contract_findings(&flow_findings))
            }
            Err(e) => {
                // Parse-level error from `mechanism.flow` is surfaced as a
                // single lint finding so the operator can see why the
                // declaration is unusable.
                let id = format!("lint.{}", FINDING_FLOW_DECLARATION_MISSING);
                let finding = RuntimeContractFinding::try_new_core(
                    id,
                    FindingSource::Lint,
                    FindingSeverity::Error,
                    FindingStage::Authoring,
                    format!("mechanism.flow declaration could not be parsed: {e}"),
                )
                .expect("lint findings never use the reserved Preflight source");
                findings.push(finding);
            }
        }
    }

    // Sort by id, then topic for deterministic output.
    findings.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then(a.details.get("topic").cmp(&b.details.get("topic")))
            .then(a.details.get("hat").cmp(&b.details.get("hat")))
    });

    // Filter out Pass findings — they are informational only and do not
    // affect the report's pass/fail status.
    findings
        .into_iter()
        .filter(|f| f.severity != FindingSeverity::Pass)
        .collect()
}
