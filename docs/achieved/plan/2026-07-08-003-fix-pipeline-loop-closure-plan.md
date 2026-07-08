---
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
title: Pipeline Loop Closure Repair - Plan
type: fix
date: 2026-07-08
origin: docs/report/2026-07-08-ce-executor-pipeline-loop-primary-20260708-084141-diagnosis.md
---

# Pipeline Loop Closure Repair - Plan

## Goal Capsule

修复 `ce-executor-pipeline-loop` 首次真实运行暴露的闭环失败：`fix.done` 已被接受并标记 `triggered=review-reentry`，但第二轮 `review.round.ready` 没有出现，后续恢复路径反复激活 `review-synthesizer` 空转，最终因缺 `report.done` 拒绝 `LOOP_COMPLETE` 并人工停止。

本计划以运行产物和当前源码为准，不直接继承诊断报告中的全部归因。
已核实的原始产物位于 sibling repo `ralph-e2e/.ralph/...`；计划内引用该目录时只使用该相对说明，不把机器本地绝对路径写入实现合同。

执行边界：

- 以 builtin preset 合同为主修复闭环：先稳定 `ce-executor-pipeline-loop.yml` 的事件 payload、hat instructions、schema/lint 和真实 scenario 覆盖。
- Rust runtime 只做必要的通用修补：尊重 preset 声明出的单消费者 handoff / targeted event，不新增按 preset 名称或业务 topic 写死的状态机。
- 不把 `reporter` 改成直接消费 `review.synthesized`；当前设计是 `review.synthesized -> review-gate -> review.accepted -> alignment -> reporter`，这条拓扑本身成立。
- 不绕过 `required_events: ["report.done"]`，也不允许 ralph 兜底直接 `LOOP_COMPLETE` 收口。
- 最终必须用 `cargo nextest run` 系列和 `./scripts/run-tests.sh` 验证，禁止裸跑 `cargo test -p ralph-cli`。

---

## Product Contract

### Problem Frame

`ce-executor-pipeline-loop` 的首轮运行在执行和首轮 review/fix 上基本走通，但在修复后重入第二轮 review 时断链。
事件链显示 L1-L14 完整到达 `fix.done`：

- `events-20260708-084141.jsonl` L11：`review.synthesized`，`p0_count=0`、`p1_count=2`、`verdict=pass_with_residuals`、`triggered=review-gate`。
- L12：`review-gate` 正确发出 `fix.requested`。
- L13：`fix-planner` 发出 `review.complete`。
- L14：`fixer` 发出 `fix.done`，`triggered=review-reentry`，`fixes_applied=6`，`fixes_skipped=0`，但 `next_review_plan:null`。
- L15-L17：`ralph` 兜底连续发 `LOOP_COMPLETE`，第一次 payload 是字符串缺 `reason`，后两次有 `reason` 但都缺 `report.done` 前置事件。

日志显示断链后的恢复行为不正确：

- `diagnostics/logs/ralph-2026-07-08T16-41-41-386-5918.log` 09:11、09:15、09:16 三次 `hat_channel_empty_after_activation` 都是 `hat=review-synthesizer`。
- 同一日志 09:16:39 才出现 `handoff dispatch timeout: routing task.resume to review-reentry`，说明 `fix.done -> review-reentry` 的 handoff tracker 认为消费者长期未激活。
- 同一日志 09:16:39 后立刻 `Hard gate exhausted: count=3`，然后 `Wrapping up: stopped`，没有 typed termination reason。

报告中“reporter 应消费 `review.synthesized`”这一判断不符合当前 preset 设计。
当前 preset 头部、hat triggers 和 `crates/ralph-cli/src/presets.rs` 测试都明确锁定：`review.synthesized` 的唯一消费者是 `review-gate`；`reporter` 消费 `align.done`、`plan.blocked`、`work.failed`、`review.loop.blocked`。
真正需要修的是：preset 没有把 `fix.done.next_review_plan` 和 `review-reentry` 输入合同约束到足够稳定，同时 runtime 对“已有明确 target / 单消费者 handoff”的通用执行保证不够硬，恢复逻辑才有机会把空转压力落到 `review-synthesizer`，使 `review.accepted -> alignment -> reporter -> report.done` 永远不可达。

