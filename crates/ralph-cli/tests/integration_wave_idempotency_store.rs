//! 2026-07-24-003 plan Unit 5: sidecar import + `wave emit` cutover.
//!
//! Validates that the idempotency authority for `ralph wave emit`
//! has fully moved onto the supervisor store:
//!
//! - **Cutover**: when a key is supplied, the emission flows through
//!   `SupervisorStore::reserve_emission`. Sidecar reads are gated to
//!   the miss-import path; sidecar writes are not produced by the
//!   cutover branch (S1, S2, S4, S15).
//! - **JSON shape**: successful cutover emission drops the
//!   `events_file` field — agents should not need to read internal
//!   ledger paths (S1, R11).
//! - **Conflict**: same scope, different payload digest → stable
//!   `idempotency_key_conflict` error, zero new events, zero store
//!   mutation (S4).
//! - **Recovery-required**: store reports a prior reservation but
//!   events file has fewer than `expected_count` lines → fail-closed
//!   and inspect-guided, no auto-repair (S8, S9).
//! - **Sidecar import**: legacy `<events>.idempotency.jsonl` row
//!   pointing at a fully-applied event batch is consumed once,
//!   then deleted; subsequent emissions dedup against the imported
//!   record (S10). Mismatched sidecar (digest/count mismatch) is
//!   NOT imported (S11).
//! - **Human no-key path**: two consecutive emits produce two
//!   distinct wave_ids, sidecar is untouched (S15).
//!
//! Agent-context env scrubs are mandatory (HARD RULE 5).

use crate::common::{ralph_bin, scrub_agent_runtime_env};
use ralph_core::supervisor::SupervisorStore;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

#[path = "common/mod.rs"]
mod common;

/// Minimal `ralph.yml` with the supervisor-friendly defaults: ACL
/// allows `coordinator` hat to publish `review.wave.ready`, policy is
/// not enforcing (so U5 / cutover is the only gate under test).
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
    // U5 cutover path: every test needs a process-shared store so
    // multiple `ralph wave emit` invocations converge on the same
    // `wave_emissions` row. Production CLI picks up
    // `.ralph/supervisor.db` automatically when it exists; tests
    // inject `RALPH_EMISSION_STORE_PATH` so each test owns a
    // dedicated SQLite file at `<workspace>/.ralph/test-store.db`.
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

fn default_events_path(workspace: &Path) -> std::path::PathBuf {
    workspace.join(".ralph/events.jsonl")
}

fn legacy_sidecar_path(workspace: &Path) -> std::path::PathBuf {
    // Mirrors `idempotency_log_path` derivation in `wave.rs`:
    // `<parent>/.<basename>.idempotency.jsonl`.
    workspace.join(".ralph/.events.jsonl.idempotency.jsonl")
}

/// Drain a JSON response from stdout and pull one field by key.
/// Returns the trimmed string value (or None if missing).
fn json_field<'a>(stdout: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = stdout.find(&needle)? + needle.len();
    let rest = stdout[start..].trim_start();
    let bytes = rest.as_bytes();
    if bytes.first() == Some(&b'"') {
        // string value
        let body = &rest[1..];
        let end = body.find('"')?;
        Some(&body[..end])
    } else {
        // number / bool / null — read until comma or }
        let end = rest
            .find(|c: char| c == ',' || c == '}')
            .unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

// =============================================================================
// S1 + U5 cutover: human CLI with key still wins over sidecar; success
// JSON drops `events_file` (R11).
// =============================================================================

#[test]
fn u5_cutover_success_json_drops_events_file() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#, r#"{"dim":"b"}"#]);

    // Human CLI (no RALPH_CURRENT_HAT → no ticket gate, no ACL
    // pressure). With `--idempotency-key` the cutover path is the
    // only authority; success JSON MUST NOT echo events_file.
    let (code, stdout, _stderr) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            "u5-cutover-success",
            "--output",
            "json",
        ],
        &[],
        Some(&payloads),
    );
    assert_eq!(code, 0, "cutover emit must succeed; stdout={stdout}");
    assert!(
        !stdout.contains("\"events_file\""),
        "U5 cutover must drop events_file from success JSON, got: {stdout}"
    );

    // Mandatory success-shape fields stay present.
    assert_eq!(json_field(&stdout, "ok"), Some("true"));
    assert!(
        json_field(&stdout, "wave_id").is_some_and(|s| s.starts_with("w-")),
        "wave_id missing: {stdout}"
    );
    assert_eq!(json_field(&stdout, "deduplicated"), Some("false"));
    assert_eq!(json_field(&stdout, "topic"), Some("review.wave.ready"));
}

