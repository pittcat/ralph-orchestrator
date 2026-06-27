//! Diagnostic logging system for Ralph orchestration.
//!
//! Captures agent output, orchestration decisions, traces, performance metrics,
//! and errors to structured JSONL files when `RALPH_DIAGNOSTICS=1` is set.
//!
//! # Activation Matrix (U0 contract)
//!
//! The collector is driven by [`DiagnosticsOptions`]. Exactly one of three modes
//! is active for a given collector:
//!
//! | `full_diagnostics` | `runtime_diagnosis_artifacts` | `session_dir` | Behavior |
//! |---|---|---|---|
//! | `false` | `false` | `None` (default) | Disabled. No I/O. `is_enabled()` is false. |
//! | `true`  | any     | `None`             | Full session. Creates `<base>/.ralph/diagnostics/<timestamp>/` and all existing loggers (orchestration, performance, errors, hook-runs, agent-output, prompt-log). U3 also wires `recovery.jsonl` / `drift.jsonl` / `diagnosis-summary.json`. |
//! | `false` | `true`  | `None`             | Minimal diagnosis session. Creates the timestamped directory but does NOT instantiate any of the historical full-diagnostics loggers. U3 adds `recovery.jsonl` / `drift.jsonl`; `diagnosis-summary.json` is written on demand via [`DiagnosticsCollector::write_diagnosis_summary_seed`]. |
//! | `true`  | any     | `Some(p)`          | Full session reusing the provided path. No new dir is created. |
//! | `false` | `true`  | `Some(p)`          | Minimal diagnosis session reusing the provided path. |
//!
//! The CLI is responsible for building **one** authoritative collector per
//! `ralph run` and threading it through the tracing layer, the loop runner
//! and `EventLoop`. Multiple collectors would create competing timestamp
//! directories, so this is enforced by convention plus this central type.

mod agent_output;
mod drift;
mod errors;
mod hook_runs;
mod log_rotation;
mod orchestration;
mod performance;
mod recovery;
mod stream_handler;
mod trace_layer;

#[cfg(test)]
mod integration_tests;

pub use agent_output::{AgentOutputContent, AgentOutputEntry, AgentOutputLogger};
pub use drift::{DriftLogger, MAX_DRIFT_MESSAGE_CHARS};
pub use errors::{DiagnosticError, ErrorLogger};
pub use hook_runs::{HookDisposition, HookRunLogger, HookRunTelemetryEntry};
pub use log_rotation::{create_log_file, rotate_logs};
pub use orchestration::{
    OrchestrationContext, OrchestrationEntry, OrchestrationEvent, OrchestrationLogger,
};
pub use performance::{PerformanceLogger, PerformanceMetric};
pub use recovery::{MAX_RECOVERY_NOTE_CHARS, RecoveryLogger};
pub use stream_handler::DiagnosticStreamHandler;
pub use trace_layer::{DiagnosticTraceLayer, TraceEntry};
// `DiagnosisSummary` is declared at module root below, so callers can
// refer to it as `crate::diagnostics::DiagnosisSummary` without a
// separate re-export.

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::NamedTempFile;

/// Activation matrix for a [`DiagnosticsCollector`].
///
/// This struct is the single source of truth for whether diagnostics are
/// captured during a run. U1 (`telemetry.runtime_diagnosis` config) will
/// populate this from YAML; for U0 the CLI populates `full_diagnostics`
/// from `RALPH_DIAGNOSTICS=1` and leaves `runtime_diagnosis_artifacts`
/// at its default `false`. U3 will read the same struct to decide which
/// minimal loggers to spin up.
///
/// `session_dir` is set by the CLI when an upstream component (typically
/// the tracing-layer setup in `main.rs`) has already created the timestamped
/// directory and we want the `EventLoop` to write to the same dir instead
/// of generating a second one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticsOptions {
    /// `RALPH_DIAGNOSTICS=1` enables the historical full diagnostic set:
    /// orchestration, performance, errors, hook-runs, agent-output, prompt-log.
    pub full_diagnostics: bool,

    /// `telemetry.runtime_diagnosis.write_artifacts=true` enables a minimal
    /// diagnosis session (timestamped dir only; U3 adds recovery/drift/summary
    /// loggers). Ignored when `full_diagnostics` is already true, since
    /// full diagnostics subsumes it.
    pub runtime_diagnosis_artifacts: bool,

    /// Reuse an existing session directory instead of creating a new one.
    /// Used by `main.rs` to share the dir between the tracing layer and the
    /// `EventLoop`. When `None`, a new timestamped dir is created lazily.
    pub session_dir: Option<PathBuf>,

    /// `trace_only=true` makes the collector create the session dir for the
    /// tracing layer and TUI stderr log, but skip ALL loop-level loggers
    /// (recovery/drift/orchestration/performance/errors/hook-runs/agent-output/
    /// prompt-log). Used by the subprocess TUI parent in `main.rs` so it
    /// does not leave an empty shell in the main repo while the child RPC
    /// process writes real data into the worktree (U1, 2026-06-14).
    ///
    /// `full_diagnostics=true` wins: when both are set, the full logger set
    /// is created (matches the existing
    /// `runtime_diagnosis_artifacts`-vs-full precedence contract).
    pub trace_only: bool,
}

impl DiagnosticsOptions {
    /// Returns true when any diagnostic capture is active.
    pub fn is_enabled(&self) -> bool {
        self.full_diagnostics || self.runtime_diagnosis_artifacts || self.trace_only
    }

    /// Returns true when the trace-only mode is requested. This is a
    /// request signal, not a final state — the actual logger set still
    /// depends on `full_diagnostics` winning when both are set. Use
    /// [`DiagnosticsCollector::is_trace_only`] for the effective state.
    pub fn wants_trace_only(&self) -> bool {
        self.trace_only && !self.full_diagnostics
    }

    /// Resolves the activation matrix entry based on env and (optionally)
    /// a pre-built session dir. Used by [`DiagnosticsCollector::new`].
    pub fn from_env(session_dir: Option<PathBuf>) -> Self {
        let full_diagnostics = std::env::var("RALPH_DIAGNOSTICS")
            .map(|v| v == "1")
            .unwrap_or(false);
        Self {
            full_diagnostics,
            runtime_diagnosis_artifacts: false,
            trace_only: false,
            session_dir,
        }
    }

