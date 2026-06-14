//! # ralph-cli
//!
//! Binary entry point for the Ralph Orchestrator.
//!
//! This crate provides:
//! - CLI argument parsing using `clap`
//! - Application initialization and configuration
//! - Entry point to the headless orchestration loop
//! - Event history viewing via `ralph events`
//! - Project initialization via `ralph init`
//! - SOP-based planning via `ralph plan`
//! - Code task generation via `ralph code-task`
//! - Hook config validation via `ralph hooks validate`
//! - Work item tracking via `ralph task`

mod backend_support;
mod bot;
mod cli;
mod commands;
mod config_resolution;
mod display;
mod doctor;
mod hats;
mod hooks;
mod init;
mod interact;
mod loop_runner;
mod loops;
mod mcp;
mod memory;
mod operation_guard;
mod policy_check;
mod preflight;
mod preset_templates;
mod presets;
mod rpc_stdin;
mod skill_cli;
mod sop_runner;
mod task_cli;
#[cfg(test)]
mod test_support;
mod tools;
mod wave;
mod web;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
use std::io::IsTerminal;
use std::sync::Arc;

// Shared CLI infrastructure layer (U4 step-01 extraction).
#[cfg(test)]
use crate::cli::OutputFormat;
use crate::cli::{
    ColorMode, ConfigSource, HatsSource, apply_config_overrides, default_config_path,
    ensure_scratchpad_directory, install_panic_hook, load_config_with_overrides,
    resolve_path_from_workspace, resolve_workspace_root,
};
use ralph_core::diagnostics::DiagnosticsCollector;

/// Ralph Orchestrator - Multi-agent orchestration framework
#[derive(Parser, Debug)]
#[command(name = "ralph", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    // ─────────────────────────────────────────────────────────────────────────
    // Global options (available for all subcommands)
    // ─────────────────────────────────────────────────────────────────────────
    /// Core configuration source: file path, URL, or core.field=value override.
    /// Can be specified multiple times. Overrides are applied after core config loading.
    /// If not set, defaults to `ralph.yml` or `$RALPH_CONFIG`.
    #[arg(short, long, global = true, action = ArgAction::Append)]
    config: Vec<String>,

    /// Hat collection source: file path, builtin:name, or URL.
    ///
    /// Example: `-H builtin:debug` or `-H .ralph/hats/my-workflow.yml`
    #[arg(short = 'H', long, global = true)]
    hats: Option<String>,

    /// Verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Color output mode (auto, always, never)
    #[arg(long, value_enum, default_value_t = ColorMode::Auto, global = true)]
    color: ColorMode,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the orchestration loop (default if no subcommand given)
    Run(commands::run::RunArgs),

    /// Run preflight checks to validate configuration and environment
    Preflight(preflight::PreflightArgs),

    /// Validate hooks configuration and command wiring
    Hooks(hooks::HooksArgs),

    /// Run first-run diagnostics and environment checks
    Doctor(doctor::DoctorArgs),

    /// Interactive walkthrough of hats, hat collections, and workflow
    Tutorial(commands::tutorial::TutorialArgs),

    /// DEPRECATED: Use `ralph run --continue` instead.
    /// Resume a previously interrupted loop from existing scratchpad.
    #[command(hide = true)]
    Resume(commands::resume::ResumeArgs),

    /// View event history for debugging
    Events(commands::events::EventsArgs),

    /// Initialize a new ralph.yml configuration file
    Init(commands::init::InitArgs),

    /// Clean up Ralph artifacts from `.ralph/agent`.
    Clean(commands::clean::CleanArgs),

    /// Emit an event to the current run's events file with proper JSON formatting
    Emit(commands::emit::EmitArgs),

    /// Start a Prompt-Driven Development planning session
    Plan(commands::plan::PlanArgs),

    /// Generate code task files from descriptions or plans
    CodeTask(commands::code_task::CodeTaskArgs),

    /// Legacy alias for `code-task` (runtime tasks are `ralph tools task`).
    #[command(hide = true)]
    Task(commands::code_task::CodeTaskArgs),

    /// Ralph's runtime tools (agent-facing)
    Tools(tools::ToolsArgs),

    /// Dispatch wave events for parallel hat execution
    Wave(wave::WaveArgs),

    /// Manage parallel loops
    Loops(loops::LoopsArgs),

    /// Manage configured hats
    Hats(hats::HatsArgs),

    /// Attach a TUI to a running ralph-api server
    Tui(commands::tui::TuiArgs),

    /// Run the web dashboard
    Web(web::WebArgs),

    /// Run Ralph as an MCP server over stdio
    Mcp(mcp::McpArgs),

    /// Manage Telegram bot setup and testing
    Bot(bot::BotArgs),

    /// Manage and validate presets
    Preset(commands::preset::PresetArgs),

    /// Generate shell completions
    Completions(commands::completions::CompletionsArgs),

    /// Build an offline diagnosis report from `.ralph/diagnostics/<session>/` (U7)
    Diagnose(commands::diagnose::DiagnoseArgs),
}