### Requirements

**Loop closure**

- R1. Preset 必须明确表达 `fix.done -> review-reentry -> review.round.ready` 的闭环合同；`fix.done` 被接受后，下一次可执行业务 hat 应是 `review-reentry`。
- R2. `fix.done.next_review_plan` 必须是非空结构化对象；`null` 不能通过 preset/schema/lint contract，因为 `review-reentry` 依赖它生成下一轮 review 计划。
- R3. `review-reentry` instructions 和 examples 必须支持 `fix.done` 触发 payload，并把 `review_round` 增为 `N+1`，把 `round_base_sha` 更新为 `fix.done.head_sha`。
- R4. 第二轮六维 review、`review-synthesizer`、`review-gate` 必须能继续按单消费者拓扑推进。
- R5. 当后续 `review-gate` 发出 `review.accepted` 时，必须到达 `alignment -> reporter -> report.done -> LOOP_COMPLETE`。

**Recovery and hard gates**

- R6. Runtime 只补通用执行保证：已存在 target 的 event（例如 `task.resume(target=...)`）和 preset 声明出的唯一 consumer handoff 必须优先于普通 round-robin 或旧恢复 residue；实现不得写死 `ce-executor-pipeline-loop`、`fix.done` 或 `review-reentry` 特例。
- R7. `hat_channel_empty_after_activation` 连续达到 hard gate 阈值后，runtime 必须产生明确 typed termination 或 blocked event，不应只记录 INFO/WARN 后等待人工停止；该 outcome 是通用 missing-event/hard-gate 语义，不是 loop preset 专属分支。
- R8. `ralph` 兜底不能在缺 `report.done` 时继续诱导 `LOOP_COMPLETE`；缺前置事件时应恢复到正确下游 hat 或明确阻塞。

**Scope violation**

- R9. 六个 `dim:*` read-only reviewer 修改 tracked plan 文件时，必须走与历史 `dimension-reviewer` 等价的 hard reject，而不是 `MissingField` 软计数。
- R10. preset lint 必须覆盖 `dim:*` read-only reviewer 的 forbidden write path，不只匹配 `hat_id == "dimension-reviewer"`。
- R11. dim hat instructions 不应鼓励修改原 plan；若需要 findings 或 Covers 建议，应写入 `.ralph/review/{plan_name}/...` 或报告 residual，而不是编辑 `docs/plans/*.md`。

**Tests and documentation**

- R12. 新增真实 workflow scenario 覆盖 `pass_with_residuals -> fix.requested -> review.complete -> fix.done -> second review -> accepted -> report.done -> LOOP_COMPLETE`。
- R13. 新增 scenario 覆盖 `fix.done.next_review_plan:null` 被拒或导向明确 correction，而不是让 `review-reentry` 空转。
- R14. 新增 regression 覆盖 `dim:*` scope violation hard reject。
- R15. 修改 preset/schema/event topology 后，同步检查 `crates/ralph-core/data/*.md`、preset operator skills、`CLAUDE.md`、`AGENTS.md` 和 zsh completion 是否需要更新。

### Acceptance Examples

- AE1. 给定 `fixer` 发出带非空 `next_review_plan` 的 `fix.done`，当 runtime 处理该事件时，下一条业务事件是 `review.round.ready`，payload 中 `review_round=2` 且 `review_plan` 等于 `fix.done.next_review_plan`。
- AE2. 给定 `fixer` 发出 `next_review_plan:null`，当 event policy 或 preset lint 检查 payload 时，该事件不能被视为可推进的正常 `fix.done`。
- AE3. 给定任一 topic 的唯一消费者是某个 hat，当 handoff timeout 到期并生成 targeted `task.resume` 时，该 target 必须获得调度优先级；本 preset 中的实例是 `fix.done -> review-reentry`。
- AE4. 给定 `dim:goal-alignment` 修改 `docs/plans/foo.md`，当 scope audit 运行时，loop 产生 `ScopeViolationHardRejected` 或等价 typed hard termination。
- AE5. 给定 `review.accepted` 已发出，流程必须产生 `align.done`、`report.done` 和合法 `LOOP_COMPLETE`；缺 `report.done` 的 `LOOP_COMPLETE` 仍必须被拒。

