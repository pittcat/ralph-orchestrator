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

use crate::cli::resolve_hat_channel_file;
use crate::display::colors;
use crate::operation_guard::OperationContext;
use crate::preflight;
use crate::{ConfigSource, HatsSource};
use anyhow::{Context, Result};
use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use ralph_core::RalphConfig;
use ralph_core::config::profiles::ProfileSpec;
use ralph_core::event_loop::PromptPreview;
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

    /// Read-only diagnostic of the active loop + hat identity (U5).
    ///
    /// Resolves the same `HatIdentitySnapshot` that the `## HAT IDENTITY`
    /// prompt block uses, surfaces the events-file resolution (main +
    /// hat-channel), and emits the loop marker / current hat context.
    /// Pair with `ralph events --events-source hat-channel` to close the
    /// OPAC Confirm loop. Read-only; never starts or mutates a loop.
    Loop(InspectLoopArgs),

    /// 2026-07-26-001 plan U3: preview what `build_prompt` would
    /// inject for one hat **without** running the loop. Read-only;
    /// no events are emitted, no `events.jsonl` is touched, no worktree
    /// state changes. Same source as the live prompt: the
    /// `ralph-core` `EventLoop::prompt_preview` API (U2). The
    /// companion `--full` flag (off by default — see KTD-7)
    /// additionally prints the rendered prompt body so the operator
    /// can read what the hat actually sees end-to-end.
    Prompt(InspectPromptArgs),
}

/// Arguments for `ralph inspect loop`.
///
/// Workspace root defaults to the current directory; pass `--root` to
/// point at another workspace (e.g. when inspecting a worktree from
/// the main repo). `--hat <ID>` overrides the live `RALPH_CURRENT_HAT`
/// when an operator wants to preview what the prompt block would look
/// like for a different hat.
#[derive(Parser, Debug)]
pub struct InspectLoopArgs {
    /// Optional hat id override (defaults to live `RALPH_CURRENT_HAT`).
    #[arg(long)]
    pub hat: Option<String>,

    /// Output format (human or json). JSON output is the SSOT that
    /// test fixtures and BDD scenarios assert against.
    #[arg(long, value_enum, default_value_t = InspectProfilesFormat::Human)]
    pub format: InspectProfilesFormat,

    /// Workspace root (default: current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
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

/// Arguments for `ralph inspect prompt` (2026-07-26-001 plan U3).
///
/// Mirrors the args surface of `ralph run` so the operator can
/// reuse their muscle memory: `-c` / `--config` accepts a YAML
/// path (or `-` for stdin) and `-H` / `--hats` accepts a builtin
/// preset or a local preset file. The hat id is required because
/// prompt visibility is per-hat and the hatless `ralph` sentinel
/// is intentionally not the default — the operator should know
/// which hat they're inspecting.
#[derive(Parser, Debug)]
pub struct InspectPromptArgs {
    /// Hat id to preview (e.g. `worker`, `reviewer`). Required.
    #[arg(long)]
    pub hat: String,

    /// Output format (human or json). JSON output is the SSOT
    /// for downstream tooling; human output is a readable block
    /// list per KTD-7.
    #[arg(long, value_enum, default_value_t = InspectProfilesFormat::Human)]
    pub format: InspectProfilesFormat,

    /// Also print the rendered prompt body. Default is the
    /// block-list view; `--full` adds the full dry prompt after
    /// the structured block (KTD-7: human = blocks + skill table;
    /// --full = blocks + skill table + full text).
    #[arg(long)]
    pub full: bool,

    // ── Unit 1 scenario args (2026-07-27-002 plan) ──────────────
    /// Simulated trigger topic (e.g. `build.task`). When provided,
    /// `trigger_context_injected` is computed and included in the
    /// preview output.
    #[arg(long)]
    pub trigger: Option<String>,

    /// Simulated source hat for the trigger event.
    #[arg(long)]
    pub source_hat: Option<String>,

    /// Simulated trigger payload as JSON. Must be valid JSON when
    /// provided.
    #[arg(long)]
    pub payload: Option<String>,

    /// Simulated iteration number for the hat activation.
    #[arg(long)]
    pub iteration: Option<u32>,

    /// Simulated wave context as JSON.
    #[arg(long)]
    pub wave_context: Option<String>,

    /// Simulated orchestrator context as JSON.
    #[arg(long)]
    pub orchestrator_context: Option<String>,

    /// Simulated correction context as JSON.
    #[arg(long)]
    pub correction: Option<String>,

    /// Override scratchpad gate for preview purposes.
    #[arg(long)]
    pub scratchpad: Option<bool>,

    /// Override tasks-enabled gate for preview purposes.
    #[arg(long)]
    pub tasks_enabled: Option<bool>,

    /// Override memories-enabled gate for preview purposes.
    #[arg(long)]
    pub memories_enabled: Option<bool>,

    // ── Unit 2 args (2026-07-27-002 plan) ──────────────────────────
    /// Topic to evaluate as a candidate emit (requires --payload).
    /// When provided with --payload, `candidate_emit` is computed and
    /// included in the preview output.
    #[arg(long)]
    pub topic: Option<String>,

