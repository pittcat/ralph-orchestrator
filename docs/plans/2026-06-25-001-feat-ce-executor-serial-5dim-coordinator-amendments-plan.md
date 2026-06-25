---
title: "feat: ce-executor-serial 终审扩展为 5 维度并赋予 Coordinator 计划修正自主性"
type: feat
status: u9-closed-u10-pending
date: 2026-06-25
origin: docs/brainstorms/2026-06-25-ce-executor-serial-review-dimensions-and-coordinator-autonomy.md
---

# feat: ce-executor-serial 终审扩展为 5 维度并赋予 Coordinator 计划修正自主性

## Overview

将 `presets/en/ce-executor-serial.yml` 的终审维度从固定的 2 维度（`correctness` → `testing`）扩展到 5 维度：

```text
correctness → testing → maintainability → project-standards → adversarial
```

其中 `maintainability` 与 `project-standards` 直接复用 `ce-code-review` skill 的对应 reviewer persona 关注点，`adversarial` 作为最后一轮 Red-Team 审视，聚焦隐藏副作用、兼容性破坏、边界与并发、安全路径、命名误导、测试充分性、维护成本与回滚安全。

同时为 `coordinator` 增加受控的计划修正能力：在目标、验收标准、范围边界不变的前提下，允许它读取同一 plan 内的历史输出（`memories.md`、`decisions.md`、`fix-log.md`、`findings.md` 等），在 scratchpad 中生成 `plan-amendments.md`，对执行单元进行拆分、合并或重排，并通过现有的 `work.ready` payload 与 scratchpad 文件继续推进。原始 plan 文件保持只读，不引入新的 event topic 或 hat。

---

## Problem Frame

当前 `ce-executor-serial` 终审只覆盖 `correctness` 与 `testing`，容易漏掉结构债务、命名/抽象问题、AGENTS.md/CLAUDE.md 合规性问题，也缺少一轮专门的对抗性审视来捕捉隐藏副作用、边界漏洞与回滚风险。同时 Coordinator 严格按初始 plan 推进，遇到单元过大、顺序不当或同一 plan 内重复出现的问题模式时，只能阻塞或被动执行，缺少受控的修正机制。

本次改动只聚焦：
1. 补齐 `maintainability`、`project-standards`、`adversarial` 三个审查维度。
2. 给 Coordinator 一个有限的“同 plan 学习 + 计划修正”能力。

（详见 origin 文档的问题框架与关键决策。）

---

## Requirements Trace

- **R1** — `review-coordinator` 的固定维度序列改为 5 个：`correctness` → `testing` → `maintainability` → `project-standards` → `adversarial`。
- **R2** — `dimension-reviewer` 为 `maintainability` 提供 checklist，复用 `ce-code-review` skill 的 `ce-maintainability-reviewer` 关注点：耦合、复杂度、命名、死代码、抽象债务。
- **R3** — `dimension-reviewer` 为 `project-standards` 提供 checklist，复用 `ce-code-review` skill 的 `ce-project-standards-reviewer` 关注点：CLAUDE.md / AGENTS.md 合规、frontmatter、引用、可移植性。
- **R4** — `dimension-reviewer` 为 `adversarial` 提供 Red-Team checklist，聚焦隐藏副作用、兼容性破坏、边界与并发、安全路径、命名误导、测试充分性、维护成本、回滚安全；允许借鉴对抗性审查模板并做适当适配，不照搬。
- **R5** — `review-synthesizer` 能合并 5 个维度的 findings，Coverage 按维度统计；fix-plan 生成逻辑不变。低优先级 findings 的降级规则保持与现有逻辑一致并扩展至 `maintainability`：`testing` 与 `maintainability` 维度的 P2/P3 advisory 归入 soft bucket；`project-standards` 与 `adversarial` 维度的 P0/P1 保留在 primary findings。
- **R6** — Coordinator 可识别并自主调整执行路径：拆分过大单元、合并过小单元、重排单元顺序。
- **R7** — Coordinator 不得修改原始 plan；所有调整写入 scratchpad 的 `plan-amendments.md`，并注明原因与原始 U-ID 映射。
- **R8** — Coordinator 对计划修正继续使用全局置信度协议（>80 自主执行、50–80 记录并继续、<50 停止并请求用户）。
- **R9** — 事件 schema、topic_deny_rules、`review-sequence.json` 结构与 5 维度序列一致，并通过 preset_lint + SSOT byte-equality 校验。
- **R10** — Coordinator 每次激活时读取同一 plan 的历史记忆与上一轮输出。
- **R11** — Coordinator 生成的修正计划采用结构化格式（Requirements / Implementation Units / Test scenarios / Verification / Change Mapping），不调用 `ce-plan` skill。
- **R12** — 不引入新的 event topic；调整通过现有 `work.ready` payload 与 scratchpad 文件体现。

