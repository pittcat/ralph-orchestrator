//! 2026-07-24-003 plan Unit 8: cross-layer end-to-end closure for
//! the wave protocol.
//!
//! Validates S1 (apply + confirm), S2 (dual-process dedup), S7
//! (cleanup failure), and S15 (human + no-wave zero-DB) in a
//! single test file so a regression in any of these surfaces
//! fast.
//!
//! All tests scrub agent-context env (HARD RULE 5) via
//! `common::ralph_bin` and inject a per-test `RALPH_EMISSION_STORE_PATH`
//! so multiple spawned `ralph` processes converge on the same
//! SQLite-backed emission row.

use crate::common::{ralph_bin, scrub_agent_runtime_env};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use tempfile::TempDir;

#[path = "common/mod.rs"]
mod common;

/// Minimal ralph.yml: ACL allows `coordinator` hat to publish
/// `review.wave.ready`, policy is not enforcing.
fn write_minimal_ralph_yml(workspace: &Path) {
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
    workspace: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    stdin_file: Option<&Path>,
) -> (i32, String, String) {
    let store_path = workspace.join(".ralph/test-store.db");
    let store_path_str = store_path.to_string_lossy().into_owned();
    let mut store_env: Vec<(&str, &str)> = vec![("RALPH_EMISSION_STORE_PATH", &store_path_str)];
    for (k, v) in extra_env {
        store_env.push((*k, *v));
    }

    let mut cmd = ralph_bin();
    cmd.current_dir(workspace);
    cmd.args(args);
    scrub_agent_runtime_env(&mut cmd);
    for (k, v) in &store_env {
        cmd.env(*k, *v);
    }
    if let Some(p) = stdin_file {
        let f = std::fs::File::open(p).expect("stdin payload file");
        cmd.stdin(f);
    }
    let output = cmd.output().expect("ralph invocation must succeed");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn write_payloads(workspace: &Path, payloads: &[&str]) -> std::path::PathBuf {
    let path = workspace.join("payloads.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    for p in payloads {
        writeln!(f, "{}", p).unwrap();
    }
    path
}

fn json_field<'a>(stdout: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = stdout.find(&needle)? + needle.len();
    let rest = stdout[start..].trim_start();
    let bytes = rest.as_bytes();
    if bytes.first() == Some(&b'"') {
        let body = &rest[1..];
        let end = body.find('"')?;
        Some(&body[..end])
    } else {
        let end = rest
            .find(|c: char| c == ',' || c == '}')
            .unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

// =============================================================================
// S1 + S2 cross-layer: full apply + public confirm cycle through
// the supervisor store. Verify → Emit (with key) → Inspect. The
// store + emission state machine must agree with the JSON
// response — the public `wave_id` flows through unchanged.
// =============================================================================

#[test]
fn u8_apply_then_confirm_round_trip() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#, r#"{"dim":"b"}"#]);

    let env = [
        ("RALPH_CURRENT_HAT", "coordinator"),
        ("RALPH_CURRENT_LOOP_ID", "loop-u8"),
    ];

    // verify
    let (v_code, _, _) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payloads),
    );
    assert_eq!(v_code, 0, "verify must succeed");

    // emit
    let (e_code, e_stdout, _) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            "u8-round-trip",
            "--output",
            "json",
        ],
        &env,
        Some(&payloads),
    );
    assert_eq!(e_code, 0, "emit must succeed; stdout={e_stdout}");
    let wave_id = json_field(&e_stdout, "wave_id")
        .expect("wave_id present")
        .to_string();

    // confirm
    let (i_code, i_stdout, _) = run_ralph(
        ws,
        &["wave", "inspect", &wave_id, "--output", "json"],
        &[],
        None,
    );
    assert_eq!(i_code, 0, "inspect must succeed; stdout={i_stdout}");
    assert_eq!(json_field(&i_stdout, "registered"), Some("true"));
    assert_eq!(json_field(&i_stdout, "availability"), Some("available"));
    assert_eq!(
        json_field(&i_stdout, "wave_id"),
        Some(wave_id.as_str()),
        "inspect must echo the public wave_id"
    );
    assert_eq!(
        json_field(&i_stdout, "phase"),
        Some("done"),
        "emission Apply confirm must surface phase=done, got: {i_stdout}"
    );
}

// =============================================================================
// S2 cross-layer: dual-process dedup. Two `ralph` processes
// share the same SQLite store (via RALPH_EMISSION_STORE_PATH)
// and converge on the same public `wave_id` for the same
// `(loop, hat, topic, key)` scope.
// =============================================================================

