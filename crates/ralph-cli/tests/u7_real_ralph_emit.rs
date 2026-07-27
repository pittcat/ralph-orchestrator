//! 2026-07-26-002 plan U7 (R7): outside-in integration test that
//! spawns the real `ralph emit` binary against a dispatcher-signed
//! per-slot wave channel and verifies:
//!
//! 1. The per-wave JSON registry under
//!    `.ralph/wave-channels/<loop-id>/<wave-id>.json` admits the
//!    channel path the dispatcher pre-committed; the spawn's
//!    `RALPH_WAVE_WORKER=1` + `RALPH_WAVE_ID` + `RALPH_WAVE_INDEX`
//!    + `RALPH_CURRENT_LOOP_ID` quadruple is the only path that
//!    passes the strict registry lookup (no main / marker
//!    fallthrough).
//! 2. The CLI appends a single event line to the channel JSONL.
//! 3. A forged channel (no registry entry) is rejected at the
//!    resolver — keeping the dispatcher's allowlist unforgeable.
//!
//! HARD RULE 5: spawn goes through `common::ralph_bin()` so the
//! process inherits a scrubbed environment — without the scrub,
//! the loop-owned env vars (RALPH_CURRENT_HAT / RALPH_WAVE_WORKER /
//! RALPH_EVENTS_FILE) would push the spawned `ralph emit` into an
//! in-loop ACL branch and the human-CLI semantics this test
//! assumes would not apply. The wave-worker env vars are then set
//! explicitly afterwards.
//!
//! 2026-07-27-003 plan U2 (KTD-1 / KTD-2) supersedes the legacy
//! `.ralph/current-wave-channels` marker with the per-wave JSON
//! registry. We replicate the registry write here in the test so
//! the dispatcher's pre-spawn `WaveChannelRegistry::prepare` is
//! observable from the spawned `ralph emit` without dragging the
//! ralph-cli crate internals into an integration test binary.

mod common;

use std::fs;
use std::path::{Component, Path, PathBuf};
use tempfile::TempDir;

/// Per-wave registry schema version that must match
/// `loop_runner::wave::channel_registry::REGISTRY_SCHEMA_VERSION`.
const REGISTRY_SCHEMA_VERSION: u32 = 1;
const LOOP_ID: &str = "u7-test-loop";
const WAVE_ID: &str = "u7";
const SLOT_INDEX: u32 = 0;

/// Replicate `loop_runner::wave::channel_registry::channel_path_bytes`:
/// serialize a `Path` into a `Vec<u8>` using `Component` parts joined
/// by `/`. The first component may be `RootDir` (on Unix absolute
/// paths) and the leading byte is therefore a single `/`. This is
/// what the production `fingerprint` hashes; if our SHA-256 input
/// differs, the readback guard refuses the channel with
/// `RegistryReadback` even though the registry entry looks correct.
fn channel_path_bytes(path: &Path) -> Vec<u8> {
    let mut buf = Vec::new();
    for component in path.components() {
        if !buf.is_empty() {
            buf.push(b'/');
        }
        match component {
            Component::Prefix(prefix) => {
                buf.extend(prefix.as_os_str().to_string_lossy().as_bytes());
            }
            Component::RootDir => {
                buf.push(b'/');
            }
            Component::CurDir => {
                buf.extend(b".");
            }
            Component::ParentDir => {
                buf.extend(b"..");
            }
            Component::Normal(normal) => {
                buf.extend(normal.to_string_lossy().as_bytes());
            }
        }
    }
    buf
}

/// SHA-256 hex of a `Path`'s `channel_path_bytes` encoding, matching
/// the production `channel_registry::fingerprint` algorithm. The
/// production helper emits `sha256:<hex>` (with the prefix), so the
/// test's fingerprint MUST carry the same prefix or the readback
/// guard refuses the channel with `RegistryReadback`.
fn channel_fingerprint(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let bytes = channel_path_bytes(path);
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64 + 7);
    out.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Percent-encode any byte that is not `[A-Za-z0-9._-]`, matching