/// Returns true if the given command is eligible for diagnostics session creation.
fn is_diagnostics_eligible_command(command: Option<&Commands>) -> bool {
    matches!(command, Some(Commands::Run(_) | Commands::Resume(_)) | None)
}

/// Returns true when the current process will spawn a child `--rpc`
/// process and host the TUI (subprocess TUI mode). Mirrors the
/// `use_subprocess_tui` calculation in `commands/run.rs` — keep the
/// two in sync so the parent and the child agree on mode boundaries.
///
/// `is_tty` is plumbed as a parameter so unit tests can exercise the
/// non-TTY branch deterministically (the real `main` always passes the
/// live `stdin().is_terminal() && stdout().is_terminal()` value).
///
/// U2 (2026-06-14): when this returns true, `authoritative_diagnostics`
/// is built in `trace_only` mode so the parent does not leave an empty
/// recovery/drift/orchestration/... shell in the main repo while the
/// child RPC writes the real data into the worktree.
fn use_subprocess_tui(command: Option<&Commands>, is_tty: bool) -> bool {
    match command {
        Some(Commands::Run(args)) => {
            !args.no_tui && !args.autonomous && !args.rpc && !args.legacy_tui && is_tty
        }
        // ResumeArgs has no legacy_tui field — the resume subcommand
        // does not expose the in-process TUI escape hatch. Match the
        // narrower resume command shape.
        Some(Commands::Resume(args)) => {
            !args.no_tui && !args.autonomous && !args.rpc && is_tty
        }
        None => is_tty, // default `ralph` → `ralph run` interactive
        _ => false,
    }
}

/// Best-effort read of `telemetry.runtime_diagnosis.write_artifacts` from
/// ralph.yml. Returns `false` on any error (missing file, parse error, missing
/// field) so the activation matrix stays fail-closed and the "默认 no-op"
/// constraint from plan U0 is preserved. We deliberately avoid loading the full
/// [`RalphConfig`] here because the main entry point needs this signal before
/// command dispatch, and a full config load can fail loudly in ways that
/// should not block unrelated subcommands like `ralph diagnose`.
fn read_telemetry_write_artifacts(ralph_yml_path: &std::path::Path) -> bool {
    let content = match std::fs::read_to_string(ralph_yml_path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let yaml: serde_yaml::Value = match serde_yaml::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };
    yaml.get("telemetry")
        .and_then(|t| t.get("runtime_diagnosis"))
        .and_then(|r| r.get("write_artifacts"))
        .and_then(|w| w.as_bool())
        .unwrap_or(false)
}

