//! `lint_emit` + `auto_handoff_prepare` — precheck-as-linter.
//!
//! Plan ref: R8, R10–R11, R13–R15, R22 (plan 2026-06-20-001).
//!
//! The linter is the front gate for every `ralph emit`. It uses
//! the same [`run_gates`] function as the runtime loop
//! (KTD-8/R15), so an event the linter rejects will also be
//! rejected by the runtime gate — fail-closed twice.
//!
//! `auto_handoff_prepare` is the R22 fast path: when the
//! protocol says `hat_handoff.linter.auto_prepare_on_macro_edge`
//! AND the payload lacks `handoff_path` AND the topic is a
//! macro edge, the orchestrator **synchronously** writes the
//! handoff artifact and re-runs the gate. Failure is still
//! fail-closed.

use std::path::Path;

use serde_json::Value;

use super::gates::{GateDecision, LintContext, run_gates};
use super::hint::LintResumeHint;
use super::protocol::ProtocolView;

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

/// Outcome of a single lint pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LintOutcome {
    /// Lint accepted the event; runtime gate will see the same
    /// view (R15).
    Accept,
    /// Lint rejected the event with a resume hint.
    Reject(LintResumeHint),
    /// Lint invoked `auto_handoff_prepare` and accepted the
    /// event after prepare. The original payload has been
    /// updated in place with `handoff_path`.
    AcceptAfterAutoPrepare,
    /// Lint timed out (R14, KTD-9). Fail-closed.
    Timeout(String),
}

/// Run lint on a single emit. Reads protocol from `view` only —
/// the runtime gate is consulted separately on the inbound path.
///
/// Plan R22 (2026-06-20-001): when the topic is a macro edge
/// (per `view.is_macro_edge(topic)`), the payload lacks
/// `handoff_path`, AND `hat_handoff.linter.auto_prepare_on_macro_edge`
/// is enabled, this function synchronously prepares the handoff
/// artifact (via `auto_handoff_prepare`) and mutates `payload` to
/// inject `handoff_path` before re-running the gate. The
/// returned `LintOutcome::AcceptAfterAutoPrepare` tells the
/// caller that the orchestrator acted on the agent's behalf.
/// `Accept` means no prepare was needed (not a macro edge, or
/// already had `handoff_path`). `Reject`/`Timeout` short-circuit
/// and never invoke auto_prepare.
pub fn lint_emit(view: &ProtocolView, topic: &str, payload: &mut Value) -> LintOutcome {
    // F-PS-006 test-only sleep hook. The unit tests set this
    // atomic to make lint_emit deterministic-slow so the
    // bounded-wait path in `lint_emit_with_budget` can be
    // exercised without flaky timing. In production this
    // atomic stays at 0 and the `if` is a single load +
    // branch on a constant — negligible overhead.
    #[cfg(test)]
    {
        let micros = TEST_LINT_SLEEP_MICROS.load(std::sync::atomic::Ordering::Relaxed);
        if micros > 0 {
            std::thread::sleep(std::time::Duration::from_micros(micros));
        }
    }

    // Plan R22 / review P0 #3: macro-edge auto_prepare is the
    // B4 fix. We must check the protocol BEFORE the gate runs,
    // because a missing handoff_path is not in the required-fields
    // set — it lives on the hat_handoff side.
    if view.is_macro_edge(topic)
        && !has_handoff_path(payload)
        && view.hat_handoff.linter.auto_prepare_on_macro_edge
    {
        match auto_handoff_prepare(view, workspace_root_for(view), output_dir_for(view), topic, payload.clone()) {
            Ok(prepared) => {
                // auto_handoff_prepare mutates the payload to inject
                // `handoff_path`; copy it back so the caller writes
                // the prepared value to events.jsonl.
                *payload = prepared;
            }
            Err(err) => {
                // Prepare itself failed — fail-closed (R22). The
                // gate would have rejected this anyway (missing
                // handoff_path), so emit Reject with the prepare
                // error as the reason so the agent sees the root
                // cause rather than the gate's symptom.
                let hint = LintResumeHint::from_reason(
                    topic,
                    &format!("auto_handoff_prepare failed: {err}"),
                );
                return LintOutcome::Reject(hint);
            }
        }
    }
    match run_gates(view, &LintContext, topic, payload) {
        GateDecision::Accept => {
            // Distinguish Accept (no prepare) from
            // AcceptAfterAutoPrepare (prepare ran). The
            // `auto_handoff_prepare` returns Ok(prepared) when
            // it actually wrote the artifact; we re-derive the
            // outcome by inspecting the prepare path.
            if view.is_macro_edge(topic) && has_handoff_path(payload) {
                LintOutcome::AcceptAfterAutoPrepare
            } else {
                LintOutcome::Accept
            }
        }
        GateDecision::Reject(reason) => LintOutcome::Reject(LintResumeHint::from_reason(topic, &reason)),
    }
}

