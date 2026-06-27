# Ralph 编排机制公共底座 — 需求文档

> **生成日期**:2026-06-27
> **方法**:ce-brainstorm 框架(Phase 1.3 对话收尾 + Phase 2.5 综合 + Phase 3 落档)
> **触发输入**:`docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md` 报告(22 events / 28 recovery / 5 drift / 7 tasks / 30 天第 7+ 次复发)
> **目标读者**:ce-plan / ralph orchestrator 维护者 / 任何写 preset 的 operator

---

## 1. 背景与目标

### 1.1 为什么做

Ralph 编排机制过去 30 天里反复出现**同一类根因的 3 个变体**——"修复机制系统性失效 + loop-termination 语义错位",已经在 `primary-20260624-092856`、`primary-20260623-152241`、`keen-fern`、`nimble-teak`、`zippy-otter` 等多次 loop 中命中,本次(2026-06-26-001)命中**报告所述问题的 95%**。

**本次触发的 3 个根本原因(第一性原理层)**:

1. **任务身份信息丢失** — worktree 复用时,`tasks.jsonl` 里 legacy task 没有 `loop_id`,`execution_contract.rs:499-532` 直接拒收(`TaskWrongLoop { actual_loop: None }`),事件被打回
2. **修复机制不会"自我纠错",只会"原地重试"** — `task.resume` 走的是普通事件流,同 task 在 28 条 recovery 记录里反复 `Pending → Recovered → Repeated → Pending`,没有"修复预算"的概念
3. **契约是软建议,运行时是宽松的** — `plan.blocked(reason="")` 这种"必填字段缺失"的事件能通过校验,drift 报警但 shipper 照样接

**这 3 个根因之所以反复发生**,是因为当前的修复策略都是**逐症状打补丁**(每个 P0 / P1 / P2 各修一处),**而不是修公共底座**。本次报告里 P0-A / P0-B / P0-C / P0-D 全部命中"30 天前已识别但未闭环"问题(参见 `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` solution:178-185)。

### 1.2 目标

把 Ralph 编排机制从"软提示 + 重试"升级为"**声明式流转 + 幂等状态 + 硬契约 + 独立修复**"的公共底座,做到:

- **不漏修**:任何一次失败都能在机制层找到对应防线,不需要再补"下一个补丁"
- **不复发**:同一类根因的 3 个变体被同时堵死,未来 30 天不会再写"第 8+ 次复发"的诊断报告
- **可移植**:所有 builtin preset(`ce-executor-serial` / `autoresearch` / `merge-loop` / `debug` / `hatless-baseline`)都自动受益,新加 preset 不需要重复发明底座

### 1.3 明确"不做"什么(YAGNI)

本轮**不做**:

- ❌ 改 hats 本身的实现(切菜工还是切菜工,试菜工还是试菜工)
- ❌ 改后端模型适配器(`crates/ralph-adapters/`)
- ❌ 改 TUI / Web Dashboard
- ❌ 引入新的依赖(serde_yaml / jsonlogic 已经在用,新增 crate 需要专门 PR 评估)
- ❌ 改动 `presets/en/ce-executor-serial.yml` 业务逻辑(只加**机制层 metadata**,不改 hat prompt)
- ❌ 一键回滚 / undo run(超出"机制底座"范畴,留作未来 plan)

---

## 2. 现状与根因(机制层)

### 2.1 当前架构图(简化)

```
┌──────────────────────────────────────────────────────────────┐
│                      Ralph 编排引擎                          │
│                                                              │
│   JSONL ─→ EventReader ─→ EventParser ─→ EventPolicy        │
│                                       │                      │
│                                       ↓                      │
│                                  StateMachine                │
│                                       │                      │
│                                       ↓                      │
│                                   EventBus ──→ Hats          │
│                                                              │
│   状态文件(tasks.jsonl / events.jsonl / recovery.jsonl /     │
│            drift.jsonl):append-only,无幂等键,无版本号         │
└──────────────────────────────────────────────────────────────┘
```

### 2.2 三大机制层根因

#### 根因 A:**"事件流自组织"导致拓扑断点**

**证据**(报告 iter=17):

```
预期流程:4/8 完成 → coordinator 决定 plan.complete 或 review.start
实际流程:4/8 完成 → coordinator 两个都没发 → shipper 被踢上场 → 
         绕过 6 维审查 → verdict_gate 看到 fail 自动终止
```

**机制层原因**:coordinator 不知道"做完 N/M 时该怎么办"。事件流是"做完一步看下一步",**没有"完整流程声明"**。

