# Ralph Loop 运行链路诊断报告

> **Run**: `primary-20260624-092856`
> **Preset**: `builtin:ce-executor-serial`
> **Plan**: `feat-python-sort-algorithms-plan`
> **诊断日期**: 2026-06-24
> **诊断方法**: 4-Agent 并行(流程还原 / 历史上下文 / 对账分析 / 归因修复)

---

## 1. 结论摘要

### 一句话总结

**该 run 在 review 终态环节断裂:`review-synthesizer` 发出 `review.complete(verdict=pass_with_residuals)` 但前置必需的 `review.passed` 被 `d8e1da3d` commit 设计性移除,导致 shipper 误镜像为 fail,manager 决策依据丢失,运行链路无法回到正轨。**

- **关键异常数**:P0 = 3 条 / P1 = 5 条 / P2 = 6 条
- **是否涉及历史重复问题**:**是**,与 `primary-20260624-032505`(8 residuals)同型,30 天内第 6+ 次复发同根因路径
- **是否真的"修复机制失效"**:**不是机制消失,而是机制改了一半**——`b9c0fe9c` commit 改了 preset 的 review 终态语义,但 BDD 场景和 runtime drift detector 没跟上,导致 4 处 SSOT 不一致

### 用户原话 vs 实际诊断对照

用户观察到:
1. "编排流程没按预设走" — **是**,Step 23 起偏离(详见 §2)
2. "修复机制失效" — **不是失效,是设计变更但未全链路同步**(详见 §5)
3. "没有把东西拨回原来的轨道" — **是**,shipper 镜像 fail → reporter → loop.terminate,无恢复路径(详见 §4)

---

## 2. 执行链路对比图

### Run 元数据

| 字段 | 值 |
|------|-----|
| Run ID | `primary-20260624-092856` |
| Preset | `builtin:ce-executor-serial` |
| Plan | `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md` |
| Iterations | 22 实际 / 50 max |
| Duration | 1h 39m 8s |
| Final Verdict | `fail`(`loop.terminate.review_failed`) |
| 终止原因 | `loop-termination-reason.json` → `{"review_failed":{"topic":"report.done"}}` |

### 简明结论

该 run 在 **Step 23**(`review-synthesizer` emit `review.complete`)处偏离预设轨道——synthesizer 没有先发前置的 `review.passed`,直接 emit `review.complete(verdict=pass_with_residuals, fix_plan=null)`,违反 verdict_gate 前置约束。Coordinator 因 scope 受限只能 emit `plan.blocked(review_terminal_drift)`,最终 shipper 镜像 fail → reporter → `loop.terminate`。

### 完整链路对比表(29 步)

> BDD scenario `ce_executor_serial_review.yml` 描述 4-dim serial review;**当前 preset `ce-executor-serial` 的 SSOT** 已迁移到 2-dim serial review(correctness → testing)。下面"预期"列以 **builtin preset 实际定义** 为准。

