---
title: ce-executor-pipeline Loop `2026-08-13-001-feat-gap01-unified-orchestration-knowledge-state-plan` 运行链路诊断报告
date: 2026-08-13
type: diagnosis
loop_id: 2026-08-13-001-feat-gap01-unified-orchestration-knowledge-state-plan
preset: builtin:ce-executor-pipeline
run_dir: ../worktree/ralph-orchestrator/2026-08-13-001-feat-gap01-unified-orchestration-knowledge-state-plan
status: 部分偏离，经过多次纠正后成功收敛；主问题是交接产物与 Unit 结算 payload 不一致
diagnostics_mode: MINIMAL
history_search: disabled
execution_capabilities: [single-chain]
---

# ce-executor-pipeline Loop `2026-08-13-001-feat-gap01-unified-orchestration-knowledge-state-plan` 运行链路诊断报告

> **生成时间**：2026-08-13
>
> **诊断对象**：`../worktree/ralph-orchestrator/2026-08-13-001-feat-gap01-unified-orchestration-knowledge-state-plan/.ralph/`
>
> **可信事件源**：`.ralph/current-events` 指向的唯一文件 `.ralph/events-20260812-165220.jsonl`。
>
> **对照配置**：`presets/en/ce-executor-pipeline.yml`、`presets/schemas/ce-executor-pipeline.yml`、运行时 `ralph.pipeline.yml`。
>
> **历史开关**：`disabled`。未读取主仓历史报告、解决方案、旧计划、brainstorm，也未读取本 run 的 `events-history-*` 与 `.ralph/history.jsonl`。

## 0. 产物盘点（Phase 0）

`execution_capabilities: [single-chain]`

- preset 是 `execution_mode: isolated` 的扁平线性链，未声明 `event_loop.supervisor.enabled: true`，事件中没有 `wave_id`。
- `.ralph/supervisor.db` 虽然存在，但它是默认 wave 路径留下的可用账本证据；按 capability 规则不能据此把本 run 判成 supervisor/wave run，也不能把它当故障。
- 任务功能由 preset 明确关闭（`tasks.enabled: false`），因此 `tasks.jsonl` 为空、无 `progress.md` 都是预期。

| Tier | 路径 | 存在 | 行数/规模 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` | 是 | 1 行 | 指向 `.ralph/events-20260812-165220.jsonl` |
| S | 指针指向的 events | 是 | 22 行 | 唯一编排事实源；含 2 个 `LOOP_COMPLETE` 候选，首个被拒收、末个被接受 |
| S | `.ralph/ledger.jsonl` | 是 | 28 行 | 记录 3 类拒收/完成边界及最终 `completion_honored` |
| S | `.ralph/recovery.jsonl` | 是 | 1 行 | 首次 `work.done` precheck 拒收 |
| S | `.ralph/current-loop-id` | 是 | 1 行 | 与目标 loop 一致 |
| S | `.ralph/loops.json` | 是 | `loops: []` | 终止后无活动 loop，符合预期 |
| S | `.ralph/loop.lock` | 否 | — | 已释放，非异常终止 |
| A | `.ralph/agent/tasks.jsonl` | 是 | 0 行 | `tasks.enabled=false`，预期 |
| A | `.ralph/agent/summary.md` | 是 | 42 行 | 标记 Completed successfully；需以事件时序修正为“恢复后成功” |
| A | `.ralph/agent/handoff.md` | 是 | 53 行 | 终止后 handoff，记录了首次拒收与修正结算 |
| A | `.ralph/agent/decisions.md` | 是 | 10 行 | 含 U1–U4 结算和首次 payload 修正证据 |
| B | `.ralph/diagnostics/2026-08-13T00-52-20/` | 是 | 4 个文件 | 有 recovery/summary/drift；无 orchestration/agent-output，因此为 `MINIMAL` |
| B | `.ralph/diagnostics/logs/ralph-2026-08-13T00-52-20-888-42922.log` | 是 | 146 行 | 有 channel fallback、completion rejection、recovery 日志 |
| B | `.ralph/supervisor.db` | 是 | 136 KiB | capability 不要求；仅作存在性记录 |
| C | `.ralph/review/<plan>/` | 是 | 33 个文件 | 含 normalized/review/fix/verification/report 产物 |
| C | `.ralph/review/<plan>/trace.md` | 终止后存在 | 72 行 | 文件正文明确写明是 executor 的 retroactive write；首次 precheck 时不存在 |

### 0.1 诊断盲区

- `MINIMAL` 没有 `agent-output.jsonl`，因此不能证明某个 agent 是否执行了 `ralph emit --policy-check`，也不能把空 channel 的直接原因归到 agent 崩溃、超时或错误命令。
- `ralph inspect loop --format json` 在本次终止后输出的是 `.ralph/events.jsonl`、大小 0；当前代码 `crates/ralph-cli/src/commands/inspect.rs:1444-1448` 固定拼接该默认路径，没有跟随 `.ralph/current-events`。本报告因此只用 current-events 指针和 ledger 对账事件，不采用 inspect 的 `events_file/events_size` 结论。

## 1. 结论摘要

### 1.1 健康度

- **判定**：部分偏离，恢复后成功；不是 silent-success，也不是死锁。
- **P0 / P1 / P2 数量**（均满足置信度门槛）：`P0=0`、`P1=1`、`P2=2`。
- **最高根因置信度**：DEV-001，`85/100`。
- **最终终态**：最后一条 `LOOP_COMPLETE` 在可信事件序列中出现，ledger 第 28 行记录 `completion_honored`；最终 verdict 为 `pass_with_residuals`。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 执行最终合规，首轮交接不合规；OPAC 只能弱审计 | `work.done` 被 precheck 拒收后重发成功；`MINIMAL` 无 agent-output，OPAC 单项不作强结论 | 75 |
| Q2 | 基座机制是否正常生效？ | ✅ 主要机制生效，通道 fallback 有可靠性风险 | ledger 记录 payload 拒收、缺 `report.done` 拒收、最终 completion honored；`hat_channel.rs:79-98` 对空通道 fail-close 并记录 fallback | 85 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ 拓扑闭合，但两个交接顺序/产物时序发生偏离 | `trace_file` 先被引用后落盘；reporter 先发 `LOOP_COMPLETE`，后才出现 `report.done` | 80 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **compound：agent/preset 交接错误为主，机制拦截正确；通道原因未完全可证** | DEV-001 是主因；DEV-002/003 是可恢复的次级偏离 | 85 |

### 1.3 根因一句话

executor 首次把没有独立 deliverable commit 的验证型 `U4` 填入 `completed_units`，同时沿用了当时尚未落盘的 `trace_file`；该错误由 preset 的成功路径 precheck 正确拒收，修正为 `completed=[U1,U2,U3] / skipped=[U4]` 后恢复成功。根因置信度 **85/100**。

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| **首轮终态** | 首轮不是成功闭环：`work.done` 在 events 第 3 行提出，随后第 4 行 `work.done.rejected`；之后首个 `LOOP_COMPLETE` 候选也因缺少 `report.done` 被 ledger 拒收。 |
| **恢复状态** | **恢复后成功**：executor 重发合法 settlement，随后 review/fix/alignment/report 链闭合；最后 ledger 第 28 行记录 `completion_honored`。 |
| **最终代码状态** | HEAD 为 `cf1747f7`；当前 run 工作树干净；最终修复 U1/U2 已提交，U3 为文档残差收口。 |
| **一致性告警** | 不能把 summary 的 “Completed successfully” 反写成“首轮完整成功”；本 run 明确经历了拒收和恢复。 |

## 2. 执行链路对比

### 2.1 实际事件时间轴

| 顺序 | 实际事件 | 结果 | 诊断 |
|---:|---|---|---|
| 1 | `work.start` | 接受 | loop 启动 |
| 2 | `plan.ready` | 接受 | payload 引用 `trace_file`，但该文件在后续 precheck 时不存在 |
| 3 | `work.done.proposed` | 提出 | `completed_units=[U1,U2,U3,U4]`、`commit_count=3` |
| 4 | `work.done.rejected` | 拒收 | failed checks `[3,4,5]`：trace 不存在、完成 Unit 与 commit 数不一致、U4 无 deliverable commit |
| 5–6 | `work.done.proposed` → `work.done` | 恢复成功 | 改为 `completed=[U1,U2,U3]`、`skipped=[U4]`、`execution_status=partial` |
| 7–13 | `stabilization.done` + 6 个维度 review | 接受 | 主链继续闭合 |
| 14–16 | `review.synthesized` + 两个 `review.complete` | 恢复成功 | fix-planner 曾空 channel，missing-terminal recovery 后补齐 |
| 17–18 | `fix.done.proposed` → `fix.done` | 接受 | fixer 成功落地 U1/U2 修复，U3 文档残差跳过 |
| 19 | `align.done` | 接受 | worktree clean |
| 20 | `LOOP_COMPLETE` | 拒收 | ledger 说明缺 `report.done` |
| 21 | `report.done` | 接受 | report 文件已写入 |
| 22 | `LOOP_COMPLETE` | 接受 | ledger `completion_honored` |

### 2.2 预期与实际

| 阶段 | 预期 | 实际 | 状态 |
|---|---|---|---|
| 计划审查 | 先写 normalized/trace，再发 `plan.ready` | `plan.ready` 先引用路径，trace 后由 executor retroactive 写入 | ⚠️ |
| executor 结算 | 每个 `completed_unit` 对应 deliverable commit；验证型 Unit 应进 `skipped` 或其它合法结算桶 | U4 无 commit 但被列为 completed | ❌ |
| precheck | 拒收不可验证/自相矛盾的成功 payload并定向恢复 | 正常执行 | ✅ |
| review/fix 主链 | 线性闭合 | 闭合；fix-planner 空 channel 后恢复 | ⚠️ |
| reporter | 先 `report.done`，再 `LOOP_COMPLETE` | 先提出 `LOOP_COMPLETE`，后补 `report.done` | ⚠️ |
| 最终终态 | `report.done` 后 accepted `LOOP_COMPLETE` | 第二次尝试符合预期 | ✅ |

未触发的 wave fan-out、runtime task ledger、`plan.blocked`、`work.failed` 均不是本 preset 本次成功路径的缺失项。

## 3. 历史问题上下文

`history_search=disabled`。本节不扫描历史文档；历史关联统一记为 `N/A (history disabled)`。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | 首次 `work.done` 把无 commit 的 U4 申报为 completed，且 `trace_file` 当时不可读 | events:L3-L6；ledger:L4；recovery.jsonl:L1；`decisions.md:6`；preset:L133-L138 | P1 | 85 | 双账本(+20) + preset 行号(+15) + Tier C 交叉验证(+10)；MINIMAL 模式上限 85 | 无 agent-output，无法判断是 agent 忘写、写入时序竞态还是执行器后置生成 |
| DEV-002 | isolated hat channel 多次为空，runner 走 main events fallback | 当前 log:L21-L23、L45-L47、L68-L70、L76-L78、L84-L86、L92-L95、L136-L138；7 个 fallback 文档；hat_channel.rs:79-98 | P2 | 75 | file:line(+25) + events/logs 交叉证据(+10) | 缺 agent-output/backend 退出状态，无法确认空 channel 的直接诱因，也未证明业务事件丢失 |
| DEV-003 | reporter 首次提出 `LOOP_COMPLETE` 时 `report.done` 尚未被 accepted | events:L20-L22；ledger:L22-L28；preset:L5417-L5443；parse_and_emit.rs:2954-2990 | P2 | 85 | file:line(+25) + 双账本(+20) + preset 行号(+15)；MINIMAL 上限 85 | 缺 agent-output，无法证明是 reporter 顺序错误还是输出/通道合并时序造成 |

### 4.1 OPAC 逐 hat 审计（MINIMAL）

| Hat/阶段 | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| plan-reviewer | ⚠️ | N/A | ⚠️ | ✅ | `plan.ready` accepted；trace 产物时序不成立；无 agent-output | 40 |
| executor | ⚠️ | ✅（被 runtime precheck 拦截） | ⚠️ | ✅（修正后 `work.done` accepted） | events:L3-L6、ledger:L4；无法核对 agent 自己的 policy-check | 50 |
| precheck-work.done | ✅ | ✅ | ✅ | ✅ | `work.done.rejected` + recovery + corrected `work.done` | 70 |
| fix-planner | ⚠️ | N/A | ⚠️ | ✅（recovery 后） | session recovery `missing_terminal_emit`；空 channel fallback | 45 |
| reporter | ⚠️ | ✅（completion guard） | ⚠️ | ✅（第二次） | 首个 `LOOP_COMPLETE` 被拒，`report.done` 后第二次成功 | 50 |

> `MINIMAL` 模式下没有 `agent-output.jsonl`；OPAC 单项不超过 50 的 agent/OPAC 推断不作为 P0 根因。runtime precheck 和 completion guard 的“拒收”是机制证据，不等价于 agent 已执行正确的 Observe/Precheck 命令。

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P1 | 首次 `work.done` 的 Unit 结算与证据文件不成立：U4 无 deliverable commit 却列入 `completed_units`，`trace_file` 在 gate 时不存在 | **compound（agent/preset 交接）**：agent payload 错填；preset 虽声明“先写 trace”，但 `plan.ready` 前没有可执行的文件存在性 gate | **85** | DEV-001 | 双账本(+20) + preset 行号(+15) + Tier C(+10)；MINIMAL 硬顶 85 | `N/A (history disabled)` | 第 1 轮：读 preset/schema 行号 + 当前 trace retroactive provenance；第 2 轮：events/ledger/recovery 双账本对账，达到 85 |
| P2 | isolated hat channel 在 7 个 activation 为空并回退主事件文件；本次未证明业务事件丢失，但隔离通道确认不可审计 | **mechanism 风险**（空 channel fail-close + fallback） | **75** | DEV-002 | file:line(+25) + events/logs/Tier B 交叉(+10)；MINIMAL 硬顶 85 | `N/A (history disabled)` | 第 1 轮：读 `merge_hat_channel` 与 runner fallback 源码；第 2 轮：对照日志、fallback 文档和主 events，未发现终态缺失 |
| P2 | reporter 首次发 `LOOP_COMPLETE` 早于 accepted `report.done`，浪费一次迭代并触发 completion correction | **compound（agent/preset 顺序）**；runtime guard 正常 | **85** | DEV-003 | file:line(+25) + 双账本(+20) + preset 行号(+15)；MINIMAL 硬顶 85 | `N/A (history disabled)` | 第 1 轮：读 reporter 明确“report.done followed by LOOP_COMPLETE”契约与 completion guard；第 2 轮：events/ledger 时序确认第一次 rejected、第二次 honored |

## 6. 修复建议

### 6.1 短期（operator workaround）

- 复跑同类 plan 时，先确认 `.ralph/review/<plan>/trace.md`、`normalized-plan.md`、`reuse-guidance.md` 已非空，再允许 executor 进入 `work.done`。
- `completed_units` 只填写本次 diff range 内确实有 deliverable commit 的 Unit；纯验证型 Unit 必须按 preset 允许的 `skipped_units`/residual 语义结算，不要为满足“每 Unit 都有结果”而伪造 commit。
- reporter 必须把 `report.done` 作为先行事件，确认 accepted 后再发 `LOOP_COMPLETE`；遇到第一次 completion rejection，应等待 correction，不要把第一次候选当作终态。

### 6.2 中期（preset/schema/instructions）

- 在 plan-reviewer 的 `plan.ready` 发送前增加明确、可执行的 `test -s` 检查：`normalized_plan_file`、`trace_file`、`reuse_guidance_file` 必须存在且可读；检查失败就 `plan.blocked` 或不发送 `plan.ready`。关联 DEV-001，置信度 85。
- 把“验证型 Unit 没有 commit 时如何填 `completed_units`/`skipped_units`”写成一个具体示例，并让 executor 在 emit 前打印 `planned = completed ∪ failed ∪ blocked ∪ skipped` 的结算表。关联 DEV-001，置信度 85。
- 为 reporter 加一个结构化的两阶段终态约束或 activation 级 Confirm：只有 `report.done` accepted 后才允许 `LOOP_COMPLETE`。当前 runtime 已有 guard，但它是在 agent 违规后才纠正。关联 DEV-003，置信度 85。

### 6.3 长期（机制/底座）

- 补充 isolated channel 的诊断字段：backend 退出码、channel 创建时间、最后写入时间、stdout/stderr 是否包含业务事件、当前 marker 指向；否则只能看到“空了”，不能归因。关联 DEV-002，置信度 75。
- 让 `ralph inspect loop` 遵循 `.ralph/current-events` 的路径解析规则，或在终止 run 读取 current-events 指针；当前 `inspect.rs:1444-1448` 固定 `.ralph/events.jsonl`，会把真实 run 误显示为 0 条事件。这是诊断工具问题，不是本次业务链路的主因。

## 7. 未核实疑点与边界

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| 空 hat-channel 的直接原因是 agent 没有调用 emit、backend 崩溃/超时，还是 marker/路径竞态 | 45 | 缺 `agent-output.jsonl` 与 backend activation 退出状态 | 已查 7 个 fallback 文档、当前 log、`merge_hat_channel`/runner 源码和主 events；只确认“channel 为空并回退”，未猜测具体诱因 |
| `trace_file` 初始缺失是否由 plan-reviewer 写入竞态而非单纯漏写 | 55 | 缺 plan-reviewer activation 的输出/写文件时间线 | 已查 `plan.ready` payload、preset artifact-first 条款和终止后的 trace retroactive provenance；不能再提高到定论 |

## 8. 关键代码与配置引用清单

- `presets/en/ce-executor-pipeline.yml:133-138`：`work.done` precheck 要求每个 completed Unit 有 deliverable commit、验证文件可读、Unit settlement 一致。
- `presets/en/ce-executor-pipeline.yml:2161-2179`：plan-reviewer 应创建 normalized projection 与 append-only trace，再发送带 `trace_file` 的 `plan.ready`。
- `presets/en/ce-executor-pipeline.yml:5417-5443`：reporter 明确要求 `report.done` 后再 `LOOP_COMPLETE`。
- `crates/ralph-cli/src/loop_runner/hat_channel.rs:79-98`：空 isolated channel 记录 fallback、删除 marker/channel 并返回错误。
- `crates/ralph-cli/src/loop_runner/inner.rs:4456-4479`：空 terminal channel 触发同一 hat 的 missing-terminal recovery。
- `crates/ralph-core/src/event_loop/event_processing.rs:553-640`：missing-terminal recovery 的 retry key、预算和最终阻断路径。
- `crates/ralph-core/src/event_loop/parse_and_emit.rs:2954-2990`：缺少 required event 时拒收 `LOOP_COMPLETE`，不把候选事件加入 accepted stream。
- `crates/ralph-cli/src/commands/inspect.rs:1444-1448`：inspect loop 当前固定使用 `.ralph/events.jsonl`，未跟随 `.ralph/current-events`。

## 9. 最终判定

这次不是“代码实现完全失败”，而是一次**恢复后成功**的 run：业务链最终闭合，runtime 的拒收、恢复、终态 guard 都按预期发挥作用。需要优先修正的是 **plan-reviewer/executor 的 artifact-first 与 Unit settlement 交接**；其次是 reporter 的终态顺序和 isolated channel 的可观测性。历史检索保持禁用，以上结论只基于本次 run 产物、当前 preset/schema 和当前源码。
