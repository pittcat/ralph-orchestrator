//! 2026-07-25-005 plan U11: integration tests for `ralph wave redrive`.
//!
//! Tests the CLI surface:
//! - `--help` text does not contain zero-disk / zero ledger confirm
//! - creates child wave for failed-parent
//! - rejects done parent
//! - idempotent on repeat call

#[path = "common/mod.rs"]
mod common;

use ralph_core::supervisor::SupervisorStore;
use tempfile::TempDir;

fn write_minimal_ralph_yml(workspace: &std::path::Path) {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: false
    mode: enforce
hats:
  coordinator:
    name: "Coordinator"
    publishes:
      - review.wave.ready
"#;
    std::fs::write(workspace.join("ralph.yml"), yaml).unwrap();
    std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
}

fn run_ralph(
    workspace: &std::path::Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> (i32, String, String) {
    let mut cmd = common::ralph_bin();
    cmd.current_dir(workspace);
    cmd.args(args);
    for (k, v) in extra_env {
        cmd.env(*k, *v);
    }
    let output = cmd.output().expect("ralph invocation must succeed");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

// redrive_help_bans_zero_disk_and_ledger_confirm
// `--help` text must not contain "zero-disk" or "zero ledger".
#[test]
fn redrive_help_bans_zero_disk_and_ledger_confirm() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let (code, stdout, stderr) = run_ralph(ws, &["wave", "redrive", "--help"], &[]);
    assert_eq!(code, 0, "help should exit successfully");
    let combined = format!("{}\n{}", stdout, stderr);
    assert!(
        !combined.to_lowercase().contains("zero-disk"),
        "help text must not mention zero-disk: {}",
        combined
    );
    assert!(
        !combined.to_lowercase().contains("zero ledger"),
        "help text must not mention zero ledger: {}",
        combined
    );
}

// redrive_creates_child_wave_for_failed_parent
// Register a wave, mark a slot failed, run redrive, assert child wave returned.
#[test]
fn redrive_creates_child_wave_for_failed_parent() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);

    // Create a temp DB path.
    let db_path = ws.join(".ralph/supervisor.db");

    // Use the store directly to set up a wave with a failed slot.
    let store =
        ralph_core::supervisor::RusqliteSupervisorStore::open(&db_path).expect("store must open");

    // Register a wave with 3 slots.
    let parent_id = store
        .register_wave(
            "redrive-test-parent",
            ralph_core::supervisor::WaveKind::Exec,
            3,
            1,
        )
        .expect("register_wave must succeed");

    // Bind worktrees for all slots.
    for i in 0..3u32 {
        let resource = ralph_core::supervisor::SlotResource {
            slot_index: i,
            worktree_path: Some(format!(".ralph/wt/{i}")),
            branch: Some(format!("ralph/u{i}")),
        };
        store
            .bind_worktree(&parent_id, i, resource)
            .expect("bind_worktree must succeed");
    }

    // Mark slot 1 as failed.
    store
        .record_slot_failure(&parent_id, 1, "test failure")
        .expect("record_slot_failure must succeed");

    // Run `ralph wave redrive --wave-id <parent> --output json`.
    let (code, stdout, stderr) = run_ralph(
        ws,
        &[
            "wave",
            "redrive",
            "--wave-id",
            &parent_id,
            "--output",
            "json",
        ],
        &[("RALPH_EMISSION_STORE_PATH", db_path.to_str().unwrap())],
    );

    assert_eq!(
        code, 0,
        "redrive should succeed; stderr={}, stdout={}",
        stderr, stdout
    );

    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert!(
        parsed.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "ok should be true: {}",
        stdout
    );
    assert!(
        !parsed["child_wave_id"].as_str().unwrap().is_empty(),
        "child_wave_id must not be empty: {}",
        stdout
    );
    assert_eq!(
        parsed["parent_wave_id"].as_str().unwrap(),
        parent_id,
        "parent_wave_id must match: {}",
        stdout
    );
    assert_eq!(
        parsed["attempt_epoch"].as_i64().unwrap(),
        1,
        "attempt_epoch must be 1: {}",
        stdout
    );
}

// redrive_rejects_done_parent
// Mark all slots completed and advance phase to Done, then redrive → non-zero.
#[test]
fn redrive_rejects_done_parent() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);

    let db_path = ws.join(".ralph/supervisor.db");
    let store =
        ralph_core::supervisor::RusqliteSupervisorStore::open(&db_path).expect("store must open");

    let parent_id = store
        .register_wave(
            "redrive-done-parent",
            ralph_core::supervisor::WaveKind::Exec,
            2,
            1,
        )
        .expect("register_wave must succeed");

    // Complete all slots.
    for i in 0..2u32 {
        store
            .record_slot_result(&parent_id, i, &format!("hash{i}"), 1)
            .expect("record_slot_result must succeed");
    }

    // Advance phase to Done.
    store
        .set_wave_phase(&parent_id, ralph_core::supervisor::WavePhase::Done)
        .expect("set_wave_phase must succeed");

    let (code, stdout, stderr) = run_ralph(
        ws,
        &["wave", "redrive", "--wave-id", &parent_id],
        &[("RALPH_EMISSION_STORE_PATH", db_path.to_str().unwrap())],
    );

    assert_ne!(
        code, 0,
        "redrive on done parent should fail; got code={} stdout={} stderr={}",
        code, stdout, stderr
    );
}

// redrive_idempotent_on_repeat_call
// Calling redrive twice with the same parent returns the same child wave_id.
#[test]
fn redrive_idempotent_on_repeat_call() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);

    let db_path = ws.join(".ralph/supervisor.db");
    let store =
        ralph_core::supervisor::RusqliteSupervisorStore::open(&db_path).expect("store must open");

    let parent_id = store
        .register_wave(
            "redrive-idempotent",
            ralph_core::supervisor::WaveKind::Exec,
            2,
            1,
        )
        .expect("register_wave must succeed");

    for i in 0..2u32 {
        let resource = ralph_core::supervisor::SlotResource {
            slot_index: i,
            worktree_path: Some(format!(".ralph/wt/{i}")),
            branch: Some(format!("ralph/u{i}")),
        };
        store
            .bind_worktree(&parent_id, i, resource)
            .expect("bind_worktree must succeed");
    }

    // Fail slot 0.
    store
        .record_slot_failure(&parent_id, 0, "test failure")
        .expect("record_slot_failure must succeed");

    let env = &[("RALPH_EMISSION_STORE_PATH", db_path.to_str().unwrap())];

    // First call.
    let (code1, stdout1, _) = run_ralph(
        ws,
        &[
            "wave",
            "redrive",
            "--wave-id",
            &parent_id,
            "--output",
            "json",
        ],
        env,
    );
    assert_eq!(code1, 0, "first redrive should succeed: {}", stdout1);
    let parsed1: serde_json::Value =
        serde_json::from_str(&stdout1).expect("first stdout must be valid JSON");

    // Second call — same child wave_id.
    let (code2, stdout2, _) = run_ralph(
        ws,
        &[
            "wave",
            "redrive",
            "--wave-id",
            &parent_id,
            "--output",
            "json",
        ],
        env,
    );
    assert_eq!(code2, 0, "second redrive should succeed: {}", stdout2);
    let parsed2: serde_json::Value =
        serde_json::from_str(&stdout2).expect("second stdout must be valid JSON");

    assert_eq!(
        parsed1["child_wave_id"].as_str().unwrap(),
        parsed2["child_wave_id"].as_str().unwrap(),
        "idempotent redrive should return same child_wave_id; first={} second={}",
        stdout1,
        stdout2
    );
}
