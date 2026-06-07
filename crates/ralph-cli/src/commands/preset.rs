//! CLI commands for the `ralph preset` namespace.
//!
//! Preset contract validation and inspection.
//!
//! Subcommands:
//! - `check`: Run preset/workflow contract validation (config, topology, payload, orphan)

use crate::display::colors;
use crate::preflight;
use crate::{ConfigSource, HatsSource};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use ralph_core::HatRegistry;
use ralph_core::runtime_contract::{
    FindingSeverity, RuntimeContractReport, RuntimeContractStrictness,
};

/// Manage and validate presets.
#[derive(Parser, Debug)]
pub struct PresetArgs {
    #[command(subcommand)]
    pub command: Option<PresetCommands>,
}

#[derive(Subcommand, Debug)]
pub enum PresetCommands {
    /// Check preset/workflow contract (config, topology, payload, orphan)
    Check {
        /// Output format (human or json)
        #[arg(long, value_enum, default_value_t = PresetCheckFormat::Human)]
        format: PresetCheckFormat,

        /// Enable strict mode: payload_strict=true AND fail_on_warnings=true.
        /// Warnings cause failure; missing schemas are errors.
        #[arg(long)]
        strict: bool,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum PresetCheckFormat {
    Human,
    Json,
}

/// Execute a preset command.
pub async fn execute(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    args: PresetArgs,
    use_colors: bool,
) -> Result<()> {
    match args.command {
        Some(PresetCommands::Check { format, strict }) => {
            check_preset(config_sources, hats_source, format, strict, use_colors).await
        }
        None => {
            // Default to check with current config
            check_preset(
                config_sources,
                hats_source,
                PresetCheckFormat::Human,
                false,
                use_colors,
            )
            .await
        }
    }
}

async fn check_preset(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    format: PresetCheckFormat,
    strict: bool,
    use_colors: bool,
) -> Result<()> {
    let source_label = preset_source_label(config_sources, hats_source);
    let config = preflight::load_config_for_preflight(config_sources, hats_source)
        .await
        .context("Failed to load config for preset check")?;

    let registry = HatRegistry::from_runtime_config(&config);

    let strictness = if strict {
        RuntimeContractStrictness::preset_check_strict()
    } else {
        RuntimeContractStrictness::default()
    };

    let report = ralph_core::runtime_contract::RuntimeContractAggregator::aggregate(
        &source_label,
        &config,
        &registry,
        strictness,
    );

    match format {
        PresetCheckFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        PresetCheckFormat::Human => {
            print_human_report(&report, use_colors);
        }
    }

    if !report.passed {
        std::process::exit(1);
    }

    Ok(())
}

fn preset_source_label(config_sources: &[ConfigSource], hats_source: Option<&HatsSource>) -> String {
    if let Some(source) = hats_source {
        return source.label().to_string();
    }
    // Use the first file-based config source as label
    for source in config_sources {
        if let ConfigSource::File(path) = source {
            return path.to_string_lossy().to_string();
        }
    }
    "current-config".to_string()
}

fn print_human_report(report: &RuntimeContractReport, use_colors: bool) {
    println!("Preset Contract Check: {}", report.source_label);
    println!();

    // Group findings by source
    let mut config_findings = Vec::new();
    let mut topology_findings = Vec::new();
    let mut orphan_findings = Vec::new();
    let mut payload_findings = Vec::new();

    for finding in &report.findings {
        match finding.source {
            ralph_core::runtime_contract::FindingSource::Config => {
                config_findings.push(finding);
            }
            ralph_core::runtime_contract::FindingSource::Topology => {
                topology_findings.push(finding);
            }
            ralph_core::runtime_contract::FindingSource::Orphan => {
                orphan_findings.push(finding);
            }
            ralph_core::runtime_contract::FindingSource::Payload => {
                payload_findings.push(finding);
            }
            ralph_core::runtime_contract::FindingSource::Preflight => {
                // Should not appear in core aggregator output
            }
        }
    }

    // Print Config section
    println!("Config:");
    if config_findings.is_empty() {
        print_finding_line(use_colors, FindingSeverity::Pass, "No config issues");
    } else {
        for finding in &config_findings {
            print_finding_line(use_colors, finding.severity, &finding.message);
        }
    }
    println!();

    // Print Topology section
    println!("Topology:");
    if topology_findings.is_empty() {
        print_finding_line(use_colors, FindingSeverity::Pass, "Topology valid");
    } else {
        for finding in &topology_findings {
            print_finding_line(use_colors, finding.severity, &finding.message);
        }
    }
    println!();

    // Print Orphan Topics section
    println!("Orphan Topics:");
    if orphan_findings.is_empty() {
        print_finding_line(use_colors, FindingSeverity::Pass, "No orphan topics");
    } else {
        for finding in &orphan_findings {
            print_finding_line(use_colors, finding.severity, &finding.message);
        }
    }
    println!();

    // Print Payload Contract section
    println!("Payload Contract:");
    if payload_findings.is_empty() {
        print_finding_line(use_colors, FindingSeverity::Pass, "Payload contract valid");
    } else {
        for finding in &payload_findings {
            print_finding_line(use_colors, finding.severity, &finding.message);
        }
    }
    println!();

    // Print Summary
    println!("Summary:");
    let result = if report.passed { "PASS" } else { "FAIL" };
    let mut details = Vec::new();
    if report.errors > 0 {
        details.push(format!("{} error(s)", report.errors));
    }
    if report.warnings > 0 {
        details.push(format!("{} warning(s)", report.warnings));
    }
    let detail_text = if details.is_empty() {
        String::new()
    } else {
        format!(" ({})", details.join(", "))
    };

    if use_colors {
        let color = if report.passed {
            colors::GREEN
        } else {
            colors::RED
        };
        println!(
            "Result: {color}{result}{reset}{detail}",
            reset = colors::RESET,
            detail = detail_text,
        );
    } else {
        println!("Result: {result}{detail}", detail = detail_text);
    }

    // Print strictness info
    if report.payload_strict || report.fail_on_warnings {
        println!();
        println!("Strictness:");
        if report.payload_strict {
            println!("  payload_strict: true");
        }
        if report.fail_on_warnings {
            println!("  fail_on_warnings: true");
        }
    }
}

fn print_finding_line(use_colors: bool, severity: FindingSeverity, msg: &str) {
    if use_colors {
        match severity {
            FindingSeverity::Pass => {
                println!("  [{}ok{}] {}", colors::GREEN, colors::RESET, msg);
            }
            FindingSeverity::Warn => {
                println!("  [{}warn{}] {}", colors::YELLOW, colors::RESET, msg);
            }
            FindingSeverity::Error => {
                println!("  [{}err{}] {}", colors::RED, colors::RESET, msg);
            }
        }
    } else {
        match severity {
            FindingSeverity::Pass => println!("  [ok] {}", msg),
            FindingSeverity::Warn => println!("  [warn] {}", msg),
            FindingSeverity::Error => println!("  [err] {}", msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::runtime_contract::{RuntimeContractFinding, RuntimeContractReport};

    #[test]
    fn preset_source_label_from_hats_source() {
        let hats_source = HatsSource::Builtin("ce-executor".to_string());
        let label = preset_source_label(&[], Some(&hats_source));
        assert_eq!(label, "builtin:ce-executor");
    }

    #[test]
    fn preset_source_label_from_config_file() {
        let config_sources = vec![ConfigSource::File("my-preset.yml".into())];
        let label = preset_source_label(&config_sources, None);
        assert_eq!(label, "my-preset.yml");
    }

    #[test]
    fn preset_source_label_default() {
        let label = preset_source_label(&[], None);
        assert_eq!(label, "current-config");
    }

    #[test]
    fn human_report_empty_findings() {
        let report = RuntimeContractReport::new("test", RuntimeContractStrictness::default());
        // Should not panic
        print_human_report(&report, false);
    }

    #[test]
    fn human_report_with_findings() {
        let mut report = RuntimeContractReport::new("test", RuntimeContractStrictness::default());
        report.add_finding(
            RuntimeContractFinding::new(
                "topology.unreachable_completion",
                ralph_core::runtime_contract::FindingSource::Topology,
                ralph_core::runtime_contract::FindingSeverity::Error,
                ralph_core::runtime_contract::FindingStage::Authoring,
                "completion promise unreachable",
            )
            .with_detail("topic", "LOOP_COMPLETE"),
        );
        // Should not panic
        print_human_report(&report, true);
    }
}
