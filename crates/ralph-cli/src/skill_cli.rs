//! CLI commands for the `ralph tools skill` namespace.
//!
//! Provides subcommands for interacting with skills:
//! - `load`: Load a skill by name and output its content
//! - `list`: List available skills

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use ralph_core::{RalphConfig, SkillRegistry};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::config_resolution;
use crate::operation_guard::OperationContext;

/// Output format for skill list command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table format
    #[default]
    Table,
    /// JSON format for programmatic access
    Json,
    /// Name-only output for scripting
    Quiet,
}

/// Skill management commands.
#[derive(Parser, Debug)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub command: SkillCommands,

    /// Working directory (default: current directory)
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum SkillCommands {
    /// Load a skill by name and output its content
    Load(LoadArgs),

    /// List available skills
    List(ListArgs),
}

#[derive(Parser, Debug)]
pub struct LoadArgs {
    /// Name of the skill to load
    pub name: String,
}

/// Arguments for the `skill list` command.
#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Execute a skill command.
pub fn execute(
    args: SkillArgs,
    config_sources: &[crate::cli::shared::ConfigSource],
) -> Result<()> {
    let root = resolve_root(args.root)?;
    let ctx = OperationContext::detect(root.clone());

    match args.command {
        SkillCommands::Load(load_args) => {
            execute_load(&root, &ctx, &load_args.name, config_sources)
        }
        SkillCommands::List(list_args) => {
            execute_list(&root, &ctx, list_args, config_sources)
        }
    }
}

/// Resolve the hat id used for skill visibility. Returns `None` only
/// when the caller is a human CLI (no agent env). Agent contexts without
/// `RALPH_CURRENT_HAT` fail closed — we never silently fall back to the
/// human-visible skill set.
fn resolve_skill_hat_filter(ctx: &OperationContext) -> Result<Option<String>> {
    if !ctx.is_agent_context {
        return Ok(None);
    }
    let hat = ctx.current_hat_id.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "agent context requires RALPH_CURRENT_HAT for `ralph tools skill`; set it before invoking"
        )
    })?;
    Ok(Some(hat.to_string()))
}

fn execute_load(
    root: &Path,
    ctx: &OperationContext,
    name: &str,
    config_sources: &[crate::cli::shared::ConfigSource],
) -> Result<()> {
    let registry = build_registry(root, config_sources)?;
    let hat_filter = resolve_skill_hat_filter(ctx)?;

    // Agent load: only allow skills visible to the current hat, and
    // never reveal names of hidden skills in the "available" list.
    let visible = registry.skills_for_hat(hat_filter.as_deref());
    if let Some(skill) = visible.into_iter().find(|s| s.name == name) {
        let wrapped = format!(
            "<{name}-skill>\n{content}\n</{name}-skill>",
            name = skill.name,
            content = skill.content
        );
        print!("{wrapped}");
        return Ok(());
    }

    // Skill is not in the visible set. If it exists in the registry but
    // is hidden by hat filter, fail closed without leaking its name.
    if registry.get(name).is_some() && hat_filter.is_some() {
        bail!(
            "requested skill is not visible to the current hat; check `hats:` and `backends:` frontmatter"
        );
    }

    // Genuinely missing. Show only the visible (not hidden) skill list.
    eprintln!("Error: skill '{}' not found", name);
    let mut names: Vec<String> = registry
        .skills_for_hat(hat_filter.as_deref())
        .into_iter()
        .map(|skill| skill.name.clone())
        .collect();
    names.sort();
    if names.is_empty() {
        eprintln!("No skills available to the current caller.");
    } else {
        eprintln!("Available skills: {}", names.join(", "));
    }
    std::process::exit(1);
}

fn execute_list(
    root: &Path,
    ctx: &OperationContext,
    args: ListArgs,
    config_sources: &[crate::cli::shared::ConfigSource],
) -> Result<()> {
    let registry = build_registry(root, config_sources)?;
    let hat_filter = resolve_skill_hat_filter(ctx)?;
    let mut skills = registry.skills_for_hat(hat_filter.as_deref());
    skills.sort_by_key(|skill| skill.name.clone());

    match args.format {
        OutputFormat::Table => {
            if skills.is_empty() {
                if ctx.is_agent_context {
                    println!(
                        "No skills visible to the current hat ({}).",
                        ctx.current_hat_id.as_deref().unwrap_or("?")
                    );
                } else {
                    println!("No skills found");
                }
                return Ok(());
            }

            println!("{:<24} {:<28} {:<60}", "Name", "Source", "Description");
            println!("{}", "-".repeat(112));

            for skill in skills {
                let name = crate::display::truncate(&skill.name, 24);
                let source = format_source(skill);
                let source_truncated = crate::display::truncate(&source, 28);
                let description = if skill.description.is_empty() {
                    "(no description)".to_string()
                } else {
                    skill.description.clone()
                };
                let description_truncated = crate::display::truncate(&description, 60);

                println!(
                    "{:<24} {:<28} {:<60}",
                    name, source_truncated, description_truncated
                );
            }
        }
        OutputFormat::Json => {
            let items: Vec<SkillListItem> = skills.into_iter().map(SkillListItem::from).collect();
            println!("{}", serde_json::to_string_pretty(&items)?);
        }
        OutputFormat::Quiet => {
            for skill in skills {
                println!("{}", skill.name);
            }
        }
    }

    Ok(())
}

