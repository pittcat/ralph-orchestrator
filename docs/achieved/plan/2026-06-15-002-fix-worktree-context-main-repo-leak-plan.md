---
title: Fix worktree context.md main-repo path leak
type: fix
status: active
date: 2026-06-15
origin: docs/report/2026-06-15-worktree-isolation-leak-diagnosis.md
---

# Fix worktree context.md main-repo path leak

## Overview

When `ralph run --worktree` creates a git worktree for a parallel loop, it writes a `.ralph/agent/context.md` file inside the worktree to orient the agent. The current template prints both the workspace (worktree) path and the main repository path. The diagnosis report `docs/report/2026-06-15-worktree-isolation-leak-diagnosis.md` confirms this caused an agent to modify the same source file in both the worktree and the main repository, leaking changes across the isolation boundary.

This plan removes the main-repo path from the agent-visible context and replaces it with an explicit workspace-only isolation rule. It also adds regression tests so the leak cannot be reintroduced.

---

## Problem Frame

- Worktree loops are supposed to be filesystem-isolated from the main repository except for shared metadata symlinks (`memories.md`, `specs/`, `tasks/`).
- The agent subprocess already runs with `cwd` and `RALPH_WORKSPACE_ROOT` pointing at the worktree (`crates/ralph-adapters/src/pty_executor.rs::inject_ralph_runtime_env`).
- Despite that, `context.md` voluntarily exposes the main repository absolute path, giving the agent two plausible write targets.
- A prior infrastructure fix (`docs/achieved/plan/2026-06-14-002-fix-worktree-agent-writes-to-main-repo-plan.md`) aligned `PWD`, `PROMPT.md` forwarding, and executor cwd, but left `context.md` unchanged.
- The remaining fix is therefore in the information layer: do not tell the agent about the main repo path, and explicitly constrain file operations to the workspace.

---

## Requirements Trace

- R1. `context.md` generated for worktree loops must not contain the main repository absolute path.
- R2. `context.md` must instruct the agent that all file operations are only allowed within the workspace path.
- R3. Existing unit tests for `generate_context_file` must be updated to enforce R1 and R2.
- R4. Integration tests for worktree isolation must assert that a real `ralph run --worktree` produces a `context.md` satisfying R1 and R2.

---

## Scope Boundaries

- In scope: `crates/ralph-core/src/loop_context.rs`, its unit tests, and `crates/ralph-cli/tests/integration_worktree_isolation.rs`.
- Out of scope: filesystem-level sandboxing or containers; those are too heavy and are documented as future architecture.
- Out of scope: converting shared metadata symlinks from absolute to relative paths. The symlinks (`memories.md`, `specs/`, `tasks/`) intentionally point at the main repo and an agent running `readlink` can still discover the path. That is a separate, larger change and is not the leak surface identified in the diagnosis report.
- Out of scope: runtime detection of main-repo modifications. This plan addresses the root cause (information exposure); a runtime guard can be added later if needed.

### Deferred to Follow-Up Work

