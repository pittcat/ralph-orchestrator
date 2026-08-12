---
date: 2026-07-29
topic: parallel-forge-wave-settlement-and-evidence-gates
status: requirements-draft
artifact_contract: ce-unified-plan/v1
artifact_readiness: requirements-only
product_contract_source: ce-brainstorm
execution: code
related:
  - presets/en/parallel-forge.yml
  - presets/schemas/parallel-forge.yml
  - presets/en/ce-executor-pipeline.yml
  - presets/templates/ce-executor-pipeline/fail-confidence-rubric.template.md
  - presets/templates/ce-executor-pipeline/settlement-evidence.template.md
  - presets/templates/ce-executor-pipeline/README.md
---

# Parallel Forge 依赖 Wave 结算、冲突决策与失败证据门禁需求

## Goal Capsule

- **目标**：让 `parallel-forge` 在 `supervisor + wave` 模型下正确执行多层 Unit DAG，使下游 Unit 只基于已经审查、集成并通过增量验证的前置代码启动，同时让执行、集成和验证失败都必须经过可复核证据与独立决策门禁。
- **产品权威**：操作者提供的需求或计划、Planner 生成的 Unit DAG、每个 Unit 的验收标准，以及最近一次通过增量验证的 integration branch 共同限定执行事实；agent 不得自行改写依赖、验收标准或失败语义。
- **开放阻塞项**：Integrator 的冲突修复权限、Verifier/Tester 的源码修改权限、验证失败后的自动修复角色尚未最终确认，进入实施规划前必须解决。

## Product Contract

### Summary

`parallel-forge` 应按 DAG 的“当前可执行前沿”分轮运行。每轮一次性派发全部 ready Unit，等待整个 wave fan-in 后进行批量审查、串行集成和一次增量验证；只有该轮结算通过，下一依赖层才可启动。

失败和冲突不得靠 agent 主观判断直接发事件。相关 hat 必须先填写统一模板、引用上一轮相关失败经验、按同一 rubric 自评，再由独立 precheck 复核。

### Problem Frame

当前 `parallel-forge` 已具备 Unit DAG、task dependency、supervisor slot、wave fan-out、隔离 worktree、串行 Integrator、Verifier 和 Tester，但这些能力尚未形成完整的“依赖代码可见性”闭环。

Dispatcher 当前把“task 为 open 且所有依赖 task 为 done”视为 ready。`exec.unit.done` 会关闭对应 task，但该 Unit 此时尚未经过 Reviewer、Integrator 和 Verifier。因此，task `done` 只证明 Executor 已结束，不能证明其产物已经成为下游 Unit 可安全消费的基线。

Worktree hat 当前会在并行执行开始前创建各 Unit worktree。对于 `U2 depends_on U1`，U2 虽然会等待 U1 的 task 结束，但其 worktree 仍可能来自 U1 完成前的旧 `base_commit`。这造成“调度上等待了依赖，代码上却看不到依赖”的错位，并把缺失依赖和文本冲突推迟到最终集成阶段。

当前 Reviewer、Integrator 和 Verifier 都位于所有开发 wave 完成之后。这种一次性收尾适用于互不依赖的叶子 Unit，却无法可靠支撑 `U1 → {U2,U3} → U4` 这类多层 DAG，因为 U4 开发前必须看到已经验证过的 U2 和 U3。

Integrator 当前遇到 rebase 或 merge conflict 后，只需写一份自由格式失败记录并发 `work.failed`。流程没有规定冲突盘点、Unit 意图映射、允许自动解决的边界、测试证据、历史经验复用或独立复核。Verifier 和 Tester 的职责文案是验证，但目前也没有明确禁止修改测试或业务代码。

### Actors

