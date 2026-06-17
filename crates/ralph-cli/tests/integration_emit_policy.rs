//! Integration tests for `ralph emit` policy precheck (Units U1, U2, U5).
//!
//! These tests verify that:
//! - `ralph emit` honors builtin preset `event_policy` when `-H` is provided.
//! - Payload contract violations are rejected at the CLI entry and leave a
//!   recovery envelope in `.ralph/recovery.jsonl`.
//! - When no preset and no `event_policy` is configured, the CLI logs that the
//!   policy check was skipped.

use std::process::Command;
use tempfile::TempDir;

fn ralph_emit(temp_path: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args(args)
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command")
}

/// Happy path: a conforming `work.done` payload is accepted and written.
#[test]
fn test_emit_with_builtin_preset_accepts_valid_work_done() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = ralph_emit(
        temp_path,
        &[
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "work.done",
            "--json",
            r#"{"plan_name":"x","plan_path":"y","task_id":"z","task_key":"k","step":"s","commit_count":1,"changed_lines":10}"#,
            "--hat",
            "executor",
        ],
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "emit should succeed for valid work.done payload: stderr={}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).expect("events file should exist");
    assert!(
        events.contains("\"topic\":\"work.done\""),
        "events: {}",
        events
    );
    assert!(
        events.contains("\"plan_name\":\"x\""),
        "events should contain payload: {}",
        events
    );
}

/// Error path: a JSON object missing required fields for `work.done` is
/// rejected and leaves a recovery envelope.
#[test]
fn test_emit_with_builtin_preset_rejects_missing_required_fields() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = ralph_emit(
        temp_path,
        &[
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "work.done",
            "--json",
            r#"{"ok":true}"#,
            "--hat",
            "executor",
        ],
    );

    assert!(
        !output.status.success(),
        "emit should fail for work.done payload missing required fields"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing") || stderr.contains("required"),
        "stderr should explain missing required fields: {}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    assert!(
        !events_file.exists()
            || std::fs::read_to_string(&events_file)
                .unwrap()
                .trim()
                .is_empty(),
        "rejected event must not be written to events file"
    );

    let recovery_file = temp_path.join(".ralph/recovery.jsonl");
    assert!(recovery_file.exists(), "recovery.jsonl should be written");
    let recovery = std::fs::read_to_string(&recovery_file).unwrap();
    let entry: serde_json::Value = recovery
        .lines()
        .next()
        .expect("recovery.jsonl should have at least one line")
        .parse()
        .expect("recovery line should be valid JSON");
    assert_eq!(entry["envelope"]["source"], "cli_emit");
    assert_eq!(entry["envelope"]["topic"], "work.done");
    assert_eq!(entry["envelope"]["source_hat"], "executor");
}

/// Error path: a string payload for `work.done` is rejected and leaves a
/// recovery envelope.
#[test]
fn test_emit_with_builtin_preset_rejects_string_payload() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = ralph_emit(
        temp_path,
        &[
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "work.done",
            "free text",
            "--hat",
            "executor",
        ],
    );

    assert!(
        !output.status.success(),
        "emit should fail for string work.done payload"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Payload is not valid JSON") || stderr.contains("payload type mismatch"),
        "stderr should explain payload rejection: {}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    assert!(
        !events_file.exists()
            || std::fs::read_to_string(&events_file)
                .unwrap()
                .trim()
                .is_empty(),
        "rejected event must not be written to events file"
    );

    let recovery_file = temp_path.join(".ralph/recovery.jsonl");
    assert!(recovery_file.exists(), "recovery.jsonl should be written");
    let recovery = std::fs::read_to_string(&recovery_file).unwrap();
    let entry: serde_json::Value = recovery
        .lines()
        .next()
        .expect("recovery.jsonl should have at least one line")
        .parse()
        .expect("recovery line should be valid JSON");
    assert_eq!(entry["envelope"]["source"], "cli_emit");
    assert_eq!(
        entry["envelope"]["reason_code"],
        "payload_contract_violation"
    );
    assert_eq!(entry["envelope"]["topic"], "work.done");
    assert_eq!(entry["envelope"]["outcome"], "not_retriable");
}

