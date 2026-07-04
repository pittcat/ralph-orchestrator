---
title: "fix: ce-executor-serial silent-success P0/P1 闭环修复（adversarial 维度 + dedup fallback + hard-reject 全栈）"
type: fix
status: planned
date: 2026-07-04
created: 2026-07-04
execution_model: strictly-sequential-atomic-tdd
related_plans:
  - docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md
  - docs/plans/2026-07-04-002-fix-opac-p0-p1-p2-issues-plan.md
  - docs/plans/2026-07-04-003-fix-task-cli-acl-coordinator-hats-and-preset-narrowing-plan.md
origin: docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md
---

# fix: ce-executor-serial silent-success P0/P1 闭环修复

## 执行模型（强制）

```
U1 ──闭环──> U2 ──闭环──> U3 ──闭环──> U4 ──闭环──> U5 ──闭环──> U6 ──闭环──> U7 ──闭环──> U8 ──闭环──> U9
       ↑ 每个 Unit 必须 RED → GREEN → REFACTOR → 验收命令 exit 0 才允许进入下一 Unit
```

| 规则 | 含义 |
|------|------|
| **严格串行** | 同一时间只做一个 Unit；Unit N 测试全绿前禁止打开 Unit N+1 的 RED |
| **绝对隔离** | 每个 Unit 只改列出的文件；测试只断言本 Unit 的输入→输出；禁止跨 Unit 集成测试 |
| **禁止前向依赖** | Unit N 不得 import/调用 Unit N+1 才存在的符号；不得写「等 Unit X 做完再测」 |
| **原子 TDD** | 每个 Unit 内部：**RED → GREEN → REFACTOR → 验收命令 exit 0**；边界问题在本 Unit 完结 |
| **dependency 引用** | U7 显式依赖 `003 plan U7`（必须先合并）；U9 跑 5 次 SC1 金丝雀回归 |

**验收命令统一入口**（每个 Unit 末尾只跑自己的子集）：

```bash
cargo nextest run -p ralph-cli --bin ralph -- <本 Unit 测试名 substring>
# 或 ralph-core 包：
cargo nextest run -p ralph-core -- <本 Unit 测试名 substring>
```

---

## 单元总览

| Unit | 交付物 | P0/P1 | 测试聚焦（仅本 Unit） |
|------|--------|-------|----------------------|
| **1** | `LoopInspectView.loop_anchor` 字段 + `LoopAnchorView` + schema bump v2 | **P0-1** | loop_anchor 在 attached / unattached 两种状态下的序列化 |
| **2** | `PolicyDecision::AcknowledgeAndForward` 变体 + `review.dimensions.complete` dedup 拒收走 fallback | **P0-2** | 单元 enum 5 变体 + dedup 拒收不触发 task.resume |
| **3** | preset `all_dimensions_failed` 硬门改"全 6 failed" + review-trace.json 字段写入要求 | **P0-3** | preset_lint `FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD` + trace schema |
| **4** | preset coordinator routing 注释升级 hard rule + `FINDING_REVIEW_COMPLETE_MISROUTED` lint | **P0-4** | preset_lint 扫描 coordinator instructions 含显式 `findings_count==0` 路由 |
| **5** | `audit_file_modifications` 改 hard-reject（RejectWithResume）+ 新增 scope_violation policy | **P0-5** | dimension-reviewer 6 次修改 plan.md frontmatter 全部被拒 |
| **6** | `DuplicateWorkDoneHint::ReviewDimensionsComplete` 新增 + reason_code 完整分离 | **P1-2** | review.dimensions.complete 重复 dedup 返回 distinct reason_code |
| **7** | 用户工作区 `ralph-e2e-serial/ralph.yml` 删除 `coordinator_hats` 段 | **P1-3** | 删除后 OPAC U7 收窄生效，executor 不能创建任务 |
| **8** | preset `mechanism.flow.unit_loop.body` 加 `review.complete` + `exempt_topics` 双轨清理 | **P1-4 + P2-1** | flow_declaration.rs lint 通过 + exempt_topics SSOT 单一 |
| **9** | skill 文档同步 + drift 脚本 + 5 次 SC1 金丝雀回归 | 全部 | 5 次同 plan 同 prompt run，silent-success 永不复发 |

**关键依赖关系**：
- **U7 强依赖**：`docs/plans/2026-07-04-003-fix-task-cli-acl-coordinator-hats-and-preset-narrowing-plan.md` 的 **Unit 7** 必须先合并（preset `coordinator_hats` 收窄到 `[coordinator, progress-steward]` + preset_lint 绿）
- **U3 软依赖 U2**：synthesizer 全失败语义改写依赖 dedup 后不触发 plan.blocked 风暴（避免 race）
- **U4 软依赖 U3**：coordinator routing 升级 hard rule 需要 synthesizer 已输出正确 verdict 语义

---

## Unit 1 — `LoopInspectView.loop_anchor` 字段补全

### 范围（孤岛）

- **只改**：
  - `crates/ralph-cli/src/commands/inspect.rs`（`LoopInspectView` struct、`inspect_loop_command` 构造、`build_loop_anchor_summary` helper、`LOOP_INSPECT_SCHEMA_VERSION` bump v1 → v2、相关测试）
- **禁止**：改 `LoopStateSnapshot` 持久化、`loop_state.rs` setter、`state_projector/orchestrator_context.rs` 注入逻辑（其他单元自然衔接）
- **测试 mod**：inspect.rs 内 `#[cfg(test)] mod loop_anchor_inspect_tests`

### RED（先写测试，预期失败）

