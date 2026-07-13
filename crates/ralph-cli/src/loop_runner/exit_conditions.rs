use super::*;

// ── PhaseWatcher: Warmup/Production Two-Phase Transition ────────────────────

/// Result of checking exit conditions for warmup phase transition.
#[derive(Debug)]
pub enum CheckExitResult {
    /// All conditions satisfied — ready to transition
    Ready,
    /// Conditions not yet met — continue warmup
    NotReady { unmet_conditions: Vec<String> },
    /// Drain required — waiting for in-flight experiments
    DrainRequired { pending_count: usize },
}

/// Run the check_exit_conditions.py script to determine if warmup can transition.
pub async fn run_check_exit_conditions(ctx: &LoopContext) -> Result<CheckExitResult> {
    let script_path = find_skills_script("check_exit_conditions.py")
        .context("check_exit_conditions.py not found in skills directory")?;

    let project_root = ctx.workspace().to_string_lossy();
    let output_json = ctx.ralph_dir().join("check_exit_result.json");

    let mut cmd = Command::new("python3");
    cmd.arg(&script_path)
        .arg("--project-root")
        .arg(project_root.as_ref())
        .arg("--output-json")
        .arg(&output_json)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    info!("Running check_exit_conditions.py: {:?}", cmd);
    let output = cmd
        .output()
        .context("Failed to run check_exit_conditions.py")?;

    // Read JSON result if file was created
    if output_json.exists() {
        let content =
            fs::read_to_string(&output_json).context("Failed to read check_exit result")?;
        let result: serde_json::Value =
            serde_json::from_str(&content).context("Failed to parse check_exit result JSON")?;

        let exit_code = result
            .get("exit_code")
            .and_then(|v| v.as_str())
            .unwrap_or("1");

        return match exit_code {
            "0" => Ok(CheckExitResult::Ready),
            "42" => {
                let pending = result
                    .get("pending_experiments")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                Ok(CheckExitResult::DrainRequired {
                    pending_count: pending,
                })
            }
            _ => {
                let unmet = result
                    .get("unmet_conditions")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(CheckExitResult::NotReady {
                    unmet_conditions: unmet,
                })
            }
        };
    }

    // Fallback: interpret exit code from process
    if output.status.success() {
        Ok(CheckExitResult::Ready)
    } else {
        Ok(CheckExitResult::NotReady {
            unmet_conditions: vec![],
        })
    }
}

/// Run the transition_warmup_to_production.py script to perform phase transition.
pub async fn run_transition_script(ctx: &LoopContext, stop: bool) -> Result<()> {
    let script_path = find_skills_script("transition_warmup_to_production.py")
        .context("transition_warmup_to_production.py not found in skills directory")?;

    let project_root = ctx.workspace().to_string_lossy();

    let mut cmd = Command::new("python3");
    cmd.arg(&script_path)
        .arg("--project-root")
        .arg(project_root.as_ref());

    if stop {
        cmd.arg("--stop");
    }

    info!("Running transition_warmup_to_production.py: {:?}", cmd);
    let output = cmd
        .output()
        .context("Failed to run transition_warmup_to_production.py")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("transition_warmup_to_production.py failed: {}", stderr);
    }

    info!("Phase transition script completed successfully");
    Ok(())
}

/// Find a script in the skills directory.
pub fn find_skills_script(name: &str) -> Result<PathBuf> {
    // Search common skills locations
    let search_paths = vec![
        PathBuf::from("."),
        PathBuf::from(".."),
        PathBuf::from("../.."),
        PathBuf::from("skills"),
        PathBuf::from("../skills"),
        PathBuf::from("../../skills"),
        // External config generators (e.g., universal-autoresearch) install
        // phase transition scripts into <project>/.uar/scripts/. Without
        // this base, PhaseWatcher silently fails to locate them and warmup
        // never transitions.
        PathBuf::from(".uar/scripts"),
    ];

    for base in search_paths {
        let path = base.join(name);
        if path.exists() {
            return Ok(path);
        }
        // Also check in universal-autoresearch/scripts
        let alt_path = base
            .join("universal-autoresearch")
            .join("scripts")
            .join(name);
        if alt_path.exists() {
            return Ok(alt_path);
        }
    }

    anyhow::bail!("Script not found: {}", name)
}

