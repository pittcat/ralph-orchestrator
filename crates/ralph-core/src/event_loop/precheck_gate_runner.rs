//! 2026-07-02-004 plan milestone B (U6): failure-closure
//! runner for synthesized `precheck-<X>` gate hats.
//!
//! Contract (locked by U6):
//! 1. When a gate hat emits `<X>.rejected`, the runtime
//!    increments a per-`(loop_id, topic)` counter.
//! 2. While `count <= retry_budget`, the runner synthesizes a
//!    `task.resume` targeting the gate's `on_fail.target` hat.
//!    The resume payload carries the gate's
//!    `failed_checks` / `reason` so the target hat sees the
//!    reason in its next prompt.
//! 3. When the budget is exhausted, the runner emits the
//!    configured `on_exhausted` topic (default:
//!    `plan.blocked(reason=precheck_failed)`).
//! 4. When the gate emits `<X>` (pass), the counter resets to
//!    zero and no resume is injected.
//! 5. The counter is in-memory per loop (`HashMap<String,
//!    u32>` keyed by `"{loop_id}|{topic}"`) — the runtime
//!    rebuilds it on each process restart and a rejection in
//!    one loop never bleeds into another. This mirrors the
//!    StallRecovery / repair_budget pattern.
//!
//! Architectural note: this module is pure CPU only. The wiring
//! into the event loop lives in `event_loop::mod`.

use std::collections::HashMap;

/// Outcome of dispatching a gate rejection.  The event loop
/// reads this to decide whether to inject a resume, escalate to
/// `plan.blocked`, or no-op (LLM-emitted pass).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Gate emitted `<X>` (pass).  No further action; counter
    /// resets to zero on the next `record_pass`.
    Pass,
    /// Gate emitted `<X>.rejected` and the retry budget has
    /// not been exhausted.  The event loop should inject a
    /// `task.resume` targeting `target_hat` with the supplied
    /// payload so the next activation re-enters the upstream
    /// hat.
    Resume {
        target_hat: String,
        payload_json: String,
        new_count: u32,
    },
    /// Gate emitted `<X>.rejected` and the budget is exhausted.
    /// The event loop should emit `on_exhausted` (default:
    /// `plan.blocked(reason=precheck_failed)`).
    Exhausted { topic: String, reason: String },
}

/// Retry-counter registry.  Keyed by `"{loop_id}|{topic}"` so
/// the same gate in different loops is tracked independently
/// (the orchestrator can run several loops against the same
/// preset, e.g. the ce-executor-serial multi-loop pattern).
#[derive(Debug, Default, Clone)]
pub struct PrecheckRetryRegistry {
    /// `"{loop_id}|{topic}"` → consecutive-rejection count.
    counters: HashMap<String, u32>,
}

impl PrecheckRetryRegistry {
    /// Construct an empty registry.  The orchestrator builds one
    /// per `EventLoop` and threads it through gate-hat
    /// dispatches.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute the storage key for a (loop, topic) pair.
    pub fn key(loop_id: &str, topic: &str) -> String {
        format!("{loop_id}|{topic}")
    }

    /// Reset the counter for `key` to zero.  Called on every
    /// successful `<X>` pass so a long-running loop does not
    /// slowly accumulate stale counts.
    pub fn reset(&mut self, key: &str) {
        self.counters.insert(key.to_string(), 0);
    }

    /// Read the current count without mutating.  Test-only
    /// accessor; runtime paths should prefer
    /// [`Self::record_pass`] / [`Self::record_rejection`].
    #[cfg(test)]
    pub fn peek(&self, key: &str) -> u32 {
        self.counters.get(key).copied().unwrap_or(0)
    }

    /// Record a pass (gate emitted `<X>`).  Resets the counter
    /// to zero.
    pub fn record_pass(&mut self, loop_id: &str, topic: &str) {
        let key = Self::key(loop_id, topic);
        self.counters.insert(key, 0);
    }

    /// Record a rejection and return the new count.  The
    /// dispatch helper uses the returned value to decide
    /// between resume and exhaustion.
    pub fn record_rejection(&mut self, loop_id: &str, topic: &str) -> u32 {
        let key = Self::key(loop_id, topic);
        let entry = self.counters.entry(key).or_insert(0);
        *entry = entry.saturating_add(1);
        *entry
    }
}

