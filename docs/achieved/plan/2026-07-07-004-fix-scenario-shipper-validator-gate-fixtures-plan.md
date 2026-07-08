---
title: "fix: Refresh BDD scenarios for shipper_validator_gate (plan 002 U5/U6)"
type: fix
date: 2026-07-07
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
origin:
  - docs/plans/2026-07-07-002-fix-ce-executor-serial-runtime-protocol-stability-plan.md
  - docs/plans/2026-07-07-003-fix-terminal-guard-policy-semantics-plan.md
related_plans:
  - docs/plans/2026-07-07-002-fix-ce-executor-serial-runtime-protocol-stability-plan.md
  - docs/plans/2026-07-07-003-fix-terminal-guard-policy-semantics-plan.md
---

# fix: Refresh BDD scenarios for shipper_validator_gate (plan 002 U5/U6)

## Goal Capsule

| Field | Value |
|---|---|
| Objective | 把 8 个 pre-existing 失败的 BDD scenario 与 plan 002 (2026-07-07) 引入的 `shipper_validator_gate` 重新对齐——场景必须显式构造 `current_step` 与 `validator_terminal_step` 才能让 shipper 走 `Allow` 分支。 |
| Authority | plan 002 R3 ("shipper 不得在当前 step validator 终态之前发出 REVIEW_COMPLETE;`pass_with_residuals` 不能作为 validator 缺席时的成功替代") 是 shipper 行为的最终权威,场景必须服从 gate。 |
| Execution profile | 8 个 scenario yml / 1 个 scenarios.rs 测试函数的 targeted 重写,逐个加 `test.passed` validator 事件并校对 expected.events。 |
| Stop condition | 8 个失败的 scenario 在 `cargo nextest run -p ralph-core --test scenarios` 下全部通过;`./scripts/run-tests.sh` 完整 baseline 通过;0 个新回归。 |

---

## Product Contract

### Summary

plan 002 (commit 97ab5b31) 引入了 `shipper_validator_gate`
(`crates/ralph-core/src/event_loop/phase_authority/shipper_helper.rs:122`),
`evaluate_shipper_validator_gate` 在 `current_step = None` 或
`validator_terminal_step` 不匹配时直接 `HardFail` / `DenyWaitForValidator`。
8 个 BDD scenario 写就于 plan 03 (2026-07-03) 时代,场景 timeline 不含
`test.passed` 事件,所以 `last_test_passed_step` / `last_validator_terminal_step` 都
是 None,导致 shipper 在每个 scenario 中都被 gate 拒,REVIEW_COMPLETE
永远不进 accepted events。

### Problem Frame

`evaluate_shipper_validator_gate` 链路(`crates/ralph-core/src/event_loop/mod.rs:13220-13282`):

1. `current_step` 来自 `state.last_test_passed_step.or(last_validator_terminal_step).or(last_plan_complete_step)`(mod.rs:13263-13268)
2. `validator_terminal_step` 来自 `state.last_validator_terminal_step`(mod.rs:13266)
3. gate 行 125-132:`current_step = None` → `HardFail{reason:"shipper_validator_gate:missing_current_step"}`

8 个失败场景的 mock_responses timeline 里没有 `test.passed` / `test.failed` 事件,
所以 `current_step` 永远为 None,REVIEW_COMPLETE 全部 hard-fail。

### Requirements

- R1. 8 个失败 scenario 全部重写 mock_responses,在 shipper 路径上插入 `test.passed`(或对应 `test.failed`)validator terminal 事件,使 `current_step` 与 `validator_terminal_step` 满足 gate 要求。
- R2. 每个 scenario 的 `expected.events` 列表与 `event_topic_counts` 与新 timeline 严格匹配;不再含已被 plan 002 gate 拒的"无 validator 直接 shipper pass"路径。
- R3. 每个 scenario 的 `iterations` 与新 mock_responses 数量一致(每个 `<event topic="...">{...}</event>` 块消耗一次 iteration;`waiting.` 占位算一次)。
- R4. 8 个场景的 `plan_end` phase 配置、`allowlist` 行为不修改,仅改 mock 路径与 expected events。
- R5. `ce_executor_serial_shipper_hard_fail_promotion` 期望"非白名单 reason 不能升级为 pass"的语义保留——通过把 `test.passed` 安排在 `plan.blocked(stall_no_events)` 之后,让 shipper 仍能 HardFail(recovery without validator terminal),test.passed 事件**不**进入 shipper 触发前的 mock 序列。
- R6. `test_u4_shipper_reason_whitelist` 期望"非白名单 reason 走 hard-fail"——同理保留 stall-recovery HardFail 路径,test.passed 不在 shipper 路径之前。
- R7. `test_verdict_gate_fail_keeps_loop_open` 与 `test_u5_review_complete_dedup` 期望"REVIEW_COMPLETE 在 verdict=fail 路径里被记录"——为它们加 `test.passed` 之前要确认 verdict_gate 的 reject 路径仍在执行。

