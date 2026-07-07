# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> The orchestrator is a thin coordination layer, not a platform. Agents are smart; let them do the work.

> **保持精简**:详细架构、模块路径、多 hat 策略、可观测性、特性用法等已下沉到 `.cursor/rules/*.mdc`(按主题按文件 glob 按需加载)。本文件只保留 always-apply 的硬规则 + 高频命令。
>
> **知识库**: `docs/solutions/` — 已解决问题的可检索文档（按 category 组织,YAML frontmatter 含 `module` / `tags` / `problem_type`）;实现、调试 preset / adapter / hat 相关能力时可查阅。`CONCEPTS.md` — 共享域词汇表（hat、dimension-reviewer、scope_violation 等）。

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
- Builtin presets: `autoresearch`, `ce-executor-pipeline` (13-hat 线性一条龙: plan-reviewer → executor(每个 U-ID 一个 subagent,主 executor 负责验收/提交/最终 emit) → 6 串行维度 hat → review-synthesizer → fix-planner → fixer → alignment → reporter), `ce-executor-serial` (9-hat — 2026-07-06 plan U10 removed `progress-steward`; topology: TDD executor + validator + 6-dim overall review: goal-alignment → correctness → testing → maintainability → project-standards → adversarial), `ce-executor-supervisor` (2026-07-03-001 plan: 16-hat + progress-steward; rusqlite-backed supervisor wave orchestration, per-slot worktrees, fan-in merge, parallel review/fix; 需用 `--features supervisor-db` 编译且 `event_loop.supervisor.enabled: true`), `ce-executor-lite` (template), `debug`, `merge-batch` (Git-first 多 worktree 批量 merge: reviewer → integrator → stabilizer 自环 → reporter), `merge-loop`(内部单 loop 自动 merge;裸 `ce-executor` 已删除:所有 plan-driven 执行请使用 `ce-executor-serial`;仅作模板时可使用 `ce-executor-lite`)
- `presets/index.json` is the user-facing preset manifest
- **Operator skills（preset 起草/评审）:** `skills/ralph-preset-author`、`skills/ralph-preset-review`（共享 `skills/ralph-preset-common/references/`）；用户 hats 仍用 `skills/ralph-hats`

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
- **AI skill guide 同步规则(HARD RULE)**: `crates/ralph-core/data/*.md` 是注入给 AI agent 的 skills/指南(`ralph-tools.md` 每次注入,其余按需加载),必须随功能演进保持同步。**在做计划阶段**,若计划引入新命令、新工作流、新配置字段、新 runtime 能力或改变现有行为,必须同时在计划里识别是否需要新增或更新 `crates/ralph-core/data/*.md` 中的对应 skill 文档。**在完成代码实现后**,必须反向检查这些 skill 文档是否仍然准确,必要时立即追加/修正。**允许编辑的文件范围**: `ralph-tools.md`(共享命令入口)、`ralph-tools-tasks.md`(任务命令)、`ralph-tools-memories.md`(记忆命令)、`ralph-tools-cmdref.md`(命令速查)、`ralph-tools-emit.md`(事件/消息命令)、`ralph-tools-wave.md`(wave 相关命令),以及后续新增的 `ralph-tools-*.md`。`.claude/skills/ralph-tools/SKILL.md` 是 `ralph-tools.md` 的 symlink,同步基础文件即可。
- **AI skill guide 可读性规则(HARD RULE)**: `crates/ralph-core/data/*.md` 会进入 agent prompt,必须按"agent 下一步能执行什么"来写,不能按"runtime 内部如何实现"来写。每条规则必须说明:触发条件、agent 应执行的命令或动作、关键字段从哪里取得、失败时停止条件。所有术语和变量首次出现时必须解释清楚,包括但不限于 `hat`、`topic`、`task_key`、`step`、`task_id`、`kind`、`reason`、`allowed_topics`、`required_fields`、`policy-check`、`业务事件`、`终态事件`、`同一轮 hat 执行`。禁止在注入 skill 中泄漏或依赖 agent 不可见的实现细节,包括内部函数名/模块名(如 `check_*`、`*_guard`、`recovery_runtime::*`)、源码行号、内部 ledger 路径(如 `.ralph/events.jsonl` / `.ralph/supervisor.db` / `.ralph/agent/tasks.jsonl`)、reviewer-only 注释、一次性事故报告路径、过窄 preset 案例、以及 `fail-close` / `silent-success` / `retry budget` 等未解释专业术语。确需给维护者留实现背景时,放到非注入开发文档或代码注释,不要写进 `crates/ralph-core/data/*.md`。
- **何时必须更新 skill guide**: 包括但不限于——新增/删除/重命名 `ralph` CLI 子命令或参数、新增/修改 event 类型、新增/修改 task/memory 工作流、新增/修改预设/配置字段、新增/修改 wave/loop/inspect/doctor 等行为、新增/修改需要 agent 在 loop 中使用的工具或输出格式。一句话:**只要 agent 的 prompt 里可能用到、或 agent 应该知道的能力变了,就要同步更新 data 下的指南**。
- **反向验证(必须)**:修改 ralph tools 子命令、被这些 skill 文档引用的源码(行号、参数、行为描述)后,必须用 `sed -n 'NN,MMp' <file>` 复核 `crates/ralph-core/data/*.md` 里所有形如 `xxx.rs:NN-MM` 的源码引用范围是否仍指向正确代码。**行号漂移、参数表与代码 clap 定义不符、引用了不存在的命令/字段,都算违规**。改完必须跑一次 `ralph <cmd> --help`(涉及命令语法)或对应 skill 列出的全部命令做冒烟测试(涉及行为),并跑 `scripts/check-cli-doc-drift.sh` 做静态 drift 扫描。发现漂移立即在文档里同步修正,不允许文档落后于代码。
- **Preset operator skills 同步规则(HARD RULE)**: `skills/ralph-preset-author`、`skills/ralph-preset-review` 及共享 `skills/ralph-preset-common/references/` 是 **loop 外** preset 起草/AAF 评审的操作规程,必须随 Ralph 能力演进保持可审计。**在做计划阶段**,若变更影响 preset 作者或评审应知道的能力,必须在计划里写明是否同步这两套 skill。**在完成代码实现后**,必须反向检查并更新,否则 review 会对新违规假阴性、对旧假设假阳性。触发范围包括但不限于:新增/删除/重命名 `ralph preset` / `ralph hats` / `ralph emit` 子命令或参数;`ralph preset check` / `preset_lint` 新增或变更 `finding_id`、severity、strict 语义;`event_loop` / hat / `event_policy` / `state_projection` / isolated prompt 注入行为变更;新增 event topic、`required_fields`、OPAC/单事件预算/三字段约束;AAF 评审所依赖的 CLI 白名单或 hat 可见性模型变更。**允许编辑范围**:`skills/ralph-preset-common/references/{agent-native-model,author-checklist,commands,finding-rubric,patterns}.md`;`skills/ralph-preset-{author,review}/SKILL.md`(workflow/guardrails);`skills/ralph-preset-common/fixtures/`(验收场景)。**最低验收**:更新 `finding-rubric.md` 中受影响 `finding_id` 映射;`commands.md` 与 `ralph <cmd> --help` 一致;对 `skills/ralph-preset-common/fixtures/aaf-review-negative-fixture.yml` 或等价场景重跑 review 流程说明仍成立。
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
- **Worktree 复用规则(HARD RULE 3)**:任何使用 `--worktree` 的 `ralph run` 都必须显式指定复用键:**`--plan <plan.md>`** 或 **`--worktree-name <name>`**。Ralph 会按 plan 的 basename(去掉 `.md`/`.html` 后缀)或精确名称在 `.ralph/loops.json` 与 `git worktree list` 中查找已完成的 worktree 并自动复用;**严禁**不看参数就 `EnterWorktree` / `git worktree add` 创建新 worktree,也禁止靠 prompt 文本"猜测"plan 路径。旧版"从 prompt 文本自动提取 plan 路径做模糊匹配"的行为已废弃,因为它对中文、标点或附加说明极其脆弱。推荐写法:
  ```bash
  ralph run --worktree --reuse-worktree --plan docs/plans/2026-06-25-002-feat-profiles-for-preset-role-tuning-plan.md
  # 或精确复用指定 worktree
  ralph run --worktree --reuse-worktree --worktree-name 2026-06-25-002-feat-profiles-for-preset-role-tuning-plan-lucky-reed
  ```
