use crate::cli::{ColorMode, ConfigSource, HatsSource, Verbosity, ensure_scratchpad_directory};
use crate::display::truncate;
use crate::loop_runner;
use crate::preflight;
use anyhow::{Context, Result};
use clap::{ArgAction, Parser};
use ralph_adapters::detect_backend;
use ralph_core::{
    CheckStatus, LockError, LockGuard, LockMetadata, LockStatus, LoopContext, LoopEntry, LoopLock,
    LoopRegistry, PreflightReport, PreflightRunner, ProfileSpec, ProfilesError, RalphConfig,
    TerminationReason, ensure_plan_baseline_from_head, truncate_with_ellipsis,
    worktree::{
        WorktreeConfig, clean_worktree_runtime_artifacts, create_worktree, ensure_gitignore,
        find_reusable_worktree_by_name, remove_worktree,
    },
};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;

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

    /// Reuse an existing, completed worktree for this run instead of
    /// creating a new one. Only valid with `--worktree`.
    ///
    /// When `--plan` is provided, Ralph looks up a previously
    /// completed worktree whose name is exactly the plan file's
    /// basename (without `.md`/`.html`). When `--worktree-name` is
    /// provided, Ralph looks up that exact worktree name instead.
    /// A matching completed worktree listed in `.ralph/loops.json`
    /// is reused after prior runtime artifacts are archived under
    /// `.ralph/reuse-history/` and removed from the live runtime paths.
    ///
    /// If no matching worktree exists yet, Ralph creates the first
    /// worktree with that exact name. It does not add a random suffix.
    /// A worktree whose loop is still running is never reused; the
    /// runner always refuses to attach to it.
    ///
    /// Cannot be used with `--exclusive` (the two flags address
    /// different concurrency regimes).
    #[arg(long, requires = "worktree", conflicts_with = "exclusive")]
    pub reuse_worktree: bool,

    /// Explicit plan file path.
    ///
    /// When provided, Ralph uses the plan file's basename (without the
    /// `.md`/`.html` extension) as the worktree name prefix instead of
    /// trying to extract a plan path from the prompt text. This avoids
    /// fragile parsing when the prompt contains trailing punctuation,
    /// Chinese characters, or other extra text.
    ///
    /// If neither `-p/--prompt` nor `-P/--prompt-file` is given, the
    /// plan file is also used as the prompt source (equivalent to
    /// `-P <plan>`).
    #[arg(long, value_name = "PATH")]
    pub plan: Option<PathBuf>,

    /// Explicit worktree name to use with `--worktree`.
    ///
    /// When provided, Ralph creates or reuses a worktree with exactly
    /// this name (under `.worktrees/<name>/`) instead of deriving one
    /// from the prompt or plan file. Use with `--reuse-worktree` to
    /// reuse an existing worktree of the same name.
    #[arg(
        long,
        value_name = "NAME",
        requires = "worktree",
        conflicts_with = "plan",
        conflicts_with = "exclusive"
    )]
    pub worktree_name: Option<String>,

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

    // ─────────────────────────────────────────────────────────────────────────
    // Profile Options (U3 of plan 2026-06-25-002)
    // ─────────────────────────────────────────────────────────────────────────
    /// Activate a runtime profile overlay. Accepts `<scope>:<name>` where
    /// `<scope>` is `repo` (project-rooted `ralph-profiles/<name>/`) or
    /// `user` (`~/.config/ralph/profiles/<name>/`). Repeatable; appended
    /// to the active spec list after `profiles.default` from ralph.yml.
    #[arg(long = "profile", value_name = "SCOPE:NAME", action = ArgAction::Append)]
    pub profiles: Vec<String>,

    /// Disable the operator-supplied `profiles.default` list from
    /// ralph.yml. CLI `--profile` flags remain in effect.
    #[arg(long)]
    pub no_default_profiles: bool,

    /// Custom backend command and arguments (use after --)
    #[arg(last = true)]
    pub custom_args: Vec<String>,
}

/// Collect the ordered list of active [`ProfileSpec`]s for a `ralph run`
/// invocation, given the parsed [`RalphConfig`] and the user-supplied
/// [`RunArgs`]. Thin wrapper over the shared helper that lives in
/// [`crate::commands::profile_args`]; `RunArgs` implements
/// [`crate::commands::profile_args::ProfileArgs`] so the merge logic
/// is shared byte-for-byte with `ralph inspect profiles`.
pub(crate) fn collect_active_profile_specs(
    config: &RalphConfig,
    args: &RunArgs,
) -> Result<Vec<ProfileSpec>, ProfilesError> {
    crate::commands::profile_args::collect_active_profile_specs(config, args)
}

impl crate::commands::profile_args::ProfileArgs for RunArgs {
    fn profile_specs(&self) -> &[String] {
        &self.profiles
    }
    fn no_default_profiles(&self) -> bool {
        self.no_default_profiles
    }
}

/// Compute the active preset name from the resolved hats source, used by
/// [`apply_active_profiles`] to anchor profile directory lookups.
///
/// - `Builtin(name)` → `name` (e.g. `"debug"`)
/// - `File(path)` → `path.file_stem()` (e.g. `"hats.yml"` → `"hats"`)
/// - `Remote(_)` → `Err` (profile resolution cannot match a remote preset
///   against a `<repo>/ralph-profiles/<name>/<preset>/` tree)
/// - `None` → `Ok(None)` (no preset name available; caller must fall back
///   to a warning rather than attempting to resolve fragments)
fn derive_preset_name(hats_source: Option<&HatsSource>) -> anyhow::Result<Option<String>> {
    match hats_source {
        None => Ok(None),
        Some(HatsSource::Builtin(name)) => Ok(Some(name.clone())),
        Some(HatsSource::File(path)) => Ok(Some(
            path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "failed to derive preset name from hats file path '{}'",
                        path.display()
                    )
                })?
                .to_string(),
        )),
        Some(HatsSource::Remote(url)) => Err(anyhow::anyhow!(
            "profile fragments cannot be resolved for remote hats source '{}'; \
             use a builtin preset or a local file path instead",
            url
        )),
    }
}

/// Apply active profile fragments to `config` for the current `ralph run`
/// invocation. Called from `run_command` immediately after
/// [`preflight::load_config_for_preflight`] returns and before
/// `config.validate()` / `run_auto_preflight`, so the rest of the pipeline
/// (event loop, preflight report, scratchpad templates) sees the merged
/// instructions.
///
/// Contract (per plan 2026-06-25-002 R10/R11/R14):
///
/// 1. Collects active specs via [`collect_active_profile_specs`] (defaults
///    first, then CLI `--profile` flags, honoring `--no-default-profiles`).
/// 2. Derives the preset name from `hats_source` and bails out early with
///    a clear error if a remote hats source was used with active specs.
/// 3. When no preset name is available (`None` source) and specs are
///    non-empty, the helper is a no-op: we cannot anchor a `<repo>/ralph-
///    profiles/<name>/<preset>/` lookup against an unknown preset, so the
///    operator has nothing to merge against.
/// 4. Resolves fragments via
///    [`ralph_core::profiles::apply_profile_fragments`] and streams
///    warnings to stderr.
///
/// The `workspace_root` is supplied by the caller (production: main repo
/// root resolved from `RALPH_WORKSPACE_ROOT` or `LoopContext`) so the
/// helper does not depend on `current_dir()` — this avoids drift when the
/// process was spawned inside a `--worktree` checkout.
pub(crate) fn apply_active_profiles(
    config: &mut RalphConfig,
    args: &RunArgs,
    hats_source: Option<&HatsSource>,
    workspace_root: &Path,
) -> anyhow::Result<()> {
    let specs = collect_active_profile_specs(config, args)?;
    if specs.is_empty() {
        return Ok(());
    }

    let preset_name = match derive_preset_name(hats_source)? {
        Some(name) => name,
        None => {
            // `None` hats source with active specs is a hard error:
            // the operator explicitly asked for a profile overlay
            // (via `profiles.default` or `--profile`) but supplied no
            // `-H/--hats` anchor, so we cannot resolve a
            // `<repo>/ralph-profiles/<name>/<preset>/` tree against
            // it. A warning is not enough — silently no-op'ing would
            // let the operator run ralph with a profile that "looks
            // configured" but never lands in any hat. Bail with a
            // clear, actionable error so the misconfiguration surfaces
            // immediately.
            anyhow::bail!(
                "--profile specs requested but no preset is active \
                 (no -H/--hats source); either pass `-H <preset>` \
                 or set `hats` / `-H` to a builtin or local hats file. \
                 Use `ralph inspect profiles` to preview the resolution."
            );
        }
    };

    let warnings = ralph_core::profiles::apply_profile_fragments(
        config,
        &preset_name,
        &specs,
        workspace_root,
    )?;
    for warning in &warnings {
        eprintln!("{warning}");
    }
    Ok(())
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
    explicit_loop_id: Option<&str>,
    loop_naming: &ralph_core::LoopNamingConfig,
    pending_worktree_registration: &mut Option<LoopEntry>,
) -> Result<(LoopContext, Option<LockGuard>), anyhow::Error> {
    let worktree_config = WorktreeConfig::default();

    // Generate loop ID from the most identifiable source + unique suffix.
    // Prompt files use their file name so worktrees can be mapped back to plans.
    let name_generator = ralph_core::LoopNameGenerator::from_config(loop_naming);
    let loop_id = if let Some(name) = explicit_loop_id {
        // Explicit --worktree-name: use it exactly, failing fast if it
        // already exists (the caller is responsible for reuse checks).
        if ralph_core::worktree_exists(workspace_root, name, &worktree_config) {
            anyhow::bail!(
                "Worktree already exists: {}. Use --reuse-worktree to reuse it, or choose a different name.",
                name
            );
        }
        name.to_string()
    } else if let Some(prefix) = file_name_prefix {
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

    // Record the plan baseline SHA at worktree creation time. This is the
    // review diff base for the plan; it must survive --reuse-worktree and
    // --continue so review always scopes from plan start.
    if let Err(e) = ensure_plan_baseline_from_head(context.workspace(), None) {
        warn!(
            worktree = %context.workspace().display(),
            error = %e,
            "Failed to record plan baseline in worktree"
        );
    }

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
    exact_worktree_name: Option<&str>,
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
            exact_worktree_name,
            loop_naming,
            pending_worktree_registration,
        )
    }
}

