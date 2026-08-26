//! The five causal domains the R8 rule chain can attribute a
//! failure to.
//!
//! Each variant corresponds to one Scenario in the U08 BDD
//! spec (S8.1-S8.5). The on-the-wire spelling is
//! `snake_case` and the order in [`ALL`] defines the canonical
//! serialization order for any consumer that iterates
//! `rejected_hypotheses` or renders a domain breakdown (the
//! U09 markdown renderer will rely on this ordering for
//! deterministic output, per R9).
//!
//! The set is closed: the parallel-dev preset requires
//! exactly one primary domain per attribution (R7), and the
//! parallel-dev `HARD RULE` for `boundary_coverage` (U07 §6)
//! forbids inventing a sixth boundary without bumping both
//! the manifest schema and this enum. Any future expansion
//! must update [`ALL`] and [`Domain::as_str`] in lockstep.

use serde::{Deserialize, Serialize};

/// One of five mutually exclusive causal domains.
///
/// The variants are ordered to match the R8 most-upstream
/// preventable chain (see `rules::attribution_chain`). The
/// ordering is **not** a severity ranking — it is a priority
/// queue. The first domain whose evidence fingerprint matches
/// wins, regardless of where it sits in the human "blame"
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    /// S8.4 — backend failure (the agent never executed or
    /// its execution was non-zero / timed out / killed).
    Backend,
    /// S8.3 — runtime commit-break (a transition was accepted
    /// but its commit receipt is missing or rolled back).
    Runtime,
    /// S8.1 — preset contract gap (the contract_digest /
    /// terminal_topics_digest / hats_digest do not expose
    /// information the runtime needed).
    Preset,
    /// S8.2 — agent failure (preset / runtime / backend were
    /// all healthy but the agent never produced the terminal
    /// event the run was waiting for).
    Agent,
    /// S8.5 — capture contract gap (the collector should
    /// have recorded evidence but the boundary is `gap`; no
    /// other domain has a fingerprint).
    DiagnosticCaptureContract,
}

impl Domain {
    /// All domains, in canonical order. Iterating this slice
    /// is the only way to enumerate domains in deterministic
    /// order — `Domain::iter()` via `strum` is intentionally
    /// not used because we want the explicit list to fail
    /// compile when a sixth domain is added.
    pub const ALL: [Domain; 5] = [
        Domain::Backend,
        Domain::Runtime,
        Domain::Preset,
        Domain::Agent,
        Domain::DiagnosticCaptureContract,
    ];

    /// Stable on-the-wire string. Mirrors
    /// `serde(rename_all = "snake_case")`; pinned here so
    /// consumer tests do not depend on macro expansion.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Domain::Backend => "backend",
            Domain::Runtime => "runtime",
            Domain::Preset => "preset",
            Domain::Agent => "agent",
            Domain::DiagnosticCaptureContract => "diagnostic_capture_contract",
        }
    }

    /// Human-readable label for the operator-facing reporter
    /// (U09). Kept here so the engine and renderer cannot drift.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Domain::Backend => "Backend",
            Domain::Runtime => "Runtime",
            Domain::Preset => "Preset",
            Domain::Agent => "Agent",
            Domain::DiagnosticCaptureContract => "Diagnostic capture contract",
        }
    }
}

#[cfg(test)]
mod domain_tests {
    use super::Domain;

    #[test]
    fn all_covers_five_domains_in_canonical_order() {
        let names: Vec<&'static str> = Domain::ALL.iter().map(|d| d.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "backend",
                "runtime",
                "preset",
                "agent",
                "diagnostic_capture_contract",
            ]
        );
    }

    #[test]
    fn serde_snake_case_roundtrip() {
        for d in Domain::ALL {
            let s = serde_json::to_string(&d).expect("serialize");
            assert_eq!(s, format!("\"{}\"", d.as_str()));
            let back: Domain = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(back, d);
        }
    }

    #[test]
    fn ord_is_canonical_chain_order() {
        // The Ord derive uses declaration order; pin it here
        // so any accidental re-ordering of the variants
        // (which would silently change the R8 chain
        // priority) breaks the test loudly.
        let mut sorted = Domain::ALL.to_vec();
        sorted.sort();
        assert_eq!(sorted, Domain::ALL.to_vec());
    }
}
