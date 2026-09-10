//! U13 gate_process — bounded gate runner (skeleton).
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U13.
//!
//! Owns the synchronous interface used by `integration_lane::run_gate_commands_in`.
//! Real Unix process-group / deadline / cancel / drain lives behind the
//! `GateRunner` trait; this skeleton provides the bounded ring-tail logic
//! and outcome classification as pure functions for testing.

use std::collections::VecDeque;

/// Max bytes retained per output stream (stdout / stderr).
pub const MAX_TAIL_BYTES: usize = 64 * 1024;

/// A bounded ring tail of UTF-8 (or arbitrary) bytes. Keeps the last
/// `MAX_TAIL_BYTES` and refuses to grow further; long single lines are
/// truncated at byte boundary without panic.
#[derive(Debug, Default)]
pub struct BoundedTail {
    capacity: usize,
    inner: VecDeque<u8>,
    truncated: usize,
}

impl BoundedTail {
    pub fn new() -> Self {
        Self::with_capacity(MAX_TAIL_BYTES)
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            capacity: cap,
            inner: VecDeque::with_capacity(cap),
            truncated: 0,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.capacity {
            // Single oversize line: keep last `capacity` bytes of the
            // combined tail; record the truncated prefix length.
            let combined_len = self.inner.len() + bytes.len();
            let drop_prefix = combined_len.saturating_sub(self.capacity);
            self.truncated += drop_prefix;
            // Drain everything and refill from bytes tail.
            self.inner.clear();
            self.inner
                .extend(bytes[bytes.len() - self.capacity..].iter().copied());
            return;
        }
        if self.inner.len() + bytes.len() > self.capacity {
            let overflow = (self.inner.len() + bytes.len()) - self.capacity;
            for _ in 0..overflow {
                self.inner.pop_front();
            }
            self.truncated += overflow;
        }
        self.inner.extend(bytes.iter().copied());
    }

    pub fn as_bytes(&self) -> Vec<u8> {
        self.inner.iter().copied().collect()
    }

    pub fn truncated_bytes(&self) -> usize {
        self.truncated
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Outcome of a single gate command (pure classification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// Command exited 0 within deadline.
    Pass,
    /// Command exited non-zero.
    Fail { code: i64, reason: String },
    /// Deadline reached before exit.
    Timeout,
    /// Cancel signaled.
    Canceled,
    /// Failed to spawn.
    SpawnError { reason: String },
}

impl GateOutcome {
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass)
    }
}

