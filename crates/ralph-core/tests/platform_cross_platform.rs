//! Cross-platform integration tests for the platform abstraction layer.
//!
//! These tests verify that file locking, process control, and filesystem links
//! work correctly on both Unix and Windows platforms.

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

// Import the platform module
use ralph_core::platform::locks::{FileLock, LockType, LockedFile};
use ralph_core::platform::process::{
    get_process_info, kill_process_tree, list_child_processes, process_exists,
};

// ═══════════════════════════════════════════════════════════════════════════════
// FileLock Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn cross_platform_file_lock_create() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");

    let lock = FileLock::new(&file_path);
    assert!(lock.is_ok(), "Should create FileLock successfully");

    let lock = lock.unwrap();
    let lock_path = temp_dir.path().join("test.txt.lock");
    assert_eq!(
        lock.lock_path(),
        lock_path,
        "Lock path should have .lock extension"
    );
}

#[test]
fn cross_platform_file_lock_acquire_shared() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");

    let lock = FileLock::new(&file_path).unwrap();
    let guard = lock.acquire(LockType::Shared);
    assert!(guard.is_ok(), "Should acquire shared lock");
}

#[test]
fn cross_platform_file_lock_acquire_exclusive() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");

    let lock = FileLock::new(&file_path).unwrap();
    let guard = lock.acquire(LockType::Exclusive);
    assert!(guard.is_ok(), "Should acquire exclusive lock");
}

#[test]
fn cross_platform_file_lock_concurrent_shared() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");

    // Multiple shared locks should be allowed concurrently
    let lock1 = Arc::new(FileLock::new(&file_path).unwrap());
    let lock2 = Arc::new(FileLock::new(&file_path).unwrap());
    let lock3 = Arc::new(FileLock::new(&file_path).unwrap());

    let barrier = Arc::new(Barrier::new(3));

    let barrier1 = Arc::clone(&barrier);
    let barrier2 = Arc::clone(&barrier);
    let barrier3 = Arc::clone(&barrier);

    let lock1_clone = Arc::clone(&lock1);
    let lock2_clone = Arc::clone(&lock2);
    let lock3_clone = Arc::clone(&lock3);

    let handle1 = thread::spawn(move || {
        barrier1.wait();
        let _guard = lock1_clone.acquire(LockType::Shared).unwrap();
        thread::sleep(Duration::from_millis(50));
        // Lock guard drops here, releasing the lock
    });

    let handle2 = thread::spawn(move || {
        barrier2.wait();
        let _guard = lock2_clone.acquire(LockType::Shared).unwrap();
        thread::sleep(Duration::from_millis(50));
        // Lock guard drops here, releasing the lock
    });

    let handle3 = thread::spawn(move || {
        barrier3.wait();
        let _guard = lock3_clone.acquire(LockType::Shared).unwrap();
        thread::sleep(Duration::from_millis(50));
        // Lock guard drops here, releasing the lock
    });

    // All threads should complete successfully
    handle1.join().expect("Thread 1 should complete");
    handle2.join().expect("Thread 2 should complete");
    handle3.join().expect("Thread 3 should complete");
}

#[test]
fn cross_platform_file_lock_exclusive_blocks_exclusive() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");

    let lock1 = FileLock::new(&file_path).unwrap();
    let lock2 = FileLock::new(&file_path).unwrap();

    // Acquire exclusive lock with first instance
    let _guard1 = lock1.acquire(LockType::Exclusive).unwrap();

    // Try to acquire with second instance (should fail or block)
    let guard2 = lock2.try_acquire(LockType::Exclusive);
    assert!(guard2.is_ok(), "try_acquire should return Ok");

    // On both Unix and Windows, the lock should not be available
    let guard2 = guard2.unwrap();
    assert!(
        guard2.is_none(),
        "Exclusive lock should block another exclusive lock"
    );
}

#[test]
fn cross_platform_file_lock_exclusive_blocks_shared() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");

    let lock1 = FileLock::new(&file_path).unwrap();
    let lock2 = FileLock::new(&file_path).unwrap();

    // Acquire exclusive lock
    let _guard1 = lock1.acquire(LockType::Exclusive).unwrap();

    // Try shared lock (should not be available while exclusive is held)
    let guard2 = lock2.try_acquire(LockType::Shared).unwrap();
    assert!(
        guard2.is_none(),
        "Shared lock should be blocked by exclusive lock"
    );
}