- **Planner**：生成可审计的 Unit DAG、依赖关系、执行 wave、集成顺序、路径边界和验收条件。
- **Guardian**：审计 DAG 无环、并行路径隔离、共享资源唯一 owner 和并行安全性。
- **Worktree/准备角色**：只负责 integration branch、Unit branch 和 worktree 的 Git 准备，不实现业务 Unit。
- **Forge Dispatcher**：根据已结算依赖状态计算当前 ready frontier，并一次性派发当前 wave。
- **Executor**：在 supervisor 槽位的隔离 worktree 内实现一个 Unit，并产生成功或失败证据。
- **Reviewer**：在当前 wave 全部槽位 fan-in 后，批量但逐 Unit 独立审查该 wave。
- **Integrator**：按 `integration_order` 将当前 wave 中已批准的 Unit 串行集成到 integration branch。
- **Verifier**：在当前 wave 集成后，对该 wave 及其依赖链执行一次增量回归。
- **Tester**：所有 Unit wave 均结算后，执行全量测试、lint、build 和 CI 等价门禁。
- **Precheck gate**：使用与生产者相同的模板和 rubric 独立复核失败、冲突及结算声明。
- **Reporter**：消费最终成功、阻塞或证据充分的失败结论，生成经理可读报告。

### Key Decisions

- **执行模型固定为 `supervisor + wave`。** (session-settled: user-directed — chosen over changing the execution model: supervisor 与 wave 是 Parallel Forge 不可改变的基础能力。) Governs R1–R3.
- **依赖表示“已集成且增量验证通过后可消费”，不只是 Executor 已完成。** (session-settled: user-directed — chosen over task-done-only dependency release: 下游 Unit 必须真实看到前置 Unit 的代码。) Governs R4–R10.
- **审查以当前 wave 为批次，在整个 fan-in 后发生。** 这不是每完成一个 Unit 就中断并行执行。Governs R7.
- **失败结论必须 evidence-first。** 生产者自评和 precheck 他评共享同一份 rubric，避免两套标准漂移。Governs R17–R28.
- **本需求阶段只定义行为与验收边界。** 不在本文件决定具体 topic 名、state projection 字段或 runtime 模块改法。

### Requirements

**执行模型与 DAG 语义**

- **R1.** `parallel-forge` 必须继续使用 `supervisor + wave`；不得降级为 single-chain、纯 wave 或纯 supervisor。
- **R2.** Planner 产出的 Unit DAG 必须继续是依赖关系、并发分组和集成顺序的计划事实源，并接受 Guardian 的无环与并发安全审计。
- **R3.** 每轮调度必须把当前所有符合 ready 条件的 Unit 作为一个逻辑 wave 一次性提交给 supervisor；运行时并发槽位上限只影响实际并发数，不改变该批 Unit 属于同一结算 wave 的事实。
- **R4.** Unit 的 `exec.unit.done` 或 task `done` 只能表示执行产物已经产生，不得单独证明该 Unit 已可被下游依赖消费。
- **R5.** 下游 Unit 只有在所有 `depends_on` Unit 均已审查通过、集成完成并通过对应增量验证后才能进入 ready frontier。
- **R6.** Dispatcher 的 ready 判定必须能观察“已结算 Unit 集合”和最近一次验证通过的 integration base，不能仅依赖 task 状态。
- **R7.** 当前 wave 的所有 supervisor 槽位必须先 fan-in；随后 Reviewer 在一次 activation 中批量审查该 wave，并为每个 Unit 保留独立 verdict 和证据。
- **R8.** 只有当前 wave 的所有必需 Unit 审查通过后，Integrator 才能按 `integration_order` 串行集成该 wave。
- **R9.** 当前 wave 集成完成后，Verifier 必须执行一次覆盖该 wave 及其依赖链的增量验证；不得把多个尚未结算的依赖层累积到最后一次验证。
- **R10.** 只有 R9 通过后，当前 wave 才能标记为 settled，并允许计算下一 ready frontier。
- **R11.** 当所有 Unit 均 settled 后，Tester 才执行最终全量门禁；通过后才能进入 Auditor 和成功报告路径。

**Worktree 与代码可见性**

- **R12.** Worktree/准备角色不得实现 Foundation Unit 或其它业务代码；Foundation Unit 必须由 Executor 执行并经过同样的审查、集成和验证闭环。
- **R13.** 不得在首轮执行前基于同一旧 `base_commit` 提前冻结所有后续依赖 Unit 的有效工作基线。
- **R14.** 每个当前 ready Unit 在 Executor 启动前，其 worktree 必须基于最近一次增量验证通过的 integration branch HEAD 创建或安全更新。
- **R15.** Executor 启动前必须能复核 worktree 的 base commit，并证明该 base 已包含所有前置 Unit 的已结算提交。
- **R16.** 同一 wave 中按 Guardian 结论可并行的 Unit 应共享同一个已验证 base；并行 Unit 不得依赖同一 wave 内尚未完成的兄弟 Unit。

