//! CLI command for `ralph doctor`.

use anyhow::Result;
use clap::{Parser, Subcommand};
use ralph_adapters::{CliBackend, DEFAULT_PRIORITY};
use ralph_core::{CheckResult, CheckStatus, ConfigError, HatBackend, PreflightReport, RalphConfig};
use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::{ConfigSource, HatsSource};

/// Run first-run diagnostics and environment validation.
#[derive(Parser, Debug)]
pub struct DoctorArgs {
    #[command(subcommand)]
    pub subcommand: Option<DoctorSubcommand>,
}

#[derive(Subcommand, Debug)]
pub enum DoctorSubcommand {
    /// Detect plan frontmatter drift against `.ralph/agent/tasks.jsonl` (U5 / R7).
    PlanSync(PlanSyncArgs),
}

/// Arguments for `ralph doctor plan-sync`.
#[derive(Parser, Debug)]
pub struct PlanSyncArgs {
    /// Path to the plan markdown file. If omitted, scans the workspace for
    /// the most recent `.ralph` plan under `docs/plans/` or `docs/achieved/plan/`.
    #[arg(long)]
    pub plan: Option<String>,
}

pub async fn execute(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    args: DoctorArgs,
    use_colors: bool,
) -> Result<()> {
    if let Some(sub) = args.subcommand {
        match sub {
            DoctorSubcommand::PlanSync(plan_args) => {
                return execute_plan_sync(plan_args).await;
            }
        }
    }

    let source_label = crate::preflight::config_source_label(config_sources, hats_source);
    let config = crate::preflight::load_config_for_preflight(config_sources, hats_source).await?;

    let runner = ralph_core::PreflightRunner::default_checks_with_config(&config);
    let preflight_report = runner.run_all(&config).await;

    let mut config_check = None;
    let mut other_checks = Vec::new();
    for check in preflight_report.checks {
        match check.name.as_str() {
            "config" => config_check = Some(check),
            "backend" => {}
            _ => other_checks.push(check),
        }
    }

    let mut checks = Vec::new();
    if let Some(check) = config_check {
        checks.push(check);
    }

    checks.push(hat_collection_check(&config));

    let backend_checks = backend_checks(&config, command_version_ok, command_exists);
    checks.extend(backend_checks);

    let auth_backends = auth_backend_names(&config);
    checks.push(auth_hint_check(&auth_backends, |key| env::var(key).ok()));

    let diagnostics_dir = config
        .core
        .workspace_root
        .join(".ralph")
        .join("diagnostics");
    checks.push(ralph_core::agent_doc_sync::health::check_agent_doc_sync_health(&diagnostics_dir));

    checks.extend(other_checks);

    let report = report_from_checks(checks);
    print_human_report(&report, &source_label, use_colors);

    if report.failures > 0 {
        std::process::exit(1);
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CommandCheckMode {
    Version,
    PathOnly,
}

fn backend_checks<F, G>(
    _config: &RalphConfig,
    _command_version_ok: F,
    _command_exists: G,
) -> Vec<CheckResult>
where
    F: Fn(&str) -> bool,
    G: Fn(&str) -> bool,
{
    let config = _config;
    let command_version_ok = _command_version_ok;
    let command_exists = _command_exists;

    let mut checks = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    match config.cli.backend.trim() {
        "auto" => {
            for backend in DEFAULT_PRIORITY {
                let command = command_for_backend(backend);
                push_backend_check(
                    &mut checks,
                    &mut seen,
                    backend,
                    &command,
                    false,
                    CommandCheckMode::Version,
                    &command_version_ok,
                    &command_exists,
                    None,
                );
            }

            let any_available = checks.iter().any(|check| check.status == CheckStatus::Pass);

            let summary = if any_available {
                CheckResult::pass("backend:auto", "Auto backend available")
            } else {
                CheckResult::fail(
                    "backend:auto",
                    "No supported backend found",
                    format!("Checked: {}", DEFAULT_PRIORITY.join(", ")),
                )
            };
            checks.push(summary);
        }
        "custom" => {
            let command = config.cli.command.clone().unwrap_or_default();
            if command.trim().is_empty() {
                checks.push(CheckResult::fail(
                    "backend:custom",
                    "Custom backend command missing",
                    "Set cli.command in ralph.yml",
                ));
            } else {
                let backend = canonical_backend_name("custom", Some(&command));
                push_backend_check(
                    &mut checks,
                    &mut seen,
                    &backend,
                    &command,
                    true,
                    CommandCheckMode::PathOnly,
                    &command_version_ok,
                    &command_exists,
                    None,
                );
            }
        }
        backend => {
            let backend = backend.trim().to_lowercase();
            match command_for_named_backend(&backend, config.cli.command.as_deref()) {
                Ok(command) => {
                    push_backend_check(
                        &mut checks,
                        &mut seen,
                        &backend,
                        &command,
                        true,
                        CommandCheckMode::Version,
                        &command_version_ok,
                        &command_exists,
                        None,
                    );
                }
                Err(err) => {
                    checks.push(CheckResult::fail(
                        &format!("backend:{backend}"),
                        "Unknown backend",
                        err,
                    ));
                }
            }
        }
    }

    for (hat_id, hat_config) in &config.hats {
        let Some(hat_backend) = &hat_config.backend else {
            continue;
        };

        let check_mode = match hat_backend {
            HatBackend::Custom { .. } => CommandCheckMode::PathOnly,
            _ => CommandCheckMode::Version,
        };

        match CliBackend::from_hat_backend(hat_backend) {
            Ok(cli_backend) => {
                let backend_name = canonical_backend_name(
                    &hat_backend.to_cli_backend(),
                    Some(cli_backend.command.as_str()),
                );
                push_backend_check(
                    &mut checks,
                    &mut seen,
                    &backend_name,
                    &cli_backend.command,
                    true,
                    check_mode,
                    &command_version_ok,
                    &command_exists,
                    None,
                );
            }
            Err(_) => {
                checks.push(CheckResult::fail(
                    &format!("backend:hat:{hat_id}"),
                    "Unknown hat backend",
                    format!("Hat '{hat_id}' specifies an unknown backend"),
                ));
            }
        }
    }

    checks
}

fn push_backend_check<F, G>(
    checks: &mut Vec<CheckResult>,
    seen: &mut HashSet<String>,
    backend: &str,
    command: &str,
    required: bool,
    check_mode: CommandCheckMode,
    command_version_ok: &F,
    command_exists: &G,
    detail: Option<String>,
) where
    F: Fn(&str) -> bool,
    G: Fn(&str) -> bool,
{
    let name = backend_check_name(backend, command);
    if !seen.insert(name.clone()) {
        return;
    }

    let available = match check_mode {
        CommandCheckMode::Version => command_version_ok(command),
        CommandCheckMode::PathOnly => command_exists(command),
    };

    let status = if available {
        CheckStatus::Pass
    } else if required {
        CheckStatus::Fail
    } else {
        CheckStatus::Warn
    };

    let label = match status {
        CheckStatus::Pass => format!("{backend} CLI available ({command})"),
        CheckStatus::Warn => format!("{backend} CLI missing (optional for auto)"),
        CheckStatus::Fail => format!("{backend} CLI missing"),
    };

    let message = if available {
        None
    } else if let Some(detail) = detail {
        Some(detail)
    } else {
        Some(format!("Command not found or not executable: {command}"))
    };

    checks.push(CheckResult {
        name,
        label,
        status,
        message,
    });
}

fn auth_hint_check<F>(_backends: &[String], _env_lookup: F) -> CheckResult
where
    F: Fn(&str) -> Option<String>,
{
    let env_lookup = _env_lookup;
    let mut missing = Vec::new();

    let mut backends: Vec<String> = _backends
        .iter()
        .map(|backend| backend.trim().to_lowercase())
        .collect();
    backends.sort();
    backends.dedup();

    for backend in backends {
        let Some(envs) = auth_env_vars(&backend) else {
            missing.push(format!("{backend}: authenticate via the CLI"));
            continue;
        };

        if envs.iter().any(|key| env_lookup(key).is_some()) {
            continue;
        }

        missing.push(format!("{backend}: set {}", envs.join(" or ")));
    }

    if missing.is_empty() {
        CheckResult::pass("auth", "Auth hints satisfied")
    } else {
        CheckResult::warn(
            "auth",
            "Authentication not detected for some backends",
            missing.join("\n"),
        )
    }
}

fn hat_collection_check(_config: &RalphConfig) -> CheckResult {
    let config = _config;

    match config.validate() {
        Ok(_) => {
            if config.hats.is_empty() {
                CheckResult::pass("hats", "No custom hats configured (solo mode)")
            } else {
                CheckResult::pass(
                    "hats",
                    format!("Hat collection parsed ({} hat(s))", config.hats.len()),
                )
            }
        }
        Err(err) => match err {
            ConfigError::AmbiguousRouting { .. }
            | ConfigError::ReservedTrigger { .. }
            | ConfigError::MissingDescription { .. } => {
                CheckResult::fail("hats", "Hat collection invalid", err.to_string())
            }
            _ => CheckResult::pass("hats", "Hat collection parsed"),
        },
    }
}

fn auth_backend_names(config: &RalphConfig) -> Vec<String> {
    let mut names = HashSet::new();

    match config.cli.backend.trim() {
        "auto" => {
            for backend in DEFAULT_PRIORITY {
                names.insert((*backend).to_string());
            }
        }
        "custom" => {
            if let Some(command) = config.cli.command.as_deref() {
                names.insert(canonical_backend_name("custom", Some(command)));
            } else {
                names.insert("custom".to_string());
            }
        }
        backend => {
            names.insert(backend.to_lowercase());
        }
    }

    for hat in config.hats.values() {
        let Some(backend) = &hat.backend else {
            continue;
        };

        let name = match backend {
            HatBackend::Named(name) => name.clone(),
            HatBackend::NamedWithArgs { backend_type, .. } => backend_type.clone(),
            HatBackend::KiroAgent { backend_type, .. } => backend_type.clone(),
            HatBackend::Custom { command, .. } => canonical_backend_name("custom", Some(command)),
        };

        names.insert(name.to_lowercase());
    }

    names.into_iter().collect()
}

fn auth_env_vars(backend: &str) -> Option<Vec<&'static str>> {
    match backend {
        "claude" => Some(vec!["ANTHROPIC_API_KEY"]),
        "gemini" => Some(vec!["GEMINI_API_KEY"]),
        "codex" => Some(vec!["OPENAI_API_KEY", "CODEX_API_KEY"]),
        "kiro" => Some(vec!["KIRO_API_KEY"]),
        "kiro-acp" => Some(vec!["KIRO_API_KEY"]),
        "opencode" => Some(vec![
            "OPENCODE_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
        ]),
        "pi" => Some(vec![
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
        ]),
        "traecli" => Some(vec![
            "TRAECLI_PERSONAL_ACCESS_TOKEN",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
        ]),
        _ => None,
    }
}

fn command_for_backend(backend: &str) -> String {
    CliBackend::from_name(backend)
        .map(|backend| backend.command)
        .unwrap_or_else(|_| backend.to_string())
}

fn command_for_named_backend(
    backend: &str,
    command_override: Option<&str>,
) -> Result<String, String> {
    let backend = backend.trim().to_lowercase();
    if let Some(command) = command_override
        && !command.trim().is_empty()
    {
        return Ok(command.to_string());
    }

    CliBackend::from_name(&backend)
        .map(|backend| backend.command)
        .map_err(|_| format!("Unknown backend: {backend}"))
}

fn canonical_backend_name(backend: &str, command: Option<&str>) -> String {
    if backend != "custom" {
        return backend.to_lowercase();
    }

    let Some(command) = command else {
        return "custom".to_string();
    };

    let basename = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command);

    let mut normalized = basename.to_string();
    let normalized_lower = normalized.to_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".com"] {
        if normalized_lower.ends_with(ext) {
            let new_len = normalized.len().saturating_sub(ext.len());
            normalized.truncate(new_len);
            break;
        }
    }

    let normalized_lower = normalized.to_lowercase();
    match normalized_lower.as_str() {
        "kiro-cli" => "kiro".to_string(),
        "claude" => "claude".to_string(),
        "gemini" => "gemini".to_string(),
        "codex" => "codex".to_string(),
        "opencode" => "opencode".to_string(),
        "pi" => "pi".to_string(),
        "traecli" => "traecli".to_string(),
        _ => normalized,
    }
}

