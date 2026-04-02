//! Cross-platform filesystem linking.
//!
//! Provides unified filesystem linking across Unix and Windows:
//! - Unix: Uses symlinks for both files and directories
//! - Windows: Uses hard links for files, junctions for directories
//!
//! # Design
//!
//! Windows symlinks require Administrator privileges or Developer Mode.
//! Hard links and junctions work without elevation, so they're preferred
//! for cross-platform compatibility.
//!
//! # Example
//!
//! ```no_run
//! use ralph_core::platform::fs_links::create_link;
//! use std::path::Path;
//!
//! fn setup_shared_state() -> std::io::Result<()> {
//!     create_link(
//!         Path::new(".worktrees/loop-1/.ralph/agent/memories.md"),
//!         Path::new(".ralph/agent/memories.md"),
//!     )?;
//!     Ok(())
//! }
//! ```

use std::io;
use std::path::Path;

/// Creates a cross-platform link.
///
/// On Unix: Creates a symlink.
/// On Windows: Creates a hard link for files, junction for directories.
///
/// # Arguments
///
/// * `link` - The path where the link will be created
/// * `target` - The path that the link points to
pub fn create_link(link: impl AsRef<Path>, target: impl AsRef<Path>) -> io::Result<()> {
    let link = link.as_ref();
    let target = target.as_ref();

    // Ensure parent directory exists
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Determine if target is a file or directory
    let metadata = target.metadata()?;

    if metadata.is_dir() {
        create_dir_link(link, target)
    } else {
        create_file_link(link, target)
    }
}

/// Creates a file link (hard link on Windows, symlink on Unix).
pub fn create_file_link(link: impl AsRef<Path>, target: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(unix)]
    {
        create_symlink(link, target)
    }

    #[cfg(windows)]
    {
        create_hard_link(link, target)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (link, target);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "File linking not supported on this platform",
        ))
    }
}

/// Creates a directory link (junction on Windows, symlink on Unix).
pub fn create_dir_link(link: impl AsRef<Path>, target: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(unix)]
    {
        create_symlink(link, target)
    }

    #[cfg(windows)]
    {
        create_junction(link, target)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (link, target);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Directory linking not supported on this platform",
        ))
    }
}

/// Creates a symlink (Unix and Windows with Developer Mode).
///
/// On Windows without Developer Mode, this will fail.
#[cfg(unix)]
fn create_symlink(link: impl AsRef<Path>, target: impl AsRef<Path>) -> io::Result<()> {
    use std::os::unix::fs::symlink;
    symlink(target, link)
}

#[cfg(windows)]
fn create_symlink(link: impl AsRef<Path>, target: impl AsRef<Path>) -> io::Result<()> {
    use std::os::windows::fs::symlink_file;
    // Try symlink_file for files, symlink_dir for directories
    let target = target.as_ref();
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        symlink_file(target, link)
    }
}

/// Creates a hard link (Windows only).
#[cfg(unix)]
#[allow(dead_code)]
fn create_hard_link(_link: impl AsRef<Path>, _target: impl AsRef<Path>) -> io::Result<()> {
    // On Unix, we prefer symlinks over hard links
    create_symlink(_link, _target)
}

#[cfg(windows)]
fn create_hard_link(link: impl AsRef<Path>, target: impl AsRef<Path>) -> io::Result<()> {
    std::fs::hard_link(target, link)
}

/// Creates a directory junction (Windows only).
///
/// Junctions work on Windows without Administrator privileges.
/// They can only link to directories on local volumes.
#[cfg(unix)]
#[allow(dead_code)]
fn create_junction(_link: impl AsRef<Path>, _target: impl AsRef<Path>) -> io::Result<()> {
    // On Unix, we use symlinks for directories
    create_symlink(_link, _target)
}

#[cfg(windows)]
fn create_junction(link: impl AsRef<Path>, target: impl AsRef<Path>) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::process::Command;

    let link = link.as_ref();
    let target = target.as_ref();

    // Convert paths to wide strings for Windows API
    let link_str = link.as_os_str().to_string_lossy();
    let target_str = target.as_os_str().to_string_lossy();

    // Use mklink /J to create a junction
    // Note: mklink is a shell builtin, so we need to use cmd.exe
    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J", &link_str, &target_str])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("mklink failed: {}", stderr),
        ));
    }

    Ok(())
}

/// Removes a link (works for symlinks, hard links, and junctions).
///
/// This safely removes only the link, not the target.
pub fn remove_link(link: impl AsRef<Path>) -> io::Result<()> {
    let link = link.as_ref();

    if !link.exists() {
        return Ok(());
    }

    let metadata = link.symlink_metadata()?;

    if metadata.is_dir() && is_junction(link) {
        // On Windows, junctions are directories that need special handling
        remove_junction(link)
    } else if metadata.is_dir() {
        // Regular directory symlink
        std::fs::remove_dir(link)
    } else {
        // File symlink or hard link
        std::fs::remove_file(link)
    }
}