**强失败门禁**

- **R17.** Executor、Integrator、Verifier 和 Tester 的失败声明必须纳入强失败门禁；仅有自然语言 `reason` 不足以发布业务级失败。
- **R18.** 每个失败声明必须先落盘证据 artifact，再执行自评、policy-check 和正式 emit；event payload 只携带短摘要、评分、必要身份字段和证据路径。
- **R19.** 失败证据必须覆盖尝试记录、不同根因假设、完整因果链、替代原因排除、命令或测试输出以及未解决项。
- **R20.** 生产者与独立 precheck 必须使用同一份评分 rubric；precheck 必须独立重算，不能接受生产者自报分数作为事实。
- **R21.** 失败 confidence 达到 90 且 evidence coverage 达到 75 才具备发布失败结论的最低资格；满足数值阈值仍不得绕过任何硬性否决条件。
- **R22.** precheck 拒收必须返回稳定 `failed_checks`、具体证据缺口和下一步可执行动作，使原 hat 能补证、换假设或停止不安全尝试。
- **R23.** 通用失败门禁应参考 `ce-executor-pipeline` 的 `fail-confidence-rubric.template.md` 与 `settlement-evidence.template.md`，但不得把与 merge conflict 不匹配的重试要求机械照搬。

**Executor 与 supervisor 终态兼容**

- **R24.** `exec.unit.done` 和 `exec.unit.failed` 必须继续作为 supervisor 槽位的真实终态，不能因引入普通 `.proposed → precheck → final` 路由导致 supervisor 无法 fan-in。
- **R25.** Executor 发 `exec.unit.failed` 前必须完成本地模板、自评、必填字段、payload consistency 和 policy-check。
- **R26.** Supervisor 聚合出失败 wave 后，业务级 `work.failed` 必须由 failure handler 读取各槽位证据，再经过独立 precheck 才能发布。
- **R27.** 单个槽位失败和整个计划失败必须是两个不同判断；一个槽位失败不能在缺少 wave 聚合上下文时直接宣告整个计划失败。

**历史失败经验复用**

- **R28.** Executor、Integrator、Verifier 和 Tester 在准备失败结论前，必须查找与当前 Unit、命令、冲突或失败指纹相关的最近历史证据。
- **R29.** 历史经验必须按匹配依据分为“可复用、部分可复用、不适用”，并记录采用或拒绝原因；不得无条件复制旧结论。
- **R30.** 历史记录必须包含足够的代码基线和问题指纹，避免把旧 SHA、旧依赖或旧冲突位置当成当前事实。
- **R31.** 同一问题再次失败时，新证据必须说明相对上次增加了什么尝试、排除了什么假因或发现了什么变化；机械重复不得提高 confidence。

**集成冲突证据模板**

- **R32.** Parallel Forge 必须提供专用的 integration conflict evidence 模板，不能只使用自由格式 `integrator-failed.md`。
- **R33.** 冲突模板必须记录 integration base SHA、待集成 Unit、前置 Unit、集成顺序、真实 Git 冲突文件和冲突块、相关 Unit 报告与验收标准。
- **R34.** 每个冲突块必须映射到相关 Unit 的目标、acceptance criteria、shared contract 或可观察行为；无法建立映射时必须标记语义不确定。
- **R35.** 模板必须记录所有候选解决方案、采用方案、放弃其它方案的原因、修改路径、测试命令、关键输出和剩余风险。
- **R36.** 模板必须包含历史经验匹配章节，列出相同或近似冲突的证据路径、匹配字段以及本轮采用或拒绝的理由。
- **R37.** 模板必须包含自评区，逐项列出硬门结果、confidence、evidence coverage、计算过程和最终建议决策。

**冲突决策指标**