```rust
#[cfg(test)]
mod loop_anchor_inspect_tests {
    // test_loop_inspect_schema_version_bumped_to_v2
    // test_inspect_loop_view_includes_loop_anchor_when_attached
    // test_inspect_loop_view_omits_loop_anchor_when_unattached
    // test_loop_anchor_warning_when_unattached
}
```

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_loop_inspect_schema_version_bumped_to_v2` | 常量检查 | `LOOP_INSPECT_SCHEMA_VERSION == "loop_inspect.v2"` |
| `test_inspect_loop_view_includes_loop_anchor_when_attached` | `loop_state` mock 含 plan_path/plan_name/plan_baseline_sha/loop_start_sha/attached_at | JSON 序列化含 `loop_anchor` 字段，5 个子字段非 null |
| `test_inspect_loop_view_omits_loop_anchor_when_unattached` | `loop_state` mock 无 anchor | JSON 序列化**不含** `loop_anchor` 字段（`skip_serializing_if = "Option::is_none"`）|
| `test_loop_anchor_warning_when_unattached` | 同上 | `warnings` 数组含 `"loop_anchor not attached"` |

```bash
cargo nextest run -p ralph-cli --bin ralph -- loop_anchor_inspect_tests
# 预期：全 FAIL（struct 尚无 loop_anchor 字段）
```

### GREEN

1. 定义 `LoopAnchorView { plan_path: PathBuf, plan_name: String, plan_baseline_sha: Option<String>, loop_start_sha: Option<String>, attached_at: chrono::DateTime<chrono::Utc> }`
2. `LoopInspectView` 加 `loop_anchor: Option<LoopAnchorView>` + `#[serde(skip_serializing_if = "Option::is_none")]`
3. `build_loop_anchor_summary(loop_state: &LoopState) -> Option<LoopAnchorView>` helper：从 `loop_state.plan_baseline_sha` / `loop_state.loop_start_sha` / `event_loop::loop_state.rs:461, 469` 读取
4. `LOOP_INSPECT_SCHEMA_VERSION` bump `"loop_inspect.v1"` → `"loop_inspect.v2"`（强制下流解析升级）
5. `inspect_loop_command` 在 anchor 未 attach 时填 `None` 并在 `warnings` push `"loop_anchor not attached; preset hats requiring loop_anchor will receive null"`
6. 跑测试至全绿

### REFACTOR

- 检查所有 `inspect_loop_view_*` 测试是否需要更新 schema version 断言（`inspect.rs:1395-1397` 的 `loop_inspect_schema_version_pinned` 测试）
- helper 复用 `build_supervisor_summary` 的 builder 模式（`inspect.rs:474-537` 参考）

### 验收（Unit 1 完结门槛）

```bash
cargo nextest run -p ralph-cli --bin ralph -- loop_anchor_inspect_tests
cargo nextest run -p ralph-cli --bin ralph -- inspect_loop_view
```

**完结定义**：4 个新测试 + 所有 inspect_loop_view_* 测试全绿；**未**要求 preset 修改（U3/U4 单元衔接）。

---

## Unit 2 — `PolicyDecision::AcknowledgeAndForward` + dedup fallback

### 前置

Unit 1 完结。

### 范围（孤岛）

- **只改**：
  - `crates/ralph-core/src/event_policy.rs`（`PolicyDecision` enum 新增 `AcknowledgeAndForward` 变体、`review.dimensions.complete` dedup 拒收分支返回新变体、相关 dedup 测试）
- **禁止**：改 `event_loop/mod.rs` 的 match 分支（U5 才统一接入）、改 preset、加新 lint
- **测试 mod**：event_policy.rs 内 `#[cfg(test)] mod dedup_fallback_tests`

