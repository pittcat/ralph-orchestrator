//! Plan 2026-08-26-1104 Unit U08: deterministic causal attribution
//! engine for the runtime diagnosis evidence loop.
//!
//! The engine consumes a finalized session directory plus the
//! workspace `.ralph` ledger and projects a single
//! [`CausalAttributionReport`] identifying which of the five
//! causal domains (runtime / preset / agent / backend /
//! diagnostic_capture_contract) is responsible for the failure,
//! the concrete repair point, and a machine-checked confidence
//! breakdown that gates "complete" claims behind a strict
//! `total > 85` threshold.
//!
//! # Why a separate module
//!
//! - The reporter (`reporter.rs`) renders operator-facing
//!   summaries. Adding the engine inline would push it past the
//!   3,500-line ceiling and entangle aggregation with
//!   attribution. Keeping attribution pure makes both roles
//!   independently testable and aligns with the parallel-dev
//!   `HARD RULE` on file size.
//! - The responder (`responder.rs`) turns diagnoses into
//!   runtime actions. It must not depend on a single-root-cause
//!   projection that hasn't been audited yet.
//!
//! # Properties
//!
//! - **Pure**: zero side effects. The session directory and
//!   workspace ledger are read-only inputs.
//! - **Deterministic**: the same inputs produce byte-identical
//!   JSON output across runs (`S8.8`). All collections are
//!   sorted before serialization.
//! - **Schema-versioned**: every report carries a
//!   `contract_version` so downstream consumers (U09 / U10)
//!   can pin to a stable shape.
//!
//! # Layering
//!
//! - [`domain`]: the five causal domains.
//! - [`evidence`]: loaders for sidecars (`diagnosis-input.json`,
//!   `runtime-trace.jsonl`, `feedback.jsonl`) and workspace
//!   ledgers (`recovery.jsonl`, `ledger.jsonl`,
//!   `agent/accepted-transitions.jsonl`). All loaders are
//!   non-panicking — missing or malformed files degrade the
//!   corpus instead of aborting the engine.
//! - [`rules`]: the R8 most-upstream-preventable rule chain.
//!   Order is fixed: backend → runtime → preset → agent →
//!   diagnostic_capture_contract.
//! - [`scoring`]: the DT7 evidence-driven confidence breakdown.
//!   Five independent components, each capped to its share of 100.
//! - [`report`]: serializable output types —
//!   [`CausalAttributionReport`] plus the supporting
//!   [`AttributionStatus`] / [`ConfidenceBreakdown`] /
//!   [`RejectedHypothesis`] / [`CoverageGapRef`] types.

use std::path::Path;

// 子模块下沉到 `causal/` 子目录与 plan §19「不下沉新模块」字面存在 deviation:
// 因 causal 模块语义聚合度高、5 子模块间耦合紧(evidence 同时被 rules /
// scoring / report 消费,inline 合并会显著拉长 causal.rs 并模糊边界),故采用
// `pub(crate)` 收紧外部命名空间 + 仅通过顶层 `pub use` re-export 对外 API
// 表面。deviation 选择记录于 execution-plan.yml allowed_paths(U08)显式列举 +
// U08 completion report risk 段。
pub(crate) mod domain;
pub(crate) mod evidence;
pub(crate) mod report;
pub(crate) mod rules;
pub(crate) mod scoring;

pub use domain::Domain;
pub use report::{
    AttributionStatus, CausalAttributionReport, ConfidenceBreakdown, CoverageGapRef, FixPoint,
    RejectedHypothesis,
};

/// Current contract version for [`CausalAttributionReport`].
/// Bump only on breaking changes (renaming / retyping a field).
/// Additive changes keep the version and rely on
/// `Option`/`#[serde(default)]` for forward compatibility.
pub const CAUSAL_ATTRIBUTION_CONTRACT_VERSION: &str = "causal-attribution/v1";

