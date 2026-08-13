//! Reproducer test for PMI-001 — Bootstrap topic label rename requires caller
//! convergence (post-merge-converge preset, .ralph/post-merge/findings/PMI-001.md).
//!
//! Invariant (from PMI-001 §"Invariant" line, file PMI-001.md:32-33):
//!   "A bootstrap event must have a single, canonical label across history,
//!    trusted events file, operator skill docs, and agent skill docs."
//!
//! This test file covers the **skill-doc half** of the invariant. The
//! code side is already covered by
//! `crates/ralph-cli/tests/integration_resume.rs::u7b_resume_topic_constants_are_stable`
//! and `test_continue_publishes_loop_resume_event`, which assert
//! `ralph_proto::LOOP_RESUME == "loop.resume"` and that the trusted events
//! file contains `loop.resume` after a `--continue` boot. Plan 2026-08-13-003
//! U4 converged the code side; PMI-001 is open because the agent-facing skill
//! docs (`ralph-tools.md`, `ralph-tools-recovery-directives.md`) still omit
//! `loop.resume`, leaving agents with only `task.resume` as the resume label —
//! the same doc-vs-code drift the finding's "Actual" section describes
//! (PMI-001.md:27-29).
//!
//! Test status at HEAD (f4dbd1d0, 2026-08-14):
//!   - `pmi_001_skill_doc_ralph_tools_md_mentions_loop_resume` → FAILS
//!     (file matches `loop.resume` 0 times; needs >= 1)
//!   - `pmi_001_skill_doc_recovery_directives_mentions_loop_resume` → FAILS
//!     (file matches `loop.resume` 0 times; needs >= 1)
//!
//! Once the doc side is fixed (see PMI-001.md §"Suggested fix" line 41), both
//! tests turn green. Until then, the failures are the durable repro evidence
//! required by reproducer hat's "stable failing test" rule
//! (`crates/ralph-cli/src/presets.rs` post-merge-converge reproducer §"Verify"
//! block, line 690-694 of CLAUDE.md context).
//!
//! Design notes:
//!   - Tests read source-controlled files via CARGO_MANIFEST_DIR-walked paths
//!     so they do not depend on `cargo run` / a worktree / mock backend.
//!   - Pure read-only assertions (grep-like). No production code modification,
//!     no event emission, no tempdir.
//!   - Deterministic: identical byte counts on every run.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = .../crates/ralph-cli; repo root is two parents up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR must be at crates/ralph-cli")
        .to_path_buf()
}

fn read_required(rel: &str) -> String {
    let abs = repo_root().join(rel);
    fs::read_to_string(&abs).unwrap_or_else(|e| {
        panic!(
            "repro_pmi_001: failed to read {}: {e}. \
             Run `cargo nextest run -p ralph-cli --test repro_pmi_001` from the repo root.",
            abs.display()
        )
    })
}

#[test]
fn pmi_001_skill_doc_ralph_tools_md_mentions_loop_resume() {
    // Invariant half: ralph-tools.md is the always-injected agent skill doc
    // (when memories.enabled or tasks.enabled). If it does not mention
    // `loop.resume` at all, an agent cannot distinguish the bootstrap label
    // from `task.resume` (runtime recovery).
    let body = read_required("crates/ralph-core/data/ralph-tools.md");
    let count = body.matches("loop.resume").count();
    assert!(
        count >= 1,
        "PMI-001 invariant violation: \
         crates/ralph-core/data/ralph-tools.md mentions `loop.resume` {count} time(s); \
         expected >= 1. \
         Code converges on ralph_proto::LOOP_RESUME = \"loop.resume\" \
         (see inner.rs:789 / inner.rs:1356 / state_recovery.rs:273), \
         but the agent-facing skill doc only ever references `task.resume`. \
         Fix per PMI-001.md §\"Suggested fix\" (line 41): \
         clarify that `loop.resume` is the bootstrap event (loop starts via --continue) \
         and `task.resume` is reserved for runtime recovery signals."
    );
}

#[test]
fn pmi_001_skill_doc_recovery_directives_mentions_loop_resume() {
    // Invariant half: ralph-tools-recovery-directives.md is the recovery
    // skill auto-injected on `task.resume`. If it never mentions
    // `loop.resume`, an agent treats the `loop.resume` bootstrap trigger
    // as a recovery signal and routes it through the bounded-retry /
    // correction paths — exactly the misroute the finding's "Impact"
    // describes (PMI-001.md:35-37).
    let body = read_required("crates/ralph-core/data/ralph-tools-recovery-directives.md");
    let count = body.matches("loop.resume").count();
    assert!(
        count >= 1,
        "PMI-001 invariant violation: \
         crates/ralph-core/data/ralph-tools-recovery-directives.md mentions `loop.resume` {count} time(s); \
         expected >= 1. \
         The recovery directives skill treats `task.resume` as the only resume signal \
         and omits `loop.resume` (the actual bootstrap label emitted by --continue). \
         Fix per PMI-001.md §\"Suggested fix\": add a paragraph disambiguating \
         loop.resume (bootstrap) from task.resume (runtime recovery)."
    );
}
