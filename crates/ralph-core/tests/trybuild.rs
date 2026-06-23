//! 2026-06-23-005 F1 (R9 BLOCKER): `trybuild` compile_fail harness.
//!
//! Pins the `RejectionKind` `#[non_exhaustive]` contract: the
//! `CoordinatorDispatcher::dispatch` match must keep the explicit
//! `_ =>` fallback arm, AND future variants added to `RejectionKind`
//! must NOT silently bypass the typed dispatch table.
//!
//! If the next refactor removes the `_ => CoordinatorAction::ReEmitWorkReady`
//! arm, the test file under `tests/ui/` will compile-fail and
//! trybuild will diff the produced stderr against the recorded
//! baseline (`tests/ui/non_exhaustive_match.stderr`).
//!
//! Run: `cargo test -p ralph-core --test trybuild`

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/non_exhaustive_match.rs");
}
