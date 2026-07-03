---
title: "feat: 引入事件路由表(Event Routing Table)取代隐式选路启发式"
type: feat
status: active
date: 2026-07-02
origin: docs/brainstorms/2026-07-02-event-routing-table-requirements.md
---

# feat: 引入事件路由表(Event Routing Table)取代隐式选路启发式

## Overview

当前 orchestrator 的"下一跳"由「事件订阅关系 + isolated 轮询 + WAC-U5 优先级抢占」三套启发式拼接而成。2026-07-01 e2e 事故(`ce-executor-serial` step-02 跳过 coordinator)证明这套隐式选路已反复出错：一个无关的 `task.resume` 残留事件就能骗过优先级抢占，把 coordinator 挤出下一跳。

本计划引入**从 preset 自动生成、人类可读、漂移可检测**的「事件路由表」，分三阶段落地：

- **里程碑 A(Phase 1，零行为变更)**：建立路由表核心类型、从 preset SSOT 派生边、提供 `ralph route build/show/check`、CI 漂移门。让 preset 拓扑对人类可见，并让"配置在合并中被丢弃"当场暴露为缺边。
- **里程碑 B(Phase 2，仍零行为变更)**：运行时加载路由表；影子模式下与现有 `next_hat` 并行计算并记录分歧；emit 校验 stage 先以 shadow-validate 方式记录越界事件，不动真实拒绝。
- **里程碑 C(Phase 3，行为变更)**：表正式接管确定性单跳选路与 emit 合法性校验，旧 handoff priority 抢占退役，真正并发 fan-out 仍保留轮询回退。

**注意：Phase 1 不治本次事故的 P0 症状**，止血由 `docs/plans/2026-07-02-001-fix-hat-routing-next-hop-plan.md` 负责。本计划是 001 的战略收敛——Fix A(主题精确抢占)止血后，由路由表机制最终取代。

---

## Problem Frame

拓扑其实早已在 preset 里声明清楚：

- `hats.*.triggers` / `publishes` 声明了谁触发谁。
- `mechanism.flow.steps[].allowed_emits` 声明了每一步能发什么。
- `mechanism.flow.terminal_emits` / `terminal_when` / `on_partial` 声明了结束条件与岔路。

但 runtime 没有一张**显式、可判定、单一权威**的"下一跳表"，而是靠以下三处隐式计算：

1. `crates/ralph-proto/src/event_bus.rs` 按订阅把事件丢进多个 hat 的 pending 队列。
2. `crates/ralph-core/src/event_loop/mod.rs` 的 `next_hat` 用 `HandoffIndex` + 轮询游标 + 优先级抢占决定激活谁。
3. `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs` 在 emit 时按 `current_step` 查 `FlowDeclaration::allows()` 做 step-scope 校验。

这三处割裂：订阅只回答"谁能收"，轮询只回答"公平地该谁上"，stage 只回答"这一步允不允许发"。它们对"step 推进到哪了、当前 hat 产出这个事件后下一步该谁"没有统一视图。结果：

- 残留 `task.resume` 让 executor 队列非空 → 抢占谓词误判 → coordinator 被跳过。
- `mechanism.flow` 在配置合并中被静默丢弃 → `FlowStepScopeStage` 空转 → 越界 emit 无人拦截。
- 每次交接没有可解释的审计痕迹，排障靠翻 events 文件考古。

---

## Requirements Trace

需求文档已锁定 13 条需求；本计划全部承接：

- **R1.** 路由表必须从 preset 自动生成，不得手写。 → U1, U2
- **R2.** 产物必须人类可读、可 diff、可 review。 → U1, U3
- **R3.** 必须提供一致性校验：重新生成与 committed 产物比对，不一致即失败。 → U4
- **R4.** 生成应基于生效后的合并配置，让配置被静默丢弃表现为缺边/异常。 → U2, U4
- **R5.** 执行前必须加载路由表。 → U5
- **R6.** 交接时的"下一跳"必须由 runtime 照表确定性判定(对确定性/唯一消费者边)，取代现有启发式。 → U6(验证), U8(执行), U9(退役旧逻辑)
- **R7.** emit 时必须用同一张表校验该终态事件在当前 step 是否合法。 → U7
- **R8.** agent 产出的非法终态事件必须走 backpressure(拒绝+反馈/恢复路径)，不得静默错路。 → U7
- **R9.** 表可向 agent 注入导航信息，但 agent 只决定产出哪个事件，不决定下一跳。 → U5, U5b, U7
- **R10.** 路由表必须支持一对多下一跳(wave/并行 review 维度)及其 join 条件；真正并发的 fan-out 保留轮询回退。 → U1, U2b, U6, U8
- **R11.** 必须支持影子模式：表计算答案但不作数，记录与现有逻辑的分歧。 → U6
- **R12.** 必须能从影子安全翻转为权威(开关/opt-in，可按 preset 放开，可回滚)。 → U8
- **R13.** 每次选路/校验决策必须留痕。 → U5, U6, U7

**验收示例映射：**

- **AE1**(残留 `task.resume` 不误导下一跳)：完全满足点 = U8 authoritative + U6 序列级 BDD 场景(`run_workflow_guard_scenario` 断言事件序列)。
- **AE2**(`mechanism.flow` 被丢表现为缺边/异常)：U2(读原始 Value 避免 lossy config 隐藏字段丢弃) + U4。
- **AE3**(改 preset 未重新生成 → CI 失败)：U4。
- **AE4**(越界 emit 被拒绝)：U7(权威期)；shadow-validate 期先记录不拒绝。
- **AE5**(影子模式记录分歧但不改真实选路)：U6。
- **新增 AE6**(导航注入可见)：U5b。
- **新增 AE7**(fan-out join 判定复用 WaveTracker)：U2b。
- **新增 AE8**(审计条目 schema 可查询)：U6/U7。

---

## Scope Boundaries

- **不手写路由表源文件**；路由表始终是生成产物(R1)。
- **不让 agent/LLM 决定"下一跳是谁"**；agent 只决定产出哪个事件(R9)。
- **不改 preset 的编排语义**；只改"如何依据 preset 选路/校验"。
- **不承诺替换所有轮询**；真正并发的 fan-out(wave 6 维度 review)保留轮询回退(R10)。
- **不治本次事故的 P0 症状**；止血由 `2026-07-02-001-fix-hat-routing-next-hop-plan.md` 负责。本计划是它的战略收敛。
- **不治理恢复类事件(`task.resume`)的生命周期/TTL**；这是独立后续(P3)，需求已 defer。
- **不一次性把 `state_machine.rs` / `flow_lifecycle.rs` / `handoff_index.rs` 全部收敛为单一路由抽象**； Deferred to Follow-Up Work 中说明可分阶段。
- **路由表不建模 PHASE 1/2 的前缀分支语义**：`test.passed` 在 `step-NN` 与 `fix-NN` 阶段语义相反，但表只管"validator 发出 `test.passed` 后下一跳是 coordinator"，分支由 agent 决定 emit 哪个事件(见 I-2 审查发现)。

