//! 2026-09-03-0959 plan Step 5a (D7): generic async PTY job kernel
//! extracted from `wave::worker::run_wave_worker_pty`.
//!
//! This module owns the *generic* half of the wave worker: PTY
//! spawn, stdin temp-file indirection, the reader thread, the
//! dual-clock lease loop (hard cap / idle window / weak-signal cap /
//! startup grace / events-file growth) and the final wait+join.
//! Wave-specific concerns — `CliBackend` command construction, env
//! injection, readable-delta sinks, events-file *payload* reading
//! and `WORKER_TIMEOUT_ERR_PREFIX` reason strings — stay with the
//! caller (`wave::worker`).
//!
//! The caller supplies:
//!   - the fully-resolved command line + cwd + env map
//!     ([`PtySpawnSpec`]); the wave adapter passes the legacy full
//!     env, the DAG side will pass an allowlist-filtered map.
//!   - the lease mode ([`PtyLeaseMode`]): legacy single-clock or
//!     dual-clock with a [`LeaseConfig`].
//!   - an `on_line` sink invoked for every stdout line.
//!
//! The kernel returns *typed* outcomes ([`PtyKernelError`] for
//! spawn-time failures, [`PtyLeaseOutcome`] for the lease loop);
//! mapping them onto channel-file cleanup / dispatcher reason
//! strings is the caller's job.
//!
//! Extraction rule: every observable behaviour of the wave worker
//! (command line, env, lease arithmetic, log lines, error message
//! prefixes) is preserved verbatim; this is a pure refactor, no
//! semantic change.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ralph_adapters::OutputFormat;
use tracing::{info, warn};

use crate::loop_runner::wave::heartbeat::{
    HeartbeatKind, LeaseConfig, LeaseDecision, LeaseState, classify_heartbeat_line,
};

/// Fully-resolved spawn inputs. The kernel does NOT build the
/// command line itself — callers resolve backend / prompt / env
/// policy up front so the same kernel can serve the legacy wave
/// worker (full env passthrough) and the DAG scheduler (allowlist
/// filtered env) without the kernel knowing the difference.
#[derive(Debug)]
pub struct PtySpawnSpec {
    /// Executable (already resolved by the caller).
    pub cmd: String,
    /// Argument vector.
    pub args: Vec<String>,
    /// When `Some`, the input is written to a temp file and the
    /// child is spawned through an `sh -c` wrapper that redirects
    /// the file into the real command's stdin. The temp file is
    /// owned by the returned handle and deleted on drop.
    pub stdin_input: Option<String>,
    /// Working directory for the child.
    pub cwd: PathBuf,
    /// Extra env entries applied on top of the inherited
    /// environment. The kernel always appends `TERM=dumb` and
    /// `NO_COLOR=1` after these, exactly like the legacy wave
    /// worker did.
    pub env: Vec<(String, String)>,
}

/// Spawn-time failure modes. `Display` strings are byte-identical
/// to the legacy wave worker error prefixes so dispatcher-side
/// classifiers and tests keep matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyKernelError {
    /// `openpty` failed.
    PtyOpen(String),
    /// Creating the stdin prompt temp file failed.
    StdinTempFileCreate(String),
    /// Writing the stdin prompt temp file failed.
    StdinTempFileWrite(String),
    /// Spawning the child into the PTY slave failed.
    Spawn(String),
    /// Cloning the PTY master reader failed.
    Reader(String),
}

impl std::fmt::Display for PtyKernelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PtyKernelError::PtyOpen(e) => write!(f, "PTY open failed: {e}"),
            PtyKernelError::StdinTempFileCreate(e) => {
                write!(f, "PTY stdin temp file creation failed: {e}")
            }
            PtyKernelError::StdinTempFileWrite(e) => {
                write!(f, "PTY stdin temp file write failed: {e}")
            }
            PtyKernelError::Spawn(e) => write!(f, "PTY spawn failed: {e}"),
            PtyKernelError::Reader(e) => write!(f, "PTY reader failed: {e}"),
        }
    }
}

impl std::error::Error for PtyKernelError {}

/// A spawned PTY child plus its line stream. Owned by the caller;
/// [`drive_pty_lease_loop`] borrows it mutably (it may kill the
/// child), [`finish_pty_job`] consumes it (wait + reader join).
pub struct PtyJobHandle {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    reader_handle: std::thread::JoinHandle<()>,
    line_rx: tokio::sync::mpsc::Receiver<String>,
    pid: Option<u32>,
    /// Keeps the stdin prompt temp file alive for the child's
    /// lifetime; deleted when the handle drops.
    _stdin_prompt_file: Option<tempfile::NamedTempFile>,
}