---

## Scope Boundaries

- 不引入动态/条件维度选择；5 维度固定顺序执行。
- Coordinator 不能修改目标、验收标准、范围边界或 Requirements Trace。
- 学习范围仅限**同一个 plan 的 earlier iterations**，不把跨 plan 记忆作为当前决策主因。
- 不引入新的 event topic 或 hat。
- 不改动 isolated execution mode、topic 所有权、终态语义。
- 不改动 Shipper / Reporter 的输出结构。
- 不改 `wave` 相关预设与测试。

### Deferred to Follow-Up Work

- `security` / `performance` / `api-contract` 等条件维度：当前先补齐最稳定的通用维度 + 一轮对抗性审视，未来可作为可选增强。
- 跨 plan 记忆匹配：当前仅要求同 plan 学习，跨 plan 学习需要额外的记忆检索与去噪机制，超出本次范围。

---

## Context & Research

### Relevant Code and Patterns

- `presets/en/ce-executor-serial.yml` — 主 preset 文件，所有 hat instructions 与 event policy 配置都在此。
- `presets/schemas/ce-executor-serial.yml` — 事件 schema / protocol SSOT，`review.dimensions.complete` 的 `dimensions` 数组元素形状由 prompt discipline 保证，EventSchema 不校验数组内部结构，因此维度数量变化不需要改 `required_fields`。
- `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`
- `crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml`
- `crates/ralph-core/tests/scenarios/ce_executor_serial_fix_applied_rereview.yml`
- `ce-code-review` skill（`~/.kimi-code/skills/ce-code-review/SKILL.md`）— 4 个 always-on reviewer persona：`correctness`、`testing`、`maintainability`、`project-standards`。
- 用户提供的对抗性审查 prompt 模板（本次对话）— `adversarial` 维度 checklist 的参考输入。

### Institutional Learnings

- AGENTS.md 硬规则：修改 `presets/en/<name>.yml` 后，必须同步检查 `presets/schemas/<name>.yml`，并跑 `preset_lint` + `test_ce_executor_root_preset_matches_embedded`。
- BDD scenarios 必须用真 EventLoop runner（`run_workflow_guard_scenario`）断言 events；不能用只查 iteration 数的 stub。
- 原始 plan 文件不可由 agent 修改，否则会导致 `ralph doctor plan-sync` 漂移。

### External References

- `ce-code-review` skill 中 `ce-maintainability-reviewer` 与 `ce-project-standards-reviewer` 的关注点作为新增 checklist 的输入。
- 用户提供的对抗性审查 prompt 作为 `adversarial` 维度 checklist 的参考模板。

---

## Key Technical Decisions

1. **固定 5 维度而非动态维度**：直接复用现有 serial sequence 机制，降低状态机复杂度与测试负担。
2. **Checklist 直接借鉴既有 reviewer 关注点**：`maintainability` / `project-standards` 复用 `ce-code-review`；`adversarial` 借鉴用户 Red-Team 模板并做适配，不发明新的审查语言。
3. **`adversarial` 放在最后作为 Red-Team gate**：前四轮从功能正确性、测试覆盖、工程标准审视后，最后一轮专门从攻击者视角找隐藏风险。
4. **修正计划写 scratchpad，原始 plan 只读**：避免 plan 文件漂移和 `ralph doctor plan-sync` 冲突。
5. **不调用 `ce-plan` skill**：Coordinator 自己按结构化模板生成 `plan-amendments.md`，避免 skill 递归与权限边界问题。
6. **不新增 event topic**：通过 `work.ready` payload 中的可选字段（如 `amendment_path`）与 scratchpad 文件传递修正信息，下游 hat 事件契约不变。
7. **`review-synthesizer` soft demotion 仅适用于 testing + maintainability**：`adversarial` 与 `project-standards` 的 P0/P1 必须保留在 primary findings；低优先级可维护性/测试建议归入 soft bucket。
8. **拒绝无价值的字符串存在性测试**：preset 与 hat instructions 的验证以 `preset_lint`、SSOT byte-equality、BDD scenario event-topology 测试为主，不在单元测试中写 "grep markdown 是否包含某关键词" 这类冗余检查。

