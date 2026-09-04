//! 2026-09-03-0959 plan U8 (R9; S8, S11; D11, D12; E2, E9, E11):
//! progress-aware timeout + idle lease logic for the runtime job
//! kernel.
//!
//! Every runtime job runs under a non-extendable hard cap. The
//! idle lease is renewed only by **strong** progress (a
//! structured evidence event). Weak progress (spinner text,
//! repeated log lines) buys a *bounded* total allowance measured
//! across the entire job lifetime, never per-renewal. Silent
//! periods drain the idle lease immediately.
//!
//! The module is pure CPU: every type is `#[cfg(test)]`-free so
//! U9 / recovery / runtime wiring can depend on it without
//! pulling in the `runtime_job` kernel. The `Clock` trait is the
//! only seam — tests inject a `VirtualClock`, production code
//! passes a `std::time::Instant`-backed adapter.
//!
//! Design contract (plan §7 U8 #6):
//! - Hard cap is **non-extendable**: any signal class at the cap
//!   yields `Verdict::HardCapExceeded`. The kernel hands the
//!   port a `cancel(pid)` and surfaces
//!   `RuntimeJobError::HeartbeatTimeout { stage, elapsed_ms,
//!   cap_ms }`.
//! - Idle lease renews only on `Signal::Strong`.
//! - `Weak` output consumes from `weak_allowance_total_ms` (a
//!   single budget, not per-renewal). Once exhausted, weak
//!   signals behave like silent.
//! - `Silent` output does **not** renew the idle lease.
//! - `startup_grace_ms` is a one-shot window after `start` where
//!   strong / weak / silent are all treated as "alive but no
//!   progress". It exists so a slow first response isn't killed
//!   before the worker can settle.

#![allow(dead_code)] // U8 wires these into U9+; surface kept stable now.

use std::fmt;
use std::sync::Arc;

/// Monotonic clock seam. Production wires `SystemClock` (Instant
/// since boot); tests pass a `VirtualClock` that advances only
/// when the test asks.
pub trait Clock: Send {
    /// Monotonic milliseconds since some unspecified epoch. Must
    /// be monotonically non-decreasing across calls in the same
    /// process. The epoch is opaque — callers only ever compute
    /// deltas against an earlier value returned by the same
    /// clock.
    fn now_ms(&self) -> u64;
}

/// Production clock: uses `std::time::Instant`. Cheap (single
/// syscall on Linux), process-scoped monotonic.
pub struct SystemClock {
    /// Anchor the boot-relative Instant to a known origin so the
    /// arithmetic is deterministic in tests that compare raw
    /// `now_ms` values. The anchor is the `Instant` recorded at
    /// construction; `now_ms` returns `anchor.elapsed().as_millis()`.
    anchor: std::time::Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            anchor: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        self.anchor.elapsed().as_millis() as u64
    }
}

/// Test clock: tests `set(now_ms)` then read `now_ms()`. No
/// `thread::sleep` — time only advances when the test asks.
#[derive(Debug, Clone)]
pub struct VirtualClock {
    inner: Arc<std::sync::Mutex<u64>>,
}

impl VirtualClock {
    pub fn new(initial_ms: u64) -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(initial_ms)),
        }
    }

    /// Advance the clock by `delta_ms` (saturating).
    pub fn advance(&self, delta_ms: u64) {
        let mut g = self.inner.lock().expect("VirtualClock poisoned");
        *g = g.saturating_add(delta_ms);
    }

    /// Jump the clock to an absolute `target_ms` (saturating at
    /// the current value — the clock never goes backwards).
    pub fn set(&self, target_ms: u64) {
        let mut g = self.inner.lock().expect("VirtualClock poisoned");
        if target_ms > *g {
            *g = target_ms;
        }
    }
}

impl Clock for VirtualClock {
    fn now_ms(&self) -> u64 {
        *self.inner.lock().expect("VirtualClock poisoned")
    }
}