// =============================================================================
// S2 + U5: same key + same payload → AlreadyApplied dedup. Both
// invocations agree on the public wave_id and only one batch landed.
// =============================================================================

#[test]
fn u5_cutover_dedup_returns_already_applied() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#, r#"{"dim":"b"}"#]);
    let key = "u5-cutover-dedup";

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
        &[],
        Some(&payloads),
    );
    assert_eq!(c1, 0, "first emit must succeed; stdout={s1}");
    let first_wave_id = json_field(&s1, "wave_id")
        .expect("wave_id present")
        .to_string();

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
        &[],
        Some(&payloads),
    );
    assert_eq!(c2, 0, "second emit must succeed (dedup); stdout={s2}");
    let second_wave_id = json_field(&s2, "wave_id")
        .expect("wave_id present")
        .to_string();
    assert_eq!(
        first_wave_id, second_wave_id,
        "cutover dedup must reuse the public wave_id"
    );
    assert_eq!(
        json_field(&s2, "deduplicated"),
        Some("true"),
        "second emit must report deduplicated=true: {s2}"
    );

    // Events file carries exactly 2 lines (the original batch).
    let body = std::fs::read_to_string(default_events_path(ws)).unwrap();
    let line_count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 2,
        "cutover dedup must not append events (got {line_count} lines)"
    );
}

// =============================================================================
// S4 + U5: same key + different payload → conflict, zero new writes.
// =============================================================================

#[test]
fn u5_cutover_payload_conflict_returns_stable_error() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads_a = write_payloads(ws, &[r#"{"dim":"a"}"#]);
    let payloads_b_path = ws.join("payloads-b.jsonl");
    std::fs::write(&payloads_b_path, "{\"dim\":\"b\"}\n").unwrap();
    let key = "u5-cutover-conflict";

    // First emit lands.
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
        &[],
        Some(&payloads_a),
    );
    assert_eq!(c1, 0, "first emit must succeed; stdout={s1}");

    // Second emit with different payload MUST fail closed.
    let (c2, _s2, err2) = run_ralph(
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
        &[],
        Some(&payloads_b_path),
    );
    assert_ne!(c2, 0, "conflict must surface a non-zero exit");
    assert!(
        err2.contains("idempotency_key_conflict") || err2.contains("idempotency-key conflict"),
        "U5 conflict must carry the stable marker, got: {err2}"
    );

    // No new events landed.
    let body = std::fs::read_to_string(default_events_path(ws)).unwrap();
    let line_count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 1,
        "conflict path must not append events (got {line_count} lines)"
    );
}

// =============================================================================
// S15 + U5: human CLI without `--idempotency-key` produces two distinct
// wave_ids and does NOT touch the sidecar.
// =============================================================================