---

## Open Questions

### Resolved During Planning

- **事件 schema 是否需要改维度数量？** 不需要。`review.dimensions.complete` 的 `dimensions` 数组元素形状由 prompt discipline 保证，SSOT 的 `required_fields` 只校验顶层字段。
- **`project-standards` checklist 是否强制检查 AGENTS.md/CLAUDE.md？** 是，但应限定为“若存在则检查”，避免在无关仓库误报。
- **`adversarial` checklist 是否照搬用户模板？** 否，允许借鉴其关注方向并做适当精简与适配，聚焦对当前 preset 最有价值的 Red-Team 项。
- **BDD scenarios 是否需要同步更新？** 是，三个 scenarios 目前硬编码 2 维度序列，必须扩展为 5 维度。

### Deferred to Implementation

- `plan-amendments.md` 的具体 frontmatter 与 U-ID 映射格式，需要在实现时与现有 `plan.md` 解析逻辑对齐。
- `coordinator` 激活时读取历史文件的具体顺序与缓存策略，可在实现时根据 prompt 长度微调。
- `adversarial` checklist 的详细措辞可在实现时根据 preset 风格进一步打磨，但不得删减已确定的关注方向。

---

## High-Level Technical Design

> *本节的目的是让读者快速理解改动形状，不是可复制的实现规范。*

```text
review.start
    │
    ▼
review-coordinator 初始化 5-dim review-sequence.json
    │
    ├── review.dimension.ready(correctness)
    ├── review.dimension.ready(testing)
    ├── review.dimension.ready(maintainability)
    ├── review.dimension.ready(project-standards)
    └── review.dimension.ready(adversarial)
    │
    ▼
review.dimensions.complete(dimensions=[...5 entries...])
    │
    ▼
review-synthesizer 合并 findings，生成 fix-plan（需要时）

Coordinator 侧：
work.start / test.passed 激活时
    │
    ├── 读取 memories.md / decisions.md / fix-log.md / findings.md / plan-amendments.md
    ├── 若识别出可自主修正的执行路径问题 → 按置信度协议决策
    ├── >80：写 plan-amendments.md，更新 scratchpad plan.md，创建子单元任务
    └── 通过 work.ready 继续推进（无新 topic）
```

---

## Implementation Units

- [ ] U1. **更新 preset 头部注释与 review-coordinator 维度序列**

**Goal:** 将 `ce-executor-serial` 的终审从 2 维度改为 5 维度，所有涉及序列顺序、状态机示例、进度统计的地方同步更新。

**Requirements:** R1, R9

**Dependencies:** 无

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`

**Approach:**
- 将文件头部注释中的 "2-dimension sequence (correctness → testing)" 改为 "5-dimension sequence (correctness → testing → maintainability → project-standards → adversarial)"。
- 将 `review-coordinator` 的 `description` 从 "2-dimension sequence" 改为 "5-dimension sequence"。
- 更新 `Sequence contract`：列出 5 个维度及固定顺序。
- 更新 `review-sequence.json` 示例数组，增加 `maintainability`、`project-standards`、`adversarial` 三行。
- 更新 `Walk the sequence` 中的 "always 2" / "total (always 2)" 等描述为 5。
- 更新 `Per-dimension focus strings`，补充 `maintainability`、`project-standards`、`adversarial` 的 focus 字符串。
- 更新 `Emit review.dimensions.complete` 示例中的 `dimensions` 数组，包含 5 个条目。

**Patterns to follow:** 保留原有的 `review.dimension.ready` payload 字段与 `--source dimension-reviewer` 等细节。

**Test scenarios:**
- Happy path: `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 通过。
- Edge case: 若 `review-coordinator` 的 sequence 与 `publishes` / `topic_deny_rules` 存在维度数或 topic 所有权冲突，`preset_lint` 会失败；不通过 grep 文本存在性来验证。

**Verification:**
- `preset_lint` 无失败；文件内 5 维度序列描述一致。

---

