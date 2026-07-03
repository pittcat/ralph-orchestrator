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
    let channel_path =
        crate::loop_runner::paths::hat_channel_events_path(ctx, hat_id, loop_id, iteration);
    let relative_path = channel_path
        .strip_prefix(ctx.workspace())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| channel_path.clone());

    fs::create_dir_all(ctx.agent_dir())
        .with_context(|| format!("Failed to create agent dir: {}", ctx.agent_dir().display()))?;
    fs::File::create(&channel_path).with_context(|| {
        format!(
            "Failed to create hat channel file: {}",
            channel_path.display()
        )
    })?;

    let marker = crate::loop_runner::paths::current_hat_events_marker(ctx);
    fs::write(&marker, relative_path.to_string_lossy().as_bytes()).with_context(|| {
        format!(
            "Failed to write current-hat-events marker: {}",
            marker.display()
        )
    })?;

    Ok(channel_path)
}

/// Merge the current hat channel into the target events file.
///
/// Every JSONL record has its `hat` field (and `source` mirror) overwritten
/// with `authoritative_hat`. In isolated mode, when `config` is provided and
/// the record has no explicit `triggered` field, the runner backfills it from
/// the topic's registered subscriber. This prevents the round-robin "next hat"
/// from leaking into `triggered` (e.g. `review.dimension.ready` being tagged
/// with `shipper` instead of `dimension-reviewer`).
/// The channel file and its marker are removed after a successful merge.
pub fn merge_hat_channel(
    ctx: &LoopContext,
    target_file: &Path,
    authoritative_hat: &str,
    config: Option<&RalphConfig>,
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
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create target events directory: {}",
                    parent.display()
                )
            })?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(target_file)
            .with_context(|| {
                format!(
                    "Failed to open target events file: {}",
                    target_file.display()
                )
            })?;

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let stamped = match serde_json::from_str::<serde_json::Value>(line) {
                Ok(mut value) => {
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert(
                            "hat".to_string(),
                            serde_json::Value::String(authoritative_hat.to_string()),
                        );
                        obj.insert(
                            "source".to_string(),
                            serde_json::Value::String(authoritative_hat.to_string()),
                        );
                        // Backfill `triggered` from the topic's real subscriber
                        // when the agent did not provide one.
                        if !obj.contains_key("triggered") {
                            if let Some(topic) = obj
                                .get("topic")
                                .and_then(|v| v.as_str())
                            {
                                if let Some(derived) = config
                                    .and_then(|c| derive_triggered_for_topic(topic, c))
                                {
                                    obj.insert(
                                        "triggered".to_string(),
                                        serde_json::Value::String(derived),
                                    );
                                }
                            }
                        }
                    }
                    serde_json::to_string(&value)?
                }
                Err(_) => {
                    // Preserve malformed lines so the event loop's malformed-line
                    // backpressure can surface the error with proper attribution.
                    line.to_string()
                }
            };
            writeln!(file, "{}", stamped).with_context(|| {
                format!(
                    "Failed to append to target events file: {}",
                    target_file.display()
                )
            })?;
        }
    } else {
        // 2026-07-03-002 plan U4: hat-channel 0 字节文件不再静默跳过。
        // emit 诊断到 .ralph/diagnostics/channel-routing-fallback-{ts}.md,
        // 让 operator 能看到 isolated 模式 hat-channel 路由失效。不
        // fail-closed(避免阻塞 loop),但升级日志级别为 error。
        emit_channel_routing_fallback_diagnostic(
            ctx,
            authoritative_hat,
            "hat_channel_empty_after_activation",
        );
    }

    fs::remove_file(&channel_path).with_context(|| {
        format!(
            "Failed to remove hat channel file: {}",
            channel_path.display()
        )
    })?;
    let _ = fs::remove_file(crate::loop_runner::paths::current_hat_events_marker(ctx));

    Ok(())
}