### Deferred to Follow-Up Work

- 恢复类事件(`task.resume` 等)的 TTL / "激活即消费或过期"语义：后续单独处理(P3)。
- 把 `state_machine.rs`、`flow_lifecycle.rs`、`handoff_index.rs` 在代码层完全收敛为单一路由抽象：可分阶段，不必一次到位。
- 路由表对非 builtin preset(用户自定义 preset / operator `ralph.yml` 重写拓扑)的覆盖：本计划先覆盖 builtin，用户自定义后续评估。

---

## Context & Research

### Relevant Code and Patterns

- **选路总入口** — `crates/ralph-core/src/event_loop/mod.rs` 的 `EventLoop::next_hat`(约 2679–2889)。Isolated 模式下分四层：pending_recovery_hat → targeted-event fast path → handoff priority 抢占 → round-robin。
- **轮询原语** — `crates/ralph-proto/src/event_bus.rs` 的 `select_next_hat_with_pending`(264–325)，只检查队列非空，priority_hat 的 topic-eligibility 由调用方保证。
- **运行时邻接索引** — `crates/ralph-core/src/workflow_contract/handoff_index.rs` 的 `HandoffIndex`，topic→唯一 consumer 的 `BTreeMap`，是"半成品路由表"。
- **静态拓扑来源** — `crates/ralph-core/src/preset_lint/workflow_activation.rs` 的 `HandoffGraph`，从 `hats.*.triggers/publishes` 构造 topic↔hat 双向映射。
- **step-scope 守卫来源** — `crates/ralph-core/src/event_loop/flow_declaration.rs` 的 `FlowDeclaration` / `FlowStepDecl`，提供 `allows(step_id, topic)` 判定。
- **现任 emit 校验 stage** — `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs`，依赖 `ctx.current_step`，有一堆 `DEFENSIVE_BYPASS` / `TRANSITION_TOPICS` 补丁。
- **stage 流水线** — `crates/ralph-core/src/event_loop/stage_pipeline.rs`，`FlowStepScopeStage` 插在 EmitSchemaGate 与 VerdictGate 之间；`StageReject{reason_code}` 是 backpressure 现成通道；首个 `Reject` 即短路。
- **wave / join 现成件** — `crates/ralph-core/src/wave_tracker.rs` 的 `WaveTracker::is_complete` 已实现 N-of-N join。
- **phase 状态机** — `crates/ralph-core/src/flow_lifecycle.rs` 的 `FlowLifecycleRegistry`，有 `current_step_id` 与合法后继表。
- **step 推进双轨问题** — `EventLoop.current_plan_step`(`mod.rs:12053 advance_plan_step`)与 `flow_lifecycle.current_step_id()`(`flow_lifecycle.rs:465`)并行；对 `ce-executor-serial`，前者会被 `test.passed` 提前拉到 `review_walk`，后者因无 `total_units` 而不推进。这正是 `FlowStepScopeStage` 需要 `DEFENSIVE_BYPASS` 的根因。
- **step-close / 校正函数** — `inject_completion_correction`(`mod.rs:2385`)、`drive_step_close_progress`(`mod.rs:10734`)、`drive_step_transition`(`mod.rs:10762`) 是真正驱动 step 推进的函数。
- **配置合并权威入口** — `crates/ralph-cli/src/preflight.rs` 的 `load_config_for_preflight_sync`，返回完全合并后的 `RalphConfig`。
- **config 层丢失字段** — `crates/ralph-core/src/config/loop_config.rs` 的 `FlowStepConfig` 只有 `{id, kind, allowed_emits, terminal_when, on_partial}`，静默丢弃 `total_units` / `over` / `emit_when` / `on_review_passed` / `on_review_failed` / `on_residual` / `body`。
- **CLI 子命令范式** — `crates/ralph-cli/src/commands/preset.rs` 的 `PresetArgs` + `PresetCommands`，`ralph route build/check` 可照此实现。
- **BDD 真 runner** — `crates/ralph-core/tests/scenarios.rs` 的 `run_workflow_guard_scenario`；拓扑类断言必须用真 runner，禁止 `run_scenario` stub(`AGENTS.md` HARD RULE)。
- **产物渲染先例** — `crates/ralph-cli/src/hats.rs` 的 `graph_hats` 支持 `Unicode/Ascii/Compact/Mermaid`。

### Institutional Learnings

- **`docs/solutions/architecture-patterns/orchestrator-expected-event-ledger-ssot.md`**：状态必须来自"账本"(刚落地事件 payload + `flow_lifecycle` + tasks + plan 拓扑缓存)，禁止每次 activation 让 LLM 重读 plan。路由表的边上必须带守卫条件(phase、是否最后 unit 等)，而不是纯 topic→consumer 映射。
- **`docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md`**：preset 层修复必须配 runtime 预算 carve-out，否则事件会被 per-turn budget 静默丢弃；路由表 fan-out 场景要同步确认预算容许。
- **`docs/solutions/developer-experience/wac-rollout-tiered-gates-2026-06-12.md`**：wave 动态注入、多分支 completion、runner-injected/虚拟 publisher 是静态派生的已知盲区，不能一次覆盖所有 preset。
- **`docs/achieved/report/2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md`**：`HANDOFF_TOPIC_SEEDS` 硬编码就是"散落的手工同步边列表"，是路由表要根除的反模式。
- **`docs/achieved/report/2026-06-21-top-3-architectural-instability-factors.md`**：软提示架构是核心脆弱点；路由表把选路从软提示变成 runtime 硬判定。越界 backpressure 要避免"拒绝→`task.resume`→agent 重 emit"回环，优先把恢复指令直接写进下一次 prompt context。
- **`docs/reviews/2026-06-27-mechanism-foundation-adversarial-review.md`**：P0-3 已预警 `mechanism.flow` 解析失败静默回退空配置 + `FlowStepScopeStage` fail-open 是致命隐患——本次 e2e 事故复现。路由表加载必须 fail-closed，禁止静默回退。
- **`AGENTS.md` preset/schema 改动后的下游同步清单 HARD RULE**：路由表作为新派生产物，必须走 CI 强制重新生成对比，不能变成第 8 处手工同步点。

### External References

- 无需外部研究：纯内部编排机制，本仓已有 event_bus / preset_lint / BDD / wave 等充分范式。

---

## Key Technical Decisions

