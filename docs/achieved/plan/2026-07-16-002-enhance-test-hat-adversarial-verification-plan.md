---
title: 增强 Test Hat 的独立缺陷验证与 Sub-agent 协作
date: 2026-07-16
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

# 增强 Test Hat 的独立缺陷验证与 Sub-agent 协作

## Goal Capsule

- **目标：** 在不改变现有 hat 拓扑、事件 topic 和稳定化职责的前提下，为两个 CE executor preset 的 `test-stabilizer` 增加需求—测试追踪、稳定失败测试优先、风险驱动主动探索三项能力，并用受控 sub-agent 协作收集独立证据。
- **执行边界：** 仅调整 Test Hat instructions、对应 AAF 作者说明和证明这些约束有效的结构化测试；不新增 hat、不引入 Test Hat 自循环、不扩展为通用测试编排引擎。
- **权限边界：** Sub-agent 不得 commit、不得 emit Ralph 事件、不得决定 `stabilization.done`；主 Test Hat 独占范围裁决、写入验收、修复归因、commit、全量门禁和最终 emit。
- **停止条件：** 本次 `work.done.completed_units` 中任一 Unit 的关键验收条件无法建立明确测试证据、生产缺陷无法先形成稳定复现且无合理例外说明、或主动探索发现未收敛高风险缺陷时，必须发布 `stabilization.blocked`。
- **交付方式：** 保持现有 `stabilization.done` / `stabilization.blocked` schema；新增证据写入既有 `stabilization_audit_file`，避免扩大下游事件契约。

## Product Contract

### Problem Frame

当前 `test-stabilizer` 擅长对已有失败做归因、最小修正和全量测试，却没有明确要求主动证明当前 Unit 的 BDD/ATDD 覆盖、先用稳定失败测试复现生产缺陷，或针对实际 diff 选择高风险缺陷假设。结果可能是“现有测试全绿”被误当成“当前 Unit 已被充分验证”。

### Requirements

- R1. Test Hat 必须以 trigger 中的 `completed_units` 作为本次验证范围，逐 Unit 测试其与已完成前置 Unit 的边界；`planned_units` 中尚未完成的 Unit 视为未来范围，不得依赖，也不得扩大业务范围。
- R2. Test Hat 必须为本次每个 completed Unit 建立需求—测试追踪：逐项核对 BDD Scenario、ATDD 验收条件、对应测试、关键断言和真实执行路径；关键验收条件缺少有效证据时不得发布 `stabilization.done`。
- R3. 发现生产缺陷时，Test Hat 必须优先形成一个修复前稳定失败、修复后通过的自动化回归测试，并保存前后证据；只有无法安全自动化或代价明显失衡时才允许例外，且必须在 audit 中说明。
- R4. Test Hat 必须根据当前 Unit 和 diff 选择最高价值的 1–3 个缺陷假设进行主动验证；风险类型按适用性选择，不机械遍历非法输入、边界、状态、幂等、并发、超时、权限等完整清单。
- R5. Test Hat 使用受控 sub-agent 协作：最多两个只读 scout 可并行分析追踪缺口与风险；需要编写复现测试时至多启用一个 test-worker，且 build/test 工作不得并行。
- R6. Sub-agent 只能返回证据、候选风险、测试改动和执行结果，不得 commit、不得 emit、不得修改 `.ralph/` runtime ledger，也不得自行宣布 Unit 可关闭。
- R7. 主 Test Hat 必须审查 sub-agent 结果，拒绝越界或弱化 Oracle 的改动，并将采用与拒绝的证据写入既有 `stabilization_audit_file`。
- R8. 两个 preset 的行为、措辞和 AAF/Payload Contract 说明必须保持一致；sub-agent 能看到什么、能调用什么、何时停止必须从单 activation 视角表达。

### Key Flow

1. 主 Test Hat 从 trigger 的 `completed_units`、规范化计划和 trace 中确定本次逐 Unit 验证范围、依赖边界及其 BDD/ATDD 契约；不凭 prompt 文本猜测 Unit 状态。
2. 主 Test Hat可并行派发一个 traceability scout 和一个 risk scout；两者只读，分别返回覆盖缺口和按影响排序的缺陷假设。
3. 主 Test Hat 合并结果，只选择最高价值的 1–3 个风险，并决定已有测试是否足以验证。
4. 若需要新的复现测试，主 Test Hat 只派发一个 test-worker；该 sub-agent 可修改测试和运行 focused tests，但不得修改生产代码、commit 或 emit。
5. 主 Test Hat 验收修复前失败证据，再按现有归因与 correction 协议进行最小测试或生产修正，完成 focused、相关回归和权威 full-suite。
6. 主 Test Hat 将追踪矩阵、风险选择、复现前后证据、sub-agent 结果和剩余风险写入 audit，再独占最终 policy-check 与 emit。

### Acceptance Examples

