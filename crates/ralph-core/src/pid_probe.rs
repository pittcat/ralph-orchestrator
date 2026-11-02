//! Cross-platform PID liveness probe.
//!
//! Unix keeps the existing `kill(pid, 0)` semantics (alive / not-found /
//! inconclusive). Windows probes via `sysinfo` and compares process start
//! time when the caller provides one, so a recycled PID does not masquerade
//! as the original process.

/// Result of probing whether a process is alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidState {
    /// The process exists (or we lack permission to signal it, which on
    /// Unix also proves existence).
    Alive,
    /// The process does not exist.
    Dead,
    /// The probe could not determine the state.
    Unknown,
}

/// Probe whether `pid` refers to a live process.
#[cfg(unix)]
pub fn probe_pid(pid: u32) -> PidState {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) | Err(Errno::EPERM) => PidState::Alive,
        Err(Errno::ESRCH) => PidState::Dead,
        Err(_) => PidState::Unknown,
    }
}

/// Probe whether `pid` refers to a live process.
#[cfg(not(unix))]
pub fn probe_pid(pid: u32) -> PidState {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

    // Targeted single-PID refresh — `new_with_specifics(nothing)` avoids
    // enumerating the full process table on every call (the PTY exit
    // watcher polls this every 100ms).
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    let target = sysinfo::Pid::from(pid as usize);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[target]),
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    if sys.process(target).is_some() {
        PidState::Alive
    } else {
        PidState::Dead
    }
}

/// Convenience boolean view of [`probe_pid`]: `Unknown` counts as alive so
/// callers that guard destructive actions stay conservative.
pub fn pid_alive(pid: u32) -> bool {
    probe_pid(pid) != PidState::Dead
}

/// Kill `root_pid` and every descendant process (Windows).
///
/// Windows has no SIGTERM/SIGKILL split — `Process::kill` is
/// `TerminateProcess`. The calling process and its ancestors are never
/// killed.
#[cfg(not(unix))]
pub fn kill_process_tree(root_pid: u32) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    sys.refresh_processes_specifics(ProcessesToUpdate::All, false, ProcessRefreshKind::nothing());

    // parent → children map for BFS.
    let mut parent_map: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for (pid, process) in sys.processes() {
        if let Some(parent) = process.parent() {
            parent_map
                .entry(parent.as_u32())
                .or_default()
                .push(pid.as_u32());
        }
    }

    // Protect the caller and all ancestors so cleanup cannot suicide.
    let mut protected: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut current = std::process::id();
    protected.insert(current);
    for _ in 0..64 {
        let Some(parent) = sys
            .process(sysinfo::Pid::from(current as usize))
            .and_then(|p| p.parent())
        else {
            break;
        };
        if !protected.insert(parent.as_u32()) {
            break;
        }
        current = parent.as_u32();
    }

    // BFS collect victims.
    let mut queue = std::collections::VecDeque::from([root_pid]);
    let mut victims = Vec::new();
    while let Some(pid) = queue.pop_front() {
        if protected.contains(&pid) {
            continue;
        }
        victims.push(pid);
        if let Some(children) = parent_map.get(&pid) {
            queue.extend(children.iter().copied());
        }
    }

    for pid in victims {
        let spid = sysinfo::Pid::from(pid as usize);
        if let Some(process) = sys.process(spid) {
            let _ = process.kill();
        }
    }
}

/// RAII handle for a Windows Job Object with `KILL_ON_JOB_CLOSE`.
///
/// While the returned guard is alive the assigned child (and its
/// descendants) are tracked; when the last job handle closes — including
/// on process crash or exit — Windows terminates the whole tree. Keep the
/// guard for at least as long as the child should live.
#[cfg(windows)]
pub struct ChildKillJob {
    _job: win32job::Job,
}

/// Assign `child` to a fresh `KILL_ON_JOB_CLOSE` job object.
///
/// Uses `AsRawHandle` (safe API) — no unsafe in this crate. Returns `None`
/// when job creation/assignment fails (callers should continue without
/// the guard, matching the previous unprotected behaviour).
#[cfg(windows)]
pub fn assign_to_kill_job(child: &std::process::Child) -> Option<ChildKillJob> {
    use std::os::windows::io::AsRawHandle;

    assign_handle_to_kill_job(child.as_raw_handle())
}

/// Assign `handle` (any live process handle) to a fresh
/// `KILL_ON_JOB_CLOSE` job object. Used for `tokio::process::Child`,
/// which does not expose `std::process::Child` but does provide
/// `raw_handle()`.
#[cfg(windows)]
pub fn assign_handle_to_kill_job(handle: std::os::windows::io::RawHandle) -> Option<ChildKillJob> {
    let job = win32job::Job::create().ok()?;
    let mut info = job.query_extended_limit_info().ok()?;
    info.limit_kill_on_job_close();
    job.set_extended_limit_info(&info).ok()?;
    job.assign_process(handle as isize).ok()?;
    Some(ChildKillJob { _job: job })
}
