---
title: "fix: ce-executor-serial perky-maple review findings"
type: fix
status: active
date: 2026-06-18
origin: docs/plans/2026-06-18-004-fix-ce-executor-serial-perky-maple-orchestration-gaps-plan.md
---

# fix: ce-executor-serial perky-maple review findings

## Overview

对 `docs/plans/2026-06-18-004-fix-ce-executor-serial-perky-maple-orchestration-gaps-plan.md` 执行结果进行 Code Review 后发现 7 项问题：1 项 P0 计划内 schema 未真正生效、1 项 P0 分支无法编译阻塞验证、2 项 P1 BDD 覆盖缺口、3 项 P2 边界/质量项。

**Repo 更新说明**：在计划起草后，仓库新增提交 `f0781abd fix(hat-handoff): P0+P1 review findings (CLI seq, task.resume, context fields)`，该提交通过 `read_loop_state_from_env()` + `check_hat_handoff_gate_with_env()` 修复了原 P0 编译问题（对应本计划 U2）。因此 U2 已从「待修复」变为「已由外部提交解决」，本计划剩余 6 个有效 Implementation Unit（U1、U3–U7）。

本计划只修 Review 发现的问题，不重做原 U1–U6 业务逻辑，也不重开 U7/U8/U9 范围。

---

## Problem Frame

原计划在 `bfc9ced9` 合并后存在以下问题：

1. **U0 schema 失效**：`presets/schemas/ce-executor-serial.yml` 已加 `fix_round`，但 `presets/en/ce-executor-serial.yml` 的 inline `review.dimensions.complete` schema 仍缺少该字段；`build.rs` 对 sequence 类型做 wholesale override，导致 embedded preset 不强制 `fix_round`。
2. **~~分支无法编译~~ 已由 `f0781abd` 修复**：原 `bfc9ced9` 无法编译，根因是 `crates/ralph-cli/src/policy_check.rs` 引用未定义函数 `cli_seq_env_available`。新提交 `f0781abd` 删除了该引用，改为 `read_loop_state_from_env()` + `check_hat_handoff_gate_with_env()` 的 env-free 测试路径，并验证 `cargo nextest run -p ralph-cli --bin ralph` 1171 tests pass。本计划不再处理 U2。
3. **BDD 覆盖缺口**：U4 的 `ce_executor_serial_fix_applied_rereview.yml` 未断言 AE5 的 `queue.advance + work.ready` 双发，也未实现计划要求的 negative scenario（无 U1 prune 时 ready 被 dedup 拒绝）。
4. **suppress 模式潜在泄漏**：`apply_robot_guidance` 在 suppress 时清空 `self.robot_guidance` 但不清理 `self.ralph` 已缓存 guidance；若运行中从非 suppress 切到 suppress，下一次 prompt 可能泄漏一次 `## ROBOT GUIDANCE`。
5. **错误消息误导**：`review.dimensions.complete` 缺 `fix_round` 时 dedup 默认按 `0` 处理，可能返回 `duplicate_dimensions_complete` 而非 schema missing-field 错误。
6. **代码重复**：`PolicyRuntimeState::from_events` 中 6 段事件解析逻辑几乎相同。

---

## Requirements Trace

- R1. Embedded preset 必须强制 `review.dimensions.complete` 携带 `fix_round`。
- R2. ~~当前分支必须能编译并通过 `./scripts/run-tests.sh`。~~ 已由 `f0781abd` 满足；本计划最终验证仍须跑 `./scripts/run-tests.sh` 确保 U1/U3–U7 不破坏编译。
- R3. BDD 必须断言 `review.passed` 后 plan-gate 发出 `queue.advance` 与 `work.ready`（AE5）。
- R4. BDD 必须包含无 U1 prune 时 `review.dimension.ready` 被 dedup 拒绝的 negative scenario。
- R5. `suppress_human_guidance` 开启时，`self.ralph` 缓存的 guidance 不得进入 prompt。
- R6. 缺 `fix_round` 时应由 schema 层报 missing-field，而非 dedup 报 duplicate。
- R7. 减少 `from_events` 中的重复 JSON 解析代码。

