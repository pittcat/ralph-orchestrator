//! Output schema for the U08 deterministic causal attribution
//! engine.
//!
//! Every field on [`CausalAttributionReport`] is part of the
//! public contract consumed by the U09 CLI renderer and the
//! U10 diagnosis skill. The serde renames pin the on-the-wire
//! spellings; the in-memory struct field names are
//! intentionally descriptive (operator-friendly) while the
//! JSON shape is operator-machine-friendly.
//!
//! Stability rules (parallel-dev preset §13.4):
//!
//! - Renaming a field is a breaking change. Bump the
//!   [`super::CAUSAL_ATTRIBUTION_CONTRACT_VERSION`] when you
//!   do.
//! - Adding a new field is non-breaking only when the new
//!   field is `Option<T>` with `#[serde(default,
//!   skip_serializing_if = "Option::is_none")]` (or `Vec<T>`
//!   with `#[serde(default)]`).
//! - Every domain-relevant collection is `Vec` with a
//!   deterministic ordering rule pinned by a test (see
//!   `super::deterministic_tests`).

use serde::{Deserialize, Serialize};

use super::domain::Domain;

/// Final status of an attribution.
///
/// The transition table is:
///
/// | Condition                                          | Status        |
/// |----------------------------------------------------|----------------------------------|
/// | Missing / legacy manifest                          | `not_evaluable` |
/// | Manifest present and `confidence.total > 85`      | `complete`     |
/// | Manifest present but `confidence.total <= 85`     | `incomplete`   |
///
/// `complete` requires `confidence.total > 85` strictly.
/// `=` is treated as incomplete to keep the rule conservative
/// — the parallel-dev preset DT7 explicitly requires strict
/// `>` (R10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionStatus {
    Complete,
    Incomplete,
    NotEvaluable,
}

/// Concrete repair point — what the operator should look at
/// first.
///
/// Different variants carry different evidence shapes; the
/// reporter (U09) renders them as markdown with a stable
/// ordering: `category` then `target` then `evidence` then
/// `summary`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "category")]
pub enum FixPoint {
    /// A backend execution row flagged failure. `target`
    /// names the hat_id / backend / iteration; `evidence` is
    /// a structured pointer (path:line-style or
    /// `runtime-trace.jsonl:L<seq>`).
    Backend {
        target: String,
        evidence: String,
        summary: String,
    },
    /// A transition's commit receipt is missing or
    /// rolled_back.
    Runtime {
        target: String,
        evidence: String,
        summary: String,
    },
    /// A required field is missing from the preset's
    /// effective contract.
    Preset {
        target: String,
        evidence: String,
        summary: String,
    },
    /// The agent failed to emit the expected terminal event
    /// despite a healthy preset / runtime / backend.
    Agent {
        target: String,
        evidence: String,
        summary: String,
    },
    /// One or more `boundary_coverage` rows are `gap`; the
    /// repair is to add the missing capture points.
    CaptureContract {
        target: String,
        evidence: String,
        summary: String,
    },
}

impl FixPoint {
    /// Domain this fix point belongs to. Used by the rule
    /// chain to confirm `primary_domain` and `fix_point`
    /// agree.
    #[must_use]
    pub const fn domain(&self) -> Domain {
        match self {
            FixPoint::Backend { .. } => Domain::Backend,
            FixPoint::Runtime { .. } => Domain::Runtime,
            FixPoint::Preset { .. } => Domain::Preset,
            FixPoint::Agent { .. } => Domain::Agent,
            FixPoint::CaptureContract { .. } => Domain::DiagnosticCaptureContract,
        }
    }
}

/// One evidence reference the operator can follow. Always
/// workspace-relative; absolute paths would defeat the
/// "machine-checked" claim because they leak environment
/// state into the report.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// Logical kind: `"diagnosis-input.json"`,
    /// `"runtime-trace.jsonl"`, `"recovery.jsonl"`, etc.
    pub kind: String,
    /// Stable pointer inside the artifact:
    /// `"L42"` for a line, `"seq=7"` for a sequence number,
    /// `"contract_digest"` for a manifest field, etc.
    pub locator: String,
    /// One-line human-readable note.
    pub note: String,
}

impl EvidenceRef {
    /// Convenience constructor used by both the rules and the
    /// report builders.
    #[must_use]
    pub fn new(
        kind: impl Into<String>,
        locator: impl Into<String>,
        note: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            locator: locator.into(),
            note: note.into(),
        }
    }
}