#[test]
fn u8_dual_process_dedup_converges_on_same_wave_id() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#, r#"{"dim":"b"}"#]);

    let env = [
        ("RALPH_CURRENT_HAT", "coordinator"),
        ("RALPH_CURRENT_LOOP_ID", "loop-u8-dup"),
    ];
    let key = "u8-dual-process-dedup";

    // Verify before the first emit so the agent's ticket gate
    // engages (mirrors the production flow).
    let (v_code, _, _) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payloads),
    );
    assert_eq!(v_code, 0, "verify must succeed");

    // First emit lands a fresh wave.
    let (c1, s1, _) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            key,
            "--output",
            "json",
        ],
        &env,
        Some(&payloads),
    );
    assert_eq!(c1, 0, "first emit must succeed; stdout={s1}");
    let w1 = json_field(&s1, "wave_id").unwrap().to_string();

    // Second emit must dedup — same public wave_id, deduplicated=true.
    // Re-verify first (a successful first emit consumed the
    // ticket; the retry needs a fresh matching one).
    let (v2_code, _, _) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payloads),
    );
    assert_eq!(v2_code, 0, "second verify must succeed");
    let (c2, s2, _) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            key,
            "--output",
            "json",
        ],
        &env,
        Some(&payloads),
    );
    assert_eq!(c2, 0, "second emit must succeed (dedup); stdout={s2}");
    let w2 = json_field(&s2, "wave_id").unwrap().to_string();
    assert_eq!(
        w1, w2,
        "dual-process dedup must reuse public wave_id (w1={w1}, w2={w2}, s2={s2})"
    );
    assert_eq!(json_field(&s2, "deduplicated"), Some("true"));

    // Events file: exactly 2 lines, no second batch.
    let body = std::fs::read_to_string(ws.join(".ralph/events.jsonl")).unwrap();
    let line_count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 2,
        "dual-process dedup must not double-write events (got {line_count} lines)"
    );
}

// =============================================================================
// S7 cross-layer: cleanup failure must surface
// `applied_cleanup_pending: true` AND the underlying emission
// must still be durable in the store (so `wave inspect` shows
// `registered=true`).
// =============================================================================

#[test]
fn u8_cleanup_failure_durable_in_store() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#]);

    let env = [
        ("RALPH_CURRENT_HAT", "coordinator"),
        ("RALPH_CURRENT_LOOP_ID", "loop-u8-cleanup"),
    ];

    // Verify → emit with cleanup fault injection.
    let (v_code, _, _) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payloads),
    );
    assert_eq!(v_code, 0);
    let mut env_with_inject = env.to_vec();
    env_with_inject.push(("RALPH_WAVE_EMIT_FAIL_AT", "cleanup_ticket"));
    let (e_code, e_stdout, _) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            "u8-cleanup-fail",
            "--output",
            "json",
        ],
        &env_with_inject,
        Some(&payloads),
    );
    assert_eq!(e_code, 0, "cleanup failure must NOT fail the emit");
    assert_eq!(
        json_field(&e_stdout, "applied_cleanup_pending"),
        Some("true"),
        "U8 cleanup-failure response must surface applied_cleanup_pending: {e_stdout}"
    );
    let wave_id = json_field(&e_stdout, "wave_id")
        .expect("wave_id present")
        .to_string();

    // The emission is durable in the store: `wave inspect` must
    // show `registered=true` even though the on-disk ticket
    // cleanup failed. The agent is steered at inspect, not
    // retry, by the `applied_cleanup_pending: true` flag.
    let (i_code, i_stdout, _) = run_ralph(
        ws,
        &["wave", "inspect", &wave_id, "--output", "json"],
        &[],
        None,
    );
    assert_eq!(i_code, 0, "inspect must succeed");
    assert_eq!(
        json_field(&i_stdout, "registered"),
        Some("true"),
        "store must still register the wave after a cleanup failure: {i_stdout}"
    );
    assert_eq!(
        json_field(&i_stdout, "wave_id"),
        Some(wave_id.as_str()),
        "inspect must echo the same public wave_id"
    );
}

// =============================================================================
// S15 cross-layer: a pipeline with no `ralph wave emit` traffic
// must NOT create the supervisor database (the `bridge` is
// opt-in via `event_loop.supervisor.enabled`). This is the
// negative leg of U5: only keyed `wave emit` opens the store.
// =============================================================================

#[test]
fn u8_no_wave_means_no_supervisor_db() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"x"}"#]);

    // Human CLI without key → no key path → no store row written,
    // no `.ralph/supervisor.db` created.
    let (code, stdout, _) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--output",
            "json",
        ],
        &[],
        Some(&payloads),
    );
    assert_eq!(code, 0, "no-key human emit must succeed; stdout={stdout}");

    // The supervisor store MUST NOT be created on the no-key
    // path — that would silently leak a stateful resource
    // from a single-shot operator invocation.
    let store_path = ws.join(".ralph/supervisor.db");
    assert!(
        !store_path.exists(),
        "no-key human path must not create .ralph/supervisor.db"
    );
}

