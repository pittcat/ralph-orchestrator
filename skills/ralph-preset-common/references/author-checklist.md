# Author Checklist

## 阶段 0：主动发现与用户确认（强制）

- [ ] 先读用户需求、现有 preset/schema、相邻文档和仓库约定；没有把仓库可查事实反问给用户
- [ ] 把缺口分为：仓库事实（自己查）、author 实现选择（自己推导并推荐）、用户意图选择（菜单确认）
- [ ] 对会改变业务结果、验收条件、修改权限、事实源、artifact 责任、失败行为或独立评审要求的缺口，使用交互选择菜单提问
- [ ] 每题提供 2–4 个互斥选项；推荐项排第一并说明影响；允许用户自定义答案
- [ ] 每轮优先只问 1–3 个相关问题；根据回答继续发现，不一次性倾倒静态问卷
- [ ] 遇到「适当处理」「必要时」「上游决定」等不可观察答案，继续追问为可执行、可验收的选择
- [ ] 不让用户替 author 决定 hat 数、topic 名或内部 topology；除非它们本身是用户明确的产品约束
- [ ] 已回显 `Preset Intent Confirmation`：目标、操作路径、输入/事实源、成功、阻塞、修改范围、独立评审、artifact/消费者、非目标、author 假设
- [ ] **执行模型菜单与 Intent Confirmation 字段**（仅当变更涉及实质拓扑 / 并行 / 多 unit / 未声明编排方式时强制；窄机械编辑可在笔记里记录推断来源后跳过）：
  - [ ] 提问菜单覆盖 `single-chain`（推荐首项）/ `wave` / `supervisor` / `supervisor+wave` 四选项，并允许自定义答案
  - [ ] 不可观察答案（「适当并行」「必要时用 supervisor」）已 grill 回到上述四选项后再继续
  - [ ] 用户否认 wave / supervisor → 一律锁定 `single-chain`，并在 Intent 写入 `execution_model: single-chain` 与 ≤50 字 why；后续拓扑不得引入 `event_loop.supervisor.enabled: true`、dispatcher 不得调 `ralph wave emit` / `ralph wave verify`
  - [ ] 选定 wave / supervisor(*) 时，按模型分支填对应 Hard questions（见「Hard questions — wave fan-out」/「Hard questions — supervisor orchestration」段）
  - [ ] YAML 能力信号（`event_loop.supervisor.enabled` / hat instructions 含 `ralph wave emit`）与 Intent.execution_model 一致；不一致按 `finding-rubric.md`「Wave / Supervisor capability audit」 段 `preset.execution_model_intent_mismatch` 入 review 主表
- [ ] 新 preset 或实质行为变更已通过最终菜单获得明确确认：「确认并开始设计 / 返回修改 / 暂停」
- [ ] 用户未确认或仍有重大歧义时已 STOP，未起草 YAML/schema

### 提问菜单示例（按真实缺口选用，不得照抄为固定问卷）

```text
1. 当独立评审发现必须立即修复的问题时，流程应该怎样结束本轮？
   1) 进入有界修复并再次独立评审（推荐）— 保证修复结果重新过门禁
   2) 记录问题并以 blocked 结束 — 不允许流程自行修改
   3) 生成报告后结束 — 只保留发现，不阻断完成状态
   4) 自定义
```

### Preset Intent Confirmation 模板

```markdown
## Preset Intent Confirmation

- **目标：**
- **操作者与启动路径：**
- **输入与事实源：**
- **成功条件：**
- **阻塞条件：**
- **允许的修改范围：**
- **必须独立执行的评审：**
- **重要 artifact、生产方与消费者：**
- **execution_model：** single-chain | wave | supervisor | supervisor+wave
  **why：** ≤50 字；锁定该执行模型的业务理由（例如「无并行/无 supervisor 需求，单链即可」 / 「需要同 topic 批并行 fan-out」 / 「需要 runtime 管理 slot / worktree / 排队」）
- **非目标：**
- **Author 推导与假设：**
- **用户确认：** 已确认 / 返回修改 / 暂停
```

