//! Plan 2026-08-05-001 U10 split: free functions and helpers moved
//! verbatim from `event_loop/mod.rs` that build the runtime flow /
//! stage pipeline / fix-unit guard payload from a resolved
//! `RalphConfig`, and the `prompt_preview` facade that lets the
//! `ralph inspect prompt` CLI compute a preview without constructing
//! an `EventLoop`. Behaviour is identical to the previous mod.rs
//! location; only module scope changed.

use super::prompt_types::{PromptGates, PromptPreview, default_evidence_level};
use crate::config::RalphConfig;
use crate::hat_registry::HatRegistry;
use crate::skill_registry::SkillRegistry;
use ralph_proto::HatId;

/// Minimal FlowDeclaration YAML retained for documentation and legacy
/// test fixtures. Hat-only presets no longer fall back to this at
/// runtime — see [`StagePipeline::with_hat_only_stages_for_loop_config`].
#[allow(dead_code)]
pub(super) fn minimal_flow_declaration_yaml() -> &'static str {
    // U11 (2026-06-27-002 plan completion) requires
    // `FlowStepScopeStage` to be fail-closed when the
    // topic is outside the declared `allowed_emits`
    // set. The minimal fallback flow therefore MUST
    // declare `unit_loop` (the default `current_step_id`
    // produced by `FlowLifecycleRegistry::current_step_id()`)
    // with a permissive `allowed_emits` set so that
    // presets without an explicit `mechanism:` block
    // continue to function as before. Operators who
    // want to enforce strict topic/step gating must
    // declare their own flow in the preset; the lint
    // `flow_declaration_missing` flags the absence.
    r"mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: []
    steps:
      - id: unit_loop
        allowed_emits:
          - work.start
          - work.start
          - work.ready
          - work.done
          - work.failed
          - test.passed
          - test.failed
          - fix.applied
          - fix.exhausted
          - task.resume
          - plan.complete
          - plan.blocked
          - plan.created
          - a.impl.done
          - b.impl.done
          - task.done
          - queue.advance
          - hypothesis.test
          - review.start
          - review.dimension.ready
          - review.dimension.done
          - review.complete
          - review.done
          - review.blocked
          - review.file
          - experiment.planned
          - experiment.ready
          - experiment.running
          - experiment.done
          - experiment.failed
          - build.blocked
          - build.done
          - loop.cancel
          - verify.passed
          - verify.failed
          - experiment.planned
          - seed.ready
          - REPORT_DONE
          - REVIEW_COMPLETE
          - LOOP_COMPLETE
          - event.malformed
          - event.isolation.boundary_violation
          - human.guidance
          - user.prompt
          - task.resume
          - task.relocate_legacy
          - task.relocate
          - repair.budget.exhausted
          - repair.close
          - report.done
          - aggregate.inbox
          - aggregate.done
          - stop_requested
          - restart_requested
"
}

/// U6: build the default emit-time stage pipeline from the loaded
/// `RalphConfig`.
///
/// Presets that **opt in** to `mechanism.flow` (top-level or legacy
/// `event_loop.mechanism`) get the full stage pipeline including
/// `FlowStepScopeStage` and `StepCloseObligationStage`.
///
/// Presets without `mechanism.flow` (hat-only linear chains such as
/// `ce-executor-pipeline`) skip flow-step gating; routing is driven by
/// hat triggers/publishes plus `event_policy`.
pub fn load_opt_in_flow_declaration(
    config: &crate::config::RalphConfig,
) -> Option<crate::event_loop::flow_declaration::FlowDeclaration> {
    use crate::event_loop::flow_declaration::FlowDeclaration;
    // Typed conversion — do NOT serde_yaml round-trip. Wrapping
    // `to_string(flow_cfg)` under `mechanism:\n  flow:\n` left the
    // body unindented, so `mechanism.flow` parsed as null and
    // `FlowStepScopeStage` rejected every emit with
    // `flow_step_undeclared` (work.ready never reached task-planner).
    effective_mechanism_config(config)
        .and_then(|m| m.flow.as_ref())
        .and_then(|flow_cfg| FlowDeclaration::from_config(flow_cfg).ok())
}

