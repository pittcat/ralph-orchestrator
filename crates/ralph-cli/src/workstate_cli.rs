//! CLI commands for the `ralph tools workstate` namespace.
//!
//! Workstate is a loop-scoped key-value store for intermediate state that
//! belongs to the current loop (unlike memories, which are global and
//! survive across loops). Scoping rules:
//! - Agent context (runtime env present): operations are scoped to the
//!   current loop id; an agent context without a loop id is rejected.
//! - Human CLI (no runtime env): operations use the loop-less scope,
//!   invisible to any loop.

use crate::operation_guard::OperationContext;
use crate::resolve_workspace_root;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ralph_core::WorkstateStore;
use std::path::PathBuf;

/// Workstate management commands for loop-scoped intermediate state.
#[derive(Parser, Debug)]
pub struct WorkstateArgs {
    #[command(subcommand)]
    pub command: WorkstateCommands,

    /// Select a loop scope for human operator commands (get the id from
    /// `ralph inspect loop --format json`). Agent context uses its
    /// runtime-injected loop id and cannot override it.
    #[arg(long, global = true)]
    pub loop_id: Option<String>,

    /// Working directory (default: current directory)
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum WorkstateCommands {
    /// Set (upsert) a workstate key for the current loop
    Set(SetArgs),

    /// Get a workstate value by key
    Get(GetArgs),

    /// List all workstate keys of the current loop
    List(ListArgs),

    /// Delete a workstate key
    Delete(DeleteArgs),
}

/// Arguments for the `workstate set` command.
#[derive(Parser, Debug)]
pub struct SetArgs {
    /// The key to set (no whitespace or control characters)
    pub key: String,

    /// The value to store (at most 10000 characters)
    pub value: String,
}

/// Arguments for the `workstate get` command.
#[derive(Parser, Debug)]
pub struct GetArgs {
    /// The key to read
    pub key: String,
}

/// Arguments for the `workstate list` command.
#[derive(Parser, Debug)]
pub struct ListArgs {}

/// Arguments for the `workstate delete` command.
#[derive(Parser, Debug)]
pub struct DeleteArgs {
    /// The key to delete
    pub key: String,
}

/// Execute a workstate command.
pub fn execute(args: WorkstateArgs) -> Result<()> {
    let root = resolve_workspace_root(args.root.as_ref());
    let store = WorkstateStore::with_default_path(&root);
    let ctx = OperationContext::detect(root);
    let scope = workstate_scope(&ctx, args.loop_id.as_deref())?;

    match args.command {
        WorkstateCommands::Set(set_args) => {
            store
                .set(
                    scope.loop_id.as_deref(),
                    &set_args.key,
                    &set_args.value,
                    scope.hat_id.as_deref(),
                )
                .context("Failed to set workstate")?;
        }
        WorkstateCommands::Get(get_args) => {
            let entry = store
                .get(scope.loop_id.as_deref(), &get_args.key)
                .context("Failed to read workstate")?;
            let Some(entry) = entry else {
                bail!("workstate key not found: {}", get_args.key);
            };
            println!("{}", entry.value);
        }
        WorkstateCommands::List(_) => {
            let entries = store
                .list(scope.loop_id.as_deref())
                .context("Failed to list workstate")?;
            for entry in entries {
                println!("{}\t{}", entry.key, entry.updated_at_ms);
            }
        }
        WorkstateCommands::Delete(delete_args) => {
            store
                .delete(
                    scope.loop_id.as_deref(),
                    &delete_args.key,
                    scope.hat_id.as_deref(),
                )
                .context("Failed to delete workstate")?;
        }
    }
    Ok(())
}

/// The `(loop_id, hat_id)` scope a workstate command operates on.
#[derive(Debug)]
struct WorkstateScope {
    loop_id: Option<String>,
    hat_id: Option<String>,
}

/// Resolve the scope from the operation context.
///
/// Agent context must carry a loop id (fail closed otherwise); the human
/// CLI falls back to the loop-less scope.
fn workstate_scope(
    ctx: &OperationContext,
    requested_loop_id: Option<&str>,
) -> Result<WorkstateScope> {
    if ctx.is_agent_context {
        if requested_loop_id.is_some() {
            bail!("workstate: agent context cannot select a loop id");
        }
        let loop_id = ctx.current_loop_id.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "workstate: agent context requires a current loop id (set RALPH_CURRENT_LOOP_ID)"
            )
        })?;
        Ok(WorkstateScope {
            loop_id: Some(loop_id),
            hat_id: ctx.current_hat_id.clone(),
        })
    } else {
        Ok(WorkstateScope {
            loop_id: requested_loop_id.map(str::to_string),
            hat_id: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_for(
        workspace: &std::path::Path,
        hat: Option<&str>,
        loop_id: Option<&str>,
    ) -> OperationContext {
        OperationContext::detect_with_env(workspace.to_path_buf(), move |key| match key {
            "RALPH_CURRENT_HAT" => hat.map(String::from),
            "RALPH_CURRENT_LOOP_ID" => loop_id.map(String::from),
            _ => None,
        })
    }

    #[test]
    fn scope_agent_uses_current_loop() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let ctx = ctx_for(tmp.path(), Some("executor"), Some("loop-1"));
        let scope = workstate_scope(&ctx, None).expect("scope");
        assert_eq!(scope.loop_id.as_deref(), Some("loop-1"));
        assert_eq!(scope.hat_id.as_deref(), Some("executor"));
    }

    #[test]
    fn scope_agent_without_loop_id_fails_closed() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let ctx = ctx_for(tmp.path(), Some("executor"), None);
        let err = workstate_scope(&ctx, None).expect_err("must fail without loop id");
        assert!(err.to_string().contains("loop id"));
    }

    #[test]
    fn scope_human_uses_loop_less_scope() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let ctx = ctx_for(tmp.path(), None, None);
        assert!(!ctx.is_agent_context);
        let scope = workstate_scope(&ctx, None).expect("scope");
        assert_eq!(scope.loop_id, None);
        assert_eq!(scope.hat_id, None);
    }

    #[test]
    fn scope_human_can_select_loop() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let ctx = ctx_for(tmp.path(), None, None);
        let scope = workstate_scope(&ctx, Some("loop-1")).expect("operator loop scope");
        assert_eq!(scope.loop_id.as_deref(), Some("loop-1"));
    }

    #[test]
    fn scope_agent_cannot_override_loop_id() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let ctx = ctx_for(tmp.path(), Some("executor"), Some("loop-1"));
        let err = workstate_scope(&ctx, Some("loop-2")).expect_err("agent cannot select loop");
        assert!(err.to_string().contains("cannot select a loop id"));
    }
}
