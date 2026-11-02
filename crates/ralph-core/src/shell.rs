//! Cross-platform shell command construction.
//!
//! Unix keeps the historical `sh -c <cmd>` behaviour verbatim. Windows
//! resolves a shell in this order:
//!
//! 1. `RALPH_SHELL` env override (`bash` / `pwsh` / `cmd` / a path)
//! 2. Git for Windows `bash.exe` (NOT `C:\Windows\System32\bash.exe`,
//!    which is the WSL launcher)
//! 3. `pwsh.exe` (PowerShell 7+) / `pwsh` on PATH
//! 4. `cmd.exe` (`%COMSPEC%` fallback)
//!
//! Windows PowerShell 5.1 (`powershell.exe`) is out of scope.

#[cfg(not(unix))]
use std::path::PathBuf;
use std::process::Command;

/// Which shell flavour was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// POSIX `sh`/`bash` (also Git Bash on Windows).
    Sh,
    /// PowerShell 7+ (`pwsh`).
    Pwsh,
    /// Windows `cmd.exe`.
    Cmd,
}

/// Build a `std::process::Command` that runs `command_line` under the
/// resolved shell.
pub fn shell_command(command_line: &str) -> Command {
    #[cfg(unix)]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command_line);
        c
    }

    #[cfg(not(unix))]
    {
        windows_shell_command(command_line)
    }
}

/// Like [`shell_command`], but Unix sites that historically invoked
/// `bash -c` keep `bash -c` verbatim (bashisms in user commands must not
/// regress). Windows uses the same resolved-shell chain.
pub fn bash_command(command_line: &str) -> Command {
    #[cfg(unix)]
    {
        let mut c = Command::new("bash");
        c.arg("-c").arg(command_line);
        c
    }
    #[cfg(not(unix))]
    {
        windows_shell_command(command_line)
    }
}

/// Build a `tokio::process::Command` equivalent of [`shell_command`].
pub fn shell_command_tokio(command_line: &str) -> tokio::process::Command {
    tokio::process::Command::from(shell_command(command_line))
}

/// Resolve an executable name to a spawnable program path.
///
/// Unix returns the name verbatim (identical to today). Windows resolves
/// via `which` so `PATHEXT` shims (`npm.cmd`, `claude.cmd`, `pwsh.exe`)
/// spawn correctly; falls back to the bare name when unresolvable.
pub fn resolve_exe(name: &str) -> std::ffi::OsString {
    resolve_exe_in(name, None)
}

/// [`resolve_exe`] with an explicit PATH value (test seam — avoids
/// `std::env::set_var`, which is `unsafe` under Rust 2024).
pub fn resolve_exe_in(name: &str, path_env: Option<&std::ffi::OsStr>) -> std::ffi::OsString {
    #[cfg(unix)]
    {
        let _ = path_env;
        std::ffi::OsString::from(name)
    }
    #[cfg(not(unix))]
    {
        let lowered = name.to_ascii_lowercase();
        let bash_family = matches!(lowered.as_str(), "sh" | "sh.exe" | "bash" | "bash.exe");
        if let Ok(path) = which_in(name, path_env) {
            // The WSL launcher at System32\bash.exe is not a usable shell.
            if !(bash_family && is_wsl_bash(&path)) {
                return path.into_os_string();
            }
        }
        // `sh`/`bash` requested but not on PATH: fall back to the Git for
        // Windows install location (same chain as `shell_command`).
        if bash_family && let Some(bash) = git_bash_in(path_env) {
            return bash.into_os_string();
        }
        std::ffi::OsString::from(name)
    }
}

/// `which::which` honouring an explicit PATH override when provided.
#[cfg(not(unix))]
fn which_in(
    name: &str,
    path_env: Option<&std::ffi::OsStr>,
) -> Result<std::path::PathBuf, which::Error> {
    match path_env {
        Some(path) => {
            let cwd = std::env::current_dir()
                .map_err(|_| which::Error::CannotGetCurrentDirAndPathListEmpty)?;
            which::which_in(name, Some(path), cwd)
        }
        None => which::which(name),
    }
}