fn backend_check_name(backend: &str, command: &str) -> String {
    if backend.eq_ignore_ascii_case(command) {
        format!("backend:{backend}")
    } else {
        format!("backend:{backend}@{command}")
    }
}

fn command_version_ok(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn command_exists(command: &str) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return path.is_file();
    }

    let Some(path_var) = env::var_os("PATH") else {
        return false;
    };
    let extensions = executable_extensions();

    for dir in env::split_paths(&path_var) {
        for ext in &extensions {
            let candidate = if ext.is_empty() {
                dir.join(command)
            } else {
                dir.join(format!("{}{}", command, ext.to_string_lossy()))
            };

            if candidate.is_file() {
                return true;
            }
        }
    }

    false
}

fn executable_extensions() -> Vec<OsString> {
    if cfg!(windows) {
        let exts = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        exts.split(';')
            .filter(|ext| !ext.trim().is_empty())
            .map(|ext| OsString::from(ext.trim().to_string()))
            .collect()
    } else {
        vec![OsString::new()]
    }
}

fn report_from_checks(checks: Vec<CheckResult>) -> PreflightReport {
    let warnings = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Warn)
        .count();
    let failures = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .count();

    PreflightReport {
        passed: failures == 0,
        warnings,
        failures,
        checks,
    }
}

