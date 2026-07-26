use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ralph_adapters::{CliBackend, StreamHandler};
use ralph_proto::RpcEvent;
use tracing::{info, warn};

use super::dispatcher::WORKER_TIMEOUT_ERR_PREFIX;
use super::io::{
    extract_readable_delta, push_to_wave_worker_buffer, read_worker_events,
    read_worker_events_with_retry, truncate_wave_worker_preview,
};

pub type WaveWorkerOutcome =
    std::result::Result<(Vec<ralph_core::Event>, Duration, bool), (String, Duration)>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaveWorkerExecutionMode {
    Pty,
}

pub fn wave_worker_execution_mode(
    output_format: ralph_adapters::OutputFormat,
) -> WaveWorkerExecutionMode {
    let _ = output_format;
    WaveWorkerExecutionMode::Pty
}

pub async fn run_wave_worker(
    index: u32,
    worker_backend: &CliBackend,
    prompt: &str,
    worker_events_path: &Path,
    wave_timeout: Duration,
    idle_heartbeat: Option<Duration>,
    idle_weak_signal_cap: u32,
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
                idle_heartbeat,
                idle_weak_signal_cap,
                tx,
                worker_rpc_tx,
                worker_tui_state,
                worker_cwd,
            )
            .await
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
    idle_heartbeat: Option<Duration>,
    idle_weak_signal_cap: u32,
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

    // U6: resolve idle configuration into LeaseConfig.
    // `idle_heartbeat == None` means legacy single-clock behaviour.
    // `Some(0s)` is also disabled (per DetectedWave::idle_heartbeat_secs).
    let idle_enabled = idle_heartbeat.map(|d| d.as_secs() > 0).unwrap_or(false);
    let lease_cfg = if idle_enabled {
        Some(super::heartbeat::LeaseConfig {
            hard_cap_ms: wave_timeout.as_millis() as u64,
            idle_window_ms: Some(idle_heartbeat.unwrap().as_millis() as u64),
            weak_cap: idle_weak_signal_cap,
        })
    } else {
        None
    };

    // U6: compute the events-file path for the U8 strong-signal ticker.
    // We use the same `RALPH_EVENTS_FILE` env var value that the dispatcher
    // injected into `worker_backend.env_vars`. If the env var is absent the
    // ticker is a no-op (file-not-found → no strong signal, not an error).
    let events_file_path: Option<PathBuf> = worker_backend
        .env_vars
        .iter()
        .find(|(name, _)| name == "RALPH_EVENTS_FILE")
        .and_then(|(_, value)| {
            if value.is_empty() {
                None
            } else {
                Some(PathBuf::from(value))
            }
        });

    // U6/U7/U8: choose the execution path.
    // Legacy path (idle disabled): single-layer tokio::time::timeout.
    // Dual-clock path (idle enabled): tokio::select! deadline-driven loop.
    let timed_out = if lease_cfg.is_none() {
        // ── Legacy single-clock path ──────────────────────────────────
        // This must be bit-for-bit identical to the pre-U6 behaviour so
        // that `partial_timeout_events_visible` and the S2 regression pin
        // stay green.
        let mut line_count: u64 = 0;
        let stream_result = async {
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
                false
            }
            Err(_) => {
                warn!(
                    timeout_secs = wave_timeout.as_secs(),
                    worker = index,
                    "Wave worker timeout, killing process"
                );
                let _ = child.kill();
                true
            }
        }
    } else {
        // ── Dual-clock path (U6/U7/U8) ────────────────────────────────
        let cfg = lease_cfg.unwrap();
        let mut lease_state = super::heartbeat::LeaseState::fresh(0);
        let hard_deadline = start + wave_timeout;

        // U8: events-file strong-signal ticker state.
        let mut events_file_ticker = events_file_path.map(|p| {
            let prev_meta = fs::metadata(&p).ok();
            (
                p,
                prev_meta.map(|m| (m.len(), m.modified().ok())),
            )
        });
        let mut events_tick_interval = tokio::time::interval(Duration::from_millis(250));
        // Don't fire immediately on the first tick.
        events_tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut timed_out = false;
        let mut killed = false;

        // Helper to compute the next deadline (hard, idle, or events-file).
        let next_deadline = || {
            let now = start.elapsed();
            let hard_remaining = hard_deadline.saturating_duration_since(start);
            if !idle_enabled {
                return hard_remaining;
            }
            let idle_remaining = if lease_state.last_hb_ms >= now.as_millis() as u64 {
                Duration::ZERO
            } else {
                let idle_window =
                    Duration::from_millis(idle_heartbeat.unwrap().as_millis() as u64);
                let elapsed_since_hb =
                    Duration::from_millis(now.as_millis() as u64 - lease_state.last_hb_ms);
                idle_window.saturating_sub(elapsed_since_hb)
            };
            hard_remaining.min(idle_remaining)
        };

        loop {
            let sleep_until = next_deadline();
            let mut hard_sleep = tokio::time::sleep(sleep_until);

            tokio::select! {
                biased;

                // Hard timer tick: this arm fires when the hard deadline OR
                // idle window has elapsed (whichever comes first).
                _ = &mut hard_sleep => {
                    let now_ms = start.elapsed().as_millis() as u64;
                    let decision = lease_state.tick(super::heartbeat::HeartbeatKind::None, now_ms, &cfg);
                    match decision {
                        super::heartbeat::LeaseDecision::HardKill => {
                            warn!(worker = index, "Wave worker hard deadline exceeded");
                            let _ = child.kill();
                            killed = true;
                            timed_out = true;
                        }
                        super::heartbeat::LeaseDecision::IdleKill => {
                            warn!(worker = index, idle_window_secs = idle_heartbeat.unwrap().as_secs(),
                                  weak_count = lease_state.weak_count,
                                  "Wave worker idle heartbeat exceeded, killing process");
                            let _ = child.kill();
                            killed = true;
                            timed_out = true;
                        }
                        super::heartbeat::LeaseDecision::Continue => {
                            // The hard sleep fired but neither kill condition was met.
                            // This means the hard deadline hasn't been reached yet and the
                            // idle window hasn't expired. Loop back to re-compute deadline.
                        }
                    }
                    if killed { break; }
                }

                line = line_rx.recv() => {
                    match line {
                        Some(line) => {
                            let now_ms = start.elapsed().as_millis() as u64;
                            let kind = super::heartbeat::classify_heartbeat_line(&line, output_format);

                            // U7: push readable delta to RPC/TUI (same as legacy path).
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

                            let decision = lease_state.tick(kind, now_ms, &cfg);
                            match decision {
                                super::heartbeat::LeaseDecision::HardKill => {
                                    warn!(worker = index, "Wave worker hard deadline exceeded");
                                    let _ = child.kill();
                                    killed = true;
                                    timed_out = true;
                                }
                                super::heartbeat::LeaseDecision::IdleKill => {
                                    warn!(worker = index, idle_window_secs = idle_heartbeat.unwrap().as_secs(),
                                          weak_count = lease_state.weak_count,
                                          "Wave worker idle heartbeat exceeded, killing process");
                                    let _ = child.kill();
                                    killed = true;
                                    timed_out = true;
                                }
                                super::heartbeat::LeaseDecision::Continue => {
                                    // Lease refreshed; loop continues.
                                }
                            }
                            if killed { break; }
                        }
                        None => {
                            // Channel closed — worker exited normally.
                            break;
                        }
                    }
                }

                // U8: events-file growth as strong signal.
                _ = events_tick_interval.tick(), if events_file_ticker.is_some() => {
                    let (ref path, ref mut prev) = events_file_ticker.as_mut().unwrap();
                    let current_meta = fs::metadata(path).ok();
                    let current_key = current_meta.as_ref().map(|m| (m.len(), m.modified().ok()));
                    if current_key != prev.as_ref().map(|m| (m.len(), m.modified().ok())) {
                        // File grew or mtime changed — strong signal.
                        let now_ms = start.elapsed().as_millis() as u64;
                        let decision = lease_state.tick(super::heartbeat::HeartbeatKind::Strong, now_ms, &cfg);
                        *prev = current_key;
                        match decision {
                            super::heartbeat::LeaseDecision::HardKill => {
                                let _ = child.kill();
                                killed = true;
                                timed_out = true;
                            }
                            super::heartbeat::LeaseDecision::IdleKill => {
                                let _ = child.kill();
                                killed = true;
                                timed_out = true;
                            }
                            super::heartbeat::LeaseDecision::Continue => {}
                        }
                        if killed { break; }
                    }
                }
            }
        }

        timed_out
    };

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

    // U6 kill reason strings — distinguished for U9歸因.
    // Hard kill uses WORKER_TIMEOUT_ERR_PREFIX so the dispatcher
    // `reason.starts_with(WORKER_TIMEOUT_ERR_PREFIX)` still matches.
    // Idle kill uses a different prefix so U9 can extend归因.
    if timed_out && events.is_empty() {
        let reason = if idle_enabled && lease_state.weak_count > 0 {
            // Idle kill path (not a hard timeout).
            format!(
                "idle heartbeat exceeded: {}s since last activity, weak_count={}",
                idle_heartbeat.unwrap().as_secs(),
                lease_state.weak_count
            )
        } else if idle_enabled {
            format!(
                "idle heartbeat exceeded: {}s since last activity",
                idle_heartbeat.unwrap().as_secs()
            )
        } else {
            format!(
                "{WORKER_TIMEOUT_ERR_PREFIX} {}s without emitting events",
                wave_timeout.as_secs()
            )
        };
        let _ = tx.send((index, false, duration));
        (index, Err((reason, duration)))
    } else {
        let _ = tx.send((index, success, duration));
        (index, Ok((events, duration, success)))
    }
}
