# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> The orchestrator is a thin coordination layer, not a platform. Agents are smart; let them do the work.

> **保持精简**:详细架构、模块路径、多 hat 策略、可观测性、特性用法等已下沉到 `.cursor/rules/*.mdc`(按主题按文件 glob 按需加载)。本文件只保留 always-apply 的硬规则 + 高频命令。

## Build & Test

> **⚠️ HARD RULE 1: 测试入口必须用 `cargo nextest run` 系列**(`./scripts/run-tests.sh` / `just test-parallel` / `cargo nextest run -p <pkg> --bin <bin> -- <subset>`)。**禁止**裸跑 `cargo test -p ralph-cli` 或 `cargo test -p ralph-cli --bin ralph`——根因是 `crates/ralph-cli/src/loop_runner/tests.rs:14-49` 的 4 个 process-global Mutex + 时间敏感测试(`std::thread::sleep(500ms)`)。Nextest 的 process-per-test 隔离 Mutex 是第一道保险。允许的例外:① `cargo test --doc` 跑 doctest;② nextest 不可用时的最后兜底 `cargo test --workspace --exclude ralph-e2e -- --test-threads=1`;③ `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 强制 flake 兜底（走单线程,仅用于竞态/时序 flake 恢复,不是默认路径）;④ `crates/ralph-core/data/ralph-tools*.md` 这类**仅文档用途**的 cargo test 引用。详见 `docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md`。
>
> **⚠️ HARD RULE 2: 默认走并发,确需串行时显式配置**。`ralph-cli` 整个包走 cli-serial 串行(`.config/nextest.toml:17-18, 20-22`,根因:Mutex + sleep CPU 抢占,2026-06-13 已验证不能放开);其他 6 个包(`ralph-proto` / `ralph-core` / `ralph-adapters` / `ralph-tui` / `ralph-api` / `ralph-bench` / `ralph-e2e`)走 nextest 默认并发(`test-threads = num-cpus`)。**两件事必须同时改**(sleep 改为事件驱动等待 + 4 个静态量改 per-test 隔离)才能放开 ralph-cli 并发,不在本轮范围。

### 并行 vs 串行分级速查表

| 范围 | 并行/串行 | 触发命令 |
|---|---|---|
| `ralph-cli` 全包 | **串行** | `cargo nextest run -p ralph-cli --bin ralph -- <substring>` |
| 其他 6 个包 | **并行** | `cargo nextest run -p <pkg> -- <substring>` |
| 单包内单测 | 走包级规则 | 同上 |
| Doctest | 单独跑 | `cargo test --workspace --exclude ralph-e2e --doc` |
| E2E | 单进程 CLI | `cargo run -p ralph-e2e -- --mock` |
| Smoke/replay | 顺序跑 | `cargo nextest run -p ralph-core --features recording --test smoke_runner` |
| BDD scenarios | 顺序跑 | `cargo nextest run -p ralph-core --test scenarios` |

详细分级 + 根因参见 `.cursor/rules/architecture-modules.mdc`("Code Locations" + 顶部 `ralph-cli` 串行配置)。

```bash
# 全 workspace 并行(ralph-cli 串行,其他 6 包并行)——CI 推荐入口
./scripts/run-tests.sh

# 子集(全部走 nextest,继承包级规则)
cargo nextest run -p ralph-cli --bin ralph -- <substring>          # 串行
cargo nextest run -p ralph-core -- <substring>                     # 并行
cargo nextest run -p ralph-api --test <integration_test_name>      # 并行
cargo nextest run -p ralph-core --test scenarios                   # BDD
cargo nextest run -p ralph-core --features recording --test smoke_runner  # Smoke
cargo run -p ralph-e2e -- --mock                                  # E2E

# Last-resort fallback ONLY when nextest is unavailable.
cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output
just test-serial                                                   # 单线程 slow path

# Other build, lint and test commands.
cargo build
cargo clippy                                 # Lint (pedantic configured in workspace)
cargo fmt                                    # Format
cargo doc --no-deps                          # Documentation
./scripts/setup-hooks.sh                     # Install pre-commit hooks (once)
```

**IMPORTANT**: Run `cargo nextest run` (or `./scripts/run-tests.sh` if nextest is installed) before declaring any task done.

### 开发基线 vs CI 基线（处理测试 flake）

- **子任务 / 开发中验证**：只跑 targeted tests（`cargo nextest run -p <crate> -- <test>`）。
- **最终验证 / 准备 `LOOP_COMPLETE` 前**：再跑完整 `./scripts/run-tests.sh`（nextest + doctest）。
- **如果全量基线出现竞态/时序类 flake**:强制走单线程兜底 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh`(跳过 nextest,执行 `cargo test --workspace --exclude ralph-e2e -- --test-threads=1`)。**仅作为 flake 兜底,不是默认路径**。
- 如果 serial fallback 仍然失败,说明是真失败,必须修复后才能继续。