#### 根因 B:**"状态变更 append-only + 无版本号"导致反复翻烧饼**

**证据**(报告 §2.7 / §4.2):

- `recovery_count=28` 与 `drift_finding_count=0` 互相矛盾
- 同一 `task-1782490209-u001` 在 7 个 iteration 反复 `Pending → Recovered → Repeated → Pending`
- worktree 复用时,老 task 无 `loop_id`,新 loop 直接拒收

**机制层原因**:状态文件是"日志",不是"数据库"。**没有幂等键 + 没有版本号**,worktree 复用导致状态污染。

#### 根因 C:**"契约是软建议 + drift 只报警不拦截"导致运行时宽松**

**证据**(报告 §4.1 E5 / §3.1):

- `plan.blocked(reason="")` 触发 shipper hard-fail → drift 报 `reason present in 0/1 events` critical
- `task.resume` 缺 `kind` 字段 → drift 报 `kind present in 0% events` critical
- 报警归报警,事件照样 emit,shipper 照样接

**机制层原因**:`presets/schemas/*.yml` 的 `required_fields` 是"建议",drift_monitor 是"事后审计",**没有 emit-time gate**。

### 2.3 报告中的 P0/P1 与 4 药方对应

| 报告问题 | 对应药方 | 报告原文 |
|---|---|---|
| P0-A TaskWrongLoop 反复触发 | 药方 1(声明式流转)+ 药方 4(独立修复) | "execution_contract.rs:518-532 legacy task + loop_scoped=true 设计性 fail-closed" |
| P0-B progress-steward 无回填通道 | 药方 4(独立修复) | "preset 只允许 steward 选 5 种 emit,没有任何路径回填 legacy task 的 loop_id" |
| P0-C shipper hard-fail vs verdict_gate 错位 | 药方 1(声明式流转)+ 药方 2(幂等状态) | "loop-termination-reason.json 写 `review_failed.topic=report.done`" |
| P0-D completion_promise 契约冲突 | 药方 1(声明式流转) | "`completion_promise: LOOP_COMPLETE` + reporter fail 禁止 emit LOOP_COMPLETE" |
| P0-E review pipeline 6-dim walk 完全缺失 | 药方 1(声明式流转) | "review.* 系列为 0" |
| P0-F coordinator task_id 永远空 | 药方 2(幂等状态)+ 药方 4(独立修复) | "coordinator step-03/04 task_id 永远空" |
| P1-A stall_recovery 死循环 | 药方 4(独立修复) | "stall_recovery 28 次空转" |
| P1-B plan.blocked reason 缺校验 | 药方 3(硬契约) | "drift_monitor 报 `plan.blocked.reason present in 0/1 events`" |

---

## 3. 范围:为什么是公共引擎层而不是单 preset

### 3.1 决策依据

报告 §1 表格里 95% 命中"30 天第 7+ 次复发"——**单 preset 修不完**。

| 候选范围 | 评诂 |
|---|---|
| 只改 `ce-executor-serial` | ❌ 其他 4 个 builtin preset 也会复发同一类问题 |
| 所有 builtin preset 逐个修 | ❌ 重复劳动,且互相不一致(报告 `keen-fern` / `zippy-otter` 等 loop 已经证明) |
| **公共引擎层(`event_loop/*` + `preset_lint/*` + 状态文件格式)** | ✅ 一次性受益所有 preset,且机制层问题**只能在机制层修** |

### 3.2 引擎层改动范围

涉及的核心模块(均已在报告 / 历史 plan 中引用):

| 模块路径 | 现状 | 本轮变动 |
|---|---|---|
| `crates/ralph-core/src/event_loop/mod.rs` | 事件循环主流程 | 集成药方 1 / 3 / 4 |
| `crates/ralph-core/src/event_loop/loop_state.rs` | 状态追踪 | 集成药方 2(幂等键 + 版本号) |
| `crates/ralph-core/src/event_loop/policy.rs` | 契约校验 | 强化为硬门禁(药方 3) |
| `crates/ralph-core/src/execution_contract.rs` | task 校验 | 扩展 legacy task 回填通道(药方 4) |
| `crates/ralph-core/src/event_loop/flow_lifecycle.rs`(已存在) | 流程声明 | 升级为完整声明式流转(药方 1) |
| `crates/ralph-core/src/preset_lint/*.rs` | preset 静态检查 | 新增 4 条机制层 lint |
| `presets/schemas/*.yml` | schema 必填字段 | 编译期约束升级(药方 3) |
| `presets/en/*.yml` | preset 定义 | **只加 mechanism metadata,不改业务逻辑** |

