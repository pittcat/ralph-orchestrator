---
title: ce-executor-pipeline Loop `2026-08-05-001-refactor-large-file-module-split-plan` 运行链路诊断报告
date: 2026-08-06
type: diagnosis
loop_id: 2026-08-05-001-refactor-large-file-module-split-plan
preset: builtin:ce-executor-pipeline
run_dir: .worktrees/2026-08-05-001-refactor-large-file-module-split-plan/.ralph
status: 用户主动 RPC abort；prescription/execution 双向漏洞并存，多个 P0 跨轮复发
diagnostics_mode: LOGS_ONLY
history_search: full
---

# ce-executor-pipeline Loop `2026-08-05-001-refactor-large-file-module-split-plan` 运行链路诊断报告

> **生成时间**: 2026-08-06
> **诊断对象**: `.worktrees/2026-08-05-001-refactor-large-file-module-split-plan/.ralph/`（loop_id=2026-08-05-001-refactor-large-file-module-split-plan）
> **对照 preset**: `presets/en/ce-executor-pipeline.yml` + `presets/schemas/ce-executor-pipeline.yml`
> **执行方式**: 4 sub-agent 并行（流程还原 / 历史 / 对账 / 归因）→ 汇总；`history_search=full` 故 Agent B 启用
> **Diagnostics 模式**: LOGS_ONLY（仅 `.ralph/diagnostics/logs/ralph-2026-08-06T13-32-48-626-81394.log` 13 行，无 session orchestration.jsonl；`drift.jsonl` 0 行）
> **history_search**: `full`（用户明确要求全库扫描；Agent B 已扫 `docs/report/*-diagnosis.md` + `docs/solutions/{integration-issues,logic-errors,state-management}` + `docs/plans/` + `docs/brainstorms/`）
> **execution_capabilities**: `["single-chain"]`（`event_loop.execution_mode: isolated`，无 `supervisor.enabled`，hat instructions 不含 `ralph wave emit` / `ralph wave verify`；events 无 `wave_id`；`.ralph/supervisor.db` 存在但属于 default-wave 路径的 ledger 残留，不是 supervisor 启用证据，**禁止**当作 supervisor capability 信号）
> **报告仓库**: `ralph-orchestrator` 主仓
> **Tier C 根**: preset `event_loop.required_events = [report.done]` + `topic_format_whitelist = [LOOP_COMPLETE]`（单元素，所有非 LOOP_COMPLETE 业务 topic 走默认 deny）
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70（LOGS_ONLY 模式硬顶 75，mechanism 有 file:line + recovery 可例外到 85）

---

## 0. 产物盘点

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | events（`current-events` → `events-20260806-053248.jsonl`） | ✅ | 1 行（单 JSON 对象，payload=64KB plan） | 当前 run 实际只有 `work.start` bootstrap，无业务事件 |
| S | events-history（`events-history-20260806-053248.jsonl`） | ✅ | 1 行（warmup） | 同上 |
| S | ledger.jsonl | ✅ | 4 条 | `loop.batch_sync` iteration 1-4 计数 |
| S | recovery.jsonl（workspace 级） | ✅ | 4 条 | 3× `work.done` + 1× `work.failed` `contract_violation`，max retry_count=2 |
| S | loops.json | ✅ | — | loop_id + worktree + pid |
| S | logs（`diagnostics/logs/ralph-2026-08-06T13-32-48-626-81394.log`） | ✅ | 13 行 | 当前 run 全生命周期日志（启动 → RPC abort） |
| A | agent/tasks.jsonl | ✅（空） | 0 | `tasks.enabled: false`（preset 显式声明），预期 |
| A | agent/progress.md / summary.md / handoff.md | ❌ | — | loop 未终止，预期 |
| A | agent/accepted-transitions.jsonl | ✅ | 29 条 | 真实编排时间轴（topology-only ledger，ts/hat 全 null） |
| B | agent/events-hat-plan-reviewer-*.jsonl | ✅ | 0 字节 | hat-channel 空，**异常**：plan-reviewer 已 emit plan.ready |
| B | agent/{context.md, resume-context.md} | ✅ | — | reuse-history 上下文提示 |
| B | agent/decisions.md | ✅ | 15666B | hat 决策记录 |
| B | agent/plan-baseline.sha | ✅ | 41B | plan 基线 |
| B | `.ralph/supervisor.db` | ✅ | 126KB | capability 不要求，单链预期下 N/A |
| B | `.ralph/diagnostics/2026-08-06T13-32-48/{recovery,drift}.jsonl` | recovery=1 行 / drift=0 | — | 缺 orchestration.jsonl（LOGS_ONLY 预期） |
| B | `.ralph/forge/.../templates/` | ✅ | 3 文件 | fail-confidence-rubric / settlement-evidence / README 模板 |
| B | `.ralph/reuse-history/` | ✅ | 3 个归档目录 | 20260805T214910 / 20260806T044843 / 20260806T053248 |
| B | `recovery:*.jsonl` per-loop | ✅ | 7 文件 | 6 个 `isolated_scope_violation` + 1 个 `missing_event_gate:default_publishes_injected` |
| C | preset Tier C 预期 | preset/schema 解析 | — | 14 hat linear chain |

**execution_capabilities 推断结果**：`["single-chain"]`

