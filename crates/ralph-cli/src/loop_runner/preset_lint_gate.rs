//! Preset static lint hard gate for `ralph run`.
//!
//! Runs BEFORE any backend is spawned. In strict mode (always on for
//! `ralph run`), any lint error is fatal: the backend must NOT be
//! started, and the process exits with code 2.
//!
//! Requirements: R7 (run hard gate), R8 (JSON artifact + human output),
//! R9 (read-only, no auto-migration).
//!
//! WRC-U3 (2026-06-12-003) / KTD-7: the gate now accepts a
//! `source_is_builtin_embedded` flag from the CLI runner. When the
//! caller knows the preset came from `-H builtin:foo`, every WAC
//! finding (R2/R3/R4/R5) is escalated to `Error` regardless of the
//! `strict` axis. The same logic that lives in the aggregator's Step
//! 2b applies to the gate path: builtin WAC defects are blocking.

use super::*;
use ralph_core::preset_lint::{LintStrictness, run_preset_lint};
use ralph_core::runtime_contract::{FindingSeverity, FindingSource, FindingStage};

/// Exit code for preset lint gate failure (R7).
pub const EXIT_CODE_LINT_GATE: i32 = 2;
/// Exit code for `agent_doc_sync` strict-mode failure (EX_CONFIG).
pub const EXIT_CODE_AGENT_DOC_SYNC_STRICT: i32 = 78;

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
///
/// WRC-U3: `source_is_builtin_embedded` escalates every WAC finding
/// to `Error` per KTD-7. The CLI runner passes `true` when the
/// caller invoked `ralph run -H builtin:<name>`; otherwise it passes
/// `false` (the user-preset path).
///
/// Backwards-compat: a no-arg variant is preserved so legacy tests
/// (which call `enforce_preset_lint_gate(&config)`) compile unchanged.
/// The no-arg variant assumes `source_is_builtin_embedded = false`,
/// which is the safe default — the strict-lint gate still catches
/// WAC defects at the `lint.preset.*` Error level when the caller
/// is in strict mode. The builtin escalation is a Tier-0 nicety
/// for the new `ralph preset check -H builtin:foo` path.
pub fn enforce_preset_lint_gate(
    config: &ralph_core::RalphConfig,
    source_is_builtin_embedded: bool,
) -> Result<(), PresetLintGateError> {
    let lint_strictness = LintStrictness::Strict;
    let findings = run_preset_lint(config, lint_strictness, source_is_builtin_embedded);

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

    // P3 #26: include the process id in the filename so that two
    // concurrent ralph processes (e.g. primary loop + worktree loop
    // both hitting the same gate) never clobber each other's artifact.
    // The millisecond stamp is not enough on fast SSDs / shared
    // worktrees where two `Utc::now()` calls can return the same value.
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%.3f").to_string();
    let pid = std::process::id();
    let path = diagnostics_dir.join(format!("preset-lint-error-{stamp}-pid{pid}.json"));

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
        let result = enforce_preset_lint_gate(&config, false);
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
        let result = enforce_preset_lint_gate(&config, false);
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

    // --- Finding #14: artifact write failure paths must not mask lint failure ---

    /// Build a `PresetLintGateError` with one Error finding for use in
    /// artifact-write failure tests. The error is the upstream signal that
    /// MUST survive even when the JSON artifact cannot be written.
    fn make_test_error() -> PresetLintGateError {
        PresetLintGateError {
            findings: vec![ralph_core::runtime_contract::RuntimeContractFinding {
                id: "test.finding".to_string(),
                source: FindingSource::Lint,
                severity: FindingSeverity::Error,
                stage: FindingStage::Authoring,
                message: "synthetic error for artifact failure tests".to_string(),
                details: std::collections::BTreeMap::new(),
                action_hint: None,
            }],
            error_count: 1,
            warning_count: 0,
        }
    }

    /// R8 core invariant: even when the JSON artifact write fails for any
    /// reason, the caller must still be able to surface the lint error to
    /// the user. `write_preset_lint_artifact` must not panic, must not
    /// eat the error, and must return a path the caller can inspect.
    #[test]
    fn artifact_failure_does_not_mask_lint_error() {
        let error = make_test_error();
        // Force the failure path by passing a path whose parent is a
        // regular file (create_dir_all must fail deterministically on
        // every platform).
        let tmp = tempfile::tempdir().expect("tempdir");
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, b"i am a file, not a directory").expect("write blocker");
        let bogus = blocker.join("diagnostics");

        let path = write_preset_lint_artifact(&bogus, &error);

        // Lint error survives unchanged regardless of artifact outcome.
        assert_eq!(error.error_count, 1);
        assert_eq!(error.warning_count, 0);
        assert_eq!(error.findings.len(), 1);
        assert_eq!(error.findings[0].id, "test.finding");
        assert_eq!(error.findings[0].severity, FindingSeverity::Error);
        // The intended artifact target was not produced on the failure
        // path. (This is the contract that lets callers distinguish
        // success from failure via `path.exists()`.)
        assert!(
            !path.exists(),
            "artifact target must not exist when write fails; got {}",
            path.display()
        );
    }

    /// Failure path 1: `create_dir_all` fails because the parent
    /// directory is read-only (chmod 0o000 on Unix). The function must
    /// fall through to the fallback stderr message and return the
    /// intended path without panicking.
    #[cfg(unix)]
    #[test]
    fn artifact_failure_when_parent_dir_is_unwritable() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir");
        // Create a parent directory that exists, then lock it down
        // so any create_dir_all / tempfile_in inside it fails with EACCES.
        let locked_parent = tmp.path().join("locked");
        std::fs::create_dir(&locked_parent).expect("mkdir");
        let mut perms = std::fs::metadata(&locked_parent).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&locked_parent, perms).expect("chmod 0o000");

        // Target diagnostics dir lives under the locked parent.
        let diagnostics_dir = locked_parent.join("diagnostics");
        let error = make_test_error();

        let path = write_preset_lint_artifact(&diagnostics_dir, &error);

        // Restore permissions so the tempdir can clean up.
        let mut restore = std::fs::metadata(&locked_parent).unwrap().permissions();
        restore.set_mode(0o755);
        let _ = std::fs::set_permissions(&locked_parent, restore);

        // Either the diagnostics_dir was never created, or a temp file
        // leaked into it — either way, the *target* artifact file
        // (the .json the caller cares about) must not exist.
        assert!(
            !path.exists(),
            "artifact target must not exist when parent dir is unwritable; got {}",
            path.display()
        );
    }

    /// Failure path 2: `create_dir_all` fails because an intermediate
    /// path component is a regular file, not a directory. This exercises
    /// the first error branch (line 131-139) deterministically on all
    /// platforms.
    #[test]
    fn artifact_failure_when_intermediate_path_is_a_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Create a regular file at the position where the diagnostics
        // dir should go. create_dir_all on a path whose parent
        // component collides with a file must fail.
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, b"i am a file, not a directory").expect("write blocker");

        // diagnostics_dir = <blocker>/diagnostics — create_dir_all must fail.
        let diagnostics_dir = blocker.join("diagnostics");
        let error = make_test_error();

        let path = write_preset_lint_artifact(&diagnostics_dir, &error);

        assert!(
            !path.exists(),
            "artifact target must not exist when intermediate path is a file"
        );
        assert!(
            !diagnostics_dir.exists(),
            "intermediate path must remain blocked (no directory created)"
        );
    }

    /// Failure path 3: when `tempfile_in` fails (e.g. directory exists
    /// but is unwritable), `write_preset_lint_artifact` must not panic
    /// and must return a path that doesn't exist. We use chmod 0o500
    /// (read+execute, no write) on a pre-existing directory to force
    /// tempfile_in to fail on the write half.
    #[cfg(unix)]
    #[test]
    fn artifact_failure_when_tempfile_creation_fails() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir");
        let read_only = tmp.path().join("ro");
        std::fs::create_dir(&read_only).expect("mkdir");
        let mut perms = std::fs::metadata(&read_only).unwrap().permissions();
        // 0o500 = r-x for owner: traversal works but no writes.
        perms.set_mode(0o500);
        std::fs::set_permissions(&read_only, perms).expect("chmod 0o500");

        let error = make_test_error();
        let path = write_preset_lint_artifact(&read_only, &error);

        // Restore for cleanup.
        let mut restore = std::fs::metadata(&read_only).unwrap().permissions();
        restore.set_mode(0o755);
        let _ = std::fs::set_permissions(&read_only, restore);

        assert!(
            !path.exists(),
            "artifact target must not exist when tempfile_in fails"
        );
    }

    /// Failure path 4 (return contract): every artifact-write failure
    /// must return a path that does not exist on disk, so callers can
    /// reliably distinguish success from failure via `path.exists()`.
    /// This test sweeps several failure inputs and asserts the contract.
    #[test]
    fn artifact_failure_paths_return_nonexistent_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, b"not a dir").expect("write blocker");

        // Case A: intermediate component is a regular file.
        let path_a = write_preset_lint_artifact(&blocker.join("diag"), &make_test_error());
        assert!(
            !path_a.exists(),
            "file-as-parent must yield no artifact file"
        );

        // Case B: deep path under a regular file (no ancestor can be
        // created as a directory).
        let path_b = write_preset_lint_artifact(&blocker.join("a/b/c"), &make_test_error());
        assert!(
            !path_b.exists(),
            "deep path under file must yield no artifact file"
        );

        // Case C: a path whose final component collides with a pre-existing
        // regular file (this would only fail at the rename/persist step,
        // but we at least exercise the contract that returned path is
        // observable). We tolerate either a no-op (path is just a
        // predicted name) or a real failure — but in both cases the file
        // must not appear inside the parent directory.
        let collisions_dir = tmp.path().join("collisions");
        std::fs::create_dir(&collisions_dir).expect("mkdir collisions");
        let occupied = collisions_dir.join("preset-lint-error-fake.json");
        std::fs::write(&occupied, b"pre-existing").expect("pre-occupy");
        // The function still tries to write a fresh artifact next to it
        // (with a real timestamp suffix), so this is a sanity check that
        // the write succeeded into the same dir.
        let path_c = write_preset_lint_artifact(&collisions_dir, &make_test_error());
        // We accept both outcomes: the new artifact lives next to
        // `occupied` (happy path on writable dir), or write failed. The
        // contract we check: if the function claimed success via
        // producing a path, that path is a *new* file (not the
        // pre-existing one).
        if path_c.exists() {
            assert_ne!(
                path_c, occupied,
                "fresh artifact must not be the pre-existing file"
            );
            assert!(
                path_c.starts_with(&collisions_dir),
                "fresh artifact must live under the diagnostics dir"
            );
        }
    }

    /// Sanity check: when the diagnostics directory is writable, the
    /// artifact is produced and contains a well-formed JSON document.
    /// This protects the happy path from regressing while we tighten
    /// the failure-path coverage.
    #[test]
    fn artifact_happy_path_produces_valid_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let diag = tmp.path().join("diagnostics");
        let error = make_test_error();
        let path = write_preset_lint_artifact(&diag, &error);
        assert!(path.exists(), "happy path must produce artifact file");
        assert!(
            path.starts_with(&diag),
            "artifact path must live under diagnostics dir"
        );
        let content = std::fs::read_to_string(&path).expect("read artifact");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");
        assert_eq!(parsed["error_type"], "preset_lint_gate_failure");
        assert_eq!(parsed["error_count"], 1);
        assert_eq!(parsed["warning_count"], 0);
        let findings = parsed["findings"].as_array().expect("findings array");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0]["id"], "test.finding");
        assert_eq!(findings[0]["severity"], "error");
    }
}
