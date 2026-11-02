//! `cargo xtask` — cross-platform repo task runner.
//!
//! `cargo xtask test` mirrors `scripts/run-tests.sh`: env scrub,
//! per-worktree target dir, cross-worktree full-suite lock, two-phase
//! nextest (parallel + serial race-sensitive), doctest crate filtering,
//! and the `RALPH_BASELINE_SERIAL=1` cargo-test fallback.

use std::collections::HashSet;
use std::env;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// Agent-context env vars that must never leak into spawned test
/// binaries (same list as `run-tests.sh` + `tests/common/mod.rs`).
const AGENT_CONTEXT_ENV_KEYS: &[&str] = &[
    "RALPH_CURRENT_HAT",
    "RALPH_CURRENT_LOOP_ID",
    "RALPH_EVENTS_FILE",
    "RALPH_WAVE_WORKER",
    "RALPH_WAVE_ID",
    "RALPH_WAVE_INDEX",
    "RALPH_WAVE_TOTAL",
    "RALPH_TRIGGERED_HAT",
    "RALPH_HATS_SOURCE",
    "RALPH_CONFIG",
    "RALPH_CURRENT_BRANCH",
    "RALPH_WORKSPACE_ROOT",
    "RALPH_LOOP_ITERATION",
    "RALPH_DAG_STORE_PATH",
];

const PROXY_ENV_KEYS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "http_proxy",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// Race-sensitive tests routed to the serial phase (same filter as
/// `run-tests.sh`'s `SERIAL_TEST_FILTER`).
const SERIAL_TEST_FILTER: &str =
    "test(/partial_timeout_events_visible/) or test(/test_execute_wave_/) or test(/wave_supervisor::/)";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("test") => test_cmd(&args[1..]),
        Some("help") | None => {
            usage();
            Ok(())
        }
        Some(other) => {
            usage();
            bail!("unknown subcommand: {other}")
        }
    }
}

fn usage() {
    println!(
        "cargo xtask — repo task runner\n\n\
         USAGE:\n    cargo xtask test [--skip-fallback]\n\n\
         ENV:\n    RALPH_BASELINE_SERIAL=1   force single-thread cargo test (skip nextest)\n\
         \x20   RALPH_NEXTTEST_JOBS=N       phase-1 concurrency (default min(cpus,4))\n\
         \x20   RALPH_CARGO_TOOLCHAIN=T    run cargo via `rustup run T cargo`\n"
    );
}

fn repo_root() -> Result<PathBuf> {
    // xtask is built+run by cargo from the workspace root.
    let dir = env::current_dir().context("current_dir")?;
    if dir.join("Cargo.toml").exists() && dir.join("crates").is_dir() {
        Ok(dir)
    } else {
        bail!(
            "run `cargo xtask` from the workspace root (got {})",
            dir.display()
        )
    }
}

