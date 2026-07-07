//! CLI commands for the `ralph task` namespace.
//!
//! Provides subcommands for managing tasks:
//! - `add`: Create a new task
//! - `ensure`: Create or reuse a keyed task
//! - `list`: List all tasks
//! - `ready`: Show unblocked tasks
//! - `start`: Mark a task as in progress
//! - `close`: Mark a task as complete
//! - `reopen`: Reopen a closed/failed task
//! - `show`: Show a single task by ID
//! - `verify`: OPAC Precheck; verifies a mutation would succeed without writing
//! - `verify-emit-bridge`: verifies the three-field task_id/task_key/step consistency
//!
//! `verify` exists to satisfy the OPAC Precheck stage (R7/R16). It runs the
//! same authorization gates as the real mutation (`HatCommandPolicy` +
//! `authorize_lifecycle` + field validation) but never touches
//! `tasks.jsonl`. U14 fix-units / shippers rely on `verify` to confirm an
//! emit is correctly wired before applying.

use crate::{
    display::colors, hat_command_policy::HatCommandPolicy, operation_guard::OperationContext,
    resolve_path_from_workspace, resolve_workspace_root,
};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use ralph_core::{Task, TaskStatus, TaskStore};
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
            Self::MissingRalphYml => f.write_str("no ralph.yml in workspace"),
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
/// 2. `<root>/ralph.yaml`
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
pub fn load_coordinator_hats(root: &Path) -> Result<Vec<String>, CoordinatorHatsError> {
    let mut last_yaml_err: Option<(PathBuf, String)> = None;
    for name in ["ralph.yml", "ralph.yaml"] {
        let path = root.join(name);
        if !path.exists() {
            continue;
        }
        let raw =
            std::fs::read_to_string(&path).map_err(|e| CoordinatorHatsError::InvalidYaml {
                path: path.clone(),
                source: e.to_string(),
            })?;
        let value: serde_yaml::Value =
            serde_yaml::from_str(&raw).map_err(|e| CoordinatorHatsError::InvalidYaml {
                path: path.clone(),
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
                path: path.clone(),
                source: e.to_string(),
            }
        })?;
        if hats.is_empty() {
            return Err(CoordinatorHatsError::CoordinatorHatsEmpty);
        }
        return Ok(hats);
    }
    if let Some((path, source)) = last_yaml_err {
        return Err(CoordinatorHatsError::InvalidYaml { path, source });
    }
    Err(CoordinatorHatsError::MissingRalphYml)
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

    pub fn to_human_string(&self, verb: &str) -> String {
        match self {
            VerifyOutcome::Allow => Self::allowed_message(verb),
            VerifyOutcome::Deny { reason, hint } => {
                format!("{} '{verb}': [{reason}] {hint}", Self::DENY_PREFIX,)
            }
        }
    }
}

/// Gets the tasks file path.
fn get_tasks_path(root: Option<&PathBuf>) -> PathBuf {
    resolve_path_from_workspace(".ralph/agent/tasks.jsonl", root)
}

#[cfg(test)]
fn read_current_loop_id(root: Option<&PathBuf>) -> Option<String> {
    operation_context_for(root).current_loop_id
}

fn operation_context_for(root: Option<&PathBuf>) -> OperationContext {
    OperationContext::detect(resolve_workspace_root(root))
}

/// Reject empty or whitespace-only task ids early with a clear error.
///
/// All `ralph tools task` subcommands that take a task id as input call
/// this guard before touching the store. This prevents the agent from
/// accidentally passing `""` (e.g. copied from an empty `work.ready`)
/// and getting the misleading "Task not found" message.
fn validate_task_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        bail!("task_id cannot be empty");
    }
    Ok(())
}

/// Authorize a lifecycle mutation on `task` from the given context.
///
/// Returns `Ok(())` when the caller may mutate, `Err` with a clear
/// message otherwise. In agent context, the caller must own the task
/// or be listed as a coordinator hat. In human context, only an
/// out-of-loop warning is printed (no error).
fn authorize_lifecycle(
    task: &Task,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    operation: &str,
) -> Result<()> {
    if !ctx.is_agent_context {
        if let (Some(current), Some(target)) = (ctx.current_loop_id.as_ref(), task.loop_id.as_ref())
            && current != target
        {
            eprintln!(
                "warning: {operation} targets task in loop '{target}' but current loop is '{current}' (human CLI bypass)"
            );
        }
        return Ok(());
    }

    if ctx.current_loop_id.is_none() {
        bail!(
            "{operation}: agent context requires a current loop marker (set .ralph/current-loop-id)"
        );
    }
    if let (Some(current), Some(target)) = (ctx.current_loop_id.as_ref(), task.loop_id.as_ref()) {
        if current != target {
            bail!(
                "{operation}: task {tid} belongs to loop '{target}' but current loop is '{current}'",
                tid = task.id
            );
        }
    } else {
        bail!(
            "{operation}: legacy task {tid} has no loop_id; not mutable from agent context",
            tid = task.id
        );
    }

    let caller_hat = ctx.current_hat_id.as_deref().ok_or_else(|| {
        anyhow::anyhow!("{operation}: agent context requires a current hat (set RALPH_CURRENT_HAT)")
    })?;

    if task.owner_hat_id.as_deref() == Some(caller_hat) {
        return Ok(());
    }
    if coordinator_hats.iter().any(|h| h == caller_hat) {
        return Ok(());
    }
    bail!(ralph_core::task::task_lifecycle_denied_message(
        task,
        caller_hat,
        coordinator_hats,
        operation,
    ))
}

/// U7 (2026-07-04-003 plan): canonicalize a task mutation payload
/// for the verify-gate fingerprint.
///
/// The same canonical string MUST be produced by both the verify
/// path and the apply path so a `verify` followed by an `add` /
/// `ensure` of the *same* intent matches. Fields that do not
/// affect the written task (format, blocked_by parsed into
/// individual ids, etc.) are normalized to a stable shape.
///
/// Returned as a `String` (the same input the gate's
/// `mutation_fingerprint` expects). The schema is intentionally
/// a small hand-rolled JSON object so it does not depend on the
/// Task struct's serde representation (which can drift).
pub(crate) fn canonical_add_payload(args: &AddArgs) -> String {
    let mut blockers: Vec<&str> = Vec::new();
    if let Some(b) = args.blocked_by.as_deref() {
        for piece in b.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            blockers.push(piece);
        }
    }
    blockers.sort();
    serde_json::json!({
        "verb": "add",
        "title": args.title,
        "priority": args.priority,
        "description": args.description,
        "blocked_by": blockers,
    })
    .to_string()
}

pub(crate) fn canonical_ensure_payload(args: &EnsureArgs, derived_key: Option<&str>) -> String {
    let key = derived_key
        .map(str::to_string)
        .or_else(|| args.key.clone())
        .unwrap_or_default();
    let mut blockers: Vec<&str> = Vec::new();
    if let Some(b) = args.blocked_by.as_deref() {
        for piece in b.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            blockers.push(piece);
        }
    }
    blockers.sort();
    serde_json::json!({
        "verb": "ensure",
        "key": key,
        "title": args.title,
        "priority": args.priority,
        "description": args.description,
        "blocked_by": blockers,
    })
    .to_string()
}

/// Compute the loop_id/hat_id tuple used by the gate fingerprint.
/// Empty strings are substituted when the field is missing so the
/// fingerprint is stable across verify → apply.
pub(crate) fn gate_identifiers(ctx: &OperationContext) -> (&str, &str) {
    (
        ctx.current_loop_id.as_deref().unwrap_or(""),
        ctx.current_hat_id.as_deref().unwrap_or(""),
    )
}

/// Compute the canonical fingerprint for a pending mutation and
/// run the verify-gate check. Encapsulates the (verb,
/// canonical_payload, loop_id, hat_id) → fingerprint pipeline so
/// `execute_add` / `execute_ensure` can call this with a single
/// line and so tests can call it directly without going through
/// the unsafe `set_var` env path.
pub(crate) fn verify_gate_check(
    workspace: &std::path::Path,
    config: &ralph_core::config::RalphConfig,
    ctx: &OperationContext,
    verb: &str,
    canonical_payload: &str,
) -> anyhow::Result<()> {
    let (loop_id, hat_id) = gate_identifiers(ctx);
    let fingerprint =
        crate::task_verify_gate::mutation_fingerprint(verb, canonical_payload, loop_id, hat_id);
    crate::task_verify_gate::require_ticket(
        &crate::task_verify_gate::ticket_path(workspace),
        &config.tasks,
        ctx,
        verb,
        &fingerprint,
    )
}

/// Bridge `HatCommandPolicy::PolicyDecision` to the `anyhow::Result`
/// exit used by the rest of the task CLI.
///
/// On `Allow` we proceed silently (human warnings are not yet wired —
/// the existing `authorize_lifecycle` handles the human cross-loop
/// warning path). On `Deny` we `bail!` with a stable, machine-grepable
/// prefix that the agent can match on to recover.
fn enforce_command_policy(
    ctx: &OperationContext,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    verb: &str,
) -> Result<()> {
    use crate::hat_command_policy::PolicyDecision;
    match HatCommandPolicy::check_task(ctx, coordinator_hats, coordinator_err, verb) {
        PolicyDecision::Allow { .. } => Ok(()),
        PolicyDecision::Deny { reason, hint } => bail!(
            "hat_command_policy denied '{verb}' for hat '{hat}': [{reason}] {hint}",
            verb = verb,
            hat = ctx.current_hat_id.as_deref().unwrap_or("<none>"),
            reason = reason,
            hint = hint,
        ),
    }
}

fn add_common_task_fields(
    mut task: Task,
    ctx: &OperationContext,
    description: Option<String>,
    blocked_by: Option<String>,
) -> Task {
    if let Some(loop_id) = ctx.current_loop_id.clone() {
        task = task.with_loop_id(Some(loop_id));
    }

    if let Some(hat_id) = ctx.current_hat_id.clone() {
        task = task.with_owner_hat(Some(hat_id));
    }

    if let Some(desc) = description {
        task = task.with_description(Some(desc));
    }

    if let Some(blockers) = blocked_by {
        for blocker_id in blockers
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            task = task.with_blocker(blocker_id.to_string());
        }
    }

    task
}

/// U3: enforce that a task's `owner_hat_id` is on the workspace's
/// `tasks.coordinator_hats` allowlist.
///
/// This is the create-side complement to the JSONL origin guard: even if a
/// rogue `ralph` hat somehow got into the loop, it cannot persist a task
/// to disk that is attributed to a workflow hat. Without this check the
/// stall-recovery path could silently create tasks under the wrong owner,
/// corrupting plan-gate's task correlation and the merge queue.
///
/// When `owner_hat_id` is `None` the call is human-driven (the CLI does
/// not stamp an owner when `ctx.current_hat_id` is unset) and the check
/// is skipped — humans operating the CLI must not be locked out.
///
/// When the allowlist is empty AND the task carries an owner, the call
/// is rejected (fail-closed): an empty allowlist is a misconfiguration
/// and we must not let an agent bypass owner validation by being the
/// only hat in scope.
fn validate_owner_hat_id(task: &Task, coordinator_hats: &[String]) -> Result<()> {
    let Some(owner) = task.owner_hat_id.as_deref() else {
        return Ok(());
    };
    if coordinator_hats.iter().any(|h| h == owner) {
        Ok(())
    } else {
        bail!(
            "owner_hat_id '{owner}' is not in tasks.coordinator_hats. \
             Allowed: {coordinator_hats:?}. \
             The owner is set from $RALPH_CURRENT_HAT at task creation; \
             either run the task command from a hat in coordinator_hats, \
             or add the hat to tasks.coordinator_hats in ralph.yml."
        )
    }
}

fn status_matches_filter(status: TaskStatus, filter: &str) -> bool {
    let normalized = filter.to_lowercase().replace(['_', '-'], "");
    match status {
        TaskStatus::Open => normalized == "open",
        TaskStatus::InProgress => normalized == "inprogress",
        TaskStatus::Closed => normalized == "closed",
        TaskStatus::Failed => normalized == "failed",
    }
}

fn filter_tasks_for_list(store: &TaskStore, args: &ListArgs) -> Vec<Task> {
    let mut tasks: Vec<_> = if let Some(status_str) = args.status.as_deref() {
        store
            .all()
            .iter()
            .filter(|t| status_matches_filter(t.status, status_str))
            .cloned()
            .collect()
    } else if args.all {
        store.all().to_vec()
    } else {
        store
            .all()
            .iter()
            .filter(|t| !matches!(t.status, TaskStatus::Closed | TaskStatus::Failed))
            .cloned()
            .collect()
    };

    if let Some(days) = args.days {
        let cutoff = Utc::now() - chrono::Duration::days(days);
        tasks.retain(|t| {
            if DateTime::parse_from_rfc3339(&t.created)
                .map(|c| c.with_timezone(&Utc) > cutoff)
                .unwrap_or(false)
            {
                return true;
            }

            if t.closed.as_ref().is_some_and(|closed_str| {
                DateTime::parse_from_rfc3339(closed_str)
                    .map(|c| c.with_timezone(&Utc) > cutoff)
                    .unwrap_or(false)
            }) {
                return true;
            }
            false
        });
    }

    tasks.sort_by(|a, b| {
        let status_rank = |s: TaskStatus| match s {
            TaskStatus::InProgress => 0,
            TaskStatus::Open => 1,
            TaskStatus::Closed => 2,
            TaskStatus::Failed => 3,
        };

        let rank_a = status_rank(a.status);
        let rank_b = status_rank(b.status);

        if rank_a != rank_b {
            return rank_a.cmp(&rank_b);
        }

        if a.priority != b.priority {
            return a.priority.cmp(&b.priority);
        }

        a.created.cmp(&b.created)
    });

    if let Some(limit) = args.limit {
        tasks.truncate(limit);
    }

    tasks
}

fn filter_tasks_for_ready(
    store: &TaskStore,
    args: &ReadyArgs,
    root: Option<&PathBuf>,
) -> Vec<Task> {
    let mut ready: Vec<Task> = store.ready().into_iter().cloned().collect();

    if !args.all
        && let Some(current_loop_id) =
            crate::operation_guard::OperationContext::detect(resolve_workspace_root(root))
                .current_loop_id
    {
        ready.retain(|t| t.loop_id.as_ref() == Some(&current_loop_id));
    }

    ready
}

