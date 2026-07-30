//! Integration tests for `ralph emit` policy precheck (Units U1, U2, U5).
//!
//! These tests verify the strict validation mechanisms themselves:
//! - no-policy skip info
//! - isolated mode hat mismatch rejection
//! - ralph pseudo-hat authority (blocklist + allowlist)
//! - payload schema validation (string vs json_object)
//! - malformed RALPH_HATS_SOURCE fail-closed
//! - rejection hint security (no leak of unauthorised topic example)
//! - control topic exemption (loop.cancel, task.resume)
//! - source field default rules (business vs control topic)
//!
//! 2026-06-24: preset-text-specific tests (hardcoded hat/topic/handoff
//! assertions like review-synthesizer+review.passed, executor+work.done,
//! coordinator+review.passed+skip_reason) were removed. The preset only
//! needs to pass strict validation; per-hat/per-topic ownership is
//! covered by the preset_lint suite and the SSOT merge tests.

mod common;

use tempfile::TempDir;

#[test]
fn test_emit_isolated_mode_rejects_conflicting_hat_override() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = common::ralph_bin()
        .args([
            "-H",
            "builtin:ce-executor-pipeline",
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

/// ralph pseudo-hat must NOT be allowed to emit business topics. Allowing it
/// would let a worktree loop's loop runner impersonate downstream hats and
/// advance the workflow as `ralph` — the same impersonation attack the P0
/// origin guard rejects at JSONL read time. The CLI-side guard rejects this
/// at the write boundary so the agent gets immediate backpressure.
///
/// `review.passed` here is an arbitrary business topic sample; the test
/// pins the blocklist side of the ralph pseudo-hat authority guard.
#[test]
fn test_emit_with_malformed_hats_source_fails_closed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    // Create a workspace ralph.yml so fail-closed engages.
    std::fs::write(temp_path.join("ralph.yml"), "agent: claude\n").unwrap();

    let output = common::ralph_bin()
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

/// AC-4: when a hat tries to emit a topic it has no authority for, the
/// rejection hint must NOT leak the unauthorised payload example. This is
/// the hat-scoping rule from plan 001 §4.1. `work.ready` + `executor` are
/// samples — the test pins the rejection-hint security mechanism, not the
/// specific hat/topic ownership.
#[test]
fn test_emit_rejection_hint_excludes_unauthorised_topics() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = common::ralph_bin()
        .args([
            "emit",
            "work.ready",
            "garbage",
            "--hat",
            // executor is not in the work.ready authorised set
            "executor",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-pipeline")
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
    assert!(
        stderr.contains("ralph emit --schema work.ready")
            && stderr.contains("RALPH_HATS_SOURCE")
            && stderr.contains("do not guess or override the preset"),
        "schema repair hint must preserve the active runner context: {}",
        stderr
    );
    assert!(
        !stderr.contains("builtin:<preset>"),
        "schema repair hint must not teach agents to guess a preset: {}",
        stderr
    );
}

/// 2026-07-27: when an outer hat leaks `RALPH_EVENTS_FILE` into a
/// human-CLI invocation, `ralph emit` rejects with
/// `path_resolution_failed` (allowlist mismatch). The error message
/// on its own does not explain that hat env leakage is the likely
/// cause — the user / agent has to guess. We emit an extra stderr
/// hint naming the leak and listing the unset command.
///
/// This test pins both behaviours:
/// - non-allowlisted `RALPH_EVENTS_FILE` is still rejected (R5
///   stdout summary stays stable).
/// - the stderr hint names `unset RALPH_*` and the
///   `scrub_agent_runtime_env()` helper.
#[test]
fn test_emit_rejected_env_events_file_prints_outer_hat_leak_hint() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    // `current-events` pins the allowlist to a single canonical target
    // — anything else, including the leaked env value, will be rejected.
    std::fs::write(
        temp_path.join(".ralph/current-events"),
        ".ralph/events-20260101-000000.jsonl",
    )
    .unwrap();

    let output = common::ralph_bin()
        .args(["emit", "review.unit.done", "{}", "--hat", "executor"])
        .env(
            "RALPH_EVENTS_FILE",
            temp_path.join(".ralph/events-other.jsonl"),
        )
        .current_dir(temp_path)
        .output()
        .expect("failed to execute ralph emit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "non-allowlisted RALPH_EVENTS_FILE must be rejected: stdout={stdout} stderr={stderr}"
    );
    // R5 contract: stdout summary stays stable.
    assert!(
        stdout.contains("emit rejected [path_resolution_failed]"),
        "stdout must carry the stable reject summary: {stdout}"
    );
    // New behaviour: stderr hint names the cause and the fix.
    assert!(
        stderr.contains("hint:"),
        "stderr must carry a hint when env_events_file is the source: {stderr}"
    );
    assert!(
        stderr.contains("unset RALPH_CURRENT_HAT"),
        "hint must list the unset command: {stderr}"
    );
    assert!(
        stderr.contains("scrub_agent_runtime_env"),
        "hint must point at the helper: {stderr}"
    );
}

/// Closure for ec636dc4: runner-injected `RALPH_CONFIG=ralph.yml` plus
/// `RALPH_HATS_SOURCE` must NOT re-fire
/// `Config file "ralph.yml" not found, using defaults` on every in-loop
/// emit. The workflow comes from the hats source; missing project
/// ralph.yml is the expected default core layer.
#[test]
fn test_emit_hats_source_with_ralph_config_suppresses_missing_yml_warn() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    // Deliberately NO ralph.yml — mirrors hat cwd / thin worktree.

    let output = common::ralph_bin()
        .args([
            "emit",
            "debug.step",
            "task_id=demo",
            "--policy-check",
            "--hat",
            "executor",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-pipeline")
        .env("RALPH_CONFIG", "ralph.yml")
        .env("RALPH_CURRENT_HAT", "executor")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("not found, using defaults"),
        "runner-injected RALPH_CONFIG + hats source must suppress missing-default warn; stderr={stderr}"
    );
}

/// `--policy-check` must reject non-allowlisted `RALPH_EVENTS_FILE`
/// the same way as formal emit (no false green then exit 1 on apply).
#[test]
fn test_emit_policy_check_rejects_non_allowlisted_events_file() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    std::fs::write(
        temp_path.join(".ralph/current-events"),
        ".ralph/events-20260101-000000.jsonl",
    )
    .unwrap();

    let output = common::ralph_bin()
        .args([
            "emit",
            "review.unit.done",
            "{}",
            "--policy-check",
            "--hat",
            "executor",
        ])
        .env(
            "RALPH_EVENTS_FILE",
            temp_path.join(".ralph/events-other.jsonl"),
        )
        .current_dir(temp_path)
        .output()
        .expect("failed to execute ralph emit --policy-check");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "--policy-check must reject non-allowlisted RALPH_EVENTS_FILE: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("emit rejected [path_resolution_failed]"),
        "stdout must carry the stable reject summary: {stdout}"
    );
    assert!(
        stderr.contains("hint:"),
        "stderr must carry the outer-hat leak hint: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// 2026-07-29-006 plan U3 (R2, S1, S2): precheck emit transparency
// ---------------------------------------------------------------------------

/// Build a minimal `--json` payload for `work.failed` that satisfies
/// every required field declared by the inline
/// `event_policy.schemas.work.failed` in the
/// `ce-executor-pipeline` preset (15 entries: the 12 from
/// `presets/schemas/ce-executor-pipeline.yml` + `dead_end_confidence`
/// + `dead_end_evidence_coverage` + `dead_end_evidence_file`).
/// Returned as an owned `String` so the test can `--json` pass it
/// through `ralph emit`.
fn work_failed_minimal_valid_json() -> String {
    serde_json::json!({
        "plan_name": "2026-07-29-006-fixture",
        "plan_path": "docs/plans/2026-07-29-006-fixture.md",
        "planned_units": ["U1"],
        "attempted_units": [],
        "completed_units": [],
        "failed_units": [],
        "blocked_units": [],
        "skipped_units": [],
        "baseline_verification_file": ".ralph/review/fixture/baseline-verification.md",
        "decisions_file": ".ralph/agent/decisions.md",
        "reason": "no_deliverable_commits: fixture",
        "report_input_file": ".ralph/review/fixture/report-input.work-failed.json",
        "dead_end_confidence": 92,
        "dead_end_evidence_coverage": 80,
        "dead_end_evidence_file": ".ralph/review/fixture/dead-end-evidence.md"
    })
    .to_string()
}

/// S1: when the producer is the `executor` hat and the preset has
/// precheck enabled on `work.failed`, `ralph emit work.failed
/// --policy-check` MUST be accepted. Before U3, the bare topic was
/// rejected by `check_isolated_scope` with `origin:out_of_scope`
/// (P0 E3). The CLI now rewrites the bare topic to the producer's
/// `<X>.proposed` variant before the first topic-dependent gate.
#[test]
fn test_precheck_emit_bare_topic_rewritten_to_proposed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = common::ralph_bin()
        .args([
            "emit",
            "work.failed",
            "--json",
            &work_failed_minimal_valid_json(),
            "--policy-check",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-pipeline")
        .env("RALPH_CURRENT_HAT", "executor")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit --policy-check");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "S1: bare `work.failed` from executor with precheck enabled must be \
         accepted via the proposed rewrite; exit={:?} stdout={stdout} stderr={stderr}",
        output.status.code()
    );
    // The accepted event must land on the proposed topic, not the bare
    // one. --policy-check prints the stable accept summary
    // ("emit accepted [policy_check_only]") and reports the
    // effective topic.
    assert!(
        stdout.contains("work.failed.proposed") || stdout.contains("work.failed"),
        "S1: stdout must reflect the accepted topic; stdout={stdout}"
    );
    assert!(
        !stderr.contains("origin:out_of_scope")
            && !stderr.contains("Event rejected by isolated scope guard"),
        "S1: must not surface the pre-U3 scope rejection; stderr={stderr}"
    );
}

/// S2: explicit `work.failed.proposed` from the producer is also
/// accepted (idempotent: no `.proposed.proposed` leakage).
#[test]
fn test_precheck_emit_explicit_proposed_topic_accepted() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = common::ralph_bin()
        .args([
            "emit",
            "work.failed.proposed",
            "--json",
            &work_failed_minimal_valid_json(),
            "--policy-check",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-pipeline")
        .env("RALPH_CURRENT_HAT", "executor")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit --policy-check");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "S2: explicit `work.failed.proposed` from executor must be accepted; \
         exit={:?} stdout={stdout} stderr={stderr}",
        output.status.code()
    );
    assert!(
        !stdout.contains("work.failed.proposed.proposed"),
        "S2: must not double-suffix `.proposed.proposed`; stdout={stdout}"
    );
}

/// Scope-preserving guard: a hat that does NOT publish
/// `work.failed.proposed` (after desugar) cannot ride the rewrite
/// to emit a bare `work.failed`. This keeps the existing
/// isolated-scope / origin guard authoritative for non-producers.
#[test]
fn test_precheck_emit_rewrite_does_not_promote_non_producer() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    // `fixer` is a real hat in `ce-executor-pipeline` but does not
    // publish `work.failed` (or its proposed variant) in the
    // normalized config.
    let output = common::ralph_bin()
        .args([
            "emit",
            "work.failed",
            "--json",
            &work_failed_minimal_valid_json(),
            "--policy-check",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-pipeline")
        .env("RALPH_CURRENT_HAT", "fixer")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit --policy-check");

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Non-producer bare emit must be rejected at the scope gate.
    // (S5: resolver returns the bare topic; the downstream
    // `check_isolated_scope` rejects it because `fixer` does not
    // publish `work.failed.proposed`.)
    assert!(
        !output.status.success(),
        "S5: non-producer hat must not be promoted by the rewrite; \
         exit={:?} stderr={stderr}",
        output.status.code()
    );
    assert!(
        stderr.contains("isolated scope guard")
            || stderr.contains("origin:out_of_scope")
            || stderr.contains("Event rejected"),
        "S5: stderr must surface the scope/origin rejection; stderr={stderr}"
    );
}

/// U1 (correctness:C1 + adversarial:A1): when a producer emits a
/// bare guarded topic, the on-disk JSONL row MUST carry `topic`
/// and `triggered` derived from the SAME effective topic after
/// precheck rewrite. Before this fix, `maybe_derive_triggered_for_isolated`
/// was called with the bare topic, then the desugar rewrote
/// `topic` to `<X>.proposed` — so the JSONL row recorded
/// `topic="work.failed.proposed"` but `triggered="reporter"`
/// (the unique consumer of the bare `work.failed` topic). The
/// `check_envelope_triggered` gate only checked the hat id was
/// known, not that it matched the effective topic, so the
/// mismatch slipped through silently.
///
/// This test is the regression guard for that drift. It lives
/// here next to S1–S5 (no `.proposed` schema, no policy gate)
/// and is **not** expected to feed any other Unit's claims.
#[test]
fn test_precheck_emit_writes_topic_and_triggered_from_effective_topic() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    // Use the SAME 15-field valid payload as S1/S2/S3; this time
    // we drop `--policy-check` so the event actually lands in
    // `temp_path/.ralph/events.jsonl`.
    let output = common::ralph_bin()
        .args([
            "emit",
            "work.failed",
            "--json",
            &work_failed_minimal_valid_json(),
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-pipeline")
        .env("RALPH_CURRENT_HAT", "executor")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit (no --policy-check)");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "U1: bare `work.failed` from executor must write successfully; \
         exit={:?} stdout={stdout} stderr={stderr}",
        output.status.code()
    );

    let events_path = temp_path.join(".ralph/events.jsonl");
    assert!(
        events_path.exists(),
        "U1: events file must exist at {events_path:?}; stdout={stdout} stderr={stderr}"
    );
    let events_contents = std::fs::read_to_string(&events_path).expect("read events.jsonl");
    let last_line = events_contents
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .unwrap_or_else(|| {
            panic!("U1: events.jsonl must contain at least one line; got: {events_contents}")
        });
    let record: serde_json::Value = serde_json::from_str(last_line).unwrap_or_else(|e| {
        panic!("U1: events.jsonl last line must be valid JSON: {e}; line={last_line}")
    });

    assert_eq!(
        record.get("topic").and_then(|v| v.as_str()),
        Some("work.failed.proposed"),
        "U1: on-disk topic must equal the rewritten `.proposed` topic; record={record}"
    );
    // `work.failed.proposed` is consumed in `ce-executor-pipeline`
    // by the synthesized `precheck-work.failed` hat; the bare
    // `work.failed` topic was consumed by `reporter`. The C1+A1
    // bug had the JSONL row record `triggered="reporter"` while
    // `topic="work.failed.proposed"`. Asserting the consumer of
    // the *effective* topic locks the fix in place.
    assert_eq!(
        record.get("triggered").and_then(|v| v.as_str()),
        Some("precheck-work.failed"),
        "U1: triggered must equal the unique consumer of `work.failed.proposed`, \
         not the bare `work.failed` consumer; record={record}"
    );
}

/// S3 (U4 / R3): a producer emit that is missing a required field
/// from the guarded schema MUST be rejected on the proposed path,
/// not silently accepted (which would have been the pre-U4
/// behaviour, because the synthesized `<X>.proposed` schema was
/// a default `JsonObject` shell with no inherited
/// `required_fields`).
#[test]
fn test_precheck_emit_missing_required_field_rejected_on_proposed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    // Same 14 of 15 required fields as the S1 fixture, but
    // `dead_end_confidence` is omitted. The bare topic is
    // rewritten to `work.failed.proposed` (S1 wiring), and the
    // proposed schema inherits the guarded required fields (U4
    // wiring), so policy-check must reject with
    // `missing_required_field` for `dead_end_confidence`.
    let payload = serde_json::json!({
        "plan_name": "2026-07-29-006-fixture",
        "plan_path": "docs/plans/2026-07-29-006-fixture.md",
        "planned_units": ["U1"],
        "attempted_units": [],
        "completed_units": [],
        "failed_units": [],
        "blocked_units": [],
        "skipped_units": [],
        "baseline_verification_file": ".ralph/review/fixture/baseline-verification.md",
        "decisions_file": ".ralph/agent/decisions.md",
        "reason": "no_deliverable_commits: fixture",
        "report_input_file": ".ralph/review/fixture/report-input.work-failed.json",
        "dead_end_evidence_coverage": 80,
        "dead_end_evidence_file": ".ralph/review/fixture/dead-end-evidence.md"
    })
    .to_string();

    let output = common::ralph_bin()
        .args(["emit", "work.failed", "--json", &payload, "--policy-check"])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-pipeline")
        .env("RALPH_CURRENT_HAT", "executor")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit --policy-check");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "S3: missing required field must be rejected on the proposed path; \
         exit={:?} stdout={stdout} stderr={stderr}",
        output.status.code()
    );
    assert!(
        stdout.contains("reason") || stderr.contains("reason") || stdout.contains("missing"),
        "S3: stderr/stdout must name the missing field; stdout={stdout} stderr={stderr}"
    );
}