#[test]
fn cross_platform_file_lock_release_on_drop() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");

    let lock1 = FileLock::new(&file_path).unwrap();
    let lock2 = FileLock::new(&file_path).unwrap();

    // Acquire and then release by dropping
    {
        let _guard = lock1.acquire(LockType::Exclusive).unwrap();
    } // Guard dropped here

    // Should be able to acquire now
    let guard2 = lock2.try_acquire(LockType::Exclusive);
    assert!(guard2.is_ok(), "try_acquire should succeed");
    assert!(
        guard2.unwrap().is_some(),
        "Lock should be available after release"
    );
}

#[test]
fn cross_platform_file_lock_convenience_methods() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");

    let lock = FileLock::new(&file_path).unwrap();

    // Test shared() convenience method
    let guard = lock.shared();
    assert!(guard.is_ok(), "shared() should work");
    drop(guard);

    // Test exclusive() convenience method
    let guard = lock.exclusive();
    assert!(guard.is_ok(), "exclusive() should work");
    drop(guard);

    // Test try_shared() convenience method
    let guard = lock.try_shared();
    assert!(guard.is_ok(), "try_shared() should work");
    drop(guard);

    // Test try_exclusive() convenience method
    let guard = lock.try_exclusive();
    assert!(guard.is_ok(), "try_exclusive() should work");
    assert!(
        guard.unwrap().is_some(),
        "try_exclusive() should acquire lock"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// LockedFile Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn cross_platform_locked_file_read_write() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");

    let locked = LockedFile::new(&file_path).unwrap();

    // Write content
    locked
        .write(&file_path, "Hello, cross-platform world!")
        .unwrap();

    // Read content back
    let content = locked.read(&file_path).unwrap();
    assert_eq!(content, "Hello, cross-platform world!");
}

#[test]
fn cross_platform_locked_file_with_lock_methods() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");

    let locked = LockedFile::new(&file_path).unwrap();

    // Test with_exclusive_lock
    locked
        .with_exclusive_lock(|| std::fs::write(&file_path, "exclusive content"))
        .unwrap();

    // Test with_shared_lock
    let content = locked
        .with_shared_lock(|| std::fs::read_to_string(&file_path))
        .unwrap();

    assert_eq!(content, "exclusive content");
}

#[test]
fn cross_platform_locked_file_concurrent_writes() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("counter.txt");

    // Initialize counter
    std::fs::write(&file_path, "0").unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let barrier1 = Arc::clone(&barrier);
    let barrier2 = Arc::clone(&barrier);

    let file_path1 = file_path.clone();
    let file_path2 = file_path.clone();

    let handle1 = thread::spawn(move || {
        let locked = LockedFile::new(&file_path1).unwrap();
        barrier1.wait();

        locked
            .with_exclusive_lock(|| {
                let content = std::fs::read_to_string(&file_path1)?;
                let n: i32 = content.trim().parse().unwrap_or(0);
                thread::sleep(Duration::from_millis(10));
                std::fs::write(&file_path1, format!("{}", n + 1))
            })
            .unwrap();
    });

    let handle2 = thread::spawn(move || {
        let locked = LockedFile::new(&file_path2).unwrap();
        barrier2.wait();

        locked
            .with_exclusive_lock(|| {
                let content = std::fs::read_to_string(&file_path2)?;
                let n: i32 = content.trim().parse().unwrap_or(0);
                thread::sleep(Duration::from_millis(10));
                std::fs::write(&file_path2, format!("{}", n + 1))
            })
            .unwrap();
    });

    handle1.join().unwrap();
    handle2.join().unwrap();

    // Both writes should have completed successfully
    let final_content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(
        final_content.trim(),
        "2",
        "Counter should be incremented twice"
    );
}

