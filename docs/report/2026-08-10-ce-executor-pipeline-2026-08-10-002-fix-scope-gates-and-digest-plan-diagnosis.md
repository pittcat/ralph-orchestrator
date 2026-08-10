---
title: ce-executor-pipeline Loop `2026-08-10-002-fix-scope-gates-and-digest-plan` 运行链路诊断报告
date: 2026-08-10
type: diagnosis
loop_id: 2026-08-10-002-fix-scope-gates-and-digest-plan
preset: presets/en/ce-executor-pipeline.yml
run_dir: ../worktree/ralph-orchestrator/2026-08-10-002-fix-scope-gates-and-digest-plan
status: 已定位：裸 work.done 绕过 isolated hat-channel/precheck 后触发 scope warning；非 P0
diagnostics_mode: MINIMAL
history_search: disabled
---

# ce-executor-pipeline Loop `2026-08-10-002-fix-scope-gates-and-digest-plan` 运行链路诊断报告

> **生成时间**：2026-08-10
> **诊断对象**：主仓与目标 worktree 的 `.ralph/` 中间产物；以目标 worktree 的 loop identity、主 events、channel-routing diagnostics、session recovery、accepted transitions、reuse-history 和 review artifacts 交叉核对。
> **对照 preset**：`presets/en/ce-executor-pipeline.yml` + `presets/schemas/ce-executor-pipeline.yml`
> **Diagnostics 模式**：`MINIMAL`
> **历史检索**：`disabled`；未读取主仓历史报告、solutions、plans 或 brainstorms；仅读取目标 loop 自己的 reuse-history 中间产物。
> **execution_capabilities**：`[supervisor]`
> **报告仓库**：`ralph-orchestrator` 主仓。

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---|---|
| S | `.ralph/current-events` | 是 | 指向 `events-20260810-133303.jsonl` | 唯一可信主 events 指针 |
| S | `.ralph/events-20260810-133303.jsonl` | 是 | 4 行 | `work.start`、`plan.ready`、`work.done`、后续裸 `work.done.proposed` |
| S | `.ralph/events-history-20260810-133303.jsonl` | 是 | 1 行 | 旁路历史，不作为主拓扑 SSOT |
| S | `.ralph/ledger.jsonl` | 是 | 2 行 | 两次 `loop.batch_sync` |
| S | `.ralph/recovery.jsonl` | 否 | 0 行 | 无 workspace recovery 主文件 |
| S | `.ralph/loops.json` / `current-loop-id` | 是 | loop id 已确认 | 无 `loop.lock`，锁已释放 |
| A | `.ralph/agent/tasks.jsonl` | 是 | 0 行 | preset `tasks.enabled: false`，属预期 |
| A | `.ralph/agent/accepted-transitions.jsonl` | 是 | 7 行 | durable outbox；不等同于主 events |
| B | `.ralph/diagnostics/2026-08-10T21-33-03/` | 是 | MINIMAL | 有 `recovery.jsonl`、`drift.jsonl`，无 orchestration/agent-output |
| B | `.ralph/supervisor.db` | 是 | runtime ledger | `event_loop.supervisor.enabled: true` |
| B | executor hat-channel iter 3 | 是 | 0 行 | 当前 marker 指向该空 channel |
| B | 三份 channel-routing fallback | 是 | 3 个诊断文件 | plan-reviewer、executor iter 2/3 均发生空 channel fallback |
| C | `.ralph/review/<plan>/` | 是 | 6 个主要 review artifact | 含 baseline/final verification、trace、normalized plan |
| C | `.agents/scratchpad/` | 未作为缺失判定 | preset 声明 `core.specs_dir` | 本次 flow-audit skip 路径不要求新增业务 artifact |

**能力推断**：`event_loop.supervisor.enabled: true`（preset 约第 65 行）且 `.ralph/supervisor.db` 存在，因此为 `supervisor`。preset 未使用 wave fan-out 指令，主 events 也无 `wave_id`；没有 wave 能力不构成故障。

**诊断盲区**：MINIMAL 没有 `orchestration.jsonl` 和 `agent-output.jsonl`，因此无法逐 tool-call 证明 agent 是否在 emit 前执行了 `--policy-check`；OPAC/agent 结论最高按模式降级。

## 1. 结论摘要

### 1.1 健康度