pub(super) fn effective_mechanism_config(
    config: &crate::config::RalphConfig,
) -> Option<&crate::config::MechanismConfig> {
    config
        .mechanism
        .as_ref()
        .or(config.event_loop.mechanism.as_ref())
}

pub(super) fn build_phase_authority_arc(
    config: &crate::config::RalphConfig,
) -> std::sync::Arc<crate::event_loop::phase_authority::WorkflowPhaseAuthority> {
    let authority = effective_mechanism_config(config)
        .and_then(|m| m.phase_authority.as_ref())
        .and_then(|cfg| {
            crate::event_loop::phase_authority::WorkflowPhaseAuthority::from_config(cfg).ok()
        })
        .unwrap_or_else(crate::event_loop::phase_authority::WorkflowPhaseAuthority::disabled);
    std::sync::Arc::new(authority)
}

pub fn build_stage_pipeline_from_config(
    config: &crate::config::RalphConfig,
) -> (
    crate::event_loop::stage_pipeline::StagePipeline,
    std::collections::HashMap<String, u32>,
    std::sync::Arc<crate::event_loop::phase_authority::WorkflowPhaseAuthority>,
) {
    use crate::event_loop::flow_declaration::FlowDeclaration;
    use crate::event_loop::stage_pipeline::StagePipeline;
    let loop_cfg = Some(&config.event_loop);
    let authority = build_phase_authority_arc(config);
    // Top-level `mechanism:` (preset SSOT) and `event_loop.mechanism`
    // must both enable the phase pipeline — `build_phase_authority_arc`
    // already reads `effective_mechanism_config`.
    let phase_authority_enabled = authority.is_enabled();

    if phase_authority_enabled {
        let flow_yaml = load_opt_in_flow_declaration(config).unwrap_or_else(|| {
            FlowDeclaration::from_yaml(minimal_flow_declaration_yaml()).unwrap()
        });
        let step_totals: std::collections::HashMap<String, u32> = flow_yaml
            .steps
            .iter()
            .filter_map(|s| s.total_units.map(|n| (s.id.clone(), n)))
            .collect();
        let pipeline = StagePipeline::with_phase_authority_stages_for_loop_config(
            flow_yaml,
            loop_cfg,
            authority.clone(),
        );
        return (pipeline, step_totals, authority);
    }

    if let Some(flow_yaml) = load_opt_in_flow_declaration(config) {
        let step_totals: std::collections::HashMap<String, u32> = flow_yaml
            .steps
            .iter()
            .filter_map(|s| s.total_units.map(|n| (s.id.clone(), n)))
            .collect();
        let pipeline = StagePipeline::with_default_stages_for_loop_config(flow_yaml, loop_cfg);
        (pipeline, step_totals, authority)
    } else {
        let pipeline = StagePipeline::with_hat_only_stages_for_loop_config(loop_cfg);
        (pipeline, std::collections::HashMap::new(), authority)
    }
}

/// Validates events against configured workflow guards is implemented by
/// [`crate::validation::rules_workflow_guard::WorkflowGuardRule`], invoked
/// from the unified pre-commit / post-commit loop in
/// `process_parse_result`. The legacy free function
/// `apply_workflow_guard_validation` and its sibling
/// `WorkflowGuardOutcome` / `WorkflowGuardRejectionDetail` structs were
/// removed in U11-T4 (post-commit wiring); the recovery-envelope writer
/// `Self::log_workflow_guard_rejection` survives because it is
/// implementation-agnostic and is reused by the unified handler.
/// P1-1 (2026-07-01-002 audit): parse the `step` field out of a
/// `work.ready` payload and return it when (a) it claims to be a
/// `fix-NN` step and (b) the id is **not** present in
/// `fix_unit_known`.  Returns `None` for non-fix-unit steps,
/// malformed payloads, or already-known ids — those are not in
/// scope for the fix-unit range guard.
pub(super) fn unknown_fix_step(
    payload: Option<&str>,
    fix_unit_known: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let payload = payload?;
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let step_id = match value.get("step")? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => map.get("id")?.as_str()?.to_string(),
        _ => return None,
    };
    if !step_id.starts_with("fix-") {
        return None;
    }
    if fix_unit_known.contains(&step_id) {
        return None;
    }
    Some(step_id)
}

