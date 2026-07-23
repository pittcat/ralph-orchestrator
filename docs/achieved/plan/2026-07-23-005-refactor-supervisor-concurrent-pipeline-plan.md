---
title: "重构 ce-executor-supervisor 为依赖感知并发 Pipeline"
date: 2026-07-23
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
status: reviewed
origin:
  - docs/report/2026-07-23-ce-executor-supervisor-primary-20260723-082003-diagnosis.md
parallel_track: supervisor-runtime-p0
depends_on: []
---

# 重构 ce-executor-supervisor 为依赖感知并发 Pipeline

## Goal Capsule

只重构 `ce-executor-supervisor` preset：保留并升级 `task-planner`，让它把输入计划重新拆分为可执行 DAG，识别 unit 依赖、耦合与文件冲突，并只批量 dispatch 当前 ready wave；executor 在 Exec wave 内并发，tester 与多维 reviewer 在同一个只读 Review wave 内并发，修复只经过一条正式 Fix 链（可含多个串行波次、每波内部并发），alignment 串行，reporter 统一处理成功与失败并结束 loop。

彻底删除 `progress-steward`、`shipper` 和重复 fallback `fixer`。`presets/en/ce-executor-pipeline.yml` 是只读对齐参考，任何 Unit 都不得修改它。

本计划与 runtime P0 计划可并发开发：本计划不修改 dispatcher/store/worker/emit；通过冻结的 runtime contract fixture 测试 preset。合并后才运行真实 supervisor 联合 E2E。

## 1. 功能目标

### 业务目标

- 让原始计划中互相独立的 Unit 真正并发，有依赖或文件冲突的 Unit 正确串行。
- 保留 task-planner 的业务价值：它不是简单复制 plan，而是生成可审计的 execution DAG。
- 让测试与评审基于同一个已合并不可变代码引用并行执行。
- 删除历史补丁式 hats，使每类失败都有明确消费者、报告和有限终止路径。

### 本次范围

- 定义 execution-plan artifact 的版本、provenance、节点、边、冲突和 ready-wave 契约。
- task-planner 读取显式 `plan_path`，解析/重拆/合并耦合 Unit，检测依赖和文件冲突。
- executor ready-wave 迭代调度；每个 wave 用一次 batch emit。
- tester 作为 Review wave 的只读验证 dimension，与其他 reviewer slots 并发。
- 唯一 review synthesis → fix plan → Fix wave → fix integration 链。
- alignment 串行。
- reporter 消费统一成功/失败结果，先写报告，再产生唯一 loop terminal。
- 删除 `progress-steward`、`shipper`、fallback `fixer` 的全部拓扑、权限、schema、lint、BDD 和文档残留。

### 非目标

- 不修改 supervisor runtime、store、dispatcher、worker 或 emit。
- 不新增独立 `Test` WaveKind；当前 runtime 仅支持 Exec/Review/Fix。
- 不修改 `presets/en/ce-executor-pipeline.yml`。
- 不修改原始计划文件；execution-plan 是派生 artifact。
- 不做 DAG UI、交互重排、跨 loop 可视化或通用 planner framework。
- 不允许 tester/reviewer 修改代码；所有修复进入唯一正式 Fix 链。

### 已知约束和假设

- “executor/tester/reviewer 并发”指各重阶段内部 supervisor fan-out；阶段边界保持：

```text
plan parse/DAG
→ ready Exec wave(s) + fan-in/integrate
→ one Review wave(test dimension + review dimensions)
→ synthesize
→ optional Fix wave + integrate
→ alignment
→ reporter
```

- tester 与 reviewer 共享 Review wave 的 `SharedReadonly` 隔离；它们必须读取同一 merged commit、原 plan、diff 和前序测试证据。
- 静态 DAG artifact 是依赖事实；动态状态以 runtime task API 为事实源，禁止两套状态并行维护。
- execution-plan artifact 位于主 workspace 的受控普通 artifact 路径，由 task-planner 单独拥有；不得写入 runtime 内部 ledger，也不得要求其他 hats 读取 `.ralph/supervisor.db` 或 events JSONL。
- `plan.blocked` 固定为 reporter 可消费的失败业务信号；源码中的 `pending_plan_blocked_for_failure` 已明确为 terminal reporting chain 预留一个 activation。所有产生 `plan.blocked` 的路径必须在 preset 中把 reporter 声明为消费者。reporter 被激活后先写失败报告，再发唯一 `LOOP_COMPLETE`。Plan B 的 contract fixture 冻结该顺序；合并后由 Plan A 的真实 runtime E2E 再证明 blocked loop 在 registry/inspect/CLI 上仍是失败而非 silent-success。

## Product Contract

### Execution-plan artifact

新建共享业务 artifact `.ralph/review/<plan-key>/execution-plan.yml`；`<plan-key>` 使用现有 plan baseline 规则从显式 plan path 派生。它不是 `.ralph/events*.jsonl`、`supervisor.db`、task ledger 等 runtime 内部状态；task-planner 只写该文件，后续 hats 只读。契约必须包含：

- `version`
- `source_plan_path`
- `source_plan_hash`
- `generated_at`
- nodes：`unit_id`、目标、输入/输出、文件写集、验证依赖、原始 provenance
- edges：显式依赖、隐式产物依赖、文件冲突依赖、合并/重拆理由
- ready-wave selection：稳定排序、当前 wave 节点和阻塞原因
- 不包含动态 started/done/failed 状态；动态状态从 task API 查询