### 3.3 不改 hats 业务逻辑

本次需求**严格不碰**以下文件的内容(只读取 metadata):

- `presets/en/ce-executor-serial.yml` 的 `coordinator.instructions` / `executor.instructions` 等 hat prompt 段
- `crates/ralph-core/src/hat_lifecycle.rs` 的 hat 行为实现
- `crates/ralph-adapters/` 所有 adapter

---

## 4. 4 个药方的需求详情

### 4.1 药方 1:**声明式流转**

#### 4.1.1 用户故事

> 作为 **operator**,我配置一个 preset 时,我应该能**一眼看清楚整个流程长什么样**——8 个单元 + 6 维审查 + ship,每一步白纸黑字。做到 4/8 时,Ralph 必须**强制**知道下一步是 `continue_to_review` 还是 `block_with_reason`,**不能模糊**。

#### 4.1.2 现状(报告 §2.5 iter=17 拓扑断点)

```
coordinator 在 4/8 状态时:
  - 不知道该发 review.start
  - 不知道该发 plan.blocked
  - 两个都没发 → shipper 被 progress-steward 强行激活
```

#### 4.1.3 改后(目标)

`presets/en/*.yml` 增加 `mechanism.flow` 段(纯 metadata,不进入 hat prompt):

```yaml
mechanism:
  flow:
    type: declared
    version: 1
    steps:
      - id: unit_loop
        kind: foreach
        over: plan.units
        body:
          - work.ready
          - work.done | work.failed
          - test.passed | test.failed
          - fix.applied (if test.failed)
        terminal_when: all_units_done
      - id: review_walk
        kind: sequence
        body:
          - review.start
          - review.dimension.ready × 6
          - review.complete
        emit_when: unit_loop.terminal == all_done
      - id: plan_end
        kind: branch
        on_test_passed: plan.complete
        on_review_failed: plan.blocked(reason="review_failed")
        on_residual: plan.complete(verdict="pass_with_residuals")
      - id: ship
        kind: sequence
        body: [REVIEW_COMPLETE, report.done, LOOP_COMPLETE]
```

**核心约束**:

| 约束 ID | 描述 | 验证点 |
|---|---|---|
| F1.1 | 每个 step 必须声明 `allowed_emits`,emit 不在列表 → runtime reject | BDD scenario: `flow_unknown_emit_rejected` |
| F1.2 | 4/8 半完成状态必须显式声明 `on_partial` 分支,缺声明 → lint reject | lint: `flow_partial_state_undeclared` |
| F1.3 | flow 必须声明 `terminal_emits`(loop 终止的合法 emit 集合),emits 不在集合 → verdict_gate reject | unit test: `flow_terminal_emit_whitelist` |
| F1.4 | coordinator 在 flow 中**只能 emit 当前 step 的 allowed_emits**,不允许跨步 emit | lint: `flow_hat_step_scope` |

#### 4.1.4 涉及模块

- **新文件**:`crates/ralph-core/src/event_loop/flow_declaration.rs`(parse `mechanism.flow` YAML → 内部 `FlowDeclaration` 结构)
- **改**:`crates/ralph-core/src/event_loop/flow_lifecycle.rs`(已存在,集成 `FlowDeclaration` 校验)
- **改**:`crates/ralph-core/src/event_loop/mod.rs`(在 step-close 分支前置校验 allowed_emits)
- **改**:`crates/ralph-core/src/preset_lint/flow_declaration.rs`(新模块,4 条 lint)
- **改**:`presets/en/*.yml`(所有 builtin preset 加 `mechanism.flow` 段)

#### 4.1.5 不做什么

- ❌ 不改 hat prompt 内容(只在 preset 顶层加 metadata)
- ❌ 不实现"自动推断 flow"(必须显式声明,YAGNI)
- ❌ 不支持 flow 内 step 之间的复杂 control flow(if-else chain / dynamic dispatch)

---

### 4.2 药方 2:**幂等状态**

#### 4.2.1 用户故事

> 作为 **diagnoser**,我看 `tasks.jsonl` / `recovery.jsonl` 时,**应该一眼看清楚"现在到底是什么状态"**——不会出现"28 条记录反复翻烧饼"的情况。worktree 复用时,**老 task 不会污染新 loop**。

#### 4.2.2 现状(报告 §2.6 / §4.2 E9 / §2.7)

