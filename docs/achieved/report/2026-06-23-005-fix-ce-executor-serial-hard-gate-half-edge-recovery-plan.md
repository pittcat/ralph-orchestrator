---
title: "fix: ce-executor-serial hard_gate 半边修复复发闭环 + 4 类同类隐患预防(双胞胎扫描 + typed enum 防护 + TerminationTrigger SSOT + AuditSeverity SSOT + plan-gate 桥接)"
date: 2026-06-23
type: fix
plan_type: deep
status: active
loop_id: primary-20260623-095708
origin: 用户诊断会话(4 个 sub-agent 并行诊断)+ 机制层根因审查
prior_plan: docs/plans/2026-06-23-004-fix-ce-executor-serial-mechanism-close-loop-plan.md
supersedes: 004 plan 残留的 hard_gate typed kind 半边修复 + plan-gate 死信兜底 + scope_violation 阻断
supersedes_prevention: 30 天第 7 次复发的 4 类同类隐患预防
---

# fix: ce-executor-serial hard_gate 半边修复复发闭环

## Summary

`primary-20260623-095708` 复现了 `2026-06-23-004 plan` 的**半边修复复发**:004 plan 修了 `build_task_resume_payload`(rejection.rs:432)加 `kind` 字段,但对偶路径 `enrich_task_resume_payload_with_stage`(rejection.rs:662)未加;hard_gate.rs 调的是后者,导致 `task.resume` 注入路径的 `kind` 覆盖率 0/1,被 schema 拒 4 次后 stall_recovery 兜底失败,最终 `consecutive_failures >= 5` 触发 `loop.terminate`。

本 plan **双层修复**——**收尾本次复发 + 预防 4 类同类隐患**。

**收尾 3 条残留风险**:
1. **hard_gate typed kind wiring**: `enrich_task_resume_payload_with_stage` 加 `kind` 参数,hard_gate.rs / runner.rs 全路径补齐 typed kind
2. **typed dispatch 覆盖不全**: `CoordinatorDispatcher::dispatch` match 扩展 3 个新 kind,触发 `PlanBlocked` 死信兜底
3. **scope_violation 阻断**: dimension-reviewer 越权 Edit 仓内文件,从 WARN 升级为 `consecutive_failures += 1`,配合 P0-2 修复一并收敛

**预防 4 类同类隐患**(30 天第 7 次复发的根因,防止下次复发落在第 8 次同类模式):
4. **双胞胎函数清单扫描**(U0):扫描 codebase 所有「同一语义两条路径」的函数对,落地 SSOT 文件,根除「修了 A 路径忘了 B 路径」模式
5. **typed enum `#[non_exhaustive]` 防护**(U2 前置):验证并强制 `RejectionKind` 标 `#[non_exhaustive]`,match 必须显式 `..` 兜底,杜绝下次加 kind 时漏 match 臂
6. **TerminationTrigger SSOT**(U3 重构):`process_output` 把 3 个终止触发器(`consecutive_failures >= 5` / `pending_dead_letter` / `plan_complete`)抽成 typed enum,只 dispatch 不特例
7. **AuditSeverity SSOT**(U4 重构):所有审计函数(scope_violation + drift_monitor 3 类)统一走 `enum AuditSeverity { Warn, Fail, BlockLoop }`,本次 U4 是「首条从 Warn 升级到 Fail」的范本

**额外补 1 条根因**(诊断报告 P0-3):
8. **plan-gate 死信显式修复**(U5):`queue.advance` → plan-gate → executor 桥接事件补齐,plan-gate 全程 0 触发问题根治

## Problem Frame

### 核心问题

`primary-20260623-095708` run 在 step-01 re-walk 阶段死锁,3 个独立 P0 缺陷叠加形成致命环路:

```
P0-2 dim-reviewer 越权 Edit 3 个仓内文件
  → audit_file_modifications 只 WARN,不阻断
  → dim-reviewer 缺 publish obligation → HARD GATE 触发
  → engine 注入 task.resume(target=dim-reviewer, reason=missing_event)
  → P0-1 task.resume payload 缺 typed `kind` 字段
  → event_policy 拒收 4 次(kind 覆盖率 0/1)
  → P0-3 task.resume 死信未触发 PlanBlocked
  → 累积 consecutive_failures >= 5
  → loop.terminate(consecutive_failures)
```

3 个 P0 都不是新反模式,而是 `2026-06-23-004 plan` 落地时的**半边修复**:
- 004 plan U5 给 `build_task_resume_payload` 加 `kind`,但**对偶函数** `enrich_task_resume_payload_with_stage` 未加
- 004 plan U4 typed dispatch 表只覆盖 3 个 kind(Handoff* 三种),未覆盖 hard_gate / stall_recovery / missing_event_gate 三种来源的 kind
- 004 plan 未触及 `audit_file_modifications` 的阻断语义,scope_violation 仍只 WARN

### 范围边界

**本次修复**:
- **收尾 3 条本次复发残留**:U1 typed kind wiring / U2 typed dispatch 覆盖 / U4 scope_violation 阻断
- **预防 4 类同类隐患**(R10/R11/R12/R13):
  - U0 双胞胎函数清单扫描(R10)
  - U2 前置 `#[non_exhaustive]` 验证(R9 BLOCKER)
  - U3 TerminationTrigger SSOT 抽离(R11),过程_output 只 dispatch
  - U4 AuditSeverity SSOT(R12),scope_violation 是首例
- **补 1 条诊断 P0-3**:U5 plan-gate 死信根因修复(R13)
- 全部修改后,ce-executor-serial 跑通完整闭环 step-01 → step-02 → LOOP_COMPLETE

**不在本次范围**:
- 重新设计 typed dispatch 的 kind × count 阶梯(KTD-1 已有,本次只扩 match 臂)
- drift_monitor 3 类告警从 Warn 升级到 Fail(U9 后续 plan,本 plan 仅留 SSOT 接口)
- 整个 `ce-executor` 调度语义重写
- BDD scenario 整套重做(`2026-06-20-002 plan` 范围,U7)
- hat-channel routing serial preset 失效(U6 deferred P3)
- `cargo xtask doppelganger-check` CI 集成(U10 后续 plan)

## Requirements

每条实现单元都引用以下 R-IDs:

- **R1**: `enrich_task_resume_payload_with_stage` 必须接受 `kind: Option<RejectionKind>` 参数,在 obj["kind"] 上落 typed reason_code
- **R2**: **所有** task.resume 注入路径必须 100% 携带 typed kind,**未来新增 caller 必须遵守此约束**(P1-10 修复,消除「3 条 caller 已覆盖」隐式契约)
  - 本 plan 范围内已识别 3 条 caller(hard_gate / stall_recovery / contract)
  - 未来第 4+ 条 caller 必须遵循:`enrich_task_resume_payload_with_stage` 必传 `Some(RejectionKind)` 不留 None 兜底
  - 验证方式:`rg "enrich_task_resume_payload_with_stage\|build_task_resume_payload" crates/ --type rust` caller 数量必须 = 实际 caller 数量(实施 PR 描述粘贴 grep 输出)
  - `RejectionKind` 标 `#[non_exhaustive]` 强制未来新增 kind 时匹配臂必须扩展
