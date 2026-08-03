---
title: Parallel Forge Preset 集成缺口闭合 - Plan
type: fix
date: 2026-07-29
origin:
  - docs/plans/2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan.md
  - docs/handoff/260729-2111-handoff.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan
execution: code
---

# Parallel Forge Preset 集成缺口闭合 - Plan

## 0. 计划状态

- **状态：READY**
- **代码基线：** `3fd59f4be6c009e596baaa57be3d5a0bab37664d`
- **当前分支：** `pittcat-dev`
- **前置事实（已核实，禁止再排查）：**
  1. Plan `2026-07-29-001` 的 U1–U4 核心机制已落地且 targeted 测试绿：
     `transition_emits`、`EnsureTaskBatch` wave/order 校验、`CloseTaskBatch`、
     静态 wave schema。
  2. Plan 001 的 U5–U7 在 **preset / schema pointers / hat instructions /
     event_filter / artifact templates / BDD / docs** 层大量缺失；基线全绿
     是因为旧测试未推翻 + 新行为无 acceptance BDD。
  3. Plan `2026-07-29-003`（readonly hat gates）**不在本计划范围**；本计划
     完成后另起 `2026-07-29-006`（或次日编号）串行闭合。
- **调查范围：** 仅闭合 001 已声明、但未接到 parallel-forge preset 的缺口。
- **已执行的验证：**
  - 阅读 handoff `docs/handoff/260729-2111-handoff.md` 全文；
  - `grep ensure_task_batch` 确认 schema 缺 `execution_wave` /
    `integration_order` / `execution_plan_digest` pointers；
  - 核对 reviewer/integrator/verifier/tester 的 `event_filter.events` ≠
    `triggers`；
  - `ls presets/templates/parallel-forge/` 确认缺 4 个新模板；
  - 阅读 `project_close_task_batch`（`task.rs:743-848`）与
    `validate_wave_schedule`。
- **未执行的验证：** 本计划文档阶段不跑 Acceptance Red / nextest / full gate。
- **阻塞项：** 无设计阻塞。产品决策全部继承 001 的 KTD（不可改）。

## Goal Capsule

**目标：** 让 `parallel-forge` preset 真正消费 001 已落地的 runtime 能力——
静态 schedule 校验生效、per-wave hat 能收到新 topic、instructions 与 schema
对齐、settlement 原子、证据模板可 materialize、核心 BDD 覆盖、文档同步。

**权威层级：**
1. 本计划 Product Contract（缺口闭合范围）
2. Plan 001 的 Requirements R1–R28 / Event required-fields / KTD（产品语义）
3. 现有源码中已落地的 U1–U4 行为（不得回退）

**停止条件：**
- 发现需要改 supervisor / DB / 新 CLI / UI → 停止，另起 plan；
- 发现 003 readonly 合同必须先做才能让某条 BDD 绿 → 停止，记录到 003 闭合 plan；
- 发现旧 topic（`forge.units.reviewed` 等）仍被其他测试/文档强依赖且无法
  在本 plan 内安全清理 → 保留双写，把清理标为 P2 deferred。

## Product Contract

### Requirements

本计划只闭合「001 已声明、代码半落地、preset 未接」的缺口。R 编号与 001
对齐处用括号标注；本计划新增的可测缺口用 G 编号。

#### Schema / Projector 接线

- G1（R2–R4）。`presets/schemas/parallel-forge.yml` 的
  `state_projection.actions.forge.plan.ready`（`ensure_task_batch`）必须声明
  `execution_wave` / `integration_order` / `execution_plan_digest` 三个
  pointer，使 runtime `validate_wave_schedule` 对 parallel-forge 生效，
  不再走 pointer=None 的 legacy 静默跳过分支。
- G2（R2–R4）。`forge.plan.ready` 的 `unit_tasks` field_docs 与
  `required_fields` 必须要求每项含正整数 `execution_wave`、
  `integration_order`，payload 顶层含非空 `execution_plan_digest` 与
  `wave_total`（与 001 Event required-fields 合同一致）。

