use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ralph_adapters::{AcpExecutor, CliBackend, StreamHandler};
use ralph_proto::RpcEvent;
use tracing::{info, warn};

use super::acp_mock::AcpWaveExecutionResult;
use super::io::{
    extract_readable_delta, push_to_wave_worker_buffer, read_worker_events,
    read_worker_events_with_retry, truncate_wave_worker_preview,
};

pub type WaveWorkerOutcome =
    std::result::Result<(Vec<ralph_core::Event>, Duration, bool), (String, Duration)>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaveWorkerExecutionMode {
    Pty,
    Acp,
}

pub fn wave_worker_execution_mode(
    output_format: ralph_adapters::OutputFormat,
) -> WaveWorkerExecutionMode {
    match output_format {
        ralph_adapters::OutputFormat::Acp => WaveWorkerExecutionMode::Acp,
        _ => WaveWorkerExecutionMode::Pty,
    }
}

pub struct WaveWorkerStreamHandler {
    worker_index: u32,
    rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
}

impl WaveWorkerStreamHandler {
    pub fn new(
        worker_index: u32,
        rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
        tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    ) -> Self {
        Self {
            worker_index,
            rpc_tx,
            tui_state,
        }
    }

    fn emit_delta(&self, delta: impl Into<String>) {
        let delta = delta.into();
        if delta.is_empty() {
            return;
        }

        if let Some(ref rpc_tx) = self.rpc_tx {
            let _ = rpc_tx.try_send(RpcEvent::WaveWorkerTextDelta {
                worker_index: self.worker_index,
                delta: delta.clone(),
            });
        }

        if let Some(ref state) = self.tui_state {
            let tui_lines = ralph_tui::text_to_lines(&delta);
            push_to_wave_worker_buffer(state, self.worker_index as usize, &tui_lines);
        }
    }
}

impl StreamHandler for WaveWorkerStreamHandler {
    fn on_text(&mut self, text: &str) {
        self.emit_delta(text);
    }

    fn on_tool_call(&mut self, name: &str, _id: &str, input: &serde_json::Value) {
        self.emit_delta(format!("⚙ {name}({input})\n"));
    }

    fn on_tool_result(&mut self, _id: &str, output: &str) {
        if output.is_empty() {
            return;
        }
        self.emit_delta(format!("→ {}\n", truncate_wave_worker_preview(output)));
    }

    fn on_error(&mut self, error: &str) {
        if error.is_empty() {
            return;
        }
        self.emit_delta(format!("✗ {}\n", truncate_wave_worker_preview(error)));
    }

    fn on_complete(&mut self, _result: &ralph_adapters::SessionResult) {}
}

pub async fn run_wave_worker(
    index: u32,
    worker_backend: &CliBackend,
    prompt: &str,
    worker_events_path: &Path,
    wave_timeout: Duration,
    tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
    worker_rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    worker_tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    // 2026-07-03-001 supervisor real-wiring: per-worker cwd
    // sourced from `SlotBinding.worktree_path`. `None` keeps
    // the legacy `std::env::current_dir()` behaviour.
    worker_cwd: Option<&Path>,
) -> (u32, WaveWorkerOutcome) {
    match wave_worker_execution_mode(worker_backend.output_format) {
        WaveWorkerExecutionMode::Pty => {
            run_wave_worker_pty(
                index,
                worker_backend,
                prompt,
                worker_events_path,
                wave_timeout,
                tx,
                worker_rpc_tx,
                worker_tui_state,
                worker_cwd,
            )
            .await
        }
        WaveWorkerExecutionMode::Acp => {
            run_wave_worker_acp(
                index,
                worker_backend,
                prompt,
                worker_events_path,
                wave_timeout,
                tx,
                worker_rpc_tx,
                worker_tui_state,
                worker_cwd,
            )
            .await
        }
    }
}

pub async fn execute_wave_worker_acp_prompt(
    index: u32,
    worker_backend: &CliBackend,
    prompt: &str,
    _worker_events_path: &Path,
    wave_timeout: Duration,
    worker_rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    worker_tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    // 2026-07-03-001 supervisor real-wiring: per-worker cwd.
    worker_cwd: Option<&Path>,
) -> AcpWaveExecutionResult {
    #[cfg(test)]
    {
        if let Some(mock) = {
            let mut queued = super::acp_mock::MOCK_ACP_EXECUTIONS
                .lock()
                .expect("mock ACP execution lock");
            queued.pop_front()
        } {
            mock.write_capture(worker_backend, prompt, _worker_events_path);
            mock.write_events(_worker_events_path);
            return match mock {
                super::acp_mock::MockAcpExecution::Success { success, .. } => {
                    AcpWaveExecutionResult::Completed(Ok(success))
                }
                super::acp_mock::MockAcpExecution::Error { error, .. } => {
                    AcpWaveExecutionResult::Completed(Err(error))
                }
                super::acp_mock::MockAcpExecution::Timeout { .. } => {
                    AcpWaveExecutionResult::TimedOut
                }
            };
        }
    }

    // 2026-07-03-001 supervisor real-wiring: prefer the
    // per-worker cwd (from `SlotBinding.worktree_path`) when
    // supplied; fall back to the process CWD for the legacy
    // dispatcher path.
    let workspace_root = worker_cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let executor = AcpExecutor::new(worker_backend.clone(), workspace_root);
    let mut handler = WaveWorkerStreamHandler::new(index, worker_rpc_tx, worker_tui_state);

    match tokio::time::timeout(wave_timeout, executor.execute(prompt, &mut handler)).await {
        Ok(Ok(result)) => AcpWaveExecutionResult::Completed(Ok(result.success)),
        Ok(Err(e)) => AcpWaveExecutionResult::Completed(Err(e.to_string())),
        Err(_) => AcpWaveExecutionResult::TimedOut,
    }
}

