//! Validation, report shaping, and emit-result shaping (Plan 2026-08-07-002 U1).
//!
//! The `use` block below mirrors the parent `policy_check.rs` so the
//! module's items keep the same crate-internal types in scope. After
//! the module split (Plan 2026-08-07-002 U1) only a subset of these
//! imports is actually referenced here; the rest are listed under
//! `#[allow(unused_imports)]` because the original file was
//! monolithic and the public-API surface (Plan §7 U1 §4: "项级搬移、
//! 模块声明、精确导入、明确列出的最小可见性调整") requires us to
//! preserve each item's import neighborhood verbatim, even when an
//! individual submodule happens not to touch it.
#[allow(unused_imports)]
use crate::cli::{ConfigSource, load_config_with_overrides, resolve_workspace_root};
#[allow(unused_imports)]
use crate::config_resolution;
#[allow(unused_imports)]
use crate::operation_guard::OperationContext;
#[allow(unused_imports)]
use anyhow::{Context, Result};
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
#[allow(unused_imports)]
use serde::Serialize;
#[allow(unused_imports)]
use std::path::{Path, PathBuf};

// `PolicyCheckContext`, `build_policy_state`, `load_policy_config_for_cli_emit`,
// `OnConfigError`, and the scope gates live in the sibling `gates`
// module (Plan 2026-08-07-002 U1). The `validate_topic_payload_with_*`
// helpers below produce them, so import through the crate-private path.
#[allow(unused_imports)]
use super::gates::{
    OnConfigError, PolicyCheckContext, build_policy_state, load_policy_config_for_cli_emit,
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PolicyCheckReport {
    /// Topic that was validated.
    pub topic: String,
    /// Hat that emitted the event (None when unknown).
    pub hat: Option<String>,
    /// Workspace root the validation was performed against.
    ///
    /// **FIX-5 (U11)**: marked `#[serde(skip)]` so the absolute
    /// path never leaks through any automatic `Serialize` path.
    /// The canonical JSON shape (built by [`Self::to_json_value`])
    /// exposes a basename-only `workspace_redacted` field instead.
    /// The struct still keeps the absolute path so in-process
    /// callers (e.g. log enrichment, file-system operations)
    /// can continue to use it.
    #[serde(skip)]
    pub workspace: PathBuf,
    /// `true` iff every U4 rule accepted the event.
    pub accepted: bool,
    /// Stable reason codes from each failed rule, in pre+post-commit
    /// pipeline order. Empty when `accepted == true`. Examples:
    /// `origin:ralph_control_only`, `engine_rejected:required_field:task_id`,
    /// `step_handoff:progress_task_mismatch:task_closed_but_progress_missing`.
    pub reason_codes: Vec<String>,
    /// Human-readable correction hints, parallel to `reason_codes`.
    /// Each entry is the `correction_hint` from the matching
    /// `ValidationResult` (empty string when the rule did not provide
    /// one).
    pub suggestions: Vec<String>,
    /// `true` when at least one rejection came from a post-commit rule.
    /// The CLI uses this to decide whether to surface a
    /// "post-state violation" warning distinct from the per-rule hints.
    pub post_commit_rejected: bool,
    /// 2026-07-09-001 plan (U4): structured validation errors
    /// for the unified path. The CLI uses this to render
    /// `field` + `expected` + `actual` + `field_description`
    /// + `suggested_payload_shape` + `suggested_command` per
    /// item, instead of the legacy `reason_codes` /
    /// `suggestions` parallel-vector format. Empty when
    /// `accepted == true`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validation_errors: Vec<ValidationError>,
}

impl PolicyCheckReport {
    /// Convert the report into a JSON `Value` so callers can layer
    /// their own envelope (e.g. the legacy `ValidationFailure`
    /// shape).
    ///
    /// **FIX-5 (U11)**: the on-disk report shape MUST NOT include
    /// the absolute `workspace` path.  Policy check reports are
    /// written into `.ralph/events.jsonl` / JSON envelopes that
    /// may be shipped to operator dashboards, telemetry, or
    /// shared with downstream services; leaking the workspace
    /// path leaks the operator's directory layout.  The
    /// `workspace` field is kept on the struct (so downstream
    /// code can still inspect it in-process) but the JSON
    /// representation only carries a basename-style redacted
    /// `workspace_redacted` string.
    #[allow(dead_code)] // public API, exposed for downstream tooling
    pub fn to_json_value(&self) -> serde_json::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "topic": self.topic,
            "hat": self.hat,
            "workspace_redacted": self
                .workspace
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<redacted>"),
            "accepted": self.accepted,
            "reason_codes": self.reason_codes,
            "suggestions": self.suggestions,
            "post_commit_rejected": self.post_commit_rejected,
        }))
    }
}

/// Build a [`PolicyCheckReport`] from a U4 [`ValidationReport`].
///
/// Internal helper so the conversion logic is testable without
/// re-running the pipeline.
pub(crate) fn report_from_validation(
    report: &ralph_core::validation::ValidationReport,
    topic: &str,
    hat: Option<&str>,
    workspace: &Path,
) -> PolicyCheckReport {
    let mut reason_codes = Vec::new();
    let mut suggestions = Vec::new();
    let mut validation_errors: Vec<ValidationError> = Vec::new();
    for r in report.pre_commit.iter().chain(report.post_commit.iter()) {
        if r.accepted {
            continue;
        }
        let code = r
            .reason_code
            .clone()
            .unwrap_or_else(|| format!("{}:rejected", r.stage));
        let hint = r.correction_hint.clone().unwrap_or_default();
        reason_codes.push(code.clone());
        suggestions.push(hint.clone());
        // 2026-07-09-001 plan (U4): the legacy
        // `code:hint` shape is what `ralph emit --policy-check`
        // already serialises via `report_to_emit_result`. We
        // also surface a `ValidationError` per rejection so
        // the agent's repair-loop can read the new
        // `field_description` / `suggested_payload_shape`
        // fields. The `field` is empty for the unified path
        // — the validation pipeline does not yet emit
        // field-level markers — so we leave it that way
        // rather than fabricating one.
        let normalised_code = code
            .strip_prefix("engine_rejected:legacy_policy:")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                code.strip_prefix("engine_rejected:")
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| code.clone())
            });
        validation_errors.push(ValidationError {
            payload_index: 0,
            field: String::new(),
            reason_code: normalised_code,
            message: hint,
            ..Default::default()
        });
    }
    PolicyCheckReport {
        topic: topic.to_string(),
        hat: hat.map(|s| s.to_string()),
        workspace: workspace.to_path_buf(),
        accepted: report.accepted,
        reason_codes,
        suggestions,
        post_commit_rejected: report.post_commit_rejected,
        validation_errors,
    }
}

/// Run the unified `ValidationPipeline` over a single event and
/// produce a structured [`PolicyCheckReport`].
///
/// This is the U6 entry point that the CLI `--policy-check` always
/// routes through. The function builds a `ProtocolView` + the
/// current `LedgerSnapshot` (replayed from `.ralph/events.jsonl`),
/// constructs the canonical `ValidationPipeline` (same rules the
/// loop uses), and runs both pre-commit and post-commit phases
/// via `validate_with_preview`. The post-commit phase uses the
/// *current* snapshot as the projected snapshot — the unified
/// pipeline's `validate_with_preview` is conservative and does
/// not mutate the caller's snapshot, so a single call is enough
/// to surface both rule families.
///
/// `event_path` is the events JSONL to replay before running the
/// pipeline. It is used only to satisfy the pipeline's signature
/// (the post-commit rules need a `LedgerSnapshot`; we use the
/// cold-start snapshot here because CLI emit runs ahead of the
/// loop, not against its in-memory state).
#[cfg(test)]
pub fn run_policy_check_unified(
    topic: &str,
    payload: Option<&str>,
    hat: Option<&str>,
    triggered: Option<&str>,
    workspace: &Path,
) -> Result<PolicyCheckReport> {
    // Load the config to build the protocol view. Reuse the
    // existing preflight loader so RALPH_HATS_SOURCE, schema
    // discovery, and the legacy fail-closed rules (C1/C4) all
    // behave identically. When the workspace has no config we
    // fall back to a default view (the unified pipeline will
    // accept everything, mirroring the legacy no-policy default).
    let workspace_root = resolve_workspace_root(Some(&workspace.to_path_buf()));
    let config =
        load_policy_config_for_cli_emit(Some(&workspace_root), OnConfigError::Tolerate, &[])?;
    run_policy_check_unified_with_config(
        topic,
        payload,
        hat,
        triggered,
        &workspace_root,
        config.as_ref(),
    )
}

