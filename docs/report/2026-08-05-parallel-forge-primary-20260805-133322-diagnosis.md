---
title: parallel-forge Loop `primary-20260805-133322` 运行链路诊断报告
date: 2026-08-05
type: diagnosis
loop_id: primary-20260805-133322
preset: builtin:parallel-forge
run_dir: /Users/pittcat/Dev/Rust/ralph-e2e
status: 业务交付完成但 settlement 投影失败，最终终态未被接受
diagnostics_mode: LOGS_ONLY
history_search: disabled
execution_capabilities: [supervisor, wave]
---

# parallel-forge Loop `primary-20260805-133322` 运行链路诊断报告

> 生成时间：2026-08-05
>
> 诊断对象：`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/`。
>
> 对照 preset：`presets/en/parallel-forge.yml` 与 `presets/schemas/parallel-forge.yml`。
>
> 历史检索：`disabled`。本报告只使用本次 run 产物、当前 preset/schema 和当前源码。
>
> 诊断模式：`LOGS_ONLY`。没有 FULL/MINIMAL orchestration artifact，因此 agent 的完整 tool-call 顺序与 OPAC Confirm 不能完全证明。

## 0. 产物盘点（Phase 0）

`execution_capabilities: [supervisor, wave]`。

- `supervisor`：preset 的 `event_loop.supervisor.enabled: true`（`presets/en/parallel-forge.yml:160-164`），且 run 中存在 `.ralph/supervisor.db`。
- `wave`：主事件 `events-20260805-133322.jsonl` 含 `wave_id`（第 6 行起）；但该 run 只把产物信号作为能力证据，不把 `exec.wave.*` 当作 wave fan-out 的唯一判据。

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` | 是 | 1 个指针 | 唯一主事件文件为 `.ralph/events-20260805-133322.jsonl` |
| S | 指向的 events | 是 | 21 行 | 第 1-20 行为业务/协调记录，第 21 行 raw `LOOP_COMPLETE`；无 accepted `forge.report.done` |
| S | `.ralph/recovery.jsonl` | 是 | 2 行 | repair-stream 记录，不等同于主事件账本中的 accepted business event |
| S | `.ralph/ledger.jsonl` | 是 | 14 行 | 迭代推进至 10，随后 completion rejection |
| S | `.ralph/agent/tasks.jsonl` | 是 | 10 行 | 5 个 Unit task 仍 open；5 个 supervisor slot task closed |
| S | `.ralph/flow-authority.jsonl` | 是 | 15 行 | 最后两条为 `stall-detector` 产生的 `forge.plan.blocked → cleanup` |
| S | `.ralph/loops.json` / `current-loop-id` | 是 | — | loop id 与 run 一致 |
| S | `.ralph/loop.lock` | 否 | released | 本次诊断时 loop 已停止；日志显示 14:23:31 用户 abort |
| B | `.ralph/diagnostics/logs/` | 是 | 2 个 log | 因没有 timestamped orchestration session，诊断模式为 `LOGS_ONLY` |
| B | `.ralph/diagnostics/channel-routing-fallback-*.md` | 是 | 1 个 | reporter channel 为空后的 fallback 记录 |
| B | `.ralph/supervisor.db` | 是 | — | supervisor capability 所需 ledger 存在 |
| B | reporter hat-channel | 是 | 0 bytes | `.ralph/agent/events-hat-reporter-primary-20260805-133322-12.jsonl` 为空 |
| C | `.ralph/forge/<plan-key>/` | 是 | 业务 artifacts 已落盘 | inspection、development/execution plan、5 个 completion、review、integration、verification、settlement、worktree map 均存在；没有 `cleanup.md` 或 block artifact |
| C | `docs/reports/2026-08-05-sorts-supervisor-e2e-manager-report.md`（run 内） | 是 | — | report 文件存在，但没有对应 accepted `forge.report.done`，不能反写终态 |

## 1. 结论摘要

### 1.1 健康度

- 判定：**部分业务交付成功，但编排死锁并以未接受终态结束**。
- P0：1（confidence ≥ 70）；P1：2（confidence ≥ 60）。
- 最高优先级根因置信度：P0-1 = **85/100**。
- 历史复发：`N/A (history disabled)`。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ | LOGS_ONLY 只能证明 runtime 拒收与终态缺失；reporter prompt visibility 正常，但无法证明每次 Confirm 的完整 tool-call 顺序 | 50 |
| Q2 | 基座机制是否正常生效？ | ✅/⚠️ | supervisor fan-out、review、integrate、verify 均产生了对应事件；projection、required-events、flow policy 也确实拒收非法推进 | 75 |
| Q3 | 编排是否合理、正常运行？ | ❌ | flow 声明要求 `forge.exec.development.done`，但没有实际 hat 发布它；单 wave settle 后永远不能进入 `full_verify` | 85 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **主要是 preset 编排缺口，叠加 agent payload 类型错误和 fallback 放大** | 主因 P0-1 为 preset；P1-2 为 producer payload；P1-3 为 fallback 后的恢复路径不适配 | 85 |

### 1.3 根因一句话

integrator 将 `forge.wave.settled.settled_task_ids` / `settled_unit_ids` 发成逗号分隔字符串，CloseTaskBatch 要求数组而拒收；5 个 Unit task 因此仍为 open，dispatcher 无法确认最后一波已结算并发出 `forge.exec.development.done`，后续 `cleanup → report` 链无法正常启动。（置信度 **85/100**）

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| 首轮终态 | 证据不足以判定成功终态；主账本没有 accepted `forge.report.done`，raw `LOOP_COMPLETE` 被 required-events 门禁拒绝 |
| 恢复状态 | 无成功恢复；两次 `forge.plan.blocked` 只出现在 flow authority / recovery repair-stream，未产生后续 accepted cleanup/report 链 |
| 最终代码状态 | wave integrated candidate 为 `d543ea86b9e5074c0a0fc32d71d21caf80351e6e`；review/verify 业务 artifacts 显示 5 个 Unit 完成，但 `.ralph/agent/tasks.jsonl` 的 5 个 Unit task 仍为 open |
| 一致性告警 | ⚠️ run 内 manager report 声称 `COMPLETED`，但该结论没有 accepted `forge.report.done` 支撑；不能把 report 文件存在当作 loop 已闭环 |

## 2. 执行链路对比

实际主账本链路：

```text
forge.start
  → forge.plan.inspected
  → forge.plan.ready
  → forge.concurrency.approved
  → forge.worktrees.ready
  → exec.unit.ready ×5
  → exec.unit.done ×5
  → exec.wave.complete
  → forge.wave.reviewed
  → forge.wave.integrated
  → forge.wave.verified
  → forge.wave.settled   [payload 中 settled_task_ids 为 string，projection 拒收]
  → 无 forge.exec.development.done
  → LOOP_COMPLETE       [required forge.report.done 缺失，拒收]
  → stall-detector forge.plan.blocked ×2
  → reporter fallback    [empty hat-channel，无 forge.report.done]
  → user abort
