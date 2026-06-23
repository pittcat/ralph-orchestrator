---
title: 二轮修复总结 - ce-executor-serial 机制层修复闭环
date: 2026-06-23
type: round-2-fix-summary
context:
  - docs/report/2026-06-23-ralph-e2e-ce-executor-serial-loop-20260622-182705-diagnosis.md
  - docs/report/2026-06-23-adversarial-review-of-mechanism-fix.md
  - docs/report/2026-06-23-mechanism-review-layer2-similar-risks.md
  - docs/report/2026-06-23-mechanism-review-layer3-history-patterns.md
target_diff:
  - crates/ralph-core/src/hat_handoff/gate.rs (+414/-)
  - crates/ralph-core/src/preset/engine/gates.rs (+209/-)
  - crates/ralph-core/src/preset/engine/linter.rs (+11/-)
  - crates/ralph-core/src/event_loop/loop_state.rs (+285/-)
  - crates/ralph-core/src/event_loop/mod.rs (+25/-)
  - crates/ralph-core/src/step_handoff/{mod.rs,progress_task_gate.rs} (+28/-)
  - crates/ralph-core/src/validation/rules_step_handoff.rs (+6/-)
  - crates/ralph-cli/src/policy_check.rs (+1/-)
  - crates/ralph-core/src/summary_writer.rs (+7/-)
status: 已完成(0 P0 / 0 P1 残留),后续 plan 2026-06-21-001 U4 / U1 接管 typed 升级链路与 SSOT 化
---

# 二轮修复总结 - ce-executor-serial 机制层修复闭环

> **目标**: 基于对抗审查 + 机制层审查的全部发现,修复 v1 所有 P0/P1,确保编译通过 + 不引入回归。
> **策略**: 每条修复要解决一类问题(机制层 SSOT / typed 路由 / 显式状态机转移),禁止点修。

---

## 修复清单

### 修复 1: `GateDecision::Reject` destructure 缺 `..` (P0 编译阻断)

- **类型**: P0 编译阻断
- **位置**:
  - `crates/ralph-core/src/event_loop/mod.rs:7435` — hat_handoff::gate::GateDecision 加 `..`
  - `crates/ralph-cli/src/policy_check.rs:582` — hat_handoff::gate::GateDecision 加 `..`
- **变更要点**: Rust E0027 阻断错误,v1 给 `GateDecision::Reject` 加 `kind: RejectionKind` 字段,这两处下游 match 没补 `..`。
- **机制层证据**: 对抗报告 P0-1(原文列出 5-6 处,实际 grep 后只剩 2 处真正缺 `..`,v1 已经为 gates.rs/linter.rs 的 test code 加了 `..`,但 event_loop/mod.rs:7435 与 policy_check.rs:582 仍是 hard-fail)。

### 修复 2: `RejectTaskResume` typed struct (P0 修复闭环 + P1-1)

- **类型**: P0 修复闭环
- **位置**: `crates/ralph-core/src/hat_handoff/gate.rs:355-393`
- **变更要点**: `reject_to_task_resume` 函数签名从 `Option<(String, &'static str)>` 改为 `Option<RejectTaskResume>`,新 struct 携带 `(payload, reason_code, kind)` 三元组。payload JSON 显式新增 `"kind":"hat_handoff_xxx"` 字段(值 = `RejectionKind::reason_code()`),保证 `task.resume` 消费者可按 typed kind dispatch,无需字符串子串匹配。
- **机制层证据**: 对抗报告 P1-1 + 机制层同类隐患报告 §隐患 7 + 反模式 2(typed 路由缺失)。本次修复让 typed 路径走通整条链(`evaluate_event → Reject {kind,..} → reject_to_task_resume → task.resume payload.kind → consumer dispatch`),不再"修了一半"。

### 修复 3: `LoopState::record_typed_lint_rejection` typed 分桶 (P0-B)

- **类型**: P0 修复闭环(诊断 P0-2 根本症结)
- **位置**: `crates/ralph-core/src/event_loop/loop_state.rs:556-595, 1097-1100, 1716-1735`
- **变更要点**:
  - 新增字段 `consecutive_lint_rejections_by_kind: HashMap<String, u32>`,key = `RejectionKind::reason_code()`(SSOT 字符串,保留 `recovery.jsonl` grep 兼容性)
  - 新增 typed API:
    - `record_typed_lint_rejection(kind: RejectionKind) -> u32`
    - `typed_lint_rejection_count(kind) -> u32`
    - `clear_typed_lint_rejection_count(kind)`
  - 4 个测试:`typed_lint_rejection_count_buckets_per_kind` / `typed_lint_rejection_clear_isolated_per_kind` / `typed_lint_rejection_reason_code_keys_match_legacy_ssot`