### Scope Boundaries

本轮不修改 `ce-executor-pipeline` 既有行为，不引入多消费者 topic，不删除 `required_events: ["report.done"]`，不把运行时内部 ledger 暴露给 hat instructions。
本轮允许修改 `presets/en/ce-executor-pipeline-loop.yml`，因为当前断链首先是 preset payload contract / instructions 不够稳定。Runtime 修改必须保持 preset-agnostic：不按 preset 名称、hat 名称或业务 topic 加定制状态机。

### Sources and Evidence

- `ralph-e2e/.ralph/events-20260708-084141.jsonl`：17 条主事件，L14 `fix.done triggered=review-reentry`，L15-L17 三次 `LOOP_COMPLETE`。
- `ralph-e2e/.ralph/ledger.jsonl`：L14 缺 `reason`，L17/L20/L21/L24 缺 `report.done`。
- `ralph-e2e/.ralph/recovery.jsonl`：L16-L19 显示 `review-synthesizer` 产出 `loop.cancel` / `report.done` repair-stream 记录，但这些不是主 events 中的 accepted business events。
- `ralph-e2e/.ralph/diagnostics/channel-routing-fallback-*.md`：一次 `ralph` 空 channel，三次 `review-synthesizer` 空 channel。
- `ralph-e2e/.ralph/diagnostics/logs/ralph-2026-07-08T16-41-41-386-5918.log`：scope violation、completion rejection、handoff timeout、hard gate exhausted 的直接日志。
- `presets/en/ce-executor-pipeline-loop.yml`：当前 schema 已要求 `fix.done.next_review_plan` 字段，但没有表达非空结构；`alignment` instructions 仍写“From `fix.done` payload”，但 trigger 实际是 `review.accepted`。
- `crates/ralph-core/src/event_loop/mod.rs`：scope hard reject 只匹配 `hat_id == "dimension-reviewer"`；handoff timeout 在 `process_output` 开头过期后才发布 targeted `task.resume`。
- `crates/ralph-core/src/preset_lint/dimension_reviewer_write_paths.rs`：lint 只检查 `dimension-reviewer`，不覆盖 `dim:*`。
- `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop.yml`：现有 scenario 只覆盖首轮无 P0/P1 的 happy path，不覆盖 fix/reentry 闭环。

---

## Planning Contract

### Key Technical Decisions

- KTD1. 保持 `review.synthesized -> review-gate` 单消费者设计。
  诊断报告把 reporter 未直接消费 `review.synthesized` 当成 P0，但当前拓扑要求 gate 先决策，reporter 只处理 accepted/blocked/failure 后的收口路径。

- KTD2. 把 `fix.done.next_review_plan` 从“字段存在”提升为“非空结构合同”，并优先在 preset/schema/lint 层表达。
  运行产物中 `next_review_plan:null` 与 `review-reentry` instructions 的“转发下一轮 review_plan”相矛盾；只检查 required field 不足以保证下一跳可执行。

- KTD3. Runtime 只做薄机制补强，不做 loop preset 定制。
  `fix.done` 的 `triggered=review-reentry` 已正确写入 events，说明拓扑识别存在；实现应加强“preset 声明出的单消费者 handoff / targeted event 必须被尊重”这条通用规则，而不是把 `fix.done` 或 `review-reentry` 写进调度器。

- KTD4. 把 `dim:*` 视为 dimension reviewer family。
  新 preset 把单个 `dimension-reviewer` 拆成六个 first-class dimension hats，机制和 lint 也必须按职责识别 read-only dimension reviewer，而不是按旧 hat_id 字符串识别。

- KTD5. 用真实 EventLoop scenario 锁定环形路径。
  现有测试覆盖首轮 accepted，但没有覆盖首轮 P1、fix 后 reentry、第二轮 accepted、max-round blocked 和 null payload contract。

### High-Level Technical Design

