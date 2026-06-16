//! Unified rejection type and targeted-retry machinery.
//!
//! 2026-06-07 plan Unit 2: 建立统一的拒绝分类与定向重试
//!
//! Origin guard, payload policy and execution contract all reject events
//! for different reasons and from different layers.  This module
//! normalises those findings into a single [`Rejection`] type so the
//! runner can:
//!
//!   1. Compute a stable per-event `retry_key` (used for bounded retry).
//!   2. Decide if the rejection is retryable (the source hat is a
//!      registered hat, the violation is in the recoverable set, and the
//!      retry budget for this key is not yet exhausted).
//!   3. Pick the targeted hat for the resume event — almost always the
//!      source/business hat, never a hat that doesn't own the topic.
//!
//! Fail-closed invariants (R1) — unknown hats, declared hat publishing
//! an off-graph topic, or violations that are not in the recoverable set
//! (e.g. `executor` cannot publish a `LOOP_COMPLETE` from a fallback
//! context) — return `Rejection::non_retryable` and the caller MUST
//! escalate to human guidance rather than auto-publish a `task.resume`.

use crate::event_origin::OriginCheck;
use crate::execution_contract::ExecutionContractFinding;
use ralph_proto::HatId;
use serde::{Deserialize, Serialize};

/// Which layer rejected the event.  Used both for diagnostics and for
/// the `stage` portion of the rejection key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionStage {
    /// Rejected by the origin guard (out-of-scope topic for declared
    /// hat, or unknown hat).
    Origin,
    /// Rejected by payload policy (event_policy.validate_event).
    Policy,
    /// Rejected by the execution contract.
    ExecutionContract,
    /// Rejected by the payload contract.
    PayloadContract,
}

impl RejectionStage {
    /// Short, stable string used inside the retry key.
    pub fn as_str(&self) -> &'static str {
        match self {
            RejectionStage::Origin => "origin",
            RejectionStage::Policy => "policy",
            RejectionStage::ExecutionContract => "execution_contract",
            RejectionStage::PayloadContract => "payload_contract",
        }
    }
}

/// Why the rejection is not retryable.  This is a closed enum so the
/// runner can pattern-match it for diagnostics.  RecoveryResponder (U6)
/// uses this same enum to escalate to the right responder level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NonRetryableReason {
    /// The hat that emitted the event is not registered in the
    /// current registry.  Cannot construct a `task.resume` target
    /// without a registered destination.
    UnknownHat,
    /// The topic is off-graph for the declared hat and the hat
    /// refuses to take ownership (origin guard).
    OutOfScope,
    /// The violation is not in the recoverable set (e.g. an
    /// `executor` tries to publish a terminal `LOOP_COMPLETE`).
    NotRecoverable,
    /// A retry budget for this rejection key has been exhausted.
    /// Caller must terminate or escalate.
    RetryBudgetExhausted,
    /// Caller-supplied custom reason (e.g. the runner detected a
    /// state it cannot recover from).
    Custom(String),
    /// Topic is not in the whitelist of known topics (R9).
    /// Rejected without retry — only writes a recovery signal (R10).
    InvalidTopicFormat,
}

/// Unified description of an event that was rejected somewhere in the
/// pipeline.  Constructed from the various finding types
/// ([`OriginCheck::Rejected`], [`ExecutionContractFinding`], payload
/// contract violations) so the runner can treat them uniformly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    /// Which layer rejected the event.
    pub stage: RejectionStage,
    /// The hat field on the original event (may be unknown for raw
    /// JSONL that was synthesised by the agent).
    pub source_hat: Option<String>,
    /// The business hat this event was attributed to by the
    /// Coordinator-mode `display_hat` resolution.  May differ from
    /// `source_hat` when outputs are remapped.
    pub business_hat: Option<String>,
    /// Topic the event was attempting to publish.
    pub topic: String,
    /// Free-form violation description, suitable for human readers
    /// and for the prompt that follows the `task.resume` event.
    pub violation: String,
    /// Stable retry key — same source hat + topic + violation class
    /// + stage collapses to the same key, which the runner uses to
    /// bound retry attempts.  Computed via [`Rejection::retry_key`].
    pub retry_key: String,
    /// True if a `task.resume` should be published to reactivate
    /// the source hat.  False → runner must escalate.
    pub retry_eligible: bool,
    /// Populated when `retry_eligible` is false.  The runner uses
    /// this to surface the right `RecoveryDiagnosisEnvelope` and
    /// final-reason class.
    pub non_retryable_reason: Option<NonRetryableReason>,
    /// Hat the runner should re-dispatch to, when retry_eligible.
    /// Mirrors `source_hat` (or `business_hat` when source is missing)
    /// and is the value the `task.resume` event's `target` field
    /// should carry.
    pub target_hat: Option<String>,
    /// 2026-06-16-001 U3: id of the original event that triggered
    /// this rejection. Used by the `task.resume` freshness filter to
    /// correlate the rejection to its source and to drop stale
    /// rejections whose source event no longer exists in the
    /// recovery log. Optional for backwards compatibility with
    /// rejection paths that pre-date the TTL filter.
    pub original_event_id: Option<String>,
    /// 2026-06-16-001 U3: timestamp of the original event. Used to
    /// compute the age against `task_resume_ttl_seconds`. Falls
    /// back to the rejection creation time when the source event
    /// had no `ts` field (legacy JSONL or synthesised records).
    pub original_ts: Option<String>,
}

