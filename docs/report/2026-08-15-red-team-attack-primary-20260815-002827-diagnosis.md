---
title: builtin:red-team-attack Loop `primary-20260815-002827` 运行链路诊断报告
date: 2026-08-15
type: diagnosis
loop_id: primary-20260815-002827
preset: builtin:red-team-attack
run_dir: .
status: 正常完成，但实验队列 handoff 的计数与重复事件存在编排偏离
diagnostics_mode: MINIMAL
bundle: finalized
bundle_path: .ralph/diagnostics/2026-08-15T08-28-27/diagnosis-input.json
history_search: disabled
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: present
evidence_gaps: ["缺少 orchestration.jsonl 与 agent-output.jsonl，OPAC 只能按 MINIMAL 模式审计"]
---

# builtin:red-team-attack Loop `primary-20260815-002827` 运行链路诊断报告

> **生成时间**：2026-08-15
> **诊断对象**：`.ralph/`，可信 events 文件由 `.ralph/current-events` 指向 `.ralph/events-20260815-002827.jsonl`
> **对照 preset**：`presets/en/red-team-attack.yml` + `presets/schemas/red-team-attack.yml`
> **历史检索**：disabled；本报告只使用本次 loop 产物。
> **执行能力**：`[single-chain]`；没有 `supervisor.enabled: true`、wave 指令或 `wave_id`，`.ralph/supervisor.db` 对本次能力不是必需品。

## 0. 产物盘点

| Tier | 路径 | 存在 | 行数/数量 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` → `events-20260815-002827.jsonl` | 是 | 55 行 | 唯一可信业务事件账本 |
| S | `.ralph/ledger.jsonl` | 是 | 106 行 | 有 ledger 观察记录 |
| S | `.ralph/recovery.jsonl` | 是 | 10 行 | 含本次运行前后多个拒收/恢复记录，需结合 session recovery 辨别 |
| S | `.ralph/loops.json` / `.ralph/current-loop-id` | 是 | — | loop registry 同时出现其它运行记录；本次诊断以 current-events 指针和 loop_id 锁定，不用 registry 反推业务事件 |
| A | `.ralph/agent/summary.md` | 是 | 33 行 | `Completed successfully`，53 iterations，4h05m15s |
| A | `.ralph/agent/handoff.md` | 是 | 42 行 | `Session completed successfully` |
| B | `.ralph/diagnostics/2026-08-15T08-28-27/` | 是 | 9 个文件 | bundle `finalized`，runtime trace 214 行，feedback 2 行，drift 0 行 |
| B | `orchestration.jsonl` / `agent-output.jsonl` | 否 | — | 因此为 MINIMAL，不对每次 tool call 作强结论 |
| B | `.ralph/supervisor.db` | 是 | 139264 bytes | 单链能力下不作为异常信号 |
| C | `.ralph/red-team/experiments/RTE-*.md` | 是 | 22 个 | 与 `attack.mapped.experiment_count=22` 一致 |
| C | `.ralph/red-team/evidence/RTE-*` | 是 | 22 个目录 | 每个实验均有证据目录 |
| C | `.ralph/red-team/findings/RTF-*.md` | 是 | 21 个 | 与最终 `finding_count=21` 一致 |
| C | `07-evidence-board.md` / `08-impact-boundary.md` / `PLAN.md` / `REPORT.md` / `QUESTIONS.md` | 是 | — | 队列汇总、影响边界、人工确认材料均已生成 |

## 1. 结论摘要

### 1.1 健康度

- **判定：部分偏离，但未假闭环。** 主链确实完成了目标工作：22 个实验执行完，21 个 qualified，1 个 rejected，影响边界和独立审查均执行，最后才发出 `redteam.complete(success=true)`。
- **主要偏离：** `redteam.experiment.next` 中段出现重复业务事件和错误的队列计数。该偏离没有改变本次实验实际覆盖结果，但说明队列 handoff 的结构化一致性没有被可靠地门禁住。
- **问题数量：** 本次 red-team 业务结果为 21 个 Finding（报告中标记 P0 9、P1 9、P2 3）；本诊断新增 1 个编排层 P1 发现。
- **最高优先级根因置信度：** `DIAG-001 = 85/100`。MINIMAL 模式封顶 85；已有 preset 行号、可信 events 和 recovery 三方证据，但没有 agent-output 逐次 tool-call 记录。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 编排结果合规，OPAC 证据不完整 | policy 为 enforce、`on_violation=reject_with_resume`；session 为 MINIMAL，缺 `agent-output.jsonl`，无法逐条确认每次 `policy-check` | 70 |
| Q2 | 基座机制是否正常生效？ | ✅ 基本生效 | scope gate 曾拒收缺 `scope_status`/占位 SHA；最终只接受完整 `plan.resolved`；终态顺序正确且 lock 已释放 | 80 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ 主拓扑正确，队列 handoff 有偏离 | `target-locker → plan-resolver → attack-surface-mapper → experiment-runner/evidence-gate` 串行推进，之后进入 impact-boundary → independent-reviewer → reporter；但 `events` 第 38、40、42、44、46、50 行的计数与预期不一致，且第 30–32 行重复 `RTE-013` | 85 |
| Q4 | 问题归因是什么？ | compound：agent payload 质量 + preset/schema 缺少跨事件一致性门禁 | evidence-gate 的 instructions 要求精确计数，但 schema 只约束字段存在、非负/单调和 `remaining_count != 0`，未约束 `completed=accepted+rejected`、`completed+remaining=total` 或重复 `next` | 85 |

### 1.3 根因一句话

实验队列本身按 `RTE-001 → RTE-022` 完成了，但 evidence-gate 发出的中间 `redteam.experiment.next` payload 在 RTE-016 前后出现计数跳变、重复 dispatch 和 repair-stream 记录；这是 **agent 生成 payload 与 preset/schema 结构约束不足的 compound 编排问题**，不是 red-team 业务链路没有跑完。

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| 首轮终态 | **首轮成功**：`redteam.evidence.gated` → `redteam.plan.ready` → `redteam.reviewed(PLAN_READY)` → `redteam.complete(success=true)`，对应可信 events 第 52–55 行 |
| 恢复状态 | **运行中有局部恢复，但没有失败终态后的恢复**：session recovery 记录了若干 `repair_dispatch`，最终仍由 accepted business events 完成收敛 |
| 最终代码状态 | tracked tree 保持干净；preset 明确禁止生产代码、正式测试和 Git 历史修改。本次运行未产生新的 tracked diff/commit，只生成 `.ralph/red-team/` 产物 |
| 一致性告警 | 中间队列计数和重复 `next` 与最终 evidence board 不一致；最终 board 自洽为 22 total / 21 qualified / 1 rejected / 0 remaining |

## 2. 编排链路对照

```mermaid
flowchart LR
  A[redteam.start] --> B[target-locker\nredteam.target.locked]
  B --> C[plan-resolver\nredteam.plan.resolved]
  C --> D[attack-surface-mapper\nredteam.attack.mapped]
  D --> E[experiment-runner\nRTE-001..022]
  E --> F[evidence-gate\naccept/reject + next]
  F -->|remaining > 0| E
  F -->|queue exhausted| G[redteam.evidence.gated]
  G --> H[impact-boundary\nredteam.plan.ready]
  H --> I[independent-reviewer\nredteam.reviewed]
  I --> J[reporter\nredteam.complete]
