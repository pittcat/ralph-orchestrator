//! Core orchestration loop implementation.
//!
//! This module contains the main `run_loop_impl` function that executes
//! the Ralph orchestration loop, along with supporting types and helper
//! functions for PTY execution and termination handling.
//!
//! Per plan `2026-08-07-004`, the `runner` submodule was split into
//! `entry` (entry-point wrapper), `run_impl` (supervisor bridge),
//! `inner` (`run_loop_impl_inner` body), `sync_timeout` (startup
//! timeout helpers), and `sync_timeout_tests` (`#[cfg(test)]`
//! timeout/lint tests). `mod.rs` itself remains the integration
//! surface: all of the original `pub use runner::*` re-exports now
//! point at the leaf modules.

mod event_logging;
mod execution;
mod exit_conditions;
mod hard_gate;
mod hat_channel;
mod hooks;
mod late_events;
mod loop_owner;
mod merge_queue;
mod notifications;
mod output_parsing;
mod paths;
mod payload_contract_gate;
mod payload_inputs;
mod preset_lint_gate;
mod prompt;
// Plan 2026-08-15-1823 (fix empty channel activation observability)
// Unit 2: bounded activation outcome rows in the runtime trace
// sidecar. Module is purely additive — observation only, no effect
// on loop / recovery / retry decisions. Items are re-exported so
// sibling modules (`inner.rs` / `entry.rs`) can call
// `activation_outcome::xxx` without re-importing each name.
mod activation_outcome;
// Plan 2026-08-15-1823 (fix empty channel activation observability)
// Unit 1: extract the `if isolated_mode` block from `inner.rs`
// into a dedicated sibling module so `inner.rs` stays at the
// HARD RULE 5000-line ceiling. The interrupt path lives in
// `entry.rs::merge_isolated_channel_on_interrupt` and does not
// depend on this module.
mod activation_outcome_close;
#[allow(unused_imports)]
pub(crate) use activation_outcome::ActivationOutcomeStatus;
#[allow(unused_imports)]
pub(crate) use activation_outcome::{
    ActivationOutcomeFacts, ChannelSnapshot, refine_after_merge, refine_for_interrupt,
};
// Plan 2026-08-07-004: `loop_runner::runner` was split into five
// sibling modules. The five `mod` declarations below are what makes
// them part of the `loop_runner` namespace; `runner.rs` itself only
// owns the `RpcSharedState` / `resolve_loop_id` helpers and the
// compatibility `pub use` re-exports that keep external callers
// (`commands/run.rs`, `commands/resume.rs`, `loop_runner/tests/*`)
// working unchanged.
mod entry;
mod inner;
mod run_impl;
mod runner;
mod rpc_bootstrap;
mod suspend;
mod sync_timeout;
mod termination_diagnostics_support;
#[cfg(test)]
mod sync_timeout_tests;
pub mod wave;

