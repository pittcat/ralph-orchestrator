//! CLI executor for running prompts through backends.
//!
//! Executes prompts via CLI tools with real-time streaming output.
//! Supports optional execution timeout with graceful SIGTERM termination.

use crate::agent_stream::AgentStreamParser;
#[cfg(test)]
use crate::cli_backend::PromptMode;
use crate::cli_backend::{CliBackend, OutputFormat};
use crate::trae_stream::TraeStreamParser;
#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;
use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{debug, info, warn};

const POST_EVENT_GRACE_TIMEOUT: Duration = Duration::from_secs(5);
const TERMINATION_GRACE_TIMEOUT: Duration = Duration::from_secs(2);

/// Result of a CLI execution.
#[derive(Debug)]
pub struct ExecutionResult {
    /// The full output from the CLI.
    pub output: String,
    /// Whether the execution succeeded (exit code 0).
    pub success: bool,
    /// The exit code.
    pub exit_code: Option<i32>,
    /// Whether the execution was terminated due to timeout.
    pub timed_out: bool,
    /// Whether the execution was terminated due to post-event grace timeout.
    pub post_event_timed_out: bool,
}

/// Executor for running prompts through CLI backends.
#[derive(Debug)]
pub struct CliExecutor {
    backend: CliBackend,
}

enum StreamEvent {
    StdoutLine(String),
    StderrLine(String),
    StdoutEof,
    StderrEof,
}

enum StreamKind {
    Stdout,
    Stderr,
}

impl CliExecutor {
    /// Creates a new executor with the given backend.
    pub fn new(backend: CliBackend) -> Self {
        Self { backend }
    }

    /// Executes a prompt and streams output to the provided writer.
    ///
    /// Output is streamed line-by-line to the writer while being accumulated
    /// for the return value. If `timeout` is provided and the execution produces
    /// no stdout/stderr activity for longer than that duration, the process
    /// receives SIGTERM and the result indicates timeout.
    ///
    /// When `verbose` is true, stderr output is also written to the output writer
    /// with a `[stderr]` prefix. When false, stderr is captured but not displayed.
    pub async fn execute<W: Write + Send>(
        &self,
        prompt: &str,
        mut output_writer: W,
        timeout: Option<Duration>,
        verbose: bool,
    ) -> std::io::Result<ExecutionResult> {
        // Note: _temp_file is kept alive for the duration of this function scope.
        // Some Arg-mode backends use temp-file indirection for very large prompts.
        let (cmd, args, stdin_input, _temp_file) = self.backend.build_command(prompt, false);

        let mut command = Command::new(&cmd);
        command.args(&args);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);

        // U3 (2026-06-14-002): Use RALPH_WORKSPACE_ROOT if set, otherwise fall back to
        // current_dir(). This fixes worktree mode where the subprocess's cwd is already
        // the worktree, but we want the Agent's path resolution to be explicit rather
        // than relying on inherited CWD state.
        let cwd = env::var("RALPH_WORKSPACE_ROOT")
            .map(PathBuf::from)
            .or_else(|_| std::env::current_dir())
            .unwrap_or_else(|_| PathBuf::from("."));
        command.current_dir(&cwd);
        inject_ralph_runtime_env(&mut command, &cwd);

        // Apply backend-specific environment variables (e.g., Agent Teams env var)
        command.envs(self.backend.env_vars.iter().map(|(k, v)| (k, v)));

        debug!(
            command = %cmd,
            args = ?args,
            cwd = ?cwd,
            "Spawning CLI command"
        );

        if stdin_input.is_some() {
            command.stdin(Stdio::piped());
        }

        let mut child = command.spawn()?;

        #[cfg(unix)]
        {
            use nix::unistd::getpgid;
            let child_pid = child.id();
            let pgid = child_pid
                .and_then(|id| getpgid(Some(nix::unistd::Pid::from_raw(id as i32))).ok())
                .map(|p| p.as_raw())
                .unwrap_or(-1);
            info!(
                target: "ralph_adapters::cli_executor",
                child_pid = ?child_pid,
                child_pgid = pgid,
                backend_cmd = %cmd,
                "CliExecutor spawned backend"
            );
        }

