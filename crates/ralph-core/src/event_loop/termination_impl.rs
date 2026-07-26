// U5b: termination text formatting helper free function SSOT。
//
// 从 event_loop/mod.rs 迁出 format_duration + termination_status_text。
// 注意:已有的 `event_loop::termination`(SSOT)保持不动,本文件用于补充
// termination text 格式化逻辑,避免命名冲突。
//
// 命名约定:`termination_impl` (而非 `termination`) 是 v14 强制约束 —
// 2026-06-23-005 落地的 typed TerminationTrigger SSOT 占了 `termination` 命名空间。
// KTD14:本 plan 不得覆盖 SSOT 模块。
//
// R-Refactor-2 / KTD5:helper 方法字节级未变(grep diff 验证)。

use super::*;

/// Formats a duration as human-readable string.
pub fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

/// Returns a human-readable status based on termination reason.
pub fn termination_status_text(reason: &TerminationReason) -> &'static str {
    match reason {
        TerminationReason::CompletionPromise => "All tasks completed successfully.",
        TerminationReason::MaxIterations => "Stopped at iteration limit.",
        TerminationReason::MaxRuntime => "Stopped at runtime limit.",
        TerminationReason::MaxCost => "Stopped at cost limit.",
        TerminationReason::ConsecutiveFailures => "Too many consecutive failures.",
        TerminationReason::LoopThrashing => {
            "Loop thrashing detected - same hat repeatedly blocked."
        }
        TerminationReason::LoopStale => {
            "Stale loop detected - same topic emitted 3+ times consecutively."
        }
        TerminationReason::ValidationFailure => "Too many consecutive malformed JSONL events.",
        TerminationReason::Stopped => "Manually stopped.",
        TerminationReason::Interrupted => "Interrupted by signal.",
        TerminationReason::RestartRequested => "Restarting by human request.",
        TerminationReason::WorkspaceGone => "Workspace directory removed externally.",
        TerminationReason::Cancelled => "Cancelled gracefully (human rejection or timeout).",
        TerminationReason::PayloadContractViolation => "Payload contract violation - loop paused.",
        TerminationReason::RecoveryExhausted { .. } => {
            "Recovery responder exhausted retry window - loop paused."
        }
        TerminationReason::ReviewFailed { .. } => {
            "Review verdict failed and propagated to final mirror - loop terminated."
        }
        TerminationReason::ScopeViolationCircuitBreakerTripped { .. } => {
            "Isolated scope violation circuit breaker tripped - loop terminated."
        }
        TerminationReason::RecoverablePayloadExhausted { .. } => {
            "Recoverable-payload budget exhausted - loop terminated."
        }
        // 2026-06-26 plan U1: completion-rejection budget exhausted
        // (recoverable) OR structural rejection. The two are
        // disambiguated by the structured `source` field on the
        // payload; the human-readable text is the same so the
        // operator drills into the payload for details.
        TerminationReason::CompletionStuck(_) => {
            "Completion stuck - correction budget exhausted or structural rejection."
        }
        // U5 (plan 2026-07-04-004): dimension-reviewer
        // scope_violation hard-reject. Surfaces as a typed
        // termination so dashboards can distinguish the silent-
        // success guard from a generic payload contract violation.
        // The full hat + diff_stat context lives in the variant
        // fields; the status text points operators at the audit
        // chain.
        TerminationReason::ScopeViolationHardRejected { .. } => {
            "dimension-reviewer scope_violation - hard-rejected, loop terminated."
        }
        // U1 (plan 2026-07-27-001): production fan-in reached a
        // terminal failure (persistent store/merge error or
        // unresolvable ContinueCollect). Distinct from MaxRuntime —
        // the wave dispatched but the supervisor could not converge.
        TerminationReason::FanInFailed => {
            "Wave fan-in failed - supervisor could not reach terminal state."
        }
    }
}
