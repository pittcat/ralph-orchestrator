---
date: 2026-06-21
topic: serial-preset-root-cause-fix
related:
  - docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md
  - docs/brainstorms/2026-06-20-serial-preset-precheck-as-linter-requirements.md
  - docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md
  - docs/achieved/plan/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md
---

# Ralph serial preset 根因修复 — 需求文档

## Summary

把 `ce-executor-serial` 当前 30 天反复出现的 21 项症状收敛为 **4 个根因修复包**，不再逐项打补丁。核心目标是让 serial preset 能跑通 `coordinator → executor → review → fix → plan-gate → shipper → reporter → LOOP_COMPLETE` 全链路。

修复包：

1. **handoff 宏观边契约修复** — 让 runtime / lint / engine 三层对「谁是宏观边、是否需要 handoff artifact」有同一答案。
2. **ralph 边界与反馈** — 堵住 `ralph` 伪 hat 越权发业务事件，并让 steward/reviewer/fixer 能被 `task.resume` 唤醒。
3. **lint / runtime 一致性** — 消除双轨拒收、计数器污染、路径歧义、可观测性缺失。
4. **state_projection 应用收尾** — 把已落地的投影机制真正接到 prompt 和 steward 决策里。

## Problem Frame

过去 30 天 serial preset 在同类 plan 上失败 5 次以上，每次诊断后补一个 patch，但根因未收敛。2026-06-21 的审计显示：一次典型 run 在 `[1] coordinator → work.ready` 就彻底阻断 —— 10 次 CLI 拒收、ralph 越权重发、recovery.jsonl 被 `hat_handoff_*` 占满，executor 从未被激活，后续 review/fix/ship/reporter 全部未触发。

根本原因不是 agent 不会写 handoff 文件，而是编排器内部对宏观边契约的三层视图不一致：

- `HANDOFF_TOPIC_SEEDS` 硬编码 4 条，漏掉 serial preset 里 13+ 条宏观边。
- `is_macro_edge` 没有实现自环排除，linter 与 runtime gate 对同一 topic 答案不同。
- engine gate 构造 `ProtocolView` 不带 `HandoffIndex`，与 CLI lint 路径的 SSOT 不一致。
- handoff 提示块被压在 `## WAVE CONTEXT` 下面，agent 实际看不到。

同时，`ralph` fallback 路径缺乏硬边界：业务事件被拒后仍落盘，stall detector 被假活跃欺骗，`task.resume` 无法唤醒 reviewer/fixer/steward。

最后，state_projection Phase 1 已落地但 prompt 注入和 steward 读法没跟上，导致 `work.done` 后 progress gate 仍可能误拒。

## Actors

- A1. **Operator**：运行 `ralph run` 并诊断失败的人。需要 serial preset 稳定跑到 `LOOP_COMPLETE`。
- A2. **Workflow hat**（coordinator / executor / reviewer / fixer 等）：按 preset 拓扑 emit 事件、消费事件的 agent。
- A3. **Orchestrator**：`event_loop`、`loop_runner`、lint/runtime gate、state projector 的集合。
- A4. **Preset maintainer**：修改 `presets/schemas/ce-executor-serial.yml` 或 `presets/en/ce-executor-serial.yml` 的人。

## Key Flows

### F1. coordinator 激活 executor

- **Trigger:** 新 loop 启动，`coordinator` 需要发 `work.ready`。
- **Actors:** A2 coordinator, A3 Orchestrator, A2 executor
- **Steps:**
  1. `build_prompt` 在 prompt 最顶部注入当前 hat 的 upstream handoff 指令。
  2. coordinator emit `work.ready`。
  3. CLI lint / engine gate / runtime gate 三层的宏观边判断一致，确认需要 handoff artifact。
  4. `auto_handoff_prepare` 在 workspace 下写出 artifact。
  5. 事件落盘，`executor` 被激活。
- **Outcome:** `work.ready` 成功把执行权交给 executor，无 recovery noise。
- **Covered by:** R1, R2, R3, R4, R5