- **R38.** Git 报告的冲突文件盘点覆盖率必须为 100%；模板中记录的冲突文件集合必须与 Git 实际集合一致。
- **R39.** 允许继续集成前，未解决 conflict marker 数必须为 0。
- **R40.** 冲突解决涉及的修改路径必须 100% 位于相关 Unit `allowed_paths` 的并集内，且不得命中任何 `forbidden_paths`。
- **R41.** 冲突块到 Unit 意图、验收标准或 shared contract 的可追溯率必须为 100%；无法唯一解释的冲突块构成语义硬阻塞。
- **R42.** 冲突解决要求执行的受影响测试和回归命令，其执行覆盖率必须为 100%，已执行命令的通过率必须为 100%。
- **R43.** 自动继续要求证据 coverage 不低于 75、decision confidence 不低于 90，并完成历史经验检查。
- **R44.** 数值总分不得覆盖硬否决条件；存在未解决 marker、越界修改、测试失败、语义分歧或证据伪造时必须禁止继续。
- **R45.** 冲突 precheck 的稳定决策集合必须至少区分 `RESOLVED_CONTINUE`、`RETRY_WITH_NEW_HYPOTHESIS`、`SEMANTIC_BLOCKED` 和 `EVIDENCE_INSUFFICIENT`。
- **R46.** Integrator 的正式成功或失败事件必须携带冲突证据路径、rubric 结果和 precheck verdict；没有冲突时应明确记录 conflict count 为零，而不是省略集成事实。

**Verifier 与 Tester 的独立性**

- **R47.** Verifier 不参与 Git 合并；它只消费成功的集成结果并验证当前 wave。
- **R48.** Tester 不参与 Unit 集成；它只在所有 wave settled 后执行全量门禁。
- **R49.** Verifier 和 Tester 的验证结论必须基于真实命令、退出状态和日志，不能用“看起来正确”替代执行证据。
- **R50.** Verifier/Tester 是否可以修改测试或业务代码必须成为显式权限合同；当前“职责未授权但工具未禁止”的模糊状态不可保留。

**Artifact 与事件边界**

- **R51.** Unit 完成报告、wave review summary、integration log、commit map、增量验证报告、冲突证据和失败证据必须作为可恢复的业务 artifact 落盘。
- **R52.** Event 只承担控制和短 handoff，不得承载完整冲突分析、完整日志或长篇失败经验。
- **R53.** 下游 hat 必须通过 trigger 或 projection 中的 repo-relative 路径读取 artifact，不得依赖 runtime 内部 ledger 或猜测固定路径。
- **R54.** 每个 artifact 必须有明确生产者、消费者、当前 wave/Unit 身份、base SHA 和生命周期 owner。

### Conflict Decision Rubric

下表是需求级判定合同。最终模板排版和机器字段由实施规划决定。

| 指标 | 允许自动继续的阈值 | 硬否决 |
|---|---:|---|
| 冲突文件盘点覆盖率 | 100% | 漏记任一 Git 冲突文件 |
| 未解决 conflict marker | 0 | 任一 marker 残留 |
| `allowed_paths` 合规率 | 100% | 越界或命中 `forbidden_paths` |
| Unit 意图/验收可追溯率 | 100% | 存在无法唯一解释的冲突块 |
| 必需测试执行覆盖率 | 100% | 必需命令未执行 |
| 已执行测试通过率 | 100% | 任一必需测试失败 |
| 历史经验检查 | 已完成 | 未查找或无采用/拒绝说明 |
| evidence coverage | ≥75 | 可复核来源虚报 |
| decision confidence | ≥90 | 自评与证据不一致 |

### Key Flows

#### Flow A：多层 DAG 正常结算

```text
Planner 生成并由 Guardian 批准 Unit DAG
  → 初始化 integration branch
  → 读取已 settled Unit 与 verified base
  → 计算当前 ready frontier
  → 基于 verified base 准备本轮 Unit worktrees
  → 一次 wave 派发全部 ready Unit
  → 等待 supervisor 完整 fan-in
  → Reviewer 批量审查本 wave，各 Unit 独立 verdict
  → Integrator 按 integration_order 串行集成本 wave
  → Verifier 对本 wave 执行一次增量验证
  → 写入 wave settlement 与新 verified base
  → 有剩余 Unit：进入下一 frontier
  → 无剩余 Unit：Tester 全量门禁 → Auditor → Reporter
```

#### Flow B：Executor 槽位失败