```mermaid
flowchart TB
  FD[fix.done accepted] --> HC[Handoff tracker records fix.done -> review-reentry]
  HC --> NH[next_hat dispatch]
  NH --> RR[review-reentry activation]
  RR --> R2[review.round.ready round=2]
  R2 --> DIMS[6 dim hats]
  DIMS --> SYN[review-synthesizer]
  SYN --> GATE[review-gate]
  GATE -->|accepted| ALIGN[alignment]
  ALIGN --> REPORT[reporter]
  REPORT --> DONE[report.done + LOOP_COMPLETE]
  GATE -->|fix.requested| FIX[fix-planner -> fixer -> fix.done]
  FIX --> FD
  GATE -->|review.loop.blocked| REPORT

  HC -. current bug .-> TIMEOUT[handoff timeout]
  TIMEOUT -. wrong recovery observed .-> SYNEMPTY[review-synthesizer empty channel]
  SYNEMPTY -. no report.done .-> REJECT[LOOP_COMPLETE rejected]
```

### Assumptions

- `review-synthesizer` 的三次空 channel 是错误恢复/调度路径的结果，不是期望中的第二轮 review 行为。
  证据是主 events 中没有第二轮 `review.round.ready` 或任何第二轮 dim event。
- `next_review_plan:null` 是有效根因候选，但不能单独解释为什么 runtime 没有激活 `review-reentry`。
  因此计划同时修 payload contract 和调度/恢复机制。
- `report.done` 缺失是结果，不是应通过放宽 completion gate 解决的原因。

### System-Wide Impact

本修复主要触碰 builtin preset YAML、preset/schema lint、BDD scenarios 和 agent-facing/operator-facing 文档；Rust runtime 只在现有 handoff / targeted-event / hard-gate 路径上做最小通用修补。
风险最高的是把 runtime 变成隐式 coordinator。实现时禁止新增 preset-specific 分支；如果需要动 runtime，必须能用一个不含 `ce-executor-pipeline-loop` 名称的单元测试说明规则。
所有 runtime 行为修改都必须有 focused tests，并且最终走 nextest。

---

## Implementation Units

### U1. Reproduce and Encode the Broken Fix-Reentry Path

- **Goal:** 把本次真实 run 的断链固化成可失败的 regression scenario。
- **Requirements:** R1, R4, R5, R12, AE1, AE3
- **Dependencies:** 无。
- **Files:**
  - Modify: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop.yml`
  - Modify or Add: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_fix_reentry.yml`
  - Modify: `crates/ralph-core/tests/scenarios.rs`
  - Reference: `ralph-e2e/.ralph/events-20260708-084141.jsonl`
- **Approach:** 新增真实 EventLoop runner 场景，首轮 `review.synthesized` 使用 `p0_count=0,p1_count=2,verdict=pass_with_residuals`，要求 `review-gate -> fix.requested -> fix-planner -> review.complete -> fixer -> fix.done -> review-reentry -> review.round.ready(round=2)`。
  场景必须断言第二轮 `review.round.ready` 出现，且不允许在 `fix.done` 后直接出现 ralph `LOOP_COMPLETE`。
- **Patterns to follow:** 现有 `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop.yml` 的 mock response shape；注册时使用真实 workflow guard runner，不能只检查 iteration 数。
- **Test Scenarios:**
  - 首轮 P1 触发 fix path，`fix.done` 后产生 round 2。
  - `fix.done.head_sha` 成为 round 2 的 `round_base_sha`。
  - `resolved_baseline_sha` 从 executor phase 原样贯穿到 round 2。
  - `review-synthesizer` 在没有第二轮 dim 事件前不能被当作下一跳激活。
- **Verification:** `cargo nextest run -p ralph-core --test scenarios ce_executor_pipeline_loop` 中新增场景先能复现当前失败，再随后续单元修绿。

### U2. Enforce Non-Null `next_review_plan` for `fix.done`