### Scope Boundaries

- 本计划只重写 8 个 BDD scenario(7 个 yml + 1 个 `verdict_gate_fail_keeps_loop_open` 的 in-line mock)。
- 不改 `evaluate_shipper_validator_gate` 本身或 `phase_authority` 配置。
- 不改 `event_policy.completion_after_terminal.business_after_completion` 默认值。
- 不动 `ce-executor-serial` preset 配置;其 `business_after_completion: reject` 路径保持冻结。
- 不混合其他 pre-existing P1(bounded retry / `TaskStore::ensure` step-locus 合并 / `plan.blocked` 接受 ledger 权威边界)。

---

## Planning Contract

### Key Technical Decisions

- KTD1. `test.passed` 事件必须先于 shipper 的 REVIEW_COMPLETE;事件 payload 含 `step` field 以触发 `loop_state::record_test_passed`(loop_state.rs:1102)。
- KTD2. 对 hard-fail 类场景(`shipper_hard_fail_promotion`、`u4_shipper_reason_whitelist`),不主动给 `test.passed` —— shipper_validator_gate 会在 `current_step = None` 时 HardFail,这正是测试期望的语义。
- KTD3. 对 recoverable 类场景(`shipper_default_publishes_recoverable`、`shipper_recoverable_reasons`),在 shipper 路径前插入 `test.passed` + 等待 shipper 收尾,让 gate 走 `Allow`。
- KTD4. `verdict_gate_fail_keeps_loop_open` 是 verdict gate 的反向测试(REVIEW_COMPLETE 的 pass_or_fail=fail 应当被 reject 而非 accept);`current_step` 不为 None 的情况下,verdict gate 在更下游处理。需要在 mock_responses 早期加 `test.passed` + `work.done` 让 pipeline 推进到 shipper,然后 shipper 发 fail verdict,verdict_gate 拒收。
- KTD5. `ce_executor_serial_fix_unit_terminal` 与 `ce_executor_serial_pass_with_residuals_terminal` 期望"serial preset full happy path with residuals"——需要最完整的 timeline(plan.ready → work.ready → work.done → test.passed → plan.complete → REVIEW_COMPLETE(pass_with_residuals) → report.done)。
- KTD6. `test_u5_review_complete_dedup` 期望"两次 byte-identical REVIEW_COMPLETE 在同 batch 内只被 seen 一次"——以 test.passed 填 current_step 后,REVIEW_COMPLETE(1st) 进 accepted + REVIEW_COMPLETE(2nd) 被 dedup 路径拒。

### High-Level Technical Design

每个 scenario 的 timeline 改造都遵循"在 shipper 路径前补 validator terminal"原则。具体每个 scenario 的 mock_responses 与 expected.events 改造在 Implementation Units 阶段定稿。

### Assumptions

- `test.passed` 事件 payload 形式 `{step, verdict:"pass"}` 即可触发 `record_test_passed`(无需 task_id / task_key 字段,因为场景 timeline 不调用 validate_execution_contract)。
- scenarios.rs 的 `load_scenario` + `run_workflow_guard_scenario` 仍按现有规则运行真 EventLoop runner,无需改。
- 8 个场景的 preset / hat 配置保持现状,只改 mock 路径。

### Sources & Research

- `crates/ralph-core/src/event_loop/phase_authority/shipper_helper.rs:122-163` — `evaluate_shipper_validator_gate` 决策
- `crates/ralph-core/src/event_loop/mod.rs:13220-13282` — wiring 与 ctx 构造
- `crates/ralph-core/src/event_loop/loop_state.rs:1102-1110` — `record_test_passed` / `record_validator_terminal`
- 8 个失败场景 yml + scenarios.rs 的 `test_verdict_gate_fail_keeps_loop_open`
- plan 002 R3 + plan 03 shipper recoverable 设计意图

---

## Implementation Units

### U1. Audit each scenario's expected gate outcome

- **Goal:** 对 8 个场景逐个写"应走哪条 gate 路径",确定哪些需要 `test.padded` 填 step,哪些依赖 HardFail。
- **Files:** `docs/notes/2026-07-07-scenario-gate-audit.md`(本 plan 期间的工作笔记)
- **Approach:** 列出 8 个场景 × 4 列:current_step 推导 / validator_terminal_step 推导 / attempting_success / 期望 gate decision。便于 U2-U9 改 yml 时不返工。