- [ ] U2. **为 dimension-reviewer 增加 maintainability checklist**

**Goal:** 让单一维度审查者能按 `maintainability` 维度的 checklist 执行只读审查。

**Requirements:** R2

**Dependencies:** U1

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`

**Approach:**
- 在 `dimension-reviewer` instructions 的 `### Dimension Checklists` 部分新增 `#### WAVE_DIMENSION == "maintainability"` block。
- Checklist 覆盖：耦合、复杂度、命名、死代码、抽象债务。
- 明确 `Do NOT flag` 边界，避免 reviewer 越界修改源码。

**Patterns to follow:** 沿用现有 `correctness` / `testing` checklist 的格式与语气；内容复用 `ce-code-review` skill 的 `ce-maintainability-reviewer` 关注点。

**Test scenarios:**
- Happy path: `preset_lint` 通过。
- Edge case: 若 `dimension-reviewer` instructions 在新增 checklist 后出现 schema/prompt 结构错误（如未闭合的代码块或非法占位符），`preset_lint` 会失败；checklist 具体内容由需求文档与代码审查保证，不写字符串存在性测试。

**Verification:**
- lint 通过；新增 checklist 与现有格式一致，不诱导 reviewer 修改源码。

---

- [ ] U3. **为 dimension-reviewer 增加 project-standards checklist**

**Goal:** 让单一维度审查者能按 `project-standards` 维度的 checklist 执行只读审查。

**Requirements:** R3

**Dependencies:** U2

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`

**Approach:**
- 在 `### Dimension Checklists` 部分新增 `#### WAVE_DIMENSION == "project-standards"` block。
- Checklist 覆盖：CLAUDE.md / AGENTS.md 合规、frontmatter、引用、可移植性。
- 明确若仓库无相关 standards 文件时降级为 "skip / N/A"，避免误报。

**Patterns to follow:** 沿用现有 checklist 格式；内容复用 `ce-code-review` skill 的 `ce-project-standards-reviewer` 关注点。

**Test scenarios:**
- Happy path: `preset_lint` 通过。
- Edge case: 若 `project-standards` checklist 破坏了 instructions 的 YAML 嵌套或引入了未声明的变量引用，`preset_lint` 会失败；不通过 grep 验证关键词存在性。

**Verification:**
- lint 通过；checklist 不诱导 reviewer 修改源码。

---

- [ ] U4. **为 dimension-reviewer 增加 adversarial checklist**

**Goal:** 让单一维度审查者能按 `adversarial` 维度的 Red-Team checklist 执行只读审查，作为最后一轮风险 gate。

**Requirements:** R4

**Dependencies:** U3

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`

**Approach:**
- 在 `### Dimension Checklists` 部分新增 `#### WAVE_DIMENSION == "adversarial"` block。
- Checklist 聚焦以下 Red-Team 方向（借鉴用户模板并适配）：
  - 隐藏副作用：函数是否修改了超出命名暗示的范围、全局/静态状态、传入参数；是否存在隐式调用顺序依赖。
  - 兼容性破坏：公共 API 语义是否变化；旧数据/配置是否降级；其他模块是否会在编译期或运行期失败。
  - 边界与并发：空值、零长度、最大值、溢出、并发竞态、资源耗尽是否显式处理；能否构造输入让代码走未测试路径。
  - 安全路径：输入是否充分校验；是否存在注入、敏感数据泄露、权限绕过路径。
  - 命名误导：函数/变量名是否准确反映行为；`get_xxx` 是否修改状态；`is_xxx` 是否触发 IO。
  - 测试充分性：测试是否验证行为而非仅执行代码；错误路径是否覆盖；是否过度 Mock；是否缺少回归测试。
  - 维护成本与回滚安全：是否引入魔法数字/硬编码；注释是否解释“为什么”；是否违反单一职责；线上故障时能否安全回滚。
- 明确 `Do NOT flag`：单纯的风格偏好、无证据的猜测、未在当前 diff 中引入的既有问题。

**Patterns to follow:** 沿用现有 checklist 格式；保持只读角色；不照搬完整模板，只保留对 preset 最有价值的条目。

**Test scenarios:**
- Happy path: `preset_lint` 通过。
- Edge case: 若 `adversarial` checklist 导致 `dimension-reviewer` instructions 解析失败或越权工具声明丢失，`preset_lint` 会失败；具体 Red-Team 关注点由需求文档与代码审查保证，不写字符串存在性测试。

