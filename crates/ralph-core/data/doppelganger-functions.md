# 双胞胎函数 SSOT 清单(Doppelganger Functions)

> **目的**: 根除 30 天第 7 次复发根因——「修了 A 路径忘了 B 路径」。本文件是 ce-executor-serial 修复类 U1-U5 的对账清单,任何 `task.resume` / `enrich_*` / `dispatch` / `audit` 类改动实施前,必须先 `grep` 本清单确认无第 3 条 caller 遗漏。

## 维护规则

- 每条 entry 必含:`主路径`(完整函数签名)/ `对偶路径`(完整函数签名)/ `对齐状态`(`待修` / `已修` / `N/A`)/ `关联 plan`(本 plan 或后续 plan)/ `grep 验证`(`rg "..." crates/ --type rust` 的预期 caller 数)
- CI 解析(U10 未来 plan):grep `状态: 待修` 数量 = 0 才允许 merge
- 实施任何双胞胎函数修复前,先 grep 该清单核对 caller 路径

---

## 对 1: `build_task_resume_payload` ↔ `enrich_task_resume_payload_with_stage`

**主路径**:
```rust
crates/ralph-core/src/event_loop/rejection.rs:432
pub fn build_task_resume_payload(
    rejection: &Rejection,
    allowed_topics: &[String],
    required_fields: &[String],
    original_trigger_topic: Option<&str>,
    original_trigger_payload: Option<&str>,
    wave_context: Option<&WaveContextForResume>,
) -> String
```
**功能**: 从 `Rejection` struct 构造完整 `task.resume` payload(含 `kind` typed field,2026-06-23-004 plan U5 已加)。

**对偶路径**:
```rust
crates/ralph-core/src/event_loop/rejection.rs:640
pub fn enrich_task_resume_payload(
    free_form_message: &str,
    reason_hint: &str,
    target_hat: Option<&str>,
) -> String
// 行 645: enrich_task_resume_payload_with_stage(free_form_message, reason_hint, target_hat, None)

crates/ralph-core/src/event_loop/rejection.rs:662
pub fn enrich_task_resume_payload_with_stage(
    free_form_message: &str,
    reason_hint: &str,
    target_hat: Option<&str>,
    stage: Option<RejectionStage>,
) -> String
```
**功能**: 自由文本输入包装 `task.resume` payload,目前**不带 typed `kind` 字段**(只带 `stage` 字符串)。

**对齐状态**: **待修**(本 plan U1)

**关联 plan**: `2026-06-23-005` U1

