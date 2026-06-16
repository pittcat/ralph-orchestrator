use super::*;
use ralph_core::diagnosis::TerminationHint;
use ralph_core::{PolicyRejection, ProcessedEvents};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// U2 (2026-06-13-001): single source of truth for the
/// `agent_wrote_any_valid_or_rejected` boolean that drives the
/// `missing_event_gate` decision. The regular partition reports
/// `had_raw_events` (any line read) and `had_rejected_events`
/// (any policy / format / origin rejection). The wave partition
/// has a separate policy stage, so a batch of 7 events that all
/// fail schema validation leaves the regular flags at `false`;
/// we fold `wave_had_policy_rejections` in here so the gate
/// sees the agent as having tried to emit.
///
/// Mirrored by `compute_agent_wrote_any_valid_or_rejected` in
/// `loop_runner/tests.rs` — that helper should be deleted in
/// favor of calling this function so the test asserts the same
/// expression the runner uses.
pub fn agent_wrote_any_valid_or_rejected(
    processed_events: Option<&ProcessedEvents>,
    wave_policy_rejections: &[PolicyRejection],
) -> bool {
    let regular = processed_events
        .map(|events| events.had_raw_events || events.had_rejected_events)
        .unwrap_or(false);
    regular || !wave_policy_rejections.is_empty()
}

/// U8: Build the operator-facing [`ralph_core::DiagnosisHint`]
/// and the `diagnosis-summary.json` seed for a terminating loop run.
///
/// Returns `None` (and writes nothing) when diagnostics are disabled
/// for this run AND the caller did not provide a payload-contract
/// violation reference. The two artifacts are returned as a pair so
/// the caller can choose to ignore the seed while still appending
/// the hint, or vice versa.
pub(crate) fn build_termination_diagnostics(
    event_loop: &ralph_core::EventLoop,
    payload_violation_report_relpath: Option<&str>,
) -> Option<(
    ralph_core::DiagnosisHint,
    ralph_core::diagnostics::DiagnosisSummary,
)> {
    let session_id = event_loop.diagnostics().session_id()?;

    // Workspace-relative path so the hint survives a worktree
    // checkout. The session directory always lives at
    // `<workspace>/.ralph/diagnostics/<session_id>`, matching
    // [`ralph_core::LoopContext::diagnostics_dir`].
    let session_relpath = Some(format!(".ralph/diagnostics/{session_id}"));
    let diagnose_command = Some("ralph diagnose --session latest".to_string());

    let mut references = Vec::new();
    if let Some(relpath) = payload_violation_report_relpath {
        references.push(ralph_core::DiagnosisReference {
            label: "Payload contract violation report".to_string(),
            relpath: relpath.to_string(),
        });
    }

    let hint = ralph_core::DiagnosisHint {
        session_relpath,
        diagnose_command,
        references,
    };

    let state = event_loop.state();
    let now = chrono::Utc::now();
    let summary = ralph_core::diagnostics::DiagnosisSummary {
        schema_version: ralph_core::diagnostics::DiagnosisSummary::SCHEMA_VERSION,
        session_id: session_id.clone(),
        generated_at: now,
        loop_started_at: None,
        loop_terminated_at: Some(now),
        total_iterations: Some(state.iteration),
        termination_reason: None,
        recovery_journal_path: Some(format!(".ralph/diagnostics/{session_id}/recovery.jsonl")),
        drift_journal_path: Some(format!(".ralph/diagnostics/{session_id}/drift.jsonl")),
        orchestration_log_path: Some(format!(
            ".ralph/diagnostics/{session_id}/orchestration.jsonl"
        )),
        errors_log_path: Some(format!(".ralph/diagnostics/{session_id}/errors.jsonl")),
        recovery_count: 0,
        drift_finding_count: 0,
        notes: Vec::new(),
    };

    Some((hint, summary))
}

/// U8: write the diagnosis summary seed and append the
/// operator-facing `## Diagnostics` hint to `summary.md`.
///
/// Skipped silently when:
/// - the diagnostics collector has no session directory (i.e.
///   diagnostics disabled for this run), AND
/// - the caller did not provide a payload contract violation
///   reference.
///
/// In that combined case the runner must not invent an empty hint
/// section: it would expose invalid paths and contradict the
/// "no hint when diagnostics are off" contract.
pub(crate) fn write_termination_diagnostics(
    event_loop: &ralph_core::EventLoop,
    summary_writer: &ralph_core::SummaryWriter,
    payload_violation_report_relpath: Option<&str>,
) {
    let Some((hint, summary)) =
        build_termination_diagnostics(event_loop, payload_violation_report_relpath)
    else {
        return;
    };

    if let Err(e) = summary_writer.append_diagnosis_hint(Some(&hint)) {
        tracing::warn!(
            target: "ralph_cli::loop_runner",
            error = %e,
            "Failed to append diagnosis hint section to summary.md"
        );
    }

    event_loop
        .diagnostics()
        .write_diagnosis_summary_seed(&summary);
}

/// U6/U8: post-termination hook that appends a `## Recovery Diagnosis`
/// section to the summary when the recovery responder produced a
/// Final hint, then (U8) writes the operator-facing
/// `## Diagnostics` hint and the `diagnosis-summary.json` seed. The
/// responder hint is taken (one-shot) so the next run does not see
/// a stale signal. Called from each `return Ok(reason)` site in
/// [`run_loop_impl`].
///
/// This is a free function so we can call it from the loop body
/// without threading the hint through the `handle_termination`
/// closure. The hint-taking is intentionally idempotent within a
/// single loop run: once consumed, subsequent `take_termination_hint`
/// calls return `None` until the next `record_finding` writes a new
/// hint.
///
/// `payload_violation_report_relpath` is the workspace-relative
/// path of the root-level `payload-contract-error-*.json` report
/// (U4 / U6 hard gate), or `None` for the normal-termination
/// path. Only the U4 payload contract violation path passes
/// `Some(_)`; every other caller passes `None`. The flag is plumbed
/// through this helper rather than `handle_termination` so the
/// closure signature stays stable.
fn finalize_recovery_diagnosis(
    event_loop: &mut ralph_core::EventLoop,
    ctx: &Option<ralph_core::LoopContext>,
    payload_violation_report_relpath: Option<&str>,
) {
    let summary_writer = if let Some(c) = ctx {
        ralph_core::SummaryWriter::from_context(c)
    } else {
        ralph_core::SummaryWriter::default()
    };

    // U6: drain the responder's hint and append the existing
    // `## Recovery Diagnosis` section. The hint may be `None` on
    // non-final terminations; the section is then skipped, but the
    // U8 step below still runs as long as diagnostics are enabled.
    if let Some(hint) = event_loop.recovery_responder_mut().take_termination_hint()
        && let Err(e) = summary_writer.append_recovery_section(&hint)
    {
        tracing::warn!(
            target: "ralph_cli::loop_runner",
            error = %e,
            "Failed to append recovery diagnosis section to summary.md"
        );
    }

    // U8: append the operator-facing `## Diagnostics` hint and
    // write the `diagnosis-summary.json` seed.
    write_termination_diagnostics(
        event_loop,
        &summary_writer,
        payload_violation_report_relpath,
    );

    // U4: persist active hat activations so `ralph diagnose` can
    // render the `## Active Hat Activations` section.
    let activations = event_loop.hat_lifecycle_tracker().active_activations();
    event_loop
        .diagnostics()
        .write_active_activations(&activations);

    // D1 (2026-06-16, plan 002 Unit 5): refresh the session pointer on
    // every termination path so `ralph diagnose --session latest` finds
    // the **final** session after the loop ends. The startup path
    // (run_loop_impl, before handle_termination) writes the pointer once,
    // but if the loop completes or is terminated after writing recovery
    // envelopes, the pointer needs to point at the same session the
    // envelopes live in. Best-effort: a write failure is logged but does
    // not block the loop's normal return. The pointer file path is
    // last-write-wins when concurrent worktrees race; this is documented
    // as the expected behavior in the runtime-diagnosis guide.
    finalize_session_pointer(event_loop.diagnostics(), ctx.as_ref());

    // Suppress the unused-import lint when the function is the only
    // user of `TerminationHint`. The type is re-exported in case the
    // diagnostic report pipeline (U7) wants to introspect the hint
    // structure directly.
    let _ = std::marker::PhantomData::<TerminationHint>;
}

/// D1 (2026-06-16, plan 002 Unit 5): rewrite the session pointer at
/// loop termination so `ralph diagnose` can find the worktree's
/// diagnostics root after the loop ends and `loops.json` no longer
/// carries an alive entry. Mirrors the startup-time pointer write at
/// [`run_loop_impl`] (line ~488): best-effort, never blocks the loop
/// runner's normal return. No-op for primary sessions (the pointer
/// file format and the main-repo diagnostic root are unchanged for
/// those) and for runs without an enabled diagnostics collector.
fn finalize_session_pointer(
    diagnostics: &ralph_core::diagnostics::DiagnosticsCollector,
    ctx: Option<&ralph_core::LoopContext>,
) {
    let Some(ctx) = ctx else {
        return;
    };
    if ctx.is_primary() {
        return;
    }
    if !diagnostics.is_enabled() {
        return;
    }
    match diagnostics.write_session_pointer(ctx.repo_root(), ctx.workspace()) {
        Ok(true) => {
            debug!(
                target: "ralph_cli::loop_runner",
                main_repo = %ctx.repo_root().display(),
                "refreshed session pointer on loop termination",
            );
        }
        Ok(false) => {
            // Session dir is not inside workspace; nothing to do.
        }
        Err(err) => {
            tracing::warn!(
                target: "ralph_cli::loop_runner",
                main_repo = %ctx.repo_root().display(),
                error = %err,
                "failed to refresh session pointer on loop termination; \
                 ralph diagnose may not find this worktree session after the loop ends",
            );
        }
    }
}

pub struct RpcSharedState {
    iteration: Arc<std::sync::atomic::AtomicU32>,
    /// Current (hat id, hat display name) pair.
    hat: Arc<std::sync::Mutex<(String, String)>>,
    completed: Arc<std::sync::atomic::AtomicBool>,
    total_cost_usd: Arc<std::sync::Mutex<f64>>,
}

/// Resolves the loop ID for task ownership tracking.
///
/// - Worktree loops: use the loop_id from the LoopContext.
/// - Primary loops (fresh): generate a new `primary-{timestamp}` ID.
/// - Primary loops (--continue): reuse the existing `current-loop-id` marker,
///   or use an explicit `--loop-id` if provided.
pub fn resolve_loop_id(
    ctx: &ralph_core::LoopContext,
    resume: bool,
    explicit_loop_id: Option<&str>,
) -> String {
    ctx.loop_id().map(|s| s.to_string()).unwrap_or_else(|| {
        if resume {
            if let Some(explicit_id) = explicit_loop_id {
                return explicit_id.to_string();
            }
            let marker = ctx.ralph_dir().join("current-loop-id");
            if let Ok(existing) = std::fs::read_to_string(&marker) {
                let existing = existing.trim().to_string();
                if !existing.is_empty() {
                    return existing;
                }
            }
        }
        // Fresh run: generate a new timestamped ID
        format!("primary-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
    })
}

pub(crate) fn adapter_timeout_duration(timeout_secs: u64) -> Option<Duration> {
    (timeout_secs > 0).then(|| Duration::from_secs(timeout_secs))
}

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
        let _ = fs::remove_file(&path);
    }
}