**Verification:**
- lint 通过；checklist 明确只读，不诱导 reviewer 修改源码。

---

- [ ] U5. **调整 review-synthesizer 以合并 5 维度 findings 并保护 adversarial 信号**

**Goal:** 让 synthesizer 正确合并 5 维度 findings，Coverage 按维度统计，并避免 Red-Team findings 被错误降级。

**Requirements:** R5

**Dependencies:** U4

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`

**Approach:**
- 将 `Mode-Aware Demotion` 中的条件从 "ALL contributing dimensions are `testing`" 扩展为 "ALL contributing dimensions are `testing` or `maintainability`"。
- `maintainability` 维度的降级 findings 写入 `residual_risks`；`testing` 维度的继续写入 `testing_gaps`。
- 明确 `adversarial` 与 `project-standards` 维度的 P0/P1 findings 不参与 soft demotion，必须保留在 primary findings。
- 在 `Coverage` 描述中明确按 5 维度统计 status 与 findings count。

**Patterns to follow:** 与 `ce-code-review` skill 的 demotion 规则对齐，但保留 Red-Team 信号。

**Test scenarios:**
- Happy path: `preset_lint` 通过。
- Integration: U10 的全量 BDD scenario 运行会验证 5 维度 findings 能被 `review-synthesizer` 正常聚合为 `review.dimensions.complete`；demotion 行为通过实际 scenario 与 synthesizer prompt 评审保证，不写字符串存在性测试。

**Verification:**
- lint 通过；synthesizer 说明文字覆盖 5 维度统计与降级规则。

---

- [ ] U6. **同步 SSOT schema 注释**

**Goal:** `presets/schemas/ce-executor-serial.yml` 中的注释与 5 维度事实保持一致，但不改 `required_fields`。

**Requirements:** R9

**Dependencies:** U1

**Files:**
- Modify: `presets/schemas/ce-executor-serial.yml`

**Approach:**
- 将 `review.start` 注释中的 "2-dimension review sequence" 改为 "5-dimension review sequence"。
- 检查并更新其他提到 "2-dim" / "two dimension" 的注释。
- 不修改任何 `required_fields`、topic_deny_rules、execution_contracts、workflow_contract、state_projection。

**Patterns to follow:** SSOT 文件只负责 payload 与 protocol 的 SSOT；prompt 文字注释需要与 preset 一致。

**Test scenarios:**
- Happy path: `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded` 通过（重新构建后 SSOT 与 embedded preset 一致）。
- Edge case: `cargo nextest run -p ralph-core -- preset_lint` 通过。

**Verification:**
- SSOT byte-equality 测试与 preset_lint 均通过。

---

- [ ] U7. **为 Coordinator 增加同 plan 历史读取与 amendment 生成协议**

**Goal:** 让 Coordinator 在不改原始 plan、不引入新 topic 的前提下，拥有受控的单元拆分/合并/重排能力。

**Requirements:** R6, R7, R8, R10, R11, R12

**Dependencies:** U1

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`

**Approach:**
- 在 `coordinator` instructions 中新增 `### Same-Plan Learning & Plan Amendment Protocol` 小节：
  - 每次激活时读取：
    - `.ralph/agent/memories.md`
    - `.ralph/agent/decisions.md`
    - `.agents/scratchpad/ce-executor/{plan_name}/fix-log.md`
    - `.agents/scratchpad/ce-executor/{plan_name}/findings.md`
    - `.agents/scratchpad/ce-executor/{plan_name}/plan-amendments.md`（如已存在）
    - 上一轮关键事件 payload：尤其是 `test.failed` 的 `failures`/`test_errors`、`review.dimension.done` 的 `findings_count`/`findings_file`、`review.complete` 的 `verdict`/`fix_plan_file`（可从 `.ralph/events.jsonl` 或运行时注入的 `## RECENT EVENTS` 块获取）。
  - 识别重复出现的问题模式（如某文件连续多轮 test.failed、同一类 review finding 反复出现）。
  - 若当前 unit 明显过大/过小/顺序不当，按置信度协议决策：>80 写 amendment 并执行；50–80 记录到 `decisions.md` 后继续；<50 发布 `work.failed` 并说明原因（计划修正场景下不存在可接受的 safe default，停止并请求用户就是该场景下的 safe default）。
  - 修正计划写入 `.agents/scratchpad/ce-executor/{plan_name}/plan-amendments.md`，格式包含 Requirements、Implementation Units（U-ID、Goal、Files、Approach、Test scenarios、Verification）、Change Mapping（原始 U-ID → 新 U-ID）。
  - 更新 scratchpad 内的 `plan.md`（不是原始 plan 文件）以反映新的 step 序列。
  - 子单元任务 key 沿用现有 R4 carve-out：`ce-executor:{plan_name}:step-03:u3a-interface`、`ce-executor:{plan_name}:step-03:u3b-implementation`（保持 `step` 字段为 `step-03`，子单元区分放在 key 末尾）。
  - 在 `work.ready` payload 中可携带可选字段 `amendment_path`，供 executor 读取；不携带也不影响现有流程。
