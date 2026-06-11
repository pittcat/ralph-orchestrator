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

BDD scenarios (YAML, exercise real runtime paths) live in `crates/ralph-core/tests/scenarios/`.

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

Key config modules in `crates/ralph-core/src/config/`:
- `RalphConfig`: Top-level config
- `CoreConfig`: Event loop, scratchpad, memories, tasks
- `HatConfig`: Per-hat backend, triggers, publishes, instructions, concurrency
- `EventPolicyConfig`: Schema validation, payload types, terminal event handling
- `EventFilterConfig`: Filter events by topic patterns
- `StateMachineConfig`: Instance lifecycle rules
- `PreflightExtensionsConfig`: External command hooks
- `EventProjectionConfig`: Event transformation/redaction rules
- `FeaturesConfig`: Feature flags (memories, tasks, loop naming, urgent steer)

### Multi-Hat Isolation Policy（强制）

`event_loop.execution_mode` 与 hat 数量配合时遵循**固定阈值**，没有豁免路径：

- **3-hat 上限（coordinator 模式）**：`hats` 数 ≤ 3 时，preset 可显式 `execution_mode: coordinator`（默认）或 `execution_mode: isolated`，任一可启动。
- **4+ hats 必须显式 isolated**：`hats` 数 ≥ 4 时，`event_loop.execution_mode: isolated` 是**强制**配置；缺少该字段、值不是 `isolated`、或被注释掉，preset 启动即被 `preset_lint::check_multi_hat_isolation` 拒绝。
- **错误消息固定**：`preset declares N hats which exceeds the coordinator limit of 3; set \`event_loop.execution_mode: isolated\` to run this preset`，调用方按字面匹配即可定位根因。
- **无豁免**：环境变量（`RALPH_ALLOW_COORDINATOR_OVERRIDE` 等）、测试开关、preset 名称维护的 exemption 均**不可用**；所有 builtin 4+ hat preset 均已迁移到 isolated（见 U6 commit `2a29e24`）。

#### Isolated 终态 Authority（U3）

在 `execution_mode: isolated` 下，**所有 agent 终态**（completion、review verdict、report completion、plan blocked）必须在 hat 的 `publishes` 列表中显式声明：

- 未在 `publishes` 中声明的终态主题（如裸 `LOOP_COMPLETE`、未声明的 `report.done`、未声明的 `review.complete`）被 `EventOriginGuard` 直接拒绝；
- 拒绝行为 emit `event.isolation.boundary_violation` 诊断事件到 `recovery.jsonl`，并把 `task.resume` 注入事件流以便 agent 看到失败原因；
- 同一规则适用于 `default_publishes` 兜底：兜底主题必须也在 `publishes` 中，否则被原 guard 拒绝。
- 唯一例外是 `hat=ralph` 运行时内置 hat：它由 `HatRegistry::from_runtime_config()` 注入 `LOOP_COMPLETE` / `work.start` / `loop.cancel`，这是单一豁免。

#### Isolated Fair Scheduling（U4）

`EventBus` 在 isolated 模式下使用**轮转游标（round-robin cursor）**而非字典序首项来选择下一个 hat：

- 同一事件后多 hat 都 pending 时，cursor 推进到当前 hat 之后，下一次从该位置继续；
- 防止「一直先选 hat 名靠前的」造成的饥饿；
- 字典序首项行为已**移除**，不要在文档、测试、prompt 中描述旧行为。

#### 上限 + 调度 + 终态 的交互

- 3-hat coordinator preset 不受 isolated 终态 authority / fair scheduling 约束（它走单 prompt 多 hat 路径）。
- 4+ hat isolated preset **必须**同时满足：execution_mode=isolated、每个 hat 的所有终态主题在 `publishes` 中显式声明、字典序不再被依赖。
- 任何 preset 校验失败都被 `ralph preset check` / `ralph preflight` / `ralph run` 启动硬门拦住，运行时不会进入半启动状态。

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
- Builtin presets: `autoresearch`, `ce-executor-isolated`, `ce-executor-lite` (template), `ce-executor-wave`, `code-assist`, `debug`, `merge-loop`, `pdd-to-code-assist`, `research`, `review`（裸 `ce-executor` 已删除：所有 plan-driven 执行请使用 `ce-executor-isolated`；仅作模板时可使用 `ce-executor-lite`）
- `presets/index.json` is the user-facing preset manifest