#### Planner / Hat 合同

- G3（R2–R3）。Planner instructions 第 4 步必须写明：
  - `wave(unit) = 1 + max(wave(dep))`（无依赖 → 1）；
  - wave 集合必须连续且从 1 开始；
  - `integration_order` 全局唯一、连续 `1..N`，且每条依赖边
    `order(dep) < order(unit)`；
  - `unit_tasks[]` 每项必须带 `execution_wave` / `integration_order`；
  - emit 必须带 `execution_plan_digest` 与 `wave_total`。
- G4。reviewer / integrator / verifier / tester 的
  `event_filter.events` 必须与各自 `triggers` 的业务入口一致（超集允许，
  缺项禁止），使新 topic 不被 filter 静默丢弃。
- G5（R9–R15, R18, R25）。上述四 hat 的 instructions 必须按 001
  Event required-fields 合同改写到新 topic，禁止仍指导 agent emit 旧
  `forge.units.reviewed` / `forge.integration.done` /
  `forge.incremental.verified` 作为主路径；禁止 Integrator/Verifier/Tester
  直接发 `work.failed`（改发对应 `*.failed` observation）。
- G6。Executor instructions 必须写明：task 只在 `forge.wave.settled` 时被
  `close_task_batch` 批量关闭；禁止暗示 `exec.unit.done` 会关 task。
- G7。Tester triggers / event_filter / instructions 必须覆盖
  `forge.exec.development.done` 与 `forge.final.correction.settled` 两条入口。
- G8。forge-failure-handler instructions 步骤编号必须连续无冲突。

#### Runtime 原子性

- G9（R14）。`project_close_task_batch` 必须保证：任一目标无法安全 close 时，
  **磁盘 ledger 零变更**；成功路径只 `persist` 一次。保留对
  `started.is_none()` 行先 `start()` 再 `close()` 的绕过（P0-4 守卫兼容）。

#### 证据模板

- G10。新增并注册 4 个 artifact templates：
  - `wave-settlement.template.md`
  - `wave-failure.template.md`
  - `merge-conflict.template.md`
  - `correction.template.md`
  同步 `presets/templates/parallel-forge/README.md`、
  `crates/ralph-cli/build.rs` 拷贝、
  `crates/ralph-cli/src/builtin_artifact_templates.rs` 的
  `PARALLEL_FORGE_TEMPLATE_NAMES` / 嵌入表。

#### BDD / 文档

- G11。至少补齐下列真实 EventLoop BDD（`run_workflow_guard_scenario`，
  禁止 stub）：S4（cap=2 + 5-unit）、S5（lazy worktree base）、S6（两 wave
  happy + 两份 settlement）、S7（slot fail → correction 非终局）、S10
  （verifier round1 recovery）、S11（3-round exhaustion）、S13（final
  correction）、S14（crash/replay seam 至少 1 条）。Scenario ID 对齐 001 §4。
- G12（R28）。同步 `AGENTS.md` / `CLAUDE.md`（byte-identical）中
  parallel-forge 描述；同步 `crates/ralph-core/data/ralph-tools-emit.md`（及
  若引用到的 wave/tasks skill）中与新 topic / `close_task_batch` 相关、
  agent 可执行的通用规则；新增 `docs/solutions/` 一篇记录本缺口根因与修法。

### 非目标

- 不实现 003 的 `disallowed_tools` / `allowed_write_paths` / workspace
  mutation guard 接线 / readonly evidence fields / precheck gate。
- 不新增 DB、CLI 子命令、UI。
- 不回退 001 U1–U4 已落地机制。
- 不强制删除 hat `publishes` 中的旧 topic 别名（可双写保留；清理属 P2）。
- 不评估/回滚 `NON_TRANSITION_TOPICS` 删 `work.failed` 的跨 preset 影响
  （记入 Appendix deferred；本计划只保证 parallel-forge BDD 行为正确）。

