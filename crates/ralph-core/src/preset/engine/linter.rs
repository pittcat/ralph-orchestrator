//! `lint_emit` — precheck-as-linter.
//!
//! Plan ref: R8, R10–R11, R13–R15 (plan 2026-06-20-001).
//!
//! The linter is the front gate for every `ralph emit`. It uses
//! the same [`run_gates`] function as the runtime loop
//! (KTD-8/R15), so an event the linter rejects will also be
//! rejected by the runtime gate — fail-closed twice.
//!
//! ## 2026-06-23-006 U5: hat_handoff removal
//!
//! `auto_handoff_prepare` and the `RALPH_HAT_HANDOFF_SEQ` env
//! read were removed together with the `hat_handoff` config
//! block. The macro-edge auto-prepare path was the only
//! consumer of `LintPaths::output_dir`; the `LintPaths` struct
//! is retained for future lint-side artifacts and is currently
//! `workspace_root` only.
//!
//! ## Thread concurrency (P1-2 fix)
//!
//! `lint_emit_with_timeout` runs on a dedicated OS thread per
//! call. Without a cap, an agent emit storm could pile up
//! hundreds of leaked threads (Rust stable cannot kill them on
//! timeout) and trip `RLIMIT_NTHR`. The linter bounds the
//! number of in-flight threads with a counting semaphore; when
//! the cap is reached, the call returns `Timeout` immediately
//! without spawning a new thread.

use std::path::PathBuf;

use serde_json::Value;

use super::gates::{GateDecision, LintContext, run_gates};
use super::hint::LintResumeHint;
use super::protocol::ProtocolView;

/// Filesystem paths used by the linter. Populated by the CLI
/// emit caller (which knows the loop workspace) or by tests
/// (which point at a tempdir).
///
/// 2026-06-23-006 U5: `output_dir` was previously the
/// handoff-artifact directory used by the now-removed
/// `auto_handoff_prepare`. The field is kept for future
/// lint-side artifacts; callers that only need `workspace_root`
/// can ignore it.
#[derive(Debug, Clone)]
pub struct LintPaths {
    pub workspace_root: PathBuf,
    pub output_dir: PathBuf,
}

impl LintPaths {
    /// Convenience constructor. With `hat_handoff` removed
    /// the `output_dir` defaults to the conventional
    /// `.ralph/agent/hat-handoff/` path so existing callers
    /// keep their previous behaviour; future lint artifacts
    /// may reuse or override the field.
    pub fn under_handoff_dir(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            output_dir: PathBuf::from(".ralph/agent/hat-handoff"),
        }
    }
}

/// F-PS-006 test-only knob: when set to a positive number of
/// microseconds, `lint_emit` sleeps for that duration at entry.
/// Used by the bounded-wait tests in `linter::tests` to make
/// the lint path deterministic-slow so the timeout can fire
/// without flaky timing.
///
/// In production (`#[cfg(not(test))]`) this is a no-op `static`
/// — the `lint_emit` function does not reference it.
#[cfg(test)]
pub static TEST_LINT_SLEEP_MICROS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// P1-2: maximum number of lint threads that may run
/// concurrently. Each call to `lint_emit_with_timeout` acquires
/// a permit from this semaphore; when the budget is exceeded
/// the caller's `recv_timeout` returns `Timeout` **immediately**
/// (bounded-wait fail-closed, F-PS-006) and the worker thread
/// keeps running in the background until it finishes
/// naturally. Without a cap, an agent emit storm (d623c09
/// primary-20260619) can pile up hundreds of leaked threads and
/// trip `RLIMIT_NTHR` / file-descriptor exhaustion; the
/// `.expect("failed to spawn lint thread")` then panics the
/// whole loop.
///
/// `num_cpus * 2` is a deliberately conservative upper bound —
/// the linter is CPU-light (gate + macro check + optional
/// `auto_handoff_prepare` write); more than `num_cpus * 2`
/// concurrent workers means we are overwhelming something
/// downstream of the linter (disk, network) and we want the
/// pressure to back-propagate as a `Timeout` rather than as
/// an OOM.
fn max_concurrent_lint_threads() -> i64 {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    (n.saturating_mul(2).max(4)) as i64
}

/// P1-2: lightweight counting semaphore built on `AtomicI64`.
/// The standard library does not (yet) expose `std::sync::Semaphore`
/// on stable, and pulling in `parking_lot` for a single counter
/// would be a heavyweight dep. The implementation is intentionally
/// minimal:
///
/// * `try_acquire` decrements the counter; if it was 0 the
///   decrement is rolled back and the call returns `None`.
///   The `compare_exchange` loop guarantees the decrement is
///   atomic with the capacity check — no torn reads even
///   under concurrent callers.
/// * `release` increments the counter (saturating at the
///   initial cap so a stray `release` cannot make the counter
///   exceed the cap).
///
/// We do **not** implement a blocking `acquire` — the linter
/// always uses the non-blocking variant and fails-closed when
/// saturated. Adding a blocking acquire would re-introduce the
/// resource-exhaustion risk this semaphore is meant to bound.
pub struct LintThreadSemaphore {
    counter: std::sync::atomic::AtomicI64,
    init_cap: i64,
}