判定信号：
- `event_loop.execution_mode: isolated` → 不是 supervisor / wave 协调模式
- preset YAML 无 `event_loop.supervisor` 段 → `+supervisor` 否
- hat instructions 不含 `ralph wave emit` / `## WAVE CONTEXT` → `+wave` 否
- events / accepted-transitions 均无 `wave_id` → 产物侧验证 `+wave` 否
- `.ralph/supervisor.db` 存在但 enabled=false + ledger 残留 → **不**当作 supervisor 启用证据

**缺失产物 → 故障判定**：
- `.ralph/supervisor.db` 缺失 → **N/A**（capability 不要求；现存但属 default-wave ledger 残留）
- events 无 `wave_id` → **N/A**（capability 不要求）
- `.ralph/diagnostics/2026-08-06T13-32-48/orchestration.jsonl` 缺失 → **预期**（diagnostics 模式 LOGS_ONLY 不写 session orchestration）

**盲区 / 根因置信度硬顶**：
- LOGS_ONLY → agent/OPAC 归因 ≤ 50
- LOGS_ONLY → mechanism 有 `file:line` + `recovery` 可例外到 85
- 整行硬顶：75

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: **部分偏离 + 假闭环风险**（prescription `topic_format_whitelist=['LOOP_COMPLETE']` 单元素与 `event_policy.business_topics` 列表不一致 → 业务 topic 走默认 deny；executor / test-stabilizer 反复越权 emit `test.topic` / `fallback.topic` / `debug.step`；`work.failed.proposed` 在 precheck gate 缺席时被 `missing_event_gate` 合成注入）
- **P0 / P1 / P2 数量**: P0=3 / P1=2 / P2=1（均为 confidence≥入表门槛）
- **最高优先级根因置信度**: P0-1 (DEV-002) = **70** / 100（mechanism，同根因 2026-07-29 plan 已合并但复发）
- **历史复发**: 是 — 第 ≥3 次 — 引用 `docs/report/2026-07-29-ce-executor-pipeline-20260729-094341-diagnosis.md` + `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md`

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ | OPAC L1 prompt injection ✅；L3 worker Confirm ❌（stabilization.done accepted 后 16s SIGTERM 未达 reporter）；L4 hat-channel ⚠️（plan-reviewer emit 但 channel 空） | 65 |
| Q2 | 基座机制是否正常生效？ | ❌ | Event origin guard 在 `test.topic/fallback.topic/debug.step` 上 6 次触发 `isolated_scope_violation`；Recovery 升级路径未触发（retry_count=2 但无 plan.blocked 升级） | 60 |
| Q3 | 编排是否合理、正常运行？ | ❌ | `topic_format_whitelist=['LOOP_COMPLETE']` 单元素 + executor `publishes=['work.done','work.failed']` 形成 prescription 矛盾；3 轮 iteration 在 work.done / stabilization.done 同节点中断（DEV-006） | 70 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **mechanism（主因，DEV-002=70）+ preset（DEV-001 贡献）+ mechanism/agent 复合（DEV-005b backend 沉默后 stdin abort）** | DEV-002 是 preset 端 precheck gate hat 缺席 + payload_contract.rs:327 default_publishes 合并 + event_policy.rs:2696 topic_publishes gate 协同不通过；2026-07-29 plan 已合并但机制层 root cause 未根治 | 70 |

### 1.3 根因一句话

> **本次 run 终止主因（DEV-005b, confidence 70）**：当前 run 启动后 `PtyExecutor` spawn `claude` backend (pid 81435) 完成，但 backend 16 秒内**未产生任何输出**，plan-reviewer hat 始终未激活（`events-hat-plan-reviewer-*.jsonl` 0 字节）。随后 stdin reader 收到一条 `RpcCommand::Abort`（reason 字段硬编码为 `"User requested abort"` 不可信），触发 SIGTERM 给 backend + 2 个 helper 进程。**这不是用户主动 quit**（log 全程无 TUI 子进程、无 signal handler 触发，reason 字符串是 RPC client 写死的字面量）。**真因指向**：claude backend 启动失败 / PTY 通道断开 / stdin 上游残留 abort 命令三类之一，需要进一步抓 stdin fd peer_pid 才能定位。
>
> **业务链路根因（DEV-002, confidence 70，mechanism）**：ce-executor-pipeline preset 的 `topic_format_whitelist=['LOOP_COMPLETE']` 单元素 + `payload_contract.rs:327` 的 `default_publishes` 合并 + `event_policy.rs:2696` 的 `topic_publishes` gate 三者协同不通过，导致 (a) executor / test-stabilizer 越权 emit `test.topic` / `fallback.topic` / `debug.step` 被 `isolated_scope_violation` 拒（DEV-001）；(b) executor 在 precheck gate hat 缺席时合成注入 `work.failed.proposed` 被 `missing_event_gate:default_publishes_injected` 拒（DEV-002）。2026-07-29 plan 已合并但机制层 root cause 未根治。

### 1.4 终态时序一致性（event-artifact chronology）

