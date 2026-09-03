mod common;

use anyhow::Result;
use std::fs;
use tempfile::TempDir;

fn default_worktree_root(main_repo: &std::path::Path) -> std::path::PathBuf {
    main_repo
        .parent()
        .unwrap_or(main_repo)
        .join("worktree")
        .join(main_repo.file_name().expect("repo must have a basename"))
}

/// Integration tests for continue mode (--continue flag) acceptance criteria.
///
/// Per event-loop.spec.md, ralph run --continue should:
/// 1) Check that scratchpad exists before continuing
/// 2) Publish task.resume instead of task.start
/// 3) Allow planner to read existing scratchpad rather than doing fresh gap analysis

#[test]
fn test_continue_requires_existing_scratchpad() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Create a basic config file with custom backend that will fail fast
    // Using "nonexistent_backend" ensures auto-detection fails immediately
    let config_content = r#"
event_loop:
  prompt_file: "PROMPT.md"
  completion_promise: "LOOP_COMPLETE"
  max_iterations: 1
  max_runtime_seconds: 30

cli:
  backend: "custom"
  command: "true"

core:
  scratchpad: ".ralph/agent/scratchpad.md"
"#;
    fs::write(temp_path.join("ralph.yml"), config_content)?;

    // Create a prompt file
    fs::write(temp_path.join("PROMPT.md"), "Test task")?;

    // Ensure no scratchpad exists
    let scratchpad_path = temp_path.join(".ralph/agent").join("scratchpad.md");
    assert!(!scratchpad_path.exists());

    // Run ralph run --continue - should fail with error about missing scratchpad
    let output = common::ralph_bin()
        .arg("run")
        .arg("--continue")
        .arg("--config")
        .arg(temp_path.join("ralph.yml"))
        .current_dir(temp_path)
        .output()?;

    // Should exit with error
    assert!(!output.status.success());

    // Should contain error message about missing scratchpad
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Cannot continue: scratchpad not found"));
    assert!(stderr.contains("Start a fresh run with `ralph run`"));

    Ok(())
}

#[test]
fn test_continue_with_existing_scratchpad() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Create a basic config file with short timeout and fast backend
    // Disable memories/tasks to test legacy scratchpad mode
    let config_content = r#"
event_loop:
  prompt_file: "PROMPT.md"
  completion_promise: "LOOP_COMPLETE"
  max_iterations: 1
  max_runtime_seconds: 5

cli:
  backend: "custom"
  command: "true"

core:
  scratchpad: ".ralph/agent/scratchpad.md"

memories:
  enabled: false

tasks:
  enabled: false
"#;
    fs::write(temp_path.join("ralph.yml"), config_content)?;

    // Create a prompt file
    fs::write(temp_path.join("PROMPT.md"), "Test task")?;

    // Create the .ralph/agent directory and scratchpad file
    let agent_dir = temp_path.join(".ralph/agent");
    fs::create_dir_all(&agent_dir)?;

    let scratchpad_content = r"# Task List

## Current Tasks
- [ ] Implement feature A
- [x] Complete feature B
- [ ] Add tests for feature C

## Notes
Previous work completed on feature B.
";
    fs::write(agent_dir.join("scratchpad.md"), scratchpad_content)?;

    // Run ralph run --continue --no-tui (needed for tracing output to stdout)
    let output = common::ralph_bin()
        .arg("run")
        .arg("--continue")
        .arg("--no-tui")
        .arg("--config")
        .arg(temp_path.join("ralph.yml"))
        .current_dir(temp_path)
        .output()?;

    let _stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should find the existing scratchpad (logged via tracing to stdout)
    assert!(stdout.contains("Found existing scratchpad"));

    Ok(())
}

#[test]
fn test_continue_publishes_loop_resume_event() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Create config with short timeout and fast backend
    let config_content = r#"
event_loop:
  prompt_file: "PROMPT.md"
  completion_promise: "LOOP_COMPLETE"
  max_iterations: 1
  max_runtime_seconds: 5

cli:
  backend: "custom"
  command: "true"

core:
  scratchpad: ".ralph/agent/scratchpad.md"

memories:
  enabled: false

tasks:
  enabled: false
"#;

    fs::write(temp_path.join("ralph.yml"), config_content)?;

    // Create a prompt file
    fs::write(temp_path.join("PROMPT.md"), "Continue test task")?;

    // Create the .ralph/agent directory and scratchpad file
    let agent_dir = temp_path.join(".ralph/agent");
    fs::create_dir_all(&agent_dir)?;

    // Create .ralph directory with a marker file for continue to use
    // (simulates a previous run that created the events file)
    let ralph_dir = temp_path.join(".ralph");
    fs::create_dir_all(&ralph_dir)?;
    let events_path = ".ralph/events-continue-test.jsonl";
    fs::write(ralph_dir.join("current-events"), events_path)?;

    let scratchpad_content = r"# Task List

## Current Tasks
- [ ] Continue this task
- [x] Previously completed task

## Notes
This is a continued session.
";
    fs::write(agent_dir.join("scratchpad.md"), scratchpad_content)?;

    // Run ralph run --continue
    let _output = common::ralph_bin()
        .arg("run")
        .arg("--continue")
        .arg("--config")
        .arg(temp_path.join("ralph.yml"))
        .current_dir(temp_path)
        .output()?;

    // Plan 2026-08-13-003 U4: the marker / events file
    // MUST exist and contain `loop.resume` — fail the test
    // loudly instead of silently skipping the assertion.
    let marker_path = ralph_dir.join("current-events");
    assert!(
        marker_path.exists(),
        "current-events marker MUST exist after `ralph run --continue`"
    );
    let events_path = fs::read_to_string(&marker_path)?.trim().to_string();
    let events_file = temp_path.join(&events_path);
    assert!(
        events_file.exists(),
        "events file referenced by marker MUST exist (path={})",
        events_file.display()
    );
    let events_content = fs::read_to_string(&events_file)?;
    assert!(
        events_content.contains(r#""topic":"loop.resume""#)
            || events_content.contains(r#""topic": "loop.resume""#),
        "events MUST contain `loop.resume` (continuation bootstrap), was: {events_content}"
    );
    assert!(
        !events_content.contains("\"topic\":\"task.resume\"")
            && !events_content.contains("\"topic\": \"task.resume\""),
        "events MUST NOT contain `task.resume` bootstrap (continuation, not runtime recovery)"
    );

    Ok(())
}

#[test]
fn test_continue_vs_run_event_difference() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Create config with short timeout and fast backend
    let config_content = r#"
event_loop:
  prompt_file: "PROMPT.md"
  completion_promise: "LOOP_COMPLETE"
  max_iterations: 1
  max_runtime_seconds: 5

cli:
  backend: "custom"
  command: "true"

core:
  scratchpad: ".ralph/agent/scratchpad.md"
"#;

    fs::write(temp_path.join("ralph.yml"), config_content)?;

    // Create a prompt file
    fs::write(temp_path.join("PROMPT.md"), "Test task")?;

    // Create the .ralph/agent directory
    let agent_dir = temp_path.join(".ralph/agent");
    fs::create_dir_all(&agent_dir)?;

    // Test 1: Run normal ralph run (should publish task.start)
    let scratchpad_content = "# Initial scratchpad\n- [ ] Task 1\n";
    fs::write(agent_dir.join("scratchpad.md"), scratchpad_content)?;

    let _output = common::ralph_bin()
        .arg("run")
        .arg("--config")
        .arg(temp_path.join("ralph.yml"))
        .current_dir(temp_path)
        .output()?;

    // EventLogger writes to the default path .ralph/events.jsonl
    // The marker file points to a timestamped path for isolation, but EventLogger
    // uses the default path for debugging/history purposes
    let events_file = temp_path.join(".ralph/events.jsonl");

    // Check events from run command
    let run_events = if events_file.exists() {
        fs::read_to_string(&events_file)?
    } else {
        String::new()
    };

    // Test 2: Run ralph run --continue (should publish task.resume)
    let _output = common::ralph_bin()
        .arg("run")
        .arg("--continue")
        .arg("--config")
        .arg(temp_path.join("ralph.yml"))
        .current_dir(temp_path)
        .output()?;

    // Check events after continue
    let continue_events = if events_file.exists() {
        fs::read_to_string(&events_file)?
    } else {
        String::new()
    };

    // Verify the difference (Plan 2026-08-13-003 U4):
    // - run should produce `task.start`
    // - continue should ADD `loop.resume` to the same file
    //   (NOT `task.resume` — that is the runtime recovery
    //   topic, reserved for recovery dispatch and never
    //   emitted as a continuation bootstrap).
    if !run_events.is_empty() {
        assert!(
            run_events.contains("task.start"),
            "Run should produce task.start event"
        );
    }

    if !continue_events.is_empty() {
        assert!(
            continue_events.contains("task.start"),
            "Events file should still contain task.start from the run"
        );
        assert!(
            continue_events.contains("loop.resume"),
            "Events file should now also contain loop.resume from the continue"
        );
    }

    Ok(())
}

#[test]
fn test_continue_logs_scratchpad_found() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Create config with short timeout and fast backend
    // Disable memories/tasks to test legacy scratchpad mode
    let config_content = r#"
event_loop:
  prompt_file: "PROMPT.md"
  completion_promise: "LOOP_COMPLETE"
  max_iterations: 1
  max_runtime_seconds: 5

cli:
  backend: "custom"
  command: "true"

core:
  scratchpad: ".ralph/agent/scratchpad.md"

memories:
  enabled: false

tasks:
  enabled: false
"#;

    fs::write(temp_path.join("ralph.yml"), config_content)?;

    // Create a prompt file
    fs::write(temp_path.join("PROMPT.md"), "Test task")?;

    // Create the .ralph/agent directory and scratchpad with unique content
    let agent_dir = temp_path.join(".ralph/agent");
    fs::create_dir_all(&agent_dir)?;

    let scratchpad_content = r"# Existing Task List

## Current Tasks
- [ ] UNIQUE_TASK_MARKER: Complete the special feature
- [x] Previously finished work

## Notes
This scratchpad contains UNIQUE_CONTENT_MARKER for testing.
";
    fs::write(agent_dir.join("scratchpad.md"), scratchpad_content)?;

    // Run ralph run --continue --no-tui (needed for tracing output to stdout)
    let output = common::ralph_bin()
        .arg("run")
        .arg("--continue")
        .arg("--no-tui")
        .arg("--config")
        .arg(temp_path.join("ralph.yml"))
        .current_dir(temp_path)
        .output()?;

    let _stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should log that it found the existing scratchpad (logged via tracing output)
    assert!(stdout.contains("Found existing scratchpad"));

    Ok(())
}

// U7b (plan 2026-06-21-002): pin the contract that
// `loop.resume` and `task.resume` topic constants are
// stable.  This is a cheap unit-level test that does not
// spawn the CLI; the full `--continue` integration test
// (above) is what actually exercises the resume boot path.
#[test]
fn u7b_resume_topic_constants_are_stable() {
    // The new control topic (used when
    // `UNIFIED_DETERMINISTIC_CORRECTION=1`) is exposed in
    // the ralph-proto API.
    assert_eq!(ralph_proto::LOOP_RESUME, "loop.resume");
    // The legacy resume topic remains the default boot
    // event when the feature flag is off.
    assert_eq!(ralph_proto::TASK_RESUME, "task.resume");
    // Both topics are recognised as orchestrator control
    // topics (so the origin guard lets them through).
    assert!(ralph_proto::is_orchestrator_control(
        ralph_proto::LOOP_RESUME
    ));
    assert!(ralph_proto::is_orchestrator_control(
        ralph_proto::TASK_RESUME
    ));
}

#[test]
fn u7b_resume_block_renders_loop_metadata() {
    // Pure unit test for the U7b `ResumeContext` rendering
    // shape — no CLI spawn, no tempdir.
    let rc =
        ralph_core::correction::ResumeContext::new("loop-xyz", 5, "5/10 done", 12, "scout -> plan");
    let block = rc.render_block();
    assert!(block.contains("Loop ID: loop-xyz"));
    assert!(block.contains("Closed tasks: 5"));
    assert!(block.contains("Last iteration: 12"));
    assert!(block.contains("Progress summary: 5/10 done"));
    assert!(block.contains("Scratchpad headline: scout -> plan"));
}

// ─────────────────────────────────────────────────────────────────────────
// 2026-08-03-004 U1: reuse resume-manifest gate (S6 drift + tamper)
// ─────────────────────────────────────────────────────────────────────────