impl LintThreadSemaphore {
    fn new(cap: i64) -> Self {
        Self {
            counter: std::sync::atomic::AtomicI64::new(cap),
            init_cap: cap,
        }
    }

    fn try_acquire(&self) -> Option<LintPermit<'_>> {
        use std::sync::atomic::Ordering;
        loop {
            let current = self.counter.load(Ordering::Acquire);
            if current <= 0 {
                return None;
            }
            match self.counter.compare_exchange(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(LintPermit { sem: self }),
                Err(_) => continue,
            }
        }
    }

    fn release(&self) {
        use std::sync::atomic::Ordering;
        loop {
            let current = self.counter.load(Ordering::Acquire);
            if current >= self.init_cap {
                return;
            }
            match self.counter.compare_exchange(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(_) => continue,
            }
        }
    }
}

/// RAII permit returned by `LintThreadSemaphore::try_acquire`.
/// The permit's `Drop` impl releases the slot, so workers
/// never leak permits even when they panic.
struct LintPermit<'a> {
    sem: &'a LintThreadSemaphore,
}

impl Drop for LintPermit<'_> {
    fn drop(&mut self) {
        self.sem.release();
    }
}

/// Global semaphore that bounds the number of in-flight lint
/// threads (P1-2). Acquired in
/// `lint_emit_with_budget`'s worker-spawn path; released when
/// the worker finishes (including after a timeout-driven
/// `recv_timeout` from the caller — the semaphore permit
/// is moved into the worker so the release happens on the
/// worker's natural completion, not the caller's).
static LINT_THREAD_SEMAPHORE: std::sync::LazyLock<LintThreadSemaphore> =
    std::sync::LazyLock::new(|| LintThreadSemaphore::new(max_concurrent_lint_threads()));

#[cfg(test)]
pub static TEST_LINT_SEMAPHORE_OVERRIDE: std::sync::Mutex<Option<&'static LintThreadSemaphore>> =
    std::sync::Mutex::new(None);

fn lint_thread_semaphore() -> &'static LintThreadSemaphore {
    #[cfg(test)]
    {
        if let Some(sem) = *TEST_LINT_SEMAPHORE_OVERRIDE.lock().unwrap() {
            return sem;
        }
    }
    &LINT_THREAD_SEMAPHORE
}

/// Outcome of a single lint pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LintOutcome {
    /// Lint accepted the event; runtime gate will see the same
    /// view (R15).
    Accept,
    /// Lint rejected the event with a resume hint.
    Reject(LintResumeHint),
    /// Lint timed out (R14, KTD-9). Fail-closed.
    Timeout(String),
}

/// Run lint on a single emit. Reads protocol from `view` only —
/// the runtime gate is consulted separately on the inbound path.
///
/// 2026-06-23-006 U5: removed the `hat_handoff` macro-edge
/// auto-prepare path (R22); the linter now just runs the
/// unified `run_gates` and translates its decision to a
/// `LintOutcome`. `paths` is kept for the future lint-side
/// artifact use case but is currently unused.
pub fn lint_emit(
    view: &ProtocolView,
    _paths: &LintPaths,
    topic: &str,
    payload: &mut Value,
) -> LintOutcome {
    // F-PS-006 test-only sleep hook.
    #[cfg(test)]
    {
        let micros = TEST_LINT_SLEEP_MICROS.load(std::sync::atomic::Ordering::Relaxed);
        if micros > 0 {
            std::thread::sleep(std::time::Duration::from_micros(micros));
        }
    }

    match run_gates(view, &LintContext, topic, payload, None) {
        GateDecision::Accept => LintOutcome::Accept,
        // P1-1: classify by the typed `kind`, not by string
        // substring matching on the message.
        GateDecision::Reject { kind, message } => {
            LintOutcome::Reject(LintResumeHint::from_typed_rejection(topic, kind, &message))
        }
    }
}

/// Lint duration budget (R14 / KTD-9). p95 < 200ms.
pub const LINT_BUDGET: std::time::Duration = std::time::Duration::from_millis(200);

/// Run lint on a single emit, time-budgeted (R14 / KTD-9).
pub fn lint_emit_with_timeout(
    view: &ProtocolView,
    paths: &LintPaths,
    topic: &str,
    payload: &mut Value,
) -> LintOutcome {
    lint_emit_with_budget(view, paths, topic, payload, LINT_BUDGET)
}