    /// Resolves the activation matrix from env + the resolved value of
    /// `telemetry.runtime_diagnosis.write_artifacts` from ralph.yml.
    ///
    /// U0 wiring fix: the legacy `from_env` hardcoded `runtime_diagnosis_artifacts:
    /// false`, which silently dropped the `write_artifacts: true` config and left
    /// the minimal session path unreachable. This variant lets the CLI thread the
    /// resolved value through so the activation matrix matches plan U0:
    /// `write_artifacts=true` ⇒ `runtime_diagnosis_artifacts=true` ⇒ minimal
    /// session created without requiring `RALPH_DIAGNOSTICS=1`.
    ///
    /// `full_diagnostics` is still driven solely by `RALPH_DIAGNOSTICS=1` so the
    /// historical full-diagnostics loggers (orchestration/performance/errors/hook-runs
    /// + agent-output/prompt-log/trace) keep the same env-gated semantics.
    pub fn from_env_with_telemetry(session_dir: Option<PathBuf>, write_artifacts: bool) -> Self {
        let full_diagnostics = std::env::var("RALPH_DIAGNOSTICS")
            .map(|v| v == "1")
            .unwrap_or(false);
        Self {
            full_diagnostics,
            runtime_diagnosis_artifacts: write_artifacts,
            trace_only: false,
            session_dir,
        }
    }
}

/// Central coordinator for diagnostic logging.
///
/// Checks `RALPH_DIAGNOSTICS` environment variable and creates a timestamped
/// session directory if enabled. U0: exactly one instance per `ralph run`,
/// built in `main.rs` and shared with the tracing layer and the `EventLoop`.
///
/// `Clone` is a shallow clone: the underlying `Arc<Mutex<...>>` loggers
/// and `PathBuf` session dir are shared by reference. Cloning the
/// collector does NOT open a second session dir.
#[derive(Clone)]
pub struct DiagnosticsCollector {
    enabled: bool,
    full_diagnostics: bool,
    runtime_diagnosis_artifacts: bool,
    trace_only: bool,
    session_dir: Option<PathBuf>,
    orchestration_logger: Option<Arc<Mutex<orchestration::OrchestrationLogger>>>,
    performance_logger: Option<Arc<Mutex<performance::PerformanceLogger>>>,
    error_logger: Option<Arc<Mutex<errors::ErrorLogger>>>,
    hook_run_logger: Option<Arc<Mutex<hook_runs::HookRunLogger>>>,
    recovery_logger: Option<Arc<Mutex<recovery::RecoveryLogger>>>,
    drift_logger: Option<Arc<Mutex<drift::DriftLogger>>>,
}

impl std::fmt::Debug for DiagnosticsCollector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagnosticsCollector")
            .field("enabled", &self.enabled)
            .field("full_diagnostics", &self.full_diagnostics)
            .field(
                "runtime_diagnosis_artifacts",
                &self.runtime_diagnosis_artifacts,
            )
            .field("trace_only", &self.trace_only)
            .field("session_dir", &self.session_dir)
            .field(
                "has_orchestration_logger",
                &self.orchestration_logger.is_some(),
            )
            .field("has_performance_logger", &self.performance_logger.is_some())
            .field("has_error_logger", &self.error_logger.is_some())
            .field("has_hook_run_logger", &self.hook_run_logger.is_some())
            .field("has_recovery_logger", &self.recovery_logger.is_some())
            .field("has_drift_logger", &self.drift_logger.is_some())
            .finish()
    }
}

impl DiagnosticsCollector {
    /// Creates a new diagnostics collector.
    ///
    /// Honors `RALPH_DIAGNOSTICS=1` (see [`DiagnosticsOptions::from_env`]).
    /// For programmatic control, build [`DiagnosticsOptions`] explicitly
    /// and call [`Self::with_options`].
    pub fn new(base_path: &Path) -> std::io::Result<Self> {
        let options = DiagnosticsOptions::from_env(None);
        Self::with_options(base_path, &options)
    }

    /// Creates a diagnostics collector with explicit enabled flag (for testing).
    ///
    /// Thin wrapper over [`Self::with_options`] that maps the legacy bool
    /// onto [`DiagnosticsOptions::full_diagnostics`].
    pub fn with_enabled(base_path: &Path, enabled: bool) -> std::io::Result<Self> {
        let options = DiagnosticsOptions {
            full_diagnostics: enabled,
            ..DiagnosticsOptions::default()
        };
        Self::with_options(base_path, &options)
    }