| Step | 时间 (UTC) | 预期事件 | 实际事件 | 状态 | 证据 |
|------|-----------|----------|----------|------|------|
| 1 | 09:28:56 | `work.start` | `work.start` | ✅ | `events-20260624-092856.jsonl:1` |
| 2 | 09:31:29 | `work.ready`(step-01:project-skeleton) | `work.ready` | ✅ | `:2` |
| 3 | 09:31:38 | (无) | `work.ready` 重复 | ⚠️ | `:3`(同 task_id,8.4s 间隔) |
| 4 | 09:39:15 | `work.done`(step-01) | `work.done` | ✅ | `:4` |
| 5 | 09:41:57 | `test.passed`(step-01, 11/11) | `test.passed` | ✅ | `:5` |
| 6 | 09:48:42 | `work.ready`(step-02:bubble-sort) | `work.ready` | ✅ | `:6` |
| 7 | 09:56:37 | `work.done`(step-02) | `work.done` | ✅ | `:7` |
| 8 | 09:58:11 | `test.passed`(step-02, 33/33) | `test.passed` | ✅ | `:8` |
| 9 | 10:00:56 | `work.ready`(UNIT 2 step-01:quick-sort) | `work.ready` | ✅ | `:9` |
| 10 | 10:10:05 | `work.done`(UNIT 2 step-01) | `work.done` | ✅ | `:10` |
| 11 | 10:11:56 | `test.passed`(58/58) | `test.passed` | ✅ | `:11` |
| 12 | 10:12:03 | (无) | `test.passed` 重复 | ⚠️ | `:12`(7.0s 间隔) |
| 13 | 10:17:41 | `work.ready`(step-02:readme-integration) | `work.ready` | ✅ | `:13` |
| 14 | 10:24:04 | `work.done` | `work.done` | ✅ | `:14` |
| 15 | 10:25:56 | `test.passed`(82/82) | `test.passed` | ✅ | `:15` |
| 16 | 10:27:57 | `review.start` | `review.start` | ✅ | `:16` |
| 17 | 10:32:20 | `review.dimension.ready`(correctness) | `review.dimension.ready` | ✅ | `:17` |
| 18 | 10:38:30 | `review.dimension.done`(correctness, 3 P3) | `review.dimension.done` | ✅ | `:18` |
| 19 | 10:41:07 | `review.dimension.ready`(testing) | `review.dimension.ready` | ✅ | `:19` |
| 20 | 10:48:29 | `review.dimension.done`(testing, 7 P2/P3) | `review.dimension.done` | ✅ | `:20` |
| 21 | 10:50:02 | `review.dimensions.complete` | `review.dimensions.complete` | ✅ | `:21` |
| 22 | 10:50:06 | (无) | `review.dimensions.complete` 重复 | ⚠️ | `:22`(4.1s 间隔) |
| **23** | **10:53:03** | **`review.passed`(verdict=pass,skip_reason=dimensions_complete)** | **`review.complete`(verdict=pass_with_residuals,fix_plan=null)** | **❌** | **`:23`** |
| 24 | 10:54:25 | (next iteration 派生 `loop.complete`) | `recovery.jsonl:2 review_terminal_drift` | ❌ | `recovery.jsonl:2` |
| 25 | 10:57:25 | `plan.complete`(verdict=pass_with_residuals) | `plan.blocked(review_terminal_drift)` | ❌ | `:24` |
| 26 | 10:58:45 | (loop.complete via auto-derive) | `plan.blocked` 重复 | ❌ | `:25` |
| 27 | 11:04:46 | `REVIEW_COMPLETE(pass_or_fail=pass)` | `REVIEW_COMPLETE(pass_or_fail=fail)` | ❌ | `:26` |
| 28 | 11:07:42 | `report.done(verdict=pass)` | `report.done(verdict=fail, awaiting_decision=true)` | ❌ | `:27` |
| 29 | 11:08:05 | `LOOP_COMPLETE(plan.success)` | `loop.terminate(review_failed)` | ❌ | `events-history-20260624-092856.jsonl:2` |

### 关键偏离点

- **❌ P0-1**: `review-synthesizer` 缺前置 `review.passed`(Step 23) — 证据 `events-20260624-092856.jsonl:23`
- **❌ P0-2**: Coordinator emit `plan.blocked` 而非 `plan.complete`(Step 25-26) — 证据 `:24-25`
- **❌ P1-1**: Shipper 镜像 `REVIEW_COMPLETE(fail)` 而非 pass(Step 27) — 证据 `:26`
- **❌ P1-2**: Reporter emit `report.done(fail, awaiting_decision=true)`(Step 28) — 证据 `:27`
- **⚠️ Issue #1-3**: 3 次重复事件(`work.ready` / `test.passed` / `review.dimensions.complete`)

---

## 3. 历史问题上下文

### 30 天第 6+ 次复发的根因路径