```
recovery.jsonl 28 条:
  iter 2: outcome=pending
  iter 3: outcome=escalated
  iter 7: outcome=recovered
  iter 8: outcome=repeated
  iter 10: outcome=recovered
  iter 11: outcome=repeated
  iter 15: outcome=recovered
  iter 16: outcome=repeated

diagnosis-summary.json:
  recovery_count=28, drift_finding_count=0 (但 drift.jsonl 实际有 5 条)
```

#### 4.2.3 改后(目标)

状态文件全部从"append-only 日志"升级为"幂等键数据库":

**JSONL 记录格式扩展**(所有 `*.jsonl` 文件):

```json
{
  "_idempotency_key": "task:task-1782490209-u001:loop:2026-06-26-001",
  "_version": "v1",
  "_final": true,
  "_created_at": "2026-06-26T16:12:42Z",
  "_transitions": [
    { "at": "16:12:42", "from": null, "to": "open" },
    { "at": "16:20:20", "from": "open", "to": "closed" }
  ],
  // 原 payload 字段
  "task_id": "task-1782490209-u001",
  "loop_id": "2026-06-26-001",
  "status": "closed",
  ...
}
```

**核心约束**:

| 约束 ID | 描述 | 验证点 |
|---|---|---|
| S2.1 | 同 `_idempotency_key` + `_final=true` 只能写**一次**,二次写直接 reject | unit test: `state_idempotent_final_write` |
| S2.2 | 中间过程写 `_transitions[]` 数组(可追溯),不直接覆盖 `_final` | unit test: `state_transition_log_preserved` |
| S2.3 | worktree 复用时,新 loop 的 `_version` 必须**递增**,老版本记录自动隔离到 `.ralph/archive/{old_loop_id}/` | integration: `worktree_reuse_state_isolation` |
| S2.4 | `diagnosis-summary.json` 的计数 = 读 `_final=true` 的记录数(不再从"行数"猜) | BDD: `diagnosis_count_matches_final_state` |
| S2.5 | `_idempotency_key` 缺失 → runtime reject,记录不允许"无键" | lint: `state_missing_idempotency_key` |

#### 4.2.4 涉及模块

- **新文件**:`crates/ralph-core/src/state/idempotent_log.rs`(幂等键写入 + 状态机)
- **改**:`crates/ralph-core/src/task_store.rs`(task 写入走幂等路径)
- **改**:`crates/ralph-core/src/diagnosis/envelope.rs` + `reporter.rs`(recovery 写入走幂等路径)
- **改**:`crates/ralph-core/src/drift/engine.rs`(drift 写入走幂等路径)
- **改**:`crates/ralph-core/src/worktree.rs`(复用检查版本号,自动 archive)
- **新文件**:`crates/ralph-core/src/state/idempotent_log_tests.rs`
- **新 lint**:`crates/ralph-core/src/preset_lint/state_idempotency.rs`(检测旧格式 jsonl)

#### 4.2.5 不做什么

- ❌ 不引入新的存储后端(继续用 JSONL + 文件锁,不引 SQLite / sled)
- ❌ 不做分布式状态合并(单 loop 范围内幂等,YAGNI)
- ❌ 不改 `.ralph/` 目录结构(继续在原路径写入,老记录自动 archive 到子目录)

---

### 4.3 药方 3:**硬契约**

#### 4.3.1 用户故事

> 作为 **operator**,我看到 `presets/schemas/*.yml` 写"必填字段 X",**X 就必须真的必填**——不是"建议必填",是"emit 时缺这个字段事件根本进不了事件流"。

#### 4.3.2 现状(报告 §3.1 / §4.1 E5)

```
plan.blocked(reason="")   → drift 报警,但 shipper 照样接
task.resume(kind=缺失)    → drift 报 critical 0%,但 orchestrator 照样 emit
loop-termination-reason.json 写 review_failed.topic=report.done → verdict_gate 自动接管
```

#### 4.3.3 改后(目标)

**前置闸门 + 事后审计**两层:

```
event emit
   │
   ↓
[emitter] → enforce_schema_at_emit → reject if missing required_fields
   │                                       │
   │                                       ↓ reject (event NOT in event stream)
   ↓ accept
[event_policy] → publish to EventBus
   │
   ↓
[drift_monitor] → audit (事后,不拦截)
```

**核心约束**:

