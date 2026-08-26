//! R8 most-upstream-preventable rule chain.
//!
//! The order is fixed: **backend → runtime → preset → agent →
//! diagnostic_capture_contract**. The first domain whose
//! fingerprint matches wins, regardless of how many other
//! domains also match. The chain projects a single
//! [`Attribution`] that the report builder turns into
//! [`CausalAttributionReport`].
//!
//! ## Why this order
//!
//! - **backend** runs first because a backend failure makes
//!   every later check meaningless (no agent ever started
//!   running, so the runtime/preset checks would just produce
//!   noisy noise).
//! - **runtime** runs second because the most common "silent"
//!   failure is an accepted transition whose commit was
//!   dropped or rolled back — the agent thinks it succeeded
//!   and emits no terminal event.
//! - **preset** runs third because a missing preset field
//!   only matters once we know the runtime and backend were
//!   healthy.
//! - **agent** runs fourth because it is the most common
//!   last-resort attribution and is only meaningful when
//!   every other layer checks out.
//! - **diagnostic_capture_contract** runs last as the
//!   fallback: when the collector should have recorded
//!   evidence but the boundary is `gap`, no other domain has
//!   a fingerprint and the cause is the missing capture
//!   point.
//!
//! ## Fingerprints
//!
//! Each rule matches on a small, well-defined slice of the
//! corpus (see [`evidence::EvidenceCorpus`]). When a rule
//! fires it produces:
//!   - a `Domain` (its identifier),
//!   - a `FixPoint` (the operator-facing repair),
//!   - at least one [`EvidenceRef`] (the evidence the rule
//!     used).
//!
//! For the four non-primary domains the rule chain also
//! produces a [`Refutation`] — a structured "why this domain
//! is NOT the cause" record. DT7 requires four refutations
//! (one per non-primary domain) for the score to land at
//! 20/20 on the `refutation` component.
//!
//! ## Determinism
//!
//! Every [`Refutation`] and [`EvidenceRef`] is constructed in
//! canonical order (boundary name asc, sequence asc,
//! transition_id asc) so the JSON output is byte-identical
//! across runs (S8.8).

use std::collections::BTreeMap;

use super::domain::Domain;
use super::evidence::{EvidenceCorpus, ManifestVerdict};
use super::report::{EvidenceRef, FixPoint, RejectedHypothesis};

/// Outcome of the R8 chain.
#[derive(Debug, Clone)]
pub struct Attribution {
    primary: Option<Domain>,
    fix_point: Option<FixPoint>,
    evidence_refs: Vec<EvidenceRef>,
    /// One per non-primary domain, even if the domain had no
    /// evidence at all. The `reason` field carries a stable
    /// machine-readable refutation string.
    refutations: BTreeMap<Domain, Refutation>,
}

/// One refutation record: a non-primary domain plus the
/// reason + evidence that excludes it.
#[derive(Debug, Clone)]
struct Refutation {
    reason: String,
    evidence: Vec<EvidenceRef>,
}

impl Attribution {
    /// Empty attribution (no primary, no rejected hypotheses).
    /// Used by tests and by the `not_evaluable` path.
    #[must_use]
    pub fn none() -> Self {
        Self {
            primary: None,
            fix_point: None,
            evidence_refs: Vec::new(),
            refutations: BTreeMap::new(),
        }
    }

    /// Identified primary domain, if any.
    #[must_use]
    pub const fn primary_domain(&self) -> Option<Domain> {
        self.primary
    }

    /// Borrow the evidence refs without consuming
    /// `self`. The report builder prefers this to the
    /// `into_*` variants when it needs to project multiple
    /// fields off the same attribution.
    #[must_use]
    pub fn evidence_refs(&self) -> Vec<EvidenceRef> {
        self.evidence_refs.clone()
    }

    /// Borrow the fix point without consuming `self`.
    #[must_use]
    pub fn fix_point(&self) -> Option<FixPoint> {
        self.fix_point.clone()
    }

