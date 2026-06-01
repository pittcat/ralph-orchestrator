# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> The orchestrator is a thin coordination layer, not a platform. Agents are smart; let them do the work.

## Build & Test

```bash
# Recommended parallel path (requires cargo-nextest; install once via `just nextest-install`).
# ralph-cli tests run serially via the cli-serial test group in .config/nextest.toml;
# every other package runs in parallel. Doctest is covered by a separate cargo test --doc step.
./scripts/run-tests.sh                                # nextest run + cargo test --doc, with fallback to cargo test
just test-parallel                                    # alias for scripts/run-tests.sh
cargo nextest run --workspace --exclude ralph-e2e     # non-doctest tests, parallel (requires nextest)
cargo test --workspace --exclude ralph-e2e --doc      # doctest coverage (nextest does not run doctest)

# Fallback path (no nextest required; same semantics as the historical CI gate).
cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output
just test-serial                                      # alias for the single-threaded slow path

# Other build, lint and test commands.
cargo build
cargo test -p ralph-core test_name           # Run single test
cargo test -p ralph-core smoke_runner        # Smoke tests (replay-based)
cargo test -p ralph-core scenarios           # BDD scenario integration tests
cargo run -p ralph-e2e -- --mock             # E2E tests (CI-safe)
cargo clippy                                 # Lint (pedantic configured in workspace)
cargo fmt                                    # Format
cargo doc --no-deps                          # Documentation
./scripts/setup-hooks.sh                     # Install pre-commit hooks (once)
```

**IMPORTANT**: Run `cargo test` (or `./scripts/run-tests.sh` if nextest is installed) before declaring any task done. Smoke test after code changes.

### Web Dashboard

```bash
ralph web                                    # Launch both servers (backend:3000, frontend:5173)
npm install                                  # Install all dependencies
npm run dev                                  # Dev mode (both)
npm run dev:server                           # Backend only
npm run dev:web                              # Frontend only
npm run test:server                          # Backend tests
npm run test                                 # All npm workspace tests
```

### BDD / Cucumber Tests

```bash
cargo test -p ralph-core scenarios            # BDD scenario tests
```

BDD scenarios live in `crates/ralph-core/tests/scenarios/` (YAML files). They exercise real runtime code paths via integration tests.

## Architecture

### Crate Map

```
ralph-proto    → Foundation types: Event, Hat, HatId, Topic, EventBus, RobotService
ralph-core     → Orchestration logic, event loop, state machine, hats, memories, tasks, hooks, skills
ralph-adapters → Backend adapters (Claude, Kiro, Gemini, Codex, Amp, Copilot CLI, OpenCode)
ralph-cli      → CLI entry point, commands (run, plan, task, loops, web, mcp, wave, bot)
ralph-telegram → Telegram bot for human-in-the-loop communication
ralph-tui      → Terminal UI (ratatui-based)
ralph-e2e      → End-to-end test framework (scenarios, mock CLI, reporters)
ralph-api      → Rust RPC API server for web dashboard backend
ralph-bench    → Benchmarking

backend/       → Web server (@ralph-web/server) - Fastify + tRPC + SQLite (legacy, deprecated)
frontend/      → Web dashboard (@ralph-web/dashboard) - React + Vite + TailwindCSS
```

### Event System Architecture

```
JSONL (agent output) → EventReader → EventParser → EventOriginGuard → EventPolicy → StateMachine → EventBus → Hats
```

- **EventReader**: Reads JSONL lines from agent output files, handles malformed lines
- **EventParser**: Parses JSONL into structured events, detects event mutations
- **EventOriginGuard** (`event_origin.rs`): Validates event provenance — rejects events from unregistered hats or out-of-scope topics. Implements fail-closed security model.
- **EventPolicy** (`event_policy.rs`): Enforces typed payload schema, terminal monotonicity, duplicate terminal detection, business-after-completion guards
- **StateMachine** (`state_machine.rs`): Pure-Rust instance lifecycle enforcement (open → active → terminal), no filesystem dependencies
- **EventBus** (`ralph-proto/src/event_bus.rs`): Pub/sub hub routing events to subscribed hats with observer pattern for recording/TUI
- **EventProjection** (`event_projection.rs`): Transforms/redacts events before forwarding

### Backend Adapter Architecture

```
CLI Config → HaBackend → auto_detect → CliExecutor / PtyExecutor → StreamHandler
```