### Acceptance Examples

- AE-G1：带合法 `execution_wave`/`integration_order` 的 `forge.plan.ready`
  通过；缺号 wave / 同 wave 依赖 / 逆序 order 被 projector 拒绝且零 task。
- AE-G2：mock EventLoop 发出 `forge.wave.reviewed` 时，reviewer hat 能被
  filter 放行（不再依赖 `forge.exec.development.done`）。
- AE-G3：两 wave happy path 出现恰好两次 `forge.wave.settled`，且第二次
  worktree base = 第一次 `verified_base_commit`。
- AE-G4：`ralph preset materialize-artifacts parallel-forge --plan-key x`
  产出含 4 个新模板文件。
- AE-G5：人为构造 close_task_batch 第 N 个 ID 不可 close 的注入路径时，
  tasks 文件内容与调用前 byte-identical。

## Planning Contract

### 关键技术决策（继承 001，本计划不重开）

| ID | 决策 | 置信度 |
|---|---|---|
| KTD1–KTD12 | 全部继承 plan 001（supervisor+wave、静态 wave、settlement 关 task、wave-fixer、3-round、无新 DB/CLI/UI） | 1.00 session-settled |
| KTD-G1 | schema pointers 与 runtime `EnsureTaskBatch` 字段名逐字对齐：`execution_wave` / `integration_order` / `execution_plan_digest` | 0.99 |
| KTD-G2 | `event_filter.events` 取该 hat `triggers` 的业务入口全集；不靠关掉 filter | 0.97 |
| KTD-G3 | instructions 主路径只教新 topic；旧 topic 若仍在 `publishes` 仅作兼容，不写进步骤 | 0.95 |
| KTD-G4 | close_task_batch 原子性：在内存 store 上完成全部 start+close，成功后单次 persist；失败路径不调用 persist，且用 clone/staging 避免污染 `tasks_cache` | 0.96 |
| KTD-G5 | BDD 优先扩真实 EventLoop scenario；不新增「断言 instructions 含某段文字」的测试 | 0.99 |

### 高层设计

```text
001 U1–U4 (已落地 runtime)
        │
        ▼
U1  schema pointers + required_fields/field_docs
        │
        ▼
U2  planner instructions（wave 算法）
        │
        ▼
U3  四 hat event_filter ↔ triggers
        │
        ▼
U4  四 hat + executor + failure-handler + tester 入口 instructions
        │
        ▼
U5  close_task_batch 原子性加固 + 单测
        │
        ▼
U6  4 artifact templates + registry
        │
        ▼
U7  BDD S4/S5/S6/S7/S10/S11/S13/S14
        │
        ▼
U8  AGENTS/CLAUDE + ralph-tools-* + docs/solutions
```

### 系统性影响

- parallel-forge 上 `ensure_task_batch` 从「静默跳过 schedule」变为「强制校验」——
  既有 fixture / mock payload 若缺 wave/order 字段会变红，必须随 U1/U7 一起修。
- hat instructions 变更会影响 agent 行为，但不影响编译；以 BDD + preset_lint
  结构化检查验收。
- 模板 registry 变更会影响 `materialize-artifacts` 输出文件数断言。

## Implementation Units

### U1. 接通 ensure_task_batch 的 wave/order/digest pointers

#### 目标
让 parallel-forge 的 `forge.plan.ready` 投影真正跑 `validate_wave_schedule`。

#### 对应
G1, G2；001 R2–R4；AE-G1。

#### 修改位置
- `presets/schemas/parallel-forge.yml`：`state_projection.actions.forge.plan.ready`
  增加：
  ```yaml
  execution_wave: execution_wave
  integration_order: integration_order
  execution_plan_digest: execution_plan_digest
  ```
  （pointer 名以 `EnsureTaskBatch` 结构体字段为准；value 为 payload 内字段名。）
