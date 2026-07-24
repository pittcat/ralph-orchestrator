//! Integration tests for the `agent_doc_sync` engine (plan §6 U4).
//!
//! These tests exercise `ralph_core::agent_doc_sync::sync_all` from the
//! CLI crate boundary, covering the seven scenarios listed in plan §6:
//!
//! 1. `sync_creates_section_when_file_missing`
//! 2. `sync_appends_block_when_marker_absent`
//! 3. `sync_skips_when_v_matches`
//! 4. `sync_replaces_in_place_on_v_mismatch`
//! 5. `sync_respects_user_content`
//! 6. `sync_retries_lock_then_succeeds`
//! 7. `sync_returns_failed_after_3_lock_retries`
//!
//! Tests use real `tempfile::TempDir` directories and real `FileLock`
//! contention; no mocks of the filesystem or locking layer. Because the
//! lock-contention tests use real threads with shared kernel state, each
//! test runs in isolation against its own temp dir and the tests do not
//! share paths or env vars across cases.
//!
//! Refs: plan §6 U4 — runner integration tests.

use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use ralph_core::agent_doc_sync::block::BlockSpec;
use ralph_core::agent_doc_sync::{OnError, SyncConfig};
use ralph_core::file_lock::FileLock;
use tempfile::TempDir;

/// Sample builtin-style block used across tests. Mirrors the pattern used
/// in `ralph-core` unit tests (see `crates/ralph-core/src/agent_doc_sync/writer.rs`).
fn sample_block() -> BlockSpec {
    BlockSpec::new(
        "hang-prevention",
        "Rule 1\nRule 2\nRule 3\nRule 4\nRule 5\n",
    )
}

