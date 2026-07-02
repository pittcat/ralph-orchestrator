//! 2026-07-02-004 plan milestone B (U5): hard-gate enforcement
//! for synthesized `precheck-<X>` gate hats.
//!
//! Contract (locked by U5):
//! 1. Each gate hat has `terminal_events = [X, X.rejected]`.
//! 2. When activated by `X.proposed`, the runtime MUST observe
//!    exactly one of those two topics before the next activation
//!    cycle closes.
//! 3. If neither arrives (the gate was silent), the runtime
//!    synthesizes a synthetic `<X>.rejected` event with
//!    `reason = "gate_silent_or_ambiguous"` and
//!    `failed_checks = vec!["<all>"]` so downstream rejection
//!    routing (U6) can dispatch a `task.resume`.
//! 4. The synthetic event is distinguishable from an
//!    LLM-emitted one via `payload.synthetic == true`; the
//!    runner (U6) treats both paths uniformly but the
//!    diagnostics surface records the synthetic flag so
//!    operators can see which path fired.
//!
//! Architectural note: this module is **pure CPU only**. It
//! holds the in-memory helper types and the synthesis function;
//! the wiring into the event loop lives in
//! `event_loop::mod` (see `enforce_precheck_gate_obligation`).
//! Cross-platform / concurrency: single-threaded by construction.

use serde::{Deserialize, Serialize};

/// Marker indicating a precheck gate hat. The hat id always
/// begins with `precheck-`; we expose this constant so call
/// sites don't repeat the string.
pub const GATE_HAT_PREFIX: &str = "precheck-";

/// Source of a `X.rejected` payload.  U6 reads this to surface
/// diagnostics; the enforcement path itself (U5) treats both
/// uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionSource {
    /// LLM emitted `X.rejected` after reading its checklist.
    Llm,
    /// Runtime synthesized `X.rejected` because the gate was
    /// silent (or emitted both branches) inside the grace window.
    Synthetic,
}

impl RejectionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectionSource::Llm => "llm",
            RejectionSource::Synthetic => "synthetic",
        }
    }
}

/// Payload of an `X.rejected` event.  Both the LLM-emitted
/// variant and the synthetic one conform to this shape (R4 in
/// the plan): `failed_checks` lists the 1-based checklist
/// indices that failed, `reason` is a short human-readable
/// string, and `synthetic` records which path emitted the
/// payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RejectedPayload {
    /// Checklist indices (1-based) that the LLM judged
    /// unsatisfactory.  For synthetic rejections this contains
    /// every checklist index (gate was silent — we cannot know
    /// which specific point failed).
    pub failed_checks: Vec<u32>,
    /// Short human-readable reason.  Defaults to the gate's
    /// `on_fail.reason` for LLM-emitted rejections, and to
    /// `"gate_silent_or_ambiguous"` for synthetic ones.
    pub reason: String,
    /// Which path emitted this rejection.  Always present so
    /// downstream diagnostics can tell LLM rejections from
    /// runtime-enforced ones.
    pub synthetic: bool,
}

impl RejectedPayload {
    /// Build an LLM-style payload from explicit checklist
    /// failures and a reason string.
    pub fn from_llm(failed_checks: Vec<u32>, reason: impl Into<String>) -> Self {
        Self {
            failed_checks,
            reason: reason.into(),
            synthetic: false,
        }
    }

    /// Build a synthetic payload for the silent-or-ambiguous
    /// case.  `total_checks` is the number of checklist items in
    /// the gate's `prompt`; the payload claims every item failed.
    pub fn synthetic(total_checks: usize) -> Self {
        Self {
            failed_checks: (1..=total_checks as u32).collect(),
            reason: "gate_silent_or_ambiguous".to_string(),
            synthetic: true,
        }
    }

    /// Render as a JSON object string suitable for
    /// `Event::new("X.rejected", payload)`.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("RejectedPayload serializes")
    }
}

/// Resolve the gate hat id for a precheck terminal emit. JSONL
/// ingest often leaves `Event::source` unset; infer
/// `precheck-<X>` from the guarded topic when possible.
pub fn resolve_gate_hat_for_emit(
    event: &ralph_proto::Event,
    rules: &std::collections::BTreeMap<String, crate::config::PrecheckRule>,
) -> Option<String> {
    if let Some(source) = event.source.as_ref() {
        if is_gate_hat(source.as_str()) {
            return Some(source.to_string());
        }
    }
    let topic = event.topic.as_str();
    if let Some(guarded) = topic.strip_suffix(".rejected") {
        if rules.contains_key(guarded) {
            return Some(format!("{GATE_HAT_PREFIX}{guarded}"));
        }
    }
    if rules.contains_key(topic) {
        return Some(format!("{GATE_HAT_PREFIX}{topic}"));
    }
    None
}

/// Returns `true` when `hat_id` is a synthesized precheck gate
/// hat (its id begins with `precheck-`).
pub fn is_gate_hat(hat_id: &str) -> bool {
    hat_id.starts_with(GATE_HAT_PREFIX)
}