- 同文件 `forge.plan.ready.required_fields` 补 `execution_plan_digest`、
  `wave_total`；`unit_tasks` field_docs 声明每项必含 `execution_wave`、
  `integration_order`。
- 若 `presets/en/parallel-forge.yml` 内嵌了重复的 `state_projection` 块，
  必须与 schema 同步（以 schema 为 SSOT，preset 侧按现有惯例）。
- 更新任何因缺字段而失败的既有 fixture / 静态 preset 测试 payload。

#### Acceptance Red
写/改一个针对 parallel-forge schema 解析的结构化测试：加载 embedded
parallel-forge 后断言 `EnsureTaskBatch` 三个 pointer 均 `Some(...)`。
当前应失败（pointer 为 None）。

```bash
cargo nextest run -p ralph-cli --bin ralph -- presets
cargo nextest run -p ralph-core -- ensure_task_batch
```

#### 完成标准
pointers 非空；缺 schedule 字段的 payload 被拒绝；合法 DAG 仍绿；可独立提交。

#### 停止条件
若 runtime `EnsureTaskBatch` 字段名与文档不一致 → 以源码
`crates/ralph-core/src/config/state_projection.rs` 为准修订本 Unit，不得猜。

---

### U2. 重写 Planner instructions：静态 wave 算法

#### 目标
Planner agent 在 `unit_tasks[]` 中写出 runtime 可校验的 wave/order/digest。

#### 对应
G3；001 R2–R3。

#### 修改位置
`presets/en/parallel-forge.yml` → `planner.instructions` 步骤 4（及步骤 5 emit
字段列表）。按 HARD RULE 4 用 hat 视角写：只说本 hat 必须产出的字段与算法，
不提 projector 内部函数名。

必须覆盖：
1. 算法 `wave(unit)=1+max(wave(dep))`，无依赖 = 1；
2. wave 连续 1..W；
3. `integration_order` 全局唯一 1..N 且依赖边保序；
4. `unit_tasks[]` 每项字段清单；
5. `execution_plan_digest` / `wave_total` 必填。

#### Acceptance Red
结构化断言：解析后的 planner hat instructions 非空，且
`forge.plan.ready` schema `required_fields` 含 `execution_plan_digest` 与
`wave_total`（U1 已加）。**禁止**断言 instructions 字面包含某句中文。
用「emit 缺 `execution_wave` 的 fixture 被 policy-check / projector 拒绝」
作为行为 Red（可复用 U1 测试）。

#### 完成标准
instructions 与 G3 对齐；preset_lint strict 绿；可独立提交。

---

### U3. 对齐四 hat 的 event_filter 与 triggers

#### 目标
新 topic 能进入 hat activation，不再被 `event_filter` 丢弃。

#### 对应
G4；AE-G2。

#### 修改位置（核实后的当前错误值 → 目标值）

| Hat | 当前 `event_filter.events` | 目标（与 triggers 业务入口对齐） |
|---|---|---|
| reviewer | `[forge.exec.development.done]` | `triggers` 全集：`forge.wave.worktrees.ready`, `exec.wave.complete`, `forge.correction.done` |
| integrator | `[forge.units.reviewed]` | `forge.wave.reviewed`, `forge.wave.verified` |
| verifier | `[forge.integration.done]` | `forge.wave.integrated`（若仍保留旧 trigger 别名则一并列入） |
| tester | `[forge.incremental.verified]` | `forge.exec.development.done`, `forge.final.correction.settled`（并同步 `triggers`） |

实施前用 `rg -n "event_filter:" -A5 presets/en/parallel-forge.yml` 再确认行号。

#### Acceptance Red
结构化测试：解析 parallel-forge 后，对上述四 hat 断言
`event_filter.events` 覆盖其 `triggers`（或显式业务入口列表）。当前 reviewer
断言应失败。

```bash
cargo nextest run -p ralph-cli --bin ralph -- presets
```