> **`execution_model` 字段硬规则**：枚举四个值已冻结在 `agent-native-model.md`「执行模型（Execution Model）」段。Author 必须在 Intent Confirmation 里填写一个值并附 ≤50 字 why；review 端按该字段与 `agent-native-model.md` 检测信号对照，YAML 与 Intent 不一致（如 `event_loop.supervisor.enabled: true` 但 Intent 写 `single-chain`）按 `finding-rubric.md` 「Wave / Supervisor capability audit」 段 `preset.execution_model_intent_mismatch` finding 入主表。用户否认 wave / supervisor → 一律锁定 `single-chain`，不得暗中升级。

## 双阶段大脑（强制）

### 阶段 1：拓扑（作者视角 OK）

- [ ] 判定路径：local（`.ralph/hats/*.yml`）vs builtin（`presets/en/` + `presets/schemas/`）
- [ ] 记录 `execution_mode`；4+ hat → 必须 `isolated`
- [ ] 读 schema SSOT：`presets/schemas/<name>.yml`（builtin）或 preset 内 `event_policy.schemas`
- [ ] 画事件流（topic 箭头，非 prompt 流）
- [ ] 每条 handoff 边：上游 Q4 emit 字段 ↔ 下游 Q2 Observe 命令/字段
- [ ] **Git handoff 纪律（写入型 hat）**：每条写入型 hat（executor / fixer / 等）在 emit 终态 topic（work.done / fix.done 等）前必须有"两阶段 Git handoff precheck"：Stage A 计算 final HEAD + commit 状态 + clean worktree（在 policy-check 之前），Stage B 在真实 emit 前重检 HEAD / clean（防止 policy-check 与 emit 之间的窗口被 hook / subagent 篡改）；fabricate `worktree_status: clean` 是 contract violation。
- [ ] **Git handoff 纪律（只读 hat）**：每条只读 review / alignment hat 在 trigger 时必须先做"Entry Precheck"：expected_head_sha（dim：executor/round tip；alignment：fixer `head_sha`，无 fix 时才是 executor tip）必须等于 actual_start_head_sha（git rev-parse HEAD），porcelain filter（排除 `.ralph/`）必须为空；violation 时走 **Handoff failure emit**（发本 hat 唯一允许事件 + `handoff_precheck_failed`，禁止 silent-stop；不可替上游 commit / restore / stash / reset）；emit 前再做 Exit Precheck 两阶段重检；start / end evidence 必须写 `.ralph/review/<plan>/{round-<NN>/}git-state-<hat>-{start,end}.txt`，不污染 Git 状态。
- [ ] `state_projection.actions` 与 emit payload 字段对齐
- [ ] **Artifact-First topic 判别**：每条 emit topic 的字段都先判断是否属于「完整结果 / 长内容 / 跨 hat 摘要 / 关键决策依据 / 验证证据 / 高成本重建」；若是，完整内容必须由实际执行的 hat 或其 sub-agent 写入当前 `.ralph/` 下的业务 artifact，event payload 不得直接搬运，只保留路径、短状态或摘要、必要身份与路由字段（event 是控制面，文件是数据面）。
- [ ] **Artifact 产出责任**：每条写入型 hat（executor / fixer / sub-agent 拥有方）必须显式声明本 activation 会写入的 artifact 路径集合；sub-agent 的完整结果、证据与未解决问题必须先落盘，再只返回短状态、摘要和路径；不得把 preset 描述为文件创建者。
- [ ] **Artifact 消费与生命周期**：每条消费型 hat 必须显式声明从哪个可见路径读取 artifact，不得依赖 prompt 中的长文本；每份重要 artifact 还要指定消费方以及最终保留、归档或清理责任。
- [ ] 每个 agent-authored emit topic 的 `event_policy.schemas.<topic>` 已检查：required handoff / identity / verdict / count / path / reason 字段有 `field_docs`，高风险 topic 有不会伪造业务事实的 `examples`
- [ ] 若 hat `publishes` 含 `review.dimensions.complete`，`state_projection.actions_chain` 须有对应投影 action（否则下游 Q2 看不到 review 汇总）
- [ ] emitter 若 instructions 要求 `--triggered <hat>`，该 `<hat>` 必须在 preset `hats[]` 里声明（否则 runtime 拒收 `triggered_not_in_topology`）
- [ ] loop preset 中 `fix.done.next_review_plan` 必须是非空 object 合同；schema、example、fixer instructions 都不能允许 `null`
- [ ] `dim:*` / `dimension-reviewer` 只读 reviewer 若禁用 Edit/Write，不得声明 `docs/plans/` 写路径；review 产物写到 `.ralph/review/**`
- [ ] 可参考 `references/patterns.md`（仅拓扑阶段）

