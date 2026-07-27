//! Integration tests for `ralph preset materialize-artifacts`.
//!
//! Closes the binary-only install loop: templates are embedded at compile
//! time and must appear on disk when the CLI runs without a source checkout
//! of `presets/templates/`.
//!
//! Given/When/Then coverage (BDD-style behavior):
//! - Happy path default dest under `.ralph/forge/<plan-key>/templates/`
//! - `--dest` override
//! - `builtin:` preset prefix
//! - Unknown preset / empty plan-key / path-traversal plan-key fail closed
//! - Idempotent rematerialize preserves TDD/BDD template markers
//! - Help text documents the subcommand

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;
use tempfile::TempDir;

fn run_materialize(cwd: &Path, args: &[&str]) -> Output {
    common::ralph_bin()
        .args(["--color", "never", "preset", "materialize-artifacts"])
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to spawn ralph preset materialize-artifacts")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "expected success\nstdout:\n{}\nstderr:\n{}",
        stdout(out),
        stderr(out)
    );
}

fn assert_failure(out: &Output) {
    assert!(
        !out.status.success(),
        "expected failure\nstdout:\n{}\nstderr:\n{}",
        stdout(out),
        stderr(out)
    );
}

const REQUIRED_FILES: &[&str] = &[
    "development-plan.template.md",
    "unit.template.yml",
    "execution-plan.template.yml",
    "unit-completion.template.md",
    "manager-report.template.md",
    "README.md",
];

/// Given a temp workspace,
/// When materialize-artifacts runs with --plan-key,
/// Then six templates appear under `.ralph/forge/<key>/templates/` with BDD markers.
#[test]
fn happy_path_writes_default_forge_templates_dir() {
    let tmp = TempDir::new().unwrap();
    let out = run_materialize(tmp.path(), &["parallel-forge", "--plan-key", "demo-plan"]);
    assert_success(&out);

    let templates = tmp
        .path()
        .join(".ralph/forge/demo-plan/templates");
    for name in REQUIRED_FILES {
        assert!(
            templates.join(name).is_file(),
            "missing {name} under {}",
            templates.display()
        );
    }

    let plan = fs::read_to_string(templates.join("development-plan.template.md")).unwrap();
    assert!(plan.contains("## 3. BDD 行为规格"));
    assert!(plan.contains("TDD 最小行为拆分"));

    let unit = fs::read_to_string(templates.join("unit.template.yml")).unwrap();
    assert!(unit.contains("acceptance_test:"));
    assert!(unit.contains("tdd:"));

    assert!(
        stdout(&out).contains("Wrote 6 artifact template"),
        "stdout should report count: {}",
        stdout(&out)
    );
}

/// Given --dest,
/// When materialize runs,
/// Then files land only under dest (not under default forge path).
#[test]
fn dest_override_writes_only_to_dest() {
    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("custom-templates");
    let out = run_materialize(
        tmp.path(),
        &[
            "parallel-forge",
            "--plan-key",
            "ignored-for-layout",
            "--dest",
            dest.to_str().unwrap(),
        ],
    );
    assert_success(&out);
    assert!(dest.join("manager-report.template.md").is_file());
    assert!(!tmp.path().join(".ralph/forge").exists());
}

/// Given builtin: prefix,
/// When materialize runs,
/// Then it resolves the same embedded catalog as bare name.
#[test]
fn accepts_builtin_prefix() {
    let tmp = TempDir::new().unwrap();
    let out = run_materialize(
        tmp.path(),
        &["builtin:parallel-forge", "--plan-key", "pfx"],
    );
    assert_success(&out);
    assert!(tmp
        .path()
        .join(".ralph/forge/pfx/templates/README.md")
        .is_file());
}