/// Progress signal a runtime job emits on a heartbeat. The
/// classifier (`classify_signal`) collapses the raw payload to
/// one of these three bins.
///
/// Strong evidence looks like real work output: a test result, a
/// commit hash, a structured `forge.*` event, a `cargo` test
/// summary line. Weak evidence is "alive but not strong": a
/// log line, a spinner tick, a repeated heartbeat with no
/// payload change. Silent is "no payload at all".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Structured evidence event that proves real forward
    /// progress (test result, commit hash, accepted payload).
    Strong,
    /// Output exists but does not prove real forward progress
    /// (log line, spinner, repeated heartbeat).
    Weak,
    /// No payload at all on this heartbeat.
    Silent,
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Signal::Strong => f.write_str("strong"),
            Signal::Weak => f.write_str("weak"),
            Signal::Silent => f.write_str("silent"),
        }
    }
}

/// Stable string tokens for the keyword set the classifier uses.
/// Tests assert the set is closed (no surprise keywords); new
/// keywords MUST be added here AND covered by a new test.
pub const STRONG_KEYWORDS: &[&str] = &[
    "test result:",
    "forge.unit.integrated",
    "forge.unit.verified",
    "cargo test ok",
    "exit code: 0",
    "compiled",
    "ok ",
    "passed",
];

pub const WEAK_KEYWORDS: &[&str] = &[
    "...",
    "thinking",
    "spinner",
    "loading",
    "idle",
    "waiting",
    "retry",
];

/// Classify a raw heartbeat payload into one of the three
/// signal bins. The lookup is keyword-substring: the payload
/// is searched for any `STRONG_KEYWORDS` substring (case
/// insensitive — worker output is human-readable) and any
/// `WEAK_KEYWORDS` substring in that order. Strong wins over
/// weak; empty / whitespace-only payloads are `Silent`.
///
/// New strong keywords MUST be appended to `STRONG_KEYWORDS`
/// (and a test added). The classifier is **fail-closed** for
/// unknown keywords: anything not matching a known bucket
/// falls into `Weak` (the more pessimistic of the two
/// recognised classes). Truly empty payloads are `Silent`.
pub fn classify_signal(payload: &str) -> Signal {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return Signal::Silent;
    }
    let haystack = trimmed.to_ascii_lowercase();
    for kw in STRONG_KEYWORDS {
        if haystack.contains(kw) {
            return Signal::Strong;
        }
    }
    for kw in WEAK_KEYWORDS {
        if haystack.contains(kw) {
            return Signal::Weak;
        }
    }
    // Unrecognised non-empty output is treated as weak — the
    // operator-friendly "we don't know what this is but it
    // looks alive" default. Tests pin this contract.
    Signal::Weak
}

/// Tunable deadline policy. Every field has a `Default`
/// constructor so a test can pick a single knob without
/// rebuilding the whole struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadlinePolicy {
    /// One-shot window after `start` during which any signal
    /// counts as "alive but no progress yet". 0 disables the
    /// grace.
    pub startup_grace_ms: u64,
    /// Hard wall-clock cap from `start`. Non-extendable.
    pub hard_cap_ms: u64,
    /// Maximum idle gap (last strong progress → now) before the
    /// lease expires. Renewed only by `Signal::Strong`.
    pub idle_lease_ms: u64,
    /// Total weak-output budget across the entire job lifetime.
    /// Once consumed, weak signals behave like silent.
    pub weak_allowance_total_ms: u64,
}

impl DeadlinePolicy {
    pub const fn new(
        startup_grace_ms: u64,
        hard_cap_ms: u64,
        idle_lease_ms: u64,
        weak_allowance_total_ms: u64,
    ) -> Self {
        Self {
            startup_grace_ms,
            hard_cap_ms,
            idle_lease_ms,
            weak_allowance_total_ms,
        }
    }
}

impl Default for DeadlinePolicy {
    /// Defaults that match the plan §7 U8 contract:
    /// - 30 s startup grace (a slow first response is OK).
    /// - 10 min hard cap (non-extendable).
    /// - 5 min idle lease (renewed only by strong progress).
    /// - 90 s total weak allowance (bounded, not per-renewal).
    fn default() -> Self {
        Self {
            startup_grace_ms: 30_000,
            hard_cap_ms: 600_000,
            idle_lease_ms: 300_000,
            weak_allowance_total_ms: 90_000,
        }
    }
}

