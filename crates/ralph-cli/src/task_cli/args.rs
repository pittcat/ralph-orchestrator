use crate::{ConfigSource, config_resolution};
use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

/// U7 (2026-07-04-003 plan): distinguishable failure modes for
/// `load_coordinator_hats`.
///
/// Each variant tells the operator (or upstream caller like
/// `HatCommandPolicy::check_task`) exactly what is missing in the
/// workspace so they can apply the right fix without re-reading
/// `ralph.yml`. This is the file-local SSOT counterpart to
/// `hat_command_policy::ConfigFault`: `ConfigFault` is consumed by
/// the policy layer; `CoordinatorHatsError` is the typed error
/// returned by the disk-reading loader here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorHatsError {
    /// `ralph.yml` (or `ralph.yaml`) does not exist in the workspace.
    MissingRalphYml,
    /// `ralph.yml` exists but cannot be parsed as YAML.
    InvalidYaml { path: PathBuf, source: String },
    /// `ralph.yml` parses but declares no `tasks:` section.
    MissingTasksSection,
    /// `ralph.yml` declares `tasks:` but no `coordinator_hats` key.
    MissingCoordinatorHatsKey,
    /// `tasks.coordinator_hats` is present but empty (`[]`).
    CoordinatorHatsEmpty,
}

impl std::fmt::Display for CoordinatorHatsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // 2026-07-13-001 plan U3 / U5: reframe the "no config
            // found" hint so it advertises every supported path
            // (`-c`, `RALPH_CONFIG`, `ralph.yml`, `ralph.yaml`)
            // rather than telling the operator to symlink their
            // custom file to `ralph.yml`.
            Self::MissingRalphYml => f.write_str(
                "no project config found (looked for -c file, $RALPH_CONFIG, ralph.yml, ralph.yaml); \
                 pass `ralph -c <file> …`, export RALPH_CONFIG, or add ralph.yml with tasks.coordinator_hats",
            ),
            Self::InvalidYaml { path, source } => write!(
                f,
                "ralph.yml at {} is not valid YAML: {}",
                path.display(),
                source
            ),
            Self::MissingTasksSection => f.write_str("ralph.yml has no `tasks:` section"),
            Self::MissingCoordinatorHatsKey => f.write_str(
                "ralph.yml `tasks:` block exists but does not declare `coordinator_hats`",
            ),
            Self::CoordinatorHatsEmpty => f.write_str("tasks.coordinator_hats is empty"),
        }
    }
}

impl std::error::Error for CoordinatorHatsError {}

/// U7 (2026-07-04-003 plan): load `tasks.coordinator_hats` from
/// `ralph.yml` and surface the *shape* of the failure as a typed
/// `CoordinatorHatsError` instead of silently returning an empty
/// `Vec<String>`.
///
/// Search order:
/// 1. `<root>/ralph.yml`
/// 2026-07-13-001 plan U3: load `tasks.coordinator_hats` from
/// the project config and surface the *shape* of the failure as a
/// typed `CoordinatorHatsError` instead of silently returning an
/// empty `Vec<String>`.
///
/// Discovery order:
/// 1. `ConfigSource::File` paths passed via `-c`
/// 2. `$RALPH_CONFIG`
/// 3. `<root>/ralph.yml`
/// 4. `<root>/ralph.yaml`
///
/// Returns `Ok(Vec<String>)` on success. On any failure (missing
/// file, invalid YAML, missing/empty key) the typed error is
/// returned — callers are expected to convert it into a structured
/// `PolicyDecision::Deny { reason, hint }` rather than swallow it.
///
/// The loader intentionally bypasses `RalphConfig` so it can
/// disambiguate "no file" vs "no tasks section" vs "no key" vs
/// "empty value" — all four collapse to `coordinator_hats == []`
/// when read through `RalphConfig::default()`.
pub fn load_coordinator_hats(
    root: &Path,
    config_sources: &[ConfigSource],
) -> Result<Vec<String>, CoordinatorHatsError> {
    if let Some(resolved) = config_resolution::resolve_project_config_path(root, config_sources) {
        return load_coordinator_hats_from_path(&resolved);
    }
    Err(CoordinatorHatsError::MissingRalphYml)
}