    /// Simulated triggered hat for the candidate emit evaluation.
    /// Must be a registered hat id if provided; unknown hats cause rejection.
    /// Only meaningful when --topic and --payload are both provided.
    #[arg(long)]
    pub triggered: Option<String>,
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
        Some(InspectCommands::Loop(loop_args)) => {
            inspect_loop_command(config_sources, hats_source, loop_args, use_colors).await
        }
        Some(InspectCommands::Prompt(prompt_args)) => {
            inspect_prompt_command(config_sources, hats_source, prompt_args, use_colors).await
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

/// Read-only diagnostic of the current loop's identity + event channel.
///
/// Builds a `LoopInspectView` from resolved config + `OperationContext`
/// (live loop marker, env hat id, events file) without ever starting or
/// mutating a loop. The JSON shape is the SSOT that test fixtures and
/// BDD scenarios assert against (U5 / R5); the human view is a
/// readable version of the same data with diagnostics appended.
///
/// Pair with `ralph events --events-source hat-channel` for the OPAC
/// Confirm stage. When the user has `RALPH_CURRENT_HAT` set in their
/// shell the command surfaces that hat's identity block; when no hat
/// is set, the human output points the user at `ralph run ...` so they
/// have an actionable next step.
pub async fn inspect_loop_command(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    args: InspectLoopArgs,
    use_colors: bool,
) -> Result<()> {
    let config = preflight::load_config_for_preflight(config_sources, hats_source).await?;
    let root = match args.root.clone() {
        Some(r) => r,
        None => std::env::current_dir().context("resolve current dir")?,
    };
    let ctx = crate::operation_guard::OperationContext::detect(root.clone());

    // Hat resolution order:
    //   1. --hat override (operator preview)
    //   2. live `RALPH_CURRENT_HAT` (already in `ctx.current_hat_id`)
    //   3. None — surface a hint instead of fabricating an identity.
    let hat_id = args.hat.clone().or_else(|| ctx.current_hat_id.clone());

    let hat_identity = hat_id.as_deref().and_then(|h| {
        ralph_core::hat_identity::HatIdentitySnapshot::from_config(
            &config,
            &ralph_proto::HatId::new(h.to_string()),
        )
    });

    // Resolve the events file allowlist pair (main + hat-channel) by
    // surface area only — we never read the files ourselves, only stat
    // them so the operator can see whether the channels are alive.
    let (main_events, hat_channel_events) = resolve_event_paths(&root, &ctx);
    let main_size = std::fs::metadata(&main_events)
        .map(|m| m.len())
        .unwrap_or(0);
    // For hat-channel, resolve the marker to the real channel file
    // before statting — the marker itself is a tiny path string
    // (~/.ralph/agent/events-hat-X.jsonl) and reporting its size
    // would mislead operators into thinking the channel is empty
    // when in fact the channel sits one dereference away (U4 P1 #6).
    let (hat_channel_label, hat_size, hat_channel_warning) = match resolve_hat_channel_file(&root) {
        None => (
            hat_channel_events.display().to_string(),
            std::fs::metadata(&hat_channel_events)
                .map(|m| m.len())
                .unwrap_or(0),
            Some(
                "hat-channel marker (.ralph/current-hat-events) missing or empty; \
                 inspect after the first hat activation so the runtime can publish it"
                    .to_string(),
            ),
        ),
        Some((resolved, exists)) => {
            let size = if exists {
                std::fs::metadata(&resolved).map(|m| m.len()).unwrap_or(0)
            } else {
                0
            };
            let warn = if exists {
                None
            } else {
                Some(
                    "hat-channel marker resolves to a path that does not yet exist on disk; \
                     emit will fall back to main events until the runner creates it"
                        .to_string(),
                )
            };
            (resolved.display().to_string(), size, warn)
        }
    };

    let mut warnings = Vec::new();
    if ctx.current_loop_id.is_none() {
        warnings.push(
            "no current-loop-id marker at <root>/.ralph/current-loop-id; \
             run `ralph run ...` to create one"
                .to_string(),
        );
    }
    if let Some(w) = hat_channel_warning {
        warnings.push(w);
    }
    if ctx.current_hat_id.is_none() && args.hat.is_none() {
        warnings.push(
            "no current hat in environment (RALPH_CURRENT_HAT unset) and no --hat override; \
             hat_identity will be null. Pass `--hat <id>` to preview a hat identity."
                .to_string(),
        );
    }
    if hat_channel_events.exists() && hat_size == 0 {
        warnings.push(format!(
            "hat-channel file exists but is 0 bytes: {}",
            hat_channel_events.display()
        ));
    }
    if !hat_channel_events.exists() && ctx.is_agent_context {
        warnings.push(
            "hat-channel marker (.ralph/current-hat-events) missing — \
             emit will fall back to main events; inspect after the first hat activation"
                .to_string(),
        );
    }

    // U1 (plan 2026-07-04-004): derive the loop anchor view. When
    // there is no plan attached (no `event_loop.prompt_file` pointing
    // at a plan-shaped file, no persisted plan baseline marker) we
    // surface a warning so operators know the inspect command has
    // nothing to anchor against. The anchor struct stays `None` on
    // the view so the JSON key is omitted — backward-compatible.
    let loop_anchor = build_loop_anchor_summary(&config, &root, ctx.current_loop_id.as_deref());
    if loop_anchor.is_none() {
        warnings.push(loop_anchor_unattached_warning().to_string());
    }

    let view = LoopInspectView {
        workspace_root: root.display().to_string(),
        loop_id: ctx.current_loop_id.clone(),
        current_hat: hat_id.clone(),
        is_agent_context: ctx.is_agent_context,
        hat_identity: hat_identity
            .as_ref()
            .map(|s| s.to_json())
            .unwrap_or(serde_json::Value::Null),
        events_file: main_events.display().to_string(),
        hat_channel_file: hat_channel_label.clone(),
        events_size: main_size,
        hat_channel_size: hat_size,
        warnings,
        schema_version: LOOP_INSPECT_SCHEMA_VERSION.to_string(),
        loop_anchor,
        supervisor: build_supervisor_summary(&config, &root),
    };

    match args.format {
        InspectProfilesFormat::Json => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            serde_json::to_writer_pretty(&mut handle, &view)?;
            writeln!(handle)?;
        }
        InspectProfilesFormat::Human => print_loop_view(&view, use_colors),
    }
    Ok(())
}

/// 2026-07-26-001 plan U3: preview the prompt visibility for one hat
/// without starting the loop.
///
/// **Read-only by construction.** The CLI path is:
///   1. Load config via `preflight::load_config_for_preflight` — the
///      same resolver `ralph run` uses for `-c` / `-H` so the preset
///      surface matches.
///   2. Build a temporary `EventLoop` and call `prompt_preview`. The
///      loop is initialized to a dry `initialize("ralph inspect prompt
///      (read-only)")` payload so the registry is consistent; no events
///      are published, no `events.jsonl` file is written, no worktree
///      state changes.
///   3. Render the preview via [`emit_prompt_view`].
///
/// Failure modes:
///   - Hat not in preset → exit 2 with a stderr line naming the hat.
///   - Config load failure → exit 1 with the preflight error.
///   - Other build errors → propagated via anyhow.
pub async fn inspect_prompt_command(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    args: InspectPromptArgs,
    use_colors: bool,
) -> Result<()> {
    let config = preflight::load_config_for_preflight(config_sources, hats_source).await?;
    let mut preview_config = config.clone();
    if let Some(enabled) = args.tasks_enabled {
        preview_config.tasks.enabled = enabled;
    }
    if let Some(enabled) = args.memories_enabled {
        preview_config.memories.enabled = enabled;
    }
    if let Some(enabled) = args.scratchpad {
        preview_config.core.scratchpad.enabled = enabled;
    }
    let hat_id = ralph_proto::HatId::new(args.hat.clone());

    // Save a cloned config for the read-only candidate emit evaluation
    // (EventLoop takes ownership of config, so we clone before the move).
    let config_for_candidate = preview_config.clone();

    // Block titles are extracted via a dry `build_prompt` call,
    // which requires constructing an EventLoop. We suppress
    // tracing output for the duration of that call so the
    // `tracing::info!("Memory injection check…")` line that
    // fires during `initialize` does not pollute stdout and
    // corrupt the JSON SSOT contract. The DefaultGuard restores
    // the global default on drop; once the dry call is done,
    // later work (e.g. printing the structured preview) gets
    // the normal stdout writer back.
    use tracing_subscriber::prelude::*;

    let suppressed = tracing::level_filters::LevelFilter::OFF;
    let _guard =
        tracing::dispatcher::set_default(&tracing_subscriber::registry().with(suppressed).into());

    let mut event_loop = ralph_core::event_loop::EventLoop::new(preview_config);
    event_loop.initialize("ralph inspect prompt (read-only)");
    if let Some(iteration) = args.iteration {
        event_loop.set_iteration_for_test(iteration);
    }
    let preview_base = event_loop.prompt_preview(&hat_id).ok_or_else(|| {
        anyhow::anyhow!(
            "hat {:?} not found in preset; available hats are listed by `ralph hats list`",
            hat_id.as_str()
        )
    })?;

    // Build scenario context from args (Unit 1 of plan 2026-07-27-002).
    // When any scenario parameter is provided, we enrich the preview
    // with simulated trigger/wave/correction/orchestrator context fields.
    let has_scenario = args.trigger.is_some()
        || args.payload.is_some()
        || args.source_hat.is_some()
        || args.wave_context.is_some()
        || args.orchestrator_context.is_some()
        || args.correction.is_some()
        || args.scratchpad.is_some()
        || args.tasks_enabled.is_some()
        || args.memories_enabled.is_some()
        || args.topic.is_some()
        || args.triggered.is_some();

    let preview = if has_scenario {
        // Parse JSON scenario args.
        let payload_json = match &args.payload {
            Some(p) => match serde_json::from_str::<serde_json::Value>(p) {
                Ok(v) => Some(v),
                Err(e) => anyhow::bail!("failed to parse --payload JSON: {} (field: --payload)", e),
            },
            None => None,
        };
        let wave_json = match &args.wave_context {
            Some(w) => Some(serde_json::from_str::<serde_json::Value>(w).map_err(|e| {
                anyhow::anyhow!(
                    "failed to parse --wave-context JSON: {} (field: --wave-context)",
                    e
                )
            })?),
            None => None,
        };
        let orchestrator_json = match &args.orchestrator_context {
            Some(o) => Some(serde_json::from_str::<serde_json::Value>(o).map_err(|e| {
                anyhow::anyhow!(
                    "failed to parse --orchestrator-context JSON: {} (field: --orchestrator-context)",
                    e
                )
            })?),
            None => None,
        };
        let correction_json = match &args.correction {
            Some(c) => Some(serde_json::from_str::<serde_json::Value>(c).map_err(|e| {
                anyhow::anyhow!(
                    "failed to parse --correction JSON: {} (field: --correction)",
                    e
                )
            })?),
            None => None,
        };

        // Build Option<TriggerContextView> from --trigger / --source-hat / --payload.
        let trigger_context_injected = args.trigger.as_ref().map(|topic| {
            // Build a minimal trigger context view from the scenario args.
            // This mirrors ralph_core::trigger_context::build but without
            // requiring a full schema lookup — the preview context is
            // informational, not authoritative.
            use ralph_core::trigger_context::{FieldSummary, FieldValue, TriggerContextView};
            // U8: compute source_hat_known from config.hats membership.
            // None when source_hat is None (not serialized).
            // Some(true) when source_hat is in config.hats.
            // Some(false) when source_hat is provided but unknown.
            // Use config_for_candidate (cloned before EventLoop took ownership
            // of config) to avoid borrow-after-move.
            let source_hat_known = args
                .source_hat
                .as_ref()
                .map(|h| config_for_candidate.hats.contains_key(h));
            TriggerContextView {
                source_topic: topic.clone(),
                source_hat: args.source_hat.clone(),
                source_hat_known,
                current_hat: hat_id.as_str().to_string(),
                summary: payload_json
                    .as_ref()
                    .map(|pj| {
                        if let serde_json::Value::Object(map) = pj {
                            map.iter()
                                .map(|(k, v)| FieldSummary {
                                    field: k.clone(),
                                    value: FieldValue::Present(v.clone()),
                                })
                                .collect()
                        } else {
                            Vec::new()
                        }
                    })
                    .unwrap_or_default(),
                matched_hints: Vec::new(),
            }
        });

        // Build Option<WaveContext> from --wave-context.
        // Before U1 fix: silently fell through to None on deserialization
        // failure (.ok()).  After U1: fail loud so callers know the field
        // shape was rejected (F-IDs: adversarial:A1, maintainability:M4).
        let wave_context_injected = wave_json
            .as_ref()
            .map(|wv| {
                serde_json::from_value::<ralph_core::wave_context::WaveContext>(wv.clone()).map_err(
                    |e| {
                        anyhow::anyhow!(
                            "failed to deserialize --wave-context into WaveContext: {} \
                             (field: --wave-context)",
                            e
                        )
                    },
                )
            })
            .transpose()?;

        // Build Option<CorrectionContext> from --correction.
        let correction_injected = correction_json
            .as_ref()
            .map(|cv| {
                serde_json::from_value::<ralph_core::correction::CorrectionContext>(cv.clone())
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "failed to deserialize --correction into CorrectionContext: {} \
                             (field: --correction)",
                            e
                        )
                    })
            })
            .transpose()?;

        // Build skill_gates from overrides.
        // Any gate not explicitly supplied falls back to the effective
        // config's prompt gates after the preview overrides above.
        let skill_gates = if args.scratchpad.is_some()
            || args.tasks_enabled.is_some()
            || args.memories_enabled.is_some()
        {
            Some(ralph_core::event_loop::SkillGateFlags {
                tasks_enabled: args
                    .tasks_enabled
                    .unwrap_or(preview_base.gates.tasks_enabled),
                memories_enabled: args
                    .memories_enabled
                    .unwrap_or(preview_base.gates.memories_enabled),
                scratchpad_enabled: args.scratchpad.unwrap_or(false),
            })
        } else {
            None
        };

