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
- [ ] **Key-stage event gate 0e 必备字段（按关键位置每行）**：
  - [ ] `key_stage` 注明 hat 与 handoff / 阶段分支的人类可读标识
  - [ ] `guard_selection` ∈ {`precheck`, `payload_consistency`, `both`, `neither`}
  - [ ] `precheck_guard` 布尔 `true` ⇔ `guard_selection ∈ {precheck, both}`
  - [ ] `precheck_retry_budget` 整数 3 / 2 / 1，`precheck_guard=false` 时填 `null`
  - [ ] `payload_consistency_guard` 布尔 `true` ⇔ `guard_selection ∈ {payload_consistency, both}`
  - [ ] `payload_consistency_retry_budget` 整数 3 / 2 / 1，`payload_consistency_guard=false` 时填 `null`
  - [ ] `reason` ≤80 字；选择 `neither` 或 budget 低于 3 必须有恢复 / 审计 / 下游依赖类理由
  - [ ] `confirmation_status` ∈ {`confirmed`, `pending`, `rejected`}；非 `confirmed` 即视为未确认
- [ ] **Key-stage event gate 与 Gate Scope 字段隔离**：0d 的 `hard/record/off` 与 0e 的 `guard_selection` / `precheck_guard` / `payload_consistency_guard` / `precheck_retry_budget` / `payload_consistency_retry_budget` 字段语义不混；不得把 Gate Scope `off` 字段当 0e 的关键位置选择。违规 → `preset.key_stage_event_gate_field_reuse` finding（参见 `finding-rubric.md`「Key-stage event gate」段，review-only）。
- [ ] **Key-stage event gate 两 budget 不共享**：不得把 `precheck_retry_budget` 与 `payload_consistency_retry_budget` 合并为一个 `retry_budget` 或共享 exhaustion state。
- [ ] **Task brief 前置输入对账**（仅当调用方提供 `task_brief_path` 时；书面规程见 `skills/ralph-task-discovery/references/author-handoff.md`，确定性参考实现 `author_handoff.py`）：
  - [ ] brief 路径与 validator 结论（`valid` / `author_ready` / `next_action`）已记录到 `preset-author-notes.md`
  - [ ] 复核顺序完整：文件存在 → YAML → `schema_version` → `project_root` 与当前目标项目根一致 → `brief_validator.validate_brief_text` → `status` / `author_ready`；任一失败输出 `task_brief_invalid` + validator code/path，停在 Discovery gate，不生成任何 preset YAML
  - [ ] brief 的 Goal、成功条件（acceptance）、阻塞条件（failure boundaries）、scope、Evidence refs 已引用进 Preset Intent Confirmation
  - [ ] selected candidate 仅取自 validator `candidate_gates` 结论为 `selected` 的候选；被 rejected 的候选不得被当作 selected 消费
  - [ ] 既有 Discovery / Intent Confirmation / AAF / Payload Contract / prompt visibility / pre-review gate / review handoff 门禁未因 brief 跳过或削弱

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
- [ ] **`event_loop.precheck`（若启用）producer 端契约**：`event_loop.precheck.enabled: true` 且当前 hat 已发布 `<X>.proposed` 时，`ralph emit <X>` 会被 CLI 透明改写为 `<X>.proposed`（U3 wiring）；写盘前的缺字段检查会按 guarded `<X>` schema 的 `required_fields` 跑（U4 wiring）。**作者**：`presets/en/*.yml` 的 `event_loop.precheck.rules` 必须与 `event_policy.schemas.<X>.required_fields` 对齐（否则 U4 不会拦截）；不要在 hat instructions 里写「手 emit `<X>.proposed` 必被拒」之类过时陈述；详见 `docs/guide/precheck-gates.md` 的「Producer 视角」段。
- [ ] **Artifact 产出责任**：每条写入型 hat（executor / fixer / sub-agent 拥有方）必须显式声明本 activation 会写入的 artifact 路径集合；sub-agent 的完整结果、证据与未解决问题必须先落盘，再只返回短状态、摘要和路径；不得把 preset 描述为文件创建者。
- [ ] **Recovery guidance contract**：`event_loop.precheck.rules.<X>.recovery_guidance` 与 `event_policy.payload_consistency.rules[].recovery_guidance` 可选。`recovery_guidance` 必须与 `on_fail` **同级**，must not nest recovery_guidance under on_fail（嵌套会被拒收）。precheck `by_check` key 必须是 `1..=prompt.len()` 的十进制字符串；consistency key 必须等于 rule `id`。rule key 必须是 hat 真实 `publishes` 的 topic。禁止在 guidance 里写 `suggested_command` / 成功 payload 模板。synthetic 拒绝只显示 `common`。
- [ ] **Artifact 消费与生命周期**：每条消费型 hat 必须显式声明从哪个可见路径读取 artifact，不得依赖 prompt 中的长文本；每份重要 artifact 还要指定消费方以及最终保留、归档或清理责任。
- [ ] 每个 agent-authored emit topic 的 `event_policy.schemas.<topic>` 已检查：required handoff / identity / verdict / count / path / reason 字段有 `field_docs`，高风险 topic 有不会伪造业务事实的 `examples`
- [ ] **Instructions ↔ schema required-fields SSOT 对账**：每个 `ralph emit <topic>` 示例的 payload 字段集合与对应 `event_policy.schemas.<topic>.required_fields` 完全一致；每个字段的占位值都能从当前 trigger、注入上下文或本 hat 产物取得，并在 `preset-author-notes.md` 记录 schema 行 + instructions 行证据
- [ ] 若 hat `publishes` 含 `review.dimensions.complete`，`state_projection.actions_chain` 须有对应投影 action（否则下游 Q2 看不到 review 汇总）
- [ ] **Triggered 路由硬规则**：`--triggered` 是事件目标 hat，不是来源 hat；普通业务 handoff 一律省略，让 isolated runtime / CLI 自动推导。显式使用必须证明是不同 hat 的必要直达例外，并在 author notes 记录目标、原因和拓扑证据；self-target 直接判为 P0。
- [ ] loop preset 中 `fix.done.next_review_plan` 必须是非空 object 合同；schema、example、fixer instructions 都不能允许 `null`
- [ ] `dim:*` / `dimension-reviewer` 只读 reviewer 若禁用 Edit/Write，不得声明 `docs/plans/` 写路径；review 产物写到 `.ralph/review/**`
- [ ] 可参考 `references/patterns.md`（仅拓扑阶段）

