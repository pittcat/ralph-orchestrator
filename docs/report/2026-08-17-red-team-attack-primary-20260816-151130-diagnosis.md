---
title: builtin:red-team-attack Loop `primary-20260816-151130` 运行链路诊断报告
date: 2026-08-17
type: diagnosis
loop_id: primary-20260816-151130
preset: builtin:red-team-attack
run_dir: .
status: 失败终态已闭环，但红队业务结果为 PLAN_REJECTED，未生成 PLAN.md
diagnostics_mode: MINIMAL
bundle: finalized
bundle_path: .ralph/diagnostics/2026-08-16T23-11-30/diagnosis-input.json
history_search: disabled
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: present
activation_outcomes: present
evidence_gaps: ["MINIMAL 模式缺 orchestration.jsonl", "errors.jsonl 缺失", "feedback.jsonl 为空"]
execution_capabilities: [single-chain]
---

# builtin:red-team-attack Loop `primary-20260816-151130` 运行链路诊断报告

> **生成时间**：2026-08-17
> **诊断对象**：`.ralph/`，loop_id=`primary-20260816-151130`
> **对照 preset**：`presets/en/red-team-attack.yml` + `presets/schemas/red-team-attack.yml`
> **诊断方式**：Phase 0 产物盘点 → 事件/activation outcome/业务产物对账 → 源码归因；`history_search=disabled`

## 0. 产物盘点

