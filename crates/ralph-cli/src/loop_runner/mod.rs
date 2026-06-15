//! Core orchestration loop implementation.
//!
//! This module contains the main `run_loop_impl` function that executes
//! the Ralph orchestration loop, along with supporting types and helper
//! functions for PTY execution and termination handling.

mod event_logging;
mod execution;
mod exit_conditions;
mod hard_gate;
mod hat_channel;
mod hooks;
mod late_events;
mod loop_owner;
mod merge_queue;
mod output_parsing;
mod paths;
mod payload_contract_gate;
mod payload_inputs;
mod preset_lint_gate;
mod prompt;
mod runner;
mod start_loop;
mod suspend;
pub mod wave;

pub(crate) use execution::ExecutionOutcome;
pub(crate) use loop_owner::register_loop_owner;
#[cfg(test)]
pub(crate) use loop_owner::register_loop_owner_with_hat;
pub use merge_queue::process_pending_merges_cli;
pub use payload_contract_gate::{
    enforce_payload_contract_gate, write_payload_contract_violation_report,
};
pub use preset_lint_gate::{
    EXIT_CODE_AGENT_DOC_SYNC_STRICT, EXIT_CODE_LINT_GATE, PresetLintGateError,
    enforce_preset_lint_gate, write_preset_lint_artifact,
};
#[cfg(test)]
pub use runner::resolve_loop_id;
pub use runner::run_loop_impl;
#[cfg(test)]
pub(crate) use runner::{build_termination_diagnostics, write_termination_diagnostics};
pub use start_loop::start_loop;

// Re-export all other module items for internal use and test access
pub use event_logging::*;
pub use execution::*;
pub use exit_conditions::*;
pub use hard_gate::*;
pub use hooks::*;
pub use late_events::*;
pub use merge_queue::*;
pub use output_parsing::*;
pub use paths::*;
pub use payload_inputs::*;
pub use prompt::*;
pub use start_loop::*;
pub use suspend::*;
pub use wave::*;

use anyhow::{Context, Result, bail};
use ralph_core::payload_contract::validate_payload_contract;

/// Payload contract hard gate (U5).
///
/// `ralph run` MUST call this BEFORE spawning any backend. In strict mode
/// (always on for `ralph run`), any payload contract error is fatal:
/// - the backend must NOT be spawned
/// - the orchestrator must exit with a non-zero status
/// - the error message must be actionable (hat id, topic, field, schema source)
///
/// There is no skip flag for this gate. Plan non-regression: payload contract
/// gate is required and cannot be bypassed.
use ralph_adapters::{
    AcpExecutor, ClaudeStreamEvent, ClaudeStreamParser, CliBackend, CliExecutor,
    ConsoleStreamHandler, ContentBlock, CopilotStreamParser, JsonRpcStreamHandler,
    OutputFormat as BackendOutputFormat, PiAssistantEvent, PiStreamEvent, PiStreamParser,
    PrettyStreamHandler, PtyConfig, PtyExecutor, QuietStreamHandler, TuiStreamHandler,
};
use ralph_core::diagnostics::{HookDisposition, HookRunTelemetryEntry};
use ralph_core::{
    CompletionAction, EventLogger, EventLoop, EventParser, EventRecord, HatConfig, HookEngine,
    HookExecutor, HookExecutorContract, HookMutationConfig, HookOnError, HookPayloadBuilderInput,
    HookPayloadContextInput, HookPhaseEvent, HookRunRequest, HookRunResult, HookSuspendMode,
    LoopCompletionHandler, LoopContext, LoopHistory, LoopRegistry, MergeQueue, Phase, PhaseConfig,
    RalphConfig, Record, SessionRecorder, SummaryWriter, SuspendStateRecord, SuspendStateStore,
    TerminationReason, UrgentSteerStore, WarmupConfig,
};
use ralph_proto::{Event, GuidanceTarget, HatId, RpcEvent, RpcState, RpcTaskCounts};
use ralph_tui::Tui;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufWriter, IsTerminal, stdin, stdout};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::cli::process_management;
use crate::cli::{ColorMode, Verbosity};
use crate::display::{
    build_tui_hat_map, print_iteration_footer, print_iteration_separator, print_loop_banner,
    print_termination,
};
use crate::rpc_stdin::{GuidanceMessage, RpcDispatcher, run_stdin_reader, run_stdout_emitter};

/// Shared atomic state written by the main loop and read by the RPC `get_state` handler.

/// Determine whether the active hat requires an explicit emit and has no
/// default_publishes fallback. Only hats that *should* publish but have no
/// automatic兜底 are hard-gated.

/// Resolves the active timestamped events JSONL file path for this run.
///
/// The authoritative source is `.ralph/current-events`, which contains a
/// relative path like `.ralph/events-YYYYMMDD-HHMMSS.jsonl`.
///
/// Falls back to `ctx.events_path()` if the marker is missing/unreadable.

/// R3: Register the current loop in the [`LoopRegistry`] with the
/// appropriate `owner_hat_id`. In `--resume` mode the existing entry is
/// left in place — re-registering would clobber the worktree path and
/// PID the merge queue still references.
///
/// Agent-owned loops (env has `RALPH_CURRENT_HAT`) stamp the hat id on
/// the entry so the P7 authorization helpers can gate cross-loop
/// operations. Human CLI invocations stay `None` so any operator can
/// still attach, view logs, or merge.

#[cfg(test)]
mod tests;