### DAG 规则

- 显式依赖优先；隐式依赖只能由原计划的输入/输出、同一 artifact 或文件写冲突证明。
- 高耦合、无法独立验收的 Unit 合并为一个可验证节点，并保留来源 Unit IDs。
- 文件写集交叉的节点不得同 wave，除非合并为一个节点。
- 只有所有前置 task 为 done 的节点可进入 ready wave。
- 一次 ready wave 只能通过一次 `ralph wave emit --payloads ...` 批量发出。
- 输入相同且 task 状态相同，DAG 与 ready-wave 顺序稳定。
- 首次通过校验的 artifact 以 `source_plan_hash` 为 replay key 持久化；同一 hash 的后续 activation 必须复用原 nodes/edges，不得再次调用 agent 重生成 DAG。task 状态变化只重新计算 ready set。源 hash 变化才允许生成新 artifact，并使用新的 artifact version。
- 空计划、重复 ID、未知依赖、自依赖、环、无 ready node 均结构化失败，不猜测。

### 冻结的 runtime 消费契约

- input：public wave ID、slot/task identity、单 slot 终态和 success resource。
- output：`exec.wave.complete|failed`、`review.wave.complete|failed`、`fix.wave.complete|failed`。
- 每 slot 恰好一个 done/failed；event_count=0 不成功。
- 本计划只在 contract fixture 中消费该契约，不实现它。

### 权限模型

- task-planner：只读 source plan；只写 execution-plan artifact 和允许的 task 编排。
- executor/fix-worker：只写自己的 slot worktree。
- integrator：唯一允许合并对应 wave 的角色。
- tester/reviewer：复用 `review-batch-worker`，由 payload 的 `dimension=testing|...` 区分职责；不修改源文件、工作树或测试。`SharedReadonly` 只表示共享主 workspace、没有独立 worktree，不视为 OS 级写保护；review coordinator 在发波前记录基线 commit 与 clean/diff fingerprint，review synthesizer 在 fan-in 后复核，任何写集变化都使 Review wave fail-close。preset 不自动恢复或删除变化，避免误删用户或其他进程的并发写入。
- alignment：只做对齐检查；若需代码修复，返回结构化 failure，不越权修改。
- reporter：只写最终报告并发 loop terminal。

## Planning Contract

### 严格串行

```text
Unit 1 → Unit 2 → Unit 3 → Unit 4 → Unit 5 → Unit 6 → Unit 7 → Unit 8 → Unit 9
```

### 文件所有权

本计划可修改：

- `presets/en/ce-executor-supervisor.yml`
- `presets/schemas/ce-executor-supervisor.yml`
- `crates/ralph-core/src/preset_lint/supervisor_preset_test.rs`
- 必要的其他 preset-specific lint 文件
- 新建/修改 `crates/ralph-core/tests/scenarios/supervisor/*.yml`
- `crates/ralph-core/tests/scenarios.rs`
- `crates/ralph-cli/tests/integration_supervisor_primary.rs`
- preset operator skills 的相关 references/checklist/fixture
- `CLAUDE.md` 与 `AGENTS.md`（必须完全同步）
- `.cursor/rules/multi-hat-isolation.mdc`
- `scripts/ralph-zsh-plugin.zsh` 仅在 builtin 名称/补全项变化时

本计划禁止修改：

- `presets/en/ce-executor-pipeline.yml`
- `crates/ralph-cli/src/loop_runner/wave/`
- `crates/ralph-core/src/supervisor/`
- `crates/ralph-cli/src/commands/emit.rs`
- `crates/ralph-core/data/ralph-tools*.md`（通用 runtime guide 归 Plan A；本 preset 的一次性规则不能写入注入指南）
- `crates/ralph-cli/tests/integration_supervisor_runtime_p0.rs`

## 2. BDD 行为规格

### Feature B1：task-planner 生成稳定 execution DAG

```gherkin
Feature: 将原始计划转换为可审计的依赖 DAG

  Scenario: 独立和依赖 Unit 被正确分波
    Given 原计划包含两个互相独立 Unit 和一个依赖二者产物的 Unit
    When task-planner 生成 execution-plan
    Then 前两个 Unit 位于同一 ready wave
    And 第三个 Unit 在前置全部完成前保持 blocked
    And 原计划文件未被修改

  Scenario: 耦合 Unit 被合并
    Given Unit B 的验收必须读取 Unit A 尚未合并的产物
    And 二者无法在独立 worktree 中分别验收
    When task-planner 分析依赖
    Then 二者合并为一个可独立验证的执行节点或形成严格依赖边
    And artifact 记录理由和来源 Unit IDs

  Scenario: 文件冲突阻止错误并发
    Given 两个 Unit 修改相同文件
    When planner 选择 ready wave
    Then 它们不会进入同一 wave

  Scenario: 相同输入稳定重放
    Given source plan hash 和 task 状态未变化
    When planner 重跑
    Then DAG、边和 ready-wave 顺序保持一致
    And 不重复创建 task 或 wave
```

### Feature B2：非法计划失败闭环