| 类别 | 历史问题 | 关联度 | 证据 |
|------|----------|--------|------|
| **preset 设计** | `review_terminal_drift`(`pass_with_residuals` 漂移)— 30 天反复 | **高** | `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` |
| **preset 设计** | `hat_handoff_filename_mismatch` — 30 天 6 次复发 | 高 | `docs/solutions/developer-experience/ce-executor-serial-30day-6th-recurrence-fix.md:42` |
| **机制层** | `task.resume` 死信 — 5 次复发 | 高 | `docs/report/2026-06-21-top-3-architectural-instability-factors.md:9-56` |
| **机制层** | Stall detector 沉默 — 4 次复发 | 高 | `docs/report/2026-06-23-mechanism-review-layer3-history-patterns.md:51-55` |
| **preset 设计** | 4 维 → 2 维降维但 BDD 场景未同步(`d8e1da3d`) | 高 | `crates/ralph-cli/src/presets.rs:1211-1217` |
| **机制层** | `max_fix_rounds` 字段被完全删除,无 Rust 端硬约束 | 中 | `crates/ralph-core/src/config/loop_config.rs:339` |
| **agent** | `run_scenario` 是 stub,不做事件流断言 | 中 | `crates/ralph-core/tests/scenarios.rs:805-833` |

### 已闭环 vs 未闭环(2026-06-24 状态)

| 项 | 状态 | 关键 commit |
|---|---|---|
| `review_terminal_drift` 3 道防线 | **代码完成,本次 run 仍触发** | `b0309dd7` |
| `hat_handoff_filename_mismatch` SSOT 真正生效 | 部分闭环 | `b9c0fe9c` |
| `hat_handoff` 全链路失效 | 重置(已 active 删 hat_handoff) | `9f89f383` |
| 11→10 hat 重写 | active plan | `f31cb48f` |
| typed counter 按 kind 分桶 | 代码完成,consumer 未接 | round-2 fix |
| Stall detector 接 typed | **未闭环** | — |
| `task.resume` ralph→coordinator 通道 | **未闭环** | — |
| `UNIFIED_DETERMINISTIC_CORRECTION` 默认反转 | 保留 off(测试未迁移) | — |

---

## 4. 证据清单

### 4.1 run_dir 关键证据

| ID | 文件 | 行号 | 证据 |
|----|------|------|------|
| E-1 | `events-20260624-092856.jsonl` | :23 | `review.complete(verdict=pass_with_residuals, fix_plan=null, residual_findings_count=8)` 缺前置 `review.passed` |
| E-2 | 同上 | :24-25 | 2 次 `plan.blocked(reason=review_terminal_drift)` |
| E-3 | 同上 | :26 | `REVIEW_COMPLETE(pass_or_fail=fail, verdict=fail)` |
| E-4 | 同上 | :27 | `report.done(verdict=fail, awaiting_decision=true)` |
| E-5 | `events-history-20260624-092856.jsonl` | :2 | `loop.terminate(review_failed)` |
| E-6 | `recovery.jsonl` | :2 | `iteration=19, reason_code=review_terminal_drift, outcome=pending` |
| E-7 | `loop-termination-reason.json` | — | `{"review_failed":{"topic":"report.done"}}` |
| E-8 | `summary.md` | :4 | 总迭代 22 次,`final_verdict=fail` |
| E-9 | `diagnostics/2026-06-24T17-28-55/recovery.jsonl` | :2 | recovery_count=2 |
| E-10 | `trace.jsonl` | :1-3 | operator `ralph.yml` 含 `debug-resolver` 和 `plan-gate` hat 配置,被 preset 忽略 |

### 4.2 代码层证据(SSOT 不一致矩阵)