| 约束 ID | 描述 | 验证点 |
|---|---|---|
| C3.1 | emit 入口强制校验 `required_fields`,缺字段 → emit 失败 + 写 recovery envelope(不写 events.jsonl) | unit test: `emit_missing_required_field_rejected` |
| C3.2 | schema 改了必须重新生成对应 Rust 类型,**编译期检查**(`build.rs` 比对 schema hash 与 generated type hash) | build.rs 集成测试 |
| C3.3 | drift_monitor **只审计已 emit 的事件**,不参与实时拦截 | refactor test: `drift_no_real_time_block` |
| C3.4 | `plan.blocked` 必须有 `reason`,`task.resume` 必须有 `kind/reason/target_hat`,缺字段 → C3.1 reject | BDD: `plan_blocked_reason_required` + `task_resume_fields_required` |
| C3.5 | `human.guidance` topic 如果 `suppress_human_guidance=true` 则**根本不允许 emit**(当前是"emit 了再丢弃",变成"不允许 emit") | unit test: `suppress_human_guidance_blocks_emit` |

#### 4.3.4 涉及模块

- **新文件**:`crates/ralph-core/src/event_loop/enforce_schema_at_emit.rs`(核心闸门)
- **改**:`crates/ralph-core/src/event_loop/policy.rs`(从"建议"升级为"门禁")
- **改**:`crates/ralph-core/src/drift/engine.rs`(只审计,不拦截)
- **改**:`crates/ralph-core/build.rs`(schema hash 比对)
- **改**:`presets/schemas/*.yml`(标记 required_fields 编译期约束)
- **改**:`presets/en/ce-executor-serial.yml`(`suppress_human_guidance` 行为升级)

#### 4.3.5 不做什么

- ❌ 不引入 JSON Schema 库(继续用现有 serde_yaml + 自研 validator)
- ❌ 不支持运行时 schema 热更新(必须重新编译,YAGNI)
- ❌ 不改 hat 内部字段命名(只校验 schema,不改 prompt 字段)

---

### 4.4 药方 4:**独立修复流程**

#### 4.4.1 用户故事

> 作为 **diagnoser**,我看到一个 task 反复出错,**Ralph 应该自动知道"修了 N 次还没好,叫人"**——同一个水管工不会反复来 28 次。修复和正常工作**完全分开**,系统能区分"在修"和"在做"。

#### 4.4.2 现状(报告 §2.7 / §4.2 E7 E8)

```
task-1782490209-u001 在 7 个 iter 反复 stall_recovery escalate
task-1782490209-u002 同样 4 轮 stall 循环
task.resume(target=validator) → validator 不激活 → 再 timeout → 再 resume
                                  (28 次循环)
```

#### 4.4.3 改后(目标)

修复流程独立,带预算 + 状态机:

```
[异常检测]
   │
   ↓
[修复流独立通道] (不与正常事件流混)
   │
   ├─ detected → diagnosing (budget=3)
   ├─ diagnosing → fixing (budget=3)
   ├─ fixing → verifying (budget=3)
   ├─ verifying → closed (成功)
   │
   └─ 任意步骤 budget 用尽 → plan.blocked(reason="repair_unrecoverable_after_N_retries")
                           → 走终止流程(verdict_gate + shipper)
```

**新增修复主题**(独立通道):

| topic | publisher | 含义 |
|---|---|---|
| `task.relocate` | repair-flow | 把 task 重新归属到正确 loop |
| `task.relink` | repair-flow | 重新关联 task_id 到 task_key |
| `task.relocate_legacy` | repair-flow | 修复 loop_id=null 的 legacy task(报告 P0-A 根因) |
| `repair.budget.exhausted` | repair-flow | 修复预算耗尽,升级到终止 |

**核心约束**:

| 约束 ID | 描述 | 验证点 |
|---|---|---|
| R4.1 | 修复事件**走独立事件流通道**(`repair_stream`),不和 `main_stream` 混 | integration: `repair_stream_isolation` |
| R4.2 | 同 task 修复 budget 默认 3 次,可由 preset `repair_budget` 覆盖 | unit test: `repair_budget_default_3` |
| R4.3 | budget 用尽 → 自动 emit `plan.blocked(reason=repair_unrecoverable_after_N_retries)` | BDD: `repair_budget_exhausted_blocks_plan` |
| R4.4 | `task.relocate_legacy` 是 progress-steward hat 的**显式权限**,其他 hat 不允许 emit | lint: `repair_topic_authorization` |
| R4.5 | 修复完成后,**清零** stall_recovery retry counter,避免"修复成功但仍 stall" | unit test: `repair_close_resets_stall_counter` |

#### 4.4.4 涉及模块