### RED（先写测试，预期失败）

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_policy_decision_has_acknowledge_and_forward_variant` | enum shape 检查 | `PolicyDecision::AcknowledgeAndForward(PolicyFinding)` 存在 |
| `test_review_dimensions_complete_dedup_hit_returns_acknowledge_and_forward` | 同一 dedup_key 第二次 emit | 返回 `AcknowledgeAndForward(PolicyFinding{reason_code: "duplicate_review_dimensions_complete", ...})`，**不**返回 `RejectWithResume` |
| `test_review_dimensions_complete_first_emit_still_accepts` | 第一次 emit | 仍返回 `Accept` |
| `test_other_topic_dedup_still_rejects_with_resume` | `work.done` dedup 第二次 | 仍返回 `RejectWithResume`（保留原行为）|

```bash
cargo nextest run -p ralph-core -- dedup_fallback_tests
# 预期：全 FAIL（enum 尚无新变体）
```

### GREEN

1. `PolicyDecision` enum 加 `AcknowledgeAndForward(PolicyFinding)` 变体（位置在 `Accept` 之后）
2. `event_policy.rs:1486-1499` `review.dimensions.complete` dedup 命中分支改返回 `AcknowledgeAndForward` 而非 `RejectWithResume`
3. `PolicyFinding` 复用现有结构（`reason_code` 字段填 `"duplicate_review_dimensions_complete"` 占位，U6 单元再细分）
4. 跑测试至全绿

### REFACTOR

- 检查 `event_loop/mod.rs` 所有 match `PolicyDecision` 的站点（grep `match.*PolicyDecision`），新增变体的 `_` 兜底必须是 `Accept`（不能让循环静默）
- 当前已知 `PolicyDecision` 在 `event_policy.rs:170` enum 定义本身已经是 4 态，加第 5 态不影响 sealed-style

### 验收（Unit 2 完结门槛）

```bash
cargo nextest run -p ralph-core -- dedup_fallback_tests
cargo nextest run -p ralph-core -- event_policy
```

**完结定义**：4 个新测试 + 所有现有 event_policy 测试全绿；**未**要求 event_loop 接入新变体（U5 统一接入）。

---

## Unit 3 — `all_dimensions_failed` 硬门改"全 6 failed"

### 前置

Unit 2 完结。

### 范围（孤岛）

- **只改**：
  - `presets/en/ce-executor-serial.yml:2355-2363`（review-synthesizer `all_dimensions_failed` 硬门措辞）
  - `presets/en/ce-executor-serial.yml:1641-1650`（review-coordinator trace 写入字段必含 `loop_id / plan_path / plan_name`）
  - `crates/ralph-core/src/preset_lint/review_synthesizer_block_guard.rs`（新子模块）
  - `crates/ralph-core/src/preset_lint/finding_id.rs`（新增 `FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD`）
  - `crates/ralph-core/src/preset_lint/mod.rs`（`pub mod` + `pub use` + `run_preset_lint` 调用链）
- **禁止**：改 review-synthesizer Rust 实现（无独立 Rust 模块，仅 preset instructions 驱动）；改 dedup 逻辑；改 event_loop
- **测试 mod**：`review_synthesizer_block_guard.rs` 内 `mod review_synthesizer_block_guard_tests`

### RED（先写测试，预期失败）

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_no_findings_when_no_block_guard_text` | preset `review-synthesizer` instructions 不含 "all_dimensions_failed" 字样 | 0 findings |
| `test_warning_when_block_guard_text_uses_vague_word` | 含 "All dimensions failed" 缺 "全 6" 显式约束 | 1 finding `FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD` warn |
| `test_no_findings_when_block_guard_text_explicit_six_dimensions` | 含 "仅当全部 6 维度 `status == \"failed\"` 时才 plan.blocked" | 0 findings |
| `test_review_trace_required_fields_present` | preset `review-coordinator` trace 写入含 `loop_id / plan_path / plan_name` | 0 findings |
| `test_review_trace_missing_loop_id_field_warns` | trace schema 缺 `loop_id` | 1 finding |

```bash
cargo nextest run -p ralph-cli --bin ralph -- review_synthesizer_block_guard
# 预期：全 FAIL（finding_id 尚无 + preset 仍用旧措辞）
```

### GREEN

1. preset `ce-executor-serial.yml:2355-2363` 改为：
   ```yaml
   # All dimensions failed check:
   # ONLY when all 6 dimensions have status == "failed" publish plan.blocked(reason="all_dimensions_failed").
   # Mixed (some done + some failed): route through normal verdict path; failed dimensions count toward residual_risks.
   ```
2. preset `ce-executor-serial.yml:1641-1650` review-coordinator trace 写入 schema 改为必含 `loop_id / plan_path / plan_name / verified_at / review_coordinator_invocation`
3. `finding_id.rs` 新增 `pub const FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD: &str = "preset.review_synthesizer_block_guard";`
4. 新增 `review_synthesizer_block_guard.rs` 实现 `pub fn check_review_synthesizer_block_guard(config: &RalphConfig, strictness: LintStrictness) -> Vec<LintFinding>`
5. `preset_lint/mod.rs` 加 `pub mod review_synthesizer_block_guard; pub use review_synthesizer_block_guard::{FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD, check_review_synthesizer_block_guard};`，并在 `run_preset_lint` 调用链插入
6. 跑测试至全绿

### REFACTOR

- helper 函数复用 `dimension_reviewer_write_paths.rs:25-56` 的子模块模式（preset config + strictness 输入 + 返回 Vec<LintFinding>）

### 验收（Unit 3 完结门槛）

```bash
cargo nextest run -p ralph-cli --bin ralph -- review_synthesizer_block_guard
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
# SSOT byte-equality（保证 preset 嵌入版本与文件版本一致）
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
```

**完结定义**：5 个新测试 + preset_lint 全套 + SSOT byte-equality 全绿；**未**要求 coordinator routing 强化（U4 衔接）。

---

## Unit 4 — Coordinator routing 注释升级 hard rule + 新 lint

### 前置

Unit 3 完结。

### 范围（孤岛）

- **只改**：
  - `presets/en/ce-executor-serial.yml:1006-1008`（coordinator `fix_plan_file == "null"` 注释升级 hard rule）
  - `crates/ralph-core/src/preset_lint/review_complete_misrouted.rs`（新子模块）
  - `crates/ralph-core/src/preset_lint/finding_id.rs`（新增 `FINDING_REVIEW_COMPLETE_MISROUTED`）
  - `crates/ralph-core/src/preset_lint/mod.rs`（`pub mod` + `pub use` + `run_preset_lint` 调用链）
- **禁止**：改 coordinator Rust 实现（无独立 Rust 模块）；改 synthesizer 语义；改 dedup 逻辑
- **测试 mod**：`review_complete_misrouted.rs` 内 `mod review_complete_misrouted_tests`

