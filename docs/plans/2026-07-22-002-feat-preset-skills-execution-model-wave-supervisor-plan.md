---
title: "feat: preset operator skills 补齐通用执行模型（single-chain / wave / supervisor）"
type: feat
date: 2026-07-22
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: conversation-2026-07-22-execution-model-skills
execution: skills-docs-fixtures-tests
origin: conversation-2026-07-22-skill-gap-supervisor-wave
---

# feat: preset operator skills 补齐通用执行模型（single-chain / wave / supervisor）

> **给 Coding Agent**：本计划只改 **operator skills**（`skills/ralph-preset-*`、`skills/ralph-run-diagnosis`、共享 `skills/ralph-preset-common`）及其 **fixtures / 契约测试**。不改 Rust runtime、不改 builtin preset YAML、不写生产业务代码以外的实现。严格按 Unit 1 → Unit N 串行执行；每个 Unit 先写/启用验收测试，确认 Red，再改文档/fixture，再 Green，再回归。

---

## 1. 功能目标

### 业务目标

- 让 **author / review / diagnose** 三套通用 skill 能正确处理三类执行模型：
  1. **single-chain**（默认推荐）
  2. **wave fan-out**（同 topic 批并行，`ralph wave emit`）
  3. **supervisor orchestration**（`event_loop.supervisor.enabled`；slot / worktree / fan-in；协调 topic 由 runtime 管）
- **Author 阶段必须询问用户**是否需要 wave / supervisor；用户否认或选默认 → **锁定 single-chain**，不得暗中写成 supervisor/wave 拓扑。
- 检查与审计一律 **capability-triggered**（由 Intent + YAML/产物信号触发），**禁止**按某个 builtin preset 名称定制流程（例如不得新增 `ce-executor-supervisor*` 名缀专检）。

### 本次范围

| ID | 能力 | 外部可观察结果 |
|----|------|----------------|
| R1 | 执行模型词汇与 Intent 字段 | `Preset Intent Confirmation` 含 `execution_model`；共享术语表可查 |
| R2 | Author Discovery 必问 | 新拓扑/并行意图时弹出互斥菜单；否认 → single-chain |
| R3 | Author Hard questions（wave / supervisor） | model∈{wave,supervisor*} 时 checklist 强制；single-chain 时标记 N/A |
| R4 | Review capability audit | 仅当检测到 wave/supervisor 能力时跑对应审计；产出通用 finding_id |
| R5 | 匿名负例 fixtures + README 验收 | 不绑 builtin 名；正负期望可核对 |
| R6 | Diagnosis 能力感知对账 | 报告声明 `execution_capabilities`；supervisor.db / wave_id 按能力读 |
| R7 | 契约测试防回归 | `skills/tests/` 结构/矩阵断言；既有 AAF fixtures 与 CE pipeline 3b 不破坏 |

### 非目标

- 不实现/不修改 Rust `SupervisorStore`、wave CLI、preset_lint 新规则（runtime 已有 lint id 只在 rubric 中**引用**）。
- 不改写 `presets/en/ce-executor-supervisor.yml` 或其它 builtin preset。
- 不把 `ce-executor-pipeline` 的 3b 名缀专检在本计划中重构掉（仅保证不被破坏；收敛列为 follow-up）。
- 不向 `crates/ralph-core/data/*.md` 注入 skill 写入本计划编号或 preset 专属内容。
- 不把 `ralph-tools-wave.md` 参数表复制进 hat `instructions:` 或 operator skill 正文（只 cite）。
- 不要求每个窄机械编辑都强制访谈；仅「新 preset / 实质拓扑或并行行为变更」触发执行模型问。

### 已知约束和假设

- **SSOT 落点**：仓库内 `skills/` 为源；`skills/install.py` 物理拷贝到 `.claude/skills` / `.agents/skills`；author/review 的 `references/` 来自 `ralph-preset-common`。
- **通用性硬约束**：新流程、新 finding、新 fixture、新诊断小节均以 **capability / 信号** 命名，不得出现「仅当 preset 名称为 X」的门控（既有 CE pipeline 3b 例外保留，本计划不扩展该模式）。
- **执行模型枚举（冻结）**：
  - `single-chain`
  - `wave`
  - `supervisor`
  - `supervisor+wave`（supervisor 且 dispatcher 使用 wave fan-out）
- **能力检测信号（冻结，供 review/diagnose 共用）**：
  - Intent：`execution_model` ∈ {wave, supervisor, supervisor+wave}
  - YAML：`event_loop.supervisor.enabled: true`
  - YAML/instructions：出现 `ralph wave emit` / `ralph wave verify` 或 hat 依赖 `## WAVE CONTEXT`
  - 产物（diagnose）：存在 `.ralph/supervisor.db`，或 events 含 `wave_id`，或日志出现 wave fan-out