/// Edge path: no preset and no event_policy logs that the policy check is skipped.
#[test]
fn test_emit_without_policy_logs_skip_info() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = ralph_emit(temp_path, &["emit", "debug.step", "task_id=demo"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "emit should stay lenient without policy: stderr={}",
        stderr
    );

    let combined = format!("{} {}", stdout, stderr);
    assert!(
        combined.contains("cli emit policy check skipped: no event_policy in resolved config"),
        "expected skip info log in stdout/stderr; stdout={} stderr={}",
        stdout,
        stderr
    );
}

/// Phase 2: in isolated mode, an agent that tries to override its hat via
/// `--hat` while `RALPH_CURRENT_HAT` points to a different hat is rejected.
#[test]
fn test_emit_isolated_mode_rejects_conflicting_hat_override() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "debug.step",
            "task_id=demo",
            "--hat",
            "review-coordinator",
        ])
        .env("RALPH_CURRENT_HAT", "review-synthesizer")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "emit should reject conflicting --hat in isolated mode: stderr={}",
        stderr
    );
    assert!(
        stderr.contains("Isolated mode hat mismatch"),
        "expected isolated-mode mismatch error, got: {}",
        stderr
    );

    // The event must not be written anywhere.
    let events_file = temp_path.join(".ralph/events.jsonl");
    assert!(
        !events_file.exists()
            || std::fs::read_to_string(&events_file)
                .unwrap()
                .trim()
                .is_empty(),
        "rejected event must not be written"
    );
}

/// Phase 3: hat-aware allowed values reject review-coordinator emitting
/// review.passed(skip_reason=aggregate_timeout) before it reaches jsonl.
#[test]
fn test_emit_isolated_mode_rejects_coordinator_aggregate_timeout() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "review.passed",
            "--json",
            r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"aggregate_timeout"}"#,
            "--hat",
            "review-coordinator",
        ])
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "emit should reject review-coordinator + aggregate_timeout: stderr={}",
        stderr
    );
    assert!(
        stderr.contains("review-coordinator") && stderr.contains("aggregate_timeout"),
        "expected hat-aware rejection message, got: {}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    assert!(
        !events_file.exists()
            || std::fs::read_to_string(&events_file)
                .unwrap()
                .trim()
                .is_empty(),
        "rejected event must not be written"
    );
}

/// Phase 2: in isolated mode, when --hat agrees with RALPH_CURRENT_HAT the
/// emit proceeds normally (provided the topic is in the hat's `publishes`).
///
/// 2026-06-17-003 plan U1: this test previously emitted `debug.step` from
/// `review-synthesizer` — a hat that does not own `debug.step`. The test
/// was capturing the pre-U1 behaviour where isolated-scope was only
/// enforced at loop runtime (events would land in events.jsonl and be
/// dropped silently). U1 closes that precheck gap; the test now emits a
/// topic the hat actually owns (`review.passed`) and asserts the
/// provenance-override path still works.
#[test]
fn test_emit_isolated_mode_allows_matching_hat() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "review.passed",
            r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"aggregate_timeout"}"#,
            "--hat",
            "review-synthesizer",
        ])
        .env("RALPH_CURRENT_HAT", "review-synthesizer")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "emit should succeed when --hat matches env: stderr={}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap();
    assert!(events.contains("\"hat\":\"review-synthesizer\""));
    assert!(events.contains("review.passed"));
}

// -------------------------------------------------------------------------
// 2026-06-17-003 plan U1: regression coverage for merry-lotus root cause.
//
// merry-lotus run: `executor` (in `ce-executor-serial`) emitted `debug.step`
// 8 times, each landing in events.jsonl before being dropped at loop runtime.
// U1 closes this precheck gap. These two tests pin the fix to the actual
// preset that triggered the bug — `ce-executor-serial` — not the
// `ce-executor-isolated` preset covered by `test_emit_isolated_mode_allows_matching_hat`.
// -------------------------------------------------------------------------

/// P0 (testing reviewer): `ce-executor-serial` executor emits `work.done` →
/// event lands in events.jsonl. Regression guard against the inverse of
/// merry-lotus.
#[test]
fn test_emit_ce_executor_serial_executor_can_emit_work_done() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "work.done",
            r#"{"plan_name":"p","plan_path":"p.md","task_id":"t","task_key":"k","step":"s","commit_count":1,"changed_lines":10}"#,
            "--hat",
            "executor",
        ])
        .env("RALPH_CURRENT_HAT", "executor")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "executor+work.done in ce-executor-serial must succeed: stderr={}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap();
    assert!(events.contains("work.done"));
    assert!(events.contains("\"hat\":\"executor\""));
}

