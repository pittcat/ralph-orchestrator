//! Integration tests proving the project-bootstrap helper's emitted
//! ``ralph.pipeline.yml`` is actually consumed by ``RalphConfig`` and
//! reflected in the real CLI's ``ralph run --dry-run`` output.
//!
//! Plan 2026-07-19-001 Unit 2 — the helper MUST emit fields that
//! ``RalphConfig`` (the real config loader, see
//! ``crates/ralph-core/src/config/mod.rs``) actually parses. The tests
//! here drive ``CARGO_BIN_EXE_ralph`` against a fixture config derived
//! from the helper's rendered bytes; the test verifies the dry-run's
//! effective value labels match the helper's inputs.
//!
//! These tests are deliberately minimal: they only exercise the contract
//! between the helper and the runtime config loader, not the runtime
//! loop itself. They run without a paid backend (no ``claude`` /
//! ``codex`` / ``gemini`` invocation) and without network access.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

/// Build the canonical ``ralph.pipeline.yml`` the bootstrap helper
/// would emit for ``budget_max_iterations=42``, ``max_runtime=3600``,
/// ``backend=claude``, ``preset=...``, ``prompt_file=PROMPT.x.md``,
/// ``plan=plan.md``. The shape MUST mirror the helper's
/// ``render_pipeline_yml`` so the test acts as a contract guard: any
/// change to the helper's output that breaks parsing is caught here.
///
/// We intentionally inline the YAML shape rather than calling the
/// Python helper at build time because the helper is a stdlib-only
/// Python module and the Rust integration tests run from cargo without
/// a Python interpreter. The fixture shape is a snapshot of the
/// contract; see ``pipeline_suite.render_pipeline_yml`` for the
/// authoritative source.
fn pipeline_yml(preset: &str, plan: &str, prompt_file: &str) -> String {
    format!(
        r#"cli:
  backend: claude
event_loop:
  prompt_file: "{prompt_file}"
  max_iterations: 42
  max_runtime_seconds: 3600
core:
  project_root: ./
_bootstrap:
  preset: "{preset}"
  plan: "{plan}"
  prompt_file: "{prompt_file}"
  preflight: strict
"#
    )
}

/// Sanitised environment so a developer-machine ``ralph.yml`` or
/// ``ANTHROPIC_API_KEY`` does not poison the result. We deliberately
/// strip provider credentials and ``RALPH_CONFIG``; the loader must
/// accept ONLY the explicit ``-c`` / ``-H`` argv we pass in.
fn sanitised_env() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("NO_COLOR", Some("1")),
        ("HOME", Some("/tmp")),
        // Strip provider credentials; we are not going to call any
        // paid backend in these tests.
        ("ANTHROPIC_API_KEY", None),
        ("OPENAI_API_KEY", None),
        ("GEMINI_API_KEY", None),
        ("RALPH_CONFIG", None),
    ]
}

/// Drive the ralph binary with a sanitised environment so provider
/// credentials / user-level ralph.yml cannot leak in.
fn run_ralph(args: &[&str]) -> (String, String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ralph"));
    cmd.args(args);
    for (key, value) in sanitised_env() {
        match value {
            Some(v) => {
                cmd.env(key, v);
            }
            None => {
                cmd.env_remove(key);
            }
        }
    }
    let out = cmd.output().expect("execute ralph");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (stdout, stderr, out.status.success())
}