- **默认推荐**：菜单第一项永远是 single-chain；用户否认 wave/supervisor → 写入 `execution_model: single-chain`。
- **测试形态**：本工作以 skill 文档 + fixture + Python 契约测试为主；软性 AAF 仍靠 fixture README 的 operator 验收矩阵（与现有 `skills/ralph-preset-common/fixtures/README.md` 同型）。不引入 E2E 跑真实 LLM。
- **与并行 runtime 计划关系**：`docs/plans/2026-07-22-001-feat-wave-protocol-suite-default-plan.md` 改的是 wave **runtime**；本计划只补 **operator skill 流程**。二者正交；本计划不得假设 001 已落地的新 CLI 行为，只依赖当前已存在的 `ralph wave` / supervisor 公开语义（见 `crates/ralph-core/data/ralph-tools-wave.md` 与现有 config 字段）。

---

## Product Contract（摘要）

### Actors

- A1. Preset author（使用 `ralph-preset-author`）
- A2. Preset reviewer（使用 `ralph-preset-review`）
- A3. Run diagnoser（使用 `ralph-run-diagnosis`）
- A4. 人类用户（回答执行模型意图）

### Key Flows

- F1. Author Discovery → 执行模型菜单 → Intent Confirmation → 按 model 分支 checklist → notes
- F2. Review：读 Intent + YAML 信号 → 决定是否跑 Wave/Supervisor audit → findings
- F3. Diagnose：Phase 0 推断 `execution_capabilities` → 条件对账 supervisor.db / wave → 报告

### Acceptance Examples（对外可观察）

- AE1. 用户选「同一条主链…」→ notes 首部 `execution_model: single-chain`；无 `supervisor.enabled`；无 dispatcher `wave emit`。
- AE2. 用户明确否认 supervisor/wave → 同 AE1；author 不得自行升级为 B/C。
- AE3. 用户选 wave → notes 填 Wave Hard questions；review 对缺 `wave verify` 的 dispatcher 报通用 finding。
- AE4. 用户选 supervisor → notes 填 Supervisor Hard questions；instructions 读 `supervisor.db` → P0 `preset.artifact_uses_internal_ledger` 或等价 visibility finding。
- AE5. single-chain preset 过 review：**不**强制跑 Wave/Supervisor audit 专段（可声明 N/A）。
- AE6. diagnose 在无 supervisor/wave 信号的 run：**不**把缺失 `supervisor.db` 当异常；有 `wave_id` 时 Confirm 对账指向 main ledger。

---

## 2. BDD 行为规格