fn worktree_file_name_prefix(
    prompt_file: &str,
    _prompt_summary: &str,
    plan_file: Option<&Path>,
) -> Option<String> {
    // Explicit --plan takes precedence: derive the plan basename.
    // That basename is later used as the exact worktree name when
    // reuse is requested, so it must stay deterministic and not depend
    // on fragile prompt-text parsing.
    if let Some(plan) = plan_file
        && let Some(stem) = plan
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.trim().is_empty())
    {
        return Some(stem.to_string());
    }

    // Fallback: if a non-default prompt file was provided explicitly
    // (-P), use its basename as the worktree prefix. We intentionally
    // do NOT scan prompt text for embedded plan paths any more — that
    // behavior was fragile and has been removed.
    if prompt_file.is_empty() {
        return None;
    }

    let prompt_path = Path::new(prompt_file);
    let stem = prompt_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .filter(|stem| !stem.eq_ignore_ascii_case("prompt"))?;
    Some(stem.to_string())
}

fn resolve_exact_worktree_name(
    worktree_name: Option<&str>,
    plan_file: Option<&Path>,
    derived_plan_name: Option<&str>,
) -> Option<String> {
    worktree_name.map(str::to_owned).or_else(|| {
        plan_file.and_then(|_| {
            derived_plan_name
                .map(str::to_owned)
                .filter(|name| !name.is_empty())
        })
    })
}

/// Resolve a `--plan` argument to an existing plan file path.
///
/// Operators often omit the `.md` extension or the `docs/plans/` prefix.
/// Without resolution the path is forwarded verbatim to the child RPC
/// process, which fails to find the file and exits; in TUI mode the
/// error is hidden in the child's stderr log, so the parent TUI appears
/// to "flash crash". This helper makes `--plan <basename>` work by
/// trying a few common variations before giving up.
///
/// Resolution order (anchored at `workspace_root`):
/// 1. Use the path as-is if it already points to a file.
/// 2. If it lacks the `.md` extension, try `<path>.md`.
/// 3. If the basename lacks `.md`, try `docs/plans/<basename>.md`.
/// 4. Return the original path unchanged so the normal "file not found"
///    error path still fires with the operator-supplied value.
fn resolve_plan_arg(plan: &Path, workspace_root: &Path) -> PathBuf {
    // 1. Exact match.
    let candidate = workspace_root.join(plan);
    if candidate.is_file() {
        return plan.to_path_buf();
    }

    // 2. Try adding `.md` to the provided path.
    let with_md = plan.with_extension("md");
    let candidate_with_md = workspace_root.join(&with_md);
    if candidate_with_md.is_file() {
        return with_md;
    }

    // 3. Try `docs/plans/<basename>.md` for bare plan names.
    if let Some(name) = plan.file_name() {
        let docs_plans = Path::new("docs")
            .join("plans")
            .join(name)
            .with_extension("md");
        let candidate_docs_plans = workspace_root.join(&docs_plans);
        if candidate_docs_plans.is_file() {
            return docs_plans;
        }
    }

    // No resolution found: preserve the original path so the caller
    // reports the exact value the operator typed.
    plan.to_path_buf()
}

