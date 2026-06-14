use crate::cli::{ColorMode, ConfigSource, HatsSource, Verbosity, ensure_scratchpad_directory};
use crate::display::truncate;
use crate::loop_runner;
use crate::preflight;
use anyhow::{Context, Result};
use clap::Parser;
use ralph_adapters::detect_backend;
use ralph_core::{
    CheckStatus, LockError, LockGuard, LockMetadata, LockStatus, LoopContext, LoopEntry, LoopLock,
    LoopRegistry, PreflightReport, PreflightRunner, RalphConfig, TerminationReason,
    truncate_with_ellipsis,
    worktree::{WorktreeConfig, create_worktree, ensure_gitignore, remove_worktree},
};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex};
use tracing::{debug, info, warn};

/// Arguments for the run subcommand.
#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Inline prompt text (mutually exclusive with -P/--prompt-file)
    #[arg(short = 'p', long = "prompt", conflicts_with = "prompt_file")]
    pub prompt_text: Option<String>,

    /// Override backend from config (cli > config > auto-detect)
    #[arg(short = 'b', long = "backend", value_name = "BACKEND")]
    pub backend: Option<String>,

    /// Prompt file path (mutually exclusive with -p/--prompt)
    #[arg(short = 'P', long = "prompt-file", conflicts_with = "prompt_text")]
    pub prompt_file: Option<PathBuf>,

    /// Override max iterations
    #[arg(long)]
    pub max_iterations: Option<u32>,

    /// Override completion promise
    #[arg(long)]
    pub completion_promise: Option<String>,

    /// Dry run - show what would be executed without running
    #[arg(long)]
    pub dry_run: bool,

    /// Continue from existing scratchpad (resume interrupted loop).
    /// Use this when a previous run was interrupted and you want to
    /// continue from where it left off.
    #[arg(long = "continue")]
    pub continue_mode: bool,

    /// Explicit loop ID to use with --continue.
    /// Reuses tasks from the specified loop instead of generating a new ID.
    /// If omitted with --continue, reuses the existing current-loop-id marker.
    #[arg(long, requires = "continue_mode")]
    pub loop_id: Option<String>,

    // ─────────────────────────────────────────────────────────────────────────
    // Execution Mode Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Disable TUI observation mode (TUI is enabled by default)
    #[arg(long, conflicts_with = "autonomous")]
    pub no_tui: bool,

    /// Force autonomous mode (headless, non-interactive).
    /// Overrides default_mode from config.
    #[arg(short, long, conflicts_with = "no_tui", conflicts_with = "rpc")]
    pub autonomous: bool,

    /// Run in RPC mode with JSON-lines protocol on stdin/stdout.
    /// All output is valid JSON; input accepts RpcCommand messages.
    /// Use this for IDE integrations and machine-readable interfaces.
    #[arg(long, conflicts_with = "no_tui", conflicts_with = "autonomous")]
    pub rpc: bool,

    /// Use legacy in-process TUI mode instead of subprocess RPC mode.
    /// This is an escape hatch during the migration to subprocess TUI.
    #[arg(long, hide = true, conflicts_with = "rpc", conflicts_with = "no_tui")]
    pub legacy_tui: bool,

    /// Idle timeout in seconds for interactive mode (default: 30).
    /// Process is terminated after this many seconds of inactivity.
    /// Set to 0 to disable idle timeout.
    #[arg(long)]
    pub idle_timeout: Option<u32>,

    /// Watchdog timeout (seconds) for autonomous / RPC / worktree paths
    /// (`--no-tui`, `--rpc`, `--worktree`). Resets on every backend
    /// output byte; fires `IdleTimeout` and SIGTERMs the child when the
    /// backend is silent for the full duration. Default: inherit from
    /// `adapters.<backend>.timeout` (300s for most backends).
    /// Set to 0 to explicitly disable the autonomous watchdog.
    #[arg(long)]
    pub autonomous_idle_timeout: Option<u64>,

    // ─────────────────────────────────────────────────────────────────────────
    // Multi-Loop Concurrency Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Wait for the primary loop slot instead of spawning into a worktree.
    /// Use this when you want to ensure only one loop runs at a time.
    #[arg(long)]
    pub exclusive: bool,

    /// Skip automatic merge after loop completes (keep worktree for manual handling).
    /// Only relevant for parallel loops running in worktrees.
    #[arg(long)]
    pub no_auto_merge: bool,

    /// Create an isolated git worktree for this run. The worktree is created
    /// at `.worktrees/<loop-id>/` and the loop runs inside it. Use this for
    /// fully isolated execution that does not affect the main working directory.
    ///
    /// End-to-end isolation contract: when set, the loop's `.ralph/` directory
    /// (events, diagnostics, current-events marker, etc.) is created inside the
    /// worktree, not the main repo. The main repo's working tree is untouched.
    /// The only exception is `.ralph/loops.json` (loop registry is shared
    /// across worktrees and lives in the main repo).
    ///
    /// Cannot be used with --exclusive.
    #[arg(long, conflicts_with = "exclusive")]
    pub worktree: bool,

    /// Internal: used by the parent process to pass an already-created worktree
    /// path to a child subprocess, so the child skips duplicate creation.
    /// Not intended for direct user use.
    #[arg(long, hide = true)]
    pub worktree_path: Option<PathBuf>,

    // ─────────────────────────────────────────────────────────────────────────
    // Phase Options (Warmup/Production Two-Phase Loop)
    // ─────────────────────────────────────────────────────────────────────────
    /// Exit loop after warmup completes (do not transition to production phase).
    /// Sets warmup_config.stop_on_exit: true in the configuration.
    #[arg(long)]
    pub warmup_only: bool,

    /// Force warmup phase even if phase.json indicates warmup was previously completed.
    /// Use this to re-run harness calibration.
    #[arg(long)]
    pub force_warmup: bool,

    // ─────────────────────────────────────────────────────────────────────────
    // Preflight Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Skip preflight checks before loop start.
    /// Overrides features.preflight.enabled from config.
    #[arg(long)]
    pub skip_preflight: bool,

    // ─────────────────────────────────────────────────────────────────────────
    // Agent Doc Sync Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Skip agent doc sync before loop start.
    /// Disables injection of managed agent doc blocks into CLAUDE.md / AGENTS.md.
    /// Equivalent to setting RALPH_AGENT_DOC_SYNC=0.
    #[arg(long)]
    pub no_sync_agent_docs: bool,

    // ─────────────────────────────────────────────────────────────────────────
    // Verbosity Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Enable verbose output (show tool results and session summary)
    #[arg(short = 'v', long, conflicts_with = "quiet")]
    pub verbose: bool,

    /// Suppress streaming output (for CI/scripting)
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Record session to JSONL file for replay testing
    #[arg(long, value_name = "FILE")]
    pub record_session: Option<PathBuf>,

    /// Custom backend command and arguments (use after --)
    #[arg(last = true)]
    pub custom_args: Vec<String>,
}

