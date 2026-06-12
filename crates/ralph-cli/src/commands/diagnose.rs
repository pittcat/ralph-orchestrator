//! `ralph diagnose` — offline report built from session artifacts.
//!
//! U7 of the drift-auto-calibration plan. Wraps the pure reporter in
//! [`ralph_core::diagnosis::reporter`] with clap and stdout discipline:
//!
//! - Markdown output goes to stdout (default).
//! - `--output <PATH>` writes the report to a file and prints only
//!   the written path (or a short summary line for JSON).
//! - `--format json` always writes JSON to stdout, never Markdown
//!   headings.
//! - Missing session is a non-zero exit with a stderr hint that
//!   points at `RALPH_DIAGNOSTICS=1 ralph run ...` or the
//!   `telemetry.runtime_diagnosis.write_artifacts` config.

use crate::cli::ColorMode;
use crate::display::colors;
use crate::operation_guard::read_loop_id_marker;
use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use ralph_core::diagnosis::{
    Report, ReporterError, SessionSelector, build_report, render_json, render_markdown,
};
use ralph_core::loop_lock::{LockStatus, LoopLock};
use ralph_core::loop_registry::LoopEntry;
use std::path::{Path, PathBuf};

/// Arguments for the `ralph diagnose` subcommand.
#[derive(Parser, Debug)]
pub struct DiagnoseArgs {
    /// Session to read from. Accepts:
    /// - "latest" (default) — pick the most recent timestamped session
    /// - an absolute path
    /// - a relative path
    /// - a timestamped session id relative to `--diagnostics-root`
    #[arg(long, default_value = "latest", value_name = "SESSION")]
    pub session: String,

    /// Output format. Markdown (default) is human-readable; JSON is
    /// the stable CI contract (schema_version="1").
    #[arg(long, value_enum, default_value_t = DiagnoseFormat::Markdown)]
    pub format: DiagnoseFormat,

    /// Write the report to this path instead of stdout. When set,
    /// stdout receives only the written path (Markdown) or a short
    /// summary line (JSON).
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Path to the diagnostics root. Defaults to: read
    /// `<workspace>/.ralph/loops.json`, take the latest active loop's
    /// `workspace.workspace` field, and use `<that-workspace>/.ralph/diagnostics`.
    /// Falls back to `<workspace>/.ralph/diagnostics` when `loops.json`
    /// is missing, empty, or its latest entry points at a dead worktree.
    #[arg(long, value_name = "PATH")]
    pub diagnostics_root: Option<PathBuf>,

    /// D7: filter the rendered report to a single `DiagnosisSource`.
    /// Accepts the snake_case name (e.g. `agent_doc_sync`,
    /// `payload_contract`, `drift_monitor`). Unknown names are rejected
    /// with a list of available values.
    #[arg(long, value_name = "SOURCE")]
    pub source: Option<String>,
}

/// Output format for `ralph diagnose`. Mirrors
/// [`crate::cli::OutputFormat`] but stays local to keep the
/// diagnose subcommand self-contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DiagnoseFormat {
    /// Render a Markdown report on stdout (default).
    Markdown,
    /// Render a stable JSON document on stdout.
    Json,
}

/// Exit codes. Match the spec from the U7 plan: missing session
/// → non-zero, everything else (including missing partial files)
/// → zero.
pub const EXIT_OK: i32 = 0;
pub const EXIT_NO_SESSION: i32 = 2;
pub const EXIT_INVALID: i32 = 3;
pub const EXIT_IO: i32 = 4;

/// Error categories surfaced by `try_diagnose`. The CLI uses these
/// to compute the right exit code and stderr message; tests use them
/// to assert the failure mode.
#[derive(Debug)]
#[allow(dead_code)]
pub enum DiagnoseExit {
    /// The session was rendered (with or without warnings).
    Ok,
    /// The diagnostics root is missing or has no sessions.
    NoSession(PathBuf),
    /// An explicit session path is invalid.
    InvalidSession(PathBuf),
    /// I/O error reading the diagnostics root.
    Io(PathBuf, std::io::Error),
}