- Relative-path metadata symlinks: `docs/solutions/` note or separate plan if we want to remove the residual `readlink` leak vector.
- Optional runtime guard: scan main-repo `git status` after agent iterations and emit `worktree.isolation.boundary_violation` if new modifications appear.

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/loop_context.rs::generate_context_file` (line ~567) creates `context.md` for worktree loops and currently prints `self.repo_root.display()` as **Main Repo**.
- `crates/ralph-cli/src/commands/run.rs:454` and `:846` call `generate_context_file` for create and `--reuse-worktree` paths respectively.
- `crates/ralph-adapters/src/pty_executor.rs::inject_ralph_runtime_env` already sets `RALPH_WORKSPACE_ROOT` and `PWD` to the worktree path, so the environment reinforces the workspace boundary.
- Existing worktree isolation tests live in `crates/ralph-cli/tests/integration_worktree_isolation.rs` and do not currently inspect `context.md` content.
- `docs/achieved/plan/2026-06-14-002-fix-worktree-agent-writes-to-main-repo-plan.md` fixed executor cwd / `PWD` / `PROMPT.md` forwarding but explicitly did not change `context.md`.

### Institutional Learnings

- `docs/solutions/developer-experience/agent-execution-contract-gates-2026-06-03.md` prefers gates/backpressure over instructions, but when instructions are used they should be consistent across layers. Removing the leaked path is itself a gate (information hiding).
- `docs/advanced/parallel-loops.md` and `AGENTS.md` document worktree isolation as an end-to-end promise: events, diagnostics, and runtime state stay in the worktree.

### External References

- None. The fix is entirely local to this codebase.

---

## Key Technical Decisions

- **Remove the `Main Repo` field rather than just warn.** The diagnosis report shows that exposing both paths is the root cause. A warning alone leaves the agent with two absolute paths to choose from. Removing the field is the minimal mechanism-level fix.
- **Keep the `Workspace` field and add a `## CRITICAL` block.** The agent still needs to know where it is running; the block explicitly ties all file operations to that path and references `RALPH_WORKSPACE_ROOT`.
- **Do not change symlink targets to relative paths in this plan.** That is a larger, separate change with its own portability and cleanup risks. The diagnosis specifically blames `context.md`, not the symlinks.
- **Regenerate `context.md` on `--reuse-worktree`.** The current code already calls `generate_context_file` during reuse cleanup; this plan preserves that behavior so the updated content applies to reused worktrees too.

---

## Implementation Units

- [ ] U1. **Remove main-repo path from worktree `context.md`**

**Goal:** Eliminate the main repository absolute path from the agent-visible `context.md` and replace it with a workspace-only isolation rule.

**Requirements:** R1, R2

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/loop_context.rs`

**Approach:**
- Edit `generate_context_file` to remove the `- **Main Repo**: {}` line and its `self.repo_root.display()` argument.
- Add a `## CRITICAL` section after the metadata list that states:
  - All file operations MUST use the `Workspace` path above.
  - Do NOT write files to any other path, especially not the main repository.
  - Prefer relative paths or the `RALPH_WORKSPACE_ROOT` environment variable.
- Keep the existing `## Notes` section about shared symlinks, since those are still true and relevant.

**Patterns to follow:**
- The existing `format!` template in `crates/ralph-core/src/loop_context.rs:588-615`.
- `truncate_with_ellipsis` usage for the prompt preview.

**Test scenarios:**
- Happy path: `generate_context_file("ralph/loop-1234", "Add footer")` for a worktree context produces a file containing `# Worktree Context`, the loop ID, the workspace path, and the branch.
- Edge case: `generate_context_file` for a primary loop returns `Ok(false)` and writes nothing.

**Verification:**
- A worktree `context.md` no longer contains the main repo path.
- A worktree `context.md` contains the `CRITICAL` workspace-only rule.

---

- [ ] U2. **Update unit tests for `generate_context_file`**

**Goal:** Make the existing unit tests enforce the new isolation contract.

**Requirements:** R1, R2, R3

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/src/loop_context.rs` (test module at the bottom)

**Approach:**
- Extend `test_generate_context_file_worktree` to:
  - Assert the content does NOT contain the main repo path (`repo_root`).
  - Assert the content contains the `CRITICAL` block / workspace-only rule.
  - Assert the content mentions `RALPH_WORKSPACE_ROOT`.
- Leave `test_generate_context_file_primary_noop` unchanged; primary loops do not create this file.

**Patterns to follow:**
- Existing assertions in `test_generate_context_file_worktree`.

**Test scenarios:**
- Happy path: generated file contains expected worktree metadata and isolation rule.
- Error path / regression: generated file must not contain the main repo path string.

**Verification:**
- `cargo nextest run -p ralph-core test_generate_context_file` passes.

---

- [ ] U3. **Add integration test for `context.md` isolation guarantee**

**Goal:** Verify that a real `ralph run --worktree` invocation produces a `context.md` that does not leak the main repo path.

**Requirements:** R1, R2, R4

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-cli/tests/integration_worktree_isolation.rs`

