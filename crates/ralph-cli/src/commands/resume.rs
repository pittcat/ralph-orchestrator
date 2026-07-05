use crate::cli::{ColorMode, ConfigSource, HatsSource, Verbosity};
use crate::display::colors;
use crate::loop_runner;
use crate::preflight;
use anyhow::{Context, Result};
use clap::Parser;
use ralph_adapters::detect_backend;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// Arguments for the resume subcommand.
///
/// Per spec: "When loop terminates due to safeguard (not completion promise),
/// user can run `ralph resume` to restart reading existing scratchpad."
#[derive(Parser, Debug)]
pub struct ResumeArgs {
    /// Override max iterations (from current position)
    #[arg(long)]
    max_iterations: Option<u32>,

    /// Disable TUI observation mode (TUI is enabled by default)
    #[arg(long, conflicts_with = "autonomous")]
    pub no_tui: bool,

    /// Force autonomous mode
    #[arg(short, long, conflicts_with = "no_tui", conflicts_with = "rpc")]
    pub autonomous: bool,

    /// Run in RPC mode with JSON-lines protocol on stdin/stdout.
    #[arg(long, conflicts_with = "no_tui", conflicts_with = "autonomous")]
    pub rpc: bool,

    /// Idle timeout in seconds for TUI mode
    #[arg(long)]
    idle_timeout: Option<u32>,

    /// Enable verbose output (show tool results and session summary)
    #[arg(short = 'v', long, conflicts_with = "quiet")]
    verbose: bool,

    /// Suppress streaming output (for CI/scripting)
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    quiet: bool,

    /// Record session to JSONL file for replay testing
    #[arg(long, value_name = "FILE")]
    record_session: Option<PathBuf>,

    /// U6 of plan 2026-07-05-005 (R4): attach (or re-attach) the
    /// resumed loop to a plan file. Writes
    /// `.ralph/agent/.ralph-anchor.json` so the resume-path
    /// `inspect loop` call surfaces the plan even when
    /// `prompt_file` is still the default sentinel (the
    /// resume command does not rewrite `prompt_file` the way
    /// `ralph run --plan` does).
    #[arg(long, value_name = "FILE")]
    plan: Option<PathBuf>,
}