1. **路由表 = `FlowDeclaration` 与 `HandoffGraph` 的 join**：边的起点是 `(step_id, from_hat, topic)`，其中 `step_id` = `mechanism.flow.steps[].id`(如 `unit_loop`/`review_walk`)；终点是 `next_hats`。守卫条件来自 `allowed_emits` / `terminal_when` / `on_partial` / `total_units` 等。
2. **builder 读合并后的原始 YAML `Value`，不走 lossy `RalphConfig`**：`FlowStepConfig` 会静默丢弃 `total_units` / `emit_when` / `on_review_*` 等字段，导致 AE2(字段级丢弃无法暴露)。生成器从 `preflight` 拿合并后的 `serde_yaml::Value`，直接读取 `mechanism.flow` 子树，再构造 `FlowDeclaration`；若某字段在 config 类型化路径丢失，会在 drift-check 与 round-trip 测试中暴露。
3. **一张表两个消费点**：执行前注入 agent 导航；emit 时校验 + 选路。两处查同一张表。
4. **确定性单跳走表，并发 fan-out 走轮询**：唯一 consumer 的边由表直接返回下一跳；多 consumer / 通配订阅 / wave fan-out 交给 `select_next_hat_with_pending` 公平调度。
5. **影子先行**：新增 `EventLoopConfig.routing_table` 开关(`shadow` / `authoritative` / `disabled`)，按 preset opt-in。影子期记录"表答案 vs 现有逻辑答案"分歧到诊断流，真实交接仍由现有逻辑执行(AE5)。
6. **分歧口径三分类**：(1) 表 `RouteTo(X)`，legacy 选 `Y` 且 `X != Y` → **真分歧**；(2) 表 `RouteTo(X)`，legacy 也选 `X` → 一致；(3) 表 `FallBack`，legacy 选 `X` → **"表弃权"**，不计入一致。
7. **fail-closed 加载**：路由表文件缺失、解析失败、或与 preset 漂移时，不静默回退到旧逻辑，而是报错/拒绝启动。运行时**只读** golden，不写 golden。
8. **产物格式默认用 YAML + Mermaid**：YAML 作为 golden 产物(`presets/en/<name>.routes.yml`)用于 drift 校验；Mermaid 作为人类可读产物(`presets/en/<name>.routes.mmd`)用于 review。
9. **影子期 emit 校验只记录不 reject**：由于 `current_step` 双轨不可靠 + `StagePipeline` 首个 reject 即短路，增强期 `RoutingTableStage` 先做 shadow-validate(记录越界但返回 Pass)，待与 `FlowStepScopeStage` 一致后再切为唯一 reject 源、同时退役 `FlowStepScopeStage`。
10. **可观测走既有审计通道**：选路/校验命中、守卫结果、影子分歧都走 `tracing` + `crates/ralph-core/src/event_loop/audit.rs` + `.ralph/diagnostics/`，不另造日志。

---

## Open Questions

### Resolved During Planning

- **产物放在哪？** → `presets/en/<name>.routes.yml`(golden，用于 drift) + `presets/en/<name>.routes.mmd`(人类可读)。与 preset 同目录，CI 重新生成比对。
- **影子模式开关放哪？** → `EventLoopConfig.routing_table: {mode: shadow | authoritative, enabled_presets: [...]}`，默认 `disabled`。
- **如何与现有 `next_hat` 轮询共存？** → 确定性唯一 consumer 边走表；fan-out/多 consumer/通配边走 `select_next_hat_with_pending`。
- **emit 校验替换还是增强 `FlowStepScopeStage`？** → 影子/增强期 `RoutingTableStage` 只记录不 reject；稳定后替代 `FlowStepScopeStage`。
- **builder 如何避免 config 类型静默丢字段？** → 直接读合并后的 `serde_yaml::Value` 的 `mechanism.flow` 子树。
- **分歧怎么才算"一致"？** → 三分类：真分歧、真分歧、表弃权；表弃权不计入一致。

### Deferred to Implementation

- 路由表文件具体 schema 字段命名(实现时再定，本计划只定语义)。
- 影子分歧日志的精确诊断落点(`.ralph/diagnostics/` 下的文件命名与格式)。
- `current_step` 双轨收敛方案：U5 之前需决定是修复 `advance_plan_step`、还是让路由表自带 step 推进逻辑、还是二者并用。
- backpressure 具体策略(拒绝 + `task.resume` vs 直接写入 prompt context)需在 U7 权威期与 prompt 注入机制联调。

---

## Output Structure

> 本树展示新增的主要目录/文件形态，是范围声明而非刚性约束；实现者可根据实际结构调整。

```
crates/ralph-core/src/
├── routing_table/
│   ├── mod.rs                 # 公共类型: EventRoutingTable, RouteEdge, RouteGuard
│   ├── builder.rs             # 从合并后 YAML Value 派生边
│   ├── loader.rs              # 从文件/内存加载路由表，fail-closed
│   ├── selector.rs            # 照表选路(含 FallBack 语义)
│   └── validator.rs           # emit 时合法性校验(权威期)
├── event_loop/
│   ├── mod.rs                 # next_hat consult selector; 注入导航信息
│   └── stages/
│       └── routing_table_stage.rs  # shadow-validate / 权威期 reject stage
└── config/
    └── loop_config.rs         # 新增 routing_table 配置块

crates/ralph-cli/src/
├── commands/
│   └── route.rs               # ralph route build / check / show
└── main.rs / mod.rs           # 注册 Route 子命令

presets/
├── en/
│   ├── ce-executor-serial.routes.yml    # golden 路由表产物
│   └── ce-executor-serial.routes.mmd    # 人类可读 Mermaid
├── schemas/
│   └── ce-executor-serial.yml           # 已存在，无需改 event 拓扑
├── manifest.yml                         # builtin preset 单一事实源(已存在)
└── index.json                           # 用户可见索引，同步 routes 产物存在性

crates/ralph-core/tests/scenarios/
├── 2026-07-02-routing-table-shadow.yml       # 影子模式分歧记录
└── 2026-07-02-routing-table-illegal-emit.yml # 权威期越界拦截
```

---

## High-Level Technical Design

> *以下用于向评审传达方案形状，是指导性说明、非实现规范。实现者应视其为上下文，而非照抄的代码。*

### 数据模型

`step_id` 是 `mechanism.flow.steps[].id`(如 `unit_loop`、`review_walk` 等)，**不是** plan unit 迭代号(`step-01`)。