impl DiagnoseExit {
    /// Numeric exit code for the public CLI contract.
    #[must_use]
    pub fn code(&self) -> i32 {
        match self {
            DiagnoseExit::Ok => EXIT_OK,
            DiagnoseExit::NoSession(_) => EXIT_NO_SESSION,
            DiagnoseExit::InvalidSession(_) => EXIT_INVALID,
            DiagnoseExit::Io(_, _) => EXIT_IO,
        }
    }
}

/// Public CLI entry point. Prints to stdout / stderr and exits with
/// the appropriate code on failure.
pub fn diagnose_command(color_mode: ColorMode, args: DiagnoseArgs) -> Result<()> {
    match try_diagnose(color_mode, args) {
        Ok(()) => Ok(()),
        Err(exit) => {
            if exit.code() != EXIT_OK {
                std::process::exit(exit.code());
            }
            Ok(())
        }
    }
}

/// Test-friendly entry point: returns the [`DiagnoseExit`] instead
/// of calling `std::process::exit`. Used by `diagnose_command` and
/// by integration / unit tests.
pub fn try_diagnose(
    color_mode: ColorMode,
    args: DiagnoseArgs,
) -> std::result::Result<(), DiagnoseExit> {
    let use_colors = color_mode.should_use_colors();
    validate_args(&args)
        .map_err(|_| DiagnoseExit::InvalidSession(PathBuf::from("<invalid --output>")))?;
    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let diagnostics_root = match args.diagnostics_root.as_ref() {
        Some(p) => p.clone(),
        None => resolve_diagnostics_root_via_loops(&workspace_root),
    };
    let selector = if args.session.eq_ignore_ascii_case("latest") || args.session.is_empty() {
        SessionSelector::Latest
    } else {
        SessionSelector::Explicit(args.session.as_str())
    };
    let report = match build_report(selector, &diagnostics_root) {
        Ok(report) => report,
        Err(ReporterError::NoSession(path)) => {
            print_no_session_hint(&diagnostics_root, &path, use_colors);
            return Err(DiagnoseExit::NoSession(path));
        }
        Err(ReporterError::InvalidSession(path)) => {
            print_invalid_session(&path, use_colors);
            return Err(DiagnoseExit::InvalidSession(path));
        }
        Err(ReporterError::Io(path, err)) => {
            print_io_error(&path, &err, use_colors);
            return Err(DiagnoseExit::Io(path, err));
        }
    };
    // D7: `--source <NAME>` filters the rendered report to a single
    // `DiagnosisSource`. Filtering happens **after** `build_report`
    // (so all parsing / aggregation still runs) and **before**
    // `emit_report` (so the markdown / json output is just the
    // matching subset).
    let report = match args.source.as_deref() {
        None => report,
        Some(name) => match filter_report_by_source(report, name) {
            Ok(filtered) => filtered,
            Err(()) => {
                print_unknown_source(name, use_colors);
                return Err(DiagnoseExit::InvalidSession(PathBuf::from(format!(
                    "--source {name}"
                ))));
            }
        },
    };
    emit_report(&report, &args, use_colors).map_err(|e| {
        DiagnoseExit::Io(
            report.session_path.clone(),
            std::io::Error::other(e.to_string()),
        )
    })?;
    Ok(())
}

/// D7: filter a [`Report`] to entries whose `source` matches `name`.
///
/// Returns `Err(())` if `name` is not a known `DiagnosisSource`
/// snake_case name; the caller surfaces a helpful error.
///
/// Scope: filters `top_findings` only (the only per-source struct in
/// the report). The `recovery_timeline` aggregates by hat and does
/// not preserve `source` post-aggregation, so timeline rows are
/// passed through unchanged.
fn filter_report_by_source(mut report: Report, name: &str) -> std::result::Result<Report, ()> {
    if !is_known_source_name(name) {
        return Err(());
    }
    report.top_findings.retain(|f| f.source == name);
    Ok(report)
}

/// D7: returns `true` when `name` matches one of the snake_case
/// `DiagnosisSource` variants.
fn is_known_source_name(name: &str) -> bool {
    [
        "stall_recovery",
        "missing_event_gate",
        "workflow_guard",
        "execution_contract",
        "payload_contract",
        "drift_monitor",
        "hook_retry",
        "loop_stale",
        "topic_format",
        "agent_doc_sync",
    ]
    .contains(&name)
}