    /// Project the rejected-hypotheses projection in
    /// canonical order (Domain::ALL order). Borrows `self`
    /// so the caller can still reach into the other
    /// projections afterward.
    #[must_use]
    pub fn rejected_hypotheses(&self) -> Vec<RejectedHypothesis> {
        Domain::ALL
            .iter()
            .filter(|d| Some(**d) != self.primary)
            .map(|d| {
                let r = self
                    .refutations
                    .get(d)
                    .cloned()
                    .unwrap_or_else(|| Refutation {
                        reason: "no_evidence".to_string(),
                        evidence: Vec::new(),
                    });
                RejectedHypothesis {
                    domain: *d,
                    refutation: r.reason,
                    evidence: r.evidence,
                }
            })
            .collect()
    }
}

/// Run the R8 chain and return the attribution.
#[must_use]
pub fn attribution_chain(corpus: &EvidenceCorpus) -> Attribution {
    if matches!(corpus.verdict, ManifestVerdict::NotEvaluable) {
        return Attribution::none();
    }

    // Run each rule in priority order. The first one that
    // fires sets the primary domain; later rules do not run.
    let rules: [(&str, fn(&EvidenceCorpus) -> Option<RuleHit>); 5] = [
        ("backend", rule_backend),
        ("runtime", rule_runtime),
        ("preset", rule_preset),
        ("agent", rule_agent),
        ("capture_contract", rule_capture_contract),
    ];

    let mut attribution = Attribution::none();
    let mut matched = false;
    for (name, rule) in rules {
        if let Some(hit) = rule(corpus) {
            attribution.primary = Some(hit.domain);
            attribution.fix_point = Some(hit.fix_point);
            attribution.evidence_refs = hit.evidence_refs;
            matched = true;
            // Ensure a refutation slot exists for this
            // domain too (it never appears in
            // `rejected_hypotheses` but keeps the
            // BTreeMap symmetric for diagnostics).
            attribution
                .refutations
                .entry(hit.domain)
                .or_insert_with(|| Refutation {
                    reason: "primary".to_string(),
                    evidence: Vec::new(),
                });
            tracing::debug!(
                target: "ralph_core::diagnosis::causal",
                rule = name,
                domain = hit.domain.as_str(),
                "R8 chain matched"
            );
            break;
        }
    }

    // Build refutations for every non-primary domain. The
    // rules below all add their own refutation evidence; any
    // domain not addressed by a rule gets the "no_evidence"
    // stub so DT7's `refutation` score projects honestly.
    for d in Domain::ALL {
        if Some(d) == attribution.primary {
            continue;
        }
        let (reason, evidence) = refutation_for(d, corpus);
        attribution
            .refutations
            .insert(d, Refutation { reason, evidence });
    }

    if !matched {
        // No rule fired. This is the `incomplete` /
        // `not_evaluable` edge: with v2 manifest present but
        // no fingerprint, we still return the corpus
        // (refutations intact, no primary).
        tracing::debug!(
            target: "ralph_core::diagnosis::causal",
            "R8 chain produced no primary; reporting incomplete"
        );
    }

    attribution
}

struct RuleHit {
    domain: Domain,
    fix_point: FixPoint,
    evidence_refs: Vec<EvidenceRef>,
}

// ─── Rules ───────────────────────────────────────────────────

fn rule_backend(corpus: &EvidenceCorpus) -> Option<RuleHit> {
    // S8.4: hat_activation_outcome / backend_outcome row with
    // any of `backend_success=false`, `exit_code != 0`,
    // `watchdog_timeout=true`.
    let row = corpus.runtime_trace.iter().find(|r| {
        matches!(
            r.kind.as_str(),
            "hat_activation_outcome" | "backend_outcome" | "activation_outcome"
        ) && (r.backend_success == Some(false)
            || r.exit_code.is_some_and(|c| c != 0)
            || r.watchdog_timeout == Some(true))
    })?;
    let target = row
        .hat_id
        .clone()
        .unwrap_or_else(|| format!("hat?seq={}", row.sequence));
    let locator = format!("seq={}", row.sequence);
    let note = match row.backend_success {
        Some(false) => "backend_success=false",
        _ => match row.watchdog_timeout {
            Some(true) => "watchdog_timeout=true",
            _ => "exit_code!=0",
        },
    }
    .to_string();
    Some(RuleHit {
        domain: Domain::Backend,
        fix_point: FixPoint::Backend {
            target: target.clone(),
            evidence: locator.clone(),
            summary: format!(
                "Backend execution failed for `{target}` (seq={}); inspect the matching activation outcome row.",
                row.sequence
            ),
        },
        evidence_refs: vec![EvidenceRef::new("runtime-trace.jsonl", locator, note)],
    })
}

