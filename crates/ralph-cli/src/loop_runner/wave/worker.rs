use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ralph_adapters::{CliBackend, StreamHandler};
use ralph_proto::RpcEvent;

use super::dispatcher::WORKER_TIMEOUT_ERR_PREFIX;
use super::io::{
    extract_readable_delta, push_to_wave_worker_buffer, read_worker_events,
    read_worker_events_with_retry,
};
use crate::loop_runner::runtime_job::pty_kernel::{
    PtyKillReason, PtyLeaseMode, PtySpawnSpec, drive_pty_lease_loop, finish_pty_job, spawn_pty_job,
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

    // 2026-09-03-0959 plan Step 5a (D7): the PTY spawn, dual-clock
    // lease loop and wait/join now live in the generic
    // `runtime_job::pty_kernel`. This function keeps every
    // wave-specific concern: `CliBackend` command construction, the
    // legacy full-env injection, the readable-delta RPC/TUI sink,
    // the events-file payload readback, and the
    // `WORKER_TIMEOUT_ERR_PREFIX` reason strings. Observable
    // behaviour is unchanged by the extraction.

    // Build and spawn process in a PTY for real-time stdout streaming.
    // Node.js structured backends buffer stdout when it's a pipe, so NDJSON
    // events only arrive when the process exits. Using a PTY forces the
    // child to see a terminal and flush after each line.
    let (cmd, args, stdin_input, _temp_file_guard) = worker_backend.build_command(prompt, false);

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

    let spec = PtySpawnSpec {
        cmd,
        args,
        stdin_input,
        cwd,
        env: worker_backend.env_vars.clone(),
        clear_env: false,
    };
    let mut handle = match spawn_pty_job(spec) {
        Ok(handle) => handle,
        Err(e) => {
            let duration = start.elapsed();
            let _ = fs::remove_file(worker_events_path);
            let _ = tx.send((index, false, duration));
            return (index, Err((e.to_string(), duration)));
        }
    };
    // 2026-09-01-001 plan U5 (R5 / D6): capture the worker's
    // OS-level pid at spawn time so `dispatch.rs` can record it
    // into `dispatch_records.pid`. PTY mode yields a session pid
    // (the first process in the PTY); non-PTY backends do not
    // expose one and the field stays `None`. The dispatcher's
    // record_slot_pid call accepts `None` and degrades to NULL
    // in the store (warning, not error).
    let worker_pid = handle.pid();

    // U6: resolve idle configuration into LeaseConfig.
    // `idle_heartbeat == None` means legacy single-clock behaviour.
    // `Some(0s)` is also disabled (per DetectedWave::idle_heartbeat_secs).
    let idle_enabled = idle_heartbeat.map(|d| d.as_secs() > 0).unwrap_or(false);
    let lease_mode = if idle_enabled {
        let cfg = super::heartbeat::LeaseConfig {
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
        };
        PtyLeaseMode::DualClock {
            cfg,
            idle_window: idle_heartbeat.unwrap(),
            startup_grace,
            // U8: events-file path for the strong-signal ticker.
            // We use the same `RALPH_EVENTS_FILE` env var value
            // that the dispatcher injected into
            // `worker_backend.env_vars`. If the env var is absent
            // the ticker is a no-op (file-not-found → no strong
            // signal, not an error).
            events_file: worker_backend
                .env_vars
                .iter()
                .find(|(name, _)| name == "RALPH_EVENTS_FILE")
                .and_then(|(_, value)| {
                    if value.is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(value))
                    }
                }),
        }
    } else {
        PtyLeaseMode::Legacy {
            hard_cap: wave_timeout,
        }
    };

    // U7: readable-delta sink shared by both lease paths — pushes
    // extracted text to RPC and the TUI worker buffer.
    let mut on_line = |line: &str| {
        if let Some(delta) = extract_readable_delta(line, output_format) {
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
    };

    let lease_outcome = drive_pty_lease_loop(
        &mut handle,
        &lease_mode,
        output_format,
        index,
        &mut on_line,
        start,
    )
    .await;
    let timed_out = lease_outcome.timed_out;

    let status = finish_pty_job(handle).await;
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
        // `startup_grace.is_some() && weak_count == 0`
        // surface. The lease loop already reported the right
        // variant via `PtyLeaseOutcome::kill_reason`.
        let reason = match lease_outcome.kill_reason {
            Some(PtyKillReason::Startup) => format!(
                "{WORKER_TIMEOUT_ERR_PREFIX} {}s of startup grace (worker_timeout/startup_kill, no first signal)",
                startup_grace.unwrap_or_default().as_secs()
            ),
            Some(PtyKillReason::Idle) if lease_outcome.weak_count > 0 => format!(
                "{WORKER_TIMEOUT_ERR_PREFIX} {}s of idle heartbeat (worker_timeout/idle_kill, weak_count={})",
                idle_heartbeat.unwrap().as_secs(),
                lease_outcome.weak_count
            ),
            Some(PtyKillReason::Idle) => format!(
                "{WORKER_TIMEOUT_ERR_PREFIX} {}s of idle heartbeat (worker_timeout/idle_kill, weak_count=0 (no signals))",
                idle_heartbeat.unwrap().as_secs()
            ),
            Some(PtyKillReason::Hard) | None => format!(
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
