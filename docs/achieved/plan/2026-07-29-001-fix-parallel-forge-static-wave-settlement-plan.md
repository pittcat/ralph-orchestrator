---
title: Parallel Forge 静态 Wave 结算与失败恢复 - Plan
type: fix
date: 2026-07-29
origin: docs/achieved/brainstorms/2026-07-29-parallel-forge-wave-settlement-and-evidence-gates-requirements.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: legacy-requirements
execution: code
---

# Parallel Forge 静态 Wave 结算与失败恢复 - Plan

## 0. 计划状态

- **状态：READY**
- **代码基线：** `089d835b6cd2a57b20be77ebae99c4f58d3ab9cf`
- **调查范围：** `parallel-forge` preset、schema、artifact templates、声明式 flow、state projection、task DAG、supervisor fan-in、wave dispatcher、precheck、BDD scenario runner、相关 Git 历史和已解决问题文档。
- **已执行的验证：**
  - 读取并比对 `presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml` 和 `presets/templates/parallel-forge/`。
  - 读取 `advance_plan_step`、`recover_current_plan_step`、`EnsureTaskBatch`、supervisor coordinator/bridge/store 的实现与测试。
  - 读取 `parallel_forge_*` BDD fixtures 及其 `run_workflow_guard_scenario` 入口。
  - 检查提交 `97601240`、`78aca63a` 对 task authority 的历史决策。
  - 本轮计划调查未运行测试，符合 `ce-plan` 不执行代码的边界。
- **已知基线异常：** 同一基线的前序调查运行
  `cargo nextest run -p ralph-cli --bin ralph -- presets`
  时，47 项通过、4 项失败；失败均来自
  `ce-executor-pipeline` 的 `dead_end_confidence` /
  `settlement_confidence` schema drift，与本计划目标无关。实施者必须在
  U1 开始时复跑并记录；若失败集合扩大或触及 `parallel-forge`，立即停止。
- **尚未执行：** 本计划列出的 Acceptance Red、targeted nextest、BDD、lint、
  build、E2E 与全量 `./scripts/run-tests.sh`。
- **阻塞项：** 无实施关键决策阻塞。Verifier/Tester 的修改权限、验证失败后的
  恢复责任、静态 wave 算法、task 关闭时点和候选分支晋升规则均已在本计划确定。

## Goal Capsule

- **目标：** 在不改变 `supervisor + wave` 执行模型的前提下，让
  `parallel-forge` 只执行 Planner/Guardian 已冻结的静态 wave，并把
  “Unit 可被下游依赖消费”的唯一时点改为该 wave 已审查、串行集成、增量验证
  并完成结算之后。
- **权威顺序：** 操作者计划与验收条件
  → `execution-plan.yml` 静态 DAG/wave
  → Guardian 审计与 digest
  → supervisor 槽位终态
  → wave review/integration/verification artifacts
  → `forge.wave.settled` 投影。
- **执行模型：** `supervisor + wave` 是用户锁定的产品约束，不允许替换。
- **失败策略：** 验证或集成失败停留在当前逻辑 wave；先复现、记录证据并读取
  上一次匹配失败，再由独立 precheck 决定进入修复或证据补全。只有有界修复耗尽、
  语义冲突或真实外部阻塞才允许进入 `work.failed`。
- **权限：** Reviewer、Verifier、Tester 只读；业务代码和测试修复只由
  Executor 或新增的 wave-fixer 在独立 worktree 中完成；Integrator 只做
  Git 集成、冲突盘点、候选分支晋升和结算。
- **停止条件：** 真实调用链与 Evidence 冲突、静态 schedule 无法机器校验、
  supervisor 槽位终态被 precheck 改写、修复需要越过 Unit 路径边界、或任何
 关键决策置信度降至 0.85 以下。

## 1. 功能目标

### 1.1 业务目标

让复杂 Unit DAG 在 Parallel Forge 中具备真实的代码依赖语义：后续 Unit
不仅“等前置 Executor 做完”，而且从包含全部前置 Unit 已验证提交的基线启动。
失败时系统应继续解决当前 wave，而不是把第一次验证失败直接包装成终局报告。

### 1.2 用户或调用方

- 操作者：通过 `ralph run -H builtin:parallel-forge -p <plan>` 启动计划。
- Planner/Guardian：冻结并审计静态 schedule。
- supervisor：执行一个逻辑 wave 内的全部槽位并产出权威 fan-in。
- 各业务 hat：按当前 wave 的 artifact 和事件完成审查、集成、验证、修复与结算。
- Reporter：只消费最终成功或证据充分的终局失败。

### 1.3 当前行为

1. Planner 已写 `depends_on`、`execution_wave`、`integration_order`，但
   Dispatcher 在每次 activation 重新用 task `open/done` 计算 ready set。
2. `exec.unit.done` 当前投影为 `close_task`，因此 Unit 在 Reviewer、
   Integrator、Verifier 之前就被当作依赖已完成。
3. Worktree hat 在首次执行前为所有 Unit 创建 worktree；依赖层可能仍基于旧
   `base_commit`。
4. Reviewer、Integrator、Verifier 只在全部开发 wave 结束后运行一次。
5. Verifier/Tester 失败直接发 `work.failed`；当前流程没有自动修复闭环。

### 1.4 目标行为与行为差异

- Planner 以确定性拓扑层级生成 `execution_wave`；Guardian 与 runtime
  机器校验同一份 schedule。Dispatcher 只读取 wave N，不再推导 DAG。
- `exec.unit.done` 只表示槽位成功；task 保持 open。只有
  `forge.wave.settled` 批量关闭该 wave 的 Unit tasks。
- 每个 wave 的 worktree 在派发前才从最近一次 verified integration HEAD 创建。
- 每个 wave 先完整 fan-in，再批量 review、按 `integration_order` 串行集成到
  候选分支、执行一次增量验证；验证通过后才晋升正式 integration branch。
- 失败进入同 wave 的独立 correction worktree；修复重新经过 review、
  integration 和 verification。

### 1.5 输入、输出与状态

- **输入：** 操作者 plan；Planner 生成的 `development-plan.md` 和
  `execution-plan.yml`；每个 Unit 的路径边界、验收测试和静态 wave；Git HEAD；
  supervisor fan-in；测试命令结果。
- **输出：** wave worktree map、Unit completion reports、wave review summary、
  candidate integration log、commit map、incremental verification report、
  settlement evidence、correction evidence、最终全量验证和经理报告。
- **状态变化：**
  - `forge.plan.ready` 原子创建全部 open tasks，并校验静态 schedule。
  - `exec.unit.done` 不关闭 task。
  - `forge.wave.settled` 原子关闭本 wave tasks，并携带新
    `verified_base_commit`。
  - 正式 integration branch 只在 verification pass 后 fast-forward。

### 1.6 错误语义

- 无环、缺失依赖、wave 不连续、依赖指向同 wave/后 wave、重复或逆序
  `integration_order`：拒绝 `forge.plan.ready`，零 task 副作用。
- supervisor 槽位失败：保留成功槽位产物，进入当前 wave correction；不直接宣告
  全计划失败。
- Reviewer 拒绝、Integrator 冲突或 Verifier/Tester 失败：产出结构化失败
  observation，经 precheck 后进入 correction。
- 修复预算耗尽、语义冲突、越界修改或外部依赖不可达：只有 confidence ≥90、
  evidence coverage ≥75 且独立 precheck 通过，才发布 `work.failed`。

### 1.7 兼容、性能、安全与权限

- **兼容性：** 不要求保持旧 `parallel-forge` 事件拓扑兼容；其他 presets 未声明
  新 flow 字段时保持当前 transition 行为。
- **性能：** `max_concurrent_workers` 只限制同时运行的 slot 数；不得拆分一个
  逻辑 wave。静态 schedule 校验为 O(V+E)。
- **安全：** Reviewer/Verifier/Tester 无 Git 或源码修改权限；wave-fixer 只能
  修改相关 Unit `allowed_paths` 并不得命中 `forbidden_paths`。
- **幂等：** 重复 plan ready、重复 wave terminal、重启后重复 promotion/
  settlement 必须不重复创建 task、commit 或关闭无关 task。

### 1.8 本次范围

- 静态 wave 生成、机器校验和运行时消费。
- 可重复 development loop 的声明式 flow authority。
- lazy worktree、候选分支、per-wave review/integration/verification/settlement。
- Executor/Reviewer/Integrator/Verifier/Tester 失败的同-wave correction。
- 上一次失败经验、证据模板、precheck、冲突指标和最终失败阈值。
- 复杂 DAG、并发限流、失败恢复、重启幂等的真实 runtime BDD/集成测试。

### 1.9 非目标

- 不改变 supervisor store、slot retry/redrive 的基础设施语义。
- 不把业务失败塞进 supervisor 的基础设施 retry budget。
- 不为所有 presets 建立通用 DAG 调度平台。
- 不新增 Web UI。
- 不允许 Verifier/Tester 自己修业务代码或测试。
- 不实现无法从 Unit 意图唯一判定的语义冲突自动解决；这类冲突必须阻塞。

### 1.10 已知约束、事实与假设

**已确认事实**

- `execution_wave` 已存在于 Unit 模板，但当前 runtime 不消费。
- `EnsureTaskBatch` 已机器校验 task DAG 的缺失依赖、自依赖、重复依赖和环。
- supervisor fan-in 已保证 `exec.wave.complete/failed` 在完整槽位聚合后注入。
- `advance_plan_step` 只支持向前跳转，并以硬编码列表识别非 transition topics。
- precheck 会把普通业务 topic 透明改写为 proposed/gate/final，但不能用于
  supervisor 必须直接观察的 `exec.unit.failed`。

**已确认假设**

- 当前 artifact-first、event-as-control 的项目模式足以承载 wave settlement；
  不需要新增数据库表。
- 正式 integration branch 与候选分支使用 fast-forward 能提供清晰的 crash
  recovery seam。

**待验证假设**

- 无实施阻塞假设。U1 的 baseline 复跑只确认环境与已知四项 schema drift，
  不承担架构决策。

## Product Contract

### Requirements

#### 静态 DAG 与调度权威

- R1. `parallel-forge` 必须继续使用 `supervisor + wave`。
- R2. Planner 必须用 `wave(unit)=1+max(wave(dep))` 生成唯一、连续、从 1
  开始的静态 wave；无依赖 Unit 为 wave 1。
- R3. `integration_order` 必须全局唯一，并满足每条依赖边
  `order(dep) < order(unit)`。
- R4. Guardian 和 runtime 必须拒绝缺失依赖、环、非连续 wave、同 wave
  依赖、逆序 integration order、payload/artifact digest 不一致。
- R5. Dispatcher 只能选择已批准 execution plan 中明确标为当前
  `wave_index` 的 Unit；不得根据 task done 重新求 ready frontier。
- R6. 一个逻辑 wave 的 Unit 数可大于 `max_concurrent_workers`；限流不得改变
  wave identity、fan-in 总数或结算边界。

#### 依赖可消费与 Git 基线

- R7. `exec.unit.done` 只结束 supervisor slot，不关闭 task。
- R8. 当前 wave 的 worktrees 必须在派发前从最近一次
  `verified_base_commit` 创建；不得提前创建未来 wave 的有效 worktree。
- R9. 当前 wave 必须完整 fan-in 后才能进入 Reviewer。
- R10. Reviewer 必须为当前 wave 每个 Unit 产出独立 verdict。
- R11. Integrator 必须按本 wave 的 `integration_order` 将批准提交串行集成到
  `candidate_branch`，不能直接推进正式 integration branch。