```

### 2.1 预期与实际对照

| 编排预期 | 实际证据 | 结论 |
|---|---|---|
| 单链、隔离执行 | preset `event_loop.execution_mode=isolated`，可信 events 没有 wave fan-out | ✅ |
| plan 未解析完成前不得进入 attack surface | 先有 `redteam.plan.resolved`，再有 `redteam.attack.mapped`；此前 recovery 有 scope gate 拒收 | ✅ |
| 每次只执行一个 RTE | 22 个 `redteam.experiment.done`，22 个实验产物目录，未发现跳过 RTE | ✅ |
| rejected 实验不阻断后续队列 | RTE-001 rejected 后仍发 `next=RTE-002`，最终 board 保留 1 rejected | ✅ |
| `experiment.next` 每次只发一个且计数准确 | RTE-013 出现 3 次相同 `next`；RTE-016 前后计数从 14/8 跳为 16/6；后续多处仍偏移 | ❌ |
| 队列耗尽后才进入 impact boundary | `evidence.gated` 在所有 22 个实验后才出现 | ✅ |
| impact boundary → independent review → reporter | events 第 53–55 行严格按该顺序 | ✅ |
| 只发现问题，不修改生产代码 | summary、report、实验 state_after 均显示代码树干净；最终无 commit 产生 | ✅ |

## 3. 历史问题上下文

`history_search=disabled`。本报告不读取 `docs/report/`、`docs/solutions/`、`docs/plans/` 或 `docs/brainstorms/` 作历史关联。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DIAG-001 | `redteam.experiment.next` 队列 handoff 计数跳变，并出现重复 next 事件 | `.ralph/events-20260815-002827.jsonl:30-32,38,40,42,44,46,48,50`；`.ralph/.ralph/recovery.jsonl:3-10`；`presets/en/red-team-attack.yml:756-762`；`presets/schemas/red-team-attack.yml:289-320` | P1 | 85 | file/event +25；双账本 +20；preset/schema 行号 +15；Tier C board 交叉验证 +10；MINIMAL 封顶 | 缺 agent-output；没有逐次 tool-call 与 precheck 原始记录 |
| DIAG-002 | 运行中若干 scope / terminal / queue 事件进入 repair-stream 后才完成 accepted 路径 | `.ralph/recovery.jsonl:1-3`；`.ralph/.ralph/recovery.jsonl:1-10`；可信 events:52-55 | P2 | 75 | 双账本 +20；preset/schema 行号 +15；Tier C 交叉验证 +10；MINIMAL 封顶 | recovery 的 repair payload 是摘要，无法只凭它区分 agent 重试、runtime 重放还是 policy-check 修复 |

### 4.1 OPAC 逐 hat 审计表

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| target-locker | ✅ | ⚠️ | ✅ | ✅ | target.locked accepted；缺 agent-output，无法逐次确认 policy-check | 70 |
| plan-resolver | ✅ | ✅ | ✅ | ✅ | recovery 明确记录 scope gate 拒收；最终 plan.resolved accepted | 80 |
| attack-surface-mapper | ✅ | ⚠️ | ✅ | ✅ | predecessor_event 与 plan.resolved 顺序正确；无 tool-call 证据 | 70 |
| experiment-runner | ✅ | ⚠️ | ✅ | ✅ | 22 个 done 事件、22 个证据目录、代码树干净；无 tool-call 证据 | 70 |
| evidence-gate | ✅ | ⚠️ | ⚠️ | ✅ | queue next 重复且计数漂移；session recovery 有 repair_dispatch；最终 aggregate 自洽 | 75 |
| impact-boundary | ✅ | ⚠️ | ✅ | ✅ | 21 findings、影响边界测试结果和 plan.ready 均存在 | 75 |
| independent-reviewer | ✅ | ⚠️ | ✅ | ✅ | 19/19 audit PASS，reviewed=PLAN_READY | 75 |
| reporter | ✅ | ⚠️ | ✅ | ✅ | report/plan/questions 均存在，complete success=true | 75 |

> MINIMAL 模式说明：O/A/C 主要由可信 events 和 Tier C 产物确认；P 只能依据 recovery、preset 和最终事件弱审计，不能把缺少 `agent-output.jsonl` 直接判为 OPAC 违规。

## 5. 问题归因

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P1 | evidence-gate 发出的 `experiment.next` 不是严格的一次性、计数一致的队列 handoff：RTE-013 重复 3 次；RTE-015 完成后直接出现 completed=16、accepted=14、rejected=1、remaining=6，违反 `completed=accepted+rejected` 与 `completed+remaining=22` | compound：agent + preset/schema | 85 | DIAG-001 | events +25；recovery +20；preset/schema 行号 +15；Tier C board +10；MINIMAL 封顶 | N/A (history disabled) | 2 轮：第 1 轮读 preset/schema；第 2 轮对账 events + recovery + board |
| P2 | 运行过程中发生过 scope / queue / terminal 的 repair-stream 记录，说明部分 emit 没有一次通过当前契约 | mechanism / agent，暂不区分 | 75 | DIAG-002 | recovery +20；preset/schema +15；Tier C +10；MINIMAL 封顶 | N/A (history disabled) | 2 轮：recovery 与 accepted events 对账；仍缺 agent-output |

## 6. 修复建议

### 6.1 短期（operator）

- 采用本次最终 `07-evidence-board.md` 作为实验覆盖事实：22 total、21 qualified、1 rejected；不要使用中间 `next` payload 的计数推断覆盖率。
- 继续把 `redteam.complete` 视为“分析完成/计划就绪”，不要将其解释为代码已修复或修复已授权。

### 6.2 中期（preset/schema）

- 为 `redteam.experiment.next` 增加结构化一致性门禁：`completed_count = accepted_count + rejected_count`、`completed_count + remaining_count = total_experiment_count`，并要求 `next_experiment_id` 是 evidence board 中唯一的下一个未执行 ID。
- 对 `redteam.experiment.next` 增加同一队列状态的去重/幂等约束；preset instructions 已写“exactly one”，但当前 schema/runtime 没有把它变成可拒收的契约。
- 让 evidence-gate 在 `next` emit 前从 durable board 重新计算计数，不接受模型自行推算的计数值。

### 6.3 长期（底座）

- 在 runtime 的跨事件 payload consistency 层提供“队列状态投影”校验，避免只校验字段类型和单字段边界。
- 在 diagnostics 中保留 agent-output 或等价的 emit/precheck trace，使类似问题可以区分 agent payload 错误、policy-check 重试和 runtime 重放。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| 重复 `redteam.experiment.next` 是 agent 重复 emit、runtime 重试，还是两者叠加 | 55 | 缺 `agent-output.jsonl` 和完整 orchestration trace | 已读可信 events、session recovery、ledger 观察和 preset/schema；不把具体责任单独归给 agent 或 runtime |

## 8. 最终判断

这次 red-team preset 的**业务编排预期基本实现**：它确实按串行队列完成了 22 个实验，并在最后经过 evidence gate、impact boundary、independent review 后报告 `PLAN_READY`。但是，**队列 handoff 的中间事件质量没有完全达到 preset 自己声明的严格契约**。因此最准确的结论是：

> **结果可用，编排不完全干净；问题发现链路完成，但队列计数和重复 dispatch 仍应作为 preset/runtime 编排缺陷处理。**
