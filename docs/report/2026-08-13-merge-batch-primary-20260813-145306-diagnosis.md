---
title: builtin:merge-batch Loop `primary-20260813-145306` 运行链路诊断报告
date: 2026-08-13
type: diagnosis
loop_id: primary-20260813-145306
preset: builtin:merge-batch
run_dir: .
status: 运行中截面；首个 reviewer 因 isolated channel 为空被定向恢复一次，后续链路已正常推进
diagnostics_mode: MINIMAL
bundle: present
bundle_path: .ralph/diagnostics/2026-08-13T22-53-06/diagnosis-input.json
history_search: disabled
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: present
evidence_gaps: ["orchestration.jsonl 未生成；agent-output 未生成；loop 尚未终止"]
---

# builtin:merge-batch Loop `primary-20260813-145306` 运行链路诊断报告

> 生成时间：2026-08-13。诊断对象为当前 workspace 的 `.ralph/`。loop 在诊断时仍持有 `.ralph/loop.lock`，因此本报告只对已发生的阶段下结论。

## 0. 产物盘点

**execution_capabilities**：`["supervisor", "wave"]`。preset 使用 `execution_mode: isolated`；运行时日志显示拾取 `.ralph/supervisor.db`，但本次 bundle 的 capability 数组尚未被最终补全。该能力信息不影响本次 reviewer 重试结论。

| Tier | 路径 | 存在 | 行数 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` 指向 `.ralph/events-20260813-145306.jsonl` | 是 | 3 | 当前已含 `merge.start`、`merge.reviewed`、`merge.integrated` |
| S | `.ralph/events-history-20260813-145306.jsonl` | 是 | 1 | 含 bootstrap 的 `merge.start` |
| S | `.ralph/ledger.jsonl` | 是 | 2 | 已记录 iteration 2 的观察 |
| A | `.ralph/recovery.jsonl` | 是 | 2 | 1 条 `missing_event_gate`，1 条 doc-sync info |
| B | `.ralph/diagnostics/2026-08-13T22-53-06/runtime-trace.jsonl` | 是 | 11 | 已记录 reviewer 两次 activation、integrator 一次 activation |
| B | `.ralph/diagnostics/2026-08-13T22-53-06/feedback.jsonl` | 是 | 2 | reviewer 第一次 activation 的缺终态反馈 |
| B | `.ralph/supervisor.db` | 是 | — | 运行时存在 |
| C | `.ralph/merge/review.md` | 是 | — | reviewer 已落盘 |
| C | `.ralph/merge/integration.md` | 是 | — | integrator 已落盘 |

bundle 状态为 `present`；`runtime-trace.jsonl` 与 `feedback.jsonl` 均可读。由于没有 `orchestration.jsonl`，本次按 MINIMAL 处理，不能对 agent 的完整 tool-call 序列作强归因。

## 1. 结论摘要

### 1.1 健康度

- **判定**：部分偏离，但已自动恢复；loop 当时仍在继续，不能判定最终成功或失败。
- **重复次数**：`reviewer` 确实启动了两次；不是 `merge.reviewed` 被接受两次。
- **最高优先级根因置信度**：P1 = **85/100**。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 执行与 OPAC 是否合规？ | ⚠️ 部分合规 | 第一次 activation 没有业务事件；第二次提交了合法 `merge.reviewed`，但缺少完整 orchestration/agent-output 证据。 | 65 |
| Q2 | 基座机制是否生效？ | ✅ 生效 | 空 channel 被识别为 missing terminal，并定向恢复原 hat；第二次 `merge.reviewed` 被 accepted。 | 85 |
| Q3 | 编排是否合理？ | ✅ 拓扑合理 | preset 中 `reviewer: merge.start → merge.reviewed` 只有一条首跳，未发现重复 trigger/重复 publishes。 | 80 |
| Q4 | 归因是什么？ | mechanism 正常兜底 + agent 首次未产出事件；不是 preset 重复路由 | `runtime-trace` 的两次 reviewer activation 与 recovery envelope 一一对应。 | 85 |

### 1.3 根因一句话

第一次 `reviewer` activation 结束时，isolated hat channel 是空文件，runtime 按“有终态义务但没有 emit”处理，发布定向 `task.resume`/missing-event recovery，于下一 iteration 再启动同一个 `reviewer`；因此用户看到首个 hat 出现两次。**这是恢复重试，不是同一业务事件的重复分发。**

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| 首轮终态 | 首轮 `reviewer` 无有效事件，`empty_batch_commit/no_progress`；不是成功终态 |
| 恢复状态 | 第 2 iteration 定向恢复后，`merge.reviewed` accepted 并提交 |
| 最终代码状态 | 诊断时 loop 仍运行；已看到 `merge.integrated` accepted，尚未看到最终 reporter 终态 |
| 一致性告警 | 无“失败终态后恢复”证据；这是缺终态后的正常 recovery 路径 |

## 2. 执行链路

```text
merge.start
  → reviewer activation #1
      → isolated channel 0 bytes
      → missing_event_gate / missing_terminal_emit
  → reviewer activation #2（定向恢复）
      → merge.reviewed accepted
  → integrator activation #1
      → merge.integrated accepted
  → stabilizer（诊断时尚未完成）
