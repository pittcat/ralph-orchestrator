use super::*;

pub fn should_hard_gate(hat_id: &HatId, event_loop: &EventLoop) -> bool {
    let Some(config) = event_loop.registry().get_config(hat_id) else {
        return false;
    };
    !config.publishes.is_empty() && config.default_publishes.is_none()
}

/// Determine whether the active hat has an emit obligation but produced no events.
///
/// This catches the "completely forgot to emit" case where the agent output
/// does not even mention `ralph emit`. Contrast with `should_hard_gate` which
/// only triggers when the agent claims to emit but writes no event.
pub fn should_gate_missing_events(hat_id: &HatId, event_loop: &EventLoop) -> bool {
    let Some(config) = event_loop.registry().get_config(hat_id) else {
        return false;
    };
    // Hat has an obligation to publish but no automatic fallback
    !config.publishes.is_empty() && config.default_publishes.is_none()
}

/// U6: Handle execution contract rejections for operator visibility.
///
/// Logs warnings with topic, hat, reason, task_id for each rejection so
/// operators see the rejection in the console. Delegates structured
/// diagnostics to `DiagnosticsCollector::log_execution_contract_rejections`,
/// which writes an `OrchestrationEvent::ExecutionContractRejected` entry to
/// `orchestration.jsonl` — the standard path that TUI/RPC observers
/// already subscribe to via the EventBus. No-op when diagnostics are
/// disabled. Does NOT terminate the loop — guidance drives the next
/// iteration.
///
/// 2026-06-04 plan U7: Also emits `OrchestrationEvent::ContractRecoveryRouted`
/// for each rejected topic so operators can distinguish
///   (a) rejected event will be retried by the source hat, from
///   (b) rejected event has no safe retry target and needs human intervention.
pub fn handle_execution_contract_rejections(
    processed: &ralph_core::ProcessedEvents,
    event_loop: &mut EventLoop,
    hat_id: &HatId,
) {
    let rejections = &processed.contract_rejections;
    if rejections.is_empty() {
        return;
    }

    let iteration = event_loop.state().iteration;
    let hat_name = hat_id.as_str();

    // Console-visible warning for each rejection. Include retry_target
    // status when available so operators can see at a glance whether the
    // rejection will auto-recover or needs intervention.
    for finding in rejections {
        let recovery = compute_recovery_status(event_loop, finding.topic.as_str());
        match &recovery {
            Some(target) => warn!(
                topic = %finding.topic,
                hat = %hat_name,
                violation = ?finding.kind,
                message = %finding.message,
                retry_target = %target,
                "Execution contract rejected event; targeted recovery routed to source hat"
            ),
            None => warn!(
                topic = %finding.topic,
                hat = %hat_name,
                violation = ?finding.kind,
                message = %finding.message,
                "Execution contract rejected event; NO safe retry target — recovery is human.guidance only"
            ),
        }

        // Structured diagnostics: record whether recovery was routed.
        let (retry_target, no_retry_reason) = match &recovery {
            Some(t) => (Some(t.clone()), None),
            None => (
                None,
                Some("no safe retry target (see human.guidance)".to_string()),
            ),
        };
        event_loop.diagnostics().log_orchestration(
            iteration,
            hat_name,
            ralph_core::diagnostics::OrchestrationEvent::ContractRecoveryRouted {
                topic: finding.topic.clone(),
                retry_target,
                no_retry_reason,
            },
        );
    }

    // Structured diagnostics: writes to orchestration.jsonl under
    // .ralph/diagnostics/<session>/. The TUI/RPC observer chain consumes
    // these via the standard EventBus path, so no separate file is needed.
    event_loop
        .diagnostics()
        .log_execution_contract_rejections(iteration, hat_name, rejections);
}

/// Inspects the bus for a targeted `task.resume` event whose target is a
/// registered hat. Returns the target hat name if a recovery was published
/// for the given topic in the current iteration's pending queues.
pub fn compute_recovery_status(event_loop: &mut EventLoop, topic: &str) -> Option<String> {
    let bus = event_loop.bus();
    for hat_id in bus.hat_ids() {
        if let Some(pending) = bus.peek_pending(hat_id) {
            for event in pending {
                if event.topic.as_str() == "task.resume"
                    && event.payload.contains(topic)
                    && let Some(target) = event.target.as_ref()
                {
                    return Some(target.as_str().to_string());
                }
            }
        }
    }
    None
}