/// Parameters that drive the dispatch decision.  Bundled so
/// call sites don't accidentally swap argument order.
#[derive(Debug, Clone)]
pub struct DispatchParams<'a> {
    /// Orchestrator loop id (so multiple loops against the
    /// same preset get isolated counters).
    pub loop_id: &'a str,
    /// Guarded topic `X` (e.g. `"review.complete"`).
    pub topic: &'a str,
    /// `target_hat` from `PrecheckOnFail`.
    pub target_hat: &'a str,
    /// `retry_budget` from `PrecheckOnFail`.
    pub retry_budget: u32,
    /// `on_exhausted` from `PrecheckOnFail` (e.g.
    /// `"plan.blocked(reason=precheck_failed)"`).
    pub on_exhausted: &'a str,
    /// Already-incremented rejection count.
    pub rejection_count: u32,
    /// Pre-rendered `<X>.rejected` payload (LLM or synthetic).
    pub rejected_payload_json: &'a str,
}

/// Decide the dispatch outcome after a rejection.  Pure
/// function: given the new count and the rule, returns the
/// `DispatchOutcome` for the event loop to act on.
///
/// `retry_budget == 0` is treated as "no retries allowed" so a
/// first rejection immediately exhausts — useful for presets
/// that want a single hard gate with no back-and-forth.
pub fn dispatch_rejection(params: &DispatchParams<'_>) -> DispatchOutcome {
    let exhausted = if params.retry_budget == 0 {
        params.rejection_count >= 1
    } else {
        params.rejection_count >= params.retry_budget
    };
    if exhausted {
        // Exhausted: emit `on_exhausted`.  We split on `(reason=`
        // for the default `plan.blocked(reason=...)` form so the
        // event loop can render a `plan.blocked` payload with the
        // reason field set explicitly (R8).
        let (topic, reason) = split_on_exhausted(params.on_exhausted);
        return DispatchOutcome::Exhausted { topic, reason };
    }

    // Within budget: build a `task.resume` payload that
    // preserves the rejection reason so the target hat sees
    // the failure on its next prompt (R5).  Mirrors the shape
    // of `event_loop::rejection::build_task_resume_payload` so
    // the prompt injector reads the fields uniformly.
    let payload = build_resume_payload(
        params.topic,
        params.target_hat,
        params.rejection_count,
        params.retry_budget,
        params.rejected_payload_json,
    );
    DispatchOutcome::Resume {
        target_hat: params.target_hat.to_string(),
        payload_json: payload,
        new_count: params.rejection_count,
    }
}

/// Parse an `on_exhausted` string of the form
/// `topic(reason="...")` (the grammar produced by the
/// default `plan.blocked(reason=precheck_failed)` value).
/// Falls back to `(topic, default_reason)` when the grammar
/// doesn't match — e.g. a bare `plan.blocked`.
fn split_on_exhausted(on_exhausted: &str) -> (String, String) {
    if let Some(start) = on_exhausted.find("(reason=") {
        let topic = on_exhausted[..start].trim().to_string();
        let rest = &on_exhausted[start + "(reason=".len()..];
        // Strip a leading quote if present, then trim trailing
        // `)` and any trailing quote.
        let rest = rest.trim_start_matches('"');
        let reason = rest.trim_end_matches(')').trim_end_matches('"').to_string();
        return (topic, reason);
    }
    (
        on_exhausted.trim().to_string(),
        "precheck_failed".to_string(),
    )
}

/// Human-readable failure text for prompt injection (R5 / AE3).
pub fn format_precheck_failure_message(guarded_topic: &str, rejected_payload_json: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(rejected_payload_json) {
        let reason = parsed
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("precheck_rejected");
        let checks = parsed
            .get("failed_checks")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "[]".to_string());
        return format!(
            "PRECHECK GATE rejected `{guarded_topic}`: reason={reason}; failed_checks={checks}"
        );
    }
    format!("PRECHECK GATE rejected `{guarded_topic}` (malformed rejection payload)")
}