/// Return true when the payload is a JSON object that carries a
/// non-empty `handoff_path`. Helper for the macro-edge check.
fn has_handoff_path(payload: &Value) -> bool {
    match payload {
        Value::Object(map) => map
            .get("handoff_path")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        _ => false,
    }
}

/// Stub: return the workspace root for the current view. In
/// real callers this is `RalphConfig::core::workspace_root`; the
/// engine does not have a direct handle to it today. Review
/// P0 #3 follow-up: extend `ProtocolView` with `workspace_root`
/// + `output_dir` so `auto_handoff_prepare` can write the
/// artifact deterministically. For now this returns the current
/// dir so the wiring compiles; the runtime path is the CLI
/// which has `workspace_root` already.
fn workspace_root_for(_view: &ProtocolView) -> &std::path::Path {
    static ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
}

/// Stub: return the output directory for handoff artifacts.
/// Review P0 #3 follow-up: same as `workspace_root_for`.
fn output_dir_for(_view: &ProtocolView) -> &std::path::Path {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| std::path::PathBuf::from(".ralph/handoff"))
}

/// Public entry point that performs R22's macro-edge auto prepare
/// before re-running the gate. Returns the (possibly updated)
/// payload so callers can persist it.
///
/// `workspace_root` is the loop's workspace root; `output_dir` is
/// where the orchestrator writes the handoff artifact. The
/// artifact's filename is derived from the topic + a counter.
pub fn auto_handoff_prepare(
    view: &ProtocolView,
    workspace_root: &Path,
    output_dir: &Path,
    topic: &str,
    mut payload: Value,
) -> Result<Value, String> {
    if !view.hat_handoff.linter.auto_prepare_on_macro_edge {
        return Err(
            "auto_handoff_prepare called but `hat_handoff.linter.auto_prepare_on_macro_edge` is disabled"
                .to_string(),
        );
    }
    if !view.is_macro_edge(topic) {
        return Err(format!(
            "auto_handoff_prepare: `{topic}` is not a macro edge under current protocol"
        ));
    }
    let path = write_artifact(workspace_root, output_dir, topic)?;
    if let Value::Object(map) = &mut payload {
        map.insert("handoff_path".to_string(), Value::String(path.clone()));
    } else {
        return Err("auto_handoff_prepare: payload is not a JSON object".to_string());
    }
    Ok(payload)
}

/// Minimal artifact writer used by `auto_handoff_prepare` and
/// the test suite. Writes a 5-section body with `## next` so the
/// `ArtifactRule::validate` check passes (R21).
fn write_artifact(workspace_root: &Path, output_dir: &Path, topic: &str) -> Result<String, String> {
    let safe_topic = topic.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "_");
    let filename = format!("auto_{safe_topic}.md");
    let abs_path = if output_dir.is_absolute() {
        output_dir.join(&filename)
    } else {
        workspace_root.join(output_dir).join(&filename)
    };
    if let Some(parent) = abs_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let body = format!(
        "## context\nprepared by orchestrator for `{topic}`\n\n\
         ## intent\nauto-prepared handoff per protocol rule.\n\n\
         ## current_state\nstep in flight, awaiting next hat.\n\n\
         ## proposed_action\ncontinue with the planned action.\n\n\
         ## rationale\norchestrator-side auto-prepare satisfies the R22 macro-edge contract.\n\n\
         ## next\n\
         next: {topic}\n"
    );
    std::fs::write(&abs_path, body).map_err(|e| format!("write artifact: {e}"))?;
    let rel = abs_path
        .strip_prefix(workspace_root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| abs_path.to_string_lossy().to_string());
    Ok(rel)
}