### 阶段 2：起草（单 hat agent 视角）

- [ ] 写 hat X 的 `instructions:` 时 **只扮演 hat X 的 agent**
- [ ] 逐 hat 填 AAF 五问表（模板见下）
- [ ] 禁止拓扑句式抄进 instructions
- [ ] Emitter hat：引用 `ralph-tools-opac`、`ralph-tools-emit` §5；强制 `--policy-check`
- [ ] Emitter hat：若 instructions 提到 payload / required fields / field shape / `ralph emit` / `ralph wave emit`，必须引用 `ralph-tools-emit`「Policy-Check 反馈」；不要复制 `field_docs` 表
- [ ] Recovery / correction 路径：引用 `ralph-tools-recovery-directives`（通用 bounded retry）；preset 内用**触发状态表**写专用动作，不复述 data skill 全文
- [ ] 终态报告类 hat：正文面向决策者、技术附录面向核验证据者；正文不得是 payload 字段流水账，失败路径不得写成 silent-success
- [ ] `task_id` / `task_key` / `step`：引用 `ralph-tools-tasks` red box
- [ ] 不复述 `ralph-tools*.md` 参数表
- [ ] **对每个 emit topic，按 payload audit 五列填行**（见下）—— schema 通过不等于字段可达
- [ ] **对每个 payload 字段，反查 schema metadata**：`field_docs.meaning/source/fill_rule` 与 Payload Contract 的值源、可见性、下游消费一致
- [ ] **收尾双事件终态**：若 hat 是 preset 的**收尾 hat**（典型为 reporter / alignment），允许其在同 activation 内先发 `event_loop.required_events[]` 中的 topic、再发 `event_loop.completion_promise`；其它 hat 的 `publishes` 一律**不得**同时包含 required_events 列表里的 topic 与 completion_promise，违反将触发 reviewer 的 P0。复核条件见 `finding-rubric.md`「required-event-to-completion 窄例外」段。

**Artifact-First 单 hat 视角审核项（每 hat 必填）**

- [ ] **Q2 Observe**：消费重要信息时，命令或动作必须能从当前 hat 可见输入取得路径，并直接读取当前 `.ralph/` 下的业务 artifact；不得从 trigger payload 反查完整结果、长内容或长解释字段，也不得读取 runtime internal ledger。
- [ ] **Q3 执行顺序**：产出重要信息时必须明确「实际执行的 hat 或其 sub-agent 先写 artifact → hat 验收文件 → policy-check → emit 携带路径的 event」；不得先 emit 再补文件。涉及命令语法只引用 `ralph-tools-emit`「Policy-Check 反馈」及相应章节，不复制参数表。
- [ ] **Q4 Payload Contract**：每行填写 `artifact 落盘`，包括「必填 / 可选 / 不需要」与路径格式约定（例如 `.ralph/<plan>/<unit>/<file>.md`）；实际路径可由任务设计决定，不要求统一目录结构。
- [ ] **Q5 handoff closure**：必须形成「emit 路径字段 → projection / 当前 hat 可见输入 → 下游 hat 读取 artifact → 消费确认」完整链路，并写明 artifact 的产出方、消费方以及最终保留 / 归档 / 清理责任。
- [ ] **不落盘例外**：只有短暂、短小且无需恢复的信息可例外；必须在 Payload Contract 同行的 `artifact 落盘` 列写「不落盘 + 理由」，并以恢复价值、审计价值和下游依赖为判据，不能只看字符数。

## AAF 五问表模板（每 hat 必填）