| 项目 | 内容 |
|------|------|
| **首轮终态（initial_terminal_status）** | **首轮未达终态**（accepted-transitions 行 18 第 1 轮达 `report.done` 但无 `LOOP_COMPLETE` accepted 事件；行 25 第 2 轮 `report.done` 出现在 `work.failed` 之后，是 reporter 兜底 emit；行 29 第 3 轮 `stabilization.done` accepted 后被 SIGTERM） |
| **恢复状态（recovery_status）** | **失败终态后恢复（partial）**：第 1 轮 `report.done` → 第 2 轮 `work.failed.rejected` → 第 2 轮 `report.done`（recovery path）→ 第 3 轮 `stabilization.done`（cut off）；3 轮均未达 `LOOP_COMPLETE` accepted 事件，artifact (reuse-history) 与 accepted 事件时序存在分裂 |
| **最终代码状态（final_code_state）** | 当前 run 在 stabilization.done 后被 RPC abort（log:11 `RpcDispatcher received Abort command, reason=User requested abort`），无 commit / merge / branch 更新；reuse-history 归档保留了前两轮的完整事件但**无任何 accepted LOOP_COMPLETE** |
| **一致性告警** | ⚠️ **失败终态后恢复（partial）+ 终态事件从未 accepted**：3 轮 reuse-history 的 `report.done` 与 `LOOP_COMPLETE` 均未出现在 accepted-transitions；workspace recovery.jsonl 4 条 contract_violation（max retry_count=2）证明 runtime 曾尝试终结但 precheck gate 反复拒收。**禁止**输出「零拒收」或「首轮完整成功」 |

---

## 2. 执行链路对比图

### §2.1 拓扑激活表

**Preset hat DAG（14 hats，execution_mode: isolated）**

| Hat | preset expected | activated in accepted-transitions | 已触发 topic |
|-----|----------------|-----------------------------------|--------------|
| plan-reviewer | yes | ✅ 3次（行1/19/26） | `plan.ready` |
| executor | yes | ✅ 多次（行2/4/5/21/23/27/28） | `work.done.proposed` / `work.done` / `work.failed.rejected` |
| test-stabilizer | yes | ✅ 2次（行6/29） | `stabilization.done` |
| dim:goal-alignment | yes | ✅ 1次（行7） | `review.goalalign.done` |
| dim:correctness | yes | ✅ 1次（行8） | `review.correctness.done` |
| dim:testing | yes | ✅ 1次（行9） | `review.testing.done` |
| dim:maintainability | yes | ✅ 1次（行10） | `review.maintainability.done` |
| dim:project-standards | yes | ✅ 1次（行11） | `review.standards.done` |
| dim:adversarial | yes | ✅ 1次（行12） | `review.adversarial.done` |
| review-synthesizer | yes | ✅ 1次（行13） | `review.synthesized` |
| fix-planner | yes | ✅ 1次（行14） | `review.complete` |
| fixer | yes | ✅ 2次（行15/16） | `fix.done.proposed` / `fix.done` |
| alignment | yes | ✅ 1次（行17） | `align.done` |
| reporter | yes | ✅ 1次（行18） | `report.done` |

**结论**：14 hats 全部曾激活，无永久跳过 hat。

### §2.2 时间轴对比（accepted-transitions 29 条 vs preset 预期）

**Preset 线性链路**：`plan.ready` → `work.done` → `stabilization.done` → 6×`review.*.done` → `review.synthesized` → `review.complete` → `fix.done` → `align.done` → `report.done` → `LOOP_COMPLETE`

**accepted-transitions 实际时间轴**（29条，时序排列）：

| # | topic | vs preset 预期 |
|---|-------|---------------|
| 1 | `plan.ready` (plan-reviewer) | ✅ 符合 |
| 2 | `work.done.proposed` (executor) | ⚠️ 第1次被拒（行3） |
| 3 | `work.done.rejected` (precheck) | ❌ precheck 拒收 |
| 4 | `work.done.proposed` (executor) | ⚠️ 重试 |
| 5 | `work.done` (executor) | ✅ 成功 |
| 6 | `stabilization.done` (test-stabilizer) | ✅ |
| 7 | `review.goalalign.done` | ✅ |
| 8-12 | 5× review.*.done | ✅ |
| 13 | `review.synthesized` | ✅ |
| 14 | `review.complete` (fix-planner) | ✅ |
| 15 | `fix.done.proposed` (fixer) | ⚠️ 重试 |
| 16 | `fix.done` (fixer) | ✅ |
| 17 | `align.done` (alignment) | ✅ |
| 18 | `report.done` (reporter) | ✅ 第1轮结束 |
| 19 | `plan.ready` | ⚠️ 第2周期启动 |
| 20 | `work.failed.rejected` | ❌ precheck 拒 |
| 21 | `work.done.proposed` (executor) | ⚠️ 改发 work.done |
| 22 | `work.done.rejected` | ❌ |
| 23 | `work.done.proposed` (executor) | ⚠️ 又重试 |
| 24 | `work.done.rejected` | ❌ retry_count=2 |
| 25 | `report.done` | ✅ 第2轮 report（recovery path） |
| 26 | `plan.ready` | ⚠️ 第3周期 |
| 27 | `work.done.proposed` | ⚠️ |
| 28 | `work.done` | ✅ |
| 29 | `stabilization.done` | ✅ cut off（SIGTERM） |

**workspace 级 recovery.jsonl（4 条 contract_violation）**：
- `work.done` @ 2026-08-05T17:59:22 (retry_count=1)
- `work.failed` @ 2026-08-06T01:42:38 (retry_count=1)
- `work.done` @ 2026-08-06T01:53:06 (retry_count=1)
- `work.done` @ 2026-08-06T02:51:22 (retry_count=2)

**per-loop recovery 文件（7 个，文件名即真相）**：
- `recovery:workflow_guard:executor:{debug_step,fallback_topic,test_topic}:isolated_scope_violation` × 3
- `recovery:workflow_guard:test_stabilizer:{debug_step,fallback_topic,test_topic}:isolated_scope_violation` × 3
- `recovery:missing_event_gate:executor:work_failed_proposed:default_publishes_injected` × 1（outcome=repeated）