```

preset 期望的成功链路是：

```text
forge.wave.settled
  → forge.exec.development.done
  → forge.full.verified
  → forge.audit.done
  → forge.finalized
  → forge.cleanup.done
  → forge.report.done
  → LOOP_COMPLETE
```

关键差异是 settlement 的状态投影：`forge-dispatcher` 实际声明并发布 `forge.exec.development.done`（`presets/en/parallel-forge.yml:545-559`），且 instructions 明确要求在“所有 static wave settled 且无 open task”时发布它（约 `presets/en/parallel-forge.yml:620-630`）。本次由于 `settled_task_ids` 类型错误，CloseTaskBatch 没有关闭 Unit task，dispatcher 没有得到有效的全量结算状态。不能通过给 integrator 额外添加 `forge.exec.development.done` 来绕过 task settlement；那会破坏现有的单 activation 单业务事件契约。

## 3. 历史问题上下文

`N/A (history disabled)`。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | `forge.wave.settled.settled_task_ids` 被发为逗号分隔字符串，CloseTaskBatch 要求数组 | log `ralph...log:66`；主 events 第 20 行；`state_projector/task.rs:788-807`；tasks ledger 5 个 Unit 仍 open | P0 | 85 | file:line(+25)、双账本(+20)、preset/schema 行号(+15) | 无 FULL agent-output，不能确认是哪个 producer tool-call 生成该字符串 |
| DEV-002 | dispatcher 未在 settlement 拒收后形成有效的全量结算推进 | `presets/en/parallel-forge.yml:545-630`；flow authority 第 13-15 行；主 events 无 `forge.exec.development.done` | P1 | 75 | file:line(+25)、双账本(+20)、preset 行号(+15) | 缺 FULL orchestration |
| DEV-003 | reporter 被 fallback 激活时 hat-channel 为空，且当前 flow 仍不允许 `forge.report.done` | fallback diagnostic；log `...log:88-90`；`hat_channel.rs:76-88`；report step `parallel-forge.yml:141-150` | P1 | 75 | file:line(+25)、双账本(+20)、preset 行号(+15) | 无 FULL activation/orchestration；无法确认空 channel 是 backend crash、timeout 还是 runner race |

### 4.1 Prompt visibility 对账

`ralph -H builtin:parallel-forge inspect prompt --hat reporter --format json --trigger forge.cleanup.done ...` 显示：

- `auto_inject`: `ralph-tools`, `ralph-tools-tasks`, `ralph-tools-memories`, `ralph-tools-opac`；
- `on_demand`: `ralph-tools-cmdref`, `ralph-tools-emit`, `ralph-tools-precheck`, `ralph-tools-recovery-directives`, `ralph-tools-wave`；
- trigger simulation 识别 `source_topic=forge.cleanup.done`、`source_hat=cleanup`，并注入 cleanup payload 字段。

没有发现 reporter 的 auto/on-demand skill visibility 矛盾。因此本次不把“agent 看不到 emit skill”列为根因；但 LOGS_ONLY 下无法证明实际 activation 是否按要求加载了 on-demand skill。

### 4.2 OPAC 逐 hat 审计（LOGS_ONLY）

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| executor/integrator | ✅ | ⚠️ | ✅ | ⚠️ | events 有 unit/wave/integrate/verify；settlement projection 拒收 | 50 |
| reporter | ✅ | ⚠️ | ✅ | ❌ | fallback reporter activation；无 report event；无法证明完整 precheck 序列 | 50 |
| stall-detector | ✅ | N/A | N/A | N/A | logs 明确记录 fail-close blocked 事件 | 60 |

LOGS_ONLY 说明：未观察到 `policy-check` 记录不能单独证明 agent 违规；只有 runtime 的明确拒收证据才用于本报告的问题归因。

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P0 | settlement payload 类型与 CloseTaskBatch 契约不符，5 个 Unit task 保持 open，后续终态链无法启动 | mechanism | **85** | DEV-001 | file:line(+25) + 双账本(+20) + schema/preset 行号(+15)；agent 具体 tool-call 因 LOGS_ONLY 未核实 | `N/A (history disabled)` | 第 1 轮：源码反查；第 2 轮：tasks ledger 交叉核验 |
| P1 | dispatcher 未在 settlement 拒收后形成有效的全量结算推进 | mechanism | **75** | DEV-002 | file:line(+25) + 双账本(+20) + preset 行号(+15)；LOGS_ONLY 上限 | `N/A (history disabled)` | 第 1 轮：dispatcher instructions/preset + flow authority 对账 |
| P1 | fallback reporter 没有可用 hat-channel，且被激活在错误 flow step；因此不能合法补发 report | mechanism | **75** | DEV-003 | file:line(+25) + 双账本(+20) + preset 行号(+15)；LOGS_ONLY 上限 | `N/A (history disabled)` | 第 1 轮：runner/hat-channel 源码 + logs/diagnostic |

DEV-002 的归因边界：本报告确认“producer payload 不符合 runtime 契约”，但不把责任进一步定性为某个 agent 的操作失误；LOGS_ONLY 缺少 FULL agent-output，因此 agent 责任不单独入表。

## 6. 修复建议

### 6.1 短期（operator workaround）

不要手工编辑 `.ralph/` 状态文件，也不要直接补发 `forge.report.done` 绕过 flow。先停止当前 run，保留该 run 作为诊断证据；若需清理资源，使用受支持的 loop/task/worktree 命令并单独记录结果。已有 5 个 Unit task 为 open，不能把 manager report 的 `COMPLETED` 当作 task ledger 已结算。

### 6.2 中期（preset / schema / instructions）

先修正 integrator 的 settlement payload，使 `settled_task_ids` 与 `settled_unit_ids` 始终为 JSON string array；不要给 integrator 追加 `forge.exec.development.done`。dispatcher 已是该事件的唯一职责方。同步检查 `presets/schemas/parallel-forge.yml`、builtin embed parity、flow tests、BDD scenario 和 agent-facing tools 文档。

同时把 `settled_task_ids` / `settled_unit_ids` 的数组类型写成明确的 schema/runtime 可验证契约，并在真实 runtime scenario 中断言 settlement 后 task ledger 关闭、dispatcher 发出 `forge.exec.development.done`，随后出现 `forge.cleanup.done`、`forge.report.done`、`LOOP_COMPLETE`。

### 6.3 长期（机制 / 底座）

为 declared loop 检查增加“每个 transition emit 至少有一个可达 publisher”的结构化 lint，避免 `transition_emits` 与 hat `publishes` 脱节。对 fallback activation 增加合法 trigger/step 约束：当 hat-channel 为空或 fallback hat 不拥有当前 step 的 publish obligation 时，应停止并给出可恢复诊断，而不是继续产生重复 stall-blocked activation。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| reporter hat-channel 为空的直接原因是 backend crash、超时、channel marker race 还是 runner cleanup 顺序 | 55 | 缺 FULL orchestration 与 agent-output | 已读 fallback diagnostic、runner/hat-channel 源码和日志；未把它作为独立主因 |
| run 内 manager report 是否由同一 reporter activation 写入、是否曾被外部流程修改 | 50 | 缺 activation output 的完整记录 | 已对照主 events、accepted-transitions、tasks ledger；仅按 artifact 存在事实引用 |