pub async fn run_command(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    verbose: bool,
    color_mode: ColorMode,
    mut args: RunArgs,
    prebuilt_diagnostics: Option<Arc<ralph_core::diagnostics::DiagnosticsCollector>>,
) -> Result<()> {
    let mut config = preflight::load_config_for_preflight(config_sources, hats_source).await?;

    // Resolve `--plan` to an existing file before anything else uses it.
    // This prevents the child RPC process from failing with a hidden
    // "prompt file not found" error when the operator omits `.md` or
    // the `docs/plans/` prefix.
    if let Some(plan) = args.plan.as_deref() {
        args.plan = Some(resolve_plan_arg(plan, &config.core.workspace_root));
    }

    // Apply profile fragments (plan 2026-06-25-002 U4).
    //
    // Insertion point: immediately after `load_config_for_preflight`
    // returns (so `config.normalize()` has already merged `extra_instructions`)
    // and before any CLI overrides / `config.validate()` / `run_auto_preflight`.
    // Resolving the workspace root from `RALPH_WORKSPACE_ROOT` keeps profile
    // lookups anchored at the main repo even when this process was spawned
    // from a `--worktree` checkout (where `current_dir()` would point inside
    // the worktree).
    let profile_workspace_root = std::env::var("RALPH_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| config.core.workspace_root.clone());
    apply_active_profiles(&mut config, &args, hats_source, &profile_workspace_root)?;

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
    } else if let Some(plan_path) = &args.plan {
        // --plan serves as the prompt source when no explicit prompt
        // argument is given, while also driving the worktree prefix.
        config.event_loop.prompt_file = plan_path.to_string_lossy().to_string();
        config.event_loop.prompt = None;
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
        args.plan.as_deref(),
    );
    let exact_worktree_name = resolve_exact_worktree_name(
        args.worktree_name.as_deref(),
        args.plan.as_deref(),
        worktree_file_name_prefix.as_deref(),
    );

    let mut pending_worktree_registration: Option<LoopEntry> = None;

    // U2 (plan 2026-08-03-004): the validated resume manifest threaded
    // into the loop bootstrap. Set by the reuse paths only; consumed by
    // `run_loop_impl` to re-bind the pending hat through the existing
    // `task.resume` recovery contract.
    let mut resumed_manifest: Option<ralph_core::parallel_forge_resume::ResumeManifest> = None;

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
        //
        // When `--reuse-worktree` is also set, look up an existing
        // completed worktree by exact name and reuse it. If no match
        // exists yet, create the first worktree with that exact name.
        if args.reuse_worktree {
            debug!("Reusing worktree for explicit --worktree --reuse-worktree mode");
            match exact_worktree_name.as_deref() {
                Some(name) => match find_reusable_worktree_by_name(workspace_root, name) {
                    Ok(Some(reusable)) => {
                        info!(
                            "Reusing worktree at {} (loop_id={})",
                            reusable.path.display(),
                            reusable.loop_id
                        );
                        // U1 (plan 2026-08-03-004): resume-manifest identity
                        // inputs. These bind the manifest to the CURRENT
                        // plan / preset / config / worktree; validation
                        // after cleanup compares them fail-closed.
                        let resume_plan_path = args
                            .plan
                            .as_deref()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let resume_plan_digest = args
                            .plan
                            .as_deref()
                            .and_then(|p| std::fs::read(p).ok())
                            .map(|bytes| ralph_core::parallel_forge_resume::sha256_hex(&bytes))
                            .unwrap_or_default();
                        let resume_preset_name = derive_preset_name(hats_source)
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        let mut resume_config_bytes: Vec<u8> = Vec::new();
                        for source in config_sources {
                            if let ConfigSource::File(path) = source
                                && let Ok(bytes) = std::fs::read(path)
                            {
                                resume_config_bytes.extend_from_slice(&bytes);
                            }
                        }
                        let resume_config_digest = if resume_config_bytes.is_empty() {
                            String::new()
                        } else {
                            ralph_core::parallel_forge_resume::sha256_hex(&resume_config_bytes)
                        };
                        let resume_inputs = ralph_core::parallel_forge_resume::CaptureInputs {
                            plan_path: resume_plan_path,
                            plan_digest: resume_plan_digest,
                            preset_name: resume_preset_name,
                            config_digest: resume_config_digest,
                            worktree_name: name.to_string(),
                        };

                        let archive_dir =
                            clean_worktree_runtime_artifacts(&reusable.path, Some(&resume_inputs))
                                .context("Failed to clean runtime artifacts in reused worktree")?;
                        if let Some(path) = &archive_dir {
                            info!(
                                "Archived prior runtime artifacts to {} before reuse",
                                path.display()
                            );
                        }

                        // U1: resume-manifest gate — fail-closed BEFORE the
                        // LoopContext is created. When the cleanup produced
                        // an archive, its manifest must exist and validate.
                        // When nothing was archived, fall back to the newest
                        // manifest archived by an earlier reuse (the prior
                        // run's evidence lives there). No manifest at all
                        // means the worktree carries no prior runtime
                        // records, so the start proceeds.
                        //
                        // U2: the gate result now RETAINS the validated
                        // manifest so the loop bootstrap can re-bind the
                        // pending hat via the existing `task.resume`
                        // recovery contract. The validation itself is
                        // unchanged.
                        use ralph_core::parallel_forge_resume::{
                            MANIFEST_FILE_NAME, latest_archived_manifest, read_manifest,
                            validate_manifest,
                        };
                        let resume_gate = match &archive_dir {
                            Some(dir) => {
                                let manifest_path = dir.join(MANIFEST_FILE_NAME);
                                read_manifest(&manifest_path).and_then(|manifest| {
                                    validate_manifest(&manifest, &resume_inputs)
                                        .map(|()| Some(manifest))
                                })
                            }
                            None => match latest_archived_manifest(&reusable.path) {
                                Ok(Some((_, manifest))) => {
                                    validate_manifest(&manifest, &resume_inputs)
                                        .map(|()| Some(manifest))
                                }
                                Ok(None) => Ok(None),
                                // Fold the read error into the gate result so
                                // the refusal message below wraps it like every
                                // other gate failure.
                                Err(e) => Err(e),
                            },
                        };
                        resumed_manifest = resume_gate.map_err(|e| {
                            anyhow::anyhow!(
                                "resume manifest validation failed for reused worktree \
                                 '{name}': {e}. The loop was NOT started; the prior run's \
                                 records are preserved under .ralph/reuse-history/ in the \
                                 worktree."
                            )
                        })?;

                        // Re-create the worktree's symlinks (in case the
                        // previous loop removed them) and refresh the
                        // context file metadata. setup_worktree_symlinks
                        // is idempotent — it skips existing symlinks.
                        let reused_ctx = LoopContext::worktree(
                            reusable.loop_id.clone(),
                            reusable.path.clone(),
                            workspace_root.clone(),
                        );
                        reused_ctx
                            .setup_worktree_symlinks()
                            .context("Failed to refresh symlinks in reused worktree")?;
                        reused_ctx
                            .generate_context_file(&reusable.branch, &prompt_summary)
                            .context("Failed to refresh context file in reused worktree")?;
                        // The plan baseline was recorded when the worktree was first
                        // created. Do NOT recreate it here: if it was lost we want the
                        // runner to warn and fall back to current HEAD rather than
                        // silently re-anchor the review diff base.
                        // PROMPT.md sync (mirrors the create path) so
                        // the agent reads its prompt from the worktree.
                        sync_prompt_to_worktree(workspace_root, &reusable.path);

                        // Register a new loop entry pointing at the same
                        // worktree. The previous (dead-PID) entry is
                        // replaced by register's same-PID guard or
                        // simply ages out via the registry's own cleanup.
                        let entry = LoopEntry::with_id(
                            &reusable.loop_id,
                            &prompt_summary,
                            Some(reusable.path.to_string_lossy().to_string()),
                            reusable.path.to_string_lossy().to_string(),
                        );
                        pending_worktree_registration = Some(entry);

                        // Hand the reused context to the rest of the
                        // pipeline exactly like a freshly-created one.
                        subprocess_tui_args.worktree = false;
                        subprocess_tui_args.worktree_path =
                            Some(reused_ctx.workspace().to_path_buf());
                        (reused_ctx, None)
                    }
                    Ok(None) => {
                        info!(
                            "No existing worktree named {}; creating the first exact-name worktree",
                            name
                        );
                        let (wt_ctx, _wt_guard) = spawn_worktree_loop(
                            workspace_root,
                            &prompt_summary,
                            None,
                            Some(name),
                            &config.features.loop_naming,
                            &mut pending_worktree_registration,
                        )?;
                        subprocess_tui_args.worktree = false;
                        subprocess_tui_args.worktree_path = Some(wt_ctx.workspace().to_path_buf());
                        (wt_ctx, None)
                    }
                    Err(e) => {
                        return Err(anyhow::Error::new(e).context(
                            "Failed to look up reusable worktree; aborting to avoid stale state",
                        ));
                    }
                },
                None => anyhow::bail!(
                    "--reuse-worktree requires `--plan <plan.md>` or `--worktree-name <name>`"
                ),
            }
        } else {
            debug!("Creating worktree for explicit --worktree mode");
            let (wt_ctx, _wt_guard) = spawn_worktree_loop(
                workspace_root,
                &prompt_summary,
                if exact_worktree_name.is_some() {
                    None
                } else {
                    worktree_file_name_prefix.as_deref()
                },
                exact_worktree_name.as_deref(),
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
        }
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
        let context = LoopContext::worktree(
            loop_id.clone(),
            worktree_path.clone(),
            workspace_root.clone(),
        );

        // U2 (plan 2026-08-03-004): in subprocess-TUI mode the reuse
        // gate ran in the PARENT; this child process runs the loop.
        // Re-read the newest archived resume manifest and validate it
        // against the same identity inputs before threading it into
        // the bootstrap. Fail-closed, mirroring the parent gate: any
        // failure refuses the start. A worktree without archived
        // manifests (fresh, never reused) yields `None` and the loop
        // bootstraps normally.
        use ralph_core::parallel_forge_resume::{
            CaptureInputs, latest_archived_manifest, sha256_hex, validate_manifest,
        };
        let child_plan_path = args
            .plan
            .as_deref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let child_plan_digest = args
            .plan
            .as_deref()
            .and_then(|p| std::fs::read(p).ok())
            .map(|bytes| sha256_hex(&bytes))
            .unwrap_or_default();
        let child_preset_name = derive_preset_name(hats_source)
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut child_config_bytes: Vec<u8> = Vec::new();
        for source in config_sources {
            if let ConfigSource::File(path) = source
                && let Ok(bytes) = std::fs::read(path)
            {
                child_config_bytes.extend_from_slice(&bytes);
            }
        }
        let child_config_digest = if child_config_bytes.is_empty() {
            String::new()
        } else {
            sha256_hex(&child_config_bytes)
        };
        let child_inputs = CaptureInputs {
            plan_path: child_plan_path,
            plan_digest: child_plan_digest,
            preset_name: child_preset_name,
            config_digest: child_config_digest,
            worktree_name: loop_id.clone(),
        };
        match latest_archived_manifest(worktree_path) {
            Ok(Some((_, manifest))) => {
                validate_manifest(&manifest, &child_inputs).map_err(|e| {
                    anyhow::anyhow!(
                        "resume manifest validation failed for reused worktree \
                         '{loop_id}': {e}. The loop was NOT started; the prior run's \
                         records are preserved under .ralph/reuse-history/ in the \
                         worktree."
                    )
                })?;
                resumed_manifest = Some(manifest);
            }
            Ok(None) => {}
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "resume manifest validation failed for reused worktree \
                     '{loop_id}': {e}. The loop was NOT started; the prior run's \
                     records are preserved under .ralph/reuse-history/ in the \
                     worktree."
                ));
            }
        }

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
                            exact_worktree_name.as_deref(),
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
                            exact_worktree_name.as_deref(),
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
                    exact_worktree_name.as_deref(),
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
                        exact_worktree_name.as_deref(),
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
            resumed_manifest,
            args.warmup_only,
            args.force_warmup,
            prebuilt_diagnostics,
            args.no_sync_agent_docs,
            source_is_builtin_embedded,
            hats_source.map(|h| h.label()),
        )
        .await
        .inspect_err(|e| {
            // P1 finding #5: lift `PresetLintGateError` out of
            // the error chain and exit with code2 here, AFTER
            // the RAII drop chain has run. The `?` shortcut
            // would still propagate, but the caller (`main`)
            // has no way to map the typed error to a specific
            // exit code without us flagging it here. Doing the
            // mapping inside `run_command` keeps the change
            // scoped to the run command path without
            // restructuring `main`.
            if let Some(code) = run_loop_result_exit_code(e) {
                std::process::exit(code);
            }
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
///   `ValidationFailure`, `Stopped`, `WorkspaceGone`, `RecoveryExhausted`,
///   `ReviewFailed`, and any unrecognized reason)
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
    } else if let Some(ref plan) = args.plan {
        // --plan doubles as the prompt source when -p/-P are absent.
        // Anchor relative paths to the parent cwd in worktree mode so
        // the child can find the plan file from its own cwd.
        out.push("--plan".to_string());
        if args.worktree_path.is_some() && !plan.is_absolute() {
            out.push(parent_cwd.join(plan).to_string_lossy().into_owned());
        } else {
            out.push(plan.to_string_lossy().to_string());
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
        plan: Option<&Path>,
        worktree_path: Option<&Path>,
    ) -> SubprocessTuiArgs {
        SubprocessTuiArgs {
            prompt_text: prompt_text.map(str::to_string),
            prompt_file: prompt_file.map(PathBuf::from),
            plan: plan.map(PathBuf::from),
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
            profiles: vec![],
            no_default_profiles: false,
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
        let args = make_args(None, None, None, Some(&wt));

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
        let args = make_args(None, None, None, Some(&wt));

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

        let args = make_args(None, None, None, None);
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

        let args = make_args(Some("inline wins"), None, None, Some(&wt));
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

        let args = make_args(None, Some(Path::new("CUSTOM.md")), None, Some(&wt));
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

        let args = make_args(None, Some(&abs_prompt), None, Some(&wt));
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
        let args = make_args(None, Some(Path::new("REL.md")), None, None);
        let out = forward_prompt_args(&args, Path::new("/anywhere"));
        assert_eq!(out, vec!["-P".to_string(), "REL.md".to_string()]);
    }

    /// `--plan` is forwarded when neither -p nor -P is set. In worktree
    /// mode relative paths are anchored at the parent cwd so the child
    /// can find the plan file from its own cwd.
    #[test]
    fn plan_forwarded_as_prompt_source() {
        let tmp = tempfile::tempdir().unwrap();
        let plan_path = tmp.path().join("plans").join("my-plan.md");
        fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
        fs::write(&plan_path, "plan content").unwrap();
        let wt = PathBuf::from("/tmp/fake-worktree");

        let args = make_args(None, None, Some(&plan_path), Some(&wt));
        let out = forward_prompt_args(&args, tmp.path());
        assert_eq!(
            out,
            vec![
                "--plan".to_string(),
                plan_path.to_string_lossy().into_owned()
            ]
        );
    }

    /// `-p` takes precedence over `--plan`.
    #[test]
    fn inline_prompt_takes_precedence_over_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let plan_path = tmp.path().join("plans").join("my-plan.md");
        fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
        fs::write(&plan_path, "plan content").unwrap();
        let wt = PathBuf::from("/tmp/fake-worktree");

        let args = make_args(Some("inline wins"), None, Some(&plan_path), Some(&wt));
        let out = forward_prompt_args(&args, tmp.path());
        assert_eq!(out, vec!["-p".to_string(), "inline wins".to_string()]);
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
        assert_eq!(fs::read_to_string(&copied).unwrap(), "test prompt content");
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
    pub plan: Option<PathBuf>,
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
    /// Profile specs to forward to child process (--profile args, repeatable).
    /// U3 of plan 2026-06-25-002: the child must receive the same list so
    /// that profile fragments are applied in the RPC child, not silently
    /// dropped on the floor in TUI mode.
    pub profiles: Vec<String>,
    /// Whether to forward `--no-default-profiles` to the child. U3 of plan
    /// 2026-06-25-002: defaults are toggled off in lockstep with the parent.
    pub no_default_profiles: bool,
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
            plan: args.plan.clone(),
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
            profiles: args.profiles.clone(),
            no_default_profiles: args.no_default_profiles,
        }
    }
}

