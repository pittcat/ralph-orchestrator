//! CLI commands for the `ralph hats` namespace.
//!
//! Manage and inspect configured hats.
//!
//! Subcommands:
//! - `list`: Show all configured hats (Name, Description)
//! - `show`: Show detailed configuration for a specific hat

use crate::backend_support;
use crate::display::colors;
use crate::preflight;
use crate::{ConfigSource, HatsSource};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};
use ralph_adapters::{CliBackend, detect_backend_default};
use ralph_core::runtime_contract::{FindingSource, RuntimeContractStrictness};
use ralph_core::{HatRegistry, RalphConfig, truncate_with_ellipsis};
use std::collections::HashSet;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Manage configured hats.
#[derive(Parser, Debug)]
pub struct HatsArgs {
    #[command(subcommand)]
    pub command: Option<HatsCommands>,
}

#[derive(Subcommand, Debug)]
pub enum HatsCommands {
    /// Validate hat topology and payload contracts
    Validate {
        /// Strict payload contract validation: missing schemas are errors (not warnings)
        #[arg(long)]
        strict: bool,
    },
    /// Display hat topology graph
    Graph {
        /// Output format (unicode, ascii, compact, mermaid)
        #[arg(long, default_value = "unicode")]
        format: GraphFormat,
        /// Backend for AI-generated diagrams (claude, gemini, codex, opencode, pi, custom)
        #[arg(short = 'b', long = "backend")]
        backend: Option<String>,
    },
    /// List all configured hats (default if no subcommand)
    List {
        /// Output format (table, json)
        #[arg(long, default_value = "table")]
        format: ListFormat,
    },
    /// Show detailed configuration for a specific hat
    Show(ShowArgs),
}

#[derive(ValueEnum, Clone, Debug, Default)]
pub enum GraphFormat {
    /// Unicode box-drawing characters (┌─┐│└┘▶) - best appearance
    #[default]
    Unicode,
    /// Pure ASCII characters (+--| chars) - maximum compatibility
    Ascii,
    /// Compact single-glyph nodes - minimal output
    Compact,
    /// Raw Mermaid syntax - for external rendering tools
    Mermaid,
}

#[derive(ValueEnum, Clone, Debug, Default)]
pub enum ListFormat {
    #[default]
    Table,
    Json,
}

#[derive(Parser, Debug)]
pub struct ShowArgs {
    /// Name of the hat to show (ID or display name)
    pub name: String,
}

/// Execute a hats command.
pub async fn execute(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    args: HatsArgs,
    use_colors: bool,
) -> Result<()> {
    let config = preflight::load_config_for_preflight(config_sources, hats_source)
        .await
        .context("Failed to load config for hats")?;

    let config_registry = HatRegistry::from_config(&config);
    let runtime_registry = HatRegistry::from_runtime_config(&config);
    let mut stdout = std::io::stdout();

    match args.command {
        None
        | Some(HatsCommands::List {
            format: ListFormat::Table,
        }) => list_hats(&mut stdout, &config_registry, use_colors),
        Some(HatsCommands::List {
            format: ListFormat::Json,
        }) => list_hats_json(&mut stdout, &config_registry),
        Some(HatsCommands::Show(show_args)) => {
            show_hat(&mut stdout, &config_registry, &show_args.name, use_colors)
        }
        Some(HatsCommands::Validate { strict }) => validate_hats(
            &mut stdout,
            &config,
            &runtime_registry,
            &config_registry,
            use_colors,
            strict,
        ),
        Some(HatsCommands::Graph { format, backend }) => graph_hats(
            &mut stdout,
            &config,
            &runtime_registry,
            format,
            backend.as_deref(),
        ),
    }
}

fn list_hats_json<W: Write>(writer: &mut W, config_registry: &HatRegistry) -> Result<()> {
    let hats: Vec<_> = config_registry.all().collect();
    serde_json::to_writer_pretty(&mut *writer, &hats)?;
    writeln!(writer)?;
    Ok(())
}

fn list_hats<W: Write>(
    writer: &mut W,
    config_registry: &HatRegistry,
    _use_colors: bool,
) -> Result<()> {
    if config_registry.is_empty() {
        writeln!(
            writer,
            "No custom hats configured (using default HatlessRalph coordination)."
        )?;
        return Ok(());
    }

    writeln!(writer, "{:<20} DESCRIPTION", "HAT")?;
    writeln!(writer, "{}", "-".repeat(80))?;

    // Sort by name for consistent output
    let mut hats: Vec<_> = config_registry.all().collect();
    hats.sort_by(|a, b| a.name.cmp(&b.name));

    for hat in hats {
        let desc = if hat.description.is_empty() {
            "-"
        } else {
            &hat.description
        };

        // Truncate desc if too long
        let desc = truncate_with_ellipsis(desc, 55);

        writeln!(writer, "{:<20} {}", hat.name, desc)?;
    }
    Ok(())
}

