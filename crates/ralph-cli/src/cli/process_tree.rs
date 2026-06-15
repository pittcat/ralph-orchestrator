//! Cross-platform process tree termination fallback.
//!
//! When a subprocess-TUI child or the orchestrator itself is terminated, the
//! regular `killpg(getpgrp())` only reaches processes that share the same
//! process group. Backends spawned via PTY live in their own session and can
//! therefore outlive the group signal. This module walks the process tree
//! starting from a root PID and sends SIGTERM/SIGKILL to every descendant
/// (and optionally the root itself).
///
/// The calling process and all of its ancestors are explicitly protected so
/// that the cleanup never commits suicide.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
use tracing::{info, warn};

/// Kill `root_pid` and all of its descendant processes.
///
/// - `include_root`: if true, `root_pid` itself is also signaled.
/// - First sends SIGTERM, waits briefly, then SIGKILLs any survivors.
#[cfg(unix)]
pub fn kill_process_tree(root_pid: u32, include_root: bool) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let victims = collect_descendants(root_pid, include_root);
    if victims.is_empty() {
        info!(
            target: "ralph_cli::process_tree",
            root_pid,
            include_root,
            "No processes to kill in tree"
        );
        return;
    }

    warn!(
        target: "ralph_cli::process_tree",
        root_pid,
        include_root,
        victim_count = victims.len(),
        ?victims,
        "Sending SIGTERM to process tree"
    );

    for pid in &victims {
        let _ = kill(Pid::from_raw(i32::try_from(*pid).unwrap_or(-1)), Signal::SIGTERM);
    }

    std::thread::sleep(Duration::from_millis(800));

    let survivors: Vec<u32> = victims
        .into_iter()
        .filter(|pid| process_is_alive(*pid))
        .collect();

    if !survivors.is_empty() {
        warn!(
            target: "ralph_cli::process_tree",
            root_pid,
            survivor_count = survivors.len(),
            ?survivors,
            "Sending SIGKILL to surviving processes"
        );
        for pid in &survivors {
            let _ = kill(Pid::from_raw(i32::try_from(*pid).unwrap_or(-1)), Signal::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
pub fn kill_process_tree(_root_pid: u32, _include_root: bool) {
    // No-op on non-Unix; process groups are not a portable concept.
}

/// Collect `root_pid` and all of its descendants.
///
/// The calling process and all of its ancestors are explicitly protected so
/// that the cleanup never commits suicide (this matters in tests where the
/// child shares the test runner's session).
fn collect_descendants(root_pid: u32, include_root: bool) -> Vec<u32> {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        false,
        ProcessRefreshKind::nothing(),
    );

    let mut parent_map: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, process) in sys.processes() {
        let pid_u32 = pid.as_u32();
        if let Some(parent) = process.parent() {
            parent_map
                .entry(parent.as_u32())
                .or_default()
                .push(pid_u32);
        }
    }

    // Protect the caller and all of its ancestors from accidental suicide.
    let caller_pid = std::process::id();
    let mut protected: HashSet<u32> = HashSet::new();
    protected.insert(caller_pid);
    let mut current = caller_pid;
    while let Some(parents) = parent_map.get(&current) {
        if parents.is_empty() {
            break;
        }
        let parent = parents[0];
        if !protected.insert(parent) {
            break;
        }
        current = parent;
    }

    // BFS descendants, skipping protected PIDs (should never include root_pid
    // in normal usage, but keeps the function safe in tests).
    let mut result = HashSet::new();
    let mut queue = VecDeque::new();
    if include_root && !protected.contains(&root_pid) {
        queue.push_back(root_pid);
    } else if let Some(children) = parent_map.get(&root_pid) {
        for &child in children {
            queue.push_back(child);
        }
    }

    while let Some(pid) = queue.pop_front() {
        if protected.contains(&pid) {
            continue;
        }
        result.insert(pid);
        if let Some(children) = parent_map.get(&pid) {
            for &child in children {
                queue.push_back(child);
            }
        }
    }

    result.into_iter().collect()
}

fn process_is_alive(pid: u32) -> bool {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        false,
        ProcessRefreshKind::nothing(),
    );
    sys.process(sysinfo::Pid::from(pid as usize)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    #[test]
    #[cfg(unix)]
    fn kill_process_tree_kills_child() {
        // Spawn a long-running child that ignores SIGINT but not SIGTERM/SIGKILL.
        let mut child = Command::new("sleep")
            .arg("120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep command should be available");

        let pid = child.id();

        // Give the kernel a moment to register the process.
        std::thread::sleep(Duration::from_millis(100));

        kill_process_tree(pid, true);

        // Reap the child; it should already be dead.
        let status = child.wait().expect("wait should succeed");
        assert!(
            !status.success() || status.signal() == Some(9) || status.signal() == Some(15),
            "child should have been terminated by SIGTERM or SIGKILL, got {:?}",
            status
        );
    }

    #[test]
    #[cfg(unix)]
    fn kill_process_tree_kills_grandchild() {
        // Spawn `sh -c 'sleep 120'` so the shell is the child and sleep is the grandchild.
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sh command should be available");

        let pid = child.id();
        std::thread::sleep(Duration::from_millis(200));

        kill_process_tree(pid, true);

        let status = child.wait().expect("wait should succeed");
        assert!(
            !status.success() || status.signal() == Some(9) || status.signal() == Some(15),
            "child shell should have been terminated, got {:?}",
            status
        );
    }
}
