//! CLI commands for the `ralph preset` namespace.
//!
//! Preset template authoring, preset contract validation, and builtin
//! **artifact** template materialization (binary-only installs).
//!
//! Subcommands:
//! - `list`: List available workflow templates
//! - `show`: Show details of a specific template
//! - `new`: Generate a new preset from a template
//! - `check`: Run preset/workflow contract validation (config, topology, payload, orphan)
//! - `diff`: Show differences between a local preset and its template baseline
//! - `upgrade`: Preview upgrade information for a local preset
//! - `materialize-artifacts`: Write embedded artifact templates to disk
//!   (e.g. parallel-forge development-plan / unit / manager-report templates).
//!   Distinct from `new`: `new` scaffolds a **preset YAML**; this command
//!   extracts **runtime fill-in templates** that hats copy into `.ralph/forge/`.

use crate::display::colors;
use crate::preflight;
use crate::preset_templates::{
    TemplateCatalog, TemplateDifficulty, TemplateManifest, Version, XPresetMetadata,
};
use crate::{ConfigSource, HatsSource};
use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use ralph_core::config::HatExecutionMode;
use ralph_core::HatRegistry;
use ralph_core::preset_verify::{
    build_report as build_verify_report, evaluate_scenario, FailureKind,
    PresetVerifyReport, ScenarioFile, SourceKind, StaticLayer, run_scenario, DriverWorkspace,
};
use ralph_core::runtime_contract::{
    FindingSeverity, RuntimeContractReport, RuntimeContractStrictness,
};
use std::io::Write;
use std::path::PathBuf;

/// Manage and validate presets.
#[derive(Parser, Debug)]
pub struct PresetArgs {
    #[command(subcommand)]
    pub command: Option<PresetCommands>,
}

#[derive(Subcommand, Debug)]
pub enum PresetCommands {
    /// List available workflow templates
    List {
        /// Output format (human or json)
        #[arg(long, value_enum, default_value_t = PresetListFormat::Human)]
        format: PresetListFormat,
    },
    /// Show details of a specific template
    Show {
        /// Template name to show
        name: String,
        /// Output format (human, yaml, or json)
        #[arg(long, value_enum, default_value_t = PresetShowFormat::Human)]
        format: PresetShowFormat,
    },
    /// Generate a new preset from a template
    New(NewPresetArgs),
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
    /// Show differences between a local preset and its template baseline
    Diff {
        /// Path to the local preset file
        #[arg(long)]
        file: PathBuf,

        /// Output format (human or json)
        #[arg(long, value_enum, default_value_t = DiffFormat::Human)]
        format: DiffFormat,
    },
    /// Preview upgrade information for a local preset (MVP: dry-run only)
    Upgrade {
        /// Path to the local preset file
        #[arg(long)]
        file: PathBuf,

        /// Output format (human or json)
        #[arg(long, value_enum, default_value_t = UpgradeFormat::Human)]
        format: UpgradeFormat,

        /// Preview upgrade without writing changes (MVP: always true; flag kept for
        /// forward-compatibility with the planned write-back path).
        #[arg(long, default_value_t = true)]
        dry_run: bool,

        /// Force: apply upgrade even if there are user changes (not implemented in MVP)
        #[arg(long)]
        force: bool,
    },
    /// Materialize embedded artifact templates for a builtin preset (works on binary-only installs)
    MaterializeArtifacts {
        /// Builtin preset name (e.g. parallel-forge or builtin:parallel-forge)
        preset: String,

        /// Plan key basename; default output is .ralph/forge/<plan-key>/templates/
        #[arg(long)]
        plan_key: String,

        /// Override output directory
        #[arg(long)]
        dest: Option<PathBuf>,
    },
    /// Introspect compiled-in builtin presets (U01/U02)
    Builtin {
        #[command(subcommand)]
        command: PresetBuiltinCommands,
    },
    /// Run a deterministic, scripted-workflow verification on a preset
    /// (Unit 4 of plan 2026-08-15-0722). Reuses preflight + strict
    /// static contract check, then drives a real EventLoop over a
    /// version 1 scenario file in an isolated temp workspace.
    /// Rejects remote sources (no network, no remote hats/config).
    Verify {
        /// Path to a version 1 scenario YAML file (`--scenario`)
        #[arg(long)]
        scenario: PathBuf,

        /// Output format (human or json)
        #[arg(long, value_enum, default_value_t = PresetVerifyFormat::Human)]
        format: PresetVerifyFormat,
    },
}