fn validate_hats<W: Write>(
    writer: &mut W,
    config: &RalphConfig,
    runtime_registry: &HatRegistry,
    config_registry: &HatRegistry,
    use_colors: bool,
    strict: bool,
) -> Result<()> {
    writeln!(writer, "Hat Topology Validation")?;
    writeln!(writer, "=======================")?;
    writeln!(writer)?;

    if config_registry.is_empty() {
        writeln!(writer, "No hats configured (solo mode).")?;
        return Ok(());
    }

    // U4: Use the shared report structure. We call the individual
    // validators directly (not the full aggregator) because `hats validate`
    // historically did NOT run config validation, and adding it would be a
    // behavioral regression. The aggregator's config step short-circuits on
    // error, which would prevent topology/payload/orphan checks from running
    // on configs that `hats validate` previously accepted.
    let strictness = RuntimeContractStrictness {
        payload_strict: strict,
        fail_on_warnings: false, // hats validate never fails on warnings alone
    };
    let mut report =
        ralph_core::runtime_contract::RuntimeContractReport::new("hats-validate", strictness);

    // Step 1: topology validation (via shared helper)
    let topology_result =
        ralph_core::preset_validator::validate_preset_topology(config, runtime_registry);
    for err in &topology_result.errors {
        let finding = ralph_core::runtime_contract::RuntimeContractFinding::new(
            "topology.error",
            FindingSource::Topology,
            ralph_core::runtime_contract::FindingSeverity::Error,
            ralph_core::runtime_contract::FindingStage::Authoring,
            err.message.clone(),
        );
        report.add_finding(finding);
    }

    // Step 2: payload contract validation (via shared helper)
    let payload_result =
        ralph_core::payload_contract::validate_payload_contract(config, runtime_registry, strict);
    for finding in ralph_core::runtime_contract::payload_findings_from_result(&payload_result) {
        report.add_finding(finding);
    }

    // Step 3: orphan topic detection (via shared helper)
    for finding in ralph_core::runtime_contract::detect_orphan_topics(config, runtime_registry) {
        report.add_finding(finding);
    }

    // Step 4: preset static lint (U4). Uses the same entry point as
    // `preset check` and the `ralph run` hard gate. In strict mode,
    // ownership warnings are promoted to errors. Only runs in strict
    // mode to preserve backward compatibility — default mode historically
    // did NOT run lint, and adding it would be a behavioral regression
    // for existing `hats validate` users.
    if strict {
        let findings = ralph_core::preset_lint::run_preset_lint(
            config,
            ralph_core::preset_lint::LintStrictness::Strict,
            false,
            None,
        );
        for finding in findings {
            report.add_finding(finding);
        }
    }

    // Render topology findings
    for finding in report
        .findings
        .iter()
        .filter(|f| f.source == FindingSource::Topology)
    {
        print_check(writer, CheckResult::Error, &finding.message, use_colors)?;
    }

    // Render payload findings (legacy format: "Payload contract: <msg> (hat=... ...)")
    for finding in report
        .findings
        .iter()
        .filter(|f| f.source == FindingSource::Payload)
    {
        let check_result = match finding.severity {
            ralph_core::runtime_contract::FindingSeverity::Error => CheckResult::Error,
            ralph_core::runtime_contract::FindingSeverity::Warn => CheckResult::Warn,
            ralph_core::runtime_contract::FindingSeverity::Pass => CheckResult::Ok,
        };
        let msg = if finding.details.is_empty() {
            format!("Payload contract: {}", finding.message)
        } else {
            format!(
                "Payload contract: {} (hat={} topic={} field={} source_hats=[{}] schema={} line={:?})",
                finding.message,
                finding.details.get("hat").map(|s| s.as_str()).unwrap_or(""),
                finding
                    .details
                    .get("topic")
                    .map(|s| s.as_str())
                    .unwrap_or(""),
                finding
                    .details
                    .get("field")
                    .map(|s| s.as_str())
                    .unwrap_or(""),
                finding
                    .details
                    .get("source_hats")
                    .map(|s| s.as_str())
                    .unwrap_or(""),
                finding
                    .details
                    .get("schema_defined_in")
                    .map(|s| s.as_str())
                    .unwrap_or(""),
                finding.details.get("instructions_line").map(|s| s.as_str()),
            )
        };
        print_check(writer, check_result, &msg, use_colors)?;
    }

    writeln!(writer, "Hats: {} configured", config_registry.len())?;
    if let Some(start) = &config.event_loop.starting_event {
        writeln!(writer, "Entry: task.start -> {}", start)?;
    } else {
        writeln!(writer, "Entry: task.start (Ralph coordinates)")?;
    }
    writeln!(writer)?;

    writeln!(writer, "Checks:")?;

    // 1. Starting event validation
    if let Some(start) = &config.event_loop.starting_event {
        if config_registry.has_subscriber(start) {
            let hat = config_registry.get_for_topic(start).unwrap();
            print_check(
                writer,
                CheckResult::Ok,
                &format!("Starting event '{}' has subscriber ({})", start, hat.name),
                use_colors,
            )?;
        } else {
            print_check(
                writer,
                CheckResult::Error,
                &format!("starting_event '{}' has no subscribers", start),
                use_colors,
            )?;
        }
    }

    // 2. Orphan findings from the shared helper
    for finding in report
        .findings
        .iter()
        .filter(|f| f.source == FindingSource::Orphan)
    {
        print_check(writer, CheckResult::Warn, &finding.message, use_colors)?;
    }

    // 3. Lint findings (U4: shared with preset check and run gate)
    for finding in report
        .findings
        .iter()
        .filter(|f| f.source == FindingSource::Lint)
    {
        let check_result = match finding.severity {
            ralph_core::runtime_contract::FindingSeverity::Error => CheckResult::Error,
            ralph_core::runtime_contract::FindingSeverity::Warn => CheckResult::Warn,
            ralph_core::runtime_contract::FindingSeverity::Pass => CheckResult::Ok,
        };
        print_check(writer, check_result, &finding.message, use_colors)?;
    }

    // 4. Dead end detection (informational, not in shared helpers)
    let mut dead_ends = 0;
    for hat in config_registry.all() {
        if hat.publishes.is_empty() {
            dead_ends += 1;
        }
    }
    if dead_ends == 0 {
        print_check(writer, CheckResult::Ok, "No dead-end hats", use_colors)?;
    }

    // Roll report counts into the totals
    let errors = report.errors;
    let warnings = report.warnings;

    writeln!(writer)?;
    if errors > 0 {
        writeln!(
            writer,
            "Result: Invalid ({} errors, {} warnings)",
            errors, warnings
        )?;
        return Err(anyhow::anyhow!("Validation failed with {} errors", errors));
    } else if warnings > 0 {
        writeln!(writer, "Result: Valid ({} warnings)", warnings)?;
    } else {
        writeln!(writer, "Result: Valid")?;
    }
    Ok(())
}

