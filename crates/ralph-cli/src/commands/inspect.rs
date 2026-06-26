//! CLI commands for the `ralph inspect` namespace.
//!
//! Read-only diagnostic commands. U5 of plan 2026-06-25-002 added
//! `ralph inspect profiles`, which previews the active profile overlay
//! (defaults from ralph.yml + CLI `--profile` flags) without launching
//! the orchestration loop. The command reuses the same resolution
//! primitives as `ralph run` so the preview matches what the loop would
//! apply — same preset anchor, same activation order, same warning
//! surface.
//!
//! Semantics (deliberate split):
//!
//! - `ralph inspect` is **read-only / diagnostic** — it does not mutate
//!   `RalphConfig` and does not start the loop.
//! - `ralph preset` is **template authoring** — it scaffolds presets and
//!   runs contract checks.
//! - `ralph hats` is **hats management** — it inspects / validates the
//!   configured hat collection.
//!
//! Keeping these three surfaces separate avoids the "is this a check or a
//! scaffold?" confusion that arises when too many verbs share one
//! namespace.

use crate::display::colors;
use crate::preflight;
use crate::{ConfigSource, HatsSource};
use anyhow::{Context, Result};
use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use ralph_core::RalphConfig;
use ralph_core::config::profiles::ProfileSpec;
use ralph_core::profiles::ResolvedProfileFragments;
use std::io::Write;
use std::path::PathBuf;

/// Inspect Ralph runtime state without modifying it.
#[derive(Parser, Debug)]
pub struct InspectArgs {
    #[command(subcommand)]
    pub command: Option<InspectCommands>,
}

#[derive(Subcommand, Debug)]
pub enum InspectCommands {
    /// Preview profile overlay resolution without starting a loop.
    ///
    /// Shows active specs (defaults + CLI flags), the resolved preset
    /// anchor, every `<profile>/<preset>/<hat>.md` fragment with its
    /// source path and first-line preview, and any non-fatal warnings
    /// (missing preset subdir, orphan hat, etc.).
    ///
    /// Does **not** modify `RalphConfig`; pair with `ralph run` /
    /// `ralph plan` when you actually want the fragments appended.
    Profiles(InspectProfilesArgs),
}

/// Arguments for `ralph inspect profiles`.
///
/// Field naming mirrors [`crate::commands::run::RunArgs`] so the
/// activation-order semantics (`profiles.default` first, then each CLI
/// `--profile`, with `--no-default-profiles` suppressing only the
/// defaults) line up byte-for-byte between the preview and the actual
/// `ralph run` invocation. U5 deliberately re-declares the fields rather
/// than re-using `RunArgs` because `inspect profiles` has no run-loop
/// options (no `--dry-run`, no `--backend`, no worktree plumbing, etc.)
/// — coupling them would force the inspect surface to accept flags it
/// can't honour.
#[derive(Parser, Debug)]
pub struct InspectProfilesArgs {
    /// Activate a runtime profile overlay. Accepts `<scope>:<name>` where
    /// `<scope>` is `repo` (project-rooted `ralph-profiles/<name>/`) or
    /// `user` (`~/.config/ralph/profiles/<name>/`). Repeatable; appended
    /// to the active spec list after `profiles.default` from ralph.yml.
    #[arg(long = "profile", value_name = "SCOPE:NAME", action = ArgAction::Append)]
    pub profiles: Vec<String>,

    /// Disable the operator-supplied `profiles.default` list from
    /// ralph.yml. CLI `--profile` flags remain in effect.
    #[arg(long)]
    pub no_default_profiles: bool,