```gherkin
Feature: Preset operator skills 按执行模型分支（通用，非某 preset 定制）

  Background:
    Given 操作者使用仓库内 skills/ 下的 ralph-preset-author / ralph-preset-review / ralph-run-diagnosis
    And 执行模型词汇已定义为 single-chain | wave | supervisor | supervisor+wave

  # --- Author Discovery ---

  Scenario: 新拓扑起草时询问执行模型且默认推荐单链
    Given 用户要求新建或实质变更 preset 拓扑（含并行/多 unit 意图或未声明编排方式）
    When author 进入 Discovery gate
    Then 必须通过交互菜单询问「多个工作单元怎么推进」
    And 推荐项为 single-chain 并说明后果
    And 选项覆盖 wave 与 supervisor（及自定义）

  Scenario: 用户否认 wave/supervisor 后锁定单链
    Given Discovery 菜单已展示
    When 用户选择 single-chain 或明确否认 wave/supervisor
    Then Intent Confirmation 写入 execution_model: single-chain
    And 后续拓扑不得引入 event_loop.supervisor.enabled
    And 后续 instructions 不得要求 dispatcher 调用 ralph wave emit

  Scenario: 窄机械编辑不强制访谈
    Given 变更仅为文案/字段 docs 等无行为歧义的机械编辑
    When author 推断 execution_model 与现网一致
    Then 允许不重新访谈
    And 必须在 Intent/notes 中写明推断来源

  Scenario: 非法/模糊答案必须追问
    Given 用户回答「适当并行」「必要时用 supervisor」
    When author 无法映射到枚举值
    Then 必须再次给出互斥可选项
    And 在确认前 STOP，不起草 YAML

  # --- Author checklists ---

  Scenario: single-chain 只跑单链 Hard questions
    Given execution_model = single-chain
    When 填写 pre-review gate
    Then Single-chain-first 5 问必须完成
    And Wave / Supervisor Hard questions 标记为 N/A（不得留空假装已答）

  Scenario: wave 模型强制 Wave Hard questions
    Given execution_model ∈ {wave, supervisor+wave}
    When 填写 pre-review gate
    Then Wave Hard questions 每一项必须有 ✓/✗ + 证据
    And emitter 引用 ralph-tools-wave / ralph-tools-emit Policy-Check 反馈（cite 不复制）

  Scenario: supervisor 模型强制 Supervisor Hard questions
    Given execution_model ∈ {supervisor, supervisor+wave}
    When 填写 pre-review gate
    Then Supervisor Hard questions 每一项必须有 ✓/✗ + 证据
    And 禁止 hat 读写 supervisor.db 作为业务接口

  # --- Review capability gate ---

  Scenario: 无 wave/supervisor 信号时不强制 Wave/Supervisor audit
    Given 被审 preset 的 Intent 为 single-chain
    And YAML 无 supervisor.enabled 且无 wave emit 指令
    When review 执行拓扑审计
    Then Wave audit 与 Supervisor audit 记录为 N/A
    And 仍必须执行 Single-chain-first 与 AAF

  Scenario: 检测到 wave 能力时运行 Wave audit
    Given YAML 或 instructions 含 ralph wave emit
    Or Intent.execution_model ∈ {wave, supervisor+wave}
    When review 运行 Wave audit
    Then 按 finding-rubric「Wave capability audit」逐项判定
    And 命中项写入主表（confidence ≥ 60）

  Scenario: worker 调用 wave emit 为 P0
    Given 某非 dispatcher hat instructions 要求 ralph wave emit
    When Wave audit 执行
    Then 报告通用 finding（如 preset.wave_worker_calls_wave_emit）severity P0

  Scenario: 检测到 supervisor 能力时运行 Supervisor audit
    Given event_loop.supervisor.enabled = true
    Or Intent.execution_model ∈ {supervisor, supervisor+wave}
    When review 运行 Supervisor audit
    Then 校验 isolated、禁发 coordination topic、禁把 supervisor.db 当业务 artifact
    And 引用已有 lint id：preset.supervisor_requires_isolated 等

  Scenario: 禁止按 preset 名称触发新审计
    Given 任意 builtin 或 local preset 名称
    When 实现或文档描述 Wave/Supervisor audit 触发条件
    Then 触发条件不得写「名称以 ce-executor-supervisor 开头」这类名缀门控

  # --- Fixtures ---

  Scenario: wave 负例 fixture 可被 review 技能按矩阵验收
    Given fixtures/aaf-wave-capability-negative-fixture.yml 存在
    When 按 fixtures/README 矩阵执行软性 AAF
    Then 至少命中文档列出的 Wave P0/P1
    And fixture 文件名与内容不出现具体 builtin preset 专属拓扑绑定

  Scenario: supervisor 负例 fixture 可被 review 技能按矩阵验收
    Given fixtures/aaf-supervisor-capability-negative-fixture.yml 存在
    When 按 fixtures/README 矩阵执行软性 AAF
    Then 至少命中文档列出的 Supervisor P0
    And 含读/写 supervisor.db 或 agent 发 coordination topic 的反模式

  # --- Diagnosis ---

  Scenario: 无能力信号时缺失 supervisor.db 不是异常
    Given run_dir 无 supervisor.db 且 events 无 wave_id
    And preset 未启用 supervisor
    When diagnose Phase 0 盘点
    Then execution_capabilities 含 single-chain（或不含 wave/supervisor）
    And 不将「缺少 supervisor.db」列为故障

  Scenario: 有 wave_id 时 Confirm 对账走 main ledger
    Given events 中存在 wave_id
    When diagnose 做 OPAC/wave 对账
    Then 指引确认路径为 main events 源
    And 不得要求用 hat-channel 验证 wave 写入

  Scenario: 存在 supervisor.db 时标记 runtime-only
    Given .ralph/supervisor.db 存在
    When 产物盘点
    Then Tier B 标注 runtime only
    And 业务结论仍以 events + tasks + Tier C artifact 为准
```

---

## 3. 验收与测试策略

