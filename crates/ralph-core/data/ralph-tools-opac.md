---
name: ralph-tools-opac
description: OPAC (Observe → Precheck → Apply → Confirm) discipline for state-changing operations in isolated mode
metadata:
  internal: true
  auto_inject: true
---

# OPAC — Agent 操作纪律框架

> **适用场景**：所有 state-changing 操作 — `ralph tools task *`、`ralph emit`、`ralph wave emit`、写 `.ralph/` 下任何文件。**顺序错误就是 bug**。

> **前提**：当 `memories.enabled` 或 `tasks.enabled` 启用时被 always-inject。读取本文档即可理解四阶段流程；**不要复制命令参数表**到 hat instructions，复述会产生漂移。

## 四阶段流程

| 阶段 | 目的 | 工具 |
|------|------|------|
| **O — Observe** | 「我现在是谁？系统是什么状态？」 | `ralph inspect loop` + `ralph tools task list` + 必要时 `ralph events --events-source hat-channel\|main`;**`event_loop.supervisor.enabled: true` 时 inspect loop JSON 含 supervisor 块**（`active_waves` / `slot_summary` / `last_coordination_topics`） |
| **P — Precheck** | 「这次操作会成功吗？写盘后会留下什么？」 | `ralph tools task verify <verb>` 或 `ralph emit/wave emit --policy-check` |
| **A — Apply** | 「实际写盘」 | `ralph tools task <verb>` 或 `ralph emit` / `ralph wave emit`（去掉 `--policy-check`） |
| **C — Confirm** | 「我真的写下去了吗？下一步要做什么？」 | `ralph events --events-source hat-channel`（单 emit）或 `--events-source main`（wave emit），跟 task close 后的 stderr warning（`close_without_completion_emit`）一起看 |

**每个 A 之前必须有 P，每个 A 之后必须有 C**。省略 Precheck 等于绕过 schema gate；省略 Confirm 等于 silent drop。

## Observe 阶段关键问题

1. **我是谁**：`ralph inspect loop --format json`（或在 prompt 中找 `## HAT IDENTITY` 块）
2. **loop 处于哪一阶段**：`## ORCHESTRATOR CONTEXT` 段
3. **任务当前状态**：`ralph tools task list` + `ready` 子命令
4. **我刚刚发的事件落到哪了**：同 activation 内用 `ralph events --events-source hat-channel`；跨 hat / 调试用 `--events-source main`
5. **supervisor 在做什么（仅当 `event_loop.supervisor.enabled: true`）**：`ralph inspect loop --format json` 的 `supervisor` 键
6. **loop 锚定的 plan 是哪个（U1 of 2026-07-04-004）**：`ralph inspect loop --format json` 的 `loop_anchor` 键

## loop_anchor 摘要（U1 of 2026-07-04-004）

当 loop 已 attach 到某个 plan（`config.event_loop.prompt_file` 指向 `.md`/`.html` 文件且不是默认 `PROMPT.md` 哨兵）时,`ralph inspect loop --format json` 的 `loop_anchor` 字段会带五段信息:

| 字段 | 含义 | 来源 |
|------|------|------|
| `plan_path` | 计划文件绝对或 workspace-相对路径 | `config.event_loop.prompt_file` |
| `plan_name` | `plan_path.file_stem()` 派生的稳定 key | `plan_path` 派生 |
| `plan_baseline_sha` | plan 启动时的 git HEAD SHA | `.ralph/agent/plan-baseline.sha` |
| `loop_start_sha` | loop runner 启动时的 git HEAD SHA（`LoopState.loop_start_sha`，line 461） | 未来 ledger 字段；当前为 `None` |
| `attached_at` | loop 注册时间（`.ralph/loops.json` 的 `started` 字段） | `LoopRegistry::list()` |

**未 attach 时**：`loop_anchor` 键整体省略（`skip_serializing_if = "Option::is_none"`），同时 `warnings` 数组会含 `"loop_anchor not attached; preset hats requiring loop_anchor will receive null. Pass --plan <path> to attach a plan, or run inside an active loop"`。

**schema bump**：v1 → v2 (`loop_inspect.v2`)；v1 消费者继续兼容（`loop_anchor` 与 `loop_id` 等新增键都是可选）。

## supervisor 摘要（U8 of 2026-07-04-002）

当 `event_loop.supervisor.enabled: true` 时,`ralph inspect loop --format json` 的 `supervisor` 字段会带四段信息:

| 字段 | 含义 | 来源 |
|------|------|------|
| `active_waves[]` | 当前未到终态的 wave 列表 | `SupervisorStore::recover_active_waves` |
| `queue_depth` | 所有 active wave 的非终态 slot 总和 | 同上 |
| `slot_summary[]` | **单一 active wave 时填充**：`{slot_id, hat, status}` 三元组,`hat` 是 wave kind 的稳定字符串标签（`exec-worker` / `fix-worker` / `review-worker`） | 同上 + `WaveSnapshot.slots` |
| `last_coordination_topics[]` | 每个 active wave 可能产出的 supervisor 协调 topic（`exec.wave.complete` / `exec.wave.failed` 等 6 个白名单项中按 wave kind 派生） | `SUPERVISOR_COORDINATION_TOPICS` ∩ wave kind,纯派生,无 db 读取 |

**字段填充契约**:

- `slot_summary` 仅当 `active_waves.len() == 1` 时填充——agent-safe 语义是"我的 slot 被什么 block",不是"全量状态 dump"
- `last_coordination_topics` 在 `active_waves` 为空时返回空数组,不伪造任何潜在 topic
- 输出**绝不**包含 db 路径、event log 内容或其他内部 ledger 字段
- 多次调用结果完全确定（同 store 状态 → 同 JSON）,适合机读 + 离线断言