/// Checks if a path is a Windows junction.
fn is_junction(path: &Path) -> bool {
    #[cfg(windows)]
    {
        // On Windows, we can check the reparse point tag
        // For simplicity, we check if it's a directory and has the junction attribute
        if let Ok(metadata) = path.symlink_metadata() {
            return metadata.is_dir() && !path.is_symlink();
        }
    }

    let _ = path;
    false
}

#[cfg(unix)]
fn remove_junction(path: &Path) -> io::Result<()> {
    // On Unix, junctions don't exist - use regular remove
    std::fs::remove_dir(path)
}

#[cfg(windows)]
fn remove_junction(path: &Path) -> io::Result<()> {
    // On Windows, use rmdir to remove junctions
    use std::process::Command;

    let path_str = path.as_os_str().to_string_lossy();

    let output = Command::new("cmd")
        .args(["/C", "rmdir", &path_str])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("rmdir failed: {}", stderr),
        ));
    }

    Ok(())
}

/// Creates a symlink or falls back to hard link on Windows.
///
/// This is a convenience function that tries symlink first (which preserves
/// the semantic meaning), then falls back to hard link on Windows if
/// Developer Mode is not enabled.
pub fn create_symlink_or_hardlink(
    link: impl AsRef<Path>,
    target: impl AsRef<Path>,
) -> io::Result<()> {
    let link = link.as_ref();
    let target = target.as_ref();

    // Try symlink first
    #[cfg(windows)]
    {
        if create_symlink(link, target).is_ok() {
            return Ok(());
        }
        // Fall back to hard link for files
        if target.is_file() {
            return create_hard_link(link, target);
        }
    }

    #[cfg(unix)]
    {
        create_symlink(link, target)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (link, target);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Linking not supported on this platform",
        ))
    }

    #[cfg(windows)]
    Err(io::Error::new(
        io::ErrorKind::Other,
        "Failed to create link",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_file_link() {
        let temp_dir = TempDir::new().unwrap();
        let target = temp_dir.path().join("target.txt");
        let link = temp_dir.path().join("link.txt");

        // Create target file
        std::fs::write(&target, "Hello, World!").unwrap();

        // Create link
        create_file_link(&link, &target).unwrap();

        // Verify link exists and points to same content
        assert!(link.exists());
        let content = std::fs::read_to_string(&link).unwrap();
        assert_eq!(content, "Hello, World!");
    }

    #[test]
    fn test_create_dir_link() {
        let temp_dir = TempDir::new().unwrap();
        let target = temp_dir.path().join("target_dir");
        let link = temp_dir.path().join("link_dir");

        // Create target directory with a file
        std::fs::create_dir(&target).unwrap();
        let target_file = target.join("test.txt");
        std::fs::write(&target_file, "content").unwrap();

        // Create link
        create_dir_link(&link, &target).unwrap();

        // Verify link exists and accessible
        assert!(link.exists());
        let linked_file = link.join("test.txt");
        assert!(linked_file.exists());
        let content = std::fs::read_to_string(&linked_file).unwrap();
        assert_eq!(content, "content");
    }

    #[test]
    fn test_remove_link() {
        let temp_dir = TempDir::new().unwrap();
        let target = temp_dir.path().join("target.txt");
        let link = temp_dir.path().join("link.txt");

        // Create target and link
        std::fs::write(&target, "content").unwrap();
        create_file_link(&link, &target).unwrap();

        // Verify link exists
        assert!(link.exists());

        // Remove link
        remove_link(&link).unwrap();

        // Link should be gone but target should remain
        assert!(!link.exists());
        assert!(target.exists());
    }

    #[test]
    fn test_create_link_auto_detects_type() {
        let temp_dir = TempDir::new().unwrap();

        // Test with file
        let target_file = temp_dir.path().join("target.txt");
        let link_file = temp_dir.path().join("link.txt");
        std::fs::write(&target_file, "file content").unwrap();
        create_link(&link_file, &target_file).unwrap();
        assert!(link_file.exists());

        // Test with directory
        let target_dir = temp_dir.path().join("target_dir");
        let link_dir = temp_dir.path().join("link_dir");
        std::fs::create_dir(&target_dir).unwrap();
        std::fs::write(target_dir.join("file.txt"), "dir content").unwrap();
        create_link(&link_dir, &target_dir).unwrap();
        assert!(link_dir.exists());
        assert!(link_dir.join("file.txt").exists());
    }
}