### F2. ralph 越权事件被拦截

- **Trigger:** loop runner 或 agent 以 `hat=ralph` 尝试发业务 topic（如 `work.ready`、`task.resume`）。
- **Actors:** A2 agent, A3 Orchestrator
- **Steps:**
  1. CLI emit 路径遇到 `hat=ralph` + 非 control topic 直接 bail。
  2. loop runner 内部 publish 路径遇到同样情况直接 reject 并写 recovery，不写 events.jsonl。
  3. stall detector 不把 ralph fallback 业务事件算作有效进展。
  4. 达到 stall 阈值后触发 `task.resume` 或 `human.guidance`。
- **Outcome:** ralph 不能绕过 workflow gate 推进流程。
- **Covered by:** R6, R7, R8, R9

### F3. work.done 后 progress 不漂移

- **Trigger:** executor emit `work.done`。
- **Actors:** A2 executor, A3 Orchestrator, A2 plan-gate
- **Steps:**
  1. `state_projector` 按 `actions_chain` 先 close_task 再 mark_step_completed。
  2. `tasks.jsonl` 和 `progress.md` 同步更新。
  3. `ORCHESTRATOR CONTEXT` 注入块读到 projector 缓存，而非过时文件。
  4. `queue.advance` 或下一条 `work.ready` 被 `progress_task_gate` 放行。
- **Outcome:** 不会因 Completed Steps 滞后导致 plan-gate 误拒。
- **Covered by:** R14, R15, R16, R17

### F4. recovery 噪音收敛为可观测信号

- **Trigger:** 某 hat 连续 emit 不符合契约的事件。
- **Actors:** A2 agent, A3 Orchestrator, A1 Operator
- **Steps:**
  1. engine gate / runtime gate 拒绝时写入结构化 reason_code。
  2. 同一 hat+reason_code 短时间累计 ≥ 3 次升级成 `drift_finding`。
  3. `ralph diagnose` 把 drift 呈现为诊断结论，而非 14 条无结构 recovery。
- **Outcome:** operator 看一眼 diagnose 就知道是哪道门在拒收。
- **Covered by:** R10, R13

## Requirements

### handoff 宏观边契约修复

- R1. `HANDOFF_TOPIC_SEEDS`（或其在 runtime 的等效派生源）必须覆盖 `ce-executor-serial` 的全部宏观边话题，包括但不限于 `review.dimension.{ready,done,failed}`、`review.dimensions.complete`、`review.passed`、`review.failed`、`review.complete`、`fix.applied`、`fix.exhausted`、`work.done`、`plan.complete`、`REVIEW_COMPLETE`、`report.done`、`LOOP_COMPLETE`。
- R2. `ProtocolView::is_macro_edge` 必须实现自环排除：当 `from_hat` 等于该 topic 的唯一 consumer 时返回 `false`；测试必须传入真实 `from_hat` 而非空字符串绕过。
- R3. engine gate 与 CLI lint 必须构造同一类 `ProtocolView`：统一使用 `from_event_loop_with_index` 并传入当前 `HandoffIndex`；禁止 engine gate 单独使用无 index 的视图。
- R4. handoff 提示块（`build_emit_instructions` 生成的 emit 指令）必须出现在 prompt 最顶部，高于 `## WAVE CONTEXT`，确保 agent 第一眼看到。
- R5. `auto_handoff_prepare` 与 `LintPaths::under_handoff_dir` 必须处理 workspace root 冷启动和路径规范化边界：`parent()==Some("")` 时不静默失败；`strip_prefix` 前 canonicalize workspace root。

### ralph 边界与反馈

- R6. loop runner 内部 publish 路径必须拦截 `hat=ralph` 的业务 topic：非 `RALPH_CONTROL_TOPICS` 内的话题不得写入 events.jsonl，必须写 recovery 并触发错误日志。
- R7. `dimension-reviewer`、`fixer`、`progress-steward` 的 `triggers` 必须包含 `task.resume`；`progress-steward` 额外包含 `human.guidance`。
- R8. stall detector 必须区分「真实业务事件」与「ralph fallback 业务事件」：ralph 伪造的活跃不能推迟 stall 判定。
- R9. CLI emit 对 `ralph` 业务 topic 的 bail 语义必须与 event origin guard、loop runner 防御一致，错误信息必须指向「改用注册 workflow hat」。

