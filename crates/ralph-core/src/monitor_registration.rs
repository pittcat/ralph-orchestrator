// ABOUTME: Best-effort ralph-monitor project auto-registration.
// ABOUTME: `ralph run` drops a JSON file into ~/.ralph/projects.d so the
// ABOUTME: monitor (backend/ralph-monitor) adopts the project without a
// ABOUTME: manual "Add project" step. Registration is idempotent and must
// ABOUTME: never fail the run, so every error path is swallowed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::parallel_forge_resume::sha256_hex;

/// Override for the registration drop directory (tests / custom installs).
pub const REGISTRY_DIR_ENV: &str = "RALPH_MONITOR_REGISTRY_DIR";
/// Set to `0`/`false`/`no` to opt out of auto-registration.
pub const DISABLE_ENV: &str = "RALPH_MONITOR_AUTOREGISTER";

/// Registry drop directory: `$RALPH_MONITOR_REGISTRY_DIR`, else
/// `~/.ralph/projects.d` (sibling of the default `~/.ralph/monitor.db`).
pub fn registry_dir() -> Option<PathBuf> {
    registry_dir_from(
        std::env::var_os(REGISTRY_DIR_ENV),
        std::env::home_dir().as_deref(),
    )
}

fn registry_dir_from(override_dir: Option<OsString>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = override_dir
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    home.map(|h| h.join(".ralph").join("projects.d"))
}

/// File name for a canonical project path — sha256 hex, matching the
/// monitor's own naming so DELETE can tombstone the file.
pub fn registration_file_name(canonical_path: &str) -> String {
    format!("{}.json", sha256_hex(canonical_path.as_bytes()))
}

fn disabled() -> bool {
    disabled_from(std::env::var(DISABLE_ENV).ok().as_deref())
}

fn disabled_from(value: Option<&str>) -> bool {
    matches!(value, Some("0" | "false" | "no"))
}

/// Drop a registration file for `project_root`. Called by `ralph run`
/// with the main-repo root (RALPH_WORKSPACE_ROOT or the workspace root)
/// so worktree child runs register the main project, not the worktree.
pub fn register_project(project_root: &Path) {
    if disabled() {
        return;
    }
    register_project_at(project_root, registry_dir());
}

fn register_project_at(project_root: &Path, dir: Option<PathBuf>) {
    match dir {
        Some(dir) => {
            if let Err(err) = write_registration(project_root, &dir) {
                tracing::debug!(error = %err, "monitor auto-registration skipped");
            }
        }
        None => {
            tracing::debug!("monitor auto-registration skipped: no home dir");
        }
    }
}

fn write_registration(project_root: &Path, dir: &Path) -> std::io::Result<()> {
    let canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let canonical_str = canonical.to_string_lossy().into_owned();
    let display_name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| canonical_str.clone());

    let payload = serde_json::json!({
        "version": 1,
        "path": canonical_str,
        "displayName": display_name,
        "registeredAt": chrono::Utc::now().to_rfc3339(),
        "source": "ralph run",
    });

    std::fs::create_dir_all(dir)?;
    let target = dir.join(registration_file_name(&canonical_str));
    let tmp = dir.join(format!(".{}.tmp", std::process::id()));
    std::fs::write(&tmp, payload.to_string())?;
    std::fs::rename(&tmp, &target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_dir_honors_env_override() {
        assert_eq!(
            registry_dir_from(
                Some(OsString::from("/tmp/ralph-mon-test")),
                Some(Path::new("/home/u")),
            ),
            Some(PathBuf::from("/tmp/ralph-mon-test"))
        );
    }

    #[test]
    fn registry_dir_defaults_to_home_projects_d() {
        assert_eq!(
            registry_dir_from(None, Some(Path::new("/home/u"))),
            Some(PathBuf::from("/home/u/.ralph/projects.d"))
        );
    }

    #[test]
    fn registry_dir_empty_override_falls_back_to_home() {
        assert_eq!(
            registry_dir_from(Some(OsString::from("")), Some(Path::new("/home/u"))),
            Some(PathBuf::from("/home/u/.ralph/projects.d"))
        );
    }

    #[test]
    fn registry_dir_none_without_home() {
        assert_eq!(registry_dir_from(None, None), None);
    }

    #[test]
    fn file_name_is_sha256_hex_of_path() {
        let name = registration_file_name("/tmp/proj");
        assert_eq!(name, format!("{}.json", sha256_hex(b"/tmp/proj")));
        assert_eq!(name.len(), 64 + 5);
    }

    #[test]
    fn disable_env_values() {
        assert!(disabled_from(Some("0")));
        assert!(disabled_from(Some("false")));
        assert!(disabled_from(Some("no")));
        assert!(!disabled_from(Some("1")));
        assert!(!disabled_from(Some("yes")));
        assert!(!disabled_from(None));
    }

    #[test]
    fn register_project_writes_json_file() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = tmp.path().join("projects.d");

        let project = tmp.path().join("my-proj");
        std::fs::create_dir_all(&project).unwrap();
        register_project_at(&project, Some(registry.clone()));

        let canonical = project
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let file = registry.join(registration_file_name(&canonical));
        assert!(file.exists(), "registration file must exist");

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["path"], canonical);
        assert_eq!(json["displayName"], "my-proj");
        assert_eq!(json["source"], "ralph run");
        assert!(json["registeredAt"].as_str().unwrap().contains('T'));
    }

    #[test]
    fn register_project_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = tmp.path().join("projects.d");

        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        register_project_at(&project, Some(registry.clone()));
        register_project_at(&project, Some(registry.clone()));

        assert_eq!(
            std::fs::read_dir(&registry).unwrap().count(),
            1,
            "second registration overwrites the same file"
        );
    }

    #[test]
    fn register_project_never_panics_on_bad_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // Point the registry at a path that cannot be created (parent is a file).
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        register_project_at(Path::new("/nonexistent-project"), Some(blocker.join("sub")));
        // No panic — best effort only.
    }

    #[test]
    fn register_project_no_dir_is_noop() {
        register_project_at(Path::new("/tmp/whatever"), None);
    }
}