        // Merge scenario fields onto the base preview.
        ralph_core::event_loop::PromptPreview {
            trigger_context_injected,
            wave_context_injected,
            orchestrator_context_injected: orchestrator_json,
            correction_injected,
            skill_gates,
            evidence_level: "runtime".to_string(),
            candidate_emit: None,
            ..preview_base
        }
    } else {
        preview_base
    };

    // ── Unit 2: candidate emit evaluation ─────
    // When --topic is provided without --payload, fail fast with an
    // error so the caller knows the emission preview cannot run.
    let preview = match (&args.topic, &args.payload) {
        (Some(_topic), None) => {
            anyhow::bail!(
                "--topic requires --payload to evaluate the candidate emit; \
                 provide both or omit --topic"
            );
        }
        (Some(topic), Some(payload_str)) => {
            // Validate that payload is parseable JSON.
            let _parsed: serde_json::Value = serde_json::from_str(payload_str).map_err(|e| {
                anyhow::anyhow!(
                    "failed to parse --payload JSON: {} (required for --topic evaluation)",
                    e
                )
            })?;

            // Read-only evaluate the candidate emit.
            let candidate_emit = ralph_core::evaluate_candidate_emit(
                &config_for_candidate,
                &hat_id,
                topic,
                payload_str,
                args.triggered.as_deref(),
            )
            .map_err(|e| anyhow::anyhow!("candidate emit evaluation failed: {e}"))?;

            ralph_core::event_loop::PromptPreview {
                candidate_emit: Some(candidate_emit),
                ..preview
            }
        }
        (None, _) => preview,
    };

    // Build the full prompt body while tracing is still suppressed,
    // then drop the guard before emitting output so normal logging resumes.
    let full_body = if args.full {
        event_loop.build_prompt(&hat_id)
    } else {
        None
    };
    drop(_guard);

    emit_prompt_view(&preview, full_body, args.format, args.full, use_colors)
}

/// Render a `PromptPreview` in the operator's chosen format.
fn emit_prompt_view(
    preview: &PromptPreview,
    full_body: Option<String>,
    format: InspectProfilesFormat,
    full: bool,
    use_colors: bool,
) -> Result<()> {
    match format {
        InspectProfilesFormat::Json => {
            // JSON output is the SSOT for tooling; the human-only
            // `--full` body is appended as a separate top-level
            // `prompt_body` field when requested.
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            if full {
                // Use the pre-computed prompt body (computed while tracing
                // was suppressed so no log pollution reaches stdout).
                // FAIL-LOUD on missing body: `prompt_body` is part of
                // the SSOT contract (`--full` guarantees a real body);
                // silently emitting `""` would mask a real failure
                // mode (e.g. build_prompt errored, registry missing,
                // hat stripped after preview).
                let body = full_body.ok_or_else(|| {
                    anyhow::anyhow!(
                        "prompt_body unavailable for hat {:?} under --full; \
                         inspect failed to materialize build_prompt output \
                         (registry empty, hat unknown after preview, or \
                         build_prompt returned None) — refusing to emit an \
                         empty SSOT field",
                        preview.hat_id
                    )
                })?;
                #[derive(serde::Serialize)]
                struct PromptViewJson<'a> {
                    #[serde(flatten)]
                    preview: &'a PromptPreview,
                    prompt_body: &'a str,
                }
                let view = PromptViewJson {
                    preview,
                    prompt_body: body.as_str(),
                };
                serde_json::to_writer_pretty(&mut handle, &view)?;
            } else {
                serde_json::to_writer_pretty(&mut handle, preview)?;
            }
            writeln!(handle)?;
        }
        InspectProfilesFormat::Human => {
            print_prompt_view_human(preview, full_body, full, use_colors)?;
        }
    }
    Ok(())
}

