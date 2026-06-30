# Ralph Orchestrator 综合诊断报告：ce-executor-serial 运行链路与新机制失效

**诊断对象**：`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/` 运行产物 + `ralph-orchestrator` 主仓库源码（`crates/ralph-core/src/event_loop/`、`crates/ralph-cli/src/policy_check.rs`、`presets/en/ce-executor-serial.yml`）  
**数据来源**：`temp2.md`（运行链路表象诊断）+ `temp.md`（9 个新机制根因诊断）  
**诊断时间**：2026-06-28  
**报告性质**：两份诊断的合并与源码确认版。运行表象与底层机制失效互为因果，必须合并修复。

---

## 1. 结论摘要

本次 `ce-executor-serial` preset 运行（`primary-20260628-070436`，pid 88421）**严重失败**：

- `fix-unit` 阶段发生编排机制冲突 + 协调器死锁 + recovery 升级未终止；
- `U4/U5 fix-unit` 从未执行；
- `LOOP_COMPLETE` 未触发；
- 9 个新机制（U3 / U6 / U7 / U8 / U9 / U9.5 / U11 / U12 / U13）**全部未生效**。

**一句话归因**：`ce-executor-serial` preset 的 `fix-unit` 流程设计选择，叠加 Ralph 基座三处未同步更新——

1. `plan_gate` 未对 `fix-unit` 豁免；
2. `allowed_topics` 未与 hat `publishes` 对齐；
3. `recovery` 升级路径未真正终止 loop；

以及 **CLI emit 路径绕开 `stage_pipeline`** 导致 5 个新机制（U6/U7/U9/U9.5/U12）失效，
**preset 配置缺失 `total_units`** 导致 U12 fail-open，
**`state_idempotency` 未接入热路径** 导致 U8/U11 失效。

**问题分级**：P0 × 8 条（基座机制）+ P1 × 6 条（preset/产物/观测）+ P2 × 2 条（漂移/口径）  
**历史关联**：D1 是 2026-06-24 同项目 P0-D 同主题复发；D2/D3 是同链路派生问题；D4 是 2026-06-24 早期已发现的 recovery 打转。  
**修复工作量**：1.5–2.5 天，建议合并为 1–2 个大 PR。

---

## 2. 运行链路：预设预期 vs 实际

| 阶段 | 预期 | 实际 | 状态 |
|---|---|---|---|
| Phase A — plan 单元执行 | 4 步 → `review.start` | step-01/04 正常；step-02 1 次 `task_id=""` 拒后重试成功；step-03 期间 5 次 `work.done`（2 次被 policy 拒） | ⚠️ |
| Phase B — 6 维度 review walk | 6 dimension done → `review.complete(fail)` | 6 维度全部正常完成，`review.complete` verdict=fail, findings=10 | ✅ |
| Phase C — fix-unit dispatch | fix-01..05 全部 → `plan.complete` | fix-01 ✅ → fix-02 ✅ → fix-03 ✅ → fix-04 仅 `work.ready` 重发 2 次后无响应 → fix-05 从未 dispatch | ⚠️❌ |
| Phase D — Shipping | `plan.complete` → shipper → reporter → `LOOP_COMPLETE` | 2 次 `REVIEW_COMPLETE` + 2 次 `report.done`（违反 `completion_after_terminal`）；`LOOP_COMPLETE` 从未触发 | ❌ |

### 2.1 关键偏离汇总（✅ × 5 / ⚠️ × 10 / ❌ × 2）

**完全符合预期的链路段（5）**：step-01 全链路、step-04 全链路、6 维度 review walk、fix-01 executor+validator 段、fix-03 启动段。

**⚠️ 设计冲突 / 状态不一致（10）**：

