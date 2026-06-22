//! `preset::engine` — preset-agnostic execution engine.
//!
//! Plan ref: U1, U2 of
//! `docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`.
//!
//! This module hosts the *generic* execution machinery that consumes
//! the protocol SSOT (either the YAML file under `presets/schemas/`
//! or the post-`build.rs`-merge embedded preset) and runs gates,
//! projects state, and emits lint hints.
//!
//! Hard rules (KTD-10):
//!   * No duplicate payload field tables in Rust. All field
//!     requirements come from the embedded `event_policy.schemas`
//!     + `execution_contracts.rules`.
//!   * `ralph-core` does **not** parse raw embedded YAML. The
//!     engine accepts `&EventLoopConfig` (or a `ProtocolView`
//!     derived from it) and reads protocol values from there.
//!   * `ralph-core` does **not** depend on `ralph-cli`. New presets
//!     are added by writing `presets/schemas/<name>.yml` only.

pub mod gates;
pub mod hint;
pub mod lint_mirror;
pub mod linter;
pub mod projection;
pub mod protocol;

pub use gates::{GateContext, GateDecision, LintContext, run_gates};
pub use hint::{LintFailureClass, LintResumeHint, LintResumeTarget, classify_lint_failure};
pub use lint_mirror::{build_lint_mirror_block, build_lint_resume_block};
pub use linter::{LintOutcome, LintPaths, auto_handoff_prepare, lint_emit, lint_emit_with_timeout};
pub use projection::apply_projection;
pub use protocol::ProtocolView;