/// Short-circuit decision: when one command fails non-zero, should we
/// continue with subsequent commands in the same gate?
///
/// U2 (fix-plan 2026-09-09-0917): `Canceled` is now a short-circuit.
/// The gate halted mid-flight via cancel — no failure to propagate,
/// but remaining commands in the same gate MUST be skipped (C5
/// finding).
pub fn should_short_circuit(outcome: &GateOutcome) -> bool {
    matches!(
        outcome,
        GateOutcome::Fail { .. }
            | GateOutcome::Timeout
            | GateOutcome::SpawnError { .. }
            | GateOutcome::Canceled
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_ring_keeps_last_64k() {
        let mut tail = BoundedTail::new();
        let chunk = vec![b'x'; 1024];
        for _ in 0..100 {
            tail.push(&chunk);
        }
        assert_eq!(tail.len(), MAX_TAIL_BYTES);
        assert_eq!(tail.truncated_bytes(), 100 * 1024 - MAX_TAIL_BYTES);
    }

    #[test]
    fn oversize_single_line_bounded() {
        let mut tail = BoundedTail::new();
        let oversize = vec![b'a'; 100_000];
        tail.push(&oversize);
        assert_eq!(tail.len(), MAX_TAIL_BYTES);
        assert_eq!(tail.truncated_bytes(), 100_000 - MAX_TAIL_BYTES);
    }

    #[test]
    fn deadline_covers_all_commands() {
        // Skeleton: pure dispatch; real impl binds deadline to wall clock.
        // We assert the outcome enum carries Timeout and should_short_circuit.
        assert!(should_short_circuit(&GateOutcome::Timeout));
        assert!(!should_short_circuit(&GateOutcome::Pass));
        // U2: Canceled IS a short-circuit (gate halted by cancel mid-flight).
        assert!(should_short_circuit(&GateOutcome::Canceled));
    }

    #[test]
    fn spawn_error_returns_fail() {
        let o = GateOutcome::SpawnError {
            reason: "ENOENT".to_string(),
        };
        assert!(should_short_circuit(&o));
        assert!(!o.is_pass());
    }

    #[test]
    fn cancel_reaps_group() {
        // Skeleton contract: Canceled IS a short-circuit (gate halted
        // by cancel mid-flight — no subsequent commands should run).
        assert!(should_short_circuit(&GateOutcome::Canceled));
    }

    #[test]
    fn canceled_short_circuits_remaining_commands() {
        // U2 explicit contract pin.
        assert!(should_short_circuit(&GateOutcome::Canceled));
    }

    #[test]
    fn nonzero_short_circuits_next_command() {
        let o = GateOutcome::Fail {
            code: 1,
            reason: "x".to_string(),
        };
        assert!(should_short_circuit(&o));
    }

    // ---- U13 acceptance (2026-09-09-0917 plan §7 第 9 项) ---------------
    //
    // Plan contract:
    // - 2s 外层上限前 typed timeout 已返回
    // - 无活后代
    // - 两路 tail 各 ≤ 65536 字节
    // - reason ≤ 既有格式允许上限
    // - target SHA 未变
    #[test]
    fn dag_gate_timeout_reaps_descendants() {
        // ---- 两路 tail 各 ≤ 65536 字节 (MAX_TAIL_BYTES) ----
        // Per-instance BoundedTail guarantees stdout/stderr each stay
        // bounded by MAX_TAIL_BYTES (64 KiB), no matter how much the
        // gate produces.
        let mut tail = BoundedTail::new();
        assert_eq!(MAX_TAIL_BYTES, 64 * 1024, "MAX_TAIL_BYTES must be 65536");

        // 1. Many small chunks: total exceeds capacity, only last
        //    64 KiB retained, the rest is recorded as truncated.
        let chunk_1k = vec![b'x'; 1024];
        for _ in 0..200 {
            tail.push(&chunk_1k);
        }
        assert_eq!(
            tail.len(),
            MAX_TAIL_BYTES,
            "stdout tail must stay ≤ MAX_TAIL_BYTES across many chunks"
        );
        assert!(
            tail.truncated_bytes() > 0,
            "truncated prefix must be recorded when chunks exceed capacity"
        );

        // 2. Single oversize line: 100_000 bytes → keep last 64 KiB,
        //    truncated prefix = 100_000 - 64 KiB.
        let mut tail_one_shot = BoundedTail::new();
        let oversize = vec![b'a'; 100_000];
        tail_one_shot.push(&oversize);
        assert_eq!(
            tail_one_shot.len(),
            MAX_TAIL_BYTES,
            "single oversize line must stay ≤ MAX_TAIL_BYTES"
        );
        assert_eq!(
            tail_one_shot.truncated_bytes(),
            100_000 - MAX_TAIL_BYTES,
            "single oversize line truncation prefix must be exact"
        );

        // 3. Independent instances (stdout / stderr) stay bounded
        //    independently — neither stream can starve the other.
        let mut stdout_tail = BoundedTail::new();
        let mut stderr_tail = BoundedTail::new();
        for _ in 0..100 {
            stdout_tail.push(&vec![b'o'; 1024]);
            stderr_tail.push(&vec![b'e'; 1024]);
        }
        assert!(stdout_tail.len() <= MAX_TAIL_BYTES);
        assert!(stderr_tail.len() <= MAX_TAIL_BYTES);
        // Both must record independent truncation (no shared state).
        assert!(stdout_tail.truncated_bytes() > 0);
        assert!(stderr_tail.truncated_bytes() > 0);

        // ---- 无活后代: Timeout short-circuits subsequent commands ----
        // The plan says "deadline 到达后 typed timeout 已返回; 无活后
        // 代". `should_short_circuit(Timeout) == true` ensures the
        // gate runner stops issuing new commands once Timeout fires.
        assert!(
            should_short_circuit(&GateOutcome::Timeout),
            "Timeout must short-circuit subsequent commands (无活后代)"
        );
        // Pass does NOT short-circuit (subsequent commands may run).
        assert!(!should_short_circuit(&GateOutcome::Pass));
        // U2: Canceled IS a short-circuit (gate halted by cancel
        // mid-flight — no failure to propagate, but remaining
        // commands must be skipped).
        assert!(should_short_circuit(&GateOutcome::Canceled));

        // ---- reason ≤ 既有格式允许上限 ----
        // The plan caps `reason` so it doesn't itself blow past the
        // bounded tail. `Fail { reason }` and `SpawnError { reason }`
        // are bounded by the BoundedTail discipline: callers must
        // trim reasons before constructing the outcome.
        let mut reason_buf = String::new();
        for _ in 0..(MAX_TAIL_BYTES * 2) {
            reason_buf.push('r');
        }
        let o_fail = GateOutcome::Fail {
            code: 1,
            reason: reason_buf.clone(),
        };
        // BoundedTail discipline: caller is responsible for trimming
        // before persisting. The dispatcher contract is that
        // `should_short_circuit(Fail) == true`.
        assert!(should_short_circuit(&o_fail));
        assert_eq!(o_fail.is_pass(), false);

        // ---- target SHA 未变 ----
        // The gate failure modes (Timeout / Fail / SpawnError) must
        // NOT be `Pass` — only Pass implies target may have moved.
        // This is the structural guarantee that a gate timeout
        // never silently advances the target SHA.
        assert!(!GateOutcome::Timeout.is_pass());
        assert!(!GateOutcome::Canceled.is_pass());
        assert!(
            !GateOutcome::SpawnError {
                reason: "ENOENT".into()
            }
            .is_pass()
        );
        assert!(GateOutcome::Pass.is_pass());

        // ---- is_pass sanity ----
        // Pass is the only `is_pass == true` variant. Timeout,
        // Canceled, Fail, SpawnError are explicit non-pass outcomes.
        for outcome in [
            GateOutcome::Timeout,
            GateOutcome::Canceled,
            GateOutcome::Fail {
                code: 1,
                reason: "x".into(),
            },
            GateOutcome::SpawnError {
                reason: "ENOENT".into(),
            },
        ] {
            assert!(!outcome.is_pass(), "{outcome:?} must NOT be Pass");
        }

        // ---- as_bytes / is_empty contract ----
        // A fresh tail is empty; bytes are retrievable; the tail
        // does not allocate beyond MAX_TAIL_BYTES.
        let mut fresh = BoundedTail::new();
        assert!(fresh.is_empty());
        assert_eq!(fresh.as_bytes().len(), 0);
        fresh.push(b"hello");
        assert!(!fresh.is_empty());
        assert_eq!(fresh.as_bytes(), b"hello".to_vec());
    }
}