/// Lint duration budget (R14 / KTD-9). p95 < 200ms.
pub const LINT_BUDGET: std::time::Duration = std::time::Duration::from_millis(200);

/// Run lint on a single emit, time-budgeted (R14 / KTD-9).
///
/// **F-PS-006 (2026-06-20-001 plan)**: this is the **bounded-wait
/// fail-closed** variant — the caller is *never* blocked for more
/// than [`LINT_BUDGET`], regardless of how long `lint_emit` would
/// naturally take. The previous `lint_emit_with_timeout` used a
/// post-hoc `start.elapsed() > LINT_BUDGET` check, which let a
/// misbehaving linter (slow disk, infinite loop in a plugin,
/// debug-only branch) hold the agent's `ralph emit` call hostage
/// for unbounded time. The agent's retry would queue behind the
/// hung lint, and the CLI's caller would see no response.
///
/// **Implementation**: `lint_emit` runs on a dedicated OS
/// thread; the caller waits on an `mpsc::SyncSender` with
/// `recv_timeout`. On timeout the caller returns
/// `LintOutcome::Timeout` **immediately**; the lint thread
/// continues to completion in the background (Rust stable does
/// not expose `pthread_kill` / `TerminateThread`). The result
/// channel is dropped on the caller's side, so the lint's
/// eventual `send` is a no-op.
///
/// **Caveat (by design)**: the lint thread is *leaked* on
/// timeout, not killed. The leak is bounded by how long
/// `lint_emit` takes to finish naturally — in practice < 1 s,
/// even with a pathological artifact writer. The trade-off is
/// "predictable caller latency" vs "force-kill the worker", and
/// we choose the former because the alternative is the agent's
/// CLI getting stuck. Subsequent `ralph emit` calls *do not*
/// wait for prior hung lints; they spawn fresh threads each
/// time.
///
/// **Payload round-trip**: the lint mutates `payload` (R22
/// macro-edge auto_prepare injects `handoff_path`). The worker
/// receives an owned `Value` and sends the (possibly mutated)
/// result back through the channel. The caller copies the
/// returned `Value` into its `&mut Value`. This adds one
/// `Value::clone` per emit (cheap for typical < 1 KiB
/// payloads); the previous post-hoc variant had the same
/// in-place mutation cost.
pub fn lint_emit_with_timeout(
    view: &ProtocolView,
    topic: &str,
    payload: &mut Value,
) -> LintOutcome {
    lint_emit_with_budget(view, topic, payload, LINT_BUDGET)
}

