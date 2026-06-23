use super::*;

/// Processes pending merges from the merge queue.
///
/// Called when the primary loop completes successfully. Spawns merge-ralph
/// processes for each queued loop in FIFO order and **synchronously waits**
/// for every spawned child to exit before returning.
///
/// Why synchronous (mechanism layer, not point-fix):
/// - **Lifecycle binding**: child stdout/stderr are redirected to per-merge
///   log files. The parent must wait for child exit so the OS flushes and
///   closes the redirected file descriptors before the parent drops the
///   `File` handles it cloned into `Stdio::from(...)`. Otherwise the log
///   file can be observed empty (no flush yet) under load.
/// - **No orphans**: under the previous fire-and-forget shape, if the
///   caller returned early the spawned `ralph run` would be reparented to
///   init and become an orphan, leaking its log fd. Synchronous wait
///   bounds lifetime to the parent's stack frame.
/// - **Test determinism**: the existing tests assert on the contents of the
///   log file; they used a `std::thread::sleep(500ms)` which is a known
///   CPU-preemption flake. Synchronous wait replaces the sleep with an
///   event-driven signal (child exit).
pub fn process_pending_merges_with_command(repo_root: &Path, ralph_cmd: &OsStr) {
    let queue = MergeQueue::new(repo_root);

    // Get all pending merges
    let pending = match queue.list_by_state(ralph_core::merge_queue::MergeState::Queued) {
        Ok(entries) => entries,
        Err(e) => {
            warn!("Failed to read merge queue: {}", e);
            return;
        }
    };

    if pending.is_empty() {
        debug!("No pending merges in queue");
        return;
    }

    info!(
        count = pending.len(),
        "Processing pending merges from queue"
    );

    // Get the merge-loop preset content
    let preset = match crate::presets::get_preset("merge-loop") {
        Some(p) => p,
        None => {
            warn!("merge-loop preset not found, pending merges will remain queued");
            return;
        }
    };

    // Write a core-only merge config once (shared by all merge loops).
    let mut core_value: serde_yaml::Value = match serde_yaml::from_str(preset.content) {
        Ok(value) => value,
        Err(e) => {
            warn!(
                error = %e,
                "Failed to parse merge-loop preset, pending merges will remain queued"
            );
            return;
        }
    };

    if let Some(mapping) = core_value.as_mapping_mut() {
        let hats_key = serde_yaml::Value::String("hats".to_string());
        let events_key = serde_yaml::Value::String("events".to_string());
        mapping.remove(&hats_key);
        mapping.remove(&events_key);
    }

    let core_yaml = match serde_yaml::to_string(&core_value) {
        Ok(yaml) => yaml,
        Err(e) => {
            warn!(
                error = %e,
                "Failed to serialize core-only merge config, pending merges will remain queued"
            );
            return;
        }
    };

    let config_path = repo_root.join(".ralph/merge-loop-config.yml");
    if let Err(e) = fs::write(&config_path, core_yaml) {
        warn!(
            error = %e,
            "Failed to write merge config, pending merges will remain queued"
        );
        return;
    }

    // Collect spawned children so we can synchronously wait for them at the
    // end of the function (see function-level doc for rationale). FIFO order
    // matches the spawn order below.
    let mut children: Vec<std::process::Child> = Vec::with_capacity(pending.len());

    // Process each pending merge
    for entry in pending {
        let loop_id = &entry.loop_id;

        info!(loop_id = %loop_id, "Spawning merge-ralph process");

        // Redirect subprocess stdio to a log file to prevent TUI corruption.
        // If log file creation fails, fall back to Stdio::null rather than
        // inheriting the parent's terminal (which would corrupt the TUI).
        let (stdout_stdio, stderr_stdio, log_path) =
            match create_merge_subprocess_log_file(repo_root, loop_id) {
                Ok((file, path)) => match file.try_clone() {
                    Ok(file_clone) => (Stdio::from(file_clone), Stdio::from(file), Some(path)),
                    Err(e) => {
                        warn!(
                            loop_id = %loop_id,
                            error = %e,
                            "Failed to clone log file handle, subprocess output will be discarded"
                        );
                        (Stdio::null(), Stdio::null(), None)
                    }
                },
                Err(e) => {
                    warn!(
                        loop_id = %loop_id,
                        error = %e,
                        "Failed to create subprocess log file, output will be discarded"
                    );
                    (Stdio::null(), Stdio::null(), None)
                }
            };

        match Command::new(ralph_cmd)
            .current_dir(repo_root)
            .args([
                "run",
                "-c",
                ".ralph/merge-loop-config.yml",
                "-H",
                "builtin:merge-loop",
                "--exclusive",
                "--no-tui",
                "-p",
                &format!("Merge loop {} from branch ralph/{}", loop_id, loop_id),
            ])
            .env("RALPH_MERGE_LOOP_ID", loop_id)
            .stdout(stdout_stdio)
            .stderr(stderr_stdio)
            .spawn()
        {
            Ok(child) => {
                if let Some(path) = log_path {
                    info!(
                        loop_id = %loop_id,
                        pid = child.id(),
                        log_file = %path.display(),
                        "merge-ralph spawned successfully"
                    );
                } else {
                    info!(
                        loop_id = %loop_id,
                        pid = child.id(),
                        "merge-ralph spawned successfully"
                    );
                }
                children.push(child);
            }
            Err(e) => {
                warn!(
                    loop_id = %loop_id,
                    error = %e,
                    "Failed to spawn merge-ralph, loop will remain queued for manual retry"
                );
            }
        }
    }

    // Synchronously wait for every spawned merge-ralph to exit. This binds
    // child lifetime to the parent's stack frame so the redirected stdout
    // file descriptors are flushed and closed by the OS before we return,
    // and so no orphans are leaked. Per-child wait is sequential in FIFO
    // order so logs are rotated/picked up deterministically.
    for mut child in children {
        let pid = child.id();
        match child.wait() {
            Ok(status) => {
                debug!(
                    pid = pid,
                    ?status,
                    "merge-ralph exited"
                );
            }
            Err(e) => {
                warn!(
                    pid = pid,
                    error = %e,
                    "Failed to wait for merge-ralph; it may have been reaped elsewhere"
                );
                // Best-effort: try to reap to avoid zombie. If wait() failed
                // it's likely already reaped, so try_wait is harmless.
                let _ = child.try_wait();
            }
        }
    }
}

/// Creates a timestamped log file for a merge subprocess under `.ralph/diagnostics/logs/`.
///
/// Uses the loop_id in the filename for easier identification when debugging.
/// Participates in the existing log rotation scheme.
pub fn create_merge_subprocess_log_file(
    repo_root: &Path,
    loop_id: &str,
) -> std::io::Result<(File, PathBuf)> {
    use chrono::Local;

    let logs_dir = repo_root.join(".ralph").join("diagnostics").join("logs");
    fs::create_dir_all(&logs_dir)?;

    let _ = ralph_core::diagnostics::rotate_logs(&logs_dir, 10);

    let timestamp = Local::now().format("%Y-%m-%dT%H-%M-%S");
    let log_path = logs_dir.join(format!("ralph-merge-{}-{}.log", loop_id, timestamp));
    let file = File::create(&log_path)?;

    Ok((file, log_path))
}

pub fn process_pending_merges(repo_root: &Path) {
    process_pending_merges_with_command(repo_root, OsStr::new("ralph"));
}

/// Public wrapper for CLI invocation of process_pending_merges.
///
/// Called by `ralph loops process` command to process the merge queue.
pub fn process_pending_merges_cli(repo_root: &Path) {
    process_pending_merges(repo_root);
}