---

## Scope Boundaries

- 只修复 Review 指出的 7 项问题；其中 U2 已由 `f0781abd` 修复，本计划实际修复 6 项。
- 不重构 `event_policy.rs` 的整体架构，只提取局部 helper。
- 不重做原计划的 U1–U6 业务逻辑；若修复编译错误时发现其它 hat_handoff 问题，仅做最小修复以恢复编译。
- 原计划 P3 项（U7 `loops.json` cleanup、U8 `hat_lifecycle` WARN、U9 `hat-channel`）仍延期。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-cli/build.rs:287-395`：`merge_preset_with_schema` / `merge_schema_mappings`。sequence 类型在 deep-merge 时走 wholesale override 分支，因此 inline `required_fields` 会完全覆盖 SSOT。
- `presets/en/ce-executor-serial.yml:317-319`：inline `review.dimensions.complete` schema。
- `presets/schemas/ce-executor-serial.yml:134-148`：SSOT 已要求 `fix_round`。
- `crates/ralph-core/src/event_loop/mod.rs:4926-4940`：`apply_robot_guidance` suppress 路径。
- `crates/ralph-core/src/event_loop/mod.rs:4510-4542`：isolated 模式 `build_prompt` 在 `collect_robot_guidance` 后调用 `clear_robot_guidance()`。
- `crates/ralph-core/src/event_policy.rs:388-545`：`from_events` 中 6 段重复解析。
- `crates/ralph-core/src/event_policy.rs:949-980`：`review.dimensions.complete` dedup 默认 `fix_round=0`。
- `crates/ralph-core/tests/scenarios/ce_executor_serial_fix_applied_rereview.yml`：现有 BDD happy path。
- `crates/ralph-cli/src/policy_check.rs`：`f0781abd` 已删除 `cli_seq_env_available` 引用，新增 `read_loop_state_from_env()` 与 `check_hat_handoff_gate_with_env()`；编译问题已解决。

### Institutional Learnings

- `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md` 记录 KTD1（dedup prune > plan-gate trigger）与 KTD3（complete dedup key 含 `fix_round`）。
- AGENTS.md 要求 builtin preset 变更同步 `presets/manifest.yml`、`scripts/ralph-zsh-plugin.zsh`、AGENTS.md/CLAUDE.md preset 列表；本次未增删 preset，无需同步。

---

## Key Technical Decisions

- **KTD1. inline schema 修正方式**：将 `fix_round` 加入 `presets/en/ce-executor-serial.yml` 的 inline `review.dimensions.complete.required_fields`，而非移除 inline 条目。保留 inline 可在 SSOT 变更时提供可审计的 override 层，同时让 embedded preset 与 SSOT 一致。
- **KTD2. 编译错误已由 `f0781abd` 修复**：新提交用 `read_loop_state_from_env()` + `check_hat_handoff_gate_with_env()` 替代了原 `cli_seq_env_available` 调用，并避免在测试中使用 `set_var`/`remove_var`（仓库有 `#![forbid(unsafe_code)]`）。本计划不再重复修复，但 U1/U3–U7 的最终验证仍需 `./scripts/run-tests.sh`。
- **KTD3. `from_events` 局部 helper**：提取一个私有 helper 将 `event.payload` 解析为 `Option<&serde_json::Map>`，不改动外部 API。
- **KTD4. 缺 `fix_round` 走 schema 错误**：在 dedup 分支中，若 `fix_round` 缺失或非数字，则不记录/不检查 dedup key，让后续 schema validation 报 `missing_required_field`。该事件本就会被拒绝，跳过 dedup 不会削弱防护。

---

## Open Questions

### Resolved During Planning

