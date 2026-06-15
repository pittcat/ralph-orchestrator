//! Per-hat write channel helpers for isolated mode.
//!
//! In isolated execution mode, each hat activation gets its own temporary
//! events file. The runner points `RALPH_EVENTS_FILE` at this channel, then
//! merges the channel back into the main/candidate events file after the
//! backend exits, stamping every record with the authoritative hat id.

use super::*;
use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

/// Prepare a per-hat write channel for the current activation.
///
/// Creates the channel file and writes the `current-hat-events` marker so
/// `ralph emit` can resolve it via the allowlist. Returns the absolute path
/// to the channel file.
pub fn prepare_hat_channel(
    ctx: &LoopContext,
    hat_id: &str,
    loop_id: &str,
    iteration: u32,
) -> Result<PathBuf> {
    let channel_path = crate::loop_runner::paths::hat_channel_events_path(ctx, hat_id, loop_id, iteration);
    let relative_path = channel_path
        .strip_prefix(ctx.workspace())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| channel_path.clone());

    fs::create_dir_all(ctx.agent_dir())
        .with_context(|| format!("Failed to create agent dir: {}", ctx.agent_dir().display()))?;
    fs::File::create(&channel_path)
        .with_context(|| format!("Failed to create hat channel file: {}", channel_path.display()))?;

    let marker = crate::loop_runner::paths::current_hat_events_marker(ctx);
    fs::write(&marker, relative_path.to_string_lossy().as_bytes())
        .with_context(|| format!("Failed to write current-hat-events marker: {}", marker.display()))?;

    Ok(channel_path)
}

/// Merge the current hat channel into the target events file.
///
/// Every JSONL record has its `hat` field (and `source` mirror) overwritten
/// with `authoritative_hat`, then the record is appended to `target_file`.
/// The channel file and its marker are removed after a successful merge.
pub fn merge_hat_channel(
    ctx: &LoopContext,
    target_file: &Path,
    authoritative_hat: &str,
) -> Result<()> {
    let Some(channel_path) = crate::loop_runner::paths::resolve_hat_channel_events_path(ctx) else {
        return Ok(());
    };

    if !channel_path.exists() {
        let _ = fs::remove_file(crate::loop_runner::paths::current_hat_events_marker(ctx));
        return Ok(());
    }

    let content = fs::read_to_string(&channel_path)
        .with_context(|| format!("Failed to read hat channel: {}", channel_path.display()))?;

    if !content.trim().is_empty() {
        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create target events directory: {}", parent.display()))?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(target_file)
            .with_context(|| format!("Failed to open target events file: {}", target_file.display()))?;

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let stamped = match serde_json::from_str::<serde_json::Value>(line) {
                Ok(mut value) => {
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("hat".to_string(), serde_json::Value::String(authoritative_hat.to_string()));
                        obj.insert("source".to_string(), serde_json::Value::String(authoritative_hat.to_string()));
                    }
                    serde_json::to_string(&value)?
                }
                Err(_) => {
                    // Preserve malformed lines so the event loop's malformed-line
                    // backpressure can surface the error with proper attribution.
                    line.to_string()
                }
            };
            writeln!(file, "{}", stamped)
                .with_context(|| format!("Failed to append to target events file: {}", target_file.display()))?;
        }
    }

    fs::remove_file(&channel_path)
        .with_context(|| format!("Failed to remove hat channel file: {}", channel_path.display()))?;
    let _ = fs::remove_file(crate::loop_runner::paths::current_hat_events_marker(ctx));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_ctx(tmp: &TempDir) -> LoopContext {
        LoopContext::primary(tmp.path().to_path_buf())
    }

    #[test]
    fn test_prepare_hat_channel_creates_file_and_marker() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_ctx(&tmp);

        let path = prepare_hat_channel(&ctx, "review-coordinator", "primary-001", 3).unwrap();
        assert!(path.exists(), "channel file should exist");
        assert_eq!(path, ctx.agent_dir().join("events-hat-review-coordinator-primary-001-3.jsonl"));

        let marker = ctx.ralph_dir().join("current-hat-events");
        assert!(marker.exists(), "current-hat-events marker should exist");
        let marker_target = fs::read_to_string(&marker).unwrap();
        assert_eq!(
            marker_target,
            ".ralph/agent/events-hat-review-coordinator-primary-001-3.jsonl"
        );
    }

    #[test]
    fn test_merge_hat_channel_stamps_hat_and_source() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_ctx(&tmp);

        let channel = prepare_hat_channel(&ctx, "review-synthesizer", "primary-002", 1).unwrap();
        fs::write(
            &channel,
            r#"{"topic":"review.passed","payload":{"findings_count":47},"ts":"2026-06-15T00:00:00Z","hat":"review-coordinator","source":"review-coordinator"}
"#,
        )
        .unwrap();

        let target = tmp.path().join(".ralph/events-main.jsonl");
        merge_hat_channel(&ctx, &target, "review-synthesizer").unwrap();

        let merged = fs::read_to_string(&target).unwrap();
        assert!(merged.contains("\"topic\":\"review.passed\""));
        assert!(merged.contains("\"hat\":\"review-synthesizer\""));
        assert!(merged.contains("\"source\":\"review-synthesizer\""));
        assert!(!merged.contains("\"hat\":\"review-coordinator\""));
        assert!(!merged.contains("\"source\":\"review-coordinator\""));

        assert!(!channel.exists(), "channel file should be removed after merge");
        assert!(!ctx.ralph_dir().join("current-hat-events").exists(), "marker should be removed after merge");
    }

    #[test]
    fn test_merge_hat_channel_preserves_malformed_lines() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_ctx(&tmp);

        let channel = prepare_hat_channel(&ctx, "executor", "primary-003", 2).unwrap();
        fs::write(&channel, "not-a-json-line\n").unwrap();

        let target = tmp.path().join(".ralph/events-main.jsonl");
        merge_hat_channel(&ctx, &target, "executor").unwrap();

        let merged = fs::read_to_string(&target).unwrap();
        assert!(merged.contains("not-a-json-line"));
    }

    #[test]
    fn test_merge_hat_channel_no_marker_is_noop() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_ctx(&tmp);

        let target = tmp.path().join(".ralph/events-main.jsonl");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "existing\n").unwrap();

        merge_hat_channel(&ctx, &target, "executor").unwrap();

        let merged = fs::read_to_string(&target).unwrap();
        assert_eq!(merged.trim(), "existing");
    }
}