fn format_preflight_summary(report: &PreflightReport) -> String {
    let icons: Vec<String> = report
        .checks
        .iter()
        .map(|check| {
            let icon = match check.status {
                CheckStatus::Pass => "✓",
                CheckStatus::Warn => "⚠",
                CheckStatus::Fail => "✗",
            };
            format!("{icon} {}", check.name)
        })
        .collect();

    let summary = if icons.is_empty() {
        "no checks".to_string()
    } else {
        icons.join(" ")
    };

    let suffix = if report.failures > 0 {
        format!(
            " ({} failure{})",
            report.failures,
            if report.failures == 1 { "" } else { "s" }
        )
    } else if report.warnings > 0 {
        format!(
            " ({} warning{})",
            report.warnings,
            if report.warnings == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    };

    format!("{summary}{suffix}")
}

enum AutoPreflightMode {
    DryRun,
    Run,
}

fn preflight_failure_detail(report: &PreflightReport, strict: bool) -> String {
    if strict && report.warnings > 0 {
        format!(
            "{} failure{}, {} warning{}",
            report.failures,
            if report.failures == 1 { "" } else { "s" },
            report.warnings,
            if report.warnings == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "{} failure{}",
            report.failures,
            if report.failures == 1 { "" } else { "s" }
        )
    }
}

async fn run_auto_preflight(
    config: &RalphConfig,
    skip_preflight: bool,
    verbose: bool,
    mode: AutoPreflightMode,
) -> Result<Option<PreflightReport>> {
    if skip_preflight || !config.features.preflight.enabled {
        return Ok(None);
    }

    let runner = PreflightRunner::default_checks_with_config(config);
    let mut report = if config.features.preflight.skip.is_empty() {
        runner.run_all(config).await
    } else {
        let skip_lower: std::collections::HashSet<String> = config
            .features
            .preflight
            .skip
            .iter()
            .map(|name| name.to_lowercase())
            .collect();
        let selected: Vec<String> = runner
            .check_names()
            .into_iter()
            .filter(|name| !skip_lower.contains(&name.to_lowercase()))
            .map(|name| name.to_string())
            .collect();
        runner.run_selected(config, &selected).await
    };

    let effective_passed = if config.features.preflight.strict {
        report.failures == 0 && report.warnings == 0
    } else {
        report.failures == 0
    };
    report.passed = effective_passed;

    match mode {
        AutoPreflightMode::DryRun => Ok(Some(report)),
        AutoPreflightMode::Run => {
            print_preflight_summary(&report, verbose, "Preflight: ", false);
            if !effective_passed {
                let detail = preflight_failure_detail(&report, config.features.preflight.strict);
                anyhow::bail!(
                    "Preflight checks failed ({}). Fix the issues above or use --skip-preflight to bypass.",
                    detail
                );
            }
            Ok(None)
        }
    }
}

fn print_preflight_summary(
    report: &PreflightReport,
    verbose: bool,
    prefix: &str,
    use_stdout: bool,
) {
    let summary = format_preflight_summary(report);
    if use_stdout {
        println!("{prefix}{summary}");
    } else {
        eprintln!("{prefix}{summary}");
    }

    let emit = |line: String| {
        if use_stdout {
            println!("{line}");
        } else {
            eprintln!("{line}");
        }
    };

    for check in &report.checks {
        if check.status == CheckStatus::Fail
            && let Some(message) = &check.message
        {
            emit(format!("  ✗ {}: {}", check.name, message));
        }
    }

    if verbose {
        for check in &report.checks {
            if check.status == CheckStatus::Warn
                && let Some(message) = &check.message
            {
                emit(format!("  ⚠ {}: {}", check.name, message));
            }
        }
    }
}

/// U2 (2026-06-14-002): sync default PROMPT.md to worktree root.
/// Copies PROMPT.md from main repo to worktree so Agent reads from worktree.
/// Silently skips if source doesn't exist or copy fails (doesn't block startup).
fn sync_prompt_to_worktree(repo_root: &Path, worktree_path: &Path) {
    let prompt_in_repo = repo_root.join("PROMPT.md");
    if !prompt_in_repo.exists() {
        debug!("No PROMPT.md in main repo, skipping sync to worktree");
        return;
    }
    let prompt_in_wt = worktree_path.join("PROMPT.md");
    if prompt_in_wt.exists() {
        debug!("PROMPT.md already exists in worktree, skipping sync");
        return;
    }
    match std::fs::copy(&prompt_in_repo, &prompt_in_wt) {
        Ok(bytes) => info!(
            "Synced PROMPT.md ({} bytes) to worktree: {}",
            bytes,
            prompt_in_wt.display()
        ),
        Err(e) => warn!(
            "Failed to copy PROMPT.md to worktree: {} (continuing without sync)",
            e
        ),
    }
}

/// Spawn a new loop in a git worktree.
///
/// This extracts the worktree creation logic from `handle_active_lock` so it can
/// also be used by the explicit `--worktree` flag path in `run_command`.
fn spawn_worktree_loop(
    workspace_root: &Path,
    prompt_summary: &str,
    file_name_prefix: Option<&str>,
    loop_naming: &ralph_core::LoopNamingConfig,
    pending_worktree_registration: &mut Option<LoopEntry>,
) -> Result<(LoopContext, Option<LockGuard>), anyhow::Error> {
    let worktree_config = WorktreeConfig::default();

    // Generate loop ID from the most identifiable source + unique suffix.
    // Prompt files use their file name so worktrees can be mapped back to plans.
    let name_generator = ralph_core::LoopNameGenerator::from_config(loop_naming);
    let loop_id = if let Some(prefix) = file_name_prefix {
        name_generator.generate_unique_with_prefix(prefix, |name| {
            ralph_core::worktree_exists(workspace_root, name, &worktree_config)
        })
    } else {
        name_generator.generate_unique(prompt_summary, |name| {
            ralph_core::worktree_exists(workspace_root, name, &worktree_config)
        })
    };

    // Ensure worktree directory is in .gitignore
    ensure_gitignore(workspace_root, ".worktrees")
        .context("Failed to update .gitignore for worktrees")?;

    // Create the worktree
    let worktree = create_worktree(workspace_root, &loop_id, &worktree_config)
        .context("Failed to create worktree for loop")?;

    info!(
        "Created worktree at {} on branch {}",
        worktree.path.display(),
        worktree.branch
    );

    // Create loop context for the worktree
    let context = LoopContext::worktree(
        loop_id.clone(),
        worktree.path.clone(),
        workspace_root.to_path_buf(),
    );

    // Set up all worktree symlinks (memories, specs, code tasks)
    context
        .setup_worktree_symlinks()
        .context("Failed to create symlinks in worktree")?;

    // U2 (2026-06-14-002): sync default PROMPT.md to worktree root.
    // This ensures the Agent reads its prompt from the worktree (relative path),
    // avoiding context anchoring to the main repo.
    sync_prompt_to_worktree(workspace_root, &worktree.path);

    // Generate context file with worktree metadata
    context
        .generate_context_file(&worktree.branch, prompt_summary)
        .context("Failed to generate context file in worktree")?;

    // Register this loop after preflight succeeds so failed runs
    // don't leave stale registry entries behind.
    let entry = LoopEntry::with_id(
        &loop_id,
        prompt_summary,
        Some(worktree.path.to_string_lossy().to_string()),
        worktree.path.to_string_lossy().to_string(),
    );
    *pending_worktree_registration = Some(entry);

    Ok((context, None))
}

/// Handle the case where another process holds the active loop lock.
///
/// Implements the existing behavior for active locks: --exclusive waits,
/// parallel disabled errors out, otherwise spawn into a worktree.
fn handle_active_lock(
    existing: LockMetadata,
    workspace_root: &Path,
    prompt_summary: &str,
    file_name_prefix: Option<&str>,
    exclusive: bool,
    parallel: bool,
    loop_naming: &ralph_core::LoopNamingConfig,
    pending_worktree_registration: &mut Option<LoopEntry>,
) -> Result<(LoopContext, Option<LockGuard>), anyhow::Error> {
    if exclusive {
        // --exclusive: wait for the lock instead of spawning worktree
        info!(
            "Loop lock held by PID {} (started {}), waiting for lock (--exclusive mode)...",
            existing.pid, existing.started
        );
        let guard = LoopLock::acquire_blocking(workspace_root, prompt_summary)
            .context("Failed to acquire loop lock in exclusive mode")?;
        debug!("Acquired loop lock after waiting");
        let context = LoopContext::primary(workspace_root.to_path_buf());
        Ok((context, Some(guard)))
    } else if !parallel {
        // Parallel loops disabled via config - error out
        anyhow::bail!(
            "Another loop is already running (PID {}, prompt: \"{}\"). \
            Parallel loops are disabled in config (features.parallel: false). \
            Use --exclusive to wait for the lock, or enable parallel loops.",
            existing.pid,
            existing.prompt.chars().take(50).collect::<String>()
        )
    } else {
        // Auto-spawn into worktree
        info!(
            "Loop lock held by PID {} ({}), spawning parallel loop in worktree",
            existing.pid,
            existing.prompt.chars().take(50).collect::<String>()
        );
        spawn_worktree_loop(
            workspace_root,
            prompt_summary,
            file_name_prefix,
            loop_naming,
            pending_worktree_registration,
        )
    }
}

fn worktree_file_name_prefix(
    prompt_file: &str,
    prompt_summary: &str,
    workspace_root: &Path,
) -> Option<String> {
    if let Some(stem) = plan_file_stem_from_text(prompt_summary) {
        return Some(stem);
    }

    if prompt_file.is_empty() {
        return None;
    }

    let prompt_path = Path::new(prompt_file);
    let prompt_path = if prompt_path.is_absolute() {
        prompt_path.to_path_buf()
    } else {
        workspace_root.join(prompt_path)
    };

    if !prompt_path.exists() {
        return None;
    }

    if let Ok(prompt_content) = std::fs::read_to_string(&prompt_path)
        && let Some(stem) = plan_file_stem_from_text(&prompt_content)
    {
        return Some(stem);
    }

    prompt_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .filter(|stem| stem.to_ascii_lowercase() != "prompt")
        .map(std::string::ToString::to_string)
}

fn plan_file_stem_from_text(text: &str) -> Option<String> {
    text.split_whitespace()
        .filter_map(plan_path_candidate)
        .find_map(|path| {
            Path::new(&path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|stem| !stem.trim().is_empty())
                .map(std::string::ToString::to_string)
        })
}

fn plan_path_candidate(token: &str) -> Option<String> {
    let token = token.trim_matches(|c: char| {
        c.is_ascii_punctuation() && c != '/' && c != '.' && c != '-' && c != '_' && c != ':'
    });
    let candidate = token
        .find("plan:")
        .map(|idx| &token[idx + "plan:".len()..])
        .unwrap_or(token)
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == ')' || c == ']');

    let lower = candidate.to_ascii_lowercase();
    if (lower.ends_with(".md") || lower.ends_with(".html")) && candidate.contains('/') {
        Some(candidate.to_string())
    } else {
        None
    }
}