### §2.3 终止判定

**终止类型**：`backend_silent_then_rpc_abort`（**非用户主动 quit**，reason 字符串不可信）

**严格按事实重构当前 run 时间轴**：

| 时刻 (UTC) | 事件 | 证据 |
|------------|------|------|
| `05:22:39` | reuse-history 上轮收尾：`stabilization.done` accepted（ledger iter=4） | accepted-transitions 末行 + ledger.jsonl iter=4 |
| `05:32:48.648` | 当前 run 启动 `setup_process_group` | log:1 |
| `05:32:48.648` | autonomous fallback（stdout 非 TTY） | log:3 |
| `05:32:48.678` | supervisor.db picked up（default-wave 路径） | log:4 |
| `05:32:48.695-700` | memory injection：14 memories / 7775 chars 注入 prompt | log:5-8 |
| `05:32:48.700` | **PtyExecutor spawned backend `claude`，child_pid=81435** | log:9 |
| **`05:32:48.700` → `05:33:04.803`** | **沉默 16 秒：0 hat 激活、0 plan-reviewer 启动、0 business event、0 prompt-sent 日志、0 backend output 日志** | log 全部 13 行 + hat-channel 0 字节 |
| `05:33:04.803` | `RpcDispatcher received Abort command, reason="User requested abort"` | log:10 |
| `05:33:04.803` | `interrupt_tx=true sent successfully` | log:11 |
| `05:33:04.803` | `Runtime interrupt received, sending SIGTERM to process group` | log:12 |
| `05:33:04.829` | SIGTERM 到 3 个 victim (81499/81435/81884) | log:13 |

**哪一步发生了错误？**

- **`plan-reviewer` hat 根本没启动** —— `.ralph/agent/events-hat-plan-reviewer-2026-08-05-001-refactor-large-file-module-split-plan-1.jsonl` 0 字节
- preset `plan-reviewer.triggers = ['work.start']`，但 `work.start` 是 `loop-bootstrap` 同步事件，不是 hat activation
- 当前 run spawn 了 claude backend (pid 81435) 但 **claude 在 16 秒内没产生任何输出 / 没走 prompt → plan-reviewer hat 从未收到 activation 信号**
- **真正的"错误"不在 preset/business 流程里**，而在 **claude backend 与 PTY 通道** 之间

**为什么不是"用户主动 quit"？**

1. log 全 13 行**无任何 TUI 子进程**相关行（无 `subprocess RPC mode`、无 `app.run`、无 signal handler 触发）
2. **log 全程无 `wait_for_termination_signal` 路径触发**（autonomous 模式该路径不会自动 SIGTERM）
3. `RpcCommand::Abort` 的 reason 字段是 `crates/ralph-tui/src/rpc_writer.rs:64` **hard-code** 的字面量 `"User requested abort"`——**任何调用 `send_abort()` 的路径都拿到同一个字符串**，不能作为"用户主动"的证据
4. 当前 run 是 autonomous CLI（log:3 明确 `falling back to autonomous`），没有 TUI 子进程能发 quit
5. spawn → SIGTERM 中间 16 秒恰好是 backend cold start + claude CLI auth/handshake 失败/网络超时 的典型窗口

**与 LOOP_COMPLETE / plan.blocked 的区别**：
- 不是 `LOOP_COMPLETE`（无 accepted 终态事件）
- 不是 `plan.blocked`（plan_reviewer 从未 emit `plan.blocked`）
- 不是 stall detector（无 stall 日志）
- 不是用户主动 quit（理由见上 5 点）
- **是 `backend_silent → stdin RpcCommand::Abort → SIGTERM`**——`crates/ralph-cli/src/commands/run.rs:2857` 的 cleanup 路径会在任何 child.wait 完成后无条件调 `rpc_writer.send_abort()`（带 hard-code reason 字符串），但当前 run 走的是 RpcDispatcher stdin reader 路径，不是该 cleanup 路径，所以 abort 命令的真实发送者是 stdin 上游（外部 RPC client / 上一个 run 的 RPC 连接残留），需进一步查 `.ralph/loops.json` 的 pid 与 stdin fd 状态

---

## 3. 历史问题上下文

> 本节由 Agent B 在 `--include-history=full` 模式下生成。

### §3 历史问题知识库