impl Rejection {
    /// Wrap a topic-format rejection (R9/R10).
    ///
    /// Topic format rejections are **always non-retryable** — the agent
    /// emitted an unknown topic and retrying with the same topic would
    /// just hit the same rejection.  Only a recovery signal is written.
    pub fn from_topic_format(
        source_hat: Option<String>,
        topic: String,
        _allowed: &[String],
    ) -> Self {
        let mut s = Self {
            stage: RejectionStage::Policy,
            source_hat: source_hat.clone(),
            business_hat: source_hat.clone(),
            topic: topic.clone(),
            violation: format!("Topic '{}' is not in the whitelist of known topics", topic),
            retry_key: String::new(),
            retry_eligible: false,
            non_retryable_reason: Some(NonRetryableReason::InvalidTopicFormat),
            target_hat: None,
            // 2026-06-16-001 U3: topic-format rejections are non-retryable
            // by definition, so freshness metadata is informational only.
            original_event_id: None,
            original_ts: None,
        };
        s.retry_key = s.compute_retry_key();
        s
    }

    /// Wrap an origin-guard rejection.  R1: unknown hat or out-of-scope
    /// topic is non-retryable; we cannot construct a `task.resume`
    /// target without a registered destination or a topic the target
    /// can publish.
    pub fn from_origin(source_hat: Option<String>, topic: String, reason: &str) -> Self {
        let (retry_eligible, non_retryable_reason) = classify_origin_reason(reason);
        let target_hat = source_hat.clone();
        let mut s = Self {
            stage: RejectionStage::Origin,
            source_hat: source_hat.clone(),
            business_hat: source_hat.clone(),
            topic: topic.clone(),
            violation: reason.to_string(),
            retry_key: String::new(),
            retry_eligible,
            non_retryable_reason,
            target_hat: if retry_eligible { target_hat } else { None },
            // 2026-06-16-001 U3: legacy origin-guard constructor does
            // not capture the source event's id/ts; freshness falls
            // back to the rejection creation time.
            original_event_id: None,
            original_ts: None,
        };
        s.retry_key = s.compute_retry_key();
        s
    }

    /// Wrap an execution-contract rejection.  Almost always retryable
    /// when the source hat is a registered business hat — the contract
    /// failure is a payload-shape problem the hat can fix on retry.
    ///
    /// `source_hat` is supplied by the caller (the runner) because
    /// `ExecutionContractFinding` does not carry provenance on its own.
    /// `business_hat` is the Coordinator-mode display hat, used only for
    /// diagnostics — retry routing always uses `source_hat`.
    pub fn from_execution_contract(
        finding: &ExecutionContractFinding,
        source_hat: Option<String>,
        business_hat: Option<String>,
    ) -> Self {
        let retry_eligible = source_hat.is_some();
        let non_retryable_reason = if retry_eligible {
            None
        } else {
            Some(NonRetryableReason::UnknownHat)
        };
        let target_hat = if retry_eligible {
            source_hat.clone()
        } else {
            None
        };
        let mut s = Self {
            stage: RejectionStage::ExecutionContract,
            source_hat: source_hat.clone(),
            business_hat,
            topic: finding.topic.clone(),
            violation: format!("{:?}: {}", finding.kind, finding.message),
            retry_key: String::new(),
            retry_eligible,
            non_retryable_reason,
            target_hat,
            // 2026-06-16-001 U3: execution-contract rejections do
            // not yet capture source event metadata — the freshness
            // filter treats them as fresh until the contract layer
            // is updated in a follow-up.
            original_event_id: None,
            original_ts: None,
        };
        s.retry_key = s.compute_retry_key();
        s
    }

