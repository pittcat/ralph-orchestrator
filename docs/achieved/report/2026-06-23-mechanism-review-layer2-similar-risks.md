---
title: 机制层审查 - 第 2 层:同类隐患扫描
date: 2026-06-23
type: mechanism-review-layer2
context:
  - docs/report/2026-06-23-ralph-e2e-ce-executor-serial-loop-20260622-182705-diagnosis.md
  - docs/report/2026-06-23-adversarial-review-of-mechanism-fix.md
target_diff:
  - crates/ralph-core/src/hat_handoff/gate.rs
  - crates/ralph-core/src/preset/engine/gates.rs
scope: 仅扫描与修复 v1(enum 字段扩展 + iter/seq 与文件名耦合 + typed routing)同机制下**未在 v1 修复覆盖**的位置
status: 已完成扫描
---

# 机制层审查 - 第 2 层:同类隐患扫描

## 扫描方法

```bash
# 命令 1: iter/seq 与文件名耦合(rg 命中 18 条)
rg "iteration.*seq|seq.*iteration|filename.*iter|iter.*filename" \
   crates/ralph-core/src --type rust -n

# 命令 2: GateDecision 使用点(rg 命中 60+ 条,跨 3 个 module)
rg "GateDecision" crates/ralph-core/src --type rust -n

# 命令 3: RejectionKind match arm(命中 16 条 destructure)
rg "RejectionKind" crates/ --type rust -n

# 命令 4: ratchet-style `Reject { kind, message }` 不完整 destructure
rg "GateDecision::Reject \{ kind" crates/ --type rust -n
```

**修复 v1 的核心模式**:
- (A) `enum` 加 variant → 下游 `match` 需 `..`(Rust 编译器 E0027)
- (B) `enum` variant 加字段 → 下游 destructure 需 `..`
- (C) 文件名命名规则(`iter-seq-from-to`)与外部 `LoopState` 计数器耦合
- (D) recovery 重试计数器与 lint 分类解耦,无法按 kind 分桶累积

---

## 同类隐患清单

### 隐患 1: `GateDecision::Reject { kind, message }` 4 处 destructure 缺 `..`(ratchet-style,编译阻断)

- **位置**:
  - `crates/ralph-core/src/preset/engine/gates.rs:306`(测试)
  - `crates/ralph-core/src/preset/engine/gates.rs:348`(测试)
  - `crates/ralph-core/src/preset/engine/linter.rs:312`(生产路径)
  - `crates/ralph-core/src/event_loop/mod.rs:5143`(生产路径)
- **风险**: **P0**(阻断编译,对抗报告 P0-1 已识别的同机制,但本扫描多找到 1 处 `event_loop/mod.rs:5143` 没在对抗报告里被独立列出)
- **根因**: 修复 v1 给 `Reject` 加 `kind: RejectionKind` 字段,但 4 个下游 match 没补 `..`
- **修复建议**: 4 处 match arm 统一改为 `GateDecision::Reject { kind, message, .. }`

### 隐患 2: `RejectionKind` 自身 match arm 缺 `..`(未来加 variant 时会编译失败)

- **位置**:
  - `crates/ralph-core/src/preset/engine/gates.rs:103-107`(`to_lint_class` 5 variants)
  - `crates/ralph-core/src/preset/engine/gates.rs:113-115`(`to_lint_class` 3 新 variants)
  - `crates/ralph-core/src/preset/engine/gates.rs:125-129`(`reason_code` 5 老 variants)
  - `crates/ralph-core/src/preset/engine/gates.rs:134-136`(`reason_code` 3 新 variants)
- **风险**: **P1**(当前 8 variants 已 exhaustive,但未加 `#[non_exhaustive]`,未来再加 variant 即编译失败)
- **根因**: `RejectionKind` 定义本身没标 `#[non_exhaustive]`,而其 match arm 也没用 `..`
- **修复建议**: (a) 给 `RejectionKind` enum 加 `#[non_exhaustive]`;(b) match arm 显式 `..` 或未来编译期阻断

