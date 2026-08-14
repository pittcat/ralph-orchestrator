---
title: builtin:red-team-attack Loop `primary-20260813-212249` 运行链路诊断报告
date: 2026-08-14
type: diagnosis
loop_id: primary-20260813-212249
preset: builtin:red-team-attack
run_dir: .
status: 失败终态未进入可信业务事件，loop 最终被人工取消
diagnostics_mode: MINIMAL
bundle: finalized
bundle_path: .ralph/diagnostics/2026-08-14T05-22-49/diagnosis-input.json
history_search: disabled
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: present
evidence_gaps: ["bundle 未包含 orchestration.jsonl、errors.jsonl 与原始 agent-output.jsonl，无法还原 Claude 进程的完整 stdout/stderr；但 agent 的 decisions.md 与 retry-board 已记录实际 emit 命令及 policy 返回值"]
---

# builtin:red-team-attack Loop `primary-20260813-212249` 运行链路诊断报告

> **生成时间**：2026-08-14
> **诊断对象**：`.ralph/`（loop_id=`primary-20260813-212249`）
> **对照 preset**：`presets/en/red-team-attack.yml` + `presets/schemas/red-team-attack.yml`
> **Diagnostics 模式**：MINIMAL；本报告不读取历史目录，`history_search=disabled`。
> **执行能力**：`["runner"]`。`supervisor.db` 虽存在，但本次 bundle 未声明 supervisor/wave 能力，缺 `wave_id` 不构成故障。

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---|---|
| S | `.ralph/current-events` → `.ralph/events-20260813-212249.jsonl` | 是 | 6 | 唯一可信 events 文件 |
| S | `.ralph/events-history-20260813-212249.jsonl` | 是 | 2 | 配对旁路，不作为业务拓扑 SSOT |
| S | `.ralph/ledger.jsonl` | 是 | 10 | 记录 4 个业务 accepted observation、取消及最终 iteration |
| S | `.ralph/recovery.jsonl` | 是 | 4 | 其中 1 条为本次 repair-stream；其余 3 条为更早 post-merge 记录 |
| S | `.ralph/current-loop-id` | 是 | — | `primary-20260813-212249` |
| S | `.ralph/loop-termination-reason.json` | 是 | `cancelled` | 最终终止原因 |
| A | `.ralph/agent/tasks.jsonl` | 条件不适用 | lock-only | preset `tasks.enabled: false` |
| A | `.ralph/agent/progress.md` | 否 | — | tasks/state projection 未启用，非故障 |
| A | `.ralph/agent/summary.md` | 是 | — | 终止后摘要 |
| A | `.ralph/agent/handoff.md` | 是 | — | 终止后 handoff |
| B | `.ralph/diagnostics/2026-08-14T05-22-49/` | 是 | `finalized` | bundle 入口存在 |
| B | `runtime-trace.jsonl` | 是 | 48 条，序列 1–48 单调 | 无坏行 |
| B | `feedback.jsonl` | 是 | 10 条 | 5 个 missing-terminal 生命周期记录 |
| B | session `recovery.jsonl` | 是 | 6 条 | 4 次 pending + 1 次 exhausted，另 1 条 doc sync |
| B | session `drift.jsonl` | 是 | 0 条 | 未发现 drift finding |
| B | `.ralph/supervisor.db` | 是 | — | 本次 capability 不要求，不作异常 |
| B | `.ralph/diagnostics/logs/*.log` | 是 | 2 个本次日志 | MINIMAL 下作为 OPAC/进程证据 |
| C | `.ralph/red-team/01-target-lock.md` | 是 | 71 | target-locker 已完成 |
| C | `.ralph/red-team/02-plan-resolution.md` | 是 | 73 | plan-resolver 已完成 |
| C | `.ralph/red-team/03-patch-reconstruction.md` | 是 | 102 | 已生成 |
| C | `.ralph/red-team/04-attack-surface.md` | 是 | 200 | 已生成 |
| C | `.ralph/red-team/05-experiment-plan.md` | 是 | 315 | 已生成 |
| C | `.ralph/red-team/experiments/RTE-001.md` | 是 | 117 | experiment-runner 已完成首个实验 |
| C | `.ralph/red-team/07-retry-board.md` | 是 | 123 | evidence-gate 的 retry-board；记录了实际 emit、policy 拒绝和恢复尝试 |
| C | `.ralph/agent/decisions.md` | 是 | 137 | 记录 evidence-gate 为什么停止重试，以及拒绝伪造事件/绕过 channel |
| C | `.ralph/agent/memories.md` | 是 | — | 记录 `redteam.retry.required` 的实际 `topic_denied` 结果 |
| C | `.ralph/red-team/REPORT.md` | 否 | — | reporter 未触发 |
| C | `.ralph/red-team/PLAN.md` | 否 | — | reporter 未触发 |
| C | `.ralph/red-team/QUESTIONS.md` | 否 | — | reporter 未触发 |

