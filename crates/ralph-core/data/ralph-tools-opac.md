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
| **O — Observe** | 「我现在是谁？系统是什么状态？」 | `ralph inspect loop` + `ralph tools task list` + 必要时 `ralph events --events-source hat-channel\|main`；若 JSON 含 `supervisor` 键，可读 `active_waves` / `slot_summary` / `last_coordination_topics`（见下方「supervisor 摘要」门控） |
| **P — Precheck** | 「这次操作会成功吗？写盘后会留下什么？」 | `ralph tools task verify <verb>`；单事件用 `ralph emit --policy-check`；wave 批用 `ralph wave verify --payloads-stdin`（不是 `wave emit --policy-check`） |
| **A — Apply** | 「实际写盘」 | `ralph tools task <verb>`；单事件用去掉 `--policy-check` 的 `ralph emit`；wave 用与 verify **相同** payload 的 `ralph wave emit`（先 verify 拿 ticket，再 emit） |
| **C — Confirm** | 「预期状态真的产生了吗？下一步要做什么？」 | 优先检查本次操作通过公开接口给出的成功反馈；反馈不足时，再按对应 skill 使用只读查询接口确认 |

**每个 A 之前必须有 P，每个 A 之后必须完成 C；Confirm 不可省略**。Apply 后必须按下方路由找到专项 skill，并取得该 skill 规定的有效证据。Confirm 不等于“所有操作固定再跑同一条查询命令”：具体证据和查询方式由专项 skill 定义。未找到 skill、未取得证据或证据不一致时，必须停止，不得继续下一次状态变更。省略 Precheck 会绕过写入前约束；省略 Confirm 会把“命令已执行”误当成“预期状态已产生”。

**单事件 emit 的完成判定是固定的**：`--policy-check` 的 `ok=true` 只表示预检通过，且 `recorded=false`；它永远不能作为 Apply 或 Confirm 的证据。必须再执行不带 `--policy-check` 的正式 emit，并看到 `ok=true` 且 `recorded=true`。没有这两个字段的明确回执，就把操作视为未完成并停止。

**Isolated hat 的额外落点约束**：保留 runner 注入的 `RALPH_EVENTS_FILE`，不要 `unset`、改写或用 `--file` 覆盖它。禁止直接编辑任何事件 JSONL；将正式 emit 的 `target_path` 和 `$RALPH_EVENTS_FILE` 规范化为绝对路径后若仍明确不一致，视为落点错误，即使命令退出码为 0 也不得继续。

## Observe 阶段关键问题

1. **我是谁**：`ralph inspect loop --format json`（或在 prompt 中找 `## HAT IDENTITY` 块）
2. **loop 处于哪一阶段**：`## ORCHESTRATOR CONTEXT` 段
3. **任务当前状态**：`ralph tools task list` + `ready` 子命令
4. **上一操作产生了什么状态**：先检查该操作的公开成功反馈；需要进一步确认时，按对应 skill 使用公开只读接口，不读取内部 ledger
5. **supervisor / wave 账本在做什么（仅当 inspect JSON 含 `supervisor` 键）**：`ralph inspect loop --format json` 的 `supervisor` 键；**先** `jq 'has("supervisor")'`，为 false 则跳过，不要读内部 ledger 文件
6. **loop 锚定的 plan 是哪个**：`ralph inspect loop --format json` 的 `loop_anchor` 键

## loop_anchor 摘要

当 loop 已 attach 到某个 plan 时，`ralph inspect loop --format json` 的 `loop_anchor` 字段会带五段信息。`ralph inspect loop` 按以下顺序解析：

1. **优先** — `.ralph/agent/.ralph-anchor.json`（`ralph resume --plan <file>` 写入；resume 路径不依赖 `prompt_file` 被改写）
2. **fallback** — `config.event_loop.prompt_file` 指向 `.md`/`.html` 且不是默认 `PROMPT.md` 哨兵（`ralph run --plan` 路径）
3. marker 缺失或 JSON 损坏 → warning 后走 fallback；两者都失败则 `loop_anchor` 省略