fn print_unknown_source(name: &str, use_colors: bool) {
    if use_colors {
        eprintln!(
            "{}error:{} unknown --source '{}'\navailable: stall_recovery, missing_event_gate, workflow_guard, execution_contract, payload_contract, drift_monitor, hook_retry, loop_stale, topic_format, agent_doc_sync",
            colors::RED,
            colors::RESET,
            name
        );
    } else {
        eprintln!(
            "error: unknown --source '{name}'\navailable: stall_recovery, missing_event_gate, workflow_guard, execution_contract, payload_contract, drift_monitor, hook_retry, loop_stale, topic_format, agent_doc_sync"
        );
    }
}

fn emit_report(report: &Report, args: &DiagnoseArgs, use_colors: bool) -> Result<()> {
    let body = match args.format {
        DiagnoseFormat::Markdown => render_markdown(report),
        DiagnoseFormat::Json => {
            let value = render_json(report);
            serde_json::to_string_pretty(&value)
                .context("failed to serialize diagnose report to JSON")?
        }
    };
    if let Some(path) = args.output.as_ref() {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create output directory {}", parent.display())
            })?;
        }
        std::fs::write(path, body.as_bytes())
            .with_context(|| format!("failed to write report to {}", path.display()))?;
        let summary = match args.format {
            DiagnoseFormat::Markdown => format!("wrote markdown report to {}", path.display()),
            DiagnoseFormat::Json => {
                format!("wrote json report to {} (schema v1)", path.display())
            }
        };
        if use_colors {
            println!("{}{}{}", colors::GREEN, summary, colors::RESET);
        } else {
            println!("{summary}");
        }
    } else {
        println!("{body}");
    }
    Ok(())
}

fn print_no_session_hint(diagnostics_root: &Path, missing: &Path, use_colors: bool) {
    if use_colors {
        eprintln!(
            "{}error:{} no diagnostics sessions at {}",
            colors::RED,
            colors::RESET,
            diagnostics_root.display()
        );
    } else {
        eprintln!(
            "error: no diagnostics sessions at {}",
            diagnostics_root.display()
        );
    }
    eprintln!("(resolved to {})", missing.display());
    eprintln!(
        "Hint: re-run with `RALPH_DIAGNOSTICS=1 ralph run ...`,\n\
         or set `telemetry.runtime_diagnosis.enabled: true` and\n\
         `telemetry.runtime_diagnosis.write_artifacts: true` in ralph.yml."
    );
}

fn print_invalid_session(path: &Path, use_colors: bool) {
    if use_colors {
        eprintln!(
            "{}error:{} session path {} is not a valid diagnostics session directory",
            colors::RED,
            colors::RESET,
            path.display()
        );
    } else {
        eprintln!(
            "error: session path {} is not a valid diagnostics session directory",
            path.display()
        );
    }
}

fn print_io_error(path: &Path, err: &std::io::Error, use_colors: bool) {
    if use_colors {
        eprintln!(
            "{}error:{} I/O reading {}: {}",
            colors::RED,
            colors::RESET,
            path.display(),
            err
        );
    } else {
        eprintln!("error: I/O reading {}: {}", path.display(), err);
    }
}

/// Validate that the CLI args are consistent. Empty `--output` is
/// rejected so users get a clear error instead of a confusing
/// "failed to write report to " failure.
pub fn validate_args(args: &DiagnoseArgs) -> Result<()> {
    if let Some(path) = &args.output
        && path.as_os_str().is_empty()
    {
        bail!("--output must not be empty");
    }
    Ok(())
}

/// R5/R6: workspace resolution result for `ralph diagnose`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LoopsDiagnosticsResolution {
    diagnostics_root: PathBuf,
    selected_loop_id: Option<String>,
    warnings: Vec<String>,
}