enum CheckResult {
    Ok,
    Warn,
    Error,
}

fn print_check<W: Write>(
    writer: &mut W,
    result: CheckResult,
    msg: &str,
    use_colors: bool,
) -> Result<()> {
    if use_colors {
        match result {
            CheckResult::Ok => {
                writeln!(writer, "  [{}ok{}] {}", colors::GREEN, colors::RESET, msg)?
            }
            CheckResult::Warn => writeln!(
                writer,
                "  [{}warn{}] {}",
                colors::YELLOW,
                colors::RESET,
                msg
            )?,
            CheckResult::Error => {
                writeln!(writer, "  [{}err{}] {}", colors::RED, colors::RESET, msg)?
            }
        }
    } else {
        match result {
            CheckResult::Ok => writeln!(writer, "  [ok] {}", msg)?,
            CheckResult::Warn => writeln!(writer, "  [warn] {}", msg)?,
            CheckResult::Error => writeln!(writer, "  [err] {}", msg)?,
        }
    }
    Ok(())
}

fn graph_hats<W: Write>(
    writer: &mut W,
    config: &RalphConfig,
    config_registry: &HatRegistry,
    format: GraphFormat,
    backend_override: Option<&str>,
) -> Result<()> {
    match format {
        GraphFormat::Mermaid => {
            writeln!(writer, "```mermaid")?;
            write!(writer, "{}", generate_mermaid_string(config_registry))?;
            writeln!(writer, "```")?;
        }
        GraphFormat::Compact => {
            write!(writer, "{}", generate_compact_graph(config_registry))?;
        }
        GraphFormat::Unicode | GraphFormat::Ascii => {
            // Generate diagram via AI backend
            let rendered = render_hat_dag_via_ai(config, config_registry, backend_override)?;
            write!(writer, "{}", rendered)?;
        }
    }
    Ok(())
}

/// Render hat topology as ASCII DAG by calling an AI backend.
///
/// Shows the logical flow: task.start -> Ralph -> Hats
/// Uses the configured backend (or auto-detects) to generate the diagram.
fn render_hat_dag_via_ai(
    config: &RalphConfig,
    config_registry: &HatRegistry,
    backend_override: Option<&str>,
) -> Result<String> {
    if config_registry.is_empty() {
        return Ok("No hats configured.\n".to_string());
    }

    // Resolve backend: CLI flag > config > auto-detect
    let backend_name = resolve_backend(backend_override, config)?;

    // Build the prompt describing the graph
    let prompt = build_diagram_prompt(config_registry);

    // Create backend and generate diagram
    let backend = CliBackend::from_name(&backend_name)
        .map_err(|e| anyhow::anyhow!("Failed to create backend '{}': {}", backend_name, e))?;

    // Show spinner while generating
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .expect("valid template"),
    );
    spinner.set_message(format!("Generating diagram via {}...", backend_name));
    spinner.enable_steady_tick(Duration::from_millis(100));

    // Build command for non-interactive mode
    let (command, args, stdin_input, _temp_file) = backend.build_command(&prompt, false);

    // Spawn and capture output
    let mut child = Command::new(&command)
        .args(&args)
        .stdin(if stdin_input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn backend command: {}", command))?;

    // Send stdin if needed
    if let Some(input) = stdin_input
        && let Some(mut stdin) = child.stdin.take()
    {
        use std::io::Write;
        stdin.write_all(input.as_bytes())?;
    }

    // Wait for completion
    let output = child
        .wait_with_output()
        .context("Failed to wait for backend")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "Backend '{}' failed (exit code: {:?}):\n{}",
            backend_name,
            output.status.code(),
            stderr
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Err(anyhow::anyhow!(
            "Backend '{}' returned empty output",
            backend_name
        ));
    }

    // Extract just the ASCII diagram from the response
    Ok(extract_diagram(&stdout))
}