- R12. Verifier 必须对 candidate HEAD 运行当前 wave 的增量门禁。
- R13. 只有 Verifier pass 后，Integrator 才能 fast-forward 正式 integration
  branch 并发 `forge.wave.settled`。
- R14. `forge.wave.settled` 必须原子关闭且只关闭本 wave tasks，并携带
  `verified_base_commit`、settled Unit 集合和证据路径。
- R15. Dispatcher 收到 settlement 后只能进入静态 `wave_index + 1`；所有计划
  wave settled 后才发 `forge.exec.development.done`。

#### 失败恢复与证据

- R16. Executor 槽位失败必须保持 `exec.unit.failed` 作为 supervisor 原生终态；
  不得对该 topic 使用 proposed precheck 改写。
- R17. Executor 的 slot failure payload 必须引用落盘证据；wave failure
  handler 在 fan-in 后决定 repair，不得把单槽位失败直接升级为计划失败。
- R18. Reviewer、Integrator、Verifier、Tester 只产生结构化 failure
  observation，不直接发布 `work.failed`。
- R19. 所有 failure observation 必须引用当前 SHA、命令/冲突事实、前一次匹配
  失败及采用/拒绝理由。
- R20. wave-fixer 是修复业务代码和测试的唯一恢复角色；每轮从当前 candidate
  或 verified base 创建独立 correction worktree，并受相关 Unit 路径边界约束。
- R21. correction 必须重新经过 Reviewer、Integrator、Verifier；final Tester
  correction 还必须重新经过 Tester。
- R22. 一个 current wave 最多 3 个 correction rounds；重复同一假设不增加
  confidence。
- R23. 终局 `work.failed` 必须满足 confidence ≥90、evidence coverage ≥75、
  尝试多样性、完整因果链、替代原因排除和独立 precheck。
- R24. Integrator 冲突自动继续的硬门为：冲突文件覆盖 100%、marker 为 0、
  路径合规 100%、Unit 意图追踪 100%、必需测试执行与通过 100%；任一硬门失败
  不能被高分覆盖。

#### 最终门禁与可恢复性

- R25. Tester 只在全部计划 wave settled 后运行全量门禁。
- R26. 重启时必须从 accepted flow authority、task 状态、Git refs 和 artifacts
  恢复当前 wave；不得重新求 DAG 或重复晋升 commit。
- R27. Reporter 只能在全量门禁通过，或终局失败 precheck 通过/耗尽后生成终态
  报告。
- R28. 所有新增 agent-facing 命令、topic、field 和 wave 行为必须同步 AI skill
  guide、preset operator skills、schema、BDD 和 builtin 文档。

### Acceptance Examples

- AE1（R2–R5）：复杂 DAG
  `A,B → C(A),D(A,B),E(B) → F(C,D),G(D,E) → H(F,G)` 固定为 4 个
  wave；runtime 不得产生第五种分组。
- AE2（R7–R15）：A 发 `exec.unit.done` 后 task 仍 open；wave 1
  settlement 后 A/B 同时关闭，C/D/E 才能派发，且 base 包含 A/B 的正式提交。
- AE3（R6）：wave 含 5 Unit、worker cap=2 时，仍只有一个 wave_id、
  expected_total=5，并在全部 5 slots terminal 后 review 一次。
- AE4（R16–R23）：Verifier 首次失败时不出现 `work.failed`；fixer round 1
  修复后重新 review/integrate/verify，成功则 settlement。
- AE5（R22–R24）：三轮不同假设仍失败且证据达标时才允许 `work.failed`；若仍有
  conflict marker，即使 confidence=95 也必须阻塞。
- AE6（R25–R27）：所有计划 wave settled 后 Tester 才运行；Tester 失败进入
  final correction stream，修复后重跑全量门禁。

## 2. 代码库现状与证据

### 2.1 当前实现入口

**外部入口**

`ralph run -H builtin:parallel-forge -p <plan>` 加载 embedded preset；
`crates/ralph-cli/build.rs` 将 preset/schema/templates 嵌入二进制。

**调用链**

```text
Planner forge.plan.ready
  → state_projector EnsureTaskBatch
  → Guardian forge.concurrency.approved
  → Worktree forge.worktrees.ready
  → forge-dispatcher 动态读取 task done/open
  → ralph wave emit exec.unit.ready
  → supervisor fan-in 注入 exec.wave.complete/failed
  → 全部开发结束
  → Reviewer → Integrator → Verifier → Tester → Auditor → Reporter
```

**核心模块**

- `presets/en/parallel-forge.yml`：hat authority、event topology、instructions。
- `presets/schemas/parallel-forge.yml`：event schema 与 state projection SSOT。
- `crates/ralph-core/src/event_loop/mod.rs`：flow step transition/recovery。
- `crates/ralph-core/src/event_loop/flow_declaration.rs` 与
  `crates/ralph-core/src/config/loop_config.rs`：typed flow config。
- `crates/ralph-core/src/state_projector/task.rs`：task DAG 原子投影。
- `crates/ralph-core/src/supervisor/` 与
  `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs`：wave fan-in、
  限流、恢复。

**数据边界**

- 可移植业务状态：`.ralph/forge/<plan-key>/` artifacts。
- runtime task authority：TaskStore，由事件投影写入；hat 只通过 task CLI 观察。
- Git authority：正式 integration branch、per-wave candidate branch、
  per-Unit/correction worktrees。
- 事件只传短字段和 repo-relative artifact path。

**现有测试**

- `crates/ralph-core/src/state_projector/tests.rs`
- `crates/ralph-core/src/event_loop/mod.rs` 内 flow recovery tests
- `crates/ralph-core/src/preset_lint/flow_declaration/tests.rs`
- `crates/ralph-core/tests/scenarios/parallel_forge_*.yml`
- `crates/ralph-core/tests/scenarios.rs`
- `crates/ralph-cli/src/presets.rs`
- `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs`
- `crates/ralph-cli/src/builtin_artifact_templates.rs`

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `presets/templates/parallel-forge/unit.template.yml` | 已有 `depends_on`、`execution_wave`、`integration_order`、路径与 TDD 合同 | 扩展现有 plan SSOT，不另建调度格式 | 高 |
| E2 | `presets/templates/parallel-forge/execution-plan.template.yml` | 明确要求三个字段形成无环 DAG | 静态 schedule 是已有设计，不是新平台 | 高 |
| E3 | `presets/en/parallel-forge.yml::planner/guardian` | Planner 写静态字段，Guardian 审 DAG；没有机器校验 wave/order | U2 扩展 `EnsureTaskBatch` 做 fail-close 校验 | 高 |
| E4 | `presets/en/parallel-forge.yml::forge-dispatcher` | Dispatcher 根据 task open + dependency done 动态算 ready | 必须删除运行时 DAG 重算 | 高 |
| E5 | `presets/schemas/parallel-forge.yml::state_projection` | `exec.unit.done → close_task` | 当前过早释放依赖，U3 改到 settlement 批量关闭 | 高 |
| E6 | `git show 97601240` | 该投影是 2026-07-29 为“slot 完成即 task 终态”新增 | 本计划明确逆转该局部决策并用更强语义测试保护 | 高 |
| E7 | `state_projector/task.rs::project_ensure_task_batch` | 已拒绝空集、count drift、重复 key、自依赖、缺失依赖和环，且写入原子化 | 在同入口增加 wave/order 校验最小且一致 | 高 |
| E8 | `event_loop/mod.rs::advance_plan_step` | 非 transition topic 由全局硬编码；跳转只向前 | 重复 wave 生命周期需显式 step transition contract，不能拼 backward step | 高 |
| E9 | `config/loop_config.rs::FlowStepConfig` | 目前无 step-local transition allowlist | U1 新增向后兼容的 `transition_emits` | 高 |
| E10 | `preset_lint/flow_declaration.rs` | lint 复制了一份非 transition topic 硬编码 | U1 必须让 lint/runtime 共用 typed 语义，消除再次漂移 | 高 |
| E11 | `parallel_forge_exec_wave_branch.yml` | BDD 已用真实 EventLoop + SupervisorCoordinator 验证完整 fan-in | 新场景必须沿用该 runner，不能用 stub | 高 |
| E12 | `supervisor/memory_protocol_tests.rs` | expected_total 可大于并发 cap，pending/in-flight/terminal 总数守恒 | 逻辑 wave 无需按 worker cap 拆分 | 高 |
| E13 | `CONCEPTS.md` 与 `supervisor` 实现 | slot retry 只处理 timeout/empty result/missing terminal 等基础设施失败 | 业务/test 修复必须走独立 correction stream | 高 |
| E14 | `presets/en/parallel-forge.yml::worktree` | 首次创建全部 Unit worktrees，且错误承担 Foundation 实现 | U4 改为按 wave lazy prepare，Foundation 走 Executor | 高 |
| E15 | `parallel-forge.yml::reviewer/integrator/verifier/tester` | 四者均在全部开发完成后运行；Verifier/Tester 文案未明确只读 | U5 改 per-wave barrier；U6 固化权限 | 高 |
| E16 | `ce-executor-pipeline.yml::precheck` | 已有透明 proposed/gate/rejected、3 次拒绝、耗尽转 `plan.blocked` | 复用 precheck，不新建 gate runtime | 高 |
| E17 | `fail-confidence-rubric.template.md` | 终局失败阈值 confidence 90、coverage 75、4 次不同角度 | U6 复用终局标准，correction 触发不机械要求 4 次 | 高 |
| E18 | `settlement-evidence.template.md` | 已有尝试、因果链、假因排除和来源格式 | 新模板引用该通用结构并增加 wave/Git/conflict 字段 | 高 |
| E19 | `crates/ralph-cli/src/builtin_artifact_templates.rs` 与 `build.rs` | Parallel Forge 模板有显式 registry/count parity | 新模板必须同步 build、registry、materialization tests | 高 |
| E20 | `AGENTS.md` HARD RULE | preset/schema 拓扑变更必须同步 runtime、lint、BDD、config、CLI、docs、AI skills 并跑 nextest/full gate | U7 明确下游同步和验证 | 高 |
| E21 | 前序 `cargo nextest ... -- presets` | 当前基线 4 个 ce-pipeline schema drift 失败，未涉及 Parallel Forge | U1 先做 baseline characterization，最终不能误归因 | 中 |
| E22 | 用户会话决定 | `supervisor + wave` 不可改；DAG 应在 plan 处理；验证失败必须继续解决 | KTD1、KTD2、KTD9 具有会话决策依据；只有用户明确锁定的 KTD2 标注 `session-settled` | 高 |

### 2.3 受影响范围

**生产模块**

- typed flow 与 transition recovery。
- state projection 的 batch task validation/close。
- embedded Parallel Forge preset/schema/templates。

**测试模块**

- flow parser/transition/lint 单元测试。
- state projector task batch tests。
- Parallel Forge runtime BDD。
- supervisor cap/fan-in/recovery tests。
- embedded preset/schema/template parity tests。

**配置与 CLI**

- `mechanism.flow.steps[].transition_emits` 新字段。
- Parallel Forge 新/改 event topics 与 required fields。
- artifact materialization registry；不新增 CLI 子命令。

**数据**

- 不迁移 runtime DB。
- 新增 wave/correction artifacts；旧未完成 Parallel Forge loop 不保证 resume 兼容。

**API/UI/外部服务**

