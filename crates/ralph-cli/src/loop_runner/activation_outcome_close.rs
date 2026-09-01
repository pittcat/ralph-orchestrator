//! Plan 2026-08-15-1823 (fix empty channel activation observability)
//! Unit 1: split the `if isolated_mode` block from `inner.rs` into a
//! dedicated sibling module so `inner.rs` stays at or below the
//! `CLAUDE.md`/`AGENTS.md` HARD RULE 5000-line ceiling.
//!
//! The function preserves the production semantics of the original
//! block; subsequent Units (U2 snapshot-before-merge reorder, U5
//! move-after-process_output) build on top of this surface. The
//! interrupt path lives in [`super::entry::merge_isolated_channel_on_interrupt`]
//! and does not depend on this module.

use ralph_core::event_loop::ProcessedEvents;
use ralph_core::{EventLoop, LoopContext, RalphConfig};
use ralph_proto::HatId;
use std::path::Path;
use tracing::{error, info, warn};

use super::activation_outcome::{
    ActivationOutcomeFacts, ChannelSnapshot, channel_exists_for, channel_readable_for,
    channel_reference_for_log, log_activation_outcome_with_diagnostics, refine_after_merge,
    snapshot_channel_with_workspace,
};
use super::late_events::output_mentions_ralph_emit;
use super::paths::resolve_emit_events_path;

/// Intermediate state captured before `event_loop.process_output` so
/// the activation outcome row can be written *after* event processing
/// without re-running the channel read or losing the merge result.
pub(crate) struct NormalMergeState {
    /// Snapshot captured *before* `merge_hat_channel` so the
    /// persisted row records the pre-merge `Empty` / `Missing` /
    /// `Unreadable` / non-empty state (U2).
    pub snapshot: ChannelSnapshot,
    /// Whether `merge_hat_channel` returned `Ok`. Drives
    /// `refine_after_merge` and the row's `merge_succeeded` flag.
    pub merge_succeeded: bool,
}

/// Outcome of the activation-outcome close block. The runner uses
/// `empty_terminal_channel` to drive the existing missing-terminal
/// recovery path; the activation outcome row is observation-only.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NormalMergeOutcome {
    /// `true` when the merged channel had zero bytes — the runner
    /// treats this as a missing emit and preserves the existing
    /// recovery / fallback path.
    pub empty_terminal_channel: bool,
    /// Pre-merge channel size. Fed to `classify_silent_activation`
    /// at the publish-obligation gate so MergeFailed is distinguishable
    /// from NeverEmitted.
    pub channel_bytes: Option<u64>,
    /// Post-retry merge result.
    pub merge_succeeded: bool,
    /// `true` when a second `merge_hat_channel_at_path` attempt ran
    /// (S4.1 / S4.2).
    pub merge_retried: bool,
}