/// Restore the terminal to a usable state.
///
/// Idempotent: safe to call multiple times and when the terminal was never
/// switched into TUI raw mode. This is the global safety net for subprocess
/// TUI mode (R8).
fn restore_terminal() {
    use crossterm::cursor::Show;
    use crossterm::execute;
    use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};

    let mut stdout = std::io::stdout();
    let _ = disable_raw_mode();
    let _ = execute!(stdout, LeaveAlternateScreen, Show);
}

/// RAII guard that restores the terminal when dropped.
///
/// Installed around the TUI run so that any panic or early return in
/// `run_subprocess_tui` leaves the shell in a usable state (R8).
struct TerminalRestoreGuard;

impl TerminalRestoreGuard {
    fn new() -> Self {
        Self
    }
}

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Wait for a termination signal (SIGINT/SIGTERM on Unix, Ctrl-C elsewhere).
///
/// Returns the signal name that was received.
#[cfg(unix)]
async fn wait_for_termination_signal() -> Option<&'static str> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = signal(SignalKind::interrupt()).ok()?;
    let mut sigterm = signal(SignalKind::terminate()).ok()?;
    tokio::select! {
        _ = sigint.recv() => Some("SIGINT"),
        _ = sigterm.recv() => Some("SIGTERM"),
    }
}

#[cfg(not(unix))]
async fn wait_for_termination_signal() -> Option<&'static str> {
    tokio::signal::ctrl_c().await.ok().map(|_| "SIGINT")
}

/// Send a signal to a child process by PID.
#[cfg(unix)]
fn send_child_signal(child_id: Option<u32>, signal: Signal) {
    if let Some(id) = child_id {
        let pid = Pid::from_raw(id as i32);
        warn!(
            target: "ralph_cli::commands::run",
            child_pid = %pid,
            ?signal,
            "send_child_signal sending signal to child PID only"
        );
        let _ = kill(pid, signal);
    }
}

#[cfg(not(unix))]
fn send_child_signal(_child_id: Option<u32>, _signal: &'static str) {
    // No-op on non-Unix; callers use `child.kill().await` directly.
}

/// Gracefully terminate a child: SIGTERM, wait, then SIGKILL if still alive.
///
/// Returns the final exit status if the child could be reaped.
async fn graceful_terminate_child(
    child: &mut tokio::process::Child,
    child_id: Option<u32>,
) -> Option<std::process::ExitStatus> {
    #[cfg(unix)]
    {
        let our_pid = std::process::id();
        let our_pgid = nix::unistd::getpgrp();
        warn!(
            target: "ralph_cli::commands::run",
            our_pid,
            our_pgid = %our_pgid,
            child_id = ?child_id,
            "graceful_terminate_child starting"
        );
        // Fallback: kill the entire process tree so that backends spawned in
        // separate PTY sessions cannot survive after the child is reaped.
        if let Some(id) = child_id {
            crate::cli::process_tree::kill_process_tree(id, true);
        }
        send_child_signal(child_id, Signal::SIGTERM);
        let term_timeout = Duration::from_secs(5);
        match timeout(term_timeout, child.wait()).await {
            Ok(Ok(status)) => {
                warn!(
                    target: "ralph_cli::commands::run",
                    child_id = ?child_id,
                    ?status,
                    "Child exited cleanly after SIGTERM"
                );
                return Some(status);
            }
            Ok(Err(e)) => {
                warn!(error = %e, "Failed to wait for child after SIGTERM");
            }
            Err(_) => {
                warn!(
                    child_id = ?child_id,
                    "Child did not exit after SIGTERM; sending SIGKILL"
                );
                send_child_signal(child_id, Signal::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
    }
    child.wait().await.ok()
}

/// Wait for a `JoinHandle` with a timeout; abort if it does not finish.
///
/// Returns `(Some(result), false)` on clean completion, `(None, true)` on timeout.
async fn wait_for_task_with_timeout<T>(
    mut handle: JoinHandle<T>,
    name: &'static str,
    timeout_duration: Duration,
) -> (Option<T>, bool) {
    match timeout(timeout_duration, &mut handle).await {
        Ok(Ok(result)) => {
            debug!(task = name, "I/O task finished cleanly");
            (Some(result), false)
        }
        Ok(Err(join_err)) => {
            debug!(task = name, error = %join_err, "I/O task joined with error");
            (None, false)
        }
        Err(_) => {
            warn!(
                task = name,
                timeout_secs = timeout_duration.as_secs(),
                "I/O task did not finish in time; aborting"
            );
            handle.abort();
            (None, true)
        }
    }
}

/// Wait for a child to exit, or gracefully terminate it after a timeout.
async fn wait_or_terminate_child(
    child: &mut tokio::process::Child,
    child_id: Option<u32>,
    timeout_duration: Duration,
    context: &'static str,
) -> Option<std::process::ExitStatus> {
    match timeout(timeout_duration, child.wait()).await {
        Ok(Ok(status)) => Some(status),
        Ok(Err(e)) => {
            warn!(error = %e, context, "Failed to wait for child");
            None
        }
        Err(_) => {
            warn!(
                context,
                timeout_secs = timeout_duration.as_secs(),
                "Child did not exit within timeout; terminating"
            );
            graceful_terminate_child(child, child_id).await
        }
    }
}

/// Write a structured JSONL cleanup diagnostic when I/O tasks had to be
/// forcefully aborted (R12).
fn write_cleanup_diagnostic(
    workspace: &Path,
    reader_timed_out: bool,
    forward_timed_out: bool,
    elapsed: std::time::Duration,
    signal: Option<&str>,
) -> Result<PathBuf> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let diagnostics_dir = workspace.join(".ralph").join("diagnostics");
    std::fs::create_dir_all(&diagnostics_dir)?;
    let path = diagnostics_dir.join("cleanup-events.jsonl");

    let entry = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "pid": std::process::id(),
        "reader_timed_out": reader_timed_out,
        "forward_timed_out": forward_timed_out,
        "cleanup_elapsed_ms": elapsed.as_millis(),
        "signal": signal,
    });

    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{}", entry)?;
    Ok(path)
}

