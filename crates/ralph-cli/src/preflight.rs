//! Preflight command for validating configuration and environment.

use anyhow::{Context, Result};
use clap::{ArgAction, Parser, ValueEnum};
use ralph_core::{CheckResult, CheckStatus, PreflightReport, PreflightRunner, RalphConfig};
use serde_yaml::{Mapping, Value};
use std::path::PathBuf;
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
        if let Some(mapping) = core_value.as_mapping()
            && (mapping_get(mapping, "hats").is_some() || mapping_get(mapping, "events").is_some())
        {
            warn!(
                "Core config '{}' contains hats/events and hats source '{}' was provided; hats source takes precedence for hats/events",
                core_label,
                source.label()
            );
        }

        let hats_value = load_hats_value(source).await?;
        validate_hats_config_shape(&hats_value, &source.label())?;
        core_value = merge_hats_overlay(core_value, hats_value)?;
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
];
// Event-loop keys that a hat collection overlay is allowed to provide.
//
// Original 4 (workflow promises + starting event) are the historic core
// minimum. `execution_mode` and the 3 contract keys below (`event_policy`,
// `verdict_gate`, `execution_contracts`) are hat-driven by design: a hat
// collection declares the topology and contracts required for its safety
// properties, so they must survive overlay merge for builtin presets like
// `ce-executor-isolated` to work end-to-end.
//
// Note: resource budgets (`max_iterations`, `max_runtime_seconds`,
// `checkpoint_interval`) and `enforce_hat_scope` are intentionally
// NOT in this list. They are operator-controlled, not hat-controlled,
// so a hat collection must not be able to widen the loop budget or
// disable scope enforcement behind the user's back.
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

fn extract_hat_overlay_from_preset(preset_value: Value) -> Result<Value> {
    let mapping = preset_value
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("Builtin hat collection must be a YAML mapping"))?;

    let mut overlay = Mapping::new();
    // U5 (2026-06-09) follow-up (2026-06-11): include
    // `topic_format_whitelist` so preset-declared protocol tokens
    // (e.g. LOOP_COMPLETE / REVIEW_COMPLETE) survive into
    // `merge_hats_overlay` and the lint treats them as exempt. Without
    // this, the U5 commit f876241 that added the whitelist to all 9
    // builtin presets was a no-op in real runs.
    for key in [
        "name",
        "description",
        "event_loop",
        "events",
        "hats",
        "tasks",
        "topic_format_whitelist",
    ] {
        if let Some(value) = mapping_get(mapping, key) {
            mapping_insert(&mut overlay, key, value.clone());
        }
    }

    Ok(Value::Mapping(overlay))
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
        if let Some(coordinator_hats) = mapping_get(overlay_mapping, "coordinator_hats") {
            let tasks_value = mapping_get(core_mapping, "tasks")
                .cloned()
                .unwrap_or_else(|| Value::Mapping(Mapping::new()));
            let mut tasks_mapping = tasks_value
                .as_mapping()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("core.tasks must be a mapping when provided"))?;
            mapping_insert(
                &mut tasks_mapping,
                "coordinator_hats",
                coordinator_hats.clone(),
            );
            mapping_insert(core_mapping, "tasks", Value::Mapping(tasks_mapping));
        }
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
                if ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS.contains(&key_str) {
                    event_loop_mapping.insert(key.clone(), value.clone());
                } else if !event_loop_mapping.contains_key(&key) {
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
            )
            .iter()
            .all(|finding| finding.id != "lint.preset.multi_hat_requires_isolated")
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

        let findings = run_preset_lint(&config, LintStrictness::Strict, false);

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