### 阶段 2：起草（单 hat agent 视角）

- [ ] **对每条 hat 跑 `ralph -c <preset>.yml inspect prompt --hat <id> --format json`**（详见 [`prompt-visibility.md`](prompt-visibility.md)），把 `auto_inject` / `on_demand` 作为该 hat 唯一可见性证据：
  - on-demand skill（`auto_inject[]` 不含、`on_demand[]` 含）→ instructions 必须显式让 agent `ralph tools skill load <name>`，**不得**写成「已自动注入」
  - auto-inject skill → instructions 可直接引用其章节名；**禁止**让 agent 再 `ralph tools skill load`
- [ ] **运行 `ralph inspect prompt --full --format json` 验证 preset instructions 在 prompt_body 中可见**（`prompt_body` 字段含完整 instructions + 注入 skill 拼接文本）
- [ ] 写 hat X 的 `instructions:` 时 **只扮演 hat X 的 agent**
- [ ] 逐 hat 填 AAF 五问表（模板见下）
- [ ] 禁止拓扑句式抄进 instructions
- [ ] Emitter hat：引用 `ralph-tools-opac`、`ralph-tools-emit` §5；强制 `--policy-check`
- [ ] Emitter hat：若 instructions 提到 payload / required fields / field shape / `ralph emit` / `ralph wave emit`，必须引用 `ralph-tools-emit`「Policy-Check 反馈」；不要复制 `field_docs` 表
- [ ] Recovery / correction 路径：引用 `ralph-tools-recovery-directives`（通用 bounded retry）；preset 内用**触发状态表**写专用动作，不复述 data skill 全文
- [ ] 终态报告类 hat：正文面向决策者、技术附录面向核验证据者；正文不得是 payload 字段流水账，失败路径不得写成 silent-success
- [ ] **操作者交付文件路径可见（硬）**：凡「本轮写操作者可读文件 + schema 要求路径字段」的 hat，payload 必须带真实路径字段；instructions 须自洽写明「先落盘 → `test -f` → `--policy-check` → 真实 emit → Confirm 打印 `DELIVERABLE_PATH:`」（不依赖 emit skill 已注入）；schema 只验字段非空、不验文件系统
- [ ] `task_id` / `task_key` / `step`：引用 `ralph-tools-tasks` red box
- [ ] **task authority 单写者**：若 preset 在 `event_loop.state_projection.actions` 配了 task-creation action（例如某 handoff → `ensure_task_batch`），对应 hat 的 instructions **不得**再调用 `ralph tools task add` / `task ensure` 走 CLI；lint `preset.instructions_task_mutation_authority_conflict` 会拒收。修补：从 hat instructions 删除 CLI mutation，统一走声明的 payload-backed 或 artifact-backed handoff；两种 task authority 不得混用。
- [ ] **task 三语义分离**：`tasks.coordinator_hats` 只授予 lifecycle administration（close/fail/reopen），不授予 execution ownership；非 owner coordinator 的 instructions 只能调度/管理，禁止 `task start` 或实现该 task。Prompt `[read-only]`、CLI start 与 wave dispatcher 检查必须对同一 owner/loop/task 状态得出一致结论。
- [ ] **task close 投影单写者**：若 preset 在 `event_loop.state_projection.actions` 配了 task-close action（例如 `exec.unit.done` → `close_task`），对应 hat 的 instructions **不得**再调用 `ralph tools task close` 走 CLI；emit 该事件即原子关闭 task。修补：从 hat instructions 删除手工 close 文字，改为「emit 即关闭，禁止手工 close」。
- [ ] 不复述 `ralph-tools*.md` 参数表
- [ ] **对每个 emit topic，按 payload audit 五列填行**（见下）—— schema 通过不等于字段可达
- [ ] **对每个 payload 字段，反查 schema metadata**：`field_docs.meaning/source/fill_rule` 与 Payload Contract 的值源、可见性、下游消费一致
- [ ] **收尾双事件终态**：若 hat 是 preset 的**收尾 hat**（典型为 reporter / alignment），允许其在同 activation 内先发 `event_loop.required_events[]` 中的 topic、再发 `event_loop.completion_promise`；其它 hat 的 `publishes` 一律**不得**同时包含 required_events 列表里的 topic 与 completion_promise，违反将触发 reviewer 的 P0。复核条件见 `finding-rubric.md`「required-event-to-completion 窄例外」段。
- [ ] **临时资源清理交接**：若 workflow 创建 worktree、临时 branch 或其它可诊断资源，必须由独立 cleanup owner 在报告/终态收敛 owner 前消费成功/失败路径；cleanup event 必须携带逐资源结果和 pending 数量，报告/终态收敛 owner 只消费 cleanup artifact，不得与 cleanup 竞争删除权。
- [ ] **全路径 vs 成功脊门禁**：`event_loop.required_events` 只放所有完成路径（含失败早退）都会经过的收敛 topic；成功脊专用 handoff（如 `work.done` → `plan.complete`）用 `event_loop.path_required_events`，不要塞进 `required_events`（否则 `topology.required_event_not_on_all_paths`，且 runtime 会误拒失败路径 `LOOP_COMPLETE`）
- [ ] **Paired completion 字段一致性**：若 preset 需要 `LOOP_COMPLETE` 与前置终态事件（如 `forge.report.done`）携带相同路径字段，启用 `event_loop.completion_payload_match` 并声明 `topic` + `fields`；reporter instructions 必须写明「resume 时不得重写既有报告事实，只能补发匹配 completion」。

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

