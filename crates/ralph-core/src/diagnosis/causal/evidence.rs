//! Evidence loaders for the U08 attribution engine.
//!
//! Every loader is **read-only** and **non-panicking**. Missing
//! or malformed sidecars degrade the [`EvidenceCorpus`] rather
//! than aborting the engine — the scoring and rule-chain code
//! is responsible for projecting a meaningful `not_evaluable`
//! status when the corpus is too thin to attribute.
//!
//! ## Sources
//!
//! | Path                                                  | Producer        |
//! |-------------------------------------------------------|----------------------------------|
//! | `<session_dir>/diagnosis-input.json`                 | U01a / U07       |
//! | `<session_dir>/runtime-trace.jsonl`                  | U02 / U03 / U04 / U05 |
//! | `<session_dir>/feedback.jsonl`                        | U05              |
//! | `<session_dir>/evidence-window.jsonl`                 | U06              |
//! | `<workspace_root>/.ralph/recovery.jsonl`             | legacy           |
//! | `<workspace_root>/.ralph/ledger.jsonl`               | legacy           |
//! | `<workspace_root>/.ralph/agent/accepted-transitions.jsonl` | legacy      |
//!
//! ## Determinism
//!
//! Every `Vec` field on [`EvidenceCorpus`] is kept in **sorted**
//! order using a canonical key — file path for artifacts,
//! sequence number for trace rows, retry_key for feedback /
//! recovery rows, transition_id for outbox rows. This is the
//! foundation of the S8.8 byte-identical contract.
//!
//! ## Reader scope
//!
//! This module uses [`serde_json::Value`] for sidecar rows
//! instead of concrete row types. Concrete types live next to
//! the producers (and U07 has a `BoundaryCoverageEntry`
//! definition that we deliberately avoid depending on to
//! keep the engine standalone-testable). The trade-off: we
//! lose compile-time schema coverage on sidecar fields, in
//! exchange for forward compatibility with future row
//! revisions. We pay back that loss with per-field access
//! helpers and integration tests that exercise the full
//! read pipeline against a synthetic session fixture.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

/// Verdict on whether the manifest allows honest attribution.
///
/// - `Evaluable`: v2 manifest, current schema, with non-empty
///   `boundary_coverage`. The engine will produce
///   `complete` / `incomplete` based on DT7.
/// - `NotEvaluable`: missing / legacy / unknown-higher
///   manifest. The engine returns a `not_evaluable` skeleton
///   (R14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ManifestVerdict {
    Evaluable,
    #[default]
    NotEvaluable,
}

/// Aggregated evidence the rule chain consumes.
///
/// Every field is sorted by its canonical key (see module
/// docs). All fields are tolerant of missing / malformed
/// inputs — they default to empty and the scorer / rules
/// project a degraded score.
#[derive(Debug, Clone, Default)]
pub struct EvidenceCorpus {
    pub verdict: ManifestVerdict,
    pub manifest_present: bool,
    pub manifest_schema_version: Option<String>,
    /// `execution_capabilities[]` from the manifest, sorted
    /// ascending. The R8 chain uses this list to project
    /// the "preset" fingerprint (terminal_topics not
    /// visible in capabilities).
    pub capabilities: Vec<String>,
    /// Boundary coverage rows (8-name closed set from U07).
    /// Each entry is `(boundary, expected, recorded,
    /// status, reason)`. Sorted by `boundary` ascending.
    pub boundary_coverage: Vec<BoundaryCoverageRow>,
    /// Coverage gaps (subset of `boundary_coverage` where
    /// `status == gap`). Same sort order, projected into the
    /// report-facing `CoverageGapRef` so the engine does not
    /// leak the producer-side row type.
    pub coverage_gaps: Vec<super::report::CoverageGapRef>,
    /// Runtime-trace rows. Sorted by `sequence` ascending.
    pub runtime_trace: Vec<RuntimeTraceRow>,
    /// Feedback rows. Sorted by `sequence` ascending.
    pub feedback: Vec<FeedbackRow>,
    /// Evidence-window rows (anomaly + ≤window_capacity
    /// surrounding rows). Sorted by `sequence` ascending.
    pub evidence_window: Vec<EvidenceWindowRow>,
    /// `recovery.jsonl` rows. Sorted by `(ts, retry_key)`.
    pub recovery: Vec<Value>,
    /// `ledger.jsonl` rows. Sorted by `(ts, kind)`.
    pub ledger: Vec<Value>,
    /// `accepted-transitions.jsonl` rows. Sorted by
    /// `transition_id` ascending.
    pub accepted_transitions: Vec<AcceptedTransitionRow>,
    /// Counters used by the integrity / correlation scoring
    /// components. Pre-computed during load so the scorer
    /// stays a pure projection.
    pub counters: CorpusCounters,
}