- **判定**：事件链已完成，但写入路径违反 isolated provenance 契约；不是“work.done 尚未写入”的死锁。
- 主 events 已接受 `work.done`，其 payload 声称 U1–U5 完成且验证为 green。
- 随后 runtime 对该终态记录再次执行 isolated scope 检查，产生 `isolated_scope_violation`；恢复 `task.resume` 被投递到内存 bus/目标 hat，但没有追加到主 events 文件。14:08 的裸 `work.done.proposed` 是后续重试中的独立污染，不是 13:51 告警的根因。
- **P0/P1/P2**：P0=0，P1=1（confidence 95），P2=0。P1 是事件写入路径与 isolated provenance/precheck 契约偏离，不否定已接受的 `work.done`。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 编排完成；OPAC 部分不可证 | 主 events 三行完成链；MINIMAL 无 agent-output，无法确认每次 emit 的 precheck | 70 |
| Q2 | 基座机制是否正常生效？ | ⚠️ scope guard 与 recovery 机制生效，但终态 replay 产生误导性 warning | `parse_and_emit.rs:610-720` 拒收并写 recovery；`recovery.jsonl` 记录 escalated | 82 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ 业务链合理完成；isolated + precheck + CLI 直写的交接不一致 | preset executor 声明 `work.done`，precheck desugar 后 producer 使用 proposed 变体；最终事件无 provenance | 80 |
| Q4 | 问题归因是什么？ | compound：agent/emit 路径来源缺失触发 mechanism 的终态 replay scope warning | 主 events 无 `hat/source/triggered`；runtime 对无来源事件 fallback 到 current isolated hat | 85 |

### 1.3 根因一句话

`work.done` 以没有 `hat/source/triggered` 的裸记录直接写入目标 worktree 主 events，绕过了 executor 的 per-hat channel 和 `work.done → work.done.proposed` precheck；随后 isolated runtime 在 executor scope 下重读这条裸终态，按当前 publish surface 拒绝 `work.done`，产生 `isolated_scope_violation`。scope guard 是按契约拦截，不是根因。

**置信度：95%。** 已确认“裸事件已落盘→isolated 重读→scope drop”的完整链；未确认更下游的具体 shell/tool invocation（例如哪一层清理了环境），该子因不纳入高置信度结论。

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| **首轮终态** | 主 events 第 3 行在 `13:50:48` 接受 `work.done`；因此首轮是成功终态，不是证据不足。 |
| **恢复状态** | `13:51:21` 产生 scope warning；`14:09:43` 重发 `work.done.proposed`，`14:15:06` drift monitor 标记 `Recovered`，主 events 随后出现 `work.done`。属于“首轮错误终态写入后由恢复轮修正”，不是整条 pipeline 被阻断。 |
| **最终代码状态** | review artifact 声称 U1–U5 在 HEAD，工作树非 `.ralph/` 路径干净；本报告不以 artifact 覆盖事件事实。 |
| **一致性告警** | ⚠️ 首轮裸 `work.done` 触发 scope-drop；`task.resume` 出现在 `accepted-transitions`/flow authority，而不在 trusted main events；恢复轮最终完成 `work.done`。 |

## 2. 执行链路对比

### 2.1 拓扑激活表

| Hat | 预期触发 | 预期终态 | 本次证据 | 判定 |
|---|---|---|---|---|
| plan-reviewer | `work.start` | `plan.ready` | 主 events 第 2 行 | ✅ |
| executor | `plan.ready` | `work.done` / `work.failed` | 主 events 第 3 行 `work.done` | ✅ |
| precheck-work.done | `work.done.proposed` | `work.done` / rejected | recovery/accepted transitions 有 proposed→done 痕迹；主 events 只保留最终 done | ⚠️ |
| 六个 dimension hats 及后续 review/fix/alignment/reporter | `work.done` 后链路 | `report.done` / `LOOP_COMPLETE` | 主 events 未出现 | ⏸️ 未在当前主账本触发 |

### 2.2 可信事件时间轴

| 顺序 | 时间 | 事件 | 证据与解释 |
|---:|---|---|---|
| 1 | 13:33:03 | `work.start` | loop bootstrap |
| 2 | 13:38:42 | `plan.ready` | plan-reviewer 结果进入主 events |
| 3 | 13:50:48 | `work.done` | payload 含 5 个 completed Units、`execution_status=complete` |
| 4 | 13:51:21 | scope warning | 只在 MINIMAL session recovery/log 中出现，未成为业务 events |

