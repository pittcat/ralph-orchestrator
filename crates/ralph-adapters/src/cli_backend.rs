//! CLI backend definitions for different AI tools.

use ralph_core::{CliConfig, HatBackend};
use std::fmt;
use std::io::Write;
use tempfile::NamedTempFile;

/// Output format supported by a CLI backend.
///
/// This allows adapters to declare whether they emit structured JSON
/// for real-time streaming or plain text output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Plain text output (default for most adapters)
    #[default]
    Text,
    /// Newline-delimited JSON stream (Claude with --output-format stream-json)
    StreamJson,
    /// Newline-delimited JSON stream (Pi with --mode json)
    PiStreamJson,
    /// Newline-delimited JSON stream (Trae CLI with --output-format stream-json)
    TraeStreamJson,
    /// Newline-delimited JSON stream (Cursor `agent` with --output-format stream-json)
    AgentStreamJson,
}

/// Error when creating a custom backend without a command.
#[derive(Debug, Clone)]
pub struct CustomBackendError;

impl fmt::Display for CustomBackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "custom backend requires a command to be specified")
    }
}

impl std::error::Error for CustomBackendError {}

/// How to pass prompts to the CLI tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    /// Pass prompt as a command-line argument.
    Arg,
    /// Write prompt to stdin.
    Stdin,
}

/// A CLI backend configuration for executing prompts.
#[derive(Debug, Clone)]
pub struct CliBackend {
    /// The command to execute.
    pub command: String,
    /// Additional arguments before the prompt.
    pub args: Vec<String>,
    /// How to pass the prompt.
    pub prompt_mode: PromptMode,
    /// Argument flag for prompt (if prompt_mode is Arg).
    pub prompt_flag: Option<String>,
    /// Output format emitted by this backend.
    pub output_format: OutputFormat,
    /// Environment variables to set when spawning the process.
    pub env_vars: Vec<(String, String)>,
}

impl CliBackend {
    /// Creates a backend from configuration.
    ///
    /// # Errors
    /// Returns `CustomBackendError` if backend is "custom" but no command is specified.
    pub fn from_config(config: &CliConfig) -> Result<Self, CustomBackendError> {
        let mut backend = match config.backend.as_str() {
            "claude" => Self::claude(),
            "gemini" => Self::gemini(),
            "codex" => Self::codex(),
            "opencode" => Self::opencode(),
            "pi" => Self::pi(),
            "traecli" => Self::traecli(),
            "agent" => Self::agent(),
            "custom" => return Self::custom(config),
            _ => Self::claude(), // Default to claude
        };

        // Apply configured extra args for named backends too.
        // This keeps ralph.yml `cli.args` consistent with CLI `-- ...` extra args behavior.
        backend.args.extend(config.args.iter().cloned());
        if backend.command == "codex" {
            Self::reconcile_codex_args(&mut backend.args);
        }

        // Honor command override for named backends (e.g., custom binary path)
        if let Some(ref cmd) = config.command {
            backend.command = cmd.clone();
        }

        Ok(backend)
    }

