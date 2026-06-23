//! 2026-06-23-005 F1 (R9 BLOCKER): compile-time guard for
//! `RejectionKind` `#[non_exhaustive]` contract.
//!
//! `static_assertions` provides a small set of compile-time
//! property checks (`assert_impl_all!`, `assert_not_impl_any!`).
//! We use it to encode two properties of the typed dispatch SSOT:
//!
//! 1. `RejectionKind` does NOT implement `Default` — variants
//!    are chosen by the gate engine, never default-constructed.
//!    This forces every caller to make an explicit choice and
//!    pairs with the `trybuild` `compile_fail` test (which
//!    proves the match arm is enforced).
//!
//! 2. `RejectionKind` is `Copy` — variants carry no heap state
//!    and the typed dispatch path is hot. Pinning this as a
//!    `const_assert` catches accidental `String` payload fields
//!    that would force a non-`Copy` derive and silently slow
//!    down `dispatch`.
//!
//! The `#[test]` functions are *trivially* satisfied at runtime;
//! the real assertion is the compile-time `assert_impl_all!` /
//! `assert_not_impl_any!` invocation. If the contract breaks, the
//! test binary fails to build, so the test "passes" only when
//! both compile-time checks hold.
//!
//! Reference: `docs/plans/2026-06-23-005-fix-ce-executor-serial-hard-gate-half-edge-recovery-plan.md` R9.
//! Run: `cargo test -p ralph-core --test rejection_kind_static_assert`

use ralph_core::preset::engine::gates::RejectionKind;
use static_assertions::{assert_impl_all, assert_not_impl_any};

// 1. Typed dispatch is hot — variants must be `Copy` to keep
//    `CoordinatorDispatcher::dispatch(k, count)` allocation-free.
//    A future contributor adding a payload field (`String`/`Vec`)
//    would force a non-`Copy` derive and this test would refuse
//    to compile, prompting a discussion on the dispatch path
//    cost trade-off.
assert_impl_all!(RejectionKind: Copy);

// 2. RejectionKind must NOT implement Default. Variants are chosen
//    by the gate engine explicitly; a default-constructed kind
//    would let call sites skip the typed match and silently route
//    through the `_ =>` arm. Pairing this with the trybuild
//    `compile_fail` test makes the typed-dispatch SSOT a
//    two-layer guard: variant selection is required at the API
//    level AND the match arm is enforced at the call site.
assert_not_impl_any!(RejectionKind: Default);

/// Marker test — the load-bearing work is the compile-time
/// `assert_impl_all!` / `assert_not_impl_any!` invocations above.
/// If this test runs at all, the `RejectionKind` SSOT contract
/// held during compilation. The body is intentionally trivial.
#[test]
fn rejection_kind_compile_time_contract_holds() {
    // Reference the type so dead-code elimination does not
    // silently drop the static_assertions block above.
    let _: fn(RejectionKind) -> &'static str = |k| k.reason_code();
}
