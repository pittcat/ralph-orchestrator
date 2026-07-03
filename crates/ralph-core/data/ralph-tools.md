---
name: ralph-tools
description: Core CLI reference and rules for Ralph orchestration agents
metadata:
  internal: true
---

# Ralph CLI 核心参考

> **前提**：本 skill 仅在 `memories.enabled` 或 `tasks.enabled` 至少一个启用时被注入。速查表中的"已注入"列均受此条件约束。

> **遇到不确定的命令语法时，先 `ralph <cmd> --help` 再执行。**

## Runner 注入的环境变量

当 `ralph run` 启动一个 hat 时，子进程会自动获得以下环境变量：

| 变量 | 含义 | 何时为空 |
|------|------|----------|
| `RALPH_CURRENT_HAT` | 当前激活的 hat id，例如 `coordinator`、`executor` | 非 loop 场景下直接调用 `ralph emit` 时 |
| `RALPH_CURRENT_LOOP_ID` | 当前 loop id | 同上 |
| `RALPH_EVENTS_FILE` | 当前 loop 的事件文件路径 | 同上 |
| `RALPH_TRIGGERED_HAT` | `ralph emit` 时默认的 `triggered` 回退值 | isolated 模式下 runner 不再注入；多消费者/无明确下游 topic 时也可能为空 |
| `RALPH_HATS_SOURCE` | hats 来源标签，例如 `builtin:ce-executor-serial` | 无预设时 |

这些变量由 runner 在每次 hat activation 前注入，agent 可直接读取，但**不要**假设某个变量一定非空。

## AI 决策速查：我现在该做什么？

| 场景 | 下一步 | 参考 |
|------|--------|------|
| Loop 拒收事件 / 收到 `task.resume` | 读下方「收到 `task.resume` 时」段；用 `ralph emit --schema <TOPIC>` 预检字段 | 本 skill |
| 要发射单个事件或预检 schema | `ralph emit <TOPIC> --policy-check -j '{...}'` | `ralph tools skill load ralph-tools-emit` |
| loop 里出现 `.proposed` / `.rejected` / `precheck-*` gate | 读 topic 表，勿手 emit 衍生 topic | `ralph tools skill load ralph-tools-precheck` |
| 要派发 review wave / 作为 wave worker 返回 | `ralph wave emit <TOPIC> --payloads-stdin` | `ralph tools skill load ralph-tools-wave` |
| 要管理 runtime task | `ralph tools task add/ensure/start/close` | `ralph-tools-tasks`（已注入，若 tasks.enabled） |
| 要记录/查找记忆或 decision journal | `ralph tools memory add/search/prime` | `ralph-tools-memories`（已注入，若 memories.enabled） |
| 要启动 loop / 复用 worktree / 预览 profiles | `ralph run ...` / `ralph inspect profiles` | `ralph tools skill load ralph-tools-cmdref` |
| 要校验 hat 拓扑 | `ralph hats validate [--strict]` | `crates/ralph-cli/src/hats.rs:170` |
| Loop 崩溃/ledger 损坏需恢复 | `ralph loops clean --ledger` + `ralph diagnose --session latest` | `docs/guide/runtime-diagnosis.md` |

**新增 flag 一句话说明**：
- `ralph run --no-default-profiles`：跳过 `ralph.yml` 中的 `profiles.default`，仅保留 CLI `--profile`。
- `ralph run --no-sync-agent-docs`：跳过启动前对 `CLAUDE.md` / `AGENTS.md` 的 managed block 同步。
- `ralph inspect profiles [--no-default-profiles]`：只读预览最终生效的 profile overlay 解析结果。
- `ralph emit --schema <TOPIC>`：只读打印 `<TOPIC>` 的 embedded 协议 + `protocol_hash`，用于 drift 检测。
- `ralph loops clean --ledger`：截断损坏的 `StateLedger` 文件，崩溃后自动重建时会降级到 cold start。

## 核心规则