| Scenario | 验收条件 | 推荐测试层级 | 是否需要 E2E |
| -------- | -------- | ------------ | ------------ |
| 新拓扑询问执行模型 | author SKILL + checklist 含菜单与触发条件；契约测试断言关键标题/字段存在 | 契约（解析 markdown） | 否 |
| 用户否认锁定单链 | Intent 模板含 `execution_model`；author 文案含「否认→single-chain」硬约束 | 契约 + 文档 Scenario 对照 | 否 |
| 窄机械编辑跳过访谈 | author SKILL 写明例外与「写明推断来源」 | 契约（关键字） | 否 |
| 模糊答案追问 | author Discovery grill 规则覆盖执行模型 | 契约 | 否 |
| single-chain N/A wave/supervisor 问 | checklist 明确 N/A 规则 | 契约 | 否 |
| wave Hard questions | checklist 新段存在且 pre-review gate 引用 | 契约 | 否 |
| supervisor Hard questions | 同上 | 契约 | 否 |
| review 无信号 N/A | review SKILL 含 capability 门控与 N/A 写法 | 契约 | 否 |
| review wave audit | rubric 表 + review 步骤；负例 fixture 矩阵 | 契约 + fixture 软性验收 | 否 |
| worker wave emit P0 | fixture + rubric finding_id | fixture 软性 + 契约（ID 存在） | 否 |
| review supervisor audit | rubric + review 步骤；负例 fixture | fixture 软性 + 契约 | 否 |
| 禁名称门控 | 契约测试扫描**本计划新增段落**不得含 `ce-executor-supervisor` 名缀触发语 | 契约 / characterization | 否 |
| wave/supervisor fixtures | README 矩阵行完整；`ralph preset check` 对 fixture 可跑（不要求新 lint） | 集成（CLI check 冒烟）+ 软性 | 否 |
| diagnose 无信号 | diagnosis 文档：缺 db 非故障 | 契约 | 否 |
| diagnose wave_id | report-template / verification 含 main Confirm | 契约 | 否 |
| diagnose supervisor.db | artifact-manifest 与报告字段一致 | 契约 | 否 |
| 既有 AAF fixtures 回归 | README 旧矩阵仍成立；install 测试绿 | 回归（既有 pytest + 文档） | 否 |
| 关键用户主路径 | 人工按 author→review 走一遍 single-chain 与 wave 各一（可选） | 手工冒烟 | 可选，非门禁 |

---

## 4. 需求—测试追踪矩阵

| 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E |
| ---- | -------- | -------- | -------- | ------------- | --- |
| R1 词汇+Intent | Intent 字段 / 术语 | `test_execution_model_intent_template_field` | 解析 Intent 模板正则 | `skills/tests/test_execution_model_contract.py` | — |
| R2 Author 必问 | 询问；否认锁定 | `test_author_discovery_menu_present` 等 | 菜单选项集合断言 | 同上 | 手工可选 |
| R3 Hard questions | wave/supervisor/N/A | `test_author_hard_questions_sections` | 段落标题存在 | 同上 | — |
| R4 Review audit | N/A / wave / supervisor / 禁名缀 | `test_review_capability_gate_wording` + fixture 矩阵 | finding_id 表含新 ID | 同上 + fixtures/README | — |
| R5 Fixtures | 两负例 Scenario | README Acceptance 更新；CLI check 冒烟 | — | `ralph preset check -H <fixture>` | — |
| R6 Diagnosis | 三 diagnose Scenario | `test_diagnosis_capability_sections` | 报告字段关键字 | 同上 | — |
| R7 回归 | 旧 fixtures / install / CE 3b 保留 | 既有 `test_install.py`；characterization：3b 段仍在 | — | install + 文档扫描 | — |

---

## 5. 严格串行开发单元

> **全局 TDD 纪律**：每个 Unit 开始时先添加/启用该 Unit 的契约测试或 fixture 期望 → 运行确认 **Red（缺段落/缺字段/缺 fixture）** → 最小改文档/fixture → Green → 重构措辞 → 跑本 Unit 回归 → 关闭 Unit。禁止删断言、跳过测试、或用「以后再写」留下本 Unit 必需逻辑。

---

### Unit 1 — 执行模型词汇与 Intent Confirmation 字段

* **Unit 目标**：在共享 references 中冻结执行模型枚举、检测信号、Intent 字段，供后续 Unit 引用。
* **对应 Scenario**：Background；Intent 相关验收基础。
* **外部可观察结果**：
  * `skills/ralph-preset-common/references/agent-native-model.md` 新增「执行模型（Execution Model）」段；
  * `skills/ralph-preset-common/references/author-checklist.md` 的 Intent 模板增加 `execution_model` 与一句 why。
* **输入与输出**：
  * 输入：已确认的枚举与信号表（本计划 §1）；
  * 输出：文档段落 + 契约测试断言字段存在。
