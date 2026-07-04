use crate::cli::{
    ColorMode, OutputFormat, resolve_hat_channel_file, resolve_marker_target,
    resolve_workspace_root,
};
use crate::display::colors;
use crate::operation_guard::OperationContext;
use anyhow::{Result, bail};
use clap::{Parser, ValueEnum};
use ralph_core::EventHistory;
use std::fs;
use std::path::PathBuf;

/// Source of events for `ralph events`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum EventsSource {
    /// Automatically select: hat-channel in agent context, main otherwise.
    #[default]
    Auto,
    /// Read from the main events ledger (current-events marker or events.jsonl).
    Main,
    /// Read from the per-hat write channel (current-hat-events marker).
    HatChannel,
}

/// Arguments for the events subcommand.
#[derive(Parser, Debug)]
pub struct EventsArgs {
    /// Show only the last N events
    #[arg(long)]
    pub last: Option<usize>,

    /// Filter by topic (e.g., "build.blocked")
    #[arg(long)]
    pub topic: Option<String>,

    /// Filter by iteration number
    #[arg(long)]
    pub iteration: Option<u32>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,

    /// Path to events file (default: auto-detects current run)
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Which events ledger to read: main loop ledger, per-hat channel, or auto-detect.
    #[arg(long, value_enum, default_value_t = EventsSource::Auto)]
    pub events_source: EventsSource,

    /// Clear the event history. Requires --confirm <loop_id> to match the
    /// active loop, so accidental or agent-triggered clears are blocked.
    #[arg(long)]
    pub clear: bool,

    /// Confirmation token: must equal the active loop id (or "current"
    /// / "default" when no loop marker exists) to authorize --clear.
    #[arg(long, value_name = "LOOP_ID")]
    pub confirm: Option<String>,
}

/// P8: validate the `--confirm` token for `ralph events --clear`.
/// Returns `Ok(())` when the token matches the active loop, or an
/// `anyhow::Error` describing the rejection otherwise. The token "current"
/// or "default" is accepted when no loop marker exists.
pub(crate) fn check_events_clear_confirm(
    confirm: Option<&str>,
    active_loop_id: Option<&str>,
) -> Result<()> {
    match confirm {
        None => bail!(
            "Refusing to clear event history without --confirm <loop_id>. \
             Pass `--confirm {id}` (where {id} is the active loop id) to authorize the clear.",
            id = active_loop_id.unwrap_or("current")
        ),
        Some("") => bail!("Refusing to clear event history with an empty --confirm value."),
        Some(provided) => {
            let matches = match active_loop_id {
                Some(active) => provided == active,
                None => provided == "current" || provided == "default",
            };
            if !matches {
                bail!(
                    "Refusing to clear event history: --confirm {provided:?} does not match \
                     the active loop ({}). Re-run with the correct loop id to authorize the clear.",
                    active_loop_id.unwrap_or("current")
                );
            }
            Ok(())
        }
    }
}

const HAT_EVENTS_MARKER: &str = ".ralph/current-hat-events";

/// Resolve the events file path according to the requested source.
///
/// - `Main` always returns the main loop ledger (`current-events` marker or `.ralph/events.jsonl`).
/// - `HatChannel` returns the path from `.ralph/current-hat-events`; errors if the marker is missing.
/// - `Auto` returns the hat-channel path when running in an agent context and a marker exists;
///   otherwise falls back to the main ledger. A missing/empty hat-channel in agent context logs
///   a warning and falls back to main, matching the hat-channel routing fallback diagnostics.
pub fn resolve_events_source(ctx: &OperationContext, source_arg: EventsSource) -> Result<PathBuf> {
    let main_path = ctx
        .resolve_accepted_events_path()
        .unwrap_or_else(|| ctx.workspace_root.join(".ralph/events.jsonl"));

    match source_arg {
        EventsSource::Main => Ok(main_path),
        EventsSource::HatChannel => match resolve_hat_channel_path(ctx) {
            Some(path) => Ok(path),
            None => bail!(
                "No hat-channel marker found at {}. \
                 `--events-source hat-channel` can only be used inside an isolated hat activation.",
                ctx.workspace_root.join(HAT_EVENTS_MARKER).display()
            ),
        },
        EventsSource::Auto => {
            if ctx.is_agent() {
                if let Some(hat_path) = resolve_hat_channel_path(ctx) {
                    if hat_path.exists()
                        && fs::metadata(&hat_path)
                            .map(|m| m.len() > 0)
                            .unwrap_or(false)
                    {
                        return Ok(hat_path);
                    }
                    // Empty or missing channel file: warn and fall back to main.
                    tracing::warn!(
                        hat_channel = %hat_path.display(),
                        fallback = %main_path.display(),
                        "agent context hat-channel is empty or missing; falling back to main events"
                    );
                } else {
                    tracing::warn!(
                        marker = %ctx.workspace_root.join(HAT_EVENTS_MARKER).display(),
                        fallback = %main_path.display(),
                        "agent context has no current-hat-events marker; falling back to main events"
                    );
                }
            }
            Ok(main_path)
        }
    }
}

