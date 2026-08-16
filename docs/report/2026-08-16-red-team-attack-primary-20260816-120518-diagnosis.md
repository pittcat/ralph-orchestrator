---
title: red-team-attack Loop `primary-20260816-120518` 运行链路诊断报告
date: 2026-08-16
type: diagnosis
loop_id: primary-20260816-120518
preset: builtin:red-team-attack
run_dir: .
status: 部分执行后因 runtime policy-check 状态可见性缺陷提前失败；攻击面未完成
diagnostics_mode: MINIMAL
bundle: finalized
bundle_path: .ralph/diagnostics/2026-08-16T20-05-18/diagnosis-input.json
history_search: disabled
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: present
activation_outcomes: present
evidence_gaps: ["本 session 无 orchestration.jsonl / agent-output.jsonl；OPAC 不能按 FULL 模式逐 tool call 审计", "feedback.jsonl 为空"]
execution_capabilities: [single-chain]
---

# red-team-attack Loop `primary-20260816-120518` 运行链路诊断报告

> 生成时间：2026-08-16
>
> 诊断对象：`.ralph/`，loop_id=`primary-20260816-120518`
>
> 对照 preset：`presets/en/red-team-attack.yml` + `presets/schemas/red-team-attack.yml`
>
> 历史检索：disabled；本报告只使用本次 run 产物与当前源码。

## 0. 产物盘点

`execution_capabilities: [single-chain]`。preset 使用 `event_loop.execution_mode: isolated`，未声明 `event_loop.supervisor.enabled: true`，hat instructions 未使用 `ralph wave emit` / `ralph wave verify`，当前 events 也没有 `wave_id`。因此 `.ralph/supervisor.db` 的存在不改变能力判定，缺 wave/supervisor 证据不构成故障。

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` → `.ralph/events-20260816-120518.jsonl` | ✅ | 7 | 唯一可信 events 账本；拓扑为 `start → locked → resolved → mapped → done → failed → complete` |
| S | `.ralph/ledger.jsonl` | ✅ | 14 | 记录 6 次 iteration 与 accepted event observation |
| S | `.ralph/recovery.jsonl` | ✅ | 11 | 当前 session recovery sidecar 仅 1 行 `agent_doc_sync`; workspace 历史内容含旧拒收记录，不把旧记录归因给本次 run |
| A | `.ralph/agent/tasks.jsonl` | 条件未启用 | 缺失 | preset `tasks.enabled: false`，预期 |
| A | `.ralph/agent/summary.md` | ✅ | 30 | 终止摘要存在 |
| A | `.ralph/agent/handoff.md` | ✅ | 41 | 终止 handoff 存在 |
| B | `.ralph/diagnostics/2026-08-16T20-05-18/` | ✅ | MINIMAL | bundle finalized；无 orchestration/agent-output |
| B | `runtime-trace.jsonl` | ✅ | 31 | record sequence 1..31 连续；6 条 activation outcome |
| B | `feedback.jsonl` | ✅ | 0 | 无 feedback 记录，形成证据缺口 |
| B | `.ralph/red-team/07-evidence-board.md` | ✅ | — | `RTE-001 accepted`, `remaining=17`, `next=RTE-002` |
| C | `.ralph/red-team/04-attack-surface.md` | ✅ | — | 15 个攻击面 |
| C | `.ralph/red-team/05-experiment-plan.md` | ✅ | — | 18 个实验，RTE-001..RTE-018 |
| C | `.ralph/red-team/evidence/RTE-001/` | ✅ | — | 原始证据与 manifest 存在 |
| C | `.ralph/red-team/PLAN.md` | ❌ | — | 未进入 impact-boundary，失败分支预期 |
| C | `.ralph/red-team/REPORT.md` | ✅ | — | reporter 输出 FAIL 报告 |

### 终态时序

| 项目 | 判定 |
|---|---|
| 首轮终态 | 失败：accepted `redteam.failed`，随后 accepted `redteam.complete(success=false)` |
| 恢复状态 | 无；没有后续 accepted 的 `redteam.experiment.next` 或成功终态 |
| 最终代码状态 | locked HEAD/tree 与 run 初始值一致；本次 red-team 只写 `.ralph/red-team/` 产物 |
| 一致性告警 | 无“失败终态后代码恢复”现象；但有“实验流程失败后 reporter 正常收束”现象 |

## 1. 结论摘要

### 1.1 健康度

**部分偏离 / 流程提前失败。** 这次 run 没有完成 Red Team 攻击。攻击面 mapper 设计了 18 个实验，但只执行到 RTE-001；RTE-002 至 RTE-018 均未执行，impact-boundary、independent-reviewer 也没有激活。`REPORT.md` 的 `PLAN_REJECTED` 是真实失败结论，不是“全部攻击完成且没有 finding”。

根因是 **mechanism/runtime**，置信度 **95/100**：evidence-gate 已把 RTE-001 写成 ACCEPTED，正确的 `redteam.experiment.next(RTE-002, completed=1, remaining=17, accepted=1, rejected=0)` handoff 在统一 policy-check 路径被错误判为“队列未初始化”。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 部分合规 | 6 个 activation 均 `merged`、backend exit=0；但本 session 为 MINIMAL，缺逐 tool-call 的 FULL 审计；RTE-001 的 handoff 被 runtime gate 拒收 | 80 |
| Q2 | 基座机制是否正常生效？ | ❌ 有缺陷 | isolated routing、channel merge、terminal completion 生效；但 CLI unified policy-check 未恢复 `PolicyRuntimeState`，误拒正确 queue handoff | 95 |
| Q3 | 编排是否合理、正常运行？ | ❌ 未完成 | preset 预期是 18 项串行队列；实际只到 `redteam.experiment.done(RTE-001)`，之后 `redteam.failed → redteam.complete` | 98 |
| Q4 | 问题归因是什么？ | mechanism；不是 agent 未攻击、不是证据不足 | 双账本显示 evidence board ACCEPTED，而 failure artifact 与源码均指向 unified policy-check 状态缺口 | 95 |

### 1.3 根因一句话

`ralph-cli` 的统一 policy-check 在 `StateLedger::replay_from_disk` 后没有把 `.ralph/events.jsonl` 重放为 `PolicyRuntimeState`；因此 `redteam.experiment.next` 虽然携带与证据板一致的队列计数，却在 `validation.rs:242-247` 被当成未由 `attack.mapped` 初始化而拒收。

### 1.4 Prompt visibility 对账

本次没有将“agent 看不到 skill”作为根因：6 个 activation outcome 均显示 `channel_exists=true`、`channel_readable=true`、`merge_succeeded=true`，且每个对应 accepted event 均已进入主 events。`ralph inspect prompt --hat evidence-gate` 在当前 operator 配置下无法解析该 hat，因此不把 visibility 模拟结果作为正面证据；这不改变上述 runtime 双账本结论。

## 2. 执行链路对比

预期链路：

```text
redteam.start
  → target-locker: redteam.target.locked
  → plan-resolver: redteam.plan.resolved
  → attack-surface-mapper: redteam.attack.mapped (18 experiments)
  → experiment-runner: one redteam.experiment.done per RTE
  → evidence-gate: redteam.experiment.next until RTE-018
  → evidence-gated → impact-boundary → plan.ready
  → independent-reviewer → reviewed
  → reporter: redteam.complete(success=true)
