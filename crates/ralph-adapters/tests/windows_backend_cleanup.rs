//! Integration tests: Backend process cleanup on Windows (and cross-platform regression).
//!
//! Verifies that ACP and CLI backends properly clean up child processes after execution,
//! with no orphan processes left behind. Uses the ralph-core platform abstraction for
//! cross-platform process detection.
//!
//! Run with: cargo test -p ralph-adapters --test windows_backend_cleanup

use std::fs;
use std::path::Path;

use ralph_adapters::{
    AcpExecutor, CliBackend, OutputFormat, PromptMode, SessionResult, StreamHandler,
};
use tempfile::TempDir;
use tokio::time::timeout;

const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Minimal handler that just collects text.
#[derive(Default)]
struct NullHandler;

impl StreamHandler for NullHandler {
    fn on_text(&mut self, _: &str) {}
    fn on_tool_call(&mut self, _: &str, _: &str, _: &serde_json::Value) {}
    fn on_tool_result(&mut self, _: &str, _: &str) {}
    fn on_error(&mut self, _: &str) {}
    fn on_complete(&mut self, _: &SessionResult) {}
}

/// Check if a process is alive using the platform abstraction.
fn pid_alive(pid: u32) -> bool {
    ralph_core::platform::process_exists(pid)
}

/// Create a mock ACP script for the current platform.
/// Spawns a child process (simulating an MCP server) and records PIDs.
#[cfg(unix)]
fn create_mock_acp_script(dir: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("mock-kiro-acp.sh");
    let pid_file = dir.join("pids.txt");

    let script = format!(
        r#"#!/usr/bin/env bash
# Spawn a child that simulates an MCP server (long-lived).
sleep 300 </dev/null >/dev/null 2>&1 &
CHILD_PID=$!

# Record PIDs so the test can check them
echo "$$:$CHILD_PID" >> {pid_file}
"#,
        pid_file = pid_file.display()
    );

    fs::write(&script_path, &script).unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

    script_path.to_string_lossy().to_string()
}

/// Create a mock ACP script for Windows.
/// Uses PowerShell to spawn a background process and record PIDs.
#[cfg(windows)]
fn create_mock_acp_script(dir: &Path) -> String {
    let script_path = dir.join("mock-kiro-acp.ps1");
    let pid_file = dir.join("pids.txt");

    let script = format!(
        r#"# Spawn a child that simulates an MCP server (long-lived)
$child = Start-Process -FilePath "powershell" -ArgumentList "-Command Start-Sleep -Seconds 300" -PassThru -WindowStyle Hidden

# Record PIDs so the test can check them
"$($PID):$($child.Id)" | Out-File -FilePath "{pid_file}" -Append
"#,
        pid_file = pid_file.display()
    );

    fs::write(&script_path, &script).unwrap();

    // Return command to run PowerShell with the script
    "powershell".to_string()
}

/// Get script arguments for the mock script.
#[cfg(unix)]
fn get_mock_script_args(dir: &Path) -> Vec<String> {
    vec![dir.join("mock-kiro-acp.sh").to_string_lossy().to_string()]
}

#[cfg(windows)]
fn get_mock_script_args(dir: &Path) -> Vec<String> {
    vec![
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        dir.join("mock-kiro-acp.ps1").to_string_lossy().to_string(),
    ]
}

/// Parse recorded PIDs from the PID file.
fn parse_pids(dir: &Path) -> Vec<(u32, u32)> {
    let pid_file = dir.join("pids.txt");
    if !pid_file.exists() {
        return vec![];
    }
    fs::read_to_string(&pid_file)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.split(':').collect();
            (
                parts[0].parse::<u32>().unwrap(),
                parts[1].parse::<u32>().unwrap(),
            )
        })
        .collect()
}

/// After AcpExecutor::execute() returns, the direct child process should be dead.
#[tokio::test]
async fn acp_kills_child_process_on_completion() {
    let temp_dir = TempDir::new().unwrap();
    let _script = create_mock_acp_script(temp_dir.path());

    let backend = CliBackend {
        #[cfg(unix)]
        command: temp_dir.path().join("mock-kiro-acp.sh").to_string_lossy().to_string(),
        #[cfg(windows)]
        command: "powershell".to_string(),
        args: get_mock_script_args(temp_dir.path()),
        prompt_mode: PromptMode::Stdin,
        prompt_flag: None,
        output_format: OutputFormat::Acp,
        env_vars: vec![],
    };

    let executor = AcpExecutor::new(backend, temp_dir.path().to_path_buf());
    let mut handler = NullHandler;

    // This will fail at ACP protocol level (mock doesn't speak JSON-RPC),
    // but the process lifecycle (spawn + cleanup) still executes.
    let _ = timeout(TEST_TIMEOUT, executor.execute("test prompt", &mut handler))
        .await
        .expect("execute() hung - deadlock in mock script?");

    // Give processes a moment to be reaped
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let pids = parse_pids(temp_dir.path());
    assert!(!pids.is_empty(), "Mock script should have recorded PIDs");

    for (parent_pid, _child_pid) in &pids {
        assert!(
            !pid_alive(*parent_pid),
            "Parent process {} should be dead after execute() returns",
            parent_pid
        );
    }
}

