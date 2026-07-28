# Changelog

All notable changes to ralph-orchestrator are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Removed

- Removed backends: amp, roo, kiro, kiro-acp, copilot. Remaining backends: claude, gemini, codex, opencode, pi, traecli, custom. The full backend
  surface is now 7 named backends + 1 custom adapter. Removed in plan
  `2026-07-14-001-refactor-remove-5-backends-plan`:
  - `copilot`: dropped `copilot_stream.rs` module, `OutputFormat::CopilotStreamJson`
    enum variant, `CliBackend::copilot()` / `copilot_tui()` / `copilot_interactive()`
    factories, all integration scripts in `pty_executor_integration.rs` and `cli_executor.rs`.
  - `amp`: dropped `CliBackend::amp()` / `amp_interactive()` factories,
    `v1_adapters::AdapterSettings::amp` field, `rph_config::get_agent_priority`
    amp entry, all `wave.rs` 5-matrix amp cases.
  - `roo`: dropped `CliBackend::roo()` / `roo_interactive()` factories,
    `build_roo_prompt_file` helper (and its special case in `build_command`),
    `v1_adapters::AdapterSettings::roo`, `get_agent_priority` roo entry,
    all `wave.rs` 5-matrix roo cases.
  - `kiro` + `kiro-acp` (merged unit, cannot split): dropped
    `acp_executor.rs` module + `OutputFormat::Acp` enum variant,
    `CliBackend::kiro()` / `kiro_with_agent()` / `kiro_acp()` /
    `kiro_acp_with_options()` / `kiro_interactive()` factories,
    `HatBackend::KiroAgent` variant (whole `HatBackend` enum lost the
    `agent` + `args` from ACP-style config), `agent-client-protocol`
    crate dependency, `loop_runner` ACP executor path (`execute_acp`,
    `run_wave_worker_acp`, `MOCK_ACP_EXECUTIONS` mocks,
    `acp_executor_integration.rs` + `acp_process_cleanup.rs` tests),
    `sop_runner` ACP TUI fallback, all `wave.rs` kiro/kiro-acp cases,
    `v1_adapters::AdapterSettings::kiro` field, `preflight::backend_command`
    kiro arm, `detect_command` kiro → kiro-cli map.
  - `presets/minimal/{amp,kiro,roo}.yml` removed (no orphan templates);
    `presets/minimal/preset-evaluator.yml` CLI backend switched
    `kiro` → `claude` (U4 deleted kiro, left the evaluator pointing at
    a backend that no longer exists).
  - `scripts/ralph-zsh-plugin.zsh` `_RALPH_BACKENDS=( ... )` array
    trimmed from 12 entries to 8 (removed kiro / amp / copilot / roo).
  - Fixtures: `crates/ralph-core/tests/fixtures/{kiro,kiro-acp}/`
    removed; `crates/ralph-core/tests/scenarios/mixed_backends.yml`
    removed (referenced kiro backend); `smoke_runner.rs` `kiro_smoke_tests`
    + `kiro_acp_smoke_tests` modules removed.
  - Docs: `docs/guide/kiro-migration.md` and `docs/guide/roo-backend.md`
    deleted; `docs/guide/index.md` table entries removed;
    `docs/deployment/qchat-production.md` deprecation warning updated;
    `.cursor/rules/architecture-modules.mdc` and `feature-flags.mdc`
    backend lists updated.
  - `ralph-e2e`: `Backend::Kiro` enum variant deleted from
    `crates/ralph-e2e/src/backend.rs` (and propagated through
    `auth.rs` / `runner.rs` / all `scenarios/*.rs`).

### Changed

- State projection topic activation is now opt-in by configured action key; custom presets must explicitly declare topics. Added atomic `ensure_task_batch` projection for task DAG materialization with all-or-nothing validation. Plan: `docs/plans/2026-07-28-001-fix-parallel-forge-dispatch-contract-plan.md` U1.

### Added

- New generic isolated fixture coverage for commit-aware over-emit recovery (`generic_isolated_committed_first_keeps_handoff` / `generic_isolated_zero_commit_injects_one_resume` / `generic_isolated_terminal_and_default_publish_unchanged`); commit-first decision contract for the recovered `task.resume` path. Plan: `docs/plans/2026-07-28-001-fix-parallel-forge-dispatch-contract-plan.md` U3.

- Parallel Forge task authority: planner no longer calls task mutation CLI; new `preset.instructions_task_mutation_authority_conflict` lint rejects agent-side mutation in projection-owned hats at preset-load time. Plan: `docs/plans/2026-07-28-001-fix-parallel-forge-dispatch-contract-plan.md` U2.

