//! Shared operation context and authorization helpers.
//!
//! Avoids scattered, inconsistent authorization checks across
//! `task`, `memory`, `loops`, `skill`, and `emit` commands by
//! providing a single `OperationContext` that all later P2-P10
//! guard steps can build on.
//!
//! This module is intentionally policy-free: it surfaces the data
//! (current loop, current hat, agent vs human context) and a
//! couple of common error variants, but it does not yet enforce
//! anything. P2-P10 layers will decide what to do with the data.

#![allow(dead_code)] // Public API surface consumed by future P2-P10 layers.

use std::path::{Path, PathBuf};

/// Error variants for cross-loop / cross-hat access attempts.
///
/// These are surfaced by helper functions so individual commands
/// can return a uniform error type once P2-P10 are in place.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OperationGuardError {
    #[error("operation '{operation}' targets loop {target:?} but current loop is {current:?}")]
    CrossLoopDenied {
        operation: String,
        target: Option<String>,
        current: Option<String>,
    },

    #[error("operation '{operation}' targets hat {target:?} but current hat is {current:?}")]
    CrossHatDenied {
        operation: String,
        target: Option<String>,
        current: Option<String>,
    },

    #[error("agent context requires a current hat (set RALPH_CURRENT_HAT)")]
    AgentContextMissingHat,

    #[error("path '{path}' is outside the current loop's events directory '{allowed}'")]
    PathOutsideCurrentLoop { path: String, allowed: String },
}

/// Shared operation context for Ralph CLI commands.
///
/// Built once at command entry so downstream code can ask
/// "are we running in an agent context?" or "what loop is active?"
/// without re-implementing marker / env parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationContext {
    pub workspace_root: PathBuf,
    pub current_loop_id: Option<String>,
    pub current_hat_id: Option<String>,
    pub is_agent_context: bool,
}

impl OperationContext {
    /// Detect context from the real process environment.
    pub fn detect(workspace_root: PathBuf) -> Self {
        Self::detect_with_env(workspace_root, |key| std::env::var(key).ok())
    }

    /// Detect context with an injected env resolver.
    ///
    /// Tests use this to simulate the runtime env without mutating
    /// the process environment (which is `unsafe` in recent Rust).
    pub fn detect_with_env<F>(workspace_root: PathBuf, env_lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let current_loop_id = read_loop_id_marker(&workspace_root);
        let current_hat_id = read_current_hat(&env_lookup);
        let is_agent_context = compute_is_agent_context(&env_lookup);

        Self {
            workspace_root,
            current_loop_id,
            current_hat_id,
            is_agent_context,
        }
    }

    /// True if this command is running in an agent-owned context.
    pub fn is_agent(&self) -> bool {
        self.is_agent_context
    }

    /// Resolve the path stored in `.ralph/current-candidate-events`,
    /// if any. The marker value may be absolute or workspace-relative.
    pub fn resolve_candidate_events_path(&self) -> Option<PathBuf> {
        read_marker_target(&self.workspace_root, ".ralph/current-candidate-events")
    }

    /// Resolve the path stored in `.ralph/current-events`,
    /// if any. The marker value may be absolute or workspace-relative.
    pub fn resolve_accepted_events_path(&self) -> Option<PathBuf> {
        read_marker_target(&self.workspace_root, ".ralph/current-events")
    }

    /// Resolve the events file that an emit should target.
    ///
    /// Priority: candidate-events marker → current-events marker →
    /// `.ralph/events.jsonl`. The chosen path is always returned
    /// (never `None`) so callers can write to it directly.
    pub fn resolve_emit_events_path(&self) -> PathBuf {
        self.resolve_candidate_events_path()
            .or_else(|| self.resolve_accepted_events_path())
            .unwrap_or_else(|| self.workspace_root.join(".ralph/events.jsonl"))
    }
}