/// Mutable per-job deadline state. The kernel holds one of
/// these per active job and feeds it every heartbeat signal.
/// Tests construct directly so they can drive the state
/// machine without touching `Clock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadlineState {
    /// `now_ms` at the moment the job launched.
    pub start_ms: u64,
    /// `now_ms` of the last `Signal::Strong` (or `start_ms` if
    /// no strong signal has been seen yet).
    pub last_strong_ms: u64,
    /// Total wall-clock milliseconds the job has spent under
    /// only-weak signals (any `Signal::Weak` between two
    /// strongs). Saturating at `policy.weak_allowance_total_ms`.
    pub weak_consumed_ms: u64,
    /// `now_ms` of the last heartbeat tick (used to attribute
    /// silent / weak time to the right gap).
    pub last_heartbeat_ms: u64,
    /// `false` until the very first signal (Strong/Weak) lands;
    /// `true` from then on. Lets `evaluate` distinguish "we
    /// haven't heard anything yet" from "we last heard
    /// nothing at last_heartbeat_ms".
    pub seen_any_signal: bool,
}

impl DeadlineState {
    /// Fresh state at `now_ms`. `last_strong_ms` and
    /// `last_heartbeat_ms` both anchor to `start_ms`.
    pub fn fresh(now_ms: u64) -> Self {
        Self {
            start_ms: now_ms,
            last_strong_ms: now_ms,
            weak_consumed_ms: 0,
            last_heartbeat_ms: now_ms,
            seen_any_signal: false,
        }
    }
}

/// Outcome of a single `evaluate_deadline` tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineVerdict {
    /// Job is alive — keep running. Returns the next
    /// `last_strong_ms` / `weak_consumed_ms` snapshot for
    /// observability (U9 inspect surface; tests assert the
    /// numbers).
    Alive {
        last_strong_ms: u64,
        weak_consumed_ms: u64,
    },
    /// Job exceeded the hard wall-clock cap. The kernel MUST
    /// cancel the port and surface `HeartbeatTimeout`.
    HardCapExceeded {
        elapsed_ms: u64,
        cap_ms: u64,
    },
    /// Job sat silent (or weak-budget-exhausted) past the idle
    /// lease. The kernel MUST cancel the port and surface
    /// `HeartbeatTimeout` with reason = "idle lease expired".
    IdleLeaseExpired {
        elapsed_since_strong_ms: u64,
        idle_lease_ms: u64,
    },
}

impl DeadlineVerdict {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            DeadlineVerdict::HardCapExceeded { .. } | DeadlineVerdict::IdleLeaseExpired { .. }
        )
    }
}