- 无公开网络 API、UI 或外部服务变更。

**调用方和构建目标**

- builtin `parallel-forge`。
- `ralph-core`、`ralph-cli`、`ralph-e2e`。
- 注入给 agent 的 `ralph-tools-*.md` 与 loop 外 preset author/review skills。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---:|
| KTD1 | DAG 在哪里分 wave | runtime 每轮重算；Planner 静态冻结；混合校正 | Planner 按最长依赖层级冻结，Guardian/runtime 校验 | E1–E4, E22 | runtime 重算引入第二权威；混合模式仍会漂移 | 0.98 |
| KTD2 | 执行模型 | 串行链；纯 wave；`supervisor + wave` | `supervisor + wave`（session-settled: user-directed — chosen over changing the execution model） | E11–E13, E22 | 用户明确不可改；supervisor 已提供 fan-in/限流 | 1.00 |
| KTD3 | 重复 wave 如何表达 flow | backward transition；复制 N 套 steps；单 development loop + step-local transition allowlist | 新增 `transition_emits`，per-wave topics 留在同 step，仅 development done/终局 topic 前进 | E8–E10 | forward-only authority 不支持 backward；N 未知不能展开 steps；继续硬编码会扩大 drift | 0.94 |
| KTD4 | 何时关闭 Unit task | `exec.unit.done`；integration done；wave settled | `forge.wave.settled` 批量关闭 | E5–E7, E22 | 前两者都不能证明验证通过；settlement 是最早可消费时点 | 0.98 |
| KTD5 | Worktree 创建时点 | 首次全建；依赖 done 后更新；每 wave lazy create | 每 wave 从最新 verified base lazy create | E14, AE2 | 全建看不到依赖；更新旧 worktree 容易残留；lazy create 边界清晰 | 0.96 |
| KTD6 | 集成失败如何防污染正式基线 | 直接集成正式 branch；失败时回滚；candidate branch 验证后 FF | per-wave candidate branch，pass 后 fast-forward | E14–E15, Git 原子引用语义 | 直接写正式 branch 会暴露未验证代码；回滚增加历史与 crash 复杂度 | 0.91 |
| KTD7 | Verifier/Tester 能否修代码 | 自己修；只修测试；完全只读 + wave-fixer | Reviewer/Verifier/Tester 只读，wave-fixer 独立修复 | E15–E18, 权责分离 | 自修自验破坏独立证据；只修测试会诱发迁就实现 | 0.91 |
| KTD8 | Executor failure 是否走普通 precheck | 直接改写 slot topic；slot 保持原生、wave 后业务 precheck | 保持 `exec.unit.failed` 原生；payload 强 schema，本 wave fan-in 后由 failure handler/precheck 决策 | E11, E13, E16 | proposed 改写会让 supervisor 看不到槽位终态 | 0.97 |
| KTD9 | 失败后的动作 | 立即报告；无限重试；有界 correction | 最多 3 个不同假设 correction rounds，成功继续，耗尽才终局失败 | E13, E16–E18, E22 | 立即失败是“摆烂”；无限重试无停止证据 | 0.93 |
| KTD10 | 失败经验如何复用 | 复制上次结论；不看历史；按 fingerprint 分类匹配 | 当前 evidence 必须列上次匹配、基线、采用/拒绝原因 | E17–E18 | 复制会继承旧事实；忽略历史会机械重复 | 0.90 |
| KTD11 | 是否新增持久化表 | supervisor DB；新 wave ledger；现有 event/task/Git/artifact | 不增 DB；accepted event + task projection + Git refs + artifacts 恢复 | E7–E8, E13, E19 | 新 DB 是平台化状态管理；现有权威足够重放 | 0.88 |
| KTD12 | 最终 Tester 失败 | 直接终局；回到某个旧计划 wave；独立 final correction stream | 建立不改变静态计划 wave 编号的 final correction stream | R25–R27, E13, E15–E18 | 直接终局不解决问题；篡改计划 wave 会破坏静态权威 | 0.87 |

所有实施关键决策均 ≥0.85，无需计划前 spike。KTD11/KTD12 的剩余风险由 U6 的
crash-recovery 与 final-correction BDD 直接验证；若 Acceptance Red 暴露现有
replay 无法恢复，则停止并把 KTD11 降级为 BLOCKED，不得临时加数据库。

## Planning Contract

### 高层技术设计

```text
Planner + runtime schedule validation
  │
  ▼
forge.concurrency.approved
  │
  ▼
forge-dispatcher: current_wave = first static wave with open tasks
  │ emits forge.wave.prepare
  ▼
worktree: from verified_base → current-wave unit worktrees
  │ emits forge.wave.worktrees.ready
  ▼
supervisor exec.unit.ready batch (one logical wave_id)
  │ complete fan-in
  ▼
Reviewer (read-only, per-unit verdicts)
  │ pass
  ▼
Integrator: unit commits → candidate branch
  │
  ▼
Verifier (read-only) on candidate HEAD
  │ pass
  ▼
Integrator: candidate FF → official integration branch
  │ emits forge.wave.settled → close current-wave tasks
  ├─ more static waves → next forge.wave.prepare
  └─ all settled → Tester → Auditor → Reporter

任一业务失败 observation
  → artifact + prior-failure match + precheck
  → wave-fixer correction worktree
  → Reviewer → Integrator(candidate) → Verifier
  → pass: settle / final test
  → 3 rounds exhausted or semantic block: guarded work.failed
```

### 核心机制约束

1. `transition_emits` 是通用 flow 机制，只表达“哪些 allowed topics 可推进当前
   step”；未配置时保持旧行为。
2. 静态 schedule 机器校验扩展现有 `EnsureTaskBatch`，不另加 CLI。
3. `unit_tasks[]` 增加 `execution_wave` 和 `integration_order`；projector 在任何
   task 写入前完成全量 O(V+E) 校验。
4. `CloseTaskBatch` 只接受 settlement payload 的 live task IDs，并在同一
   exclusive lock 内校验全存在、全 open、无重复后批量关闭。
5. 静态 current wave 的求法是“execution plan 中最小且仍有 open task 的已声明
   wave”，不是按依赖边求 ready set。
6. correction stream 不创建新的计划 Unit task；它引用 affected task IDs、失败
   fingerprint 和 correction round。
7. slot infrastructure retry 与 business correction 严格分离。

### 事件拓扑合同

以下 topic 名称是本计划的确定实现合同，Executor 不得另行命名：

| Topic | 唯一生产者 | 消费者 | 语义 |
|---|---|---|---|
| `forge.wave.prepare` | forge-dispatcher | worktree | 准备 execution plan 中明确的当前 `wave_index` |
| `forge.wave.worktrees.ready` | worktree | forge-dispatcher | 当前 wave 的 worktree/candidate/base 已核验 |
| `exec.unit.ready` | forge-dispatcher wave batch | executor/supervisor | 保持现有 supervisor slot ready |
| `exec.unit.done` / `exec.unit.failed` | executor | supervisor | 保持现有原生 slot terminal，不做 precheck 改写 |
| `exec.wave.complete` / `exec.wave.failed` | supervisor runtime | reviewer / forge-failure-handler | 权威完整 fan-in |
| `forge.wave.reviewed` | reviewer | integrator | 当前 wave 或 correction 全部批准 |
| `forge.wave.review.failed` | reviewer，经 precheck | forge-failure-handler | 只读审查失败 observation |
| `forge.wave.integrated` | integrator | verifier | 提交已进入 candidate，正式 branch 未晋升 |
| `forge.wave.integration.failed` | integrator，经 precheck | forge-failure-handler | 冲突或集成失败 observation |
| `forge.wave.verified` | verifier | integrator | candidate HEAD 增量门禁通过 |
| `forge.wave.verification.failed` | verifier，经 precheck | forge-failure-handler | 增量验证失败 observation |
| `forge.wave.settled` | integrator | state projector / forge-dispatcher | 正式 branch 已晋升并关闭当前计划 wave tasks |
| `forge.correction.requested` | forge-failure-handler，经 precheck | wave-fixer | 当前计划 wave 或 final scope 的有界修复请求 |
| `forge.correction.done` / `forge.correction.failed` | wave-fixer | reviewer / forge-failure-handler | correction commit 终态 |
| `forge.exec.development.done` | forge-dispatcher | tester | 所有计划 wave settled |
| `forge.full.verified` | tester | auditor | 全量门禁通过 |
| `forge.full.verification.failed` | tester，经 precheck | forge-failure-handler | final correction observation |
| `forge.final.correction.settled` | integrator | tester | final correction 已增量验证并晋升，要求重跑全量门禁 |
| `work.failed` | forge-failure-handler，经 precheck | reporter | correction 耗尽或语义/外部硬阻塞的唯一业务终局失败 |

`development_loop.transition_emits` 只包含
`forge.exec.development.done` 和 `work.failed`。`final_gate.transition_emits`
只包含 `forge.full.verified` 和 `work.failed`；final correction 的全部中间 topic
留在 `final_gate`，不需要 backward transition。

### Precheck 合同

- `forge.wave.review.failed` 拒收回 reviewer。
- `forge.wave.integration.failed` 拒收回 integrator。
- `forge.wave.verification.failed` 拒收回 verifier。
- `forge.full.verification.failed` 拒收回 tester。
- `forge.correction.requested` 与 `work.failed` 拒收回 forge-failure-handler。
- `forge.correction.failed` 拒收回 wave-fixer。
- 所有规则 `retry_budget: 3`，耗尽统一产生
  `plan.blocked{kind: precheck_exhausted}`。
- `exec.unit.failed` 不挂 precheck；其 schema 强制 evidence path/fingerprint，
  独立复核发生在 supervisor fan-in 后的 `forge.correction.requested` gate。

### Event required-fields 合同

schema 可以增加 `field_docs` 和 allowed values，但不得删减下表字段。所有 path
均为 repo-relative；长日志和分析只存 artifact。

