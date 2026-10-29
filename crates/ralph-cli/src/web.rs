// ABOUTME: Web dashboard launcher and lifecycle manager.
// ABOUTME: `ralph web` runs the monitor (or --dev stack) in the foreground;
// ABOUTME: `ralph web start|status|stop` manages a detached background instance
// ABOUTME: via a state file at ~/.ralph/web.json.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command as AsyncCommand};
use tokio::sync::Notify;

#[cfg(unix)]
use nix::sys::signal::kill;
#[cfg(unix)]
use nix::unistd::Pid;

/// Grace period for servers to shut down before SIGKILL (matches backend's SIGINT handler)
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(10);

/// Timeout for both servers to become ready
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long `ralph web start` waits for the detached child to write its
/// state file before reporting "still starting" (npm installs can be slow).
const DAEMON_STATE_TIMEOUT: Duration = Duration::from_secs(90);

/// State file name inside the web state dir (`~/.ralph/` by default).
const STATE_FILE_NAME: &str = "web.json";

/// Daemon log file name inside the web state dir.
const LOG_FILE_NAME: &str = "web.log";

/// Env override for the web state dir (tests / custom installs).
const STATE_DIR_ENV: &str = "RALPH_WEB_STATE_DIR";

/// Default monitor port — also the port `status`/`stop` probe for an
/// unmanaged monitor when no state file exists.
const DEFAULT_MONITOR_PORT: u16 = 3200;

/// Arguments for the web subcommand
#[derive(Parser, Debug)]
pub struct WebArgs {
    #[command(subcommand)]
    pub command: Option<WebCommand>,

    #[command(flatten)]
    pub run: RunArgs,
}

/// Lifecycle subcommands for `ralph web`.
#[derive(clap::Subcommand, Debug)]
pub enum WebCommand {
    /// Start the dashboard in the background (detached).
    /// Pass --foreground to run attached like bare `ralph web`.
    Start(StartArgs),
    /// Report dashboard status
    Status,
    /// Stop the running dashboard
    Stop,
}

/// Arguments for `ralph web start`.
#[derive(clap::Args, Debug)]
pub struct StartArgs {
    /// Stay in the foreground instead of detaching into the background
    #[arg(long)]
    pub foreground: bool,

    #[command(flatten)]
    pub run: RunArgs,
}

/// Run flags shared by bare `ralph web` and `ralph web start`.
#[derive(clap::Args, Debug, Clone, PartialEq, Eq)]
pub struct RunArgs {
    /// Host/interface to bind (default: 127.0.0.1). Use 0.0.0.0 to expose
    /// the dashboard on the LAN — it has no auth layer.
    #[arg(long)]
    pub host: Option<String>,

    /// Backend port (default: 3200 for the monitor, 3000 for --dev)
    #[arg(long)]
    pub backend_port: Option<u16>,

    /// Frontend port (--dev mode only, default: 5173)
    #[arg(long, default_value = "5173")]
    pub frontend_port: u16,

    /// Workspace root directory (default: current directory)
    #[arg(long)]
    pub workspace: Option<PathBuf>,

    /// Use deprecated Node tRPC backend instead of Rust RPC API (--dev mode only)
    #[arg(long)]
    pub legacy_node_api: bool,

    /// Run the monitor dashboard only (single process; no vite/api).
    /// This is the default; kept for backwards compatibility.
    #[arg(long, conflicts_with = "dev")]
    pub monitor: bool,

    /// Run the Vite + ralph-api development stack instead of the monitor
    #[arg(long)]
    pub dev: bool,

    /// Don't open the dashboard in the default browser
    #[arg(long)]
    pub no_open: bool,
}

impl Default for RunArgs {
    fn default() -> Self {
        Self {
            host: None,
            backend_port: None,
            frontend_port: 5173,
            workspace: None,
            legacy_node_api: false,
            monitor: false,
            dev: false,
            no_open: false,
        }
    }
}

/// Which surface `ralph web` is running; persisted in the state file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WebMode {
    Monitor,
    Dev,
}

impl std::fmt::Display for WebMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebMode::Monitor => f.write_str("monitor"),
            WebMode::Dev => f.write_str("dev"),
        }
    }
}

/// Managed-instance record persisted at `~/.ralph/web.json`. The pid is the
/// `ralph web` supervisor process itself — signalling it triggers the
/// graceful child-shutdown handlers already installed for both modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebState {
    pid: u32,
    mode: WebMode,
    /// Interface the surface binds. `127.0.0.1` when the state file was
    /// written before host tracking existed.
    #[serde(default = "default_state_host")]
    host: String,
    backend_port: u16,
    frontend_port: u16,
    workspace: PathBuf,
    started: DateTime<Utc>,
}

fn default_state_host() -> String {
    "127.0.0.1".to_string()
}

/// Result of inspecting the state file on disk.
#[derive(Debug)]
enum StateProbe {
    /// State file exists and its pid is alive.
    Live(WebState),
    /// State file existed but was stale (dead pid or unparseable) — removed.
    Stale,
    /// No state file.
    Absent,
}

fn is_transient_exec_error(err: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        const ETXTBSY: i32 = 26;
        if err.raw_os_error() == Some(ETXTBSY) {
            return true;
        }
    }

    let message = err.to_string().to_ascii_lowercase();
    message.contains("text file busy") || message.contains("etxtbsy")
}

fn run_command_with_retry(command: &OsStr, args: &[&str]) -> std::io::Result<std::process::Output> {
    const MAX_ATTEMPTS: usize = 5;

    for attempt in 0..MAX_ATTEMPTS {
        match Command::new(command).args(args).output() {
            Ok(output) => return Ok(output),
            Err(err) => {
                if is_transient_exec_error(&err) && attempt + 1 < MAX_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(25));
                    continue;
                }
                return Err(err);
            }
        }
    }

    unreachable!("retry loop should always return before exhausting attempts")
}

async fn run_async_command_with_retry(
    command: &OsStr,
    args: &[&str],
    current_dir: &Path,
) -> std::io::Result<std::process::Output> {
    const MAX_ATTEMPTS: usize = 5;

    for attempt in 0..MAX_ATTEMPTS {
        match AsyncCommand::new(command)
            .args(args)
            .current_dir(current_dir)
            .output()
            .await
        {
            Ok(output) => return Ok(output),
            Err(err) => {
                if is_transient_exec_error(&err) && attempt + 1 < MAX_ATTEMPTS {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    continue;
                }
                return Err(err);
            }
        }
    }

    unreachable!("retry loop should always return before exhausting attempts")
}

/// Check that Node.js is installed and >= 18. Returns the version string.
fn check_node_with(node_cmd: &OsStr) -> Result<String> {
    let output = run_command_with_retry(node_cmd, &["--version"]).map_err(|_| {
        anyhow::anyhow!(
            "Node.js is not installed or not in PATH.\n\
             Install Node.js 18+: https://nodejs.org/\n\
             Or via nvm: nvm install 18"
        )
    })?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to run `node --version`.\n\
             Install Node.js 18+: https://nodejs.org/"
        );
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Parse major version from e.g. "v18.17.0"
    let major: u32 = version
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if major < 18 {
        anyhow::bail!(
            "Node.js {} is too old (need >= 18).\n\
             Update: https://nodejs.org/ or `nvm install 18`",
            version
        );
    }

    Ok(version)
}

/// Check that npm is installed and working. Returns the version string.
fn check_npm_with(npm_cmd: &OsStr) -> Result<String> {
    let output = run_command_with_retry(npm_cmd, &["--version"]).map_err(|_| {
        anyhow::anyhow!(
            "npm is not installed or not in PATH.\n\
             npm should come with Node.js. Try reinstalling Node: https://nodejs.org/"
        )
    })?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to run `npm --version`.\n\
             Try reinstalling Node.js: https://nodejs.org/"
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Check if npm dependencies need to be installed.
fn needs_install(root: &Path) -> bool {
    !root.join("node_modules/.package-lock.json").exists()
}

/// Run npm install (or npm ci if lockfile present) with a spinner.
async fn run_npm_install_with(root: &Path, npm_cmd: &OsStr) -> Result<()> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .expect("valid template"),
    );

    let has_lockfile = root.join("package-lock.json").exists();
    let install_cmd = if has_lockfile { "ci" } else { "install" };

    spinner.set_message(format!("Running npm {}...", install_cmd));
    spinner.enable_steady_tick(Duration::from_millis(100));

    let output = run_async_command_with_retry(npm_cmd, &[install_cmd], root)
        .await
        .context("Failed to run npm install")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("npm {} failed:\n{}", install_cmd, stderr.trim());
    }

    println!("Dependencies installed successfully.");
    Ok(())
}

/// Check that a TCP port is available for binding.
fn check_port_available(port: u16) -> Result<()> {
    match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => Ok(()),
        Err(_) => {
            anyhow::bail!(
                "Port {} is already in use.\n\
                 Use --backend-port or --frontend-port to pick a different port.\n\
                 To free the port: fuser -k {}/tcp",
                port,
                port
            );
        }
    }
}