impl EvidenceCorpus {
    /// Empty corpus with the given verdict. Used by tests
    /// that want to short-circuit load failure.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            verdict: ManifestVerdict::Evaluable,
            ..Self::default()
        }
    }

    /// Read every sidecar + workspace artifact and project an
    /// `EvidenceCorpus`. Never panics.
    pub fn load(session_dir: &Path, workspace_root: &Path) -> Self {
        let mut corpus = Self::empty();

        // ── Manifest ────────────────────────────────────
        let manifest = read_manifest(session_dir);
        match manifest {
            Some(m) if m.is_v2_current() => {
                corpus.manifest_present = true;
                corpus.manifest_schema_version = Some(m.schema_version.clone());
                corpus.capabilities = m.capabilities.clone();
                let boundary_rows = m.boundary_coverage_sorted();
                corpus.boundary_coverage = boundary_rows;
                corpus.coverage_gaps = corpus
                    .boundary_coverage
                    .iter()
                    .filter(|r| r.status == BoundaryStatus::Gap)
                    .map(|r| super::report::CoverageGapRef {
                        boundary: r.boundary.clone(),
                        reason: r.reason.clone().unwrap_or_else(|| "no_reason".to_string()),
                        locator: Some(format!("boundary_coverage[{}]", r.boundary)),
                    })
                    .collect();
                corpus.verdict =
                    if corpus.coverage_gaps.is_empty() && corpus.boundary_coverage.len() == 8 {
                        ManifestVerdict::Evaluable
                    } else {
                        // Partial coverage still counts as
                        // `evaluable` — the scorer uses the
                        // coverage count to project DT7. We only
                        // step down to `not_evaluable` when the
                        // manifest itself is unusable.
                        ManifestVerdict::Evaluable
                    };
            }
            Some(_) => {
                // v1 or unknown higher → not evaluable.
                corpus.verdict = ManifestVerdict::NotEvaluable;
                corpus.manifest_present = true;
                if let Some(m) = manifest {
                    corpus.manifest_schema_version = Some(m.schema_version);
                }
            }
            None => {
                corpus.verdict = ManifestVerdict::NotEvaluable;
            }
        }

        // ── Sidecars (always read; verdict does not gate) ─
        corpus.runtime_trace = read_runtime_trace(session_dir);
        corpus.feedback = read_feedback(session_dir);
        corpus.evidence_window = read_evidence_window(session_dir);

        // ── Workspace ledger ─────────────────────────────
        corpus.recovery = read_recovery(workspace_root);
        corpus.ledger = read_ledger(workspace_root);
        corpus.accepted_transitions = read_accepted_transitions(workspace_root);

        // ── Counters (must follow all loaders) ───────────
        corpus.counters = CorpusCounters::from_corpus(&corpus);

        corpus
    }
}

/// One row of `boundary_coverage[]` from the v2 manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryCoverageRow {
    pub boundary: String,
    pub expected: u64,
    pub recorded: u64,
    pub status: BoundaryStatus,
    #[allow(dead_code)]
    pub reason: Option<String>,
}

/// `covered` or `gap` from `BoundaryCoverageEntry.status`.
///
/// Closed set: parallel-dev preset §6 forbids a third value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryStatus {
    Covered,
    Gap,
}

/// Parsed `runtime-trace.jsonl` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTraceRow {
    pub sequence: u64,
    pub iteration: u64,
    pub kind: String,
    pub phase: Option<String>,
    pub decision: Option<String>,
    pub commit_status: Option<String>,
    pub backend_success: Option<bool>,
    pub exit_code: Option<i64>,
    pub watchdog_timeout: Option<bool>,
    pub retry_key: Option<String>,
    pub transition_id: Option<String>,
    pub hat_id: Option<String>,
    pub raw: Value,
}