    /// Canonical constructor.
    ///
    /// Drives the activation matrix in [`DiagnosticsOptions`]. When all
    /// flags are false, returns a no-op disabled collector with no I/O.
    /// When enabled, creates (or reuses) a timestamped session directory
    /// and instantiates the appropriate logger set.
    ///
    /// `trace_only=true` (with `full_diagnostics=false`) creates the
    /// session dir for the tracing layer but skips every loop-level
    /// logger (recovery/drift/orchestration/performance/errors/hook-runs/
    /// agent-output/prompt-log). `full_diagnostics=true` always wins
    /// (U1, 2026-06-14).
    pub fn with_options(base_path: &Path, options: &DiagnosticsOptions) -> std::io::Result<Self> {
        if !options.is_enabled() {
            return Ok(Self::disabled());
        }

        // Resolve or create the session directory exactly once per collector.
        let session_dir = match options.session_dir.as_ref() {
            Some(p) => {
                fs::create_dir_all(p)?;
                p.clone()
            }
            None => {
                let timestamp = Local::now().format("%Y-%m-%dT%H-%M-%S");
                let dir = base_path
                    .join(".ralph")
                    .join("diagnostics")
                    .join(timestamp.to_string());
                fs::create_dir_all(&dir)?;
                dir
            }
        };

        // Effective mode: full_diagnostics wins, otherwise honor
        // runtime_diagnosis_artifacts, otherwise honor trace_only.
        // trace_only is request-only — the actual logger set is determined
        // by the resolved `effective_*` booleans below.
        let effective_full = options.full_diagnostics;
        let effective_runtime = options.runtime_diagnosis_artifacts && !options.full_diagnostics;
        let effective_trace_only = options.wants_trace_only();

        // Historical loggers are tied to full_diagnostics. The minimal
        // runtime-diagnosis session deliberately skips them so we don't
        // create files nobody asked for. trace_only skips them too —
        // the parent TUI only needs the session dir, not loop-level files.
        let (orchestration_logger, performance_logger, error_logger, hook_run_logger) =
            if effective_full {
                let orch_logger = orchestration::OrchestrationLogger::new(&session_dir)?;
                let perf_logger = performance::PerformanceLogger::new(&session_dir)?;
                let err_logger = errors::ErrorLogger::new(&session_dir)?;
                let hook_logger = hook_runs::HookRunLogger::new(&session_dir)?;
                (
                    Some(Arc::new(Mutex::new(orch_logger))),
                    Some(Arc::new(Mutex::new(perf_logger))),
                    Some(Arc::new(Mutex::new(err_logger))),
                    Some(Arc::new(Mutex::new(hook_logger))),
                )
            } else {
                (None, None, None, None)
            };

        // U3: recovery / drift loggers. They are part of BOTH
        // `full_diagnostics` and the minimal `runtime_diagnosis_artifacts`
        // session, because the diagnosis pipeline is the whole point of
        // telemetry. They do NOT pull in agent-output / prompt-log.
        // The session dir is already guaranteed to exist at this point.
        //
        // trace_only skips these too: parent TUI has no loop events to
        // record, only trace/log.
        let recovery_logger = if effective_full || effective_runtime {
            match recovery::RecoveryLogger::new(&session_dir) {
                Ok(logger) => Some(Arc::new(Mutex::new(logger))),
                Err(err) => {
                    tracing::warn!(
                        target: "ralph_core::diagnostics",
                        session_dir = %session_dir.display(),
                        error = %err,
                        "failed to create recovery logger; recovery journal disabled for this session",
                    );
                    None
                }
            }
        } else {
            None
        };

        let drift_logger = if effective_full || effective_runtime {
            match drift::DriftLogger::new(&session_dir) {
                Ok(logger) => Some(Arc::new(Mutex::new(logger))),
                Err(err) => {
                    tracing::warn!(
                        target: "ralph_core::diagnostics",
                        session_dir = %session_dir.display(),
                        error = %err,
                        "failed to create drift logger; drift journal disabled for this session",
                    );
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            enabled: true,
            full_diagnostics: effective_full,
            runtime_diagnosis_artifacts: effective_runtime,
            trace_only: effective_trace_only,
            session_dir: Some(session_dir),
            orchestration_logger,
            performance_logger,
            error_logger,
            hook_run_logger,
            recovery_logger,
            drift_logger,
        })
    }

    /// Creates a disabled diagnostics collector without any I/O (for testing).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            full_diagnostics: false,
            runtime_diagnosis_artifacts: false,
            trace_only: false,
            session_dir: None,
            orchestration_logger: None,
            performance_logger: None,
            error_logger: None,
            hook_run_logger: None,
            recovery_logger: None,
            drift_logger: None,
        }
    }

    /// Returns whether any diagnostics are enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns true if the historical full-diagnostics logger set is active.
    pub fn is_full_diagnostics(&self) -> bool {
        self.full_diagnostics
    }

    /// Returns true if the minimal runtime-diagnosis session is active.
    pub fn has_runtime_diagnosis_artifacts(&self) -> bool {
        self.runtime_diagnosis_artifacts
    }

    /// Returns true if the collector is in trace-only mode (parent TUI):
    /// session dir is created for the tracing layer, but no loop-level
    /// loggers (recovery/drift/etc.) are instantiated. Effective state —
    /// if `full_diagnostics` is also true this returns `false` because
    /// full wins. (U1, 2026-06-14)
    pub fn is_trace_only(&self) -> bool {
        self.trace_only
    }

    /// Returns the session directory if diagnostics are enabled.
    pub fn session_dir(&self) -> Option<&Path> {
        self.session_dir.as_deref()
    }

    /// Writes a session pointer to
    /// `<main_repo>/.ralph/diagnostics-session-pointer.json` pointing
    /// back at this collector's session directory. Only call this when
    /// the collector is running inside a worktree — pass
    /// `worktree_path` (the worktree's absolute path, e.g. via
    /// `loop_context.workspace()`) so the function can detect that the
    /// session lives *inside* the worktree, not in main_repo. The
    /// pointer lets `ralph diagnose` find the worktree session after
    /// the loop ends and `loops.json` no longer carries an alive
    /// entry for it (U4, 2026-06-14; fix on 2026-06-14: previous
    /// `session_dir.starts_with(main_repo)` guard was inverted for
    /// the production worktree layout `<main_repo>/.worktrees/<id>/`
    /// where the worktree is a subpath of main_repo).
    ///
    /// Returns `Ok(false)` when the session is not inside the
    /// given worktree (i.e. a primary session), so the caller can
    /// log or no-op without checking the return value's "did it
    /// write" semantics.
    pub fn write_session_pointer(
        &self,
        main_repo: &Path,
        worktree_path: &Path,
    ) -> std::io::Result<bool> {
        let session_dir = match self.session_dir() {
            Some(d) => d,
            None => return Ok(false),
        };
        // Only emit a pointer when the session dir lives inside the
        // explicit worktree path. The caller (`run_loop_impl`) only
        // invokes us on non-primary contexts and passes the
        // LoopContext::workspace() value, so this check is the
        // canonical signal — not a fragile lexical prefix match
        // against main_repo (which would be inverted for the
        // production worktree layout).
        if !session_dir.starts_with(worktree_path) {
            return Ok(false);
        }
        let pointer_path = main_repo
            .join(".ralph")
            .join("diagnostics-session-pointer.json");
        let payload = serde_json::json!({
            "session_path": session_dir,
            "written_at": Utc::now().to_rfc3339(),
        });
        write_session_pointer_file(&pointer_path, &payload)?;
        Ok(true)
    }

    /// Wraps a stream handler with diagnostic logging.
    ///
    /// Returns the original handler if diagnostics are disabled.
    pub fn wrap_stream_handler<H>(&self, handler: H) -> Result<DiagnosticStreamHandler<H>, H> {
        if let Some(session_dir) = &self.session_dir
            && self.full_diagnostics
        {
            match AgentOutputLogger::new(session_dir) {
                Ok(logger) => {
                    let logger = Arc::new(Mutex::new(logger));
                    Ok(DiagnosticStreamHandler::new(handler, logger))
                }
                Err(_) => Err(handler), // Return original handler on error
            }
        } else {
            Err(handler) // Diagnostics disabled or minimal, return original
        }
    }

    /// Logs an orchestration event.
    ///
    /// Does nothing if full diagnostics are disabled.
    pub fn log_orchestration(&self, iteration: u32, hat: &str, event: OrchestrationEvent) {
        if let Some(logger) = &self.orchestration_logger
            && let Ok(mut logger) = logger.lock()
        {
            let _ = logger.log(iteration, hat, event);
        }
    }

    /// Logs execution contract rejections to diagnostics.
    ///
    /// Does nothing if full diagnostics are disabled.
    pub fn log_execution_contract_rejections(
        &self,
        iteration: u32,
        hat: &str,
        rejections: &[crate::execution_contract::ExecutionContractFinding],
    ) {
        if !rejections.is_empty() {
            for finding in rejections {
                let event = OrchestrationEvent::ExecutionContractRejected {
                    topic: finding.topic.clone(),
                    violation_kind: format!("{:?}", finding.kind),
                    message: finding.message.clone(),
                };
                self.log_orchestration(iteration, hat, event);
            }
        }
    }

    /// Logs a performance metric.
    ///
    /// Does nothing if full diagnostics are disabled.
    pub fn log_performance(&self, iteration: u32, hat: &str, metric: PerformanceMetric) {
        if let Some(logger) = &self.performance_logger
            && let Ok(mut logger) = logger.lock()
        {
            let _ = logger.log(iteration, hat, metric);
        }
    }

    /// Logs an error.
    ///
    /// Does nothing if full diagnostics are disabled.
    pub fn log_error(&self, iteration: u32, hat: &str, error: DiagnosticError) {
        if let Some(logger) = &self.error_logger
            && let Ok(mut logger) = logger.lock()
        {
            logger.set_context(iteration, hat);
            logger.log(error);
        }
    }

    /// Logs a hook run telemetry entry.
    ///
    /// Does nothing if full diagnostics are disabled.
    pub fn log_hook_run(&self, entry: HookRunTelemetryEntry) {
        if let Some(logger) = &self.hook_run_logger
            && let Ok(mut logger) = logger.lock()
        {
            let _ = logger.log(&entry);
        }
    }

    /// Logs the full prompt for an iteration to `prompt-log.md`.
    ///
    /// Does nothing if full diagnostics are disabled.
    pub fn log_prompt(&self, iteration: u32, hat: &str, prompt: &str) {
        if let Some(session_dir) = &self.session_dir
            && self.full_diagnostics
        {
            use std::io::Write;
            let path = session_dir.join("prompt-log.md");
            if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(
                    file,
                    "# Iteration {} — {}\n\n{}\n\n---\n",
                    iteration, hat, prompt
                );
            }
        }
    }

    /// Logs a recovery journal entry to `recovery.jsonl`.
    ///
    /// No-op if the recovery logger was not instantiated (i.e. when
    /// the collector is disabled or its creation failed at startup).
    /// Internal I/O errors are emitted via `tracing::warn!` and
    /// swallowed: the orchestration main path is never affected.
    pub fn log_recovery(&self, entry: crate::diagnosis::RecoveryJournalEntry) {
        let Some(logger) = self.recovery_logger.as_ref() else {
            return;
        };
        match logger.lock() {
            Ok(mut guard) => {
                if let Err(err) = guard.log(&entry) {
                    tracing::warn!(
                        target: "ralph_core::diagnostics",
                        session_dir = ?self.session_dir,
                        error = %err,
                        "failed to write recovery.jsonl entry; continuing without blocking the loop",
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    target: "ralph_core::diagnostics",
                    session_dir = ?self.session_dir,
                    error = %err,
                    "recovery logger mutex poisoned; skipping entry",
                );
            }
        }
    }

    /// Logs a drift journal entry to `drift.jsonl`.
    ///
    /// No-op if the drift logger was not instantiated. Internal I/O
    /// errors are emitted via `tracing::warn!` and swallowed.
    pub fn log_drift(&self, entry: crate::diagnosis::DriftJournalEntry) {
        let Some(logger) = self.drift_logger.as_ref() else {
            return;
        };
        match logger.lock() {
            Ok(mut guard) => {
                if let Err(err) = guard.log(&entry) {
                    tracing::warn!(
                        target: "ralph_core::diagnostics",
                        session_dir = ?self.session_dir,
                        error = %err,
                        "failed to write drift.jsonl entry; continuing without blocking the loop",
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    target: "ralph_core::diagnostics",
                    session_dir = ?self.session_dir,
                    error = %err,
                    "drift logger mutex poisoned; skipping entry",
                );
            }
        }
    }

    /// U8 (2026-06-27 mechanism foundation): idempotent variant of
    /// `log_recovery`. Routes the entry through the shared
    /// `IdempotentLog` (under the per-`retry_key`+`loop_id` key
    /// shape) so the recovery signal survives process restarts and
    /// cannot be overwritten by a stale `_final=true` from a
    /// previous loop on the same workspace.
    ///
    /// `is_final` flips the record's `_final` bit. When `true`,
    /// subsequent writes for the same `retry_key` are rejected by
    /// `IdempotentLog` — that is the entire point of the wiring
    /// (it stops the 2026-06-26 "two records claim `_final=true`"
    /// class of bug from happening).
    ///
    /// Internal I/O errors are emitted via `tracing::warn!` and
    /// swallowed — the existing `log_recovery` semantics that
    /// never block the orchestration main path are preserved.
    /// The caller must hold the `MutexGuard` from
    /// `EventLoop::idempotent_log()` because `IdempotentLog::append`
    /// requires `&mut self`.
    pub fn log_recovery_via_idempotent(
        &self,
        log: &mut crate::state::idempotent_log::IdempotentLog,
        retry_key: &str,
        payload: serde_json::Value,
        is_final: bool,
    ) {
        use crate::event_loop::idempotent_wiring as wiring;
        // Borrow checker: snapshot `loop_id` first so the mutable
        // borrow for `write_recovery` doesn't conflict with the
        // immutable `log.loop_id()` call.
        let loop_id = log.loop_id().to_string();
        if let Err(err) = wiring::write_recovery(log, retry_key, &loop_id, payload, is_final) {
            tracing::warn!(
                target: "ralph_core::diagnostics",
                retry_key = retry_key,
                error = %err,
                "idempotent log write for recovery entry failed; continuing without blocking the loop",
            );
        }
    }

    /// U8 (2026-06-27 mechanism foundation): idempotent variant of
    /// `log_drift`. Routes the finding through the shared
    /// `IdempotentLog` under the canonical `drift:{finding_id}:loop:{loop_id}`
    /// key. Drift records are advisory and always
    /// `_final=true` from creation, so a repeated write for the
    /// same finding_id surfaces as `FinalAlreadySet` and is
    /// silently swallowed (it is the expected outcome, not an
    /// error).
    ///
    /// Same error / lifetime contract as
    /// [`Self::log_recovery_via_idempotent`].
    pub fn log_drift_via_idempotent(
        &self,
        log: &mut crate::state::idempotent_log::IdempotentLog,
        finding_id: &str,
        payload: serde_json::Value,
    ) {
        use crate::event_loop::idempotent_wiring as wiring;
        let loop_id = log.loop_id().to_string();
        match wiring::write_drift(log, finding_id, &loop_id, payload) {
            Ok(()) => {}
            Err(crate::event_loop::idempotent_wiring::WiringError::Idempotent(
                crate::state::idempotent_log::IdempotentError::FinalAlreadySet(_),
            )) => {
                // Already finalised by an earlier observation; this
                // is the expected outcome for drift findings, not
                // a real error.
            }
            Err(err) => {
                tracing::warn!(
                    target: "ralph_core::diagnostics",
                    finding_id = finding_id,
                    error = %err,
                    "idempotent log write for drift entry failed; continuing without blocking the loop",
                );
            }
        }
    }

    /// Persist a `diagnosis-summary.json` seed file in the session
    /// directory.
    ///
    /// This is the "report seed" written at loop termination: it
    /// contains the known metadata (session id, paths, counts) so
    /// that `ralph diagnose` can refresh / complete the picture
    /// without re-parsing every journal line. It overwrites any
    /// existing file at `<session_dir>/diagnosis-summary.json`.
    ///
    /// No-op when no session directory is set. Internal I/O errors
    /// are emitted via `tracing::warn!` and swallowed.
    pub fn write_diagnosis_summary_seed(&self, summary: &DiagnosisSummary) {
        let Some(session_dir) = self.session_dir.as_ref() else {
            return;
        };
        let path = session_dir.join("diagnosis-summary.json");
        // Atomic write (R8): same `tempfile + persist` pattern as
        // `write_active_activations`. The destination is renamed into
        // place only after the payload has been fully serialized.
        let tmp = match NamedTempFile::new_in(session_dir) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(
                    target: "ralph_core::diagnostics",
                    session_dir = %session_dir.display(),
                    error = %err,
                    "failed to create temp file for diagnosis-summary.json; continuing without blocking the loop",
                );
                return;
            }
        };
        if let Err(err) = serde_json::to_writer_pretty(tmp.as_file(), summary) {
            tracing::warn!(
                target: "ralph_core::diagnostics",
                session_dir = %session_dir.display(),
                error = %err,
                "failed to serialize diagnosis-summary.json",
            );
            return;
        }
        if let Err(err) = tmp.persist(&path) {
            tracing::warn!(
                target: "ralph_core::diagnostics",
                session_dir = %session_dir.display(),
                error = %err,
                "failed to persist diagnosis-summary.json",
            );
        }
    }

    /// Persist the current active hat activations to
    /// `<session_dir>/active-activations.json`.
    ///
    /// Called both at loop termination (so the offline `ralph diagnose`
    /// reporter — U7 — can render the `## Active Hat Activations`
    /// section) and periodically by the loop runner's heartbeat (so the
    /// section stays fresh while the loop is still running; see
    /// `crates/ralph-cli/src/loop_runner/runner.rs` — `RALPH_ACTIVATIONS_HEARTBEAT_SEC`).
    /// The file is a JSON array of [`ActivationSnapshot`]s. An empty
    /// array is written when no activations are active.
    ///
    /// The write is atomic via `tempfile::NamedTempFile::persist` so a
    /// reader never sees a half-written file even if the loop is killed
    /// mid-write (R8 contract).
    ///
    /// No-op when no session directory is set. Internal I/O errors are
    /// emitted via `tracing::warn!` and swallowed.
    pub fn write_active_activations(
        &self,
        activations: &[crate::hat_lifecycle::ActivationSnapshot],
    ) {
        let Some(session_dir) = self.session_dir.as_ref() else {
            return;
        };
        let path = session_dir.join("active-activations.json");
        // Atomic write: write to a sibling temp file in the same dir
        // (so `persist` is a single `rename(2)`), then `persist`
        // atomically replaces the destination. Mirrors the R8
        // contract used by the other journal writers.
        let tmp = match NamedTempFile::new_in(session_dir) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(
                    target: "ralph_core::diagnostics",
                    session_dir = %session_dir.display(),
                    error = %err,
                    "failed to create temp file for active-activations.json; continuing without blocking the loop",
                );
                return;
            }
        };
        if let Err(err) = serde_json::to_writer_pretty(tmp.as_file(), activations) {
            tracing::warn!(
                target: "ralph_core::diagnostics",
                session_dir = %session_dir.display(),
                error = %err,
                "failed to serialize active-activations.json",
            );
            return;
        }
        if let Err(err) = tmp.persist(&path) {
            tracing::warn!(
                target: "ralph_core::diagnostics",
                session_dir = %session_dir.display(),
                error = %err,
                "failed to persist active-activations.json",
            );
        }
    }

    /// Returns the diagnostics session id, which is the timestamped
    /// directory name (e.g. `2026-06-05T10-20-30`). Returns `None`
    /// when the collector is disabled or has no session dir.
    ///
    /// U3 callers (U4 / U5 / U6) pass this value into
    /// [`crate::diagnosis::RecoveryDiagnosisEnvelopeBuilder::session_id`]
    /// so each entry can be traced back to its session.
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        self.session_dir
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(String::from)
    }
}