| 标记 | 偏离 | 时间 | 事件/文件 |
|---|---|---|---|
| A | coordinator 在 `work.ready` 用 placeholder task_id | 08:05 / 08:42 | events #35, #46 |
| B | fix-unit `test.passed` 后 coordinator 未走 fix-unit 分支（应 `work.ready fix-02`，实际 shipper 提前 `REVIEW_COMPLETE`） | 08:14 → 08:22 | events #37 → #38 |
| C | shipper 在 fix-02..05 未完成时 `REVIEW_COMPLETE` 提前 | 08:22:33 | events #38 |
| D | `ralph.task.resume` 的 `allowed_topics` 不含 `work.ready` | 08:27:31 | events #40 |
| E | coordinator 走 `plan.blocked` 而非 fix-unit `work.ready` | 08:31:41 | events #41 |
| F | `duplicate_terminal`：出现 2× `REVIEW_COMPLETE` / 2× `report.done` | 08:22:33 / 08:34:18 | events #38 + #42 |
| G | `human.guidance` 在 suppress 路径下仍被发出 | 08:39:27 | events #45 |
| H | coordinator 越权发 `work.start` 触发 isolated scope violation | 08:42:22 | recovery.jsonl 第 1 条 |
| I | fix-02 `work.ready` 复用 fix-01 placeholder task_id + task_key/step 不一致 | 08:42:29 | events #46 |
| K | fix-03 executor 30s 内双发 `work.done` | 08:57:14 / 08:57:44 | events #51, #52 |
| L | fix-04 coordinator 4s 内重发 `work.ready` | 09:00:30 / 09:00:34 | events #54, #55 |

**❌ 链路断裂 / 终止（2）**：

| 标记 | 偏离 | 时间 |
|---|---|---|
| M | fix-04 executor/validator 未响应，fix-05 从未 dispatch，loop 终止于 iteration 49 | 09:01:33（ledger seq 49） |
| N | `plan.complete` / `REVIEW_COMPLETE`（第 3 次）/ `LOOP_COMPLETE` 全缺失；`shipping.md` 标 `INCOMPLETE` 与 plan frontmatter=`complete` 不一致 | — |

---

## 3. 底层机制失效：9 个新机制全面未生效

| 机制 | 期望 | 实际 | 根因分类 |
|---|---|---|---|
| U3 legacy task 回填 | coordinator 派发 fix-unit 时使用真实 task_id | 两次 `work.ready` 都用 `task-fix-01-placeholder`，触发 `TaskWrongLoop` | R-A：占位符不是 legacy 任务；U3 是事后补救，非事前阻止 |
| U6 硬契约 emit gate | `missing_required_fields` 拦截缺字段事件 | recovery.jsonl 中 0 条 `missing_required_fields` | R-A：CLI 路径绕开 stage_pipeline；本 run 字段恰好齐全 |
| U7 独立 repair stream | `task.relocate*` / `repair.close` 走独立 sink | 0 条 repair topic / 0 条 `repair_dispatch` envelope | R-A：CLI 路径不走 stage；事件源缺失 |
| U8 幂等状态写入 | task/recovery/drift 写入带 `_idempotency_key` / `_final` | tasks.jsonl 无任何任务带这些字段 | R-B：未接入热路径 |
| U9 flow step scope | 拦截未声明 emit、缺 reason 的 `plan.blocked` | 0 命中 `flow_unknown_emit` / `flow_partial_state_undeclared` | R-A：CLI 路径不通；当前 step 都合规 |
| U9.5 verdict gate | 仅 `LOOP_COMPLETE` 能终止 loop | `LOOP_COMPLETE` 0 次 | R-A + reporter fail 不发；U9.5 本身被动 |
| U11 worktree archive | 启动写 `loop-version.json` | `.ralph/loop-version.json` 不存在 | R-C：首次跑 no-op；依赖 U8 |
| U12 step-close obligation | partial state 下拦截不在 `on_partial` 分支的 emit | 0 命中 `step_close_obligation_violated` | R-B：`total_units` 未声明 |
| U13 archive fail-closed | archive 失败时 loop 启动 abort | 无法验证（archive 永远不失败） | R-C：依赖 U11 |

### 3.1 三类根因

#### R-A：CLI emit 路径绕开 stage_pipeline（最致命，影响 U6/U7/U9/U9.5/U12）

**证据**：`crates/ralph-cli/src/policy_check.rs:609-737` 的 `run_policy_check_unified` 是 CLI `ralph emit` 入口，它调用 `ValidationPipeline::from_config(&view, &event_loop_config)`（旧 d623c09 那套），**完全不调用 `evaluate_emit_gate`**，也不写 `record_repair_event` 路径的 envelope。

`recovery.jsonl` 第 1 条 envelope 结构为旧 schema：`source: "cli_emit"`、`reason_code: "semantic_gate_violation"`，正是 `policy_check.rs:452` 输出的旧路径 envelope 形状，而非 `record_repair_event` 写的 `repair_dispatch` envelope。

