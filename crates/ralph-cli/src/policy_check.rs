//! Shared policy precheck module for CLI emit commands.
//!
//! Extracted from `commands/emit.rs` so that `ralph emit` and
//! `ralph wave emit` apply the same policy validation before writing
//! to the events JSONL. The shared module exposes:
//!
//! - [`PolicyCheckMode`] — three-valued decision (Skip, ExplicitCheck, Enforce).
//! - [`resolve_policy_check_mode`] — combine CLI flags + loaded config to a mode.
//! - [`ValidationError`] — single failure (payload index + field + reason_code).
//! - [`validate_topic_payload_against_config`] — single-payload validator.
//! - [`validate_batch_against_config`] — batch validator; collects all violations.
//! - [`emit_policy_validation_failure`] — format the failure payload (text or JSON).
//!
//! Both `ralph emit` and `ralph wave emit` use the same policy check
//! semantics so the loop and CLI can never disagree on what "valid"
//! means. U4 (2026-06-13-001 fix-wave-policy-gate-chain-plan) routes
//! the wave path through this shared module; `ralph emit` keeps its
//! existing single-payload path and just delegates to the same
//! helpers.

// The `use` block below is needed by the in-module `mod tests` (and the
// other `#[cfg(test)] mod *_tests;` children) which use `use super::*;`
// to resolve external types like `EventPolicyConfig`, `RalphConfig`, and
// `GateDecision` that the test bodies pin by name. After the module
// split (Plan 2026-08-07-002 U1) none of these types are referenced by
// the items that actually live in this root facade, so clippy treats
// the imports as unused. The `#[allow(unused_imports)]` keeps the
// re-exports discoverable via `super::*` for the test submodules
// without touching the test bodies (Plan §7 U1 §4 forbids rewriting
// tests).
#[allow(unused_imports)]
use anyhow::{Context, Result};
#[allow(unused_imports)]
use serde::Serialize;
#[allow(unused_imports)]
use std::path::{Path, PathBuf};

#[allow(unused_imports)]
use crate::cli::{ConfigSource, load_config_with_overrides, resolve_workspace_root};
#[allow(unused_imports)]
use crate::config_resolution;
#[allow(unused_imports)]
use crate::operation_guard::OperationContext;
#[allow(unused_imports)]
use ralph_core::config::HatExecutionMode;
#[allow(unused_imports)]
use ralph_core::config::{EventFieldDoc, EventSchema, PayloadType};
#[allow(unused_imports)]
use ralph_core::emit_schema_hint;
#[allow(deprecated, unused_imports)]
use ralph_core::step_handoff::progress_task_gate::{
    GateDecision, ProgressTaskMismatch, check_progress_task_alignment, is_gated_topic,
};
#[allow(unused_imports)]
use ralph_core::{
    EventLoopHandoffConfig, EventPolicyConfig, HatRegistry, PolicyDecision, PolicyRuntimeState,
    RalphConfig, ViolationType, validate_event, validate_event_with_options,
};
#[allow(unused_imports)]
use ralph_proto::HatId;

/// Determines whether and how a CLI emit should undergo policy validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyCheckMode {
    /// Skip policy check entirely.
    Skip,
    /// User explicitly requested `--policy-check`.
    ExplicitCheck,
    /// Config mandates policy check for CLI emit (`require_policy_check_for_cli_emit`).
    Enforce,
}

/// CLI flags that influence policy-check mode resolution.
pub struct PolicyCheckFlags {
    /// `--policy-check` (explicit opt-in).
    pub policy_check: bool,
    /// `--unsafe-no-policy-check` (bypass; only honored when config allows).
    pub no_policy_check: bool,
}

/// Decides the policy-check mode based on CLI arguments and loaded config.
///
/// If `--unsafe-no-policy-check` is passed but the config disallows unsafe
/// bypasses, this returns `Enforce` so the caller can reject the flag.
// 2026-07-16 cleanup U4 (KTD-3): reserved for U15 emit-path parity
// (CLI vs agent policy-check invocation). Pinning the signature now
// avoids churn when U15 lands.
#[allow(dead_code)]
pub fn resolve_policy_check_mode(
    flags: &PolicyCheckFlags,
    config: Option<&RalphConfig>,
) -> PolicyCheckMode {
    resolve_policy_check_mode_with_ctx(flags, config, false)
}