#### 完成标准
四 hat filter/triggers 一致；preset_lint 绿；可独立提交。

---

### U4. 重写四 hat + executor + failure-handler instructions

#### 目标
Agent 按新 topic / required-fields 合同行动。

#### 对应
G5–G8；001 Event required-fields 表。

#### 修改位置
`presets/en/parallel-forge.yml`：

1. **reviewer**：入口改为 fan-in / correction.done；成功
   `forge.wave.reviewed`；失败 `forge.wave.review.failed`（含 failure
   fingerprint 字段）；禁止直接 `work.failed`。
2. **integrator**：区分两入口——
   - `forge.wave.reviewed` → 按 `integration_order` 写入 **candidate**，emit
     `forge.wave.integrated`；
   - `forge.wave.verified` → FF 正式 branch + emit `forge.wave.settled`
     （`settled_task_ids` 等）；
   失败走 `forge.wave.integration.failed` / 既有 failure observation，不直接
   `work.failed`。
3. **verifier**：入口 `forge.wave.integrated`；成功 `forge.wave.verified`；
   失败 `forge.verification.failed`（或 schema 中的
   `forge.wave.verification.failed`——以 schema 实际 topic 名为准）。
4. **tester**：入口 `forge.exec.development.done` /
   `forge.final.correction.settled`；成功 `forge.full.verified`；失败
   `forge.full.verification.failed`。
5. **executor**：删除「runtime 由事件投影原子关闭对应 task」；改为
   「task 在 `forge.wave.settled` 批量关闭」。
6. **forge-failure-handler**：步骤编号改为连续 1..N。

引用 skill 章节，不复述 `ralph-tools-*.md` 正文（HARD RULE 4.8）。

#### Acceptance Red
- 结构化：四 hat 的 `publishes` / `terminal_events` 含新 topic；
- 行为：至少 1 条 BDD（可与 U7 S6 共享）在 mock 路径上走到
  `forge.wave.settled` 而非旧 `forge.integration.done` 终态。

**禁止**「instructions 字符串包含某某句」测试。

#### 完成标准
instructions 与 schema required_fields 可对照；preset_lint 绿；可独立提交。

#### 停止条件
若 schema 中失败 topic 命名与 001 表不一致（例如
`forge.verification.failed` vs `forge.wave.verification.failed`）→ 以
**schema 现有 topic 名**为准改 instructions，并在 Unit 提交说明中记录偏差；
不得同时发明第三个名字。

---

### U5. 加固 `project_close_task_batch` 原子性

#### 目标
任一 close 失败 → 磁盘 tasks ledger 与调用前一致。

#### 对应
G9；001 R14；AE-G5。

#### 修改位置
`crates/ralph-core/src/state_projector/task.rs` → `project_close_task_batch`。

推荐实现（方向性，非贴码）：
1. 保持现有预校验（空数组 / 非字符串 / 重复 / 未知 / 混合 open-closed）；
2. 对 active settlement：在 **store 的 clone（或等价 staging）** 上完成全部
   `start()`（若需要）+ `close()`；
3. 全部成功后一次性写回并 `persist`；
4. 任何失败不调用 `persist`，不更新 `ctx.tasks_cache`。

保留 P0-4：`started.is_none()` 时先 `start()`。

#### Acceptance Red
新增/扩展 `close_task_batch` 单测：
- 批量中插入一个会导致 `close` 失败的 ID（或 mock），断言 tasks 文件
  byte-identical；
- 全成功路径仍关闭全部 ID 且只 persist 一次（可用现有成功用例）。

```bash
cargo nextest run -p ralph-core -- close_task_batch
```

#### 完成标准
原子性单测绿；既有 close_task_batch 表测全绿；可独立提交。

---

### U6. 补 4 个 artifact templates 并注册

#### 目标
`ralph preset materialize-artifacts parallel-forge` 能产出 settlement /
failure / conflict / correction 证据模板。