**根因**：`event_loop/mod.rs:6422-6437` 注释明确说明 `unified_pipeline = build_unified_validation_pipeline(...)` 仅在 `UNIFIED_VALIDATION=1` 环境变量下激活，默认走 legacy gate stack。

#### R-B：preset 配置缺失 / 代码不读 flag（U8 / U12）

**U8**：`state_idempotency: required` 已在 preset 设置，但 `grep state_idempotency crates/ralph-core/src/event_loop/mod.rs` 仅 4 处注释，无业务分支。`idempotent_wiring::write_task` 唯一调用在 `task_store.rs:174` 的批量回填路径，非热路径。

**U12**：`grep "total_units" presets/en/ce-executor-serial.yml` 0 命中。`FlowStepDecl::total_units` 是 `Option<u32>`，不填即 `None` → `flow_step_totals` map 空 → `drive_step_close_progress` 早 `return` → `StepCloseObligationStage::update_progress` 永不调用 → stage fail-open。

#### R-C：U11/U13 设计性 no-op

`archive_version_stage.rs:67-79`：首次跑（无 `loop-version.json`）直接返回 `Ok(None)`；同 `loop_id` 复用也不 archive。注释说明依赖 `IdempotentLog::open` 写新版本，但 U8 未生效 → `loop-version.json` 永不写。

---

## 4. 统一问题归因表（P0 / P1 / P2）

| 优先级 | 问题 | 根因 | 关键源码 / 事件 | 修复责任 |
|---|---|---|---|---|
| **P0 D1** | fix-unit 流程与 `plan_gate_review_not_terminal` 设计冲突，导致 `plan.complete` 被拒、协调器死锁 | 基座 + preset 设计 | `review_step_state.rs:254-281`（`plan_gate_step_gate` 不按 step 前缀豁免）；events L40 `task.resume kind=plan_gate_review_not_terminal`；events L41 `plan.blocked reason=fix_unit_design_conflict`；`event_policy.rs:1381-1421`（2026-06-24 P0-D 同主题修复仅补字符串守卫，未动 plan_gate 豁免）；preset L751-803（fix-unit 显式禁止 `review.start`） | preset + loop |
| **P0 D2** | `task-fix-01-placeholder` 污染 task ledger，`work.done` 反复撞 `TaskWrongLoop` | 基座（执行契约过严 + coordinator 用 placeholder） | `execution_contract.rs:180-185`（`TaskWrongLoop` 定义）、`499-525`（loop_scoped 检查）；`task_cli.rs:250-254`（legacy task 不可写）；events L35/L46/L47；`recovery.jsonl:1`；`tasks.jsonl:10/12/15`；`legacy_task_relocate.rs:64-120` 仅 worktree 复用场景回填 | loop（协调器合约） |
| **P0 D3** | coordinator 08:42:22 试图发 `work.start`，触发 isolated scope violation | 基座（recovery 路由没限定 `allowed_topics`） | `recovery.jsonl:1`；events L35；preset L602-603；`event_loop/mod.rs:1205`（`isolated_publish_allowed`）、`6697`（scope 检查）、`6921`（circuit breaker 触发） | loop（recovery routing） |
| **P0 D4** | recovery 升级到 `RECOVERY-FINAL-WARNING` 但 loop 继续运行到 iter 49 | 基座（warning 路径没有真 terminate） | events L45；`scratchpad.md` 08:50:18；`drift/engine.rs:443-470`；`diagnosis/responder.rs:512-528`；ledger L5,9,12,43 | loop |
| **P0 R-A** | CLI emit 路径绕开 stage_pipeline，导致 U6/U7/U9/U9.5/U12 无法拦截 | 架构分裂 | `policy_check.rs:609-737`；`wave.rs:296-336`；`event_loop/mod.rs:6422-6437` | loop + CLI |
| **P0 R-B1** | `state_idempotency: required` 未接入热路径，U8 完全失效 | 接入缺失 | `idempotent_wiring.rs`；`task_store.rs:174`；`event_loop/mod.rs` 仅 4 处注释 | loop |
| **P0 R-B2** | preset 未声明 `total_units`，U12 fail-open | 配置缺失 | `step_close_obligation_stage.rs`；`event_loop/mod.rs:9118-9150`；preset `total_units` 0 命中 | preset + loop |
| **P1 D5** | executor step-03 期间 5 次 `work.done`，ledger 2 次 `duplicate_work_done` 拒，但 `work.done` 实际被接受 | 基座（dedup 依赖 task_id；D2 让 task_id 无效） | events L10-15；ledger L10-11；events L51-52 | loop |
| **P1 D6** | `shipping.md` 与实际不符：标 step-01~04 + fix-01 完成，但 `progress.md` 已写 fix-02/03；frontmatter `status: complete` 手动更新 | agent 产物 | `shipping.md:5-15`；`progress.md:11-13`；`memories.md:23-27` | agent skill 数据 |
| **P1 D7** | progress-steward 未在合适时机介入（4 次 `no_progress`），recovery 反复打转未升级到 `loop.stalled` | 基座 + preset 拓扑 | ledger L5,9,12,43；`event_loop/tests/progress_steward.rs:85-88`；`state/snapshot.rs:95-100` + `loop_state.rs:1147-1190` | preset |
| **P1 D8** | scratchpad/memory 的 HUMAN GUIDANCE injection 把执行合同拒绝信息混在 prompt 里（4 次注入） | 基座设计选择 | `scratchpad.md:28-32`；`event_loop/mod.rs:2820-2900` | loop |
| **P1 R-C** | U11/U13 设计性 no-op，首次跑不写 `loop-version.json`，无法验证 fail-closed | 设计依赖 U8 | `archive_version_stage.rs:61-79`；`event_loop/mod.rs:592-630` | loop |
| **P2 D9** | `drift.jsonl` 5 条字段缺失告警（实际字段都存在） | drift_monitor schema 解析问题 | `diagnostics/2026-06-28T15-04-35/drift.jsonl:1-5`；对比 events L40, 41, 45 | drift monitor |
| **P2 D10** | `plan.blocked` 报 verdict: "fail" 但 `shipping.md` 写 `INCOMPLETE` — shipper/reporter/shipping 三套口径不一致 | agent 产物 | events L38, 42；`shipping.md:5`；preset L2270-2290 | preset / skill 数据 |