* **可依赖的已完成能力**：现有 Intent Confirmation、Artifact-First 文档结构。
* **明确禁止依赖的未来能力**：Author 菜单文案（U2）、Hard questions 正文（U3）、Review 门控（U5）、fixtures（U6）、diagnosis（U7）。
* **验收测试**：`skills/tests/test_execution_model_contract.py::test_intent_template_has_execution_model`；`test_agent_native_model_defines_execution_models`。
* **需要拆分的单元测试**：枚举四值均出现；检测信号四类关键字均出现；明确写「capability-triggered / 禁止 preset 名缀门控」。
* **Red 预期失败原因**：模板与 agent-native-model 尚无 `execution_model` / 执行模型段。
* **最小实现范围**：仅上述两个文件的新增段落；不改 SKILL.md workflow。
* **集成验证**：`pytest skills/tests/test_execution_model_contract.py -k intent_or_vocab`（或全文件当前已启用用例）。
* **回归范围**：`pytest skills/tests/test_install.py -q`（确保未破坏安装契约）。
* **完成标准**：相关契约测试绿；枚举与信号与本计划逐字一致；无 builtin preset 名作为触发条件。
* **风险与注意事项**：`patterns.md` 里历史 builtin 样例可保留，但 U1 **不要**把样例写成门控；术语首次出现必须解释 wave / supervisor 对 agent 的可见差异（WAVE CONTEXT、禁读 supervisor.db）。

---

### Unit 2 — Author Discovery：询问执行模型并默认单链

* **Unit 目标**：Author 在 Discovery gate 对用户意图提问；否认 → 锁定 `single-chain`。
* **对应 Scenario**：新拓扑询问；用户否认锁定；窄机械编辑例外；模糊答案追问。
* **外部可观察结果**：
  * `skills/ralph-preset-author/SKILL.md` Workflow 0 增加执行模型菜单与锁定规则；
  * Topology 段删除/改写「如 ce-executor-supervisor 风格」点名升级暗示，改为「仅当 Intent.execution_model 允许」；
  * `author-checklist.md` 阶段 0 增加对应勾选项。
* **输入与输出**：
  * 输入：U1 Intent 字段；
  * 输出：可执行的提问规程（菜单 2–4 选项，推荐 single-chain 第一）。
* **可依赖**：U1。
* **禁止依赖**：U3 Hard questions 细则、U5 review、U6 fixtures。
* **验收测试**：`test_author_skill_asks_execution_model`；`test_author_deny_locks_single_chain`；`test_author_mechanical_edit_exception`。
* **单元测试要点**：断言推荐项文案含「推荐」；断言否认路径写 `execution_model: single-chain`；断言禁止在未确认时写 `supervisor.enabled` / dispatcher `wave emit`。
* **Red 预期失败原因**：SKILL 尚无菜单或仍用 preset 名暗示升级。
* **最小实现范围**：author SKILL + checklist 阶段 0；不写 Wave/Supervisor Hard questions 正文（可留「见 U3」锚点标题则不推荐——宁可 U3 再加，避免空标题骗过测试）。
* **集成验证**：执行模型契约测试子集 + install。
* **回归范围**：通读既有 Discovery 规则未被削弱（仍禁止把仓库可查事实反问用户）。
* **完成标准**：四个 Scenario 在文档层可逐步对照；契约测试绿。
* **风险与注意事项**：菜单用业务语言，避免堆砌 runtime jargon；与「author-owned vs user-owned」分类一致——执行模型是 **user-owned intent**。

---

### Unit 3 — Author Hard questions：Wave 与 Supervisor + pre-review 接线

* **Unit 目标**：为 wave / supervisor 提供与 single-chain-first 同级的 Hard questions；pre-review gate 按 model 强制或 N/A。
* **对应 Scenario**：single-chain N/A；wave 强制；supervisor 强制。
* **外部可观察结果**：
  * `author-checklist.md` 新增：
    * `Hard questions — wave fan-out`
    * `Hard questions — supervisor orchestration`
    * N/A 规则一段；
  * `ralph-preset-author/SKILL.md` pre-review gate 引用上述两段。
* **Hard questions 最低覆盖（实现时不得删减语义）**：
  * **Wave**：唯一 dispatcher；worker 禁 `wave emit`；`wave verify`→emit；Confirm 用 main ledger；禁 agent 发协调 topic；batch 失败可定位（`payload_index` / 等价公开错误）；cite `ralph-tools-wave`。
  * **Supervisor**：`supervisor.enabled` + isolated；禁读/写 `supervisor.db` 作业务接口；禁发 coordination topic；unit 状态经 task API / 业务 artifact；timeout/partial 有业务可见出口；与 Intent 一致。
* **输入与输出**：U1–U2 Intent → 完整 notes 勾选能力。
* **可依赖**：U1、U2。
* **禁止依赖**：U4 finding 表（可先用描述性缺口，U4 再挂 id）；U5/U6。
* **验收测试**：`test_author_wave_hard_questions_section`；`test_author_supervisor_hard_questions_section`；`test_prereview_gate_references_model_branches`。
* **Red 预期失败原因**：无对应 Hard questions 段或 gate 未引用。
* **最小实现范围**：checklist + author SKILL gate 文案。
* **集成验证**：契约测试全文件当前用例。
* **回归范围**：Single-chain-first 与 Artifact-First Hard questions 段落仍在且未被改语义。
* **完成标准**：三 Scenario 文档可执行；测试绿。
* **风险与注意事项**：问题必须是 hat 视角可答（HARD RULE 4），不要问「supervisor 内部 queue 如何实现」。

