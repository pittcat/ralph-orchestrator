---
date: 2026-06-12
title: WAC rollout tiered gates — 接线顺序、假阳性处理与 003 计划落地
module: developer-experience
tags: [wac, lint, preset, builtin-strict, tiered-gates, ktd-7]
problem_type: workflow-mechanism
---

# WAC Rollout Tiered Gates（2026-06-12）

`docs/plans/2026-06-12-002-feat-workflow-activation-contract-plan.md`
把 WAC 规则、HandoffIndex、HandoffTracker 落地为单测层。`docs/plans/2026-06-12-003-feat-wac-rollout-completion-plan.md`
把 WAC 推进到对用户生效的护栏。三类知识是这次 rollout 沉淀的核心：

## 1. 接线顺序：先语义，再 preset 修，最后接 lint

`run_preset_lint(Strict)` 把所有 lint findings 提升到 Error。**如果先
做接线（WRC-U1）**，`ce-executor-isolated` 会立即产出 19 个
WAC Error 而阻断 `ralph preset check`。**正确顺序是**：

1. **WRC-U2**：定义虚拟 publisher `ralph` + runner-injected topics
   （`starting_event`、`cancellation_promise`），并锁定 R2 窄语义
   文档。这消除了"`executor` triggers `work.ready` 但不 publish
   `work.ready`"的假阳性。
2. **WRC-U3 (preset 端)**：在接线前先 `cargo test -p ralph-cli
   presets::tests::test_tier_0_wac_presets_have_no_wac_errors`，
   让 `ce-executor-isolated` 自身 WAC 干净。如果先做 lint 接线，
   失败信息会指向"接入式错误"而不是"preset 拓扑错误"，调试时间
   增加 5–10 倍。
3. **WRC-U1 (接线)**：lint 接入 + aggregator Step 2b always-on。
4. **WRC-U3 (KTD-7)**：`wac_severity(strict, source_is_builtin_embedded)`
   升级机制。

KTD-2 之前我们把 lint 门控在 `fail_on_warnings` 之后；KTD-2 + WRC-U1
之后 lint 始终 on，severity 由 strictness 与 builtin flag 决定。这个
flip 是 003 计划最大的行为变更——所有"good preset"测试 fixture 都
需要补 `topic_format_whitelist: [LOOP_COMPLETE]` 与
`tasks.coordinator_hats`。

## 2. 假阳性的三类来源

WAC 上线时遇到了三类假阳性（都已修在 WRC-U2/U3 内）：

1. **`executor` 不 publish `work.ready`**：executor 触发 plan-gate
   发出的 `work.ready` 但只 publish `work.done`。**字面 R2**会
   报 trap；**窄 R2**（唯一消费者 + 无 closure path）会沉默，因为
   executor 的 `work.done → review-coordinator → plan-gate` 闭合
   路径存在。修复：把 R2 文档改为窄语义 + 在测试中
   `re_emit_trap_does_not_fire_on_healthy_handoff_chain` 锁定。

2. **`plan-gate` 触发 `loop.cancel` 但 preset 无 publisher**：
   `loop.cancel` 是 `cancellation_promise`，由 loop runner 的 ralfhat
   在用户请求取消时发布。WAC 看不到这个虚拟 publisher。修复：R5
   增加 `cancellation_promise` 局部豁免（与 `starting_event` 对称）。

3. **2-hop BFS 截断 5-node closure 链**：
   `ce-executor-isolated` 的 `work.done → review-coordinator →
   plan-gate → shipper → reporter → LOOP_COMPLETE` 是 5 跳闭合
   路径。原始 002 计划 2-hop BFS 在第 2 跳就停，看到
   `work.done → review-coordinator`，判定未闭合。修复：WRC-U3 把
   `EGRESS_MAX_HOPS` 提到 4，能覆盖 ce-executor 链。

## 3. Tiered Gate：KTD-7 的实施

不是所有 builtin preset 都是 Tier-0。一次修完所有 builtin 的 WAC
问题等于重构所有拓扑——`ce-executor-wave` 的 dispatcher 路径把
事件动态注入静态图，WAC 因此看不到闭合。`autoresearch` 的
多分支 completion path 让 WAC R3 报 activation_egress_missing。

**Tier-0 = `["ce-executor-isolated"]` 是 WAC-clean**。其余 builtin
走 warn-only，CI 不阻断。`scripts/validate-builtin-presets.sh`
的 `TIER_0_WAC_PRESETS` 数组与
`crates/ralph-cli/src/presets.rs::TIER_0_WAC_PRESETS` 常量必须同
步更新——两处重复是刻意的，shell 不能查 ralph 二进制。