const BLOCKED_TSX_VERSION: &str = "4.20.0";

/// Check for tsx 4.20.0 which has known issues.
fn check_tsx_version_with(backend_dir: &Path, npx_cmd: &OsStr) -> Result<()> {
    // On some CI filesystems, executing a just-written test helper script can
    // transiently fail with ETXTBSY/Text file busy. Retry briefly to avoid flakes.
    let output = run_tsx_version_command_with_retry(backend_dir, npx_cmd);

    if let Some(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if contains_blocked_tsx_version(&stdout) || contains_blocked_tsx_version(&stderr) {
            anyhow::bail!(
                "tsx 4.20.0 has known issues that affect the web server.\n\
                 Fix: npm install tsx@^4.21.0 -w @ralph-web/server"
            );
        }
    }

    // If we can't run tsx or it doesn't match, proceed silently.
    Ok(())
}

fn run_tsx_version_command_with_retry(
    backend_dir: &Path,
    npx_cmd: &OsStr,
) -> Option<std::process::Output> {
    const MAX_ATTEMPTS: usize = 5;

    for attempt in 0..MAX_ATTEMPTS {
        match Command::new(npx_cmd)
            .args(["tsx", "--version"])
            .current_dir(backend_dir)
            .output()
        {
            Ok(output) => {
                // Success: parse this output.
                if output.status.success() {
                    return Some(output);
                }

                // Retry transient ETXTBSY/text-file-busy errors seen in CI.
                let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
                let is_transient = stderr.contains("text file busy") || stderr.contains("etxtbsy");
                if is_transient && attempt + 1 < MAX_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(25));
                    continue;
                }

                // Non-transient failure: return output for best-effort parsing.
                return Some(output);
            }
            Err(err) => {
                // Retry transient execution errors for freshly written helper scripts.
                if is_transient_exec_error(&err) && attempt + 1 < MAX_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(25));
                    continue;
                }

                // Command missing/unrunnable.
                return None;
            }
        }
    }

    None
}

fn contains_blocked_tsx_version(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }

        // Bare outputs: "4.20.0" or "v4.20.0"
        if trimmed == BLOCKED_TSX_VERSION
            || trimmed
                .strip_prefix('v')
                .is_some_and(|version| version == BLOCKED_TSX_VERSION)
        {
            return true;
        }

        // Labeled output examples:
        // - "tsx 4.20.0"
        // - "tsx v4.20.0"
        // - "tsx v4.20.0 (node v22.0.0)"
        if trimmed.to_ascii_lowercase().contains("tsx") {
            return trimmed.split_whitespace().any(|token| {
                let cleaned = token
                    .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.')
                    .trim_start_matches('v');
                cleaned == BLOCKED_TSX_VERSION
            });
        }

        false
    })
}

/// Run pre-flight checks: verify Node.js/npm and install frontend dependencies.
/// In legacy mode, also validates tsx backend tooling.
async fn preflight_with(
    root: &Path,
    backend_dir: &Path,
    legacy_node_api: bool,
    node_cmd: &OsStr,
    npm_cmd: &OsStr,
    npx_cmd: &OsStr,
) -> Result<()> {
    let node_version = check_node_with(node_cmd)?;
    let npm_version = check_npm_with(npm_cmd)?;
    println!(
        "Using Node {} with npm {}",
        node_version.trim_start_matches('v'),
        npm_version
    );

    if needs_install(root) {
        println!("node_modules not found — installing dependencies...");
        run_npm_install_with(root, npm_cmd).await?;
    }

    if legacy_node_api {
        check_tsx_version_with(backend_dir, npx_cmd)?;
    }

    Ok(())
}

async fn preflight(root: &Path, backend_dir: &Path, legacy_node_api: bool) -> Result<()> {
    preflight_with(
        root,
        backend_dir,
        legacy_node_api,
        OsStr::new("node"),
        OsStr::new("npm"),
        OsStr::new("npx"),
    )
    .await
}

/// Forward output from a child process, prefixing each line with a label.
/// Notifies `ready` when the output contains the given ready pattern.
async fn forward_output(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    label: &str,
    ready_pattern: &str,
    ready: std::sync::Arc<Notify>,
) {
    let label_out = label.to_string();
    let label_err = label.to_string();
    let pattern_out = ready_pattern.to_string();
    let pattern_err = ready_pattern.to_string();
    let ready_out = ready.clone();
    let ready_err = ready;

    let stdout_task = tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut notified = false;
        while let Ok(Some(line)) = lines.next_line().await {
            println!("[{}] {}", label_out, line);
            if !notified && line.contains(&pattern_out) {
                ready_out.notify_one();
                notified = true;
            }
        }
    });

    let stderr_task = tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut notified = false;
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[{}] {}", label_err, line);
            if !notified && line.contains(&pattern_err) {
                ready_err.notify_one();
                notified = true;
            }
        }
    });

    // Run both forwarding tasks to completion (they end when the process closes its pipes)
    let _ = tokio::join!(stdout_task, stderr_task);
}

// ---------------------------------------------------------------------------
// Lifecycle management (start / status / stop + state file)
// ---------------------------------------------------------------------------

/// The web state dir: `$RALPH_WEB_STATE_DIR` if set, else `~/.ralph`
/// (sibling of the monitor's own `monitor.db` / `projects.d`).
fn web_state_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os(STATE_DIR_ENV)
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    env::home_dir().map(|home| home.join(".ralph"))
}

fn state_file_path(dir: &Path) -> PathBuf {
    dir.join(STATE_FILE_NAME)
}

/// Check whether a pid refers to a live process. EPERM counts as alive
/// (we cannot signal it, but it exists and is not ours to clean up).
fn pid_alive(pid: u32) -> bool {
    // kill() treats pid <= 0 specially (process group / every process),
    // so values that cannot be a real single-process pid report dead.
    let Ok(pid_i32) = i32::try_from(pid) else {
        return false;
    };
    if pid_i32 <= 0 {
        return false;
    }
    #[cfg(unix)]
    {
        match kill(Pid::from_raw(pid_i32), None) {
            Ok(()) | Err(nix::errno::Errno::EPERM) => !is_zombie(pid_i32),
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
        let mut sys = sysinfo::System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            false,
            ProcessRefreshKind::nothing(),
        );
        sys.process(sysinfo::Pid::from_u32(pid)).is_some()
    }
}

/// Linux: zombies still answer kill(pid, 0) but are already dead — a
/// zombie supervisor no longer serves anything, so report it as dead.
#[cfg(target_os = "linux")]
fn is_zombie(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            // Fields after the last ')' are `state ppid ...`; comm may
            // itself contain parens, so split from the right.
            stat.rsplit(')')
                .next()
                .map(|rest| rest.trim_start().starts_with('Z'))
        })
        .unwrap_or(false)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn is_zombie(_pid: i32) -> bool {
    false
}

/// Read + validate the state file. Stale entries (dead pid, unparseable
/// JSON) are removed as a side effect.
fn inspect_state(dir: &Path) -> StateProbe {
    let path = state_file_path(dir);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(_) => return StateProbe::Absent,
    };
    match serde_json::from_str::<WebState>(&content) {
        Ok(state) if pid_alive(state.pid) => StateProbe::Live(state),
        _ => {
            let _ = std::fs::remove_file(&path);
            StateProbe::Stale
        }
    }
}