    /// Creates the Claude backend.
    ///
    /// Uses `--print` for headless execution and sends the prompt over stdin.
    /// This avoids Claude's large-prompt `-p` behavior, which can stall before
    /// emitting any stream output when asked to read the real prompt from an
    /// intermediate temp-file instruction.
    ///
    /// Emits `--output-format stream-json` for NDJSON streaming output.
    /// Note: `--verbose` is required when using `--output-format stream-json`.
    pub fn claude() -> Self {
        Self {
            command: "claude".to_string(),
            args: vec![
                "--dangerously-skip-permissions".to_string(),
                "--verbose".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--setting-sources".to_string(),
                "project,local".to_string(),
                "--print".to_string(),
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet".to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::StreamJson,
            env_vars: vec![],
        }
    }

    /// Creates the Claude backend for interactive prompt injection.
    ///
    /// Runs Claude without `-p` flag, passing prompt as a positional argument.
    /// Used by SOP runner for interactive command injection.
    ///
    /// Note: This is NOT for TUI mode - Ralph's TUI uses the standard `claude()`
    /// backend. This is for cases where Claude's interactive mode is needed.
    /// Uses `=` syntax for `--disallowedTools` to prevent variadic consumption
    /// of the positional prompt argument.
    pub fn claude_interactive() -> Self {
        Self {
            command: "claude".to_string(),
            args: vec![
                "--dangerously-skip-permissions".to_string(),
                "--setting-sources".to_string(),
                "project,local".to_string(),
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet".to_string(),
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates a backend from a named backend with additional args.
    ///
    /// # Errors
    /// Returns error if the backend name is invalid.
    pub fn from_name_with_args(
        name: &str,
        extra_args: &[String],
    ) -> Result<Self, CustomBackendError> {
        let mut backend = Self::from_name(name)?;
        backend.args.extend(extra_args.iter().cloned());
        if backend.command == "codex" {
            Self::reconcile_codex_args(&mut backend.args);
        }
        Ok(backend)
    }

    /// Creates a backend from a named backend string.
    ///
    /// # Errors
    /// Returns error if the backend name is invalid.
    pub fn from_name(name: &str) -> Result<Self, CustomBackendError> {
        match name {
            "claude" => Ok(Self::claude()),
            "gemini" => Ok(Self::gemini()),
            "codex" => Ok(Self::codex()),
            "opencode" => Ok(Self::opencode()),
            "pi" => Ok(Self::pi()),
            "traecli" => Ok(Self::traecli()),
            "agent" => Ok(Self::agent()),
            _ => Err(CustomBackendError),
        }
    }

    /// Creates a backend from a HatBackend configuration.
    ///
    /// # Errors
    /// Returns error if the backend configuration is invalid.
    pub fn from_hat_backend(hat_backend: &HatBackend) -> Result<Self, CustomBackendError> {
        match hat_backend {
            HatBackend::Named(name) => Self::from_name(name),
            HatBackend::NamedWithArgs { backend_type, args } => {
                Self::from_name_with_args(backend_type, args)
            }
            HatBackend::Custom { command, args } => Ok(Self {
                command: command.clone(),
                args: args.clone(),
                prompt_mode: PromptMode::Arg,
                prompt_flag: None,
                output_format: OutputFormat::Text,
                env_vars: vec![],
            }),
        }
    }

    /// Creates the Gemini backend.
    pub fn gemini() -> Self {
        Self {
            command: "gemini".to_string(),
            args: vec!["--yolo".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("-p".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Codex backend.
    pub fn codex() -> Self {
        Self {
            command: "codex".to_string(),
            args: vec!["exec".to_string(), "--yolo".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None, // Positional argument
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Claude interactive backend with Agent Teams support.
    ///
    /// Like `claude_interactive()` but with reduced `--disallowedTools` (only `TodoWrite`)
    /// and `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` env var.
    pub fn claude_interactive_teams() -> Self {
        Self {
            command: "claude".to_string(),
            args: vec![
                "--dangerously-skip-permissions".to_string(),
                "--setting-sources".to_string(),
                "project,local".to_string(),
                "--disallowedTools=TodoWrite".to_string(),
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![(
                "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".to_string(),
                "1".to_string(),
            )],
        }
    }

    /// Creates a backend configured for interactive mode with initial prompt.
    ///
    /// This factory method returns the correct backend configuration for running
    /// an interactive session with an initial prompt. The key differences from
    /// headless mode are:
    ///
    /// | Backend | Interactive + Prompt |
    /// |---------|---------------------|
    /// | Claude  | positional arg (no `-p` flag) |
    /// | Gemini  | uses `-i` instead of `-p` |
    /// | Codex   | no `exec` subcommand |
    /// | OpenCode| `run` subcommand with positional prompt |
    ///
    /// # Errors
    /// Returns `CustomBackendError` if the backend name is not recognized.
    pub fn for_interactive_prompt(backend_name: &str) -> Result<Self, CustomBackendError> {
        match backend_name {
            "claude" => Ok(Self::claude_interactive()),
            "gemini" => Ok(Self::gemini_interactive()),
            "codex" => Ok(Self::codex_interactive()),
            "opencode" => Ok(Self::opencode_interactive()),
            "pi" => Ok(Self::pi_interactive()),
            "traecli" => Ok(Self::traecli_interactive()),
            // Cursor `agent` is a headless-only backend for v1; no interactive
            // factory is provided. Falling through to the default `Err` arm
            // keeps `agent` out of any "pseudo-interactive" surface that would
            // silently drop the mandatory `--force`/`--trust` flags (R5/S13).
            _ => Err(CustomBackendError),
        }
    }

    /// Gemini in interactive mode with initial prompt (uses -i, not -p).
    ///
    /// **Critical quirk**: Gemini requires `-i` flag for interactive+prompt mode.
    /// Using `-p` would make it run headless and exit after one response.
    pub fn gemini_interactive() -> Self {
        Self {
            command: "gemini".to_string(),
            args: vec!["--yolo".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("-i".to_string()), // NOT -p!
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Codex in interactive TUI mode (no exec subcommand).
    ///
    /// Unlike headless `codex()`, this runs without `exec` and `--full-auto`
    /// flags, allowing interactive TUI mode.
    pub fn codex_interactive() -> Self {
        Self {
            command: "codex".to_string(),
            args: vec![], // No exec, no --full-auto
            prompt_mode: PromptMode::Arg,
            prompt_flag: None, // Positional argument
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the OpenCode backend for autonomous mode.
    ///
    /// Uses OpenCode CLI with `run` subcommand. The prompt is passed as a
    /// positional argument after the subcommand:
    /// ```bash
    /// opencode run "prompt text here"
    /// ```
    ///
    /// Output is plain text (no JSON streaming available).
    pub fn opencode() -> Self {
        Self {
            command: "opencode".to_string(),
            args: vec!["run".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None, // Positional argument
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the OpenCode TUI backend for interactive mode.
    ///
    /// Runs OpenCode with `run` subcommand. The prompt is passed as a
    /// positional argument:
    /// ```bash
    /// opencode run "prompt text here"
    /// ```
    pub fn opencode_tui() -> Self {
        Self {
            command: "opencode".to_string(),
            args: vec!["run".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None, // Positional argument
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// OpenCode in interactive TUI mode.
    ///
    /// Runs OpenCode TUI with an initial prompt via `--prompt` flag:
    /// ```bash
    /// opencode --prompt "prompt text here"
    /// ```
    ///
    /// Unlike `opencode()` which uses `opencode run` (headless mode),
    /// this launches the interactive TUI and injects the prompt.
    pub fn opencode_interactive() -> Self {
        Self {
            command: "opencode".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("--prompt".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Pi backend for headless execution.
    ///
    /// Uses `-p` for print mode with `--mode json` for NDJSON streaming output.
    /// Emits `PiStreamJson` output format for structured event parsing.
    /// Pins skills to the workspace's `.agents/skills` via `--no-skills` + `--skill`
    /// so the system prompt only carries project skills, not the user-global
    /// skill index (`~/.pi/agent/skills` / `~/.agents/skills`). Global Pi
    /// extensions are left enabled.
    pub fn pi() -> Self {
        Self {
            command: "pi".to_string(),
            args: vec![
                "-p".to_string(),
                "--mode".to_string(),
                "json".to_string(),
                "--no-session".to_string(),
                "--no-skills".to_string(),
                "--skill".to_string(),
                ".agents/skills".to_string(),
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None, // Positional argument
            output_format: OutputFormat::PiStreamJson,
            env_vars: vec![],
        }
    }

    /// Creates the Pi backend for interactive mode with initial prompt.
    ///
    /// Runs pi TUI without `-p` or `--mode json`, passing the prompt as a
    /// positional argument. Used by `ralph plan` for interactive sessions.
    pub fn pi_interactive() -> Self {
        Self {
            command: "pi".to_string(),
            args: vec!["--no-session".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None, // Positional argument
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Trae CLI backend for headless execution.
    ///
    /// Uses `--yolo` to auto-approve tools, `--print` for non-interactive
    /// output, and `--output-format stream-json` to emit NDJSON event
    /// streams (paired with `OutputFormat::TraeStreamJson` so the executor
    /// can parse assistant text, tool calls, and session results).
    /// Without `stream-json`, `trae-cli --yolo --print` exits with code 1
    /// and produces empty stdout.
    pub fn traecli() -> Self {
        Self {
            command: "trae-cli".to_string(),
            args: vec![
                "--yolo".to_string(),
                "--print".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::TraeStreamJson,
            env_vars: vec![],
        }
    }

    /// Creates the Trae CLI backend for interactive mode with initial prompt.
    ///
    /// Runs trae-cli TUI without `--yolo` or `--print`, passing the prompt
    /// as a positional argument. Output is plain `Text` because interactive
    /// TUI mode does not emit the stream-json protocol — see
    /// `claude_interactive()` for the same pattern.
    /// Used by `ralph plan` for interactive sessions.
    pub fn traecli_interactive() -> Self {
        Self {
            command: "trae-cli".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Cursor Headless CLI `agent` backend for headless execution.
    ///
    /// Contract (R4–R6 / R5):
    /// - `-p` / `--print` is the print flag (we pass prompt as positional arg).
    /// - `--force` + `--trust` are pinned (R5: factory-level, no public knob
    ///   to drop them — `NamedWithArgs` may append more args but cannot
    ///   strip these).
    /// - `--output-format stream-json` paired with `OutputFormat::AgentStreamJson`
    ///   so `PtyExecutor` can dispatch to the `agent_stream` parser (S4).
    /// - The CLI binary name is `agent`, matching PATH discovery for `auto`.
    pub fn agent() -> Self {
        Self {
            command: "agent".to_string(),
            args: vec![
                "-p".to_string(),
                "--force".to_string(),
                "--trust".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None, // positional arg follows `-p`
            output_format: OutputFormat::AgentStreamJson,
            env_vars: vec![],
        }
    }

    /// Creates a custom backend from configuration.
    ///
    /// # Errors
    /// Returns `CustomBackendError` if no command is specified.
    pub fn custom(config: &CliConfig) -> Result<Self, CustomBackendError> {
        let command = config.command.clone().ok_or(CustomBackendError)?;
        let prompt_mode = if config.prompt_mode == "stdin" {
            PromptMode::Stdin
        } else {
            PromptMode::Arg
        };

        Ok(Self {
            command,
            args: config.args.clone(),
            prompt_mode,
            prompt_flag: config.prompt_flag.clone(),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        })
    }

    /// Builds the command for PTY (non-interactive) execution.
    ///
    /// Forces arg mode to avoid PTY line-discipline deadlocks on large prompts.
    /// The PTY canonical input buffer (~4 KB) cannot handle 30-50 KB+ prompts
    /// delivered via stdin. Instead, the prompt is passed as a command argument
    /// (with temp-file indirection for prompts over 7000 chars).  See #280.
    pub fn build_command_pty(
        &self,
        prompt: &str,
    ) -> (String, Vec<String>, Option<String>, Option<NamedTempFile>) {
        if self.prompt_mode == PromptMode::Stdin {
            // Convert stdin-mode to arg-mode for PTY safety
            let mut pty_backend = self.clone();
            pty_backend.prompt_mode = PromptMode::Arg;
            // Use -p flag for Claude when forcing arg mode
            if pty_backend.prompt_flag.is_none() {
                pty_backend.prompt_flag = Some("-p".to_string());
            }
            pty_backend.build_command(prompt, false)
        } else {
            self.build_command(prompt, false)
        }
    }

    /// Builds the full command with arguments for execution.
    ///
    /// # Arguments
    /// * `prompt` - The prompt text to pass to the agent
    /// * `interactive` - Whether to run in interactive mode (affects agent flags)
    pub fn build_command(
        &self,
        prompt: &str,
        interactive: bool,
    ) -> (String, Vec<String>, Option<String>, Option<NamedTempFile>) {
        let mut args = self.args.clone();

        // Filter args based on execution mode per interactive-mode.spec.md
        if interactive {
            args = self.filter_args_for_interactive(args);
        }

        // Handle prompt passing: all backends use temp file for large prompts
        let (stdin_input, temp_file) = match self.prompt_mode {
            PromptMode::Arg => {
                // Use temp file for large prompts (>7000 chars) to avoid shell ARG_MAX limits
                let (prompt_text, temp_file) = if prompt.len() > 7000 {
                    match NamedTempFile::new() {
                        Ok(mut file) => {
                            if let Err(e) = file.write_all(prompt.as_bytes()) {
                                tracing::warn!("Failed to write prompt to temp file: {}", e);
                                (prompt.to_string(), None)
                            } else {
                                let path = file.path().display().to_string();
                                (
                                    format!("Please read and execute the task in {}", path),
                                    Some(file),
                                )
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to create temp file: {}", e);
                            (prompt.to_string(), None)
                        }
                    }
                } else {
                    (prompt.to_string(), None)
                };

                if let Some(ref flag) = self.prompt_flag {
                    args.push(flag.clone());
                }
                args.push(prompt_text);
                (None, temp_file)
            }
            PromptMode::Stdin => (Some(prompt.to_string()), None),
        };

        // Log the full command being built
        tracing::debug!(
            command = %self.command,
            args_count = args.len(),
            prompt_len = prompt.len(),
            interactive = interactive,
            uses_stdin = stdin_input.is_some(),
            uses_temp_file = temp_file.is_some(),
            "Built CLI command"
        );
        // Log full prompt at trace level for debugging
        tracing::trace!(prompt = %prompt, "Full prompt content");

        (self.command.clone(), args, stdin_input, temp_file)
    }

    /// Filters args for interactive mode per spec table.
    fn filter_args_for_interactive(&self, args: Vec<String>) -> Vec<String> {
        match self.command.as_str() {
            "codex" => args.into_iter().filter(|a| a != "--full-auto").collect(),
            "claude" => args.into_iter().filter(|a| a != "--print").collect(),
            "trae-cli" => args
                .into_iter()
                .filter(|a| a != "--yolo" && a != "--print")
                .collect(),
            _ => args, // gemini, opencode unchanged
        }
    }

    fn reconcile_codex_args(args: &mut Vec<String>) {
        let had_dangerous_bypass = args
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox");
        if had_dangerous_bypass {
            args.retain(|arg| arg != "--dangerously-bypass-approvals-and-sandbox");
            if !args.iter().any(|arg| arg == "--yolo") {
                if let Some(pos) = args.iter().position(|arg| arg == "exec") {
                    args.insert(pos + 1, "--yolo".to_string());
                } else {
                    args.push("--yolo".to_string());
                }
            }
        }

        if args.iter().any(|arg| arg == "--yolo") {
            args.retain(|arg| arg != "--full-auto");
            // Collapse duplicate --yolo entries to a single flag.
            let mut seen_yolo = false;
            args.retain(|arg| {
                if arg == "--yolo" {
                    if seen_yolo {
                        return false;
                    }
                    seen_yolo = true;
                }
                true
            });
            if !seen_yolo {
                if let Some(pos) = args.iter().position(|arg| arg == "exec") {
                    args.insert(pos + 1, "--yolo".to_string());
                } else {
                    args.push("--yolo".to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_roo_prompt_file_helper_removed() {
        // U3: roo backend and its helper `build_roo_prompt_file` are deleted.
        // We verify the helpers are gone by source-grepping cli_backend.rs.
        let src = include_str!("cli_backend.rs");
        let has_helper = src
            .lines()
            .map(str::trim_start)
            .any(|l| l.starts_with("fn build_roo_prompt_file"));
        let has_factory = src
            .lines()
            .map(str::trim_start)
            .any(|l| l.starts_with("pub fn roo()"));
        let has_interactive = src
            .lines()
            .map(str::trim_start)
            .any(|l| l.starts_with("pub fn roo_interactive()"));
        assert!(!has_helper, "build_roo_prompt_file helper must be deleted");
        assert!(!has_factory, "CliBackend::roo() factory must be deleted");
        assert!(
            !has_interactive,
            "CliBackend::roo_interactive() factory must be deleted"
        );
    }

    #[test]
    fn test_claude_backend() {
        let backend = CliBackend::claude();
        let (cmd, args, stdin, temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "claude");
        assert_eq!(
            args,
            vec![
                "--dangerously-skip-permissions",
                "--verbose",
                "--output-format",
                "stream-json",
                "--setting-sources",
                "project,local",
                "--print",
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
            ]
        );
        assert_eq!(stdin, Some("test prompt".to_string()));
        assert!(temp.is_none());
        assert_eq!(backend.output_format, OutputFormat::StreamJson);
    }

    #[test]
    fn test_claude_interactive_backend() {
        let backend = CliBackend::claude_interactive();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "claude");
        // Should have --dangerously-skip-permissions, --setting-sources, --disallowedTools=..., and prompt as positional arg
        // No -p flag, no --output-format, no --verbose
        // Uses = syntax to prevent variadic consumption of the prompt
        assert_eq!(
            args,
            vec![
                "--dangerously-skip-permissions",
                "--setting-sources",
                "project,local",
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
                "test prompt"
            ]
        );
        assert!(stdin.is_none()); // Uses positional arg, not stdin
        assert_eq!(backend.output_format, OutputFormat::Text);
        assert_eq!(backend.prompt_flag, None);
    }

    #[test]
    fn test_claude_large_prompt_uses_stdin_not_temp_file() {
        let backend = CliBackend::claude();
        let large_prompt = "x".repeat(7001);
        let (cmd, args, stdin, temp) = backend.build_command(&large_prompt, false);

        assert_eq!(cmd, "claude");
        assert!(args.contains(&"--print".to_string()));
        assert_eq!(stdin, Some(large_prompt));
        assert!(temp.is_none());
    }

    /// Regression test for #280: build_command_pty converts Claude's stdin mode
    /// to arg mode so large prompts don't deadlock the PTY line discipline.
    #[test]
    fn test_claude_build_command_pty_uses_arg_mode() {
        let backend = CliBackend::claude();
        let large_prompt = "x".repeat(7001);
        let (cmd, args, stdin, temp) = backend.build_command_pty(&large_prompt);

        assert_eq!(cmd, "claude");
        // --print should still be present (headless mode flag)
        assert!(args.contains(&"--print".to_string()));
        // stdin should be None — prompt delivered via arg, not PTY stdin
        assert!(stdin.is_none(), "PTY mode should not use stdin");
        // Large prompt should use temp file
        assert!(
            temp.is_some(),
            "Large prompt in PTY mode should use temp file"
        );
        assert!(args.iter().any(|a| a.contains("Please read and execute")));
    }

    #[test]
    fn test_claude_build_command_pty_small_prompt_uses_arg_directly() {
        let backend = CliBackend::claude();
        let (cmd, args, stdin, temp) = backend.build_command_pty("small prompt");

        assert_eq!(cmd, "claude");
        assert!(args.contains(&"--print".to_string()));
        assert!(stdin.is_none());
        assert!(temp.is_none());
        // The prompt should be a direct arg with -p flag
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"small prompt".to_string()));
    }

    #[test]
    fn test_for_interactive_prompt_gemini() {
        let backend = CliBackend::for_interactive_prompt("gemini").unwrap();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "gemini");
        // Critical: should use -i flag, NOT -p
        assert_eq!(args, vec!["--yolo", "-i", "test prompt"]);
        assert_eq!(backend.prompt_flag, Some("-i".to_string()));
        assert!(stdin.is_none());
    }

    #[test]
    fn test_for_interactive_prompt_codex() {
        let backend = CliBackend::for_interactive_prompt("codex").unwrap();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "codex");
        // Should NOT have exec or --full-auto
        assert_eq!(args, vec!["test prompt"]);
        assert!(!args.contains(&"exec".to_string()));
        assert!(!args.contains(&"--full-auto".to_string()));
        assert!(stdin.is_none());
    }

    #[test]
    fn test_for_interactive_prompt_invalid() {
        let result = CliBackend::for_interactive_prompt("invalid_backend");
        assert!(result.is_err());
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Tests for OpenCode backend
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_opencode_backend() {
        let backend = CliBackend::opencode();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "opencode");
        // Uses `run` subcommand with positional prompt arg
        assert_eq!(args, vec!["run", "test prompt"]);
        assert!(stdin.is_none());
        assert_eq!(backend.output_format, OutputFormat::Text);
        assert_eq!(backend.prompt_flag, None);
    }

    #[test]
    fn test_opencode_tui_backend() {
        let backend = CliBackend::opencode_tui();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "opencode");
        // Uses `run` subcommand with positional prompt arg
        assert_eq!(args, vec!["run", "test prompt"]);
        assert!(stdin.is_none());
        assert_eq!(backend.output_format, OutputFormat::Text);
        assert_eq!(backend.prompt_flag, None);
    }

    #[test]
    fn test_opencode_interactive_mode_unchanged() {
        // OpenCode has no flags to filter in interactive mode
        let backend = CliBackend::opencode();
        let (cmd, args_auto, stdin_auto, _) = backend.build_command("test prompt", false);
        let (_, args_interactive, stdin_interactive, _) =
            backend.build_command("test prompt", true);

        assert_eq!(cmd, "opencode");
        // Should be identical in both modes
        assert_eq!(args_auto, args_interactive);
        assert_eq!(args_auto, vec!["run", "test prompt"]);
        assert!(stdin_auto.is_none());
        assert!(stdin_interactive.is_none());
    }

    #[test]
    fn test_from_name_opencode() {
        let backend = CliBackend::from_name("opencode").unwrap();
        assert_eq!(backend.command, "opencode");
        assert_eq!(backend.prompt_flag, None); // Positional argument
    }

    #[test]
    fn test_for_interactive_prompt_opencode() {
        let backend = CliBackend::for_interactive_prompt("opencode").unwrap();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "opencode");
        // Uses --prompt flag for TUI mode (no `run` subcommand)
        assert_eq!(args, vec!["--prompt", "test prompt"]);
        assert!(stdin.is_none());
        assert_eq!(backend.prompt_flag, Some("--prompt".to_string()));
    }

    #[test]
    fn test_opencode_interactive_launches_tui_not_headless() {
        // Issue #96: opencode backend doesn't start interactive session with ralph plan
        //
        // The bug: opencode_interactive() uses `opencode run "prompt"` which is headless mode.
        // The fix: Interactive mode should use `opencode --prompt "prompt"` (without `run`)
        // to launch the TUI with an initial prompt.
        //
        // From `opencode --help`:
        // - `opencode [project]` = start opencode tui (interactive mode) [default]
        // - `opencode run [message..]` = run opencode with a message (headless mode)
        let backend = CliBackend::opencode_interactive();
        let (cmd, args, _, _) = backend.build_command("test prompt", true);

        assert_eq!(cmd, "opencode");
        // Interactive mode should NOT include "run" subcommand
        // `run` makes opencode execute headlessly, which defeats the purpose of interactive mode
        assert!(
            !args.contains(&"run".to_string()),
            "opencode_interactive() should not use 'run' subcommand. \
             'opencode run' is headless mode, but interactive mode needs TUI. \
             Expected: opencode --prompt \"test prompt\", got: opencode {}",
            args.join(" ")
        );
        // Should pass prompt via --prompt flag for TUI mode
        assert!(
            args.contains(&"--prompt".to_string()),
            "opencode_interactive() should use --prompt flag for TUI mode. \
             Expected args to contain '--prompt', got: {:?}",
            args
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Tests for Pi backend
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_pi_backend() {
        let backend = CliBackend::pi();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "pi");
        assert_eq!(
            args,
            vec![
                "-p",
                "--mode",
                "json",
                "--no-session",
                "--no-skills",
                "--skill",
                ".agents/skills",
                "test prompt",
            ]
        );
        assert!(stdin.is_none());
        assert_eq!(backend.output_format, OutputFormat::PiStreamJson);
        assert_eq!(backend.prompt_flag, None); // Positional argument
    }

    #[test]
    fn test_pi_interactive_backend() {
        let backend = CliBackend::pi_interactive();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "pi");
        // No -p, no --mode json, just --no-session + positional prompt
        assert_eq!(args, vec!["--no-session", "test prompt"]);
        assert!(stdin.is_none());
        assert_eq!(backend.output_format, OutputFormat::Text);
        assert_eq!(backend.prompt_flag, None);
    }

    #[test]
    fn test_from_name_pi() {
        let backend = CliBackend::from_name("pi").unwrap();
        assert_eq!(backend.command, "pi");
        assert_eq!(backend.prompt_flag, None);
        assert_eq!(backend.output_format, OutputFormat::PiStreamJson);
    }

    #[test]
    fn test_for_interactive_prompt_pi() {
        let backend = CliBackend::for_interactive_prompt("pi").unwrap();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "pi");
        assert_eq!(args, vec!["--no-session", "test prompt"]);
        assert!(stdin.is_none());
        assert_eq!(backend.output_format, OutputFormat::Text);
    }

    #[test]
    fn test_from_config_pi() {
        let config = CliConfig {
            backend: "pi".to_string(),
            command: None,
            prompt_mode: "arg".to_string(),
            args: vec![
                "--provider".to_string(),
                "zai".to_string(),
                "--model".to_string(),
                "glm-5".to_string(),
            ],
            ..Default::default()
        };
        let backend = CliBackend::from_config(&config).unwrap();
        let (_cmd, args, _stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(backend.command, "pi");
        assert_eq!(backend.output_format, OutputFormat::PiStreamJson);
        // Headless defaults + user-supplied provider/model + prompt, in that order.
        assert_eq!(
            args,
            vec![
                "-p",
                "--mode",
                "json",
                "--no-session",
                "--no-skills",
                "--skill",
                ".agents/skills",
                "--provider",
                "zai",
                "--model",
                "glm-5",
                "test prompt",
            ]
        );
    }

    #[test]
    fn test_from_hat_backend_named_with_args_pi() {
        let hat_backend = HatBackend::NamedWithArgs {
            backend_type: "pi".to_string(),
            args: vec![
                "--provider".to_string(),
                "anthropic".to_string(),
                "--model".to_string(),
                "claude-sonnet-4".to_string(),
            ],
        };
        let backend = CliBackend::from_hat_backend(&hat_backend).unwrap();
        let (cmd, args, _, _) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "pi");
        // Default args + extra args + prompt, in that order, with the new
        // skill budget pinned before user-supplied provider/model flags.
        assert_eq!(
            args,
            vec![
                "-p",
                "--mode",
                "json",
                "--no-session",
                "--no-skills",
                "--skill",
                ".agents/skills",
                "--provider",
                "anthropic",
                "--model",
                "claude-sonnet-4",
                "test prompt",
            ]
        );
    }

    #[test]
    fn test_pi_large_prompt_uses_temp_file() {
        let backend = CliBackend::pi();
        let large_prompt = "x".repeat(7001);
        let (cmd, args, _stdin, temp) = backend.build_command(&large_prompt, false);

        assert_eq!(cmd, "pi");
        assert!(temp.is_some());
        assert!(args.iter().any(|a| a.contains("Please read and execute")));
    }

    #[test]
    fn test_pi_interactive_mode_unchanged() {
        // Pi has no flags to filter in interactive mode
        let backend = CliBackend::pi();
        let (_, args_auto, _, _) = backend.build_command("test prompt", false);
        let (_, args_interactive, _, _) = backend.build_command("test prompt", true);

        assert_eq!(args_auto, args_interactive);
    }

    #[test]
    fn test_custom_args_can_be_appended() {
        // Verify that custom args can be appended to backend args
        // This is used for `ralph run -b opencode -- --model="some-model"`
        let mut backend = CliBackend::opencode();

        // Append custom args
        let custom_args = vec!["--model=gpt-4".to_string(), "--temperature=0.7".to_string()];
        backend.args.extend(custom_args.clone());

        // Build command and verify custom args are included
        let (cmd, args, _, _) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "opencode");
        // Should have: original args + custom args + prompt
        assert!(args.contains(&"run".to_string())); // Original arg
        assert!(args.contains(&"--model=gpt-4".to_string())); // Custom arg
        assert!(args.contains(&"--temperature=0.7".to_string())); // Custom arg
        assert!(args.contains(&"test prompt".to_string())); // Prompt

        // Verify order: original args come before custom args
        let run_idx = args.iter().position(|a| a == "run").unwrap();
        let model_idx = args.iter().position(|a| a == "--model=gpt-4").unwrap();
        assert!(
            run_idx < model_idx,
            "Original args should come before custom args"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Tests for Agent Teams backends
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_claude_interactive_teams_backend() {
        let backend = CliBackend::claude_interactive_teams();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "claude");
        assert_eq!(
            args,
            vec![
                "--dangerously-skip-permissions",
                "--setting-sources",
                "project,local",
                "--disallowedTools=TodoWrite",
                "test prompt"
            ]
        );
        assert!(stdin.is_none());
        assert_eq!(backend.output_format, OutputFormat::Text);
        assert_eq!(backend.prompt_flag, None);
        assert_eq!(
            backend.env_vars,
            vec![(
                "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".to_string(),
                "1".to_string()
            )]
        );
    }

    #[test]
    fn test_env_vars_default_empty() {
        // All non-teams constructors should have empty env_vars
        assert!(CliBackend::claude().env_vars.is_empty());
        assert!(CliBackend::claude_interactive().env_vars.is_empty());
        assert!(CliBackend::gemini().env_vars.is_empty());
        assert!(CliBackend::codex().env_vars.is_empty());
        assert!(CliBackend::opencode().env_vars.is_empty());
        assert!(CliBackend::pi().env_vars.is_empty());
    }

    #[test]
    fn test_all_claude_constructors_isolate_user_settings() {
        let claude = CliBackend::claude();
        let claude_interactive = CliBackend::claude_interactive();
        let claude_interactive_teams = CliBackend::claude_interactive_teams();
        let interactive_prompt = CliBackend::for_interactive_prompt("claude").unwrap();

        for backend in [
            &claude,
            &claude_interactive,
            &claude_interactive_teams,
            &interactive_prompt,
        ] {
            let mut setting_sources = backend
                .args
                .windows(2)
                .filter(|window| window[0] == "--setting-sources")
                .map(|window| window[1].as_str());

            assert_eq!(setting_sources.next(), Some("project,local"));
            assert_eq!(setting_sources.next(), None);
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Tests for Trae CLI backend
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_traecli_backend() {
        let backend = CliBackend::traecli();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "trae-cli");
        assert_eq!(
            args,
            vec![
                "--yolo",
                "--print",
                "--output-format",
                "stream-json",
                "test prompt"
            ]
        );
        assert!(stdin.is_none());
        assert_eq!(backend.output_format, OutputFormat::TraeStreamJson);
        assert_eq!(backend.prompt_flag, None);
    }

    #[test]
    fn test_traecli_interactive() {
        let backend = CliBackend::traecli_interactive();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "trae-cli");
        assert_eq!(args, vec!["test prompt"]);
        assert!(stdin.is_none());
        assert_eq!(backend.output_format, OutputFormat::Text);
        assert_eq!(backend.prompt_flag, None);
    }

    #[test]
    fn test_from_name_traecli() {
        let backend = CliBackend::from_name("traecli").unwrap();
        assert_eq!(backend.command, "trae-cli");
        assert_eq!(backend.prompt_flag, None);
        assert_eq!(backend.output_format, OutputFormat::TraeStreamJson);
    }

    #[test]
    fn test_from_config_traecli() {
        let config = CliConfig {
            backend: "traecli".to_string(),
            command: None,
            prompt_mode: "arg".to_string(),
            ..Default::default()
        };
        let backend = CliBackend::from_config(&config).unwrap();

        assert_eq!(backend.command, "trae-cli");
        assert_eq!(backend.output_format, OutputFormat::TraeStreamJson);
        assert!(backend.args.contains(&"--yolo".to_string()));
        assert!(backend.args.contains(&"--print".to_string()));
        assert!(backend.args.contains(&"--output-format".to_string()));
        assert!(backend.args.contains(&"stream-json".to_string()));
    }

    #[test]
    fn test_for_interactive_prompt_traecli() {
        let backend = CliBackend::for_interactive_prompt("traecli").unwrap();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "trae-cli");
        assert_eq!(args, vec!["test prompt"]);
        assert!(stdin.is_none());
        assert_eq!(backend.output_format, OutputFormat::Text);
    }

    #[test]
    fn test_traecli_interactive_mode_removes_yolo_print() {
        let backend = CliBackend::traecli();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", true);

        assert_eq!(cmd, "trae-cli");
        // In interactive mode, --yolo and --print should be removed
        assert!(
            !args.contains(&"--yolo".to_string()),
            "interactive mode should remove --yolo"
        );
        assert!(
            !args.contains(&"--print".to_string()),
            "interactive mode should remove --print"
        );
        assert!(stdin.is_none());
    }

    #[test]
    fn test_traecli_env_vars_default_empty() {
        assert!(CliBackend::traecli().env_vars.is_empty());
        assert!(CliBackend::traecli_interactive().env_vars.is_empty());
    }

    // ---------- Cursor `agent` backend (U2 — S1, S9, S10, S13) ----------

    #[test]
    fn test_agent_backend() {
        // S1: command is `agent`; args include `-p`, `--force`, `--trust`,
        // `--output-format`, `stream-json`; output_format is AgentStreamJson.
        let backend = CliBackend::agent();
        let (cmd, args, stdin, _temp) = backend.build_command("test prompt", false);

        assert_eq!(cmd, "agent");
        // Force/trust must remain present even after NamedWithArgs can append
        // more args; build_command does not strip them (factory-only contract).
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"--force".to_string()));
        assert!(args.contains(&"--trust".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        // Positional prompt at the tail (no -p prompt_flag).
        assert_eq!(args.last().map(String::as_str), Some("test prompt"));
        assert!(stdin.is_none());
        assert_eq!(backend.output_format, OutputFormat::AgentStreamJson);
        assert_eq!(backend.prompt_flag, None);
        assert_eq!(backend.prompt_mode, PromptMode::Arg);
    }

    #[test]
    fn test_from_name_agent() {
        // S1 + S9 boundary: `agent` must round-trip through from_name.
        let backend = CliBackend::from_name("agent").expect("agent must be recognized");
        assert_eq!(backend.command, "agent");
        assert_eq!(backend.output_format, OutputFormat::AgentStreamJson);
    }

    #[test]
    fn test_from_name_unknown_still_errors() {
        // S9: unknown backend name (e.g. `bogus`) still returns CustomBackendError.
        // We must NOT silently fall back to claude via from_name; only from_config
        // has the silent fallback (legacy config behavior, preserved by design).
        assert!(CliBackend::from_name("bogus").is_err());
        assert!(CliBackend::from_name("agent-but-not").is_err());
    }

    #[test]
    fn test_from_config_agent() {
        // S1 + R2: CliConfig { backend: "agent" } resolves to the agent factory.
        let cfg = CliConfig {
            backend: "agent".to_string(),
            command: None,
            args: vec![],
            prompt_mode: "arg".to_string(),
            default_mode: "autonomous".to_string(),
            idle_timeout_secs: 30,
            autonomous_idle_timeout_secs: None,
            prompt_flag: None,
        };
        let backend = CliBackend::from_config(&cfg).expect("agent must resolve from config");
        assert_eq!(backend.command, "agent");
        assert!(backend.args.contains(&"--force".to_string()));
        assert!(backend.args.contains(&"--trust".to_string()));
        assert_eq!(backend.output_format, OutputFormat::AgentStreamJson);
    }

    #[test]
    fn test_for_interactive_prompt_agent_errors() {
        // S10: agent has no interactive factory; for_interactive_prompt must
        // return Err (not silently produce a stripped-down agent backend).
        assert!(CliBackend::for_interactive_prompt("agent").is_err());
    }

    #[test]
    fn test_agent_args_include_force_and_trust_after_extra_args() {
        // S13: `NamedWithArgs` may append more args but must not be able to
        // delete --force/--trust. Factory guarantees presence; build_command
        // does not filter them. We verify by appending an extra arg via
        // from_name_with_args and asserting both flags still appear.
        let backend = CliBackend::from_name_with_args("agent", &["--some-extra".to_string()])
            .expect("agent must resolve");
        let (cmd, args, _stdin, _temp) = backend.build_command("p", false);
        assert_eq!(cmd, "agent");
        assert!(
            args.contains(&"--force".to_string()),
            "--force must persist"
        );
        assert!(
            args.contains(&"--trust".to_string()),
            "--trust must persist"
        );
        assert!(args.contains(&"--some-extra".to_string()));
    }
}