**`presets/manifest.yml` 是 builtin preset 的 single source of truth**（`crates/ralph-cli/build.rs` 和 `crates/ralph-cli/src/presets.rs` 都从这里读取并在不一致时 panic）。新增/重命名/删除一个 builtin preset 必须**同步改 4 处**：

1. `presets/en/<name>.yml`（实际 YAML）
2. `presets/manifest.yml` 的 `embedded:` 列表
3. `crates/ralph-cli/src/presets.rs` 的 `PRESETS` 数组（`EmbeddedPreset { name, description, content, public }`）
4. `presets/index.json`（如对用户可见）
5. 同步更新本文件 Presets & Hats 段的 builtin preset 列表，以及 `scripts/ralph-zsh-plugin.zsh` 的 zsh 补全

### Key Files

| File | Purpose |
|------|---------|
| `.ralph/agent/memories.md` | Persistent learning across sessions |
| `.ralph/agent/tasks.jsonl` | Runtime work tracking |
| `.ralph/loop.lock` | Contains PID + prompt of primary loop |
| `.ralph/loops.json` | Registry of all tracked loops. Each `LoopEntry` has `worktree_path: Option<String>` and `workspace: String`: in worktree mode both equal the worktree absolute path; in primary mode `worktree_path` is `None` and `workspace` is the main repo root. `None` vs `Some(_)` is the canonical primary-vs-worktree signal consumed by `ralph loops list`, `is_alive()` checks, and the web dashboard's domain model. The registry is shared across worktrees and always lives in the main repo (the only `.ralph/` artifact that does). |
| `.ralph/merge-queue.jsonl` | Event-sourced merge queue |
| `.ralph/telegram-state.json` | Telegram bot state (chat ID, pending questions) |
| `docs/solutions/` | Documented solutions to past problems (bugs, best practices, workflow patterns), organized by category with YAML frontmatter (`module`, `tags`, `problem_type`). Relevant when implementing or debugging in documented areas. |
| `docs/guide/harness-extensions.md` | User guide for Harness 4 extension mechanisms (event filtering, projection, state injection, preflight hooks) |
| `presets/COLLECTION.md` | Preset metadata and authorship docs |
| `presets/index.json` | Preset manifest index |
| `crates/ralph-core/data/` | Embedded tool definitions (`ralph-tools.md`, `ralph-tools-tasks.md`, `ralph-tools-memories.md`) |

### Code Locations

| Module | Path | Purpose |
|---|---|---|
| Event loop | `crates/ralph-core/src/event_loop/` | `mod.rs` (main loop), `loop_state.rs` |
| Hat system | `crates/ralph-core/src/hatless_ralph.rs`, `hat_registry.rs` | Topology, subscription matching, selection |
| State machine | `crates/ralph-core/src/state_machine.rs` | Instance lifecycle (`open → active → terminal`) |
| Event policy | `crates/ralph-core/src/event_policy.rs` | Schema, terminal monotonicity |
| Event origin | `crates/ralph-core/src/event_origin.rs` | JSONL provenance guard (fail-closed) |
| Event projection | `crates/ralph-core/src/event_projection.rs` | Transform / redact events |
| Memory | `crates/ralph-core/src/memory.rs`, `memory_store.rs` | Persistent learning (markdown) |
| Task | `crates/ralph-core/src/task.rs`, `task_store.rs`, `task_definition.rs` | JSONL work tracking |
| Hooks | `crates/ralph-core/src/hooks/` | engine, executor, suspend state |
| Skills | `crates/ralph-core/src/skill.rs`, `skill_registry.rs` | Discovery, auto-injection |
| Lock coordination | `crates/ralph-core/src/worktree.rs`, `loop_lock.rs`, `file_lock.rs` | Git-worktree + lockfiles |
| Loop registry | `crates/ralph-core/src/loop_registry.rs` | Tracked loops across worktrees |
| Merge queue | `crates/ralph-core/src/merge_queue.rs` | Event-sourced queue |
| Config | `crates/ralph-core/src/config/` | v1/v2 compat; `robot.rs` for RObot |
| CLI commands | `crates/ralph-cli/src/` | `commands/`, `cli/`, `loops.rs`, `task_cli.rs`, `wave.rs`, `bot.rs`, `web.rs`, `mcp.rs`, `init.rs`, `hats.rs`, `presets.rs`, `hooks.rs`, `tools.rs`, `doctor.rs` |
| Telegram | `crates/ralph-telegram/src/` | bot, service, state, handler |
| Wave | `crates/ralph-core/src/wave_tracker.rs`, `wave_detection.rs`, `wave_prompt.rs`; CLI in `crates/ralph-cli/src/wave.rs`; loop dispatch in `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | Intra-loop parallelism |
| Adapters | `crates/ralph-adapters/src/` | `cli_backend.rs`, `cli_executor.rs`, `pty_executor.rs`, `auto_detect.rs`, stream parsers |
| Preflight | `crates/ralph-core/src/preflight.rs` | Acceptance criteria extraction / validation |
| Harness extensions | `crates/ralph-core/src/{config,event_loop,event_projection,state_file_injector,preflight}.rs` | Event filtering, projection, state injection, preflight hooks |
| Web server (Rust) | `crates/ralph-api/src/` | Axum REST/WS for TUI/dashboard |
| Web server (Node, legacy) | `backend/ralph-web-server/src/` | Fastify + tRPC + SQLite (deprecated) |
| Web frontend | `frontend/ralph-web/src/` | React components |
| E2E | `crates/ralph-e2e/src/` | scenarios, mock CLI, reporter |
| BDD scenarios | `crates/ralph-core/tests/scenarios/` | YAML integration scenarios |
| Smoke fixtures | `crates/ralph-core/tests/fixtures/` | Recorded JSONL for replay |
| Proto types | `crates/ralph-proto/src/` | `Event`, `Hat`, `HatId`, `Topic`, `EventBus`, `RobotService`, `DaemonAdapter`, `FrameCapture`, `UxEvent` |

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
- **Loop integration**: `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` (`execute_wave`)

### Presets

- `presets/wave-review.yml` — Scatter-gather code review

## Smoke Tests (Replay-Based)

Use the `smoke_runner` entry point from Build & Test above. Per-backend filters are passed as test name substrings, e.g. `cargo test -p ralph-core -- kiro`.

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

## Runtime Diagnosis

Runtime Diagnosis（U0–U8）是在上述 TUI / full diagnostics 之上的**可观测性 + 自校准层**：把反压点（payload / execution contract、workflow guard、stall、loop stale 等）落 `recovery.jsonl`，把 U5 drift detector 的 3 个指标（field completeness / coord join rate / emit cadence）跌破阈值时落 `drift.jsonl`，loop 终止时把 `## Diagnostics` 段追加到 `.ralph/agent/summary.md`，并写 `diagnosis-summary.json` 种子。