/// Extract the guarded topic `X` from a gate hat id
/// `precheck-X`. Returns `None` when `hat_id` is not a gate
/// hat.
pub fn gate_topic(hat_id: &str) -> Option<&str> {
    hat_id.strip_prefix(GATE_HAT_PREFIX)
}

/// Compute the two derived topics that U2's desugar introduced:
/// the proposed variant and the rejected variant.  Both are
/// returned as owned strings so callers can build event
/// objects without lifetime juggling.
pub fn derived_topics(topic: &str) -> (String, String) {
    (format!("{topic}.proposed"), format!("{topic}.rejected"))
}

/// Decide whether a given emit from a gate hat satisfies the
/// hard-gate obligation.  Returns `true` for the two terminal
/// topics (`X`, `X.rejected`) and `false` for everything else
/// (including `X.proposed`, which would mean the gate emitted
/// the candidate rather than a decision — that's a routing
/// bug, not a valid close).
pub fn is_satisfying_emit(emitted_topic: &str, gate_topic: &str) -> bool {
    emitted_topic == gate_topic || emitted_topic == format!("{gate_topic}.rejected")
}

/// Build a synthetic `<X>.rejected` event payload string for
/// the silent-or-ambiguous case.  `total_checks` is the number
/// of checklist items declared on the gate; when `None`, the
/// payload records zero failed checks (used when the gate has
/// no checklist at all).
pub fn synthesize_rejection_payload(total_checks: Option<usize>) -> String {
    let payload = match total_checks {
        Some(n) if n > 0 => RejectedPayload::synthetic(n),
        _ => RejectedPayload {
            failed_checks: Vec::new(),
            reason: "gate_silent_or_ambiguous".to_string(),
            synthetic: true,
        },
    };
    payload.to_json()
}

/// Build an LLM-emitted `<X>.rejected` payload from explicit
/// failures and a reason.  Mirrors what the gate hat's
/// instructions ask the LLM to emit.
pub fn llm_rejection_payload(failed_checks: Vec<u32>, reason: &str) -> String {
    RejectedPayload::from_llm(failed_checks, reason).to_json()
}

/// Outcome of scanning a gate hat's emissions in one accepted
/// batch.  Used by U5 enforcement to detect silence / ambiguity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateEmitOutcome {
    /// No terminal emit from the gate hat in this batch.
    Silent,
    /// Exactly one pass (`X`).
    Pass,
    /// Exactly one reject (`X.rejected`).
    Reject,
    /// Both `X` and `X.rejected`, or other ambiguous combination.
    Ambiguous,
}

/// Classify what a precheck gate hat emitted in `accepted`.
pub fn classify_gate_emit_outcome(
    gate_hat_id: &str,
    guarded: &str,
    accepted: &[ralph_proto::Event],
) -> GateEmitOutcome {
    let rejected_topic = format!("{guarded}.rejected");
    let mut saw_pass = false;
    let mut saw_reject = false;
    for event in accepted {
        let Some(source) = event.source.as_ref() else {
            continue;
        };
        if source.as_str() != gate_hat_id {
            continue;
        }
        let topic = event.topic.as_str();
        if topic == guarded {
            saw_pass = true;
        } else if topic == rejected_topic {
            saw_reject = true;
        }
    }
    match (saw_pass, saw_reject) {
        (true, true) => GateEmitOutcome::Ambiguous,
        (true, false) => GateEmitOutcome::Pass,
        (false, true) => GateEmitOutcome::Reject,
        (false, false) => GateEmitOutcome::Silent,
    }
}

/// A synthetic `<X>.rejected` the runtime must inject because the
/// gate hat was silent or ambiguous (U5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticPrecheckRejection {
    pub gate_hat_id: String,
    pub guarded_topic: String,
    pub rejected_topic: String,
    pub payload_json: String,
}

