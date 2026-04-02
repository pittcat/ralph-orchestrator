//! Cross-platform process detection and control.
//!
//! Provides unified process enumeration, existence checking, and
//! tree termination across Unix and Windows.
//!
//! # Example
//!
//! ```no_run
//! use ralph_core::platform::process::{process_exists, kill_process_tree};
//!
//! fn check_and_stop(pid: u32) {
//!     if process_exists(pid) {
//!         kill_process_tree(pid).expect("Failed to stop process");
//!     }
//! }
//! ```

use std::io;
use sysinfo::{ProcessRefreshKind, RefreshKind, System};

/// Information about a running process.
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    /// Process ID.
    pub pid: u32,
    /// Process name (if available).
    pub name: Option<String>,
    /// Parent process ID (if available).
    pub parent_pid: Option<u32>,
}

/// Checks if a process with the given PID exists.
///
/// On Unix: Uses signal 0 (no actual signal sent) to check process existence.
/// On Windows: Enumerates processes using sysinfo.
pub fn process_exists(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    let system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );

    system.process(sysinfo::Pid::from(pid as usize)).is_some()
}

/// Lists all child processes of the given PID (recursive).
pub fn list_child_processes(pid: u32) -> Vec<ProcessInfo> {
    let system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );

    let mut children = Vec::new();
    let mut to_check = vec![pid];

    while let Some(parent_pid) = to_check.pop() {
        for process in system.processes().values() {
            if let Some(ppid) = process.parent() {
                let ppid_u32 = ppid.as_u32();
                if ppid_u32 == parent_pid {
                    let child_pid = process.pid().as_u32();
                    children.push(ProcessInfo {
                        pid: child_pid,
                        name: Some(process.name().to_string_lossy().to_string()),
                        parent_pid: Some(parent_pid),
                    });
                    to_check.push(child_pid);
                }
            }
        }
    }

    children
}

/// Kills a process and all its children (tree termination).
///
/// On Unix: Sends SIGTERM to the process group, then SIGKILL if needed.
/// On Windows: Uses `taskkill /T /F` for reliable tree termination.
pub fn kill_process_tree(pid: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        kill_process_tree_unix(pid)
    }

    #[cfg(windows)]
    {
        kill_process_tree_windows(pid)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Process termination not supported on this platform",
        ))
    }
}

#[cfg(unix)]
fn kill_process_tree_unix(pid: u32) -> io::Result<()> {
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;

    // First try SIGTERM on the process group
    let pgid = -(pid as i32);
    let _ = signal::kill(Pid::from_raw(pgid), Signal::SIGTERM);

    // Give processes a moment to terminate gracefully
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Check if process is still alive, send SIGKILL if needed
    if process_exists(pid) {
        let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
    }

    // Also kill any remaining children
    for child in list_child_processes(pid) {
        if process_exists(child.pid) {
            let _ = signal::kill(Pid::from_raw(child.pid as i32), Signal::SIGKILL);
        }
    }

    Ok(())
}

#[cfg(windows)]
fn kill_process_tree_windows(pid: u32) -> io::Result<()> {
    // Use taskkill /T /F for reliable tree termination on Windows
    // /T = terminate process and any children
    // /F = force termination
    let output = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // taskkill returns 128 if the process was already terminated
        // or 1 if the process was not found - both are acceptable
        let code = output.status.code().unwrap_or(-1);
        if code != 128 && code != 1 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("taskkill failed: {}", stderr),
            ));
        }
    }

    Ok(())
}

/// Gets information about a process.
pub fn get_process_info(pid: u32) -> Option<ProcessInfo> {
    let system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );

    system
        .process(sysinfo::Pid::from(pid as usize))
        .map(|p| ProcessInfo {
            pid,
            name: Some(p.name().to_string_lossy().to_string()),
            parent_pid: p.parent().map(|ppid| ppid.as_u32()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_exists_current_pid() {
        // Current process should exist
        let current_pid = std::process::id();
        assert!(process_exists(current_pid));
    }

    #[test]
    fn test_process_exists_invalid_pid() {
        // PID 0 should not exist
        assert!(!process_exists(0));

        // Very high PID unlikely to exist
        assert!(!process_exists(999_999));
    }

    #[test]
    fn test_get_process_info_current() {
        let current_pid = std::process::id();
        let info = get_process_info(current_pid);

        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.pid, current_pid);
    }

    #[test]
    fn test_get_process_info_nonexistent() {
        let info = get_process_info(999_999);
        assert!(info.is_none());
    }

    #[test]
    fn test_list_children_no_children() {
        // Current process should have no children in test context
        let current_pid = std::process::id();
        let children = list_child_processes(current_pid);

        // This might have test runner children, so we just verify it doesn't panic
        // and any returned children have the correct parent
        for child in children {
            assert_eq!(child.parent_pid, Some(current_pid));
        }
    }
}