- 启用（env 优先）：`RALPH_DIAGNOSTICS=1 ralph run -c ralph.yml -H builtin:<preset> -p "..."`。仅想写盘不写 prompt alert，可在 `ralph.yml` 配 `telemetry.runtime_diagnosis: { enabled: true, write_artifacts: true, prompt_injection_enabled: true, ... }`。
- 报告：`ralph diagnose --session latest`（Markdown）或 `--format json`（CI，schema_version="1"）；`--diagnostics-root` 可自定义根目录。
- 退出码：`0` 渲染成功 / `2` 无 session / `3` 路径非法 / `4` I/O 失败。
- 8 个 envelope source：`stall_recovery / missing_event_gate / workflow_guard / execution_contract / payload_contract / drift_monitor / hook_retry / loop_stale`；6 个 outcome：`pending / recovered / repeated / escalated / failed / not_retriable`。
- Responder 三档升级：Soft（prompt alert）→ Hard（`task.resume` 路由到 safe target）→ Final（`TerminationHint`，不覆盖 `PayloadContractViolation`）。

详见 `docs/guide/runtime-diagnosis.md`（配置矩阵、report 字段、常见症状排查流程、磁盘文件清单）。

## IMPORTANT

> **以下规则优先级最高，请在动手写任何代码前先完整读完本段。** 任何「先看了某段就开始写」的冲动都应当先回头对照本段。