### lint / runtime 一致性

- R10. engine gate 拒绝事件时必须向 recovery.jsonl 输出结构化 reason_code（如 `engine_rejected:required_field`、`engine_rejected:macro_edge`），不能只有自由文本 message。
- R11. engine gate 必须使用 `LintResumeHint::from_typed_rejection` 按 `RejectionKind` 路由，禁止按 message 子串字符串匹配。
- R12. `LintPaths::under_handoff_dir` 必须 canonicalize workspace root，避免 `./`、相对路径、tmp 路径导致 fallback 成绝对路径。
- R13. 同一 hat+reason_code 在 5 分钟内累计拒收 ≥ 3 次必须升级为 `drift_finding`（severity=Warning），让 `ralph diagnose` 呈现根因而非噪声列表。

### state_projection 应用收尾

- R14. `preset_lint::run_preset_lint` 的运行顺序必须把 `state_projection` 检查放在 `schema_parity` 之前，保证报告行号与发现顺序一致。
- R15. `## ORCHESTRATOR CONTEXT` 注入必须读取 projector 内存缓存（或 canonical 文件），与 `state_projector::apply` 后的状态一致；projection disabled 时注入 stub 说明。
- R16. `progress-steward` 的 instructions 必须从「直读 tasks.jsonl / progress.md / 四文件决策树」改为「读 `## ORCHESTRATOR CONTEXT`」。
- R17. `presets/schemas/ce-executor-serial.yml` 的 `state_projection.actions_chain.work.done` 必须配置为 `close_task` 后接 `mark_step_completed`，`preset_lint` 对该顺序报错。

## Acceptance Examples

- AE1. **Covers R1, R2, R3.** Given `ce-executor-serial` preset，when engine gate 和 CLI lint 分别判断 `review.dimension.ready` 是否为宏观边，then 两者结论一致，且 coordinator 自环 `queue.advance` 不被误判为宏观边。
- AE2. **Covers R4, R5.** Given 冷启动 workspace，when coordinator 收到 prompt，then prompt 顶部出现 `## HAT HANDOFF` / emit 指令块；`auto_handoff_prepare` 能在 `.ralph/...` 下创建首份 artifact 而不静默失败。
- AE3. **Covers R6, R9.** Given loop runner 尝试以 `ralph` hat publish `work.ready`，then events.jsonl 不增加该行，recovery.jsonl 出现 `ralph_business_topic_rejected` 记录。
- AE4. **Covers R7, R8.** Given executor 不响应导致 stall，when ralph fallback 连发 2 条业务事件，then stall detector 仍触发 `task.resume(target=progress-steward)`，steward 被唤醒。
- AE5. **Covers R14, R15, R16, R17.** Given executor emit `work.done` 含 `task_id` 和 `step`，then `tasks.jsonl` 关闭 task、`progress.md` 标记 step 完成；下一条 `queue.advance` 的 `progress_task_gate` 放行；steward prompt 中的 `## ORCHESTRATOR CONTEXT` 显示 Completed Steps 已更新。

## Success Criteria

- SC1. 用 `ce-executor-serial` 跑同一个 plan，能到达 `LOOP_COMPLETE`，无 `consecutive_failures`、无用户 abort。
- SC2. 一次 run 中 `recovery.jsonl` 的 `hat_handoff_*` 条目收敛到 0 或偶发可自恢复（agent 一次修正即通过）。
- SC3. `ralph diagnose` 对失败 run 能给出结构化根因（如 `hat_handoff_macro_edge_mismatch`、`ralph_business_topic_issued`、`engine_gate_required_field`），而非 14 条无分类 recovery。
- SC4. 修改 `presets/schemas/ce-executor-serial.yml` 中宏观边相关配置后，`cargo build` 能通过一致性校验；linter、engine gate、runtime gate 三者不再对同一事件给出矛盾结论。