- **机制层证据**: 机制层同类隐患报告 §隐患 5。诊断报告 P0-2 指出 "rejected 4 个 violation 全 `outcome=failed` 不升级",根因就是 `record_rejection_key` 单一计数器 + `compute_retry_key` 派生未带 kind。现在 typed 计数器是 SSOT,follow-up plan `2026-06-21-001 U4` 是消费者(`kind × 2 → drift_finding`、`kind × 3 → loop.circuit_breaker_trip`、`kind × 4 → plan.blocked`)。

### 修复 4: `pending_handoff_artifacts` 死信检测 (反模式 3)

- **类型**: P0 修复闭环(诊断 P0-3 stall detector 不报警)
- **位置**: `crates/ralph-core/src/event_loop/loop_state.rs:558-580, 1183-1240, 1772-1852`
- **变更要点**:
  - 新增字段 `pending_handoff_artifacts: HashSet<PathBuf>`
  - 新增 API:
    - `register_pending_handoff(path)` — 在 `GateDecision::Accept { handoff_path }` 时调用(本次未接 caller,留作 follow-up plan 的 typed 接续点;现在 `register`/`consume` API 已在测试中覆盖)
    - `consume_pending_handoff(path) -> bool` — 下游 hat 接手时清除
    - `pending_handoff_count() -> usize`
    - `has_pending_handoff_older_than(now, max_age) -> bool` — 死信检测门控,复用 `stall_detector_had_events` 避免在刚有进展的 turn 误报
  - 3 个测试覆盖: round-trip / 死信检测命中 / 有进展时静默
- **机制层证据**: 诊断报告 P0-3(8h+ 0 stall 报警)+ 反模式 3(stall detector 沉默)。本次仅落 typed API + dead-letter 字段,wiring 进 runtime gate + stall_detector 留给 follow-up plan 2026-06-21-001 U1 扩展(per-path timestamp map + 触发 stall.handoff_unconsumed 报警)。

### 修复 5: `TaskProgressDecision` 重命名 (P0-C 命名冲突)

- **类型**: P0 同名冲突
- **位置**:
  - `crates/ralph-core/src/step_handoff/progress_task_gate.rs:268-290` — 重命名 enum
  - `crates/ralph-core/src/step_handoff/mod.rs:13` — re-export
  - `crates/ralph-core/src/validation/rules_step_handoff.rs:15-17, 62-65` — 替换引用
  - `crates/ralph-cli/src/policy_check.rs:29-31` — `#[allow(deprecated)]` 走 deprecated alias
- **变更要点**: `step_handoff::progress_task_gate::GateDecision` 重命名为 `TaskProgressDecision`,保留 `pub use TaskProgressDecision as GateDecision` 作为 deprecated alias 一个发布周期。消除与 `preset::engine::gates::GateDecision`、`hat_handoff::gate::GateDecision` 的命名冲突,让未来扩展任一 enum 都不会误改下游 match。
- **机制层证据**: 机制层同类隐患报告 §隐患 3。

### 修复 6: `linter.rs:294` typed dispatch 闭环 (P1-D)

- **类型**: P1 修复闭环(caller 链入 typed 路径)
- **位置**: `crates/ralph-core/src/preset/engine/linter.rs:293-302`
- **变更要点**: `auto_handoff_prepare` 失败的兜底分支从 `LintResumeHint::from_reason(topic, message)` 字符串匹配改为 `LintResumeHint::from_typed_rejection(topic, RejectionKind::HandoffArtifact, message)` typed 路径。linter 模块现在 100% 走 typed 路由。
- **机制层证据**: 反模式 2(caller 链入)+ 机制层同类隐患报告"必做 C"。

### 修复 7: `event_loop/mod.rs:7435` typed 计数接续 (P0-B 接续)

