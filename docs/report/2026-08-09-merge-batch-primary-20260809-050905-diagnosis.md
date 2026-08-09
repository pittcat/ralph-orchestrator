---
title: merge-batch Loop `primary-20260809-050905` 运行链路诊断报告
date: 2026-08-09
type: diagnosis
loop_id: primary-20260809-050905
preset: builtin:merge-batch
run_dir: .
status: git 合并与全量验证已完成，但 preset 事件门禁阻断 formal completion；loop 已取消
diagnostics_mode: MINIMAL
history_search: disabled
execution_capabilities: [single-chain]
---

# merge-batch Loop `primary-20260809-050905` 运行链路诊断报告

> **生成时间**：2026-08-09
> **诊断对象**：`.ralph/` 当前可信运行产物；仅当前 loop，不扫描历史。
> **对照 preset**：`presets/en/merge-batch.yml` + `presets/schemas/merge-batch.yml`
> **Diagnostics 模式**：MINIMAL
> **报告仓库**：`ralph-orchestrator` 主仓
> **history_search**：`disabled`（用户明确要求不用历史）
> **execution_capabilities**：`[single-chain]`；preset 未启用 supervisor，events 无 `wave_id`，因此缺 `supervisor.db` / `wave_id` 均不构成故障。

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---|---|
| S | `.ralph/current-events` | 是 | 指向 `.ralph/events-20260809-050905.jsonl` | 唯一可信 events 文件 |
| S | `.ralph/events-20260809-050905.jsonl` | 是 | 5 行 | 当前 loop：start → reviewed → integrated → plan.blocked → loop.cancel |
| S | `.ralph/ledger.jsonl` | 是 | 8 行 | 4 次 iteration；记录 cancellation request |
| S | `.ralph/recovery.jsonl` | 是 | 13 行 | 当前 loop 含两次 `merge.stabilized` 语义拒收 |
| S | `.ralph/loops.json` | 是 | `loops: []` | loop 已退出 |
| S | `.ralph/loop.lock` | 是 | 文件存在 | 运行残留锁，需按 operator 流程确认/清理，不在本诊断中手改 |
| A | `.ralph/agent/tasks.jsonl` | 否 | N/A | preset 未启用 tasks |
| A | `.ralph/agent/summary.md` | 是 | 28 行 | 已生成，但只概括当前 5 个 accepted events |
| A | `.ralph/agent/handoff.md` | 否 | N/A | 取消路径未生成 |
| B | `.ralph/diagnostics/2026-08-09T13-09-05/` | 是 | MINIMAL | 有 trace/recovery/drift/summary，无 orchestration |
| B | `.ralph/supervisor.db` | 是 | N/A | 单链 capability 下不作为故障证据 |
| C | `.ralph/merge/integration.md` | 是 | 当前 SHA `c9afbda0` | 当前 activation 的 integrator 证据 |
| C | `.ralph/merge/merge-boundary.json` | 是 | `batch_head_sha=c9afbda0` | canonical digest 为 `e7023264…`，与 accepted `merge.integrated` 一致 |
| C | `.ralph/merge/review.md` | 是 | 当前 reviewer 产物 | review 证据 |
| C | `.ralph/merge/REPORT.md` | 是 | 19393 字节 | 内容仍描述旧 loop `034731` / `e02e2ac4`，不是当前 loop 的事实源 |

## 1. 结论摘要

### 1.1 健康度

- **判定**：部分偏离；git 层合并成功，正式稳定化/报告完成事件被 preset 门禁阻断。
- **P0**：0（没有证据表明生产代码或 merge 结果损坏）。
- **P1**：2（preset 门禁误触发；当前 `REPORT.md` 陈旧且会误导 operator）。
- **最高根因置信度**：P1-1 compound = **75/100**；其中 mechanism = 85，preset = 75。
- **历史关联**：`N/A (history disabled)`。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 执行与 OPAC 是否合规？ | ⚠️ 业务执行成功；stabilized emit 被 policy gate 拒收，loop.cancel 成功 | 当前 events 第 3 行为 `merge.integrated`，第 5 行为 `loop.cancel`；recovery 第 12–13 行为 `merge.stabilized` 拒收 | 75 |
| Q2 | 基座机制是否生效？ | ✅ 大体生效 | scope handoff gate 对 path/digest 做了结构校验；拒收被记录到 recovery；控制 topic `loop.cancel` 可用 | 80 |
| Q3 | 编排是否合理？ | ❌ 关键 gate 配置不合理 | `merge.stabilized` 需要 echo 前序事件字段，但 `payload_consistency` 只评估当前 payload，无法表达跨事件比较 | 85 |
| Q4 | 归因是什么？ | compound：preset 规则错误 + evaluator 能力边界未被配置约束 | preset 行 138–144 与 evaluator 源码行 1–12、180–233 共同证明 | 75 |

### 1.3 根因一句话

`merge-stabilized-boundary-echo` 把“字段存在且非空”配置成了违规条件；因此合法的 `merge.stabilized` payload 必然命中 fail-close 规则，stabilizer 无法完成，reporter 没有机会发布 `merge.batch.complete`。

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| **首轮终态** | `merge.integrated` 已 accepted，随后 formal stabilization 未形成 accepted 事件；loop 进入 `plan.blocked`，最终 accepted `loop.cancel`。 |
| **恢复状态** | 无成功恢复；后续只接受了 `loop.cancel`，没有 accepted `merge.stabilized` / `merge.batch.complete`。 |
| **最终代码状态** | 当前分支 `pittcat-dev` HEAD 为 `c9afbda0`，工作树干净；source worktree 仍存在，分支仍保留。 |
| **一致性告警** | ⚠️ 当前 `.ralph/merge/REPORT.md` 是旧 activation 的报告：其中 `e02e2ac4`、`events-20260809-034731.jsonl` 与当前 `c9afbda0`、`events-20260809-050905.jsonl` 不一致。不可用它覆盖当前 accepted event verdict。 |