fn write_state(dir: &Path, state: &WebState) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = state_file_path(dir);
    let tmp = dir.join(format!("{STATE_FILE_NAME}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(state)?)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Removes the state file if it still points at `pid` (guards against
/// deleting a newer instance's record).
fn remove_state_if_matches(dir: &Path, pid: u32) {
    let path = state_file_path(dir);
    let stale = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<WebState>(&c).ok())
        .is_none_or(|s| s.pid == pid);
    if stale {
        let _ = std::fs::remove_file(&path);
    }
}

/// Deletes our own state file when the supervisor exits for any reason
/// (signal shutdown, child exit, error bail). Only removes the file while
/// it still names our pid, so a newer instance's record survives.
struct StateGuard {
    dir: PathBuf,
    pid: u32,
}

impl StateGuard {
    fn new(dir: &Path, pid: u32) -> Self {
        Self {
            dir: dir.to_path_buf(),
            pid,
        }
    }
}

impl Drop for StateGuard {
    fn drop(&mut self) {
        remove_state_if_matches(&self.dir, self.pid);
    }
}

/// The user-facing URLs for a running instance. Loopback binds show a
/// single dashboard line; wildcard binds show local + LAN; a specific
/// non-loopback bind shows that address only.
fn display_urls(state: &WebState) -> Vec<(&'static str, String)> {
    let port = match state.mode {
        WebMode::Monitor => state.backend_port,
        WebMode::Dev => state.frontend_port,
    };
    display_urls_for(&state.host, port)
}

fn display_urls_for(host: &str, port: u16) -> Vec<(&'static str, String)> {
    let normalized = host.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    match normalized.as_str() {
        "127.0.0.1" | "localhost" | "::1" => {
            vec![("dashboard", format!("http://localhost:{port}"))]
        }
        "0.0.0.0" | "::" | "*" => {
            let mut urls = vec![("local", format!("http://localhost:{port}"))];
            if let Some(ip) = lan_ip() {
                urls.push(("lan", format!("http://{ip}:{port}")));
            }
            urls
        }
        specific => vec![("dashboard", format!("http://{specific}:{port}"))],
    }
}

fn print_urls(state: &WebState) {
    for (label, url) in display_urls(state) {
        println!("  {label}: {url}");
    }
}

/// Primary LAN IPv4 of this machine — the best-scoring non-loopback v4
/// address across interfaces (RFC1918 wins over VPN/overlay ranges like
/// Tailscale's CGNAT block). No egress needed — pure interface
/// enumeration.
fn lan_ip() -> Option<String> {
    use std::net::IpAddr;
    let networks = sysinfo::Networks::new_with_refreshed_list();
    networks
        .values()
        .flat_map(|data| data.ip_networks().iter())
        .filter_map(|net| match net.addr {
            IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local() => {
                Some((lan_ipv4_score(&v4), v4.to_string()))
            }
            _ => None,
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, ip)| ip)
}

fn lan_ipv4_score(ip: &std::net::Ipv4Addr) -> u8 {
    let o = ip.octets();
    if o[0] == 192 && o[1] == 168 {
        4
    } else if o[0] == 10 {
        3
    } else if o[0] == 172 && (16..32).contains(&o[1]) {
        2
    } else {
        1
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(
        host.trim()
            .trim_matches(['[', ']'])
            .to_ascii_lowercase()
            .as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

/// Probe the monitor's health endpoint; returns `machine_id` when a Ralph
/// monitor answers. Used to distinguish "our dashboard is up" from "an
/// unrelated process holds the port".
async fn probe_monitor_health(port: u16) -> Option<String> {
    probe_health_json(port, "/api/monitor/v1/health")
        .await
        .and_then(|body| {
            if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                Some(
                    body.get("machine_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                )
            } else {
                None
            }
        })
}

/// Probe ralph-api's `/health` (dev mode); true when it answers 2xx.
async fn probe_api_health(port: u16) -> bool {
    probe_health_json(port, "/health").await.is_some()
}

async fn probe_health_json(port: u16, path: &str) -> Option<serde_json::Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .build()
        .ok()?;
    let resp = client
        .get(format!("http://127.0.0.1:{port}{path}"))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

fn port_in_use(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
}

fn resolve_workspace(workspace: Option<&Path>) -> Result<PathBuf> {
    match workspace {
        Some(path) => path
            .canonicalize()
            .with_context(|| format!("Invalid workspace path: {}", path.display())),
        None => env::current_dir().context("Failed to get current directory"),
    }
}

fn print_already_running(state: &WebState) {
    println!(
        "ralph web is already running (pid {}, {}):",
        state.pid, state.mode
    );
    print_urls(state);
    println!("Use `ralph web status` for details or `ralph web stop` to stop it.");
}

fn print_unmanaged_monitor(port: u16, machine_id: &str) {
    println!("A Ralph monitor is already running on port {port} (not managed by `ralph web`):");
    println!("  Dashboard:  http://localhost:{port}");
    println!("  Machine ID: {machine_id}");
    println!();
    println!("`ralph web stop` only manages instances started via `ralph web`.");
    println!("To stop this one manually: fuser -k {port}/tcp");
}

/// `ralph web` / `ralph web start` — start foreground or detached. When a
/// managed (or recognizable unmanaged) instance is already up, report its
/// status instead of dying on the port check. `state_dir` is injected so
/// tests can point at a tempdir instead of `~/.ralph`.
async fn start_command(run: &RunArgs, foreground: bool, state_dir: &Path) -> Result<()> {
    let workspace_root = resolve_workspace(run.workspace.as_deref())?;
    if let StateProbe::Live(state) = inspect_state(state_dir) {
        print_already_running(&state);
        return Ok(());
    }

    let mode = if run.dev {
        WebMode::Dev
    } else {
        WebMode::Monitor
    };

    // Monitor mode: when the port is taken, adopt the "it is a ralph
    // monitor" case via the health endpoint instead of erroring out.
    // Runs before the daemon/foreground split so `start` reports the
    // running instance instead of the detached child quietly exiting.
    if mode == WebMode::Monitor {
        let port = run.backend_port.unwrap_or(DEFAULT_MONITOR_PORT);
        if port_in_use(port) {
            if let Some(machine_id) = probe_monitor_health(port).await {
                print_unmanaged_monitor(port, &machine_id);
                return Ok(());
            }
            // Unknown holder or the port just freed — surface the
            // standard port error when it is still busy.
            check_port_available(port)?;
        }
    }

    // Effective bind host recorded in state. Monitor defaults to loopback;
    // dev's Vite binds all interfaces by default (`host: true` in the
    // frontend config) unless --host restricts it.
    let host = run.host.clone().unwrap_or_else(|| match mode {
        WebMode::Monitor => "127.0.0.1".to_string(),
        WebMode::Dev => "0.0.0.0".to_string(),
    });
    if mode == WebMode::Monitor && !is_loopback_host(&host) {
        println!(
            "WARNING: binding the monitor to {host} — it has no auth layer \
             and will be reachable from the LAN."
        );
    }

    if !foreground {
        return spawn_daemon(run, &workspace_root, state_dir).await;
    }

    let backend_port = run.backend_port.unwrap_or(match mode {
        WebMode::Monitor => DEFAULT_MONITOR_PORT,
        WebMode::Dev => 3000,
    });
    write_state(
        state_dir,
        &WebState {
            pid: std::process::id(),
            mode,
            host: host.clone(),
            backend_port,
            frontend_port: run.frontend_port,
            workspace: workspace_root.clone(),
            started: Utc::now(),
        },
    )?;
    let _state_guard = StateGuard::new(state_dir, std::process::id());

    match mode {
        WebMode::Monitor => execute_monitor(run, &workspace_root, backend_port).await,
        WebMode::Dev => execute_dev(run, &workspace_root).await,
    }
}

/// Build the argv for the detached child: bare `ralph web` (foreground
/// path) plus the run flags that affect the instance.
fn daemon_argv(run: &RunArgs, workspace_root: &Path) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        OsStr::new("web").to_os_string(),
        OsStr::new("--workspace").to_os_string(),
        workspace_root.as_os_str().to_os_string(),
        OsStr::new("--no-open").to_os_string(),
    ];
    if let Some(host) = &run.host {
        argv.push(OsStr::new("--host").to_os_string());
        argv.push(host.as_str().into());
    }
    if let Some(port) = run.backend_port {
        argv.push(OsStr::new("--backend-port").to_os_string());
        argv.push(port.to_string().into());
    }
    if run.frontend_port != RunArgs::default().frontend_port {
        argv.push(OsStr::new("--frontend-port").to_os_string());
        argv.push(run.frontend_port.to_string().into());
    }
    if run.dev {
        argv.push(OsStr::new("--dev").to_os_string());
    }
    if run.monitor {
        argv.push(OsStr::new("--monitor").to_os_string());
    }
    if run.legacy_node_api {
        argv.push(OsStr::new("--legacy-node-api").to_os_string());
    }
    argv
}

/// `ralph web start` (no --foreground): spawn a detached `ralph web`
/// child, redirect its output to `~/.ralph/web.log`, then wait for the
/// state file (and health endpoint) before reporting.
async fn spawn_daemon(run: &RunArgs, workspace_root: &Path, state_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("failed to create {}", state_dir.display()))?;
    let log_path = state_dir.join(LOG_FILE_NAME);
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;
    let err_log = log.try_clone()?;

    let exe = env::current_exe().context("failed to resolve the ralph binary")?;
    let mut cmd = Command::new(exe);
    cmd.args(daemon_argv(run, workspace_root))
        .current_dir(workspace_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err_log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // New session/process group so the daemon survives terminal exit.
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .context("failed to spawn the detached `ralph web` process")?;
    println!("ralph web starting in background (pid {})", child.id());
    println!("  log: {}", log_path.display());

    let deadline = Instant::now() + DAEMON_STATE_TIMEOUT;
    let state = loop {
        if let Some(status) = child.try_wait().context("failed to poll daemon")? {
            eprintln!();
            eprintln!("`ralph web` exited during startup ({status}); recent log:");
            tail_log(&log_path, 20);
            anyhow::bail!("background start failed — see {}", log_path.display());
        }
        if let StateProbe::Live(state) = inspect_state(state_dir) {
            break state;
        }
        if Instant::now() >= deadline {
            println!();
            println!("Still starting (first-run npm install can take a while).");
            println!("Check `ralph web status` in a moment.");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };

    // Wait for the surface to actually answer so the printed URL is live.
    let ready_deadline = Instant::now() + READY_TIMEOUT;
    let ready = loop {
        let up = match state.mode {
            WebMode::Monitor => probe_monitor_health(state.backend_port).await.is_some(),
            WebMode::Dev => !port_in_use(state.frontend_port),
        };
        if up {
            break true;
        }
        if let Some(status) = child.try_wait().context("failed to poll daemon")? {
            eprintln!();
            eprintln!("`ralph web` exited during startup ({status}); recent log:");
            tail_log(&log_path, 20);
            anyhow::bail!("background start failed — see {}", log_path.display());
        }
        if Instant::now() >= ready_deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };

    println!();
    if ready {
        println!("ralph web is running:");
    } else {
        println!("ralph web is still starting:");
    }
    println!("  pid:       {}", state.pid);
    println!("  mode:      {}", state.mode);
    print_urls(&state);
    println!("  workspace: {}", state.workspace.display());
    println!();
    println!("`ralph web status` for details · `ralph web stop` to stop");
    Ok(())
}

fn tail_log(path: &Path, lines: usize) {
    let Ok(content) = std::fs::read_to_string(path) else {
        eprintln!("  (log not readable)");
        return;
    };
    let tail: Vec<&str> = content.lines().rev().take(lines).collect();
    if tail.is_empty() {
        eprintln!("  (log is empty)");
        return;
    }
    for line in tail.into_iter().rev() {
        eprintln!("  {line}");
    }
}

/// `ralph web status` — managed instance first, then an unmanaged-monitor
/// probe on the default port. Exits non-zero when nothing is running.
async fn status_command(state_dir: &Path) -> Result<()> {
    if let StateProbe::Live(state) = inspect_state(state_dir) {
        let healthy = match state.mode {
            WebMode::Monitor => probe_monitor_health(state.backend_port).await.is_some(),
            WebMode::Dev => probe_api_health(state.backend_port).await,
        };
        println!("ralph web is running:");
        println!("  pid:       {}", state.pid);
        println!("  mode:      {}", state.mode);
        print_urls(&state);
        if state.mode == WebMode::Dev {
            println!("  api:       http://localhost:{}", state.backend_port);
        }
        println!("  workspace: {}", state.workspace.display());
        println!(
            "  started:   {}",
            state.started.format("%Y-%m-%d %H:%M:%S UTC")
        );
        println!(
            "  health:    {}",
            if healthy {
                "responding"
            } else {
                "not responding"
            }
        );
        println!();
        println!("`ralph web stop` to stop");
        return Ok(());
    }

    if let Some(machine_id) = probe_monitor_health(DEFAULT_MONITOR_PORT).await {
        println!(
            "A Ralph monitor is running on port {DEFAULT_MONITOR_PORT} (not managed by `ralph web`):"
        );
        println!("  Dashboard:  http://localhost:{DEFAULT_MONITOR_PORT}");
        println!("  Machine ID: {machine_id}");
        println!();
        println!(
            "`ralph web stop` cannot control it — stop manually: fuser -k {DEFAULT_MONITOR_PORT}/tcp"
        );
        return Ok(());
    }

    println!("ralph web is not running.");
    std::process::exit(1);
}

/// `ralph web stop` — SIGTERM the supervisor pid recorded in the state
/// file, escalate to SIGKILL after the grace period, drop the state file.
async fn stop_command(state_dir: &Path) -> Result<()> {
    if let StateProbe::Live(state) = inspect_state(state_dir) {
        println!("Stopping ralph web (pid {}, {}):", state.pid, state.mode);
        print_urls(&state);
        terminate_pid(state.pid, SHUTDOWN_GRACE_PERIOD).await?;
        remove_state_if_matches(state_dir, state.pid);
        println!("Stopped.");
        return Ok(());
    }

    if let Some(machine_id) = probe_monitor_health(DEFAULT_MONITOR_PORT).await {
        println!(
            "A Ralph monitor is running on port {DEFAULT_MONITOR_PORT} (machine {machine_id})"
        );
        println!("but was not started via `ralph web` — it is not managed.");
        println!("Stop it manually: fuser -k {DEFAULT_MONITOR_PORT}/tcp");
        anyhow::bail!("no managed `ralph web` instance to stop");
    }

    println!("ralph web is not running.");
    Ok(())
}

/// Stop the recorded supervisor pid and its whole process tree
/// (`npm run` wrappers orphan their node/tsx grandchildren when only the
/// direct child is signaled). `kill_process_tree` sends SIGTERM to every
/// descendant at once, then SIGKILLs survivors.
#[cfg(unix)]
async fn terminate_pid(pid: u32, _grace: Duration) -> Result<()> {
    // Refuse values kill() would reinterpret as a process group or
    // "every process" — signalling those would be catastrophic.
    i32::try_from(pid)
        .ok()
        .filter(|p| *p > 0)
        .context("invalid pid")?;
    tokio::task::spawn_blocking(move || {
        crate::cli::process_tree::kill_process_tree(pid, true);
    })
    .await
    .context("process-tree termination task")?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while pid_alive(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

/// Non-Unix fallback: no signal API, so go straight to a hard kill.
#[cfg(not(unix))]
async fn terminate_pid(pid: u32, _grace: Duration) -> Result<()> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, false, ProcessRefreshKind::nothing());
    if let Some(process) = sys.process(sysinfo::Pid::from_u32(pid)) {
        process.kill();
    }
    Ok(())
}

/// Parent-level flags cannot follow a subcommand — `ralph web --dev start`
/// parses with `--dev` on the parent where it would be silently dropped.
/// Catch that with a pointer at the right spelling.
fn ensure_no_parent_flags(run: &RunArgs, subcommand: &str) -> Result<()> {
    if *run == RunArgs::default() {
        return Ok(());
    }
    if subcommand == "start" {
        anyhow::bail!("run flags must come after `start` (e.g. `ralph web {subcommand} --dev`)");
    }
    anyhow::bail!("`ralph web {subcommand}` does not accept run flags");
}

/// Monitor mode (Plan5-U4): spawn the monitor service alone — it
/// serves the built SPA itself, so there is no vite dev server or
/// ralph-api process in the tree.
async fn execute_monitor(run: &RunArgs, workspace_root: &Path, port: u16) -> Result<()> {
    println!("Starting Ralph monitor...");
    if run.frontend_port != RunArgs::default().frontend_port {
        println!(
            "Note: --frontend-port is ignored in monitor mode \
             (the monitor serves the dashboard on --backend-port)."
        );
    }

    let monitor_dir = workspace_root.join("backend/ralph-monitor");
    if !monitor_dir.join("package.json").exists() {
        anyhow::bail!(
            "Monitor package not found at {}",
            monitor_dir.join("package.json").display()
        );
    }
    check_npm_with(OsStr::new("npm"))?;
    check_port_available(port)?;

    let dist_dir = workspace_root.join("frontend/ralph-web/dist");
    let host = run.host.clone().unwrap_or_else(|| "127.0.0.1".to_string());
    let mut monitor_cmd = AsyncCommand::new("npm");
    monitor_cmd
        .args(["run", "start", "-w", "@ralph-monitor/server"])
        .current_dir(workspace_root)
        .env("RALPH_MONITOR_PORT", port.to_string())
        .env("RALPH_MONITOR_HOST", &host)
        .env("RALPH_MONITOR_STATIC_DIR", &dist_dir)
        .env("RALPH_WORKSPACE_ROOT", workspace_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if !is_loopback_host(&host) {
        // Explicit acknowledgement that the auth-less service may bind LAN.
        monitor_cmd.env("RALPH_MONITOR_ALLOW_LAN", "1");
    }
    let mut monitor = monitor_cmd
        .spawn()
        .context("Failed to start monitor server. Is npm installed and deps set up?")?;

    let stdout = monitor.stdout.take().expect("monitor stdout piped");
    let stderr = monitor.stderr.take().expect("monitor stderr piped");
    let ready = std::sync::Arc::new(Notify::new());
    let ready_clone = ready.clone();
    tokio::spawn(async move {
        forward_output(stdout, stderr, "monitor", "listening", ready_clone).await;
    });

    let urls = display_urls_for(&host, port);
    match tokio::time::timeout(READY_TIMEOUT, ready.notified()).await {
        Ok(()) => {
            println!();
            for (label, url) in &urls {
                println!("{label}: {url}");
            }
            println!();
            if !run.no_open {
                let _ = open::that(&urls[0].1);
            }
        }
        Err(_) => {
            println!();
            println!(
                "Warning: monitor did not report ready within {} seconds.",
                READY_TIMEOUT.as_secs()
            );
            for (label, url) in &urls {
                println!("  {label}: {url}");
            }
        }
    }
    println!("Press Ctrl+C to stop");

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let shutdown_tx_sigint = shutdown_tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            println!("\nReceived Ctrl+C, shutting down monitor...");
            let _ = shutdown_tx_sigint.send(true);
        }
    });
    #[cfg(unix)]
    {
        let shutdown_tx_sigterm = shutdown_tx.clone();
        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to register SIGTERM handler");
            sigterm.recv().await;
            println!("\nReceived SIGTERM, shutting down monitor...");
            let _ = shutdown_tx_sigterm.send(true);
        });
    }

    tokio::select! {
        _ = shutdown_rx.changed() => {
            terminate_gracefully(&mut monitor, SHUTDOWN_GRACE_PERIOD).await;
            println!("Monitor stopped.");
        }
        r = monitor.wait() => {
            println!("Monitor exited: {:?}", r);
        }
    }
    Ok(())
}