| ID | 文件:行号 | 声称 | 实际 | 不一致 |
|----|-----------|------|------|--------|
| C-1 | `presets/en/ce-executor-serial.yml:80-85` | `review-synthesizer` 仅发 `review.complete`,不需 `review.passed` 对子 | runtime drift detector 仍 expect `review.passed` | preset ↔ runtime drift |
| C-2 | `presets/en/ce-executor-serial.yml:1350-1351` | `publishes: ["review.complete"]` | 同 C-1 | preset 自身 |
| C-3 | `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml:77-84` | `review-synthesizer.publishes: [review.passed, review.failed, review.complete, plan.blocked]` | 生产 preset 已无 `review.passed`/`review.failed` emit | BDD ↔ preset |
| C-4 | `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml:128-165` | 4-dim review(correctness/testing/maintainability/requirements) | 生产 preset `ce-executor-serial.yml:877-881` 强制 2-dim | BDD ↔ preset |
| C-5 | `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml:85-94` | 定义 `plan-gate` hat | 生产 preset 已删除 plan-gate(`presets/en/ce-executor-serial.yml:9, 48`) | BDD ↔ preset |
| C-6 | `crates/ralph-core/tests/scenarios.rs:805-833` | `run_scenario` 是 stub,不查 events | `run_workflow_guard_scenario`(459-773)才是真 EventLoop runner | 测试基础设施 |
| C-7 | `crates/ralph-core/src/event_loop/mod.rs:9500-9548` | drift detector 按"review.passed 必须先于 review.complete" | preset 取消 review.passed 后 drift detector 未更新 | runtime ↔ preset |
| C-8 | `crates/ralph-core/src/config/loop_config.rs:339` | `max_fix_rounds` 字段已删除 | fixer hat 仍注释 `max 10`,无 Rust 端强制 | Rust ↔ preset instructions |
| C-9 | `presets/schemas/ce-executor-serial.yml` | `fix_plan` 改 `fix_plan_file`(文件路径) | BDD 场景未携带 `fix_plan_file` 字段 | schema ↔ BDD |
| C-10 | `presets/en/ce-executor-serial.yml:877-881` | "The sequence is always 2 dimensions" | BDD 场景发出 4-dim 事件流 | preset ↔ BDD |

### 4.3 历史 commit 佐证

| commit | 类型 | 与本次关联 |
|--------|------|-----------|
| `d8e1da3d` | feat(ce-executor-serial) | 降维至 correctness + testing — 场景未同步的起点 |
| `b9c0fe9c` | fix(ce-executor-serial) | 对抗性审查 P0/P1/P2 全量修复 — SSOT 真正生效 |
| `b0309dd7` | fix(ce-executor-serial) | 双轮 review P0/P1 修复 |
| `65e0bc62` | fix(ce-executor-serial) | 3 道防线机制层修复 |
| `f31cb48f` | docs(plan) | ce-executor-serial preset 重写计划 |
| `06a83079` | fix(ce-executor-serial) | fix.exhausted 改单消费者路由 |
| `b6c3d551` | feat(config) | `max_fix_rounds` 首次引入(现已被反向删除) |

---

## 5. 问题归因表(P0 / P1 / P2)

### 直接回答用户三个问题

#### Q1:编排机制有问题吗?

**有问题,但不是机制本身错,是机制改了一半。** 见 P0-1。

#### Q2:修复机制失效吗?

**没有失效,是设计性移除后未同步。** `b9c0fe9c` + `d8e1da3d` commit 主动决定:
- `review-synthesizer` 只发 `review.complete`,不发 `review.passed`/`review.failed` 对子
- 但 runtime drift detector 仍按旧契约检查 `review.passed` 前置
- shipper 收到 `plan.blocked` 后只走 fail 分支,不识别 `pass_with_residuals`

这是**契约不一致**,不是机制失效。

#### Q3:Ralph 自身有 bug 吗?

**有 2 个 P0:**
- `run_scenario` 是 stub(已存在 6+ 天,2026-06-20-002 U3 review v2 报告 F1 标记但未修)
- drift detector 取消 `review.passed` 前置约束后未更新