- New TDD tests covering the backend deletion: `test_valid_backends_does_not_contain_*`
  in `backend_support.rs`; `test_default_priority_does_not_contain_*` in
  `auto_detect.rs`; `test_copilot_stream_module_removed` /
  `test_acp_executor_module_removed` in `ralph-adapters/src/lib.rs`;
  `test_build_roo_prompt_file_helper_removed` in `cli_backend.rs`;
  `test_hat_backend_kiro_agent_variant_removed` in
  `ralph-core/src/config/hat.rs`; `test_backend_enum_excludes_kiro`
  + `test_kiro_cases_removed_from_scenarios` in `ralph-e2e`;
  `test_kiro_fixtures_dir_removed` /
  `test_kiro_acp_fixtures_dir_removed` /
  `test_mixed_backends_scenario_excludes_deleted_backends` in
  `ralph-core/tests/smoke_runner.rs`;
  `test_minimal_preset_files_exclude_deleted_backends` /
  `test_zsh_plugin_backend_array_excludes_deleted_backends` /
  `test_tools_evaluate_scripts_exclude_kiro` in `crates/ralph-cli/src/presets.rs`.

### Historical Note (state projection phase 1)

> The entries below describe the original `state projection phase 1`
> rollout (plan `2026-06-17-003`). The named presets (`ce-executor-isolated`,
> `ce-executor-serial`) were the active builtins at that time; both have
> since been retired — `ce-executor-isolated` was removed in 2026-06-23,
> `ce-executor-serial` was retired as a public builtin by plan
> `2026-07-07-006`. Kept verbatim so the rollout history stays auditable;
> current builtins are listed in `presets/manifest.yml`.

### Added

- State projection: the orchestrator is the canonical writer for
  `.ralph/agent/tasks.jsonl` and `.ralph/agent/progress.md`. New
  `state_projector` module drives both ledgers from the inbound
  event batch; the new `## ORCHESTRATOR CONTEXT` block exposes the
  live values to every hat prompt so the agent never has to
  hand-read a ledger. Opt-in via
  `event_loop.state_projection.enabled`; the `ce-executor-isolated`
  and `ce-executor-serial` presets opt in by default. Plan:
  `docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md`.

### Changed

- The two `ce-executor` presets now route task creation and
  progress updates through the projector. Per-hat instructions
  still reference the legacy hand-written paths; the new HARD
  RULE comment in `event_loop.state_projection` documents the
  orchestrator-owned surfaces. Fail-closed on bad payloads —
  rejected events are dropped with an
  `event.state_projection.rejected` diagnostic and never
  reach the bus.

### Fixed

- Multi-event batches where two events share a topic and one
  fails the projector used to drop the whole topic. The hook
  now retains by `(topic, payload)` so only the matching
  rejection is dropped; sibling events of the same topic
  survive. P0 fix in review 2026-06-17-003.
- `state_projection.project_plan_complete` now closes any open
  tasks in `tasks.jsonl`. The previous behaviour only touched
  `progress.md`, leaving stale open rows that the U4
  `progress_task_gate` would reject on the next step. P1 fix.
- `runtime_state.derive_plan_name` now reads the plan name from
  the canonical `task.key` shape
  (`ce-executor:<plan>:<step>:<unit>`) instead of the
  free-form `description` field. An agent that overwrites its
  own description can no longer poison the snapshot. P2 fix.

- Claude child sessions now default to `--setting-sources project,local`, preventing host user-level `~/.claude/settings.json` hooks, plugins, and MCP servers from leaking into Ralph orchestration runs. Users who want the old behavior can opt back in with `cli.args: ["--setting-sources", "user,project,local"]`.

## [2.9.2] - 2026-04-10

### Changed

- Added a post-publish `cargo install` smoke test to the release workflow for faster detection of broken release artifacts.
- Expanded `ralph doctor` auth environment variable detection for the Pi and Roo backends.
- Documented the Pi backend in the backend guide.

### Fixed

- Fixed Claude + TUI blank/stuck output on large prompts by avoiding the PTY stdin deadlock in non-interactive PTY execution.

## [2.9.1] - 2026-04-04

### Fixed

- `cargo install ralph-cli` fails due to cross-crate `#[path]` include of `tool_preview.rs` from `ralph-adapters` into `ralph-tui`. Moved `tool_preview` to a public module in `ralph-adapters` and added it as a proper dependency.

## [2.9.0] - 2026-04-03

### Added

- Agent waves for intra-loop parallel hat execution.
- Current branch display in the TUI header.
- Richer Copilot JSON stream event handling.

### Changed

- Improved rich TUI tool output and the end-to-end harness.
- Shifted coverage and Rust gate validation earlier in CI, with aligned local/GitHub checks.
- Refreshed documentation links and coverage publishing plumbing.

### Fixed

- TUI mouse toggle behavior.
- Scratchpad path resolution for loop state.
- Large prompt handoff for arg-based backends by routing through temp files.
- CLI executor streaming and inactivity-timeout handling.
- Self-loop event publishing guidance and backend contract validation coverage.

## [2.8.1] - 2026-03-16

### Changed

- Version bump for 2.8.1 release.

## [2.8.0] - 2026-03-10

### Added

- `ralph mcp serve` for exposing Ralph as a workspace-scoped MCP server over stdio.
- User-scoped default config discovery and support for per-user Ralph defaults.
- TUI update availability notices in the header.
- Human guidance can now trigger a clean restart request flow.

### Changed

- Consolidated the core preset set around the maintained workflows and refreshed preset docs, examples, and evaluation tooling.
- Refined PDD and code-task guidance to reduce Ralph-specific noise and improve handoff quality.

