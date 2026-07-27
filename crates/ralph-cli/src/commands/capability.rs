//! `ralph capability inventory` subcommand.

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::io::Write;

#[derive(Parser, Debug)]
pub struct CapabilityArgs {
    #[command(subcommand)]
    pub command: CapabilityCommands,
}

#[derive(Subcommand, Debug)]
pub enum CapabilityCommands {
    /// List preset-facing capabilities and their audit status.
    Inventory(InventoryArgs),
}

#[derive(Parser, Debug)]
pub struct InventoryArgs {
    /// Output format (json / human).
    #[arg(long, value_enum, default_value_t = InventoryFormat::Json)]
    pub format: InventoryFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum InventoryFormat {
    Human,
    Json,
}

#[derive(Serialize)]
struct InventoryView {
    version: &'static str,
    capabilities: Vec<ralph_core::capability_inventory::Capability>,
}

/// Execute `ralph capability inventory`.
pub fn execute(args: CapabilityArgs, _use_colors: bool) -> Result<()> {
    let CapabilityArgs { command } = args;
    match command {
        CapabilityCommands::Inventory(inv_args) => execute_inventory(inv_args),
    }
}

fn execute_inventory(args: InventoryArgs) -> Result<()> {
    let caps = ralph_core::capability_inventory::capability_inventory();
    match args.format {
        InventoryFormat::Json => {
            let view = InventoryView {
                version: "capability_inventory/v1",
                capabilities: caps,
            };
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            serde_json::to_writer_pretty(&mut handle, &view)?;
            writeln!(handle)?;
        }
        InventoryFormat::Human => {
            println!("Capability Inventory");
            println!("=====================");
            for c in caps {
                println!("\n[{}] ({})", c.id, c.recommended_evidence_level);
                println!("  Trigger signal: {}", c.trigger_signal);
                println!("  Applies when:   {}", c.applies_when);
                println!("  Source:         {}", c.source);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_capability_inventory_minimal() {
        let parsed = CapabilityArgs::try_parse_from(["capability", "inventory"]).expect("parse");
        match parsed.command {
            CapabilityCommands::Inventory(inv) => {
                assert_eq!(inv.format, InventoryFormat::Json);
            }
        }
    }

    #[test]
    fn cli_parses_capability_inventory_human() {
        let parsed =
            CapabilityArgs::try_parse_from(["capability", "inventory", "--format", "human"])
                .expect("parse");
        match parsed.command {
            CapabilityCommands::Inventory(inv) => {
                assert_eq!(inv.format, InventoryFormat::Human);
            }
        }
    }
}