#[test]
fn cross_platform_locked_file_read_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("nonexistent.txt");

    let locked = LockedFile::new(&file_path).unwrap();

    // Reading a non-existent file should return empty string
    let content = locked.read(&file_path).unwrap();
    assert_eq!(
        content, "",
        "Reading non-existent file should return empty string"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lock Path Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn cross_platform_lock_path_with_extension() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("data.jsonl");

    let lock = FileLock::new(&file_path).unwrap();
    let expected = temp_dir.path().join("data.jsonl.lock");
    assert_eq!(
        lock.lock_path(),
        expected,
        "Should append .lock to extension"
    );
}

#[test]
fn cross_platform_lock_path_without_extension() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("data");

    let lock = FileLock::new(&file_path).unwrap();
    // Implementation uses with_extension(".lock") which produces "data..lock" for files without extension
    let expected = temp_dir.path().join("data..lock");
    assert_eq!(
        lock.lock_path(),
        expected,
        "Should add .lock extension (implementation detail: with_extension)"
    );
}

#[test]
fn cross_platform_lock_path_nested_directory() {
    let temp_dir = TempDir::new().unwrap();
    let nested_dir = temp_dir.path().join("a").join("b").join("c");
    let file_path = nested_dir.join("file.txt");

    let lock = FileLock::new(&file_path).unwrap();
    assert!(!lock.lock_path().exists(), "Lock file should not exist yet");

    // Acquiring the lock should create parent directories
    let _guard = lock.acquire(LockType::Exclusive).unwrap();
    assert!(
        lock.lock_path().parent().unwrap().exists(),
        "Parent directories should be created"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Stress Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn cross_platform_file_lock_stress_many_threads() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("counter.txt");

    // Initialize counter
    std::fs::write(&file_path, "0").unwrap();

    const NUM_THREADS: usize = 10;
    const INCREMENTS_PER_THREAD: usize = 10;

    let barrier = Arc::new(Barrier::new(NUM_THREADS));

    let handles: Vec<_> = (0..NUM_THREADS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let file_path = file_path.clone();

            thread::spawn(move || {
                let locked = LockedFile::new(&file_path).unwrap();
                barrier.wait();

                for _ in 0..INCREMENTS_PER_THREAD {
                    locked
                        .with_exclusive_lock(|| {
                            let content = std::fs::read_to_string(&file_path)?;
                            let n: usize = content.trim().parse().unwrap_or(0);
                            std::fs::write(&file_path, format!("{}", n + 1))
                        })
                        .unwrap();
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    // Counter should be NUM_THREADS * INCREMENTS_PER_THREAD
    let final_content = std::fs::read_to_string(&file_path).unwrap();
    let final_count: usize = final_content.trim().parse().unwrap();
    assert_eq!(
        final_count,
        NUM_THREADS * INCREMENTS_PER_THREAD,
        "Counter should have been incremented exactly {} times",
        NUM_THREADS * INCREMENTS_PER_THREAD
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Process Control Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn cross_platform_process_exists_current() {
    // Current process should exist
    let current_pid = std::process::id();
    assert!(process_exists(current_pid), "Current process should exist");
}

#[test]
fn cross_platform_process_exists_invalid() {
    // PID 0 should not exist on any platform
    assert!(!process_exists(0), "PID 0 should not exist");

    // Very high PID unlikely to exist
    assert!(!process_exists(999_999), "Very high PID should not exist");
}

#[test]
fn cross_platform_get_process_info_current() {
    let current_pid = std::process::id();
    let info = get_process_info(current_pid);

    assert!(info.is_some(), "Should get info for current process");
    let info = info.unwrap();
    assert_eq!(info.pid, current_pid, "PID should match");
    assert!(
        info.name.is_some(),
        "Process name should be available for current process"
    );
}

#[test]
fn cross_platform_get_process_info_nonexistent() {
    let info = get_process_info(999_999);
    assert!(
        info.is_none(),
        "Should return None for non-existent process"
    );
}

#[test]
fn cross_platform_process_spawn_and_check() {
    // Spawn a child process that will exist long enough for us to check it
    let mut child = std::process::Command::new("sleep")
        .arg("10")
        .spawn()
        .expect("Failed to spawn child process");

    let child_pid = child.id();

    // Give the process a moment to start
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Process should exist
    assert!(
        process_exists(child_pid),
        "Child process should exist after spawning"
    );

    // Get process info
    let info = get_process_info(child_pid);
    assert!(info.is_some(), "Should get info for child process");
    let info = info.unwrap();
    assert_eq!(info.pid, child_pid);

    // Clean up
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cross_platform_list_children_spawned() {
    // Spawn a child process
    let mut child = std::process::Command::new("sleep")
        .arg("10")
        .spawn()
        .expect("Failed to spawn child process");

    let child_pid = child.id();
    let parent_pid = std::process::id();

    // Give the process a moment to start
    std::thread::sleep(std::time::Duration::from_millis(100));

    // List children of current process
    let children = list_child_processes(parent_pid);

    // Should find our spawned child
    let found = children.iter().any(|c| c.pid == child_pid);
    assert!(found, "Should find spawned child in children list");

    // Verify parent relationship
    let child_info = children.iter().find(|c| c.pid == child_pid);
    if let Some(info) = child_info {
        assert_eq!(
            info.parent_pid,
            Some(parent_pid),
            "Child should have correct parent PID"
        );
    }

    // Clean up
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cross_platform_kill_process_tree_single() {
    // Spawn a child process
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("Failed to spawn child process");

    let child_pid = child.id();

    // Give the process a moment to start
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Verify process exists
    assert!(
        process_exists(child_pid),
        "Child process should exist before kill"
    );

    // Kill the process tree
    let result = kill_process_tree(child_pid);
    assert!(result.is_ok(), "kill_process_tree should succeed");

    // Give the system a moment to terminate
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Process should no longer exist
    assert!(
        !process_exists(child_pid),
        "Process should not exist after kill_process_tree"
    );

    // Clean up (in case it survived)
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cross_platform_kill_process_tree_grandchildren() {
    // Spawn a shell that spawns another process
    // This tests that the tree termination works through child processes
    let mut child = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "timeout /t 30"])
            .spawn()
            .expect("Failed to spawn cmd process")
    } else {
        std::process::Command::new("sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .expect("Failed to spawn shell process")
    };

    let child_pid = child.id();

    // Give the process a moment to start
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Verify process exists
    assert!(
        process_exists(child_pid),
        "Parent process should exist before kill"
    );

    // Kill the process tree
    let result = kill_process_tree(child_pid);
    assert!(result.is_ok(), "kill_process_tree should succeed");

    // Give the system a moment to terminate
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Process should no longer exist
    assert!(
        !process_exists(child_pid),
        "Process should not exist after kill_process_tree"
    );

    // Clean up (in case it survived)
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cross_platform_kill_already_dead() {
    // Spawn a short-lived process
    let mut child = std::process::Command::new("echo")
        .arg("hello")
        .spawn()
        .expect("Failed to spawn child process");

    let child_pid = child.id();

    // Wait for it to complete naturally
    let _ = child.wait();

    // Give a moment for process state to update
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Attempting to kill an already-dead process should not fail
    let result = kill_process_tree(child_pid);
    // On most platforms this succeeds (process already gone)
    // or returns a specific error we can accept
    if let Err(ref e) = result {
        // Some platforms may return "process not found" which is acceptable
        println!("kill_process_tree returned error for dead process: {}", e);
    }
    // Test passes if either Ok or Err - we just verify it doesn't panic
}

#[test]
fn cross_platform_process_info_consistency() {
    // Verify that process info from get_process_info matches list_child_processes
    let parent_pid = std::process::id();

    // Spawn a child
    let mut child = std::process::Command::new("sleep")
        .arg("10")
        .spawn()
        .expect("Failed to spawn child process");

    let child_pid = child.id();

    // Give a moment for process to start
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Get info directly
    let direct_info = get_process_info(child_pid);
    assert!(direct_info.is_some());
    let direct_info = direct_info.unwrap();

    // Get info from children list
    let children = list_child_processes(parent_pid);
    let listed_info = children.iter().find(|c| c.pid == child_pid);
    assert!(
        listed_info.is_some(),
        "Child should appear in children list"
    );
    let listed_info = listed_info.unwrap();

    // Info should be consistent
    assert_eq!(direct_info.pid, listed_info.pid);
    assert_eq!(direct_info.parent_pid, listed_info.parent_pid);

    // Clean up
    let _ = child.kill();
    let _ = child.wait();
}