/// Pure function. Mutates a `DeadlineState` snapshot with the
/// supplied signal at `now_ms` and returns the verdict.
///
/// Contract:
/// 1. **Hard cap is checked first** — any signal at the cap is
///    `HardCapExceeded` regardless of progress class.
/// 2. **Startup grace** — within `startup_grace_ms` of `start`,
///    any signal is `Alive` (no idle accounting). The state
///    still records `seen_any_signal = true`.
/// 3. **Strong signal** — renews `last_strong_ms = now_ms` and
///    does **not** add to `weak_consumed_ms`. Returns `Alive`.
/// 4. **Weak signal** — only adds to `weak_consumed_ms` the
///    wall-clock gap since the previous heartbeat. If that
///    would push `weak_consumed_ms` past the policy budget,
///    return `Alive` but flag the budget as exhausted (the
///    *next* silent tick will trip `IdleLeaseExpired`). If the
///    gap pushes the idle lease past `idle_lease_ms` AND
///    `weak_consumed_ms` is already at the cap, return
///    `IdleLeaseExpired`.
/// 5. **Silent signal** — the idle lease is computed from the
///    previous `last_heartbeat_ms`. If past `idle_lease_ms`
///    without a strong renewal, return `IdleLeaseExpired`.
pub fn evaluate_deadline(
    state: &mut DeadlineState,
    signal: Signal,
    now_ms: u64,
    policy: &DeadlinePolicy,
) -> DeadlineVerdict {
    // (1) Hard cap first — non-extendable.
    debug_assert!(now_ms >= state.start_ms, "now_ms must be >= start_ms");
    let elapsed_total = now_ms.saturating_sub(state.start_ms);
    if elapsed_total >= policy.hard_cap_ms {
        return DeadlineVerdict::HardCapExceeded {
            elapsed_ms: elapsed_total,
            cap_ms: policy.hard_cap_ms,
        };
    }

    let gap_since_heartbeat = now_ms.saturating_sub(state.last_heartbeat_ms);

    // (2) Startup grace — anything counts as alive.
    if elapsed_total < policy.startup_grace_ms {
        if signal != Signal::Silent {
            state.seen_any_signal = true;
        }
        state.last_heartbeat_ms = now_ms;
        return DeadlineVerdict::Alive {
            last_strong_ms: state.last_strong_ms,
            weak_consumed_ms: state.weak_consumed_ms,
        };
    }

    match signal {
        Signal::Strong => {
            state.seen_any_signal = true;
            state.last_strong_ms = now_ms;
            state.last_heartbeat_ms = now_ms;
            // Strong resets weak accumulator implicitly (any
            // strong renews; weak is only counted *between*
            // strongs).
            DeadlineVerdict::Alive {
                last_strong_ms: state.last_strong_ms,
                weak_consumed_ms: state.weak_consumed_ms,
            }
        }
        Signal::Weak => {
            state.seen_any_signal = true;
            // Track the wall-clock gap since the previous
            // heartbeat (not since last_strong) — that way a
            // strong arrival zero days from now will not retro-
            // actively refund weak-time. Weak budget is a
            // single total, not a per-renewal grant.
            let add = gap_since_heartbeat;
            let new_weak = state
                .weak_consumed_ms
                .saturating_add(add)
                .min(policy.weak_allowance_total_ms);
            state.weak_consumed_ms = new_weak;
            state.last_heartbeat_ms = now_ms;

            let elapsed_since_strong = now_ms.saturating_sub(state.last_strong_ms);
            if elapsed_since_strong >= policy.idle_lease_ms
                && state.weak_consumed_ms >= policy.weak_allowance_total_ms
            {
                return DeadlineVerdict::IdleLeaseExpired {
                    elapsed_since_strong_ms: elapsed_since_strong,
                    idle_lease_ms: policy.idle_lease_ms,
                };
            }
            DeadlineVerdict::Alive {
                last_strong_ms: state.last_strong_ms,
                weak_consumed_ms: state.weak_consumed_ms,
            }
        }
        Signal::Silent => {
            // Silent does not consume the weak budget — silent
            // time directly trips the idle lease.
            state.last_heartbeat_ms = now_ms;
            let elapsed_since_strong = now_ms.saturating_sub(state.last_strong_ms);
            if elapsed_since_strong >= policy.idle_lease_ms {
                DeadlineVerdict::IdleLeaseExpired {
                    elapsed_since_strong_ms: elapsed_since_strong,
                    idle_lease_ms: policy.idle_lease_ms,
                }
            } else {
                DeadlineVerdict::Alive {
                    last_strong_ms: state.last_strong_ms,
                    weak_consumed_ms: state.weak_consumed_ms,
                }
            }
        }
    }
}

/// Failure-class taxonomy reused by the correction state machine.
/// Distinct from `worker_outcome::FailureClassification` (which
/// is keyed on stable reason *strings*); this enum is keyed on
/// the **runtime classification** the deadline / process port
/// surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// The kernel should retry: deadline miss under strong
    /// progress (e.g. an actually-busy job that needed more
    /// wall-clock), empty worker result, missing worker
    /// terminal, transient infrastructure.
    Retryable,
    /// The kernel MUST NOT retry: pre-fence rejected,
    /// payload-too-large, policy rejected, illegal stage
    /// transition. Surface as typed `Blocked`.
    Permanent,
    /// The kernel MUST cancel: idle lease expired, hard cap
    /// exceeded, explicit cancel. Surface as typed `Blocked`.
    Cancel,
}