### 隐患 3: 同名 `GateDecision` 跨 3 个 module 定义

- **位置**:
  - `crates/ralph-core/src/hat_handoff/gate.rs:64`(`pub enum GateDecision` 主仓 hat_handoff)
  - `crates/ralph-core/src/step_handoff/progress_task_gate.rs:259`(`pub enum GateDecision` step_handoff)
  - `crates/ralph-core/src/validation/rules_step_handoff.rs:62`(使用方)
- **风险**: **P1**(命名冲突,任何扩展 `Reject` 字段时容易误改错文件)
- **根因**: 两个 module 各自定义同名 enum,语义不同(hat_handoff 的 `Reject` 是 lint,step_handoff 的 `Mismatch` 是 progress),但名字撞车
- **修复建议**: 把 `step_handoff/progress_task_gate.rs:259` 的 enum 重命名为 `ProgressGateDecision` 或 `TaskProgressDecision`,隔离命名空间

### 隐患 4: `LoopState.hat_handoff_seq` 与文件名 `parse_filename` 解析点耦合但 SSOT 不强

- **位置**:
  - `crates/ralph-core/src/event_loop/loop_state.rs:158`(`hat_handoff_seq: u32` 字段定义)
  - `crates/ralph-core/src/hat_handoff/allocator.rs:72`(`parse_filename` → `(u32, u32, String, String)`)
  - `crates/ralph-core/src/hat_handoff/gate.rs:198`(`let (file_iter, file_seq, ...) = match allocator::parse_filename(handoff_path)`)
  - `crates/ralph-core/src/hat_handoff/gate.rs:216`(`if !inputs.skip_seq_check && (file_iter != inputs.iteration || file_seq != expected_seq)`)
- **风险**: **P0**(诊断报告 §3.3 已识别为 30 天第 6 次复发根因)
- **根因**: 文件名 `<iter>-<seq>-<from>-<to>.md` 由 agent 自填,LoopState 计数器与文件名解析在两处,任一漂移即 filename_mismatch
- **修复建议**: 把文件名构造完全交给 `allocator::prepare_with_dedup`(已有,见 `state/ledger.rs:624`),agent 不再直接拼文件名;linter 拒绝任何文件名不符合 `allocator::compute()` 派生结果的 emit

### 隐患 5: `record_rejection_key` 单一计数器,不按 `RejectionKind` 分桶

- **位置**:
  - `crates/ralph-core/src/event_loop/loop_state.rs:870`(`pub fn record_rejection_key(&mut self, key: &str) -> u32`)
  - `crates/ralph-core/src/event_loop/loop_state.rs:887`(`record_recoverable_rejection_key`)
  - `crates/ralph-core/src/event_loop/loop_state.rs:916`(`rejection_key_is_exhausted`)
- **风险**: **P0**(诊断报告 P0-2 + 对抗报告 P0-2 都识别的核心 typed 路由死端)
- **根因**: `key` 是 `String`,由 `compute_retry_key` 派生,但**派生逻辑未把 `RejectionKind` 包含进去**(本次修复给 `Reject` 加了 `kind` 字段,但 `compute_retry_key` 的 caller 没同步传 kind)
- **修复建议**: (a) `compute_retry_key` 签名加 `kind: RejectionKind` 参数;(b) `record_rejection_key` 改为按 kind 分桶的 `HashMap<RejectionKind, u32>`;(c) `consecutive_lint_rejections:{kind}` 字段补到 `LoopState`

### 隐患 6: `stall_detector_had_events` 与 hat_handoff 死信检测未联动

- **位置**:
  - `crates/ralph-core/src/event_loop/loop_state.rs:334`(`pub stall_detector_had_events: bool`)
  - `crates/ralph-core/src/event_loop/mod.rs:9877`(`fn run_stall_detector_on_state`)
  - `crates/ralph-core/src/event_loop/mod.rs:6597`(注释:"so they do NOT count as progress toward the stall detector")