**grep 验证**:
```bash
rg "enrich_task_resume_payload_with_stage\(" crates/ --type rust
# 预期 caller 数 = 2(生产 hard_gate 路径,实施 U1 + F2 后):
# - crates/ralph-cli/src/loop_runner/hard_gate.rs:568  → inject_hard_gate_guidance_with_triggers (claim-but-no-write 路径) → RejectionKind::MissingEventGate
# - crates/ralph-cli/src/loop_runner/hard_gate.rs:770  → inject_missing_event_hard_gate_guidance (missing-event 路径) → RejectionKind::MissingEventGate
# 加上 tests/ 内联 case 4 个,合计 6 个 grep 命中。

rg "enrich_task_resume_payload\(" crates/ --type rust
# 生产 mod.rs caller = 4(2026-07-01 复核,base `enrich_task_resume_payload(`):
#   - crates/ralph-core/src/event_loop/mod.rs:2202 → PersistentLoopActive(persistent 模式兜底)
#   - crates/ralph-core/src/event_loop/mod.rs:2249 → OpenTasksBlocking(open tasks 拒绝完成信号)
#   - crates/ralph-core/src/event_loop/mod.rs:3036 → aggregate_timeout(review-synthesizer 恢复,3-arg,不带 typed kind)
#   - crates/ralph-core/src/event_loop/mod.rs:8227 → isolated_extra_business_event_dropped(isolated 单业务事件超发)
# 注:原 stall_recovery 三处 StallNoEvents 已迁移到新变体 `enrich_task_resume_payload_full(`
#     (带 hat publishes 参数),现位于 mod.rs:3308 / 3365 / 3401;grep base 变体时不再命中。
# 其余命中为 tests(event_loop/tests/enrich_kind_wiring.rs、rejection.rs 内联)。

rg "build_task_resume_payload\(" crates/ --type rust
# 2026-07-01 复核:已新增 1 个生产 caller(R5 wave context 恢复路径),不再是"无生产 caller":
# - crates/ralph-core/src/event_loop/mod.rs:7977         (生产:isolated stale-rejection 恢复)
# - crates/ralph-cli/tests/ce_executor_recovery.rs:393   (e2e 集成)
# - crates/ralph-core/src/event_loop/rejection.rs:1164+  (内联 tests 多个)
```

**实施步骤**(U1):
1. 函数签名变更:`fn enrich_task_resume_payload_with_stage(free_form_message, reason_hint, target_hat, stage: Option<RejectionStage>, kind: Option<RejectionKind>) -> String`
2. `obj["kind"] = match kind { Some(k) => json!(k.reason_code()), None => json!(extract_reason_code(reason_hint)) }`
3. `enrich_task_resume_payload` 同步加 `kind: Option<RejectionKind>` 参数并转发
4. 2 caller(hard_gate.rs:568/769)传 `Some(RejectionKind::IllegalEmitClaim)` 与 `Some(RejectionKind::MissingEventGate)` — **见 RejectionKind 扩展条目**

---

## 对 2: `RejectionKind` enum (gates.rs) ↔ `RejectionStage` enum (rejection.rs)

**主路径**:
```rust
crates/ralph-core/src/preset/engine/gates.rs:56-96
#[non_exhaustive]
pub enum RejectionKind {
    MissingField, TopicOwnership, UpstreamState, HandoffArtifact, PreCheck,
    HandoffFilenameMismatch, HandoffStructureInvalid, HandoffIllegalEmitTopic,
    // (9 variants, 2026-06-23-004 plan 已落地)
}
```
**功能**: 拒绝原因 typed SSOT,`reason_code()` 提供稳定字符串。

**对偶路径**:
```rust
crates/ralph-core/src/event_loop/rejection.rs:32-67
pub enum RejectionStage {
    Origin, Policy, ExecutionContract, PayloadContract,
    MissingEvent, EmitClaimedButNotWritten,  // 2 个 hard_gate 来源 variant
    // (6 variants)
}
```
**功能**: 拒绝层级(stage)分类,作 `task.resume` payload 的 `stage` 字符串值。

**对齐状态**: **部分对齐**(`build_task_resume_payload` 已用 kind 字段,`enrich_*` 只用 stage)

**关联 plan**: `2026-06-23-005` U1(typed kind 全覆盖) + U2(`#[non_exhaustive]` 防护已落地,见 R9 BLOCKER 状态)

**grep 验证**:
```bash
rg "pub enum RejectionKind" crates/ralph-core/src/preset/engine/gates.rs
# 预期 1 命中(已标 #[non_exhaustive],R9 BLOCKER 已满足)

rg "#\[non_exhaustive\]" crates/ralph-core/src/preset/engine/gates.rs
# RejectionKind 标 #[non_exhaustive] ✅
```

**实施步骤**(U1 + U2):
1. RejectionKind 加 3 个 variant(配合 Plan KTD-1 / R2):
   - `MissingEventGate`(hard_gate.rs:769 caller 来源)
   - `StallNoEvents`(orchestrator stall_recovery 路径来源,mod.rs:2755 等)
   - `ContractViolation`(payload contract 拒绝来源)
2. RejectionKind::reason_code() 加 3 个 reason_code 字符串映射
3. RejectionKind::to_lint_class() 加 3 个 → `LintFailureClass::PayloadError`
4. U2 验证 `CoordinatorDispatcher::dispatch` match 编译通过 + 显式补 `..` 兜底

---

## 对 3: `CoordinatorDispatcher::dispatch` (typed escalation) ↔ 散落 RejectionKind match

**主路径**:
```rust
crates/ralph-core/src/event_loop/rejection.rs:780-806
pub struct CoordinatorDispatcher;
impl CoordinatorDispatcher {
    pub fn dispatch(
        kind: crate::preset::engine::gates::RejectionKind,
        consecutive_count: u32,
    ) -> CoordinatorAction
}
```
**功能**: 按 KTD-1 表阶梯触发 typed 升级事件(DriftFinding / CircuitBreakerTrip / PlanBlocked)。当前已覆盖 3 个 Handoff* kind。

**对偶路径**(散落匹配):
```rust
crates/ralph-core/src/event_loop/mod.rs:2202 / 2249 / 3036 / 8227   (base enrich_task_resume_payload)
crates/ralph-core/src/event_loop/mod.rs:3308 / 3365 / 3401          (StallNoEvents,已迁移到 enrich_task_resume_payload_full)
// 2026-07-01 复核:base 变体现有 4 处生产调用(见对 1);原 3 处 StallNoEvents
// 已迁移到带 publishes 参数的 _full 变体,各自独立 reason_hint 字符串。
```

**对齐状态**: **已修**(本 plan U2 + F2)

**关联 plan**: `2026-06-23-005` U2

**grep 验证**:
```bash
rg "CoordinatorDispatcher::dispatch\(" crates/ --type rust
# 预期 caller 数 = 10(rejection.rs:1401/1408/1415/1426/1438/1455/1463/1475/1482 + mod.rs:7697 + tests/serial_lint.rs:431)
# 实施 U2 前必须逐一 inspect 确认每个 caller 的 (kind, count) 来源

rg "RejectionKind::" crates/ --type rust
# 实施 U2 后 match 必须覆盖新增 3 variant(MissingEventGate / StallNoEvents / ContractViolation)
```

**实施步骤**(U2):
1. 验证 `RejectionKind` 已标 `#[non_exhaustive]`(已确认 ✅,见对 2)
2. `CoordinatorDispatcher::dispatch` match 扩展 3 臂:
   - `MissingEventGate`: 阶梯 count >= 2 → `PlanBlocked`(Plan KTD-2)
   - `StallNoEvents`: 阶梯 count >= 3 → `PlanBlocked`(同 HandoffIllegalEmitTopic)
   - `ContractViolation`: 阶梯 count >= 1 → `DriftFinding`(早期报警)
3. 显式 `_ => ReEmitWorkReady`(或保留 `_ => PlanBlocked` 兜底)兜底臂
4. 加 trybuild `compile_fail` 测试验证 `#[non_exhaustive]` 强制 match 覆盖

---

## 对 4: `audit_file_modifications` ↔ `audit_scope_violation` (待确认 / 实施 U4 评估)

**主路径**:
```rust
crates/ralph-core/src/event_loop/mod.rs:6961
fn audit_file_modifications(&mut self, hat_id: &HatId)
```
**功能**: 检测 scope_violation(hat 改了不该改的文件),目前只 emit `{hat}.scope_violation` 诊断事件 + WARN 日志,**不计入 consecutive_failures**。

**对偶路径**: 无独立 `audit_scope_violation` 函数,**逻辑全部内联在 `audit_file_modifications`**。

**对齐状态**: **待修**(本 plan U4)

**关联 plan**: `2026-06-23-005` U4

**grep 验证**:
```bash
rg "fn audit_" crates/ralph-core/src/event_loop/mod.rs
# 预期命中:
# - audit_file_modifications (mod.rs:6961)
# 实施 U4 前 grep 全 codebase 确认无遗漏 audit 函数

rg "scope_violation_circuit_breaker_tripped" crates/ralph-core/src --type rust
# 已有 typed field 但未走 AuditSeverity SSOT(对 6)
```

**实施步骤**(U4):
1. 新模块 `crates/ralph-core/src/event_loop/audit.rs` 落地 `AuditSeverity` typed enum
2. `audit_file_modifications` 重构为返 `(AuditSeverity, AuditContext)` 元组
3. scope_violation 升级为 `AuditSeverity::Fail { add_failures: 1 }`(本 plan 范围)
4. drift_monitor 3 类告警改返 `AuditSeverity::Warn`(本 plan 仅迁移接口,不改 severity,留 U9)

---

## 对 5: `process_output` 终止触发器散落 ↔ `TerminationTrigger` typed enum

**主路径**(目标):
```rust
// 本 plan U3 新增
crates/ralph-core/src/event_loop/termination.rs (新增模块)
pub enum TerminationTrigger {
    Failure { consecutive_count: u32 },
    DeadLetter { kind: RejectionKind, source: DeadLetterSource },
    PlanComplete { plan_id: String },
    QueueOverflow { pushed_count: u32 },
}
```

**对偶路径**(现状态):
```rust
crates/ralph-core/src/event_loop/mod.rs:6606
pub fn process_output(
    &mut self,
    hat_id: &HatId,
    output: &str,
    success: bool,
) -> Option<TerminationReason>
```
**功能**: 当前在 `success=false` 分支独立判 `consecutive_failures >= 5` / `pending_dead_letter` flag / `plan_complete` 3 个终止触发器,3 个 if 散落。

**对齐状态**: **待修**(本 plan U3)

**关联 plan**: `2026-06-23-005` U3

**grep 验证**:
```bash
rg "pending_dead_letter" crates/ docs/ tests/ --type rust --type markdown
# 实施 U3 后应 0 命中(R-D 风险 + P1-7 修复要求)

rg "consecutive_failures\s*>=\s*5" crates/ralph-core/src --type rust
# 当前命中数:grep 验证(U3 完成后应只剩 process_output 单点判)
```

**实施步骤**(U3):
1. 新模块 `crates/ralph-core/src/event_loop/termination.rs` 落地 TerminationTrigger typed enum
2. `LoopState::pending_dead_letter` 字段删除,改用 `termination_triggers: VecDeque<TerminationTrigger>`
3. `process_output` 重构为单 match `TerminationTrigger` 分支
4. Schema v1 → v2 迁移(R15):v1 含 `pending_dead_letter` → 自动转 `TerminationTrigger::DeadLetter` 入队 + warn
5. typed serialization:`TerminationReason::serialize(&PlanBlocked { kind })` 替代字面字符串拼接

---

## 对 6: drift_monitor 3 类告警散落 ↔ AuditSeverity SSOT

**主路径**(目标):
```rust
crates/ralph-core/src/event_loop/audit.rs (本 plan U4 新增)
pub enum AuditSeverity {
    Warn,
    Fail { add_failures: u32 },
    BlockLoop { reason: String },
}
```

**对偶路径**(现状态):drift_monitor 3 类告警各自返 `Option<DriftFinding>`,**不走 AuditSeverity**。

**对齐状态**: **半边收敛**(本 plan U4 仅迁移接口,不改 severity)

**关联 plan**: `2026-06-23-005` U4(本 plan 落地 SSOT)+ U9(后续 plan 完整升级到 Fail)

**grep 验证**:
```bash
rg "fn (coord_join_rate|field_completeness|drift_unconsumed)" crates/ralph-core/src --type rust
# 预期命中 3 个 drift_monitor 子函数,实施 U4 后返类型统一改为 AuditSeverity::Warn
```

**实施步骤**(U4):
1. drift_monitor 3 类改返 `AuditSeverity::Warn`
2. 由 `AuditDispatcher::dispatch` 统一处理
3. **完整升级到 Fail 留 U9**(本 plan 仅留 SSOT 接口)

---

## 实施前对账清单(每次 U 启动前必查)

实施任何 U 之前,先 `grep` 上述 6 对清单 + 各自预期 caller 数;**任何 caller 遗漏(未在本清单中标注) = PR 拒绝 merge**。

```bash
# U1 启动前(对 1 + 对 2):
rg "enrich_task_resume_payload_with_stage\(|enrich_task_resume_payload\(|build_task_resume_payload\(" crates/ --type rust

# U2 启动前(对 2 + 对 3):
rg "CoordinatorDispatcher::dispatch\(" crates/ --type rust
rg "pub enum RejectionKind" crates/ralph-core/src/preset/engine/gates.rs

# U3 启动前(对 5):
rg "pending_dead_letter" crates/ --type rust
rg "fn process_output" crates/ --type rust

# U4 启动前(对 4 + 对 6):
rg "fn audit_" crates/ralph-core/src/event_loop/ --type rust
rg "drift_monitor|coord_join_rate|field_completeness|drift_unconsumed" crates/ralph-core/src --type rust

# U5 启动前(plan-gate 桥接,与对 1-6 独立):
rg "work\.ready" crates/ presets/ --type rust
# 预期唯一 emit 点 = plan-gate(其它 hat 不允许 emit)
```

---

## 版本与维护

- **创建**: 2026-06-23,`2026-06-23-005` plan U0
- **关联**:
  - 上游: `2026-06-23-004-fix-ce-executor-serial-mechanism-close-loop-plan.md`(typed kind typed counter)
- **维护**: 任何 `task.resume` / `enrich_*` / `dispatch` / `audit` 类改动前,先 grep 本清单核对;新增双胞胎函数对时,按本格式添加 entry