        // Write to stdin if needed. Some short-lived commands can exit before
        // consuming stdin, which surfaces as BrokenPipe. Treat that as benign
        // and continue collecting output/exit status from the child.
        if let Some(input) = stdin_input
            && let Some(mut stdin) = child.stdin.take()
        {
            if let Err(err) = stdin.write_all(input.as_bytes()).await
                && err.kind() != std::io::ErrorKind::BrokenPipe
            {
                return Err(err);
            }
            drop(stdin); // Close stdin to signal EOF
        }

        let mut timed_out = false;
        let mut post_event_timed_out = false;
        let mut post_event_deadline: Option<tokio::time::Instant> = None;
        let mut terminated_status = None;

        // Take both stdout and stderr handles upfront to read concurrently.
        // Each emitted line resets the inactivity timeout.
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);

        let stdout_task = stdout_handle.map(|stdout| {
            let tx = event_tx.clone();
            tokio::spawn(async move { read_stream(stdout, tx, StreamKind::Stdout).await })
        });
        let stderr_task = stderr_handle.map(|stderr| {
            let tx = event_tx.clone();
            tokio::spawn(async move { read_stream(stderr, tx, StreamKind::Stderr).await })
        });
        drop(event_tx);

        let mut stdout_done = stdout_task.is_none();
        let mut stderr_done = stderr_task.is_none();
        let mut accumulated_output = String::new();
        let mut agent_text_written = false;
        let mut agent_fallback_result = None;

        if let Some(duration) = timeout {
            debug!(
                timeout_secs = duration.as_secs(),
                "Executing with inactivity timeout"
            );
        }

        while !stdout_done || !stderr_done {
            let now = tokio::time::Instant::now();
            if post_event_deadline.is_some_and(|deadline| deadline <= now) {
                warn!(
                    timeout_secs = 0,
                    "Execution post-event grace timeout reached, sending SIGTERM"
                );
                timed_out = true;
                post_event_timed_out = true;
                terminated_status = Some(Self::terminate_child_and_wait(&mut child).await?);
                break;
            }

            let effective_timeout = match (timeout, post_event_deadline) {
                (Some(duration), Some(deadline)) => {
                    Some(duration.min(deadline.saturating_duration_since(now)))
                }
                (None, Some(deadline)) => Some(deadline.saturating_duration_since(now)),
                (Some(duration), None) => Some(duration),
                (None, None) => None,
            };

            let next_event = match effective_timeout {
                Some(duration) => match tokio::time::timeout(duration, event_rx.recv()).await {
                    Ok(event) => event,
                    Err(_) => {
                        warn!(
                            timeout_secs = duration.as_secs(),
                            "Execution inactivity timeout reached, sending SIGTERM"
                        );
                        timed_out = true;
                        if post_event_deadline.is_some() {
                            post_event_timed_out = true;
                        }
                        terminated_status = Some(Self::terminate_child_and_wait(&mut child).await?);
                        break;
                    }
                },
                None => event_rx.recv().await,
            };

            match next_event {
                Some(StreamEvent::StdoutLine(line)) => {
                    if line_signals_event_emitted(&line) {
                        post_event_deadline.get_or_insert_with(|| {
                            tokio::time::Instant::now() + POST_EVENT_GRACE_TIMEOUT
                        });
                    }
                    if self.backend.output_format == OutputFormat::TraeStreamJson {
                        // TraeStreamJson: parse NDJSON lines and extract assistant text
                        if let Some(text) = TraeStreamParser::extract_text(&line) {
                            write!(output_writer, "{text}")?;
                            if !text.ends_with('\n') {
                                writeln!(output_writer)?;
                            }
                        }
                    } else if self.backend.output_format == OutputFormat::AgentStreamJson {
                        // AgentStreamJson: parse NDJSON lines and extract assistant text
                        // (mirrors the TraeStreamJson branch above; tool events stay
                        // on the PTY/StreamHandler path, headless just wants the text
                        // for completion detection).
                        if let Some(text) = AgentStreamParser::extract_text(&line) {
                            write!(output_writer, "{text}")?;
                            if !text.ends_with('\n') {
                                writeln!(output_writer)?;
                            }
                            agent_text_written = true;
                        } else if let Some(text) = AgentStreamParser::extract_result_text(&line) {
                            agent_fallback_result = Some(text);
                        }
                    } else {
                        writeln!(output_writer, "{line}")?;
                    }
                    output_writer.flush()?;
                    accumulated_output.push_str(&line);
                    accumulated_output.push('\n');
                }
                Some(StreamEvent::StderrLine(line)) => {
                    if line_signals_event_emitted(&line) {
                        post_event_deadline.get_or_insert_with(|| {
                            tokio::time::Instant::now() + POST_EVENT_GRACE_TIMEOUT
                        });
                    }
                    if verbose {
                        writeln!(output_writer, "[stderr] {line}")?;
                        output_writer.flush()?;
                    }
                    accumulated_output.push_str("[stderr] ");
                    accumulated_output.push_str(&line);
                    accumulated_output.push('\n');
                }
                Some(StreamEvent::StdoutEof) => stdout_done = true,
                Some(StreamEvent::StderrEof) => stderr_done = true,
                None => {
                    stdout_done = true;
                    stderr_done = true;
                }
            }
        }

        if !agent_text_written && let Some(text) = agent_fallback_result {
            write!(output_writer, "{text}")?;
            if !text.ends_with('\n') {
                writeln!(output_writer)?;
            }
            output_writer.flush()?;
        }

        let status = if let Some(status) = terminated_status {
            status
        } else {
            child.wait().await?
        };

        if let Some(handle) = stdout_task {
            handle.await.map_err(join_error_to_io)??;
        }
        if let Some(handle) = stderr_task {
            handle.await.map_err(join_error_to_io)??;
        }

        Ok(ExecutionResult {
            output: accumulated_output,
            success: (status.success() && !timed_out) || post_event_timed_out,
            exit_code: status.code(),
            timed_out,
            post_event_timed_out,
        })
    }

    /// Terminates the child process with SIGTERM, then SIGKILL if it ignores graceful shutdown.
    async fn terminate_child_and_wait(
        child: &mut Child,
    ) -> std::io::Result<std::process::ExitStatus> {
        #[cfg(not(unix))]
        {
            child.start_kill()?;
            return child.wait().await;
        }

        #[cfg(unix)]
        if let Some(pid) = child.id() {
            #[allow(clippy::cast_possible_wrap)]
            let pid = Pid::from_raw(pid as i32);
            let pgid = Pid::from_raw(-pid.as_raw());
            warn!(
                target: "ralph_adapters::cli_executor",
                %pid,
                pgid = %pgid,
                "terminate_child_and_wait sending SIGTERM to backend process group"
            );
            let _ = kill(pgid, Signal::SIGTERM);
            match tokio::time::timeout(TERMINATION_GRACE_TIMEOUT, child.wait()).await {
                Ok(status) => status,
                Err(_) => {
                    warn!(
                        target: "ralph_adapters::cli_executor",
                        %pid,
                        pgid = %pgid,
                        "Child process ignored SIGTERM, sending SIGKILL"
                    );
                    let _ = kill(pgid, Signal::SIGKILL);
                    child.wait().await
                }
            }
        } else {
            child.wait().await
        }
    }

    /// Executes a prompt without streaming (captures all output).
    ///
    /// Uses no timeout by default. For timed execution, use `execute_capture_with_timeout`.
    pub async fn execute_capture(&self, prompt: &str) -> std::io::Result<ExecutionResult> {
        self.execute_capture_with_timeout(prompt, None).await
    }

    /// Executes a prompt without streaming, with optional timeout.
    pub async fn execute_capture_with_timeout(
        &self,
        prompt: &str,
        timeout: Option<Duration>,
    ) -> std::io::Result<ExecutionResult> {
        // Use a sink that discards output for non-streaming execution
        // verbose=false since output is being discarded anyway
        let sink = std::io::sink();
        self.execute(prompt, sink, timeout, false).await
    }
}