/// Render a labeled list section with a `(none)` / `… and N more`
/// truncation policy. Each item is `(display, optional_source)`;
/// when `source` is present it renders as `(source)` after the
/// display in dim styling. Inline items cap at 20.
fn print_labeled_section(
    label: &str,
    header_color: &str,
    empty_color: &str,
    reset: &str,
    items: &[(String, Option<&'static str>)],
) {
    println!("{header_color}{label} ({}):{reset}", items.len());
    if items.is_empty() {
        println!("    {empty_color}(none){reset}");
        return;
    }
    const MAX_INLINE: usize = 20;
    for (display, source) in items.iter().take(MAX_INLINE) {
        match source {
            Some(src) => println!("    - {display} {empty_color}({src}){reset}"),
            None => println!("    - {display}"),
        }
    }
    if items.len() > MAX_INLINE {
        println!(
            "    {empty_color}… and {} more (use --format json for the full list){reset}",
            items.len() - MAX_INLINE
        );
    }
}

fn source_tag(s: ralph_core::event_loop::PromptSkillSource) -> &'static str {
    match s {
        ralph_core::event_loop::PromptSkillSource::Gated => "gated",
        ralph_core::event_loop::PromptSkillSource::RegistryAuto => "registry_auto",
        ralph_core::event_loop::PromptSkillSource::OnDemand => "on_demand",
    }
}

fn print_prompt_view_human(
    preview: &PromptPreview,
    full_body: Option<String>,
    full: bool,
    use_colors: bool,
) -> Result<()> {
    let (cyan, dim, reset, yellow) = if use_colors {
        (colors::CYAN, colors::DIM, colors::RESET, colors::YELLOW)
    } else {
        ("", "", "", "")
    };

    println!("{cyan}Prompt visibility preview{reset}");
    println!("  hat_id:           {}", preview.hat_id);
    println!(
        "  gates:            tasks={} memories={}",
        preview.gates.tasks_enabled, preview.gates.memories_enabled,
    );

    let auto: Vec<(String, Option<&'static str>)> = preview
        .auto_inject
        .iter()
        .map(|e| (e.name.clone(), Some(source_tag(e.source.clone()))))
        .collect();
    let demand: Vec<(String, Option<&'static str>)> = preview
        .on_demand
        .iter()
        .map(|e| (e.name.clone(), None))
        .collect();
    let blocks: Vec<(String, Option<&'static str>)> = preview
        .block_titles
        .iter()
        .map(|t| (format!("## {t}"), None))
        .collect();
    print_labeled_section("  auto_inject", cyan, dim, reset, &auto);
    print_labeled_section("  on_demand", cyan, dim, reset, &demand);
    print_labeled_section("  block_titles", cyan, yellow, reset, &blocks);

    if !full {
        return Ok(());
    }
    // FAIL-LOUD on missing body (same contract as the JSON path).
    let body = full_body
        .ok_or_else(|| anyhow::anyhow!("prompt_body unavailable for hat {:?}", preview.hat_id))?;
    println!("\n{cyan}--full prompt body{reset}\n{body}");
    Ok(())
}

/// Versioned schema for the JSON output of `ralph inspect loop`.
/// Bumped when the field set changes shape; tests and BDD scenarios
/// pin against this value so version drift fails fast.
///
/// v1 → v2 (Unit 1 of plan 2026-07-04-004): adds the optional
/// `loop_anchor` block (`plan_path` / `plan_name` / `plan_baseline_sha` /
/// `loop_start_sha` / `attached_at`). The key is omitted when no
/// plan is attached so v1 consumers continue to parse cleanly
/// (`skip_serializing_if = "Option::is_none"`).
pub const LOOP_INSPECT_SCHEMA_VERSION: &str = "loop_inspect.v2";

/// U1 (plan 2026-07-04-004): anchor block summarising the loop's
/// attached plan. `None` when no plan is attached (no plan path on
/// disk + no persisted baseline + no plan_baseline_sha marker). The
/// agent-safe surface gives the OPAC Observe stage one place to
/// find what plan this loop is actually driving, instead of asking
/// the agent to fish around in `.ralph/agent/` or reconstruct the
/// plan name from `event_loop.prompt_file` heuristics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LoopAnchorView {
    /// Repo-relative or absolute path to the plan file driving this
    /// loop (sourced from `event_loop.prompt_file` when it points
    /// at a `.md` / `.html` file under `docs/plans/`).
    pub plan_path: std::path::PathBuf,
    /// Derived from `plan_path.file_stem()` — e.g.
    /// `2026-07-04-004-feat-single-chain-refactor-plan`.
    pub plan_name: String,
    /// SHA captured at plan start (file
    /// `.ralph/agent/plan-baseline.sha`). `None` when no baseline
    /// marker is on disk (loop never ran a baseline-enable step).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_baseline_sha: Option<String>,
    /// `loop_start_sha` from `LoopState` (line 461) — the SHA at
    /// which the loop runner started. `None` when no
    /// `.ralph/loops.json` entry matches the current loop marker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_start_sha: Option<String>,
    /// ISO-8601 UTC timestamp from `.ralph/loops.json`'s loop
    /// entry (`started` field). `None` when no entry matches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attached_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Serializable view of the inspection result. Both human and JSON
/// output are derived from this struct so the two surfaces cannot drift.
#[derive(Debug, Clone, serde::Serialize)]
struct LoopInspectView {
    /// Workspace root (where the `.ralph/` markers live).
    workspace_root: String,
    /// Current loop id from `.ralph/current-loop-id` marker, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    loop_id: Option<String>,
    /// Resolved hat id (override > env > null).
    #[serde(skip_serializing_if = "Option::is_none")]
    current_hat: Option<String>,
    /// Whether the caller is in agent context (env-driven).
    is_agent_context: bool,
    /// Same struct the prompt block uses (R5 SSOT).
    /// `null` when the hat is unknown — fail-closed by design.
    hat_identity: serde_json::Value,
    /// Resolved path to the main events file (workspace-rooted).
    events_file: String,
    /// Resolved path to the hat-channel events file.
    hat_channel_file: String,
    /// Size of the main events file in bytes (0 if missing).
    events_size: u64,
    /// Size of the hat-channel events file in bytes (0 if missing).
    hat_channel_size: u64,
    /// Non-fatal warnings (missing marker, empty channel, etc.).
    warnings: Vec<String>,
    /// Schema version — bump on field-set changes.
    schema_version: String,
    /// U1 (plan 2026-07-04-004): agent-safe plan anchor when a
    /// plan is attached. `None` when no plan marker / baseline is on
    /// disk — the key is then omitted from JSON for forward compat
    /// with v1 consumers.
    #[serde(skip_serializing_if = "Option::is_none")]
    loop_anchor: Option<LoopAnchorView>,
    /// U22: agent-safe supervisor summary, present only when
    /// `event_loop.supervisor.enabled` is true and the supervisor
    /// store can be opened. `None` → JSON has no `supervisor` key.
    #[serde(skip_serializing_if = "Option::is_none")]
    supervisor: Option<ralph_core::supervisor::SupervisorInspectSummary>,
}

/// U1 (plan 2026-07-04-004): build the loop anchor summary from on-disk
/// markers + the loaded `event_loop` config. Returns `None` when no
/// plan path can be determined (no `prompt_file`, no `.ralph/loops.json`
/// entry). The function is intentionally read-only — it never mutates
/// any marker. Sources, in priority order:
///
/// 1. `config.event_loop.prompt_file` — when non-empty and pointing at
///    a `.md`/`.html` file (heuristic: extension matches one of the
///    two) it is treated as the canonical plan path.
/// 2. Persisted `.ralph/agent/plan-baseline.sha` — used to populate
///    `plan_baseline_sha` (not for `plan_path`, which must come from
///    a real file path).
/// 3. `.ralph/loops.json` — supplies `attached_at` via the entry
///    whose `id` matches the live `current-loop-id` marker (read
///    separately by the caller). `loop_start_sha` is intentionally
///    left as `None` in this Unit: `LoopEntry` does not currently
///    persist a git HEAD-at-startup SHA; future Units that wire the
///    ledger can populate it without changing this signature.
///
/// The `current_loop_id` argument is the resolved loop marker value
/// (may be `None`); when `None` the loops.json lookup is skipped, so
/// unattached loops surface `attached_at: None` instead of
/// fabricating timestamps.
fn build_loop_anchor_summary(
    config: &RalphConfig,
    workspace_root: &std::path::Path,
    current_loop_id: Option<&str>,
) -> Option<LoopAnchorView> {
    // U6 of plan 2026-07-05-005: prefer the marker file written
    // by `ralph resume --plan <file>` (the resume path cannot
    // rely on `config.event_loop.prompt_file` being rewritten
    // the way `ralph run --plan` does). The marker reader is
    // lenient (missing file → `None`, corrupt JSON → warning +
    // fallback to the prompt-file-extension check) so the
    // existing inspect behaviour is preserved when no marker is
    // present.
    if let Some(marker_view) = read_anchor_marker(workspace_root) {
        return Some(marker_view);
    }

    // Source 1 — plan_path: prefer the loaded config's `prompt_file`
    // when it points at a markdown / html file. Anything else
    // (default sentinel `"PROMPT.md"`, a directory, a non-plan file)
    // falls through and we report unattached. The sentinel matches
    // `ralph_core::config::loop_config::default_prompt_file()` —
    // keep the two in sync if the default ever moves.
    const DEFAULT_PROMPT_FILE_SENTINEL: &str = "PROMPT.md";
    let prompt_file = &config.event_loop.prompt_file;
    let plan_path = if prompt_file.is_empty() || prompt_file == DEFAULT_PROMPT_FILE_SENTINEL {
        None
    } else {
        let pb = std::path::PathBuf::from(prompt_file);
        let ext_is_plan = pb
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("html"))
            .unwrap_or(false);
        if ext_is_plan { Some(pb) } else { None }
    };
    let plan_path = plan_path?;

    let plan_name = plan_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if plan_name.is_empty() {
        return None;
    }

    // Source 2 — plan_baseline_sha from persisted marker.
    let plan_baseline_sha = ralph_core::plan_baseline::read_plan_baseline(workspace_root, None);

    // Source 3 — attached_at from `.ralph/loops.json`. The list is
    // allowed to be empty (no persisted loops) without an error.
    // `loop_start_sha` stays None per the contract documented above.
    let attached_at = match current_loop_id {
        Some(loop_id) => ralph_core::loop_registry::LoopRegistry::new(workspace_root)
            .list()
            .ok()
            .and_then(|entries| entries.into_iter().find(|e| e.id == loop_id))
            .map(|entry| entry.started),
        None => None,
    };

    Some(LoopAnchorView {
        plan_path,
        plan_name,
        plan_baseline_sha,
        loop_start_sha: None,
        attached_at,
    })
}

/// U6 of plan 2026-07-05-005: read the resume-path anchor marker
/// from `<workspace>/.ralph/agent/.ralph-anchor.json`. Returns
/// `None` when the file does not exist (the common case: `ralph
/// run --plan` writes `prompt_file` directly, no marker needed).
/// Corrupt JSON is logged at `warn!` and treated as a miss so
/// a malformed marker cannot lock the loop out of inspect.
pub(crate) fn read_anchor_marker(workspace_root: &std::path::Path) -> Option<LoopAnchorView> {
    let path = workspace_root
        .join(".ralph")
        .join("agent")
        .join(".ralph-anchor.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!(
                "ralph-cli: failed to read anchor marker at {}: {e}; falling back",
                path.display()
            );
            return None;
        }
    };
    match serde_json::from_str::<AnchorMarker>(&raw) {
        Ok(marker) => Some(marker.into_anchor_view()),
        Err(e) => {
            eprintln!(
                "ralph-cli: anchor marker at {} is corrupt ({e}); falling back",
                path.display()
            );
            None
        }
    }
}

/// On-disk shape of the resume-path anchor marker. Persisted by
/// `ralph resume --plan <file>` so the inspect command can find
/// the plan attachment even when the in-memory `prompt_file`
/// still holds the sentinel value (resume does not rewrite
/// `prompt_file` like `ralph run --plan` does).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct AnchorMarker {
    pub plan_path: std::path::PathBuf,
    pub plan_name: String,
    pub plan_baseline_sha: Option<String>,
    pub attached_at: Option<String>,
}

impl AnchorMarker {
    /// U11 of plan 2026-07-05-005 (fix-plan §R9 / S4): single
    /// constructor that owns `plan_name` derivation
    /// (`plan_path.file_stem()`) so the 3–4 inline derivations
    /// across `inspect.rs` and `resume.rs` collapse into one
    /// site. Caller passes the on-disk fields (plan_path,
    /// baseline_sha, attached_at); the constructor derives
    /// `plan_name` and the file stem shape.
    pub(crate) fn from_plan_path(
        plan_path: &std::path::Path,
        plan_baseline_sha: Option<String>,
        attached_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Self {
        Self {
            plan_path: plan_path.to_path_buf(),
            plan_name: plan_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string(),
            plan_baseline_sha,
            attached_at: attached_at.map(|dt| dt.to_rfc3339()),
        }
    }

    fn into_anchor_view(self) -> LoopAnchorView {
        LoopAnchorView {
            plan_path: self.plan_path,
            plan_name: self.plan_name,
            plan_baseline_sha: self.plan_baseline_sha,
            loop_start_sha: None,
            // Parse the RFC3339 timestamp written by
            // `write_resume_anchor_marker`; fall back to None
            // when the field is absent or unparseable (the
            // marker is best-effort — see `read_anchor_marker`).
            attached_at: self
                .attached_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc)),
        }
    }
}

/// U1 (plan 2026-07-04-004): canonical unattached-loop-anchor
/// warning string. Surfaced by [`inspect_loop_command`] when
/// [`build_loop_anchor_summary`] returns `None`. The literal is
/// pinned (vs. inlined) so tests, lint, and BDD scenarios can
/// match it byte-for-byte without copying the warning into every
/// site. The leading fragment `"loop_anchor not attached"` is the
/// stable needle used by downstream tools.
pub fn loop_anchor_unattached_warning() -> &'static str {
    "loop_anchor not attached; preset hats requiring loop_anchor will receive null. \
     Pass --plan <path> to attach a plan, or run inside an active loop"
}