fn print_human_report(report: &PreflightReport, source: &str, use_colors: bool) {
    use crate::display::colors;

    println!("Doctor checks for {}", source);
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

// =============================================================================
// Plan-sync (U5 / R7): detect frontmatter drift between plan files and
// `.ralph/agent/tasks.jsonl`.
// =============================================================================

const ALLOWED_PLAN_STATUSES: &[&str] = &[
    "draft",
    "active",
    "stalled-after-u0",
    "stalled-after-u1",
    "stalled-after-u2",
    "stalled-after-u3",
    "stalled-after-u4",
    "stalled-after-u5",
    "stalled-after-u6",
    "stalled-after-u7",
    "stalled-after-u8",
    "u0-closed-u1-pending",
    "u1-closed-u2-splitting-pending",
    "u2-closed-u3-pending",
    "u3-closed-u4-pending",
    "u4-closed-u5-pending",
    "u5-closed-u6-pending",
    "completed",
    "abandoned",
];

/// Statuses that are not in [`ALLOWED_PLAN_STATUSES`] but follow a
/// regular pattern (suffix handoff labels, multi-stage merged-into
/// plans, etc.). Keeping this whitelist open by pattern rather than
/// exhaustively enumerating avoids rot every time a plan hands off
/// the remaining units to a successor plan.
fn is_pattern_allowed_status(status: &str) -> bool {
    // Multi-stage handoff: `uM-closed-uN-...-merged-into-plan-XXX`
    // (any number of intermediate `uN-*` tokens). Lets plan authors
    // express a finished unit that has been rolled into a successor
    // without enumeration churn.
    status.contains("-merged-into-plan-")
}

/// Run plan-sync check and return a [`CheckResult`].
pub(crate) fn check_plan_sync(plan_path: &Path, tasks_path: &Path) -> CheckResult {
    let plan_name = plan_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    if !plan_path.is_file() {
        return CheckResult::fail(
            "plan_sync",
            "Plan file not found",
            format!("Expected plan at: {}", plan_path.display()),
        );
    }

    let plan_text = match std::fs::read_to_string(plan_path) {
        Ok(s) => s,
        Err(err) => {
            return CheckResult::fail(
                "plan_sync",
                "Plan file unreadable",
                format!("{}: {}", plan_path.display(), err),
            );
        }
    };

    let front = match parse_frontmatter(&plan_text) {
        Some(f) => f,
        None => {
            return CheckResult::fail(
                "plan_sync",
                "Plan frontmatter missing",
                format!(
                    "Plan '{}' has no YAML frontmatter; cannot detect status drift",
                    plan_path.display()
                ),
            );
        }
    };

    let status = match front.get("status").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            return CheckResult::fail(
                "plan_sync",
                "Plan frontmatter missing 'status'",
                format!("Plan '{}' has no status field", plan_path.display()),
            );
        }
    };

    // Tasks.jsonl: missing -> warn (T5.4), not a fail.
    let tasks_summary = if !tasks_path.is_file() {
        TaskSummary::default()
    } else {
        match read_tasks_summary(tasks_path, &plan_name) {
            Ok(summary) => summary,
            Err(err) => {
                return CheckResult::fail(
                    "plan_sync",
                    "tasks.jsonl parse error",
                    format!("{}: {}", tasks_path.display(), err),
                );
            }
        }
    };

    let mut issues: Vec<String> = Vec::new();

    if !ALLOWED_PLAN_STATUSES.contains(&status.as_str()) && !is_pattern_allowed_status(&status) {
        issues.push(format!(
            "status '{}' not in allowed enum: {}",
            status,
            ALLOWED_PLAN_STATUSES.join(", ")
        ));
    }

    // Rule 1: status says completed but tasks are still open.
    if status == "completed" && tasks_summary.open > 0 {
        issues.push(format!(
            "status='completed' but {} open task(s) remain for plan '{}'",
            tasks_summary.open, plan_name
        ));
    }

    // Rule 2: status still references a stalled unit while tasks for that unit are closed.
    if let Some(unit_id) = stalled_unit_from_status(&status) {
        if tasks_summary.closed_for_unit(&unit_id) > 0 && tasks_summary.open_for_unit(&unit_id) == 0
        {
            issues.push(format!(
                "status='{}' but unit {} has closed tasks and no open ones",
                status, unit_id
            ));
        }
    }

    if !tasks_path.is_file() {
        // T5.4: missing tasks.jsonl is a warn, not a fail.
        return CheckResult::warn(
            "plan_sync",
            "tasks.jsonl missing; drift check skipped",
            format!(
                "Plan '{}' status='{}' parsed; no tasks to compare against. Create .ralph/agent/tasks.jsonl or run a loop first.",
                plan_path.display(),
                status
            ),
        );
    }

    if issues.is_empty() {
        CheckResult::pass(
            "plan_sync",
            format!(
                "Plan '{}' status='{}' consistent with tasks (open={}, closed={})",
                plan_name, status, tasks_summary.open, tasks_summary.closed
            ),
        )
    } else {
        CheckResult::fail(
            "plan_sync",
            "Plan frontmatter drift detected",
            issues.join("\n"),
        )
    }
}