/// Inject a human.guidance event directly into the events file so the agent
/// sees it on the next iteration. Used when the agent claimed to emit but no
/// event was actually written.
pub fn inject_hard_gate_guidance(ctx: &LoopContext, hat_id: &HatId, expected_topics: &[String]) {
    let events_path = resolve_current_events_path(ctx);
    let topics_str = if expected_topics.is_empty() {
        "(check hat configuration)".to_string()
    } else {
        expected_topics.join("`, `")
    };

    let payload = format!(
        "⚠️ HARD GATE TRIGGERED: Previous iteration by hat `{hat}` claimed to emit an event, \
         but NO EVENT WAS WRITTEN to the events file.\n\n\
         You MUST use the bash tool to execute: ralph emit <topic>\n\
         Allowed topics: `{topics}`\n\n\
         Writing `ralph emit` in prose or comments is NOT sufficient. \
         The turn is incomplete until the command succeeds and the event appears in the events file.",
        hat = hat_id.as_str(),
        topics = topics_str
    );

    let timestamp = chrono::Utc::now().to_rfc3339();
    let event = serde_json::json!({
        "topic": "human.guidance",
        "payload": payload,
        "ts": timestamp,
    });

    match serde_json::to_string(&event) {
        Ok(line) => {
            use std::io::Write;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path);
            match file {
                Ok(f) => {
                    let mut writer = std::io::BufWriter::new(f);
                    if writeln!(writer, "{}", line).is_err() {
                        warn!(path = ?events_path, "Failed writing hard-gate guidance event");
                    }
                }
                Err(e) => {
                    warn!(error = %e, path = ?events_path, "Failed opening events file for hard-gate guidance");
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed serializing hard-gate guidance event");
        }
    }
}

/// Inject a human.guidance event when the agent completely forgot to emit.
///
/// This is distinct from `inject_hard_gate_guidance` which handles the case
/// where the agent claimed to emit but no event was written. This function
/// handles the case where the agent simply did not emit any event at all.
pub fn inject_missing_event_hard_gate_guidance(
    ctx: &LoopContext,
    hat_id: &HatId,
    expected_topics: &[String],
) {
    let events_path = resolve_current_events_path(ctx);
    let topics_str = if expected_topics.is_empty() {
        "(check hat configuration)".to_string()
    } else {
        expected_topics.join("`, `")
    };

    let payload = format!(
        "⚠️ HARD GATE TRIGGERED: Previous iteration by hat `{hat}` did NOT emit any event.\n\n\
         This hat is configured to publish events but emitted nothing. Ralph cannot \
         proceed without observable completion signals.\n\n\
         You MUST use the bash tool to execute: ralph emit <topic>\n\
         Allowed topics: `{topics}`\n\n\
         If the work is complete, emit `work.done`. If the work failed, emit `work.failed`. \
         Do not update files or write prose without emitting an event.",
        hat = hat_id.as_str(),
        topics = topics_str
    );

    let timestamp = chrono::Utc::now().to_rfc3339();
    let event = serde_json::json!({
        "topic": "human.guidance",
        "payload": payload,
        "ts": timestamp,
    });

    match serde_json::to_string(&event) {
        Ok(line) => {
            use std::io::Write;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path);
            match file {
                Ok(f) => {
                    let mut writer = std::io::BufWriter::new(f);
                    if writeln!(writer, "{}", line).is_err() {
                        warn!(path = ?events_path, "Failed writing missing-event hard-gate guidance");
                    }
                }
                Err(e) => {
                    warn!(error = %e, path = ?events_path, "Failed opening events file for missing-event hard-gate guidance");
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed serializing missing-event hard-gate guidance event");
        }
    }
}

pub fn resolve_display_hat_for_execution(
    event_loop: &EventLoop,
    hat_id: &HatId,
    preview_display_hat: &HatId,
) -> HatId {
    if hat_id.as_str() != "ralph" {
        return hat_id.clone();
    }

    event_loop
        .state()
        .last_active_hat_ids
        .first()
        .cloned()
        .unwrap_or_else(|| preview_display_hat.clone())
}

pub fn resolve_hat_for_output_processing(hat_id: &HatId, display_hat: &HatId) -> HatId {
    if hat_id.as_str() == "ralph" {
        display_hat.clone()
    } else {
        hat_id.clone()
    }
}