- **R3**: `CoordinatorDispatcher::dispatch` match 必须覆盖 `MissingEventGate` / `StallNoEvents` / `ContractViolation` 三个新 kind,每个 kind 配置阶梯阈值
- **R4**: `process_output` 在 typed dead_letter 命中时(对应 kind 计数 ≥ 阶梯阈值),优先返 `TerminationReason::PlanBlocked(reason="task_resume_dead_letter")`,不增 `consecutive_failures`
- **R5**: `audit_file_modifications` 检测到 scope_violation(有 `disallowed_tools` 约束的 hat 改了仓内文件)时,触发 `consecutive_failures += 1`,与现有 WARN 双写
- **R6**: 全部修改后,`./scripts/run-tests.sh` 全基线 0 failed(允许 pre-existing flaky skip)
- **R7**: 全部修改后,ce-executor-serial 跑一个完整 step(plan-gate → shipper → reporter → LOOP_COMPLETE)验证闭环
- **R8**: 全部修改后,`recovery.jsonl` envelope 中 task.resume 事件必带 typed kind 字段,覆盖率 100%
- **R9 [机制层 BLOCKER]**: `RejectionKind` 必须标 `#[non_exhaustive]`,`CoordinatorDispatcher::dispatch` match 必须显式 `..` 兜底。U2 启动前置条件,不满足则 U2 不启动。
  - **强制约束**: `RejectionKind` 新增 variant 后,`CoordinatorDispatcher::dispatch` 必须编译通过(否则**不能 merge**)。验证方式采用 `trybuild::TestCases::compile_fail` 写 `tests/ui/non_exhaustive_match.rs`,故意构造一个遗漏 match 臂的 sample,验证编译失败信息含 "missing match arm"
  - **静态断言补充**: 用 `static_assertions::assert_not_impl_any!` 或 `const _: () = assert!(...)` 在编译期验证 `RejectionKind` 已标 `#[non_exhaustive]`(运行时检查注解是否存在)
- **R10 [预防]**: 落地 `crates/ralph-core/data/doppelganger-functions.md`,列出 codebase 所有「同一语义两条路径」的函数对(至少 5 对),作为本次修复的 SSOT 对账清单
  - **强制约束(P0-2 修复)**: 每个对偶函数修复 U(U1/U2/U3/U4/U5)在 **Approach** 步骤必须包含**显式 grep 命令**作为前置步骤:
    - `rg "fn (build|enrich|parse|compute|lint|audit)_<func_name>" crates/ --type rust` 列出 caller 路径
    - 实施 PR 描述必须粘贴 grep 完整输出
    - 任何 caller 遗漏(未在 SSOT 清单中标注) = **PR 拒绝 merge**
  - **CI 衔接**: 落地 xtask `cargo xtask doppelganger-check`(本 plan 不实施,留 U10),但本 plan 内 grep 必须人工执行
- **R11 [机制重构]**: `process_output` 的 3 个终止触发器(`consecutive_failures >= 5` / `pending_dead_letter` / `plan_complete`)必须抽成 `enum TerminationTrigger { Failure { count }, DeadLetter { reason }, PlanComplete { ... } }`,`process_output` 只 dispatch 不特例
- **R12 [机制 SSOT]**: 所有审计函数(scope_violation + drift_monitor 3 类)必须统一走 `enum AuditSeverity { Warn, Fail { add_failures: u32 }, BlockLoop { reason } }`,本次 U4 是「首条从 Warn 升级到 Fail」的范本,后续 drift 类按相同模式收敛
- **R13 [诊断 P0-3 根因]**: `queue.advance` → plan-gate → executor 桥接事件必须补齐,plan-gate 全程 0 触发的根因(plan-gate publishes 缺 `work.ready`)显式修复(参考 `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` Path A 方案)
- **R14 [实现细节]**: U3 终止 reason 必须用 typed enum 序列化(`TerminationReason::PlanBlocked { kind, source }`),禁止字面字符串拼接(`"plan.blocked:task_resume_dead_letter:<kind>"`)
- **R15 [持久化迁移,P0-3 修复]**: U3 删除 `LoopState::pending_dead_letter` 字段必须配套 schema version bump
  - `LoopState` 持久化路径:`.ralph/state.json` / `loop_state.jsonl`(如有) / 任何序列化点
  - `loop_state.rs` 加 `const LOOP_STATE_SCHEMA_VERSION: u32 = 2;`(v1 含 `pending_dead_letter`,v2 不含)
  - 反序列化时:若读到 schema_version=1 且有 `pending_dead_letter`,迁移为 `TerminationTrigger::DeadLetter` 入队,warn! 一行,然后持久化为 v2
  - `loop-termination-reason.json` 加 `schema_version` 字段(默认 1)
  - **回滚安全**: 新代码读旧 state 文件不 panic,旧代码读新 state 文件见到 schema_version=2 显式 warning
- **R16 [外部 caller 迁移清单,P0-3 修复]**: `CoordinatorDispatcher::dispatch` 返 PlanBlocked 时改 `push_termination_trigger` 而非 set `pending_dead_letter` 字段,必须列出**所有外部 caller**:
  - grep `crates/` 下所有 `CoordinatorDispatcher` 引用点
  - 任何外部 caller 仍读 `state.pending_dead_letter` 视为编译错误(字段删除)
  - 必须迁移到 `state.pop_termination_trigger()` / `state.push_termination_trigger()`
- **R17 [P0-4 修复]**: U5 plan-gate 补 `work.ready` 必须**唯一 emit**,不允许双重发布
  - grep 全 codebase `work.ready` emit 点,plan-gate 应是**唯一** emit 点
  - preset Linter / drift_monitor / 其它 hat 不允许 emit `work.ready`
  - events 文件中 step-N `work.ready` 出现次数必须 = 1(无双发)
  - serial preset consumer 是 **executor** hat(不是 isolated 的 coordinator,这点必须在 plan 内显式标注)

## Key Technical Decisions

### KTD-1: typed kind 注入统一走 `enrich_task_resume_payload_with_stage` 一条路径

**不**新增第三条注入路径,而是把现有的 `enrich_*` 升级为 typed 路径(对齐 `build_task_resume_payload` 的设计):
- 函数签名新增 `kind: Option<RejectionKind>` 参数
- obj["kind"] = kind.map(|k| json!(k.reason_code())).unwrap_or_else(|| json!(extract_reason_code(reason_hint)))
- 3 个 caller 全部传 Some(...):hard_gate 传 MissingEventGate,stall_recovery 传 StallNoEvents,contract 传 ContractViolation

理由:对齐 004 plan U5 的 `build_task_resume_payload` 设计,根除「两条路径只修一条」的半边修复。

### KTD-2: typed dispatch match 扩展保留阶梯阈值,新增 kind 用保守阈值

**不**给新 kind 用激进阈值,而是给 hard_gate 路径的 3 个新 kind 配保守阶梯:
- `MissingEventGate` count >= 2 → `PlanBlocked`(硬栅栏,防止 stall 持续 30+ min)
- `StallNoEvents` count >= 3 → `PlanBlocked`(匹配现有 HandoffIllegalEmitTopic 阈值)
- `ContractViolation` count >= 1 → `DriftFinding`(早期报警,但不上 plan.blocked,留给 contract 自愈机会)

理由:新 kind 首次落地用保守阈值,跑过 1-2 个真实 run 后根据实际 freq 调整(KTD-1 的 kind × count 阶梯表的迭代原则)。

### KTD-3: `process_output` 终止路径选择用 dead_letter flag 优先

**不**在 `process_output` 内部做 kind 计数判断,而是用 `LoopState::pending_dead_letter` flag 桥接:
- `CoordinatorDispatcher::dispatch` 返回 `PlanBlocked` 时,在 `LoopState` 上 set `pending_dead_letter = Some(reason_code)`
- `process_output` 在 success=false 时先检查 flag,若 set 则返 `TerminationReason::PlanBlocked` 而非 `consecutive_failures += 1`
- 退出 reason 落 `loop-termination-reason.json` 字面 `"plan.blocked:task_resume_dead_letter"`

理由:不污染 `process_output` 的既有 consecutive_failures 语义,只增加「dead_letter 优先」的一条短路。

### KTD-4: scope_violation 升级为 failure 但保留 WARN 审计