/// Derive the `triggered` field for a business topic from the handoff index.
///
/// The handoff index records topics with a unique downstream consumer.
/// Multi-consumer topics, wildcard subscribers, and control/diagnostic
/// topics intentionally leave `triggered` unset so we do not misattribute
/// a target.
fn derive_triggered_for_topic(topic: &str, config: &RalphConfig) -> Option<String> {
    if ralph_core::event_origin::is_ralph_control_topic(topic)
        || ralph_core::is_orchestrator_diagnostic_topic(topic)
    {
        return None;
    }
    let index = ralph_core::workflow_contract::HandoffIndex::from_config(config);
    index.consumer_of(topic).map(|id| id.to_string())
}

/// 2026-07-03-002 plan U4: emit a diagnostic file when hat-channel routing
/// falls back (0-byte channel file or merge failure). Not fail-closed —
/// the loop continues on the main events fallback path, but the operator
/// gets a visible artifact in `.ralph/diagnostics/` and an `error!` log.
pub(crate) fn emit_channel_routing_fallback_diagnostic(
    ctx: &LoopContext,
    hat_id: &str,
    reason: &str,
) {
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S");
    let diagnostics_dir = ctx.ralph_dir().join("diagnostics");
    let _ = fs::create_dir_all(&diagnostics_dir);
    let path = diagnostics_dir.join(format!("channel-routing-fallback-{}.md", ts));
    let content = format!(
        "# Hat-Channel Routing Fallback\n\n\
         - **hat**: {}\n\
         - **reason**: {}\n\
         - **timestamp**: {}\n\
         - **impact**: isolated mode hat-channel routing failed; events fall back to main events.jsonl\n\
         - **action**: check whether `prepare_hat_channel` was interrupted by a hat crash/timeout; \
           verify `.ralph/current-hat-events` marker is not stale (pointing at a prior hat's channel)\n",
        hat_id, reason, ts
    );
    let _ = fs::write(&path, content);
    tracing::error!(
        hat = %hat_id,
        reason = %reason,
        diagnostic_path = %path.display(),
        "hat-channel routing fallback (see diagnostic file)"
    );
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
        assert_eq!(
            path,
            ctx.agent_dir()
                .join("events-hat-review-coordinator-primary-001-3.jsonl")
        );

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
        merge_hat_channel(&ctx, &target, "review-synthesizer", None
        ).unwrap();

        let merged = fs::read_to_string(&target).unwrap();
        assert!(merged.contains("\"topic\":\"review.passed\""));
        assert!(merged.contains("\"hat\":\"review-synthesizer\""));
        assert!(merged.contains("\"source\":\"review-synthesizer\""));
        assert!(!merged.contains("\"hat\":\"review-coordinator\""));
        assert!(!merged.contains("\"source\":\"review-coordinator\""));

        assert!(
            !channel.exists(),
            "channel file should be removed after merge"
        );
        assert!(
            !ctx.ralph_dir().join("current-hat-events").exists(),
            "marker should be removed after merge"
        );
    }

    #[test]
    fn test_merge_hat_channel_preserves_malformed_lines() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_ctx(&tmp);

        let channel = prepare_hat_channel(&ctx, "executor", "primary-003", 2).unwrap();
        fs::write(&channel, "not-a-json-line\n").unwrap();

        let target = tmp.path().join(".ralph/events-main.jsonl");
        merge_hat_channel(&ctx, &target, "executor", None
        ).unwrap();

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

        merge_hat_channel(&ctx, &target, "executor", None
        ).unwrap();

        let merged = fs::read_to_string(&target).unwrap();
        assert_eq!(merged.trim(), "existing");
    }

    #[test]
    fn test_merge_hat_channel_backfills_triggered_from_subscriber() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_ctx(&tmp);

        let mut config = RalphConfig::default();
        config.event_loop.execution_mode =
            ralph_core::config::HatExecutionMode::Isolated;
        config.hats.insert(
            "review-coordinator".to_string(),
            ralph_core::HatConfig {
                name: "Review Coordinator".to_string(),
                triggers: vec!["review.start".to_string()],
                publishes: vec![
                    "review.dimension.ready".to_string(),
                    "review.dimensions.complete".to_string(),
                ],
                ..Default::default()
            },
        );
        config.hats.insert(
            "dimension-reviewer".to_string(),
            ralph_core::HatConfig {
                name: "Dimension Reviewer".to_string(),
                triggers: vec!["review.dimension.ready".to_string()],
                publishes: vec![
                    "review.dimension.done".to_string(),
                    "review.dimension.failed".to_string(),
                ],
                ..Default::default()
            },
        );

        let channel = prepare_hat_channel(&ctx, "review-coordinator", "primary-004", 1
        )
        .unwrap();
        fs::write(
            &channel,
            r#"{"topic":"review.dimension.ready","payload":{"dimension":"goal-alignment"},"ts":"2026-07-03T00:00:00Z"}
{"topic":"loop.cancel","payload":{},"ts":"2026-07-03T00:00:01Z"}
{"topic":"review.dimension.ready","payload":{"dimension":"correctness"},"ts":"2026-07-03T00:00:02Z","triggered":"explicit-reviewer"}
"#,
        )
        .unwrap();

        let target = tmp.path().join(".ralph/events-main.jsonl");
        merge_hat_channel(&ctx, &target, "review-coordinator", Some(&config)
        )
        .unwrap();

        let merged = fs::read_to_string(&target).unwrap();
        let lines: Vec<&str> = merged.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].contains("\"triggered\":\"dimension-reviewer\""),
            "missing triggered should be backfilled from subscriber: {}",
            lines[0]
        );
        assert!(
            !lines[1].contains("\"triggered\""),
            "control topic should stay without triggered: {}",
            lines[1]
        );
        assert!(
            lines[2].contains("\"triggered\":\"explicit-reviewer\""),
            "explicit triggered should be preserved: {}",
            lines[2]
        );
    }

    #[test]
    fn test_merge_hat_channel_empty_file_emits_diagnostic() {
        // 2026-07-03-002 plan U4: 0 字节 channel 文件不再静默跳过,
        // 必须 emit 诊断文件到 .ralph/diagnostics/channel-routing-fallback-*.md
        let tmp = TempDir::new().unwrap();
        let ctx = make_ctx(&tmp);

        // prepare 创建空文件,不写任何内容
        let channel = prepare_hat_channel(&ctx, "executor", "primary-005", 1).unwrap();
        assert_eq!(fs::read_to_string(&channel).unwrap(), "");

        let target = tmp.path().join(".ralph/events-main.jsonl");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "existing\n").unwrap();

        merge_hat_channel(&ctx, &target, "executor", None).unwrap();

        // target 应未被修改(空 channel 不 merge 任何内容)
        assert_eq!(fs::read_to_string(&target).unwrap(), "existing\n");

        // 诊断文件应存在
        let diagnostics_dir = ctx.ralph_dir().join("diagnostics");
        let mut entries: Vec<_> = fs::read_dir(&diagnostics_dir)
            .expect("diagnostics dir should exist")
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        assert!(
            !entries.is_empty(),
            "diagnostic file should be emitted for empty channel"
        );
        let diag_path = &entries[0];
        assert!(
            diag_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("channel-routing-fallback-"),
            "diagnostic file name should start with channel-routing-fallback-, got {:?}",
            diag_path
        );
        let diag_content = fs::read_to_string(diag_path).unwrap();
        assert!(
            diag_content.contains("hat_channel_empty_after_activation"),
            "diagnostic should mention the reason, got {}",
            diag_content
        );
        assert!(
            diag_content.contains("executor"),
            "diagnostic should mention the hat id, got {}",
            diag_content
        );

        // channel 文件和 marker 仍应被清理
        assert!(!channel.exists(), "channel file should still be removed");
        assert!(
            !ctx.ralph_dir().join("current-hat-events").exists(),
            "marker should still be removed"
        );
    }
}