- **风险**: **P0**(诊断报告 P0-3:8h+ 0 业务事件 0 stall 报警)
- **根因**: stall detector 只看「整体事件流」,不区分 hat_handoff artifact 已写但下游未消费的死信状态
- **修复建议**: `LoopState` 加 `pending_handoff_artifacts: HashSet<PathBuf>`,发 `work.ready` 等宏观边时登记,executor 接手时清除;超时(5 min)未清除触发 `stall.handoff_unconsumed`

### 隐患 7: `task.resume` payload typed routing 信息丢失

- **位置**:
  - `crates/ralph-core/src/hat_handoff/gate.rs:355-365`(`reject_to_task_resume` 函数,构造 `(reason_code, message)` payload)
  - `crates/ralph-core/src/event_loop/mod.rs:7435-7478`(`task.resume` 消费者按 reason_code 字符串匹配)
- **风险**: **P1**(对抗报告 P1-1 已识别)
- **根因**: 修复 v1 给 `Reject` 加 `kind` 字段,但 `reject_to_task_resume` 函数没把 `kind` 注入 payload,消费者拿到的是字符串无法 typed route
- **修复建议**: 函数签名扩展为 `(&'static str, RejectionKind, String)` 三元组,payload 显式带 `kind` 字段

### 隐患 8: `recovery.jsonl` 升级链路缺 typed kind

- **位置**:
  - `crates/ralph-core/src/diagnosis/envelope.rs:349-403`(`RecoveryDiagnosisEnvelope.retry_attempt` / `safe_target`)
  - `crates/ralph-core/src/diagnosis/responder.rs:489`(`classify(&retry_key, current_iteration, safe_target)`)
  - `crates/ralph-core/src/diagnosis/responder.rs:760`(`format_finding_line` 把 env + iter 写 recovery.jsonl)
- **风险**: **P1**(诊断报告 P0-2 已识别:本次 4 个 violation 全 `outcome=failed` 不升级)
- **根因**: `envelope` 序列化 JSON 不带 `RejectionKind`,只有字符串 `reason_code`,operational scripts 与 CLI help 全靠字符串匹配
- **修复建议**: `RecoveryDiagnosisEnvelope` 加 `kind: Option<RejectionKind>` 字段(序列化时用 `reason_code()` 字符串保兼容,内存保留 typed)

---

## 二轮修复建议(基于本扫描的必做项,共 5 条)

1. **必做 A**:把 4 处 `GateDecision::Reject { kind, message }` 补 `..`(隐患 1),跑 `cargo check --workspace --all-targets` 全绿
2. **必做 B**:补 `compute_retry_key` 的 `kind: RejectionKind` 参数 + `LoopState.consecutive_lint_rejections:{kind}` 分桶字段(隐患 5)
3. **必做 C**:把 `reject_to_task_resume` 函数扩展为返回 typed kind,`task.resume` payload 显式带 `kind` 字段(隐患 7)
4. **必做 D**:`RejectionKind` enum 加 `#[non_exhaustive]`,所有 match arm 显式 `..`(隐患 2)
5. **必做 E**:把 step_handoff 的同名 `GateDecision` 重命名(隐患 3),消除命名冲突

**优先级**:A > B > C(阻断 + 核心机制) > D > E(代码卫生)

**未在本扫描覆盖范围**:stall detector 与 hat_handoff 联动(隐患 6)、文件名 SSOT 化(隐患 4)、recovery.jsonl kind 字段(隐患 8)—— 建议与本批修复并行在 plan 2026-06-21-001 U1/U4 的 follow-up 计划中落地。

---

**扫描结果统计**:
- 总同类隐患数:**8**
- P0:**4**(隐患 1, 4, 5, 6)
- P1:**4**(隐患 2, 3, 7, 8)
- 二轮修复必做项:**5**
- 本扫描新增发现(对抗报告未独立列出):**1**(隐患 1 中 `event_loop/mod.rs:5143` + 隐患 2/3)