pub async fn run_command(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    verbose: bool,
    color_mode: ColorMode,
    args: RunArgs,
    prebuilt_diagnostics: Option<Arc<ralph_core::diagnostics::DiagnosticsCollector>>,
) -> Result<()> {
    let mut config = preflight::load_config_for_preflight(config_sources, hats_source).await?;

    // Handle --continue mode: check scratchpad exists before proceeding
    let resume = args.continue_mode;
    if resume {
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
    }

    // Capture args for subprocess TUI mode BEFORE fields are consumed below.
    // `let mut` is required so the worktree branch (below) can rewrite
    // `worktree=false` + `worktree_path=Some(...)` after spawn_worktree_loop
    // returns. P1-F fix on 2026-06-10.
    let mut subprocess_tui_args = SubprocessTuiArgs::new(&args, config_sources, hats_source);

    // Apply CLI overrides (after normalization so they take final precedence)
    // Per spec: CLI -p and -P are mutually exclusive (enforced by clap)
    if let Some(text) = args.prompt_text {
        config.event_loop.prompt = Some(text);
        config.event_loop.prompt_file = String::new(); // Clear file path
    } else if let Some(path) = args.prompt_file {
        config.event_loop.prompt_file = path.to_string_lossy().to_string();
        config.event_loop.prompt = None; // Clear inline
    }
    if let Some(max_iter) = args.max_iterations {
        config.event_loop.max_iterations = max_iter;
    }
    if let Some(promise) = args.completion_promise {
        config.event_loop.completion_promise = promise;
    }
    if verbose {
        config.verbose = true;
    }

    // Apply execution mode overrides per spec
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

    // Override autonomous watchdog if specified. Takes precedence over the
    // per-adapter `adapters.<backend>.timeout` default (see plan
    // 2026-06-06-001, R5/R6).
    if let Some(timeout) = args.autonomous_idle_timeout {
        config.cli.autonomous_idle_timeout_secs = Some(timeout);
    }

    // Apply backend override from CLI (takes precedence over config)
    if let Some(backend) = args.backend {
        config.cli.backend = backend;
    }

    // Validate configuration and emit warnings
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

    let preflight_verbose = verbose || args.verbose;

    if args.dry_run {
        let preflight_report = run_auto_preflight(
            &config,
            args.skip_preflight,
            preflight_verbose,
            AutoPreflightMode::DryRun,
        )
        .await?;
        println!("Dry run mode - configuration:");
        println!(
            "  Hats: {}",
            if config.hats.is_empty() {
                "planner, builder (default)".to_string()
            } else {
                config.hats.keys().cloned().collect::<Vec<_>>().join(", ")
            }
        );

        // Show prompt source
        if let Some(ref inline) = config.event_loop.prompt {
            let preview = truncate_with_ellipsis(&inline.replace('\n', " "), 60);
            println!("  Prompt: inline text ({})", preview);
        } else {
            println!("  Prompt file: {}", config.event_loop.prompt_file);
        }

        println!(
            "  Completion promise: {}",
            config.event_loop.completion_promise
        );
        println!("  Max iterations: {}", config.event_loop.max_iterations);
        println!("  Max runtime: {}s", config.event_loop.max_runtime_seconds);
        println!(
            "  Scratchpad: {} (enabled: {})",
            config.core.scratchpad.path, config.core.scratchpad.enabled
        );
        println!("  Specs dir: {}", config.core.specs_dir);
        println!("  Backend: {}", config.cli.backend);
        println!("  Verbose: {}", config.verbose);
        // Execution mode info
        println!("  Default mode: {}", config.cli.default_mode);
        if config.cli.default_mode == "interactive" {
            println!("  Idle timeout: {}s", config.cli.idle_timeout_secs);
        } else {
            // Autonomous / RPC / worktree path: the inactivity watchdog comes
            // from cli.autonomous_idle_timeout_secs (override) or
            // adapters.<backend>.timeout (default 300s). Print it so operators
            // can confirm the watchdog is wired up and is not the old broken
            // "always disabled" behavior.
            let autonomous_timeout = config.autonomous_idle_timeout_secs(&config.cli.backend);
            println!(
                "  Autonomous watchdog: {}s (0 = disabled)",
                autonomous_timeout
            );
        }
        if !warnings.is_empty() {
            println!("  Warnings: {}", warnings.len());
        }
        if let Some(report) = preflight_report.as_ref() {
            print_preflight_summary(report, preflight_verbose, "  Preflight: ", true);
        }
        return Ok(());
    }

    // Ensure scratchpad directory exists (auto-create with depth limit)
    // This is done after dry-run check to avoid creating directories during dry-run
    ensure_scratchpad_directory(&config)?;

    // Get the prompt for lock metadata (short version for display)
    // When prompt_file is used, read its content for the summary instead of showing the file path
    let prompt_summary = config
        .event_loop
        .prompt
        .clone()
        .or_else(|| {
            let prompt_file = &config.event_loop.prompt_file;
            if prompt_file.is_empty() {
                None
            } else {
                let path = std::path::Path::new(prompt_file);
                if path.exists() {
                    std::fs::read_to_string(path).ok()
                } else {
                    None
                }
            }
        })
        .map(|p| truncate(&p, 100))
        .unwrap_or_else(|| "[no prompt]".to_string());
    let workspace_root = &config.core.workspace_root;
    let worktree_file_name_prefix = worktree_file_name_prefix(
        &config.event_loop.prompt_file,
        &prompt_summary,
        workspace_root,
    );

    let mut pending_worktree_registration: Option<LoopEntry> = None;

    // Determine TUI mode early (before lock acquisition) to avoid self-lock contention
    // in subprocess TUI mode. The child RPC process will acquire the lock itself.
    let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let use_subprocess_tui =
        !args.no_tui && !args.autonomous && !args.rpc && !args.legacy_tui && is_tty;

    // Try to acquire the loop lock for multi-loop concurrency support
    // This implements the lock detection flow from the multi-loop spec
    // Skip lock acquisition in --worktree mode - create worktree directly without needing the lock
    // Skip lock acquisition in subprocess TUI mode - let the child acquire it
    //
    // U1 (2026-06-10): `args.worktree` takes PRIORITY over `use_subprocess_tui`.
    // When both are true (TTY + --worktree flag), we MUST create the worktree
    // first so parent gets LoopContext::worktree and passes --worktree-path to child.
    // The old order (use_subprocess_tui first) caused parent to use primary context
    // and skip worktree creation entirely - child then created a second worktree
    // in the main repo, defeating the isolation guarantee.
    let (loop_context, _lock_guard) = if args.worktree {
        // Explicit --worktree flag: create worktree directly without acquiring lock
        // Worktree mode does not hold .ralph/loop.lock - it's fully isolated
        debug!("Creating worktree for explicit --worktree mode");
        let (wt_ctx, _wt_guard) = spawn_worktree_loop(
            workspace_root,
            &prompt_summary,
            worktree_file_name_prefix.as_deref(),
            &config.features.loop_naming,
            &mut pending_worktree_registration,
        )?;
        // P1-F (2026-06-10): when the parent itself creates the worktree
        // and then spawns a child RPC, the child MUST NOT receive
        // `--worktree` (it would create a *second* worktree inside the
        // parent's). Pass the worktree path instead so the child
        // chdir's into the parent's worktree and runs there. Without
        // this, the child sees `--worktree`, hits the same `args.worktree`
        // branch, and spawns a nested worktree — the parent loop_context
        // points at the first worktree (orphaned), the child runs in the
        // second (different path), and LoopRegistry registers the first
        // (inconsistent). See findings-correctness-task-*.json for the
        // full causal chain.
        subprocess_tui_args.worktree = false;
        subprocess_tui_args.worktree_path = Some(wt_ctx.workspace().to_path_buf());
        (wt_ctx, None)
    } else if args.worktree_path.is_some() {
        // U2 (2026-06-10): child received --worktree-path from parent: the worktree
        // was already created by the parent, so we skip spawn_worktree_loop and
        // use the path directly. This is the child's side of the fix — the parent
        // creates the worktree once, passes the path via --worktree-path, and the
        // child uses it without re-creating.
        let worktree_path = args.worktree_path.as_ref().unwrap();
        // Defensive: validate the path exists. The parent should never pass
        // an invalid path, but if it does, fail fast with a clear error
        // instead of writing events into a phantom directory.
        if !worktree_path.exists() {
            return Err(anyhow::anyhow!(
                "--worktree-path '{}' does not exist. The parent process must \
                 create the worktree before passing it to the child.",
                worktree_path.display()
            ));
        }
        let loop_id = worktree_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        debug!(
            "Child using existing worktree at '{}' (loop_id={})",
            worktree_path.display(),
            loop_id
        );
        let context = LoopContext::worktree(loop_id, worktree_path.clone(), workspace_root.clone());
        (context, None)
    } else if use_subprocess_tui {
        // In subprocess TUI mode, don't acquire lock here - the child RPC process will do it
        // This avoids the self-lock contention where parent holds lock and child sees it,
        // then incorrectly spawns a worktree thinking there's another concurrent loop
        debug!("Skipping lock acquisition in subprocess TUI mode (child will acquire)");
        let context = LoopContext::primary(workspace_root.clone());
        (context, None)
    } else {
        match LoopLock::inspect(workspace_root) {
            Ok(LockStatus::None) => {
                // No lock file - try to acquire normally
                match LoopLock::try_acquire(workspace_root, &prompt_summary) {
                    Ok(guard) => {
                        debug!("Acquired loop lock, running as primary loop");
                        let context = LoopContext::primary(workspace_root.clone());
                        (context, Some(guard))
                    }
                    Err(LockError::AlreadyLocked(existing)) => {
                        // Race: lock became active between inspect and try_acquire
                        handle_active_lock(
                            existing,
                            workspace_root,
                            &prompt_summary,
                            worktree_file_name_prefix.as_deref(),
                            args.exclusive,
                            config.features.parallel,
                            &config.features.loop_naming,
                            &mut pending_worktree_registration,
                        )?
                    }
                    Err(LockError::UnsupportedPlatform) => {
                        warn!("Loop locking not supported on this platform, running without lock");
                        let context = LoopContext::primary(workspace_root.clone());
                        (context, None)
                    }
                    Err(e) => {
                        return Err(anyhow::Error::new(e).context("Failed to acquire loop lock"));
                    }
                }
            }
            Ok(LockStatus::Stale(metadata)) => {
                // Stale lock from a previous crash or abnormal termination
                info!(
                    "Detected stale lock from PID {} (started {}). Cleaning up and continuing...",
                    metadata.pid, metadata.started
                );
                let lock_path = workspace_root.join(LoopLock::LOCK_FILE);
                let _ = std::fs::remove_file(&lock_path);
                match LoopLock::try_acquire(workspace_root, &prompt_summary) {
                    Ok(guard) => {
                        debug!("Acquired loop lock after stale cleanup");
                        let context = LoopContext::primary(workspace_root.clone());
                        (context, Some(guard))
                    }
                    Err(LockError::AlreadyLocked(existing)) => {
                        // Race: another process acquired it while we were cleaning up
                        handle_active_lock(
                            existing,
                            workspace_root,
                            &prompt_summary,
                            worktree_file_name_prefix.as_deref(),
                            args.exclusive,
                            config.features.parallel,
                            &config.features.loop_naming,
                            &mut pending_worktree_registration,
                        )?
                    }
                    Err(LockError::UnsupportedPlatform) => {
                        warn!("Loop locking not supported on this platform, running without lock");
                        let context = LoopContext::primary(workspace_root.clone());
                        (context, None)
                    }
                    Err(e) => {
                        return Err(anyhow::Error::new(e)
                            .context("Failed to acquire loop lock after stale cleanup"));
                    }
                }
            }
            Ok(LockStatus::Active(metadata)) => {
                // Active lock held by another process
                handle_active_lock(
                    metadata,
                    workspace_root,
                    &prompt_summary,
                    worktree_file_name_prefix.as_deref(),
                    args.exclusive,
                    config.features.parallel,
                    &config.features.loop_naming,
                    &mut pending_worktree_registration,
                )?
            }
            Err(e) => {
                warn!(
                    "Lock inspection failed: {}. Falling back to direct acquisition.",
                    e
                );
                match LoopLock::try_acquire(workspace_root, &prompt_summary) {
                    Ok(guard) => {
                        debug!("Acquired loop lock, running as primary loop");
                        let context = LoopContext::primary(workspace_root.clone());
                        (context, Some(guard))
                    }
                    Err(LockError::AlreadyLocked(existing)) => handle_active_lock(
                        existing,
                        workspace_root,
                        &prompt_summary,
                        worktree_file_name_prefix.as_deref(),
                        args.exclusive,
                        config.features.parallel,
                        &config.features.loop_naming,
                        &mut pending_worktree_registration,
                    )?,
                    Err(LockError::UnsupportedPlatform) => {
                        warn!("Loop locking not supported on this platform, running without lock");
                        let context = LoopContext::primary(workspace_root.clone());
                        (context, None)
                    }
                    Err(e) => {
                        return Err(anyhow::Error::new(e).context("Failed to acquire loop lock"));
                    }
                }
            }
        }
    };

    // U3 (2026-06-10): workspace is now set from loop_context.workspace()
    // after loop_context is determined. This ensures workspace is always
    // correct regardless of which branch was taken.
    subprocess_tui_args.workspace = loop_context.workspace().to_path_buf();

    // Update workspace_root in config if running in worktree
    if !loop_context.is_primary() {
        config.core.workspace_root = loop_context.workspace().to_path_buf();
        // Also update scratchpad path to use worktree location
        config.core.scratchpad.path = loop_context.scratchpad_path().to_string_lossy().to_string();
        debug!(
            "Running in worktree: workspace={}, scratchpad={}",
            config.core.workspace_root.display(),
            config.core.scratchpad.path
        );
    }

    // Ensure directories exist in the loop context
    loop_context
        .ensure_directories()
        .context("Failed to create loop directories")?;

    if let Err(err) = run_auto_preflight(
        &config,
        args.skip_preflight,
        preflight_verbose,
        AutoPreflightMode::Run,
    )
    .await
    {
        if !loop_context.is_primary()
            && let Err(clean_err) =
                remove_worktree(loop_context.repo_root(), loop_context.workspace())
        {
            warn!(
                "Preflight failed; unable to remove worktree {}: {}",
                loop_context.workspace().display(),
                clean_err
            );
        }
        return Err(err);
    }

    if let Some(entry) = pending_worktree_registration {
        let registry = LoopRegistry::new(loop_context.repo_root());
        registry
            .register(entry)
            .context("Failed to register loop in registry")?;
    }

    // Run the orchestration loop and exit with proper exit code
    // TUI is enabled by default (unless --no-tui, --autonomous, or --rpc is specified)
    let wants_tui = !args.no_tui && !args.autonomous && !args.rpc;
    let use_legacy_tui = args.legacy_tui;
    let enable_rpc = args.rpc;
    let verbosity = Verbosity::resolve(verbose || args.verbose, args.quiet);
    let custom_args = args.custom_args.clone();
    // --no-auto-merge and --worktree both disable auto-merge
    let auto_merge_override = if args.no_auto_merge || args.worktree {
        Some(false)
    } else {
        None
    };
    let workspace_root = config.core.workspace_root.clone();

    // Determine TUI mode:
    // 1. Subprocess TUI (default): TUI spawns `ralph run --rpc` as child, reads JSON events
    // 2. Legacy TUI: In-process TUI (--legacy-tui escape hatch)
    // 3. RPC mode: Headless JSON-lines output (--rpc)
    // 4. CLI mode: No TUI (--no-tui or --autonomous)
    // Note: use_subprocess_tui is now determined earlier (before lock acquisition)
    let reason = if use_subprocess_tui {
        // Subprocess TUI mode: spawn child with --rpc and attach TUI
        run_subprocess_tui(subprocess_tui_args, resume, custom_args).await?
    } else {
        // In-process mode: run_loop_impl handles everything
        let enable_tui = wants_tui && use_legacy_tui;
        // WRC-U3: surface builtin detection to the lint gate so
        // the WAC severity upgrade (KTD-7) fires for `ralph run
        // -H builtin:foo`. `HatsSource::Builtin(_)` is the
        // canonical marker from the CLI parser.
        let source_is_builtin_embedded = matches!(
            hats_source,
            Some(crate::cli::shared::HatsSource::Builtin(_))
        );
        loop_runner::run_loop_impl(
            config,
            color_mode,
            resume,
            enable_tui,
            enable_rpc,
            verbosity,
            args.record_session,
            Some(loop_context),
            custom_args,
            auto_merge_override,
            args.loop_id,
            args.warmup_only,
            args.force_warmup,
            prebuilt_diagnostics,
            args.no_sync_agent_docs,
            source_is_builtin_embedded,
        )
        .await
        .map_err(|e| {
            // P1 finding #5: lift `PresetLintGateError` out of
            // the error chain and exit with code2 here, AFTER
            // the RAII drop chain has run. The `?` shortcut
            // would still propagate, but the caller (`main`)
            // has no way to map the typed error to a specific
            // exit code without us flagging it here. Doing the
            // mapping inside `run_command` keeps the change
            // scoped to the run command path without
            // restructuring `main`.
            if let Some(code) = run_loop_result_exit_code(&e) {
                std::process::exit(code);
            }
            e
        })?
    };

    // Handle restart: run required single-command restart sequence.
    if matches!(reason, TerminationReason::RestartRequested) {
        clear_restart_request_signal(&workspace_root);

        #[cfg(unix)]
        {
            let restart_cmd = required_restart_command(std::process::id());
            info!(
                "Restart requested — launching single-command restart: {}",
                restart_cmd
            );

            std::process::Command::new("sh")
                .arg("-lc")
                .arg(&restart_cmd)
                .spawn()
                .with_context(|| format!("Failed to spawn restart command: {}", restart_cmd))?;

            // Shell command takes over restarting this loop after kill.
            return Ok(());
        }

        #[cfg(not(unix))]
        {
            anyhow::bail!("Restart via single-command shell restart is only supported on Unix");
        }
    }

    let exit_code = parent_exit_code_for_reason(&reason);

    // U3: use explicit exit for non-zero codes so the parent process does not
    // hang or return 0 after a non-clean loop termination.
    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

fn required_restart_command(pid: u32) -> String {
    format!("kill {pid} && RALPH_DIAGNOSTICS=1 cargo run --bin ralph -- resume -c ralph.test.yml")
}

fn clear_restart_request_signal(workspace_root: &std::path::Path) {
    let restart_path = workspace_root.join(".ralph/restart-requested");
    let _ = std::fs::remove_file(&restart_path);
}

// P1 finding #5: lift the preset-lint error into a typed error chain so we
// can map it to exit code2 *after* the RAII drop chain runs. The inner
// `run_loop_impl` returns `anyhow::Error::new(PresetLintGateError)` instead
// of calling `std::process::exit` directly.
//
// The user-facing stderr message already came from `enforce_preset_lint_gate`
// (via the `Display` impl on `PresetLintGateError`), and the JSON artifact was
// already written to `.ralph/diagnostics/preset-lint-error-*.json`. This
// function only decides the *exit code*, not the operator-facing
// narrative.
pub(crate) fn run_loop_result_exit_code(err: &anyhow::Error) -> Option<i32> {
    for cause in err.chain() {
        if cause.is::<loop_runner::PresetLintGateError>() {
            return Some(loop_runner::EXIT_CODE_LINT_GATE);
        }
    }
    for cause in err.chain() {
        if cause
            .to_string()
            .contains("agent_doc_sync failed in strict mode")
        {
            return Some(loop_runner::EXIT_CODE_AGENT_DOC_SYNC_STRICT);
        }
    }
    None
}

/// U3: exit code used by the `ralph run` parent process for a loop
/// termination reason. Distinct from [`TerminationReason::exit_code`] so the
/// parent can document stable, reason-specific codes without changing the
/// broader event-loop semantics.
///
/// Documented exit codes:
/// - 0: clean completion (`CompletionPromise`, `Cancelled`)
/// - 1: generic failure (`ConsecutiveFailures`, `LoopThrashing`, `LoopStale`,
///      `ValidationFailure`, `Stopped`, `WorkspaceGone`, `RecoveryExhausted`,
///      `ReviewFailed`, and any unrecognized reason)
/// - 2: payload contract violation (`PayloadContractViolation`)
/// - 3: max iterations exceeded (`MaxIterations`)
/// - 4: max runtime exceeded (`MaxRuntime`)
/// - 5: max cost exceeded (`MaxCost`)
/// - 6: restart requested (`RestartRequested`)
/// - 130: interrupted by signal (`Interrupted`)
pub(crate) fn parent_exit_code_for_reason(reason: &TerminationReason) -> i32 {
    match reason {
        TerminationReason::CompletionPromise | TerminationReason::Cancelled => 0,
        TerminationReason::PayloadContractViolation => 2,
        TerminationReason::MaxIterations => 3,
        TerminationReason::MaxRuntime => 4,
        TerminationReason::MaxCost => 5,
        TerminationReason::RestartRequested => 6,
        TerminationReason::Interrupted => 130,
        _ => 1,
    }
}

/// U3: read the termination reason sentinel written by the loop runner child.
/// Returns `None` when the sentinel is missing or cannot be parsed.
fn read_termination_sentinel(workspace: &Path) -> Option<TerminationReason> {
    let path = workspace.join(".ralph/loop-termination-reason.json");
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// U3: resolve the termination reason for a subprocess-TUI child from the
/// sentinel first, falling back to coarse exit-code mapping when the sentinel
/// is unavailable.
fn resolve_subprocess_termination_reason(
    workspace: &Path,
    exit_status: &std::process::ExitStatus,
) -> TerminationReason {
    if exit_status.success() {
        return TerminationReason::CompletionPromise;
    }
    if let Some(reason) = read_termination_sentinel(workspace) {
        return reason;
    }
    // No sentinel means the child exited through an error path rather than a
    // typed non-success reason. Treat unknown non-zero codes as a generic stop.
    match exit_status.code() {
        Some(130) => TerminationReason::Interrupted,
        _ => TerminationReason::Stopped,
    }
}

/// Build the prompt-forwarding argv for a subprocess-TUI child invocation.
///
/// Precedence matches `resolve_prompt_content` in the child, so the child
/// takes the same path it would take if it were started directly in the
/// parent's cwd:
///
/// 1. `args.prompt_text` (CLI `-p`): emit `-p <text>`. Always absolute —
///    no path resolution needed.
/// 2. `args.prompt_file` (CLI `-P`): emit `-P <path>`. In worktree mode
///    (`args.worktree_path` set) with a relative path, anchor it at
///    `parent_cwd` (the main repo) so the child — whose cwd is the
///    worktree — can still find the file. Without this, the child fails
///    with "Prompt file 'PROMPT.md' not found" and the TUI shows
///    "Subprocess exited before starting the orchestration loop".
/// 3. Default `PROMPT.md` (no CLI flag): U2 (2026-06-14-002) changed this.
///    In worktree mode, the parent now syncs PROMPT.md to the worktree root,
///    so we forward a relative path (`PROMPT.md`) for the child to read from
///    its own cwd (which is the worktree). Outside worktree mode, the child
///    reads `PROMPT.md` from its own cwd and we do nothing here.
///
/// `parent_cwd` is passed in (rather than read from `std::env`) so the
/// function is unit-testable without chdir side effects.
fn forward_prompt_args(args: &SubprocessTuiArgs, parent_cwd: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(ref prompt) = args.prompt_text {
        out.push("-p".to_string());
        out.push(prompt.clone());
    } else if let Some(ref prompt_file) = args.prompt_file {
        out.push("-P".to_string());
        if args.worktree_path.is_some() && !prompt_file.is_absolute() {
            out.push(parent_cwd.join(prompt_file).to_string_lossy().into_owned());
        } else {
            out.push(prompt_file.to_string_lossy().to_string());
        }
    } else if args.worktree_path.is_some() {
        // U2 (2026-06-14-002): Default PROMPT.md has been synced to worktree root.
        // Forward a relative path so the child reads from its own cwd (worktree).
        // No existence check here — if sync failed, we warn in spawn_worktree_loop
        // but still forward the path; the child will fail loudly if file missing.
        out.push("-P".to_string());
        out.push("PROMPT.md".to_string());
    }
    out
}

#[cfg(test)]
mod forward_prompt_args_tests {
    use super::*;
    use std::fs;

    fn make_args(
        prompt_text: Option<&str>,
        prompt_file: Option<&Path>,
        worktree_path: Option<&Path>,
    ) -> SubprocessTuiArgs {
        SubprocessTuiArgs {
            prompt_text: prompt_text.map(str::to_string),
            prompt_file: prompt_file.map(PathBuf::from),
            backend: None,
            max_iterations: None,
            completion_promise: None,
            continue_mode: false,
            loop_id: None,
            idle_timeout: None,
            verbose: false,
            quiet: false,
            record_session: None,
            exclusive: false,
            no_auto_merge: false,
            skip_preflight: false,
            no_sync_agent_docs: false,
            worktree: false,
            worktree_path: worktree_path.map(PathBuf::from),
            workspace: PathBuf::new(),
            config_sources: vec![],
            hats_source: None,
        }
    }

    /// U2 (2026-06-14-002): In worktree mode with no CLI -p/-P, the function
    /// now forwards a relative `PROMPT.md` path. This is correct because:
    /// 1. The parent syncs PROMPT.md to the worktree root during spawn.
    /// 2. The child's cwd is the worktree, so relative resolution works.
    /// 3. No need to anchor at parent_cwd — the file is already in worktree.
    #[test]
    fn worktree_default_prompt_forwards_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        // No PROMPT.md needed in parent cwd — sync happens in spawn_worktree_loop
        let wt = PathBuf::from("/tmp/fake-worktree");
        let args = make_args(None, None, Some(&wt));

        let out = forward_prompt_args(&args, tmp.path());
        assert_eq!(
            out,
            vec!["-P".to_string(), "PROMPT.md".to_string()],
            "worktree mode must forward relative PROMPT.md (U2: file synced to worktree root)"
        );
    }

    /// U2 (2026-06-14-002): In worktree mode with no CLI -p/-P, we always
    /// forward `PROMPT.md` (no existence check). If sync failed, the child
    /// will fail loudly when it can't read the file — that's the correct
    /// behavior (fail fast, not silently fall back).
    #[test]
    fn worktree_default_prompt_always_forwards_regardless_of_parent_cwd_existence() {
        let tmp = tempfile::tempdir().unwrap();
        // No PROMPT.md in parent cwd — simulating sync failure
        let wt = PathBuf::from("/tmp/fake-worktree");
        let args = make_args(None, None, Some(&wt));

        let out = forward_prompt_args(&args, tmp.path());
        assert_eq!(
            out,
            vec!["-P".to_string(), "PROMPT.md".to_string()],
            "must forward PROMPT.md even if it doesn't exist in parent cwd (U2)"
        );
    }

    /// Outside worktree mode with no CLI -p/-P, we must NOT inject a
    /// `-P` for default PROMPT.md — the child's cwd is the same as the
    /// parent's, and `resolve_prompt_content` handles the default there.
    /// This guards against regression: injecting `-P` here in primary
    /// mode would change the child's prompt resolution in subtle ways.
    #[test]
    fn primary_mode_default_prompt_emits_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("PROMPT.md"), "x").unwrap();

        let args = make_args(None, None, None);
        let out = forward_prompt_args(&args, tmp.path());
        assert!(
            out.is_empty(),
            "primary mode must not inject -P (got: {:?})",
            out
        );
    }

    /// `-p <inline text>` takes priority over both -P and the default.
    /// Path resolution is irrelevant — text is forwarded verbatim.
    #[test]
    fn inline_prompt_takes_priority() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("PROMPT.md"), "ignored").unwrap();
        let wt = PathBuf::from("/tmp/fake-worktree");

        let args = make_args(Some("inline wins"), None, Some(&wt));
        let out = forward_prompt_args(&args, tmp.path());
        assert_eq!(out, vec!["-p".to_string(), "inline wins".to_string()]);
    }

    /// `-P <relative path>` in worktree mode must be anchored at the
    /// parent's cwd. This is the case the user hits when they pass
    /// `-P PROMPT.md` explicitly: the child would otherwise resolve it
    /// against its own (worktree) cwd and miss.
    #[test]
    fn worktree_relative_prompt_file_anchored_at_parent_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let prompt_path = tmp.path().join("CUSTOM.md");
        fs::write(&prompt_path, "custom prompt").unwrap();
        let wt = PathBuf::from("/tmp/fake-worktree");

        let args = make_args(None, Some(Path::new("CUSTOM.md")), Some(&wt));
        let out = forward_prompt_args(&args, tmp.path());
        assert_eq!(
            out,
            vec!["-P".to_string(), prompt_path.to_string_lossy().into_owned()]
        );
    }

    /// `-P <absolute path>` in worktree mode is passed through as-is.
    /// The user already gave us a resolved path; do not re-anchor.
    #[test]
    fn worktree_absolute_prompt_file_passed_through() {
        let tmp = tempfile::tempdir().unwrap();
        let abs_prompt = tmp.path().join("ABS.md");
        fs::write(&abs_prompt, "abs").unwrap();
        let wt = PathBuf::from("/tmp/fake-worktree");

        let args = make_args(None, Some(&abs_prompt), Some(&wt));
        let out = forward_prompt_args(&args, tmp.path());
        assert_eq!(
            out,
            vec!["-P".to_string(), abs_prompt.to_string_lossy().into_owned()]
        );
    }

    /// `-P <relative path>` in PRIMARY mode (no worktree) is forwarded
    /// verbatim — the child's cwd is the same as the parent's, so
    /// relative resolution works. Do NOT re-anchor here, that would
    /// break user's relative -P expectations outside worktree mode.
    #[test]
    fn primary_relative_prompt_file_forwarded_verbatim() {
        let args = make_args(None, Some(Path::new("REL.md")), None);
        let out = forward_prompt_args(&args, Path::new("/anywhere"));
        assert_eq!(out, vec!["-P".to_string(), "REL.md".to_string()]);
    }

    // ── U4: sync_prompt_to_worktree regression tests ───────────────────────

    /// U4 (2026-06-14-002): Happy path — copies PROMPT.md to worktree when source exists.
    #[test]
    fn sync_prompt_copies_to_worktree_when_source_exists() {
        let repo = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();
        fs::write(repo.path().join("PROMPT.md"), "test prompt content").unwrap();

        sync_prompt_to_worktree(repo.path(), wt.path());

        let copied = wt.path().join("PROMPT.md");
        assert!(copied.exists(), "PROMPT.md should be copied to worktree");
        assert_eq!(
            fs::read_to_string(&copied).unwrap(),
            "test prompt content"
        );
    }

    /// U4: Silently skips when source PROMPT.md doesn't exist.
    #[test]
    fn sync_prompt_skips_when_source_missing() {
        let repo = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();

        sync_prompt_to_worktree(repo.path(), wt.path());

        assert!(!wt.path().join("PROMPT.md").exists());
    }

    /// U4: Silently skips when worktree already has PROMPT.md.
    #[test]
    fn sync_prompt_skips_when_worktree_has_it() {
        let repo = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();
        fs::write(repo.path().join("PROMPT.md"), "repo version").unwrap();
        fs::write(wt.path().join("PROMPT.md"), "worktree version").unwrap();

        sync_prompt_to_worktree(repo.path(), wt.path());

        // Should NOT overwrite existing file in worktree
        assert_eq!(
            fs::read_to_string(wt.path().join("PROMPT.md")).unwrap(),
            "worktree version"
        );
    }
}