```markdown
## Hat: <id>

- **Q1 使命:** …
- **Q2 输入 (Observe 命令 + 期望字段):** …
- **Q3 执行 (OPAC 命令序列):** Observe → Precheck → Apply → Confirm
- **Q4 输出 (topic + payload 合同):** 见下方 Payload Contract 表
- **Q5 交接 (emit 字段 → 下游 Observe 路径):** …
```

**不可交付信号：** 任一为空；含「待定」「同上」「上游会处理」。

## Payload Contract 表模板（每 emit topic 必填）

每个 hat 至少填一张；多 trigger 须按 trigger 拆分。

```markdown
### Hat: <id> — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `<topic>` | `task_id` | string | `ralph tools task list` → 当前 active task | `## ORCHESTRATOR CONTEXT` | 必须 live；禁手写 | reviewer 决定后续 fix / block | `field_docs.task_id.source` 指向 live task list；`fill_rule` 禁手写 | 必填 · 关联的完整任务结果写到 `.ralph/<plan>/tasks.jsonl`，event 仅携带 live `task_id` 与 artifact 路径 |
| `<topic>` | `verdict` | enum | 本 hat work 输出 | `## HAT IDENTITY` trigger payload | 不涉及 | 同上 | `field_docs.verdict.meaning` 解释判定语义；`allowed_values` 列枚举 | 必填 · `reason` 若为长解释，落盘到 `.ralph/<plan>/decisions/<unit>.md`，event 只带短 verdict、短 reason 与路径 |
| `<topic>` | summary field（如 `must_fix_now_count`） | 整数 | 当前 trigger payload 字段 | `## TRIGGER CONTEXT`（preset/schema `trigger_context.summary_fields` 声明） | 不涉及 | 本 hat 决定路由分支 | schema `trigger_context.summary_fields` 列声明字段；missing 渲染为 `<missing>` | 可选 · 若是可立即重算的短计数，写「不落盘 + 无恢复、审计或历史依赖」 |
| `<topic>` | matched hint guidance | 字符串 | `trigger_context.routing_hints` 命中条目 | `## TRIGGER CONTEXT`「Matched routing hints」段 | 不涉及 | 本 hat 行动指导 | `routing_hints[*].label` 唯一；同 `exclusive_group` 内不可同时命中 | 不需要 · 短暂路由指导，无恢复、审计或下游历史依赖 |
| `<topic>` | `artifact_path` | path | 本 hat 或其 sub-agent 本 activation 实际写入的业务 artifact | 本 hat 写入结果；下游从 projection / trigger 可见字段取得 | 如与 task 绑定则校验 live identity | `<consumer-hat>` 读取完整结果并确认消费；`<owner>` 负责最终保留 / 归档 / 清理 | `field_docs.artifact_path` 说明文件语义、值源和修复方式 | 必填 · `.ralph/<plan>/<unit>/<file>.md`；完整结果先落盘，event 只传此路径与短控制字段 |
```

**新行示例使用规则：** 每个承载重要信息的 topic 至少增加一行实际 artifact 路径字段；不得因为 payload 已出现路径就省略文件语义、消费方、消费动作或生命周期责任。

**Trigger Context 审核项（适用于 trigger-consuming hats）**

- [ ] **声明位置**：`trigger_context` 写在 `event_policy.schemas.<topic>` 下，不要放在 `hats[*].instructions` 或独立 sibling。
- [ ] **`summary_fields` 值源**：每条声明字段必须来自 trigger payload，且在 schema `required_fields` ∪ `known_fields` ∪ `field_docs.keys()` ∪ `allowed_values.keys()` 内。
- [ ] **`known_fields` 缺口**：若 hint 条件或 summary 字段引用非 required 字段，须把它加入 schema `known_fields`，不要靠放宽 lint。
- [ ] **`routing_hints` 形状**：每条 hint 用 `conditions: [{field, op, value}]` 形式，`op` 仅允许 `eq` / `ne` / `gt` / `gte` / `lt` / `lte` / `exists` / `missing`；数值比较 op 的 `value` 必须是 number；`exists` / `missing` 不带 `value`。
- [ ] **`label` 唯一性**：同 topic 内 hint `label` 不重复；同 `exclusive_group` 内 hint 条件须可静态判定为互斥。
- [ ] **拓扑消费方**：只为 `hats[*].triggers` / `subscribes_to` 包含该 source topic 的 hat 注入；无消费者触发 `trigger_context_no_consumer`。
- [ ] **guidance 是 agent 行动**：hint `guidance` 用「你应该如何处理」表达，不要写 runtime 控制命令、不要修改 routing / 权限。
- [ ] **instructions 不复制**：hat `instructions` 只引用 `## TRIGGER CONTEXT` 区块，不要把 hint 条件值复制进 instructions。