/// Reference to a coverage gap on the manifest side. Always
/// references the boundary name (8-name closed set from
/// `diagnostics::CausalBoundary`) and a free-form reason
/// captured by the collector.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CoverageGapRef {
    pub boundary: String,
    pub reason: String,
    /// Optional in-band reference into the manifest itself
    /// (e.g. `boundary_coverage[3].reason`). Always present
    /// when the gap came from a structured annotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
}

/// One rejected hypothesis — a non-primary domain that did
/// NOT cause the failure, paired with the evidence that
/// refutes it.
///
/// DT7 requires every non-primary domain to have at least one
/// refuting evidence reference (the `refutation` component is
/// capped at 5 points per domain). Empty refutations lower
/// the confidence score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedHypothesis {
    pub domain: Domain,
    /// Why this domain is *not* the cause. Stable, machine-
    /// readable string. e.g. `"backend.outcome.success=true"`,
    /// `"commit_receipt.committed=true"`,
    /// `"contract_digest.terminal_topics_present=true"`.
    pub refutation: String,
    /// The evidence reference that backs the refutation.
    /// Always at least one entry (DT7 requires it).
    pub evidence: Vec<EvidenceRef>,
}

/// DT7 confidence breakdown. Five independent components,
/// each capped to its share of 100; `total` is the sum
/// (capped at 100) and the gate is `total > 85`.
///
/// The breakdown is intentionally additive — not weighted —
/// because every component is a hard requirement on the
/// evidence chain. Dropping any one of them is observable
/// evidence loss, not a soft preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfidenceBreakdown {
    /// 30 — every `boundary_coverage` row is `covered`
    /// (`expected == recorded`).
    pub coverage: u8,
    /// 25 — outbox ↔ commit_receipt join complete; retry_key
    /// join complete; sequence monotonic.
    pub integrity: u8,
    /// 20 — each of the four non-primary domains has at least
    /// one refuting evidence reference.
    pub refutation: u8,
    /// 15 — exactly one `contract_receipt`; `causal.loop_id /
    /// iteration` consistent across rows; sequence monotonic.
    pub correlation: u8,
    /// 10 — when an anomaly triggered the freeze window,
    /// `evidence-window.jsonl` exists and is well-formed.
    pub freeze_window: u8,
    /// Sum of the five components, capped at 100.
    pub total: u8,
}

/// The final output. Always returned by
/// [`super::analyze_session`]; never partially populated.
///
/// Determinism contract (S8.8): two runs on identical inputs
/// produce byte-identical JSON. The order of every `Vec`
/// field is canonical: insertion order is itself deterministic
/// (we sort by canonical key before insertion), and
/// `BTreeMap` is used internally for any keyed collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CausalAttributionReport {
    /// `causal-attribution/v1` for the current contract.
    /// Mirrors [`super::CAUSAL_ATTRIBUTION_CONTRACT_VERSION`]
    /// in the serialized output so consumers can pin it
    /// without a sidecar file.
    pub contract_version: String,
    pub status: AttributionStatus,
    /// `None` only on `not_evaluable`. On both `complete` and
    /// `incomplete` the engine still surfaces its
    /// best-supported domain so the operator can act while
    /// the score is being audited (R9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_domain: Option<Domain>,
    /// `None` only on `not_evaluable`. When present, agrees
    /// with `primary_domain` (test pinned).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_point: Option<FixPoint>,
    pub confidence: ConfidenceBreakdown,
    /// 4 entries on `complete` / `incomplete`, 0 on
    /// `not_evaluable` (no domain was evaluated, so nothing
    /// to reject).
    pub rejected_hypotheses: Vec<RejectedHypothesis>,
    /// 0+ entries; sourced directly from
    /// `diagnosis-input.json::boundary_coverage` rows where
    /// `status == gap`.
    pub coverage_gaps: Vec<CoverageGapRef>,
    /// 1+ entries on `complete` / `incomplete`. Workspace-
    /// relative pointers; see [`EvidenceRef`].
    pub evidence_refs: Vec<EvidenceRef>,
}

