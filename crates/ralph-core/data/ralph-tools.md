---
name: ralph-tools
description: Core CLI reference and rules for Ralph orchestration agents
metadata:
  internal: true
---

# Ralph CLI 核心参考

> **前提**：本 skill 仅在 `memories.enabled` 或 `tasks.enabled` 至少一个启用时被注入（`crates/ralph-core/src/event_loop/mod.rs:5152-5257` `inject_memories_and_tools_skill`，门控条件在 `5217`）。速查表中的"已注入"列均受此条件约束。

> **遇到不确定的命令语法时，先 `ralph <cmd> --help` 再执行。**

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
2. **对照 `required_fields` 补齐 payload**；用 `ralph emit <topic> --policy-check -j '...'` 在写盘前预检（与 loop gate 同源 schema）。
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
| `ralph run` | 启动编排循环 | `ralph tools skill load ralph-tools-cmdref` |
| `ralph inspect profiles` | 预览 profile overlay 解析结果（只读，不启动 loop） | `ralph tools skill load ralph-tools-cmdref` |
| `ralph hats validate [--strict]` | 拓扑/payload/orphan/lint 校验 | `crates/ralph-cli/src/hats.rs:170`（strict 时启用 lint 所有权检查） |

> **按需加载需要 hat 上下文**：`ralph tools skill load` 在 agent 上下文中要求 `RALPH_CURRENT_HAT` 已设置（`crates/ralph-cli/src/skill_cli.rs:77-87`），否则会以非零退出。如加载失败，先检查 `echo $RALPH_CURRENT_HAT` 是否非空。

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

## Agent Output Governance（2026-06-14 计划 003 — `ce-executor-serial` only）

`ce-executor-serial` preset 在四个卡点上加硬规则。agent 可能遇到如下机制 — 当以下块 / 变量 / 行为出现时，按对应说明处理。

### `## WAVE CONTEXT` Block（R1 — review-synthesizer only）

当 `review-synthesizer` hat 被激活时，runner 在 prompt 顶部注入固定格式块：

```text
## WAVE CONTEXT
The following wave metadata is injected by the runner. Do not count events manually — use this context.

```json
{
  "wave_id": "w-abc",
  "wave_total": 7,
  "received_count": 7,
  "expected_dimensions": ["correctness", "testing", "..."],
  "missing_dimensions": [],
  "ALL_DIMENSIONS_RECEIVED": true,
  "AGGREGATE_TIMEOUT": false
}
```
```

- 不要重新数 `events.jsonl` — 使用此上下文。`received_count` 已经包含 dimension done 事件的最新计数。
- 当 `ALL_DIMENSIONS_RECEIVED: true` 且 `AGGREGATE_TIMEOUT: false` 时，直接 emit `review.passed`。
- 当 `AGGREGATE_TIMEOUT: true` 时，list 中维度未齐，按 plan 的 `skip_reason=aggregate_timeout` 走。
- 当 `missing_dimensions` 非空时，复核是否还有未收的 worker。

**等效 env var**：`RALPH_WAVE_CONTEXT`（JSON 字符串，可 `echo $RALPH_WAVE_CONTEXT | jq`）。

#### Wave worker 环境变量

`ralph wave emit` 派发的 dimension worker 进程会注入以下 env var：

- `RALPH_WAVE_WORKER`：固定值 `1`（标记当前进程是 wave worker）。
- `RALPH_WAVE_ID`：当前 wave 的 `wave_id`。
- `RALPH_WAVE_INDEX`：当前 worker 在 wave 内的 0-based 索引。
- `RALPH_EVENTS_FILE`：当前 worker 专属的 events.jsonl 路径。
- `RALPH_WAVE_DIMENSION`：当前 worker 的分配维度；emit `review.dimension.done` 时必须用**精确等于**此值的 `dimension` 字段。
- `RALPH_WAVE_CONTEXT`：wave 元数据 JSON，review-synthesizer / synthesizer 路径用。

### `## EPHEMERAL RELOCATED` Block（R3 — review-synthesizer / executor / fixer / shipper）

当 agent 在源码树下写了 `scratchpad.md` / `notes.md` / `tmp*.md` / `*.bak` 等运行时产物时，runner 在下一轮 prompt 顶部注入：

```text
## EPHEMERAL RELOCATED
The following runtime artefacts were moved out of the source tree by the runner. Do NOT recreate these files inside the source tree; write runtime notes to `.ralph/agent/` instead.

- `crates/ralph-core/scratchpad.md` → `.ralph/agent/scratchpad-{loop_id}.md` (1234 bytes appended)
```

- 已经迁移的内容在 `.ralph/agent/scratchpad-{loop_id}.md`。`cat .ralph/agent/scratchpad-*.md` 可读。
- 不要再在源码树下重建这些文件（触发 review wave + P0 finding）。
- 该块被首次读取后消费（`std::mem::take`），后续 hat activation 看不到 — 这是设计。

### `ralph tools task ensure` 与 R4 Single-U 契约

**默认关闭**。当 `ce-executor-serial` preset 启动后，`ralph run` 写 `.ralph/agent/.ralph-enforce-current-unit` marker，子进程 `ralph tools task ensure` 检测后激活契约。standalone CLI 用户可设环境变量 `RALPH_ENFORCE_CURRENT_UNIT=1`。

**契约**：

- key 形如 `ce-executor:{plan}:step-XX:uN-impl`（N 是数字）才被 gate。`u1a-impl` / `u1b-impl` 塌缩到 `u1`，允许并存。
- 同一 `(loop_id, plan_name, step)` 下已 open U1 task，再 ensure `u2-impl` 时：CLI 退出非零，stderr 输出 `rejected by R4 single-U contract: ...`；ensure 返回已存在的 U1 task。
- 同 key 重复 ensure 是幂等的 — 返回同一 task。
- 旧 key / 非 `uN-` 形状（`step-99-impl`、`review-bug-impl` 等）**不被 gate** — 这是已知边界，**不要**依赖 R4 保护非 canonical keys。
- 失败时不要重试同一 key — 切换到下一 U 或关闭冲突 task。