impl PtyJobHandle {
    /// OS-level pid of the PTY session leader, captured at spawn
    /// time. `None` when the platform backend does not expose one.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

/// Spawn a child process inside a fresh PTY and start the
/// line-oriented reader thread.
///
/// The PTY forces the child to see a terminal so Node.js
/// structured backends flush stdout after each line instead of
/// buffering until exit.
pub fn spawn_pty_job(spec: PtySpawnSpec) -> Result<PtyJobHandle, PtyKernelError> {
    let PtySpawnSpec {
        cmd,
        args,
        stdin_input,
        cwd,
        env,
    } = spec;

    let pty_system = portable_pty::native_pty_system();
    let pty_pair = pty_system
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| PtyKernelError::PtyOpen(e.to_string()))?;

    let mut stdin_prompt_file = None;
    let (spawn_cmd, spawn_args) = if let Some(input) = stdin_input.as_ref() {
        let mut prompt_file = tempfile::NamedTempFile::new()
            .map_err(|e| PtyKernelError::StdinTempFileCreate(e.to_string()))?;
        prompt_file
            .write_all(input.as_bytes())
            .map_err(|e| PtyKernelError::StdinTempFileWrite(e.to_string()))?;

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
        (cmd, args)
    };

    let mut cmd_builder = portable_pty::CommandBuilder::new(&spawn_cmd);
    cmd_builder.args(&spawn_args);
    cmd_builder.cwd(&cwd);
    for (key, value) in &env {
        cmd_builder.env(key, value);
    }
    cmd_builder.env("TERM", "dumb");
    cmd_builder.env("NO_COLOR", "1");

    let child = pty_pair
        .slave
        .spawn_command(cmd_builder)
        .map_err(|e| PtyKernelError::Spawn(e.to_string()))?;
    let pid = child.process_id();
    drop(pty_pair.slave);

    if let Some(input) = stdin_input
        && stdin_prompt_file.is_none()
        && let Ok(mut writer) = pty_pair.master.take_writer()
    {
        let _ = writer.write_all(input.as_bytes());
        let _ = writer.write_all(b"\n");
        let _ = writer.flush();
    }

    let pty_reader = pty_pair
        .master
        .try_clone_reader()
        .map_err(|e| PtyKernelError::Reader(e.to_string()))?;

    let (line_tx, line_rx) = tokio::sync::mpsc::channel::<String>(256);
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

    Ok(PtyJobHandle {
        child,
        reader_handle,
        line_rx,
        pid,
        _stdin_prompt_file: stdin_prompt_file,
    })
}

/// Which lease discipline the loop enforces.
///
/// `Legacy` is the pre-dual-clock behaviour: a single wall-clock
/// timeout around the stream loop, no heartbeat classification, no
/// events-file ticker. It must stay bit-for-bit identical so the
/// `partial_timeout_events_visible` regression pins stay green.
///
/// `DualClock` runs the deadline-driven `tokio::select!` lease
/// loop with hard cap / idle window / weak-signal cap / startup
/// grace, plus the optional events-file growth ticker as an
/// additional strong-signal source.
#[derive(Debug)]
pub enum PtyLeaseMode {
    /// Legacy single-clock path.
    Legacy {
        /// Wall-clock timeout around the stream loop.
        hard_cap: Duration,
    },
    /// Dual-clock lease path.
    DualClock {
        /// Lease decision configuration (milliseconds).
        cfg: LeaseConfig,
        /// Idle window as a `Duration` (for log lines).
        idle_window: Duration,
        /// Optional startup grace window (for log lines).
        startup_grace: Option<Duration>,
        /// Optional events-file path whose growth counts as a
        /// strong signal. `None` disables the ticker entirely.
        events_file: Option<PathBuf>,
    },
}

/// Three-way kill attribution discriminator, returned to the
/// caller so it can build its own post-kill reason string without
/// inferring the reason from post-hoc state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyKillReason {
    /// Hard StartToClose ceiling exceeded.
    Hard,
    /// Idle heartbeat window (or weak-signal cap) exceeded.
    Idle,
    /// Startup grace window exceeded before any first signal.
    Startup,
}

