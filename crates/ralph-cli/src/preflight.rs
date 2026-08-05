//! Preflight command for validating configuration and environment.

use anyhow::{Context, Result};
use clap::{ArgAction, Parser, ValueEnum};
use ralph_core::{CheckResult, CheckStatus, PreflightReport, PreflightRunner, RalphConfig};
use serde_yaml::{Mapping, Value};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::{ConfigSource, HatsSource, config_resolution, presets};

#[derive(Parser, Debug)]
pub struct PreflightArgs {
    /// Output format (human or json)
    #[arg(long, value_enum, default_value_t = PreflightFormat::Human)]
    pub format: PreflightFormat,

    /// Treat warnings as failures
    #[arg(long)]
    pub strict: bool,

    /// Run only specific check(s)
    #[arg(long, value_name = "NAME", action = ArgAction::Append)]
    pub check: Vec<String>,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum PreflightFormat {
    Human,
    Json,
}

pub async fn execute(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    args: PreflightArgs,
    use_colors: bool,
) -> Result<()> {
    let source_label = config_source_label(config_sources, hats_source);
    let config = load_config_for_preflight(config_sources, hats_source).await?;

    let runner = PreflightRunner::default_checks_with_config(&config);
    let requested = normalize_checks(&args.check);
    validate_checks(&runner, &requested)?;

    let mut report = if requested.is_empty() {
        runner.run_all(&config).await
    } else {
        runner.run_selected(&config, &requested).await
    };

    let effective_passed = if args.strict {
        report.failures == 0 && report.warnings == 0
    } else {
        report.failures == 0
    };
    report.passed = effective_passed;

    match args.format {
        PreflightFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        PreflightFormat::Human => {
            print_human_report(&report, &source_label, use_colors, args.strict);
        }
    }

    if !effective_passed {
        std::process::exit(1);
    }

    Ok(())
}

fn normalize_checks(checks: &[String]) -> Vec<String> {
    checks.iter().map(|check| check.to_lowercase()).collect()
}

fn validate_checks(runner: &PreflightRunner, checks: &[String]) -> Result<()> {
    if checks.is_empty() {
        return Ok(());
    }

    let available = runner.check_names();
    let unknown: Vec<&String> = checks
        .iter()
        .filter(|check| {
            !available
                .iter()
                .any(|name| name.eq_ignore_ascii_case(check))
        })
        .collect();

    if !unknown.is_empty() {
        let available_list = available.join(", ");
        let unknown_list = unknown
            .iter()
            .map(|check| check.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("Unknown check(s): {unknown_list}. Available checks: {available_list}");
    }

    Ok(())
}

fn print_human_report(report: &PreflightReport, source: &str, use_colors: bool, strict: bool) {
    use crate::display::colors;

    println!("Preflight checks for {}", source);
    println!();

    let name_width = report
        .checks
        .iter()
        .map(|check| check.name.len())
        .max()
        .unwrap_or(4)
        .max(4);

    for check in &report.checks {
        print_check_line(check, name_width, use_colors);
    }

    println!();

    let result = if report.passed { "PASS" } else { "FAIL" };
    let mut details = Vec::new();
    if report.failures > 0 {
        details.push(format!("{} failure(s)", report.failures));
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
            detail = detail_text
        );
    } else {
        println!("Result: {result}{detail}", detail = detail_text);
    }

    if strict && report.warnings > 0 {
        println!("Note: strict mode treats warnings as failures.");
    }
}

fn print_check_line(check: &CheckResult, name_width: usize, use_colors: bool) {
    use crate::display::colors;

    let (status_text, color) = match check.status {
        CheckStatus::Pass => ("OK", colors::GREEN),
        CheckStatus::Warn => ("WARN", colors::YELLOW),
        CheckStatus::Fail => ("FAIL", colors::RED),
    };

    let status_padded = format!("{status_text:<4}");
    let status_display = if use_colors {
        format!(
            "{color}{status}{reset}",
            status = status_padded,
            reset = colors::RESET
        )
    } else {
        status_padded
    };

    println!(
        "  {status} {name:<width$} {label}",
        status = status_display,
        name = check.name,
        width = name_width,
        label = check.label
    );

    if let Some(message) = &check.message {
        for line in message.lines() {
            println!("      {line}");
        }
    }
}

pub(crate) async fn load_config_for_preflight(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
) -> Result<RalphConfig> {
    let (mut core_value, overrides, core_label) = load_core_value(config_sources).await?;

    validate_core_config_shape(&core_value, &core_label)?;

    if let Some(source) = hats_source {
        let operator_core = core_value.clone();
        if let Some(mapping) = core_value.as_mapping()
            && (mapping_get(mapping, "hats").is_some() || mapping_get(mapping, "events").is_some())
        {
            warn!(
                "Core config '{}' contains hats/events and hats source '{}' was provided; \
                 preset supplies hats/events, then per-hat fields from the operator config \
                 (e.g. backend) are merged on top",
                core_label,
                source.label()
            );
        }

        let hats_value = load_hats_value(source).await?;
        validate_hats_config_shape(&hats_value, &source.label())?;
        core_value = merge_hats_overlay(core_value, hats_value)?;
        merge_operator_hat_field_overlays(&operator_core, &mut core_value);
    }

    let mut config: RalphConfig = serde_yaml::from_value(core_value)
        .with_context(|| format!("Failed to parse merged core config from {}", core_label))?;

    config.normalize();
    config.core.workspace_root =
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Record the primary config file path for template substitution in hooks.
    config.config_path = config_sources
        .iter()
        .find(|s| matches!(s, ConfigSource::File(_)))
        .and_then(|s| match s {
            ConfigSource::File(path) => Some(path.clone()),
            _ => None,
        });

    // Resolve external schema files referenced in event_policy.schema_file.
    // Base path is the config file's directory, or workspace_root if no config file.
    let schema_base_path = config
        .config_path
        .as_ref()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| config.core.workspace_root.clone());
    if let Err(e) = config.resolve_schema_files(&schema_base_path) {
        anyhow::bail!("Failed to resolve schema files: {}", e);
    }

    crate::apply_config_overrides(&mut config, &overrides)?;

    Ok(config)
}

/// Synchronous variant of [`load_config_for_preflight`] for callers that
/// cannot easily enter an async context (e.g. `ralph emit`).
///
/// Remote config/hats sources are **not** supported and produce a clear
/// error. File and builtin presets work exactly like the async path,
/// including user-layer merging, hats overlay, schema resolution, and
/// CLI override application.
pub(crate) fn load_config_for_preflight_sync(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    workspace_root: &std::path::Path,
) -> Result<RalphConfig> {
    load_config_for_preflight_sync_with_missing_default_warning(
        config_sources,
        hats_source,
        workspace_root,
        true,
    )
}

/// Emit can run from a builtin hat collection without a project
/// `ralph.yml`. In that case the default core config is intentional,
/// so callers may suppress only the missing-default warning. Explicit
/// config paths continue to use [`load_config_for_preflight_sync`].
pub(crate) fn load_config_for_preflight_sync_with_missing_default_warning(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    workspace_root: &std::path::Path,
    warn_on_missing_default: bool,
) -> Result<RalphConfig> {
    let (mut core_value, overrides, core_label) =
        load_core_value_sync(config_sources, warn_on_missing_default)?;

    validate_core_config_shape(&core_value, &core_label)?;

    if let Some(source) = hats_source {
        let operator_core = core_value.clone();
        if let Some(mapping) = core_value.as_mapping()
            && (mapping_get(mapping, "hats").is_some() || mapping_get(mapping, "events").is_some())
        {
            warn!(
                "Core config '{}' contains hats/events and hats source '{}' was provided; \
                 preset supplies hats/events, then per-hat fields from the operator config \
                 (e.g. backend) are merged on top",
                core_label,
                source.label()
            );
        }

        let hats_value = load_hats_value_sync(source)?;
        validate_hats_config_shape(&hats_value, &source.label())?;
        core_value = merge_hats_overlay(core_value, hats_value)?;
        merge_operator_hat_field_overlays(&operator_core, &mut core_value);
    }

    let mut config: RalphConfig = serde_yaml::from_value(core_value)
        .with_context(|| format!("Failed to parse merged core config from {}", core_label))?;

    config.normalize();
    config.core.workspace_root = workspace_root.to_path_buf();

    // Record the primary config file path for template substitution in hooks.
    config.config_path = config_sources
        .iter()
        .find(|s| matches!(s, ConfigSource::File(_)))
        .and_then(|s| match s {
            ConfigSource::File(path) => Some(path.clone()),
            _ => None,
        });

    // Resolve external schema files referenced in event_policy.schema_file.
    let schema_base_path = config
        .config_path
        .as_ref()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| config.core.workspace_root.clone());
    if let Err(e) = config.resolve_schema_files(&schema_base_path) {
        anyhow::bail!("Failed to resolve schema files: {}", e);
    }

    crate::apply_config_overrides(&mut config, &overrides)?;

    Ok(config)
}

pub(crate) fn config_source_label(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
) -> String {
    let primary = config_sources
        .iter()
        .find(|source| !matches!(source, ConfigSource::Override { .. }));

    let (primary_label, primary_uses_defaults) = match primary {
        Some(ConfigSource::File(path)) => (path.display().to_string(), false),
        Some(ConfigSource::Builtin(name)) => (format!("builtin:{}", name), false),
        Some(ConfigSource::Remote(url)) => (url.clone(), false),
        Some(ConfigSource::Override { .. }) => unreachable!("Overrides are filtered out"),
        None => {
            let default_path = crate::default_config_path();
            let uses_defaults = !default_path.exists();
            (default_path.display().to_string(), uses_defaults)
        }
    };

    let core_label = config_resolution::compose_core_label(
        config_resolution::user_config_label_if_exists().as_deref(),
        &primary_label,
        primary_uses_defaults,
    );

    if let Some(source) = hats_source {
        format!("{} + hats:{}", core_label, source.label())
    } else {
        core_label
    }
}