- **CliBackend**: Defines backend CLI path, args, prompt mode, output format
- **CliExecutor**: Spawns backend process, streams output, handles termination
- **PtyExecutor**: PTY-based execution for Claude CLI (preserves rich TUI output), supports interactive and observe modes
- **auto_detect**: Scans PATH for available backends (`claude`, `code`/`codex`, `gemini`, `kiro`, `amp`, `copilot`, `opencode`)
- **StreamHandler**: Console/pretty/quiet/TUI output handlers for displaying agent output
- **Backend-specific stream parsers**: `claude_stream.rs`, `copilot_stream.rs`, `pi_stream.rs`, `json_rpc_handler.rs`

### Configuration System

Config supports both v1.x flat format and v2.0 nested format for zero-config migration. Resolution chain:

```
User YAML → RalphConfig → EventLoopConfig → HatConfig overrides → effective runtime config
```

Key config modules in `crates/ralph-core/src/config.rs`:
- `RalphConfig`: Top-level config
- `CoreConfig`: Event loop, scratchpad, memories, tasks
- `HatConfig`: Per-hat backend, triggers, publishes, instructions, concurrency
- `EventPolicyConfig`: Schema validation, payload types, terminal event handling
- `EventFilterConfig`: Filter events by topic patterns
- `StateMachineConfig`: Instance lifecycle rules
- `PreflightExtensionsConfig`: External command hooks
- `EventProjectionConfig`: Event transformation/redaction rules
- `FeaturesConfig`: Feature flags (memories, tasks, loop naming, urgent steer)

### Hook System

Hooks run external commands at lifecycle points. Located in `crates/ralph-core/src/hooks/`:

- **HookEngine**: Manages hook lifecycle (discovery, resolution, execution)
- **HookExecutor**: Runs external commands as hooks with timeout and output capture
- **SuspendStateStore**: Persists suspend state across loop restarts

Hook stages: `pre_agent` → `post_agent` → `pre_event_processing` → `post_event_processing` → `completion`

### Skill System

Skills are markdown documents with YAML frontmatter providing knowledge and tool instructions to agents. Located in `crates/ralph-core/src/skill.rs` and `skill_registry.rs`:

- Support auto-injection, hat-scoping, backend-scoping, tags
- Both built-in (compiled via `include_str!`) and filesystem sources
- Registry discovers and indexes skills from multiple sources

### Session Recording & Playback

- **SessionRecorder** (`session_recorder.rs`): Records all events to JSONL files (behind `recording` feature flag)
- **SessionPlayer** (`session_player.rs`): Replays recorded sessions for smoke tests, supports step-through and replay modes
- Fixtures stored in `crates/ralph-core/tests/fixtures/`

### Presets & Hats System

Presets define collections of hats. Located in `presets/` directory and `crates/ralph-cli/src/presets.rs` (~1100 lines):

- **HatlessRalph** (`hatless_ralph.rs`): Hat topology, event subscription matching, hat selection algorithm
- **HatRegistry**: Manages hat discovery, registration, subscription
- Presets support Chinese (`*-zh.yml`) variants and chainable configurations
- Builtin presets: `code-assist`, `debug`, `research`, `review`, `pdd-to-code-assist`, `ce-executor`, `autoresearch`, `harness-demo`, `hatless-baseline`
- `index.json` is the preset manifest

### Key Files

| File | Purpose |
|------|---------|
| `.ralph/agent/memories.md` | Persistent learning across sessions |
| `.ralph/agent/tasks.jsonl` | Runtime work tracking |
| `.ralph/loop.lock` | Contains PID + prompt of primary loop |
| `.ralph/loops.json` | Registry of all tracked loops |
| `.ralph/merge-queue.jsonl` | Event-sourced merge queue |
| `.ralph/telegram-state.json` | Telegram bot state (chat ID, pending questions) |
| `docs/solutions/` | Documented solutions to past problems (bugs, best practices, workflow patterns), organized by category with YAML frontmatter (`module`, `tags`, `problem_type`). Relevant when implementing or debugging in documented areas. |
| `docs/guide/harness-extensions.md` | User guide for Harness 4 extension mechanisms (event filtering, projection, state injection, preflight hooks) |
| `presets/COLLECTION.md` | Preset metadata and authorship docs |
| `presets/index.json` | Preset manifest index |
| `crates/ralph-core/data/` | Embedded tool definitions (`ralph-tools.md`, `ralph-tools-tasks.md`, `ralph-tools-memories.md`) |