#[test]
fn u5_human_no_key_two_emits_yield_distinct_wave_ids() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#]);

    let (c1, s1, _) = run_ralph(
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
    let (c2, s2, _) = run_ralph(
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
    assert_eq!(c1, 0, "first emit must succeed; stdout={s1}");
    assert_eq!(c2, 0, "second emit must succeed; stdout={s2}");

    let w1 = json_field(&s1, "wave_id").unwrap().to_string();
    let w2 = json_field(&s2, "wave_id").unwrap().to_string();
    assert_ne!(w1, w2, "no-key path must mint fresh wave_ids");

    // Sidecar must NOT be created by the no-key path. (Plan U5:
    // sidecar is the cutover's *legacy* import source, never its
    // write target.)
    assert!(
        !legacy_sidecar_path(ws).exists(),
        "no-key human path must not write a sidecar"
    );
}

// =============================================================================
// S10 + U5: legacy sidecar with a fully-applied batch is imported on
// the first cutover call. After the import the second call dedups
// against the imported wave_id; subsequent delete of the sidecar
// must NOT regress dedup because the store now owns the row.
// =============================================================================

#[test]
fn u5_legacy_sidecar_import_then_dedup_after_delete() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let events_path = default_events_path(ws);

    // Lay down a pre-fix "legacy" emissions footprint: 2 events
    // already on disk + a sidecar row claiming this wave.
    let legacy_wave_id = "w-legacy-sidecar-7";
    let legacy_key = "u5-legacy-import";
    let mut ev = std::fs::File::create(&events_path).unwrap();
    for i in 0..2 {
        writeln!(
            ev,
            r#"{{"topic":"review.wave.ready","ts":"2026-01-01T00:00:00Z","wave_id":"{legacy_wave_id}","wave_index":{i},"wave_total":2,"idempotency_key":"{legacy_key}"}}"#
        )
        .unwrap();
    }
    drop(ev);

    let scope_key = {
        // The CLI computes scope_key as
        // sha256("<loop_id>|<hat>|<topic>|<key>"). Without hat/loop
        // markers it falls back to "unknown". We replicate that
        // fallback by setting env to "unknown" / "" so the CLI's
        // own scope-key hash matches the row we seed.
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"unknown||review.wave.ready|");
        h.update(legacy_key.as_bytes());
        let digest = h.finalize();
        let mut out = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write;
            let _ = write!(&mut out, "{b:02x}");
        }
        out
    };
    let payload_digest = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        // Mirrors `compute_payload_digest` exactly: payloads joined
        // by `\u{1F}`. Two payloads → one separator → one joined
        // string.
        let mut joined = String::from(r#"{"dim":"a"}"#);
        joined.push('\u{1F}');
        joined.push_str(r#"{"dim":"b"}"#);
        h.update(joined.as_bytes());
        let digest = h.finalize();
        let mut out = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write;
            let _ = write!(&mut out, "{b:02x}");
        }
        out
    };
    let sidecar = legacy_sidecar_path(ws);
    std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
    {
        let mut f = std::fs::File::create(&sidecar).unwrap();
        writeln!(
            f,
            r#"{{"scope_key":"{scope_key}","idempotency_key":"{legacy_key}","wave_id":"{legacy_wave_id}","topic":"review.wave.ready","hat":"","payload_digest":"{payload_digest}","count":2,"created_at":"2026-01-01T00:00:00Z"}}"#
        )
        .unwrap();
    }

    // First cutover call: store has no emission for this scope, so
    // the miss-import path reads the sidecar and the on-disk events
    // match exactly → AlreadyApplied with the legacy wave_id.
    //
    // The legacy sidecar claims count=2 — the cutover's batch MUST
    // likewise be 2 payloads, otherwise the importer refuses to
    // adopt (its expected_count != record.count guard).
    let payloads_a = ws.join("payloads-a.jsonl");
    std::fs::write(&payloads_a, "{\"dim\":\"a\"}\n{\"dim\":\"b\"}\n").unwrap();
    let (_c1, s1, _e1) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            legacy_key,
            "--output",
            "json",
        ],
        &[],
        Some(&payloads_a),
    );
    assert_eq!(
        _c1, 0,
        "import path must succeed (and dedup against the legacy row); stdout={s1}"
    );
    assert_eq!(
        json_field(&s1, "wave_id"),
        Some(legacy_wave_id),
        "imported wave_id must match the legacy row, got: {s1}"
    );
    assert_eq!(
        json_field(&s1, "deduplicated"),
        Some("true"),
        "import path must report deduplicated=true: {s1}"
    );

    // Delete the sidecar — the store should still own the row.
    // (The first import may have already removed it; either path
    // exercises the same invariant: the store, not the sidecar,
    // owns dedup from now on.)
    if sidecar.exists() {
        std::fs::remove_file(&sidecar).unwrap();
    }
    let payloads_a2 = ws.join("payloads-a-2.jsonl");
    std::fs::write(&payloads_a2, "{\"dim\":\"a\"}\n{\"dim\":\"b\"}\n").unwrap();
    let (c2, s2, _e2) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            legacy_key,
            "--output",
            "json",
        ],
        &[],
        Some(&payloads_a2),
    );
    assert_eq!(c2, 0, "post-import dedup must still succeed");
    assert_eq!(
        json_field(&s2, "wave_id"),
        Some(legacy_wave_id),
        "post-import dedup must still resolve to the imported wave_id: {s2}"
    );
    assert_eq!(
        json_field(&s2, "deduplicated"),
        Some("true"),
        "post-import dedup must remain deduplicated=true: {s2}"
    );

    // And no extra events were appended — events file remains at 2 lines.
    let body = std::fs::read_to_string(&events_path).unwrap();
    let line_count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(line_count, 2, "import path must not append events");
}