/// Find open `precheck-<X>` obligations whose gate hat did not
/// emit exactly one terminal topic in `accepted`.
pub fn collect_synthetic_precheck_rejections(
    obligations: &std::collections::VecDeque<crate::event_loop::loop_state::HatObligation>,
    accepted: &[ralph_proto::Event],
    checklist_len: impl Fn(&str) -> Option<usize>,
) -> Vec<SyntheticPrecheckRejection> {
    let mut out = Vec::new();
    for obligation in obligations {
        let gate_hat_id = obligation.hat_id.as_str();
        if !is_gate_hat(gate_hat_id) {
            continue;
        }
        let Some(guarded) = gate_topic(gate_hat_id) else {
            continue;
        };
        let outcome = classify_gate_emit_outcome(gate_hat_id, guarded, accepted);
        if matches!(outcome, GateEmitOutcome::Pass | GateEmitOutcome::Reject) {
            continue;
        }
        let total_checks = checklist_len(guarded);
        let payload_json = synthesize_rejection_payload(total_checks);
        let rejected_topic = format!("{guarded}.rejected");
        out.push(SyntheticPrecheckRejection {
            gate_hat_id: gate_hat_id.to_string(),
            guarded_topic: guarded.to_string(),
            rejected_topic,
            payload_json,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_topic_strips_prefix() {
        assert_eq!(gate_topic("precheck-review.complete"), Some("review.complete"));
        assert_eq!(gate_topic("precheck-build.done"), Some("build.done"));
        assert_eq!(gate_topic("ralph"), None);
        assert_eq!(gate_topic("builder"), None);
        assert_eq!(gate_topic(""), None);
    }

    #[test]
    fn is_gate_hat_matches_prefix() {
        assert!(is_gate_hat("precheck-review.complete"));
        assert!(is_gate_hat("precheck-x"));
        assert!(!is_gate_hat("ralph"));
        assert!(!is_gate_hat("builder"));
        assert!(!is_gate_hat(""));
    }

    #[test]
    fn derived_topics_appends_suffixes() {
        let (proposed, rejected) = derived_topics("review.complete");
        assert_eq!(proposed, "review.complete.proposed");
        assert_eq!(rejected, "review.complete.rejected");
    }

    #[test]
    fn is_satisfying_emit_matches_terminal_events() {
        assert!(is_satisfying_emit("review.complete", "review.complete"));
        assert!(is_satisfying_emit("review.complete.rejected", "review.complete"));
        assert!(!is_satisfying_emit("review.complete.proposed", "review.complete"));
        assert!(!is_satisfying_emit("other.event", "review.complete"));
    }

    #[test]
    fn synthetic_payload_marks_all_checks_failed() {
        let json = synthesize_rejection_payload(Some(3));
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["synthetic"], serde_json::Value::Bool(true));
        assert_eq!(parsed["reason"], "gate_silent_or_ambiguous");
        assert_eq!(
            parsed["failed_checks"],
            serde_json::json!([1, 2, 3])
        );
    }

    #[test]
    fn synthetic_payload_zero_checks_is_safe() {
        let json = synthesize_rejection_payload(Some(0));
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["failed_checks"].as_array().unwrap().is_empty());
        assert_eq!(parsed["synthetic"], serde_json::Value::Bool(true));
    }

    #[test]
    fn llm_payload_round_trips_with_synthetic_false() {
        let json = llm_rejection_payload(vec![1, 3], "missing file paths");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["synthetic"], serde_json::Value::Bool(false));
        assert_eq!(parsed["reason"], "missing file paths");
        assert_eq!(parsed["failed_checks"], serde_json::json!([1, 3]));
    }

    #[test]
    fn rejection_source_str_is_stable() {
        // The string form is part of the on-disk contract
        // (rejected_payload.synthetic is read by U6 routing and
        // surfaced in diagnostics).
        assert_eq!(RejectionSource::Llm.as_str(), "llm");
        assert_eq!(RejectionSource::Synthetic.as_str(), "synthetic");
    }

    #[test]
    fn collect_synthetic_when_gate_is_silent() {
        use crate::event_loop::loop_state::HatObligation;
        use ralph_proto::{Event, HatId};
        use std::time::Instant;

        let obligations = std::collections::VecDeque::from([HatObligation {
            hat_id: HatId::new("precheck-work.done"),
            trigger_topic: "work.done.proposed".to_string(),
            expected_topics: vec![
                "work.done".to_string(),
                "work.done.rejected".to_string(),
            ],
            created_at: Instant::now(),
            redispatch_count: 0,
        }]);
        let synthetics = collect_synthetic_precheck_rejections(
            &obligations,
            &[],
            |_| Some(2),
        );
        assert_eq!(synthetics.len(), 1);
        assert_eq!(synthetics[0].guarded_topic, "work.done");
        let parsed: serde_json::Value =
            serde_json::from_str(&synthetics[0].payload_json).unwrap();
        assert_eq!(parsed["synthetic"], true);
    }

    #[test]
    fn no_synthetic_when_gate_emits_reject() {
        use crate::event_loop::loop_state::HatObligation;
        use ralph_proto::{Event, HatId};
        use std::time::Instant;

        let obligations = std::collections::VecDeque::from([HatObligation {
            hat_id: HatId::new("precheck-work.done"),
            trigger_topic: "work.done.proposed".to_string(),
            expected_topics: vec![
                "work.done".to_string(),
                "work.done.rejected".to_string(),
            ],
            created_at: Instant::now(),
            redispatch_count: 0,
        }]);
        let accepted = vec![Event::new(
            "work.done.rejected",
            r#"{"failed_checks":[1],"reason":"no","synthetic":false}"#,
        )
        .with_source(HatId::new("precheck-work.done"))];
        let synthetics = collect_synthetic_precheck_rejections(
            &obligations,
            &accepted,
            |_| Some(1),
        );
        assert!(synthetics.is_empty());
    }
}