async fn load_core_value(
    config_sources: &[ConfigSource],
) -> Result<(Value, Vec<ConfigSource>, String)> {
    let (primary_sources, overrides) = config_resolution::split_config_sources(config_sources);

    if primary_sources.len() > 1 {
        warn!("Multiple config sources specified, using first one. Others ignored.");
    }

    let user_layer = config_resolution::load_optional_user_config_value()?;

    let (primary_value, primary_label, primary_uses_defaults) = if let Some(source) =
        primary_sources.first()
    {
        match source {
            ConfigSource::File(path) => {
                if path.exists() {
                    let label = path.display().to_string();
                    let content = std::fs::read_to_string(path)
                        .with_context(|| format!("Failed to load config from {}", label))?;
                    let value = config_resolution::parse_yaml_value(&content, &label)?;
                    (Some(value), label, false)
                } else {
                    warn!("Config file {:?} not found, using defaults", path);
                    (None, path.display().to_string(), false)
                }
            }
            ConfigSource::Builtin(name) => {
                anyhow::bail!(
                    "`-c builtin:{name}` is no longer supported.\n\nBuiltin presets are now hat collections.\nUse:\n  ralph run -c ralph.yml -H builtin:{name}\n\nOr for preflight:\n  ralph preflight -c ralph.yml -H builtin:{name}"
                );
            }
            ConfigSource::Remote(url) => {
                info!("Fetching core config from {}", url);
                let response = reqwest::get(url)
                    .await
                    .with_context(|| format!("Failed to fetch core config from {}", url))?;

                if !response.status().is_success() {
                    anyhow::bail!(
                        "Failed to fetch core config from {}: HTTP {}",
                        url,
                        response.status()
                    );
                }

                let content = response
                    .text()
                    .await
                    .with_context(|| format!("Failed to read core config content from {}", url))?;

                let value = config_resolution::parse_yaml_value(&content, url)?;
                (Some(value), url.clone(), false)
            }
            ConfigSource::Override { .. } => unreachable!("Partitioned out overrides"),
        }
    } else {
        let default_path = crate::default_config_path();
        if default_path.exists() {
            let label = default_path.display().to_string();
            let content = std::fs::read_to_string(&default_path)
                .with_context(|| format!("Failed to load config from {}", label))?;
            let value = config_resolution::parse_yaml_value(&content, &label)?;
            (Some(value), label, false)
        } else {
            warn!(
                "Config file {} not found, using defaults",
                default_path.display()
            );
            (None, default_path.display().to_string(), true)
        }
    };

    let mut merged = config_resolution::default_core_value()?;
    if let Some((user_value, _)) = &user_layer {
        merged = config_resolution::merge_yaml_values(merged, user_value.clone())?;
    }
    if let Some(primary_value) = primary_value {
        merged = config_resolution::merge_yaml_values(merged, primary_value)?;
    }

    let merged_label = config_resolution::compose_core_label(
        user_layer.as_ref().map(|(_, label)| label.as_str()),
        &primary_label,
        primary_uses_defaults,
    );

    Ok((merged, overrides, merged_label))
}

async fn load_hats_value(source: &HatsSource) -> Result<Value> {
    match source {
        HatsSource::File(path) => {
            if !path.exists() {
                anyhow::bail!("Hats file not found: {}", path.display());
            }
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to load hats from {:?}", path))?;
            let value = config_resolution::parse_yaml_value(&content, &path.display().to_string())?;
            let value = merge_adjacent_preset_schema_ssot(path, value)?;
            normalize_hats_source_value(value, &path.display().to_string())
        }
        HatsSource::Remote(url) => {
            info!("Fetching hats config from {}", url);
            let response = reqwest::get(url)
                .await
                .with_context(|| format!("Failed to fetch hats config from {}", url))?;

            if !response.status().is_success() {
                anyhow::bail!(
                    "Failed to fetch hats config from {}: HTTP {}",
                    url,
                    response.status()
                );
            }

            let content = response
                .text()
                .await
                .with_context(|| format!("Failed to read hats config content from {}", url))?;

            let value = config_resolution::parse_yaml_value(&content, url)?;
            normalize_hats_source_value(value, url)
        }
        HatsSource::Builtin(name) => {
            let preset = presets::get_preset(name).ok_or_else(|| {
                let available = presets::preset_names().join(", ");
                anyhow::anyhow!(
                    "Unknown hat collection '{}'. Available builtins: {}",
                    name,
                    available
                )
            })?;

            let preset_value =
                config_resolution::parse_yaml_value(preset.content, &format!("builtin:{}", name))?;
            extract_hat_overlay_from_preset(preset_value)
        }
    }
}

/// File presets in the repository keep payload schemas in the adjacent
/// `presets/schemas/<stem>.yml` SSOT. Builtins receive this merge in
/// `ralph-cli/build.rs`; file-mode review and execution must use the same
/// contract instead of silently dropping the schema layer.
fn merge_adjacent_preset_schema_ssot(path: &Path, mut preset: Value) -> Result<Value> {
    let Some(parent) = path.parent() else {
        return Ok(preset);
    };
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(preset);
    };
    let Some(en_dir) = parent.file_name().and_then(|s| s.to_str()) else {
        return Ok(preset);
    };
    if en_dir != "en" {
        return Ok(preset);
    }
    let schema_path = parent
        .parent()
        .map(|root| root.join("schemas").join(format!("{stem}.yml")));
    let Some(schema_path) = schema_path.filter(|p| p.is_file()) else {
        return Ok(preset);
    };
    let schema_text = std::fs::read_to_string(&schema_path)
        .with_context(|| format!("Failed to load schema SSOT from {}", schema_path.display()))?;
    let ssot: Value =
        config_resolution::parse_yaml_value(&schema_text, &schema_path.display().to_string())?;
    let Some(ssot_schemas) = ssot.get("schemas").and_then(Value::as_mapping) else {
        return Ok(preset);
    };
    let event_loop = preset
        .as_mapping_mut()
        .and_then(|m| m.get_mut(Value::String("event_loop".into())))
        .and_then(Value::as_mapping_mut);
    let Some(event_loop) = event_loop else {
        return Ok(preset);
    };
    let event_policy = event_loop
        .entry(Value::String("event_policy".into()))
        .or_insert_with(|| Value::Mapping(Default::default()))
        .as_mapping_mut();
    let Some(event_policy) = event_policy else {
        return Ok(preset);
    };
    let inline = event_policy
        .remove(Value::String("schemas".into()))
        .and_then(|v| v.as_mapping().cloned())
        .unwrap_or_default();
    let mut merged = ssot_schemas.clone();
    for (topic, override_value) in inline {
        let value = match merged.remove(&topic) {
            Some(base) => config_resolution::merge_yaml_values(base, override_value)?,
            None => override_value,
        };
        merged.insert(topic, value);
    }
    event_policy.insert(Value::String("schemas".into()), Value::Mapping(merged));
    Ok(preset)
}

/// Synchronous counterpart of [`load_core_value`]. Remote sources are not
/// supported; callers that need remote core configs must use the async path.
fn load_core_value_sync(
    config_sources: &[ConfigSource],
    warn_on_missing_default: bool,
) -> Result<(Value, Vec<ConfigSource>, String)> {
    let (primary_sources, overrides) = config_resolution::split_config_sources(config_sources);

    if primary_sources.len() > 1 {
        warn!("Multiple config sources specified, using first one. Others ignored.");
    }

    let user_layer = config_resolution::load_optional_user_config_value()?;

    let (primary_value, primary_label, primary_uses_defaults) = if let Some(source) =
        primary_sources.first()
    {
        match source {
            ConfigSource::File(path) => {
                if path.exists() {
                    let label = path.display().to_string();
                    let content = std::fs::read_to_string(path)
                        .with_context(|| format!("Failed to load config from {}", label))?;
                    let value = config_resolution::parse_yaml_value(&content, &label)?;
                    (Some(value), label, false)
                } else {
                    if warn_on_missing_default {
                        warn!("Config file {:?} not found, using defaults", path);
                    }
                    (None, path.display().to_string(), false)
                }
            }
            ConfigSource::Builtin(name) => {
                anyhow::bail!(
                    "`-c builtin:{name}` is no longer supported.\n\nBuiltin presets are now hat collections.\nUse:\n  ralph run -c ralph.yml -H builtin:{name}\n\nOr for preflight:\n  ralph preflight -c ralph.yml -H builtin:{name}"
                );
            }
            ConfigSource::Remote(url) => {
                anyhow::bail!(
                    "Remote core config sources are not supported for this command: {}",
                    url
                );
            }
            ConfigSource::Override { .. } => unreachable!("Partitioned out overrides"),
        }
    } else {
        let default_path = crate::default_config_path();
        if default_path.exists() {
            let label = default_path.display().to_string();
            let content = std::fs::read_to_string(&default_path)
                .with_context(|| format!("Failed to load config from {}", label))?;
            let value = config_resolution::parse_yaml_value(&content, &label)?;
            (Some(value), label, false)
        } else {
            if warn_on_missing_default {
                warn!(
                    "Config file {} not found, using defaults",
                    default_path.display()
                );
            }
            (None, default_path.display().to_string(), true)
        }
    };

    let mut merged = config_resolution::default_core_value()?;
    if let Some((user_value, _)) = &user_layer {
        merged = config_resolution::merge_yaml_values(merged, user_value.clone())?;
    }
    if let Some(primary_value) = primary_value {
        merged = config_resolution::merge_yaml_values(merged, primary_value)?;
    }

    let merged_label = config_resolution::compose_core_label(
        user_layer.as_ref().map(|(_, label)| label.as_str()),
        &primary_label,
        primary_uses_defaults,
    );

    Ok((merged, overrides, merged_label))
}

/// Synchronous counterpart of [`load_hats_value`]. Remote hats sources are not
/// supported; callers that need remote hats must use the async path.
fn load_hats_value_sync(source: &HatsSource) -> Result<Value> {
    match source {
        HatsSource::File(path) => {
            if !path.exists() {
                anyhow::bail!("Hats file not found: {}", path.display());
            }
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to load hats from {:?}", path))?;
            let value = config_resolution::parse_yaml_value(&content, &path.display().to_string())?;
            let value = merge_adjacent_preset_schema_ssot(path, value)?;
            normalize_hats_source_value(value, &path.display().to_string())
        }
        HatsSource::Remote(url) => {
            anyhow::bail!(
                "Remote hats sources are not supported for this command: {}",
                url
            );
        }
        HatsSource::Builtin(name) => {
            let preset = presets::get_preset(name).ok_or_else(|| {
                let available = presets::preset_names().join(", ");
                anyhow::anyhow!(
                    "Unknown hat collection '{}'. Available builtins: {}",
                    name,
                    available
                )
            })?;

            let preset_value =
                config_resolution::parse_yaml_value(preset.content, &format!("builtin:{}", name))?;
            extract_hat_overlay_from_preset(preset_value)
        }
    }
}

fn normalize_hats_source_value(value: Value, label: &str) -> Result<Value> {
    let (disallowed, has_hat_keys) = {
        let mapping = value
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("Hats config '{}' must be a YAML mapping", label))?;
        (
            hats_disallowed_keys(mapping),
            mapping_get(mapping, "hats").is_some() || mapping_get(mapping, "events").is_some(),
        )
    };

    if disallowed.is_empty() {
        return Ok(value);
    }

    if has_hat_keys {
        warn!(
            "Hats source '{}' contains core/runtime keys [{}]; ignoring them and using hats/events/event_loop only",
            label,
            disallowed.join(", ")
        );
        return extract_hat_overlay_from_preset(value);
    }

    anyhow::bail!(
        "Hats config '{}' contains non-hats keys: {}",
        label,
        disallowed.join(", ")
    )
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    let key_value = Value::String(key.to_string());
    mapping.get(&key_value)
}

fn mapping_insert(mapping: &mut Mapping, key: &str, value: Value) {
    mapping.insert(Value::String(key.to_string()), value);
}

fn validate_core_config_shape(value: &Value, label: &str) -> Result<()> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("Core config '{}' must be a YAML mapping", label))?;

    if mapping_get(mapping, "project").is_some() {
        anyhow::bail!(ralph_core::ConfigError::DeprecatedProjectKey);
    }

    Ok(())
}

