//! CLI commands for the `ralph task` namespace.
//!
//! Provides subcommands for managing tasks:
//! - `add`: Create a new task
//! - `ensure`: Create or reuse a keyed task
//! - `list`: List all tasks
//! - `ready`: Show unblocked tasks
//! - `start`: Mark a task as in progress
//! - `close`: Mark a task as complete
//! - `reopen`: Reopen a closed/failed task
//! - `show`: Show a single task by ID
//! - `verify`: OPAC Precheck; verifies a mutation would succeed without writing
//! - `verify-emit-bridge`: verifies the three-field task_id/task_key/step consistency
//!
//! `verify` exists to satisfy the OPAC Precheck stage (R7/R16). It runs the
//! same authorization gates as the real mutation (`HatCommandPolicy` +
//! `authorize_lifecycle` + field validation) but never touches
//! `tasks.jsonl`. U14 fix-units / shippers rely on `verify` to confirm an
//! emit is correctly wired before applying.

mod args;
mod cmd_add_ensure;
mod cmd_fail_verify;
mod cmd_list_close;
mod validation;

#[cfg(test)]
mod ensure_for_fix_unit_clap_tests;
#[cfg(test)]
mod load_coordinator_hats_tests;
#[cfg(test)]
mod task_verify_gate_wiring_tests;
#[cfg(test)]
mod tests;