**典型用法**:

```bash
# 收到 `ralph emit` 拒收 → supervisor wave 还在路上
ralph inspect loop --format json | jq '.supervisor.slot_summary[] | select(.status=="dispatched")'

# 想确认这条 wave 落地后会出什么协调 topic
ralph inspect loop --format json | jq '.supervisor.last_coordination_topics'

# supervisor 没启用 → JSON 没有 `supervisor` 键（不要假设 key 存在）
ralph inspect loop --format json | jq 'has("supervisor")'
```

## Precheck 阶段关键命令

- **task 变更**：`ralph tools task verify <add\|ensure\|start\|close\|fail\|reopen> [args…]`，三字段一致性：`ralph tools task verify-emit-bridge --task-id ID --task-key KEY --step STEP`
- **单事件 emit**：`ralph emit <TOPIC> --policy-check -j '<payload>'`，**不带 flag 写盘会被 agent context 默认 enforce 拒收**（参考 `ralph-tools-emit` §5 precheck）
- **wave emit**：`ralph wave verify --payloads-stdin`（零写盘 batch precheck）。**worker hat 不可 wave emit**（已在 `HatCommandPolicy` / dispatcher hat 限定）
- **shell 残留 `RALPH_CURRENT_HAT`**：operator 在 agent shell 残留变量是常见误用源；如发现 context 错乱，先 `unset RALPH_CURRENT_HAT`

## Apply 阶段两步式 task verify gate（U7 of 2026-07-04-003）

> **强制**：当 preset 启用了 `tasks.require_verify_for_cli_mutate: true`（ce-executor-serial 默认开），agent 调用 `task add` / `task ensure` 必须先走两步：

1. **P — Precheck record**：`ralph tools task verify <verb> [args…]` 通过后（Allow），runtime 在 `<workspace>/.ralph/agent/.ralph-task-verify-ticket` 写一个 one-shot ticket（SHA-256 fingerprint of `verb + canonical_payload + loop_id + hat_id`）
2. **A — Apply consume**：紧接着用**完全相同**的参数调 `ralph tools task <verb>` → gate 读 ticket、匹配 fingerprint、consume ticket、放行写盘

**漂移触发拒绝**（gate 必拒）：

- 没先 verify → `task_verify_gate denied '<verb>': no verify ticket at ... — run ralph tools task verify <verb> <args...> first`
- verify 后改了参数再 add → `task_verify_gate denied '<verb>': ticket fingerprint mismatch (on-disk=... pending=...)`
- 跨 hat 重放 ticket → `task_verify_gate denied '<verb>': ticket (loop, hat) = (...) but caller is (...)`

**人类 CLI 永远 bypass**（`is_agent_context == false`）；agent 在 `tasks.allow_unsafe_task_mutate: true` 时也 bypass（仅用于 recovery 紧急情况）。

## Apply 阶段红线

- 跨 loop/跨 hat 调用 `task add` / `task ensure` → **Deny**（agent context；人类 bypass + warning）
- 用 `task create` / `task make` 字面量 → **命令不存在**；用 `add` 或 `ensure`
- 同一 activation 内发第 2 条业务事件 → runtime 静默丢弃（终态事件前面的夹带事件会被一起丢）
- 对不存在的 `task_id` emit → payload_contract 拒收；先 `ralph tools task list` 拿 live id

## Confirm 阶段两种路径

| 写入路径 | Confirm 命令 | 文件 |
|---------|-------------|------|
| `ralph emit`（单事件） | `ralph events --events-source hat-channel` | `.ralph/current-hat-events` |
| `ralph wave emit`（批量） | `ralph events --events-source main` | `.ralph/events.jsonl`（wave 写 main 而非 hat-channel） |

agent context 默认 `--events-source auto` 优先 hat-channel；显式 `--events-source main` 用于 wave Confirm。

## 完成后：什么都不漏

`ralph tools task close` 后：

- 若 `event_loop.event_policy.{terminal_topics,business_topics}` 与 hat `publishes` 的交集中任一 topic 已写进 hat-channel → 无 warning
- 若无 → stderr JSON `close_without_completion_emit` 提示 `expected_topics` + `next_step`。**这个 warning 不阻塞 close**，但忽略它意味着 loop 进入 stall 30s 等待 rescue path

## 与其它 skill 的关系

| 任务 | 引用 |
|------|------|
| 命令参数表 / zsh 补全 | `ralph-tools-cmdref` |
| `--policy-check` + policy_check shape | `ralph-tools-emit` §5 |
| task_id/task_key/step red box | `ralph-tools-tasks` |
| Wave OPAC（dispatcher hat 视角） | `ralph-tools-wave` |
| precheck gate / proposed events | `ralph-tools-precheck` |
| 拒收后 `task.resume` 修复序列 | `ralph-tools-recovery-directives` |

## 反模式（出现即重写）

1. 直接 `echo … >> .ralph/agent/tasks.jsonl` — 绕过 store lock + auth
2. 在 hat instructions 里写完整 `ralph emit --policy-check -j '{"topic": ...}'` 字符串 — 这是 skill 的内容，**引用**而非**复述**
3. 用 `ralph events` 不带 `--events-source` 假设读 main — agent context 下默认 hat-channel
4. close 后立即走人 — 漏掉 Confirm 的 `close_without_completion_emit` warning
5. 跨 activation 共享一个 `task_id` — close 是 terminal，第二次 emit 一定被拒
6. 在 hat instructions 里写"读 `.ralph/supervisor.db`"或"运行 `ralph diagnose --supervisor`" — supervisor 的内部 ledger 与诊断输出都不在 hat 可观测范围；Observe 阶段用 `ralph inspect loop --format json` 的 `supervisor` 块即可