/// P0 (testing reviewer): `ce-executor-serial` executor emits `debug.step` →
/// CLI rejects before write. Reproduces the merry-lotus root cause and
/// asserts the U1 precheck now catches it (instead of letting it land in
/// events.jsonl for the loop to silently drop).
#[test]
fn test_emit_ce_executor_serial_executor_cannot_emit_debug_step() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "debug.step",
            "task_id=demo",
            "--hat",
            "executor",
        ])
        .env("RALPH_CURRENT_HAT", "executor")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "executor+debug.step in ce-executor-serial must be rejected (merry-lotus root cause): \
         stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("isolated_scope_violation") || stderr.contains("isolated scope guard"),
        "expected isolated scope rejection message, got stderr={stderr}"
    );

    // Critical: the event must NOT have landed in events.jsonl.
    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap_or_default();
    assert!(
        !events.contains("debug.step"),
        "rejected event must not be written to events.jsonl (merry-lotus regression), got: {events}"
    );
}

// -------------------------------------------------------------------------
// Plan 001 §4.3 C1/C4/C5: RALPH_HATS_SOURCE env routes pre-publish check.
// -------------------------------------------------------------------------

/// Run `ralph emit` with an explicit env var, no `-H` flag. The CLI must
/// pick up the preset advertised by `RALPH_HATS_SOURCE`, enforce its
/// `event_policy.schemas`, and refuse to write a string payload for a
/// topic that requires a JSON object (AC-2).
#[test]
fn test_emit_with_env_hats_source_rejects_string_payload_for_work_ready() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "emit",
            "work.ready",
            "中文长字符串 payload",
            "--hat",
            "coordinator",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-isolated")
        .env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    assert!(
        !output.status.success(),
        "string payload must be rejected when RALPH_HATS_SOURCE preset requires json_object"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rejected") || stderr.contains("policy") || stderr.contains("required"),
        "stderr should explain rejection: {}",
        stderr
    );
    // Plan 001 AC-4 / §4.3 C3: stderr should expose a copy-pasteable
    // `ralph emit work.ready --json ...` example restricted to topics
    // the active hat may publish. The legacy bare rejection would NOT
    // contain the example line, so this assertion also catches drift.
    assert!(
        stderr.contains("ralph emit work.ready --json"),
        "stderr should expose the schema-aware fix hint: {}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    if events_file.exists() {
        let events = std::fs::read_to_string(&events_file).unwrap_or_default();
        assert!(
            !events.contains("中文长字符串"),
            "rejected payload MUST NOT land on disk: {}",
            events
        );
    }
}

/// AC-3: with RALPH_HATS_SOURCE, a properly-formed JSON payload is
/// accepted and written to the events file.
#[test]
fn test_emit_with_env_hats_source_accepts_valid_json_payload() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "emit",
            "work.ready",
            "--json",
            r#"{"plan_name":"p","plan_path":"/tmp/p","task_id":"t","task_key":"k","step":"s","complexity":3}"#,
            "--hat",
            "coordinator",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-isolated")
        .env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "valid payload must succeed when RALPH_HATS_SOURCE preset is honoured: stderr={}",
        stderr
    );
    let events = std::fs::read_to_string(temp_path.join(".ralph/events.jsonl")).unwrap();
    assert!(events.contains("\"topic\":\"work.ready\""));
    assert!(events.contains("\"plan_name\":\"p\""));
}

/// AC-8: malformed RALPH_HATS_SOURCE (no such preset) with a workspace
/// config present must fail closed — never silently Skip.
#[test]
fn test_emit_with_malformed_hats_source_fails_closed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    // Create a workspace ralph.yml so fail-closed engages.
    std::fs::write(temp_path.join("ralph.yml"), "agent: claude\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args(["emit", "work.ready", "--json", "{}", "--hat", "coordinator"])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:not-a-real-preset")
        .env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    assert!(
        !output.status.success(),
        "malformed RALPH_HATS_SOURCE + workspace config must fail closed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not-a-real-preset") || stderr.contains("Pre-publish"),
        "stderr should explain the fail-closed reason: {}",
        stderr
    );
}

