---
title: "Pi headless skill 上下文预算 - Plan"
date: 2026-07-23
type: feat
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-brainstorm
execution: code
related:
  - docs/brainstorms/2026-07-23-small-context-model-orchestration-requirements.md
---

# Pi headless skill 上下文预算 - Plan

## Goal Capsule

- **Objective:** 让 `ralph run -b pi` 的 headless Pi 默认跳过用户全局 skill 索引，只加载 workspace 的 `.agents/skills`，同时保留全局 extensions、交互式 Pi 行为和用户参数追加能力。
- **Product authority:** Ralph 决定 headless backend 默认参数；Pi 继续负责按 CLI 参数发现 skills 与 extensions。
- **Execution profile:** 一个行为单元，先用现有 Pi backend 测试锁定新 argv 契约，再做最小实现。
- **Stop conditions:** R1–R5、F1、AE1–AE2 与 S1 满足；需求 B 不属于本计划的实现或完成条件。
- **Tail ownership:** 执行方负责目标测试与 workspace 全量门禁；不在计划文档记录执行进度。

## Product Contract

### Summary

本计划只交付需求 A：headless Pi 默认使用 `--no-skills --skill .agents/skills` 控制 skill 上下文预算。
需求 B 的分阶段 Pi extension 保留为后续独立工作，不在本次实现。

### Problem Frame

`ralph run -b pi` 当前会让 Pi 自动发现 `~/.pi/agent/skills` 与 `~/.agents/skills`，大量与当前 hat 无关的 skill name/description 会进入小上下文本地模型的 system prompt。
Ralph 只需调整 headless Pi 的默认 argv，无需改变编排拓扑、event 契约或交互式 Pi。

### Key Decisions

- KD1. 需求 A 独立交付；需求 B 延后，不共享实现边界。
- KD2. headless 默认增加 `--no-skills --skill .agents/skills`，不增加 `--no-extensions`，因此全局 extensions 仍可加载。
- KD6. `.agents/skills` 不存在时不在 Ralph 侧预检或硬失败；沿用 Pi 对缺失 CLI skill 路径记录诊断后继续执行的行为。

### Actors

- A1. 运营方：配置 `backend: pi`，并可继续通过 `cli.args` 或 hat `backend_args` 追加 Pi 参数。
- A2. Ralph loop runner：构造 headless Pi 默认 argv，并在 workspace cwd 中启动进程。
- A3. Pi（headless）：根据 argv 加载项目 skills 与仍启用的 extensions。

### Requirements

- R1. 当 Ralph 以 headless Pi backend 启动时，默认 CLI args 必须包含 `--no-skills` 与 `--skill .agents/skills`；相对路径按进程 cwd（workspace）解析。
- R2. 默认不得附加 `--no-extensions`；全局 `pi install` packages 与显式 `-e` 仍可加载。
- R3. `ralph plan` 等使用的交互式 `pi_interactive` 路径不得被迫套用 R1。
- R4. workspace 不存在 `.agents/skills` 时，Ralph 不得自行拒绝启动；Pi 可使用空 skill 索引继续运行。
- R5. 用户仍可通过 `cli.args` 或 hat `backend_args` 追加 Pi 参数；本次不提供恢复全局 skills 的专用开关。

### Key Flow

- F1. Ralph headless Pi 启动
  - **Trigger:** `ralph run` 或等价入口解析到 `cli.backend: pi` / `-b pi`。
  - **Actors:** A2, A3
  - **Steps:** `CliBackend::pi()` 构造默认参数；配置层在默认参数之后追加用户参数；命令构造层最后追加 prompt；Pi 跳过全局 skill 自动发现并加载 `.agents/skills`。
  - **Outcome:** 默认 system prompt 的 skill 索引不再包含用户全局 skills；extensions 仍启用。
  - **Covered by:** R1–R5

### Acceptance Examples

- AE1. headless 默认参数与 extensions
  - **Given:** workspace 有 `.agents/skills`，用户全局存在大量 skills，并已安装全局 extensions。
  - **When:** Ralph 构造默认 headless Pi 命令。
  - **Then:** argv 包含 `--no-skills --skill .agents/skills`，不包含 `--no-extensions`；用户追加参数位于默认参数之后。

- AE2. 缺失项目 skill 目录
  - **Given:** workspace 不存在 `.agents/skills`。
  - **When:** Ralph 仍以同一组默认参数启动 Pi。
  - **Then:** Ralph 不做目录存在性拦截；Pi 可按其既有缺失路径语义继续运行。

### Success Criteria

- S1. headless Pi 的默认 argv 确定性地关闭全局 skill 自动发现并指定 `.agents/skills`，同时不关闭 extensions；交互式 Pi argv 保持原样。

### Scope Boundaries

**In scope**

- `CliBackend::pi()` 的 headless 默认参数。
- 默认参数、用户追加参数、prompt 的顺序回归测试。
- `pi_interactive` 不受影响的回归断言。

**Out of scope / non-goals**

- 不新增配置字段或“恢复全局 skills”开关。
- 不改变 event loop、hat 拓扑、emit schema、isolated 语义或 Pi 输出解析。
- 不改写 `crates/ralph-core/data/ralph-tools*.md`；本次没有改变 agent 可调用命令、工作流或输出格式。

