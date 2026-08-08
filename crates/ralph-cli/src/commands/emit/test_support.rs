//! Test-only helpers for the `commands::emit` test modules.
//!
//! The `emit_command_with_root` wrapper was previously a `#[cfg(test)] pub fn`
//! free function inside the monolithic `commands/emit.rs` (lines 845–852 of
//! HEAD `7909f159`). The integration tests under `tests_integration` and the
//! JSON-shape tests (`tests_policy_check_*`, `tests_apply_recorded`, …) all
//! call it, so it is hoisted into a shared `cfg(test)` submodule and
//! re-exported via `super::emit_command_with_root` for the original call sites
//! to keep compiling unchanged.

use anyhow::Result;
use std::path::PathBuf;

use crate::cli::ColorMode;
use crate::commands::emit::EmitArgs;

/// Test-friendly wrapper that pins `hats_source=None`,
/// `config_sources=[]`, and `config_was_explicit=false`, leaving only the
/// `color_mode` / `args` / `root` knobs the tests actually exercise.
///
/// Re-exported via `super::emit_command_with_root` so the test modules
/// continue to call it by the historical name.
#[cfg(test)]
pub fn emit_command_with_root(
    color_mode: ColorMode,
    args: EmitArgs,
    root: Option<&PathBuf>,
) -> Result<()> {
    super::command_impl::emit_command_with_root_and_hats(color_mode, args, root, None, &[], false)
}