pub async fn run_wave_worker_acp(
    index: u32,
    worker_backend: &CliBackend,
    prompt: &str,
    worker_events_path: &Path,
    wave_timeout: Duration,
    tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
    worker_rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    worker_tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    worker_cwd: Option<&Path>,
) -> (u32, WaveWorkerOutcome) {
    let start = std::time::Instant::now();
    let result = execute_wave_worker_acp_prompt(
        index,
        worker_backend,
        prompt,
        worker_events_path,
        wave_timeout,
        worker_rpc_tx,
        worker_tui_state,
        worker_cwd,
    )
    .await;
    let duration = start.elapsed();
    let events = read_worker_events(worker_events_path);
    let _ = fs::remove_file(worker_events_path);

    match result {
        AcpWaveExecutionResult::Completed(Ok(success)) => {
            let _ = tx.send((index, success, duration));
            (index, Ok((events, duration, success)))
        }
        AcpWaveExecutionResult::Completed(Err(error)) => {
            let _ = tx.send((index, false, duration));
            (
                index,
                Err((format!("ACP worker failed: {error}"), duration)),
            )
        }
        AcpWaveExecutionResult::TimedOut if events.is_empty() => {
            let _ = tx.send((index, false, duration));
            (
                index,
                Err((
                    format!(
                        "Worker timed out after {}s without emitting events",
                        wave_timeout.as_secs()
                    ),
                    duration,
                )),
            )
        }
        AcpWaveExecutionResult::TimedOut => {
            let _ = tx.send((index, false, duration));
            (index, Ok((events, duration, false)))
        }
    }
}

#[cfg(test)]
fn forced_test_wave_pty_failure<'a>(worker_backend: &'a CliBackend, key: &str) -> Option<&'a str> {
    worker_backend
        .env_vars
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value.as_str()))
}

