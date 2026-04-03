//! Integration tests for cross-platform loop management (list/stop).
//!
//! Tests per spec: specs/windows-native-support

use std::process::Command;
use tempfile::TempDir;

fn run_ralph(temp_path: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args(args)
        .current_dir(temp_path)
        .output()
        .expect("execute ralph")
}

fn setup_workspace() -> TempDir {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    Command::new("git")
        .args(["init"])
        .current_dir(temp_path)
        .output()
        .expect("git init");
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(temp_path)
        .output()
        .expect("git config email");
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(temp_path)
        .output()
        .expect("git config name");

    std::fs::create_dir_all(temp_path.join(".ralph")).expect("create .ralph");

    temp_dir
}

#[cfg(unix)]
fn spawn_sleeper(seconds: u64) -> std::process::Child {
    Command::new("sleep")
        .arg(seconds.to_string())
        .spawn()
        .expect("spawn sleep")
}

#[cfg(windows)]
fn spawn_sleeper(seconds: u64) -> std::process::Child {
    Command::new("cmd")
        .args(["/C", &format!("timeout /t {} >nul", seconds)])
        .spawn()
        .expect("spawn timeout")
}

#[test]
fn cross_platform_loops_list_stop_primary() {
    let temp_dir = setup_workspace();
    let temp_path = temp_dir.path();

    // Acquire the primary loop lock in this test process
    let _guard =
        ralph_core::LoopLock::try_acquire(temp_path, "test prompt").expect("acquire loop lock");

    // List should show the primary loop as running
    let output = run_ralph(temp_path, &["loops", "list"]);
    assert!(
        output.status.success(),
        "loops list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("(primary)") && stdout.contains("running"),
        "expected primary loop in list output: {}",
        stdout
    );

    // Stop the primary loop
    let output = run_ralph(temp_path, &["loops", "stop"]);
    assert!(
        output.status.success(),
        "loops stop failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Stop requested"),
        "expected stop request confirmation: {}",
        stdout
    );

    // Verify stop signal file was written
    assert!(
        temp_path.join(".ralph/stop-requested").exists(),
        "stop-requested file should exist"
    );
}

#[test]
fn cross_platform_loops_stop_and_orphan_cleanup() {
    let temp_dir = setup_workspace();
    let temp_path = temp_dir.path();

    // Spawn a child process that will be our "orphan" loop
    let mut child = spawn_sleeper(30);

    // Register the loop with a worktree path that does not exist
    let registry = ralph_core::LoopRegistry::new(temp_path);
    let missing_worktree = temp_path.join(".worktrees/orphan-loop");
    let mut entry = ralph_core::LoopEntry::with_id(
        "orphan-loop",
        "orphan test prompt",
        Some(missing_worktree.display().to_string()),
        temp_path.display().to_string(),
    );
    entry.pid = child.id();
    registry.register(entry).expect("register loop");

    // Stop should detect the missing worktree and kill the orphan process
    let output = run_ralph(temp_path, &["loops", "stop", "orphan-loop"]);
    assert!(
        output.status.success(),
        "loops stop orphan failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("cleaned up") || stdout.contains("Killing") || stdout.contains("orphan"),
        "expected orphan cleanup confirmation: {}",
        stdout
    );

    // Verify the child process was terminated
    assert!(
        !ralph_core::platform::process_exists(child.id()),
        "orphan child process should have been killed"
    );

    // Verify registry entry was removed
    assert!(
        registry.get("orphan-loop").expect("registry get").is_none(),
        "orphan entry should have been deregistered"
    );

    // Clean up just in case
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cross_platform_web_command_unsupported_on_windows() {
    let temp_dir = setup_workspace();
    let temp_path = temp_dir.path();

    // Run 'ralph web' command
    let output = run_ralph(temp_path, &["web", "--no-open"]);

    // On Windows, the command should fail with a clear error message
    // On Unix, it may succeed or fail depending on environment (no node_modules, etc.)
    // This test primarily validates the Windows error path exists
    #[cfg(windows)]
    {
        assert!(
            !output.status.success(),
            "ralph web should fail on Windows: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("not supported on Windows") || stderr.contains("WSL"),
            "expected Windows unsupported message, got: {}",
            stderr
        );
    }

    // On Unix, just verify the command runs without panic (may fail due to missing deps)
    #[cfg(unix)]
    {
        // The command should at least start processing (exit code may vary based on environment)
        // We just verify it doesn't segfault or panic
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{}{}", stdout, stderr);

        // Should NOT contain the Windows-specific error message on Unix
        assert!(
            !combined.contains("not supported on Windows"),
            "Unix should not show Windows unsupported message: {}",
            combined
        );
    }
}