/// Capture the pre-merge snapshot and merge the isolated hat-channel
/// back into the main events file. Returns the state required to
/// write the activation outcome row *after* `event_loop.process_output`
/// (U5) plus the `empty_terminal_channel` flag that drives the
/// missing-terminal recovery path.
///
/// `process_output` MUST be called before
/// [`write_activation_outcome_for_normal_merge`] — the row's four
/// event counters come from the runner's processed state, not from
/// the merge itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_normal_merge(
    ctx: &LoopContext,
    config: &RalphConfig,
    state_machine_enabled: bool,
    display_hat: &HatId,
    success: bool,
    outcome_watchdog_timeout: bool,
    backend_termination: Option<&String>,
    output: &str,
    owned_channel_path: Option<&Path>,
) -> (NormalMergeOutcome, NormalMergeState) {
    let mut result = NormalMergeOutcome::default();

    let channel_path = owned_channel_path.map(Path::to_path_buf);
    let pre_snapshot =
        snapshot_channel_with_workspace(channel_path.as_deref(), Some(ctx.workspace()));
    let pre_bytes = pre_snapshot.bytes;
    let channel_path_display = channel_path
        .as_deref()
        .and_then(|path| channel_reference_for_log(Some(path), ctx.workspace()));

    let target_events_path = resolve_emit_events_path(ctx, state_machine_enabled);
    let merge_once = || {
        crate::loop_runner::hat_channel::merge_hat_channel_at_path(
            ctx,
            &target_events_path,
            display_hat.as_str(),
            Some(config),
            channel_path.as_deref(),
        )
    };
    let first_merge = merge_once();
    // 2026-09-01-001 plan U4 (S4.1): a non-empty channel whose first
    // merge hit a transient IO error is retried once. Empty-channel
    // errors already quarantine the file inside `merge_hat_channel`
    // and must NOT be retried — a retry would see a missing file and
    // return Ok, collapsing NeverEmitted into a false merge success.
    let (merge_succeeded, merge_retried, merge_err) = match first_merge {
        Ok(()) => (true, false, None),
        Err(first_err) => {
            let channel_still_present = channel_path
                .as_deref()
                .is_some_and(|path| path.exists());
            if pre_bytes.unwrap_or(0) > 0 && channel_still_present {
                #[cfg(test)]
                run_merge_retry_test_hook();
                match merge_once() {
                    Ok(()) => {
                        info!(
                            hat = %display_hat.as_str(),
                            reason = "merge_failed_retried",
                            "U4: isolated hat-channel merge succeeded on retry"
                        );
                        (true, true, None)
                    }
                    Err(retry_err) => (false, true, Some(retry_err)),
                }
            } else {
                (false, false, Some(first_err))
            }
        }
    };
    result.channel_bytes = pre_bytes;
    result.merge_succeeded = merge_succeeded;
    result.merge_retried = merge_retried;
    if let Some(e) = merge_err {
        // 2026-07-03-002 plan U4: 从 warn! 升级为 error! + emit 诊断文件。
        // 093813 run 暴露:merge 失败仅 warn! 导致 operator 看不到 events
        // 丢失风险。emit 诊断让 operator 能看到,loop 继续走 fallback。
        crate::loop_runner::hat_channel::emit_channel_routing_fallback_diagnostic(
            ctx,
            display_hat.as_str(),
            "merge_hat_channel_failed",
        );
        error!(
            error = %e,
            hat = %display_hat.as_str(),
            retried = merge_retried,
            "Failed to merge isolated hat channel; events may be lost (see diagnostic file)"
        );
        if pre_bytes.unwrap_or(0) > 0
            && let Some(path) = channel_path.as_deref()
            && path.exists()
            && let Err(qerr) =
                crate::loop_runner::hat_channel::quarantine_failed_channel(ctx, path)
        {
            warn!(
                error = %qerr,
                hat = %display_hat.as_str(),
                "U4: failed to quarantine merge-failed hat channel"
            );
        }
        // An empty channel is a known missing-terminal condition,
        // not an unreadable-channel condition. Preserve the
        // responsible-hat recovery path even though the merge now
        // fails closed instead of returning success.
        if pre_bytes == Some(0) {
            result.empty_terminal_channel = true;
        }
    } else if pre_bytes == Some(0) {
        // Only treat an empty channel as a missing emit after the
        // channel was merged successfully. A missing or unreadable
        // channel is a routing failure and must stay on the existing
        // fallback path instead of being retried as an agent error.
        result.empty_terminal_channel = true;
        warn!(
            hat = %display_hat.as_str(),
            channel_path = %channel_path_display.as_deref().unwrap_or("<unknown>"),
            channel_bytes = 0u64,
            backend_success = success,
            watchdog_timeout = outcome_watchdog_timeout,
            backend_termination = ?backend_termination,
            output_bytes = output.len(),
            output_mentions_emit = output_mentions_ralph_emit(output),
            "Isolated hat activation ended with an empty event channel"
        );
    }

    let state = NormalMergeState {
        snapshot: pre_snapshot,
        merge_succeeded,
    };
    (result, state)
}

/// Write the activation outcome row AFTER `event_loop.process_output`
/// (U5). The `processed` snapshot is the
/// `Option<&ProcessedEventsWithWaves>` returned by the runner's
/// process phase; when the runner cannot provide it the function
/// falls back to zero counters. The row carries the pre-merge
/// snapshot refined by the merge outcome, plus bounded backend and
/// event-processing scalars.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_activation_outcome_for_normal_merge(
    event_loop: &EventLoop,
    ctx: &LoopContext,
    state: NormalMergeState,
    outcome_backend_exit_code: Option<i32>,
    outcome_watchdog_timeout: bool,
    output: &str,
    display_hat: &HatId,
    loop_id: &str,
    success: bool,
    backend_termination: Option<&String>,
    iteration: u64,
    processed: Option<&ProcessedEvents>,
    wave_policy_rejection_count: usize,
    wave_raw_count: usize,
) {
    let refined_snapshot = refine_after_merge(state.snapshot, state.merge_succeeded);
    // Build event counters from the runner's processed snapshot, then
    // overlay the bounded backend/channel facts for this activation.
    let event_facts = ActivationOutcomeFacts::from_processed(
        processed,
        wave_policy_rejection_count,
        wave_raw_count,
    );
    let facts = ActivationOutcomeFacts {
        loop_id: Some(ctx.loop_id().unwrap_or(loop_id).to_string()),
        channel_exists: channel_exists_for(refined_snapshot.status),
        channel_bytes: refined_snapshot.bytes,
        channel_readable: channel_readable_for(refined_snapshot.status),
        merge_succeeded: state.merge_succeeded,
        backend_success: success,
        backend_exit_code: outcome_backend_exit_code,
        watchdog_timeout: outcome_watchdog_timeout,
        backend_termination: backend_termination.is_some(),
        output_bytes: output.len() as u64,
        output_mentions_emit: output_mentions_ralph_emit(output),
        terminal_obligation_topics: event_loop
            .registry()
            .get_config(display_hat)
            .map(|hat| hat.terminal_events.clone())
            .unwrap_or_default(),
        ..event_facts
    };
    log_activation_outcome_with_diagnostics(
        event_loop.diagnostics(),
        iteration,
        display_hat.as_str(),
        &refined_snapshot,
        &facts,
    );
}

#[cfg(test)]
std::thread_local! {
    static MERGE_RETRY_TEST_HOOK: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_merge_retry_test_hook(hook: Option<fn()>) {
    MERGE_RETRY_TEST_HOOK.with(|cell| cell.set(hook));
}

#[cfg(test)]
fn run_merge_retry_test_hook() {
    MERGE_RETRY_TEST_HOOK.with(|cell| {
        if let Some(hook) = cell.get() {
            hook();
        }
    });
}