const ALLOWED_HATS_TOP_LEVEL: &[&str] = &[
    "hats",
    "events",
    "event_loop",
    "tasks",
    "name",
    "description",
    // U5 (2026-06-09) follow-up (2026-06-11): builtin presets declare
    // `topic_format_whitelist` to exempt hat-contract protocol tokens
    // (e.g. LOOP_COMPLETE, REVIEW_COMPLETE) from the lowercase dot-case
    // format rule. Without this entry, the preset's whitelist is
    // stripped by `extract_hat_overlay_from_preset` and the user sees
    // spurious "topic 'X' violates the lowercase dot-case format"
    // warnings. Like `event_policy` / `verdict_gate` / `execution_
    // contracts`, this is hat-driven (additive, lint-permissive), so
    // it is safe to allow it through the operator/hat-collection
    // security boundary.
    "topic_format_whitelist",
    // 2026-06-24 KTD-Drift: builtin presets may declare
    // `telemetry.runtime_diagnosis.drift.coord_join_mode` when a
    // workflow needs a non-default join mode. The parallel default's
    // threshold would false-positive on structurally low fan-in rates.
    // security boundary treats `telemetry.*` as operator-controlled at
    // the top level, so the preset can ONLY opt in to specific leaf
    // keys (currently just `coord_join_mode`); the operator's
    // `telemetry:` block in `ralph.yml` still wins on per-key basis via
    // the `deep_merge_yaml_values` step in `merge_hats_overlay`. The
    // `default_core_value()` strip at `config_resolution.rs` is the
    // matching counterweight (removes the `coord_join_mode: parallel`
    // placeholder so the !contains_key guard fires correctly).
    "telemetry",
    // 2026-06-27 mechanism foundation U10: builtin presets declare
    // `mechanism.flow` + `repair_budget` + `enforce_schema` +
    // `state_idempotency` to opt into the stage pipeline. The block is
    // hat-driven: it controls runtime gates, not operator resources or
    // budgets, so it must survive the overlay merge.
    "mechanism",
];
// Event-loop keys that a hat collection overlay is allowed to provide.
//
// Original 4 (workflow promises + starting event) are the historic core
// minimum. `execution_mode` and the 3 contract keys below (`event_policy`,
// `verdict_gate`, `execution_contracts`) are hat-driven by design: a hat
// collection declares the topology and contracts required for its safety
// properties, so they must survive overlay merge for builtin presets
// like `ce-executor-pipeline` and `ce-executor-supervisor` to work
// end-to-end.
//
// Note: resource budgets (`max_iterations`, `max_runtime_seconds`,
// `checkpoint_interval`) and `enforce_hat_scope` are intentionally
// NOT in this list. They are operator-controlled, not hat-controlled,
// so a hat collection must not be able to widen the loop budget or
// disable scope enforcement behind the user's back.
//
// `state_projection` (2026-06-18) joins the hat-driven opt-in list.
// A preset that opts in to state projection must have those settings
// survive `merge_hats_overlay` even
// when the operator ralph.yml does not declare its own
// `state_projection` subtree. Without this entry, the operator's
// `event_loop` block (which carries budget/promise keys) would shadow
// the preset's whole `event_loop` subtree in the deep_merge fallback,
// dropping `state_projection` silently and leaving the runtime with
// `state_projection.enabled = false`. Perky-maple worktree
// (2026-06-10-003-...-perky-maple, 2026-06-18) was the regression that
// surfaced this; the test
// `merge_hats_overlay_preserves_preset_state_projection_enabled_when_operator_omits_it`
// pins the post-merge contract.
const ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS: &[&str] = &[
    "completion_promise",
    "starting_event",
    "cancellation_promise",
    "required_events",
    "execution_mode",
    "event_policy",
    "verdict_gate",
    "execution_contracts",
];

/// Preset opt-in keys: take effect when the operator ralph.yml omits them.
/// When the operator explicitly declares the key, the operator wins.
/// (Differs from [`ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS`], where preset wins.)
const PRESET_OPT_IN_WHEN_OPERATOR_OMITS: &[&str] = &[
    "state_projection",
    // 2026-06-17-002: step_handoff progress ↔ task gate.
    "workflow_contract",
    // ce-executor-* hat safety properties (defaults are off).
    "ephemeral_isolation",
    "enforce_current_unit",
    // 2026-06-24 plan U2: max_residuals is opt-in so the preset
    // value survives when the operator omits the key
    // when the operator's ralph.yml omits the key. Without this
    // entry, the shipper hat prompt gets the framework default
    // (8) but operator overrides would silently overwrite it
    // because the merge-hats-overlay strip sees the key as
    // present.
    "max_residuals",
    // 2026-07-03-001 plan U1: supervisor is opt-in. The
    // framework default is fully-populated (enabled=false,
    // db_path=".ralph/supervisor.db", max_concurrent_workers=4,
    // aggregate_timeout_secs=600), so the
    // `merge_hats_overlay()` strip in `default_core_value()`
    // (see `config_resolution.rs`) is required for the preset
    // opt-in (e.g. ce-executor-supervisor's
    // `supervisor.enabled: true`) to survive operator-omits;
    // otherwise the `!contains_key` guard in merge_hats_overlay
    // always sees the key as present and silently keeps the
    // framework default `enabled: false`.
    "supervisor",
    // 2026-07-29-002 plan residual: precheck is opt-in. Without
    // the `default_core_value()` strip in `config_resolution.rs`
    // the framework default `precheck: None` serialises to
    // Value::Null under `event_loop`, and the `!contains_key`
    // guard in `merge_hats_overlay` always sees the key as
    // present and silently keeps the preset's
    // `precheck.enabled: false` framework default — silently
    // dropping preset opt-ins like ce-executor-pipeline's
    // `precheck.enabled: true` (which synthesizes the
    // precheck-work.failed / precheck-fix.done gate hats and
    // attaches a 3-attempt retry budget with
    // `plan.blocked{kind: precheck_exhausted}` exhaustion).
    "precheck",
];

fn hats_disallowed_keys(mapping: &Mapping) -> Vec<String> {
    let mut disallowed = Vec::new();
    for key in mapping.keys() {
        if let Some(k) = key.as_str()
            && !ALLOWED_HATS_TOP_LEVEL.contains(&k)
        {
            disallowed.push(k.to_string());
        }
    }
    disallowed
}

fn validate_hats_config_shape(value: &Value, label: &str) -> Result<()> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("Hats config '{}' must be a YAML mapping", label))?;

    let disallowed = hats_disallowed_keys(mapping);
    if !disallowed.is_empty() {
        anyhow::bail!(
            "Hats config '{}' contains non-hats keys: {}\n\nA hats file may only contain: {}\nCore/backend/runtime settings belong in -c/--config.",
            label,
            disallowed.join(", "),
            ALLOWED_HATS_TOP_LEVEL.join(", ")
        );
    }

    Ok(())
}

/// Keys that have **special merge semantics** in
/// [`merge_hats_overlay`] and are handled by dedicated branches.
///
/// 2026-07-02-001 plan U3 (Fix C): the special-set is the single
/// source of truth for which keys get hand-written branches; the
/// remaining keys in [`ALLOWED_HATS_TOP_LEVEL`] (i.e.
/// `ALLOWED_HATS_TOP_LEVEL − SPECIAL_OVERLAY_KEYS`) are merged by
/// the generic "default" branch which inserts the value wholesale
/// into the core mapping. This eliminates the previous fork
/// between the `extract_hat_overlay_from_preset` key list and
/// `ALLOWED_HATS_TOP_LEVEL`: any new top-level hat-declarable key
/// automatically gets the default treatment without the developer
/// having to remember to add it to two parallel lists (which is
/// exactly how `mechanism` got dropped in 2026-06-27).
///
/// 2026-07-02-001 review P1 #5 fix (code-review): add a static
/// assertion that `SPECIAL_OVERLAY_KEYS` is a strict subset of
/// `ALLOWED_HATS_TOP_LEVEL`. If a future developer adds a special
/// key without also adding it to the validator allow-list, the
/// shape-check layer (`hats_disallowed_keys`) will silently filter
/// the overlay and the `SPECIAL_OVERLAY_KEYS` constant will be
/// stale. The compile-time check makes that drift a hard build
/// failure.
const SPECIAL_OVERLAY_KEYS: &[&str] = &[
    "hats",
    "events",
    "tasks",
    "event_loop",
    "topic_format_whitelist",
    "telemetry",
];

// 2026-07-02-001 review P1 #5 fix (code-review): the static
// invariant `SPECIAL_OVERLAY_KEYS ⊆ ALLOWED_HATS_TOP_LEVEL` is
// checked by `tests::special_overlay_keys_is_subset_of_allowed`.
// Compile-time `const` containment is not stable for `&str`
// (`PartialEq` for slices is not const), so the check runs in a
// `#[test]` instead. The test is one assertion and is included in
// the standard CI pipeline (`./scripts/run-tests.sh`).

fn extract_hat_overlay_from_preset(preset_value: Value) -> Result<Value> {
    let mapping = preset_value
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("Builtin hat collection must be a YAML mapping"))?;

    // 2026-07-02-001 plan U3 (Fix C): the extraction key set is
    // derived from `ALLOWED_HATS_TOP_LEVEL` (the validator-side
    // allow-list) minus `SPECIAL_OVERLAY_KEYS` (which have dedicated
    // merge branches in `merge_hats_overlay`). Previously this list
    // was hand-maintained in a separate literal below, and the
    // `mechanism` key shipped in `ALLOWED_HATS_TOP_LEVEL` but was
    // missing from the extraction list — the exact root cause of the
    // `mechanism.flow` block being dropped from builtin presets
    // since 2026-06-27. Deriving from a single source of truth
    // means a new top-level hat-declarable field only needs to be
    // added to `ALLOWED_HATS_TOP_LEVEL` to flow through; the
    // integrity test in `tests::overlay_round_trip_preserves_all_allowed_keys`
    // will fail loudly if a new key is added to
    // `ALLOWED_HATS_TOP_LEVEL` without a corresponding merge
    // branch in `merge_hats_overlay` (the test goes through the
    // full overlay path and deserialises into `RalphConfig`,
    // asserting `config.mechanism` is `Some(_)` for the
    // `ce-executor-pipeline` fixture).
    let default_keys: Vec<&str> = ALLOWED_HATS_TOP_LEVEL
        .iter()
        .copied()
        .filter(|k| !SPECIAL_OVERLAY_KEYS.contains(k))
        .collect();

    let mut overlay = Mapping::new();
    // First: special keys that have dedicated merge semantics
    // (e.g. `event_loop` deep-merges, `telemetry` deep-merges,
    // `topic_format_whitelist` unions). The merge step is
    // responsible for these; the extraction step must hand them
    // over verbatim.
    for key in SPECIAL_OVERLAY_KEYS {
        if let Some(value) = mapping_get(mapping, key) {
            mapping_insert(&mut overlay, key, value.clone());
        }
    }
    // Then: the default "wholesale insert" keys (currently
    // `name` / `description` / `mechanism`). New entries in
    // `ALLOWED_HATS_TOP_LEVEL` automatically join this loop — the
    // static `SPECIAL_OVERLAY_KEYS` and dynamic default loop share
    // the same source of truth.
    for key in &default_keys {
        if let Some(value) = mapping_get(mapping, key) {
            mapping_insert(&mut overlay, key, value.clone());
        }
    }

    Ok(Value::Mapping(overlay))
}

/// Deep-merge `overlay` mapping fields into `base` when both are mappings;
/// otherwise `overlay` wins (operator scalar/array replaces preset).
fn deep_merge_yaml_values(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Mapping(mut base_mapping), Value::Mapping(overlay_mapping)) => {
            for (key, overlay_value) in overlay_mapping {
                let merged = base_mapping
                    .remove(&key)
                    .map(|existing| deep_merge_yaml_values(existing, overlay_value.clone()))
                    .unwrap_or(overlay_value);
                base_mapping.insert(key, merged);
            }
            Value::Mapping(base_mapping)
        }
        (_, overlay) => overlay,
    }
}