### Web Dashboard

```bash
ralph web                 # Launch both servers (backend:3000, frontend:5173)
npm install               # Install all dependencies
npm run dev               # Dev mode (both)
npm run dev:server        # Backend only
npm run dev:web           # Frontend only
npm run test:server       # Backend tests
npm run test              # All npm workspace tests
```

BDD scenarios (YAML, exercise real runtime paths) live in `crates/ralph-core/tests/scenarios/`.

## Architecture (High Level)

> **详细模块路径 / 代码位置表**:`.cursor/rules/architecture-modules.mdc`(自动按 `**/*.rs` glob 加载)
> **多 hat 隔离 / Agent Output Governance (R1-R6) / preset 单一事实源**:`.cursor/rules/multi-hat-isolation.mdc`
> **可观测性(Runtime Diagnosis U0-U8 + Doctor) / 诊断**:`.cursor/rules/observability.mdc`
> **Parallel Loops / Waves / Smoke / E2E / Specs & Tasks**:`.cursor/rules/feature-flags.mdc`

### Crate Map (Top-Level)

```
ralph-proto → Foundation types: Event, Hat, HatId, Topic, EventBus
ralph-core → Orchestration logic, event loop, state machine, hats, memories, tasks, hooks, skills
ralph-adapters → Backend adapters (Claude, Kiro, Gemini, Codex, Amp, Copilot CLI, OpenCode)
ralph-cli → CLI entry point, commands (run, plan, task, loops, web, mcp, wave)
ralph-tui → Terminal UI (ratatui-based)
ralph-e2e → End-to-end test framework
ralph-api → Rust RPC API server for web dashboard backend
ralph-bench → Benchmarking
```

### Event System Architecture

```
JSONL (agent output) → EventReader → EventParser → EventOriginGuard → EventPolicy → StateMachine → EventBus → Hats
```

### Backend Adapter Architecture

```
CLI Config → HaBackend → auto_detect → CliExecutor / PtyExecutor → StreamHandler
```

### Configuration System

```
User YAML → RalphConfig → EventLoopConfig → HatConfig overrides → effective runtime config
```

### Multi-Hat Isolation Policy（强制）

- **3-hat 上限（coordinator 模式）**：`hats` 数 ≤ 3 时,preset 可显式 `execution_mode: coordinator`(默认)或 `execution_mode: isolated`,任一可启动。
- **4+ hats 必须显式 isolated**：`hats` 数 ≥ 4 时,`event_loop.execution_mode: isolated` 是**强制**配置;缺少该字段、值不是 `isolated`、或被注释掉,preset 启动即被 `preset_lint::check_multi_hat_isolation` 拒绝。
- **错误消息固定**:`preset declares N hats which exceeds the coordinator limit of 3; set \`event_loop.execution_mode: isolated\` to run this preset`,调用方按字面匹配即可定位根因。
- **无豁免**:环境变量(`RALPH_ALLOW_COORDINATOR_OVERRIDE` 等)、测试开关、preset 名称维护的 exemption 均**不可用**;所有 builtin 4+ hat preset 均已迁移到 isolated(见 U6 commit `2a29e24`)。

完整 Isolated 终态 Authority(U3)/ Fair Scheduling(U4)/ Agent Output Governance(R1-R6)/ preset SSoT 多点同步规则 → `.cursor/rules/multi-hat-isolation.mdc`。

### Presets & Hats System

Presets define collections of hats. Located in `presets/` directory and `crates/ralph-cli/src/presets.rs` (~1100 lines):
- **HatlessRalph** (`hatless_ralph.rs`): Hat topology, event subscription matching, hat selection algorithm
- **HatRegistry**: Manages hat discovery, registration, subscription
- Presets support Chinese (`*-zh.yml`) variants and chainable configurations
- Builtin presets: `autoresearch`, `ce-executor-serial` (10-hat: TDD executor + validator + 总体 review), `ce-executor-lite` (template), `debug`, `merge-loop`(裸 `ce-executor` 已删除:所有 plan-driven 执行请使用 `ce-executor-serial`;仅作模板时可使用 `ce-executor-lite`)
- `presets/index.json` is the user-facing preset manifest