/// 2026-07-13-001 plan U3: load `coordinator_hats` from an
/// already-resolved project config path. The helper is shared
/// between the explicit `-c`/`RALPH_CONFIG` discovery path and any
/// future caller that already has a `Path` in hand.
pub fn load_coordinator_hats_from_path(path: &Path) -> Result<Vec<String>, CoordinatorHatsError> {
    if !path.exists() {
        return Err(CoordinatorHatsError::MissingRalphYml);
    }
    let raw = std::fs::read_to_string(path).map_err(|e| CoordinatorHatsError::InvalidYaml {
        path: path.to_path_buf(),
        source: e.to_string(),
    })?;
    let value: serde_yaml::Value =
        serde_yaml::from_str(&raw).map_err(|e| CoordinatorHatsError::InvalidYaml {
            path: path.to_path_buf(),
            source: e.to_string(),
        })?;

    let tasks = match value.get("tasks") {
        Some(t) => t,
        None => return Err(CoordinatorHatsError::MissingTasksSection),
    };
    let coordinator_hats = match tasks.get("coordinator_hats") {
        Some(c) => c,
        None => return Err(CoordinatorHatsError::MissingCoordinatorHatsKey),
    };
    let hats: Vec<String> = serde_yaml::from_value(coordinator_hats.clone()).map_err(|e| {
        CoordinatorHatsError::InvalidYaml {
            path: path.to_path_buf(),
            source: e.to_string(),
        }
    })?;
    if hats.is_empty() {
        return Err(CoordinatorHatsError::CoordinatorHatsEmpty);
    }
    Ok(hats)
}

/// Output format for task commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table format
    #[default]
    Table,
    /// JSON format for programmatic access
    Json,
    /// ID-only output for scripting
    Quiet,
}

/// Task management commands for tracking work items.
#[derive(Parser, Debug)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub command: TaskCommands,

    /// Working directory (default: current directory)
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum TaskCommands {
    /// Create a new task
    Add(AddArgs),

    /// Create or reuse a task by stable key
    Ensure(EnsureArgs),

    /// List all tasks
    List(ListArgs),

    /// Show unblocked tasks
    Ready(ReadyArgs),

    /// Mark a task as in progress
    Start(StartArgs),

    /// Mark a task as complete
    Close(CloseArgs),

    /// Mark a task as failed
    Fail(FailArgs),

    /// Reopen a closed or failed task
    Reopen(ReopenArgs),

    /// OPAC Precheck: verify a task mutation would succeed without writing
    Verify(VerifyArgs),

    /// OPAC Precheck: verify the three-field task_id/task_key/step emit-bridge
    VerifyEmitBridge(VerifyEmitBridgeArgs),

    /// Show a single task by ID
    Show(ShowArgs),

    /// Consume a pending confirmation recorded by a protected add/ensure
    Confirm(ConfirmArgs),
}

/// Arguments for the `task add` command.
#[derive(Parser, Debug)]
pub struct AddArgs {
    /// Task title
    pub title: String,

    /// Priority (1-5, default 3)
    #[arg(short = 'p', long, default_value = "3")]
    pub priority: u8,

    /// Task description
    #[arg(short = 'd', long)]
    pub description: Option<String>,

    /// Task IDs that must complete first (comma-separated)
    #[arg(long)]
    pub blocked_by: Option<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `task ensure` command.
#[derive(Parser, Debug)]
pub struct EnsureArgs {
    /// Task title
    pub title: String,

    /// Stable key used to deduplicate orchestrator-managed tasks.
    /// Mutually exclusive with `--for-fix-unit`: pass either `--key`
    /// OR `--for-fix-unit`, never both.
    #[arg(long, required_unless_present = "for_fix_unit")]
    pub key: Option<String>,

    /// Priority (1-5, default 3)
    #[arg(short = 'p', long, default_value = "3")]
    pub priority: u8,

    /// Task description
    #[arg(short = 'd', long)]
    pub description: Option<String>,

    /// Task IDs that must complete first (comma-separated)
    #[arg(long)]
    pub blocked_by: Option<String>,