---

### Unit 4 — finding-rubric：Wave / Supervisor capability audit 表

* **Unit 目标**：在 `finding-rubric.md` 增加**通用** Wave / Supervisor audit 表与 finding_id；供 review 引用。
* **对应 Scenario**：worker wave emit P0；supervisor 禁 ledger；禁名称门控（表内无名缀触发）。
* **外部可观察结果**：rubric 新节：
  * `Wave capability audit`
  * `Supervisor capability audit`
  * 映射到 finding_id（新建 review-only 或引用已有 lint id）。
* **建议 finding_id（可在实现时微调命名，但必须稳定入库并写进契约测试）**：
  * 复用：`preset.supervisor_requires_isolated`、`preset.supervisor_hat_publishes_coord_topic`、`preset.instructions_supervisor_coordination_topic`、`preset.artifact_uses_internal_ledger`、`preset.instructions_read_internal_ledger`
  * 新增（review-only，示例名）：`preset.wave_worker_calls_wave_emit`、`preset.wave_missing_verify_before_emit`、`preset.wave_confirm_uses_hat_channel`、`preset.wave_agent_emits_coordination_topic`、`preset.supervisor_unit_state_not_via_task_api`、`preset.execution_model_intent_mismatch`
* **输入与输出**：U3 问题语义 → 可引用 ID。
* **可依赖**：U1 术语；可选回链 U3。
* **禁止依赖**：U5 SKILL 步骤、U6 fixture 文件（本 Unit 只定义表）。
* **验收测试**：`test_rubric_has_wave_capability_audit`；`test_rubric_has_supervisor_capability_audit`；`test_new_audit_sections_have_no_preset_name_gate`。
* **Red 预期失败原因**：rubric 无新节或新节含名缀门控句。
* **最小实现范围**：仅 `finding-rubric.md`（必要时一行链到 `agent-native-model.md`）。
* **集成验证**：契约测试。
* **回归范围**：既有 Artifact-First / Single-chain-first / CE pipeline 软性表仍在。
* **完成标准**：ID 列表被测试钉死；无 `名称以 … 开头` 触发语。
* **风险与注意事项**：CE pipeline 专表保留但不扩展；新表标题必须带 `capability`。

---

### Unit 5 — Review skill：capability-triggered 审计流程

* **Unit 目标**：review workflow 在 3a 之后增加 Wave/Supervisor audit，**仅**按 U1 信号触发。
* **对应 Scenario**：无信号 N/A；有 wave 跑 Wave audit；有 supervisor 跑 Supervisor audit；禁名称门控。
* **外部可观察结果**：
  * `skills/ralph-preset-review/SKILL.md` 新增步骤（建议编号 `3d`/`3e`，避免改动既有 `3b` CE pipeline 语义）；
  * 明确检测顺序：Intent → YAML supervisor.enabled → instructions/publishes 中的 wave 命令；
  * 指向 U4 rubric 表。
* **输入与输出**：被审 preset + 可选 notes → Findings Table 可含新 category。
* **可依赖**：U1、U4（U3 notes 字段可选用于 mismatch finding）。
* **禁止依赖**：U6 fixture 文件内容（workflow 不写死 fixture 名以外的验收；fixture 列表更新放 U6）。
* **验收测试**：`test_review_skill_capability_gates`；`test_review_skill_preserves_ce_pipeline_3b`；`test_review_new_gates_not_name_prefixed`。
* **Red 预期失败原因**：无 capability 步骤，或误加 supervisor 名缀专检。
* **最小实现范围**：review SKILL.md；如需一句链到 checklist N/A 规则可改 common 一行。
* **集成验证**：契约测试 + 人工对照 `builtin:debug` 文档路径应 N/A wave/supervisor audit。
* **回归范围**：3a single-chain-first、3b CE pipeline、3c operator fixtures 列表结构仍在（U6 再追加新 fixture 名）。
* **完成标准**：四个 Scenario 可逐步执行；测试绿；3b 文本未被删。
* **风险与注意事项**：`execution_model_intent_mismatch`：YAML 开了 supervisor 但 Intent 写 single-chain → P0/P1（实现时定 severity 并钉在 rubric）。

---

### Unit 6 — 匿名负例 fixtures + README ATDD 矩阵 + CLI 冒烟