### Deferred to Follow-Up Work

- 需求 B 整体延后：KD3–KD5、R6–R14、F2、AE3–AE5，以及分阶段 Pi extension 的检测、解析、阶段推进、确定性完成信号和终态 emit 约束。

### Dependencies / Assumptions

- Pi 继续支持 `--no-skills` 与可重复使用的 `--skill <path>` 参数。
- backend 进程 cwd 继续指向 Ralph workspace，因此 `.agents/skills` 保持项目相对语义。
- Pi 对缺失 `--skill` 路径保持已确认的非致命行为；Ralph 不复制该外部工具的目录校验。

## Planning Contract

### Product Contract Preservation

Product Contract changed: KD3–KD5、R6–R14、F2、AE3–AE5 按用户指令移入后续工作；需求 A 的 R1–R5、F1、AE1–AE2 与 S1 语义不变。

### Key Technical Decisions

- KTD1. 只修改 `CliBackend::pi()` 的默认 `args`，不改 `pi_interactive()` 或交互式路由，直接形成 headless/interactive 隔离。
- KTD2. 沿用现有参数合并链：headless defaults → `cli.args` / hat `backend_args` → prompt；不归一化、不删除用户追加的 Pi 参数。
- KTD3. 将 `.agents/skills` 保持为字面相对路径，不在 adapter 中解析成绝对路径或检查目录存在性。
- KTD4. 用 `CliBackend::build_command` 的结构化 argv 测试作为稳定门禁；不把依赖本机 Pi 或模型响应的 PTY 集成测试设为 CI 必需条件。

### Sequencing

先更新现有 Pi backend 测试并确认旧实现因缺少新参数而失败，再只修改 `CliBackend::pi()` 默认参数，最后运行目标测试与全量门禁。

## Implementation Units

### U1. 收紧 headless Pi 默认 skill 发现

- **Goal:** 在不影响 extensions、交互式 Pi 与用户追加参数的前提下，为 headless Pi 固定项目级 skill 预算。
- **Requirements:** R1–R5、F1、AE1–AE2、S1
- **Dependencies:** 无
- **Files:**
  - Modify/Test: `crates/ralph-adapters/src/cli_backend.rs`
- **Approach:** 在 `CliBackend::pi()` 现有 `-p --mode json --no-session` 后加入 `--no-skills --skill .agents/skills`；保留 `from_config`、`from_name_with_args` 和 `build_command` 的追加逻辑。
- **Execution note:** 先强化现有测试并观察旧实现因 argv 缺项而失败，再做单点实现；不触碰需求 B。
- **Patterns to follow:** 同文件 `pi()` / `pi_interactive()` 工厂分离模式，以及 `from_config` / `from_name_with_args` 的 `extend` 追加模式。
- **Test scenarios:**
  1. Covers F1 / AE1. `CliBackend::pi().build_command("test prompt", false)` 产出 `pi -p --mode json --no-session --no-skills --skill .agents/skills "test prompt"` 的精确顺序。
  2. Covers R2 / AE1. headless 默认 argv 不含 `--no-extensions`。
  3. Covers R3 / S1. `pi_interactive()` 与 `for_interactive_prompt("pi")` 仍只使用 `--no-session` 加 positional prompt。
  4. Covers R5. `CliConfig.args` 中的 provider/model 参数位于 headless defaults 之后、prompt 之前，且没有被改写或丢弃。
  5. Covers R5. hat `NamedWithArgs` 的 provider/model 参数位于 headless defaults 之后、prompt 之前。
  6. Covers R4 / AE2. adapter 不检查 `.agents/skills` 是否存在；构造命令在任意 cwd 下都保持同一 argv，不因本地目录状态返回错误。
- **Verification:** 目标 Pi backend 测试通过；diff 仅包含默认参数与对应测试断言；全量 workspace 门禁无回归。

## Verification Contract

| Gate | Scope | Done signal |
|---|---|---|
| Pi adapter 目标测试 | `cargo nextest run -p ralph-adapters -- pi` | headless、interactive、config 与 hat 参数场景全部通过 |
| Rust 格式检查 | `cargo fmt --all -- --check` | 无格式差异 |
| Workspace 基线 | `./scripts/run-tests.sh` | nextest 与 doctest 全部通过；若仅出现时序 flake，按仓库规则使用串行兜底确认 |

已安装 Pi 时可补充非 CI smoke：在缺少 `.agents/skills` 的临时 cwd 启动同等 headless 命令并确认 Pi 不因该路径非零退出；这验证外部 Pi 假设，但不替代 Rust argv 契约测试。

## Definition of Done

- `CliBackend::pi()` 默认参数包含 `--no-skills --skill .agents/skills`，且不包含 `--no-extensions`。
- `pi_interactive()` 与交互式路由的 argv 保持不变。
- `cli.args` 与 hat `backend_args` 继续在默认参数之后、prompt 之前追加。
- Ralph 不增加 `.agents/skills` 目录预检或硬失败逻辑。
- U1 的测试场景与 Verification Contract 门禁通过。
- diff 不包含需求 B 的 extension、状态机、prompt 分段或其它越界实现，也不保留废弃尝试代码。