/// Resolve `prog` (a name or path) to a spawnable program plus argument
/// prefix.
///
/// Returns `(program, arg_prefix)`; callers spawn
/// `Command::new(program).args(arg_prefix).args(their_args)`. Unix returns
/// `(prog, [])` verbatim — identical to `Command::new(prog)` today.
///
/// Windows resolves through `which`/`PATHEXT` (as [`resolve_exe`]) and,
/// when the resolved program is a script (`.sh`, `.ps1`, `.py`, …), routes
/// it through the matching interpreter so `Command::new("./backend.sh")`
/// does not fail with `os error 193`. `.cmd`/`.bat` stay direct — Rust
/// std already runs batch files through `cmd.exe`.
pub fn spawn_program(prog: &str) -> (std::ffi::OsString, Vec<std::ffi::OsString>) {
    spawn_program_in(prog, None)
}

/// [`spawn_program`] with an explicit PATH value (test seam — see
/// [`resolve_exe_in`]).
pub fn spawn_program_in(
    prog: &str,
    path_env: Option<&std::ffi::OsStr>,
) -> (std::ffi::OsString, Vec<std::ffi::OsString>) {
    #[cfg(unix)]
    {
        let _ = path_env;
        (std::ffi::OsString::from(prog), Vec::new())
    }

    #[cfg(not(unix))]
    {
        let resolved = resolve_exe_in(prog, path_env);
        let path = PathBuf::from(&resolved);
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        match ext.as_deref() {
            Some("sh") | Some("bash") => {
                let bash = git_bash_in(path_env)
                    .map(|p| p.into_os_string())
                    .unwrap_or_else(|| std::ffi::OsString::from("bash"));
                (bash, vec![resolved])
            }
            Some("ps1") => {
                let pwsh = which_in("pwsh", path_env)
                    .map(|p| p.into_os_string())
                    .unwrap_or_else(|_| std::ffi::OsString::from("pwsh"));
                (
                    pwsh,
                    vec![
                        std::ffi::OsString::from("-NoProfile"),
                        std::ffi::OsString::from("-NonInteractive"),
                        std::ffi::OsString::from("-ExecutionPolicy"),
                        std::ffi::OsString::from("Bypass"),
                        std::ffi::OsString::from("-File"),
                        resolved,
                    ],
                )
            }
            Some("py") => {
                let py = which_in("python", path_env)
                    .map(|p| p.into_os_string())
                    .unwrap_or_else(|_| std::ffi::OsString::from("python"));
                (py, vec![resolved])
            }
            _ => (resolved, Vec::new()),
        }
    }
}

/// Build a `Command` that executes a script file under the right
/// interpreter for its extension.
///
/// Unix keeps the historical `bash <script>` behaviour. Windows picks an
/// interpreter by extension: `.ps1` → `pwsh -File`, `.sh` → Git Bash,
/// `.cmd`/`.bat` → direct spawn (std handles batch files), anything else →
/// direct spawn.
pub fn script_command(script: &std::path::Path) -> Command {
    #[cfg(unix)]
    {
        let mut c = Command::new("bash");
        c.arg(script);
        c
    }
    #[cfg(not(unix))]
    {
        let ext = script
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        match ext.as_deref() {
            Some("ps1") => {
                let pwsh = which::which("pwsh")
                    .map(|p| p.into_os_string())
                    .unwrap_or_else(|_| std::ffi::OsString::from("pwsh"));
                let mut c = Command::new(pwsh);
                c.args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(script);
                c
            }
            Some("sh") | Some("bash") => {
                let bash = git_bash()
                    .map(|p| p.into_os_string())
                    .unwrap_or_else(|| std::ffi::OsString::from("bash"));
                let mut c = Command::new(bash);
                c.arg(script);
                c
            }
            Some("py") => {
                let py = which::which("python")
                    .map(|p| p.into_os_string())
                    .unwrap_or_else(|_| std::ffi::OsString::from("python"));
                let mut c = Command::new(py);
                c.arg(script);
                c
            }
            _ => Command::new(script),
        }
    }
}

/// The resolved shell kind (useful for prompt injection / diagnostics).
#[cfg(unix)]
pub fn resolved_shell_kind() -> ShellKind {
    ShellKind::Sh
}

/// The resolved shell kind (useful for prompt injection / diagnostics).
#[cfg(not(unix))]
pub fn resolved_shell_kind() -> ShellKind {
    windows_resolve().0
}

#[cfg(not(unix))]
fn windows_shell_command(command_line: &str) -> Command {
    let (kind, exe) = windows_resolve();
    match kind {
        ShellKind::Sh | ShellKind::Pwsh | ShellKind::Cmd => {
            let mut c = Command::new(exe);
            match kind {
                ShellKind::Sh => {
                    c.arg("-c").arg(command_line);
                }
                ShellKind::Pwsh => {
                    c.args(["-NoProfile", "-NonInteractive", "-Command"])
                        .arg(command_line);
                }
                ShellKind::Cmd => {
                    // /s: treat the rest of the line literally; use raw_arg
                    // so the command is not re-quoted by std.
                    use std::os::windows::process::CommandExt;
                    c.args(["/d", "/s", "/c"]);
                    c.raw_arg(command_line);
                }
            }
            c
        }
    }
}