```

实际链路：

```text
start → locked → resolved → attack.mapped(18)
      → experiment.done(RTE-001, accepted)
      → [next handoff rejected by runtime policy-check]
      → failed(evidence-gate) → complete(success=false)
```

因此“攻击面没有攻击完成”不是感觉，而是由三份独立证据直接证明：主 events 第 4 行声明总数 18，第 5 行只完成 RTE-001；证据板记录 `completed=1 / remaining=17 / next=RTE-002`；`PLAN.md`、impact boundary 和 independent review 均不存在。

## 3. 历史问题上下文

`N/A (history disabled)`。本次未读取 `docs/report/`、`docs/solutions/`、`docs/plans/` 或 `docs/brainstorms/`。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 初判 | 置信度 |
|---|---|---|---|---:|
| DEV-001 | 攻击队列未完成 | `.ralph/events-20260816-120518.jsonl:4-7`；`.ralph/red-team/07-evidence-board.md:29-33,213-218` | P0 | 98 |
| DEV-002 | 正确的 next handoff 被 runtime 拒收 | `.ralph/red-team/failures/evidence-gate-rte001-handoff-blocked.md`；`crates/ralph-core/src/event_policy/validation.rs:242-247` | P0 | 95 |
| DEV-003 | CLI unified path 未注入 policy runtime state | `crates/ralph-cli/src/policy_check/unified.rs:291-303,379-382`；`crates/ralph-core/src/event_policy/runtime.rs:343-386` | P0 | 95 |
| DEV-004 | RTE-001 本身证据完整并 ACCEPTED | `.ralph/red-team/07-evidence-board.md:37-218`；`.ralph/red-team/evidence/RTE-001/evidence-manifest.json` | P1 | 90 |

### 4.1 OPAC 逐 hat 审计（MINIMAL）

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| target-locker | ✅ | ⚠️ | ✅ | ✅ | activation outcome seq 5；accepted `target.locked` | 70 |
| plan-resolver | ✅ | ⚠️ | ✅ | ✅ | seq 10；accepted `plan.resolved`，coverage/traceability=100 | 70 |
| attack-surface-mapper | ✅ | ⚠️ | ✅ | ✅ | seq 15；accepted `attack.mapped`, experiment_count=18 | 70 |
| experiment-runner | ✅ | ⚠️ | ✅ | ✅ | seq 20；RTE-001 manifest 与证据板均存在 | 70 |
| evidence-gate | ✅ | ⚠️ | ✅ | ❌ | seq 25 merged，但 `experiment.next` 被 unified policy-check 拒收 | 70 |
| reporter | ✅ | ⚠️ | ✅ | ✅ | seq 30；accepted `complete(success=false)` | 70 |

说明：MINIMAL 只有 events、session recovery、runtime trace 和日志类证据，不能把未观察到的 `--policy-check` 过程升格为 FULL 模式结论；因此 P 列保守标为 ⚠️，不把这个观测限制单独定为 OPAC P0。

### 4.2 Activation outcome

| sequence | hat | status | backend | merge | terminal obligation | 分类 | 置信度 |
|---:|---|---|---:|---:|---|---|---:|
| 5 | target-locker | merged | 0 | true | target.locked | — | 90 |
| 10 | plan-resolver | merged | 0 | true | plan.resolved / failed | — | 90 |
| 15 | attack-surface-mapper | merged | 0 | true | attack.mapped / failed | — | 90 |
| 20 | experiment-runner | merged | 0 | true | experiment.done / failed | — | 90 |
| 25 | evidence-gate | merged | 0 | true | experiment.next / evidence.gated / failed | attempted_but_rejected | 88 |
| 30 | reporter | merged | 0 | true | complete | — | 90 |

`evidence-gate` 的 `status=merged` 只说明 hat-channel 成功合并了它最终发出的 `redteam.failed`；它不表示 `redteam.experiment.next` 成功写入。第二账本是主 events 第 6 行的 accepted `redteam.failed`，与 failure artifact 的 runtime rejection 原因一致。

## 5. 问题归因

| 优先级 | 问题 | 分类 | 置信度 | 证据 | 历史关联 | 加深 |
|---|---|---|---:|---|---|---:|
| P0 | 18 项攻击队列在 RTE-001 后提前终止，17 个实验未执行 | mechanism / compound | 98 | DEV-001 + preset `red-team-attack.yml:687-778` + board counters | `N/A (history disabled)` | 1 |
| P0 | unified policy-check 的 `PolicyRuntimeState` 与 events 账本脱节，误拒 `experiment.next` | mechanism | 95 | DEV-002 + DEV-003 + `runtime.rs:343-386` | `N/A (history disabled)` | 1 |
| P1 | 运行报告把流程收束为 `PLAN_REJECTED`，但不能作为 18 项攻击结论 | preset/runtime contract | 90 | `events...jsonl:6-7` + `REPORT.md` 的 1/18、未生成 PLAN 说明 | `N/A (history disabled)` | 0 |

RTE-001 的安全结论只能限于其自身目标：该实验的控制组、攻击组与 restart/replay 检查均通过，当前证据板将其 ACCEPTED；它不能外推为 RTE-002..RTE-018 的安全结论，也不能外推为 3 个计划整体无 finding。

## 6. 修复建议（仅人工执行）

### 6.1 短期

- 在修复 unified policy-check 并重建 CLI 后，人工恢复/重跑当前 red-team queue；验收标准是主 events 出现 `redteam.experiment.next(RTE-002)`，且 counters 与 evidence board 一致。不要把 reporter 的 `complete(success=false)` 当作攻击完成。

### 6.2 中期

- 为 `run_policy_check_unified_with_config` 增加回归测试：events 中已有 `attack.mapped` 与 `experiment.done` 时，unified `experiment.next` 应与 direct `PolicyRuntimeState::from_events` 得出相同 queue state；同时覆盖空 ledger snapshot 与重复 handoff。
- 让 CLI unified path 在进入 `validate_with_preview` 前恢复与 loop 相同的 policy runtime state，避免 legacy gate 通过、unified gate 误拒的双路径漂移。

### 6.3 长期

- 统一 CLI policy-check 与 EventLoop 的 state bootstrap authority，避免一条路径从 events 重放、另一条路径只从 ledger snapshot 取状态。该建议关联置信度：95。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| RTE-002..RTE-018 是否存在真实安全 finding | 0（不可判定） | 实验根本未执行 | 已核对 attack map、experiment plan、events、evidence board；不得由静态攻击面代替实验 |
| unified path 的最小代码修复是否应直接复用 `build_policy_state` | 55 | 尚未修改/测试；本次诊断不实施修复 | 已定位 `build_policy_state` 与 `PolicyRuntimeState::from_events`，未运行修复验证 |

## 8. 最终判断

是的，这次运行没有把攻击面攻击完成。准确状态是：**攻击面设计完成（15 surfaces / 18 experiments），RTE-001 完成且证据通过，但在 evidence-gate 向 RTE-002 移交时触发 runtime policy-check 机制缺陷，导致剩余 17 项及后续 impact-boundary / independent-review 全部未执行。**

本诊断只读并已落盘报告；未修改生产代码、preset、正式测试或 `.ralph` 运行时状态文件。