#[cfg(test)]
mod run_loop_result_exit_code_tests {
    use super::*;
    use loop_runner::{EXIT_CODE_AGENT_DOC_SYNC_STRICT, EXIT_CODE_LINT_GATE, PresetLintGateError};

    #[test]
    fn detects_lint_gate_error_in_chain() {
        let inner = PresetLintGateError {
            findings: vec![],
            error_count: 1,
            warning_count: 0,
        };
        let err = anyhow::Error::new(inner);
        assert_eq!(run_loop_result_exit_code(&err), Some(EXIT_CODE_LINT_GATE));
        assert_eq!(EXIT_CODE_LINT_GATE, 2);
    }

    #[test]
    fn ignores_unrelated_errors() {
        let err = anyhow::anyhow!("some unrelated IO failure");
        assert_eq!(run_loop_result_exit_code(&err), None);
    }

    #[test]
    fn detects_agent_doc_sync_strict_in_chain() {
        // The error string literal matches what `runner.rs` raises on
        // agent_doc_sync strict-mode failure (B1-边界 contract).
        let err = anyhow::anyhow!("agent_doc_sync failed in strict mode");
        assert_eq!(
            run_loop_result_exit_code(&err),
            Some(EXIT_CODE_AGENT_DOC_SYNC_STRICT)
        );
        assert_eq!(EXIT_CODE_AGENT_DOC_SYNC_STRICT, 78);
    }