/// Unified policy check using the configuration already resolved by
/// the emit boundary. This avoids a second config discovery pass whose
/// defaults or environment could diverge from the config used for
/// provenance and routing checks.
pub fn run_policy_check_unified_with_config(
    topic: &str,
    payload: Option<&str>,
    hat: Option<&str>,
    triggered: Option<&str>,
    workspace: &Path,
    config: Option<&RalphConfig>,
) -> Result<PolicyCheckReport> {
    use ralph_core::Event;
    use ralph_core::preset::engine::protocol::ProtocolView;
    use ralph_core::state::{LedgerSnapshot, StateLedger};
    use ralph_core::validation::{ValidationContext, ValidationPipeline};

    let workspace_root = resolve_workspace_root(Some(&workspace.to_path_buf()));
    let event_loop_config = config.map(|c| c.event_loop.clone()).unwrap_or_default();

    // P2-#6 (002-adversarial-review): production-only env
    // read; tests must use `ProtocolView::from_event_loop`
    // (env-free) to stay isolated under `cargo nextest`.
    //
    // 2026-07-06-004 fix-plan U6 (R6): use the hats-aware
    // variant so the topology whitelist (every hat's
    // `triggers` ∪ `publishes`) is populated; the
    // `EventPolicyRule` consults it to reject
    // `success_signal` / `failure_signal` outside the
    // declared topology.
    let hats = config.map(|c| c.hats.clone()).unwrap_or_default();
    let view =
        ProtocolView::from_event_loop_with_feature_for_env_and_hats(&event_loop_config, &hats);
    // 2026-07-07-001 plan U1: derive the runtime hat registry
    // from the loaded config so the unified pipeline's
    // `EventPolicyRule` registry-aware checks
    // (`unknown_to_hat`, `signal_outside_topology`) fire at
    // the CLI boundary. Previously the pipeline was built with
    // `from_config(&view, ...)` which defaulted the registry to
    // `None`, silently bypassing the `to_hat` check on the CLI
    // path. The no-config dry-run path still gets `None` so it
    // does not panic and only enforces schema / topology that
    // is derivable from the cold-start view.
    let pipeline = if let Some(cfg) = config {
        ValidationPipeline::from_ralph_config(&view, cfg)
    } else {
        ValidationPipeline::from_config(&view, &event_loop_config)
    };

    // R12 (U11-T7): load .ralph/events.jsonl into LedgerSnapshot so
    // the unified pipeline sees terminal/business state. The legacy
    // `LedgerSnapshot::cold_start()` produced an empty snapshot, which
    // made the post-commit rules in `validate_with_preview` reject
    // legitimate terminal events (e.g. `work.done` with `task_id`
    // pointing at a queue that does not exist in the snapshot).
    let events_path = workspace_root.join(".ralph/events.jsonl");
    let mut snapshot = if events_path.exists() {
        match StateLedger::replay_from_disk(&workspace_root) {
            Ok(snap) => snap,
            Err(e) => {
                eprintln!("Warning: ledger replay failed for policy check: {e}. Using cold start.");
                LedgerSnapshot::cold_start()
            }
        }
    } else {
        LedgerSnapshot::cold_start()
    };
    let mut projected = snapshot.clone();

    // U11-T7-R12b: legacy `validate_topic_payload_against_config` is
    // the canonical path for **terminal-monotonicity** and
    // **duplicate-terminal** enforcement. The unified pipeline does
    // not yet model these stateful rules (terminal_observed comes
    // from `PolicyRuntimeState::from_events`, not from
    // `LedgerSnapshot`), so we replay events.jsonl once more via
    // the legacy helper and short-circuit with a synthetic reject
    // when the legacy gate flags the event. This keeps the CLI's
    // terminal-monotonicity / duplicate-terminal contract intact
    // across the unified path; without this fallback, the unified
    // path would silently accept events that the loop would later
    // reject on the same events.jsonl replay.
    if let Some(policy) = event_loop_config.event_policy.as_ref()
        && policy.enabled
        && events_path.exists()
    {
        match validate_topic_payload_against_config(
            topic,
            payload.unwrap_or(""),
            policy,
            &events_path,
        ) {
            Ok(Some(legacy_error)) => {
                // Build a synthetic pre-rejected ValidationReport so
                // `report_from_validation` produces the same shape
                // downstream callers already parse. The legacy
                // `terminal_monotonicity_violation` /
                // `duplicate_terminal_event` /
                // `business_event_after_completion` reason codes
                // are surfaced verbatim — they are part of the
                // public CLI surface and tests pin them.
                use ralph_core::validation::ValidationReport;
                use ralph_core::validation::{ValidationResult, ValidationStage};
                let wrapped_reason =
                    format!("engine_rejected:legacy_policy:{}", legacy_error.reason_code);
                let reject = ValidationResult::reject(
                    ValidationStage::ExecutionContract,
                    wrapped_reason,
                    Some(legacy_error.message),
                    true,
                );
                let report = ValidationReport {
                    pre_commit: vec![reject],
                    post_commit: vec![],
                    accepted: false,
                    post_commit_rejected: false,
                };
                return Ok(report_from_validation(&report, topic, hat, &workspace_root));
            }
            Ok(None) => {
                // Legacy gate accepted; fall through to the unified
                // ValidationPipeline for the rest of the rules.
            }
            Err(e) => {
                eprintln!(
                    "Warning: legacy policy check failed for unified path: {e}. Falling back to unified pipeline only."
                );
            }
        }
    }

    let event = Event {
        topic: topic.to_string(),
        payload: payload.map(|s| s.to_string()),
        ts: chrono::Utc::now().to_rfc3339(),
        hat: hat.map(|s| s.to_string()),
        triggered: None,
        source: Some("cli".to_string()),
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };

    let mut ctx = ValidationContext::new(&mut snapshot);
    let mut projected_ctx = ValidationContext::new(&mut projected);
    let report = pipeline.validate_with_preview(&view, &mut ctx, &mut projected_ctx, &event);
    let final_report = report_from_validation(&report, topic, hat, &workspace_root);

    // 2026-06-28-002 U7: when the unified pipeline rejects the
    // event, mirror the rejection into `.ralph/recovery.jsonl`
    // as a `repair_dispatch` envelope so the recovery stream
    // aggregator (downstream of `record_repair_event`) sees the
    // CLI emit path. Without this, CLI `ralph emit` failures
    // bypass U6/U7/U9.5/U12 entirely because the pipeline
    // result was a CLI-only side-effect. We append best-effort:
    // a missing `recovery.jsonl` is not fatal, and a write
    // failure is logged at WARN level.
    if !report.accepted {
        append_cli_reject_to_recovery(&workspace_root, topic, hat, payload, &report);
    }

    // U7 of plan 2026-07-05-005 (R6): envelope-layer
    // `triggered` validation. Runs after the unified pipeline
    // so the gate order is "payload schema → envelope
    // topology". Mirrors the apply-path gate so `--policy-check`
    // and the real write share the same rejection surface.
    if let Some(cfg) = config
        && let Err(err) = check_envelope_triggered(topic, hat, triggered, cfg)
    {
        let mut rej = final_report;
        rej.accepted = false;
        rej.reason_codes.push(err.reason_code);
        rej.suggestions.push(err.message);
        return Ok(rej);
    }

    // 2026-07-26-004 plan U7 (R7 / S7): enforce the SAME flow-step
    // scope the resident EventLoop applies, using the single recovered
    // current-step authority. When the accepted-step ledger exists, that
    // is the source of truth; otherwise, if `.ralph/current-events`
    // points at an active main ledger, replay that ledger's accepted
    // topic sequence. Falling all the way back to the static workspace
    // snapshot is only correct when no active main ledger exists.
    if let Some(cfg) = config
        && let Some(reason_code) = check_cli_flow_step_scope(
            cfg,
            &workspace_root,
            Some(events_path.as_path()),
            topic,
            hat,
            payload,
        )
    {
        let mut rej = final_report;
        rej.accepted = false;
        rej.reason_codes.push(reason_code);
        return Ok(rej);
    }

    Ok(final_report)
}