// Re-export the public API surface that `tools.rs`, `hat_command_policy.rs`,
// and the docs (e.g. `task_cli::load_coordinator_hats`,
// `task_cli::validate_owner_hat_id`, `task_cli::authorize_lifecycle`,
// `task_cli::emit_close_completion_warning`, `task_cli::read_current_loop_id`)
// rely on. Item-level moves only — no behavior changes.
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use args::{
    AddArgs, CloseArgs, ConfirmArgs, EnsureArgs, FailArgs, ListArgs, OutputFormat, ReadyArgs,
    ReopenArgs, ShowArgs, StartArgs, VerifyAddArgs, VerifyArgs, VerifyCommands,
    VerifyEmitBridgeArgs, VerifyEnsureArgs, VerifyFormatArgs, VerifyOutcome,
};
pub use args::{CoordinatorHatsError, TaskArgs, TaskCommands, load_coordinator_hats};
// `authorize_lifecycle`, `validate_owner_hat_id`, and `read_current_loop_id`
// remain `pub(crate)` (item-level visibility unchanged). Doc comments in
// `hat_command_policy.rs` and `operation_guard.rs` still reference
// `task_cli::*` paths for narrative context; they are not external APIs.
// Internal helpers re-exported at crate-internal scope so the four test
// submodules (`tests`, `load_coordinator_hats_tests`,
// `ensure_for_fix_unit_clap_tests`, `task_verify_gate_wiring_tests`)
// can keep `use super::*;` and reach the same items they called before
// the file split.
#[cfg(test)]
pub(crate) use cmd_add_ensure::{add_task_with_args, ensure_task_with_args};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use cmd_add_ensure::{
    add_task_with_confirmation, ensure_task_with_confirmation, print_added_task, print_ensured_task,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use cmd_fail_verify::{
    emit_bridge_deny, execute_verify_emit_bridge, fail_task_with_context, gate_outcome,
    print_confirmed_task, reopen_task_with_context, verify_add, verify_ensure, verify_lifecycle,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use cmd_list_close::{
    build_close_warning_payload, build_close_warning_payload_missing_marker,
    close_task_with_context, close_task_with_context_and_config, emit_close_completion_warning,
    parse_topics_from_jsonl_tail, start_task_with_context,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use validation::read_current_loop_id;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use validation::{
    add_common_task_fields, authorize_lifecycle, canonical_add_payload, canonical_ensure_payload,
    enforce_command_policy, filter_tasks_for_list, filter_tasks_for_ready, gate_identifiers,
    get_tasks_path, load_config_or_default, operation_context_for, settle_gate_claim,
    validate_owner_hat_id, validate_task_id, verify_gate_claim,
};

use crate::{ConfigSource, resolve_workspace_root};
use anyhow::Result;
// These types are referenced through the `pub use` re-exports above and
// by the four test submodules via `use super::*;`. Production `execute`
// only uses `ConfigSource` directly; the others are imported so the test
// submodules (which sit at `task_cli::tests` and friends) inherit them
// through the root module's namespace.
#[allow(unused_imports)]
use crate::operation_guard::OperationContext;
#[allow(unused_imports)]
use ralph_core::{Task, TaskStatus, TaskStore};
#[allow(unused_imports)]
use std::path::{Path, PathBuf};

/// Executes task CLI commands.
pub fn execute(args: TaskArgs, use_colors: bool, config_sources: &[ConfigSource]) -> Result<()> {
    let root = args.root.clone();
    let workspace = resolve_workspace_root(root.as_ref());
    // U7 (2026-07-04-003 plan): load `coordinator_hats` through the
    // typed loader so we can surface the *shape* of the failure
    // (missing ralph.yml vs missing tasks: vs missing key vs empty)
    // instead of silently treating all four as "empty allowlist".
    // Human CLI gets `unwrap_or_default()` so a missing/empty config
    // does not lock the operator out of `task add`; agents always
    // see the typed Err converted into a hint.
    //
    // 2026-07-13-001 plan U3: `config_sources` carries the explicit
    // `-c` path so a custom project config file is honored without
    // the workspace needing a `ralph.yml` symlink.
    let (coordinator_hats, coordinator_err) =
        match load_coordinator_hats(&workspace, config_sources) {
            Ok(hats) => (hats, None),
            Err(err) => (Vec::new(), Some(err)),
        };
    let config = validation::load_config_or_default(root.as_ref(), config_sources);

    match args.command {
        TaskCommands::Add(add_args) => cmd_add_ensure::execute_add(
            add_args,
            root.as_ref(),
            &coordinator_hats,
            coordinator_err.as_ref(),
            use_colors,
            config_sources,
        ),
        TaskCommands::Ensure(ensure_args) => cmd_add_ensure::execute_ensure(
            ensure_args,
            root.as_ref(),
            &coordinator_hats,
            coordinator_err.as_ref(),
            use_colors,
            config_sources,
        ),
        TaskCommands::List(list_args) => {
            cmd_list_close::execute_list(list_args, root.as_ref(), use_colors)
        }
        TaskCommands::Ready(ready_args) => {
            cmd_list_close::execute_ready(ready_args, root.as_ref(), use_colors)
        }
        TaskCommands::Start(start_args) => cmd_list_close::execute_start(
            start_args,
            root.as_ref(),
            &coordinator_hats,
            use_colors,
            config_sources,
        ),
        TaskCommands::Close(close_args) => cmd_list_close::execute_close(
            close_args,
            root.as_ref(),
            &coordinator_hats,
            &config,
            use_colors,
            config_sources,
        ),
        TaskCommands::Fail(fail_args) => cmd_fail_verify::execute_fail(
            fail_args,
            root.as_ref(),
            &coordinator_hats,
            use_colors,
            config_sources,
        ),
        TaskCommands::Reopen(reopen_args) => cmd_fail_verify::execute_reopen(
            reopen_args,
            root.as_ref(),
            &coordinator_hats,
            use_colors,
            config_sources,
        ),
        TaskCommands::Show(show_args) => {
            cmd_fail_verify::execute_show(show_args, root.as_ref(), use_colors)
        }
        TaskCommands::Confirm(confirm_args) => {
            cmd_fail_verify::execute_confirm(confirm_args, root.as_ref())
        }
        TaskCommands::Verify(verify_args) => {
            cmd_fail_verify::execute_verify(verify_args, use_colors, config_sources)
        }
        TaskCommands::VerifyEmitBridge(bridge_args) => {
            cmd_fail_verify::execute_verify_emit_bridge(bridge_args, root.as_ref())
        }
    }
}