```

关键原始证据：`runtime-trace.jsonl` 第 1–7 行；`.ralph/recovery.jsonl` 第 2 行；`.ralph/events-20260813-145306.jsonl` 第 2–3 行。

## 3. 历史问题上下文

本次 `history_search=disabled`，未读取主仓历史报告、solutions、plans 或 brainstorms；历史关联统一为 `N/A (history disabled)`。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | reviewer 第一次 activation 为空，随后定向重试 | `.ralph/diagnostics/2026-08-13T22-53-06/runtime-trace.jsonl:1-7`; `.ralph/recovery.jsonl:2` | P1 | 85 | 源码行号 +25；双账本 +20；总分按机制证据封顶前为 85 | 无 agent-output，无法判断空 channel 的具体 agent 行为 |
| DEV-002 | preset 首跳没有重复声明 | `presets/en/merge-batch.yml:44-55`; reviewer hat 的 `triggers/publishes` 结构 | P2 | 80 | preset 行级证据 +15；events 与 topology 对账 | 缺最终完整运行图 |

### 4.1 OPAC 逐 hat 审计表

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| reviewer #1 | ✅ | ⚠️ | ⚠️ 未产生事件 | ❌ 无终态 | empty channel + missing terminal | 70 |
| reviewer #2 | ✅ | ✅ | ✅ emit `merge.reviewed` | ✅ accepted | trace 第 4–7 行 | 80 |
| integrator #1 | ✅ | ✅ | ✅ emit `merge.integrated` | ✅ accepted | trace 第 8–11 行 | 80 |

## 5. 问题归因

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P1 | 首个 hat 在空 isolated channel 后被定向恢复一次 | mechanism 的恢复行为 + agent 首次无有效 emit；不是 preset 重复路由 | **85** | DEV-001 | `inner.rs:3662-3721` 将空 channel 标记为 missing terminal；`inner.rs:4667-4688` 定向恢复；`event_processing.rs:601-710` 构造同 hat 的 `task.resume`；runtime trace + recovery 双账本 | `N/A (history disabled)` | 第 1 轮：源码反查 + 双账本，40→85 |
| P2 | 多 preset 都可能出现相同“首 hat 两次”表象 | mechanism 共用路径 | **75** | DEV-001/002 | 共用 loop runner 源码 + 本 preset 实例证据；MINIMAL 模式封顶 | `N/A (history disabled)` | 第 1 轮：当前 run 证据；跨 preset 复发尚未做历史扫描 |

## 6. 修复建议

### 6.1 短期（operator workaround）

- 把首个 hat 的第一次空 activation 视为 recovery 信号，先看 `.ralph/recovery.jsonl` 的 `reason_code=missing_terminal_emit`；不要把后续成功 activation 当作重复业务事件。
- 若同一 hat 连续超过恢复预算仍为空，再停止并检查该 hat 是否真的执行了 `ralph emit`、是否在正确 workspace 中运行、以及 backend 是否提前退出。

### 6.2 中期（preset / instructions）

- 对首个 hat 的 instructions 增加“完成前必须产生且确认一个声明的终态事件”的可验证约束；这只能降低 agent 空输出概率，不能消除 runtime 的安全恢复。
- 运行结束后补充可消费的 agent-output/ orchestration 证据，区分“agent 没 emit”和“emit 写到了错误路径”。

### 6.3 长期（机制 / 底座）

- 统计所有 preset 的 `empty_terminal_channel`：记录 backend 退出状态、输出字节数、是否出现 emit 命令、channel 创建/删除时间，定位为什么首个 activation 更容易空。
- 若确认 agent 实际已 emit 但 channel 仍为空，再修 isolated channel 路由；当前证据只能确认 channel 为空，不能把它归因于 channel race。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| 第一次 reviewer 为什么没有写事件：agent 未调用 emit、backend 提前结束、还是 channel 写入路径异常 | 55 | 缺 `agent-output` / `orchestration.jsonl` | 已查 runtime trace、recovery、runner 源码和日志；未自动修改或重跑 |