    /// Stable retry key (R2 + R3).  The same source hat + topic +
    /// violation class + stage always collapses to the same key, so
    /// the runner can count "this is the 2nd time `executor` failed
    /// `work.done` with a `MissingPayloadField`" without conflating
    /// different violation kinds.
    pub fn compute_retry_key(&self) -> String {
        let source = self.source_hat.as_deref().unwrap_or("unknown");
        let violation_class = violation_class(&self.violation);
        format!(
            "{}:{}:{}:{}",
            self.stage.as_str(),
            source,
            self.topic,
            violation_class,
        )
    }

    /// True if a `task.resume` should be published.  Convenience over
    /// `retry_eligible` to keep call sites readable.
    pub fn should_publish_resume(&self) -> bool {
        self.retry_eligible && self.target_hat.is_some()
    }
}

fn classify_origin_reason(reason: &str) -> (bool, Option<NonRetryableReason>) {
    if reason.contains("unknown") {
        (false, Some(NonRetryableReason::UnknownHat))
    } else if reason.contains("out-of-scope") {
        (false, Some(NonRetryableReason::OutOfScope))
    } else {
        (false, Some(NonRetryableReason::NotRecoverable))
    }
}

/// Bucket a free-form violation string into a stable class.  Used by
/// the retry key so two different `MissingPayloadField` rejections
/// (one for `plan_path`, one for `task_id`) collapse to the same key
/// while a `MissingPayloadField` and a `TypeMismatch` stay distinct.
fn violation_class(violation: &str) -> &'static str {
    let lower = violation.to_lowercase();
    if lower.contains("not in the whitelist") || lower.contains("topic format") {
        "invalid_topic_format"
    } else if lower.contains("missingpayloadfield") || lower.contains("missing") {
        "missing_field"
    } else if lower.contains("typemismatch") {
        "type_mismatch"
    } else if lower.contains("task") && lower.contains("not") {
        "task_state"
    } else if lower.contains("out-of-scope") {
        "out_of_scope"
    } else if lower.contains("unknown hat") {
        "unknown_hat"
    } else {
        "other"
    }
}

/// Convert an [`OriginCheck::Rejected`] to a [`Rejection`].  Helper
/// for callers that already have the origin guard's verdict.
pub fn rejection_from_origin(check: &OriginCheck, source_hat: Option<String>) -> Option<Rejection> {
    match check {
        OriginCheck::Rejected { topic, hat, reason } => Some(Rejection::from_origin(
            hat.clone().or(source_hat),
            topic.clone(),
            reason,
        )),
        OriginCheck::Accepted => None,
    }
}

/// Helper for tests: build a [`Rejection`] with a custom retry key
/// override.  Used to test bounded retry counting where two rejections
/// would otherwise share a key.
#[allow(dead_code)]
pub fn rejection_with_key(
    stage: RejectionStage,
    source_hat: Option<String>,
    topic: impl Into<String>,
    violation: impl Into<String>,
    retry_key: impl Into<String>,
) -> Rejection {
    let target_hat = source_hat.clone();
    Rejection {
        stage,
        source_hat: source_hat.clone(),
        business_hat: source_hat,
        topic: topic.into(),
        violation: violation.into(),
        retry_key: retry_key.into(),
        retry_eligible: target_hat.is_some(),
        non_retryable_reason: if target_hat.is_some() {
            None
        } else {
            Some(NonRetryableReason::UnknownHat)
        },
        target_hat,
        // 2026-06-16-001 U3: `rejection_with_key` is the legacy
        // constructor used by tests and a few ad-hoc paths. The
        // freshness filter falls back to the rejection creation
        // time when these fields are None, so omitting them here
        // is safe for backwards compatibility.
        original_event_id: None,
        original_ts: None,
    }
}

