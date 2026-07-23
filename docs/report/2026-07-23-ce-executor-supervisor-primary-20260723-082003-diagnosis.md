---
title: ce-executor-supervisor Loop `primary-20260723-082003` 运行链路诊断报告
date: 2026-07-23
type: diagnosis
loop_id: primary-20260723-082003
preset: builtin:ce-executor-supervisor
run_dir: ralph-supervisor
status: 部分偏离：执行波失败，恢复路径未闭合，未产生 LOOP_COMPLETE
diagnostics_mode: LOGS_ONLY
---

# ce-executor-supervisor Loop `primary-20260723-082003` 运行链路诊断报告

> **诊断对象**：`ralph-supervisor/.ralph/`；唯一可信事件文件由 `current-events` 指向 `events-20260723-082003.jsonl`。
> **对照**：`presets/en/ce-executor-supervisor.yml`、`presets/schemas/ce-executor-supervisor.yml`。
> **execution_capabilities**：`[supervisor, wave]`；由 `event_loop.supervisor.enabled: true`、hat instructions 中的 `ralph wave emit`、`.ralph/supervisor.db` 和事件中的 `wave_id` 共同确认。
> **报告仓库**：`ralph-orchestrator` 主仓；本报告不是 run_dir 内运行时状态文件。
> **诊断盲区**：LOGS_ONLY 没有 orchestration/agent-output session；因此 OPAC 单项置信度不超过 50，纯 agent 归因不作定论，整条根因通常不超过 75。

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数/规模 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` → `events-20260723-082003.jsonl` | 是 | 11 | 唯一事件 SSOT；含 1 个 wave |
| S | `events-history-20260723-082003.jsonl` | 是 | 1 | 旁路历史，不替代主事件 |
| S | `.ralph/ledger.jsonl` | 是 | 5 | iteration 1–4、6；无拒收提交 |
| S | `.ralph/recovery.jsonl` | 是 | 2 | 均为 `plan.blocked` repair_dispatch Info |
| S | `.ralph/loops.json` / `current-loop-id` | 是 | — | loop=`primary-20260723-082003` |
| S | `.ralph/loop.lock` | 否 | — | 已释放 |
| A | `.ralph/agent/tasks.jsonl` | 是 | 5 | u1–u5 全为 `open`，owner=`coordinator` |
| A | `progress.md` / `summary.md` / `handoff.md` | 否 | — | 终止前未生成，属条件产物缺失 |
| B | `.ralph/supervisor.db` | 是 | 约 4 KiB | 1 wave、5 slots、5 dispatch records、4 worker results |
| B | `diagnostics/logs/` | 是 | 2 文件/83 行 | 因无 session orchestration，模式为 LOGS_ONLY |
| B | hat-channel / orphan diagnostics | 部分 | — | coordinator 空 channel 回退；3 个 worktree orphan events 被记录 |
| C | preset 计划业务产物 `sorts/**/*` | 否 | 0 | 5 个 slot 未完成 fan-in，不能判定为已生成 |

Tier C 预期由本次 plan 的 5 个 unit 定义：`sorts/` 骨架、4 种算法、各自测试、README 与集成测试；实际主 workspace 无这些文件。`.ralph/supervisor.db` 和 `wave_id` 均为本能力集要求的产物，二者存在/出现，不是缺失故障。

## 1. 结论摘要

### 1.1 健康度

- **判定**：执行波在 slot 4 超时后进入失败恢复；slot 0–2 的 worker 事件落入 worktree-local orphan ledger，主账本不可见；修复后 `plan.blocked` 无消费者，形成终态死路。
- **问题数量**：P0 0（本次 LOGS_ONLY 下将可疑机制/编排项降为 P1，避免超出证据上限）；P1 6；P2 1；均达到入表置信度门槛。
- **最高根因置信度**：75/100（LOGS_ONLY 上限；机制证据虽有源码行，但 recovery 未记录对应根因码）。
- **历史复发**：是；orphan emit、supervisor fan-in/生产接线、空 hat-channel、task 全 open、`review_terminal_drift` 均有高或中关联历史；本次是这些家族的组合，不足以仅凭症状定为全新机制。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 执行与 OPAC 是否合规？ | ⚠️ 执行拓扑可还原；OPAC 只能弱审计 | 11 条事件合同可逐条通过；LOGS_ONLY 看不到完整 precheck/Confirm | 50 |
| Q2 | 基座机制是否生效？ | ⚠️ 部分生效 | supervisor 能创建 wave、识别 slot 4 failure、写 recovery；但 worker 结果可见性与任务闭合未完成 | 75 |
| Q3 | 编排是否合理并正常运行？ | ❌ | `exec.wave.failed` 后进入 fallback；`plan.blocked` 没有 consumer；无 `LOOP_COMPLETE` | 75 |
| Q4 | 归因是什么？ | compound：机制可见性/slot 结果处理为主，preset 终态闭合为次 | 见 §5；不能把 slot4 内部原因归为 agent 定论 | 75 |

### 1.3 根因一句话

worker 结果至少部分写入 worktree-local `.ralph/events.jsonl`，没有形成主账本可见的 `exec.unit.done`；slot 4 随后超时触发失败恢复，而 preset 又允许 `progress-steward` 发出无人消费的 `plan.blocked`，所以执行与终态两条链都未闭合（置信度 75）。

## 2. 执行链路对比

### 2.1 激活表

| Hat | 次数 | 结果 |
|---|---:|---|
| coordinator | 1（另一次被触发但无业务结果） | `work.ready` 成功；后续未产出 `plan.complete` |
| task-planner | 1 | 单批 `exec.unit.ready` ×5 |
| worker | 5 个 slot 被 dispatch；主账本 0 条 done | 0–2 有 orphan 证据，4 超时，3 缺证据 |
| exec-failure-handler | 1 | `exec.wave.failed` → `work.failed` |
| fixer | 1 | `fix.applied` |
| progress-steward | 1 | `plan.blocked` |
| exec-integrator、review-coordinator、review-batch-worker×6、review-synthesizer、fix-task-planner、fix-worker、fix-integrator、alignment、shipper、reporter | 0 | 上游事件未到达或无对应触发 |

### 2.2 实际时间轴与预期

| # | 事件 | 实际 | 预期对照 |
|---:|---|---|---|
| 1 | `plan.ready` | bootstrap 入场 | ✅ |
| 2 | `work.ready` | coordinator 注册 5 个 open task | ✅（任务注册成功，未闭合） |
| 3–7 | `exec.unit.ready`×5 | task-planner 同批 wave，slot 0–4 | ✅ |
| 8 | `exec.wave.failed` | `blocking_slots=[4]`、`required_slot_failure` | ⚠️ worker fan-in 未达成 |
| 9 | `work.failed` | failure handler 转发 u5 | ✅ |
| 10 | `fix.applied` | fixer 1 commit、316 changed lines | ✅ fallback；不等于完整 review/fix wave |
| 11 | `plan.blocked` | reason=`review_terminal_drift` | ❌ preset 无 consumer；应有可消费的终态路径 |
| — | `LOOP_COMPLETE` | 未出现 | ❌ |

终止类型：`plan.blocked` 后自然无法继续，用户随后中止；不是自然成功完成，也不是 silent-success。

## 3. 历史问题上下文

| 问题类型 | 代表路径 | 本次关联 | 闭环状态 |
|---|---|---|---|
| supervisor 生产接线 / fan-in acceptance gap | `docs/report/2026-07-22-ce-executor-supervisor-primary-20260722-084810-diagnosis.md` | 高 | 历史 closure 曾处理部分路径，本次仍出现相同家族 |
| workspace root shadowing / orphan emit | `docs/solutions/integration-issues/emit-workspace-root-cwd-drift.md` | 高 | 既有诊断/fallback，运行时仍观测到 orphan |
| hat-channel empty after activation | `docs/report/2026-07-22-ce-executor-supervisor-primary-20260722-084810-diagnosis.md` | 高 | 部分闭环；本次再次触发 fallback |
| task ownership / 全 open | `docs/report/2026-07-22-ce-executor-supervisor-primary-20260722-084810-diagnosis.md` | 中 | 历史曾列疑点；本次仍未闭合 |
| `review_terminal_drift` | `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` | 中 | 历史多次修复，仍作为本次 fallback reason |
| multi-consumer pending drain | `docs/solutions/logic-errors/isolated-ralph-must-not-drain-multi-consumer-pending.md` | 低—中 | 机制已解决，不能直接当作本次根因 |

## 4. 证据清单

| ID | 描述 | 锚点 | 初判 | 初估 | 缺口 |
|---|---|---|---|---:|---|
| DEV-001 | worker 事件至少部分落入 nested worktree orphan ledger | orphan diagnostics；主 events 无 `exec.unit.done` | P1 | 75 | slot 3/4 agent stdout 不可见 |
| DEV-002 | DB 把 4 个 worker result 记录为 completed，但主事件不可见 | `supervisor.db` `wave_slots`/`worker_results`；主 events | P1 | 75 | completed 的确切语义需完整 runtime trace |
| DEV-003 | slot 4 300 秒无事件后失败 | DB failure reason；主 events `exec.wave.failed` | P1 | 75 | 无 agent-output，不能判断内部原因 |
| DEV-004 | 5 个任务持续 open | `agent/tasks.jsonl`；日志多次 5 open/0 closed | P1 | 75 | 无 task transition 账本 |
| DEV-005 | coordinator 空 hat-channel 触发 fallback | `channel-routing-fallback-*.md`；日志 | P1 | 50 | LOGS_ONLY，不可升级为 agent/OPAC 根因 |
| DEV-006 | `plan.blocked` 无声明 consumer | preset progress-steward `triggers`/`publishes`；主事件 | P1 | 75 | 未执行 preset lint 复核 |
| DEV-007 | declared flow 未覆盖实际 fallback 到 blocked 的完整链 | preset `mechanism.flow`；事件 8–11 | P1 | 75 | 是否由通用 runtime 补全需源码/BDD确认 |
| DEV-008 | progress/summary/handoff 与业务产物缺失 | Phase 0 盘点 | P2 | 75 | 终止前生成条件未完全满足 |

### 4.1 OPAC 审计（LOGS_ONLY）

| Hat | Observe | Precheck | Apply | Confirm | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| coordinator | ✅/⚠️ | N/A | ✅ 首次 | N/A | tasks 与 `work.ready` 可见；命令轨迹缺失 | 50 |
| task-planner | ✅ | N/A | ✅ 单批 wave | N/A | 5 个 ready 同批落主账本 | 50 |
| worker | ⚠️ | N/A | ⚠️ | N/A | orphan/timeout；无法确认 precheck 与 main Confirm | 50 |
| exec-failure-handler | ✅ | N/A | ✅ | N/A | injected failure 与 `work.failed` 对应 | 50 |
| fixer | ✅ | N/A | ✅ | N/A | `fix.applied` 落主账本 | 50 |
| progress-steward | ✅ | N/A | ✅ | N/A | `plan.blocked` 与 recovery 双记录 | 50 |

> LOGS_ONLY 下 `N/A` 表示没有可审计命令轨迹，不表示已违规；未见 `--policy-check` 不能单独构成 P0。

## 5. 问题归因与置信度

| 优先级 | 问题 | 分类 | 置信度 | DEV | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|
| P1 | worker 结果可落入 worktree-local ledger，主账本/ fan-in 不可见 | mechanism | 75 | 001 | 高 | 1：补读 orphan、DB、`emit.rs` 与 `worktree_bind.rs`；75 |
| P1 | 空/不可见 worker 结果仍进入 completed 记账，造成 fan-in 语义不一致 | mechanism | 75 | 002 | 高 | 1：补读 dispatcher 结果记录路径；75 |
| P1 | slot 4 超时触发 required-slot failure；内部 agent 原因未核实 | agent/mechanism 候选 | 75 | 003 | 中 | 2：DB+日志仍缺 agent-output；不把内部原因定为 agent |
| P1 | 任务完成状态未闭合，导致修复后无法满足 completion 条件 | compound：mechanism 50% + preset 50% | 75 | 004 | 中 | 1：tasks/events/preset 对账；75 |
| P1 | `plan.blocked` 没有下游 consumer，终态无法到 reporter | preset | 75 | 006 | 高 | 1：逐 hat triggers 扫描；75 |
| P1 | declared flow 未覆盖 failure/recovery 到 blocked 的实际路径 | preset / mechanism | 75 | 007 | 中 | 1：preset flow 与实际时间轴对照；75 |
| P1 | coordinator 空 hat-channel 后的运行证据不足 | mechanism 候选 | 50 | 005 | 高 | 2：仍缺 agent-output，保留弱结论；不驱动修复 |
| P2 | 业务进度、终止摘要和主 workspace 产物缺失 | compound | 75 | 008 | 中 | 1：盘点确认；75 |

> 由于 LOGS_ONLY 硬顶，以上 P1 采用保守 75；`slot 4` 的 agent 内部失败原因、具体 task transition 缺口和空 channel 的触发根因仍不能定论。

## 6. 修复建议

仅针对 §5 已入表项；不把 §7 疑点转成修复结论。

### 6.1 短期 operator workaround

1. 在重新运行前以 `ralph diagnose --full` 获取 orchestration/agent-output，并确认 worker emit 的目标路径；关联 DEV-001/003/005，置信度 75。
2. 失败后先确认 `tasks` 状态与主账本的 worker 完成事件一致，再允许进入修复/终态；关联 DEV-004，置信度 75。

### 6.2 中期 preset/schema/instructions

1. 为 `plan.blocked` 声明明确的终态 consumer 或把 blocked 路径定义为可审计的 operator terminal；同步 schema、triggers/publishes 与 BDD；关联 DEV-006，置信度 75。
2. 将 `work.failed → fix.applied/fix.exhausted → plan.blocked/plan.complete` 的恢复语义纳入 preset 的 declared flow，并跑结构化 preset lint；关联 DEV-007，置信度 75。
3. 明确 task close 的责任 hat、完成字段和失败字段，避免 task view 与 supervisor 结果分叉；关联 DEV-004，置信度 75。

### 6.3 长期机制/底座

1. 在 supervisor worker spawn 边界审计 workspace/event 环境透传，确保 worktree cwd 不会把业务 emit 隐式写入 nested `.ralph/events.jsonl`；关联 DEV-001，置信度 75。
2. 对 slot result 增加“事件计数/内容哈希/主账本可见性”的一致性门禁，禁止空结果被当作 completed；关联 DEV-002，置信度 75。
3. 对空 hat-channel 的 fallback 增加可观测且可恢复的状态转换，避免在 LOGS_ONLY 下只留下“空 channel”而无 activation 证据；关联 DEV-005，置信度 50。

## 7. 未核实疑点

以下均低于 60 或受 LOGS_ONLY 证据封顶约束，不驱动修复建议：

| 候选 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| slot 4 是进程崩溃、超时还是 agent 自身未 emit | 40 | 缺 worker stdout/stderr 与 agent-output | DB + 两份 logs |
| coordinator 空 channel 的具体 race/marker 状态 | 50 | 缺 orchestration 与 activation trace | fallback diagnostic + source read |
| `presets/index.json` 是否存在另一路径漂移 | 30 | 未做 preset manifest 专项校验 | 盘点未命中 |

## 8. 机制生效矩阵与质量结论

| 机制 | 判定 | 证据 |
|---|---|---|
| origin/payload/publish contract | ✅ | 主事件 required fields 与 publishes 对得上 |
| supervisor wave dispatch | ⚠️ | 5 slots dispatch；slot 4 timeout |
| worker event collection/fan-in | ❌ | orphan events；DB 与主账本不一致 |
| isolated 单事件预算 | ✅ 未见违反 | 11 条主事件无同 activation 多业务证据 |
| task 三字段 | ✅ shape；❌ lifecycle | payload identity 可对齐，5 task 全 open |
| recovery | ⚠️ | failure 有 recovery，但只记录最终 blocked repair |
| hat-channel routing | ⚠️ | 空 channel diagnostic + fallback |
| terminal/completion | ❌ | 无 `plan.complete`、无 `LOOP_COMPLETE` |

**最终门禁**：Phase 0–3 均有产出；唯一 events 指针纪律满足；LOGS_ONLY 未因缺 orchestration 标机制 P0；所有入表项 confidence≥60，P0未虚标；低置信度疑点独立放入 §7；未引用禁止的已删除概念或错误路径。