---

## 5. 修复建议（统一优先级与顺序）

### 🔴 P0 必做（否则 preset 永远跑不通 fix-unit，5 个机制永远不生效）

| 编号 | 修复 | 目标文件 | 改法 | 预期效果 | 工作量 |
|---|---|---|---|---|---|
| **R1** | plan_gate 豁免 fix-unit | `crates/ralph-core/src/event_loop/review_step_state.rs:254-281` | 在 `plan.complete` 检查前增加 `if step.starts_with("fix-")` 早返回 `None`；为 `synth_terminal` 初始化增加 `observe_accepted` 路径——当 coordinator 的 `review.complete` payload 的 `fix_plan_file` 非空时，给所有 `fix-{NN}` step key 预填 `synth_terminal = "review.complete"` | fix-01 `test.passed` 后直接发 `plan.complete` 不再被拒 | 小 |
| **R2** | 协调器用真实 task_id 替代 placeholder | `presets/en/ce-executor-serial.yml:789-803` + `crates/ralph-cli/src/task_cli.rs` | 协调器在 `review.complete` 解析 fix-plan 后，先用 `ralph tools task create --for-fix-unit` 创建每个 U-ID 的真实 task 并取得 task_id，再填 `work.ready`。禁止 `task-{step}-placeholder` | fix-01/02/03 的 `work.done` 不再撞 `TaskWrongLoop` | 中 |
| **R3** | recovery 路由 `allowed_topics` 与 hat publishes 对齐 | `crates/ralph-core/src/diagnosis/responder.rs:512-528` + `event_loop/mod.rs:2820-2900` | `publish_hard_recovery_event` 注入 `task.resume` 前，取 `registry.get(hat).publishes` 过滤 `allowed_topics` | 协调器不会再试 `work.start`，circuit breaker 不 trip | 小 |
| **R4** | Final 路径提早触发 | `event_loop/mod.rs` stall detector + `diagnosis/responder.rs` | 同一 `retry_key` 累计 3 次 escalated 后强制走 `EscalationLevel::Final` | iteration 30 后真正终止 loop | 中 |
| **M1** | CLI emit 路径走 stage_pipeline | `crates/ralph-cli/src/policy_check.rs:609-737` + `wave.rs:296-336` | `run_policy_check_unified` 增加 `evaluate_emit_gate` 调用 + envelope 适配（CLI 写 `repair_dispatch` 兼容 envelope） | U6/U7/U9/U9.5/U12 在 CLI 路径生效 | 中 |
| **M2** | U8 接入热路径 | `crates/ralph-core/src/task_store.rs` | `TaskStore::save` / `create` / `update` 在 `state_idempotency == "required"` 时路由到 `idempotent_wiring::write_task` | task 写入带 `_idempotency_key` / `_final` | 中 |
| **M3** | U12 注入 total_units 或运行时计算 | `presets/en/ce-executor-serial.yml` + `event_loop/mod.rs:9125` | 方案 B（更稳）：`flow_step_total_units` 加 fallback，从 `tasks.jsonl` 读 fix-unit 计数作为 `total_units` | U12 不再 fail-open | 小/中 |
| **M4** | U11 写初始 loop-version.json | `crates/ralph-core/src/event_loop/mod.rs:592-630` | `EventLoop::new` archive 调用后显式写 `loop-version.json` | U11 生效，U13 可验证 | 小 |