- 在 `Constraints` 中重申：禁止修改原始 plan 文件。

**Patterns to follow:** 复用现有置信度协议与 sub-unit key 格式；amendment 文件格式借鉴 `ce-plan` 的 Implementation Unit 模板。

**Test scenarios:**
- Happy path: `preset_lint` 通过。
- Edge case: 若 amendment 协议中的新指令导致 `coordinator` instructions YAML 结构错误、未闭合的代码块或非法事件引用，`preset_lint` 会失败。
- Error path: 若协议意外允许修改原始 plan，`preset_lint` 或 SSOT 测试会暴露 topic/contract 不一致；不在测试中写 "markdown 是否包含某禁止语句" 的字符串检查。

**Verification:**
- lint 通过；新增协议不引入新 event topic；原始 plan 只读约束明确。

---

- [ ] U8. **更新 BDD scenario：ce_executor_serial_review.yml 为 5 维度**

**Goal:** 让主场景覆盖 5 维度终审链路。

**Requirements:** R1, R9

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`

**Approach:**
- 更新文件头部注释，描述 5-dim 链路。
- 在 mock_responses 中插入 `maintainability`、`project-standards`、`adversarial` 的 `review.dimension.ready` / `review.dimension.done` 事件。
- 更新 `review.dimensions.complete` 的 `dimensions` 数组为 5 个条目。
- 更新 `expected.events` 中的 ready/done 对数量（5 对）。
- 更新迭代数 `iterations`：每新增一个维度增加 2 个 turn（ready + done），3 个新维度共 +6；`ce_executor_serial_review.yml` 从 15 改为 21。

**Patterns to follow:** 沿用现有 scenario 的事件格式与字段；保持 `changed_files`、`intent_summary` 等示例值不变。

**Test scenarios:**
- Happy path: `cargo nextest run -p ralph-core --test scenarios -- ce_executor_serial_review` 通过。
- Error path: 若维度数量错误，scenario 应因 expected events 不匹配而失败。

**Verification:**
- 该 scenario 在真 EventLoop runner 中通过。

---

- [ ] U9. **更新其余两个 BDD scenarios 为 5 维度**

**Goal:** 让 silent-reviewer-recovery 与 fix-applied-rereview 场景与 5 维度事实保持一致。

**Requirements:** R1, R9

**Dependencies:** U8

**Files:**
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml`
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_serial_fix_applied_rereview.yml`

**Approach:**
- 对每个文件执行与 U8 相同的变更：头部注释、mock_responses 中插入三个新维度、更新 `dimensions` 数组、更新 `expected.events` 与 `iterations`。
- 迭代数公式：`iterations_new = iterations_old + (新增维度数 × 2)`。
  - `ce_executor_serial_review_silent_reviewer_recovers.yml` 原 16 → 22。
  - `ce_executor_serial_fix_applied_rereview.yml` 原 17 → 23。
- 对于 silent-reviewer 场景，保持第一次 `dimension-reviewer` silent 发生在 `correctness` 维度的设定。
- 对于 fix-applied 场景，保持 `fix_round=1` 不变。

**Test scenarios:**
- Happy path: `cargo nextest run -p ralph-core --test scenarios -- ce_executor_serial_review_silent_reviewer_recovers` 通过。
- Happy path: `cargo nextest run -p ralph-core --test scenarios -- ce_executor_serial_fix_applied_rereview` 通过。

**Verification:**
- 两个 scenario 均通过。

---

- [ ] U10. **全量校验与回归测试**

**Goal:** 确保 preset、schema、BDD scenarios 全部一致，满足 origin 中的成功标准。

**Requirements:** R9, R12（不引入新 topic 的回归验证）

**Dependencies:** U1–U9

**Files:**
- 不涉及新文件创建；运行测试验证上述修改。

**Approach:**
- 重新构建以嵌入最新 preset：`cargo build -p ralph-cli`。
- 跑 preset lint：
  - `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
  - `cargo nextest run -p ralph-core -- preset_lint`