| 字段 | 含义 | 来源 |
|------|------|------|
| `plan_path` | 计划文件路径 | marker 或 `config.event_loop.prompt_file` |
| `plan_name` | `plan_path.file_stem()` 派生的稳定 key | `plan_path` 派生 |
| `plan_baseline_sha` | plan 启动时的 git HEAD SHA | `.ralph/agent/plan-baseline.sha` |
| `loop_start_sha` | loop runner 启动时的 git HEAD SHA | 未来 ledger 字段；当前为 `None` |
| `attached_at` | attach 时间 | `ralph inspect loop --format json` 的 `loop_anchor.attached_at`（runtime 已解析；agent 只读该公开字段，不要打开内部 ledger 文件） |

**未 attach 时**：`loop_anchor` 键整体省略，同时 `warnings` 数组会含 `"loop_anchor not attached; preset hats requiring loop_anchor will receive null. Pass --plan <path> to attach a plan, or run inside an active loop"`。

**marker 文件**：`.ralph/agent/.ralph-anchor.json`（runtime 状态，勿手改；由 `ralph resume --plan` 原子写入）。

## supervisor 摘要

**何时出现 `supervisor` 键（Observe 门控）**：`ralph inspect loop --format json` 在下列**任一**成立时包含 `supervisor` 块；**两者都不成立**时整键省略（纯单链 pipeline 保持安静）：

1. 当前配置 `event_loop.supervisor.enabled: true`；或
2. runtime 已能打开本 loop 的 wave 账本（常见于 default-wave：配置里 supervisor 未开，但先前 wave 已留下可恢复账本）

agent **只**用 `jq 'has("supervisor")'` 判断，**不要**去读或探测内部 ledger 文件路径。键存在但账本暂不可读时，字段多为空默认值（`active_waves: []`、`queue_depth: 0` 等），仍属合法 Observe 结果。

键存在时，`supervisor` 带四段公开摘要：

| 字段 | 含义 | agent 用法 |
|------|------|------|
| `active_waves[]` | 当前未到终态的 wave 列表 | 判断是否仍有进行中的 batch |
| `queue_depth` | 所有 active wave 的非终态 slot 总和 | 粗看积压 |
| `slot_summary[]` | **仅当恰好一个 active wave 时填充**：`{slot_id, hat, status}`；`hat` 为 wave kind 标签（如 `exec-worker` / `fix-worker` / `review-worker`） | 看本 activation 相关 slot 是否仍 `dispatched` |
| `last_coordination_topics[]` | 当前 active wave 可能产出的协调 topic 派生列表（如 `exec.wave.complete` / `exec.wave.failed`） | 只读预期；不要自行 emit 这些 topic |

**字段填充契约**:

- `slot_summary` 仅当 `active_waves.len() == 1` 时填充——语义是「我的 slot 被什么挡住」，不是全量 dump
- `last_coordination_topics` 在 `active_waves` 为空时返回空数组，不伪造潜在 topic
- 输出**绝不**包含内部 ledger 路径、event log 正文或其他不可见字段
- 同 store 状态下多次调用结果确定，适合机读

**典型用法**:

```bash
# 先看键在不在（门控）；false → 跳过本段
ralph inspect loop --format json | jq 'has("supervisor")'

# 收到拒收且 supervisor 键存在 → 看 slot 是否仍在路上
ralph inspect loop --format json | jq '.supervisor.slot_summary[]? | select(.status=="dispatched")'

# 只读：这条 wave 落地后可能出现的协调 topic（不要自行 emit）
ralph inspect loop --format json | jq '.supervisor.last_coordination_topics // []'
```

## Precheck 阶段关键命令