```gherkin
Feature: 非法或不可编排计划不会静默停止

  Scenario Outline: 非法 DAG 输入
    Given 原计划包含 <problem>
    When task-planner 解析
    Then 不 dispatch worker
    And 产生结构化失败，包含 source plan、reason 和证据
    And reporter 生成失败报告并结束 loop

    Examples:
      | problem |
      | 空 Unit 列表 |
      | 重复 Unit ID |
      | 未知依赖 |
      | 自依赖 |
      | 依赖环 |
      | 不可读取的 plan_path |
      | 无 ready node 且无已知失败 |
```

### Feature B3：依赖感知 Exec waves

```gherkin
Feature: 只并发执行当前 ready nodes

  Scenario: 一个 ready wave 单次批量发射
    Given 三个独立 ready nodes
    When task-planner dispatch
    Then 只调用一次 batch wave emit
    And 三个 payload 共享一个 public wave ID
    And wave_total 等于 3

  Scenario: 前置成功后解锁下一 wave
    Given downstream node 依赖 wave 1 的全部节点
    When wave 1 fan-in 并集成成功
    Then task-planner 重新查询 task API
    And 只 dispatch 新解锁的 downstream node

  Scenario: 前置失败阻止下游启动
    Given upstream slot 失败、超时或取消
    When exec wave failed
    Then downstream task 标记 failed 并记录 upstream_dependency_failed 与上游 reason
    And 不创建其 worker
    And failure 进入 reporter
```

### Feature B4：tester 与 reviewer 共享只读 Review wave

```gherkin
Feature: 已合并代码上的测试和多维评审并发

  Scenario: tester 与 reviewers 检查同一 immutable code reference
    Given 所有 Exec waves 已集成并产生 merged commit
    When review coordinator 发出一个 Review wave
    Then testing dimension slot 和各 review dimension slots 由同一个 review-batch-worker hat 并发执行
    And 每个 slot 获得相同 source plan、merged commit、diff 和测试上下文
    And 所有 slot 均使用 SharedReadonly binding

  Scenario: tester 尝试写代码
    Given tester slot 处于 shared-readonly
    When tester 尝试修改源文件
    Then fan-in 后的主 workspace 写集校验检测到变化
    And 未授权变更证据被保留且 review fail-close
    And preset 不自动恢复或删除共享 workspace 的变化
    And review wave 产生结构化 failure

  Scenario: 任一检查失败
    Given tester 或任一 required reviewer failed
    When review fan-in
    Then synthesizer 获得完整成功与失败证据
    And 不把 review 标为 passed
```

### Feature B5：唯一正式 Fix 链与串行 alignment

```gherkin
Feature: 所有代码修复通过唯一正式修复链

  Scenario: 有 must-fix findings
    Given review synthesis 产生可执行 fix units
    When fix planner 建立 fix DAG
    Then 唯一正式 Fix 链按依赖产生一个或多个串行 wave
    And 每个 wave 内的独立 fix units 并发
    And fix integrator 合并并运行门禁
    And 不触发 fallback fixer

  Scenario: 无 must-fix findings
    Given review synthesis 判定无需修复
    When review 完成
    Then 跳过 Fix wave
    And 进入串行 alignment

  Scenario: alignment 发现阻塞问题
    Given alignment 只读检查失败
    When 它不能在权限内修复
    Then 产生结构化 failure 交给 reporter
    And 不自行修改代码
```

### Feature B6：删除 hats 后成功和失败都有限终止

```gherkin
Feature: Reporter 是唯一报告与 loop completion 所有者

  Scenario: 正常成功
    Given execution、review、fix（如有）和 alignment 全部通过
    When reporter 被激活
    Then 写成功报告
    And 发出唯一 LOOP_COMPLETE

  Scenario Outline: 任一阶段失败
    Given <failure> 已形成结构化失败摘要
    When reporter 被激活
    Then 写失败报告，包含 plan provenance、public wave IDs、task/slot 状态、未执行节点和 reason
    And 发出唯一 LOOP_COMPLETE
    And 之后不再接受业务事件

    Examples:
      | failure |
      | planner invalid/blocked |
      | exec wave failed |
      | merge conflict |
      | tester/reviewer failed |
      | fix wave failed or exhausted |
      | aggregate timeout |
      | operator cancel |
      | alignment failed |

  Scenario: reporter 自身失败
    Given reporter 无法写目标报告
    When 单次写入失败
    Then reporter 不重试写文件
    And 在唯一 LOOP_COMPLETE payload 中携带 status=failed、report_written=false、目标路径、写入错误和原本应写入的最小失败摘要
    And loop 不进入无消费者等待

  Scenario: 已删除 hats 无残留
    Given 新 preset 与 schema
    When strict lint 和 workflow activation 分析运行
    Then progress-steward、shipper、fallback fixer 均不存在
    And 没有 trigger、publish、deny rule、state projection 或权限仍引用它们
    And 没有 dead-end topic
```

## 3. 验收与测试策略