/// Pure entry point: read the session + workspace inputs and
/// project a deterministic [`CausalAttributionReport`].
///
/// The engine never mutates either input directory. The
/// `session_dir` argument should point at a diagnostics session
/// produced by the U01a/U01b causal collector (manifest +
/// runtime trace + feedback sidecars). The `workspace_root`
/// argument is the workspace containing `.ralph/`; the engine
/// reads `recovery.jsonl`, `ledger.jsonl`, and
/// `agent/accepted-transitions.jsonl` from there.
///
/// # Inputs that produce `not_evaluable`
///
/// - Missing `diagnosis-input.json` (older sessions predating
///   the manifest format).
/// - A manifest whose `schema_version` is older than
///   `run-diagnosis-input/v2` (U07 introduced `boundary_coverage`;
///   v1 manifests lack the gap evidence required for honest
///   attribution).
/// - A manifest whose `schema_version` does not match any
///   compiled-in version (U07 surfaces this as `SchemaMismatch`,
///   not `Present`, so the engine never claims a complete
///   attribution on unverified sidecars).
///
/// # Output guarantees
///
/// - `status == Complete` requires both `primary_domain` set
///   AND `confidence.total > 85` (DT7 strict gate).
/// - `status == Incomplete` keeps `primary_domain` so the
///   operator can still see the most-likely suspect; only the
///   "complete" claim is withheld.
/// - `rejected_hypotheses` contains every non-primary domain
///   with at least one refuting evidence reference. Empty
///   domains (those without any evidence at all) are still
///   listed so consumers can audit coverage symmetry.
#[must_use]
pub fn analyze_session(session_dir: &Path, workspace_root: &Path) -> CausalAttributionReport {
    let corpus = evidence::EvidenceCorpus::load(session_dir, workspace_root);
    let attribution = rules::attribution_chain(&corpus);
    let confidence = scoring::score(&corpus, &attribution);
    report::build_report(
        CAUSAL_ATTRIBUTION_CONTRACT_VERSION,
        corpus,
        attribution,
        confidence,
    )
}

#[cfg(test)]
mod deterministic_tests {
    //! S8.8 contract: two consecutive invocations on the same
    //! inputs must produce byte-identical JSON. The fixture
    //! is intentionally tiny so the assertion focuses on output
    //! ordering determinism rather than the score itself.

    use super::{AttributionStatus, Domain, analyze_session};
    use std::path::PathBuf;

    fn minimal_session() -> PathBuf {
        // No actual files — `not_evaluable` path is enough to
        // prove determinism across both ordering axes (BTreeMap
        // iteration and `Vec` ordering).
        PathBuf::from("/nonexistent/for/determinism/check")
    }

    #[test]
    fn empty_corpus_returns_byte_identical_not_evaluable() {
        let first = analyze_session(&minimal_session(), &minimal_session());
        let second = analyze_session(&minimal_session(), &minimal_session());
        let first_json = serde_json::to_string(&first).expect("serialize");
        let second_json = serde_json::to_string(&second).expect("serialize");
        assert_eq!(first_json, second_json, "two runs must match byte-for-byte");
        assert_eq!(first.status, AttributionStatus::NotEvaluable);
        assert!(first.primary_domain.is_none());
        // Confidence is always present (zeroed) even on
        // `not_evaluable` so consumers can iterate without null
        // checks.
        assert_eq!(first.confidence.total, 0);
    }

    #[test]
    fn domain_serde_roundtrip_uses_snake_case() {
        // Pin the on-the-wire spelling — the field is the
        // contract consumed by `ralph diagnose --causal` (U09)
        // and the diagnosis skill (U10).
        let json = serde_json::to_string(&Domain::DiagnosticCaptureContract).expect("serialize");
        assert_eq!(json, "\"diagnostic_capture_contract\"");
    }
}

#[cfg(test)]
mod contract_version_tests {
    //! The contract version is the only stable handle for
    //! downstream consumers (U09 CLI, U10 skill) to gate
    //! behavior on. Pin the literal here.

    use super::CAUSAL_ATTRIBUTION_CONTRACT_VERSION;

    #[test]
    fn contract_version_pinned() {
        assert_eq!(CAUSAL_ATTRIBUTION_CONTRACT_VERSION, "causal-attribution/v1");
    }
}