- 讨论 ralph-orchestrator 的任何功能、架构、行为时，必须先去读源码确认，不允许凭记忆或猜测讨论
- Run `cargo test` before declaring any task done
- Backwards compatibility doesn't matter — it adds clutter for no reason
- Prefer replay-based smoke tests over live API calls for CI
- BDD/Cucumber tests MUST exercise real runtime code paths via integration tests (not placeholder/source-only assertions)
- Run python tests using a .venv
- You MUST not commit ephemeral files
- When I ask you to view something that means to use playwright/chrome tools to go view it.
- When adding or changing `ralph tools` subcommands, update the appropriate file in `crates/ralph-core/data/`: `ralph-tools.md` (shared commands), `ralph-tools-tasks.md` (task commands), or `ralph-tools-memories.md` (memory commands). `.claude/skills/ralph-tools/SKILL.md` is a symlink to the base `ralph-tools.md`
- **反向验证（必须）**：修改 ralph tools 子命令、被这些 skill 文档引用的源码（行号、参数、行为描述）后，必须用 `sed -n 'NN,MMp' <file>` 复核 `crates/ralph-core/data/*.md` 里所有形如 `xxx.rs:NN-MM` 的源码引用范围是否仍指向正确代码。**行号漂移、参数表与代码 clap 定义不符、引用了不存在的命令/字段，都算违规**。改完必须跑一次 `ralph <cmd> --help`（涉及命令语法）或对应 skill 列出的全部命令做冒烟测试（涉及行为）。发现漂移立即在文档里同步修正，不允许文档落后于代码。
- When adding, removing, renaming, or changing builtin hat collections/presets in `crates/ralph-cli/src/presets.rs` or mirrored preset files, update `scripts/ralph-zsh-plugin.zsh` so `ralph run -H builtin:<TAB>` stays accurate. Preserve the current `compadd`-based completion style for values containing `:`; do not use `_describe` for `builtin:*` values. After updating the script, install it for the current user with `cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh` and verify zsh completion loads.
- **builtin preset 改动后**：除上述 zsh 脚本外，还必须同步更新本文件「Presets & Hats System」段的 builtin preset 列表（参见该段对 `presets/manifest.yml` 单一事实源的说明）。文档落后于代码视为违规。
- Design docs and specs go in `.ralph/specs` and one-off code tasks and bug fixes go in `.ralph/tasks`
- **`DEVELOPMENT.md` 已弃用**：它描述的是旧 `specs/` 目录规范，已被 `.ralph/specs/` 取代；请遵循本文件「Specs & Tasks」段的规范。
- **不要手动编辑 `.ralph/` 下的运行时状态文件**（`loop.lock` / `events.jsonl` / `agent/memories.md` / `agent/tasks.jsonl` / `loops.json` / `merge-queue.jsonl` / `telegram-state.json` / `diagnostics/`）。这些由 loop 自己维护；手工改动会与 in-flight 状态错位。确实需要重置时，先停掉所有相关 loop 再用对应 CLI（如 `ralph loops clean`）清理。
- **所有中文输出规则**：无论使用哪个 skill 进行操作，所有面向人类的输出——包括但不限于计划文档、设计文档、需求文档、实施计划、任务文件、报告、总结、注释说明、代码 review 意见、PR 描述等——都必须使用中文撰写。不影响：文件名、代码中的字符串字面量、代码注释中的技术标识符（如变量名、函数名、crate 名）、命令行输出块。这条规则优先于任何 skill 内置的语言默认值。
- **CLAUDE.md 与 AGENTS.md 同步规则**：这两个文件必须保持内容完全一致。修改其中一个时，必须同步更新另一个（推荐 `cp CLAUDE.md AGENTS.md`），确保不会出现差异。

<!-- rtk-instructions v2 -->
# RTK (Rust Token Killer) - Token-Optimized Commands

> **可选优化**：本节是 token 节省提示，不影响项目行为。RTK 不可用时直接跑原命令即可。

## Golden Rule

**Always prefix commands with `rtk`**. If RTK has a dedicated filter, it uses it. If not, it passes through unchanged. This means RTK is always safe to use.

**Important**: Even in command chains with `&&`, use `rtk`:
```bash
# ❌ Wrong
git add . && git commit -m "msg" && git push

# ✅ Correct
rtk git add . && rtk git commit -m "msg" && rtk git push
```

## RTK Commands by Workflow

### Build & Compile (80-90% savings)
```bash
rtk cargo build         # Cargo build output
rtk cargo check         # Cargo check output
rtk cargo clippy        # Clippy warnings grouped by file (80%)
rtk tsc                 # TypeScript errors grouped by file/code (83%)
rtk lint                # ESLint/Biome violations grouped (84%)
rtk prettier --check    # Files needing format only (70%)
rtk next build          # Next.js build with route metrics (87%)
```

### Test (60-99% savings)
```bash
rtk cargo test          # Cargo test failures only (90%)
rtk go test             # Go test failures only (90%)
rtk jest                # Jest failures only (99.5%)
rtk vitest              # Vitest failures only (99.5%)
rtk playwright test     # Playwright failures only (94%)
rtk pytest              # Python test failures only (90%)
rtk rake test           # Ruby test failures only (90%)
rtk rspec               # RSpec test failures only (60%)
rtk test <cmd>          # Generic test wrapper - failures only
```

