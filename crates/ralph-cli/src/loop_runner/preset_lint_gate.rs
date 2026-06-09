//! Preset static lint hard gate for `ralph run`.
//!
//! Runs BEFORE any backend is spawned. In strict mode (always on for
//! `ralph run`), any lint error is fatal: the backend must NOT be
//! started, and the process exits with code 2.
//!
//! Requirements: R7 (run hard gate), R8 (JSON artifact + human output),
//! R9 (read-only, no auto-migration).

use super::*;
use ralph_core::preset_lint::{LintStrictness, run_preset_lint};
use ralph_core::runtime_contract::FindingSeverity;

/// Exit code for preset lint gate failure (R7).
pub const EXIT_CODE_LINT_GATE: i32 = 2;

/// Typed error for preset lint gate failure.
///
/// Carries the report so callers can render it in multiple formats
/// (human stderr, JSON artifact) without re-running the lint.
#[derive(Debug)]
pub struct PresetLintGateError {
    /// The full lint report with all findings.
    pub findings: Vec<ralph_core::runtime_contract::RuntimeContractFinding>,
    /// Number of error-severity findings.
    pub error_count: usize,
    /// Number of warning-severity findings.
    pub warning_count: usize,
}

impl std::fmt::Display for PresetLintGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "Preset lint gate failed ({} error(s), {} warning(s)):",
            self.error_count, self.warning_count
        )?;
        for finding in &self.findings {
            if finding.severity == FindingSeverity::Error {
                writeln!(f, "  [err] {} — {}", finding.id, finding.message)?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for PresetLintGateError {}

/// Run the preset static lint gate in strict mode.
///
/// This is the single entry point called by `run_loop_impl` before any
/// backend is spawned. Returns `Ok(())` when no lint errors are found,
/// or `Err(PresetLintGateError)` when errors exist.
///
/// Warnings are surfaced on stderr but do NOT cause failure (R7: only
/// errors are fatal in the run gate).
pub fn enforce_preset_lint_gate(
    config: &ralph_core::RalphConfig,
) -> Result<(), PresetLintGateError> {
    let lint_strictness = LintStrictness::Strict;
    let findings = run_preset_lint(config, lint_strictness);

    let error_count = findings
        .iter()
        .filter(|f| f.severity == FindingSeverity::Error)
        .count();
    let warning_count = findings
        .iter()
        .filter(|f| f.severity == FindingSeverity::Warn)
        .count();

    // Surface warnings on stderr (non-fatal).
    for finding in &findings {
        if finding.severity == FindingSeverity::Warn {
            eprintln!("[preset-lint] warning: {}", finding.message);
        }
    }

    if error_count == 0 {
        return Ok(());
    }

    Err(PresetLintGateError {
        findings,
        error_count,
        warning_count,
    })
}

/// Write the lint gate failure as a JSON artifact to
/// `.ralph/diagnostics/preset-lint-error-{timestamp}.json` (R8).
///
/// Uses atomic write (tempfile + rename) to prevent partial reads.
/// If the write fails, a fallback message is printed to stderr.
/// The main exit code (2) is always preserved regardless of artifact
/// write success.
pub fn write_preset_lint_artifact(
    diagnostics_dir: &std::path::Path,
    error: &PresetLintGateError,
) -> std::path::PathBuf {
    use std::io::Write as _;

    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%.3f").to_string();
    let path = diagnostics_dir.join(format!("preset-lint-error-{}.json", stamp));

    #[derive(serde::Serialize)]
    struct LintArtifact<'a> {
        error_type: &'static str,
        timestamp: String,
        error_count: usize,
        warning_count: usize,
        findings: &'a [ralph_core::runtime_contract::RuntimeContractFinding],
    }

    let artifact = LintArtifact {
        error_type: "preset_lint_gate_failure",
        timestamp: stamp.clone(),
        error_count: error.error_count,
        warning_count: error.warning_count,
        findings: &error.findings,
    };

    let body = match serde_json::to_string_pretty(&artifact) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[preset-lint] failed to serialize artifact: {}", e);
            return path;
        }
    };

    if let Err(e) = std::fs::create_dir_all(diagnostics_dir) {
        eprintln!(
            "[preset-lint] failed to create diagnostics dir {}: {}",
            diagnostics_dir.display(),
            e
        );
        print_lint_fallback(error);
        return path;
    }

    // Atomic write: tempfile + rename.
    match tempfile::Builder::new()
        .prefix(".preset-lint-")
        .suffix(".json")
        .tempfile_in(diagnostics_dir)
    {
        Ok(temp) => {
            let temp_path = temp.path().to_path_buf();
            if let Err(e) = temp.as_file().write_all(body.as_bytes()) {
                eprintln!(
                    "[preset-lint] failed to write artifact to {}: {}",
                    temp_path.display(),
                    e
                );
                print_lint_fallback(error);
                return path;
            }
            if let Err(e) = temp.as_file().sync_all() {
                eprintln!("[preset-lint] failed to sync artifact: {}", e);
                print_lint_fallback(error);
                return path;
            }
            if let Err(e) = temp.persist(&path) {
                eprintln!(
                    "[preset-lint] failed to persist artifact to {}: {}",
                    path.display(),
                    e.error
                );
                print_lint_fallback(error);
                return path;
            }
        }
        Err(e) => {
            eprintln!(
                "[preset-lint] failed to create temp file in {}: {}",
                diagnostics_dir.display(),
                e
            );
            print_lint_fallback(error);
            return path;
        }
    }

    eprintln!(
        "[PRESET LINT GATE] Loop blocked. Diagnostic written to {}",
        path.display()
    );
    path
}