- **新文件**:`crates/ralph-core/src/event_loop/repair_flow.rs`(独立通道 + 状态机 + budget)
- **新文件**:`crates/ralph-core/src/event_loop/repair_state.rs`(per-task 修复状态)
- **改**:`crates/ralph-core/src/execution_contract.rs`(扩展:legacy task 回填通道 `relocate_legacy_tasks`)
- **改**:`crates/ralph-core/src/event_loop/loop_state.rs`(stall_recovery counter 在 repair_close 时清零)
- **改**:`presets/en/ce-executor-serial.yml`(`progress-steward` 权限扩展,加 `repair_budget: 3`)
- **新 lint**:`crates/ralph-core/src/preset_lint/repair_authorization.rs`

#### 4.4.5 不做什么

- ❌ 不实现"自动选择修复策略"(修复哪条路径由 preset 显式声明,YAGNI)
- ❌ 不让 hats 自己 emit 修复主题(只有 progress-steward / repair-flow 模块可以)
- ❌ 不改 fixer hat 本身(fixer 还是 fixer,只是在 repair_flow 失败后被升级替换)

---

## 5. 用户视角的"用起来长什么样"(before vs after)

### 5.1 配置 preset 时

**before**:

```yaml
# presets/en/ce-executor-serial.yml
hats:
  - coordinator
  - executor
  - validator
  # ...
```

operator 看完只知道"有 10 个帽子",**不知道流程怎么走**。

**after**:

```yaml
hats:
  - coordinator
  - executor
  - validator
  # ...

mechanism:
  flow:
    type: declared
    steps: [unit_loop, review_walk, plan_end, ship]
  repair_budget: 3
  enforce_schema: hard
  state_idempotency: required
```

operator 看完**立刻知道流程、修复预算、契约强度**。

### 5.2 单次 run 失败时

**before**(报告 2026-06-26 实际发生):

```
19:08:24 executor  work.done
19:23:04 validator  test.passed (4/8 完成)
19:41:41 shipper    REVIEW_COMPLETE(fail) ← shipper 被强行激活
19:44:21 reporter   report.done(fail)
19:44:39 loop.terminate(reason=review_failed)

operator 看到的状态:
  - recovery.jsonl 28 条(反复翻烧饼)
  - drift.jsonl 5 条 finding(但 summary 报 0)
  - verdict_gate topic=report.done(看不懂为啥是这个 topic)
  - 完全不知道"iter=17 该 review.start 还是 plan.blocked"
```

**after**(同样输入,机制层修复后):

```
19:08:24 executor  work.done
19:23:04 validator  test.passed (4/8 完成)
19:23:05 [mechanism] coordinator step 4/8 未声明 on_partial → 
         flow_partial_state_undeclared REJECT(operator 立刻收到明确报错)

或者,operator 已在 preset 声明 on_partial:
19:23:05 [mechanism] coordinator emit plan.blocked(reason="4_of_8_incomplete_continue_to_review")
         (reason 必填,emit 通过硬契约)
19:23:06 shipper    REVIEW_COMPLETE(pass_with_residuals)
19:23:07 reporter   report.done(awaiting_decision=true)
19:23:08 LOOP_COMPLETE (verdict_gate 接受 pass_with_residuals)

operator 看到的状态:
  - recovery.jsonl 1-3 条(每个 task 最多 3 次,不会 28 条)
  - drift.jsonl 0 条(前置闸门拦住了)
  - verdict_gate topic=plan.blocked(语义清晰)
  - 立刻知道"preset 缺 on_partial 声明"(lint 给的明确指引)
```

### 5.3 worktree 复用时

**before**(报告 §2.6 legacy task):

```bash
$ git worktree add ../wt-reuse 2026-06-26-001
$ ralph run --worktree-name wt-reuse --plan ...
# 失败:TaskWrongLoop { actual_loop: None } × 7 iter
```

**after**:

```bash
$ git worktree add ../wt-reuse 2026-06-26-001
$ ralph run --worktree-name wt-reuse --plan ...
# 机制层检测到旧 version,自动 archive 老 task 到 .ralph/archive/{old_loop_id}/
# 新 loop 用新 version,无污染
# 如果 operator 想"复活老 task",显式 --migrate-legacy-tasks,触发 task.relocate_legacy
```

---

## 6. 成功标准

### 6.1 必须达成(MUST)

