use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;

/// Arguments for the `ralph tui` subcommand.
#[derive(Parser, Debug)]
pub struct TuiArgs {
    /// ralph-api server URL to connect to.
    /// Defaults to RALPH_API_URL env var, or http://127.0.0.1:3000.
    #[arg(short = 'u', long = "url")]
    url: Option<String>,
}

pub async fn tui_command(args: TuiArgs) -> Result<()> {
    use ralph_tui::Tui;

    let url = args
        .url
        .or_else(|| std::env::var("RALPH_API_URL").ok())
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());

    info!(url = %url, "Attaching TUI to ralph-api server");

    let tui =
        Tui::connect(&url).with_context(|| format!("Failed to create TUI client for {url}"))?;

    tui.run().await.context("TUI exited with error")
}
