use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpCommands,
}

#[derive(Subcommand, Debug)]
pub enum McpCommands {
    /// Run the Ralph control plane as an MCP server over stdio
    Serve(ServeArgs),
}

#[derive(Parser, Debug, Default)]
pub struct ServeArgs {}

pub async fn execute(args: McpArgs) -> Result<()> {
    match args.command {
        McpCommands::Serve(_args) => {
            let mut config = ralph_api::ApiConfig::from_env()?;
            config.served_by = "ralph-mcp".to_string();
            config.auth_mode = ralph_api::AuthMode::TrustedLocal;
            config.token = None;
            ralph_api::serve_stdio(config).await
        }
    }
}