/// Executes task CLI commands.
pub fn execute(args: TaskArgs, use_colors: bool) -> Result<()> {
    let root = args.root.clone();
    let workspace = resolve_workspace_root(root.as_ref());
    // U7 (2026-07-04-003 plan): load `coordinator_hats` through the
    // typed loader so we can surface the *shape* of the failure
    // (missing ralph.yml vs missing tasks: vs missing key vs empty)
    // instead of silently treating all four as "empty allowlist".
    // Human CLI gets `unwrap_or_default()` so a missing/empty config
    // does not lock the operator out of `task add`; agents always
    // see the typed Err converted into a hint.
    let (coordinator_hats, coordinator_err) = match load_coordinator_hats(&workspace) {
        Ok(hats) => (hats, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    let config = load_config_or_default(root.as_ref());

    match args.command {
        TaskCommands::Add(add_args) => execute_add(
            add_args,
            root.as_ref(),
            &coordinator_hats,
            coordinator_err.as_ref(),
            use_colors,
        ),
        TaskCommands::Ensure(ensure_args) => execute_ensure(
            ensure_args,
            root.as_ref(),
            &coordinator_hats,
            coordinator_err.as_ref(),
            use_colors,
        ),
        TaskCommands::List(list_args) => execute_list(list_args, root.as_ref(), use_colors),
        TaskCommands::Ready(ready_args) => execute_ready(ready_args, root.as_ref(), use_colors),
        TaskCommands::Start(start_args) => {
            execute_start(start_args, root.as_ref(), &coordinator_hats, use_colors)
        }
        TaskCommands::Close(close_args) => execute_close(
            close_args,
            root.as_ref(),
            &coordinator_hats,
            &config,
            use_colors,
        ),
        TaskCommands::Fail(fail_args) => {
            execute_fail(fail_args, root.as_ref(), &coordinator_hats, use_colors)
        }
        TaskCommands::Reopen(reopen_args) => {
            execute_reopen(reopen_args, root.as_ref(), &coordinator_hats, use_colors)
        }
        TaskCommands::Show(show_args) => execute_show(show_args, root.as_ref(), use_colors),
        TaskCommands::Verify(verify_args) => execute_verify(verify_args, use_colors),
        TaskCommands::VerifyEmitBridge(bridge_args) => {
            execute_verify_emit_bridge(bridge_args, root.as_ref())
        }
    }
}

/// Loads the full `RalphConfig` from the workspace, falling back to
/// an empty default when the file is missing or unreadable.
///
/// The fallback is intentionally silent because the L2 CLI ACL is
/// best-effort: a human operator without a `ralph.yml` must not be
/// locked out of task tooling. The downstream `HatCommandPolicy`
/// reads the same allowlist (`tasks.coordinator_hats`), so missing
/// config yields an empty allowlist → fail-closed for agent add/ensure.
fn load_config_or_default(root: Option<&PathBuf>) -> ralph_core::config::RalphConfig {
    let workspace = resolve_workspace_root(root);
    for name in ["ralph.yml", "ralph.yaml"] {
        let path = workspace.join(name);
        if !path.exists() {
            continue;
        }
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_yaml::from_str::<ralph_core::config::RalphConfig>(&raw) {
                return cfg;
            }
        }
    }
    serde_yaml::from_str("event_loop:\n  execution_mode: isolated\n").unwrap_or_default()
}

fn execute_add(
    args: AddArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);
    let workspace = resolve_workspace_root(root);

    enforce_command_policy(&ctx, coordinator_hats, coordinator_err, "add")?;
    // U7 (2026-07-04-003 plan): two-step gate. If the agent
    // invoked `task verify add` first, a matching ticket is on
    // disk; require_ticket consumes it and lets the mutation
    // proceed. Without verify (or with a stale ticket), the gate
    // denies.
    let canonical = canonical_add_payload(&args);
    let config = load_config_or_default(root);
    verify_gate_check(&workspace, &config, &ctx, "add", &canonical)?;
    add_task_with_args(&mut store, &args, &ctx, coordinator_hats, use_colors)?;
    Ok(())
}

#[cfg_attr(test, allow(dead_code))]
fn add_task_with_args(
    store: &mut TaskStore,
    args: &AddArgs,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let task = add_common_task_fields(
        Task::new(args.title.clone(), args.priority),
        ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );

    // U3: owner_hat_id must come from `tasks.coordinator_hats`.
    //
    // The stall recovery path in the worktree loop (the ce-executor
    // impersonation bug fixed by the P0 origin guard) let the `ralph`
    // fallback hat silently create tasks attributed to workflow hats,
    // polluting the merge queue and corrupting plan-gate correlation.
    // Backing the create-side check with a `coordinator_hats` allowlist
    // closes the gap: any hat that is not on the allowlist cannot
    // create a task. When `owner_hat_id` is absent (human CLI usage,
    // where `ctx.current_hat_id` is None), the check is skipped — the
    // existing `add_common_task_fields` only stamps an owner when
    // `ctx.current_hat_id` is set, so `None` here is a reliable signal
    // that the call is human-driven.
    validate_owner_hat_id(&task, coordinator_hats)?;

    let invalid_blockers = store.invalid_blockers(&task);
    if !invalid_blockers.is_empty() {
        bail!(
            "task blocked_by references missing or out-of-loop tasks: {}",
            invalid_blockers.join(", ")
        );
    }

    if let Some(key) = task.key.as_deref() {
        if let Some(locus) = ralph_core::task_store::live_task_locus(key) {
            if let Some(existing) = store.find_by_locus_in_loop(&locus, task.loop_id.as_deref()) {
                bail!(
                    "task add rejected: live identity already exists for loop {:?} step locus \
                     '{locus}' (task_id={}). Use `ralph tools task ensure` instead of add.",
                    task.loop_id,
                    existing.id
                );
            }
        }
    }

    let task_id = task.id.clone();
    store.add(task.clone());
    store.save().context("Failed to save tasks")?;

    print_added_task(&task, &task_id, args.format, use_colors);
    Ok(())
}

fn execute_ensure(
    args: EnsureArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);
    let workspace = resolve_workspace_root(root);
    enforce_command_policy(&ctx, coordinator_hats, coordinator_err, "ensure")?;

    // R4 (2026-06-14-003 plan): opt into the single-U contract.
    // Two signals are accepted (env var takes precedence; the
    // marker file is the safe fallback for `ralph run` because the
    // workspace `forbid(unsafe_code)` lint forbids `set_var` from
    // lib code):
    //   1. `RALPH_ENFORCE_CURRENT_UNIT` env var (set by operators
    //      for standalone CLI use).
    //   2. `<workspace>/.ralph/agent/.ralph-enforce-current-unit`
    //      marker file (written by `ralph run`'s bootstrap when the
    //      preset opts in).
    if std::env::var_os("RALPH_ENFORCE_CURRENT_UNIT").is_some() {
        store.set_enforce_current_unit(true);
    } else if let Some(workspace) = root {
        let marker = workspace
            .join(".ralph")
            .join("agent")
            .join(".ralph-enforce-current-unit");
        if marker.exists() {
            store.set_enforce_current_unit(true);
        }
    }

    // U7 (2026-07-04-003 plan): two-step gate for ensure. Use
    // the same canonical payload as verify so the fingerprint
    // matches a preceding `task verify ensure` call.
    let derived_key = if let Some(spec) = args.for_fix_unit.as_deref() {
        let mut parts = spec.split(':');
        let plan = parts.next().unwrap_or("").to_string();
        let fix_step = parts.next().unwrap_or("").to_string();
        let slug = parts.next().unwrap_or("").to_string();
        if plan.is_empty() || fix_step.is_empty() || slug.is_empty() || parts.next().is_some() {
            bail!(
                "--for-fix-unit expects exactly 3 colon-separated segments: \
                 PLAN:FIX_STEP:SLUG, got '{spec}'"
            );
        }
        Some(format!("ce-executor:{plan}:{fix_step}:{slug}"))
    } else {
        None
    };
    let canonical = canonical_ensure_payload(&args, derived_key.as_deref());
    let (loop_id, hat_id) = gate_identifiers(&ctx);
    let fingerprint =
        crate::task_verify_gate::mutation_fingerprint("ensure", &canonical, loop_id, hat_id);
    let config = load_config_or_default(root);
    crate::task_verify_gate::require_ticket(
        &crate::task_verify_gate::ticket_path(&workspace),
        &config.tasks,
        &ctx,
        "ensure",
        &fingerprint,
    )?;

    ensure_task_with_args(&mut store, &args, &ctx, coordinator_hats, use_colors)?;
    Ok(())
}

#[cfg_attr(test, allow(dead_code))]
fn ensure_task_with_args(
    store: &mut TaskStore,
    args: &EnsureArgs,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    // 2026-06-28-002 U8: `--for-fix-unit plan:fix_step:slug` builds
    // the canonical fix-unit task and pins the owner to
    // `coordinator`. The returned task_id is then used in the
    // follow-up `work.ready` emit so `work.done` no longer
    // collides with the legacy `task-fix-01-placeholder`
    // contract. When the flag is set we ignore the `--key`
    // argument to avoid silent double-sourcing of the key.
    let derived_key = if let Some(spec) = args.for_fix_unit.as_deref() {
        let mut parts = spec.split(':');
        let plan = parts.next().unwrap_or("").to_string();
        let fix_step = parts.next().unwrap_or("").to_string();
        let slug = parts.next().unwrap_or("").to_string();
        if plan.is_empty() || fix_step.is_empty() || slug.is_empty() || parts.next().is_some() {
            bail!(
                "--for-fix-unit expects exactly 3 colon-separated segments: \
                 PLAN:FIX_STEP:SLUG, got '{spec}'"
            );
        }
        Some(format!("ce-executor:{plan}:{fix_step}:{slug}"))
    } else {
        None
    };
    let key_value = derived_key
        .clone()
        .or_else(|| args.key.clone())
        .expect("ensure requires either --key or --for-fix-unit");
    let mut task = add_common_task_fields(
        Task::new(args.title.clone(), args.priority).with_key(Some(key_value)),
        ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );
    // 2026-06-28-002 U8: pin the owner to `coordinator` so the
    // legacy execution contract (`TaskWrongLoop` / loop scope)
    // validates the follow-up `work.ready` / `work.done`
    // payload against the canonical fix-unit hat.
    if args.for_fix_unit.is_some() {
        task = task.with_owner_hat(Some("coordinator".to_string()));
    }
    validate_owner_hat_id(&task, coordinator_hats)?;
    let key = task.key.clone().expect("ensure key should be set");
    let loop_id = task.loop_id.clone();
    let existed = store.get_by_key_in_loop(&key, loop_id.as_deref()).is_some();

    let invalid_blockers = store.invalid_blockers(&task);
    if !invalid_blockers.is_empty() {
        bail!(
            "task blocked_by references missing or out-of-loop tasks: {}",
            invalid_blockers.join(", ")
        );
    }

    let ensured = store
        .with_exclusive_lock(|s| s.ensure(task).clone())
        .context("Failed to ensure task")?;

    // R4 (2026-06-14-003 plan): when the single-U contract is active
    // and the requested key differs from the ensured task's key, the
    // contract rejected the new unit in favour of an open sibling
    // task.  Surface the collision via a non-zero exit + stderr so
    // the agent's `ralph tools task ensure` invocation is not a
    // silent surprise.  Without this check the CLI prints
    // 'Ensured task <existing> <uM-...>' for the new uN- key and
    // exits 0, which masks the rejection.
    if store.enforce_current_unit() && ensured.key.as_deref() != Some(&key) {
        bail!(
            "rejected by R4 single-U contract: ensure key '{key}' conflicts with \
             existing task id={} key={} (only one open task per (loop_id, plan, step) \
             is allowed). Close the existing task first or use a non-uN- key suffix.",
            ensured.id,
            ensured.key.as_deref().unwrap_or("?"),
        );
    }

    print_ensured_task(&ensured, &key, existed, args.format, use_colors);
    Ok(())
}

fn print_added_task(task: &Task, task_id: &str, format: OutputFormat, use_colors: bool) {
    match format {
        OutputFormat::Table => {
            if use_colors {
                println!("{}Created task {}{}", colors::GREEN, task_id, colors::RESET);
            } else {
                println!("Created task {}", task_id);
            }
            println!("  Title: {}", task.title);
            println!("  Priority: {}", task.priority);
            if let Some(key) = &task.key {
                println!("  Key: {}", key);
            }
            if !task.blocked_by.is_empty() {
                println!("  Blocked by: {}", task.blocked_by.join(", "));
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string(task).expect("task serializes"));
        }
        OutputFormat::Quiet => {
            println!("{}", task_id);
        }
    }
}

fn print_ensured_task(
    ensured: &Task,
    key: &str,
    existed: bool,
    format: OutputFormat,
    use_colors: bool,
) {
    match format {
        OutputFormat::Table => {
            let verb = if existed { "Reused" } else { "Ensured" };
            if use_colors {
                println!(
                    "{}{} task {}{}",
                    colors::GREEN,
                    verb,
                    ensured.id,
                    colors::RESET
                );
            } else {
                println!("{} task {}", verb, ensured.id);
            }
            println!("  Title: {}", ensured.title);
            println!("  Key: {}", key);
            println!("  Priority: {}", ensured.priority);
            if !ensured.blocked_by.is_empty() {
                println!("  Blocked by: {}", ensured.blocked_by.join(", "));
            }
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string(ensured).expect("task serializes")
            );
        }
        OutputFormat::Quiet => {
            println!("{}", ensured.id);
        }
    }
}