- **是否移除 inline `review.dimensions.complete` schema？** 否。保留 inline 并加 `fix_round`，与 SSOT 对齐，便于未来 hotfix 时不触发 SSOT 变更。
- **编译错误涉及 2026-06-18-005 的未提交变更，是否直接采纳？** 已由 `f0781abd` 解决：该提交引入了 `FileContent`/`skip_seq_check` 等 005 功能以修复 hat-handoff review findings。本计划 U2 因此关闭，不重复引入。

### Deferred to Implementation

- 具体 helper 函数命名与签名在提取时确定。
- negative scenario 的 `max_iterations` 精确值在实现时根据事件数调整。

---

## Final Verification

所有单元完成后必须运行：

```bash
./scripts/run-tests.sh
```

该命令是项目 HARD RULE 1 指定的唯一可接受全量测试入口。任何 targeted test 通过都不能替代它。

---

## Implementation Units

- [ ] U1. **Fix inline `review.dimensions.complete` schema**

**Goal:** 让 embedded preset 真正强制 `fix_round`。

**Requirements:** R1

**Dependencies:** None

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`

**Approach:**
- 在 `event_loop.event_policy.schemas.review.dimensions.complete.required_fields` 中加入 `fix_round`，保持与 `presets/schemas/ce-executor-serial.yml` 一致。

**Patterns to follow:**
- 与同一文件中 `fix.applied` 的 inline schema 风格保持一致。

**Test scenarios:**
- Happy path: `RalphConfig::parse_yaml` 后 `review.dimensions.complete` schema 的 `required_fields` 包含 `fix_round`。
- Integration: 在 `crates/ralph-cli/src/presets.rs` 新增 static test，从 embedded preset 内容中解析 `event_loop.event_policy.schemas.review.dimensions.complete.required_fields` 并断言包含 `fix_round`。
- Edge case: 验证 `presets/schemas/ce-executor-serial.yml` 与该 inline 条目在 `fix_round` 上一致。

**Verification:**
- Embedded preset 的 `review.dimensions.complete` required_fields 与 SSOT 一致。

---

- [x] U2. **~~Fix `ralph-cli` compilation errors~~ 已由 `f0781abd` 修复**

**Goal:** ~~恢复 `./scripts/run-tests.sh` 可运行。~~ 已达成。

**Requirements:** R2（已满足）

**Dependencies:** None

**Files:**
- 无需修改（`crates/ralph-cli/src/policy_check.rs` 已在 `f0781abd` 中修复）。

**Resolved by:**
- 提交 `f0781abd` 删除未定义函数 `cli_seq_env_available` 的引用。
- 新增 `read_loop_state_from_env()` 读取 `RALPH_LOOP_ITERATION` / `RALPH_HAT_HANDOFF_SEQ`。
- 新增 `check_hat_handoff_gate_with_env()` 使单元测试无需 `set_var`/`remove_var`（避免触发 `#![forbid(unsafe_code)]`）。

**Verification already done:**
- `cargo build --workspace` 成功。
- `cargo nextest run -p ralph-cli --bin ralph`：1171 passed, 3 skipped。

---

- [ ] U3. **Strengthen BDD for AE5 dual-publish**

**Goal:** 断言 re-review 后 plan-gate 发出 `queue.advance` 与 `work.ready`。

**Requirements:** R3

**Dependencies:** None（U2 已由 `f0781abd` 解决）