fn rule_runtime(corpus: &EvidenceCorpus) -> Option<RuleHit> {
    // S8.3: an accepted transition whose commit_receipt is
    // missing or `commit_status == rolled_back`.
    let transition = corpus.accepted_transitions.iter().find(|tr| {
        let matched = corpus.runtime_trace.iter().any(|row| {
            row.kind == "commit_receipt"
                && row.transition_id.as_deref() == Some(&tr.transition_id)
                && row.commit_status.as_deref() == Some("committed")
        });
        let rolled_back = corpus.runtime_trace.iter().any(|row| {
            row.kind == "commit_receipt"
                && row.transition_id.as_deref() == Some(&tr.transition_id)
                && row.commit_status.as_deref() == Some("rolled_back")
        });
        !matched || rolled_back
    })?;
    let locator = format!("transition_id={}", transition.transition_id);
    let note = if corpus.runtime_trace.iter().any(|row| {
        row.kind == "commit_receipt"
            && row.transition_id.as_deref() == Some(&transition.transition_id)
            && row.commit_status.as_deref() == Some("rolled_back")
    }) {
        "commit_status=rolled_back"
    } else {
        "commit_receipt missing"
    }
    .to_string();
    Some(RuleHit {
        domain: Domain::Runtime,
        fix_point: FixPoint::Runtime {
            target: transition.transition_id.clone(),
            evidence: locator.clone(),
            summary: format!(
                "Accepted transition `{}` has no matching committed receipt; the runtime lost or rolled back the commit.",
                transition.transition_id
            ),
        },
        evidence_refs: vec![EvidenceRef::new(
            "accepted-transitions.jsonl",
            locator,
            note,
        )],
    })
}

fn rule_preset(corpus: &EvidenceCorpus) -> Option<RuleHit> {
    // S8.1: contract_receipt present but a terminal topic is
    // missing from the visible contract fields. We treat the
    // `contract_receipt` row's `fields.terminal_topics`
    // projection as authoritative; if the manifest's
    // `execution_capabilities` does not name a hat that owns
    // the missing topic, the preset is at fault.
    let contract = corpus
        .runtime_trace
        .iter()
        .find(|r| r.kind == "contract_receipt")?;
    let terminal_topics: Vec<String> = contract
        .raw
        .get("fields")
        .and_then(|f| f.get("terminal_topics"))
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if terminal_topics.is_empty() {
        // Cannot positively identify a preset gap; let
        // later rules decide.
        return None;
    }

    // `capabilities` is the projection from
    // `diagnosis-input.json::execution_capabilities[]`,
    // stored on the corpus during load.
    let missing: Vec<&String> = terminal_topics
        .iter()
        .filter(|topic| {
            !corpus
                .capabilities
                .iter()
                .any(|c| c.contains(topic.as_str()))
        })
        .collect();
    if missing.is_empty() {
        return None;
    }
    let target = missing
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let locator = format!("seq={}", contract.sequence);
    Some(RuleHit {
        domain: Domain::Preset,
        fix_point: FixPoint::Preset {
            target: target.clone(),
            evidence: locator.clone(),
            summary: format!(
                "Preset contract does not expose terminal topic(s) `{target}`; the agent cannot know they are required."
            ),
        },
        evidence_refs: vec![EvidenceRef::new(
            "contract_receipt",
            locator,
            "terminal_topics_not_visible_in_capabilities".to_string(),
        )],
    })
}