| Scenario | 验收条件 | 推荐测试层级 | 是否需要 E2E |
| --- | --- | --- | --- |
| B1 独立/依赖分波 | DAG、边、waves 与原计划一致 | 契约 + BDD | 否 |
| B1 耦合/文件冲突 | 不错误并发，理由可审计 | 单元规则 + BDD | 否 |
| B1 稳定重放 | 不重复 task/wave | 幂等集成 | 否 |
| B2 非法计划矩阵 | 0 worker，失败报告，loop 结束 | BDD + 集成 | 是，选环依赖 |
| B3 batch emit | 单 wave ID、wave_total=N、真实并发 | Runtime fixture 契约 | 是 |
| B3 依赖解锁 | 上波完成后才 dispatch | 状态机 BDD | 是 |
| B3 失败传播 | downstream 不启动 | 集成测试 | 否 |
| B4 统一代码引用 | tester/reviewer 同 commit | 契约 + 集成 | 是 |
| B4 只读权限 | 写入被实际拒绝 | 权限集成 | 否 |
| B4 失败聚合 | 任一 required failure 不通过 | BDD | 否 |
| B5 唯一正式 fix chain | 无 fallback fixer 激活；允许依赖导致多个串行波次 | 结构 lint + BDD | 是 |
| B6 成功终止 | 报告先落盘，唯一 LOOP_COMPLETE | Full-chain E2E | 是 |
| B6 失败终止矩阵 | 每条失败均有 reporter consumer | BDD + 关键 E2E | 是，代表路径 |
| B6 无残留 | schema/topology 无 dead refs | preset lint | 否 |

## 4. 需求—测试追踪矩阵

| 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E |
| --- | --- | --- | --- | --- | --- |
| R-B1 保留升级 task-planner | B1/B2 | DAG fixture | parse/edge/cycle/stable sort | artifact contract | invalid plan |
| R-B2 依赖感知并发 executor | B3 | ready-wave scenario | readiness/conflict | batch wave fixture | multi-wave |
| R-B3 tester/reviewer 并发只读 | B4 | review scenario | payload completeness | permission/commit contract | review wave |
| R-B4 唯一正式修复链 | B5 | fix scenario | fix readiness | fix fan-in contract | fix path |
| R-B5 删除三个 hats | B6.4 | topology acceptance | lint rules | strict preset lint | full-chain |
| R-B6 失败也报告并结束 | B2/B6 | failure matrix | routing table | workflow guard | representative failures |
| R-B7 pipeline yml 只读 | 全部 | git diff guard | 不适用 | path ownership check | 否 |

## 5. 严格串行开发单元

### Unit 1：建立 preset 对冻结 runtime 契约的独立测试夹具

- **Unit 目标**：建立一个可独立 Green 的冻结 runtime fake/fixture，只模拟 public wave ID、slot identity、单终态、success resources 和 Exec/Review/Fix coordination events；真实 worker 空结果、ID 恢复和终态竞态只属于两计划合并后的联合门禁。
- **对应 Scenario**：为 B3–B6 提供 runtime contract 边界，本 Unit 只验收 fixture 忠实度。
- **外部可观察结果**：fixture contract self-test 通过，Plan B targeted tests 不需要 Plan A 生产修复。
- **输入与输出**：输入当前 preset/schema/diagnosis；输出测试 fixture 和决策记录。
- **可依赖的已完成能力**：现有 parse_yaml、preset lint、`run_workflow_guard_scenario`、supervisor contract fixture。
- **明确禁止依赖的未来能力**：不得依赖 Unit 2 的 planner 实现或 Unit 6 的删除结果。
- **验收测试**：fixture self-test 覆盖 complete/failed、success resources、重复 terminal 拒绝和 public ID 透传。
- **需要拆分的单元测试**：fixture payload schema、single terminal、Memory-only task snapshot、stage failure injection。
- **Red 预期失败原因**：Plan B 专属冻结 contract fixture 尚不存在。
- **Execution note**：test-first——先写 fixture self-test 断言（Red），再实现冻结 contract fixture（Green）；fixture 必须明确标记为 Plan B contract double，不能冒充真实 worker E2E。
- **最小实现范围**：仅新增 preset test fixture，不修改 runtime 或生产 preset；fixture 必须清楚标记为 Plan B contract double，不能冒充真实 worker E2E。
- **集成验证**：`cargo nextest run -p ralph-core -- supervisor_preset`。
- **回归范围**：现有 preset parse/lint 继续通过。
- **完成标准**：fixture self-test Green；文件 ownership guard 明确断言 `ce-executor-pipeline.yml` 不在改动集。
- **风险与注意事项**：真实 runtime contract 只在合并后联合门禁验证；本 Unit 不掩盖该发布依赖。

### Unit 2：让 task-planner 产出带 provenance 的 execution-plan artifact

