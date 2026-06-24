---
title: BDD Harness 扩展 + Serial Lint 11 Scenario 落地
type: refactor
status: active
date: 2026-06-20
revised: 2026-06-20
revision_note: |
  v1 — BDD harness 扩展允许断言"运行时内存状态"(pending_lint_resume /
  rejection_digest / circuit_breaker 等),落地 2026-06-20-001 plan U6
  11 个 serial_lint scenario,承接 SC-1(CI) 验收。
origin:
  - docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md  # U6 deferred 来源
  - crates/ralph-core/tests/scenarios.rs:173-206  # 当前 run_scenario 是 stub
  - crates/ralph-core/src/event_loop/loop_state.rs  # pending_lint_resume 状态机位置
  - crates/ralph-core/src/event_loop/tests/serial_lint.rs  # 已落地的 5 个 unit tests
---

# BDD Harness 扩展 + Serial Lint 11 Scenario 落地

## Summary

扩展 `crates/ralph-core/tests/scenarios.rs` 的 BDD harness，让 YAML scenario 可以断言**运行时内存状态**——`state.pending_lint_resume` 是否被设置、何时被消费、circuit_breaker 阈值、`rejection_digest` 内容、`## LINT MIRROR` 是否在 prompt 中——而不只是"事件数 + 主题名"。

然后落地 2026-06-20-001 plan U6 注册表里的 11 个 serial_lint scenario，覆盖 in-loop lint feedback path（lint reject → pending hint → next build_prompt 注入 `## LINT RESUME REQUIRED` → consume-on-use），承接 SC-1（CI）验收（`serial_lint_step_chain_replay.yaml` 全绿 = 12U plan 关键路径 replay）。

BDD harness 扩展本身是通用基础设施（其他 plan 也能用，不止 serial preset）。

---

## Problem Frame

**当前 BDD harness 的能力缺口**（`scenarios.rs:173-206`）：

- `run_scenario` 是 stub：只走 `MockBackend` + `ScenarioRunner`，不创建真实 `EventLoop`，不接触 `LoopState`
- `ExpectedYaml` 只有 `iterations` / `events` / `absent_events` / `prompt_contains` 四种断言——全是**事后断言**（事件是否被发出）
- **缺失**对**运行时内存状态**的断言能力：`pending_lint_resume` / `consecutive_lint_rejections`（circuit breaker）/ `rejection_digest` 等

**前次 session 实证**（2026-06-20-001 plan review）：尝试写 3 个 BDD scenario（resume_hint_consumed / handoff_auto_prepare / timeout_fail_closed）时，全部因 harness 限制失败：
- `run_workflow_guard_scenario` 的 hat routing 第二轮返回 fallback `ralph` hat
- `inject_pending_lint_resume` 在 hat mismatch 时恢复 hint 而非注入
- Scenario YAML 里没法断言"hint 是否真的被注入到了 prompt"

**为什么不直接改 unit tests**（5 个 unit tests 已部分覆盖）：

- Unit tests 在 `event_loop/tests/serial_lint.rs`，是模块内黑盒测试
- BDD scenarios 在 `crates/ralph-core/tests/scenarios/`，走 fixtures + replay，是 SC-1（CI）验收的必需形式
- BDD harness 的状态断言能力一旦扩展，未来 plan 也能用（不止 serial preset）

---

## Goals

1. **BDD harness 扩展**：YAML 支持断言 `pending_lint_resume` / `consecutive_lint_rejections` / `rejection_digest` / `linter_disabled` 等运行时状态。
2. **11 个 serial_lint scenario 落地**：全部走真实 `EventLoop` + fixtures，全绿。
3. **SC-1（CI）验收**：`serial_lint_step_chain_replay.yaml` 全绿，代表 12U plan 关键路径 replay。
4. **可复用**：扩展后的 harness 不带 serial preset 特化（断言字段是通用 YAML 接口），其他 plan 可以用同一套机制。

## Non-Goals

- 不重写 BDD harness（保留 `run_scenario` / `run_workflow_guard_scenario` 两条路径，仅扩展 `ExpectedYaml` 字段）
- 不引入新 framework（继续 `serde_yaml` + 现有 `ScenarioRunner`）
- 不改 production 代码（仅测试代码 + YAML fixtures）

---

## Architecture

### Harness 扩展方案

**新增 `ExpectedYaml.assert_state` 段**：

```yaml
expected:
  iterations: 8
  completion: true
  assert_state:
    # in-loop lint resume 状态机
    - pending_lint_resume:
        at_iteration: 3
        topic: work.done
        reason_contains: "missing required fields"
    - pending_lint_resume_cleared:
        at_iteration: 4   # consume-on-use 后必须为 None

    # circuit breaker
    - consecutive_lint_rejections:
        at_iteration: 5
        gte: 3              # 触发阈值

    # rejection digest 累积
    - rejection_digest:
        at_iteration: 6
        contains_topic: work.done
        contains_reason: "missing"

    # linter 被 circuit breaker 禁用
    - linter_disabled:
        at_iteration: 7

    # prompt 注入（补充 prompt_contains 的语义层断言）
    - prompt_injects:
        at_iteration: 4
        hat: executor
        block: "## LINT RESUME REQUIRED"
```