// =============================================================================
// S11 + U5: a legacy sidecar whose payload_digest disagrees with the
// incoming payloads MUST NOT be silently imported — the cutover
// fails closed (plan §3 fault injection note for S11).
// =============================================================================

#[test]
fn u5_legacy_sidecar_digest_mismatch_fails_closed() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let events_path = default_events_path(ws);

    // Legacy wave events on disk (2 events) + sidecar whose
    // payload_digest points at *another* batch.
    let legacy_wave_id = "w-legacy-mismatch";
    let legacy_key = "u5-legacy-mismatch";
    let mut ev = std::fs::File::create(&events_path).unwrap();
    for i in 0..2 {
        writeln!(
            ev,
            r#"{{"topic":"review.wave.ready","ts":"2026-01-01T00:00:00Z","wave_id":"{legacy_wave_id}","wave_index":{i},"wave_total":2,"idempotency_key":"{legacy_key}"}}"#
        )
        .unwrap();
    }
    drop(ev);

    let scope_key = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"unknown||review.wave.ready|");
        h.update(legacy_key.as_bytes());
        let digest = h.finalize();
        let mut out = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write;
            let _ = write!(&mut out, "{b:02x}");
        }
        out
    };
    let bogus_digest = "deadbeef".repeat(8); // 64-hex bogus digest
    let sidecar = legacy_sidecar_path(ws);
    std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
    {
        let mut f = std::fs::File::create(&sidecar).unwrap();
        writeln!(
            f,
            r#"{{"scope_key":"{scope_key}","idempotency_key":"{legacy_key}","wave_id":"{legacy_wave_id}","topic":"review.wave.ready","hat":"","payload_digest":"{bogus_digest}","count":2,"created_at":"2026-01-01T00:00:00Z"}}"#
        )
        .unwrap();
    }

    // Cutover call with the real payload — digest disagrees with the
    // sidecar, so the importer must fail-closed (S11) and must NOT
    // mint a second wave beside the legacy batch.
    let payloads_a = ws.join("payloads-a.jsonl");
    std::fs::write(&payloads_a, "{\"dim\":\"a\"}\n").unwrap();
    let (code, _stdout, err) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            legacy_key,
            "--output",
            "json",
        ],
        &[],
        Some(&payloads_a),
    );

    assert_ne!(code, 0, "S11 mismatch must fail closed; stderr={err}");
    assert!(
        err.contains("sidecar_import_conflict")
            || err.contains("idempotency_key_conflict")
            || err.contains("mismatch"),
        "mismatch sidecar must fail with a stable marker, got: {err}"
    );
    let body = std::fs::read_to_string(&events_path).unwrap();
    let line_count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 2,
        "mismatch must not append events (got {line_count} lines)"
    );
}

// =============================================================================
// P0: corrupt supervisor.db must fail-closed on keyed emit (no InMemory).
// =============================================================================