**Payload Consistency 审核项（适用于声明 `event_policy.payload_consistency.rules` 的 preset）**

- [ ] **每个 `rule.topic` 在 `event_policy.schemas` 内**：所有规则的 `topic` 都必须在 schema map 里声明；未声明的 `topic` 会被 lint 拒收为 `preset.payload_consistency_unknown_topic`。
- [ ] **`when` 引用的 `field` 在该 topic schema 字段并集内**：声明字段必须出现在 `required_fields` ∪ `known_fields` ∪ `field_docs.keys()` ∪ `allowed_values.keys()` ∪ `element_constraints` 中；未声明字段会让谓词永不命中（runtime 视为 miss），lint 拒收为 `preset.payload_consistency_unknown_field`。
- [ ] **`rule.id` 在 preset 内唯一**：`id` 是 runtime `payload_consistency:<id>` gate 的稳定标识；重复的 `id` 会被 lint 拒收为 `preset.payload_consistency_duplicate_id`，并使 agent 收到的拒收原因变得无法解析。
- [ ] **`when.op` 在白名单内**：`op` 只能是 `eq` / `ne` / `gt` / `gte` / `exists` / `non_empty`；其它 op 会被 lint 拒收为 `preset.payload_consistency_unknown_op`，runtime 也会直接拒收。
- [ ] **`when` 是 object 形态**：单谓词 `{field, op, value}` 或组合 `{all:[...]}` / `{any:[...]}`；非 object `when` 会被 lint 拒收为 `preset.payload_consistency_non_object_when`。
- [ ] **`rule.message` 安全且有界**：`message` 是不可信诊断数据（不是 agent 指令），长度 ≤ 1024 UTF-8 bytes，不含 ANSI escape / C0/C1 控制字符 / 零宽字符；违规会被 lint 拒收为 `preset.payload_consistency_unsafe_message`。runtime 对绕过 lint 的旧/动态配置会按 code-point 边界安全截断并标记 `truncated=true`。
- [ ] **mode/action parity**：`mode: observe` 与 `mode: enforce` + `on_violation: warn` / `reject_with_resume` 在 Precheck (`--policy-check`) 与 Apply 路径行为一致——不要假设 CLI 会私自升级 Warn 为 fatal；runtime 由 `PolicyDecision` 统一决定。authoring 时按业务意图选 mode/action，不需要为 Precheck/Apply 分叉写额外配置。
- [ ] **`referenced_fields` 由 predicate AST 自动派生**：`ValidationError` 的 `referenced_fields` 是 runtime 从 `when` 谓词 AST 遍历得到的稳定字段路径数组（按声明顺序去重），agent 据此定位需修复的字段；author **不**在 rule 里手写 `referenced_fields`，也**不**把 gate ID 塞进 `field`。
- [ ] **`gate` 与 `field` 分离**：拒收反馈里 `gate` 字段携带 `payload_consistency:<rule_id>` 标识，`field` 字段只承载业务字段名（组合规则可为首个稳定字段）；不要在文档或 instructions 中暗示 agent 从 `field` 解析 gate ID。

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
- emitter instructions 的 emit 示例少于 schema `required_fields`、字段名漂移、引用错误上游 topic 或字段值无可见来源
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