| Topic | Required fields |
|---|---|
| `forge.plan.ready` | `plan_path`, `development_plan_path`, `execution_plan_path`, `execution_plan_digest`, `unit_count`, `unit_tasks`, `wave_total`, `plan_key` |
| `forge.concurrency.approved` | `execution_plan_path`, `execution_plan_digest`, `approval_report_path`, `approved`, `approved_base_commit`, `wave_total`, `plan_key` |
| `forge.wave.prepare` | `execution_plan_path`, `execution_plan_digest`, `wave_index`, `wave_total`, `unit_ids`, `verified_base_commit`, `integration_branch`, `plan_key` |
| `forge.wave.worktrees.ready` | `execution_plan_path`, `execution_plan_digest`, `wave_index`, `wave_total`, `unit_ids`, `unit_count`, `worktree_map_path`, `candidate_branch`, `candidate_base_commit`, `integration_branch`, `plan_key` |
| `exec.unit.ready` | 现有字段加 `wave_index`, `wave_total`, `verified_base_commit`, `candidate_branch` |
| `exec.unit.done` | 现有字段加 `wave_index`, `evidence_file`；现有 `content_hash` 是成功产物指纹 |
| `exec.unit.failed` | 现有字段加 `task_id`, `task_key`, `wave_index`, `evidence_file`, `failure_fingerprint`, `attempt_count`, `prior_failure_matches` |
| `forge.wave.reviewed` | `wave_id`, `wave_index`, `wave_total`, `scope`, `correction_round`, `reviewed_unit_ids`, `review_summary_path`, `all_approved`, `candidate_branch`, `candidate_head`, `plan_key` |
| `forge.wave.review.failed` | common failure fields + `reviewed_unit_ids`, `review_summary_path`, `candidate_head` |
| `forge.wave.integrated` | `wave_id`, `wave_index`, `wave_total`, `scope`, `correction_round`, `integrated_unit_ids`, `candidate_branch`, `candidate_head`, `integration_log_path`, `commit_map_path`, `conflict_count`, `plan_key` |
| `forge.wave.integration.failed` | common failure fields + `candidate_branch`, `candidate_head`, `integration_log_path`, `conflict_evidence_path`, `conflict_count` |
| `forge.wave.verified` | `wave_id`, `wave_index`, `wave_total`, `scope`, `correction_round`, `candidate_branch`, `candidate_head`, `verification_report_path`, `passed`, `plan_key` |
| `forge.wave.verification.failed` | common failure fields + `candidate_branch`, `candidate_head`, `verification_report_path`, `failed_commands` |
| `forge.wave.settled` | `wave_id`, `wave_index`, `wave_total`, `settled_task_ids`, `settled_unit_ids`, `integration_branch`, `verified_base_commit`, `candidate_branch`, `settlement_evidence_path`, `review_summary_path`, `integration_log_path`, `verification_report_path`, `plan_key` |
| `forge.correction.requested` | common failure fields + `scope`, `correction_round`, `affected_unit_ids`, `affected_task_ids`, `verified_base_commit`, `candidate_branch`, `candidate_head`, `allowed_paths`, `forbidden_paths` |
| `forge.correction.done` | `scope`, `correction_round`, `affected_unit_ids`, `correction_branch`, `correction_base_commit`, `correction_commit`, `correction_report_path`, `evidence_file`, `failure_fingerprint`, `plan_key` |
| `forge.correction.failed` | common failure fields + `scope`, `correction_round`, `affected_unit_ids`, `correction_report_path` |
| `forge.exec.development.done` | `execution_plan_path`, `execution_plan_digest`, `wave_total`, `settled_wave_count`, `settled_unit_count`, `verified_base_commit`, `integration_branch`, `plan_key` |
| `forge.full.verified` | 现有字段加 `verified_base_commit`, `full_verification_digest` |
| `forge.full.verification.failed` | common failure fields + `verified_base_commit`, `verification_report_path`, `failed_commands`, `full_verification_digest` |
| `forge.final.correction.settled` | `scope`, `correction_round`, `integration_branch`, `verified_base_commit`, `correction_commit`, `settlement_evidence_path`, `plan_key` |
| `work.failed` | common failure fields + `scope`, `correction_round`, `attempt_count`, `dead_end_confidence`, `dead_end_evidence_coverage` |

`common failure fields` 精确表示：
`reason`, `plan_path`, `plan_key`, `wave_id`（final scope 允许固定值
`final-gate`）, `wave_index`, `scope`, `correction_round`, `failure_kind`, `failure_fingerprint`,
`evidence_file`, `prior_failure_matches`, `attempt_count`,
`current_base_commit`。`scope` allowed values 为 `plan_wave` 或
`final_gate`；`correction_round` allowed values 为 0、1、2、3。

首次 wave 的 `approved_base_commit` 由 Guardian 在只读审计时运行
`git rev-parse HEAD` 记录；Worktree 在处理 wave 1 的
`forge.wave.prepare` 时创建/确认正式 integration branch。后续 wave 的
`verified_base_commit` 只能逐字复制前一条 accepted settlement。

### Correction base 与回接点

| 失败来源 | correction base | affected scope | 修复后的回接点 |
|---|---|---|---|
| `exec.wave.failed` | 每个失败 Unit 原 worktree 的最后可复核 commit；若无 commit 则该 Unit 的 `verified_base_commit` | 失败 slots；成功 sibling 保留 | Reviewer 重新审查整个 current wave |
| `forge.wave.review.failed` | 被拒 Unit 的现有 Unit/correction branch HEAD | Reviewer 明确拒绝的 Unit | Reviewer 重新审查整个 current wave |
| `forge.wave.integration.failed` | candidate branch 最后一个无冲突 commit；冲突文件路径边界取相关 Unit `allowed_paths` 并集 | 冲突关联 Unit | Reviewer 审 correction，再由 Integrator 从失败的 `integration_order` 重启 |
| `forge.wave.verification.failed` | 失败 candidate HEAD | failure fingerprint 映射到的 Unit；无法唯一映射时为 current wave 且必须标记 | Reviewer 审 correction → Integrator 合入 candidate → Verifier 重跑 |
| `forge.full.verification.failed` | 正式 integration branch 的最新 verified HEAD | failure fingerprint 映射 Unit；无法唯一映射时为 `final_gate` cross-wave correction | Reviewer → Integrator → Verifier → `forge.final.correction.settled` → Tester 重跑 |

每个 correction 创建新的 branch/worktree，命名必须包含 `plan_key`、`scope`、
`wave_index`、`correction_round` 和稳定 fingerprint 前缀。不得直接在原 Unit
worktree、candidate branch 或正式 integration branch 上修业务代码。Integrator
只把已审查 correction commit 合入 candidate/正式晋升路径。

### 系统性影响

- flow runtime/lint 从 topic 名硬编码转向 step-local declaration。
- task `Closed` 语义从“Executor 完成”升级为“依赖可消费”。
- Parallel Forge 的 Git 正式基线变成验证后晋升。
- 帽子数量增加 `wave-fixer`，但 preset 已是 isolated，满足 4+ hats 规则。
- 新 topic/field 会进入 agent prompt，必须同步 `ralph-tools` 和 operator skills。

## 4. BDD 行为规格

```gherkin
Feature: Parallel Forge 静态依赖 wave 结算与恢复

  Background:
    Given Parallel Forge 运行在 isolated supervisor 模式
    And Planner 已生成 execution-plan.yml
    And 正式 integration branch 指向一个已验证提交

  Scenario S1: 复杂 DAG 被确定性分成四个静态 wave
  Given Unit 依赖为 A,B；C(A),D(A,B),E(B)；F(C,D),G(D,E)；H(F,G)
    When Planner 发布 forge.plan.ready
    Then runtime 接受 wave 1=[A,B], wave 2=[C,D,E], wave 3=[F,G], wave 4=[H]
    And 所有 task 原子创建为 open

  Scenario S2: 非法静态 schedule 零副作用拒绝
    Given execution_wave 存在缺号、同 wave 依赖或依赖后的 integration_order
    When forge.plan.ready 进入 state projection
    Then 事件被拒绝并指出具体 Unit/edge
    And 不创建任何 task

  Scenario S3: 槽位完成不释放下游依赖
    Given A 和 B 属于 wave 1
    When A 与 B 分别发布 exec.unit.done
    Then supervisor 可以完成 slot fan-in
    But A 与 B 的 task 仍为 open
    And wave 2 不得派发

  Scenario S4: worker cap 不拆分逻辑 wave
    Given wave 2 有五个 Unit且 max_concurrent_workers=2
    When Dispatcher 批量发布 exec.unit.ready
    Then 五个 payload 使用同一个 wave_id 且 expected_total=5
    And 最多两个 slot 同时执行
    And Reviewer 只在五个 slot 全部 terminal 后激活一次

  Scenario S5: 当前 wave worktree 看得到全部已结算依赖
    Given wave 1 已 settlement 且 verified_base_commit=V1
    When Dispatcher 准备 wave 2
    Then wave 2 的每个 worktree base 都等于 V1
    And V1 包含 A 与 B 的最终提交
    And 不存在 wave 3 的有效 worktree

  Scenario S6: happy path 以 wave barrier 推进
    Given wave 2 的所有 slot 成功
    When Reviewer 全部批准并且 Integrator 依序写入 candidate
    And Verifier 对 candidate HEAD 通过增量门禁
    Then Integrator fast-forward 正式 integration branch
    And forge.wave.settled 原子关闭 wave 2 tasks
    And Dispatcher 仅进入静态 wave 3

  Scenario S7: Executor 单槽位失败不会立即终结计划
    Given wave 2 中 C 成功而 D 失败
    When supervisor 注入 exec.wave.failed
    Then C 的成功产物被保留
    And failure handler 读取 D 的 evidence 后请求 current-wave correction
    And work.failed 不出现

  Scenario S8: Reviewer 拒绝进入独立修复
    Given 当前 wave fan-in 成功但 Reviewer 拒绝 D
    When failure observation 通过 precheck
    Then wave-fixer 从当前 verified base/candidate 建 correction worktree
    And Reviewer 不修改代码
    And 修复重新经过 review、integration、verification

  Scenario S9: Integrator 冲突受硬门约束
    Given Integrator 遇到两个冲突文件
    When conflict evidence 漏记一个文件或仍有 conflict marker
    Then precheck 拒绝 RESOLVED_CONTINUE
    And confidence=95 也不能覆盖硬门

  Scenario S10: Verifier 失败后修复成功
    Given candidate HEAD 的增量测试失败
    When Verifier 写命令、退出码、SHA、fingerprint 和历史匹配
    And wave-fixer 在 round 1 修复根因
    And correction 通过重新 review/integrate/verify
    Then 正式 branch 才被晋升
    And 当前 wave 正常 settlement

  Scenario S11: 三轮修复耗尽后才允许终局失败
    Given correction rounds 1..3 使用不同根因假设且均失败
    When terminal evidence confidence>=90 且 coverage>=75
    And 独立 precheck 复算通过
    Then 仅出现一条 work.failed
    And Reporter 可生成 FAILED 报告

  Scenario S12: 证据不足不能发终局事件
    Given producer 未引用上一次匹配失败或 coverage<75
    When 它尝试发布 work.failed
    Then precheck 返回稳定 failed_checks 和整改动作
    And 重试预算未耗尽时回到 failure handler

  Scenario S13: 全量 Tester 失败进入 final correction
    Given 所有计划 wave 已 settled
    And Tester 的全量门禁失败
    When final correction 修复后重新通过增量与全量门禁
    Then 不改变 execution-plan.yml 的计划 wave 编号
    And Auditor 只在最终 Tester pass 后启动

  Scenario S14: crash recovery 不重复推进
    Given crash 分别发生在 candidate merge 后、verification pass 后、正式 branch FF 后、settlement 接受后
    When loop resume
    Then current wave 从 event/task/Git/artifact 恢复
    And 不重复 commit、promotion、task close 或下一 wave dispatch
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐层级 | 风险补充测试 | E2E |
|---|---|---|---|---|---|
| S1 | 复杂 DAG 精确得到 4 wave | `state_projector/tests.rs` + new BDD | 单元+集成 | table-driven DAG | 否 |
| S2 | 非法 schedule reject，task 文件不变 | `state_projector/tests.rs` | 单元 | property-style invalid edges | 否 |
| S3 | slot done 后 task open | state projector + workflow BDD | 单元+集成 | characterization | 否 |
| S4 | 5 slots/2 cap 保持一个 wave | `wave_supervisor.rs` | 集成 | concurrency invariant | 否 |
| S5 | worktree base 等于 verified SHA | temp Git repo integration test / external E2E sandbox | 集成 | crash window | 是 |
| S6 | fan-in→review→candidate→verify→settle→next | `run_workflow_guard_scenario` | 集成 BDD | state-machine | 是 |
| S7 | slot fail 不直接 work.failed | supervisor BDD | 集成 | partial-success salvage | 是 |
| S8 | Reviewer read-only，fixer 修复再审 | workflow BDD | 集成 | authority/deny rules | 是 |
| S9 | conflict 硬门不可被分数覆盖 | precheck runtime tests | 单元+集成 | mutation-style hard gate cases | 否 |
| S10 | verifier fail→round1 fix→settle | workflow BDD | 集成 | fault injection | 是 |
| S11 | round3 后 evidence 合格才终局 | precheck BDD | 集成 | boundary 2/3/4 rounds | 否 |
| S12 | 历史/coverage 缺失被拒收 | precheck tests | 单元+集成 | malformed artifact | 否 |
| S13 | Tester fail→final correction→retest | workflow BDD | 集成 | state-machine | 是 |
| S14 | 四个 crash seam 均幂等 | event loop + temp Git tests | 集成 | restart/differential | 是 |

所有测试必须同时断言：

- 目标事件出现的次数；
- 不允许事件缺席；
- task/Git/artifact 副作用；
- 正式 branch 在 verification pass 前不移动；
- 重放后的状态与无 crash 正常路径一致。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1–R6 | 静态 schedule 与 supervisor wave | S1,S2,S4 | PF static schedule BDD | projector DAG tests | wave supervisor cap | mock | E1–E4,E7,E12 |
| R7–R15 | settlement 后才可消费 | S3,S5,S6 | PF barrier BDD | close batch tests | flow/Git integration | mock+sandbox | E5–E15 |
| R16–R18 | 槽位终态和 failure observation | S7,S8 | failed-wave BDD | schema/policy tests | supervisor fan-in | mock | E11,E13,E16 |
| R19–R24 | 证据、历史、修复、冲突硬门 | S8–S12 | correction/precheck BDD | rubric parser/checklist tests | precheck runtime | mock | E16–E18 |
| R25–R27 | 最终测试与恢复 | S13,S14 | final correction/restart BDD | replay tests | temp Git + scenarios | mock+sandbox | E8,E13,E15 |
| R28 | 文档和下游同步 | S1–S14 | strict lint/doc drift | preset parity | all presets/scenarios | full mock | E19,E20 |

## 7. 严格串行开发单元

```text
U1 显式 flow transition 权威
  ↓ 完成全部测试、重构和回归