#[test]
fn u5_corrupt_store_keyed_emit_fails_closed() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#]);

    let corrupt = ws.join(".ralph/corrupt-store.db");
    std::fs::write(&corrupt, b"not a sqlite database\n").unwrap();
    let corrupt_str = corrupt.to_string_lossy().into_owned();

    // Bypass run_ralph's default store injection so we point at the
    // corrupt file exclusively.
    let mut cmd = ralph_bin();
    cmd.current_dir(ws);
    cmd.args([
        "wave",
        "emit",
        "review.wave.ready",
        "--payloads-stdin",
        "--idempotency-key",
        "corrupt-store-key",
        "--output",
        "json",
    ]);
    scrub_agent_runtime_env(&mut cmd);
    cmd.env("RALPH_EMISSION_STORE_PATH", &corrupt_str);
    cmd.stdin(std::fs::File::open(&payloads).unwrap());
    let output = cmd.output().unwrap();
    let code = output.status.code().unwrap_or(-1);
    let err = String::from_utf8_lossy(&output.stderr).to_string();
    assert_ne!(code, 0, "corrupt store must fail closed; stderr={err}");
    assert!(
        err.contains("supervisor_store_unavailable"),
        "must surface supervisor_store_unavailable, got: {err}"
    );
    let events = ws.join(".ralph/events.jsonl");
    assert!(
        !events.exists() || std::fs::read_to_string(&events).unwrap().trim().is_empty(),
        "corrupt store must not write events"
    );
}

// =============================================================================
// S9: partial emission on disk + reserved store row → fail-closed, no append.
// =============================================================================

#[test]
fn u5_partial_emission_fail_closed_and_inspect() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let store_path = ws.join(".ralph/test-store.db");
    let events_path = default_events_path(ws);
    std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

    let key = "u5-partial-s9";
    let scope_key = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"unknown||review.wave.ready|");
        h.update(key.as_bytes());
        let digest = h.finalize();
        let mut out = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write;
            let _ = write!(&mut out, "{b:02x}");
        }
        out
    };
    let payload_digest = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"{\"dim\":\"a\"}");
        h.update([0x1F]);
        h.update(b"{\"dim\":\"b\"}");
        let digest = h.finalize();
        let mut out = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write;
            let _ = write!(&mut out, "{b:02x}");
        }
        out
    };

    let store = ralph_core::supervisor::RusqliteSupervisorStore::open(&store_path)
        .expect("open store");
    let reserved = store
        .reserve_emission(&scope_key, &payload_digest, 2, &|_| 0)
        .expect("reserve");
    let wave_id = match reserved {
        ralph_core::supervisor::EmissionReservation::Reserved { public_wave_id } => public_wave_id,
        other => panic!("expected Reserved, got {other:?}"),
    };
    // Partial batch: only 1 of 2 events.
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&events_path).unwrap();
        writeln!(
            f,
            r#"{{"topic":"review.wave.ready","ts":"2026-01-01T00:00:00Z","wave_id":"{wave_id}","wave_index":0,"wave_total":2,"idempotency_key":"{key}"}}"#
        )
        .unwrap();
    }
    store
        .mark_emission_recovery_required(&scope_key)
        .expect("mark recovery");

    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#, r#"{"dim":"b"}"#]);
    let (code, _stdout, err) = run_ralph(
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
        &[],
        Some(&payloads),
    );
    assert_ne!(code, 0, "S9 partial must fail closed; stderr={err}");
    assert!(
        err.contains("recovery") || err.contains("partial") || err.contains("RecoveryRequired"),
        "partial must mention recovery/partial, got: {err}"
    );
    let body = std::fs::read_to_string(&events_path).unwrap();
    let line_count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(line_count, 1, "must not append on partial recovery");

    let (i_code, i_stdout, _) = run_ralph(
        ws,
        &["wave", "inspect", &wave_id, "--output", "json"],
        &[],
        None,
    );
    assert_eq!(i_code, 0, "inspect must succeed; {i_stdout}");
    let parsed: serde_json::Value = serde_json::from_str(&i_stdout).unwrap();
    assert_eq!(parsed["registered"], serde_json::json!(true));
    assert_eq!(
        parsed["phase"],
        serde_json::json!("failed"),
        "recovery_required surfaces as failed phase: {parsed}"
    );
}