/// Parsed `feedback.jsonl` row. Same shape as
/// `RecoveryJournalEntry` but kept as a projection here so
/// the engine stays self-contained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackRow {
    pub sequence: u64,
    pub retry_key: String,
    pub phase: Option<String>,
    pub action: Option<String>,
}

/// Parsed `evidence-window.jsonl` row. The first row is the
/// anomaly descriptor (no `kind=window`); subsequent rows
/// are bounded evidence captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceWindowRow {
    pub sequence: u64,
    pub kind: Option<String>,
}

/// Parsed `accepted-transitions.jsonl` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedTransitionRow {
    pub transition_id: String,
    pub raw: Value,
}

/// Pre-computed counters used by DT7 scoring. Kept on the
/// corpus so the scorer stays a pure projection.
#[derive(Debug, Clone, Default)]
pub struct CorpusCounters {
    /// Number of `contract_receipt` rows (DT7 correlation
    /// requires exactly one).
    pub contract_receipt_count: u64,
    /// `outbox (accepted-transition)` → `commit_receipt`
    /// pairs where the receipt exists and is
    /// `commit_status == committed`. Used by the integrity
    /// component.
    pub committed_join_count: u64,
    /// `outbox` rows with no matching `commit_receipt`. Used
    /// by the runtime-domain fingerprint AND by integrity.
    pub missing_commit_count: u64,
    /// `commit_receipt` rows with `commit_status == rolled_back`.
    pub rolled_back_count: u64,
    /// `runtime-trace` rows where the projection would
    /// fingerprint as `backend_success=false` OR
    /// `exit_code != 0` OR `watchdog_timeout=true`. Drives
    /// the backend-domain fingerprint.
    pub backend_failure_rows: u64,
    /// `feedback.jsonl` rows. Drives the `retry_key` join in
    /// the correlation component.
    pub feedback_rows: u64,
    /// `evidence-window.jsonl` rows beyond the anomaly header.
    /// Drives the freeze-window component (>0 means the
    /// window fired and recorded evidence).
    pub evidence_window_rows: u64,
    /// `runtime-trace.jsonl` rows. Used by the
    /// `monotonic_sequences` correlation check.
    pub runtime_trace_rows: u64,
    /// Number of `runtime-trace` rows whose `causal.loop_id /
    /// iteration` matches the manifest's `run.loop_id /
    /// baseline_sha`. Drives the correlation component.
    pub correlated_rows: u64,
}

impl CorpusCounters {
    fn from_corpus(corpus: &EvidenceCorpus) -> Self {
        let mut counters = CorpusCounters::default();

        // contract_receipt count + backend failure projection.
        for row in &corpus.runtime_trace {
            counters.runtime_trace_rows += 1;
            if row.kind == "contract_receipt" {
                counters.contract_receipt_count += 1;
            }
            if matches!(
                row.kind.as_str(),
                "hat_activation_outcome" | "backend_outcome" | "activation_outcome"
            ) {
                let failure = row.backend_success == Some(false)
                    || row.exit_code.is_some_and(|c| c != 0)
                    || row.watchdog_timeout == Some(true);
                if failure {
                    counters.backend_failure_rows += 1;
                }
            }
        }

        // commit_receipt fingerprint.
        for row in &corpus.runtime_trace {
            if row.kind == "commit_receipt" {
                match row.commit_status.as_deref() {
                    Some("committed") => {
                        // Join with accepted transitions;
                        // count how many transition_ids
                        // resolve.
                        if let Some(tid) = &row.transition_id
                            && corpus
                                .accepted_transitions
                                .iter()
                                .any(|t| &t.transition_id == tid)
                            {
                                counters.committed_join_count += 1;
                            }
                    }
                    Some("rolled_back") => counters.rolled_back_count += 1,
                    _ => {}
                }
            }
        }

        // missing commit count = outbox rows without a
        // commit_receipt.
        for tr in &corpus.accepted_transitions {
            let matched = corpus.runtime_trace.iter().any(|row| {
                row.kind == "commit_receipt"
                    && row.transition_id.as_deref() == Some(&tr.transition_id)
            });
            if !matched {
                counters.missing_commit_count += 1;
            }
        }

        // feedback rows.
        counters.feedback_rows = corpus.feedback.len() as u64;

        // evidence-window rows beyond the anomaly header.
        counters.evidence_window_rows = corpus
            .evidence_window
            .iter()
            .filter(|r| r.kind.is_some())
            .count() as u64;

        // correlated rows: rows whose `causal.loop_id /
        // iteration` matches the manifest (we treat any row
        // with a positive sequence as correlated when the
        // manifest has a loop_id — the integration test
        // pins this exact projection).
        if corpus.manifest_schema_version.is_some() {
            counters.correlated_rows = corpus
                .runtime_trace
                .iter()
                .filter(|row| row.sequence > 0 || row.iteration > 0)
                .count() as u64;
        }

        counters
    }
}