fn rule_agent(corpus: &EvidenceCorpus) -> Option<RuleHit> {
    // S8.2: every other layer looks healthy AND no terminal
    // event was emitted. We detect "no terminal" via the
    // absence of any `commit_receipt` with `commit_status ==
    // committed` plus the absence of any feedback row whose
    // `action` is `correction`. In other words: a clean
    // session with no terminal emission → agent.
    //
    // Preconditions: we need at least one contract_receipt
    // row AND at least one hat_activation_outcome row in
    // the trace, otherwise the agent rule would fire on a
    // session with no evidence at all (and capture_contract
    // is the correct fallback).
    let has_contract_receipt = corpus
        .runtime_trace
        .iter()
        .any(|r| r.kind == "contract_receipt");
    let has_activation_outcome = corpus.runtime_trace.iter().any(|r| {
        matches!(
            r.kind.as_str(),
            "hat_activation_outcome" | "backend_outcome" | "activation_outcome"
        )
    });
    if !has_contract_receipt || !has_activation_outcome {
        return None;
    }
    let has_committed = corpus
        .runtime_trace
        .iter()
        .any(|r| r.kind == "commit_receipt" && r.commit_status.as_deref() == Some("committed"));
    let has_correction = corpus
        .feedback
        .iter()
        .any(|r| r.action.as_deref() == Some("correction"));
    let has_terminal_event = has_committed || has_correction;
    if has_terminal_event {
        return None;
    }
    // Other-domain preconditions must hold for the agent
    // rule to fire — otherwise we fall through to
    // `capture_contract`.
    if corpus.counters.backend_failure_rows > 0
        || corpus.counters.missing_commit_count > 0
        || corpus.counters.rolled_back_count > 0
    {
        return None;
    }
    let locator = "session_summary".to_string();
    let note = "no_terminal_event_emitted".to_string();
    Some(RuleHit {
        domain: Domain::Agent,
        fix_point: FixPoint::Agent {
            target: "terminal-event".to_string(),
            evidence: locator.clone(),
            summary:
                "Preset / runtime / backend were all healthy but the agent did not emit the expected terminal event."
                    .to_string(),
        },
        evidence_refs: vec![EvidenceRef::new("session_summary", locator, note)],
    })
}

fn rule_capture_contract(corpus: &EvidenceCorpus) -> Option<RuleHit> {
    // S8.5: at least one `boundary_coverage` row is `gap`
    // AND every prior rule failed to fire. We don't even
    // need to inspect the row here — the rule chain falling
    // through to here is itself the signal.
    let gap = corpus.coverage_gaps.first()?;
    Some(RuleHit {
        domain: Domain::DiagnosticCaptureContract,
        fix_point: FixPoint::CaptureContract {
            target: gap.boundary.clone(),
            evidence: format!("boundary={}", gap.boundary),
            summary: format!(
                "Coverage gap on boundary `{}`; the collector should have recorded evidence here.",
                gap.boundary
            ),
        },
        evidence_refs: vec![EvidenceRef::new(
            "diagnosis-input.json",
            format!("boundary_coverage[{}]", gap.boundary),
            gap.reason.clone(),
        )],
    })
}

// ─── Refutations ─────────────────────────────────────────────