/// Summary seed written to `<session_dir>/diagnosis-summary.json` at
/// loop termination.
///
/// This is the "report seed": it captures the *known* metadata
/// (session id, generated-at, paths, counts) so `ralph diagnose`
/// (U7) can produce a Markdown / JSON report without having to
/// re-derive everything by hand. It is intentionally additive —
/// missing fields default to `None` / `0` / `[]` and U7 may extend it
/// without breaking older writers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosisSummary {
    /// Schema version. Bump when the JSON shape changes
    /// non-additively. Currently `1`.
    pub schema_version: u32,

    /// Diagnostics session id (timestamped directory name).
    pub session_id: String,

    /// Wall-clock time the seed was generated.
    pub generated_at: DateTime<Utc>,

    /// Loop start timestamp, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_started_at: Option<DateTime<Utc>>,

    /// Loop termination timestamp, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_terminated_at: Option<DateTime<Utc>>,

    /// Total loop iterations, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_iterations: Option<u32>,

    /// Termination reason (free-form), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination_reason: Option<String>,

    /// Relative or absolute path to `recovery.jsonl` (if present).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_journal_path: Option<String>,

    /// Relative or absolute path to `drift.jsonl` (if present).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_journal_path: Option<String>,

    /// Path to `orchestration.jsonl` (if full diagnostics).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration_log_path: Option<String>,

    /// Path to `errors.jsonl` (if full diagnostics).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors_log_path: Option<String>,

    /// Number of `RecoveryJournalEntry` records (so U7 can render
    /// without re-counting).
    pub recovery_count: u32,

    /// Number of `DriftJournalEntry` records.
    pub drift_finding_count: u32,

    /// Free-form notes for the operator (e.g. truncation warnings,
    /// missing-field warnings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl DiagnosisSummary {
    /// Schema version of [`DiagnosisSummary`]. Bump when the JSON
    /// shape changes non-additively.
    pub const SCHEMA_VERSION: u32 = 1;

    /// Build a [`DiagnosisSummary`] with sensible defaults for a
    /// given session id. All optional fields default to `None`,
    /// counts to `0`, and `notes` to an empty vector.
    #[must_use]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            session_id: session_id.into(),
            generated_at: Utc::now(),
            loop_started_at: None,
            loop_terminated_at: None,
            total_iterations: None,
            termination_reason: None,
            recovery_journal_path: None,
            drift_journal_path: None,
            orchestration_log_path: None,
            errors_log_path: None,
            recovery_count: 0,
            drift_finding_count: 0,
            notes: Vec::new(),
        }
    }
}