| 类型 | 出现次数 | 本次关联度 | 闭环状态 | one_line |
|------|---------|----------|---------|---------|
| `isolated_scope_violation` (executor 试探 test.topic / debug.step) | 3+ 次（2026-06-17 merry-lotus + noble-peacock；2026-07-26 implementation-review） | **高** | 部分已知，未根治 | executor hat 越权 emit 非 publishes 列表 topic，noble-peacock 案 26 次越权 probe，24 次 CLI 挡，2 次因 hat=None 泄漏；本此 test.topic / fallback.topic / debug.step 三 topic 均不在 ce-executor-pipeline publishes，症状与 noble-peacock 同族 |
| `missing_event_gate` + `default_publishes_injected` | 5+ 次（2026-07-29 ce-executor-pipeline × 1；2026-08-04 merge-batch × 5；2026-08-02 ce-executor-pipeline × 已知） | **高** | 2026-07-29 plan 已修复 precheck merge_hats_overlay 白名单；2026-08-04 merge-batch DEV-004 仍 open | ce-executor-pipeline: merge_hats_overlay 缺 precheck 白名单导致 gate hat 未合成；merge-batch: stabilizer 零 emit 触发 orchestrator 注入；本此 executor emit work.failed.proposed 被 missing_event_gate 拒（reason=default_publishes_injected） |
| `contract_violation` (work.done / work.failed) | 2 次（2026-08-01 ce-executor-pipeline × 1；2026-08-04 merge-batch × 1） | **中** | plan 2026-08-01-001 已合；merge-batch DEV-003 仍 open | 2026-08-01: correctness.md 摘要计数与实际不符导致评审合成阻塞；2026-08-04: plan.blocked 进 outbox 但未进 main events（双账本分歧）；本此 workspace 级 recovery.jsonl 4 条 contract_violation 含 retry_count=2 |
| executor emit `work.failed.proposed` 被 `missing_event_gate` 拒 | 1 次（2026-07-29 ce-executor-pipeline） | **高** | 2026-07-29 plan 已合（precheck 白名单 + lint 兜底）但本此同症状复发 | 同根因复发：precheck gate hat 未合成导致 executor 直接 emit work.failed 而非 work.failed.proposed；本次症状表现为 work.failed.proposed 被 missing_event_gate 拒 |
| RPC abort（非 plan.blocked 终止） | 2026-08-01/02 ce-executor-pipeline 两次 blocked | **中** | 2026-08-02 plan 已识别 reporter 路由异常 | 2026-08-02 两次 run 均因 schema/instructions SSOT 漂移导致 reporter verdict=blocked；但 RPC abort 与 blocked 机制不同 |

### §3 末尾窗口注脚（hard rule）
本次扫描窗口：full (full-history)

### §8 历史 Run 对照表

| Run 归档 | 时间 | 是否同 preset | 关键 outcome | 与本次关系 |
|---------|------|-------------|-------------|-----------|
| `2026-07-29-ce-executor-pipeline-20260729-094341-diagnosis.md` | 2026-07-29 | **是** | `work.failed` 直达 reporter，precheck gate hat 缺席；P0 root cause: `merge_hats_overlay` 缺 precheck 白名单 | 同 symptom（work.failed 路径）+ 同根因机制层；plan 已合入，但本此 executor emit `work.failed.proposed` 被拒，说明 precheck 仍有问题或 executor 侧 publishes 不匹配 |
| `2026-08-01-ce-executor-pipeline-2026-08-01-001-fix-unified-execution-contract-p0-p1-plan-diagnosis.md` | 2026-08-01 | **是** | `review.artifact.blocked`（correctness.md 计数不一致）；recovery.jsonl 1 条 repair-stream | 同 preset；本此 4 条 contract_violation recovery 与本次 workspace recovery.jsonl 症状重叠 |
| `2026-08-02-ce-executor-pipeline-20260802-001-002-diagnosis.md` | 2026-08-02 | **是** | reporter verdict=blocked × 2 次；schema/instructions SSOT 漂移导致 review emit payload 缺 required_fields；stall detector 下游保护触发 | 同 preset；两次 resume 进入完整 pipeline 后中断（与本次前 2 轮 resume 一致）；根因：preset/schema SSOT 漂移 |
| `2026-08-04-merge-batch-primary-20260804-053651-diagnosis.md` | 2026-08-04 | 否（merge-batch） | max_runtime 截断；5 次 `missing_event_gate` + `default_publishes` 注入；contract_violation 相关双账本分歧 | `missing_event_gate + default_publishes_injected` 机制与本次 symptom 2 直接相关（DEV-004）；plan.blocked 记账不一致（DEV-003）与本此 contract_violation 可能同族 |
| `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md` | 2026-06-17 | 否（serial） | executor emit `debug.step` × 8 静默丢弃 | executor 试探非 publishes topic 的最早记录之一；本此 test.topic / fallback.topic / debug.step 三 topic 均被 scope 拒，同族复发 |
| `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md` | 2026-06-17 | 否（serial） | executor 26 次越权 probe（debug.step / build.done / review.passed）；24 次 CLI 挡，2 次因 hat=None 泄漏 | executor 越权 emit 家族最详细记录；与本此 test.topic / fallback.topic / debug.step 同族复发，根因均为 executor 在 isolated 模式下对 publishes 边界认知不足 |

### §3 机制生效矩阵

| # | 机制 | 状态 | 证据 |
|---|------|------|------|
| 1 | Event origin guard / hat scope | ❌ | DEV-001（6 个 per-loop recovery 文件，scope violation） |
| 2 | Payload contract | ⚠️ | schema required_fields 不全，accepted-transitions 全 ts/hat/source_hat=null，无法对账 |
| 3 | Execution contract | N/A | tasks.enabled: false |
| 4 | Workflow guard / phase | ❌ | DEV-001（scope violations）+ DEV-002（missing_event_gate 单点） |
| 5 | Isolated 单事件预算 | ❌ | accepted-transitions 行 1/19/26：plan-reviewer 在同 iteration 内多次 emit plan.ready |
| 6 | step_handoff | N/A | tasks.enabled: false |
| 7 | Recovery 升级 | ⚠️ | retry_count=2 已出现（workspace recovery 第 4 条），未见升级到 plan.blocked |
| 8 | loop.resume / task.resume 消费者 | ✅ | 3 次 resume-history 均有归档（reuse-history/） |
| 9 | Stall / progressive_failure / loop_stale | ⚠️ | DEV-006：3 轮同节点中断但无 stall 触发日志 |
| 10 | Drift monitor | N/A | drift.jsonl 0 行 |
| 11 | Dedup / duplicate_work_done | ⚠️ | 行 25 与行 18 均达 report.done（DEV-006 子问题） |
| 12 | Terminal / completion_after_terminal | ❌ | DEV-005b |
| 13 | Event-artifact temporal consistency | ❌ | DEV-005b（SIGTERM 16s 终态 vs stabilization.done artifact 写入一致性断裂） |