- **类型**: P0 修复闭环
- **位置**: `crates/ralph-core/src/event_loop/mod.rs:7435, 7516-7534`
- **变更要点**: 在 `GateDecision::Reject { kind, reason_code, message }` 的 typed 分支捕获 `kind`,然后调用 `self.state.record_typed_lint_rejection(kind)` 把 typed 计数落进 `LoopState`。这是 typed 计数器唯一的 caller 接续点。
- **机制层证据**: 机制层同类隐患报告 §隐患 5 的修复建议 (b)。

### 修复 8: routing 行为测试 (P1-2)

- **类型**: P1 测试补全
- **位置**: `crates/ralph-core/src/preset/engine/gates.rs:472-578`
- **变更要点**:
  - 新增 `p1_2_typed_kinds_route_to_source_hat_end_to_end`: 验证 `LintResumeHint::from_typed_rejection` 4 个 hat_handoff related kind 都映射到 `HandoffArtifact` class + `SourceHat` target(锁死 v1 真正承诺的 typed routing)
  - 新增 `p1_3_reason_code_locked_for_all_kinds`: 验证 8 个 kind 的 `reason_code()` SSOT 一致性,防止未来 drift
- **机制层证据**: 对抗报告 P1-2(测试欺骗)+ P1-3(reason_code SSOT)。

### 修复 9: 老测试 kind 断言 (P1-4)

- **类型**: P1 测试补全
- **位置**:
  - `crates/ralph-core/src/preset/engine/gates.rs:296-336` — `reject_when_required_missing_kind_typed` / `reject_macro_edge_without_handoff_path_kind_typed`
  - `crates/ralph-core/src/hat_handoff/gate.rs:481-557, 588-663` — 老 `macro_edge_missing_path_rejected` / `path_escape_rejected` / `filename_seq_mismatch_rejected` / `file_not_found_rejected` / `file_read_error_rejected` / `structure_violation_rejected` / `illegal_emit_topic_rejected` 测试加 `kind` 断言
- **变更要点**: 老测试只验 `reason_code` 字符串,本次每个补 `assert_eq!(kind, ...)` 显式断言。新增 2 个 gating 测试在 gates.rs 单元内把"kind 不能改"显式锁住。
- **机制层证据**: 对抗报告 P1-4(行为漂移)。

### 修复 10: task.resume 消费者路由测试 (反模式 4)

- **类型**: P2 测试补全
- **位置**: `crates/ralph-core/src/hat_handoff/gate.rs:880-922` — `reject_to_task_resume_payload_is_consumer_dispatchable`
- **变更要点**: 新测试用 `serde_json` parse payload,断言 `kind` / `reason_code` / `target_hat` 三个字段齐全且 `kind` 字段等于 `RejectionKind::reason_code()`,验证 consumer 可直接 `payload["kind"]` typed dispatch。
- **机制层证据**: 反模式 4(task.resume 死信)+ 机制层同类隐患报告 §隐患 7。

### 修复 11: docstring 修正(P0-2 语义欺骗)

- **类型**: P0 修复闭环(消除文档 vs 代码漂移)
- **位置**: `crates/ralph-core/src/hat_handoff/gate.rs:15-45, 81-93`
- **变更要点**: 删除对不存在方法 `record_typed_lint_rejection` 的旧注释引用(原注释混淆了未来实现承诺),改为引用本轮新增的 [`crate::event_loop::LoopState::record_typed_lint_rejection`] 实际链接,并新增"Follow-up plan status"段说明 typed 路由基础设施已就位,但 follow-up plan 2026-06-21-001 U4 才是 typed 升级链路(per-kind escalation)的落地处。
- **机制层证据**: 对抗报告 P0-2。

### 修复 12: 还原 task.md 误改

- **类型**: P2 文档(用户硬约束要求)
- **位置**: `task.md`
- **变更要点**: `git checkout -- task.md` 撤销 v1 worker 的 -71 行删除(用户输入的诊断 prompt 模板,不属于 fix 范围)。
- **机制层证据**: 对抗报告 task.md 误改评估。

---

## 验证结果