/// U3/R5/R6: Resolve the diagnostics root via `loops.json`.
///
/// When `--diagnostics-root` is not explicitly provided, read
/// `<workspace_root>/.ralph/loops.json` to find the latest active
/// loop entry and use its `workspace` field as the diagnostics root
/// base. Falls back to `<workspace_root>/.ralph/diagnostics` when:
/// - `loops.json` does not exist or is unreadable.
/// - No active loop entries are found.
/// - The resolved workspace's diagnostics dir does not exist
///   (the caller still gets a proper `NoSession` error).
///
/// Emits stderr warnings (R6) when the root `.ralph/current-loop-id`
/// marker or primary `loop.lock` disagree with the selected live loop.
fn resolve_diagnostics_root_via_loops(workspace_root: &Path) -> PathBuf {
    let resolution = resolve_diagnostics_root_with_warnings(workspace_root);
    for warning in &resolution.warnings {
        eprintln!("warning: {warning}");
    }
    resolution.diagnostics_root
}

fn resolve_diagnostics_root_with_warnings(
    workspace_root: &Path,
) -> LoopsDiagnosticsResolution {
    let fallback = workspace_root.join(".ralph").join("diagnostics");
    let loops_path = workspace_root.join(".ralph").join("loops.json");
    let mut warnings = Vec::new();

    let entries: Vec<LoopEntry> = match std::fs::read_to_string(&loops_path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => {
            collect_loop_pointer_warnings(workspace_root, None, &[], &mut warnings);
            return LoopsDiagnosticsResolution {
                diagnostics_root: fallback,
                selected_loop_id: None,
                warnings,
            };
        }
    };

    let live: Vec<&LoopEntry> = entries.iter().filter(|e| e.is_pid_alive()).collect();
    let dead_count = entries.len().saturating_sub(live.len());
    if dead_count > 0 {
        warnings.push(format!(
            "{} of {} loop entries in {} are dead; ignoring them for workspace resolution",
            dead_count,
            entries.len(),
            loops_path.display()
        ));
    }

    let selected = live.iter().max_by_key(|e| e.started).copied();
    collect_loop_pointer_warnings(workspace_root, selected, &entries, &mut warnings);

    if let Some(latest) = selected {
        let ws = PathBuf::from(&latest.workspace);
        let diag = ws.join(".ralph").join("diagnostics");
        if diag.exists() {
            return LoopsDiagnosticsResolution {
                diagnostics_root: diag,
                selected_loop_id: Some(latest.id.clone()),
                warnings,
            };
        }
    }

    LoopsDiagnosticsResolution {
        diagnostics_root: fallback,
        selected_loop_id: selected.map(|e| e.id.clone()),
        warnings,
    }
}