- AE1. 某 ATDD 条件没有对应测试或只有无效断言时，即使现有 full-suite 全绿，Test Hat 也不得直接成功；补充有效测试并通过后才可发布 `stabilization.done`。
- AE2. 风险 scout 提出八个候选场景时，主 Test Hat 只选择与当前 Unit 直接相关且影响最高的 1–3 个，并在 audit 中记录选择依据，不把所有场景机械转成测试。
- AE3. Test-worker 为生产缺陷提交测试改动后，主 Test Hat 必须先确认该测试在修复前稳定失败，再允许生产修正；修复后同一测试和相关回归均通过。
- AE4. 无法安全自动化复现时，audit 记录例外原因、替代证据和剩余风险；若替代证据不足则发布 `stabilization.blocked`。
- AE5. 任一 sub-agent 尝试 commit、emit、修改生产代码或扩大当前 Unit 范围时，主 Test Hat 拒绝其结果并记录越界；该 sub-agent 结果不能成为成功依据。

### Scope Boundaries

- 不新增独立 runtime Test Agent、事件 topic、review round 或 retry engine。
- 不让 `dim:testing` 获得写权限；它继续作为下游只读独立复审，检查 Test Hat 的 audit、新增测试和 Oracle 强度。
- 不要求 Test Hat 使用全部测试技术，也不设置机械覆盖率阈值。
- 不改变 Executor/Fixer 对各自 Unit 测试完整性的责任；Test Hat 不是上游测试债务的默认接收者。

## Planning Contract

### Key Technical Decisions

- KTD1. **证据增强优先于 schema 扩张。** 三项新增能力先落入 `stabilization_audit_file` 的固定章节，继续使用现有成功/阻塞事件字段；只有结构化 runtime 门禁无法由现有字段表达时才重新评估 schema。
- KTD2. **两类只读 scout + 一个串行 test-worker。** Traceability scout 与 risk scout 可并行读取，但任何 build/test worker 同时最多一个，符合当前 prompt 的 sub-agent 限制并避免共享工作树竞态。
- KTD3. **主 Test Hat 保持唯一 authority。** Sub-agent 不参与事件输出和 commit；主 Test Hat负责选择风险、验收失败证据、接受 diff、生产修正和最终结论，避免把 isolated hat 内部协作误建成第二套 orchestrator。
- KTD4. **以行为证明 instructions，而非锁定文案。** 不增加检查 YAML/prompt 是否包含某句话的测试；优先通过 preset lint/AAF 结构、真实 EventLoop 路由和可执行 fixture 证明边界仍成立。

### Existing Patterns

- `presets/en/ce-executor-pipeline.yml` 的 Executor/Fixer 已采用“sub-agent 修改、主 hat review/commit/emit”的 authority 模式，Test Hat 应复用该模式但缩小到测试证据收集。
- `crates/ralph-core/src/instructions.rs` 已规定搜索可并行、build/test sub-agent 同时最多一个；Test Hat instructions 必须与该运行时注入保持一致。
- `skills/ralph-preset-author/SKILL.md` 与 `skills/ralph-preset-common/references/author-checklist.md` 要求从单 hat activation 视角记录 AAF 五问和 Payload Contract。
- `crates/ralph-core/tests/scenarios/ce_executor_pipeline_stabilization_blocked_report.yml` 等现有真实 EventLoop 场景提供成功/阻塞路由模式；新测试只补行为缺口，不重复拓扑覆盖。

### Audit Contract

既有 `stabilization_audit_file` 增加以下固定内容，不新增新的独立报告文件：

- `completed_units` 中逐 Unit 的验证范围、允许依赖的前置 Unit 和明确排除的未完成 Unit；
- Scenario/ATDD、对应测试、关键断言、执行路径和结果的追踪表；
- 两个 scout 的候选项、主 Test Hat 选中的 1–3 个风险及选择依据；
- test-worker 的任务边界、改动文件、修复前失败证据、修复后结果；
- 被主 Test Hat 拒绝的越界或弱证据及理由；
- focused、相关回归、full-suite 命令与结果，以及剩余风险。

## Implementation Units

### U1. 增强两个 Test Hat instructions 与 AAF 契约

- **Goal:** 把三项测试能力和受控 sub-agent 流程融合进两个 preset 的 `test-stabilizer`，保持现有事件拓扑、schema 和稳定化成功定义不变。
- **Files:** `presets/en/ce-executor-pipeline.yml`、`presets/en/ce-executor-pipeline-loop.yml`、`presets/en/ce-executor-pipeline-preset-author-notes.md`、`presets/en/ce-executor-pipeline-loop-preset-author-notes.md`。
- **Approach:** 在现有 Read context 后加入 Unit 范围与追踪；在 failure attribution 前加入两个只读 scout 的受控派发与风险收敛；在 minimal corrections 前加入单 test-worker 的稳定失败证据门；扩展 audit 内容。明确所有 sub-agent 禁止 commit/emit/生产修改，主 Test Hat 必须 review 后才能采用结果。
- **Test Scenarios:**
  - `completed_units` 中任一 Unit 的关键 ATDD 无有效测试时，instructions 要求阻塞或补齐，不能以 full-suite 绿直接放行。
  - 两个只读 scout 可并行返回候选证据，但风险选择被限制为与当前 Unit 相关的 1–3 项。
  - test-worker 同时最多一个，且只有修复前稳定失败证据被主 Test Hat 验收后才能进入生产修正。
  - sub-agent 越权 commit/emit/生产修改时，主 Test Hat 明确拒绝结果并记录证据。
  - 无法自动化复现时存在有界例外；替代证据不足时走 `stabilization.blocked`。
