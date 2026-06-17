# Changelog

All notable changes to ralph-orchestrator are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