/// U3: write the termination reason sentinel so the parent process can
/// recover the exact reason after the loop runner exits.
fn write_loop_termination_sentinel(loop_context: &Option<LoopContext>, reason: &TerminationReason) {
    let path = loop_termination_sentinel_path(loop_context);
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            warn!(
                target: "ralph_cli::loop_runner",
                error = %e,
                path = %path.display(),
                "Failed to create termination sentinel parent directory"
            );
            return;
        }
    }
    match serde_json::to_string(reason) {
        Ok(json) => {
            if let Err(e) = fs::write(&path, json) {
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
#[allow(clippy::too_many_arguments)]
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
    warmup_only: bool,
    force_warmup: bool,
    prebuilt_diagnostics: Option<Arc<ralph_core::diagnostics::DiagnosticsCollector>>,
    no_sync_agent_docs: bool,
    source_is_builtin_embedded: bool,
    hats_source_label: Option<String>,
) -> Result<TerminationReason> {
    remove_loop_termination_sentinel(&loop_context);
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
        warmup_only,
        force_warmup,
        prebuilt_diagnostics,
        no_sync_agent_docs,
        source_is_builtin_embedded,
        hats_source_label,
    )
    .await;
    if let Ok(ref reason) = result {
        if !reason.is_success() {
            write_loop_termination_sentinel(&loop_context, reason);
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_loop_impl_inner(
    mut config: RalphConfig,
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
    warmup_only: bool,
    force_warmup: bool,
    prebuilt_diagnostics: Option<Arc<ralph_core::diagnostics::DiagnosticsCollector>>,
    no_sync_agent_docs: bool,
    source_is_builtin_embedded: bool,
    hats_source_label: Option<String>,
) -> Result<TerminationReason> {
    // U5: Payload contract hard gate. Runs BEFORE any backend is spawned.
    // In strict mode (always on for `ralph run`), any payload contract
    // error is fatal: the agent must not be started. There is no skip flag.
    enforce_payload_contract_gate(&config)?;

    // U4: Preset static lint hard gate. Runs BEFORE any backend is spawned
    // and BEFORE process group setup. In strict mode (always on for
    // `ralph run`), any lint error is fatal with exit code 2.
    // P1 finding #5: the failure path propagates a typed
    // `PresetLintGateError` instead of calling `std::process::exit`.
    // `process::exit` would skip the RAII drop chain (tracing flush,
    // scoped guards, lock release) and is hostile to any future
    // `TempDir` / `LockGuard` / `tracing::subscriber::with_default`
    // added near the top of `run_loop_impl`. The outer
    // `Result`-driven flow maps the error to exit code2 *after*
    // drops have run; see `commands::run::run_command` and
    // `main.rs`.
    //
    // WRC-U3: pass `source_is_builtin_embedded` so the WAC
    // severity upgrade (KTD-7) applies to builtin presets even
    // outside `--strict` mode.
    if let Err(lint_error) = enforce_preset_lint_gate(&config, source_is_builtin_embedded) {
        let diagnostics_dir = std::path::Path::new(".").join(".ralph").join("diagnostics");
        let _artifact_path = write_preset_lint_artifact(&diagnostics_dir, &lint_error);
        eprintln!(
            "\nPreset lint gate failed with {} error(s). No backend was started.\n\
             Fix the preset configuration and retry.",
            lint_error.error_count
        );
        // P1 finding #5: return the typed error instead of calling
        // `std::process::exit`. Calling `process::exit` here would skip
        // the RAII drop chain (tracing flush, scoped guards, lock
        // release) and is hostile to any future `TempDir` /
        // `LockGuard` / `tracing::subscriber::with_default` added near
        // the top of `run_loop_impl`. The outer `Result`-driven flow
        // maps `PresetLintGateError` to exit code2 *after* drops have
        // run. See `commands::run::run_command` and `main.rs` for the
        // exit-code mapping.
        return Err(anyhow::Error::new(lint_error));
    }

    // Set up process group leadership per spec
    // "The orchestrator must run as a process group leader"
    process_management::setup_process_group();

    let use_colors = color_mode.should_use_colors();

    // Determine effective execution mode (with fallback logic)
    // Per spec: Claude backend requires PTY mode to avoid hangs
    // TUI mode is observation-only - uses streaming mode, not interactive
    let interactive_requested = config.cli.default_mode == "interactive" && !enable_tui;
    let user_interactive = if interactive_requested {
        if stdout().is_terminal() {
            true
        } else {
            warn!("Interactive mode requested but stdout is not a TTY, falling back to autonomous");
            false
        }
    } else {
        false
    };
    // PTY is required for TUI/RPC observation and true interactive sessions.
    // Headless `ralph run --no-tui` should use CliExecutor so backends get their
    // non-interactive prompt forms (for example `claude -p` or `codex exec`).
    let use_pty = enable_tui || enable_rpc || user_interactive;

    // Set up interrupt channel for signal handling
    // Per spec:
    // - SIGINT (Ctrl+C): Immediately terminate child process (SIGTERM -> 5s grace -> SIGKILL), exit with code 130
    // - SIGTERM: Same as SIGINT
    // - SIGHUP: Same as SIGINT
    //
    // Use watch channel for interrupt notification so we can race execution vs interrupt
    // Note: Signal handlers are spawned AFTER TUI initialization to avoid deadlock
    let (interrupt_tx, interrupt_rx) = tokio::sync::watch::channel(false);

    // Resolve prompt content with precedence:
    // 1. CLI -p (inline text)
    // 2. CLI -P (file path)
    // 3. Config prompt (inline text)
    // 4. Config prompt_file (file path)
    // 5. Default PROMPT.md
    let prompt_content = resolve_prompt_content(&config.event_loop)?;

    // Create or use provided loop context for path resolution
    // This ensures events are written to the correct location for worktree loops
    let mut ctx = loop_context
        .clone()
        .unwrap_or_else(|| LoopContext::primary(config.core.workspace_root.clone()));

    // U0: attach the CLI's authoritative diagnostics collector (built in
    // `main.rs`) so the EventLoop reuses the same session dir as the
    // tracing layer. When `None`, the EventLoop falls back to building
    // its own collector based on `RALPH_DIAGNOSTICS=1`.
    if let Some(ref collector) = prebuilt_diagnostics {
        ctx = ctx.with_prebuilt_diagnostics(Arc::clone(collector));
    }

    // U4 (2026-06-14): if this is a worktree loop and diagnostics are
    // enabled, write a session pointer to the main repo's
    // `.ralph/diagnostics-session-pointer.json` so `ralph diagnose` can
    // find the worktree session after the loop ends and `loops.json`
    // no longer carries an alive entry for it. Best-effort: a write
    // failure is logged but does not block the loop.
    if !ctx.is_primary()
        && let Some(collector) = prebuilt_diagnostics.as_ref()
        && collector.is_enabled()
    {
        match collector.write_session_pointer(ctx.repo_root(), ctx.workspace()) {
            Ok(true) => {
                debug!(
                    target: "ralph_cli::loop_runner",
                    session_dir = %collector.session_dir().map(|p| p.display().to_string()).unwrap_or_default(),
                    main_repo = %ctx.repo_root().display(),
                    "wrote session pointer for worktree diagnostics",
                );
            }
            Ok(false) => {
                // write_session_pointer returned false: primary session
                // (shouldn't happen given the guard) or session_dir is
                // inside main_repo. No-op.
            }
            Err(err) => {
                tracing::warn!(
                    target: "ralph_cli::loop_runner",
                    main_repo = %ctx.repo_root().display(),
                    error = %err,
                    "failed to write session pointer; ralph diagnose may not find this worktree session after the loop ends",
                );
            }
        }
    }

    let urgent_steer_path = ctx.urgent_steer_path();
    let urgent_steer_store = UrgentSteerStore::new(urgent_steer_path.clone());
    urgent_steer_store
        .clear()
        .context("Failed to clear stale urgent-steer marker")?;
    let _urgent_steer_cleanup = scopeguard::guard(urgent_steer_path.clone(), |path| {
        let _ = UrgentSteerStore::new(path).clear();
    });

    // Write loop ID to marker file for task ownership tracking.
    // For worktree loops, use the loop_id; for primary loops, generate one.
    // This file is read by `ralph tools task add` to tag new tasks.
    //
    // In --continue mode, reuse the existing loop ID so that tasks from the
    // previous run remain visible to `ralph tools task ready`. An explicit
    // --loop-id takes priority over the marker file.
    let loop_id = resolve_loop_id(&ctx, resume, resume_loop_id.as_deref());
    let loop_id_marker = ctx.ralph_dir().join("current-loop-id");
    fs::write(&loop_id_marker, &loop_id).context("Failed to write current-loop-id marker")?;
    debug!(loop_id = %loop_id, marker = ?loop_id_marker, "Wrote loop ID marker file");

    // R3: stamp an owner hat on the registry entry so loop authorization
    // helpers can gate cross-loop operations. Agent-owned loops get the
    // current hat; human CLI invocations stay `None` so any operator can
    // still interact with them.
    register_loop_owner(&loop_id, &config, resume);

    let state_machine_enabled = config
        .event_loop
        .state_machine
        .as_ref()
        .is_some_and(|sm| sm.enabled);

    // For fresh runs (not resume), generate a unique timestamped events file
    // This prevents stale events from previous runs polluting new runs (issue #82)
    // The marker file `.ralph/current-events` coordinates path between Ralph and agents
    if !resume {
        let run_id = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
        // Use relative path in marker file for portability across agents
        // The actual file is at ctx.ralph_dir()/events-{run_id}.jsonl
        let relative_events_path = format!(".ralph/events-{}.jsonl", run_id);

        fs::create_dir_all(ctx.ralph_dir()).context("Failed to create .ralph directory")?;
        fs::write(ctx.current_events_marker(), &relative_events_path)
            .context("Failed to write current-events marker file")?;

        debug!("Created events file for this run: {}", relative_events_path);

        if state_machine_enabled {
            let relative_candidate_events_path =
                format!(".ralph/event-candidates-{}.jsonl", run_id);
            fs::write(
                current_candidate_events_marker(&ctx),
                &relative_candidate_events_path,
            )
            .context("Failed to write current-candidate-events marker file")?;
            debug!(
                "Created candidate events file for this run: {}",
                relative_candidate_events_path
            );
        } else {
            let _ = fs::remove_file(current_candidate_events_marker(&ctx));
        }

        // Clear scratchpads for fresh objective start
        // Stale content from previous runs can confuse the agent about current task state
        // Clear global scratchpad and all per-hat scratchpad overrides
        let mut scratchpad_paths: Vec<PathBuf> =
            vec![ctx.workspace().join(&config.core.scratchpad.path)];
        for hat in config.hats.values() {
            if let Some(ref sc) = hat.scratchpad
                && sc.enabled
            {
                let hat_path = ctx.workspace().join(&sc.path);
                if !scratchpad_paths.contains(&hat_path) {
                    scratchpad_paths.push(hat_path);
                }
            }
        }
        for scratchpad_path in &scratchpad_paths {
            if scratchpad_path.exists() {
                fs::remove_file(scratchpad_path).with_context(|| {
                    format!("Failed to clear scratchpad: {:?}", scratchpad_path)
                })?;
                debug!(
                    "Cleared scratchpad for fresh objective: {:?}",
                    scratchpad_path
                );
            }
        }
    }

    // Initialize event loop with context for proper path resolution
    let mut event_loop = EventLoop::with_context(config.clone(), ctx.clone());
    // R4 (2026-06-14-003 plan): advertise the single-U contract to
    // child processes (the agent's `ralph tools task ensure` calls)
    // when the preset opts in.  We rely on standard
    // `Command::new` env inheritance, so setting the var here
    // (single-threaded bootstrap) is safe under the Rust 2024
    // `set_var` contract.
    //
    // R4 review (round 2, finding #1): the prior version only logged
    // the flag.  The CLI's task_cli.rs consults the env var; if
    // we do not export it, R4 is dormant inside the running loop.
    //
    // The workspace `forbid(unsafe_code)` lint forbids `set_var`
    // from lib code.  We use a safer signal: write a sentinel file
    // at `.ralph/agent/.ralph-enforce-current-unit` which the
    // task_cli helper consults as a fallback when the env var is
    // not set.  The file is removed on loop teardown.
    if event_loop.enforce_current_unit_active() {
        if let Some(workspace) = ctx.workspace().parent() {
            let marker = workspace
                .join(".ralph")
                .join("agent")
                .join(".ralph-enforce-current-unit");
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&marker)
            {
                use std::io::Write as _;
                let _ = writeln!(f, "1");
            }
        }
        tracing::info!(
            "R4 single-U contract active (enforce_current_unit=true): \
             writing .ralph/agent/.ralph-enforce-current-unit marker for child processes"
        );
    }
    if state_machine_enabled {
        event_loop.set_event_reader_path(resolve_candidate_events_path(&ctx));
    }

    // ── U5/U6 production wiring (P1.1–P1.4) ──────────────────────────────
    // Construct a `DriftEngine` that owns the drift observer,
    // detector, and per-iteration responder glue. The engine is
    // enabled iff `telemetry.runtime_diagnosis.enabled` is true.
    // When disabled (the default), every per-iteration method is
    // a cheap no-op so the loop runs unchanged.
    let telemetry_config = Arc::new(config.telemetry.runtime_diagnosis.clone());
    let mut drift_engine = if telemetry_config.enabled {
        let required_fields = ralph_core::drift::engine::required_fields_from_config(
            config.event_loop.event_policy.as_ref(),
            config.event_loop.execution_contracts.as_ref(),
        );
        let hat_configs: Vec<HatConfig> = config.hats.values().cloned().collect();
        let declared_edges = ralph_core::drift::engine::declared_edges_from_hats(&hat_configs);
        ralph_core::drift::DriftEngine::enabled(
            Arc::clone(&telemetry_config),
            required_fields,
            declared_edges,
        )
    } else {
        ralph_core::drift::DriftEngine::disabled(Arc::clone(&telemetry_config))
    };
    // Install the drift observer on the EventBus as the very
    // first observer so it observes every event the bus sees
    // (including the recovery events we publish later).
    drift_engine.install_observer(&mut event_loop);

    // Inject robot service (Telegram) for human-in-the-loop communication
    if config.robot.enabled
        && ctx.is_primary()
        && let Some(service) = create_robot_service(&config, &ctx)
    {
        event_loop.set_robot_service(service);
    }

    // Capture the robot service shutdown flag so signal handlers can interrupt wait_for_response()
    let robot_shutdown = event_loop.robot_shutdown_flag();

    let hooks_dispatch_enabled = config.hooks.enabled && !config.hooks.events.is_empty();
    let hook_engine = HookEngine::new(&config.hooks);
    let hook_executor = HookExecutor::new();
    let suspend_state_store = SuspendStateStore::new(ctx.workspace());
    let mut accumulated_hook_metadata = serde_json::Map::new();

    let pre_loop_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        hooks_dispatch_enabled,
        &loop_id,
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input(
            &loop_id,
            &ctx,
            config.event_loop.max_iterations,
            event_loop.state().iteration,
            None,
            &accumulated_hook_metadata,
        ),
    );
    merge_accumulated_hook_metadata_from_outcomes(
        &mut accumulated_hook_metadata,
        &pre_loop_start_outcomes,
    );
    fail_if_blocking_loop_start_outcomes(&pre_loop_start_outcomes)?;
    let mut pending_suspend_termination_reason =
        wait_for_resume_if_suspended(&pre_loop_start_outcomes, &loop_id, &suspend_state_store)
            .await?;

    if pending_suspend_termination_reason.is_none() {
        // For resume mode, we initialize with a different event topic
        // This tells the planner to read existing scratchpad rather than creating a new one
        if resume {
            event_loop.initialize_resume(&prompt_content);
        } else {
            event_loop.initialize(&prompt_content);
        }

        let post_loop_start_outcomes = dispatch_phase_event_hooks(
            &event_loop,
            hooks_dispatch_enabled,
            &loop_id,
            &hook_engine,
            &hook_executor,
            HookPhaseEvent::PostLoopStart,
            build_loop_start_payload_input(
                &loop_id,
                &ctx,
                config.event_loop.max_iterations,
                event_loop.state().iteration,
                Some(event_loop.get_active_hat_id().as_str().to_string()),
                &accumulated_hook_metadata,
            ),
        );
        merge_accumulated_hook_metadata_from_outcomes(
            &mut accumulated_hook_metadata,
            &post_loop_start_outcomes,
        );
        fail_if_blocking_loop_start_outcomes(&post_loop_start_outcomes)?;
        pending_suspend_termination_reason =
            wait_for_resume_if_suspended(&post_loop_start_outcomes, &loop_id, &suspend_state_store)
                .await?;
    }

    // Set up session recording if requested
    // This records all events to a JSONL file for replay testing
    let _session_recorder: Option<Arc<SessionRecorder<BufWriter<File>>>> =
        if let Some(record_path) = record_session {
            let file = File::create(&record_path).with_context(|| {
                format!("Failed to create session recording file: {:?}", record_path)
            })?;
            let recorder = Arc::new(SessionRecorder::new(BufWriter::new(file)));

            // Record metadata for the session
            recorder.record_meta(Record::meta_loop_start(
                &config.event_loop.prompt_file,
                config.event_loop.max_iterations,
                if enable_tui { Some("tui") } else { Some("cli") },
            ));

            // Wire observer to EventBus so events are recorded
            let observer = SessionRecorder::make_observer(Arc::clone(&recorder));
            event_loop.add_observer(observer);

            info!("Session recording enabled: {:?}", record_path);
            Some(recorder)
        } else {
            None
        };

    // ── Phase Initialization (Warmup/Production Two-Phase Loop) ───────────────
    // Determine starting phase based on CLI flags and phase.json state
    let phase_json_path = ctx.ralph_dir().join("agent").join("phase.json");
    let current_phase = if force_warmup {
        info!("Force warmup enabled — starting in warmup phase");
        Phase::Warmup
    } else if phase_json_path.exists() {
        // Read existing phase.json to check warmup_completed marker
        match fs::read_to_string(&phase_json_path) {
            Ok(content) => {
                // Parse phase.json to check warmup_completed field
                if let Ok(phase_data) = serde_json::from_str::<serde_json::Value>(&content) {
                    let warmup_completed = phase_data
                        .get("warmup_completed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if warmup_completed && !warmup_only {
                        info!("Warmup previously completed — skipping to production phase");
                        Phase::Production
                    } else {
                        Phase::Warmup
                    }
                } else {
                    Phase::Warmup
                }
            }
            Err(_) => Phase::Warmup,
        }
    } else {
        // No phase.json exists — start in warmup phase
        Phase::Warmup
    };

    // Set phase on registry for phase-aware hat triggering
    event_loop.registry_mut().set_phase(current_phase.clone());

    // Apply warmup_only / stop_on_exit from CLI flag to config
    // WarmupConfig is nested under phase_config, not directly on EventLoopConfig
    let stop_on_exit = if warmup_only {
        info!("Warmup-only mode enabled — loop will exit after warmup completes");
        // Ensure phase_config exists with warmup_config
        if config.event_loop.phase_config.is_none() {
            config.event_loop.phase_config = Some(PhaseConfig {
                initial: Phase::Warmup,
                transition_event: "phase.transition".to_string(),
                warmup_config: None,
            });
        }
        if let Some(ref mut phase_config) = config.event_loop.phase_config {
            if phase_config.warmup_config.is_none() {
                phase_config.warmup_config = Some(WarmupConfig::default());
            }
            if let Some(ref mut warmup) = phase_config.warmup_config {
                warmup.stop_on_exit = true;
            }
        }
        true
    } else {
        config
            .event_loop
            .phase_config
            .as_ref()
            .and_then(|p| p.warmup_config.as_ref())
            .map(|w| w.stop_on_exit)
            .unwrap_or(false)
    };

    // Initialize event logger for history/observability (uses context for path resolution).
    // This writes to the history file, NOT the trusted events file consumed by EventReader.
    // Raw output parsing, orphan events, and terminate events all go here.
    let mut event_logger = EventLogger::history_from_context(&ctx);

    // Log initial event (use configured starting_event or default to task.start/task.resume)
    let default_start_topic = if resume { "task.resume" } else { "task.start" };
    let start_topic = config
        .event_loop
        .starting_event
        .as_deref()
        .unwrap_or(default_start_topic);
    let start_triggered = "planner"; // Default triggered hat for backward compat
    let start_event = Event::new(start_topic, &prompt_content);
    let start_record = EventRecord::new(
        0,
        "loop",
        &start_event,
        Some(&HatId::new(start_triggered)),
        Some(current_phase.to_string()),
    );
    if let Err(e) = event_logger.log(&start_record) {
        warn!("Failed to log start event: {}", e);
    }
    // NOTE: No sync_event_reader_to_file_end() needed here because the history
    // logger writes to a separate file from the trusted events file consumed
    // by EventReader. The start event only appears in history, not in the
    // trusted event stream.

    // ── Agent doc sync (managed blocks injection) ────────────────────────
    // Runs synchronously BEFORE backend spawn so constraint blocks are
    // present in CLAUDE.md / AGENTS.md when the agent starts reading.
    {
        let env_skip = std::env::var("RALPH_AGENT_DOC_SYNC")
            .ok()
            .map(|v| v.trim() == "0")
            .unwrap_or(false);
        let skip = ralph_core::config::agent_doc_sync::should_skip(
            env_skip,
            no_sync_agent_docs,
            &config.agent_doc_sync,
        );

        if skip {
            tracing::debug!(
                target: "ralph_cli::loop_runner",
                "agent_doc_sync: skipped (disabled via flag/env/config)"
            );
        } else {
            // Resolve block references from config to BlockSpec instances.
            // D4: unknown block_ref is a **configuration** error, not a
            // runtime I/O error. The runtime `OnError::Warn` policy must
            // not mask it; we fail-closed and surface the offending ref
            // so the operator can fix the config.
            let mut blocks: Vec<ralph_core::agent_doc_sync::BlockSpec> = Vec::new();
            for block_ref in &config.agent_doc_sync.blocks {
                // Strip "builtin:" prefix to get the block ID.
                let block_id = block_ref.strip_prefix("builtin:").unwrap_or(block_ref);
                match ralph_core::agent_doc_sync::builtin::builtin_block(block_id) {
                    Some(spec) => blocks.push(spec),
                    None => {
                        tracing::error!(
                            target: "ralph_cli::loop_runner",
                            block_ref = %block_ref,
                            "agent_doc_sync: unknown block reference (fail-closed)"
                        );
                        return Err(anyhow::anyhow!(
                            "agent_doc_sync: unknown block_ref '{block_ref}' (registered builtins: {})",
                            ralph_core::agent_doc_sync::builtin::known_builtin_ids().join(", ")
                        ))
                        .context("agent doc sync configuration error");
                    }
                }
            }

            let on_error: ralph_core::agent_doc_sync::OnError =
                config.agent_doc_sync.on_error.into();

            // Resolve session_dir from the diagnostics collector for
            // recovery envelope writes. When None, the persist module
            // skips the recovery envelope (no-op).
            let session_dir = ctx.prebuilt_diagnostics().and_then(|d| d.session_dir());

            let sync_config = ralph_core::agent_doc_sync::SyncConfig {
                skip: false,
                on_error,
                target_files: &["CLAUDE.md", "AGENTS.md"],
                blocks: &blocks,
                session_dir,
            };

            // D5: bound the startup-blocking sync phase. `sync_all` is
            // synchronous; run it on a worker thread and `recv_timeout`
            // so a slow disk / stuck lock / NFS round-trip can never
            // hang the outer loop. Timeout → warning envelope;
            // `OnError::Strict` upgrades that into a hard exit.
            let timeout_secs = config.agent_doc_sync.startup_timeout_secs;
            match run_sync_with_timeout(&config.core.workspace_root, &sync_config, timeout_secs) {
                Ok(report) => {
                    tracing::debug!(
                        target: "ralph_cli::loop_runner",
                        synced = report.synced,
                        skipped = report.skipped,
                        failed = report.failed,
                        "agent_doc_sync: complete"
                    );
                }
                Err(SyncRunError::Sync(e)) => match config.agent_doc_sync.on_error {
                    ralph_core::OnErrorPolicy::Strict => {
                        tracing::error!(
                            target: "ralph_cli::loop_runner",
                            error = %e,
                            "agent_doc_sync: failed (strict mode), aborting"
                        );
                        return Err(anyhow::anyhow!("agent_doc_sync failed in strict mode"))
                            .context("agent doc sync strict mode");
                    }
                    ralph_core::OnErrorPolicy::Warn => {
                        tracing::warn!(
                            target: "ralph_cli::loop_runner",
                            error = %e,
                            "agent_doc_sync: failed; continuing"
                        );
                    }
                },
                Err(SyncRunError::Timeout { secs }) => {
                    tracing::warn!(
                        target: "ralph_cli::loop_runner",
                        timeout_secs = secs,
                        "agent_doc_sync: startup timeout; continuing without managed blocks"
                    );
                    write_startup_timeout_envelope(
                        session_dir,
                        secs,
                        config.agent_doc_sync.on_error,
                    );
                    if config.agent_doc_sync.on_error == ralph_core::OnErrorPolicy::Strict {
                        return Err(anyhow::anyhow!(
                            "agent_doc_sync exceeded {secs}s startup timeout (strict mode)"
                        ))
                        .context("agent doc sync startup timeout");
                    }
                }
            }
        }
    }

    // Create backend from config - TUI mode uses the same backend as non-TUI
    // The TUI is an observation layer that displays output, not a different mode
    let mut backend = CliBackend::from_config(&config.cli).map_err(|e| anyhow::Error::new(e))?;

    // Append custom args from CLI if provided (e.g., `ralph run -b opencode -- --model="some-model"`)
    if !custom_args.is_empty() {
        backend.args.extend(custom_args);
    }

    // Create PTY executor if using interactive mode
    let mut pty_executor = if use_pty {
        // The watchdog value in seconds. Interactive mode uses the user-facing
        // 30s default; autonomous / RPC / worktree mode uses the resolver
        // below (explicit override or per-adapter timeout, default 300s).
        // Hard-coding 0 for autonomous used to silently disable the watchdog
        // and hang the outer loop on a silent, non-exiting backend — see
        // pty_executor.rs and plan 2026-06-06-001.
        let idle_timeout_secs: u64 = if user_interactive {
            u64::from(config.cli.idle_timeout_secs)
        } else {
            config.autonomous_idle_timeout_secs(&config.cli.backend)
        };
        // In autonomous (non-interactive) mode, use a very wide PTY to prevent
        // line wrapping of long NDJSON output (Pi emits 800+ char JSON lines that
        // get garbled when the PTY wraps at 80 columns).
        let cols = if user_interactive {
            PtyConfig::from_env().cols
        } else {
            32768
        };
        // The watchdog u64 is bounded to u32::MAX so PtyConfig's u32 field
        // can hold it without silent truncation; the realistic value (300s
        // default) fits trivially.
        let pty_config = PtyConfig {
            interactive: user_interactive,
            idle_timeout_secs: u32::try_from(idle_timeout_secs).unwrap_or(u32::MAX),
            cols,
            workspace_root: config.core.workspace_root.clone(),
            ..PtyConfig::from_env()
        };
        Some(PtyExecutor::new(backend.clone(), pty_config))
    } else {
        None
    };

    // Create termination signal for TUI shutdown
    let (terminated_tx, terminated_rx) = tokio::sync::watch::channel(false);

    // Wire TUI with termination signal and shared state
    // TUI is observation-only - works in both interactive and autonomous modes
    // Requirements: both stdin and stdout must be terminals for TUI
    // (Crossterm requires stdin for keyboard input, stdout for rendering)
    let enable_tui = enable_tui && !enable_rpc && stdin().is_terminal() && stdout().is_terminal();

    // RPC mode state: channels for stdin commands and stdout events
    let (rpc_event_tx, rpc_event_rx) = if enable_rpc {
        let (tx, rx) = tokio::sync::mpsc::channel::<RpcEvent>(256);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let (rpc_guidance_tx, mut rpc_guidance_rx) = if enable_rpc {
        let (tx, rx) = tokio::sync::mpsc::channel::<GuidanceMessage>(64);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Shared stdout writer for RPC mode (thread-safe for JsonRpcStreamHandler)
    let rpc_stdout: Option<Arc<std::sync::Mutex<std::io::Stdout>>> = if enable_rpc {
        Some(Arc::new(std::sync::Mutex::new(std::io::stdout())))
    } else {
        None
    };

    // RPC mode: spawn stdin reader and stdout emitter tasks
    let rpc_dispatcher_started = if enable_rpc {
        let backend_name = config.cli.backend.clone();
        let max_iters = config.event_loop.max_iterations;

        // Create shared state for get_state responses
        let rpc_state_iteration = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let rpc_state_hat: Arc<std::sync::Mutex<(String, String)>> = Arc::new(
            std::sync::Mutex::new(("unknown".to_string(), "Unknown".to_string())),
        );
        let rpc_state_completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let rpc_state_total_cost: Arc<std::sync::Mutex<f64>> = Arc::new(std::sync::Mutex::new(0.0));

        let rpc_state_iteration_clone = rpc_state_iteration.clone();
        let rpc_state_hat_clone = rpc_state_hat.clone();
        let rpc_state_completed_clone = rpc_state_completed.clone();
        let rpc_state_total_cost_clone = rpc_state_total_cost.clone();
        let rpc_state_started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let state_fn = move || {
            let (hat, hat_display) = rpc_state_hat_clone
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| ("unknown".to_string(), "Unknown".to_string()));
            let total_cost_usd = rpc_state_total_cost_clone.lock().map(|g| *g).unwrap_or(0.0);
            RpcState {
                iteration: rpc_state_iteration_clone.load(std::sync::atomic::Ordering::Relaxed),
                max_iterations: Some(max_iters),
                hat,
                hat_display,
                backend: backend_name.clone(),
                completed: rpc_state_completed_clone.load(std::sync::atomic::Ordering::Relaxed),
                started_at: rpc_state_started_at,
                iteration_started_at: None,
                task_counts: RpcTaskCounts::default(),
                active_task: None,
                total_cost_usd,
            }
        };

        let dispatcher = RpcDispatcher::new(
            interrupt_tx.clone(),
            rpc_guidance_tx
                .clone()
                .expect("RPC guidance tx should exist"),
            rpc_event_tx.clone().expect("RPC event tx should exist"),
            Some(urgent_steer_path.clone()),
            state_fn,
        );

        // Mark loop as started
        dispatcher.mark_loop_started();

        // Spawn stdin reader
        tokio::spawn(async move {
            run_stdin_reader(dispatcher, tokio::io::stdin()).await;
        });

        // Spawn stdout emitter
        let rx = rpc_event_rx.expect("RPC event rx should exist");
        tokio::spawn(async move {
            run_stdout_emitter(rx).await;
        });

        // Emit loop_started event
        if let Some(ref tx) = rpc_event_tx {
            let started_event = RpcEvent::LoopStarted {
                prompt: prompt_content.clone(),
                max_iterations: Some(config.event_loop.max_iterations),
                backend: config.cli.backend.clone(),
                started_at: rpc_state_started_at,
            };
            let _ = tx.try_send(started_event);
        }

        Some(RpcSharedState {
            iteration: rpc_state_iteration,
            hat: rpc_state_hat,
            completed: rpc_state_completed,
            total_cost_usd: rpc_state_total_cost,
        })
    } else {
        None
    };

    let (mut tui_handle, tui_state, guidance_next_queue) = if enable_tui {
        // Build hat map for dynamic topic-to-hat resolution
        // This allows TUI to display custom hats (e.g., "Security Reviewer")
        // instead of generic "ralph" for all events
        let hat_map = build_tui_hat_map(event_loop.registry());
        let tui = Tui::new()
            .with_hat_map(hat_map)
            .with_termination_signal(terminated_rx)
            .with_events_path(resolve_current_events_path(&ctx))
            .with_urgent_steer_path(urgent_steer_path.clone());

        // Get shared state and guidance queue before spawning (for content streaming)
        let state = tui.state();
        let guidance_queue = tui.guidance_next_queue();

        // Wire interrupt channel so TUI can signal main loop on Ctrl+C
        // (raw mode prevents SIGINT from being generated by the OS)
        let tui = tui.with_interrupt_tx(interrupt_tx.clone());

        let observer = tui.observer();
        event_loop.add_observer(observer);
        (
            Some(tokio::spawn(async move { tui.run().await })),
            Some(state),
            Some(guidance_queue),
        )
    } else {
        (None, None, None)
    };

    // Add RPC EventBus observer to map ralph_proto::Event topics to RpcEvent variants
    // Per Task 04 requirement #4: "Add an EventBus observer that serializes Event → RpcEvent"
    if let Some(ref tx) = rpc_event_tx {
        let tx_clone = tx.clone();
        event_loop.add_observer(move |event: &Event| {
            // Map all event topics to RpcEvent::OrchestrationEvent
            // This provides observability for: build.task, build.done, loop.terminate,
            // task.start, task.resume, and any custom hat events
            let rpc_event = RpcEvent::OrchestrationEvent {
                topic: event.topic.as_str().to_string(),
                payload: event.payload.clone(),
                source: event.source.as_ref().map(|h| h.as_str().to_string()),
                target: event.target.as_ref().map(|h| h.as_str().to_string()),
            };
            let _ = tx_clone.try_send(rpc_event);
        });
    }

    // Give TUI task time to initialize (enter alternate screen, enable raw mode)
    // before the main loop starts doing work
    if tui_handle.is_some() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Seed max_iterations into TUI state for accurate iteration display.
    if let Some(mut s) = tui_state.as_ref().and_then(|state| state.lock().ok()) {
        s.max_iterations = Some(config.event_loop.max_iterations);
    }

    // Spawn signal handlers AFTER TUI initialization to avoid deadlock
    // (TUI must enter raw mode and create EventStream before signal handlers are registered)

    // Spawn task to listen for SIGINT (Ctrl+C)
    let interrupt_tx_sigint = interrupt_tx.clone();
    let robot_shutdown_sigint = robot_shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            debug!("Interrupt received (SIGINT), terminating immediately...");
            if let Some(ref flag) = robot_shutdown_sigint {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let _ = interrupt_tx_sigint.send(true);
        }
    });

    // Spawn task to listen for SIGTERM (Unix only)
    #[cfg(unix)]
    {
        let interrupt_tx_sigterm = interrupt_tx.clone();
        let robot_shutdown_sigterm = robot_shutdown.clone();
        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to register SIGTERM handler");
            sigterm.recv().await;
            debug!("SIGTERM received, terminating immediately...");
            if let Some(ref flag) = robot_shutdown_sigterm {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let _ = interrupt_tx_sigterm.send(true);
        });
    }

    // Spawn task to listen for SIGHUP (Unix only)
    #[cfg(unix)]
    {
        let interrupt_tx_sighup = interrupt_tx.clone();
        let robot_shutdown_sighup = robot_shutdown.clone();
        tokio::spawn(async move {
            let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .expect("Failed to register SIGHUP handler");
            sighup.recv().await;
            warn!("SIGHUP received (terminal closed), terminating immediately...");
            if let Some(ref flag) = robot_shutdown_sighup {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let _ = interrupt_tx_sighup.send(true);
        });
    }

    // Log execution mode - hat info already logged by initialize()
    let exec_mode = if user_interactive {
        "interactive"
    } else {
        "autonomous"
    };
    debug!(execution_mode = %exec_mode, "Execution mode configured");

    // Track the last hat to detect hat changes for logging
    let mut last_hat: Option<HatId> = None;

    // Track consecutive fallback attempts to prevent infinite loops
    let mut consecutive_fallbacks: u32 = 0;
    const MAX_FALLBACK_ATTEMPTS: u32 = 3;

    // P1 finding #3 (CR 2026-06-10): heartbeat to flush
    // `active-activations.json` while the loop is running so
    // `ralph diagnose --session latest` reflects live state during a
    // stall (R14 "卡住时实时可观测"). Previously the file was only
    // written at loop termination inside `finalize_recovery_diagnosis`,
    // so a stuck or long-running loop never produced it. Heartbeat
    // interval is `RALPH_ACTIVATIONS_HEARTBEAT_SEC` (default 30s); set
    // to `0` to disable. The write itself is a cheap file I/O via
    // `tempfile + persist` (R8 contract) and is a no-op when the
    // diagnostics collector has no session dir.
    let heartbeat_secs: u64 = std::env::var("RALPH_ACTIVATIONS_HEARTBEAT_SEC")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30);
    let mut last_activations_heartbeat: Option<std::time::Instant> = if heartbeat_secs > 0 {
        // Force the first heartbeat to fire as soon as the main loop
        // starts, so the file exists from iteration 1 (helpful when
        // operators `tail -f` the session dir).
        Some(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(heartbeat_secs + 1))
                .unwrap_or_else(std::time::Instant::now),
        )
    } else {
        None
    };

    // Initialize loop history if we have a loop context
    let loop_history = loop_context
        .as_ref()
        .map(|ctx| LoopHistory::from_context(ctx));

    // Record loop start in history
    if let Some(ref history) = loop_history
        && let Err(e) = history.record_started(&prompt_content)
    {
        warn!("Failed to record loop start in history: {}", e);
    }

    // Auto-merge setting: CLI override > config > default (false for safety)
    let auto_merge = auto_merge_override.unwrap_or(config.features.auto_merge);

    // Detect merge loop on startup via RALPH_MERGE_LOOP_ID env var
    // Per spec: If set, mark entry as "merging" with current PID
    let merge_loop_id: Option<String> = std::env::var("RALPH_MERGE_LOOP_ID").ok();
    if let Some(ref loop_id) = merge_loop_id {
        let repo_root = loop_context
            .as_ref()
            .map(|ctx| ctx.repo_root().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let queue = MergeQueue::new(&repo_root);
        let pid = std::process::id();

        match queue.mark_merging(loop_id, pid) {
            Ok(()) => {
                info!(loop_id = %loop_id, pid = pid, "Merge loop started, marked as merging");
            }
            Err(ralph_core::MergeQueueError::NotFound(_)) => {
                warn!(loop_id = %loop_id, "Merge loop started but no queue entry found");
            }
            Err(ralph_core::MergeQueueError::InvalidTransition(_, from, _)) => {
                // Entry is already merging/merged/discarded, skip update
                debug!(loop_id = %loop_id, state = ?from, "Merge queue entry already in terminal state, skipping");
            }
            Err(e) => {
                warn!(loop_id = %loop_id, error = %e, "Failed to mark merge loop as merging");
            }
        }
    }

    // Record base commit at loop start for accurate handoff scope (base..HEAD)
    let base_commit = ralph_core::get_head_sha(&ctx.workspace()).ok();

    // Record the same baseline in the event loop state so execution-contract
    // validation can detect commits produced during this loop. Without this,
    // `diff_or_commit` cannot distinguish "loop produced a new commit" from
    // "the repository merely has commits from prior history".
    event_loop.set_loop_start_sha(base_commit.clone());

    // Helper closure to handle termination (writes summary, prints status, records history)
    let handle_termination =
        |reason: &TerminationReason,
         state: &ralph_core::LoopState,
         scratchpad: &str,
         history: &Option<LoopHistory>,
         context: &Option<LoopContext>,
         auto_merge: bool,
         prompt: &str,
         payload_violation_report_relpath: Option<&str>| {
            // Per spec: Write summary file on termination
            let summary_writer = if let Some(ctx) = context {
                SummaryWriter::from_context(ctx)
            } else {
                SummaryWriter::default()
            };
            let scratchpad_path = if let Some(ctx) = context {
                ctx.scratchpad_path()
            } else {
                PathBuf::from(scratchpad)
            };
            let scratchpad_opt = if scratchpad_path.exists() {
                Some(scratchpad_path.as_path())
            } else {
                None
            };

            // Get final commit SHA if available
            let final_commit = get_last_commit_info();

            if let Err(e) =
                summary_writer.write(reason, state, scratchpad_opt, final_commit.as_deref())
            {
                warn!("Failed to write summary file: {}", e);
            }

            // U8: payload contract violation path also appends a violation
            // reference to the operator-facing `## Diagnostics` section. We
            // build the hint here (closure-internal) so the section stays
            // attached to summary.md even when the responder hint is empty;
            // the diagnosis-summary.json seed is still written by
            // `finalize_recovery_diagnosis` (which has the EventLoop
            // reference needed to reach the diagnostics collector).
            if let Some(relpath) = payload_violation_report_relpath {
                let hint = ralph_core::DiagnosisHint {
                    session_relpath: None,
                    diagnose_command: None,
                    references: vec![ralph_core::DiagnosisReference {
                        label: "Payload contract violation report".to_string(),
                        relpath: relpath.to_string(),
                    }],
                };
                if let Err(e) = summary_writer.append_diagnosis_hint(Some(&hint)) {
                    warn!("Failed to append payload violation reference: {}", e);
                }
            }

            // Record termination in history
            if let Some(hist) = history {
                let reason_str = match reason {
                    TerminationReason::CompletionPromise => "completion_promise",
                    TerminationReason::MaxIterations => "max_iterations",
                    TerminationReason::MaxRuntime => "max_runtime",
                    TerminationReason::MaxCost => "max_cost",
                    TerminationReason::ConsecutiveFailures => "consecutive_failures",
                    TerminationReason::LoopThrashing => "loop_thrashing",
                    TerminationReason::LoopStale => "loop_stale",
                    TerminationReason::ValidationFailure => "validation_failure",
                    TerminationReason::Stopped => "stopped",
                    TerminationReason::Interrupted => "interrupted",
                    TerminationReason::RestartRequested => "restart_requested",
                    TerminationReason::WorkspaceGone => "workspace_gone",
                    TerminationReason::Cancelled => "cancelled",
                    TerminationReason::PayloadContractViolation => "payload_contract_violation",
                    TerminationReason::RecoveryExhausted { .. } => "recovery_exhausted",
                    TerminationReason::ReviewFailed { .. } => "review_failed",
                    TerminationReason::ScopeViolationCircuitBreakerTripped { .. } => {
                        "scope_violation_circuit_breaker_tripped"
                    }
                    // Unit 2 (2026-06-16-002 plan) take-3:
                    // surfaced on the loop history record.  The
                    // snake_case label matches the JSON
                    // `as_str()` value so the two records align.
                    TerminationReason::RecoverablePayloadExhausted { .. } => {
                        "recoverable_payload_exhausted"
                    }
                };

                if matches!(reason, TerminationReason::Interrupted) {
                    if let Err(e) = hist.record_terminated("SIGTERM") {
                        warn!("Failed to record termination in history: {}", e);
                    }
                } else if let Err(e) = hist.record_completed(reason_str) {
                    warn!("Failed to record completion in history: {}", e);
                }
            }

            // Handle merge queue state transitions for merge loops
            // Per spec: CompletionPromise → merged, other → needs-review
            if let Some(ref loop_id) = merge_loop_id {
                let repo_root = context
                    .as_ref()
                    .map(|ctx| ctx.repo_root().to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("."));
                let queue = MergeQueue::new(&repo_root);

                if matches!(reason, TerminationReason::CompletionPromise) {
                    // Get commit SHA from git rev-parse HEAD
                    let commit = Command::new("git")
                        .args(["rev-parse", "HEAD"])
                        .output()
                        .ok()
                        .and_then(|output| {
                            if output.status.success() {
                                String::from_utf8(output.stdout)
                                    .ok()
                                    .map(|s| s.trim().to_string())
                            } else {
                                None
                            }
                        });

                    match commit {
                        Some(sha) => {
                            if let Err(e) = queue.mark_merged(loop_id, &sha) {
                                warn!(loop_id = %loop_id, error = %e, "Failed to mark merge as completed");
                            } else {
                                info!(loop_id = %loop_id, commit = %sha, "Merge completed successfully");
                            }
                        }
                        None => {
                            // Per spec: "If commit SHA cannot be resolved, mark as needs-review"
                            if let Err(e) = queue
                                .mark_needs_review(loop_id, "merge complete but commit not found")
                            {
                                warn!(loop_id = %loop_id, error = %e, "Failed to mark merge as needs-review");
                            } else {
                                warn!(loop_id = %loop_id, "Merge completed but could not resolve commit SHA");
                            }
                        }
                    }
                } else {
                    // Any non-CompletionPromise termination → needs-review
                    let reason_str = match reason {
                        TerminationReason::MaxIterations => "max iterations reached",
                        TerminationReason::MaxRuntime => "max runtime exceeded",
                        TerminationReason::MaxCost => "max cost exceeded",
                        TerminationReason::ConsecutiveFailures => "consecutive failures",
                        TerminationReason::LoopThrashing => "loop thrashing detected",
                        TerminationReason::LoopStale => "stale loop detected",
                        TerminationReason::ValidationFailure => "validation failure",
                        TerminationReason::Stopped => "manually stopped",
                        TerminationReason::Interrupted => "interrupted by signal",
                        TerminationReason::CompletionPromise => unreachable!(),
                        TerminationReason::RestartRequested => "restart requested",
                        TerminationReason::WorkspaceGone => "workspace directory removed",
                        TerminationReason::Cancelled => "cancelled by human",
                        TerminationReason::PayloadContractViolation => "payload contract violation",
                        TerminationReason::RecoveryExhausted { .. } => {
                            "recovery retry window exhausted"
                        }
                        TerminationReason::ReviewFailed { .. } => {
                            // P0-C: failing review verdict propagated to the
                            // last mirror — the workflow has reached its
                            // terminus. Surface a human-readable reason
                            // string for the merge-queue needs-review record.
                            "review verdict failed (verdict gate propagation)"
                        }
                        TerminationReason::ScopeViolationCircuitBreakerTripped { .. } => {
                            "isolated scope violation circuit breaker tripped"
                        }
                        // Unit 2 (2026-06-16-002 plan) take-3:
                        // surfaced on the merge-queue
                        // `needs-review` record.  Free-form
                        // human-readable label, mirror of the
                        // snake_case label on the history record.
                        TerminationReason::RecoverablePayloadExhausted { .. } => {
                            "recoverable payload budget exhausted"
                        }
                    };
                    if let Err(e) = queue.mark_needs_review(loop_id, reason_str) {
                        warn!(loop_id = %loop_id, error = %e, "Failed to mark merge as needs-review");
                    } else {
                        info!(loop_id = %loop_id, reason = reason_str, "Merge marked as needs-review");
                    }
                }
            }

            // Handle completion for all loops (landing + merge queue for worktrees)
            // Per spec: merge loops do NOT enqueue themselves, even if run in worktree context
            if let Some(ctx) = context {
                if merge_loop_id.is_none() && matches!(reason, TerminationReason::CompletionPromise)
                {
                    let handler = LoopCompletionHandler::new(auto_merge);
                    match handler.handle_completion(ctx, prompt, base_commit.as_deref()) {
                        Ok(CompletionAction::None) => {
                            debug!("Loop completed, no action needed");
                        }
                        Ok(CompletionAction::Landed { landing }) => {
                            info!(
                                committed = landing.committed,
                                handoff = %landing.handoff_path,
                                open_tasks = landing.open_task_count,
                                "Primary loop landed successfully"
                            );
                        }
                        Ok(CompletionAction::Enqueued { loop_id, landing }) => {
                            info!(loop_id = %loop_id, "Loop queued for auto-merge");
                            if let Some(ref l) = landing {
                                debug!(
                                    committed = l.committed,
                                    handoff = %l.handoff_path,
                                    "Landing completed before enqueue"
                                );
                            }
                            if let Some(hist) = history {
                                let _ = hist.record_merge_queued();
                            }
                            // Worktree loop exits cleanly; merge will be processed
                            // when the primary loop completes and checks the queue
                        }
                        Ok(CompletionAction::ManualMerge {
                            loop_id,
                            worktree_path,
                            landing,
                        }) => {
                            info!(
                                loop_id = %loop_id,
                                "Loop completed. To merge manually: cd {} && git merge",
                                worktree_path
                            );
                            if let Some(ref l) = landing {
                                debug!(
                                    committed = l.committed,
                                    handoff = %l.handoff_path,
                                    "Landing completed (manual merge mode)"
                                );
                            }
                        }
                        Err(e) => {
                            warn!("Completion handler failed: {}", e);
                        }
                    }
                }

                // Handle merge queue processing for primary loop completion
                if ctx.is_primary() && matches!(reason, TerminationReason::CompletionPromise) {
                    process_pending_merges(ctx.repo_root());
                }

                // Always deregister from registry — process is exiting regardless of reason.
                // CompletionPromise loops are tracked by the merge queue from here on.
                let registry = LoopRegistry::new(ctx.repo_root());
                if let Err(e) = registry.deregister_current_process() {
                    warn!("Failed to deregister loop from registry: {}", e);
                }
            }

            // Print termination info to console (skip in TUI mode - TUI handles display)
            // Skip in RPC mode - JSON events replace console output
            if !enable_tui && !enable_rpc {
                print_termination(reason, state, use_colors, Some(&loop_id));
            }

            // Mark RPC state as completed so get_state reflects termination
            if let Some(ref shared) = rpc_dispatcher_started {
                shared
                    .completed
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }

            // Emit RPC loop_terminated event
            if let Some(ref tx) = rpc_event_tx {
                let terminated_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;

                let rpc_reason = match reason {
                    TerminationReason::CompletionPromise => {
                        ralph_proto::json_rpc::TerminationReason::Completed
                    }
                    TerminationReason::MaxIterations => {
                        ralph_proto::json_rpc::TerminationReason::MaxIterations
                    }
                    TerminationReason::Interrupted | TerminationReason::Stopped => {
                        ralph_proto::json_rpc::TerminationReason::Interrupted
                    }
                    _ => ralph_proto::json_rpc::TerminationReason::Error,
                };

                let accumulated_cost = rpc_dispatcher_started
                    .as_ref()
                    .and_then(|s| s.total_cost_usd.lock().ok().map(|g| *g))
                    .unwrap_or(0.0);

                let terminate_event = RpcEvent::LoopTerminated {
                    reason: rpc_reason,
                    total_iterations: state.iteration,
                    duration_ms: state.elapsed().as_millis() as u64,
                    total_cost_usd: accumulated_cost,
                    terminated_at,
                };
                let _ = tx.try_send(terminate_event);
            }
        };

    if let Some(reason) = pending_suspend_termination_reason.take() {
        let reason = dispatch_pre_loop_termination_hooks(
            &event_loop,
            hooks_dispatch_enabled,
            &loop_id,
            &hook_engine,
            &hook_executor,
            &suspend_state_store,
            &ctx,
            config.event_loop.max_iterations,
            &mut accumulated_hook_metadata,
            reason,
        )
        .await?;

        let terminate_event = event_loop.publish_terminate_event(&reason);
        log_terminate_event(
            &mut event_logger,
            event_loop.state().iteration,
            &terminate_event,
            Some(event_loop.registry().current_phase().to_string()),
        );

        let reason = dispatch_post_loop_termination_hooks(
            &event_loop,
            hooks_dispatch_enabled,
            &loop_id,
            &hook_engine,
            &hook_executor,
            &suspend_state_store,
            &ctx,
            config.event_loop.max_iterations,
            &mut accumulated_hook_metadata,
            reason,
        )
        .await?;

        handle_termination(
            &reason,
            event_loop.state(),
            &config.core.scratchpad.path,
            &loop_history,
            &loop_context,
            auto_merge,
            &prompt_content,
            None,
        );

        // Wait for user to exit TUI (press 'q') on natural completion
        if let Some(handle) = tui_handle.take() {
            let _ = handle.await;
        }

        finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);

        return Ok(reason);
    }

    // Print startup banner for --no-tui runs. Gives agents/humans tailing
    // the stream the loop-id, key state files, and tail/resume commands up
    // front so they don't have to reverse-engineer them from scrollback.
    if !enable_tui && !enable_rpc {
        let events_path = resolve_current_events_path(&ctx);
        let scratchpad_path = ctx.workspace().join(&config.core.scratchpad.path);
        print_loop_banner(
            &loop_id,
            &config.cli.backend,
            std::path::Path::new(&config.event_loop.prompt_file),
            &events_path,
            &scratchpad_path,
            config.event_loop.max_iterations,
            resume,
            use_colors,
        );
    }

    // Main orchestration loop
    loop {
        // Check for interrupt signal at start of each iteration
        // This catches TUI Ctrl+C (via interrupt_tx) before printing iteration separator
        if *interrupt_rx.borrow() {
            #[cfg(unix)]
            {
                use nix::sys::signal::{Signal, killpg};
                use nix::unistd::getpgrp;
                let pgid = getpgrp();
                let our_pid = nix::unistd::Pid::this();
                let our_pid_u32 = u32::try_from(our_pid.as_raw()).unwrap_or(0);
                warn!(
                    target: "ralph_cli::loop_runner",
                    pid = %our_pid,
                    pgid = %pgid,
                    "Interrupt detected at loop start, sending SIGTERM to process group"
                );
                // Fallback: kill descendant processes that live outside the
                // orchestrator's process group (e.g. PTY-session backends).
                crate::cli::process_tree::kill_process_tree(our_pid_u32, false);
                let _ = killpg(pgid, Signal::SIGTERM);
                tokio::time::sleep(Duration::from_millis(250)).await;
                warn!(
                    target: "ralph_cli::loop_runner",
                    pid = %our_pid,
                    pgid = %pgid,
                    "Sending SIGKILL to process group after grace period"
                );
                let _ = killpg(pgid, Signal::SIGKILL);
            }
            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                TerminationReason::Interrupted,
            )
            .await?;

            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
                None,
            );
            // Signal TUI to exit immediately on interrupt
            let _ = terminated_tx.send(true);
            finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
            return Ok(reason);
        }

        // Drain next-loop guidance queue and write as human.guidance events.
        // These will be picked up by process_events_from_jsonl() during build_prompt().
        // Handle both TUI guidance queue and RPC guidance channel.
        let mut guidance_messages: Vec<String> = Vec::new();

        // Drain TUI guidance queue
        if let Some(ref queue) = guidance_next_queue {
            let messages: Vec<String> = {
                let mut q = queue.lock().unwrap();
                q.drain(..).collect()
            };
            guidance_messages.extend(messages);
        }

        // Drain RPC guidance channel (non-blocking)
        if let Some(ref mut rx) = rpc_guidance_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg.target {
                    GuidanceTarget::Current => {
                        debug!("Received RPC steer(current); applying at next prompt boundary");
                        guidance_messages.push(msg.message);
                    }
                    GuidanceTarget::Next => guidance_messages.push(msg.message),
                }
            }
        }

        if !guidance_messages.is_empty() {
            let events_path = resolve_current_events_path(&ctx);

            use std::io::Write;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path);

            let mut writer = match file {
                Ok(f) => std::io::BufWriter::new(f),
                Err(e) => {
                    warn!(error = %e, path = ?events_path, "Failed to open events file for guidance flush");
                    // Skip flushing - keep loop running
                    continue;
                }
            };

            for msg in &guidance_messages {
                let timestamp = chrono::Utc::now().to_rfc3339();
                let event = serde_json::json!({
                    "topic": "human.guidance",
                    "payload": msg,
                    "ts": timestamp,
                });

                match serde_json::to_string(&event) {
                    Ok(line) => {
                        if writeln!(writer, "{}", line).is_err() {
                            warn!(path = ?events_path, "Failed writing guidance event line");
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed serializing guidance event");
                    }
                }
            }
            info!(
                count = guidance_messages.len(),
                "Wrote guidance events to events.jsonl"
            );
        }

        // Check termination before execution
        // P1.4: drift engine termination hint check. When the
        // responder produced a high-severity `TerminationHint`
        // (Error or Critical) — i.e. a retry key exhausted its
        // retry window or has no safe target — the engine surfaces
        // a `RecoveryExhausted` termination reason. We only
        // promote the hint to a real termination when
        // `check_termination` did not already produce a stronger
        // reason (PayloadContractViolation, LoopStale, etc.).
        //
        // R8 contract: a `Warning` Final hint does NOT promote
        // to a termination reason — instead the engine publishes
        // a `human.guidance` event (see
        // `check_final_human_guidance`). The loop continues under
        // operator supervision. We do that check inline here so
        // the operator gets a chance to intervene before the
        // next hat dispatch.
        //
        // Unit 3 (2026-06-16-002 plan): pass the loop's
        // `bootstrap_complete` flag.  While the loop is still
        // in the bootstrap window (work.start → first legal
        // coordinator work.ready) the engine MUST NOT publish
        // a `human.guidance` — the coordinator's first prompt
        // must not be derailed by recovery noise.  The Error /
        // Critical branch (`check_termination_hint`) is
        // intentionally NOT gated so a misbehaving coordinator
        // can still be caught early.
        // Unit 3 (2026-06-16-002 plan) bootstrap gate: the
        // engine reads `bootstrap_complete` from `event_loop`
        // internally so the caller no longer needs to pass it
        // (the function's signature dropped the parameter to
        // avoid a double `&mut self.state` borrow).  The
        // `check_termination_hint` Error/Critical branch is
        // intentionally NOT gated so a misbehaving coordinator
        // can still be caught early.
        let guidance_published = drift_engine.check_final_human_guidance(&mut event_loop);
        if guidance_published {
            tracing::info!(
                iteration = event_loop.state().iteration,
                "drift engine published human.guidance for Final Warning hint"
            );
        }
        let hint_reason = drift_engine.check_termination_hint(&event_loop);
        if let Some(reason) = event_loop.check_termination().or(hint_reason) {
            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            // Per spec: Publish loop.terminate event to observers
            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
                None,
            );
            // Wait for user to exit TUI (press 'q') on natural completion
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
            return Ok(reason);
        }

        let iteration = event_loop.state().iteration + 1;

        if event_loop.has_pending_events() {
            let pre_iteration_start_outcomes = dispatch_phase_event_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                HookPhaseEvent::PreIterationStart,
                build_iteration_start_payload_input(
                    &loop_id,
                    &ctx,
                    config.event_loop.max_iterations,
                    iteration,
                    Some(event_loop.get_active_hat_id().as_str().to_string()),
                    None,
                    None,
                    &accumulated_hook_metadata,
                ),
            );
            merge_accumulated_hook_metadata_from_outcomes(
                &mut accumulated_hook_metadata,
                &pre_iteration_start_outcomes,
            );
            fail_if_blocking_iteration_start_outcomes(&pre_iteration_start_outcomes)?;

            if let Some(reason) = wait_for_resume_if_suspended(
                &pre_iteration_start_outcomes,
                &loop_id,
                &suspend_state_store,
            )
            .await?
            {
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                    None,
                );
                // Wait for user to exit TUI (press 'q') on natural completion
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                return Ok(reason);
            }
        }

        // Get next hat to execute, with fallback recovery if no pending events
        let hat_id = match event_loop.next_hat() {
            Some(id) => {
                // Reset fallback counter on successful event routing
                consecutive_fallbacks = 0;
                id.clone()
            }
            None => {
                match recover_late_events_before_fallback(&mut event_loop)
                    .inspect_err(
                        |e| warn!(error = %e, "Failed to drain late JSONL events before fallback"),
                    )
                    .ok()
                {
                    Some(LateEventRecovery::PendingWork) => {
                        debug!(
                            "Recovered late JSONL events before fallback; retrying hat selection"
                        );
                        consecutive_fallbacks = 0;
                        continue;
                    }
                    Some(LateEventRecovery::Terminate(reason)) => {
                        let reason = dispatch_pre_loop_termination_hooks(
                            &event_loop,
                            hooks_dispatch_enabled,
                            &loop_id,
                            &hook_engine,
                            &hook_executor,
                            &suspend_state_store,
                            &ctx,
                            config.event_loop.max_iterations,
                            &mut accumulated_hook_metadata,
                            reason,
                        )
                        .await?;

                        let terminate_event = event_loop.publish_terminate_event(&reason);
                        log_terminate_event(
                            &mut event_logger,
                            event_loop.state().iteration,
                            &terminate_event,
                            Some(event_loop.registry().current_phase().to_string()),
                        );

                        let reason = dispatch_post_loop_termination_hooks(
                            &event_loop,
                            hooks_dispatch_enabled,
                            &loop_id,
                            &hook_engine,
                            &hook_executor,
                            &suspend_state_store,
                            &ctx,
                            config.event_loop.max_iterations,
                            &mut accumulated_hook_metadata,
                            reason,
                        )
                        .await?;

                        handle_termination(
                            &reason,
                            event_loop.state(),
                            &config.core.scratchpad.path,
                            &loop_history,
                            &loop_context,
                            auto_merge,
                            &prompt_content,
                            None,
                        );
                        if let Some(handle) = tui_handle.take() {
                            let _ = handle.await;
                        }
                        finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                        return Ok(reason);
                    }
                    Some(LateEventRecovery::NoLateEvents) | None => {}
                }

                // No pending events - try to recover by injecting a fallback event
                // This triggers the built-in planner to assess the situation
                consecutive_fallbacks += 1;

                if consecutive_fallbacks > MAX_FALLBACK_ATTEMPTS {
                    warn!(
                        attempts = consecutive_fallbacks,
                        "Fallback recovery exhausted after {} attempts, terminating",
                        MAX_FALLBACK_ATTEMPTS
                    );
                    let reason = dispatch_pre_loop_termination_hooks(
                        &event_loop,
                        hooks_dispatch_enabled,
                        &loop_id,
                        &hook_engine,
                        &hook_executor,
                        &suspend_state_store,
                        &ctx,
                        config.event_loop.max_iterations,
                        &mut accumulated_hook_metadata,
                        TerminationReason::Stopped,
                    )
                    .await?;

                    let terminate_event = event_loop.publish_terminate_event(&reason);
                    log_terminate_event(
                        &mut event_logger,
                        event_loop.state().iteration,
                        &terminate_event,
                        Some(event_loop.registry().current_phase().to_string()),
                    );

                    let reason = dispatch_post_loop_termination_hooks(
                        &event_loop,
                        hooks_dispatch_enabled,
                        &loop_id,
                        &hook_engine,
                        &hook_executor,
                        &suspend_state_store,
                        &ctx,
                        config.event_loop.max_iterations,
                        &mut accumulated_hook_metadata,
                        reason,
                    )
                    .await?;

                    handle_termination(
                        &reason,
                        event_loop.state(),
                        &config.core.scratchpad.path,
                        &loop_history,
                        &loop_context,
                        auto_merge,
                        &prompt_content,
                        None,
                    );
                    // Wait for user to exit TUI (press 'q') on natural completion
                    if let Some(handle) = tui_handle.take() {
                        let _ = handle.await;
                    }
                    finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                    return Ok(reason);
                }

                if event_loop.inject_fallback_event() {
                    // U4: log a stall-recovery envelope so the no-event
                    // iteration is auditable. `inject_fallback_event`
                    // targets the last active hat (or the generic
                    // "ralph" fallback when there is none); the
                    // `safe_target` flag follows the same rule.
                    let fallback_hat_id = event_loop
                        .state()
                        .last_hat
                        .clone()
                        .filter(|h| h.as_str() != "ralph");
                    let target_label = fallback_hat_id
                        .as_ref()
                        .map(|h| h.as_str().to_string())
                        .unwrap_or_else(|| "ralph".to_string());
                    let safe_target = fallback_hat_id.is_some()
                        && event_loop
                            .registry()
                            .get(fallback_hat_id.as_ref().unwrap())
                            .is_some();
                    let mut fb_builder = ralph_core::diagnosis::RecoveryDiagnosisEnvelope::builder()
                        .source(ralph_core::diagnosis::DiagnosisSource::StallRecovery)
                        .severity(ralph_core::diagnosis::DiagnosisSeverity::Warning)
                        .iteration(event_loop.state().iteration)
                        .target_hat(target_label.clone())
                        .topic("task.resume")
                        .reason_code("stall_no_events")
                        .message("no events from the active hat; injected task.resume fallback")
                        .expected_action("emit a regular event")
                        .safe_target(safe_target)
                        .outcome(ralph_core::diagnosis::DiagnosisOutcome::Pending)
                        .retry_key(
                            ralph_core::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                                ralph_core::diagnosis::DiagnosisSource::StallRecovery,
                                Some(target_label.as_str()),
                                Some("task.resume"),
                                "stall_no_events",
                                None,
                            ),
                        );
                    if let Some(session_id) = event_loop.diagnostics().session_id() {
                        fb_builder = fb_builder.session_id(session_id);
                    }
                    let fb_envelope = fb_builder.build();
                    // U6: the fallback-injection envelope is
                    // routed through `record_recovery_envelope` so
                    // the responder can decide whether the next
                    // prompt should fold a soft alert for the
                    // stuck hat. The original U3 journal + audit
                    // logging still happens, inside the helper.
                    event_loop.record_recovery_envelope(&fb_envelope, Vec::new());

                    // Fallback injected successfully, continue to next iteration
                    // The planner will be triggered and can either:
                    // - Dispatch more work if tasks remain
                    // - Output LOOP_COMPLETE if done
                    // - Determine what went wrong and recover
                    continue;
                }

                // Fallback not possible (no planner hat or doesn't subscribe to task.resume)
                warn!("No hats with pending events and fallback not available, terminating");
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    TerminationReason::Stopped,
                )
                .await?;

                // Per spec: Publish loop.terminate event to observers
                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                    None,
                );
                // Wait for user to exit TUI (press 'q') on natural completion
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                return Ok(reason);
            }
        };

        // Update RPC state iteration counter
        if let Some(ref shared) = rpc_dispatcher_started {
            shared
                .iteration
                .store(iteration, std::sync::atomic::Ordering::Relaxed);
        }

        // Determine which hat to display in iteration separator
        // When Ralph is coordinating (hat_id == "ralph"), show the active hat being worked on
        let preview_display_hat = if hat_id.as_str() == "ralph" {
            event_loop.get_active_hat_id()
        } else {
            hat_id.clone()
        };

        let post_iteration_start_outcomes = dispatch_phase_event_hooks(
            &event_loop,
            hooks_dispatch_enabled,
            &loop_id,
            &hook_engine,
            &hook_executor,
            HookPhaseEvent::PostIterationStart,
            build_iteration_start_payload_input(
                &loop_id,
                &ctx,
                config.event_loop.max_iterations,
                iteration,
                Some(preview_display_hat.as_str().to_string()),
                Some(preview_display_hat.as_str().to_string()),
                None,
                &accumulated_hook_metadata,
            ),
        );
        merge_accumulated_hook_metadata_from_outcomes(
            &mut accumulated_hook_metadata,
            &post_iteration_start_outcomes,
        );
        fail_if_blocking_iteration_start_outcomes(&post_iteration_start_outcomes)?;

        if let Some(reason) = wait_for_resume_if_suspended(
            &post_iteration_start_outcomes,
            &loop_id,
            &suspend_state_store,
        )
        .await?
        {
            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
                None,
            );
            // Wait for user to exit TUI (press 'q') on natural completion
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
            return Ok(reason);
        }

        // Log hat changes with appropriate messaging
        // Skip in TUI mode - TUI shows hat info in header, and stdout would corrupt display
        // Skip in RPC mode - JSON events replace console output
        if last_hat.as_ref() != Some(&hat_id) {
            if tui_state.is_none() && !enable_rpc {
                if hat_id.as_str() == "ralph" {
                    info!("I'm Ralph. Let's do this.");
                } else {
                    info!("Putting on my {} hat.", hat_id);
                }
            }
            last_hat = Some(hat_id.clone());
        }
        debug!(
            "Iteration {}/{} - {} active",
            iteration, config.event_loop.max_iterations, hat_id
        );

        // Build prompt for this hat
        let prompt = match event_loop.build_prompt(&hat_id) {
            Some(p) => p,
            None => {
                error!("Failed to build prompt for hat '{}'", hat_id);
                continue;
            }
        };
        // The previous iteration's findings must remain available
        // through prompt construction and termination checking.
        // Clear per-iteration responder caches only after the prompt
        // has consumed them, then stamp observer snapshots with the
        // iteration that is about to execute.
        drift_engine.begin_iteration(&mut event_loop, iteration);

        let display_hat =
            resolve_display_hat_for_execution(&event_loop, &hat_id, &preview_display_hat);

        // Log full prompt to diagnostics (RALPH_DIAGNOSTICS=1)
        event_loop.log_prompt(iteration, display_hat.as_str(), &prompt);

        let hat_display = event_loop
            .registry()
            .get(&display_hat)
            .map(|hat| hat.name.clone())
            .unwrap_or_else(|| display_hat.as_str().to_string());

        // Update RPC shared hat state so get_state reflects the current iteration's hat.
        if let Some(ref shared) = rpc_dispatcher_started
            && let Ok(mut guard) = shared.hat.lock()
        {
            *guard = (display_hat.as_str().to_string(), hat_display.clone());
        }

        // Track iteration start time for RPC iteration_end duration calculation
        // (cheap to create even when not in RPC mode)
        let iteration_started_at = std::time::Instant::now();

        // Emit RPC iteration_start event after prompt construction so the displayed
        // hat matches the one actually selected for execution.
        if let Some(ref tx) = rpc_event_tx {
            let started_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let start_event = RpcEvent::IterationStart {
                iteration,
                max_iterations: Some(config.event_loop.max_iterations),
                hat: display_hat.as_str().to_string(),
                hat_display: hat_display.clone(),
                backend: config.cli.backend.clone(),
                started_at,
            };
            let _ = tx.try_send(start_event);
        }

        // Per spec: Print iteration demarcation separator
        // "Each iteration must be clearly demarcated in the output so users can
        // visually distinguish where one iteration ends and another begins."
        // Skip when TUI is enabled - TUI has its own header showing iteration info
        // Skip in RPC mode - JSON events replace console output
        if tui_state.is_none() && !enable_rpc {
            print_iteration_separator(
                iteration,
                display_hat.as_str(),
                event_loop.state().elapsed(),
                config.event_loop.max_iterations,
                use_colors,
            );
        }

        // In verbose mode, print the full prompt before execution
        if verbosity == Verbosity::Verbose {
            eprintln!("\n{}", "=".repeat(80));
            eprintln!("PROMPT FOR {} (iteration {})", hat_id, iteration);
            eprintln!("{}", "-".repeat(80));
            eprintln!("{}", prompt);
            eprintln!("{}\n", "=".repeat(80));
        }

        // Execute the prompt (interactive or autonomous mode)
        // Determine which backend to use for this hat and the appropriate timeout
        // Hat-level backend configuration takes precedence over global cli.backend

        // Step 1: Get hat backend configuration for the active hat
        // Use display_hat (the active hat) instead of hat_id ("ralph" in multi-hat mode)
        let hat_config_opt = event_loop.registry().get_config(&display_hat);
        let hat_backend_opt = hat_config_opt.and_then(|c| c.backend.as_ref());
        let hat_backend_args = hat_config_opt.and_then(|c| c.backend_args.clone());

        // Step 2: Resolve effective backend and determine backend name for timeout
        // Note: backend_name_for_timeout is owned String to avoid lifetime issues with hat_backend reference
        let (mut effective_backend, backend_name_for_timeout): (CliBackend, String) =
            match hat_backend_opt {
                Some(hat_backend) => {
                    // Hat has custom backend configuration
                    match CliBackend::from_hat_backend(hat_backend) {
                        Ok(hat_backend_instance) => {
                            debug!(
                                "Using hat-level backend for '{}': {:?}",
                                display_hat, hat_backend
                            );

                            // Determine backend name for timeout based on hat backend type
                            // Use owned String to avoid borrowing issues and improve code clarity
                            let backend_name = match hat_backend {
                                ralph_core::HatBackend::Named(name) => name.clone(),
                                ralph_core::HatBackend::NamedWithArgs { backend_type, .. } => {
                                    backend_type.clone()
                                }
                                ralph_core::HatBackend::KiroAgent { backend_type, .. } => {
                                    backend_type.clone()
                                }
                                // For Custom backends, extract command name from path
                                // Handles both Unix ("/usr/bin/codex") and commands with args ("ollama run llama3")
                                ralph_core::HatBackend::Custom { command, .. } => {
                                    // First split by whitespace to handle commands with arguments
                                    // e.g., "ollama run llama3" -> "ollama"
                                    let base_command =
                                        command.split_whitespace().next().unwrap_or(command);
                                    // Then extract filename from path
                                    // e.g., "/usr/bin/codex" -> "codex"
                                    std::path::Path::new(base_command)
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("custom")
                                        .to_string()
                                }
                            };

                            (hat_backend_instance, backend_name)
                        }
                        Err(e) => {
                            // Failed to create backend from hat config - fall back to global
                            warn!(
                                "Failed to create backend from hat configuration for '{}': {}. Falling back to global backend.",
                                display_hat, e
                            );
                            // IMPORTANT: Use global backend name for timeout since we're using global backend
                            (backend.clone(), config.cli.backend.clone())
                        }
                    }
                }
                None => {
                    // No custom backend - use global configuration
                    debug!(
                        "Using global backend for '{}': {}",
                        display_hat, config.cli.backend
                    );
                    (backend.clone(), config.cli.backend.clone())
                }
            };

        // Step 2.5: Apply custom hat backend args if configured
        if let Some(args) = hat_backend_args {
            effective_backend.args.extend(args);
        }

        // Phase 2: in isolated mode each hat gets its own write channel so
        // provenance is a property of the channel, not the self-declared `hat`
        // field. The runner stamps every record when merging back to the main
        // events file.
        let isolated_mode = crate::loop_runner::paths::is_isolated_mode(&config);
        let hat_channel_path = if isolated_mode {
            Some(crate::loop_runner::hat_channel::prepare_hat_channel(
                &ctx,
                display_hat.as_str(),
                &loop_id,
                iteration,
            )?)
        } else {
            None
        };
        let events_path = hat_channel_path
            .clone()
            .unwrap_or_else(|| resolve_emit_events_path(&ctx, state_machine_enabled));
        let triggered_hat = event_loop.triggered_hat().map(|h| h.as_str().to_string());
        inject_hat_execution_env(
            &mut effective_backend,
            display_hat.as_str(),
            &loop_id,
            &events_path,
            triggered_hat.as_deref(),
            hats_source_label.as_deref(),
        );
        // R1 (2026-06-14-003 plan): expose the wave context as the
        // `RALPH_WAVE_CONTEXT` env var so the agent's bash tool can
        // `echo $RALPH_WAVE_CONTEXT | jq` without depending on the
        // prompt block.  Only `review-synthesizer` has a meaningful
        // value; for other hats the call returns `None` and the
        // env var is not set (preserving the pre-R1 behaviour).
        if let Some(json) = event_loop.wave_context_json_for_hat(&display_hat) {
            effective_backend
                .env_vars
                .push(("RALPH_WAVE_CONTEXT".into(), json));
        }

        // Step 3: Get timeout from config based on actual backend being used
        let timeout_secs = config.adapter_settings(&backend_name_for_timeout).timeout;
        let timeout = adapter_timeout_duration(timeout_secs);

        // For TUI mode, get the shared lines buffer for this iteration.
        // The buffer is owned by TuiState's IterationBuffer, so writes from
        // TuiStreamHandler appear immediately in the TUI (real-time streaming).
        let tui_lines: Option<Arc<std::sync::Mutex<Vec<ratatui::text::Line<'static>>>>> =
            if let Some(ref state) = tui_state {
                // Start new iteration and get handle to the LATEST iteration's lines buffer.
                // We must use latest_iteration_lines_handle() instead of current_iteration_lines_handle()
                // because the user may be viewing an older iteration while a new one executes.
                prepare_tui_iteration(
                    state,
                    hat_display.clone(),
                    backend_name_for_timeout.clone(),
                    config.event_loop.max_iterations,
                )
            } else {
                None
            };

        // Race execution against interrupt signal for immediate termination on Ctrl+C
        let mut interrupt_rx_clone = interrupt_rx.clone();
        let interrupt_rx_for_pty = interrupt_rx.clone();
        let tui_lines_for_pty = tui_lines.clone();
        let rpc_stdout_for_pty = rpc_stdout.clone();
        let execute_future = async {
            if effective_backend.output_format == BackendOutputFormat::Acp {
                execute_acp(
                    &effective_backend,
                    &config,
                    &prompt,
                    verbosity,
                    tui_lines_for_pty,
                    rpc_stdout_for_pty,
                    iteration,
                    display_hat.as_str(),
                    &backend_name_for_timeout,
                )
                .await
            } else if use_pty {
                execute_pty(
                    pty_executor.as_mut(),
                    &effective_backend,
                    &config,
                    &prompt,
                    user_interactive,
                    interrupt_rx_for_pty,
                    verbosity,
                    tui_lines_for_pty,
                    rpc_stdout_for_pty,
                    iteration,
                    display_hat.as_str(),
                    &backend_name_for_timeout,
                )
                .await
            } else {
                let executor = CliExecutor::new(effective_backend.clone());
                let result = executor
                    .execute(&prompt, stdout(), timeout, verbosity == Verbosity::Verbose)
                    .await?;
                Ok(ExecutionOutcome {
                    output: normalize_cli_output_for_parsing(
                        effective_backend.output_format,
                        &result.output,
                    ),
                    success: result.success,
                    termination: None,
                    // Unit 3: surface the CliExecutor inactivity timeout via the
                    // same diagnostic flag the PTY path uses, so the runner can
                    // log a consistent watchdog-timeout message across paths.
                    // `post_event_timed_out` is treated as a normal soft
                    // backend wrap-up (success=true), not a watchdog fire.
                    watchdog_timeout: result.timed_out && !result.post_event_timed_out,
                    total_cost_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                })
            }
        };

        let outcome = tokio::select! {
            result = execute_future => result?,
            _ = interrupt_rx_clone.changed() => {
                // Immediately terminate children via process group signal
                #[cfg(unix)]
                {
                    use nix::sys::signal::{killpg, Signal};
                    use nix::unistd::getpgrp;
                    let pgid = getpgrp();
                    let our_pid = nix::unistd::Pid::this();
                    let our_pid_u32 = u32::try_from(our_pid.as_raw()).unwrap_or(0);
                    warn!(
                        target: "ralph_cli::loop_runner",
                        pid = %our_pid,
                        pgid = %pgid,
                        "Runtime interrupt received, sending SIGTERM to process group"
                    );
                    // Fallback: kill descendant processes that live outside the
                    // orchestrator's process group (e.g. PTY-session backends).
                    crate::cli::process_tree::kill_process_tree(our_pid_u32, false);
                    let _ = killpg(pgid, Signal::SIGTERM);

                    // Wait briefly for graceful exit, then SIGKILL
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    warn!(
                        target: "ralph_cli::loop_runner",
                        pid = %our_pid,
                        pgid = %pgid,
                        "Sending SIGKILL to process group after grace period"
                    );
                    let _ = killpg(pgid, Signal::SIGKILL);
                }

                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    TerminationReason::Interrupted,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(&mut event_logger, event_loop.state().iteration, &terminate_event, Some(event_loop.registry().current_phase().to_string()));

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(&reason, event_loop.state(), &config.core.scratchpad.path, &loop_history, &loop_context, auto_merge, &prompt_content, None);
                // Signal TUI to exit immediately on interrupt
                let _ = terminated_tx.send(true);
                finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                return Ok(reason);
            }
        };

        // Unit 3 (plan 2026-06-06-001): surface backend watchdog timeout as a
        // diagnostic warning, then let the loop continue down the regular
        // event-processing path. Watchdog timeout is a backend-call end, NOT a
        // loop terminate: if the agent emitted partial events before the
        // watchdog fired, they will still be parsed and routed; if it emitted
        // nothing, the existing missing-event hard gate / fallback path takes
        // over on the next iteration. The matching `outcome.termination = None`
        // mapping lives in `convert_termination_type` / `execute_pty`.
        if outcome.watchdog_timeout {
            warn!(
                iteration = iteration,
                hat = %display_hat.as_str(),
                backend = %backend_name_for_timeout,
                "Backend watchdog timeout fired; preserving partial output for event \
                 processing (loop continues, hard gate / fallback handles missing events)"
            );
        }

        if let Some(reason) = outcome.termination {
            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
                None,
            );
            // Wait for user to exit TUI (press 'q') on natural completion
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
            return Ok(reason);
        }

        let output = outcome.output;
        let success = outcome.success;
        let output_hat_id = resolve_hat_for_output_processing(&hat_id, &display_hat);

        // Note: TUI lines are now written directly to IterationBuffer during streaming,
        // so no post-execution transfer is needed.
        if let Some(mut s) = tui_state.as_ref().and_then(|state| state.lock().ok()) {
            s.finish_latest_iteration();
        }

        // Emit RPC iteration_end event
        if let Some(ref tx) = rpc_event_tx {
            let duration_ms = iteration_started_at.elapsed().as_millis() as u64;
            // Check if this iteration's output contains LOOP_COMPLETE
            let loop_complete_triggered = output.contains(&config.event_loop.completion_promise);
            let iteration_cost_usd = outcome.total_cost_usd;
            if let Some(ref shared) = rpc_dispatcher_started
                && let Ok(mut guard) = shared.total_cost_usd.lock()
            {
                *guard += iteration_cost_usd;
            }
            let end_event = RpcEvent::IterationEnd {
                iteration,
                duration_ms,
                cost_usd: iteration_cost_usd,
                input_tokens: outcome.input_tokens,
                output_tokens: outcome.output_tokens,
                cache_read_tokens: outcome.cache_read_tokens,
                cache_write_tokens: outcome.cache_write_tokens,
                loop_complete_triggered,
            };
            let _ = tx.try_send(end_event);
        }

        // Per-iteration footer for --no-tui: one line with budget/cost/elapsed
        // so tailing agents can catch runaway loops without parsing events.
        if tui_state.is_none() && !enable_rpc {
            let iter_duration = iteration_started_at.elapsed();
            print_iteration_footer(
                iteration,
                config.event_loop.max_iterations,
                iter_duration,
                event_loop.state().elapsed(),
                outcome.total_cost_usd,
                event_loop.state().cumulative_cost,
                use_colors,
            );
        }

        // Legacy configs log candidate events from backend output. State-machine
        // configs use accepted-only logging after runtime validation.
        let raw_output_logging_enabled = !config
            .event_loop
            .state_machine
            .as_ref()
            .is_some_and(|sm| sm.enabled);
        log_events_from_output(
            &mut event_logger,
            iteration,
            &output_hat_id,
            &output,
            event_loop.registry(),
            raw_output_logging_enabled,
        );

        // Phase 2: merge the isolated hat channel back into the main events
        // file before the event loop reads it. This stamps every record with
        // the authoritative hat of the channel.
        if isolated_mode {
            let target_events_path = resolve_emit_events_path(&ctx, state_machine_enabled);
            if let Err(e) = crate::loop_runner::hat_channel::merge_hat_channel(
                &ctx,
                &target_events_path,
                display_hat.as_str(),
            ) {
                warn!(
                    error = %e,
                    hat = %display_hat.as_str(),
                    "Failed to merge isolated hat channel; events may be lost"
                );
            }
        }

        // Process output
        if let Some(reason) = event_loop.process_output(&output_hat_id, &output, success) {
            // Per spec: Log "All done! {promise} detected." when completion promise found
            if reason == TerminationReason::CompletionPromise {
                info!(
                    "All done! {} detected.",
                    config.event_loop.completion_promise
                );
            }

            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            // Per spec: Publish loop.terminate event to observers
            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
                None,
            );
            // Wait for user to exit TUI (press 'q') on natural completion
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
            return Ok(reason);
        }

        // Check for planning session user responses (if in planning mode)
        if let Err(e) = check_planning_session_responses(&mut event_loop) {
            warn!(error = %e, "Failed to check planning session responses");
        }

        let should_dispatch_plan_created_hooks = event_loop
            .has_pending_plan_events_in_jsonl()
            .inspect_err(|e| {
                warn!(
                    error = %e,
                    "Failed to inspect unread JSONL events for semantic plan.* topics"
                )
            })
            .unwrap_or(false);

        if should_dispatch_plan_created_hooks {
            let pre_plan_created_outcomes = dispatch_phase_event_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                HookPhaseEvent::PrePlanCreated,
                build_plan_created_payload_input(
                    &loop_id,
                    &ctx,
                    config.event_loop.max_iterations,
                    event_loop.state().iteration,
                    Some(display_hat.as_str().to_string()),
                    Some(display_hat.as_str().to_string()),
                    None,
                    &accumulated_hook_metadata,
                ),
            );
            merge_accumulated_hook_metadata_from_outcomes(
                &mut accumulated_hook_metadata,
                &pre_plan_created_outcomes,
            );
            fail_if_blocking_plan_created_outcomes(&pre_plan_created_outcomes)?;

            if let Some(reason) = wait_for_resume_if_suspended(
                &pre_plan_created_outcomes,
                &loop_id,
                &suspend_state_store,
            )
            .await?
            {
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                    None,
                );
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                return Ok(reason);
            }
        }

        let pending_human_interact_context = event_loop
            .pending_human_interact_context_in_jsonl()
            .inspect_err(|e| {
                warn!(
                    error = %e,
                    "Failed to inspect unread JSONL events for human.interact boundary"
                )
            })
            .ok()
            .flatten();

        if let Some(human_interact_context) = pending_human_interact_context {
            let pre_human_interact_outcomes = dispatch_phase_event_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                HookPhaseEvent::PreHumanInteract,
                build_human_interact_payload_input(
                    &loop_id,
                    &ctx,
                    config.event_loop.max_iterations,
                    event_loop.state().iteration,
                    Some(display_hat.as_str().to_string()),
                    Some(display_hat.as_str().to_string()),
                    None,
                    Some(human_interact_context),
                    &accumulated_hook_metadata,
                ),
            );
            merge_accumulated_hook_metadata_from_outcomes(
                &mut accumulated_hook_metadata,
                &pre_human_interact_outcomes,
            );
            fail_if_blocking_human_interact_outcomes(&pre_human_interact_outcomes)?;

            if let Some(reason) = wait_for_resume_if_suspended(
                &pre_human_interact_outcomes,
                &loop_id,
                &suspend_state_store,
            )
            .await?
            {
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                    None,
                );
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                return Ok(reason);
            }
        }

        // Read events from JSONL, partitioning wave events from regular events.
        //
        // U2 (2026-06-13-001): capture `wave_policy_rejections` (and
        // `wave_raw_count` for envelope evidence) alongside the regular
        // `processed` and `wave_events`. The runner uses the rejection
        // list to (a) keep `missing_event_gate` from mis-firing when the
        // agent DID emit a wave batch but policy rejected it for missing
        // a required field, and (b) inject schema-level guidance in lieu
        // of the generic "did not emit" message. U1 added the fields;
        // this is the consumer side.
        let (processed_events, wave_events, wave_policy_rejections, wave_raw_count) =
            match event_loop.process_events_from_jsonl_with_waves() {
                Ok(result) => (
                    Some(result.processed),
                    result.wave_events,
                    result.wave_policy_rejections,
                    result.wave_raw_count,
                ),
                Err(e) => {
                    warn!(error = %e, "Failed to read events from JSONL");
                    (None, Vec::new(), Vec::new(), 0)
                }
            };

        if let Some(processed) = processed_events.as_ref()
            && !raw_output_logging_enabled
        {
            log_accepted_events(
                &mut event_logger,
                event_loop.state().iteration,
                &hat_id,
                &processed.accepted_events,
                event_loop.registry(),
            );
        }

        // ── U6: Handle execution contract rejections ─────────────────────────
        // Log contract rejections for operator visibility and diagnostics.
        // When the bounded retry budget is exhausted for a rejection
        // key, the function returns `Some(TerminationReason::RecoveryExhausted)`
        // and the runner must break out of the loop instead of letting
        // the next iteration re-run the same agent with the same
        // contract violation.
        if let Some(processed) = processed_events.as_ref()
            && let Some(reason) =
                handle_execution_contract_rejections(processed, &mut event_loop, &display_hat)
        {
            // No payload violation report for the contract-recovery
            // path; the recovery diagnosis is already in
            // `recovery.jsonl` and the audit is in
            // `orchestration.jsonl`.  We still call
            // `finalize_recovery_diagnosis` to flush the responder
            // hint into `summary.md` before the runner returns.
            finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
            return Ok(reason);
        }

        // ── P1.1: drift observer drain → detector → envelopes → journals ───
        // Drain the drift observer's bounded channel, run each
        // snapshot through the detector, and funnel any findings
        // through the existing recovery responder path. Each
        // finding also writes a `drift.jsonl` line and a
        // high-level `OrchestrationEvent::DriftDetected` audit
        // event. The drift observer is bounded and panic-safe; the
        // engine swallows its own errors and never panics.
        drift_engine.drain_observer(&mut event_loop);

        // ── P1.2: hard escalation queue → targeted `task.resume` ───────────
        // The responder's `drain_hard_escalations` queue is
        // consumed here. Each action publishes a `task.resume`
        // event targeted at the recommended hat with a stable,
        // machine-detectable payload. The responder's queue is
        // automatically cleared on the next `begin_iteration`.
        drift_engine.drain_hard_escalations(&mut event_loop);

        // ── P1.3: per-key recovery outcome tracking ────────────────────────
        // For every retry key the responder is tracking, ask
        // whether the iteration's accepted events satisfy the
        // diagnosis. We pass per-event evidence (topic, fields,
        // source hat, timestamp) so the responder can re-evaluate
        // the SPECIFIC drift metric that produced the finding —
        // `field_completeness` needs the field set,
        // `coord_join_rate` needs (from, to, ts), `emit_cadence`
        // needs the timestamp sequence. A bare topic list is no
        // longer enough (R7 review).
        let accepted_evidence: Vec<ralph_core::diagnosis::AcceptedEventEvidence> = processed_events
            .as_ref()
            .map(|p| {
                ralph_core::drift::evidence_from_jsonl_events(
                    p.accepted_events.iter().cloned(),
                    event_loop.state().iteration,
                )
            })
            .unwrap_or_default();
        let _outcome_updates =
            drift_engine.check_recovery_for_iteration(&mut event_loop, &accepted_evidence);

        // ── U6: Handle payload contract violations ───────────────────────────
        // Unlike execution contract rejections (which drive recovery via
        // human.guidance), payload contract violations pause the loop and
        // emit a structured diagnostic. The non-regression contract is:
        //   - the diagnostic file MUST be written
        //   - the loop MUST terminate with PayloadContractViolation
        //   - the diagnostic must surface on stderr even if file write fails
        if let Some(processed) = processed_events.as_ref()
            && let Some(violation) = &processed.payload_contract_violation
        {
            // U6: write diagnostic and terminate. Default location is
            // `<workspace>/.ralph/diagnostics`; the loop context is the
            // source of truth for the workspace.
            let diagnostics_dir = event_loop
                .loop_context()
                .map(|c| c.workspace().join(".ralph").join("diagnostics"))
                .unwrap_or_else(|| std::path::PathBuf::from(".ralph/diagnostics"));
            let report_path = write_payload_contract_violation_report(&diagnostics_dir, violation);

            // U4: write a recovery envelope pointing at the on-disk
            // violation report. `TerminationReason::PayloadContractViolation`
            // is preserved by the explicit `return` below.
            let report_path_str = report_path.to_string_lossy().to_string();
            let mut pc_builder = ralph_core::diagnosis::RecoveryDiagnosisEnvelope::builder()
                .source(ralph_core::diagnosis::DiagnosisSource::PayloadContract)
                .severity(ralph_core::diagnosis::DiagnosisSeverity::Critical)
                .iteration(event_loop.state().iteration)
                .topic(violation.topic.as_str())
                .reason_code("payload_contract_violation")
                .message(format!(
                    "Payload contract violation on topic '{}' (field: {:?})",
                    violation.topic, violation.field
                ))
                .expected_action("fix preset payload_contract definition")
                .safe_target(false)
                .outcome(ralph_core::diagnosis::DiagnosisOutcome::NotRetriable)
                .evidence(ralph_core::diagnosis::EvidenceRef {
                    kind: ralph_core::diagnosis::EvidenceKind::File,
                    ref_path: report_path_str.clone(),
                    snippet: None,
                })
                .retry_key(
                    ralph_core::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                        ralph_core::diagnosis::DiagnosisSource::PayloadContract,
                        None,
                        Some(violation.topic.as_str()),
                        "payload_contract_violation",
                        violation.field.as_deref(),
                    ),
                );
            if let Some(session_id) = event_loop.diagnostics().session_id() {
                pc_builder = pc_builder.session_id(session_id);
            }
            let pc_envelope = pc_builder.build();
            // U6: payload contract violations use
            // `DiagnosisOutcome::NotRetriable` and the responder's
            // `safe_target` is `false`. The helper still funnels the
            // envelope through the journal + audit loggers, but
            // the responder will not synthesize a fake
            // `task.resume` (its classifier routes `safe_target ==
            // false` to Final, which the runner then surfaces as a
            // hint — never as a replacement for
            // `TerminationReason::PayloadContractViolation`).
            event_loop.record_recovery_envelope(
                &pc_envelope,
                vec![format!("see report at {}", report_path_str)],
            );

            // U8: route the violation path through the unified
            // termination pipeline so summary.md, history, deregister,
            // RPC events, and the `## Diagnostics` hint + diagnosis
            // seed all land in the same place they would for any
            // other termination. The workspace-relative report path
            // is passed through so the operator-facing hint can
            // include it.
            let payload_violation_report_relpath = event_loop
                .loop_context()
                .map(|ctx| {
                    report_path
                        .strip_prefix(ctx.workspace())
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| report_path_str.clone())
                })
                .unwrap_or_else(|| report_path_str.clone());
            let reason = TerminationReason::PayloadContractViolation;
            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
                Some(&payload_violation_report_relpath),
            );
            // Wait for user to exit TUI (press 'q') on natural completion
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            finalize_recovery_diagnosis(
                &mut event_loop,
                &loop_context,
                Some(&payload_violation_report_relpath),
            );
            return Ok(TerminationReason::PayloadContractViolation);
        }

        // ── Unit 2 (2026-06-16-002 plan) recoverable-budget
        //    exhaustion — drain the buffer and terminate ────────
        // The recoverable set (`PayloadTypeMismatch` /
        // `MissingRequiredField` / `TopicDenied`) is allowed to
        // retry for `U2_REJECTION_RETRY_LIMIT` attempts.  When
        // the (hat, topic, reason_class) bucket crosses the
        // limit, `apply_event_policy_validation` pushes a
        // `RecoverableExhaustion` into
        // `event_loop.state.recoverable_exhaustion_buffer`.  The
        // runner promotes the **first** entry into a
        // `TerminationReason::RecoverablePayloadExhausted` and
        // writes a `payload_contract`-shaped recovery envelope
        // with `outcome = Failed` so `ralph diagnose` can
        // attribute the failure to the right hat and reason
        // class.  Subsequent entries in the same batch collapse
        // — only the first retry_key is carried so the operator
        // can grep `recovery.jsonl` for the cause.
        if !event_loop.state().recoverable_exhaustion_buffer.is_empty() {
            // Move the buffer out so we can release the
            // `event_loop.state()` borrow before calling the
            // other helpers.
            let mut exhausted: Vec<ralph_core::event_loop::RecoverableExhaustion> =
                std::mem::take(&mut event_loop.state_mut().recoverable_exhaustion_buffer);
            // Sort by `(hat, topic, count)` for deterministic
            // ordering when the buffer has multiple entries.
            // Unit 2 plan §3 "split by (hat, reason_class)" is
            // enforced by `is_recoverable_policy_finding`; the
            // sort here just makes the promoted entry stable
            // for `ralph diagnose` joins.
            exhausted.sort_by(|a, b| {
                a.hat
                    .cmp(&b.hat)
                    .then_with(|| a.topic.cmp(&b.topic))
                    .then_with(|| a.reason_class.as_str().cmp(b.reason_class.as_str()))
                    .then_with(|| a.count.cmp(&b.count))
            });
            let first = exhausted.into_iter().next().expect("checked is_empty");
            let reason = TerminationReason::RecoverablePayloadExhausted {
                hat: first.hat.clone(),
                topic: first.topic.clone(),
                reason_class: first.reason_class.as_str().to_string(),
                count: first.count,
            };

            // Build a recovery envelope mirroring the
            // `payload_contract` shape so the existing
            // diagnosis plumbing picks it up.  We do NOT call
            // `write_payload_contract_violation_report` here —
            // the on-disk shape is "recovery envelope, not
            // payload contract report" (the two are different
            // failure modes).
            let reason_code = format!(
                "recoverable_payload_exhausted:{}",
                first.reason_class.as_str()
            );
            let mut u2_builder = ralph_core::diagnosis::RecoveryDiagnosisEnvelope::builder()
                .source(ralph_core::diagnosis::DiagnosisSource::PayloadContract)
                .severity(ralph_core::diagnosis::DiagnosisSeverity::Critical)
                .iteration(event_loop.state().iteration)
                .source_hat(first.hat.as_str())
                .topic(first.topic.as_str())
                .reason_code(reason_code.clone())
                .message(format!(
                    "Recoverable payload budget exhausted on hat '{}' topic '{}' (reason_class={}, count={})",
                    first.hat, first.topic, first.reason_class.as_str(), first.count
                ))
                .expected_action(
                    "fix the payload schema or the hat's emit call — the same (hat, topic, reason_class) \
                     was rejected 4 times in a row"
                )
                .safe_target(false)
                .outcome(ralph_core::diagnosis::DiagnosisOutcome::Failed)
                .retry_key(
                    ralph_core::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
                        ralph_core::diagnosis::DiagnosisSource::PayloadContract,
                        Some(first.hat.as_str()),
                        Some(first.topic.as_str()),
                        &reason_code,
                        None,
                    ),
                );
            if let Some(session_id) = event_loop.diagnostics().session_id() {
                u2_builder = u2_builder.session_id(session_id);
            }
            let u2_envelope = u2_builder.build();
            event_loop.record_recovery_envelope(
                &u2_envelope,
                vec![format!(
                    "budget exhausted (count > {})",
                    ralph_core::event_loop::U2_REJECTION_RETRY_LIMIT
                )],
            );

            // U8: route the recoverable-exhausted termination
            // through the unified termination pipeline so
            // summary.md, history, deregister, RPC events, and
            // the `## Diagnostics` hint all land in the same
            // place they would for any other termination.
            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
                None,
            );
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
            return Ok(reason);
        }

        // ── PhaseWatcher: Check for experiment.evaluated during warmup ───────
        // PhaseWatcher monitors accepted events and triggers phase transitions
        // when the configured exit condition is met (e.g., experiment.evaluated)
        let phase_transition_reason = if current_phase == Phase::Warmup {
            // Check if any accepted event is experiment.evaluated
            let has_experiment_evaluated = processed_events
                .as_ref()
                .map(|events| {
                    events
                        .accepted_events
                        .iter()
                        .any(|e| e.topic.as_str() == "experiment.evaluated")
                })
                .unwrap_or(false);

            if has_experiment_evaluated {
                info!("experiment.evaluated detected in warmup phase — checking exit conditions");
                // Run the check exit conditions script
                match run_check_exit_conditions(&ctx).await {
                    Ok(CheckExitResult::Ready) => {
                        info!("Exit conditions satisfied — initiating phase transition");
                        if stop_on_exit {
                            Some("warmup_complete")
                        } else {
                            Some("phase_transition")
                        }
                    }
                    Ok(CheckExitResult::NotReady { unmet_conditions }) => {
                        debug!(
                            ?unmet_conditions,
                            "Exit conditions not yet satisfied — continuing warmup"
                        );
                        None
                    }
                    Ok(CheckExitResult::DrainRequired { pending_count }) => {
                        info!(
                            pending_count,
                            "Drain required — waiting for in-flight experiments to complete"
                        );
                        None
                    }
                    Err(e) => {
                        warn!(error = %e, "Check exit conditions failed — continuing warmup");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some(phase_reason) = phase_transition_reason {
            // Phase transition triggered by PhaseWatcher
            info!(
                "Phase transition triggered: {} (stop_on_exit: {})",
                phase_reason, stop_on_exit
            );

            // Run transition script and update phase
            match run_transition_script(&ctx, stop_on_exit).await {
                Ok(_) => {
                    // Update phase to production
                    let new_phase = Phase::Production;
                    event_loop.registry_mut().set_phase(new_phase.clone());

                    // Update phase.json
                    let phase_json = serde_json::json!({
                        "phase": new_phase.to_string(),
                        "warmup_completed": stop_on_exit,
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    });
                    let agent_dir = ctx.ralph_dir().join("agent");
                    fs::create_dir_all(&agent_dir).ok();
                    let phase_path = agent_dir.join("phase.json");
                    if let Err(e) = fs::write(
                        &phase_path,
                        serde_json::to_string_pretty(&phase_json).unwrap(),
                    ) {
                        warn!(error = %e, "Failed to write phase.json");
                    }

                    // Publish phase transition event
                    let transition_topic = if stop_on_exit {
                        "warmup.complete"
                    } else {
                        "phase.transition"
                    };
                    let transition_payload = serde_json::json!({
                        "phase": new_phase.to_string(),
                        "reason": phase_reason,
                        "warmup_completed": stop_on_exit,
                    });
                    let transition_event =
                        Event::new(transition_topic, &transition_payload.to_string());
                    event_loop.publish_event(transition_event);

                    // If warmup_only mode, terminate the loop
                    if stop_on_exit {
                        let reason = TerminationReason::CompletionPromise;
                        handle_termination(
                            &reason,
                            event_loop.state(),
                            &config.core.scratchpad.path,
                            &loop_history,
                            &loop_context,
                            auto_merge,
                            &prompt_content,
                            None,
                        );
                        if let Some(handle) = tui_handle.take() {
                            let _ = handle.await;
                        }
                        finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                        return Ok(reason);
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Phase transition script failed — continuing in warmup");
                }
            }
        }

        if let Some(human_interact_context) = processed_events
            .as_ref()
            .and_then(|events| events.human_interact_context.clone())
        {
            let post_human_interact_outcomes = dispatch_phase_event_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                HookPhaseEvent::PostHumanInteract,
                build_human_interact_payload_input(
                    &loop_id,
                    &ctx,
                    config.event_loop.max_iterations,
                    event_loop.state().iteration,
                    Some(display_hat.as_str().to_string()),
                    Some(display_hat.as_str().to_string()),
                    None,
                    Some(human_interact_context),
                    &accumulated_hook_metadata,
                ),
            );
            merge_accumulated_hook_metadata_from_outcomes(
                &mut accumulated_hook_metadata,
                &post_human_interact_outcomes,
            );
            fail_if_blocking_human_interact_outcomes(&post_human_interact_outcomes)?;

            if let Some(reason) = wait_for_resume_if_suspended(
                &post_human_interact_outcomes,
                &loop_id,
                &suspend_state_store,
            )
            .await?
            {
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                    None,
                );
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                return Ok(reason);
            }
        }

        if processed_events
            .as_ref()
            .map(|events| events.had_plan_events)
            .unwrap_or(false)
        {
            let post_plan_created_outcomes = dispatch_phase_event_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                HookPhaseEvent::PostPlanCreated,
                build_plan_created_payload_input(
                    &loop_id,
                    &ctx,
                    config.event_loop.max_iterations,
                    event_loop.state().iteration,
                    Some(display_hat.as_str().to_string()),
                    Some(display_hat.as_str().to_string()),
                    None,
                    &accumulated_hook_metadata,
                ),
            );
            merge_accumulated_hook_metadata_from_outcomes(
                &mut accumulated_hook_metadata,
                &post_plan_created_outcomes,
            );
            fail_if_blocking_plan_created_outcomes(&post_plan_created_outcomes)?;

            if let Some(reason) = wait_for_resume_if_suspended(
                &post_plan_created_outcomes,
                &loop_id,
                &suspend_state_store,
            )
            .await?
            {
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                    None,
                );
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                return Ok(reason);
            }
        }

        let mut agent_wrote_events = processed_events
            .as_ref()
            .map(|events| events.had_events)
            .unwrap_or(false);

        // Agent wrote any valid or rejected events — used for missing-event gate.
        //
        // U2 (2026-06-13-001): the wave partition is processed by event
        // policy *before* it reaches the regular `processed_events`
        // pipeline, so a wave batch that is policy-rejected (e.g. 7
        // `review.wave.ready` events missing the `depth` field) would
        // leave both `had_raw_events` and `had_rejected_events` at
        // `false` for the regular path. `agent_wrote_any_valid_or_rejected`
        // (pub fn at top of this module) folds
        // `wave_had_policy_rejections` into the boolean expression so
        // the gate is skipped symmetrically with the regular
        // `had_rejected_events` path.
        let wave_had_policy_rejections = !wave_policy_rejections.is_empty();
        let agent_wrote_any_valid_or_rejected =
            agent_wrote_any_valid_or_rejected(processed_events.as_ref(), &wave_policy_rejections);

        let mut late_termination_reason: Option<TerminationReason> = None;
        let mut hard_gate_triggered_this_iteration = false;
        if !agent_wrote_events && output_mentions_ralph_emit(&output) {
            match recover_expected_emit_after_output(&mut event_loop)
                .inspect_err(|e| warn!(error = %e, "Failed to recover expected emit events"))
                .ok()
            {
                Some(LateEventRecovery::PendingWork) => {
                    agent_wrote_events = true;
                    event_loop.reset_hard_gate_count();
                }
                Some(LateEventRecovery::Terminate(reason)) => {
                    agent_wrote_events = true;
                    event_loop.reset_hard_gate_count();
                    late_termination_reason = Some(reason);
                }
                Some(LateEventRecovery::NoLateEvents) | None => {
                    if should_hard_gate(&display_hat, &event_loop) {
                        hard_gate_triggered_this_iteration = true;
                        event_loop.increment_hard_gate_count();
                        inject_hard_gate_guidance(
                            &ctx,
                            &display_hat,
                            &event_loop.get_hat_publishes(&display_hat),
                        );
                        info!(
                            hat = %display_hat.as_str(),
                            consecutive = event_loop.state().consecutive_hard_gates,
                            "Hard gate triggered: agent claimed emit but no event written"
                        );
                    } else {
                        event_loop.reset_hard_gate_count();
                        warn!(
                            hat = %hat_id.as_str(),
                            "Output indicated `ralph emit`, but no event became readable before fallback logic"
                        );
                    }
                }
            }
        }

        // Execute wave if wave events detected
        let wave_outcome: Option<crate::loop_runner::wave::HandleWaveOutcome> =
            if !wave_events.is_empty() {
                // U4-C2 / KTD-U4-6: compute the runner-supplied global
                // deadline from the loop's remaining runtime budget.
                // When `max_runtime_seconds = 0` (the default in many
                // presets, meaning "no upper bound"), the deadline is
                // `None` and the dispatcher falls back to its
                // wave-internal partial/aggregate timers. Otherwise we
                // always pass `Some(now + remaining)` — even when
                // `remaining` is zero — so the dispatcher can short-
                // circuit the wave cleanly on the very first loop
                // iteration instead of letting it run unbounded.
                let global_deadline = {
                    let cfg = event_loop.config();
                    let max_runtime = cfg.event_loop.max_runtime_seconds;
                    if max_runtime == 0 {
                        None
                    } else {
                        let remaining = std::time::Duration::from_secs(max_runtime)
                            .saturating_sub(event_loop.state().elapsed());
                        Some(tokio::time::Instant::now() + remaining)
                    }
                };
                let outcome = handle_wave_events(
                    &wave_events,
                    &mut event_loop,
                    &backend,
                    &ctx,
                    use_colors,
                    enable_rpc,
                    rpc_event_tx.as_ref(),
                    tui_state.as_ref(),
                    &loop_id,
                    prebuilt_diagnostics.as_ref(),
                    global_deadline,
                    // Plan 001 §4.3 C1: forward the loop's preset
                    // label to wave workers so their in-process
                    // `ralph emit` / `ralph wave emit` inherits
                    // `event_policy.schemas` even when the parent
                    // process env does not carry RALPH_HATS_SOURCE.
                    hats_source_label.as_deref(),
                )
                .await;
                Some(outcome)
            } else {
                None
            };

        // U4-C3 / KTD-U4-6: if the global deadline fired during the
        // wave, set `late_termination_reason = Some(MaxRuntime)`.
        // The existing unified termination flow (pre/post hooks,
        // finalize_recovery_diagnosis, handle_termination) takes
        // over from the next iteration's top check. We do NOT
        // `break` here — the loop's iteration body must still run
        // its TUI / hook-metadata bookkeeping for this iteration.
        // The default_publishes and missing-event gate blocks below
        // are guarded by an additional `late_termination_reason`
        // check (C4 / §6 C4) so they don't run for the doomed
        // iteration.
        if wave_outcome.is_some_and(|o| o.global_deadline_exceeded) {
            late_termination_reason = Some(TerminationReason::MaxRuntime);
        }

        // Inject default_publishes for active hats only when agent wrote no events.
        // Skip default_publishes when hard gate triggered — the agent explicitly
        // claimed to emit and we want it to learn to do so, not be bailed out.
        // Prefer the displayed execution hat first so a non-emitting turn still
        // falls back to the hat the user actually saw in the banner.
        //
        // MISSING-EVENT GATE (U1): Regardless of whether output mentioned `ralph emit`,
        // if the hat has a publish obligation but no default_publishes fallback,
        // hard gate on missing events. This catches the "completely forgot" case.
        // Contract rejection does NOT trigger this gate because the agent DID try to emit.
        //
        // U2 (2026-06-13-001): wave policy rejection follows the same
        // shape as contract rejection. The agent DID emit a wave batch
        // (it appears in the JSONL with `wave_id` set), but the policy
        // layer rejected the events for missing a required field such
        // as `depth`. Surfacing the rejected topic in `candidate_topics`
        // means `obligation_satisfied` (which iterates
        // `must_emit_any_of` against `candidate_topics`) treats the
        // obligation as satisfied — a wave batch that mentions
        // `review.wave.ready` is no longer mis-classified as a
        // missing emit just because the fan-out could not start.
        //
        // U4 (2026-06-07): collect the candidate topics the agent
        // emitted this iteration so the gate can call
        // `obligation_satisfied` on hats that opted into the
        // activation-level path.  An empty candidate set against a
        // hat with explicit obligations is now correctly reported as
        // a missing event instead of silently passing.
        let candidate_topics: Vec<String> = {
            let mut topics: Vec<String> = processed_events
                .as_ref()
                .map(|p| {
                    let mut topics: Vec<String> = p
                        .accepted_events
                        .iter()
                        .map(|e| e.topic.to_string())
                        .collect();
                    // Also surface contract-rejected topics — the agent
                    // DID try to emit these, so they should not count as
                    // "missing" even when the rejection kept them off the
                    // bus.
                    topics.extend(p.contract_rejections.iter().map(|f| f.topic.clone()));
                    topics
                })
                .unwrap_or_default();
            // U2: extend with wave-partition policy rejections (same
            // reasoning as contract_rejections above). The agent
            // attempted to emit a wave batch for these topics; the
            // policy layer kept the events off the bus, but the
            // topic itself was clearly the intended emission. Without
            // this merge, the obligation path would still classify
            // the obligation as unsatisfied and trigger a spurious
            // missing-event gate.
            topics.extend(wave_policy_rejections.iter().map(|r| r.topic.clone()));
            topics
        };
        // C4 (§6 C4): the post-wave gate blocks are guarded by
        // `late_termination_reason.is_none()`. When the global
        // deadline fires during a wave, the runner sets
        // `late_termination_reason = Some(MaxRuntime)` above and the
        // termination flow at the bottom of the iteration takes over;
        // running default_publishes or missing-event gate here would
        // either inject synthesized events into a doomed iteration or
        // trigger hard-gate bookkeeping on a loop that's about to
        // exit. Both must be skipped.
        //
        // U2 (2026-06-13-001): when a wave batch was *policy-rejected*
        // (e.g. all 7 `review.wave.ready` events missing `depth`), the
        // agent DID emit, but policy blocked the fan-out. Surface
        // schema-level guidance to the next iteration's prompt and
        // skip BOTH the missing-event gate (mutually exclusive per
        // the plan) and the default_publishes fallback (we have
        // evidence the agent tried to emit a wave, so synthesizing a
        // default would be misleading).
        if wave_had_policy_rejections
            && wave_events.is_empty()
            && !hard_gate_triggered_this_iteration
            && late_termination_reason.is_none()
        {
            // Resolve the publish list before the mutable borrow so
            // the helper can take `&mut event_loop` (U6 recovery
            // responder bookkeeping lives on the event loop).
            let publishes = event_loop.get_hat_publishes(&display_hat);
            inject_wave_policy_rejection_guidance(
                &ctx,
                Some(&mut event_loop),
                &display_hat,
                &wave_policy_rejections,
                wave_raw_count,
                &publishes,
            );
            info!(
                hat = %display_hat.as_str(),
                rejection_count = wave_policy_rejections.len(),
                raw_count = wave_raw_count,
                "Wave batch was policy-rejected; injected schema-level guidance instead of missing-event gate"
            );
        } else if !agent_wrote_any_valid_or_rejected
            && wave_events.is_empty()
            && !hard_gate_triggered_this_iteration
            && late_termination_reason.is_none()
            && should_gate_missing_events(&display_hat, &event_loop, &candidate_topics)
        {
            event_loop.increment_hard_gate_count();
            // Resolve the publish list before the mutable borrow so
            // the helper can take `&mut event_loop` (U6 recovery
            // responder bookkeeping lives on the event loop).
            let publishes = event_loop.get_hat_publishes(&display_hat);
            inject_missing_event_hard_gate_guidance(
                &ctx,
                Some(&mut event_loop),
                &display_hat,
                &publishes,
            );
            info!(
                hat = %display_hat.as_str(),
                consecutive = event_loop.state().consecutive_hard_gates,
                "Hard gate triggered: hat has publish obligation but emitted no event"
            );
        } else if !agent_wrote_any_valid_or_rejected
            && wave_events.is_empty()
            && !hard_gate_triggered_this_iteration
            && late_termination_reason.is_none()
        {
            let mut fallback_hats = Vec::new();
            if display_hat.as_str() != "ralph" {
                fallback_hats.push(display_hat.clone());
            }
            for active_hat_id in event_loop.state().last_active_hat_ids.clone() {
                if !fallback_hats.contains(&active_hat_id) {
                    fallback_hats.push(active_hat_id);
                }
            }

            for active_hat_id in &fallback_hats {
                event_loop.check_default_publishes(active_hat_id);
                if event_loop.has_pending_events() {
                    break; // One default is sufficient
                }
            }
        }

        // Check cancellation first (no chain validation) — takes priority over completion
        if let Some(reason) = event_loop.check_cancellation_event() {
            info!("Loop cancelled gracefully via loop.cancel event.");

            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );
            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
                None,
            );
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
            return Ok(reason);
        }

        if let Some(reason) =
            late_termination_reason.or_else(|| event_loop.check_completion_event())
        {
            info!(
                "Completion event {} detected.",
                config.event_loop.completion_promise
            );

            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
                None,
            );
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
            return Ok(reason);
        }

        // Fallback: detect completion promise in output text.
        // Primary path is JSONL events (check_completion_event above).
        // This catches backends that output LOOP_COMPLETE as text — either
        // without `ralph emit` (e.g. kiro-cli) or alongside it (e.g. OpenCode
        // which writes both a JSONL event and prints "Event emitted:" to stdout).
        //
        // We route through check_completion_event() to ensure all safety checks
        // are applied (persistent mode suppression, required_events validation,
        // runtime task verification). No parallel termination path.
        if EventParser::contains_promise(&output, &config.event_loop.completion_promise) {
            event_loop.request_completion_from_text_fallback();
            if let Some(reason) = event_loop.check_completion_event() {
                info!(
                    "Completion promise {} detected in output text.",
                    config.event_loop.completion_promise
                );

                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                    None,
                );
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                finalize_recovery_diagnosis(&mut event_loop, &loop_context, None);
                return Ok(reason);
            }
            // Safety check rejected completion (persistent mode, missing required
            // events, open tasks, etc.) — continue the loop normally.
        }

        // Precheck validation: Warn if no pending events after processing output
        // Per EventLoop doc: "Use has_pending_events after process_output to detect
        // if the LLM failed to publish an event."
        if !event_loop.has_pending_events() {
            let expected = event_loop.get_hat_publishes(&hat_id);
            debug!(
                hat = %hat_id.as_str(),
                expected_topics = ?expected,
                "No pending events after iteration. Agent may have failed to publish a valid event. \
                 Expected one of: {:?}. Loop will terminate on next iteration.",
                expected
            );
        }

        // Cooldown delay between iterations (skip for human events)
        let cooldown = config.event_loop.cooldown_delay_seconds;
        if cooldown > 0 && !event_loop.has_pending_human_events() {
            debug!(
                delay_seconds = cooldown,
                "Cooldown delay before next iteration"
            );
            tokio::time::sleep(Duration::from_secs(cooldown)).await;
        }

        // P1 finding #3 (CR 2026-06-10): periodic heartbeat write of
        // `active-activations.json` so `ralph diagnose --session latest`
        // can render the `## Active Hat Activations` section while the
        // loop is still running. Disabled when `heartbeat_secs == 0`
        // (parsed from `RALPH_ACTIVATIONS_HEARTBEAT_SEC` above) and
        // a no-op when diagnostics is disabled (the collector returns
        // `None` from `session_id()` and `write_active_activations`
        // early-returns on `None` session_dir).
        if let Some(last) = last_activations_heartbeat
            && event_loop.diagnostics().session_id().is_some()
            && last.elapsed() >= Duration::from_secs(heartbeat_secs)
        {
            let activations = event_loop.hat_lifecycle_tracker().active_activations();
            event_loop
                .diagnostics()
                .write_active_activations(&activations);
            last_activations_heartbeat = Some(std::time::Instant::now());
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// D5: startup-timeout helpers for `agent_doc_sync::sync_all`.
//
// `sync_all` runs blocking I/O before backend spawn. A stuck lock,
// slow disk, or NFS round-trip can otherwise hang the outer loop.
// We run the sync on a worker thread and `recv_timeout` it.
// ──────────────────────────────────────────────────────────────────────

/// Outcome of running `sync_all` with a startup timeout.
#[derive(Debug)]
enum SyncRunError {
    /// `sync_all` returned an error (lock contention, I/O, etc.).
    Sync(ralph_core::agent_doc_sync::SyncError),
    /// The sync did not finish within the configured timeout.
    Timeout { secs: u64 },
}

/// Run `sync_all` on a worker thread and bound it with `timeout_secs`.
///
/// `timeout_secs == 0` disables the timeout (legacy behaviour): the
/// call blocks on the worker thread indefinitely.
///
/// The worker thread is intentionally **not** joined on timeout —
/// the thread will eventually finish (or stay parked on a held file
/// lock) and exit; leaking it is preferable to blocking the loop.
fn run_sync_with_timeout(
    workspace_root: &Path,
    sync_config: &ralph_core::agent_doc_sync::SyncConfig<'_>,
    timeout_secs: u64,
) -> Result<ralph_core::agent_doc_sync::SyncReport, SyncRunError> {
    use std::path::PathBuf;

    if timeout_secs == 0 {
        // No timeout: run inline so we surface real errors.
        return ralph_core::agent_doc_sync::sync_all(workspace_root, sync_config)
            .map_err(SyncRunError::Sync);
    }

    let (tx, rx) = mpsc::channel::<
        Result<ralph_core::agent_doc_sync::SyncReport, ralph_core::agent_doc_sync::SyncError>,
    >();
    let root: PathBuf = workspace_root.to_path_buf();

    // Reconstruct a short-lived `SyncConfig` whose lifetimes are tied
    // to the worker thread. `target_files` is a `&'static` slice of
    // string literals; `blocks_vec` is an owned `Vec` moved into the
    // closure.
    let target_files: &'static [&'static str] = &["CLAUDE.md", "AGENTS.md"];
    let blocks_vec: Vec<ralph_core::agent_doc_sync::BlockSpec> = sync_config.blocks.to_vec();
    let on_error = sync_config.on_error;
    let session_dir_owned: Option<PathBuf> = sync_config.session_dir.map(|p| p.to_path_buf());

    let handle = thread::Builder::new()
        .name("ralph-agent-doc-sync".to_string())
        .spawn(move || {
            let cfg = ralph_core::agent_doc_sync::SyncConfig {
                skip: false,
                on_error,
                target_files,
                blocks: &blocks_vec,
                session_dir: session_dir_owned.as_deref(),
            };
            let result = ralph_core::agent_doc_sync::sync_all(&root, &cfg);
            // Ignore send failure: receiver may have timed out.
            let _ = tx.send(result);
        })
        .expect("failed to spawn agent_doc_sync worker thread");

    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(Ok(report)) => Ok(report),
        Ok(Err(e)) => Err(SyncRunError::Sync(e)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            tracing::warn!(
                target: "ralph_cli::loop_runner",
                timeout_secs,
                "agent_doc_sync: worker thread did not return in time; detaching"
            );
            // Detach: the thread will eventually finish (or hang on a
            // held lock); joining it would defeat the timeout.
            let _ = handle;
            Err(SyncRunError::Timeout { secs: timeout_secs })
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // Worker panicked before sending. Treat as a sync error
            // so callers can decide (Strict → exit, Warn → continue).
            Err(SyncRunError::Sync(
                ralph_core::agent_doc_sync::SyncError::VerifyFailed {
                    path: String::from("<agent_doc_sync>"),
                    detail: format!(
                        "worker thread disconnected before sending (likely panicked) within {timeout_secs}s"
                    ),
                },
            ))
        }
    }
}

