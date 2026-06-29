---
title: ralph-runtime-recovery 运行时纠错引擎(覆盖原 ralph-tools-pitfalls 知识库方案)
type: feat
status: active
date: 2026-06-28
origin: docs/brainstorms/2026-06-28-ralph-core-data-agent-guide-refresh-requirements.md
supersedes: null
superseded_by: null
revised: 2026-06-29
revision_reason: 重新定位为运行时纠错引擎(非事后看报告的知识库);新增 4 套纠错判定函数 + 2 道防线
---

# ralph-runtime-recovery 运行时纠错引擎

## Overview

把原 plan(`ralph-tools-pitfalls` 失败模式知识库)重新定位为**运行时纠错引擎**——编译进 ralph 的代码与 prompt,作为内置纠错器运行:**不让人/agent 看报告**也能在错误发生当下识别 + 阻断 + 拉回正轨。

**核心定位**:
- ❌ **不是**"agent 调 `skill load ralph-tools-pitfalls` 去看 12 条 pitfall 描述"
- ❌ **不是**"事后查 `docs/report/172725-diagnosis.md` 找根因"
- ✅ **是**"agent 拿到 task.resume 时,prompt 顶部自动贴上反模式警告,agent 没意识到自己被纠错"
- ✅ **是**"runtime 跑 loop 时,纠错判定函数实时识别 4 类症状,自动阻断/转 plan.blocked 终态"

**为什么必须重写**:6-29 报告(`primary-20260628-172725`)显示 6-28 plan 的 12 条 pitfall 仅覆盖 5/9 类老错误(4 类 runtime 状态机错误完全未覆盖),且"沉淀知识库"是**事后视角**——agent 拿不到报告,也不会主动查。**纠错逻辑必须编译进 ralph 的 hot path**。

---

## Problem Frame

30 天第 11 次复发(`primary-20260628-172725`)+ 6-28 plan 起草后无任何 `pitfall` 关键字 commit → 暴露**两类**根本缺陷:

1. **预防缺失**:agent 收到 `task.resume(target_hat=executor, kind=missing_event_gate)` 时,prompt 里**完全没有反模式警告**,于是反复重发 `work.done` 8 次(报告 #5-11)/ 6 次(报告 #17-22),把 task_store 写脏
2. **纠错失能**:runtime 看到 stall_recovery 锚错(同 iter 出现 `stall_recovery:validator` + `missing_event_gate:executor`)时,**没有去重 + 共享 retry_key**,两条 envelope 各自跑 attempt 窗口,误判 `EscalationLevel::Final` → `RecoveryExhausted` 硬退出(不走 plan.blocked)

**结果**:每次同类错复发都要靠人工跑诊断报告(平均 30+ 分钟)→ 修 plan → commit(平均 1-2 天)→ 等下次复发再验证。本 plan 把这个 1-2 天的反应链路缩短为"**0 秒**"(runtime 实时纠错)。

---

## Two-Defense Architecture(两道防线)

```
                       ┌──────────────────────────┐
                       │   ralph-runtime-recovery │  ← 编译进 ralph
                       │   (本 plan 全部产物)     │
                       └──────────────────────────┘
                                  │
            ┌─────────────────────┴─────────────────────┐
            │                                           │
   ┌────────▼────────┐                         ┌────────▼────────┐
   │  Defense 1:     │                         │  Defense 2:     │
   │  PRE-FLIGHT     │                         │  IN-FLIGHT      │
   │  (prevention)   │                         │  (correction)   │
   │                 │                         │                 │
   │ agent 拿到      │                         │ runtime 跑      │
   │ task.resume 时  │                         │ loop 时,纠错    │
   │ prompt 顶部     │                         │ 判定函数实时    │
   │ 自动贴上        │                         │ 识别 4 类症状   │
   │ 反模式警告      │                         │ 自动阻断/转     │
   │                 │                         │ plan.blocked    │
   └─────────────────┘                         └─────────────────┘
            │                                           │
            ▼                                           ▼
   4 套反模式警告 markdown              4 套纠错判定函数 rust
   (注入到 prompt)                     (编译进 runtime hot path)
```

### Defense 1: PRE-FLIGHT(预防)

**位置**: `crates/ralph-core/data/ralph-tools-recovery-directives.md`(原 plan `ralph-tools-pitfalls.md` 改名)

**触发**: `task.resume` 事件入 events.jsonl 后的下一轮 iteration,`inject_memories_and_tools_skill` 路径消费

**机制**:
- `rejection.rs::build_task_resume_payload` 末尾追加 `recovery_directives: Vec<String>`(轻量 ID 列表,**不**含 markdown 内容)
- 注入路径按 ID 查 directives 全文,前置拼接到 prompt 顶部 `## RECOVERY DIRECTIVES` 段
- agent 不知道自己被注入纠错;但行为会被反向引导(不再重发/不再用空 task_id/不再写 task_store)

**4 条 directives(对应 4 类 runtime 症状的反面)**:
| ID | 反向引导 |
|---|---|
| `RD-EXECUTOR-RESEND-LIMIT` | 收到 task.resume(target=executor, kind=missing_event_gate)后,**最多重发 2 次** work.done,第 3 次改发 work.failed(reason="re-emit_exhausted") |
| `RD-TASK-ID-MUST-BE-LOOP-SCOPED` | 重发 work.done 前**必须**先读 `.ralph/agent/tasks.jsonl` 找 loop_scoped task_id;禁止 `""` / `from_key:...` 形态 |
| `RD-STALL-DETECT-AND-YIELD` | 看到 stall 30s 还没收到 test.passed,主动 yield + 改发 `human.guidance` 或 `loop.stalled`(不是无限重发) |
| `RD-PLAN-BLOCKED-ON-RECOVERY-EXHAUSTED` | 看到 `recovery_exhausted` 信号,直接发 `plan.blocked(reason="...")` 走终态,不再尝试重发 |

### Defense 2: IN-FLIGHT(纠错)

**位置**: `crates/ralph-core/src/recovery_runtime/`(新模块,4 套判定函数)

**触发**: runtime 在 iteration 关键节点(`post_stall_check` / `pre_decision` / `on_recovery_envelope` / `on_work_done_emit`)实时调用

**4 套纠错判定函数**:

| 判定函数 | 触发信号 | 纠错动作 |
|---|---|---|
| `dedupe_stall_recovery_with_missing_event_gate` | 同一 iteration 内 `recovery.jsonl` 同时存在 `stall_recovery:validator:*` 和 `missing_event_gate:executor:*` | (1) hard_gate 跳过二次检测 (2) 两条 retry_key 共享 attempt 窗口 (3) 不再误判 `EscalationLevel::Final` |
| `finalize_recovery_outcome_on_flapping` | 同一 retry_key 8 iter 内 `pending ↔ recovered ↔ repeated` 翻转 ≥ 3 次 | (1) 强制收敛到 `Final` (2) 同步发 `plan.blocked(reason=retry_key)` (3) 走 `REVIEW_COMPLETE(fail)` 终态 |
| `publish_loop_stalled_business_event` | `stall_recovery` 已写 envelope 但 `events.jsonl` 无 `loop.stalled` | (1) stall_recovery 路径**同步发布** `loop.stalled(reason)` 业务事件 (2) progress-steward 兜底链激活 |
| `block_executor_resend_storm` | 同 `task_key` 8 iter 内 `work.done` 计数 ≥ 3 且 `commit_count` 不变 | (1) hard_gate 注入 `ralph stop` (2) 强制转 `work.failed(reason="re-emit_exhausted")` (3) coordinator 走 plan.blocked |

**4 套函数的共同点**:
- 输入:**runtime 可见信号**(recovery.jsonl envelope / retry_key 翻转次数 / events.jsonl 事件流 / commit_count)
- 输出:**明确的纠错动作**(dedupe / finalize / publish / block)
- 不会让 agent 知道"自己被纠错了",只是 runtime 不再继续错下去

---

## Origin carry-forward(与 6-28 plan 的关系)

| 项 | 6-28 plan 处理 | 本 plan 处理 |
|---|---|---|
| F1 `task.resume` 修复 | 已纳入 R3: 解释字段 + 修复顺序 | **升级为 U3+U4**: payload 字段改名 `recovery_directives`(从知识 ID 列表升级为纠错指令 ID) |
| F2 emit vs wave 选择 | 已纳入 R7 场景导航 | 保持 |
| F3 task/memory/decision 管理 | 已纳入 R6 场景导航 | **升级为 U6**: 3 份 skill 改为"运行时纠错"语境(不是失败模式工作流) |
| F4 崩溃/恢复/诊断 | 已纳入 R7 场景导航 | 保持 |
| R1 行号引用审计 | 由 6-28 plan 负责 | 不在本 plan |
| R2 命令表/示例与 --help 对齐 | 由 6-28 plan 负责 | 不在本 plan |
| R7 场景导航 | 由 6-28 plan 负责 | **本 plan 在场景导航下补充** runtime-recovery 指针 |
| R12 check-cli-doc-drift / guard-prompt-size | 由 6-28 plan 负责 | **升级为 U7+U8**: 守门对象改为 runtime-recovery 模块(不是 skill markdown) |

**并存策略**:本 plan 与 6-28 plan 同时 active,本 plan 的 U1/U2/U3/U4 全面替代原 plan 的同名 IU;U5-U8 范围缩小(只守 runtime-recovery 自身,不覆盖其他 skill)。

---

## Scope Boundaries

### Inside

- 新增 1 份 `crates/ralph-core/data/ralph-tools-recovery-directives.md`(4 条 directive,每条 15-20 行,**只含反向引导,不含症状描述**)
- 按 `skill_registry.rs` 的 built-in 模式注册(`BUILT_IN_SKILLS` 数组追加 `ralph-tools-recovery-directives`)
- 新增 1 个 rust 模块 `crates/ralph-core/src/recovery_runtime/`(4 套判定函数 + 1 个 dispatcher)
- `crates/ralph-core/src/event_loop/rejection.rs` 的 `build_task_resume_payload` 末尾追加 `recovery_directives: Vec<String>` 字段(替代原 `recommended_pitfalls`)
- `crates/ralph-core/src/event_loop/mod.rs` 的 `inject_memories_and_tools_skill` 路径消费 `recovery_directives` 前置注入(原 plan U4 不动,只改字段名)
- 4 套纠错判定函数挂到 runtime hot path 4 个关键节点(`post_stall_check` / `pre_decision` / `on_recovery_envelope` / `on_work_done_emit`)
- 3 份现有 skill 文件(`ralph-tools-emit.md` / `ralph-tools-memories.md` / `ralph-tools-tasks.md`)末尾追加"运行时纠错行为指引"小节(30-50 行,**不**改主体内容)
- `scripts/check-cli-doc-drift.sh` 加 runtime-recovery 模块覆盖断言
- `scripts/guard-prompt-size.sh` 守 1 份新 skill markdown(单文件 ≤ 200 行)

### Outside (本次明确不做)

- **不**保留原 plan 的"知识库/事后视角"内容(skill 不再列 12 条 pitfall 描述,只列 4 条 directive)
- **不**修源码层 8 条"机制未闭环"根因(typed kind consumer 缺失 / shipper 二值化 / plan_gate 不豁免 fix-unit / U8/U11/U12 no-op / CLI emit 绕 stage_pipeline / completion correction 无 retry 上限 / agent 产物口径漂移)— 留给后续 fix plan
- **不**改 6-28 plan 已定的 R1-R12 范围
- **不**让 agent 主动 load runtime-recovery 全文(skill 是**强制注入**不是**按需加载**;agent 不会知道有这份 skill)
- **不**为 IDE/Claude Code 单独维护另一套 skill 文档
- **不**改 `recovery.jsonl` / `events.jsonl` schema(纠错函数消费现有信号,不新增字段)

### Deferred to Follow-Up Work

- 把 4 套纠错判定函数扩展到其他 rejection 阶段(当前只覆盖 stall_recovery / work_done / loop_stalled)
- runtime-recovery 模块的"自我终止阈值"参数化(当前硬编码:8 iter / 3 次翻转 / 3 次重发)
- `recovery_directives` 字段支持 hot reload(无需重启 ralph 即可更新 directive 内容)
- 4 套纠错函数覆盖到 CLI 路径(当前只走 event_loop 路径;CLI 直发场景留给 6-28-002 plan U11 闭合)

---

## Key Technical Decisions

1. **runtime-recovery 编译进 ralph,非纯 markdown skill**:
   - 原 plan U1(注册为 built-in skill)只承担 50% 任务——另一半是 4 套 rust 判定函数编译进 `recovery_runtime/` 模块
   - 两道防线必须**同时落地**:只有 markdown 没有 rust 判定函数 = 知识摆着,无纠错动作;只有 rust 没有 markdown = agent 反复重发再被硬阻断,体验差
2. **Defense 1 字段名 `recovery_directives` 而非 `recommended_pitfalls`**:
   - `pitfall` 是**事后视角**(人看完报告说"哦这是个 pitfall")
   - `directive` 是**事前视角**(runtime 给 agent 的行为指令)
   - 字段名换名 = 整个语义从"知识"变"指令"
3. **Defense 2 纠错判定函数**:
   - 不新增"纠错 orchestrator"统一调度(每套函数独立触发,各自挂到对应 hot path 节点)
   - 4 套函数**互不感知**:dedupe 只管 stall_recovery 去重;finalize 只管 outcome 收敛;publish 只管 loop.stalled 业务事件;block 只管重发风暴
   - 4 套函数**不读 recovery.jsonl 全部**——只看自己关心的 1-2 个 envelope 字段,降低耦合
4. **不修源码层根因,只在 runtime 层加判定**:
   - stall_recovery 锚错的真因在 `hard_gate.rs:912` 不知 work.done 已发(源码问题)— 本 plan 不动 hard_gate.rs,**只在 dedupe 函数里加"看到 stall_recovery 已存在就跳过 hard gate"**
   - recovery_exhausted 不走 plan.blocked 的真因在 `drift/engine.rs:392-406` 直接 return— 本 plan 不动 engine.rs,**只在 finalize 函数里加"outcome 翻转 ≥ 3 次时同步发 plan.blocked"**
5. **agent 不知道有 runtime-recovery skill**:
   - 注入的 `## RECOVERY DIRECTIVES` 段不带"反模式"标签(那是事后视角)
   - 改用"行为指引"措辞("收到 task.resume 后**最多**重发 2 次","**必须**先读 tasks.jsonl")
   - agent 看到的是"系统对这类操作的规范",不是"系统提示你别犯老错误"
6. **守门脚本守新模块,不是 6 份 skill**:
   - `check-cli-doc-drift.sh` 新增 `RECOVERY_RUNTIME_FUNCTIONS` 列表(4 套函数名),grep 验证函数在源码中存在
   - `guard-prompt-size.sh` 只守新 skill 1 份(不再扩展到 6 份)— 因为另外 5 份 skill 主体不动,只是末尾追加小节

---

## Risks & Dependencies

### Risks

- **R-1**:`build_task_resume_payload` 改名 `recovery_directives` 是 breaking 变更,可能影响其他 plan / BDD 场景对 `recommended_pitfalls` 字段的引用。**缓解**:U3 实施时先全仓 grep `recommended_pitfalls`,如果有引用点同步改;若无引用点(plan 未落地,确认无 caller),直接改字段名
- **R-2**:4 套纠错判定函数挂到 hot path 后,可能误判正常 case(例:`dedupe_stall_recovery_with_missing_event_gate` 在 `stall_recovery` 已写但 envelope schema 升级后字段名变了会漏判)。**缓解**:每套函数加 1 个 "schema 兼容性" 守门——读不到期望字段时**不纠错**(silent skip),不阻断
- **R-3**:runtime-recovery 模块的 hot path 调用增加单 iter latency。**缓解**:4 套函数总耗时预算 ≤ 5ms/iter(BTreeMap 查找 + 计数 + 一次事件 publish);不读 recovery.jsonl 全部,只读最近 8 行
- **R-4**:Defense 1 + Defense 2 同时落地时,可能存在指令冲突(agent 收到 `RD-EXECUTOR-RESEND-LIMIT`(重发 ≤ 2)但 runtime `block_executor_resend_storm` 也在 iter 3 阻断)。**缓解**:Defense 2 的 `block` 阈值(3 次)略大于 Defense 1 的 `limit`(2 次),agent 有 1 次 grace 重发机会;若 Defense 1 引导有效,Defense 2 永远不会触发
- **R-5**:Defense 2 注入 `plan.blocked` 业务事件会冲击现有 preset 的失败路径设计(preset 可能假设 plan.blocked 只来自 coordinator)。**缓解**:U2 实施时先 grep 所有 preset 对 `plan.blocked` 的 trigger 列表,把"runtime-recovery"也加入 trigger 名单(`plan.blocked` 由 ralph/runtime 也可发,不仅 coordinator)

### Dependencies

- **执行顺序**:`U1 (markdown 写 4 条 directive) → U2 (rust 4 套判定函数) → (U3 + U4 + U5 并行) → U6 (3 份 skill 节) → U7 + U8 (守门)`
- U1 必须先完成(U3 字段名 `recovery_directives` 引用 U1 的 directive ID 列表)
- U2 必须先完成(U5 4 套函数挂 hot path 需要函数已存在)
- U3 + U4 + U5 互相独立,可并行
- U6 依赖 U1 directive ID 稳定
- 依赖现有 `skill_registry.rs` 的 `include_str!` 注册机制(不需新增基础设施)
- 依赖现有 hot path 节点 4 个:`post_stall_check` / `pre_decision` / `on_recovery_envelope` / `on_work_done_emit`(均已存在,需确认注入点)
- 不依赖 preset 改动(U5 R-5 的 trigger 名单是 grep 后批量改,非新设计)

---

## High-Level Technical Design

### Component diagram

```
                              ┌──────────────────────────────┐
                              │  task.resume 事件入          │
                              │  events.jsonl 后,下一轮      │
                              │  iteration 触发              │
                              └──────────────────────────────┘
                                       │
            ┌──────────────────────────┴──────────────────────────┐
            │                                                     │
   Defense 1                                                    Defense 2
   (预防)                                                       (纠错)
            │                                                     │
   ┌────────▼────────┐                                  ┌──────────▼──────────┐
   │ rejection.rs    │                                  │ recovery_runtime/  │
   │ build_task_     │                                  │   - dedupe_stall_  │
   │ resume_payload  │                                  │     recovery_...   │
   │ 末尾追加        │                                  │   - finalize_      │
   │ recovery_       │                                  │     recovery_...   │
   │ directives[]    │                                  │   - publish_loop_  │
   └────────┬────────┘                                  │     stalled_...    │
            │                                           │   - block_executor_│
            ▼                                           │     resend_storm   │
   ┌────────────────┐                                   └──────────┬──────────┘
   │ events.jsonl   │                                              │
   │ task.resume    │                                              │ 挂到 hot path 4 节点
   │ 携带           │                                              │ post_stall_check
   │ directives     │                                              │ pre_decision
   └────────┬───────┘                                              │ on_recovery_envelope
            │                                                      │ on_work_done_emit
            ▼                                                      │
   ┌────────────────────┐                                          │
   │ inject_memories_   │                                          │
   │ and_tools_skill    │                                          │
   │ 消费 directives    │                                          │
   │ 拼 ## RECOVERY     │                                          │
   │ DIRECTIVES 段      │                                          │
   │ 到 prompt 顶部     │                                          │
   └────────┬───────────┘                                          │
            │                                                      │
            ▼                                                      │
   ┌────────────────┐                                              │
   │ agent 收到     │                                              │
   │ prompt 不自    │                                              │
   │ 知被纠错       │                                              │
   │ 行为被反向    │                                              │
   │ 引导(最多    │                                              │
   │ 重发 2 次)    │                                              │
   └────────────────┘                                              │
                                                                   ▼
                                              runtime 实时纠错(agent 不知道)
                                              dedupe / finalize / publish / block
```

### Runtime hot path injection points

| Hot path 节点 | 现有文件:行 | 挂载函数 | 触发时机 |
|---|---|---|---|
| `post_stall_check` | `crates/ralph-core/src/event_loop/mod.rs:6225` | `dedupe_stall_recovery_with_missing_event_gate` | stall_recovery 写完 envelope 后,hard_gate 二次检测前 |
| `pre_decision` | `crates/ralph-core/src/drift/engine.rs:392-406`(原 `check_termination_hint`) | `finalize_recovery_outcome_on_flapping` | runtime 决定是否触发 `RecoveryExhausted` 前 |
| `on_recovery_envelope` | `crates/ralph-core/src/event_loop/mod.rs:6080-6225` | `publish_loop_stalled_business_event` | stall_recovery 写 envelope 后,events.jsonl 提交前 |
| `on_work_done_emit` | `crates/ralph-cli/src/loop_runner/hard_gate.rs:912`(原 hard_gate 二次检测) | `block_executor_resend_storm` | executor 提交 work.done 时,hard_gate 校验 payload 后 |

---

## Implementation Units

### U1. 编写 runtime-recovery directives markdown(4 条 directive,只含行为引导)

**Goal**: 把 4 条 directive 沉淀进 `ralph-tools-recovery-directives.md`,每条 15-20 行,**只含反向引导,不含症状描述**(原 plan U2 的 12 条 pitfall 描述全部删除)。
**Requirements**: R-RR-1(directive 内容覆盖 4 条 runtime 症状)
**Dependencies**: 无
**Files**:
- `crates/ralph-core/data/ralph-tools-recovery-directives.md`(新建,≤ 200 行,精简版)
**Approach**:
- 4 条 directive ID 清单:
  | ID | 注入触发 |
  |----|----------|
  | `RD-EXECUTOR-RESEND-LIMIT` | `task.resume(target=executor, kind=missing_event_gate)` |
  | `RD-TASK-ID-MUST-BE-LOOP-SCOPED` | `task.resume(target=executor, kind=execution_contract:TaskWrongLoop)` |
  | `RD-STALL-DETECT-AND-YIELD` | `task.resume(target=executor, kind=stall_recovery)` |
  | `RD-PLAN-BLOCKED-ON-RECOVERY-EXHAUSTED` | `task.resume(target=executor, kind=recovery_exhausted)` |
- 每条 directive 一节,**结构**:触发条件(给 runtime 注入器看) + 行为规范(给 agent 看,正向引导) + 反例(给 agent 看,1-2 句不超 30 字)。**不**含症状描述/根因分析/历史 KB 引用(agent 不知道有报告)
- 末尾加「对应 runtime 判定函数」meta 段(给 reviewer 看,**不**注入 prompt)
**Test scenarios**:
- 单元: `wc -l ralph-tools-recovery-directives.md` ≤ 200
- 内容: 4 条 directive 标题(`## RD-N`)连续编号无跳号
- 静态: `rg -c "行为规范" directives.md` = 4
- 静态: `rg -c "症状" directives.md` = 0(确认无"症状"字样,纯正向引导)
- 静态: `rg -c "docs/report" directives.md` = 0(确认无历史报告引用)
**Verification**: `wc -l` 通过 + 4 条 `rg` 静态断言通过 + reviewer 通读无"症状"字样。

### U2. 新增 `recovery_runtime/` 模块(4 套判定函数 + 1 个 dispatcher)

**Goal**: 把 4 套纠错判定函数编译进 ralph hot path,作为 runtime 内置纠错器运行。
**Requirements**: R-RR-2(runtime 纠错判定覆盖 4 类症状)
**Dependencies**: 无
**Files**:
- `crates/ralph-core/src/recovery_runtime/mod.rs`(新建,dispatcher)
- `crates/ralph-core/src/recovery_runtime/dedupe_stall_recovery.rs`(新建)
- `crates/ralph-core/src/recovery_runtime/finalize_recovery_outcome.rs`(新建)
- `crates/ralph-core/src/recovery_runtime/publish_loop_stalled.rs`(新建)
- `crates/ralph-core/src/recovery_runtime/block_executor_resend.rs`(新建)
**Approach**:
- 每套函数**输入** = 1 个 `RuntimeContext` struct(recovery.jsonl 最近 8 行 / events.jsonl 最近 8 行 / 当前 retry_key 状态)
- 每套函数**输出** = `RecoveryAction` enum(Skip / DedupeEnvelope / PublishEvent { topic, payload } / InjectDirective { text } / ForcePlanBlocked { reason })
- dispatcher `mod.rs::dispatch(ctx: RuntimeContext) -> Vec<RecoveryAction>` 4 套函数各自独立调用,actions 合并返回
- 4 套函数**互不感知**,各自挂到 hot path 不同节点
**Patterns to follow**: 现有 `crates/ralph-core/src/diagnosis/responder.rs::classify` 的"输入上下文 + 输出 action"模式
**Test scenarios**:
- 单元: `dedupe_stall_recovery` 在 `recovery.jsonl` 同时存在两条 envelope 时返回 `DedupeEnvelope { drop: "missing_event_gate" }`
- 单元: `finalize_recovery_outcome` 在 retry_key 翻转 ≥ 3 次时返回 `ForcePlanBlocked { reason: "outcome_flapping" }`
- 单元: `publish_loop_stalled` 在 `events.jsonl` 无 `loop.stalled` 时返回 `PublishEvent { topic: "loop.stalled", payload: { reason: "stall_recovery" } }`
- 单元: `block_executor_resend_storm` 在 work.done ≥ 3 次且 commit_count 不变时返回 `InjectDirective { text: "ralph stop" }`
- Edge case: 4 套函数在缺字段时全部 `silent skip`(不抛错,不纠错)
- Integration: dispatcher 4 套函数并发调用,总耗时 ≤ 5ms
**Verification**: `cargo nextest run -p ralph-core -- recovery_runtime` 通过 + dispatcher 性能测试通过。

### U3. `build_task_resume_payload` 字段名 `recommended_pitfalls` → `recovery_directives`

**Goal**: 让 task.resume payload 末尾追加 `recovery_directives: Vec<String>`,只含 directive ID 列表(不含 markdown 内容)。
**Requirements**: R-RR-3(字段名从知识 ID 升级为指令 ID)
**Dependencies**: U1(需要 directive ID 列表)
**Files**:
- `crates/ralph-core/src/event_loop/rejection.rs`(修改 `build_task_resume_payload` / `enrich_task_resume_payload_full` / `enrich_task_resume_payload_with_stage`,**三个函数都同步加 `recovery_directives` 字段**)
- `crates/ralph-cli/src/loop_runner/hard_gate.rs`(两处手工 `insert` 段同步补 `recovery_directives: Vec::new()`)
**Approach**:
- `build_task_resume_payload` 末尾追加 `recovery_directives: directive_ids_for_kind(kind)`(新私有函数,按 `kind` 字段返回对应 directive ID)
- 不删除任何旧字段;`recovery_directives` 在末尾追加避免 schema 漂移
- 全仓 grep `recommended_pitfalls`,若有引用点同步改(plan 未落地,确认无 caller)
**Patterns to follow**: `rejection.rs:424-498` 已有的 payload 构造
**Test scenarios**:
- Happy path: `kind == "missing_event_gate"` → `recovery_directives` 含 `["RD-EXECUTOR-RESEND-LIMIT"]`
- Happy path: `kind == "execution_contract:TaskWrongLoop"` → `recovery_directives` 含 `["RD-TASK-ID-MUST-BE-LOOP-SCOPED"]`
- Edge case: `kind == None` → `recovery_directives` 为空列表
- 兼容性: 旧 caller 读取时不报缺字段错(JSON 序列化允许尾部新字段)
- 字段名: 全仓 `rg "recommended_pitfalls"` = 0(确认无残留)
**Verification**: `cargo nextest run -p ralph-core -- rejection` 通过 + `rg "recommended_pitfalls" .` = 0。

### U4. 注入路径消费 `recovery_directives` 前置注入(原 U4 不动,只改字段名)

**Goal**: `inject_memories_and_tools_skill` 路径识别 task.resume 后,把 `recovery_directives` 列表对应的 directive 段**前置注入**到 prompt 顶部。
**Requirements**: R-RR-4(前置注入机制)
**Dependencies**: U1, U2, U3
**Files**:
- `crates/ralph-core/src/event_loop/mod.rs`(修改 `inject_memories_and_tools_skill` 或新增 `inject_recovery_directives_for_resume` 入口)
- `crates/ralph-core/src/event_loop/tests/u*_wiring.rs`(新增测试)
**Approach**:
- 扫描 PENDING EVENTS 中的 `task.resume` envelope,提取 `recovery_directives`
- 按 ID 在 directives markdown 中定位对应节,前置拼接到 prompt 的 `## RECOVERY DIRECTIVES` 段
- 已消费的 envelope 标记 `consumed`(`std::mem::take`)
**Patterns to follow**: `inject_memories_and_tools_skill`(mod.rs:4479-4582)的"读 PENDING EVENTS → 拼 prompt"流程
**Test scenarios**:
- Happy path: 注入 `task.resume(recovery_directives=["RD-EXECUTOR-RESEND-LIMIT"])` 后,prompt 顶部出现 `## RECOVERY DIRECTIVES` 段
- Happy path: 多 ID 列表全部正确展开
- Edge case: `recovery_directives == []` 时不注入
- Edge case: `recovery_directives` 含未知 ID 时跳过该 ID 但不报错
- Integration: 同一 envelope 不被注入两次
- Prompt 顶部确认无"症状/根因/历史报告"等字样(只在 markdown 中,不被注入)
**Verification**: `cargo nextest run -p ralph-core -- inject_recovery_directives` 通过 + `rg "症状" injected_prompt.txt` = 0。

### U5. 4 套纠错函数挂到 runtime hot path 4 个节点

**Goal**: 把 U2 的 4 套判定函数挂到 hot path 4 个关键节点,让 runtime 实时纠错。
**Requirements**: R-RR-5(hot path 集成)
**Dependencies**: U2(函数已存在)
**Files**:
- `crates/ralph-core/src/event_loop/mod.rs:6225`(`post_stall_check` 节点,挂 `dedupe_stall_recovery`)
- `crates/ralph-core/src/drift/engine.rs:392-406`(`pre_decision` 节点,挂 `finalize_recovery_outcome`)
- `crates/ralph-core/src/event_loop/mod.rs:6080-6225`(`on_recovery_envelope` 节点,挂 `publish_loop_stalled`)
- `crates/ralph-cli/src/loop_runner/hard_gate.rs:912`(`on_work_done_emit` 节点,挂 `block_executor_resend_storm`)
**Approach**:
- 4 个节点各加 1 行 `recovery_runtime::dispatch(ctx)` 调用
- dispatcher 4 套函数并发执行,actions 合并后**按顺序应用**:
  1. `DedupeEnvelope` → 直接 drop envelope(不写 recovery.jsonl)
  2. `PublishEvent` → 写 events.jsonl
  3. `InjectDirective` → 改 hard_gate 注入的 guidance 文本
  4. `ForcePlanBlocked` → 写 events.jsonl + 转 coordinator
- 4 套函数**互不感知**,只通过 dispatcher 合并
**Patterns to follow**: 现有 `mod.rs:6225` 的 `stall_recovery` 写 envelope 后的硬编码处理
**Test scenarios**:
- Integration: 报告 `primary-20260628-172725` 复现 case → 跑 4 套函数 → 不再触发 `RecoveryExhausted`,转 `plan.blocked`
- Integration: 报告 #5-11 work.done 重发 → 第 3 次 hard_gate 注入 `ralph stop`
- Integration: report #26 task.resume 错配 → `dedupe_stall_recovery` 跳过 hard_gate 二次检测
- Performance: 4 套函数总耗时 ≤ 5ms/iter
**Verification**: 跑复现 case + 性能测试 + 回归 `cargo nextest run -p ralph-core -- recovery_envelope_u7_u8`。

### U6. 3 份现有 skill 末尾追加"运行时纠错行为指引"小节

**Goal**: 让 agent 在任何场景下都能看到"行为规范"(不只是 task.resume 触发时),把"行为规范"语料扩散到 emit/memories/tasks 3 份 skill。
**Requirements**: R-RR-6(行为规范扩散)
**Dependencies**: U1
**Files**:
- `crates/ralph-core/data/ralph-tools-emit.md`(末尾追加 "## 运行时行为规范" 小节,30-40 行)
- `crates/ralph-core/data/ralph-tools-memories.md`(末尾追加,30-40 行)
- `crates/ralph-core/data/ralph-tools-tasks.md`(末尾追加,30-40 行)
**Approach**:
- 每节用"**规范**"措辞("**必须**..."/"**禁止**..."),**不**用"反模式/症状"措辞
- 每节引用 1-2 个 `recovery_directives` ID(让 agent 知道"还有更细的规范在另一个 skill")
- 单文件增量 ≤ 50 行,改后不超 250 行
**Test scenarios**:
- 静态: `rg -c "## 运行时行为规范" ralph-tools-emit.md` ≥ 1
- 静态: `rg -c "反模式\|症状" ralph-tools-emit.md` = 0(确认小节无事后视角字样)
- 静态: 3 份 skill `wc -l` ≤ 250
**Verification**: `wc -l` 通过 + 静态断言通过。

### U7. `check-cli-doc-drift.sh` 加 runtime-recovery 模块覆盖

**Goal**: 在现有 flag 字面校验段之外,新增 `RECOVERY_RUNTIME_FUNCTIONS` 段,grep 验证 4 套函数在源码中存在。
**Requirements**: R-RR-7(模块覆盖守门)
**Dependencies**: U2
**Files**:
- `scripts/check-cli-doc-drift.sh`(新增 `RECOVERY_RUNTIME_FUNCTIONS` 数组 + 模块断言段)
**Approach**:
- 定义 `RECOVERY_RUNTIME_FUNCTIONS=("dedupe_stall_recovery_with_missing_event_gate" "finalize_recovery_outcome_on_flapping" "publish_loop_stalled_business_event" "block_executor_resend_storm")`
- 对每个函数名 grep `crates/ralph-core/src/recovery_runtime/`,若不存在则视为 drift
- 不影响现有 flag 校验段;两段并行运行
**Test scenarios**:
- Happy path: 4 套函数全部在源码中,脚本 exit 0
- Drift 检测: 手动把 `dedupe_stall_recovery_with_missing_event_gate` 改名为 `dedupe_stall_recovery`,跑脚本报该项缺失
**Verification**: 跑 `scripts/check-cli-doc-drift.sh` 现有行为不变 + 新段 2 条测试通过。

### U8. `guard-prompt-size.sh` 守 1 份新 skill markdown

**Goal**: 现有脚本守 `ralph-tools.md` 单文件 ≤ 200 行,本次新增 1 份 `ralph-tools-recovery-directives.md` ≤ 200 行守门。
**Requirements**: R-RR-8(size guard 1 份)
**Dependencies**: U1
**Files**:
- `scripts/guard-prompt-size.sh`(新增 1 份 skill 单文件检查)
**Approach**:
- 新增 `RECOVERY_DIRECTIVES_FILE="ralph-tools-recovery-directives"` 单独守 ≤ 200 行
- 不再扩展到 6 份 skill(原 plan U8 改为守 1 份,因为另外 5 份 skill 主体不动)
- 保留原 `ralph-tools.md ≤ 200` 检查
**Test scenarios**:
- Happy path: 新 skill 在阈值内,脚本 exit 0
- Drift 检测: 临时把 directives 加 50 行,脚本报该项超阈值
**Verification**: 跑 `scripts/guard-prompt-size.sh` 通过 + mock 测试通过。

---

## Verification Strategy(整体)

1. **静态守门**:`scripts/check-cli-doc-drift.sh` + `scripts/guard-prompt-size.sh` 两条均 pass。
2. **单元 + 集成**:`cargo nextest run -p ralph-core -- recovery_runtime rejection inject_recovery_directives` 全部通过。
3. **回归**:`./scripts/run-tests.sh`(nextest + doctest)无新 fail。
4. **复现验证**(关键):
   - 拿 `docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-diagnosis.md` 的 events.jsonl 跑 `recovery_runtime::dispatch` 单测,**断言**:
     - iter 4/9/12 stall_recovery 锚错时,`dedupe` 返回 `DedupeEnvelope`,hard_gate 跳过二次检测
     - iter 24-38 recovery_outcome_update 翻转 ≥ 3 次时,`finalize` 返回 `ForcePlanBlocked`,不再触发 `RecoveryExhausted`
     - iter 1-38 全程 `events.jsonl` 都同步有 `loop.stalled` 业务事件
     - iter 5-11 work.done 重发 ≥ 3 次时,`block` 返回 `InjectDirective { "ralph stop" }`
5. **端到端冒烟**(用单测替代 e2e):
   - 不起 `ce-executor-serial` preset 真实 loop
   - U2 的 dispatcher 单测已直接断言 4 套函数返回值
6. **反向验证**:用 `sed -n 'NN,MMp' <file>` 复核 directives 文档与 skill 文件里所有 `*.rs:NN` 引用范围未漂移
7. **行为规范守门**:全仓 `rg "症状\|反模式" ralph-tools-*.md` = 0(确认无事后视角字样泄漏到 agent 可见的 skill)

---

## Deferred Questions

- **DQ-1**:4 套纠错判定函数是否覆盖 CLI 路径?——否,留给 6-28-002 plan U11 闭合(CLI emit 已经在评估 stage_pipeline hook)
- **DQ-2**:`recovery_directives` 字段是否支持 hot reload?——否,留 follow-up
- **DQ-3**:runtime-recovery 模块的"自我终止阈值"(8 iter / 3 次翻转 / 3 次重发)是否参数化?——否,留 follow-up

---

## System-Wide Impact

| 角色 | 影响 |
|------|------|
| Loop 内 agent | 收到 task.resume 时 prompt 顶部多一段"运行时行为规范"(默认 100-300 token,正向引导,无事后视角字样);agent **不知道**有 runtime-recovery skill,只知道"系统对这类操作有规范" |
| 维护者 | 写新 directive 时手工同步;6-28 plan 的 R1-R12 维护流程不变;runtime-recovery 判定函数新增 1 个 mod + 4 个文件 |
| 现有 caller | `build_task_resume_payload` 末尾追加新字段 `recovery_directives`(从 `recommended_pitfalls` 改名),JSON 序列化兼容;4 套 hot path 节点各加 1 行 `dispatch(ctx)` 调用 |
| 守门脚本 | `check-cli-doc-drift.sh` 多 1 段模块覆盖;`guard-prompt-size.sh` 多 1 份 skill 检查;无破坏性 |
| 报告 | 报告**不受影响**——runtime-recovery 模块是 runtime 层,不动 events.jsonl / recovery.jsonl schema;但报告里的 9 类老错误 runtime 都会自动阻断(报告成为"事后验证材料",不再是"事前 debug 入口") |

---

## Key Differences from 6-28 plan(原 `ralph-tools-pitfalls`)

| 维度 | 原 plan(ralph-tools-pitfalls) | 本 plan(ralph-runtime-recovery) |
|------|-------------------------------|----------------------------------|
| **定位** | 失败模式知识库(agent 调 skill load 看 12 条 pitfall) | 运行时纠错引擎(两道防线编译进 ralph) |
| **视角** | 事后(症状 / 根因 / 历史 KB 引用) | 事前(行为规范) + 事中(纠错判定) |
| **触发方式** | 主动 load / 注入器按需 | 强制注入(task.resume 自动)+ 被动纠错(runtime hot path) |
| **U2 内容** | 12 条 pitfall 5 字段描述 | 4 条 directive 行为规范(只含触发条件 + 行为规范 + 反例) |
| **核心模块** | 无(纯 markdown) | 新增 `recovery_runtime/` 模块(4 套判定函数) |
| **Defense 数** | 1 道(注入) | 2 道(注入 + hot path 纠错) |
| **agent 可见性** | agent 主动 load 时可见 | agent 看到行为规范但不知道"被纠错"(反例措辞 ≤ 30 字) |
| **守门对象** | 6 份 skill markdown | 1 份新 skill + 1 个新 rust 模块 |
| **覆盖度(报告 9 类老错误)** | 5/9(50%) | **9/9(100%)** |
| **不修源码层根因** | ✓ | ✓(同) |

---

## Sources & Research

- **Origin**: `docs/brainstorms/2026-06-28-ralph-core-data-agent-guide-refresh-requirements.md`
- **6-28 plan**: `docs/plans/2026-06-28-001-feat-ralph-core-data-agent-guide-refresh-plan.md`(并行)
- **本次重写驱动**: `docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-diagnosis.md`(9 类老错误,4 类未被原 plan 覆盖)
- **历史诊断报告**(≥ 2 份复现 ≥ 8 条 pitfall):
  - `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md`
  - `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md`
  - `docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md`
  - `docs/report/2026-06-26-ce-executor-serial-5dim-and-profiles-loops-cancelled-and-review-failed-diagnosis.md`
  - `docs/report/2026-06-24-ce-executor-serial-primary-20260624-092856-diagnosis.md`
- **不再补充外部研究**:项目已有完整内部模式;runtime-recovery 是 runtime 层的 hot path 加固,无跨领域先例需求

---

## Appendix · IU 依赖图

```
U1 (markdown 写 4 条 directive) ──→ U3 (payload 字段 recovery_directives)
   │                                       │
   │                                  U4 (注入路径消费)
   │                                       │
   └───────────────────────────────→ U6 (3 份 skill 行为规范节)
                                           
U2 (4 套判定函数) ──→ U5 (挂 hot path 4 节点)

U3 ──→ U7 (drift 断言守 4 套函数)
U1 ──→ U8 (size guard 守 1 份 skill)
```

**建议执行顺序**:
1. `U1` (markdown)
2. `U2` (rust 4 套函数)
3. `U3 + U5 + U7 + U8` 并行
4. `U4` (注入路径)
5. `U6` 末位(需 U1 + U4 引用稳定)

---

## Provenance

- Generated by `ce-plan` skill (compound-engineering 3.11.2) on 2026-06-28
- Revised by user on 2026-06-29 from "knowledge base" framing to "runtime recovery engine" framing
- 5 critical sub-questions during revision: (1) 报告不给人看 (2) 编译进 ralph (3) 不犯错误或犯错后拉回 (4) 重新定位为运行时纠错引擎 (5) 同意直接更新开发计划
- Origin brainstorm authored same day by user
- Compiled from brainstorming design A-F phases and the Phase 0.7 scope synthesis approved by user

---

**Plan written to**: `docs/plans/2026-06-28-003-feat-ralph-tools-pitfalls-and-injection-hardening-plan.md`(本文件覆盖原 6-28 版本)