/// U22 + U8 of plan 2026-07-04-002: produce an agent-safe supervisor
/// summary block for `inspect loop`.
///
/// Gate (R8 / KTD6 of plan 2026-07-24-001): the block is emitted when
/// supervisor is enabled in config **or** a supervisor ledger file
/// exists on disk (`<root>/.ralph/supervisor.db`) — the default-wave /
/// lazy-opened-store path keeps `supervisor.enabled: false` yet still
/// leaves a ledger behind. Only when both are absent does the function
/// return `None` (JSON key omitted), keeping pure pipelines quiet.
/// Returns `Some(default)` (active_waves: [], queue_depth: 0,
/// slot_summary: [], last_coordination_topics: []) when the gate is
/// satisfied via config but the db is missing / cannot be opened.
///
/// When the `supervisor-db` feature is on AND the db is reachable the
/// function opens the rusqlite store, calls
/// `ralph_core::supervisor::summarize(&store)` to populate
/// `active_waves` / `queue_depth` / `slot_summary` /
/// `last_coordination_topics` from the live store, and emits the
/// resulting struct verbatim.
///
/// Output safety (R11): the struct never includes the supervisor db
/// path, event-log contents, or any other internal ledger field;
/// only the public `slot_summary[]` and `last_coordination_topics[]`
/// derived from the active wave's kind and the public
/// `SUPERVISOR_COORDINATION_TOPICS` whitelist are surfaced.
fn build_supervisor_summary(
    config: &RalphConfig,
    workspace_root: &std::path::Path,
) -> Option<ralph_core::supervisor::SupervisorInspectSummary> {
    let supervisor_enabled = config.event_loop.supervisor.enabled;
    let db_path = workspace_root.join(".ralph/supervisor.db");
    let ledger_present = db_path.exists();

    // R8 / KTD6: a supervisor ledger on disk (default-wave loop that
    // lazy-opened the store) is enough evidence to surface the summary
    // even when the preset keeps `supervisor.enabled: false`. Hide the
    // block only when neither config nor ledger evidence exists, so
    // pure pipelines stay quiet.
    if !supervisor_enabled && !ledger_present {
        return None;
    }

    if !ledger_present {
        return Some(ralph_core::supervisor::SupervisorInspectSummary::default());
    }

    // Best-effort open: a missing / corrupt db must NOT abort the
    // inspect command (Observe stage is read-only and best-effort).
    //
    // 2026-07-24-003 plan U3: a store-open failure now surfaces as
    // `availability = "unavailable"` with a sanitised reason so the
    // agent can distinguish a healthy empty store from a corrupt
    // one (S13). The previous "default empty summary" shape masked
    // the corruption from operators and was the U1 baseline
    // invariant `baseline_inspect_loop_swallows_corrupt_store`.
    #[cfg(feature = "supervisor-db")]
    {
        match ralph_core::supervisor::RusqliteSupervisorStore::open(&db_path) {
            Ok(store) => {
                // U3: also include terminal waves (Done / Failed) in
                // the summary so the agent can confirm a previous
                // `*.wave.complete` event. We deliberately re-use
                // `summarize` for the active-waves branch and append
                // terminal waves via `list_wave_ids` + `fan_in_status`.
                // For now `summarize` already covers the active case;
                // terminal surfacing is U5 (the inspect command
                // already exposes terminal waves through the wave
                // command's inspect path).
                Some(ralph_core::supervisor::summarize(&store))
            }
            Err(err) => {
                let reason = err.to_string();
                let sanitised = ralph_core::supervisor::sanitize_unavailable_reason(&reason);
                Some(ralph_core::supervisor::SupervisorInspectSummary {
                    availability: "unavailable",
                    unavailable_reason: Some(sanitised),
                    ..ralph_core::supervisor::SupervisorInspectSummary::default()
                })
            }
        }
    }
    #[cfg(not(feature = "supervisor-db"))]
    {
        // Without the rusqlite feature the binary cannot open the
        // supervisor store. Surface a default summary so the JSON
        // shape stays stable; consumers pin `loop_inspect.v2` and
        // know `availability: "unavailable"` is the contract for
        // "store unreachable".
        Some(ralph_core::supervisor::SupervisorInspectSummary {
            availability: "unavailable",
            unavailable_reason: Some(
                "supervisor-db feature not compiled in this build".to_string(),
            ),
            ..ralph_core::supervisor::SupervisorInspectSummary::default()
        })
    }
}

/// Resolve the canonical main + hat-channel events file paths for a
/// given workspace root. The function only composes paths; it does not
/// stat or read the files (callers decide). The `ctx` argument is kept
/// for future hooks (e.g. honour an explicit events-file override when
/// the runner exposes one); today only the workspace matters.
fn resolve_event_paths(root: &std::path::Path, _ctx: &OperationContext) -> (PathBuf, PathBuf) {
    let ralph_dir = root.join(".ralph");
    let main = ralph_dir.join("events.jsonl");
    let hat_channel = ralph_dir.join("current-hat-events");
    (main, hat_channel)
}