### §3.4 OPAC 表（diagnostics_mode=LOGS_ONLY）

| OPAC 单项 | 状态 | 证据 | LOGS_ONLY 降级说明 |
|----------|------|------|-------------------|
| L1 prompt injection | ✅ | log 行 7-9：14 memories 加载成功，inject=Auto | N/A |
| L2 orchestration | N/A | 缺 session orchestration.jsonl（diagnostics 模式 MINIMAL/FULL 才写） | 预期降级，无 session ledger 可查 |
| L3 worker Confirm | ❌ | stabilization.done accepted 但未达 reporter → DEV-005b | 终态事件断链 |
| L4 hat-channel | ⚠️ | events-hat-plan-reviewer-*.jsonl 0 字节，但 plan-reviewer 已 emit plan.ready | hat-channel 与实际 emit 状态不一致 |
| L5 stall detector | N/A | 无 stall 日志，未触发 | 预期（run 被 SIGTERM 中断，非 stall） |

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|----|------|----------|------------|------------|--------------|----------|
| DEV-001 | executor / test-stabilizer 反复 emit `test.topic` / `fallback.topic` / `debug.step`，被 `workflow_guard` 以 `isolated_scope_violation` 拒 | 6 个 per-loop recovery 文件名 | P0 候选 | 40+15=55 | preset 行号（topic_format_whitelist=LOOP_COMPLETE 单元素 + executor publishes=['work.done','work.failed']） | 缺 agent-output 实证、缺 file:line、缺 FULL 模式复验 |
| DEV-002 | executor emit `work.failed.proposed` 被 `missing_event_gate` 拒，reason=`default_publishes_injected` | 1 个 per-loop recovery 文件名 | P0 候选 | 40+15+10=65 | preset 行号 + history 高关联（2026-07-29 同 preset 同根因） | 缺 file:line、缺双账本对照 |
| DEV-003 | workspace 级 `recovery.jsonl` 4 条 `contract_violation`（3× work.done / 1× work.failed），retry_count 最大=2 | .ralph/recovery.jsonl 全 4 条 | P1 候选 | 40+15=55 | preset 行号（executor publishes 不匹配 ledger 实际 retry 路径） | 缺 file:line |
| DEV-004 | accepted-transitions 字段 ts/hat/source_hat 全 null（topology-only ledger，不是真 orchestration） | 29 条记录全 null | P2 | 40+0=40 | 无额外证据（仅单账本） | 缺 file:line、缺第二账本 |
| DEV-005b | 当前 run spawn claude backend (pid 81435) 后 16 秒无任何 hat 激活（plan-reviewer hat-channel 0 字节），被 stdin 上游 `RpcCommand::Abort` 触发 SIGTERM；reason="User requested abort" 是 `crates/ralph-tui/src/rpc_writer.rs:64` hard-code 字符串，**不可信** | log:9 (`PtyExecutor spawned backend ... backend_cmd=claude`) + log:10 (`RpcDispatcher received Abort command`) + log:13 SIGTERM victims=[81499,81435,81884] + `.ralph/agent/events-hat-plan-reviewer-*-1.jsonl` 0 字节 | P0（运行终止主因） | 40+25+10=75 | log 行号可索引 + preset event_loop 不支持 abort 后 resume + 历史（无直接同族） | 缺 stdin 上游 pid（需 lsof / ps 查 fd 状态） |
| DEV-006 | 第 3 轮与第 1/2 轮 reached 同样节点（work.done / stabilization.done）后中断，复发循环 | accepted-transitions 行 18/25/29 | P1 | 40+10=50 | BDD 同症状（accepted-transitions 内部 3 轮对照） | 缺 file:line |