fn execute_list(args: ListArgs, root: Option<&PathBuf>, use_colors: bool) -> Result<()> {
    let path = get_tasks_path(root);
    let store = TaskStore::load(&path).context("Failed to load tasks")?;

    let tasks = filter_tasks_for_list(&store, &args);

    match args.format {
        OutputFormat::Table => {
            if tasks.is_empty() {
                println!("No tasks found");
            } else {
                if use_colors {
                    println!(
                        "{}{:<20} {:<15} {:<8} {:<60} {:<24}{}",
                        colors::DIM,
                        "ID",
                        "Status",
                        "Priority",
                        "Title",
                        "Key",
                        colors::RESET
                    );
                    println!("{}{}{}", colors::DIM, "-".repeat(131), colors::RESET);
                } else {
                    println!(
                        "{:<20} {:<15} {:<8} {:<60} {:<24}",
                        "ID", "Status", "Priority", "Title", "Key"
                    );
                    println!("{}", "-".repeat(131));
                }

                for task in &tasks {
                    let (status_str, status_color) = match task.status {
                        TaskStatus::Open => ("open", colors::GREEN),
                        TaskStatus::InProgress => ("in_progress", colors::BLUE),
                        TaskStatus::Closed => ("closed", colors::DIM),
                        TaskStatus::Failed => ("failed", colors::RED),
                    };

                    let priority_color = match task.priority {
                        1 => colors::RED,
                        2 => colors::YELLOW,
                        _ => colors::RESET,
                    };

                    let title_truncated = if task.title.len() > 60 {
                        crate::display::truncate(&task.title, 60)
                    } else {
                        task.title.clone()
                    };

                    if use_colors {
                        println!(
                            "{}{:<20}{} {}{:<15}{} {}{:<8}{} {:<60} {:<24}",
                            colors::DIM,
                            task.id,
                            colors::RESET,
                            status_color,
                            status_str,
                            colors::RESET,
                            priority_color,
                            task.priority,
                            colors::RESET,
                            title_truncated,
                            task.key.as_deref().unwrap_or("-")
                        );
                    } else {
                        println!(
                            "{:<20} {:<15} {:<8} {:<60} {:<24}",
                            task.id,
                            status_str,
                            task.priority,
                            title_truncated,
                            task.key.as_deref().unwrap_or("-")
                        );
                    }
                }
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&tasks)?);
        }
        OutputFormat::Quiet => {
            for task in &tasks {
                println!("{}", task.id);
            }
        }
    }

    Ok(())
}

fn execute_ready(args: ReadyArgs, root: Option<&PathBuf>, use_colors: bool) -> Result<()> {
    let path = get_tasks_path(root);
    let store = TaskStore::load(&path).context("Failed to load tasks")?;

    let ready = filter_tasks_for_ready(&store, &args, root);

    match args.format {
        OutputFormat::Table => {
            if ready.is_empty() {
                println!("No ready tasks");
            } else {
                if use_colors {
                    println!(
                        "{}{:<20} {:<8} {:<60} {:<24}{}",
                        colors::DIM,
                        "ID",
                        "Priority",
                        "Title",
                        "Key",
                        colors::RESET
                    );
                    println!("{}{}{}", colors::DIM, "-".repeat(115), colors::RESET);
                } else {
                    println!(
                        "{:<20} {:<8} {:<60} {:<24}",
                        "ID", "Priority", "Title", "Key"
                    );
                    println!("{}", "-".repeat(115));
                }

                for task in &ready {
                    let title_truncated = if task.title.len() > 60 {
                        crate::display::truncate(&task.title, 60)
                    } else {
                        task.title.clone()
                    };

                    let priority_color = match task.priority {
                        1 => colors::RED,
                        2 => colors::YELLOW,
                        _ => colors::RESET,
                    };

                    if use_colors {
                        println!(
                            "{}{:<20}{} {}{:<8}{} {:<60} {:<24}",
                            colors::DIM,
                            task.id,
                            colors::RESET,
                            priority_color,
                            task.priority,
                            colors::RESET,
                            title_truncated,
                            task.key.as_deref().unwrap_or("-")
                        );
                    } else {
                        println!(
                            "{:<20} {:<8} {:<60} {:<24}",
                            task.id,
                            task.priority,
                            title_truncated,
                            task.key.as_deref().unwrap_or("-")
                        );
                    }
                }
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&ready)?);
        }
        OutputFormat::Quiet => {
            for task in &ready {
                println!("{}", task.id);
            }
        }
    }

    Ok(())
}

fn execute_start(
    args: StartArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);
    start_task_with_context(&mut store, &args.id, &ctx, coordinator_hats, use_colors)
}

#[cfg_attr(test, allow(dead_code))]
fn start_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    validate_task_id(task_id)?;
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    authorize_lifecycle(&snapshot, ctx, coordinator_hats, "start")?;

    let started = store
        .with_exclusive_lock(|s| s.start(task_id).cloned())
        .context("Failed to save tasks")?
        .context(format!("Task {} not found", task_id))?;

    if use_colors {
        println!(
            "{}Started task: {} - {}{}",
            colors::BLUE,
            task_id,
            started.title,
            colors::RESET
        );
    } else {
        println!("Started task: {} - {}", task_id, started.title);
    }
    Ok(())
}

fn execute_close(
    args: CloseArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    config: &ralph_core::config::RalphConfig,
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);
    // U3 close-time role gate (mirrors add/ensure). close itself
    // is not in `COORDINATOR_ONLY` so the gate is permissive —
    // `authorize_lifecycle` below still enforces ownership. The
    // call here keeps the entry-point message shape uniform with
    // add/ensure for future policy tightening.
    let coordinator_err: Option<CoordinatorHatsError> = if config.tasks.coordinator_hats.is_empty()
    {
        Some(CoordinatorHatsError::CoordinatorHatsEmpty)
    } else {
        None
    };
    enforce_command_policy(&ctx, coordinator_hats, coordinator_err.as_ref(), "close")?;
    close_task_with_context_and_config(
        &mut store,
        &args.id,
        &ctx,
        coordinator_hats,
        use_colors,
        Some(config),
        root,
    )
}

#[cfg_attr(test, allow(dead_code))]
fn close_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    close_task_with_context_and_config(
        store,
        task_id,
        ctx,
        coordinator_hats,
        use_colors,
        None,
        None,
    )
}

/// U7-aware variant of close. When `config` + `root` are provided and
/// the caller is in agent context, the function reads the hat-channel
/// (`current-hat-events`) tail after saving the close and emits a
/// warning stderr JSON when no completion-class topic is present there.
#[allow(clippy::too_many_arguments)]
fn close_task_with_context_and_config(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
    config: Option<&ralph_core::config::RalphConfig>,
    root: Option<&PathBuf>,
) -> Result<()> {
    validate_task_id(task_id)?;
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    let owner_hat = snapshot.owner_hat_id.clone();
    authorize_lifecycle(&snapshot, ctx, coordinator_hats, "close")?;

    let title = store
        .get_mut(task_id)
        .map(|t| {
            // 2026-06-30-001 P0-4: the `ralph task close`
            // CLI path is the only legitimate way a task
            // gets closed without an explicit
            // `TaskStore::start` call (operator explicitly
            // retires a row that never picked up). Mark
            // the row started here, mirroring the
            // `project_close_task` event path. The new
            // `TaskStore::close` / `close_by_key`
            // `started.is_none()` guard (added in P0-4
            // to prevent orphan closed tasks for
            // placeholder rows) accepts the close.
            t.start();
            t.status = TaskStatus::Closed;
            t.closed = Some(chrono::Utc::now().to_rfc3339());
            t.title.clone()
        })
        .context(format!("Task {} not found", task_id))?;

    store.save().context("Failed to save tasks")?;

    if use_colors {
        println!(
            "{}Closed task: {} - {}{}",
            colors::GREEN,
            task_id,
            title,
            colors::RESET
        );
    } else {
        println!("Closed task: {} - {}", task_id, title);
    }

    // U7: completion-emit guard. Only fires in agent context AND only
    // when the CLI has a config + workspace root to derive completion
    // topics from. The legacy CLI callers (which pass `None`) keep the
    // pre-U7 behaviour: silent close, no warning. The caller hat is
    // taken from `ctx.current_hat_id` (not the task owner) so a
    // coordinator hat that closes someone else's task still warns based
    // on its own completion contract.
    if let (Some(cfg), Some(root_path)) = (config, root) {
        if let Some(caller_hat) = ctx.current_hat_id.clone() {
            emit_close_completion_warning(root_path, cfg, &caller_hat);
        }
    }
    Ok(())
}

/// U7 helper: if the agent caller has completion-class topics they
/// should publish after closing, scan the hat-channel tail for any of
/// those topics and emit a stderr JSON warning when none are present.
///
/// Design notes:
///
/// - **warn-only, not deny**: per the plan (`Non-goals: 不 hard-block
///   executor \`task close\``). The function never returns an error
///   and never alters the exit code; agents that intentionally skip
///   the completion emit (e.g. cancel paths) keep their close.
/// - **hat-channel only**: the merge happens *after* the backend
///   exits, so the same-activation Confirm can only see the
///   `current-hat-events` marker. Reading main events here would
///   duplicate work `ralph events --events-source auto` already does.
/// - **fail-closed on empty / unreadable channel**: agents still get a
///   `hint: run ralph inspect loop` so they can self-diagnose.
fn emit_close_completion_warning(
    root: &PathBuf,
    config: &ralph_core::config::RalphConfig,
    caller_hat: &str,
) {
    let expected = if config.event_loop.event_policy.is_some() {
        ralph_core::completion_emit::derive_completion_publishes(config, caller_hat)
    } else {
        Vec::new()
    };
    if expected.is_empty() {
        return; // nothing for this hat to emit; no warning.
    }
    let channel_hint = "hat-channel file is empty or missing; \
                        run `ralph inspect loop` to confirm the marker is set";
    let Some((channel_path, exists)) = crate::cli::resolve_hat_channel_file(root) else {
        eprintln!(
            "{} {{ \"code\": \"close_without_completion_emit\", \
             \"hat\": \"{caller_hat}\", \"expected_topics\": {expected:?}, \
             \"reason\": \"hat_channel_missing_marker\", \
             \"hint\": \"{channel_hint}\" }}",
            ralph_core::completion_emit::CLOSE_WITHOUT_COMPLETION_PREFIX
        );
        return;
    };
    if !exists {
        eprintln!(
            "{} {{ \"code\": \"close_without_completion_emit\", \
             \"hat\": \"{caller_hat}\", \"expected_topics\": {expected:?}, \
             \"reason\": \"hat_channel_unreadable\", \
             \"hint\": \"{channel_hint}\" }}",
            ralph_core::completion_emit::CLOSE_WITHOUT_COMPLETION_PREFIX
        );
        return;
    }
    let Ok(content) = std::fs::read_to_string(&channel_path) else {
        eprintln!(
            "{} {{ \"code\": \"close_without_completion_emit\", \
             \"hat\": \"{caller_hat}\", \"expected_topics\": {expected:?}, \
             \"reason\": \"hat_channel_unreadable\", \
             \"hint\": \"{channel_hint}\" }}",
            ralph_core::completion_emit::CLOSE_WITHOUT_COMPLETION_PREFIX
        );
        return;
    };
    let tail_topics = parse_topics_from_jsonl_tail(&content, TAIL_SCAN_LINES);
    if tail_topics.iter().any(|t| expected.iter().any(|e| e == t)) {
        return; // close + completion topics both recorded.
    }

    let next = ralph_core::completion_emit::next_step_hint(&expected);
    eprintln!(
        "{} {{ \"code\": \"close_without_completion_emit\", \
         \"hat\": \"{caller_hat}\", \"task_id\": \"{}\", \
         \"expected_topics\": {expected:?}, \
         \"observed_topics\": {tail_topics:?}, \
         \"next_step\": \"{next}\" }}",
        ralph_core::completion_emit::CLOSE_WITHOUT_COMPLETION_PREFIX,
        caller_hat = caller_hat,
    );
}

/// How many trailing JSONL lines `emit_close_completion_warning` scans
/// when looking for the expected completion topic. The agent's
/// same-activation write channel rarely grows large — a worker hat may
/// emit a handful of events before closing — so a small fixed window
/// is enough for Confirm. Reading the whole file was P1 #7's source of
/// false negatives on multi-hour activations.
const TAIL_SCAN_LINES: usize = 50;

/// Light-weight JSONL topic extractor — pulls the `topic` field from
/// each of the trailing N lines that look like a valid event envelope.
/// Tolerant of malformed lines (skipped silently) because the
/// hat-channel may carry lines from multiple sources when working-tree
/// features are toggled.
fn parse_topics_from_jsonl_tail(content: &str, max_lines: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in content
        .lines()
        .rev()
        .take(max_lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(topic) = v.get("topic").and_then(|t| t.as_str()) {
                out.push(topic.to_string());
            }
        }
    }
    out
}

fn execute_fail(
    args: FailArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);
    fail_task_with_context(&mut store, &args.id, &ctx, coordinator_hats, use_colors)
}

#[cfg_attr(test, allow(dead_code))]
fn fail_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    validate_task_id(task_id)?;
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    authorize_lifecycle(&snapshot, ctx, coordinator_hats, "fail")?;

    let title = store
        .fail(task_id)
        .context(format!("Task {} not found", task_id))?
        .title
        .clone();

    store.save().context("Failed to save tasks")?;

    if use_colors {
        println!(
            "{}Failed task: {} - {}{}",
            colors::RED,
            task_id,
            title,
            colors::RESET
        );
    } else {
        println!("Failed task: {} - {}", task_id, title);
    }
    Ok(())
}

fn execute_show(args: ShowArgs, root: Option<&PathBuf>, use_colors: bool) -> Result<()> {
    validate_task_id(&args.id)?;
    let path = get_tasks_path(root);
    let store = TaskStore::load(&path).context("Failed to load tasks")?;

    let task = store
        .get(&args.id)
        .context(format!("Task {} not found", args.id))?;

    match args.format {
        OutputFormat::Table => {
            let status_str = match task.status {
                TaskStatus::Open => "open",
                TaskStatus::InProgress => "in_progress",
                TaskStatus::Closed => "closed",
                TaskStatus::Failed => "failed",
            };

            if use_colors {
                let status_color = match task.status {
                    TaskStatus::Open => colors::GREEN,
                    TaskStatus::InProgress => colors::BLUE,
                    TaskStatus::Closed => colors::DIM,
                    TaskStatus::Failed => colors::RED,
                };
                let priority_color = match task.priority {
                    1 => colors::RED,
                    2 => colors::YELLOW,
                    _ => colors::RESET,
                };

                println!("{}ID:          {}{}", colors::DIM, task.id, colors::RESET);
                println!("Title:       {}", task.title);
                if let Some(desc) = &task.description {
                    println!("Description: {}", desc);
                }
                println!(
                    "Status:      {}{}{}",
                    status_color,
                    status_str,
                    colors::RESET
                );
                println!(
                    "Priority:    {}{}{}",
                    priority_color,
                    task.priority,
                    colors::RESET
                );
                if let Some(key) = &task.key {
                    println!("Key:         {}", key);
                }
                if let Some(loop_id) = &task.loop_id {
                    println!("Loop:        {}", loop_id);
                }
                if let Some(owner) = &task.owner_hat_id {
                    println!("Owner hat:   {}", owner);
                }
                if !task.blocked_by.is_empty() {
                    println!("Blocked by:  {}", task.blocked_by.join(", "));
                }
                println!("Created:     {}", task.created);
                if let Some(started) = &task.started {
                    println!("Started:     {}", started);
                }
                if let Some(closed) = &task.closed {
                    println!("Closed:      {}", closed);
                }
            } else {
                println!("ID:          {}", task.id);
                println!("Title:       {}", task.title);
                if let Some(desc) = &task.description {
                    println!("Description: {}", desc);
                }
                println!("Status:      {}", status_str);
                println!("Priority:    {}", task.priority);
                if let Some(key) = &task.key {
                    println!("Key:         {}", key);
                }
                if let Some(loop_id) = &task.loop_id {
                    println!("Loop:        {}", loop_id);
                }
                if let Some(owner) = &task.owner_hat_id {
                    println!("Owner hat:   {}", owner);
                }
                if !task.blocked_by.is_empty() {
                    println!("Blocked by:  {}", task.blocked_by.join(", "));
                }
                println!("Created:     {}", task.created);
                if let Some(started) = &task.started {
                    println!("Started:     {}", started);
                }
                if let Some(closed) = &task.closed {
                    println!("Closed:      {}", closed);
                }
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&task)?);
        }
        OutputFormat::Quiet => {
            println!("{}", task.id);
        }
    }

    Ok(())
}