`ralph diagnose --legacy --session latest --diagnostics-root .ralph/diagnostics --format json --output <临时文件>` 返回的 bundle 为 `finalized`；`diagnosis-summary.json` 报告总 iteration=10。此前报告把可变的 Tier C 产物误报为缺失；本次重新盘点确认 `07-retry-board.md`、`decisions.md` 和 memories 均存在。可信 events 仍只有 6 条，且没有任何 `redteam.retry.required` 或 `redteam.evidence.gated`；Tier C 产物只能解释 agent 的动作，不能替代 accepted event 账本。

## 1. 结论摘要

### 1.1 健康度

- **判定**：失败终态未完成，随后被人工取消；不是成功闭环，也不是 silent-success。
- **P0/P1/P2**：P0=1，P1=1，P2=0；agent“忘记 emit”不成立，原始 backend stdout/stderr 仍是观测盲区。
- **最高优先级根因置信度**：P0-1 = **85/100**（MINIMAL 模式上限）；独立复现与 agent 产物均指向同一 preset deny 规则。
- **历史复发**：`N/A (history disabled)`。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ✅ 就关键 emit 路径可确认 | `decisions.md`/`07-retry-board.md` 记录了 policy-check；agent 没有伪造 gated、直接写 JSONL 或 unsafe emit。完整 backend stdout/stderr 仍缺失 | 80 |
| Q2 | 基座机制是否正常生效？ | ⚠️ 局部正常、局部暴露缺口 | policy 正确拒绝了被 deny 的 emit，empty-channel 检测和定向 `task.resume` 生效；原运行的 exhausted 分支只留下 repair 信号，未形成 red-team 可消费的失败终态 | 85 |
| Q3 | 编排是否合理、正常运行？ | ❌ 未闭合 | 前 4 个业务阶段完成；evidence-gate 5 次激活均无 terminal emit，`impact-boundary`、reviewer、reporter 从未触发 | 90 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **preset policy 主因；runtime recovery 为次因；不是 agent 忘记 emit** | evidence-gate 按要求尝试 emit，但 `topic_deny_rules` 自我拒绝；CLI 在写 channel 前退出，于是产生 0 字节 channel；后续 recovery 又没有让 red-team 工作流闭合 | 85 |

### 1.3 根因一句话

真正的因果链是：`evidence-gate` 评估出证据不足 → 按 preset 指令尝试发布 `redteam.retry.required` → `topic_deny_rules` 又把 `{hat_id: evidence-gate, topic: redteam.retry.required}` 拒绝 → CLI 在追加事件前返回 `recorded=false` → isolated channel 保持 0 字节 → runner 报 `hat_channel_empty_after_activation` 并删除空 channel → missing-terminal recovery 注入 `task.resume` → 同一 deny 重复 → 原运行最终没有 `redteam.evidence.gated`/`redteam.retry.required`，只能 `loop.cancel`。**主因置信度：85/100。**

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| 首轮终态 | 证据显示首轮失败/阻塞：session recovery 在 iteration 9 记录 `missing_terminal_emit_exhausted`；可信 events 没有 `redteam.complete` |
| 恢复状态 | 无成功恢复；iteration 5–8 的定向恢复均未产生 terminal event |
| 最终代码/产物状态 | `.ralph/red-team/07-retry-board.md` 与 `.ralph/agent/decisions.md` 存在并记录阻塞；`.ralph/red-team/REPORT.md`、`PLAN.md`、`QUESTIONS.md` 缺失；最终可信 events 末尾为 `loop.cancel` |
| 一致性告警 | `LOOP_COMPLETE` 不存在；`loop.cancel` 只证明人工终止，不证明 red-team 工作流成功 |