### 🟡 P1 应做（产物质量 / 观测）

| 编号 | 修复 | 目标文件 | 改法 | 工作量 |
|---|---|---|---|---|
| **R5** | shipping.md / shipper 口径对齐 | `presets/en/ce-executor-serial.yml:2270-2290` + `ralph-tools-tasks.md` | hard-fail 时 `shipping.md` Status 填 `"FAIL"` 而非 `"INCOMPLETE"` | 小 |
| **R6** | progress-steward hat 显式声明 | `presets/en/ce-executor-serial.yml` hats 段 | 增加 `progress-steward` + `progress_steward.enabled: true` + `steward_hat_id: "progress-steward"` | 小 |
| **R7** | drift_monitor 字段识别修复 | `crates/ralph-core/src/drift/monitor.rs` | 修正 `task.resume.kind` / `reason` / `target_hat` 等字段解析 | 小 |
| **M5** | U7 自动 emit repair topic | `event_loop/mod.rs:9601/9766` 的 `record_repair_event` 之前 | `TaskWrongLoop` envelope 写出后自动 emit `task.relocate_legacy` | 中 |
| **M6** | U13 fail-closed 集成测试 | `stages/archive_version_stage.rs/tests.rs` | M4 修好后注入 archive 失败，断言 `EventLoop::new` 返回 Err | 小 |

### 🟢 P2 可选

| 编号 | 修复 | 目标文件 | 改法 |
|---|---|---|---|
| **R8** | 文档一致性 | `crates/ralph-core/data/ralph-tools-cmdref.md` + `docs/solutions/developer-experience/` | 更新 `task.resume` 字段说明；新增 `ralph-fix-unit-plan-gate-conflict.md` 记录本次复发链路 |
| **M7** | 协调器禁止 placeholder（预防性） | `presets/en/ce-executor-serial.yml:789-803` | instructions 增加 HARD RULE：必须先 `ralph tools task create --for-fix-unit` |
| **M8** | IdempotentLog 独立 open | `crates/ralph-core/src/state/idempotent_log.rs` | 让 IdempotentLog 在没有 task 写入时也能 open（为 U13 完整闭环） |

---

## 6. 执行顺序建议

```
第一阶段（基座最小闭环）：
  R1 → R3 → R4（loop 基座 3 件，同步进行）
  M4（解锁 U11/U13 验证）

第二阶段（CLI 路径 + 配置接入）：
  M1（核心，解锁 5 个机制）
  M3（快速胜利，解锁 U12）
  M2 + M5（联动，完整 U7 + U8）

第三阶段（preset + 收尾）：
  R2（preset + CLI 协同）
  R6（preset 拓扑）
  R5 + R7 + R8 + M6 + M7 + M8（产物/观测/测试）
```

**总工作量**：6 项小 + 5 项中 = **1.5–2.5 天**。

---

## 7. 关键提醒

1. **D1 复发警告**：2026-06-24 同项目已发 P0-D 修复（`event_policy.rs:1381-1421`），但只补了 `review.complete.fix_plan_file` 字符串守卫，`plan_gate` 的 fix-unit 豁免完全没动。若 R1 仅做字段守卫而不同步 `plan_gate`，下次还会复发。