**不**取消 WARN(用户仍需可观测),而是在 WARN 双写的基础上额外 `consecutive_failures += 1`:
- `audit_file_modifications` 返回 `Some(Violation { hat, diff_files })` 时,同时 emit 现有 WARN 日志 + `state.consecutive_failures += 1`
- hat 越权一次 = 1 次失败,与 PTY 退出码非零同等计费
- 同一 hat 第二次越权即触发 `consecutive_failures >= 5` 终止(原本只能依赖 stall_detector 兜底)

理由:scope_violation 是确定性 bug(预设 `disallowed_tools` 已声明),不该仅 WARN。计为 failure 迫使 hat 收敛。

### KTD-5: 双胞胎函数 SSOT 清单 + 禁止新增双胞胎模式

**不**只修当前的对偶函数,而是落地 `crates/ralph-core/data/doppelganger-functions.md`:
- 全 codebase 扫描至少 5 对「同一语义两条路径」:`build_*` vs `enrich_*`、`parse_*` vs `compute_*`、`lint_*` vs runtime gate、`task_start` vs `task_resume` vs `work_resume`、`start_loop` vs `begin_iteration` 等
- 每对标注:**主路径**(SSOT)、**对偶路径**(已被废弃/即将对齐)、**对齐状态**(待修/已修)
- 实施任何 task.resume / handoff / emit 改动前,先 grep 该清单核对 caller 路径
- CI 加 `cargo xtask doppelganger-check`(暂未实施,本 plan 落地文件即可,CI 由后续 plan 接力)

理由:30 天第 7 次复发的根因就是「修了 A 路径忘了 B 路径」,SSOT 清单是治本。

### KTD-6: typed enum `#[non_exhaustive]` + match `..` 兜底强制

**不**依赖人工记忆 match 覆盖,而是用类型系统防护:
- `RejectionKind` enum 标 `#[non_exhaustive]`(004 plan KTD-5 已要求,本 plan 验证落地)
- 所有 match 显式补 `..` 或列出全部 variants
- `cargo build` 必须编译通过才能进 U2
- 新增 kind 时编译器强制提示修改所有 match 臂

理由:这是 R9 BLOCKER 的类型系统保障,不依赖开发者主动检查。

### KTD-7: TerminationTrigger typed enum + process_output 只 dispatch

**不**在 `process_output` 内部特例化处理 3 个终止触发器,而是抽 typed enum:
```rust
enum TerminationTrigger {
    Failure { consecutive_count: u32 },        // 原 consecutive_failures >= 5
    DeadLetter { kind: RejectionKind, source: DeadLetterSource },  // U3 新
    PlanComplete { plan_id: String },         // 现有,plan-gate 触发
}
```
- `process_output` 只 match `TerminationTrigger` 一次,每种 trigger 走独立分支
- 新增第 4 个终止条件时,只需扩 enum + 加 match 臂,不改 `process_output` 控制流
- `LoopState::pending_dead_letter` 字段移除(改用 `TerminationTrigger` typed enum 替代)

理由:这是 R11 的 SSOT 设计,治「process_output 多触发器散落」根因。

### KTD-8: AuditSeverity typed enum + 所有审计函数统一收敛(本次半边覆盖)

**不**让每个审计函数自行决定「WARN 还是 FAIL」,而是抽 typed enum:
```rust
enum AuditSeverity {
    Warn,                                           // 信息性告警,不影响退出
    Fail { add_failures: u32 },                     // 计为 failure(state.consecutive_failures += add_failures)
    BlockLoop { reason: String },                   // 立即触发 plan.blocked
}
```
- scope_violation(本次 U4):`AuditSeverity::Fail { add_failures: 1 }` ← 本次 plan 范围内
- drift_monitor 3 类告警(coord_join_rate / field_completeness / drift_unconsumed):**本次 plan 仅迁移接口为 `AuditSeverity::Warn`,不改 severity**(返回类型从原本的 `Option<DriftFinding>` 改为 `AuditSeverity::Warn`)。完整升级到 Fail 留 U9 后续 plan。
- audit 函数返 `(AuditSeverity, AuditContext)` 元组,统一走 `AuditDispatcher`

**关键澄清(P1-9 修复)**: 本次 plan 仅在「scope_violation 是首例从 Warn 升级到 Fail」的 SSOT 上落地;drift 类告警**不在本次升级 scope**。这是自觉的半边修复,不是声称"统一"。Deferred U9 接力完整迁移。

## Implementation Units

### U0. 双胞胎函数 SSOT 清单扫描(R10 预防,先于 U1-U5 启动)

**Goal**: 落地 `crates/ralph-core/data/doppelganger-functions.md`,列出 codebase 所有「同一语义两条路径」的函数对,作为本次 plan 的 sibling fix 预防基础,根除 30 天第 7 次复发根因「修了 A 路径忘了 B 路径」。

**Requirements**: R10

**Dependencies**: 无(本 plan 首个 U,先于所有修复类 U)

**Files**:
- `crates/ralph-core/data/doppelganger-functions.md` — 新增 SSOT 文档
- 扫描覆盖:`crates/ralph-core/src/event_loop/`、`crates/ralph-cli/src/loop_runner/`、`crates/ralph-core/src/hat_handoff/`

**Approach**:
1. grep `fn build_` + `fn enrich_` + `fn parse_` + `fn compute_` + `fn lint_` + `fn audit_` 等命名模式
2. 每对标注:函数 A / 函数 B / 主路径 / 对偶路径 / 对齐状态(待修 / 已修 / N/A)
3. 至少 5 对(目标 8-10 对):`build_task_resume_payload` vs `enrich_task_resume_payload_with_stage`、`parse_filename` vs `compute_filename`、`lint_emit` vs runtime gate、`task_start` vs `task_resume`、`start_loop` vs `begin_iteration`、`audit_file_modifications` vs `audit_scope_violation` 等
4. U1 实施前先 grep 该清单核对 caller 路径,确认无第 3 条对偶函数遗漏

**Test scenarios**:
- 文档存在 + 至少 5 对函数
- 每对都有 主路径 / 对偶路径 / 对齐状态 3 字段
- 至少 3 对对齐状态 = 待修(本次 plan 范围内)
- Integration: U1 实施前 grep 该清单确认无遗漏 caller 路径

**Verification**:
- 文件存在,`wc -l` ≥ 80 行
- 每对函数标注字段完整

**Execution note**: 本 U 必须先于 U1 完成(否则 U1 实施时无 SSOT 对账清单)。

---

### U1. enrich_task_resume_payload_with_stage 加 typed kind 参数

**Goal**: 对齐 004 plan U5 的 `build_task_resume_payload` 设计,让 `enrich_*` 这条 hard_gate 走的对偶路径也带 typed kind,从根上消除半边修复。

**Requirements**: R1, R2, R8

**Dependencies**: U0(必须先核对双胞胎清单确认无第 3 条 caller)

**Files**:
- `crates/ralph-core/src/event_loop/rejection.rs` — `enrich_task_resume_payload_with_stage` 函数签名加 `kind: Option<RejectionKind>`;obj["kind"] = kind.map(|k| json!(k.reason_code())).unwrap_or_else(...)
- `crates/ralph-cli/src/loop_runner/hard_gate.rs:769-794` — `inject_missing_event_hard_gate_guidance` 调 `enrich_*` 时传 `Some(RejectionKind::MissingEventGate)`;`inject_hard_gate_guidance` 传 `Some(RejectionKind::ContractViolation)`
- `crates/ralph-cli/src/loop_runner/runner.rs:5660` — stall_recovery 路径传 `Some(RejectionKind::StallNoEvents)`
- `crates/ralph-core/src/event_loop/tests/enrich_kind_wiring.rs` — 新增测试,3 条 caller 路径各 1 个 case