- **Hat `instructions:` 必须用 hat 视角编写(HARD RULE 4)**:**preset 中每个 hat 的 `instructions:` 字段是写给「该 hat 在自己那一轮 activation 里的 agent」看的**,不是给 preset 作者看的,也不是给该 hat 能看到的其它 hat 看的。**`event_loop.execution_mode: isolated` 是 preset 的默认且唯一的执行模式**(2026-06-18 起全 preset 收敛到 isolated;coordinator 模式已弃用),在此模式下:
  1. **每个 activation 是隔离的进程级单元**:该 hat 看不到其它 hat 的进程、状态、history,只能通过 runtime 注入的环境变量(`RALPH_CURRENT_HAT` / `RALPH_CURRENT_LOOP_ID` / `RALPH_EVENTS_FILE` / `RALPH_HATS_SOURCE` 等)与 `ralph tools task <cmd>` / `ralph emit` / `ralph wave emit` 这些 **runtime API** 通信。
  2. **单业务事件预算**:`ralph emit` 在同一 activation 内只保留**第一个**业务事件,后续业务事件被 runtime 静默丢弃;**终态事件(`plan.complete` / `LOOP_COMPLETE` / `plan.blocked`)前面绝不要夹带其它业务事件**,否则终态事件会被丢弃。
  3. **专有名词必须讲「该 hat 上下文里的语义」**:不要在 hat instructions 里展开 `enforce_hat_scope` / `exempt_topics` / `origin guard` / `repair_budget` / `mechanism.flow` / `enforce_current_unit` / `ephemeral_isolation` 等**框架级实现细节**——这些是 preset 作者视角,agent 看不见也不需要理解。讲到具体 topic / 命令时,直接列它在本 hat 上下文里**能发什么 / 不能发什么 / 必填字段是什么**即可。
  4. **不要预设 hat 知道拓扑**:不要在 instructions 里说"worker hat 写代码、reviewer hat 评审"等**拓扑内相对位置**的描述——该 hat 看不到其它 hat,这些信息会误导它越权。**只**说"你的职责是 X、不要做 Y、做完发 Z"——X/Y/Z 都是该 hat 自己**直接可观测**或**直接可调用**的。
  5. **不要让 hat 直接读 `.ralph/events.jsonl` / `.ralph/supervisor.db` / `.ralph/loops.json`**:这些是 runtime / supervisor 的内部 ledger,hat 进程级不可见。**共享状态只能通过 `ralph tools task <cmd>`**(runtime task API,所有 hat 可见)。
  6. **违规示例(已发现并修复)**:2026-07-03 `ce-executor-supervisor` preset 的 `coordinator` hat 第一版 instructions 写"read `.ralph/events.jsonl` tail"——coordinator 看不到 events.jsonl,因为 events.jsonl 是 supervisor 内部 ledger,只有 supervisor / runtime 进程能写。正确写法是"用 `ralph tools task list -s done` 查 unit 状态"。
  7. **enforcement**:新加 / 改 hat `instructions:` 时,作者必须先问自己「该 hat 在自己 activation 里**能直接看到**什么?能**直接调用**什么?」——如果某条 instruction 要求 hat 去看 / 调它**看不到 / 调不到**的东西,就违反本规则,必须改写。**Lint**:目前 preset_lint 暂无 hat-instructions 视角的静态检查(U13 follow-up),改写时人工 review。
  8. **Hat `instructions:` 必须引用 `crates/ralph-core/data/*.md` 注入的 skill doc,不复述其内容**:`ralph-tools.md` 是 always-injected(`tasks.enabled` 或 `memories.enabled` 至少一个为 true 时),`ralph-tools-tasks.md` / `ralph-tools-memories.md` 按 enabled 条件 auto-injected,`ralph-tools-emit.md` / `ralph-tools-wave.md` / `ralph-tools-cmdref.md` / `ralph-tools-precheck.md` / `ralph-tools-recovery-directives.md` 按需 load(`ralph tools skill load <name>`)。Hat instructions 涉及命令语法 / 字段约束 / 反模式时,**引用**对应 skill 的章节(`ralph-tools-tasks` red box / `ralph-tools-wave` red box / `ralph-tools` §5 precheck / `ralph-tools` §6 isolated 单事件预算),**不要把内容复制**到 instructions 里——复制会产生**漂移**:skill doc 修订时 hat instructions 不会自动同步,违反 HARD RULE。
  9. **emitter hat 必须 `ralph emit/wave emit --policy-check` 强预检**:`--policy-check` 是 loop 的 precheck gate(同源 schema),**任何 `ralph emit` 或 `ralph wave emit --payloads-stdin` 之前必须先跑 `--policy-check`**,通过后再去掉 `--policy-check` 真正写盘(per `ralph-tools` §5)。Hat instructions 必须把这个 precheck 写为**强约束**,不能写成"可选"或"建议"。
  10. **emitter hat 涉及 `task_id` / `task_key` / `step` 三字段时必须引用同源约束**:`task_id` 是当前 loop 的真实 live id(`ralph tools task list` 取得),`task_key` 是注册时的稳定 key,`step` 值必须匹配 `task_key` 中 `:step-<n>:` 段(per `ralph-tools-tasks` red box)。**禁止**手写 `task_id`(避免 reuse closed task id)。Hat instructions 涉及三字段时必须引用这条规则,不要凭直觉构造 payload。

