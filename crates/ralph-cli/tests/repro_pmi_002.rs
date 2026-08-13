//! Reproducer test for PMI-002 — `publish_targeted_resume_for_hat` legacy
//! fallback drops resolver priority chain
//! (post-merge-converge preset, .ralph/post-merge/findings/PMI-002.md).
//!
//! Invariant (from PMI-002 §"Invariant" line, file PMI-002.md:31-33):
//!   "A legacy path that the plan explicitly migrated (U2) must preserve
//!    the same identity resolution contract as the new path; the change
//!    cannot silently drop the D2 priority chain on no-ledger branches."
//!
//! The doc-comment of `publish_targeted_resume_for_hat` (resume_routing.rs
//! lines 535-540) claims the wrapper "now threads `payload_target_hat`
//! from the resume payload through the resolver" so the D2 priority chain
//! is exercised in production. The runtime wrapper that production callers
//! use is `task_resume_ingress` (resume_routing.rs:574-615). Its
//! `ledger=None` branch (the legacy fallback, lines 602-613) hard-codes
//! `payload_target_hat: None`, `task_id: None`, and `task_key: None` — it
//! never reads the payload. The D2 priority chain therefore becomes
//! unreachable on the legacy path.
//!
//! This test covers the **wrapper half** of the invariant. The resolver
//! half is already covered by `task_resume_runtime_routing.rs::u2_publish_targeted_resume_for_hat_threads_payload_target`
//! (which exercises the production `publish_targeted_resume_for_hat` only
//! when callers pass `payload_target_hat` explicitly).
//!
//! Test status at HEAD (f4dbd1d0, 2026-08-14):
//!   - `pmi_002_legacy_branch_drops_payload_target_hat` → FAILS
//!     (the source file `resume_routing.rs` legacy branch contains the
//!     literal `None,` at the `payload_target_hat` argument slot; the
//!     symptom is byte-stable across runs).
//!   - `pmi_002_doc_claim_threads_payload_target_hat` → PASSES (the doc
//!     comment is present at HEAD). This is the anchor that ties the
//!     contract claim to the gap so a future fix does not delete the
//!     doc without restoring the code.
//!
//! Once the production wrapper threads the payload through
//! `payload_target_hat(payload)` / `payload_task_id(payload)` /
//! `payload_task_key(payload)` (the helpers already exported from
//! resume_routing.rs lines 482-510), the first test turns green. The
//! second test stays green as long as the plan 003 U2 contract is
//! documented.
//!
//! Design notes (post-merge-converge reproducer §"DON'T" compliance):
//!   - Pure read-only assertions on source-controlled files. No
//!     production code modification, no event emission, no tempdir.
//!   - Deterministic: identical byte counts on every run.
//!   - Mirrors `repro_pmi_001.rs` style (grep / source-controlled file
//!     inspection) so future reproducers stay consistent.

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
            "repro_pmi_002: failed to read {}: {e}. \
             Run `cargo nextest run -p ralph-cli --test repro_pmi_002` from the repo root.",
            abs.display()
        )
    })
}

/// Slice the source of `task_resume_ingress`'s `None =>` (legacy) branch.
/// The wrapper lives at resume_routing.rs:574-615. The legacy branch is
/// the second arm of the `match ledger` and produces the call to
/// `publish_targeted_resume_for_hat` with hard-coded `None` for the
/// payload-derived identity fields.
fn legacy_branch_body() -> String {
    let src = read_required("crates/ralph-core/src/event_loop/resume_routing.rs");
    // Anchor on the `None => publish_targeted_resume_for_hat(` call so the
    // test does not silently drift if the wrapper gains a second
    // `match` arm in the future.
    let start = src
        .find("None => publish_targeted_resume_for_hat(")
        .unwrap_or_else(|| {
            panic!(
                "PMI-002 anchor missing: `None => publish_targeted_resume_for_hat(` not found in \
                 crates/ralph-core/src/event_loop/resume_routing.rs. The legacy fallback wrapper \
                 may have been refactored; review PMI-002 §\"Suggested fix\" against the new shape."
            )
        });
    // Restricted search: find the next `        )` (8-space indent) AFTER
    // the anchor, not the first occurrence in the file. The legacy branch
    // call is exactly 11 lines long (one open line + 9 argument lines + one
    // closing `        ),` line), so the first `        )` in the slice
    // [start..] is the closing paren of the call.
    let after = &src[start..];
    let close_offset = after.find("        )").unwrap_or_else(|| {
        panic!(
            "PMI-002 anchor missing: closing `        )` for the legacy fallback call not found \
             after the `None => publish_targeted_resume_for_hat(` anchor."
        )
    });
    // Include the line containing the closing paren.
    let end = close_offset + "        )".len();
    src[start..start + end].to_string()
}