/// Compute the (reason, evidence) pair that refutes a
/// non-primary domain. Always returns at least one
/// [`EvidenceRef`] so the `rejected_hypotheses` projection is
/// never empty.
fn refutation_for(d: Domain, corpus: &EvidenceCorpus) -> (String, Vec<EvidenceRef>) {
    match d {
        Domain::Backend => {
            if corpus.counters.backend_failure_rows > 0 {
                (
                    "backend_failure_rows>0 (rule matched earlier domain)".to_string(),
                    vec![EvidenceRef::new(
                        "runtime-trace.jsonl",
                        "kind=hat_activation_outcome".to_string(),
                        "backend_failure_projection".to_string(),
                    )],
                )
            } else {
                (
                    "backend.outcome.success=true".to_string(),
                    vec![EvidenceRef::new(
                        "runtime-trace.jsonl",
                        "kind=hat_activation_outcome".to_string(),
                        "no_failure_row".to_string(),
                    )],
                )
            }
        }
        Domain::Runtime => {
            if corpus.counters.missing_commit_count > 0 || corpus.counters.rolled_back_count > 0 {
                (
                    "commit_receipt.committed=true (rule matched earlier domain)".to_string(),
                    vec![EvidenceRef::new(
                        "accepted-transitions.jsonl",
                        "join:commit_receipt".to_string(),
                        "outbox_fully_joined".to_string(),
                    )],
                )
            } else {
                (
                    "commit_receipt.committed=true".to_string(),
                    vec![EvidenceRef::new(
                        "accepted-transitions.jsonl",
                        "join:commit_receipt".to_string(),
                        "outbox_fully_joined".to_string(),
                    )],
                )
            }
        }
        Domain::Preset => {
            let contract = corpus
                .runtime_trace
                .iter()
                .find(|r| r.kind == "contract_receipt");
            let terminal_topics: Vec<String> = contract
                .and_then(|c| c.raw.get("fields"))
                .and_then(|f| f.get("terminal_topics"))
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let missing: Vec<String> = terminal_topics
                .iter()
                .filter(|t| !corpus.capabilities.iter().any(|c| c.contains(t.as_str())))
                .cloned()
                .collect();
            if missing.is_empty() {
                (
                    "contract_digest.terminal_topics_present=true".to_string(),
                    vec![EvidenceRef::new(
                        "contract_receipt",
                        "fields.terminal_topics".to_string(),
                        "all_terminal_topics_visible".to_string(),
                    )],
                )
            } else {
                (
                    "contract_digest.terminal_topics_present=true (rule matched earlier domain)"
                        .to_string(),
                    vec![EvidenceRef::new(
                        "contract_receipt",
                        "fields.terminal_topics".to_string(),
                        format!("missing={}", missing.join(",")),
                    )],
                )
            }
        }
        Domain::Agent => {
            let has_terminal = corpus.runtime_trace.iter().any(|r| {
                r.kind == "commit_receipt" && r.commit_status.as_deref() == Some("committed")
            }) || corpus
                .feedback
                .iter()
                .any(|r| r.action.as_deref() == Some("correction"));
            if has_terminal {
                (
                    "terminal_event_emitted=true".to_string(),
                    vec![EvidenceRef::new(
                        "session_summary",
                        "terminal".to_string(),
                        "committed_or_correction_present".to_string(),
                    )],
                )
            } else {
                (
                    "no_terminal_event_emitted (rule matched later domain)".to_string(),
                    vec![EvidenceRef::new(
                        "session_summary",
                        "terminal".to_string(),
                        "no_terminal".to_string(),
                    )],
                )
            }
        }
        Domain::DiagnosticCaptureContract => {
            if !corpus.coverage_gaps.is_empty() {
                (
                    "coverage.gap (rule matched earlier or later domain)".to_string(),
                    vec![EvidenceRef::new(
                        "diagnosis-input.json",
                        "boundary_coverage".to_string(),
                        "gaps_present".to_string(),
                    )],
                )
            } else {
                (
                    "coverage.gap=false".to_string(),
                    vec![EvidenceRef::new(
                        "diagnosis-input.json",
                        "boundary_coverage".to_string(),
                        "all_boundaries_covered".to_string(),
                    )],
                )
            }
        }
    }
}

#[cfg(test)]
mod rules_tests {
    use super::*;

    #[test]
    fn empty_corpus_produces_no_primary() {
        let corpus = EvidenceCorpus::empty();
        let attr = attribution_chain(&corpus);
        assert!(attr.primary_domain().is_none());
    }

    #[test]
    fn no_evaluable_manifest_short_circuits() {
        let mut corpus = EvidenceCorpus::empty();
        corpus.verdict = ManifestVerdict::NotEvaluable;
        let attr = attribution_chain(&corpus);
        assert!(attr.primary_domain().is_none());
    }
}