```yaml
# 产物示例(presets/en/ce-executor-serial.routes.yml 语义示意)
source: builtin:ce-executor-serial
mode: declared
edges:
  - step_id: ~                       # 根边：runner 注入 starting_event
    from: ralph
    topic: work.start
    next_hats: [coordinator]
    guard: {allowed: true}

  - step_id: unit_loop
    from: coordinator
    topic: work.ready
    next_hats: [executor]
    guard: {allowed: true}

  - step_id: unit_loop
    from: executor
    topic: work.done
    next_hats: [validator]
    guard:
      allowed: true
      terminal_when: ~

  - step_id: unit_loop
    from: validator
    topic: test.passed
    next_hats: [coordinator]
    guard:
      allowed: true
      terminal_when: all_done
      on_partial: {any_failed: plan.blocked}

  - step_id: review_walk
    from: review-coordinator
    topic: review.dimension.ready
    next_hats: [goal-alignment, correctness, testing, maintainability, project-standards, adversarial]
    guard:
      fan_out: wave
      join: wave_complete              # 由 WaveTracker::is_complete 判定
```

### 运行时 consult 流程

```
next_hat():
  1. pending_recovery_hat? → 优先
  2. targeted event fast path? → 优先(恢复类事件，不归表)
  3. 若 routing_table 启用且加载成功：
       selector 查 (current_step, from_hat=last_active_hat, last_event_topic):
       - RouteTo(hat)        → 直接返回该 hat(仅 authoritative)
       - FallBackToRoundRobin → 继续走轮询
     shadow 模式下同时算 legacy_hat，记录三分类分歧，但仍返回 legacy_hat
  4. 无表或未命中 → 走 select_next_hat_with_pending 轮询
  5. 写审计日志

emit 时(影子/增强期):
  1. EmitSchemaGate 校验 schema
  2. RoutingTableStage shadow-validate:
     - 查表，记录命中/未命中，但总是 Pass(不破坏现有 FlowStepScopeStage)
  3. FlowStepScopeStage 兜底
  4. VerdictGate

emit 时(权威期):
  1. EmitSchemaGate
  2. RoutingTableStage 查表:
     - 命中且 allowed → Pass
     - 未命中 → Reject{reason: routing_table_undeclared_emit}
  3. VerdictGate(此时 FlowStepScopeStage 已退役)
```

### 生成与漂移检测

```
ralph route build -H builtin:ce-executor-serial:
  1. 调 preflight 拿**合并后的原始 serde_yaml::Value**(不是 lossy RalphConfig)
  2. 从 Value 读取 hats triggers/publishes 构造 HandoffGraph
  3. 从 Value 读取 mechanism.flow 构造 FlowDeclaration
  4. builder 派生 edges(含 fan-out / join 标注)
  5. 写出 presets/en/<name>.routes.yml 与 .routes.mmd

ralph route check -H builtin:ce-executor-serial:
  1. 用与 build 相同的原始 Value 重新生成
  2. 与 committed presets/en/<name>.routes.yml 做结构化反序列化比对
  3. 不一致 → exit non-zero + diff

CI:
  在 scripts/ci-rust-gate.sh 中于 preset_lint 之后挂 ralph route check --all-builtins
```

---

## Implementation Units

### Phase 1: 路由表核心类型、生成器、CLI 产物、漂移门(可独立发布)

- [ ] U1. **定义路由表核心数据模型与边语义**

**Goal:** 建立 `EventRoutingTable` / `RouteEdge` / `RouteGuard` / `NextHops` 等类型，精确表达 `(flow_step_id, from_hat, topic) → next_hats + guard`。

**Requirements:** R1, R2, R10

**Dependencies:** None

**Files:**
- Create: `crates/ralph-core/src/routing_table/mod.rs`
- Test: `crates/ralph-core/src/routing_table/tests.rs`

**Approach:**
- `RouteEdge` 字段：`step_id: Option<String>`(根边如 `starting_event` 可为空)、`from_hat: String`、`topic: String`、`next_hops: NextHops`、 `guard: RouteGuard`。
- `NextHops` 枚举：`Single(HatId)` / `FanOut { hats: Vec<HatId>, join: JoinCondition }` / `Terminal`。
- `RouteGuard` 字段：`allowed: bool`、`terminal_when: Option<String>`、`on_partial: Option<BTreeMap<String,String>>`、`wave_id: Option<String>`。
- 提供 `EventRoutingTable::lookup(step, from_hat, topic) -> Option<&RouteEdge>`。
- 提供序列化/反序列化(YAML)，用于产物落盘与加载。

**Patterns to follow:**
- `crates/ralph-core/src/workflow_contract/handoff_index.rs` 的 `HandoffEntry` 结构风格。
- `crates/ralph-core/src/event_loop/flow_declaration.rs` 的 `FlowStepDecl`/`FlowDeclaration` 结构风格。

**Test scenarios:**
- Happy path：构造一个 executor→validator→coordinator 的 2 条边表，按 `(unit_loop, executor, work.done)` 查到 `Single(validator)`。
- Edge case：`step_id=None` 的根边(starting_event)可正常 lookup。
- Edge case：`FanOut` 边的 `next_hops` 返回 6 个 reviewer 集合。
- Edge case：无匹配时返回 `None`(fail-closed 基础)。
- Error path：反序列化非法产物(如 `next_hops` 既标 `Single` 又标 `FanOut`)报错，不静默回退。

**Verification:**
- 单元测试覆盖所有 lookup / ser / de 路径；非法输入必须报错。

---

- [ ] U2. **实现从 preset 派生路由表的 builder**

**Goal:** 从合并后的原始 YAML `Value` 自动生成 `EventRoutingTable`。

**Requirements:** R1, R4, R10

**Dependencies:** U1

**Files:**
- Create: `crates/ralph-core/src/routing_table/builder.rs`
- Modify: `crates/ralph-core/src/workflow_contract/handoff_index.rs`(如需复用/扩展)
- Test: `crates/ralph-core/src/routing_table/tests/builder.rs`

**Approach:**
- Builder 输入：`serde_yaml::Value`(合并后完整 preset，包含 `hats`、`mechanism.flow`、`workflow_contract` 等)。使用 `preflight` 中返回合并 `Value` 的路径(如 `load_config_value_for_preflight` 或等效函数，避免误取返回 `RalphConfig` 的 `load_config_for_preflight_sync`)。
- 从 `Value` 构造 `HandoffGraph` + `FlowDeclaration`；直接读 `mechanism.flow` 原始子树，避免 `FlowStepConfig` 类型化丢字段。
- 遍历 `FlowDeclaration.steps`：对每个 flow step，遍历其 `allowed_emits`，对每个 topic 查 `HandoffGraph::unique_consumer_of(topic)`：
  - 唯一 consumer → `NextHops::Single`。
  - 多 consumer / 通配订阅 → `NextHops::FanOut`；若 topic 属于 wave review 维度(`review.dimension.*`)，标注 `join = WaveComplete(wave_id)`。
  - 无 consumer 但 topic 在 `terminal_emits` 中 → `NextHops::Terminal`。
  - 无 consumer 且非 terminal → 生成告警边(路由表仍包含，但标记为 `dead_end`)，供 lint 暴露。