/// Fallback stderr output when the JSON artifact cannot be written.
fn print_lint_fallback(error: &PresetLintGateError) {
    eprintln!(
        "[PRESET LINT GATE] {} error(s), {} warning(s):",
        error.error_count, error.warning_count
    );
    for finding in &error.findings {
        if finding.severity == FindingSeverity::Error {
            eprintln!("  [err] {} — {}", finding.id, finding.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::RalphConfig;

    #[test]
    fn gate_passes_on_clean_config() {
        let yaml = r#"
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["loop.complete"]
event_loop:
  starting_event: "work.start"
  completion_promise: "loop.complete"
tasks:
  enabled: false
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = enforce_preset_lint_gate(&config);
        assert!(
            result.is_ok(),
            "clean config must pass lint gate: {:?}",
            result
        );
    }

    #[test]
    fn gate_fails_on_strict_errors() {
        // owner_unknown_hat is always Error even in Default mode.
        let yaml = r#"
topic_owners:
  work.ready: ["nonexistent_hat"]
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["loop.complete"]
event_loop:
  starting_event: "work.start"
  completion_promise: "loop.complete"
tasks:
  enabled: false
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let result = enforce_preset_lint_gate(&config);
        assert!(result.is_err(), "config with unknown owner hat must fail");
        let err = result.unwrap_err();
        assert!(err.error_count > 0);
        assert!(
            err.findings
                .iter()
                .any(|f| f.id.contains("owner_unknown_hat"))
        );
    }

    #[test]
    fn exit_code_is_2() {
        assert_eq!(EXIT_CODE_LINT_GATE, 2);
    }

    #[test]
    fn artifact_writes_to_diagnostics_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let error = PresetLintGateError {
            findings: vec![],
            error_count: 1,
            warning_count: 0,
        };
        let path = write_preset_lint_artifact(tmp.path(), &error);
        assert!(
            path.exists(),
            "artifact file must exist: {}",
            path.display()
        );
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["error_type"], "preset_lint_gate_failure");
        assert_eq!(parsed["error_count"], 1);
    }

    #[test]
    fn display_impl_shows_errors() {
        let error = PresetLintGateError {
            findings: vec![],
            error_count: 2,
            warning_count: 1,
        };
        let msg = format!("{}", error);
        assert!(msg.contains("2 error(s)"));
        assert!(msg.contains("1 warning(s)"));
    }
}