**`presets/manifest.yml` 是 builtin preset 的 single source of truth**(`crates/ralph-cli/build.rs` 和 `crates/ralph-cli/src/presets.rs` 都从这里读取并在不一致时 panic)。新增/重命名/删除一个 builtin preset 必须**同步改 4 处**:
1. `presets/en/<name>.yml`(实际 YAML)
2. `presets/manifest.yml` 的 `embedded:` 列表
3. `crates/ralph-cli/src/presets.rs` 的 `PRESETS` 数组(`EmbeddedPreset { name, description, content, public }`)
4. `presets/index.json`(如对用户可见)
5. 同步更新本文件 Presets & Hats 段的 builtin preset 列表,以及 `scripts/ralph-zsh-plugin.zsh` 的 zsh 补全

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

## IMPORTANT (Hard Rules — read before writing any code)

> **以下规则优先级最高,请在动手写任何代码前先完整读完本段。** 任何「先看了某段就开始写」的冲动都应当先回头对照本段。

- 讨论 ralph-orchestrator 的任何功能、架构、行为时,必须先去读源码确认,不允许凭记忆或猜测讨论
- Run `cargo nextest run`(或 `./scripts/run-tests.sh`)before declaring any task done——绝对不要用裸 `cargo test` 跑 `ralph-cli` 测试,会触发 loop_runner 的 process-global Mutex 中毒 flake(参见本文档「Build & Test」段 HARD RULE 1)。**默认走并发**(ralph-cli 除外,见分级表)
- Backwards compatibility doesn't matter — it adds clutter for no reason
- Prefer replay-based smoke tests over live API calls for CI
- BDD/Cucumber tests MUST exercise real runtime code paths via integration tests (not placeholder/source-only assertions)
- Run python tests using a .venv
- You MUST not commit ephemeral files
- When I ask you to view something that means to use playwright/chrome tools to go view it.
- When adding or changing `ralph tools` subcommands, update the appropriate file in `crates/ralph-core/data/`: `ralph-tools.md` (shared commands), `ralph-tools-tasks.md` (task commands), or `ralph-tools-memories.md` (memory commands). `.claude/skills/ralph-tools/SKILL.md` is a symlink to the base `ralph-tools.md`
- **反向验证(必须)**:修改 ralph tools 子命令、被这些 skill 文档引用的源码(行号、参数、行为描述)后,必须用 `sed -n 'NN,MMp' <file>` 复核 `crates/ralph-core/data/*.md` 里所有形如 `xxx.rs:NN-MM` 的源码引用范围是否仍指向正确代码。**行号漂移、参数表与代码 clap 定义不符、引用了不存在的命令/字段,都算违规**。改完必须跑一次 `ralph <cmd> --help`(涉及命令语法)或对应 skill 列出的全部命令做冒烟测试(涉及行为)。发现漂移立即在文档里同步修正,不允许文档落后于代码。
- When adding, removing, renaming, or changing builtin hat collections/presets in `crates/ralph-cli/src/presets.rs` or mirrored preset files, update `scripts/ralph-zsh-plugin.zsh` so `ralph run -H builtin:<TAB>` stays accurate. Preserve the current `compadd`-based completion style for values containing `:`; do not use `_describe` for `builtin:*` values. After updating the script, install it for the current user with `cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh` and verify zsh completion loads.
- **builtin preset 改动后**:除上述 zsh 脚本外,还必须同步更新本文件「Presets & Hats System」段的 builtin preset 列表(参见该段对 `presets/manifest.yml` 单一事实源的说明)。文档落后于代码视为违规。
- **preset yml 改动后必须同步 schema 并跑校验(HARD RULE)**:修改 `presets/en/<name>.yml` 后,必须检查 `presets/schemas/<name>.yml`(SSOT)是否需要同步修改——event schema 字段增删改、`required_fields` 变更、`execution_contracts` 规则调整、`topic_deny_rules` 变更等都要同步。改完必须跑 yml 校验:`cargo nextest run -p ralph-cli --bin ralph -- preset_lint` + `cargo nextest run -p ralph-core -- preset_lint` + `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`(SSOT byte-equality)。schema 与 preset 不一致或漏跑校验视为违规。
- **preset/schema 改动后的下游同步清单(HARD RULE)**:改 `presets/schemas/<name>.yml` 或 `presets/en/<name>.yml` 的 event 拓扑(终态事件增删、hat triggers/publishes、required_fields、topic_deny_rules)后,必须逐层检查并同步以下下游,任一漏改即违规:
  1. **runtime 事件循环**:`crates/ralph-core/src/event_loop/mod.rs` 的 step-close 分支(`fix.applied` 等)与 `inject_completion_correction`——终态语义改了这里必须同步,否则 runtime 仍按旧契约拦截(2026-06-24 P0-1 根因:drift detector 已删除,因 `review.passed` 已从新架构移除)
  2. **preset_lint 静态检查**:`crates/ralph-core/src/preset_lint/`(`schema_parity.rs`、`workflow_activation.rs`、`ownership.rs`、`multi_hat.rs`、`topic_format.rs`、`state_projection.rs`)——删除某条 lint 规则时,必须同步删除 `mod.rs` 的 `mod` 声明 + `pub use` + `finding_id.rs` 的 finding 常量注释引用
  3. **BDD 场景**:`crates/ralph-core/tests/scenarios/*.yml`(mock_responses payload 字段 + `expected.events` 列表)+ `crates/ralph-core/tests/scenarios.rs` 测试函数。**必须用 `run_workflow_guard_scenario`(真 EventLoop runner,断言 events),禁止用 `run_scenario` stub**(stub 只查 iterations 数,不断言事件,会静默吞掉拓扑失配——2026-06-24 P0-2/P0-3 根因)
  4. **config 字段**:`crates/ralph-core/src/config/loop_config.rs`(event_loop 配置字段增删)+ `crates/ralph-cli/src/preflight.rs`(`PRESET_OPT_IN_WHEN_OPERATOR_OMITS` 列表)+ `crates/ralph-cli/src/config_resolution.rs`(`PRESET_OPT_IN_KEYS` strip 列表)+ `append_runtime_config_block` 签名与注释(字段删除后注释里的旧字段名也要清掉)
  5. **CLI presets**:`crates/ralph-cli/src/presets.rs` 的 `PRESETS` 数组 + 静态断言测试
  6. **manifest / 索引**:`presets/manifest.yml` + `presets/index.json`(增删重命名 preset 时)
  7. **文档**:`CLAUDE.md` / `AGENTS.md`(同步 `cp`)+ `.cursor/rules/multi-hat-isolation.mdc` + `scripts/ralph-zsh-plugin.zsh`
  改完跑全量校验:`./scripts/run-tests.sh`(含 preset_lint + WAC + scenarios + SSOT byte-equality)。
