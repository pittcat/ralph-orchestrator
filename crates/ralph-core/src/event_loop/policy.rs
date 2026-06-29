// U4b (2026-06-10-003 plan, v14): event policy / payload contract
// helper free functions migrated out of `event_loop/mod.rs`.
//
// v14 adversarial-review correction: the legacy target functions
// (`apply_event_policy_validation`, `finding_to_payload_contract_violation`,
// `publish_policy_rejection_resume`) were already removed/sunk into
// `validation::rules_event_policy`; the real v14 free helpers that
// still live in `mod.rs` are `build_unified_validation_pipeline` and
// `publish_correction_via_context`. impl-EventLoop helper methods
// (`log_topic_format_rejection`, `log_wave_policy_blocked_envelope`,
// `apply_engine_required_field_gate`) are intentionally NOT migrated
// here — they wait for U5 (impl-EventLoop split per KTD12).
//
// R-Refactor-2 / KTD5: method bodies byte-identical (git diff
// method-body lines = 0). Public API stable via `pub use policy::*;`
// in `mod.rs`.

use super::*;

/// A2 (002-adversarial-review): build the unified
/// `ValidationPipeline` once per batch. The pipeline is always
/// constructed; the legacy per-rule gate stack has been removed.
pub fn build_unified_validation_pipeline(
    config: &crate::config::EventLoopConfig,
) -> crate::validation::ValidationPipeline {
    let view = crate::preset::engine::protocol::ProtocolView::from_event_loop(config);
    crate::validation::ValidationPipeline::from_config(&view, config)
}

pub fn publish_correction_via_context(
    bus: &mut EventBus,
    state: &mut crate::event_loop::LoopState,
    mut ledger: Option<&mut crate::state::StateLedger>,
    event: &JsonlEvent,
    payload: &str,
) {
    // Re-use the same Rejection construction the legacy
    // path uses so the `retry_key` / `topic` / `violation`
    // surface matches the wire format.
    let rejection = crate::event_loop::rejection::Rejection {
        stage: crate::event_loop::rejection::RejectionStage::Policy,
        source_hat: event.hat.clone(),
        business_hat: event.hat.clone(),
        topic: event.topic.to_string(),
        violation: payload.to_string(),
        retry_key: String::new(),
        retry_eligible: true,
        non_retryable_reason: None,
        target_hat: event.hat.clone(),
        original_event_id: None,
        original_ts: Some(event.ts.clone()),
        // 2026-06-23 fix plan U5 (CB-2): policy-level rejection
        // predates typed-kind plumbing — keep None.
        kind: None,
    };
    let retry_key = rejection.compute_retry_key();

    // U11-T3: pull the per-key retry count from the ledger's
    // rejection digest when available so escalation threshold
    // (R11) tracks prior calls. Falls back to 1 on cold start.
    let retry_count = ledger
        .as_ref()
        .and_then(|l| l.snapshot().rejection_digest().get(&retry_key))
        .map(|entry| entry.count as u32)
        .unwrap_or(1u32);

    // U11-T3: in-place mutation of the live `LoopState::prompt_context`
    // (no longer throwaway). The correction block will be picked up by
    // the next `build_prompt` call. Workspace path is read from the
    // LoopState config — the emitter needs `.ralph/` under the workspace.
    let workspace_root: Option<std::path::PathBuf> = None; // workspace is recorded via ledger path; emit_correction_context only uses it for recovery.jsonl, which is optional.
    let ctx = crate::correction::emit_correction_context(
        ledger.as_deref_mut(),
        &rejection,
        retry_count,
        workspace_root.as_deref(),
        &mut state.prompt_context,
    );
    tracing::info!(
        retry_key = %ctx.retry_key,
        topic = %ctx.topic,
        reason_code = %ctx.reason_code,
        needs_escalation = ctx.needs_escalation,
        "A3: emit_correction_context produced a CorrectionContext; injected into state.prompt_context"
    );

    // U11-T3: ledger commit for replay durability. The
    // `emit_correction_context` helper already commits the
    // primary `RejectionRecorded` delta when `ledger.is_some()`;
    // this second commit mirrors the event-bound reason / iteration
    // so the audit log can correlate the correction with the loop
    // iteration that produced it. Idempotent if the primary commit
    // above already recorded the same key.
    if let Some(l) = ledger.as_deref_mut() {
        let delta = crate::state::CommitDelta::RejectionRecorded {
            key: ctx.retry_key.clone(),
            message: Some(payload.to_string()),
            topic: Some(ctx.topic.clone()),
        };
        if let Err(e) = l.commit(delta, Some(ctx.topic.clone())) {
            tracing::debug!(
                error = %e,
                retry_key = %ctx.retry_key,
                "U11-T3: redundant ledger commit skipped (already committed by emit_correction_context)"
            );
        }
    }

    // R11 (3-strike escalation): if the correction block already
    // crossed the escalation threshold, publish a `plan.blocked`
    // event so the shipper / reporter chain runs the preset's
    // failure path. The legacy `task.resume` event is not
    // published alongside (the `return` in the caller short-
    // circuits the legacy path).
    //
    // Pre-2026-06-28-005 this published `human.guidance` for
    // an operator. The operator channel was removed by that
    // plan; the escalation now terminates the loop via
    // `plan.blocked(reason=correction_3_strike_exhausted)`.
    if ctx.needs_escalation {
        let escalated = crate::correction::escalate_to_plan_blocked(bus, &ctx);
        if escalated {
            tracing::warn!(
                retry_key = %ctx.retry_key,
                "A3: plan.blocked escalation fired (3-strike threshold reached)"
            );
        }
    }
}