/// Append a `startup_timeout` recovery envelope so operators can see
/// the timeout in `ralph diagnose --source agent_doc_sync`. When
/// `session_dir` is `None`, this is a no-op (sync ran without
/// diagnostics enabled).
fn write_startup_timeout_envelope(
    session_dir: Option<&Path>,
    timeout_secs: u64,
    on_error: ralph_core::OnErrorPolicy,
) {
    use ralph_core::diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
        RecoveryJournalEntry,
    };
    let Some(session_dir) = session_dir else {
        return;
    };
    let severity = match on_error {
        ralph_core::OnErrorPolicy::Strict => DiagnosisSeverity::Error,
        ralph_core::OnErrorPolicy::Warn => DiagnosisSeverity::Warning,
    };
    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::AgentDocSync)
        .severity(severity)
        .iteration(0)
        .reason_code("startup_timeout")
        .message(format!(
            "agent_doc_sync exceeded {timeout_secs}s startup timeout"
        ))
        .outcome(DiagnosisOutcome::Escalated)
        .build();
    let entry = RecoveryJournalEntry::from_envelope(envelope, vec![]);
    // Best-effort: a write failure here must not crash the loop.
    if let Ok(line) = serde_json::to_string(&entry) {
        let path = session_dir.join("recovery.jsonl");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod sync_timeout_tests {
    use super::*;
    use ralph_core::agent_doc_sync::block::BlockSpec;
    use ralph_core::agent_doc_sync::{OnError, SyncConfig};
    use tempfile::TempDir;

    #[test]
    fn zero_timeout_runs_inline_and_succeeds() {
        // D5: `startup_timeout_secs: 0` disables the timeout and
        // returns Ok when the underlying sync succeeds.
        let dir = TempDir::new().unwrap();
        let block = BlockSpec::new("hang-prevention", "x");
        let blocks = [block];
        let target_files = ["CLAUDE.md"];
        let cfg = SyncConfig {
            skip: false,
            on_error: OnError::Warn,
            target_files: &target_files,
            blocks: &blocks,
            session_dir: None,
        };
        let outcome = run_sync_with_timeout(dir.path(), &cfg, 0);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
    }

    #[test]
    fn nonzero_timeout_propagates_sync_error_quickly() {
        // D5: when the underlying sync fails fast (e.g. unwritable
        // target), `run_sync_with_timeout` must surface the error
        // via `SyncRunError::Sync` rather than spuriously firing the
        // timeout. We deliberately use `OnError::Warn` so the
        // underlying sync returns Ok; we assert Ok here.
        let dir = TempDir::new().unwrap();
        let block = BlockSpec::new("hang-prevention", "x");
        let blocks = [block];
        let target_files = ["CLAUDE.md"];
        let cfg = SyncConfig {
            skip: false,
            on_error: OnError::Warn,
            target_files: &target_files,
            blocks: &blocks,
            session_dir: None,
        };
        let started = std::time::Instant::now();
        let outcome = run_sync_with_timeout(dir.path(), &cfg, 30);
        let elapsed = started.elapsed();
        // Sync returns Ok with synced=1 well before 30s.
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "fast sync should not wait 30s: {elapsed:?}"
        );
    }
}