- Design docs and specs go in `.ralph/specs` and one-off code tasks and bug fixes go in `.ralph/tasks`
- **`DEVELOPMENT.md` 已弃用**:它描述的是旧 `specs/` 目录规范,已被 `.ralph/specs/` 取代;请遵循本文件「Specs & Tasks」段的规范。
- **不要手动编辑 `.ralph/` 下的运行时状态文件**(`loop.lock` / `events.jsonl` / `agent/memories.md` / `agent/tasks.jsonl` / `loops.json` / `merge-queue.jsonl` / `diagnostics/`)。这些由 loop 自己维护;手工改动会与 in-flight 状态错位。确实需要重置时,先停掉所有相关 loop 再用对应 CLI(如 `ralph loops clean`)清理。
- **所有中文输出规则**:无论使用哪个 skill 进行操作,所有面向人类的输出——包括但不限于计划文档、设计文档、需求文档、实施计划、任务文件、报告、总结、注释说明、代码 review 意见、PR 描述等——都必须使用中文撰写。不影响:文件名、代码中的字符串字面量、代码注释中的技术标识符(如变量名、函数名、crate 名)、命令行输出块。这条规则优先于任何 skill 内置的语言默认值。
- **CLAUDE.md 与 AGENTS.md 同步规则**:这两个文件必须保持内容完全一致。修改其中一个时,必须同步更新另一个(推荐 `cp CLAUDE.md AGENTS.md`),确保不会出现差异。
- **测试入口强制 nextest(HARD RULE 1 + 2)**:
  - **HARD RULE 1**:本项目所有测试入口必须是 `cargo nextest run` 系列(`./scripts/run-tests.sh` / `just test-parallel` / `cargo nextest run -p <pkg> --bin <bin> -- <subset>`),**禁止**裸跑 `cargo test -p ralph-cli` 或 `cargo test -p ralph-cli --bin ralph`。根因是 `crates/ralph-cli/src/loop_runner/tests.rs:14-49` 的 4 个 process-global Mutex + 时间敏感测试。允许的例外:① `cargo test --doc` 跑 doctest;② nextest 不可用时的最后兜底 `cargo test --workspace --exclude ralph-e2e -- --test-threads=1`;③ `crates/ralph-core/data/ralph-tools*.md` 这类**仅文档用途**的 cargo test 引用。
  - **HARD RULE 2**:默认走并发,确需串行时显式配置。**能用并行的必须用并行**(快是默认值,不是可选优化)。具体分级见「Build & Test」段「并行 vs 串行分级速查表」——`ralph-cli` 整个包走 cli-serial 串行(根因:Mutex + sleep CPU 抢占,2026-06-13 已验证不能放开);其他 6 个包全部走 nextest 默认并发(`test-threads = num-cpus`)。
  - **修改任一规则须先确认所有 IDE/hook/CI 入口已切到 nextest**,并跑 3+1 验证(ralph-cli 子集 3 跑 + 全 workspace 1 跑)。
