use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LateEventRecovery {
    NoLateEvents,
    PendingWork,
    Terminate(TerminationReason),
}

pub const LATE_EVENT_RECOVERY_MAX_POLLS: u32 = 5;
pub const LATE_EVENT_RECOVERY_POLL_INTERVAL_MS: u64 = 50;
pub const EMIT_RECOVERY_MAX_POLLS: u32 = 20;
pub const EMIT_RECOVERY_POLL_INTERVAL_MS: u64 = 250;

pub fn poll_for_late_events(
    event_loop: &mut EventLoop,
    max_polls: u32,
    poll_interval_ms: u64,
) -> std::io::Result<LateEventRecovery> {
    for poll_attempt in 0..=max_polls {
        let processed = event_loop.process_events_from_jsonl()?;

        if let Some(reason) = event_loop.check_cancellation_event() {
            return Ok(LateEventRecovery::Terminate(reason));
        }

        if let Some(reason) = event_loop.check_completion_event() {
            return Ok(LateEventRecovery::Terminate(reason));
        }

        if event_loop.has_pending_events() {
            return Ok(LateEventRecovery::PendingWork);
        }

        let observed_events = processed.had_events
            || processed.has_orphans
            || processed.human_interact_context.is_some();

        if observed_events {
            debug!(
                had_events = processed.had_events,
                has_orphans = processed.has_orphans,
                had_human_interact = processed.human_interact_context.is_some(),
                "Late JSONL drain found events but no new pending work"
            );
            return Ok(LateEventRecovery::NoLateEvents);
        }

        if poll_attempt == max_polls {
            break;
        }

        std::thread::sleep(Duration::from_millis(poll_interval_ms));
    }

    Ok(LateEventRecovery::NoLateEvents)
}

pub fn recover_late_events_before_fallback(
    event_loop: &mut EventLoop,
) -> std::io::Result<LateEventRecovery> {
    poll_for_late_events(
        event_loop,
        LATE_EVENT_RECOVERY_MAX_POLLS,
        LATE_EVENT_RECOVERY_POLL_INTERVAL_MS,
    )
}

pub fn recover_expected_emit_after_output(
    event_loop: &mut EventLoop,
) -> std::io::Result<LateEventRecovery> {
    poll_for_late_events(
        event_loop,
        EMIT_RECOVERY_MAX_POLLS,
        EMIT_RECOVERY_POLL_INTERVAL_MS,
    )
}

pub fn output_mentions_ralph_emit(output: &str) -> bool {
    output.contains("ralph emit")
}