- **Unit 目标**：task-planner 从 runner 已注入 prompt/context 中的原始 plan 路径读取计划，并产出符合 schema、带 source path/hash/provenance 的静态 artifact；本 Unit 不推导最终边或 dispatch。
- **对应 Scenario**：B1 的原计划不变与 artifact provenance；B2 的不可读、空 Unit、重复 ID。
- **外部可观察结果**：合法输入生成节点清单；基础非法输入零 dispatch 并发 `plan.blocked`；原计划 hash 不变。
- **输入与输出**：输入 runner 注入的原始 plan path 和 plan 内容；输出 execution-plan artifact 的 source/nodes 部分。
- **可依赖的已完成能力**：Unit 1 runtime contract fixture。
- **明确禁止依赖的未来能力**：不依赖 Exec dispatch、Review 或 Fix 实现。
- **验收测试**：正常、不可读 plan、空、重复 ID、source hash、原文件不变。
- **需要拆分的单元测试**：本能力由 agent 产生 artifact，不虚构 Rust parser 单元测试；使用 schema validation、mock agent response 和真实 EventLoop BDD 验证节点字段、provenance 与失败 handoff。
- **Red 预期失败原因**：当前 task-planner 不产出共享 artifact，且 instructions 没有把 runner 注入的原始 plan 上下文当作唯一来源。
- **Execution note**：test-first——先写 artifact provenance 与非法输入（不可读/空/重复 ID）的 BDD scenario（Red），再修改 task-planner hat instructions 与 schema。
- **最小实现范围**：修改 hat instructions/schema/BDD，使 agent 生成 artifact；不新增确定性 parser、CLI 或 runtime 模块。instructions 只引用现有可执行命令和注入 skill。
- **集成验证**：用隔离 task-planner activation 读取 fixture plan，检查 artifact 与 task API；回归入口 `cargo nextest run -p ralph-core --test scenarios -- task_planner`（覆盖 artifact happy path 与非法输入场景）。通过判据：artifact nodes/provenance 字段断言全绿、非法输入零 dispatch 且 `plan.blocked` 可达 reporter、source plan hash 未变。
- **回归范围**：plan parsing、task three-field、isolated single-business-event budget。
- **完成标准**：artifact schema/provenance 稳定；动态状态不写入 artifact；基础非法输入 reporter 可达；source plan 未修改。
- **风险与注意事项**：artifact 必须在主 workspace 对后续 hats 可见，不能落 ephemeral private worktree。

### Unit 3：让 task-planner 推导依赖、耦合与文件冲突

- **Unit 目标**：在 Unit 2 节点清单上生成有证据的 edges，并拒绝未知依赖、自依赖、环和无证据猜测。
- **对应 Scenario**：B1 的独立/依赖、耦合、文件冲突与稳定重放；B2 的 unknown/self/cycle/no-ready。
- **外部可观察结果**：artifact 增加带 `kind`、`from`、`to`、`evidence` 的边；输入计划中需跨 worktree 集成验收的能力（原文称“U5 类”，指输入计划中的示例 Unit，非本计划的 Unit 5）被合并或严格排序；非法图零 dispatch 并到 reporter。
- **输入与输出**：输入 Unit 2 静态 nodes 和原计划证据；输出 edges、合并节点 provenance、stable ordering key 和 blocked reason。
- **可依赖的已完成能力**：Unit 2 artifact source/nodes。
- **明确禁止依赖的未来能力**：不创建 runtime task，不计算动态 ready 状态，不 emit wave。
- **验收测试**：显式依赖、artifact 输入输出依赖、同文件冲突、耦合合并、unknown/self/cycle；同一 source hash 的第二次 activation 必须复用首次 artifact，不再次生成。
- **需要拆分的单元测试**：该能力由 task-planner agent 完成；以 schema、mock responses 和 BDD 验证证据完整性、环拒绝和规范化排序，不虚构 Rust SCC/parser 单元测试。
- **Red 预期失败原因**：当前 planner 没有共享 DAG/边证据，所有 Unit 被一次性广播。
- **Execution note**：test-first——先写边证据完整性、unknown/self/cycle 拒绝与稳定排序的 scenario（Red），再扩展 artifact schema 与 task-planner instructions。
- **最小实现范围**：扩展 artifact schema 与 task-planner instructions；不新增 runtime 算法模块。稳定性由 source-hash keyed artifact reuse 保证；首次生成只要求 nodes/edges 规范化排序和证据完整，不要求自然语言理由 byte-equal。
- **集成验证**：`run_workflow_guard_scenario`（真实 EventLoop runner，禁用 `run_scenario` stub）对 fixture plans 验证 artifact 与零 dispatch failure；回归入口 `cargo nextest run -p ralph-core --test scenarios -- task_planner`。通过判据：每条边 evidence 可追溯、unknown/self/cycle 图零 worker、同 source hash 的二次 activation 复用首次 artifact 不重生成。
- **回归范围**：artifact provenance、单事件预算、plan.blocked reporter consumer。
- **完成标准**：每条边可追溯；非法图无 worker；相同 source hash 不重生成；本 Unit 不触碰 task/wave。
- **风险与注意事项**：LLM 不负责机械最优，只要求 schema、证据和行为门禁；缺证据时 fail-close。

### Unit 4：把 DAG 节点幂等物化为 runtime tasks