pub async fn run_wave_worker_pty(
    index: u32,
    worker_backend: &CliBackend,
    prompt: &str,
    worker_events_path: &Path,
    wave_timeout: Duration,
    tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
    worker_rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    worker_tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    worker_cwd: Option<&Path>,
) -> (u32, WaveWorkerOutcome) {
    let start = std::time::Instant::now();

    // Build and spawn process in a PTY for real-time stdout streaming.
    // Node.js structured backends buffer stdout when it's a pipe, so NDJSON
    // events only arrive when the process exits. Using a PTY forces the
    // child to see a terminal and flush after each line.
    let (cmd, args, stdin_input, _temp_file_guard) = worker_backend.build_command(prompt, false);
    let mut stdin_prompt_file = None;

    // 2026-07-03-001 supervisor real-wiring: prefer the
    // per-worker cwd (from `SlotBinding.worktree_path`) when
    // supplied; fall back to the process CWD for the legacy
    // dispatcher path.
    let cwd = worker_cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let output_format = worker_backend.output_format;

    #[cfg(test)]
    if let Some(error) =
        forced_test_wave_pty_failure(worker_backend, "RALPH_TEST_FORCE_PTY_OPEN_FAIL")
    {
        let duration = start.elapsed();
        let _ = fs::remove_file(worker_events_path);
        let _ = tx.send((index, false, duration));
        return (index, Err((format!("PTY open failed: {error}"), duration)));
    }

    // Spawn worker in a PTY so stdout is unbuffered
    let pty_system = portable_pty::native_pty_system();
    let pty_pair = match pty_system.openpty(portable_pty::PtySize {
        rows: 24,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(e) => {
            let duration = start.elapsed();
            let _ = fs::remove_file(worker_events_path);
            let _ = tx.send((index, false, duration));
            return (index, Err((format!("PTY open failed: {e}"), duration)));
        }
    };

    let (spawn_cmd, spawn_args) = if let Some(input) = stdin_input.as_ref() {
        let mut prompt_file = match tempfile::NamedTempFile::new() {
            Ok(file) => file,
            Err(e) => {
                let duration = start.elapsed();
                let _ = fs::remove_file(worker_events_path);
                let _ = tx.send((index, false, duration));
                return (
                    index,
                    Err((
                        format!("PTY stdin temp file creation failed: {e}"),
                        duration,
                    )),
                );
            }
        };
        if let Err(e) = std::io::Write::write_all(&mut prompt_file, input.as_bytes()) {
            let duration = start.elapsed();
            let _ = fs::remove_file(worker_events_path);
            let _ = tx.send((index, false, duration));
            return (
                index,
                Err((format!("PTY stdin temp file write failed: {e}"), duration)),
            );
        }

        let wrapper_args = std::iter::once("-c".to_string())
            .chain(std::iter::once(
                r#"prompt_file="$1"; shift; exec "$@" < "$prompt_file""#.to_string(),
            ))
            .chain(std::iter::once("sh".to_string()))
            .chain(std::iter::once(prompt_file.path().display().to_string()))
            .chain(std::iter::once(cmd.clone()))
            .chain(args.iter().cloned())
            .collect::<Vec<_>>();
        stdin_prompt_file = Some(prompt_file);
        ("sh".to_string(), wrapper_args)
    } else {
        (cmd.clone(), args.clone())
    };

    let mut cmd_builder = portable_pty::CommandBuilder::new(&spawn_cmd);
    cmd_builder.args(&spawn_args);
    cmd_builder.cwd(&cwd);
    for (key, value) in &worker_backend.env_vars {
        cmd_builder.env(key, value);
    }
    cmd_builder.env("TERM", "dumb");
    cmd_builder.env("NO_COLOR", "1");

    let mut child = match pty_pair.slave.spawn_command(cmd_builder) {
        Ok(child) => child,
        Err(e) => {
            let duration = start.elapsed();
            let _ = fs::remove_file(worker_events_path);
            let _ = tx.send((index, false, duration));
            return (index, Err((format!("PTY spawn failed: {e}"), duration)));
        }
    };
    drop(pty_pair.slave);

    if let Some(input) = stdin_input
        && stdin_prompt_file.is_none()
        && let Ok(mut writer) = pty_pair.master.take_writer()
    {
        let _ = writer.write_all(input.as_bytes());
        let _ = writer.write_all(b"\n");
        let _ = writer.flush();
    }

    #[cfg(test)]
    if let Some(error) =
        forced_test_wave_pty_failure(worker_backend, "RALPH_TEST_FORCE_PTY_READER_FAIL")
    {
        let duration = start.elapsed();
        let _ = fs::remove_file(worker_events_path);
        let _ = tx.send((index, false, duration));
        return (
            index,
            Err((format!("PTY reader failed: {error}"), duration)),
        );
    }

    let pty_reader = match pty_pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(e) => {
            let duration = start.elapsed();
            let _ = fs::remove_file(worker_events_path);
            let _ = tx.send((index, false, duration));
            return (index, Err((format!("PTY reader failed: {e}"), duration)));
        }
    };

    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<String>(256);
    let reader_handle = std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(pty_reader);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if line_tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut timed_out = false;
    let stream_result = async {
        let mut line_count: u64 = 0;
        while let Some(line) = line_rx.recv().await {
            line_count += 1;
            if line_count == 1 {
                info!(
                    worker = index,
                    line_len = line.len(),
                    ?output_format,
                    "Wave worker: first stdout line received"
                );
            }
            if let Some(delta) = extract_readable_delta(&line, output_format) {
                if let Some(ref rpc_tx) = worker_rpc_tx {
                    let _ = rpc_tx.try_send(RpcEvent::WaveWorkerTextDelta {
                        worker_index: index,
                        delta: delta.clone(),
                    });
                }
                if let Some(ref state) = worker_tui_state {
                    let tui_lines = ralph_tui::text_to_lines(&delta);
                    push_to_wave_worker_buffer(state, index as usize, &tui_lines);
                }
            }
        }
        Ok::<_, std::io::Error>(())
    };

    match tokio::time::timeout(wave_timeout, stream_result).await {
        Ok(result) => {
            if let Err(e) = result {
                warn!(error = %e, worker = index, "Wave worker I/O error");
            }
        }
        Err(_) => {
            warn!(
                timeout_secs = wave_timeout.as_secs(),
                worker = index,
                "Wave worker timeout, killing process"
            );
            timed_out = true;
            let _ = child.kill();
        }
    }

    let (status, _) = tokio::task::spawn_blocking(move || {
        let status = child.wait();
        let _ = reader_handle.join();
        (status, ())
    })
    .await
    .unwrap_or_else(|_| (Err(std::io::Error::other("join task panicked")), ()));
    let success = status.map(|s| s.success() && !timed_out).unwrap_or(false);
    let duration = start.elapsed();

    let events = if timed_out {
        read_worker_events_with_retry(worker_events_path, Duration::from_secs(1))
    } else {
        read_worker_events(worker_events_path)
    };
    let _ = fs::remove_file(worker_events_path);

    if timed_out && events.is_empty() {
        let _ = tx.send((index, false, duration));
        (
            index,
            Err((
                format!(
                    "Worker timed out after {}s without emitting events",
                    wave_timeout.as_secs()
                ),
                duration,
            )),
        )
    } else {
        let _ = tx.send((index, success, duration));
        (index, Ok((events, duration, success)))
    }
}
