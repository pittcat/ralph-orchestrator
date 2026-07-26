//! U3: worker outcome reason classification — retryable vs
//! permanent vs unknown-fail-closed.
//!
//! Constants and classification logic live in `worker_outcome.rs`.
//! This module contains only the standalone test suite.

#[cfg(test)]
mod tests {
    use crate::supervisor::worker_outcome::{
        FailureClassification, REASON_CONFLICTING_WORKER_TERMINAL, REASON_EMPTY_WORKER_RESULT,
        REASON_INVALID_CONTROL_PLANE_PATH, REASON_MISSING_WORKER_TERMINAL,
        REASON_SLOT_NEVER_STARTED, REASON_WORKER_CANCELLED, REASON_WORKER_TIMEOUT,
        classify_failure_reason, is_retryable_slot_reason,
    };

    // ── retryable reasons ────────────────────────────────────────────────

    #[test]
    fn retryable_reasons_are_retryable() {
        for reason in [
            REASON_WORKER_TIMEOUT,
            REASON_EMPTY_WORKER_RESULT,
            REASON_MISSING_WORKER_TERMINAL,
            REASON_SLOT_NEVER_STARTED,
        ] {
            assert!(
                is_retryable_slot_reason(reason),
                "reason {reason:?} must be retryable"
            );
        }
    }

    #[test]
    fn retryable_reasons_classify_as_retryable() {
        for reason in [
            REASON_WORKER_TIMEOUT,
            REASON_EMPTY_WORKER_RESULT,
            REASON_MISSING_WORKER_TERMINAL,
            REASON_SLOT_NEVER_STARTED,
        ] {
            assert_eq!(
                classify_failure_reason(reason),
                FailureClassification::Retryable,
                "reason {reason:?} must classify as Retryable"
            );
        }
    }

    // ── non-retryable reasons ─────────────────────────────────────────────

    #[test]
    fn non_retryable_reasons_are_not_retryable() {
        for reason in [
            REASON_CONFLICTING_WORKER_TERMINAL,
            REASON_INVALID_CONTROL_PLANE_PATH,
            REASON_WORKER_CANCELLED,
        ] {
            assert!(
                !is_retryable_slot_reason(reason),
                "reason {reason:?} must NOT be retryable"
            );
        }
    }

    #[test]
    fn non_retryable_reasons_classify_as_permanent() {
        for reason in [
            REASON_CONFLICTING_WORKER_TERMINAL,
            REASON_INVALID_CONTROL_PLANE_PATH,
            REASON_WORKER_CANCELLED,
        ] {
            assert_eq!(
                classify_failure_reason(reason),
                FailureClassification::Permanent,
                "reason {reason:?} must classify as Permanent"
            );
        }
    }

    // ── aggregate / wave-level failures are Permanent ────────────────────

    #[test]
    fn aggregate_timeout_is_permanent() {
        assert!(
            !is_retryable_slot_reason("aggregate_timeout"),
            "aggregate_timeout must NOT be retryable"
        );
        assert_eq!(
            classify_failure_reason("aggregate_timeout"),
            FailureClassification::Permanent,
        );
    }

    #[test]
    fn aggregate_deadline_exceeded_is_permanent() {
        assert!(
            !is_retryable_slot_reason("aggregate_deadline_exceeded"),
            "aggregate_deadline_exceeded must NOT be retryable"
        );
        assert_eq!(
            classify_failure_reason("aggregate_deadline_exceeded"),
            FailureClassification::Permanent,
        );
    }

    #[test]
    fn wave_cancelled_is_permanent() {
        assert!(
            !is_retryable_slot_reason("wave_cancelled"),
            "wave_cancelled must NOT be retryable"
        );
        assert_eq!(
            classify_failure_reason("wave_cancelled"),
            FailureClassification::Permanent,
        );
    }

    // ── unknown reasons: fail-closed ─────────────────────────────────────

    #[test]
    fn unknown_reason_fail_closed() {
        assert!(
            !is_retryable_slot_reason("random_new_reason_xyz"),
            "unknown reason must NOT be retryable (fail-closed)"
        );
        assert_eq!(
            classify_failure_reason("random_new_reason_xyz"),
            FailureClassification::UnknownFailClosed,
        );
    }

    #[test]
    fn whitespace_only_reason_fail_closed() {
        assert!(
            !is_retryable_slot_reason(""),
            "empty reason must NOT be retryable"
        );
        assert_eq!(
            classify_failure_reason(""),
            FailureClassification::UnknownFailClosed
        );

        assert!(
            !is_retryable_slot_reason("   "),
            "whitespace-only reason must NOT be retryable"
        );
        assert_eq!(
            classify_failure_reason("   "),
            FailureClassification::UnknownFailClosed,
        );
    }

    #[test]
    fn case_sensitive_lowercase_only() {
        // All constants are lowercase; wrong case is an unknown reason.
        assert!(
            !is_retryable_slot_reason("WORKER_TIMEOUT"),
            "uppercase variant must be unknown"
        );
        assert_eq!(
            classify_failure_reason("WORKER_TIMEOUT"),
            FailureClassification::UnknownFailClosed,
        );

        assert!(
            !is_retryable_slot_reason("Worker_Timeout"),
            "mixed-case variant must be unknown"
        );
        assert_eq!(
            classify_failure_reason("Worker_Timeout"),
            FailureClassification::UnknownFailClosed,
        );
    }

    #[test]
    fn truncated_or_partial_reason_fail_closed() {
        // A truncated constant is a new, unknown string.
        assert!(
            !is_retryable_slot_reason("worker_tim"),
            "truncated reason must be unknown"
        );
        assert_eq!(
            classify_failure_reason("worker_tim"),
            FailureClassification::UnknownFailClosed,
        );

        assert!(
            !is_retryable_slot_reason("missing_worker_term"),
            "partial reason must be unknown"
        );
        assert_eq!(
            classify_failure_reason("missing_worker_term"),
            FailureClassification::UnknownFailClosed,
        );
    }
}