/// U15: agent-context-aware variant of [`resolve_policy_check_mode`].
///
/// When `is_agent_context` is true, callers behave **as if**
/// `event_loop.event_policy.require_policy_check_for_cli_emit: true`,
/// irrespective of the resolved config. This closes the gap where an
/// agent path fell through to `Skip` because the preset author did
/// not enable `event_policy.enabled` — agents must always pass the
/// schema precheck before writing an event.
///
/// Preset opt-out via `event_policy.allow_unsafe_cli_emit: true` is
/// still honoured: an agent calling `ralph emit --unsafe-no-policy-check`
/// on such a preset gets `Skip` (with a deprecation warning emitted
/// via [`crate::commands::emit::emit_unsafe_bypass_deprecation`]).
/// Human CLI (`is_agent_context == false`) keeps the legacy semantics.
pub fn resolve_policy_check_mode_with_ctx(
    flags: &PolicyCheckFlags,
    config: Option<&RalphConfig>,
    is_agent_context: bool,
) -> PolicyCheckMode {
    if flags.policy_check {
        return PolicyCheckMode::ExplicitCheck;
    }

    let config_strict = config
        .and_then(|c| c.event_loop.event_policy.as_ref())
        .map(|p| p.enabled && p.require_policy_check_for_cli_emit)
        .unwrap_or(false);
    let allow_unsafe_bypass = config
        .and_then(|c| c.event_loop.event_policy.as_ref())
        .map(|p| p.allow_unsafe_cli_emit)
        .unwrap_or(false);

    // The effective strict flag is "config-says-strict OR agent-context
    // defaults to strict". Human CLI strictly follows the config.
    let effective_strict = config_strict || is_agent_context;

    if effective_strict {
        if flags.no_policy_check && allow_unsafe_bypass {
            return PolicyCheckMode::Skip;
        }
        return PolicyCheckMode::Enforce;
    }

    // When neither config nor agent-context asks for strict, skip.
    // `--unsafe-no-policy-check` without strict config is a no-op.
    PolicyCheckMode::Skip
}

/// Backwards-compatible re-export of [`resolve_policy_check_mode`]
/// without operation context. Human CLI keep the legacy semantics.
// 2026-07-16 cleanup U4 (KTD-3): reserved for U15 emit-path parity
// (CLI legacy callers that predate `is_agent_context`).
#[allow(dead_code)]
pub fn legacy_resolve(flags: &PolicyCheckFlags, config: Option<&RalphConfig>) -> PolicyCheckMode {
    resolve_policy_check_mode_with_ctx(flags, config, false)
}

// ============================================================================
// Module structure (Plan 2026-08-07-002 U1: policy_check split)
// ============================================================================
mod gates;
mod scope;
mod unified;

#[cfg(test)]
mod read_main_ledger_topics_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod u1_warn_parity_tests;
#[cfg(test)]
mod u2_structured_feedback_tests;
#[cfg(test)]
mod u6_unified_path_tests;

// Re-export the in-module private helpers that the `mod tests`
// submodules still reach via `use super::*;`. These were top-level
// `fn` items in the pre-split `policy_check.rs` and tests pin
// their names; without the re-exports the `use super::*;` inside
// `mod tests` / `mod u6_unified_path_tests` / `mod u1_warn_parity_tests`
// / `mod u2_structured_feedback_tests` / `mod read_main_ledger_topics_tests`
// cannot resolve them. Each is `pub(crate)` in its submodule so
// the re-export widens the visibility to the entire `policy_check`
// sub-tree but **not** beyond the crate (no caller outside
// `ralph-cli` reaches into them). The `#[allow(unused_imports)]`
// suppresses clippy's `unused_imports` warning because this root
// facade itself does not name these items — they exist solely so
// `use super::*;` inside the test submodules can find them.
#[allow(unused_imports)]
pub(crate) use gates::{
    check_wave_dimension_assignment_with_env, extract_dimension_field,
    extract_step_and_task_id_from_payload, mismatch_to_validation_error,
};
#[allow(unused_imports)]
pub(crate) use unified::{
    append_cli_reject_to_recovery, check_cli_flow_step_scope, envelope_summary_enabled,
    extract_quoted_value, finding_record, finding_to_validation_error, payload_type_label,
    read_main_ledger_topics, recover_from_topics, recover_from_workspace_state,
    report_from_validation, resolved_allowed_values, validation_errors_to_emit_errors,
};

#[allow(unused_imports)]
pub use gates::{
    OnConfigError, PolicyCheckContext, build_policy_state, check_isolated_scope,
    check_scope_handoff_guard, check_step_handoff_gate, check_wave_dimension_assignment,
    enabled_event_policy, load_policy_config_for_cli_emit, load_workspace_config,
};
#[cfg(test)]
pub(crate) use unified::run_policy_check_unified;
#[allow(unused_imports)]
pub use unified::{
    BatchValidation, OutputMode, PolicyCheckReport, ValidationError, ValidationFailure,
    build_emit_result_parts, check_emit_provenance, check_envelope_triggered,
    emit_policy_validation_failure, enrich_report_with_schema, enrich_validation_error,
    enrich_validation_error_with_topic, render_validation_error_repair_block,
    report_to_emit_result, run_policy_check_unified_with_config, validate_batch_against_config,
    validate_topic_payload_against_config, validate_topic_payload_with_handoff,
    validate_topic_payload_with_state,
};
/// Unstable helper for looking up EventSchema::required_target_hat at the CLI layer.
/// Exposed pub(crate) so `commands::emit::command_impl` can mirror the runtime guard
/// without duplicating the schema-walking logic.
pub(crate) use unified::required_target_hat_for_topic;