/// Resolve the per-hat channel events path from the marker, if present and non-empty.
/// Thin re-export over the `cli::resolve_hat_channel_file` helper so the
/// marker-read lives in exactly one place (used to be duplicated by U7 fix).
fn resolve_hat_channel_path(ctx: &OperationContext) -> Option<PathBuf> {
    resolve_hat_channel_file(&ctx.workspace_root).map(|(path, _exists)| path)
}

pub fn events_command(color_mode: ColorMode, args: EventsArgs) -> Result<()> {
    let use_colors = color_mode.should_use_colors();
    let workspace_root = resolve_workspace_root(None);
    let ctx = OperationContext::detect(workspace_root.clone());

    // Explicit --file always wins. Otherwise resolve from --events-source.
    let history = match args.file {
        Some(path) => EventHistory::new(path),
        None => EventHistory::new(resolve_events_source(&ctx, args.events_source)?),
    };

    // Handle clear command. P8: require --confirm <loop_id> matching the
    // active loop (or "current" / "default" when no loop marker exists).
    if args.clear {
        let active_loop_id = fs::read_to_string(workspace_root.join(".ralph/current-loop-id"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        check_events_clear_confirm(args.confirm.as_deref(), active_loop_id.as_deref())?;
        tracing::warn!(
            events_path = %history.path().display(),
            confirm = ?args.confirm,
            active_loop_id = ?active_loop_id,
            "Clearing event history after explicit confirmation"
        );
        history.clear()?;
        if use_colors {
            println!("{}✓{} Event history cleared", colors::GREEN, colors::RESET);
        } else {
            println!("Event history cleared");
        }
        return Ok(());
    }

    if !history.exists() {
        if use_colors {
            println!(
                "{}No event history found.{} Run `ralph` to generate events.",
                colors::DIM,
                colors::RESET
            );
        } else {
            println!("No event history found. Run `ralph` to generate events.");
        }
        return Ok(());
    }

    // Read and filter events
    let mut records = history.read_all()?;

    // Apply filters in sequence
    if let Some(ref topic) = args.topic {
        records.retain(|r| r.topic == *topic);
    }

    if let Some(iteration) = args.iteration {
        records.retain(|r| r.iteration == iteration);
    }

    // Apply 'last' filter after other filters (to get last N of filtered results)
    if let Some(n) = args.last
        && records.len() > n
    {
        records = records.into_iter().rev().take(n).rev().collect();
    }

    if records.is_empty() {
        if use_colors {
            println!("{}No matching events found.{}", colors::DIM, colors::RESET);
        } else {
            println!("No matching events found.");
        }
        return Ok(());
    }

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&records)?;
            println!("{json}");
        }
        OutputFormat::Table => {
            crate::display::print_events_table(&records, use_colors);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_guard::OperationContext;
    use std::fs;
    use tempfile::TempDir;

    fn empty_env() -> impl Fn(&str) -> Option<String> {
        |_| None
    }

    fn env_with(key: &'static str, value: &'static str) -> impl Fn(&str) -> Option<String> {
        move |k| {
            if k == key {
                Some(value.to_string())
            } else {
                None
            }
        }
    }

    #[test]
    fn test_events_clear_without_confirm_rejected() {
        let result = check_events_clear_confirm(None, Some("loop-1"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--confirm"));
    }

    #[test]
    fn test_events_clear_empty_confirm_rejected() {
        let result = check_events_clear_confirm(Some(""), Some("loop-1"));
        assert!(result.is_err());
    }

    #[test]
    fn test_events_clear_wrong_loop_confirm_rejected() {
        let result = check_events_clear_confirm(Some("loop-other"), Some("loop-1"));
        assert!(result.is_err());
    }

    #[test]
    fn test_events_clear_matching_loop_confirm_succeeds() {
        let result = check_events_clear_confirm(Some("loop-1"), Some("loop-1"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_events_clear_no_loop_marker_requires_literal_confirm_current_or_default() {
        let result = check_events_clear_confirm(Some("current"), None);
        assert!(result.is_ok());
        let result = check_events_clear_confirm(Some("default"), None);
        assert!(result.is_ok());
        let result = check_events_clear_confirm(Some("loop-1"), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_events_source_agent_auto_prefers_hat_channel() {
        let tmp = TempDir::new().unwrap();
        let ralph_dir = tmp.path().join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        fs::create_dir_all(&agent_dir).unwrap();
        let channel = agent_dir.join("events-hat-executor-loop-1.jsonl");
        fs::write(&channel, r#"{"topic":"work.ready","payload":{}}"#).unwrap();
        fs::write(
            ralph_dir.join("current-hat-events"),
            ".ralph/agent/events-hat-executor-loop-1.jsonl",
        )
        .unwrap();

        let ctx = OperationContext::detect_with_env(
            tmp.path().to_path_buf(),
            env_with("RALPH_CURRENT_HAT", "executor"),
        );
        let resolved = resolve_events_source(&ctx, EventsSource::Auto).unwrap();
        assert_eq!(resolved, channel);
    }

    #[test]
    fn test_resolve_events_source_human_auto_uses_main_events() {
        let tmp = TempDir::new().unwrap();
        let ctx = OperationContext::detect_with_env(tmp.path().to_path_buf(), empty_env());
        let resolved = resolve_events_source(&ctx, EventsSource::Auto).unwrap();
        assert_eq!(resolved, tmp.path().join(".ralph/events.jsonl"));
    }

    #[test]
    fn test_resolve_events_source_human_auto_uses_current_events_marker() {
        let tmp = TempDir::new().unwrap();
        let ralph_dir = tmp.path().join(".ralph");
        fs::create_dir_all(&ralph_dir).unwrap();
        fs::write(
            ralph_dir.join("current-events"),
            ".ralph/events-20260704.jsonl",
        )
        .unwrap();

        let ctx = OperationContext::detect_with_env(tmp.path().to_path_buf(), empty_env());
        let resolved = resolve_events_source(&ctx, EventsSource::Auto).unwrap();
        assert_eq!(resolved, tmp.path().join(".ralph/events-20260704.jsonl"));
    }

    #[test]
    fn test_resolve_events_source_explicit_hat_channel_missing_marker_errors() {
        let tmp = TempDir::new().unwrap();
        let ctx = OperationContext::detect_with_env(tmp.path().to_path_buf(), empty_env());
        let err = resolve_events_source(&ctx, EventsSource::HatChannel).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("hat-channel"),
            "error should mention hat-channel: {msg}"
        );
        assert!(
            msg.contains("current-hat-events"),
            "error should mention the marker: {msg}"
        );
    }

    #[test]
    fn test_resolve_events_source_agent_auto_falls_back_when_hat_channel_missing() {
        let tmp = TempDir::new().unwrap();
        let ctx = OperationContext::detect_with_env(
            tmp.path().to_path_buf(),
            env_with("RALPH_CURRENT_HAT", "executor"),
        );
        let resolved = resolve_events_source(&ctx, EventsSource::Auto).unwrap();
        assert_eq!(resolved, tmp.path().join(".ralph/events.jsonl"));
    }

    #[test]
    fn test_resolve_events_source_roundtrip_emit_then_events() {
        let tmp = TempDir::new().unwrap();
        let ralph_dir = tmp.path().join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        fs::create_dir_all(&agent_dir).unwrap();
        let channel = agent_dir.join("events-hat-executor-loop-2.jsonl");
        fs::File::create(&channel).unwrap();
        fs::write(
            ralph_dir.join("current-hat-events"),
            ".ralph/agent/events-hat-executor-loop-2.jsonl",
        )
        .unwrap();

        // Simulate `ralph emit` writing a business event into the hat-channel.
        fs::write(&channel, r#"{"topic":"work.ready","payload":{}}"#).unwrap();

        let ctx = OperationContext::detect_with_env(
            tmp.path().to_path_buf(),
            env_with("RALPH_CURRENT_HAT", "executor"),
        );
        let resolved = resolve_events_source(&ctx, EventsSource::Auto).unwrap();
        assert_eq!(resolved, channel);
        let content = fs::read_to_string(&resolved).unwrap();
        assert!(content.contains("work.ready"));
    }
}