### 完整归因表

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|--------|----------|----------|------|----------|
| **P0-1** | **`review-synthesizer` 缺前置 `review.passed`,drift detector 仍按旧契约拦截** | **preset ↔ runtime SSOT 不一致** | C-1, C-2, C-7 | 是(`review_terminal_drift` 复发) |
| **P0-2** | **BDD 场景 `ce_executor_serial_review.yml` 拓扑(4-dim + plan-gate + review.passed)与生产 preset 完全失配,但 `run_scenario` stub 不查事件,失配被静默吞掉** | **agent(测试基础设施 bug)** | C-3, C-4, C-5, C-6 | 否(根因是 stub) |
| **P0-3** | **`run_scenario` stub 长期存在(6+ 天),不做任何事件流断言** | **agent(测试基础设施 bug)** | `crates/ralph-core/tests/scenarios.rs:805-833` | 否 |
| **P1-1** | **3 个场景文件全部基于旧 topology(4-dim / plan-gate / fixer / review.passed)** | **preset(场景未同步)** | C-3, C-4, C-5, C-9 | 是(同 `d8e1da3d` 起点) |
| **P1-2** | **`max_fix_rounds` 字段被完全删除,fixer 无 Rust 端硬上限** | **preset/Rust 基座** | C-8 | 是(`b6c3d551` 反向) |
| **P1-3** | **3 次重复事件(work.ready/test.passed/review.dimensions.complete)— dedup 强度不足** | **机制层** | E-1 行 :3, :12, :22 | 是(perky-maple 同型) |
| **P1-4** | **shipper 镜像 `plan.blocked` 直接翻译为 fail,无 `pass_with_residuals` 分支** | **preset(行为设计)** | E-3 行 :26 | 是(`pass_with_residuals` 漂移复发) |
| **P1-5** | **场景 payload 未携带 schema 新增的 `fix_plan_file` 字段** | **preset(场景未同步)** | C-9 | 否 |
| **P2-1** | **预设警告:operator `ralph.yml` 含 `debug-resolver` 和 `plan-gate` hat 配置,被 preset 忽略** | **配置层** | E-10 | 是(ralph.yml 警告) |
| **P2-2** | **软提示架构未根治(`## notes` 35 词超 15 上限)** | **agent 软提示** | `docs/report/2026-06-21-top-3-architectural-instability-factors.md:58-95` | 是 |
| **P2-3** | **SSOT 多点不同步(`presets/en` vs `presets/schemas` vs BDD vs runtime drift)** | **preset 设计** | C-1 ~ C-10 矩阵 | 是 |
| **P2-4** | **payload `residual_findings_count=8` 但 `findings.md` 未列出 8 条 finding 详情** | **agent 产物** | E-1 vs `findings.md` | 否 |
| **P2-5** | **Recovery journal `outcome=pending` 长时间未升级** | **机制层** | E-6 | 是(typed routing consumer 未接) |
| **P2-6** | **BDD 4-dim 与生产 2-dim 测试断言不一致** | **preset** | C-10 | 是 |

---

## 6. 修复建议(按优先级排序)

### Fix-1【P0,关键】:修复 `review-synthesizer` 与 drift detector 的 SSOT 不一致

**目标文件**:
- `crates/ralph-core/src/event_loop/mod.rs:9500-9548`(drift detector)
- `presets/en/ce-executor-serial.yml:80-85`(preset 注释)

**具体修改**:
- **方案 A(recommended)**:`review-synthesizer` 在 emit `review.complete(verdict=pass_with_residuals)` 前**自动补发**一个 `review.passed(verdict=pass, skip_reason=pass_with_residuals_acknowledged)`,让 verdict_gate 仍能识别。
- **方案 B**:drift detector 改为读取 `review.complete` 自身 verdict 字段,不再要求 `review.passed` 前置。
- **方案 C**:coordinator 收到 `review.complete(verdict=pass_with_residuals)` 时,直接 emit `plan.complete(verdict=pass_with_residuals)`(允许残留通过)。

**预期效果**:本次 run 的 Step 23 → Step 25 链路恢复正常,`pass_with_residuals` 不再被镜像为 fail。

**验证方法**:
- 重跑 plan `feat-python-sort-algorithms-plan`,预期 verdict=pass_with_residuals 且 loop 完成
- 加单测:`cargo nextest run -p ralph-core --test drift_detector` 新增 `test_review_complete_with_residuals_accepted`