    #[test]
    fn detects_agent_doc_sync_through_wrapped_context() {
        let err: anyhow::Error = anyhow::anyhow!("agent_doc_sync failed in strict mode")
            .context("agent doc sync strict mode");
        assert_eq!(
            run_loop_result_exit_code(&err),
            Some(EXIT_CODE_AGENT_DOC_SYNC_STRICT)
        );
    }

    #[test]
    fn detects_lint_gate_through_wrapped_context() {
        let inner = PresetLintGateError {
            findings: vec![],
            error_count: 2,
            warning_count: 0,
        };
        // Wrap via `anyhow::Error::context` (a method on the error
        // itself, not on `anyhow::Context::context` which only works
        // for `Result<T, E>`). This adds a layer above the inner
        // `PresetLintGateError`; the chain walk must still find it.
        let err: anyhow::Error = anyhow::Error::new(inner).context("wrapping context");
        assert_eq!(run_loop_result_exit_code(&err), Some(EXIT_CODE_LINT_GATE));
    }
}
/// Arguments needed for subprocess TUI mode.
/// We clone these early before RunArgs fields are consumed.
#[derive(Clone)]
struct SubprocessTuiArgs {
    pub prompt_text: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub backend: Option<String>,
    pub max_iterations: Option<u32>,
    pub completion_promise: Option<String>,
    pub continue_mode: bool,
    pub loop_id: Option<String>,
    pub idle_timeout: Option<u32>,
    pub verbose: bool,
    pub quiet: bool,
    pub record_session: Option<PathBuf>,
    pub exclusive: bool,
    pub no_auto_merge: bool,
    pub skip_preflight: bool,
    pub no_sync_agent_docs: bool,
    pub worktree: bool,
    /// When set, the child RPC process chdir's into this path before starting.
    /// Populated by the parent when `--worktree` is used: parent already
    /// created the worktree, so the child must NOT receive `--worktree` (that
    /// would cause a duplicate worktree). Instead, the child inherits the
    /// parent's worktree path here. P1-F fix on 2026-06-10.
    pub worktree_path: Option<PathBuf>,
    /// Workspace cwd for child: worktree path in worktree mode, main repo in
    /// primary mode. Used as base path for parent's stderr log and as
    /// Command::current_dir when spawning the child.
    pub workspace: PathBuf,
    /// Config sources to forward to child process (-c args)
    pub config_sources: Vec<String>,
    /// Hats source to forward to child process (-H arg)
    pub hats_source: Option<String>,
}

impl SubprocessTuiArgs {
    /// Create from RunArgs with config/hats sources from Cli.
    /// Note: workspace is set AFTER this constructor is called, after
    /// loop_context is determined (see line ~792).
    fn new(
        args: &RunArgs,
        config_sources: &[ConfigSource],
        hats_source: Option<&HatsSource>,
    ) -> Self {
        Self {
            prompt_text: args.prompt_text.clone(),
            prompt_file: args.prompt_file.clone(),
            backend: args.backend.clone(),
            max_iterations: args.max_iterations,
            completion_promise: args.completion_promise.clone(),
            continue_mode: args.continue_mode,
            loop_id: args.loop_id.clone(),
            idle_timeout: args.idle_timeout,
            verbose: args.verbose,
            quiet: args.quiet,
            record_session: args.record_session.clone(),
            exclusive: args.exclusive,
            no_auto_merge: args.no_auto_merge,
            skip_preflight: args.skip_preflight,
            no_sync_agent_docs: args.no_sync_agent_docs,
            worktree: args.worktree,
            worktree_path: None,
            workspace: PathBuf::new(), // Set after loop_context is determined
            config_sources: config_sources.iter().map(|s| s.to_cli_string()).collect(),
            hats_source: hats_source.map(|h| h.label()),
        }
    }
}