/// `loop_runner::wave::channel_registry::encode_identity`. The
/// fixtures only use safe identities, so this is a small helper
/// rather than a re-implementation of the full algorithm.
fn encode_identity(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            for byte in ch.to_string().as_bytes() {
                use std::fmt::Write as _;
                let _ = write!(&mut out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Compute the absolute registry file path for a `(loop_id,
/// wave_id)` pair — the same layout the production
/// `WaveChannelRegistry::prepare` writes to.
fn registry_path(workspace: &Path, loop_id: &str, wave_id: &str) -> PathBuf {
    workspace
        .join(".ralph")
        .join("wave-channels")
        .join(encode_identity(loop_id))
        .join(format!("{}.json", encode_identity(wave_id)))
}

/// Canonicalize a channel path the way `WaveChannelRegistry::prepare`
/// does, by treating the workspace root as the prefix and rejecting
/// any path that escapes it. We avoid a true `canonicalize()` so the
/// test does not depend on the tempdir being a stable path on every
/// system; the production call site canonicalizes the parent (not the
/// channel leaf), so a child file at
/// `<workspace>/.ralph/wave-<id>-<idx>.jsonl` keeps the same
/// canonical form on Linux (no symlinks involved at `/tmp`).
fn canonical_channel(channel: &Path, workspace_root: &Path) -> PathBuf {
    let absolute = if channel.is_absolute() {
        channel.to_path_buf()
    } else {
        workspace_root.join(channel)
    };
    for component in absolute.components() {
        if matches!(component, Component::ParentDir) {
            panic!(
                "test channel {} escapes workspace {}",
                channel.display(),
                workspace_root.display()
            );
        }
    }
    absolute
}

/// Write the per-wave JSON registry the dispatcher would have
/// written via `WaveChannelRegistry::prepare`. The schema mirrors
/// `loop_runner::wave::channel_registry::RegistryFile`.
fn write_registry(workspace: &Path, loop_id: &str, wave_id: &str, slot_index: u32, channel: &Path) {
    let path = registry_path(workspace, loop_id, wave_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir registry dir");
    }
    let canonical = canonical_channel(channel, workspace);
    let fingerprint = channel_fingerprint(&canonical);
    let body = serde_json::json!({
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "loop_id": loop_id,
        "wave_id": wave_id,
        "prepared_at": chrono::Utc::now().to_rfc3339(),
        "bindings": [
            {
                "slot_index": slot_index,
                "channel_path": canonical,
                "channel_fingerprint": fingerprint,
            }
        ]
    });
    fs::write(
        &path,
        serde_json::to_string_pretty(&body).expect("serialize registry"),
    )
    .expect("write registry");
}

/// Smoke test for the U7 outside-in slice: dispatcher writes the
/// registry, real `ralph emit` writes the channel, and the
/// resolver accepts the per-slot channel only because the
/// `(loop_id, wave_id, slot_index, path)` tuple matches the
/// registry entry.
#[test]
fn u7_real_ralph_emit_writes_marker_signed_channel() {
    let tmp = TempDir::new().expect("temp workspace");
    let workspace = tmp.path();
    let ralph_dir = workspace.join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");

    // Dispatcher-style contract: write the per-wave JSON registry
    // entry the dispatcher would have committed via
    // `WaveChannelRegistry::prepare` before spawning any worker.
    let channel = ralph_dir.join("wave-u7-0.jsonl");
    write_registry(workspace, LOOP_ID, WAVE_ID, SLOT_INDEX, &channel);

    // Spawn the real binary. Scrub agent-context env (HARD RULE 5)
    // and set the wave-worker env vars explicitly so the CLI sees a
    // genuine wave-worker invocation. `RALPH_CURRENT_LOOP_ID` is
    // REQUIRED — the strict registry lookup refuses env-only
    // self-claim from any isolated hat (U2 R4 forgery guard).
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
        .env("RALPH_WAVE_ID", WAVE_ID)
        .env("RALPH_WAVE_INDEX", SLOT_INDEX.to_string())
        .env("RALPH_CURRENT_LOOP_ID", LOOP_ID)
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

/// Forgery guard regression: when no registry entry exists for
/// the `(loop_id, wave_id, slot_index)` tuple, the strict
/// registry lookup refuses the emit so the spawn cannot smuggle a
/// channel write past the dispatcher-signed allowlist. This is
/// the environmental counterpart to the unit-level
/// `test_emit_wave_worker_channel_rejected_without_marker_signature`.
#[test]
fn u7_real_ralph_emit_rejects_when_marker_missing() {
    let tmp = TempDir::new().expect("temp workspace");
    let workspace = tmp.path();
    let ralph_dir = workspace.join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
    // Deliberately do NOT write a registry entry for the
    // (loop_id, wave_id, slot_index) tuple the worker will claim.

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
        .env("RALPH_WAVE_ID", WAVE_ID)
        .env("RALPH_WAVE_INDEX", SLOT_INDEX.to_string())
        .env("RALPH_CURRENT_LOOP_ID", LOOP_ID)
        .current_dir(workspace)
        .output()
        .expect("spawn ralph emit");

    assert!(
        !output.status.success(),
        "ralph emit must reject a forged channel without registry entry; \
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !channel.exists(),
        "channel file must NOT be created when the registry entry is absent"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("registry")
            || stderr.contains("wave-channels")
            || stderr.contains("allowlist")
            || stderr.contains("BindingNotFound")
            || stderr.contains("wave_channel_registry_reject"),
        "stderr must reference the missing registry entry; got: {stderr}"
    );
}
