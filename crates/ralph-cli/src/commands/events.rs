use crate::cli::{ColorMode, OutputFormat, resolve_marker_target, resolve_workspace_root};
use crate::display::colors;
use anyhow::{Result, bail};
use clap::Parser;
use ralph_core::EventHistory;
use std::fs;
use std::path::PathBuf;

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

pub fn events_command(color_mode: ColorMode, args: EventsArgs) -> Result<()> {
    let use_colors = color_mode.should_use_colors();
    let workspace_root = resolve_workspace_root(None);
    let current_events_marker = workspace_root.join(".ralph/current-events");

    // Read events path from marker file, fall back to default if marker doesn't exist
    // This ensures `ralph events` reads from the same events file as the active run
    let history = match args.file {
        Some(path) => EventHistory::new(path),
        None => fs::read_to_string(&current_events_marker)
            .map(|s| EventHistory::new(resolve_marker_target(&workspace_root, &s)))
            .unwrap_or_else(|_| EventHistory::new(workspace_root.join(".ralph/events.jsonl"))),
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
}