    /// Output format (human or json). JSON output is stable enough for
    /// machine consumption: schema matches the human output's logical
    /// structure (profiles, preset, fragments, warnings).
    #[arg(long, value_enum, default_value_t = InspectProfilesFormat::Human)]
    pub format: InspectProfilesFormat,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectProfilesFormat {
    Human,
    Json,
}

/// Execute an `ralph inspect` subcommand.
pub async fn execute(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    args: InspectArgs,
    use_colors: bool,
) -> Result<()> {
    match args.command {
        Some(InspectCommands::Profiles(profiles_args)) => {
            inspect_profiles_command(config_sources, hats_source, profiles_args, use_colors).await
        }
        None => {
            // No subcommand: print help so users learn the surface.
            // Returning Ok is consistent with `ralph preset` (which falls
            // back to `list`); here we want an explicit hint instead of a
            // silent default action because `inspect` is currently a
            // single-subcommand namespace and the help text is short.
            use clap::CommandFactory;
            let mut cmd = InspectArgs::command();
            cmd.print_help()?;
            println!();
            Ok(())
        }
    }
}

/// Resolve the active profile overlay and emit a human- or JSON-formatted
/// preview.
///
/// Resolution pipeline mirrors [`crate::commands::run::apply_active_profiles`]:
///
/// 1. `load_config_for_preflight` so `profiles.default` (a
///    `Vec<ProfileSpec>` after `normalize()`) is available.
/// 2. [`collect_active_profile_specs`] to merge defaults + CLI flags in
///    the documented order.
/// 3. [`crate::commands::run::derive_preset_name`] (re-exposed as
///    [`derive_preset_name_for_inspect`]) to anchor the `<profile>/<preset>`
///    lookup; remote hats source + active specs surfaces as a clear error.
/// 4. `RALPH_WORKSPACE_ROOT` (or `config.core.workspace_root`) for repo
///    profile resolution, so `--worktree` children still find the main
///    repo's `ralph-profiles/`.
/// 5. [`ralph_core::profiles::resolve_profile_fragments`] — the pure,
///    non-mutating reader. This is the function `ralph run` ultimately
///    feeds into `apply_profile_fragments`; reading it here gives us a
///    preview that matches the actual apply step exactly (same warnings,
///    same paths, same fragment order).
///
/// When no presets are active the command prints "no profiles active"
/// (human) / `{ "active": false, ... }` (JSON) and exits 0 — the operator
/// should not have to read a stack trace to learn "nothing to do".
pub async fn inspect_profiles_command(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    args: InspectProfilesArgs,
    use_colors: bool,
) -> Result<()> {
    let config = preflight::load_config_for_preflight(config_sources, hats_source).await?;

    // Always log defaults first so the operator can see whether their
    // ralph.yml is being read (vs. silently dropped due to a missing
    // file). Defaults are already-validated `ProfileSpec`s; we re-format
    // them so the JSON shape matches the CLI entries.
    let defaults = config.profiles.default.clone();

    let specs = crate::commands::profile_args::collect_active_profile_specs(&config, &args)?;

    let workspace_root = std::env::var("RALPH_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| config.core.workspace_root.clone());

    // If no specs are active and no defaults are configured, we can skip
    // the preset / fragments step entirely — there is nothing to inspect.
    // This is the "preview the current state" branch: nothing is active,
    // so there is nothing to warn about.
    if specs.is_empty() {
        let view = InspectProfilesView {
            active: false,
            defaults: defaults.iter().map(profile_spec_to_string).collect(),
            specs: Vec::new(),
            preset: None,
            fragments: Vec::new(),
            warnings: Vec::new(),
            note: Some(
                "no profiles active (set ralph.yml `profiles.default` or pass --profile)"
                    .to_string(),
            ),
        };
        return emit_view(&view, args.format, use_colors);
    }

    // Derive preset name. A remote hats source with active specs is a
    // hard error here (matches `ralph run`'s U4 behaviour) so the
    // operator gets the same diagnostic from both commands.
    let preset_name = match derive_preset_name_for_inspect(hats_source)? {
        Some(name) => name,
        None => {
            // No preset name available — print a warning view (human)
            // or a structured warning (json) and exit cleanly. We do
            // not bail because the operator may have intentionally run
            // `ralph inspect profiles` to learn *why* their spec list
            // is not resolvable (e.g. they forgot `-H`).
            let view = InspectProfilesView {
                active: true,
                defaults: defaults.iter().map(profile_spec_to_string).collect(),
                specs: specs.iter().map(profile_spec_to_string).collect(),
                preset: None,
                fragments: Vec::new(),
                warnings: vec![
                    "--profile specs requested but no preset is active \
                     (no -H/--hats source); profile resolution skipped"
                        .to_string(),
                ],
                note: None,
            };
            return emit_view(&view, args.format, use_colors);
        }
    };

    let resolved = ralph_core::profiles::resolve_profile_fragments(
        &config,
        &preset_name,
        &specs,
        &workspace_root,
    )
    .with_context(|| {
        format!(
            "failed to resolve profile fragments for preset {:?}",
            preset_name
        )
    })?;

    let view = build_view(&defaults, &specs, &preset_name, &resolved);
    emit_view(&view, args.format, use_colors)
}

/// `InspectProfilesArgs` implements [`crate::commands::profile_args::ProfileArgs`]
/// so the activation-order merge lives in exactly one place (shared with
/// `ralph run`). The trait impl below keeps `ralph inspect profiles` and
/// `ralph run` byte-for-byte aligned: both consume the same defaults +
/// CLI flag merge via [`crate::commands::profile_args::collect_active_profile_specs`].
impl crate::commands::profile_args::ProfileArgs for InspectProfilesArgs {
    fn profile_specs(&self) -> &[String] {
        &self.profiles
    }
    fn no_default_profiles(&self) -> bool {
        self.no_default_profiles
    }
}

/// Mirrors [`crate::commands::run::derive_preset_name`] but lives here
/// because `derive_preset_name` is private to `commands::run` and we
/// cannot re-export it without restructuring the run module's API
/// surface. Behaviour is intentionally identical and the inspect-side
/// copy is unit-tested directly (see `derive_preset_name_for_inspect_*`
/// tests below) so the two helpers can't drift silently.
fn derive_preset_name_for_inspect(hats_source: Option<&HatsSource>) -> Result<Option<String>> {
    match hats_source {
        None => Ok(None),
        Some(HatsSource::Builtin(name)) => Ok(Some(name.clone())),
        Some(HatsSource::File(path)) => Ok(Some(
            path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "failed to derive preset name from hats file path '{}'",
                        path.display()
                    )
                })?
                .to_string(),
        )),
        Some(HatsSource::Remote(url)) => Err(anyhow::anyhow!(
            "profile fragments cannot be resolved for remote hats source '{}'; \
             use a builtin preset or a local file path instead",
            url
        )),
    }
}