#[cfg(test)]
pub(crate) use execution::ExecutionOutcome;
pub(crate) use loop_owner::register_loop_owner;
#[cfg(test)]
pub(crate) use loop_owner::register_loop_owner_with_hat;
pub use merge_queue::process_pending_merges_cli;
pub use payload_contract_gate::{
    enforce_payload_contract_gate, write_payload_contract_violation_report,
};
// 2026-07-16 cleanup U4 (KTD-3): `enforce_preset_lint_gate` (2-arg
// variant) is a reserved public API. The 3-arg `*_with_preset_name`
// sibling is what the runner actually calls; the 2-arg helper stays
// exported so preset authors / external callers can pin the simpler
// signature without churn.
#[allow(unused_imports)]
pub use preset_lint_gate::enforce_preset_lint_gate;
pub use preset_lint_gate::{
    EXIT_CODE_AGENT_DOC_SYNC_STRICT, EXIT_CODE_LINT_GATE, PresetLintGateError,
    enforce_preset_lint_gate_with_preset_name, write_preset_lint_artifact,
};
#[cfg(test)]
pub use runner::resolve_loop_id;
pub use runner::run_loop_impl;
// Compatibility shims for callers that reached the original
// `crate::loop_runner::runner::*` paths. After plan 2026-08-07-004
// the items live in dedicated sibling modules (`entry`, `inner`,
// `run_impl`, `sync_timeout`); re-export them here so external
// callers (`loop_runner/tests/*`, `commands/run.rs`,
// `commands/resume.rs`) keep compiling without changes.
#[allow(unused_imports)]
pub(crate) use entry::persist_starting_event_to_events_file;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use inner::build_termination_diagnostics;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use inner::write_termination_diagnostics;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use run_impl::bridge_build_invocations;
#[allow(unused_imports)]
pub(crate) use run_impl::build_supervisor_bridge;
#[cfg(all(test, feature = "supervisor-db"))]
#[allow(unused_imports)]
pub(crate) use run_impl::clear_factory_override_for_test;
#[cfg(all(test, feature = "supervisor-db"))]
#[allow(unused_imports)]
pub(crate) use run_impl::install_factory_override_for_test;
#[allow(unused_imports)]
pub(crate) use sync_timeout::adapter_timeout_duration;
// Re-export all other module items for internal use and test access
pub use event_logging::*;
pub use execution::*;
pub use exit_conditions::*;
pub use hard_gate::*;
// DEC-001:`hooks::termination::dispatch_pre/post_loop_termination_hooks` 改为
// `pub(super) fn`,让 `pub use hooks::*;` glob 不再 re-export 它们到 `loop_runner::*`。
// 这样 `loop_runner::tests::legacy` `use super::super::*;` 不引入同名 fn,避免与
// `tests/common.rs` 内的同名包装 fn 歧义。runner.rs 改用 `hooks::termination::dispatch_*`
// 显式 path 访问。`hooks::termination` 命名空间内 `pub(super)` 仍可见
// (sibling 子模块 / 子 fn 可调)。
pub use hooks::*;
pub use late_events::*;
pub use merge_queue::*;
pub use output_parsing::*;
pub use paths::*;
// DEC-001/DEC-002:`pub use payload_inputs::*;` glob 暴露会让 `loop_runner::tests::legacy`
// `use super::super::*;` 引入与 `tests/common.rs` 同名包装 fn 歧义(E0659)。runner.rs 内
// 全部 `build_*_payload_input` 调用已改用 `payload_inputs::*` 显式 path(commit `c64882f6`),
// 无需 re-export。这里**不**做 `pub use payload_inputs::*;` glob,保持外部访问唯一路径:
//   - `loop_runner::payload_inputs::build_*_payload_input` (供 tests/common.rs / runner.rs)
//   - 不暴露 `loop_runner::build_*_payload_input` 短名(避免 E0659)
//
// 注:`dispatch_pre/post_loop_termination_hooks` 通过 `pub use hooks::*;` 在
// `loop_runner::*` glob 暴露,本身不在 `payload_inputs::*` 内。runner.rs 内调用
// 不需要处理(由 `pub use hooks::*;` 继续生效)。
pub use prompt::*;
pub use suspend::*;
pub use wave::*;

#[allow(unused_imports)]
use anyhow::{Context, Result, bail};
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
#[allow(unused_imports)]
use ralph_adapters::{
    ClaudeStreamEvent, ClaudeStreamParser, CliBackend, CliExecutor, ConsoleStreamHandler,
    ContentBlock, ExecutionResult, JsonRpcStreamHandler, OutputFormat as BackendOutputFormat,
    PiAssistantEvent, PiStreamEvent, PiStreamParser, PrettyStreamHandler, PtyConfig,
    PtyExecutionResult, PtyExecutor, QuietStreamHandler, TuiStreamHandler,
};
#[allow(unused_imports)]
use ralph_core::diagnostics::{HookDisposition, HookRunTelemetryEntry};
#[allow(unused_imports)]
use ralph_core::payload_contract::validate_payload_contract;
#[allow(unused_imports)]
use ralph_core::{
    CompletionAction, EventLogger, EventLoop, EventParser, EventRecord, HatConfig, HookEngine,
    HookExecutor, HookExecutorContract, HookMutationConfig, HookOnError, HookPayloadBuilderInput,
    HookPayloadContextInput, HookPhaseEvent, HookRunRequest, HookRunResult, HookSuspendMode,
    LoopCompletionHandler, LoopContext, LoopHistory, LoopRegistry, MergeQueue, Phase, PhaseConfig,
    RalphConfig, Record, SessionRecorder, SummaryWriter, SuspendStateRecord, SuspendStateStore,
    TerminationReason, UrgentSteerStore, WarmupConfig,
};
#[allow(unused_imports)]
use ralph_proto::{Event, GuidanceTarget, HatId, RpcEvent, RpcState, RpcTaskCounts};
#[allow(unused_imports)]
use ralph_tui::Tui;
#[allow(unused_imports)]
use std::ffi::OsStr;
#[allow(unused_imports)]
use std::fs::{self, File};
#[allow(unused_imports)]
use std::io::{BufWriter, IsTerminal, stdin, stdout};
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use std::process::{Command, Stdio};
#[allow(unused_imports)]
use std::sync::Arc;
#[allow(unused_imports)]
use std::time::Duration;
#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

#[allow(unused_imports)]
use crate::cli::process_management;
#[allow(unused_imports)]
use crate::cli::{ColorMode, Verbosity};
#[allow(unused_imports)]
use crate::display::{
    build_tui_hat_map, print_iteration_footer, print_iteration_separator, print_loop_banner,
    print_termination,
};
#[allow(unused_imports)]
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