/// Outcome of one [`drive_pty_lease_loop`] run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyLeaseOutcome {
    /// `true` when the child was killed by the lease (any
    /// [`PtyKillReason`]); `false` when the stream ended on its
    /// own.
    pub timed_out: bool,
    /// Which clock fired, when `timed_out` is `true`.
    pub kill_reason: Option<PtyKillReason>,
    /// Final weak-signal counter observed by the lease. Always 0
    /// on the legacy path (no heartbeat loop runs there).
    pub weak_count: u32,
}

/// Next sleep deadline for the dual-clock lease loop.
///
/// While `in_startup_grace` is true (configured grace + no first
/// qualifying signal yet), idle is excluded: folding a zero
/// `idle_remaining` into the min caused a busy-wait until grace
/// expired (review P1#2). After the first signal, restore
/// `min(hard, idle)`.
pub fn next_lease_sleep(
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

/// Drive the lease loop over a spawned PTY child until the stream
/// ends or a kill condition fires. `start` is the caller's spawn
/// epoch: the dual-clock hard deadline and all lease timestamps
/// are measured from it. `on_line` is invoked for every stdout
/// line before the heartbeat classifier sees it.
///
/// On a kill decision the child is signalled (`child.kill()`)
/// before this function returns; the caller still MUST call
/// [`finish_pty_job`] to reap the process and join the reader
/// thread.
pub async fn drive_pty_lease_loop(
    handle: &mut PtyJobHandle,
    mode: &PtyLeaseMode,
    output_format: OutputFormat,
    worker_index: u32,
    on_line: &mut (dyn FnMut(&str) + Send),
    start: Instant,
) -> PtyLeaseOutcome {
    let child = &mut handle.child;
    let line_rx = &mut handle.line_rx;

    // C4 helper: collapse the 9-place duplication of
    //   `let _ = child.kill(); kill_reason = ...; killed = true; timed_out = true;`
    // into a single closure. The closure captures `&mut kill_reason`,
    // `child` (reborrowed), and `worker_index` so the three select!
    // arms can call it without copy-pasting the same 4-line kill
    // sequence per kill kind.
    let mut kill_reason: Option<PtyKillReason> = None;
    let mut apply_kill = |reason: PtyKillReason, killed: &mut bool, timed_out: &mut bool| {
        let _ = child.kill();
        kill_reason = Some(reason);
        *killed = true;
        *timed_out = true;
    };

    match mode {
        PtyLeaseMode::DualClock {
            cfg,
            idle_window,
            startup_grace,
            events_file,
        } => {
            // ── Dual-clock path ─────────────────────────────────────
            let mut lease_state = LeaseState::fresh(0);
            let hard_deadline = start + Duration::from_millis(cfg.hard_cap_ms);

            // Events-file strong-signal ticker state.
            let mut events_file_ticker: Option<(
                PathBuf,
                Option<(u64, Option<std::time::SystemTime>)>,
            )> = events_file.as_ref().map(|p| {
                let prev_meta = std::fs::metadata(p).ok();
                (p.clone(), prev_meta.map(|m| (m.len(), m.modified().ok())))
            });
            let mut events_tick_interval = tokio::time::interval(Duration::from_millis(250));
            // Don't fire immediately on the first tick.
            events_tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            let mut timed_out = false;
            let mut killed = false;

            // Helper to compute the next deadline (hard, idle, or
            // events-file).
            //
            // 2026-07-25-006 U6 fix: this used to be a closure that
            // captured `&mut lease_state`, which made the closure
            // `!Unpin` and made `tokio::select!` reject
            // `&mut hard_sleep` (PhantomPinned). Extract the relevant
            // scalars up front so the helper is a plain `fn`
            // (`Unpin`) that takes borrowed snapshots.
            //
            // 2026-07-28-003 U2 (R2 / S1 / S2): while the worker has
            // not yet observed its first qualifying signal AND
            // startup grace is configured, `hard_remaining` is paired
            // against `startup_grace_remaining` so the timer tick can
            // fire before either idle or hard cap. After
            // `seen_first_signal` flips the grace window collapses to
            // zero and the helper naturally falls back to idle-window
            // arithmetic.
            let idle_window_ms = idle_window.as_millis() as u64;
            let startup_grace_ms: Option<u64> = cfg.startup_grace_ms;
            let compute_next_deadline = |lease_state: &LeaseState| -> Duration {
                let now = start.elapsed();
                let hard_remaining = hard_deadline.saturating_duration_since(start);
                let now_ms = now.as_millis() as u64;
                let in_startup_grace = matches!(
                    (startup_grace_ms, lease_state.seen_first_signal),
                    (Some(_), false)
                );
                let grace_remaining = match (startup_grace_ms, lease_state.seen_first_signal) {
                    (Some(grace_ms), false) => Duration::from_millis(grace_ms)
                        .saturating_sub(Duration::from_millis(now_ms)),
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
                // `tokio::time::Sleep` is `!Unpin`; `Pin<Box<Sleep>>`
                // is `Unpin`, which is what `tokio::select!` requires
                // for the `&mut future` shape.
                let mut hard_sleep: std::pin::Pin<Box<tokio::time::Sleep>> =
                    Box::pin(tokio::time::sleep(sleep_until));

                tokio::select! {
                    biased;

                    // Hard timer tick: this arm fires when the hard deadline OR
                    // idle window has elapsed (whichever comes first).
                    _ = &mut hard_sleep => {
                        let now_ms = start.elapsed().as_millis() as u64;
                        let decision = lease_state.tick(HeartbeatKind::None, now_ms, cfg);
                        match decision {
                            LeaseDecision::HardKill => {
                                warn!(worker = worker_index, "Wave worker hard deadline exceeded");
                                apply_kill(PtyKillReason::Hard, &mut killed, &mut timed_out);
                            }
                            LeaseDecision::IdleKill => {
                                warn!(worker = worker_index, idle_window_secs = idle_window.as_secs(),
                                      weak_count = lease_state.weak_count,
                                      "Wave worker idle heartbeat exceeded, killing process");
                                apply_kill(PtyKillReason::Idle, &mut killed, &mut timed_out);
                            }
                            LeaseDecision::StartupKill => {
                                warn!(worker = worker_index,
                                      startup_grace_secs = startup_grace.map(|d| d.as_secs()).unwrap_or(0),
                                      "Wave worker startup grace exceeded, killing process");
                                apply_kill(PtyKillReason::Startup, &mut killed, &mut timed_out);
                            }
                            LeaseDecision::Continue => {
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
                                let kind = classify_heartbeat_line(&line, output_format);

                                // Caller sink (RPC/TUI readable delta) —
                                // invoked before the lease tick, same
                                // order as the legacy worker.
                                on_line(&line);

                                let decision = lease_state.tick(kind, now_ms, cfg);
                                match decision {
                                    LeaseDecision::HardKill => {
                                        warn!(worker = worker_index, "Wave worker hard deadline exceeded");
                                        apply_kill(PtyKillReason::Hard, &mut killed, &mut timed_out);
                                    }
                                    LeaseDecision::IdleKill => {
                                        warn!(worker = worker_index, idle_window_secs = idle_window.as_secs(),
                                              weak_count = lease_state.weak_count,
                                              "Wave worker idle heartbeat exceeded, killing process");
                                        apply_kill(PtyKillReason::Idle, &mut killed, &mut timed_out);
                                    }
                                    LeaseDecision::StartupKill => {
                                        warn!(worker = worker_index,
                                              startup_grace_secs = startup_grace.map(|d| d.as_secs()).unwrap_or(0),
                                              "Wave worker startup grace exceeded, killing process");
                                        apply_kill(PtyKillReason::Startup, &mut killed, &mut timed_out);
                                    }
                                    LeaseDecision::Continue => {
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

                    // Events-file growth as strong signal.
                    _ = events_tick_interval.tick(), if events_file_ticker.is_some() => {
                        let (path, prev_key) = events_file_ticker.as_ref().unwrap();
                        let current_meta = std::fs::metadata(path).ok();
                        let current_key = current_meta.as_ref().map(|m| (m.len(), m.modified().ok()));
                        // Borrow-checker: snapshot the prev key into locals
                        // to allow the partial compare without the previous
                        // double-borrow of `prev`. `prev_mtime` is
                        // `Option<SystemTime>` matching the inner type of
                        // `current_key` so the equality check on
                        // `Option<Option<SystemTime>>` reduces to a direct
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
                            let decision = lease_state.tick(HeartbeatKind::Strong, now_ms, cfg);
                            if let Some(slot) = events_file_ticker.as_mut() {
                                slot.1 = current_key;
                            }
                            match decision {
                                LeaseDecision::HardKill => {
                                    warn!(worker = worker_index, "Wave worker hard deadline exceeded");
                                    apply_kill(PtyKillReason::Hard, &mut killed, &mut timed_out);
                                }
                                LeaseDecision::IdleKill => {
                                    warn!(worker = worker_index, idle_window_secs = idle_window.as_secs(),
                                          weak_count = lease_state.weak_count,
                                          "Wave worker idle heartbeat exceeded, killing process");
                                    apply_kill(PtyKillReason::Idle, &mut killed, &mut timed_out);
                                }
                                LeaseDecision::StartupKill => {
                                    warn!(worker = worker_index,
                                          startup_grace_secs = startup_grace.map(|d| d.as_secs()).unwrap_or(0),
                                          "Wave worker startup grace exceeded, killing process");
                                    apply_kill(PtyKillReason::Startup, &mut killed, &mut timed_out);
                                }
                                LeaseDecision::Continue => {}
                            }
                            if killed { break; }
                        }
                    }
                }
            }

            PtyLeaseOutcome {
                timed_out,
                kill_reason,
                // Carry the last observed weak_count out so the
                // caller's post-kill reason attribution keeps
                // working.
                weak_count: lease_state.weak_count,
            }
        }
        PtyLeaseMode::Legacy { hard_cap } => {
            // ── Legacy single-clock path ────────────────────────────
            // This must be bit-for-bit identical to the pre-U6
            // behaviour so that `partial_timeout_events_visible` and
            // the S2 regression pin stay green.
            let mut line_count: u64 = 0;
            let stream_result = async {
                while let Some(line) = line_rx.recv().await {
                    line_count += 1;
                    if line_count == 1 {
                        info!(
                            worker = worker_index,
                            line_len = line.len(),
                            ?output_format,
                            "Wave worker: first stdout line received"
                        );
                    }
                    on_line(&line);
                }
                Ok::<_, std::io::Error>(())
            };

            match tokio::time::timeout(*hard_cap, stream_result).await {
                Ok(result) => {
                    if let Err(e) = result {
                        warn!(error = %e, worker = worker_index, "Wave worker I/O error");
                    }
                    PtyLeaseOutcome {
                        timed_out: false,
                        kill_reason: None,
                        weak_count: 0,
                    }
                }
                Err(_) => {
                    warn!(
                        timeout_secs = hard_cap.as_secs(),
                        worker = worker_index,
                        "Wave worker timeout, killing process"
                    );
                    let mut killed = false;
                    let mut timed_out = false;
                    apply_kill(PtyKillReason::Hard, &mut killed, &mut timed_out);
                    PtyLeaseOutcome {
                        timed_out,
                        kill_reason,
                        weak_count: 0,
                    }
                }
            }
        }
    }
}

/// Reap the child and join the reader thread. Consumes the handle
/// (and with it the stdin temp-file guard, which deletes the temp
/// file). Runs on the blocking thread pool so a wedged child does
/// not stall the async executor.
pub async fn finish_pty_job(handle: PtyJobHandle) -> std::io::Result<portable_pty::ExitStatus> {
    let PtyJobHandle {
        mut child,
        reader_handle,
        ..
    } = handle;
    tokio::task::spawn_blocking(move || {
        let status = child.wait();
        let _ = reader_handle.join();
        status
    })
    .await
    .unwrap_or_else(|_| Err(std::io::Error::other("join task panicked")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── `next_lease_sleep` deadline arithmetic (moved from
    // `wave::worker` during the Step 5a extraction). ─────────────

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

    // ── Kernel-level behavioural tests: real (tiny) child
    // processes, no LLM calls. ───────────────────────────────────

    fn sh_spec(script: &str) -> PtySpawnSpec {
        PtySpawnSpec {
            cmd: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            stdin_input: None,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env: Vec::new(),
        }
    }

    fn dual_clock_mode(hard_cap_ms: u64, idle_window_ms: u64) -> PtyLeaseMode {
        PtyLeaseMode::DualClock {
            cfg: LeaseConfig {
                hard_cap_ms,
                idle_window_ms: Some(idle_window_ms),
                weak_cap: 4,
                startup_grace_ms: None,
            },
            idle_window: Duration::from_millis(idle_window_ms),
            startup_grace: None,
            events_file: None,
        }
    }

    /// Spawn failure surfaces the typed `Spawn` error.
    #[cfg(unix)]
    #[test]
    fn spawn_failure_returns_typed_error() {
        let spec = PtySpawnSpec {
            cmd: "/nonexistent/ralph-pty-kernel-test-binary".to_string(),
            args: Vec::new(),
            stdin_input: None,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env: Vec::new(),
        };
        let err = match spawn_pty_job(spec) {
            Ok(_) => panic!("missing binary must fail to spawn"),
            Err(e) => e,
        };
        assert!(matches!(err, PtyKernelError::Spawn(_)));
        assert!(err.to_string().starts_with("PTY spawn failed: "));
    }

    /// Hard cap: a silent child outliving the hard ceiling is
    /// killed and attributed to `PtyKillReason::Hard`.
    #[cfg(unix)]
    #[tokio::test]
    async fn hard_cap_timeout_kills_child_with_hard_reason() {
        let start = Instant::now();
        let mut handle =
            spawn_pty_job(sh_spec("sleep 30")).unwrap_or_else(|e| panic!("spawn sleep: {e}"));
        let pid = handle.pid();
        assert!(pid.is_some(), "PTY child must expose a pid");

        // Idle window (60 s) is far wider than the hard cap (300 ms)
        // so only the hard ceiling can fire.
        let mode = dual_clock_mode(300, 60_000);
        let mut lines: Vec<String> = Vec::new();
        let outcome = drive_pty_lease_loop(
            &mut handle,
            &mode,
            OutputFormat::Text,
            0,
            &mut |line: &str| lines.push(line.to_string()),
            start,
        )
        .await;

        assert!(outcome.timed_out, "hard cap must mark the job timed out");
        assert_eq!(outcome.kill_reason, Some(PtyKillReason::Hard));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "kill must happen promptly, got {:?}",
            start.elapsed()
        );

        let status = finish_pty_job(handle).await.expect("wait succeeds");
        assert!(
            !status.success(),
            "killed child must not report success (process cancel)"
        );
    }

    /// Idle kill: a child that produces no output past the idle
    /// window is killed and attributed to `PtyKillReason::Idle`.
    #[cfg(unix)]
    #[tokio::test]
    async fn idle_silence_kills_child_with_idle_reason() {
        let start = Instant::now();
        let mut handle =
            spawn_pty_job(sh_spec("sleep 30")).unwrap_or_else(|e| panic!("spawn sleep: {e}"));

        // Hard cap (30 s) is far wider than the idle window (300 ms)
        // so only the idle clock can fire.
        let mode = dual_clock_mode(30_000, 300);
        let mut lines: Vec<String> = Vec::new();
        let outcome = drive_pty_lease_loop(
            &mut handle,
            &mode,
            OutputFormat::Text,
            0,
            &mut |line: &str| lines.push(line.to_string()),
            start,
        )
        .await;

        assert!(
            outcome.timed_out,
            "idle silence must mark the job timed out"
        );
        assert_eq!(outcome.kill_reason, Some(PtyKillReason::Idle));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "idle kill must fire near the idle window, got {:?}",
            start.elapsed()
        );

        let status = finish_pty_job(handle).await.expect("wait succeeds");
        assert!(!status.success(), "idle-killed child must not succeed");
    }

    /// Normal exit: a child that prints two lines and exits 0
    /// yields every line through `on_line`, no timeout, and a
    /// successful exit status.
    #[cfg(unix)]
    #[tokio::test]
    async fn normal_exit_collects_all_output_lines() {
        let start = Instant::now();
        let mut handle = spawn_pty_job(sh_spec("printf 'hello\\nworld\\n'"))
            .unwrap_or_else(|e| panic!("spawn printf: {e}"));

        let mode = PtyLeaseMode::Legacy {
            hard_cap: Duration::from_secs(10),
        };
        let mut lines: Vec<String> = Vec::new();
        let outcome = drive_pty_lease_loop(
            &mut handle,
            &mode,
            OutputFormat::Text,
            0,
            &mut |line: &str| lines.push(line.to_string()),
            start,
        )
        .await;

        assert!(!outcome.timed_out, "clean exit must not time out");
        assert_eq!(outcome.kill_reason, None);
        assert_eq!(lines, vec!["hello".to_string(), "world".to_string()]);

        let status = finish_pty_job(handle).await.expect("wait succeeds");
        assert!(status.success(), "clean exit must report success");
    }
}