#### 对应
G10；AE-G4。

#### 修改位置
- 新增：
  - `presets/templates/parallel-forge/wave-settlement.template.md`
  - `presets/templates/parallel-forge/wave-failure.template.md`
  - `presets/templates/parallel-forge/merge-conflict.template.md`
  - `presets/templates/parallel-forge/correction.template.md`
- 更新 `presets/templates/parallel-forge/README.md` 表格；
- 更新 `crates/ralph-cli/src/builtin_artifact_templates.rs` 的
  `PARALLEL_FORGE_TEMPLATE_NAMES` + `PARALLEL_FORGE_TEMPLATES`；
- 确认 `crates/ralph-cli/build.rs` 拷贝目录逻辑覆盖新文件（通常是整目录
  copy；若有白名单则扩白名单）。

模板内容：章节骨架 + 必填字段提示即可；对齐 001 Event required-fields 中
对应 artifact 路径语义。不要塞 plan-id / U-ID（AI skill 去计划化规则：
模板是 agent 填写物，保持通用）。

#### Acceptance Red
`list_template_names("parallel-forge")` 含 4 个新 basename；
catalog 长度与 `PARALLEL_FORGE_TEMPLATE_NAMES` 一致（已有 drift 断言会红）。

```bash
cargo nextest run -p ralph-cli --bin ralph -- builtin_artifact
# 或等价 materialize / template 测试名
```

#### 完成标准
materialize 输出含新文件；registry 测试绿；可独立提交。

---

### U7. 补核心 BDD scenarios

#### 目标
用真实 EventLoop 路径锁住 001 的关键外部行为。

#### 对应
G11；001 §4 S4/S5/S6/S7/S10/S11/S13/S14；AE-G3。

#### 修改位置
- 新增/改写 `crates/ralph-core/tests/scenarios/parallel_forge_*.yml`
- 注册 `crates/ralph-core/tests/scenarios.rs`
- **必须** `run_workflow_guard_scenario`（禁止 `run_scenario` stub）

最低场景集合：

| ID | 断言要点 |
|---|---|
| S4 | `max_concurrent_workers=2`、wave 5 unit、同一 `wave_id`、`expected_total=5`、review 一次 |
| S5 | wave2 worktree base == wave1 `verified_base_commit`；无未来 wave worktree |
| S6 | 两波各一份 `forge.wave.settled`；正式 branch 在 verify 前不移动 |
| S7 | `exec.wave.failed` 后出现 correction 路径；`work.failed` absent |
| S10 | verifier 首次失败 → correction → 再 verify → settle |
| S11 | round 耗尽前无终局 `work.failed`；耗尽且证据达标才允许 |
| S13 | final tester 失败 → final correction → `forge.final.correction.settled` → 重跑 full |
| S14 | 至少一条 crash/replay：settlement emit 前中断后恢复，tasks/refs 与 uninterrupted 一致 |

旧 5 个 parallel_forge scenario：能改则改到新 topic；不能改的标 deprecated 并
确保不与新合同冲突（或删掉错误断言）。

#### Acceptance Red
先写 S6（或 S4）scenario 期望两次 `forge.wave.settled` / 新 topic 链；
当前应红。

```bash
cargo nextest run -p ralph-core --test scenarios -- parallel_forge
cargo nextest run -p ralph-cli -- wave_supervisor
```

#### 完成标准
上表场景全绿；旧场景无矛盾断言；可独立提交。

#### 停止条件
若某条场景必须依赖 003 readonly guard 才有意义 → 将该条移出本计划，写入
003 闭合 plan，不得在本 Unit 假绿。

---

### U8. 同步文档与 agent skills

#### 目标
满足 AGENTS.md HARD RULE：preset 拓扑变更的下游文档同步。

#### 对应
G12；001 R28。

#### 修改位置
1. `AGENTS.md` / `CLAUDE.md`：parallel-forge 描述从「旧 14-step」改为
   「静态 wave + per-wave settlement + development_loop」；改完
   `cp AGENTS.md CLAUDE.md`（或反向）保证 byte-identical。