`accepted-transitions.jsonl` 中的 `task.resume`（第 7 行）是 durable transition/outbox 记录；源码 `parse_and_emit.rs:974-1031` 同时说明恢复事件发布到 bus，并将合成事件仅推入本轮 `accepted` 集合用于 `had_events`，不是主 events 持久化。

## 3. 历史问题上下文

`history_search: disabled`。按诊断开关纪律，本次未扫描主仓历史文档；历史关联字段统一为 `N/A (history disabled)`。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | accepted `work.done` 后 isolated replay 产生 scope warning，恢复信号不进入主 events | `.ralph/events-20260810-133303.jsonl:3`；session recovery:2-3；log:18-20 | P1 | 70 | 主账本 + session recovery + logs（+30）；源码行号（+25） | 无 orchestration；无 agent-output |
| DEV-002 | executor 使用 CLI 直写的终态事件缺少 provenance 字段，触发 replay fallback 语义 | 主 events:3 的 `hat/source/triggered=null`；`event_origin.rs:239-290`；`parse_and_emit.rs:605-610` | P1 候选 | 65 | 事件字段 + 源码（+40） | 无 CLI invocation 原始日志，无法确认实际参数 |
| DEV-003 | prompt visibility 对账未包含 `ralph-tools-precheck` 自动注入，而是 on-demand | `inspect prompt --hat executor --format json` 输出 | P2/Q3 | 55 | inspect JSON（+25）；preset instructions 要求加载 emit/opac（+15） | 未证明 agent 是否实际加载 skill；MINIMAL 无 agent-output |

### 4.1 OPAC 逐 hat 审计表（MINIMAL）

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| plan-reviewer | ✅ | ⚠️ | ✅ | ⚠️ | 主 events 有 `plan.ready`；空 channel fallback；无 agent-output | 65 |
| executor | ✅ | ⚠️ | ⚠️ | ⚠️ | logs 有 `work.done` scope drop；无法确认 emit 前 policy-check；主 events 已有 done | 70 |

MINIMAL 模式下，Precheck 只能由 recovery/events/logs 弱推断，不能因看不到 tool call 单独判定 OPAC P0。Confirm 对合成 `task.resume` 不走主 events 持久化，按当前源码属于 targeted bus recovery 路径。

### 4.2 Prompt visibility 对账

`inspect prompt --hat executor --format json` 成功输出：

- `auto_inject`: `ralph-tools`、`ralph-tools-memories`、`ralph-tools-opac`
- `on_demand`: `ralph-tools-cmdref`、`ralph-tools-emit`、`ralph-tools-precheck`、`ralph-tools-recovery-directives`、`ralph-tools-tasks`、`ralph-tools-wave`
- `block_titles` 包含 `收到 task.resume 时`、`Precheck 阶段关键命令`、`Confirm 阶段通用规则`。

因此 executor 不能把 `ralph-tools-precheck` 视为自动注入；若需要其细节，必须显式 `ralph tools skill load ralph-tools-precheck`。本项因缺 agent-output 保留为低置信度疑点，不进入 §5。

## 5. 问题归因表（仅保留 confidence ≥ 85）

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P1 | 裸 `work.done` 绕过 isolated channel/precheck 写入主 events，随后被 isolated scope guard 拒绝 | 事件写入路径与运行时 scope 契约不一致 | **95** | DEV-001、DEV-002 + 目标 worktree 中间产物 | 目标 events 裸字段；13:39/13:51/14:09 三次空 channel；源码显示非空 channel 才盖章；recovery 精确匹配同一 topic | N/A (history disabled) | 2→95 |

## 6. 修复建议

本报告不修改代码。以下是针对已入表 P1 的后续建议：

### 6.1 短期（operator workaround）

- 目标：避免把已接受 terminal event 当成缺失事件重复提交。
- 动作：先以 `ralph events --events-source main` 核对 terminal chronology；若已有 `work.done`，不要再次发送 `work.done` 或 `work.done.proposed`。将后续处理交给 loop/operator，除非主账本确实没有 terminal event。
- 关联置信度：85。

### 6.2 中期（preset / emit contract）

- 目标：让 agent 产生的业务事件保留明确 producer provenance，并与 precheck proposed surface 一致。
- 动作：审计 executor instructions 与 CLI emit wrapper 的参数契约，明确 agent 应使用带 hat/provenance 的 emit 路径；为“已 accepted terminal event 被 replay”增加真实 runtime 场景，断言不会把成功终态误报成可重试缺失。
- 关联置信度：80。