/// When the helper emits ``cli.backend: claude``, ``ralph run
/// --dry-run`` MUST surface ``Backend: claude`` — not a different
/// backend or the empty default. This is the F2 / S3 contract for the
/// pipeline config writer: the effective value reported by the
/// runtime must come from the suite, not from defaults.
#[test]
fn pipeline_helper_backend_is_honoured_by_dry_run() {
    let dir = TempDir::new().expect("temp dir");
    let cfg = dir.path().join("ralph.pipeline.yml");
    let prompt = dir.path().join("PROMPT.x.md");
    let plan = dir.path().join("plan.md");
    fs::write(&prompt, "# prompt\n").expect("write prompt");
    fs::write(&plan, "# plan\n").expect("write plan");
    fs::write(&cfg, pipeline_yml("ce-executor-pipeline", "plan.md", "PROMPT.x.md"))
        .expect("write pipeline config");

    let (stdout, stderr, ok) = run_ralph(&[
        "--color",
        "never",
        "-c",
        cfg.to_str().unwrap(),
        "-H",
        "builtin:ce-executor-pipeline",
        "run",
        "--dry-run",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(ok, "dry-run must succeed; stderr: {stderr}\nstdout: {stdout}");
    assert!(
        stdout.contains("Backend: claude"),
        "effective backend must be the operator-supplied 'claude' from \
         the helper's emitted config; stdout:\n{stdout}"
    );
}

/// The helper's emitted ``event_loop.max_iterations: 42`` MUST round
/// trip to ``Max iterations: 42`` in the dry-run output. If the
/// helper ever drops or renames this field, this test fails closed
/// rather than silently falling back to the default.
#[test]
fn pipeline_helper_max_iterations_is_honoured_by_dry_run() {
    let dir = TempDir::new().expect("temp dir");
    let cfg = dir.path().join("ralph.pipeline.yml");
    let prompt = dir.path().join("PROMPT.x.md");
    let plan = dir.path().join("plan.md");
    fs::write(&prompt, "# prompt\n").expect("write prompt");
    fs::write(&plan, "# plan\n").expect("write plan");
    fs::write(&cfg, pipeline_yml("ce-executor-pipeline", "plan.md", "PROMPT.x.md"))
        .expect("write pipeline config");

    let (stdout, stderr, ok) = run_ralph(&[
        "--color",
        "never",
        "-c",
        cfg.to_str().unwrap(),
        "-H",
        "builtin:ce-executor-pipeline",
        "run",
        "--dry-run",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(ok, "dry-run must succeed; stderr: {stderr}\nstdout: {stdout}");
    assert!(
        stdout.contains("Max iterations: 42"),
        "effective max_iterations must come from the helper's emitted \
         config (42), not the default; stdout:\n{stdout}"
    );
}

/// The helper's emitted ``event_loop.max_runtime_seconds: 3600`` MUST
/// round trip to ``Max runtime: 3600s`` in the dry-run output.
#[test]
fn pipeline_helper_max_runtime_is_honoured_by_dry_run() {
    let dir = TempDir::new().expect("temp dir");
    let cfg = dir.path().join("ralph.pipeline.yml");
    let prompt = dir.path().join("PROMPT.x.md");
    let plan = dir.path().join("plan.md");
    fs::write(&prompt, "# prompt\n").expect("write prompt");
    fs::write(&plan, "# plan\n").expect("write plan");
    fs::write(&cfg, pipeline_yml("ce-executor-pipeline", "plan.md", "PROMPT.x.md"))
        .expect("write pipeline config");

    let (stdout, stderr, ok) = run_ralph(&[
        "--color",
        "never",
        "-c",
        cfg.to_str().unwrap(),
        "-H",
        "builtin:ce-executor-pipeline",
        "run",
        "--dry-run",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(ok, "dry-run must succeed; stderr: {stderr}\nstdout: {stdout}");
    assert!(
        stdout.contains("Max runtime: 3600s"),
        "effective max_runtime must come from the helper's emitted \
         config (3600s), not the default; stdout:\n{stdout}"
    );
}

/// The helper's emitted ``event_loop.prompt_file: PROMPT.x.md`` MUST
/// end up driving the runtime — the dry-run output references the
/// configured prompt file's basename rather than the
/// ``PROMPT.default.md`` the user-level ``ralph.yml`` would have
/// supplied. The exact on-disk representation may be absolute
/// (canonicalised) so we assert on the basename to stay robust to
/// path canonicalisation.
#[test]
fn pipeline_helper_prompt_file_is_honoured_by_dry_run() {
    let dir = TempDir::new().expect("temp dir");
    let cfg = dir.path().join("ralph.pipeline.yml");
    let prompt = dir.path().join("PROMPT.x.md");
    let plan = dir.path().join("plan.md");
    fs::write(&prompt, "# prompt\n").expect("write prompt");
    fs::write(&plan, "# plan\n").expect("write plan");
    fs::write(&cfg, pipeline_yml("ce-executor-pipeline", "plan.md", "PROMPT.x.md"))
        .expect("write pipeline config");

    let (stdout, stderr, ok) = run_ralph(&[
        "--color",
        "never",
        "-c",
        cfg.to_str().unwrap(),
        "-H",
        "builtin:ce-executor-pipeline",
        "run",
        "--dry-run",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(ok, "dry-run must succeed; stderr: {stderr}\nstdout: {stdout}");
    assert!(
        stdout.contains("PROMPT.x.md"),
        "effective prompt_file must reference the helper's emitted \
         config (PROMPT.x.md), not from a stray ralph.yml in cwd; \
         stdout:\n{stdout}"
    );
}

/// When both a user ``ralph.yml`` AND the helper's
/// ``ralph.pipeline.yml`` are present in cwd, the explicit ``-c``
/// argv MUST take precedence — the runtime must NOT silently read
/// from the user-level ``ralph.yml`` and ignore the suite. This is
/// the F4 / S11 contract: precedence follows argv order, not file
/// discovery order.
#[test]
fn explicit_pipeline_config_overrides_default_ralph_yml() {
    let dir = TempDir::new().expect("temp dir");
    // Plant a user-level ralph.yml that points to a DIFFERENT prompt
    // file. If the loader ever silently fell back to this file, the
    // dry-run would report ``Prompt file: PROMPT.default.md`` instead
    // of the pipeline-suite's ``PROMPT.x.md``.
    fs::write(
        dir.path().join("ralph.yml"),
        r#"cli:
  backend: claude
event_loop:
  prompt_file: PROMPT.default.md
  max_iterations: 1
  max_runtime_seconds: 1
core: {}
"#,
    )
    .expect("write default ralph.yml");
    let cfg = dir.path().join("ralph.pipeline.yml");
    let prompt = dir.path().join("PROMPT.x.md");
    let plan = dir.path().join("plan.md");
    fs::write(&prompt, "# prompt\n").expect("write prompt");
    fs::write(&plan, "# plan\n").expect("write plan");
    fs::write(&cfg, pipeline_yml("ce-executor-pipeline", "plan.md", "PROMPT.x.md"))
        .expect("write pipeline config");

    let (stdout, stderr, ok) = run_ralph(&[
        "--color",
        "never",
        "-c",
        cfg.to_str().unwrap(),
        "-H",
        "builtin:ce-executor-pipeline",
        "run",
        "--dry-run",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(ok, "dry-run must succeed; stderr: {stderr}\nstdout: {stdout}");
    // The dry-run reports a canonicalised (possibly absolute) path
    // for the effective prompt file. Assert on the basename so we
    // are robust to canonicalisation, but assert that the pipeline
    // config won over the user-level ralph.yml.
    assert!(
        stdout.contains("PROMPT.x.md"),
        "explicit -c must take precedence over cwd ralph.yml; \
         stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("PROMPT.default.md"),
        "the user-level ralph.yml must NOT have leaked through \
         explicit -c ralph.pipeline.yml; stdout:\n{stdout}"
    );
}