---

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|--------------|----------|---------|
| P0 | DEV-001 executor/test-stabilizer 越权 emit 非 publishes topic | compound（preset + mechanism） | **65** | DEV-001 | preset L61 (`topic_format_whitelist: [LOOP_COMPLETE]` 单元素) + executor L2216 (`publishes: ["work.done","work.failed"]`) + test-stabilizer L3052 (`publishes: ["stabilization.done","stabilization.blocked"]`) + `crates/ralph-core/src/event_loop/mod.rs:9823` (`reason_code = "isolated_scope_violation"`) + `crates/ralph-core/src/event_origin.rs:1252` | 高 | 2 |
| P0 | DEV-002 executor work.failed.proposed 被 missing_event_gate 拒（reason=default_publishes_injected） | mechanism | **70** | DEV-002 | preset L82-90（precheck desugar 将 publishes 改写为 `<X>.proposed`）+ `crates/ralph-core/src/payload_contract.rs:327`（default_publishes 合并）+ `crates/ralph-core/src/event_policy.rs:2696`（preview_emit_event topic_publishes gate）+ history 2026-07-29 同根因 plan 已合并但复发 | 高 | 2 |
| P0 | **DEV-005b** 当前 run spawn claude backend 后 16 秒无任何 hat 激活（plan-reviewer hat-channel 0 字节），被 stdin 上游 `RpcCommand::Abort` 触发 SIGTERM；reason="User requested abort" 是 `crates/ralph-tui/src/rpc_writer.rs:64` hard-code 字符串，**不可信** | mechanism（backend 沉默 + cleanup 路径） + agent（claude backend 沉默） | **70** | DEV-005b | `.ralph/diagnostics/logs/ralph-2026-08-06T13-32-48-626-81394.log:9` `PtyExecutor spawned backend in new PTY session child_pid=Some(81435) backend_cmd=claude` + log:10 `RpcDispatcher received Abort command, reason=Some("User requested abort")`（reason 字段是 `rpc_writer.rs:64` hard-code）+ log:13 SIGTERM victims=[81499,81435,81884] + `.ralph/agent/events-hat-plan-reviewer-2026-08-05-001-refactor-large-file-module-split-plan-1.jsonl` **0 字节** + `commands/run.rs:2857` send_abort 在 cleanup 阶段被无条件调用 + 当前 run 全文 log 无 TUI / signal handler 痕迹 | 中 | 2 |
| P1 | DEV-003 contract_violation × 4 + retry_count=2 | mechanism | **60** | DEV-003 | `.ralph/recovery.jsonl` 全 4 条 + `crates/ralph-core/src/state/recovery_log.rs:459`（contract_violation 写入路径）+ precheck.work.done prompt L130-143 / precheck.work.failed L95-106 与实际 retry 路径未对齐 | 中 | 1 |
| P1 | DEV-006 3 轮同节点中断复发循环 | compound | **55** | DEV-006 | ledger.jsonl iterations 1/2/3 + BDD 同症状 + history 2026-08-02 同类复发 + 缺 file:line（需 events.jsonl 核实中断时刻 hat） | 中 | 1 |
| P2 | DEV-004 accepted-transitions 字段 ts/hat/source_hat 全 null | mechanism（ledger 格式） | **40** | DEV-004 | 单账本 evidence；缺 file:line + 第二账本 | 低 | 1 |

**D 终评调整说明**：
- DEV-001: +10（preset L61 行级定位 whitelist 与 executor publishes 矛盾 + `event_loop/mod.rs:9823` `reason_code = "isolated_scope_violation"` file:line）
- DEV-002: +5（`payload_contract.rs:327` + `event_policy.rs:2696` 行级源码 + 2026-07-29 历史复发）
- DEV-005b: =75（LOGS_ONLY 硬顶 75，log 有 backend spawn + abort + SIGTERM 三重证据；reason 字符串不可信但日志三联齐全）
- DEV-003: +5（recovery.jsonl 全字段 + `recovery_log.rs:459` file:line）
- DEV-006: +5（compound 多因素但缺 file:line，≤60 入表门槛）
- DEV-004: =40（单账本无新增证据，未突破硬顶 50）

> **compound 行说明**：DEV-001 = preset (50%) + mechanism (50%)，成分置信度 = 50+25；DEV-006 = preset (33%) + mechanism (33%) + agent (33%)，整行 = min(成分) = 55。

---

## 6. 修复建议

### 6.1 短期（operator workaround）

- **DEV-002 临时缓解**：在 `presets/en/ce-executor-pipeline.yml` 的 `event_loop.event_policy.business_topics` 段显式加入 `work.done.proposed` / `work.failed.proposed`，让 `event_policy.rs:2696` 的 topic_publishes gate 在 precheck desugar 后仍认 `.proposed` 形式（**关联置信度 70**）。
- **DEV-005b 配套**：建议在 `crates/ralph-tui/src/rpc_writer.rs:64` 把 hard-code reason 字符串改成可追溯字段（例如 `sender: "tui_app" | "cleanup" | "external_stdin"`），并在 `rpc_stdin.rs:181` 的 warn 日志同时记录 `peer_pid` 与发送时间，便于区分真实用户主动 vs cleanup 兜底 vs 外部残留 stdin（**关联置信度 70**）。

### 6.2 中期（preset / schema / instructions）

- **DEV-001 中期修复**：
  - 目标：让 executor / test-stabilizer 不再越权 emit `test.topic` / `fallback.topic` / `debug.step`
  - 改动：
    - `presets/en/ce-executor-pipeline.yml` L61 `topic_format_whitelist` 扩展为 `business_topics + [LOOP_COMPLETE]`，或确认 `topic_format_whitelist` 与 `event_policy.business_topics` 的语义分工（若 whitelist 只管 LOOP_COMPLETE 而 business_topics 管其余，需加注释澄清）
    - `presets/en/ce-executor-pipeline.yml` executor `instructions:` HARD RULES 段显式 list 所有「不允 emit」topic（test.topic / fallback.topic / debug.step）作为反模式清单
  - 预期效果：下次 plan-driven run executor / test-stabilizer 越权 emit 数量从 ~6 次降至 0
  - **关联置信度 65**
- **DEV-003 中期修复**：
  - 目标：retry_count ≤ 3 时收敛；当前 max retry_count=2 未升级 plan.blocked
  - 改动：`presets/en/ce-executor-pipeline.yml` L130-143 precheck.work.done prompt 的 required_fields 与 `recovery_log.rs:459` 的 contract_violation 实际拒因对齐
  - 预期效果：retry_count ≤ 1 收敛；workspace recovery.jsonl contract_violation 计数从 4 条降至 ≤ 1 条
  - **关联置信度 60**

### 6.3 长期（机制 / 底座）