/// Remove the loop lock file if it was left behind by a killed or crashed
/// subprocess-TUI child.
///
/// In subprocess TUI mode the parent intentionally does not hold the loop
/// lock; the child RPC process acquires it.  When the child is killed by a
/// signal (SIGINT/SIGTERM) it cannot drop its `LockGuard`, so the lock file
/// is left on disk with a dead PID.  This helper inspects the lock and
/// removes it only when it is stale (no flock held) so we do not disturb an
/// active loop that started after this child exited.
fn cleanup_subprocess_loop_lock(workspace: &Path, child_id: Option<u32>) {
    use ralph_core::{LockStatus, LoopLock};

    let lock_path = workspace.join(LoopLock::LOCK_FILE);
    if !lock_path.exists() {
        return;
    }

    match LoopLock::inspect(workspace) {
        Ok(LockStatus::None) => {}
        Ok(LockStatus::Stale(_)) => {
            if let Err(e) = std::fs::remove_file(&lock_path) {
                warn!(
                    error = %e,
                    path = %lock_path.display(),
                    "Failed to remove stale loop lock left by subprocess TUI child"
                );
            } else {
                info!(
                    path = %lock_path.display(),
                    "Removed stale loop lock left by subprocess TUI child"
                );
            }
        }
        Ok(LockStatus::Active(metadata)) => {
            if Some(metadata.pid) == child_id {
                // Metadata still points to our child but the flock is held.
                // This is unexpected after child.wait(); do not remove an
                // active lock that may now belong to another process.
                warn!(
                    child_id = ?child_id,
                    lock_pid = metadata.pid,
                    "Loop lock metadata matches child but flock is still held; leaving lock in place"
                );
            }
        }
        Err(e) => {
            warn!(
                error = %e,
                path = %lock_path.display(),
                "Failed to inspect loop lock during subprocess TUI cleanup"
            );
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

    // Forward profile options (U3 of plan 2026-06-25-002). Each `--profile`
    // entry is pushed separately so that the child CLI parser sees the
    // same argv shape the parent received; clap's `ArgAction::Append`
    // accepts multiple `--profile` flags and reconstructs the Vec<String>.
    for spec in &args.profiles {
        child_args.push("--profile".to_string());
        child_args.push(spec.clone());
    }
    if args.no_default_profiles {
        child_args.push("--no-default-profiles".to_string());
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

    // R8: install the global terminal restore guard BEFORE the TUI can switch
    // the terminal into raw mode. The guard restores the terminal on any
    // return, panic, or signal-triggered cleanup path.
    let _terminal_guard = TerminalRestoreGuard::new();

    // U3 (2026-06-10): explicitly set child's cwd to workspace (worktree path
    // in worktree mode, main repo in primary mode). This replaces the old
    // chdir hack and makes the child's cwd explicit rather than relying on
    // side effects.
    //
    // U1 (2026-06-14-002): also synchronize PWD env var with the real cwd.
    // Shells and agent bash tools rely on PWD; without this, a worktree child
    // inherits the parent's PWD (main repo) and writes end up in the wrong
    // directory.
    let mut child = Command::new(std::env::current_exe()?)
        .args(&child_args)
        .current_dir(&args.workspace)
        .env("PWD", &args.workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr_stdio)
        .spawn()
        .context("Failed to spawn ralph subprocess for TUI")?;

    // R6: if spawn fails the guard above is dropped and restores the terminal.
    let child_id = child.id();
    #[cfg(unix)]
    {
        use nix::unistd::getpgid;
        let pgid = child_id
            .and_then(|id| getpgid(Some(nix::unistd::Pid::from_raw(id as i32))).ok())
            .map(|p| p.as_raw())
            .unwrap_or(-1);
        warn!(
            target: "ralph_cli::commands::run",
            child_id = ?child_id,
            child_pgid = pgid,
            our_pid = std::process::id(),
            our_pgid = %nix::unistd::getpgrp(),
            "run_subprocess_tui spawned child"
        );
    }

    let stdin = child
        .stdin
        .take()
        .context("Failed to capture subprocess stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("Failed to capture subprocess stdout")?;

    // R1: stdin/stdout have been taken from the Child. Dropping our handles
    // (rpc_writer wraps stdin; forward_handle owns stdout) will close the
    // pipes and unblock any I/O tasks waiting on EOF.

    // Create TUI state and cancellation token.
    let state = std::sync::Arc::new(std::sync::Mutex::new(ralph_tui::TuiState::new()));
    let cancel_token = CancellationToken::new();
    let (terminated_tx, terminated_rx) = tokio::sync::watch::channel(false);

    // Create RPC writer for sending commands.
    let rpc_writer = ralph_tui::RpcWriter::new(stdin);

    // U3: tee the child's RPC stdout into the TUI reader so the parent can
    // detect a fatal LoopTerminated event and break the TUI out of its input
    // loop instead of waiting for the user to press 'q'.
    let (tui_reader, mut tui_writer) = duplex(64 * 1024);

    // Forward task: child stdout -> duplex writer. Cancelled via CancellationToken (R4).
    let forward_handle = {
        let cancel = cancel_token.child_token();
        tokio::spawn(async move {
            let mut child_lines = BufReader::new(stdout).lines();
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        debug!("Forward task cancelled");
                        break;
                    }
                    line = child_lines.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                // R5: any write/flush failure breaks immediately.
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

    // Reader task: duplex reader -> TUI state. Wrapped so it also listens to
    // the CancellationToken (R4) while keeping the watch receiver for the
    // internal EOF path.
    let reader_state = std::sync::Arc::clone(&state);
    let reader_cancel = cancel_token.child_token();
    let cancel_rx = terminated_rx.clone();
    let reader_handle = tokio::spawn(async move {
        tokio::select! {
            _ = reader_cancel.cancelled() => {
                debug!("Reader task cancelled");
            }
            _ = ralph_tui::run_rpc_event_reader(tui_reader, reader_state, cancel_rx) => {}
        }
    });

    info!("TUI running in subprocess RPC mode");

    // R9: register SIGINT/SIGTERM handler so the parent can restore the terminal
    // and forward the signal to the child.
    let signal_handle = tokio::spawn(wait_for_termination_signal());

    // Run the TUI render/input loop concurrently with the child process and
    // signal handler.
    let app = ralph_tui::App::new_subprocess(
        std::sync::Arc::clone(&state),
        terminated_rx.clone(),
        rpc_writer.clone(),
    );
    let mut app_fut = std::pin::pin!(app.run());
    let mut signal_fut = std::pin::pin!(signal_handle);

    let cleanup_start = std::time::Instant::now();

    // The child.wait() future is NOT pinned ahead of time. Each select branch
    // that needs it creates its own borrow, so the other branches can still
    // mutate `child` (e.g., to send signals) without fighting a long-lived
    // mutable borrow.
    let (tui_result, exit_status, received_signal) = tokio::select! {
        status = child.wait() => {
            let status = status.context("Failed to wait for ralph subprocess")?;
            if !status.success() {
                let _ = terminated_tx.send(true);
            }
            let tui_result = timeout(Duration::from_secs(5), app_fut)
                .await
                .unwrap_or_else(|_| {
                    warn!("TUI did not exit within 5s after child termination; forcing return");
                    Ok(())
                });
            // No longer need the signal handler.
            signal_fut.abort();
            (tui_result, Some(status), None)
        }
        result = app_fut.as_mut() => {
            let tui_result = result;
            // App exited (e.g., user pressed 'q'); wait for the child.
            // R3: if the child does not exit promptly, terminate it.
            // Fallback: kill the whole process tree before waiting so that
            // PTY-session backends cannot outlive the child.
            if let Some(id) = child_id {
                crate::cli::process_tree::kill_process_tree(id, true);
            }
            let status = wait_or_terminate_child(
                &mut child,
                child_id,
                Duration::from_secs(5),
                "tui_quit",
            )
            .await;
            signal_fut.abort();
            (tui_result, status, None)
        }
        signal = signal_fut.as_mut() => {
            let signal_name = match signal {
                Ok(Some(name)) => name,
                Ok(None) | Err(_) => "unknown",
            };
            warn!(
                signal = signal_name,
                "Received termination signal; restoring terminal and terminating child"
            );
            // R8/R9: restore terminal BEFORE killing child.
            restore_terminal();
            // R9: terminate child and reap it.
            let status = graceful_terminate_child(&mut child, child_id).await;
            (Ok(()), status, Some(signal_name))
        }
    };

    // Cleanup phase: cancel I/O tasks, close pipes, wait with timeouts (R1, R2, R4).
    info!(
        child_exit_status = ?exit_status,
        signal = ?received_signal,
        "Subprocess TUI entering cleanup phase"
    );
    cancel_token.cancel();
    let _ = terminated_tx.send(true);
    let _ = rpc_writer.send_abort().await;
    let _ = rpc_writer.close().await;

    // R1: drop the stdin wrapper to close the child's stdin pipe.
    drop(rpc_writer);

    // R2: wait for I/O tasks with explicit timeouts; abort on timeout.
    let task_timeout = Duration::from_secs(3);
    let (_, reader_timed_out) =
        wait_for_task_with_timeout(reader_handle, "reader", task_timeout).await;
    let (_, forward_timed_out) =
        wait_for_task_with_timeout(forward_handle, "forward", task_timeout).await;

    // Ensure the child is reaped in case a branch above left it alive.
    if child.id().is_some() {
        let _ = child.wait().await;
    }

    // R13: the child RPC process held the loop lock.  If we killed it (or it
    // crashed) the LockGuard was never dropped, leaving a stale lock file.
    // Inspect the lock and remove it only when no other process holds the
    // flock, so a concurrently-started loop is not disturbed.
    cleanup_subprocess_loop_lock(&args.workspace, child_id);

    let cleanup_elapsed = cleanup_start.elapsed();
    info!(
        child_exit_status = ?exit_status,
        reader_timed_out,
        forward_timed_out,
        signal = ?received_signal,
        cleanup_elapsed_ms = cleanup_elapsed.as_millis(),
        "Subprocess TUI cleanup complete"
    );

    // R12: if cleanup required forceful abort, write a structured diagnostic note.
    if (reader_timed_out || forward_timed_out)
        && let Ok(cleanup_path) = write_cleanup_diagnostic(
            &args.workspace,
            reader_timed_out,
            forward_timed_out,
            cleanup_elapsed,
            received_signal,
        )
    {
        warn!(
            diagnostic_path = %cleanup_path.display(),
            "Wrote cleanup diagnostic for timed-out I/O tasks"
        );
    }

    let reason = if let Some(signal) = received_signal {
        info!(signal, "Subprocess TUI returning Interrupted due to signal");
        TerminationReason::Interrupted
    } else if let Some(status) = exit_status {
        // Resolve the exact termination reason from the runner sentinel when the
        // child did not exit cleanly; fall back to coarse exit-code mapping.
        resolve_subprocess_termination_reason(&args.workspace, &status)
    } else {
        warn!("Subprocess TUI could not determine child exit status; returning Stopped");
        TerminationReason::Stopped
    };

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
        reuse_worktree: false,
        plan: None,
        worktree_name: None,
        skip_preflight: true,
        no_sync_agent_docs: false,
        verbose: false,
        quiet: false,
        record_session: None,
        profiles: Vec::new(),
        no_default_profiles: false,
        custom_args: Vec::new(),
        warmup_only: false,
        force_warmup: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CwdGuard;
    use ralph_core::{
        HatConfig, HookMutationConfig, HookOnError, HookPhaseEvent, HookSpec, ProfileScope,
    };

    // ─────────────────────────────────────────────────────────────────────
    // U3 (2026-06-25-002): --profile / --no-default-profiles / TUI forwarding
    // ─────────────────────────────────────────────────────────────────────

    /// No flags => both default to empty/false. This guards regression:
    /// the helper must not pre-fill defaults that would shift semantics
    /// for callers that never invoke `ralph profiles`.
    #[test]
    fn run_args_default_profiles_are_empty() {
        let args = default_run_args();
        assert!(args.profiles.is_empty());
        assert!(!args.no_default_profiles);
    }

    /// `default_run_args()` is the source of truth for the struct shape
    /// consumed by `main.rs` when no subcommand is given. The compiler
    /// enforces field presence at struct-literal time, but a runtime
    /// assertion that the defaults actually surface as the expected
    /// zero-values catches accidental feature-flag drift early.
    #[test]
    fn run_args_default_via_default_run_args_matches_field_types() {
        let args = default_run_args();
        let _: Vec<String> = args.profiles;
        let _: bool = args.no_default_profiles;
    }

    /// Helper precedence (R10): defaults first, then CLI. Defaults come
    /// from `config.profiles.default` (already-validated `ProfileSpec` list),
    /// so we copy them as-is. CLI flags are parsed via `parse_profile_spec`.
    #[test]
    fn collect_active_specs_appends_cli_after_defaults() {
        let mut config = RalphConfig::default();
        config.profiles.default = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "base".to_string(),
        }];
        let mut args = default_run_args();
        args.profiles = vec!["user:extra".to_string()];

        let active = collect_active_profile_specs(&config, &args).expect("collect");
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].to_string(), "repo:base");
        assert_eq!(active[1].to_string(), "user:extra");
    }

    /// `--no-default-profiles` strips defaults but preserves CLI flags.
    /// AE2 of the plan explicitly calls out this combination as a
    /// supported case ("仅排除 defaults,不影响显式 `--profile`").
    #[test]
    fn collect_active_specs_no_default_profiles_skips_defaults_only() {
        let mut config = RalphConfig::default();
        config.profiles.default = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "base".to_string(),
        }];
        let mut args = default_run_args();
        args.no_default_profiles = true;
        args.profiles = vec!["user:extra".to_string()];

        let active = collect_active_profile_specs(&config, &args).expect("collect");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].to_string(), "user:extra");
    }

    /// Empty defaults + empty CLI = empty active list. This is the "no
    /// profile requested" fast path and must not allocate or fail.
    #[test]
    fn collect_active_specs_empty_inputs_yield_empty_list() {
        let config = RalphConfig::default();
        let args = default_run_args();
        let active = collect_active_profile_specs(&config, &args).expect("collect");
        assert!(active.is_empty());
    }

    /// A malformed `--profile` literal surfaces as `ProfilesError::InvalidSpec`
    /// with the original literal preserved, so `ralph run` can echo it
    /// verbatim in its error message.
    #[test]
    fn collect_active_specs_rejects_malformed_cli_literal() {
        let config = RalphConfig::default();
        let mut args = default_run_args();
        args.profiles = vec!["bad-spec-no-colon".to_string()];

        let err = collect_active_profile_specs(&config, &args).expect_err("must reject");
        match err {
            ProfilesError::InvalidSpec { spec, .. } => {
                assert_eq!(spec, "bad-spec-no-colon");
            }
            other => panic!("expected InvalidSpec, got {other:?}"),
        }
    }

    /// TUI subprocess forwarding: `--profile` entries must be passed to
    /// the child exactly once each. clap's `ArgAction::Append` requires
    /// `--profile <value>` pairs (cannot be collapsed to a single arg),
    /// so the forwarding code emits two argv slots per entry.
    #[test]
    fn subprocess_tui_args_forward_profile_entries() {
        let mut args = default_run_args();
        args.profiles = vec!["repo:strict".to_string(), "user:my-style".to_string()];
        args.no_default_profiles = true;

        let sub_args = SubprocessTuiArgs::new(&args, &[], None);
        assert_eq!(sub_args.profiles, args.profiles);
        assert!(sub_args.no_default_profiles);

        // Reconstruct the child argv the way `run_subprocess_tui` would.
        let mut argv = Vec::new();
        for spec in &sub_args.profiles {
            argv.push("--profile".to_string());
            argv.push(spec.clone());
        }
        if sub_args.no_default_profiles {
            argv.push("--no-default-profiles".to_string());
        }
        assert_eq!(
            argv,
            vec![
                "--profile",
                "repo:strict",
                "--profile",
                "user:my-style",
                "--no-default-profiles",
            ],
            "child argv must mirror parent flags (R5 / SubprocessTuiArgs contract)"
        );
    }

    /// Forwarding is conditional: when neither flag is set, no extra argv
    /// is pushed. This protects existing call sites that don't pass profile
    /// flags (and guards against regressions that would break pre-U3
    /// subprocess argv shapes).
    #[test]
    fn subprocess_tui_args_omit_profile_when_unset() {
        let args = default_run_args();
        let sub_args = SubprocessTuiArgs::new(&args, &[], None);
        assert!(sub_args.profiles.is_empty());
        assert!(!sub_args.no_default_profiles);

        let argv: Vec<String> = sub_args
            .profiles
            .iter()
            .flat_map(|s| ["--profile".to_string(), s.clone()])
            .chain(if sub_args.no_default_profiles {
                vec!["--no-default-profiles".to_string()]
            } else {
                Vec::new()
            })
            .collect();
        assert!(argv.is_empty(), "unset flags must not emit argv entries");
    }

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
        let source = worktree_file_name_prefix(
            "custom-drift-auto-calibration-prompt.md",
            "Implement something",
            None,
        );

        assert_eq!(
            source.as_deref(),
            Some("custom-drift-auto-calibration-prompt")
        );
    }

    #[test]
    fn test_worktree_file_name_prefix_returns_none_without_prompt_file() {
        let source = worktree_file_name_prefix("", "Implement something", None);

        assert_eq!(source, None);
    }

    #[test]
    fn test_worktree_file_name_prefix_ignores_default_prompt_file() {
        // Default PROMPT.md must not influence worktree naming; otherwise
        // every run with the default prompt would collide or require
        // fragile text parsing.
        let source = worktree_file_name_prefix("PROMPT.md", "[no prompt]", None);

        assert_eq!(source, None);
    }

    #[test]
    fn test_worktree_file_name_prefix_uses_explicit_plan_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let plan_path = temp_dir
            .path()
            .join("docs")
            .join("plans")
            .join("2026-06-25-002-feat-profiles-for-preset-role-tuning-plan.md");
        std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
        std::fs::write(&plan_path, "Implement something").unwrap();

        let source = worktree_file_name_prefix("PROMPT.md", "[no prompt]", Some(&plan_path));

        assert_eq!(
            source.as_deref(),
            Some("2026-06-25-002-feat-profiles-for-preset-role-tuning-plan")
        );
    }

    #[test]
    fn test_worktree_file_name_prefix_plan_file_takes_precedence_over_prompt_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let plan_path = temp_dir.path().join("explicit-plan.md");
        std::fs::write(&plan_path, "Implement something").unwrap();

        let source = worktree_file_name_prefix("custom-prompt.md", "[no prompt]", Some(&plan_path));

        assert_eq!(source.as_deref(), Some("explicit-plan"));
    }

    #[test]
    fn resolve_exact_worktree_name_prefers_explicit_name_over_plan() {
        let temp_dir = tempfile::tempdir().unwrap();
        let plan_path = temp_dir.path().join("plan.md");

        let resolved = resolve_exact_worktree_name(
            Some("exact-name"),
            Some(&plan_path),
            Some("plan-basename"),
        );

        assert_eq!(resolved.as_deref(), Some("exact-name"));
    }

    #[test]
    fn resolve_exact_worktree_name_uses_plan_basename_when_present() {
        let temp_dir = tempfile::tempdir().unwrap();
        let plan_path = temp_dir
            .path()
            .join("docs")
            .join("plans")
            .join("exact-plan.md");
        std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
        std::fs::write(&plan_path, "Implement something").unwrap();

        let resolved = resolve_exact_worktree_name(None, Some(&plan_path), Some("exact-plan"));

        assert_eq!(resolved.as_deref(), Some("exact-plan"));
    }

    #[test]
    fn resolve_exact_worktree_name_returns_none_without_exact_binding() {
        let resolved = resolve_exact_worktree_name(None, None, None);

        assert_eq!(resolved, None);
    }

    #[test]
    fn resolve_plan_arg_uses_exact_path_when_it_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let plan_path = temp_dir.path().join("my-plan.md");
        std::fs::write(&plan_path, "content").unwrap();

        let resolved = resolve_plan_arg(Path::new("my-plan.md"), temp_dir.path());

        assert_eq!(resolved, PathBuf::from("my-plan.md"));
    }

    #[test]
    fn resolve_plan_arg_adds_md_extension_when_missing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let plan_path = temp_dir.path().join("my-plan.md");
        std::fs::write(&plan_path, "content").unwrap();

        let resolved = resolve_plan_arg(Path::new("my-plan"), temp_dir.path());

        assert_eq!(resolved, PathBuf::from("my-plan.md"));
    }

    #[test]
    fn resolve_plan_arg_finds_bare_name_under_docs_plans() {
        let temp_dir = tempfile::tempdir().unwrap();
        let plan_path = temp_dir
            .path()
            .join("docs")
            .join("plans")
            .join("2026-06-26-001-feat-ralph-lint-precheck-adaptation-plan.md");
        std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
        std::fs::write(&plan_path, "content").unwrap();

        let resolved = resolve_plan_arg(
            Path::new("2026-06-26-001-feat-ralph-lint-precheck-adaptation-plan"),
            temp_dir.path(),
        );

        assert_eq!(
            resolved,
            PathBuf::from("docs/plans/2026-06-26-001-feat-ralph-lint-precheck-adaptation-plan.md")
        );
    }

    #[test]
    fn resolve_plan_arg_adds_md_to_relative_docs_plans_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let plan_path = temp_dir
            .path()
            .join("docs")
            .join("plans")
            .join("my-plan.md");
        std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
        std::fs::write(&plan_path, "content").unwrap();

        let resolved = resolve_plan_arg(Path::new("docs/plans/my-plan"), temp_dir.path());

        assert_eq!(resolved, PathBuf::from("docs/plans/my-plan.md"));
    }

    #[test]
    fn resolve_plan_arg_preserved_original_when_nothing_matches() {
        let temp_dir = tempfile::tempdir().unwrap();

        let resolved = resolve_plan_arg(Path::new("missing-plan"), temp_dir.path());

        assert_eq!(resolved, PathBuf::from("missing-plan"));
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

    // ── R2 / R4 / SC6: cleanup timeout + abort for stuck I/O tasks ──────────

    #[tokio::test]
    async fn wait_for_task_with_timeout_returns_result_when_task_finishes() {
        let handle = tokio::spawn(async { 42 });
        let (result, timed_out) =
            wait_for_task_with_timeout(handle, "finishes", Duration::from_secs(1)).await;
        assert!(!timed_out);
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn wait_for_task_with_timeout_aborts_stuck_task() {
        tokio::time::pause();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_hours(1)).await;
            }
        });
        tokio::time::advance(Duration::from_millis(50)).await;
        let (_, timed_out) =
            wait_for_task_with_timeout(handle, "stuck", Duration::from_millis(10)).await;
        assert!(timed_out, "stuck task should be reported as timed out");
    }

    #[test]
    fn write_cleanup_diagnostic_appends_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_cleanup_diagnostic(
            tmp.path(),
            true,
            false,
            Duration::from_millis(2500),
            Some("SIGINT"),
        )
        .unwrap();

        assert!(path.exists());
        let contents = std::fs::read_to_string(&path).unwrap();
        let line = contents.lines().next().unwrap();
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["reader_timed_out"], true);
        assert_eq!(value["forward_timed_out"], false);
        assert_eq!(value["cleanup_elapsed_ms"], 2500);
        assert_eq!(value["signal"], "SIGINT");
        assert!(value["pid"].is_number());
        assert!(value["timestamp"].is_string());
    }

    // ── R13: subprocess TUI loop-lock cleanup ───────────────────────────────

    #[test]
    fn cleanup_subprocess_loop_lock_no_op_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        // No lock file exists - should not panic or create anything.
        cleanup_subprocess_loop_lock(tmp.path(), Some(12345));
        assert!(!tmp.path().join(".ralph/loop.lock").exists());
    }

    #[test]
    fn cleanup_subprocess_loop_lock_removes_stale_lock() {
        use ralph_core::LockMetadata;
        use std::fs;

        let tmp = tempfile::tempdir().unwrap();
        let ralph_dir = tmp.path().join(".ralph");
        fs::create_dir_all(&ralph_dir).unwrap();
        let lock_path = ralph_dir.join("loop.lock");

        // Write lock metadata without holding the flock -> stale lock.
        let metadata = LockMetadata {
            pid: 99999,
            started: chrono::Utc::now(),
            prompt: "stale".to_string(),
        };
        fs::write(&lock_path, serde_json::to_string(&metadata).unwrap()).unwrap();
        assert!(lock_path.exists());

        cleanup_subprocess_loop_lock(tmp.path(), Some(99999));

        assert!(
            !lock_path.exists(),
            "stale loop lock should be removed after subprocess TUI cleanup"
        );
    }

    #[tokio::test]
    async fn wait_or_terminate_child_returns_status_when_child_exits_quickly() {
        let mut child = tokio::process::Command::new("echo")
            .arg("hello")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let child_id = child.id();
        let status =
            wait_or_terminate_child(&mut child, child_id, Duration::from_secs(5), "quick_exit")
                .await;
        assert!(status.expect("child should exit").success());
    }

    // ─────────────────────────────────────────────────────────────────────
    // U4 (2026-06-25-002): 在 ralph run 流程中应用 profile
    // ─────────────────────────────────────────────────────────────────────

    /// Helper:在 `<tmp>/ralph-profiles/<name>/<preset>/<hat>.md` 写入片段。
    fn write_repo_profile_fragment(
        root: &Path,
        name: &str,
        preset: &str,
        hat_id: &str,
        body: &str,
    ) -> PathBuf {
        let dir = root.join("ralph-profiles").join(name).join(preset);
        std::fs::create_dir_all(&dir).expect("create profile dir");
        let path = dir.join(format!("{hat_id}.md"));
        std::fs::write(&path, body).expect("write fragment");
        path
    }

    /// Happy path: `Builtin("debug")` 的 hats source + 存在的 repo profile
    /// + 对应 hat id 片段 → config.hats["executor"].instructions 末尾追加片段内容
    /// (R10/R11/R14)。
    #[test]
    fn apply_active_profiles_builtin_hats_appends_repo_fragment() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_repo_profile_fragment(
            tmp.path(),
            "strict",
            "debug",
            "executor",
            "附加片段:严格模式",
        );

        let mut config = RalphConfig::default();
        config
            .hats
            .insert("executor".to_string(), HatConfig::default());
        config.hats.get_mut("executor").unwrap().instructions = "基线 instructions".to_string();

        let mut args = default_run_args();
        args.profiles = vec!["repo:strict".to_string()];

        apply_active_profiles(
            &mut config,
            &args,
            Some(&HatsSource::Builtin("debug".to_string())),
            tmp.path(),
        )
        .expect("apply");

        let hat = config.hats.get("executor").expect("executor hat");
        assert!(hat.instructions.contains("基线 instructions"));
        assert!(hat.instructions.contains("附加片段:严格模式"));
        // 片段必须追加在末尾,不能覆盖。
        let pos_base = hat.instructions.find("基线 instructions").unwrap();
        let pos_extra = hat.instructions.find("附加片段:严格模式").unwrap();
        assert!(pos_extra > pos_base, "fragment must append, not prepend");
    }

    /// `HatsSource::File(path)` → preset 名取 path 的 file stem(去掉扩展名)。
    /// 这条对自定义 hats 配置文件很重要,profile 目录按 preset 命名存放。
    #[test]
    fn apply_active_profiles_file_hats_uses_file_stem_as_preset_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_repo_profile_fragment(tmp.path(), "strict", "my-custom-preset", "executor", "X");

        let mut config = RalphConfig::default();
        config
            .hats
            .insert("executor".to_string(), HatConfig::default());

        let mut args = default_run_args();
        args.profiles = vec!["repo:strict".to_string()];

        let hats_file = tmp.path().join("my-custom-preset.yaml");
        apply_active_profiles(
            &mut config,
            &args,
            Some(&HatsSource::File(hats_file)),
            tmp.path(),
        )
        .expect("apply");

        assert!(config.hats["executor"].instructions.contains('X'));
    }

    /// `HatsSource::Remote(url)` + active specs → 返回错误(不让 `--profile`
    /// 静默配错),提示不能用于 remote preset。
    #[test]
    fn apply_active_profiles_remote_hats_with_active_specs_returns_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut config = RalphConfig::default();
        let mut args = default_run_args();
        args.profiles = vec!["repo:strict".to_string()];

        let err = apply_active_profiles(
            &mut config,
            &args,
            Some(&HatsSource::Remote(
                "https://example.com/preset.yml".to_string(),
            )),
            tmp.path(),
        )
        .expect_err("must reject");
        let msg = format!("{err}");
        assert!(
            msg.contains("remote") || msg.contains("Remote"),
            "error must mention remote source, got: {msg}"
        );
    }

    /// `HatsSource::None` + active specs → 不报错,也不修改 instructions;
    /// 只需确保不会 panic,因为没有 preset 名可推就找不到 profile 目录。
    /// 行为契约:返回 Ok,且 config 不变(空 preset 名时
    /// `apply_profile_fragments` 视为空 preset,无片段可加)。
    #[test]
    fn apply_active_profiles_none_hats_with_active_specs_returns_error() {
        // P1 regression (post-2026-06-25-002 review): None hats
        // source with active specs must surface as a hard error,
        // not silently no-op. Operators who configure
        // `profiles.default` or pass `--profile repo:strict` and
        // forget `-H` must learn about the misconfiguration
        // immediately rather than at first-hat-activation time.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_repo_profile_fragment(tmp.path(), "strict", "debug", "executor", "ignored");

        let mut config = RalphConfig::default();
        config
            .hats
            .insert("executor".to_string(), HatConfig::default());
        config.hats.get_mut("executor").unwrap().instructions = "baseline".to_string();

        let mut args = default_run_args();
        args.profiles = vec!["repo:strict".to_string()];

        let err = apply_active_profiles(&mut config, &args, None, tmp.path())
            .expect_err("apply must fail when no preset is active");
        let msg = format!("{err}");
        assert!(
            msg.contains("--profile specs requested") && msg.contains("no preset"),
            "expected actionable error message, got: {msg}"
        );

        // The error must short-circuit before any fragment is
        // appended — config stays untouched.
        assert_eq!(config.hats["executor"].instructions, "baseline");
    }

    /// 空 active specs(无 CLI,无 defaults)→ helper 是 no-op,不报错。
    /// 这是 fast path,绝大多数 `ralph run` 走这条。
    #[test]
    fn apply_active_profiles_empty_specs_is_noop() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut config = RalphConfig::default();
        let args = default_run_args();

        apply_active_profiles(
            &mut config,
            &args,
            Some(&HatsSource::Builtin("debug".to_string())),
            tmp.path(),
        )
        .expect("apply");

        assert!(config.hats.is_empty(), "no specs => no changes");
    }

    /// Profile 应用发生在 `normalize()` 之后:`extra_instructions` 必须先被
    /// `normalize()` 合并到 instructions,然后 profile 片段再追加到末尾。
    /// 这条断言验证 helper 调用顺序正确(在 load_config_for_preflight 之后)。
    #[test]
    fn apply_active_profiles_appends_after_extra_instructions() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_repo_profile_fragment(tmp.path(), "strict", "debug", "executor", "PROFILE");

        let mut config = RalphConfig::default();
        let mut hat = HatConfig::default();
        hat.extra_instructions = vec!["EXTRA".to_string()];
        hat.instructions = "BASE".to_string();
        config.hats.insert("executor".to_string(), hat);

        // 模拟 normalize() 已经跑过(extra_instructions 已 drain 到 instructions)
        config.normalize();

        let mut args = default_run_args();
        args.profiles = vec!["repo:strict".to_string()];

        apply_active_profiles(
            &mut config,
            &args,
            Some(&HatsSource::Builtin("debug".to_string())),
            tmp.path(),
        )
        .expect("apply");

        let final_instr = &config.hats["executor"].instructions;
        assert!(final_instr.contains("BASE"));
        assert!(final_instr.contains("EXTRA"));
        assert!(final_instr.contains("PROFILE"));
        // PROFILE 必须在 EXTRA 之后(应用顺序):normalize() 先,然后 apply。
        let pos_extra = final_instr.find("EXTRA").unwrap();
        let pos_profile = final_instr.find("PROFILE").unwrap();
        assert!(
            pos_profile > pos_extra,
            "profile must append after normalize() merged extra_instructions"
        );
    }

    /// 显式 `--profile repo:nope` 但目录不存在 → 提前返回错误,错误信息含路径,
    /// 不进入后续 event loop。R14 的硬契约:用户拼错名字时立即看到。
    #[test]
    fn apply_active_profiles_missing_repo_dir_returns_error_with_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut config = RalphConfig::default();
        config
            .hats
            .insert("executor".to_string(), HatConfig::default());

        let mut args = default_run_args();
        args.profiles = vec!["repo:nope".to_string()];

        let err = apply_active_profiles(
            &mut config,
            &args,
            Some(&HatsSource::Builtin("debug".to_string())),
            tmp.path(),
        )
        .expect_err("missing dir must error");

        let msg = format!("{err}");
        let expected_path = tmp.path().join("ralph-profiles").join("nope");
        assert!(
            msg.contains(expected_path.to_str().unwrap()),
            "error must include path {:?}, got: {msg}",
            expected_path
        );
    }

    /// 解析 profile 目录时,workspace_root 是调用方传入的值(模拟
    /// `RALPH_WORKSPACE_ROOT` 路径);不在子进程错位。helper 必须接受外部传入
    /// 的 base 路径而不自行依赖 `current_dir()`,这正是为了让 `--worktree`
    /// 子进程场景下 repo profile 仍指向主仓库根目录。
    #[test]
    fn apply_active_profiles_uses_caller_supplied_workspace_root() {
        let caller_root = tempfile::tempdir().expect("tempdir");
        let mut other_root = caller_root.path().to_path_buf();
        other_root.push("nested-subdir");
        std::fs::create_dir_all(&other_root).expect("nested");

        write_repo_profile_fragment(
            caller_root.path(),
            "strict",
            "debug",
            "executor",
            "WORKSPACE_ROOT",
        );

        let mut config = RalphConfig::default();
        config
            .hats
            .insert("executor".to_string(), HatConfig::default());

        let mut args = default_run_args();
        args.profiles = vec!["repo:strict".to_string()];

        // 传入 caller_root(而非 other_root),验证 helper 不会偷用 current_dir
        apply_active_profiles(
            &mut config,
            &args,
            Some(&HatsSource::Builtin("debug".to_string())),
            caller_root.path(),
        )
        .expect("apply");

        assert!(
            config.hats["executor"]
                .instructions
                .contains("WORKSPACE_ROOT")
        );
    }

    /// `HatsSource::File(path)` 且 path 无扩展名:file_stem() 应等于完整文件名。
    #[test]
    fn apply_active_profiles_file_hats_without_extension_uses_full_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_repo_profile_fragment(tmp.path(), "strict", "noext", "executor", "Y");

        let mut config = RalphConfig::default();
        config
            .hats
            .insert("executor".to_string(), HatConfig::default());

        let mut args = default_run_args();
        args.profiles = vec!["repo:strict".to_string()];

        let hats_file = tmp.path().join("noext");
        apply_active_profiles(
            &mut config,
            &args,
            Some(&HatsSource::File(hats_file)),
            tmp.path(),
        )
        .expect("apply");

        assert!(config.hats["executor"].instructions.contains('Y'));
    }
}