/// Build a `SyncConfig` for one target file. Avoids repeating the boilerplate
/// in every test.
fn sync_config_for(blocks: &[BlockSpec]) -> SyncConfig<'_> {
    SyncConfig {
        skip: false,
        on_error: OnError::Warn,
        target_files: &["CLAUDE.md"],
        blocks,
        session_dir: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. sync_creates_section_when_file_missing
// ─────────────────────────────────────────────────────────────────────────────
//
// Invariant: when the target file is absent, `sync_all` creates it with the
// `## Ralph Managed Blocks` section and the block content. `synced` counter
// is incremented by 1; no `failed` entries.
#[test]
fn sync_creates_section_when_file_missing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CLAUDE.md");
    assert!(!path.exists(), "precondition: file must not exist");

    let block = sample_block();
    let report =
        ralph_core::agent_doc_sync::sync_all(dir.path(), &sync_config_for(std::slice::from_ref(&block)))
            .expect("warn mode must not propagate errors");

    assert_eq!(report.synced, 1);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.failed, 0);

    assert!(path.exists(), "file must be created");
    let content = fs::read_to_string(&path).unwrap();
    assert!(
        content.contains("## Ralph Managed Blocks"),
        "section header must be present; got: {content:?}"
    );
    assert!(
        content.contains(&format!(
            "<!-- ralph:begin {} v=sha256:{} -->",
            block.id, block.content_sha256
        )),
        "begin marker must use new sha256; got: {content:?}"
    );
    assert!(
        content.contains("<!-- ralph:end hang-prevention -->"),
        "end marker must be present; got: {content:?}"
    );
    assert!(content.contains("Rule 1"));
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. sync_appends_block_when_marker_absent
// ─────────────────────────────────────────────────────────────────────────────
//
// Invariant: when the file already exists but contains no managed block for
// the given `block_id`, sync appends a new block (no replacement). The user's
// existing content is preserved byte-for-byte at the start of the file.
#[test]
fn sync_appends_block_when_marker_absent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CLAUDE.md");
    let prefix = "# Project\n\nSome pre-existing notes.\n";
    fs::write(&path, prefix).unwrap();

    let block = sample_block();
    let report =
        ralph_core::agent_doc_sync::sync_all(dir.path(), &sync_config_for(std::slice::from_ref(&block)))
            .expect("warn mode must not propagate errors");

    assert_eq!(report.synced, 1, "one block must be appended");
    assert_eq!(report.skipped, 0);
    assert_eq!(report.failed, 0);

    let content = fs::read_to_string(&path).unwrap();
    assert!(
        content.starts_with(prefix),
        "existing content must be preserved; got prefix: {:?}",
        &content[..prefix.len().min(content.len())]
    );
    assert!(
        content.contains("## Ralph Managed Blocks"),
        "section header must be present after append; got: {content:?}"
    );
    assert!(
        content.contains(&format!(
            "<!-- ralph:begin {} v=sha256:{} -->",
            block.id, block.content_sha256
        )),
        "new begin marker must use new sha256; got: {content:?}"
    );
    assert!(content.contains("Rule 1"));

    // Sanity: exactly one begin marker (no duplicate).
    let begin_count = content.matches("<!-- ralph:begin hang-prevention").count();
    assert_eq!(
        begin_count, 1,
        "exactly one begin marker expected, got {begin_count}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. sync_skips_when_v_matches
// ─────────────────────────────────────────────────────────────────────────────
//
// Invariant: when both markers are present and the embedded `v=sha256:HEX`
// matches the builtin hash, sync performs zero writes. The file's content,
// mtime, and size remain identical.
#[test]
fn sync_skips_when_v_matches() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CLAUDE.md");
    let block = sample_block();

    let existing = format!(
        "# Project\n\n## Ralph Managed Blocks\n\n\
         <!-- ralph:begin hang-prevention v=sha256:{} -->\n\
         Rule 1\nRule 2\nRule 3\nRule 4\nRule 5\n\
         <!-- ralph:end hang-prevention -->\n",
        block.content_sha256
    );
    fs::write(&path, &existing).unwrap();

    let original_mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let original_size = fs::metadata(&path).unwrap().len();
    let original_content = fs::read_to_string(&path).unwrap();

    // Sleep long enough that a real write would tick the mtime past resolution.
    thread::sleep(Duration::from_millis(50));

    let report = ralph_core::agent_doc_sync::sync_all(dir.path(), &sync_config_for(&[block]))
        .expect("warn mode must not propagate errors");

    assert_eq!(report.skipped, 1, "block must be reported as skipped");
    assert_eq!(report.synced, 0);
    assert_eq!(report.failed, 0);

    let after_content = fs::read_to_string(&path).unwrap();
    assert_eq!(
        after_content, original_content,
        "content must be byte-identical when v matches"
    );

    let after_mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let after_size = fs::metadata(&path).unwrap().len();
    assert_eq!(after_size, original_size, "size must be unchanged");
    assert_eq!(
        after_mtime, original_mtime,
        "mtime must not change when sync is a no-op"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. sync_replaces_in_place_on_v_mismatch
// ─────────────────────────────────────────────────────────────────────────────
//
// Invariant: when both markers are present and the embedded `v=sha256:HEX`
// does NOT match the builtin hash, sync replaces only the content between
// the markers; the section header (`## Ralph Managed Blocks`) and any other
// user content outside the block region are preserved byte-for-byte.
#[test]
fn sync_replaces_in_place_on_v_mismatch() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CLAUDE.md");
    let block = sample_block();

    // 64 hex chars; deliberately not equal to block.content_sha256.
    let stale_hash = "a".repeat(64);
    let existing = format!(
        "# Project\n\n## Ralph Managed Blocks\n\n\
         <!-- ralph:begin hang-prevention v=sha256:{stale_hash} -->\n\
         stale old content\n\
         <!-- ralph:end hang-prevention -->\n"
    );
    fs::write(&path, &existing).unwrap();

    let report =
        ralph_core::agent_doc_sync::sync_all(dir.path(), &sync_config_for(std::slice::from_ref(&block)))
            .expect("warn mode must not propagate errors");

    assert_eq!(report.synced, 1, "block must be reported as replaced");
    assert_eq!(report.skipped, 0);
    assert_eq!(report.failed, 0);

    let content = fs::read_to_string(&path).unwrap();

    // Section header preserved.
    assert!(
        content.contains("## Ralph Managed Blocks"),
        "section header must be preserved; got: {content:?}"
    );

    // Begin marker now points at new hash; old hash absent.
    assert!(
        content.contains(&format!(
            "<!-- ralph:begin hang-prevention v=sha256:{} -->",
            block.content_sha256
        )),
        "begin marker must reference new sha256; got: {content:?}"
    );
    assert!(
        !content.contains(&format!("v=sha256:{stale_hash}")),
        "stale hash must be gone; got: {content:?}"
    );

    // Old content between markers replaced; new content present.
    assert!(
        !content.contains("stale old content"),
        "stale body must be replaced; got: {content:?}"
    );
    assert!(content.contains("Rule 1"));

    // User content outside the block region preserved.
    assert!(
        content.contains("# Project"),
        "user-written header must be preserved; got: {content:?}"
    );

    // Exactly one begin marker (no duplicate).
    let begin_count = content.matches("<!-- ralph:begin hang-prevention").count();
    assert_eq!(
        begin_count, 1,
        "exactly one begin marker expected, got {begin_count}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. sync_respects_user_content
// ─────────────────────────────────────────────────────────────────────────────
//
// Invariant: when the file contains only user-written content (no
// `## Ralph Managed Blocks` section), sync must append the section at the
// end. The user's leading content is preserved byte-for-byte; sync never
// rewrites user content.
#[test]
fn sync_respects_user_content() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CLAUDE.md");
    let user_content =
        "# My Project\n\nUser-written intro.\n\n## Notes\n\nUser-written notes here.\n";
    fs::write(&path, user_content).unwrap();

    let block = sample_block();
    let report = ralph_core::agent_doc_sync::sync_all(dir.path(), &sync_config_for(&[block]))
        .expect("warn mode must not propagate errors");

    assert_eq!(report.synced, 1);
    assert_eq!(report.failed, 0);

    let content = fs::read_to_string(&path).unwrap();

    // Leading user content byte-identical.
    assert!(
        content.starts_with(user_content),
        "user content must be preserved byte-for-byte; got prefix: {:?}",
        &content[..user_content.len().min(content.len())]
    );

    // Section appended at end.
    assert!(
        content.contains("## Ralph Managed Blocks"),
        "section header must be appended; got: {content:?}"
    );
    assert!(content.contains("Rule 1"));

    // Order check: user content precedes managed section.
    let user_pos = content.find("# My Project").unwrap();
    let section_pos = content.find("## Ralph Managed Blocks").unwrap();
    assert!(
        user_pos < section_pos,
        "user content must precede managed section"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. sync_retries_lock_then_succeeds
// ─────────────────────────────────────────────────────────────────────────────
//
// Invariant: when an external holder briefly holds the file lock and
// releases it before the retry budget is exhausted, sync must succeed on
// a later attempt. This exercises the 3-attempt retry path inside
// `writer::try_lock_with_retry`.
#[test]
fn sync_retries_lock_then_succeeds() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CLAUDE.md");
    let block = sample_block();

    // Two threads rendezvous via a barrier so the lock holder releases the
    // file lock AFTER the first sync attempt has failed, but BEFORE the
    // full 3x50ms = 150ms retry window has elapsed.
    let release_after_ms = 75_u64;
    let barrier = Arc::new(Barrier::new(2));
    let holder_path = path.clone();
    let holder_barrier = Arc::clone(&barrier);

    let holder = thread::spawn(move || {
        let lock = FileLock::new(&holder_path).unwrap();
        let _guard = lock
            .exclusive()
            .expect("holder must acquire exclusive lock");

        // Signal the main thread that the lock is now held; then sleep long
        // enough for the first sync attempt to fail and consume ~50ms of
        // the retry budget, but short enough that the third attempt still
        // succeeds after we release.
        holder_barrier.wait();
        thread::sleep(Duration::from_millis(release_after_ms));
        drop(_guard);
    });

    // Wait until the holder has acquired the lock, then start sync.
    barrier.wait();
    let started = Instant::now();
    let report =
        ralph_core::agent_doc_sync::sync_all(dir.path(), &sync_config_for(std::slice::from_ref(&block)))
            .expect("warn mode must not propagate errors");
    let elapsed = started.elapsed();

    holder.join().expect("holder thread must not panic");

    assert_eq!(
        report.synced, 1,
        "sync must succeed after holder releases; report={report:?}"
    );
    assert_eq!(report.failed, 0);
    assert!(
        elapsed < Duration::from_millis(500),
        "sync must not hang; took {elapsed:?}"
    );

    // File is on disk with new content.
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("## Ralph Managed Blocks"));
    assert!(content.contains(&format!(
        "<!-- ralph:begin hang-prevention v=sha256:{} -->",
        block.content_sha256
    )));
    assert!(content.contains("Rule 1"));
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. sync_returns_failed_after_3_lock_retries
// ─────────────────────────────────────────────────────────────────────────────
//
// Invariant: when the file lock is held for longer than the 3x50ms retry
// budget, sync records `failed = blocks.len()` and returns `Ok(report)`
// under `OnError::Warn` (default). The file must not be created, and the
// attempt must complete in roughly the retry window, not hang.
#[test]
fn sync_returns_failed_after_3_lock_retries() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CLAUDE.md");
    let block = sample_block();

    // Hold the lock for substantially longer than 3 × 50ms = 150ms so all
    // retry attempts fail.
    let holder = thread::spawn({
        let holder_path = path.clone();
        move || {
            let lock = FileLock::new(&holder_path).unwrap();
            let _guard = lock
                .exclusive()
                .expect("holder must acquire exclusive lock");
            thread::sleep(Duration::from_millis(500));
            drop(_guard);
        }
    });

    // Brief delay to ensure the holder has the lock before we start.
    thread::sleep(Duration::from_millis(20));

    let started = Instant::now();
    let report = ralph_core::agent_doc_sync::sync_all(dir.path(), &sync_config_for(&[block]))
        .expect("warn mode must not propagate errors");
    let elapsed = started.elapsed();

    holder.join().expect("holder thread must not panic");

    assert_eq!(
        report.failed, 1,
        "block must be reported as failed when lock never acquired; report={report:?}"
    );
    assert_eq!(report.synced, 0);
    assert_eq!(report.skipped, 0);

    // File must not exist — sync did not write anything.
    assert!(
        !path.exists(),
        "file must not be created when sync fails; got: {:?}",
        fs::read_to_string(&path)
    );

    // Sanity: total wait is at least the retry budget and far less than
    // an unbounded hang. The writer's retry loop runs 3 attempts with
    // 50ms sleeps between them, so the floor is roughly 2 × 50ms = 100ms
    // (the final attempt does not sleep before returning).
    assert!(
        elapsed >= Duration::from_millis(80),
        "sync must wait through retry budget; took {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(800),
        "sync must not exceed retry budget by much; took {elapsed:?}"
    );
}