- 把 `terminal_when` / `on_partial` / `total_units` / `emit_when` 等直接写入 `RouteGuard`。
- 起点为 `starting_event` 的边单独生成：`from_hat` 为虚拟 `ralph` / runner，`step_id=None`。

**Execution note:** 先写测试：用 `ce-executor-serial` 的合并后 Value 生成表，断言 `unit_loop` `work.done` 的下一跳是 `validator`、`test.passed` 的下一跳是 `coordinator`；断言 `review_walk` fan-out 边为 `FanOut`。

**Patterns to follow:**
- `crates/ralph-core/src/preset_lint/workflow_activation.rs` 的 `HandoffGraph::from_config`。
- `crates/ralph-core/src/event_loop/flow_declaration.rs` 的 `FlowDeclaration::from_yaml`。

**Test scenarios:**
- Happy path(Covers AE1 基础)：`ce-executor-serial` 生成表，`unit_loop` `test.passed` → `Single(coordinator)`。
- Happy path(Covers AE7 fan-out)：`review_walk` 的 `review.dimension.ready` → 6 个 reviewer 的 `FanOut`。
- Edge case：多 consumer topic(无唯一 consumer)不生成 `Single`，避免误判定。
- Edge case(Covers AE2 整块丢失)：`mechanism.flow` 缺失时 builder 报错/产出空表。
- Edge case(Covers AE2 字段级丢失)：给 preset 声明 `on_review_passed` 后再删/改，drift-check 必须失败(由 U4 保证)。
- Integration：builder 使用合并后的原始 `Value`，而不是 lossy `RalphConfig`。

**Verification:**
- builder 单元测试通过；对 `ce-executor-serial` 的生成结果与预期拓扑一致；`mechanism.flow` 缺失时生成失败或明显缺边。

---

- [ ] U3. **CLI `ralph route build/show` 与产物落盘**

**Goal:** 提供生成路由表的 CLI 入口，输出 YAML golden 产物与人类可读 Mermaid。

**Requirements:** R2, R3

**Dependencies:** U1, U2

**Files:**
- Create: `crates/ralph-cli/src/commands/route.rs`
- Modify: `crates/ralph-cli/src/main.rs`(`Commands` 枚举 + 分发臂)
- Modify: `crates/ralph-cli/src/commands/mod.rs`
- Modify: `presets/index.json`(同步 routes 产物存在性)
- Test: `crates/ralph-cli/src/commands/route.rs` 内 `#[cfg(test)]` 或集成测试

**Approach:**
- `RouteArgs { #[command(subcommand)] command: RouteCommands }`。
- `RouteCommands::Build { preset: String, format: Vec<RouteOutputFormat> }`。
- `RouteCommands::Show { preset: String, format: RouteOutputFormat }`。
- `build` 调 preflight 拿合并后原始 `Value` + `routing_table::builder`，写出：
  - `presets/en/<name>.routes.yml`(YAML golden)
  - `presets/en/<name>.routes.mmd`(Mermaid 图)
- `show` 直接打印到 stdout，支持 `yaml` / `mermaid`。
- 复用 `HatsSource` / `ConfigSource` 参数解析范式。
- **runtime 不写 golden**；`build` 仅供作者/CI 调用。

**Patterns to follow:**
- `crates/ralph-cli/src/commands/preset.rs` 的子命令结构与 `execute` 签名。
- `crates/ralph-cli/src/hats.rs:419` 的 `graph_hats` Mermaid 输出。

**Test scenarios:**
- Happy path：`ralph route show -H builtin:ce-executor-serial --format yaml` 输出包含 coordinator→executor→validator→coordinator 边。
- Happy path：`ralph route build -H builtin:ce-executor-serial` 写出 `presets/en/*.routes.yml` 和 `.routes.mmd`。
- Error path：未知 preset 名报错。
- Error path：builtin preset 加载失败(如 `mechanism` 仍被丢)报错。

**Verification:**
- 新增 CLI 命令冒烟测试通过；产物文件可在磁盘上读到。

---

- [ ] U4. **一致性漂移检测门(`ralph route check` + CI)**

**Goal:** 路由表与 preset 永不分叉；重新生成与 committed 产物比对，不一致即失败。

**Requirements:** R3, R4

**Dependencies:** U2, U3

**Files:**
- Modify: `crates/ralph-cli/src/commands/route.rs`(新增 `Check` 子命令)
- Modify: `scripts/ci-rust-gate.sh`(追加 `ralph route check --all-builtins`)
- Modify: `crates/ralph-cli/src/presets.rs`(新增或复用测试：所有 builtin 产物存在且一致)
- Test: `crates/ralph-cli/src/commands/route.rs` 漂移测试

**Approach:**
- `RouteCommands::Check { preset: Option<String>, all_builtins: bool }`。
- 对指定 preset 用与 `build` 相同的原始 Value 重新生成 `EventRoutingTable`，与 `presets/en/<name>.routes.yml` 做结构化反序列化比对(不是文本 diff)。
- 不一致时输出 diff 并 exit non-zero。
- `--all-builtins` 遍历 `PRESETS` 数组中所有 public builtin。
- 在 `scripts/ci-rust-gate.sh` 中，于 `preset_lint` 之后调用 `ralph route check --all-builtins`。

**Execution note:** 先写测试：手动改 `presets/en/ce-executor-serial.routes.yml` 的某条边，断言 `check` 失败。

**Patterns to follow:**
- `crates/ralph-cli/src/presets.rs` 的 `test_ce_executor_root_preset_matches_embedded` byte-equality 校验模式。

**Test scenarios:**
- Happy path(Covers AE3)：未改动 preset 和产物 → `check` 通过。
- Error path(Covers AE3)：改 preset 拓扑但未重新生成 → `check` 失败并给出 diff。
- Error path(Covers AE2 字段级)：改 preset 的 `emit_when`/`on_review_passed` 但未重新生成 → `check` 失败。
- Edge case：只改产物格式但语义一致 → 结构化比对通过(不强求文本一致)。
- Integration：CI gate 调用 `--all-builtins` 且失败时阻塞合并。

**Verification:**
- `ralph route check` 对当前所有 builtin preset 通过；人为制造漂移后失败。

---

### Phase 2: 运行时加载、影子模式、emit 校验 shadow-validate

- [ ] U5. **运行时加载路由表**

**Goal:** EventLoop 在启动时加载路由表；加载失败 fail-closed。

**Requirements:** R5, R13

**Dependencies:** U1

**Files:**
- Create: `crates/ralph-core/src/routing_table/loader.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`(启动时加载表)
- Modify: `crates/ralph-core/src/config/loop_config.rs`(新增 `routing_table` 配置块)
- Test: `crates/ralph-core/src/routing_table/tests/loader.rs`