#[cfg(not(unix))]
fn windows_resolve() -> (ShellKind, PathBuf) {
    windows_resolve_from(
        std::env::var_os("RALPH_SHELL"),
        std::env::var_os("COMSPEC"),
        None,
    )
}

/// [`windows_resolve`] with explicit env values (test seam — avoids
/// `std::env::set_var`, which is `unsafe` under Rust 2024).
/// `path_env` feeds every `which` lookup; `None` uses ambient PATH.
#[cfg(not(unix))]
#[doc(hidden)]
pub fn windows_resolve_from(
    ralph_shell: Option<std::ffi::OsString>,
    comspec: Option<std::ffi::OsString>,
    path_env: Option<&std::ffi::OsStr>,
) -> (ShellKind, PathBuf) {
    if let Some(explicit) = explicit_shell_from(ralph_shell, comspec.as_ref(), path_env) {
        return explicit;
    }
    if let Some(bash) = git_bash_in(path_env) {
        return (ShellKind::Sh, bash);
    }
    if let Some(pwsh) = which_in("pwsh", path_env)
        .ok()
        .or_else(|| which_in("pwsh.exe", path_env).ok())
    {
        return (ShellKind::Pwsh, pwsh);
    }
    let comspec = comspec
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cmd.exe"));
    (ShellKind::Cmd, comspec)
}

#[cfg(not(unix))]
fn explicit_shell_from(
    ralph_shell: Option<std::ffi::OsString>,
    comspec: Option<&std::ffi::OsString>,
    path_env: Option<&std::ffi::OsStr>,
) -> Option<(ShellKind, PathBuf)> {
    let value = ralph_shell?;
    let text = value.to_string_lossy().trim().to_string();
    if text.is_empty() {
        return None;
    }
    let lowered = text.to_ascii_lowercase();
    let path = PathBuf::from(&text);
    if path.is_file() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let kind = if name.contains("bash") || name == "sh.exe" || name == "sh" {
            ShellKind::Sh
        } else if name.starts_with("pwsh") || name.starts_with("powershell") {
            ShellKind::Pwsh
        } else {
            ShellKind::Cmd
        };
        return Some((kind, path));
    }
    match lowered.as_str() {
        "bash" | "sh" => git_bash_in(path_env)
            .map(|p| (ShellKind::Sh, p))
            .or_else(|| {
                which_in("bash", path_env)
                    .ok()
                    .filter(|p| !is_wsl_bash(p))
                    .map(|p| (ShellKind::Sh, p))
            }),
        "pwsh" | "powershell" => which_in("pwsh", path_env)
            .ok()
            .map(|p| (ShellKind::Pwsh, p)),
        "cmd" => {
            let comspec = comspec
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("cmd.exe"));
            Some((ShellKind::Cmd, comspec))
        }
        _ => None,
    }
}

/// True if `path` is the WSL launcher `bash.exe` that ships in System32.
#[cfg(not(unix))]
fn is_wsl_bash(path: &std::path::Path) -> bool {
    let p = path
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('/', "\\");
    p.ends_with("\\windows\\system32\\bash.exe")
}

/// Locate Git for Windows `bash.exe` without hitting the WSL launcher.
#[cfg(not(unix))]
fn git_bash() -> Option<PathBuf> {
    git_bash_in(None)
}

/// [`git_bash`] with an explicit PATH value (test seam).
#[cfg(not(unix))]
fn git_bash_in(path_env: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    // Derive from `git.exe` location: <Git>\cmd\git.exe → <Git>\bin\bash.exe
    if let Ok(git) = which_in("git", path_env) {
        for ancestor in git.ancestors().skip(1) {
            for candidate in [
                ancestor.join("bin").join("bash.exe"),
                ancestor.join("usr").join("bin").join("bash.exe"),
            ] {
                if candidate.is_file() && !is_wsl_bash(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    // A `bash` on PATH that is not the WSL launcher (e.g. Git\bin on PATH).
    which_in("bash", path_env)
        .ok()
        .filter(|bash| bash.is_file() && !is_wsl_bash(bash))
}