/// The grandchild process (simulated MCP server) should also be dead after cleanup.
/// This is the actual orphan leak - the direct child dies but its children survive.
#[tokio::test]
async fn acp_kills_grandchild_processes_no_orphans() {
    let temp_dir = TempDir::new().unwrap();
    let _script = create_mock_acp_script(temp_dir.path());

    let backend = CliBackend {
        #[cfg(unix)]
        command: temp_dir.path().join("mock-kiro-acp.sh").to_string_lossy().to_string(),
        #[cfg(windows)]
        command: "powershell".to_string(),
        args: get_mock_script_args(temp_dir.path()),
        prompt_mode: PromptMode::Stdin,
        prompt_flag: None,
        output_format: OutputFormat::Acp,
        env_vars: vec![],
    };

    let executor = AcpExecutor::new(backend, temp_dir.path().to_path_buf());
    let mut handler = NullHandler;

    let _ = timeout(TEST_TIMEOUT, executor.execute("test prompt", &mut handler))
        .await
        .expect("execute() hung - deadlock in mock script?");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let pids = parse_pids(temp_dir.path());
    assert!(!pids.is_empty(), "Mock script should have recorded PIDs");

    for (_parent_pid, child_pid) in &pids {
        assert!(
            !pid_alive(*child_pid),
            "Grandchild process {} (simulated MCP server) should be dead - \
             this is an orphan leak! The ACP executor must kill the entire \
             process tree, not just the direct child.",
            child_pid
        );
    }
}

/// Simulate two consecutive hat transitions. After each, all processes from
/// the previous execution should be fully cleaned up.
#[tokio::test]
async fn acp_no_orphans_across_hat_transitions() {
    let temp_dir = TempDir::new().unwrap();
    let _script = create_mock_acp_script(temp_dir.path());

    let backend = CliBackend {
        #[cfg(unix)]
        command: temp_dir.path().join("mock-kiro-acp.sh").to_string_lossy().to_string(),
        #[cfg(windows)]
        command: "powershell".to_string(),
        args: get_mock_script_args(temp_dir.path()),
        prompt_mode: PromptMode::Stdin,
        prompt_flag: None,
        output_format: OutputFormat::Acp,
        env_vars: vec![],
    };

    // Hat 1: "planning" hat
    let executor1 = AcpExecutor::new(backend.clone(), temp_dir.path().to_path_buf());
    let mut handler = NullHandler;
    let _ = timeout(
        TEST_TIMEOUT,
        executor1.execute("plan the feature", &mut handler),
    )
    .await
    .expect("execute() hung - deadlock in mock script?");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let pids_after_hat1 = parse_pids(temp_dir.path());
    assert_eq!(pids_after_hat1.len(), 1, "Should have 1 execution recorded");

    // Verify hat 1 processes are dead before hat 2 starts
    let (p1, c1) = pids_after_hat1[0];
    assert!(!pid_alive(p1), "Hat 1 parent should be dead before hat 2");
    assert!(
        !pid_alive(c1),
        "Hat 1 grandchild (MCP server) should be dead before hat 2 - orphan leak!"
    );

    // Hat 2: "builder" hat
    let executor2 = AcpExecutor::new(backend.clone(), temp_dir.path().to_path_buf());
    let _ = timeout(
        TEST_TIMEOUT,
        executor2.execute("build the feature", &mut handler),
    )
    .await
    .expect("execute() hung - deadlock in mock script?");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let pids_after_hat2 = parse_pids(temp_dir.path());
    assert_eq!(
        pids_after_hat2.len(),
        2,
        "Should have 2 executions recorded"
    );

    // ALL processes from both hats should be dead
    for (i, (parent, child)) in pids_after_hat2.iter().enumerate() {
        assert!(
            !pid_alive(*parent),
            "Hat {} parent process {} still alive",
            i + 1,
            parent
        );
        assert!(
            !pid_alive(*child),
            "Hat {} grandchild process {} still alive - orphan leak!",
            i + 1,
            child
        );
    }
}

/// Test that process_exists works correctly with the platform abstraction.
#[test]
fn test_process_exists_current_process() {
    let current_pid = std::process::id();
    assert!(
        ralph_core::platform::process_exists(current_pid),
        "Current process should exist"
    );
}

#[test]
fn test_process_exists_nonexistent() {
    // Very high PID unlikely to exist
    assert!(
        !ralph_core::platform::process_exists(999_999),
        "Non-existent PID should return false"
    );
}
