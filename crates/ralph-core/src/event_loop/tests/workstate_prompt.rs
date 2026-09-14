//! Plan 2026-09-14-001 Unit 2: `## WORKSTATE` prompt injection tests.
//!
//! The loop-scoped workstate store (Unit 1) is injected into the hat
//! activation prompt as a `## WORKSTATE` block. These tests pin the
//! contract:
//!
//! - `workstate_prompt_empty_store_no_block` (characterization): with an
//!   empty/absent store the injection is a strict no-op — the prefix is
//!   byte-identical.
//! - `workstate_prompt_injects_current_loop_entries`: entries of the
//!   current loop appear escaped.
//! - `workstate_prompt_disabled_or_manual_no_block`: `enabled: false` or
//!   `inject: manual` disable the block.
//! - `workstate_prompt_budget_truncates_at_entry_boundary`: budget
//!   truncation happens at entry boundaries with a visible marker.
//! - `workstate_prompt_only_current_loop`: entries from other loops never
//!   leak into the block.
//!
//! The pure renderer (`EventLoop::render_workstate_block`) has its own
//! unit tests at the bottom of this file.

use super::*;
use crate::workstate::{WorkstateEntry, WorkstateStore};

/// Build an EventLoop over a fresh tempdir workspace, write the given
/// `(loop_id, key, value)` rows into the workstate store, run
/// `inject_workstate` for `inject_loop`, and return the resulting prefix.
fn inject_with_entries(
    rows: &[(Option<&str>, &str, &str)],
    inject_loop: Option<&str>,
    configure: impl FnOnce(&mut RalphConfig),
) -> String {
    let temp_root = tempfile::tempdir().expect("create tempdir for workstate_prompt test");
    let workspace_root = temp_root.path().to_path_buf();

    let store = WorkstateStore::with_default_path(&workspace_root);
    for (loop_id, key, value) in rows {
        store
            .set(*loop_id, key, value, Some("tester"))
            .expect("seed workstate row");
    }

    let mut config = common::minimal_isolated_config(false, false);
    config.core.workspace_root = workspace_root;
    configure(&mut config);

    let event_loop = EventLoop::new(config);
    let mut prefix = String::new();
    event_loop.inject_workstate(&mut prefix, inject_loop);
    // temp_root drops here; no manual cleanup needed.
    prefix
}

/// Characterization: with no store file at all (or an empty store), the
/// injection must leave the prefix byte-identical — zero prompt change.
#[test]
fn workstate_prompt_empty_store_no_block() {
    let prefix = inject_with_entries(&[], Some("loop-1"), |_| {});
    assert!(
        prefix.is_empty(),
        "empty workstate store must not change the prompt prefix; got: {prefix:?}"
    );
}

/// Entries of the current loop are rendered, escaped, and carry the
/// `## WORKSTATE` heading.
#[test]
fn workstate_prompt_injects_current_loop_entries() {
    let prefix = inject_with_entries(
        &[
            (Some("loop-1"), "alpha", "方案A待验证"),
            (Some("loop-1"), "beta", "line1\nline2 `tick`"),
        ],
        Some("loop-1"),
        |_| {},
    );
    assert!(
        prefix.contains("## WORKSTATE"),
        "non-empty store must produce a WORKSTATE block; prefix={prefix}"
    );
    assert!(
        prefix.contains("alpha") && prefix.contains("方案A待验证"),
        "entry alpha must appear with its value; prefix={prefix}"
    );
    // Newlines and backticks in values must be escaped before they reach
    // the prompt (same contract as the handoff-envelope renderer).
    assert!(
        prefix.contains("line1\\nline2 ``tick``"),
        "value must be escaped (newline -> \\n literal, backtick doubled); prefix={prefix}"
    );
    assert!(
        !prefix.contains("line1\nline2"),
        "raw newline must not survive into the prompt block; prefix={prefix}"
    );
}

/// `workstate.enabled: false` or `workstate.inject: manual` must both
/// leave the prefix byte-identical (zero prompt change).
#[test]
fn workstate_prompt_disabled_or_manual_no_block() {
    let rows: &[(Option<&str>, &str, &str)] = &[(Some("loop-1"), "alpha", "v")];

    let disabled = inject_with_entries(rows, Some("loop-1"), |config: &mut RalphConfig| {
        config.workstate.enabled = false
    });
    assert!(
        disabled.is_empty(),
        "workstate.enabled=false must skip injection; prefix={disabled}"
    );

    let manual = inject_with_entries(rows, Some("loop-1"), |config: &mut RalphConfig| {
        config.workstate.inject = InjectMode::Manual;
    });
    assert!(
        manual.is_empty(),
        "workstate.inject=manual must skip injection; prefix={manual}"
    );
}