/// Read the loop ID stored in `.ralph/current-loop-id`.
///
/// Returns `None` if the marker is missing, unreadable, or
/// contains only whitespace. This matches the legacy behavior
/// of `task_cli::read_current_loop_id`.
pub fn read_loop_id_marker(workspace_root: &Path) -> Option<String> {
    let path = workspace_root.join(".ralph/current-loop-id");
    let raw = std::fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Read the current hat id from `RALPH_CURRENT_HAT`.
///
/// Empty strings are treated as missing so callers don't have
/// to special-case unset vs blank values.
pub fn read_current_hat<F>(env_lookup: &F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    env_lookup("RALPH_CURRENT_HAT")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Decide whether the current invocation is an agent-owned one.
///
/// True if any of the runtime env vars the loop runner injects
/// for hat-owned execution is present and non-empty.
///
/// The actual env var name for the current loop is
/// `RALPH_CURRENT_LOOP_ID` (see `loop_runner::inject_hat_execution_env`).
/// The plan lists `RALPH_LOOP_ID` as an example, but the real
/// code uses `RALPH_CURRENT_LOOP_ID`; we honor the code.
fn compute_is_agent_context<F>(env_lookup: &F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    const AGENT_ENV_KEYS: &[&str] = &[
        "RALPH_CURRENT_HAT",
        "RALPH_CURRENT_LOOP_ID",
        "RALPH_EVENTS_FILE",
        "RALPH_WAVE_WORKER",
    ];

    AGENT_ENV_KEYS
        .iter()
        .any(|key| env_lookup(key).is_some_and(|v| !v.trim().is_empty()))
}

fn read_marker_target(workspace_root: &Path, marker_relpath: &str) -> Option<PathBuf> {
    let marker = workspace_root.join(marker_relpath);
    let raw = std::fs::read_to_string(&marker).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(trimmed);
    Some(resolve_marker_path(workspace_root, candidate))
}

/// Resolve a marker-stored path against the workspace root if
/// the stored value is relative. Mirrors `main::resolve_marker_target`.
fn resolve_marker_path(workspace_root: &Path, value: PathBuf) -> PathBuf {
    if value.is_absolute() {
        value
    } else {
        workspace_root.join(value)
    }
}

/// True when a destructive command should fail closed (reject the
/// operation outright) rather than ask for confirmation.
///
/// In agent context the loop runner expects structured, silent
/// failures — humans are not in the loop. In human CLI context
/// we can still confirm interactively, so we don't fail closed.
pub fn should_fail_closed(ctx: &OperationContext) -> bool {
    ctx.is_agent_context
}

/// True when an interactive human CLI should require explicit
/// confirmation for a destructive action. Agent contexts never
/// require confirmation: they fail closed instead.
pub fn requires_human_confirmation(ctx: &OperationContext) -> bool {
    !ctx.is_agent_context
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Empty env resolver — simulates a human CLI invocation.
    fn empty_env() -> impl Fn(&str) -> Option<String> {
        |_| None
    }

    /// Single-key env resolver — simulates a specific runtime var.
    fn env_with(key: &'static str, value: &'static str) -> impl Fn(&str) -> Option<String> {
        move |k| {
            if k == key {
                Some(value.to_string())
            } else {
                None
            }
        }
    }

    #[test]
    fn test_operation_context_reads_current_loop_id() {
        let tmp = TempDir::new().expect("temp dir");
        let ralph_dir = tmp.path().join(".ralph");
        fs::create_dir_all(&ralph_dir).expect("ralph dir");
        fs::write(ralph_dir.join("current-loop-id"), "loop-abc").expect("write marker");

        let ctx = OperationContext::detect_with_env(tmp.path().to_path_buf(), empty_env());

        assert_eq!(ctx.current_loop_id.as_deref(), Some("loop-abc"));
    }

    #[test]
    fn test_operation_context_empty_loop_marker_is_none() {
        let tmp = TempDir::new().expect("temp dir");
        let ralph_dir = tmp.path().join(".ralph");
        fs::create_dir_all(&ralph_dir).expect("ralph dir");
        fs::write(ralph_dir.join("current-loop-id"), "   \n").expect("write marker");

        let ctx = OperationContext::detect_with_env(tmp.path().to_path_buf(), empty_env());

        assert_eq!(ctx.current_loop_id, None);
    }

    #[test]
    fn test_operation_context_agent_when_current_hat_set() {
        let tmp = TempDir::new().expect("temp dir");

        let ctx = OperationContext::detect_with_env(
            tmp.path().to_path_buf(),
            env_with("RALPH_CURRENT_HAT", "executor"),
        );

        assert!(ctx.is_agent_context);
        assert_eq!(ctx.current_hat_id.as_deref(), Some("executor"));
    }

    #[test]
    fn test_operation_context_human_when_no_runtime_env() {
        let tmp = TempDir::new().expect("temp dir");

        let ctx = OperationContext::detect_with_env(tmp.path().to_path_buf(), empty_env());

        assert!(!ctx.is_agent_context);
        assert_eq!(ctx.current_hat_id, None);
    }

    #[test]
    fn test_operation_context_wave_worker_is_agent() {
        let tmp = TempDir::new().expect("temp dir");

        let ctx = OperationContext::detect_with_env(
            tmp.path().to_path_buf(),
            env_with("RALPH_WAVE_WORKER", "1"),
        );

        assert!(ctx.is_agent_context);
    }

    #[test]
    fn test_operation_context_resolves_candidate_events_marker() {
        let tmp = TempDir::new().expect("temp dir");
        let ralph_dir = tmp.path().join(".ralph");
        fs::create_dir_all(&ralph_dir).expect("ralph dir");
        fs::write(
            ralph_dir.join("current-candidate-events"),
            ".ralph/event-candidates-20260601.jsonl",
        )
        .expect("write marker");

        let ctx = OperationContext::detect_with_env(tmp.path().to_path_buf(), empty_env());

        let resolved = ctx
            .resolve_candidate_events_path()
            .expect("candidate marker present");
        assert_eq!(
            resolved,
            tmp.path().join(".ralph/event-candidates-20260601.jsonl")
        );
    }

    #[test]
    fn test_operation_context_resolves_accepted_events_marker() {
        let tmp = TempDir::new().expect("temp dir");
        let ralph_dir = tmp.path().join(".ralph");
        fs::create_dir_all(&ralph_dir).expect("ralph dir");
        fs::write(
            ralph_dir.join("current-events"),
            ".ralph/events-20260601.jsonl",
        )
        .expect("write marker");

        let ctx = OperationContext::detect_with_env(tmp.path().to_path_buf(), empty_env());

        let resolved = ctx
            .resolve_accepted_events_path()
            .expect("accepted marker present");
        assert_eq!(resolved, tmp.path().join(".ralph/events-20260601.jsonl"));
    }

    #[test]
    fn test_operation_context_missing_markers_defaults_events_jsonl() {
        let tmp = TempDir::new().expect("temp dir");

        let ctx = OperationContext::detect_with_env(tmp.path().to_path_buf(), empty_env());

        assert_eq!(
            ctx.resolve_emit_events_path(),
            tmp.path().join(".ralph/events.jsonl")
        );
    }
}