## 2. 执行链路对比图

### 2.1 拓扑激活表

| Hat | 预期 terminal event | 实际激活 | 结果 |
|---|---|---:|---|
| target-locker | `redteam.target.locked` | 1 | ✅ |
| plan-resolver | `redteam.plan.resolved` / `redteam.plan.unresolved` | 1 | ✅ |
| attack-surface-mapper | `redteam.attack.mapped` | 1 | ✅ |
| experiment-runner | `redteam.experiment.done` | 1 | ✅ |
| evidence-gate | `redteam.evidence.gated` / `redteam.retry.required` | 5（iteration 5–9） | ❌ 业务 events 均未接受；retry.required 的 emit 被 policy deny，channel 因此为空 |
| impact-boundary | `redteam.impact.rejected` / `redteam.plan.ready` | 0 | ⏸️ 上游 gate 缺失 |
| independent-reviewer | `redteam.reviewed` | 0 | ⏸️ 上游 gate 缺失 |
| reporter | `redteam.complete` | 0 | ⏸️ 上游 review 缺失 |

### 2.2 实际时间轴

| events 行 | 时间 | Hat | Topic | 状态 |
|---:|---|---|---|---|
| 1 | 21:22:49 | loop | `redteam.start` | ✅ 启动 |
| 2 | 21:23:53 | target-locker | `redteam.target.locked` | ✅ |
| 3 | 21:27:25 | plan-resolver | `redteam.plan.resolved` | ✅ |
| 4 | 21:29:55 | attack-surface-mapper | `redteam.attack.mapped` | ✅ |
| 5 | 21:31:47 | experiment-runner | `redteam.experiment.done` | ✅ control/attack 均通过 |
| — | 21:34:03–21:40:48 | evidence-gate | 无可信 events | ❌ 4 次 recovery + 1 次 exhausted |
| 6 | 21:43:55 | ralph | `loop.cancel` | ⚠️ 人工取消 |

### 2.3 预期 vs 实际

```text
redteam.start
  → target.locked ✅
  → plan.resolved ✅
  → attack.mapped ✅
  → experiment.done ✅
  → evidence.gated / retry.required ❌（5 次 activation；retry.required 被 owner self-deny）
  → impact-boundary ⏸ → independent-reviewer ⏸ → reporter ⏸ → redteam.complete ⏸
```

### 2.4 “agent 没有 emit”复核结果

这个判断经复核不成立，应该区分“agent 发起了 emit”与“事件成功进入 channel/events ledger”两件事：

1. `.ralph/red-team/07-retry-board.md:90-103` 记录了初次 precheck、正式 apply，以及 `task.resume` 后的重复尝试；结果反复是 `ok=false, recorded=false, errors=[event_policy:topic_denied]`。
2. `.ralph/agent/decisions.md:6-19,28-48,50-73` 记录了同一结论，并明确 agent 拒绝伪造 `redteam.evidence.gated`、拒绝直接编辑 JSONL、拒绝 unsafe emit。
3. 独立用当前 preset 重放同一个 `evidence-gate` emit，仍得到：`Hat 'evidence-gate' is denied from publishing topic 'redteam.retry.required'`；`--policy-check` 未创建 target file。
4. preset 本身同时声明 evidence-gate 发布并拥有该 topic：`presets/en/red-team-attack.yml:577-583`；但 `topic_deny_rules` 在 `:142-148` 又包含 owner 自己的 deny，具体是 `:145`。
5. runtime 的 `check_topic_deny_rules` 在 `crates/ralph-core/src/event_policy/validation.rs:325-375` 对该 `(hat, topic)` 做精确拒绝；CLI 在 `crates/ralph-cli/src/commands/emit/command_impl.rs:1407-1441` 明确在写 events 文件前 bail。因此空 channel 是 policy rejection 的结果，不是 agent 没调用 emit 的证据。