## Hard questions — wave fan-out

> **触发条件**：`execution_model ∈ {wave, supervisor+wave}`。`single-chain` preset **不**适用本段（标记 N/A 而不是留空）；详见下方「N/A 规则」段。
> **目的**：在 topolo­gy 选 wave 前,把 hat 视角可答的问题先钉死;这些是 hat 在自己那一轮 activation 里能 Observe / 能调用的能力,不是 framework 内部细节。
> **命中时**:`ralph-preset-review` 按 `finding-rubric.md`「Wave capability audit」段逐项判定;`preset.wave_*` 系列 finding 默认 P0（详见 rubric）。

1. **唯一 dispatcher**：本 preset 中**只有 1 个 hat**（典型为 executor / dispatcher）允许调用 `ralph wave emit` / `ralph wave verify`；其它 hat 一律通过 dispatcher 走事件流。✓ / ✗ + 列出 dispatcher hat id + 证据
2. **worker 禁 `wave emit`**：所有非 dispatcher hat 的 `instructions:` 中不得出现 `ralph wave emit` 字样（哪怕只是示例）。✓ / ✗ + grep 证据
3. **`wave verify` → emit**：dispatcher 必须先 `ralph wave verify --payloads-stdin`（业务事件零写盘；**通过后写一次性 ticket**），再用**未改动的同一批** payload 跑 `ralph wave emit --payloads-stdin`；禁止把 `wave emit --policy-check` 当成 wave Precheck，也不在同一调用里合并预检 + 写盘。✓ / ✗ + 引用 `ralph-tools-wave` Wave OPAC / Policy-Check 反馈章节
4. **Confirm 用 main ledger**：worker 完成态由 dispatcher 通过 `ralph events --events-source main` 对账，**不**用 hat-channel（hat-channel 是 dispatcher's own 私有落盘点，不是 worker 的 Confirm 入口）。✓ / ✗ + 列命令
5. **禁 agent 发协调 topic**：worker / dispatcher 一律不得 publish 任何 `wave.*` / `exec.wave.*` 协调 topic；这些由 runtime / supervisor 管。✓ / ✗ + grep `publishes`
6. **batch 失败可定位**：`ralph wave verify` 拒收时 policy-check JSON 必须含 `payload_index`；dispatcher 必须按 index 定位失败 item 并精准修复（而不是整批重发）。✓ / ✗ + 引用 `ralph-tools-wave`「Policy-Check 反馈」段
7. **emitter cite skill**：dispatcher / worker 涉及 `ralph wave emit` / `ralph wave verify` 时必须引用 `ralph-tools-wave` 与 `ralph-tools-emit` §5「Policy-Check」反馈，**不**复制参数表。✓ / ✗ + 引用章节名