- **task 变更**：`ralph tools task verify <add\|ensure\|start\|close\|fail\|reopen> [args…]`，三字段一致性：`ralph tools task verify-emit-bridge --task-id ID --task-key KEY --step STEP`
- **单事件 emit**：`ralph emit <TOPIC> --policy-check -j '<payload>'`，**不带 flag 写盘会被 agent context 默认 enforce 拒收**（参考 `ralph-tools-emit` §5 precheck）
- **wave emit**：先 `ralph wave verify --payloads-stdin`（同源 schema 预检 + 一次性 ticket），再用**未改动的同一批** payload 跑 `ralph wave emit --payloads-stdin`。**不要**把 `wave emit --policy-check` 当成 wave 的 Precheck。**worker hat 不可 wave emit**（仅 dispatcher hat 可调用）；细节见 `ralph-tools-wave`
- **shell 残留 `RALPH_CURRENT_HAT`**：operator 在 agent shell 残留变量是常见误用源；如发现 context 错乱，先 `unset RALPH_CURRENT_HAT`

## Apply 阶段两步式 task verify gate

> **强制**：当 preset 启用了 `tasks.require_verify_for_cli_mutate: true` 时，agent 调用 `task add` / `task ensure` 必须先走两步：

1. **P — Precheck record**：`ralph tools task verify <verb> [args…]` 通过后（Allow），runtime 在 `<workspace>/.ralph/agent/task-tickets/` 写一个 one-shot ticket（SHA-256 fingerprint of `verb + canonical_payload + loop_id + hat_id`）
2. **A — Apply consume**：紧接着用**完全相同**的参数调 `ralph tools task <verb>` → gate 读 ticket、匹配 fingerprint、consume ticket、放行写盘。成功的 Apply 会在写入的 task 行附带一条 confirmation 记录（状态 `pending`）；Apply 的 `--format json` 输出含 `confirmation.reference`（唯一确认凭证）与 `confirmation.digest`（该 mutation 的指纹）
3. **C — Confirmation consume**：在同一 loop + 同一 hat 发起**下一次** protected mutation（即 gate 生效时受两步式 verify gate 保护的 `task add` / `task ensure`；下文 protected Apply 同指这两条命令）之前，先执行 `ralph tools task confirm <task_id> --reference <reference> --digest <digest>`——两个字段值直接取自上一步 Apply 的 JSON 输出，不要手工构造或复用其它 task 的值。若产生 pending 记录的 Apply 输出已不在当前上下文（例如新一轮 iteration），执行 `ralph tools task show <task_id> --format json`，行内 `confirmation.reference` / `confirmation.digest` 即所需值，同样不要手工构造。confirm 成功后状态变 `confirmed`，同 scope 的下一次 protected mutation 放行；重复 confirm（相同 reference + digest）幂等、exit 0、不重复写盘

**漂移触发拒绝**（gate 必拒）：

- 没先 verify → `task_verify_gate denied '<verb>': no verify ticket at ... — run ralph tools task verify <verb> <args...> first`
- verify 后改了参数再 add → `task_verify_gate denied '<verb>': ticket fingerprint mismatch (on-disk=... pending=...)`
- 跨 hat 重放 ticket → `task_verify_gate denied '<verb>': ticket (loop, hat) = (...) but caller is (...)`
- 同 loop + 同 hat 上一条 protected Apply 的 confirmation 仍为 `pending` → `task_verify_gate denied '<verb>': confirmation_required — ...`；按 stderr 里的指引先 `ralph tools task confirm <task_id> --reference ... --digest ...` 再重试（prepared ticket 保留，confirm 后同一参数重试无需重新 verify）。若产生 pending 记录的 Apply 输出已不在当前上下文（例如新一轮 iteration），执行 `ralph tools task show <task_id> --format json`，行内 `confirmation.reference` / `confirmation.digest` 即所需值，同样不要手工构造
- confirm 的 reference 与记录不符 → `confirmation_unavailable`；reference 相符但 digest 或 loop/hat 不符 → `confirmation_mismatch`（状态保持 `pending`）。两者都不要靠猜参数重试——重新读取产生该记录的 Apply 输出

**人类 CLI 永远 bypass**（`is_agent_context == false`）；agent 在 `tasks.allow_unsafe_task_mutate: true` 时也 bypass（仅用于 recovery 紧急情况）。

## Apply 阶段红线