/// R6: surface registry drift without blocking diagnose.
fn collect_loop_pointer_warnings(
    workspace_root: &Path,
    selected: Option<&LoopEntry>,
    entries: &[LoopEntry],
    warnings: &mut Vec<String>,
) {
    let marker_id = read_loop_id_marker(workspace_root);

    if let Some(marker) = marker_id.as_deref() {
        match selected {
            Some(sel) if marker != sel.id => {
                warnings.push(format!(
                    ".ralph/current-loop-id points to '{marker}' but diagnostics resolved to live loop '{}' (workspace: {})",
                    sel.id, sel.workspace
                ));
            }
            None => {
                let marker_entry = entries.iter().find(|e| e.id == marker);
                if marker_entry.is_some_and(|e| !e.is_pid_alive()) {
                    warnings.push(format!(
                        ".ralph/current-loop-id points to dead loop '{marker}'; no live loop selected for workspace resolution"
                    ));
                } else {
                    warnings.push(format!(
                        ".ralph/current-loop-id is '{marker}' but no live loop entry found in loops.json"
                    ));
                }
            }
            _ => {}
        }
    }

    let Some(sel) = selected else {
        return;
    };

    // loop.lock only applies to the primary workspace; worktree loops do not hold it.
    if sel.worktree_path.is_some() {
        return;
    }

    match LoopLock::inspect(workspace_root) {
        Ok(LockStatus::Active(meta)) => {
            if meta.pid != sel.pid {
                warnings.push(format!(
                    ".ralph/loop.lock is held by pid {} but selected live loop '{}' has pid {}",
                    meta.pid, sel.id, sel.pid
                ));
            }
        }
        Ok(LockStatus::Stale(meta)) => {
            warnings.push(format!(
                ".ralph/loop.lock is stale (pid {}); selected live loop '{}' has pid {}",
                meta.pid, sel.id, sel.pid
            ));
        }
        Ok(LockStatus::None) => {
            warnings.push(format!(
                "selected primary loop '{}' is live but .ralph/loop.lock is absent",
                sel.id
            ));
        }
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::diagnosis::RankedFinding;

    fn base_args(diagnostics_root: &Path) -> DiagnoseArgs {
        DiagnoseArgs {
            session: "latest".to_string(),
            format: DiagnoseFormat::Markdown,
            output: None,
            diagnostics_root: Some(diagnostics_root.to_path_buf()),
            source: None,
        }
    }

    #[test]
    fn empty_output_path_is_rejected() {
        let mut args = DiagnoseArgs {
            session: "latest".to_string(),
            format: DiagnoseFormat::Markdown,
            output: Some(PathBuf::new()),
            diagnostics_root: None,
            source: None,
        };
        assert!(validate_args(&args).is_err());
        args.output = None;
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn try_diagnose_with_missing_root_returns_no_session() {
        let tmp = tempfile::tempdir().unwrap();
        let args = base_args(&tmp.path().join(".ralph/diagnostics"));
        let result = try_diagnose(ColorMode::Never, args);
        match result {
            Err(DiagnoseExit::NoSession(_)) => {}
            other => panic!("expected NoSession, got {other:?}"),
        }
    }

    #[test]
    fn try_diagnose_with_session_writes_report() {
        let tmp = tempfile::tempdir().unwrap();
        let diag = tmp.path().join(".ralph/diagnostics");
        let session = diag.join("2026-06-05T10-20-30");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("recovery.jsonl"),
            "{\"schema_version\":1,\"envelope\":{\"schema_version\":1,\"diagnosis_id\":\"d1\",\"iteration\":1,\"source\":\"missing_event_gate\",\"severity\":\"error\",\"reason_code\":\"r\",\"message\":\"m\",\"retry_key\":\"k:1:r:*\",\"retry_attempt\":0,\"safe_target\":true,\"outcome\":\"pending\",\"timestamp\":\"2026-06-05T10:20:30Z\"},\"iteration\":1,\"timestamp\":\"2026-06-05T10:20:30Z\"}\n",
        )
        .unwrap();
        let args = base_args(&diag);
        try_diagnose(ColorMode::Never, args).expect("try_diagnose should succeed");
    }

    #[test]
    fn try_diagnose_with_output_writes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let diag = tmp.path().join(".ralph/diagnostics");
        let session = diag.join("2026-06-05T10-20-30");
        std::fs::create_dir_all(&session).unwrap();
        let out = tmp.path().join("report.md");
        let args = DiagnoseArgs {
            session: "latest".to_string(),
            format: DiagnoseFormat::Markdown,
            output: Some(out.clone()),
            diagnostics_root: Some(diag),
            source: None,
        };
        try_diagnose(ColorMode::Never, args).expect("try_diagnose should succeed");
        assert!(out.exists(), "output file should be created");
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(content.contains("# Ralph Diagnose Report"));
    }

    #[test]
    fn try_diagnose_with_json_format_does_not_emit_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let diag = tmp.path().join(".ralph/diagnostics");
        let session = diag.join("2026-06-05T10-20-30");
        std::fs::create_dir_all(&session).unwrap();
        let out = tmp.path().join("report.json");
        let args = DiagnoseArgs {
            session: "latest".to_string(),
            format: DiagnoseFormat::Json,
            output: Some(out.clone()),
            diagnostics_root: Some(diag),
            source: None,
        };
        try_diagnose(ColorMode::Never, args).expect("try_diagnose should succeed");
        let content = std::fs::read_to_string(&out).unwrap();
        // No markdown headings in the JSON output.
        assert!(!content.contains("## "));
        // JSON must carry the schema_version field.
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(value["schema_version"], "1");
    }

    #[test]
    fn try_diagnose_with_invalid_session_returns_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let diag = tmp.path().join(".ralph/diagnostics");
        std::fs::create_dir_all(&diag).unwrap();
        let args = DiagnoseArgs {
            session: "definitely-not-a-timestamp".to_string(),
            format: DiagnoseFormat::Markdown,
            output: None,
            diagnostics_root: Some(diag),
            source: None,
        };
        let result = try_diagnose(ColorMode::Never, args);
        assert!(matches!(result, Err(DiagnoseExit::InvalidSession(_))));
    }

    #[test]
    fn exit_codes_match_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let diag = tmp.path().join(".ralph/diagnostics");
        // No session under it.
        std::fs::create_dir_all(&diag).unwrap();
        let args = base_args(&diag);
        match try_diagnose(ColorMode::Never, args) {
            Err(exit) => assert_eq!(exit.code(), EXIT_NO_SESSION),
            Ok(()) => panic!("expected error"),
        }
    }

    // ── D7: --source filter ────────────────────────────────────────

    #[test]
    fn is_known_source_name_accepts_all_ten_variants() {
        for name in [
            "stall_recovery",
            "missing_event_gate",
            "workflow_guard",
            "execution_contract",
            "payload_contract",
            "drift_monitor",
            "hook_retry",
            "loop_stale",
            "topic_format",
            "agent_doc_sync",
        ] {
            assert!(is_known_source_name(name), "expected {name} known");
        }
    }

    #[test]
    fn is_known_source_name_rejects_unknown() {
        assert!(!is_known_source_name("nope"));
        assert!(!is_known_source_name(""));
        assert!(!is_known_source_name("AgentDocSync"));
        assert!(!is_known_source_name("agent_doc_syncs")); // trailing char
    }

    #[test]
    fn filter_report_keeps_only_matching_source() {
        // Build a minimal Report with two findings on different
        // sources; the filter must keep only the matching one.
        use ralph_core::diagnosis::{DiagnosisOutcome, DiagnosisSeverity};
        let report = Report {
            schema_version: "1",
            session_path: PathBuf::from("/tmp/sess"),
            summary: None,
            top_findings: vec![
                RankedFinding {
                    retry_key: "agent_doc_sync:executor:work.done:startup_timeout:*".to_string(),
                    severity: DiagnosisSeverity::Error,
                    outcome: DiagnosisOutcome::Escalated,
                    source: "agent_doc_sync".to_string(),
                    target_hat: Some("executor".to_string()),
                    topic: Some("work.done".to_string()),
                    reason_code: "startup_timeout".to_string(),
                    message: "m".to_string(),
                    occurrences: 1,
                    first_iteration: 1,
                    last_iteration: 1,
                    evidence: vec![],
                    safe_target: false,
                    escalated: true,
                },
                RankedFinding {
                    retry_key: "payload_contract:reviewer:work.done:missing_field:*".to_string(),
                    severity: DiagnosisSeverity::Warning,
                    outcome: DiagnosisOutcome::Repeated,
                    source: "payload_contract".to_string(),
                    target_hat: Some("reviewer".to_string()),
                    topic: Some("work.done".to_string()),
                    reason_code: "missing_field".to_string(),
                    message: "m".to_string(),
                    occurrences: 1,
                    first_iteration: 2,
                    last_iteration: 2,
                    evidence: vec![],
                    safe_target: false,
                    escalated: false,
                },
            ],
            recovery_timeline: vec![],
            drift_findings: vec![],
            orchestration: vec![],
            errors: vec![],
            warnings: vec![],
            active_activations: vec![],
        };
        let filtered = filter_report_by_source(report, "agent_doc_sync").unwrap();
        assert_eq!(filtered.top_findings.len(), 1);
        assert_eq!(filtered.top_findings[0].source, "agent_doc_sync");
    }

    #[test]
    fn filter_report_rejects_unknown_source() {
        let report = Report {
            schema_version: "1",
            session_path: PathBuf::from("/tmp/sess"),
            summary: None,
            top_findings: vec![],
            recovery_timeline: vec![],
            drift_findings: vec![],
            orchestration: vec![],
            errors: vec![],
            warnings: vec![],
            active_activations: vec![],
        };
        assert!(filter_report_by_source(report, "no_such_source").is_err());
    }

    #[test]
    fn resolve_diagnostics_root_via_loops_falls_back_when_no_loops() {
        let tmp = tempfile::tempdir().unwrap();
        let root = resolve_diagnostics_root_via_loops(tmp.path());
        assert_eq!(root, tmp.path().join(".ralph").join("diagnostics"));
    }

    #[test]
    fn resolve_diagnostics_root_via_loops_uses_workspace_from_active_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir_all(ws.join(".ralph").join("diagnostics")).unwrap();
        // Inject a minimal loops.json with an active loop entry
        // pointing at our workspace. Must use the current PID so
        // the stale-cleanup in LoopRegistry::list() does not evict it.
        let loops_dir = tmp.path().join(".ralph");
        std::fs::create_dir_all(&loops_dir).unwrap();
        let entry = serde_json::json!([{
            "id": "loop-1747016430-a1b2",
            "pid": std::process::id(),
            "started": "2026-06-05T10:20:30Z",
            "prompt": "test prompt",
            "workspace": ws.to_string_lossy(),
        }]);
        std::fs::write(loops_dir.join("loops.json"), serde_json::to_string(&entry).unwrap())
            .unwrap();
        let root = resolve_diagnostics_root_via_loops(tmp.path());
        assert_eq!(root, ws.join(".ralph").join("diagnostics"));
    }

    #[test]
    fn resolve_diagnostics_root_ignores_dead_loop_with_newer_started() {
        let tmp = tempfile::tempdir().unwrap();
        let live_ws = tmp.path().join("live-workspace");
        std::fs::create_dir_all(live_ws.join(".ralph").join("diagnostics")).unwrap();
        let loops_dir = tmp.path().join(".ralph");
        std::fs::create_dir_all(&loops_dir).unwrap();
        let entry = serde_json::json!([
            {
                "id": "loop-dead-newer",
                "pid": 1,
                "started": "2026-06-05T12:00:00Z",
                "prompt": "dead",
                "workspace": "/tmp/dead-workspace",
            },
            {
                "id": "loop-live-older",
                "pid": std::process::id(),
                "started": "2026-06-05T10:00:00Z",
                "prompt": "live",
                "workspace": live_ws.to_string_lossy(),
            }
        ]);
        std::fs::write(loops_dir.join("loops.json"), serde_json::to_string(&entry).unwrap())
            .unwrap();

        let resolution = resolve_diagnostics_root_with_warnings(tmp.path());
        assert_eq!(
            resolution.diagnostics_root,
            live_ws.join(".ralph").join("diagnostics")
        );
        assert_eq!(resolution.selected_loop_id.as_deref(), Some("loop-live-older"));
        assert!(
            resolution
                .warnings
                .iter()
                .any(|w| w.contains("dead") && w.contains("ignoring")),
            "expected dead-loop warning, got: {:?}",
            resolution.warnings
        );
    }

    #[test]
    fn resolve_diagnostics_root_warns_when_current_loop_id_points_at_stale_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let live_ws = tmp.path().join("worktree");
        std::fs::create_dir_all(live_ws.join(".ralph").join("diagnostics")).unwrap();
        let loops_dir = tmp.path().join(".ralph");
        std::fs::create_dir_all(&loops_dir).unwrap();
        std::fs::write(loops_dir.join("current-loop-id"), "loop-primary-dead").unwrap();
        let entry = serde_json::json!([
            {
                "id": "loop-primary-dead",
                "pid": 1,
                "started": "2026-06-05T09:00:00Z",
                "prompt": "primary dead",
                "workspace": tmp.path().to_string_lossy(),
            },
            {
                "id": "loop-worktree-live",
                "pid": std::process::id(),
                "started": "2026-06-05T11:00:00Z",
                "prompt": "worktree live",
                "workspace": live_ws.to_string_lossy(),
                "worktree_path": live_ws.to_string_lossy(),
            }
        ]);
        std::fs::write(loops_dir.join("loops.json"), serde_json::to_string(&entry).unwrap())
            .unwrap();

        let resolution = resolve_diagnostics_root_with_warnings(tmp.path());
        assert_eq!(
            resolution.diagnostics_root,
            live_ws.join(".ralph").join("diagnostics")
        );
        assert!(
            resolution.warnings.iter().any(|w| {
                w.contains("current-loop-id")
                    && w.contains("loop-primary-dead")
                    && w.contains("loop-worktree-live")
            }),
            "expected stale current-loop-id warning, got: {:?}",
            resolution.warnings
        );
    }
}