任一问 ✗ → 必须改写或显式说明为何 wave 无法表达。完整 finding 默认 severity / confidence / aaf_question 见 `finding-rubric.md`「Wave capability audit」段。

## Hard questions — supervisor orchestration

> **触发条件**：`execution_model ∈ {supervisor, supervisor+wave}`。`single-chain` 与 `wave` preset **不**适用本段（标记 N/A）。
> **目的**：把 supervisor 视角下 hat 能 Observe / 调用的边界先钉死; supervisor 内部 ledger / 队列 / slot 调度由 runtime 管控,hat 不得越界。

1. **`supervisor.enabled` + isolated**：preset `event_loop.execution_mode: isolated` 且 `event_loop.supervisor.enabled: true`。✓ / ✗ + 引用字段路径
2. **禁读 / 写 `supervisor.db` 作业务接口**：任何 hat `instructions:` 不得要求把 `.ralph/supervisor.db` 当业务 artifact 接口（写或读）。需要 unit 状态 → 走 `ralph tools task list` 或读业务 artifact；需要 history → 走 `ralph events --events-source main`。✓ / ✗ + grep
3. **禁发 coordination topic**：所有 hat `publishes` 一律不得包含 `exec.wave.*` / `slot.*` 等 supervisor 协调 topic；这些由 runtime emit。✓ / ✗ + grep
4. **unit 状态经 task API / 业务 artifact**：每个 sub-unit 必须有 live `task_id`（`ralph tools task list` 取得,禁手写),或 sub-unit 进度落业务 artifact 由 dispatcher 汇总。✓ / ✗ + 列命令路径
5. **timeout / partial 有业务可见出口**：supervisor 触发 partial / timeout 时必须通过 dispatcher 发 `plan.blocked` 或 `work.failed` 等业务可见事件；不得 silent-success 留在 supervisor 内部。✓ / ✗ + 列事件与 schema 字段
6. **与 Intent 一致**：preset `event_loop.supervisor.enabled` 必须与 Intent.execution_model 一致;不一致按 `finding-rubric.md` 「Supervisor capability audit」 段 `preset.execution_model_intent_mismatch` 入 review 主表。✓ / ✗ + 字段值
7. **wave consumer `concurrency > 1`**：每个 `triggers:` 含 `*.unit.ready` 的 hat 必须显式声明 `concurrency > 1`；缺省（=1）会被 wave detector 静默拒收（`SequentialTarget`），整个 batch 不会被分发。✓ / ✗ + 列出每个 wave consumer hat 的 `concurrency` 值
8. **`slot_retry_budget > 0` 时 resume 协议已写清**：worker hat `instructions:` 必须要求「先盘点同一目录下已有成果与实测验收结果、只补缺口、禁止回退或重做、已有提交不等于成功」；消费 `*.wave.failed` 的 handler hat 必须把该事件表述为「自动重试已耗尽」而非「失败一次」。禁止复述注入块格式或把 `aggregate_timeout_secs` 手动乘以尝试次数。✓ / ✗ + 引用 `patterns.md`「Wave slot 自动重试」段

任一问 ✗ → 必须改写或显式说明为何 supervisor 无法表达。完整 finding 默认 severity / confidence / aaf_question 见 `finding-rubric.md`「Supervisor capability audit」段。

## Hard questions — N/A 规则（执行模型分支）