- 跨 loop/跨 hat 调用 `task add` / `task ensure` → **Deny**（agent context；人类 bypass + warning）
- agent 调 `task add` / 普通 `task ensure --key` 若被拒且 hint 指向 handoff 事件 → 改走 hat Trigger State Table，不要重复 CLI 建 task
- 用 `task create` / `task make` 字面量 → **命令不存在**；用 `add` 或 `ensure`
- 同一 activation 内发第 2 条业务事件 → runtime 静默丢弃（终态事件前面的夹带事件会被一起丢）
- 对不存在的 `task_id` emit → payload_contract 拒收；先 `ralph tools task list` 拿 live id

## Confirm 阶段通用规则

Confirm 验证的是**预期效果**，不是机械重复读取：

1. **先找到专项规则**：按下表加载或阅读当前操作对应的 skill，不得凭经验猜测有效证据。
2. **按专项规则检查证据**：只有专项 skill 规定的公开成功反馈或只读查询结果可以完成 Confirm；仅有命令退出成功不算完成。
3. **区分本地操作成功与流程推进**：操作成功只证明本次修改已生效，不代表下游 hat 已处理或工作流已进入下一阶段。只有当前任务要求确认流程推进时，才查询下游可见状态。
4. **失败即停止**：未找到专项 skill、没有取得有效证据、查询结果不一致或出现 warning 时，不要继续下一次状态变更；先按专项 skill 的恢复步骤处理。
5. **操作者交付文件（多数 hat 跳过）**：仅当 hat instructions 要你写出操作者可读文件，且 `ralph emit --schema <TOPIC>` 显示该 topic 的 `required_fields` 含路径字段时——先 `test -f`，再 emit，再在可见回复打印 `DELIVERABLE_PATH: <同路径>`。中间业务 emit 不要打印。细节（含双事件收尾）在按需 skill：`ralph tools skill load ralph-tools-emit` →「操作者交付文件路径」。

| 当前操作 | Confirm 规则来源 | Agent 动作 |
|---------|------------------|------------|
| `ralph emit` | `ralph-tools-emit` | `ralph tools skill load ralph-tools-emit` |
| `ralph tools task` | `ralph-tools-tasks` | 使用 prompt 中已注入的 task skill；若不可见则停止 |
| `ralph tools memory` | `ralph-tools-memories` | 使用 prompt 中已注入的 memory skill；若不可见则停止 |
| `ralph wave` | `ralph-tools-wave` | `ralph tools skill load ralph-tools-wave` |
| 无法判断操作类别 | 当前 hat 的可见 skill 列表 | `ralph tools skill list --format json`；仍找不到就停止，不得猜测 |

不要为了 Confirm 读取或修改 runtime 内部文件。专项确认方式见上表列出的 skill。

## 完成后：什么都不漏

完成状态变更后，检查公开成功反馈和 warning。若反馈指出仍缺少后续动作，就按其中给出的下一步处理；不要因为命令退出成功就直接结束 activation。task close 的具体完成检查见 `ralph-tools-tasks`，event 与 wave 的确认方式分别见 `ralph-tools-emit`、`ralph-tools-wave`。

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
3. 把某一种 event 查询方式当成所有操作的通用 Confirm — 应按对应 skill 选择公开反馈或只读接口
4. 命令退出成功后立即走人 — 应先确认预期状态，并处理公开反馈中的 warning 或下一步动作
5. 跨 activation 共享一个 `task_id` — close 是 terminal，第二次 emit 一定被拒
6. 在 hat instructions 里写"读 `.ralph/supervisor.db`"或"运行 `ralph diagnose --supervisor`" — supervisor 的内部 ledger 与诊断输出都不在 hat 可观测范围；Observe 阶段用 `ralph inspect loop --format json` 的 `supervisor` 块即可
7. 本轮要交操作者文件却不打印 `DELIVERABLE_PATH`，或打印了不存在的路径 — 操作者在 TUI 找不到交付物；按需 load `ralph-tools-emit`「操作者交付文件路径」
8. schema 不要求路径字段的中间业务 emit 也打印 `DELIVERABLE_PATH` — 噪音；只在「写操作者文件 + schema 要求路径字段」时打印
9. 用 `tail` 读事件文件当作 emit Confirm — 应用 `--output json` 的 EmitResult（`ok` / `recorded`）