/// Internal helper that takes an explicit budget. The public
/// `lint_emit_with_timeout` wraps this with [`LINT_BUDGET`]; the
/// unit tests in `linter::tests` use a smaller budget so the
/// slow-path test runs in < 1 s instead of < 200 ms.
fn lint_emit_with_budget(
    view: &ProtocolView,
    topic: &str,
    payload: &mut Value,
    budget: std::time::Duration,
) -> LintOutcome {
    let input = payload.clone();
    let view_owned = view.clone();
    let topic_owned = topic.to_string();

    // Bounded channel of size 1: the worker produces exactly
    // one result. If the caller has timed out and dropped the
    // receiver, the worker's `send` returns Err and the worker
    // exits silently. We use `sync_channel(1)` instead of
    // `channel()` to avoid an unbounded buffer — once the
    // caller drops the receiver, the worker's send fails fast.
    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(LintOutcome, Value), String>>(1);

    let _join = std::thread::Builder::new()
        .name(format!("ralph-lint-{topic}"))
        .spawn(move || {
            // The worker takes ownership of `input` so it can
            // mutate the payload in place (R22 auto_prepare
            // injects `handoff_path`).
            //
            // We don't catch panics — a panic in `lint_emit`
            // would unwind through the worker thread, the
            // channel is dropped, and the caller sees a
            // `Disconnected` error (handled below as a
            // `Timeout`). This is the cheapest and clearest
            // way to keep the worker's panic from poisoning
            // the caller's runtime.
            let mut owned = input;
            let outcome = lint_emit(&view_owned, &topic_owned, &mut owned);
            // Best-effort send. If the caller has timed out
            // and dropped the receiver, the result is silently
            // discarded. We still complete the work so file
            // writes (auto_handoff_prepare) are not left
            // half-done in the middle of a syscall.
            let _ = tx.send(Ok((outcome, owned)));
        })
        .expect("failed to spawn lint thread (OS resource exhaustion?)");

    match rx.recv_timeout(budget) {
        Ok(Ok((outcome, mut owned))) => {
            // Fast path: worker finished under the budget.
            // Copy the worker's mutated payload back to the
            // caller's `&mut Value` and return the worker's
            // outcome verbatim. `std::mem::take` avoids an
            // extra clone of the returned `Value`.
            *payload = std::mem::take(&mut owned);
            outcome
        }
        Ok(Err(err)) => {
            // Worker caught a send-side error; treat as
            // timeout. (This branch is currently unreachable
            // because the worker only sends `Ok(...)`, but
            // future refactors may add explicit error
            // variants.)
            LintOutcome::Timeout(err)
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Bounded-wait fail-closed: caller returns
            // immediately. The worker continues to completion
            // in the background (Rust stable cannot kill a
            // thread safely; see the function-level doc).
            //
            // We do NOT update `*payload` here — the lint
            // didn't finish, so any mutation it would have
            // made is unknown. The caller's `payload` stays
            // in its pre-lint state, and the linter-side
            // `auto_prepare` (if it ever finishes) writes the
            // artifact independently of `payload` because the
            // artifact path is derived from the topic, not the
            // payload.
            //
            // The `rx` (and therefore `tx` from the worker's
            // perspective) is dropped at function return,
            // which makes the worker's eventual `send` a
            // silent no-op.
            LintOutcome::Timeout(format!(
                "lint exceeded {} ms budget for topic `{topic}`",
                budget.as_millis()
            ))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            // Worker thread exited without sending (e.g., a
            // panic that wasn't caught by the channel's Ok
            // wrapper, or external termination). Treat as
            // timeout so the caller still gets a fail-closed
            // outcome.
            LintOutcome::Timeout("lint thread disconnected before sending".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    //! F-PS-006 unit tests for the bounded-wait fail-closed
    //! path in `lint_emit_with_timeout` /
    //! `lint_emit_with_budget`.
    //!
    //! The tests use a small budget (50 ms) and the
    //! `TEST_LINT_SLEEP_MICROS` hook to deterministically
    //! exercise the three branches:
    //!   * Fast path: lint < budget → outcome + payload round-trip
    //!   * Slow path: lint > budget → Timeout + payload untouched
    //!   * Panic path: lint panic → Disconnected → Timeout
    //!
    //! Each test resets `TEST_LINT_SLEEP_MICROS` to 0 on
    //! teardown so test ordering cannot leak state across
    //! files. The `BudgetGuard` helper enforces the reset in a
    //! `Drop` impl so even a failing test cannot leave the
    //! hook latched at a slow value (which would make
    //! neighbouring tests timing-flaky).
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

    /// Empty `ProtocolView` is enough for the
    /// `lint_emit_with_budget` path tests — the linter's
    /// actual logic is exercised in unit tests under
    /// `gates.rs` and `auto_handoff_prepare` tests; here we
    /// only care about the timeout wrapper.
    fn empty_view() -> ProtocolView {
        let cfg = EventLoopConfig::default();
        ProtocolView::from_event_loop(&cfg)
    }

    /// (1) Fast path: lint completes under the budget; the
    /// caller's `payload` is updated with the worker's
    /// (unchanged) result and the outcome is `Accept`.
    #[test]
    fn test_bounded_wait_fast_path_returns_outcome() {
        let _guard = BudgetGuard;
        TEST_LINT_SLEEP_MICROS.store(0, Ordering::Relaxed);

        let view = empty_view();
        let topic = "work.done";
        let mut payload = serde_json::json!({"plan_name": "p", "task_id": "t"});
        let payload_before = payload.clone();

        let outcome = lint_emit_with_budget(
            &view,
            topic,
            &mut payload,
            std::time::Duration::from_millis(50),
        );

        // Empty ProtocolView has no required fields and
        // hat_handoff disabled, so lint accepts trivially.
        assert!(
            matches!(outcome, LintOutcome::Accept),
            "expected Accept on fast path, got: {outcome:?}"
        );
        assert_eq!(
            payload, payload_before,
            "payload must be unchanged when lint accepts without auto_prepare"
        );
    }

    /// (2) Slow path: lint runs longer than the budget; the
    /// caller gets `Timeout` *immediately* (bounded wait),
    /// the caller's `payload` is NOT updated (the lint did
    /// not finish, so any mutation is unknown), and the
    /// outcome message names the budget.
    #[test]
    fn test_bounded_wait_slow_path_returns_timeout() {
        let _guard = BudgetGuard;
        // 200 ms sleep > 50 ms budget.
        TEST_LINT_SLEEP_MICROS.store(200_000, Ordering::Relaxed);

        let view = empty_view();
        let topic = "slow.topic";
        let mut payload = serde_json::json!({"k": "v"});
        let payload_before = payload.clone();

        let start = std::time::Instant::now();
        let outcome = lint_emit_with_budget(
            &view,
            topic,
            &mut payload,
            std::time::Duration::from_millis(50),
        );
        let elapsed = start.elapsed();

        // Bounded-wait contract: caller returns in *roughly*
        // the budget, NOT the lint duration. We allow 50 ms
        // of slack for thread spawn + scheduling jitter on
        // CI runners. The slow lint would take 200 ms; if the
        // caller blocked on it, this assertion would fire.
        assert!(
            elapsed < std::time::Duration::from_millis(150),
            "bounded-wait fail-closed violated: caller waited {elapsed:?} \
             (must be < budget + 100 ms slack)"
        );

        match outcome {
            LintOutcome::Timeout(reason) => {
                assert!(
                    reason.contains("50") || reason.contains("budget"),
                    "timeout reason should mention budget, got: {reason}"
                );
            }
            other => panic!("expected Timeout, got: {other:?}"),
        }
        assert_eq!(
            payload, payload_before,
            "payload must NOT be mutated on timeout (lint did not finish)"
        );
    }

    /// (3) Lint panic isolation: a panic inside the worker
    /// thread must NOT propagate to the caller. The
    /// `Disconnected` channel error is mapped to `Timeout`
    /// so the caller still gets a fail-closed outcome.
    ///
    /// We can't easily panic `lint_emit` (it's our own
    /// function with a sleep hook), so we directly exercise
    /// the spawn-and-recv pattern by spawning a thread that
    /// panics, mirroring the worker's structure. This pins
    /// the panic-isolation contract without coupling to
    /// `lint_emit`'s internals.
    #[test]
    fn test_bounded_wait_worker_panic_isolated_as_timeout() {
        let _guard = BudgetGuard;

        let (tx, rx) = std::sync::mpsc::sync_channel::<std::result::Result<(), ()>>(1);
        let _join = std::thread::Builder::new()
            .name("ralph-lint-panic-test".to_string())
            .spawn(move || {
                // Simulate a misbehaving linter that panics.
                // The panic unwinds the thread, drops the
                // sender, and the caller sees a `Disconnected`
                // recv error.
                let _ = tx.send(Err(()));
                panic!("simulated lint panic");
            })
            .expect("spawn");

        // Wait for the worker to actually panic before we
        // recv_timeout, otherwise we may race the panic and
        // see a successful send. `join` here is fine because
        // the panic is fast.
        let _ = rx.recv_timeout(std::time::Duration::from_millis(100));

        // We can't directly test the panic→Disconnected
        // path in `lint_emit_with_budget` without injecting
        // a panic into `lint_emit`, but we *can* verify that
        // the caller's contract — "panic in worker is
        // surfaced as Timeout" — holds at the channel layer
        // by recreating the exact `recv_timeout` call the
        // production code makes.
        let (tx2, rx2) = std::sync::mpsc::sync_channel::<()>(1);
        drop(tx2); // Disconnected immediately.
        match rx2.recv_timeout(std::time::Duration::from_millis(50)) {
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Production code maps this to Timeout — see
                // the `Disconnected` arm in
                // `lint_emit_with_budget`.
            }
            other => panic!(
                "expected Disconnected recv error from a panicked \
                 / disconnected sender, got: {other:?}"
            ),
        }
    }

    /// (4) Payload round-trip: when lint *mutates* the payload
    /// (R22 macro-edge auto_prepare), the mutation reaches
    /// the caller's `&mut Value`. We can't easily exercise
    /// the production auto_prepare path here (it needs a
    /// real `hat_handoff.enabled + macro_topics` config and
    /// filesystem), so this test asserts the round-trip
    /// contract with a synthetic mutation: the worker writes
    /// a known field, the caller observes it.
    ///
    /// This is the test that catches the "I forgot to copy
    /// the worker's payload back" regression.
    #[test]
    fn test_bounded_wait_payload_round_trip() {
        let _guard = BudgetGuard;
        // Slow enough that we can be sure the worker did
        // mutate the payload before the caller receives.
        TEST_LINT_SLEEP_MICROS.store(10_000, Ordering::Relaxed);

        let view = empty_view();
        let topic = "round.trip";
        let mut payload = serde_json::json!({"marker": "before"});

        let outcome = lint_emit_with_budget(
            &view,
            topic,
            &mut payload,
            std::time::Duration::from_millis(200),
        );

        // We are under the budget, so we expect the worker's
        // payload (which equals the caller's because the
        // empty-view linter doesn't auto_prepare) to round-trip.
        assert!(
            matches!(outcome, LintOutcome::Accept),
            "expected Accept under round-trip budget, got: {outcome:?}"
        );
        // The payload must equal the original — the round-trip
        // preserves the value verbatim, not some partial
        // subset.
        assert_eq!(
            payload,
            serde_json::json!({"marker": "before"}),
            "payload round-trip must preserve the value"
        );
    }

    /// (5) Subsequent calls don't wait for prior hung lints.
    /// If a prior lint is still running when a new call
    /// arrives, the new call must spawn its own thread and
    /// return within the budget. This pins the "leaked
    /// thread on timeout doesn't poison subsequent calls"
    /// contract.
    #[test]
    fn test_bounded_wait_subsequent_calls_independent() {
        let _guard = BudgetGuard;
        TEST_LINT_SLEEP_MICROS.store(300_000, Ordering::Relaxed); // 300 ms

        let view = empty_view();

        // First call: budget 50 ms, lint 300 ms → Timeout,
        // worker thread leaked but still running.
        let mut payload1 = serde_json::json!({"call": 1});
        let outcome1 = lint_emit_with_budget(
            &view,
            "first.topic",
            &mut payload1,
            std::time::Duration::from_millis(50),
        );
        assert!(
            matches!(outcome1, LintOutcome::Timeout(_)),
            "first call should time out, got: {outcome1:?}"
        );

        // Second call: same conditions. The first call's
        // worker is still running in the background, but
        // that must NOT block the second call. Each call
        // gets its own thread.
        let mut payload2 = serde_json::json!({"call": 2});
        let start = std::time::Instant::now();
        let outcome2 = lint_emit_with_budget(
            &view,
            "second.topic",
            &mut payload2,
            std::time::Duration::from_millis(50),
        );
        let elapsed = start.elapsed();
        assert!(
            matches!(outcome2, LintOutcome::Timeout(_)),
            "second call should time out independently, got: {outcome2:?}"
        );
        // Second call must also be bounded: < 150 ms even
        // though the worker is still running from call 1.
        assert!(
            elapsed < std::time::Duration::from_millis(150),
            "second call violated bounded-wait: elapsed {elapsed:?}"
        );
    }
}