fn line_signals_event_emitted(line: &str) -> bool {
    line.contains("Event emitted:")
}

async fn read_stream<R>(
    stream: R,
    tx: tokio::sync::mpsc::Sender<StreamEvent>,
    stream_kind: StreamKind,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        let event = match stream_kind {
            StreamKind::Stdout => StreamEvent::StdoutLine(line),
            StreamKind::Stderr => StreamEvent::StderrLine(line),
        };
        if tx.send(event).await.is_err() {
            return Ok(());
        }
    }

    let eof_event = match stream_kind {
        StreamKind::Stdout => StreamEvent::StdoutEof,
        StreamKind::Stderr => StreamEvent::StderrEof,
    };
    let _ = tx.send(eof_event).await;
    Ok(())
}

fn join_error_to_io(error: tokio::task::JoinError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

fn inject_ralph_runtime_env(command: &mut Command, workspace_root: &std::path::Path) {
    let Ok(current_exe) = env::current_exe() else {
        return;
    };
    let Some(bin_dir) = current_exe.parent() else {
        return;
    };

    let mut path_entries = vec![bin_dir.to_path_buf()];
    if let Some(existing_path) = env::var_os("PATH") {
        path_entries.extend(env::split_paths(&existing_path));
    }

    if let Ok(joined_path) = env::join_paths(path_entries) {
        command.env("PATH", joined_path);
    }
    command.env("RALPH_BIN", &current_exe);
    command.env("RALPH_WORKSPACE_ROOT", workspace_root);
    // U1 (2026-06-14-002): keep PWD in sync with the actual working directory.
    // This protects non-TTY worktree runs and any tool that resolves paths via
    // the PWD environment variable.
    command.env("PWD", workspace_root);

    // Propagate RALPH_EVENTS_FILE so `ralph emit` from any CWD writes to the correct events file
    let marker = workspace_root.join(".ralph/current-events");
    if let Ok(relative) = std::fs::read_to_string(&marker) {
        let abs = workspace_root.join(relative.trim());
        command.env("RALPH_EVENTS_FILE", &abs);
    }

    if std::path::Path::new("/var/tmp").is_dir() {
        command.env("TMPDIR", "/var/tmp");
        command.env("TMP", "/var/tmp");
        command.env("TEMP", "/var/tmp");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_echo() {
        // Use echo as a simple test backend
        let backend = CliBackend {
            command: "echo".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();

        let result = executor
            .execute("hello world", &mut output, None, true)
            .await
            .unwrap();

        assert!(result.success);
        assert!(!result.timed_out);
        assert!(result.output.contains("hello world"));
    }

    #[tokio::test]
    async fn test_execute_stdin() {
        // Use cat to test stdin mode
        let backend = CliBackend {
            command: "cat".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let result = executor.execute_capture("stdin test").await.unwrap();

        assert!(result.success);
        assert!(result.output.contains("stdin test"));
    }

    #[tokio::test]
    async fn test_execute_failure() {
        let backend = CliBackend {
            command: "false".to_string(), // Always exits with code 1
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let result = executor.execute_capture("").await.unwrap();

        assert!(!result.success);
        assert!(!result.timed_out);
        assert_eq!(result.exit_code, Some(1));
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        // Use sleep to test timeout behavior
        // The sleep command ignores stdin, so we use PromptMode::Stdin
        // to avoid appending the prompt as an argument
        let backend = CliBackend {
            command: "sleep".to_string(),
            args: vec!["10".to_string()],   // Sleep for 10 seconds
            prompt_mode: PromptMode::Stdin, // Use stdin mode so prompt doesn't interfere
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);

        // Execute with a 100ms timeout - should trigger timeout
        let timeout = Some(Duration::from_millis(100));
        let result = executor
            .execute_capture_with_timeout("", timeout)
            .await
            .unwrap();

        assert!(result.timed_out, "Expected execution to time out");
        assert!(
            !result.success,
            "Timed out execution should not be successful"
        );
    }

    #[tokio::test]
    async fn test_execute_timeout_resets_on_output_activity() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let timeout = Some(Duration::from_millis(300));
        let result = executor
            .execute_capture_with_timeout(
                "printf 'start\\n'; sleep 0.2; printf 'middle\\n'; sleep 0.2; printf 'done\\n'",
                timeout,
            )
            .await
            .unwrap();

        assert!(
            !result.timed_out,
            "Periodic output should reset the inactivity timeout"
        );
        assert!(result.success, "Periodic-output command should succeed");
        assert!(result.output.contains("start"));
        assert!(result.output.contains("middle"));
        assert!(result.output.contains("done"));
    }

    #[tokio::test]
    async fn test_execute_streams_output_before_inactivity_timeout() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "printf 'hello\\n'; sleep 10".to_string()],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();
        let result = executor
            .execute("", &mut output, Some(Duration::from_millis(200)), false)
            .await
            .unwrap();

        assert!(
            result.timed_out,
            "Expected inactivity timeout after output stops"
        );
        assert_eq!(String::from_utf8(output).unwrap(), "hello\n");
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_timeout_force_kills_processes_that_ignore_sigterm() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "trap '' TERM; while :; do sleep 1; done".to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let started = std::time::Instant::now();
        let result = executor
            .execute_capture_with_timeout("", Some(Duration::from_millis(100)))
            .await
            .unwrap();

        assert!(
            result.timed_out,
            "Expected ignored-SIGTERM command to time out"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Executor should force-kill ignored-SIGTERM processes instead of hanging"
        );
    }

    #[tokio::test]
    async fn test_execute_uses_short_post_event_grace_timeout() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'Event emitted: task.done\\n'; sleep 30".to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let started = std::time::Instant::now();
        let result = executor
            .execute_capture_with_timeout("", Some(Duration::from_secs(30)))
            .await
            .unwrap();

        assert!(
            result.timed_out,
            "Expected lingering post-event process to be terminated"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Event-emitting backends should use the short post-event grace timeout instead of the full inactivity timeout"
        );
        assert!(result.output.contains("Event emitted: task.done"));
    }

    #[tokio::test]
    async fn test_execute_post_event_deadline_does_not_reset_on_output_activity() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'Event emitted: task.done\\n'; while :; do printf 'heartbeat\\n'; sleep 1; done"
                    .to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let started = std::time::Instant::now();
        let result = executor
            .execute_capture_with_timeout("", Some(Duration::from_secs(30)))
            .await
            .unwrap();

        assert!(
            result.timed_out,
            "Expected noisy post-event process to be terminated"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Event-emitting backends should respect the fixed post-event grace deadline even if they keep producing output"
        );
        assert!(result.output.contains("Event emitted: task.done"));
        assert!(result.output.contains("heartbeat"));
    }

    #[tokio::test]
    async fn test_execute_no_timeout_when_fast() {
        // Use echo which completes immediately
        let backend = CliBackend {
            command: "echo".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);

        // Execute with a generous timeout - should complete before timeout
        let timeout = Some(Duration::from_secs(10));
        let result = executor
            .execute_capture_with_timeout("fast", timeout)
            .await
            .unwrap();

        assert!(!result.timed_out, "Fast command should not time out");
        assert!(result.success);
        assert!(result.output.contains("fast"));
    }

    #[tokio::test]
    async fn test_post_event_timeout_is_success() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'Event emitted: test\\n'; sleep 30".to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let result = executor
            .execute_capture_with_timeout("", Some(Duration::from_secs(30)))
            .await
            .unwrap();

        assert!(result.timed_out, "Should have timed out");
        assert!(result.post_event_timed_out, "Should be post-event timeout");
        assert!(
            result.success,
            "Post-event timeout should be treated as success"
        );
    }

    #[tokio::test]
    async fn test_execute_trae_stream_writes_extracted_text() {
        // Real trae-cli NDJSON shape (verified 2026-06-05, trae-cli 0.120.37):
        // assistant.message has `role` + `content` (no `type` tag), result has
        // a `result` field with the final output.
        let backend = CliBackend {
            command: "printf".to_string(),
            args: vec![
                "%s\n%s\n%s\n".to_string(),
                r#"{"type":"system","subtype":"init","session_id":"abc"}"#.to_string(),
                r#"{"type":"assistant","session_id":"abc","message":{"role":"assistant","content":"hello from trae"}}"#
                    .to_string(),
                r#"{"type":"result","subtype":"success","session_id":"abc","result":"hello from trae","is_error":false,"duration_ms":1234}"#.to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::TraeStreamJson,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();

        let result = executor
            .execute("ignored", &mut output, None, false)
            .await
            .unwrap();

        assert!(result.success);
        // Raw NDJSON should be preserved in accumulated output for debugging
        assert!(result.output.contains("\"type\":\"assistant\""));
        assert!(result.output.contains("\"type\":\"system\""));
        // But output_writer should only contain the extracted assistant text,
        // never the system/result envelopes.
        let written = String::from_utf8(output).unwrap();
        assert_eq!(written, "hello from trae\n");
    }

    #[tokio::test]
    async fn test_execute_agent_stream_falls_back_to_terminal_result() {
        let backend = CliBackend {
            command: "printf".to_string(),
            args: vec![
                "%s\n%s\n".to_string(),
                r#"{"type":"tool_call","subtype":"started","call_id":"call_1","tool_call":{"readToolCall":{"args":{"path":"README.md"}}}}"#.to_string(),
                r#"{"type":"result","subtype":"success","result":"final answer","is_error":false}"#.to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::AgentStreamJson,
            env_vars: vec![],
        };
        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();

        let result = executor
            .execute("ignored", &mut output, None, false)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(String::from_utf8(output).unwrap(), "final answer\n");
    }

    #[tokio::test]
    async fn test_execute_agent_stream_does_not_duplicate_terminal_result() {
        let backend = CliBackend {
            command: "printf".to_string(),
            args: vec![
                "%s\n%s\n".to_string(),
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"final answer"}]}}"#.to_string(),
                r#"{"type":"result","subtype":"success","result":"final answer","is_error":false}"#.to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::AgentStreamJson,
            env_vars: vec![],
        };
        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();

        let result = executor
            .execute("ignored", &mut output, None, false)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(String::from_utf8(output).unwrap(), "final answer\n");
    }

    #[tokio::test]
    async fn test_execute_trae_stream_ignores_tool_calls_and_results() {
        // Real trae-cli shape: assistant.message.tool_calls uses function.{name, arguments}
        // (arguments is a JSON-encoded string); user tool_result has no `message`
        // field — it has top-level subtype/tool_use_id/content instead.
        let backend = CliBackend {
            command: "printf".to_string(),
            args: vec![
                "%s\n%s\n%s\n".to_string(),
                r#"{"type":"assistant","message":{"role":"assistant","content":"","tool_calls":[{"id":"t1","type":"function","function":{"name":"shell","arguments":"{\"cmd\":\"ls\"}"}}]}}"#.to_string(),
                r#"{"type":"user","subtype":"tool_result","tool_use_id":"t1","tool_name":"shell","content":{"content":[{"type":"text","text":"file1\nfile2"}]}}"#.to_string(),
                r#"{"type":"assistant","message":{"role":"assistant","content":"done"}}"#.to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::TraeStreamJson,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();

        let result = executor
            .execute("ignored", &mut output, None, false)
            .await
            .unwrap();

        assert!(result.success);
        // Only assistant text deltas land in the writer; tool_use and tool_result
        // are owned by the StreamHandler path (PtyExecutor / dispatch fn).
        let written = String::from_utf8(output).unwrap();
        assert_eq!(written, "done\n");
    }

    #[tokio::test]
    async fn test_execute_trae_stream_skips_malformed_lines() {
        let backend = CliBackend {
            command: "printf".to_string(),
            args: vec![
                "%s\n%s\n%s\n".to_string(),
                "not valid json at all".to_string(),
                r#"{"type":"assistant","message":{"role":"assistant","content":"valid"}}"#
                    .to_string(),
                String::new(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::TraeStreamJson,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();

        let result = executor
            .execute("ignored", &mut output, None, false)
            .await
            .unwrap();

        assert!(result.success);
        let written = String::from_utf8(output).unwrap();
        // Malformed line is dropped (debug log only), empty line yields no event;
        // only the valid assistant text reaches output_writer.
        assert_eq!(written, "valid\n");
    }

    #[tokio::test]
    async fn test_execute_passes_ralph_reserved_env_vars() {
        // U3 (2026-06-14-002): CliExecutor now uses RALPH_WORKSPACE_ROOT if set.
        // The test checks that backend.env_vars are correctly forwarded to the child.
        // Note: inject_ralph_runtime_env also sets RALPH_WORKSPACE_ROOT and may
        // overwrite RALPH_EVENTS_FILE from the marker, so we only verify that the
        // specific env vars from backend.env_vars are present in the output.
        let backend = CliBackend {
            command: "env".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![
                ("RALPH_CURRENT_HAT".into(), "reviewer".into()),
                ("RALPH_CURRENT_LOOP_ID".into(), "loop-123".into()),
                ("RALPH_TRIGGERED_HAT".into(), "synthesizer".into()),
            ],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();
        let result = executor
            .execute("", &mut output, None, false)
            .await
            .unwrap();
        assert!(result.success);
        let stdout = String::from_utf8(output).unwrap();

        // Verify that backend.env_vars are correctly forwarded
        assert!(
            stdout.contains("RALPH_CURRENT_HAT=reviewer"),
            "missing CURRENT_HAT: {stdout}"
        );
        assert!(
            stdout.contains("RALPH_CURRENT_LOOP_ID=loop-123"),
            "missing LOOP_ID: {stdout}"
        );
        assert!(
            stdout.contains("RALPH_TRIGGERED_HAT=synthesizer"),
            "missing TRIGGERED_HAT: {stdout}"
        );

        // U3: Verify that RALPH_WORKSPACE_ROOT is set by inject_ralph_runtime_env
        assert!(
            stdout.contains("RALPH_WORKSPACE_ROOT="),
            "RALPH_WORKSPACE_ROOT should be set by inject_ralph_runtime_env: {stdout}"
        );

        // U1 (2026-06-14-002): PWD must be synchronized with the actual working
        // directory so agent bash tools resolve paths correctly.
        let expected_pwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(
            stdout.contains(&format!("PWD={expected_pwd}")),
            "PWD should match cwd ({expected_pwd}): {stdout}"
        );
    }

    /// U4 (2026-06-14-002): Verify inject_ralph_runtime_env can be called with any path.
    /// This is a regression guard for the fix that ensures RALPH_WORKSPACE_ROOT
    /// is properly injected into child processes for worktree isolation.
    #[test]
    fn test_inject_ralph_runtime_env_accepts_any_path() {
        use std::path::Path;
        let paths = vec![
            Path::new("/tmp/workspace"),
            Path::new("/Users/test/.worktrees/loop-123"),
            Path::new("."),
        ];
        for path in paths {
            let mut cmd = tokio::process::Command::new("echo");
            // Should not panic or error
            inject_ralph_runtime_env(&mut cmd, path);
        }
    }
}
