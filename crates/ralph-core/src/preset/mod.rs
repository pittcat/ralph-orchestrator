//! `preset` — preset-agnostic execution engine.
//!
//! Plan ref: plan 2026-06-20-001 U1/U2 (protocol SSOT engine).
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

pub mod engine;