/// AC-7: `ralph wave emit` honours RALPH_HATS_SOURCE — a batch with a
/// missing required field is rejected and no candidate events are
/// written.
#[test]
fn test_wave_emit_with_env_hats_source_rejects_missing_required_field() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads",
            r#"{"dim":"d1"}"#,
            r#"{"dim":"d2"}"#,
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-wave")
        .env("RALPH_CURRENT_HAT", "dimension-reviewer")
        .env("RALPH_WAVE_WORKER", "1")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph wave emit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "missing required field in batch must reject: stderr={} stdout={}",
        stderr,
        stdout
    );

    let candidate = temp_path.join(".ralph/candidate-events.jsonl");
    if candidate.exists() {
        let contents = std::fs::read_to_string(&candidate).unwrap_or_default();
        assert!(
            contents.trim().is_empty(),
            "rejected batch must NOT write candidate-events: {}",
            contents
        );
    }
}

/// AC-4: when an executor-hat child process tries to emit a topic it has
/// no authority for (e.g. `work.ready`, which only `coordinator`/`plan-gate`
/// may publish), the rejection hint must NOT leak the unauthorised
/// payload example. This is the hat-scoping rule from plan 001 §4.1.
#[test]
fn test_emit_rejection_hint_excludes_unauthorised_topics() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "emit",
            "work.ready",
            "garbage",
            "--hat",
            // executor is not in the work.ready authorised set
            "executor",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-isolated")
        .env("RALPH_CURRENT_HAT", "executor")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "executor must not be allowed to emit work.ready: stderr={}",
        stderr
    );
    // When fix_hint_for_hat_topic returns None (the hat cannot publish
    // the topic), the rejection message must not leak the example
    // `ralph emit work.ready --json '{...}'` form — that would teach
    // the agent to bypass provenance.
    assert!(
        !stderr.contains("ralph emit work.ready --json"),
        "fix hint must not surface an unauthorised-topic payload example: {}",
        stderr
    );
}

// -------------------------------------------------------------------------
// 2026-06-17-004 plan U1 (R1, R2): CLI provenance fail-closed tests.
// -------------------------------------------------------------------------

/// T1.1 Happy path: isolated + `--hat executor` + legal `work.done` → write.
#[test]
fn test_emit_t1_1_isolated_with_hat_legal_work_done_succeeds() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "work.done",
            "--json",
            r#"{"plan_name":"p","plan_path":"p.md","task_id":"t","task_key":"k","step":"s","commit_count":1,"changed_lines":10}"#,
            "--hat",
            "executor",
        ])
        .env("RALPH_CURRENT_HAT", "executor")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "executor+work.done in ce-executor-serial must succeed (T1.1): stderr={}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap();
    assert!(events.contains("work.done"));
    assert!(events.contains("\"hat\":\"executor\""));
}

/// T1.2 Error: isolated + no hat + `review.passed` + `aggregate_timeout` →
/// reject (Covers AE1 — the original noble-peacock root cause).
#[test]
fn test_emit_t1_2_isolated_no_hat_review_passed_rejected() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    // NOTE: --hat is deliberately omitted; RALPH_CURRENT_HAT is also unset
    // so the CLI must run check_emit_provenance.
    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "review.passed",
            "--json",
            r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"aggregate_timeout"}"#,
        ])
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "isolated+no-hat+review.passed must be rejected (T1.2 / AE1): stderr={}",
        stderr
    );
    // The blanket `Event provenance required` check fires first
    // (preset has `require_emit_provenance: true`); the smart
    // `check_emit_provenance` gate below is the second line of
    // defense for business topics. Both messages prove the event
    // was blocked at the CLI boundary before reaching JSONL.
    assert!(
        stderr.contains("missing_provenance")
            || stderr.contains("missing provenance")
            || stderr.contains("Event provenance required"),
        "expected missing_provenance or Event provenance required rejection, got: {}",
        stderr
    );

    // Critical: the event must NOT have landed in events.jsonl.
    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap_or_default();
    assert!(
        !events.contains("review.passed"),
        "rejected review.passed must NOT be written (T1.2 / AE1): {}",
        events
    );
}