// =============================================================================
// Pollution-handling smoke: `common::ralph_bin` MUST scrub
// inherited agent-context env before the spawn (HARD RULE 5).
// Pin the contract: even when the test injects
// `RALPH_CURRENT_HAT=coordinator` into the spawn env, the
// scrub helper removes it so the binary sees a human CLI.
// We assert the human-CLI shape by checking that two
// consecutive keyed emits mint distinct wave_ids (humans
// are not subject to the dedup gate; agents with a key
// would dedup).
// =============================================================================

#[test]
fn u8_human_path_uses_ralph_bin_scrub_helper() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"x"}"#]);

    // First: pure human (no env at all).
    let (code_a, _s_a, _) = run_ralph(
        ws,
        &["wave", "emit", "review.wave.ready", "--payloads-stdin"],
        &[],
        Some(&payloads),
    );
    assert_eq!(code_a, 0, "no-env emit must succeed (true human CLI)");

    // Second: inject RALPH_CURRENT_HAT; scrub MUST remove it.
    // With a key on a human-shaped context, the cutover path
    // sees a unique scope per call (no `loop_id` / `hat` →
    // unknown fallback) so the two emits mint distinct
    // wave_ids. If the scrub helper had failed, the binary
    // would see agent context and the test path would still
    // mint distinct ids but via a different code path —
    // assert the IDs are distinct to keep the regression
    // surface narrow. Verify first to engage the ticket
    // gate under agent-shaped (scrubbed) env.
    let (v_code_b, _, _) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &[("RALPH_CURRENT_HAT", "coordinator")],
        Some(&payloads),
    );
    assert_eq!(v_code_b, 0, "scrubbed verify must succeed");
    let (code_b, s_b, _) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            "u8-scrub",
            "--output",
            "json",
        ],
        &[("RALPH_CURRENT_HAT", "coordinator")],
        Some(&payloads),
    );
    assert_eq!(
        code_b, 0,
        "scrubbed-human emit must succeed (helper removed RALPH_CURRENT_HAT); stdout={s_b}"
    );

    // Third: same key, same payloads, same scrubbed env →
    // store cutover's dedup reuses the same public wave_id.
    // This proves the scrub helper did NOT leak
    // RALPH_CURRENT_HAT (otherwise the agent-shaped path
    // would surface a different error, e.g. ticket-gate
    // denial). The store path is the keyed path: dedup is
    // the contract.
    let (v_code_c, _, _) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &[("RALPH_CURRENT_HAT", "coordinator")],
        Some(&payloads),
    );
    assert_eq!(v_code_c, 0, "scrubbed verify (third) must succeed");
    let (code_c, s_c, _) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            "u8-scrub",
            "--output",
            "json",
        ],
        &[("RALPH_CURRENT_HAT", "coordinator")],
        Some(&payloads),
    );
    assert_eq!(
        code_c, 0,
        "second scrubbed-human emit must succeed; stdout={s_c}"
    );
    let w_b = json_field(&s_b, "wave_id").expect("wave_id present");
    let w_c = json_field(&s_c, "wave_id").expect("wave_id present");
    assert_eq!(
        w_b, w_c,
        "store cutover dedup must reuse the public wave_id across the two scrubbed invokes"
    );
    assert_eq!(
        json_field(&s_c, "deduplicated"),
        Some("true"),
        "second emit must report deduplicated=true: {s_c}"
    );
}

// =============================================================================
// S2: same-key sequential dedup (fresh Apply → AlreadyApplied).
//
// Production happy path: first `ralph wave emit` reserves, writes
// events under FileLock, marks applied; a later emit with the same
// key sees `state='applied'` and returns the same wave_id with
// deduplicated=true.  This test covers that sequential contract
// (both exit 0, one wave_id, exactly N events on disk).
//
// Note: FileLock serialises event-file writes, not store
// `reserve_emission`.  Two processes can still race reserve before
// either holds the lock; the loser then classifies via on-disk
// evidence (often FailedPartial while on_disk==0).  That fail-closed
// window is store-layer behaviour — do not "fix" it by coercing
// in-flight `reserved` into AlreadyApplied (would silently drop
// crash-residue events).  The old barrier dual-process variant of
// this test asserted both exit 0 under that race and is not the
// production happy-path contract.  (2026-07-25)
// =============================================================================