#[cfg(test)]
mod tests {
    //! Unit tests for `find_skills_script` — the search-base order is a
    //! silent compatibility surface; without these, future refactors can
    //! quietly break external generators (e.g., universal-autoresearch)
    //! that install scripts into `.uar/scripts/`.

    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Run `body` with cwd set to `dir`, restoring the original cwd on
    /// drop. Tests must not run concurrently in the same process when
    /// relying on cwd; `cargo test` defaults to a thread pool, so we
    /// mark each `find_skills_script` test with `--test-threads=1` via
    /// a shared `#[serial]` mutex below.
    struct CwdGuard {
        original: PathBuf,
    }

    impl CwdGuard {
        fn enter(dir: &std::path::Path) -> Self {
            let original = std::env::current_dir().expect("cwd");
            std::env::set_current_dir(dir).expect("set_current_dir");
            Self { original }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
        }
    }

    /// Serial mutex so cwd-manipulating tests don't stomp each other.
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn write_script(dir: &std::path::Path, relative: &str) {
        let full = dir.join(relative);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, "#!/usr/bin/env python3\n").unwrap();
    }

    /// `.uar/scripts/` is the install location used by
    /// universal-autoresearch (see generate_autoresearch.py:_copy_support_scripts).
    /// PhaseWatcher must locate `check_exit_conditions.py` placed there.
    #[test]
    fn find_skills_script_discovers_uar_scripts() {
        let _lock = CWD_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        write_script(dir.path(), ".uar/scripts/check_exit_conditions.py");

        let _cwd = CwdGuard::enter(dir.path());
        let found = find_skills_script("check_exit_conditions.py").unwrap();
        assert!(
            found.ends_with(".uar/scripts/check_exit_conditions.py"),
            "expected .uar/scripts path, got {found:?}"
        );
    }

    /// Same discovery guarantee for the second phase script.
    #[test]
    fn find_skills_script_discovers_uar_transition_script() {
        let _lock = CWD_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        write_script(dir.path(), ".uar/scripts/transition_warmup_to_production.py");

        let _cwd = CwdGuard::enter(dir.path());
        let found = find_skills_script("transition_warmup_to_production.py").unwrap();
        assert!(found.ends_with(".uar/scripts/transition_warmup_to_production.py"));
    }

    /// Legacy `skills/` location must keep winning when both exist, so
    /// projects that still install scripts the old way don't regress.
    /// Search order is `.`, `..`, `../..`, `skills`, `../skills`,
    /// `../../skills`, `.uar/scripts`.
    #[test]
    fn find_skills_script_legacy_skills_path_takes_priority_over_uar() {
        let _lock = CWD_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        write_script(dir.path(), "skills/check_exit_conditions.py");
        write_script(dir.path(), ".uar/scripts/check_exit_conditions.py");

        let _cwd = CwdGuard::enter(dir.path());
        let found = find_skills_script("check_exit_conditions.py").unwrap();
        assert!(
            found.ends_with("skills/check_exit_conditions.py"),
            "legacy skills/ path must take priority, got {found:?}"
        );
    }

    /// Old fallback chain must still resolve: top-level `.` and the
    /// `universal-autoresearch/scripts` alt-suffix were the historical
    /// installation layouts and must not regress after we added
    /// `.uar/scripts/`.
    #[test]
    fn find_skills_script_legacy_fallbacks_still_work() {
        let _lock = CWD_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        // Only the universal-autoresearch/scripts alt-suffix exists.
        write_script(dir.path(), "universal-autoresearch/scripts/check_exit_conditions.py");

        let _cwd = CwdGuard::enter(dir.path());
        let found = find_skills_script("check_exit_conditions.py").unwrap();
        assert!(
            found.ends_with("universal-autoresearch/scripts/check_exit_conditions.py"),
            "universal-autoresearch/scripts fallback regressed: {found:?}"
        );
    }

    /// Bare top-level install (no subdirectory) must still resolve via
    /// the `.` search base.
    #[test]
    fn find_skills_script_top_level_dot_still_works() {
        let _lock = CWD_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        write_script(dir.path(), "check_exit_conditions.py");

        let _cwd = CwdGuard::enter(dir.path());
        let found = find_skills_script("check_exit_conditions.py").unwrap();
        assert!(found.ends_with("check_exit_conditions.py"));
    }

    /// When nothing matches, the error message must include the script
    /// name so upstream log triage can identify which script Ralph was
    /// trying to locate.
    #[test]
    fn find_skills_script_missing_returns_helpful_error() {
        let _lock = CWD_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        // Empty tempdir; nothing on disk.

        let _cwd = CwdGuard::enter(dir.path());
        let err = find_skills_script("check_exit_conditions.py")
            .expect_err("expected bail when no script is found");
        let msg = err.to_string();
        assert!(
            msg.contains("check_exit_conditions.py"),
            "error message must contain script name, got: {msg}"
        );
    }
}