U2 静态 schedule 机器校验
  ↓
U3 wave settlement task authority
  ↓
U4 静态派发与 lazy worktree
  ↓
U5 per-wave review/integration/verification/promotion
  ↓
U6 evidence-first correction 与终局失败
  ↓
U7 最终门禁、恢复与全链路同步
```

## Implementation Units

### U1. 用 step-local transition contract 支撑重复 wave 生命周期

#### 1. Unit 目标

当 flow step 声明 `transition_emits` 时，只有该列表中的 accepted topic 推进 step；
其余 `allowed_emits` 留在当前 step。未声明的 presets 保持现状。

#### 2. 对应需求与 Scenario

- Requirement：R26、R28
- Scenario：S6、S14 的 flow authority 前置
- Decision：KTD3
- Evidence：E8–E10

#### 3. 外部可观察结果

同一个 `development_loop` 可接受多轮 prepare/slot/review/integration/verify/repair
事件而不误前进；`forge.exec.development.done` 才进入 final test。

#### 4. 当前行为基线

`advance_plan_step` 使用全局 `NON_TRANSITION_TOPICS`，任何未列入硬编码的
allowed topic都会向前跳；lint 复制同一列表。先保留现有测试作为
Characterization，并记录已知 Parallel Forge 14-step 行为。

#### 5. 输入与输出

- 输入：`FlowStepConfig.transition_emits`、current step、accepted topic。
- 输出：`Some(next_step)` 或 `None`。
- 错误：transition topic 不在 allowed emits 时 strict lint error。
- 状态：resident/replay 使用同一函数。
- 不变量：未配置字段时行为不变；只能向前跳。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `crates/ralph-core/src/config/loop_config.rs::FlowStepConfig` | typed YAML | 新增默认空 `transition_emits` | 不改 event loop 总配置 |
| `crates/ralph-core/src/event_loop/flow_declaration.rs::FlowStepDecl/from_config` | runtime flow view | 透传/校验字段 | 不新增 flow version |
| `crates/ralph-core/src/event_loop/mod.rs::advance_plan_step` | resident/replay transition | 非空时以字段为唯一 transition allowlist | 不支持 backward jump |
| `crates/ralph-core/src/preset_lint/flow_declaration.rs` | flow structural lint | 校验 subset/target，移除本规则对 topic 硬编码依赖 | 不重写其它 lint |
| 现有对应 tests | contract tests | 增加 parser/runtime/replay/lint cases | 不做 prompt 文本断言 |

#### 7. 可依赖能力

现有 `allowed_emits`、`on/on_any_of`、forward-only search、
`recover_current_plan_step` 与 strict lint fixture。

#### 8. 禁止依赖的未来能力

不得引用 U2–U7 新 topics 或 Parallel Forge schedule；U1 必须是通用机制。

#### 9. 验收测试

- 名称：`transition_emits_keeps_non_transition_topics_in_current_step`。
- 前置：step allowed `[unit.done,wave.settled,development.done]`，
  transition only `[development.done]`。
- 动作：依次 fold 前两 topic。
- 断言：current step 不变；development done 后精确进入声明 target。
- 副作用：resident 与 replay 结果相同。
- 命令：`cargo nextest run -p ralph-core -- transition_emits`。

#### 10. Acceptance Red

先写上述 test。当前 `unit.done` 不在硬编码表时会错误推进，或 typed config
忽略新字段。有效 Red 必须到达 `advance_plan_step`；YAML 语法错误不算。

#### 11. 单元测试拆分

1. serde 默认空字段保持 legacy。
2. parser 保留显式列表。
3. topic 在 allowed 但不在 transition → None。
4. topic 同时在 transition/forward target → 精确前进。
5. transition 不属于 allowed → lint error。
6. restart fold 与 incremental fold 相同。

不允许 mock `advance_plan_step` 或 replay authority。

#### 12. Red → Green → Refactor 顺序

```text
serde Red → 字段与透传 → Green
→ transition semantics Red → 最小分支 → Green
→ lint Red → subset/target 校验 → Green
→ replay Red → 复用同一 authority → Green
→ 删除可替代的 Parallel Forge topic 硬编码 → 全部 Green
```

#### 13. 最小实现范围

只增加一个默认空字段、runtime 条件分支和 lint；保留 legacy fallback。不得引入
通用状态机、backward edge 或 preset-specific topic 判断。

#### 14. 集成验证

联合 typed config、FlowDeclaration、EventLoop recovery 和 lint。执行：

```bash
cargo nextest run -p ralph-core -- flow_declaration
cargo nextest run -p ralph-core -- current_plan_step
cargo nextest run -p ralph-core -- preset_lint
```

全部通过才进入 U2。

#### 15. 风险驱动测试

- Characterization：现有没有 `transition_emits` 的 flow 结果不变。
- Differential：相同 topic 序列 incremental 与 replay current step 相同。
- Fuzz 不适用：字段是有界 string list，serde/lint 已覆盖。

#### 16. 回归范围

所有 declared-flow presets、flow scope、restart recovery、policy-check step
reconstruction。原因是共享 transition authority 被修改。

#### 17. 预期文件变更

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/config/loop_config.rs` | 修改生产文件 | typed field | E9 |
| `crates/ralph-core/src/event_loop/flow_declaration.rs` | 修改生产文件 | runtime view | E8 |
| `crates/ralph-core/src/event_loop/mod.rs` | 修改生产文件/测试 | transition authority | E8 |
| `crates/ralph-core/src/preset_lint/flow_declaration.rs` 及 tests | 修改生产文件/测试 | lint parity | E10 |

#### 18. 完成标准

Acceptance、单元、集成和回归全绿；`cargo fmt --check`、`cargo clippy`、
`cargo build` 通过；无 skip/弱化断言；Evidence/KTD3 仍成立；可独立提交。

#### 19. 停止条件

若现有 flow 依赖同一 allowed topic 在不同 replay 时产生不同结果，或新字段要求
flow version bump，停止并更新 KTD3；不得用更多全局 topic 硬编码修补。

#### 20. 风险与注意事项

- 风险：legacy positional flow 被无意改写。
- 检测：全量 flow tests 与 embedded preset strict lint。
- 缓解：空字段明确走旧分支。
- 剩余风险：后续 preset 作者漏声明；operator skill 在 U7 增加检查。

### U2. 在 plan handoff 原子校验静态 schedule

#### 1. Unit 目标

只有满足确定性 wave 和 integration order 规则的 `forge.plan.ready` 才能原子创建
tasks；非法 schedule 零副作用拒绝。

#### 2. 对应需求与 Scenario

- R2–R5；S1、S2；KTD1；E1–E4、E7。

#### 3. 外部可观察结果

Planner 不能发一个“DAG 对但 wave 标错”的计划；Guardian 收到的 plan ready
已经通过 runtime 结构校验。

#### 4. 当前行为基线

`EnsureTaskBatch` 只校验 dependency graph，不读取 `execution_wave` 或
`integration_order`。先增加测试证明错误 schedule 当前仍会创建 tasks。

#### 5. 输入与输出

- `unit_tasks[]` 新增正整数 `execution_wave`、`integration_order`。
- plan ready 新增 `execution_plan_digest`。
- 输出仍是原子 task batch。
- 拒绝信息包含 item key、edge 和违反的规则。
- 不变量：失败时磁盘与 cache 都不变。

#### 6. 修改位置

- `StateProjectionAction::EnsureTaskBatch`：增加可选 pointer，保持其它 presets
  兼容。
- `project_ensure_task_batch`：在锁和写入前完成 schedule 校验。
- `parallel-forge` schema/planner/guardian/templates：必填静态字段与 digest。
- `presets.rs`：只做结构化 schema/projection 断言。

#### 7. 可依赖能力

现有 cycle/missing dependency validation、exclusive lock、event schema element
constraints、SHA/digest shell能力。

#### 8. 禁止依赖的未来能力

不得关闭 tasks、创建 worktrees 或派发 wave；不得实现 U3/U4。

#### 9. 验收测试

table-driven cases：AE1 正常复杂 DAG；wave 缺号；同 wave edge；后 wave dep；
重复 order；order 逆依赖；字段缺失。每个失败 case 断言 tasks file 不存在或
内容字节不变。命令：
`cargo nextest run -p ralph-core -- ensure_task_batch`。

#### 10. Acceptance Red

先提交“同 wave 依赖仍被接受”的测试；当前应 Green（暴露缺口），将断言改为
reject 后形成真实 Red。失败必须来自缺少 schedule validation，不得来自
fixture count mismatch。

#### 11. 单元测试拆分

1. longest-path 合法 schedule accepted。
2. wave 从 0/2 开始 rejected。
3. edge 必须从较小 wave 指向较大 wave。
4. wave index 必须连续。
5. integration order 唯一且尊重 edge。
6. 重复相同 batch 幂等。
7. existing task metadata/schedule drift fail-close。

#### 12. Red → Green → Refactor 顺序

```text
字段解析 Red → optional pointers → Green
→ wave invariants Red → O(V+E) 校验 → Green
→ order invariants Red → 映射校验 → Green
→ atomicity Red → 全校验前置于 lock mutation → Green
→ preset/schema/template Red → 同步 contracts → Green
```

#### 13. 最小实现范围

扩展已有 action，不新增命令或存储。Planner 生成公式和 Guardian digest check
写入 instructions/templates；runtime 只校验 payload，不打开业务 artifact。

