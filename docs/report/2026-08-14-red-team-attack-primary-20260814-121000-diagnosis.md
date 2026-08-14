---
title: builtin:red-team-attack Loop `primary-20260814-121000` 运行链路诊断报告
date: 2026-08-14
type: diagnosis
loop_id: primary-20260814-121000
preset: builtin:red-team-attack
run_dir: .
status: 部分完成但未形成有效 Red Team 结论：首个实验在证据门禁失败，后续实验不可达
diagnostics_mode: MINIMAL
bundle: finalized
bundle_path: .ralph/diagnostics/2026-08-14T20-10-00/diagnosis-input.json
history_search: preset-only
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: missing
evidence_gaps:
  - feedback.jsonl 为空，缺少 feedback lifecycle 记录
  - MINIMAL 模式没有 orchestration.jsonl 与 agent-output.jsonl，无法逐 tool-call 审计 OPAC
  - 未找到 red-team 专属 BDD 场景来验证 10 个实验的调度闭环
---

# builtin:red-team-attack Loop `primary-20260814-121000` 运行链路诊断报告

> **生成时间**：2026-08-14
> **诊断对象**：`.ralph/`（loop_id=`primary-20260814-121000`，启动至终止）
> **对照 preset**：`presets/en/red-team-attack.yml` + `presets/schemas/red-team-attack.yml`
> **诊断方式**：主 Agent 按流程还原、历史、对账、归因四个视角顺序完成；未修改代码，也未重新运行 preset。
> **Diagnostics 模式**：MINIMAL；OPAC 只能做事件、日志和 prompt visibility 级别核验，不能逐 tool-call 证明。
> **历史范围**：`preset-only`，近 30 天与 `red-team-attack` / evidence-gate 症状相关的历史。
> **execution_capabilities**：`[runner]`
> **报告仓库**：`ralph-orchestrator` 主仓；中间 JSON 仅保存在临时 `DIAG_WORKDIR`，已清理。

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数/数量 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` 指向的 `events-20260814-121000.jsonl` | 是 | 7 行 | 本次唯一可信业务事件源 |
| S | 配对 `events-history-20260814-121000.jsonl` | 是 | 2 行 | 旁证，不覆盖 current-events |
| S | `.ralph/ledger.jsonl` | 是 | 14 行 | 有 state commit |
| S | `.ralph/recovery.jsonl` | 是 | 4 行 | 本 workspace 的历史 repair/recovery 记录；本次 session 没有业务拒收 |
| S | `.ralph/current-loop-id` | 是 | — | `primary-20260814-121000` |
| S | `.ralph/loops.json` | 是 | 0 个活动 loop | 当前 loop 已终止，空列表不是运行失败 |
| S | `.ralph/loop.lock` | 否 | — | 已正常释放 |
| A | `.ralph/agent/tasks.jsonl` | 否 | — | preset `tasks.enabled: false`，按条件不要求 |
| A | `.ralph/agent/summary.md` | 是 | 30 行 | 终止后生成 |
| A | `.ralph/agent/handoff.md` | 是 | 42 行 | 终止后生成 |
| B | `.ralph/diagnostics/2026-08-14T20-10-00/` | 是 | 18 个 session 产物 | bundle 已 finalized |
| B | `runtime-trace.jsonl` | 是 | 27 条 | sequence 1–27 连续、无坏行 |
| B | `feedback.jsonl` | 是但为空 | 0 行 | 诊断器按 Missing 处理，属于证据缺口 |
| B | `drift.jsonl` | 是 | 0 行 | 未发现 drift finding |
| B | `orchestration.jsonl` / `agent-output.jsonl` | 否 | — | MINIMAL 模式，不把缺失误判为编排故障，但限制 OPAC 结论强度 |
| B | `.ralph/supervisor.db` / `wave_id` | 不要求 | — | capability 只有 runner；无 supervisor/wave 信号 |
| C | `.ralph/red-team/01-target-lock.md` 至 `05-experiment-plan.md` | 是 | — | 前置阶段完成 |
| C | `.ralph/red-team/experiments/RTE-001.md` 至 `RTE-010.md` | 是 | 10 个 | 计划/实验定义存在，不等于已执行 |
| C | `.ralph/red-team/evidence/RTE-001/` | 是 | 15 个文件 | 只有 RTE-001 有执行证据，且缺 `ledger_sha256` |
| C | `.ralph/red-team/REPORT.md` / `QUESTIONS.md` | 是 | — | reporter 失败分支完成 |
| C | `.ralph/red-team/PLAN.md` | 否 | — | 没有正式 Finding 时按 preset 规则不生成 |

**execution_capabilities 推断**：`[runner]`。preset 没有启用 `event_loop.supervisor`，hat instructions 没有 `ralph wave emit` / `ralph wave verify` / `WAVE CONTEXT`，可信 events 也没有 `wave_id`。因此缺 supervisor.db 和 wave_id 均为 N/A，不是故障。

**Bundle 摘要**：`diagnosis-input.json` 为 `finalized`；`runtime-trace.jsonl` 为 present，27 条记录连续；`feedback.jsonl` 为空；诊断器未发现 drift 或错误日志，但报告能力受 MINIMAL 模式限制。

## 1. 结论摘要

### 1.1 健康度

- **判定**：工作流基础执行成功，但 Red Team 业务审查在首个实验的 evidence-gate 处终止，属于“部分完成、覆盖不足”，不是完整红队审查。
- **P0 / P1 / P2**：P0=0；P1=3；P2=1（均满足置信度入表门槛）。
- **最高优先级根因置信度**：P1-1 = **85/100**（MINIMAL 模式硬顶）。
- **正式安全 Finding**：0；当前没有足够实验覆盖支持产品漏洞结论。
- **历史复发**：同 preset 在近 30 天内至少两次在 evidence-gate 附近提前终止，但具体根因不同；属于 preset 闭环稳定性复发，不把历史不同根因硬合并为同一缺陷。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 关键事件路径可对账，逐 tool-call 不可证明 | 所有业务事件均被 accepted，prompt visibility 显示 `ralph-tools-opac` auto-inject、`ralph-tools-emit` on-demand；但无 agent-output | 70 |
| Q2 | 基座机制是否正常生效？ | ⚠️ 门禁和终态机制生效，但终止文案误导 | evidence-gate 正确拒绝缺失 raw evidence；同时 `success:false` 的 `redteam.complete` 被通用 termination 文案显示为 “All tasks completed successfully” | 85 |
| Q3 | 编排是否合理、正常运行？ | ❌ 未形成完整实验闭环 | attack mapper 声明 `experiment_count=10`，但实际只有 1 个 `redteam.experiment.done`；后续 9 个实验没有调度入口 | 85 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **主因是 preset 编排/证据契约，次因是底座可观测性；不是已证实的 agent 忘记 emit** | runner 只触发于 `redteam.attack.mapped` 且每 activation 执行一个实验；evidence gate 失败后按 preset 终止 | 85 |

### 1.3 根因一句话

当前 preset 把“10 个实验的计划”交给了一个“只触发一次、每 activation 只执行一个实验”的 runner；RTE-001 又因缺少 `ledger_sha256` 被 terminal evidence-gate 拒绝，于是流程在第一个实验后合法结束，未进入其余攻击面。**主因置信度：85/100。**

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| **首轮终态** | accepted 事件序列为 `redteam.failed`（evidence-gate）→ `redteam.complete`（reporter，`success:false`）；首轮是失败终态，不是成功审查 |
| **恢复状态** | 无本次业务恢复；没有后续 accepted 的 `redteam.evidence.gated`、`redteam.plan.ready` 或 `redteam.reviewed` |
| **最终代码状态** | HEAD/tree 与 target lock 一致，tracked tree clean；没有生产代码修改 |
| **一致性告警** | loop 日志中的 `Wrapping up: completed` / `Primary loop landed successfully` 只表示 runner 终止，不表示红队业务成功；报告中的 `success:false` 才是本次业务结果 |

## 2. 执行链路对比图

### 2.1 拓扑激活表

| Hat | preset 触发 | 实际激活 | 结果 |
|---|---|---:|---|
| target-locker | `redteam.start` | 1 | ✅ `target.locked` |
| plan-resolver | `target.locked` | 1 | ✅ `plan.resolved` |
| attack-surface-mapper | `plan.resolved` | 1 | ✅ `attack.mapped`，声明 10 个实验 |
| experiment-runner | `attack.mapped` | 1 | ⚠️ 只执行 RTE-001 |
| evidence-gate | `experiment.done` | 1 | ❌ `BINARY_GATE_FAILED` |
| impact-boundary | `evidence.gated` | 0 | ⏸️ 上游没有 gated |
| independent-reviewer | `plan.ready` | 0 | ⏸️ 上游没有 plan.ready |
| reporter | `failed` | 1 | ✅ 生成失败报告并发出 `complete success:false` |

### 2.2 预期 vs 实际时间轴

| 顺序 | 实际事件 | 结论 |
|---:|---|---|
| 1 | `redteam.start` | ✅ 启动 |
| 2 | `redteam.target.locked` | ✅ 目标锁定 |
| 3 | `redteam.plan.resolved` | ✅ 2/2 plans resolved |
| 4 | `redteam.attack.mapped`，`experiment_count=10` | ✅ 设计阶段完成 |
| 5 | `redteam.experiment.done`，`experiment_id=RTE-001` | ⚠️ 只有首个实验完成 |
| 6 | `redteam.failed`，`failed_stage=evidence-gate` | ❌ 缺 `ledger_sha256`，终止失败路径 |
| 7 | `redteam.complete`，`success=false` | ✅ 失败报告闭环，不是业务成功 |

关键静态矛盾在 `presets/en/red-team-attack.yml:484-493`：`experiment-runner` 的唯一 trigger 是 `redteam.attack.mapped`；其 mission 又明确写成 “one experiment per activation”。`redteam.experiment.done` 只触发 evidence-gate，当前没有“evidence 通过后调度下一个实验、全部完成后再汇总”的 dispatcher/aggregator 路径。

## 3. 历史问题上下文

扫描窗口：`preset-only (30d sliding)`。

| 文档 | 关联问题 | 相关性 | 结论 |
|---|---|---|---|
| `docs/report/2026-08-10-red-team-attack-red-teamprompt-cool-falcon-diagnosis.md` | scope gate predicate 反向，主链未进入 attack | 高 | 同 preset 早停，但根因是 scope gate 语义，不是本次 runner fan-out |
| `docs/report/2026-08-14-red-team-attack-primary-20260813-212249-diagnosis.md` | evidence-gate 自己被 deny，retry/terminal 未闭合，最终人工取消 | 高 | 同 preset、同 gate 阶段附近的闭环问题；本次运行已能直接 accepted `redteam.failed`，说明具体 self-deny 症状未复现 |
| `docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md` | evidence gate 被视为跨 preset 统一契约基础 | 中 | 需求层强调证据门禁，但没有证明当前 red-team-attack 已实现多实验聚合闭环 |

历史结论：这是同 preset 的重复可靠性风险——之前是 scope gate 和 retry route，本次是实验调度与证据交接；不能把它们伪装成一个已经完全定位的单一 runtime 根因。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | 计划有 RTE-001～RTE-010，但可信事件只有一个 `redteam.experiment.done` | `.ralph/red-team/05-experiment-plan.md:12-21`；`.ralph/events-20260814-121000.jsonl:4-5`；`presets/en/red-team-attack.yml:484-493` | P1 | 85 | preset 行号 +15；events+ledger 双账本 +20；Tier C 交叉验证 +10 | 没有 red-team BDD 场景验证多实验闭环 |
| DEV-002 | `ledger_sha256` 是 RTE-001 mandatory evidence，但 runner emit 的 evidence_paths 没有它，gate 只能在最后拒绝 | `.ralph/red-team/experiments/RTE-001.md:42`；`.ralph/red-team/07-retry-board.md:25-36`；`.ralph/events-20260814-121000.jsonl:5-6`；`presets/en/red-team-attack.yml:505-529,602-618` | P1 | 85 | preset/schema 行号 +15；events+ledger 双账本 +20；Tier C 交叉验证 +10 | 无 agent-output，不能判定缺证据是 agent 漏写还是模板/流程诱导 |
| DEV-003 | `redteam.complete.success=false`，但通用终止状态写成 “All tasks completed successfully” | `.ralph/events-20260814-121000.jsonl:7`；`.ralph/diagnostics/logs/ralph-2026-08-14T20-10-00-341-15722.log:36-42`；`crates/ralph-core/src/event_loop/termination_impl.rs:31-35`；`presets/schemas/red-team-attack.yml:344-356` | P1 | 85 | 源码行号 +25；events+logs/history 双账本 +20；preset/schema 行号 +15；MINIMAL 硬顶 | 当前业务报告本身写了失败，问题集中在通用终止/summary 文案 |
| DEV-004 | 报告称“Retry 耗尽”，但当前 preset 明确“不自动 retry”，本次只有一次 evidence-gate activation | `.ralph/red-team/REPORT.md:46-52`；`presets/en/red-team-attack.yml:570-574`；`.ralph/diagnostics/2026-08-14T20-10-00/runtime-trace.jsonl:17-24` | P2 | 75 | preset 行号 +15；events/runtime trace 双账本 +20 | 无 agent-output，无法确认是 reporter wording 还是模板/记忆污染 |

### 4.1 OPAC 逐 hat 审计表

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| target-locker | ✅ | ⚠️ | ✅ | ✅ | target artifact 与 accepted `target.locked` 一致；无 agent-output | 55 |
| plan-resolver | ✅ | ⚠️ | ✅ | ✅ | resolved payload 被 accepted；无逐 tool-call 证据 | 55 |
| attack-surface-mapper | ✅ | ⚠️ | ✅ | ✅ | `experiment_count=10` 的 mapped event 被 accepted | 55 |
| experiment-runner | ✅ | ⚠️ | ✅ | ✅ | RTE-001 event 被 accepted，tracked tree clean；mandatory evidence 后置检查失败 | 60 |
| evidence-gate | ✅ | ⚠️ | ✅ | ✅ | `redteam.failed` 被 accepted，`BINARY_GATE_FAILED` 与 retry board 一致 | 65 |
| reporter | ✅ | ⚠️ | ✅ | ✅ | `REPORT.md`/`QUESTIONS.md` 存在，complete payload 为 `success:false` | 60 |

> `O/P/A/C` 分别表示 Observe/Precheck/Apply/Confirm。MINIMAL 模式没有 agent-output，未观察到 `--policy-check` 不能证明 agent 没有执行；因此不把 OPAC 观测缺口单独升级为 P0。Prompt visibility 对账显示：`auto_inject` 包含 `ralph-tools-opac`，`on_demand` 包含 `ralph-tools-emit`，且 evidence-gate instructions 要求加载 emit skill 后执行 policy-check；没有发现“agent 看不到 emit skill”的解释。

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| **P1** | 多实验计划没有可达的逐项调度/聚合闭环，实际只运行 RTE-001 | **preset 编排** | **85** | DEV-001 | preset 行号(+15)；双账本(+20)；Tier C(+10)；MINIMAL 硬顶 | 高：同 preset 多次在前置 gate 附近早停，但精确根因不同 | 第1轮 preset 行级对账；第2轮 events+ledger+Tier C |
| **P1** | 实验完整性在 `experiment.done` 之后才被 gate 发现；单个证据缺口会终止整轮且没有继续其余实验的策略 | **compound：preset 证据契约 60% + terminal 编排 40%** | **85** | DEV-002 | preset/schema(+15)；双账本(+20)；Tier C(+10)；MINIMAL 硬顶 | 高：同 preset 曾在 evidence-gate retry/terminal 路径早停；具体 self-deny 本次未复现 | 第1轮 runner/gate 行级对账；第2轮 artifact/event 对账 |
| **P1** | 业务失败被通用终止文本包装成成功，造成 operator 误判 | **mechanism / observability contract gap** | **85** | DEV-003 | 源码行号(+25)；双账本(+20)；preset/schema(+15)；MINIMAL 硬顶 | 中：历史报告也反复强调 workflow completion 不等于业务成功 | 第1轮源码反查；第2轮 event+logs+schema |
| **P2** | “Retry 耗尽”与当前 no-auto-retry 运行事实不一致 | **report artifact contract** | **75** | DEV-004 | preset 行号(+15)；event+runtime trace(+20)；MINIMAL 硬顶 | 中：历史 retry 语义曾真实存在，但本次 preset 已明确 terminal/no-auto-retry | 第1轮报告/preset/trace 对账；因缺 agent-output 不继续归责 |

## 6. 修复建议（non-executing）

以下仅列人工后续建议，本诊断没有自动执行任何建议。

### 6.1 短期（operator workaround）

1. 不要把本次 `REPORT.md` 当作完整 Red Team 结论；它只证明 RTE-001 的安全结果和证据门禁失败。
2. 补证后即使重新跑通 RTE-001，也不能默认 RTE-002～RTE-010 会自动执行；应先人工确认多实验调度方案。
3. 继续保持“没有正式 Finding 不启动生产修复”的约束。

### 6.2 中期（preset / schema / instructions）

1. 为 10 个实验增加明确的 coordinator/dispatcher：维护当前实验游标；每个实验经 evidence gate 后调度下一个；只有全部实验完成后才发布聚合的 `redteam.evidence.gated` 或最终失败结果。
2. 明确失败策略：单个实验证据失败时，是立即终止整轮，还是记录该实验失败后继续独立实验并在最后汇总。当前 preset 两者之间没有可执行的选择机制。
3. 将 mandatory evidence manifest 纳入 `redteam.experiment.done` 合约，至少让 `ledger_sha256` 的路径、哈希、命令、目标文件和 run 标识在 emit 前可机器检查，而不是等 evidence-gate 才首次发现。
4. 增加真实 EventLoop/BDD 场景：断言 10 个实验的调度顺序、单个实验失败后的终态、全部实验成功后的聚合终态；不要只测 YAML 文本。
5. 将报告中的“Retry 耗尽”改为“本轮 evidence gate 首次失败，preset 未自动重试”，除非确有 retry event 被 accepted。

### 6.3 长期（机制 / 底座）

1. 让终止状态同时携带并展示业务 verdict：`CompletionPromise` 只能表示 loop 停止，不能固定映射为 “All tasks completed successfully”。对于 `success:false`，summary、TUI、history 应显示“工作流终止但业务失败”。
2. 为通用 completion 文案增加回归测试，覆盖 terminal topic 已接受但 payload `success:false` 的 preset。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| `experiment-runner` 的设计意图是否本来就是“一次 run 只执行一个实验、操作者手工多次启动”，还是应在一个 loop 内跑完 10 个实验 | 55 | preset 没有显式的 batch/dispatcher 语义；无 agent-output/作者意图证据 | 已核对 experiment_count、runner trigger、terminal_events 和实际事件；不把意图猜测写成定论 |
| `ledger_sha256` 缺失究竟是 agent 漏写、实验模板未要求生成，还是临时 ledger 生命周期导致不可记录 | 55 | MINIMAL 模式缺 agent-output 与实验过程 stdout；当前证据只证明缺失及其 gate 结果 | 已核对 mandatory_evidence、evidence_paths、retry board、runner/gate instructions |

## 8. 诊断边界与提交前检查

- 只把 `.ralph/current-events` 指向的 `events-20260814-121000.jsonl` 作为本次编排事实源；其它 events 文件未用于覆盖本次事件结论。
- 历史只扫描近 30 天且限定 `red-team-attack` / evidence-gate 相关文档；历史只用于复发背景，不覆盖本次 accepted events。
- 未修改生产代码、测试、preset、schema、Git 历史或 `.ralph` 运行时状态。
- 未执行重新运行、cargo、git 写操作或删除操作。
- `DIAG_WORKDIR` 中间产物已清理；最终只新增本诊断 Markdown 报告。