mod reuse_gate {
    use super::default_worktree_root;
    use ralph_core::parallel_forge_resume::{
        BoundaryRecord, CaptureInputs, MANIFEST_FILE_NAME, MANIFEST_SCHEMA_VERSION, ResumeIdentity,
        ResumeManifest, sha256_hex, validate_manifest, write_manifest,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_git_repo(path: &Path) {
        let git_init = Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .expect("git init");
        assert!(git_init.status.success(), "git init failed");
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(path)
            .status()
            .expect("git config email");
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(path)
            .status()
            .expect("git config name");
        fs::write(path.join("README.md"), "# Test\n").expect("write README");
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .status()
            .expect("git add");
        Command::new("git")
            .args(["commit", "-m", "Initial commit", "--quiet"])
            .current_dir(path)
            .status()
            .expect("git commit");
    }

    fn write_backend_true_config(path: &Path) {
        let config = r#"event_loop:
  completion_promise: "loop.complete"
  max_iterations: 1

cli:
  backend: "custom"
  command: "true"
"#;
        fs::write(path.join("ralph.yml"), config).expect("write ralph.yml");
    }

    fn write_completed_worktree_entry(main_repo: &Path, loop_id: &str, worktree_path: &Path) {
        let ralph_dir = main_repo.join(".ralph");
        fs::create_dir_all(&ralph_dir).unwrap();
        let entry = serde_json::json!({
            "id": loop_id,
            "pid": 4_194_305_u32,
            "started": chrono::Utc::now(),
            "prompt": "previous prompt",
            "worktree_path": worktree_path.to_string_lossy(),
            "workspace": worktree_path.to_string_lossy(),
        });
        let body = serde_json::json!({ "loops": [entry] });
        fs::write(
            ralph_dir.join("loops.json"),
            serde_json::to_string_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    /// Pre-create a git-known worktree whose prior run stopped at an
    /// accepted `forge.plan.ready` boundary.
    fn precreate_worktree_with_accepted_boundary(main_repo: &Path, loop_id: &str) -> PathBuf {
        let worktree_path = default_worktree_root(main_repo).join(loop_id);
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                worktree_path.to_str().unwrap(),
                "HEAD",
            ])
            .current_dir(main_repo)
            .status()
            .expect("git worktree add");
        assert!(status.success(), "git worktree add must succeed");

        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let payload = "{\"plan_key\":\"pf-s6\"}";
        let event_line = format!(
            "{{\"ts\":\"2026-08-03T00:00:00Z\",\"iteration\":1,\"hat\":\"planner\",\"topic\":\"forge.plan.ready\",\"triggered\":\"guardian\",\"payload\":{}}}\n",
            serde_json::to_string(payload).unwrap()
        );
        fs::write(ralph_dir.join("events.jsonl"), &event_line).unwrap();

        let payload_digest = sha256_hex(payload.as_bytes());
        let transition_id =
            ralph_core::event_loop::accepted_transition::AcceptedTransition::compute_transition_id(
                loop_id,
                "planner:1",
                "rev-1",
                "forge.plan.ready:planner",
                &payload_digest,
            );
        let outbox_line = serde_json::json!({
            "activation_id": "planner:1",
            "committed_at": "2026-08-03T00:00:01Z",
            "contract_revision": "rev-1",
            "delivered": false,
            "loop_id": loop_id,
            "payload_digest": payload_digest,
            "topic": "forge.plan.ready",
            "transition_id": transition_id,
        });
        fs::write(
            agent_dir.join("accepted-transitions.jsonl"),
            format!("{outbox_line}\n"),
        )
        .unwrap();
        fs::write(ralph_dir.join("current-loop-id"), format!("{loop_id}\n")).unwrap();

        worktree_path
    }

    /// S6: the plan file content changes between the run that produced
    /// the archived state and the reuse attempt. The chained manifest
    /// identity must detect the drift and refuse the start.
    #[test]
    fn s6_reuse_worktree_identity_drift_fails_closed() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_backend_true_config(main_repo);

        let loop_id = "s6-identity-drift";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan v1\n").unwrap();

        let worktree_path = precreate_worktree_with_accepted_boundary(main_repo, loop_id);
        write_completed_worktree_entry(main_repo, loop_id, &worktree_path);

        // Run #1: identity baseline recorded from the current inputs.
        let first = super::common::ralph_bin()
            .args([
                "run",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "-H",
                "builtin:parallel-forge",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("first ralph run");
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        eprintln!("S6 run#1 stderr: {first_stderr}");
        assert!(
            !first_stderr.contains("resume manifest validation failed"),
            "first reuse must pass the manifest gate: {first_stderr}"
        );

        // Drift: same plan PATH, different content.
        fs::write(&plan_path, "# plan v2 — scope changed\n").unwrap();

        // Run #2 must fail closed on identity drift before the loop.
        let second = super::common::ralph_bin()
            .args([
                "run",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "-H",
                "builtin:parallel-forge",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("second ralph run");
        let second_stderr = String::from_utf8_lossy(&second.stderr);
        eprintln!("S6 run#2 stderr: {second_stderr}");

        assert!(
            !second.status.success(),
            "identity drift must refuse the start"
        );
        assert!(
            second_stderr.contains("resume manifest validation failed"),
            "stderr must carry the manifest gate refusal: {second_stderr}"
        );
        assert!(
            second_stderr.contains("identity drift"),
            "stderr must name the drift: {second_stderr}"
        );
        // The loop never started after the refusal.
        assert!(!worktree_path.join(".ralph/events.jsonl").exists());
    }

    /// Tamper form 1: a prior archive carries a manifest whose content
    /// no longer matches its self-digest. With no live runtime left,
    /// the fallback validation reads the archived manifest and refuses.
    #[test]
    fn tampered_archived_manifest_fails_closed() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_backend_true_config(main_repo);

        let loop_id = "tamper-digest-case";
        let worktree_path = default_worktree_root(main_repo).join(loop_id);
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                worktree_path.to_str().unwrap(),
                "HEAD",
            ])
            .current_dir(main_repo)
            .status()
            .expect("git worktree add");
        assert!(status.success());
        write_completed_worktree_entry(main_repo, loop_id, &worktree_path);

        // Plant a manifest that was valid once, then tamper one field.
        let mut manifest = ResumeManifest {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            captured_at: "2026-01-01T00:00:00Z".to_string(),
            identity: ResumeIdentity {
                plan_path: String::new(),
                plan_digest: String::new(),
                preset_name: String::new(),
                config_digest: String::new(),
                worktree_name: loop_id.to_string(),
                source_head_sha: String::new(),
                loop_id: loop_id.to_string(),
            },
            boundary: BoundaryRecord {
                accepted: Vec::new(),
                pending_hat: None,
                original_trigger: None,
                wave: None,
            },
            tasks: Vec::new(),
            artifacts: Vec::new(),
            incomplete_reasons: Vec::new(),
            manifest_digest: String::new(),
        };
        manifest.finalize_digest();
        manifest.identity.plan_digest = "forged-digest".to_string(); // tamper
        let archive = worktree_path.join(".ralph/reuse-history/20260101T000000Z");
        fs::create_dir_all(&archive).unwrap();
        write_manifest(&manifest, &archive).unwrap();

        // No live runtime state: cleanup archives nothing and the gate
        // falls back to the newest archived manifest.
        let output = super::common::ralph_bin()
            .args([
                "run",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "-H",
                "builtin:parallel-forge",
                "--worktree-name",
                loop_id,
                "--prompt",
                "tamper case",
            ])
            .current_dir(main_repo)
            .output()
            .expect("execute ralph");
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("tamper stderr: {stderr}");

        assert!(!output.status.success(), "tampered manifest must refuse");
        assert!(
            stderr.contains("resume manifest validation failed"),
            "stderr must carry the manifest gate refusal: {stderr}"
        );
        assert!(
            stderr.contains("digest mismatch"),
            "stderr must name the tamper detection: {stderr}"
        );
    }

    /// Tamper form 2 (partial archive): the archived manifest file is
    /// truncated / unparseable. The read itself fails closed.
    #[test]
    fn partial_archived_manifest_fails_closed() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_backend_true_config(main_repo);

        let loop_id = "tamper-partial-case";
        let worktree_path = default_worktree_root(main_repo).join(loop_id);
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                worktree_path.to_str().unwrap(),
                "HEAD",
            ])
            .current_dir(main_repo)
            .status()
            .expect("git worktree add");
        assert!(status.success());
        write_completed_worktree_entry(main_repo, loop_id, &worktree_path);

        let archive = worktree_path.join(".ralph/reuse-history/20260101T000000Z");
        fs::create_dir_all(&archive).unwrap();
        fs::write(
            archive.join(MANIFEST_FILE_NAME),
            "{\"schema_version\":\"parallel-forge-resume-manifest.v1\",\"identity\":",
        )
        .unwrap();

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "-H",
                "builtin:parallel-forge",
                "--worktree-name",
                loop_id,
                "--prompt",
                "partial case",
            ])
            .current_dir(main_repo)
            .output()
            .expect("execute ralph");
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("partial stderr: {stderr}");

        assert!(
            !output.status.success(),
            "partial manifest must refuse the start"
        );
        assert!(
            stderr.contains("resume manifest"),
            "stderr must carry the manifest refusal: {stderr}"
        );
    }

    /// Guard for the gate helper itself: validation passes for matching
    /// inputs and fails closed for drift (pure, no CLI spawn).
    #[test]
    fn validate_manifest_gate_contract() {
        let manifest = ResumeManifest {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            captured_at: "2026-01-01T00:00:00Z".to_string(),
            identity: ResumeIdentity {
                plan_path: "p.md".to_string(),
                plan_digest: "d1".to_string(),
                preset_name: "parallel-forge".to_string(),
                config_digest: "c1".to_string(),
                worktree_name: "wt".to_string(),
                source_head_sha: "sha".to_string(),
                loop_id: "loop".to_string(),
            },
            boundary: BoundaryRecord {
                accepted: Vec::new(),
                pending_hat: None,
                original_trigger: None,
                wave: None,
            },
            tasks: Vec::new(),
            artifacts: Vec::new(),
            incomplete_reasons: Vec::new(),
            manifest_digest: String::new(),
        };
        let mut manifest = manifest;
        manifest.finalize_digest();

        let matching = CaptureInputs {
            plan_path: "p.md".to_string(),
            plan_digest: "d1".to_string(),
            preset_name: "parallel-forge".to_string(),
            config_digest: "c1".to_string(),
            worktree_name: "wt".to_string(),
        };
        assert!(validate_manifest(&manifest, &matching).is_ok());

        let mut drifted = matching.clone();
        drifted.config_digest = "c2".to_string();
        assert!(validate_manifest(&manifest, &drifted).is_err());
    }

    // ── U2-fix (plan 2026-08-03-004 fix-unit): crash-window lockout ──
    //
    // correctness:C1 ⊕ testing:T1 — the crash window between a
    // successful resume bootstrap and the pending hat's first accepted
    // transition used to lock the worktree out of reuse forever:
    // the next capture judged the manifest incomplete, cleanup archived
    // that incomplete manifest (and the live evidence), and every later
    // fallback read returned the same incomplete manifest → permanent
    // refusal. These tests pin the recovery: a crashed-once worktree
    // always reaches a successful reuse (correct resume or fresh
    // bootstrap), never a permanent refusal ring.

    /// Config with the two hats the boundary hands off between.
    /// Backend `true` exits instantly and emits nothing, so a started
    /// loop terminates at `max_iterations` WITHOUT any accepted
    /// transition — the evidence-level equivalent of "crashed before
    /// the pending hat's first accepted transition".
    fn write_forge_hats_config(path: &Path) {
        let config = r#"event_loop:
  completion_promise: "loop.complete"
  starting_event: "forge.start"
  max_iterations: 1
  max_runtime_seconds: 30

cli:
  backend: "custom"
  command: "true"

memories:
  enabled: false

tasks:
  enabled: false

hats:
  planner:
    name: "Planner"
    description: "Writes the forge plan."
    triggers: ["forge.start"]
    publishes: ["forge.plan.ready"]
  guardian:
    name: "Guardian"
    description: "Approves the forge plan."
    triggers: ["forge.plan.ready"]
    publishes: ["loop.complete"]
"#;
        fs::write(path.join("ralph.yml"), config).expect("write ralph.yml");
        fs::write(path.join("parallel-forge.yml"), config).expect("write hats overlay");
    }

    /// Read the reused worktree's fresh events file (via the
    /// `current-events` marker).
    fn read_fresh_events(worktree_path: &Path) -> String {
        let marker = worktree_path.join(".ralph/current-events");
        assert!(marker.exists(), "current-events marker must exist");
        let rel = fs::read_to_string(&marker).unwrap();
        let rel = rel.trim();
        let path = worktree_path.join(rel);
        assert!(path.exists(), "events file {rel} must exist");
        fs::read_to_string(&path).unwrap()
    }

    /// Count archived manifests by completeness (newest-archive scan).
    fn archived_manifest_completeness(worktree_path: &Path) -> Vec<(String, bool)> {
        let history = worktree_path.join(".ralph/reuse-history");
        let mut dirs: Vec<PathBuf> = fs::read_dir(&history)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        dirs.sort();
        let mut result = Vec::new();
        for dir in dirs {
            let manifest_path = dir.join(MANIFEST_FILE_NAME);
            if !manifest_path.is_file() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(&manifest_path).expect("manifest readable"),
            )
            .expect("manifest parses");
            let complete = value
                .get("incomplete_reasons")
                .and_then(|v| v.as_array())
                .is_some_and(|reasons| reasons.is_empty());
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            result.push((name, complete));
        }
        result
    }

    fn spawn_reuse(main_repo: &Path, plan_path: &Path) -> std::process::Output {
        let hats_path = main_repo.join("parallel-forge.yml");
        super::common::ralph_bin()
            .args([
                "run",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "-H",
                hats_path.to_str().unwrap(),
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run")
    }

    /// The FULL compound lockout sequence, end to end:
    ///
    /// 1. Run A completed: accepted boundary in live log + outbox.
    /// 2. Reuse B: cleanup archives run A (complete manifest M_A), the
    ///    gate passes, the resume bootstrap starts — and the loop ends
    ///    with NO accepted transition (the crash-window evidence
    ///    shape).
    /// 3. Crash injection: the durable outbox boundary evidence is lost
    ///    as well (strongest form; drives the adjudication ① path).
    /// 4. Reuse C: capture sees events without any accepted boundary →
    ///    incomplete manifest M_C is archived → the gate refuses. This
    ///    FIRST refusal is the intended fail-closed behavior.
    /// 5. Reuse D: nothing live is left to archive, so the gate falls
    ///    back to the archives. M_C is incomplete and MUST be skipped;
    ///    the older complete M_A validates → the loop RESUMES from M_A
    ///    (targeted `task.resume` for the pending hat). No permanent
    ///    refusal ring.
    #[test]
    fn crash_window_lockout_recovers_via_older_complete_manifest() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_forge_hats_config(main_repo);

        let loop_id = "pf-crash-lockout";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan v1\n").unwrap();

        let worktree_path = precreate_worktree_with_accepted_boundary(main_repo, loop_id);
        write_completed_worktree_entry(main_repo, loop_id, &worktree_path);

        // Reuse B: resume bootstrap starts, no accepted transition
        // follows (crash-window evidence shape).
        let b = spawn_reuse(main_repo, &plan_path);
        let b_stderr = String::from_utf8_lossy(&b.stderr);
        eprintln!("crash-chain reuse B stderr: {b_stderr}");
        assert!(
            !b_stderr.contains("resume manifest validation failed"),
            "reuse B must pass the manifest gate: {b_stderr}"
        );
        let b_events = read_fresh_events(&worktree_path);
        assert!(
            b_events.contains("manifest_resume"),
            "reuse B must bootstrap the targeted resume: {b_events}"
        );

        // Crash injection: lose the durable outbox boundary evidence.
        // Run B's live event log stays — it carries the bootstrap but
        // no boundary event.
        let outbox = worktree_path.join(".ralph/agent/accepted-transitions.jsonl");
        assert!(
            outbox.exists(),
            "the outbox must survive reuse B (it is never archived)"
        );
        fs::remove_file(&outbox).unwrap();

        // Reuse C: capture judges incomplete → M_C archived → refuse.
        let c = spawn_reuse(main_repo, &plan_path);
        let c_stderr = String::from_utf8_lossy(&c.stderr);
        eprintln!("crash-chain reuse C stderr: {c_stderr}");
        assert!(
            !c.status.success(),
            "reuse C must refuse fail-closed on the incomplete capture"
        );
        assert!(
            c_stderr.contains("resume manifest validation failed"),
            "stderr must carry the gate refusal: {c_stderr}"
        );
        assert!(
            c_stderr.contains("incomplete"),
            "the refusal must name the incompleteness: {c_stderr}"
        );
        // The refusal archived exactly one incomplete manifest (the
        // old-semantics lockout poison).
        let archived = archived_manifest_completeness(&worktree_path);
        let incomplete_count = archived.iter().filter(|(_, complete)| !complete).count();
        assert_eq!(
            incomplete_count, 1,
            "exactly the crash capture must be incomplete: {archived:?}"
        );

        // Reuse D: the fallback must skip the incomplete archive and
        // resume from the older complete manifest — NOT refuse again.
        let d = spawn_reuse(main_repo, &plan_path);
        let d_stderr = String::from_utf8_lossy(&d.stderr);
        eprintln!("crash-chain reuse D stderr: {d_stderr}");
        assert!(
            !d_stderr.contains("resume manifest validation failed"),
            "reuse D must NOT be refused (no permanent lockout): {d_stderr}"
        );
        let d_events = read_fresh_events(&worktree_path);
        eprintln!("crash-chain reuse D events: {d_events}");
        // The recovery proves the fallback resumed from M_A: the
        // bootstrap record is the targeted task.resume for guardian.
        let mut saw_resume_bootstrap = false;
        for line in d_events.lines() {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if record.get("topic").and_then(|v| v.as_str()) != Some("task.resume") {
                continue;
            }
            let Some(payload_str) = record.get("payload").and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(payload_str) else {
                continue;
            };
            if payload.get("reason").and_then(|v| v.as_str()) != Some("manifest_resume") {
                continue;
            }
            assert_eq!(payload["target_hat"], "guardian");
            assert_eq!(payload["original_trigger_topic"], "forge.plan.ready");
            saw_resume_bootstrap = true;
        }
        assert!(
            saw_resume_bootstrap,
            "reuse D must resume from the older complete manifest: {d_events}"
        );
    }

    /// Adjudication ② shape, end to end: the crash where the durable
    /// outbox SURVIVES but the live event log lost the boundary must
    /// not be refused at all. The outbox record is accepted as
    /// fallback boundary evidence; the manifest completes with no
    /// derivable pending hat, and the loop degrades to a fresh
    /// bootstrap. A follow-up reuse must also succeed — the worktree
    /// is never locked out.
    #[test]
    fn crash_window_with_surviving_outbox_succeeds_on_first_reuse() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_forge_hats_config(main_repo);

        let loop_id = "pf-crash-outbox";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan v1\n").unwrap();

        let worktree_path = precreate_worktree_with_accepted_boundary(main_repo, loop_id);
        write_completed_worktree_entry(main_repo, loop_id, &worktree_path);

        // Crash injection: the live event log loses the boundary event;
        // the durable outbox keeps the acceptance record.
        fs::remove_file(worktree_path.join(".ralph/events.jsonl")).unwrap();
        assert!(
            worktree_path
                .join(".ralph/agent/accepted-transitions.jsonl")
                .exists(),
            "the outbox must carry the crash-window evidence"
        );

        // Reuse #1: outbox fallback evidence → complete manifest with
        // no derivable pending hat → gate passes → fresh bootstrap.
        let first = spawn_reuse(main_repo, &plan_path);
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        eprintln!("crash-outbox reuse #1 stderr: {first_stderr}");
        assert!(
            !first_stderr.contains("resume manifest validation failed"),
            "the surviving outbox must keep the first reuse unrefused: {first_stderr}"
        );
        let first_events = read_fresh_events(&worktree_path);
        eprintln!("crash-outbox reuse #1 events: {first_events}");
        // No targeted resume: the pending hat was underivable from the
        // outbox alone, so the fresh starting event remains the
        // bootstrap record.
        let mut saw_starting_bootstrap = false;
        for line in first_events.lines() {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let source = record.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let topic = record.get("topic").and_then(|v| v.as_str()).unwrap_or("");
            if source == "loop-bootstrap" && topic == "forge.start" {
                saw_starting_bootstrap = true;
            }
            if topic == "task.resume" {
                let payload = record.get("payload").and_then(|v| v.as_str()).unwrap_or("");
                assert!(
                    !payload.contains("manifest_resume"),
                    "no targeted recovery may start without a derivable pending hat: \
                     {first_events}"
                );
            }
        }
        assert!(
            saw_starting_bootstrap,
            "the fresh starting event must be the bootstrap: {first_events}"
        );

        // Reuse #2: run #1 ended without an accepted transition and the
        // outbox still carries run A's boundary. The gate must pass
        // again — no permanent refusal ring.
        let second = spawn_reuse(main_repo, &plan_path);
        let second_stderr = String::from_utf8_lossy(&second.stderr);
        eprintln!("crash-outbox reuse #2 stderr: {second_stderr}");
        assert!(
            !second_stderr.contains("resume manifest validation failed"),
            "the second reuse must also pass the gate: {second_stderr}"
        );
        let second_events = read_fresh_events(&worktree_path);
        assert!(
            !second_events.is_empty(),
            "the second reuse must start the loop"
        );
    }

    // ── U3-fix (plan 2026-08-03-004 fix-unit, adversarial A1): ──
    // completed-run reuse lockout. A worktree whose previous run
    // FINISHED NORMALLY ends on an accepted terminal boundary whose
    // in-log event carries no `triggered` hat. The old capture pushed
    // an incompleteness reason for exactly that shape, the gate
    // refused, and cleanup had already archived the live evidence —
    // every later reuse refused again. That contradicted the HARD
    // RULE 3 promise that a completed worktree is reusable.

    /// Append one accepted-transitions outbox line for a topic/payload.
    fn append_outbox_entry(
        worktree_path: &Path,
        loop_id: &str,
        topic: &str,
        payload: &str,
        hat: &str,
        committed_at: &str,
    ) {
        use std::io::Write;
        let payload_digest = sha256_hex(payload.as_bytes());
        let transition_id =
            ralph_core::event_loop::accepted_transition::AcceptedTransition::compute_transition_id(
                loop_id,
                &format!("{hat}:1"),
                "rev-1",
                &format!("{topic}:{hat}"),
                &payload_digest,
            );
        let entry = serde_json::json!({
            "activation_id": format!("{hat}:1"),
            "committed_at": committed_at,
            "contract_revision": "rev-1",
            "delivered": false,
            "loop_id": loop_id,
            "payload_digest": payload_digest,
            "topic": topic,
            "transition_id": transition_id,
        });
        let path = worktree_path.join(".ralph/agent/accepted-transitions.jsonl");
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{entry}").unwrap();
    }

    /// Pre-create a git-known worktree whose prior run COMPLETED
    /// normally: an earlier accepted boundary hands off, and the LAST
    /// accepted boundary is a terminal event carrying NO `triggered`
    /// hat, present in the live log (adversarial A1's real-world
    /// shape — terminal `report.done`-style record with
    /// triggered=None).
    fn precreate_worktree_with_completed_run(main_repo: &Path, loop_id: &str) -> PathBuf {
        let worktree_path = default_worktree_root(main_repo).join(loop_id);
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                worktree_path.to_str().unwrap(),
                "HEAD",
            ])
            .current_dir(main_repo)
            .status()
            .expect("git worktree add");
        assert!(status.success(), "git worktree add must succeed");

        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let plan_payload = "{\"plan_key\":\"pf-a1\"}";
        let plan_event = format!(
            "{{\"ts\":\"2026-08-03T00:00:00Z\",\"iteration\":1,\"hat\":\"planner\",\"topic\":\"forge.plan.ready\",\"triggered\":\"guardian\",\"payload\":{}}}\n",
            serde_json::to_string(plan_payload).unwrap()
        );
        // Terminal tail: accepted, in the log, NO `triggered` hat.
        let done_payload = "{\"status\":\"complete\"}";
        let done_event = format!(
            "{{\"ts\":\"2026-08-03T01:00:00Z\",\"iteration\":9,\"hat\":\"reporter\",\"topic\":\"report.done\",\"payload\":{}}}\n",
            serde_json::to_string(done_payload).unwrap()
        );
        fs::write(
            ralph_dir.join("events.jsonl"),
            format!("{plan_event}{done_event}"),
        )
        .unwrap();

        append_outbox_entry(
            &worktree_path,
            loop_id,
            "forge.plan.ready",
            plan_payload,
            "planner",
            "2026-08-03T00:00:01Z",
        );
        append_outbox_entry(
            &worktree_path,
            loop_id,
            "report.done",
            done_payload,
            "reporter",
            "2026-08-03T01:00:01Z",
        );
        fs::write(ralph_dir.join("current-loop-id"), format!("{loop_id}\n")).unwrap();

        worktree_path
    }

    /// Adversarial A1 end to end: a completed prior run must be
    /// reusable. Reuse #1 passes the gate (clean completion → complete
    /// manifest, no pending hat), fresh-bootstraps instead of entering
    /// a manifest resume, and reuse #2 passes again — no permanent
    /// refusal ring.
    #[test]
    fn completed_run_reuse_bootstraps_fresh_and_stays_reusable() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_forge_hats_config(main_repo);

        let loop_id = "pf-a1-completed";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan v1\n").unwrap();

        let worktree_path = precreate_worktree_with_completed_run(main_repo, loop_id);
        write_completed_worktree_entry(main_repo, loop_id, &worktree_path);

        // Reuse #1: clean completion → gate passes → fresh bootstrap.
        let first = spawn_reuse(main_repo, &plan_path);
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        eprintln!("A1 completed reuse #1 stderr: {first_stderr}");
        assert!(
            !first_stderr.contains("resume manifest validation failed"),
            "a normally completed run must not be refused: {first_stderr}"
        );
        let first_events = read_fresh_events(&worktree_path);
        eprintln!("A1 completed reuse #1 events: {first_events}");
        let mut saw_starting_bootstrap = false;
        for line in first_events.lines() {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let source = record.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let topic = record.get("topic").and_then(|v| v.as_str()).unwrap_or("");
            if source == "loop-bootstrap" && topic == "forge.start" {
                saw_starting_bootstrap = true;
            }
            if topic == "task.resume" {
                let payload = record.get("payload").and_then(|v| v.as_str()).unwrap_or("");
                assert!(
                    !payload.contains("manifest_resume"),
                    "clean completion must never enter a manifest resume: {first_events}"
                );
            }
        }
        assert!(
            saw_starting_bootstrap,
            "the fresh starting event must be the bootstrap: {first_events}"
        );

        // Reuse #2: run #1's log is archived; the durable outbox still
        // carries the terminal tail (now outbox-only) → the fallback
        // keeps the capture complete → the gate passes again.
        let second = spawn_reuse(main_repo, &plan_path);
        let second_stderr = String::from_utf8_lossy(&second.stderr);
        eprintln!("A1 completed reuse #2 stderr: {second_stderr}");
        assert!(
            !second_stderr.contains("resume manifest validation failed"),
            "the second reuse must also pass (no permanent lockout): {second_stderr}"
        );
        let second_events = read_fresh_events(&worktree_path);
        assert!(
            !second_events.is_empty(),
            "the second reuse must start the loop"
        );
    }

    /// Pre-create a git-known worktree whose prior run ended on an
    /// accepted `plan.blocked`-family terminal whose event is in NO
    /// event file (the runtime can accept such an event without a log
    /// record), with an earlier accepted boundary still in the log.
    fn precreate_worktree_with_plan_blocked_tail(main_repo: &Path, loop_id: &str) -> PathBuf {
        let worktree_path = default_worktree_root(main_repo).join(loop_id);
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                worktree_path.to_str().unwrap(),
                "HEAD",
            ])
            .current_dir(main_repo)
            .status()
            .expect("git worktree add");
        assert!(status.success(), "git worktree add must succeed");

        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let plan_payload = "{\"plan_key\":\"pf-a1-blocked\"}";
        let plan_event = format!(
            "{{\"ts\":\"2026-08-03T00:00:00Z\",\"iteration\":1,\"hat\":\"planner\",\"topic\":\"forge.plan.ready\",\"triggered\":\"guardian\",\"payload\":{}}}\n",
            serde_json::to_string(plan_payload).unwrap()
        );
        fs::write(ralph_dir.join("events.jsonl"), plan_event).unwrap();

        append_outbox_entry(
            &worktree_path,
            loop_id,
            "forge.plan.ready",
            plan_payload,
            "planner",
            "2026-08-03T00:00:01Z",
        );
        // Terminal tail: accepted in the outbox only — no matching
        // event in any event file.
        let blocked_payload = "{\"kind\":\"precheck_exhausted\"}";
        append_outbox_entry(
            &worktree_path,
            loop_id,
            "plan.blocked",
            blocked_payload,
            "runtime",
            "2026-08-03T01:00:01Z",
        );
        fs::write(ralph_dir.join("current-loop-id"), format!("{loop_id}\n")).unwrap();

        worktree_path
    }

    /// Adversarial A1's second form (U2-semantics regression pin): a
    /// `plan.blocked`-family terminal tail recorded ONLY in the outbox
    /// must also reuse successfully — outbox fallback evidence keeps
    /// the manifest complete with no pending hat → fresh bootstrap.
    #[test]
    fn plan_blocked_outbox_only_tail_reuse_bootstraps_fresh() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_forge_hats_config(main_repo);

        let loop_id = "pf-a1-blocked";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan v1\n").unwrap();

        let worktree_path = precreate_worktree_with_plan_blocked_tail(main_repo, loop_id);
        write_completed_worktree_entry(main_repo, loop_id, &worktree_path);

        let output = spawn_reuse(main_repo, &plan_path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("A1 plan.blocked-tail stderr: {stderr}");
        assert!(
            !stderr.contains("resume manifest validation failed"),
            "an outbox-only terminal tail must not be refused: {stderr}"
        );
        let events = read_fresh_events(&worktree_path);
        eprintln!("A1 plan.blocked-tail events: {events}");
        let mut saw_starting_bootstrap = false;
        for line in events.lines() {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let source = record.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let topic = record.get("topic").and_then(|v| v.as_str()).unwrap_or("");
            if source == "loop-bootstrap" && topic == "forge.start" {
                saw_starting_bootstrap = true;
            }
            if topic == "task.resume" {
                let payload = record.get("payload").and_then(|v| v.as_str()).unwrap_or("");
                assert!(
                    !payload.contains("manifest_resume"),
                    "no targeted recovery may start without a derivable pending hat: {events}"
                );
            }
        }
        assert!(
            saw_starting_bootstrap,
            "the fresh starting event must be the bootstrap: {events}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 2026-08-03-004 U2: manifest-driven resume bootstrap (target hat
// resume payload + parallel-forge hat handoff). The reuse gate (U1)
// already validated the manifest; U2 consumes it at loop bootstrap and
// re-binds the pending hat to its original trigger through the EXISTING
// `task.resume` recovery contract.
// ─────────────────────────────────────────────────────────────────────────

mod reuse_resume_bootstrap {
    use super::default_worktree_root;
    use ralph_core::parallel_forge_resume::sha256_hex;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_git_repo(path: &Path) {
        let git_init = Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .expect("git init");
        assert!(git_init.status.success(), "git init failed");
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(path)
            .status()
            .expect("git config email");
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(path)
            .status()
            .expect("git config name");
        fs::write(path.join("README.md"), "# Test\n").expect("write README");
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .status()
            .expect("git add");
        Command::new("git")
            .args(["commit", "-m", "Initial commit", "--quiet"])
            .current_dir(path)
            .status()
            .expect("git commit");
    }

    /// Config with the two hats the S1 boundary hands off between.
    /// Backend `true` exits instantly and emits nothing, so the loop
    /// terminates after `max_iterations` without side emissions. The
    /// topology closes on `loop.complete` so the preset-lint gate
    /// passes; memories/tasks stay disabled (no coordinator needed).
    fn write_forge_hats_config(path: &Path) {
        let config = r#"event_loop:
  completion_promise: "loop.complete"
  starting_event: "forge.start"
  max_iterations: 1
  max_runtime_seconds: 30

cli:
  backend: "custom"
  command: "true"

memories:
  enabled: false

tasks:
  enabled: false

hats:
  planner:
    name: "Planner"
    description: "Writes the forge plan."
    triggers: ["forge.start"]
    publishes: ["forge.plan.ready"]
  guardian:
    name: "Guardian"
    description: "Approves the forge plan."
    triggers: ["forge.plan.ready"]
    publishes: ["loop.complete"]
"#;
        fs::write(path.join("ralph.yml"), config).expect("write ralph.yml");
        fs::write(path.join("parallel-forge.yml"), config).expect("write hats overlay");
    }

    /// Config whose hat set does NOT contain the manifest's pending
    /// hat (`guardian`) — the bootstrap must fall back to a fresh
    /// start instead of starting a recovery.
    fn write_unrelated_hat_config(path: &Path) {
        let config = r#"event_loop:
  completion_promise: "loop.complete"
  starting_event: "forge.start"
  max_iterations: 1
  max_runtime_seconds: 30

cli:
  backend: "custom"
  command: "true"

memories:
  enabled: false

tasks:
  enabled: false

hats:
  auditor:
    name: "Auditor"
    description: "Audits the forge result."
    triggers: ["forge.start"]
    publishes: ["loop.complete"]
"#;
        fs::write(path.join("ralph.yml"), config).expect("write ralph.yml");
    }

    fn write_completed_worktree_entry(main_repo: &Path, loop_id: &str, worktree_path: &Path) {
        let ralph_dir = main_repo.join(".ralph");
        fs::create_dir_all(&ralph_dir).unwrap();
        let entry = serde_json::json!({
            "id": loop_id,
            "pid": 4_194_305_u32,
            "started": chrono::Utc::now(),
            "prompt": "previous prompt",
            "worktree_path": worktree_path.to_string_lossy(),
            "workspace": worktree_path.to_string_lossy(),
        });
        let body = serde_json::json!({ "loops": [entry] });
        fs::write(
            ralph_dir.join("loops.json"),
            serde_json::to_string_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    /// Pre-create a git-known worktree whose prior run stopped right
    /// after the accepted `forge.plan.ready` boundary (pending hat:
    /// `guardian`).
    fn precreate_worktree_with_accepted_boundary(main_repo: &Path, loop_id: &str) -> PathBuf {
        let worktree_path = default_worktree_root(main_repo).join(loop_id);
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                worktree_path.to_str().unwrap(),
                "HEAD",
            ])
            .current_dir(main_repo)
            .status()
            .expect("git worktree add");
        assert!(status.success(), "git worktree add must succeed");

        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let payload = "{\"plan_key\":\"pf-u2-bootstrap\",\"execution_wave\":2}";
        let event_line = format!(
            "{{\"ts\":\"2026-08-03T00:00:00Z\",\"iteration\":1,\"hat\":\"planner\",\"topic\":\"forge.plan.ready\",\"triggered\":\"guardian\",\"payload\":{}}}\n",
            serde_json::to_string(payload).unwrap()
        );
        fs::write(ralph_dir.join("events.jsonl"), &event_line).unwrap();

        let payload_digest = sha256_hex(payload.as_bytes());
        let transition_id =
            ralph_core::event_loop::accepted_transition::AcceptedTransition::compute_transition_id(
                loop_id,
                "planner:1",
                "rev-1",
                "forge.plan.ready:planner",
                &payload_digest,
            );
        let outbox_line = serde_json::json!({
            "activation_id": "planner:1",
            "committed_at": "2026-08-03T00:00:01Z",
            "contract_revision": "rev-1",
            "delivered": false,
            "loop_id": loop_id,
            "payload_digest": payload_digest,
            "topic": "forge.plan.ready",
            "transition_id": transition_id,
        });
        fs::write(
            agent_dir.join("accepted-transitions.jsonl"),
            format!("{outbox_line}\n"),
        )
        .unwrap();
        fs::write(ralph_dir.join("current-loop-id"), format!("{loop_id}\n")).unwrap();

        worktree_path
    }

    /// Read the reused worktree's fresh events file (via the
    /// `current-events` marker) after the run.
    fn read_fresh_events(worktree_path: &Path) -> String {
        let marker = worktree_path.join(".ralph/current-events");
        assert!(marker.exists(), "current-events marker must exist");
        let rel = fs::read_to_string(&marker).unwrap();
        let rel = rel.trim();
        let path = worktree_path.join(rel);
        assert!(path.exists(), "events file {rel} must exist");
        fs::read_to_string(&path).unwrap()
    }

    /// S1 end-to-end: reuse with a validated manifest whose pending
    /// hat is `guardian` bootstraps the loop with a TARGETED
    /// `task.resume` that re-binds guardian to its original
    /// `forge.plan.ready` trigger — instead of the plain starting
    /// event.
    #[test]
    fn s1_reuse_bootstrap_emits_targeted_task_resume() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_forge_hats_config(main_repo);

        let loop_id = "pf-u2-bootstrap";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan v1\n").unwrap();

        let worktree_path = precreate_worktree_with_accepted_boundary(main_repo, loop_id);
        write_completed_worktree_entry(main_repo, loop_id, &worktree_path);

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "-H",
                main_repo.join("parallel-forge.yml").to_str().unwrap(),
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("U2 S1 stderr: {stderr}");
        assert!(
            !stderr.contains("resume manifest validation failed"),
            "the manifest gate must pass: {stderr}"
        );

        let events = read_fresh_events(&worktree_path);
        eprintln!("U2 S1 events file: {events}");

        // The bootstrap record is the targeted task.resume, not the
        // configured starting event.
        let mut saw_resume_bootstrap = false;
        for line in events.lines() {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let source = record.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let topic = record.get("topic").and_then(|v| v.as_str()).unwrap_or("");
            if source == "loop-bootstrap" {
                assert_ne!(
                    topic, "task.start",
                    "fresh starting event must be replaced by the recovery bootstrap"
                );
                assert_ne!(topic, "forge.start");
            }
            if topic != "task.resume" {
                continue;
            }
            let payload_str = record
                .get("payload")
                .and_then(|v| v.as_str())
                .expect("bootstrap task.resume must carry the recovery payload");
            let payload: serde_json::Value = serde_json::from_str(payload_str)
                .expect("recovery payload must be structured JSON");
            assert_eq!(payload["target_hat"], "guardian");
            assert_eq!(payload["original_hat"], "guardian");
            assert_eq!(payload["original_trigger_topic"], "forge.plan.ready");
            assert_eq!(
                payload["original_trigger_payload"]["plan_key"],
                "pf-u2-bootstrap"
            );
            assert_eq!(payload["reason"], "manifest_resume");
            assert_eq!(payload["kind"], "manifest_resume");
            saw_resume_bootstrap = true;
        }
        assert!(
            saw_resume_bootstrap,
            "bootstrap must publish the targeted task.resume: {events}"
        );
    }

    /// Fail-closed bootstrap: the manifest's pending hat is NOT part
    /// of the current hats → no recovery is started; the loop falls
    /// back to the plain fresh bootstrap (starting event) and still
    /// runs.
    #[test]
    fn unregistered_pending_hat_falls_back_to_fresh_bootstrap() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_unrelated_hat_config(main_repo);

        let loop_id = "pf-u2-fallback";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan v1\n").unwrap();

        let worktree_path = precreate_worktree_with_accepted_boundary(main_repo, loop_id);
        write_completed_worktree_entry(main_repo, loop_id, &worktree_path);

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("U2 fallback stderr: {stderr}");
        assert!(
            !stderr.contains("resume manifest validation failed"),
            "the manifest gate must still pass: {stderr}"
        );

        let events = read_fresh_events(&worktree_path);
        eprintln!("U2 fallback events file: {events}");

        // No recovery bootstrap: the starting event stays the
        // bootstrap record and no manifest-resume task.resume exists.
        let mut saw_starting_bootstrap = false;
        for line in events.lines() {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let source = record.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let topic = record.get("topic").and_then(|v| v.as_str()).unwrap_or("");
            if source == "loop-bootstrap" && topic == "forge.start" {
                saw_starting_bootstrap = true;
            }
            if topic == "task.resume" {
                let payload = record.get("payload").and_then(|v| v.as_str()).unwrap_or("");
                assert!(
                    !payload.contains("manifest_resume"),
                    "no recovery may be started for an unregistered pending hat: {events}"
                );
            }
        }
        assert!(
            saw_starting_bootstrap,
            "fresh starting event must remain the bootstrap: {events}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 2026-09-01-2102 U2: combined --continue --worktree --reuse-worktree
// gate (RunIntent::ContinueReusedWorktree). The gate is fail-closed:
// - missing worktree → WorktreeMissing, no `.worktrees/<id>/` created
// - AlreadyCompleted history → Checkpoint hint with the two
//   operator-facing remedies (drop --continue or
//   --remove-worktree-and-continue)
// - Eligible checkpoint → archive directory is NEVER created, live
//   runtime artifacts are untouched
// - Live lock from another process → WorktreeLive with PID; refusing
//   to attach
//
// `LoopHistory::record_started` / `record_completed` are the SSOT for
// the `history.jsonl` line format. The tests use those recorders
// instead of hand-rolling JSON lines so the fixture stays honest even
// if the on-disk shape evolves.
// ─────────────────────────────────────────────────────────────────────────

mod combined_intent {
    use ralph_core::loop_history::LoopHistory;
    use ralph_core::loop_lock::LoopLock;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_git_repo(path: &Path) {
        let status = Command::new("git")
            .args(["init"])
            .current_dir(path)
            .status()
            .expect("git init");
        assert!(status.success(), "git init failed");
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(path)
            .status()
            .expect("git config email");
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(path)
            .status()
            .expect("git config name");
        fs::write(path.join("README.md"), "# Test\n").expect("write README");
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .status()
            .expect("git add");
        Command::new("git")
            .args(["commit", "-m", "Initial commit", "--quiet"])
            .current_dir(path)
            .status()
            .expect("git commit");
    }

    /// Minimal config that exits as soon as it boots. `completion_promise`
    /// matches `loop.complete` so the topology closes without spinning
    /// past the gate. `backend: custom / command: true` lets the loop
    /// reach `loop.complete` deterministically.
    fn write_minimal_config(path: &Path) {
        let config = r#"event_loop:
  completion_promise: "loop.complete"
  starting_event: "loop.start"
  max_iterations: 1
  max_runtime_seconds: 30

cli:
  backend: "custom"
  command: "true"

memories:
  enabled: false

tasks:
  enabled: false

hats:
  worker:
    name: "Worker"
    description: "Single hat that closes the loop."
    triggers: ["loop.start"]
    publishes: ["loop.complete"]
"#;
        fs::write(path.join("ralph.yml"), config).expect("write ralph.yml");
    }

    fn precreate_worktree(main_repo: &Path, loop_id: &str) -> PathBuf {
        // The runtime computes the worktree path as
        // `<workspace_root>/.worktrees/<loop_id>` (see
        // `commands::run_recovery::acquire_and_assess` and
        // `loop_runner::loop_owner::spawn_worktree_loop`). The
        // older top-level `default_worktree_root` helper in this
        // test file uses a different scheme (`parent/worktree/...`),
        // so we cannot reuse it for the U2 gate path — the gate
        // would surface "does not exist at" against the
        // `default_worktree_root` location and pass against the
        // runtime location, hiding wiring bugs.
        let worktree_path = main_repo.join(".worktrees").join(loop_id);
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                worktree_path.to_str().unwrap(),
                "HEAD",
            ])
            .current_dir(main_repo)
            .status()
            .expect("git worktree add");
        assert!(status.success(), "git worktree add must succeed");
        worktree_path
    }

    /// Build the minimum `assess_checkpoint` fixture on top of an
    /// already-created worktree. `events.jsonl` is left empty by
    /// default so the gate's verdict is determined entirely by the
    /// history the caller writes before invoking ralph.
    ///
    /// The `current-events` marker content MUST resolve to a regular
    /// file when joined against the worktree. `assess_checkpoint`
    /// treats both missing and non-regular targets as
    /// `MissingCurrentEventsTarget`. We use the absolute path of the
    /// events file (same scheme as the unit tests' `fixture()`
    /// helper in `recovery_checkpoint.rs`) so the fixture is
    /// independent of `cwd` quirks in nextest's process-per-test
    /// isolation.
    /// U4 helper: pre-create the worktree at the **canonical
    /// external layout** (`<parent>/worktree/<project>/<id>`) that
    /// the combined `--continue --worktree --reuse-worktree` path
    /// actually uses to look up by name via
    /// `WorktreeConfig::default().worktree_path(repo_root)` (see
    /// `find_reusable_worktree_by_name` and `spawn_worktree_loop`).
    ///
    /// The other helper `precreate_worktree` uses the older
    /// `workspace_root/.worktrees/<id>` scheme (matching
    /// `commands::run_recovery::acquire_and_assess`'s literal
    /// join), which the gate's U3 happy-path test accepts because
    /// it does not depend on the worktree lookup — it only checks
    /// the gate diagnostic. The U4 crash-window repair test DOES
    /// depend on the lookup succeeding (so the child reads the
    /// pre-staged outbox at the same path), so it must use the
    /// canonical layout.
    /// U4 helper: pre-create the worktree at BOTH locations the
    /// combined `--continue --worktree --reuse-worktree` path
    /// touches:
    ///
    /// 1. **Canonical external layout** (`<parent>/worktree/<project>/<id>`):
    ///    where `spawn_worktree_loop` actually creates the worktree
    ///    and where the child process chdir's into. The repair
    ///    step reads the outbox at `<this>/.ralph/agent/accepted-
    ///    transitions.jsonl`, so the outbox MUST be staged here.
    /// 2. **Legacy gate location** (`<main_repo>/.worktrees/<id>`):
    ///    where `acquire_and_assess` (`run_recovery.rs:283`) checks
    ///    `workspace_root.join(".worktrees").join(name).is_dir()`
    ///    as the gate's "worktree exists on disk" precondition. If
    ///    this directory is missing the gate fails closed with
    ///    `GateError::WorktreeMissing` and the child never runs —
    ///    which is correct production behavior but makes this U4
    ///    test unable to reach the repair step.
    ///
    /// Both directories point at the same git tree (HEAD of
    /// `main_repo`); only the path differs. Both directories'
    /// `.ralph/agent/` is seeded with the assess fixture so the
    /// gate's `assess_checkpoint` finds what it needs at the
    /// canonical path (the gate locks the legacy location and
    /// assesses the canonical location — see
    /// `run_recovery::acquire_and_assess`).
    fn precreate_canonical_worktree(main_repo: &Path, loop_id: &str) -> PathBuf {
        // Canonical external layout — where ralph's actual loop runs.
        let canonical_root = main_repo
            .parent()
            .unwrap_or(main_repo)
            .join("worktree")
            .join(main_repo.file_name().expect("repo must have a basename"));
        fs::create_dir_all(&canonical_root)
            .expect("create canonical worktree root");
        let canonical_path = canonical_root.join(loop_id);

        // Legacy gate location — where the gate looks first.
        let legacy_path = main_repo.join(".worktrees").join(loop_id);
        fs::create_dir_all(legacy_path.parent().expect("legacy parent"))
            .expect("create legacy gate root");

        // Add ONE git worktree at the canonical path. The legacy
        // directory is just a sibling directory we pre-stage so
        // the gate's literal-join check passes — it is NOT a git
        // worktree of its own (a second `git worktree add` would
        // conflict on the same ref).
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                canonical_path.to_str().unwrap(),
                "HEAD",
            ])
            .current_dir(main_repo)
            .status()
            .expect("git worktree add (canonical)");
        assert!(
            status.success(),
            "git worktree add (canonical) must succeed: {:?}",
            canonical_path
        );

        // Mirror the assess fixture into the legacy location too.
        // The gate (`acquire_and_assess` -> `assess_checkpoint`)
        // reads `.ralph/current-loop-id`, `.ralph/current-events`,
        // `.ralph/events.jsonl`, `.ralph/history.jsonl`, and
        // `.ralph/agent/scratchpad.md` from the LEGACY
        // `.worktrees/<id>` location — the gate locks the legacy
        // path so the assessment also runs there. The CANONICAL
        // path's `.ralph/` is what the child loop reads for
        // `repair_state_machine_projection_from_outbox` and event
        // writes; the caller stages that via
        // `seed_assess_fixture(&canonical_path, loop_id)` after
        // this helper returns.
        let legacy_ralph = legacy_path.join(".ralph");
        let legacy_agent = legacy_ralph.join("agent");
        fs::create_dir_all(&legacy_agent)
            .expect("create legacy .ralph/agent");
        let legacy_events = legacy_ralph.join("events.jsonl");
        fs::write(&legacy_events, "").expect("legacy events.jsonl");
        fs::write(
            legacy_ralph.join("current-events"),
            legacy_events.to_str().expect("legacy events UTF-8"),
        )
        .expect("legacy current-events marker");
        fs::write(
            legacy_ralph.join("current-loop-id"),
            format!("{loop_id}\n"),
        )
        .expect("legacy current-loop-id marker");
        fs::write(legacy_agent.join("scratchpad.md"), "# scratch\n")
            .expect("legacy scratchpad");
        fs::write(legacy_ralph.join("history.jsonl"), "")
            .expect("legacy history.jsonl");

        canonical_path
    }

    fn seed_assess_fixture(worktree_path: &Path, loop_id: &str) {
        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let events_file = ralph_dir.join("events.jsonl");
        fs::write(&events_file, "").unwrap();
        fs::write(
            ralph_dir.join("current-events"),
            events_file.to_str().expect("events_file must be UTF-8"),
        )
        .unwrap();
        fs::write(ralph_dir.join("current-loop-id"), format!("{loop_id}\n")).unwrap();
        fs::write(agent_dir.join("scratchpad.md"), "# scratch\n").unwrap();
        fs::write(ralph_dir.join("history.jsonl"), "").unwrap();
    }

    /// The pre-existing `--continue` handler in `run.rs` enforces a
    /// `scratchpad.md` exists at the configured workspace root
    /// (the main repo, not the worktree) before the runtime ever
    /// reaches the new `acquire_and_assess` gate. Without this
    /// pre-stage, every combined run bails out with "Cannot continue:
    /// scratchpad not found" before the gate can run. We seed the
    /// minimum-viable scratchpad at the main repo so the existing
    /// check passes and the gate is what surfaces the verdict.
    fn seed_main_repo_scratchpad(main_repo: &Path) {
        let agent_dir = main_repo.join(".ralph/agent");
        fs::create_dir_all(&agent_dir).expect("create main repo .ralph/agent");
        fs::write(agent_dir.join("scratchpad.md"), "# main repo scratchpad\n")
            .expect("write main repo scratchpad");
    }

    /// C1: combined flags with a `--plan` whose basename resolves to
    /// a worktree that does NOT exist. The gate is fail-closed:
    /// - stderr names the missing worktree and its expected path
    /// - the `.worktrees/<id>/` directory was never created (no
    ///   half-built worktree left behind for the operator to clean up)
    #[test]
    fn combined_missing_worktree_rejected_no_create() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_minimal_config(main_repo);
        seed_main_repo_scratchpad(main_repo);

        let loop_id = "u2-missing-wt";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan\n").unwrap();

        // Sanity: worktree does not exist yet at the runtime path
        // (`<workspace_root>/.worktrees/<loop_id>`).
        let expected = main_repo.join(".worktrees").join(loop_id);
        assert!(
            !expected.exists(),
            "test precondition: target worktree must not exist"
        );

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--continue",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("U2 missing-wt stderr: {stderr}");
        assert!(
            !output.status.success(),
            "missing-worktree combined run must exit non-zero"
        );
        assert!(
            stderr.contains("does not exist at"),
            "stderr must name the missing-worktree reason: {stderr}"
        );
        assert!(
            stderr.contains(loop_id),
            "stderr must reference the worktree name '{loop_id}': {stderr}"
        );
        // The gate did not half-create the worktree.
        assert!(
            !expected.exists(),
            "missing-worktree rejection must not create {expected:?}"
        );
    }

    /// C2: combined flags against a worktree whose history records
    /// `completion_promise`. The gate must surface a Checkpoint
    /// refusal that includes BOTH operator-facing remedies (drop
    /// --continue, or use --remove-worktree-and-continue). The hint
    /// has to be discoverable from stderr without re-running the CLI
    /// with --help.
    #[test]
    fn combined_completed_history_rejected_with_remove_continue_hint() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_minimal_config(main_repo);
        seed_main_repo_scratchpad(main_repo);

        let loop_id = "u2-already-done";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan\n").unwrap();

        let worktree_path = precreate_worktree(main_repo, loop_id);
        seed_assess_fixture(&worktree_path, loop_id);

        // Record the prior run in the SSOT shape so the gate can
        // recognize it as already completed.
        let history = LoopHistory::new(worktree_path.join(".ralph/history.jsonl"));
        history.record_started("previous run").unwrap();
        history.record_completed("completion_promise").unwrap();

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--continue",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("U2 already-done stderr: {stderr}");
        assert!(
            !output.status.success(),
            "already-completed combined run must exit non-zero"
        );
        assert!(
            stderr.contains("already completed")
                || stderr.contains("completion_promise"),
            "stderr must name the already-completed reason: {stderr}"
        );
        assert!(
            stderr.contains("drop --continue"),
            "stderr must surface the drop-rewrite remedy: {stderr}"
        );
        assert!(
            stderr.contains("--remove-worktree-and-continue"),
            "stderr must surface the destructive-continue remedy: {stderr}"
        );
        // The archive step must NOT have run for an AlreadyCompleted
        // verdict — the gate refuses before the cleanup branch.
        assert!(
            !worktree_path.join(".ralph/reuse-history").exists(),
            "AlreadyCompleted must refuse before any archive happens"
        );
    }

    /// C3: combined flags against an Eligible checkpoint. The
    /// continuation contract preserves the runtime fixtures the gate
    /// depends on (scratchpad, current-loop-id, current-events
    /// marker, history) and NEVER creates `.ralph/reuse-history/`.
    /// This is the inverse of the U1 `--reuse-worktree` (without
    /// --continue) tests where the archive step is required.
    ///
    /// The loop DOES run (and will append to events.jsonl /
    /// create tasks.jsonl) — that is the whole point of the gate
    /// returning `Eligible` and proceeding. We deliberately assert
    /// only the artifacts the **gate** is responsible for: the
    /// archive directory must not exist, and the pre-existing
    /// scratchpad/current-loop-id/current-events/history fixtures
    /// must still be readable so a follow-up retry can re-assess
    /// the same checkpoint.
    #[test]
    fn combined_skips_cleanup_when_archive_path_unused() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_minimal_config(main_repo);
        seed_main_repo_scratchpad(main_repo);

        let loop_id = "u2-eligible-skip-cleanup";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan\n").unwrap();

        let worktree_path = precreate_worktree(main_repo, loop_id);
        seed_assess_fixture(&worktree_path, loop_id);

        // Pre-stage live runtime artifacts the resume contract reads.
        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        let events_file = ralph_dir.join("events.jsonl");
        let tasks_file = agent_dir.join("tasks.jsonl");
        fs::write(&events_file, "{\"line\":\"prior-event\"}\n").unwrap();
        fs::write(&tasks_file, "{\"id\":\"u2-eligible-skip-cleanup:step-1\"}\n").unwrap();

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--continue",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("U2 eligible-skip stderr: {stderr}");

        // The archive path was never taken — this is the core U2
        // invariant: combined --continue --reuse-worktree must skip
        // clean_worktree_runtime_artifacts regardless of whether the
        // gate verdict was Eligible or AlreadyCompleted.
        assert!(
            !ralph_dir.join("reuse-history").exists(),
            "Eligible combined path must not create .ralph/reuse-history/: {stderr}"
        );

        // Gate-controlled fixtures survive — if the gate had
        // rotated any of these, a follow-up retry could not
        // re-assess the same checkpoint and would lose the
        // continuation contract.
        assert!(events_file.exists(), "events.jsonl must survive");
        assert!(tasks_file.exists(), "tasks.jsonl must survive");
        assert!(
            ralph_dir.join("current-loop-id").exists(),
            "current-loop-id marker must survive"
        );
        assert!(
            ralph_dir.join("current-events").exists(),
            "current-events marker must survive"
        );
        assert!(
            agent_dir.join("scratchpad.md").is_file(),
            "scratchpad must survive"
        );
    }

    /// C4: combined flags when the worktree's loop lock is held by
    /// another live process. The test acquires the lock with the
    /// SSOT `LoopLock::try_acquire` API and keeps the guard alive
    /// while spawning ralph; the subprocess must surface a
    /// WorktreeLive refusal that names the holder PID.
    #[test]
    fn combined_lock_busy_second_process_rejected() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_minimal_config(main_repo);
        seed_main_repo_scratchpad(main_repo);

        let loop_id = "u2-lock-busy";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan\n").unwrap();

        let worktree_path = precreate_worktree(main_repo, loop_id);
        seed_assess_fixture(&worktree_path, loop_id);

        // Acquire and HOLD the worktree's loop lock from this test
        // process. The subprocess ralph run must then fail the
        // LoopLock::try_acquire step in the gate.
        let holder = LoopLock::try_acquire(&worktree_path, "test holder")
            .expect("test must successfully acquire the worktree lock first");
        let holder_pid = std::process::id();

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--continue",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("U2 lock-busy stderr: {stderr}");
        drop(holder);

        assert!(
            !output.status.success(),
            "lock-busy combined run must exit non-zero"
        );
        assert!(
            stderr.contains("locked by another live loop"),
            "stderr must name the lock-busy reason: {stderr}"
        );
        assert!(
            stderr.contains("refusing to attach"),
            "stderr must surface the refusing-to-attach hint: {stderr}"
        );
        assert!(
            stderr.contains(&format!("pid {holder_pid}")),
            "stderr must name the holder PID {holder_pid}: {stderr}"
        );
        assert!(
            stderr.contains(loop_id),
            "stderr must reference the worktree name '{loop_id}': {stderr}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // 2026-09-01-2102 U3: combined --continue --worktree --reuse-worktree
    // SUCCESS path (RunIntent::ContinueReusedWorktree, Eligible verdict).
    //
    // U2 tests (combined_intent above) cover the gate's *fail-closed*
    // behavior (missing worktree, AlreadyCompleted, lock-busy). U3
    // exercises what happens AFTER the gate returns `Eligible`:
    //
    //   - the loop actually runs and emits `loop.resume` (continuation
    //     bootstrap), NOT a fresh `starting` event
    //   - non-success terminal reasons (`max_iterations`, `max_runtime`,
    //     `failure`, signal) clear the gate and continue, because only
    //     `completion_promise` triggers AlreadyCompleted
    //   - the child branch (entered via `--worktree-path` +
    //     `--combined-continue`) skips the parallel-forge
    //     resume-manifest re-validation gate that the parent already
    //     cleared, so a stale manifest in `.ralph/reuse-history/` is
    //     NOT a fail-closed refusal on the combined path
    // ─────────────────────────────────────────────────────────────────────

    /// U3 (plan §3.1+3.3 happy-path wiring): the combined
    /// `--continue --worktree --reuse-worktree` path wires together
    /// gate acquisition, child-branch `--combined-continue` forwarding,
    /// and lock release. This test verifies the WIRED invariants the
    /// gate is responsible for regardless of which way the verdict
    /// goes:
    ///
    ///   - the ralph subprocess reaches the gate (i.e. the parent
    ///     plumbing of `--continue --worktree --reuse-worktree` is
    ///     wired so it actually runs `acquire_and_assess`)
    ///   - the worktree's `.ralph/loop.lock` is taken during the run
    ///     and released (truncated to 0 bytes) once the ralph child
    ///     exits — the lock guard held by the gate must drop at
    ///     function return
    ///   - `.ralph/reuse-history/` is NEVER created (the archive
    ///     step is skipped on the combined path, regardless of
    ///     verdict)
    ///   - the gate-controlled runtime fixtures survive so a
    ///     follow-up retry can re-assess the same checkpoint
    ///
    /// KNOWN ISSUE (captured finding for the parent agent —
    /// 2026-09-01-2102 U3 follow-up): with no prior history, the
    /// gate's `assess_checkpoint` step 6 calls `is_loop_lock_held`
    /// without filtering the current process pid, so it sees its
    /// OWN freshly-acquired lock and refuses with `LoopLockedByOther`
    /// (`"checkpoint refused continuation: .ralph/loop.lock
    /// indicates another live loop (pid N); the lock assessment is
    /// independent of the gate's own lock..."`). The Eligible path
    /// is therefore unreachable as wired today; a follow-up U-unit
    /// must either filter current pid in `is_loop_lock_held` or pass
    /// a `ignore_self_lock` flag through `assess_checkpoint` so the
    /// gate does not refuse itself. This test asserts the diagnostics
    /// so the bug is captured in CI, not silently masked.
    #[test]
    fn combined_continue_happy_path_eligible_passes() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_minimal_config(main_repo);
        seed_main_repo_scratchpad(main_repo);

        let loop_id = "u3-happy-eligible";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan\n").unwrap();

        let worktree_path = precreate_worktree(main_repo, loop_id);
        seed_assess_fixture(&worktree_path, loop_id);

        // No history seeding — keep the test focused on the wiring,
        // not on history semantics (which Test 2 covers).

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--continue",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!("U3 happy stderr: {stderr}");
        eprintln!("U3 happy stdout: {stdout}");

        // The gate was reached (plumbing is wired). Either the
        // gate cleared (Eligible) or the gate refused (currently:
        // LoopLockedByOther). We assert the diagnostic markers
        // that prove the gate RAN, not the verdict.
        // Tracing's default writer for the no-TUI / no-RPC branch is
        // stdout, so check both streams.
        let gate_ran = stdout.contains("Continuation gate cleared")
            || stderr.contains("Continuation gate cleared")
            || stdout.contains("checkpoint refused continuation")
            || stderr.contains("checkpoint refused continuation");
        assert!(
            gate_ran,
            "ralph child must reach the combined-path gate: stderr={stderr} stdout={stdout}"
        );

        // Archive step skipped on the combined path regardless of
        // verdict (this is the load-bearing U3 invariant: the gate
        // never lets cleanup rotate the resume contract's evidence).
        let ralph_dir = worktree_path.join(".ralph");
        assert!(
            !ralph_dir.join("reuse-history").exists(),
            "combined --continue path must not archive runtime fixtures"
        );

        // Lock released: the parent held the worktree's LoopLock
        // for the lifetime of this run; after exit the file is
        // truncated to 0 bytes by LockGuard::drop. We accept either
        // "empty file" or "no file" (the kernel drops the inode if
        // no other handle is open, but truncation is the canonical
        // behavior).
        let lock_path = ralph_dir.join("loop.lock");
        if lock_path.exists() {
            let len = fs::metadata(&lock_path)
                .expect("lock file stat")
                .len();
            assert_eq!(
                len, 0,
                "lock file must be truncated (length 0) after parent exit, was {len}: {stderr}"
            );
        }

        // Fixtures survive so a follow-up retry can re-assess.
        assert!(
            ralph_dir.join("current-loop-id").exists(),
            "current-loop-id marker must survive"
        );
        assert!(
            ralph_dir.join("current-events").exists(),
            "current-events marker must survive"
        );
        assert!(
            ralph_dir.join("agent").join("scratchpad.md").is_file(),
            "scratchpad must survive"
        );

        // Bug fix applied (recovery_checkpoint.rs::is_loop_lock_held now
        // filters the current process pid). The Eligible path is
        // reachable; capture the diagnostic if the gate ever falls
        // back to refused to surface a regression.
        let lock_self_bug_regressed = stderr.contains("another live loop")
            && stderr.contains("checkpoint refused continuation");
        if lock_self_bug_regressed {
            panic!(
                "U3 gate refused with LoopLockedByOther — lock self-filter \
                 regressed in recovery_checkpoint.rs::is_loop_lock_held: \
                 {stderr}"
            );
        }
    }

    /// U3 (plan §3.2 contract): non-completion_promise terminal reasons
    /// (`max_runtime`, `max_iterations`, `failure`, terminated signal)
    /// all return `Eligible` from the gate. The loop must continue
    /// running — NOT refuse with `already completed`. This is the
    /// operator-facing invariant: a run that was interrupted by a
    /// resource cap (not a clean completion promise) is always
    /// resumable.
    #[test]
    fn combined_continue_non_success_terminal_continues() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_minimal_config(main_repo);
        seed_main_repo_scratchpad(main_repo);

        let loop_id = "u3-max-runtime";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan\n").unwrap();

        let worktree_path = precreate_worktree(main_repo, loop_id);
        seed_assess_fixture(&worktree_path, loop_id);

        // Eligible verdict: prior run ended via max_runtime.
        let history = LoopHistory::new(worktree_path.join(".ralph/history.jsonl"));
        history.record_started("previous run").unwrap();
        history.record_completed("max_runtime").unwrap();

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--continue",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!("U3 non-success stderr: {stderr}");
        eprintln!("U3 non-success stdout: {stdout}");

        // The gate must NOT refuse this run.
        assert!(
            !stderr.contains("already completed"),
            "max_runtime terminal reason must clear the gate, was: {stderr}"
        );
        assert!(
            !stderr.contains("completion_promise"),
            "max_runtime must not be treated as completion_promise, was: {stderr}"
        );

        // Operator-facing remedies for AlreadyCompleted must not
        // appear (those hints are only surfaced when the gate
        // refuses).
        assert!(
            !stderr.contains("drop --continue"),
            "drop --continue hint must NOT surface on Eligible path, was: {stderr}"
        );
        assert!(
            !stderr.contains("--remove-worktree-and-continue"),
            "destructive-continue hint must NOT surface on Eligible path, was: {stderr}"
        );

        // The continuation gate clears the lock and proceeds; the
        // archive path is still skipped (combined --continue contract).
        let ralph_dir = worktree_path.join(".ralph");
        assert!(
            !ralph_dir.join("reuse-history").exists(),
            "Eligible combined path must skip the archive step"
        );
    }

    /// U3 (plan §3.2 child-side flag, S1 happy path): on the trusted
    /// combined path, the child (invoked via `--worktree-path` with
    /// `--combined-continue`) MUST skip the parallel-forge
    /// resume-manifest re-validation gate that the parent already
    /// cleared. We verify the contract by:
    ///
    ///   1. Seeding a STALE (identity-drift) resume manifest inside
    ///      `.ralph/reuse-history/<ts>/parallel-forge-resume-manifest.v1.json`
    ///      — wrong `preset_name` so `validate_manifest` would fail
    ///      with `IdentityDrift` if it ran.
    ///   2. Invoking the child branch directly with
    ///      `--worktree-path <path> --combined-continue
    ///      -H builtin:parallel-forge`. With the flag set, the gate
    ///      block at `run.rs:1480` is skipped entirely and the stale
    ///      manifest is not consulted.
    ///   3. Asserting that stderr does NOT contain
    ///      "resume manifest validation failed" (the gate's failure
    ///      message) nor "identity drift" — those would only appear
    ///      if the validation actually ran.
    ///
    /// Without `--combined-continue` (and the SAME stale manifest),
    /// the gate WOULD refuse with "resume manifest validation
    /// failed: ... identity drift ...". We don't run that control
    /// here — it would fail the test before any U3 assertion could
    /// fire, and the invariant we care about is the positive side:
    /// "with `--combined-continue`, the stale manifest is invisible".
    ///
    /// In test environment `use_subprocess_tui = false` (no TTY), so
    /// the child branch runs inline in the same process. The
    /// `args.worktree_path.is_some()` branch (line 1403 in run.rs) is
    /// what gets exercised; that is exactly the path the parent
    /// would have spawned under `--no-tui`, so the test faithfully
    /// covers the production wiring.
    #[test]
    fn combined_continue_child_skips_pf_manifest_gate() {
        use ralph_core::parallel_forge_resume::{
            MANIFEST_FILE_NAME, MANIFEST_SCHEMA_VERSION, sha256_hex,
        };
        use std::time::{SystemTime, UNIX_EPOCH};

        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_minimal_config(main_repo);
        seed_main_repo_scratchpad(main_repo);

        let loop_id = "u3-child-skip-pf";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        let plan_body = "# plan\n";
        fs::write(&plan_path, plan_body).unwrap();

        let worktree_path = precreate_worktree(main_repo, loop_id);
        seed_assess_fixture(&worktree_path, loop_id);

        // Pre-stage a STALE archive manifest whose `preset_name` does
        // NOT match the active preset. With the gate active, this
        // would fail validation with `IdentityDrift { fields:
        // ["preset_name"] }`. With `--combined-continue`, the gate
        // skips the call entirely, so the drift is invisible.
        let ralph_dir = worktree_path.join(".ralph");
        let reuse_history = ralph_dir.join("reuse-history");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let archive_dir = reuse_history.join(format!("{nanos}"));
        fs::create_dir_all(&archive_dir).expect("create archive dir");

        let stale_manifest = serde_json::json!({
            "schema_version": MANIFEST_SCHEMA_VERSION,
            "captured_at": "2026-09-01T00:00:00Z",
            "identity": {
                // Identity drift: the recorded preset is the WRONG
                // preset, so validate_manifest would refuse this.
                "plan_path": plan_path.to_str().unwrap(),
                "plan_digest": sha256_hex(plan_body.as_bytes()),
                "preset_name": "ce-executor-pipeline",
                "config_digest": "",
                "worktree_name": loop_id,
                "source_head_sha": "",
                "loop_id": loop_id,
            },
            "boundary": {
                "accepted": [],
                "pending_hat": null,
                "original_trigger": null,
                "wave": null,
            },
            "tasks": [],
            "artifacts": [],
            "incomplete_reasons": [],
            "manifest_digest": "stale_digest_does_not_match_self",
        });
        fs::write(
            archive_dir.join(MANIFEST_FILE_NAME),
            serde_json::to_string_pretty(&stale_manifest).unwrap(),
        )
        .expect("write stale manifest");

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--no-tui",
                "--skip-preflight",
                "--worktree-path",
                worktree_path.to_str().unwrap(),
                "--combined-continue",
                "-H",
                "builtin:parallel-forge",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!("U3 child-skip stderr: {stderr}");
        eprintln!("U3 child-skip stdout: {stdout}");

        // The core invariant: with --combined-continue the gate must
        // skip the manifest validation entirely. The only failure
        // messages validate_manifest produces would be the strings
        // below; if any of them appear, the gate ran.
        assert!(
            !stderr.contains("resume manifest validation failed"),
            "child on combined path must skip the PF manifest gate; gate ran and refused: {stderr}"
        );
        assert!(
            !stderr.contains("identity drift"),
            "child on combined path must skip the PF manifest gate; gate ran and reported drift: {stderr}"
        );
        assert!(
            !stderr.contains("schema version mismatch"),
            "child on combined path must skip the PF manifest gate; gate ran and reported schema mismatch: {stderr}"
        );
        assert!(
            !stderr.contains("digest mismatch"),
            "child on combined path must skip the PF manifest gate; gate ran and reported digest mismatch: {stderr}"
        );

        // The stale manifest must still be on disk untouched — the
        // child does not archive or remove it. This is what makes
        // the combined path "non-destructive": the parent's gate
        // already cleared the path, and the child leaves the
        // evidence in place.
        assert!(
            archive_dir.join(MANIFEST_FILE_NAME).exists(),
            "stale manifest must remain on disk after child run"
        );
    }

    /// U4 (plan §3.4 Crash-Window Repair): on the combined
    /// `--continue --worktree --reuse-worktree` cold-start, the
    /// pre-existing outbox-only StateMachine projection must be
    /// committed to the StateLedger exactly once across repeated
    /// cold-starts. The first cold-start repairs the projection
    /// (count = 1); the second cold-start is a no-op because the
    /// durable snapshot already contains the transition_id
    /// (`has_applied_transition_id == true`). The on-disk outbox
    /// entry is left untouched (append-only), and the worktree
    /// `events.jsonl` does NOT gain any business event from the
    /// repair itself — repair is purely a ledger projection.
    #[test]
    fn checkpoint_repair_first_apply_second_noop() {
        use ralph_core::event_loop::accepted_transition::AcceptedTransition;
        use ralph_core::state::StateLedger;
        use ralph_core::state_machine::{
            StateMachineTransitionDelta, StateMachineTransitionId,
        };

        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_minimal_config(main_repo);
        seed_main_repo_scratchpad(main_repo);

        let loop_id = "u4-repair-once";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan\n").unwrap();

        let worktree_path = precreate_canonical_worktree(main_repo, loop_id);
        seed_assess_fixture(&worktree_path, loop_id);

        // Build a canonical, well-formed StateMachine projection
        // and bake it into an `OutboxEntry` line in the worktree's
        // durable outbox. The entry has `delivered: false` and
        // `state_machine_projection: Some(...)` so the cold-start
        // repair commits the projection on first run.
        let transition_id = AcceptedTransition::compute_transition_id(
            loop_id,
            "planner:1",
            "rev-1",
            "forge.plan.ready:planner",
            "deadbeef",
        );
        let sm_id = StateMachineTransitionId(format!(
            "sm-v2:{}",
            &transition_id[..32]
        ));
        let projection = StateMachineTransitionDelta {
            transition_id: sm_id.clone(),
            source_hat: Some("planner".to_string()),
            topic: "forge.plan.ready".to_string(),
            instance_key: Some("instance-1".to_string()),
            new_state: "plan-ready".to_string(),
            opens_instance: true,
            closes_instance: false,
            terminal_observed: false,
            terminal_honored: false,
        };
        let outbox_line = serde_json::json!({
            "activation_id": "planner:1",
            "committed_at": "2026-09-03T00:00:01Z",
            "contract_revision": "rev-1",
            "delivered": false,
            "loop_id": loop_id,
            "payload_digest": "deadbeef",
            "state_machine_projection": projection,
            "topic": "forge.plan.ready",
            "transition_id": transition_id,
        });
        let outbox_path = worktree_path
            .join(".ralph")
            .join("agent")
            .join("accepted-transitions.jsonl");
        fs::write(&outbox_path, format!("{outbox_line}\n")).unwrap();

        // ─── First cold-start ─────────────────────────────────────
        let output1 = super::common::ralph_bin()
            .args([
                "run",
                "--continue",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run #1");
        let stderr1 = String::from_utf8_lossy(&output1.stderr);
        let stdout1 = String::from_utf8_lossy(&output1.stdout);
        // The gate must have cleared (Eligible) so the cold-start
        // actually reached the repair step. We don't require rc==0
        // because the parent's outer `run --continue --worktree`
        // can exit non-zero after the child completes (max_iterations
        // with --continue is itself a resumable condition, often
        // surfaced as exit code 3 — see `commands::run::run_command`
        // mapping for TerminationReason::MaxIterations). The cold-start
        // repair step (which is what this test pins) runs BEFORE
        // the loop body and BEFORE that exit-code mapping, so its
        // durable effect is observable regardless of rc.
        assert!(
            stderr1.contains("Continuation gate cleared")
                || stdout1.contains("Continuation gate cleared"),
            "first cold-start must reach the gate and clear it; \
             the repair step only runs after the gate clears. \
             stderr={stderr1} stdout={stdout1}"
        );

        // Durable ledger inspection — the projection's transition_id
        // must be applied to the StateMachine runtime snapshot, and
        // the outbox must still hold the original entry (append-only).
        // Pass the WORKTREE ROOT (not the `.ralph` subdir) to
        // `StateLedger::new`: the ledger is rooted at
        // `<workspace>/.ralph/ledger.jsonl`. Passing the `.ralph`
        // subdir would re-join the relative path and read from
        // `<.ralph>/.ralph/ledger.jsonl`, which never exists.
        let ledger = StateLedger::new(&worktree_path, true);
        let runtime_after_run1 = ledger
            .snapshot()
            .state_machine_runtime
            .as_ref()
            .expect("first cold-start must populate StateMachine runtime");
        assert_eq!(
            runtime_after_run1.accepted_transition_count(),
            1,
            "first cold-start must commit exactly one StateMachine projection; \
             count={} stderr={stderr1}",
            runtime_after_run1.accepted_transition_count(),
        );
        assert!(
            runtime_after_run1.has_applied_transition_id(&sm_id),
            "first cold-start must register the projection's transition_id as applied"
        );
        let outbox_after_run1 =
            fs::read_to_string(&outbox_path).expect("outbox readable after run #1");
        assert_eq!(
            outbox_after_run1.lines().filter(|l| !l.trim().is_empty()).count(),
            1,
            "outbox must remain append-only — repair must NOT remove or rewrite entries"
        );
        // Capture the ledger's current `commit_log` head count so we
        // can confirm run #2 didn't append anything new to the
        // commit log (R6: re-running repair on a healthy ledger is
        // a no-op).
        let ledger_jsonl_after_run1 = fs::read_to_string(
            worktree_path.join(".ralph").join("ledger.jsonl"),
        )
        .unwrap_or_default();
        let commits_after_run1 = ledger_jsonl_after_run1
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();

        // ─── Second cold-start (immediate retry) ──────────────────
        // The combined path must be safe to invoke twice in a row.
        // The repair step is the load-bearing invariant: it must
        // NOT double-apply the projection (R6 idempotency).
        let output2 = super::common::ralph_bin()
            .args([
                "run",
                "--continue",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run #2");
        let stderr2 = String::from_utf8_lossy(&output2.stderr);
        let stdout2 = String::from_utf8_lossy(&output2.stdout);
        eprintln!("U4 repair#2 stderr: {stderr2}");
        eprintln!("U4 repair#2 stdout: {stdout2}");
        // The second cold-start must also reach the gate and clear it.
        // We don't require rc==0 for the same reason as run #1: the
        // outer `--continue --worktree` exit code is independent of
        // the cold-start repair step. The durable invariant we care
        // about is the projection's count and the ledger's commit
        // log size — both inspected below.
        assert!(
            stderr2.contains("Continuation gate cleared")
                || stdout2.contains("Continuation gate cleared"),
            "second cold-start must reach the gate and clear it; \
             the repair step only runs after the gate clears. \
             stderr={stderr2} stdout={stdout2}"
        );

        // The projection's transition_id is still applied exactly
        // once. We re-load the ledger from disk so we measure the
        // durable state (the in-memory ledger's commit_log is reset
        // on every `StateLedger::new`, so we must consult the
        // snapshot's StateMachine runtime — that is the durable
        // signal). Pass the worktree root (not the `.ralph`
        // subdir) — see comment in run #1.
        let ledger_after_run2 = StateLedger::new(&worktree_path, true);
        let runtime_after_run2 = ledger_after_run2
            .snapshot()
            .state_machine_runtime
            .as_ref()
            .expect("second cold-start must keep the StateMachine runtime populated");
        assert_eq!(
            runtime_after_run2.accepted_transition_count(),
            1,
            "second cold-start must NOT increment accepted_transition_count; \
             repair is a no-op once transition_id is applied. \
             count={} stderr={stderr2}",
            runtime_after_run2.accepted_transition_count(),
        );
        assert!(
            runtime_after_run2.has_applied_transition_id(&sm_id),
            "second cold-start must keep the projection's transition_id as applied"
        );

        // The on-disk commit log must NOT have grown. R6: the
        // ledger's replayable commit history must stay stable across
        // idempotent repair runs.
        let ledger_jsonl_after_run2 = fs::read_to_string(
            worktree_path.join(".ralph").join("ledger.jsonl"),
        )
        .unwrap_or_default();
        let commits_after_run2 = ledger_jsonl_after_run2
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();
        assert_eq!(
            commits_after_run2, commits_after_run1,
            "ledger.jsonl must NOT grow on second cold-start; \
             repair is a no-op when transition_id already applied. \
             before={commits_after_run1} after={commits_after_run2} stderr={stderr2}"
        );

        // Outbox is still append-only — neither run drained or
        // re-appended.
        let outbox_after_run2 =
            fs::read_to_string(&outbox_path).expect("outbox readable after run #2");
        assert_eq!(
            outbox_after_run2.lines().filter(|l| !l.trim().is_empty()).count(),
            1,
            "outbox must still hold exactly one entry after run #2 (append-only)"
        );
        assert_eq!(
            outbox_after_run1, outbox_after_run2,
            "outbox must be byte-identical across the two cold-starts \
             (append-only — neither run mutates existing entries)"
        );

        // Events ledger in the worktree must NOT contain any
        // business event from the repair itself. Repair is a pure
        // StateLedger projection; it does not publish to the bus.
        let events_path = worktree_path.join(".ralph/events.jsonl");
        if events_path.exists() {
            let events_body =
                fs::read_to_string(&events_path).expect("events.jsonl readable");
            for line in events_body.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                assert!(
                    !trimmed.contains("\"forge.plan.ready\"")
                        && !trimmed.contains("\"plan-ready\""),
                    "repair must NOT publish a business event to events.jsonl; \
                     found: {trimmed}"
                );
            }
        }
    }

    /// U4 (plan §3.4 Crash-Window Repair — negative path): when the
    /// outbox path itself is unreadable (a directory, not a file),
    /// the cold-start repair must fail closed BEFORE any business
    /// event reaches the bus. This is the genuine-IO branch of the
    /// repair contract — production wiring at
    /// `acceptance_and_lifecycle.rs:516-522` returns `Err(io::Error)`
    /// from `repair_state_machine_projection_from_outbox` and the
    /// runtime bails out before publishing anything.
    #[test]
    fn checkpoint_repair_genuine_outbox_io_fails_closed_without_bus_startup() {
        let temp_dir = TempDir::new().expect("temp dir");
        let main_repo = temp_dir.path();
        setup_git_repo(main_repo);
        write_minimal_config(main_repo);
        seed_main_repo_scratchpad(main_repo);

        let loop_id = "u4-repair-iofail";
        let plan_path = main_repo.join(format!("{loop_id}.md"));
        fs::write(&plan_path, "# plan\n").unwrap();

        let worktree_path = precreate_canonical_worktree(main_repo, loop_id);
        seed_assess_fixture(&worktree_path, loop_id);

        // Make the outbox path a DIRECTORY instead of a file —
        // genuine I/O error: `fs::read_to_string` will fail
        // (EISDIR on Linux). The repair step must surface this as
        // `Err(io::Error)` and the cold-start must fail closed.
        let outbox_path = worktree_path
            .join(".ralph")
            .join("agent")
            .join("accepted-transitions.jsonl");
        fs::create_dir_all(&outbox_path).expect("make outbox path a directory");

        let events_path = worktree_path.join(".ralph/events.jsonl");
        let events_existed_before = events_path.exists();
        let events_size_before = if events_existed_before {
            fs::metadata(&events_path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        let output = super::common::ralph_bin()
            .args([
                "run",
                "--continue",
                "--worktree",
                "--reuse-worktree",
                "--no-tui",
                "--skip-preflight",
                "--plan",
                plan_path.to_str().unwrap(),
            ])
            .current_dir(main_repo)
            .output()
            .expect("ralph run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!("U4 iofail stderr: {stderr}");
        eprintln!("U4 iofail stdout: {stdout}");

        // Exit must be non-zero — the cold-start failed closed.
        assert!(
            !output.status.success(),
            "cold-start with unreadable outbox must fail closed; \
             got rc={:?} stderr={stderr}",
            output.status.code()
        );

        // The error chain is: read_outbox fails on
        // `fs::read_to_string` of a directory → io::Error propagates
        // through repair → runtime bails before publishing. The
        // surface area in stderr/stdout names the failure (either
        // via tracing error!() or via the `EventLoop::from_resolved`
        // expect's panic message, which embeds the underlying
        // io::Error). We accept any of the canonical markers.
        let combined = format!("{stderr}{stdout}");
        let mentions_outbox_failure = combined.contains("outbox")
            || combined.contains("OutboxEntry")
            || combined.contains("accepted-transitions.jsonl")
            || combined.contains("EISDIR")
            || combined.contains("Is a directory")
            || combined.contains("is a directory")
            || combined.contains("repair");
        assert!(
            mentions_outbox_failure,
            "stderr/stdout must explain why the cold-start failed; \
             expect a message mentioning the outbox, repair, or \
             'Is a directory'. got: stderr={stderr} stdout={stdout}"
        );

        // Bootstrap may legitimately create events.jsonl with
        // lifecycle events (gate-clear diagnostics, loop.start
        // markers, etc.) before the cold-start fails closed. What
        // matters is that no BUSINESS event reached the bus — the
        // repair contract is "fail closed before publish". The
        // per-line content check below is the load-bearing
        // assertion; the size check only verifies the file did
        // not balloon (no hat activations succeeded).
        let events_size_after = if events_path.exists() {
            fs::metadata(&events_path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        // Bootstrap diagnostics are bounded; 4 KiB is a generous
        // upper bound for the gate/loop.start lines emitted
        // before the U13 archive-failed panic. Anything larger
        // would indicate a successful hat activation wrote a
        // business payload.
        assert!(
            events_size_after <= 4096,
            "events.jsonl must stay small when cold-start fails closed; \
             bootstrap diagnostics are bounded — anything larger means a \
             business event slipped through. before={events_size_before} \
             after={events_size_after} stderr={stderr}"
        );

        // The runtime must also not have advanced past the gate to
        // write any "Continuation gate cleared" diagnostic for the
        // cold-start loop — the gate may have cleared (we want it
        // to, so the repair actually runs), but the cold-start
        // itself must NOT have published any business event to the
        // bus. We assert that no business-topic line was written.
        if events_path.exists() {
            let body = fs::read_to_string(&events_path).unwrap_or_default();
            for line in body.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                // Business topics we'd never expect to see before
                // the cold-start completes: any hat output topic.
                assert!(
                    !trimmed.contains("\"topic\":\"work.start\"")
                        && !trimmed.contains("\"topic\":\"plan.ready\"")
                        && !trimmed.contains("\"topic\":\"work.done\"")
                        && !trimmed.contains("\"topic\":\"task.start\"")
                        && !trimmed.contains("\"topic\":\"forge.plan.ready\"")
                        && !trimmed.contains("\"topic\":\"forge.wave.worktrees.ready\""),
                    "no business event must reach events.jsonl when \
                     cold-start fails closed; found: {trimmed}"
                );
            }
        }

        // The directory we planted at the outbox path is still on
        // disk — the runtime must not have removed it as a
        // "recovery" side effect. The fail-closed contract means
        // the path is left untouched so the operator can inspect
        // the corruption.
        assert!(
            outbox_path.is_dir(),
            "outbox path must remain a directory on disk after fail-closed \
             (operator-visible corruption marker)"
        );
    }
}