- **Skill/doc sync:** 本变更只改变 builtin preset hat 工作流，不新增 CLI、event 或 runtime 能力；检查 `crates/ralph-core/data/ralph-tools*.md` 后预计无需修改，但必须在实现后反向确认。更新两份 preset author notes 的 AAF 与 sub-agent 可见性说明；若 operator checklist 无法表达 sub-agent authority，再最小更新 `skills/ralph-preset-common/references/author-checklist.md`。
- **Dependency:** 无。

### U2. 增加结构化验收与真实回归验证

- **Goal:** 证明增强没有改变路由、没有允许 Test Hat 自批或绕过 policy-check，并确保两份 preset 保持一致。
- **Files:** `crates/ralph-cli/src/presets.rs`、`crates/ralph-core/tests/scenarios/*.yml`、`crates/ralph-core/tests/scenarios.rs`，以及确有必要时的 `presets/schemas/ce-executor-pipeline-loop.yml`。
- **Approach:** 优先扩展现有结构化 preset 测试和一条最接近的真实 EventLoop scenario；不对 instructions 精确文本或整份 preset 做 byte-equality 断言。若没有 runtime 字段变化，不修改 schema；若实现中决定新增机器可判定字段，则同时同步线性 preset schema、loop schema SSOT、field_docs、BDD payload 和下游消费者。
- **Test Scenarios:**
  - `work.done → test-stabilizer → stabilization.done → review` 的既有顺序保持不变。
  - 追踪缺口或稳定复现证据不足导致 `stabilization.blocked` 后，不产生 review/fix/align 事件。
  - Test Hat 仍只有 `stabilization.done` / `stabilization.blocked` 发布权限，sub-agent 不形成新 topic 或旁路。
  - 两个 builtin preset 均可解析并通过 strict lint，author notes 的 hat 数量和 Payload Contract 覆盖保持一致。
  - 项目注入的“build/test sub-agent 同时最多一个”约束与 Test Hat instructions 不冲突。
- **Evidence limit:** EventLoop mock 只能证明事件路由与阻塞结果，不能单独证明 agent 会忠实执行自然语言 instructions；三项工作纪律由 AAF/Payload Contract 审查、受控 mock 场景和真实运行 audit 抽检共同验证，不伪造一条“文案存在即行为正确”的测试。
- **Dependency:** U1。

## Verification Contract

| Gate | Command | Covers | Pass signal |
|---|---|---|---|
| 线性 preset lint | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | U1, U2 | 线性与 builtin preset lint 全部通过 |
| Core preset lint | `cargo nextest run -p ralph-core -- preset_lint` | U1, U2 | schema、ownership、workflow activation 无 finding |
| Embedded preset parity | `cargo nextest run -p ralph-cli --bin ralph -- presets` | U1, U2 | manifest、embedded preset 与 strict lint 通过 |
| EventLoop BDD | `cargo nextest run -p ralph-core --test scenarios` | U2 | 使用真实 runner 的相关 scenario 通过且事件顺序正确 |
| CLI 文档漂移 | `scripts/check-cli-doc-drift.sh` | U1 | 注入 skill 与命令引用无漂移；若无 CLI 变化应保持绿 |
| Workspace baseline | `./scripts/run-tests.sh` | U1, U2 | nextest 全量与 doctest 通过；仅竞态/时序 flake 才使用项目规定的串行 fallback |

实现期间先运行 targeted nextest；完成前必须运行 `./scripts/run-tests.sh`。禁止裸跑 `cargo test -p ralph-cli`。

## Definition of Done

- 两个 `test-stabilizer` 都明确执行当前 Unit 的需求—测试追踪、稳定失败测试优先和最高价值 1–3 项风险探索。
- Sub-agent 机制保持有界：最多两个并行只读 scout、同时最多一个 test-worker；所有 commit、production 修正、policy-check 和 emit 均由主 Test Hat 完成。
- `stabilization_audit_file` 能追溯 Scenario/ATDD、风险选择、sub-agent 证据、修复前后测试及剩余风险。
- 未新增不必要的 event/schema/runtime 抽象，也未把 Test Hat 变成自循环或通用测试编排器。
- 两份 preset author notes 与实际 hat instructions 一致；`crates/ralph-core/data/ralph-tools*.md` 和 preset operator skills 已完成反向准确性检查。
- 结构化 preset 测试、真实 EventLoop BDD、drift 检查和全 workspace 基线全部通过。
