//! Runner entry point and outer lifecycle.
//!
//! This module hosts the public `run_loop_impl` wrapper that every
//! `ralph run` / `ralph resume` / `ralph web` invocation reaches, plus
//! the supporting helpers it owns:
//!
//! - Termination sentinel helpers (`loop_termination_sentinel_path`,
//!   `remove_loop_termination_sentinel`, `write_loop_termination_sentinel`)
//! - Interrupt-path channel merge (`merge_isolated_channel_on_interrupt`)
//! - Bootstrap event persistence (`persist_starting_event_to_events_file`)
//!
//! The actual loop body lives in [`super::inner`]. The split is
//! pure refactor — no behavioural change.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use ralph_core::{EventLoop, LoopContext, RalphConfig, TerminationReason};
use tracing::{error, warn};

use crate::cli::{ColorMode, Verbosity};

use super::inner::run_loop_impl_inner;
use super::paths::resolve_current_events_path;
use super::paths::resolve_emit_events_path;

/// U3: path to the lightweight termination sentinel written by the loop
/// runner before returning a non-success [`TerminationReason`]. The parent
/// `ralph run` process (stdio or TUI) reads this file after the child exits
/// to recover the exact reason without relying on coarse exit-code mapping.
fn loop_termination_sentinel_path(loop_context: &Option<LoopContext>) -> PathBuf {
    loop_context
        .as_ref()
        .map(|ctx| ctx.ralph_dir().join("loop-termination-reason.json"))
        .unwrap_or_else(|| PathBuf::from(".ralph/loop-termination-reason.json"))
}

/// U3: remove any stale termination sentinel at loop start so a successful
/// run cannot be misclassified by an artifact from a previous run.
fn remove_loop_termination_sentinel(loop_context: &Option<LoopContext>) {
    let path = loop_termination_sentinel_path(loop_context);
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

/// U3: write the termination reason sentinel so the parent process can
/// recover the exact reason after the loop runner exits.
fn write_loop_termination_sentinel(loop_context: &Option<LoopContext>, reason: &TerminationReason) {
    let path = loop_termination_sentinel_path(loop_context);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        warn!(
            target: "ralph_cli::loop_runner",
            error = %e,
            path = %path.display(),
            "Failed to create termination sentinel parent directory"
        );
        return;
    }
    match serde_json::to_string(reason) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                warn!(
                    target: "ralph_cli::loop_runner",
                    error = %e,
                    path = %path.display(),
                    "Failed to write termination sentinel"
                );
            }
        }
        Err(e) => {
            warn!(
                target: "ralph_cli::loop_runner",
                error = %e,
                "Failed to serialize termination reason sentinel"
            );
        }
    }
}

/// Best-effort merge of the isolated hat-channel into the main events file.
///
/// Called from both interrupt paths (iteration-top and mid-loop `tokio::select!`)
/// so an OPERATOR_ABORT / SIGTERM / SIGHUP / timeout does not strand events that
/// the active hat already wrote to its isolated channel.
///
/// Repro of the latent bug fixed here: `docs/report/2026-08-07-merge-batch-primary-20260806-230934-diagnosis.md`
/// (DEV-001A, P0 mechanism, confidence 90).
///
/// Properties:
/// - Idempotent: `merge_hat_channel` itself removes the channel file on success
///   and skips when the marker is missing, so this never produces duplicate events.
/// - Fail-soft: errors are logged at `error` level and an operator-facing
///   diagnostic file is emitted; we never propagate so interrupt flows stay
///   non-blocking.
/// - Falls back to `"ralph"` as the authoritative hat label when no
///   `state.last_hat` has been recorded (e.g. very first interrupt before any
///   iteration has run); this case always finds an empty channel so the
///   fallback label is cosmetic.
#[allow(clippy::too_many_arguments)]
pub(crate) fn merge_isolated_channel_on_interrupt(
    ctx: &LoopContext,
    config: &RalphConfig,
    state_machine_enabled: bool,
    event_loop: &EventLoop,
    interrupt_kind: &'static str,
) {
    let target_events_path = resolve_emit_events_path(ctx, state_machine_enabled);
    let authoritative_hat = event_loop
        .state()
        .last_hat
        .as_ref()
        .map(|h| h.as_str())
        .unwrap_or("ralph");

    match crate::loop_runner::hat_channel::merge_hat_channel(
        ctx,
        &target_events_path,
        authoritative_hat,
        Some(config),
    ) {
        Ok(()) => {
            // On success the channel file and `current-hat-events` marker are
            // already removed by `merge_hat_channel`; nothing else to do.
        }
        Err(e) => {
            crate::loop_runner::hat_channel::emit_channel_routing_fallback_diagnostic(
                ctx,
                authoritative_hat,
                "merge_hat_channel_failed_on_interrupt",
            );
            error!(
                target: "ralph_cli::loop_runner",
                error = %e,
                hat = %authoritative_hat,
                interrupt_kind = %interrupt_kind,
                "Failed to merge isolated hat channel on interrupt; events may be lost (see diagnostic file)"
            );
        }
    }
}

