---
title: implementation-review Loop `primary-20260806-090515` 运行链路诊断报告
date: 2026-08-06
type: diagnosis
loop_id: primary-20260806-090515
preset: builtin:implementation-review
run_dir: .
status: 六个 reviewer 均成功，但默认 wave 与 fan-in 使用不同聚合超时，最终被误判 timeout
initial_terminal_status: blocked
recovery_status: no_recovery
diagnostics_mode: MINIMAL
history_search: disabled
execution_capabilities:
  - wave
---

# implementation-review Loop `primary-20260806-090515` 运行链路诊断报告

> **生成时间**：2026-08-06  
> **诊断对象**：主仓 `.ralph/`，loop `primary-20260806-090515`  
> **对照 preset**：`presets/en/implementation-review.yml` + `presets/schemas/implementation-review.yml`  
> **Diagnostics 模式**：`MINIMAL`；无 `orchestration.jsonl` / `agent-output.jsonl`，OPAC 审计上限 70  
> **history_search**：`disabled`；未扫描本次 run 外的历史文档  
> **execution_capabilities**：`["wave"]`；preset 明确使用 `ralph wave emit`、events 有 `wave_id=w-rs-1`，但 `supervisor.enabled=false`  
> **Tier C 根**：`.ralph/review/2026-08-05-001-refactor-large-file-module-split-plan/`

---

## 0. 产物盘点