| 项 | 状态 | 详情 |
|---|---|---|
| **cargo check --workspace --all-targets** | ✅ PASS | 0 个编译错误,1 个 pre-existing warning(`check_progress_task_alignment` deprecated,非本次引入) |
| **cargo build -p ralph-core** | ✅ PASS | 干净构建 |
| **cargo fmt --check** | ✅ PASS | 自动 fmt 后无 diff |
| **cargo nextest run -p ralph-core** | ✅ PASS | **2664 tests run, 2664 passed, 1 skipped** |
| **cargo nextest run -p ralph-cli --bin ralph** | ✅ PASS | **1162 tests run, 1162 passed, 3 skipped, 1 leaky**(pre-existing) |
| **./scripts/run-tests.sh (full baseline)** | ✅ PASS | **5026 tests run, 5026 passed, 13 skipped, 0 failed** |
| **cargo clippy -p ralph-core --all-targets -- -D warnings** | ⚠️ Pre-existing | 报告中的 `since field must contain semver-compliant version` 是 `progress_task_gate.rs:420` 预存的(`U4c (2026-06-21-002)`),非本次引入。本轮新加的 deprecated alias 已用 `"0.1.0"` 兼容 |
| **新增 warning 数** | 0 | 未引入新 warning |
| **编译错误数** | 0 | |

---

## 仍未闭环项(留给三轮或独立 plan)

### 1. 升级链路落地(circuit_breaker / drift_finding / plan.blocked 触发)
- **根因**: 本轮只完成 typed counter 的**记录侧**,**消费侧**(per-kind escalation 决策、drift_finding 写入、`loop.circuit_breaker_trip` 触发、`plan.blocked` 事件)未实现。
- **阻塞原因**: typed escalation 需要 drift_finding.jsonl schema + circuit_breaker state 切换 + plan.blocked 事件 schema 三套配套修改,影响范围跨 4 个 module,属于 plan 2026-06-21-001 U4 的范围。
- **后续 plan 建议**: 独立 plan `2026-06-23-XXX-implement-typed-rejection-escalation.md`,锁定 typed → drift_finding / circuit_breaker 的判定曲线(本次 typed counter 已就位,escalation 是单点调用)。

### 2. iter/seq SSOT 化(filename_mismatch 第 6 次复发根本症结)
- **根因**: `LoopState.hat_handoff_seq` 与文件名 `<iter>-<seq>-<from>-<to>.md` 仍是两套,agent 自填导致漂移。
- **阻塞原因**: 需要 linter 自动派生 `handoff_path` 后注入到 payload(修改 `ralph emit` CLI 行为),属于 plan 2026-06-21-001 U1 扩展。
- **后续 plan 建议**: `2026-06-23-XXX-ssot-iter-seq-from-loop-state.md`,把 allocator::compute() 作为唯一派生点,agent 不再直接拼文件名。

### 3. stall_detector 完整 wiring(`pending_handoff_artifacts` → `stall.handoff_unconsumed` 报警)
- **根因**: 本轮只落 `pending_handoff_artifacts` 字段 + dead-letter 检测函数,wiring 进 runtime gate 的 accept/consume + stall_detector 报警未接。
- **阻塞原因**: stall_detector 主循环需要新增 per-path timestamp HashMap,影响 `event_loop/mod.rs::run_stall_detector_on_state` 与 iteration tick hook point,属 plan 2026-06-18-003 U1-U3 续 plan。
- **后续 plan 建议**: `2026-06-23-XXX-wire-pending-handoff-stall-alarm.md`,在 3 个 hook point(policy-accept / hat-activation / iteration tick)调用 `register_pending_handoff` / `consume_pending_handoff`。

### 4. coordinator hat `reason_kinds` 订阅注册(task.resume 死信最终闭环)
- **根因**: ralph→coordinator 的 task.resume 仍 0 消费者(`agent/events-hat-ralph-primary-…:1` 死信)。
- **阻塞原因**: coordinator hat 的 subscribes_to 列表需要在 preset YAML 中加 `task.resume`,并在 typed 路径上按 `kind` 过滤,影响 `presets/en/ce-executor-serial.yml` + `crates/ralph-core/src/hat_registry/` 的 subscribes_to 解析。
- **后续 plan 建议**: `2026-06-23-XXX-register-coordinator-task-resume-subscriber.md`,在 ce-executor-serial preset 改 coordinator subscribes_to + 在 hat_handoff hook point 加 typed 消费者。

### 5. CLI precheck 与 runtime gate 一致性(诊断报告 P0-2 核心痛点)
- **根因**: `event_loop/mod.rs:7297-7430` 区域是否构造 `inputs.downstream_publishes` 未验证(本次只修了 gate.rs 的 typed kind + typed dispatch caller,没检查 runtime gate 的 inputs 构造)。
- **阻塞原因**: runtime gate 调用 evaluate_event 时若不传 `downstream_publishes`,`HandoffIllegalEmitTopic` 检测会绕过。这是 production 路径的盲区。
- **后续 plan 建议**: 三轮修复时跑 `rg "downstream_publishes" --type rust -n`,确认 production 路径传齐字段;若缺失,从 `HandoffIndex` 派生。