- **Unit 目标**：为每个规范化 DAG node 创建唯一 task，并建立 node key 到 task_id/task_key 的可审计映射。
- **对应 Scenario**：B1 稳定重放、B3 前置失败传播。
- **外部可观察结果**：首次执行创建一次 tasks；重放复用原 tasks；downstream 初始保持 open，前置最终失败时转为 failed，reason=`upstream_dependency_failed`。
- **输入与输出**：输入静态 DAG；输出 task API entries 和 artifact 中仅含 identity 的映射，不把动态状态写回 artifact。
- **可依赖的已完成能力**：Unit 3 合法 DAG。
- **明确禁止依赖的未来能力**：不选择 ready wave、不调用 wave emit。
- **验收测试**：首次 materialize、相同 source hash 重放、部分 tasks 已存在、identity 冲突、上游失败的 downstream failed 投影。
- **需要拆分的单元测试**：使用 task API 集成/BDD 验证三字段、idempotency 和 reason；不直接编辑内部 task ledger。
- **Red 预期失败原因**：当前 task ownership/identity 由 coordinator 一次注册且全程 open，没有 DAG node 映射。
- **Execution note**：test-first——先写 materialize 幂等、identity 冲突与上游失败投影的 scenario（Red），再调整 preset task 编排契约。
- **最小实现范围**：复用现有 `ralph tools task` API；不新增 task status。
- **集成验证**：隔离 activation + `ralph tools task list`/`show` 输出（禁直读 `.ralph/agent/tasks.jsonl`）；回归入口 `cargo nextest run -p ralph-core --test scenarios -- task_planner`。通过判据：一个 node 恰好一个 task、重放不重复创建、上游最终失败的 downstream task reason 为 `upstream_dependency_failed`。
- **回归范围**：task three-field、外层 hat env、重复 activation。
- **完成标准**：一个 node 对应一个 task；重放不重复；静态/动态状态分权明确。
- **风险与注意事项**：禁止 task-planner 直接读取或写 `.ralph/agent/tasks.jsonl`。

### Unit 5：实现依赖感知的迭代 Exec waves

- **Unit 目标**：只 dispatch 当前 ready nodes，并在每次 fan-in/integrate 后解锁下一波。
- **对应 Scenario**：B3。
- **外部可观察结果**：独立 nodes 同 wave 并发；依赖/冲突 nodes 分波；失败后下游不启动。
- **输入与输出**：输入静态 DAG + task API 动态状态；输出一次 batch `exec.unit.ready` wave 或结构化完成/失败 handoff。
- **可依赖的已完成能力**：Unit 3 DAG 与 Unit 4 task 映射。
- **明确禁止依赖的未来能力**：不依赖 Review/Fix/删除 hats。
- **验收测试**：3 independent batch、diamond DAG、file conflict、multi-wave unlock、upstream failed/timeout/cancel、idempotent replay。
- **需要拆分的单元测试**：ready predicate、single batch construction、identity fields、downstream `upstream_dependency_failed` reason、all-done detection。
- **Red 预期失败原因**：当前 planner 一次性发所有 Unit；需跨 worktree 集成验收的依赖（原文称“U5 类”，指输入计划中的示例 Unit，非本计划的 Unit 5）会落不同 worktree 并无法独立验收。
- **Execution note**：test-first——先写 ready-wave batch emit、diamond/文件冲突分波与失败传播的 scenario（Red），再调整 coordinator/task-planner/exec-integrator 的 preset contract。
- **最小实现范围**：调整 coordinator/task-planner/exec-integrator 的 preset contract；不修改 dispatcher。
- **集成验证**：真实 workflow guard scenario（`run_workflow_guard_scenario`，禁用 `run_scenario` stub）断言事件顺序、共享 wave ID、wave_total 和未启动节点；回归入口 `cargo nextest run -p ralph-core --test scenarios -- supervisor`。通过判据：每个 ready wave 仅一次 batch emit、独立 nodes 同 wave 并发、上游失败后下游零启动。
- **回归范围**：exec.wave.complete/failed consumer、merge resources、task close、单事件预算。
- **完成标准**：每个 ready wave 一次 emit；无依赖节点提前执行；失败路径有 reporter consumer。
- **风险与注意事项**：integrator 使用 runtime success resources，不读取内部 DB；merge conflict 视为结构化失败。

### Unit 6：把 tester 与多维 reviewers 收敛为并发只读 Review wave

- **Unit 目标**：在已合并 immutable commit 上并发执行测试验证与多维评审。
- **对应 Scenario**：B4。
- **外部可观察结果**：tester + reviewers 同一 Review wave、同一 commit、shared-readonly；任一 required failure 阻止通过。
- **输入与输出**：输入 source plan、execution DAG、merged commit/diff、前序证据；输出每维 `review.unit.done|failed` 和 aggregate review result。
- **可依赖的已完成能力**：Unit 5 完成全部 Exec waves 并产生 merged reference。
- **明确禁止依赖的未来能力**：不依赖 Fix wave 或删除 hats；tester 不调用修复。
- **验收测试**：payload context parity、并发 slots、review 前后 git 写集 fingerprint、test failure、review failure、missing context fail-close。
- **需要拆分的单元测试**：review payload semantic fields、required dimensions、aggregate all-required rule、immutable ref equality、baseline/diff comparison。
- **Red 预期失败原因**：当前 review 是六维批次但 tester 不是明确独立 dimension，测试/评审输入与权限合同不足。
- **Execution note**：test-first——先写 context parity、写集 fingerprint fail-close 与 required 聚合的 scenario（Red），再收敛 review 拓扑为单一 Review wave。
- **最小实现范围**：复用 `WaveKind::Review`；不新增 Test kind/topic family。
- **集成验证**：`run_workflow_guard_scenario` 真实 EventLoop（不得使用 `run_scenario` stub）；回归入口 `cargo nextest run -p ralph-core --test scenarios -- supervisor`。通过判据：tester 与各 review dimension 读同一 merged commit、review 前后写集 fingerprint 变化被检测并 fail-close（不自动清理共享 workspace）、任一 required slot 失败则 review 不 pass。
- **回归范围**：review wave fan-in、review synthesizer、shared-readonly isolation、timeout/failure。
- **完成标准**：测试与评审实际并发；全部读取同一 commit；任何写集变化被 synthesizer 检测、保留证据并 fail-close；aggregate 证据完整。
- **风险与注意事项**：`SharedReadonly` 不是 OS sandbox；本计划通过 hat scope + 前后 fingerprint 检测并 fail-close，但绝不自动清理共享 workspace。tester 的测试失败是 finding/failure 证据，不允许它改测试或生产代码。