/// A small budget truncates at an entry boundary and appends a visible
/// truncation marker; the omitted entries' keys never appear.
#[test]
fn workstate_prompt_budget_truncates_at_entry_boundary() {
    let prefix = inject_with_entries(
        &[
            (Some("loop-1"), "k1", "v1"),
            (Some("loop-1"), "k2", "v2"),
            (Some("loop-1"), "k3", "v3"),
        ],
        Some("loop-1"),
        |config: &mut RalphConfig| config.workstate.budget = 6,
    );
    assert!(
        prefix.contains("## WORKSTATE"),
        "block header must survive budget truncation; prefix={prefix}"
    );
    assert!(
        prefix.contains("k1"),
        "the first entry must fit; prefix={prefix}"
    );
    assert!(
        !prefix.contains("k2") && !prefix.contains("k3"),
        "entries past the budget must be dropped at the entry boundary; prefix={prefix}"
    );
    assert!(
        prefix.contains("<!-- truncated:"),
        "a truncation marker must make the cut visible; prefix={prefix}"
    );
}

/// Entries belonging to other loops (or the loop-less human scope) must
/// never leak into the current loop's block.
#[test]
fn workstate_prompt_only_current_loop() {
    let prefix = inject_with_entries(
        &[
            (Some("loop-1"), "mine", "v-mine"),
            (Some("loop-2"), "other-loop-key", "v-other"),
            (None, "human-key", "v-human"),
        ],
        Some("loop-1"),
        |_| {},
    );
    assert!(
        prefix.contains("mine"),
        "current loop entry must appear; prefix={prefix}"
    );
    assert!(
        !prefix.contains("other-loop-key") && !prefix.contains("human-key"),
        "foreign-loop and loop-less entries must not leak; prefix={prefix}"
    );
}

// ---------------------------------------------------------------------
// `EventLoop::render_workstate_block` — pure renderer unit tests.
// ---------------------------------------------------------------------

fn entry(key: &str, value: &str) -> WorkstateEntry {
    WorkstateEntry {
        loop_id: Some("loop-1".to_string()),
        key: key.to_string(),
        value: value.to_string(),
        updated_at_ms: 1,
        hat: None,
        deleted: false,
    }
}

#[test]
fn render_workstate_block_empty_returns_none() {
    assert_eq!(EventLoop::render_workstate_block(&[], 0), None);
}

#[test]
fn render_workstate_block_single_entry() {
    let block = EventLoop::render_workstate_block(&[entry("alpha", "v")], 0)
        .expect("single entry must render");
    assert!(block.starts_with("## WORKSTATE\n"));
    assert!(block.contains("alpha: v"));
}

#[test]
fn render_workstate_block_sorts_entries_by_key() {
    let entries = [entry("zeta", "1"), entry("alpha", "2"), entry("mid", "3")];
    let block = EventLoop::render_workstate_block(&entries, 0).expect("render");
    let alpha_pos = block.find("alpha").expect("alpha present");
    let mid_pos = block.find("mid").expect("mid present");
    let zeta_pos = block.find("zeta").expect("zeta present");
    assert!(
        alpha_pos < mid_pos && mid_pos < zeta_pos,
        "entries must render sorted by key; block={block}"
    );
}

#[test]
fn render_workstate_block_escapes_key_and_value() {
    let entries = [entry("k", "a\nb`c\x07")];
    let block = EventLoop::render_workstate_block(&entries, 0).expect("render");
    assert!(
        block.contains("a\\nb``c\\x07"),
        "newlines, backticks and control chars must be escaped; block={block}"
    );
    assert!(
        !block.contains('\x07'),
        "raw control chars must not survive into the block; block={block:?}"
    );
}

#[test]
fn render_workstate_block_budget_zero_is_unlimited() {
    let big = "x".repeat(5000);
    let entries = [entry("k", &big)];
    let block = EventLoop::render_workstate_block(&entries, 0).expect("render");
    assert!(block.contains(&big), "budget 0 must not truncate");
    assert!(!block.contains("<!-- truncated:"));
}

#[test]
fn render_workstate_block_budget_exact_fit_has_no_marker() {
    // The smallest budget that still fits the full render must reproduce
    // the unbounded render exactly (no truncation marker).
    let entries = [entry("k1", "v1"), entry("k2", "v2")];
    let full = EventLoop::render_workstate_block(&entries, 0).expect("render");
    let exact_budget = full.len().div_ceil(4);
    let fitted = EventLoop::render_workstate_block(&entries, exact_budget).expect("render");
    assert_eq!(
        fitted, full,
        "an exactly-fitting budget must not alter the render"
    );
}