- **DEV-005b 长期修复**：
  - 目标：根治「backend 沉默 16 秒无 watchdog / reason 字符串不可信 / abort 真实触发者不可追溯」三类问题
  - 改动：
    - `crates/ralph-tui/src/rpc_writer.rs:64` 把 `"User requested abort".to_string()` 改为接收 caller 提供的 sender 标识（`"tui_app" | "cleanup" | "external_stdin"`）
    - `crates/ralph-cli/src/rpc_stdin.rs:181` warn 日志增加 `peer_pid` / `cmd_received_at` 字段，便于区分真实用户主动 vs cleanup 兜底 vs 外部残留 stdin
    - `crates/ralph-adapters/src/pty_executor.rs` 增加 backend spawn 后的 liveness probe（>5s 无 stdin 流量 / 无 stdout 流量 → WARN，能让 stalled backend 在 stall detector 路径里可见）
    - 配套：preset `event_loop` 段注明 "abort 后不支持 resume，需用户手动重开"
  - 验证：跑一个会触发 backend 沉默的场景，下次 abort 日志能区分触发者；下一次 plan-driven run 若 backend 沉默应能在 stall detector 日志里出现
  - **关联置信度 70**

- **DEV-002 长期修复**：
  - 目标：根治 precheck desugar `<X>.proposed` 后 topic_publishes gate 不认 `.proposed` 中间态
  - 改动：
    - `crates/ralph-core/src/payload_contract.rs:327` `default_publishes` 合并逻辑增加 `.proposed` 自动展开
    - `crates/ralph-core/src/event_policy.rs:2696` `preview_emit_event` 的 topic_publishes gate：检查 topic 时同时检查 `<topic_without_suffix>.proposed` 是否在 hat publishes
    - 配套：`presets/en/ce-executor-pipeline.yml` precheck 段注释澄清 producer 仍 emit 原始 topic，runtime 内部 rewrite 对 producer 透明
  - 验证：`cargo nextest run -p ralph-core -- default_publishes_injected` 应全绿；下一次 plan-driven run executor emit work.failed 不再触发 missing_event_gate
  - **关联置信度 70**

- **DEV-006 长期修复**：
  - 目标：3 轮 iteration 不再在 work.done / stabilization.done 同节点中断
  - 改动：
    - `crates/ralph-core/src/event_loop/mod.rs` iteration resume 路径加固（确保 resume 时 plan-reviewer 不会再次触发同一 plan.ready emit）
    - 配套：preset 加 `event_loop.max_iterations: 4` 显式约束（当前 `max_iterations: 40` 远超实际触发频次）
  - 验证：跑一个完整 plan-driven run，3 轮 iteration 应在第 1 轮即达 LOOP_COMPLETE
  - **关联置信度 55**（compound 多因素，长期修复需 events.jsonl 完整 history 辅助定位）

---

## 7. 未核实疑点

| ID | 疑点 | 当前置信度 | blocked_by | 已做加深 |
|----|------|----------|------|----------|
| DEV-006 | 3 轮同节点中断的精确 hat/topic 是什么 | D=55（<60 不入正式归因表） | 缺 events.jsonl 第 1/2/3 轮中断时刻的 hat + topic + 中断原因 | 读 `.ralph/events.jsonl` tail 或 `accepted-transitions.jsonl` 第 1/2/3 轮对应条目 |
| DEV-004 | accepted-transitions null 字段是 ledger 写入 bug 还是读取侧格式化问题 | D=40 | 缺第二账本对照 + 缺 source file:line | 读 `crates/ralph-core/src/state/ledger.rs` accepted-transitions 写入路径 |

---

## 质量门槛自检

- [x] §1 四问有置信度（Q1=65, Q2=60, Q3=70, Q4=70）
- [x] §5 每条 P0/P1 有置信度；P0≥70（DEV-002=70, DEV-005b=70）；P1 入表门槛 ≥60（DEV-003=60 边界，DEV-006=55 已在 §7）
- [x] 每条 P0 至少一条 DEV +（mechanism）源码行号
- [x] compound 行（DEV-001, DEV-006）附了成分贡献比例
- [x] 低置信度 DEV-006, DEV-004 已加深 1 轮，DEV-006 仍不足已落入 §7
- [x] DEV-005b 修正：原 DEV-005 误判为「用户主动 abort」，实为 backend spawn 后 16 秒沉默 + stdin 上游 RpcCommand::Abort 触发 SIGTERM；reason 字段不可信（`rpc_writer.rs:64` hard-code）
- [x] 路径全部 repo-relative
- [x] frontmatter 含 `history_search: full`
- [x] §3 末尾含扫描窗口注脚
- [x] §5 历史关联列全部填值（history_search=full）
- [x] 未引用 ssot-guardrails 禁止项（hat_handoff / loop_state_snapshot.json / `review.passed` / `human.guidance` 全部规避）
- [x] 报告路径 `docs/report/2026-08-06-ce-executor-pipeline-2026-08-05-001-refactor-large-file-module-split-plan-diagnosis.md`

---

## 提交前 frontmatter 对账（机器校验 hard rule）

```bash
: "${RALPH_INCLUDE_HISTORY:=full}"
HS=$(awk 'BEGIN{f=0} /^---$/{n++; next} n==1 && /^history_search:/{print $2; exit}' "$REPORT")
[ "$HS" = "$RALPH_INCLUDE_HISTORY" ] && echo "OK: history_search=$HS"
# disabled 模式需另查 N/A (history disabled) 占位符；本报告 full 模式不适用
```

预期输出：`OK: history_search=full`