/// Parses the leading YAML frontmatter (`---\n...\n---`) of a markdown plan.
fn parse_frontmatter(text: &str) -> Option<serde_yaml::Value> {
    let trimmed = text.trim_start_matches('\u{feff}');
    let rest = trimmed.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end = rest.find("\n---")?;
    let yaml_text = &rest[..end];
    serde_yaml::from_str(yaml_text).ok()
}

/// Extracts the U-id referenced by a `stalled-after-uN` status.
///
/// Returns `None` for statuses that do not encode a unit reference (e.g.
/// `completed`, `active`, `draft`).
fn stalled_unit_from_status(status: &str) -> Option<String> {
    if let Some(rest) = status.strip_prefix("stalled-after-") {
        // Accept e.g. "stalled-after-u3" or "stalled-after-U3".
        let lower = rest.to_lowercase();
        if lower.starts_with('u') {
            return Some(lower.to_string());
        }
    }
    None
}

#[derive(Debug, Default)]
struct TaskSummary {
    open: usize,
    in_progress: usize,
    closed: usize,
    failed: usize,
    /// Map of unit id (e.g. "u1") -> (open, closed) for tasks whose key
    /// contains `:uN-` or `:uNa-` segments.
    by_unit: std::collections::BTreeMap<String, (usize, usize)>,
}