### Code Locations

- **Event loop**: `crates/ralph-core/src/event_loop/` — main orchestration loop (`mod.rs`), loop state (`loop_state.rs`)
- **Hat system**: `crates/ralph-core/src/hatless_ralph.rs`, `hat_registry.rs`
- **State machine**: `crates/ralph-core/src/state_machine.rs` — instance lifecycle enforcement
- **Event policy**: `crates/ralph-core/src/event_policy.rs` — schema validation, terminal monotonicity
- **Event origin**: `crates/ralph-core/src/event_origin.rs` — JSONL provenance guard
- **Event projection**: `crates/ralph-core/src/event_projection.rs` — event transformation/redaction
- **Memory system**: `crates/ralph-core/src/memory.rs`, `memory_store.rs`
- **Task system**: `crates/ralph-core/src/task.rs`, `task_store.rs`, `task_definition.rs`
- **Hook system**: `crates/ralph-core/src/hooks/` — engine, executor, suspend state
- **Skill system**: `crates/ralph-core/src/skill.rs`, `skill_registry.rs`
- **Lock coordination**: `crates/ralph-core/src/worktree.rs`, `loop_lock.rs`, `file_lock.rs`
- **Loop registry**: `crates/ralph-core/src/loop_registry.rs`
- **Merge queue**: `crates/ralph-core/src/merge_queue.rs`
- **Config**: `crates/ralph-core/src/config.rs` — all config types, v1/v2 format compatibility
- **CLI commands**: `crates/ralph-cli/src/` — `loop_runner.rs`, `loops.rs`, `task_cli.rs`, `wave.rs`, `bot.rs`, `web.rs`, `mcp.rs`, `init.rs`, `hats.rs`, `presets.rs`, `hooks.rs`, `tools.rs`, `doctor.rs`
- **Telegram integration**: `crates/ralph-telegram/src/` (bot, service, state, handler)
- **RObot config**: `crates/ralph-core/src/config.rs` (`RobotConfig`, `TelegramBotConfig`)
- **Wave system**: `crates/ralph-core/src/wave_tracker.rs`, `wave_detection.rs`, `wave_prompt.rs`
- **Wave CLI**: `crates/ralph-cli/src/wave.rs`
- **Adapters**: `crates/ralph-adapters/src/` — `cli_backend.rs`, `cli_executor.rs`, `pty_executor.rs`, `auto_detect.rs`, stream parsers
- **Preflight checks**: `crates/ralph-core/src/preflight.rs` — acceptance criteria extraction and validation
- **Harness extensions**: `crates/ralph-core/src/config.rs` (config schema), `event_loop/mod.rs` (integration), `event_projection.rs`, `state_file_injector.rs`, `preflight.rs` (external command hooks)
- **Web server**: `backend/ralph-web-server/src/` (tRPC routes in `api/`, runners in `runner/`)
- **Web dashboard**: `frontend/ralph-web/src/` (React components in `components/`)
- **E2E tests**: `crates/ralph-e2e/src/` — scenarios in `scenarios/`, mock CLI, reporter
- **BDD scenarios**: `crates/ralph-core/tests/scenarios/` — YAML-based integration test scenarios
- **Smoke fixtures**: `crates/ralph-core/tests/fixtures/` — recorded JSONL for replay tests
- **Ralph proto types**: `crates/ralph-proto/src/` — Event, Hat, HatId, Topic, EventBus, RobotService, CheckinContext

## The Ralph Tenets

1. **Fresh Context Is Reliability** — Each iteration clears context. Re-read specs, plan, code every cycle. Optimize for the "smart zone" (40-60% of ~176K usable tokens).

2. **Backpressure Over Prescription** — Don't prescribe how; create gates that reject bad work. Tests, typechecks, builds, lints. For subjective criteria, use LLM-as-judge with binary pass/fail.

3. **The Plan Is Disposable** — Regeneration costs one planning loop. Cheap. Never fight to save a plan.

4. **Disk Is State, Git Is Memory** — Memories and Tasks are the handoff mechanisms. No sophisticated coordination needed.

5. **Steer With Signals, Not Scripts** — The codebase is the instruction manual. When Ralph fails a specific way, add a sign for next time.

