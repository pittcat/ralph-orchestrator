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
        super::common::ralph_bin()
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