/// Atomic write helper for the session pointer file
/// `<main-repo>/.ralph/diagnostics-session-pointer.json`.
///
/// The child RPC process writes this pointer when a worktree loop starts
/// so that `ralph diagnose` can find the worktree's diagnostics root
/// after the loop ends and `loops.json` is no longer carrying an alive
/// entry for it (U4, 2026-06-14). The writer follows the same
/// `NamedTempFile::persist` pattern as `write_diagnosis_summary_seed`
/// and `write_active_activations` — the destination is only renamed
/// into place after the JSON payload is fully serialized, so readers
/// never see a half-written file.
///
/// Errors are surfaced as `io::Error` so the caller can decide whether
/// to log and continue (the loop should not block on a best-effort
/// pointer write).
pub fn write_session_pointer_file(
    pointer_path: &Path,
    payload: &serde_json::Value,
) -> std::io::Result<()> {
    if let Some(parent) = pointer_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = NamedTempFile::new_in(
        pointer_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
    )?;
    if let Err(err) = serde_json::to_writer_pretty(tmp.as_file(), payload) {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
    }
    tmp.persist(pointer_path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_diagnostics_disabled_by_default() {
        let temp = TempDir::new().unwrap();

        let collector =
            DiagnosticsCollector::with_options(temp.path(), &DiagnosticsOptions::default())
                .unwrap();

        assert!(!collector.is_enabled());
        assert!(collector.session_dir().is_none());
    }

    #[test]
    fn test_diagnostics_enabled_creates_directory() {
        let temp = TempDir::new().unwrap();

        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();

        assert!(collector.is_enabled());
        assert!(collector.session_dir().is_some());
        assert!(collector.session_dir().unwrap().exists());
    }

    #[test]
    fn test_session_directory_format() {
        let temp = TempDir::new().unwrap();

        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();

        let session_dir = collector.session_dir().unwrap();
        let dir_name = session_dir.file_name().unwrap().to_str().unwrap();

        // Verify format: YYYY-MM-DDTHH-MM-SS
        assert!(dir_name.len() == 19); // 2024-01-21T08-49-56
        assert!(dir_name.chars().nth(4) == Some('-'));
        assert!(dir_name.chars().nth(7) == Some('-'));
        assert!(dir_name.chars().nth(10) == Some('T'));
        assert!(dir_name.chars().nth(13) == Some('-'));
        assert!(dir_name.chars().nth(16) == Some('-'));
    }

    #[test]
    fn test_performance_logger_integration() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();

        // Log some performance metrics
        collector.log_performance(
            1,
            "ralph",
            PerformanceMetric::IterationDuration { duration_ms: 1500 },
        );
        collector.log_performance(
            1,
            "builder",
            PerformanceMetric::AgentLatency { duration_ms: 800 },
        );
        collector.log_performance(
            1,
            "builder",
            PerformanceMetric::TokenCount {
                input: 1000,
                output: 500,
            },
        );

        // Verify file exists
        let perf_file = collector.session_dir().unwrap().join("performance.jsonl");
        assert!(perf_file.exists(), "performance.jsonl should exist");

        // Verify content
        let content = std::fs::read_to_string(perf_file).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 3, "Should have 3 performance entries");

        // Verify each line is valid JSON
        for line in lines {
            let _: performance::PerformanceEntry = serde_json::from_str(line).unwrap();
        }
    }

    #[test]
    fn test_error_logger_integration() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();

        // Log some errors
        collector.log_error(
            1,
            "ralph",
            DiagnosticError::ParseError {
                source: "agent_output".to_string(),
                message: "Invalid JSON".to_string(),
                input: "{invalid".to_string(),
            },
        );
        collector.log_error(
            2,
            "builder",
            DiagnosticError::ValidationFailure {
                rule: "tests_required".to_string(),
                message: "Missing test evidence".to_string(),
                evidence: "tests: missing".to_string(),
            },
        );

        // Verify file exists
        let error_file = collector.session_dir().unwrap().join("errors.jsonl");
        assert!(error_file.exists(), "errors.jsonl should exist");

        // Verify content
        let content = std::fs::read_to_string(error_file).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 2, "Should have 2 error entries");

        // Verify each line is valid JSON
        for line in lines {
            let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(parsed.get("error_type").is_some());
            assert!(parsed.get("message").is_some());
            assert!(parsed.get("context").is_some());
        }
    }

    // ── U0 activation matrix tests ───────────────────────────────────────

    #[test]
    fn test_activation_matrix_default_disabled() {
        let temp = TempDir::new().unwrap();
        let collector =
            DiagnosticsCollector::with_options(temp.path(), &DiagnosticsOptions::default())
                .unwrap();

        assert!(!collector.is_enabled());
        assert!(!collector.is_full_diagnostics());
        assert!(!collector.has_runtime_diagnosis_artifacts());
        assert!(collector.session_dir().is_none());
        assert!(!temp.path().join(".ralph").join("diagnostics").exists());
    }

    #[test]
    fn test_activation_matrix_full_diagnostics() {
        let temp = TempDir::new().unwrap();
        let options = DiagnosticsOptions {
            full_diagnostics: true,
            ..DiagnosticsOptions::default()
        };
        let collector = DiagnosticsCollector::with_options(temp.path(), &options).unwrap();

        assert!(collector.is_enabled());
        assert!(collector.is_full_diagnostics());
        let session_dir = collector.session_dir().expect("session dir must exist");
        assert!(session_dir.exists());
        // Historical files (orchestration/performance/errors/hook-runs) are
        // created lazily by their respective loggers, but the dir is ready.
        assert!(session_dir.join("orchestration.jsonl").exists());
        assert!(session_dir.join("performance.jsonl").exists());
        assert!(session_dir.join("errors.jsonl").exists());
        assert!(session_dir.join("hook-runs.jsonl").exists());
    }

    #[test]
    fn test_activation_matrix_runtime_only_creates_dir_no_historical_files() {
        let temp = TempDir::new().unwrap();
        let options = DiagnosticsOptions {
            full_diagnostics: false,
            runtime_diagnosis_artifacts: true,
            ..DiagnosticsOptions::default()
        };
        let collector = DiagnosticsCollector::with_options(temp.path(), &options).unwrap();

        assert!(collector.is_enabled());
        assert!(!collector.is_full_diagnostics());
        assert!(collector.has_runtime_diagnosis_artifacts());
        let session_dir = collector.session_dir().expect("session dir must exist");
        assert!(session_dir.exists());

        // Verify the timestamp format.
        let dir_name = session_dir.file_name().unwrap().to_str().unwrap();
        assert_eq!(dir_name.len(), 19, "expected YYYY-MM-DDTHH-MM-SS");

        // The historical full-diagnostics files MUST NOT be present.
        assert!(!session_dir.join("orchestration.jsonl").exists());
        assert!(!session_dir.join("performance.jsonl").exists());
        assert!(!session_dir.join("errors.jsonl").exists());
        assert!(!session_dir.join("hook-runs.jsonl").exists());
        assert!(!session_dir.join("prompt-log.md").exists());
    }

    #[test]
    fn test_activation_matrix_session_dir_reuse_full() {
        let temp = TempDir::new().unwrap();
        let preset_dir = temp.path().join("reused-session");
        std::fs::create_dir_all(&preset_dir).unwrap();

        let options = DiagnosticsOptions {
            full_diagnostics: true,
            session_dir: Some(preset_dir.clone()),
            ..DiagnosticsOptions::default()
        };
        let collector = DiagnosticsCollector::with_options(temp.path(), &options).unwrap();

        assert_eq!(collector.session_dir().unwrap(), preset_dir);
        // Make sure the timestamped dir under .ralph/diagnostics was NOT created.
        assert!(!temp.path().join(".ralph").join("diagnostics").exists());
    }

    #[test]
    fn test_activation_matrix_session_dir_reuse_minimal() {
        let temp = TempDir::new().unwrap();
        let preset_dir = temp.path().join("reused-session");
        std::fs::create_dir_all(&preset_dir).unwrap();

        let options = DiagnosticsOptions {
            full_diagnostics: false,
            runtime_diagnosis_artifacts: true,
            session_dir: Some(preset_dir.clone()),
            ..DiagnosticsOptions::default()
        };
        let collector = DiagnosticsCollector::with_options(temp.path(), &options).unwrap();

        assert_eq!(collector.session_dir().unwrap(), preset_dir);
        assert!(!temp.path().join(".ralph").join("diagnostics").exists());
    }

    #[test]
    fn test_init_failure_does_not_panic() {
        // An unwritable base_path must surface as an io::Error, not a panic.
        // On Linux, writing under /proc/self/foo is invalid.
        let bogus = std::path::Path::new("/proc/self/cannot-write-here");
        let options = DiagnosticsOptions {
            full_diagnostics: true,
            ..DiagnosticsOptions::default()
        };
        let result = DiagnosticsCollector::with_options(bogus, &options);
        assert!(
            result.is_err(),
            "expected io::Error, got {:?}",
            result.is_ok()
        );
    }

    // ── trace_only mode (U1, 2026-06-14) ─────────────────────────────────
    // The subprocess TUI parent must create a session dir for the tracing
    // layer and TUI stderr log, but MUST NOT instantiate loop-level loggers
    // (recovery/drift/orchestration/performance/errors/hook-runs/agent-output/
    // prompt-log) — otherwise the parent leaves empty shells in the main repo
    // while the child RPC process writes the real data into the worktree.

    #[test]
    fn test_trace_only_creates_session_dir_without_loop_loggers() {
        let temp = TempDir::new().unwrap();
        let options = DiagnosticsOptions {
            full_diagnostics: false,
            runtime_diagnosis_artifacts: false,
            trace_only: true,
            ..DiagnosticsOptions::default()
        };
        let collector = DiagnosticsCollector::with_options(temp.path(), &options).unwrap();

        // Session dir must exist (parent TUI needs it for the trace layer).
        assert!(collector.is_enabled());
        assert!(collector.is_trace_only());
        assert!(!collector.is_full_diagnostics());
        assert!(!collector.has_runtime_diagnosis_artifacts());
        let session_dir = collector.session_dir().expect("session dir must exist");
        assert!(session_dir.exists());

        // No loop-level files: the parent's empty shell problem.
        for name in [
            "recovery.jsonl",
            "drift.jsonl",
            "orchestration.jsonl",
            "performance.jsonl",
            "errors.jsonl",
            "hook-runs.jsonl",
            "agent-output.jsonl",
            "prompt-log.md",
        ] {
            assert!(
                !session_dir.join(name).exists(),
                "trace_only must not create {name}"
            );
        }

        // The session dir is created under .ralph/diagnostics/<timestamp>/
        // just like the full / minimal modes. trace_only only differs by
        // skipping the loop-level loggers; the directory shape is
        // identical so downstream tooling (TUI, trace layer) sees the
        // same path layout.
        assert!(session_dir.starts_with(temp.path().join(".ralph").join("diagnostics")));
    }

    #[test]
    fn test_trace_only_full_diagnostics_priority_wins() {
        let temp = TempDir::new().unwrap();
        let options = DiagnosticsOptions {
            full_diagnostics: true,
            runtime_diagnosis_artifacts: false,
            trace_only: true,
            ..DiagnosticsOptions::default()
        };
        let collector = DiagnosticsCollector::with_options(temp.path(), &options).unwrap();

        // full_diagnostics wins: trace_only is ignored, full logger set
        // is created. This mirrors the existing
        // runtime_diagnosis_artifacts-vs-full precedence contract.
        assert!(!collector.is_trace_only());
        assert!(collector.is_full_diagnostics());
        let session_dir = collector.session_dir().expect("session dir must exist");
        assert!(session_dir.exists());
        assert!(session_dir.join("orchestration.jsonl").exists());
        assert!(session_dir.join("performance.jsonl").exists());
    }

    #[test]
    fn test_trace_only_query_method_default_false() {
        // Default (no flags set) must NOT be trace_only: backwards compat.
        let temp = TempDir::new().unwrap();
        let collector =
            DiagnosticsCollector::with_options(temp.path(), &DiagnosticsOptions::default())
                .unwrap();
        assert!(!collector.is_trace_only());
        assert!(!collector.is_enabled());
    }

    #[test]
    fn test_trace_only_reuses_preset_session_dir() {
        // Mirrors the existing session_dir reuse contract: if the caller
        // pins session_dir, we do not create a timestamped subdir.
        let temp = TempDir::new().unwrap();
        let preset_dir = temp.path().join("parent-trace-session");
        std::fs::create_dir_all(&preset_dir).unwrap();
        let options = DiagnosticsOptions {
            full_diagnostics: false,
            runtime_diagnosis_artifacts: false,
            trace_only: true,
            session_dir: Some(preset_dir.clone()),
        };
        let collector = DiagnosticsCollector::with_options(temp.path(), &options).unwrap();
        assert!(collector.is_trace_only());
        assert_eq!(collector.session_dir().unwrap(), preset_dir);
        assert!(!temp.path().join(".ralph").join("diagnostics").exists());
    }

    // ── U4 (2026-06-14): session pointer file for worktree loop resolve ──

    #[test]
    fn test_write_session_pointer_for_worktree_session() {
        // Simulate a worktree loop: session lives under the worktree,
        // main_repo is somewhere else entirely. write_session_pointer
        // must emit a pointer in main_repo's .ralph/.
        let temp = TempDir::new().unwrap();
        let worktree = temp.path().join("worktree");
        let main_repo = temp.path().join("main-repo");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&main_repo).unwrap();

        let session_dir = worktree
            .join(".ralph")
            .join("diagnostics")
            .join("2026-06-14T10-20-30");
        std::fs::create_dir_all(&session_dir).unwrap();

        let options = DiagnosticsOptions {
            full_diagnostics: true,
            session_dir: Some(session_dir.clone()),
            ..DiagnosticsOptions::default()
        };
        let collector = DiagnosticsCollector::with_options(&worktree, &options).unwrap();
        assert_eq!(collector.session_dir().unwrap(), session_dir);

        let wrote = collector
            .write_session_pointer(&main_repo, &worktree)
            .expect("write ok");
        assert!(wrote, "expected pointer to be written for worktree session");

        let pointer_path = main_repo
            .join(".ralph")
            .join("diagnostics-session-pointer.json");
        assert!(pointer_path.exists());
        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&pointer_path).unwrap()).unwrap();
        assert_eq!(
            payload.get("session_path").and_then(|v| v.as_str()),
            Some(session_dir.to_str().unwrap())
        );
        assert!(payload.get("written_at").is_some());
    }

    /// Regression test for the production worktree layout
    /// (`<main_repo>/.worktrees/<id>/` — worktree is a SUBPATH of
    /// main_repo). The original `starts_with(main_repo)` guard was
    /// inverted and never wrote the pointer in production (ce-code-
    /// review P0, 2026-06-14). The fix passes the explicit worktree
    /// path so the comparison is unambiguous.
    #[test]
    fn test_write_session_pointer_for_production_subpath_worktree() {
        let temp = TempDir::new().unwrap();
        // Production layout: worktree lives under main_repo/.worktrees/<id>
        let main_repo = temp.path();
        let worktree = main_repo.join(".worktrees").join("loop-1234");
        std::fs::create_dir_all(&worktree).unwrap();

        let session_dir = worktree
            .join(".ralph")
            .join("diagnostics")
            .join("2026-06-14T10-20-30");
        std::fs::create_dir_all(&session_dir).unwrap();

        let options = DiagnosticsOptions {
            full_diagnostics: true,
            session_dir: Some(session_dir.clone()),
            ..DiagnosticsOptions::default()
        };
        let collector = DiagnosticsCollector::with_options(&worktree, &options).unwrap();
        assert_eq!(collector.session_dir().unwrap(), session_dir);

        // Pre-fix code (starts_with(main_repo)) would return Ok(false)
        // here because the worktree IS under main_repo. The fix
        // accepts the worktree path explicitly and writes the pointer.
        let wrote = collector
            .write_session_pointer(main_repo, &worktree)
            .expect("write ok");
        assert!(wrote, "P0: production worktree subpath must write pointer");

        let pointer_path = main_repo
            .join(".ralph")
            .join("diagnostics-session-pointer.json");
        assert!(pointer_path.exists(), "pointer file must be created");
        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&pointer_path).unwrap()).unwrap();
        assert_eq!(
            payload.get("session_path").and_then(|v| v.as_str()),
            Some(session_dir.to_str().unwrap())
        );
    }

    #[test]
    fn test_write_session_pointer_skipped_for_primary_session() {
        // When the session is in the main repo (primary mode), we
        // MUST NOT write a pointer — the diagnose path already finds
        // primary sessions via loops.json / fallback to .ralph/diagnostics.
        // Pass a *different* worktree_path that the session does NOT
        // live under; the function must no-op.
        let temp = TempDir::new().unwrap();
        let options = DiagnosticsOptions {
            full_diagnostics: true,
            ..DiagnosticsOptions::default()
        };
        let collector = DiagnosticsCollector::with_options(temp.path(), &options).unwrap();
        let _session_dir = collector.session_dir().unwrap().to_path_buf();

        // Pretend the worktree is a sibling of main_repo (not a subpath).
        // session_dir starts with temp.path() but NOT with this sibling.
        let fake_worktree = temp.path().join("..").join("sibling-worktree");
        let wrote = collector
            .write_session_pointer(temp.path(), &fake_worktree)
            .expect("write ok");
        assert!(!wrote, "primary session must not write a pointer");

        // Sanity: session dir is indeed under main_repo.
        assert!(_session_dir.starts_with(temp.path()));
    }

    #[test]
    fn test_write_session_pointer_disabled_collector_returns_false() {
        // A disabled collector has no session_dir → write_session_pointer
        // must be a no-op and return Ok(false), not error.
        let temp = TempDir::new().unwrap();
        let main_repo = temp.path().join("main-repo");
        std::fs::create_dir_all(&main_repo).unwrap();
        let collector =
            DiagnosticsCollector::with_options(temp.path(), &DiagnosticsOptions::default())
                .unwrap();
        assert!(!collector.is_enabled());
        // Pass any worktree_path; the disabled collector's session_dir
        // is None, so the function returns Ok(false) before comparing.
        let wrote = collector
            .write_session_pointer(&main_repo, &main_repo)
            .expect("disabled write ok");
        assert!(!wrote);
        assert!(
            !main_repo
                .join(".ralph")
                .join("diagnostics-session-pointer.json")
                .exists()
        );
    }
}