### 6.3 长期（机制 / 底座）

- 目标：终态事件的路由验证与恢复信号持久化语义一致。
- 动作：评估在 main events 已存在 accepted terminal event 后，replay 是否应跳过 producer scope revalidation，或将 replay 事件补齐原始 provenance；同时明确 `task.resume` 是 bus-only recovery 还是应进入某个 operator-visible ledger，避免 accepted-transitions、flow-authority 与 main events 三者被误读为同一账本。
- 关联置信度：85。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| CLI emit 是否因缺少显式 `--hat executor` 导致 provenance 丢失 | 55 | MINIMAL 无原始 agent-output/命令 tool trace | 已核对 main event 字段与 `event_origin`/precheck 源码；未凭空定论 |
| `inspect prompt` 的 on-demand precheck 是否被 agent 正确加载 | 45 | MINIMAL 无 agent-output | 已完成 JSON visibility 对账；未把缺失加载证据写成 P1 |

## 8. 机制生效矩阵

| 机制 | 判定 | 证据 |
|---|---|---|
| Origin guard / hat scope | ⚠️ 生效但 replay 产生 warning | `parse_and_emit.rs:605-720`；session recovery `isolated_scope_violation` |
| Payload contract | ✅ 未见拒收 | 主 events `work.done` payload 完整；无 payload recovery |
| Execution contract | ✅ 产物与 payload 自称一致 | final verification；但终态业务链未继续到 reporter |
| Workflow guard | ✅ 未见乱序拒收 | 主 events 顺序正确 |
| Isolated 单事件预算 | ⚠️ channel fallback 使路径降级 | 两份 `channel-routing-fallback`；日志 10-12、18-20 |
| step_handoff / semantic gate | N/A | `tasks.enabled=false`；无 tasks 业务流程 |
| Recovery 升级 | ✅ scope warning 为 escalated；后续 drift 更新 pending | session recovery 第 2-3 行 |
| loop.resume / task.resume 消费者 | ⚠️ targeted bus 有消费者路径；主 events 不持久化 | `parse_and_emit.rs:974-1031`；accepted transition 第 7 行 |
| Stall / stale | ⚠️ 曾有 `missing_event_gate` recovery，最终为 recovered | workspace recovery 文件名与 payload |
| Drift monitor | ✅ 有 session `drift.jsonl` | session drift 文件存在；无 orchestration |
| Dedup | ✅ scope retry key 有运行时 dedup 设计；本次无重复主 event | `parse_and_emit.rs:739-761`；主 events 仅 3 行 |
| Terminal / silent-success | ⚠️ terminal 已接受，但后继 reporter 链未在主账本出现 | 主 events 第 3 行；无 `report.done`/`LOOP_COMPLETE` |
| Event-artifact temporal consistency | ⚠️ accepted terminal 后出现 recovery，不能用最终 artifact 反写 chronology | events:3 > session recovery:2 |

## 9. 关键主仓代码引用清单

- `crates/ralph-core/src/event_loop/parse_and_emit.rs:605-610`：以事件自身 `hat` 为 scope anchor，无 `hat` 时 fallback 到当前 isolated hat。
- `crates/ralph-core/src/event_loop/parse_and_emit.rs:610-720`：不在 hat publish scope 的业务 topic 被 drop，并写 `isolated_scope_violation` recovery。
- `crates/ralph-core/src/event_loop/parse_and_emit.rs:974-984`：scope-drop recovery 通过 targeted resume routing 发布到目标 hat。
- `crates/ralph-core/src/event_loop/parse_and_emit.rs:987-1031`：合成 `task.resume` 只 push 到本轮 `accepted`，用于 `had_events`，不等于写入主 events。
- `crates/ralph-core/src/event_origin.rs:239-290`：已知 current isolated hat 下无 provenance 的事件不判 anonymous，而交给 scope fallback。
- `crates/ralph-core/src/config/precheck.rs:219-291`：无 hat identity 时不做 proposed rewrite；只有 producer 已发布 `<topic>.proposed` 才改写。
- `presets/en/ce-executor-pipeline.yml:2212-2219`：executor 原始配置声明 `work.done`/`work.failed`，运行时 precheck 可能改写 producer surface。
- `presets/schemas/ce-executor-pipeline.yml:work.done`：`work.done` required fields 与 terminal payload contract。
