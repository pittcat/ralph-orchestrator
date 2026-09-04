use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ralph_adapters::{CliBackend, StreamHandler};
use ralph_proto::RpcEvent;
use tracing::{info, warn};

use super::dispatcher::WORKER_TIMEOUT_ERR_PREFIX;

/// Three-way kill attribution discriminator. The post-kill
/// reason builder reads this directly instead of inferring
/// `startup_kill` from `startup_grace.is_some() &&
/// final_weak_count == 0` (which used to mis-attribute HardKill
/// and the first-Signal-then-idle IdleKill).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KillReason {
    Hard,
    Idle,
    Startup,
}

/// Next sleep deadline for the dual-clock lease loop.
///
/// While `in_startup_grace` is true (configured grace + no first
/// qualifying signal yet), idle is excluded: folding a zero
/// `idle_remaining` into the min caused a busy-wait until grace
/// expired (review P1#2). After the first signal, restore
/// `min(hard, idle)`.
pub(crate) fn next_lease_sleep(
    hard_remaining: Duration,
    idle_remaining: Duration,
    grace_remaining: Duration,
    in_startup_grace: bool,
) -> Duration {
    if in_startup_grace {
        hard_remaining.min(grace_remaining)
    } else {
        hard_remaining.min(idle_remaining)
    }
}

use super::io::{
    extract_readable_delta, push_to_wave_worker_buffer, read_worker_events,
    read_worker_events_with_retry, truncate_wave_worker_preview,
};