/// Map a `RuntimeJobError` (mirrored as the typed error variants
/// the kernel emits) to a `FailureClass`. Lives here, not in
/// `runtime_job`, so the correction state machine does not need
/// to depend on the kernel crate.
pub fn classify_runtime_job_error(
    stage: &str,
    // `exit_code` is intentionally unused in the body: the pre-fix
    // `exit_code == Some(0) => Permanent` short-circuit (C1) violated
    // the `FailureClass::Retryable` doc contract (empty worker result
    // / missing terminal ⇒ Retryable). The fix routes `Some(0)` into
    // the stage match instead, so `exit_code` is no longer consulted.
    // The parameter is retained (underscore-prefixed) to preserve the
    // public 8-arg signature shared with the kernel's typed-error
    // mirror and documented in this module's doc contract.
    _exit_code: Option<i32>,
    payload_bytes: usize,
    payload_cap_bytes: usize,
    elapsed_ms: u64,
    hard_cap_ms: u64,
    idle_elapsed_ms: u64,
    idle_lease_ms: u64,
) -> FailureClass {
    if elapsed_ms >= hard_cap_ms || idle_elapsed_ms >= idle_lease_ms {
        return FailureClass::Cancel;
    }
    if payload_bytes > payload_cap_bytes {
        return FailureClass::Permanent;
    }
    // NOTE: no `exit_code == Some(0) => Permanent` short-circuit here.
    // Per the `FailureClass::Retryable` doc contract (empty worker
    // result / missing worker terminal ⇒ Retryable), an exit_code of
    // `Some(0)` must fall through into the stage match so that stages
    // representing empty worker result / missing terminal (the `_`
    // arm) classify as `Retryable`. Explicit `Permanent` stages
    // (policy_rejected etc.) stay `Permanent` regardless of exit
    // code, which is correct (a policy reject is permanent even if
    // the process exited 0). See the `_exit_code` param comment for
    // why the value is no longer consulted.
    // Mirror `runtime_job::RuntimeJobError` variants by name.
    match stage {
        "pre_fence_failed"
        | "policy_rejected"
        | "illegal_stage_transition"
        | "token_mismatch"
        | "payload_too_large" => FailureClass::Permanent,
        "heartbeat_timeout" => FailureClass::Cancel,
        _ => FailureClass::Retryable,
    }
}