2. **D2/D3 是 D1 的连锁反应**：`plan.complete` 被拒 → coordinator 走 `plan.blocked` → `task.resume` → 协调器试 `work.start`（不在 publishes）→ circuit breaker。必须 **R1 + R2 + R3 一起修**，单修任何一条都不够。

3. **D4 是隐性 killer**：recovery 升级到 `RECOVERY-FINAL-WARNING` 但 loop 不终止，会让 pid 88421 长时间占 CPU。R4 必须做。

4. **CLI 路径分裂是核心症结**：`policy_check::run_policy_check_unified` 和 `evaluate_emit_gate` 是两套并行机制，任何只修一套的方案都会留下“幽灵路径”。M1 必须把两套合并。

5. **U8 是“鸡生蛋”问题**：M4 写初始 `loop-version.json` 可解 U11，但 U8 完全没接热路径——不修 M2，即使 IdempotentLog 启用，任务写入也不会带 `_idempotency_key`。

6. **Agent C 重要修正**：Agent D 原文写“U2-U5 从未 dispatch”，实测 U2/U3 实际成功流转，U4 重发待响应，U5 完全未发。最终失败链路是 **U4 重发后 executor/validator 无响应 + U5 未派发**，而非 U2-U5 全军覆没。这影响 R1/R3 的回归测试范围。

---

## 8. 验证方法

- 修复后跑同 plan 的 replay：`ralph doctor plan-replay --plan docs/plans/2026-06-20-001-...`
- 检查 `recovery.jsonl` 是否出现对应 envelope，`events.jsonl` 是否走 `stage_pipeline`。
- BDD 反向验证必须 **使用 `run_workflow_guard_scenario`（真 EventLoop runner，断言 events）**，禁止用 `run_scenario` stub（stub 只查 iterations 数，不断言事件）。
- 全量回归：`./scripts/run-tests.sh`（含 `preset_lint` + WAC + scenarios + SSOT byte-equality）。

---

## 9. 复核确认（基于实际 `.ralph` 中间产物 + 源码）

本次复核直接读取了 `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/` 的运行产物，并与主仓库源码逐条对账。原报告结论全部成立，部分源码行号已按实际文件校正。

### 9.1 产物读取范围

| 文件 | 行数 / 条目数 | 复核用途 |
|---|---|---|
| `events-20260628-070436.jsonl` | 55 条事件 | 还原 Phase A-D 全链路 |
| `ledger.jsonl` | 49 条 sequence | 确认 iteration 分布、4 次 `no_progress`、2 次 `duplicate_work_done` |
| `recovery.jsonl` | 1 条 envelope | 确认 coordinator 越权发 `work.start` 被 isolated scope 拒绝 |
| `agent/tasks.jsonl` | 15 条 task | 确认 `task-fix-01-placeholder` 跨 step 复用、U2/U3 真实任务已关闭 |
| `agent/scratchpad.md` | 134 行 | 确认 4 次 HUMAN GUIDANCE 注入、RECOVERY-FINAL-WARNING 内容 |
| `agent/shipping.md` | 31 行 | 确认 `Status: INCOMPLETE` 与 `plan.blocked` verdict=fail 口径不一致 |
| `agent/progress.md` | 13 行 | 确认 fix-01~fix-03 已进入 Completed Steps |
| `diagnostics/2026-06-28T15-04-35/drift.jsonl` | 5 条 | 确认 drift 字段缺失告警与实际 events 字段存在矛盾 |

### 9.2 关键事件复核（与源码交叉验证）