- 跑 SSOT byte-equality：
  - `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`
- 跑 BDD scenarios：
  - `cargo nextest run -p ralph-core --test scenarios -- ce_executor_serial`
- 可选全量入口：
  - `./scripts/run-tests.sh`

**Test scenarios:**
- Happy path: 上述所有命令均通过。
- Error path: 任一 lint/SSOT/scenario 失败则回退到对应 U 修复。

**Verification:**
- `ralph-cli` preset_lint、ralph-core preset_lint、SSOT byte-equality、三个 scenarios 全部 green。

---

## System-Wide Impact

- **Interaction graph:** `review-coordinator` 与 `dimension-reviewer` 之间的事件数量从 2 对增加到 5 对，但 topic 与 payload 字段不变；`review-synthesizer` 的输入 `dimensions` 数组变长，合并逻辑保持不变。
- **Error propagation:** 单个 dimension-reviewer 失败仍由 `review.dimension.failed` 承载，coordinator 继续下一个维度，行为不变。
- **State lifecycle risks:** `review-sequence.json` 的数组长度从 2 变为 5，旧循环若复用遗留的 2 行文件会被 coordinator 的 corruption-recovery 规则重新初始化为 5 行。
- **API surface parity:** 无新增 CLI flag 或公开 API。
- **Integration coverage:** 5 维度链路需要通过 BDD scenarios 验证；preset_lint 验证 schema/topic 一致性。
- **Unchanged invariants:** `work.ready` / `work.done` / `test.passed` / `test.failed` / `fix.applied` / `plan.complete` / `REVIEW_COMPLETE` / `report.done` / `LOOP_COMPLETE` 的事件契约、topic_deny_rules、execution_mode 均不变。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| 新增 checklist 诱导 dimension-reviewer 尝试修改源码 | 在 checklist 前重申 read-only 角色；保留 `disallowed_tools: ["Edit"]`。 |
| `adversarial` checklist 过宽导致大量误报 | 聚焦“隐藏副作用、兼容性、边界/并发/安全、命名误导、测试、回滚”等可验证项；明确 `Do NOT flag` 边界。 |
| Coordinator amendment 协议写得过宽，导致擅自修改目标/范围 | 明确约束：只能拆/合/重排单元，不能改目标、验收标准、Requirements Trace；<50 置信度必须 `work.failed`。 |
| BDD scenarios 中迭代数算错导致 scenario 失败 | 按新增 3 个 ready/done 对 + 可能的多轮 task.resume 重新计数；运行测试验证。 |
| SSOT byte-equality 失败 | 修改 `presets/en/ce-executor-serial.yml` 后先 `cargo build -p ralph-cli` 再跑测试；确保没有遗漏的 inline schema 覆盖。 |

---

## Documentation / Operational Notes

- 无需新增用户文档；preset 自身的 instructions 就是文档。
- 若后续 Operator 需要了解 amendment 机制，可查看 `plan-amendments.md` 与 `decisions.md`。
- 不改 `scripts/ralph-zsh-plugin.zsh`，因为 builtin preset 名称未变。

---

## Sources & References

- **Origin document:** `docs/brainstorms/2026-06-25-ce-executor-serial-review-dimensions-and-coordinator-autonomy.md`
- **Preset file:** `presets/en/ce-executor-serial.yml`
- **Schema SSOT:** `presets/schemas/ce-executor-serial.yml`
- **BDD scenarios:** `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`、`ce_executor_serial_review_silent_reviewer_recovers.yml`、`ce_executor_serial_fix_applied_rereview.yml`
- **Review 维度参考：** `~/.kimi-code/skills/ce-code-review/SKILL.md`
- **Adversarial 参考：** 用户提供的对抗性审查 prompt 模板（本次对话）