pub type WaveWorkerOutcome =
    std::result::Result<(Vec<ralph_core::Event>, Duration, bool, Option<u32>), (String, Duration)>;

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
    // 2026-07-28-003 plan U3 (R1): optional startup grace window.
    // Mirrors the `run_wave_worker_pty` semantics — see the
    // matching parameter docs on that function.
    startup_grace: Option<Duration>,
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
                startup_grace,
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
    // 2026-07-28-003 plan U2/U3: per-worker startup grace window.
    // When `Some(n)` and the idle dual-clock lease is enabled
    // (`idle_heartbeat` is also `Some` and non-zero), the lease
    // uses `startup_grace_ms` instead of `idle_window_ms` while the
    // worker has not yet observed its first qualifying signal.
    // Cross-references: heartbeat::LeaseConfig::startup_grace_ms,
    // `Decide_lease` R2 / S1 / S2 arms.
    startup_grace: Option<Duration>,
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
    // 2026-09-01-001 plan U5 (R5 / D6): capture the worker's
    // OS-level pid at spawn time so `dispatch.rs` can record it
    // into `dispatch_records.pid`. PTY mode yields a session pid
    // (the first process in the PTY); non-PTY backends do not
    // expose one and the field stays `None`. The dispatcher's
    // record_slot_pid call accepts `None` and degrades to NULL
    // in the store (warning, not error).
    let worker_pid = child.process_id();
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
            // 2026-07-28-003 plan U2: plug the optional startup
            // grace window into the lease. Idle-enabled is a
            // precondition for grace to take effect (KTD1: when
            // idle mode is off, the field is ignored). `None`
            // here = no grace (current behaviour); `Some(0)` is
            // also `None` because `DetectedWave` already collapses
            // `Some(0)` upstream. U3 will source the value from
            // `WorkerRequest.startup_grace` (hat config).
            startup_grace_ms: startup_grace
                .filter(|d| d.as_secs() > 0)
                .map(|d| d.as_millis() as u64),
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

    // U9: counter is read by the post-kill reason string; we need it
    // outside the dual-clock branch so Err reporting can attribute
    // `idle heartbeat exceeded` vs `worker_timeout` correctly even
    // when the legacy single-clock path is taken. In the legacy path
    // the counter is always 0 (no heartbeat loop), so the discriminator
    // reduces to `kill_reason`.
    let mut final_weak_count: u32 = 0;

    // U6/U7/U8: choose the execution path.
    // Legacy path (idle disabled): single-layer tokio::time::timeout.
    // Dual-clock path (idle enabled): tokio::select! deadline-driven loop.
    let mut kill_reason: Option<KillReason> = None;
    // C4 helper: collapse the 9-place duplication of
    //   `let _ = child.kill(); kill_reason = ...; killed = true; timed_out = true;`
    // into a single closure bound at the top of the dual-clock
    // block. The closure captures `&mut kill_reason`, `&mut killed`,
    // `&mut timed_out`, `child` (reborrowed), and `index` so the
    // three select! arms can call it without copy-pasting the
    // same 4-line kill sequence per kill kind.
    let mut apply_kill = |reason: KillReason, killed: &mut bool, timed_out: &mut bool| {
        let _ = child.kill();
        kill_reason = Some(reason);
        *killed = true;
        *timed_out = true;
    };
    let timed_out = if let Some(cfg) = lease_cfg {
        // ── Dual-clock path (U6/U7/U8) ────────────────────────────────
        let mut lease_state = super::heartbeat::LeaseState::fresh(0);
        let hard_deadline = start + wave_timeout;

        // U8: events-file strong-signal ticker state.
        let mut events_file_ticker: Option<(
            PathBuf,
            Option<(u64, Option<std::time::SystemTime>)>,
        )> = events_file_path.map(|p| {
            let prev_meta = fs::metadata(&p).ok();
            (p, prev_meta.map(|m| (m.len(), m.modified().ok())))
        });
        let mut events_tick_interval = tokio::time::interval(Duration::from_millis(250));
        // Don't fire immediately on the first tick.
        events_tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut timed_out = false;
        let mut killed = false;

        // Helper to compute the next deadline (hard, idle, or events-file).
        //
        // 2026-07-25-006 U6 fix: this used to be a closure that captured
        // `&mut lease_state`, which made the closure `!Unpin` and made
        // `tokio::select!` reject `&mut hard_sleep` (PhantomPinned).
        // Extract the relevant scalars up front so the helper is a plain
        // `fn` (`Unpin`) that takes borrowed snapshots.
        //
        // 2026-07-28-003 U2 (R2 / S1 / S2): while the worker has not
        // yet observed its first qualifying signal AND startup grace
        // is configured, `hard_remaining` is paired against
        // `startup_grace_remaining` so the timer tick can fire before
        // either idle or hard cap. After `seen_first_signal` flips
        // the grace window collapses to zero and the helper naturally
        // falls back to idle-window arithmetic.
        let idle_window_ms = idle_heartbeat.unwrap().as_millis() as u64;
        let startup_grace_ms: Option<u64> = lease_cfg.as_ref().and_then(|c| c.startup_grace_ms);
        let compute_next_deadline = |lease_state: &super::heartbeat::LeaseState| -> Duration {
            let now = start.elapsed();
            let hard_remaining = hard_deadline.saturating_duration_since(start);
            let now_ms = now.as_millis() as u64;
            let in_startup_grace = matches!(
                (startup_grace_ms, lease_state.seen_first_signal),
                (Some(_), false)
            );
            let grace_remaining = match (startup_grace_ms, lease_state.seen_first_signal) {
                (Some(grace_ms), false) => {
                    Duration::from_millis(grace_ms).saturating_sub(Duration::from_millis(now_ms))
                }
                _ => Duration::MAX,
            };
            let idle_remaining = if lease_state.last_hb_ms >= now_ms {
                Duration::ZERO
            } else {
                let elapsed_since_hb = Duration::from_millis(now_ms - lease_state.last_hb_ms);
                Duration::from_millis(idle_window_ms).saturating_sub(elapsed_since_hb)
            };
            next_lease_sleep(
                hard_remaining,
                idle_remaining,
                grace_remaining,
                in_startup_grace,
            )
        };

        loop {
            let sleep_until = compute_next_deadline(&lease_state);
            // `tokio::time::Sleep` is `!Unpin`; `Pin<Box<Sleep>>` is
            // `Unpin`, which is what `tokio::select!` requires for the
            // `&mut future` shape.
            let mut hard_sleep: std::pin::Pin<Box<tokio::time::Sleep>> =
                Box::pin(tokio::time::sleep(sleep_until));

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
                            apply_kill(KillReason::Hard, &mut killed, &mut timed_out);
                        }
                        super::heartbeat::LeaseDecision::IdleKill => {
                            warn!(worker = index, idle_window_secs = idle_heartbeat.unwrap().as_secs(),
                                  weak_count = lease_state.weak_count,
                                  "Wave worker idle heartbeat exceeded, killing process");
                            apply_kill(KillReason::Idle, &mut killed, &mut timed_out);
                        }
                        super::heartbeat::LeaseDecision::StartupKill => {
                            warn!(worker = index,
                                  startup_grace_secs = startup_grace.map(|d| d.as_secs()).unwrap_or(0),
                                  "Wave worker startup grace exceeded, killing process");
                            apply_kill(KillReason::Startup, &mut killed, &mut timed_out);
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
                                    apply_kill(KillReason::Hard, &mut killed, &mut timed_out);
                                }
                                super::heartbeat::LeaseDecision::IdleKill => {
                                    warn!(worker = index, idle_window_secs = idle_heartbeat.unwrap().as_secs(),
                                          weak_count = lease_state.weak_count,
                                          "Wave worker idle heartbeat exceeded, killing process");
                                    apply_kill(KillReason::Idle, &mut killed, &mut timed_out);
                                }
                                super::heartbeat::LeaseDecision::StartupKill => {
                                    warn!(worker = index,
                                          startup_grace_secs = startup_grace.map(|d| d.as_secs()).unwrap_or(0),
                                          "Wave worker startup grace exceeded, killing process");
                                    apply_kill(KillReason::Startup, &mut killed, &mut timed_out);
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
                    let (path, prev_key) = events_file_ticker.as_ref().unwrap();
                    let current_meta = fs::metadata(path).ok();
                    let current_key = current_meta.as_ref().map(|m| (m.len(), m.modified().ok()));
                    // Borrow-checker: snapshot the prev key into locals to
                    // allow the partial compare without the previous
                    // double-borrow of `prev`. `prev_mtime` is `Option<SystemTime>`
                    // matching the inner type of `current_key` so the equality
                    // check on `Option<Option<SystemTime>>` reduces to a direct
                    // 3-state comparison.
                    let (prev_len, prev_mtime) = match prev_key {
                        Some((len, mtime)) => (Some(*len), Some(*mtime)),
                        None => (None, None),
                    };
                    let grew = match (&current_key, prev_len) {
                        (Some((cur_len, _)), Some(pl)) => cur_len != &pl,
                        (Some(_), None) => true,
                        _ => false,
                    };
                    let mtime_changed = match (&current_key, &prev_mtime) {
                        (Some((_, cur_mtime)), Some(pm)) => cur_mtime != pm,
                        (Some(_), None) => true,
                        _ => false,
                    };
                    if grew || mtime_changed {
                        // File grew or mtime changed — strong signal.
                        let now_ms = start.elapsed().as_millis() as u64;
                        let decision = lease_state.tick(super::heartbeat::HeartbeatKind::Strong, now_ms, &cfg);
                        if let Some(slot) = events_file_ticker.as_mut() {
                            slot.1 = current_key;
                        }
                        match decision {
                            super::heartbeat::LeaseDecision::HardKill => {
                                warn!(worker = index, "Wave worker hard deadline exceeded");
                                apply_kill(KillReason::Hard, &mut killed, &mut timed_out);
                            }
                            super::heartbeat::LeaseDecision::IdleKill => {
                                warn!(worker = index, idle_window_secs = idle_heartbeat.unwrap().as_secs(),
                                      weak_count = lease_state.weak_count,
                                      "Wave worker idle heartbeat exceeded, killing process");
                                apply_kill(KillReason::Idle, &mut killed, &mut timed_out);
                            }
                            super::heartbeat::LeaseDecision::StartupKill => {
                                warn!(worker = index,
                                      startup_grace_secs = startup_grace.map(|d| d.as_secs()).unwrap_or(0),
                                      "Wave worker startup grace exceeded, killing process");
                                apply_kill(KillReason::Startup, &mut killed, &mut timed_out);
                            }
                            super::heartbeat::LeaseDecision::Continue => {}
                        }
                        if killed { break; }
                    }
                }
            }
        }

        // Carry the last observed weak_count out to the post-kill
        // reason builder so the U9 attribution keeps working.
        final_weak_count = lease_state.weak_count;

        timed_out
    } else {
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
                kill_reason = Some(KillReason::Hard);
                true
            }
        }
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
    // 2026-09-01-001 plan U1 (R1 / S1.1 / S1.3):
    // persist-before-delete. The dispatcher now owns the
    // channel-file lifecycle from worker exit onward. The worker
    // reads the events here and returns them in `WaveWorkerOutcome`;
    // the dispatcher will (a) record them to the supervisor store
    // (record_slot_event_payloads) and only then (b) delete the
    // channel file. Removing the file here would defeat recovery
    // (U2): if the loop dies before the dispatcher's store write,
    // the events are gone for good.
    //
    // PTY-open-failure paths above still call `fs::remove_file`
    // because the channel never received any events in those
    // branches — there is nothing to recover from.

    // U6/U9 kill reason strings — distinguished for U9归因.
    // Hard kill uses WORKER_TIMEOUT_ERR_PREFIX so the dispatcher
    // `reason.starts_with(WORKER_TIMEOUT_ERR_PREFIX)` still matches.
    // Idle kill uses a different prefix so U9 can extend归因.
    //
    // 2026-07-28-003 U2 (KTD3): startup-kill uses the same
    // WORKER_TIMEOUT_ERR_PREFIX as idle-kill so the dispatcher
    // classifier (which only checks the prefix) routes it into
    // the `worker_timeout` family for free. The body carries the
    // distinct `startup_kill` tag so operators can tell apart
    // cold-start misses from runtime idle hangs (S2 / R6).
    if timed_out && events.is_empty() {
        // Read the actual kill reason recorded by the dual-clock
        // / legacy path. We MUST NOT infer `startup_kill` from
        // post-hoc state — the dual-clock arm could have fired
        // HardKill or a post-first-signal IdleKill with the same
        // `startup_grace.is_some() && final_weak_count == 0`
        // surface. The dual-clock path already sets `kill_reason`
        // to the right variant.
        let reason = match kill_reason {
            Some(KillReason::Startup) => format!(
                "{WORKER_TIMEOUT_ERR_PREFIX} {}s of startup grace (worker_timeout/startup_kill, no first signal)",
                startup_grace.unwrap_or_default().as_secs()
            ),
            Some(KillReason::Idle) if final_weak_count > 0 => format!(
                "{WORKER_TIMEOUT_ERR_PREFIX} {}s of idle heartbeat (worker_timeout/idle_kill, weak_count={})",
                idle_heartbeat.unwrap().as_secs(),
                final_weak_count
            ),
            Some(KillReason::Idle) => format!(
                "{WORKER_TIMEOUT_ERR_PREFIX} {}s of idle heartbeat (worker_timeout/idle_kill, weak_count=0 (no signals))",
                idle_heartbeat.unwrap().as_secs()
            ),
            Some(KillReason::Hard) | None => format!(
                "{WORKER_TIMEOUT_ERR_PREFIX} {}s without emitting events",
                wave_timeout.as_secs()
            ),
        };
        let _ = tx.send((index, false, duration));
        (index, Err((reason, duration)))
    } else {
        let _ = tx.send((index, success, duration));
        (index, Ok((events, duration, success, worker_pid)))
    }
}

#[cfg(test)]
mod tests {
    use super::next_lease_sleep;
    use std::time::Duration;

    #[test]
    fn grace_phase_excludes_zero_idle_from_deadline() {
        // Simulate: idle already expired (ZERO), grace still has 180s,
        // hard has 1800s. Pre-fix this returned ZERO → busy-wait.
        let sleep = next_lease_sleep(
            Duration::from_secs(1800),
            Duration::ZERO,
            Duration::from_secs(180),
            true,
        );
        assert_eq!(
            sleep,
            Duration::from_secs(180),
            "grace phase must sleep until grace/hard, never idle=0"
        );
    }

    #[test]
    fn post_signal_uses_idle_deadline() {
        let sleep = next_lease_sleep(
            Duration::from_secs(1800),
            Duration::from_secs(30),
            Duration::from_secs(180), // ignored once grace ended
            false,
        );
        assert_eq!(sleep, Duration::from_secs(30));
    }

    #[test]
    fn grace_phase_respects_hard_cap() {
        let sleep = next_lease_sleep(
            Duration::from_secs(10),
            Duration::ZERO,
            Duration::from_secs(300),
            true,
        );
        assert_eq!(sleep, Duration::from_secs(10));
    }
}