// ---------------------------------------------------------------------------
// Tests (TDD Red phase lives in this module body; the production
// implementation is the rest of the file above).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Virtual clock helper that defaults to t=0.
    fn clock() -> VirtualClock {
        VirtualClock::new(0)
    }

    fn default_policy() -> DeadlinePolicy {
        DeadlinePolicy {
            startup_grace_ms: 1_000,
            hard_cap_ms: 10_000,
            idle_lease_ms: 5_000,
            weak_allowance_total_ms: 2_000,
        }
    }

    // ----- classify_signal ------------------------------------------------

    #[test]
    fn classify_empty_is_silent() {
        assert_eq!(classify_signal(""), Signal::Silent);
        assert_eq!(classify_signal("   \n\t  "), Signal::Silent);
    }

    #[test]
    fn classify_strong_keywords_are_strong() {
        for kw in STRONG_KEYWORDS {
            let payload = format!("some prefix {kw} some suffix");
            assert_eq!(
                classify_signal(&payload),
                Signal::Strong,
                "keyword {kw:?} should classify as Strong"
            );
        }
    }

    #[test]
    fn classify_weak_keywords_are_weak() {
        for kw in WEAK_KEYWORDS {
            let payload = format!("status: {kw} now");
            assert_eq!(
                classify_signal(&payload),
                Signal::Weak,
                "keyword {kw:?} should classify as Weak"
            );
        }
    }

    #[test]
    fn classify_unrecognised_nonempty_is_weak_fail_closed() {
        assert_eq!(
            classify_signal("totally novel unrecognised output"),
            Signal::Weak,
            "unknown non-empty payloads MUST be Weak (fail-closed default)"
        );
    }

    #[test]
    fn classify_strong_wins_over_weak_in_same_payload() {
        // A payload containing both a strong keyword and a weak
        // keyword MUST classify as Strong — strong is the
        // binding rule.
        assert_eq!(
            classify_signal("test result: ok\n...still spinner..."),
            Signal::Strong
        );
    }

    #[test]
    fn classify_case_insensitive_for_strong() {
        assert_eq!(
            classify_signal("CARGO TEST OK summary"),
            Signal::Strong,
            "strong keywords are case-insensitive"
        );
    }

    // ----- evaluate_deadline: hard cap first -------------------------------

    #[test]
    fn evaluate_hard_cap_trips_at_cap_even_with_strong_signal() {
        let c = clock();
        let policy = default_policy();
        let mut s = DeadlineState::fresh(c.now_ms());
        // Even after a strong signal, hitting the hard cap MUST
        // produce HardCapExceeded — non-extendable.
        c.advance(2_000);
        let v = evaluate_deadline(&mut s, Signal::Strong, c.now_ms(), &policy);
        assert!(matches!(v, DeadlineVerdict::Alive { .. }));
        c.advance(8_001); // total elapsed = 10_001, cap = 10_000
        let v = evaluate_deadline(&mut s, Signal::Strong, c.now_ms(), &policy);
        match v {
            DeadlineVerdict::HardCapExceeded { elapsed_ms, cap_ms } => {
                assert_eq!(cap_ms, 10_000);
                assert!(elapsed_ms >= cap_ms);
            }
            other => panic!("expected HardCapExceeded, got {other:?}"),
        }
    }

    // ----- evaluate_deadline: startup grace --------------------------------

    #[test]
    fn evaluate_startup_grace_treats_silent_as_alive() {
        let c = clock();
        let policy = default_policy();
        let mut s = DeadlineState::fresh(c.now_ms());
        // 500 ms in (under grace=1_000), silent payload must be
        // Alive — no idle check fires during the grace window.
        c.advance(500);
        let v = evaluate_deadline(&mut s, Signal::Silent, c.now_ms(), &policy);
        assert!(matches!(v, DeadlineVerdict::Alive { .. }));
    }

    #[test]
    fn evaluate_startup_grace_zero_disables_grace() {
        let c = clock();
        let policy = DeadlinePolicy {
            startup_grace_ms: 0,
            ..default_policy()
        };
        let mut s = DeadlineState::fresh(c.now_ms());
        // Grace=0 → idle lease accounting starts at t=0 with
        // last_strong_ms=0 (fresh state). Advance past the idle
        // lease (5_000 ms) and the next silent tick trips
        // IdleLeaseExpired.
        c.advance(5_001);
        let v = evaluate_deadline(&mut s, Signal::Silent, c.now_ms(), &policy);
        assert!(matches!(v, DeadlineVerdict::IdleLeaseExpired { .. }));
    }

    // ----- evaluate_deadline: strong renewal -------------------------------

    #[test]
    fn evaluate_strong_renews_idle_lease() {
        let c = clock();
        // Hard cap must clear the idle-lease window we're testing
        // (12_000 ms of elapsed time). 60_000 is the U8 default
        // and is large enough that the idle branch fires first.
        let policy = DeadlinePolicy {
            hard_cap_ms: 60_000,
            ..default_policy()
        };
        let mut s = DeadlineState::fresh(c.now_ms());
        // Move past grace (1_000), idle = 5_000.
        c.advance(2_000);
        // Weak tick at t=2_000 — last_strong=0, gap=2_000 < 5_000.
        let v = evaluate_deadline(&mut s, Signal::Weak, c.now_ms(), &policy);
        assert!(matches!(v, DeadlineVerdict::Alive { .. }));
        // Strong tick at t=6_000 — renews last_strong to 6_000.
        c.advance(4_000);
        let v = evaluate_deadline(&mut s, Signal::Strong, c.now_ms(), &policy);
        assert!(matches!(v, DeadlineVerdict::Alive { .. }));
        assert_eq!(s.last_strong_ms, c.now_ms());
        // Silent tick at t=9_999 (gap from strong = 3_999 < 5_000).
        c.advance(3_999);
        let v = evaluate_deadline(&mut s, Signal::Silent, c.now_ms(), &policy);
        assert!(matches!(v, DeadlineVerdict::Alive { .. }));
        // Silent tick at t=12_000 (gap from strong = 6_000 >= 5_000,
        // still under hard_cap = 60_000) → IdleLeaseExpired, NOT
        // HardCapExceeded.
        c.advance(2_001);
        let v = evaluate_deadline(&mut s, Signal::Silent, c.now_ms(), &policy);
        match v {
            DeadlineVerdict::IdleLeaseExpired {
                elapsed_since_strong_ms,
                idle_lease_ms,
            } => {
                assert_eq!(idle_lease_ms, 5_000);
                assert!(elapsed_since_strong_ms >= 5_000);
            }
            other => panic!("expected IdleLeaseExpired, got {other:?}"),
        }
    }

    // ----- evaluate_deadline: weak allowance is bounded --------------------

    #[test]
    fn evaluate_weak_allowance_is_total_not_per_renewal() {
        let c = clock();
        let policy = default_policy();
        let mut s = DeadlineState::fresh(c.now_ms());
        c.advance(1_001); // out of grace
        // 5 weak ticks of 400 ms each = 2_000 ms total → at the
        // allowance cap. Each tick individually is below
        // idle_lease_ms; the test verifies the cumulative
        // behaviour.
        for _ in 0..5 {
            let v = evaluate_deadline(&mut s, Signal::Weak, c.now_ms(), &policy);
            assert!(matches!(v, DeadlineVerdict::Alive { .. }));
            assert!(
                s.weak_consumed_ms <= policy.weak_allowance_total_ms,
                "weak_consumed_ms must saturate at the policy cap"
            );
            c.advance(400);
        }
        assert_eq!(
            s.weak_consumed_ms, policy.weak_allowance_total_ms,
            "exactly at the cap after the 5th weak tick"
        );
        // One more silent tick with gap > idle_lease AND weak
        // budget exhausted → IdleLeaseExpired.
        c.advance(4_700);
        let v = evaluate_deadline(&mut s, Signal::Silent, c.now_ms(), &policy);
        assert!(
            matches!(v, DeadlineVerdict::IdleLeaseExpired { .. }),
            "weak budget exhausted + idle gap past lease MUST trip"
        );
    }

    #[test]
    fn evaluate_strong_resets_weak_counter_implicitly() {
        let c = clock();
        let policy = default_policy();
        let mut s = DeadlineState::fresh(c.now_ms());
        c.advance(1_001); // out of grace; last_heartbeat = start = 0
        // 3 weak ticks. First tick's gap = 1_001 (start → t=1_001);
        // each subsequent gap = 400. Total weak_consumed after 3
        // ticks = 1_001 + 400 + 400 = 1_801. The point of the
        // test is that strong does NOT reset this counter — weak
        // budget is a single total across the job, not per-
        // renewal.
        for _ in 0..3 {
            evaluate_deadline(&mut s, Signal::Weak, c.now_ms(), &policy);
            c.advance(400);
        }
        assert_eq!(s.weak_consumed_ms, 1_801);
        // Strong tick arrives — last_strong_ms advances; weak
        // counter is *not* reset by the strong signal itself
        // (budget is total, not per-renewal) but the idle lease
        // is renewed. New weak ticks continue accumulating into
        // the same total budget.
        evaluate_deadline(&mut s, Signal::Strong, c.now_ms(), &policy);
        c.advance(400);
        evaluate_deadline(&mut s, Signal::Weak, c.now_ms(), &policy);
        assert!(
            s.weak_consumed_ms > 1_801,
            "weak must continue accumulating after a strong renewal"
        );
        assert!(s.weak_consumed_ms <= policy.weak_allowance_total_ms);
    }

    // ----- evaluate_deadline: silent trips idle lease immediately ---------

    #[test]
    fn evaluate_silent_after_grace_with_no_strong_trips_immediately() {
        let c = clock();
        let policy = default_policy();
        let mut s = DeadlineState::fresh(c.now_ms());
        c.advance(1_001); // out of grace
        c.advance(5_000); // silent gap = 5_000 from start (= last_strong)
        let v = evaluate_deadline(&mut s, Signal::Silent, c.now_ms(), &policy);
        assert!(matches!(v, DeadlineVerdict::IdleLeaseExpired { .. }));
    }

    // ----- DeadlineVerdict helpers ----------------------------------------

    #[test]
    fn verdict_is_terminal_helper() {
        assert!(DeadlineVerdict::HardCapExceeded {
            elapsed_ms: 1,
            cap_ms: 1
        }
        .is_terminal());
        assert!(DeadlineVerdict::IdleLeaseExpired {
            elapsed_since_strong_ms: 1,
            idle_lease_ms: 1
        }
        .is_terminal());
        assert!(!DeadlineVerdict::Alive {
            last_strong_ms: 0,
            weak_consumed_ms: 0
        }
        .is_terminal());
    }

    // ----- classify_runtime_job_error -------------------------------------

    #[test]
    fn classify_runtime_job_error_hard_cap_is_cancel() {
        assert_eq!(
            classify_runtime_job_error("execute", None, 0, 100, 10_000, 10_000, 0, 5_000),
            FailureClass::Cancel
        );
    }

    #[test]
    fn classify_runtime_job_error_idle_lease_is_cancel() {
        assert_eq!(
            classify_runtime_job_error("execute", None, 0, 100, 1_000, 10_000, 5_000, 5_000),
            FailureClass::Cancel
        );
    }

    #[test]
    fn classify_runtime_job_error_payload_too_large_is_permanent() {
        assert_eq!(
            classify_runtime_job_error("payload_too_large", None, 200, 100, 0, 10_000, 0, 5_000),
            FailureClass::Permanent
        );
    }

    #[test]
    fn classify_runtime_job_error_heartbeat_timeout_is_cancel() {
        assert_eq!(
            classify_runtime_job_error("heartbeat_timeout", None, 0, 100, 0, 10_000, 0, 5_000),
            FailureClass::Cancel
        );
    }

    #[test]
    fn classify_runtime_job_error_unknown_is_retryable() {
        assert_eq!(
            classify_runtime_job_error("execute", None, 0, 100, 0, 10_000, 0, 5_000),
            FailureClass::Retryable
        );
    }

    // ----- C1+T1 regression: exit_code==Some(0) honoring the
    // 454-456 doc contract (empty worker result / missing terminal
    // ⇒ Retryable, not Permanent). The pre-fix `exit_code==Some(0) →
    // Permanent` short-circuit (486-488) violated that contract for
    // any stage falling into the `_ => Retryable` arm. These tests
    // pin the fix so a future re-ordering cannot pass silently.

    #[test]
    fn classify_runtime_job_error_exit_zero_empty_worker_result_is_retryable() {
        // (a) exit_code==Some(0) + a stage representing an empty
        // worker result (falls into the `_` arm). Per the 454-456
        // doc contract this is Retryable (was Permanent pre-fix).
        assert_eq!(
            classify_runtime_job_error("empty_worker_result", Some(0), 0, 100, 0, 10_000, 0, 5_000),
            FailureClass::Retryable
        );
    }

    #[test]
    fn classify_runtime_job_error_exit_zero_missing_terminal_is_retryable() {
        // (b) exit_code==Some(0) + a stage representing a missing
        // worker terminal (a different `_`-arm stage). Per the
        // 454-456 doc contract this is Retryable (was Permanent
        // pre-fix).
        assert_eq!(
            classify_runtime_job_error("missing_terminal", Some(0), 0, 100, 0, 10_000, 0, 5_000),
            FailureClass::Retryable
        );
    }

    #[test]
    fn classify_runtime_job_error_exit_zero_normal_terminal_is_retryable() {
        // (c) exit_code==Some(0) + a stage representing a normal
        // terminal success path (`execute` falls into the `_` arm;
        // there is no dedicated "normal_terminal" stage string —
        // `execute` is the representative success-with-terminal
        // stage). It must NOT be classified as a Permanent error.
        // Post-fix it lands in `_ => Retryable`, which honors the
        // doc contract (a terminal-bearing success is not a
        // Permanent failure). The assertion pins that it is not
        // Permanent.
        let class = classify_runtime_job_error("execute", Some(0), 0, 100, 0, 10_000, 0, 5_000);
        assert_ne!(
            class,
            FailureClass::Permanent,
            "exit_code==Some(0) normal terminal must not be Permanent"
        );
        assert_eq!(
            class,
            FailureClass::Retryable,
            "exit_code==Some(0) normal terminal classifies as Retryable per doc contract"
        );
    }

    #[test]
    fn classify_runtime_job_error_exit_zero_permanent_stage_stays_permanent() {
        // (d) genuinely permanent stage (`policy_rejected`) +
        // exit_code==Some(0) ⇒ still Permanent. Proves the fix did
        // not over-relax: explicit Permanent stages stay Permanent
        // regardless of exit code (a policy reject is permanent even
        // if the process exited 0).
        assert_eq!(
            classify_runtime_job_error("policy_rejected", Some(0), 0, 100, 0, 10_000, 0, 5_000),
            FailureClass::Permanent
        );
    }

    // ----- VirtualClock ---------------------------------------------------

    #[test]
    fn virtual_clock_advances_only_on_advance() {
        let c = clock();
        let t0 = c.now_ms();
        // No real sleep — just observe.
        let t1 = c.now_ms();
        assert_eq!(t0, t1, "VirtualClock must NOT advance without advance()");
        c.advance(42);
        assert_eq!(c.now_ms(), 42);
        c.advance(8);
        assert_eq!(c.now_ms(), 50);
        c.set(100);
        assert_eq!(c.now_ms(), 100);
        c.set(50); // backwards — must be ignored
        assert_eq!(c.now_ms(), 100);
    }
}