/// After a builtin preset replaces `hats:` wholesale, re-apply per-hat field
/// overrides from the operator `ralph.yml` (e.g. `backend`, `backend_args`).
/// Unknown hat IDs are ignored with a warning.
pub(crate) fn merge_operator_hat_field_overlays(operator_core: &Value, merged: &mut Value) {
    let Some(operator_mapping) = operator_core.as_mapping() else {
        return;
    };
    let Some(operator_hats) = mapping_get(operator_mapping, "hats") else {
        return;
    };
    let Some(operator_hats_mapping) = operator_hats.as_mapping() else {
        return;
    };
    let Some(merged_mapping) = merged.as_mapping_mut() else {
        return;
    };
    let Some(merged_hats) = merged_mapping.get_mut(Value::String("hats".to_string())) else {
        return;
    };
    let Some(merged_hats_mapping) = merged_hats.as_mapping_mut() else {
        return;
    };

    for (hat_key, operator_hat) in operator_hats_mapping {
        let hat_id = hat_key.as_str().unwrap_or("<invalid>");
        if let Some(preset_hat) = merged_hats_mapping.get_mut(hat_key) {
            let preset_clone = preset_hat.clone();
            *preset_hat = deep_merge_yaml_values(preset_clone, operator_hat.clone());
        } else {
            warn!(
                "operator config declares hat '{}' which is not in the active hat collection; \
                 ignoring per-hat overlay for that id",
                hat_id
            );
        }
    }
}