### Fix-2【P0,关键】:把 3 个 serial review 场景迁到 `run_workflow_guard_scenario`

**目标文件**:
- `crates/ralph-core/tests/scenarios.rs:1520-1532`(test_ce_executor_serial_review_scenario)
- `crates/ralph-core/tests/scenarios.rs:1762-1766`(test_ce_executor_serial_review_silent_reviewer_recovers_scenario)
- `crates/ralph-core/tests/scenarios.rs:1771-1775`(test_ce_executor_serial_fix_applied_rereview_scenario)

**具体修改**:把 `run_scenario` 调用换成 `run_workflow_guard_scenario`,这样才会真正断言 `expected.events` / `completion` / `absent_events`。

**预期效果**:改动后这 3 个测试会因 `expected.events` 断言触发而**直接失败**,暴露出拓扑漂移问题。

**验证方法**:
- `cargo nextest run -p ralph-core --test scenarios -- test_ce_executor_serial_review_scenario`
- 预期 fail 报 `Expected event 'review.passed' to be seen (accepted), but it was not recorded`

### Fix-3【P0,关键】:将 3 个场景重写到新 topology(2-dim + plan-gate removed + review.complete 终态)

**目标文件**:
- `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`
- `crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml`
- `crates/ralph-core/tests/scenarios/ce_executor_serial_fix_applied_rereview.yml`

**具体修改**:
1. 去掉 `plan-gate` hat,`coordinator` 直接发 `plan.complete`
2. dimension 从 4 降到 2(只保留 correctness + testing),mock_responses 从 16 缩到 12
3. `review-synthesizer` 改为只发 `review.complete`,payload 含 `verdict` / `fix_plan_file` 或 `"null"`
4. 去掉 `queue.advance` 中间事件
5. 去掉 `fixer` hat(在 fix_applied_rereview 中)
6. 同步 `expected.events` 列表(从 16 个缩到 12 个)

**预期效果**:3 个场景成为真正的 smoke alarm,任何 topology 回归会立即在 CI 红掉。

**验证方法**:`cargo nextest run -p ralph-core --test scenarios` 全套 3 个 serial 场景 PASS。

### Fix-4【P1,稳健性】:在 Rust 端给 fixer 设置硬上限

**目标文件**:`crates/ralph-core/src/event_loop/mod.rs`(rejection_stall 检测器)

**具体修改**:在 `rejection_stall` 检测器或 `progress-steward` 逻辑里加规则——当 `state.fix_round >= 10` 时,自动注入 `plan.blocked` 并停 loop。

**预期效果**:`max_fix_rounds` 字段被删除后,fixer 仍有 Rust 端硬约束,instructions 漂移时仍能在第 11 轮自动停摆。

**验证方法**:`cargo nextest run -p ralph-core --test progress_steward`,mock fixer 11 次循环后断言 `plan.blocked` 被 emit。

### Fix-5【P1,防御性】:把 `run_scenario` 标记为 `#[deprecated]` 或直接删除

**目标文件**:`crates/ralph-core/tests/scenarios.rs:805-833`

**具体修改**:在 `run_scenario` 上加 `#[deprecated(note = "stub only — does not assert events; use run_workflow_guard_scenario")]`。

**预期效果**:防止新加的 BDD 场景误用 stub。

**验证方法**:跑 `cargo nextest run -p ralph-core --test scenarios --no-fail-fast 2>&1 | grep -i deprecat`。

### Fix-6【P2,长期】:建立"preset 改 → 场景同步"反向门禁

**目标文件**:`crates/ralph-cli/src/presets.rs:1188-1218` 旁边

**具体修改**:在 `test_ce_executor_serial_review_sequence_is_two_dimensions` 旁边加反向断言:

```rust
let scenario_yaml = include_str!("../../ralph-core/tests/scenarios/ce_executor_serial_review.yml");
assert!(!scenario_yaml.contains("maintainability"));
assert!(!scenario_yaml.contains("requirements"));
assert!(!scenario_yaml.contains("plan-gate"));
```

