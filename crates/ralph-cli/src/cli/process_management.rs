//! Platform-conditional process-group leadership setup.
//!
//! Unix: sets the orchestrator as the process-group leader so all spawned
//! CLI processes (Claude, Kiro, etc.) belong to the same group and receive
//! termination signals together. Non-Unix: no-op (process groups are not
//! a portable concept).
//!
//! Originally `mod process_management` inside `main.rs`; U4 lifts it to a
//! top-level `cli/` module so the cfg branches can be a single `pub fn`
//! re-exported through `cli::process_management::setup_process_group`.

// Unix-specific process management for process group leadership
#[cfg(unix)]
mod unix {
    use nix::unistd::{Pid, getpgrp, setpgid, tcgetpgrp};
    use std::io::{IsTerminal, stdin, stdout};
    use tracing::debug;

    /// Sets up process group leadership.
    ///
    /// Per spec: "The orchestrator must run as a process group leader. All spawned
    /// CLI processes (Claude, Kiro, etc.) belong to this group. On termination,
    /// the entire process group receives the signal, preventing orphans."
    pub fn setup_process_group() {
        // Make ourselves the process group leader when safe.
        // If we're launched by a wrapper (e.g., `npx`), moving to a new process
        // group can drop us out of the foreground TTY group and break TUI input.
        let pid = Pid::this();
        let pgrp = getpgrp();
        if pgrp == pid {
            debug!("Already process group leader: PID {}", pid);
            return;
        }

        if is_foreground_tty_group(pgrp) {
            debug!(
                "Skipping setpgid: keeping foreground process group {}",
                pgrp
            );
            return;
        }

        if let Err(e) = setpgid(pid, pid) {
            // EPERM is OK - we're already a process group leader (e.g., started from shell)
            if e != nix::errno::Errno::EPERM {
                debug!(
                    "Note: Could not set process group ({}), continuing anyway",
                    e
                );
            }
        }
        debug!("Process group initialized: PID {}", pid);
    }

    fn is_foreground_tty_group(current_pgrp: Pid) -> bool {
        // Prefer stdin for foreground checks, fall back to stdout.
        if stdin().is_terminal()
            && let Ok(fg) = tcgetpgrp(stdin())
        {
            return fg == current_pgrp;
        }

        if stdout().is_terminal()
            && let Ok(fg) = tcgetpgrp(stdout())
        {
            return fg == current_pgrp;
        }

        false
    }
}

#[cfg(not(unix))]
mod non_unix {
    /// No-op on non-Unix platforms.
    pub fn setup_process_group() {}
}

#[cfg(not(unix))]
pub use non_unix::setup_process_group;
#[cfg(unix)]
pub use unix::setup_process_group;
