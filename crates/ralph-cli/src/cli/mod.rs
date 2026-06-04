//! Shared CLI infrastructure layer.
//!
//! This module hosts types, helpers, and platform glue that are referenced
//! from many subcommand handlers in `commands/` and from `main.rs` itself.
//! The original code lived in `main.rs`; U4 splits it out so that
//! `main.rs` only owns the `Cli` / `Commands` dispatch and `fn main`.
//!
//! Module split (mirrors `docs/plans/2026-06-03-002-refactor-split-large-files-plan.md`
//! KTD1 + the `cli/` file structure):
//!
//! - [`shared`]: pure data types (color mode, verbosity, output format,
//!   config source, hats source).
//! - [`config_loader`]: config discovery, loading, override application,
//!   scratchpad directory bootstrap, workspace-root resolution.
//! - [`emit_path`]: the `P6` allowlist guard for `ralph emit` and the
//!   marker-target resolution helper.
//! - [`panic_hook`]: terminal-state-restore hook for the TUI panic path.
//! - [`process_management`]: Unix-only process-group leadership (no-op on
//!   non-Unix platforms).

pub mod config_loader;
pub mod emit_path;
pub mod panic_hook;
pub mod process_management;
pub mod shared;

pub(crate) use config_loader::{
    apply_config_overrides, default_config_path, ensure_scratchpad_directory,
    load_config_with_overrides, resolve_path_from_workspace, resolve_workspace_root,
    urgent_steer_path_from_workspace,
};
pub(crate) use emit_path::{resolve_emit_path, resolve_marker_target};
pub(crate) use panic_hook::install_panic_hook;
pub(crate) use shared::{ColorMode, ConfigSource, HatsSource, OutputFormat, Verbosity};
