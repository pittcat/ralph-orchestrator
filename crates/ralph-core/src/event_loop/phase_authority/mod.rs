//! 2026-07-02-006 plan: opt-in `WorkflowPhaseAuthority` engine.
//!
//! U1 — pure serde config (`config.rs`).
//! U2 — YAML → `PhaseAuthorityDeclaration` parser (`declaration.rs`).
//! U3+ — evaluator, primitives, stage, etc.
//!
//! The current module is intentionally a thin facade: only the
//! config type and its round-trip test module are public. Future
//! Units add submodules without changing the public re-exports
//! here.

pub mod config;
pub mod declaration;
// U4: per-phase per-hat topic whitelist (pure decision fn).
pub mod whitelist;
// U5: PhaseSnapshot value type (no I/O).
pub mod snapshot;

#[cfg(test)]
mod declaration_tests;
#[cfg(test)]
mod tests;