| ID | 标准 | 测量方式 |
|---|---|---|
| SC-1 | 报告 2026-06-26 命中的 95% 历史问题**全部不再发生** | 跑同样 plan(worktree 复用 + 4/8 半完成),看 recovery_count ≤ 3 且 drift_finding_count = 0 |
| SC-2 | 单次 run 在遇到"半完成状态"时,**不再绕过 review 流程** | iter=17 必须 emit `review.start` 或 `plan.blocked(reason non-empty)` |
| SC-3 | 修复类空转从"28 次"降到"≤ 3 次" | recovery.jsonl 中 `task.resume` retry 计数 ≤ `repair_budget`(默认 3) |
| SC-4 | 契约缺失字段在 emit 入口就被拦,drift 不再报"必填字段 0%" | drift.jsonl 中 `*_present in 0/1 events` 类型 finding = 0 |
| SC-5 | 状态文件计数与 summary 计数一致 | `diagnosis-summary.json` 的 `recovery_count` = `_final=true` 的 recovery 记录数 |
| SC-6 | worktree 复用时老 task 自动隔离,不污染新 loop | `worktree_reuse_state_isolation` integration test 通过 |

### 6.2 应当达成(SHOULD)

| ID | 标准 | 测量方式 |
|---|---|---|
| SC-7 | 单次 run wall time 比 baseline 减少 20-30% | 对比 2026-06-26 baseline(空转 ~2h / 总 ~5h) |
| SC-8 | 30 天内不再写"ce-executor-serial 第 N 次复发"的诊断报告 | 未来 30 天 monitor |
| SC-9 | 新 preset 上线周期从"周"缩短到"天"(只需写 mechanism metadata) | 主观指标 |

### 6.3 暂不量化(NEEDS MONITORING)

- drift_monitor 报警数从"每天 5 条"降到"每周 1 条"——**实际数字需要等机制层修复后才能统计**
- 整体 preset 数量是否增长——**不在本轮目标范围内**

---

## 7. 显式边界与不做清单

### 7.1 本轮**绝对不做**

- ❌ 改 hats 业务逻辑(切菜工还是切菜工)
- ❌ 改后端模型适配器
- ❌ 引入新 crate 依赖
- ❌ 改 TUI / Web Dashboard
- ❌ 改 `.ralph/` 目录结构
- ❌ 改 CLI 命令的 clap 定义(只新增 `repair_budget` 等 metadata 字段,不改语法)

### 7.2 本轮**做但不写进验收**

- 给 `flow_lifecycle.rs` 加更多注释和示例(已经存在但散乱)
- 给 `state/idempotent_log.rs` 加完整的 rustdoc
- 给 4 个新 lint 模块写"为什么需要这条 lint"的注释

### 7.3 留到下一轮(NEEDS DECISION)

- 是否把 `task.resume` 的 `target_hat` 字段升级为强校验(目前是软建议)
- 是否把 `loop_id` 升级为 UUID(目前是字符串,collision 风险)
- 是否引入"状态机可视化"工具(把 `_transitions[]` 渲染成 dot 图)

---

## 8. 风险与开放问题

### 8.1 技术风险

| 风险 | 触发条件 | 缓解 |
|---|---|---|
| 4 个药方互相干扰(contract reject 后 repair flow 拿不到事件) | 集成测试早期 | 4 个药方各自先有独立 BDD,再联调 |
| 幂等键设计错误导致并发写竞争 | 多 hat 并发 emit | 文件锁 + 测试用 mock 模拟并发 |
| 编译期 schema hash 校验让 dev 体验变差 | build.rs 报错信息不友好 | 写专门的"schema 怎么改"文档 |
| `progress-steward` 权限扩展破坏现有 preset | 现有 preset 没有 `repair_budget` 字段 | Default 3 + lint 警告但不禁用 |

### 8.2 流程风险

| 风险 | 触发条件 | 缓解 |
|---|---|---|
| 4 个药方一起做,任何一个失败全盘失败 | 任一药方未达 SC | 分阶段提交,每阶段独立 SC |
| 改 `execution_contract.rs` 引入回归 | legacy task 现在直接拒,改后回填 | 加专门的 regression test 覆盖 legacy 场景 |
| preset author 不理解 `mechanism.flow` 语法 | 新字段没人写过 | 写迁移指南 + 示例 preset |

### 8.3 开放问题(留到 plan 阶段)

1. **F1.4 的"coordinator 只能 emit 当前 step"是否过严?** — 当前 progress-steward 可以在 loop 任意 step emit `task.resume`,新约束可能破坏现有行为
2. **S2.3 的 archive 策略** — 老记录移到 `.ralph/archive/{old_loop_id}/`,这个目录清理策略由谁负责?(operator 手动?RLM 自动?)
3. **R4.4 的"只有 progress-steward 可以 emit 修复主题"** — 是否要给未来新 preset 留扩展点?

---