**拒交付信号：**

- 字段值源写「上游会处理」「待定」「约定俗成」
- `task_id` / `task_key` / `step` 字段未标 `live required`
- 多 trigger hat 合并成一行（必须按 trigger 拆分差异）
- 决策字段（`verdict` / `reason` / `summary` / `next_action`）无下游消费说明
- 某字段无任何一行说明它对 hat 可见
- required handoff / identity / decision 字段没有 `field_docs`，也没有说明为什么现有 injected skill 已覆盖
- `examples` 填了业务结论占位（例如固定 `0` / `pass`），而不是安全示例
- event payload 直接携带完整结果正文（尤其是超过 200 字符，或任意长度但具有恢复、审计、下游依赖价值的内容）；字符数仅是提示，不是主判据
- emitter hat instructions 未要求「先写业务 artifact，再 emit 携带路径的 event」
- consumer hat instructions 未要求从当前可见路径读取完整内容，仍依赖 prompt 或 trigger payload 中的长文本
- artifact 路径未指定消费方、消费动作或最终保留 / 归档 / 清理责任
- event payload 把 `.ralph/events.jsonl`、`.ralph/loops.json`、`.ralph/supervisor.db` 等 runtime internal ledger 路径当作业务 artifact
- 「不落盘」例外只以「字符很少」为理由，没有说明恢复价值、审计价值和下游依赖

## 交 review 前门禁

- [ ] `preset-author-notes.md` 首部包含已确认的 `Preset Intent Confirmation`（新 preset 或实质行为变更）
- [ ] 每 hat 一张 AAF 表 + Payload Contract 表，写入 `preset-author-notes.md`
- [ ] hat 表数 == YAML hat 数；每 emit topic 都填了 Payload Contract 行
- [ ] Payload Contract 的 `schema metadata` 列已同步到 `presets/schemas/<name>.yml` 或 preset inline `event_policy.schemas`
- [ ] Emitter instructions 引用 `ralph-tools-emit`「Policy-Check 反馈」，不复制字段说明
- [ ] 自问：「若我只收到这份 instructions + Ralph 注入，能否做完 Q1？Q4 每个字段我能取到值吗？」
- [ ] **Artifact-First 路径闸口**：每条重要信息（完整结果 / 长内容 / 跨 hat 摘要 / 关键决策依据 / 验证证据 / 高成本重建信息）都有当前 `.ralph/` 下的对应业务 artifact 路径，且生产方在本 activation 写入，消费方能从当前 hat 可见输入取得该路径。
- [ ] **控制面最小化闸口**：event / message 字段只保留短状态、短摘要、artifact 路径、必要身份与路由字段；完整结果、证据、进度和决策依据留在文件中。
- [ ] **例外闸口**：每条「不落盘」都在 Payload Contract 同行给出可审核理由，并说明其短暂、短小、无需恢复且不承担审计或下游历史依赖；不能仅以字符数判断。
- [ ] 对 builtin 改动：列出 7 点同步清单（见下），不自动执行
- [ ] 建议调用 `ralph-preset-review`（不替代 `ralph preset check`）

## Hard questions — single-chain-first (2026-07-07-006 Unit 6)

任何 preset 起草阶段在「默认走单链」之前必须先回答以下 5 问：

1. **本 preset 的 unit 拆分能否由 executor 内部 subagent 完成？** ✓ / ✗ + ≤50 字理由
2. **任何业务 topic 是否超过一个消费者？** ✓ / ✗ + ≤50 字理由
3. **fallback 是否可能路由到 success？** ✓ / ✗ + ≤50 字理由
4. **是否有 hat 把 tasks / progress / recovery 当业务事实？** ✓ / ✗ + ≤50 字理由
5. **是否有 rescue hat 能改变业务链路？** ✓ / ✗ + ≤50 字理由