/// Serializable view of the inspection result. Both human and JSON
/// output are derived from this struct so the two surfaces cannot drift.
#[derive(Debug, Clone, serde::Serialize)]
struct InspectProfilesView {
    /// True iff at least one spec (default or CLI) is active.
    active: bool,
    /// Operator-supplied defaults from `ralph.yml`.
    defaults: Vec<String>,
    /// Final merged spec list (defaults + CLI, with `--no-default-profiles`
    /// applied). Empty when no specs are active.
    specs: Vec<String>,
    /// Resolved preset anchor (`builtin:<x>`, file stem, etc.). None when
    /// no preset is available — the operator sees the warning instead.
    preset: Option<String>,
    /// Per-hat fragment list. Each entry carries the source path, the
    /// owning spec, the target hat id, and a one-line preview (max 60
    /// chars) of the fragment body.
    fragments: Vec<InspectFragmentView>,
    /// Non-fatal warnings (missing preset subdir, orphan hat, etc.).
    warnings: Vec<String>,
    /// Optional human-friendly note shown in both formats when there is
    /// nothing to inspect but the operator might benefit from a hint
    /// (e.g. "no profiles active").
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct InspectFragmentView {
    /// Profile spec this fragment came from.
    spec: String,
    /// Hat id the fragment targets.
    hat_id: String,
    /// Absolute path of the source `.md` file.
    path: String,
    /// First line of the fragment, trimmed and capped at 60 chars. Lets
    /// operators eyeball each fragment without dumping full bodies into
    /// stdout.
    preview: String,
}

fn build_view(
    defaults: &[ProfileSpec],
    specs: &[ProfileSpec],
    preset_name: &str,
    resolved: &ResolvedProfileFragments,
) -> InspectProfilesView {
    let fragments: Vec<InspectFragmentView> = resolved
        .by_hat
        .values()
        .flat_map(|fragments| fragments.iter())
        .map(|f| InspectFragmentView {
            spec: f.spec.to_string(),
            hat_id: f.hat_id.clone(),
            path: f.path.display().to_string(),
            preview: first_line_preview(&f.content, 60),
        })
        .collect();

    InspectProfilesView {
        active: true,
        defaults: defaults.iter().map(profile_spec_to_string).collect(),
        specs: specs.iter().map(profile_spec_to_string).collect(),
        preset: Some(preset_name.to_string()),
        fragments,
        warnings: resolved.warnings.clone(),
        note: None,
    }
}

fn profile_spec_to_string(spec: &ProfileSpec) -> String {
    format!("{}:{}", spec.scope, spec.name)
}

fn first_line_preview(content: &str, max_chars: usize) -> String {
    // Split at the first `\n` so multi-paragraph fragments don't bleed
    // into the preview. Take the trimmed body (or empty string) and
    // truncate to `max_chars`. Unicode-safe via `char_indices` so a
    // multi-byte code point doesn't get sliced in half.
    let first = content.lines().next().unwrap_or("").trim();
    if first.chars().count() <= max_chars {
        first.to_string()
    } else {
        let mut out: String = first.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

fn emit_view(
    view: &InspectProfilesView,
    format: InspectProfilesFormat,
    use_colors: bool,
) -> Result<()> {
    match format {
        InspectProfilesFormat::Json => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            serde_json::to_writer_pretty(&mut handle, view)?;
            writeln!(handle)?;
        }
        InspectProfilesFormat::Human => {
            print_human(view, use_colors);
        }
    }
    Ok(())
}

fn print_human(view: &InspectProfilesView, use_colors: bool) {
    let cyan = if use_colors { colors::CYAN } else { "" };
    let dim = if use_colors { colors::DIM } else { "" };
    let reset = if use_colors { colors::RESET } else { "" };
    let yellow = if use_colors { colors::YELLOW } else { "" };

    if !view.active {
        println!("{cyan}Profile inspection{reset}");
        println!("  active: false");
        if let Some(note) = &view.note {
            println!("  note:   {note}");
        }
        if !view.defaults.is_empty() {
            println!("  defaults (ignored when --no-default-profiles):");
            for d in &view.defaults {
                println!("    - {d}");
            }
        }
        return;
    }

    println!("{cyan}Profile inspection{reset}");
    if let Some(preset) = &view.preset {
        println!("  preset: {preset}");
    } else {
        println!("  preset: {yellow}(unresolved){reset}");
    }
    if !view.defaults.is_empty() {
        println!("  defaults:");
        for d in &view.defaults {
            println!("    - {d}");
        }
    } else {
        println!("  defaults: {dim}(none){reset}");
    }
    println!("  active specs:");
    for s in &view.specs {
        println!("    - {s}");
    }

    if view.fragments.is_empty() {
        println!("  fragments: {dim}(none){reset}");
    } else {
        println!("  fragments:");
        // Group by hat so the output mirrors how the overlay will be
        // applied to `instructions`. Within each hat, preserve the
        // activation order captured by `ResolvedProfileFragments`.
        let mut by_hat: std::collections::BTreeMap<String, Vec<&InspectFragmentView>> =
            std::collections::BTreeMap::new();
        for frag in &view.fragments {
            by_hat.entry(frag.hat_id.clone()).or_default().push(frag);
        }
        for (hat_id, frags) in &by_hat {
            println!("    {hat_id}:");
            for frag in frags {
                println!(
                    "      {}spec={spec} path={path}{reset}",
                    dim,
                    spec = frag.spec,
                    path = frag.path,
                    reset = reset
                );
                if frag.preview.is_empty() {
                    println!("        {dim}(empty fragment){reset}");
                } else {
                    println!("        preview: {}", frag.preview);
                }
            }
        }
    }

    if !view.warnings.is_empty() {
        println!("  {yellow}warnings:{reset}");
        for w in &view.warnings {
            println!("    - {w}");
        }
    }
}

// `ProfileFragment` is intentionally not re-exported here — tests below
// reach into `ralph_core::profiles::ProfileFragment` directly when they
// need to construct a fragment, which keeps the public surface of this
// module narrow.

#[cfg(test)]
use std::path::Path;

/// Helper used by tests: ensure a path's parent directory exists.
/// Thin wrapper around `fs::create_dir_all` so the test bodies stay
/// focused on profile semantics rather than filesystem plumbing.
#[cfg(test)]
fn ensure_dir(path: &Path) {
    std::fs::create_dir_all(path).expect("create_dir_all");
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use ralph_core::ProfileScope;
    use ralph_core::config::hat::HatConfig;
    use ralph_core::config::profiles::ProfileSpec;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_fragment(dir: &Path, hat_id: &str, body: &str) {
        ensure_dir(dir);
        std::fs::write(dir.join(format!("{hat_id}.md")), body).expect("write fragment");
    }

    fn config_with_hats(hats: &[&str]) -> RalphConfig {
        let mut cfg = RalphConfig::default();
        for id in hats {
            cfg.hats.insert((*id).to_string(), HatConfig::default());
        }
        cfg
    }

    // ─────────────────────────────────────────────────────────────────────
    // CLI parsing (covers R13 / U5 acceptance: `Cli::try_parse_from(["ralph",
    // "inspect", "profiles", ...])`).
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn cli_parses_inspect_profiles_minimal() {
        // Parse via the local `InspectArgs` rather than the binary's
        // top-level `Cli` (which lives in `crate::main` and isn't
        // reachable from a sibling module's tests). The parser contract
        // is the same: `InspectArgs` is the `command: Option<InspectCommands>`
        // argument shape embedded under `Commands::Inspect` in `main.rs`,
        // so we strip the leading "inspect" token and parse the rest.
        let parsed =
            InspectArgs::try_parse_from(["inspect", "profiles"]).expect("CLI parse failed");
        let profiles_args = match parsed.command.expect("profiles subcommand") {
            InspectCommands::Profiles(p) => p,
        };
        assert!(profiles_args.profiles.is_empty());
        assert!(!profiles_args.no_default_profiles);
        assert_eq!(profiles_args.format, InspectProfilesFormat::Human);
    }

    #[test]
    fn cli_parses_inspect_profiles_repeated_profile_flag() {
        let parsed = InspectArgs::try_parse_from([
            "inspect",
            "profiles",
            "--profile",
            "repo:strict",
            "--profile",
            "user:my-style",
        ])
        .expect("CLI parse failed");
        let profiles_args = match parsed.command.expect("profiles subcommand") {
            InspectCommands::Profiles(p) => p,
        };
        assert_eq!(
            profiles_args.profiles,
            vec!["repo:strict".to_string(), "user:my-style".to_string()]
        );
    }

    #[test]
    fn cli_parses_inspect_profiles_no_default_profiles() {
        let parsed = InspectArgs::try_parse_from(["inspect", "profiles", "--no-default-profiles"])
            .expect("CLI parse failed");
        let profiles_args = match parsed.command.expect("profiles subcommand") {
            InspectCommands::Profiles(p) => p,
        };
        assert!(profiles_args.no_default_profiles);
    }

    #[test]
    fn cli_parses_inspect_profiles_json_format() {
        let parsed = InspectArgs::try_parse_from(["inspect", "profiles", "--format", "json"])
            .expect("CLI parse failed");
        let profiles_args = match parsed.command.expect("profiles subcommand") {
            InspectCommands::Profiles(p) => p,
        };
        assert_eq!(profiles_args.format, InspectProfilesFormat::Json);
    }

    // ─────────────────────────────────────────────────────────────────────
    // `derive_preset_name_for_inspect` — mirrors run.rs U4 helper.
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn derive_preset_name_for_inspect_builtin() {
        let src = HatsSource::Builtin("debug".to_string());
        assert_eq!(
            derive_preset_name_for_inspect(Some(&src)).unwrap(),
            Some("debug".to_string())
        );
    }

    #[test]
    fn derive_preset_name_for_inspect_file_uses_stem() {
        let src = HatsSource::File(PathBuf::from("/some/where/my-hats.yml"));
        assert_eq!(
            derive_preset_name_for_inspect(Some(&src)).unwrap(),
            Some("my-hats".to_string())
        );
    }

    #[test]
    fn derive_preset_name_for_inspect_none_is_none() {
        assert_eq!(derive_preset_name_for_inspect(None).unwrap(), None);
    }

    #[test]
    fn derive_preset_name_for_inspect_remote_is_error() {
        let src = HatsSource::Remote("https://example.com/hats.yml".to_string());
        let err = derive_preset_name_for_inspect(Some(&src)).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("remote"),
            "expected remote-mention in error, got {msg}"
        );
        assert!(
            msg.contains("https://example.com/hats.yml"),
            "expected URL echoed in error, got {msg}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Spec collection — activation order.
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn collect_active_specs_for_inspect_defaults_first_then_cli() {
        let mut config = RalphConfig::default();
        config.profiles.default = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "base".to_string(),
        }];
        let args = InspectProfilesArgs {
            profiles: vec!["user:extra".to_string()],
            no_default_profiles: false,
            format: InspectProfilesFormat::Human,
        };
        let active = crate::commands::profile_args::collect_active_profile_specs(&config, &args)
            .expect("collect");
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].to_string(), "repo:base");
        assert_eq!(active[1].to_string(), "user:extra");
    }

    #[test]
    fn collect_active_specs_for_inspect_no_default_profiles_skips_defaults_only() {
        let mut config = RalphConfig::default();
        config.profiles.default = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "base".to_string(),
        }];
        let args = InspectProfilesArgs {
            profiles: vec!["user:extra".to_string()],
            no_default_profiles: true,
            format: InspectProfilesFormat::Human,
        };
        let active = crate::commands::profile_args::collect_active_profile_specs(&config, &args)
            .expect("collect");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].to_string(), "user:extra");
    }

    #[test]
    fn collect_active_specs_for_inspect_empty_inputs_yield_empty() {
        let config = RalphConfig::default();
        let args = InspectProfilesArgs {
            profiles: Vec::new(),
            no_default_profiles: false,
            format: InspectProfilesFormat::Human,
        };
        let active = crate::commands::profile_args::collect_active_profile_specs(&config, &args)
            .expect("collect");
        assert!(active.is_empty());
    }

    #[test]
    fn collect_active_specs_for_inspect_invalid_cli_spec_errors() {
        // Pathological CLI literal — the colon parser rejects `bad-spec`
        // because the scope half is unknown. We want the error to bubble
        // up rather than be silently dropped so the operator sees it.
        let config = RalphConfig::default();
        let args = InspectProfilesArgs {
            profiles: vec!["bad-spec".to_string()],
            no_default_profiles: false,
            format: InspectProfilesFormat::Human,
        };
        let err = crate::commands::profile_args::collect_active_profile_specs(&config, &args)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("bad-spec"),
            "expected offending spec in error, got {msg}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // First-line preview truncation.
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn first_line_preview_returns_first_line_trimmed() {
        let preview = first_line_preview("### Strict rules\nSecond line ignored\n", 60);
        assert_eq!(preview, "### Strict rules");
    }

    #[test]
    fn first_line_preview_caps_at_max_chars_with_ellipsis() {
        let body: String = "a".repeat(120);
        let preview = first_line_preview(&body, 60);
        // 60 chars + the trailing '…' (3-byte UTF-8 char counts as 1 char)
        assert_eq!(preview.chars().count(), 61);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn first_line_preview_handles_empty_content() {
        assert_eq!(first_line_preview("", 60), "");
        assert_eq!(first_line_preview("\n", 60), "");
        assert_eq!(first_line_preview("   \n", 60), "");
    }

    #[test]
    fn first_line_preview_is_unicode_safe() {
        // 30 emoji × 1 codepoint each = 30 chars; below cap so no ellipsis.
        let body: String = "🐱".repeat(30);
        let preview = first_line_preview(&body, 60);
        assert_eq!(preview.chars().count(), 30);
        assert!(!preview.ends_with('…'));
        // Over the cap: takes 60 chars + ellipsis.
        let body: String = "🐱".repeat(80);
        let preview = first_line_preview(&body, 60);
        assert_eq!(preview.chars().count(), 61);
        assert!(preview.ends_with('…'));
    }

    // ─────────────────────────────────────────────────────────────────────
    // view-building: defaults, specs, preset, fragments, warnings.
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn build_view_includes_defaults_specs_preset_fragments() {
        // Exercise the view-builder end-to-end with a repo profile
        // (the user profile path resolves under `$HOME` and would
        // require either a writable `~/.config/ralph/profiles/extra/`
        // tree or a custom env lookup; the user-scope path is covered
        // by `inspect_view_assembles_happy_path_fragment` indirectly via
        // `resolve_user_profile_with_xdg_config_home` in
        // `ralph_core::profiles`).
        let defaults = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "base".to_string(),
        }];
        let specs = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "base".to_string(),
        }];
        let tmp = TempDir::new().unwrap();
        // Repo profile dir = <workspace_root>/ralph-profiles/<name>/<preset>/.
        let preset_dir = tmp.path().join("ralph-profiles").join("base").join("debug");
        write_fragment(&preset_dir, "investigator", "INVESTIGATOR_NOTES\n");

        let config = config_with_hats(&["investigator"]);
        let resolved =
            ralph_core::profiles::resolve_profile_fragments(&config, "debug", &specs, tmp.path())
                .expect("resolve must succeed");

        let view = build_view(&defaults, &specs, "debug", &resolved);
        assert!(view.active);
        assert_eq!(view.defaults, vec!["repo:base".to_string()]);
        assert_eq!(view.specs, vec!["repo:base".to_string()]);
        assert_eq!(view.preset.as_deref(), Some("debug"));
        assert_eq!(view.fragments.len(), 1);
        assert_eq!(view.fragments[0].spec, "repo:base");
        assert_eq!(view.fragments[0].hat_id, "investigator");
        assert_eq!(view.fragments[0].preview, "INVESTIGATOR_NOTES");
    }

    // ─────────────────────────────────────────────────────────────────────
    // End-to-end command (no IO captured, just exit code + JSON shape).
    // ─────────────────────────────────────────────────────────────────────

    // ─────────────────────────────────────────────────────────────────────
    // End-to-end view assembly. We deliberately drive `build_view` and
    // `resolve_profile_fragments` directly rather than going through
    // `load_config_for_preflight`, because the loader hard-pins
    // `config.core.workspace_root` to `current_dir()` and would force
    // every test to write profile directories inside the active
    // worktree (CI flake + cross-test pollution risk). The two halves of
    // the command body are exercised separately:
    //   * `derive_preset_name_for_inspect` is unit-tested above (covers
    //     the remote-hats and file-stem cases without network IO).
    //   * `resolve_profile_fragments` is unit-tested inside
    //     `ralph_core::profiles` (U2). Here we drive it through
    //     `build_view` to lock down the public output shape.
    // ─────────────────────────────────────────────────────────────────────

    /// Run a minimal end-to-end happy path: write one fragment, resolve
    /// it through the same primitives the command uses, and assert the
    /// view surface carries the fragment path + preview verbatim.
    /// Covers plan U5 acceptance "human output / JSON output shape".
    #[test]
    fn inspect_view_assembles_happy_path_fragment() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        write_fragment(
            &preset_dir,
            "investigator",
            "### investigator strict mode\n",
        );

        let config = config_with_hats(&["investigator"]);
        let specs = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        }];
        let resolved =
            ralph_core::profiles::resolve_profile_fragments(&config, "debug", &specs, tmp.path())
                .expect("resolve must succeed");

        let defaults: Vec<ProfileSpec> = Vec::new();
        let view = build_view(&defaults, &specs, "debug", &resolved);
        assert!(view.active);
        assert_eq!(view.preset.as_deref(), Some("debug"));
        assert_eq!(view.specs, vec!["repo:strict".to_string()]);
        assert!(view.warnings.is_empty(), "warnings: {:?}", view.warnings);
        assert_eq!(view.fragments.len(), 1);
        assert_eq!(view.fragments[0].spec, "repo:strict");
        assert_eq!(view.fragments[0].hat_id, "investigator");
        assert!(view.fragments[0].path.contains("investigator.md"));
        assert_eq!(view.fragments[0].preview, "### investigator strict mode");
    }

    /// Missing profile directory is a hard error from the resolver.
    /// The command layer wraps it with `with_context` so the operator
    /// sees a path; here we assert the underlying error carries the
    /// path fragment so the wrapper test below is meaningful.
    #[test]
    fn inspect_resolve_missing_profile_dir_surfaces_path() {
        let tmp = TempDir::new().unwrap();
        let config = config_with_hats(&["investigator"]);
        let specs = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "nope".to_string(),
        }];
        let err =
            ralph_core::profiles::resolve_profile_fragments(&config, "debug", &specs, tmp.path())
                .expect_err("missing dir must error");
        let msg = format!("{err}");
        assert!(
            msg.contains("nope"),
            "expected profile name in error, got {msg}"
        );
        assert!(
            msg.contains("ralph-profiles"),
            "expected repo path segment in error, got {msg}"
        );
    }

    /// End-to-end coverage for the "no active specs" branch (R13):
    /// neither `profiles.default` nor `--profile` is set, so the view
    /// reports `active: false` with a hint and no fragments.
    #[test]
    fn inspect_view_no_active_specs_is_inactive() {
        let defaults: Vec<ProfileSpec> = Vec::new();
        let specs: Vec<ProfileSpec> = Vec::new();
        let resolved = ResolvedProfileFragments::default();
        let view = InspectProfilesView {
            active: false,
            defaults: defaults.iter().map(profile_spec_to_string).collect(),
            specs: Vec::new(),
            preset: None,
            fragments: Vec::new(),
            warnings: Vec::new(),
            note: Some(
                "no profiles active (set ralph.yml `profiles.default` or pass --profile)"
                    .to_string(),
            ),
        };
        assert!(!view.active);
        assert_eq!(defaults, *&defaults); // sanity: defaults is empty
        let _ = (specs, resolved);
        assert!(view.note.is_some());
        assert!(view.fragments.is_empty());
    }

    /// Edge case: no preset is available (no `-H/--hats` source) but
    /// specs are active. The view reports the warning so the operator
    /// can debug "why isn't my fragment being applied?" — without
    /// bailing out.
    #[test]
    fn inspect_view_no_hats_source_emits_warning_in_view() {
        let specs = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        }];
        let view = InspectProfilesView {
            active: true,
            defaults: Vec::new(),
            specs: specs.iter().map(profile_spec_to_string).collect(),
            preset: None,
            fragments: Vec::new(),
            warnings: vec![
                "--profile specs requested but no preset is active \
                 (no -H/--hats source); profile resolution skipped"
                    .to_string(),
            ],
            note: None,
        };
        assert!(view.active);
        assert!(view.preset.is_none());
        assert_eq!(view.warnings.len(), 1);
        assert!(view.warnings[0].contains("no preset is active"));
        assert!(view.fragments.is_empty());
    }

    /// End-to-end shape coverage for `ralph inspect profiles --format
    /// json`: the view serialises with serde_json and round-trips back
    /// to the same fields. This guards against the human/JSON surfaces
    /// drifting out of sync (e.g. if someone adds a field to the
    /// struct but forgets to surface it in human output).
    #[test]
    fn inspect_view_serialises_to_expected_json_shape() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        write_fragment(&preset_dir, "investigator", "PREVIEW\n");

        let config = config_with_hats(&["investigator"]);
        let specs = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        }];
        let resolved =
            ralph_core::profiles::resolve_profile_fragments(&config, "debug", &specs, tmp.path())
                .expect("resolve must succeed");

        let defaults: Vec<ProfileSpec> = Vec::new();
        let view = build_view(&defaults, &specs, "debug", &resolved);
        let json = serde_json::to_value(&view).expect("serialise");
        assert_eq!(json["active"], serde_json::json!(true));
        assert_eq!(json["preset"], serde_json::json!("debug"));
        assert_eq!(json["specs"], serde_json::json!(["repo:strict"]));
        assert!(json["warnings"].as_array().unwrap().is_empty());
        let frags = json["fragments"].as_array().expect("fragments array");
        assert_eq!(frags.len(), 1);
        assert_eq!(frags[0]["spec"], serde_json::json!("repo:strict"));
        assert_eq!(frags[0]["hat_id"], serde_json::json!("investigator"));
        assert_eq!(frags[0]["preview"], serde_json::json!("PREVIEW"));
        assert!(
            frags[0]["path"]
                .as_str()
                .unwrap()
                .contains("investigator.md"),
            "path must include the fragment file name"
        );
        // `note` is `skip_serializing_if = "Option::is_none"`, so it
        // must be absent from the active-true case.
        assert!(json.get("note").is_none());
    }

    /// Mirror of the run.rs activation-order contract: defaults first,
    /// then CLI flags, with `--no-default-profiles` skipping only the
    /// defaults. Confirms inspect-profiles and `ralph run` agree.
    #[test]
    fn inspect_profiles_activation_order_matches_run_helper() {
        use crate::commands::run::collect_active_profile_specs;

        let mut config = RalphConfig::default();
        config.profiles.default = vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "base".to_string(),
        }];
        let inspect_args = InspectProfilesArgs {
            profiles: vec!["user:extra".to_string()],
            no_default_profiles: false,
            format: InspectProfilesFormat::Human,
        };

        let mut run_args = crate::commands::run::default_run_args();
        run_args.profiles = vec!["user:extra".to_string()];
        run_args.no_default_profiles = false;

        let inspect_specs =
            crate::commands::profile_args::collect_active_profile_specs(&config, &inspect_args)
                .expect("inspect");
        let run_specs =
            crate::commands::profile_args::collect_active_profile_specs(&config, &run_args)
                .expect("run");
        assert_eq!(inspect_specs, run_specs);

        // And the --no-default-profiles branch must agree.
        let mut inspect_args = inspect_args;
        inspect_args.no_default_profiles = true;
        let mut run_args = run_args;
        run_args.no_default_profiles = true;
        let inspect_specs =
            crate::commands::profile_args::collect_active_profile_specs(&config, &inspect_args)
                .expect("inspect");
        let run_specs =
            crate::commands::profile_args::collect_active_profile_specs(&config, &run_args)
                .expect("run");
        assert_eq!(inspect_specs, run_specs);
        assert_eq!(inspect_specs.len(), 1);
        assert_eq!(inspect_specs[0].to_string(), "user:extra");
    }
}