## 9. 验收清单

### 9.1 药方 1(声明式流转)

| 证据 | 行为 | 验证 |
|---|---|---|
| `crates/ralph-core/src/event_loop/flow_declaration.rs` 存在 | parse `mechanism.flow` → `FlowDeclaration` | unit test: `parse_minimal_flow` |
| `flow_partial_state_undeclared` lint 存在 | 4/8 未声明 on_partial → preset 加载失败 | integration: `partial_state_lint_blocks_load` |
| `flow_unknown_emit_rejected` gate 存在 | emit 不在 allowed_emits → runtime reject | BDD scenario: `flow_unknown_emit_rejected` |
| `presets/en/ce-executor-serial.yml` 加 `mechanism.flow` 段 | 4 个 step 完整声明 | snapshot test |

### 9.2 药方 2(幂等状态)

| 证据 | 行为 | 验证 |
|---|---|---|
| `crates/ralph-core/src/state/idempotent_log.rs` 存在 | 幂等键写入 + final 锁 | unit test: `idempotent_final_locked` |
| `worktree_reuse_state_isolation` 存在 | 新 loop version 隔离老记录 | integration test |
| `diagnosis_count_matches_final_state` BDD 存在 | summary 计数 = final 记录数 | BDD scenario |
| `state_missing_idempotency_key` lint 存在 | 旧格式 jsonl 不允许 | lint test |

### 9.3 药方 3(硬契约)

| 证据 | 行为 | 验证 |
|---|---|---|
| `enforce_schema_at_emit.rs` 存在 | 缺必填字段 → reject | unit test |
| `build.rs` 加 schema hash 比对 | schema 改了必须重新生成 | build script test |
| `drift_no_real_time_block` 重构存在 | drift 只审计不拦截 | refactor test |
| `plan_blocked_reason_required` BDD 存在 | plan.blocked 必须有 reason | BDD scenario |

### 9.4 药方 4(独立修复流程)

| 证据 | 行为 | 验证 |
|---|---|---|
| `repair_flow.rs` 存在 | 独立通道 + 状态机 + budget | unit test |
| `relocate_legacy_tasks` 函数存在 | legacy task 回填 loop_id | unit test: `relocate_legacy_tasks_idempotent` |
| `repair_budget_exhausted_blocks_plan` BDD 存在 | budget 用尽 → plan.blocked | BDD scenario |
| `repair_topic_authorization` lint 存在 | 只 progress-steward 可 emit 修复主题 | lint test |

### 9.5 端到端验收(必须全部通过)

- [ ] 跑报告 2026-06-26 同样的 plan(worktree 复用 + 4/8 半完成),recovery_count ≤ 3
- [ ] drift.jsonl 在跑完后 0 条 finding(前置闸门拦住)
- [ ] verdict_gate topic 与 shipper emit 主题一致(语义对齐)
- [ ] review 流水线 6 维全部正常激活
- [ ] `cargo nextest run -p ralph-cli -- preset_lint` 全绿(所有 builtin preset 通过 4 条新 lint)
- [ ] `cargo nextest run -p ralph-core --test scenarios` 全绿(新 BDD scenario 全过)

---

## 10. 附录:与已有解决方案文档的关系

本需求文档**建立在以下已有工作之上**,不是从零开始:

| 已有文档 | 已有贡献 | 本轮扩展 |
|---|---|---|
| `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` | 防线 A/B/C(review_terminal_coherence lint + record_review_terminal_observation + verdict_gate 双层 fail) | 本轮把同样的"三道防线"思路应用到 4 个药方全部 |
| `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` | progress-steward 概念 | 扩展为 4.4 节的修复主题权限 |
| `docs/solutions/developer-experience/wac-rollout-tiered-gates-2026-06-12.md` | tiered gates 思路 | 4.3 节前置闸门 + 事后审计 = 同模式 |
| `crates/ralph-core/src/event_loop/flow_lifecycle.rs`(已存在) | flow 框架骨架 | 4.1 节声明式流转的底座 |

**本轮不去覆盖**:`docs/solutions/integration-issues/ce-executor-serial-noble-peacock-*.md` 等单次事件修复(已被本轮药方 1 + 2 + 4 覆盖)。

---

**报告完成时间**:2026-06-27
**报告状态**:待用户确认 → 进入 ce-plan 阶段
**下一步**:ce-plan 把本需求拆分为可执行单元,建议先做药方 3(硬契约)+ 药方 4(独立修复)——这两个药方不依赖 preset 业务逻辑,改完能立刻看到机制层效果。
