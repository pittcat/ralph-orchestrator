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

use ralph_core::{EventLoop, LoopContext, RalphConfig};
use ralph_proto::HatId;
use tracing::{error, warn};

use super::activation_outcome::{
    ActivationOutcomeFacts, ActivationOutcomeStatus, log_activation_outcome, snapshot_channel,
};
use super::late_events::output_mentions_ralph_emit;
use super::paths::resolve_emit_events_path;

/// Outcome of the activation-outcome close block. The runner uses
/// `empty_terminal_channel` to drive the existing missing-terminal
/// recovery path; the activation outcome row is observation-only.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NormalMergeOutcome {
    /// `true` when the merged channel had zero bytes — the runner
    /// treats this as a missing emit and preserves the existing
    /// recovery / fallback path.
    pub empty_terminal_channel: bool,
}

/// Snapshot the pre-merge channel state and merge the isolated
/// hat-channel back into the main events file, then write a single
/// bounded `hat_activation_outcome` row to `runtime-trace.jsonl`.
///
/// This is the entry point extracted from `inner.rs` so the
/// orchestration body stays at the HARD RULE 5000-line ceiling. The
/// function is observation-only: it never alters `task.resume`,
/// retry, recovery, or any other loop decision. The caller is
/// responsible for surfacing `NormalMergeOutcome::empty_terminal_channel`
/// to the existing missing-terminal branch.
///
/// Snapshot-before-merge ordering (U2): `snapshot_channel` runs
/// *before* `merge_hat_channel` so the activation outcome row
/// captures the pre-merge `Empty` / `Missing` / `Unreadable` /
/// non-empty state — production would otherwise re-read metadata
/// on the (already deleted) empty channel and misclassify the row
/// as `unreadable`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_activation_outcome_for_normal_merge(
    ctx: &LoopContext,
    config: &RalphConfig,
    state_machine_enabled: bool,
    event_loop: &EventLoop,
    outcome_backend_exit_code: Option<i32>,
    outcome_watchdog_timeout: bool,
    output: &str,
    display_hat: &HatId,
    loop_id: &str,
    success: bool,
    backend_termination: Option<&String>,
    iteration: u64,
) -> NormalMergeOutcome {
    let mut result = NormalMergeOutcome::default();

    let channel_path = crate::loop_runner::paths::resolve_hat_channel_events_path(ctx);
    let pre_snapshot = snapshot_channel(channel_path.as_deref());
    let pre_bytes = pre_snapshot.bytes;
    let channel_path_display = channel_path
        .as_ref()
        .map(|p| p.display().to_string());

    let target_events_path = resolve_emit_events_path(ctx, state_machine_enabled);
    let merge_result = crate::loop_runner::hat_channel::merge_hat_channel(
        ctx,
        &target_events_path,
        display_hat.as_str(),
        Some(config),
    );
    let merge_succeeded = merge_result.is_ok();
    if let Err(e) = merge_result {
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
            "Failed to merge isolated hat channel; events may be lost (see diagnostic file)"
        );
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

    // U2: refine the PRE-MERGE snapshot with the merge outcome.
    // The snapshot was captured *before* `merge_hat_channel`
    // deleted the file; the `refined_snapshot` therefore preserves
    // the pre-merge `Empty` / `Missing` / `Unreadable` distinction
    // even though the file no longer exists on disk by the time
    // the activation outcome row is written.
    let refined_snapshot =
        super::activation_outcome::refine_after_merge(pre_snapshot, merge_succeeded);
    let facts = ActivationOutcomeFacts {
        loop_id: Some(ctx.loop_id().unwrap_or(loop_id).to_string()),
        channel_exists: refined_snapshot.bytes.is_some()
            || matches!(refined_snapshot.status, ActivationOutcomeStatus::Empty),
        channel_bytes: refined_snapshot.bytes,
        channel_readable: !matches!(
            refined_snapshot.status,
            ActivationOutcomeStatus::Unreadable
        ),
        merge_succeeded,
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
        ..Default::default()
    };
    log_activation_outcome(
        event_loop.diagnostics().session_dir(),
        iteration,
        display_hat.as_str(),
        &refined_snapshot,
        &facts,
    );

    result
}