## 2. 当前执行链路

```text
merge.start
  → merge.reviewed (reviewer)
  → merge.integrated (integrator, c9afbda0, verification pass)
  → merge.stabilized (stabilizer, 两次 CLI emit 被拒收)
  → plan.blocked (控制事件，业务 scope 不允许 ralph 发布)
  → loop.cancel (ralph control topic，accepted)
```

当前 `merge.integrated` payload 已包含：`integration_complete=true`、`ready_for_stabilization=true`、`merge_boundary_path`、`merge_boundary_digest=e7023264…`、`merge_boundary_status=complete`。`.ralph/merge/merge-boundary.json` 的 canonical digest 与该值一致，当前 run 没有旧报告中记录的 `e02e2ac4` / `7438…` digest 情况。

## 3. 历史问题上下文

`N/A (history disabled)`

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | `merge.stabilized` 的 payload consistency 规则必然命中 | `presets/en/merge-batch.yml:138-144`；`.ralph/events-20260809-050905.jsonl:4`；`.ralph/recovery.jsonl:12-13` | P1 | 75 | preset 行号 +15；双账本 +20 | 无 FULL agent-output；跨事件比较需专门机制支持 |
| DEV-002 | 当前 REPORT 与 current-events 不一致 | `.ralph/merge/REPORT.md:7,27,34`；`.ralph/events-20260809-050905.jsonl:3-5`；`.ralph/merge/integration.md:5,15,46` | P1 | 75 | Tier C 交叉验证 +10；双产物账本 +20；当前事件锚点 | 缺 reporter accepted completion event |
| DEV-003 | `ralph` 尝试发布 `plan.blocked` 被 isolated scope 拒绝 | `.ralph/diagnostics/2026-08-09T13-09-05/recovery.jsonl:2` | P2 | 60 | recovery 记录 + 当前事件链 | 缺完整 orchestration；不影响最终 loop.cancel |

### 4.1 OPAC 逐 hat 审计表

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| reviewer | ✅ | ⚠️ | ✅ | N/A | current events 已有 `merge.reviewed`；无 agent-output | 50 |
| integrator | ✅ | ✅ | ✅ | N/A | `merge.integrated` accepted，integration.md 与 boundary 均指向 `c9afbda0` | 65 |
| stabilizer | ✅ | ⚠️ | ❌ | N/A | recovery 第 12–13 行明确 `merge.stabilized` 被拒收；无 FULL tool-call 序列 | 70 |
| ralph | ✅ | ✅ | ✅ | N/A | `loop.cancel` accepted；`plan.blocked` scope violation 被 recovery 记录 | 65 |

> MINIMAL 模式没有完整 orchestration/agent-output，Confirm 只能以 current events/recovery 弱确认；不据此单独判定 agent 违规。

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P1 | `merge-stabilized-boundary-echo` 将合法 stabilized payload fail-close | compound（preset 60% + mechanism 40%） | **75** | DEV-001 | mechanism：源码行号 +25、events/recovery 双账本 +20 = 85；preset：preset 行号 +15、双账本 +20 = 75 | `N/A (history disabled)` | 第 1 轮源码反查；第 2 轮双账本对账 |
| P1 | `REPORT.md` 没有随当前 activation 更新，仍引用旧 SHA/旧 events 文件 | agent/artifact handoff | **75** | DEV-002 | Tier C 产物交叉验证 +10；当前 events 与 integration.md 双账本 +20 | `N/A (history disabled)` | 第 1 轮 current-events 与 artifact 对账 |

## 6. 修复建议

### 6.1 短期（operator workaround）

1. 不要重用当前 `REPORT.md` 作为本轮结论；以本报告、`events-20260809-050905.jsonl`、`integration.md`、`merge-boundary.json` 为准。
2. 若要重跑，应先修正或临时移除 `merge-stabilized-boundary-echo`，再启动 `merge-batch`；不要手工伪造 `merge.stabilized` 或 `merge.batch.complete`。
3. 按 operator 流程确认 `.ralph/loop.lock` 是否为残留锁，并决定是否清理 source worktree/branch；本诊断未修改这些状态。

### 6.2 中期（preset/schema）

将 `presets/en/merge-batch.yml:138-144` 改成真正表达“echo 一致性”的结构化 gate；当前 `payload_consistency` 只能做同一 payload 的 `exists/non_empty/eq/...` 判断，不能读取 `merge.integrated` 触发 payload。若已有 `scope_handoff_guard` 能完成 path 可读性、作用域和 digest 校验，则删除该误配规则，并同步 schema/结构化测试。

### 6.3 长期（机制）

若产品确实需要跨事件 echo 校验，应新增明确的 stateful/event-pair gate：读取已 accepted 的 `merge.integrated` 状态，再比较 `merge.stabilized` 的 path/digest；不要把跨事件语义伪装成 `payload_consistency`。

## 7. 未核实疑点

无须影响当前根因结论的未核实疑点。MINIMAL 模式下无法确认 stabilizer agent 的完整 tool-call 顺序，但拒收原因已经由 recovery + 当前终态事件充分锚定为门禁/配置问题。

