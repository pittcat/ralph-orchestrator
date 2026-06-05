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