* **Unit 目标**：提供不绑定 builtin 名的 wave/supervisor 负例，并写入 fixtures README 验收矩阵。
* **对应 Scenario**：wave 负例；supervisor 负例。
* **外部可观察结果**：
  * `skills/ralph-preset-common/fixtures/aaf-wave-capability-negative-fixture.yml`
  * `skills/ralph-preset-common/fixtures/aaf-supervisor-capability-negative-fixture.yml`
  * 更新 `fixtures/README.md`（文件表 + Acceptance + review SKILL 3c 列表追加）
* **Fixture 设计约束**：
  * 最小 hat 数（建议 3–5）；`execution_mode: isolated`；
  * 故意植入 U4 反模式；
  * **不得**复制 `ce-executor-supervisor` 全拓扑；
  * 不注册进 `presets/manifest.yml`。
* **输入与输出**：rubric ID → 可教可验的 YAML。
* **可依赖**：U4、U5。
* **禁止依赖**：U7 diagnosis。
* **验收测试**：
  * README 矩阵行断言（契约测试可读 README 锚点）；
  * `ralph preset check -H skills/ralph-preset-common/fixtures/aaf-wave-capability-negative-fixture.yml --strict --format json` 可执行（允许仅环境性/既有 lint，不要求新 lint 必现——软性 finding 靠 README）；
  * supervisor fixture 同理。
* **单元测试**：fixture YAML 可被 `RalphConfig::parse` 或 CLI check 加载（集成冒烟）；内容含反模式关键字（`wave emit` 在 worker、`supervisor.db` 等）。
* **Red 预期失败原因**：文件不存在或 README 无矩阵。
* **最小实现范围**：两 fixture + README + review 3c 列表一行追加。
* **集成验证**：CLI check 冒烟 + 契约测试。
* **回归范围**：旧 fixture README 章节 1–6 不被删改语义；`pytest skills/tests/test_install.py`。
* **完成标准**：矩阵列出期望 P0/P1 与对应 finding_id；人工按 review skill 能标出命中。
* **风险与注意事项**：与 `aaf-artifact-first-negative-fixture` 中 supervisor.db 反模式可互补，本 Unit fixture 应更侧重 **wave OPAC** 与 **coordination topic** 轴，避免完全重复。

---

### Unit 7 — Diagnosis：能力感知盘点与对账

* **Unit 目标**：diagnose 根据信号声明 `execution_capabilities`，并条件化处理 supervisor.db / wave。
* **对应 Scenario**：无信号缺 db 非故障；wave_id→main Confirm；supervisor.db runtime-only。
* **外部可观察结果**：
  * `skills/ralph-run-diagnosis/SKILL.md` Phase 0 增加能力推断；
  * `references/artifact-manifest.md` / `artifact-discovery.md` / `verification-pipeline.md` / `report-template.md` 增加可选字段与步骤；
  * `mechanism-checklist.md` 如需补 wave Confirm 源一行。
* **输入与输出**：run_dir + preset → 报告 §0 含 capabilities。
* **可依赖**：U1 信号表。
* **禁止依赖**：无（不依赖 U6 fixture）。
* **验收测试**：`test_diagnosis_report_template_has_execution_capabilities`；`test_diagnosis_wave_confirm_main_ledger_guidance`；`test_diagnosis_missing_supervisor_db_not_fault_without_signal`。
* **Red 预期失败原因**：模板/流程无 capabilities 或仍暗示「无 supervisor.db = 坏」。
* **最小实现范围**：diagnosis skill 树内文档；不改 Rust。
* **集成验证**：契约测试。
* **回归范围**：Tier S 唯一 events 指针等 ssot-guardrails 不变。
* **完成标准**：三 Scenario 可逐步执行；测试绿。
* **风险与注意事项**：报告仍禁止指导 agent/诊断者把 supervisor.db 当业务事实源。

---

### Unit 8 — 横切回归、去点名化收尾、质量门禁

* **Unit 目标**：确保通用性约束落地；旧路径不回归；安装与文档交叉链接完整。
* **对应 Scenario**：禁名称门控；既有 fixtures 回归；AE1–AE6 总检。
* **外部可观察结果**：
  * 契约测试增加全量扫描：`ralph-preset-author` / `ralph-preset-review` / `ralph-run-diagnosis` 中 **本计划新增标题下** 无「名称以 `ce-executor-supervisor`」门控；
  * author/review/diagnosis 互相链接 Intent ↔ audit ↔ capabilities；
  * `patterns.md` 若提及 supervisor，仅作拓扑样例且标注「非门控」。