空 channel 的形成可以压缩为：

```text
channel 预创建（0 bytes）
  → agent 执行 ralph emit --policy-check
  → topic_deny_rules 命中 evidence-gate 自己的 deny
  → CLI 记录拒绝并在 append 前退出
  → channel 仍为 0 bytes
  → merge_hat_channel 报 empty_after_activation 并删除空文件
  → runtime 注入 task.resume
```

## 3. 历史问题上下文

`history_search=disabled`；本次不扫描主仓历史目录。所有历史关联字段均为 `N/A (history disabled)`。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | preset 把 evidence-gate 自己 deny 在其 declared terminal topic 上，导致合法 retry emit 永远不能写入 channel | `presets/en/red-team-attack.yml:142-148`（`:145`）；同文件 `:577-583`；`.ralph/red-team/07-retry-board.md:74-103`；`.ralph/agent/decisions.md:6-73`；独立 policy-check 重放 | P0 | 85 | preset owner/publish 矛盾(+25)、agent decision/retry-board(+20)、runtime policy source(+20)、独立重放(+15)、trusted events 未出现该 topic(+5) | MINIMAL 无原始 backend stdout，但不影响 policy 根因 |
| DEV-002 | isolated hat-channel 在 evidence-gate 激活后为空，runner 因此触发 fallback/recovery | `.ralph/diagnostics/logs/ralph-2026-08-14T05-22-49-418-89193.log:35-39,45-76`；`crates/ralph-cli/src/loop_runner/hat_channel.rs:76-98`；`.ralph/diagnostics/2026-08-14T05-22-49/recovery.jsonl:2-6` | P1 | 85 | 重复日志(+15)、channel merge 源码(+20)、recovery 双账本(+20)、与 policy rejection 的因果链(+20)、跨 activation 重现(+10) | 仍缺原始 channel 写入 syscall/Claude stdout，但结果链已闭合 |
| DEV-003 | missing-terminal exhaustion 在原运行中没有闭合 red-team 失败终态 | `.ralph/diagnostics/2026-08-14T05-22-49/recovery.jsonl:6`；`.ralph/events-20260813-212249.jsonl:1-6`；原运行 `event_processing.rs` exhausted 分支；现行修复路径 `crates/ralph-core/src/event_loop/event_processing.rs:666-712` | P1 | 80 | recovery exhausted(+20)、trusted events 无业务失败终态(+20)、runtime 分支(+20)、最终 loop.cancel(+15) | 这是次生问题；不能解释第一次空 channel 的产生 |

### 4.1 OPAC 逐 hat 审计表

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| target-locker | ✅ | ⚠️ | ⚠️ | ✅ | trusted event + artifact；无 agent-output | 45 |
| plan-resolver | ✅ | ⚠️ | ⚠️ | ✅ | trusted event + 解析产物；日志有 channel fallback | 50 |
| attack-surface-mapper | ✅ | ⚠️ | ⚠️ | ✅ | trusted event + attack-surface/experiment-plan | 45 |
| experiment-runner | ✅ | ⚠️ | ⚠️ | ✅ | `redteam.experiment.done` payload 声明 control/attack/evidence | 50 |
| evidence-gate | ✅ | ✅ | ⚠️ | ❌ | decisions/retry-board 证明尝试过 emit；policy 返回 `topic_denied`，因此没有 accepted terminal event | 85 |
| 后续 3 hats | N/A | N/A | N/A | N/A | 上游 `evidence-gate` 未闭合 | 90 |

### Prompt visibility 对账

`ralph -c ralph.red-team-attack.yml -H builtin:red-team-attack inspect prompt --hat evidence-gate --format json` 显示：