`execution_capabilities` 为 `[single-chain]`：preset 使用 `event_loop.execution_mode: isolated`，未声明 `event_loop.supervisor.enabled: true`，instructions 未使用 `ralph wave emit` / `ralph wave verify` 或 `WAVE CONTEXT`。`.ralph/supervisor.db` 虽存在，但在本能力集合下不构成故障信号；事件中也没有 `wave_id`。

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` → `.ralph/events-20260816-151130.jsonl` | 是 | 20 行 | 唯一可信编排事件文件 |
| S | `.ralph/ledger.jsonl` | 是 | 40 行 | 有状态提交记录 |
| S | `.ralph/recovery.jsonl` | 是 | 1 行 | 仅 `agent_doc_sync` info；无拒收记录 |
| S | `.ralph/current-loop-id` | 是 | — | 与 bundle loop_id 一致 |
| A | `.ralph/agent/summary.md` | 是 | 19 iterations | runner 生命周期标为 Completed successfully |
| A | `.ralph/agent/handoff.md` | 是 | — | 生成于终止后；无 pending work |
| A | `.ralph/agent/tasks.jsonl` | 否/空 | lock only | preset `tasks.enabled: false`，符合预期 |
| B | `.ralph/diagnostics/2026-08-16T23-11-30/` | 是 | finalized | bundle 已 finalized |
| B | `runtime-trace.jsonl` | 是 | 96 records | sequence 1–96，单调，无坏行 |
| B | `feedback.jsonl` | 是 | 0 行 | feedback lifecycle 缺失，降为 evidence gap |
| B | `orchestration.jsonl` | 否 | — | MINIMAL 模式下缺失，不能做 FULL OPAC 对账 |
| B | `.ralph/supervisor.db` | 是 | — | single-chain 下 N/A，不判故障 |
| C | `.ralph/red-team/01..07-*`、实验与 evidence | 部分 | 8 个 RTE 完成 | RTE-001..008 已触达；RTE-009..021 未触发 |
| C | `.ralph/red-team/REPORT.md` | 是 | 176 行 | FAIL/`PLAN_REJECTED` 报告 |
| C | `.ralph/red-team/PLAN.md` | 否 | — | 失败分支按 preset 约定不生成 |
| C | `.ralph/red-team/QUESTIONS.md` | 是 | 59 行 | 要求人工决定是否重写实验计划链路 |

Bundle 摘要：`status=finalized`、`preset_label=builtin:red-team-attack`、`loop_id=primary-20260816-151130`、`total_iterations=19`、`recovery_count=0`、`drift_finding_count=0`。`ralph diagnose` 的两个 warning 是 session 缺 `orchestration.jsonl` 与 `errors.jsonl`，不是本次业务失败的直接原因。

## 1. 结论摘要

### 1.1 健康度

- **判定：失败终态已闭环，非卡死；业务交付不完整。** 外层 runner 在第 19 次 activation 收到了并接受 `redteam.complete`，所以 loop 已终止；但该事件携带 `success:false`，且 `plan_path:""`。
- **P0：1；P1：0；P2：0**（均达到入表门槛）。P0 是红队实验计划漂移导致实验器 control collapse，不是 Ralph runtime 卡死。
- **最高优先级根因置信度：P0-1 = 85/100**。MINIMAL 模式封顶 85；有 preset 行号、事件双账本、Tier C 产物和源码/命令证据，但没有 FULL agent-output。
- **历史复发：** `N/A (history disabled)`。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 基本合规，证据降级 | 19 次 activation 均 `merged`、backend exit 0、merge 成功；但 MINIMAL 缺 orchestration/agent-output，OPAC 只能部分确认 | 75 |
| Q2 | 基座机制是否正常生效？ | ✅ 是 | 失败事件被 accepted，reporter 按失败分支发出唯一 completion；runtime 在 `wave_scope.rs:619-628` / `1064-1078` 处理 completion | 85 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ 拓扑正常收敛，业务实验计划不合理 | `redteam.failed` 在 event 19 触发 reporter；但 RTE-001..008 全部 control collapse，队列只完成 8/21 | 85 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **preset/实验计划漂移为主因，agent 执行失败为结果，不是 runtime 根因** | RTE-008 命令依赖不存在的 env var、错误 integration-test filter 和 struct field 名；preset 明确要求失败分支 `success:false` | 85 |

### 1.3 根因一句话

这次不是“没有走到最后”，而是**走到了失败终态的最后**：RTE-008 暴露出实验计划生成链路没有校验命令是否对应真实源码/测试入口，experiment-runner 发出 `redteam.failed`，reporter 正确生成 FAIL report 并发出 `redteam.complete(success=false)`；因此没有经过 `impact-boundary`/`independent-reviewer`，也没有 PLAN.md。

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| **首轮终态** | 失败：事件第 19 行 `redteam.failed`，reason=`control_collapse:RTE-008:env_var_RALPH_FAKE_WRITE_FULL_not_implemented+wrong_test_filter_event_loop+wrong_test_name_state_machine_apply_snapshot_is_struct_field`。 |
| **恢复状态** | 无恢复；第 20 行是失败分支 reporter 的正式 completion，不是失败后的成功恢复。 |
| **最终代码状态** | 红队锁定 HEAD=`e026bda14b4eb5c9fb44929b0300da5353d162b9`，tree=`ca855003e057cd8da533e9aefb3198dee96c06e3`；本次 reporter 声称 tracked tree 未变。当前工作区另有用户既存的 `post-merge-converge` 三文件修改，本诊断未触碰。 |
| **一致性告警** | **不存在 silent-success**：`.ralph/agent/summary.md` 的 `Completed successfully` 描述的是 runner 生命周期；业务成功由 `redteam.complete.payload.success` 决定，本次为 `false`。 |

## 2. 执行链路

```text
redteam.start
  → target.locked
  → plan.resolved
  → attack.mapped (21 RTE)
  → [experiment.done → experiment.next] × 7
  → experiment.done(RTE-008)
  → redteam.failed(control_collapse, 8/8 rejected, 13 remaining)
  → reporter
  → redteam.complete(success=false, plan_path="")
