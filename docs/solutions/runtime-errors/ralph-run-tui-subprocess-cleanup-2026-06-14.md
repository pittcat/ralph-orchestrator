---
module: ralph-cli
tags: [tui, cleanup, loop-lock, signals]
problem_type: runtime-error
---

# `ralph run` TUI subprocess cleanup leaves stale `loop.lock`

## Symptom

After killing a `ralph run` TUI session with `SIGINT`/`SIGTERM` (or `kill`),
`.ralph/loop.lock` remains on disk with the PID of the now-dead RPC child.
Subsequent `ls` shows the file, and operators may think the loop is still
running or that cleanup failed.

## Root cause

In subprocess TUI mode the parent process intentionally does **not** acquire
`.ralph/loop.lock`; the child RPC process (`ralph run --rpc`) acquires it.
When the parent receives a termination signal it restores the terminal and
kills the child, but the child's `LockGuard` is never dropped, so the lock
file is left behind with stale metadata.

The lock is detected as stale on the next `ralph run` and cleaned up, but
visible cleanup is incomplete.

## Fix

`crates/ralph-cli/src/commands/run.rs::run_subprocess_tui` now calls
`cleanup_subprocess_loop_lock(&args.workspace, child_id)` after reaping the
child. The helper uses `LoopLock::inspect` to remove the lock file only when
no other process holds the flock, avoiding races with concurrently-started
loops.

## Verification

- Reproduce: run `ralph -H builtin:ce-executor-isolated run`, send `SIGINT`
  to the parent, confirm `.ralph/loop.lock` is removed.
- Tests: `cargo nextest run -p ralph-cli --bin ralph cleanup_subprocess_loop_lock`

## Affected code

- `crates/ralph-cli/src/commands/run.rs`
  - `cleanup_subprocess_loop_lock`
  - `run_subprocess_tui`