/// Given an unknown preset,
/// When materialize runs,
/// Then exit non-zero and write nothing under .ralph/forge.
#[test]
fn unknown_preset_fails_closed() {
    let tmp = TempDir::new().unwrap();
    let out = run_materialize(tmp.path(), &["nope-preset", "--plan-key", "x"]);
    assert_failure(&out);
    let combined = format!("{}{}", stdout(&out), stderr(&out));
    assert!(
        combined.contains("no embedded artifact templates")
            || combined.contains("nope-preset"),
        "unexpected error text: {combined}"
    );
    assert!(!tmp.path().join(".ralph").exists());
}

/// Given empty --plan-key,
/// When materialize runs,
/// Then exit non-zero.
#[test]
fn empty_plan_key_fails() {
    let tmp = TempDir::new().unwrap();
    let out = run_materialize(tmp.path(), &["parallel-forge", "--plan-key", ""]);
    assert_failure(&out);
}

/// Given a plan-key with path separators,
/// When materialize runs,
/// Then exit non-zero (no directory traversal).
#[test]
fn plan_key_path_traversal_fails() {
    let tmp = TempDir::new().unwrap();
    let out = run_materialize(tmp.path(), &["parallel-forge", "--plan-key", "../escape"]);
    assert_failure(&out);
    let combined = format!("{}{}", stdout(&out), stderr(&out));
    assert!(
        combined.contains("path segment") || combined.contains("plan-key"),
        "unexpected error: {combined}"
    );
}

/// Given an existing templates dir,
/// When materialize runs again,
/// Then content is overwritten and TDD/BDD markers remain (idempotent).
#[test]
fn rematerialize_is_idempotent_and_keeps_bdd_tdd_markers() {
    let tmp = TempDir::new().unwrap();
    let args = ["parallel-forge", "--plan-key", "loop"];
    assert_success(&run_materialize(tmp.path(), &args));

    let target = tmp
        .path()
        .join(".ralph/forge/loop/templates/development-plan.template.md");
    fs::write(&target, "CORRUPTED").unwrap();

    assert_success(&run_materialize(tmp.path(), &args));
    let restored = fs::read_to_string(&target).unwrap();
    assert!(restored.contains("## 3. BDD 行为规格"));
    assert!(!restored.contains("CORRUPTED"));
}

/// Given agent-context env pollution (hat inheritance),
/// When human-CLI materialize runs via scrubbed ralph_bin,
/// Then it still succeeds (HARD RULE 5).
#[test]
fn succeeds_under_polluted_agent_env_when_scrubbed() {
    let tmp = TempDir::new().unwrap();
    let mut cmd = common::ralph_bin();
    // Simulate outer hat pollution, then scrub again (as tests must).
    cmd.env("RALPH_CURRENT_HAT", "planner");
    cmd.env("RALPH_EVENTS_FILE", "/tmp/x.jsonl");
    cmd.env("RALPH_CURRENT_LOOP_ID", "loop-x");
    common::scrub_agent_runtime_env(&mut cmd);
    let out = cmd
        .args([
            "--color",
            "never",
            "preset",
            "materialize-artifacts",
            "parallel-forge",
            "--plan-key",
            "polluted",
        ])
        .current_dir(tmp.path())
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert_success(&out);
    assert!(tmp
        .path()
        .join(".ralph/forge/polluted/templates/unit.template.yml")
        .is_file());
}

#[test]
fn help_lists_materialize_artifacts() {
    let out = common::ralph_bin()
        .args(["--color", "never", "preset", "--help"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains("materialize-artifacts"),
        "preset --help missing materialize-artifacts:\n{text}"
    );
}

#[test]
fn subcommand_help_documents_plan_key_and_dest() {
    let out = common::ralph_bin()
        .args([
            "--color",
            "never",
            "preset",
            "materialize-artifacts",
            "--help",
        ])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("--plan-key"));
    assert!(text.contains("--dest"));
    assert!(
        text.contains("binary") || text.contains("Embedded") || text.contains("embedded"),
        "help should mention embedded/binary templates:\n{text}"
    );
}