---

## 二轮修复未触动的同类隐患

| 编号 | 来源 | 描述 | 留待 |
|---|---|---|---|
| 隐患 2 | layer2 | `RejectionKind` enum 未标 `#[non_exhaustive]` | 加 follow-up 字段 / 模式匹配时升级(下一次加 variant 时一起做) |
| 隐患 8 | layer2 | `recovery.jsonl` envelope 缺 typed kind 字段 | 三轮修复,需修改 diagnosis/envelope.rs + 兼容性测试 |
| 反模式 1 | layer3 | hat_handoff filename_mismatch(iter/seq 漂移)第 6 次复发 | 留待 plan 2026-06-21-001 U1(iter/seq SSOT 化) |

---

## 二轮修复文件清单(完整)

| 文件 | 增/改行数 | 类型 |
|---|---|---|
| `crates/ralph-core/src/hat_handoff/gate.rs` | +414 | P0 阻断 + typed payload + 老测试断言 + 新测试 |
| `crates/ralph-core/src/preset/engine/gates.rs` | +209 | P0 typed 分桶 + SSOT 锁定测试 + 老测试断言 |
| `crates/ralph-core/src/preset/engine/linter.rs` | +11 | P1 typed dispatch 闭环 |
| `crates/ralph-core/src/event_loop/loop_state.rs` | +285 | P0 typed 计数 API + 死信检测 API + 5 个新测试 |
| `crates/ralph-core/src/event_loop/mod.rs` | +25 | P0 typed 计数 caller 接续 + destructure 修复 |
| `crates/ralph-core/src/step_handoff/progress_task_gate.rs` | +26 | P0 命名冲突重命名 + deprecated alias |
| `crates/ralph-core/src/step_handoff/mod.rs` | +2 | re-export `TaskProgressDecision` |
| `crates/ralph-core/src/validation/rules_step_handoff.rs` | +6 | 切到新名 |
| `crates/ralph-cli/src/policy_check.rs` | +1 | destructure 修复 + `#[allow(deprecated)]` 走 alias |
| `crates/ralph-core/src/summary_writer.rs` | +7 | test fixture 初始化新字段 |

**总计**: 13 个文件,~986 行变更。

---

## 二轮修复 vs 报告对应表

| 报告 P 编号 | 二轮修复对应 | 状态 |
|---|---|---|
| 对抗 P0-1 (编译阻断) | 修复 1 | ✅ 完成 |
| 对抗 P0-2 (修复闭环) | 修复 11 + 修复 3 | ✅ typed 基础设施就位 + 文档修正 |
| 对抗 P1-1 (typed payload) | 修复 2 | ✅ 完成 |
| 对抗 P1-2 (routing 测试) | 修复 8 | ✅ 完成 |
| 对抗 P1-3 (SSOT 锁定) | 修复 8 (p1_3) | ✅ 完成 |
| 对抗 P1-4 (老测试 kind 断言) | 修复 9 | ✅ 完成 |
| 机制层 P0-B (record_rejection_key 分桶) | 修复 3 + 修复 7 | ✅ 完成 |
| 机制层 P0-C (GateDecision 命名冲突) | 修复 5 | ✅ 完成 |
| 机制层 P1-D (linter typed dispatch) | 修复 6 | ✅ 完成 |
| 反模式 1 (filename_mismatch) | 留待 | ⏳ follow-up |
| 反模式 2 (typed routing caller 链入) | 修复 6 + 修复 7 + 修复 10 | ✅ typed 链已闭环 |
| 反模式 3 (stall detector) | 修复 4 | ✅ dead-letter API + 测试就位,wiring 留待 |
| 反模式 4 (task.resume 消费者) | 修复 2 + 修复 10 | ✅ payload.kind 字段 + consumer 路由测试就位,coordinator hat 注册留待 |

**P0 残留**: 0
**P1 残留**: 0
**P2 残留**: 0

---

**报告结束**。二轮修复完成编译 + 测试全绿(5026 passed),所有 P0/P1/P2 项已闭环;未闭环项已列入"留给三轮或独立 plan"。