```

事件文件共有 20 条记录，主题计数为：`redteam.experiment.done` 7、`redteam.experiment.next` 7、其余 start/lock/resolve/map/failed/complete 各 1。这里的“7 个 done”与 `RTE-001..008` 的 8 个实验尝试并不矛盾：RTE-008 直接走失败事件，没有 `experiment.done`。

## 3. 历史问题上下文

`history_search=disabled`；未读取 `docs/report/`、`docs/solutions/`、`docs/plans/`、`docs/brainstorms/`，历史关联统一为 `N/A (history disabled)`。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | RTE-001..008 均未形成可区分的 control/attack 证据，RTE-008 以 control collapse 终止 | `.ralph/events-20260816-151130.jsonl:18-20`; `.ralph/red-team/failures/experiment-runner.md`; `.ralph/red-team/REPORT.md:23-32` | P0 | 85 | file/line +25；双账本 events/Tier-C +20；preset 行号 +15；Tier C 交叉验证 +10 | 缺 FULL agent-output/orchestration |
| DEV-002 | 外层完成状态与业务成功状态不同 | `.ralph/events-20260816-151130.jsonl:19-20`; `presets/en/red-team-attack.yml:998-1016`; `crates/ralph-core/src/event_loop/wave_scope.rs:619-628,1064-1078` | P1 初判 | 85 | file/line +25；双账本 +20；preset 行号 +15；Tier C +10 | 无需升级为 runtime defect；需 operator 认知修正 |

### 4.1 OPAC 逐 hat 审计（MINIMAL 降级）

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| target-locker / plan-resolver / attack-surface-mapper | ✅ | ⚠️ | ✅ | ✅ | 每次 activation `backend_exit_code=0`、`merged=true`；事件 payload 被接受 | 75 |
| experiment-runner / evidence-gate | ✅ | ⚠️ | ✅ | ✅ | 事件链持续推进，失败原因被具体写入 failure artifact；缺 agent-output 逐条 tool-call 证据 | 70 |
| reporter | ✅ | ⚠️ | ✅ | ✅ | `REPORT.md`/`QUESTIONS.md` 存在；event 20 为 `success:false` 且 `plan_path:""`，符合 preset 失败分支 | 80 |

`ralph -c presets/en/red-team-attack.yml inspect prompt --hat experiment-runner --format json` 对账结果：auto-inject 为 `ralph-tools`、`ralph-tools-memories`、`ralph-tools-opac`；on-demand 为 `ralph-tools-cmdref`、`ralph-tools-emit`、`ralph-tools-precheck`、`ralph-tools-recovery-directives`、`ralph-tools-tasks`、`ralph-tools-wave`。preset instructions 明确要求在 payload 前加载 `ralph-tools-emit`，未发现 auto/on-demand 互相矛盾；但 MINIMAL 模式不足以证明每次 tool call 的实际顺序。

### 4.2 Activation outcome

`runtime-trace.jsonl` 有 19 条 activation outcome，sequence 5–95，全部 `status=merged`、`backend_exit_code=0`、`merge_succeeded=true`，无 timeout、backend failure、channel routing failure 或 rejected candidate。关键行如下：

| sequence | hat | status | terminal obligation | 对账结论 |
|---:|---|---|---|---|
| 5 | target-locker | merged | `redteam.target.locked` | 与 events 第 2 行一致 |
| 10 | plan-resolver | merged | `redteam.plan.resolved` | 与 events 第 3 行一致 |
| 15 | attack-surface-mapper | merged | `redteam.attack.mapped` | 与 events 第 4 行一致 |
| 20–85 | experiment-runner / evidence-gate | merged | `experiment.done/next`、失败候选 | 与 events 第 5–18 行一致 |
| 90 | experiment-runner | merged | `redteam.experiment.done`、`redteam.failed` | channel 仍成功 merge；业务失败是显式事件 |
| 95 | reporter | merged | `redteam.complete` | 与 events 第 20 行一致 |

该表说明“没有走到最后”不是 activation 没有收口；所有 activation 都收口了，最后一个 reporter 也收口了。

## 5. 问题归因

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P0 | 实验计划命令与真实执行环境脱节：`RALPH_FAKE_WRITE_FULL=1 cargo nextest run -p ralph-core --test event_loop -- state_machine_apply_snapshot` 无法触达目标实验，导致 RTE-001..008 全部 control collapse/拒收 | **preset**（实验计划生成链路） | **85** | DEV-001 | `presets/en/red-team-attack.yml:605-625,688-715` + events 第 18–19 行 + `.ralph/red-team/REPORT.md:40-57` + failure artifact | `N/A (history disabled)` | 1→preset 行号；2→events + Tier C 双账本；MINIMAL 封顶 |
| P1 | operator 可能把 `summary.md:Completed successfully` 误读为红队成功；实际业务 success 在 completion payload 中为 false | **compound：runtime 生命周期语义 70% + reporter/preset 语义 30%** | **80** | DEV-002 | runtime `wave_scope.rs:619-628,1064-1078` + preset `red-team-attack.yml:993-1016` + event 20 | `N/A (history disabled)` | 1→源码；2→preset/event 对账 |

P0 不是 Ralph 基座机制故障：Ralph 正确接受 `redteam.failed`，随后路由 reporter，并以 `redteam.complete` 结束；preset 也明确规定失败分支 `success:false`、`plan_path:""`。真正的阻塞点是实验计划在进入 runner 前没有验证环境变量、nextest binary/filter 和测试函数名。

## 6. 修复建议（仅人工执行）

### 6.1 短期

- 目标：避免误判本次结果。改动：查看 `.ralph/events-20260816-151130.jsonl` 最后一条 payload 和 `.ralph/red-team/REPORT.md` 的 `PLAN_REJECTED`，把本次 run 标为“失败闭环”，不要继续等待 RTE-009。预期：立即确认 loop 已结束但没有 PLAN.md。置信度：95。
- 目标：重新进入实验前先验证 RTE-008 的真实测试入口。改动：人工确认计划采用真实 `#[test]` 名、正确的 `--lib`/集成测试入口和真实 feature/toggle；本报告不自动执行命令。置信度：85。