- **Worktree 复用规则(HARD RULE 3)**:执行任何 dev plan 前,**先跑 `git worktree list` 检查是否已有与该 plan 同名的 worktree**——命名约定:`<plan-basename>-<adjective-noun>`(plan 文件去掉 `.md` 后缀 + 两个随机英文单词,例如 `2026-06-25-001-feat-ce-executor-serial-5dim-coordinator-amendments-plan-lucky-reed`)。已有则直接 `cd` 进去复用,**禁止盲目新开**。只有确认无匹配 worktree 时,才按上述约定创建新 worktree(避免重名/重复消耗磁盘,也避免与已 in-flight 的 loop 状态错位)。

<!-- rtk-instructions v2 -->
# RTK (Rust Token Killer) - Token-Optimized Commands

> **完整 RTK 命令手册已迁移到 `.cursor/rules/rtk-token-killer.mdc`**(按 `**/*.sh` glob 按需加载)。本节只保留「Golden Rule」。

## Golden Rule

**Always prefix commands with `rtk`**. If RTK has a dedicated filter, it uses it. If not, it passes through unchanged. Always safe to use.

```bash
# Wrong
git add . && git commit -m "msg" && git push

# Correct
rtk git add . && rtk git commit -m "msg" && rtk git push
```

常用高频: `rtk cargo test` / `rtk cargo clippy` / `rtk cargo build` / `rtk git status` / `rtk git diff`。完整 30+ 命令表见 `.cursor/rules/rtk-token-killer.mdc`。
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

6. Background processes must be bounded. `cmd &` without a kill switch leaks
   when the parent shell exits abnormally (Ctrl-C, timeout, panic, parent
   kill). On 2026-06-20, fourteen `yes > /dev/null` processes survived a
   debug session and drove the box to 100% CPU for hours; `pkill -f
   "yes > /dev/null"` did not match because `>` was interpreted as a shell
   redirect, leaving the pattern as a literal substring that none of the
   processes actually carried in their argv.
   - For CPU/IO stress during debugging, use `timeout` to bound the
     lifetime: `timeout 30s yes > /dev/null`. The `timeout` binary
     reaps the child whether you Ctrl-C the parent or the deadline fires.
   - To clean up leaked `yes` (or any process whose argv you cannot
     predict), use `pkill -9 -x yes`. The `-x` flag matches the process
     name exactly, bypassing the regex / substring trap.
   - When wrapping a script that itself spawns N workers, give the
     script a `trap 'pkill -9 -x yes 2>/dev/null' EXIT INT TERM` and a
     `timeout` outer wrapper, so every exit path (clean, signal,
     parent-kill) reaps the children.
   - Treat any backgrounded CPU/disk/network loop (`yes`, `stress-ng`,
     `dd if=/dev/zero`, `nc -l`, `redis-server --daemonize yes`,
     `python3 -m http.server`) as a leak risk by default.
<!-- ralph:end hang-prevention -->

OUTPUT STYLE: concise
- Bullet points over paragraphs
- Skip filler words and hedging ("I think", "probably", "it seems")
- 1-sentence explanations max, then code/action
- No repeating what the user said
