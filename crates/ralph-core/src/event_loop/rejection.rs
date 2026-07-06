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
use crate::preset::engine::gates::RejectionKind;
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
    /// 2026-06-17-004 U3 (R4+R5): synthesised by the missing-event
    /// hard gate (`hard_gate::inject_missing_event_hard_gate_guidance`).
    /// The agent did not emit any event on its publish obligation;
    /// the gate injects a `task.resume` so the hat gets another
    /// chance.  The `stage` value in the resume payload is
    /// `"missing_event"` so the drift detector's field-completeness
    /// metric counts these as a recognisable rejection class
    /// (rather than collapsing them into the generic `policy` or
    /// `execution_contract` buckets).
    MissingEvent,
    /// 2026-06-17-004 U4 (R1): synthesised by the claim-but-no-write
    /// hard gate (`hard_gate::inject_hard_gate_guidance_with_triggers`).
    /// The agent's output mentioned `ralph emit` but no event was
    /// actually written to the events file.  The gate injects a
    /// `task.resume` with the original trigger topic + payload
    /// embedded so the next activation lands on the right
    /// `review.dimension`.  The `stage` value in the resume
    /// payload is `"emit_claimed_but_not_written"` so the drift
    /// detector can distinguish "I forgot to emit" from "I tried
    /// to emit but the run fell off the rails" (different root
    /// causes, different recovery shapes).
    EmitClaimedButNotWritten,
}

