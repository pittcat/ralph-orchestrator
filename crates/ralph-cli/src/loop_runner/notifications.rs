//! plan 2026-07-25-001 U4: loop-termination webhook notification dispatch.
//!
//! `notify_loop_termination` is the single best-effort call site mounted on
//! `run_loop_impl`: every `Ok(reason)` return of the loop passes through it.
//! It never returns an error and never panics, so a hung or failing webhook
//! endpoint can never alter the loop's [`TerminationReason`] or block loop
//! exit (a bounded outer timeout wraps the dispatch).

use ralph_core::LoopContext;
use ralph_core::config::NotificationsConfig;
use ralph_core::event_loop::TerminationReason;
use ralph_core::notifications::ReqwestTransport;
use ralph_core::notifications::TerminationContext;
use ralph_core::notifications::dispatch;

/// Dispatches loop-completion webhook notifications, best-effort.
///
/// Returns immediately when `config.enabled` is false. Otherwise builds a
/// [`TerminationContext`] from `loop_context` + `reason` and awaits dispatch
/// under a bounded outer timeout so a hung endpoint can never block loop
/// exit. This function NEVER returns an error, NEVER panics, and can never
/// change the loop's [`TerminationReason`].
///
/// `iteration_current` / `iteration_max` / `active_hat` are left empty: the
/// `run_loop_impl` wrapper cannot cheaply observe them (plan U4 explicitly
/// allows the minimal v1 variable set).
pub(crate) async fn notify_loop_termination(
    config: &NotificationsConfig,
    loop_context: &Option<LoopContext>,
    reason: &TerminationReason,
) {
    if !config.enabled {
        return;
    }

    let loop_id = loop_context
        .as_ref()
        .and_then(|ctx| ctx.loop_id())
        .unwrap_or("primary");
    let status = if reason.is_success() {
        "success"
    } else {
        "failure"
    };
    let workspace = loop_context
        .as_ref()
        .map(|ctx| ctx.workspace().display().to_string())
        .unwrap_or_default();
    let repo_root = loop_context
        .as_ref()
        .map(|ctx| ctx.repo_root().display().to_string())
        .unwrap_or_default();

    let ctx = TerminationContext::new(
        loop_id,
        status,
        reason.as_str(),
        workspace,
        repo_root,
        "",
        "",
        "",
    );

    // Per-endpoint timeout is enforced inside the transport; the outer budget
    // covers the worst case (every endpoint hitting its full timeout) plus a
    // small slack so dispatch always finishes before the outer timeout fires
    // under normal operation.
    let budget = std::time::Duration::from_secs(
        config
            .timeout_seconds
            .saturating_mul(config.endpoints.len().max(1) as u64)
            .saturating_add(5),
    );
    let transport = ReqwestTransport;

    // Best-effort: ignore elapsed/err entirely — NEVER propagate.
    let _ = tokio::time::timeout(budget, dispatch(config, &ctx, reason, &transport)).await;
}