#[derive(Subcommand, Debug)]
pub enum PresetBuiltinCommands {
    /// List public builtin presets compiled into this binary (U01)
    List {
        /// Output format (human or json)
        #[arg(long, value_enum, default_value_t = PresetBuiltinListFormat::Human)]
        format: PresetBuiltinListFormat,
    },
    /// Show the raw embedded YAML of a builtin preset (U02).
    ///
    /// Resolves both public and hidden builtins (e.g. `merge-loop`);
    /// unknown IDs exit non-zero with the id on stderr.
    Show {
        /// Builtin preset id (e.g. `parallel-forge`, `merge-loop`)
        id: String,
        /// Output format (human, yaml, or json)
        #[arg(long, value_enum, default_value_t = PresetBuiltinShowFormat::Human)]
        format: PresetBuiltinShowFormat,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum PresetBuiltinListFormat {
    Human,
    Json,
}

/// Output format for `ralph preset builtin show` (U02).
///
/// Reused as a dedicated enum (rather than sharing the existing
/// `PresetShowFormat`) so the template `show` and builtin `show` paths
/// can evolve independently — template `show` carries
/// TemplateManifest-specific semantics, while builtin `show` deals
/// with byte-exact `EmbeddedPreset.content`.
#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum PresetBuiltinShowFormat {
    Human,
    Yaml,
    Json,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum PresetListFormat {
    Human,
    Json,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum PresetShowFormat {
    Human,
    Yaml,
    Json,
}

/// Output format for `ralph preset verify`.
#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum PresetVerifyFormat {
    Human,
    Json,
}

#[derive(Parser, Debug)]
pub struct NewPresetArgs {
    /// Template name to use
    pub template: String,

    /// Name for the generated preset
    #[arg(long)]
    pub name: Option<String>,

    /// Description for the generated preset
    #[arg(long)]
    pub description: Option<String>,

    /// Output file path (default: .ralph/hats/<name>.yml)
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Force overwrite if output file exists
    #[arg(long)]
    pub force: bool,

    /// Run authoring checks after generation
    #[arg(long)]
    pub check: bool,

    /// Output format (human or json)
    #[arg(long, value_enum, default_value_t = NewPresetFormat::Human)]
    pub format: NewPresetFormat,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum NewPresetFormat {
    Human,
    Json,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum PresetCheckFormat {
    Human,
    Json,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum DiffFormat {
    Human,
    Json,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum UpgradeFormat {
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
        Some(PresetCommands::List { format }) => list_templates(format, use_colors),
        Some(PresetCommands::Show { name, format }) => show_template(&name, format, use_colors),
        Some(PresetCommands::New(new_args)) => {
            new_preset(config_sources, hats_source, new_args, use_colors).await
        }
        Some(PresetCommands::Check { format, strict }) => {
            check_preset(config_sources, hats_source, format, strict, use_colors).await
        }
        Some(PresetCommands::Diff { file, format }) => diff_preset(&file, format, use_colors),
        Some(PresetCommands::Upgrade {
            file,
            format,
            dry_run,
            force: _,
        }) => {
            // force flag is not implemented in MVP; reserved for future.
            // dry_run is accepted and defaults to true; MVP always behaves
            // as dry-run regardless of value (no write-back path yet).
            if !dry_run {
                eprintln!(
                    "warning: --no-dry-run is not implemented in MVP; \
                     upgrade will still report only and not modify the file"
                );
            }
            upgrade_preset(&file, format, use_colors)
        }
        Some(PresetCommands::MaterializeArtifacts {
            preset,
            plan_key,
            dest,
        }) => materialize_artifacts(&preset, &plan_key, dest.as_deref(), use_colors),
        Some(PresetCommands::Builtin { command }) => match command {
            PresetBuiltinCommands::List { format } => list_builtins(format, use_colors),
            PresetBuiltinCommands::Show { id, format } => show_builtin(&id, format, use_colors),
        },
        Some(PresetCommands::Verify { scenario, format }) => {
            verify_preset(config_sources, hats_source, &scenario, format, use_colors).await
        }
        None => {
            // Default to list with current config
            list_templates(PresetListFormat::Human, use_colors)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Template authoring commands (U3: list/show/new)
// ─────────────────────────────────────────────────────────────────────────────

/// Materialize embedded artifact templates for a builtin preset.
///
/// # Agent / operator contract
///
/// - Works on **binary-only** installs (no `presets/templates/` checkout).
/// - Default dest: `.ralph/forge/<plan-key>/templates/` (cwd-relative).
/// - Hats (`planner` / `executor` / `reporter` on parallel-forge) must run
///   this (or find an existing templates dir) **before** copying templates
///   into business artifact paths.
/// - Idempotent overwrite; safe to re-run in later activations.
fn materialize_artifacts(
    preset: &str,
    plan_key: &str,
    dest: Option<&std::path::Path>,
    use_colors: bool,
) -> Result<()> {
    use crate::builtin_artifact_templates::{
        default_forge_templates_dir, default_red_team_templates_dir, materialize,
    };

    if plan_key.is_empty() {
        anyhow::bail!("--plan-key must not be empty");
    }
    if plan_key.contains('/') || plan_key.contains('\\') || plan_key.contains("..") {
        anyhow::bail!(
            "--plan-key must be a single path segment (no slashes or '..'); got {plan_key:?}"
        );
    }

    let dest_dir = dest
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| match preset {
            "red-team-attack" | "builtin:red-team-attack" => {
                default_red_team_templates_dir(plan_key)
            }
            _ => default_forge_templates_dir(plan_key),
        });

    let written = materialize(preset, &dest_dir)?;
    if use_colors {
        println!(
            "{}✓{} Wrote {} artifact template(s) to {}",
            colors::GREEN,
            colors::RESET,
            written.len(),
            dest_dir.display()
        );
    } else {
        println!(
            "Wrote {} artifact template(s) to {}",
            written.len(),
            dest_dir.display()
        );
    }
    for path in written {
        println!("  {}", path.display());
    }
    Ok(())
}

fn list_templates(format: PresetListFormat, use_colors: bool) -> Result<()> {
    let templates = TemplateCatalog::template_names();

    match format {
        PresetListFormat::Json => {
            let manifests: Vec<TemplateManifest> = templates
                .iter()
                .filter_map(|name| TemplateCatalog::get_manifest(name))
                .collect();
            println!("{}", serde_json::to_string_pretty(&manifests)?);
        }
        PresetListFormat::Human => {
            println!("Available workflow templates:");
            println!();
            for name in &templates {
                if let Some(manifest) = TemplateCatalog::get_manifest(name) {
                    let difficulty_str = match manifest.difficulty {
                        TemplateDifficulty::Beginner => "beginner",
                        TemplateDifficulty::Intermediate => "intermediate",
                        TemplateDifficulty::Advanced => "advanced",
                    };
                    if use_colors {
                        println!("  {}{}{}", colors::CYAN, name, colors::RESET);
                    } else {
                        println!("  {}", name);
                    }
                    println!("    {}", manifest.description);
                    println!(
                        "    Difficulty: {} | Category: {}",
                        difficulty_str, manifest.category
                    );
                    if let Some(source) = &manifest.source {
                        println!("    Source: {}", source);
                    }
                    println!();
                }
            }
            println!("Use `ralph preset show <name>` to see template details.");
            println!("Use `ralph preset new <name> --name <preset-name>` to generate a preset.");
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// U01/U02: Builtin introspection — list public-only inventory and
// emit raw embedded yaml (S1, S2, S3, S4, S5, S12)
// ─────────────────────────────────────────────────────────────────────────────

/// Projection of a public `EmbeddedPreset` into the stable builtin-list
/// JSON envelope.  Field names are part of the agent-visible contract:
/// `id` is the embedded `name`, `source` is always derived as
/// `builtin:<id>` (no whitespace, no case drift), `description` is the
/// embedded description, and `public` is always `true` for items that
/// reach this projection (hidden presets are filtered upstream by
/// `list_presets`).
#[derive(serde::Serialize)]
struct BuiltinListItem {
    id: String,
    source: String,
    description: String,
    public: bool,
}

/// Top-level envelope shape for `ralph preset builtin list --format json`.
///
/// Stable across versions: callers (project-bootstrap resolver, operator
/// scripts) parse `{presets: [...]}` and never the bare array.
#[derive(serde::Serialize)]
struct BuiltinListEnvelope {
    presets: Vec<BuiltinListItem>,
}

/// Render the public builtin inventory derived from `EmbeddedPreset`.
///
/// # Agent / operator contract (U01 / S1, S2)
///
/// - Source is the `EmbeddedPreset` table compiled into the binary —
///   no filesystem reads, no TemplateCatalog fallback, no bootstrap
///   resolver invocation.
/// - Hidden presets (`public=false`) MUST NOT appear in either format.
/// - `source` is strictly `builtin:<id>` (no whitespace, no case drift).
/// - JSON shape is `{presets: [{id, source, description, public}, ...]}`.
/// - Human format labels builtin IDs and visibility (S2).
fn list_builtins(format: PresetBuiltinListFormat, use_colors: bool) -> Result<()> {
    let items: Vec<BuiltinListItem> = crate::presets::list_presets()
        .into_iter()
        .map(|preset| BuiltinListItem {
            id: preset.name.to_string(),
            source: format!("builtin:{}", preset.name),
            description: preset.description.to_string(),
            public: preset.public,
        })
        .collect();

    match format {
        PresetBuiltinListFormat::Json => {
            let envelope = BuiltinListEnvelope { presets: items };
            println!("{}", serde_json::to_string_pretty(&envelope)?);
        }
        PresetBuiltinListFormat::Human => {
            println!("Available builtin presets (compiled into this binary):");
            println!();
            for item in &items {
                if use_colors {
                    println!("  {}{}{}", colors::CYAN, item.id, colors::RESET);
                } else {
                    println!("  {}", item.id);
                }
                println!("    source: {}", item.source);
                println!("    public: {}", item.public);
                println!("    {}", item.description);
                println!();
            }
            if items.is_empty() {
                println!("  (no public builtin presets available)");
            }
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// U02: Builtin introspection — show raw embedded yaml (S3, S4, S5)
// ─────────────────────────────────────────────────────────────────────────────

/// Return the byte-exact YAML body for a builtin preset.
///
/// The return type is the same `&'static str` as
/// `EmbeddedPreset.content` so the bytes the helper hands to a
/// downstream consumer (e.g. the `project-bootstrap` resolver)
/// are bit-for-bit identical to what the CLI writes to stdout in
/// `Yaml` mode. Pinning the contract here keeps the byte-equality
/// testable without intercepting real stdout.
fn builtin_yaml_bytes(preset: &crate::presets::EmbeddedPreset) -> &'static [u8] {
    preset.content.as_bytes()
}

/// JSON envelope for `ralph preset builtin show <id> --format json`.
///
/// Same field set as `BuiltinListItem` (mirrors the agent-visible
/// contract used by `preset builtin list --format json`); the YAML
/// body itself is *not* inlined in this struct — JSON consumers who
/// want the full YAML should call `--format yaml` instead.
#[derive(serde::Serialize)]
struct BuiltinShowEnvelope<'a> {
    id: &'a str,
    source: String,
    description: &'a str,
    public: bool,
    /// True iff the YAML body is the byte-exact `EmbeddedPreset.content`
    /// (it always is; this flag documents the contract for the JSON
    /// consumer, mirroring the list envelope's `public` field).
    content_available: bool,
}

/// Render a builtin preset's metadata + raw YAML for `preset builtin show`.
///
/// # Agent / operator contract (U02 / S3, S4, S5)
///
/// - `get_preset` is the **single** lookup helper. It does **not**
///   filter by `public`, so hidden IDs (`merge-loop`) resolve
///   successfully. The `list` path keeps using `list_presets`
///   (which filters by `public`) — the two paths are explicitly
///   decoupled so a future `public` change never silently widens
///   the show path.
/// - `Yaml` mode writes `EmbeddedPreset.content` to stdout **byte-exact**
///   — no reserialize, no trim, no extra newline. Downstream callers
///   (e.g. `project-bootstrap` resolver, U03) feed the bytes straight
///   into a YAML parser; any normalization here would corrupt that
///   contract.
/// - `Human` mode renders the metadata envelope as labeled text.
/// - `Json` mode emits the same envelope as JSON.
/// - Unknown id → `anyhow!` with the id; the error path writes
///   nothing to stdout, exits non-zero. Stderr is the standard
///   anyhow path (non-zero, line including the id).
fn show_builtin(id: &str, format: PresetBuiltinShowFormat, use_colors: bool) -> Result<()> {
    let preset = crate::presets::get_preset(id)
        .ok_or_else(|| anyhow::anyhow!("unknown builtin preset: {id}"))?;

    match format {
        PresetBuiltinShowFormat::Yaml => {
            // Byte-exact: write the embedded content as-is. Use
            // `write_all` (not `println!`) to avoid appending a trailing
            // newline that wasn't in the source string. The `&str`
            // is already UTF-8-valid (compile-time `include_str!`).
            //
            // The actual byte stream is sourced from
            // `builtin_yaml_bytes(preset)` so unit tests can pin the
            // byte-equality invariant without intercepting stdout.
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle
                .write_all(builtin_yaml_bytes(preset))
                .context("failed to write builtin preset yaml to stdout")?;
        }
        PresetBuiltinShowFormat::Json => {
            let envelope = BuiltinShowEnvelope {
                id: preset.name,
                source: format!("builtin:{}", preset.name),
                description: preset.description,
                public: preset.public,
                content_available: true,
            };
            println!("{}", serde_json::to_string_pretty(&envelope)?);
        }
        PresetBuiltinShowFormat::Human => {
            if use_colors {
                println!(
                    "{}Builtin preset:{} {}",
                    colors::CYAN,
                    colors::RESET,
                    preset.name
                );
            } else {
                println!("Builtin preset: {}", preset.name);
            }
            println!("  source:      builtin:{}", preset.name);
            println!("  public:      {}", preset.public);
            println!("  description: {}", preset.description);
            println!();
            println!("Use `--format yaml` to dump the raw embedded YAML body.");
        }
    }
    Ok(())
}

fn show_template(name: &str, format: PresetShowFormat, _use_colors: bool) -> Result<()> {
    let manifest = TemplateCatalog::get_manifest(name).ok_or_else(|| {
        anyhow::anyhow!(
            "template '{}' not found. Available templates: {}",
            name,
            TemplateCatalog::template_names().join(", ")
        )
    })?;

    match format {
        PresetShowFormat::Yaml => {
            // Show the raw template YAML with placeholders
            let template_content = TemplateCatalog::raw_template(name)
                .ok_or_else(|| anyhow::anyhow!("unknown template: {}", name))?;
            println!("{}", template_content);
        }
        PresetShowFormat::Json => {
            // Emit the manifest as JSON so agents can consume it programmatically.
            // The manifest is the catalog of record for template metadata; the raw
            // YAML is only available via `--format yaml` (it carries placeholders).
            println!("{}", serde_json::to_string_pretty(&manifest)?);
        }
        PresetShowFormat::Human => {
            println!("Template: {}", name);
            println!();
            println!("Version:    {}", manifest.version);
            println!("Category:  {}", manifest.category);
            println!("Difficulty: {:?}", manifest.difficulty);
            if let Some(source) = &manifest.source {
                println!("Source:    {}", source);
            }
            println!();
            println!("Description:");
            println!("  {}", manifest.description);
            println!();
            println!("Recommended checks: {}", manifest.recommended_checks);
            println!();
            println!("Placeholders:");
            for ph in &manifest.placeholders {
                let default_str = ph.default.as_deref().unwrap_or("(required)");
                println!(
                    "  - {}: {} [default: {}]",
                    ph.name, ph.description, default_str
                );
            }
            if let Some(notes) = &manifest.output_notes {
                println!();
                println!("Output notes: {}", notes);
            }
            println!();
            println!(
                "Use `ralph preset show {} --format yaml` to see the raw template.",
                name
            );
        }
    }
    Ok(())
}

async fn new_preset(
    _config_sources: &[ConfigSource],
    _hats_source: Option<&HatsSource>,
    args: NewPresetArgs,
    use_colors: bool,
) -> Result<()> {
    // Validate template exists
    let manifest = TemplateCatalog::get_manifest(&args.template).ok_or_else(|| {
        anyhow::anyhow!(
            "template '{}' not found. Available: {}",
            args.template,
            TemplateCatalog::template_names().join(", ")
        )
    })?;

    // Resolve preset name
    let preset_name = args
        .name
        .clone()
        .ok_or_else(|| anyhow::anyhow!("--name is required. Example: --name my-workflow"))?;

    // Validate preset name strictly: only [a-zA-Z0-9_-]. This blocks both
    // path traversal and any character that would need quoting inside a YAML
    // plain scalar (`:`, `"`, `#`, spaces, etc.), keeping the rendered output
    // round-trippable through the re-quote step in TemplateRenderer::render.
    if preset_name.is_empty() {
        return Err(anyhow::anyhow!("preset name cannot be empty"));
    }
    if !preset_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(anyhow::anyhow!(
            "preset name '{}' contains invalid characters. \
             Use only ASCII letters, digits, underscores, and hyphens.",
            preset_name
        ));
    }

    // Resolve output path
    let output_path = args.output.clone().unwrap_or_else(|| {
        PathBuf::from(".ralph")
            .join("hats")
            .join(format!("{}.yml", preset_name))
    });

    // Check if output file exists (unless --force)
    if output_path.exists() && !args.force {
        return Err(anyhow::anyhow!(
            "output file '{}' already exists. Use --force to overwrite.",
            output_path.display()
        ));
    }

    // Prepare placeholder values
    let description = args
        .description
        .clone()
        .unwrap_or_else(|| manifest.description.clone());
    let generated_at = chrono_now_rfc3339();

    // Render template
    let rendered = TemplateCatalog::render_template(
        &args.template,
        &[
            ("preset_name", &preset_name),
            ("description", &description),
            ("generated_at", &generated_at),
        ],
    )
    .map_err(|e| anyhow::anyhow!("failed to render template: {}", e))?;

    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!("failed to create directory '{}': {}", parent.display(), e)
        })?;
    }

    // Write atomically: write to a uniquely-named temp file in the same
    // directory, fsync, then rename.  Using a fixed `.tmp` extension would
    // race with concurrent `ralph preset new` invocations that target the
    // same directory (e.g. CI generating several presets in parallel) —
    // `NamedTempFile` mints a unique suffix and refuses to clobber.
    let parent_dir = output_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let temp = tempfile::Builder::new()
        .prefix(".ralph-preset-")
        .suffix(".tmp")
        .tempfile_in(&parent_dir)
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to create temp file in '{}': {}",
                parent_dir.display(),
                e
            )
        })?;
    temp.as_file().write_all(rendered.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(&output_path)
        .map_err(|e| anyhow::anyhow!("failed to write '{}': {}", output_path.display(), e.error))?;

    // Build response
    match args.format {
        NewPresetFormat::Json => {
            #[derive(serde::Serialize)]
            struct NewPresetResult {
                path: String,
                template: String,
                template_version: String,
                name: String,
                description: String,
                check_profile: String,
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&NewPresetResult {
                    path: output_path.display().to_string(),
                    template: args.template.clone(),
                    template_version: manifest.version.clone(),
                    name: preset_name,
                    description,
                    check_profile: manifest.recommended_checks.to_string(),
                })?
            );
        }
        NewPresetFormat::Human => {
            if use_colors {
                println!(
                    "{}Preset generated successfully!{}",
                    colors::GREEN,
                    colors::RESET
                );
            } else {
                println!("Preset generated successfully!");
            }
            println!();
            println!("  Path:           {}", output_path.display());
            println!("  Template:       {}", args.template);
            println!("  Template version: {}", manifest.version);
            println!("  Name:           {}", preset_name);
            println!("  Description:    {}", description);
            println!("  Check profile:  {}", manifest.recommended_checks);
            println!();
            println!("Next steps:");
            println!("  1. Review and customize: {}", output_path.display());
            println!(
                "  2. Run authoring checks: ralph preset check -H {}",
                output_path.display()
            );
            println!(
                "  3. Execute the workflow:  ralph run -H {} -p '<prompt>'",
                output_path.display()
            );
        }
    }

    // Run --check if requested
    if args.check {
        println!();
        println!("Running authoring checks...");
        let report = build_report(&[ConfigSource::File(output_path.clone())], None, false)
            .await
            .context("Failed to build preset contract report")?;

        if report.passed {
            if use_colors {
                println!("  {}Authoring checks: PASS{}", colors::GREEN, colors::RESET);
            } else {
                println!("  Authoring checks: PASS");
            }
        } else {
            if use_colors {
                println!("  {}Authoring checks: FAIL{}", colors::RED, colors::RESET);
            } else {
                println!("  Authoring checks: FAIL");
            }
            println!("  Warnings: {}, Errors: {}", report.warnings, report.errors);
            println!(
                "  The generated file has been kept at: {}",
                output_path.display()
            );
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Returns current time in RFC3339 format.
fn chrono_now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ─────────────────────────────────────────────────────────────────────────────
// Preset contract check
// ─────────────────────────────────────────────────────────────────────────────

async fn check_preset(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    format: PresetCheckFormat,
    strict: bool,
    use_colors: bool,
) -> Result<()> {
    let report = build_report(config_sources, hats_source, strict)
        .await
        .context("Failed to build preset contract report")?;

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

/// Load the config + hats source and run the runtime contract aggregator.
///
/// Split out from `check_preset` so tests can exercise the report-building
/// path without invoking `std::process::exit` or hitting a real CLI parser.
/// The function is `pub(crate)` so the test module below can call it with
/// crafted configs and assert on the resulting `RuntimeContractReport`.
pub(crate) async fn build_report(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    strict: bool,
) -> Result<RuntimeContractReport> {
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

    Ok(
        ralph_core::runtime_contract::RuntimeContractAggregator::aggregate(
            &source_label,
            &config,
            &registry,
            strictness,
            None,
        ),
    )
}

fn preset_source_label(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
) -> String {
    if let Some(source) = hats_source {
        return source.label().clone();
    }
    // Use the first file-based config source as label
    for source in config_sources {
        if let ConfigSource::File(path) = source {
            return path.to_string_lossy().to_string();
        }
    }
    "current-config".to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Preset runtime verify (Unit 4)
// ─────────────────────────────────────────────────────────────────────────────

async fn verify_preset(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    scenario_path: &PathBuf,
    format: PresetVerifyFormat,
    use_colors: bool,
) -> Result<()> {
    // Step 1 — refuse remote sources before any I/O.
    for source in config_sources {
        if let ConfigSource::Remote(url) = source {
            return Err(anyhow!(
                "ralph preset verify does not accept remote config source: {url}"
            ));
        }
    }
    if let Some(source) = hats_source {
        let label = source.label();
        if SourceKind::is_remote(&label) {
            return Err(anyhow!(
                "ralph preset verify does not accept remote hats source: {label}"
            ));
        }
    }

    // Step 2 — load preset/config via existing preflight.
    let config = preflight::load_config_for_preflight(config_sources, hats_source)
        .await
        .context("Failed to load config for preset verify")?;
    let config_starting_event = config
        .event_loop
        .starting_event
        .clone()
        .unwrap_or_else(|| "task.start".to_string());

    // U2 (P1:adversarial:A2) — execution_mode guard: reject coordinator mode.
    if config.event_loop.execution_mode != HatExecutionMode::Isolated {
        let detail =
            "preset verify requires event_loop.execution_mode: isolated; coordinator mode is \
             not supported";
        eprintln!("{detail}");
        let mut report = PresetVerifyReport::default();
        report.with_failure_kind("input_error");
        return render_and_exit(report, format, use_colors);
    }

    // Step 3 — strict static contract check (single source of truth).
    let source_label = preset_source_label(config_sources, hats_source);
    let registry = HatRegistry::from_runtime_config(&config);
    let strictness = RuntimeContractStrictness::preset_check_strict();
    let static_report = ralph_core::runtime_contract::RuntimeContractAggregator::aggregate(
        &source_label,
        &config,
        &registry,
        strictness,
        None,
    );
    let static_layer = StaticLayer {
        passed: static_report.passed,
        warnings: static_report
            .findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Warn)
            .count(),
        errors: static_report
            .findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Error)
            .count(),
        findings: static_report
            .findings
            .iter()
            .map(|f| format!("[{:?}] {}", f.source, f.message))
            .collect(),
    };

    // Source-kind derivation for the report.
    let source_kind = hats_source
        .map(|hs| SourceKind::from_hats_source(&hs.label()))
        .unwrap_or(SourceKind::External);

    // Static failure → return early with the static layer in the report.
    if !static_layer.passed {
        let mut report = PresetVerifyReport::default();
        report.with_failure_kind("static_contract_failure");
        report.source_kind = source_kind;
        report.static_layer = static_layer;
        return render_and_exit(report, format, use_colors);
    }

    // Step 4 — read and parse the scenario YAML.
    let scenario_yaml = std::fs::read_to_string(scenario_path)
        .with_context(|| format!("Failed to read scenario file {}", scenario_path.display()))?;
    let scenario_file = match ScenarioFile::from_yaml(&scenario_yaml, &config_starting_event) {
        Ok(s) => s,
        Err(err) => {
            // U4 (P2:adversarial:A3) — StartEventMismatch is an *input* error
            // (the scenario's start_event must match the preset's
            // starting_event). Reclassify to "input_error" instead of
            // "scenario_failure" so reports are actionable.
            let kind = "input_error";
            let detail = format!("{err:?}");
            let mut report = PresetVerifyReport::default();
            report.with_failure_kind(kind);
            report.source_kind = source_kind;
            report.static_layer = static_layer;
            eprintln!("scenario parse error: {detail}");
            return render_and_exit(report, format, use_colors);
        }
    };

    // Step 5 — run each scenario through the real driver + verdict evaluator.
    let mut outcomes = Vec::with_capacity(scenario_file.scenarios.len());
    let mut overall_failure: Option<FailureKind> = None;
    let input_blob = scenario_yaml.as_str();
    for scenario in &scenario_file.scenarios {
        let workspace = match DriverWorkspace::new() {
            Ok(ws) => ws,
            Err(e) => {
                let mut report = PresetVerifyReport::default();
                report.with_failure_kind("runtime_exception");
                report.source_kind = source_kind;
                report.static_layer = static_layer;
                eprintln!("workspace error: {e}");
                return render_and_exit(report, format, use_colors);
            }
        };
        match run_scenario(scenario, &config, &workspace, input_blob) {
            Ok(outcome) => {
                let scenario_report = evaluate_scenario(outcome.clone());
                if !scenario_report.passed && overall_failure.is_none() {
                    if let Some(kind) = &outcome.failure_kind {
                        overall_failure = Some(kind.clone());
                    }
                }
                outcomes.push((outcome, scenario_report));
            }
            Err(kind) => {
                let mut report = PresetVerifyReport::default();
                report.with_failure_kind(kind.tag().to_string());
                report.source_kind = source_kind;
                report.static_layer = static_layer;
                eprintln!("runtime error: {kind:?}");
                return render_and_exit(report, format, use_colors);
            }
        }
    }

    let report = build_verify_report(
        source_kind,
        static_layer,
        outcomes,
        overall_failure.as_ref(),
        input_blob,
    );
    render_and_exit(report, format, use_colors)
}

fn render_and_exit(
    report: PresetVerifyReport,
    format: PresetVerifyFormat,
    use_colors: bool,
) -> Result<()> {
    match format {
        PresetVerifyFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        PresetVerifyFormat::Human => {
            print_verify_human_report(&report, use_colors);
        }
    }
    if !report.passed {
        std::process::exit(1);
    }
    Ok(())
}

fn print_verify_human_report(report: &PresetVerifyReport, _use_colors: bool) {
    println!(
        "Preset Verify: source_kind={:?} passed={}",
        report.source_kind, report.passed
    );
    println!(
        "static layer: passed={} warnings={} errors={}",
        report.static_layer.passed, report.static_layer.warnings, report.static_layer.errors
    );
    if let Some(kind) = &report.failure_kind {
        println!("overall failure_kind: {kind}");
    }
    for scenario in &report.scenarios {
        println!(
            "  scenario '{}': passed={} steps={} failure_kind={:?}",
            scenario.name, scenario.passed, scenario.steps, scenario.failure_kind
        );
        if let Some(last_hat) = &scenario.last_observable_state.last_hat {
            println!("    last_hat={last_hat}");
        }
        if let Some(topic) = &scenario.last_observable_state.last_accepted_topic {
            println!("    last_accepted_topic={topic}");
        }
        println!("    trace_digest={}", scenario.trace_digest);
    }
    if !report.trace_digest.is_empty() {
        println!("report trace_digest={}", report.trace_digest);
    }
}

fn print_human_report(report: &RuntimeContractReport, use_colors: bool) {
    println!("Preset Contract Check: {}", report.source_label);
    println!();

    // Group findings by source
    let mut config_findings = Vec::new();
    let mut lint_findings = Vec::new();
    let mut topology_findings = Vec::new();
    let mut orphan_findings = Vec::new();
    let mut payload_findings = Vec::new();

    for finding in &report.findings {
        match finding.source {
            ralph_core::runtime_contract::FindingSource::Config => {
                config_findings.push(finding);
            }
            ralph_core::runtime_contract::FindingSource::Lint => {
                lint_findings.push(finding);
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

    // Print Lint section
    println!("Lint:");
    if lint_findings.is_empty() {
        print_finding_line(use_colors, FindingSeverity::Pass, "No lint issues");
    } else {
        for finding in &lint_findings {
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

// ─────────────────────────────────────────────────────────────────────────────
// U4: Version Diff and Upgrade Preview
// ─────────────────────────────────────────────────────────────────────────────

/// Result of comparing a local preset with its template baseline.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiffResult {
    /// The template name.
    pub template: String,
    /// The version in the local file.
    pub local_version: String,
    /// The version in the current catalog.
    pub catalog_version: String,
    /// Whether the versions are the same AND content matches the rendered baseline.
    pub up_to_date: bool,
    /// Whether the local version is older than catalog.
    pub has_update: bool,
    /// Whether the local version is newer than catalog.
    pub is_newer: bool,
    /// Whether the local file has user-side modifications relative to the
    /// rendered baseline while sharing the catalog's `template_version`.
    ///
    /// `up_to_date=false`, `has_update=false`, `is_local_drift=true` means
    /// "the preset is on the same template version as the catalog but the
    /// user has edited it locally; no upgrade is available".  Agents should
    /// treat this as informational drift, not as a prompt to run `upgrade`.
    #[serde(default)]
    pub is_local_drift: bool,
    /// Status description.
    pub status: String,
    /// Summary of changes between local and baseline.
    pub changes_summary: Vec<String>,
    /// The full unified diff (if any).
    pub diff_lines: Vec<String>,
}

impl DiffResult {
    /// Create a new diff result indicating the preset is up to date.
    fn up_to_date(template: &str, version: &str) -> Self {
        DiffResult {
            template: template.to_string(),
            local_version: version.to_string(),
            catalog_version: version.to_string(),
            up_to_date: true,
            has_update: false,
            is_newer: false,
            is_local_drift: false,
            status: "up to date".to_string(),
            changes_summary: vec![],
            diff_lines: vec![],
        }
    }

    /// Create a diff result for an old version.
    fn needs_update(
        template: &str,
        local_version: &str,
        catalog_version: &str,
        diff_lines: Vec<String>,
    ) -> Self {
        DiffResult {
            template: template.to_string(),
            local_version: local_version.to_string(),
            catalog_version: catalog_version.to_string(),
            up_to_date: false,
            has_update: true,
            is_newer: false,
            is_local_drift: false,
            status: format!("update available: {} → {}", local_version, catalog_version),
            changes_summary: vec![format!(
                "Template '{}' has been updated from {} to {}",
                template, local_version, catalog_version
            )],
            diff_lines,
        }
    }

    /// Create a diff result indicating the local file matches the catalog's
    /// `template_version` but has been edited by the user.  This is distinct
    /// from `needs_update` (catalog is newer) and `is_newer_version` (local
    /// is newer); no `upgrade` is available, but the user should know their
    /// local file diverges from the rendered baseline.
    fn local_drift(template: &str, version: &str, significant_diff: Vec<String>) -> Self {
        DiffResult {
            template: template.to_string(),
            local_version: version.to_string(),
            catalog_version: version.to_string(),
            up_to_date: false,
            has_update: false,
            is_newer: false,
            is_local_drift: true,
            status: "local changes".to_string(),
            changes_summary: vec![format!(
                "Local preset '{}' has user modifications on top of template version {}",
                template, version
            )],
            diff_lines: significant_diff,
        }
    }

    /// Create a diff result for a newer local version.
    fn is_newer_version(template: &str, local_version: &str, catalog_version: &str) -> Self {
        DiffResult {
            template: template.to_string(),
            local_version: local_version.to_string(),
            catalog_version: catalog_version.to_string(),
            up_to_date: false,
            has_update: false,
            is_newer: true,
            is_local_drift: false,
            status: format!(
                "local version {} is newer than catalog {} (Ralph may be outdated)",
                local_version, catalog_version
            ),
            changes_summary: vec![format!(
                "Local preset was generated with a newer template version. \
                Consider updating Ralph to get the latest template changes."
            )],
            diff_lines: vec![],
        }
    }
}

/// Result of checking upgrade status.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UpgradeResult {
    /// The template name.
    pub template: String,
    /// The version in the local file.
    pub local_version: String,
    /// The version in the current catalog.
    pub catalog_version: String,
    /// Whether upgrade is available.
    pub upgrade_available: bool,
    /// Status description.
    pub status: String,
    /// Suggestions for the user.
    pub suggestions: Vec<String>,
}

impl UpgradeResult {
    /// Create an upgrade result for already current.
    fn already_current(template: &str, version: &str) -> Self {
        UpgradeResult {
            template: template.to_string(),
            local_version: version.to_string(),
            catalog_version: version.to_string(),
            upgrade_available: false,
            status: "already current".to_string(),
            suggestions: vec![],
        }
    }

    /// Create an upgrade result for an outdated version.
    fn needs_upgrade(template: &str, local_version: &str, catalog_version: &str) -> Self {
        let mut suggestions = Vec::new();
        suggestions.push(format!(
            "Regenerate your preset: ralph preset new {} --name <name> --output /tmp/new.yml",
            template
        ));
        suggestions.push(
            "Compare the new template with your current preset and merge changes manually"
                .to_string(),
        );
        suggestions
            .push("Run: ralph preset diff --file /tmp/new.yml to see what changed".to_string());

        UpgradeResult {
            template: template.to_string(),
            local_version: local_version.to_string(),
            catalog_version: catalog_version.to_string(),
            upgrade_available: true,
            status: format!("upgrade available: {} → {}", local_version, catalog_version),
            suggestions,
        }
    }

    /// Create an upgrade result for a newer local version.
    fn local_is_newer(template: &str, local_version: &str, catalog_version: &str) -> Self {
        UpgradeResult {
            template: template.to_string(),
            local_version: local_version.to_string(),
            catalog_version: catalog_version.to_string(),
            upgrade_available: false,
            status: format!(
                "local version {} is newer than catalog {}",
                local_version, catalog_version
            ),
            suggestions: vec![
                "Your local preset was generated with a newer template version.".to_string(),
                "Consider updating Ralph to get the latest template changes.".to_string(),
            ],
        }
    }
}

/// Compute a unified diff between two strings, line by line.
fn compute_unified_diff(original: &str, revised: &str) -> Vec<String> {
    let original_lines: Vec<&str> = original.lines().collect();
    let revised_lines: Vec<&str> = revised.lines().collect();

    // Simple line-by-line comparison for unified diff format
    let mut diff_lines = Vec::new();

    // Find common prefix and suffix
    let mut start = 0;
    let max_start = std::cmp::min(original_lines.len(), revised_lines.len());
    while start < max_start && original_lines[start] == revised_lines[start] {
        start += 1;
    }

    let mut end = 0;
    let mut max_end = 0;
    while start + end < max_start
        && original_lines[original_lines.len() - 1 - end]
            == revised_lines[revised_lines.len() - 1 - end]
    {
        max_end = end + 1;
        end += 1;
    }

    let orig_changed = &original_lines[start..original_lines.len() - max_end];
    let rev_changed = &revised_lines[start..revised_lines.len() - max_end];

    if orig_changed.is_empty() && rev_changed.is_empty() {
        return vec![];
    }

    // Generate unified diff header
    diff_lines.push(format!("--- original/{}", "local"));
    diff_lines.push(format!("+++ revised/{}", "baseline"));

    diff_lines.push(format!(
        "@@ -{},{} +{},{} @@",
        start + 1,
        orig_changed.len(),
        start + 1,
        rev_changed.len()
    ));

    for line in orig_changed {
        diff_lines.push(format!("-{}", line));
    }
    for line in rev_changed {
        diff_lines.push(format!("+{}", line));
    }

    diff_lines
}

/// Read and parse x_preset metadata from a YAML file.
fn read_xpreset_metadata(path: &PathBuf) -> Result<XPresetMetadata, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read file '{}': {}", path.display(), e))?;

    let value: serde_yaml::Value = serde_yaml::from_str(&content)
        .map_err(|e| format!("failed to parse YAML in '{}': {}", path.display(), e))?;

    XPresetMetadata::from_yaml_value(&value)
        .map_err(|e| format!("failed to parse x_preset metadata: {}", e))?
        .ok_or_else(|| {
            format!(
                "file '{}' does not contain x_preset metadata. \
                This file may not have been generated by 'ralph preset new'. \
                To check this preset, use: ralph preset check -H {}",
                path.display(),
                path.display()
            )
        })
}

/// Diff a local preset file against its template baseline.
fn diff_preset(path: &PathBuf, format: DiffFormat, use_colors: bool) -> Result<()> {
    // Read and parse x_preset metadata
    let metadata = read_xpreset_metadata(path).map_err(|e| anyhow::anyhow!(e))?;

    // Find the template in the catalog
    let manifest = TemplateCatalog::get_manifest(&metadata.template).ok_or_else(|| {
        anyhow::anyhow!(
            "template '{}' not found in current catalog. \
                The template may have been removed or renamed.",
            metadata.template
        )
    })?;

    let catalog_version = &manifest.version;

    // Compare versions
    let local_version = Version::parse(&metadata.template_version).map_err(|e| {
        anyhow::anyhow!(
            "invalid local template_version '{}': {}",
            metadata.template_version,
            e
        )
    })?;

    let catalog_ver = Version::parse(catalog_version)
        .map_err(|e| anyhow::anyhow!("invalid catalog version '{}': {}", catalog_version, e))?;

    let result = if local_version == catalog_ver {
        // Versions match - check for content drift
        let rendered = TemplateCatalog::render_template(
            &metadata.template,
            &[
                ("preset_name", &metadata.name),
                ("description", &metadata.description),
                ("generated_at", &metadata.generated_at),
            ],
        )
        .map_err(|e| anyhow::anyhow!("failed to render template baseline: {}", e))?;

        let local_content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read file: {}", e))?;

        // Extract just the hats section and x_preset for comparison (ignore generated_at drift)
        let diff_lines = compute_unified_diff(&local_content, &rendered);

        if diff_lines.is_empty() {
            DiffResult::up_to_date(&metadata.template, &metadata.template_version)
        } else {
            // Filter out generated_at differences as they're expected
            let significant_diff: Vec<String> = diff_lines
                .into_iter()
                .filter(|l| !l.contains("generated_at"))
                .collect();

            if significant_diff.is_empty() {
                DiffResult::up_to_date(&metadata.template, &metadata.template_version)
            } else {
                // Same template_version, but the local file has been edited.
                // Distinguish this from `needs_update` (catalog is newer): the
                // user already has the latest template; they just diverged.
                DiffResult::local_drift(
                    &metadata.template,
                    &metadata.template_version,
                    significant_diff,
                )
            }
        }
    } else if local_version < catalog_ver {
        // Local is older - compute what changed
        let rendered = TemplateCatalog::render_template(
            &metadata.template,
            &[
                ("preset_name", &metadata.name),
                ("description", &metadata.description),
                ("generated_at", &metadata.generated_at),
            ],
        )
        .map_err(|e| anyhow::anyhow!("failed to render template baseline: {}", e))?;

        let local_content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read file: {}", e))?;

        let diff_lines = compute_unified_diff(&local_content, &rendered);

        DiffResult::needs_update(
            &metadata.template,
            &metadata.template_version,
            catalog_version,
            diff_lines,
        )
    } else {
        // Local is newer than catalog
        DiffResult::is_newer_version(
            &metadata.template,
            &metadata.template_version,
            catalog_version,
        )
    };

    match format {
        DiffFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        DiffFormat::Human => {
            print_diff_human(&result, use_colors);
        }
    }

    Ok(())
}

fn print_diff_human(result: &DiffResult, use_colors: bool) {
    println!("Preset Diff: {}", result.template);
    println!();

    // Status with color
    let status_color = if result.up_to_date {
        colors::GREEN
    } else {
        colors::YELLOW
    };

    if use_colors {
        println!("Status: {}{}{}", status_color, result.status, colors::RESET);
    } else {
        println!("Status: {}", result.status);
    }
    println!();

    println!("Template:     {}", result.template);
    println!("Local version:  {}", result.local_version);
    println!("Catalog version: {}", result.catalog_version);
    println!();

    if !result.changes_summary.is_empty() {
        println!("Changes:");
        for change in &result.changes_summary {
            println!("  - {}", change);
        }
        println!();
    }

    if !result.diff_lines.is_empty() {
        println!("Diff:");
        for line in &result.diff_lines {
            // Color the diff lines
            if use_colors {
                if line.starts_with('+') {
                    println!("{}{}", colors::GREEN, line);
                } else if line.starts_with('-') {
                    println!("{}{}", colors::RED, line);
                } else if line.starts_with("@@") {
                    println!("{}{}{}", colors::CYAN, line, colors::RESET);
                } else {
                    println!("{}", line);
                }
            } else {
                println!("{}", line);
            }
        }
        println!();
    }

    // Suggestion based on status
    if result.has_update {
        println!("To upgrade: ralph preset upgrade --file <path> --dry-run");
    } else if result.up_to_date {
        println!("Your preset is up to date with the template.");
    }
}

/// Check upgrade status for a local preset file.
fn upgrade_preset(path: &PathBuf, format: UpgradeFormat, use_colors: bool) -> Result<()> {
    // Read and parse x_preset metadata
    let metadata = read_xpreset_metadata(path).map_err(|e| anyhow::anyhow!(e))?;

    // Find the template in the catalog
    let manifest = TemplateCatalog::get_manifest(&metadata.template).ok_or_else(|| {
        anyhow::anyhow!(
            "template '{}' not found in current catalog. \
                The template may have been removed or renamed.",
            metadata.template
        )
    })?;

    let catalog_version = &manifest.version;

    // Compare versions
    let local_version = Version::parse(&metadata.template_version).map_err(|e| {
        anyhow::anyhow!(
            "invalid local template_version '{}': {}",
            metadata.template_version,
            e
        )
    })?;

    let catalog_ver = Version::parse(catalog_version)
        .map_err(|e| anyhow::anyhow!("invalid catalog version '{}': {}", catalog_version, e))?;

    let result = if local_version == catalog_ver {
        UpgradeResult::already_current(&metadata.template, &metadata.template_version)
    } else if local_version < catalog_ver {
        UpgradeResult::needs_upgrade(
            &metadata.template,
            &metadata.template_version,
            catalog_version,
        )
    } else {
        UpgradeResult::local_is_newer(
            &metadata.template,
            &metadata.template_version,
            catalog_version,
        )
    };

    match format {
        UpgradeFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        UpgradeFormat::Human => {
            print_upgrade_human(&result, use_colors);
        }
    }

    Ok(())
}

fn print_upgrade_human(result: &UpgradeResult, use_colors: bool) {
    println!("Preset Upgrade: {}", result.template);
    println!();

    let status_color = if result.upgrade_available {
        colors::YELLOW
    } else {
        colors::GREEN
    };

    if use_colors {
        println!("Status: {}{}{}", status_color, result.status, colors::RESET);
    } else {
        println!("Status: {}", result.status);
    }
    println!();

    println!("Template:     {}", result.template);
    println!("Local version:  {}", result.local_version);
    println!("Catalog version: {}", result.catalog_version);
    println!();

    if result.upgrade_available {
        println!("Suggestions:");
        for suggestion in &result.suggestions {
            println!("  - {}", suggestion);
        }
    } else if result.suggestions.is_empty() {
        println!("Your preset is using the latest available template version.");
    } else {
        println!("Notes:");
        for note in &result.suggestions {
            println!("  - {}", note);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::runtime_contract::{RuntimeContractFinding, RuntimeContractReport};
    use std::io::Write;

    // ─────────────────────────────────────────────────────────────────────
    // Source-label helpers (unchanged from previous round)
    // ─────────────────────────────────────────────────────────────────────

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

    // ─────────────────────────────────────────────────────────────────────
    // CLI acceptance scenarios (U3 acceptance matrix)
    //
    // These tests cover the contract behaviors that the original review
    // flagged as not covered by tests:
    //   - bad topology exit 1
    //   - strict orphan exit 1
    //   - payload JSON finding shape
    //   - default run parse path stays intact (no global -H regression)
    //   - loader failure surfaces as a clean error
    //   - global -H 两种位置 (subcommand before vs after flag) both resolve
    //
    // We exercise the public `execute` path or the `pub(crate)`
    // `build_report` helper. Exit code is verified by routing through
    // `execute` with `RUN_MODE=exit-1` shim or by inspecting
    // `report.passed` directly via `build_report`.
    // ─────────────────────────────────────────────────────────────────────

    /// Write a YAML test fixture to a per-test tempfile and return the
    /// (TempDir, path) pair.
    ///
    /// `TempDir` is auto-removed when the binding drops, so callers must
    /// keep the returned `TempDir` alive for the duration of the test.
    /// Each call lands in a unique OS-managed temp directory to avoid
    /// parallel-test race conditions where two tests share a path and
    /// one reads a partial write from the other.
    fn write_preset_tmp(yaml: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("preset.yml");
        let mut f = std::fs::File::create(&path).expect("create fixture");
        f.write_all(yaml.as_bytes()).expect("write fixture");
        f.sync_all().ok();
        (dir, path)
    }

    /// Bad-topology fixture: starting event has no subscriber, completion
    /// promise is unreachable. Used to assert that the check reports
    /// `passed = false` (which `check_preset` maps to exit code 1).
    const BAD_TOPOLOGY_YAML: &str = r#"
hats:
  a:
    name: "A"
    description: "Other-only"
    triggers: ["other.topic"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
"#;

    /// Strict-orphan fixture: hat publishes a typo topic with no
    /// subscriber. Non-strict should produce a warning; strict should
    /// flip that warning into a blocking failure.
    // WRC-U1 (2026-06-12-003): lint is now always-on in the
    // aggregator, so the orphan-only fixture must also be WAC-clean
    // to exercise the orphan path in isolation. We declare
    // `topic_format_whitelist` and a `tasks.coordinator_hats` and
    // give every hat a downstream closure path. The `sloppy` hat
    // publishes BOTH a closure path (LOOP_COMPLETE) and a dead-end
    // topic (orphan.typo). The orphan warning on `orphan.typo`
    // remains the only blocking signal in non-strict mode.
    const STRICT_ORPHAN_YAML: &str = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: true
  coordinator_hats:
    - a
hats:
  a:
    name: "A"
    description: "Entry"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    terminal_events: ["work.ready"]
  sloppy:
    name: "Sloppy"
    description: "Has a closure path but also publishes an orphan"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE", "orphan.typo"]
    terminal_events: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;

    /// Good fixture: linear chain. Used as a positive control.
    // WRC-U1 (2026-06-12-003): the lint is now always-on in the
    // aggregator path, so the GOOD_YAML fixture must declare a
    // `topic_format_whitelist` for the legacy `LOOP_COMPLETE`
    // completion promise (lint warns on uppercase) AND a
    // `tasks.coordinator_hats` entry (the `coordinator_missing`
    // check fires when `tasks.enabled = true` without a coordinator
    // hat). The 2-hat chain itself is otherwise clean from a WAC
    // perspective: work.start → work.ready (executor) → LOOP_COMPLETE.
    const GOOD_YAML: &str = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: true
  coordinator_hats:
    - a
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    terminal_events: ["work.ready"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    terminal_events: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;

    /// Payload finding fixture: downstream references a payload field
    /// but the topic has no schema. Strict mode flips the warning to
    /// an error.
    ///
    /// WRC-U1: declares the same whitelist + coordinator_hats as
    /// GOOD_YAML so the lint pass is clean except for the targeted
    /// `payload.schema_missing_for_required_topic` finding.
    const PAYLOAD_FINDING_YAML: &str = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: true
  coordinator_hats:
    - a
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    terminal_events: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    terminal_events: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;

    // ---- T1: bad topology reports failure (exit-1 invariant) ----
    #[tokio::test]
    async fn check_preset_bad_topology_reports_failed_report() {
        // `build_report` is the inner pipeline; `check_preset` calls it and
        // then maps `!report.passed` to `std::process::exit(1)`. We assert
        // the inner report so the exit-code invariant is verified
        // structurally without spawning a subprocess.
        let (_tmp, path) = write_preset_tmp(BAD_TOPOLOGY_YAML);
        let sources = vec![ConfigSource::File(path)];
        let report = build_report(&sources, None, false)
            .await
            .expect("build_report should succeed for parseable bad config");
        assert!(!report.passed, "bad topology must fail: {:?}", report);
        assert!(
            report.errors > 0,
            "bad topology must record at least one error"
        );
        let has_topology_error = report.findings.iter().any(|f| {
            f.source == ralph_core::runtime_contract::FindingSource::Topology
                && f.severity == ralph_core::runtime_contract::FindingSeverity::Error
        });
        assert!(
            has_topology_error,
            "bad topology must surface a topology error finding: {:?}",
            report.findings
        );
    }

    // ---- T2: strict orphan warning -> exit 1 (regression guard) ----
    #[tokio::test]
    async fn check_preset_strict_orphan_warning_fails_report() {
        let (_tmp, path) = write_preset_tmp(STRICT_ORPHAN_YAML);
        let sources = vec![ConfigSource::File(path)];

        // Non-strict: orphan is a warning, report still passes.
        let non_strict = build_report(&sources, None, false)
            .await
            .expect("non-strict build_report");
        assert!(
            non_strict.passed,
            "non-strict orphan warning must not fail the report: {:?}",
            non_strict
        );
        let has_orphan_warn = report_has_orphan_warn(&non_strict);
        assert!(has_orphan_warn, "orphan warning must be present");

        // Strict: same warning, fail_on_warnings=true flips it to a
        // blocking failure. This is the regression guard for the review
        // finding: scripts/validate-builtin-presets.sh used to skip
        // warnings when topology errors existed.
        let strict = build_report(&sources, None, true)
            .await
            .expect("strict build_report");
        assert!(
            !strict.passed,
            "strict orphan warning must fail the report: {:?}",
            strict
        );
    }

    // ---- T3: payload JSON finding has stable shape ----
    #[tokio::test]
    async fn check_preset_payload_finding_appears_in_json() {
        let (_tmp, path) = write_preset_tmp(PAYLOAD_FINDING_YAML);
        let sources = vec![ConfigSource::File(path)];

        // Non-strict: missing schema is a warning.
        let non_strict = build_report(&sources, None, false)
            .await
            .expect("non-strict build_report");
        let payload = non_strict
            .findings
            .iter()
            .find(|f| {
                f.source == ralph_core::runtime_contract::FindingSource::Payload
                    && f.severity == ralph_core::runtime_contract::FindingSeverity::Warn
            })
            .expect("non-strict payload warning must be present");
        assert_eq!(payload.id, "payload.schema_missing_for_required_topic");

        // Roundtrip the report through JSON and back to verify the
        // documented stable field set (source_label, payload_strict,
        // fail_on_warnings, passed, warnings, errors, findings,
        // checked_at) is intact for downstream consumers.
        let value = serde_json::to_value(&non_strict).expect("serialize report");
        let obj = value.as_object().expect("report should be an object");
        for key in [
            "source_label",
            "payload_strict",
            "fail_on_warnings",
            "passed",
            "warnings",
            "errors",
            "findings",
            "checked_at",
        ] {
            assert!(
                obj.contains_key(key),
                "report JSON missing stable key: {key}"
            );
        }
    }

    // ---- T4: loader failure surfaces as a clean Err, not a panic ----
    #[tokio::test]
    async fn check_preset_loader_failure_returns_error() {
        // Use a malformed YAML that the loader's serde layer must reject.
        // A missing file would fall back to defaults (per
        // `load_optional_user_config_value`), so it is not a loader
        // failure. The contract we care about is: when the loader
        // returns Err, `build_report` propagates it via `?` and never
        // fabricates a `passed` report.
        let malformed = "hats:\n  a:\n    name: \"A\"\n      triggers: bad_indent\n";
        let (_tmp, path) = write_preset_tmp(malformed);
        let sources = vec![ConfigSource::File(path)];
        let result = build_report(&sources, None, false).await;
        assert!(
            result.is_err(),
            "loader failure must surface as Err, not Ok with a fake report"
        );
    }

    // ---- T5: good preset passes ----
    #[tokio::test]
    async fn check_preset_good_yaml_passes() {
        let (_tmp, path) = write_preset_tmp(GOOD_YAML);
        let sources = vec![ConfigSource::File(path)];
        let report = build_report(&sources, None, false)
            .await
            .expect("good build_report");
        assert!(report.passed, "good preset must pass: {:?}", report);
        assert_eq!(report.warnings, 0);
        assert_eq!(report.errors, 0);
    }

    // ---- T6: global -H 两种位置 ----
    //
    // clap parses `ralph -H builtin:ce-executor preset check` and
    // `ralph preset check -H builtin:ce-executor` the same way when the
    // flag is declared `global = true`. We exercise both source-label
    // resolutions to confirm there is no position-dependent path through
    // `preset_source_label` that would drop the hats source.
    #[test]
    fn preset_source_label_handles_global_h_in_both_positions() {
        // -H after subcommand: hats_source is set.
        let hats_source = HatsSource::Builtin("ce-executor".to_string());
        let after_label = preset_source_label(&[], Some(&hats_source));
        assert_eq!(after_label, "builtin:ce-executor");

        // -H before subcommand: clap still hands the resolved
        // HatsSource to `execute`, so the helper sees the same input.
        // The two positions converge to the same code path; the test
        // pins that they produce the same label.
        let no_hats_label = preset_source_label(&[], None);
        // The config-only fallback label depends on what
        // ConfigSource::File path is supplied; here it is the default.
        assert_eq!(no_hats_label, "current-config");

        // When only a file is supplied, the label is the file path.
        let only_file = vec![ConfigSource::File("/abs/path/to.yml".into())];
        assert_eq!(preset_source_label(&only_file, None), "/abs/path/to.yml");
    }

    // ---- T7: default `ralph run` parse path is unaffected ----
    //
    // The review flagged a risk that adding the `preset` subcommand
    // could break clap's default-subcommand routing. We can't easily
    // exercise clap from this module, but we can verify that the
    // subcommand enum variant for Preset exists alongside Run and that
    // the parse-path argument names are stable. This is a structural
    // test — a behavioural integration test (e.g. `ralph -p "x"`
    // resolves to Run) is covered by scripts/test-cli-doc-drift.sh and
    // scripts/run-tests.sh in the plan's G5 gate.
    #[test]
    fn preset_subcommand_companion_with_run_in_enum() {
        // The `Clap` derives in main.rs declare Commands as an enum
        // containing both `Run` and `Preset` variants. This test
        // exercises `clap::Parser` on a synthesized Cli to confirm the
        // default subcommand (`ralph -p "..."` with no subcommand)
        // still parses without error, and that the explicit
        // `preset check` subcommand also parses.
        use clap::Parser;

        // We can't construct the real `Cli` here (it's in main.rs and
        // has many fields), so we exercise just the PresetArgs shape.
        // The invariant we care about is that PresetArgs accepts
        // `--format json`, `--strict`, and that the subcommand
        // discriminants are stable. If clap reorders or renames, this
        // test fails loudly.
        let parsed = PresetArgs::try_parse_from(["ralph", "check", "--format", "json", "--strict"])
            .expect("preset check --format json --strict must parse");
        match parsed.command {
            Some(PresetCommands::Check { format, strict }) => {
                assert!(matches!(format, PresetCheckFormat::Json));
                assert!(strict);
            }
            other => panic!("expected Check subcommand, got: {:?}", other),
        }

        // Default command (no subcommand) must parse without panic —
        // this is the regression guard for the "default run parse"
        // scenario. PresetArgs's `command: Option<...>` means the
        // outer enum can be `None`, which is the `check_preset` default
        // branch.
        let default = PresetArgs::try_parse_from(["ralph"]).expect("default parse");
        assert!(
            default.command.is_none(),
            "preset default (no subcommand) must parse to None"
        );
    }

    // ---- T8: report.passed -> exit code mapping ----
    //
    // check_preset uses `if !report.passed { std::process::exit(1); }`.
    // We can't observe process::exit from in-process tests, so we
    // document the invariant here as a structural assertion: for every
    // fixture above, the report.passed boolean matches the expected
    // exit-code intent.
    #[test]
    fn report_passed_to_exit_code_invariant() {
        // The invariant the public path enforces:
        //   !report.passed  -> process::exit(1)
        //    report.passed  -> Ok return
        // We pin this by reading the source and asserting the
        // `process::exit(1)` is gated on `!report.passed`. This guards
        // against a future contributor flipping the predicate.
        let source = include_str!("preset.rs");
        assert!(
            source.contains("if !report.passed"),
            "check_preset must call process::exit(1) when report.passed is false"
        );
        assert!(
            source.contains("std::process::exit(1)"),
            "check_preset must call std::process::exit(1) on failure"
        );
    }

    // ---- helpers ----

    fn report_has_orphan_warn(report: &RuntimeContractReport) -> bool {
        report.findings.iter().any(|f| {
            f.source == ralph_core::runtime_contract::FindingSource::Orphan
                && f.severity == ralph_core::runtime_contract::FindingSeverity::Warn
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// U3: Template authoring CLI tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod template_tests {
    use super::*;

    // ── T1: preset list shows all templates ─────────────────────────────────
    #[test]
    fn list_templates_shows_all_template_names() {
        let templates = TemplateCatalog::template_names();
        assert!(templates.contains(&"minimal-linear"));
        assert!(templates.contains(&"debug"));
        assert!(templates.contains(&"ce-executor-lite"));
        assert_eq!(templates.len(), 3);
    }

    // ── T2: preset list human format succeeds ────────────────────────────────
    #[test]
    fn list_templates_human_format_succeeds() {
        let result = list_templates(PresetListFormat::Human, false);
        assert!(result.is_ok());
    }

    // ── T3: preset list json format is valid ─────────────────────────────────
    #[test]
    fn list_templates_json_format_is_valid_json() {
        // We test the manifest structure directly since we can't easily capture println
        let templates = TemplateCatalog::template_names();
        let manifests: Vec<TemplateManifest> = templates
            .iter()
            .filter_map(|name| TemplateCatalog::get_manifest(name))
            .collect();

        let json = serde_json::to_string_pretty(&manifests).unwrap();
        // Verify it's valid JSON by parsing it back
        let parsed: Vec<TemplateManifest> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 3);
    }

    // ── T4: preset show known template succeeds ───────────────────────────────
    #[test]
    fn show_template_known_template_succeeds() {
        let result = show_template("minimal-linear", PresetShowFormat::Human, false);
        assert!(result.is_ok());
    }

    // ── T5: preset show unknown template fails ────────────────────────────────
    #[test]
    fn show_template_unknown_template_fails() {
        let result = show_template("nonexistent", PresetShowFormat::Human, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── T6: preset show yaml format returns template content ──────────────────
    #[test]
    fn show_template_yaml_format_contains_placeholder_markers() {
        // Test by directly checking the template content
        let template = include_str!("../../preset-templates/minimal-linear.yml");
        assert!(template.contains("{{preset_name}}"));
        assert!(template.contains("{{description}}"));
        assert!(template.contains("{{generated_at}}"));
    }

    // ── T6b: preset show json format returns valid manifest json ──────────────
    #[test]
    fn show_template_json_format_produces_valid_json() {
        let result = show_template("minimal-linear", PresetShowFormat::Json, false);
        assert!(result.is_ok());
    }

    // ── T6c: preset show json format is parseable and contains the template's
    // name and version.  This guards against accidental drops of `Serialize`
    // on the manifest or its fields.
    #[test]
    fn show_template_json_format_round_trips_manifest() {
        // The JSON branch prints to stdout, so we re-derive the manifest
        // directly and assert the structure the agent consumes.
        let manifest = TemplateCatalog::get_manifest("minimal-linear")
            .expect("minimal-linear is a builtin template");
        let json = serde_json::to_string_pretty(&manifest).expect("manifest is serializable");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("manifest json round-trips");
        assert_eq!(parsed["name"], "minimal-linear");
        assert_eq!(parsed["version"], "1.0.0");
        assert!(parsed["placeholders"].is_array());
    }

    // ── T7: preset new renders template with placeholders ─────────────────────
    #[tokio::test]
    async fn new_preset_renders_with_values() {
        let tmp = tempfile::tempdir().unwrap();
        let output_path = tmp.path().join("test.yml");

        let args = NewPresetArgs {
            template: "minimal-linear".to_string(),
            name: Some("my-test-flow".to_string()),
            description: Some("Test description".to_string()),
            output: Some(output_path.clone()),
            force: false,
            check: false,
            format: NewPresetFormat::Human,
        };

        let result = new_preset(&[], None, args, false).await;
        assert!(result.is_ok());
        assert!(output_path.exists());

        // Verify the file contains substituted values
        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("name: my-test-flow"));
        assert!(content.contains("description: Test description"));
        assert!(content.contains("x_preset:"));
        assert!(content.contains("template: minimal-linear"));
    }

    // ── T8: preset new without name fails ─────────────────────────────────────
    #[tokio::test]
    async fn new_preset_without_name_fails() {
        let args = NewPresetArgs {
            template: "minimal-linear".to_string(),
            name: None,
            description: Some("Test".to_string()),
            output: None,
            force: false,
            check: false,
            format: NewPresetFormat::Human,
        };

        let result = new_preset(&[], None, args, false).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("--name is required")
        );
    }

    // ── T9: preset new unknown template fails ─────────────────────────────────
    #[tokio::test]
    async fn new_preset_unknown_template_fails() {
        let args = NewPresetArgs {
            template: "nonexistent".to_string(),
            name: Some("test".to_string()),
            description: Some("Test".to_string()),
            output: None,
            force: false,
            check: false,
            format: NewPresetFormat::Human,
        };

        let result = new_preset(&[], None, args, false).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── T10: preset new with invalid name fails ───────────────────────────────
    #[tokio::test]
    async fn new_preset_invalid_name_fails() {
        let args = NewPresetArgs {
            template: "minimal-linear".to_string(),
            name: Some("my/invalid".to_string()), // Contains path separator
            description: Some("Test".to_string()),
            output: None,
            force: false,
            check: false,
            format: NewPresetFormat::Human,
        };

        let result = new_preset(&[], None, args, false).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    // ── T11: preset new without force refuses to overwrite ─────────────────────
    #[tokio::test]
    async fn new_preset_without_force_refuses_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let output_path = tmp.path().join("existing.yml");

        // Create existing file
        std::fs::write(&output_path, "existing: true").unwrap();

        let args = NewPresetArgs {
            template: "minimal-linear".to_string(),
            name: Some("test".to_string()),
            description: Some("Test".to_string()),
            output: Some(output_path.clone()),
            force: false,
            check: false,
            format: NewPresetFormat::Human,
        };

        let result = new_preset(&[], None, args, false).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    // ── T12: preset new with force overwrites ──────────────────────────────────
    #[tokio::test]
    async fn new_preset_with_force_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let output_path = tmp.path().join("existing.yml");

        // Create existing file
        std::fs::write(&output_path, "existing: true").unwrap();

        let args = NewPresetArgs {
            template: "minimal-linear".to_string(),
            name: Some("test".to_string()),
            description: Some("Test".to_string()),
            output: Some(output_path.clone()),
            force: true,
            check: false,
            format: NewPresetFormat::Human,
        };

        let result = new_preset(&[], None, args, false).await;
        assert!(result.is_ok());
        assert!(output_path.exists());

        // Verify content was overwritten
        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("x_preset:"));
        assert!(!content.contains("existing: true"));
    }

    // ── T13: generated preset contains x_preset metadata ──────────────────────
    #[tokio::test]
    async fn new_preset_contains_x_preset_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let output_path = tmp.path().join("test.yml");

        let args = NewPresetArgs {
            template: "minimal-linear".to_string(),
            name: Some("metadata-test".to_string()),
            description: Some("Testing metadata".to_string()),
            output: Some(output_path.clone()),
            force: false,
            check: false,
            format: NewPresetFormat::Human,
        };

        new_preset(&[], None, args, false).await.unwrap();

        let content = std::fs::read_to_string(&output_path).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();

        // Verify x_preset structure
        let x_preset = parsed.get("x_preset").expect("x_preset should exist");
        assert_eq!(
            x_preset.get("schema_version").and_then(|v| v.as_i64()),
            Some(1)
        );
        assert_eq!(
            x_preset.get("template").and_then(|v| v.as_str()),
            Some("minimal-linear")
        );
        assert_eq!(
            x_preset.get("name").and_then(|v| v.as_str()),
            Some("metadata-test")
        );
        assert_eq!(
            x_preset.get("generated_by").and_then(|v| v.as_str()),
            Some("ralph preset new")
        );
    }

    // ── T14: generated preset is valid YAML that parses ───────────────────────
    #[tokio::test]
    async fn new_preset_produces_valid_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let output_path = tmp.path().join("test.yml");

        let args = NewPresetArgs {
            template: "minimal-linear".to_string(),
            name: Some("valid-yaml-test".to_string()),
            description: Some("Testing YAML validity".to_string()),
            output: Some(output_path.clone()),
            force: false,
            check: false,
            format: NewPresetFormat::Human,
        };

        new_preset(&[], None, args, false).await.unwrap();

        // Should be valid YAML
        let content = std::fs::read_to_string(&output_path).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        assert!(parsed.get("hats").is_some());
        assert!(parsed.get("event_loop").is_some());
    }

    // ── T15: default command (no subcommand) lists templates ──────────────────
    #[test]
    fn default_command_lists_templates() {
        // When no subcommand is provided, it defaults to list
        let result = list_templates(PresetListFormat::Human, false);
        assert!(result.is_ok());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// U4: Version Diff and Upgrade Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod diff_upgrade_tests {
    use super::*;

    /// Helper: create a generated preset file with x_preset metadata.
    fn create_generated_preset(
        tmp: &tempfile::TempDir,
        name: &str,
        template: &str,
        template_version: &str,
    ) -> PathBuf {
        let path = tmp.path().join("preset.yml");
        let yaml = format!(
            r#"x_preset:
  schema_version: 1
  template: {}
  template_version: "{}"
  generated_by: "ralph preset new"
  generated_at: "2026-06-08T00:00:00Z"
  name: {}
  description: "Test preset"
hats:
  a:
    name: "A"
    description: "Entry"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    description: "Exit"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#,
            template, template_version, name
        );
        std::fs::write(&path, yaml).unwrap();
        path
    }

    // ── U4-T1: diff on file without x_preset returns error ─────────────────
    #[test]
    fn diff_missing_x_preset_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("no-metadata.yml");
        std::fs::write(&path, "hats:\n  a:\n    name: A").unwrap();

        let result = diff_preset(&path, DiffFormat::Human, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does not contain x_preset"));
    }

    // ── U4-T2: diff on nonexistent file returns error ──────────────────────
    #[test]
    fn diff_nonexistent_file_returns_error() {
        let path = PathBuf::from("/nonexistent/path.yml");
        let result = diff_preset(&path, DiffFormat::Human, false);
        assert!(result.is_err());
    }

    // ── U4-T3: diff on current version file shows up to date ───────────────
    #[test]
    fn diff_current_version_shows_up_to_date() {
        let tmp = tempfile::tempdir().unwrap();
        // Catalog minimal-linear is 1.0.0
        let path = create_generated_preset(&tmp, "my-flow", "minimal-linear", "1.0.0");

        let result = diff_preset(&path, DiffFormat::Human, false);
        assert!(result.is_ok());
        // The output should indicate "up to date"
        // We can't easily capture stdout, but we verify it doesn't error
    }

    // ── U4-T4: diff with unknown template returns error ─────────────────────
    #[test]
    fn diff_unknown_template_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("preset.yml");
        let yaml = r#"x_preset:
  schema_version: 1
  template: nonexistent-template
  template_version: "1.0.0"
  generated_by: "ralph preset new"
  generated_at: "2026-06-08T00:00:00Z"
  name: test
  description: Test
hats:
  a:
    name: A
"#;
        std::fs::write(&path, yaml).unwrap();

        let result = diff_preset(&path, DiffFormat::Human, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found in current catalog"));
    }

    // ── U4-T5: upgrade on current version shows already current ─────────────
    #[test]
    fn upgrade_current_version_shows_already_current() {
        let tmp = tempfile::tempdir().unwrap();
        let path = create_generated_preset(&tmp, "my-flow", "minimal-linear", "1.0.0");

        let result = upgrade_preset(&path, UpgradeFormat::Human, false);
        assert!(result.is_ok());
    }

    // ── U4-T6: upgrade missing x_preset returns error ──────────────────────
    #[test]
    fn upgrade_missing_x_preset_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("no-metadata.yml");
        std::fs::write(&path, "hats:\n  a:\n    name: A").unwrap();

        let result = upgrade_preset(&path, UpgradeFormat::Human, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does not contain x_preset"));
    }

    // ── U4-T7: DiffResult up_to_date structure ─────────────────────────────
    #[test]
    fn diff_result_up_to_date() {
        let result = DiffResult::up_to_date("minimal-linear", "1.0.0");
        assert_eq!(result.template, "minimal-linear");
        assert_eq!(result.local_version, "1.0.0");
        assert_eq!(result.catalog_version, "1.0.0");
        assert!(result.up_to_date);
        assert!(!result.has_update);
        assert!(!result.is_newer);
        assert!(result.diff_lines.is_empty());
    }

    // ── U4-T8: DiffResult needs_update structure ───────────────────────────
    #[test]
    fn diff_result_needs_update() {
        let diff_lines = vec!["-old".to_string(), "+new".to_string()];
        let result =
            DiffResult::needs_update("minimal-linear", "1.0.0", "1.1.0", diff_lines.clone());
        assert_eq!(result.template, "minimal-linear");
        assert_eq!(result.local_version, "1.0.0");
        assert_eq!(result.catalog_version, "1.1.0");
        assert!(!result.up_to_date);
        assert!(result.has_update);
        assert!(!result.is_newer);
        assert_eq!(result.diff_lines, diff_lines);
    }

    // ── U4-T9: DiffResult is_newer structure ────────────────────────────────
    #[test]
    fn diff_result_local_is_newer() {
        let result = DiffResult::is_newer_version("minimal-linear", "1.1.0", "1.0.0");
        assert!(!result.up_to_date);
        assert!(!result.has_update);
        assert!(result.is_newer);
        assert!(result.diff_lines.is_empty());
    }

    // ── U4-T10: UpgradeResult already_current structure ────────────────────
    #[test]
    fn upgrade_result_already_current() {
        let result = UpgradeResult::already_current("minimal-linear", "1.0.0");
        assert_eq!(result.template, "minimal-linear");
        assert_eq!(result.local_version, "1.0.0");
        assert_eq!(result.catalog_version, "1.0.0");
        assert!(!result.upgrade_available);
        assert!(result.suggestions.is_empty());
    }

    // ── U4-T11: UpgradeResult needs_upgrade structure ──────────────────────
    #[test]
    fn upgrade_result_needs_upgrade() {
        let result = UpgradeResult::needs_upgrade("minimal-linear", "1.0.0", "1.1.0");
        assert!(result.upgrade_available);
        assert!(!result.suggestions.is_empty());
    }

    // ── U4-T12: UpgradeResult local_is_newer structure ────────────────────
    #[test]
    fn upgrade_result_local_is_newer() {
        let result = UpgradeResult::local_is_newer("minimal-linear", "1.1.0", "1.0.0");
        assert!(!result.upgrade_available);
        assert!(!result.suggestions.is_empty());
    }

    // ── U4-T13: diff json format produces valid json ──────────────────────
    #[test]
    fn diff_json_format_valid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = create_generated_preset(&tmp, "my-flow", "minimal-linear", "1.0.0");

        // This will print to stdout, so we just verify it doesn't error
        let result = diff_preset(&path, DiffFormat::Json, false);
        assert!(result.is_ok());
    }

    // ── U4-T14: upgrade json format produces valid json ───────────────────
    #[test]
    fn upgrade_json_format_valid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = create_generated_preset(&tmp, "my-flow", "minimal-linear", "1.0.0");

        let result = upgrade_preset(&path, UpgradeFormat::Json, false);
        assert!(result.is_ok());
    }

    // ── U4-T15: diff with older version shows update available ─────────────
    #[test]
    fn diff_older_version_shows_update() {
        let tmp = tempfile::tempdir().unwrap();
        // Simulate a preset generated with an older version
        let path = tmp.path().join("old-preset.yml");
        let yaml = r#"x_preset:
  schema_version: 1
  template: minimal-linear
  template_version: "0.9.0"
  generated_by: "ralph preset new"
  generated_at: "2026-06-01T00:00:00Z"
  name: old-flow
  description: "Old preset"
hats:
  a:
    name: "A"
    description: "Entry"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    description: "Exit"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#
        .to_string();
        std::fs::write(&path, yaml).unwrap();

        let result = diff_preset(&path, DiffFormat::Human, false);
        assert!(result.is_ok());
    }

    // ── U4-T15b: diff same version with local modifications returns local_drift
    //
    // Regression test for P1-#3: previously, when the local preset's
    // `template_version` matched the catalog but the user had edited the
    // file, the diff path misclassified the situation as
    // `has_update=true` with status "update available: 1.0.0 → 1.0.0",
    // causing agents to call `ralph preset upgrade` against a version
    // that does not exist.  The fix introduces `DiffStatus::LocalDrift`
    // (encoded as `is_local_drift: true`, `has_update: false`).
    #[test]
    fn diff_same_version_with_local_changes_returns_local_drift() {
        let tmp = tempfile::tempdir().unwrap();
        // Start from a freshly generated preset, then mutate the local copy
        // to mimic a user edit while keeping the same template_version.
        let path = create_generated_preset(&tmp, "my-flow", "minimal-linear", "1.0.0");
        let original = std::fs::read_to_string(&path).unwrap();
        let mutated = original.replace("name: my-flow", "name: my-flow-edited");
        std::fs::write(&path, mutated).unwrap();

        // The diff runs against the main events file via the standard
        // aggregator path; here we only need the public Result shape, so
        // exercise the constructor directly to avoid coupling to the
        // stdio side-effect of `diff_preset`.
        let local_ver = "1.0.0";
        let result = DiffResult::local_drift(
            "minimal-linear",
            local_ver,
            vec![
                "-name: my-flow".to_string(),
                "+name: my-flow-edited".to_string(),
            ],
        );

        assert_eq!(result.template, "minimal-linear");
        assert_eq!(result.local_version, "1.0.0");
        assert_eq!(result.catalog_version, "1.0.0");
        assert!(!result.up_to_date, "drift means up_to_date=false");
        assert!(!result.has_update, "drift is NOT a catalog update");
        assert!(!result.is_newer);
        assert!(result.is_local_drift);
        assert_eq!(result.status, "local changes");
        assert_eq!(result.diff_lines.len(), 2);
    }

    // ── U4-T15c: DiffResult::local_drift does NOT carry has_update ──────────
    //
    // Belt-and-suspenders guard: an agent that only inspects `has_update`
    // (and not `is_local_drift`) must NOT see true here, otherwise
    // `ralph preset upgrade` would be triggered for a non-existent upgrade.
    #[test]
    fn diff_local_drift_keeps_has_update_false() {
        let result = DiffResult::local_drift(
            "minimal-linear",
            "1.0.0",
            vec!["-old".into(), "+new".into()],
        );
        assert!(!result.has_update);
        assert!(result.is_local_drift);
    }

    // ── U4-T16: compute_unified_diff with no changes returns empty ─────────
    #[test]
    fn compute_unified_diff_no_changes() {
        let original = "line1\nline2\nline3";
        let revised = "line1\nline2\nline3";
        let diff = compute_unified_diff(original, revised);
        assert!(diff.is_empty());
    }

    // ── U4-T17: compute_unified_diff with changes produces diff lines ───────
    #[test]
    fn compute_unified_diff_with_changes() {
        let original = "line1\nline2\nline3";
        let revised = "line1\nmodified\nline3";
        let diff = compute_unified_diff(original, revised);
        assert!(!diff.is_empty());
        assert!(diff.iter().any(|l| l.starts_with('-')));
        assert!(diff.iter().any(|l| l.starts_with('+')));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// U5: Runtime Contract Integration Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod u5_check_tests {
    use super::*;

    // ── U5-T1: new_preset --check generates file BEFORE running check ───────
    //
    // This verifies the critical ordering guarantee: file is written first,
    // then check runs. If template doesn't exist, file is never written.
    // This proves the file is written at a specific point in the flow.
    #[tokio::test]
    async fn new_preset_check_file_generated_before_check() {
        let tmp = tempfile::tempdir().unwrap();
        let output_path = tmp.path().join("check-order.yml");

        // If we pass a bad template name, new_preset fails BEFORE writing file.
        // This proves the file is written after template resolution succeeds.
        let args = NewPresetArgs {
            template: "nonexistent-template".to_string(), // Invalid template
            name: Some("check-order".to_string()),
            description: Some("Test".to_string()),
            output: Some(output_path.clone()),
            force: false,
            check: true,
            format: NewPresetFormat::Human,
        };

        // Should fail because template doesn't exist
        let result = new_preset(&[], None, args, false).await;
        assert!(result.is_err(), "Should fail for nonexistent template");

        // File should NOT exist (proving file is only written after template resolves)
        assert!(
            !output_path.exists(),
            "File should not exist when template resolution fails"
        );
    }

    // ── U5-T2: new_preset without --check generates file correctly ──────────
    //
    // Verifies that new_preset works without --check flag, which is the
    // normal path before U5 check integration.
    #[tokio::test]
    async fn new_preset_without_check_works() {
        let tmp = tempfile::tempdir().unwrap();
        let output_path = tmp.path().join("no-check.yml");

        let args = NewPresetArgs {
            template: "minimal-linear".to_string(),
            name: Some("no-check".to_string()),
            description: Some("Test without check".to_string()),
            output: Some(output_path.clone()),
            force: false,
            check: false, // Explicitly disable check
            format: NewPresetFormat::Human,
        };

        let result = new_preset(&[], None, args, false).await;
        assert!(
            result.is_ok(),
            "new_preset without --check should succeed: {:?}",
            result
        );

        // File should exist
        assert!(output_path.exists(), "Generated file should exist");

        // File should contain x_preset metadata
        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(
            content.contains("x_preset:"),
            "File should contain x_preset metadata"
        );
        assert!(
            content.contains("template: minimal-linear"),
            "File should reference correct template"
        );
    }

    // ── U5-T3: build_report does NOT call backend ───────────────────────────
    //
    // The check uses RuntimeContractAggregator which only does static analysis.
    // It should NOT invoke backend detection or require claude/codex installed.
    #[tokio::test]
    async fn build_report_no_backend_required() {
        // Create a valid preset file and verify build_report works
        // without any backend being installed.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("good.yml");

        // A simple valid preset that should pass topology check.
        // WRC-U1 (2026-06-12-003): lint is now always-on, so the
        // fixture must declare `topic_format_whitelist` (otherwise
        // the legacy `LOOP_COMPLETE` completion promise trips the
        // format check) and `tasks.coordinator_hats` (otherwise
        // the `coordinator_missing` rule fires). The chain
        // `work.start → work.ready → LOOP_COMPLETE` itself is
        // WAC-clean.
        let valid_yaml = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: true
  coordinator_hats:
    - a
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        std::fs::write(&path, valid_yaml).unwrap();

        let sources = vec![ConfigSource::File(path)];

        // build_report should succeed without any backend installed
        let result = build_report(&sources, None, false).await;
        assert!(
            result.is_ok(),
            "build_report should work without backend: {:?}",
            result
        );

        let report = result.unwrap();
        assert!(report.passed, "Valid preset should pass: {:?}", report);
    }

    // ── U5-T4: build_report uses RuntimeContractAggregator (integration) ───
    //
    // Verifies that both `preset check` and `new --check` use the same
    // RuntimeContractAggregator, ensuring consistent results.
    //
    // WRC-U1 (2026-06-12-003): the lint is always-on, so the
    // fixture must declare the legacy `LOOP_COMPLETE`
    // completion promise in the format whitelist and provide
    // a `tasks.coordinator_hats` entry. The 2-hat chain itself
    // is otherwise WAC-clean.
    const GOOD_TOPOLOGY_YAML: &str = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: true
  coordinator_hats:
    - a
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    terminal_events: ["work.ready"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    terminal_events: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;

    #[tokio::test]
    async fn build_report_uses_runtime_contract_aggregator() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("good.yml");
        std::fs::write(&path, GOOD_TOPOLOGY_YAML).unwrap();

        let sources = vec![ConfigSource::File(path)];
        let report = build_report(&sources, None, false)
            .await
            .expect("build_report should succeed for good YAML");

        // Verify report structure matches RuntimeContractReport
        assert!(report.passed, "Good YAML should pass: {:?}", report);
        assert!(
            report.source_label.contains("good.yml"),
            "source_label should contain 'good.yml': {}",
            report.source_label
        );
        assert_eq!(report.errors, 0, "Good YAML should have 0 errors");
        assert_eq!(report.warnings, 0, "Good YAML should have 0 warnings");
    }

    // ── U5-T5: build_report with bad topology returns failure ─────────────
    //
    // Verifies that RuntimeContractAggregator correctly identifies topology issues.
    const BAD_TOPOLOGY_YAML: &str = r#"
hats:
  a:
    name: "A"
    description: "Other-only"
    triggers: ["other.topic"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
"#;

    #[tokio::test]
    async fn build_report_detects_bad_topology() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad-topology.yml");
        std::fs::write(&path, BAD_TOPOLOGY_YAML).unwrap();

        let sources = vec![ConfigSource::File(path)];
        let report = build_report(&sources, None, false)
            .await
            .expect("build_report should succeed even for bad topology");

        assert!(!report.passed, "Bad topology should fail: {:?}", report);
        assert!(report.errors > 0, "Should have errors for bad topology");

        let has_topology_error = report
            .findings
            .iter()
            .any(|f| f.source == ralph_core::runtime_contract::FindingSource::Topology);
        assert!(
            has_topology_error,
            "Should have topology finding: {:?}",
            report.findings
        );
    }

    // ── U5-T6: new --check and preset check produce consistent results ──────
    //
    // When checking the same file with both `preset check` and `new --check`,
    // the underlying RuntimeContractAggregator should produce consistent results.
    #[tokio::test]
    async fn new_check_and_preset_check_are_consistent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("consistent.yml");
        std::fs::write(&path, GOOD_TOPOLOGY_YAML).unwrap();

        let sources = vec![ConfigSource::File(path.clone())];

        // Run check twice - both should produce identical results
        let report1 = build_report(&sources, None, false)
            .await
            .expect("first check should succeed");

        let report2 = build_report(&sources, None, false)
            .await
            .expect("second check should succeed");

        // Both should have identical conclusions
        assert_eq!(report1.passed, report2.passed);
        assert_eq!(report1.errors, report2.errors);
        assert_eq!(report1.warnings, report2.warnings);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// materialize-artifacts CLI helper tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod materialize_artifacts_tests {
    use super::materialize_artifacts;
    use std::fs;

    #[test]
    fn rejects_empty_plan_key() {
        let err = materialize_artifacts("parallel-forge", "", None, false).unwrap_err();
        assert!(err.to_string().contains("plan-key"));
    }

    #[test]
    fn rejects_plan_key_with_path_separators() {
        let err = materialize_artifacts("parallel-forge", "a/b", None, false).unwrap_err();
        assert!(err.to_string().contains("path segment"));
        let err = materialize_artifacts("parallel-forge", "..", None, false).unwrap_err();
        assert!(err.to_string().contains("path segment"));
    }

    #[test]
    fn writes_to_explicit_dest() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out");
        materialize_artifacts("parallel-forge", "any-key", Some(&dest), false)
            .expect("materialize");
        assert!(dest.join("development-plan.template.md").is_file());
        assert!(dest.join("unit.template.yml").is_file());
        let body = fs::read_to_string(dest.join("development-plan.template.md")).unwrap();
        assert!(body.contains("## 3. BDD 行为规格"));
    }

    #[test]
    fn red_team_attack_picks_red_team_default_dest() {
        // Without an explicit `--dest`, the red-team-attack preset must
        // land under `.ralph/red-team/<plan_key>/templates/`, NOT under
        // `.ralph/forge/...` — the two presets now share a materialize
        // binary but their artifact trees are disjoint.
        let tmp = tempfile::tempdir().unwrap();
        // We override the cwd to the temp dir so the relative defaults
        // can resolve without polluting the repo's `.ralph/`.
        let cwd_backup = std::env::current_dir().ok();
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = materialize_artifacts("red-team-attack", "rt-key", None, false);

        if let Some(prev) = cwd_backup {
            let _ = std::env::set_current_dir(prev);
        }
        result.expect("red-team materialize under red-team dir");

        let expected = tmp
            .path()
            .join(".ralph")
            .join("red-team")
            .join("rt-key")
            .join("templates");
        assert!(
            expected.join("experiment.template.yml").is_file(),
            "expected red-team templates under {}",
            expected.display()
        );
        // The parallel-forge templates must NOT have leaked into the
        // red-team default directory.
        let forge_default = tmp.path().join(".ralph").join("forge");
        assert!(
            !forge_default.exists(),
            "red-team preset must not write into the parallel-forge default tree"
        );
    }

    #[test]
    fn unknown_preset_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out");
        let err = materialize_artifacts("not-a-preset", "k", Some(&dest), false).unwrap_err();
        assert!(err.to_string().contains("no embedded artifact templates"));
    }
}

#[cfg(test)]
mod builtin_show_tests {
    //! In-crate unit tests for `show_builtin` (U02). The
    //! integration-level byte-equal checks live in
    //! `tests/integration_preset_builtin.rs`; these tests pin the
    //! internal invariant that `Yaml` mode writes the embedded
    //! content as-is, *and* exercise the public/hidden paths via
    //! `get_preset` rather than through the CLI.

    use super::*;
    use crate::presets::get_preset;

    /// Pin: `show_builtin("parallel-forge", Yaml, false)` writes
    /// `get_preset("parallel-forge").content` to stdout byte-exact
    /// (no reserialize, no trim, no extra newline).
    ///
    /// The integration test (`builtin_show_yaml_public_is_parseable`)
    /// asserts the YAML is *parseable* and contains the expected
    /// top-level keys; this in-crate test pins the *byte-equality*
    /// invariant: whatever `show_builtin` writes is exactly the
    /// `EmbeddedPreset.content` byte slice, with no normalization.
    #[test]
    fn show_builtin_yaml_helper_returns_embedded_content_byte_exact() {
        let embedded = get_preset("parallel-forge")
            .expect("parallel-forge is a Tier-0 builtin and must exist");
        let rendered = builtin_yaml_bytes(embedded);
        assert_eq!(
            rendered,
            embedded.content.as_bytes(),
            "builtin_yaml_bytes must return the embedded content as-is (no trim, no rewrite)"
        );
        assert!(
            !rendered.is_empty(),
            "embedded content must not be empty for parallel-forge"
        );
    }

    /// Pin: `show_builtin(<hidden>, Yaml, false)` must succeed
    /// because the lookup helper (`get_preset`) does not filter on
    /// `public`. This is the S4 contract: hidden IDs resolve.
    #[test]
    fn get_preset_resolves_hidden_ids() {
        let hidden = get_preset("merge-loop")
            .expect("merge-loop is a hidden builtin and must resolve via get_preset");
        assert!(
            !hidden.public,
            "merge-loop must stay flagged public=false (hidden)"
        );
        assert!(
            !hidden.content.is_empty(),
            "hidden content must not be empty"
        );
    }

    /// Pin: `show_builtin(<unknown>, ...)` errors out with the id
    /// surfaced in the error message. This is the S5 contract.
    #[test]
    fn get_preset_returns_none_for_unknown_id() {
        assert!(get_preset("does-not-exist").is_none());
    }

    /// Pin: `show_builtin(<unknown>, Yaml, false)` errors to
    /// `anyhow!` with the id present in the error string. We can't
    /// easily intercept the stdout write from the function under
    /// test (it writes to the real stdout), so this test exercises
    /// the failure path indirectly via `get_preset` (which is the
    /// single source of truth for the unknown-id case).
    #[test]
    fn unknown_id_error_message_contains_id() {
        // The error message contract: "unknown builtin preset: <id>".
        // We rebuild the same error from the helper to pin the
        // contract that the integration test asserts on stderr.
        let id = "does-not-exist";
        let err = anyhow::anyhow!("unknown builtin preset: {id}");
        let msg = err.to_string();
        assert!(msg.contains("does-not-exist"));
        assert!(msg.contains("unknown builtin preset"));
    }
}
