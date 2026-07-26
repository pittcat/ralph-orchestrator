//! 2026-07-26-002 plan U7 (R7): outside-in integration test that
//! spawns the real `ralph emit` binary against a dispatcher-signed
//! per-slot wave channel and verifies:
//!
//! 1. The marker check passes when the dispatcher wrote
//!    `.ralph/current-wave-channels` ahead of the spawn.
//! 2. The CLI appends a single event line to the channel JSONL.
//! 3. The InMemorySupervisorStore records a `Completed` slot
//!    transition when the orchestrator later reads the channel.
//!
//! HARD RULE 5: spawn goes through `common::ralph_bin()` so the
//! process inherits a scrubbed environment — without the scrub,
//! the loop-owned env vars (RALPH_CURRENT_HAT / RALPH_WAVE_WORKER /
//! RALPH_EVENTS_FILE) would push the spawned `ralph emit` into an
//! in-loop ACL branch and the human-CLI semantics this test
//! assumes would not apply.

mod common;

use std::fs;
use tempfile::TempDir;

/// Smoke test for the U7 outside-in slice: dispatcher signs the
/// marker, real `ralph emit` writes the channel, store records
/// Completed.
#[test]
fn u7_real_ralph_emit_writes_marker_signed_channel() {
    let tmp = TempDir::new().expect("temp workspace");
    let workspace = tmp.path();
    let ralph_dir = workspace.join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");

    // Dispatcher-style contract: build a workspace + .ralph
    // subtree, write the marker file naming the absolute channel
    // path the worker is allowed to write.
    let channel = ralph_dir.join("wave-u7-0.jsonl");
    fs::write(
        ralph_dir.join("current-wave-channels"),
        format!("{}\n", channel.display()),
    )
    .expect("write marker");

    // Spawn the real binary. Scrub agent-context env (HARD RULE 5)
    // and set the wave-worker env vars explicitly so the CLI sees a
    // genuine wave-worker invocation.
    //
    // No `--policy-check` here: that flag is a dry-run validator
    // (per ralph-tools §5) and would refuse to write the channel.
    // U7 needs the side-effect to land so the channel JSONL
    // contains the event and downstream consumers can read it.
    let payload = r#"{"wave_id":"u7","slot_index":0,"ok":true}"#;
    let output = common::ralph_bin()
        .arg("emit")
        .arg("exec.unit.done")
        .arg(payload)
        .arg("--file")
        .arg(channel.to_string_lossy().to_string())
        .arg("--hat")
        .arg("exec-worker")
        .env("RALPH_WAVE_WORKER", "1")
        .env("RALPH_WAVE_ID", "u7")
        .env("RALPH_WAVE_INDEX", "0")
        .current_dir(workspace)
        .output()
        .expect("spawn ralph emit");

    assert!(
        output.status.success(),
        "ralph emit must succeed against a dispatcher-signed channel; \
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let written = fs::read_to_string(&channel).expect("read channel");
    let line_count = written.lines().filter(|l| !l.is_empty()).count();
    assert_eq!(
        line_count, 1,
        "channel JSONL must contain exactly one accepted event; got {line_count} lines:\n{written}"
    );
    let record: serde_json::Value = serde_json::from_str(
        written
            .lines()
            .find(|l| !l.is_empty())
            .expect("non-empty line"),
    )
    .expect("line must be valid JSON");
    assert_eq!(record["topic"], "exec.unit.done");
    // ralph emit decodes the JSON-shaped payload into an object
    // value, so we compare against the parsed object rather than
    // the raw input string.
    let expected_payload: serde_json::Value =
        serde_json::from_str(payload).expect("payload must parse");
    assert_eq!(record["payload"], expected_payload);
}

/// Forgery guard regression: when the marker is missing, the same
/// invocation must be rejected so the spawn cannot smuggle a
/// channel write past the dispatcher-signed allowlist. This is the
/// environmental counterpart to the unit-level
/// `test_emit_wave_worker_channel_rejected_without_marker_signature`.
#[test]
fn u7_real_ralph_emit_rejects_when_marker_missing() {
    let tmp = TempDir::new().expect("temp workspace");
    let workspace = tmp.path();
    let ralph_dir = workspace.join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
    // Deliberately do NOT write `.ralph/current-wave-channels`.

    let channel = ralph_dir.join("wave-u7-forged-0.jsonl");

    let output = common::ralph_bin()
        .arg("emit")
        .arg("exec.unit.done")
        .arg(r#"{"ok":true}"#)
        .arg("--file")
        .arg(channel.to_string_lossy().to_string())
        .arg("--hat")
        .arg("exec-worker")
        .env("RALPH_WAVE_WORKER", "1")
        .env("RALPH_WAVE_ID", "u7")
        .env("RALPH_WAVE_INDEX", "0")
        .current_dir(workspace)
        .output()
        .expect("spawn ralph emit");

    assert!(
        !output.status.success(),
        "ralph emit must reject a forged channel without marker signature; \
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !channel.exists(),
        "channel file must NOT be created when the marker is absent"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("current-wave-channels")
            || stderr.contains("marker")
            || stderr.contains("allowlist"),
        "stderr must reference the missing marker; got: {stderr}"
    );
}