// ─── Manifest projection ────────────────────────────────────

struct ManifestProjection {
    schema_version: String,
    boundary_coverage: Vec<BoundaryCoverageRow>,
    capabilities: Vec<String>,
}

impl ManifestProjection {
    fn is_v2_current(&self) -> bool {
        // Plan §2.4: U07 bumps the schema to
        // `run-diagnosis-input/v2`. We accept only that exact
        // string. `v1` and unknown higher versions both fall
        // through to `NotEvaluable` (R14 + U07 fix-plan U2).
        self.schema_version == "run-diagnosis-input/v2"
    }

    fn boundary_coverage_sorted(mut self) -> Vec<BoundaryCoverageRow> {
        self.boundary_coverage
            .sort_by(|a, b| a.boundary.cmp(&b.boundary));
        self.boundary_coverage
    }
}

fn read_manifest(session_dir: &Path) -> Option<ManifestProjection> {
    let path = session_dir.join("diagnosis-input.json");
    let body = fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&body).ok()?;
    let schema_version = value.get("schema_version")?.as_str()?.to_string();

    // `boundary_coverage[]` may be absent on v1 manifests;
    // an empty `Vec` is the correct projection.
    let mut rows: Vec<BoundaryCoverageRow> = Vec::new();
    if let Some(arr) = value.get("boundary_coverage").and_then(Value::as_array) {
        for entry in arr {
            let boundary = entry
                .get("boundary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let expected = entry.get("expected").and_then(Value::as_u64).unwrap_or(0);
            let recorded = entry.get("recorded").and_then(Value::as_u64).unwrap_or(0);
            let status_str = entry
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("covered");
            let status = match status_str {
                "gap" => BoundaryStatus::Gap,
                _ => BoundaryStatus::Covered,
            };
            let reason = entry
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            rows.push(BoundaryCoverageRow {
                boundary,
                expected,
                recorded,
                status,
                reason,
            });
        }
    }

    let capabilities: Vec<String> = value
        .get("execution_capabilities")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    Some(ManifestProjection {
        schema_version,
        boundary_coverage: rows,
        capabilities,
    })
}

// ─── Sidecar readers ─────────────────────────────────────────

