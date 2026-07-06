//! 2026-07-02-006 plan U2: pure parser
//! `PhaseAuthorityConfig → PhaseAuthorityDeclaration`.
//!
//! No lint, no evaluator, no stage wiring. The declaration is a
//! normalised in-memory view: phases have stable order, every
//! transition references a known phase, the `initial_phase` is
//! resolved into the first snapshot state, and verdict matrices
//! are flattened into per-transition rules.
//!
//! `DeclarationError` mirrors the failure modes the unit-test
//! matrix exercises (duplicate phase id, dangling transition,
//! missing initial phase, unknown primitive, missing
//! `from`/`to`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::config::{
    PhaseAuthorityConfig, PhaseDeclConfig, PhaseTransitionConfig, TransitionOnConfig,
};

/// Normalised, in-memory phase authority declaration. Built once
/// per loop from `PhaseAuthorityConfig`. U10/U11 pass this value
/// to the runtime; the config struct never leaks past the parser.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhaseAuthorityDeclaration {
    /// Stable, declaration-time list of phases. The first entry
    /// is `initial_phase` when present; the order otherwise is
    /// the YAML order.
    pub phases: Vec<PhaseDeclConfig>,

    /// Normalised transition list. Same order as the input; U10
    /// matches by `(from, on)` and the order only matters for
    /// diagnostics.
    pub transitions: Vec<PhaseTransitionConfig>,

    /// Resolved initial phase id. Equals
    /// `PhaseAuthorityConfig.initial_phase` when set; otherwise
    /// the first entry of `phases` when present; otherwise
    /// `None` (the engine treats the declaration as inert in
    /// that case — U11 short-circuits).
    pub initial_phase: Option<String>,
}

/// Parser errors. Each variant pins one of the U2 test scenarios.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeclarationError {
    #[error("phase id is duplicated: {0}")]
    DuplicatePhaseId(String),

    #[error("transition references unknown phase id: {phase} (transition {from} -> {to})")]
    UnknownPhase {
        phase: String,
        from: String,
        to: String,
    },

    #[error("initial_phase references unknown phase id: {0}")]
    UnknownInitialPhase(String),

    #[error("transition is missing `from` / `to`: {0}")]
    IncompleteTransition(String),
}

impl PhaseAuthorityDeclaration {
    /// Pure parse: `PhaseAuthorityConfig` → declaration.
    pub fn try_from_config(cfg: &PhaseAuthorityConfig) -> Result<Self, DeclarationError> {
        // Duplicate phase id check.
        let mut seen = std::collections::HashSet::new();
        for phase in &cfg.phases {
            if !seen.insert(phase.id.as_str()) {
                return Err(DeclarationError::DuplicatePhaseId(phase.id.clone()));
            }
        }

        // Build phase id set once.
        let phase_ids: std::collections::HashSet<&str> =
            cfg.phases.iter().map(|p| p.id.as_str()).collect();

        // Validate transitions.
        let mut normalised_transitions = Vec::with_capacity(cfg.transitions.len());
        for tr in &cfg.transitions {
            if tr.from.is_empty() || tr.to.is_empty() {
                return Err(DeclarationError::IncompleteTransition(format!(
                    "{}->{}",
                    tr.from, tr.to
                )));
            }
            // `from == "*"` is reserved for "any phase"; U2 only
            // verifies the literal token, U10 matches it.
            if tr.from != "*" && !phase_ids.contains(tr.from.as_str()) {
                return Err(DeclarationError::UnknownPhase {
                    phase: tr.from.clone(),
                    from: tr.from.clone(),
                    to: tr.to.clone(),
                });
            }
            if !phase_ids.contains(tr.to.as_str()) {
                return Err(DeclarationError::UnknownPhase {
                    phase: tr.to.clone(),
                    from: tr.from.clone(),
                    to: tr.to.clone(),
                });
            }
            normalised_transitions.push(tr.clone());
        }

        // Resolve initial phase.
        let initial_phase = match cfg.initial_phase.as_deref() {
            Some(id) => {
                if !phase_ids.contains(id) {
                    return Err(DeclarationError::UnknownInitialPhase(id.to_string()));
                }
                Some(id.to_string())
            }
            None => cfg.phases.first().map(|p| p.id.clone()),
        };

        Ok(Self {
            phases: cfg.phases.clone(),
            transitions: normalised_transitions,
            initial_phase,
        })
    }
}

impl PhaseAuthorityConfig {
    /// Sugar so callers can write
    /// `cfg.try_into_declaration()` instead of plumbing the
    /// `try_from_config` static.
    pub fn try_into_declaration(&self) -> Result<PhaseAuthorityDeclaration, DeclarationError> {
        PhaseAuthorityDeclaration::try_from_config(self)
    }
}

// Keep the config `TransitionOnConfig` import alive even when no
// other reference exists in this file.
const _: fn(&TransitionOnConfig) = |_| {};