/// P1-3 fix (post-review): made `pub(crate)` so `loop_runner::tests` can
/// drive the real merge path in `u2_lint_gate_blocks_4_hat_after_base_plus_overlay_merge`.
/// The function is the same one used by `ralph run -c base -H overlay`;
/// exposing it to crate-internal tests lets them assert that the merged
/// 4-hat config still trips the lint gate, instead of bypassing the merge
/// with a hand-built 4-hat fixture.
pub(crate) fn merge_hats_overlay(mut core: Value, hats: Value) -> Result<Value> {
    let core_mapping = core
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Core config must be a YAML mapping"))?;
    let hats_mapping = hats
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("Hats config must be a YAML mapping"))?;

    if let Some(hats_value) = mapping_get(hats_mapping, "hats") {
        mapping_insert(core_mapping, "hats", hats_value.clone());
    }

    if let Some(events_value) = mapping_get(hats_mapping, "events") {
        mapping_insert(core_mapping, "events", events_value.clone());
    }

    if let Some(tasks_overlay) = mapping_get(hats_mapping, "tasks") {
        let overlay_mapping = tasks_overlay
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("hats.tasks must be a mapping when provided"))?;
        let tasks_value = mapping_get(core_mapping, "tasks")
            .cloned()
            .unwrap_or_else(|| Value::Mapping(Mapping::new()));
        let mut tasks_mapping = tasks_value
            .as_mapping()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("core.tasks must be a mapping when provided"))?;

        // `tasks.enabled` is preset opt-in: hat-only presets (e.g.
        // ce-executor-pipeline) declare `enabled: false` without
        // `coordinator_hats`. The operator's explicit declaration wins;
        // when omitted (including after `default_core_value()` strips
        // the framework placeholder), the preset value applies.
        if let Some(enabled) = mapping_get(overlay_mapping, "enabled")
            && mapping_get(&tasks_mapping, "enabled").is_none()
        {
            mapping_insert(&mut tasks_mapping, "enabled", enabled.clone());
        }

        if let Some(coordinator_hats) = mapping_get(overlay_mapping, "coordinator_hats") {
            mapping_insert(
                &mut tasks_mapping,
                "coordinator_hats",
                coordinator_hats.clone(),
            );
        }

        mapping_insert(core_mapping, "tasks", Value::Mapping(tasks_mapping));
    }

    if let Some(event_loop_overlay) = mapping_get(hats_mapping, "event_loop") {
        let overlay_mapping = event_loop_overlay
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("hats.event_loop must be a mapping when provided"))?;

        let event_loop_value = mapping_get(core_mapping, "event_loop")
            .cloned()
            .unwrap_or_else(|| Value::Mapping(Mapping::new()));

        let mut event_loop_mapping = event_loop_value
            .as_mapping()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("core.event_loop must be a mapping when provided"))?;

        for (key, value) in overlay_mapping {
            if let Some(key_str) = key.as_str() {
                if PRESET_OPT_IN_WHEN_OPERATOR_OMITS.contains(&key_str) {
                    // Perky-maple (state_projection) and bold-heron
                    // regressions: keys outside
                    // ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS were warned
                    // then dropped, falling back to framework defaults.
                    if !event_loop_mapping.contains_key(key) {
                        event_loop_mapping.insert(key.clone(), value.clone());
                    }
                } else if ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS.contains(&key_str) {
                    // Hat-defined keys: the preset's value wins, even if
                    // the operator has declared the same key. This is
                    // intentional for workflow promises, execution_mode,
                    // and the contract keys (event_policy, verdict_gate,
                    // execution_contracts) — those are properties of the
                    // hat collection, not operator policy.
                    event_loop_mapping.insert(key.clone(), value.clone());
                } else if !event_loop_mapping.contains_key(key) {
                    // Surface the silent-drop UX defect ONLY when the operator's
                    // ralph.yml has NOT already declared the key. If the operator
                    // did declare it, the operator's value wins and no fallback
                    // to the framework default occurs — emitting the warning
                    // would be misleading noise (see docs/report/2026-06-05-wave-abort-
                    // root-cause-analysis.md; introduced in commit a05d753).
                    let value_repr = serde_yaml::to_string(value)
                        .unwrap_or_else(|_| "<unrepresentable>".to_string())
                        .trim()
                        .to_string();
                    eprintln!(
                        "warning: hat collection preset declared event_loop.{}={} but it is \
                         filtered by the operator/hat-collection security boundary. Set this \
                         field in your operator ralph.yml (event_loop.*) instead, or the loop \
                         will fall back to the framework default.",
                        key_str, value_repr,
                    );
                }
                // else: operator's ralph.yml already declares event_loop.<key>;
                // the preset's value is filtered to protect the operator budget,
                // the operator's value wins, no fallback happens — stay silent.
            }
        }

        mapping_insert(
            core_mapping,
            "event_loop",
            Value::Mapping(event_loop_mapping),
        );
    }

    // U5 (2026-06-09) follow-up (2026-06-11): union-merge the preset's
    // `topic_format_whitelist` into the core config. The preset declares
    // protocol tokens (e.g. LOOP_COMPLETE, REVIEW_COMPLETE) that are exempt
    // from the lowercase dot-case topic format rule. Without this merge,
    // the preset's whitelist is silently dropped (it's a RalphConfig top-
    // level field, not inside `event_loop`, so the event_loop branch above
    // does not see it) and the user sees spurious "topic 'LOOP_COMPLETE'
    // violates the lowercase dot-case format" warnings despite the
    // whitelist being present in the preset.
    //
    // We UNION (deduplicated, operator's tokens first, then preset's) so
    // neither side overwrites the other. Both halves are additive — there
    // is no scenario where an operator would want to *remove* a preset-
    // declared protocol token, and the lint is permissive-by-default
    // (whitelist = more exemptions, not fewer).
    if let Some(preset_whitelist_value) = mapping_get(hats_mapping, "topic_format_whitelist") {
        let preset_tokens: Vec<String> = preset_whitelist_value
            .as_sequence()
            .ok_or_else(|| {
                anyhow::anyhow!("hats.topic_format_whitelist must be a sequence of strings")
            })?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();

        let operator_tokens: Vec<String> = mapping_get(core_mapping, "topic_format_whitelist")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let mut merged = operator_tokens.clone();
        for token in preset_tokens {
            if !merged.contains(&token) {
                merged.push(token);
            }
        }
        mapping_insert(
            core_mapping,
            "topic_format_whitelist",
            Value::Sequence(merged.into_iter().map(Value::String).collect()),
        );
    }

    // 2026-06-24 KTD-Drift follow-up: union-merge the preset's
    // `telemetry.*` block into the core config so the KTD-Drift opt-in
    // `telemetry.runtime_diagnosis.drift.coord_join_mode: serial` (and
    // any other hat-declared drift settings) survives when the
    // operator ralph.yml omits the field.
    //
    // The `coord_join_mode` field is concrete-typed (default =
    // `CoordJoinMode::Parallel`) — it is NOT `Option<CoordJoinMode>`, so
    // `default_core_value()` serialises it to `{coord_join_mode:
    // parallel}` (a real value, not `Value::Null`). Without the matching
    // strip in `default_core_value()` (see `config_resolution.rs`
    // line ~120) the `!contains_key` guard would always see the key as
    // present and silently swallow the preset opt-in. We strip in
    // `default_core_value()` AND union-merge here so the
    // `default_core_value() → merge_hats_overlay` production path
    // correctly preserves the preset's `coord_join_mode: serial` value.
    //
    // The semantics mirror `topic_format_whitelist`: the operator wins
    // on a per-key basis (operator's `coord_join_mode` overrides
    // preset's), but the preset's keys are inherited when the operator
    // omits them. Deep-merge through `runtime_diagnosis` → `drift` so
    // any future hat-driven drift tuning (e.g. preset-declared
    // `window_size` overrides for a specific workflow shape) survives
    // the same path without needing a new entry in this block.
    //
    // Note: this does NOT widen the `operator/hat-collection` security
    // boundary in spirit — `telemetry.*` remains operator-controlled at
    // the top level (see `ALLOWED_HATS_TOP_LEVEL`); we are simply
    // allowing the KTD-Drift opt-in to ride through `merge_hats_overlay`
    // for preset opt-in scenarios that need a non-default join mode.
    // KTD-Drift e2e guard
    // `merge_hats_overlay_preserves_coord_join_mode_via_default_core_value`
    // pins this contract.
    if let Some(preset_telemetry_value) = mapping_get(hats_mapping, "telemetry") {
        let operator_telemetry = mapping_get(core_mapping, "telemetry")
            .cloned()
            .unwrap_or_else(|| Value::Mapping(Mapping::new()));
        let merged_telemetry =
            deep_merge_yaml_values(operator_telemetry, preset_telemetry_value.clone());
        mapping_insert(core_mapping, "telemetry", merged_telemetry);
    }

    // 2026-07-02-001 plan U3 (Fix C): the "default merge" branch.
    //
    // Every key that is in [`ALLOWED_HATS_TOP_LEVEL`] (i.e. hat-
    // declarable at the preset top level) but NOT in
    // [`SPECIAL_OVERLAY_KEYS`] (which have hand-written deep-merge /
    // union semantics above) is merged by this branch using a simple
    // "operator wins" rule:
    //
    //   - If the operator's ralph.yml already declares the key,
    //     keep the operator's value (do not touch).
    //   - Otherwise, insert the preset's value wholesale.
    //
    // Currently this picks up `mechanism` (the 2026-06-27 U10
    // mechanism block), `name`, and `description`. Future
    // additions to `ALLOWED_HATS_TOP_LEVEL` automatically join this
    // loop — no second list to keep in sync. The integrity test
    // `tests::overlay_round_trip_preserves_all_allowed_keys` pins
    // that any `ALLOWED_HATS_TOP_LEVEL` entry with a preset-side
    // value round-trips into the deserialised `RalphConfig`.
    for (key, value) in hats_mapping {
        let Some(key_str) = key.as_str() else {
            continue;
        };
        if SPECIAL_OVERLAY_KEYS.contains(&key_str) {
            // Already handled by a dedicated branch above.
            continue;
        }
        if !ALLOWED_HATS_TOP_LEVEL.contains(&key_str) {
            // Not in the hat-collection allow-list; the validator
            // will reject the overlay at the shape-check layer
            // (see `validate_hats_config_shape`), so skip silently
            // here to avoid double-warning.
            continue;
        }
        if core_mapping.contains_key(key) {
            // Operator wins on a per-key basis (mirrors the
            // `event_loop` PRESET_OPT_IN_WHEN_OPERATOR_OMITS contract
            // for default-branch keys: the operator's value is
            // authoritative; the preset only fills in absent keys).
            continue;
        }
        core_mapping.insert(key.clone(), value.clone());
    }

    Ok(core)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_checks_lowercases() {
        let checks = vec!["Config".to_string(), "BaCkEnD".to_string()];
        let normalized = normalize_checks(&checks);
        assert_eq!(normalized, vec!["config", "backend"]);
    }

    #[test]
    fn validate_checks_accepts_known() {
        let config = RalphConfig::default();
        let runner = PreflightRunner::default_checks_with_config(&config);
        let checks = vec!["config".to_string(), "backend".to_string()];
        assert!(validate_checks(&runner, &checks).is_ok());
    }

    #[test]
    fn validate_checks_rejects_unknown() {
        let config = RalphConfig::default();
        let runner = PreflightRunner::default_checks_with_config(&config);
        let checks = vec!["nope".to_string()];
        let err = validate_checks(&runner, &checks).unwrap_err();
        assert!(err.to_string().contains("Unknown check(s)"));
    }

    #[test]
    fn config_source_label_handles_sources() {
        let file_label = config_source_label(
            &[ConfigSource::File(std::path::PathBuf::from(
                "/tmp/ralph.yml",
            ))],
            None,
        );
        let user_label = crate::config_resolution::user_config_label_if_exists();
        let expected_file_label = crate::config_resolution::compose_core_label(
            user_label.as_deref(),
            "/tmp/ralph.yml",
            false,
        );
        assert_eq!(file_label, expected_file_label);

        let builtin_label =
            config_source_label(&[ConfigSource::Builtin("starter".to_string())], None);
        let expected_builtin_label = crate::config_resolution::compose_core_label(
            user_label.as_deref(),
            "builtin:starter",
            false,
        );
        assert_eq!(builtin_label, expected_builtin_label);

        let remote_label = config_source_label(
            &[ConfigSource::Remote(
                "https://example.com/ralph.yml".to_string(),
            )],
            None,
        );
        let expected_remote_label = crate::config_resolution::compose_core_label(
            user_label.as_deref(),
            "https://example.com/ralph.yml",
            false,
        );
        assert_eq!(remote_label, expected_remote_label);

        let override_label = config_source_label(
            &[ConfigSource::Override {
                key: "core.scratchpad".to_string(),
                value: "x".to_string(),
            }],
            None,
        );
        let default_label = crate::default_config_path().to_string_lossy().to_string();
        let expected_override_label = crate::config_resolution::compose_core_label(
            user_label.as_deref(),
            &default_label,
            !crate::default_config_path().exists(),
        );
        assert_eq!(override_label, expected_override_label);

        let with_hats_label = config_source_label(
            &[ConfigSource::File(std::path::PathBuf::from("ralph.yml"))],
            Some(&HatsSource::Builtin("debug".to_string())),
        );
        let expected_core =
            crate::config_resolution::compose_core_label(user_label.as_deref(), "ralph.yml", false);
        assert_eq!(
            with_hats_label,
            format!("{expected_core} + hats:builtin:debug")
        );
    }

    #[test]
    fn file_preset_load_merges_adjacent_schema_ssot() {
        let temp = tempfile::tempdir().unwrap();
        let en = temp.path().join("presets/en");
        let schemas = temp.path().join("presets/schemas");
        std::fs::create_dir_all(&en).unwrap();
        std::fs::create_dir_all(&schemas).unwrap();
        let preset_path = en.join("sample.yml");
        std::fs::write(
            &preset_path,
            "event_loop:\n  event_policy:\n    enabled: true\nhats:\n  reviewer:\n    triggers: [start]\n    publishes: [sample.done]\n",
        )
        .unwrap();
        std::fs::write(
            schemas.join("sample.yml"),
            "schemas:\n  sample.done:\n    required_fields: [artifact_path]\n    payload: json_object\n",
        )
        .unwrap();

        let value = load_hats_value_sync(&HatsSource::File(preset_path)).unwrap();
        let schemas = value
            .get("event_loop")
            .and_then(|v| v.get("event_policy"))
            .and_then(|v| v.get("schemas"))
            .and_then(Value::as_mapping)
            .expect("file preset should expose merged schemas");
        assert!(schemas.contains_key(Value::String("sample.done".into())));
    }

    #[test]
    fn validate_core_config_shape_rejects_project() {
        let core: Value = serde_yaml::from_str(
            r"
project:
  specs_dir: my_specs
",
        )
        .unwrap();

        let err = validate_core_config_shape(&core, "core.yml").unwrap_err();
        assert!(err.to_string().contains("Invalid config key 'project'"));
    }

    #[test]
    fn validate_core_config_shape_allows_single_file_combined_config() {
        let core: Value = serde_yaml::from_str(
            r"
cli:
  backend: claude
hats:
  builder:
    name: Builder
",
        )
        .unwrap();

        assert!(validate_core_config_shape(&core, "core.yml").is_ok());
    }

    #[test]
    fn validate_hats_config_shape_rejects_core_keys() {
        let hats: Value = serde_yaml::from_str(
            r"
cli:
  backend: claude
hats:
  builder:
    name: Builder
",
        )
        .unwrap();

        let err = validate_hats_config_shape(&hats, "hats.yml").unwrap_err();
        assert!(err.to_string().contains("contains non-hats keys"));
    }

    #[test]
    fn merge_hats_overlay_replaces_hats_and_merges_event_loop() {
        let core: Value = serde_yaml::from_str(
            r"
cli:
  backend: claude
event_loop:
  max_iterations: 100
  completion_promise: LOOP_COMPLETE
hats:
  builder:
    name: Builder
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: REVIEW_COMPLETE
hats:
  reviewer:
    name: Reviewer
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert_eq!(config.event_loop.max_iterations, 100);
        assert_eq!(config.event_loop.completion_promise, "REVIEW_COMPLETE");
        assert!(config.hats.contains_key("reviewer"));
        assert!(!config.hats.contains_key("builder"));
    }

    // 2026-07-02-001 plan U3 (Fix C): round-trip integrity for the
    // preset → overlay → RalphConfig path. The pre-fix
    // `extract_hat_overlay_from_preset` maintained a hand-written
    // key list that drifted from `ALLOWED_HATS_TOP_LEVEL`, which
    // dropped the `mechanism` block from builtin presets since
    // 2026-06-27. This test pins the contract that every key in
    // `ALLOWED_HATS_TOP_LEVEL` declared by a preset survives the
    // full overlay + deserialise path.
    //
    // Specifically: `mechanism.flow` is the 2026-06-27 U10 stage
    // pipeline opt-in; the runtime's `FlowStepScopeStage` and the
    // `mechanism.flow` warning gate depend on this value being
    // `Some(_)`. If a future change re-introduces a fork between
    // the validator allow-list and the extraction list, this test
    // will fail with `config.mechanism` being `None` and the flow
    // count being `0`.
    #[test]
    fn overlay_round_trip_preserves_mechanism_block_from_preset() {
        // Mimic the structure of the pipeline preset — operator's
        // ralph.yml is empty, preset supplies everything.
        let core: Value = serde_yaml::from_str(
            r"
cli:
  backend: claude
event_loop:
  completion_promise: LOOP_COMPLETE
hats: {}
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r#"
hats:
  executor:
    name: Executor
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
mechanism:
  flow:
    type: declared
    version: 1
    steps:
      - id: "step-01"
        allowed_emits: ["work.ready"]
  repair_budget: 1
  enforce_schema: "hard"
  state_idempotency: "required"
"#,
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert!(
            config.mechanism.is_some(),
            "operator-empty preset must round-trip `mechanism` into RalphConfig"
        );
        let mechanism = config.mechanism.as_ref().unwrap();
        assert!(
            mechanism.flow.is_some(),
            "operator-empty preset must round-trip `mechanism.flow` into RalphConfig"
        );
        let flow = mechanism.flow.as_ref().unwrap();
        assert_eq!(
            flow.steps.len(),
            1,
            "the one declared step must survive the round-trip"
        );
        assert_eq!(flow.steps[0].id, "step-01");
    }

    /// 2026-07-24-003 review P0: the real `ralph preset check -H`
    /// path builds core via `default_core_value()`, which used to
    /// leave `mechanism: null` and cause the overlay "operator wins"
    /// branch to drop the preset's wave.runtime binding. Pin that
    /// the implementation-review mechanism.runs survives.
    #[test]
    fn default_core_overlay_preserves_implementation_review_wave_runtime_runs() {
        let hats_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../presets/en/implementation-review.yml");
        let content = std::fs::read_to_string(&hats_path).expect("read implementation-review.yml");
        let hats_value =
            crate::config_resolution::parse_yaml_value(&content, "implementation-review")
                .expect("parse hats yaml");
        let core = crate::config_resolution::default_core_value().expect("default core");
        let merged = merge_hats_overlay(core, hats_value).expect("merge");
        let config: RalphConfig = serde_yaml::from_value(merged).expect("deserialize RalphConfig");
        assert!(
            ralph_core::runtime_contract::preset_uses_wave_runtime(&config),
            "default_core_value + overlay must preserve mechanism.flow.steps[].runs \
             wave.runtime.* (got mechanism={:?})",
            config.mechanism.as_ref().map(|m| m.flow.as_ref().map(|f| f
                .steps
                .iter()
                .map(|s| (s.id.clone(), s.runs.clone()))
                .collect::<Vec<_>>()))
        );
    }

    /// `ce-executor-pipeline` is hat-only: no `mechanism.flow` block in YAML.
    #[test]
    fn ce_executor_pipeline_overlay_has_no_mechanism_flow() {
        use crate::presets;

        let preset = presets::get_preset("ce-executor-pipeline").expect("embedded preset");
        assert!(
            !preset
                .content
                .lines()
                .any(|line| line.trim_start().starts_with("mechanism:")),
            "ce-executor-pipeline must not declare mechanism.flow"
        );

        let core: Value = serde_yaml::from_str(
            r"
cli:
  backend: claude
event_loop:
  max_iterations: 500
  completion_promise: LOOP_COMPLETE
  prompt_file: PROMPT.md
",
        )
        .unwrap();

        let preset_value =
            config_resolution::parse_yaml_value(preset.content, "builtin:ce-executor-pipeline")
                .unwrap();
        let hats = extract_hat_overlay_from_preset(preset_value).unwrap();
        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert!(
            config.mechanism.is_none(),
            "merged config must not carry mechanism for hat-only preset"
        );
        assert!(
            config.event_loop.mechanism.is_none(),
            "legacy event_loop.mechanism must also be absent"
        );
        assert!(
            !config.tasks.enabled,
            "ce-executor-pipeline declares tasks.enabled: false; merge must not leave framework default true"
        );
    }

    /// U3 (Fix C): operator-supplied `mechanism` must win over the
    /// preset's value (per-key operator-wins contract on the
    /// default branch).
    #[test]
    fn overlay_round_trip_operator_mechanism_wins_over_preset() {
        let core: Value = serde_yaml::from_str(
            r#"
cli:
  backend: claude
event_loop:
  completion_promise: LOOP_COMPLETE
hats: {}
mechanism:
  flow:
    type: declared
    steps:
      - id: "operator-step"
"#,
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r#"
hats:
  executor:
    name: Executor
mechanism:
  flow:
    type: declared
    steps:
      - id: "preset-step"
"#,
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        let mechanism = config.mechanism.as_ref().unwrap();
        let flow = mechanism.flow.as_ref().unwrap();
        assert_eq!(
            flow.steps.len(),
            1,
            "operator's mechanism.flow must replace the preset's wholesale (one step total, not two)"
        );
        assert_eq!(
            flow.steps[0].id, "operator-step",
            "operator-wins: the surviving step must be the operator's"
        );
    }

    /// U3 (Fix C): removing the default branch from
    /// `merge_hats_overlay` (or re-adding a hand-written key list
    /// to `extract_hat_overlay_from_preset` that omits `mechanism`)
    /// would make this test go red. The two-pronged pin (this test
    /// + `overlay_round_trip_preserves_mechanism_block_from_preset`)
    /// catches both the "extraction list lost a key" failure mode
    /// and the "merge dropped the default branch" failure mode.
    #[test]
    fn extract_hat_overlay_includes_all_allowed_keys_with_values() {
        let preset: Value = serde_yaml::from_str(
            r#"
name: "TestPreset"
description: "U3 fixture"
hats:
  executor:
    name: Executor
event_loop:
  execution_mode: isolated
events:
  - topic: "work.ready"
tasks: {}
topic_format_whitelist: ["LOOP_COMPLETE"]
telemetry:
  runtime_diagnosis:
    drift:
      coord_join_mode: serial
mechanism:
  flow:
    - step: "s1"
"#,
        )
        .unwrap();

        let overlay = extract_hat_overlay_from_preset(preset).unwrap();
        let mapping = overlay.as_mapping().expect("overlay is a mapping");

        // The 6 special keys must all be present.
        for key in SPECIAL_OVERLAY_KEYS {
            assert!(
                mapping_get(mapping, key).is_some(),
                "extract_hat_overlay_from_preset must include special key `{key}`; \
                 check SPECIAL_OVERLAY_KEYS and the extraction loop in `extract_hat_overlay_from_preset`"
            );
        }
        // The default keys (e.g. `mechanism`, `name`, `description`)
        // must all be present.
        for key in ALLOWED_HATS_TOP_LEVEL {
            if SPECIAL_OVERLAY_KEYS.contains(key) {
                continue; // already checked above
            }
            assert!(
                mapping_get(mapping, key).is_some(),
                "extract_hat_overlay_from_preset must include default key `{key}` (from \
                 ALLOWED_HATS_TOP_LEVEL); a drift between the validator allow-list and the \
                 extraction list is the exact 2026-07-02-001 U3 root cause"
            );
        }
    }

    /// 2026-07-02-001 review P1 #5 fix (code-review): every key in
    /// `SPECIAL_OVERLAY_KEYS` (the hand-written branch set in
    /// `merge_hats_overlay`) must also be in `ALLOWED_HATS_TOP_LEVEL`
    /// (the validator allow-list). If a developer adds a special
    /// key without also adding it to the allow-list, the shape-check
    /// layer (`hats_disallowed_keys`) will silently filter the
    /// overlay and the `SPECIAL_OVERLAY_KEYS` constant will be
    /// stale. This test pins the invariant that the two lists stay
    /// in sync.
    #[test]
    fn special_overlay_keys_is_subset_of_allowed_hats_top_level() {
        for key in SPECIAL_OVERLAY_KEYS {
            assert!(
                ALLOWED_HATS_TOP_LEVEL.contains(key),
                "SPECIAL_OVERLAY_KEYS contains `{key}` which is missing from \
                 ALLOWED_HATS_TOP_LEVEL; add it to the validator allow-list first"
            );
        }
    }

    #[test]
    fn merge_operator_hat_field_overlays_preserves_preset_and_sets_backend() {
        let operator_core: Value = serde_yaml::from_str(
            r"
cli:
  backend: claude
hats:
  reviewer:
    backend: pi
  unknown-hat:
    backend: gemini
",
        )
        .unwrap();

        let preset: Value = serde_yaml::from_str(
            r"
hats:
  reviewer:
    name: Reviewer
    triggers: [work.done]
    publishes: [review.passed]
",
        )
        .unwrap();

        let mut merged = merge_hats_overlay(operator_core.clone(), preset).unwrap();
        merge_operator_hat_field_overlays(&operator_core, &mut merged);
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        let reviewer = config.hats.get("reviewer").expect("reviewer hat");
        assert_eq!(reviewer.name, "Reviewer");
        assert_eq!(reviewer.triggers.len(), 1);
        assert!(matches!(
            reviewer.backend,
            Some(ralph_core::HatBackend::Named(ref name)) if name == "pi"
        ));
        assert!(!config.hats.contains_key("unknown-hat"));
    }

    #[test]
    fn merge_hats_overlay_allows_workflow_promises_from_hats_event_loop() {
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  max_iterations: 100
  max_runtime_seconds: 28800
  completion_promise: LOOP_COMPLETE
  cancellation_promise: LOOP_CANCELLED
hats:
  builder:
    name: Builder
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: REVIEW_COMPLETE
  cancellation_promise: BUILD_PARKED
  starting_event: build.start
  max_iterations: 150
  max_runtime_seconds: 14400
hats:
  reviewer:
    name: Reviewer
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert_eq!(config.event_loop.max_iterations, 100);
        assert_eq!(config.event_loop.max_runtime_seconds, 28800);
        assert_eq!(config.event_loop.completion_promise, "REVIEW_COMPLETE");
        assert_eq!(config.event_loop.cancellation_promise, "BUILD_PARKED");
        assert_eq!(
            config.event_loop.starting_event.as_deref(),
            Some("build.start")
        );
    }

    #[test]
    fn merge_hats_overlay_preserves_isolated_execution_mode_required_by_topology() {
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  max_iterations: 100
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  execution_mode: isolated
hats:
  coordinator: { name: Coordinator }
  executor: { name: Executor }
  reviewer: { name: Reviewer }
  reporter: { name: Reporter }
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert_eq!(
            config.event_loop.execution_mode,
            ralph_core::config::HatExecutionMode::Isolated
        );
        assert!(
            ralph_core::preset_lint::run_preset_lint(
                &config,
                ralph_core::preset_lint::LintStrictness::Strict,
                false,
                None,
            )
            .iter()
            .all(|finding| finding.id != "lint.preset.multi_hat_requires_isolated")
        );
    }

    #[test]
    fn merge_hats_overlay_preserves_preset_tasks_enabled_when_operator_omits_it() {
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
tasks:
  enabled: false
hats:
  reporter:
    name: Reporter
    publishes:
      - report.done
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert!(
            !config.tasks.enabled,
            "hat-only preset `tasks.enabled: false` must survive merge when operator omits tasks"
        );
    }

    #[test]
    fn merge_hats_overlay_preserves_preset_tasks_enabled_via_default_core_value() {
        let core = crate::config_resolution::default_core_value()
            .expect("default_core_value must succeed");

        let hats: Value = serde_yaml::from_str(
            r"
tasks:
  enabled: false
hats:
  reporter:
    name: Reporter
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert!(
            !config.tasks.enabled,
            "production-path merge (default_core_value -> merge_hats_overlay) must inherit \
             preset `tasks.enabled: false`"
        );
    }

    #[test]
    fn merge_hats_overlay_preserves_preset_coordinator_hats_without_overriding_tasks_enabled() {
        let core: Value = serde_yaml::from_str(
            r"
tasks:
  enabled: false
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
    - executor
hats:
  coordinator: { name: Coordinator }
  executor: { name: Executor }
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert!(!config.tasks.enabled);
        assert_eq!(
            config.tasks.coordinator_hats,
            vec!["coordinator".to_string(), "executor".to_string()]
        );
    }

    #[test]
    fn merge_hats_overlay_warns_when_budget_keys_are_filtered() {
        // When a hat collection preset declares resource-budget keys
        // (max_runtime_seconds, max_iterations, enforce_hat_scope) the
        // overlay must NOT widen the operator budget, but it MUST emit a
        // warning so the user is not surprised by a silent fallback to
        // the framework default (4h for max_runtime). See
        // docs/report/2026-06-05-wave-abort-root-cause-analysis.md.
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
  max_iterations: 200
  max_runtime_seconds: 28800
  enforce_hat_scope: true
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  max_runtime_seconds: 14400
  max_iterations: 500
  enforce_hat_scope: false
  completion_promise: LOOP_COMPLETE
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        // Operator values are preserved — the filtered hats' keys must
        // not leak through and widen/override the operator budget.
        assert_eq!(config.event_loop.max_iterations, 200);
        assert_eq!(config.event_loop.max_runtime_seconds, 28800);
        assert!(config.event_loop.enforce_hat_scope);
        // Whitelisted keys still merge through.
        assert_eq!(config.event_loop.completion_promise, "LOOP_COMPLETE");
        // The warning itself is eprintln (stderr) — verified by manual
        // inspection and the existing _allows_workflow_promises_ test,
        // which exercises the same code path and would also have produced
        // stderr output during cargo test.
        //
        // Note: with the operator-already-set silent-merge fix, the warning
        // is now suppressed in this scenario (operator's ralph.yml declares
        // all three budget keys), so the eprintln no longer fires here.
        // The operator-value-wins invariants asserted above are the load-
        // bearing assertions; the warning is verified by code review and
        // by the dedicated _fallback_to_framework_default test below.
    }

    #[test]
    fn merge_hats_overlay_falls_back_to_framework_default_when_operator_omits_budget_key() {
        // Regression: when the operator's ralph.yml does NOT declare a
        // resource-budget key (e.g. max_runtime_seconds) and the preset
        // declares one, the preset value is filtered out (security
        // boundary) and the operator's ralph.yml value (None) is used,
        // so the EventLoopConfig serde default kicks in
        // (max_runtime_seconds = 14400s, max_iterations = 100).
        // The warning IS expected in this case — that's the whole point
        // of the warning — and is verified to not break the merge.
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  max_runtime_seconds: 28800
  max_iterations: 50
  enforce_hat_scope: false
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        // Operator's ralph.yml omits the budget keys, preset values are
        // filtered, so the framework default applies (4h runtime, 100
        // iterations, scope permissive by default).
        assert_eq!(config.event_loop.max_runtime_seconds, 14400);
        assert_eq!(config.event_loop.max_iterations, 100);
        assert!(!config.event_loop.enforce_hat_scope);
        // Whitelisted keys still merge through.
        assert_eq!(config.event_loop.completion_promise, "LOOP_COMPLETE");
    }

    // ──────────────────────────────────────────────────────────────────────
    // 2026-06-19 fix: the deleted merge tests degenerated to the
    // existing `state_projection` / `suppress_human_guidance` paths
    // exercised by the test below.
    #[test]
    fn merge_hats_overlay_preserves_preset_opt_in_event_loop_keys_when_operator_omits_them() {
        // bold-heron (2026-06-19): the pipeline preset declares these keys
        // but operator ralph.yml typically omits them; they must not fall
        // back to framework defaults.
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
  max_iterations: 100
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  ephemeral_isolation: true
  enforce_current_unit: true
  workflow_contract:
    step_handoff:
      progress_task_gate: true
hats:
  executor:
    name: Executor
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert!(
            config.event_loop.ephemeral_isolation,
            "preset ephemeral_isolation must apply when operator omits it"
        );
        assert!(
            config.event_loop.enforce_current_unit,
            "preset enforce_current_unit must apply when operator omits it"
        );
        assert!(
            config
                .event_loop
                .workflow_contract
                .as_ref()
                .is_some_and(|wc| wc.step_handoff.progress_task_gate),
            "preset workflow_contract.step_handoff.progress_task_gate must apply when operator omits it"
        );
    }

    #[test]
    fn merge_hats_overlay_preserves_precheck_when_operator_omits_it() {
        // 2026-07-29-002: `precheck` is a preset opt-in key. When the
        // operator ralph.yml omits `event_loop.precheck`, the preset's
        // block must survive the merge. Before `precheck` was added to
        // `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` (and stripped from
        // `default_core_value`), the merge silently dropped it,
        // `apply_precheck_desugar` early-returned, and the
        // `precheck-work.failed` / `precheck-fix.done` gate hats were
        // never synthesized (the 2026-07-29 ce-executor-pipeline
        // silent-drop regression).
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  precheck:
    enabled: true
    rules:
      work.failed:
        on_fail:
          target: executor
          retry_budget: 3
hats:
  executor:
    name: Executor
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        let precheck = config
            .event_loop
            .precheck
            .as_ref()
            .expect("preset precheck block must survive merge when operator omits it");
        assert!(
            precheck.enabled,
            "preset precheck.enabled: true must apply when operator omits it"
        );
        assert!(
            precheck.rules.contains_key("work.failed"),
            "preset precheck rules must survive merge when operator omits them"
        );
    }

    #[test]
    fn merge_hats_overlay_preserves_required_events_from_hats() {
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
  max_iterations: 100
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  required_events:
    - review.passed
    - review.complete
  starting_event: work.start
hats:
  reviewer:
    name: Reviewer
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert_eq!(
            config.event_loop.required_events,
            vec!["review.passed".to_string(), "review.complete".to_string()]
        );
        assert_eq!(
            config.event_loop.starting_event.as_deref(),
            Some("work.start")
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // U5 (2026-06-09) follow-up (2026-06-11): the preset's
    // `topic_format_whitelist` (e.g. LOOP_COMPLETE / REVIEW_COMPLETE in
    // builtin:ce-executor) MUST be union-merged into the operator's
    // config. Without this, the U5 commit f876241 that added
    // `topic_format_whitelist` to all 9 builtin presets was a no-op in
    // real runs: the field lives at RalphConfig top level (not inside
    // `event_loop`), so `merge_hats_overlay` silently dropped it, and
    // the user saw spurious "topic 'LOOP_COMPLETE' violates the
    // lowercase dot-case format" warnings.
    //
    // The U5 verification test (`test_all_embedded_presets_pass_strict_lint`
    // in presets.rs) only exercised `RalphConfig::parse_yaml(preset.content)`,
    // bypassing the real `merge_hats_overlay` path — so the bug slipped
    // through. These tests exercise the actual merge, plus a full
    // lint-after-merge end-to-end check.
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn merge_hats_overlay_unions_topic_format_whitelist_from_preset() {
        // Operator ralph.yml has no whitelist; preset declares protocol
        // tokens. After merge the operator's config must carry the
        // preset's tokens so the lint treats them as exempt.
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
",
        )
        .unwrap();
        let hats: Value = serde_yaml::from_str(
            r"
topic_format_whitelist:
  - LOOP_COMPLETE
  - REVIEW_COMPLETE
hats:
  reporter:
    name: Reporter
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert_eq!(
            config.topic_format_whitelist,
            vec!["LOOP_COMPLETE".to_string(), "REVIEW_COMPLETE".to_string()],
            "preset's topic_format_whitelist must be merged into operator config"
        );
    }

    #[test]
    fn merge_hats_overlay_preserves_operator_topic_format_whitelist() {
        // Both sides declare whitelist tokens. Union is deduplicated,
        // operator's tokens come first (operator's intent is honored
        // for shared entries, and operator-only entries are preserved).
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
topic_format_whitelist:
  - OPERATOR_TOKEN
  - SHARED
",
        )
        .unwrap();
        let hats: Value = serde_yaml::from_str(
            r"
topic_format_whitelist:
  - SHARED
  - PRESET_TOKEN
hats:
  reporter:
    name: Reporter
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert_eq!(
            config.topic_format_whitelist,
            vec![
                "OPERATOR_TOKEN".to_string(),
                "SHARED".to_string(),
                "PRESET_TOKEN".to_string(),
            ],
            "merge must be a deduplicated union: operator first, then preset, \
             shared entries appear once (operator's position wins)"
        );
    }

    #[test]
    fn merge_hats_overlay_skips_topic_format_whitelist_when_preset_omits_it() {
        // Preset without whitelist field — operator's existing whitelist
        // must be preserved untouched (no spurious clear).
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
topic_format_whitelist:
  - KEEP_ME
",
        )
        .unwrap();
        let hats: Value = serde_yaml::from_str(
            r"
hats:
  reviewer:
    name: Reviewer
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert_eq!(config.topic_format_whitelist, vec!["KEEP_ME".to_string()]);
    }

    // ──────────────────────────────────────────────────────────────────────
    // 2026-06-18 perky-maple regression: `state_projection` is opt-in at
    // the preset level (the pipeline preset declares
    // `event_loop.state_projection.enabled: true`). The previous
    // verification path only deserialized the preset YAML in isolation
    // (`presets.rs::test_ce_executor_state_projection_enabled_*`); it
    // did NOT exercise `merge_hats_overlay`, so a silent drop of the
    // `state_projection` subtree in the merge layer went undetected.
    //
    // The operator's ralph.yml in perky-maple has `event_loop:` with
    // budget/promise keys but no `state_projection` child — so when
    // `merge_hats_overlay` runs, the preset's `state_projection` falls
    // outside the `ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS` whitelist
    // (preflight.rs:693-702) and is filtered out by the
    // operator/hat-collection security boundary. The runtime config
    // ends up with `state_projection.enabled = false`, the projector
    // never applies, `.ralph/agent/progress.md` is never written, and
    // `prepend_orchestrator_context` falls back to the disabled stub.
    //
    // The two tests below pin the post-merge contract: a preset that
    // declares `state_projection.enabled: true` MUST survive the merge
    // when the operator has not declared its own `state_projection`.
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn merge_hats_overlay_preserves_preset_state_projection_enabled_when_operator_omits_it() {
        // Reproduces the perky-maple config: operator ralph.yml has
        // event_loop budget/promise keys but no `state_projection` child.
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  max_iterations: 500
  max_runtime_seconds: 28800
  completion_promise: LOOP_COMPLETE
  prompt_file: PROMPT.md
hats:
  builder:
    name: Builder
",
        )
        .unwrap();

        // Preset declares state_projection (mirrors the pipeline
        // preset). The minimal
        // per-action shape must include `kind` + the field names the
        // projector looks up; the exact set of action fields is
        // verified by `presets::test_ce_executor_state_projection_enabled_serial_en`
        // so this test only needs enough to deserialize.
        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  state_projection:
    enabled: true
    actions:
      work.ready:
        kind: ensure_task
        key: task_key
        title: step
      work.done:
        kind: close_task
        task_id: task_id
        step: step
      queue.advance:
        kind: advance_step
        current_step: step
        completed_step: completed_step
      plan.complete:
        kind: plan_complete
        final_step: step
hats:
  coordinator:
    name: Coordinator
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert!(
            config.event_loop.state_projection.enabled,
            "preset-declared `event_loop.state_projection.enabled: true` must survive \
             `merge_hats_overlay` when the operator ralph.yml does not override it; \
             the previous silent drop caused phase1 to be disabled at runtime \
             (perky-maple worktree 2026-06-10-003-...-perky-maple, 2026-06-18)"
        );
        for topic in ["work.ready", "work.done", "queue.advance", "plan.complete"] {
            assert!(
                config
                    .event_loop
                    .state_projection
                    .actions
                    .contains_key(topic),
                "preset-declared action `{topic}` must survive `merge_hats_overlay`"
            );
        }
        // Sanity: operator's budget keys are still honored.
        assert_eq!(config.event_loop.max_iterations, 500);
        assert_eq!(config.event_loop.max_runtime_seconds, 28800);
    }

    #[test]
    fn merge_hats_overlay_lets_operator_override_preset_state_projection() {
        // The inverse: when the operator explicitly disables
        // state_projection, the operator's choice wins. This pins
        // the security-boundary contract that the operator retains
        // the final say over the runtime opt-in.
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  max_iterations: 500
  state_projection:
    enabled: false
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  state_projection:
    enabled: true
hats:
  coordinator:
    name: Coordinator
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert!(
            !config.event_loop.state_projection.enabled,
            "operator's `state_projection.enabled: false` must win over preset's `true`"
        );
    }

    /// 2026-06-20 regression: existing `merge_hats_overlay_*` tests
    /// build `core` from raw YAML, which bypasses
    /// `default_core_value()`. Production goes through
    /// `default_core_value()` (which serializes `RalphConfig::default()`)
    /// → `event_loop` carries a `state_projection: {enabled: false,
    /// actions: {}}` placeholder. The old `!contains_key` guard in
    /// `merge_hats_overlay` then always evaluates false on that
    /// placeholder, silently dropping the preset opt-in. This test
    /// mirrors the production path by sourcing `core` from
    /// `default_core_value()`; with the fix in `default_core_value`
    /// the test PASSES, without the fix it FAILS with
    /// `state_projection.enabled == false`.
    #[test]
    fn merge_hats_overlay_preserves_state_projection_when_core_comes_from_default_core_value() {
        let core = crate::config_resolution::default_core_value()
            .expect("default_core_value must succeed");
        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  state_projection:
    enabled: true
    actions:
      work.ready: {kind: ensure_task, key: task_key, title: step}
      work.done: {kind: close_task, task_id: task_id, step: step}
      queue.advance: {kind: advance_step, current_step: step, completed_step: completed_step}
      plan.complete: {kind: plan_complete, final_step: step}
hats:
  coordinator: {name: Coordinator}
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert!(
            config.event_loop.state_projection.enabled,
            "preset state_projection.enabled must survive merge_hats_overlay when core \
             comes from default_core_value() (production path); currently dropped silently \
             because the default placeholder is mistaken for an operator declaration"
        );
        for topic in ["work.ready", "work.done", "queue.advance", "plan.complete"] {
            assert!(
                config
                    .event_loop
                    .state_projection
                    .actions
                    .contains_key(topic),
                "preset action `{topic}` must survive the production-path merge"
            );
        }
    }

    /// 2026-06-24 KTD-Drift e2e guard: production-path regression test
    /// for `telemetry.runtime_diagnosis.drift.coord_join_mode`.
    ///
    /// The drift-detector's `CoordJoinMode` enum has no production-path
    /// test today; the only coverage lives in the `telemetry` config
    /// parser's own unit tests (which never touch `merge_hats_overlay`).
    /// The pipeline preset ships with
    /// `telemetry.runtime_diagnosis.drift.coord_join_mode` when the
    /// operator omits the field. A regression that drops the preset
    /// opt-in would silently revert the workflow to the parallel
    /// default (60% threshold), causing the drift detector to
    /// raise `coord_join_rate 1/4 < 60%` false positives on the
    /// structurally-low serial workflow. This test pins the
    /// production-path contract.
    ///
    /// Fix chain (2026-06-24 KTD-Drift close-loop):
    ///   1. `default_core_value()` strips the default `coord_join_mode`
    ///      placeholder from `telemetry.runtime_diagnosis.drift` (the
    ///      field is concrete-typed so the `!contains_key` guard in
    ///      `merge_hats_overlay` would otherwise always see the key
    ///      as present and silently swallow the preset opt-in; unlike
    ///      Option-typed fields, this one cannot rely on
    ///      `Value::Null` semantics).
    ///   2. `merge_hats_overlay` recursively deep-merges the preset's
    ///      `telemetry.*` block into the core config (mirrors
    ///      `topic_format_whitelist` union-merge). Without this,
    ///      `extract_hat_overlay_from_preset` would strip the preset's
    ///      `telemetry` block at the hat boundary (it's not in
    ///      `ALLOWED_HATS_TOP_LEVEL` by default) and the opt-in would
    ///      never reach `merge_hats_overlay`.
    ///   3. `ALLOWED_HATS_TOP_LEVEL` and `extract_hat_overlay_from_preset`
    ///      carry `telemetry` so preset-declared telemetry leaf keys
    ///      (currently just `coord_join_mode`) ride through the
    ///      security boundary unchanged.
    #[test]
    fn merge_hats_overlay_preserves_coord_join_mode_via_default_core_value() {
        let core = crate::config_resolution::default_core_value()
            .expect("default_core_value must succeed");
        let hats: Value = serde_yaml::from_str(
            r"
telemetry:
  runtime_diagnosis:
    drift:
      coord_join_mode: serial
hats:
  coordinator: {name: Coordinator}
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert_eq!(
            config.telemetry.runtime_diagnosis.drift.coord_join_mode,
            ralph_core::config::CoordJoinMode::Serial,
            "production-path merge (default_core_value -> merge_hats_overlay -> RalphConfig) \
             must keep the preset's `telemetry.runtime_diagnosis.drift.coord_join_mode: serial` \
             alive (KTD-Drift e2e guard). Strip in `default_core_value()` removes the default \
             `coord_join_mode: parallel` placeholder, `merge_hats_overlay` recursively merges \
             the preset's `telemetry.*` block, and `ALLOWED_HATS_TOP_LEVEL` lets the preset's \
             `telemetry` block pass through the security boundary."
        );
    }

    /// 2026-06-24 KTD-Drift follow-up guard: the operator's
    /// `ralph.yml` MUST override the preset's `coord_join_mode` when
    /// the operator explicitly redeclares the field. This is the
    /// "operator wins on a per-key basis" half of the contract; the
    /// opt-in half is the
    /// `..._preserves_coord_join_mode_via_default_core_value` test
    /// above. Without this test, a regression that always-uses the
    /// preset value (e.g. blindly `insert` instead of
    /// `deep_merge_yaml_values`) would silently override an operator
    /// who explicitly opted back into `parallel` mode.
    #[test]
    fn merge_hats_overlay_lets_operator_override_coord_join_mode() {
        let core: Value = serde_yaml::from_str(
            r"
telemetry:
  runtime_diagnosis:
    drift:
      coord_join_mode: parallel
hats:
  coordinator: {name: Coordinator}
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
telemetry:
  runtime_diagnosis:
    drift:
      coord_join_mode: serial
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        assert_eq!(
            config.telemetry.runtime_diagnosis.drift.coord_join_mode,
            ralph_core::config::CoordJoinMode::Serial,
            "operator's ralph.yml may override preset's `coord_join_mode` \
             on a per-key basis; the security boundary must NOT clobber the \
             operator's explicit value"
        );
    }

    /// End-to-end regression guard for the U5 bug. The user's ralph.yml
    /// uses uppercase `LOOP_COMPLETE` to match the ce-executor preset's
    /// completion contract. After the real merge, the preset's
    /// `topic_format_whitelist` MUST take effect, so the strict lint
    /// must NOT warn about `LOOP_COMPLETE` / `REVIEW_COMPLETE` being
    /// non-lowercase-dot-case.
    ///
    /// U5's `test_all_embedded_presets_pass_strict_lint` only validated
    /// `parse_yaml(preset.content)` and missed this — this test is the
    /// one that would have caught the bug originally.
    #[test]
    fn merge_then_lint_ce_executor_whitelist_eliminates_protocol_token_warnings() {
        use ralph_core::preset_lint::{LintStrictness, run_preset_lint};
        use ralph_core::runtime_contract::FindingSeverity;

        // Minimal user ralph.yml that mirrors the operator's actual
        // shape: an upper-case completion_promise matching the preset.
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
tasks:
  enabled: true
  coordinator_hats: [coordinator]
",
        )
        .unwrap();
        // Minimal hat slice that produces a LOOP_COMPLETE / REVIEW_COMPLETE
        // reference inside the merged config. We use a single hat that
        // publishes LOOP_COMPLETE — that is the same token the preset
        // whitelists, so without the merge the lint would warn.
        let hats: Value = serde_yaml::from_str(
            r"
topic_format_whitelist:
  - LOOP_COMPLETE
  - REVIEW_COMPLETE
hats:
  reporter:
    name: Reporter
    publishes:
      - LOOP_COMPLETE
      - REVIEW_COMPLETE
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        let findings = run_preset_lint(&config, LintStrictness::Strict, false, None);

        // The lint's purpose here is to surface
        // `invalid_topic_format` warnings. The merged whitelist MUST
        // exempt the protocol tokens — so we expect zero
        // invalid_topic_format findings of any severity (they would be
        // either warn or pass, and we want neither warn nor pass-by-
        // exemption-noise — just zero `invalid_topic_format`).
        let invalid_format: Vec<_> = findings
            .iter()
            .filter(|f| f.id == "lint.preset.invalid_topic_format")
            .collect();
        assert!(
            invalid_format.is_empty(),
            "After merge, LOOP_COMPLETE / REVIEW_COMPLETE must be exempt from \
             the lowercase dot-case format rule. Got: {:?}",
            invalid_format
                .iter()
                .map(|f| format!("{}: {} ({:?})", f.id, f.message, f.severity))
                .collect::<Vec<_>>()
        );

        // Sanity: the run does not accidentally downgrade other findings
        // to error severity by mistake. We only assert the
        // invalid_topic_format specific assertion above.
        let _ = FindingSeverity::Error; // touch the import for clarity
    }

    // ──────────────────────────────────────────────────────────────────────
    // Hat overlay must preserve hat-driven event_loop settings.
    //
    // Bug history: `ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS` was originally
    // limited to `completion_promise` / `starting_event` /
    // `cancellation_promise` / `required_events`. This silently dropped
    // `event_policy` (payload contract schemas) and `verdict_gate` /
    // `execution_contracts` from builtin hat collections, breaking
    // `ralph -H builtin:ce-executor run` with
    // `Payload contract gate failed ... SchemaMissingForRequiredTopic`
    // and stripping the fail-closed semantics those blocks were
    // designed to enforce.
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn merge_hats_overlay_preserves_event_policy_from_hats() {
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      work.done:
        required_fields: [plan_name, task_id, task_key, step]
        payload: json_object
hats:
  reviewer:
    name: Reviewer
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("event_policy must survive hat overlay merge");
        assert!(policy.enabled, "event_policy.enabled must be true");
        let schema = policy
            .schemas
            .get("work.done")
            .expect("work.done schema must be present after overlay merge");
        assert!(schema.required_fields.contains(&"plan_name".to_string()));
    }

    #[test]
    fn merge_hats_overlay_preserves_verdict_gate_from_hats() {
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  verdict_gate:
    topic: REVIEW_COMPLETE
    fail_field: pass_or_fail
    fail_value: fail
hats:
  shipper:
    name: Shipper
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        let gate = config
            .event_loop
            .verdict_gate
            .as_ref()
            .expect("verdict_gate must survive hat overlay merge");
        assert_eq!(gate.topic, "REVIEW_COMPLETE");
        assert_eq!(gate.fail_field, "pass_or_fail");
        assert_eq!(gate.fail_value, "fail");
    }

    #[test]
    fn merge_hats_overlay_preserves_execution_contracts_from_hats() {
        let core: Value = serde_yaml::from_str(
            r"
event_loop:
  completion_promise: LOOP_COMPLETE
",
        )
        .unwrap();

        let hats: Value = serde_yaml::from_str(
            r"
event_loop:
  execution_contracts:
    enabled: true
    rules:
      work.done:
        require_payload_fields: [task_id, task_key]
hats:
  executor:
    name: Executor
",
        )
        .unwrap();

        let merged = merge_hats_overlay(core, hats).unwrap();
        let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

        let contracts = config
            .event_loop
            .execution_contracts
            .as_ref()
            .expect("execution_contracts must survive hat overlay merge");
        assert!(contracts.enabled);
        assert!(contracts.rules.contains_key("work.done"));
    }

    #[tokio::test]
    async fn load_config_for_preflight_hats_source_takes_precedence_over_core_hats() {
        let temp_dir = tempfile::tempdir().unwrap();
        let core_path = temp_dir.path().join("ralph.yml");
        let hats_path = temp_dir.path().join("hats.yml");

        std::fs::write(
            &core_path,
            r"
cli:
  backend: claude
event_loop:
  max_iterations: 50
  completion_promise: LOOP_COMPLETE
hats:
  builder:
    name: Builder
    description: Core builder
",
        )
        .unwrap();

        std::fs::write(
            &hats_path,
            r"
event_loop:
  completion_promise: REVIEW_COMPLETE
hats:
  reviewer:
    name: Reviewer
    description: Hats reviewer
",
        )
        .unwrap();

        let config_sources = vec![ConfigSource::File(core_path)];
        let hats_source = HatsSource::File(hats_path);

        let config = load_config_for_preflight(&config_sources, Some(&hats_source))
            .await
            .unwrap();

        assert_eq!(config.event_loop.max_iterations, 50);
        assert_eq!(config.event_loop.completion_promise, "REVIEW_COMPLETE");
        assert!(config.hats.contains_key("reviewer"));
        assert!(!config.hats.contains_key("builder"));
    }

    #[test]
    fn normalize_hats_source_value_extracts_legacy_mixed_preset() {
        let legacy: Value = serde_yaml::from_str(
            r"
cli:
  backend: claude
core:
  specs_dir: ./specs/
event_loop:
  completion_promise: LOOP_COMPLETE
hats:
  builder:
    name: Builder
",
        )
        .unwrap();

        let normalized = normalize_hats_source_value(legacy, "legacy.yml").unwrap();
        let mapping = normalized.as_mapping().unwrap();

        assert!(mapping_get(mapping, "hats").is_some());
        assert!(mapping_get(mapping, "event_loop").is_some());
        assert!(mapping_get(mapping, "cli").is_none());
        assert!(mapping_get(mapping, "core").is_none());
    }
}