/// T1.3 Error: isolated + no hat + `build.done` → reject (topic_denied or
/// missing_provenance depending on gate ordering).
#[test]
fn test_emit_t1_3_isolated_no_hat_build_done_rejected() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "build.done",
            "--json",
            r#"{"ok":true}"#,
        ])
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "isolated+no-hat+build.done must be rejected (T1.3): stderr={}",
        stderr
    );
    // Either missing_provenance (caught at the new U1 gate), or
    // `Event provenance required` (existing blanket gate for presets
    // with `require_emit_provenance: true`), or topic_denied (caught
    // at the existing topic-deny-rules gate) is acceptable — all
    // three prove no event was written.
    assert!(
        stderr.contains("missing_provenance")
            || stderr.contains("missing provenance")
            || stderr.contains("Event provenance required")
            || stderr.contains("topic_denied")
            || stderr.contains("topic denied"),
        "expected provenance or topic_denied rejection, got: {}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap_or_default();
    assert!(
        !events.contains("build.done"),
        "rejected build.done must NOT be written (T1.3): {}",
        events
    );
}

/// T1.4 Edge: isolated + `hat=ralph` + `loop.cancel` → allowed (control topic
/// exemption for the runtime pseudo-hat).
#[test]
fn test_emit_t1_4_isolated_ralph_hat_loop_cancel_allowed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "loop.cancel",
            "--json",
            r#"{"reason":"demo"}"#,
            "--hat",
            "ralph",
        ])
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "isolated+ralph+loop.cancel must succeed (T1.4 control topic exemption): \
         stdout={stdout} stderr={stderr}"
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap();
    assert!(
        events.contains("loop.cancel"),
        "loop.cancel must be written (T1.4): {}",
        events
    );
}

/// T1.5 Integration: with `RALPH_CURRENT_HAT=executor`, `debug.step` →
/// `isolated_scope_violation` (regression for plan 003 U1 — executor must
/// not be allowed to publish `debug.step`).
#[test]
fn test_emit_t1_5_isolated_executor_debug_step_scope_violation() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "debug.step",
            "task_id=demo",
            "--hat",
            "executor",
        ])
        .env("RALPH_CURRENT_HAT", "executor")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "executor+debug.step must be rejected (T1.5 plan-003 regression): \
         stderr={}",
        stderr
    );
    assert!(
        stderr.contains("isolated_scope_violation") || stderr.contains("isolated scope guard"),
        "expected isolated_scope_violation rejection, got: {}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap_or_default();
    assert!(
        !events.contains("debug.step"),
        "rejected debug.step must NOT be written (T1.5): {}",
        events
    );
}

/// T1.6 Integration: review-synthesizer emits `review.passed` with a
/// hat-allowed `skip_reason` → allowed. Confirms the U1 fail-closed gate
/// does not mis-block legitimate emits when provenance is supplied.
#[test]
fn test_emit_t1_6_isolated_review_synthesizer_legal_emit_allowed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    // `review-synthesizer` is allowed `skip_reason=dimensions_complete`
    // in ce-executor-serial (per the SSOT schema). Pass --hat
    // explicitly to pin the positive path; the test confirms the new
    // U1 gate does not over-block when provenance is supplied.
    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "review.passed",
            r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"dimensions_complete"}"#,
            "--hat",
            "review-synthesizer",
        ])
        .env("RALPH_CURRENT_HAT", "review-synthesizer")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "isolated+review-synthesizer+review.passed(dimensions_complete) must succeed (T1.6): \
         stdout={stdout} stderr={stderr}"
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap();
    assert!(events.contains("review.passed"));
    assert!(events.contains("\"hat\":\"review-synthesizer\""));
    assert!(events.contains("dimensions_complete"));
}