- **Goal:** 在 preset/schema/lint 层让 `fix.done` 的下一轮 review 输入成为机器可验证合同，防止 `null` payload 进入 reentry。
- **Requirements:** R2, R3, R13, AE2
- **Dependencies:** U1。
- **Files:**
  - Modify: `presets/en/ce-executor-pipeline-loop.yml`
  - Modify: `presets/schemas/ce-executor-pipeline-loop.yml` if schema parity requires it
  - Modify: `crates/ralph-core/src/preset_lint/`
  - Modify: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_fix_reentry.yml`
  - Modify: `crates/ralph-cli/src/presets.rs`
- **Approach:** 优先在 preset inline schema / mirrored schema / preset lint 中表达 `fix.done.next_review_plan` 必须是对象，并包含 `focus_areas`、`fixed_findings`、`verification_performed`、`residual_risks`、`diff_ranges`。
  如果现有 runtime schema 只支持 `required_fields`，不要为这个 preset 新增 runtime 状态机；新增 preset_lint finding，要求 loop preset 的 `fix.done` schema、examples 和 instructions 都把 `next_review_plan` 写成非空对象。
  修改 fixer instructions：不能发 `next_review_plan:null`；即使没有残余风险，也必须发空数组字段齐全的对象。
- **Patterns to follow:** `event_policy.schemas.required_fields` 的现有写法；`preset_lint` 中按 finding_id 输出 actionable hint 的模式。
- **Test Scenarios:**
  - `next_review_plan:null` 的 `fix.done` 在 preset lint 或 scenario validation 中产生明确 finding。
  - 字段齐全但数组为空的 `next_review_plan` 可通过。
  - `review-reentry` mock 能直接把该对象转发为 `review.round.ready.review_plan`。
- **Verification:** targeted preset lint 和 scenario 都能区分 null 与结构化对象。

### U3. Verify and Minimally Fix Generic Handoff Priority

- **Goal:** 确保 preset 声明出的单消费者 handoff 在下一轮获得调度；只修通用优先级缺口，不加 preset-specific 分支。
- **Requirements:** R1, R6, AE1, AE3
- **Dependencies:** U1。
- **Files:**
  - Modify if needed: `crates/ralph-core/src/event_loop/mod.rs`
  - Modify/Add: `crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs`
  - Modify/Add: `crates/ralph-core/src/event_loop/tests/next_hat_topic_preemption.rs`
- **Approach:** 先写/扩展不含 preset 名称的通用测试：给定 topic `a.done` 的唯一 consumer 是 `consumer-b`，当 `a.done` accepted 后，`next_hat()` 必须优先返回 `consumer-b`，并在普通 round-robin / stale recovery residue 前执行。
  再用 `ce_executor_pipeline_loop_fix_reentry.yml` 作为集成回归，证明这个通用规则覆盖 `fix.done -> review-reentry`。
  只有当通用测试失败时才修改 runtime；修改范围限制在现有 handoff index、pending queue、targeted event fast path 或 handoff clear 时机，禁止新增 `if preset == ...`、`if topic == "fix.done"`、`if hat == "review-reentry"`。
- **Patterns to follow:** `next_hat_topic_preemption` 对 topic-exact predicate 的现有测试风格；`handoff_dispatch.rs` 对 accepted handoff 和 timeout escalation 的测试风格。
- **Test Scenarios:**
  - 通用 topic `a.done` accepted 后，`next_hat()` 返回唯一 consumer。
  - 存在 unrelated `task.resume` 或 stale recovery residue 时，topic-exact handoff 仍优先。
  - `handoff_tracker.expired` 生成 target=`consumer-b` 的 `task.resume` 后，targeted fast path 选中 `consumer-b`。
  - consumer prompt 构建后 pending handoff 被清除，后续不再重复 timeout。
- **Verification:** targeted event-loop tests 通过，并使 U1 场景推进到第二轮。

### U4. Constrain Recovery So It Does Not Override Fresh Handoffs

- **Goal:** 防止旧 recovery / aggregate timeout 覆盖更新鲜、目标明确的 pending handoff；实现必须是通用优先级规则。
- **Requirements:** R6, R7, AE3
- **Dependencies:** U3。
- **Files:**
  - Modify if needed: `crates/ralph-core/src/event_loop/mod.rs`
  - Modify/Add: `crates/ralph-core/src/event_loop/tests/`
  - Modify if needed: `crates/ralph-core/src/diagnosis/`
  - Reference: `ralph-e2e/.ralph/diagnostics/channel-routing-fallback-2026-07-08T09-11-40.md`
- **Approach:** Audit recovery branches around aggregate timeout and hard fallback, but do not add a `review-synthesizer` special case unless an existing feature already has that contract.
  Generic rule: when a fresh pending handoff has a concrete target, recovery candidates with older or less-specific evidence are not eligible to preempt it.
  For review aggregation, recovery remains valid only when there is current-round evidence that the aggregate consumer is actually waiting on an incomplete dimension set; it must not be triggered merely by no-progress after an unrelated fresh handoff.
- **Patterns to follow:** Existing handoff index / target_hat checks and diagnosis responder safe_target handling.
- **Test Scenarios:**
  - A fresh pending handoff to target B prevents older recovery from targeting unrelated C.
  - During a true incomplete six-dim review, aggregate timeout can still route to the aggregate consumer when that is the intended recovery.
  - Three consecutive empty channel activations produce typed blocked/termination instead of manual stop.
- **Verification:** Logs in reproduced scenario no longer show `review-synthesizer` fallback before round 2 dim events.

### U5. Promote `dim:*` Scope Violations to Hard Reject

- **Goal:** Make split dimension hats inherit the read-only hard reject semantics previously limited to `dimension-reviewer`.
- **Requirements:** R9, R10, R11, R14
- **Dependencies:** 无。
- **Files:**
  - Modify: `crates/ralph-core/src/event_loop/mod.rs`
  - Modify: `crates/ralph-core/src/preset_lint/dimension_reviewer_write_paths.rs`
  - Modify: `crates/ralph-core/src/preset_lint/finding_id.rs`
  - Modify: `crates/ralph-core/src/preset_lint/mod.rs`
  - Modify: `presets/en/ce-executor-pipeline-loop.yml`
  - Modify: `crates/ralph-core/tests/scenarios/`
  - Modify: `crates/ralph-cli/src/loop_runner/tests/legacy.rs` or newer scoped test file if this coverage has moved.
- **Approach:** Replace string-only `hat_id == "dimension-reviewer"` with a predicate that recognizes read-only dimension reviewer roles.
  Conservative v1 predicate: `hat_id == "dimension-reviewer" || hat_id.starts_with("dim:")` combined with `disallowed_tools` containing `Edit` or `Write`.
  Update lint to inspect all matching hats for forbidden `allowed_write_paths` and produce finding messages that name the actual hat.
  Update dim instructions so findings/Covers suggestions are reported in review product files, not applied to `docs/plans/*.md`.
- **Patterns to follow:** Existing `ScopeViolationHardRejected` termination reason and display handling; `dimension_reviewer_write_paths` finding style.
- **Test Scenarios:**
  - `dim:goal-alignment` modifying `docs/plans/foo.md` triggers hard reject.
  - Non-dimension executor modifying code still follows normal allowed behavior.
  - A dim hat with no forbidden writes and only `.ralph/review/...` product output does not trip scope violation.
  - Lint catches `dim:testing.allowed_write_paths: ["docs/plans/"]`.
- **Verification:** Scope violation tests pass and the run no longer accumulates six soft `MissingField` failures for dim plan edits.

### U6. Make Hard Gate Exhaustion Produce a Typed Outcome

- **Goal:** Replace “hard gate exhausted then manual stop” with an explicit generic runtime outcome that operators and reports can consume.
- **Requirements:** R7, R8
- **Dependencies:** U3, U4。
- **Files:**
  - Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
  - Modify: `crates/ralph-core/src/event_loop/mod.rs`
  - Modify: `crates/ralph-core/src/event_loop/types.rs`
  - Modify: `crates/ralph-core/src/event_loop/termination.rs` only if trigger mapping is used
  - Modify: `crates/ralph-core/tests/scenarios/`
  - Modify: `crates/ralph-cli/src/display.rs`
- **Approach:** When any hat with publish obligation emits no event for the configured hard gate threshold, the loop should emit a typed blocked/termination reason that includes hat id, expected publishes, and last trigger topic.
  This is not a loop-preset branch: the same reason applies to any preset whose hat repeatedly produces an empty channel while under a publish obligation.
  The loop must still keep completion strict: no synthetic `report.done`, no raw `LOOP_COMPLETE`, and no conversion to generic manual `Stopped`.
- **Patterns to follow:** Existing `ScopeViolationHardRejected` typed termination path; completion rejection injection that keeps `LOOP_COMPLETE` gate strict.
- **Test Scenarios:**
  - Three empty activations of any obligated hat produce a stable termination reason instead of `"stopped"`.
  - The final summary distinguishes hard gate exhausted from user manual stop.
  - No synthetic `report.done` is accepted unless reporter actually emits it.
- **Verification:** A reproduced empty-channel scenario terminates deterministically and records machine-readable reason.

### U7. Repair Preset Instructions and Contract Drift

- **Goal:** Align `ce-executor-pipeline-loop` instructions, schemas, and tests with the real topology.
- **Requirements:** R2, R3, R11, R15
- **Dependencies:** U2, U5。
- **Files:**
  - Modify: `presets/en/ce-executor-pipeline-loop.yml`
  - Modify: `crates/ralph-cli/src/presets.rs`
  - Modify: `presets/manifest.yml` only if embedded content plumbing requires regeneration checks
  - Modify: `presets/index.json` only if user-facing description needs precision
  - Modify: `scripts/ralph-zsh-plugin.zsh` only if builtin list text changes
- **Approach:** Fix internal contradictions:
  - `review-synthesizer` step 8 must include `review_round` and `round_base_sha` in example payload.
  - `fix-planner` instructions currently say it receives `review.synthesized` while trigger is `fix.requested`; rewrite from the hat perspective.
  - `alignment` trigger is `review.accepted`, so instructions must not say “From `fix.done` payload”; required fields must be passed through `review.accepted` or retrieved via sanctioned `ralph events` only if needed.
  - `reporter` may use `ralph events --events-source main --output json`, but instructions must avoid direct internal ledger paths and must require policy-check before both `report.done` and `LOOP_COMPLETE`.
  - Dim hats must not run commands the instructions forbid, and must not edit original plan docs.
- **Patterns to follow:** AGENTS hard rule 4 for hat perspective; existing `ralph-tools-emit` references instead of copying full command doctrine.
- **Test Scenarios:**
  - CLI preset tests assert critical phrases for non-null `next_review_plan`, fix-planner trigger source, and alignment trigger source.
  - No instruction mentions direct reads of `.ralph/events.jsonl`, `.ralph/supervisor.db`, or `.ralph/loops.json`.
  - All emitted example payloads satisfy inline required fields.
- **Verification:** `cargo nextest run -p ralph-cli --bin ralph -- preset` and preset lint targeted tests pass.

### U8. Extend Loop Scenarios for Accepted, Blocked, and Null-Payload Paths

- **Goal:** Cover every terminal branch and the failure mode seen in this run.
- **Requirements:** R12, R13, R14, AE1-AE5
- **Dependencies:** U1-U7。
- **Files:**
  - Add: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_fix_reentry.yml`
  - Add: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_max_round_blocked.yml`
  - Add: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_null_next_review_plan.yml`
  - Modify: `crates/ralph-core/tests/scenarios.rs`
- **Approach:** Keep existing first-round happy path, then add focused scenarios:
  - Fix/reentry happy path: first round P1, fix done, second round no P0/P1, accepted, report done, complete.
  - Round policy path: round 4 has P1 but no P0, accepted.
  - Max-round blocked path: round 6 still has P0, emits `review.loop.blocked -> report.done -> LOOP_COMPLETE`.
  - Null payload path: `fix.done.next_review_plan:null` does not silently continue.
  - Dim scope path: dim hat tracked-file edit hard rejects.
- **Patterns to follow:** Existing scenario `payload_matches`, `absent_events`, and `event_topic_counts` assertions.
- **Test Scenarios:** Same as approach list; each scenario must assert both expected events and absent wrong events.
- **Verification:** `cargo nextest run -p ralph-core --test scenarios ce_executor_pipeline_loop` passes.

### U9. Sync Agent-Facing and Operator-Facing Documentation

- **Goal:** Keep prompt-injected docs and preset author/review skills aligned with new contracts.
- **Requirements:** R15
- **Dependencies:** U7, U8。
- **Files:**
  - Inspect/Modify: `crates/ralph-core/data/ralph-tools.md`
  - Inspect/Modify: `crates/ralph-core/data/ralph-tools-emit.md`
  - Inspect/Modify: `crates/ralph-core/data/ralph-tools-cmdref.md`
  - Inspect/Modify: `skills/ralph-preset-common/references/agent-native-model.md`
  - Inspect/Modify: `skills/ralph-preset-common/references/author-checklist.md`
  - Inspect/Modify: `skills/ralph-preset-common/references/commands.md`
  - Inspect/Modify: `skills/ralph-preset-common/references/finding-rubric.md`
  - Inspect/Modify: `skills/ralph-preset-common/references/patterns.md`
  - Modify: `CLAUDE.md`
  - Modify: `AGENTS.md`
- **Approach:** Update only if behavior visible to agents or preset authors changes.
  Likely updates: document that split `dim:*` read-only reviewers inherit dimension-reviewer hard scope semantics; preset author checklist should flag `required_fields` that allow `null` where the next hat requires an object; finding rubric should cover loop preset reentry contract.
  If `CLAUDE.md` changes, sync `AGENTS.md` by copying so both files remain identical.
- **Patterns to follow:** Agent skill guide readability rule: trigger condition, command/action, where fields come from, stop condition; no internal function names or source line references in injected docs.
- **Test Scenarios:**
  - `scripts/check-cli-doc-drift.sh` passes if command/help docs are touched.
  - Manual grep confirms injected docs do not mention internal ledger paths as agent actions.
  - `CLAUDE.md` and `AGENTS.md` are byte-identical after sync.
- **Verification:** Documentation diff only describes agent-visible or preset-author-visible behavior.

### U10. Final Validation and Baseline Run

- **Goal:** Prove the repair is safe across targeted and full test gates.
- **Requirements:** R12-R15
- **Dependencies:** U1-U9。
- **Files:**
  - No planned source files; this unit validates the whole diff.
- **Approach:** Run formatting, targeted tests, preset lint, scenario tests, doc drift, and full baseline with the project-mandated nextest entry points.
- **Patterns to follow:** AGENTS Build & Test hard rules.
- **Test Scenarios:**
  - Targeted preset lint catches schema/instruction drift.
  - Scenario tests cover all loop closure branches.
  - Full workspace test proves no shared runtime regression.
- **Verification:** Commands in Verification Contract pass, or final implementation report records exact command, failure, and residual risk.

---

## Verification Contract

| Gate | Command | Proves |
|---|---|---|
| Format | `cargo fmt` | Rust formatting is stable. |
| CLI preset lint subset | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | CLI preset lint paths stay green. |
| Core preset lint subset | `cargo nextest run -p ralph-core -- preset_lint` | Core preset lint catches new contracts. |
| Embedded preset equality | `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded` | Builtin YAML and embedded content stay synchronized. |
| Loop scenario subset | `cargo nextest run -p ralph-core --test scenarios ce_executor_pipeline_loop` | The repaired loop paths pass real workflow scenarios. |
| CLI docs drift | `scripts/check-cli-doc-drift.sh` | Required if CLI help or injected skill docs touched. |
| Full baseline | `./scripts/run-tests.sh` | Workspace-level regression check. |

If full baseline shows timing/concurrency flake, use only the documented fallback:

```bash
RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh
```

Do not run bare `cargo test -p ralph-cli` or `cargo test -p ralph-cli --bin ralph`.

---

## Definition of Done

- `fix.done -> review-reentry -> review.round.ready(round=2)` is covered by a real scenario and passes.
- `next_review_plan:null` no longer silently advances or causes empty-channel recovery.
- `review-synthesizer` is not activated by recovery while the latest accepted handoff is `fix.done -> review-reentry`.
- `review.accepted -> alignment -> reporter -> report.done -> LOOP_COMPLETE` passes after a fix/re-review path.
- `review.loop.blocked -> reporter -> report.done -> LOOP_COMPLETE` passes at max review rounds.
- `dim:*` read-only scope violations hard reject like historical `dimension-reviewer`.
- Hard gate exhaustion records a typed outcome rather than ending as manual `"stopped"`.
- Preset instructions, inline schemas, CLI preset tests, scenario tests, and operator/agent docs agree.
- `CLAUDE.md` and `AGENTS.md` remain identical if touched.
- Full validation uses `cargo nextest run` series and `./scripts/run-tests.sh`.