**Approach**:
1. 函数签名变更:`fn enrich_task_resume_payload_with_stage(free_form_message, reason_hint, target_hat, stage, kind: Option<RejectionKind>) -> String`
2. obj 构造:`obj["kind"] = match kind { Some(k) => json!(k.reason_code()), None => json!(extract_reason_code(reason_hint)) }`
3. 3 个 caller 全部传 Some(...),不留 `None` 兜底(老路径 0 caller,纯增量)

**Test scenarios**:
- Happy path: `enrich_*` 传 `Some(MissingEventGate)` → obj["kind"] = "missing_event_gate"
- Hard gate caller 路径: `inject_missing_event_hard_gate_guidance` 注入 task.resume 后,从 events 文件反序列化能读到 `kind="missing_event_gate"`
- Stall recovery caller 路径: `runner.rs:5660` 注入 task.resume 后,kind="stall_no_events"
- Contract caller 路径: `inject_hard_gate_guidance` 注入 task.resume 后,kind="contract_violation"
- Coverage: `recovery.jsonl` envelope 解析后,task.resume 事件的 kind 字段覆盖率 100%
- 集成: U0 清单核对无第 3 条 caller 路径(grep 双胞胎清单 + `grep -rn "enrich_task_resume_payload_with_stage" crates/`)

**Verification**:
- `cargo nextest run -p ralph-core -- enrich_kind_wiring` 全部 case 通过
- 跑 ce-executor-serial 完整 run,验证 `recovery.jsonl` 中 task.resume kind 字段非空

**Execution note**: 测试优先。先写 caller 路径的反向验证测试,再改函数签名。

---

### U2. CoordinatorDispatcher::dispatch match 扩展覆盖 3 个新 kind + #[non_exhaustive] 防护验证

**Goal**: 补齐 004 plan U4 typed dispatch 表,让 hard_gate / stall_recovery / contract 来源的 task.resume 死信有 typed 消费路径,触发 KTD-2 阶梯阈值。同时验证 `RejectionKind` 标 `#[non_exhaustive]`(R9 BLOCKER,本 U 启动前置)。

**Requirements**: R3, R8, **R9(机制层 BLOCKER)**