/// Resolves which backend to use for diagram generation.
///
/// Precedence (highest to lowest):
/// 1. CLI flag (`--backend`)
/// 2. Config file (`cli.backend` in ralph.yml)
/// 3. Auto-detect (first available from claude → gemini → codex)
fn resolve_backend(flag_override: Option<&str>, config: &RalphConfig) -> Result<String> {
    // 1. CLI flag takes precedence
    if let Some(backend) = flag_override {
        validate_backend_name(backend)?;
        return Ok(backend.to_string());
    }

    // 2. Check config (if not "auto")
    if config.cli.backend != "auto" {
        return Ok(config.cli.backend.clone());
    }

    // 3. Auto-detect
    detect_backend_default().map_err(|e| anyhow::anyhow!("{}", e))
}

/// Validates a backend name.
fn validate_backend_name(name: &str) -> Result<()> {
    if !backend_support::is_known_backend(name) {
        return Err(anyhow::anyhow!(
            "{}",
            backend_support::unknown_backend_message(name)
        ));
    }

    Ok(())
}

/// Builds the prompt for diagram generation.
fn build_diagram_prompt(config_registry: &HatRegistry) -> String {
    let mut prompt = String::from(
        "Generate an ASCII diagram showing this directed acyclic graph.\n\
         Use simple box-drawing characters that work in any terminal.\n\
         Show clear arrows between nodes.\n\n\
         Nodes and edges:\n",
    );

    prompt.push_str("- task.start → Ralph\n");

    // Collect all hats sorted for deterministic output
    let mut hats: Vec<_> = config_registry.all().collect();
    hats.sort_by(|a, b| a.name.cmp(&b.name));

    // Ralph -> Hats (based on subscriptions)
    for hat in &hats {
        for sub in &hat.subscriptions {
            prompt.push_str(&format!(
                "- Ralph → {} (triggers on: {})\n",
                hat.name,
                sub.as_str()
            ));
        }
    }

    // Hats -> Ralph (based on publishes)
    for hat in &hats {
        for pub_event in &hat.publishes {
            prompt.push_str(&format!(
                "- {} → Ralph (publishes: {})\n",
                hat.name,
                pub_event.as_str()
            ));
        }
    }

    // Hat -> Hat (direct flows)
    for source in &hats {
        for pub_event in &source.publishes {
            for target in &hats {
                if target.id == source.id {
                    continue;
                }
                if target
                    .subscriptions
                    .iter()
                    .any(|s| s.as_str() == pub_event.as_str())
                {
                    prompt.push_str(&format!(
                        "- {} → {} (via event: {})\n",
                        source.name,
                        target.name,
                        pub_event.as_str()
                    ));
                }
            }
        }
    }

    prompt.push_str("\nOutput ONLY the ASCII diagram, no explanation or markdown fences.");
    prompt
}

/// Extracts the ASCII diagram from the AI response.
/// Removes any markdown fences or explanatory text.
fn extract_diagram(response: &str) -> String {
    let mut lines: Vec<&str> = response.lines().collect();

    // Remove leading/trailing markdown fences
    if lines.first().is_some_and(|l| l.starts_with("```")) {
        lines.remove(0);
    }
    if lines.last().is_some_and(|l| l.starts_with("```")) {
        lines.pop();
    }

    // Remove any leading blank lines or "Here is" type intros
    while lines
        .first()
        .is_some_and(|l| l.trim().is_empty() || l.to_lowercase().starts_with("here"))
    {
        lines.remove(0);
    }

    let result = lines.join("\n");
    if result.ends_with('\n') {
        result
    } else {
        format!("{}\n", result)
    }
}

fn generate_compact_graph(config_registry: &HatRegistry) -> String {
    if config_registry.is_empty() {
        return "No hats configured.\n".to_string();
    }

    let mut output = String::new();
    output.push_str("Graph:\n");
    output.push_str("  task.start -> Ralph\n");

    // Sort hats for deterministic output
    let mut hats: Vec<_> = config_registry.all().collect();
    hats.sort_by(|a, b| a.name.cmp(&b.name));

    for hat in &hats {
        output.push_str(&format!("  Ralph -> {}\n", hat.name));

        for publish in &hat.publishes {
            output.push_str(&format!("    {} => {}\n", hat.name, publish.as_str()));
        }

        for subscription in &hat.subscriptions {
            output.push_str(&format!("    {} <= {}\n", hat.name, subscription.as_str()));
        }
    }

    if !output.ends_with('\n') {
        output.push('\n');
    }

    output
}

/// Generate Mermaid flowchart syntax for the hat topology.
fn generate_mermaid_string(config_registry: &HatRegistry) -> String {
    let mut output = String::new();
    output.push_str("flowchart LR\n");
    output.push_str("    Start[task.start] --> Ralph\n");

    // Reconstruct Ralph's publishes (what hats subscribe to)
    let mut ralph_publishes: HashSet<String> = HashSet::new();
    for hat in config_registry.all() {
        for sub in &hat.subscriptions {
            ralph_publishes.insert(sub.as_str().to_string());
        }
    }

    // Ralph -> Hats
    for hat in config_registry.all() {
        let node_id = sanitize_id(&hat.name);
        for sub in &hat.subscriptions {
            output.push_str(&format!("    Ralph -->|{}| {}\n", sub.as_str(), node_id));
        }
    }

    // Hats -> Ralph
    for hat in config_registry.all() {
        let node_id = sanitize_id(&hat.name);
        for pub_event in &hat.publishes {
            output.push_str(&format!(
                "    {} -->|{}| Ralph\n",
                node_id,
                pub_event.as_str()
            ));
        }
    }

    // Hat -> Hat (direct flow visualization)
    // Even though everything goes through Ralph, it's useful to see A -> B
    for source in config_registry.all() {
        let source_id = sanitize_id(&source.name);
        for pub_event in &source.publishes {
            // Find hats that subscribe to this
            for target in config_registry.all() {
                if target.id == source.id {
                    continue;
                }
                if target
                    .subscriptions
                    .iter()
                    .any(|s| s.as_str() == pub_event.as_str())
                {
                    let target_id = sanitize_id(&target.name);
                    output.push_str(&format!(
                        "    {} -.->|{}| {}\n",
                        source_id,
                        pub_event.as_str(),
                        target_id
                    ));
                }
            }
        }
    }

    output
}

