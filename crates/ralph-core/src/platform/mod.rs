//! Platform Abstraction Layer (PAL) for cross-platform compatibility.
//!
//! Provides unified interfaces for:
//! - File locking (fs4-based, compatible with flock semantics)
//! - Process detection and termination
//! - Filesystem links (symlink on Unix, hard link + junction on Windows)
//!
//! # Design
//!
//! All platform-specific code is encapsulated in submodules:
//! - `locks`: Cross-platform file locking
//! - `process`: Process enumeration and control
//! - `fs_links`: Cross-platform filesystem linking

pub mod fs_links;
pub mod locks;
pub mod process;

pub use fs_links::{create_dir_link, create_file_link, create_link, create_symlink_or_hardlink};
pub use locks::{FileLock, LockGuard, LockType};
pub use process::{ProcessInfo, kill_process_tree, process_exists};