**Approach:**
- `loader::load_from_config(config: &RalphConfig) -> Result<EventRoutingTable, RoutingTableLoadError>`：
  - 若配置启用 routing_table，先尝试读 `presets/en/<name>.routes.yml`；
  - 文件不存在/解析失败 → `RoutingTableLoadError`；authoritative 模式下启动失败，shadow 模式下可降级为不加载(但仍记录审计)。
- `EventLoop` 增加字段 `routing_table: Option<EventRoutingTable>`。
- 审计日志：加载成功/失败、路径、版本。

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/mod.rs` 中现有 `FlowDeclaration` 加载模式。
- `crates/ralph-core/src/event_loop/audit.rs` 的审计入口。

**Test scenarios:**
- Happy path：配置启用时，从文件加载表成功，`EventLoop` 字段为 `Some`。
- Error path：文件缺失 → `load_from_config` 返回 `Err`。
- Error path：文件 YAML 非法 → `Err`，不静默回退。
- Edge case：shadow 模式下加载失败 → 不阻塞启动，但审计记录 `routing_table_load_failed`。
- Error path：authoritative 模式下加载失败 → 启动失败(fail-closed)。

**Verification:**
- loader 单元测试通过；启动时加载失败不静默降级；authoritative 下 fail-closed。

---

- [ ] U5b. **向 agent prompt 注入路由导航信息**

**Goal:** 在 agent prompt 中注入"当前 step / 合法出口 / 下一跳"导航信息(R9)。

**Requirements:** R9, R13

**Dependencies:** U5

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`(`build_prompt` / `inject_phase_into_prompt` 附近注入导航)
- Test: `crates/ralph-core/src/event_loop/tests/build_prompt.rs`(如存在)或新增测试

**Approach:**
- 在 prompt 组装阶段(靠近 `inject_phase_into_prompt`)，若 `routing_table` 已加载，向 agent 注入一段自然语言导航："当前 step: X；合法终态事件: […]；每个事件将路由到: […]"。
- 导航只陈述事实，不代替 agent 决策。

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/mod.rs` 的 `inject_phase_into_prompt`(`mod.rs:4868`)。

**Test scenarios:**
- Covers AE6. prompt 中可检测到注入的导航文本(匹配子串)。
- Edge case. 表未加载 → prompt 中无导航段，不报错。
- Edge case. fan-out 边的导航应列出多个可能的下一跳。

**Verification:**
- prompt 包含导航信息；表未加载时不破坏既有 prompt。

---

- [ ] U6. **影子模式：表答案 vs 现有逻辑并行计算 + 分歧记录**

**Goal:** 在不影响真实选路的前提下，验证表的判定与现有逻辑一致；分歧必须留痕。

**Requirements:** R6(验证), R11, R13

**Dependencies:** U5

**Files:**
- Create: `crates/ralph-core/src/routing_table/selector.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`(`next_hat` 调用 selector 并比对)
- Create/Modify: `crates/ralph-core/src/diagnostics/routing_table_divergence.rs`(分歧落盘)
- Test: `crates/ralph-core/src/event_loop/tests/routing_table_shadow.rs`

**Approach:**
- `selector::select_next_hat(table, current_step, from_hat, last_event_topic, bus) -> RoutingDecision`：
  - 先查表：若命中唯一 consumer 边 → `Decision::RouteTo(hat)`。
  - 若命中 fan-out / 无匹配 → `Decision::FallBackToRoundRobin`。
- 在 `next_hat` 中：
  - 仍先执行 recovery / targeted fast path(这些不归表)。
  - 然后调用旧逻辑得到 `legacy_hat`。
  - shadow 模式下调用 selector 得到 `table_decision`；**真实返回值仍为 `legacy_hat`**。
  - 记录分歧到 `.ralph/diagnostics/<session>/routing_table_divergence.jsonl`。
- **分歧口径三分类**：
  1. 表 `RouteTo(X)`，legacy 选 `Y` 且 `X != Y` → **真分歧**。
  2. 表 `RouteTo(X)`，legacy 也选 `X` → 一致。
  3. 表 `FallBack`，legacy 选 `X` → **表弃权**，**不计入一致**。
- 日志 schema：`{ts, current_step, from_hat, last_event_topic, legacy_hat, table_decision_kind, table_hat?}`。

**Execution note:** 先写测试：构造一个表判定与旧逻辑会分歧的场景(如残留 `task.resume`)，断言 shadow 模式下返回旧逻辑结果但分歧日志中出现该事件。

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/mod.rs` 现有 `tracing::debug!` 决策日志。
- `crates/ralph-core/src/diagnostics/` 现有诊断文件写入范式。

**Test scenarios:**
- Covers AE5. shadow 模式下表答案与旧逻辑不一致 → 记录真分歧，但实际仍返回旧逻辑结果。
- Happy path. shadow 模式下表答案与旧逻辑一致 → 无真分歧日志。
- Edge case. 表 `FallBack` 而 legacy 选 X → 记录为"表弃权"，不计入一致。
- Edge case. 表未加载或加载失败 → shadow 不触发，旧逻辑正常运行。
- Integration. 分歧日志文件可被读到并解析。

**Verification:**
- shadow 模式单元/集成测试通过；残留事件场景下记录分歧且不改变真实选路。

---

- [ ] U7. **emit-time 路由表 shadow-validate stage**

**Goal:** 在 emit 时用同一张表判定合法性，但影子/增强期只记录不 reject；稳定后切换为权威 reject。

**Requirements:** R7, R8(权威期), R13

**Dependencies:** U1, U5

**Files:**
- Create: `crates/ralph-core/src/event_loop/stages/routing_table_stage.rs`
- Modify: `crates/ralph-core/src/event_loop/stage_pipeline.rs`(注册新 stage)
- Modify: `crates/ralph-core/src/event_loop/stages/mod.rs`
- Test: `crates/ralph-core/src/event_loop/stages/routing_table_stage_tests.rs`

**Approach:**
- 新增 `RoutingTableStage`，实现 `Stage` trait。
- `check(ctx, event)`：
  - 从 `ctx.current_step.id` 和 `event.topic` 查表。
  - 命中且 `guard.allowed == true` → 记录命中边，`StageVerdict::Pass`。
  - 未命中或 `allowed == false` → 记录越界，`StageVerdict::Pass`(shadow-validate 期)；但在配置中标记 `"routing_table_stage_authoritative": true` 时返回 `StageVerdict::Reject(..., reason_code: "routing_table_undeclared_emit")`。