fn build_registry(
    root: &Path,
    config_sources: &[crate::cli::shared::ConfigSource],
) -> Result<SkillRegistry> {
    let config = load_config(root, config_sources);
    let active_backend = Some(config.cli.backend.as_str());
    SkillRegistry::from_config(&config.skills, root, active_backend)
        .context("Failed to build skill registry")
}

fn format_source(skill: &ralph_core::SkillEntry) -> String {
    match &skill.source {
        ralph_core::SkillSource::BuiltIn => "built-in".to_string(),
        ralph_core::SkillSource::File(path) => path.display().to_string(),
    }
}

#[derive(Debug, Serialize)]
struct SkillListItem {
    name: String,
    description: String,
    source: String,
    path: Option<String>,
    hats: Vec<String>,
    backends: Vec<String>,
    tags: Vec<String>,
    auto_inject: bool,
}

impl From<&ralph_core::SkillEntry> for SkillListItem {
    fn from(skill: &ralph_core::SkillEntry) -> Self {
        let (source, path) = match &skill.source {
            ralph_core::SkillSource::BuiltIn => ("built-in".to_string(), None),
            ralph_core::SkillSource::File(path) => {
                ("file".to_string(), Some(path.display().to_string()))
            }
        };

        Self {
            name: skill.name.clone(),
            description: skill.description.clone(),
            source,
            path,
            hats: skill.hats.clone(),
            backends: skill.backends.clone(),
            tags: skill.tags.clone(),
            auto_inject: skill.auto_inject,
        }
    }
}

fn resolve_root(explicit_root: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = explicit_root {
        return Ok(root);
    }

    let cwd = std::env::current_dir().context("failed to get current directory")?;
    if let Some(found) = find_workspace_root(&cwd) {
        return Ok(found);
    }

    Ok(cwd)
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if config_resolution::find_workspace_config_path(dir).is_some() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

fn find_default_skills_dir(root: &Path) -> Option<PathBuf> {
    let default_dir = root.join(".claude/skills");
    if default_dir.is_dir() {
        return Some(default_dir);
    }

    let cwd = std::env::current_dir().ok()?;
    if !cwd.starts_with(root) {
        return None;
    }

    let mut current = Some(cwd.as_path());
    while let Some(dir) = current {
        let candidate = dir.join(".claude/skills");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if dir == root {
            break;
        }
        current = dir.parent();
    }

    // Fallback: if the workspace root is nested (ralph.yml inside a subdir),
    // allow discovering a parent-level .claude/skills directory.
    let mut current = root.parent();
    while let Some(dir) = current {
        let candidate = dir.join(".claude/skills");
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = dir.parent();
    }

    None
}

fn resolve_configured_skills_dir(root: &Path, dir: &Path) -> PathBuf {
    if dir.is_absolute() {
        return dir.to_path_buf();
    }

    let candidate = root.join(dir);
    if candidate.is_dir() {
        return candidate;
    }

    let mut current = root.parent();
    while let Some(parent) = current {
        let candidate = parent.join(dir);
        if candidate.is_dir() {
            return candidate;
        }
        current = parent.parent();
    }

    candidate
}

/// Load config from workspace root, falling back to defaults.
/// Load config from workspace root, falling back to defaults.
///
/// 2026-07-13-001 plan U4 + review #C3: when the caller passes
/// `config_sources` (e.g. `-c custom.yml` from the top-level CLI),
/// the skill registry must honour the same project-config discovery
/// SSOT. With `config_sources` empty we still consult
/// `RALPH_CONFIG` (via the SSOT helper) so agents inheriting the
/// runner-injected env continue to work.
fn load_config(
    root: &Path,
    config_sources: &[crate::cli::shared::ConfigSource],
) -> RalphConfig {
    let mut merged = match config_resolution::default_core_value() {
        Ok(value) => value,
        Err(_) => return RalphConfig::default(),
    };

    if let Ok(Some((user_value, _))) = config_resolution::load_optional_user_config_value() {
        if let Ok(next) = config_resolution::merge_yaml_values(merged, user_value) {
            merged = next;
        } else {
            return RalphConfig::default();
        }
    }

    if let Some(path) = config_resolution::resolve_project_config_path(root, config_sources)
        && let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(value) =
            config_resolution::parse_yaml_value(&content, &path.display().to_string())
    {
        if let Ok(next) = config_resolution::merge_yaml_values(merged, value) {
            merged = next;
        } else {
            return RalphConfig::default();
        }
    }

    let mut config: RalphConfig = serde_yaml::from_value(merged).unwrap_or_default();

    config.normalize();

    if config.skills.dirs.is_empty() {
        if let Some(default_dir) = find_default_skills_dir(root) {
            config.skills.dirs.push(default_dir);
        }
    } else {
        config.skills.dirs = config
            .skills
            .dirs
            .iter()
            .map(|dir| resolve_configured_skills_dir(root, dir))
            .collect();
    }

    config
}