| 事件 | 产物证据 | 源码确认 |
|---|---|---|
| fix-01 用 placeholder task_id | events L35 `task_id="task-fix-01-placeholder"` | `legacy_task_relocate.rs` 仅回填已存在 tasks.jsonl 中的空 loop_id，不阻止 coordinator 继续发 placeholder |
| fix-02 复用 placeholder | events L46 `task_id="task-fix-01-placeholder"`, step="fix-02" | `task_cli.rs:250-254`：legacy task（无 loop_id）在 agent context 不可写，导致 placeholder 任务无法关闭 |
| executor work.done 撞 `TaskWrongLoop` | events L47 `task_id="task-fix-01-placeholder"`；scratchpad 08:47:20 | `execution_contract.rs:499-525`：`loop_scoped=true` 时 task.loop_id 为空即触发 `TaskWrongLoop { actual_loop: None }` |
| coordinator 越权发 `work.start` | recovery.jsonl L1 `reason_code="semantic_gate_violation"`，`allowed publishes=[work.ready, review.start, plan.complete, plan.blocked]` | preset L602-603 coordinator publishes 仅含这 4 项；`event_loop/mod.rs:6697` scope 检查拒绝 |
| `plan.complete` 被 plan_gate 拒 | events L40 `task.resume kind=plan_gate_review_not_terminal` | `review_step_state.rs:254-281`：`plan.complete` 要求所有匹配 step 的 `synth_terminal` 为 `review.passed`/`review.complete`，fix-unit 无 review-terminal |
| recovery 升级未终止 | scratchpad 08:50:18 `RECOVERY-FINAL-WARNING` iteration=44 | `diagnosis/responder.rs:512-528` Final 仅写 hint；`drift/engine.rs:443-470` Warning 仅发 `human.guidance`，不终止 loop |
| U12 fail-open | ledger L10-11 同一 iteration 内 2 次 `duplicate_work_done` 未触发 step_close | `event_loop/mod.rs:9118-9144`：`flow_step_total_units` 返回 `None` 时早 `return`，`StepCloseObligationStage::update_progress` 永不调用 |
| U8 未生效 | tasks.jsonl 15 条记录均无 `_idempotency_key` / `_final` | `idempotent_wiring.rs` 实现完整，但 `task_store.rs:174` 唯一调用在批量回填路径，热路径未接入 |
| U11 未生效 | `.ralph/loop-version.json` 不存在 | `archive_version_stage.rs:62-65`：无 `loop-version.json` 时直接返回 `Ok(None)` |
| CLI 路径绕开 stage_pipeline | recovery.jsonl L1 `source="cli_emit"`、旧 envelope schema | `policy_check.rs:609-737` `run_policy_check_unified` 使用 `ValidationPipeline`，未调用 `evaluate_emit_gate` |

### 9.3 与初版报告的微小校正

- **execution_contract.rs 行号**：初版写 `194-201, 517-526`，实际 `TaskWrongLoop` 定义在 `180-185`，检查逻辑在 `499-525`。
- **task_cli.rs 行号**：初版写 `249-254`，实际 legacy task 不可写分支在 `250-254`。
- **event_loop/mod.rs 行号**：初版写 `6697-6932` 描述 isolated scope 全链路，实际关键函数为 `1205`（`isolated_publish_allowed`）、`6697`（scope 检查）、`6921`（circuit breaker）。
- **fix-unit 实际流转**：U2/U3 真实 task 已成功创建并关闭（`tasks.jsonl:13` task-1782636501-e45d 状态 `closed`），U4 仅 `work.ready` 重发 2 次后无响应，U5 完全未发——与初版报告一致。

---

## 10. 诊断元信息

- **数据范围**：1 个 loop run（`primary-20260628-070436`，pid 88421，55 events / 49 ledger / 15 tasks / 1 recovery / 5 drift / 49 iterations）+ 主仓库 `crates/ralph-core/src/` 全树 + `crates/ralph-cli/src/policy_check.rs` + `presets/en/ce-executor-serial.yml`
- **关键源码文件**：`review_step_state.rs` / `legacy_task_relocate.rs` / `execution_contract.rs` / `task_cli.rs` / `diagnosis/responder.rs` / `event_loop/mod.rs` / `policy_check.rs` / `stage_pipeline.rs` / `emit_gate.rs` / `emit_schema_gate_stage.rs` / `repair_dispatch_stage.rs` / `repair_stream_sink.rs` / `step_close_obligation_stage.rs` / `flow_step_scope_stage.rs` / `verdict_gate_stage.rs` / `archive_version_stage.rs` / `idempotent_wiring.rs`
- **输出文件**：`docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md`

---

**最终归因一句话**：本次失败是 `ce-executor-serial` preset 的 `fix-unit` 流程设计选择，叠加 Ralph 基座 `plan_gate` / `allowed_topics` / `recovery 终止` 三处未同步更新，再叠加 **CLI emit 路径绕开 stage_pipeline** 与 **preset 配置缺失 `total_units`** 导致的复合型失效。修复 1.5–2.5 天可闭环，建议合并为 1–2 个大 PR。