**Files:**
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_serial_fix_applied_rereview.yml`

**Approach:**
- 将当前 single-step 结尾从 `queue.advance` + `plan.complete` 调整为 `queue.advance` + `work.ready` + `plan.complete`，或新增一个 multi-step 变体 scenario 专门覆盖 AE5。
- 更新 `expected.events` 与 `iterations`。

**Patterns to follow:**
- 参考 `crates/ralph-core/tests/scenarios/` 中其它 multi-step plan-gate 场景的事件顺序。

**Test scenarios:**
- Covers AE5. `review.passed` 之后 `expected.events` 中同时出现 `queue.advance` 与 `work.ready`。
- Edge case: 更新后的 `iterations` 仍覆盖全部 stub 事件 + transport hops。

**Verification:**
- `cargo nextest run -p ralph-core --test scenarios ce_executor_serial_fix_applied_rereview` 通过。

---

- [ ] U4. **Add negative unit test for U1 prune**

**Goal:** 证明无 U1 prune 时 fix 后 re-review ready 会被 dedup 拒绝。

**Requirements:** R4

**Dependencies:** None（U2 已由 `f0781abd` 解决）

**Files:**
- Modify: `crates/ralph-core/src/event_policy.rs`

**Approach:**
- 在 `event_policy.rs` 测试模块新增单元测试：构造一个 `PolicyRuntimeState`，先接受 round 0 的 `review.dimension.ready(correctness)`，然后**不调用** `prune_review_dimension_ready_bucket`，直接再次 validate 同一个 ready 事件，断言其被 `DuplicateWorkDone` 拒绝。
- 该测试是 U1 happy path 的直接反例：它说明如果没有 `fix.applied` 触发的 prune，re-review 无法推进。
- 不采用 BDD scenario，因为 BDD harness 中 policy prune 由 `fix.applied` accept 自动触发，无法在不修改运行时的情况下构造“无 U1 prune”的反事实。

**Patterns to follow:**
- 与现有 `u1_dedup_helper_prunes_allow_fix_round_rereview` 测试对应，形成正反一对。

**Test scenarios:**
- Error path: 无 prune 时，第二次 `review.dimension.ready(correctness)` 返回 `RejectWithResume(DuplicateWorkDone)`。
- Error path: 无 prune 时，state 中仍保留第一次的 dedup key。

**Verification:**
- `cargo nextest run -p ralph-core -- review_dimension_ready` 全部通过。

---

- [ ] U5. **Clear cached robot guidance under suppress mode**

**Goal:** 消除 config 切换时 `## ROBOT GUIDANCE` 的一次性泄漏。

**Requirements:** R5