1. **绝不用 echo/cat 写 tasks 或 memories** — 必须用 CLI 工具
2. **emit 后必须校验** — 确认事件已写入事件文件
3. **task/memory 操作后必须确认状态** — 用 `--format json` + `jq` 验证
4. **失败时先查 `--help`** — 不要猜测参数，文档可能已更新
5. **emit step handoff 事件前，先用 schema 预检** — `ralph emit --schema <TOPIC>` 会列出 `required_fields`；payload 必须包含全部 required fields，且字段之间不自相矛盾（例如 `step` 与 `task_key` 中的 step 段必须一致）。不要凭记忆构造 payload。
6. **isolated 模式:一个 activation 只发 1 个业务事件** —— 运行时只保留你这一回合最先发出的那个,其后的全部静默丢弃(不分 topic)。发完即停,**终态事件(如 `plan.complete`)前面绝不要夹带 `work.ready` 等其它 emit**,否则终态事件会被丢弃。细则见 `ralph emit` 深参考「isolated mode 单业务事件 / 重发规则」。
7. **Worktree 复用规则（显式参数,严禁盲目创建）**:使用 `--worktree --reuse-worktree` 时必须同时给出 `--plan <plan.md>` 或 `--worktree-name <name>`。Ralph 会按 plan 的 basename 或精确名称查找 `.ralph/loops.json` 与 `git worktree list` 中已完成的 worktree 并自动复用;**严禁**不看参数就 `EnterWorktree` / `git worktree add` 创建新 worktree。旧版"从 prompt 文本自动猜测 plan 路径"的行为已废弃。推荐示例:
   ```bash
   ralph -H builtin:ce-executor-serial run --worktree --reuse-worktree \
     --plan docs/plans/2026-06-25-002-feat-profiles-for-preset-role-tuning-plan.md
   ```

## 收到 `task.resume` 时（policy / origin / contract 拒收后自动注入）

编排器拒收后会在 PENDING EVENTS 注入 `task.resume`（payload 形状：`crates/ralph-core/src/event_loop/rejection.rs:424-500+` `build_task_resume_payload`）。**不要重发同样 payload**，按以下顺序修复：

1. **读 PENDING EVENTS 里 `task.resume` 的 JSON payload**，关键字段：
   - `stage`：`origin` / `policy` / `execution_contract` / `payload_contract`
   - `topic`：被拒收的事件主题
   - `violation`：人类可读原因（含字段名 / 类型不匹配）
   - `required_fields`：当前 topic 缺失或类型错的字段清单
   - `allowed_topics`：当前 hat 可发布的所有 topic（**只在这列里挑**）
   - `reason` / `kind`：结构化 reason code（U2）
   - `target_hat`：应当修复并重发的目标 hat
2. **对照 `required_fields` 补齐 payload**；用 `ralph emit <topic> --policy-check -j '...'` 预检（与 loop gate 同源 schema，**不写盘**）；通过后再正式 `ralph emit` 落盘。
3. **确认 hat 作用域**：isolated 模式下未在 `allowed_topics`（与 hat `publishes` 交集）的 topic 越权 — 改用 hat 实际可发的 topic，不要靠 `--unsafe-no-policy-check` 绕过。
4. **不要**用 `--unsafe-no-policy-check` 绕 policy；`ce-executor-serial` preset 默认 `allow_unsafe_cli_emit: false`，该参数直接被拒。**不要**直写 `events.jsonl` — 写完仍会被 `payload_contract` 拒。
5. **复杂 violation**：按需加载对应 skill 查 deep-dive；当前 `ralph tools skill` 列表见命令速查表。
6. **仍不明**：`RALPH_DIAGNOSTICS=1` 启的 loop 把 envelope 写到 `recovery.jsonl`；`ralph diagnose --session latest` 出报告（`docs/guide/runtime-diagnosis.md` §10）。

## 命令速查表

### `ralph tools` 命名空间（已注入，按需读取对应子 skill）

| 命令 | 用途 | 详细参考 |
|------|------|---------|
| `ralph tools task` | 任务管理 | 已注入（`ralph-tools-tasks` skill，仅当 `tasks.enabled`） |
| `ralph tools memory` | 记忆管理 | 已注入（`ralph-tools-memories` skill，仅当 `memories.enabled`） |
| `ralph tools skill` | 加载 skill | `ralph tools skill load ralph-tools-cmdref` |

> U8 (2026-06-25): `ralph tools interact` 与 `ralph bot` 已随 `ralph-telegram` crate 一起删除;运行时不再提供人工通道(`human.guidance` 已废弃,见 plan 2026-06-28-005;`task.resume` 恢复通道保留)。