/// Entry point for `ralph web`. Bare `ralph web` keeps the historical
/// foreground behavior; the subcommands provide lifecycle management.
pub async fn execute(args: WebArgs) -> Result<()> {
    match args.command {
        Some(WebCommand::Status) => {
            ensure_no_parent_flags(&args.run, "status")?;
            status_command(&require_state_dir()?).await
        }
        Some(WebCommand::Stop) => {
            ensure_no_parent_flags(&args.run, "stop")?;
            stop_command(&require_state_dir()?).await
        }
        Some(WebCommand::Start(start)) => {
            ensure_no_parent_flags(&args.run, "start")?;
            start_command(&start.run, start.foreground, &require_state_dir()?).await
        }
        // No subcommand: attached foreground run, as before.
        None => start_command(&args.run, true, &require_state_dir()?).await,
    }
}

fn require_state_dir() -> Result<PathBuf> {
    web_state_dir().context("cannot locate the web state dir ($HOME is unset)")
}

/// Run both backend and frontend dev servers in parallel
async fn execute_dev(run: &RunArgs, workspace_root: &Path) -> Result<()> {
    let backend_port = run.backend_port.unwrap_or(3000);

    println!("Starting Ralph web servers...");

    // Compute absolute paths for backend and frontend directories
    let backend_dir = workspace_root.join("backend/ralph-web-server");
    let frontend_dir = workspace_root.join("frontend/ralph-web");

    if run.legacy_node_api && !backend_dir.join("package.json").exists() {
        anyhow::bail!(
            "Legacy Node backend not found at {}",
            backend_dir.join("package.json").display()
        );
    }

    // Verify frontend Node.js/npm tooling, install deps, and legacy tsx checks when requested.
    preflight(workspace_root, &backend_dir, run.legacy_node_api).await?;

    // Check ports before spawning anything
    check_port_available(backend_port)?;
    check_port_available(run.frontend_port)?;

    println!("Using workspace: {}", workspace_root.display());

    // Spawn backend (default Rust RPC API, optional legacy Node backend) and frontend.
    let backend_label = if run.legacy_node_api {
        "backend"
    } else {
        "api"
    };
    let backend_ready_pattern = if run.legacy_node_api {
        "Server started on"
    } else {
        "ralph-api listening"
    };

    let mut backend = if run.legacy_node_api {
        AsyncCommand::new("npm")
            .args(["run", "dev"])
            .current_dir(&backend_dir)
            .env("RALPH_WORKSPACE_ROOT", workspace_root)
            .env("PORT", backend_port.to_string())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to start legacy backend server. Is npm installed and {} set up?\nError: {}",
                    backend_dir.join("package.json").display(),
                    e
                )
            })?
    } else {
        AsyncCommand::new("cargo")
            .args(["run", "-p", "ralph-api"])
            .current_dir(workspace_root)
            .env("RALPH_API_PORT", backend_port.to_string())
            .env("RALPH_API_WORKSPACE_ROOT", workspace_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to start Rust RPC API (cargo run -p ralph-api).\nError: {}",
                    e
                )
            })?
    };

    // Pass --port for Vite and RALPH_BACKEND_PORT for proxy config. An
    // explicit --host is forwarded to Vite (its config otherwise binds all
    // interfaces via `host: true`).
    let frontend_port = run.frontend_port.to_string();
    let mut vite_args = vec!["run", "dev", "--", "--port", frontend_port.as_str()];
    if let Some(host) = &run.host {
        vite_args.push("--host");
        vite_args.push(host.as_str());
    }
    let mut frontend = AsyncCommand::new("npm")
        .args(&vite_args)
        .current_dir(&frontend_dir)
        .env("RALPH_BACKEND_PORT", backend_port.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to start frontend server. Is npm installed and {} set up?\nError: {}",
                frontend_dir.join("package.json").display(),
                e
            )
        })?;

    // Take ownership of stdout/stderr pipes.
    let backend_stdout = backend.stdout.take().expect("backend stdout piped");
    let backend_stderr = backend.stderr.take().expect("backend stderr piped");
    let frontend_stdout = frontend.stdout.take().expect("frontend stdout piped");
    let frontend_stderr = frontend.stderr.take().expect("frontend stderr piped");

    // Set up ready detection.
    let backend_ready = std::sync::Arc::new(Notify::new());
    let frontend_ready = std::sync::Arc::new(Notify::new());

    // Spawn output forwarding tasks.
    let backend_ready_clone = backend_ready.clone();
    tokio::spawn(async move {
        forward_output(
            backend_stdout,
            backend_stderr,
            backend_label,
            backend_ready_pattern,
            backend_ready_clone,
        )
        .await;
    });

    let frontend_ready_clone = frontend_ready.clone();
    tokio::spawn(async move {
        forward_output(
            frontend_stdout,
            frontend_stderr,
            "frontend",
            "Local:",
            frontend_ready_clone,
        )
        .await;
    });

    // Wait for both servers to become ready. Dev Vite binds all interfaces
    // by default (`host: true`), so the dashboard is LAN-reachable unless
    // --host restricts it.
    let effective_host = run.host.as_deref().unwrap_or("0.0.0.0");
    let urls = display_urls_for(effective_host, run.frontend_port);
    let dashboard_url = urls[0].1.clone();
    let api_url = format!("http://localhost:{}", backend_port);

    let ready_result = tokio::time::timeout(READY_TIMEOUT, async {
        tokio::join!(backend_ready.notified(), frontend_ready.notified());
    })
    .await;

    match ready_result {
        Ok(()) => {
            println!();
            println!("Both servers ready!");
            for (label, url) in &urls {
                println!("  {label}: {url}");
            }
            println!("  api:       {api_url}");
            println!();

            if !run.no_open {
                let _ = open::that(&dashboard_url);
            }
        }
        Err(_) => {
            println!();
            println!(
                "Warning: servers did not report ready within {} seconds.",
                READY_TIMEOUT.as_secs()
            );
            println!("They may still be starting. Check the output above for errors.");
            for (label, url) in &urls {
                println!("  {label}: {url}");
            }
            println!("  api:       {api_url}");
            println!();
        }
    }

    println!("Press Ctrl+C to stop both servers");

    // Set up shutdown channel
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    // Spawn signal handlers
    let shutdown_tx_sigint = shutdown_tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            println!("\nReceived Ctrl+C, shutting down servers...");
            let _ = shutdown_tx_sigint.send(true);
        }
    });

    #[cfg(unix)]
    {
        let shutdown_tx_sigterm = shutdown_tx.clone();
        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to register SIGTERM handler");
            sigterm.recv().await;
            println!("\nReceived SIGTERM, shutting down servers...");
            let _ = shutdown_tx_sigterm.send(true);
        });

        let shutdown_tx_sighup = shutdown_tx.clone();
        tokio::spawn(async move {
            let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .expect("Failed to register SIGHUP handler");
            sighup.recv().await;
            println!("\nReceived SIGHUP (terminal closed), shutting down servers...");
            let _ = shutdown_tx_sighup.send(true);
        });
    }

    // Wait for shutdown signal or server exit
    tokio::select! {
        _ = shutdown_rx.changed() => {
            // Signal received - gracefully terminate both servers
            println!("Stopping backend server...");
            terminate_gracefully(&mut backend, SHUTDOWN_GRACE_PERIOD).await;
            println!("Stopping frontend server...");
            terminate_gracefully(&mut frontend, SHUTDOWN_GRACE_PERIOD).await;
            println!("All servers stopped.");
        }
        r = backend.wait() => {
            println!("Backend exited: {:?}", r);
            // Gracefully terminate frontend on backend exit
            println!("Stopping frontend server...");
            terminate_gracefully(&mut frontend, SHUTDOWN_GRACE_PERIOD).await;
        }
        r = frontend.wait() => {
            println!("Frontend exited: {:?}", r);
            // Gracefully terminate backend on frontend exit
            println!("Stopping backend server...");
            terminate_gracefully(&mut backend, SHUTDOWN_GRACE_PERIOD).await;
        }
    }

    Ok(())
}