<!-- rtk-instructions v2 -->
# RTK (Rust Token Killer) - Token-Optimized Commands

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

# CLAUDE.md

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.

<!-- SEMBLE_START -->
## Semble Code Search

A `semble` MCP server is available with two tools:
- `mcp__semble__search` — search the codebase with a natural-language or code query.
- `mcp__semble__find_related` — find code similar to a specific file and line.

Use `mcp__semble__search` to find where something is implemented — instead of using Grep or Glob to discover files. After semble returns the file and line, navigate there directly and read that file. Do not grep for the same content again.

Pass `--content docs` to search documentation and prose, `--content config` for config files, or `--content all` to search code, docs, and config together.

For CLI fallback or sub-agents without MCP access, use:

```bash
semble search "authentication flow" ./my-project --max-snippet-lines 10
semble search "deployment guide" ./my-project --content docs
semble search "database host port" ./my-project --content config
semble find-related src/auth.py 42 ./my-project
semble search "save model to disk" ./my-project --top-k 10
```

The index is built on first run and cached automatically. If `semble` is not on `$PATH`, use `uvx --from "semble[mcp]" semble`.

### Workflow

1. Call `mcp__semble__search` with a query describing what the code does or its name. The tool returns results with 10 lines of context each (function/class signature + first body lines, enough to confirm the location).
2. Navigate directly to the top result's file and line. Read only the function or class at that location.
3. Make the edit. Do not re-search or grep for the same content.
4. Use `--content docs` for documentation, `--content config` for config files, or `--content all` for everything.
5. Optionally use `mcp__semble__find_related` with `file_path` and `line` to discover similar code elsewhere.
6. Use Grep only when you need every occurrence of a literal string across the whole repo (e.g., all callers of a renamed function).
<!-- SEMBLE_END -->