* **可依赖**：U1–U7 全部完成。
* **禁止依赖**：无后续 Unit。
* **验收测试**：`test_no_new_preset_name_gates_for_supervisor_wave`；全文件 `test_execution_model_contract.py`；`test_install.py`；对旧负例 `ralph preset check` 冒烟（至少 `aaf-review-negative-fixture`）。
* **Red 预期失败原因**：残留点名门控或断链。
* **最小实现范围**：扫描失败处的文案清理 + 测试钉死。
* **集成验证**：`pytest skills/tests/test_execution_model_contract.py skills/tests/test_install.py`。
* **回归范围**：手动确认 review `3b` CE pipeline 段仍在；single-chain-first 仍 mandatory。
* **完成标准**：§6 最终质量门禁全部勾选。
* **风险与注意事项**：不要顺手重构 3b；不要改 `crates/ralph-core/data/*.md`。

---

## 6. 最终质量门禁

执行 Agent 在宣称完成前必须满足：

* [ ] 计划内全部 BDD Scenario 在文档/fixture/测试层有对应证据
* [ ] `pytest skills/tests/test_execution_model_contract.py` 通过
* [ ] `pytest skills/tests/test_install.py` 通过
* [ ] 两份新 fixture 的 `ralph preset check -H … --strict` 可运行（不崩溃）；README 软性矩阵完整
* [ ] 抽查：Intent 缺省路径文案 = 用户否认 → `single-chain`
* [ ] 抽查：review 对无信号 preset 写明 Wave/Supervisor audit = N/A
* [ ] 抽查：diagnosis 报告模板含 `execution_capabilities`
* [ ] 新增段落无 preset **名缀门控**；未削弱既有 CE pipeline 3b
* [ ] 无新增 skip/xfail；无删除既有断言
* [ ] Lint：markdown 链接相对路径有效（references 互通）
* [ ] **未验证 / 剩余风险**（必须写入完成说明）：
  * 软性 AAF 依赖操作者按 skill 执行，无 LLM 自动判分 E2E
  * 未将 CE pipeline 3b 泛化为 capability 规则（显式 follow-up）
  * 未改 Rust lint；部分 supervisor 规则仍仅 review-only
  * 与 `2026-07-22-001` wave runtime 计划正交；若 001 落地后 Confirm/ticket 语义变化，需另开 skill 同步计划

---

## 附录 A — 建议菜单文案（Author Discovery）

```text
多个工作单元怎么推进？
1) 同一条主链顺序推进；并行只在执行 hat 内部拆分（推荐）— 默认路径，复杂度最低
2) 主链上某步需要对多份同构工作做并行 fan-out（wave）
3) 需要 runtime 管理多 slot / worktree / 排队与 fan-in（supervisor）
4) 自定义（请用一句话描述可观察的成功/失败条件）
```

用户选 3 后如仍不确定是否 wave，可追问一轮：是否需要同 topic 批并行 fan-out → 得到 `supervisor` 或 `supervisor+wave`。

---

## 附录 B — 主要改动文件清单（按 Unit）

| Unit | 文件 |
|------|------|
| 1 | `skills/ralph-preset-common/references/agent-native-model.md`, `author-checklist.md`, `skills/tests/test_execution_model_contract.py`（新建） |
| 2 | `skills/ralph-preset-author/SKILL.md`, `author-checklist.md`, 契约测试追加 |
| 3 | `author-checklist.md`, `ralph-preset-author/SKILL.md`, 契约测试追加 |
| 4 | `skills/ralph-preset-common/references/finding-rubric.md`, 契约测试追加 |
| 5 | `skills/ralph-preset-review/SKILL.md`, 契约测试追加 |
| 6 | `fixtures/aaf-wave-capability-negative-fixture.yml`, `fixtures/aaf-supervisor-capability-negative-fixture.yml`, `fixtures/README.md`, review 3c 列表 |
| 7 | `skills/ralph-run-diagnosis/SKILL.md`, `references/artifact-manifest.md`, `artifact-discovery.md`, `verification-pipeline.md`, `report-template.md`, 可选 `mechanism-checklist.md` |
| 8 | 文案收尾 + 全量契约/install 回归 |

安装后本地 `.claude/skills` 需跑 `skills/install.py ralph-preset-author ralph-preset-review ralph-run-diagnosis`（或全量）以同步物理拷贝；**不要**手改安装副本作为 SSOT。

---

## 附录 C — Executor 启动检查单

1. 读本计划 §1–§4 与附录 A/B。
2. 从 **Unit 1** 开始：先写契约测试 → Red → 改文档 → Green。
3. 禁止跳 Unit；禁止并行改 U6 fixture 与 U2 菜单「顺便做」。
4. 每 Unit 结束更新简短完成笔记（可选 `.ralph/tasks/` 或计划下方 checkboxes）。
5. 全部完成后对照 §6 门禁输出剩余风险。