/// Gracefully terminate a child process by sending SIGTERM to its whole
/// process tree first, then waiting out the grace period. Tree-wide
/// signalling matters because `npm run` wrappers do not forward signals —
/// a single-pid SIGTERM would orphan the real server grandchildren.
#[cfg(unix)]
async fn terminate_gracefully(child: &mut Child, grace_period: Duration) {
    if let Some(pid) = child.id() {
        tokio::task::spawn_blocking(move || {
            crate::cli::process_tree::kill_process_tree(pid, true);
        })
        .await
        .unwrap_or_else(|err| {
            eprintln!("  process-tree termination failed: {err}");
        });

        // Wait for the direct child to reap; it should already be dead.
        match tokio::time::timeout(grace_period, child.wait()).await {
            Ok(_) => {
                // Process exited
            }
            Err(_) => {
                println!("  Grace period elapsed, forcing termination...");
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
    } else {
        // No PID means process already exited or wasn't started
        let _ = child.wait().await;
    }
}

/// Gracefully terminate a child process (non-Unix fallback using start_kill)
#[cfg(not(unix))]
async fn terminate_gracefully(child: &mut Child, _grace_period: Duration) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::time::Duration;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn write_fake_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        let script = format!("#!/bin/sh\n{}\n", body);
        std::fs::write(&path, script).expect("write script");
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
        path
    }

    #[test]
    fn needs_install_detects_missing_node_modules() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();

        assert!(needs_install(root));

        let node_modules = root.join("node_modules");
        std::fs::create_dir_all(&node_modules).expect("create node_modules");
        std::fs::write(node_modules.join(".package-lock.json"), "").expect("write lock");

        assert!(!needs_install(root));
    }

    #[test]
    fn check_port_available_detects_in_use() {
        match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => {
                let port = listener.local_addr().expect("addr").port();
                assert!(check_port_available(port).is_err());
                drop(listener);

                // Some environments (CI, heavily loaded systems) can take a moment to fully
                // release the port after the listener is dropped. Retry briefly to avoid flakes.
                let mut freed = false;
                for _ in 0..25 {
                    if check_port_available(port).is_ok() {
                        freed = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(
                    freed,
                    "port {port} should become available after listener is dropped"
                );
            }
            Err(err) => {
                // Some sandboxes disallow binding; ensure we handle that path gracefully.
                assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
                assert!(check_port_available(0).is_err());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn check_node_accepts_supported_version() {
        let temp_dir = TempDir::new().expect("temp dir");
        let node_path = write_fake_executable(temp_dir.path(), "node", "echo v18.17.0");
        let version = check_node_with(node_path.as_os_str()).expect("check node");
        assert_eq!(version.trim(), "v18.17.0");
    }

    #[cfg(unix)]
    #[test]
    fn check_node_rejects_old_version() {
        let temp_dir = TempDir::new().expect("temp dir");
        let node_path = write_fake_executable(temp_dir.path(), "node", "echo v16.5.0");
        let err = check_node_with(node_path.as_os_str()).expect_err("expected version error");
        let msg = format!("{err}");
        assert!(msg.contains("too old"), "msg: {msg}");
    }

    #[test]
    fn check_node_reports_missing_binary() {
        let err =
            check_node_with(OsStr::new("definitely-missing-node-12345")).expect_err("missing");
        let msg = format!("{err}");
        assert!(msg.contains("Node.js is not installed"), "msg: {msg}");
    }

    #[cfg(unix)]
    #[test]
    fn check_node_reports_failed_command() {
        let temp_dir = TempDir::new().expect("temp dir");
        let node_path = write_fake_executable(temp_dir.path(), "node", "exit 1");
        let err = check_node_with(node_path.as_os_str()).expect_err("node failure");
        let msg = format!("{err}");
        assert!(
            msg.contains("Failed to run `node --version`")
                || msg.contains("Node.js is not installed"),
            "msg: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn check_npm_reads_version() {
        let temp_dir = TempDir::new().expect("temp dir");
        let npm_path = write_fake_executable(temp_dir.path(), "npm", "echo 9.1.0");
        let version = check_npm_with(npm_path.as_os_str()).expect("check npm");
        assert_eq!(version.trim(), "9.1.0");
    }

    #[cfg(unix)]
    #[test]
    fn check_npm_reports_failed_command() {
        let temp_dir = TempDir::new().expect("temp dir");
        let npm_path = write_fake_executable(temp_dir.path(), "npm", "exit 1");
        let err = check_npm_with(npm_path.as_os_str()).expect_err("npm failure");
        let msg = format!("{err}");
        assert!(
            msg.contains("Failed to run `npm --version`") || msg.contains("npm is not installed"),
            "msg: {msg}"
        );
    }

    #[test]
    fn check_npm_reports_missing_binary() {
        let err = check_npm_with(OsStr::new("definitely-missing-npm-12345")).expect_err("missing");
        let msg = format!("{err}");
        assert!(msg.contains("npm is not installed"), "msg: {msg}");
    }

    #[test]
    fn contains_blocked_tsx_version_detects_supported_formats() {
        assert!(contains_blocked_tsx_version("4.20.0"));
        assert!(contains_blocked_tsx_version("v4.20.0"));
        assert!(contains_blocked_tsx_version("tsx 4.20.0"));
        assert!(contains_blocked_tsx_version("tsx v4.20.0"));
        assert!(contains_blocked_tsx_version("tsx v4.20.0 (node v22.20.0)"));
        assert!(contains_blocked_tsx_version("tsx v4.20.0\nnode v22.20.0"));
    }

    #[test]
    fn contains_blocked_tsx_version_ignores_other_versions() {
        assert!(!contains_blocked_tsx_version("4.21.0"));
        assert!(!contains_blocked_tsx_version("tsx v4.21.0"));
        assert!(!contains_blocked_tsx_version("node v4.20.0"));
    }

    #[cfg(unix)]
    #[test]
    fn check_tsx_version_blocks_known_bad_release() {
        let temp_dir = TempDir::new().expect("temp dir");
        let backend_dir = temp_dir.path().join("server");
        std::fs::create_dir_all(&backend_dir).expect("backend dir");
        let npx_path = write_fake_executable(temp_dir.path(), "npx", "echo 4.20.0");
        let err =
            check_tsx_version_with(&backend_dir, npx_path.as_os_str()).expect_err("tsx error");
        let msg = format!("{err}");
        assert!(msg.contains("tsx 4.20.0"), "msg: {msg}");
    }

    #[cfg(unix)]
    #[test]
    fn check_tsx_version_blocks_known_bad_release_with_v_prefix() {
        let temp_dir = TempDir::new().expect("temp dir");
        let backend_dir = temp_dir.path().join("server");
        std::fs::create_dir_all(&backend_dir).expect("backend dir");
        let npx_path = write_fake_executable(temp_dir.path(), "npx", "echo v4.20.0");
        let err =
            check_tsx_version_with(&backend_dir, npx_path.as_os_str()).expect_err("tsx error");
        let msg = format!("{err}");
        assert!(msg.contains("tsx 4.20.0"), "msg: {msg}");
    }

    #[cfg(unix)]
    #[test]
    fn check_tsx_version_blocks_known_bad_release_with_label() {
        let temp_dir = TempDir::new().expect("temp dir");
        let backend_dir = temp_dir.path().join("server");
        std::fs::create_dir_all(&backend_dir).expect("backend dir");
        let npx_path = write_fake_executable(temp_dir.path(), "npx", "echo tsx v4.20.0");
        let err =
            check_tsx_version_with(&backend_dir, npx_path.as_os_str()).expect_err("tsx error");
        let msg = format!("{err}");
        assert!(msg.contains("tsx 4.20.0"), "msg: {msg}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preflight_runs_install_with_fake_tools() {
        let temp_dir = TempDir::new().expect("temp dir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");

        let node_path = write_fake_executable(&bin_dir, "node", "echo v20.1.0");
        let npx_path = write_fake_executable(&bin_dir, "npx", "echo 4.21.0");
        let npm_path = write_fake_executable(
            &bin_dir,
            "npm",
            "if [ \"$1\" = \"--version\" ]; then echo 9.6.0; else touch npm_install_called; fi",
        );

        let root = temp_dir.path().join("workspace");
        let backend_dir = root.join("server");
        std::fs::create_dir_all(&backend_dir).expect("backend dir");

        // Retrying avoids rare CI flakes where executing freshly written
        // fake scripts can transiently fail (e.g. ETXTBSY surfaced as
        // "Node.js is not installed").
        let mut preflight_ok = false;
        for _ in 0..5 {
            match preflight_with(
                &root,
                &backend_dir,
                true,
                node_path.as_os_str(),
                npm_path.as_os_str(),
                npx_path.as_os_str(),
            )
            .await
            {
                Ok(()) => {
                    preflight_ok = true;
                    break;
                }
                Err(err) if format!("{err}").contains("Node.js is not installed") => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(err) => panic!("preflight: {err}"),
            }
        }
        assert!(preflight_ok, "preflight should succeed after retries");
        assert!(root.join("npm_install_called").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preflight_skips_install_when_node_modules_present() {
        let temp_dir = TempDir::new().expect("temp dir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");

        let node_path = write_fake_executable(&bin_dir, "node", "echo v20.1.0");
        let npx_path = write_fake_executable(&bin_dir, "npx", "echo 4.21.0");
        let npm_path = write_fake_executable(
            &bin_dir,
            "npm",
            "if [ \"$1\" = \"--version\" ]; then echo 9.6.0; exit 0; fi\n\
if [ \"$1\" = \"ci\" ] || [ \"$1\" = \"install\" ]; then touch npm_install_called; exit 0; fi\n\
exit 1",
        );

        let root = temp_dir.path().join("workspace");
        let backend_dir = root.join("server");
        std::fs::create_dir_all(&backend_dir).expect("backend dir");
        std::fs::create_dir_all(root.join("node_modules")).expect("node_modules dir");
        std::fs::write(root.join("node_modules/.package-lock.json"), "").expect("lockfile");

        let mut preflight_ok = false;
        for _ in 0..5 {
            match preflight_with(
                &root,
                &backend_dir,
                true,
                node_path.as_os_str(),
                npm_path.as_os_str(),
                npx_path.as_os_str(),
            )
            .await
            {
                Ok(()) => {
                    preflight_ok = true;
                    break;
                }
                Err(err) if format!("{err}").contains("Node.js is not installed") => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(err) => panic!("preflight: {err}"),
            }
        }
        assert!(preflight_ok, "preflight should succeed after retries");
        assert!(!root.join("npm_install_called").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preflight_reports_bad_tsx_version() {
        let temp_dir = TempDir::new().expect("temp dir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");

        let node_path = write_fake_executable(&bin_dir, "node", "echo v20.1.0");
        let npx_path = write_fake_executable(&bin_dir, "npx", "echo 4.20.0");
        let npm_path = write_fake_executable(
            &bin_dir,
            "npm",
            "if [ \"$1\" = \"--version\" ]; then echo 9.6.0; exit 0; fi",
        );

        let root = temp_dir.path().join("workspace");
        let backend_dir = root.join("server");
        std::fs::create_dir_all(&backend_dir).expect("backend dir");
        std::fs::create_dir_all(root.join("node_modules")).expect("node_modules dir");
        std::fs::write(root.join("node_modules/.package-lock.json"), "").expect("lockfile");

        let err = preflight_with(
            &root,
            &backend_dir,
            true,
            node_path.as_os_str(),
            npm_path.as_os_str(),
            npx_path.as_os_str(),
        )
        .await
        .expect_err("preflight should fail on bad tsx");
        let msg = format!("{err}");
        assert!(msg.contains("tsx 4.20.0"), "msg: {msg}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_npm_install_uses_ci_with_lockfile() {
        let temp_dir = TempDir::new().expect("temp dir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");

        let root = temp_dir.path().join("workspace");
        std::fs::create_dir_all(&root).expect("workspace dir");
        std::fs::write(root.join("package-lock.json"), "{}").expect("lockfile");

        let npm_path = write_fake_executable(&bin_dir, "npm", r#"echo "$1" > "$PWD/command.txt""#);

        run_npm_install_with(&root, npm_path.as_os_str())
            .await
            .expect("npm ci");

        let command = std::fs::read_to_string(root.join("command.txt")).expect("read command");
        assert_eq!(command.trim(), "ci");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_npm_install_uses_install_without_lockfile() {
        let temp_dir = TempDir::new().expect("temp dir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");

        let root = temp_dir.path().join("workspace");
        std::fs::create_dir_all(&root).expect("workspace dir");

        let npm_path = write_fake_executable(&bin_dir, "npm", r#"echo "$1" > "$PWD/command.txt""#);

        run_npm_install_with(&root, npm_path.as_os_str())
            .await
            .expect("npm install");

        let command = std::fs::read_to_string(root.join("command.txt")).expect("read command");
        assert_eq!(command.trim(), "install");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_npm_install_reports_failure() {
        let temp_dir = TempDir::new().expect("temp dir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");

        let root = temp_dir.path().join("workspace");
        std::fs::create_dir_all(&root).expect("workspace dir");

        let npm_path = write_fake_executable(&bin_dir, "npm", r#"echo "boom" 1>&2; exit 1"#);

        let err = run_npm_install_with(&root, npm_path.as_os_str())
            .await
            .expect_err("npm install failure");
        let msg = format!("{err}");
        assert!(
            msg.contains("npm install failed") || msg.contains("Failed to run npm install"),
            "msg: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn check_tsx_version_with_missing_binary_is_ok() {
        let temp_dir = TempDir::new().expect("temp dir");
        let backend_dir = temp_dir.path().join("server");
        std::fs::create_dir_all(&backend_dir).expect("backend dir");

        assert!(check_tsx_version_with(&backend_dir, OsStr::new("missing-npx-123")).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn forward_output_notifies_on_stdout_pattern() {
        let mut child = AsyncCommand::new("sh")
            .arg("-c")
            .arg("echo READY")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn sh");

        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let ready = std::sync::Arc::new(Notify::new());
        let ready_clone = ready.clone();

        let forward_task = tokio::spawn(async move {
            forward_output(stdout, stderr, "test", "READY", ready_clone).await;
        });

        tokio::time::timeout(Duration::from_secs(2), ready.notified())
            .await
            .expect("ready notification");

        let _ = child.wait().await;
        forward_task.await.expect("forward task");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn forward_output_notifies_on_stderr_pattern() {
        let mut child = AsyncCommand::new("sh")
            .arg("-c")
            .arg("echo READY 1>&2")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn sh");

        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let ready = std::sync::Arc::new(Notify::new());
        let ready_clone = ready.clone();

        let forward_task = tokio::spawn(async move {
            forward_output(stdout, stderr, "test", "READY", ready_clone).await;
        });

        tokio::time::timeout(Duration::from_secs(2), ready.notified())
            .await
            .expect("ready notification");

        let _ = child.wait().await;
        forward_task.await.expect("forward task");
    }

    #[tokio::test]
    async fn execute_invalid_workspace_returns_error() {
        let temp_dir = TempDir::new().expect("temp dir");
        let missing = temp_dir.path().join("missing");
        let args = WebArgs {
            command: None,
            run: RunArgs {
                backend_port: Some(3000),
                workspace: Some(missing),
                dev: true,
                no_open: true,
                ..RunArgs::default()
            },
        };

        let err = execute(args).await.expect_err("invalid workspace");
        assert!(err.to_string().contains("Invalid workspace path"));
    }

    // -----------------------------------------------------------------
    // Lifecycle state file + subcommands
    // -----------------------------------------------------------------

    fn test_state(dir: &Path, pid: u32) -> WebState {
        WebState {
            pid,
            mode: WebMode::Monitor,
            host: "127.0.0.1".to_string(),
            backend_port: 3200,
            frontend_port: 5173,
            workspace: dir.to_path_buf(),
            started: Utc::now(),
        }
    }

    #[test]
    fn web_args_parse_host_flag() {
        let args = WebArgs::try_parse_from(["ralph-web", "--host", "0.0.0.0"]).expect("parse");
        assert_eq!(args.run.host.as_deref(), Some("0.0.0.0"));

        let args = WebArgs::try_parse_from([
            "ralph-web",
            "start",
            "--foreground",
            "--host",
            "192.168.1.10",
        ])
        .expect("parse");
        match args.command {
            Some(WebCommand::Start(start)) => {
                assert_eq!(start.run.host.as_deref(), Some("192.168.1.10"));
            }
            other => panic!("expected start subcommand, got {other:?}"),
        }
    }

    #[test]
    fn loopback_host_detection() {
        for host in ["127.0.0.1", "localhost", "::1", "[::1]", "LOCALHOST"] {
            assert!(is_loopback_host(host), "{host} should be loopback");
        }
        for host in ["0.0.0.0", "192.168.1.10", "10.0.0.5", "::", "example.com"] {
            assert!(!is_loopback_host(host), "{host} should not be loopback");
        }
    }

    #[test]
    fn display_urls_loopback_shows_single_dashboard() {
        let urls = display_urls_for("127.0.0.1", 3200);
        assert_eq!(
            urls,
            vec![("dashboard", "http://localhost:3200".to_string())]
        );
        let urls = display_urls_for("localhost", 5173);
        assert_eq!(
            urls,
            vec![("dashboard", "http://localhost:5173".to_string())]
        );
    }

    #[test]
    fn display_urls_wildcard_shows_local_and_lan() {
        let urls = display_urls_for("0.0.0.0", 3200);
        assert_eq!(urls[0], ("local", "http://localhost:3200".to_string()));
        // A LAN line appears only when the box actually has a non-loopback
        // IPv4 — CI sandboxes may not, so assert shape, not the IP itself.
        if urls.len() == 2 {
            assert_eq!(urls[1].0, "lan");
            assert!(urls[1].1.starts_with("http://"));
            assert!(urls[1].1.ends_with(":3200"));
        }
    }

    #[test]
    fn lan_ipv4_score_prefers_rfc1918() {
        use std::net::Ipv4Addr;
        let home: Ipv4Addr = "192.168.1.10".parse().unwrap();
        let corp: Ipv4Addr = "10.0.0.5".parse().unwrap();
        let priv172: Ipv4Addr = "172.16.0.2".parse().unwrap();
        let tailscale: Ipv4Addr = "100.80.192.75".parse().unwrap();
        let public: Ipv4Addr = "8.8.8.8".parse().unwrap();
        assert!(lan_ipv4_score(&home) > lan_ipv4_score(&corp));
        assert!(lan_ipv4_score(&corp) > lan_ipv4_score(&priv172));
        assert!(lan_ipv4_score(&priv172) > lan_ipv4_score(&tailscale));
        assert_eq!(lan_ipv4_score(&tailscale), lan_ipv4_score(&public));
    }

    #[test]
    fn display_urls_specific_address_shows_it() {
        let urls = display_urls_for("192.168.1.10", 3200);
        assert_eq!(
            urls,
            vec![("dashboard", "http://192.168.1.10:3200".to_string())]
        );
    }

    #[test]
    fn state_without_host_defaults_to_loopback() {
        // State files written before host tracking existed have no `host`
        // field — serde must default them to loopback.
        let json = r#"{
            "pid": 1234,
            "mode": "monitor",
            "backend_port": 3200,
            "frontend_port": 5173,
            "workspace": "/tmp/ws",
            "started": "2026-01-01T00:00:00Z"
        }"#;
        let state: WebState = serde_json::from_str(json).expect("parse legacy state");
        assert_eq!(state.host, "127.0.0.1");
        assert_eq!(
            display_urls(&state),
            vec![("dashboard", "http://localhost:3200".to_string())]
        );
    }

    #[test]
    fn web_args_parse_bare_flags() {
        let args = WebArgs::try_parse_from(["ralph-web", "--dev", "--no-open"]).expect("parse");
        assert!(args.command.is_none());
        assert!(args.run.dev);
        assert!(args.run.no_open);
    }

    #[test]
    fn web_args_parse_start_with_flags() {
        let args = WebArgs::try_parse_from([
            "ralph-web",
            "start",
            "--dev",
            "--backend-port",
            "4321",
            "--foreground",
        ])
        .expect("parse");
        match args.command {
            Some(WebCommand::Start(start)) => {
                assert!(start.foreground);
                assert!(start.run.dev);
                assert_eq!(start.run.backend_port, Some(4321));
            }
            other => panic!("expected start subcommand, got {other:?}"),
        }
    }

    #[test]
    fn web_args_parse_status_and_stop() {
        let args = WebArgs::try_parse_from(["ralph-web", "status"]).expect("parse");
        assert!(matches!(args.command, Some(WebCommand::Status)));
        let args = WebArgs::try_parse_from(["ralph-web", "stop"]).expect("parse");
        assert!(matches!(args.command, Some(WebCommand::Stop)));
    }

    #[test]
    fn ensure_no_parent_flags_accepts_default() {
        assert!(ensure_no_parent_flags(&RunArgs::default(), "start").is_ok());
    }

    #[test]
    fn ensure_no_parent_flags_rejects_flags_before_subcommand() {
        let mut run = RunArgs::default();
        run.dev = true;
        let err = ensure_no_parent_flags(&run, "start").expect_err("should reject");
        assert!(err.to_string().contains("ralph web start"), "{err}");
        let err = ensure_no_parent_flags(&run, "status").expect_err("should reject");
        assert!(err.to_string().contains("status"), "{err}");
    }

    #[test]
    fn inspect_state_absent_without_file() {
        let temp_dir = TempDir::new().expect("temp dir");
        assert!(matches!(inspect_state(temp_dir.path()), StateProbe::Absent));
    }

    #[test]
    fn inspect_state_live_for_own_pid() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_state(
            temp_dir.path(),
            &test_state(temp_dir.path(), std::process::id()),
        )
        .expect("write state");
        match inspect_state(temp_dir.path()) {
            StateProbe::Live(state) => assert_eq!(state.pid, std::process::id()),
            other => panic!("expected live state, got {other:?}"),
        }
    }

    #[test]
    fn inspect_state_removes_dead_pid() {
        let temp_dir = TempDir::new().expect("temp dir");
        // u32::MAX is never a live pid.
        write_state(temp_dir.path(), &test_state(temp_dir.path(), u32::MAX)).expect("write state");
        assert!(matches!(inspect_state(temp_dir.path()), StateProbe::Stale));
        assert!(!state_file_path(temp_dir.path()).exists());
    }

    #[test]
    fn inspect_state_removes_unparseable_file() {
        let temp_dir = TempDir::new().expect("temp dir");
        std::fs::write(state_file_path(temp_dir.path()), "not json").expect("write");
        assert!(matches!(inspect_state(temp_dir.path()), StateProbe::Stale));
        assert!(!state_file_path(temp_dir.path()).exists());
    }

    #[test]
    fn state_guard_removes_own_state_on_drop() {
        let temp_dir = TempDir::new().expect("temp dir");
        let pid = std::process::id();
        write_state(temp_dir.path(), &test_state(temp_dir.path(), pid)).expect("write state");
        {
            let _guard = StateGuard::new(temp_dir.path(), pid);
        }
        assert!(!state_file_path(temp_dir.path()).exists());
    }

    #[test]
    fn state_guard_keeps_other_pid_state() {
        let temp_dir = TempDir::new().expect("temp dir");
        let pid = std::process::id();
        write_state(temp_dir.path(), &test_state(temp_dir.path(), pid)).expect("write state");
        {
            // A guard from an older (already-exited) supervisor must not
            // delete the newer instance's record.
            let _guard = StateGuard::new(temp_dir.path(), pid.wrapping_add(1));
        }
        assert!(state_file_path(temp_dir.path()).exists());
    }

    #[test]
    fn daemon_argv_forwards_run_flags() {
        let run = RunArgs {
            host: Some("0.0.0.0".to_string()),
            backend_port: Some(4321),
            frontend_port: 6000,
            dev: true,
            legacy_node_api: true,
            ..RunArgs::default()
        };
        let root = Path::new("/ws");
        let argv = daemon_argv(&run, root);
        let argv: Vec<String> = argv
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            argv,
            vec![
                "web",
                "--workspace",
                "/ws",
                "--no-open",
                "--host",
                "0.0.0.0",
                "--backend-port",
                "4321",
                "--frontend-port",
                "6000",
                "--dev",
                "--legacy-node-api"
            ]
        );
    }

    #[test]
    fn daemon_argv_omits_defaults() {
        let run = RunArgs::default();
        let argv = daemon_argv(&run, Path::new("/ws"));
        let argv: Vec<String> = argv
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, vec!["web", "--workspace", "/ws", "--no-open"]);
    }

    #[cfg(unix)]
    #[test]
    fn pid_alive_self_and_dead() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(u32::MAX));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminate_pid_stops_process() {
        // A supervised-style child: `sleep` just sits there like a server.
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();

        terminate_pid(pid, Duration::from_secs(5))
            .await
            .expect("terminate");

        // Reap so the pid is fully gone for the liveness check.
        let _ = child.wait();
        assert!(!pid_alive(pid));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_command_terminates_recorded_pid() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep");
        write_state(temp_dir.path(), &test_state(temp_dir.path(), child.id()))
            .expect("write state");

        stop_command(temp_dir.path()).await.expect("stop");

        let _ = child.wait();
        assert!(!pid_alive(child.id()));
        assert!(!state_file_path(temp_dir.path()).exists());
    }

    #[tokio::test]
    async fn stop_command_succeeds_when_not_running() {
        let temp_dir = TempDir::new().expect("temp dir");
        // No state file and nothing on the default monitor port (usually).
        // If a real monitor happens to answer on :3200 this test would
        // see the unmanaged branch — still an error, so only assert in
        // the quiet case by skipping when the probe hits.
        if probe_monitor_health(DEFAULT_MONITOR_PORT).await.is_some() {
            return; // real monitor present; nothing meaningful to assert here
        }
        stop_command(temp_dir.path()).await.expect("stop no-op");
    }

    #[tokio::test]
    async fn start_command_reports_already_running() {
        let temp_dir = TempDir::new().expect("temp dir");
        let state_dir = TempDir::new().expect("state dir");
        write_state(
            state_dir.path(),
            &test_state(temp_dir.path(), std::process::id()),
        )
        .expect("write state");

        let run = RunArgs {
            workspace: Some(temp_dir.path().to_path_buf()),
            no_open: true,
            ..RunArgs::default()
        };
        // Live state short-circuits before any spawn — returns Ok.
        start_command(&run, true, state_dir.path())
            .await
            .expect("already-running report");
    }
}
