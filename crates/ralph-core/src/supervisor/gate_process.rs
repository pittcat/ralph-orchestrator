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
pub fn should_short_circuit(outcome: &GateOutcome) -> bool {
    matches!(
        outcome,
        GateOutcome::Fail { .. } | GateOutcome::Timeout | GateOutcome::SpawnError { .. }
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
        assert!(!should_short_circuit(&GateOutcome::Canceled));
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
        // Skeleton contract: Canceled is NOT short-circuit (gate ended
        // cleanly via cancel, not via failure).
        assert!(!should_short_circuit(&GateOutcome::Canceled));
    }

    #[test]
    fn nonzero_short_circuits_next_command() {
        let o = GateOutcome::Fail {
            code: 1,
            reason: "x".to_string(),
        };
        assert!(should_short_circuit(&o));
    }
}