/// T1.7 Edge: isolated + no hat + `task.resume` → allowed (control topic
/// exemption for the loop's recovery signal).
#[test]
fn test_emit_t1_7_isolated_no_hat_task_resume_allowed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "task.resume",
            "--json",
            r#"{"target_hat":"executor","reason":"missing_event_gate","original_trigger_topic":"review.dimension.ready"}"#,
        ])
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "isolated+no-hat+task.resume must succeed (T1.7 control topic exemption): \
         stdout={stdout} stderr={stderr}"
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap();
    assert!(
        events.contains("task.resume"),
        "task.resume must be written (T1.7): {}",
        events
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 2026-06-17-004 plan U6 (T6.3): noble-peacock review.passed never-lands regression
//
// Root cause from the noble-peacock run: in isolated mode, an `executor`
// hat emitted `review.passed` with `skip_reason=aggregate_timeout` and
// the event landed in events.jsonl. The runtime origin guard later
// dropped it, but the agent had already received no actionable
// backpressure — the executor kept emitting more out-of-scope events.
//
// This test pins the U1 fix for the specific noble-peacock payload
// shape. It runs the same payload the noble-peacock run used
// (`plan_name="p"`, `task_id="t"`, `skip_reason=aggregate_timeout`)
// and asserts that:
//   1. The CLI exits non-zero (provenance fail-closed or topic-deny
//      rejection, not a silent drop).
//   2. `events.jsonl` does NOT contain the rejected event — the
//      noble-peacock leak is fully closed.
//   3. The recovery envelope (`.ralph/recovery.jsonl`) records the
//      rejection, so the agent's next turn can read it and adjust.
//
// If this test ever passes with `status.success() == true` or with
// `events.jsonl` containing the payload, the noble-peacock P0-1 leak
// has been re-introduced.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_noble_peacock_executor_review_passed_never_lands() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    // This is the literal payload the noble-peacock run emitted
    // (`events-20260617-095504.jsonl:3-4`). Dummy `plan_name="p"`,
    // `task_id="t"`, `task_key="k"`, `step="s"` — the diagnostic report
    // identified these as agent prompt drift (PROMPT.md referenced the
    // ralph emit API without a worked example). The combination of
    // `skip_reason=aggregate_timeout` + executor hat is the root cause:
    // aggregate_timeout is NOT in the preset's skip_reason allowed_values
    // for `review.passed`, so the event was always going to be rejected
    // by the policy layer — the only fix is to keep it OUT of events.jsonl.
    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "review.passed",
            "--json",
            r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"aggregate_timeout"}"#,
            "--hat",
            "executor",
        ])
        .env("RALPH_CURRENT_HAT", "executor")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Assertion 1: CLI must exit non-zero. The exact reason code
    // depends on the gate ordering (U1 check_emit_provenance vs the
    // existing topic-deny-rules check), but `aggregate_timeout` is not
    // in the allowed_values for `review.passed` in ce-executor-serial,
    // so the event MUST be rejected. If `status.success()` is true,
    // the noble-peacock P0-1 leak has been re-introduced.
    assert!(
        !output.status.success(),
        "noble-peacock root cause: executor + review.passed + aggregate_timeout \
         must be rejected (U6 T6.3 regression). Got status=success. \
         stderr={}",
        stderr
    );

    // The rejection must be a provenance / scope / topic-deny / schema
    // violation — not a panic or generic error. Acceptable reason
    // codes: missing_provenance, isolated_scope_violation, topic_denied,
    // invalid_field_value, missing_required_field, payload_contract_violation.
    let stderr_lower = stderr.to_lowercase();
    let has_actionable_rejection = stderr_lower.contains("missing_provenance")
        || stderr_lower.contains("missing provenance")
        || stderr_lower.contains("event provenance required")
        || stderr_lower.contains("isolated_scope_violation")
        || stderr_lower.contains("isolated scope")
        || stderr_lower.contains("topic_denied")
        || stderr_lower.contains("topic denied")
        || stderr_lower.contains("invalid_field_value")
        || stderr_lower.contains("invalid field value")
        || stderr_lower.contains("skip_reason")
        || stderr_lower.contains("allowed_values");
    assert!(
        has_actionable_rejection,
        "rejection must be actionable (cite provenance/scope/deny/field), got: {}",
        stderr
    );

    // Assertion 2: events.jsonl must NOT contain the rejected event.
    // This is the core of the noble-peacock P0-1 fix: the event must
    // not only be rejected at the CLI boundary, it must not be written
    // to disk in the first place. The pre-fix behavior was that the
    // event was written and then dropped at runtime — leaving the
    // agent with no actionable backpressure and the loop runner
    // cleaning up after the fact.
    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap_or_default();
    assert!(
        !events.contains("review.passed"),
        "rejected review.passed from executor must NEVER land in events.jsonl \
         (noble-peacock P0-1 leak). events: {}",
        events
    );
    assert!(
        !events.contains("aggregate_timeout"),
        "rejected aggregate_timeout payload must NEVER land in events.jsonl \
         (noble-peacock P0-1 leak). events: {}",
        events
    );
}