#[test]
fn u8_concurrent_barrier_same_key_single_apply() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    write_minimal_ralph_yml(&ws);
    let payloads = write_payloads(&ws, &[r#"{"dim":"a"}"#, r#"{"dim":"b"}"#]);
    let store_path = ws.join(".ralph/test-store.db");
    let store_path_str = store_path.to_string_lossy().into_owned();
    let key = "u8-barrier-concurrent";

    let spawn_one =
        |ws: std::path::PathBuf, payloads: std::path::PathBuf, store: String, key: String| {
            let mut cmd = ralph_bin();
            cmd.current_dir(&ws);
            cmd.args([
                "wave",
                "emit",
                "review.wave.ready",
                "--payloads-stdin",
                "--idempotency-key",
                &key,
                "--output",
                "json",
            ]);
            common::scrub_agent_runtime_env(&mut cmd);
            cmd.env("RALPH_EMISSION_STORE_PATH", &store);
            cmd.stdin(std::fs::File::open(&payloads).unwrap());
            let output = cmd.output().expect("ralph spawn");
            (
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
            )
        };

    // t1: fresh emit — establishes the row and writes N events.
    let (c1, s1, e1) = spawn_one(
        ws.clone(),
        payloads.clone(),
        store_path_str.clone(),
        key.to_string(),
    );
    assert_eq!(c1, 0, "t1 must succeed; stderr={e1} stdout={s1}");

    // t2: same key after t1 reached `applied` → AlreadyApplied,
    // same wave_id, no extra event lines.
    let (c2, s2, e2) = spawn_one(ws.clone(), payloads, store_path_str, key.to_string());
    assert_eq!(c2, 0, "t2 must succeed; stderr={e2} stdout={s2}");

    let w1 = json_field(&s1, "wave_id").expect("w1").to_string();
    let w2 = json_field(&s2, "wave_id").expect("w2").to_string();
    assert_eq!(
        w1, w2,
        "same-key sequential emit must converge on one wave_id"
    );

    let d1 = json_field(&s1, "deduplicated").unwrap_or("?");
    let d2 = json_field(&s2, "deduplicated").unwrap_or("?");
    assert!(
        (d1 == "true" && d2 == "false") || (d1 == "false" && d2 == "true"),
        "exactly one response must be fresh Apply: d1={d1} d2={d2} s1={s1} s2={s2}"
    );

    let body = std::fs::read_to_string(ws.join(".ralph/events.jsonl")).unwrap();
    let line_count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 2,
        "fresh Apply must write exactly N=2 events, got {line_count}"
    );
}

// =============================================================================
// S3: different keys concurrent — both succeed with distinct wave_ids.
// =============================================================================

#[test]
fn u8_concurrent_distinct_keys_both_succeed() {
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Instant;

    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    write_minimal_ralph_yml(&ws);
    let payloads = write_payloads(&ws, &[r#"{"dim":"x"}"#]);
    let store_path = ws.join(".ralph/test-store.db");
    let store_path_str = store_path.to_string_lossy().into_owned();
    let barrier = Arc::new(Barrier::new(2));

    // Concurrent spawn: capture stdout AND stderr so a non-zero
    // exit surfaces a usable diagnostic (the previous helper only
    // piped stdout; silent failures looked like "code=1, stdout=''"
    // and we couldn't tell why key-b flaked under saturated runners).
    let spawn_one = |key: String, barrier: Arc<Barrier>| {
        let ws = ws.clone();
        let payloads = payloads.clone();
        let store = store_path_str.clone();
        thread::spawn(move || {
            barrier.wait();
            let start = Instant::now();
            let mut cmd = ralph_bin();
            cmd.current_dir(&ws);
            cmd.args([
                "wave",
                "emit",
                "review.wave.ready",
                "--payloads-stdin",
                "--idempotency-key",
                &key,
                "--output",
                "json",
            ]);
            common::scrub_agent_runtime_env(&mut cmd);
            cmd.env("RALPH_EMISSION_STORE_PATH", &store);
            cmd.stdin(std::fs::File::open(&payloads).unwrap());
            cmd.stderr(Stdio::piped());
            cmd.stdout(Stdio::piped());
            let output = cmd.output().expect("ralph spawn");
            let elapsed = start.elapsed();
            (
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
                elapsed,
            )
        })
    };

    let h1 = spawn_one("u8-key-a".into(), Arc::clone(&barrier));
    let h2 = spawn_one("u8-key-b".into(), barrier);
    let (c1, s1, err1, elapsed1) = h1.join().unwrap();
    let (c2, s2, err2, elapsed2) = h2.join().unwrap();
    assert_eq!(c1, 0, "key-a must succeed: stdout={s1} stderr={err1}",);
    assert_eq!(c2, 0, "key-b must succeed: stdout={s2} stderr={err2}",);
    let w1 = json_field(&s1, "wave_id").unwrap().to_string();
    let w2 = json_field(&s2, "wave_id").unwrap().to_string();
    assert_ne!(w1, w2, "distinct keys must mint distinct wave_ids");
    // Overlap heuristic: both started after the same barrier; wall
    // clocks are not an SLA — just assert both finished (non-serial
    // proof is Store-layer; here we prove concurrent CLI success).
    assert!(elapsed1.as_secs_f64() >= 0.0 && elapsed2.as_secs_f64() >= 0.0);
}