/// Build a `task.resume` payload that carries the rejection
/// context to the target hat.  Mirrors the wire shape of
/// `event_loop::rejection::build_task_resume_payload` (which
/// the runner already injects for other rejection kinds) so
/// downstream consumers can parse both uniformly.
fn build_resume_payload(
    topic: &str,
    target_hat: &str,
    rejection_count: u32,
    retry_budget: u32,
    rejected_payload_json: &str,
) -> String {
    // Try to extract `failed_checks` / `reason` from the
    // rejected payload so they surface in the prompt without
    // a second round-trip.  Fall back to empty arrays when the
    // payload isn't a JSON object (defensive — LLM emits are
    // not always valid JSON).
    let mut obj = serde_json::Map::new();
    obj.insert("stage".into(), serde_json::Value::String("precheck".into()));
    obj.insert("topic".into(), serde_json::Value::String(topic.to_string()));
    obj.insert(
        "violation".into(),
        serde_json::Value::String("precheck_rejected".into()),
    );
    obj.insert(
        "reason".into(),
        serde_json::Value::String("precheck_rejected".into()),
    );
    obj.insert(
        "kind".into(),
        serde_json::Value::String("precheck_rejected".into()),
    );
    obj.insert(
        "target_hat".into(),
        serde_json::Value::String(target_hat.to_string()),
    );
    obj.insert(
        "precheck_count".into(),
        serde_json::Value::Number(rejection_count.into()),
    );
    obj.insert(
        "precheck_budget".into(),
        serde_json::Value::Number(retry_budget.into()),
    );
    // Embed the original rejected payload's structured fields
    // so the prompt injector can render them verbatim.
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(rejected_payload_json) {
        if let Some(arr) = parsed.get("failed_checks").cloned() {
            obj.insert("failed_checks".into(), arr);
        }
        if let Some(reason) = parsed.get("reason").and_then(|v| v.as_str()) {
            obj.insert(
                "precheck_reason".into(),
                serde_json::Value::String(reason.to_string()),
            );
        }
    }
    serde_json::Value::Object(obj).to_string()
}

/// Build the payload for an `on_exhausted` topic.  Splits the
/// directive on `(` so the common `plan.blocked(reason=X)` form
/// produces a payload with `reason` set explicitly.
pub fn build_exhausted_payload(topic: &str, reason: &str) -> String {
    serde_json::json!({
        "topic": topic,
        "reason": reason,
        "kind": "precheck_exhausted",
    })
    .to_string()
}

/// U2 (plan 2026-08-06-001, R1/R5): convert a rejected-payload
/// JSON string into a `CorrectionContext` evidence detail.
///
/// - `RejectedPayload::synthetic` (`synthetic == true`) →
///   `synthetic = true`, observed stays empty, invariant + proof
///   are filled with the gate-silent/ambiguous markers so the
///   agent cannot mistake the missing evidence for a clean
///   observation.  `failed_checks` is recorded as a
///   comma-separated invariant suffix so the agent still sees
///   which checklist indices the synthetic rejection covers.
/// - LLM-emitted (`synthetic == false`) → per-check
///   `ObservationValue::Unchecked` entries (the gate's
///   structured "did not pass" answer — the agent must re-verify
///   each check, we never invent the check result).  `reason` is
///   the invariant; the proof asks the agent to re-run the gate
///   after fixing the artifact.
///
/// Returns `None` when the payload is malformed JSON so the
/// caller can fall back to a generic "rejected payload
/// malformed" diagnostic without inventing evidence.
///
/// U3 (plan 2026-08-17-1841, R2 / R3 / D2 / D3): when
/// `rule.recovery_guidance` is present, the function threads
/// the preset-supplied common / by_check items into the
/// `EvidenceDetail` so the U2 correction renderer surfaces
/// them at the target hat's prompt.  The function also records
/// `failed_check_keys` (the failed-checks string list) so the
/// renderer can filter `by_check` to only the actually-failed
/// checks.  Synthetic rejections carry the guidance block too,
/// but the renderer suppresses the specific sub-section (D3).
///
/// U3 / M3 / R10: the body is split into four single-purpose
/// helpers (`parse_payload_for_precheck` /
/// `extract_synthetic_and_failed_keys` /
/// `build_observed_invariant_proof` /
/// `inject_recovery_guidance_into_proof`). The top-level
/// function is now a 12-line orchestrator.
pub fn build_precheck_evidence(
    guarded_topic: &str,
    rejected_payload_json: &str,
    rule: Option<&crate::config::PrecheckRule>,
) -> Option<crate::correction::EvidenceDetail> {
    let parsed = parse_payload_for_precheck(rejected_payload_json)?;
    let (synthetic, failed_checks, reason) = extract_synthetic_and_failed_keys(&parsed);
    let (observed, invariant, proof) =
        build_observed_invariant_proof(guarded_topic, synthetic, &failed_checks, &reason);
    inject_recovery_guidance_into_proof(
        crate::correction::EvidenceDetail {
            observed,
            invariant,
            proof,
            synthetic,
            guidance: None,
            failed_check_keys: None,
        },
        rule,
        &failed_checks,
    )
}