- 将 stage 插入 `stage_pipeline.rs` 的 `EmitSchemaGate` 之后、`FlowStepScopeStage` 之前。
- shadow-validate 期间，用审计/诊断流记录"若表权威会拒绝的事件"，供后续评估与 `FlowStepScopeStage` 的一致性。
- 稳定后，删除 `FlowStepScopeStage`，`RoutingTableStage` 直接 reject。

**Execution note:** 先写 BDD 场景：agent 在 `unit_loop` emit 一个不在 `allowed_emits` 中的 topic，断言 shadow-validate 期间事件仍被处理但诊断流记录 `routing_table_would_reject`。

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs` 的 `check` 实现。
- `crates/ralph-core/src/event_loop/stage_pipeline.rs` 的 stage 注册。

**Test scenarios:**
- Covers AE4(权威期). 越界 emit → `StageReject` reason_code = `routing_table_undeclared_emit`。
- Happy path. 合法 emit → Pass，审计记录命中边。
- Edge case. shadow-validate 期越界 emit → Pass，但诊断流记录 `routing_table_would_reject`。
- Edge case. 当前 step 未知 → 权威期 Reject(不 fail-open)。
- Edge case. 表未加载 → Pass(不阻塞非表启用场景)。

**Verification:**
- 新 stage 单元测试通过；shadow-validate 不破坏既有 `FlowStepScopeStage`。

---

### Phase 3: 翻转为权威 + 旧逻辑退役

- [ ] U8. **配置开关：按 preset 从影子翻转为权威**

**Goal:** 提供安全、可回滚的切换机制；先在确定性 preset(`ce-executor-serial`)上启用 authoritative。

**Requirements:** R12

**Dependencies:** U6

**Files:**
- Modify: `crates/ralph-core/src/config/loop_config.rs`(`RoutingTableConfig` 结构)
- Modify: `crates/ralph-cli/src/preflight.rs`(`PRESET_OPT_IN_WHEN_OPERATOR_OMITS`)
- Modify: `crates/ralph-cli/src/config_resolution.rs`(`PRESET_OPT_IN_KEYS`)
- Modify: `crates/ralph-core/src/event_loop/mod.rs`(`append_runtime_config_block` 注释/签名如需)
- Modify: `presets/en/ce-executor-serial.yml`(若需要显式声明 `event_loop.routing_table.mode: shadow` 或 `authoritative`)
- Test: `crates/ralph-core/src/event_loop/tests/routing_table_authoritative.rs`

**Approach:**
- `RoutingTableConfig { mode: RoutingTableMode, enabled_presets: Vec<String>, emit_authoritative: bool }`。
- `RoutingTableMode` 枚举：`Disabled` / `Shadow` / `Authoritative`。
- `emit_authoritative` 单独控制 U7 是否 reject，与选路解耦。
- 默认全局 `Disabled`，避免影响所有 preset。
- `enabled_presets` 支持按 preset 名开启；builtin 用 `builtin:ce-executor-serial` 形式。
- 当 `mode == Authoritative` 且 preset 在 `enabled_presets` 中时，`next_hat` 直接返回 `table_hat`；`FallBack` 时走轮询。
- 切换回 `Shadow` 或 `Disabled` 即可回滚。
- AGENTS.md HARD RULE 同步：新增 `event_loop.routing_table.*` 字段必须同时改 `loop_config.rs` / `preflight.rs` / `config_resolution.rs` / `append_runtime_config_block`。

**Patterns to follow:**
- `crates/ralph-cli/src/preflight.rs` 的 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` 模式。
- `crates/ralph-core/src/config/loop_config.rs` 中 `HatExecutionMode` 枚举的 serde 处理。

**Test scenarios:**
- Happy path(Covers AE1). `ce-executor-serial` 开启 authoritative → `next_hat` 按表返回 coordinator。
- Edge case. 未在 `enabled_presets` 中的 preset → 即使全局 `Authoritative` 也对其 `Disabled`。
- Edge case. 全局 `Shadow` 时所有启用 preset 只记录分歧。
- Error path. 表加载失败但 authoritative 启用 → 启动失败(fail-closed)。
- Edge case. operator 省略 `routing_table` 配置时，preset 的默认值仍存活(验证 `PRESET_OPT_IN_KEYS` 同步)。

**Verification:**
- 配置解析测试通过；authoritative 对 `ce-executor-serial` 的选路符合预期；operator 省略时 preset opt-in 不失效。

---

- [ ] U9. **旧 handoff priority 抢占逻辑退役**

**Goal:** 当路由表 authoritative 覆盖所有确定性单跳场景后，移除 `next_hat` 中冗余的 handoff priority 抢占代码路径。

**Requirements:** R6