## Scope Boundaries

- 本次覆盖 `ce-executor-serial` 的 handoff 契约、ralph 边界、lint/runtime 一致性、state_projection 应用。
- 本次覆盖 `crates/ralph-core/src/preset/engine/`、`crates/ralph-core/src/event_loop/`、`crates/ralph-core/src/hat_handoff/`、`crates/ralph-core/src/state_projector/`、`crates/ralph-cli/src/commands/emit.rs`、`crates/ralph-cli/src/loop_runner/runner.rs`、`presets/en/ce-executor-serial.yml`、`presets/schemas/ce-executor-serial.yml` 中与本需求直接相关的改动。

### Deferred for later

- wave worker 共享状态抽象错误 / supervisor 协议 6 件套（§9 / 21.8）。
- `ce-executor-isolated` 与 `ce-executor-wave` 的移除或重构。
- `ralph-tools*.md` 完整文档同步（P2 级别，可在主修复验证后批量补）。
- `loop.cancel` 与 `loop.terminate` 的语义统一、loops.json stale 清理。
- agent 写 handoff 文件走 `ralph tools handoff prepare --force` 的 instruction 习惯培养（可在 prompt 里加 HARD RULE，但不单独建工具）。

### Outside this product's identity

- 把 Ralph 改造成通用 workflow DSL 或可视化编排器。
- 重写整个 EventBus / StateMachine。
- 为 isolated/wave preset 设计新的 handoff 协议。

## Key Decisions

- **D-1. 根因优先，不打 21 个补丁。** 报告列出的 21 项症状是 4 个根因的不同表现；先修根因，再评估剩余症状是否自然消失。
- **D-2. 把 state_projection Phase 2 应用纳入 Phase 1。** 它是编排状态一致性的最后一公里，与 serial preset 跑通直接相关。
- **D-3. 不碰 wave supervisor 协议升级。** 它是独立战略需求，当前无 plan；先让 serial preset 端到端可用。
- **D-4. SSOT 仍落在 `presets/schemas/ce-executor-serial.yml` + `preset/engine/`。** 不复用 2026-06-20 已废弃的 `ralph-proto/serial_protocol` 方案。
- **D-5. 保持运行时 gate 作为兜底，lint 与 runtime 共用同一 `ProtocolView`。** 不追求 100% 事前拦截，但要求三层对协议有同一答案。

## Dependencies / Assumptions

- **DEP-1.** `crates/ralph-core/src/state_projector/` 已具备 `apply`、`bootstrap_from_disk`、task/progress 写能力（Phase 1 已落地）。
- **DEP-2.** `HandoffIndex`、`ProtocolView`、`preset/engine/gates.rs` 已存在，可接受索引化视图。
- **DEP-3.** `RALPH_CONTROL_TOPICS` 已在 `crates/ralph-core/src/event_origin.rs` 定义。
- **DEP-4.** nextest 测试环境可用，`ralph-core` BDD scenarios 可复用。

- **ASSUM-1.** 修复期间允许少量 preset YAML 调整（triggers、state_projection mapping）。
- **ASSUM-2.** Operator 接受 `ralph` hat 不再能发任何业务 topic；确有紧急情况走 operator CLI 或 bypass 机制。

## Outstanding Questions

### Resolve Before Planning

- （无）

### Deferred to Planning

- **[Technical]** `HANDOFF_TOPIC_SEEDS` 是直接扩常量，还是改为 runtime 从 `HandoffIndex` 动态派生？后者更治本但改动面更大。
- **[Technical]** `build_emit_instructions` 置顶后，是否会影响 `review-synthesizer` 对 `## WAVE CONTEXT` 的解析习惯？需要评估 token 优先级。
- **[Needs research]** `progress-steward` 改读 `## ORCHESTRATOR CONTEXT` 后，原四文件决策树中哪些分支可以删除，哪些保留为 fallback。

## Next Steps

-> `/ce-plan` 进行结构化实施规划。