**预期效果**:任何未来对 preset 的拓扑改动会立即被 CI 拦截。

**验证方法**:故意把场景改回 4-dim,跑 `cargo nextest run -p ralph-cli` 应 fail。

### Fix-7【P1,可选】:为 shipper 增加 `pass_with_residuals` 显式分支

**目标文件**:`presets/en/ce-executor-serial.yml:499-500` 或 shipper hat 配置

**具体修改**:shipper 收到 `plan.complete(verdict=pass_with_residuals)` 时,emit `REVIEW_COMPLETE(pass_or_fail=pass)` 而不是镜像 fail。

**预期效果**:即使 Fix-1 未实施,shipper 仍能正确处理残留通过场景。

**验证方法**:mock 一个 `plan.complete(verdict=pass_with_residuals)`,断言 shipper emit pass。

---

## 7. 关键文件路径速查

### Preset 源(SSOT)

- `presets/en/ce-executor-serial.yml` — 新 topology(2-dim,coordinator 直驱,review.complete 终态)
- `presets/schemas/ce-executor-serial.yml` — payload 契约(已 `fix_plan` → `fix_plan_file`)

### BDD 场景(失配)

- `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml:77-84, 85-94, 128-165` — 4-dim + plan-gate + review.passed 终态
- `crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml:64-114` — 同上 + DR 恢复
- `crates/ralph-core/tests/scenarios/ce_executor_serial_fix_applied_rereview.yml:69-90` — 同上 + fixer + fix.applied

### 测试基础设施(stub 根因)

- `crates/ralph-core/tests/scenarios.rs:805-833` — `run_scenario` stub
- `crates/ralph-core/tests/scenarios.rs:459-773` — `run_workflow_guard_scenario` 真 EventLoop runner
- `crates/ralph-core/tests/scenarios.rs:1520-1532, 1762-1766, 1771-1775` — 3 个失调用点

### Rust 改动(2026-06-24)

- `crates/ralph-core/src/config/loop_config.rs:339, 464, 625-627` — `max_fix_rounds` 字段删除
- `crates/ralph-cli/src/preflight.rs:746, 1672-1700` — opt-in + 测试删除
- `crates/ralph-cli/src/config_resolution.rs:73, 282, 303` — strip + 测试删除
- `crates/ralph-core/src/event_loop/mod.rs:520-543, 3589, 3748` — `append_runtime_config_block` 签名变更
- `crates/ralph-core/src/event_loop/mod.rs:9500-9548` — drift detector(未跟随 preset 更新)

---

## 8. 报告边界与免责

- **本报告基于 run_dir `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph` 的中间产物**(注:本机不存在该路径,实际 run_dir 在 `/home/chaowen/Dev/agent_tools/ralph-orchestrator/.ralph`)
- **Agent A/B/C/D 并行运行**,各自独立收集证据,交叉验证
- **所有证据带文件路径 + 行号 / 事件 ID**
- **历史关联引用**:`docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md`、`docs/report/2026-06-21-top-3-architectural-instability-factors.md`、`docs/report/2026-06-23-mechanism-review-layer3-history-patterns.md`
- **本次 run 极可能与 `review_terminal_drift` 同型**(`docs/report/2026-06-24-ce-executor-serial-dual-review-fix.md:220` 已 PASS 但实际未覆盖所有路径)

---

## 9. 给用户的一句话总结

**编排流程没按预设走 + 修复机制失效 + 没回到正轨** 这三个症状的根因是**同一个**:`d8e1da3d` + `b9c0fe9c` 这两个 commit 改了 preset 的 review 终态语义,但 BDD 场景、runtime drift detector、shipper 三个地方都没同步更新,导致 SSOT 四点不一致。`run_scenario` stub 又把所有 BDD 守护链静默吞掉,所以这个问题潜伏到了生产 run 才爆。

**最关键修复**:**Fix-1**(让 `review.complete` 自带 verdict 通过 gate)+ **Fix-2/3**(把 BDD 场景迁到真 EventLoop runner 并同步新 topology)。其他都是加固。