fn print_loop_view(view: &LoopInspectView, use_colors: bool) {
    let cyan = if use_colors { colors::CYAN } else { "" };
    let dim = if use_colors { colors::DIM } else { "" };
    let reset = if use_colors { colors::RESET } else { "" };
    let yellow = if use_colors { colors::YELLOW } else { "" };

    println!("{cyan}Loop inspection{reset} ({} )", view.schema_version);
    println!("  workspace:  {}", view.workspace_root);
    match &view.loop_id {
        Some(id) => println!("  loop_id:    {id}"),
        None => println!("  loop_id:    {yellow}(no marker){reset}"),
    }
    match &view.current_hat {
        Some(h) => println!("  current_hat: {h}"),
        None => println!("  current_hat: {yellow}(unset){reset}"),
    }
    println!(
        "  agent_ctx:  {}",
        if view.is_agent_context {
            "true"
        } else {
            "false"
        }
    );
    println!(
        "  events_file: {} ({} bytes)",
        view.events_file, view.events_size
    );
    println!(
        "  hat_channel: {} ({} bytes)",
        view.hat_channel_file, view.hat_channel_size
    );

    if view.hat_identity.is_null() {
        println!("  hat_identity: {yellow}null{reset}");
    } else {
        println!("  hat_identity:");
        match &view.hat_identity {
            serde_json::Value::Object(map) => {
                if let Some(allowed) = map.get("allowed_task_commands").and_then(|v| v.as_array()) {
                    println!("    allowed_task_commands:");
                    for v in allowed {
                        println!("      - {v}");
                    }
                }
                if let Some(denied) = map.get("denied_task_commands").and_then(|v| v.as_array())
                    && !denied.is_empty()
                {
                    println!("    denied_task_commands:");
                    for v in denied {
                        println!("      - {v}");
                    }
                }
                if let Some(pubs) = map.get("publishes").and_then(|v| v.as_array()) {
                    println!("    publishes:");
                    for v in pubs {
                        println!("      - {v}");
                    }
                }
            }
            _ => println!("    {dim}(unparseable){reset}"),
        }
    }

    if !view.warnings.is_empty() {
        println!("  {yellow}warnings:{reset}");
        for w in &view.warnings {
            println!("    - {w}");
        }
    }
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
    if view.defaults.is_empty() {
        println!("  defaults: {dim}(none){reset}");
    } else {
        println!("  defaults:");
        for d in &view.defaults {
            println!("    - {d}");
        }
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
    use ralph_core::hat_identity::HatIdentitySnapshot;
    use ralph_proto::HatId;
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
            other => panic!("expected Profiles, got {other:?}"),
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
            other => panic!("expected Profiles, got {other:?}"),
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
            other => panic!("expected Profiles, got {other:?}"),
        };
        assert!(profiles_args.no_default_profiles);
    }

    #[test]
    fn cli_parses_inspect_profiles_json_format() {
        let parsed = InspectArgs::try_parse_from(["inspect", "profiles", "--format", "json"])
            .expect("CLI parse failed");
        let profiles_args = match parsed.command.expect("profiles subcommand") {
            InspectCommands::Profiles(p) => p,
            other => panic!("expected Profiles, got {other:?}"),
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
        assert_eq!(defaults, defaults); // sanity: defaults is empty
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
        let specs = [ProfileSpec {
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

    // ─────────────────────────────────────────────────────────────────────
    // U5 — `ralph inspect loop` (read-only diagnostic for OPAC Observe).
    // ─────────────────────────────────────────────────────────────────────

    /// CLI parser coverage for `ralph inspect loop [--hat X] [--format json]`.
    #[test]
    fn cli_parses_inspect_loop_minimal() {
        let parsed = InspectArgs::try_parse_from(["inspect", "loop"]).expect("CLI parse failed");
        let loop_args = match parsed.command.expect("loop subcommand") {
            InspectCommands::Loop(l) => l,
            _ => panic!("expected Loop"),
        };
        assert!(loop_args.hat.is_none());
        assert_eq!(loop_args.format, InspectProfilesFormat::Human);
        assert!(loop_args.root.is_none());
    }

    #[test]
    fn cli_parses_inspect_loop_with_hat_override_and_json() {
        let parsed = InspectArgs::try_parse_from([
            "inspect",
            "loop",
            "--hat",
            "coordinator",
            "--format",
            "json",
        ])
        .expect("CLI parse failed");
        let loop_args = match parsed.command.expect("loop subcommand") {
            InspectCommands::Loop(l) => l,
            _ => panic!("expected Loop"),
        };
        assert_eq!(loop_args.hat.as_deref(), Some("coordinator"));
        assert_eq!(loop_args.format, InspectProfilesFormat::Json);
    }

    /// Schema version is exposed via the constant; pin it so the JSON
    /// shape's compatibility surface is explicit.
    #[test]
    fn loop_inspect_schema_version_pinned() {
        assert_eq!(LOOP_INSPECT_SCHEMA_VERSION, "loop_inspect.v2");
    }

    /// `resolve_event_paths` returns `<root>/.ralph/events.jsonl` for the
    /// main channel and `<root>/.ralph/current-hat-events` for the
    /// hat-channel; both are workspace-rooted.
    #[test]
    fn resolve_event_paths_uses_workspace_relative() {
        let tmp = TempDir::new().expect("temp dir");
        let ctx = crate::operation_guard::OperationContext::detect_with_env(
            tmp.path().to_path_buf(),
            |_| None,
        );
        let (main, hat_channel) = resolve_event_paths(tmp.path(), &ctx);
        assert_eq!(main, tmp.path().join(".ralph/events.jsonl"));
        assert_eq!(hat_channel, tmp.path().join(".ralph/current-hat-events"));
    }

    /// Empty workspace + no marker → warnings list is non-empty (no
    /// loop-id + no hat) and JSON shape stays stable.
    #[test]
    fn inspect_loop_view_stable_when_no_markers() {
        // Build the view directly so we cover the warning paths without
        // touching async / config loading.
        let tmp = TempDir::new().expect("temp dir");
        let ctx = crate::operation_guard::OperationContext::detect_with_env(
            tmp.path().to_path_buf(),
            |_| None,
        );
        let (main, hat_channel) = resolve_event_paths(tmp.path(), &ctx);
        let view = LoopInspectView {
            workspace_root: tmp.path().display().to_string(),
            loop_id: None,
            current_hat: None,
            is_agent_context: false,
            hat_identity: serde_json::Value::Null,
            events_file: main.display().to_string(),
            hat_channel_file: hat_channel.display().to_string(),
            events_size: 0,
            hat_channel_size: 0,
            warnings: vec![
                "no current-loop-id marker at <root>/.ralph/current-loop-id; \
                 run `ralph run ...` to create one"
                    .to_string(),
                "no current hat in environment (RALPH_CURRENT_HAT unset) and no --hat override; \
                 hat_identity will be null. Pass `--hat <id>` to preview a hat identity."
                    .to_string(),
            ],
            schema_version: LOOP_INSPECT_SCHEMA_VERSION.to_string(),
            loop_anchor: None,
            supervisor: None,
        };
        assert!(view.loop_id.is_none());
        assert!(view.current_hat.is_none());
        assert_eq!(view.warnings.len(), 2);
        let json = serde_json::to_value(&view).expect("serialise");
        assert_eq!(json["schema_version"], serde_json::json!("loop_inspect.v2"));
        assert_eq!(json["loop_id"], serde_json::Value::Null);
        assert_eq!(json["hat_identity"], serde_json::Value::Null);
    }

    /// Hat identity block appears in the JSON output when a known hat
    /// is resolved (R5 / U1 SSOT).
    #[test]
    fn inspect_loop_view_includes_hat_identity_when_known() {
        use ralph_core::config::hat::HatConfig;

        let mut cfg = RalphConfig::default();
        cfg.tasks.coordinator_hats = vec!["coordinator".to_string()];
        cfg.hats.insert(
            "coordinator".to_string(),
            HatConfig {
                publishes: vec!["work.ready".to_string(), "work.done".to_string()],
                ..HatConfig::default()
            },
        );

        let snapshot =
            HatIdentitySnapshot::from_config(&cfg, &HatId::new("coordinator".to_string()))
                .expect("snapshot for known hat");
        let view = LoopInspectView {
            workspace_root: "/tmp/x".into(),
            loop_id: Some("loop-x".into()),
            current_hat: Some("coordinator".into()),
            is_agent_context: true,
            hat_identity: snapshot.to_json(),
            events_file: "/tmp/x/.ralph/events.jsonl".into(),
            hat_channel_file: "/tmp/x/.ralph/current-hat-events".into(),
            events_size: 0,
            hat_channel_size: 0,
            warnings: vec![],
            schema_version: LOOP_INSPECT_SCHEMA_VERSION.to_string(),
            loop_anchor: None,
            supervisor: None,
        };

        let json = serde_json::to_value(&view).expect("serialise");
        assert_eq!(json["current_hat"], serde_json::json!("coordinator"));
        let identity = &json["hat_identity"];
        assert_eq!(identity["hat_id"], serde_json::json!("coordinator"));
        assert_eq!(identity["is_coordinator"], serde_json::json!(true));
        let allowed = identity["allowed_task_commands"].as_array().unwrap();
        assert!(
            allowed.iter().any(|v| v.as_str() == Some("add")),
            "coord allowed_task_commands missing 'add': {allowed:?}"
        );
        let denied = identity["denied_task_commands"].as_array().unwrap();
        assert!(denied.is_empty(), "coordinator denied list must be empty");
    }

    // ─────────────────────────────────────────────────────────────────────
    // U22 — `ralph inspect loop` supervisor summary block.
    // ─────────────────────────────────────────────────────────────────────

    /// Supervisor disabled + no ledger on disk → no `supervisor` key in JSON.
    #[test]
    fn build_supervisor_summary_omitted_when_disabled() {
        let cfg = RalphConfig::default();
        let tmp = TempDir::new().expect("temp dir");
        let out = build_supervisor_summary(&cfg, tmp.path());
        assert!(out.is_none());
    }

    /// Supervisor enabled + no db file → default summary (empty active_waves).
    #[test]
    fn build_supervisor_summary_enabled_no_db_returns_default() {
        let mut cfg = RalphConfig::default();
        cfg.event_loop.supervisor.enabled = true;
        let tmp = TempDir::new().expect("temp dir");
        let out = build_supervisor_summary(&cfg, tmp.path());
        let summary = out.expect("enabled must yield Some");
        assert!(summary.active_waves.is_empty());
        assert_eq!(summary.queue_depth, 0);
    }

    /// Seed `<tmp>/.ralph/supervisor.db` with one active Exec wave so
    /// gate tests can exercise the "ledger on disk" branch against a
    /// real rusqlite store (C4: "exists with at least one wave row").
    #[cfg(feature = "supervisor-db")]
    fn seed_ledger_with_one_wave(tmp: &TempDir) {
        use ralph_core::supervisor::{RusqliteSupervisorStore, SupervisorStore, WaveKind};
        let ralph_dir = tmp.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("create .ralph dir");
        let db_path = ralph_dir.join("supervisor.db");
        // Scope so the seeding connection is released before
        // `build_supervisor_summary` re-opens the store read-only.
        let store = RusqliteSupervisorStore::open(&db_path).expect("open store");
        store
            .register_wave("u4-ledger", WaveKind::Exec, 1, 1)
            .expect("register_wave");
    }

    /// R8 / KTD6 / AE6: supervisor disabled **but** a ledger file with
    /// at least one wave row exists on disk → the summary block is
    /// still emitted (default-wave lazy-open path), surfacing the
    /// active wave — while never leaking db_path / internal ledger
    /// fields (R11).
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn build_supervisor_summary_disabled_with_ledger_on_disk_yields_summary() {
        let cfg = RalphConfig::default();
        assert!(
            !cfg.event_loop.supervisor.enabled,
            "precondition: default config has supervisor disabled"
        );
        let tmp = TempDir::new().expect("temp dir");
        seed_ledger_with_one_wave(&tmp);

        let out = build_supervisor_summary(&cfg, tmp.path());
        let summary = out.expect("disabled + db on disk must yield Some (R8/KTD6)");
        assert_eq!(
            summary.active_waves.len(),
            1,
            "ledger wave must surface even when enabled=false"
        );

        // Output safety (R11): the struct must NOT leak any path / db
        // or internal ledger field — same contract as line-1861 pins.
        let json = serde_json::to_value(&summary).expect("serialise");
        assert!(json.get("db_path").is_none(), "must not leak db_path");
        assert!(json.get("event_log").is_none(), "must not leak event_log");
    }

    /// enabled=true + db on disk → live summary from the store (the
    /// enabled path does not regress under the new ledger-aware gate).
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn build_supervisor_summary_enabled_with_ledger_on_disk_yields_summary() {
        let mut cfg = RalphConfig::default();
        cfg.event_loop.supervisor.enabled = true;
        let tmp = TempDir::new().expect("temp dir");
        seed_ledger_with_one_wave(&tmp);

        let out = build_supervisor_summary(&cfg, tmp.path());
        let summary = out.expect("enabled + db must yield Some");
        assert_eq!(summary.active_waves.len(), 1);

        let json = serde_json::to_value(&summary).expect("serialise");
        assert!(json.get("db_path").is_none(), "must not leak db_path");
        assert!(json.get("event_log").is_none(), "must not leak event_log");
    }

    // ─────────────────────────────────────────────────────────────────────
    // U8 of plan 2026-07-04-002 — `ralph inspect loop` supervisor summary
    // must populate `slot_summary[]` and `last_coordination_topics[]`
    // (R11 supervisor 摘要部分). The `build_supervisor_summary` helper
    // needs a real rusqlite db on disk; for unit-level coverage we drive
    // `ralph_core::supervisor::summarize` directly with an
    // `InMemorySupervisorStore` (the trait object contract is identical
    // for the live store, so the field-population contract is what we
    // are pinning here).
    // ─────────────────────────────────────────────────────────────────────

    /// Supervisor enabled + one active Exec wave + a slot → `slot_summary`
    /// contains the slot with stable `hat` and `status` fields, and
    /// `last_coordination_topics` lists the matching Exec kind's
    /// coordination topics (no db path / internal ledger leakage).
    #[test]
    fn summarize_populates_slot_and_coordination_topics_for_active_wave() {
        use ralph_core::supervisor::{
            InMemorySupervisorStore, SupervisorStore, WaveKind, summarize,
        };
        let store = InMemorySupervisorStore::new();
        let wave = store
            .register_wave("u8-summarize", WaveKind::Exec, 2, 1)
            .expect("register_wave");
        // Bind both slots so dispatch is allowed; then mark slot 0
        // Completed and slot 1 still Pending so we have a mix.
        store
            .bind_worktree(
                &wave,
                0,
                ralph_core::supervisor::SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/u8/0".to_string()),
                    branch: Some("ralph/u8-0".to_string()),
                },
            )
            .unwrap();
        store
            .bind_worktree(
                &wave,
                1,
                ralph_core::supervisor::SlotResource {
                    slot_index: 1,
                    worktree_path: Some(".ralph/u8/1".to_string()),
                    branch: Some("ralph/u8-1".to_string()),
                },
            )
            .unwrap();
        store.record_slot_result(&wave, 0, "h0", 1).unwrap();

        let summary = summarize(&store);
        assert_eq!(summary.active_waves.len(), 1);
        assert_eq!(summary.active_waves[0].wave_id, wave);
        // One pending slot (slot 1 still Pending after slot 0 Completed).
        assert_eq!(summary.queue_depth, 1);
        assert_eq!(summary.slot_summary.len(), 2);
        // Slot 0 → completed; slot 1 → pending.
        assert_eq!(summary.slot_summary[0].slot_id, 0);
        assert_eq!(summary.slot_summary[0].status, "completed");
        assert_eq!(summary.slot_summary[0].hat, "exec-worker");
        assert_eq!(summary.slot_summary[1].slot_id, 1);
        assert_eq!(summary.slot_summary[1].status, "pending");
        assert_eq!(summary.slot_summary[1].hat, "exec-worker");
        // Exec wave → two Exec coordination topics, in stable order.
        assert_eq!(
            summary.last_coordination_topics,
            vec![
                "exec.wave.complete".to_string(),
                "exec.wave.failed".to_string()
            ],
        );
        // Output-safety: the struct must NOT leak any path / db fields.
        let json = serde_json::to_value(&summary).expect("serialise");
        assert!(json.get("db_path").is_none(), "must not leak db_path");
        assert!(json.get("event_log").is_none(), "must not leak event_log");
    }

    /// No active waves → default summary (empty `slot_summary`,
    /// empty `last_coordination_topics`). The agent-safe Observe
    /// contract is "non-empty wave list → non-empty fields", so the
    /// empty case stays empty rather than fabricating topics.
    #[test]
    fn summarize_empty_store_yields_empty_slot_and_coordination_fields() {
        use ralph_core::supervisor::{InMemorySupervisorStore, summarize};
        let store = InMemorySupervisorStore::new();
        let summary = summarize(&store);
        assert!(summary.active_waves.is_empty());
        assert_eq!(summary.queue_depth, 0);
        assert!(summary.slot_summary.is_empty());
        assert!(summary.last_coordination_topics.is_empty());
    }

    /// Fix wave → `last_coordination_topics` lists Fix coordination
    /// topics; `slot_summary` is empty when zero waves are active.
    /// Pin the per-kind topic derivation contract.
    #[test]
    fn summarize_fix_wave_uses_fix_coordination_topics() {
        use ralph_core::supervisor::{
            InMemorySupervisorStore, SupervisorStore, WaveKind, summarize,
        };
        let store = InMemorySupervisorStore::new();
        store
            .register_wave("u8-fix", WaveKind::Fix, 1, 1)
            .expect("register_wave");
        let summary = summarize(&store);
        assert_eq!(
            summary.last_coordination_topics,
            vec![
                "fix.wave.complete".to_string(),
                "fix.wave.failed".to_string()
            ],
        );
    }

    /// The default (no active waves) summary serialises with the
    /// stable agent-safe JSON shape: `active_waves: []`,
    /// `queue_depth: 0`, `slot_summary: []`,
    /// `last_coordination_topics: []`. Pin against the schema so the
    /// `loop_inspect.v1` JSON consumers can rely on the field set.
    #[test]
    fn supervisor_summary_default_json_shape_is_stable() {
        let s = ralph_core::supervisor::SupervisorInspectSummary::default();
        let json = serde_json::to_value(&s).expect("serialise");
        assert_eq!(json["active_waves"], serde_json::json!([]));
        assert_eq!(json["queue_depth"], serde_json::json!(0));
        assert_eq!(json["slot_summary"], serde_json::json!([]));
        assert_eq!(json["last_coordination_topics"], serde_json::json!([]));
    }

    // ─────────────────────────────────────────────────────────────────────
    // U1 of plan 2026-07-04-004 — `ralph inspect loop` exposes a
    // `loop_anchor` block describing the attached plan (P0-1). Schema
    // bumped from `loop_inspect.v1` → `loop_inspect.v2`. Tests cover
    // attached vs. unattached serialisation + warning semantics so
    // downstream consumers can rely on the field set without
    // re-reading the inspect source.
    // ─────────────────────────────────────────────────────────────────────

    /// `LOOP_INSPECT_SCHEMA_VERSION` is bumped to `loop_inspect.v2`
    /// when the field set changes shape (U1 / P0-1).
    #[test]
    fn test_loop_inspect_schema_version_bumped_to_v2() {
        assert_eq!(LOOP_INSPECT_SCHEMA_VERSION, "loop_inspect.v2");
    }

    // ─────────────────────────────────────────────────────────────────
    // U6 of plan 2026-07-05-005 (R4): resume-path anchor marker.
    // The marker is the SSoT for the resume path; the
    // prompt-file-extension check stays as the fallback for the
    // `ralph run --plan` path.
    // ─────────────────────────────────────────────────────────────────

    fn write_marker(tmp: &TempDir, plan_path: &std::path::Path) {
        let agent_dir = tmp.path().join(".ralph").join("agent");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        let marker = AnchorMarker {
            plan_path: plan_path.to_path_buf(),
            plan_name: plan_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string(),
            plan_baseline_sha: Some("deadbeef".to_string()),
            attached_at: Some("2026-07-05T00:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&marker).expect("serialise");
        std::fs::write(agent_dir.join(".ralph-anchor.json"), json).expect("write marker");
    }

    #[test]
    fn u6_anchor_marker_present_is_returned() {
        let tmp = TempDir::new().expect("temp dir");
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "# plan").expect("write plan");
        write_marker(&tmp, &plan_path);

        // Even with the default config (prompt_file is sentinel),
        // the marker takes precedence.
        let cfg = RalphConfig::default();
        let anchor = build_loop_anchor_summary(&cfg, tmp.path(), None)
            .expect("anchor must be Some when marker is present");
        assert_eq!(anchor.plan_path, plan_path);
        assert_eq!(anchor.plan_name, "plan");
        assert_eq!(anchor.plan_baseline_sha.as_deref(), Some("deadbeef"));
        assert!(anchor.attached_at.is_some());
    }

    #[test]
    fn u6_anchor_marker_missing_returns_none_when_no_prompt() {
        let tmp = TempDir::new().expect("temp dir");
        // No marker AND prompt_file is sentinel → unattached.
        let cfg = RalphConfig::default();
        let anchor = build_loop_anchor_summary(&cfg, tmp.path(), None);
        assert!(
            anchor.is_none(),
            "missing marker + sentinel prompt_file must surface unattached"
        );
    }

    #[test]
    fn u6_anchor_marker_corrupt_falls_back_to_prompt_extension() {
        let tmp = TempDir::new().expect("temp dir");
        let agent_dir = tmp.path().join(".ralph").join("agent");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        std::fs::write(agent_dir.join(".ralph-anchor.json"), "{ not json").expect("write");

        // Fallback to prompt-file-extension path: a .md prompt
        // file is still parsed and returned.
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "# plan").expect("write plan");
        let mut cfg = RalphConfig::default();
        cfg.event_loop.prompt_file = plan_path.to_string_lossy().to_string();

        let anchor = build_loop_anchor_summary(&cfg, tmp.path(), None)
            .expect("anchor must fall back to prompt extension");
        assert_eq!(anchor.plan_path, plan_path);
    }

    #[test]
    fn u6_read_anchor_marker_returns_none_for_missing_file() {
        let tmp = TempDir::new().expect("temp dir");
        assert!(read_anchor_marker(tmp.path()).is_none());
    }

    /// When a plan is attached (`prompt_file` is a `.md` under
    /// `docs/plans/`) the JSON contains `loop_anchor` with the
    /// five canonical fields. `plan_baseline_sha` is `None` when no
    /// baseline marker is on disk; `attached_at` is `None` when no
    /// matching `loops.json` entry exists; `loop_start_sha` is
    /// omitted entirely via `skip_serializing_if`.
    #[test]
    fn test_inspect_loop_view_includes_loop_anchor_when_attached() {
        let tmp = TempDir::new().expect("temp dir");
        let plan_dir = tmp.path().join("docs").join("plans");
        std::fs::create_dir_all(&plan_dir).expect("plan dir");
        let plan_path = plan_dir.join("2026-07-04-004-feat-single-chain-refactor-plan.md");
        std::fs::write(&plan_path, "# plan").expect("write plan");

        let mut cfg = RalphConfig::default();
        cfg.event_loop.prompt_file = plan_path.to_string_lossy().to_string();

        let anchor = build_loop_anchor_summary(&cfg, tmp.path(), None)
            .expect("anchor must be Some when prompt_file points at a plan");
        assert_eq!(anchor.plan_path, plan_path);
        assert_eq!(
            anchor.plan_name,
            "2026-07-04-004-feat-single-chain-refactor-plan"
        );
        assert!(anchor.plan_baseline_sha.is_none());
        assert!(anchor.loop_start_sha.is_none());
        assert!(anchor.attached_at.is_none());

        // Serialise the anchor struct and confirm the JSON shape.
        let json = serde_json::to_value(&anchor).expect("serialise");
        assert_eq!(
            json["plan_name"],
            serde_json::json!("2026-07-04-004-feat-single-chain-refactor-plan")
        );
        assert_eq!(json["plan_path"], serde_json::json!(plan_path));
        assert_eq!(json["plan_baseline_sha"], serde_json::Value::Null);
        // `loop_start_sha` and `attached_at` are `skip_serializing_if`,
        // so they are omitted entirely (not present at all in the
        // serialised form). Pinning that contract prevents accidental
        // null-flooding of the JSON payload.
        assert!(
            json.get("loop_start_sha").is_none(),
            "loop_start_sha must be skipped when None"
        );
        assert!(
            json.get("attached_at").is_none(),
            "attached_at must be skipped when None"
        );
    }

    /// When no plan path is on the loaded config (`prompt_file` is
    /// the default sentinel `"PROMPT.md"`) the `loop_anchor` is
    /// `None` so the JSON key is omitted entirely (forward compat
    /// with v1 consumers; per `skip_serializing_if =
    /// "Option::is_none"`).
    #[test]
    fn test_inspect_loop_view_omits_loop_anchor_when_unattached() {
        let tmp = TempDir::new().expect("temp dir");
        let cfg = RalphConfig::default();
        // Default config has prompt_file == "PROMPT.md"; explicitly
        // confirm the helper short-circuits in that state.
        assert_eq!(cfg.event_loop.prompt_file, "PROMPT.md");

        let anchor = build_loop_anchor_summary(&cfg, tmp.path(), None);
        assert!(anchor.is_none(), "default RalphConfig has no plan attached");

        // Render through the same view struct `inspect_loop_command`
        // builds so the JSON-shape contract is pinned end-to-end.
        let view = LoopInspectView {
            workspace_root: tmp.path().display().to_string(),
            loop_id: None,
            current_hat: None,
            is_agent_context: false,
            hat_identity: serde_json::Value::Null,
            events_file: tmp.path().join(".ralph/events.jsonl").display().to_string(),
            hat_channel_file: tmp
                .path()
                .join(".ralph/current-hat-events")
                .display()
                .to_string(),
            events_size: 0,
            hat_channel_size: 0,
            warnings: vec![],
            schema_version: LOOP_INSPECT_SCHEMA_VERSION.to_string(),
            loop_anchor: None,
            supervisor: None,
        };
        let json = serde_json::to_value(&view).expect("serialise");
        assert!(
            json.get("loop_anchor").is_none(),
            "JSON must omit the `loop_anchor` key when None"
        );
    }

    /// Inspect commands which cannot resolve a plan anchor must push
    /// a stable warning string into the warnings list so operators
    /// have an actionable hint instead of silently dropping the
    /// field. The canonical warning is exported via
    /// [`loop_anchor_unattached_warning`] so tests and runtime
    /// reference the same literal.
    #[test]
    fn test_loop_anchor_warning_when_unattached() {
        let warning = loop_anchor_unattached_warning();
        let needle = "loop_anchor not attached";
        assert!(
            warning.contains(needle),
            "warning literal must contain `{needle}` so downstream lint and BDD scenarios can match"
        );
        // Pin the actionable hint so operators know how to recover.
        assert!(
            warning.contains("--plan"),
            "warning must mention `--plan` as the recovery hint"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// Unit 2 of plan 2026-07-27-002: CLI parsing for --topic/--triggered.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn cli_parses_inspect_prompt_minimal() {
    let parsed = InspectArgs::try_parse_from(["inspect", "prompt", "--hat", "worker"])
        .expect("CLI parse failed");
    let prompt_args = match parsed.command.expect("prompt subcommand") {
        InspectCommands::Prompt(p) => p,
        other => panic!("expected Prompt, got {other:?}"),
    };
    assert_eq!(prompt_args.hat, "worker");
    assert!(prompt_args.topic.is_none());
    assert!(prompt_args.triggered.is_none());
}

#[test]
fn cli_parses_inspect_prompt_with_topic_and_triggered() {
    let parsed = InspectArgs::try_parse_from([
        "inspect",
        "prompt",
        "--hat",
        "reviewer",
        "--topic",
        "work.ready",
        "--payload",
        r#"{"task_key": "abc"}"#,
        "--triggered",
        "worker",
    ])
    .expect("CLI parse failed");
    let prompt_args = match parsed.command.expect("prompt subcommand") {
        InspectCommands::Prompt(p) => p,
        other => panic!("expected Prompt, got {other:?}"),
    };
    assert_eq!(prompt_args.hat, "reviewer");
    assert_eq!(prompt_args.topic.as_deref(), Some("work.ready"));
    assert_eq!(prompt_args.triggered.as_deref(), Some("worker"));
}

#[test]
fn cli_parses_inspect_prompt_topic_without_triggered() {
    let parsed = InspectArgs::try_parse_from([
        "inspect",
        "prompt",
        "--hat",
        "worker",
        "--topic",
        "work.ready",
        "--payload",
        r#"{"task_key": "abc"}"#,
    ])
    .expect("CLI parse failed");
    let prompt_args = match parsed.command.expect("prompt subcommand") {
        InspectCommands::Prompt(p) => p,
        other => panic!("expected Prompt, got {other:?}"),
    };
    assert_eq!(prompt_args.topic.as_deref(), Some("work.ready"));
    assert!(prompt_args.triggered.is_none());
}

#[test]
fn cli_parses_inspect_prompt_triggered_without_topic_is_ok() {
    let parsed = InspectArgs::try_parse_from([
        "inspect",
        "prompt",
        "--hat",
        "worker",
        "--triggered",
        "builder",
    ])
    .expect("CLI parse failed");
    let prompt_args = match parsed.command.expect("prompt subcommand") {
        InspectCommands::Prompt(p) => p,
        other => panic!("expected Prompt, got {other:?}"),
    };
    assert!(prompt_args.topic.is_none());
    assert_eq!(prompt_args.triggered.as_deref(), Some("builder"));
}

// ─────────────────────────────────────────────────────────────────────
// Unit 1 (plan 2026-07-27-002): fail-loud on JSON deserialization error.
// Before the fix `--wave-context` / `--correction` silently fell through
// to `None` via `.ok()`.  After the fix they must return an `Err`
// containing the field name and the serde error message.
// ─────────────────────────────────────────────────────────────────────

/// WaveContext requires `wave_id`, `wave_total`, `received_count`,
/// `expected_dimensions`, `pending_dimensions` fields.  A JSON that
/// parses as valid syntax but has the wrong shape must NOT silently
/// become `None` — it must return an error mentioning `--wave-context`.
#[test]
fn inspect_prompt_wave_context_wrong_shape_is_err() {
    let parsed = InspectArgs::try_parse_from([
        "inspect",
        "prompt",
        "--hat",
        "worker",
        "--wave-context",
        r#"{"wrong_field": "foo"}"#,
    ])
    .expect("CLI parse failed");
    let prompt_args = match parsed.command.expect("prompt subcommand") {
        InspectCommands::Prompt(p) => p,
        other => panic!("expected Prompt, got {other:?}"),
    };
    // The CLI parser accepts any string for --wave-context; the
    // deserialization error is raised inside `inspect_prompt_command`.
    // We test that the parsed args shape is correct for passing to the
    // command.
    assert!(prompt_args.wave_context.is_some());
    let json_input = prompt_args.wave_context.unwrap();
    // Valid JSON syntax, wrong shape for WaveContext → must fail loud.
    let result: Result<ralph_core::wave_context::WaveContext, _> =
        serde_json::from_str(&json_input);
    assert!(
        result.is_err(),
        "wrong-shape JSON for WaveContext must error, got Ok"
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("wave_id") || err.to_string().contains("missing field"),
        "error message should mention the missing field: {}",
        err
    );
}

/// CorrectionContext requires `reason_code`, `stage`, `topic` fields.
/// A JSON that parses but doesn't match the struct must return an error
/// mentioning `--correction`.
#[test]
fn inspect_prompt_correction_wrong_shape_is_err() {
    let parsed = InspectArgs::try_parse_from([
        "inspect",
        "prompt",
        "--hat",
        "worker",
        "--correction",
        r#"{"not_a_real_field": 123}"#,
    ])
    .expect("CLI parse failed");
    let prompt_args = match parsed.command.expect("prompt subcommand") {
        InspectCommands::Prompt(p) => p,
        other => panic!("expected Prompt, got {other:?}"),
    };
    assert!(prompt_args.correction.is_some());
    let json_input = prompt_args.correction.unwrap();
    let result: Result<ralph_core::correction::CorrectionContext, _> =
        serde_json::from_str(&json_input);
    assert!(
        result.is_err(),
        "wrong-shape JSON for CorrectionContext must error, got Ok"
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("reason_code") || err.to_string().contains("missing field"),
        "error message should mention the missing field: {}",
        err
    );
}

/// Valid JSON that happens to be a different type (e.g. a plain string)
/// is also a deserialization error for WaveContext.
#[test]
fn inspect_prompt_wave_context_wrong_type_is_err() {
    let json_input = r#""just a string, not an object""#;
    let result: Result<ralph_core::wave_context::WaveContext, _> = serde_json::from_str(json_input);
    assert!(
        result.is_err(),
        "string JSON for WaveContext struct must error, got Ok"
    );
}