### Git (59-80% savings)
```bash
rtk git status          # Compact status
rtk git log             # Compact log (works with all git flags)
rtk git diff            # Compact diff (80%)
rtk git show            # Compact show (80%)
rtk git add             # Ultra-compact confirmations (59%)
rtk git commit          # Ultra-compact confirmations (59%)
rtk git push            # Ultra-compact confirmations
rtk git pull            # Ultra-compact confirmations
rtk git branch          # Compact branch list
rtk git fetch           # Compact fetch
rtk git stash           # Compact stash
rtk git worktree        # Compact worktree
```

Note: Git passthrough works for ALL subcommands, even those not explicitly listed.

### GitHub (26-87% savings)
```bash
rtk gh pr view <num>    # Compact PR view (87%)
rtk gh pr checks        # Compact PR checks (79%)
rtk gh run list         # Compact workflow runs (82%)
rtk gh issue list       # Compact issue list (80%)
rtk gh api              # Compact API responses (26%)
```

### JavaScript/TypeScript Tooling (70-90% savings)
```bash
rtk pnpm list           # Compact dependency tree (70%)
rtk pnpm outdated       # Compact outdated packages (80%)
rtk pnpm install        # Compact install output (90%)
rtk npm run <script>    # Compact npm script output
rtk npx <cmd>           # Compact npx command output
rtk prisma              # Prisma without ASCII art (88%)
```

### Files & Search (60-75% savings)
```bash
rtk ls <path>           # Tree format, compact (65%)
rtk read <file>         # Code reading with filtering (60%)
rtk grep <pattern>      # Search grouped by file (75%)
rtk find <pattern>      # Find grouped by directory (70%)
```

### Analysis & Debug (70-90% savings)
```bash
rtk err <cmd>           # Filter errors only from any command
rtk log <file>          # Deduplicated logs with counts
rtk json <file>         # JSON structure without values
rtk deps                # Dependency overview
rtk env                 # Environment variables compact
rtk summary <cmd>       # Smart summary of command output
rtk diff                # Ultra-compact diffs
```

### Infrastructure (85% savings)
```bash
rtk docker ps           # Compact container list
rtk docker images       # Compact image list
rtk docker logs <c>     # Deduplicated logs
rtk kubectl get         # Compact resource list
rtk kubectl logs        # Deduplicated pod logs
```

### Network (65-70% savings)
```bash
rtk curl <url>          # Compact HTTP responses (70%)
rtk wget <url>          # Compact download output (65%)
```

### Meta Commands
```bash
rtk gain                # View token savings statistics
rtk gain --history      # View command history with savings
rtk discover            # Analyze Claude Code sessions for missed RTK usage
rtk proxy <cmd>         # Run command without filtering (for debugging)
rtk init                # Add RTK instructions to CLAUDE.md
rtk init --global       # Add RTK to ~/.claude/CLAUDE.md
```

## Token Savings Overview

| Category | Commands | Typical Savings |
|----------|----------|-----------------|
| Tests | vitest, playwright, cargo test | 90-99% |
| Build | next, tsc, lint, prettier | 70-87% |
| Git | status, log, diff, add, commit | 59-80% |
| GitHub | gh pr, gh run, gh issue | 26-87% |
| Package Managers | pnpm, npm, npx | 70-90% |
| Files | ls, read, grep, find | 60-75% |
| Infrastructure | docker, kubectl | 85% |
| Network | curl, wget | 65-70% |

Overall average: **60-90% token reduction** on common development operations.
<!-- /rtk-instructions -->

## Ralph Managed Blocks

<!-- ralph:begin hang-prevention v=sha256:272439a4f9f9b6d5ebbf4b0edda64a2f4464396077c351e1b2e83d33e4a1ee7a -->
## Command Hang Prevention Rules

1. Never run infinite-follow commands directly.
   Forbidden examples:
   - tail -f
   - tail -F
   - journalctl -f
   - adb logcat
   - dmesg -w
   - watch
   - while true

2. If follow mode is necessary, always wrap it with timeout:
   - timeout 30s tail -f <file>
   - timeout 60s adb logcat
   - timeout 30s journalctl -f

3. Prefer bounded commands:
   - tail -n 200 <file>
   - grep -n "ERROR" <file> | head -100
   - journalctl -n 300 --no-pager
   - dmesg | tail -200

4. For large files, never cat the whole file.
   Use:
   - wc -l <file>
   - tail -n 200 <file>
   - head -n 100 <file>
   - grep -n "keyword" <file> | head -50

5. Every external command that may block must have timeout.

<!-- ralph:end hang-prevention -->