    /// 2026-06-28-002 U8: auto-derive the canonical fix-unit key
    /// from `plan:fix_step:slug` (e.g. `myplan:fix-02:patch-foo`)
    /// AND pin the owner to `coordinator`. The coordinator then
    /// uses the returned task_id in subsequent `work.ready`
    /// emits so `work.done` no longer collides with the legacy
    /// `task-fix-01-placeholder` contract. Mutually exclusive
    /// with `--key`: pass either `--key` OR `--for-fix-unit`,
    /// never both.
    #[arg(long, value_name = "PLAN:FIX_STEP:SLUG", conflicts_with = "key")]
    pub for_fix_unit: Option<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `task list` command.
#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Filter by status: open, in_progress, closed, failed
    #[arg(short = 's', long)]
    pub status: Option<String>,

    /// Show only tasks from the last N days
    #[arg(long, short = 'd')]
    pub days: Option<i64>,

    /// Limit the number of tasks displayed
    #[arg(long, short = 'l')]
    pub limit: Option<usize>,

    /// Show all tasks including closed and failed (hidden by default)
    #[arg(long, short = 'a')]
    pub all: bool,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `task ready` command.
#[derive(Parser, Debug)]
pub struct ReadyArgs {
    /// Show tasks from all loops, not just the current one
    #[arg(long, short = 'a')]
    pub all: bool,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `task start` command.
#[derive(Parser, Debug)]
pub struct StartArgs {
    /// Task ID to mark as in progress
    pub id: String,
}

/// Arguments for the `task close` command.
#[derive(Parser, Debug)]
pub struct CloseArgs {
    /// Task ID to close
    pub id: String,
}

/// Arguments for the `task confirm` command (Unit 1 task confirmation).
#[derive(Parser, Debug)]
pub struct ConfirmArgs {
    /// Task ID whose pending confirmation should be consumed
    pub id: String,

    /// Confirmation reference printed by the protected Apply
    #[arg(long)]
    pub reference: String,

    /// Confirmation digest (the mutation fingerprint recorded at Apply)
    #[arg(long)]
    pub digest: String,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `task fail` command.
#[derive(Parser, Debug)]
pub struct FailArgs {
    /// Task ID to mark as failed
    pub id: String,
}

/// Arguments for the `task reopen` command.
#[derive(Parser, Debug)]
pub struct ReopenArgs {
    /// Task ID to reopen
    pub id: String,
}

/// Arguments for the `task show` command.
#[derive(Parser, Debug)]
pub struct ShowArgs {
    /// Task ID
    pub id: String,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

// ─────────────────────────────────────────────────────────────────────────
// U4: OPAC Precheck — `task verify <verb>` and `task verify-emit-bridge`.
// ─────────────────────────────────────────────────────────────────────────

/// Outer args for `ralph tools task verify <verb>`.
///
/// Mirrors the `TaskArgs` shape (`Parser` + `#[command(subcommand)]` field)
/// so `Verify(VerifyArgs)` slots into `TaskCommands` cleanly. The shared
/// `--root` flag lets agents redirect verify to another workspace's store
/// the same way `task add` does.
#[derive(Parser, Debug)]
pub struct VerifyArgs {
    #[command(subcommand)]
    pub command: VerifyCommands,

    /// Working directory (default: current directory)
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
}

/// Nested subcommands for `ralph tools task verify`.
///
/// Each verb mirrors the argument set of the corresponding mutation so the
/// agent can copy its real command, prepend `verify`, and see the exact
/// authorization message it would have received (R7). None of these
/// variants writes to `tasks.jsonl`.
#[derive(Subcommand, Debug)]
pub enum VerifyCommands {
    /// Verify a `task add` would succeed (does not create the task)
    Add(VerifyAddArgs),

    /// Verify a `task ensure` would succeed (does not create the task)
    Ensure(VerifyEnsureArgs),

    /// Verify a `task start` would succeed (does not start the task)
    Start(StartArgs),

    /// Verify a `task close` would succeed (does not close the task)
    Close(CloseArgs),

    /// Verify a `task fail` would succeed (does not fail the task)
    Fail(FailArgs),

    /// Verify a `task reopen` would succeed (does not reopen the task)
    Reopen(ReopenArgs),
}

/// Common output flag for `task verify` subcommands. Mirrors the
/// `--format` on mutation verbs so verify and apply produce the same
/// output shape.
#[derive(Args, Debug)]
pub struct VerifyFormatArgs {
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Argument mirror of `AddArgs` for `task verify add`.
#[derive(Args, Debug)]
pub struct VerifyAddArgs {
    /// Task title (ignored when --skip is set; required otherwise)
    pub title: Option<String>,