/// 2026-07-26-004 plan U7 (R7 / S7): run the resident EventLoop's
/// `FlowStepScopeStage` against a CLI emit / `--policy-check` candidate,
/// using the current flow step recovered from the replayed main ledger
/// via the single [`recover_current_plan_step`] authority. Returns
/// `Some(reason_code)` when the topic is not allowed at the recovered
/// step (mirroring the loop's `flow_unknown_emit` / `flow_step_undeclared`),
/// `None` when there is no flow declaration or the topic is admitted.
pub(crate) fn check_cli_flow_step_scope(
    config: &ralph_core::config::RalphConfig,
    workspace_root: &Path,
    events_file: Option<&Path>,
    topic: &str,
    hat: Option<&str>,
    payload: Option<&str>,
) -> Option<String> {
    use ralph_core::event_loop::load_flow_authority_current_step;
    use ralph_core::event_loop::load_opt_in_flow_declaration;
    use ralph_core::event_loop::stage_pipeline::{EmitStage, FlowStep, StageContext};
    use ralph_core::event_loop::stages::flow_step_scope_stage::FlowStepScopeStage;

    // No declared flow → flow-step gating is skipped (hat-only presets).
    let flow = load_opt_in_flow_declaration(config)?;

    // Plan 004 R7 (P0-4): the resident EventLoop writes accepted
    // step transitions to `.ralph/flow-authority.jsonl`; we read
    // the same ledger so CLI policy-check never disagrees with
    // the resident loop on rejected events.
    //
    // If the accepted-step ledger is missing, the default workspace
    // path falls back to the accepted-state projection
    // (`StateLedger::workflow_phase`) instead of guessing from raw
    // topics. Only an explicit caller-provided `events_file` keeps
    // the legacy topic-replay path, because external replay inputs
    // are expected to already represent the accepted sequence the
    // caller wants to validate.
    let default_events_path = workspace_root.join(".ralph/events.jsonl");
    let active_main_ledger = OperationContext::detect(workspace_root.to_path_buf())
        .resolve_accepted_events_path()
        .unwrap_or_else(|| default_events_path.clone());
    // Plan 2026-07-31-001: pass the active loop_id (read from
    // `.ralph/current-loop-id`) so `load_flow_authority_current_step`
    // ignores stale entries left by a previous loop. Without this
    // filter, `ralph emit --policy-check` would inherit the
    // previous loop's terminal step (e.g. `finalize`) and reject
    // the very first emit of the new loop via `flow_unknown_emit`
    // even though the resident EventLoop's in-memory
    // `current_plan_step` is correct — a dual-source drift the CLI
    // surfaces but the resident loop does not.
    let active_loop_id = std::fs::read_to_string(workspace_root.join(".ralph/current-loop-id"))
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    let current = if let Some(step) =
        load_flow_authority_current_step(workspace_root, active_loop_id.as_deref())
    {
        if step.is_empty() {
            recover_from_workspace_state(config, workspace_root)
        } else {
            step
        }
    } else if let Some(events_path) = events_file {
        recover_from_topics(config, workspace_root, Some(events_path))
    } else if active_main_ledger != default_events_path {
        recover_from_topics(config, workspace_root, Some(active_main_ledger.as_path()))
    } else {
        recover_from_workspace_state(config, workspace_root)
    };
    if current.is_empty() {
        return None;
    }

    let stage = FlowStepScopeStage::new(flow);
    let mut repair_states = std::collections::HashMap::new();
    let mut ctx = StageContext::new(
        FlowStep::new(current),
        "cli-policy-check",
        0,
        &mut repair_states,
    );
    let mut event = ralph_proto::Event::new(topic, payload.unwrap_or(""));
    if let Some(h) = hat {
        event = event.with_source(h);
    }
    match stage.check(&mut ctx, &event) {
        Ok(()) => None,
        Err(reject) => Some(reject.reason_code),
    }
}

/// Topic-replay fallback for `check_cli_flow_step_scope`: only used
/// when no accepted transition has been written yet, so the
/// "reject/accept mix" P0-4 concern does not apply (every prior
/// topic is implicitly accepted because no accept ledger exists).
pub(crate) fn recover_from_topics(
    config: &ralph_core::config::RalphConfig,
    workspace_root: &Path,
    events_file: Option<&Path>,
) -> String {
    use ralph_core::event_loop::recover_current_plan_step;
    let topics = read_main_ledger_topics(workspace_root, events_file);
    let topic_refs: Vec<&str> = topics.iter().map(|s| s.as_str()).collect();
    recover_current_plan_step(config, &topic_refs)
}

/// Recover the current step from accepted workspace state rather
/// than from raw topic logs. This is the default-path fallback when
/// the accepted-step ledger is absent.
pub(crate) fn recover_from_workspace_state(
    config: &ralph_core::config::RalphConfig,
    workspace_root: &Path,
) -> String {
    use ralph_core::event_loop::recover_current_plan_step;
    use ralph_core::state::StateLedger;

    StateLedger::replay_from_disk(workspace_root)
        .ok()
        .and_then(|snap| snap.workflow_phase)
        .map(|phase| phase.phase_id)
        .filter(|phase_id| !phase_id.is_empty())
        .unwrap_or_else(|| recover_current_plan_step(config, &[]))
}

