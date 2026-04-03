//! File locking for shared resources in multi-loop scenarios.
//!
//! Provides fine-grained file locking using cross-platform fs4 for concurrent access
//! to shared resources like `.ralph/agent/tasks.jsonl` and `.ralph/agent/memories.md`.
//! This enables multiple Ralph loops (in worktrees) to safely read and write
//! shared state files.
//!
//! # Design
//!
//! - **Shared locks** for reading: Multiple readers can hold shared locks simultaneously
//! - **Exclusive locks** for writing: Only one writer at a time, blocks readers
//! - **Blocking by default**: Operations wait for lock availability
//! - **RAII guards**: Locks are automatically released when guards are dropped
//!
//! # Example
//!
//! ```no_run
//! use ralph_core::file_lock::FileLock;
//!
//! fn read_tasks(path: &std::path::Path) -> std::io::Result<String> {
//!     let lock = FileLock::new(path)?;
//!     let _guard = lock.shared()?;  // Acquire shared lock
//!     std::fs::read_to_string(path)
//! }
//!
//! fn write_tasks(path: &std::path::Path, content: &str) -> std::io::Result<()> {
//!     let lock = FileLock::new(path)?;
//!     let _guard = lock.exclusive()?;  // Acquire exclusive lock
//!     std::fs::write(path, content)
//! }
//! ```

pub use crate::platform::locks::{FileLock, LockGuard, LockedFile};
