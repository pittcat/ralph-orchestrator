---
title: implementation-review Loop `primary-20260727-051801` 运行链路诊断报告
date: 2026-07-27
type: diagnosis
loop_id: primary-20260727-051801
preset: presets/en/implementation-review.yml
run_dir: ../ralph-e2e
status: 部分偏离：wave fan-in 将已有 review 结果判为失败，最终安全阻断
diagnostics_mode: LOGS_ONLY
history_search: preset-only
execution_capabilities: [wave]
---

# implementation-review Loop `primary-20260727-051801` 运行链路诊断报告

> **生成时间**：2026-07-27
> **诊断对象**：`../ralph-e2e/.ralph/`（loop_id=`primary-20260727-051801`）
> **对照 preset**：`presets/en/implementation-review.yml` + `presets/schemas/implementation-review.yml`
> **执行方式**：Phase 0 产物盘点 → Agent A/B/C/D 分阶段只读调查 → 主 Agent 汇总
> **Diagnostics 模式**：`LOGS_ONLY`
> **history_search**：`preset-only`（最近 30 天、preset/loop/symptom 相关条目）
> **execution_capabilities**：`[wave]`；preset 的 dispatcher instructions 含 `ralph wave emit`，events 含 `wave_id=w-rs-1`；`event_loop.supervisor.enabled` 未开启，因此 `.ralph/supervisor.db` 只作为 default-wave ledger 证据，不把本 run 归类为 supervisor capability
> **报告仓库**：`ralph-orchestrator` 主仓；报告不写入 run workspace
> **唯一 events SSOT**：`../ralph-e2e/.ralph/current-events` → `../ralph-e2e/.ralph/events-20260727-051801.jsonl`
> **置信度规则**：§5 仅收录 confidence≥60；P0 须 confidence≥70；LOGS_ONLY 下 agent/OPAC 不能单独构成 P0

---

## 0. 产物盘点（Phase 0）

### 0.1 产物盘点表