**`AssertionContext` 设计**：

```rust
struct AssertionContext<'a> {
    loop_state: &'a LoopState,        // 读 pending_lint_resume / consecutive_lint_rejections / rejection_digest
    runtime_flags: &'a RuntimeFlags,   // 读 linter_disabled
    build_prompt_history: &'a [BuildPromptSnapshot],  // 读 prompt 内容
}

trait StateAssertion {
    fn evaluate(&self, ctx: &AssertionContext, at_iteration: usize) -> Result<(), String>;
}
```

**Snapshot 录制**：

`run_workflow_guard_scenario` 在每个 iteration 末尾录制 `LoopStateSnapshot` + `BuildPromptSnapshot`（已有 prompt 录制机制扩展）→ 写到 `Vec<Snapshot>` → `assert_state` 在所有 iteration 跑完后**顺序评估**，每条 `at_iteration` 字段决定查哪一帧。

**不引入 mutation**：`assert_state` 是 read-only，不修改任何状态，scenario runner 不感知。

### 11 Scenario 注册表

来源：2026-06-20-001 plan U6 (行 522-538) — 保留全表，每条加 harness 断言要点：

| # | 文件 | 覆盖 | 新增 harness 断言 |
|---|---|---|---|
| 1 | `serial_lint_internal_source_bypass.yaml` | R7-1 / AE-5 partial | `absent_events` + 新增 `assert_state.linter_disabled: false` (禁止误熔断) |
| 2 | `serial_lint_rejection_digest.yaml` | R7-2 | `assert_state.rejection_digest.contains_topic` |
| 3 | `serial_lint_steward_guidance_exempt.yaml` | R7-3 | `prompt_contains` + `assert_state.linter_disabled: false` |
| 4 | `serial_lint_resume_hint_consumed.yaml` | R7-4 / AE-4 | `assert_state.pending_lint_resume` (at_iteration=N) + `assert_state.pending_lint_resume_cleared` (at_iteration=N+1) |
| 5 | `serial_lint_fix_applied_dedup.yaml` | R7-5 | `absent_events` + `assert_state.rejection_digest` |
| 6 | `serial_lint_handoff_auto_prepare.yaml` | R7-6 / R22 / B4 | `assert_state.pending_lint_resume` 出现再被消费 + `## HAT HANDOFF` 文件被创建 |
| 7 | `serial_lint_handoff_seeds_coverage.yaml` | R7-7 / R19 | 9+ seeds 由 preset_lint 静态断言（不进 BDD），scenario 仅走通 |
| 8 | `serial_lint_step_chain_replay.yaml` | AE-6 / SC-1 CI | **必须全 11 个 invariant 在 12U 端到端 replay 中成立**（most complex） |
| 9 | `serial_lint_timeout_fail_closed.yaml` | R14 / KTD-9 | `assert_state.rejection_digest.contains_reason: timeout`（post-hoc fallback；F-PS-006 跟进真 fail-closed） |
| 10 | `serial_lint_circuit_breaker.yaml` | 熔断仅 disable linter | `assert_state.consecutive_lint_rejections.gte: 3` → `assert_state.linter_disabled: true` |
| 11 | `serial_lint_isolated_unaffected.yaml` | R18 / AE-7 | `assert_state.linter_disabled: false` + `absent_events` for lint 触发 |

---

## Implementation Units

### U1. Harness 扩展：`assert_state` 段 + `AssertionContext`

- **Goal**: `ExpectedYaml` 加 `assert_state` 字段；`ScenarioRunner` 录制 `LoopStateSnapshot` + `BuildPromptSnapshot`；`assert_state` 在迭代完成后顺序评估。
- **Requirements**: R-H1 ~ R-H5（harness 新规则）
- **Files**:
  - 修改 `crates/ralph-core/tests/scenarios.rs`（加 `AssertionYaml` struct + `evaluate_assert_state`）
  - 不修改 `crates/ralph-core/src/`（production 代码）
- **Patterns to follow**: 现有 `ExpectedYaml.prompt_contains` / `CheckpointYaml` 的 lazy evaluation 风格
- **Test scenarios**: 写 1 个 `assert_state_harness_smoke.yaml` 测试 harness 自身（不依赖 serial preset）—— 验证 `assert_state` 字段能被解析 + 评估
- **Verification**: 
  - `cargo nextest run -p ralph-core --test scenarios assert_state_harness` 全绿
  - 现有 27 个 scenario 全部继续 pass（不破坏向后兼容）