/// Build the payload for a `task.resume` event that re-dispatches the
/// source hat with the violation context.  Returns the JSON-serialised
/// payload string ready to be written to the events file.  The caller
/// is responsible for writing the line.
///
/// `original_trigger_payload` is the JSON-serialised payload of the
/// event that originally activated the source hat (typically
/// `work.ready` for an executor) so the resumed hat sees the same
/// context it saw on the first dispatch.
///
/// `wave_context` is `Some` when the rejection is wave-related (the
/// source hat emitted a `review.*` event tagged with a `wave_id`).
/// When present the payload gains `wave_id`, `wave_index`, and
/// `wave_total` fields so the resumed hat can re-derive the wave
/// context the runner injects for `review-synthesizer` (R1+R5).  The
/// `original_hat` field is always written when the rejection has a
/// `source_hat`, mirroring the resume event's `.target` field.
pub fn build_task_resume_payload(
    rejection: &Rejection,
    allowed_topics: &[String],
    required_fields: &[String],
    original_trigger_topic: Option<&str>,
    original_trigger_payload: Option<&str>,
    wave_context: Option<&WaveContextForResume>,
) -> String {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "stage".into(),
        serde_json::Value::String(rejection.stage.as_str().into()),
    );
    payload.insert(
        "topic".into(),
        serde_json::Value::String(rejection.topic.clone()),
    );
    payload.insert(
        "violation".into(),
        serde_json::Value::String(rejection.violation.clone()),
    );
    payload.insert(
        "allowed_topics".into(),
        serde_json::Value::Array(
            allowed_topics
                .iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect(),
        ),
    );
    payload.insert(
        "required_fields".into(),
        serde_json::Value::Array(
            required_fields
                .iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect(),
        ),
    );
    if let Some(t) = original_trigger_topic {
        payload.insert(
            "original_trigger_topic".into(),
            serde_json::Value::String(t.to_string()),
        );
    }
    if let Some(p) = original_trigger_payload {
        // Best-effort: embed as JSON value if it parses, else as string.
        let value = serde_json::from_str::<serde_json::Value>(p)
            .unwrap_or_else(|_| serde_json::Value::String(p.to_string()));
        payload.insert("original_trigger_payload".into(), value);
    }
    payload.insert(
        "retry_key".into(),
        serde_json::Value::String(rejection.retry_key.clone()),
    );
    if let Some(hat) = rejection.source_hat.as_deref() {
        payload.insert(
            "original_hat".into(),
            serde_json::Value::String(hat.to_string()),
        );
    }
    if let Some(wc) = wave_context {
        payload.insert(
            "wave_id".into(),
            serde_json::Value::String(wc.wave_id.clone()),
        );
        if let Some(idx) = wc.wave_index {
            payload.insert("wave_index".into(), serde_json::Value::Number(idx.into()));
        }
        if let Some(total) = wc.wave_total {
            payload.insert("wave_total".into(), serde_json::Value::Number(total.into()));
        }
    }
    serde_json::Value::Object(payload).to_string()
}

/// Minimal wave metadata carried into a `task.resume` payload by the
/// R5 policy/workflow rejection paths.  The fields mirror the wire
/// shape of `Event::wave_id` / `wave_index` / `wave_total` so the
/// resumed hat can recover the wave it was working on.
#[derive(Debug, Clone, Default)]
pub struct WaveContextForResume {
    pub wave_id: String,
    pub wave_index: Option<u32>,
    pub wave_total: Option<u32>,
}

impl WaveContextForResume {
    /// Build a resume-context from the source event.  Returns `None`
    /// when the source event carries no `wave_id` (the caller
    /// should fall back to the pre-R5 behaviour of an un-targeted
    /// payload).
    pub fn from_event(event: &ralph_proto::Event) -> Option<Self> {
        let wave_id = event.wave_id.clone()?;
        Some(Self {
            wave_id,
            wave_index: event.wave_index,
            wave_total: event.wave_total,
        })
    }

    /// Same as [`Self::from_event`] but accepts the wire shape
    /// used by the in-loop rejection paths
    /// (`crate::event_reader::Event`).  The two structs share the
    /// same `wave_id` / `wave_index` / `wave_total` fields.
    pub fn from_reader_event(event: &crate::event_reader::Event) -> Option<Self> {
        let wave_id = event.wave_id.clone()?;
        Some(Self {
            wave_id,
            wave_index: event.wave_index,
            wave_total: event.wave_total,
        })
    }
}

