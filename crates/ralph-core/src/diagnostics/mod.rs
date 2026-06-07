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
pub use orchestration::{OrchestrationEntry, OrchestrationEvent, OrchestrationLogger};
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
}

impl DiagnosticsOptions {
    /// Returns true when any diagnostic capture is active.
    pub fn is_enabled(&self) -> bool {
        self.full_diagnostics || self.runtime_diagnosis_artifacts
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
    /// Drives the activation matrix in [`DiagnosticsOptions`]. When both
    /// flags are false, returns a no-op disabled collector with no I/O.
    /// When enabled, creates (or reuses) a timestamped session directory
    /// and instantiates the appropriate logger set.
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

        // Historical loggers are tied to full_diagnostics. The minimal
        // runtime-diagnosis session deliberately skips them so we don't
        // create files nobody asked for.
        let (orchestration_logger, performance_logger, error_logger, hook_run_logger) =
            if options.full_diagnostics {
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
        let recovery_logger = if options.is_enabled() {
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

        let drift_logger = if options.is_enabled() {
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
            full_diagnostics: options.full_diagnostics,
            runtime_diagnosis_artifacts: options.runtime_diagnosis_artifacts,
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

    /// Returns the session directory if diagnostics are enabled.
    pub fn session_dir(&self) -> Option<&Path> {
        self.session_dir.as_deref()
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
        let file = match fs::File::create(&path) {
            Ok(f) => f,
            Err(err) => {
                tracing::warn!(
                    target: "ralph_core::diagnostics",
                    session_dir = %session_dir.display(),
                    error = %err,
                    "failed to create diagnosis-summary.json; continuing without blocking the loop",
                );
                return;
            }
        };
        if let Err(err) = serde_json::to_writer_pretty(file, summary) {
            tracing::warn!(
                target: "ralph_core::diagnostics",
                session_dir = %session_dir.display(),
                error = %err,
                "failed to serialize diagnosis-summary.json",
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
}