### U2. Rewrite `ce_executor_serial_shipper_default_publishes_recoverable.yml`

- **Goal:** 修通 `default_publishes` reason → `pass_with_residuals` 路径,加 test.passed 填 step。
- **Approach:** mock_responses 顺序:`plan.blocked(default_publishes)` → `test.passed` (step-01) → `REVIEW_COMPLETE(pass_with_residuals)`,expected.events 含三 topic。

### U3. Rewrite `ce_executor_serial_shipper_recoverable_reasons.yml`

- **Goal:** 修通 shipper recoverable reasons 路径。
- **Approach:** 同 U2 pattern,多一个 recoverable reason(recovery_exhausted 等)的 test.passed 注入。

### U4. Rewrite `ce_executor_serial_shipper_hard_fail_promotion.yml`

- **Goal:** 保留 stall_recovery without validator terminal → HardFail 语义。
- **Approach:** **不**主动加 test.passed,让 `current_step = None` 触发 HardFail。expected.events 仅含 `plan.blocked` 与 `REVIEW_COMPLETE(fail)`(fail path 通过 HardFail 之外的其他门走通)。

### U5. Rewrite `ce_executor_serial_fix_unit_terminal.yml`

- **Goal:** serial fix-unit happy path with test.passed / plan.complete / report.done。
- **Approach:** 补齐 full timeline:`plan.ready → work.ready → work.done → test.passed → plan.complete → REVIEW_COMPLETE(pass_with_residuals) → report.done`。

### U6. Rewrite `ce_executor_serial_pass_with_residuals_terminal.yml`

- **Goal:** serial pass_with_residuals happy path with full pipeline。
- **Approach:** 同 U5 完整 timeline。

### U7. Rewrite `2026-06-30-001-u4-shipper-reason-whitelist.yml`

- **Goal:** 保留非白名单 reason → HardFail 语义。
- **Approach:** **不**主动加 test.passed;`current_step = None` 触发 HardFail。expected.events 含 `plan.blocked` + `REVIEW_COMPLETE(fail)`。

### U8. Rewrite `2026-06-30-001-u5-review-complete-dedup.yml`

- **Goal:** test.passed 填 step,REVIEW_COMPLETE(1st) accepted,REVIEW_COMPLETE(2nd, byte-identical) 被 dedup。
- **Approach:** 加 test.passed 事件,让两次 REVIEW_COMPLETE 在同 batch 内,dedup 路径拒 second。

### U9. Rewrite `test_verdict_gate_fail_keeps_loop_open` mock (in scenarios.rs)

- **Goal:** verdict=fail 路径下,REVIEW_COMPLETE 应被 verdict_gate 拒收,让 loop 不被 LOOP_COMPLETE 关闭。
- **Approach:** mock 早期加 test.passed + work.done 让 pipeline 推进到 shipper,shipper 发 REVIEW_COMPLETE(verdict=fail),verdict_gate 拒收;loop 不进入 completion,符合测试注释语义。

### U10. 文档同步

- **Goal:** `crates/ralph-core/data/ralph-tools-emit.md` 与 `ralph-tools-recovery-directives.md` 中关于 "shipper-after-validator" 的描述与 plan 002 R3 一致(plan 003 文档同步已经覆盖,但场景侧描述需要再次确认)。
- **Files:** 必要时小修。

---

## Verification Contract

| Gate | Scope | Done signal |
|---|---|---|
| 8 个 scenario 单独跑 | `cargo nextest run -p ralph-core --test scenarios -- <test_name>` | 8 个全过 |
| scenarios 全套 | `cargo nextest run -p ralph-core --test scenarios` | 全过 |
| ralph-core 全套 | `cargo nextest run -p ralph-core --no-fail-fast` | 0 失败或仅文档/style 失败 |
| 全 workspace baseline | `./scripts/run-tests.sh` | 通过 |
| Doc drift | `scripts/check-cli-doc-drift.sh` | 通过(若动了 data doc) |

---

## Definition of Done

- 8 个 BDD scenario 在 `cargo nextest run -p ralph-core --test scenarios` 下全过。
- 0 个新回归(本 plan 之前已修的 7 个测试维持绿)。
- 不修改 `evaluate_shipper_validator_gate` 本身或 phase_authority 配置。
- 不修改 `ce-executor-serial` preset 或 schema。
- 写完 8 个 scenario 的 audit 笔记到 `docs/notes/2026-07-07-scenario-gate-audit.md`。