/// P1-1 (2026-07-01-002 audit): shape the `task.resume` payload
/// for JSONL events read from `apply_emit_gate`.  The JSONL
/// `Event` only carries `topic` / `hat` / `payload` — there is
/// no `source` field, so `target` is sourced from `hat`.
pub(super) fn build_invalid_step_target_resume_payload_for_jsonl(
    finding: &crate::execution_contract::ExecutionContractFinding,
    original_event: &crate::event_reader::Event,
    known_fix_units: &[String],
) -> String {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "stage".into(),
        serde_json::Value::String("FixUnitRangeGuard".into()),
    );
    payload.insert(
        "original_topic".into(),
        serde_json::Value::String(original_event.topic.clone()),
    );
    payload.insert(
        "violation".into(),
        serde_json::Value::String("invalid_step_target".into()),
    );
    payload.insert(
        "reason_code".into(),
        serde_json::Value::String(
            crate::validation::ReasonCode::CONTRACT_INVALID_STEP_TARGET.to_string(),
        ),
    );
    if let Some(hat) = original_event.hat.as_ref() {
        payload.insert("target".into(), serde_json::Value::String(hat.clone()));
    }
    payload.insert(
        "known_fix_units".into(),
        serde_json::Value::Array(
            known_fix_units
                .iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect(),
        ),
    );
    payload.insert(
        "guidance".into(),
        serde_json::Value::String(finding.message.clone()),
    );
    serde_json::to_string(&serde_json::Value::Object(payload)).unwrap_or_else(|_| "{}".to_string())
}

/// Pure config-driven preview that does **not** require a
/// constructed `EventLoop`. Used by `ralph inspect prompt` to
/// avoid the noisy `tracing::info!("Memory injection check…")`
/// path that runs when an EventLoop is constructed (its
/// initialization logs to stdout, which corrupts the JSON SSOT
/// contract). The `EventLoop::prompt_preview` method delegates to
/// this function with a closure that runs `build_prompt` for the
/// block-title extraction.
///
/// `block_titles` is supplied via a closure so the caller can opt
/// into the heavier `build_prompt`-driven extraction; the pure
/// CLI path passes `|_| Vec::new()` to keep the command
/// side-effect-free.
pub fn preview_prompt_for_config<F>(
    config: &RalphConfig,
    hat_id: &HatId,
    block_titles: F,
) -> Option<PromptPreview>
where
    F: FnOnce(&HatId) -> Vec<String>,
{
    let hat_registry = HatRegistry::from_config(config);
    if hat_registry.get(hat_id).is_none() && hat_id.as_str() != "ralph" {
        return None;
    }

    let skill_registry = SkillRegistry::from_config(
        &config.skills,
        std::path::Path::new(&config.core.workspace_root),
        Some(config.cli.backend.as_str()),
    )
    .unwrap_or_else(|_| SkillRegistry::new(Some(config.cli.backend.as_str())));

    let gates = PromptGates {
        tasks_enabled: config.tasks.enabled,
        memories_enabled: config.memories.enabled,
    };

    let (gated, registry_auto, on_demand) =
        super::prompt_types::SkillInjector::plan_auto_inject(config, hat_id, &skill_registry);

    let auto_inject = [gated, registry_auto].concat();
    let block_titles = block_titles(hat_id);

    Some(PromptPreview {
        hat_id: hat_id.as_str().to_string(),
        gates,
        auto_inject,
        on_demand,
        block_titles,
        // 2026-07-27-002 plan Unit 1: scenario injection defaults.
        // These are populated by `inspect_prompt_command` when
        // scenario args are supplied; the pure config path leaves
        // them at their default (None / "static").
        trigger_context_injected: None,
        wave_context_injected: None,
        orchestrator_context_injected: None,
        correction_injected: None,
        skill_gates: None,
        evidence_level: default_evidence_level(),
        // 2026-07-27-002 plan Unit 2: candidate emit preview.
        candidate_emit: None,
    })
}