/// Resume a previously interrupted loop from existing scratchpad.
///
/// DEPRECATED: Use `ralph run --continue` instead.
///
/// Per spec: "When loop terminates due to safeguard (not completion promise),
/// user can run `ralph run --continue` to restart reading existing scratchpad,
/// continuing from where it left off."
pub async fn resume_command(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    verbose: bool,
    color_mode: ColorMode,
    args: ResumeArgs,
    prebuilt_diagnostics: Option<Arc<ralph_core::diagnostics::DiagnosticsCollector>>,
) -> Result<()> {
    // Show deprecation warning
    eprintln!(
        "{}warning:{} `ralph resume` is deprecated. Use `ralph run --continue` instead.",
        colors::YELLOW,
        colors::RESET
    );

    // Load split core + hats config
    let mut config = preflight::load_config_for_preflight(config_sources, hats_source).await?;

    // Check that scratchpad exists (required for resume)
    let scratchpad_path = std::path::Path::new(&config.core.scratchpad.path);
    if !scratchpad_path.exists() {
        anyhow::bail!(
            "Cannot continue: scratchpad not found at '{}'. \
             Start a fresh run with `ralph run`.",
            config.core.scratchpad.path
        );
    }

    info!(
        "Found existing scratchpad at '{}', continuing from previous state",
        config.core.scratchpad.path
    );

    // U6 of plan 2026-07-05-005 (R4): when `--plan` is passed,
    // write the anchor marker so the inspect command can find
    // the plan attachment without rewriting `prompt_file`.
    // Resume is the only path where this matters — `ralph run
    // --plan` rewrites `prompt_file` directly via
    // `commands/run.rs:859-863`.
    if let Some(plan_path) = args.plan.as_ref() {
        if let Err(e) = write_resume_anchor_marker(plan_path) {
            eprintln!(
                "{}warning:{} failed to write anchor marker: {e}",
                colors::YELLOW,
                colors::RESET
            );
        }
    }

    // Apply CLI overrides
    if let Some(max_iter) = args.max_iterations {
        config.event_loop.max_iterations = max_iter;
    }
    if verbose {
        config.verbose = true;
    }

    // Apply execution mode overrides
    // TUI is enabled by default (unless --no-tui is specified)
    if args.autonomous {
        config.cli.default_mode = "autonomous".to_string();
    } else if !args.no_tui {
        config.cli.default_mode = "interactive".to_string();
    }

    // Override idle timeout if specified
    if let Some(timeout) = args.idle_timeout {
        config.cli.idle_timeout_secs = timeout;
    }

    // Validate configuration
    let warnings = config
        .validate()
        .context("Configuration validation failed")?;
    for warning in &warnings {
        eprintln!("{warning}");
    }

    // Handle auto-detection if backend is "auto"
    if config.cli.backend == "auto" {
        let priority = config.get_agent_priority();
        let detected = detect_backend(&priority, |backend| {
            config.adapter_settings(backend).enabled
        });

        match detected {
            Ok(backend) => {
                info!("Auto-detected backend: {}", backend);
                config.cli.backend = backend;
            }
            Err(e) => {
                eprintln!("{e}");
                return Err(anyhow::Error::new(e));
            }
        }
    }

    // Run the orchestration loop in resume mode
    // The key difference: we publish task.resume instead of task.start,
    // signaling the planner to read the existing scratchpad
    // TUI is enabled by default (unless --no-tui, --autonomous, or --rpc is specified)
    let enable_tui = !args.no_tui && !args.autonomous && !args.rpc;
    let enable_rpc = args.rpc;
    let verbosity = Verbosity::resolve(verbose || args.verbose, args.quiet);
    let reason = loop_runner::run_loop_impl(
        config,
        color_mode,
        true,
        enable_tui,
        enable_rpc,
        verbosity,
        args.record_session,
        None,       // Deprecated resume command doesn't have loop_context
        Vec::new(), // Resume command doesn't support custom args
        None,       // Use config.features.auto_merge (deprecated command)
        None,       // Deprecated resume command doesn't support --loop-id
        false,      // warmup_only (resume uses normal flow)
        false,      // force_warmup (resume uses normal flow)
        prebuilt_diagnostics,
        false, // no_sync_agent_docs (resume uses config default)
        false, // source_is_builtin_embedded (resume re-resolves builtin via its own path)
        None,  // hats_source_label (resume re-resolves builtin via its own path)
    )
    .await
    .map_err(|e| {
        // Map `agent_doc_sync` strict-mode failures (and the typed
        // preset-lint error) to their explicit exit codes so CI / cron
        // callers can distinguish them from generic failures. Without
        // this branch, `?` would propagate a plain `anyhow::Error` and
        // the process would exit with 1 — swallowing the 78 / 2
        // contract the run command already preserves (see
        // `run_loop_result_exit_code` in commands/run.rs).
        if let Some(code) = crate::commands::run::run_loop_result_exit_code(&e) {
            std::process::exit(code);
        }
        e
    })?;
    let exit_code = reason.exit_code();

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// U6 of plan 2026-07-05-005 (R4): persist the resume-time
/// anchor marker so `ralph inspect loop` can surface the plan
/// attachment. The marker lives at
/// `<workspace>/.ralph/agent/.ralph-anchor.json` and carries the
/// same fields as [`crate::commands::inspect::AnchorMarker`]
/// (kept as the SSoT shape — this writer is a thin
/// serialisation helper).
///
/// U9 of plan 2026-07-05-005 (fix-plan §R13 / A6): the write
/// is atomic — same tmp + fsync + rename pattern as
/// `task_store::write_jsonl_atomic` so a concurrent
/// `ralph resume --plan` race never observes a half-written
/// marker. The tmp path lives in the same directory as the
/// target so the rename is a single-filesystem call.
fn write_resume_anchor_marker(plan_path: &std::path::Path) -> anyhow::Result<()> {
    use std::io::Write;

    use crate::commands::inspect::AnchorMarker;

    let workspace_root = std::env::current_dir()
        .context("resume: failed to resolve current_dir for marker write")?;
    let agent_dir = workspace_root.join(".ralph").join("agent");
    std::fs::create_dir_all(&agent_dir)
        .context("resume: failed to create .ralph/agent dir for marker")?;
    let plan_baseline_sha = ralph_core::plan_baseline::read_plan_baseline(&workspace_root, None);
    let marker = AnchorMarker {
        plan_path: plan_path.to_path_buf(),
        plan_name: plan_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string(),
        plan_baseline_sha,
        attached_at: Some(chrono::Utc::now().to_rfc3339()),
    };
    let json = serde_json::to_string_pretty(&marker)
        .context("resume: failed to serialise anchor marker")?;
    let path = agent_dir.join(".ralph-anchor.json");
    // Atomic write: tmp + fsync + rename (matches
    // task_store::write_jsonl_atomic). The tmp suffix lives in
    // the same directory so the rename is single-filesystem.
    let tmp_path = {
        let mut candidate = path.clone();
        let file_name = candidate
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".ralph-anchor.json".to_string());
        candidate.set_file_name(format!(".{file_name}.tmp"));
        candidate
    };
    {
        let mut file = std::fs::File::create(&tmp_path)
            .context("resume: failed to create anchor marker tmp file")?;
        file.write_all(json.as_bytes())
            .context("resume: failed to write anchor marker body")?;
        file.sync_all()
            .context("resume: failed to fsync anchor marker tmp file")?;
    }
    if let Err(e) = std::fs::rename(&tmp_path, &path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e).context("resume: failed to atomic-rename anchor marker");
    }
    info!(path = %path.display(), "U6: wrote resume anchor marker (atomic)");
    Ok(())
}