#### 14. 集成验证

```bash
cargo nextest run -p ralph-core -- state_projector
cargo nextest run -p ralph-cli --bin ralph -- presets
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
```

#### 15. 风险驱动测试

- Property-style：对一组小 DAG 生成合法 rank，再随机破坏一个字段，必须 reject。
- Idempotency：重复相同 plan ready 不增加 task。
- 不引入 property-test 新依赖；用确定性 table cases。

#### 16. 回归范围

所有使用 `ensure_task_batch` 的 presets；旧 action 未提供新 pointers 时保持旧
校验。Parallel Forge strict schema parity 必须通过。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/config/state_projection.rs` | 修改生产 | optional pointers | E7 |
| `crates/ralph-core/src/state_projector/task.rs` | 修改生产 | schedule validation | E7 |
| `crates/ralph-core/src/state_projector/tests.rs` | 新增测试 | S1/S2 | E7 |
| `presets/en/parallel-forge.yml` | 修改配置 | Planner/Guardian contract | E3 |
| `presets/schemas/parallel-forge.yml` | 修改 schema | fields/projection | E3 |
| `presets/templates/parallel-forge/{unit,execution-plan}.template.yml` | 修改模板 | static SSOT | E1,E2 |
| `crates/ralph-cli/src/presets.rs` | 修改测试 | structured parity | E19 |

#### 18. 完成标准

S1/S2 全部断言、targeted tests、lint/build 通过；非法输入零副作用；无新依赖；
可独立提交。

#### 19. 停止条件

若 `EnsureTaskBatch` 不是所有 Parallel Forge plan ready 的真实入口，或 schedule
字段无法在事件大小限制内传递，停止并重新决定 validation seam。

#### 20. 风险与注意事项

主要风险是 payload/artifact 双份数据漂移。以 digest + Guardian reopen 为 gate；
runtime payload 负责机器结构，artifact 负责完整 Unit 合同。

### U3. 将 task 终态移动到 wave settlement

#### 1. Unit 目标

`exec.unit.done` 不关闭 task；`forge.wave.settled` 在一个 exclusive lock 内只关闭
当前 wave 的全部 tasks。

#### 2. 对应需求与 Scenario

- R7、R14；S3；KTD4；E5–E7。

#### 3. 外部可观察结果

slot fan-in 后 tasks 仍 open；settlement accepted 后该 wave 全部 closed，兄弟/
未来 wave tasks 保持 open。

#### 4. 当前行为基线

提交 `97601240` 建立 `exec.unit.done → close_task`。现有
`accepted_exec_unit_done_closes_exact_task` 固定了错误产品语义，需要先改为
Characterization 注释，再以新 Acceptance Red 取代，不能简单删除覆盖。

#### 5. 输入与输出

- settlement payload：`wave_index`、`wave_id`、`settled_task_ids`、
  `settled_unit_ids`、`verified_base_commit`、evidence paths。
- 原子输出：所有 target open→closed。
- 错误：空集、重复 ID、未知 ID、数量/identity 不一致均拒绝；全部 target 已关闭
  视为同一 settlement 的幂等 no-op；open/closed 混合集合视为不一致并拒绝。
- 副作用：任一错误时零关闭。

#### 6. 修改位置

新增 `CloseTaskBatch` action、task projector implementation/tests；schema 将
projection 从 `exec.unit.done` 移到 `forge.wave.settled`。

#### 7. 可依赖能力

TaskStore exclusive lock、single close semantics、U2 创建的 live tasks。

#### 8. 禁止依赖的未来能力

不创建 settlement event producer、不改 Git；U3 只提供 state authority。

#### 9. 验收测试

先 seed wave1/2 tasks；应用两个 exec done，断言全 open；应用 wave1 settlement，
断言 wave1 closed、wave2 open；重复完全相同 settlement 为幂等 no-op；混合
open/closed batch 稳定拒绝。命令：
`cargo nextest run -p ralph-core -- close_task_batch`。

#### 10. Acceptance Red

当前第一个 exec done 会关闭 task，故 S3 首断言失败，这是有效 Red。未知 task
导致 fixture 失败不算。

#### 11. 单元测试拆分

1. exec done inert to tasks。
2. settlement close exact batch。
3. future wave untouched。
4. duplicate IDs fail-close。
5. one unknown ID causes zero close。
6. replay duplicate不产生额外状态。

#### 12. Red → Green → Refactor 顺序

```text
exec done open Red → 移除旧 projection → Green
→ exact batch Red → action+projector → Green
→ atomic failure Red → prevalidate all IDs → Green
→ replay Red → 对齐 event idempotency → Green
→ preset structural tests Green
```

#### 13. 最小实现范围

只新增 batch close action和改 projection；不改变 TaskStatus 枚举或 task CLI。

#### 14. 集成验证

运行 state projector 全组、Parallel Forge task dispatch BDD 和 preset tests。

#### 15. 风险驱动测试

State-machine：open→closed 唯一合法方向；Atomicity：混合 valid/invalid IDs；
Concurrency：exclusive lock 下两个不同 batch 不丢关闭。

#### 16. 回归范围

TaskStore、progress gate、所有 state projection actions、parallel forge task dispatch。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `config/state_projection.rs` | 修改生产 | action variant | E5–E7 |
| `state_projector/task.rs` / `mod.rs` | 修改生产 | batch dispatch | E7 |
| `state_projector/tests.rs` | 修改/新增测试 | new terminal semantics | E6 |
| `presets/schemas/parallel-forge.yml` | 修改配置 | projection owner | E5 |
| `crates/ralph-cli/src/presets.rs` | 修改测试 | schema/action parity | E19 |

#### 18. 完成标准

S3 全绿、atomicity proven、旧 close-exact 测试以新语义重写而非删断言、build/lint
通过、可独立提交。

#### 19. 停止条件

若 supervisor fan-in 或 progress gate硬要求 slot terminal 同时关闭 task，停止并
更新影响分析；不得伪造第二套 settled task ledger。

#### 20. 风险与注意事项

任务会在较长时间保持 open，这是目标行为。监控/提示文案必须避免把 open
解释为 Executor 尚未返回。

### U4. 按静态 wave 从 verified base 惰性准备并派发

#### 1. Unit 目标

Dispatcher 只选择 execution plan 中当前最小 open `execution_wave`，Worktree
只为该 wave 从 `verified_base_commit` 创建 worktrees，并一次 batch emit。

#### 2. 对应需求与 Scenario

- R5、R6、R8；S4、S5；KTD1、KTD5；E4、E12、E14。

#### 3. 外部可观察结果

复杂 DAG 每次严格使用 plan wave；未来 wave 不提前建 worktree；cap=2 的 5-unit
wave仍注册 expected_total=5。

#### 4. 当前行为基线

当前 Dispatcher 动态算 ready，Worktree 首次全建且承担 Foundation。现有
`parallel_forge_task_dispatch_runtime` 先固定旧事件顺序，再改写为 static
wave contract。

#### 5. 输入与输出

- 输入：approved plan、execution plan、task list、verified base（首次为
  approved base，后续来自 settlement）。
- 输出：`forge.wave.prepare`、`forge.wave.worktrees.ready`、单 batch
  `exec.unit.ready`。
- 错误：计划 wave tasks 部分 closed/identity drift、base SHA 不存在、已有
  worktree base 不一致 → block/correction，不得派发。

#### 6. 修改位置

Parallel Forge flow/hats/schema；worktree-map template 新增 wave/base/candidate
字段；BDD 和 supervisor integration tests。Runtime supervisor 不改算法。

#### 7. 可依赖能力

U1 development loop、U2 static fields、U3 open tasks、`ralph wave verify/emit`、
supervisor cap。

#### 8. 禁止依赖的未来能力

不 review/integrate/verify/settle；不提前写 U5 events。

#### 9. 验收测试

- S4：5 payload one wave_id、cap2、fan-in after 5。
- S5：临时 Git repo 中 wave2 worktree HEAD=V1，wave3 path absent。
- Dispatcher 输入中依赖 edge 即使可推导其他 frontier，也必须按 declared
  wave index。

#### 10. Acceptance Red

旧 Worktree 会提前创建 future wave，旧 Dispatcher 用 task done 得出 ready；
新 fixture 断言不存在 future map entries/current wave exact list，应失败。

#### 11. 单元测试拆分

1. first open static wave selection。
2. partially closed same wave fail-close。
3. no open tasks → development done（先只返回决策，U7 接 final route）。
4. batch identity stable and sorted by integration_order/unit_id。
5. cap 不影响 expected_total。
6. base SHA mismatch stops emit。

#### 12. Red → Green → Refactor 顺序

```text
static selection Red → 删除 dependency ready 重算 → Green
→ lazy map Red → worktree current-wave only → Green
→ base assertion Red → SHA 写入/复核 → Green
→ cap Red → single batch expected_total=N → Green
→ BDD real fan-in Green
```

#### 13. 最小实现范围

主要是 preset orchestration 与 schema backpressure；不在 dispatcher runtime
自动合并多个 wave，不新增 Git service。

#### 14. 集成验证

```bash
cargo nextest run -p ralph-cli -- wave_supervisor
cargo nextest run -p ralph-core --test scenarios -- parallel_forge
cargo nextest run -p ralph-cli --bin ralph -- presets
```

#### 15. 风险驱动测试

Concurrency：expected_total 守恒；Idempotency：重复 prepare 不重复 worktree；
Fault injection：`git worktree add` 后 artifact 写失败，重入复用精确 branch/path。

#### 16. 回归范围

wave verify/emit、supervisor worktree bind、fan-in、duplicate handoff、task dispatch
BDD。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改配置 | hats/flow/dispatcher/worktree | E4,E14 |
| `presets/schemas/parallel-forge.yml` | 修改 schema | prepare/worktrees payload | E19 |
| `presets/templates/parallel-forge/execution-plan.template.yml` | 修改模板 | verified base/wave fields | E2 |
| `crates/ralph-core/tests/scenarios/parallel_forge_*.yml` | 修改/新增 BDD | S4/S5 | E11 |
| `crates/ralph-core/tests/scenarios.rs` | 注册测试 | real runner | E11 |
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | 新增集成测试 | cap/fan-in | E12 |

#### 18. 完成标准

当前 wave exact dispatch、lazy base、single batch/cap、fan-in 全绿；无 prompt
文本测试；strict lint/build 通过；可独立提交。

#### 19. 停止条件

若 agent-only Worktree 无法通过 event/schema/BDD 提供可验证 base evidence，
停止评估是否需要现有 Git helper 的小型复用；不得只加强自然语言。

#### 20. 风险与注意事项

worktree 创建是外部 Git 副作用，crash seam 必须通过“分支/path 已存在且 SHA
匹配则复用，否则拒绝”实现，不得模糊匹配。

### U5. 完成 per-wave review、candidate integration、verification 与 settlement

#### 1. Unit 目标

当前 wave 完整 fan-in 后，严格经过 Reviewer→candidate Integrator→Verifier→
Integrator promotion/settlement，才解锁下一 static wave。

#### 2. 对应需求与 Scenario

- R9–R15；S3、S6；KTD4、KTD6；E11、E14、E15。

#### 3. 外部可观察结果

每个计划 wave 都有独立 review/integration/verification/settlement artifacts；
正式 branch 在 Verifier pass 前不移动。

#### 4. 当前行为基线

当前流程在全部开发完成后才一次性 review/integrate/verify。现有 14-step BDD
固定该行为，需改为一个可重复 development loop，测试多个 settlement。

#### 5. 输入与输出

- fan-in 输入必须含 wave_id/current units。
- review 输出 per-unit verdict + aggregate。
- integration 输出 candidate branch/head、commit map、冲突计数。
- verification 输出 command evidence/pass。
- settlement 输出正式 verified HEAD 和 task batch。
- 不变量：失败 observation 不推进下一个 wave。

#### 6. 修改位置

Parallel Forge reviewer/integrator/verifier hats、event schemas、flow
`development_loop.transition_emits`、new settlement template/registry、BDD。

#### 7. 可依赖能力

U1 flow、U3 batch close、U4 current-wave map/fan-in。

#### 8. 禁止依赖的未来能力

happy path不依赖 U6 fixer；失败 observation 可以暂时停在明确非终态 topic，
不得在 U5 顺手实现 correction。

#### 9. 验收测试

两 wave happy path BDD：
wave1 two slots → one review → ordered candidate commits → verify → promote →
settle/close → wave2 worktrees base=wave1 verified HEAD。断言正式 branch 在 verify
之前等于旧 SHA。

#### 10. Acceptance Red

旧 flow 的 `exec.wave.complete` 会进入 `exec_finalize` 并最终只 review 一次；
预期两个 `forge.wave.settled` 的 BDD 当前缺失而失败。

#### 11. 单元测试拆分

1. fan-in payload identity matches execution plan current wave。
2. reviewer summary covers every slot exactly once。
3. integration order subset exact and unique。
4. candidate starts at verified base。
5. verify evidence anchored to candidate HEAD。
6. promotion requires pass + unchanged candidate HEAD。
7. settlement task/unit identities exact。

#### 12. Red → Green → Refactor 顺序

```text
flow loop Red → development_loop transition contract → Green
→ per-wave review Red → current-wave summary → Green
→ candidate Red → isolated branch integration → Green
→ verifier SHA gate Red → evidence contract → Green
→ promotion/settlement Red → FF + batch close event → Green
→ two-wave BDD Green
```

#### 13. 最小实现范围

只实现 success spine 与明确 failure observations；不自动修复。Integrator 不修改
Unit 业务意图，只整合已批准提交与晋升。

#### 14. 集成验证

真实 EventLoop BDD + temp Git branch test + state projector。执行：

```bash
cargo nextest run -p ralph-core --test scenarios -- parallel_forge
cargo nextest run -p ralph-core -- state_projector
cargo nextest run -p ralph-cli --bin ralph -- presets
```

#### 15. 风险驱动测试

State-machine：任何缺 review/integration/verification 的 settlement reject；
Fault injection：candidate merge 后 crash、verify 后 crash、FF 后 emit 前 crash；
Differential：resume 与 uninterrupted final refs/tasks 相同。

#### 16. 回归范围

flow scope、state projection、completion path、Git cleanliness、artifact
materialization、existing Parallel Forge success/failure BDD。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改配置 | per-wave hats/flow | E15 |
| `presets/schemas/parallel-forge.yml` | 修改 schema | new handoffs/fields | E19 |
| `presets/templates/parallel-forge/wave-settlement.template.md` | 新增模板 | evidence SSOT | E18,E19 |
| `crates/ralph-cli/build.rs` / `builtin_artifact_templates.rs` | 修改生产/测试 | embed new template | E19 |
| Parallel Forge BDD files/`scenarios.rs` | 修改/新增测试 | S6 | E11 |

#### 18. 完成标准

两 wave success spine、branch non-movement、promotion/settlement atomicity、
artifact registry parity、lint/build 全绿；可独立提交。

#### 19. 停止条件

若正式 branch 无法保证 fast-forward 或现有 worktree model对 candidate branch
有隐式 merge，停止并重查 Git authority；不得用 reset/force push。

#### 20. 风险与注意事项

promotion 后 settlement emit 前 crash 是最大窗口。重入必须先比较正式 ref、
candidate ref 和 settlement event；相等视为 promotion 已完成，仅补 event。

### U6. 以证据和历史经验驱动同-wave correction

#### 1. Unit 目标

Executor/Reviewer/Integrator/Verifier/Tester 失败不直接终结；failure handler 经
precheck 后派发 wave-fixer，最多三轮不同假设，成功回到审查链，耗尽才 guarded
`work.failed`。

#### 2. 对应需求与 Scenario

- R16–R24；S7–S13；KTD7–KTD10、KTD12；E13、E16–E18。

#### 3. 外部可观察结果

第一次验证失败后看到 correction requested/fixer/review/reverify，而不是 FAILED
报告；终局失败一定能追溯当前及上次证据。

#### 4. 当前行为基线

当前 failure handler 把 `exec.wave.failed` 直接转 `work.failed`；Reviewer block、
Integrator/Verifier/Tester 失败也直接进入报告。先用现有 failed BDD 固定该路径，
再写“work.failed 必须 absent”的 Acceptance Red。

#### 5. 输入与输出

- failure observation：source hat、wave/final scope、SHA、fingerprint、
  evidence path、prior match。
- correction request：affected Unit/task IDs、round、base/candidate、
  allowed/forbidden paths。
- fixer done：commit/report/evidence。
- terminal failure：confidence、coverage、attempt ledger、hard checks。
- 错误：missing/unreadable evidence、重复假设、路径越界、冲突硬门失败 →
  `.rejected`。

#### 6. 修改位置

Parallel Forge precheck、failure handler、new wave-fixer hat、all failure-producing
hat contracts、schemas/topic deny rules、failure/conflict templates/registry、
precheck and BDD tests。

#### 7. 可依赖能力

现有 precheck runtime、U4 worktree isolation、U5 review/integration/verification
loop、ce pipeline rubric/templates。

#### 8. 禁止依赖的未来能力

不得改 supervisor infra retry/redrive；不得让 verifier/tester写代码；不得自动
解决语义不确定冲突。

#### 9. 验收测试

- S7 partial slot failure：成功 slot retained，correction only failed Unit。
- S9 conflict hard gates table。
- S10 verifier round1 recovery。
- S11 round 0/1/2 不允许 terminal，round3 + 90/75 + pass 才允许。
- S12 prior history/coverage missing rejected。
- S13 final tester correction 后重跑 full。

#### 10. Acceptance Red

在 existing failure BDD 中断言 `work.failed` absent、correction requested present；
当前会直接出现 work.failed，形成正确 Red。

#### 11. 单元测试拆分

1. `exec.unit.failed` 保持 supervisor 终态且 required evidence fields。
2. wave failure handler 区分 infra retry 与 business correction。
3. fingerprint 由 failure kind/command/test/conflict files/base SHA 构成。
4. prior evidence 分类：reusable/partial/not_applicable。
5. round 单调且最多 3。
6. duplicate hypothesis 不计新 round confidence。
7. conflict inventory/markers/path/trace/test 硬门。
8. terminal confidence/coverage boundary 89/90、74/75。
9. precheck exhausted 进入 `plan.blocked(kind=precheck_exhausted)`。

#### 12. Red → Green → Refactor 顺序

```text
slot failure Red → evidence fields但保持原生 terminal → Green
→ failure observation Red → central handler/precheck → Green
→ fixer round1 Red → correction worktree+done → Green
→ re-review/reverify Red → 回接 U5 chain → Green
→ conflict hard gates Red → template/checklist → Green
→ round3 terminal Red → 90/75 precheck → Green
→ final Tester correction Red → final stream/retest → Green
```

#### 13. 最小实现范围

新增一个 wave-fixer 功能 hat和两个/三个 templates；复用 precheck runtime。
correction request 的最低证据用于开始修复，不机械要求先失败四次；只有终局 fail
采用 ce rubric 的四次尝试阈值。

#### 14. 集成验证

```bash
cargo nextest run -p ralph-core -- precheck
cargo nextest run -p ralph-core --test scenarios -- parallel_forge
cargo nextest run -p ralph-cli --bin ralph -- presets
cargo nextest run -p ralph-cli -- builtin_artifact_templates
```

#### 15. 风险驱动测试

Fault injection：test timeout、invalid output、conflict、artifact missing；
State-machine：repair success/failed/exhausted；Mutation-style：逐个翻转硬门，确保
任一失败都拒绝 continue；Idempotency：重复 correction request不重复 commit。

#### 16. 回归范围

precheck desugar、synthetic hats、topic ownership、single business event、supervisor
slot terminals、reporter blocked path、artifact materialization。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改配置 | precheck/fixer/authority | E16 |
| `presets/schemas/parallel-forge.yml` | 修改 schema | evidence/correction fields | E18 |
| `presets/templates/parallel-forge/{wave-failure,merge-conflict,correction}.template.md` | 新增模板 | evidence SSOT | E17–E19 |
| `crates/ralph-cli/build.rs` / `builtin_artifact_templates.rs` | 修改生产/测试 | embed templates | E19 |
| precheck/runtime BDD tests | 新增测试 | S7–S13 | E16 |

#### 18. 完成标准

所有失败源均有证据路径；Verifier/Tester read-only 被 ownership/deny rule约束；
三轮边界、硬门、历史匹配、precheck exhaust、final correction 全绿；可独立提交。

#### 19. 停止条件

若 precheck 透明改写触及 `exec.unit.failed` 导致 fan-in 不收敛，立即停止；该
topic 必须恢复原生，仅在 fan-in 后 gate 业务决策。若修复需要越界路径，转语义
blocked，不能扩大 fixer 权限。

#### 20. 风险与注意事项

最大风险是“为了继续而伪造高 confidence”。独立 gate 必须重新打开 artifact、
spot-check 命令/路径并重算；数值不能覆盖硬否决。

### U7. 闭合最终门禁、复杂 DAG 恢复和全部下游同步

#### 1. Unit 目标

所有静态 wave settled 后才运行 Tester/Auditor/Reporter，并用复杂 DAG、
crash recovery、strict lint、AI docs 和全量测试证明整个 P0 闭环。

#### 2. 对应需求与 Scenario

- R25–R28；S1–S14；KTD11、KTD12；E19–E22。

#### 3. 外部可观察结果

四层复杂 DAG 从正确基线完成；任何中途失败都恢复或证据化阻塞；成功报告只在
full gate pass 后出现；resume 不重复副作用。

#### 4. 当前行为基线

现有 success BDD 是单 wave/14 steps，failed BDD 将 verifier failure直接报告。
U7 不新增此前未实现的业务能力，只把 final route 和跨单元恢复接上。

#### 5. 输入与输出

- 输入：全部 settled tasks/verified base/full test commands。
- 输出：full verification、audit、manager report、LOOP_COMPLETE。
- 错误：存在 open plan task、unsettled wave、missing evidence 时禁止 Tester/
  Auditor/terminal。
- 状态：resume 与 uninterrupted一致。

#### 6. 修改位置

final flow/preset/schema、complex DAG/restart BDD、AI skill guides、preset operator
skills、AGENTS/CLAUDE builtin description、zsh completion check、solution doc。

#### 7. 可依赖能力

U1–U6 全部已验证能力。

#### 8. 禁止依赖的未来能力

不扩展 UI、不新增 DB、不顺手修其它 presets 的 schema drift；但最终全量门禁
不能忽略真实失败。

#### 9. 验收测试

完整 S1–S14 suite；尤其四层 DAG、cap、verifier recovery、tester recovery、
四个 crash seams、terminal event counts。BDD 必须调用
`run_workflow_guard_scenario`。

#### 10. Acceptance Red

先扩展 full-flow scenario 到四层并要求 4 settlements；在 U1–U6 未接完整 final
route时 Auditor/Reporter 次数或 resume state 会失败。测试必须到达真实
EventLoop，不得只读 YAML。

#### 11. 单元测试拆分

1. all tasks closed 才 development done。
2. full Tester trigger 只发生一次。
3. failed full test不触发 audit。
4. final correction success后 tester重跑。
5. report/LOOP_COMPLETE payload match。
6. restart at four seams。
7. doc commands/static drift。

#### 12. Red → Green → Refactor 顺序

```text
complex DAG full BDD Red → 接 final route → Green
→ crash seam 1..4 逐个 Red/Green
→ docs/skills drift Red → 同步 agent-facing contracts → Green
→ strict preset/lint/scenario gates
→ full workspace gate
→ 清理 dead-end/临时代码与 ephemeral artifacts
```

#### 13. 最小实现范围

只闭合 final trigger、recovery 和文档；不得在 U7 才补 U1–U6 必需边界。发现缺口
必须退回对应 Unit 修订并重跑，而不是堆在 U7。

#### 14. 集成验证

执行 Verification Contract 全部命令。真实 Git E2E 使用临时 repo/worktree，
不得依赖开发者当前脏工作区。

#### 15. 风险驱动测试

State-machine、restart、idempotency、concurrency、fault injection；不做 fuzz，
因为风险集中在有界事件状态与 Git crash windows。

#### 16. 回归范围

全 workspace、doctest、E2E mock、all embedded presets、CLI doc drift、hat-env
污染复跑、zsh completion load。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| Parallel Forge preset/schema/BDD | 修改配置/测试 | final flow/recovery | E11,E15 |
| `crates/ralph-core/data/ralph-tools-{emit,wave,tasks}.md` | 修改文档 | agent-facing topics/actions | E20 |
| `skills/ralph-preset-common/references/*.md` 相关文件 | 修改文档/fixture | author/reviewer audit | E20 |
| `AGENTS.md`、`CLAUDE.md` | 同步修改 | builtin 描述 | E20 |
| `.cursor/rules/multi-hat-isolation.mdc` | 按实际影响修改 | hat topology | E20 |
| `scripts/ralph-zsh-plugin.zsh` | 检查并按需同步 | builtin completion hard rule | E20 |
| `docs/solutions/...` 新文档 | 新增文档 | durable mechanism learning | E20 |

#### 18. 完成标准

S1–S14、targeted、lint、build、E2E、全量 gate 全通过；两个 agent guides准确；
AGENTS/CLAUDE byte-identical；zsh completion已安装并加载；无 plan residual/ephemeral
文件；每个 Unit 可追溯、独立提交。

#### 19. 停止条件

全量测试出现 serial fallback 仍失败、hat-env污染回归、schema/preset不一致、
AGENTS/CLAUDE漂移、或实际 diff 超出计划。停止并回到对应 Unit，不得声明完成。

#### 20. 风险与注意事项

已有 ce-pipeline 四项 schema drift 必须复核。若仍是同一预存集合，可继续 targeted
开发，但最终 `./scripts/run-tests.sh` 必须全绿；否则本计划不能完成。

## 8. Unit 串行依赖图

```text
U1 transition_emits
  ↓ U2 需要可重复 development step 的结构，但先建立 plan authority
U2 static schedule validation
  ↓ U3 settlement 需要 U2 创建的完整 wave/task identity
U3 close_task_batch
  ↓ U4 派发时依赖 task 在 exec done 后仍 open
U4 static dispatch + lazy worktree
  ↓ U5 需要真实 current wave/fan-in/candidate inputs
U5 per-wave success settlement
  ↓ U6 correction 必须回接已经验证的 review/integrate/verify spine
U6 evidence-first correction
  ↓ U7 才能验证完整成功/失败/恢复与 final gate
U7 final convergence
```

- U1/U2 不可交换：U2 的新 topics最终需要停留在可重复 step，但 U2 自身可独立
  提交；先建 flow primitive 避免 preset 临时硬编码。
- U2/U3 不可交换：batch close 必须使用已校验的 wave/task identity。
- U3/U4 不可交换：静态 Dispatcher 判断“当前最小 open wave”要求 slot done不
  提前关闭。
- U4/U5 不可交换：Reviewer 必须消费真实完整 current-wave fan-in和 map。
- U5/U6 不可交换：修复回路复用同一条成功验证 spine。
- U6/U7 不可交换：final Tester failure recovery 是 U6 correction 的 final scope。

所有 Unit 严格串行；即使某些文件修改面独立，也不得并行，以避免 preset/schema/
BDD 同时漂移。

## 9. 执行命令清单

| 时机 | 命令 | 目的 | 进入下一步条件 |
|---|---|---|---|
| U1 baseline | `cargo nextest run -p ralph-cli --bin ralph -- presets` | 重现并记录已知 baseline | 仅已知4项或全绿；新增失败停止 |
| U1 | `cargo nextest run -p ralph-core -- transition_emits` | 新 flow semantics | 全绿 |
| U1 | `cargo nextest run -p ralph-core -- flow_declaration` | parser/lint/replay | 全绿 |
| U2 | `cargo nextest run -p ralph-core -- ensure_task_batch` | static schedule | 全绿 |
| U2/U3 | `cargo nextest run -p ralph-core -- state_projector` | task atomicity | 全绿 |
| U4–U7 | `cargo nextest run -p ralph-cli -- wave_supervisor` | cap/fan-in/recovery | 全绿 |
| U4–U7 | `cargo nextest run -p ralph-core --test scenarios -- parallel_forge` | real EventLoop BDD | 全绿 |
| preset每次修改 | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | CLI lint | 全绿 |
| preset每次修改 | `cargo nextest run -p ralph-core -- preset_lint` | core lint | 全绿 |
| preset每次修改 | `cargo nextest run -p ralph-cli --bin ralph -- presets` | embedded parity | 全绿；已知外部 drift需先解决 |
| template修改 | `cargo nextest run -p ralph-cli -- builtin_artifact_templates` | registry/materialize parity | 全绿 |
| CLI/doc行为修改 | `scripts/check-cli-doc-drift.sh` | agent guide drift | exit 0 |
| hat-env风险 | `RALPH_CURRENT_HAT=executor RALPH_CURRENT_LOOP_ID=loop-x RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-cli -- wave_supervisor` | 外层 hat env scrub | 全绿 |
| 每 Unit close | `cargo fmt --check` | 格式 | exit 0 |
| 每 Unit close | `cargo clippy` | lint/type | exit 0 |
| 每 Unit close | `cargo build` | build | exit 0 |
| U7 | `cargo run -p ralph-e2e -- --mock` | mock E2E | exit 0 |
| final | `./scripts/run-tests.sh` | 两阶段 nextest + doctest | 全绿 |
| flake only | `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 竞态/时序兜底 | 仅确认 flake；仍失败则停止 |

禁止裸跑 `cargo test -p ralph-cli`。BDD 必须顺序使用
`cargo nextest run -p ralph-core --test scenarios` 系列。

## Verification Contract

### Red 证据合同

每个 Unit 首次失败必须记录：

- 测试全名与命令；
- 实际失败摘要；
- 失败到达目标代码路径的证据；
- 为什么是能力缺失而非 fixture/环境错误；
- Green 后同命令结果。

### 每 Unit gate

1. 当前 Scenario Acceptance test。
2. 当前 Unit 最小单元测试。
3. 相关集成/BDD。
4. 受影响 crate regression。
5. `cargo fmt --check`、`cargo clippy`、`cargo build`。
6. 无 skip/only/弱化断言/无解释 snapshot。
7. 独立 commit，commit message描述可观察行为。

### 最终 gate

执行命令清单 final subset与 `./scripts/run-tests.sh`。任何真实失败都不允许
`LOOP_COMPLETE`；serial fallback 仍失败即为真实 blocker。

## 10. 最终质量门禁

- S1–S14 全部通过且 event counts/absent events 精确。
- R1–R28 每项至少一个测试，并能追溯到 U-ID。
- 静态 schedule invalid cases 零 task 副作用。
- `exec.unit.done` 不关闭 task；settlement batch atomic。
- worker cap 不拆逻辑 wave。
- future worktree 不提前创建，base SHA 可复核。
- 正式 branch 在 Verifier pass 前不移动。
- correction round、历史匹配、冲突硬门和终局阈值生效。
- Reviewer/Verifier/Tester 无源码修改 authority。
- full Tester 只在所有计划 wave settled 后运行。
- crash recovery 四个 seam 与 uninterrupted 结果一致。
- preset/schema/runtime/lint/BDD/config/CLI/docs 全部同步。
- `AGENTS.md` 与 `CLAUDE.md` 完全一致。
- CLI doc drift、zsh completion load、hat-env污染验证通过。
- 所有 nextest、build、clippy、fmt、E2E、full script通过。
- 没有新增跳过测试、`.only`、忽略标记、弱化断言或超时放大。
- 没有未解释 snapshot/golden 变化。
- 没有未处理 BLOCKED 决策；所有关键决策保持 ≥0.85。
- 实际变更不超范围；无临时/实验/plan residual文件。

## Definition of Done

本计划完成仅当：

1. `parallel-forge` 对复杂 DAG 只执行 Planner 冻结的静态 wave；
2. 每一计划 wave 都完成 fan-in、review、candidate integration、verification、
   promotion 和 settlement；
3. 所有失败先进入证据化 correction，终局失败满足独立 gate；
4. final Tester/Auditor/Reporter 顺序正确；
5. U1→U7 每个 Unit 均形成 Acceptance Red→Unit Red→Green→Refactor→
   Integration→Regression→Close；
6. Verification Contract 全绿；
7. 废弃的 14-step 一次性收尾路径、动态 ready 算法、提前 task close、提前
   worktree 和无证据直发失败路径均从代码/config/tests/docs中移除；
8. 所有 dead-end 实验代码和临时 artifact 被清理。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 7 个行为 Unit 均有真实入口、Red、最小实现、验证与停止条件 |
| Executor 是否仍需做关键设计决策 | 否 | KTD1–KTD12 已确定算法、权限、状态、失败与测试 seam |
| 所有文件和接口是否有代码库证据 | 是 | E1–E22；新增文件均标为新增 |
| 所有关键决策置信度是否 ≥0.85 | 是 | 最低 KTD12=0.87 |
| 是否存在未处理的低置信度假设 | 否 | KTD11/KTD12 由明确 BDD stop condition保护 |
| 每个 Unit 是否只有一个可观察行为 | 是 | flow、schedule、task、dispatch、settlement、correction、final 各自独立 |
| 每个 Unit 是否可以独立验证 | 是 | 每 Unit 有 targeted commands与独立 commit边界 |
| 每个 Unit 是否有真实 Red | 是 | 各 Unit §10 指定当前失败机制 |
| 每个 Unit 是否包含回归范围 | 是 | 各 Unit §16 |
| 是否存在未来 Unit 依赖 | 否 | 每个 Unit 只依赖已完成前置，§8 显式禁止 future behavior |
| 是否存在泛化任务描述 | 否 | 无“完善/视情况/相关测试”等未定动作 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §5、§6、U1–U7 |
| 所有关键决策是否有 Evidence | 是 | KTD1–KTD12 均引用 E-ID |
| 计划是否可以严格串行执行 | 是 | U1→U7 唯一顺序 |
