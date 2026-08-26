//! `ralph emit` subcommand: CLI arguments, handler, schema view, and tests.
//!
//! Public path `crate::commands::emit::*` is preserved verbatim from the
//! pre-split monolithic `commands/emit.rs`:
//! - `EmitArgs` (clap-derived arg struct) — declared here so the type lives at
//!   the module root and remains reachable as `commands::emit::EmitArgs`.
//! - `emit_command` (top-level entry) — re-exported from `command_impl`.
//! - `schema_view` (path-contract submodule) — `pub mod schema_view`.
//! - `normalize_wave_worker_system_fields`, `looks_like_json`,
//!   `resolve_provenance`, `PolicyCheckMode`, `should_policy_check_emit*` —
//!   re-exported from `command_impl`.
//! - `emit_command_with_root` (cfg(test) helper) — re-exported from
//!   `test_support`.
//!
//! Layout (plan 2026-08-07-003):
//! - `command_impl` — the `emit_command*` entry points and their private helpers
//! - `test_support` — `cfg(test)` helpers used by the integration tests (the
//!   `emit_command_with_root` wrapper plus workspace / fixture builders)
//! - `schema_view` — `pub mod schema_view` (path-contract preserved verbatim)
//! - 6 `tests_*` files — one per former `#[cfg(test)] mod` from the original
//!   monolithic `commands/emit.rs`

use clap::Parser;
use std::path::PathBuf;

/// Arguments for the emit subcommand.
#[derive(Parser, Debug)]
pub struct EmitArgs {
    /// Event topic (e.g., "build.done", "review.complete").
    ///
    /// Required when emitting an event; ignored when `--schema <TOPIC>`
    /// is set, because the schema mode already names its topic via the
    /// flag. We model it as `Option<String>` because clap forbids
    /// `required = true` together with `required_unless_present` on a
    /// positional argument; the handler enforces "topic must be set"
    /// for the emit path.
    pub topic: Option<String>,

    /// Event payload - string or JSON (optional, defaults to empty)
    #[arg(default_value = "")]
    pub payload: String,

    /// Parse payload as JSON object instead of string
    #[arg(long, short)]
    pub json: bool,

    /// Path to events file (defaults to .ralph/events.jsonl)
    #[arg(long, default_value = ".ralph/events.jsonl")]
    pub file: PathBuf,

    /// Validate event against current event policy before emitting
    #[arg(long)]
    pub policy_check: bool,

    /// Bypass mandatory policy check (only allowed when config permits)
    #[arg(long = "unsafe-no-policy-check", conflicts_with = "policy_check")]
    pub no_policy_check: bool,

    /// Hat that published this event (falls back to $RALPH_CURRENT_HAT)
    #[arg(long)]
    pub hat: Option<String>,

    /// Target hat triggered by this event (falls back to $RALPH_TRIGGERED_HAT)
    #[arg(long)]
    pub triggered: Option<String>,

    /// Source identifier for this event (falls back to $RALPH_EVENT_SOURCE)
    #[arg(long)]
    pub source: Option<String>,

    /// Print the embedded protocol JSON view for `TOPIC` (plan 2026-06-20-001
    /// U5 / R6). When set, no event is emitted, no events file is touched,
    /// and no iteration is consumed. Mutually exclusive with payload / json
    /// because schema mode is read-only.
    #[arg(long, value_name = "TOPIC", conflicts_with_all = ["payload", "json"])]
    pub schema: Option<String>,

    /// Output mode for policy-check / validation failures (U7).
    /// `json` prints EmitResult JSON on stdout (machine-parseable);
    /// `text` keeps the legacy human-readable stderr format.
    #[arg(long, value_name = "MODE", default_value = "text")]
    pub output: String,

    /// Evaluation token (U5, plan 2026-07-30-004) proving this payload
    /// passed `ralph emit <topic> --policy-check` against the SAME
    /// Effective Execution Contract revision. Required on the apply path
    /// in an agent context (`RALPH_CURRENT_HAT` set) when the preset has
    /// no event-policy pipeline to validate the emit; ignored otherwise.
    /// Obtain it from the `policy_check_token` field printed by a prior
    /// `--policy-check` run.
    #[arg(long = "policy-check-token", value_name = "TOKEN")]
    pub policy_check_token: Option<String>,
}

pub mod command_impl;
pub mod schema_view;

#[cfg(test)]
mod tests_apply_recorded;
#[cfg(test)]
mod tests_integration;
#[cfg(test)]
mod tests_policy_check_accept;
#[cfg(test)]
mod tests_policy_check_reject;
#[cfg(test)]
mod tests_reject_summary;
#[cfg(test)]
mod tests_schema_emit_result;

// `test_support` is `#[cfg(test)]` only; pulling it in unconditionally would
// leak test-only public items into release builds.
#[cfg(test)]
mod test_support;

// Re-export public items so the historical paths
// `crate::commands::emit::emit_command`, `schema_view`, `EmitArgs`, etc.
// keep working. `normalize_wave_worker_system_fields` is `pub(crate)` in
// `command_impl` (it is only called from `commands/u2_wave_system_field_tests.rs`
// inside this crate) — its `crate::commands::emit::*` path is preserved by
// the sibling `crate::commands::emit::normalize_wave_worker_system_fields` lookup,
// which resolves to the `pub(crate)` definition directly. The other
// re-exports (`looks_like_json`, `resolve_provenance`, `PolicyCheckMode`,
// `should_policy_check_emit*`) are preserved for the historical path
// contract even when no current code path uses them — `allow(unused_imports)`
// silences the lint without changing the public surface.
#[allow(unused_imports)]
pub use command_impl::{
    PolicyCheckMode, emit_command, looks_like_json, resolve_provenance, should_policy_check_emit,
    should_policy_check_emit_with_ctx,
};
#[allow(unused_imports)]
pub(crate) use command_impl::{normalize_wave_worker_system_fields, validate_wave_worker_context};
#[cfg(test)]
pub use test_support::emit_command_with_root;