若测试稳定化 hat 可以修改生产代码，还必须确认：

- [ ] executor/fixer 的每个生产修改出口都先稳定化再独立 review；不存在直通 success/alignment。
- [ ] phase 是显式 payload 字段并逐跳透传；最终 review 不会开启无界修复轮。
- [ ] 稳定化事件携带输入/输出 SHA、审计、correction、统一计划/trace 身份和真实 worktree 状态。
- [ ] blocked 路径进入 Reporter/阻塞终态，不能降级为 accepted。

任一问 ✗ → 必须改写或显式说明为何单链无法表达（默认应迁移到 executor 内部 subagent）。见 `references/finding-rubric.md` 的「Single-chain-first audit」段。

## Hard questions — Artifact-First Handoff

任何 preset 交 review 前必须逐条回答；任一项不满足且没有符合例外条件的理由即拒绝交付：

1. **每条写入型 hat 是否声明了当前 `.ralph/` 下的 artifact 路径集合，且拓扑层没有把这些路径描述为「preset 创建」？** ✓ / ✗ + 证据
2. **每条 consumer hat 的 instructions 是否要求它从当前可见输入取得路径并显式读取 artifact，而不是依赖 prompt 中的长文本？** ✓ / ✗ + 证据
3. **每个被传递的完整结果、长内容或跨 hat 摘要是否都已先落盘，event / message 是否只保留短状态、短摘要、路径、必要身份与路由字段？** ✓ / ✗ + 证据
4. **是否有任何 hat 把 `.ralph/events.jsonl`、`.ralph/loops.json`、`.ralph/supervisor.db` 等 runtime internal ledger 当作自定义状态或 handoff 文件？** ✓ / ✗ + 证据；此项必须为 ✗
5. **每条声明「不落盘」的信息是否都标注了简短理由，并按恢复价值、审计价值和下游依赖解释，而非只按字符数判断？** ✓ / ✗ + 证据

预演 finding 时按 `references/finding-rubric.md`「Artifact-First Handoff finding_id」表入主表（review-only，不进 `ralph preset check` JSON）：`preset.artifact_path_not_in_visible_context` / `preset.artifact_no_consumer_declared` / `preset.artifact_no_lifecycle_owner` / `preset.artifact_uses_internal_ledger` / `preset.payload_carries_full_content` / `preset.artifact_first_field_docs_missing` / `preset.artifact_first_exemption_unjustified` / `preset.artifact_first_passed_on_path_presence` / `preset.subagent_result_returned_only_in_message` / `preset.artifact_described_as_preset_owned` / `preset.artifact_content_insufficient_for_decision`。完整默认 severity / confidence / aaf_question 见该表。

## Builtin 7 点同步清单（摘要）

改 `presets/en/<name>.yml` 或 `presets/schemas/<name>.yml` 事件拓扑后，逐层检查：

1. `crates/ralph-core/src/event_loop/mod.rs` — step-close / completion 语义
2. `crates/ralph-core/src/preset_lint/` — 相关 lint 规则
3. `crates/ralph-core/tests/scenarios/*.yml` + `scenarios.rs`
4. `crates/ralph-core/src/config/loop_config.rs`、`preflight.rs`、`config_resolution.rs`
5. `crates/ralph-cli/src/presets.rs` + SSOT 测试
6. `presets/manifest.yml`、`presets/index.json`
7. `CLAUDE.md` / `AGENTS.md`、`.cursor/rules/multi-hat-isolation.mdc`、`scripts/ralph-zsh-plugin.zsh`

详见 `docs/handbook/serial-preset-development.md`。

## 产出物

| 文件 | 位置 |
|---|---|
| Preset YAML | `presets/en/<name>.yml` 或 `.ralph/hats/<name>.yml` |
| `preset-author-notes.md` | 与 preset 同目录（默认）；含 AAF 五问表 + Payload Contract 表 |