```text
Executor 读取当前 Unit 与相关历史失败
  → 按模板记录尝试、因果链和排除项
  → 自评 confidence / evidence coverage
  → policy-check
  → 发 supervisor 槽位终态 exec.unit.failed
  → supervisor 完成 wave fan-in 并注入失败聚合
  → failure handler 汇总各槽位证据
  → 独立 precheck 复评
  → 通过：发布有根据的业务失败
  → 拒收：返回 failed_checks，不得生成无根据失败报告
```

#### Flow C：Integrator 遇到冲突

```text
Integrator 停在真实冲突状态
  → 盘点 Git 冲突集合并生成冲突指纹
  → 读取相关 Unit 报告、验收标准和历史冲突经验
  → 填写 conflict evidence 模板
  → 对候选决策执行 rubric 自评
  → 独立 precheck 复评
  → RESOLVED_CONTINUE：完成安全集成并交给 Verifier
  → RETRY_WITH_NEW_HYPOTHESIS：在安全边界内换角度
  → SEMANTIC_BLOCKED：保留证据并进入阻塞/修复路径
  → EVIDENCE_INSUFFICIENT：补证，禁止正式结算
```

#### Flow D：Verifier 增量验证失败

```text
Verifier 运行当前 wave 的增量门禁
  → 记录命令、退出状态、失败测试和代码基线
  → 查找相关历史失败
  → 排除环境、依赖、flake、baseline 和残留状态
  → 填写证据并自评
  → 独立 precheck
  → 证据充分：进入已确认的失败或修复路径
  → 证据不足：补跑诊断，不得随意发失败事件
```

### Acceptance Examples

- **AE1（Covers R4–R10）**：给定 `U2 depends_on U1`，当 U1 仅发出 `exec.unit.done`、尚未集成或验证时，U2 不得被派发。
- **AE2（Covers R5, R14–R16）**：当 U1 已集成并通过增量验证后，U2 的 worktree base 必须包含 U1 的最终集成提交。
- **AE3（Covers R3, R7）**：给定 U2、U3 同时 ready，它们被放入同一个逻辑 wave；Reviewer 只在该 wave 所有槽位终态 fan-in 后激活一次，并分别给出 U2、U3 verdict。
- **AE4（Covers R8–R10）**：给定 `U1 → {U2,U3} → U4`，U4 只能在 U2、U3 均审查、串行集成且本 wave 增量验证通过后启动。
- **AE5（Covers R12）**：Foundation Unit 不得由 Worktree hat 实现；它必须走 Executor、Reviewer、Integrator 和 Verifier 闭环。
- **AE6（Covers R24–R27）**：一个 Executor 槽位失败后，supervisor 能正常收敛该 wave；业务级失败只有在聚合证据通过独立 precheck 后出现。
- **AE7（Covers R28–R31）**：同一 Unit 的同类失败再次发生时，新证据明确引用上次记录，并说明本次新增尝试或为何旧经验不适用。
- **AE8（Covers R32–R46）**：Integrator 遇到两个 Git 冲突文件时，证据模板精确记录这两个文件、全部冲突块、意图映射、测试和评分；漏记任一文件时 precheck 必须拒收。
- **AE9（Covers R39–R44）**：即使 confidence 为 95，只要仍有 conflict marker、越界修改或必需测试失败，Integrator 都不得继续。
- **AE10（Covers R47–R50）**：Verifier 不执行 merge；当 integration 未成功完成时，Verifier 不被触发。
- **AE11（Covers R51–R54）**：loop 从中断恢复后，下游 hat 能通过事件中的 repo-relative artifact 路径恢复 wave、基线、审查、集成和验证上下文。

### Success Criteria

- **SC1.** 多层 DAG 中不存在“下游 Unit 已启动但 worktree 不包含前置 settled Unit”的路径。
- **SC2.** 同一 ready frontier 的 Unit 保持并行执行，审查发生在整个 wave fan-in 之后，而不是逐 Unit 打断。
- **SC3.** 每个 wave 都在解锁下一依赖层前完成批量审查、串行集成和一次增量验证。
- **SC4.** Worktree hat 不再承担任何 Foundation 或业务实现职责。
- **SC5.** Integrator 的冲突继续、重试或阻塞决策均可由模板、命令输出和 rubric 复核。
- **SC6.** Executor、Integrator、Verifier 和 Tester 无法在证据缺失或分数不达标时发布业务级失败。
- **SC7.** 相同或相近失败再次发生时，当前 evidence artifact 能显示上一轮经验如何影响本轮尝试和判断。
- **SC8.** Verifier 与 Tester 的合并、验证和修改权限没有隐式灰区。
- **SC9.** 所有新增事件与 artifact handoff 都能从单 hat 可见上下文构造，且不依赖 runtime 内部 ledger。