| Tier | 路径 | 存在 | 行数/数量 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` → `.ralph/events-20260806-090515.jsonl` | 是 | 22 行 | 本报告唯一 events SSOT |
| S | 配对 `.ralph/events-history-20260806-090515.jsonl` | 是 | 2 行 | 非编排 SSOT |
| S | `.ralph/ledger.jsonl` | 是 | 5 行 | completion requested/honored 均存在 |
| S | `.ralph/recovery.jsonl` | 否 | 0 | 本次无 workspace 级拒收 |
| S | `.ralph/diagnostics/2026-08-06T17-05-15/recovery.jsonl` | 是 | 1 行 | 仅 `agent_doc_sync` 信息项 |
| A | `.ralph/agent/tasks.jsonl` | 是 | 6 行 | 六个 slot task 均 closed；内部 store wave id 为 `w-2` |
| A | `.ralph/agent/summary.md` | 是 | 29 行 | 摘要错误地写“Completed successfully”，见 P1-2 |
| A | `.ralph/agent/handoff.md` | 是 | 52 行 | 同样错误地写 session completed successfully |
| B | diagnostics session | 是 | MINIMAL | 有 trace/recovery/drift/summary，无 orchestration/agent-output |
| B | `.ralph/supervisor.db` | 是 | 1 DB | default-wave 的 ledger 证据；不把它推断为 `+supervisor` |
| C | `scope-manifest.json` / `review.diff.patch` / `review-context.md` / `scope-analysis.md` | 是 | 4 件 | scope 阶段完整 |
| C | `dimensions/*.md` | 是 | 6 件 | 六个维度报告均落盘 |
| C | `wave-blocked.md` | 是 | 15 行 | `reason: wave_failed:timeout`，`missing_dimensions: []` |
| C | `synthesized-review.md` / `fix-plan.md` | 否 | 0 | 因误入 failed 分支，阶段未触发 |

**盲区声明**：MINIMAL 模式无法逐条还原各 hat 的 tool call、`--policy-check` 与 Confirm；因此不把“未看见 precheck”当作违规。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**：编排终态正确 fail-close，但 wave fan-in 发生确定性假失败。
- **问题数**：P0 1 条，P1 1 条，P2 1 条。
- **最高优先级根因置信度**：P0-1 = **95/100**。
- **历史复发**：`N/A (history disabled)`。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 执行与 OPAC 是否合规？ | 编排执行部分失败；OPAC 未见明确违规 | 六个 worker 都产出 artifact 与 `review.unit.done`；MINIMAL 无 tool-call 证据 | 70（OPAC 上限） |
| Q2 | 基座机制是否生效？ | 部分生效 | wave 扇出、并发执行、slot close、runtime-injected failed、finalizer 终态都生效；fan-in deadline 对账失效 | 95 |
| Q3 | 编排是否合理？ | preset 意图合理，超时配置与机制耦合不闭合 | preset 声明 worker 900s、预计 aggregate ~930s，却未配置 supervisor aggregate，而 fan-in读取 600s | 94 |
| Q4 | 归因是什么？ | compound：机制 70% + preset 30%；agent 0% | dispatcher 使用 930s 执行 wave，却向 coordinator 传 supervisor 默认 600s | 95 |

### 1.3 根因一句话

`review-worker` 的六个 slot 在 **726.156s** 内全部成功，但默认 wave dispatcher 按约 **930s** 的波次预算执行，fan-in 却改读 `SupervisorConfig.aggregate_timeout_secs=600`，`evaluate_phase` 又在检查“全部完成”之前先判断 `726 > 600`，因此注入 `review.wave.failed(reason=timeout)`；置信度 **95**。

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| **首轮终态** | `review.wave.failed` → finalizer → `LOOP_COMPLETE(result=blocked)` |
| **恢复状态** | 无恢复；失败后没有 accepted 成功事件 |
| **最终代码状态** | 本诊断不评价代码质量；六份 review artifact 已生成，但未进入 synthesizer/fix-planner |
| **一致性告警** | `.ralph/agent/summary.md` 与 `handoff.md` 把 blocked 终态错误汇总为成功 |

---

## 2. 执行链路对比

| 阶段 | 预期 | 实际 |
|---|---|---|
| scope | `review.start → scope.ready` | 正常 |
| dispatch | 6 条 `review.unit.ready(w-rs-1)` | 正常 |
| worker | 6 个 slot，各 1 条 `review.unit.done` | worker 执行成功；main ledger 出现 12 条 done（每 slot 重复一次） |
| fan-in | 全部完成后注入 `review.wave.complete` | 错误注入 `review.wave.failed(reason=timeout)` |
| synthesize | `review.wave.complete → review-synthesizer` | 未触发 |
| finalizer | 失败时 artifact-first blocked 终止 | 正常执行，写 `wave-blocked.md` 后 `LOOP_COMPLETE` |

关键时间线：

1. 09:13:09：wave `w-rs-1` 启动，6 slots 并发。
2. 09:25:15：dispatcher 记录 `results=6, failures=0, duration_ms=726156`。
3. 同一毫秒：fan-in 返回 `InjectedFailed`。
4. main events 写入 `review.wave.failed`，随后 finalizer 写 `LOOP_COMPLETE`。

---

## 3. 历史问题上下文

`N/A (history disabled)`

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度 | 初始置信度 |
|---|---|---|---|---:|
| DEV-001 | 6 个 worker 全成功且耗时 726.156s | `.ralph/diagnostics/logs/ralph-2026-08-06T17-05-15-607-10178.log:24` | P0 | 95 |
| DEV-002 | fan-in 在 worker 成功后返回 `InjectedFailed` | `.ralph/diagnostics/logs/ralph-2026-08-06T17-05-15-607-10178.log:31` | P0 | 95 |
| DEV-003 | finalizer artifact 明示 timeout 且 `missing_dimensions: []` | `.ralph/review/2026-08-05-001-refactor-large-file-module-split-plan/wave-blocked.md:1` | P0 | 95 |
| DEV-004 | dispatcher 本地 wave deadline 用 worker/batch 公式 | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:1385` | P0 | 95 |
| DEV-005 | fan-in 改读 supervisor aggregate timeout | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:920` | P0 | 95 |
| DEV-006 | supervisor 默认 aggregate timeout 为 600s | `crates/ralph-core/src/config/loop_config.rs:1287` | P0 | 95 |
| DEV-007 | timeout 判断先于全部完成判断 | `crates/ralph-core/src/supervisor/phase.rs:133` | P0 | 95 |
| DEV-008 | preset 声明 worker 900s、预计 aggregate ~930s，但 supervisor 仅配置并发上限 | `presets/en/implementation-review.yml:60`, `presets/en/implementation-review.yml:101`, `presets/en/implementation-review.yml:1278` | P0 | 94 |
| DEV-009 | summary/handoff 把 blocked 终态写成成功 | `.ralph/agent/summary.md:3`, `.ralph/agent/handoff.md:25` | P1 | 90 |
| DEV-010 | main events 含 12 条 done，且日志将 `w-2` rows 当 orphan | `.ralph/events-20260806-090515.jsonl:9`, `.ralph/diagnostics/logs/ralph-2026-08-06T17-05-15-607-10178.log:25` | P2 | 80 |

### 4.1 OPAC 逐 hat 审计

| Hat | Observe | Precheck | Apply | Confirm | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| scope-preparer | ✅ | ⚠️ | ✅ | ✅ | scope artifacts + accepted `scope.ready`；无 tool-call trace | 70 |
| review-dispatcher | ✅ | ⚠️ | ✅ | ✅ | 6 条 accepted ready；无 tool-call trace | 70 |
| review-worker | ✅ | ⚠️ | ✅ | ✅ | 6 artifacts + 6 slot successes；无 tool-call trace | 70 |
| finalizer | ✅ | ⚠️ | ✅ | ✅ | `wave-blocked.md` + accepted `LOOP_COMPLETE` | 70 |

---

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 | 历史关联 | 加深 |
|---|---|---|---:|---|---|---|
| P0 | 全部 slot 成功后仍被 timeout 判失败 | compound：机制 70%（95）+ preset 30%（94） | **95** | DEV-001～008 | `N/A (history disabled)` | events → logs → artifact → preset → 源码 |
| P1 | blocked loop 被 summary/handoff 描述为成功 | mechanism | **90** | DEV-009 + accepted terminal events | `N/A (history disabled)` | 双账本对账 |
| P2 | main ledger 的 done 重复且内部/public wave id 映射产生 orphan 告警 | mechanism（不构成本次失败主因） | **80** | DEV-010；slots 仍全 completed | `N/A (history disabled)` | events + logs + tasks |

**P0 compound 分解**：

- 机制成分（70%，置信度 95）：执行阶段使用约 930s 的有效 aggregate deadline，但 fan-in 使用独立的 600s `SupervisorConfig`；并且 phase evaluator 在 terminal-complete 检查之前执行超时判断。
- preset 成分（30%，置信度 94）：注释明确宣称约 930s，却只设置 `max_concurrent_workers: 6`，未将 `supervisor.aggregate_timeout_secs` 对齐；preset 现有结构测试也只钉 worker timeout 与并发上限。
- agent 成分（0%，置信度 95）：6 个 reviewer 均完成、6 份 artifact 存在、slot snapshot 全为 completed。

---

## 6. 修复建议

### 6.1 短期 operator workaround

- 在 `implementation-review` 的 `event_loop.supervisor` 显式配置 `aggregate_timeout_secs: 930`（或略高于该预算），避免 600s fan-in 误杀；关联置信度 **94**。
- 不要直接复用本次 `LOOP_COMPLETE` 的“成功”摘要；真实结果是 `blocked`，六份维度报告可人工读取；关联置信度 **95**。

### 6.2 中期 preset

- 将 supervisor aggregate timeout 与注释、worker timeout 和实际 batch 公式统一为同一个声明，并增加结构化测试：当 6 slots 在 600～930s 区间全部 completed 时必须产生 `review.wave.complete`；关联置信度 **95**。
- 同步 schema/preset 校验与 builtin preset 测试，避免只钉 `review-worker.timeout` 而遗漏 fan-in budget；关联置信度 **94**。

### 6.3 长期机制

- fan-in 应消费 dispatcher 已解析出的**同一个有效 wave deadline**，不要再次从 `SupervisorConfig` 读取另一套时钟；关联置信度 **95**。
- 明确 terminal precedence：如果全部 required slots 已完成且无失败，完成态是否应优先于“fan-in 调用发生时已超过 deadline”；无论选择哪种语义，都需由单一 deadline SSOT 和回归测试固定；关联置信度 **92**。
- summary/handoff 应从 accepted terminal payload 读取 `result=blocked`，不得仅因 `LOOP_COMPLETE` 存在就写 success；关联置信度 **90**。
- 调查并去重 main ledger 的两组 `review.unit.done`，统一 public wave id `w-rs-1` 与 store wave id `w-2` 的映射，消除 orphan backscan；关联置信度 **80**。

---

## 7. 未核实疑点

无低于 60 分且会驱动修复的疑点。