**Dependencies:** U8 且已在生产稳定运行一段时间

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`(删除 isolated 模式下 handoff priority 抢占分支)
- Modify: `crates/ralph-proto/src/event_bus.rs`(可保留 `select_next_hat_with_pending` 作为轮询原语)
- Test: 更新相关测试，确保无旧抢占路径残留

**Approach:**
- 在确认 authoritative 模式无真分歧、表弃权率可解释并稳定后，删除 `next_hat` 中基于 `HandoffIndex` 的 priority 抢占分支。
- 保留 `HandoffIndex` 与 `HandoffTracker` 用于超时/escalation 与 fan-out 轮询调度，不删除这些通用设施。
- 保留 `select_next_hat_with_pending` 作为 fan-out / 未匹配边的回退轮询。

**Execution note:** 本单元不急于在首次合并完成；建议在 U8 上线并观测至少一个发布周期、且真分歧=0 后再执行。

**Test scenarios:**
- Happy path. 删除旧抢占后，纯轮询 + 表选路仍覆盖所有 builtin preset 的 BDD 场景。
- Edge case. 无表启用时 `next_hat` 仍正确走纯轮询。

**Verification:**
- `./scripts/run-tests.sh` 全绿；BDD 场景事件序列无回归。

---

## System-Wide Impact

- **Interaction graph:**
  - `next_hat` 增加对 `routing_table` 的 consult；recovery / targeted fast path 不变。
  - prompt 组装增加导航注入。
  - stage pipeline 新增 `RoutingTableStage`。
  - CLI 新增 `ralph route` 子命令。
  - CI 新增 `ralph route check` gate。
- **Error propagation:**
  - 路由表加载失败 fail-closed(启动失败，authoritative 下)。
  - 越界 emit 权威期通过 `StageReject` 走既有 backpressure 路径。
  - 影子分歧只记录、不改真实行为。
- **State lifecycle risks:**
  - 表产物与 preset 漂移即 CI 失败，防止"第二份手工同步真相"。
  - `current_step` **双轨且不可靠**是本计划最大外部依赖：`EventLoop.current_plan_step` 会被 `test.passed` 提前推进，`flow_lifecycle.current_step_id()` 因 `ce-executor-serial` 无 `total_units` 而不推进。U5/U7 必须与 `inject_completion_correction` / `drive_step_transition` 联合验证，否则 emit 校验会查错行。
- **API surface parity:**
  - 新增 `ralph route build/check/show` 命令。
  - 新增 `EventLoopConfig.routing_table` 配置块。
  - `EventBus::select_next_hat_with_pending` 签名不变。
- **Integration coverage:**
  - 选路 + prompt 注入 + emit 校验的端到端交互必须用 BDD guard 场景覆盖。
  - 影子模式分歧记录需验证真实运行与旧逻辑比对。
- **Unchanged invariants:**
  - 不改 `event_bus::publish` 的订阅路由。
  - 不改 `event_bus` 轮询游标语义。
  - 不改 preset 的 `triggers/publishes/mechanism.flow` 语义。
  - 不改 recovery / targeted event fast path。
  - 不删 `HandoffIndex` / `HandoffTracker`(仅停用其作为 priority 抢占依据)。

---

## Risks & Dependencies

| Risk | 等级 | Mitigation |
|------|------|------------|
| `current_step` 双轨推进不可靠，导致表查错行 | **Blocker** | 在 U5 之前先收敛 `EventLoop.current_plan_step` 与 `flow_lifecycle.current_step_id()`，或让路由表自带 step 推进逻辑；U7 必须联合 `inject_completion_correction` / `drive_step_transition` 验证 |
| 配置层 `FlowStepConfig` 丢字段导致生成器产物不完整 | **Blocker** | U2 直接读合并后原始 `serde_yaml::Value` 的 `mechanism.flow` 子树；U4 加字段级 drift-check；补 round-trip 等价性测试 |
| 路由表生成遗漏 runner-injected / 虚拟 publisher 边，导致误报缺边 | Med | 生成器显式识别 `starting_event` / `task.resume` / `LOOP_COMPLETE` 等 control topic，不把它们当业务 handoff 边；缺边只报 terminal/control 之外的业务 topic |
| 影子模式在生产产生大量分歧日志 | Med | 先只在 `ce-executor-serial` 开启 shadow；分歧聚合/采样；设置每日上限 |
| 旧 handoff priority 路径过早删除导致 fan-out/边缘场景退化 | Med | U9 延迟到 U8 稳定后再执行；保留轮询原语；全面 BDD 回归 |
| 路由表产物成为新的手工同步点 | Med | U4 强制 CI 重新生成对比；产物由 `ralph route build` 生成，不接受手改；runtime 只读不写 |
| 测试入口误用裸 `cargo test -p ralph-cli` | Low | 全程 `cargo nextest run`；ralph-cli 串行、其它包并行；BDD 用 `run_workflow_guard_scenario` |

---

## Phased Delivery

### 里程碑 A(Phase 1)：表核心 + 生成器 + 产物 + 漂移门 —— 可独立发布
- U1 核心类型
- U2 builder
- U3 CLI build/show
- U4 CLI check + CI
- **成功标准：** `ralph route check --all-builtins` 通过；改 preset 后 `check` 失败；`ce-executor-serial` 产物人类可读。
- **行为变更：** 无。runtime 仍走旧逻辑。

### 里程碑 B(Phase 2)：运行时加载 + 影子 + emit 校验 shadow-validate —— 可独立发布
- U5 运行时加载
- U5b 导航注入
- U6 影子模式(只记录分歧，不改真实选路)
- U7 emit 校验 shadow-validate(只记录不 reject)
- **成功标准：** shadow 下运行 `ce-executor-serial` 无真分歧、表弃权率可解释；诊断流可读到分歧/越界记录。
- **行为变更：** 无(默认 Disabled)。开启 shadow 后仍无行为变更。

### 里程碑 C(Phase 3)：权威切换 + 旧逻辑退役 —— 需在前一里程碑稳定后发布
- U8 配置开关 + authoritative 启用
- U9 旧 handoff priority 抢占退役(延迟执行)
- **成功标准：** `ce-executor-serial` authoritative 下 step-02 必回 coordinator；越界 emit 被 `routing_table_undeclared_emit` 拒绝；全量 BDD 无回归。
- **行为变更：** 有。确定性单跳由表接管。

---

## Documentation / Operational Notes

- 新增 `crates/ralph-core/data/ralph-tools-routing.md` skill 指南(因新增 `ralph route` 子命令，触发 `AGENTS.md` skill guide 同步 HARD RULE)。
- 更新 `AGENTS.md` / `CLAUDE.md` 中 preset 改动后的下游同步清单，加入：
  - 改 `event_loop.routing_table.*` 字段需同步 `loop_config.rs` / `preflight.rs` / `config_resolution.rs` / `append_runtime_config_block`。
  - 改 preset event 拓扑后需重新生成 `presets/en/<name>.routes.yml`。
- 更新 `scripts/ralph-zsh-plugin.zsh` 的 `ralph route <TAB>` 补全。
- 影子模式启用时，运维可通过 `.ralph/diagnostics/<session>/routing_table_divergence.jsonl` 监控三类指标：真分歧率、表弃权率、按 `(step,topic)` 聚合的分歧分布。
- 翻转判据(建议)：首发 preset = `ce-executor-serial`；shadow 观测 N 次 e2e 后真分歧 = 0 且无未解释弃权，才允许翻 authoritative。
- 每阶段合并前跑 `./scripts/run-tests.sh`；如遇竞态 flake 用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底。

---

## Sources & References

- **Origin document:** `docs/brainstorms/2026-07-02-event-routing-table-requirements.md`
- **前置战术修复计划:** `docs/plans/2026-07-02-001-fix-hat-routing-next-hop-plan.md`
- **关键代码:**
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-proto/src/event_bus.rs`
  - `crates/ralph-core/src/workflow_contract/handoff_index.rs`
  - `crates/ralph-core/src/preset_lint/workflow_activation.rs`
  - `crates/ralph-core/src/event_loop/flow_declaration.rs`
  - `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs`
  - `crates/ralph-core/src/wave_tracker.rs`
  - `crates/ralph-core/src/flow_lifecycle.rs`
  - `crates/ralph-cli/src/preflight.rs`
  - `crates/ralph-cli/src/config_resolution.rs`
  - `crates/ralph-cli/src/commands/preset.rs`
- **机构知识:**
  - `docs/solutions/architecture-patterns/orchestrator-expected-event-ledger-ssot.md`
  - `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md`
  - `docs/solutions/developer-experience/wac-rollout-tiered-gates-2026-06-12.md`
  - `docs/achieved/report/2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md`
  - `docs/achieved/report/2026-06-21-top-3-architectural-instability-factors.md`
  - `docs/reviews/2026-06-27-mechanism-foundation-adversarial-review.md`