fn lint_emit_with_budget(
    view: &ProtocolView,
    paths: &LintPaths,
    topic: &str,
    payload: &mut Value,
    budget: std::time::Duration,
) -> LintOutcome {
    let input = payload.clone();
    let view_owned = view.clone();
    let paths_owned = paths.clone();
    let topic_owned = topic.to_string();

    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(LintOutcome, Value), String>>(1);

    // P1-2: acquire a permit from the global semaphore before
    // spawning. If all permits are taken, we fail-closed with
    // a Timeout **without** spawning a new thread. The permit
    // is moved into the worker closure so it is released when
    // `lint_emit` returns (whether due to natural completion,
    // panic, or caller timeout).
    let sem = lint_thread_semaphore();
    let permit = match sem.try_acquire() {
        Some(permit) => permit,
        None => {
            tracing::warn!(
                topic = %topic_owned,
                max = max_concurrent_lint_threads(),
                "lint thread pool saturated; refusing to spawn (P1-2)"
            );
            return LintOutcome::Timeout(format!(
                "lint thread pool saturated (max {} concurrent); refusing to spawn for topic `{topic}`",
                max_concurrent_lint_threads()
            ));
        }
    };

    let _join = std::thread::Builder::new()
        .name(format!("ralph-lint-{topic}"))
        .spawn(move || {
            let _permit = permit;
            let mut owned = input;
            let outcome = lint_emit(&view_owned, &paths_owned, &topic_owned, &mut owned);
            let _ = tx.send(Ok((outcome, owned)));
        })
        .expect("failed to spawn lint thread (OS resource exhaustion?)");

    match rx.recv_timeout(budget) {
        Ok(Ok((outcome, mut owned))) => {
            *payload = std::mem::take(&mut owned);
            outcome
        }
        Ok(Err(err)) => LintOutcome::Timeout(err),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => LintOutcome::Timeout(format!(
            "lint exceeded {} ms budget for topic `{topic}`",
            budget.as_millis()
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            LintOutcome::Timeout("lint thread disconnected before sending".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EventLoopConfig;
    use std::sync::atomic::Ordering;

    /// A `Drop` guard that resets the test sleep hook to 0.
    /// Wrap the body of each test in a `BudgetGuard` so a
    /// panic mid-test cannot leak a non-zero sleep value to
    /// the next test.
    struct BudgetGuard;
    impl Drop for BudgetGuard {
        fn drop(&mut self) {
            TEST_LINT_SLEEP_MICROS.store(0, Ordering::Relaxed);
        }
    }

    fn empty_view() -> ProtocolView {
        let cfg = EventLoopConfig::default();
        ProtocolView::from_event_loop(&cfg)
    }

    /// All test fixtures use a tempdir-backed `LintPaths` so
    /// the bounded-wait / round-trip tests are isolated from
    /// the real filesystem.
    fn test_paths() -> LintPaths {
        let temp = tempfile::tempdir().expect("tempdir");
        LintPaths::under_handoff_dir(temp.path().to_path_buf())
    }

    #[test]
    fn test_bounded_wait_fast_path_returns_outcome() {
        let _guard = BudgetGuard;
        TEST_LINT_SLEEP_MICROS.store(0, Ordering::Relaxed);
        let view = empty_view();
        let paths = test_paths();
        let topic = "work.done";
        let mut payload = serde_json::json!({"plan_name": "p", "task_id": "t"});
        let payload_before = payload.clone();
        let outcome = lint_emit_with_budget(
            &view,
            &paths,
            topic,
            &mut payload,
            std::time::Duration::from_millis(50),
        );
        assert!(matches!(outcome, LintOutcome::Accept));
        assert_eq!(payload, payload_before);
    }

    #[test]
    fn test_bounded_wait_slow_path_returns_timeout() {
        let _guard = BudgetGuard;
        TEST_LINT_SLEEP_MICROS.store(200_000, Ordering::Relaxed);
        let view = empty_view();
        let paths = test_paths();
        let topic = "slow.topic";
        let mut payload = serde_json::json!({"k": "v"});
        let payload_before = payload.clone();
        let start = std::time::Instant::now();
        let outcome = lint_emit_with_budget(
            &view,
            &paths,
            topic,
            &mut payload,
            std::time::Duration::from_millis(50),
        );
        let elapsed = start.elapsed();
        assert!(elapsed < std::time::Duration::from_millis(150));
        match outcome {
            LintOutcome::Timeout(reason) => assert!(reason.contains("budget")),
            other => panic!("expected Timeout, got: {other:?}"),
        }
        assert_eq!(payload, payload_before);
    }

    #[test]
    fn test_bounded_wait_worker_panic_isolated_as_timeout() {
        let _guard = BudgetGuard;
        let (tx, rx) = std::sync::mpsc::sync_channel::<std::result::Result<(), ()>>(1);
        let _join = std::thread::Builder::new()
            .name("ralph-lint-panic-test".to_string())
            .spawn(move || {
                let _ = tx.send(Err(()));
                panic!("simulated lint panic");
            })
            .expect("spawn");
        let _ = rx.recv_timeout(std::time::Duration::from_millis(100));
        let (tx2, rx2) = std::sync::mpsc::sync_channel::<()>(1);
        drop(tx2);
        match rx2.recv_timeout(std::time::Duration::from_millis(50)) {
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
            other => panic!("expected Disconnected recv error, got: {other:?}"),
        }
    }

    #[test]
    fn test_bounded_wait_payload_round_trip() {
        let _guard = BudgetGuard;
        TEST_LINT_SLEEP_MICROS.store(10_000, Ordering::Relaxed);
        let view = empty_view();
        let paths = test_paths();
        let topic = "round.trip";
        let mut payload = serde_json::json!({"marker": "before"});
        let outcome = lint_emit_with_budget(
            &view,
            &paths,
            topic,
            &mut payload,
            std::time::Duration::from_millis(200),
        );
        assert!(matches!(outcome, LintOutcome::Accept));
        assert_eq!(payload, serde_json::json!({"marker": "before"}));
    }

    #[test]
    fn test_bounded_wait_subsequent_calls_independent() {
        let _guard = BudgetGuard;
        TEST_LINT_SLEEP_MICROS.store(300_000, Ordering::Relaxed);
        let view = empty_view();
        let paths = test_paths();
        let mut payload1 = serde_json::json!({"call": 1});
        let outcome1 = lint_emit_with_budget(
            &view,
            &paths,
            "first.topic",
            &mut payload1,
            std::time::Duration::from_millis(50),
        );
        assert!(matches!(outcome1, LintOutcome::Timeout(_)));
        let mut payload2 = serde_json::json!({"call": 2});
        let start = std::time::Instant::now();
        let outcome2 = lint_emit_with_budget(
            &view,
            &paths,
            "second.topic",
            &mut payload2,
            std::time::Duration::from_millis(50),
        );
        let elapsed = start.elapsed();
        assert!(matches!(outcome2, LintOutcome::Timeout(_)));
        assert!(elapsed < std::time::Duration::from_millis(150));
    }

    /// P1-2: when the global semaphore is saturated, the call
    /// returns `Timeout` immediately without spawning a thread.
    /// Uses a 1-permit override so a single in-flight call
    /// saturates the pool; the second call must return
    /// Timeout immediately (the 500 ms budget never elapses
    /// because no thread is spawned).
    #[test]
    fn p1_2_semaphore_saturation_returns_timeout_without_spawn() {
        let sem = Box::leak(Box::new(LintThreadSemaphore::new(1)));
        {
            let mut slot = TEST_LINT_SEMAPHORE_OVERRIDE.lock().unwrap();
            *slot = Some(sem);
        }

        let _guard = BudgetGuard;
        // Long sleep so the first call's worker holds the
        // permit for well past the 50 ms budget (we expect a
        // timeout in the first call, NOT a fast return).
        TEST_LINT_SLEEP_MICROS.store(200_000, Ordering::Relaxed);

        let view = empty_view();
        let paths = test_paths();

        // First call: budget 50 ms, lint 200 ms → Timeout.
        // The worker thread is still running and holds the
        // permit; the caller's Timeout does NOT release the
        // permit (P1-2 contract: the permit is bound to the
        // worker's lifetime).
        let mut payload1 = serde_json::json!({"k": "v1"});
        let outcome1 = lint_emit_with_budget(
            &view,
            &paths,
            "first.topic",
            &mut payload1,
            std::time::Duration::from_millis(50),
        );
        assert!(
            matches!(outcome1, LintOutcome::Timeout(_)),
            "first call should time out, got: {outcome1:?}"
        );

        // Second call: the permit is still held by the first
        // call's worker; this call must return Timeout
        // *immediately* (no new thread spawned).
        let mut payload2 = serde_json::json!({"k": "v2"});
        let start = std::time::Instant::now();
        let outcome2 = lint_emit_with_budget(
            &view,
            &paths,
            "second.topic",
            &mut payload2,
            std::time::Duration::from_millis(500),
        );
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "saturated call violated bounded-wait: elapsed {elapsed:?}"
        );
        match outcome2 {
            LintOutcome::Timeout(reason) => {
                assert!(reason.contains("saturated") || reason.contains("max"));
            }
            other => panic!("expected Timeout on saturation, got: {other:?}"),
        }

        // Restore the default semaphore for subsequent tests.
        {
            let mut slot = TEST_LINT_SEMAPHORE_OVERRIDE.lock().unwrap();
            *slot = None;
        }
    }
}