### RED（先写测试，预期失败）

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_no_findings_when_coordinator_has_findings_count_zero_rule` | coordinator instructions 含 "`findings_count == 0` always routes to `plan.complete(verdict=\"pass_with_residuals\")` regardless of `verdict` field" | 0 findings |
| `test_warning_when_coordinator_only_has_fix_plan_file_null_rule` | instructions 仅含 "`fix_plan_file == \"null\"` route to plan.complete"，**缺** `findings_count == 0` 显式约束 | 1 finding warn |
| `test_error_when_coordinator_only_routes_on_verdict_field` | instructions 含 "if verdict=blocked then plan.blocked"，**缺** `findings_count` 路由约束 | 1 finding error |
| `test_no_findings_when_other_hat_has_vague_rule` | non-coordinator hat 含模糊 routing 描述 | 0 findings（lint 仅约束 coordinator）|

```bash
cargo nextest run -p ralph-cli --bin ralph -- review_complete_misrouted
# 预期：全 FAIL（finding_id 尚无 + preset 仍用注释）
```

### GREEN

1. preset `ce-executor-serial.yml:1006-1008` 改为：
   ```yaml
   # HARD RULE — review.complete routing:
   # When review.complete payload has findings_count == 0, ALWAYS publish
   # plan.complete(verdict="pass_with_residuals", final_findings_count=0),
   # REGARDLESS of the verdict field. The verdict field reflects synthesizer
   # technical verdicts (which may have been escalations from a partial-failed
   # dimension); findings_count=0 means there are no real code defects to fix.
   # Only when findings_count > 0 does the verdict field drive plan.blocked routing.
   ```
2. `finding_id.rs` 新增 `pub const FINDING_REVIEW_COMPLETE_MISROUTED: &str = "preset.review_complete_misrouted";`
3. 新增 `review_complete_misrouted.rs` 实现 `pub fn check_review_complete_misrouted(config: &RalphConfig, strictness: LintStrictness) -> Vec<LintFinding>`：扫描 coordinator hat 的 `instructions` 文本，**正则匹配** 是否含 `findings_count == 0` + `plan.complete` 显式短语
4. `preset_lint/mod.rs` 加 `pub mod review_complete_misrouted; pub use review_complete_misrouted::{FINDING_REVIEW_COMPLETE_MISROUTED, check_review_complete_misrouted};`，并在 `run_preset_lint` 调用链插入
5. 跑测试至全绿

### REFACTOR

- 正则表达式提取到 `static REGEX_FINDINGS_COUNT_RULE: Lazy<Regex>` 复用（避免每次 lint 编译）
- helper 函数复用 `dimension_reviewer_write_paths.rs:25-56` 模式

### 验收（Unit 4 完结门槛）

```bash
cargo nextest run -p ralph-cli --bin ralph -- review_complete_misrouted
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
```

**完结定义**：4 个新测试 + preset_lint 全套 + SSOT 全绿；**未**要求 enforce_hat_scope 升级（U5 衔接）。

---

## Unit 5 — `audit_file_modifications` 改 hard-reject

### 前置

Unit 2 完结（`PolicyDecision` 新变体可复用）。

### 范围（孤岛）

- **只改**：
  - `crates/ralph-core/src/event_loop/mod.rs:7652-7719`（`audit_file_modifications` 改 hard-reject）
  - `crates/ralph-core/src/event_loop/audit.rs`（`AuditSeverity` 新增 `BlockLoop { reason: String }` 变体）
  - `crates/ralph-core/src/event_loop/types.rs:158-167`（`TerminationReason` 新增 `ScopeViolationHardRejected` 变体）
  - `crates/ralph-core/src/event_loop/mod.rs:2141-2157`（`check_termination` 末尾接入新终止原因）
- **禁止**：改 preset；改 dimension-reviewer instructions（已有 HARD RULE 831d0626 兜底）；改其它 audit 路径
- **测试 mod**：`event_loop/mod.rs` 内 `mod scope_violation_hard_reject_tests`

### RED（先写测试，预期失败）

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_audit_severity_has_block_loop_variant` | enum shape | `AuditSeverity::BlockLoop { reason: String }` 存在 |
| `test_dimension_reviewer_writing_plan_md_frontmatter_emits_block_loop_audit` | mock dimension-reviewer 调 `Edit` 改 `docs/plans/X.md` frontmatter | 返回 `AuditSeverity::BlockLoop { reason: "scope_violation" }` 而非 `Fail { add_failures: 1 }` |
| `test_other_hat_writing_plan_md_still_emits_fail_audit` | mock coordinator 调 `Edit` 改 `docs/plans/X.md` | 仍返回 `Fail { add_failures: 1 }`（scope hard-reject 仅 dimension-reviewer）|
| `test_check_termination_handles_scope_violation_hard_rejected` | mock `TerminationReason::ScopeViolationHardRejected` | loop 立即终止，返回 LOOP_COMPLETE reason=`scope_violation_hard_rejected` |
| `test_block_loop_severity_does_not_increment_consecutive_failures` | 同 test 2 路径 | `consecutive_failures` 不递增（避免影响后续 unit dispatch）|

```bash
cargo nextest run -p ralph-core -- scope_violation_hard_reject
# 预期：全 FAIL（AuditSeverity 尚无 BlockLoop 变体）
```

### GREEN

1. `AuditSeverity` enum 加 `BlockLoop { reason: String }` 变体（位置在 `BlockLoop { reason: "scope_violation" }` 现有块之后，参考 `event_loop/types.rs:158-167` 已有 `TerminationReason::ScopeViolationCircuitBreakerTripped`）
2. `audit_file_modifications` 在 `dimension-reviewer` 路径（已用 `allowed_write_paths` 静态 lint 拦截的 hat）触发时返回 `AuditSeverity::BlockLoop { reason: "scope_violation" }` 而非 `Fail { add_failures: 1 }`
3. `AuditDispatcher::dispatch` 新增 BlockLoop 分支：设置 `TerminationReason::ScopeViolationHardRejected`，**不**递增 `consecutive_failures`
4. `check_termination` 末尾读取 `TerminationReason::ScopeViolationHardRejected` 触发 LOOP_COMPLETE 终止，reason 为 `"scope_violation_hard_rejected"`
5. 跑测试至全绿

### REFACTOR

- AuditSeverity 现有 `Fail { add_failures }` 与新 `BlockLoop { reason }` 共享公共字段提取 trait（如 `pub fn is_terminal(&self) -> bool`）
- check_termination 路径已有 circuit breaker 触发模式（`event_loop/types.rs:158-167`），BlockLoop 复用同一终止路径

### 验收（Unit 5 完结门槛）

```bash
cargo nextest run -p ralph-core -- scope_violation_hard_reject
cargo nextest run -p ralph-core -- audit_file_modifications
cargo nextest run -p ralph-core -- check_termination
```

**完结定义**：5 个新测试 + 所有 audit/termination 测试全绿；**未**要求 preset 修改。

---

## Unit 6 — `DuplicateWorkDoneHint` 完整分离

### 前置

Unit 2 完结。

### 范围（孤岛）

- **只改**：
  - `crates/ralph-core/src/event_policy.rs:95`（`DuplicateWorkDoneHint` enum 加 `ReviewDimensionsComplete` 变体）
  - `crates/ralph-core/src/event_policy.rs:140-155`（reason_code 归一映射：`ReviewDimensionsComplete` → `"duplicate_review_dimensions_complete"`）
  - `crates/ralph-core/src/event_policy.rs:1490`（`review.dimensions.complete` dedup 改用 `ReviewDimensionsComplete` hint）
- **禁止**：改 dedup 拒收分支（已 U2 改完）；改 lint；改 preset
- **测试 mod**：event_policy.rs 内 `#[cfg(test)] mod duplicate_reason_code_tests`

### RED（先写测试，预期失败）

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_duplicate_work_done_hint_has_review_dimensions_complete_variant` | enum shape | 5 个变体（`DuplicateStallBypass, DuplicateSameStep, ReviewDimensionDuplicate, ReviewDimensionsComplete`）|
| `test_review_dimensions_complete_duplicate_emits_distinct_reason_code` | 同 dedup_key 第二次 emit | 返回 finding `reason_code == "duplicate_review_dimensions_complete"`（非 `"duplicate_work_done"`）|
| `test_review_dimension_ready_duplicate_still_uses_review_dimension_duplicate` | 已有行为保持 | reason_code == `"duplicate_review_dimension_ready"` |
| `test_other_topics_dedup_still_use_duplicate_work_done` | `work.done` dedup | reason_code == `"duplicate_work_done"`（保留原行为）|
| `test_distinct_reason_codes_invariant` | 4 个 hint × 实际 reason_code 映射 | 4 个 distinct string（无重复归一）|

```bash
cargo nextest run -p ralph-core -- duplicate_reason_code
# 预期：全 FAIL（enum 尚无新变体）
```

### GREEN

1. `DuplicateWorkDoneHint` enum 加 `ReviewDimensionsComplete` 变体（位置在 `ReviewDimensionDuplicate` 之后）
2. reason_code 归一映射表加 `ReviewDimensionsComplete => "duplicate_review_dimensions_complete"`
3. `event_policy.rs:1490` `review.dimensions.complete` dedup 改返回 `ReviewDimensionsComplete` hint
4. 跑测试至全绿

### REFACTOR

- 把 4 个 hint → reason_code 的映射表抽成 `static REASON_CODE_MAP: &[(DuplicateWorkDoneHint, &str)]` 便于 future 扩展

### 验收（Unit 6 完结门槛）

```bash
cargo nextest run -p ralph-core -- duplicate_reason_code
cargo nextest run -p ralph-core -- event_policy
```

**完结定义**：5 个新测试 + 所有 event_policy 测试全绿。

---

## Unit 7 — 用户工作区 `ralph.yml` 删除 `coordinator_hats` 漂移

### 前置

**`docs/plans/2026-07-04-003-fix-task-cli-acl-coordinator-hats-and-preset-narrowing-plan.md` 的 Unit 7 必须先合并**（preset `coordinator_hats` 收窄到 `[coordinator, progress-steward]` + preset_lint 绿）。

### 范围（孤岛）

- **只改**：`/Users/pittcat/Dev/Rust/ralph-e2e-serial/ralph.yml:10-13`（删除 `coordinator_hats: [coordinator, executor]` 段，或改为 `[coordinator, progress-steward]`）
- **禁止**：改 preset；改 task_cli.rs（003 plan U1/U2 衔接）；改 ACL policy
- **测试**：本 Unit 不新增 Rust 测试，仅做静态验证

### RED（先验证前置，确认前置状态）

```bash
# 必须确认 003 plan U7 已合并到当前分支
git log --oneline -20 | grep "U7 coordinator_hats 收窄"
# 必须确认 preset 已收窄
grep -A 2 "coordinator_hats:" /Users/pittcat/Dev/Rust/ralph-orchestrator/presets/en/ce-executor-serial.yml | head -5
# 预期输出：coordinator_hats: [coordinator, progress-steward]

# 当前用户工作区 ralph.yml 必须仍含漂移（这是 RED 状态）
cat /Users/pittcat/Dev/Rust/ralph-e2e-serial/ralph.yml | grep -A 2 "coordinator_hats"
# 预期输出：coordinator_hats: [coordinator, executor]
```

### GREEN

1. **选项 A（推荐）**：删除 `/Users/pittcat/Dev/Rust/ralph-e2e-serial/ralph.yml` 的 `coordinator_hats` 整段（让 preset 默认生效）
2. **选项 B**：改为 `coordinator_hats: [coordinator, progress-steward]`
3. 跑预设 lint 确认：
   ```bash
   cd /Users/pittcat/Dev/Rust/ralph-e2e-serial && ralph --help | head -3
   ```

### REFACTOR

- 不需要重构（单文件修改）

### 验收（Unit 7 完结门槛）

```bash
# 1. 003 plan U7 已合并
git log --oneline -5

# 2. 用户 ralph.yml 不再含漂移
cat /Users/pittcat/Dev/Rust/ralph-e2e-serial/ralph.yml | grep "coordinator_hats" || echo "OK: drift removed"

# 3. 跑一次 dry-run 验证
cd /Users/pittcat/Dev/Rust/ralph-e2e-serial && ralph run --plan docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md --dry-run 2>&1 | head -20
```

**完结定义**：3 项验证全绿；**未**要求 Rust 代码变更。

---

## Unit 8 — `mechanism.flow.unit_loop.body` 拓扑 + `exempt_topics` 双轨清理

### 前置

Unit 2 完结（dedup 后不再触发风暴，可安全扩展拓扑）。

### 范围（孤岛）

- **只改**：
  - `presets/en/ce-executor-serial.yml:74-94`（`unit_loop` step 的 `body` 列表加 `review.complete`，**仅**在末 unit 时触发）
  - `presets/en/ce-executor-serial.yml:1522`（review-coordinator `exempt_topics` 块移除 `review.dimension.ready` / `review.dimensions.complete`，保持 SSOT 在 line 470-471）
  - `crates/ralph-core/src/preset_lint/flow_declaration.rs`（如有 `unit_loop.body` 元素校验，新增 `review.complete` 校验）
- **禁止**：改 event_loop 拓扑白名单实现；改其它 preset；改 hat routing
- **测试 mod**：`preset_lint/flow_declaration.rs` 内 `mod flow_review_complete_tests`

### RED（先写测试，预期失败）

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_flow_declaration_accepts_review_complete_in_unit_loop_body` | preset `unit_loop.body` 含 `review.complete` | 0 findings |
| `test_flow_declaration_warns_review_complete_in_first_unit_only` | preset `body` 在 first unit 含 `review.complete` | 1 finding（review.complete 仅允许在末 unit）|
| `test_review_coordinator_exempt_topics_does_not_contain_serial_walk_topics` | preset review-coordinator `exempt_topics` 不含 `review.dimension.ready` / `review.dimensions.complete` | 0 findings（SSOT 在 business_topics）|
| `test_review_coordinator_exempt_topics_warning_when_contains_serial_walk_topic` | preset review-coordinator `exempt_topics` 含 `review.dimension.ready` | 1 finding error |

```bash
cargo nextest run -p ralph-cli --bin ralph -- flow_review_complete
# 预期：全 FAIL（preset 仍缺 review.complete + exempt_topics 仍双轨）
```

### GREEN

1. preset `ce-executor-serial.yml:74-94` 的 `unit_loop.body` 列表加 `review.complete`(条件约束注释："ONLY when this unit_loop is the last unit AND test.passed fired")
2. preset `ce-executor-serial.yml:1522` review-coordinator `exempt_topics` 块移除 `review.dimension.ready` / `review.dimensions.complete`（保持 SSOT 单一在 line 470-471 的 `business_topics`）
3. `preset_lint/flow_declaration.rs`（如有）加 `review.complete` 元素校验（仅允许末 unit）
4. 跑测试至全绿

### REFACTOR

- 不需要重构（preset + lint 文本调整）

### 验收（Unit 8 完结门槛）

```bash
cargo nextest run -p ralph-cli --bin ralph -- flow_review_complete
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
```

**完结定义**：4 个新测试 + preset_lint 全套 + SSOT byte-equality 全绿。

---

## Unit 9 — Skill 文档同步 + drift 脚本 + 5 次 SC1 金丝雀回归

### 前置

Unit 1-8 全部完结。

### 范围（孤岛）

- **只改**：
  - `crates/ralph-core/data/ralph-tools-opac.md`（§Observe 阶段 schema 说明加 `loop_anchor` 字段）
  - `crates/ralph-core/data/ralph-tools-cmdref.md:144`（`ralph inspect loop` 命令速查加 `loop_anchor` 字段说明）
  - `scripts/check-cli-doc-drift.sh`（如已有 inspect 字段校验，新增 `loop_anchor` 字段引用）
  - 回归 run 记录写入 `docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md` 末尾作为闭环证据
- **禁止**：改 Rust 代码；改 preset；改 lint
- **测试**：5 次 SC1 金丝雀（不写新测试，跑 5 次同 plan 同 prompt run）

### RED → GREEN

不写 RED 测试（文档/脚本修改）。直接验证：

1. 文档同步：
   ```bash
   # 确认 ralph-tools-opac.md §Observe 已包含 loop_anchor
   grep "loop_anchor" /Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/data/ralph-tools-opac.md
   # 预期：含 loop_anchor.plan_path/plan_name/plan_baseline_sha/loop_start_sha/attached_at 说明
   ```

2. drift 脚本验证：
   ```bash
   bash /Users/pittcat/Dev/Rust/ralph-orchestrator/scripts/check-cli-doc-drift.sh
   # 预期：exit 0
   ```

3. **5 次 SC1 金丝雀回归**（关键回归锁）：
   ```bash
   for i in 1 2 3 4 5; do
     echo "=== SC1 run #$i ==="
     cd /Users/pittcat/Dev/Rust/ralph-e2e-serial
     ralph run --worktree --reuse-worktree \
       --plan docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md \
       --preset ce-executor-serial 2>&1 | tail -30
     echo "--- verdict ---"
     cat .ralph/loops.json | jq '.loops[-1].verdict'
     cat .ralph/events-history-*.jsonl | tail -1 | jq '.topic + " reason=" + (.payload.reason // "ok")'
   done
   ```

   **回归判据**（5 次必须全部满足）：
   - `verdict == "pass"` 或 `verdict == "pass_with_residuals"` **且** `final_findings_count >= 0`
   - **不**出现 `review.dimension.failed(reason="loop_anchor_not_found")`
   - **不**出现 `review.complete(verdict="blocked", findings_count=0)` 的自相矛盾组合
   - **不**出现 `coordinator.plan.blocked(reason="review_failed")` 当 `findings_count == 0` 时
   - `LOOP_COMPLETE` reason 含 `pass` 或 `pass_with_residuals`，**不**为 `review_failed` 或 `scope_violation_hard_rejected`

4. **基线回归**（验证没破坏其它包）：
   ```bash
   ./scripts/run-tests.sh
   ```
   包含：nextest + doctest + preset_lint strict + SSOT byte-equality + BDD scenarios + check-cli-doc-drift + validate-builtin-presets --strict

### 验收（Unit 9 完结门槛）

| 验收项 | 命令 | 期望 |
|--------|------|------|
| 文档同步 | `grep loop_anchor ralph-tools-opac.md` | 含字段说明 |
| drift 脚本 | `bash scripts/check-cli-doc-drift.sh` | exit 0 |
| SC1 金丝雀 5 次 | 上述 for 循环 | 5/5 全过 silent-success 判据 |
| 全量基线 | `./scripts/run-tests.sh` | exit 0 |

**完结定义**：4 项验收全绿，**本 plan 才算真正闭环**。

---

## 关键决策与权衡（Key Technical Decisions）

### KTD-1：scope hard-reject 范围仅 dimension-reviewer（U5）
- **决策**：`AuditSeverity::BlockLoop` 仅在 `dimension-reviewer` 触发 scope_violation 时启用，其它 hat 仍走 `Fail { add_failures: 1 }` 路径
- **理由**：本次 run 仅 dimension-reviewer 越权；扩大范围会误伤 coordinator hat 的合法 plan 修改
- **回滚**：`enforce_hat_scope: false` 全局开关保留（`config/loop_config.rs:224-228`），可立即关闭

### KTD-2：dedup fallback 仅对 `review.dimensions.complete` 启用（U2）
- **决策**：`AcknowledgeAndForward` 仅在 `event_policy.rs:1490` 分支返回，其它 dedup 仍走 `RejectWithResume`
- **理由**：本次 run 仅 `review.dimensions.complete` dedup 风暴；扩大范围会让 agent 收到 dedup reject 时无明确 signal
- **回滚**：分支条件收紧即可

### KTD-3：coordinator routing 升级为 hard rule 而非新增 enforcement（U4）
- **决策**：仅用 preset_lint 静态扫描 coordinator instructions 是否含 `findings_count == 0` 路由约束，**不**新增 runtime gate
- **理由**：runtime gate 已能正确处理 `findings_count == 0` + `plan.complete` 的合法路径；缺的是 agent 凭 `verdict=blocked` 误判的语义保护（lint 已覆盖）
- **回滚**：lint finding_id 注释即可

### KTD-4：003 plan U7 强依赖（U7）
- **决策**：U7 必须等 `003-fix-task-cli-acl-coordinator-hats-and-preset-narrowing-plan.md` 的 Unit 7 先合并，否则 `coordinator_hats: [coordinator, executor]` 漂移未消除，本 plan 修复不闭环
- **理由**：用户 ralph.yml 是单文件修改，但删除漂移前 003 U7 已收窄 preset，否则会出现"用户文件正确但 preset 仍是宽口径"的反模式
- **等待信号**：`git log --oneline -20 | grep "U7 coordinator_hats 收窄"` 必须非空

### KTD-5：5 次 SC1 金丝雀是真正的回归锁（U9）
- **决策**：U9 完结必须 5 次同 plan 同 prompt run 全部满足 silent-success 判据
- **理由**：单测 + preset_lint + SSOT 全过不等于 silent-success 真闭环（per `docs/achieved/solutions/integration-issues/hat_handoff_filename_mismatch_recurrence.md` "测试通过 ≠ 语义正确"经验）
- **失败回滚**：任何 1 次复发立即回滚对应 Unit 的 commit 并重新设计

---

## 系统级影响（System-Wide Impact）

| 受影响方 | 影响 | 应对 |
|----------|------|------|
| **agent hat prompts** | U1/U3/U4/U8 改 preset instructions，所有 hat 触发路径可能受影响 | U9 跑 5 次 SC1 金丝雀验证 |
| **CLI `ralph inspect loop`** | U1 新增 `loop_anchor` 字段，schema bump v2 | 旧脚本读 v1 字段仍兼容（`skip_serializing_if` 保证向后兼容） |
| **`ralph run` 行为** | U1/U3/U4/U5/U6/U8 联合改写事件流 | U9 SC1 金丝雀 + `./scripts/run-tests.sh` 全量基线 |
| **preset_lint 调用方** | U3/U4/U8 新增 3 条 lint finding | preset_lint 全套测试 + SSOT byte-equality |
| **BDD scenarios** | U1/U2/U3/U5/U6 新增 19 个测试场景 | U9 验证不破坏现有 67 个 BDD scenarios |
| **下游 consumer（e2e-serial 用户）** | U7 删除 ralph.yml 漂移 | 单文件修改，不影响 API |

---

## 风险与缓解（Risks & Dependencies）

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| **003 plan U7 未及时合并** | 中 | U7 阻塞 | 在 U7 入口显式等待 `git log` 信号；不强推 003 合并 |
| **U2 `AcknowledgeAndForward` 引入新 race** | 低 | dedup 拒收后下游误激活 | U2 单元测试覆盖 4 个 topic 类型；U5 audit 测试覆盖 race |
| **U5 hard-reject 误伤合法 hat** | 中 | dimension-reviewer 必要的 fix 写 plan.md 被拒 | 仅 dimension-reviewer 触发；其它 hat 仍走 Fail { add_failures } 路径；`enforce_hat_scope` 全局开关保留 |
| **U4 lint 误报** | 低 | coordinator instructions 改写困难 | lint 文本匹配留模糊边界（`fix_plan_file == "null"` 仍接受）；4 个测试用例覆盖正/反/边界 |
| **U9 SC1 金丝雀 5 次有 1 次复发** | 中 | 修复不闭环 | 立即回滚对应 Unit commit 并重新设计；不进入下一 Unit |

---

## 文档影响（Documentation Plan）

- **`crates/ralph-core/data/ralph-tools-opac.md`** §Observe 阶段加 `loop_anchor` schema 说明（U1 触发）
- **`crates/rollout-emit.md`** §5 precheck 不变（本次未触及 emit 路径）
- **`scripts/check-cli-doc-drift.sh`** 加 `loop_anchor` 字段引用检测（U9 触发）
- **`scripts/ralph-zsh-plugin.zsh`** 不变（本次未改 builtin preset）
- **本 plan** 是 docs/plans/2026-07-04-005+ 的 dependency（任何后续 serial preset 修改需引用）

---

## Phase 交付清单（按 Unit 完成顺序）

| Unit | Commit Subject 模板 | 包含 |
|------|---------------------|------|
| U1 | `fix(ralph-cli): U1 LoopInspectView.loop_anchor 字段 + schema v2` | inspect.rs struct + helper + bump v2 |
| U2 | `fix(ralph-core): U2 PolicyDecision::AcknowledgeAndForward + review.dimensions.complete dedup fallback` | event_policy.rs enum + dedup 拒收分支 |
| U3 | `fix(preset+lint): U3 review-synthesizer all_dimensions_failed 全 6 failed 语义 + trace 必含 loop_id` | preset yml + finding_id + review_synthesizer_block_guard.rs |
| U4 | `fix(preset+lint): U4 coordinator findings_count==0 路由 hard rule + FINDING_REVIEW_COMPLETE_MISROUTED` | preset yml + finding_id + review_complete_misrouted.rs |
| U5 | `fix(ralph-core): U5 enforce_hat_scope hard-reject (AuditSeverity::BlockLoop + TerminationReason::ScopeViolationHardRejected)` | audit.rs + types.rs + event_loop/mod.rs |
| U6 | `fix(ralph-core): U6 DuplicateWorkDoneHint::ReviewDimensionsComplete + reason_code 完整分离` | event_policy.rs enum + 映射表 |
| U7 | `fix(e2e-serial): U7 删除用户 ralph.yml coordinator_hats 漂移` | ralph-e2e-serial/ralph.yml |
| U8 | `fix(preset+lint): U8 unit_loop.body 加 review.complete + exempt_topics 双轨清理` | preset yml + flow_declaration.rs |
| U9 | `docs(opac)+scripts+regression: U9 skill 文档同步 + drift 脚本 + 5 次 SC1 金丝雀` | ralph-tools-opac.md + check-cli-doc-drift.sh + 5 次 run |

---

## 后续 follow-up（不在本 plan 范围）

- **`ce-executor-supervisor` preset 修复**(F-019 `review.wave.complete` 被拒 + 10 hat instructions 缺失,per `docs/plans/2026-07-04-001` review 报告) — 建议建独立 plan 005
- **`ralph 抢发 LOOP_COMPLETE` / process exit 未触发**(本次未触发,但 `event_loop/mod.rs:1667 + 2159` 仍是潜在风险) — 建议建独立 plan 006
- **`summary_writer.rs:296-303` scratchpad 路径错位**(本次未触发,但仍是 known issue) — 建议建独立 plan 007
- **`hat-channel 0 字节文件静默降级`**(`hat_channel.rs:79`) — 002 plan U4 已部分修复，验证回归即可
- **跨 preset SSOT 同步自动化**(F-PS-005) — 既有自动 sync 已建，本 plan 不涉及

---

## Phase 5.3 Confidence Check

| 检查项 | 状态 |
|--------|------|
| Plan depth 分类 | **Deep**(5 P0 + 7 P1 + 9 Units + 跨多模块 + 高风险 + 战略级) |
| Topic risk | **高**(event_policy / preset_lint / scope enforcement 多处协同改动) |
| Load-bearing external research | **无**(本地调研已覆盖) |
| Thin grounding | **无**(本地 patterns 充分) |
| **结论**:进入 **Deepening 模式**(auto) | 强 |

---

**Plan 写盘路径**：`/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/plans/2026-07-04-004-fix-ce-executor-serial-silent-success-p0-p1-plan.md`

**实施者**:Claude Code 主 Agent 严格按 Unit 顺序串行执行(对齐 003 plan 风格)

**回归锁**:Unit 9 完结的 5 次 SC1 金丝雀全部通过 + `./scripts/run-tests.sh` exit 0,才允许宣告"silent-success 真闭环"。