    /// Priority (1-5, default 3)
    #[arg(short = 'p', long, default_value = "3")]
    pub priority: u8,

    /// Task description
    #[arg(short = 'd', long)]
    pub description: Option<String>,

    /// Task IDs that must complete first (comma-separated)
    #[arg(long)]
    pub blocked_by: Option<String>,

    #[command(flatten)]
    pub format: VerifyFormatArgs,
}

/// Argument mirror of `EnsureArgs` for `task verify ensure`.
#[derive(Args, Debug)]
pub struct VerifyEnsureArgs {
    /// Task title (ignored when --skip is set; required otherwise)
    pub title: Option<String>,

    /// Stable key used to deduplicate orchestrator-managed tasks.
    /// Mutually exclusive with `--for-fix-unit`.
    #[arg(long, required_unless_present = "for_fix_unit")]
    pub key: Option<String>,

    /// Auto-derive canonical fix-unit key `PLAN:FIX_STEP:SLUG`.
    #[arg(long, value_name = "PLAN:FIX_STEP:SLUG", conflicts_with = "key")]
    pub for_fix_unit: Option<String>,

    /// Priority (1-5, default 3)
    #[arg(short = 'p', long, default_value = "3")]
    pub priority: u8,

    /// Task description
    #[arg(short = 'd', long)]
    pub description: Option<String>,

    /// Task IDs that must complete first (comma-separated)
    #[arg(long)]
    pub blocked_by: Option<String>,

    #[command(flatten)]
    pub format: VerifyFormatArgs,
}

/// Argument mirror of the task_id/task_key/step emit bridge.
///
/// `ralph tools task verify-emit-bridge` checks that three fields an
/// agent is about to put on a `ralph emit` payload are mutually
/// consistent AND consistent with the live task store (R16). This is
/// the OPAC Precheck equivalent of the `ralph-tools-tasks.md` red box
/// rule: never hand-construct a `task_id`; always read it back from
/// the store immediately before emit.
///
/// All three flags are required. The check fails on:
/// - `task_id` not present in the current loop, or status is terminal (Closed/Failed)
/// - `task_key` mismatching the registered key on the task
/// - `step` missing the `:step-<n>:` segment required by
///   `ralph-tools-tasks.md` (red box) or not matching the segment in `task_key`
#[derive(Args, Debug)]
pub struct VerifyEmitBridgeArgs {
    /// Live task_id currently registered with the task store (current loop).
    /// DO NOT hand-construct; read with `ralph tools task list` first.
    #[arg(long)]
    pub task_id: String,

    /// Stable task_key the agent intends to embed on the emit payload.
    #[arg(long)]
    pub task_key: String,

    /// Step number/slug the agent intends to embed (must match the
    /// `:step-<n>:` segment inside `task_key` per
    /// `ralph-tools-tasks.md` red box).
    #[arg(long)]
    pub step: String,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Format enum shared with mutation verbs but exposed for the verify
/// command's exit-code contract: `verify` always exits non-zero on
/// gate failure even when `--format json` is selected so the agent's
/// `$?` check fires deterministically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    Allow,
    Deny { reason: String, hint: String },
}

impl VerifyOutcome {
    /// Machine-grepable prefix agents can match on. Stable across
    /// versions — do not change without updating `ralph-tools-tasks.md`
    /// and any `ralph<agent>.error_msg` assertions in BDD.
    pub const DENY_PREFIX: &'static str = "task_verify denied";

    pub fn allowed_message(verb: &str) -> String {
        format!(
            "task_verify: {verb} would be admitted by the same authorization gates as `ralph tools task {verb}` (no write performed)"
        )
    }

    // 2026-07-16 cleanup U4 (KTD-3): reserved for future preset
    // policy rewrites that surface the verify outcome to the
    // tui.
    #[allow(dead_code)]
    pub fn to_human_string(&self, verb: &str) -> String {
        match self {
            VerifyOutcome::Allow => Self::allowed_message(verb),
            VerifyOutcome::Deny { reason, hint } => {
                format!("{} '{verb}': [{reason}] {hint}", Self::DENY_PREFIX)
            }
        }
    }
}