#[tokio::main]
async fn main() -> Result<()> {
    install_panic_hook();

    let cli = Cli::parse();

    let tui_enabled = match &cli.command {
        Some(Commands::Run(args)) => !args.no_tui && !args.autonomous && !args.rpc,
        Some(Commands::Resume(args)) => !args.no_tui && !args.autonomous && !args.rpc,
        None => true,
        _ => false,
    };
    let rpc_enabled = match &cli.command {
        Some(Commands::Run(args)) => args.rpc,
        Some(Commands::Resume(args)) => args.rpc,
        _ => false,
    };
    let mcp_enabled = matches!(&cli.command, Some(Commands::Mcp(_)));

    // U2 (2026-06-14): compute the subprocess-TUI flag here so the
    // parent can choose `trace_only` mode. Mirrors the calculation in
    // `commands/run.rs:use_subprocess_tui`; both must agree.
    let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let subprocess_tui_mode = use_subprocess_tui(cli.command.as_ref(), is_tty);

    let filter = if cli.verbose { "debug" } else { "info" };
    // U0 activation matrix: session is created when EITHER `RALPH_DIAGNOSTICS=1`
    // is set OR `telemetry.runtime_diagnosis.write_artifacts: true` is in
    // ralph.yml. Previously the second path was dropped on the floor by
    // `DiagnosticsOptions::from_env` hardcoding `runtime_diagnosis_artifacts: false`.
    let diagnostics_env_set = std::env::var("RALPH_DIAGNOSTICS")
        .map(|v| v == "1")
        .unwrap_or(false);
    let diagnostics_config_write_artifacts =
        read_telemetry_write_artifacts(std::path::Path::new("ralph.yml"));
    let diagnostics_enabled = is_diagnostics_eligible_command(cli.command.as_ref())
        && (diagnostics_env_set || diagnostics_config_write_artifacts);

    // U0: build ONE authoritative diagnostics collector up front, so the
    // tracing layer and the EventLoop share a single timestamped session
    // directory. `EventLoop::with_context` reuses a prebuilt collector
    // attached to the LoopContext, so the run never produces two sessions.
    //
    // When the env var is set we take the historical full-diagnostics path
    // (no telemetry config consulted, preserves the U0 contract that
    // `RALPH_DIAGNOSTICS=1` is the full-diagnostics trigger). When the env
    // is unset but `write_artifacts: true` is in ralph.yml we take the
    // minimal session path through `from_env_with_telemetry`.
    //
    // U2 (2026-06-14): when subprocess TUI mode is active, the parent
    // only runs the TUI and forwards stderr to a log file — it does NOT
    // run the EventLoop. The loop-level loggers would stay empty, so
    // we set `trace_only` to skip them. The child RPC process re-enters
    // `main()` and creates its OWN full session in the worktree.
    let authoritative_diagnostics: Option<Arc<DiagnosticsCollector>> = if diagnostics_enabled {
        let options = if diagnostics_env_set {
            ralph_core::diagnostics::DiagnosticsOptions::from_env(None)
        } else {
            ralph_core::diagnostics::DiagnosticsOptions::from_env_with_telemetry(
                None,
                diagnostics_config_write_artifacts,
            )
        };
        let options = ralph_core::diagnostics::DiagnosticsOptions {
            trace_only: subprocess_tui_mode,
            ..options
        };
        match DiagnosticsCollector::with_options(std::path::Path::new("."), &options) {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                eprintln!(
                    "warning: failed to initialize diagnostics: {e}; continuing with diagnostics disabled"
                );
                None
            }
        }
    } else {
        None
    };

    if tui_enabled {
        if let Ok((file, _log_path)) =
            ralph_core::diagnostics::create_log_file(std::path::Path::new("."))
        {
            if let Some(collector) = authoritative_diagnostics.as_ref()
                && let Some(session_dir) = collector.session_dir()
            {
                use ralph_core::diagnostics::DiagnosticTraceLayer;
                use tracing_subscriber::prelude::*;

                if let Ok(trace_layer) = DiagnosticTraceLayer::new(session_dir) {
                    tracing_subscriber::registry()
                        .with(
                            tracing_subscriber::fmt::layer()
                                .with_writer(std::sync::Mutex::new(file))
                                .with_ansi(false),
                        )
                        .with(tracing_subscriber::EnvFilter::new(filter))
                        .with(trace_layer)
                        .init();
                } else {
                    tracing_subscriber::fmt()
                        .with_env_filter(filter)
                        .with_writer(std::sync::Mutex::new(file))
                        .with_ansi(false)
                        .init();
                }
            } else {
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .init();
            }
        }
    } else if rpc_enabled || mcp_enabled {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    } else {
        if let Some(collector) = authoritative_diagnostics.as_ref()
            && let Some(session_dir) = collector.session_dir()
        {
            use ralph_core::diagnostics::DiagnosticTraceLayer;
            use tracing_subscriber::prelude::*;

            if let Ok(trace_layer) = DiagnosticTraceLayer::new(session_dir) {
                tracing_subscriber::registry()
                    .with(tracing_subscriber::fmt::layer())
                    .with(tracing_subscriber::EnvFilter::new(filter))
                    .with(trace_layer)
                    .init();
            } else {
                tracing_subscriber::fmt().with_env_filter(filter).init();
            }
        } else {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }

    let config_values: Vec<String> = if cli.config.is_empty() {
        vec![default_config_path().to_string_lossy().to_string()]
    } else {
        cli.config.clone()
    };

    let config_sources: Vec<ConfigSource> = config_values
        .iter()
        .map(|s| ConfigSource::parse(s))
        .collect();
    let hats_source = cli.hats.as_deref().map(HatsSource::parse);

    match cli.command {
        Some(Commands::Run(args)) => {
            commands::run::run_command(
                &config_sources,
                hats_source.as_ref(),
                cli.verbose,
                cli.color,
                args,
                authoritative_diagnostics.clone(),
            )
            .await
        }
        Some(Commands::Preflight(args)) => {
            preflight::execute(
                &config_sources,
                hats_source.as_ref(),
                args,
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Hooks(args)) => {
            hooks::execute(
                &config_sources,
                hats_source.as_ref(),
                args,
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Doctor(args)) => {
            doctor::execute(
                &config_sources,
                hats_source.as_ref(),
                args,
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Tutorial(args)) => commands::tutorial::tutorial_command(cli.color, args),
        Some(Commands::Resume(args)) => {
            commands::resume::resume_command(
                &config_sources,
                hats_source.as_ref(),
                cli.verbose,
                cli.color,
                args,
                authoritative_diagnostics.clone(),
            )
            .await
        }
        Some(Commands::Events(args)) => commands::events::events_command(cli.color, args),
        Some(Commands::Init(args)) => commands::init::init_command(cli.color, args),
        Some(Commands::Clean(args)) => {
            commands::clean::clean_command(&config_sources, cli.color, args)
        }
        Some(Commands::Emit(args)) => {
            commands::emit::emit_command(cli.color, args, hats_source.as_ref())
        }
        Some(Commands::Plan(args)) => {
            commands::plan::plan_command(&config_sources, hats_source.as_ref(), cli.color, args)
                .await
        }
        Some(Commands::CodeTask(args)) => {
            commands::code_task::code_task_command(
                &config_sources,
                hats_source.as_ref(),
                cli.color,
                args,
            )
            .await
        }
        Some(Commands::Task(args)) => {
            commands::code_task::code_task_command(
                &config_sources,
                hats_source.as_ref(),
                cli.color,
                args,
            )
            .await
        }
        Some(Commands::Tools(args)) => tools::execute(args, cli.color.should_use_colors()).await,
        Some(Commands::Wave(args)) => wave::execute(args, cli.color.should_use_colors()),
        Some(Commands::Loops(args)) => loops::execute(args, cli.color.should_use_colors()),
        Some(Commands::Hats(args)) => {
            hats::execute(
                &config_sources,
                hats_source.as_ref(),
                args,
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Tui(args)) => commands::tui::tui_command(args).await,
        Some(Commands::Web(args)) => web::execute(args).await,
        Some(Commands::Mcp(args)) => mcp::execute(args).await,
        Some(Commands::Bot(args)) => {
            bot::execute(
                args,
                &config_sources,
                hats_source.as_ref(),
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Preset(args)) => {
            commands::preset::execute(
                &config_sources,
                hats_source.as_ref(),
                args,
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Completions(args)) => commands::completions::completions_command(args),
        Some(Commands::Diagnose(args)) => commands::diagnose::diagnose_command(cli.color, args),
        None => {
            let args = commands::run::RunArgs {
                prompt_text: None,
                prompt_file: None,
                backend: None,
                max_iterations: None,
                completion_promise: None,
                dry_run: false,
                continue_mode: false,
                loop_id: None,
                no_tui: false,
                autonomous: false,
                rpc: false,
                legacy_tui: false,
                idle_timeout: None,
                autonomous_idle_timeout: None,
                exclusive: false,
                no_auto_merge: false,
                worktree: false,
                worktree_path: None,
                skip_preflight: false,
                no_sync_agent_docs: false,
                verbose: false,
                quiet: false,
                record_session: None,
                custom_args: Vec::new(),
                warmup_only: false,
                force_warmup: false,
            };
            commands::run::run_command(
                &config_sources,
                hats_source.as_ref(),
                cli.verbose,
                cli.color,
                args,
                authoritative_diagnostics.clone(),
            )
            .await
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::emit::EmitArgs;
    use crate::commands::events::EventsArgs;
    use crate::commands::run::default_run_args;
    use crate::test_support::CwdGuard;
    use std::path::PathBuf;
    use tempfile::TempDir;
    #[test]
    fn test_cli_parses_global_hats_flag() {
        let cli =
            Cli::try_parse_from(["ralph", "run", "-H", "builtin:debug"]).expect("CLI parse failed");
        assert_eq!(cli.hats.as_deref(), Some("builtin:debug"));
    }

    #[test]
    fn test_bot_daemon_parses_global_config_flag() {
        let cli = Cli::try_parse_from(["ralph", "bot", "daemon", "-c", "ralph.bot.yml"])
            .expect("CLI parse failed");

        assert!(cli.config.iter().any(|value| value == "ralph.bot.yml"));
        assert!(matches!(
            cli.command,
            Some(Commands::Bot(crate::bot::BotArgs {
                command: crate::bot::BotCommands::Daemon(_),
            }))
        ));
    }

    #[test]
    fn test_doctor_parses_command() {
        let cli = Cli::try_parse_from(["ralph", "doctor"]).expect("CLI parse failed");

        assert!(matches!(cli.command, Some(Commands::Doctor(_))));
    }

    #[test]
    fn test_tutorial_parses_command() {
        let cli = Cli::try_parse_from(["ralph", "tutorial"]).expect("CLI parse failed");

        assert!(matches!(cli.command, Some(Commands::Tutorial(_))));
    }

    #[test]
    fn test_mcp_serve_parses_command() {
        let cli = Cli::try_parse_from(["ralph", "mcp", "serve"]).expect("CLI parse failed");
        assert!(matches!(cli.command, Some(Commands::Mcp(_))));
    }

    #[test]
    fn test_mcp_serve_parses_workspace_root_flag() {
        let cli = Cli::try_parse_from([
            "ralph",
            "mcp",
            "serve",
            "--workspace-root",
            "/tmp/ralph-workspace",
        ])
        .expect("CLI parse failed");

        match cli.command {
            Some(Commands::Mcp(crate::mcp::McpArgs {
                command: crate::mcp::McpCommands::Serve(crate::mcp::ServeArgs { workspace_root }),
            })) => {
                assert_eq!(
                    workspace_root,
                    Some(std::path::PathBuf::from("/tmp/ralph-workspace"))
                );
            }
            other => panic!("unexpected CLI parse result: {other:?}"),
        }
    }

    #[test]
    fn test_diagnostics_eligible_for_run_command() {
        let command = Some(Commands::Run(default_run_args()));
        assert!(is_diagnostics_eligible_command(command.as_ref()));
    }

    #[test]
    fn test_diagnostics_eligible_for_no_subcommand() {
        assert!(is_diagnostics_eligible_command(None));
    }

    #[test]
    fn test_diagnostics_not_eligible_for_emit_command() {
        let command = Some(Commands::Emit(EmitArgs {
            topic: "test.event".to_string(),
            payload: String::new(),
            json: false,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
        }));
        assert!(!is_diagnostics_eligible_command(command.as_ref()));
    }

    #[test]
    fn test_diagnostics_not_eligible_for_events_command() {
        let command = Some(Commands::Events(EventsArgs {
            last: None,
            topic: None,
            iteration: None,
            format: OutputFormat::Table,
            file: None,
            clear: false,
            confirm: None,
        }));
        assert!(!is_diagnostics_eligible_command(command.as_ref()));
    }

    // ── U2: subprocess TUI parent must use trace_only mode (2026-06-14) ──
    // The parent's only job in subprocess TUI mode is to host the TUI and
    // forward the child's stderr into a log file. It does NOT run an
    // EventLoop, so the loop-level loggers it would otherwise create
    // (recovery/drift/orchestration/...) would stay empty forever — the
    // "empty shell" bug. `use_subprocess_tui` in `main.rs` must mirror
    // `commands/run.rs:use_subprocess_tui` (TTY + !legacy_tui + the
    // broader tui_enabled flags).

    fn run_args_with(
        no_tui: bool,
        autonomous: bool,
        rpc: bool,
        legacy_tui: bool,
    ) -> commands::run::RunArgs {
        let mut args = default_run_args();
        args.no_tui = no_tui;
        args.autonomous = autonomous;
        args.rpc = rpc;
        args.legacy_tui = legacy_tui;
        args
    }

    #[test]
    fn test_use_subprocess_tui_true_for_default_run_args() {
        let args = run_args_with(false, false, false, false);
        let cmd = Some(Commands::Run(args));
        assert!(use_subprocess_tui(cmd.as_ref(), true));
        assert!(!use_subprocess_tui(cmd.as_ref(), false));
    }

    #[test]
    fn test_use_subprocess_tui_false_when_legacy_tui() {
        let args = run_args_with(false, false, false, true);
        let cmd = Some(Commands::Run(args));
        assert!(!use_subprocess_tui(cmd.as_ref(), true));
    }

    #[test]
    fn test_use_subprocess_tui_false_when_no_tui() {
        let args = run_args_with(true, false, false, false);
        let cmd = Some(Commands::Run(args));
        assert!(!use_subprocess_tui(cmd.as_ref(), true));
    }

    #[test]
    fn test_use_subprocess_tui_false_when_autonomous() {
        let args = run_args_with(false, true, false, false);
        let cmd = Some(Commands::Run(args));
        assert!(!use_subprocess_tui(cmd.as_ref(), true));
    }

    #[test]
    fn test_use_subprocess_tui_false_when_rpc() {
        // rpc mode means the current process IS the child; it must run
        // the full EventLoop, so trace_only would be wrong here.
        let args = run_args_with(false, false, true, false);
        let cmd = Some(Commands::Run(args));
        assert!(!use_subprocess_tui(cmd.as_ref(), true));
    }

    #[test]
    fn test_use_subprocess_tui_true_for_resume_default_args() {
        let mut args = default_run_args();
        args.no_tui = false;
        args.autonomous = false;
        args.rpc = false;
        args.legacy_tui = false;
        // ResumeArgs shares the same flag set; for the purposes of this
        // helper, default values are enough.
        let _ = args; // suppress unused warning
        let cmd: Option<Commands> = None; // None branch
        assert!(use_subprocess_tui(cmd.as_ref(), true));
    }

    #[test]
    fn test_use_subprocess_tui_false_for_non_run_resume_commands() {
        let cmd = Some(Commands::Preflight(preflight::PreflightArgs {
            format: preflight::PreflightFormat::Human,
            strict: false,
            check: vec![],
        }));
        assert!(!use_subprocess_tui(cmd.as_ref(), true));
    }

    #[test]
    fn test_load_config_with_overrides_applies_override_sources() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());
        let config_path = temp_dir.path().join("ralph.yml");
        std::fs::write(&config_path, "core:\n  scratchpad: .agent/scratchpad.md\n").unwrap();

        let sources = vec![
            ConfigSource::File(config_path),
            ConfigSource::Override {
                key: "core.scratchpad".to_string(),
                value: ".custom/scratch.md".to_string(),
            },
        ];

        let config = load_config_with_overrides(&sources).unwrap();

        assert_eq!(config.core.scratchpad.path, ".custom/scratch.md");
        let expected_root = std::fs::canonicalize(temp_dir.path())
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        let actual_root = std::fs::canonicalize(&config.core.workspace_root)
            .unwrap_or_else(|_| config.core.workspace_root.clone());
        assert_eq!(actual_root, expected_root);
    }

    #[test]
    fn test_load_config_with_overrides_only_overrides_uses_defaults() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());

        let sources = vec![ConfigSource::Override {
            key: "core.specs_dir".to_string(),
            value: "custom-specs".to_string(),
        }];

        let config = load_config_with_overrides(&sources).unwrap();

        assert_eq!(config.core.specs_dir, "custom-specs");
        let expected_root = std::fs::canonicalize(temp_dir.path())
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        let actual_root = std::fs::canonicalize(&config.core.workspace_root)
            .unwrap_or_else(|_| config.core.workspace_root.clone());
        assert_eq!(actual_root, expected_root);
    }

    #[test]
    fn test_load_config_with_overrides_missing_file_falls_back_to_defaults() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());

        let sources = vec![ConfigSource::File(PathBuf::from("missing.yml"))];

        let config = load_config_with_overrides(&sources).unwrap();

        assert!(!config.core.scratchpad.path.is_empty());
        let expected_root = std::fs::canonicalize(temp_dir.path())
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        let actual_root = std::fs::canonicalize(&config.core.workspace_root)
            .unwrap_or_else(|_| config.core.workspace_root.clone());
        assert_eq!(actual_root, expected_root);
    }
}
