mod common;

use anyhow::Result;
use std::fs;
use tempfile::TempDir;

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
fn test_continue_publishes_task_resume_event() -> Result<()> {
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

    // Check that the event log contains task.resume instead of task.start
    // Events are now stored in .ralph/ directory, read path from marker file
    let marker_path = ralph_dir.join("current-events");
    if marker_path.exists() {
        let events_path = fs::read_to_string(&marker_path)?.trim().to_string();
        let events_file = temp_path.join(&events_path);
        if events_file.exists() {
            let events_content = fs::read_to_string(&events_file)?;

            // Should contain task.resume event
            assert!(events_content.contains("task.resume"));

            // Should NOT contain task.start event (since this is continue mode)
            assert!(!events_content.contains("task.start"));
        }
    }

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

    // Verify the difference:
    // - run should have task.start
    // - continue should ADD task.resume to the same file
    if !run_events.is_empty() {
        assert!(
            run_events.contains("task.start"),
            "Run should produce task.start event"
        );
    }

    // After continue, the file should contain both task.start (from run) and task.resume (from continue)
    if !continue_events.is_empty() {
        assert!(
            continue_events.contains("task.start"),
            "Events file should still contain task.start from the run"
        );
        assert!(
            continue_events.contains("task.resume"),
            "Events file should now also contain task.resume from the continue"
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
            &ralph_dir.join("loops.json"),
            serde_json::to_string_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    /// Pre-create a git-known worktree whose prior run stopped at an
    /// accepted `forge.plan.ready` boundary.
    fn precreate_worktree_with_accepted_boundary(main_repo: &Path, loop_id: &str) -> PathBuf {
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
}

// ─────────────────────────────────────────────────────────────────────────
// 2026-08-03-004 U2: manifest-driven resume bootstrap (target hat
// resume payload + parallel-forge hat handoff). The reuse gate (U1)
// already validated the manifest; U2 consumes it at loop bootstrap and
// re-binds the pending hat to its original trigger through the EXISTING
// `task.resume` recovery contract.
// ─────────────────────────────────────────────────────────────────────────

mod reuse_resume_bootstrap {
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
            &ralph_dir.join("loops.json"),
            serde_json::to_string_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    /// Pre-create a git-known worktree whose prior run stopped right
    /// after the accepted `forge.plan.ready` boundary (pending hat:
    /// `guardian`).
    fn precreate_worktree_with_accepted_boundary(main_repo: &Path, loop_id: &str) -> PathBuf {
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