### Unit 7：收敛为唯一正式 Fix 链与串行 alignment

- **Unit 目标**：所有 must-fix 只经正式 fix planner/worker/integrator；无 fallback fixer。
- **对应 Scenario**：B5。
- **外部可观察结果**：有 findings 时唯一正式 Fix 链按依赖产生串行波次且每波内部并发；无 findings 直接 alignment；alignment 不写代码。
- **输入与输出**：输入 review synthesis；输出 fix DAG、fix wave results、integrated commit、alignment outcome。
- **可依赖的已完成能力**：Unit 6 aggregate review evidence。
- **明确禁止依赖的未来能力**：尚不删除旧 hats；测试须证明新路径不调用它们。
- **验收测试**：no-fix、independent fixes、dependent/conflicting fixes、fix failure/exhausted、alignment pass/fail。
- **需要拆分的单元测试**：fix readiness、batch emit、all-findings traceability、alignment routing。
- **Red 预期失败原因**：当前存在正式 fix chain 与 fallback fixer 双路径，失败可能转 progress-steward。
- **Execution note**：test-first——先写唯一正式 Fix 链、no-fix 跳过与 alignment 只读的 scenario（Red），再调整 supervisor preset 的 fix/alignment 路由。
- **最小实现范围**：只调整 supervisor preset 的 fix/alignment 路由；不复制 pipeline preset 文本。
- **集成验证**：fix workflow guard scenario（真实 EventLoop）+ `cargo nextest run -p ralph-cli --test integration_supervisor_primary`（contract-double 用例）。通过判据：每个修复可追溯到 review finding、fallback fixer 零激活、alignment 只读且失败进入 reporter。
- **回归范围**：fix.wave.complete/failed、merge+tests、review finding traceability。
- **完成标准**：每个代码修复可追溯到 finding；只有 fix-worker 写代码；alignment 失败进入 reporter。
- **风险与注意事项**：不要让 alignment 形成第二 fixer；修复耗尽必须有限终止。

### Unit 8：原子删除 progress-steward、shipper 和 fallback fixer

- **Unit 目标**：删除三 hats 及所有残留引用，同时让 reporter 成为成功/失败唯一报告与终止 owner。
- **对应 Scenario**：B6。
- **外部可观察结果**：hat set 不含三者；所有业务路径可达 reporter；无 dead-end topic；成功/失败各唯一 LOOP_COMPLETE。
- **输入与输出**：输入各阶段 success/failure handoff；输出报告和唯一终态。
- **可依赖的已完成能力**：Unit 2–7 已替代三 hats 的职责。
- **明确禁止依赖的未来能力**：不把清理债务留给 Unit 9；本 Unit 必须同步 preset、schema、lint、BDD、权限、flow、state projection 和文档。
- **验收测试**：正常、planner invalid、exec failed、merge conflict、review failed、fix exhausted、timeout、cancel、alignment failed、reporter write failure；最后一种断言单次写入、`report_written=false` 的唯一 LOOP_COMPLETE 和无再次激活。
- **需要拆分的单元测试**：topic reachability、hat ownership、consumer uniqueness、terminal uniqueness、deleted-name absence as structured hat IDs（允许结构测试，不锁 prompt 文案）。
- **Red 预期失败原因**：当前三 hats 存在，`plan.blocked` 无 consumer，shipper/reporter multi-consumer 可能被 fallback 抽干。
- **Execution note**：characterization-first——先以 characterization 固化当前三 hats 拓扑与 `plan.blocked` 无消费者现状（Red 基线），再执行原子删除至 Green；不保留兼容 alias。
- **最小实现范围**：一次原子 topology migration；不要保留兼容 alias。
- **集成验证**：三个 preset lint 命令（`cargo nextest run -p ralph-cli --bin ralph -- preset_lint` + `cargo nextest run -p ralph-core -- preset_lint` + `cargo nextest run -p ralph-cli --bin ralph -- presets`）+ supervisor BDD failure matrix（`cargo nextest run -p ralph-core --test scenarios -- supervisor`）。通过判据：三个已删除 hat 零结构引用、无 dead-end topic、成功与全部失败路径均由 reporter 单一终止。
- **回归范围**：event policy schemas、required fields、topic deny rules、mechanism.flow、workflow activation、ownership/state projection。
- **完成标准**：无任何 deleted hat 引用；正常时 reporter 先写报告再终止；报告写失败时以 LOOP_COMPLETE payload 保存最小失败报告并一次终止，不重试风暴。
- **风险与注意事项**：本 Unit 用真实 EventLoop 证明 `plan.blocked` 能激活 reporter；Plan A 联合门禁证明最终外部状态仍为失败。产品语义已经冻结，禁止换用另一 failure topic。