fn sanitize_id(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn show_hat<W: Write>(
    writer: &mut W,
    config_registry: &HatRegistry,
    name: &str,
    use_colors: bool,
) -> Result<()> {
    // Try to find by ID first, then by display name
    let hat = config_registry
        .all()
        .find(|h| h.id.as_str() == name || h.name == name);

    let hat = hat.context(format!("Hat '{}' not found", name))?;

    if use_colors {
        writeln!(writer, "{}{}{}", colors::BOLD, hat.name, colors::RESET)?;
    } else {
        writeln!(writer, "{}", hat.name)?;
    }

    if !hat.description.is_empty() {
        writeln!(writer, "{}", hat.description)?;
    }
    writeln!(writer)?;

    writeln!(writer, "ID: {}", hat.id)?;

    writeln!(writer, "\nTriggers On:")?;
    if hat.subscriptions.is_empty() {
        writeln!(writer, "  (none)")?;
    } else {
        for trigger in &hat.subscriptions {
            writeln!(writer, "  - {}", trigger.as_str())?;
        }
    }

    writeln!(writer, "\nPublishes:")?;
    if hat.publishes.is_empty() {
        writeln!(writer, "  (none)")?;
    } else {
        for topic in &hat.publishes {
            writeln!(writer, "  - {}", topic.as_str())?;
        }
    }

    // Multi-consumer opt-in topics are hat-config metadata, not part of
    // the public event contract, but they determine routing behavior in
    // isolated mode. Surface them so operators/agents can discover why a
    // topic is consumed by more than one hat.
    if let Some(config) = config_registry.get_config(&hat.id) {
        if !config.trigger_multi_consumer_topics.is_empty() {
            writeln!(writer, "\nMulti-consumer topics (opt-in):")?;
            for topic in &config.trigger_multi_consumer_topics {
                writeln!(writer, "  - {}", topic)?;
            }
        }
    }

    if !hat.instructions.is_empty() {
        writeln!(writer, "\nInstructions:")?;
        for line in hat.instructions.lines() {
            writeln!(writer, "  {}", line)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_proto::Hat;

    fn mock_hat(name: &str, subs: &[&str], pubs: &[&str]) -> Hat {
        let mut hat = Hat::new(sanitize_id(name), name);
        hat.description = format!("Description for {}", name);
        hat.subscriptions = subs.iter().map(|s| (*s).into()).collect();
        hat.publishes = pubs.iter().map(|s| (*s).into()).collect();
        hat
    }

    #[test]
    fn test_sanitize_id() {
        assert_eq!(sanitize_id("My Hat"), "MyHat");
        assert_eq!(sanitize_id("cool-hat"), "coolhat");
        assert_eq!(sanitize_id("Hat!@#"), "Hat");
        assert_eq!(sanitize_id("123"), "123");
    }

    #[test]
    fn test_list_hats_empty() {
        let config_registry = HatRegistry::new();
        let mut buf = Vec::new();
        list_hats(&mut buf, &config_registry, false).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("No custom hats configured"));
    }

    #[test]
    fn test_list_hats_with_entries() {
        let mut config_registry = HatRegistry::new();
        config_registry.register(mock_hat("Builder", &["build.task"], &["build.done"]));
        config_registry.register(mock_hat("Planner", &["plan.start"], &["build.task"]));

        let mut buf = Vec::new();
        list_hats(&mut buf, &config_registry, false).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("HAT                  DESCRIPTION"));
        assert!(output.contains("Builder"));
        assert!(output.contains("Planner"));
    }

    #[test]
    fn test_validate_hats_orphan() {
        let mut config_registry = HatRegistry::new();
        // Builder publishes build.done, but no one listens
        config_registry.register(mock_hat("Builder", &["build.task"], &["build.done"]));

        let config = RalphConfig::default();
        let mut buf = Vec::new();

        // Validation might exit process on error, so we test warning scenario
        // Test registries have no builtin ralph, so pass the same as both params
        validate_hats(
            &mut buf,
            &config,
            &config_registry,
            &config_registry,
            false,
            false,
        )
        .unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Should warn about build.done having no subscribers
        assert!(
            output.contains("Event 'build.done' published by 'Builder' has no hat subscribers")
        );
        assert!(output.contains("Result: Valid (1 warnings)"));
    }

    /// `required_events` topics are loop-level gates (consumed by the
    /// loop runner via `missing_required_events`), not hat-to-hat
    /// signals. They must NOT trigger an orphan warning.
    ///
    /// Regression case: ce-executor's `report.done` is in
    /// `required_events` and the loop runner checks it before accepting
    /// `LOOP_COMPLETE`. Pre-fix this was a false positive.
    #[test]
    fn test_validate_hats_orphan_required_event_is_exempt() {
        let mut config_registry = HatRegistry::new();
        config_registry.register(mock_hat(
            "Reporter",
            &["REVIEW_COMPLETE"],
            &["report.done", "LOOP_COMPLETE"],
        ));

        let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let mut buf = Vec::new();

        validate_hats(
            &mut buf,
            &config,
            &config_registry,
            &config_registry,
            false,
            false,
        )
        .unwrap();
        let output = String::from_utf8(buf).unwrap();

        // `report.done` must NOT be flagged — it's a required_events gate.
        assert!(
            !output.contains("'report.done' published by 'Reporter' has no hat subscribers"),
            "`report.done` is a required_events gate, must not be flagged as orphan. Output: {}",
            output
        );
        // `LOOP_COMPLETE` must not be flagged either (it's the completion_promise).
        assert!(
            !output.contains("'LOOP_COMPLETE' published by 'Reporter' has no hat subscribers"),
            "completion_promise must not be flagged. Output: {}",
            output
        );
    }

    /// Topics the loop runner consumes directly (currently just
    /// `build.blocked` for thrashing detection) must NOT trigger an
    /// orphan warning. See `LOOP_RUNNER_INTERNAL_TOPICS` for the
    /// rationale and the audit checklist before adding new entries.
    ///
    /// Regression case: a previous builtin preset had a Builder hat
    /// publishing `build.blocked` with no hat subscriber. The loop
    /// runner's thrashing detector consumes it instead.
    #[test]
    fn test_validate_hats_orphan_loop_runner_internal_is_exempt() {
        let mut config_registry = HatRegistry::new();
        config_registry.register(mock_hat("Builder", &["build.task"], &["build.blocked"]));

        let config = RalphConfig::default();
        let mut buf = Vec::new();

        validate_hats(
            &mut buf,
            &config,
            &config_registry,
            &config_registry,
            false,
            false,
        )
        .unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(
            !output.contains("'build.blocked' published by 'Builder' has no hat subscribers"),
            "`build.blocked` is consumed by the loop runner's thrashing detector, \
             must not be flagged as orphan. Output: {}",
            output
        );
    }

    /// CRITICAL REGRESSION GUARD: the exemptions above must not
    /// silently swallow real orphan events. A hat that publishes a
    /// typo (e.g. `work.dnoe` instead of `work.done`) must still
    /// produce a warning. If this test fails, the orphan check has
    /// been over-broadened and `hats validate` has lost its
    /// ability to catch typos and missing subscribers.
    #[test]
    fn test_validate_hats_real_orphan_still_warns() {
        let mut config_registry = HatRegistry::new();
        // Hat publishes a topic that no one subscribes to, is not the
        // completion promise, is not in required_events, and is not a
        // known loop-runner-internal topic. This MUST be flagged.
        config_registry.register(mock_hat("Sloppy", &["trigger.z"], &["orphan.typo"]));

        let config = RalphConfig::default();
        let mut buf = Vec::new();

        validate_hats(
            &mut buf,
            &config,
            &config_registry,
            &config_registry,
            false,
            false,
        )
        .unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(
            output.contains("Event 'orphan.typo' published by 'Sloppy' has no hat subscribers"),
            "Real orphan events must still be warned. The exemptions added to \
             validate_hats must not silently widen past their intended scope. \
             Output: {}",
            output
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // U4: --strict flag and payload contract validation in `ralph hats validate`
    // ──────────────────────────────────────────────────────────────────────

    fn config_with_payload_contracts() -> RalphConfig {
        // Hat b references payload fields, but the trigger topic has no schema.
        // Default mode → warning only.
        // Strict mode → error, validation fails.
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn test_validate_hats_default_runs_payload_contract_as_warning() {
        let config = config_with_payload_contracts();
        let config_registry = HatRegistry::from_config(&config);
        let runtime_registry = HatRegistry::from_runtime_config(&config);
        let mut buf = Vec::new();
        // strict=false (default). Payload contract is a warning, not error.
        let result = validate_hats(
            &mut buf,
            &config,
            &runtime_registry,
            &config_registry,
            false,
            false,
        );
        assert!(result.is_ok(), "Default mode should not fail: {:?}", result);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("payload contract") || output.contains("Payload"),
            "Output should mention payload contract validation: {}",
            output
        );
    }

    #[test]
    fn test_validate_hats_strict_fails_on_missing_schema() {
        let config = config_with_payload_contracts();
        let config_registry = HatRegistry::from_config(&config);
        let runtime_registry = HatRegistry::from_runtime_config(&config);
        let mut buf = Vec::new();
        // strict=true → missing schema is an error → validation fails.
        let result = validate_hats(
            &mut buf,
            &config,
            &runtime_registry,
            &config_registry,
            false,
            true,
        );
        assert!(
            result.is_err(),
            "Strict mode should fail when payload contract is violated"
        );
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("SchemaMissingForRequiredTopic") || output.contains("schema"),
            "Output should mention schema issue: {}",
            output
        );
    }

    #[test]
    fn test_validate_hats_payload_field_missing_in_schema() {
        // Schema exists but required_fields does not include `plan_name`.
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["task_id"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let config_registry = HatRegistry::from_config(&config);
        let runtime_registry = HatRegistry::from_runtime_config(&config);
        let mut buf = Vec::new();
        // FieldMissingFromSchema is always an error (default and strict).
        let result = validate_hats(
            &mut buf,
            &config,
            &runtime_registry,
            &config_registry,
            false,
            false,
        );
        assert!(result.is_err(), "Field missing from schema must error");
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("plan_name"),
            "Output must mention field: {}",
            output
        );
        assert!(
            output.contains("work.ready"),
            "Output must mention topic: {}",
            output
        );
    }

    #[test]
    fn test_validate_hats_output_includes_preset_path_and_line() {
        // The error output must include hat id, topic, field, schema source.
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["task_id"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let config_registry = HatRegistry::from_config(&config);
        let runtime_registry = HatRegistry::from_runtime_config(&config);
        let mut buf = Vec::new();
        validate_hats(
            &mut buf,
            &config,
            &runtime_registry,
            &config_registry,
            false,
            false,
        )
        .unwrap_err();
        let output = String::from_utf8(buf).unwrap();
        // Must include hat id, topic, field
        assert!(output.contains("b"), "must include hat id: {}", output);
        assert!(
            output.contains("work.ready"),
            "must include topic: {}",
            output
        );
        assert!(
            output.contains("plan_name"),
            "must include field: {}",
            output
        );
    }

    #[test]
    fn test_graph_hats_compact() {
        let mut config_registry = HatRegistry::new();
        config_registry.register(mock_hat("Builder", &["build.task"], &["build.done"]));
        config_registry.register(mock_hat("Planner", &["planner.start"], &["planner.done"]));

        let config = RalphConfig::default();
        let mut buf = Vec::new();

        graph_hats(
            &mut buf,
            &config,
            &config_registry,
            GraphFormat::Compact,
            None,
        )
        .unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("Graph:"));
        assert!(output.contains("task.start -> Ralph"));
        assert!(output.contains("Ralph -> Builder"));
        assert!(
            output.contains("Builder => build.task") || output.contains("Builder <= build.task")
        );
    }

    #[test]
    #[ignore = "requires live AI backend"]
    fn test_graph_hats_ascii() {
        let mut config_registry = HatRegistry::new();
        config_registry.register(mock_hat("Builder", &["build.task"], &["build.done"]));

        let config = RalphConfig::default();
        let mut buf = Vec::new();

        graph_hats(
            &mut buf,
            &config,
            &config_registry,
            GraphFormat::Ascii,
            None,
        )
        .unwrap();
        let output = String::from_utf8(buf).unwrap();

        // AI-generated output should contain the node names
        assert!(output.contains("Builder") || output.contains("Ralph"));
    }

    #[test]
    #[ignore = "requires live AI backend"]
    fn test_graph_hats_unicode() {
        let mut config_registry = HatRegistry::new();
        config_registry.register(mock_hat("Coder", &["code.task"], &["code.done"]));

        let config = RalphConfig::default();
        let mut buf = Vec::new();

        graph_hats(
            &mut buf,
            &config,
            &config_registry,
            GraphFormat::Unicode,
            None,
        )
        .unwrap();
        let output = String::from_utf8(buf).unwrap();

        // AI-generated output should contain node names
        assert!(output.contains("Coder") || output.contains("Ralph"));
    }

    #[test]
    fn test_generate_mermaid_string() {
        let mut config_registry = HatRegistry::new();
        config_registry.register(mock_hat("A", &["start"], &["mid"]));
        config_registry.register(mock_hat("B", &["mid"], &["end"]));

        let output = generate_mermaid_string(&config_registry);

        assert!(output.contains("flowchart LR"));
        assert!(output.contains("Ralph -->|start| A"));
        assert!(output.contains("A -->|mid| Ralph"));
        assert!(output.contains("Ralph -->|mid| B"));
        // Hat-to-hat connection (A publishes mid, B subscribes to mid)
        assert!(output.contains("A -.->|mid| B"));
    }

    #[test]
    fn test_show_hat_found() {
        let mut config_registry = HatRegistry::new();
        config_registry.register(mock_hat("Builder", &["build.task"], &["build.done"]));

        let mut buf = Vec::new();
        show_hat(&mut buf, &config_registry, "Builder", false).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("Builder"));
        assert!(output.contains("Triggers On:"));
        assert!(output.contains("build.task"));
        assert!(output.contains("Publishes:"));
        assert!(output.contains("build.done"));
    }

    #[test]
    fn test_show_hat_includes_multi_consumer_topics() {
        use ralph_core::config::HatConfig;
        use std::collections::HashSet;

        let mut config_registry = HatRegistry::new();
        let hat = mock_hat("Router", &["task.*"], &["task.done"]);
        let mut cfg = HatConfig::default();
        cfg.trigger_multi_consumer_topics =
            HashSet::from(["fix.exhausted".to_string(), "debug.exhausted".to_string()]);
        config_registry.register_with_config(hat, cfg);

        let mut buf = Vec::new();
        show_hat(&mut buf, &config_registry, "Router", false).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("Multi-consumer topics (opt-in):"));
        assert!(output.contains("fix.exhausted"));
        assert!(output.contains("debug.exhausted"));
    }

    #[test]
    fn test_show_hat_not_found() {
        let config_registry = HatRegistry::new();
        let mut buf = Vec::new();
        let result = show_hat(&mut buf, &config_registry, "Nonexistent", false);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_validate_hats_empty_config_registry() {
        let config_registry = HatRegistry::new();
        let config = RalphConfig::default();
        let mut buf = Vec::new();

        // Test registries have no builtin ralph, so pass the same as both params
        validate_hats(
            &mut buf,
            &config,
            &config_registry,
            &config_registry,
            false,
            false,
        )
        .unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("No hats configured"));
    }

    #[test]
    fn test_validate_hats_valid_topology() {
        let mut config_registry = HatRegistry::new();
        // Create a closed loop: A subscribes to start, publishes mid; B subscribes to mid
        config_registry.register(mock_hat("A", &["start"], &["mid"]));
        config_registry.register(mock_hat("B", &["mid"], &[]));

        let config = RalphConfig::default();
        let mut buf = Vec::new();

        // Test registries have no builtin ralph, so pass the same as both params
        validate_hats(
            &mut buf,
            &config,
            &config_registry,
            &config_registry,
            false,
            false,
        )
        .unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("No dead-end hats") || output.contains("Result: Valid"));
    }

    #[test]
    fn test_list_hats_json() {
        let mut config_registry = HatRegistry::new();
        config_registry.register(mock_hat("Builder", &["build.task"], &["build.done"]));

        let mut buf = Vec::new();
        list_hats_json(&mut buf, &config_registry).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_print_check_ok() {
        let mut buf = Vec::new();
        print_check(&mut buf, CheckResult::Ok, "Test passed", false).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("[ok]"));
        assert!(output.contains("Test passed"));
    }

    #[test]
    fn test_print_check_warn() {
        let mut buf = Vec::new();
        print_check(&mut buf, CheckResult::Warn, "Warning message", false).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("[warn]"));
        assert!(output.contains("Warning message"));
    }

    #[test]
    fn test_print_check_error() {
        let mut buf = Vec::new();
        print_check(&mut buf, CheckResult::Error, "Error message", false).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("[err]"));
        assert!(output.contains("Error message"));
    }

    #[test]
    fn test_print_check_colored() {
        let mut buf = Vec::new();
        print_check(&mut buf, CheckResult::Ok, "Color test", true).unwrap();
        let output = String::from_utf8(buf).unwrap();
        // Should contain ANSI color codes
        assert!(output.contains("\x1b["));
    }

    #[test]
    fn test_list_hats_truncates_long_description() {
        let mut config_registry = HatRegistry::new();
        let mut hat = mock_hat("LongDesc", &["start"], &["end"]);
        hat.description = "A".repeat(100); // Very long description
        config_registry.register(hat);

        let mut buf = Vec::new();
        list_hats(&mut buf, &config_registry, false).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Description should be truncated with "..."
        assert!(output.contains("..."));
    }

    #[test]
    fn test_build_diagram_prompt() {
        let mut config_registry = HatRegistry::new();
        config_registry.register(mock_hat("Builder", &["build.task"], &["build.done"]));
        config_registry.register(mock_hat("Tester", &["test.task"], &["test.done"]));

        let prompt = build_diagram_prompt(&config_registry);

        // Should contain the key elements
        assert!(prompt.contains("task.start → Ralph"));
        assert!(prompt.contains("Ralph → Builder"));
        assert!(prompt.contains("build.task"));
        assert!(prompt.contains("build.done"));
        assert!(prompt.contains("Ralph → Tester"));
        assert!(prompt.contains("Output ONLY the ASCII diagram"));
    }

    #[test]
    fn test_extract_diagram_plain() {
        let response = "┌─────┐\n│Ralph│\n└─────┘";
        let diagram = extract_diagram(response);
        assert!(diagram.contains("Ralph"));
        assert!(diagram.ends_with('\n'));
    }

    #[test]
    fn test_extract_diagram_with_markdown_fences() {
        let response = "```\n┌─────┐\n│Ralph│\n└─────┘\n```";
        let diagram = extract_diagram(response);
        assert!(diagram.contains("Ralph"));
        assert!(!diagram.contains("```"));
    }

    #[test]
    fn test_extract_diagram_with_intro() {
        let response = "Here is the diagram:\n\n┌─────┐\n│Ralph│\n└─────┘";
        let diagram = extract_diagram(response);
        assert!(diagram.contains("Ralph"));
        assert!(!diagram.to_lowercase().contains("here"));
    }

    #[test]
    fn test_validate_backend_name_valid() {
        assert!(validate_backend_name("claude").is_ok());
        assert!(validate_backend_name("gemini").is_ok());
        assert!(validate_backend_name("codex").is_ok());
        assert!(validate_backend_name("custom").is_ok());
    }

    #[test]
    fn test_validate_backend_name_invalid() {
        let result = validate_backend_name("unknown-backend");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown backend"));
        assert!(err.contains("Valid backends"));
    }

    #[test]
    fn test_resolve_backend_flag_override() {
        let config = RalphConfig::default();
        let result = resolve_backend(Some("codex"), &config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "codex");
    }

    #[test]
    fn test_resolve_backend_from_config() {
        let mut config = RalphConfig::default();
        config.cli.backend = "gemini".to_string();

        let result = resolve_backend(None, &config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "gemini");
    }
}