/// Read the `topic` field of every JSONL line in the loop's main
/// ledger, in order. Malformed lines are skipped. Used by
/// [`check_cli_flow_step_scope`] to recover the current flow step.
///
/// Plan 004 P1-8: `events_file` overrides the default
/// `<workspace_root>/.ralph/events.jsonl` path. The CLI is
/// expected to thread the events path the caller already
/// resolved (e.g. via `RALPH_EVENTS_FILE` or `--events-file`)
/// so a multi-loop / multi-worktree caller cannot accidentally
/// read another loop's ledger. When `events_file` is `None`
/// we fall back to the default location, preserving the
/// pre-P1-8 behaviour for single-loop invocations.
///
/// Plan 2026-07-31-001 (regression test for implementation-review
/// runs primary-20260731-131515 + primary-20260731-133437):
/// entries with `"system_injected": true` are runtime
/// fallbacks (e.g. `scope.blocked` injected when a hat emits
/// no events) and MUST NOT be treated as accepted topics by
/// `recover_current_plan_step`. Without this filter the CLI
/// advances the recovered step to `finalize` because
/// `scope.blocked ∈ finalize.on_any_of`, then rejects the
/// fresh `scope.ready.proposed` emit via `flow_unknown_emit`.
/// The filter matches the EventLoop's own accepted-event rule
/// (the resident loop only calls
/// `append_flow_authority_snapshot` from the accept branch).
pub(crate) fn read_main_ledger_topics(
    workspace_root: &Path,
    events_file: Option<&Path>,
) -> Vec<String> {
    let path = match events_file {
        Some(p) => p.to_path_buf(),
        None => workspace_root.join(".ralph/events.jsonl"),
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        // Plan 2026-07-31-001: drop runtime-injected fallbacks.
        // They carry `"system_injected": true` (see
        // `inject_default_publishes_topic` in event_loop/mod.rs);
        // treating them as accepted topics would advance the
        // recovered step to a terminal value and reject every
        // fresh emit of the next loop iteration.
        .filter(|v| v.get("system_injected").and_then(|b| b.as_bool()) != Some(true))
        .filter_map(|v| {
            v.get("topic")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

/// 2026-06-28-002 U7: append a `repair_dispatch` envelope to
/// `.ralph/recovery.jsonl` when the unified pipeline rejects a
/// CLI emit. Delegates to `record_repair_event` so the envelope
/// shape is identical to the loop's internal repair stream —
/// downstream consumers (e.g. `ralph diagnose`) do not need to
/// special-case CLI rejects. Best-effort: a write failure is
/// logged at WARN level.
pub(crate) fn append_cli_reject_to_recovery(
    workspace_root: &Path,
    topic: &str,
    hat: Option<&str>,
    payload: Option<&str>,
    report: &ralph_core::validation::ValidationReport,
) {
    use ralph_proto::{Event, HatId};
    let event = Event {
        topic: ralph_proto::Topic::from(topic),
        payload: payload.unwrap_or("").to_string(),
        source: hat.map(|h| HatId::new(h)),
        target: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    if let Err(e) = ralph_core::event_loop::repair_stream_sink::record_repair_event(
        &event,
        &workspace_root.join(".ralph"),
    ) {
        tracing::warn!(
            target: "ralph_cli::policy_check",
            workspace_root = %workspace_root.display(),
            error = %e,
            "U7: failed to append CLI reject to recovery.jsonl via repair_stream_sink; \
             continuing without blocking the CLI"
        );
    }
    // Suppress unused-variable warning when the report is no longer
    // inspected after delegation.
    let _ = report;
}

#[allow(clippy::result_large_err)]
pub fn check_emit_provenance(
    hat: Option<&str>,
    topic: &str,
    config: &RalphConfig,
) -> std::result::Result<(), ValidationError> {
    if config.event_loop.execution_mode != HatExecutionMode::Isolated {
        return Ok(());
    }

    if hat.is_some() {
        // Provenance present — scope guard (check_isolated_scope) and
        // runtime origin guard handle the rest.
        return Ok(());
    }

    // Control topics are produced by the loop / runtime ralph pseudo-hat
    // and are exempt from hat provenance. Diagnostic events are emitted
    // by the loop itself for observability.
    if ralph_core::RALPH_CONTROL_TOPICS.contains(&topic) {
        return Ok(());
    }
    if ralph_core::is_orchestrator_diagnostic_topic(topic) {
        return Ok(());
    }

    Err(ValidationError {
        payload_index: 0,
        field: "hat".to_string(),
        reason_code: "missing_provenance".to_string(),
        message: format!(
            "missing provenance: isolated mode requires a hat for business topic '{topic}'. \
             Pass --hat <hat-id> or set RALPH_CURRENT_HAT=<hat-id>. \
             (Control topics {:?} and orchestrator diagnostics bypass this gate.)",
            ralph_core::RALPH_CONTROL_TOPICS
        ),
        ..Default::default()
    })
}

/// U7 of plan 2026-07-05-005 (R6, R12): envelope-layer
/// `triggered` validator. The `triggered` field on the emit
/// record is an envelope field (not a payload field), so it
/// sits outside `policy_check`'s schema-driven path. This gate
/// is the dedicated check, mirroring the
/// `check_emit_provenance` style. Missing `triggered` is
/// allowed (R12) — only present-but-unknown values are
/// rejected.
///
/// Returns `Ok(())` when:
/// - `triggered` is `None` (R12)
/// - `triggered` is `Some(_)` AND the value matches a hat id
///   declared in `config.hats`
///
/// Returns `Err(ValidationError)` with
/// `reason_code = "triggered_not_in_topology"` when `triggered`
/// is set to a value that does not appear in the loaded
/// preset's `hats[]` map. The error message names the offending
/// value AND the resolved hat ids so the agent can self-correct.
#[allow(clippy::result_large_err)]
pub fn check_envelope_triggered(
    topic: &str,
    source_hat: Option<&str>,
    triggered: Option<&str>,
    config: &RalphConfig,
) -> std::result::Result<(), ValidationError> {
    let Some(value) = triggered else {
        return Ok(());
    };
    if value.is_empty() {
        return Ok(());
    }
    // U7 carve-out: control / orchestrator-internal topics may
    // carry a pseudo-hat in `triggered` (e.g. `ralph-runner`)
    // that is not a preset hat id. Skip the topology check when
    // the topic is in the same allowlist that
    // `check_emit_provenance` uses; the runtime origin guard
    // handles downstream validation.
    //
    // U7 of plan 2026-07-05-005 (fix-plan §R11): layer the
    // carve-out by topic-trust so the same matrix as
    // `check_emit_provenance` applies:
    //   - ralph-control topics (loop.cancel / task.resume /
    //     loop.complete / human.*) — `triggered` is allowed
    //     even if not in preset hats[] (runtime pseudo-hat
    //     origin is fine).
    //   - orchestrator diagnostic topics (event.*) — same as
    //     ralph-control (the runtime injects them).
    //   - business topics — strict topology check applies;
    //     `triggered` must be a declared hat id.
    use ralph_core::event_origin::is_ralph_control_topic;
    use ralph_core::is_orchestrator_diagnostic_topic;
    if is_ralph_control_topic(topic) || is_orchestrator_diagnostic_topic(topic) {
        return Ok(());
    }
    if config.event_loop.execution_mode == HatExecutionMode::Isolated
        && source_hat == Some(value)
    {
        return Err(ValidationError {
            payload_index: 0,
            field: "triggered".to_string(),
            reason_code: "triggered_self_target".to_string(),
            message: format!(
                "triggered='{value}' points back to the publishing hat; omit --triggered for ordinary isolated handoffs or choose a different declared downstream hat"
            ),
            actual: Some(value.to_string()),
            ..Default::default()
        });
    }
    if config.hats.contains_key(value) {
        return Ok(());
    }
    let mut allowed: Vec<&str> = config.hats.keys().map(String::as_str).collect();
    allowed.sort_unstable();
    Err(ValidationError {
        payload_index: 0,
        field: "triggered".to_string(),
        reason_code: "triggered_not_in_topology".to_string(),
        message: format!(
            "triggered='{value}' is not declared in preset hats[]. Resolved hat ids: [{}]. \
             Pass --triggered <hat-id> matching one of the declared hats, or omit --triggered \
             entirely (R12: missing triggered is allowed).",
            allowed.join(", ")
        ),
        ..Default::default()
    })
}

/// A single structured validation error. Stable JSON shape — agents
/// rely on `field` + `reason_code` to programmatically diagnose
/// payload issues (see `crates/ralph-core/data/ralph-tools.md`).
///
/// 2026-07-09-001 plan (U3): the trailing `Option<...>` fields
/// are the agent-facing enrichment layer (see
/// `enrich_validation_error`). They are
/// `#[serde(skip_serializing_if = "Option::is_none")]` so the
/// canonical JSON shape stays backwards-compatible — old
/// consumers see exactly the same keys as before, and new
/// consumers can opt into the enrichment.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct ValidationError {
    /// Index of the failing payload in the original batch (0-based).
    pub payload_index: usize,
    /// Field that failed (e.g. `depth`, `plan_name`); empty when the
    /// violation is at the payload / topic level rather than a field.
    pub field: String,
    /// Stable machine-readable code; one of:
    /// `missing_required_field`, `invalid_field_value`,
    /// `payload_type_mismatch`, `terminal_monotonicity_violation`,
    /// `duplicate_terminal_event`, `business_event_after_completion`,
    /// `invalid_topic_format`, `topic_denied`,
    /// `policy_unavailable`, `config_error`.
    pub reason_code: String,
    /// Human-readable message suitable for logs and stderr summaries.
    pub message: String,
    /// 2026-07-09-001 plan (U3): what the field is supposed to be
    /// (e.g. the allowed-values list, or the literal
    /// `<required field name>` for `missing_required_field`).
    /// `None` for violations where the field-level expectation
    /// cannot be expressed (e.g. unknown semantic gate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// 2026-07-09-001 plan (U3): the actual value that violated
    /// the rule, serialised to a string. `None` for
    /// `missing_required_field` (no actual value exists).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// 2026-07-09-001 plan (U3): the schema's `field_docs.<f>`
    /// meaning, when the violation is at a known field. `None`
    /// when the field is unknown or has no doc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_description: Option<String>,
    /// 2026-07-09-001 plan (U3): a JSON-serialisable skeleton
    /// payload the agent can edit. Uses
    /// `emit_schema_hint::suggested_payload_shape` so it never
    /// invents business values (e.g. `0` for
    /// `must_fix_now_count`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_payload_shape: Option<serde_json::Value>,
    /// 2026-07-09-001 plan (U3): a copy-pasteable
    /// `ralph emit <topic> --policy-check -j '<shape>'` command
    /// the agent can re-run after fixing the payload. `None`
    /// when the violation is at the topic level (no payload to
    /// shape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_command: Option<String>,
    /// U2 (2026-07-23-002 plan, KTD2): independent gate identifier
    /// for `SemanticGateViolation`. Carries the canonical gate
    /// name (e.g. `payload_consistency:<rule_id>` or
    /// `review_passed_while_wave_open`) so agent repair tooling
    /// can dispatch on gate without parsing `message`. `None` for
    /// schema-level violations (missing/invalid field, type
    /// mismatch, etc.) where `reason_code` already identifies the
    /// class. The legacy `field` slot is NOT used to carry the
    /// gate ID — `field` stays empty for `SemanticGateViolation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    /// U2 (2026-07-23-002 plan, KTD2): the static, declaration-order
    /// set of business fields the rule's predicate AST references.
    /// Agent repair tooling reads this list to know which payload
    /// fields to inspect, and never parses `message` to recover
    /// them. `None` for schema-level violations (field-scoped
    /// violations already carry the single field in `field`).
    /// Empty `Some(vec![])` for timing/state gates where the
    /// violation is not field-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenced_fields: Option<Vec<String>>,
    /// U4 (plan 2026-08-06-001, R2): bounded field observations
    /// the rule saw when it fired.  Each entry is a JSON object
    /// `{field, value}` carrying the literal value (or the
    /// sentinel `unavailable` / `unchecked`).  Always `None`
    /// for non-evidence findings (mechanical schema violations
    /// keep `field` + `expected` + `actual` instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<Vec<serde_json::Value>>,
    /// U4 (plan 2026-08-06-001, R2/R3): the violated rule,
    /// expressed as a stable human-readable string.  Distinct
    /// from `message` (which is the legacy diagnostic text) so
    /// the agent can match on the rule without parsing free-
    /// form prose.  Empty for schema-level violations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invariant: Option<String>,
    /// U4 (plan 2026-08-06-001, R2): the condition the agent
    /// must re-prove on the next attempt (e.g. "rebuild the
    /// payload from the artifact and rerun
    /// `ralph emit --policy-check`").  Empty for schema-level
    /// violations and for semantic violations where the
    /// gate did not supply a proof (legacy fallback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_proof: Option<String>,
}

impl ValidationError {
    /// 2026-07-09-001 plan (U3): convenience constructor for
    /// the legacy four-field shape. New code should prefer
    /// struct-literal initialisation with the optional
    /// enrichment fields, but the 10+ existing call sites
    /// can keep their struct-literal shape via this helper
    /// plus `..Self::default()`.
    // 2026-07-16 cleanup U4 (KTD-3): reserved for future preset
    // policy rewrites that build `ValidationError` directly
    // from the lint findings stream.
    #[allow(dead_code)]
    pub fn new(
        payload_index: usize,
        field: impl Into<String>,
        reason_code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            payload_index,
            field: field.into(),
            reason_code: reason_code.into(),
            message: message.into(),
            ..Self::default()
        }
    }
}

/// 2026-07-09-001 plan (U3): pure enrichment helper. Given a
/// `ValidationError`, the original payload (best-effort parsed
/// `serde_json::Value`), and the schema for the topic,
/// populate the optional `expected` / `actual` /
/// `field_description` / `suggested_payload_shape` /
/// `suggested_command` fields. The function is pure (no
/// I/O, no clock, no env) so the unit tests in
/// `tests::u3_enriched_validation_error_*` cover the full
/// matrix.
///
/// Enrichment rules:
/// - `missing_required_field`: `expected` is the field name;
///   `field_description` is the schema's
///   `field_docs.<f>.meaning` when present;
///   `suggested_payload_shape` is the placeholder shape;
///   `suggested_command` is the `--policy-check` invocation.
/// - `invalid_field_value`: `expected` is the joined
///   `allowed_values` / `hat_allowed_values` set when
///   resolvable; `actual` is the offending JSON value
///   re-rendered to a string.
/// - `payload_type_mismatch`: `expected` is the schema's
///   declared `payload` type; `field_description` stays
///   `None` (no field to describe); the shape suggestion
///   returns `Null` (see `suggested_payload_shape`).
/// - All other codes: only `expected` / `actual` are
///   populated when the legacy `message` carries enough
///   context; the rest stay `None`. The function never
///   fabricates a field description for an unknown
///   semantic gate.
pub fn enrich_validation_error(
    mut error: ValidationError,
    hat: Option<&str>,
    payload: Option<&serde_json::Value>,
    schema: Option<&EventSchema>,
) -> ValidationError {
    use serde_json::Value;
    let payload_obj = payload.and_then(|v| v.as_object());

    if let Some(s) = schema {
        match error.reason_code.as_str() {
            "missing_required_field" => {
                if !error.field.is_empty() {
                    error.expected = Some(error.field.clone());
                    if let Some(EventFieldDoc { meaning, .. }) = s.field_docs.get(&error.field)
                        && !meaning.trim().is_empty()
                    {
                        error.field_description = Some(meaning.clone());
                    }
                    let shape = emit_schema_hint::suggested_payload_shape(
                        s,
                        payload.unwrap_or(&Value::Null),
                    );
                    if shape.is_object() {
                        // 2026-07-09-001 plan (U6): the previously
                        // here-and-now-overwritten
                        // `error.suggested_command = Some(format!(...))`
                        // + comment block + `= None` reassignment
                        // were a dead-code trap — the
                        // `enrich_validation_error_with_topic`
                        // wrapper regenerates suggested_command
                        // from the real topic downstream, so
                        // any inline placeholder was
                        // misleading. Keep only the
                        // `suggested_payload_shape` write.
                        error.suggested_payload_shape = Some(shape);
                    }
                }
            }
            "invalid_field_value" => {
                if !error.field.is_empty() {
                    if let Some(values) = resolved_allowed_values(s, hat, &error.field) {
                        let joined = values
                            .iter()
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");
                        error.expected = Some(joined);
                    }
                    if let Some(obj) = payload_obj.and_then(|o| o.get(&error.field)) {
                        error.actual = Some(obj.to_string());
                    } else if error.actual.is_none() {
                        // The legacy message often contains the
                        // offending value as a quoted token; if
                        // not, leave `actual` as None.
                        error.actual = extract_quoted_value(&error.message);
                    }
                    error.suggested_payload_shape =
                        Some(emit_schema_hint::suggested_payload_shape(
                            s,
                            payload.unwrap_or(&Value::Null),
                        ));
                }
            }
            "payload_type_mismatch" => {
                error.expected = Some(
                    s.payload
                        .as_ref()
                        .map(payload_type_label)
                        .unwrap_or_else(|| "json_object".to_string()),
                );
                if let Some(obj) = payload_obj
                    && let Some((_, v)) = obj.iter().next()
                {
                    error.actual = Some(v.to_string());
                }
                // No field, no shape — a payload-level
                // violation does not map onto a single
                // suggestion.
            }
            _ => {
                // Semantic / monotonicity / duplicate-terminal
                // gates: only fill `expected` / `actual` when
                // the legacy message can be parsed cheaply.
                // Do not invent a field description.
            }
        }
    }

    error
}

/// 2026-07-09-001 plan (U3): wrapper that knows the topic so
/// the `suggested_command` field can name the real
/// `ralph emit <topic> ...` command. Topic-level violations
/// (empty `field`) get no command suggestion; field-level
/// violations get the `--policy-check` invocation the
/// agent should re-run after fixing the payload.
pub fn enrich_validation_error_with_topic(
    error: ValidationError,
    topic: &str,
    hat: Option<&str>,
    payload: Option<&serde_json::Value>,
    schema: Option<&EventSchema>,
) -> ValidationError {
    let mut enriched = enrich_validation_error(error, hat, payload, schema);
    if let Some(shape) = enriched.suggested_payload_shape.as_ref()
        && shape.is_object()
    {
        enriched.suggested_command = Some(format!(
            "ralph emit {topic} --policy-check -j '{shape}'",
            topic = topic,
            shape = shape
        ));
    }
    enriched
}

/// Resolve allowed values for a field, preferring the rule that
/// matches the current hat when one exists.
pub(crate) fn resolved_allowed_values(
    schema: &EventSchema,
    hat: Option<&str>,
    field: &str,
) -> Option<Vec<serde_json::Value>> {
    if let Some(hat_id) = hat
        && let Some(rules) = schema.hat_allowed_values.get(field)
        && let Some(rule) = rules.iter().find(|rule| rule.hat_id == hat_id)
        && !rule.values.is_empty()
    {
        return Some(rule.values.clone());
    }

    schema
        .allowed_values
        .get(field)
        .filter(|values| !values.is_empty())
        .cloned()
}

/// 2026-07-09-001 plan (U3): map a `PayloadType` to the
/// stable snake_case label the agent expects (e.g.
/// `JsonObject` → `"json_object"`). Kept separate from
/// `format!("{:?}", p)` because the Debug form is
/// PascalCase — wrong for human-facing repair text.
pub(crate) fn payload_type_label(p: &PayloadType) -> String {
    match p {
        PayloadType::JsonObject => "json_object".to_string(),
        PayloadType::String => "string".to_string(),
        PayloadType::Number => "number".to_string(),
        PayloadType::Bool => "bool".to_string(),
        PayloadType::Array => "array".to_string(),
    }
}

/// 2026-07-09-001 plan (U3): best-effort extractor for the
/// offending value embedded in a legacy `message` string.
/// The legacy `invalid_field_value` message format is
/// `... = '<value>' ...`. Returns `None` when the message
/// has no quoted substring; never panics.
pub(crate) fn extract_quoted_value(message: &str) -> Option<String> {
    let bytes = message.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' || bytes[i] == b'"' {
            let quote = bytes[i];
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != quote {
                j += 1;
            }
            if j < bytes.len() {
                return Some(message[start..j].to_string());
            }
        }
        i += 1;
    }
    None
}

/// Result of a batch validation. Empty `errors` means the batch is
/// acceptable; non-empty means the caller MUST reject the entire
/// batch atomically (no partial write).
#[derive(Debug, Clone, Default)]
pub struct BatchValidation {
    pub errors: Vec<ValidationError>,
}

impl BatchValidation {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validates a single payload against the loaded event policy and
/// returns a structured `ValidationError` (or `None` if the payload
/// is acceptable). Replays prior events from `events_file` to
/// enforce terminal-monotonicity / duplicate-terminal checks —
/// the same replay path the loop uses when reading the JSONL.
#[allow(dead_code)] // Exposed for single-event callers (e.g. ralph emit).
/// 2026-07-06-004 plan U10: typed entry point that plumbs the
/// typed handoff envelope config into the policy pipeline. The
/// legacy `validate_topic_payload_against_config` /
/// `validate_topic_payload_with_state` keep their default-no-op
/// behaviour for callers that don't care about handoff envelopes;
/// new CLI code (e.g. `ralph emit --policy-check`) goes through
/// this entry point.
pub fn validate_topic_payload_with_handoff(
    topic: &str,
    payload_str: &str,
    policy: &EventPolicyConfig,
    events_file: &Path,
    handoff: &ralph_core::config::HandoffEnvelopeConfig,
) -> Result<Option<ValidationError>> {
    let ctx = PolicyCheckContext {
        events_file: events_file.to_path_buf(),
    };
    let mut state = build_policy_state(policy, &ctx);
    let adapter = EventLoopHandoffConfig {
        handoff_envelope: handoff,
    };
    let decision =
        validate_event_with_options(topic, Some(payload_str), policy, &mut state, None, &adapter);
    Ok(finding_to_validation_error(&decision, topic))
}

pub fn validate_topic_payload_against_config(
    topic: &str,
    payload_str: &str,
    policy: &EventPolicyConfig,
    events_file: &Path,
) -> Result<Option<ValidationError>> {
    let ctx = PolicyCheckContext {
        events_file: events_file.to_path_buf(),
    };
    let mut state = build_policy_state(policy, &ctx);
    let decision = validate_event(topic, Some(payload_str), policy, &mut state);
    Ok(finding_to_validation_error(&decision, topic))
}

/// Validates a single payload against the loaded event policy using
/// a pre-built runtime state. Used by the batch validator so the
/// caller reuses one replay instead of replaying the events file
/// once per payload.
pub fn validate_topic_payload_with_state(
    topic: &str,
    payload_str: &str,
    policy: &EventPolicyConfig,
    state: &mut PolicyRuntimeState,
) -> Result<Option<ValidationError>> {
    let decision = validate_event(topic, Some(payload_str), policy, state);
    Ok(finding_to_validation_error(&decision, topic))
}

/// Validates a batch of payloads. Scans every payload (atomicity:
/// a single bad payload rejects the entire batch) and reuses one
/// [`PolicyRuntimeState`] across the loop — terminal-monotonicity
/// semantics match what the loop sees on its end.
///
/// `topic` is shared by every payload in the batch (waves are
/// single-topic by construction).
pub fn validate_batch_against_config(
    topic: &str,
    payloads: &[String],
    policy: &EventPolicyConfig,
    events_file: &Path,
) -> Result<BatchValidation> {
    let ctx = PolicyCheckContext {
        events_file: events_file.to_path_buf(),
    };
    let mut state = build_policy_state(policy, &ctx);
    let mut errors = Vec::new();
    for (index, payload) in payloads.iter().enumerate() {
        if let Some(err) = validate_topic_payload_with_state(topic, payload, policy, &mut state)? {
            errors.push(ValidationError {
                payload_index: index,
                ..err
            });
        }
    }
    Ok(BatchValidation { errors })
}

pub(crate) fn finding_to_validation_error(
    decision: &PolicyDecision,
    _topic: &str,
) -> Option<ValidationError> {
    let finding = match decision {
        PolicyDecision::Accept => return None,
        // U1 (2026-07-23-002 plan, KTD1): `Warn` is non-fatal at
        // the precheck boundary, matching the runtime Apply
        // disposition. The previous `payload_consistency:` gate
        // prefix carve-out is removed — the CLI no longer
        // escalates Warn by gate namespace. Enforce-mode gates
        // that need to reject payloads use `RejectWithResume` /
        // `Hold` / `Block`, which the runtime and CLI both
        // surface as `ValidationError`. `AcknowledgeAndForward`
        // (dedup carve-out) is likewise non-fatal.
        PolicyDecision::Warn(_findings) => return None,
        PolicyDecision::AcknowledgeAndForward(_finding) => return None,
        PolicyDecision::RejectWithResume(f)
        | PolicyDecision::Hold(f)
        | PolicyDecision::Block(f)
        | PolicyDecision::Ignore(f) => f,
    };
    Some(finding_record(finding))
}

pub(crate) fn finding_record(finding: &ralph_core::PolicyFinding) -> ValidationError {
    // U2 (2026-07-23-002 plan, KTD2): `SemanticGateViolation` carries
    // its own `gate` and `referenced_fields`. The legacy `field` slot
    // stays empty for semantic-gate violations — `field` is reserved
    // for single-field schema violations and must NOT carry the gate
    // ID (RF3). Schema-level variants keep populating `field` and
    // leave `gate`/`referenced_fields` as `None` so the JSON shape
    // stays backwards-compatible (skip_serializing_if = None).
    let (field, reason_code, gate, referenced_fields) = match &finding.violation_type {
        ViolationType::MissingRequiredField { field } => (
            field.clone(),
            "missing_required_field".to_string(),
            None,
            None,
        ),
        ViolationType::InvalidFieldValue { field, .. } => {
            (field.clone(), "invalid_field_value".to_string(), None, None)
        }
        ViolationType::PayloadTypeMismatch { .. } => (
            String::new(),
            "payload_type_mismatch".to_string(),
            None,
            None,
        ),
        ViolationType::TerminalMonotonicityViolation { .. } => (
            String::new(),
            "terminal_monotonicity_violation".to_string(),
            None,
            None,
        ),
        ViolationType::DuplicateTerminalEvent { .. } => (
            String::new(),
            "duplicate_terminal_event".to_string(),
            None,
            None,
        ),
        ViolationType::BusinessEventAfterCompletion { .. } => (
            String::new(),
            "business_event_after_completion".to_string(),
            None,
            None,
        ),
        ViolationType::InvalidTopicFormat { .. } => (
            String::new(),
            "invalid_topic_format".to_string(),
            None,
            None,
        ),
        ViolationType::TopicDenied { .. } => {
            (String::new(), "topic_denied".to_string(), None, None)
        }
        ViolationType::SemanticGateViolation {
            gate: g,
            referenced_fields: rf,
            ..
        } => (
            String::new(),
            "semantic_gate_violation".to_string(),
            Some(g.clone()),
            Some(rf.clone()),
        ),
        ViolationType::DuplicateWorkDone { .. } => {
            (String::new(), "duplicate_work_done".to_string(), None, None)
        }
    };
    ValidationError {
        payload_index: 0, // caller (single vs batch) fills this in
        field,
        reason_code,
        message: finding.message.clone(),
        gate,
        referenced_fields,
        // U4 (plan 2026-08-06-001, R2/R3): propagate the
        // structured evidence from the PolicyFinding into the
        // ValidationError so the JSON / text projection surfaces
        // the same observed facts / violated invariant / required
        // proof that the loop prompt uses (R2: same source of
        // truth for CLI and runtime feedback).  Mechanical /
        // legacy findings carry `evidence = None`, so the new
        // fields stay `None` and the JSON shape is unchanged for
        // them.
        observed: finding.evidence.as_ref().map(|ev| {
            ev.observed
                .iter()
                .map(|o| {
                    serde_json::json!({
                        "field": o.field,
                        "value": match &o.value {
                            ralph_core::correction::ObservationValue::Value(v) => {
                                serde_json::Value::String(v.clone())
                            }
                            ralph_core::correction::ObservationValue::Unavailable => {
                                serde_json::Value::Null
                            }
                            ralph_core::correction::ObservationValue::Unchecked => {
                                serde_json::Value::String("unchecked".into())
                            }
                        },
                    })
                })
                .collect()
        }),
        invariant: finding.evidence.as_ref().and_then(|ev| {
            if ev.invariant.is_empty() {
                None
            } else {
                Some(ev.invariant.clone())
            }
        }),
        required_proof: finding.evidence.as_ref().and_then(|ev| {
            if ev.proof.is_empty() {
                None
            } else {
                Some(ev.proof.clone())
            }
        }),
        ..Default::default()
    }
}

/// Output format for the validation-failure response. Mirrors
/// `wave::WaveOutputFormat` so we can reuse the same `--output`
/// flag from both `ralph emit` and `ralph wave emit` (when wired
/// up; wave is the U4 entry point).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Human-readable summary on stderr.
    Text,
    /// Structured JSON on stdout.
    Json,
}

/// Top-level failure payload structure (the new shape defined in
/// the U4 plan). Kept as a top-level shape so `serde_json::to_string`
/// produces a stable, agent-friendly document.
#[derive(Debug, Serialize)]
pub struct ValidationFailure {
    pub ok: bool,
    pub error: &'static str,
    pub topic: String,
    pub validation_errors: Vec<ValidationError>,
}

impl ValidationFailure {
    /// Build a failure payload from a [`BatchValidation`].
    pub fn from_batch(topic: &str, batch: BatchValidation) -> Self {
        Self {
            ok: false,
            error: "policy_validation_failed",
            topic: topic.to_string(),
            validation_errors: batch.errors,
        }
    }

    /// 2026-07-09-001 plan (U5): enrich every item in
    /// `validation_errors` with the U3 fields
    /// (`expected` / `actual` / `field_description` /
    /// `suggested_payload_shape` / `suggested_command`) by
    /// looking at the matching payload in the original
    /// batch (`payloads[index]`) and the topic's
    /// `EventSchema`. `payloads` is passed as a slice of
    /// `serde_json::Value` so the function is testable
    /// without I/O; the wave CLI parses each line into a
    /// `Value` before calling this.
    ///
    /// `validation_errors[].payload_index` is preserved so
    /// the agent can map back to the original batch entry.
    /// Required: U5 / R14a / SC2a contract — every error
    /// must carry `payload_index` so the agent fixes the
    /// whole batch in one round.
    pub fn enrich_with_schema(
        mut self,
        topic: &str,
        hat: Option<&str>,
        payloads: &[serde_json::Value],
        schema: Option<&EventSchema>,
    ) -> Self {
        for error in &mut self.validation_errors {
            let payload = payloads.get(error.payload_index);
            let enriched =
                enrich_validation_error_with_topic(error.clone(), topic, hat, payload, schema);
            *error = enriched;
        }
        self
    }
}

/// Emit the validation failure in the requested output mode. Returns
/// `Err(anyhow::Error)` so the caller can `?`-propagate with a
/// non-zero exit; both modes print the failure before the error is
/// raised so the agent / operator can see the structured details.
pub fn emit_policy_validation_failure(
    failure: &ValidationFailure,
    output: OutputMode,
) -> Result<()> {
    match output {
        OutputMode::Text => {
            // R22 (U4 plan): short human summary on stderr.
            let total = failure.validation_errors.len();
            // Aggregate the most common field to give a focused hint.
            let mut field_counts: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for v in &failure.validation_errors {
                if !v.field.is_empty() {
                    *field_counts.entry(v.field.as_str()).or_insert(0) += 1;
                }
            }
            let field_hint = if let Some((field, count)) = field_counts.iter().next() {
                format!("missing required field '{field}' in {count}")
            } else if total > 0 {
                failure.validation_errors[0]
                    .reason_code
                    .replace('_', " ")
                    .clone()
            } else {
                "policy check".to_string()
            };
            eprintln!(
                "policy validation failed: {total} payload{}, {field_hint}",
                if total == 1 { "" } else { "s" }
            );
            if let Some(repair_block) =
                render_validation_error_repair_block(&failure.topic, &failure.validation_errors)
            {
                eprintln!("{repair_block}");
            }
        }
        OutputMode::Json => {
            // R19: structured JSON on stdout (machine-parseable).
            println!("{}", serde_json::to_string(failure)?);
        }
    }
    anyhow::bail!("policy validation failed for topic '{}'", failure.topic);
}

/// Build [`EmitResultParts`] for CLI `--output json` paths.
///
/// U4 (2026-07-06-002 plan, R5): the `target_path` parameter carries the
/// resolved events file path so it can surface in `EmitResult.target_path`
/// (`recorded: true` 的 apply 路径有效;其它路径默认 `None`).
///
/// U2 (2026-07-06-004 fix-plan): the `payload` parameter carries
/// the JSON payload the caller is about to emit. When `ok=true`
/// AND the typed `emit_result_summary` config is on AND the
/// payload contains a valid `handoff_envelope`, the summary is
/// extracted via `validate_handoff_envelope_payload` →
/// `HandoffEnvelopeSummary::from(...)` so the agent can confirm
/// the envelope was recognised. `ok=false` paths always emit
/// `None` per `assemble.rs` forced-clear logic.
#[allow(clippy::too_many_arguments)]
pub fn build_emit_result_parts(
    topic: String,
    ok: bool,
    recorded: bool,
    errors: Vec<ralph_core::emit_result::EmitError>,
    config: Option<&RalphConfig>,
    workspace: &Path,
    hat: Option<&str>,
    target_path: Option<String>,
    payload: Option<&str>,
) -> ralph_core::emit_result::assemble::EmitResultParts {
    use ralph_core::emit_result::resolve_emit_routing_from_config;
    use ralph_core::handoff_envelope::validate_handoff_envelope_payload;

    let routing = resolve_emit_routing_from_config(config, workspace, hat);
    let handoff_envelope = if ok && payload.is_some() && envelope_summary_enabled(config) {
        payload.and_then(|p| {
            serde_json::from_str::<serde_json::Value>(p)
                .ok()
                .and_then(
                    |value| match validate_handoff_envelope_payload(&value, None) {
                        Ok(parsed) => Some(ralph_core::emit_result::HandoffEnvelopeSummary::from(
                            &parsed,
                        )),
                        Err(_) => None,
                    },
                )
        })
    } else {
        None
    };
    ralph_core::emit_result::assemble::EmitResultParts {
        ok,
        recorded,
        topic,
        phase: routing.phase,
        allowed_next: routing.allowed_next,
        activate_next: Vec::new(),
        errors,
        handoff: None,
        target_path,
        handoff_envelope,
    }
}

/// Whether the typed `emit_result_summary` config flag is on.
/// U2 (2026-07-06-004 fix-plan) gates the summary extraction on
/// this so pipeline presets and ad-hoc emits see no
/// behavioural change.
pub(crate) fn envelope_summary_enabled(config: Option<&RalphConfig>) -> bool {
    config
        .map(|c| c.event_loop.handoff_envelope.emit_result_summary)
        .unwrap_or(false)
}

/// 2026-07-09-001 plan (U4): take a freshly built
/// `PolicyCheckReport` and, for each `validation_error`,
/// consult the loaded `EventSchema` + the original payload
/// to populate the U3 enrichment fields
/// (`expected` / `actual` / `field_description` /
/// `suggested_payload_shape` / `suggested_command`). The
/// schema is the same one the unified pipeline ran against,
/// so any `field_docs` / `allowed_values` the preset author
/// declared flows through to the agent. Schemas without
/// field metadata still produce the legacy four-field
/// shape (U3 backward-compat).
///
/// `payload` is `Option<&serde_json::Value>`: the unified
/// pipeline doesn't thread the payload back to the
/// `PolicyCheckReport` builder, so the caller passes
/// whatever it has. The function is pure: no I/O, no
/// clock, no env.
pub fn enrich_report_with_schema(
    mut report: PolicyCheckReport,
    topic: &str,
    hat: Option<&str>,
    payload: Option<&serde_json::Value>,
    schema: Option<&EventSchema>,
) -> PolicyCheckReport {
    for error in &mut report.validation_errors {
        let enriched =
            enrich_validation_error_with_topic(error.clone(), topic, hat, payload, schema);
        *error = enriched;
    }
    report
}

/// U7 (2026-07-06-001 plan): bridge `PolicyCheckReport` → `EmitResult`
/// for the `--output json` policy-check rejection path.
///
/// 2026-07-09-001 plan (U4): when the report carries enriched
/// `validation_errors` (the U3 path), use them to build the
/// `EmitError` list so the agent can read `field` /
/// `expected` / `actual` / `field_description` /
/// `suggested_payload_shape` / `suggested_command` per item.
/// Falls back to the legacy `reason_codes` + `suggestions`
/// flattening when `validation_errors` is empty (e.g. the
/// pre-U3 path or a topic that has no schema metadata).
pub fn report_to_emit_result(
    report: &PolicyCheckReport,
    config: Option<&RalphConfig>,
) -> ralph_core::emit_result::EmitResult {
    let errors = if report.validation_errors.is_empty() {
        ralph_core::emit_result::map_policy_report_to_errors(
            &report.reason_codes,
            &report.suggestions,
        )
    } else {
        validation_errors_to_emit_errors(&report.validation_errors)
    };

    let parts = build_emit_result_parts(
        report.topic.clone(),
        false,
        false,
        errors,
        config,
        &report.workspace,
        report.hat.as_deref(),
        None,
        // U2: rejection paths never carry a payload summary
        // because the assemble layer forced-clears the field
        // when `ok=false`. Pass `None` for clarity.
        None,
    );
    ralph_core::emit_result::EmitResult::assemble(parts)
}

/// 2026-07-09-001 plan (U4 + U3): convert a `Vec<ValidationError>`
/// into the `EmitError` shape `EmitResult` expects,
/// threading the U3 enrichment fields (`field`,
/// `field_description`, `suggested_payload_shape`,
/// `suggested_command` plus the new `expected` / `actual`)
/// into the JSON consumer. The function is pure: it does
/// not consult the schema; the caller (or
/// `enrich_report_with_schema`) is responsible for
/// populating those fields before calling this.
///
/// `expected` / `actual` are sourced as `Option<String>` on
/// `ValidationError` but `EmitError` carries them as
/// `Option<serde_json::Value>` (per the U3 plan). String
/// sources are wrapped in `Value::String` so the JSON shape
/// stays additive: the field becomes a JSON string scalar
/// rather than being absent.
pub(crate) fn validation_errors_to_emit_errors(
    errors: &[ValidationError],
) -> Vec<ralph_core::emit_result::EmitError> {
    use ralph_core::emit_result::EmitError;
    errors
        .iter()
        .map(|e| EmitError {
            code: e.reason_code.clone(),
            message: e.message.clone(),
            field: if e.field.is_empty() {
                None
            } else {
                Some(e.field.clone())
            },
            suggested_command: e.suggested_command.clone(),
            expected: e
                .expected
                .as_ref()
                .map(|s| serde_json::Value::String(s.clone())),
            actual: e
                .actual
                .as_ref()
                .map(|s| serde_json::Value::String(s.clone())),
            field_description: e.field_description.clone(),
            suggested_payload_shape: e.suggested_payload_shape.clone(),
        })
        .collect()
}

/// Render a concise repair block from enriched validation errors.
///
/// This is shared by the CLI text rejection path and the wave
/// precheck text path so agents see the same fix guidance in both
/// command families.
pub fn render_validation_error_repair_block(
    topic: &str,
    errors: &[ValidationError],
) -> Option<String> {
    if errors.is_empty() {
        return None;
    }

    let mut lines = vec![format!("Repair hints for topic `{topic}`:")];
    for error in errors {
        let mut header = String::from("- ");
        if error.field.is_empty() {
            header.push_str("payload-level violation");
        } else {
            header.push_str(&format!("field `{}`", error.field));
        }
        if errors.len() > 1 || error.payload_index != 0 {
            header.push_str(&format!(" (payload[{}])", error.payload_index));
        }
        lines.push(header);

        if let Some(desc) = error.field_description.as_deref() {
            lines.push(format!("  meaning: {desc}"));
        }
        if let Some(expected) = error.expected.as_deref() {
            lines.push(format!("  expected: {expected}"));
        }
        if let Some(actual) = error.actual.as_deref() {
            lines.push(format!("  actual: {actual}"));
        }
        if let Some(shape) = error.suggested_payload_shape.as_ref() {
            lines.push(format!("  suggested payload shape: {shape}"));
        }
        if let Some(cmd) = error.suggested_command.as_deref() {
            lines.push(format!("  rerun: {cmd}"));
        }
        // U4 (plan 2026-08-06-001, R2): render the structured
        // evidence on the text projection as well so the human
        // and the agent see the same source of truth.  Distinct
        // from `expected` / `actual` (which the mechanical path
        // uses) and from `suggested_*` (which the semantic path
        // forbids).
        if let Some(observed) = error.observed.as_ref() {
            if !observed.is_empty() {
                let pairs: Vec<String> = observed
                    .iter()
                    .map(|v| {
                        let field = v
                            .get("field")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("?");
                        let value = v
                            .get("value")
                            .map(|x| x.to_string())
                            .unwrap_or_else(|| "?".to_string());
                        format!("{field}={value}")
                    })
                    .collect();
                lines.push(format!("  observed: {}", pairs.join(", ")));
            } else {
                lines.push(
                    "  observed: (none — gate did not return any fact-checked observations)"
                        .to_string(),
                );
            }
        }
        if let Some(invariant) = error.invariant.as_deref() {
            lines.push(format!("  invariant: {invariant}"));
        }
        if let Some(proof) = error.required_proof.as_deref() {
            lines.push(format!("  must re-prove: {proof}"));
        }
    }

    Some(lines.join("\n"))
}