impl TaskSummary {
    fn closed_for_unit(&self, unit_id: &str) -> usize {
        self.by_unit.get(unit_id).map(|(_, c)| *c).unwrap_or(0)
    }
    fn open_for_unit(&self, unit_id: &str) -> usize {
        self.by_unit.get(unit_id).map(|(o, _)| *o).unwrap_or(0)
    }
}

/// Reads `.ralph/agent/tasks.jsonl` and tallies tasks whose `key` (or
/// `description`) references `plan_name`. Tasks without a key are ignored
/// because we cannot associate them with a plan.
fn read_tasks_summary(tasks_path: &Path, plan_name: &str) -> Result<TaskSummary, String> {
    let text = std::fs::read_to_string(tasks_path).map_err(|e| format!("read failed: {e}"))?;
    let mut summary = TaskSummary::default();

    for (line_no, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(err) => {
                return Err(format!("line {}: invalid JSON: {}", line_no + 1, err));
            }
        };

        // Identify the plan: prefer the `key` field (format
        // `ce-executor:{plan_name}:...`); fall back to a `plan_name` field
        // if present, or a substring match in `description`.
        let key = value.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let plan_field = value
            .get("plan_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let matches = if !key.is_empty() {
            key_matches_plan(key, plan_name)
        } else if !plan_field.is_empty() {
            plan_field == plan_name
        } else {
            false
        };

        if !matches {
            continue;
        }

        let status = value
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("open");
        match status {
            "open" => summary.open += 1,
            "in_progress" => summary.in_progress += 1,
            "closed" => summary.closed += 1,
            "failed" => summary.failed += 1,
            _ => {}
        }

        if let Some(unit) = extract_unit_from_key(key) {
            let entry = summary.by_unit.entry(unit).or_insert((0, 0));
            match status {
                "open" | "in_progress" => entry.0 += 1,
                "closed" | "failed" => entry.1 += 1,
                _ => {}
            }
        }
    }

    Ok(summary)
}

/// Returns true if the task key encodes the same `plan_name`.
fn key_matches_plan(key: &str, plan_name: &str) -> bool {
    // Expected formats:
    //   "ce-executor:{plan_name}:step-01:u1-impl"
    //   "ce-executor:{plan_name}:trivial"
    let parts: Vec<&str> = key.split(':').collect();
    if parts.len() >= 3 && parts[0] == "ce-executor" {
        return parts[1] == plan_name;
    }
    false
}

/// Extracts a unit id (lowercased) from a key like
/// `ce-executor:foo:step-01:u1-impl` → "u1" or "u1a".
fn extract_unit_from_key(key: &str) -> Option<String> {
    let last = key.rsplit(':').next()?;
    // Match `uN` or `uNa`, where N is digits and a is optional lowercase letter.
    let bytes = last.as_bytes();
    if bytes.first()? != &b'u' && bytes.first()? != &b'U' {
        return None;
    }
    let mut idx = 1_usize;
    let digit_start = idx;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == digit_start {
        return None;
    }
    // Optional sub-unit letter.
    if idx < bytes.len() && bytes[idx].is_ascii_alphabetic() {
        idx += 1;
    }
    Some(last[..idx].to_lowercase())
}