6. **Let Ralph Ralph** — Sit *on* the loop, not *in* it. Tune like a guitar, don't conduct like an orchestra.

## Anti-Patterns

- ❌ Building features into the orchestrator that agents can handle
- ❌ Complex retry logic (fresh context handles recovery)
- ❌ Detailed step-by-step instructions (use backpressure instead)
- ❌ Scoping work at task selection time (scope at plan creation instead)
- ❌ Assuming functionality is missing without code verification

## Specs & Tasks

- Create specs in `.ralph/specs/` — do NOT implement without an approved spec first
- Create code tasks in `.ralph/tasks/` using `.code-task.md` extension
- Work step-by-step: spec → dogfood spec → implement → dogfood implementation → done

### Memories and Tasks (Default Mode)

Memories and tasks are enabled by default. Both must be enabled/disabled together:

When enabled (default):
- Scratchpad is disabled
- Tasks replace scratchpad for completion verification
- Loop terminates when no open tasks + consecutive LOOP_COMPLETE

To disable (legacy scratchpad mode):
```yaml
memories:
  enabled: false
tasks:
  enabled: false
```

## Parallel Loops

Ralph supports multiple orchestration loops in parallel using git worktrees.

```
Primary Loop (holds .ralph/loop.lock)
├── Runs in main workspace
├── Processes merge queue on completion
└── Spawns merge-ralph for queued loops

Worktree Loops (.worktrees/<loop-id>/)
├── Isolated filesystem via git worktree
├── Symlinked memories, specs, tasks → main repo
├── Queue for merge on completion
└── Exit cleanly (no spawn)
```

### Testing Parallel Loops

```bash
cd $(mktemp -d) && git init && echo "<p>Hello</p>" > index.html && git add . && git commit -m "init"

# Terminal 1: Primary loop
ralph run -p "Add header before <p>" --max-iterations 5

# Terminal 2: Worktree loop
ralph run -p "Add footer after </p>" --max-iterations 5

# Monitor
ralph loops
```

## Agent Waves (Intra-Loop Parallelism)

Waves enable a single hat to process multiple work items in parallel within one iteration.

### Hat Config Fields

```yaml
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 4              # Max parallel workers (default: 1)
    instructions: "..."

  synthesizer:
    triggers: ["review.done"]
    publishes: ["review.complete"]
    aggregate:                   # Buffer results until all arrive
      mode: wait_for_all
      timeout: 300               # Seconds to wait
```

- `concurrency > 1` enables wave execution for a hat
- `aggregate` makes a hat wait for all wave results before activating
- A hat cannot have both `concurrency > 1` and `aggregate`

### Wave Dispatch

Agents dispatch waves via CLI:
```bash
ralph wave emit review.file --payloads "src/main.rs" "src/lib.rs" "src/config.rs"
```

### How It Works

1. Agent emits wave events (tagged with shared `wave_id`)
2. Loop runner detects wave events, resolves target hat
3. Spawns N parallel backend instances (up to `concurrency` limit)
4. Each worker gets: focused prompt, per-worker events file, wave env vars
5. Results merged back to main events file
6. Aggregator hat picks up results on next iteration

### Key Code Locations

- **Wave CLI**: `crates/ralph-cli/src/wave.rs`
- **Wave detection**: `crates/ralph-core/src/wave_detection.rs`
- **Worker prompt**: `crates/ralph-core/src/wave_prompt.rs`
- **Wave tracker**: `crates/ralph-core/src/wave_tracker.rs`
- **Loop integration**: `crates/ralph-cli/src/loop_runner.rs` (`execute_wave`)

### Presets

- `presets/wave-review.yml` — Scatter-gather code review

## Smoke Tests (Replay-Based)

Smoke tests use recorded JSONL fixtures instead of live API calls:

```bash
cargo test -p ralph-core smoke_runner        # All smoke tests
cargo test -p ralph-core kiro                # Kiro-specific
```

**Fixtures location:** `crates/ralph-core/tests/fixtures/`

### Recording New Fixtures

```bash
cargo run --bin ralph -- run -c ralph.claude.yml --record-session session.jsonl -p "your prompt"
```

## E2E Testing

```bash
cargo run -p ralph-e2e -- claude             # Live API tests
cargo run -p ralph-e2e -- --mock             # CI-safe mock mode
cargo run -p ralph-e2e -- --mock --filter connect  # Filter scenarios
cargo run -p ralph-e2e -- --list             # List scenarios
```