| `execution_model` | 「Hard questions — single-chain-first」 | 「Hard questions — wave fan-out」 | 「Hard questions — supervisor orchestration」 |
|---|---|---|---|
| `single-chain`（默认 / 用户否认） | 必填（5 问全 ✓） | **N/A**（不得留空假装已答;不写 wave 字段、不写 dispatcher emit、不写 supervisor） | **N/A**（不得引入 `event_loop.supervisor.enabled`） |
| `wave` | 必填（与 wave 同存） | 必填（7 问全 ✓ + 证据） | **N/A** |
| `supervisor` | 必填（single-chain-first 默认也仍答,作为基线） | **N/A** | 必填（8 问全 ✓ + 证据） |
| `supervisor+wave` | 必填 | 必填 | 必填（三段并列;每段独立判定） |

**N/A 写法**:勾选框标 `N/A` + ≤30 字理由（如「execution_model=single-chain,无 wave 拓扑」），**不得**留空 / 写「同上」 / 写「由 wave 段覆盖」。N/A 不是「跳过」而是「显式不适用」。

## Hard questions — Artifact-First Handoff

任何 preset 交 review 前必须逐条回答；任一项不满足且没有符合例外条件的理由即拒绝交付：

1. **每条写入型 hat 是否声明了当前 `.ralph/` 下的 artifact 路径集合，且拓扑层没有把这些路径描述为「preset 创建」？** ✓ / ✗ + 证据
2. **每条 consumer hat 的 instructions 是否要求它从当前可见输入取得路径并显式读取 artifact，而不是依赖 prompt 中的长文本？** ✓ / ✗ + 证据
3. **每个被传递的完整结果、长内容或跨 hat 摘要是否都已先落盘，event / message 是否只保留短状态、短摘要、路径、必要身份与路由字段？** ✓ / ✗ + 证据
4. **是否有任何 hat 把 `.ralph/events.jsonl`、`.ralph/loops.json`、`.ralph/supervisor.db` 等 runtime internal ledger 当作自定义状态或 handoff 文件？** ✓ / ✗ + 证据；此项必须为 ✗
5. **每条声明「不落盘」的信息是否都标注了简短理由，并按恢复价值、审计价值和下游依赖解释，而非只按字符数判断？** ✓ / ✗ + 证据

预演 finding 时按 `references/finding-rubric.md`「Artifact-First Handoff finding_id」表入主表（review-only，不进 `ralph preset check` JSON）：`preset.artifact_path_not_in_visible_context` / `preset.artifact_no_consumer_declared` / `preset.artifact_no_lifecycle_owner` / `preset.artifact_uses_internal_ledger` / `preset.payload_carries_full_content` / `preset.artifact_first_field_docs_missing` / `preset.artifact_first_exemption_unjustified` / `preset.artifact_first_passed_on_path_presence` / `preset.subagent_result_returned_only_in_message` / `preset.artifact_described_as_preset_owned` / `preset.artifact_content_insufficient_for_decision`。完整默认 severity / confidence / aaf_question 见该表。

## Hard questions — Key-stage event gate (0e)

任何 preset 交 review 前必须按关键位置逐条回答；任一项不满足即按 `finding-rubric.md`「Key-stage event gate」段入主表（review-only）：

1. **每个被 Gate Scope 列入的关键 hat 是否已逐位置识别关键 handoff / 阶段分支，并按位置询问 4 选 1 guard 选择？** ✓ / ✗ + 列出位置 + 询问 menu 证据
2. **每个被选中的 guard 类型是否独立确认 retry budget（3 / 2 / 1，默认 3）？** ✓ / ✗ + 列每位置 budget 数值
3. **`precheck_retry_budget` 与 `payload_consistency_retry_budget` 是否分别记录、不共享总预算 / 计数器 / exhaustion state？** ✓ / ✗ + 证据
4. **选择 `neither` 或低于默认 budget（3）的位置是否都有 ≤80 字 `reason`（恢复 / 审计 / 下游依赖类）？** ✓ / ✗ + 证据
5. **每行 `confirmation_status` 是否为 `confirmed`？非 `confirmed` 是否阻断后续 YAML / schema 设计？** ✓ / ✗ + 证据
6. **0e 字段是否复用 0d 的 `hard/record/off` 字段？** 此项必须为 ✗（否）；✓ 表示字段被错误复用 → `preset.key_stage_event_gate_field_reuse`
7. **0e 字段是否引入新的 runtime 规则、计数器、恢复路径？** 此项必须为 ✗；✓ 表示 author 越权定义 runtime → `preset.key_stage_event_gate_unsupported_runtime_rule`

完整 finding 默认 severity / confidence / aaf_question 见 `finding-rubric.md`「Key-stage event gate」段。

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