**Dependencies**: U1(typed kind 必须先到位,dispatcher 才能读),**Pre: R9 验证通过(RejectionKind 已标 #[non_exhaustive])**

**Files**:
- `crates/ralph-core/src/event_loop/rejection.rs` — `RejectionKind` enum 验证/补 `#[non_exhaustive]`(若未标)
- `crates/ralph-core/src/event_loop/rejection.rs:797-799` — `CoordinatorDispatcher::dispatch` match 扩展,新增 3 个 kind 臂,显式补 `..` 兜底
- `crates/ralph-core/src/event_loop/rejection.rs:741-749` — KTD-4 阈值表加入 3 个新 kind 的阶梯配置
- `crates/ralph-core/src/event_loop/tests/coordinator_dispatch_coverage.rs` — 新增测试,3 个新 kind × 阶梯阈值 × 1 个 fallback arm = 至少 7 个 case
- `crates/ralph-core/src/event_loop/tests/ui/non_exhaustive_match.rs` — **新增 trybuild `compile_fail` 测试**(P0-1 修复)。故意遗漏 `_ =>` 兜底臂,期望 stderr 含 "missing match arm" / "non-exhaustive patterns"
- `crates/ralph-core/src/event_loop/tests/rejection_kind_static_assert.rs` — **新增静态断言测试**,验证 `RejectionKind` 标 `#[non_exhaustive]`(用 `static_assertions` 在编译期检查)
- `crates/ralph-core/src/event_loop/tests/rejection_kind_non_exhaustive.rs` — 新增测试,验证编译期阻断(添加 variant 时强制修改所有 match)

**Approach**:
1. **Pre-check(BLOCKER)**: 先 `grep "pub enum RejectionKind" rejection.rs` + `grep "#\[non_exhaustive\]" rejection.rs`,若 `RejectionKind` 未标 `#[non_exhaustive]`,本 U 第一步补上,跑 `cargo build` 验证所有 match 编译通过
2. match 扩 3 臂:
   ```rust
   K::MissingEventGate => PlanBlocked { kind, count },    // 阶梯 count >= 2
   K::StallNoEvents => PlanBlocked { kind, count },      // 阶梯 count >= 3
   K::ContractViolation => DriftFinding { kind, count }, // 阶梯 count >= 1
   _ => PlanBlocked { kind, count },                     // 默认升级(必须显式,#[non_exhaustive] 强制)
   ```
3. KTD-4 阈值表常量(per preset dispatcher_config)
4. 兜底 `_` 臂保 PlanBlocked 不静默吞
5. 新增 `rejection_kind_non_exhaustive.rs` 测试:**mock 一个虚拟 variant**,验证编译期阻断(此测试用 `#[cfg(test)]` 临时取消 `#[non_exhaustive]` 注解,故意构造遗漏 match 触发编译失败)

**Test scenarios**:
- Happy path: `dispatch(MissingEventGate, count=2)` → 返 PlanBlocked
- Threshold boundary: `dispatch(MissingEventGate, count=1)` → 不返 PlanBlocked(走兜底,需在 caller 端加 dead_letter 兜底逻辑,本 U 不覆盖)
- StallNoEvents: `dispatch(StallNoEvents, count=3)` → 返 PlanBlocked
- ContractViolation: `dispatch(ContractViolation, count=1)` → 返 DriftFinding
- Unknown kind: `dispatch(FutureKind, count=10)` → 兜底返 PlanBlocked
- **#[non_exhaustive] 防护**: 用 trybuild 跑 `compile_fail` 测试,故意移除 `_` 兜底臂,验证 stderr 含 "missing match arm"(plan 不可只写普通单元测试;CI 必须跑通 trybuild suite)
- **静态断言**: 用 `static_assertions` 验证 `RejectionKind` 标 `#[non_exhaustive]`(防止后续有人删除注解)
- AE 关联:对应 004 plan 反模式 4 acceptance example(task.resume 死信兜底)

**Verification**:
- `cargo nextest run -p ralph-core -- coordinator_dispatch_coverage` 7 case 全过
- `cargo test -p ralph-core --test trybuild` 跑通(trybuild compile_fail 测试)
- `cargo nextest run -p ralph-core -- rejection_kind_static_assert` 静态断言全过
- 集成测试:模拟 2 次 missing_event_gate → 验证 dispatch 返 PlanBlocked

**Execution note**: **R9 BLOCKER 必须先验证通过**,否则本 U 不启动。测试优先。先写 match 扩展的纯函数测试,再接 caller。

---

### U3. TerminationTrigger typed enum 抽离 + process_output 只 dispatch

**Goal**: 治本修复「process_output 多触发器散落」根因。把 3 个终止触发器(`consecutive_failures >= 5` / `pending_dead_letter` / `plan_complete`)抽成 `enum TerminationTrigger`,`process_output` 只 dispatch 不特例。同时让 task.resume 死信触发 `PlanBlocked` 终止路径而非 `consecutive_failures`。

**Requirements**: R4, R7, **R11(机制重构)**, **R14(typed enum 序列化)**

**Dependencies**: U2(必须先有 `PlanBlocked` 决策点)

**Files**:
- `crates/ralph-core/src/event_loop/mod.rs` — 新增 `enum TerminationTrigger { Failure { consecutive_count: u32 }, DeadLetter { kind: RejectionKind, source: DeadLetterSource }, PlanComplete { plan_id: String } }`
- `crates/ralph-core/src/event_loop/mod.rs` — `process_output` 重构:从 3 个独立 if 分支改为 1 个 match `TerminationTrigger` 分支
- `crates/ralph-core/src/event_loop/loop_state.rs` — **`pending_dead_letter: Option<DeadLetterReason>` 字段移除**(改用 `TerminationTrigger` typed enum 替代,KTD-7 SSOT 设计)
- `crates/ralph-core/src/event_loop/rejection.rs` — `CoordinatorDispatcher::dispatch` 返 `PlanBlocked` 时,**不再写 `state.pending_dead_letter`**,而是 `state.push_termination_trigger(TerminationTrigger::DeadLetter { ... })`
- `crates/ralph-core/src/event_loop/termination.rs` — 新增模块,封装 `TerminationTrigger` enum + `TerminationReason` typed serialization(`serialize(reason: TerminationReason) -> String` 统一格式,禁止字面字符串拼接)
- `crates/ralph-core/src/event_loop/tests/termination_trigger_dispatch.rs` — 新增测试,3 种 trigger 各 1 case + 1 case 验证 process_output 单 match 分支
- `crates/ralph-core/src/event_loop/tests/plan_blocked_termination.rs` — 新增测试,3 个 dead_letter 触发路径

**Approach**:
1. **新模块 `termination.rs`**: 落地 `TerminationTrigger` enum + `TerminationReason` typed enum + `serialize` 函数(KTD-7 + R14)
2. **`process_output` 重构**(KTD-7 SSOT):
   ```rust
   fn process_output(state: &mut LoopState, success: bool) -> TerminationReason {
       if !success {
           if let Some(trigger) = state.pop_termination_trigger() {
               return trigger.into_termination_reason();  // typed conversion,无字面拼接
           }
           state.consecutive_failures += 1;
       }
       match state.pop_termination_trigger() {
           Some(trigger) => trigger.into_termination_reason(),
           None if state.consecutive_failures >= 5 => TerminationReason::Failure { count: 5 },
           None if state.plan_complete => TerminationReason::PlanComplete { ... },
           None => TerminationReason::Continue,
       }
   }
   ```
3. **移除** `LoopState::pending_dead_letter` 字段(单一 SSOT)
4. `loop-termination-reason.json` 落 typed serialization(`TerminationReason::serialize(&reason)`,而非字面字符串)
5. `CoordinatorDispatcher::dispatch` 返 `PlanBlocked` 时调 `state.push_termination_trigger(TerminationTrigger::DeadLetter { kind, source })`

**Test scenarios**:
- Happy path: 推 `TerminationTrigger::DeadLetter { MissingEventGate, source: HardGate }` → process_output 返 `TerminationReason::PlanBlocked`,consecutive_failures 不增
- Trigger queue semantics: process_output 推多个 trigger 后,按 FIFO pop
- Failure trigger: 推 `TerminationTrigger::Failure { count: 5 }` → 返 `TerminationReason::Failure`,loop 退出
- PlanComplete trigger: 推 `TerminationTrigger::PlanComplete { plan_id }` → 返 `TerminationReason::PlanComplete`
- No trigger: pop None → 走原 consecutive_failures += 1 路径
- **Typed serialization**: `TerminationReason::serialize(&PlanBlocked { kind: MissingEventGate })` → `"plan.blocked:task_resume_dead_letter:missing_event_gate"`(typed 拼接,不是字符串拼接)
- **队列容量上限(P1-6 修复)**: `TerminationTrigger` 队列默认上限 16。push 第 17 个时触发 critical alert + 立即 force terminate(防止 OOM)
- **overflow 测试**: 推 17 个 trigger,验证第 17 个触发 critical alert + process_output 返 `TerminationReason::QueueOverflow`
- **持久化 migration(P0-3 修复)**: 故意写入 v1 schema state(含 `pending_dead_letter`),验证新代码读时不 panic + 迁移为 `TerminationTrigger::DeadLetter` 入队
- **回滚兼容(P0-3 修复)**: 故意在新代码写入 v2 schema state,验证旧代码读时显式 warning + 不 panic
- 真实 run 验证: ce-executor-serial 跑 missing_event_gate 路径,loop-termination-reason.json 出现 typed serialized 字符串

**Verification**:
- `cargo nextest run -p ralph-core -- termination_trigger_dispatch` 全部 case 通过(含 overflow 测试)
- `cargo nextest run -p ralph-core -- plan_blocked_termination` 全部 case 通过
- **migration 测试**: `cargo nextest run -p ralph-core -- loop_state_schema_migration` 全部 case 通过
- 集成测试:模拟 hard_gate 触发 → dispatch 返 PlanBlocked → process_output 终止路径为 PlanBlocked
- **Grep 验证(P1-7 修复)**: `rg "pending_dead_letter" crates/ docs/ tests/ --type rust --type markdown` 应**0 命中**(含 docs 和 tests)
- PR 描述必须粘贴 grep 完整输出作为 check item

**Execution note**: 测试优先。state 字段设计是 typed enum,后续加 kind 时编译器会强制提示修改 dispatch 路径。**先删 `pending_dead_letter` 字段,再写 trigger 队列测试**——防止新旧两套状态同时存在的混合态。

---

### U4. AuditSeverity typed enum SSOT + scope_violation 升级为 Fail

**Goal**: 治本修复「audit 阻断职责不分离」根因。引入 `enum AuditSeverity { Warn, Fail, BlockLoop }` SSOT,所有审计函数统一走该 SSOT。本次 scope_violation 是「首条从 Warn 升级到 Fail」的范本,后续 drift 类按相同模式收敛(本 plan 不升级 drift,留待后续 plan)。

**Requirements**: R5, R7, **R12(机制 SSOT)**

**Dependencies**: 无

**Files**:
- `crates/ralph-core/src/event_loop/audit.rs` — 新增模块,落地 `AuditSeverity` typed enum + `AuditDispatcher` 统一入口
- `crates/ralph-core/src/event_loop/mod.rs:5905-5907` — `audit_file_modifications` 重构:返回 `(AuditSeverity, AuditContext)` 元组,统一走 `AuditDispatcher::dispatch`
- `crates/ralph-core/src/event_loop/tests/audit_severity_ssot.rs` — 新增测试,scope_violation 走 Fail 路径 + 未来 drift 类可接入
- `crates/ralph-core/src/event_loop/tests/scope_violation_failure.rs` — 新增测试
- `presets/en/ce-executor-serial.yml:1157` — `disallowed_tools: ["Edit"]` 扩到 `["Edit", "Write"]`(dim-reviewer 只能写到 scratchpad 子目录,不应在仓内路径用 Write)

**Approach**:
1. **新模块 `audit.rs`**: 落地 `AuditSeverity` typed enum(KTD-8):
   ```rust
   pub enum AuditSeverity {
       Warn,
       Fail { add_failures: u32 },
       BlockLoop { reason: String },
   }
   pub struct AuditContext { pub hat: HatId, pub kind: RejectionKind, pub details: String }
   pub struct AuditDispatcher;
   impl AuditDispatcher {
       pub fn dispatch(state: &mut LoopState, severity: AuditSeverity, ctx: AuditContext) {
           match severity {
               AuditSeverity::Warn => warn!(...),                  // 仅日志
               AuditSeverity::Fail { add_failures } => {
                   state.consecutive_failures += add_failures;     // 计为失败
                   warn!(...);                                     // 仍双写日志
               },
               AuditSeverity::BlockLoop { reason } => {
                   state.push_termination_trigger(TerminationTrigger::BlockLoop { reason });  // 走 U3 typed trigger
               },
           }
       }
   }
   ```
2. `audit_file_modifications` 重构:返 `(AuditSeverity::Fail { add_failures: 1 }, AuditContext { hat, kind: ScopeViolation, details: diff_files })`
3. preset `disallowed_tools` 扩到 Write
4. **drift_monitor 3 类告警**(本次不升级,留接口):改返 `AuditSeverity::Warn`,由 `AuditDispatcher` 统一处理——半边收敛,完整迁移留后续 plan

**Test scenarios**:
- Happy path: dim-reviewer 改 1 个仓内文件 → AuditDispatcher::dispatch 触发 Fail,state.consecutive_failures = 1
- Multiple violations: dim-reviewer 改 3 个仓内文件 → AuditSeverity::Fail { add_failures: 1 }(单次 activation 内聚合)
- Mixed Edit/Write: dim-reviewer 改 1 Edit + 1 Write → 同样计 1 次
- No violation: dim-reviewer 只读 → state.consecutive_failures 不变
- **scratchpad 排除(P1-5 修复)**: dim-reviewer 写 `/scratchpad/dim-reviewer/report.md`(合法 scratchpad 报告)→ state.consecutive_failures = **0**(必须显式排除)
- **路径过滤前置 check(P1-5 修复)**: 实施前先 grep `crates/ralph-core/src/event_loop/audit.rs` 确认路径过滤已实现,否则 plan 不启动
- **SSOT 统一**: drift_monitor 改返 `AuditSeverity::Warn` 后,AuditDispatcher 走 Warn 分支
- **BlockLoop severity 预留测试**: mock 一个 `AuditSeverity::BlockLoop` 触发,验证 process_output 走 BlockLoop trigger
- **activation 边界(P2-16 修复)**: "单次 activation 内聚合" 必须显式定义 activation 边界(`hat_activate()` 调用周期 / `loop_iteration` 边界),测试覆盖跨 activation 计数
- Integration: 跑 ce-executor-serial 一轮 dim-reviewer 越权 → loop-termination-reason.json 出现 `"consecutive_failures"` 或 `"plan.blocked"`,而非 8h+ stall

**Verification**:
- `cargo nextest run -p ralph-core -- audit_severity_ssot` 全部 case 通过
- `cargo nextest run -p ralph-core -- scope_violation_failure` 全部 case 通过(包含 scratchpad 排除 case)
- **路径过滤 grep**:`rg "scratchpad" crates/ralph-core/src/event_loop/audit.rs` 应命中路径排除逻辑
- 集成测试:mock dim-reviewer 越权 → 验证 consecutive_failures 计数正确

**Execution note**: 测试优先。改 audit_file_modifications 是关键风险点,先写反向验证。**drift_monitor 的 SSOT 收敛是 half-baked 改进,标记「首条」后续 plan 接力**。

---

### U5. plan-gate 死信根因修复(R13 诊断 P0-3,补 004 plan 漏掉的根因)

**Goal**: 治本修复诊断报告 P0-3(plan-gate 全程 0 触发)。补齐 `queue.advance` → plan-gate → executor 桥接事件,plan-gate publishes 必须包含 `work.ready` 才能在 `queue.advance` 后被正确触发(参考 `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` Path A 方案)。**重点防双重发布**(P0-4 修复)。

**Requirements**: R13, **R17(P0-4 防双重发布)**

**Dependencies**: 无(独立于 U1-U4)

**Files**:
- `presets/en/ce-executor-serial.yml` — `plan-gate` hat 的 `publishes` 字段加 `work.ready`(与 isolated preset Path A 一致)
- `crates/ralph-core/src/event_loop/mod.rs` — `plan-gate` subscription / emit 链路验证:`queue.advance` 事件触发后必须 emit `work.ready` 到 executor
- `crates/ralph-core/src/event_loop/tests/plan_gate_bridge.rs` — 新增测试,模拟 plan-gate emit 链路完整性
- `crates/ralph-core/src/event_loop/tests/plan_gate_unique_emit.rs` — **新增防双重发布测试**(P0-4 修复):grep `work.ready` emit 唯一性

**Approach**:

1. **Trigger / Consumer 链路表(P0-4 修复)**:

   | preset | plan-gate subscribes | plan-gate publishes | work.ready consumer |
   |---|---|---|---|
   | isolated (`ce-executor-isolated.yml`) | `review.passed`, `review.complete`, `work.failed`, `fix.exhausted`, `debug.exhausted`, `loop.cancel`, `queue.advance` | `queue.advance`, `plan.complete`, `plan.blocked`, `work.ready` | **coordinator** hat |
   | serial (`ce-executor-serial.yml`) | `review.passed`, `review.complete`, `work.failed`, `fix.exhausted`, `debug.exhausted`, `loop.cancel`, `queue.advance` | `queue.advance`, `plan.complete`, `plan.blocked` ← 本 U 补 `work.ready` | **executor** hat ← 与 isolated **不同** |

   **关键差异**: serial preset 的 `work.ready` consumer 是 **executor**(不是 coordinator)。若实施者误以为和 isolated 一样走 coordinator,work.ready 死信。

2. **防双重发布 grep 验证(P0-4 修复)**:
   - 实施前置步骤: `rg "emit.*work\.ready\|publish.*work\.ready\|work_ready" crates/ presets/ --type rust` 列出所有 emit 点
   - 本 U 完成后 grep 结果应只有 plan-gate 一个 emit 点
   - **任何其它 hat emit `work.ready` = 编译错误**(preset Linter 校验)

3. **读取 `presets/en/ce-executor-serial.yml`**,确认 `plan-gate` 的 `publishes` 字段当前只有 `[queue.advance, plan.complete, plan.blocked]`
4. 补 `work.ready`:`publishes: [queue.advance, plan.complete, plan.blocked, work.ready]`
5. **HARD RULE 文档化**: plan-gate 只在 `queue.advance` 后 emit `work.ready`,禁止在其他场景发
6. plan-gate 的 triggers 必须包含 `queue.advance`(已有,验证不漏)
7. **serial vs isolated consumer 差异**:在 commit message 显式标注 "serial preset consumer = executor, NOT coordinator (different from isolated Path A)"
8. 加测试:
   - 模拟 plan-gate emit 链路,验证 `queue.advance` 后立即有 `work.ready` 落地(模拟 run 验证 plan-gate 真实触发,而非仅 listener 注册)
   - **防双重发布**:grep events 文件中 `work.ready` 出现次数 = 1
   - **consumer 验证**:executor hat 接到 `work.ready` 后真正激活(不是 coordinator)

**Test scenarios**:
- plan-gate publishes 字段包含 work.ready(单元测试读 yaml)
- 模拟 run: review.passed → queue.advance → work.ready 全链路触发,executor 接到 work.ready(不靠 task.resume 兜底)
- 真实 run 验证: ce-executor-serial 跑完整 step-01 + step-02,验证 events 文件中出现 `work.ready(step-02)`(而非仅 task.resume 兜底)
- 防回归: plan-gate 不在 review.passed 之外场景发 work.ready(如 plan.complete 直接发 work.ready 应被拒)
- **防双重发布**: events 文件中 `work.ready` 出现次数 = 1(grep `work.ready` | wc -l)
- **consumer 正确性**: events 文件中 `work.ready` 后的下一个 active-activations.json entry 是 `hat_id: executor`(不是 coordinator)
- **preset Linter**: preset LintRunner 校验 plan-gate publishes 包含 work.ready(自动测试)

**Verification**:
- `cargo nextest run -p ralph-core -- plan_gate_bridge` 全部 case 通过
- `cargo nextest run -p ralph-core -- plan_gate_unique_emit` 全部 case 通过
- 集成测试:完整跑 ce-executor-serial 一个 plan(U1 + U2),events 文件出现 step-02 work.ready,**且 grep `work.ready` 出现次数 = 1**
- `rg "work\.ready" crates/ presets/ --type rust` 输出唯一 emit 点(plan-gate)

**Execution note**: 与 U1-U4 并行可启动。直接读 isolated preset Path A 已落地方案,**移植时务必注意 serial preset consumer 是 executor 而非 coordinator**。PR 描述必须包含 trigger/consumer 链路表 + 防双重发布 grep 输出。

---

## System-Wide Impact

**受影响方**:
- **loops 执行者**: ce-executor-serial 跑 30 天 7 次复发后,本次闭环应让典型 run 跑通完整 U1 → LOOP_COMPLETE
- **preset 维护者**: `ce-executor-serial.yml` 需更新 `disallowed_tools`,触发 ad-hoc lint check
- **agent 实现者**: dimension-reviewer / dimension-reviewer agent 提示词需提醒「严格遵守 `disallowed_tools`,改仓内文件即终止」
- **测试维护者**: 5066 baseline 需补 4 个新测试文件,本 plan 整体不破 baseline

**已影响方**:
- 无(本次是 004 plan 的迭代修复,无外部 API / CLI 改动)

**未影响方**:
- 其它 preset(autoresearch / ce-executor-lite / debug / merge-loop)无 hard_gate 路径,不受 typed kind 改动影响
- BDD scenarios(2026-06-20-002 plan 范围)
- hat-channel routing serial preset 失效(U9 deferred P3,本 plan 不动)

## Risks & Dependencies

### 风险

- **R-A**: U3 加 `pending_dead_letter` 字段,若与既有 `consecutive_failures` 终止判定有竞态,可能误终止。缓解:U3 重构为 `TerminationTrigger` typed enum 队列,移除 `pending_dead_letter` 字段(KTD-7 SSOT),用 FIFO 队列避免竞态。
- **R-B**: U4 `disallowed_tools: ["Edit", "Write"]` 扩到 Write,可能误伤合法 scratchpad 写。缓解:scope_violation 检测只看仓内路径(`/repo/`),scratchpad 路径(`/scratchpad/`)不在 audit 范围;OQ-2 决议已说明。
- **R-C**: U2 dispatch 兜底 `_` 臂返 PlanBlocked,可能过度升级。缓解:`#[non_exhaustive]` 强制显式兜底,首次落地跑 1-2 个真实 run 后根据实际 freq 收紧。
- **R-D [新增]**: U3 重构 process_output 删除 `pending_dead_letter` 字段,可能与既有 caller 不兼容。缓解:grep 全 codebase 调用点,一次性迁移到 `push_termination_trigger` / `pop_termination_trigger` API;遗留调用视为编译错误。
- **R-E [新增]**: U4 引入 `AuditSeverity` SSOT,scope_violation 升级为 Fail,可能影响 BDD scenarios 既有断言(断言 "scope_violation 后 loop 继续运行")。缓解:BDD scenarios 单独跑,若失败则更新断言为 "scope_violation 后 loop 计 1 次失败"。
- **R-F [新增]**: U5 plan-gate publishes 补 `work.ready`,可能与其他 preset 不兼容。缓解:仅改 ce-executor-serial.yml,其它 preset 暂不动;若 preset 间有 SSOT 化需求,留后续 plan。

### 依赖

- **D-1**: 004 plan commit `230bbbff` 之前的 baseline(typed kind 类型已存在,本 plan 只补 caller 路径)
- **D-2**: `RejectionKind` enum 已含 `MissingEventGate` / `StallNoEvents` / `ContractViolation` 三个 variant(由 004 plan KTD-5 + 本 plan KTD-1 联合落地)
- **D-3**: 5066 unit baseline(`./scripts/run-tests.sh` 通过,本 plan 加 7+ 个新测试文件不破 baseline)
- **D-4 [新增,U2 前置]**: `RejectionKind` 必须已标 `#[non_exhaustive]`(004 plan KTD-5 落地验证)。若未标,U2 启动第一步补上
- **D-5 [新增,U3 前置]**: U2 必须已完成(match 扩 3 臂 + 兜底,plan-blocked 决策点到位)

### 兼容性评估(P0-3 修复显式回应)

**API 变更清单**:
- `enrich_task_resume_payload_with_stage` 签名变(`+ kind: Option<RejectionKind>`)—— 公开函数,**编译期阻断所有 caller**
- `LoopState::pending_dead_letter` 字段删除 —— 状态 schema 变化,**配套 v1 → v2 migration(R15)**
- `loop-termination-reason.json` 格式变 —— 持久化文件,**加 `schema_version` 字段(R15)**
- `audit_file_modifications` 返回值变(`Option<Violation>` → `(AuditSeverity, AuditContext)`)—— 公开 API
- `CoordinatorDispatcher` 内部行为变 —— 配套 R16 外部 caller 迁移清单

**数据格式迁移路径(R15)**:
- `LoopState` schema_version: v1(含 `pending_dead_letter`) → v2(只有 trigger queue)
- 反序列化 migration:v1 → 自动转 `TerminationTrigger::DeadLetter` 入队,warn! 一行后持久化 v2
- `loop-termination-reason.json` schema_version 字段(默认 1)

**回滚安全**:
- ⚠️ **本 plan 完成后,新旧代码不可直接互换**——需配套回滚脚本
- 新代码读旧 v1 state:不 panic,自动迁移
- 旧代码读新 v2 state:见到 `schema_version: 2` warning,不 panic 但建议升级
- **回滚步骤**: revert commit + 旧 schema 字段仍在 `.ralph/state.json`(因为 v2 写盘兼容 v1 reader)

**依赖变更**:
- 新增 3 个模块:`data/doppelganger-functions.md` + `event_loop/termination.rs` + `event_loop/audit.rs`(API 见下方"新增模块对外 API 声明")
- `lib.rs` 注册:`pub mod termination; pub mod audit;`
- preset Linter 可能需要更新 preset 校验规则(U5 plan-gate publishes 字段变)

### 持久化 schema 迁移详细设计(R15,P0-3 修复)

**LoopState schema v1 → v2 迁移流程**:

```rust
// loop_state.rs
pub const LOOP_STATE_SCHEMA_VERSION: u32 = 2;

impl LoopState {
    pub fn deserialize_v1(v1_state: V1LoopState) -> Self {
        let mut state = Self::default();
        warn!("migrating LoopState schema v1 → v2, deprecated fields: pending_dead_letter");
        if let Some(reason) = v1_state.pending_dead_letter {
            state.push_termination_trigger(TerminationTrigger::DeadLetter {
                kind: reason.kind,
                source: reason.source,
            });
        }
        state.schema_version = LOOP_STATE_SCHEMA_VERSION;
        state
    }
    
    pub fn serialize(&self) -> V2LoopState {
        V2LoopState {
            schema_version: LOOP_STATE_SCHEMA_VERSION,
            termination_triggers: self.termination_triggers.clone(),
            consecutive_failures: self.consecutive_failures,
            // ... 其它字段
        }
    }
}
```

**测试矩阵**:
- v1 state 文件 → 新代码读:不 panic + migration 触发 + warn 日志
- v2 state 文件 → 新代码读:正常
- v1 state 文件 → 旧代码读:旧代码无 schema_version 概念,直接读 v1 字段(若回滚到旧版本)
- v2 state 文件 → 旧代码读:旧代码见不到 schema_version 字段,正常处理其它字段,但 trigger queue 数据丢失(warning 提示)

**CoordinatorDispatcher 外部 caller 迁移清单(R16)**:
- grep 步骤: `rg "CoordinatorDispatcher\|\.dispatch\(" crates/ --type rust` 列出所有调用点
- 必须迁移的 caller(已知):
  - `event_loop/mod.rs` process_output(主路径)
  - `event_loop/rejection.rs` typed counter(消费侧)
  - drift_monitor 3 类告警(部分场景)
- 任何 caller 仍 `state.pending_dead_letter` 访问 = 编译错误(字段删除)
- PR 描述必须粘贴 grep 完整 caller 清单

## Open Questions

- **OQ-1**: U3 `pending_dead_letter` 字段是否需要在 `rejection.rs` 暴露 API 给 `coordinator` hat 直接调用?目前设计是 `CoordinatorDispatcher` 内部 set,coordinator hat 不直接操作。
  - **当前决议**: 不暴露(避免 coordinator hat 绕过 dispatcher),由 dispatcher 单一写入点
- **OQ-2**: U4 `disallowed_tools: ["Edit", "Write"]` 扩展到 Write 后,dim-reviewer 在 scratchpad 子目录的 Write 是否仍被 audit 抓到?
  - **当前决议**: 不抓(audit 只看仓内路径,scratchpad 在仓外),但需在 PR 描述明确此边界

## Acceptance Examples

- **AE-1**: hard_gate typed kind 全覆盖。`enrich_task_resume_payload_with_stage` 加 `kind` 参数后,3 条 caller 路径(hard_gate / stall_recovery / contract)100% 携带 typed kind,recovery.jsonl envelope 解析覆盖率 100%
- **AE-2**: typed dispatch 死信兜底。`CoordinatorDispatcher::dispatch` 扩展 3 个新 kind 后,MissingEventGate count >= 2 触发 PlanBlocked,StallNoEvents count >= 3 触发 PlanBlocked,ContractViolation count >= 1 触发 DriftFinding
- **AE-3**: 终止路径选择正确。ce-executor-serial 跑一个完整 step(包含 hard_gate 触发 + typed dead_letter),loop-termination-reason.json 输出 `"plan.blocked:task_resume_dead_letter:<kind>"` 而非 `"consecutive_failures"`
- **AE-4**: scope_violation 阻断。dim-reviewer 改 1 个仓内文件 → consecutive_failures += 1,跑 ce-executor-serial 验证不出现 8h+ stall
- **AE-5 [预防 R10]**: 双胞胎函数 SSOT 清单。`crates/ralph-core/data/doppelganger-functions.md` 落地,至少 5 对函数(主路径 / 对偶路径 / 对齐状态 3 字段),至少 3 对 = 待修
- **AE-6 [预防 R13]**: plan-gate 桥接修复。ce-executor-serial 跑完整 step-01 → step-02,events 文件中出现 `work.ready(step-02)`(plan-gate emits work.ready 后被 executor 接收),而非仅靠 task.resume 兜底

## Deferred to Follow-Up Work

- **U5**(本次已含): plan-gate 死信根因修复(R13 诊断 P0-3,补 004 plan 漏掉的根因)—— **本次 plan 范围内,不 deferred**
- **U6**(未来): hat-channel routing serial preset 失效(deferred P3 per `2026-06-18-001 plan` 后续工作段)
- **U7**(未来): BDD scenarios 整套重做(`2026-06-20-002 plan` 范围),R-E 风险需配合更新
- **U8**(未来): typed dispatch kind × count 阶梯表首跑后基于实际 freq 调优(本 plan 收尾后,等 1-2 个真实 run 数据)
- **U9**(未来): drift_monitor 3 类告警从 Warn 升级到 Fail,按 U4 AuditSeverity SSOT 范本收敛(scope_violation 是首例,drift 类留待后续 plan)
- **U10**(未来): `cargo xtask doppelganger-check` CI 集成(本 plan 落地 `doppelganger-functions.md` 即可,CI 由后续 plan 接力)

## Sources & Research

- 用户诊断会话: 4 个 sub-agent 并行诊断(流程还原 / 历史上下文 / 对账分析 / 归因修复)
- **机制层根因审查**(5 层递进): 治本 vs 治标 / 同类隐患 / 历史反模式 / 机制健壮性 / 可观测性 — 1 项 BLOCKER(R9)+ 4 项 P1 预防(R10/R11/R12/R13)+ 1 项 P2(R14)+ 3 项新增风险(R-D/R-E/R-F)
- `docs/plans/2026-06-23-004-fix-ce-executor-serial-mechanism-close-loop-plan.md` — 前序 plan,本次是其残留风险收尾
- `docs/solutions/integration-issues/hat_handoff_filename_mismatch_recurrence.md` — 30 天 6 次复发档案
- `docs/solutions/developer-experience/ce-executor-serial-30day-6th-recurrence-fix.md` — 30 天 lessons learned
- `docs/plans/2026-06-18-001-fix-ce-executor-serial-recovery-handoff-plan.md` — U3 missing_event_grace 设计来源
- `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` — **U5 plan-gate 桥接修复 Path A 范本**
- 关键源文件(修复入口速查):
  - `crates/ralph-core/src/event_loop/rejection.rs` 432-548 / 662-687 / 780-800 / 797-799
  - `crates/ralph-cli/src/loop_runner/hard_gate.rs` 740-830 / 769-794
  - `crates/ralph-cli/src/loop_runner/runner.rs` 3254 / 5660
  - `crates/ralph-core/src/event_loop/mod.rs` 1371 / 5898-5907 / 5905-5907
  - `presets/en/ce-executor-serial.yml` 373-375 / 1157
- **新增文件**:
  - `crates/ralph-core/data/doppelganger-functions.md` — U0 双胞胎函数 SSOT 清单
  - `crates/ralph-core/src/event_loop/termination.rs` — U3 TerminationTrigger SSOT
  - `crates/ralph-core/src/event_loop/audit.rs` — U4 AuditSeverity SSOT

### 新增模块对外 API 声明(P1-8 修复)

**`crates/ralph-core/src/event_loop/termination.rs` 模块对外 API**:
- `pub enum TerminationTrigger { Failure { consecutive_count: u32 }, DeadLetter { kind, source }, PlanComplete { plan_id }, QueueOverflow { pushed_count: u32 } }` — typed trigger 队列元素
- `pub enum TerminationReason { Failure { count: u32 }, PlanBlocked { kind: RejectionKind, source: DeadLetterSource }, PlanComplete { plan_id: String }, QueueOverflow, Continue }` — typed termination 输出
- `pub fn TerminationReason::serialize(reason: &TerminationReason) -> String` — 统一序列化,禁止字面拼接
- `pub trait TriggerQueue { fn push(&mut self, t: TerminationTrigger) -> Result<(), Overflow>; fn pop(&mut self) -> Option<TerminationTrigger>; }`
- `lib.rs` 注册:`pub mod termination;`
- **依赖方向**: `termination.rs` → `rejection.rs`(只用 RejectionKind),`termination.rs` ← `mod.rs`(process_output 用)
- **依赖方向单向 DAG,无循环**

**`crates/ralph-core/src/event_loop/audit.rs` 模块对外 API**:
- `pub enum AuditSeverity { Warn, Fail { add_failures: u32 }, BlockLoop { reason: String } }`
- `pub struct AuditContext { pub hat: HatId, pub kind: RejectionKind, pub details: String }`
- `pub struct AuditDispatcher`
- `impl AuditDispatcher { pub fn dispatch(state: &mut LoopState, severity: AuditSeverity, ctx: AuditContext); }`
- `lib.rs` 注册:`pub mod audit;`
- **依赖方向**: `audit.rs` → `termination.rs`(BlockLoop severity 触发 termination trigger),`audit.rs` ← `mod.rs`(audit_file_modifications / drift_monitor 用)
- **依赖方向单向 DAG,无循环**

**`crates/ralph-core/data/doppelganger-functions.md` 数据文件 schema**:
- 每条 entry 必含字段:`主路径`(完整函数签名) / `对偶路径`(完整函数签名) / `对齐状态`(`待修` / `已修` / `N/A`) / `关联 plan`(本 plan 或后续 plan)
- CI 解析(P1-8 后续 U10): grep `状态: 待修` 数量 = 0 才允许 merge