### Fixed

- Hardened multi-hat preset event contracts, late-event recovery, active hat display, and downstream debug/review handoffs.
- Preserved runtime limits from core config when using hats.
- Fixed headless loop runner backend selection.
- Made restart resumption use the required single-command shell flow and added contract coverage for it.

## [2.7.0] - 2026-03-06

### Added

- Per-project orchestrator lifecycle hooks v1.
- `kiro-acp` backend with ACP executor support.
- Subprocess TUI over JSON-RPC stdin/stdout.
- Improved TUI tool rendering for ACP-backed flows.

### Changed

- Simplified internal code paths by removing redundant clones and deduplicating `now_ts`.
- Replaced deprecated `Duration` method usage with `from_secs`.
- `ralph plan` PDD SOP now syncs from the canonical `strands-agents/agent-sop` upstream source, with a small Ralph-specific loop handoff addendum.
- Added embedded asset sync, check, and upstream refresh helpers for SOP maintenance.
- Unified and modernized preset documentation.
- Added `llms.txt` map generation and CI validation.
- Hardened web `tsx` preflight behavior and added funding metadata.

### Fixed

- Avoid self-lock contention in subprocess TUI mode.
- Accumulate Pi text deltas into flowing paragraphs in the TUI.
- Clean up zombie worktree loops more reliably.
- Fix ACP orphaned processes, garbled TUI output, and missing tool details.
- Resolve clippy issues and missing struct fields.

## [2.6.0] - 2026-02-25

### Added

- Rust RPC v1 control plane and web client migration to the new RPC contract.
- Shell completions support for `ralph` CLI.
- `fresh-eyes` preset with enforced review passes.

### Fixed

- Hat display no longer gets stuck on the previous iteration's hat.
- UTF-8 safe truncation to prevent panics on multi-byte characters.
- Hat-level backend shorthand `args` is honored for custom hats (including OpenCode).
- Deprecated `project.*` config keys now fail fast with a clear migration hint to `core.*`.

## [2.5.1] - 2026-02-14

### Changed

- Version bump for 2.5.1 release.

## [2.3.0] - 2025-01-28

### Added

- **Web Dashboard (Alpha)**: Full-featured web UI for monitoring and managing Ralph orchestration loops
  - React + Vite + TailwindCSS frontend with Fastify + tRPC + SQLite backend
  - `ralph web` command to launch both servers (backend:3000, frontend:5173)
  - Preflight checks and auto-install for fresh installs
  - Port conflict detection, labeled output, and automatic browser open
  - Node 22 pinned for backend dev with tsc+node compilation
- **Hats CLI**: Topology visualization and AI-powered diagrams (`ralph hats`)
- **Event Publishing Guide**: Skip topology display when a hat is already active
- **Parallel config gate**: `features.parallel` config option to control worktree spawning
- **Per-hat backend args**: `args` support in hat-level backend configurations
- **New presets**: Additional presets and improved workflow patterns
- **Documentation**: Reorganized docs with governance files and enhanced README

### Fixed

- Honor hat-level backend configuration and args overrides
- Backend dev workflow uses tsc+node instead of ts-node

## [2.2.5] - 2025-01-17

### Added

- Loop merge command (`ralph loop merge`) and custom backend args
- Config override support for core fields via CLI
- Mock adapter for cost-free E2E testing
- CI: Run mock E2E tests on every PR/push

### Fixed

- CI workaround for claude-code-action fork PR bug
- CI write permissions for handling fork PRs

## [2.2.4] - 2025-01-14

### Fixed

- TUI hang under npx process group
- Clarify cost display as estimate for subscription users

## [2.2.3] - 2025-01-12

### Added

- Multi-loop concurrency via git worktrees
- OBJECTIVE section in prompts to prevent goal drift
- Claude Code GitHub workflow

### Fixed

- UTF-8 truncation panics in event output

### Changed

- Updated preset configurations

## [2.2.2] - 2025-01-10

### Fixed

- Signal handler registration moved after TUI initialization
- Docs: markdown attribute on divs for badge rendering

## [2.2.1] - 2025-01-08

### Added

- CLI ergonomics: backend flag, builtin presets, URL configs
- Comprehensive MkDocs documentation site for v2

### Fixed

- TUI: require stdin to be terminal for TUI enablement
- MkDocs strict build failures
- Confession-loop preset updated to use `ralph emit` command

### Changed

- Modularized codebase and fixed TUI mode

[Unreleased]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.8.0...HEAD
[2.8.0]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.7.0...v2.8.0
[2.7.0]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.6.0...v2.7.0
[2.6.0]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.5.1...v2.6.0
[2.5.1]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.5.0...v2.5.1
[2.3.0]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.2.5...v2.3.0
[2.2.5]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.2.4...v2.2.5
[2.2.4]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.2.3...v2.2.4
[2.2.3]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.2.2...v2.2.3
[2.2.2]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.2.1...v2.2.2
[2.2.1]: https://github.com/mikeyobrien/ralph-orchestrator/compare/v2.2.0...v2.2.1