- `auto_inject`：`ralph-tools`、`ralph-tools-memories`、`ralph-tools-opac`；
- `on_demand`：`ralph-tools-cmdref`、`ralph-tools-emit`、`ralph-tools-precheck`、`ralph-tools-recovery-directives`、`ralph-tools-tasks`、`ralph-tools-wave`；
- `block_titles` 包含 `EVIDENCE GATE MODE` 与 `Precheck 阶段关键命令`。

因此不能以“emit skill 不可见”解释本次 failure；它是 on-demand，但 preset 指令已经明确要求 `ralph tools skill load ralph-tools-emit` 后进行 `--policy-check`。

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P0 | evidence-gate 无法发布自己声明拥有的 `redteam.retry.required` | preset policy（owner self-deny） | **85** | DEV-001 | preset owner/publish 矛盾(+25)；agent decision/retry-board(+20)；runtime policy source(+20)；独立重放(+15)；trusted events 未出现该 topic(+5) | N/A (history disabled) | 3→85 |
| P1 | `missing_terminal_emit_exhausted` 在原运行中没有闭合 red-team 失败终态 | mechanism / preset contract gap | **80** | DEV-003 | recovery exhausted(+20)；trusted events 对账(+20)；runtime 分支(+20)；最终 loop.cancel(+15) | N/A (history disabled) | 2→80 |

> 本轮重新分析后，DEV-002 不再是“agent 忘记 emit”的低置信度猜测：policy rejection、CLI 写入前 bail、空 channel 日志和 agent 记录已形成闭环。仍未获得的是 Claude 原始 stdout/stderr，而不是关键根因。

## 6. 修复建议（non-executing）

以下仅列人工可执行建议，不自动运行命令、不修改 preset、不执行清理。

### 6.1 短期（preset 修复，最小边界）

- **目标**：恢复 evidence-gate 发布自己已声明拥有的 retry terminal event。
- **改动**：只删除 `presets/en/red-team-attack.yml:145` 的 `{hat_id: evidence-gate, topic: redteam.retry.required}`；保留 `:142-144` 和 `:146-148` 对其他 hat 的 deny。`evidence-gate.exempt_topics` 与 `publishes` 已在 `:581-583` 声明 owner 关系。
- **预期效果**：证据不足时，agent 的合法 `redteam.retry.required` emit 可以通过 policy-check 并写入 isolated channel；证据达标时，`redteam.evidence.gated` 路径不受影响。
- **关联置信度**：85。

### 6.2 短期（operator workaround）

- **目标**：避免再次留下等待 `redteam.complete` 的长时间 loop。
- **改动**：人工运行时启用可观察日志并在出现 `missing_terminal_emit_exhausted` 后停止该 run，保留 session bundle 与可信 events；不要把 `loop.cancel` 当作 red-team 成功。
- **预期效果**：缩短无效等待，保留可归因证据。
- **关联置信度**：80。

### 6.3 中期（preset / schema / instructions）

- **目标**：为 red-team failure 提供明确、可消费的终态契约。
- **改动**：保留上面的 owner deny 修复，并补一条结构化 lint：任何 hat 的 `publishes`/`exempt_topics` 不得同时被同一 hat 的 `topic_deny_rules` deny；同步 `presets/schemas/red-team-attack.yml` 与 BDD real-runtime scenario，覆盖 evidence-gate 的 `retry.required` 成功落盘和 reporter 的 recovery failure 消费。
- **预期效果**：evidence-gate 无法恢复时仍能生成失败报告，而不是依赖用户取消。
- **关联置信度**：85。

### 6.4 长期（机制 / 底座）