#[test]
fn pmi_002_legacy_branch_drops_payload_target_hat() {
    // Invariant half: the production wrapper `task_resume_ingress`'s
    // legacy (no-ledger) branch must call at least one of the
    // payload-extraction helpers — `payload_target_hat(payload)`,
    // `payload_task_id(payload)`, or `payload_task_key(payload)` — so
    // the D2 priority chain is exercised end-to-end. The current
    // implementation passes `None, None, None` for the three identity
    // argument slots, which means the resolver never sees the payload's
    // declared target / task identity.
    let body = legacy_branch_body();

    // The legacy branch passes `payload_target_hat: None` directly.
    // We assert against the *presence* of `payload_target_hat(` (the
    // helper call) — a string match on the helper invocation is
    // robust to formatting changes while still catching the contract
    // drift.
    let calls_payload_target_hat = body.contains("payload_target_hat(");
    let calls_payload_task_id = body.contains("payload_task_id(");
    let calls_payload_task_key = body.contains("payload_task_key(");

    assert!(
        calls_payload_target_hat || calls_payload_task_id || calls_payload_task_key,
        "PMI-002 invariant violation: production `task_resume_ingress` legacy branch (no-ledger) \
         never reads the payload. Captured legacy branch body:\n\n{body}\n\n\
         Per PMI-002.md §\"Suggested fix\" (line 41): the legacy fallback must call \
         `payload_target_hat(payload)` (or `payload_task_id` / `payload_task_key`) so the D2 \
         priority chain is exercised in production. The plan 003 U2 doc-comment at \
         resume_routing.rs:535-540 explicitly promises this behaviour, but the wrapper body \
         at resume_routing.rs:602-613 still passes `None, None, None` for the three identity \
         argument slots — the resolver never sees the payload's declared target / task identity."
    );
}

#[test]
fn pmi_002_doc_claim_threads_payload_target_hat() {
    // Anchor: the doc-comment of `publish_targeted_resume_for_hat` is the
    // load-bearing contract claim from plan 003 U2. If the claim is
    // removed or weakened, future readers will not know the wrapper was
    // supposed to thread the payload through the resolver. This test
    // pins the contract so a fix that satisfies the runtime test cannot
    // silently drop the claim.
    let src = read_required("crates/ralph-core/src/event_loop/resume_routing.rs");
    let doc_anchor = "pub fn publish_targeted_resume_for_hat(";
    let doc_start = src.find(doc_anchor).unwrap_or_else(|| {
        panic!(
            "PMI-002 anchor missing: function `publish_targeted_resume_for_hat` not found in \
             crates/ralph-core/src/event_loop/resume_routing.rs."
        )
    });
    // Walk backwards to find the start of the doc comment block.
    let doc_window = &src[..doc_start];
    let doc_block_start = doc_window
        .rfind("/// Plan 2026-08-13-003 U2")
        .or_else(|| doc_window.rfind("/// U2"))
        .unwrap_or_else(|| {
            panic!(
                "PMI-002 anchor missing: doc-comment of `publish_targeted_resume_for_hat` is \
                 not preceded by a `/// Plan 2026-08-13-003 U2` (or `/// U2`) marker. The plan 003 \
                 contract claim must remain in the doc-comment so the contract drift is visible."
            )
        });
    let doc_block = &src[doc_block_start..doc_start];
    let doc_block = doc_block.trim();

    let claims_threading = doc_block.contains("payload_target_hat")
        && (doc_block.contains("threads")
            || doc_block.contains("priority chain")
            || doc_block.contains("resolver"));

    assert!(
        claims_threading,
        "PMI-002 contract claim dropped: the doc-comment of `publish_targeted_resume_for_hat` \
         no longer mentions threading `payload_target_hat` through the resolver / priority chain. \
         Captured doc block (resume_routing.rs preceding pub fn):\n\n{doc_block}\n\n\
         Per plan 2026-08-13-003 U2, the wrapper must preserve the contract that \
         `payload_target_hat` is threaded from the resume payload through the resolver. If the \
         contract is intentionally revised, update PMI-002.md §\"Suggested fix\" accordingly \
         and bump this test instead of deleting the claim silently."
    );
}