| Tier | 路径 | 存在 | 行数/大小 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` | 是 | 指向 1 个文件 | 仅此指针解析出的 events 用于编排对账 |
| S | 指针 → `.ralph/events-20260727-051801.jsonl` | 是 | 16 行 | 唯一可信主 events，含终态 |
| S | 配对 `events-history-20260727-051801.jsonl` | 是 | 2 行 | 旁路历史，不作为编排 SSOT |
| S | `.ralph/ledger.jsonl` | 是 | 5 行 | loop counter/completion ledger；未扩展为业务事件源 |
| S | `.ralph/flow-authority.jsonl` | 是 | 3 行 | flow authority 记录 |
| S | `.ralph/recovery.jsonl` | 否 | N/A | 无 workspace recovery；在 LOGS_ONLY 下不能据此证明无拒收 |
| S | `.ralph/loops.json` | 是 | `{"loops": []}` | 当前 loop 由 `current-loop-id` 与 lock/事件关联 |
| S | `.ralph/current-loop-id` | 是 | `primary-20260727-051801` | 与目标 loop 一致 |
| S | `.ralph/loop.lock` | 终态后已释放 | — | Phase 0 冻结时 run 已终止 |
| A | `.ralph/agent/tasks.jsonl` | 是 | 6 行 | 6 个 supervisor slot 均 `failed`，task 均 closed |
| A | `.ralph/agent/summary.md` | 是 | 23 行 | 写 `Completed successfully`，但未反映业务 `result=blocked` |
| A | `.ralph/agent/handoff.md` | 是 | 48 行 | 列出 6 个 `w-2` slot 为 remaining，和 tasks closed/终态存在语义差异 |
| A | `.ralph/agent/progress.md` | 否 | N/A | preset 未启用该独立 progress artifact；不判丢失 |
| B | `.ralph/diagnostics/logs/*.log` | 是 | 2 个文件 | 仅日志证据，决定 `LOGS_ONLY` |
| B | `.ralph/diagnostics/wave-w-rs-1-slots.json` | 是 | 707 bytes | `injected_failed`，6 slots 均 `empty_worker_result` |
| B | `.ralph/diagnostics/channel-routing-fallback-*.md` | 是 | 7 行 | dispatcher hat-channel 空回退诊断 |
| B | `.ralph/diagnostics/agent_doc_sync.json` | 是 | 6 行 | `synced=0, skipped=2, failed=0`；与主因无直接因果证据 |
| B | `.ralph/supervisor.db` | 是 | 118784 bytes | default-wave ledger 证据；本报告不读取其内部表，不据此猜测中间状态 |
| C | `.ralph/review/<plan>/scope-analysis.md` | 是 | — | scope freeze 产物 |
| C | `.ralph/review/<plan>/review-context.md` | 是 | — | review context |
| C | `.ralph/review/<plan>/review.diff.patch` | 是 | 12401 bytes | frozen patch |
| C | `.ralph/review/<plan>/scope-manifest.json` | 是 | — | digest/HEAD identity |
| C | `.ralph/review/<plan>/dispatch-batch/payloads.jsonl` | 是 | 6 行 | 六个 immutable payload |
| C | `.ralph/review/<plan>/dimensions/*.md` | 是 | 6 个文件 | 六个 dimension artifact，findings 总数 22 |
| C | `.ralph/review/<plan>/wave-blocked.md` | 是 | 18 行 | 终态 blocked artifact，`missing_dimensions=[testing]` |
| C | `synthesized-review.md` / `fix-plan.md` | 否 | N/A | 失败分支未触发；按 preset 失败路径不判丢失 |
| C | `review-blocked.md` / `scope-blocked.md` | 否 | N/A | 本 run 未走对应 block 分支，不判丢失 |

### 0.2 终态与能力判定

- `events` 最后一条为 `LOOP_COMPLETE`，payload 为 `result=blocked`，`artifact_path` 指向可读的 `wave-blocked.md`；因此本 run **有终态**，不是无终态 stale loop。
- `review-worker` 六个 dimension artifact 都存在，且六个 `review.unit.done` 都出现在唯一主 events；但 `.ralph/diagnostics/wave-w-rs-1-slots.json` 同时记录六个 slot 为 `failed/empty_worker_result`。
- `review.wave.failed` 是 preset 明确允许的 runtime coordination failure 分支，最终由 finalizer 写 `wave-blocked.md` 并发布唯一 `LOOP_COMPLETE`；该 blocked 收尾本身符合 preset 的安全失败契约，不把“没有 synth/fix artifact”单独列为 preset 缺陷。

### 0.3 Diagnostics 盲区

本 run 是 `LOGS_ONLY`：没有 session `orchestration.jsonl` / `agent-output.jsonl`，workspace `recovery.jsonl` 也不存在。因此：

- 只能用 events + workspace artifacts + logs 证明现象和部分 runtime 机制行为；
- 无法证明每个 worker activation 内实际执行了哪条命令、是否先执行 `--policy-check`、哪个具体 channel 文件写入失败；
- 无法读取 `.ralph/supervisor.db` 内部 slot/evidence 状态，不能把 `empty_worker_result` 进一步断言为某个具体 SQL/transport 分支；
- agent/OPAC 单项置信度按 LOGS_ONLY 上限≤50；机制项只有在 events/产物与源码行号形成闭合时才可高于该上限。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**：部分偏离，最终安全阻断，但 review 结果未完成聚合。
- **P0**：1（confidence≥70）。
- **P1**：1（confidence≥60）。
- **P2**：0。
- **最高优先级根因置信度**：P0-1 = **97/100**。
- **历史复发**：是；近 30 天内至少 4 个高度相关、尚未完全闭合的 wave fan-in / salvage / provenance 计划或诊断条目，本次属于同源问题的再次命中。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 编排终态合规，OPAC 证据不足 | 事件拓扑最终走 `review.wave.failed → finalizer → LOOP_COMPLETE`，但 LOGS_ONLY 未见完整 precheck/agent-output，且 wave 结果与主 events 互相矛盾 | 50 |
| Q2 | 基座机制是否正常生效？ | ❌ wave fan-in / provenance 对账未正常收敛 | runtime 的 `CompletedWave`/slot 结果为 0 成功、6 失败，而 raw main backscan 又扣除 5 个维度；随后发生 5 次 out-of-scope drop | 97 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ 失败分支合理，成功聚合未完成 | scope freeze、六路 dispatch、六个 artifact、blocked finalizer 都闭合；但 `review.wave.complete`、synth、fix 未触发 | 97 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **机制主导，producer provenance/transport 为复合面；无独立 agent 定论** | 私有 worker 结果、raw main backscan、scope filtering 三处真值源不一致；LOGS_ONLY 不足以定 agent 责任 | 97 |

> Q1 的 50 是 OPAC 审计置信度，不是对运行链路健康度的评价；Q2/Q3 评价的是可由事件、产物和源码闭合的机制/编排事实。

### 1.3 根因一句话

本次 review 的权威 worker 结果来自每个 slot 的私有事件文件；但失败 payload 的 `main_backscan` 又直接采信原始主 events 中带同一 `wave_id` 的 `review.unit.done`，没有检查这些行是否随后被 isolated scope drop。结果是：slot/fan-in ledger 将 6 个 worker 判为 `empty_worker_result`，raw main backscan 却扣除了 5 个维度，缺少 `wave_id` 的 testing 才被列入 `missing_dimensions`；随后 provenance 缺失的 5 条事件又触发 scope drop 与 dispatcher hard-gate。**根因置信度：97/100。**

---

## 2. 执行链路对比图

### 2.1 拓扑激活表

| Hat | 预期触发 | 实际激活 | 实际输出 | 结果 |
|---|---|---:|---|---|
| `scope-preparer` | `review.start` | 1 | `scope.ready` | ✅ scope clean |
| `review-dispatcher` | `scope.ready` | 1 次成功 dispatch，之后至少 1 次 hard-gate retry | 6 条 `review.unit.ready` | ⚠️ 二次 activation 的 hat-channel 为空 |
| `review-worker` | 6 条 `review.unit.ready` | 6 slots | 6 个 `review.unit.done` + 6 个 dimension artifact | ⚠️ 主 events 有结果，但 slot ledger 全 failed |
| `review-synthesizer` | `review.wave.complete` | 0 | — | ⏸️ runtime 未注入 complete |
| `fix-planner` | `review.synthesized` | 0 | — | ⏸️ 上游未触发 |
| `finalizer` | `review.wave.failed` | 1 次 failed path，随后发布终态 | `wave-blocked.md` + `LOOP_COMPLETE` | ✅ blocked path |

### 2.2 预期 vs 实际时间轴

> 事件行号按唯一可信 `events-20260727-051801.jsonl` 的 1-based 行号；`review.unit.done` 的 `wave_id=w-rs-1` 在事件 envelope 中存在，但其中 5 条缺少 `hat/source`，最后一条带 `hat=review-worker/source=review-worker`。

| # | events 行 | 时间（UTC） | topic | hat/source | payload 摘要 | 状态 |
|---:|---:|---|---|---|---|---|
| 1 | 1 | 05:18:01.449 | `review.start` | `loop-bootstrap` | plan payload | ✅ |
| 2 | 2 | 05:20:26.656 | `scope.ready` | `scope-preparer` | scope/patch/head/digest 全齐，`dirty_verdict=clean` | ✅ |
| 3 | 3–8 | 05:21:58.798 | `review.unit.ready` ×6 | `review-dispatcher` | `wave_id=w-rs-1`，slot 0–5，六个 payload 同一 idempotency key | ✅ |
| 4 | 9 | 05:22:58.295 | `review.unit.done` | 无 hat/source | `goal-alignment`, findings=0 | ⚠️ provenance 缺失 |
| 5 | 10 | 05:23:31.372 | `review.unit.done` | 无 hat/source | `correctness`, findings=2 | ⚠️ provenance 缺失 |
| 6 | 11 | 05:23:32.878 | `review.unit.done` | 无 hat/source | `maintainability`, findings=6 | ⚠️ provenance 缺失 |
| 7 | 12 | 05:24:13.919 | `review.unit.done` | 无 hat/source | `adversarial`, findings=5 | ⚠️ provenance 缺失 |
| 8 | 13 | 05:24:28.842 | `review.unit.done` | 无 hat/source | `testing`, findings=5 | ⚠️ provenance 缺失；后续被列为 missing |
| 9 | 14 | 05:25:04.029 | `review.unit.done` | `review-worker` | `project-standards`, findings=4 | ✅ 仅此条带 producer attribution |
| 10 | 15 | 05:25:16.644 | `review.wave.failed` | `hat=finalizer`, `source=ralph`, system-injected | `missing_dimensions=[testing]`, `reason=required_slot_failure` | ❌ 与 events/维度 artifact 冲突 |
| 11 | 16 | 05:28:51.557 | `LOOP_COMPLETE` | `finalizer` | `result=blocked`, artifact=`wave-blocked.md` | ✅ 安全失败终态 |

预期成功路径为：

```text
review.start
  → scope.ready
  → review.unit.ready × 6
  → review.unit.done × 6
  → review.wave.complete
  → review.synthesized
  → fix.plan.ready
  → LOOP_COMPLETE{clean|residual_only|fixes_required}
```

本次实际路径为：

```text
review.start
  → scope.ready
  → review.unit.ready × 6
  → review.unit.done × 6（5 条缺 hat/source）
  → review.wave.failed{missing_dimensions:[testing]}
  → finalizer 写 wave-blocked.md
  → LOOP_COMPLETE{blocked}
```

### 2.3 运行时偏离点

- 日志 `ralph-...424-830946.log:24-25` 记录 `Wave completed results=0 failures=6`、随后 `fan_in=InjectedFailed`；这与 events 行 9–14 六个 done 和六个 dimension artifact 同时存在相矛盾。
- 日志 `:26-30` 连续记录 5 次 `event out of hat scope — dropping hat=review-dispatcher topic=review.unit.done`；该行为符合 scope 机制对 dispatcher 未声明 topic 的拒绝，但它暴露出失败收敛阶段仍有 dispatcher 侧 `review.unit.done` 输入。
- 日志 `:37-38` 记录 `hat_channel_empty_after_activation` 与 dispatcher hard gate；随后 `:44-50` 才出现 `LOOP_COMPLETE` 与 landing，说明终态被二次 activation 延迟，而不是在 failed coordination 后立即完成。

---

## 3. 历史问题上下文

> **历史开关**：`preset-only`；仅扫描主仓近 30 天且与本 preset/loop/symptom 相关的 `docs/report/`、允许的 `docs/solutions/` 子目录、active `docs/plans/` 与 `docs/brainstorms/`。历史扫描仅用于复发判断，以当前 SSOT 护栏和源码为本次机制事实准绳。

| 历史项 | 问题类型 | 窗口内次数 | 闭合状态 | 与本次关联 |
|---|---|---:|---|---|
| `docs/plans/2026-07-25-003-fix-supervisor-wave-worker-emit-channel-plan.md` | worker emit channel/store 错位 | ≥2 | 部分闭合，仍有 residual | 高：同样是 worker 结果落盘面与 fan-in 读取面不一致 |
| `docs/plans/2026-07-25-005-fix-supervisor-slot-activity-salvage-redrive-plan.md` | failed 前 completed-slot salvage 缺口 | ≥2 | 未闭；U1 characterization 仍为 RED | 高：本次 6 个业务 artifact 未形成可聚合 review 结论 |
| `docs/plans/2026-07-26-003-fix-review-wave-failed-convergence-plan.md` | `review.wave.failed` 失败路径收敛 | ≥2 | design-ready，未完全闭合 | 高：本次正命中 review wave failed 分支 |
| `docs/plans/2026-07-26-004-fix-supervisor-wave-contract-closure-plan.md` | main/supervisor ledger provenance 断层 | ≥2 | design-ready，未完全闭合 | 高：本次 5/6 done 缺 producer attribution |
| `docs/plans/2026-07-27-001-fix-wave-terminal-fan-in-convergence-plan.md` | terminal fan-in / ContinueCollect 收敛 | 2 个最新诊断命中 | 未闭 | 高：本次最后 done 后约 12.6 秒即进入 failed fan-in |
| `docs/report/2026-07-27-implementation-review-primary-20260727-023002-diagnosis.md` | implementation-review wave fan-in 复发 | 1 个直接报告 | 未闭 | 中高：同 preset、同类终态/协调面 |

**历史关联结论**：本次不是新问题家族，而是上述未闭合 wave fan-in、producer provenance、failure salvage 交接面的再次触发；历史关联只用于复发判断，不替代本次 run 的证据。

本次扫描窗口：preset-only (30d sliding)

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 证据缺口 |
|---|---|---|---|---:|---|
| DEV-001 | 主 events/业务 artifact 有 6 个 done，但 worker 私有 slot 结果全为 failed/empty；raw main backscan 又扣除 5 个维度，最终把 testing 列为 missing | `events-20260727-051801.jsonl:9-16`；`wave-w-rs-1-slots.json:1-35`；`diagnostics/logs/...424-830946.log:24-25`；`dispatcher.rs:1284-1348, 2954-2980, 3158-3213` | P0 | 95 | 未读 supervisor.db；无 recovery/session agent-output，无法确认 producer/transport 的具体失败阶段 |
| DEV-002 | 5/6 `review.unit.done` 缺 hat/source，失败 fan-in 后又出现 dispatcher scope drop 与空 channel hard-gate | `events...jsonl:9-14`；`diagnostics/logs/...424-830946.log:26-38`；`channel-routing-fallback-2026-07-27T05-27-10.md:2-6` | P1 | 65 | 缺 worker channel 原文和 agent-output，无法判断 attribution 在 worker、channel merge 还是 retry 期间丢失 |
| DEV-003 | summary/handoff 与终态/task ledger 视角不一致 | `summary.md:2,10-22`；`handoff.md:9-18`；`tasks.jsonl:1-6`；日志 `:49-50` | P2 | 45 | 缺 landing 源码对 failed/open 语义的完整对账；不作为根因定论 |
| DEV-004 | scope-preparer 出现 already-closed activation warning | `diagnostics/logs/...424-830946.log:10` | P2 | 30 | 缺 activation 生命周期完整日志；scope.ready 最终已成功，不单独驱动修复 |

### 4.1 OPAC 逐 hat 审计表（LOGS_ONLY）

> `Confirm=N/A` 或弱确认在 LOGS_ONLY 下允许；未见 `--policy-check` 日志不单独升为 P0。每项置信度≤50。

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| `scope-preparer` | ✅ | ⚠️ | ✅ | ⚠️ | scope.ready 10 字段齐全且 scope artifacts 存在；日志有 already-closed warning，未见 precheck 命令 | 30 |
| `review-dispatcher` 首次 | ✅ | ⚠️ | ✅ | ⚠️ | 6 个 ready、6 行 payloads.jsonl、wave_id/idempotency 一致；未见 `wave verify` 命令日志，worker fan-in 结果不一致 | 35 |
| `review-worker` ×6 | ✅ | ⚠️ | ⚠️ | ⚠️ | 6 个 dimension 文件和 done events 存在；5/6 producer attribution 缺失，slot JSON 全 failed；无 agent-output | 30 |
| `review-dispatcher` 二次 activation | ✅ | N/A | ❌ | ❌ | fallback artifact + 日志 `hat_channel_empty_after_activation`、hard gate；无业务事件输出 | 50 |
| `finalizer` failed path | ✅ | ✅ | ✅ | ✅ | `review.wave.failed` 触发，`wave-blocked.md` 可读且 digest/字段完整 | 50 |
| `finalizer` terminal emit | ✅ | ✅ | ✅ | ✅ | `LOOP_COMPLETE` 五个 required fields 齐全，终态事件可在 main events 确认 | 50 |

### 4.2 Prompt visibility 对账

三个关键 hat 的 `inspect prompt --format json` 结果一致：

- `auto_inject`：`ralph-tools`、`ralph-tools-memories`、`ralph-tools-opac`；
- `on_demand`：`ralph-tools-cmdref`、`ralph-tools-emit`、`ralph-tools-precheck`、`ralph-tools-recovery-directives`、`ralph-tools-tasks`、`ralph-tools-wave`；
- 因此 dispatcher/worker/finalizer instructions 中引用 emit/wave 命令时，不能把这些文档当作自动注入；应显式要求按需加载。当前可见性结果本身未证明某个 agent 实际漏加载，只构成审计约束。

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|
| P0 | worker 私有 slot 结果与 raw main backscan 对同一 wave 给出互斥判定：私有结果侧 6 个 slot 均 `failed/empty_worker_result`，raw main backscan 却扣除 5 个维度，最终只生成 `missing_dimensions=[testing]` | **mechanism**（worker 私有结果、scope filtering、failed fan-in reconciliation 的真值源不一致） | **97** | DEV-001；`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:1284-1348` 为每 slot 注入私有 `RALPH_EVENTS_FILE`；`crates/ralph-cli/src/loop_runner/wave/worker.rs:535-540` 只从私有文件读回并删除；`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:2954-2980`、`:3158-3213` 用 raw main backscan 扣除 done；`crates/ralph-core/src/event_loop/mod.rs:9215-9254` 随后按缺失 provenance 回退到 dispatcher 并 drop；`dispatcher.rs:3334-3425`/`:3447-3547` 负责失败协调与 salvage | 高：2026-07-25-003、2026-07-25-005、2026-07-26-003、2026-07-26-004、2026-07-27-001 | 未读 supervisor.db 内部状态，但不影响“私有结果 vs raw backscan vs scope drop”三段机制链闭合 |
| P1 | 5/6 done 缺少 producer `hat/source`，重读主 events 时被当作 dispatcher 事件 drop；失败后空 channel fallback 触发 dispatcher hard-gate retry，增加二次错误路径 | **compound**（上游 producer/transport 交接异常 + runtime scope/retry 放大） | **68** | DEV-002；`crates/ralph-cli/src/loop_runner/hat_channel.rs:79-88` 空 channel fallback、`:113-132` 成功 merge 才补 attribution；`crates/ralph-core/src/event_loop/mod.rs:9235-9341` scope drop；`crates/ralph-cli/src/loop_runner/runner.rs:4722-4742` hard gate；日志 `...424-830946.log:26-38` | 高：2026-07-26-004 provenance/hat-scope closure、2026-07-25-003 channel/store mismatch、2026-07-27-001 terminal fan-in | R1：对比 6 条 done envelope；R2：源码闭合 drop/fallback/gate；无法定位 producer 丢失的具体阶段，保留 compound 分类

> **未将以下内容列为 §5 根因**：
> - `review.wave.failed → finalizer → LOOP_COMPLETE{blocked}` 是 preset 明确声明的失败路径，不是本次 preset 拓扑缺陷；
> - `synthesized-review.md` / `fix-plan.md` 缺失是因为 success/synth path 未触发，按 preset artifact contract 属于条件未满足；
> - 缺 `recovery.jsonl`、orchestration、agent-output 不单独构成 P0；
> - 未把 worker agent 本身定性为根因，因为 LOGS_ONLY 没有 agent-output 或 hat-channel 原文。

### 5.1 关键源码引用清单

| 主题 | 当前实现证据 | 诊断含义 |
|---|---|---|
| worker 私有事件与 main backscan | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:1284-1348`；`crates/ralph-cli/src/loop_runner/wave/worker.rs:535-540`；`dispatcher.rs:3158-3213` | fan-in 先以私有 slot 文件决定 `CompletedWave`，failed payload 又以未经 scope 过滤的 raw main 行计算 done hints，形成真值源分裂 |
| worker outcome 分类 | `crates/ralph-core/src/supervisor/worker_outcome.rs:340-353`；`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:4567-4595` | accepted event/terminal 缺失会进入 failure classification；本次不把它单独解释为 agent 根因 |
| wave fan-in | `dispatcher.rs:2150-2695` | bridge tick 决定 `InjectedComplete` / `InjectedFailed` / `ContinueCollect`；本次日志为 `InjectedFailed` |
| failed missing 计算 | `dispatcher.rs:2954-3000, 3071-3308` | raw main backscan 只检查 topic/wave_id/payload dimension，不检查后续 scope-drop |
| Review failed salvage | `dispatcher.rs:3427-3547` | 失败时只 salvage `CompletedWave.results` 中未列入 failures 的 `review.unit.done` |
| coordination attribution | `dispatcher.rs:3334-3425` | runtime producer=`ralph`、failed review consumer=`finalizer`；该失败路由符合 preset |
| channel fallback | `crates/ralph-cli/src/loop_runner/hat_channel.rs:79-88, 113-132` | 空 channel 记录 fallback；成功 merge 才统一补 attribution |
| isolated scope drop | `crates/ralph-core/src/event_loop/mod.rs:9235-9341` | 缺 provenance 时回退当前 isolated hat，未声明 topic 被 drop |
| hard gate | `crates/ralph-cli/src/loop_runner/runner.rs:4722-4742` | publish obligation 无合法事件时增加 hard-gate counter |
| wave runtime consumer | `crates/ralph-core/src/event_origin.rs:164-213` | `review.unit.done` 由虚拟 wave runtime consumer 收敛，不要求普通 agent hat 订阅 |

---

## 6. 修复建议

> 以下建议只针对 §5 已入表项；不对 §7 疑点提出确定性修复。

### 6.1 短期（operator workaround）

1. **暂将本 run 的 review 结论视为 blocked，而不是“代码无问题”**：`wave-blocked.md` 只证明本轮聚合没有形成，不代表 22 个 dimension findings 已被综合或可忽略。
2. **下一次复现前保留完整诊断产物**：启用项目既有的 runtime diagnosis artifact 机制，使 worker output、orchestration、recovery 与 wave slot ledger 可同轮对账；不要手工编辑 `.ralph/` 状态文件，也不要从本次 events 直接手写补发终态。
3. **重跑前先清理/隔离旧 run 的 workspace 状态**：使用 Ralph 自己的 loop/worktree 复用与清理命令，确保 `current-events`、hat-channel marker、supervisor wave identity 属于同一新 run；不采用未在当前 CLI 中确认的 `replay-fanin` 或手工 `inject --wave` 命令。

### 6.2 中期（preset / schema / instructions）

1. **增加真实 runtime/BDD 对账场景**：覆盖“主 events 已出现全部 `review.unit.done`，但 slot result 分类为 failed/empty”的交叉源场景，断言最终 `missing_dimensions`、salvage 顺序和 terminal payload；不能只断言某段 YAML 文案。
2. **在 preset instructions 中明确 producer provenance 的来源**：worker 应从当前 wave trigger/runtime context 获取 wave identity；dispatcher/worker/finalizer 的 emit/wave skill 必须显式按需加载，并遵守 `--policy-check` 先行规则。此项是说明性加固，不能替代 runtime 对 provenance 的强制校验。
3. **不要把 envelope 元数据简单塞进 `review.unit.done.required_fields`**：`hat/source/wave_id` 属于事件 envelope/运行时身份面，需先确认 schema/runtime 的字段层次；本次证据不足以支持仅改 preset schema 的结论。

### 6.3 长期（机制 / 底座）

1. **建立单一、可审计的 wave completion evidence**：同一 slot 的 worker accepted events、producer attribution、terminal evidence、supervisor slot status 和 main-ledger merge 必须由一个 idempotent fan-in 状态机收敛；如果出现两本账冲突，先输出结构化 reconciliation failure，不应把已有主事件静默表现为 `missing_dimensions`。
2. **修复 failure fan-in 的交接顺序**：确认 `CompletedWave.results`、`main_backscan`、`store_completed` 三类 evidence 的并集与优先级，确保失败 payload 的 `missing_dimensions` 只表示确实没有 terminal evidence 的维度；同时保持 failed slot 不被伪造为成功。
3. **保留真实 producer provenance，禁止 retry activation 改写 owner**：hat-channel merge、main replay、runtime coordination 应区分 producer 与 consumer；dispatcher 的二次 hard-gate 不应把 `review.unit.done` 作为自身 topic 重发。
4. **为现有 runtime tests 增加本 run 形状的回归**：重点对照 `crates/ralph-core/tests/scenarios.rs:1900-1920` 的 success/failure runtime fan-in 场景，并补充六个 done 已落盘但 slot ledger 为空/failed 的 negative/characterization 场景。

---

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| Q1：5 条 done 的 `hat/source` 是 worker 未写、hat-channel merge 未执行，还是 retry/fallback 重写造成 | 55 | 无 worker agent-output、无 hat-channel body、无 supervisor.db 内部 evidence | 已对照 events 分布、`hat_channel.rs:113-132`、fallback artifact；两种来源在 LOGS_ONLY 下不可分 |
| Q2：`results=0 failures=6` 与 `InjectedFailed` 是否同一 fan-in tick，或经历 transient/二次 tick | 50 | 仅 INFO 日志，缺 supervisor 内部 tick/slot state 时间线 | 已对照日志 `:24-25`、slot JSON `generated_at_kind=injected_failed`；未读取 SQLite |
| Q3：handoff 的 `wave w-2` 与 events 的 `w-rs-1` 是合法内部命名空间映射还是 handoff stale | 45 | 缺 handoff 生成路径和内部 key 映射的完整证据 | 已对照 tasks/handoff/events；不作为根因 |
| Q4：summary `Completed successfully` 与业务终态 `result=blocked` 是否为 landing 层状态视角分裂 | 45 | 缺 landing/summary writer 的完整源码对账 | 已对照 summary、events、logs `:44-50`；不作为根因 |
| Q5：scope-preparer already-closed activation warning 是否代表重复激活或正常 close race | 30 | 缺完整 activation lifecycle/orchestration | 已对照日志 `:10` 与 scope.ready；不驱动修复 |

---

## 8. 盲区与审计边界声明

- 本报告只读取 `.ralph/current-events` 指向的单一主 events 文件，没有使用 `events*.jsonl` 通配来重建链路。
- 历史扫描已获得用户明确选择，范围固定为 `preset-only (30d sliding)`；没有把历史文档当作本次 run 的事实账本。
- 本报告没有读取或改写 `.ralph/supervisor.db`、没有手工修改 `.ralph/` 状态文件；历史扫描只用于复发判断，以当前 SSOT 护栏和源码为本次机制事实准绳。
- `LOGS_ONLY` 下没有完整 agent/OPAC 记录；所有涉及 agent 行为、policy-check 是否执行、具体 channel 读写路径的结论均已降级或放入 §7。
- 主仓已有未提交改动，诊断过程未触碰这些文件；本次只新增本报告文件。

---

## 9. 提交前检查清单

- [x] Phase 0 产物盘点表与 `execution_capabilities` 已写入。
- [x] 只读 `current-events` 指向的唯一 events 文件。
- [x] `LOGS_ONLY` 未因缺 orchestration/agent-output 标 P0。
- [x] §5 P0 confidence≥70，所有入表项 confidence≥60。
- [x] 低置信度候选进入 §7，未写入 §5/§6 的确定性根因。
- [x] 已区分当前终态 `review.wave.failed → finalizer` 与已删除/过时机制。
- [x] 报告 frontmatter 含 `history_search: preset-only` 与 `execution_capabilities: [wave]`。
- [x] 报告路径在主仓 `docs/report/`。