/// Compute the target hat for a rejection.  Prefers the explicit
/// `business_hat` (which captures Coordinator-mode display hat), then
/// falls back to `source_hat`, and finally returns `None` if neither
/// is present.
pub fn resolve_target_hat(business_hat: Option<&str>, source_hat: Option<&str>) -> Option<HatId> {
    business_hat
        .or(source_hat)
        .map(|s| HatId::new(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_origin::OriginCheck;
    use crate::execution_contract::{ExecutionContractFinding, ExecutionContractViolationKind};

    #[test]
    fn from_origin_unknown_hat_is_non_retryable() {
        let r = Rejection::from_origin(
            Some("ghost-hat".into()),
            "work.done".into(),
            "unknown hat rejected",
        );
        assert!(!r.retry_eligible);
        assert_eq!(r.non_retryable_reason, Some(NonRetryableReason::UnknownHat));
        assert_eq!(r.stage.as_str(), "origin");
        assert_eq!(r.source_hat.as_deref(), Some("ghost-hat"));
        assert_eq!(r.topic, "work.done");
        assert!(!r.should_publish_resume());
        assert!(
            r.retry_key
                .contains("origin:ghost-hat:work.done:unknown_hat")
        );
    }

    #[test]
    fn from_origin_out_of_scope_is_non_retryable() {
        // The hat is registered but the topic is off-graph.  We do not
        // auto-relax the publish boundary (R1), so the rejection is
        // non-retryable; the next iteration must escalate.
        let r = Rejection::from_origin(
            Some("review-coordinator".into()),
            "work.done".into(),
            "out-of-scope topic for declared hat",
        );
        assert!(!r.retry_eligible);
        assert_eq!(r.non_retryable_reason, Some(NonRetryableReason::OutOfScope));
        assert_eq!(r.target_hat, None);
    }

    #[test]
    fn from_execution_contract_with_business_hat_is_retryable() {
        let finding = ExecutionContractFinding {
            topic: "work.done".into(),
            kind: ExecutionContractViolationKind::MissingPayloadField {
                field: "plan_path".into(),
            },
            message: "missing plan_path".into(),
            source_hat: None,
        };
        let r = Rejection::from_execution_contract(
            &finding,
            Some("executor".into()),
            Some("executor".into()),
        );
        assert!(r.retry_eligible);
        assert!(r.non_retryable_reason.is_none());
        assert_eq!(r.target_hat.as_deref(), Some("executor"));
        assert_eq!(r.stage.as_str(), "execution_contract");
        assert!(
            r.retry_key
                .contains("execution_contract:executor:work.done:missing_field")
        );
        assert!(r.should_publish_resume());
    }

    #[test]
    fn from_execution_contract_without_hat_is_non_retryable() {
        let finding = ExecutionContractFinding {
            topic: "work.done".into(),
            kind: ExecutionContractViolationKind::MissingPayloadField {
                field: "plan_path".into(),
            },
            message: "missing plan_path".into(),
            source_hat: None,
        };
        let r = Rejection::from_execution_contract(&finding, None, None);
        assert!(!r.retry_eligible);
        assert_eq!(r.non_retryable_reason, Some(NonRetryableReason::UnknownHat));
    }

    #[test]
    fn rejection_from_origin_helper_ignores_accepted() {
        let accepted = OriginCheck::Accepted;
        assert!(rejection_from_origin(&accepted, None).is_none());
    }

    #[test]
    fn rejection_from_origin_helper_extracts_rejection() {
        let rejected = OriginCheck::Rejected {
            topic: "work.done".into(),
            hat: Some("executor".into()),
            reason: "out-of-scope topic for declared hat",
        };
        let r = rejection_from_origin(&rejected, None).unwrap();
        assert_eq!(r.topic, "work.done");
        assert_eq!(r.source_hat.as_deref(), Some("executor"));
    }

    #[test]
    fn build_task_resume_payload_includes_all_context() {
        let r = Rejection::from_execution_contract(
            &ExecutionContractFinding {
                topic: "work.done".into(),
                kind: ExecutionContractViolationKind::MissingPayloadField {
                    field: "plan_path".into(),
                },
                message: "missing plan_path".into(),
                source_hat: None,
            },
            Some("executor".into()),
            Some("executor".into()),
        );
        let payload_str = build_task_resume_payload(
            &r,
            &["work.done".into()],
            &["plan_path".into()],
            Some("work.ready"),
            Some("{\"task_id\":\"task-x\"}"),
            None,
        );
        let v: serde_json::Value = serde_json::from_str(&payload_str).unwrap();
        assert_eq!(v["stage"], "execution_contract");
        assert_eq!(v["topic"], "work.done");
        assert_eq!(v["allowed_topics"][0], "work.done");
        assert_eq!(v["required_fields"][0], "plan_path");
        assert_eq!(v["original_trigger_topic"], "work.ready");
        assert_eq!(v["original_trigger_payload"]["task_id"], "task-x");
        assert!(v["retry_key"].as_str().unwrap().contains("executor"));
    }

    #[test]
    fn build_task_resume_payload_includes_wave_context() {
        // R5: when a wave event is policy-rejected, the resume
        // payload must carry `wave_id` / `wave_index` / `wave_total`
        // so the resumed hat can recover the wave context.
        let r = Rejection::from_execution_contract(
            &ExecutionContractFinding {
                topic: "review.dimension.done".into(),
                kind: ExecutionContractViolationKind::MissingPayloadField {
                    field: "findings_file".into(),
                },
                message: "missing findings_file".into(),
                source_hat: None,
            },
            Some("dimension-reviewer".into()),
            Some("dimension-reviewer".into()),
        );
        let wc = WaveContextForResume {
            wave_id: "w-abc".into(),
            wave_index: Some(3),
            wave_total: Some(7),
        };
        let payload_str = build_task_resume_payload(
            &r,
            &["review.dimension.done".into()],
            &["findings_file".into()],
            None,
            None,
            Some(&wc),
        );
        let v: serde_json::Value = serde_json::from_str(&payload_str).unwrap();
        assert_eq!(v["wave_id"], "w-abc");
        assert_eq!(v["wave_index"], 3);
        assert_eq!(v["wave_total"], 7);
        assert_eq!(v["original_hat"], "dimension-reviewer");
    }

    #[test]
    fn resolve_target_hat_prefers_business_hat() {
        let id = resolve_target_hat(Some("executor"), Some("ralph")).unwrap();
        assert_eq!(id.as_str(), "executor");
    }

    #[test]
    fn resolve_target_hat_falls_back_to_source() {
        let id = resolve_target_hat(None, Some("ralph")).unwrap();
        assert_eq!(id.as_str(), "ralph");
    }

    #[test]
    fn resolve_target_hat_returns_none_when_unknown() {
        assert!(resolve_target_hat(None, None).is_none());
    }

    #[test]
    fn same_violation_class_collapses_retry_key() {
        // R3: a MissingPayloadField on plan_path and a MissingPayloadField
        // on task_id must share the same retry key, so the runner can
        // detect "this hat has been failing the same kind of contract
        // check" without conflating with type-mismatch errors.
        let r1 = Rejection::from_execution_contract(
            &ExecutionContractFinding {
                topic: "work.done".into(),
                kind: ExecutionContractViolationKind::MissingPayloadField {
                    field: "plan_path".into(),
                },
                message: "missing plan_path".into(),
                source_hat: None,
            },
            Some("executor".into()),
            Some("executor".into()),
        );
        let r2 = Rejection::from_execution_contract(
            &ExecutionContractFinding {
                topic: "work.done".into(),
                kind: ExecutionContractViolationKind::MissingPayloadField {
                    field: "task_id".into(),
                },
                message: "missing task_id".into(),
                source_hat: None,
            },
            Some("executor".into()),
            Some("executor".into()),
        );
        assert_eq!(r1.retry_key, r2.retry_key);
    }

    #[test]
    fn from_topic_format_is_non_retryable() {
        let r = Rejection::from_topic_format(
            Some("executor".into()),
            "REVIEW_COMPLETE".into(),
            &["work.done".into(), "review.passed".into()],
        );
        assert!(!r.retry_eligible);
        assert_eq!(
            r.non_retryable_reason,
            Some(NonRetryableReason::InvalidTopicFormat)
        );
        assert_eq!(r.stage.as_str(), "policy");
        assert_eq!(r.source_hat.as_deref(), Some("executor"));
        assert_eq!(r.topic, "REVIEW_COMPLETE");
        assert!(!r.should_publish_resume());
        assert!(r.retry_key.contains("invalid_topic_format"));
    }

    #[test]
    fn from_topic_format_with_unknown_hat() {
        let r = Rejection::from_topic_format(None, "BAD_TOPIC".into(), &["work.done".into()]);
        assert!(!r.retry_eligible);
        assert_eq!(r.source_hat, None);
        assert_eq!(r.topic, "BAD_TOPIC");
        assert!(
            r.retry_key
                .contains("unknown:BAD_TOPIC:invalid_topic_format")
        );
    }
}