/// U5 (2026-06-17-004 R5): append a single JSONL record for the
/// configured `starting_event` (typically `work.start` for serial
/// presets, `task.start` otherwise) to the trusted events file
/// resolved from the current-events marker.
///
/// The record shape mirrors what `ralph emit` would write — a
/// top-level `topic`, JSON-string `payload`, RFC3339 `ts`, and
/// `source: "loop-bootstrap"` so downstream provenance checks
/// recognise this as an orchestrator-owned write.  We deliberately
/// omit the `hat` field (consistent with the orchestrator's
/// internal emits) so the origin guard does not need to whitelist a
/// new producer identity.
///
/// The freshly-built `EventLoop` calls
/// `sync_event_reader_to_file_end()` immediately after
/// `with_context` so the appended record is not re-delivered to the
/// bus.  Resume mode never reaches this function — it uses
/// `EventLoop::initialize_resume` which emits `task.resume` to the
/// bus without persisting a new bootstrap record.
///
/// Returns `Err` on I/O failure (e.g. directory not writable).  The
/// caller already logs a `warn!` and continues because the history
/// logger retains a copy of the start event regardless.
pub(crate) fn persist_starting_event_to_events_file(
    ctx: &LoopContext,
    topic: &str,
    prompt_content: &str,
) -> std::io::Result<()> {
    use std::io::Write;

    let events_path = resolve_current_events_path(ctx);

    // Use the same JSON object shape `ralph emit` produces so
    // `EventReader` can parse the line uniformly.  We build it as a
    // `serde_json::Value` first so missing or oddly-escaped fields
    // surface as a single serialization error rather than corrupting
    // the events file with a partial line.
    let record = serde_json::json!({
        "topic": topic,
        "payload": prompt_content,
        "ts": chrono::Utc::now().to_rfc3339(),
        "source": "loop-bootstrap",
    });
    let line = serde_json::to_string(&record).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("serialize: {e}"))
    })?;

    // `append(true)` + `create(true)` mirrors the hard-gate writers
    // in `hard_gate.rs`; the file is normally already created by the
    // surrounding `if !resume` block, but we tolerate races where
    // another process clears it between marker write and persistence.
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)?;
    writeln!(file, "{line}")?;
    file.flush()?;
    Ok(())
}

/// Core loop implementation supporting both fresh start and continue modes.
///
/// This public wrapper exists so U3 can write a termination sentinel for
/// every non-success return without threading sentinel logic through the
/// huge `run_loop_impl_inner` body. The inner function performs all real work
/// and returns the typed reason; the wrapper then persists the sentinel and
/// forwards the result unchanged.
///
/// # Arguments
///
/// * `resume` - If true, publishes `task.resume` instead of `task.start`,
///   signaling the planner to read existing scratchpad rather than doing fresh gap analysis.
/// * `record_session` - If provided, records all events to the specified JSONL file for replay testing.
/// * `auto_merge_override` - Explicit auto-merge setting. If `Some(false)`, disables auto-merge
///   (equivalent to `--no-auto-merge`). If `None`, uses `config.features.auto_merge`.
/// * `resume_loop_id` - Explicit loop ID to use when resuming (`--loop-id`).
///   If `None` and `resume` is true, reuses the existing `current-loop-id` marker.
/// * `resume_manifest` - U2 (plan 2026-08-03-004): the VALIDATED parallel-forge
///   resume manifest threaded from the reuse gate. When present (and `resume`
///   is false), the loop bootstrap re-binds the manifest's pending hat to its
///   original trigger through the existing `task.resume` recovery contract
///   instead of publishing the configured starting event. `None` keeps the
///   plain fresh-start semantics unchanged.
#[allow(clippy::too_many_arguments, clippy::large_futures)]
pub async fn run_loop_impl(
    config: RalphConfig,
    color_mode: ColorMode,
    resume: bool,
    enable_tui: bool,
    enable_rpc: bool,
    verbosity: Verbosity,
    record_session: Option<PathBuf>,
    loop_context: Option<LoopContext>,
    custom_args: Vec<String>,
    auto_merge_override: Option<bool>,
    resume_loop_id: Option<String>,
    resume_manifest: Option<ralph_core::parallel_forge_resume::ResumeManifest>,
    warmup_only: bool,
    force_warmup: bool,
    prebuilt_diagnostics: Option<Arc<ralph_core::diagnostics::DiagnosticsCollector>>,
    no_sync_agent_docs: bool,
    source_is_builtin_embedded: bool,
    hats_source_label: Option<String>,
) -> Result<TerminationReason> {
    remove_loop_termination_sentinel(&loop_context);
    // plan 2026-07-25-001 U4: `config` moves into `run_loop_impl_inner`, so
    // snapshot the notifications settings before the call.
    let notifications_config = config.notifications.clone();
    let result = run_loop_impl_inner(
        config,
        color_mode,
        resume,
        enable_tui,
        enable_rpc,
        verbosity,
        record_session,
        loop_context.clone(),
        custom_args,
        auto_merge_override,
        resume_loop_id,
        resume_manifest,
        warmup_only,
        force_warmup,
        prebuilt_diagnostics,
        no_sync_agent_docs,
        source_is_builtin_embedded,
        hats_source_label,
    )
    .await;
    if let Ok(ref reason) = result
        && !reason.is_success()
    {
        write_loop_termination_sentinel(&loop_context, reason);
    }
    // plan 2026-07-25-001 U4: single chokepoint for loop-completion webhook
    // notifications — every `Ok(reason)` return passes through here, so the
    // shortcut paths inside `run_loop_impl_inner` are covered too. Strictly
    // best-effort: never mutates `result`.
    if let Ok(ref reason) = result {
        crate::loop_runner::notifications::notify_loop_termination(
            &notifications_config,
            &loop_context,
            reason,
        )
        .await;
    }
    result
}