fn test_cmd(args: &[String]) -> Result<()> {
    let mut skip_fallback = false;
    for arg in args {
        match arg.as_str() {
            "--skip-fallback" => skip_fallback = true,
            "--help" | "-h" => {
                usage();
                return Ok(());
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    let root = repo_root()?;
    env::set_current_dir(&root)?;

    // Pin per-worktree target dir so a leaked global CARGO_TARGET_DIR
    // cannot merge two worktrees into one lock-contended dir.
    env::set_var("CARGO_TARGET_DIR", root.join("target"));

    // Scrub proxy + agent-context env so spawned binaries see a clean
    // "human shell" environment.
    for key in PROXY_ENV_KEYS.iter().chain(AGENT_CONTEXT_ENV_KEYS) {
        env::remove_var(key);
    }

    let jobs = env::var("RALPH_NEXTTEST_JOBS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get().min(4)))
        .max(1);

    let _lock = FullTestLock::acquire(&root)?;

    let serial = env::var("RALPH_BASELINE_SERIAL").ok().as_deref() == Some("1");
    let mut failed = false;

    if !serial && cargo_has_subcommand("nextest")? {
        eprintln!("Phase 1: full workspace at {jobs} concurrent jobs...");
        if !run_streamed(
            "cargo",
            &[
                OsStr::new("nextest"),
                OsStr::new("run"),
                OsStr::new("--workspace"),
                OsStr::new("--exclude"),
                OsStr::new("ralph-e2e"),
                OsStr::new("-j"),
                OsStr::new(&jobs.to_string()),
                OsStr::new("-E"),
                OsStr::new(&format!("not ({SERIAL_TEST_FILTER})")),
            ],
        )? {
            failed = true;
        }

        eprintln!("Phase 2: race-sensitive tests at -j 1...");
        if !run_streamed(
            "cargo",
            &[
                OsStr::new("nextest"),
                OsStr::new("run"),
                OsStr::new("--workspace"),
                OsStr::new("--exclude"),
                OsStr::new("ralph-e2e"),
                OsStr::new("-j"),
                OsStr::new("1"),
                OsStr::new("-E"),
                OsStr::new(SERIAL_TEST_FILTER),
            ],
        )? {
            failed = true;
        }

        // Doctest: only crates that actually contain ```rust blocks
        // (rustdoc still runs the full pipeline on empty crates).
        let doc_crates = crates_with_doctests(&root)?;
        if doc_crates.is_empty() {
            eprintln!("Skipping doctest: no crates contain ```rust blocks");
        } else {
            for crate_name in &doc_crates {
                if !run_streamed("cargo", &["test", "-p", crate_name, "--doc"])? {
                    failed = true;
                }
            }
        }
    } else if serial {
        eprintln!("RALPH_BASELINE_SERIAL=1: single-thread cargo test fallback...");
        if !run_streamed(
            "cargo",
            &[
                "test",
                "--workspace",
                "--exclude",
                "ralph-e2e",
                "--",
                "--test-threads=1",
            ],
        )? {
            failed = true;
        }
        bail_if(failed)?;
    } else if skip_fallback {
        bail!("cargo-nextest not installed and --skip-fallback set (install: cargo install cargo-nextest --locked)");
    } else {
        eprintln!("cargo-nextest unavailable; falling back to single-thread cargo test...");
        if !run_streamed(
            "cargo",
            &[
                "test",
                "--workspace",
                "--exclude",
                "ralph-e2e",
                "--",
                "--test-threads=1",
            ],
        )? {
            failed = true;
        }
    }

    bail_if(failed)
}

fn bail_if(failed: bool) -> Result<()> {
    if failed {
        bail!("test run had failures");
    }
    Ok(())
}

fn cargo_has_subcommand(name: &str) -> Result<bool> {
    let status = Command::new("cargo")
        .arg(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(matches!(status, Ok(s) if s.success()))
}

/// Stream a child process's stdout+stderr merged to our own stderr/stdout
/// while inheriting exit status. Returns `Ok(true)` on exit code 0.
fn run_streamed<S: AsRef<OsStr>>(program: &str, args: &[S]) -> Result<bool> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    // Honor RALPH_CARGO_TOOLCHAIN like run-tests.sh's run_cargo().
    let mut cmd = if let Ok(tc) = env::var("RALPH_CARGO_TOOLCHAIN") {
        let mut c = Command::new("rustup");
        c.args(["run", &tc, program]);
        c.args(args);
        c
    } else {
        cmd
    };
    let mut child = cmd
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn {program}"))?;
    Ok(child.wait()?.success())
}

/// Find workspace crates under `crates/*/src` containing a ```rust
/// fenced block (the only crates worth running `cargo test --doc` on).
fn crates_with_doctests(root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))? {
        let entry = entry?;
        let src = entry.path().join("src");
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "ralph-e2e" || !src.is_dir() {
            continue;
        }
        if dir_contains_rust_fence(&src)? {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

fn dir_contains_rust_fence(dir: &Path) -> Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if dir_contains_rust_fence(&path)? {
                return Ok(true);
            }
        } else if path.extension() == Some(OsStr::new("rs")) && file_contains_rust_fence(&path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn file_contains_rust_fence(path: &Path) -> Result<bool> {
    let file = std::fs::File::open(path)?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim_start().starts_with("```rust") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Cross-worktree full-suite lock inside the shared git common dir.
/// Mirrors `acquire_full_test_lock`: mkdir-based spin lock with a pid
/// file; stale locks are reaped when the owning pid is gone.
struct FullTestLock {
    dir: PathBuf,
    pid_file: PathBuf,
    owned: bool,
}

impl FullTestLock {
    fn acquire(root: &Path) -> Result<Self> {
        let common = git_common_dir(root)?;
        let dir = common.join("ralph-full-tests.lock");
        let pid_file = dir.join("pid");
        let mut lock = Self {
            dir: dir.clone(),
            pid_file: pid_file.clone(),
            owned: false,
        };
        loop {
            match std::fs::create_dir(&dir) {
                Ok(()) => {
                    std::fs::write(&pid_file, std::process::id().to_string())?;
                    std::fs::write(dir.join("worktree"), root.display().to_string())?;
                    lock.owned = true;
                    return Ok(lock);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock.stale_owner_gone() {
                        let _ = std::fs::remove_file(&pid_file);
                        let _ = std::fs::remove_file(dir.join("worktree"));
                        let _ = std::fs::remove_dir(&dir);
                    } else {
                        std::thread::sleep(Duration::from_secs(5));
                    }
                }
                Err(e) => return Err(e).context("create full-test lock dir"),
            }
        }
    }

    /// True when the pid file is missing/undecodable (owner died without
    /// cleanup) or names a dead process.
    fn stale_owner_gone(&self) -> bool {
        let Ok(text) = std::fs::read_to_string(&self.pid_file) else {
            // pid file missing but worktree file exists → post-Ctrl+C residue.
            return self.dir.join("worktree").exists();
        };
        let Ok(pid) = text.trim().parse::<u32>() else {
            return true;
        };
        !pid_alive(pid)
    }
}

impl Drop for FullTestLock {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_file(&self.pid_file);
            let _ = std::fs::remove_file(self.dir.join("worktree"));
            let _ = std::fs::remove_dir(&self.dir);
        }
    }
}

fn git_common_dir(root: &Path) -> Result<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(root)
        .output()
        .context("git rev-parse --git-common-dir")?;
    if !out.status.success() {
        bail!("git rev-parse --git-common-dir failed");
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let path = PathBuf::from(raw);
    let abs = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    Ok(abs.canonicalize().unwrap_or(abs))
}

/// Cross-platform PID liveness (sysinfo covers both unix + windows).
fn pid_alive(pid: u32) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    let target = sysinfo::Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[target]),
        false,
        ProcessRefreshKind::nothing(),
    );
    sys.process(target).is_some()
}

#[allow(dead_code)]
fn _unused(_: HashSet<()>) {}