### 顶层命令（按需加载对应 skill）

| 命令 | 用途 | 详细参考 |
|------|------|---------|
| `ralph emit` | 发射事件（最常用） | `ralph tools skill load ralph-tools-emit` |
| `ralph emit --schema <TOPIC>` | 只读查 topic 协议 + protocol_hash | 同上 |
| Precheck gate（`event_loop.precheck`） | 非 CLI；loop 内 `.proposed`/gate 行为 | `ralph tools skill load ralph-tools-precheck` |
| `ralph wave emit` | 并行 wave 调度 | `ralph tools skill load ralph-tools-wave` |
| `ralph tools task verify <verb>` | 零写盘 Precheck（含 `verify-emit-bridge` 三字段同源） | `ralph tools skill load ralph-tools-tasks` |
| `ralph wave verify --payloads-stdin` | 零写盘 wave batch Precheck（dispatcher hat） | `ralph tools skill load ralph-tools-wave` |
| `ralph inspect loop` | 机器可读 loop + hat 身份摘要（OPAC Observe） | `ralph tools skill load ralph-tools-cmdref` |
| `ralph run` | 启动编排循环 | `ralph tools skill load ralph-tools-cmdref` |
| `ralph inspect profiles` | 预览 profile overlay 解析结果（只读，不启动 loop） | `ralph tools skill load ralph-tools-cmdref` |
| `ralph hats validate [--strict]` | 拓扑/payload/orphan/lint 校验 | `crates/ralph-cli/src/hats.rs:170`（strict 时启用 lint 所有权检查） |

> **OPAC 纪律（agent context）**: 所有 state-changing 操作遵循 Observe → Precheck → Apply → Confirm 四阶段；详见 always-injected `ralph-tools-opac` skill。本表所有 `ralph emit` / `ralph tools task` / `ralph wave emit` 行均按 OPAC 纪律执行——agent 上下文默认 enforce `--policy-check`（见 U15）。

> **按需加载需要 hat 上下文**：`ralph tools skill load` 在 agent 上下文中要求 `RALPH_CURRENT_HAT` 已设置，否则会以非零退出。如加载失败，先检查 `echo $RALPH_CURRENT_HAT` 是否非空。

## 通用错误恢复

| 错误场景 | 可能原因 | 修复方式 |
|----------|---------|---------|
| `events file not in allowlist` | `RALPH_EVENTS_FILE`/`--file` 指向了非 allowlist 路径 | 查看错误信息中列出的 allowlist 条目；优先移除显式参数让 `ralph emit` 走 marker 解析 |
| `topic is required` | 缺少必需的位置参数 | 补上 topic 参数 |
| `policy check failed` | 事件不符合策略 | 读 stderr / `--output json` 取 `validation_errors[].field`；修正后用 `ralph emit <topic> --policy-check -j '...'` 预检通过再正式发出 |
| `task not found` | task ID 不存在或属于其他 loop | `ralph tools task list` 确认当前可用任务 |
| `memory not found` | memory ID 不存在或无权访问 | `ralph tools memory list` 确认可用记忆 |
| `skill not found` | skill 名称错误或对当前 hat 不可见 / `RALPH_CURRENT_HAT` 未设 | `ralph tools skill list` 确认可用 skill；检查 `RALPH_CURRENT_HAT` |
| `progress rate limited` | 5 秒内重复发送 | 等待 5 秒后重试 |
| 退出码 2 (lint gate) | preset 静态 lint 在 strict 模式下发现 error | 修复 preset 配置后重试；查看 `.ralph/diagnostics/preset-lint-error-*.json` |
| `policy validation failed` (`ralph wave emit`) | 任一 payload 违反 required_fields，整批拒绝 | 用 `--output json` 读 `validation_errors[].field` 一次性拿到全部缺失字段，修正后重发 |
| `StateLedger replay failed` / ledger 损坏 | 进程崩溃导致 ledger 文件不完整 | `ralph loops clean --ledger` 截断损坏文件；下次启动自动 cold start 重建 |
| `plan.blocked` / `progress_task_mismatch` | 当前 step 的 task 状态与预期不一致 | 检查 `ralph tools task list`，按需关闭/重开相关 task |
| 任何命令失败 | 通用恢复 | 1. `ralph <cmd> --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |
