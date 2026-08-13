---
title: "builtin:ce-executor-pipeline Loop `2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan` 运行链路诊断报告"
date: 2026-08-13
type: diagnosis
loop_id: 2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan
preset: builtin:ce-executor-pipeline
run_dir: ../worktree/ralph-orchestrator/2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan
status: "P0：预存 dirty 被误识别为 dimension reviewer scope violation，loop 在第 7 轮硬终止；P1：isolated hat-channel 两次为空并回退"
diagnostics_mode: MINIMAL
bundle: finalized
bundle_path: ../worktree/ralph-orchestrator/2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan/.ralph/diagnostics/2026-08-13T12-50-19/diagnosis-input.json
history_search: full
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: present
evidence_gaps: []
execution_capabilities: [runner]
---

# builtin:ce-executor-pipeline Loop `2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan` 运行链路诊断报告

> **生成时间**：2026-08-13
>
> **诊断对象**：`../worktree/ralph-orchestrator/2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan/.ralph/`
>
> **对照**：`presets/en/ce-executor-pipeline.yml`、`presets/schemas/ce-executor-pipeline.yml`、plan `docs/plans/2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan.md`
>
> **历史范围**：`full`。用户明确允许查看历史；扫描了 `docs/report/`、指定的 `docs/solutions/`、`docs/plans/` 和 `docs/brainstorms/`。

## 0. 产物盘点（Phase 0）

本次 run 的真正 workspace 不是主仓，而是 `.ralph/loops.json` 指向的外置复用 worktree。主仓仅有诊断 session 壳，缺少 `current-events`；若从主仓诊断会读错对象。

**execution_capabilities**：bundle 原样报告 `[runner]`，`diagnosis-input.json` 同时记录 `execution_capability: isolated`、`worktree: true`。preset 的执行模式为 `isolated`，未配置 `event_loop.supervisor.enabled: true`，可信业务 events 无 `wave_id`；`.ralph/supervisor.db` 虽存在且日志显示被 default-wave 路径拾取，但不足以把本次业务链改判为 wave/supervisor 故障。

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---|---|
| S | `.ralph/current-events` → `.ralph/events-20260813-045019.jsonl` | 是 | 6 | 唯一可信业务 events；无 `report.done` / `LOOP_COMPLETE` |
| S | `.ralph/events-history-20260813-045019.jsonl` | 是 | 2 | 旁路记录：bootstrap 与 `loop.terminate`，不覆盖 current-events |
| S | `.ralph/ledger.jsonl` | 是 | 8 | iteration 2、4、5、6 的 accepted observation/batch commit |
| S | `.ralph/recovery.jsonl` | 是 | 1 | workspace 侧 repair-stream 记录 |
| S | `.ralph/loop-termination-reason.json` | 是 | — | `scope_violation_hard_rejected`，hat=`dim:goal-alignment` |
| S | `.ralph/loops.json` / `current-loop-id` | 是 | 2 / 0 | loop 身份与外置 worktree 可对上 |
| A | `.ralph/agent/summary.md` | 是 | 29 | 明确写入 Failed；有 U4 最终 commit，但不是 loop 成功 |
| A | `.ralph/agent/tasks.jsonl` | 是 | 0 | preset `tasks.enabled: false`，预期 |
| A | `.ralph/agent/accepted-transitions.jsonl` | 是 | 4 | `plan.ready`、`work.done.proposed`、`work.done`、`stabilization.done` |
| B | `.ralph/diagnostics/2026-08-13T12-50-19/` | 是 | MINIMAL | bundle finalized；trace 24 行、feedback 8 行、recovery 7 行；无 orchestration/errors |
| B | `.ralph/diagnostics/logs/ralph-2026-08-13T12-50-19-442-54251.log` | 是 | 168 | 关键退出与两次空 channel 证据 |
| B | `.ralph/supervisor.db` | 是 | 139264 bytes | 仅作产物/默认 wave 路径证据，本次无 `wave_id` |
| C | `.ralph/review/<plan>/` | 是 | 多份 | baseline/final verification、stabilization audit、goal-alignment、trace、reuse guidance |

Bundle 读取结果：`manifest_status=finalized`；`runtime-trace.jsonl` 24 条、sequence 1–24 单调、无坏行；`feedback.jsonl` 与 session recovery 均可读；`ralph diagnose --legacy --session latest --diagnostics-root <run>/.ralph/diagnostics --format json` 成功。

## 1. 结论摘要

### 1.1 健康度

- **判定**：失败终止，非随机 quit。代码执行和测试稳定化已完成，但六维 review 链在第一维入口被错误的 dirty-handoff 硬门禁截断。
- **P0**：1 条，置信度 95/100。
- **P1**：1 条，置信度 85/100（空 channel 症状与 transport fallback 已证实；agent/backend 的最后原因因 MINIMAL 证据不足，不作定论）。
- **最高优先级根因置信度**：DEV-001 = **95/100**。
- **历史复发**：空 channel 家族是；本次 dirty-handoff 与历史报告中的同类空 channel 不是同一个已证实根因。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 部分可判定 | MINIMAL 无 agent-output/orchestration，无法证明每个 agent 的完整 O/P/A/C；prompt visibility 本身无明显缺口 | 50 |
| Q2 | 基座机制是否正常生效？ | ❌ 部分失效 | accepted event、recovery、typed hard termination 都工作；但 `git diff --stat HEAD` 没有 activation 起点，预存 dirty 被当成 reviewer 新改动 | 90 |
| Q3 | 编排是否合理、正常运行？ | ❌ 未完成 | `work.done → stabilization.done → dim:goal-alignment` 成功到达，之后未形成 correctness/reporter/LOOP_COMPLETE | 90 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **机制 + preset 契约冲突为主；agent 不可判定** | runtime audit 的比较基线错误，preset 又把任何 dirty carryover 定为 P0；无 agent-output，不能把空 channel 归咎 agent | 95 |

### 1.3 根因一句话

`dim:goal-alignment` 的 handoff precheck 按设计拒绝外部 dirty worktree，但 runtime 的全局文件审计又以当前 `HEAD` 为唯一基线；在本计划明确要求“保留用户未提交修改”的 `--reuse-worktree` 场景下，38 个预存路径被误判成 reviewer scope violation，触发首个维度审计的 hard-reject，loop 因此退出。**置信度：95/100。**

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| 首轮终态 | 失败。current-events 最后一个业务事件是 `review.goalalign.done`；其后没有 accepted `report.done` 或 `LOOP_COMPLETE`。termination 文件与日志明确为 `scope_violation_hard_rejected`。 |
| 恢复状态 | 无最终 hard-reject 后的恢复；此前两次 missing-terminal recovery 曾成功使 `plan.ready`、`work.done.proposed` 继续推进。 |
| 最终代码状态 | worktree `HEAD=dada34ff`，executor 已提交 U1–U4 四个 GAP-02 commit；另有 38 个 `.ralph/` 外未提交路径，start/end 证据显示在 goal-alignment activation 前后未变化。 |
| 一致性告警 | ⚠️ 代码交付/681 tests green 不等于 loop 成功；没有 accepted `report.done` / `LOOP_COMPLETE`，不得把最终代码状态反写成完整成功。 |

## 2. 执行链路对比

| 轮次 | 激活 hat | 预期终态 | 实际 | 判定 |
|---:|---|---|---|---|
| 1 | `plan-reviewer` | `plan.ready` / `plan.blocked` | isolated channel 为空，触发 `missing_terminal_emit` | ⚠️ transport 退化，进入定向恢复 |
| 2 | `plan-reviewer` | `plan.ready` | `plan.ready` accepted；ledger 记录 batch commit | ✅ |
| 3 | `executor` | `work.done.proposed` / `work.failed.proposed` | isolated channel 为空，触发 `missing_terminal_emit` | ⚠️ transport 退化，进入定向恢复 |
| 4 | `executor` | `work.done.proposed` | `work.done.proposed` accepted | ✅ |
| 5 | `precheck-work.done` | `work.done` | `work.done` accepted | ✅ |
| 6 | `test-stabilizer` | `stabilization.done` / `stabilization.blocked` | `stabilization.done` accepted；681/681 scoped tests | ✅ |
| 7 | `dim:goal-alignment` | `review.goalalign.done` | reviewer 发现 38 个 dirty paths，写 P0 `handoff_precheck_failed`，事件发出后全局 audit hard-reject | ❌ |

预期链路应继续：

```text
stabilization.done
  → dim:goal-alignment
  → dim:correctness → dim:testing → dim:maintainability
  → dim:project-standards → dim:adversarial
  → review.synthesized → review.complete → fix.done → align.done
  → report.done → LOOP_COMPLETE
```

实际链路在第一维 reviewer 的 handoff precheck 后终止；后续 5 个 dimension、synthesizer、fixer、alignment、reporter 均无 activation 证据。

## 3. 历史问题上下文

| 历史材料 | 关联度 | 观察 |
|---|---:|---|
| `docs/report/2026-08-13-ce-executor-pipeline-2026-08-13-001-feat-gap01-unified-orchestration-knowledge-state-plan-diagnosis.md` | 高 | 同 preset、同一轮前置 hat；`plan-reviewer` 与 `executor` 也出现空 isolated channel/fallback。 |
| `docs/report/2026-08-12-ce-executor-pipeline-2026-08-12-001-diagnosis.md` | 高 | 同 preset 的 executor、test-stabilizer、goal-alignment transport/编排问题；报告确认空 channel、目标 handoff 失效与终态链断裂。 |
| `docs/report/2026-08-08-parallel-forge-primary-20260808-021642-diagnosis.md` | 中-高 | 历史报告统计 30 天内 `hat_channel_empty_after_activation` 命中 30 次，且 `channel routing fallback` 持续出现。不同 preset，但属于同一 isolated transport 家族。 |
| `docs/report/2026-07-23-ce-executor-supervisor-primary-20260723-082003-diagnosis.md` | 中 | hat-channel fallback/orphan/no-progress 家族的早期记录。 |
| `docs/report/2026-07-27-implementation-review-primary-20260727-111552-diagnosis.md` | 中 | `handoff_precheck_failed` 曾在并发 review 中出现，但原因是 patch artifact 被覆盖，不是本次 dirty carryover。 |

**本次扫描窗口：full (full-history)。**

历史对照结论：空 channel 是复发问题；本次“预存 dirty 被 dimension reviewer 当作自身 scope violation”的确切证据在历史材料中未找到，属于新暴露的 preset/runtime 契约冲突。历史材料仅用于复发确认，当前根因以本次 run 产物和当前源码为准。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度 | 置信度 | 根因分类 |
|---|---|---|---|---:|---|
| DEV-001 | reviewer 的 clean-handoff gate 与 reuse-worktree/foreign dirt 约束冲突；全局审计没有按 activation 起点排除预存 dirty | run `git-state-goal-alignment-start.txt` / `-end.txt`；plan:20、889；preset:3438–3478；`dispatch_and_handoff.rs:970–1064`；log:51–168 | P0 | 95 | compound：机制 60% + preset 40% |
| DEV-002 | `plan-reviewer`、`executor` 两次 activation 的 isolated hat-channel 均为空，merge 失败后回退主事件流 | log:10–12、25–27；两个 `channel-routing-fallback-*.md`；`hat_channel.rs:79–98`；session runtime trace:1–14 | P1 | 85 | mechanism（具体 backend/agent 原因未核实） |
| DEV-003 | recovery idempotent log 在后续同 retry key 写入时出现 `_final=true` 拒绝，反馈状态出现 Recovered 后又 Pending | log:28、40；session `feedback.jsonl`:3–8；session `recovery.jsonl`:3–7 | P2 | 80 | mechanism/observability |

### 4.1 OPAC 逐 hat 审计（MINIMAL）

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| `plan-reviewer` | ✅ | ⚠️ | ⚠️ | ✅ | 首轮空 channel；第二次 `plan.ready` accepted；无 agent-output | 50 |
| `executor` | ✅ | ⚠️ | ⚠️ | ✅ | 首轮空 channel；第二次 `work.done.proposed` accepted；无 agent-output | 50 |
| `precheck-work.done` | ✅ | ✅ | ✅ | ✅ | `work.done` accepted | 60 |
| `test-stabilizer` | ✅ | ⚠️ | ✅ | ✅ | `stabilization.done` accepted，审计文件宣称 681/681；无 agent-output | 50 |
| `dim:goal-alignment` | ✅ | ⚠️ | ✅ | ❌ | 写出 P0 handoff finding 并发出 review event，但随后被全局 scope hard-reject | 50 |
| 后续五个 dimension / synth / fixer / reporter | ⚠️ | N/A | ❌ | N/A | 没有 activation | 40 |

MINIMAL 模式下 agent/OPAC 单项置信度受证据上限约束；没有 agent-output，不能据此判断某个 agent 未加载 skill、未执行 policy-check 或主动 quit。

### 4.2 Prompt visibility 对账

对 `plan-reviewer`、`executor`、`dim:goal-alignment` 执行 `ralph -c presets/en/ce-executor-pipeline.yml inspect prompt --hat <hat> --format json`，三者结果一致：

- `auto_inject[].name`：`ralph-tools`、`ralph-tools-memories`、`ralph-tools-opac`。
- `on_demand[].name`：`ralph-tools-cmdref`、`ralph-tools-emit`、`ralph-tools-precheck`、`ralph-tools-recovery-directives`、`ralph-tools-tasks`、`ralph-tools-wave`。

没有发现把 on-demand skill 宣称为 auto-inject 的证据；preset instructions 也要求在 emit 前加载 `ralph-tools-emit`。由于缺 agent-output，本次不能证明实际 tool load/emit 是否发生；但没有 prompt visibility 证据支持“agent 看不到 skill”这一归因。

## 5. 归因与置信度

| 优先级 | 问题 | 根因 | 置信度 | 计分证据 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---:|
| P0 | 预存 dirty 被判为 reviewer scope violation，首个 dimension activation 触发 loop hard-reject | **compound：mechanism + preset**。机制 `audit_file_modifications` 以 `git diff --stat HEAD` 判断 reviewer 是否修改文件，没有保存 activation 前快照；preset reviewer 又把任意 `.ralph/` 外 dirty carryover 直接写为 P0/block。 | **95** | 精确日志/终止文件 + start/end 双证 + preset/plan 行号 + 当前源码行号 | 本次是新组合；历史有 handoff precheck 家族，但非同一 dirty 证据 | 2：源码反查；preset/plan 契约反查 |
| P1 | isolated channel 为空并回退主事件流 | **mechanism**；空 channel 与 fallback 已确定，agent/backend 最后原因未确定 | **85** | 两次日志 + 两个 fallback artifact + runtime trace + `hat_channel.rs` | 高；同 preset 2026-08-12/08-13-001，跨 preset 30 天多次 | 2：日志/产物；历史对照 |
| P2 | recovery outcome 反复 Pending/Recovered，重复 final key 写入被拒 | **mechanism/observability**；当前实现继续运行，未证明它造成最终退出 | **80** | log + feedback + recovery 三账本 | 中；与历史 recovery provenance 分叉同族 | 1：三账本对账 |

DEV-001 的主因贡献比例：mechanism 60%、preset 40%；agent 成分 0%（证据不足，不代表 agent 一定无问题）。

## 6. 机制生效矩阵

| 检查项 | 状态 | 证据 |
|---|---|---|
| Event origin / hat scope | ✅ | accepted events 的 source/hat 与 preset 允许发布拓扑一致；未见业务 origin rejection |
| Payload contract | ✅ | `plan.ready`、`work.done.proposed`、`work.done`、`stabilization.done`、`review.goalalign.done` 均被接受 |
| Execution contract | ✅ | `work.done` 经过 `precheck-work.done` 后 accepted |
| Workflow guard / phase | ⚠️ | 已按顺序推进到 goal-alignment，但 dirty precheck 使 review 链无法继续 |
| Isolated 单事件预算 | ⚠️ | accepted batch 每轮最多一个；两轮 channel 为空，transport 退化 |
| step_handoff / tasks | N/A | `tasks.enabled=false`；没有 tasks ledger 证据，不把缺失当故障 |
| Recovery 升级 | ⚠️ | 两次 missing-terminal recovery 可恢复；最终 scope hard-reject 不再恢复 |
| `task.resume` 消费 | ⚠️ | recovery 记录目标 hat 并重新激活 producer；缺完整 orchestration，无法证明所有 transport 细节 |
| Stall / progressive failure | ✅ | 本次不是 stall breaker；typed scope termination 正确触发 |
| Drift monitor | ✅ | session drift 0 findings；不能把 recovery outcome update 当 drift finding |
| Dedup / duplicate work | ✅ | current-events 未见重复业务终态；accepted transitions 四条与主业务链相符 |
| Terminal / silent-success | ✅ | 没有 `report.done` / `LOOP_COMPLETE`，summary 与 termination 明确失败；未误报成功 |
| Event-artifact temporal consistency | ⚠️ | 失败终态与最终代码树一致，但 `loop.terminate` 在 events-history 旁路而非 current-events；应保留为终态 provenance gap，不改写为成功 |

## 7. 非执行性修复建议

### 短期（人工操作）

- 不要在同一个 dirty reuse-worktree 上直接重跑并把结果当作修复验证。人工选择一种可审计边界：提交/暂存/转移那 38 个既有修改，或为 loop 使用干净 worktree；不要无确认地清理用户修改。
- 本次 GAP-02 的四个 commit 与 681/681 验证产物应保留为“代码交付证据”，但不能替代未发生的六维 review 与 reporter 终态。

### 中期（runtime / preset）

- 为每个受限 hat 在 activation 开始记录 dirty path + content identity，结束时只报告相对该起点新增/变化的路径；不要用 `git diff --stat HEAD` 作为“本 activation 是否写盘”的判断。
- 让 `stabilization.done` 的 `foreign_dirt`/baseline 证据成为 downstream reviewer 的共享输入：reviewer 只审 plan diff 与 activation 新增变化，不把明确标注且前后未变的 foreign dirt 判为自身 scope violation。
- 明确 clean-handoff、reuse-worktree、foreign-dirt 三者的契约：若 preset 选择 clean-only，就应在 loop 启动前 fail-fast；若允许保留 dirty，就不能在 dimension activation 中把它升级为 P0 hard termination。
- 对首次空 isolated channel 记录 backend exit code、stderr、channel path、marker 生命周期和 emit policy result；当前 fallback diagnostic 只能说明“空了”，不能说明“为什么空”。

### 长期（回归门禁）

- 增加真实 EventLoop/CLI nextest 场景：预置一个与 plan 无关且前后不变的 dirty 文件，运行 read-only dimension reviewer，断言不会产生 scope violation；再增加 reviewer activation 期间新增文件的正例，断言仍会 hard-reject。
- 增加 isolated channel crash/empty、targeted recovery、主 ledger accepted event 和最终 termination provenance 的集成回归；历史上空 channel 已多次复发，不能只靠日志提示。

## 8. 未核实疑点

| 疑点 | 当前置信度 | blocked_by |
|---|---:|---|
| `plan-reviewer`/`executor` 空 channel 是 agent 未 emit、backend 提前退出、marker race，还是 emit 被 policy 拒绝 | 50 | 缺 `agent-output`、backend exit code/stderr、FULL orchestration |
| `loop.terminate` 为何只出现在 events-history 而不在 current-events | 55 | 本次 bundle 没有 orchestration/errors，且 current-events 是最终 6 行业务流 |
| recovery outcome 的 Pending 回写是否会影响未来 resume 判定 | 45 | 本次最终 hard termination 前未发生后续 resume，缺 recovery consumer 运行证据 |

## 9. 关键源码与产物引用

- `presets/en/ce-executor-pipeline.yml:3438-3478`：dimension reviewer clean-handoff entry precheck 与 P0 `handoff_precheck_failed` emit。
- `crates/ralph-core/src/event_loop/dispatch_and_handoff.rs:946-978`：受限 hat 结束后用 `git diff --stat HEAD` 检查文件变化。
- `crates/ralph-core/src/event_loop/dispatch_and_handoff.rs:996-1064`：`dim:*` reviewer 的 scope violation 被提升为 `BlockLoop` 并推入 typed termination trigger。
- `crates/ralph-core/src/event_loop/wave_scope.rs:466-478`：下一次 termination check 消费 trigger。
- `crates/ralph-core/src/event_loop/termination.rs:153-167`：trigger 转为 `ScopeViolationHardRejected`。
- `crates/ralph-cli/src/loop_runner/hat_channel.rs:79-98`：空 isolated channel 记录 fallback 并返回错误。
- run `.ralph/diagnostics/logs/ralph-2026-08-13T12-50-19-442-54251.log:51-168`：dirty scope violation、typed hard termination、最终 wrap-up。
- run `.ralph/review/2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan/git-state-goal-alignment-start.txt` 与 `-end.txt`：38 条 dirty paths 在 reviewer activation 前后不变。

## 10. 诊断边界与提交检查

- [x] Phase 0 产物盘点、唯一 `current-events`、bundle-first 已完成。
- [x] Diagnostics 模式按 MINIMAL 处理，未因缺 orchestration 单独升级 P0。
- [x] execution capability 已记录为 bundle 原值 `[runner]`；未因单链无 `wave_id` 或 default `supervisor.db` 缺失/存在误判故障。
- [x] P0 置信度 ≥70，所有 §5 条目置信度 ≥60；低于 60 的具体空 channel 原因移入 §8。
- [x] 已执行三组 hat 的 prompt visibility 对账。
- [x] 历史检索状态已写入 frontmatter：`full`；§3 已写扫描窗口。
- [x] 已按 SSOT 护栏完成过时概念与错误路径扫描，报告只使用当前 recovery/event 术语。
- [x] 诊断不修改代码、不执行修复命令；只在主仓写入本最终报告。
- [x] 诊断中间文件均位于临时目录；临时目录已清理，不在 `docs/report/` 留 JSON/stderr/工作笔记。