- **目标**：让所有 preset 的 recovery exhaustion 遵守统一终止契约。
- **改动**：人工修复并验证 `inject_missing_terminal_emit_recovery` 的 exhausted 分支：必须让 loop runner 获得终止信号，或把失败事件通过当前 accepted/business terminal 路径送入 preset 可消费的失败链；同时增加一个真实 isolated runtime 回归场景，断言 `missing_terminal_emit_exhausted` 后不会继续激活 agent。
- **预期效果**：避免 repair-stream 仅记录诊断而不改变控制流，消除“诊断已失败、loop 仍等待”的悬挂状态。
- **关联置信度**：80；它不能替代 preset owner 修复。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| Claude backend 在执行已被 policy 拒绝的 CLI 后是否还有额外 stdout/stderr 或退出码异常 | 55 | bundle 未包含原始 agent-output.jsonl、orchestration.jsonl、errors.jsonl；但这不改变 emit 在 policy 层被拒绝、channel 为空的主因 | 第 1 轮：bundle/日志；第 2 轮：补读 agent decisions、retry-board、preset owner 声明、policy 源码；第 3 轮：独立 `--policy-check` 重放，确认 target file 不创建。原“agent 没有 emit”已被证伪，不再作为正式 finding |

## 8. 关键主仓代码引用清单

- `crates/ralph-core/src/event_loop/event_processing.rs:617-712`：missing-terminal recovery 入口与 exhaustion 分支。
- `crates/ralph-core/src/event_loop/event_processing.rs:666-712`：耗尽后构造 `plan.blocked`、记录 recovery envelope；当前修改在 reporter opt-in 时将其发布到 bus。
- `crates/ralph-core/src/event_loop/event_processing.rs:720-785`：未耗尽时构造并定向发布 `task.resume`，记录 pending recovery。
- `crates/ralph-core/src/event_loop/repair_stream_sink.rs:71-79`：repair event 仅 append 到 workspace `.ralph/recovery.jsonl`。
- `crates/ralph-cli/src/loop_runner/hat_channel.rs:79-98`：empty isolated channel 返回错误并写 fallback diagnostic。
- `crates/ralph-cli/src/loop_runner/inner.rs:3662-3701`：merge 失败后标记 `empty_terminal_channel`，让后续 recovery 分支接管。
- `crates/ralph-cli/src/loop_runner/inner.rs:4667-4689`：仅在成功 activation、空 channel、非 supervisor 且存在 terminal obligation 时触发 missing-terminal recovery。
- `crates/ralph-core/src/event_policy/validation.rs:325-375`：精确执行 `(hat_id, topic)` 的 `topic_deny_rules` 拒绝。
- `crates/ralph-cli/src/commands/emit/command_impl.rs:1407-1441`：policy deny 在写入 events 文件前直接返回。
- `crates/ralph-core/src/preset_lint/hat_scope_invariant.rs:154-200`：lint 以 `exempt_topics` 作为 owner 信号，但没有发现 owner self-deny 的矛盾，这是本次静态检查缺口。
- `presets/en/red-team-attack.yml:142-148`：`redteam.retry.required` 的 deny 列表，`:145` 错误包含 owner evidence-gate。
- `presets/en/red-team-attack.yml:577-583`：evidence-gate 声明 publishes/exempt/terminal。
- `presets/schemas/red-team-attack.yml`：evidence-gate 的 payload contract；本次未达到该 hat 的任何 declared terminal event。

## 9. 诊断盲区与提交前检查

- [x] Phase 0 产物盘点已写入。
- [x] 仅读取 `.ralph/current-events` 指向的一个 events 文件作为可信拓扑。
- [x] MINIMAL 模式缺 orchestration/agent-output 已记录为盲区；未把该盲区误判成 agent 根因。
- [x] 已重新盘点并纳入 `07-retry-board.md`、`decisions.md`、`memories.md` 等中间产物。
- [x] 已独立重放 `evidence-gate → redteam.retry.required --policy-check`，确认 `topic_denied` 且未创建 target file。
- [x] 已把“agent 没有 emit”改判为不成立，并区分 emit 尝试与 accepted event 落账。
- [x] P0 置信度为 85，满足 P0≥70；confidence<60 的候选仅放 §7。
- [x] 未使用 `hat_handoff`、`loop_state_snapshot.json` 或 `human.guidance` 作为当前机制。
- [x] `task.resume` 被描述为当前 recovery transport；没有将其描述为已删除。
- [x] `docs/report/` 仅写入本最终 Markdown 报告；过程 JSON 与 stderr 在临时目录，已在落盘后清理。
- [x] frontmatter 已记录 `history_search: disabled`、bundle、trace、feedback 状态。