晋升 Tier-0 的标准：preset 在 strict 模式下 WAC findings 为零。
`test_tier_0_wac_presets_have_no_wac_errors` 是晋升的 in-process
断言（CI 中也跑）；`scripts/validate-builtin-presets.sh --strict`
的 Tier-0 fast-path 阻断是 CI 门禁。

## 4. KTD-7 行为的两个执行点

`source_is_builtin_embedded` 升级 WAC 严重度的逻辑在两处生效：

1. **Aggregator 路径**：`RuntimeContractAggregator::aggregate` 在
   Step 2 把 `source_label` 喂给 `source_label_is_builtin_embedded`，
   把结果作为第三个参数传给 `run_preset_lint`。这是
   `ralph preset check -H builtin:foo` 的路径。
2. **CLI 硬门路径**：`run_command` 检查 `HatsSource::Builtin(_)`，
   把 `bool` 透传给 `run_loop_impl`，再透给
   `enforce_preset_lint_gate`。这是 `ralph run -H builtin:foo`
   的路径。

两处使用同一 `source_label_is_builtin_embedded` helper，但调用栈
不同。CLI 路径不经过 aggregator，因为它要避免构造完整
`RuntimeContractReport`；只调 `run_preset_lint(Strict, builtin=true)`。
这意味着 `enforce_preset_lint_gate` 的 `source_is_builtin_embedded`
参数对 `runner.rs` 是显式传递（不是从 `config` 字段反推），避
免了"config 内嵌结构"与"外部 `-H` 标志"两个来源的歧义。

## 5. HandoffTracker 集成的钩子顺序

WRC-U4 把 HandoffTracker 挂到主循环的 3 个钩子点。**顺序敏感**：

1. **policy-accept**（`apply_event_policy_validation` 之后）：每
   个 accepted event 调 `on_handoff_accepted`。`policy_rejections`
   集合（policy/origin/scenario 拒收）**不**进入 tracker，否则
   会产生 phantom escalation。
2. **hat-activation 完成**（`hat_lifecycle_tracker.complete` 同一
   行）：调 `on_hat_activated(consumer)` 清空该 consumer 的 pending。
3. **iteration tick**（`process_output` 开头）：调 `expired(now)`
   收齐 escalations，每个 escalation 合成 `task.resume`（payload
   含 topic/consumer/event_id/safe_target/reason）发布到 bus。

钩子 1 必须在 `events = policy_result.events` 之前完成（move
之前 borrow）——这是 event_loop/mod.rs:4919 的一处隐性顺序约束。
钩子 3 的合成事件用 `Event::with_source(HatId::from(safe_target))`
而非 `with_hat`（`with_hat` 在 Event 上不存在），让
`EventOriginGuard` 接受（safe_target 必然在自己 hat 的 publishes
列表里）。

## 6. 002 plan 状态变更：`active` → `partial-complete`

002 plan 在 commit 84b8281 落地后状态为 `active`——意味着"全部
R1–R15 已完成"。WRC-U7 把状态改为 `partial-complete`：

- 002 plan 落地了 WAC 规则、HandoffIndex、HandoffTracker、单测。
- 002 plan **未**完成 R6（builtin 违反 WAC 必须拒绝启动）、
  R8（30s handoff 超时 escalation）、R13（preset 同步到 CI）。
- 这三件由 003 plan 承接。

诚实标记 `partial-complete` 而不是 `complete`，避免后人误判
002 plan 已 closure。"partial-complete" 是 WAC 这类**机制层而非
功能层**项目的标准状态——机制在、硬门未上时是 partial-complete。

## 7. 2026-06-10-003 dogfood 仍 deferred

R14–R15（`2026-06-10-003` 类 multi-step plan 的 8-step E2E
dogfood）在 002 plan 即被标记 deferred。003 plan 没有触及——
它只验证"机制可工作"，不验证"真实 plan 跑通"。**真实 plan
的 E2E 验收**仍归类为后续工作，需要在 `worktree` 里跑一个
step-U1 → step-U8 的实际 plan 并检查 dispatch gap 是否复现。

`docs/report/2026-06-12-ce-executor-isolated-dispatch-gap-diagnosis.md`
的 merry-wren 案例是已知未修的 dispatch gap；该 case 需要单独的
手工 dogfood（不在 003 plan 范围）。