2. `crates/ralph-core/data/ralph-tools-emit.md`（必要时
   `ralph-tools-wave.md` / `ralph-tools-tasks.md`）：只补 **通用**、agent
   可执行规则（新 topic 怎么 emit、`close_task_batch` 语义对 agent 的含义）；
   **禁止**写入 plan-id / U-ID / 过窄 preset 案例。
3. `docs/solutions/` 新增一篇（建议 path：
   `docs/solutions/workflow-orchestration/parallel-forge-preset-integration-gap.md`）：
   记录「runtime 已有、schema pointer 未接 → 静默跳过」根因与修法。
4. 若 operator skills（`skills/ralph-preset-*`）引用 parallel-forge 旧
   14-step 假设，做最小必要同步。

#### Acceptance Red
- `diff -q AGENTS.md CLAUDE.md` 无输出；
- `scripts/check-cli-doc-drift.sh`（若涉及命令）通过；
- solutions 文档存在且 frontmatter 含 module/tags。

#### 完成标准
文档与行为一致；全量 `./scripts/run-tests.sh` 绿；可独立提交。

## Verification Contract

### 每 Unit gate

1. 本 Unit Acceptance Red → Green
2. 本 Unit 列出的 targeted `cargo nextest run ...`
3. `cargo fmt` / 相关 clippy 无新增告警
4. 不引入 instructions 字面量锁定测试

### 最终 gate

```bash
cargo nextest run -p ralph-cli --bin ralph -- presets
cargo nextest run -p ralph-core -- ensure_task_batch close_task_batch state_projector
cargo nextest run -p ralph-cli -- wave_supervisor
cargo nextest run -p ralph-core --test scenarios -- parallel_forge
./scripts/run-tests.sh
diff -q AGENTS.md CLAUDE.md
```

### Red 证据合同

每个 Unit 合并前必须保留「曾红过」的证据叙述（测试名 + 失败原因一句话）
写在 commit message 或 solutions 文档；禁止未红先绿。

## Definition of Done

1. G1–G12 均可追溯到测试或结构化断言。
2. U1→U8 严格串行完成；每 Unit 可独立提交。
3. parallel-forge 上 `validate_wave_schedule` 不再被 pointer=None 跳过。
4. 四 hat `event_filter` 与 triggers 对齐；instructions 主路径使用新 topic。
5. `project_close_task_batch` 失败路径磁盘零变更有单测证明。
6. 4 个新模板可 materialize；registry 无 drift。
7. S4/S5/S6/S7/S10/S11/S13/S14 真实 EventLoop BDD 绿。
8. AGENTS.md ≡ CLAUDE.md；solutions 文已写；`./scripts/run-tests.sh` 全绿。
9. 无 `.ralph/review/**/residuals*` / scratch / draft 进入 git。
10. **不宣称 001 §11 DoD 全部成立**——003 readonly 与 001 中依赖 readonly
    的条款留给后续 plan；本计划 DoD 只覆盖「preset 集成缺口闭合」。

## Appendix

### Deferred（明确不做）

- Plan 003 全量闭合（guard 接线、三 hat 双禁、readonly evidence、S1–S17）。
- 清理 `publishes` 中旧 topic 别名。
- `NON_TRANSITION_TOPICS` 删 `work.failed` 的跨 preset release notes。
- `workspace_mutation_guard::compute_delta` 死参数。
- Plan 002 DoD 复核（003 入口前置；本计划不依赖）。

### 证据速查（实施前再跑一遍）

```bash
grep -A 12 "ensure_task_batch" presets/schemas/parallel-forge.yml
rg -n "event_filter:" -A5 presets/en/parallel-forge.yml
ls presets/templates/parallel-forge/
rg -n "project_close_task_batch" crates/ralph-core/src/state_projector/task.rs
```