### U2. Snapshot 录制基础设施

- **Goal**: `run_workflow_guard_scenario` 在每个 iteration 末尾录制 snapshot（不引入新 EventLoop API，**只读** `self.state` / `self.config.event_loop.runtime_flags`）。
- **Requirements**: R-H2
- **Files**: `crates/ralph-core/tests/scenarios.rs`
- **Patterns to follow**: `prompt_contains` 已有的"iter 末尾追加到 history"机制
- **Test scenarios**: U1 的 smoke scenario 包含 1 个简单 snapshot 断言
- **Verification**: smoke scenario 全绿

### U3. Scenario 1-5（contract 不变量，5 个最简单）

- **Goal**: 落地 internal_source_bypass / rejection_digest / steward_guidance_exempt / resume_hint_consumed / fix_applied_dedup
- **Files**: `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_{1..5}.yaml`
- **Test scenarios**: scenario 自身
- **Verification**: 5 个 scenario 全绿
- **Execution note**: 4 (resume_hint_consumed) 是最关键——**必须**精确验证 `at_iteration=N` 设置 hint，`at_iteration=N+1` 消费 hint 后清空

### U4. Scenario 6-7（配置完整性）

- **Goal**: 落地 handoff_auto_prepare / handoff_seeds_coverage
- **Files**: 同上
- **Verification**: 2 个 scenario 全绿
- **Execution note**: 7 (handoff_seeds_coverage) 主要验证 preset 配置正确性，可被 preset_lint 静态断言；scenario 部分只验证"能跑通"

### U5. Scenario 8-11（边界 + replay）

- **Goal**: 落地 step_chain_replay (SC-1 CI 验收) / timeout_fail_closed / circuit_breaker / isolated_unaffected
- **Files**: 同上
- **Verification**: 4 个 scenario 全绿
- **Execution note**: 10 (circuit_breaker) 需要在 mock_responses 里**故意**触发 3+ 次 lint reject；11 (isolated_unaffected) 需要切到 `execution_mode: isolated` 验证 linter 不挂

### U6. SC-1 (CI) 验收 + reverse-validation

- **Goal**: `serial_lint_step_chain_replay.yaml`（scenario 8）作为 SC-1 (CI) 验收依据；CLAUDE.md 的"反向验证"规则应用到 scenario YAML 文件的行号引用。
- **Files**: `docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md` 状态从 `superseded_by` 解除 → 已实现
- **Verification**:
  - `./scripts/run-tests.sh` 全绿
  - 11 个 scenario 在 nextest 下全绿
  - Scenario YAML 的 `fixtures:` / `mock_responses:` 引用的 preset / 路径都真实存在（反向 grep 校验）

---

## Files

### 新增

- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_1_internal_source_bypass.yaml`
- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_2_rejection_digest.yaml`
- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_3_steward_guidance_exempt.yaml`
- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_4_resume_hint_consumed.yaml`  ← 最关键
- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_5_fix_applied_dedup.yaml`
- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_6_handoff_auto_prepare.yaml`
- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_7_handoff_seeds_coverage.yaml`
- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_8_step_chain_replay.yaml`  ← SC-1 CI
- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_9_timeout_fail_closed.yaml`
- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_10_circuit_breaker.yaml`
- `crates/ralph-core/tests/scenarios/serial_lint/serial_lint_11_isolated_unaffected.yaml`
- `crates/ralph-core/tests/scenarios/serial_lint/assert_state_harness_smoke.yaml`  ← U1 自测

### 修改

- `crates/ralph-core/tests/scenarios.rs`（U1 + U2）—— 加 `AssertionYaml` / `AssertionContext` / snapshot 录制
- `docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`（U6）—— `superseded_by` 移除

---

## Requirements (R-H* = harness 新规则)

| ID | 描述 | 验收 |
|---|---|---|
| **R-H1** | `ExpectedYaml.assert_state` 是可空 Vec；字段缺失 = no assertion | U1 smoke |
| **R-H2** | `run_workflow_guard_scenario` 在每 iteration 末尾录制 snapshot，不引入新 EventLoop API | U2 smoke |
| **R-H3** | `at_iteration` 字段必须 ≥ 1 且 ≤ 实际 iteration 数（边界外 → fail-fast 报错） | U1 smoke |
| **R-H4** | snapshot 是 read-only，scenario runner 不感知（保持 backward compat） | U1 smoke + 现有 27 scenario 全绿 |
| **R-H5** | 断言失败时输出"at_iteration=N expected X, got Y"的清晰消息 | U1 smoke |
| **R-H6** | `assert_state` 不依赖 serial preset 特化（通用字段，未来其他 plan 可用） | review |

---

## Verification