impl RejectionStage {
    /// Short, stable string used inside the retry key.
    pub fn as_str(&self) -> &'static str {
        match self {
            RejectionStage::Origin => "origin",
            RejectionStage::Policy => "policy",
            RejectionStage::ExecutionContract => "execution_contract",
            RejectionStage::PayloadContract => "payload_contract",
            RejectionStage::MissingEvent => "missing_event",
            RejectionStage::EmitClaimedButNotWritten => "emit_claimed_but_not_written",
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
    /// 2026-06-23 fix plan U5 (CB-2): typed `RejectionKind` so the
    /// `task.resume` consumer can dispatch on kind rather than
    /// substring-matching the free-form `violation` string. None
    /// when the source layer predates the typed-kinds plumbing
    /// (e.g. topic-format / payload-contract rejections) — callers
    /// that need a string fallback MUST serialise `violation`.
    /// Marked `#[non_exhaustive]` is NOT applied (struct is public
    /// and consumers read `violation` directly); instead we use
    /// `Option<RejectionKind>` so legacy builders stay
    /// source-compatible (CLAUDE.md "backwards compatibility doesn't
    /// matter" but cargo's `pub` field addition would force every
    /// test struct-literal to update — Option<> keeps literals valid).
    /// `#[serde(default)]` so JSONL written before this field
    /// existed deserialises without error (deserialise-as-None).
    #[serde(default)]
    pub kind: Option<crate::preset::engine::gates::RejectionKind>,
    /// U4 of plan 2026-07-05-005 (fix-plan §R4 / §R9):
    /// `RecoveryDiagnosisEnvelope::hint` discriminator string
    /// for `DuplicateWorkDone` rejections. Carries the
    /// `DuplicateWorkDoneHint::as_hint_str()` value so
    /// `ralph diagnose --session latest` and the recovery JSONL
    /// can distinguish `DuplicateSameStep` from
    /// `DuplicateStallBypass` while `reason_code` stays the
    /// stable legacy literal `duplicate_work_done` (per KTD-3).
    /// `#[serde(default)]` so JSONL written before this field
    /// existed deserialises without error.
    #[serde(default)]
    pub duplicate_work_done_hint: Option<crate::event_policy::DuplicateWorkDoneHint>,
    /// U5 of plan 2026-07-05-005 (R8): `work.ready` dedup hit
    /// counter surfaced on recovery payloads for storm detection.
    #[serde(default)]
    pub seen_count: Option<u32>,
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
            // 2026-06-23 fix plan U5 (CB-2): topic-format predates
            // typed-kind plumbing — keep None.
            kind: None,
            duplicate_work_done_hint: None,
            seen_count: None,
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
            // 2026-06-23 fix plan U5 (CB-2): origin-guard predates
            // typed-kind plumbing — keep None.
            kind: None,
            duplicate_work_done_hint: None,
            seen_count: None,
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
            // 2026-06-23 fix plan U5 (CB-2): execution-contract
            // predates typed-kind plumbing — keep None.
            kind: None,
            duplicate_work_done_hint: None,
            seen_count: None,
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

/// Map a `Rejection`'s free-form violation string to a stable short
/// reason code suitable for the `task.resume` payload's `reason`
/// field.  Schema validators (e.g. `ce-executor-serial.yml`) require
/// `reason` to be a non-empty string; this helper produces codes
/// that the drift detector can count as "field present and
/// well-typed".  Mirrors [`violation_class`] but is `pub` so
/// callers (e.g. the audit gate at `event_loop/mod.rs`) can
/// reason about the same vocabulary the payload uses.
pub fn extract_reason_code(violation: &str) -> &'static str {
    violation_class(violation)
}

/// Return the recovery directive IDs that should be injected into the
/// agent prompt when a `task.resume` event with this `kind` is
/// dispatched.  The directive list is intentionally small and stable;
/// unknown kinds map to an empty list.  The `target_hat` is accepted for
/// forward compatibility but is intentionally not used as a gate — these
/// directives apply to whichever hat is being resumed.
///
/// See plan 2026-06-28-003 (ralph-runtime-recovery) for the mapping.
pub fn recovery_directives_for_kind(kind: &str, _target_hat: &str) -> Vec<String> {
    match kind {
        "missing_event_gate" => vec!["RD-EXECUTOR-RESEND-LIMIT".to_string()],
        "stall_no_events" | "stall_recovery" => vec!["RD-STALL-DETECT-AND-YIELD".to_string()],
        "execution_contract:TaskWrongLoop" | "task_wrong_loop" => {
            vec!["RD-TASK-ID-MUST-BE-LOOP-SCOPED".to_string()]
        }
        "recovery_exhausted" => vec!["RD-PLAN-BLOCKED-ON-RECOVERY-EXHAUSTED".to_string()],
        _ => Vec::new(),
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
        // 2026-06-23 fix plan U5 (CB-2): legacy helper predates
        // typed-kind plumbing — keep None so payload falls back
        // to violation-derived reason.
        kind: None,
        duplicate_work_done_hint: None,
        seen_count: None,
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
    // U2 (2026-06-17-003 plan): schema-required `reason` and
    // `target_hat` fields.  These were previously missing from the
    // payload, which caused the drift detector to report
    // `field_completeness=0%` for the `task.resume` topic.  Both
    // fields are top-level strings so the preset schema validator
    // (e.g. `ce-executor-serial.yml`) sees them as present.
    payload.insert(
        "reason".into(),
        serde_json::Value::String(extract_reason_code(&rejection.violation).to_string()),
    );
    // 2026-06-23 fix plan U5 (CB-2): typed `kind` field for the
    // `task.resume` consumer dispatch (plan 2026-06-23-004 U4).
    // When `rejection.kind` is Some, surface the kind's
    // `reason_code()` string as the typed SSOT; when None (legacy
    // paths: topic-format, payload-contract), fall back to the
    // `reason` string (which mirrors `violation` substring). This
    // mirrors `gate::reject_to_task_resume` behaviour so all
    // `task.resume` payloads in the system carry the typed kind.
    let kind_value = rejection
        .kind
        .map(|k| k.reason_code().to_string())
        .unwrap_or_else(|| extract_reason_code(&rejection.violation).to_string());
    payload.insert("kind".into(), serde_json::Value::String(kind_value.clone()));
    // 2026-06-28-003: surface recovery directives so the runner can
    // inject targeted behaviour guidance into the prompt on the next
    // iteration. Empty list is the default and is skipped by the
    // injector.
    // U4 of plan 2026-07-05-005 (fix-plan §R4): surface
    // `DuplicateWorkDoneHint` discriminator so the recovery JSONL
    // carries the variant distinction. The hint string travels
    // alongside the stable `kind` field; post-mortem tooling and
    // `ralph diagnose` can read it without parsing `reason_code`.
    if let Some(ref dup_hint) = rejection.duplicate_work_done_hint {
        payload.insert(
            "hint".into(),
            serde_json::Value::String(dup_hint.as_hint_str().to_string()),
        );
    }
    if let Some(seen_count) = rejection.seen_count {
        payload.insert(
            "seen_count".into(),
            serde_json::Value::Number(seen_count.into()),
        );
    }
    let resolved_target_hat = rejection
        .target_hat
        .as_deref()
        .or(rejection.source_hat.as_deref())
        .or(rejection.business_hat.as_deref())
        .unwrap_or("ralph");
    payload.insert(
        "recovery_directives".into(),
        serde_json::Value::Array(
            recovery_directives_for_kind(&kind_value, resolved_target_hat)
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    // `target_hat` resolution: explicit `target_hat` first, then
    // `source_hat` (which is what `resolve_target_hat` falls back
    // to), then `business_hat`.  Mirrors the existing helper
    // `resolve_target_hat` so the values are consistent.
    // (resolved_target_hat already computed above for recovery directives.)
    if let Some(hat) = rejection
        .target_hat
        .as_deref()
        .or(rejection.source_hat.as_deref())
        .or(rejection.business_hat.as_deref())
    {
        payload.insert(
            "target_hat".into(),
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

/// Returns `true` when the JSON payload string contains both
/// `reason` and `target_hat` as string fields.  Used by all
/// orchestrator-injected `task.resume` paths to fail-closed when
/// the schema-required fields are missing — the drift detector
/// would otherwise report `0%` field completeness for the
/// `task.resume` topic.
///
/// `payload` is the JSON-serialised payload (the value passed
/// to `Event::new("task.resume", payload)`).  Returns `false`
/// when the payload is not a valid JSON object, or when either
/// of the two fields is absent or not a string.
pub fn task_resume_payload_has_required_fields(payload: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return false;
    };
    let Some(obj) = value.as_object() else {
        return false;
    };
    let reason_ok = obj
        .get("reason")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    let target_hat_ok = obj
        .get("target_hat")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    reason_ok && target_hat_ok
}

/// Wrap a free-form `task.resume` payload in a JSON object that
/// carries the schema-required `reason` and `target_hat` fields.
/// Used by orchestrator-injected `task.resume` paths (completion
/// rejection, hard fallback, handoff escalation, etc.) that
/// historically shipped only a free-form message string.  Without
/// the wrap, the drift detector would flag the injected event
/// as `field_completeness=0%`.
///
/// `reason_hint` is a free-form text used to derive a stable
/// `reason` code (see [`extract_reason_code`]).  When
/// `target_hat` is `None` and no default is provided, the
/// function falls back to `"ralph"`.
pub fn enrich_task_resume_payload(
    free_form_message: &str,
    reason_hint: &str,
    target_hat: Option<&str>,
    kind: Option<RejectionKind>,
) -> String {
    enrich_task_resume_payload_with_stage(free_form_message, reason_hint, target_hat, None, kind)
}

/// 2026-06-17-004 U3 (R4+R5): extend `enrich_task_resume_payload`
/// with an explicit `stage` field on the JSON payload.  When the
/// caller passes `Some(stage)`, the produced JSON includes a
/// top-level `stage` key whose value is the `RejectionStage::as_str()`
/// of the supplied variant (e.g. `"missing_event"` for the
/// missing-event hard gate).  When the caller passes `None`, the
/// legacy behaviour is preserved (no `stage` field) so existing
/// callers that derive `stage` from the rejection remain
/// unchanged.
///
/// The function is the single entry point for synthesising
/// `task.resume` JSON in the orchestrator (see
/// `hard_gate::inject_missing_event_hard_gate_guidance` and
/// `hard_gate::inject_hard_gate_guidance`).
pub fn enrich_task_resume_payload_with_stage(
    free_form_message: &str,
    reason_hint: &str,
    target_hat: Option<&str>,
    stage: Option<RejectionStage>,
    kind: Option<RejectionKind>,
) -> String {
    enrich_task_resume_payload_full(free_form_message, reason_hint, target_hat, stage, kind, &[])
}

/// 2026-06-28-002 U3: full-control variant of
/// `enrich_task_resume_payload_with_stage` that lets the caller
/// stamp `allowed_topics` onto the payload. The fallback stall
/// injection path and the drift engine's hard-recovery publishing
/// path both use this variant to surface the target hat's
/// `publishes` list, so the resumed agent knows which topics are
/// in scope and the `isolated_publish_allowed` scope check sees
/// the same list.
pub fn enrich_task_resume_payload_full(
    free_form_message: &str,
    reason_hint: &str,
    target_hat: Option<&str>,
    stage: Option<RejectionStage>,
    kind: Option<RejectionKind>,
    allowed_topics: &[String],
) -> String {
    let reason_code = extract_reason_code(reason_hint);
    let target_hat_value = target_hat
        .filter(|h| !h.is_empty())
        .unwrap_or("ralph")
        .to_string();
    let mut obj = serde_json::json!({
        "reason": reason_code,
        "target_hat": target_hat_value,
        "message": free_form_message,
    });
    if let Some(stage_value) = stage {
        if let serde_json::Value::Object(ref mut map) = obj {
            map.insert(
                "stage".into(),
                serde_json::Value::String(stage_value.as_str().into()),
            );
        }
    }
    // 2026-06-23-005 U1 (R1+R2): typed kind SSOT.
    let kind_value_string = kind
        .map(|k| k.reason_code().to_string())
        .unwrap_or_else(|| reason_code.to_string());
    if let serde_json::Value::Object(ref mut map) = obj {
        map.insert(
            "kind".into(),
            serde_json::Value::String(kind_value_string.clone()),
        );
    }
    // 2026-06-28-003: inject recovery directive IDs when the kind and
    // target_hat match a known runtime-recovery pattern.
    if let serde_json::Value::Object(ref mut map) = obj {
        map.insert(
            "recovery_directives".into(),
            serde_json::Value::Array(
                recovery_directives_for_kind(&kind_value_string, &target_hat_value)
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    // 2026-06-28-002 U3: surface the target hat's allowed
    // publish topics. The legacy path leaves this empty so the
    // existing `task.resume` consumers see no change.
    if !allowed_topics.is_empty()
        && let serde_json::Value::Object(ref mut map) = obj
    {
        map.insert(
            "allowed_topics".into(),
            serde_json::Value::Array(
                allowed_topics
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    obj.to_string()
}

/// U1 (plan 2026-06-23-004): typed 拒绝计数器消费侧。
///
/// 把 round-2 已落地的 `consecutive_lint_rejections_by_kind` 接上消费侧,
/// 按 KTD-1 表阶梯触发 typed 升级事件。纯函数 — 输入 `(kind, count)`,
/// 输出 `Option<EscalationAction>`,无副作用,易测。
///
/// ## KTD-1 阈值表
///
/// | RejectionKind    | threshold | action         |
/// |------------------|-----------|----------------|
/// | MissingEventGate | 1         | DriftFinding   |
/// | StallNoEvents    | 2         | DriftFinding   |
/// | StallNoEvents    | 3         | PlanBlocked    |
/// | ContractViolation| 1         | DriftFinding   |
///
/// 其他 kind 不在升级链中(typed 计数器仍记录,但消费侧不动作)。
/// 返回 `None` 表示该次拒绝无需升级(尚未达到阶梯)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationAction {
    /// Emit typed `drift_finding` 事件,记录 kind × count。
    DriftFinding {
        kind: crate::preset::engine::gates::RejectionKind,
        count: u32,
    },
    /// Emit `loop.circuit_breaker_trip` 事件(只对 filename_mismatch)。
    CircuitBreakerTrip {
        kind: crate::preset::engine::gates::RejectionKind,
        count: u32,
    },
    /// Emit `plan.blocked` 事件(强制人工介入,只对 structure/illegal_emit)。
    PlanBlocked {
        kind: crate::preset::engine::gates::RejectionKind,
        count: u32,
    },
}

pub struct RejectionEscalator;

impl RejectionEscalator {
    /// 纯函数:输入 `(kind, count)`,按 KTD-1 表返回应触发的升级动作或 None。
    pub fn check(
        kind: crate::preset::engine::gates::RejectionKind,
        count: u32,
    ) -> Option<EscalationAction> {
        use crate::preset::engine::gates::RejectionKind as K;
        match kind {
            // 2026-06-23-005 U2 (R3+KTD-2): three new typed kinds from
            // task.resume injection paths (hard_gate / stall_recovery /
            // contract).
            K::MissingEventGate => match count {
                1.. => Some(EscalationAction::DriftFinding { kind, count }),
                _ => None,
            },
            K::StallNoEvents => match count {
                2..=2 => Some(EscalationAction::DriftFinding { kind, count }),
                3.. => Some(EscalationAction::PlanBlocked { kind, count }),
                _ => None,
            },
            K::ContractViolation => match count {
                1.. => Some(EscalationAction::DriftFinding { kind, count }),
                _ => None,
            },
            // 其他 kind 不在升级链中(typed 计数器仍记录,但消费侧不动作)。
            // #[non_exhaustive] 强制未来新增 kind 时必须显式列在这里或 _ 兜底。
            _ => None,
        }
    }
}

/// U4 (plan 2026-06-23-004, anti-pattern 4): coordinator 对 task.resume
/// 的 typed kind dispatch。
///
/// 接收 `task.resume` 携带的 `RejectionKind`,按 kind 路由到对应修复策略:
/// - `MissingEventGate` / `StallNoEvents` / `PersistentLoopActive` /
///   `OpenTasksBlocking` / `MissingField` / `TopicOwnership` / ... → 重新 emit
///   work.ready(让源 hat 再试一次)
/// - `ContractViolation` → 修复 payload schema 后重发
///
/// 死信兜底:连续 N=3 次同 kind task.resume 仍未消费 → emit `plan.blocked`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatorAction {
    /// 重新 emit work.ready(SSOT 文件名派生)
    ReEmitWorkReady,
    /// 修复 payload schema 后重发
    FixPayloadSchema,
    /// 改 emit target 后重发
    FixEmitTarget,
    /// 死信:连续 N 次同 kind 未消费,emit plan.blocked
    PlanBlocked {
        kind: crate::preset::engine::gates::RejectionKind,
        count: u32,
    },
}

/// U4 (plan 2026-06-23-004): 死信阈值——同一 kind 累计 N 次 task.resume
/// 仍未消费 → emit plan.blocked。
pub const COORDINATOR_DEAD_LETTER_THRESHOLD: u32 = 3;

pub struct CoordinatorDispatcher;

impl CoordinatorDispatcher {
    /// 纯函数:输入 `(kind, consecutive_count)` 按 KTD-4 dispatch。
    /// `consecutive_count` 由调用方统计(同一 kind 累计 task.resume 次数)。
    pub fn dispatch(
        kind: crate::preset::engine::gates::RejectionKind,
        consecutive_count: u32,
    ) -> CoordinatorAction {
        use crate::preset::engine::gates::RejectionKind as K;
        // 死信兜底先于具体 dispatch
        if consecutive_count >= COORDINATOR_DEAD_LETTER_THRESHOLD {
            return CoordinatorAction::PlanBlocked {
                kind,
                count: consecutive_count,
            };
        }
        match kind {
            // 2026-06-23-005 U2 (R3+KTD-2): three new typed kinds from
            // task.resume injection paths.
            //
            // - MissingEventGate → ReEmitWorkReady (missing-event hard gate
            //   synthesises task.resume; the recovery action is to re-emit
            //   the original work.ready so the hat gets another chance).
            // - StallNoEvents → ReEmitWorkReady (stall_recovery path; re-emit
            //   work.ready to break the no-events stall).
            // - ContractViolation → FixPayloadSchema (payload contract
            //   rejected the emit; the agent must rewrite the payload).
            K::MissingEventGate => CoordinatorAction::ReEmitWorkReady,
            K::StallNoEvents => CoordinatorAction::ReEmitWorkReady,
            K::ContractViolation => CoordinatorAction::FixPayloadSchema,
            // 2026-06-23-005 F2: completion-signal rejection paths
            // (persistent mode / open tasks blocking completion).
            // Both routes are re-emit-work-ready — the recovery
            // action is to nudge the hat (typically the
            // coordinator) so it can either continue (persistent
            // mode) or close/reopen the blocking task (open tasks).
            K::PersistentLoopActive => CoordinatorAction::ReEmitWorkReady,
            K::OpenTasksBlocking => CoordinatorAction::ReEmitWorkReady,
            // 其他 kind 不在 typed dispatch 范围,走 default 修复策略
            // (重发原 task.resume,等待下一个信号)。
            // `#[non_exhaustive]` 强制未来新增 kind 时必须显式列在这里或保留此 _ 兜底。
            _ => CoordinatorAction::ReEmitWorkReady,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_origin::OriginCheck;
    use crate::execution_contract::{ExecutionContractFinding, ExecutionContractViolationKind};

    // 2026-06-28-002 U3: `enrich_task_resume_payload_full`
    // stamps the target hat's publishes onto the payload as
    // `allowed_topics` so the resumed agent knows the legal emit
    // surface and the isolated scope check sees the same list.
    #[test]
    fn u3_enrich_full_stamps_allowed_topics() {
        let allowed = vec![
            "work.ready".to_string(),
            "plan.complete".to_string(),
            "plan.blocked".to_string(),
        ];
        let payload = enrich_task_resume_payload_full(
            "RECOVERY: previous iteration by hat `coordinator` did not publish an event.",
            "stall_no_events",
            Some("coordinator"),
            None,
            Some(crate::preset::engine::gates::RejectionKind::StallNoEvents),
            &allowed,
        );
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        let topics = v["allowed_topics"]
            .as_array()
            .expect("allowed_topics must be present")
            .iter()
            .filter_map(|s| s.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            topics,
            vec!["work.ready", "plan.complete", "plan.blocked"],
            "coordinator's allowed_topics must equal its publishes"
        );
        // Sanity: coordinator must NOT see work.start (executor-only).
        assert!(
            !topics.contains(&"work.start"),
            "coordinator must not be allowed to emit work.start, got: {topics:?}"
        );
    }

    #[test]
    fn u3_enrich_full_omits_allowed_topics_when_empty() {
        // Backward compatibility: when no allowed_topics is
        // supplied the JSON envelope MUST NOT carry the field, so
        // legacy readers that look for it see no change.
        let payload = enrich_task_resume_payload_full(
            "RECOVERY",
            "stall_no_events",
            Some("ralph"),
            None,
            Some(crate::preset::engine::gates::RejectionKind::StallNoEvents),
            &[],
        );
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert!(
            v.get("allowed_topics").is_none(),
            "empty allowed_topics must not be serialised, got: {payload}"
        );
    }

    // U1 (plan 2026-06-23-004): 12 个 typed rejection escalation case
    // (4 kind × 3 threshold band),SSOT 化阶梯触发链。
    mod rejection_escalation_unit {
        use crate::preset::engine::gates::RejectionKind;

        #[test]
        fn escalation_thresholds_match_ktd_1() {
            // MissingEventGate: 1 → drift_finding (typed hard-gate path).
            assert_eq!(
                super::super::RejectionEscalator::check(RejectionKind::MissingEventGate, 0),
                None
            );
            assert!(matches!(
                super::super::RejectionEscalator::check(RejectionKind::MissingEventGate, 1),
                Some(super::super::EscalationAction::DriftFinding { .. })
            ));
        }

        #[test]
        fn stall_no_events_triggers_at_2_and_plan_blocks_at_3() {
            // StallNoEvents: 1 → none, 2 → drift_finding, 3+ → plan.blocked.
            assert_eq!(
                super::super::RejectionEscalator::check(RejectionKind::StallNoEvents, 1),
                None
            );
            assert!(matches!(
                super::super::RejectionEscalator::check(RejectionKind::StallNoEvents, 2),
                Some(super::super::EscalationAction::DriftFinding { .. })
            ));
            assert!(matches!(
                super::super::RejectionEscalator::check(RejectionKind::StallNoEvents, 3),
                Some(super::super::EscalationAction::PlanBlocked { .. })
            ));
        }

        #[test]
        fn contract_violation_triggers_at_1() {
            // ContractViolation: 1+ → drift_finding.
            assert_eq!(
                super::super::RejectionEscalator::check(RejectionKind::ContractViolation, 0),
                None
            );
            assert!(matches!(
                super::super::RejectionEscalator::check(RejectionKind::ContractViolation, 1),
                Some(super::super::EscalationAction::DriftFinding { .. })
            ));
            assert!(matches!(
                super::super::RejectionEscalator::check(RejectionKind::ContractViolation, 4),
                Some(super::super::EscalationAction::DriftFinding { .. })
            ));
        }

        #[test]
        fn kind_isolation_does_not_cross_pollute() {
            // MissingEventGate 累计 5 次不影响 StallNoEvents counter 0.
            assert_eq!(
                super::super::RejectionEscalator::check(RejectionKind::StallNoEvents, 0),
                None
            );
            // 不在升级链中的 kind(MissingField/TopicOwnership/...) 任何 count 都不触发.
            assert_eq!(
                super::super::RejectionEscalator::check(RejectionKind::MissingField, 5),
                None
            );
            assert_eq!(
                super::super::RejectionEscalator::check(RejectionKind::TopicOwnership, 5),
                None
            );
        }
    }

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
        // U2 (2026-06-17-003 plan): schema-required fields.
        assert_eq!(v["reason"], "missing_field");
        assert_eq!(v["target_hat"], "executor");
        // 2026-06-23 fix plan U5 (CB-2): payload MUST carry typed
        // `kind` field (falls back to `reason` when rejection.kind
        // is None — this path predates typed plumbing).
        assert!(
            v["kind"].as_str().is_some(),
            "task.resume payload MUST carry `kind` field; got {v:?}"
        );
        // Legacy paths (None) → kind == reason.
        assert_eq!(v["kind"], "missing_field");
    }

    /// 2026-06-23 fix plan U5 (CB-2): when the rejection carries
    /// a typed `RejectionKind`, the payload's `kind` field MUST
    /// surface the kind's `reason_code()` (typed SSOT). This is
    /// the consumer-dispatchable path (plan 2026-06-23-004 U4).
    #[test]
    fn build_task_resume_payload_surfaces_typed_kind() {
        let mut r = Rejection::from_execution_contract(
            &ExecutionContractFinding {
                topic: "work.ready".into(),
                kind: ExecutionContractViolationKind::MissingPayloadField {
                    field: "task_id".into(),
                },
                message: "missing task_id".into(),
                source_hat: Some("coordinator".into()),
            },
            Some("coordinator".into()),
            Some("coordinator".into()),
        );
        // Inject typed kind (in real code the gate Reject arm in
        // event_loop passes the typed kind here, CB-2 wires it).
        r.kind = Some(crate::preset::engine::gates::RejectionKind::MissingField);
        let payload_str = build_task_resume_payload(&r, &[], &[], None, None, None);
        let v: serde_json::Value = serde_json::from_str(&payload_str).unwrap();
        assert_eq!(
            v["kind"], "missing_field",
            "typed kind's reason_code() MUST surface as payload `kind` field"
        );
        // And the legacy `reason` field stays the same so existing
        // drift-detector greps continue to match.
        assert_eq!(v["reason"], "missing_field");
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
        // U2 (2026-06-17-003 plan): schema-required fields.
        assert_eq!(v["reason"], "missing_field");
        assert_eq!(v["target_hat"], "dimension-reviewer");
    }

    /// U2 (2026-06-17-003 plan): the `reason` field must be derived
    /// from the rejection's violation string, and `target_hat` must
    /// fall back through `source_hat` → `business_hat` when the
    /// explicit `target_hat` is `None`.  Drift detector field
    /// completeness was 0% before this fix.
    #[test]
    fn build_task_resume_payload_includes_reason_and_target_hat() {
        // Case 1: explicit target_hat wins.
        let r1 = Rejection {
            stage: RejectionStage::Policy,
            source_hat: Some("executor".into()),
            business_hat: Some("executor".into()),
            topic: "work.done".into(),
            violation: "TypeMismatch: expected bool, got string".into(),
            retry_key: "policy:executor:work.done:type_mismatch".into(),
            retry_eligible: true,
            non_retryable_reason: None,
            target_hat: Some("explicit-target".into()),
            original_event_id: None,
            original_ts: None,
            kind: None,
            duplicate_work_done_hint: None,
            seen_count: None,
        };
        let payload1 = build_task_resume_payload(&r1, &[], &[], None, None, None);
        let v1: serde_json::Value = serde_json::from_str(&payload1).unwrap();
        assert_eq!(v1["reason"], "type_mismatch");
        assert_eq!(v1["target_hat"], "explicit-target");

        // Case 2: no explicit target_hat → fall back to source_hat.
        let r2 = Rejection {
            target_hat: None,
            source_hat: Some("review-coordinator".into()),
            business_hat: Some("review-coordinator".into()),
            topic: "review.dimension.ready".into(),
            violation: "out-of-scope topic for declared hat".into(),
            ..r1.clone()
        };
        let payload2 = build_task_resume_payload(&r2, &[], &[], None, None, None);
        let v2: serde_json::Value = serde_json::from_str(&payload2).unwrap();
        assert_eq!(v2["reason"], "out_of_scope");
        assert_eq!(v2["target_hat"], "review-coordinator");

        // Case 3: neither target_hat nor source_hat → business_hat.
        let r3 = Rejection {
            target_hat: None,
            source_hat: None,
            business_hat: Some("business-fallback".into()),
            topic: "work.done".into(),
            violation: "task not open".into(),
            ..r1.clone()
        };
        let payload3 = build_task_resume_payload(&r3, &[], &[], None, None, None);
        let v3: serde_json::Value = serde_json::from_str(&payload3).unwrap();
        assert_eq!(v3["reason"], "task_state");
        assert_eq!(v3["target_hat"], "business-fallback");

        // Case 4: reason falls back to "other" when no pattern matches.
        let r4 = Rejection {
            target_hat: Some("executor".into()),
            source_hat: Some("executor".into()),
            business_hat: Some("executor".into()),
            topic: "work.done".into(),
            violation: "completely unrecognised violation message".into(),
            ..r1.clone()
        };
        let payload4 = build_task_resume_payload(&r4, &[], &[], None, None, None);
        let v4: serde_json::Value = serde_json::from_str(&payload4).unwrap();
        assert_eq!(v4["reason"], "other");
        assert_eq!(v4["target_hat"], "executor");
    }

    /// U4 of plan 2026-07-05-005 (fix-plan §R4): when a
    /// `DuplicateWorkDone` rejection carries a `duplicate_work_done_hint`,
    /// `build_task_resume_payload` surfaces the discriminator string on
    /// the recovery envelope's `hint` field so `ralph diagnose` and the
    /// recovery JSONL can distinguish `DuplicateSameStep` from
    /// `DuplicateStallBypass` without parsing `reason_code` (which
    /// stays the stable `duplicate_work_done` literal per KTD-3).
    #[test]
    fn u4_hint_carried_in_envelope() {
        use crate::event_policy::DuplicateWorkDoneHint;

        // Same-step hint travels as `duplicate_work_done_same_step`.
        let r_same = Rejection {
            stage: RejectionStage::Policy,
            source_hat: Some("executor".into()),
            business_hat: Some("executor".into()),
            topic: "work.done".into(),
            violation: "duplicate".into(),
            retry_key: "policy:executor:work.done:duplicate_work_done".into(),
            retry_eligible: true,
            non_retryable_reason: None,
            target_hat: Some("executor".into()),
            original_event_id: None,
            original_ts: None,
            kind: None,
            duplicate_work_done_hint: Some(DuplicateWorkDoneHint::DuplicateSameStep),
            seen_count: None,
        };
        let payload_same = build_task_resume_payload(&r_same, &[], &[], None, None, None);
        let v_same: serde_json::Value = serde_json::from_str(&payload_same).unwrap();
        assert_eq!(
            v_same["hint"], "duplicate_work_done_same_step",
            "U4: DuplicateSameStep hint must surface on the envelope"
        );

        // Stall-bypass hint travels as `duplicate_work_done_stall_bypass`.
        let r_stall = Rejection {
            duplicate_work_done_hint: Some(DuplicateWorkDoneHint::DuplicateStallBypass),
            ..r_same.clone()
        };
        let payload_stall = build_task_resume_payload(&r_stall, &[], &[], None, None, None);
        let v_stall: serde_json::Value = serde_json::from_str(&payload_stall).unwrap();
        assert_eq!(
            v_stall["hint"], "duplicate_work_done_stall_bypass",
            "U4: DuplicateStallBypass hint must surface on the envelope"
        );

        // Hint absent → payload has no `hint` field (legacy behaviour).
        let r_none = Rejection {
            duplicate_work_done_hint: None,
            seen_count: None,
            ..r_same.clone()
        };
        let payload_none = build_task_resume_payload(&r_none, &[], &[], None, None, None);
        let v_none: serde_json::Value = serde_json::from_str(&payload_none).unwrap();
        assert!(
            v_none.get("hint").is_none(),
            "U4: rejection without DuplicateWorkDone hint must omit the field, got: {v_none}"
        );
    }

    /// U2 (2026-06-17-003 plan): the gate helper that audit-logs
    /// `task.resume` payloads before publishing must return true
    /// for payloads built by `build_task_resume_payload` and
    /// false for anything that is missing either required field.
    #[test]
    fn task_resume_payload_has_required_fields_helper() {
        // Valid payload from build_task_resume_payload.
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
        let payload = build_task_resume_payload(&r, &[], &[], None, None, None);
        assert!(task_resume_payload_has_required_fields(&payload));

        // Missing reason → false.
        let bad1 = r#"{"target_hat":"executor","stage":"policy"}"#;
        assert!(!task_resume_payload_has_required_fields(bad1));

        // Missing target_hat → false.
        let bad2 = r#"{"reason":"missing_field","stage":"policy"}"#;
        assert!(!task_resume_payload_has_required_fields(bad2));

        // reason not a string → false.
        let bad3 = r#"{"reason":42,"target_hat":"executor"}"#;
        assert!(!task_resume_payload_has_required_fields(bad3));

        // target_hat not a string → false.
        let bad4 = r#"{"reason":"missing_field","target_hat":null}"#;
        assert!(!task_resume_payload_has_required_fields(bad4));

        // Empty string reason → false.
        let bad5 = r#"{"reason":"","target_hat":"executor"}"#;
        assert!(!task_resume_payload_has_required_fields(bad5));

        // Empty string target_hat → false.
        let bad6 = r#"{"reason":"missing_field","target_hat":""}"#;
        assert!(!task_resume_payload_has_required_fields(bad6));

        // Not valid JSON → false.
        assert!(!task_resume_payload_has_required_fields("not json at all"));
        assert!(!task_resume_payload_has_required_fields(""));

        // Not a JSON object → false.
        let array = r#"["reason","target_hat"]"#;
        assert!(!task_resume_payload_has_required_fields(array));
    }

    /// U2 (2026-06-17-003 plan): `enrich_task_resume_payload` wraps
    /// a free-form message in a JSON object with the
    /// schema-required `reason` and `target_hat` fields.  The
    /// output must satisfy `task_resume_payload_has_required_fields`.
    ///
    /// 2026-06-23-005 U1: also assert the new typed `kind` field
    /// when caller passes `Some(RejectionKind)`. Legacy callers
    /// pass `None` and get `kind == reason` (fallback SSOT).
    #[test]
    fn enrich_task_resume_payload_wraps_free_form() {
        // Explicit target_hat + reason hint that contains "missing" → missing_field.
        // Pass Some(RejectionKind::MissingField) → kind field equals "missing_field".
        let payload = enrich_task_resume_payload(
            "WORKFLOW_GUARD_REJECTED: out-of-order event 'work.done'",
            "missing plan_path",
            Some("executor"),
            Some(RejectionKind::MissingField),
        );
        assert!(task_resume_payload_has_required_fields(&payload));
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["reason"], "missing_field");
        assert_eq!(v["target_hat"], "executor");
        assert_eq!(v["kind"], "missing_field");
        assert_eq!(
            v["message"],
            "WORKFLOW_GUARD_REJECTED: out-of-order event 'work.done'"
        );

        // No target_hat → defaults to "ralph". Pass None for kind → fallback to reason.
        let payload2 = enrich_task_resume_payload("RECOVERY hint", "out-of-scope", None, None);
        assert!(task_resume_payload_has_required_fields(&payload2));
        let v2: serde_json::Value = serde_json::from_str(&payload2).unwrap();
        assert_eq!(v2["target_hat"], "ralph");
        assert_eq!(v2["reason"], "out_of_scope");
        assert_eq!(v2["kind"], "out_of_scope"); // fallback mirrors reason

        // Empty target_hat → also defaults to "ralph".
        let payload3 = enrich_task_resume_payload("RECOVERY hint", "out-of-scope", Some(""), None);
        let v3: serde_json::Value = serde_json::from_str(&payload3).unwrap();
        assert_eq!(v3["target_hat"], "ralph");

        // Reason hint that matches "type" → type_mismatch.
        let payload4 =
            enrich_task_resume_payload("bad", "TypeMismatch: expected bool", Some("h"), None);
        let v4: serde_json::Value = serde_json::from_str(&payload4).unwrap();
        assert_eq!(v4["reason"], "type_mismatch");
        assert_eq!(v4["kind"], "type_mismatch");
    }

    /// 2026-06-28-003: recovery directives are surfaced on task.resume
    /// payloads when the kind/target_hat pair matches a known pattern.
    #[test]
    fn recovery_directives_are_injected_for_known_patterns() {
        // missing_event_gate + executor → RD-EXECUTOR-RESEND-LIMIT
        let payload = enrich_task_resume_payload(
            "missing event",
            "hard_gate_missing_event",
            Some("executor"),
            Some(RejectionKind::MissingEventGate),
        );
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        let directives: Vec<String> = v["recovery_directives"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        assert_eq!(directives, vec!["RD-EXECUTOR-RESEND-LIMIT"]);

        // stall_no_events + executor → RD-STALL-DETECT-AND-YIELD
        let payload2 = enrich_task_resume_payload(
            "stall",
            "stall_no_events",
            Some("executor"),
            Some(RejectionKind::StallNoEvents),
        );
        let v2: serde_json::Value = serde_json::from_str(&payload2).unwrap();
        let directives2: Vec<String> = v2["recovery_directives"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        assert_eq!(directives2, vec!["RD-STALL-DETECT-AND-YIELD"]);

        // Non-executor target still gets directives (directives apply to
        // whichever hat is being resumed).
        let payload3 = enrich_task_resume_payload(
            "missing event",
            "hard_gate_missing_event",
            Some("reviewer"),
            Some(RejectionKind::MissingEventGate),
        );
        let v3: serde_json::Value = serde_json::from_str(&payload3).unwrap();
        let directives3: Vec<String> = v3["recovery_directives"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        assert_eq!(directives3, vec!["RD-EXECUTOR-RESEND-LIMIT"]);

        // Unknown kind → empty list
        let payload4 = enrich_task_resume_payload("x", "out-of-scope", Some("executor"), None);
        let v4: serde_json::Value = serde_json::from_str(&payload4).unwrap();
        assert!(v4["recovery_directives"].as_array().unwrap().is_empty());
    }

    #[test]
    fn recovery_directives_helper_handles_custom_kind_strings() {
        assert_eq!(
            recovery_directives_for_kind("execution_contract:TaskWrongLoop", "executor"),
            vec!["RD-TASK-ID-MUST-BE-LOOP-SCOPED".to_string()]
        );
        assert_eq!(
            recovery_directives_for_kind("recovery_exhausted", "executor"),
            vec!["RD-PLAN-BLOCKED-ON-RECOVERY-EXHAUSTED".to_string()]
        );
        assert_eq!(
            recovery_directives_for_kind("missing_event_gate", "reviewer"),
            vec!["RD-EXECUTOR-RESEND-LIMIT".to_string()]
        );
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

    // U4 (plan 2026-06-23-004, anti-pattern 4): coordinator dispatcher 测试。
    mod task_resume_consumer {
        use super::*;
        use crate::preset::engine::gates::RejectionKind;

        #[test]
        fn dispatch_routes_by_kind() {
            // AE4 (反模式 4): ralph emit task.resume 按 kind 路由。
            // 除 ContractViolation 走 FixPayloadSchema 外,其余默认 ReEmitWorkReady。
            assert_eq!(
                CoordinatorDispatcher::dispatch(RejectionKind::MissingField, 1),
                CoordinatorAction::ReEmitWorkReady
            );
            assert_eq!(
                CoordinatorDispatcher::dispatch(RejectionKind::TopicOwnership, 1),
                CoordinatorAction::ReEmitWorkReady
            );
            assert_eq!(
                CoordinatorDispatcher::dispatch(RejectionKind::ContractViolation, 1),
                CoordinatorAction::FixPayloadSchema
            );
        }

        #[test]
        fn dead_letter_kicks_in_at_3() {
            // 死信兜底:连续 3 次同 kind → emit plan.blocked
            let action = CoordinatorDispatcher::dispatch(
                RejectionKind::MissingField,
                COORDINATOR_DEAD_LETTER_THRESHOLD,
            );
            assert!(matches!(
                action,
                CoordinatorAction::PlanBlocked {
                    kind: RejectionKind::MissingField,
                    count: COORDINATOR_DEAD_LETTER_THRESHOLD,
                }
            ));
            // 4 次同样进入死信
            let action = CoordinatorDispatcher::dispatch(RejectionKind::TopicOwnership, 4);
            assert!(matches!(
                action,
                CoordinatorAction::PlanBlocked {
                    kind: RejectionKind::TopicOwnership,
                    count: 4,
                }
            ));
        }

        #[test]
        fn dispatch_does_not_cross_pollute_kinds() {
            // MissingField × 1 → ReEmitWorkReady
            assert_eq!(
                CoordinatorDispatcher::dispatch(RejectionKind::MissingField, 1),
                CoordinatorAction::ReEmitWorkReady
            );
            // ContractViolation 走独立的 FixPayloadSchema path
            assert_eq!(
                CoordinatorDispatcher::dispatch(RejectionKind::ContractViolation, 1),
                CoordinatorAction::FixPayloadSchema
            );
        }

        #[test]
        fn threshold_boundary_below_3_does_not_dead_letter() {
            // 1 / 2 次都不应触发死信,ContractViolation 走 FixPayloadSchema
            assert_eq!(
                CoordinatorDispatcher::dispatch(RejectionKind::ContractViolation, 1),
                CoordinatorAction::FixPayloadSchema
            );
            assert_eq!(
                CoordinatorDispatcher::dispatch(RejectionKind::ContractViolation, 2),
                CoordinatorAction::FixPayloadSchema
            );
        }

        /// 2026-06-23 fix plan P0 (CB-6): when the dispatcher
        /// returns PlanBlocked (count >= threshold), the
        /// event_loop must persist a `task_resume_dead_letter`
        /// entry to `.ralph/recovery.jsonl` so the operator
        /// can see the cumulative count even when stdout
        /// logs rotate. We test the envelope construction
        /// (the loop's dead-letter path is in
        /// `event_loop/mod.rs` and not unit-testable in
        /// isolation; the SSOT is here, the wiring is
        /// verified by the dispatch + persistence contract).
        #[test]
        fn dead_letter_envelope_carries_kind_and_count() {
            use crate::diagnosis::{
                DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
            };
            let kind = RejectionKind::MissingField;
            let count: u32 = 3;
            let envelope = RecoveryDiagnosisEnvelope::builder()
                .source(DiagnosisSource::LoopStale)
                .severity(DiagnosisSeverity::Error)
                .topic("work.ready")
                .reason_code("task_resume_dead_letter")
                .message(format!(
                    "coordinator dead-letter after {count} consecutive task.resume (kind={}); emitting plan.blocked",
                    kind.reason_code()
                ))
                .outcome(DiagnosisOutcome::Failed)
                .source_hat("coordinator")
                .safe_target(false)
                .build();
            // The reason_code is the new SSOT — distinct from
            // the per-iteration rejection kind so downstream tools
            // can filter the terminal state separately.
            assert_eq!(envelope.reason_code, "task_resume_dead_letter");
            assert!(envelope.message.contains("kind=missing_field"));
            assert!(envelope.message.contains("3 consecutive"));
            // Serialized form must include the kind/count
            // payload so operators reading recovery.jsonl can
            // see the cumulative count and trigger kind.
            let serialized = serde_json::to_value(&envelope).expect("serialize");
            assert_eq!(serialized["reason_code"], "task_resume_dead_letter");
            assert_eq!(serialized["source"], "loop_stale");
            assert_eq!(serialized["source_hat"], "coordinator");
        }
    }
}
