//! Accepted-event commit boundary (2026-07-07-002 plan Unit 1).
//!
//! Pure helpers that classify a candidate event as committable to the main
//! events ledger, rejected to diagnostics only, or ignored (duplicate /
//! post-terminal duplicate). No disk I/O, no EventLoop wiring — Unit 2+
//! consume these types at the commit gate.

use crate::event_loop::rejection::RejectionStage;
use serde::{Deserialize, Serialize};

/// Topic classification for terminal-closed and commit-boundary decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicClass {
    /// Business workflow topics (`work.done`, `plan.blocked`, etc.).
    Business,
    /// Terminal-adjacent topics whose duplicate payloads are ignored.
    TerminalAdjacent,
    /// Control topics (`task.resume`, `loop.cancel`, …).
    Control,
    /// Diagnostic / inspect topics — never blocked by terminal guard.
    Diagnostic,
}

/// Metadata carried when a candidate is accepted for main-events commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedMetadata {
    /// Which gate stage last accepted the candidate (if known).
    pub last_stage: Option<RejectionStage>,
    /// Optional human-readable note for diagnostics.
    pub note: Option<String>,
}

/// Candidate event at the commit boundary (in-memory only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateEvent {
    pub topic: String,
    pub payload: String,
}

/// Why a candidate is not committable to main events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NonCommittableReason {
    /// Hard rejection — write rejection/recovery diagnostics only.
    Rejected {
        stage: RejectionStage,
        reason_code: String,
        message: String,
    },
    /// Soft ignore — duplicate terminal-adjacent or post-terminal duplicate;
    /// not a hard rejection but must not enter main events.
    Ignored {
        reason_code: String,
        message: String,
    },
}

/// Outcome of the commit-boundary classifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum CommitDisposition {
    /// Safe to write to main events and update state projection.
    Committable {
        candidate: CandidateEvent,
        metadata: AcceptedMetadata,
    },
    /// Must not enter main events.
    NonCommittable {
        candidate: CandidateEvent,
        reason: NonCommittableReason,
    },
}

impl CommitDisposition {
    /// True when the candidate may be written to the main events ledger.
    #[must_use]
    pub fn is_committable(&self) -> bool {
        matches!(self, Self::Committable { .. })
    }

    /// Topic of the underlying candidate regardless of disposition.
    #[must_use]
    pub fn topic(&self) -> &str {
        match self {
            Self::Committable { candidate, .. } | Self::NonCommittable { candidate, .. } => {
                candidate.topic.as_str()
            }
        }
    }
}

/// Classify an already-validated accept path as committable.
#[must_use]
pub fn classify_accepted(
    candidate: CandidateEvent,
    metadata: AcceptedMetadata,
) -> CommitDisposition {
    CommitDisposition::Committable {
        candidate,
        metadata,
    }
}

/// Classify a hard rejection — diagnostics only, never main events.
#[must_use]
pub fn classify_rejected(
    candidate: CandidateEvent,
    stage: RejectionStage,
    reason_code: impl Into<String>,
    message: impl Into<String>,
) -> CommitDisposition {
    CommitDisposition::NonCommittable {
        candidate,
        reason: NonCommittableReason::Rejected {
            stage,
            reason_code: reason_code.into(),
            message: message.into(),
        },
    }
}

/// Classify an ignored duplicate — non-committable but not a hard rejection.
#[must_use]
pub fn classify_ignored(
    candidate: CandidateEvent,
    reason_code: impl Into<String>,
    message: impl Into<String>,
) -> CommitDisposition {
    CommitDisposition::NonCommittable {
        candidate,
        reason: NonCommittableReason::Ignored {
            reason_code: reason_code.into(),
            message: message.into(),
        },
    }
}

/// Map a rejection stage + violation code into a commit-boundary rejection.
#[must_use]
pub fn from_execution_contract_rejection(
    candidate: CandidateEvent,
    stage: RejectionStage,
    violation_code: impl Into<String>,
    message: impl Into<String>,
) -> CommitDisposition {
    classify_rejected(candidate, stage, violation_code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work_done_candidate() -> CandidateEvent {
        CandidateEvent {
            topic: "work.done".to_string(),
            payload: r#"{"task_id":"t1","task_key":"k1","step":"step-01"}"#.to_string(),
        }
    }

    #[test]
    fn test_classify_accepted_work_done_is_committable() {
        let disposition = classify_accepted(
            work_done_candidate(),
            AcceptedMetadata {
                last_stage: None,
                note: None,
            },
        );
        assert!(disposition.is_committable());
        assert_eq!(disposition.topic(), "work.done");
        match disposition {
            CommitDisposition::Committable { candidate, .. } => {
                assert_eq!(candidate.topic, "work.done");
            }
            CommitDisposition::NonCommittable { .. } => panic!("expected committable"),
        }
    }

    #[test]
    fn test_classify_rejected_task_not_terminal_is_non_committable() {
        let disposition = classify_rejected(
            work_done_candidate(),
            RejectionStage::ExecutionContract,
            "task_not_terminal",
            "task t1 status open, allowed [closed]",
        );
        assert!(!disposition.is_committable());
        match disposition {
            CommitDisposition::NonCommittable { reason, .. } => match reason {
                NonCommittableReason::Rejected {
                    stage,
                    reason_code,
                    ..
                } => {
                    assert_eq!(stage, RejectionStage::ExecutionContract);
                    assert_eq!(reason_code, "task_not_terminal");
                }
                NonCommittableReason::Ignored { .. } => panic!("expected rejected"),
            },
            CommitDisposition::Committable { .. } => panic!("expected non-committable"),
        }
    }

    #[test]
    fn test_classify_ignored_duplicate_terminal_is_non_committable_not_hard_reject() {
        let disposition = classify_ignored(
            CandidateEvent {
                topic: "LOOP_COMPLETE".to_string(),
                payload: r#"{"status":"done"}"#.to_string(),
            },
            "duplicate_terminal_adjacent",
            "byte-identical LOOP_COMPLETE already honored",
        );
        assert!(!disposition.is_committable());
        match disposition {
            CommitDisposition::NonCommittable { reason, .. } => {
                assert!(matches!(reason, NonCommittableReason::Ignored { .. }));
            }
            CommitDisposition::Committable { .. } => panic!("expected non-committable"),
        }
    }

    #[test]
    fn test_rejected_and_ignored_never_committable() {
        let rejected = classify_rejected(
            work_done_candidate(),
            RejectionStage::Policy,
            "duplicate_work_done",
            "duplicate",
        );
        let ignored = classify_ignored(
            work_done_candidate(),
            "post_terminal_duplicate",
            "terminal closed",
        );
        assert!(!rejected.is_committable());
        assert!(!ignored.is_committable());
    }
}