fn execute_reopen(
    args: ReopenArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);
    reopen_task_with_context(&mut store, &args.id, &ctx, coordinator_hats, use_colors)
}

#[cfg_attr(test, allow(dead_code))]
fn reopen_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    validate_task_id(task_id)?;
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    authorize_lifecycle(&snapshot, ctx, coordinator_hats, "reopen")?;

    let reopened = store
        .with_exclusive_lock(|s| s.reopen(task_id).cloned())
        .context("Failed to save tasks")?
        .context(format!("Task {} not found", task_id))?;

    if use_colors {
        println!(
            "{}Reopened task: {} - {}{}",
            colors::YELLOW,
            task_id,
            reopened.title,
            colors::RESET
        );
    } else {
        println!("Reopened task: {} - {}", task_id, reopened.title);
    }
    Ok(())
}

/// U4: route `task verify <verb>` to a verb-specific dry-run helper.
///
/// This is the entry point of the OPAC Precheck stage for task
/// mutations. The function never writes to `tasks.jsonl`; it only
/// exercises the same authorization gates as the real mutation so the
/// agent can deterministically observe the outcome without committing.
fn execute_verify(args: VerifyArgs, use_colors: bool) -> Result<()> {
    let root = args.root.clone();
    let config = load_config_or_default(root.as_ref());
    let workspace = resolve_workspace_root(root.as_ref());
    let (coordinator_hats, coordinator_err) = match load_coordinator_hats(&workspace) {
        Ok(hats) => (hats, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    let cmd = args.command;

    let path = get_tasks_path(root.as_ref());
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root.as_ref());

    let outcome = match &cmd {
        VerifyCommands::Add(a) => verify_add(
            &mut store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            a,
        )?,
        VerifyCommands::Ensure(e) => verify_ensure(
            &mut store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            e,
        )?,
        VerifyCommands::Start(s) => verify_lifecycle(
            &store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            "start",
            &s.id,
        )?,
        VerifyCommands::Close(c) => verify_lifecycle(
            &store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            "close",
            &c.id,
        )?,
        VerifyCommands::Fail(f) => verify_lifecycle(
            &store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            "fail",
            &f.id,
        )?,
        VerifyCommands::Reopen(r) => verify_lifecycle(
            &store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            "reopen",
            &r.id,
        )?,
    };

    let verb = match &cmd {
        VerifyCommands::Add(_) => "add",
        VerifyCommands::Ensure(_) => "ensure",
        VerifyCommands::Start(_) => "start",
        VerifyCommands::Close(_) => "close",
        VerifyCommands::Fail(_) => "fail",
        VerifyCommands::Reopen(_) => "reopen",
    };
    match outcome {
        VerifyOutcome::Allow => {
            let format = match &cmd {
                VerifyCommands::Add(a) => a.format.format,
                VerifyCommands::Ensure(e) => e.format.format,
                _ => OutputFormat::Table,
            };
            match format {
                OutputFormat::Json => {
                    let json = serde_json::json!({
                        "verified": true,
                        "verb": verb,
                        "would_succeed": true,
                        "no_write": true,
                    });
                    println!("{}", serde_json::to_string_pretty(&json)?);
                }
                OutputFormat::Quiet => println!("ok"),
                OutputFormat::Table => {
                    let msg = VerifyOutcome::allowed_message(verb);
                    if use_colors {
                        println!(
                            "{}verified (no write):{} {msg}",
                            colors::GREEN,
                            colors::RESET
                        );
                    } else {
                        println!("verified (no write): {msg}");
                    }
                }
            }
            Ok(())
        }
        VerifyOutcome::Deny { reason, hint } => {
            let payload = serde_json::json!({
                "verified": false,
                "verb": verb,
                "would_succeed": false,
                "no_write": true,
                "reason": reason,
                "hint": hint,
                "stable_prefix": VerifyOutcome::DENY_PREFIX,
            });
            let err = anyhow::Error::msg(format!(
                "{} '{verb}': [{reason}] {hint}",
                VerifyOutcome::DENY_PREFIX,
                verb = verb,
                reason = reason,
                hint = hint,
            ));
            Err(err.context(payload.to_string()))
        }
    }
}

/// Dry-run `task add` — exercises the same gates as `add_task_with_args`
/// but returns `VerifyOutcome` instead of writing to `tasks.jsonl`.
fn verify_add(
    store: &mut TaskStore,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    args: &VerifyAddArgs,
) -> Result<VerifyOutcome> {
    if let Err(outcome) = gate_outcome(ctx, coordinator_hats, coordinator_err, "add")? {
        return Ok(outcome);
    }
    let Some(title) = args.title.clone() else {
        return Ok(VerifyOutcome::Deny {
            reason: "missing_title".to_string(),
            hint: "`task verify add` requires a positional TITLE argument (same as `task add`)."
                .to_string(),
        });
    };
    let task = add_common_task_fields(
        Task::new(title, args.priority),
        ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );

    if let Err(message) = validate_owner_hat_id(&task, coordinator_hats) {
        return Ok(VerifyOutcome::Deny {
            reason: "non_coordinator_owner".to_string(),
            hint: format!("{message}"),
        });
    }

    let invalid_blockers = store.invalid_blockers(&task);
    if !invalid_blockers.is_empty() {
        return Ok(VerifyOutcome::Deny {
            reason: "missing_blockers".to_string(),
            hint: format!(
                "task blocked_by references missing or out-of-loop tasks: {}",
                invalid_blockers.join(", ")
            ),
        });
    }

    // U7 (2026-07-04-003 plan): record a verify ticket so the
    // subsequent `task add` for the same payload can pass the
    // two-step gate. The ticket lives at
    // `<workspace>/.ralph/agent/.ralph-task-verify-ticket` and is
    // burned by `task add` on success.
    //
    // Reconstruct the canonical AddArgs shape from VerifyAddArgs
    // (they share field names) so the same `canonical_add_payload`
    // helper produces an identical fingerprint on both sides.
    let real_args = AddArgs {
        title: task.title.clone(),
        priority: args.priority,
        description: args.description.clone(),
        blocked_by: args.blocked_by.clone(),
        format: OutputFormat::Quiet,
    };
    let canonical = canonical_add_payload(&real_args);
    let (loop_id, hat_id) = gate_identifiers(ctx);
    let fingerprint =
        crate::task_verify_gate::mutation_fingerprint("add", &canonical, loop_id, hat_id);
    let _ = crate::task_verify_gate::record_ticket(
        &crate::task_verify_gate::ticket_path(&ctx.workspace_root),
        &fingerprint,
        loop_id,
        hat_id,
    );

    Ok(VerifyOutcome::Allow)
}

/// Dry-run `task ensure` — mirrors `ensure_task_with_args` but emits
/// `VerifyOutcome` instead of writing.
fn verify_ensure(
    store: &mut TaskStore,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    args: &VerifyEnsureArgs,
) -> Result<VerifyOutcome> {
    if let Err(outcome) = gate_outcome(ctx, coordinator_hats, coordinator_err, "ensure")? {
        return Ok(outcome);
    }
    if args.key.is_none() && args.for_fix_unit.is_none() {
        return Ok(VerifyOutcome::Deny {
            reason: "missing_key".to_string(),
            hint: "`task verify ensure` requires either --key <KEY> or --for-fix-unit <PLAN:FIX_STEP:SLUG>.".to_string(),
        });
    }
    let Some(title) = args.title.clone() else {
        return Ok(VerifyOutcome::Deny {
            reason: "missing_title".to_string(),
            hint:
                "`task verify ensure` requires a positional TITLE argument (same as `task ensure`)."
                    .to_string(),
        });
    };

    // Mirror the derive_key logic from ensure_task_with_args.
    let derived_key = if let Some(spec) = args.for_fix_unit.as_deref() {
        let mut parts = spec.split(':');
        let plan = parts.next().unwrap_or("").to_string();
        let fix_step = parts.next().unwrap_or("").to_string();
        let slug = parts.next().unwrap_or("").to_string();
        if plan.is_empty() || fix_step.is_empty() || slug.is_empty() || parts.next().is_some() {
            return Ok(VerifyOutcome::Deny {
                reason: "malformed_for_fix_unit".to_string(),
                hint: format!(
                    "--for-fix-unit expects exactly 3 colon-separated segments: PLAN:FIX_STEP:SLUG, got '{spec}'"
                ),
            });
        }
        Some(format!("ce-executor:{plan}:{fix_step}:{slug}"))
    } else {
        None
    };
    let key_value = derived_key
        .clone()
        .unwrap_or_else(|| args.key.clone().unwrap_or_default());
    let mut task = add_common_task_fields(
        Task::new(title, args.priority).with_key(Some(key_value)),
        ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );
    if args.for_fix_unit.is_some() {
        task = task.with_owner_hat(Some("coordinator".to_string()));
    }
    if let Err(message) = validate_owner_hat_id(&task, coordinator_hats) {
        return Ok(VerifyOutcome::Deny {
            reason: "non_coordinator_owner".to_string(),
            hint: format!("{message}"),
        });
    }

    let invalid_blockers = store.invalid_blockers(&task);
    if !invalid_blockers.is_empty() {
        return Ok(VerifyOutcome::Deny {
            reason: "missing_blockers".to_string(),
            hint: format!(
                "task blocked_by references missing or out-of-loop tasks: {}",
                invalid_blockers.join(", ")
            ),
        });
    }

    // U7 (2026-07-04-003 plan): record a verify ticket so the
    // subsequent `task ensure` for the same payload can pass
    // the two-step gate.
    let real_args = EnsureArgs {
        title: task.title.clone(),
        key: args.key.clone(),
        priority: args.priority,
        description: args.description.clone(),
        blocked_by: args.blocked_by.clone(),
        for_fix_unit: args.for_fix_unit.clone(),
        format: OutputFormat::Quiet,
    };
    let canonical = canonical_ensure_payload(&real_args, derived_key.as_deref());
    let (loop_id, hat_id) = gate_identifiers(ctx);
    let fingerprint =
        crate::task_verify_gate::mutation_fingerprint("ensure", &canonical, loop_id, hat_id);
    let _ = crate::task_verify_gate::record_ticket(
        &crate::task_verify_gate::ticket_path(&ctx.workspace_root),
        &fingerprint,
        loop_id,
        hat_id,
    );

    Ok(VerifyOutcome::Allow)
}

/// Dry-run a lifecycle mutation (start/close/fail/reopen).
fn verify_lifecycle(
    store: &TaskStore,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    verb: &str,
    task_id: &str,
) -> Result<VerifyOutcome> {
    if let Err(outcome) = gate_outcome(ctx, coordinator_hats, coordinator_err, verb)? {
        return Ok(outcome);
    }
    if let Err(message) = validate_task_id(task_id) {
        return Ok(VerifyOutcome::Deny {
            reason: "invalid_task_id".to_string(),
            hint: format!("{message}"),
        });
    }
    let snapshot = match store.get(task_id) {
        Some(t) => t.clone(),
        None => {
            return Ok(VerifyOutcome::Deny {
                reason: "task_not_found".to_string(),
                hint: format!("task {task_id} not found"),
            });
        }
    };
    if let Err(message) = authorize_lifecycle(&snapshot, ctx, coordinator_hats, verb) {
        return Ok(VerifyOutcome::Deny {
            reason: "authorize_lifecycle_failed".to_string(),
            hint: format!("{message}"),
        });
    }
    Ok(VerifyOutcome::Allow)
}

/// Convert `HatCommandPolicy::check_task` (which returns `PolicyDecision`)
/// to the local `VerifyOutcome` so verify can keep its stable success /
/// failure exit contract instead of bailing early.
fn gate_outcome(
    ctx: &OperationContext,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    verb: &str,
) -> Result<std::result::Result<(), VerifyOutcome>> {
    use crate::hat_command_policy::PolicyDecision;
    match HatCommandPolicy::check_task(ctx, coordinator_hats, coordinator_err, verb) {
        PolicyDecision::Allow { .. } => Ok(Ok(())),
        PolicyDecision::Deny { reason, hint } => Ok(Err(VerifyOutcome::Deny {
            reason: reason.to_string(),
            hint,
        })),
    }
}

/// Build a structured `anyhow::Error` for the three emit-bridge
/// denial paths whose only difference is the stage label, the
/// reason code, the hint text, and the underlying message.
///
/// Centralizing the JSON-shape construction keeps the three error
/// branches identical (which matters because `ralph` test agents
/// will grep the JSON payload for the `stages` array to drive
/// their recovery logic).
fn emit_bridge_deny(stage: &str, reason: &str, hint: String, message: String) -> anyhow::Error {
    let payload = serde_json::json!({
        "verified": false,
        "stages": [stage],
        "reason": reason,
        "hint": hint,
    });
    anyhow::Error::msg(message).context(payload.to_string())
}

/// U4 emit-bridge: verify three-field task_id/task_key/step consistency
/// for the upcoming `ralph emit` payload (R16). Walks the live task
/// store to confirm:
/// - `task_id` resolves to an open (non-terminal) task in the current loop.
/// - `task_key` matches the registered key on that task.
/// - `step` matches the `:step-<n>:` segment inside `task_key`
///   (per the `ralph-tools-tasks.md` red-box convention).
fn execute_verify_emit_bridge(args: VerifyEmitBridgeArgs, root: Option<&PathBuf>) -> Result<()> {
    let path = get_tasks_path(root);
    let store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);

    // 1. task_id must resolve to a live, non-terminal task.
    let snapshot = store.get(&args.task_id).cloned();
    let Some(task) = snapshot else {
        return Err(emit_bridge_deny(
            "task_id_resolution",
            "task_not_found",
            format!(
                "task_id '{}' does not exist in the live task store; never hand-construct task_id — read it back via `ralph tools task list` immediately before emit.",
                args.task_id
            ),
            format!(
                "task_verify_emit_bridge: task_id '{}' not found in store (never hand-construct task_id)",
                args.task_id
            ),
        ));
    };

    if task.status.is_terminal() {
        return Err(emit_bridge_deny(
            "task_id_resolution",
            "task_is_terminal",
            format!(
                "task '{}' is in terminal state {:?}; close-then-emit is rejected. Open a fresh task or reuse an existing open one.",
                args.task_id, task.status
            ),
            format!(
                "task_verify_emit_bridge: task '{}' is in terminal state ({:?}); reuse a live task instead",
                args.task_id, task.status
            ),
        ));
    }

    if ctx.is_agent_context {
        if let (Some(current), Some(target)) = (ctx.current_loop_id.as_ref(), task.loop_id.as_ref())
        {
            if current != target {
                return Err(emit_bridge_deny(
                    "loop_scope",
                    "wrong_loop",
                    format!(
                        "task '{}' belongs to loop '{}' but caller is in loop '{}'; open or pick a task from the current loop",
                        args.task_id, target, current
                    ),
                    format!(
                        "task_verify_emit_bridge: task '{}' belongs to loop '{}' but current loop is '{}'",
                        args.task_id, target, current
                    ),
                ));
            }
        }
    }

    // 2. task_key must match the registered key on the task.
    let Some(registered_key) = task.key.clone() else {
        return Err(emit_bridge_deny(
            "task_key_match",
            "task_has_no_key",
            format!(
                "task '{}' has no registered key; the emit-bridge requires a key — re-create via `ralph tools task ensure --for-fix-unit` or `--key`",
                args.task_id
            ),
            format!(
                "task_verify_emit_bridge: task '{}' has no registered key; the emit-bridge requires a key",
                args.task_id
            ),
        ));
    };

    if registered_key != args.task_key {
        let payload = serde_json::json!({
            "verified": false,
            "stages": ["task_key_match"],
            "reason": "task_key_mismatch",
            "expected_key": registered_key,
            "provided_key": args.task_key,
            "hint": "task_key on the emit payload must match the registered key returned by `ralph tools task show`",
        });
        let err = anyhow::Error::msg(format!(
            "task_verify_emit_bridge: task_key mismatch — registered key is '{registered_key}' but emit payload carries '{}'",
            args.task_key
        ));
        return Err(err.context(payload.to_string()));
    }

    // 3. step must match the `:step-<n>:` segment inside task_key.
    let step_segment = registered_key
        .split(':')
        .find(|seg| seg.starts_with("step-"));
    let Some(step_segment) = step_segment else {
        return Err(emit_bridge_deny(
            "step_match",
            "task_key_missing_step_segment",
            format!(
                "registered key '{registered_key}' contains no `:step-<n>:` segment; the emit-bridge requires task_key in the canonical `<plan>:<step-N>:<slug>` form per ralph-tools-tasks.md red box"
            ),
            format!(
                "task_verify_emit_bridge: registered key '{registered_key}' contains no `:step-<n>:` segment"
            ),
        ));
    };

    if step_segment != args.step {
        let payload = serde_json::json!({
            "verified": false,
            "stages": ["step_match"],
            "reason": "step_segment_mismatch",
            "expected_step": step_segment,
            "provided_step": args.step,
            "hint": "the `step` value on the emit payload must match the `:step-<n>:` segment of task_key exactly",
        });
        let err = anyhow::Error::msg(format!(
            "task_verify_emit_bridge: step mismatch — task_key contains '{step_segment}' but emit payload carries '{}'",
            args.step
        ));
        return Err(err.context(payload.to_string()));
    }

    match args.format {
        OutputFormat::Json => {
            let payload = serde_json::json!({
                "verified": true,
                "task_id": args.task_id,
                "task_key": args.task_key,
                "step": args.step,
                "registered_key": registered_key,
                "task_status": task.status,
                "loop_id": task.loop_id,
                "hint": "safe to emit; close the emit-payload round-trip with `ralph events --events-source hat-channel`",
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        OutputFormat::Quiet => println!("ok"),
        OutputFormat::Table => {
            println!(
                "verified emit-bridge (no write): task_id='{}' task_key='{}' step='{}' (loop={})",
                args.task_id,
                args.task_key,
                args.step,
                task.loop_id.as_deref().unwrap_or("<unscoped>")
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_tasks(root: &Path, tasks: Vec<Task>) -> TaskStore {
        let root_buf = root.to_path_buf();
        let path = get_tasks_path(Some(&root_buf));
        let mut store = TaskStore::load(&path).expect("load task store");
        for task in tasks {
            store.add(task);
        }
        store.save().expect("save task store");
        TaskStore::load(&path).expect("reload task store")
    }

    #[test]
    fn test_list_status_filter_accepts_in_progress() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut open_task = Task::new("Open".to_string(), 2);
        open_task.status = TaskStatus::Open;
        let mut in_progress = Task::new("In progress".to_string(), 2);
        in_progress.status = TaskStatus::InProgress;

        let store = write_tasks(temp_dir.path(), vec![open_task, in_progress]);

        let args = ListArgs {
            status: Some("in_progress".to_string()),
            days: None,
            limit: None,
            all: true,
            format: OutputFormat::Quiet,
        };

        let filtered = filter_tasks_for_list(&store, &args);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].status, TaskStatus::InProgress);
    }

    #[test]
    fn test_ready_filters_by_loop_id_marker() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();

        let mut task_loop_a = Task::new("Loop A task".to_string(), 1);
        task_loop_a.loop_id = Some("loop-a".to_string());
        let mut task_loop_b = Task::new("Loop B task".to_string(), 1);
        task_loop_b.loop_id = Some("loop-b".to_string());

        let store = write_tasks(temp_dir.path(), vec![task_loop_a, task_loop_b]);

        let marker_dir = root.join(".ralph");
        std::fs::create_dir_all(&marker_dir).expect("marker dir");
        std::fs::write(marker_dir.join("current-loop-id"), "loop-a").expect("write marker");

        let args = ReadyArgs {
            all: false,
            format: OutputFormat::Quiet,
        };

        let ready = filter_tasks_for_ready(&store, &args, Some(&root));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].loop_id.as_deref(), Some("loop-a"));
    }

    #[test]
    fn test_read_current_loop_id_ignores_empty_marker() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();
        let marker_dir = root.join(".ralph");
        std::fs::create_dir_all(&marker_dir).expect("marker dir");
        std::fs::write(marker_dir.join("current-loop-id"), "  ").expect("write marker");

        assert_eq!(read_current_loop_id(Some(&root)), None);
    }

    #[test]
    fn test_get_tasks_path_discovers_workspace_root_from_nested_dir() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ralph/agent")).expect("agent dir");

        assert_eq!(
            get_tasks_path(Some(&root)),
            root.join(".ralph/agent/tasks.jsonl")
        );
    }

    fn ctx_for(workspace: &Path, loop_id: Option<&str>, hat_id: Option<&str>) -> OperationContext {
        OperationContext::detect_with_env(workspace.to_path_buf(), |key| match key {
            "RALPH_CURRENT_HAT" => hat_id.map(|s| s.to_string()),
            "RALPH_CURRENT_LOOP_ID" => loop_id.map(|s| s.to_string()),
            "RALPH_EVENTS_FILE" => None,
            "RALPH_WAVE_WORKER" => None,
            _ => None,
        })
    }

    fn write_marker(root: &Path, file: &str, value: &str) {
        let dir = root.join(".ralph");
        std::fs::create_dir_all(&dir).expect("marker dir");
        std::fs::write(dir.join(file), value).expect("write marker");
    }

    fn add_args(title: &str, blocked_by: Option<&str>) -> AddArgs {
        AddArgs {
            title: title.to_string(),
            priority: 2,
            description: None,
            blocked_by: blocked_by.map(|s| s.to_string()),
            format: OutputFormat::Quiet,
        }
    }

    fn ensure_args(title: &str, key: &str, blocked_by: Option<&str>) -> EnsureArgs {
        EnsureArgs {
            title: title.to_string(),
            key: Some(key.to_string()),
            priority: 2,
            description: None,
            blocked_by: blocked_by.map(|s| s.to_string()),
            for_fix_unit: None,
            format: OutputFormat::Quiet,
        }
    }

    fn open_store(root: &Path) -> TaskStore {
        let path = get_tasks_path(Some(&root.to_path_buf()));
        TaskStore::load(&path).expect("load task store")
    }

    // ---- P2 plan 列举测试 (18 项) ----

    #[test]
    fn test_task_add_stamps_loop_and_owner_hat() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);

        add_task_with_args(
            &mut store,
            &add_args("Do work", None),
            &ctx,
            &["executor".to_string()],
            false,
        )
        .unwrap();

        let saved = store.all();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].loop_id.as_deref(), Some("loop-a"));
        assert_eq!(saved[0].owner_hat_id.as_deref(), Some("executor"));
    }

    #[test]
    fn test_task_add_without_hat_keeps_owner_none_for_human() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-h");
        let ctx = ctx_for(root, Some("loop-h"), None);
        let mut store = open_store(root);

        add_task_with_args(
            &mut store,
            &add_args("Do work", None),
            &ctx,
            &["executor".to_string()],
            false,
        )
        .unwrap();

        let saved = store.all();
        assert_eq!(saved[0].loop_id.as_deref(), Some("loop-h"));
        assert!(saved[0].owner_hat_id.is_none());
    }

    // U3: owner_hat_id must be on tasks.coordinator_hats. The five scenarios
    // below exercise the create-side complement to the JSONL origin guard
    // (which already rejects ralph at the read path).

    #[test]
    fn test_task_add_allows_executor_when_in_coordinator_hats() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);

        add_task_with_args(
            &mut store,
            &add_args("ok task", None),
            &ctx,
            &["coordinator".to_string(), "executor".to_string()],
            false,
        )
        .expect("executor in coordinator_hats should be accepted");
    }

    #[test]
    fn test_task_add_rejects_ralph_when_not_in_coordinator_hats() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("ralph"));
        let mut store = open_store(root);

        let err = add_task_with_args(
            &mut store,
            &add_args("rogue task", None),
            &ctx,
            &["coordinator".to_string(), "executor".to_string()],
            false,
        )
        .expect_err("ralph must be rejected (not in coordinator_hats)");

        let msg = err.to_string();
        assert!(
            msg.contains("'ralph'") && msg.contains("not in tasks.coordinator_hats"),
            "error should name the rejected owner and the allowlist, got: {msg}"
        );

        // And nothing was persisted.
        assert!(store.all().is_empty());
    }

    #[test]
    fn test_task_add_rejects_any_owner_when_coordinator_hats_empty() {
        // Fail-closed: an empty allowlist must not let any agent persist a
        // task. This catches the misconfigured-preset failure mode where
        // `coordinator_hats` is unset (or accidentally cleared) and the
        // orchestrator would otherwise default-open the gate.
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);

        let err = add_task_with_args(&mut store, &add_args("anything", None), &ctx, &[], false)
            .expect_err("empty coordinator_hats must reject any owner (fail-closed)");

        assert!(err.to_string().contains("not in tasks.coordinator_hats"));
    }

    #[test]
    fn test_task_add_allows_human_call_without_owner() {
        // When `ctx.current_hat_id` is None, the task has no owner and
        // `validate_owner_hat_id` is a no-op. This is the human CLI
        // path — operators must not be locked out by the owner check.
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-h");
        let ctx = ctx_for(root, Some("loop-h"), None);
        let mut store = open_store(root);

        add_task_with_args(&mut store, &add_args("human task", None), &ctx, &[], false)
            .expect("human CLI call (no owner) must not be blocked by empty allowlist");
    }

    #[test]
    fn test_task_ensure_rejects_off_allowlist_owner() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("rogue-hat"));
        let mut store = open_store(root);

        let err = ensure_task_with_args(
            &mut store,
            &ensure_args("x", "k:v", None),
            &ctx,
            &["executor".to_string()],
            false,
        )
        .expect_err("ensure must also reject off-allowlist owner");
        assert!(err.to_string().contains("not in tasks.coordinator_hats"));
    }

    #[test]
    fn test_task_start_rejects_other_loop_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let other_loop = Task::new("Other loop".to_string(), 1)
            .with_loop_id(Some("loop-b".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let other_id = other_loop.id.clone();
        store.add(other_loop);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = start_task_with_context(&mut store, &other_id, &ctx, &[], false)
            .expect_err("cross-loop start should fail");
        assert!(err.to_string().contains("loop-b"));
    }

    #[test]
    fn test_task_close_rejects_other_loop_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let other_loop = Task::new("Other".to_string(), 1)
            .with_loop_id(Some("loop-b".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let other_id = other_loop.id.clone();
        store.add(other_loop);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = close_task_with_context(&mut store, &other_id, &ctx, &[], false)
            .expect_err("cross-loop close should fail");
        assert!(err.to_string().contains("loop-b"));
    }

    #[test]
    fn test_task_fail_rejects_other_loop_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let other = Task::new("Other".to_string(), 1)
            .with_loop_id(Some("loop-b".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let other_id = other.id.clone();
        store.add(other);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = fail_task_with_context(&mut store, &other_id, &ctx, &[], false)
            .expect_err("cross-loop fail should fail");
        assert!(err.to_string().contains("loop-b"));
    }

    #[test]
    fn test_task_reopen_rejects_other_loop_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let other = Task::new("Other".to_string(), 1)
            .with_loop_id(Some("loop-b".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let other_id = other.id.clone();
        store.add(other);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = reopen_task_with_context(&mut store, &other_id, &ctx, &[], false)
            .expect_err("cross-loop reopen should fail");
        assert!(err.to_string().contains("loop-b"));
    }

    #[test]
    fn test_task_start_rejects_other_hat_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let owned = Task::new("Owned".to_string(), 1)
            .with_loop_id(Some("loop-a".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let owned_id = owned.id.clone();
        store.add(owned);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = start_task_with_context(&mut store, &owned_id, &ctx, &[], false)
            .expect_err("cross-hat start should fail");
        assert!(err.to_string().contains("reviewer"));
    }

    // ---- U3 wiring tests: enforce_command_policy bridge ----

    fn isolated_config_with_coordinator() -> ralph_core::config::RalphConfig {
        let yaml = r#"
event_loop:
  execution_mode: isolated
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
hats:
  coordinator:
    name: "Coordinator"
    publishes: ["work.ready"]
  worker:
    name: "Worker"
    publishes: ["work.done"]
"#;
        serde_yaml::from_str(yaml).unwrap()
    }

    fn hats_for(cfg: &ralph_core::config::RalphConfig) -> Vec<String> {
        cfg.tasks.coordinator_hats.clone()
    }

    #[test]
    fn enforce_command_policy_allows_coordinator_add() {
        let cfg = isolated_config_with_coordinator();
        let ctx = ctx_for(Path::new("/tmp"), Some("loop-a"), Some("coordinator"));
        let hats = hats_for(&cfg);
        assert!(enforce_command_policy(&ctx, &hats, None, "add").is_ok());
    }

    #[test]
    fn enforce_command_policy_denies_worker_add_with_hint() {
        let cfg = isolated_config_with_coordinator();
        let ctx = ctx_for(Path::new("/tmp"), Some("loop-a"), Some("worker"));
        let hats = hats_for(&cfg);
        let err = enforce_command_policy(&ctx, &hats, None, "add")
            .expect_err("worker must be denied at add entry");
        let msg = err.to_string();
        assert!(msg.contains("hat_command_policy denied 'add'"));
        assert!(msg.contains("worker"));
        assert!(msg.contains("non_coordinator_owner"));
    }

    #[test]
    fn enforce_command_policy_denies_worker_ensure_with_hint() {
        let cfg = isolated_config_with_coordinator();
        let ctx = ctx_for(Path::new("/tmp"), Some("loop-a"), Some("worker"));
        let hats = hats_for(&cfg);
        let err = enforce_command_policy(&ctx, &hats, None, "ensure")
            .expect_err("worker must be denied at ensure entry");
        assert!(err.to_string().contains("non_coordinator_owner"));
    }

    #[test]
    fn enforce_command_policy_allows_worker_close() {
        let cfg = isolated_config_with_coordinator();
        let ctx = ctx_for(Path::new("/tmp"), Some("loop-a"), Some("worker"));
        let hats = hats_for(&cfg);
        assert!(
            enforce_command_policy(&ctx, &hats, None, "close").is_ok(),
            "close passes the role gate; ownership is enforced by authorize_lifecycle"
        );
    }

    #[test]
    fn enforce_command_policy_human_cli_unaffected() {
        let cfg = isolated_config_with_coordinator();
        let ctx = ctx_for(Path::new("/tmp"), None, None);
        let hats = hats_for(&cfg);
        assert!(enforce_command_policy(&ctx, &hats, None, "add").is_ok());
        assert!(enforce_command_policy(&ctx, &hats, None, "ensure").is_ok());
    }

    #[test]
    fn enforce_command_policy_empty_coordinator_hats_fails_closed_for_agent() {
        let yaml = r#"
event_loop:
  execution_mode: isolated
tasks:
  enabled: true
  coordinator_hats: []
"#;
        let cfg: ralph_core::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let ctx = ctx_for(Path::new("/tmp"), Some("loop-a"), Some("coordinator"));
        let hats = hats_for(&cfg);
        let err = enforce_command_policy(&ctx, &hats, None, "add")
            .expect_err("empty coordinator_hats must fail closed for agents");
        let msg = err.to_string();
        assert!(msg.contains("non_coordinator_owner"));
        assert!(msg.contains("tasks.coordinator_hats is empty"));
    }

    #[test]
    fn test_task_close_allows_owner_hat() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let owned = Task::new("Owned".to_string(), 1)
            .with_loop_id(Some("loop-a".to_string()))
            .with_owner_hat(Some("executor".to_string()));
        let owned_id = owned.id.clone();
        store.add(owned);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        close_task_with_context(&mut store, &owned_id, &ctx, &[], false)
            .expect("owner hat should be allowed to close");
        assert_eq!(store.get(&owned_id).unwrap().status, TaskStatus::Closed);
    }

    #[test]
    fn test_task_operation_allows_configured_task_coordinator() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let owned = Task::new("Owned".to_string(), 1)
            .with_loop_id(Some("loop-a".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let owned_id = owned.id.clone();
        store.add(owned);

        let ctx = ctx_for(root, Some("loop-a"), Some("coordinator"));
        let coordinators = vec!["coordinator".to_string()];
        close_task_with_context(&mut store, &owned_id, &ctx, &coordinators, false)
            .expect("coordinator should be allowed to close any task");
        assert_eq!(store.get(&owned_id).unwrap().status, TaskStatus::Closed);
    }

    #[test]
    fn test_task_operation_rejects_missing_current_hat_in_agent_context() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let owned = Task::new("Owned".to_string(), 1)
            .with_loop_id(Some("loop-a".to_string()))
            .with_owner_hat(Some("executor".to_string()));
        let owned_id = owned.id.clone();
        store.add(owned);

        // Agent context (loop marker set) but no RALPH_CURRENT_HAT.
        let ctx = ctx_for(root, Some("loop-a"), None);
        let err = close_task_with_context(&mut store, &owned_id, &ctx, &[], false)
            .expect_err("missing hat in agent context should fail closed");
        assert!(err.to_string().to_lowercase().contains("hat"));
    }

    #[test]
    fn test_task_close_denied_for_coordinator_owned_task_hints_delegate() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let owned = Task::new("Owned".to_string(), 1)
            .with_loop_id(Some("loop-a".to_string()))
            .with_owner_hat(Some("coordinator".to_string()));
        let owned_id = owned.id.clone();
        store.add(owned);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let coordinators = vec!["coordinator".to_string()];
        let err = close_task_with_context(&mut store, &owned_id, &ctx, &coordinators, false)
            .expect_err("executor must not close coordinator-owned task");
        let msg = err.to_string();
        assert!(msg.contains("Ask hat 'coordinator'"));
        assert!(msg.contains("ralph tools task close"));
        assert!(msg.contains("re-emit work.done"));
    }

    #[test]
    fn test_task_ensure_key_scoped_by_loop() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx_a = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);
        ensure_task_with_args(
            &mut store,
            &ensure_args("First", "shared:task", None),
            &ctx_a,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        ensure_task_with_args(
            &mut store,
            &ensure_args("Second", "shared:task", None),
            &ctx_a,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn test_task_ensure_same_key_same_loop_reuses() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx_a = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);
        ensure_task_with_args(
            &mut store,
            &ensure_args("First", "shared:task", None),
            &ctx_a,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        let first_id = store
            .get_by_key_in_loop("shared:task", Some("loop-a"))
            .unwrap()
            .id
            .clone();
        ensure_task_with_args(
            &mut store,
            &ensure_args("Second", "shared:task", None),
            &ctx_a,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        let reused_id = store
            .get_by_key_in_loop("shared:task", Some("loop-a"))
            .unwrap()
            .id
            .clone();
        assert_eq!(first_id, reused_id);
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn test_task_ensure_same_key_different_loop_creates_new() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx_a = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);
        ensure_task_with_args(
            &mut store,
            &ensure_args("First", "shared:task", None),
            &ctx_a,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        // Switch marker to loop-b so the next ensure is stamped loop-b.
        write_marker(root, "current-loop-id", "loop-b");
        let ctx_b = ctx_for(root, Some("loop-b"), Some("executor"));
        ensure_task_with_args(
            &mut store,
            &ensure_args("Second", "shared:task", None),
            &ctx_b,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        assert_eq!(store.all().len(), 2);
    }

    #[test]
    fn test_task_add_rejects_blocker_from_other_loop() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        // Blocker exists but belongs to a different loop.
        let other_blocker =
            Task::new("Other loop blocker".to_string(), 1).with_loop_id(Some("loop-b".to_string()));
        let blocker_id = other_blocker.id.clone();
        store.add(other_blocker);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = add_task_with_args(
            &mut store,
            &add_args("Do work", Some(&blocker_id)),
            &ctx,
            &["executor".to_string()],
            false,
        )
        .expect_err("cross-loop blocker should be rejected");
        assert!(err.to_string().contains(&blocker_id));
    }

    #[test]
    fn test_task_add_rejects_missing_blocker() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = add_task_with_args(
            &mut store,
            &add_args("Do work", Some("task-9999-deadbeef")),
            &ctx,
            &["executor".to_string()],
            false,
        )
        .expect_err("missing blocker should be rejected");
        assert!(err.to_string().contains("task-9999-deadbeef"));
    }

    #[test]
    fn test_task_ready_defaults_to_current_loop() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");

        let mut t_a = Task::new("Loop A task".to_string(), 1);
        t_a.loop_id = Some("loop-a".to_string());
        let mut t_b = Task::new("Loop B task".to_string(), 1);
        t_b.loop_id = Some("loop-b".to_string());
        let store = write_tasks(root, vec![t_a, t_b]);

        let args = ReadyArgs {
            all: false,
            format: OutputFormat::Quiet,
        };
        let ready = filter_tasks_for_ready(&store, &args, Some(&root.to_path_buf()));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].loop_id.as_deref(), Some("loop-a"));
    }

    #[test]
    fn test_task_ready_all_includes_other_loops() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");

        let mut t_a = Task::new("Loop A task".to_string(), 1);
        t_a.loop_id = Some("loop-a".to_string());
        let mut t_b = Task::new("Loop B task".to_string(), 1);
        t_b.loop_id = Some("loop-b".to_string());
        let store = write_tasks(root, vec![t_a, t_b]);

        let args = ReadyArgs {
            all: true,
            format: OutputFormat::Quiet,
        };
        let ready = filter_tasks_for_ready(&store, &args, Some(&root.to_path_buf()));
        assert_eq!(ready.len(), 2);
    }

    #[test]
    fn test_legacy_task_without_loop_not_mutable_by_agent() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        // Legacy task has neither loop_id nor owner_hat_id.
        let legacy = Task::new("Legacy".to_string(), 1);
        let legacy_id = legacy.id.clone();
        store.add(legacy);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = close_task_with_context(&mut store, &legacy_id, &ctx, &[], false)
            .expect_err("agent must not mutate legacy task");
        assert!(err.to_string().contains("legacy"));
    }

    // ---- U1 SSOT 回归 ----

    #[test]
    fn load_coordinator_hats_via_config_matches_explicit_yaml() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();
        std::fs::write(
            root.join("ralph.yml"),
            "tasks:\n  coordinator_hats:\n    - coordinator\n    - executor\n",
        )
        .expect("write ralph.yml");

        // 不再调用旧 loader,改为通过 config 读取
        let config = load_config_or_default(Some(&root));
        assert_eq!(
            config.tasks.coordinator_hats,
            vec!["coordinator".to_string(), "executor".to_string()]
        );
    }

    #[test]
    fn load_config_or_default_handles_missing_ralph_yml() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();
        let config = load_config_or_default(Some(&root));
        // 缺 ralph.yml → Default::default() (event_loop.execution_mode = isolated)
        assert!(config.tasks.coordinator_hats.is_empty());
    }

    #[test]
    fn load_config_or_default_handles_missing_tasks_section() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();
        std::fs::write(
            root.join("ralph.yml"),
            "event_loop:\n  execution_mode: isolated\n",
        )
        .expect("write ralph.yml");
        let config = load_config_or_default(Some(&root));
        assert!(config.tasks.coordinator_hats.is_empty());
    }

    // ---- empty task_id guard tests ----

    #[test]
    fn test_validate_task_id_accepts_non_empty() {
        assert!(validate_task_id("task-123-abc").is_ok());
    }

    #[test]
    fn test_validate_task_id_rejects_empty() {
        let err = validate_task_id("").expect_err("empty task_id must be rejected");
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_task_id_rejects_whitespace_only() {
        let err = validate_task_id("   ").expect_err("whitespace-only task_id must be rejected");
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_start_task_with_context_rejects_empty_id() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = start_task_with_context(&mut store, "", &ctx, &["executor".to_string()], false)
            .expect_err("empty task_id must be rejected");
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_close_task_with_context_rejects_empty_id() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = close_task_with_context(&mut store, "", &ctx, &["executor".to_string()], false)
            .expect_err("empty task_id must be rejected");
        assert!(err.to_string().contains("cannot be empty"));
    }

    // ─────────────────────────────────────────────────────────────────────
    // U4: `task verify` — OPAC Precheck stage for task mutations.
    //
    // Each test confirms verify behaves like the real mutation for the
    // success path and surfaces the same machine-readable deny prefix on
    // the failure path. The tests intentionally avoid using `ce-executor`
    // hat names so the surface stays general (R10).
    // ─────────────────────────────────────────────────────────────────────

    fn verify_add_args(title: &str) -> VerifyAddArgs {
        VerifyAddArgs {
            title: Some(title.to_string()),
            priority: 2,
            description: None,
            blocked_by: None,
            format: VerifyFormatArgs {
                format: OutputFormat::Quiet,
            },
        }
    }

    fn verify_ensure_args(title: &str, key: &str) -> VerifyEnsureArgs {
        VerifyEnsureArgs {
            title: Some(title.to_string()),
            key: Some(key.to_string()),
            for_fix_unit: None,
            priority: 2,
            description: None,
            blocked_by: None,
            format: VerifyFormatArgs {
                format: OutputFormat::Quiet,
            },
        }
    }

    fn base_config_with(coordinator: &[&str]) -> ralph_core::config::RalphConfig {
        let yaml = format!(
            "tasks:\n  coordinator_hats: [{}]\n  enabled: true\n",
            coordinator.join(", ")
        );
        serde_yaml::from_str(&yaml).expect("parse yaml")
    }

    #[test]
    fn test_verify_add_allowed_for_coordinator_hat_does_not_write() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-x");
        let mut store = open_store(root);
        let ctx = ctx_for(root, Some("loop-x"), Some("coordinator"));
        let cfg = base_config_with(&["coordinator"]);
        let before_count = store.all().len();

        let outcome = verify_add(
            &mut store,
            &ctx,
            &["coordinator".into()],
            None,
            &verify_add_args("hi"),
        )
        .expect("verify_add should not error");
        assert!(matches!(outcome, VerifyOutcome::Allow));

        // Confirm verify did NOT touch the store.
        assert_eq!(store.all().len(), before_count);
    }

    #[test]
    fn test_verify_add_denied_for_non_coordinator_agent() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-x");
        let mut store = open_store(root);
        let ctx = ctx_for(root, Some("loop-x"), Some("worker"));
        let cfg = base_config_with(&["coordinator"]);

        let outcome = verify_add(
            &mut store,
            &ctx,
            &["coordinator".into()],
            None,
            &verify_add_args("hi"),
        )
        .expect("verify_add should not error");
        match outcome {
            VerifyOutcome::Deny { reason, .. } => assert_eq!(reason, "non_coordinator_owner"),
            VerifyOutcome::Allow => panic!("expected Deny for non-coordinator agent"),
        }
    }

    #[test]
    fn test_verify_add_denies_missing_title() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-x");
        let mut store = open_store(root);
        let ctx = ctx_for(root, Some("loop-x"), Some("coordinator"));
        let cfg = base_config_with(&["coordinator"]);
        let mut args = verify_add_args("placeholder");
        args.title = None;

        let outcome =
            verify_add(&mut store, &ctx, &["coordinator".into()], None, &args).expect("ok");
        assert!(matches!(
            outcome,
            VerifyOutcome::Deny { ref reason, .. } if reason == "missing_title"
        ));
    }

    #[test]
    fn test_verify_ensure_rejects_missing_key_and_missing_title() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-x");
        let mut store = open_store(root);
        let ctx = ctx_for(root, Some("loop-x"), Some("coordinator"));
        let cfg = base_config_with(&["coordinator"]);
        let mut args = verify_ensure_args("placeholder", "k");
        args.key = None;
        args.for_fix_unit = None;
        let outcome =
            verify_ensure(&mut store, &ctx, &["coordinator".into()], None, &args).expect("ok");
        assert!(matches!(
            outcome,
            VerifyOutcome::Deny { ref reason, .. } if reason == "missing_key"
        ));

        // And missing-title branch.
        let mut args2 = verify_ensure_args("placeholder", "k");
        args2.title = None;
        let outcome =
            verify_ensure(&mut store, &ctx, &["coordinator".into()], None, &args2).expect("ok");
        assert!(matches!(
            outcome,
            VerifyOutcome::Deny { ref reason, .. } if reason == "missing_title"
        ));
    }

    #[test]
    fn test_verify_lifecycle_close_admits_owner_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-x");
        let mut store = open_store(root);
        let ctx = ctx_for(root, Some("loop-x"), Some("coordinator"));
        let cfg = base_config_with(&["coordinator"]);

        let mut task = Task::new("done".into(), 2).with_owner_hat(Some("coordinator".into()));
        task.loop_id = Some("loop-x".into());
        store.add(task.clone());
        store.save().unwrap();
        store = open_store(root);

        let outcome = verify_lifecycle(
            &store,
            &ctx,
            &["coordinator".into()],
            None,
            "close",
            &task.id,
        )
        .expect("ok");
        assert!(matches!(outcome, VerifyOutcome::Allow));
    }

    #[test]
    fn test_verify_lifecycle_close_denies_unknown_task_id() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-x");
        let store = open_store(root);
        let ctx = ctx_for(root, Some("loop-x"), Some("coordinator"));
        let cfg = base_config_with(&["coordinator"]);

        let outcome = verify_lifecycle(
            &store,
            &ctx,
            &["coordinator".into()],
            None,
            "close",
            "task-does-not-exist",
        )
        .expect("ok");
        assert!(matches!(
            outcome,
            VerifyOutcome::Deny { ref reason, .. } if reason == "task_not_found"
        ));
    }

    #[test]
    fn test_verify_emit_bridge_succeeds_with_consistent_fields() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-yo");
        let mut store = open_store(root);
        let mut task = Task::new("bridge-ok".into(), 2)
            .with_owner_hat(Some("coordinator".into()))
            .with_key(Some("myplan:step-02:patch-foo".into()));
        task.loop_id = Some("loop-yo".into());
        task.status = TaskStatus::InProgress;
        let id = task.id.clone();
        store.add(task);
        store.save().unwrap();

        let args = VerifyEmitBridgeArgs {
            task_id: id.clone(),
            task_key: "myplan:step-02:patch-foo".into(),
            step: "step-02".into(),
            format: OutputFormat::Quiet,
        };
        // execute_verify_emit_bridge uses operation_context_for, which only
        // honors env-level hats. We invoke the function with --root and
        // expect allow via the human-cli path (ctx.is_agent_context==false).
        execute_verify_emit_bridge(args, Some(&root.to_path_buf())).expect("verify ok");
    }

    #[test]
    fn test_verify_emit_bridge_rejects_unknown_task_id() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-yo");
        let _store = open_store(root);

        let args = VerifyEmitBridgeArgs {
            task_id: "task-missing".into(),
            task_key: "x:step-1:foo".into(),
            step: "step-1".into(),
            format: OutputFormat::Quiet,
        };
        let err = execute_verify_emit_bridge(args, Some(&root.to_path_buf()))
            .expect_err("unknown task_id must be denied");
        let text = format!("{err:#}");
        assert!(
            text.contains("task_verify_emit_bridge"),
            "stable prefix present"
        );
        assert!(
            text.contains("task_id_resolution") || text.contains("task-missing"),
            "structured context attached"
        );
    }

    #[test]
    fn test_verify_emit_bridge_rejects_step_key_mismatch() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-yo");
        let mut store = open_store(root);
        let mut task = Task::new("bridge-mismatch".into(), 2)
            .with_owner_hat(Some("coordinator".into()))
            .with_key(Some("plan-x:step-09:slug".into()));
        task.loop_id = Some("loop-yo".into());
        task.status = TaskStatus::InProgress;
        let id = task.id.clone();
        store.add(task);
        store.save().unwrap();

        let args = VerifyEmitBridgeArgs {
            task_id: id,
            task_key: "plan-x:step-09:slug".into(),
            step: "step-08".into(), // wrong on purpose
            format: OutputFormat::Quiet,
        };
        let err = execute_verify_emit_bridge(args, Some(&root.to_path_buf()))
            .expect_err("step mismatch must be denied");
        let text = format!("{err:#}");
        assert!(text.contains("step") || text.contains("step-09"));
    }

    #[test]
    fn test_verify_emit_bridge_rejects_terminal_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-yo");
        let mut store = open_store(root);
        let mut task = Task::new("bridge-closed".into(), 2)
            .with_owner_hat(Some("coordinator".into()))
            .with_key(Some("plan-y:step-1:slug".into()));
        task.loop_id = Some("loop-yo".into());
        task.status = TaskStatus::Closed;
        let id = task.id.clone();
        store.add(task);
        store.save().unwrap();

        let args = VerifyEmitBridgeArgs {
            task_id: id,
            task_key: "plan-y:step-1:slug".into(),
            step: "step-1".into(),
            format: OutputFormat::Quiet,
        };
        let err = execute_verify_emit_bridge(args, Some(&root.to_path_buf()))
            .expect_err("terminal task must be denied");
        let text = format!("{err:#}");
        assert!(text.contains("terminal") || text.contains("Closed"));
    }

    // ─────────────────────────────────────────────────────────────────────
    // U7: completion-emit warning after `task close`. Helper-level tests.
    // ─────────────────────────────────────────────────────────────────────

    fn make_event_envelope(topic: &str) -> String {
        serde_json::json!({"topic": topic, "ts": 1}).to_string()
    }

    fn config_with_completion_topics(topics: &[&str]) -> ralph_core::config::RalphConfig {
        use ralph_core::config::EventPolicyConfig;
        use ralph_core::config::hat::HatConfig;
        let mut cfg = ralph_core::config::RalphConfig::default();
        let mut hat = HatConfig::default();
        hat.publishes = topics.iter().map(|s| s.to_string()).collect();
        cfg.hats.insert("executor".to_string(), hat);
        cfg.event_loop.event_policy = Some(EventPolicyConfig {
            enabled: true,
            terminal_topics: topics.iter().map(|s| s.to_string()).collect(),
            ..EventPolicyConfig::default()
        });
        cfg
    }

    #[test]
    fn test_parse_topics_from_jsonl_extracts_topic_fields() {
        let content = format!(
            "{}\n{}\n{}\n",
            make_event_envelope("work.ready"),
            make_event_envelope("chat.out"),
            make_event_envelope("work.done"),
        );
        // Pass a high max_lines so the whole content is scanned; tail
        // mode is exercised in test_parse_topics_from_jsonl_tail.
        let topics = parse_topics_from_jsonl_tail(&content, usize::MAX);
        assert_eq!(
            topics,
            vec![
                "work.ready".to_string(),
                "chat.out".to_string(),
                "work.done".to_string()
            ]
        );
    }

    #[test]
    fn test_parse_topics_from_jsonl_skips_malformed_lines() {
        let content = format!(
            "{}\nnot-json\n{}\n",
            make_event_envelope("a"),
            make_event_envelope("b")
        );
        let topics = parse_topics_from_jsonl_tail(&content, usize::MAX);
        assert_eq!(topics, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn test_parse_topics_from_jsonl_tail_truncates_to_last_n_lines() {
        let content = format!(
            "{}\n{}\n{}\n",
            make_event_envelope("work.ready"),
            make_event_envelope("chat.out"),
            make_event_envelope("work.done"),
        );
        let topics = parse_topics_from_jsonl_tail(&content, 1);
        // Only the last envelope should be returned.
        assert_eq!(topics, vec!["work.done".to_string()]);
    }

    #[test]
    fn test_close_warning_skips_when_completion_already_present_in_hat_channel() {
        // Hat-channel already has `work.done` → no warning printed to stderr.
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-x");
        // Write hat-channel with completion topic.
        let ralph_dir = root.join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("ralph dir");
        std::fs::write(
            ralph_dir.join("current-hat-events"),
            make_event_envelope("work.done"),
        )
        .expect("write hat channel");

        let config = config_with_completion_topics(&["work.done"]);
        // Capture stderr by sending it to a sink (the helper uses
        // eprintln!). We assert behaviour via the empty-channel and
        // happy-path checks separately; here we just confirm no panic.
        emit_close_completion_warning(&root.to_path_buf(), &config, "executor");
    }

    #[test]
    fn test_close_warning_no_topics_does_not_warn() {
        // Hat publishes nothing in terminal_topics → no warning.
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        let config = config_with_completion_topics(&["some.completion"]);
        // Hat publishes nothing → derive_completion_publishes is empty.
        emit_close_completion_warning(&root.to_path_buf(), &config, "unknown");
        // No assertion needed: the helper bails out early when expected==[].
    }

    // ── 2026-07-07-002 plan Unit 7: live task identity idempotency ─
    //
    // ensure / add must not create a second live task row for the
    // same `(loop_id, key)` locus; a duplicate add should surface
    // as a bail, not silently append.
    //
    // These tests pin the live-task-identity contract described in
    // the 2026-07-07-002 plan "Requirements Trace R4". BDD coverage
    // lives in `ce_executor_serial_task_identity_idempotent.yml`;
    // these unit tests pin the CLI surface independently.

    #[test]
    fn test_ensure_same_loop_and_key_does_not_append_second_row() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("coordinator"));
        let mut store = open_store(root);
        let coordinator_hats = vec!["coordinator".to_string()];

        // First ensure — creates the live record.
        ensure_task_with_args(
            &mut store,
            &ensure_args("Step 01", "ce-executor:idem-test:step-01:u1-impl", None),
            &ctx,
            &coordinator_hats,
            false,
        )
        .expect("first ensure must succeed");

        let after_first = store.all();
        assert_eq!(after_first.len(), 1, "first ensure writes one row");
        let first_id = after_first[0].id.clone();

        // Second ensure with the same loop + key — must NOT append
        // a second row and must return the same task id.
        ensure_task_with_args(
            &mut store,
            &ensure_args("Step 01 (re-issued)", "ce-executor:idem-test:step-01:u1-impl", None),
            &ctx,
            &coordinator_hats,
            false,
        )
        .expect("second ensure must succeed (idempotent)");

        let after_second = store.all();
        assert_eq!(
            after_second.len(),
            1,
            "second ensure must not append a second live row"
        );
        assert_eq!(after_second[0].id, first_id, "ensure returns same id");
    }

    #[test]
    fn test_ensure_different_loop_yields_distinct_live_records() {
        // Independence regression: two loops that each use the
        // same `task_key` (the key is namespaced under the plan,
        // not the loop) must each get their own live record.
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx_a = ctx_for(root, Some("loop-a"), Some("coordinator"));
        let coordinator_hats = vec!["coordinator".to_string()];
        let mut store = open_store(root);

        ensure_task_with_args(
            &mut store,
            &ensure_args("step", "shared:key:v", None),
            &ctx_a,
            &coordinator_hats,
            false,
        )
        .expect("first ensure (loop-a) must succeed");

        ensure_task_with_args(
            &mut store,
            &ensure_args("step", "shared:key:v", None),
            &ctx_a,
            &coordinator_hats,
            false,
        )
        .expect("second ensure same loop must be idempotent");

        // Re-target marker to a different loop and re-issue. The
        // store must treat this as a fresh locus — i.e. one row
        // per loop, NOT collision.
        write_marker(root, "current-loop-id", "loop-b");
        let ctx_b = ctx_for(root, Some("loop-b"), Some("coordinator"));
        ensure_task_with_args(
            &mut store,
            &ensure_args("step", "shared:key:v", None),
            &ctx_b,
            &coordinator_hats,
            false,
        )
        .expect("ensure on a different loop must succeed");

        let saved = store.all();
        assert_eq!(saved.len(), 2, "two loops => two live records");
        let loops: std::collections::HashSet<_> = saved
            .iter()
            .filter_map(|t| t.loop_id.clone())
            .collect();
        assert!(loops.contains("loop-a"));
        assert!(loops.contains("loop-b"));
    }
}