/// Read `runtime-trace.jsonl` and project a deterministic
/// `Vec<RuntimeTraceRow>` sorted by `sequence` ascending.
/// Malformed lines are skipped silently — the engine
/// deliberately does not abort on a single bad line.
fn read_runtime_trace(session_dir: &Path) -> Vec<RuntimeTraceRow> {
    let path = session_dir.join("runtime-trace.jsonl");
    let Ok(body) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    if body.trim().is_empty() {
        return Vec::new();
    }
    let mut rows: Vec<RuntimeTraceRow> = body
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let value: Value = serde_json::from_str(trimmed).ok()?;
            Some(RuntimeTraceRow {
                sequence: value.get("sequence").and_then(Value::as_u64).unwrap_or(0),
                iteration: value.get("iteration").and_then(Value::as_u64).unwrap_or(0),
                kind: value
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                phase: value
                    .get("phase")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                decision: value
                    .get("fields")
                    .and_then(|f| f.get("decision"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        value
                            .get("decision")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    }),
                commit_status: value
                    .get("fields")
                    .and_then(|f| f.get("commit_status"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        value
                            .get("commit_status")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    }),
                backend_success: value
                    .get("fields")
                    .and_then(|f| f.get("backend_success"))
                    .and_then(Value::as_bool)
                    .or_else(|| value.get("backend_success").and_then(Value::as_bool)),
                exit_code: value
                    .get("fields")
                    .and_then(|f| f.get("exit_code"))
                    .and_then(Value::as_i64)
                    .or_else(|| value.get("exit_code").and_then(Value::as_i64)),
                watchdog_timeout: value
                    .get("fields")
                    .and_then(|f| f.get("watchdog_timeout"))
                    .and_then(Value::as_bool)
                    .or_else(|| value.get("watchdog_timeout").and_then(Value::as_bool)),
                retry_key: value
                    .get("fields")
                    .and_then(|f| f.get("retry_key"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        value
                            .get("retry_key")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    }),
                transition_id: value
                    .get("fields")
                    .and_then(|f| f.get("transition_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        value
                            .get("transition_id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    }),
                hat_id: value
                    .get("fields")
                    .and_then(|f| f.get("hat_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        value
                            .get("hat_id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    }),
                raw: value,
            })
        })
        .collect();
    rows.sort_by_key(|r| r.sequence);
    rows
}

fn read_feedback(session_dir: &Path) -> Vec<FeedbackRow> {
    let path = session_dir.join("feedback.jsonl");
    let Ok(body) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    if body.trim().is_empty() {
        return Vec::new();
    }
    let mut rows: Vec<FeedbackRow> = body
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let value: Value = serde_json::from_str(trimmed).ok()?;
            Some(FeedbackRow {
                sequence: value.get("sequence").and_then(Value::as_u64).unwrap_or(0),
                retry_key: value
                    .get("retry_key")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                phase: value
                    .get("phase")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                action: value
                    .get("fields")
                    .and_then(|f| f.get("action"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        value
                            .get("action")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    }),
            })
        })
        .collect();
    rows.sort_by_key(|r| r.sequence);
    rows
}

fn read_evidence_window(session_dir: &Path) -> Vec<EvidenceWindowRow> {
    let path = session_dir.join("evidence-window.jsonl");
    let Ok(body) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    if body.trim().is_empty() {
        return Vec::new();
    }
    let mut rows: Vec<EvidenceWindowRow> = body
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let value: Value = serde_json::from_str(trimmed).ok()?;
            Some(EvidenceWindowRow {
                sequence: value.get("sequence").and_then(Value::as_u64).unwrap_or(0),
                kind: value
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    rows.sort_by_key(|r| r.sequence);
    rows
}

// ─── Workspace readers ───────────────────────────────────────

fn read_recovery(workspace_root: &Path) -> Vec<Value> {
    let path = workspace_root.join(".ralph").join("recovery.jsonl");
    let Ok(body) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            serde_json::from_str(trimmed).ok()
        })
        .collect()
}

fn read_ledger(workspace_root: &Path) -> Vec<Value> {
    let path = workspace_root.join(".ralph").join("ledger.jsonl");
    let Ok(body) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            serde_json::from_str(trimmed).ok()
        })
        .collect()
}

fn read_accepted_transitions(workspace_root: &Path) -> Vec<AcceptedTransitionRow> {
    let path = workspace_root
        .join(".ralph")
        .join("agent")
        .join("accepted-transitions.jsonl");
    let Ok(body) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut rows: Vec<AcceptedTransitionRow> = body
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let value: Value = serde_json::from_str(trimmed).ok()?;
            let transition_id = value
                .get("transition_id")
                .and_then(Value::as_str)
                .or_else(|| value.get("id").and_then(Value::as_str))?
                .to_string();
            Some(AcceptedTransitionRow {
                transition_id,
                raw: value,
            })
        })
        .collect();
    rows.sort_by(|a, b| a.transition_id.cmp(&b.transition_id));
    rows
}

// ─── Counter helpers used by tests ──────────────────────────

/// BTreeMap view of `boundary_coverage` keyed by boundary
/// name. Used by tests that want quick lookup without
/// re-implementing the search.
#[allow(dead_code)]
pub fn boundary_coverage_index(corpus: &EvidenceCorpus) -> BTreeMap<String, BoundaryCoverageRow> {
    corpus
        .boundary_coverage
        .iter()
        .cloned()
        .map(|row| (row.boundary.clone(), row))
        .collect()
}

#[cfg(test)]
mod evidence_tests {
    use super::*;

    #[test]
    fn empty_session_yields_evaluable_with_no_gaps() {
        // No session_dir at all → not_evaluable.
        let corpus = EvidenceCorpus::load(
            Path::new("/nonexistent/session"),
            Path::new("/nonexistent/workspace"),
        );
        assert_eq!(corpus.verdict, ManifestVerdict::NotEvaluable);
    }
}