**Approach:**
- Add a new test function (e.g. `test_worktree_context_md_does_not_expose_main_repo`).
- Use the existing helper `setup_git_repo` and `write_minimal_config`.
- Run `env!("CARGO_BIN_EXE_ralph") run --worktree --no-tui --skip-preflight --prompt "context isolation test"`.
- Discover the created worktree directory (as existing tests do via `fs::read_dir(main_repo.join(".worktrees"))`).
- Read `.worktrees/<id>/.ralph/agent/context.md`.
- Assert it does not contain the main repo absolute path.
- Assert it contains the workspace-only / `RALPH_WORKSPACE_ROOT` isolation language.
- Do not assert on exact prompt/branch formatting; focus on the isolation contract.

**Patterns to follow:**
- Existing integration tests in `crates/ralph-cli/tests/integration_worktree_isolation.rs` (e.g. `test_worktree_creates_exactly_one_and_registry_correct`).

**Test scenarios:**
- Integration / happy path: a fresh `--worktree` run creates a `context.md` that hides the main repo and states the workspace-only rule.
- Edge case: the test should tolerate the worktree directory name being dynamically generated (derive it from `.worktrees/`).

**Verification:**
- `cargo nextest run -p ralph-cli --test integration_worktree_isolation test_worktree_context_md_does_not_expose_main_repo` passes.

---

## System-Wide Impact

- **Interaction graph:** `context.md` is read by the agent backend (Claude/codex/etc.), not by Ralph Rust code. The change only affects agent orientation.
- **Error propagation:** No error paths change. If `generate_context_file` fails, the existing `?` propagation in `run.rs` remains unchanged.
- **State lifecycle risks:** Reused worktrees already regenerate `context.md`; this is preserved, so stale content is not a concern. The file is created only if absent, but `--reuse-worktree` cleanup does not delete it, so the updated text only appears on new or reused worktrees after this change.
- **API surface parity:** No public API changes.
- **Integration coverage:** The new integration test exercises the real CLI path end-to-end.
- **Unchanged invariants:** Worktree creation, symlink setup, event routing, executor cwd, and `RALPH_WORKSPACE_ROOT` injection remain unchanged.

---

## Risks & Dependencies

| Risk | Mitigation |
|------|-----------|
| Agent still discovers main repo via absolute symlink targets (`readlink .ralph/specs`) and writes there anyway | Accepted residual risk. The diagnosis report identifies `context.md` as the leak surface; symlink relativization is deferred as follow-up work. |
| Removing `Main Repo` breaks an agent workflow that legitimately needed the path | None expected. The main repo path is not required for worktree file operations; shared metadata is accessed through the symlinks. |
| Test flakiness from dynamically named worktree directories | Derive the directory from `.worktrees/` at test runtime, matching existing integration tests. |

---

## Documentation / Operational Notes

- Update `docs/report/2026-06-15-worktree-isolation-leak-diagnosis.md` after implementation to mark the fix and reference this plan.
- No user-facing docs changes required; the change is invisible to users except through reduced isolation leaks.

---

## Sources & References

- **Origin document:** `docs/report/2026-06-15-worktree-isolation-leak-diagnosis.md`
- **Related prior plan:** `docs/achieved/plan/2026-06-14-002-fix-worktree-agent-writes-to-main-repo-plan.md`
- **Related code:** `crates/ralph-core/src/loop_context.rs`, `crates/ralph-cli/src/commands/run.rs`, `crates/ralph-adapters/src/pty_executor.rs`
- **Related tests:** `crates/ralph-cli/tests/integration_worktree_isolation.rs`