**Dependencies:** None（U2 已由 `f0781abd` 解决）

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/tests/guidance_dedup.rs`

**Approach:**
- 在 `apply_robot_guidance` 的 suppress early-return 分支中，额外调用 `self.ralph.clear_robot_guidance()`，确保 `self.ralph` 中缓存的 guidance 也被清空。

**Patterns to follow:**
- 与 `build_prompt` isolated 分支中 `collect_robot_guidance()` 后 `clear_robot_guidance()` 的对称处理保持一致。

**Test scenarios:**
- Edge case: 先让 guidance 进入 `self.ralph`（suppress=false），再启用 suppress 并调用 `apply_robot_guidance`，随后 `build_prompt` 不出现 guidance 文本。
- Happy path: 现有 `u2_apply_robot_guidance_clears_stale_cache_when_suppress_on` 仍通过。

**Verification:**
- `cargo nextest run -p ralph-core -- suppress_human_guidance` 全部通过。

---

- [ ] U6. **Surface schema error instead of duplicate when fix_round is missing**

**Goal:** 缺 `fix_round` 时让 agent 看到正确的 schema 错误。

**Requirements:** R6

**Dependencies:** None（U2 已由 `f0781abd` 解决）

**Files:**
- Modify: `crates/ralph-core/src/event_policy.rs`

**Approach:**
- 在 `review.dimensions.complete` dedup 分支中，将 `fix_round` 获取逻辑改为：仅当 `fix_round` 存在且为数字时才记录/检查 dedup key；否则不做任何 dedup 操作，让后续 schema validation 报 `missing_required_field`。

**Patterns to follow:**
- 保持现有 `DuplicateWorkDone` recovery shape 不变。

**Test scenarios:**
- Error path: 第一个 `review.dimensions.complete` 带 `fix_round=0`，第二个缺 `fix_round` → 第二个应被 schema 拒绝（missing field），而非 duplicate。
- Happy path: 两个都带相同 `fix_round=0` → 仍报 duplicate。
- Edge case: `fix_round` 为字符串 `"1"` → schema 拒绝（类型错误），不报 duplicate。
- **Test update:** 现有 `u5_review_dimensions_complete_dedup_missing_fix_round_defaults_to_zero` 预期“缺 fix_round 默认 0 并 dedup”，需更新为断言 schema 拒绝（或重命名/替换为新的 characterization test）。

**Verification:**
- `cargo nextest run -p ralph-core -- dimensions_complete_dedup` 全部通过。

---

- [ ] U7. **Refactor `from_events` JSON parsing duplication**

**Goal:** 减少重复代码，提高可维护性。

**Requirements:** R7

**Dependencies:** None（U2 已由 `f0781abd` 解决）

**Files:**
- Modify: `crates/ralph-core/src/event_policy.rs`

**Approach:**
- 在 `PolicyRuntimeState` impl 内新增私有 helper（如 `payload_object(payload: Option<&str>) -> Option<serde_json::Map<String, Value>>`），将 `event.payload` 解析为 JSON object 并返回 owned `Map`。
- 用该 helper 替换 `from_events` 中 6 段重复解析逻辑；调用方在 `if let Some(obj) = payload_object(...)` 内按业务提取字段。
- 注意 `serde_json::from_str` 返回 owned `Value`，不能返回引用给局部变量，因此 helper 返回 owned `Map`。

**Patterns to follow:**
- 保持现有行为不变；helper 只做解析，不做业务逻辑。

**Test scenarios:**
- Happy path: 所有现有 `from_events` 相关测试仍通过。
- Edge case: 非 object payload、非 JSON payload、空 payload 仍按原逻辑处理（返回 None，不 panic）。

**Verification:**
- `cargo nextest run -p ralph-core -- review_dimension_ready` 与 `fix_applied_prunes` 相关测试通过。

---

## System-Wide Impact

- **Preset surface:** U1 只改 embedded preset 的 schema，不新增/删除 preset，不影响 CLI 命令行接口。
- **Config surface:** U5 不改 `EventLoopConfig` 结构，只改运行时行为。
- **Policy surface:** U6/U7 不改 `PolicyDecision` / `ViolationType` 公共枚举，只改内部 dedup 细节。
- **Test surface:** U3/U4 新增/调整 BDD；U2 已由 `f0781abd` 修复，`./scripts/run-tests.sh` 成为可靠门禁。
- **Unchanged invariants:** `suppress_human_guidance` 默认 `false`；`review.dimensions.complete` dedup key 形状不变；plan-gate triggers 仍不听 `fix.applied`。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|-----------|
| `policy_check.rs` 的 `set_var`/`remove_var` 在并行测试中可能有竞态 | 已由 `f0781abd` 通过 `check_hat_handoff_gate_with_env()` 规避，无需 `unsafe` 块 |
| BDD 调整导致 iteration 计数不准确 | 实现时先跑一次 scenario，根据实际迭代数调整 `max_iterations` 与 `expected.iterations` |
| U6 改变“缺 fix_round 默认 0”的行为可能破坏既有测试 | 显式更新/替换 `u5_review_dimensions_complete_dedup_missing_fix_round_defaults_to_zero`，并验证 CLI precheck 仍拒绝缺字段事件 |

---

## Documentation / Operational Notes

- 无需更新 `AGENTS.md` / `CLAUDE.md` preset 列表（无 preset 增删）。
- 无需更新 `scripts/ralph-zsh-plugin.zsh`。
- 若 U1 选择保留 inline schema，建议在 `presets/en/ce-executor-serial.yml` 该条目旁加注释说明与 SSOT 的对齐关系。

---

## Sources & References

- **Origin review target:** `docs/plans/2026-06-18-004-fix-ce-executor-serial-perky-maple-orchestration-gaps-plan.md`
- **Diagnosis report:** `docs/report/2026-06-18-003-perky-maple-loop-link-diagnosis.md`
- **Solution doc:** `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md`
- Related code: `crates/ralph-core/src/event_policy.rs`, `crates/ralph-core/src/event_loop/mod.rs`, `presets/en/ce-executor-serial.yml`, `crates/ralph-cli/build.rs`