### Unit 9：Full-chain 验收、operator skill/文档同步与全量门禁

- **Unit 目标**：证明重构 preset 在独立 runtime contract fixture 上完整，并为合并后联合 E2E 做准备。
- **对应 Scenario**：全部 B scenarios。
- **外部可观察结果**：复杂 DAG 正确分波；tester/reviewer 并发；唯一正式 Fix 链；成功/失败都有报告和 LOOP_COMPLETE。
- **输入与输出**：输入代表性 plan fixtures；输出 execution artifact、事件序列、报告和 exit status。
- **可依赖的已完成能力**：Unit 1–8。
- **明确禁止依赖的未来能力**：targeted preset tests 使用 Unit 1 冻结 contract fixture，不要求 Plan A 分支代码；真实 worker 的空结果、public/store ID 恢复、单终态竞态与 orphan 路由只在两分支合并后的联合 E2E 执行。
- **验收测试**：blocking full chain、invalid cycle、exec partial failure、review/test failure、fix path、cancel；测试用真实 EventLoop runner。
- **需要拆分的单元测试**：不新增业务实现；只补 integration 暴露的遗漏。
- **Red 预期失败原因**：若失败，必须回到对应 Unit 修正，不通过 mock runtime success 或削弱报告断言绕过。
- **Execution note**：test-first（Outside-In）——先写 full-chain 与失败矩阵 BDD（Red），再补齐 integration 暴露的遗漏；本 Unit 不新增业务实现。
- **最小实现范围**：同步 schema、operator skills、CLAUDE/AGENTS 和规则文档；只有 builtin 名称变化才更新 zsh completion。
- **集成验证**：
  - `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
  - `cargo nextest run -p ralph-core -- preset_lint`
  - `cargo nextest run -p ralph-cli --bin ralph -- presets`
  - `cargo nextest run -p ralph-core --test scenarios`
  - `cargo nextest run -p ralph-cli --test integration_supervisor_primary`（Plan B 分支只运行 contract-double/preset topology 用例；带真实 worker 的用例标记为联合门禁并在合并后运行，但不得用 skip/ignore 隐藏，需通过测试过滤名称明确分组）
  - `./scripts/run-tests.sh`
- **回归范围**：所有 builtin preset strict lint、schema parity、WAC、workflow activation、supervisor scenarios。
- **完成标准**：operator author/review skill 了解新拓扑和 lint；CLAUDE.md/AGENTS.md 完全一致；pipeline yml git diff 为零。
- **风险与注意事项**：禁止精确锁定 YAML instructions；测试结构化 topics、schemas、events 和真实行为。

## Verification Contract

### 风险驱动测试

- Characterization：旧 hats、全量一次发波、blocked dead-end。
- Contract：preset 对冻结 runtime Exec/Review/Fix 契约。
- State-machine：DAG ready/blocked/done、阶段路由、terminal。
- Idempotency/Concurrency：planner replay、单 batch wave、重复 fan-in。
- Differential：原 `ce-executor-pipeline.yml` 仅作为人工/结构对齐参考；不得 byte-lock，不得修改。
- Fault Injection：cycle、missing plan、slot failure、merge conflict、timeout、cancel、reporter write failure。

### Outside-In 执行顺序

1. 先以 full-chain/failure BDD 描述 operator 可见行为。
2. 再发现 artifact、DAG、ready-wave、review aggregation、reporter routing 能力。
3. 每个能力用最小单元 Red→Green→Refactor。
4. 回到真实 EventLoop scenario 证明协作。
5. 最后才跑 CLI full-chain 与 workspace 全量。

## 6. 最终质量门禁

- 所有计划内 Scenario 通过。
- 所有 artifact schema、DAG 行为、task mapping、ready-wave 和 route 测试通过；不虚构不存在的 parser 单元层。
- 所有必要的 preset lint、schema parity、workflow activation 和 contract tests 通过。
- Plan B 分支上的成功、非法计划、exec failure、review/test failure、fix failure contract-double E2E 通过；两分支合并后对应真实 worker E2E 通过。
- tester/reviewer 的只读权限由实际行为测试证明。
- `cargo fmt --check`、`cargo clippy`、`cargo build` 通过。
- `./scripts/run-tests.sh` 通过。
- 没有新增失败、skip、ignored、`.only` 或无解释 golden 更新。
- `progress-steward`、`shipper`、fallback `fixer` 无任何结构残留。
- `presets/en/ce-executor-pipeline.yml` 未修改。
- `CLAUDE.md` 与 `AGENTS.md` 完全一致。
- 未验证内容和剩余风险明确。

## Definition of Done

- task-planner 成为真实依赖编排者，不是一次性广播器。
- Exec、Review（tester + reviewers）和 Fix 阶段内部真正并发，阶段之间按已合并 artifact 串行。
- 原始 plan 保持 SSOT，execution artifact 可审计且不与 task API 双写状态。
- 三个不需要的 hats 完全删除。
- reporter 对成功与失败都能产出报告并有限结束 loop。
- 本计划 targeted/contract tests 可在独立分支通过。
- 与 runtime P0 分支合并后，运行共同真实 supervisor 主路径、失败路径、污染 env 路径和全量回归，通过后才能发布。