/// U3 / M3 / R10: parse the rejected payload JSON. Returns `None`
/// when the payload is malformed so the caller can fall back to
/// the legacy "rejected payload malformed" path without inventing
/// evidence.
fn parse_payload_for_precheck(rejected_payload_json: &str) -> Option<serde_json::Value> {
    match serde_json::from_str(rejected_payload_json) {
        Ok(v) => Some(v),
        Err(_) => None,
    }
}

/// U3 / M3 / R10: pull the structured `synthetic` /
/// `failed_checks` / `reason` triple out of the parsed payload.
/// Defaults: `synthetic = false`, `failed_checks = []`,
/// `reason = "precheck_rejected"`.
fn extract_synthetic_and_failed_keys(parsed: &serde_json::Value) -> (bool, Vec<String>, String) {
    let synthetic = parsed
        .get("synthetic")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let failed_checks: Vec<String> = parsed
        .get("failed_checks")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|value| match value {
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let reason = parsed
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("precheck_rejected")
        .to_string();
    (synthetic, failed_checks, reason)
}

/// U3 / M3 / R10: produce the `(observed, invariant, proof)` triple
/// for the evidence. Synthetic rejections suppress observed
/// entries and use the silent/ambiguous invariant; LLM-emitted
/// rejections keep per-check `Unchecked` observations.
fn build_observed_invariant_proof(
    guarded_topic: &str,
    synthetic: bool,
    failed_checks: &[String],
    reason: &str,
) -> (Vec<crate::correction::ObservationEntry>, String, String) {
    use crate::correction::{ObservationEntry, ObservationValue};
    let observed: Vec<ObservationEntry> = if synthetic {
        Vec::new()
    } else {
        failed_checks
            .iter()
            .map(|check| ObservationEntry {
                field: format!("check_{check}"),
                value: ObservationValue::Unchecked,
            })
            .collect()
    };
    let invariant = if synthetic {
        format!(
            "precheck gate for `{guarded_topic}` was silent or ambiguous; cannot confirm any checklist item passed"
        )
    } else {
        format!(
            "precheck gate for `{guarded_topic}` failed: {reason}; failed_checks={:?}",
            failed_checks
        )
    };
    let proof = format!(
        "Reinvestigate the artifact / test for `{guarded_topic}` against the gate's checklist; do not change only the failed_check indices. After fixing the underlying artifact, re-run `ralph emit --policy-check` and re-emit the original `{guarded_topic}` event."
    );
    (observed, invariant, proof)
}

/// U3 / M3 / R10: thread the preset-supplied recovery guidance
/// into the evidence so the U2 correction renderer can surface
/// the common / by_check items at the target hat's prompt, and
/// record `failed_check_keys` so the renderer can filter
/// `by_check` to the actually-failed checks.
///
/// When the gate reported no `failed_checks` but the rule declares
/// a non-empty `by_check` map, pin `failed_check_keys` to an empty
/// list so the renderer keeps `common` and does not fall back to
/// "render every `by_check` key". Synthetic rejections always keep
/// guidance: the renderer already suppresses the specific
/// sub-section (D3) even if `failed_checks` is non-empty.
fn inject_recovery_guidance_into_proof(
    mut evidence: crate::correction::EvidenceDetail,
    rule: Option<&crate::config::PrecheckRule>,
    failed_checks: &[String],
) -> Option<crate::correction::EvidenceDetail> {
    let rule_guidance = rule.and_then(|r| r.recovery_guidance.clone());
    let rule_has_specific_guidance = rule_guidance
        .as_ref()
        .map(|g| !g.by_check.is_empty())
        .unwrap_or(false);

    evidence.guidance = rule_guidance;
    let pin_empty_specific_keys =
        !evidence.synthetic && rule_has_specific_guidance && failed_checks.is_empty();
    evidence.failed_check_keys = if pin_empty_specific_keys {
        Some(Vec::new())
    } else if failed_checks.is_empty() {
        None
    } else {
        Some(failed_checks.to_vec())
    };
    Some(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_params<'a>(
        loop_id: &'a str,
        topic: &'a str,
        target_hat: &'a str,
        retry_budget: u32,
        on_exhausted: &'a str,
        rejection_count: u32,
        rejected_payload_json: &'a str,
    ) -> DispatchParams<'a> {
        DispatchParams {
            loop_id,
            topic,
            target_hat,
            retry_budget,
            on_exhausted,
            rejection_count,
            rejected_payload_json,
        }
    }

    #[test]
    fn record_pass_resets_counter() {
        let mut reg = PrecheckRetryRegistry::new();
        let key = PrecheckRetryRegistry::key("loop1", "review.complete");
        reg.counters.insert(key.clone(), 2);
        reg.record_pass("loop1", "review.complete");
        assert_eq!(reg.peek(&key), 0);
    }

    #[test]
    fn record_rejection_increments() {
        let mut reg = PrecheckRetryRegistry::new();
        let n1 = reg.record_rejection("loop1", "review.complete");
        let n2 = reg.record_rejection("loop1", "review.complete");
        let n3 = reg.record_rejection("loop1", "review.complete");
        assert_eq!(n1, 1);
        assert_eq!(n2, 2);
        assert_eq!(n3, 3);
    }

    #[test]
    fn counters_isolated_per_loop_and_topic() {
        let mut reg = PrecheckRetryRegistry::new();
        reg.record_rejection("loop1", "review.complete");
        reg.record_rejection("loop1", "review.complete");
        reg.record_rejection("loop2", "review.complete");
        reg.record_rejection("loop1", "build.done");
        assert_eq!(
            reg.peek(&PrecheckRetryRegistry::key("loop1", "review.complete")),
            2
        );
        assert_eq!(
            reg.peek(&PrecheckRetryRegistry::key("loop2", "review.complete")),
            1
        );
        assert_eq!(
            reg.peek(&PrecheckRetryRegistry::key("loop1", "build.done")),
            1
        );
    }

    #[test]
    fn dispatch_within_budget_emits_resume() {
        let rejected = r#"{"failed_checks":[1],"reason":"missing","synthetic":false}"#;
        let outcome = dispatch_rejection(&default_params(
            "loop1",
            "review.complete",
            "reviewer",
            3,
            "plan.blocked(reason=precheck_failed)",
            1,
            rejected,
        ));
        match outcome {
            DispatchOutcome::Resume {
                target_hat,
                payload_json,
                new_count,
            } => {
                assert_eq!(target_hat, "reviewer");
                assert_eq!(new_count, 1);
                let parsed: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
                assert_eq!(parsed["target_hat"], "reviewer");
                assert_eq!(parsed["topic"], "review.complete");
                assert_eq!(parsed["precheck_count"], 1);
                assert_eq!(parsed["precheck_budget"], 3);
                assert_eq!(parsed["failed_checks"], serde_json::json!([1]));
                assert_eq!(parsed["precheck_reason"], "missing");
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_at_budget_emits_exhausted() {
        // count == budget → AE2: third rejection exhausts when budget is 3.
        let outcome = dispatch_rejection(&default_params(
            "loop1",
            "x",
            "target",
            3,
            "plan.blocked(reason=precheck_failed)",
            3,
            "{}",
        ));
        assert!(matches!(outcome, DispatchOutcome::Exhausted { .. }));
    }

    #[test]
    fn dispatch_within_budget_before_exhaustion() {
        let outcome = dispatch_rejection(&default_params(
            "loop1",
            "x",
            "target",
            3,
            "plan.blocked(reason=precheck_failed)",
            2,
            "{}",
        ));
        assert!(matches!(outcome, DispatchOutcome::Resume { .. }));
    }

    #[test]
    fn dispatch_exhausted_emits_default_topic() {
        let outcome = dispatch_rejection(&default_params(
            "loop1",
            "x",
            "target",
            3,
            "plan.blocked(reason=precheck_failed)",
            4,
            "{}",
        ));
        match outcome {
            DispatchOutcome::Exhausted { topic, reason } => {
                assert_eq!(topic, "plan.blocked");
                assert_eq!(reason, "precheck_failed");
            }
            other => panic!("expected Exhausted, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_zero_budget_exhausts_on_first_rejection() {
        let outcome = dispatch_rejection(&default_params(
            "loop1",
            "x",
            "target",
            0,
            "plan.blocked(reason=precheck_failed)",
            1,
            "{}",
        ));
        assert!(matches!(outcome, DispatchOutcome::Exhausted { .. }));
    }

    #[test]
    fn dispatch_custom_on_exhausted_is_parsed() {
        let outcome = dispatch_rejection(&default_params(
            "loop1",
            "x",
            "target",
            0,
            "custom.terminal(reason=\"custom_value\")",
            1,
            "{}",
        ));
        match outcome {
            DispatchOutcome::Exhausted { topic, reason } => {
                assert_eq!(topic, "custom.terminal");
                assert_eq!(reason, "custom_value");
            }
            other => panic!("expected Exhausted, got {other:?}"),
        }
    }

    #[test]
    fn split_on_exhausted_handles_bare_topic() {
        let (topic, reason) = split_on_exhausted("plan.blocked");
        assert_eq!(topic, "plan.blocked");
        assert_eq!(reason, "precheck_failed");
    }

    #[test]
    fn build_exhausted_payload_carries_topic_and_reason() {
        let payload = build_exhausted_payload("plan.blocked", "precheck_failed");
        let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["topic"], "plan.blocked");
        assert_eq!(parsed["reason"], "precheck_failed");
        assert_eq!(parsed["kind"], "precheck_exhausted");
    }

    #[test]
    fn resume_payload_survives_malformed_rejected_json() {
        // The LLM sometimes emits invalid JSON; the resume
        // payload must still serialize cleanly so the prompt
        // injector can read its top-level fields.
        let outcome = dispatch_rejection(&default_params(
            "loop1",
            "x",
            "target",
            3,
            "plan.blocked(reason=precheck_failed)",
            1,
            "this is not json",
        ));
        match outcome {
            DispatchOutcome::Resume { payload_json, .. } => {
                let parsed: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
                assert_eq!(parsed["target_hat"], "target");
                assert_eq!(parsed["topic"], "x");
                // failed_checks / precheck_reason absent when
                // input was malformed.
                assert!(parsed.get("failed_checks").is_none());
                assert!(parsed.get("precheck_reason").is_none());
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    // -- U2 evidence builder (plan 2026-08-06-001) -----

    #[test]
    fn u2_build_precheck_evidence_marks_synthetic() {
        let json =
            r#"{"failed_checks":[1,2,3],"reason":"gate_silent_or_ambiguous","synthetic":true}"#;
        let evidence = build_precheck_evidence("work.done", json, None).unwrap();
        assert!(evidence.synthetic);
        assert!(
            evidence.observed.is_empty(),
            "synthetic rejections must not invent observations"
        );
        assert!(evidence.invariant.contains("silent or ambiguous"));
        assert!(evidence.proof.contains("Reinvestigate"));
        // No replacement guidance in proof.
        assert!(!evidence.proof.contains("suggested"));
    }

    #[test]
    fn u2_build_precheck_evidence_marks_llm_checks_unchecked() {
        use crate::correction::ObservationValue;
        let json = r#"{"failed_checks":[2],"reason":"missing test report","synthetic":false}"#;
        let evidence = build_precheck_evidence("work.done", json, None).unwrap();
        assert!(!evidence.synthetic);
        assert_eq!(evidence.observed.len(), 1);
        assert_eq!(evidence.observed[0].field, "check_2");
        assert!(matches!(
            evidence.observed[0].value,
            ObservationValue::Unchecked
        ));
        assert!(evidence.invariant.contains("missing test report"));
    }

    #[test]
    fn u2_build_precheck_evidence_preserves_string_check_identity() {
        use crate::correction::ObservationValue;
        let json = r#"{"failed_checks":["confidence_inflated"],"reason":"missing evidence","synthetic":false}"#;
        let evidence = build_precheck_evidence("work.done", json, None).unwrap();
        assert_eq!(evidence.observed.len(), 1);
        assert_eq!(evidence.observed[0].field, "check_confidence_inflated");
        assert!(matches!(
            evidence.observed[0].value,
            ObservationValue::Unchecked
        ));
        assert!(evidence.invariant.contains("confidence_inflated"));
    }

    #[test]
    fn u2_build_precheck_evidence_returns_none_for_malformed() {
        let evidence = build_precheck_evidence("work.done", "not json", None);
        assert!(evidence.is_none());
    }

    // ── U3 (plan 2026-08-17-1841) — guidance selection ──

    use crate::config::{PrecheckOnFail, PrecheckRule};

    fn rule_with_prompt(
        prompt: Vec<&str>,
        guidance: Option<crate::config::RecoveryGuidance>,
    ) -> PrecheckRule {
        PrecheckRule {
            prompt: prompt.into_iter().map(String::from).collect(),
            on_fail: PrecheckOnFail {
                target: "executor".into(),
                retry_budget: 3,
                on_exhausted: String::new(),
                reason: String::new(),
            },
            recovery_guidance: guidance,
        }
    }

    fn guidance(common: &[&str], by_check: &[(&str, &[&str])]) -> crate::config::RecoveryGuidance {
        crate::config::RecoveryGuidance {
            common: common.iter().map(|s| s.to_string()).collect(),
            by_check: by_check
                .iter()
                .map(|(k, v)| {
                    (
                        k.to_string(),
                        v.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                    )
                })
                .collect(),
        }
    }

    /// U3 / R2: when the rule declares `recovery_guidance` and
    /// the gate reports `failed_checks = [2]`, the evidence
    /// carries the preset-supplied guidance + the failed check
    /// keys so the renderer can filter by_check.
    #[test]
    fn u3_build_precheck_evidence_threads_guidance_and_failed_keys() {
        let json = r#"{"failed_checks":[2],"reason":"missing report","synthetic":false}"#;
        let rule = rule_with_prompt(
            vec!["a", "b", "c"],
            Some(guidance(
                &["common hint"],
                &[("2", &["fill required report"])],
            )),
        );
        let evidence = build_precheck_evidence("work.done", json, Some(&rule)).unwrap();
        let g = evidence.guidance.as_ref().expect("guidance populated");
        assert_eq!(g.common, vec!["common hint".to_string()]);
        assert_eq!(
            g.by_check.get("2").map(|v| v.clone()),
            Some(vec!["fill required report".to_string()])
        );
        let keys = evidence.failed_check_keys.as_ref().expect("failed keys");
        assert_eq!(keys, &vec!["2".to_string()]);
    }

    /// U3 / D3: synthetic rejection carries the guidance but
    /// `synthetic = true` — the renderer must suppress the
    /// specific sub-section.
    #[test]
    fn u3_synthetic_carries_guidance_but_renderer_suppresses() {
        // U4 / A2 (plan 2026-08-17-1841) updated this fixture:
        // `synthetic = true` requires `failed_checks` to be empty
        // (synthetic means "the gate was silent or ambiguous",
        // which is incompatible with a structured list of failed
        // checks). The prior version of this test used
        // `failed_checks:[1,2,3]` together with `synthetic:true`
        // — A2 fail-loud now suppresses the guidance block in
        // that case.
        let json = r#"{"failed_checks":[],"reason":"silent","synthetic":true}"#;
        let rule = rule_with_prompt(
            vec!["a", "b", "c"],
            Some(guidance(&["common"], &[("1", &["specific-1"])])),
        );
        let evidence = build_precheck_evidence("work.done", json, Some(&rule)).unwrap();
        assert!(evidence.synthetic);
        assert!(
            evidence.guidance.is_some(),
            "guidance must be threaded for synthetic too"
        );
        assert!(evidence.failed_check_keys.is_none());
    }

    /// U3 / R2: when no rule is supplied the function still
    /// populates `failed_check_keys` (the gate reported them),
    /// and `guidance` stays `None` (legacy callers).
    #[test]
    fn u3_no_rule_still_records_failed_keys() {
        let json = r#"{"failed_checks":["confidence_inflated"],"reason":"x","synthetic":false}"#;
        let evidence = build_precheck_evidence("work.done", json, None).unwrap();
        assert!(evidence.guidance.is_none());
        let keys = evidence.failed_check_keys.as_ref().expect("keys present");
        assert_eq!(keys, &vec!["confidence_inflated".to_string()]);
    }

    /// U3 / R2 / D3: empty `failed_checks` leaves the
    /// `failed_check_keys` field as `None`.  The renderer
    /// falls back to "render all by_check keys" only when
    /// the field is absent; here we want to preserve the
    /// rule's intent even when the gate reported no
    /// specific check.
    #[test]
    fn u3_empty_failed_checks_leaves_failed_keys_none() {
        let json = r#"{"reason":"x","synthetic":false}"#;
        let evidence = build_precheck_evidence("work.done", json, None).unwrap();
        assert!(evidence.failed_check_keys.is_none());
    }

    /// U2 / T4 / R6: a payload carrying multiple `failed_checks`
    /// (e.g. `[1, 3]`) threads each key into
    /// `failed_check_keys` so the renderer can filter `by_check`
    /// to the exactly-failed keys. Pins the multi-key behaviour
    /// the U2 cap test in `correction/mod.rs` relies on.
    #[test]
    fn u2_build_precheck_evidence_threads_multiple_failed_checks() {
        let json = r#"{"failed_checks":[1,3],"reason":"two checks failed","synthetic":false}"#;
        let mut by_check = std::collections::BTreeMap::new();
        by_check.insert("1".to_string(), vec!["fix 1".to_string()]);
        by_check.insert("2".to_string(), vec!["fix 2".to_string()]);
        by_check.insert("3".to_string(), vec!["fix 3".to_string()]);
        let rule = rule_with_prompt(
            vec!["a", "b", "c"],
            Some(crate::config::RecoveryGuidance {
                common: vec!["common hint".into()],
                by_check,
            }),
        );
        let evidence = build_precheck_evidence("work.done", json, Some(&rule)).unwrap();
        let keys = evidence.failed_check_keys.as_ref().expect("keys present");
        assert_eq!(keys, &vec!["1".to_string(), "3".to_string()]);
    }

    /// U4 / A1 (plan 2026-08-17-1841): when the gate reported an
    /// empty `failed_checks` list but the rule declared a
    /// non-empty `by_check` map, the prior implementation set
    /// `failed_check_keys = None` and let the renderer fall back
    /// to "render every by_check key". Keep `common` and pin
    /// `failed_check_keys` to an empty list so specific items
    /// stay suppressed.
    #[test]
    fn u4_a1_empty_failed_checks_with_by_check_keeps_common_only() {
        let json = r#"{"reason":"silent gate","synthetic":false}"#;
        let mut by_check = std::collections::BTreeMap::new();
        by_check.insert("1".to_string(), vec!["fix 1".to_string()]);
        let rule = rule_with_prompt(
            vec!["a", "b", "c"],
            Some(crate::config::RecoveryGuidance {
                common: vec!["common hint".into()],
                by_check,
            }),
        );
        let evidence = build_precheck_evidence("work.done", json, Some(&rule)).unwrap();
        let guidance = evidence.guidance.expect("common must remain");
        assert_eq!(guidance.common, vec!["common hint".to_string()]);
        assert_eq!(
            evidence.failed_check_keys.as_deref(),
            Some(&[][..]),
            "empty failed_checks + by_check must pin keys to [] so specific items do not render"
        );
    }

    /// Synthetic + non-empty `failed_checks` still keeps guidance.
    /// The renderer suppresses the specific sub-section (D3).
    #[test]
    fn u4_a2_synthetic_with_failed_checks_keeps_common_guidance() {
        let json = r#"{"failed_checks":[1,2],"reason":"silent","synthetic":true}"#;
        let mut by_check = std::collections::BTreeMap::new();
        by_check.insert("1".to_string(), vec!["fix 1".to_string()]);
        let rule = rule_with_prompt(
            vec!["a", "b", "c"],
            Some(crate::config::RecoveryGuidance {
                common: vec!["common".into()],
                by_check,
            }),
        );
        let evidence = build_precheck_evidence("work.done", json, Some(&rule)).unwrap();
        let guidance = evidence.guidance.expect("synthetic must keep common");
        assert_eq!(guidance.common, vec!["common".to_string()]);
        assert!(evidence.synthetic);
    }

    /// U4 / A2 happy path: `synthetic = true` with no
    /// `failed_checks` is the canonical synthetic shape and
    /// guidance threads through (renderer will suppress the
    /// specific sub-section per D3).
    #[test]
    fn u4_a2_synthetic_with_no_failed_checks_keeps_guidance() {
        let json = r#"{"reason":"silent","synthetic":true}"#;
        let mut by_check = std::collections::BTreeMap::new();
        by_check.insert("1".to_string(), vec!["fix 1".to_string()]);
        let rule = rule_with_prompt(
            vec!["a", "b", "c"],
            Some(crate::config::RecoveryGuidance {
                common: vec!["common".into()],
                by_check,
            }),
        );
        let evidence = build_precheck_evidence("work.done", json, Some(&rule)).unwrap();
        assert!(evidence.guidance.is_some());
    }

    /// U4 / A1 control: empty `failed_checks` AND no
    /// `by_check` (common-only guidance) is allowed — the
    /// rule author did not promise specific guidance, so
    /// the fail-loud path does not fire.
    #[test]
    fn u4_a1_empty_failed_checks_without_by_check_keeps_common_guidance() {
        let json = r#"{"reason":"silent gate","synthetic":false}"#;
        let rule = rule_with_prompt(
            vec!["a", "b", "c"],
            Some(crate::config::RecoveryGuidance {
                common: vec!["common hint".into()],
                by_check: std::collections::BTreeMap::new(),
            }),
        );
        let evidence = build_precheck_evidence("work.done", json, Some(&rule)).unwrap();
        assert!(
            evidence.guidance.is_some(),
            "common-only guidance must pass through; got None"
        );
    }
}