### Scope Boundaries

**本需求包含**

- `parallel-forge` 的多层 Unit DAG wave 结算语义。
- worktree 基线与已验证依赖的代码可见性。
- 当前 wave 的批量审查、串行集成和增量验证。
- Executor、Integrator、Verifier、Tester 的失败证据门禁。
- Integrator conflict evidence 模板、决策 rubric 和历史经验复用。
- 为支撑上述行为所需的 preset、schema、runtime 路由、BDD、operator skill 和 agent guide 同步要求。

**本需求不包含**

- 改变 `supervisor + wave` 执行模型。
- 让 Verifier 参与 Git 合并。
- 用 Runtime 自动编造失败原因、冲突决策或业务字段。
- 用事件 payload 替代完整 evidence artifact。
- 在需求阶段确定具体 Rust 模块、数据库表、CLI 参数或最终 topic 名。
- 为所有 preset 建立通用工作流引擎；本工作聚焦 `parallel-forge`，可复用机制由实施规划评估。

### Dependencies and Constraints

- `presets/en/parallel-forge.yml` 与 `presets/schemas/parallel-forge.yml` 是本需求直接影响的 preset/schema 对。
- `ce-executor-pipeline` 的 failure rubric 和 settlement evidence 模板是门禁行为参考，但 Parallel Forge 的冲突判定需要独立模板。
- `exec.unit.done` / `exec.unit.failed` 与 supervisor fan-in 的既有终态合同必须保持可运行。
- 任何 event topology、required fields 或 state projection 变化都必须遵守仓库的 preset/schema 下游同步清单。
- 如果 agent 可见命令、事件或操作流程变化，必须同步 `crates/ralph-core/data/ralph-tools*.md`。
- Preset operator author/review skill 必须同步新的 AAF、artifact-first、precheck 和 supervisor wave 审核规则。
- 行为验收必须覆盖真实 EventLoop/supervisor/wave 路径，不能只断言 YAML 或 prompt 文案包含特定文本。

### Outstanding Questions

#### Resolve Before Planning

- **O1 — Integrator 冲突修复权限**：Integrator 是只能证据化冲突并停止，还是可以解决证据充分且语义唯一的机械冲突；或者是否新增独立 `conflict-resolver`。
- **O2 — Verifier/Tester 修改权限**：两者是否都严格只读；如果需要自动修复测试或业务代码，是否新增独立 stabilizer/repairer。
- **O3 — 验证失败后的恢复路径**：Verifier 增量失败后是终止并报告，还是进入有界修复循环后重新审查、集成和验证。
- **O4 — Review 拒绝后的恢复路径**：当前 wave 中一个 Unit 被 Reviewer 拒绝时，是只返修该 Unit，还是整 wave 作废后重新派发。

#### Deferred to Planning

- **O5（Affects R6, R10）**：已 settled Unit 集合与 verified base 的状态由哪个 artifact 和 projection 组合承载。
- **O6（Affects R3）**：ready Unit 数超过 supervisor 并发槽位时，一个逻辑 wave如何分批占用槽位且保持一次 fan-in 结算。
- **O7（Affects R23）**：通用 rubric 的“1 次初始 + 3 次 retry”如何调整到 merge conflict，避免为了凑次数执行有风险的重复 Git 操作。
- **O8（Affects R28–R31）**：失败和冲突指纹使用哪些稳定输入，才能兼顾相似匹配与避免旧基线污染。
- **O9（Affects R45–R46）**：冲突 precheck 的最终事件 topic、payload 字段和 exhaustion 终态如何命名。
- **O10（Affects R24–R27）**：Executor 本地证据检查与 wave 聚合 precheck 如何共享 rubric，同时不阻塞 supervisor 槽位释放。