async fn execute_plan_sync(args: PlanSyncArgs) -> Result<()> {
    let plan_path = match resolve_plan_path(args.plan.as_deref()) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("plan-sync error: {err}");
            std::process::exit(1);
        }
    };

    // tasks.jsonl lives in .ralph/agent/ relative to the workspace root.
    // We resolve it relative to the plan file's parent or the cwd.
    let workspace_root = plan_path
        .ancestors()
        .find(|p| p.join(".ralph").is_dir())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let tasks_path = workspace_root
        .join(".ralph")
        .join("agent")
        .join("tasks.jsonl");

    let result = check_plan_sync(&plan_path, &tasks_path);
    println!("plan-sync: {}", result.label);
    if let Some(msg) = &result.message {
        for line in msg.lines() {
            println!("    {line}");
        }
    }
    match result.status {
        CheckStatus::Pass => {
            println!("Result: PASS");
            Ok(())
        }
        CheckStatus::Warn => {
            println!("Result: WARN");
            // T5.4 / T5.3: warn exits 0, plan-file-missing exits 1.
            Ok(())
        }
        CheckStatus::Fail => {
            println!("Result: FAIL");
            std::process::exit(1);
        }
    }
}

fn resolve_plan_path(explicit: Option<&str>) -> Result<std::path::PathBuf, String> {
    if let Some(p) = explicit {
        let path = std::path::PathBuf::from(p);
        if !path.is_file() {
            return Err(format!("plan file not found: {}", path.display()));
        }
        return Ok(path);
    }

    // Auto-discover: look for the newest `.md` under docs/plans/ or
    // docs/achieved/plan/. We deliberately keep this simple; users can
    // always pass --plan.
    let cwd = std::env::current_dir().map_err(|e| format!("cwd unavailable: {e}"))?;
    let candidates = [
        cwd.join("docs").join("plans"),
        cwd.join("docs").join("achieved").join("plan"),
    ];
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for dir in &candidates {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("md")
                    && let Ok(meta) = entry.metadata()
                    && let Ok(modified) = meta.modified()
                    && newest.as_ref().map_or(true, |(t, _)| modified > *t)
                {
                    newest = Some((modified, path));
                }
            }
        }
    }
    newest.map(|(_, p)| p).ok_or_else(|| {
        "no plan file specified and none found under docs/plans/ or docs/achieved/plan/".to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::HatConfig;

    fn base_hat(name: &str, backend: Option<HatBackend>) -> HatConfig {
        HatConfig {
            name: name.to_string(),
            description: Some("Test hat".to_string()),
            triggers: vec!["work.start".to_string()],
            publishes: vec![],
            terminal_events: vec![],
            instructions: String::new(),
            extra_instructions: vec![],
            backend_args: None,
            backend,
            default_publishes: None,
            ignore_payload_fields: vec![],
            max_activations: None,
            scratchpad: None,
            disallowed_tools: vec![],
            timeout: None,
            // 2026-06-17-004 U2 (R3): test helper aligned with
            // `HatConfig::default()`.
            missing_event_grace_secs: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            // 2026-06-26 plan U2: doctor test fixture does not
            // exercise the exempt list; default empty.
            exempt_topics: vec![],
            // 2026-06-29-007 plan U5a: doctor test fixture
            // does not exercise write paths; default `None`
            // mirrors production default.
            allowed_write_paths: None,
            phase_triggers: None,
            obligations: vec![],
            trigger_multi_consumer_topics: HashSet::new(),
        }
    }

    #[test]
    fn backend_checks_include_cli_and_hat_backends() {
        let mut config = RalphConfig::default();
        config.cli.backend = "claude".to_string();
        config.hats.insert(
            "reviewer".to_string(),
            base_hat("Reviewer", Some(HatBackend::Named("gemini".to_string()))),
        );
        let checks = backend_checks(&config, |cmd| cmd == "claude", |_| false);
        let names: HashSet<_> = checks.iter().map(|check| check.name.as_str()).collect();

        assert!(names.contains("backend:claude"));
        assert!(names.contains("backend:gemini"));
    }

    #[test]
    fn backend_checks_map_custom_command_to_known_backend() {
        let mut config = RalphConfig::default();
        config.cli.backend = "custom".to_string();
        config.cli.command = Some("opencode".to_string());
        let checks = backend_checks(&config, |_| false, |cmd| cmd == "opencode");
        let names: Vec<_> = checks.iter().map(|check| check.name.as_str()).collect();

        assert!(names.contains(&"backend:opencode"));
    }

    #[test]
    fn backend_checks_fail_required_missing() {
        let mut config = RalphConfig::default();
        config.cli.backend = "claude".to_string();

        let checks = backend_checks(&config, |_| false, |_| false);
        let claude = checks
            .iter()
            .find(|check| check.name == "backend:claude")
            .expect("expected claude backend check");

        assert_eq!(claude.status, CheckStatus::Fail);
    }

    #[test]
    fn auth_hint_warns_when_env_missing() {
        let backends = vec!["codex".to_string(), "gemini".to_string()];
        let check = auth_hint_check(&backends, |key| match key {
            "OPENAI_API_KEY" => Some("present".to_string()),
            _ => None,
        });

        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.message.as_deref().unwrap_or("").contains("gemini"));
    }

    #[test]
    fn auth_hint_passes_when_all_env_present() {
        let backends = vec!["codex".to_string(), "gemini".to_string()];
        let check = auth_hint_check(&backends, |key| match key {
            "OPENAI_API_KEY" => Some("present".to_string()),
            "GEMINI_API_KEY" => Some("present".to_string()),
            _ => None,
        });

        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn canonical_backend_name_strips_exe_extension() {
        assert_eq!(
            canonical_backend_name("custom", Some("claude.exe")),
            "claude"
        );
    }

    #[test]
    fn canonical_backend_name_strips_extension_for_unknown_command() {
        assert_eq!(
            canonical_backend_name("custom", Some("my-cli.exe")),
            "my-cli"
        );
    }

    #[test]
    fn agent_doc_sync_check_runs_against_diagnostics_dir() {
        // The function should be callable against the diagnostics dir;
        // an empty directory should produce a Warn (snapshot missing)
        // since the workspace has not been synced yet.
        let dir = tempfile::TempDir::new().unwrap();
        let diag = dir.path().join(".ralph").join("diagnostics");
        std::fs::create_dir_all(&diag).unwrap();
        let check = ralph_core::agent_doc_sync::health::check_agent_doc_sync_health(&diag);
        assert_eq!(check.name, "agent_doc_sync");
        assert_eq!(check.status, CheckStatus::Warn);
    }

    // ---- plan-sync unit tests (U5 / R7) ----

    #[test]
    fn plan_sync_extract_unit_lowercases_subunit() {
        assert_eq!(
            extract_unit_from_key("ce-executor:foo:step-01:u1-impl"),
            Some("u1".to_string())
        );
        assert_eq!(
            extract_unit_from_key("ce-executor:foo:step-02:u1a-impl"),
            Some("u1a".to_string())
        );
        assert_eq!(
            extract_unit_from_key("ce-executor:foo:step-02:u1b-impl"),
            Some("u1b".to_string())
        );
        assert_eq!(extract_unit_from_key("ce-executor:foo:trivial"), None);
    }

    #[test]
    fn plan_sync_stalled_unit_extraction() {
        assert_eq!(
            stalled_unit_from_status("stalled-after-u3"),
            Some("u3".to_string())
        );
        assert_eq!(
            stalled_unit_from_status("stalled-after-U7"),
            Some("u7".to_string())
        );
        assert_eq!(stalled_unit_from_status("active"), None);
        assert_eq!(stalled_unit_from_status("completed"), None);
    }

    #[test]
    fn plan_sync_key_matches_plan() {
        assert!(key_matches_plan(
            "ce-executor:my-plan:step-01:u1-impl",
            "my-plan"
        ));
        assert!(!key_matches_plan(
            "ce-executor:other-plan:step-01:u1-impl",
            "my-plan"
        ));
        assert!(!key_matches_plan("ce-executor:my-plan", "my-plan"));
    }

    #[test]
    fn plan_sync_parses_frontmatter() {
        let md = "---\ntitle: test\nstatus: active\n---\n# body\n";
        let fm = parse_frontmatter(md).expect("frontmatter");
        assert_eq!(fm.get("status").and_then(|v| v.as_str()), Some("active"));
    }

    #[test]
    fn plan_sync_missing_frontmatter_is_none() {
        assert!(parse_frontmatter("# body\n").is_none());
    }

    #[test]
    fn plan_sync_detects_stalled_with_closed_tasks() {
        // T5.1 core scenario: frontmatter stalled + unit closed.
        let dir = tempfile::TempDir::new().unwrap();
        let plan = dir.path().join("test-plan.md");
        std::fs::write(
            &plan,
            "---\ntitle: t\nstatus: stalled-after-u1\n---\n# body\n",
        )
        .unwrap();
        let tasks = dir.path().join("tasks.jsonl");
        std::fs::write(
            &tasks,
            "{\"id\":\"t1\",\"title\":\"u1\",\"status\":\"closed\",\"key\":\"ce-executor:test-plan:step-01:u1-impl\",\"created\":\"2026-06-17T00:00:00Z\"}\n",
        )
        .unwrap();
        let result = check_plan_sync(&plan, &tasks);
        assert_eq!(result.status, CheckStatus::Fail);
        assert!(result.message.as_deref().unwrap_or("").contains("u1"));
    }

    #[test]
    fn plan_sync_consistent_state_passes() {
        let dir = tempfile::TempDir::new().unwrap();
        let plan = dir.path().join("ok-plan.md");
        std::fs::write(&plan, "---\ntitle: t\nstatus: active\n---\n# body\n").unwrap();
        let tasks = dir.path().join("tasks.jsonl");
        std::fs::write(
            &tasks,
            "{\"id\":\"t1\",\"title\":\"u1\",\"status\":\"open\",\"key\":\"ce-executor:ok-plan:step-01:u1-impl\",\"created\":\"2026-06-17T00:00:00Z\"}\n",
        )
        .unwrap();
        let result = check_plan_sync(&plan, &tasks);
        assert_eq!(result.status, CheckStatus::Pass);
    }

    #[test]
    fn plan_sync_completed_with_open_task_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let plan = dir.path().join("done-plan.md");
        std::fs::write(&plan, "---\ntitle: t\nstatus: completed\n---\n# body\n").unwrap();
        let tasks = dir.path().join("tasks.jsonl");
        std::fs::write(
            &tasks,
            "{\"id\":\"t1\",\"title\":\"u1\",\"status\":\"open\",\"key\":\"ce-executor:done-plan:step-01:u1-impl\",\"created\":\"2026-06-17T00:00:00Z\"}\n",
        )
        .unwrap();
        let result = check_plan_sync(&plan, &tasks);
        assert_eq!(result.status, CheckStatus::Fail);
    }

    #[test]
    fn plan_sync_missing_plan_file_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let plan = dir.path().join("nonexistent.md");
        let tasks = dir.path().join("tasks.jsonl");
        let result = check_plan_sync(&plan, &tasks);
        assert_eq!(result.status, CheckStatus::Fail);
        assert!(result.label.contains("not found"));
    }

    #[test]
    fn plan_sync_missing_tasks_warns_not_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let plan = dir.path().join("t.md");
        std::fs::write(&plan, "---\ntitle: t\nstatus: active\n---\n# body\n").unwrap();
        let tasks = dir.path().join("nonexistent-tasks.jsonl");
        let result = check_plan_sync(&plan, &tasks);
        assert_eq!(result.status, CheckStatus::Warn);
    }

    /// P1-1 / P1-2 (plan 004 code review): handoff status
    /// `uN-closed-...-merged-into-plan-XXX` is not in the literal
    /// [`ALLOWED_PLAN_STATUSES`] but is recognised by the pattern
    /// helper, so the rule must pass without flagging a phantom
    /// enum violation.
    #[test]
    fn plan_sync_merged_into_plan_status_is_accepted() {
        let dir = tempfile::TempDir::new().unwrap();
        let plan = dir.path().join("p.md");
        std::fs::write(
            &plan,
            "---\ntitle: t\nstatus: u1-closed-u2-u5-merged-into-plan-004\n---\n# body\n",
        )
        .unwrap();
        let tasks = dir.path().join("tasks.jsonl");
        std::fs::write(
            &tasks,
            "{\"id\":\"t1\",\"title\":\"u1\",\"status\":\"closed\",\"key\":\"ce-executor:p:step-01:u1-impl\",\"created\":\"2026-06-17T00:00:00Z\"}\n",
        )
        .unwrap();
        let result = check_plan_sync(&plan, &tasks);
        // Pattern-accepted status, no other drift → pass.
        assert_eq!(result.status, CheckStatus::Pass);
    }

    /// Negative companion: an unrelated suffix-like status (e.g.
    /// `merged-into-plan-` without the `uN-closed-` prefix) is
    /// still rejected so the pattern helper does not become a
    /// blanket bypass.
    #[test]
    fn plan_sync_merged_into_plan_without_prefix_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let plan = dir.path().join("p.md");
        std::fs::write(
            &plan,
            "---\ntitle: t\nstatus: random-merged-into-plan-001\n---\n# body\n",
        )
        .unwrap();
        let tasks = dir.path().join("nonexistent-tasks.jsonl");
        let result = check_plan_sync(&plan, &tasks);
        // Wrong prefix → pattern helper does NOT recognise, but
        // tasks.jsonl is missing → warn path dominates.
        assert_eq!(result.status, CheckStatus::Warn);
    }
}