/// Run the orchestration loop as a subprocess with TUI attached.
///
/// This spawns `ralph run --rpc` as a child process and attaches the TUI
/// as a client that reads JSON events from stdout and sends commands to stdin.
/// This two-process model allows the TUI to be decoupled from the orchestration loop.
async fn run_subprocess_tui(
    args: SubprocessTuiArgs,
    resume: bool,
    custom_args: Vec<String>,
) -> Result<TerminationReason> {
    use std::process::Stdio;
    use tokio::process::Command;

    // Build child command: ralph [-c ...] [-H ...] run --rpc <forwarded args>
    // Note: -c and -H are global options that must come BEFORE the subcommand
    let mut child_args = Vec::new();

    // Forward config sources (global option, before subcommand)
    for config_source in &args.config_sources {
        child_args.push("-c".to_string());
        child_args.push(config_source.clone());
    }

    // Forward hats source (global option, before subcommand)
    if let Some(ref hats) = args.hats_source {
        child_args.push("-H".to_string());
        child_args.push(hats.clone());
    }

    // Add subcommand and mode
    child_args.push("run".to_string());
    child_args.push("--rpc".to_string());

    // Forward prompt
    child_args.extend(forward_prompt_args(
        &args,
        &std::env::current_dir().unwrap_or_default(),
    ));

    // Forward backend
    if let Some(ref backend) = args.backend {
        child_args.push("-b".to_string());
        child_args.push(backend.clone());
    }

    // Forward max iterations
    if let Some(max_iters) = args.max_iterations {
        child_args.push("--max-iterations".to_string());
        child_args.push(max_iters.to_string());
    }

    // Forward completion promise
    if let Some(ref promise) = args.completion_promise {
        child_args.push("--completion-promise".to_string());
        child_args.push(promise.clone());
    }

    // Forward continue mode and loop ID
    if resume || args.continue_mode {
        child_args.push("--continue".to_string());
    }
    if let Some(ref loop_id) = args.loop_id {
        child_args.push("--loop-id".to_string());
        child_args.push(loop_id.clone());
    }

    // Forward idle timeout
    if let Some(timeout) = args.idle_timeout {
        child_args.push("--idle-timeout".to_string());
        child_args.push(timeout.to_string());
    }

    // Forward verbosity
    if args.verbose {
        child_args.push("-v".to_string());
    }
    if args.quiet {
        child_args.push("-q".to_string());
    }

    // Forward record session
    if let Some(ref path) = args.record_session {
        child_args.push("--record-session".to_string());
        child_args.push(path.to_string_lossy().to_string());
    }

    // Forward multi-loop options
    if args.exclusive {
        child_args.push("--exclusive".to_string());
    }
    if args.no_auto_merge {
        child_args.push("--no-auto-merge".to_string());
    }
    // U2 (2026-06-10): forward --worktree-path (not --worktree) when the parent
    // already created a worktree. Passing --worktree would cause the child to
    // create a duplicate worktree inside the parent's. The child detects
    // --worktree-path and enters LoopContext::worktree directly, skipping creation.
    if let Some(ref worktree_path) = args.worktree_path {
        child_args.push("--worktree-path".to_string());
        child_args.push(worktree_path.to_string_lossy().into_owned());
    }

    // Forward preflight options
    if args.skip_preflight {
        child_args.push("--skip-preflight".to_string());
    }

    // Forward agent doc sync options
    if args.no_sync_agent_docs {
        child_args.push("--no-sync-agent-docs".to_string());
    }

    // Forward custom args (after --)
    if !custom_args.is_empty() {
        child_args.push("--".to_string());
        child_args.extend(custom_args);
    }

    info!(child_args = ?child_args, "Spawning subprocess for TUI mode");

    // Spawn child process.
    // Redirect stderr to a log file to prevent child tracing output from
    // corrupting the TUI display (ratatui runs in raw terminal mode).
    // U3 (2026-06-10): use args.workspace explicitly instead of
    // std::env::current_dir() which was fragile (relied on chdir side effect).
    let stderr_stdio = match ralph_core::diagnostics::create_log_file(&args.workspace) {
        Ok((file, path)) => {
            info!(log_file = %path.display(), "TUI subprocess stderr redirected to log file");
            Stdio::from(file)
        }
        Err(_) => Stdio::null(),
    };

    // U3 (2026-06-10): explicitly set child's cwd to workspace (worktree path
    // in worktree mode, main repo in primary mode). This replaces the old
    // chdir hack and makes the child's cwd explicit rather than relying on
    // side effects.
    let mut child = Command::new(std::env::current_exe()?)
        .args(&child_args)
        .current_dir(&args.workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr_stdio)
        .spawn()
        .context("Failed to spawn ralph subprocess for TUI")?;

    let stdin = child
        .stdin
        .take()
        .context("Failed to capture subprocess stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("Failed to capture subprocess stdout")?;

    // Create TUI state and start event reader
    let state = std::sync::Arc::new(std::sync::Mutex::new(ralph_tui::TuiState::new()));
    let (terminated_tx, terminated_rx) = tokio::sync::watch::channel(false);

    // Create RPC writer for sending commands
    let rpc_writer = ralph_tui::RpcWriter::new(stdin);

    // U3: tee the child's RPC stdout into the TUI reader so the parent can
    // detect a fatal LoopTerminated event and break the TUI out of its input
    // loop instead of waiting for the user to press 'q'.
    let (tui_reader, mut tui_writer) = duplex(64 * 1024);

    let forward_handle = {
        let mut cancel_rx = terminated_rx.clone();
        tokio::spawn(async move {
            let mut child_lines = BufReader::new(stdout).lines();
            loop {
                tokio::select! {
                    _ = cancel_rx.changed() => {
                        if *cancel_rx.borrow() {
                            break;
                        }
                    }
                    line = child_lines.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if tui_writer.write_all(line.as_bytes()).await.is_err() {
                                    break;
                                }
                                if tui_writer.write_all(b"\n").await.is_err() {
                                    break;
                                }
                                if tui_writer.flush().await.is_err() {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                }
            }
        })
    };

    // Spawn the event reader as a background task
    let reader_state = std::sync::Arc::clone(&state);
    let cancel_rx = terminated_rx.clone();
    let reader_handle = tokio::spawn(async move {
        ralph_tui::run_rpc_event_reader(tui_reader, reader_state, cancel_rx).await;
    });

    info!("TUI running in subprocess RPC mode");

    // Run the TUI render/input loop with subprocess support concurrently with
    // the child process. If the child exits with a non-success status, signal
    // the TUI to exit immediately rather than waiting for the user.
    let app = ralph_tui::App::new_subprocess(
        std::sync::Arc::clone(&state),
        terminated_rx.clone(),
        rpc_writer.clone(),
    );
    let mut app_fut = std::pin::pin!(app.run());
    let mut child_wait = std::pin::pin!(child.wait());

    let (tui_result, exit_status) = tokio::select! {
        status = child_wait.as_mut() => {
            let status = status.context("Failed to wait for ralph subprocess")?;
            if !status.success() {
                let _ = terminated_tx.send(true);
            }
            let tui_result = tokio::time::timeout(Duration::from_secs(5), app_fut)
                .await
                .unwrap_or_else(|_| {
                    warn!("TUI did not exit within 5s after child termination; forcing return");
                    Ok(())
                });
            (tui_result, status)
        }
        result = app_fut.as_mut() => {
            let tui_result = result;
            // App exited (e.g., user pressed 'q'); wait for the child.
            let status = child_wait
                .await
                .context("Failed to wait for ralph subprocess")?;
            (tui_result, status)
        }
    };

    // Signal cancellation to the reader/forward tasks and clean up the child.
    let _ = terminated_tx.send(true);
    let _ = rpc_writer.send_abort().await;
    let _ = rpc_writer.close().await;
    let _ = reader_handle.await;
    let _ = forward_handle.await;

    // Resolve the exact termination reason from the runner sentinel when the
    // child did not exit cleanly; fall back to coarse exit-code mapping.
    let reason = resolve_subprocess_termination_reason(&args.workspace, &exit_status);

    // Return TUI result if it failed, otherwise the termination reason
    tui_result.map(|_| reason)
}

#[cfg(test)]
pub(crate) fn default_run_args() -> RunArgs {
    RunArgs {
        prompt_text: None,
        backend: Some("claude".to_string()),
        prompt_file: None,
        max_iterations: None,
        completion_promise: None,
        dry_run: false,
        continue_mode: false,
        loop_id: None,
        no_tui: true,
        autonomous: false,
        rpc: false,
        legacy_tui: false,
        idle_timeout: None,
        autonomous_idle_timeout: None,
        exclusive: false,
        no_auto_merge: false,
        worktree: false,
        worktree_path: None,
        skip_preflight: true,
        no_sync_agent_docs: false,
        verbose: false,
        quiet: false,
        record_session: None,
        custom_args: Vec::new(),
        warmup_only: false,
        force_warmup: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CwdGuard;
    use ralph_core::{HatConfig, HookMutationConfig, HookOnError, HookPhaseEvent, HookSpec};

    #[test]
    fn test_required_restart_command_matches_contract() {
        let command = required_restart_command(4242);
        assert_eq!(
            command,
            "kill 4242 && RALPH_DIAGNOSTICS=1 cargo run --bin ralph -- resume -c ralph.test.yml"
        );
    }

    #[test]
    fn test_clear_restart_request_signal_removes_sentinel_file() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let restart_dir = temp_dir.path().join(".ralph");
        std::fs::create_dir_all(&restart_dir).expect("create .ralph dir");
        let restart_path = restart_dir.join("restart-requested");
        std::fs::write(&restart_path, "requested").expect("write sentinel");

        clear_restart_request_signal(temp_dir.path());

        assert!(
            !restart_path.exists(),
            "restart sentinel should be removed before restart command dispatch"
        );
    }

    #[test]
    fn test_worktree_file_name_prefix_uses_prompt_file_stem() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prompt_path = temp_dir
            .path()
            .join("custom-drift-auto-calibration-prompt.md");
        std::fs::write(&prompt_path, "Implement something").unwrap();

        let source = worktree_file_name_prefix(
            prompt_path.to_str().unwrap(),
            "Implement something",
            temp_dir.path(),
        );

        assert_eq!(
            source.as_deref(),
            Some("custom-drift-auto-calibration-prompt")
        );
    }

    #[test]
    fn test_worktree_file_name_prefix_returns_none_without_prompt_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = worktree_file_name_prefix("", "Implement something", temp_dir.path());

        assert_eq!(source, None);
    }

    #[test]
    fn test_worktree_file_name_prefix_uses_inline_plan_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = worktree_file_name_prefix(
            "PROMPT.md",
            "Implement dev plan:docs/plans/2026-06-04-004-feat-drift-auto-calibration-plan.md",
            temp_dir.path(),
        );

        assert_eq!(
            source.as_deref(),
            Some("2026-06-04-004-feat-drift-auto-calibration-plan")
        );
    }

    #[test]
    fn test_worktree_file_name_prefix_ignores_task_md_and_uses_prompt_file_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("task.md"),
            "Implement dev plan:docs/plans/2026-06-04-004-feat-from-task-md-plan.md",
        )
        .unwrap();
        std::fs::write(
            temp_dir.path().join("PROMPT.md"),
            "Implement dev plan:docs/plans/2026-06-04-004-feat-from-prompt-md-plan.md",
        )
        .unwrap();

        let source = worktree_file_name_prefix("PROMPT.md", "[no prompt]", temp_dir.path());

        assert_eq!(
            source.as_deref(),
            Some("2026-06-04-004-feat-from-prompt-md-plan")
        );
    }

    #[test]
    fn test_worktree_file_name_prefix_ignores_missing_default_prompt_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = worktree_file_name_prefix("PROMPT.md", "[no prompt]", temp_dir.path());

        assert_eq!(source, None);
    }

    #[tokio::test]
    async fn test_auto_preflight_dry_run_returns_report() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();
        config.features.preflight.enabled = true;
        config.features.preflight.skip = vec!["git".to_string(), "tools".to_string()];
        config.cli.backend = "custom".to_string();
        config.cli.command = Some("definitely-missing-12345".to_string());

        let report = run_auto_preflight(&config, false, false, AutoPreflightMode::DryRun)
            .await
            .unwrap();

        let report = report.expect("expected preflight report in dry-run mode");
        assert!(!report.passed);
        assert!(report.failures >= 1);
    }

    #[tokio::test]
    async fn test_auto_preflight_skip_list_can_omit_hooks_check_failures() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();
        // WRC-U1 (2026-06-12-003): the lint is now always-on.
        // The default `RalphConfig` enables tasks with no
        // coordinator_hats, which trips the
        // `coordinator_missing` rule. Disable tasks so the
        // contract check focuses on the hooks-skip behaviour
        // the test was originally written to exercise.
        config.tasks.enabled = false;
        config.features.preflight.enabled = true;
        config.cli.backend = "custom".to_string();

        let backend_cmd = temp_dir.path().join("backend-ok");
        std::fs::write(&backend_cmd, "ok").unwrap();
        config.cli.command = Some(backend_cmd.to_string_lossy().to_string());

        config.hooks.enabled = true;
        config.hooks.events.insert(
            HookPhaseEvent::PreLoopStart,
            vec![HookSpec {
                name: "broken-hook".to_string(),
                command: vec!["./scripts/hooks/missing.sh".to_string()],
                cwd: None,
                env: std::collections::HashMap::new(),
                timeout_seconds: None,
                max_output_bytes: None,
                on_error: Some(HookOnError::Block),
                suspend_mode: None,
                mutate: HookMutationConfig::default(),
                extra: std::collections::HashMap::new(),
            }],
        );

        let unskipped = run_auto_preflight(&config, false, false, AutoPreflightMode::DryRun)
            .await
            .unwrap()
            .expect("dry-run preflight report");

        assert!(!unskipped.passed);
        let hooks_check = unskipped
            .checks
            .iter()
            .find(|check| check.name == "hooks")
            .expect("hooks check should be present without skip");
        assert_eq!(hooks_check.status, CheckStatus::Fail);

        config.features.preflight.skip = vec!["hooks".to_string()];
        let skipped = run_auto_preflight(&config, false, false, AutoPreflightMode::DryRun)
            .await
            .unwrap()
            .expect("dry-run preflight report");

        assert!(skipped.passed);
        assert!(skipped.checks.iter().all(|check| check.name != "hooks"));
    }

    #[tokio::test]
    async fn test_auto_preflight_run_fails_on_check_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();
        config.features.preflight.enabled = true;
        config.features.preflight.skip = vec!["git".to_string(), "tools".to_string()];
        config.cli.backend = "custom".to_string();
        config.cli.command = Some("definitely-missing-12345".to_string());

        let err = run_auto_preflight(&config, false, false, AutoPreflightMode::Run)
            .await
            .expect_err("expected preflight failure in run mode");

        assert!(err.to_string().contains("Preflight checks failed"));
    }

    // ──────────────────────────────────────────────────────────────────────
    // U0 characterization tests: lock in existing preflight gating behavior
    // so the U1/U2 shared contract layer cannot silently change semantics.
    // ──────────────────────────────────────────────────────────────────────

    /// 30s wall-clock guard for U0 async preflight tests. U0 tests are
    /// pure (no network, no I/O beyond `tempdir`), but if a future refactor
    /// pulls in a network call (e.g. `git fetch` for topology) or a
    /// blocking `Command::output()` on a hung child, the test could hang
    /// the entire `cargo test` invocation. Tokio's default test runtime
    /// has no per-test timeout (and `.config/nextest.toml` has no
    /// `default-timeout` key), so wrap the future explicitly.
    async fn u0_timeout<F, T>(fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        match tokio::time::timeout(std::time::Duration::from_secs(30), fut).await {
            Ok(value) => value,
            Err(_) => {
                panic!("U0 preflight test exceeded 30s timeout (likely a blocking syscall hung)")
            }
        }
    }

    /// U0 characterization: the default `RalphConfig` has
    /// `features.preflight.enabled = false`. `run_auto_preflight()` must
    /// return `Ok(None)` and never invoke the preflight runner. This is the
    /// default `ralph run` behavior and must not regress.
    #[tokio::test]
    async fn u0_auto_preflight_disabled_returns_none() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();
        // Default: enabled = false. No skip list, no strict, no override.
        assert!(
            !config.features.preflight.enabled,
            "default config must have preflight disabled"
        );

        let result = u0_timeout(run_auto_preflight(
            &config,
            false,
            false,
            AutoPreflightMode::DryRun,
        ))
        .await
        .expect("run_auto_preflight should not error when preflight is disabled");

        assert!(
            result.is_none(),
            "preflight disabled must short-circuit to None (no report, no checks); \
             got: {:?}",
            result
        );

        // Same for Run mode: no error, no report, no checks.
        let run_result = u0_timeout(run_auto_preflight(
            &config,
            false,
            false,
            AutoPreflightMode::Run,
        ))
        .await
        .expect("run_auto_preflight Run mode should not error when preflight is disabled");
        assert!(
            run_result.is_none(),
            "preflight disabled in Run mode must short-circuit to None; got: {:?}",
            run_result
        );
    }

    /// U0 characterization: `skip_preflight=true` (the `--skip-preflight` flag)
    /// must override `features.preflight.enabled = true` and return `None`
    /// without running any checks. This is the documented escape hatch.
    ///
    /// **Ordering invariant**: the short-circuit guard at `run_auto_preflight`
    /// (line 215: `if skip_preflight || !config.features.preflight.enabled { return Ok(None) }`)
    /// MUST execute before `PreflightRunner::default_checks_with_config(config)`
    /// construction (line 219) and `runner.run_all/config` (line 222). This test
    /// deliberately uses a non-resolvable backend (`definitely-missing-12345`) to
    /// prove the short-circuit fires before any check would have a chance to fail
    /// on the missing backend. If a future refactor moves the short-circuit
    /// below the runner construction, this test's failure mode becomes
    /// "backend missing" — still red, but no longer interpretable as
    /// 'short-circuit ordering broke'. The test name and doc pin that
    /// ordering so the failure mode stays meaningful.
    #[tokio::test]
    async fn u0_skip_preflight_short_circuits_before_backend_check() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();
        config.features.preflight.enabled = true;
        // Backend missing would fail if preflight actually ran.
        config.cli.backend = "custom".to_string();
        config.cli.command = Some("definitely-missing-12345".to_string());

        // skip_preflight=true must short-circuit.
        let result = u0_timeout(run_auto_preflight(
            &config,
            true,
            false,
            AutoPreflightMode::DryRun,
        ))
        .await
        .expect("skip_preflight=true should not error");
        assert!(
            result.is_none(),
            "skip_preflight=true must short-circuit; got: {:?}",
            result
        );

        let run_result = u0_timeout(run_auto_preflight(
            &config,
            true,
            false,
            AutoPreflightMode::Run,
        ))
        .await
        .expect("skip_preflight=true in Run mode should not error");
        assert!(
            run_result.is_none(),
            "skip_preflight=true in Run mode must short-circuit; got: {:?}",
            run_result
        );
    }

    /// U0 characterization: when `features.preflight.enabled = true`, the
    /// env-dependent checks are skipped, and the topology is valid,
    /// `run_auto_preflight()` must return a passing report. This locks in
    /// the "valid preset → preflight passes" semantic for the G0 gate.
    #[tokio::test]
    async fn u0_auto_preflight_enabled_with_valid_topology_passes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();
        config.features.preflight.enabled = true;
        // Skip env-only checks (git, tools) so we are left with config
        // semantic + topology + hooks checks.
        config.features.preflight.skip = vec!["git".to_string(), "tools".to_string()];
        // WRC-U1 (2026-06-12-003): the lint is now always-on, so
        // declare the legacy `LOOP_COMPLETE` completion promise in
        // the format whitelist and enable tasks with a coordinator
        // hat. Without these, the contract check fails before the
        // topology check has a chance to run.
        config.topic_format_whitelist = vec!["LOOP_COMPLETE".to_string()];
        config.tasks.enabled = true;
        config.tasks.coordinator_hats = vec!["executor".to_string()];
        // Configure a valid linear topology so preset-topology passes.
        config.event_loop.starting_event = Some("work.start".to_string());
        config.event_loop.completion_promise = "LOOP_COMPLETE".to_string();
        config.hats.insert(
            "executor".to_string(),
            HatConfig {
                name: "Executor".to_string(),
                description: Some("Execute the task.".to_string()),
                triggers: vec!["work.start".to_string()],
                publishes: vec!["work.done".to_string()],
                ..Default::default()
            },
        );
        config.hats.insert(
            "reporter".to_string(),
            HatConfig {
                name: "Reporter".to_string(),
                description: Some("Report the result.".to_string()),
                triggers: vec!["work.done".to_string()],
                publishes: vec!["LOOP_COMPLETE".to_string()],
                ..Default::default()
            },
        );

        // Provide a resolvable backend so the backend check is satisfied.
        let backend_cmd = temp_dir.path().join("backend-ok");
        std::fs::write(&backend_cmd, "ok").unwrap();
        config.cli.backend = "custom".to_string();
        config.cli.command = Some(backend_cmd.to_string_lossy().to_string());

        let report = u0_timeout(run_auto_preflight(
            &config,
            false,
            false,
            AutoPreflightMode::DryRun,
        ))
        .await
        .expect("preflight should not error")
        .expect("preflight enabled must return a report");

        assert!(
            report.passed,
            "valid topology + skipped env checks must produce a passing report; \
             failures={} warnings={} checks={:?}",
            report.failures, report.warnings, report.checks
        );
        assert_eq!(report.failures, 0, "no check should fail in this scenario");
    }

    #[test]
    fn test_prompt_summary_reads_file_content_not_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prompt_path = temp_dir.path().join("PROMPT.md");
        let prompt_content = "Build a feature that does amazing things";

        // Write the prompt file
        std::fs::write(&prompt_path, prompt_content).unwrap();

        // Create config with prompt_file set
        let mut config = RalphConfig::default();
        config.event_loop.prompt_file = prompt_path.to_string_lossy().to_string();
        config.event_loop.prompt = None;

        // Simulate the prompt_summary logic from run_command
        let prompt_summary = config
            .event_loop
            .prompt
            .clone()
            .or_else(|| {
                let prompt_file = &config.event_loop.prompt_file;
                if prompt_file.is_empty() {
                    None
                } else {
                    let path = std::path::Path::new(prompt_file);
                    if path.exists() {
                        std::fs::read_to_string(path).ok()
                    } else {
                        None
                    }
                }
            })
            .map(|p| truncate_with_ellipsis(&p, 100))
            .unwrap_or_else(|| "[no prompt]".to_string());

        // Assert: summary contains file content, NOT the file path
        assert_eq!(prompt_summary, prompt_content);
        assert!(!prompt_summary.contains("PROMPT.md"));
        assert!(!prompt_summary.contains(&temp_dir.path().to_string_lossy().to_string()));
    }

    #[test]
    fn test_prompt_summary_truncates_long_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prompt_path = temp_dir.path().join("LONG_PROMPT.md");
        let long_content = "X".repeat(150); // 150 chars, exceeds 100 limit

        std::fs::write(&prompt_path, &long_content).unwrap();

        let mut config = RalphConfig::default();
        config.event_loop.prompt_file = prompt_path.to_string_lossy().to_string();
        config.event_loop.prompt = None;

        // Simulate the prompt_summary logic
        let prompt_summary = config
            .event_loop
            .prompt
            .clone()
            .or_else(|| {
                let prompt_file = &config.event_loop.prompt_file;
                if prompt_file.is_empty() {
                    None
                } else {
                    let path = std::path::Path::new(prompt_file);
                    if path.exists() {
                        std::fs::read_to_string(path).ok()
                    } else {
                        None
                    }
                }
            })
            .map(|p| truncate_with_ellipsis(&p, 100))
            .unwrap_or_else(|| "[no prompt]".to_string());

        // Assert: truncated to 100 chars total
        assert_eq!(prompt_summary.len(), 100);
        assert!(prompt_summary.ends_with("..."));
    }

    #[test]
    fn test_prompt_summary_returns_no_prompt_for_missing_file() {
        let mut config = RalphConfig::default();
        config.event_loop.prompt_file = "/nonexistent/path/PROMPT.md".to_string();
        config.event_loop.prompt = None;

        // Simulate the prompt_summary logic
        let prompt_summary = config
            .event_loop
            .prompt
            .clone()
            .or_else(|| {
                let prompt_file = &config.event_loop.prompt_file;
                if prompt_file.is_empty() {
                    None
                } else {
                    let path = std::path::Path::new(prompt_file);
                    if path.exists() {
                        std::fs::read_to_string(path).ok()
                    } else {
                        None
                    }
                }
            })
            .map(|p| truncate_with_ellipsis(&p, 100))
            .unwrap_or_else(|| "[no prompt]".to_string());

        // Assert: returns "[no prompt]" for missing file
        assert_eq!(prompt_summary, "[no prompt]");
    }

    #[test]
    fn test_format_preflight_summary_with_failures() {
        let report = PreflightReport {
            passed: false,
            warnings: 1,
            failures: 1,
            checks: vec![
                ralph_core::CheckResult::pass("config", "Config"),
                ralph_core::CheckResult::warn("backend", "Backend", "Missing"),
                ralph_core::CheckResult::fail("paths", "Paths", "Missing path"),
            ],
        };

        let summary = format_preflight_summary(&report);

        assert!(summary.contains("✓"));
        assert!(summary.contains("⚠"));
        assert!(summary.contains("✗"));
        assert!(summary.contains("(1 failure)"));
    }

    #[test]
    fn test_format_preflight_summary_no_checks() {
        let report = PreflightReport {
            passed: true,
            warnings: 0,
            failures: 0,
            checks: Vec::new(),
        };

        let summary = format_preflight_summary(&report);

        assert_eq!(summary, "no checks");
    }

    #[test]
    fn test_preflight_failure_detail_strict_includes_warnings() {
        let report = PreflightReport {
            passed: false,
            warnings: 2,
            failures: 1,
            checks: Vec::new(),
        };

        assert_eq!(preflight_failure_detail(&report, false), "1 failure");
        assert_eq!(
            preflight_failure_detail(&report, true),
            "1 failure, 2 warnings"
        );
    }

    #[test]
    fn test_print_preflight_summary_handles_failures_and_warnings() {
        let report = PreflightReport {
            passed: false,
            warnings: 1,
            failures: 1,
            checks: vec![
                ralph_core::CheckResult::pass("config", "Config"),
                ralph_core::CheckResult::warn("backend", "Backend", "Missing"),
                ralph_core::CheckResult::fail("paths", "Paths", "Missing path"),
            ],
        };

        print_preflight_summary(&report, true, "Preflight: ", true);
        print_preflight_summary(&report, false, "Preflight: ", false);
    }

    #[tokio::test]
    async fn test_run_command_continue_missing_scratchpad_returns_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());

        let mut args = default_run_args();
        args.continue_mode = true;

        let err = run_command(&[], None, false, ColorMode::Never, args, None)
            .await
            .expect_err("expected missing scratchpad error");
        assert!(err.to_string().contains("scratchpad not found"));
    }

    #[tokio::test]
    async fn test_run_command_dry_run_inline_prompt_skips_execution() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());

        let mut args = default_run_args();
        args.dry_run = true;
        args.prompt_text = Some("Test inline prompt".to_string());

        run_command(&[], None, false, ColorMode::Never, args, None)
            .await
            .expect("dry run should succeed");
    }

    #[tokio::test]
    async fn test_run_command_allows_single_file_combined_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());

        std::fs::write(
            temp_dir.path().join("ralph.yml"),
            r#"
cli:
  backend: claude
hats:
  builder:
    name: Builder
    description: Test builder
    triggers: ["build.task"]
    publishes: ["build.done"]
"#,
        )
        .unwrap();

        let mut args = default_run_args();
        args.dry_run = true;
        args.prompt_text = Some("Test inline prompt".to_string());

        run_command(
            &[ConfigSource::File(std::path::PathBuf::from("ralph.yml"))],
            None,
            false,
            ColorMode::Never,
            args,
            None,
        )
        .await
        .expect("combined config should be accepted");
    }

    // ── U3 (2026-06-14): subprocess TUI + worktree diagnostics layout ──
    // Verifies the cross-process contract:
    //   1. Parent in main repo uses trace_only → no loop-level files.
    //   2. Child RPC re-enters main() in worktree cwd → full session.
    //   3. Worktree session contains real recovery/drift/etc. data.
    // This is the integration that closes the "empty shell" bug.

    fn init_test_git_repo(repo_root: &std::path::Path) {
        use std::process::Command;
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo_root)
            .status()
            .expect("git init");
        Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=t@e.com",
                "commit",
                "--allow-empty",
                "-m",
                "init",
                "-q",
            ])
            .current_dir(repo_root)
            .status()
            .expect("git commit");
    }

    #[test]
    fn parent_trace_only_leaves_no_loop_level_files_in_main_repo() {
        // Simulate the parent: cwd is the main repo, subprocess TUI mode
        // is on, so trace_only=true. Assert: no recovery/drift/...
        // files anywhere under .ralph/diagnostics/ in the main repo.
        let temp = tempfile::TempDir::new().unwrap();
        init_test_git_repo(temp.path());

        let options = ralph_core::diagnostics::DiagnosticsOptions {
            full_diagnostics: false,
            runtime_diagnosis_artifacts: false,
            trace_only: true,
            ..Default::default()
        };
        let collector =
            ralph_core::diagnostics::DiagnosticsCollector::with_options(temp.path(), &options)
                .unwrap();

        // Session dir is created for the trace layer / TUI stderr log.
        let session_dir = collector
            .session_dir()
            .expect("trace_only creates session_dir");
        assert!(session_dir.exists());

        // No loop-level files: this is the bug fix.
        for name in [
            "recovery.jsonl",
            "drift.jsonl",
            "orchestration.jsonl",
            "performance.jsonl",
            "errors.jsonl",
            "hook-runs.jsonl",
            "agent-output.jsonl",
            "prompt-log.md",
        ] {
            assert!(
                !session_dir.join(name).exists(),
                "trace_only must not create {name} (empty shell bug)"
            );
        }
    }

    #[test]
    fn child_rpc_creates_full_session_in_worktree_cwd() {
        // Simulate the child RPC: cwd is the worktree, full_diagnostics
        // is on (or runtime_diagnosis_artifacts). Assert: full session
        // is created under worktree's .ralph/diagnostics/<ts>/, and the
        // main repo under the same timestamp contains no recovery.jsonl
        // because the parent never ran with cwd=main repo.
        use std::process::Command;
        let temp = tempfile::TempDir::new().unwrap();
        let repo_root = temp.path();
        init_test_git_repo(repo_root);

        // Create a worktree.
        let worktree_path = repo_root.join(".worktrees").join("test-loop");
        std::fs::create_dir_all(worktree_path.parent().unwrap()).unwrap();
        Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                "test-loop",
                worktree_path.to_str().unwrap(),
            ])
            .current_dir(repo_root)
            .status()
            .expect("git worktree add");

        // Parent: trace_only, cwd=main repo. No loop-level files.
        let parent_options = ralph_core::diagnostics::DiagnosticsOptions {
            full_diagnostics: false,
            runtime_diagnosis_artifacts: false,
            trace_only: true,
            ..Default::default()
        };
        let parent_collector =
            ralph_core::diagnostics::DiagnosticsCollector::with_options(repo_root, &parent_options)
                .unwrap();
        let parent_session = parent_collector.session_dir().unwrap();
        assert!(!parent_session.join("recovery.jsonl").exists());

        // Child: cwd=worktree, full diagnostics. Real session created.
        let child_options = ralph_core::diagnostics::DiagnosticsOptions {
            full_diagnostics: true,
            ..Default::default()
        };
        let child_collector = ralph_core::diagnostics::DiagnosticsCollector::with_options(
            &worktree_path,
            &child_options,
        )
        .unwrap();
        let child_session = child_collector.session_dir().unwrap();
        assert!(child_session.exists());
        // Full mode creates the historical loggers.
        assert!(child_session.join("orchestration.jsonl").exists());
        assert!(child_session.join("performance.jsonl").exists());

        // Sanity: child session is in the worktree, not the main repo.
        assert!(child_session.starts_with(&worktree_path));
    }
}
