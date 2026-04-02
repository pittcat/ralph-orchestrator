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
    assert!(
        !lock.lock_path().exists(),
        "Lock file should not exist yet"
    );

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