Reports generated in `.e2e-tests/`.

## RObot (Human-in-the-Loop)

Ralph supports human interaction during orchestration via Telegram. Agents can ask questions and humans can send proactive guidance.

### Configuration

```yaml
# ralph.yml
RObot:
  enabled: true
  timeout_seconds: 300    # How long to block waiting for a response
  telegram:
    bot_token: "your-token"  # Or set RALPH_TELEGRAM_BOT_TOKEN env var
```

### Event Types

| Event / Command | Direction | Purpose |
|-------|-----------|---------|
| `human.interact` | Agent to Human | Agent asks a question; loop blocks until response or timeout |
| `human.response` | Human to Agent | Reply to a `human.interact` question |
| `human.guidance` | Human to Agent | Proactive guidance injected as `## ROBOT GUIDANCE` in prompt |
| `ralph tools interact progress` | Agent to Human | Non-blocking progress notification via Telegram (no event, direct send) |

### How It Works

- The Telegram bot starts only on the **primary loop** (the one holding `.ralph/loop.lock`)
- When an agent emits `human.interact`, the event loop sends the question via Telegram and **blocks**
- Responses are published as `human.response` events on the bus
- Proactive messages become `human.guidance` events, squashed into a numbered list in the prompt
- Send failures retry with exponential backoff (3 attempts); if all fail, treated as timeout
- Parallel loops route messages via reply-to, `@loop-id` prefix, or default to primary

See `crates/ralph-telegram/README.md` for setup instructions.

## Diagnostics

TUI mode always logs to `.ralph/diagnostics/logs/ralph-{timestamp}.log` (last 5 kept automatically).

```bash
RALPH_DIAGNOSTICS=1 ralph run -p "your prompt"
```

Output in `.ralph/diagnostics/<timestamp>/`:
- `agent-output.jsonl` — Agent text, tool calls, results
- `orchestration.jsonl` — Hat selection, events, backpressure
- `errors.jsonl` — Parse errors, validation failures

```bash
jq 'select(.type == "tool_call")' .ralph/diagnostics/*/agent-output.jsonl
ralph clean --diagnostics
```

## IMPORTANT

- 讨论 ralph-orchestrator 的任何功能、架构、行为时，必须先去读源码确认，不允许凭记忆或猜测讨论
- Run `cargo test` before declaring any task done
- Backwards compatibility doesn't matter — it adds clutter for no reason
- Prefer replay-based smoke tests over live API calls for CI
- BDD/Cucumber tests MUST exercise real runtime code paths via integration tests (not placeholder/source-only assertions)
- Run python tests using a .venv
- You MUST not commit ephemeral files
- When I ask you to view something that means to use playwright/chrome tools to go view it.
- When adding or changing `ralph tools` subcommands, update the appropriate file in `crates/ralph-core/data/`: `ralph-tools.md` (shared commands), `ralph-tools-tasks.md` (task commands), or `ralph-tools-memories.md` (memory commands). `.claude/skills/ralph-tools/SKILL.md` is a symlink to the base `ralph-tools.md`
- When adding, removing, renaming, or changing builtin hat collections/presets in `crates/ralph-cli/src/presets.rs` or mirrored preset files, update `scripts/ralph-zsh-plugin.zsh` so `ralph run -H builtin:<TAB>` stays accurate. Preserve the current `compadd`-based completion style for values containing `:`; do not use `_describe` for `builtin:*` values. After updating the script, install it for the current user with `cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh` and verify zsh completion loads.
- Design docs and specs go in `.ralph/specs` and one-off code tasks and bug fixes go in `.ralph/tasks`
- **所有中文输出规则**：无论使用哪个 skill 进行操作，所有面向人类的输出——包括但不限于计划文档、设计文档、需求文档、实施计划、任务文件、报告、总结、注释说明、代码 review 意见、PR 描述等——都必须使用中文撰写。不影响：文件名、代码中的字符串字面量、代码注释中的技术标识符（如变量名、函数名、crate 名）、命令行输出块。这条规则优先于任何 skill 内置的语言默认值。
- **CLAUDE.md 与 AGENTS.md 同步规则**：这两个文件必须保持内容完全一致。修改其中一个时，必须同步更新另一个，确保不会出现差异。