/// Builder helper. Owns the assembly rules so `causal.rs` is
/// just orchestration.
pub(crate) fn build_report(
    contract_version: &str,
    corpus: super::evidence::EvidenceCorpus,
    attribution: super::rules::Attribution,
    confidence: ConfidenceBreakdown,
) -> CausalAttributionReport {
    use super::evidence::ManifestVerdict;

    let status = compute_status(&corpus, &confidence, attribution.primary_domain());

    // Coverage gaps flow straight from the manifest verdict;
    // they are independent of the rule-chain outcome.
    let coverage_gaps = corpus.coverage_gaps.clone();

    // On `not_evaluable`, every domain is undefined — we
    // return a bare skeleton with no `primary_domain`, no
    // `fix_point`, no `rejected_hypotheses`, and only the
    // score (zeroed) so consumers can iterate safely.
    if matches!(corpus.verdict, ManifestVerdict::NotEvaluable) {
        return CausalAttributionReport {
            contract_version: contract_version.to_string(),
            status: AttributionStatus::NotEvaluable,
            primary_domain: None,
            fix_point: None,
            confidence,
            rejected_hypotheses: Vec::new(),
            coverage_gaps,
            evidence_refs: Vec::new(),
        };
    }

    let primary_domain = attribution.primary_domain();
    let fix_point = attribution.fix_point();

    // Evidence refs come straight from the rule chain so they
    // stay in sync with the fingerprint that produced the
    // domain.
    let evidence_refs = attribution.evidence_refs();

    // Rejected hypotheses = the four non-primary domains,
    // each with at least one refuting evidence reference.
    // The order is canonical (Domain::ALL order) so the JSON
    // shape is stable across runs.
    let rejected_hypotheses = attribution.rejected_hypotheses();

    CausalAttributionReport {
        contract_version: contract_version.to_string(),
        status,
        primary_domain,
        fix_point,
        confidence,
        rejected_hypotheses,
        coverage_gaps,
        evidence_refs,
    }
}

/// Compute the final status. Pulled out so both the
/// integration test and the builder can call it.
///
/// `status == complete` requires:
///   1. manifest verdict is evaluable, AND
///   2. at least one domain matched the rule chain, AND
///   3. `confidence.total > 85` strictly (DT7 R10).
///
/// All other evaluable cases are `incomplete` — we still
/// return the suspect domain so the operator can act on it.
fn compute_status(
    corpus: &super::evidence::EvidenceCorpus,
    confidence: &ConfidenceBreakdown,
    primary: Option<Domain>,
) -> AttributionStatus {
    use super::evidence::ManifestVerdict;

    if matches!(corpus.verdict, ManifestVerdict::NotEvaluable) {
        return AttributionStatus::NotEvaluable;
    }
    if confidence.total > 85 && primary.is_some() {
        AttributionStatus::Complete
    } else {
        AttributionStatus::Incomplete
    }
}

#[cfg(test)]
mod report_tests {
    use super::*;
    use crate::diagnosis::causal::evidence::{EvidenceCorpus, ManifestVerdict};
    use crate::diagnosis::causal::rules::Attribution;

    #[test]
    fn contract_version_propagates_from_input() {
        // The serialized contract_version must come from the
        // engine's constant, not be hard-coded in the
        // builder, so U09/U10 pin a single source of truth.
        let corpus = EvidenceCorpus {
            verdict: ManifestVerdict::NotEvaluable,
            ..EvidenceCorpus::empty()
        };
        let attribution = Attribution::none();
        let report = build_report(
            "causal-attribution/v1",
            corpus,
            attribution,
            ConfidenceBreakdown {
                coverage: 0,
                integrity: 0,
                refutation: 0,
                correlation: 0,
                freeze_window: 0,
                total: 0,
            },
        );
        assert_eq!(report.contract_version, "causal-attribution/v1");
    }

    #[test]
    fn not_evaluable_strips_domain_specific_fields() {
        let corpus = EvidenceCorpus {
            verdict: ManifestVerdict::NotEvaluable,
            ..EvidenceCorpus::empty()
        };
        let report = build_report(
            "v1",
            corpus,
            Attribution::none(),
            ConfidenceBreakdown {
                coverage: 0,
                integrity: 0,
                refutation: 0,
                correlation: 0,
                freeze_window: 0,
                total: 0,
            },
        );
        assert_eq!(report.status, AttributionStatus::NotEvaluable);
        assert!(report.primary_domain.is_none());
        assert!(report.fix_point.is_none());
        assert!(report.rejected_hypotheses.is_empty());
        assert!(report.evidence_refs.is_empty());
    }
}