- **U1**: `assert_state_harness_smoke.yaml` 全绿；现有 27 个 scenario 0 regression
- **U2**: snapshot 录制不引入 race condition（nextest process-per-test 隔离保证）
- **U3-U5**: 11 个 serial_lint scenario 全绿
- **U6**: `./scripts/run-tests.sh` 全绿；`serial_lint_step_chain_replay.yaml`（scenario 8）作为 SC-1 (CI) 验收依据

**最终验收**：

- **SC-1 (CI)**：`cargo nextest run -p ralph-core --test scenarios serial_lint_step_chain_replay` 全绿
- **SC-1 (CI) 反向验证**：scenario YAML 的所有 fixture / mock_response 引用 grep 校验存在

---

## Risks & Dependencies

### Dependencies

- **DEP-1**. 2026-06-20-001 plan 已 ship U4/U4b（`pending_lint_resume` / `inject_pending_lint_resume` 实现存在）—— 已落地，✅
- **DEP-2**. `LoopState::pending_lint_resume` / `consecutive_lint_rejections` 字段存在 —— 已落地，✅
- **DEP-3**. circuit breaker 实现存在 —— 需 grep 确认；若不存在，U5 (scenario 10) 需降级或补实现
- **DEP-4**. `BuildPromptSnapshot` 录制机制 —— 现有 `prompt_contains` 已部分实现，需扩展为全 prompt snapshot

### Risks

- **RISK-1**（中）. harness snapshot 录制增加测试时间 → Mitigation：snapshot 是 Vec<RefCell>，每 iter 仅 push 不 clone 大对象
- **RISK-2**（低）. scenario 10 (circuit_breaker) 触发条件可能依赖未落地的熔断实现 → Mitigation：U5 实施前 grep 确认；若未落地，scenario 改为 TDD 驱动先补实现
- **RISK-3**（低）. 11 scenario 数量过多，CI 时间翻倍 → Mitigation：scenario 8 (step_chain_replay) 已经覆盖大部分 invariant；其他 10 个可标记为 `#[ignore]` 默认不跑，CI 触发时再启用（待和 user 确认）
- **RISK-4**（中）. serial preset YAML 变动导致 fixture 过期 → Mitigation：scenario YAML 的 fixture 段用相对路径 + 显式 `version: 1`，变更 preset 时同步 bump

### 早期止损线

- **Phase 1（必须）**：U1 + U2 + U3（5 个 contract 不变量 scenario）—— **核心 in-loop hint 路径覆盖**
- **Phase 2（应做）**：U4 + U5（其余 6 个 scenario）—— SC-1 (CI) 完整验收
- **Phase 3（可选）**：U6 reverse-validation 脚本化

---

## Sources / Research

- **来源 plan**：`docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`（U6 注册表，行 522-538）
- **当前 stub**：`crates/ralph-core/tests/scenarios.rs:173-206`（run_scenario 不创建 EventLoop）
- **现有 scenario 模式**：`crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`（走 `run_workflow_guard_scenario`）
- **prompt snapshot 现有机制**：`ExpectedYaml.prompt_contains` + `PromptContainsYaml`（行 121-137）
- **5 个已落地的 unit tests**：`crates/ralph-core/src/event_loop/tests/serial_lint.rs`（作为 BDD scenario 的 reference impl）
- **circuit breaker 代码位置**：TBD grep（U5 实施前确认）
- **loop_state 字段**：`crates/ralph-core/src/event_loop/loop_state.rs`（`pending_lint_resume` / `consecutive_lint_rejections` / `rejection_digest`）

---

## Execution Order（建议）

`U1 → U2 → U3 → U4 → U5 → U6`

U1 + U2 是 harness 基础设施，必须先做。U3 是最关键的 5 个 contract scenario（包含 `resume_hint_consumed`，承接 2026-06-20-001 plan U4b 的核心 invariant），先做。U4 + U5 是补全。U6 是验收 + 反向验证。

预估：
- U1: 2-3 小时（struct + evaluate + smoke scenario）
- U2: 1-2 小时（snapshot 录制）
- U3: 3-5 小时（5 个 scenario + edge case 调试）
- U4: 1-2 小时（2 个 scenario）
- U5: 3-5 小时（4 个 scenario + circuit_breaker 触发条件可能补实现）
- U6: 1-2 小时（reverse-validation + 文档）

总计：**11-19 小时**，建议分 2-3 个 session 完成。

---

## Plan Closing Criteria（plan 转 completed 条件）

- [ ] U1-U6 全部实施完成
- [ ] 11 个 serial_lint scenario 全绿
- [ ] `./scripts/run-tests.sh` 全绿（不引入新 flake）
- [ ] 2026-06-20-001 plan 的 `superseded_by` 字段移除
- [ ] SC-1 (CI) 验收文档化（指向 `serial_lint_step_chain_replay.yaml`）