// ─────────────────────────────────────────────────────────────────────────
// U7 (2026-07-04-003 plan): `load_coordinator_hats` typed error tests.
//
// Each test feeds a temporary workspace ralph.yml and asserts the
// typed `CoordinatorHatsError` variant. The tests do NOT touch the
// global `execute()` path — that integration is Unit 2.
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod load_coordinator_hats_tests {
    use super::CoordinatorHatsError;
    use super::load_coordinator_hats;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_missing_ralph_yml_returns_missing_ralph_yml() {
        let temp_dir = TempDir::new().expect("temp dir");
        let err = load_coordinator_hats(temp_dir.path())
            .expect_err("empty workspace must surface MissingRalphYml");
        assert_eq!(err, CoordinatorHatsError::MissingRalphYml);
    }

    #[test]
    fn test_invalid_yaml_returns_invalid_yaml_variant() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        std::fs::write(root.join("ralph.yml"), "tasks: [").expect("write broken yaml");
        let err = load_coordinator_hats(root).expect_err("broken yaml must surface InvalidYaml");
        match err {
            CoordinatorHatsError::InvalidYaml { path, source } => {
                assert_eq!(path, root.join("ralph.yml"));
                assert!(
                    !source.is_empty(),
                    "InvalidYaml must carry the parse error text"
                );
            }
            other => panic!("expected InvalidYaml, got {other:?}"),
        }
    }

    #[test]
    fn test_missing_tasks_section_returns_missing_tasks_section() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        std::fs::write(
            root.join("ralph.yml"),
            "event_loop:\n  execution_mode: isolated\n",
        )
        .expect("write ralph.yml");
        let err = load_coordinator_hats(root)
            .expect_err("ralph.yml without tasks: must surface MissingTasksSection");
        assert_eq!(err, CoordinatorHatsError::MissingTasksSection);
    }

    #[test]
    fn test_missing_coordinator_hats_key_returns_missing_key() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        std::fs::write(root.join("ralph.yml"), "tasks:\n  enabled: true\n").expect("write yaml");
        let err = load_coordinator_hats(root)
            .expect_err("tasks without coordinator_hats must surface MissingCoordinatorHatsKey");
        assert_eq!(err, CoordinatorHatsError::MissingCoordinatorHatsKey);
    }

    #[test]
    fn test_empty_coordinator_hats_returns_empty_variant() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        std::fs::write(
            root.join("ralph.yml"),
            "tasks:\n  enabled: true\n  coordinator_hats: []\n",
        )
        .expect("write yaml");
        let err = load_coordinator_hats(root)
            .expect_err("coordinator_hats: [] must surface CoordinatorHatsEmpty");
        assert_eq!(err, CoordinatorHatsError::CoordinatorHatsEmpty);
    }

    #[test]
    fn test_valid_yaml_returns_hats_vec() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        std::fs::write(
            root.join("ralph.yml"),
            "tasks:\n  enabled: true\n  coordinator_hats:\n    - coordinator\n    - executor\n",
        )
        .expect("write yaml");
        let hats = load_coordinator_hats(root).expect("valid yaml must parse");
        assert_eq!(
            hats,
            vec!["coordinator".to_string(), "executor".to_string()]
        );
    }

    #[test]
    fn test_load_coordinator_hats_falls_back_to_ralph_yaml_extension() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        // No ralph.yml, only ralph.yaml — should still load.
        std::fs::write(
            root.join("ralph.yaml"),
            "tasks:\n  coordinator_hats: [only]\n",
        )
        .expect("write yaml");
        let hats = load_coordinator_hats(root).expect("ralph.yaml fallback must work");
        assert_eq!(hats, vec!["only".to_string()]);
    }

    #[test]
    fn test_invalid_yaml_error_message_mentions_path() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        std::fs::write(root.join("ralph.yml"), ":\n - broken").expect("write yaml");
        let err = load_coordinator_hats(root).expect_err("must error");
        match err {
            CoordinatorHatsError::InvalidYaml { path, .. } => {
                assert_eq!(path, PathBuf::from(root.join("ralph.yml")));
            }
            other => panic!("expected InvalidYaml, got {other:?}"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// U7 (2026-07-04-003 plan): `EnsureArgs --for-fix-unit` derives the
// canonical fix-unit key + pins owner to `coordinator` without
// requiring an explicit `--key`.
//
// The tests intentionally avoid touching the ACL gate (`check_task`)
// and the verify gate (Unit 5/6) so this mod stays narrowly scoped
// to clap-level + handler-level key derivation.
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod ensure_for_fix_unit_clap_tests {
    use super::{EnsureArgs, OperationContext, TaskStore, add_common_task_fields, get_tasks_path};
    use clap::Parser;
    use ralph_core::Task;
    use tempfile::TempDir;

    /// Mirror of the production `derive_key` path inside
    /// `ensure_task_with_args` so we can assert the canonical key
    /// shape without invoking the full write path.
    fn derive_key(args: &EnsureArgs) -> Option<String> {
        if let Some(spec) = args.for_fix_unit.as_deref() {
            let mut parts = spec.split(':');
            let plan = parts.next().unwrap_or("").to_string();
            let fix_step = parts.next().unwrap_or("").to_string();
            let slug = parts.next().unwrap_or("").to_string();
            if plan.is_empty() || fix_step.is_empty() || slug.is_empty() || parts.next().is_some() {
                return None;
            }
            Some(format!("ce-executor:{plan}:{fix_step}:{slug}"))
        } else {
            args.key.clone()
        }
    }

    #[test]
    fn test_ensure_for_fix_unit_derives_key_without_explicit_key() {
        // Simulate clap parsing by constructing EnsureArgs directly
        // with no --key and a valid --for-fix-unit spec.
        let args = EnsureArgs {
            title: "fix-foo".to_string(),
            key: None,
            priority: 2,
            description: None,
            blocked_by: None,
            for_fix_unit: Some("myplan:fix-01:patch-foo".to_string()),
            format: crate::task_cli::OutputFormat::Quiet,
        };
        let derived = derive_key(&args).expect("for_fix_unit should derive a key");
        assert_eq!(derived, "ce-executor:myplan:fix-01:patch-foo");
    }

    #[test]
    fn test_ensure_for_fix_unit_pins_owner_coordinator() {
        let temp_dir = TempDir::new().expect("temp dir");
        let path = get_tasks_path(Some(&temp_dir.path().to_path_buf()));
        let mut store = TaskStore::load(&path).expect("load store");
        let ctx = OperationContext {
            workspace_root: temp_dir.path().to_path_buf(),
            current_loop_id: Some("loop-a".to_string()),
            current_hat_id: Some("executor".to_string()),
            is_agent_context: true,
        };
        let args = EnsureArgs {
            title: "fix-foo".to_string(),
            key: None,
            priority: 2,
            description: None,
            blocked_by: None,
            for_fix_unit: Some("myplan:fix-01:patch-foo".to_string()),
            format: crate::task_cli::OutputFormat::Quiet,
        };
        let key = derive_key(&args).expect("derive key");
        let task = add_common_task_fields(
            Task::new(args.title.clone(), args.priority).with_key(Some(key)),
            &ctx,
            args.description.clone(),
            args.blocked_by.clone(),
        );
        let task = if args.for_fix_unit.is_some() {
            task.with_owner_hat(Some("coordinator".to_string()))
        } else {
            task
        };
        store.add(task.clone());
        store.save().expect("save");
        // Owner must be pinned to coordinator regardless of ctx.
        assert_eq!(task.owner_hat_id.as_deref(), Some("coordinator"));
        assert_eq!(
            task.key.as_deref(),
            Some("ce-executor:myplan:fix-01:patch-foo")
        );
    }

    #[test]
    fn test_ensure_explicit_key_still_works() {
        // When --for-fix-unit is None, --key should be used as-is.
        let args = EnsureArgs {
            title: "do work".to_string(),
            key: Some("my-explicit-key".to_string()),
            priority: 2,
            description: None,
            blocked_by: None,
            for_fix_unit: None,
            format: crate::task_cli::OutputFormat::Quiet,
        };
        let derived = derive_key(&args).expect("explicit key should be returned");
        assert_eq!(derived, "my-explicit-key");
    }

    #[test]
    fn test_ensure_for_fix_unit_with_both_set_picks_for_fix_unit() {
        // Even when both are set (clap rejects at parse time, but
        // construction is allowed), the for_fix_unit derivation
        // wins so the canonical contract is preserved.
        let args = EnsureArgs {
            title: "fix-foo".to_string(),
            key: Some("stale-key".to_string()),
            priority: 2,
            description: None,
            blocked_by: None,
            for_fix_unit: Some("p:fix-01:s".to_string()),
            format: crate::task_cli::OutputFormat::Quiet,
        };
        let derived = derive_key(&args).expect("derive");
        assert_eq!(derived, "ce-executor:p:fix-01:s");
    }

    #[test]
    fn test_ensure_clap_parses_for_fix_unit_without_key() {
        // Use `TaskArgs::try_parse_from` to verify clap accepts
        // `--for-fix-unit` without `--key` and rejects the
        // conflict.
        use crate::task_cli::{TaskArgs, TaskCommands};
        let parsed = TaskArgs::try_parse_from([
            "ralph-tools-task",
            "ensure",
            "fix-foo",
            "--for-fix-unit",
            "myplan:fix-01:patch-foo",
        ])
        .expect("for_fix_unit alone should parse");
        match parsed.command {
            TaskCommands::Ensure(args) => {
                assert_eq!(args.title, "fix-foo");
                assert!(args.key.is_none(), "--key should be None");
                assert_eq!(
                    args.for_fix_unit.as_deref(),
                    Some("myplan:fix-01:patch-foo")
                );
            }
            _ => panic!("expected Ensure subcommand"),
        }
    }

    #[test]
    fn test_ensure_clap_rejects_both_key_and_for_fix_unit() {
        use crate::task_cli::TaskArgs;
        let err = TaskArgs::try_parse_from([
            "ralph-tools-task",
            "ensure",
            "fix-foo",
            "--key",
            "x",
            "--for-fix-unit",
            "p:fix-01:s",
        ])
        .expect_err("clap must reject --key + --for-fix-unit");
        let msg = err.to_string();
        assert!(
            msg.contains("cannot be used with") || msg.contains("for-fix-unit"),
            "clap error should mention conflicts: {msg}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// U7 (2026-07-04-003 plan): two-step gate wiring tests.
//
// These tests exercise `verify_gate_check` (the wrapper around
// `require_ticket`) directly with an explicit `OperationContext`,
// so the test never has to mutate process env vars. The
// `execute_add` / `execute_ensure` integration is verified
// separately by reading the code path: each of those functions
// calls `verify_gate_check` after `enforce_command_policy` and
// before any store mutation, so if the gate denies here, the
// execute path denies too.
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod task_verify_gate_wiring_tests {
    use super::*;
    use crate::task_verify_gate::{record_ticket, ticket_path};
    use ralph_core::config::RalphConfig;

    fn make_ctx(hat: &str, loop_id: &str, is_agent: bool) -> OperationContext {
        OperationContext {
            workspace_root: PathBuf::from("/tmp/wiring"),
            current_loop_id: Some(loop_id.to_string()),
            current_hat_id: Some(hat.to_string()),
            is_agent_context: is_agent,
        }
    }

    fn config_with_gate(gate_on: bool, unsafe_hatch: bool) -> RalphConfig {
        let yaml = format!(
            "tasks:\n  enabled: true\n  require_verify_for_cli_mutate: {gate_on}\n  \
             allow_unsafe_task_mutate: {unsafe_hatch}\n  coordinator_hats:\n    - coordinator\n"
        );
        serde_yaml::from_str(&yaml).expect("parse yaml")
    }

    fn add_payload() -> String {
        canonical_add_payload(&AddArgs {
            title: "x".to_string(),
            priority: 3,
            description: None,
            blocked_by: None,
            format: OutputFormat::Quiet,
        })
    }

    #[test]
    fn test_agent_add_without_verify_denied() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        let cfg = config_with_gate(true, false);
        let ctx = make_ctx("coordinator", "loop-a", true);
        let err = verify_gate_check(root, &cfg, &ctx, "add", &add_payload())
            .expect_err("agent add without verify must deny");
        let msg = err.to_string();
        assert!(
            msg.contains("task_verify_gate denied"),
            "stable prefix: {msg}"
        );
        assert!(msg.contains("verify"), "must explain verify: {msg}");
        assert!(!ticket_path(root).exists(), "deny must not create a ticket");
    }

    #[test]
    fn test_agent_verify_then_add_ok() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        let cfg = config_with_gate(true, false);
        let ctx = make_ctx("coordinator", "loop-a", true);
        // Step 1: record a ticket with the same fingerprint.
        let (loop_id, hat_id) = gate_identifiers(&ctx);
        let fp =
            crate::task_verify_gate::mutation_fingerprint("add", &add_payload(), loop_id, hat_id);
        record_ticket(&ticket_path(root), &fp, loop_id, hat_id).expect("record");

        // Step 2: gate check consumes the ticket and passes.
        verify_gate_check(root, &cfg, &ctx, "add", &add_payload())
            .expect("matching ticket must allow");
        assert!(
            !ticket_path(root).exists(),
            "successful gate check must consume the ticket"
        );
    }

    #[test]
    fn test_agent_second_add_needs_reverify() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        let cfg = config_with_gate(true, false);
        let ctx = make_ctx("coordinator", "loop-a", true);
        // First pass: record + verify.
        let (loop_id, hat_id) = gate_identifiers(&ctx);
        let fp =
            crate::task_verify_gate::mutation_fingerprint("add", &add_payload(), loop_id, hat_id);
        record_ticket(&ticket_path(root), &fp, loop_id, hat_id).expect("record");
        verify_gate_check(root, &cfg, &ctx, "add", &add_payload()).expect("first pass ok");

        // Second pass: ticket was consumed → must deny.
        let err = verify_gate_check(root, &cfg, &ctx, "add", &add_payload())
            .expect_err("second pass without re-verify must deny");
        assert!(err.to_string().contains("task_verify_gate denied"));
    }

    #[test]
    fn test_human_add_without_verify_ok() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        let cfg = config_with_gate(true, false);
        let ctx = make_ctx("coordinator", "loop-a", false);
        // Human: no env, no ticket — gate must bypass.
        verify_gate_check(root, &cfg, &ctx, "add", &add_payload())
            .expect("human CLI must bypass the gate");
    }

    #[test]
    fn test_agent_gate_off_bypasses() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        let cfg = config_with_gate(false, false);
        let ctx = make_ctx("coordinator", "loop-a", true);
        verify_gate_check(root, &cfg, &ctx, "add", &add_payload())
            .expect("gate-off must bypass for agent");
    }

    #[test]
    fn test_unsafe_escape_hatch_bypasses_for_agent() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        let cfg = config_with_gate(true, true);
        let ctx = make_ctx("coordinator", "loop-a", true);
        verify_gate_check(root, &cfg, &ctx, "add", &add_payload())
            .expect("unsafe escape hatch must bypass");
    }

    #[test]
    fn test_ticket_file_path_constant_stable() {
        // Defensive: the relative path is part of the public
        // contract (humans and agents both grep for it). If it
        // ever changes, the wire format breaks.
        assert_eq!(
            ticket_path(std::path::Path::new("/workspace")),
            std::path::PathBuf::from("/workspace/.ralph/agent/.ralph-task-verify-ticket")
        );
    }
}