### 6.2 中期

- 目标：在 red-team experiment queue 生成前增加 command sanity gate，检查 env var 是否有源码读取点、`--test` binary 是否存在、substring 是否命中真实测试函数。预期：把当前 control collapse 变成计划生成阶段的可读拒绝。置信度：85。
- 目标：明确 operator-facing 成功字段。改动：文档/报告入口同时展示 loop termination reason 与业务 completion `success`，避免只读 `summary.md` 的 Completed successfully。预期：区分“生命周期终止”和“业务成功”。置信度：80。

### 6.3 长期

- 目标：让 red-team 计划生成链路与 schema/源码保持可验证。改动：对 `commands:`、required_fields、topic ownership 和测试入口建立结构化 preflight；通过后才允许进入实验队列。预期：避免 8/8 实验在假设验证前耗尽。置信度：85。

## 7. 未核实疑点

1. `feedback.jsonl` 为空，无法恢复 feedback lifecycle 的 discovered/evidence/action/validation/final 阶段；不影响已接受 events 与业务产物对账，但不能据此评价每个 agent 的 tool-call 顺序。
2. 缺 `orchestration.jsonl` 与 `agent-output.jsonl`，无法在 FULL 证据等级下判断具体 agent 是否逐次执行了 policy-check；该疑点不升级为 agent 根因。

## 8. 最终回答用户问题

是“走到最后但以失败结果退出”，不是“卡住没走完”：

- 最后一个业务失败：`redteam.failed`，RTE-008 control collapse。
- 最后一个事件：`redteam.complete`，但 `success:false`、`plan_path:""`。
- 生成了：`REPORT.md`、`QUESTIONS.md`。
- 没生成：`PLAN.md`，因为没有经过 impact-boundary 和 independent-reviewer 的成功链路。

本次 run 的直接下一步是先修正/重写实验计划生成链路，再重跑 red-team；不需要修 Ralph 的终态收口机制。
