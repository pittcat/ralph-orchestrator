# Agent-Native Model (AAF)

Preset YAML 是作者视角的单文件；**运行时从不把整份 preset 喂给一个 agent**。

## 两层视角（必须分清）

| 视角 | 谁用 | 看到什么 |
|---|---|---|
| **Whole-preset 拓扑视角** | Author / Review 看 YAML 文件本身 | 所有 hat 的 `instructions`、`state_projection`、handoff 边 |
| **Activated-hat 视角** | 运行时该 hat 的 agent | 只看自己 `instructions` + 注入的 prompt 栈（见下）；看不到其它 hat |

AAF 评审的核心是：**从 activated-hat 视角独立模拟每一步**，再对照 whole-preset 视角确认 handoff 是否闭合。两层之间出现矛盾 → review material。

## Agent 视角可行性（AAF）五问

对 **每一个 hat**（author 起草、review 验收），必须逐条回答。任一问答不出或答案为「缺」→ 该 hat 编排不可执行。

| # | 问题（站在该 hat 的 agent 身上） | Author 产出 | Review 验证 |
|---|---|---|---|
| **Q1 使命** | 这一轮我要完成什么？完成标准？ | 单一可判定职责 + 终态 emit | 是否可判定；是否夹带其它 hat 职责 |
| **Q2 输入** | 我此刻能 Observe 到什么？够不够开工？ | Observe 命令 + 期望字段 | 输入是否 hat 可见；是否缺上游已 emit 未投影字段 |
| **Q3 执行** | 我要跑哪些 Ralph 允许的命令？顺序？ | OPAC 四阶段命令序列 | 命令是否存在、有权限；是否跳过 Precheck |
| **Q4 输出** | 做完我向 runtime 交什么？ | `publishes` 内 topic + **payload 合同**（见下） | topic 在 `publishes`；`required_fields` 可填；单事件预算（**收尾 hat 例外**：见下）；**每个字段对 hat 可见且值源可达** |
| **Q5 交接** | 下游需要我传什么？它怎么 Observe？ | 拓扑层 `state_projection`；instructions 只写「我 emit 的字段」 | emit 字段 → projection → 下游 Q2 是否闭合 |

**Q4 单事件预算例外（review-only）**：默认 Q4 单事件预算严格适用（`finding-rubric.md` 主表两条 P0）。唯一例外是 preset 显式配置的「收尾双事件终态」——当且仅当 hat 是 preset 中负责收尾的 hat（典型为 reporter / alignment），且 preset `event_loop.required_events[]` 与 `event_loop.completion_promise` 配对声明、且该 hat `publishes` 同时包含二者、且顺序为先 required event 再 completion promise、且两个事件均带同一 hat provenance 时，review 可放过其同 activation 双 emit。其它任何 hat 一律不享受本例外，复核细则见 `finding-rubric.md`「required-event-to-completion 窄例外」段。

## Payload 审计模型（Q4 第一类公民）

`ralph emit --schema` 与 `ralph emit --policy-check` **只能证明 shape 合规**，不能证明：

1. **字段可见性（visibility）**：emitting hat 真的能从自己的 prompt 栈里拿到这个字段的值。
2. **值源可达性（source）**：hat 能引用一条具体命令 / projection / trigger payload 字段，把值算出来。
3. **运行时身份一致性（identity）**：`task_id` / `task_key` / `step` / `task.state` 这类字段值必须从 live runtime 取得，不是手写。
4. **语义充分性（semantic sufficiency）**：字段类型合法、字面值却无法支撑下游决策（例如 `summary: "done"` 无法让下游决定 fix / block / complete）。
5. **下游消费（downstream consumption）**：下游 hat 的 Q2 Observe 路径实际能否读到、并按预期使用。

> **Rule of thumb**: schema 过得去 ≠ payload 可发。review 把 shape 通过但字段看不见 / 算不出 / 决策不够用 都算 P0/P1。

**Payload audit 行表（每个 emit topic 必填）**

| 列 | 内容 |
|---|---|
| topic | `publishes` 中的 topic 名 |
| 字段 | payload 字段名（重点：`task_id` / `task_key` / `step` / `verdict` / `reason` 等决策字段） |
| 值源 | trigger payload 字段 / `ralph tools task list` / 上游 emit 投影字段 / 自己 work 输出 |
| 可见性证据 | 在 hat-X prompt 栈中具体可见的 prompt 段（trigger payload / orchestrator context / scratchpad） |
| 身份检查 | 是否需要 live runtime identity；若是，命令路径是否在白名单 |
| 语义下游 | 下游 hat 用此字段做什么决策；字面值能否支撑该决策 |
| verdict | pass / P0 / P1 + 修复面 |

## Artifact-First Handoff 模型

AAF 五问与 Payload Audit 解决「字段可达 / 语义充分」；**Artifact-First Handoff** 解决「完整结果与状态放在哪里、由谁读」。

**三句话定义**

- **文件是重要信息的事实源。** 一旦信息丢失会导致无法恢复、重新调查、证据不可审计或再次调用 sub-agent，就必须落盘。
- **事件承担控制面，文件承担数据面。** event / message 保留短状态、摘要、路径、必要身份与路由字段；完整结果、证据、进度与决策依据放在文件中。
- **默认强制，但允许有理由的例外。** 短暂、短小且无需恢复的信息可以不落盘，但必须显式记录理由（见下「不落盘例外规则」）。

**落盘判定三标准（每条独立成立即构成必落盘理由）**

- **恢复价值**：信息丢失后能否低成本重建。sub-agent 重跑成本高、跨 hat 重新调查不可行、决策依据仅在运行期可见 → 必落盘。
- **审计价值**：能否作为独立可审计证据。下游决定依赖它、事后复核需要它、终态报告需要它 → 必落盘。
- **下游依赖**：是否有 hat 或人需要后续读取。下游 hat 的决策依赖文件内容、文件作为下次 activation 的输入、文件被纳入 reviewer / fixer 的复核范围 → 必落盘。

> **不使用字符数作为主判据**。字符数可作为辅助信号（短小可能更容易例外），但不构成独立理由；真正决定是否落盘的是恢复 / 审计 / 下游依赖三标准。

**典型落盘 vs 典型不落盘（对照）**

| 类别 | 内容 | 处理 |
|---|---|---|
| 典型落盘 | sub-agent 完整报告、长 diff 分析、跨 hat 完整正文、关键决策依据、验证证据、高成本重建的状态 | hat 或其 sub-agent 先写 `.ralph/<...>`，event / message 只带路径与短摘要 |
| 典型不落盘 | 短计数（`must_fix_now_count: 0`）、状态枚举（`verdict: pass`）、单 token verdict（`accepted`）、路径字符串本身 | 直接放在 payload，不写文件 |
| 灰色地带 | 长 verdict reason（>200 字符且说明关键依据） | 落盘 + payload 只带 `reason_id` / 路径（详见下「不落盘例外规则」） |
| 灰色地带 | 短 verdict reason（≤1 句） | 可不落盘（满足短暂 + 短小 + 无需恢复三条件） |

**灰色地带判定方法**：先问「下游是否需要原文做决定 / 审计 / 复用」；答「是」→ 落盘。答「否」再看「内容能否低成本重建」；能重建且下游不需要原文 → 可不落盘。任一答「否 / 不确定」都倾向落盘。

## Isolated prompt 栈（每个 activation）

`execution_mode: isolated` 时，prompt **仅含该 hat 自己的** `instructions:`（不含其它 hat、不含 `## HATS` 段）。

典型栈（自上而下）：

1. `## HAT IDENTITY` — hat id、触发事件上下文
2. 可选 `## WAVE CONTEXT` — wave worker 场景
3. 可选 `## TRIGGER CONTEXT` — preset/schema 在 `event_policy.schemas.<topic>.trigger_context` 声明的当前 trigger payload 摘要（`source topic`、可选 `source hat`、declared summary fields、命中 routing hints）。本块只来自当前 activation 的 trigger payload，不来自 runtime ledger 或事件历史；缺失字段显示 `<missing>`，不推断为默认值。**v1 `source hat` 是 optional**：runtime 不知道实际发布 hat 时显示 `(unknown source hat)`，不要把它当成必然存在字段。
4. `## ORCHESTRATOR CONTEXT` — 投影后的 task/progress 视图
5. **本 hat `instructions:`**
6. Auto-inject skills — `ralph-tools.md`（tasks/memories 启用时）、`ralph-tools-tasks.md`、`ralph-tools-memories.md` 等
7. Scratchpad / state files / open tasks 块

证据：`crates/ralph-core/src/event_loop/tests/payload_types.rs` — `test_isolated_prompt_contains_only_target_hat_instructions`。

Review 模拟每 hat 时，按上面七段核对每条 Q2 Observe / Q4 字段的可见性。`## TRIGGER CONTEXT` 是消费方 prompt context，不是 `ralph emit --policy-check` 的替代品；emit shape 验证仍走 schema gate。

## 状态传递（禁止空想）

```
上游 hat emit (Q4)
  → state_projection.actions
  → task / progress 视图
  → 下游 hat Observe (Q2)
```

`state_projection` 本身不直接喂下游 hat；它把 emit 字段写到 task / progress / orchestrator context 视图，下游 hat 通过 `ralph tools task list` / `ralph inspect loop` / trigger payload 字段名 读到。**review 必须把"上游 emit → projection → 下游可见视图 → 下游 Q2 Observe 命令"这条完整链路拉通**。

### Projection-Owned Task Creation（单事件原子建 task DAG）

当 hat 的输出是「一组内部 task」而不是单事件，普通 `state_projection.actions` 单 action 模式不够用。配置 typed `ensure_task_batch`：

```
上游 hat emit forge.plan.ready (Q4, payload: unit_tasks[])
  → state_projection.actions.forge.plan.ready = ensure_task_batch
  → Projector.try_with_exclusive_lock 一次性校验 + ID mint + 持久化（任一失败整批零写）
  → .ralph/agent/tasks.jsonl 全批新增
  → 下游 hat 通过 ralph tools task list 读 live task_id（不是 planner 在 instructions 里手写）
```

- hat instructions 在 batch action 配置存在时**不得**让同一 hat 调 `ralph tools task add` / `task ensure` 走 CLI；preset lint `preset.instructions_task_mutation_authority_conflict` 会在载入时拒收。
- Q3 适用：在 batch 模式下"task 创建"的 OPAC 命令是 Planner **emit handoff**，不是 `ralph tools task add`。
- 下游 hat 的 `task_id` 字段必须来自 `ralph tools task list` 实时返回（prompt 的 `## ORCHESTRATOR CONTEXT` 块会注入），禁止把 inventory 里的占位 ID 抄进 payload。

**允许的路径（Q3 白名单入口）**

| 用途 | 引用 skill / 命令 |
|---|---|
| 共享任务事实 | `ralph tools task` — 见 `ralph-tools-tasks` |
| 发业务/终态事件 | `ralph emit` / `ralph wave emit` — 见 `ralph-tools-emit` |
| 环状态 | `ralph inspect loop` |
| hat-channel 事件确认 | `ralph events --events-source hat-channel` |
| OPAC 纪律 | `ralph-tools-opac` |
| 写盘前预检 | `ralph emit --policy-check` — 见 `ralph-tools` §5 |
| emit 与 hat 通道预检 | `ralph tools task verify-emit-bridge` — 见 `ralph-tools-tasks` |

### Artifact-First 状态传递（文件作数据面、event 作控制面）

```
上游 hat 创建 / 更新 .ralph/<plan>/<unit>/<file>.md (Q3 Apply)
  → emit 携带路径 / 摘要 / 必要身份 (Q4)
  → state_projection.actions 把路径字段投影 (Q5)
  → 下游 hat 通过 trigger payload / orchestrator context 读到路径 (Q2)
  → 下游 hat 读 .ralph/<...>/<file>.md 取得完整内容 (Q3 Observe)
```

- **event / message 只携带「短状态 / 摘要 / 路径 / 必要身份 / 路由字段」**，不携带完整正文。
- **路径必须对下游 hat 可见**：出现在 trigger payload、projection、task view 或命令输出中的任一可见位置；下游 hat 的 `instructions:` 必须显式要求读取该路径（Q3 Apply）。
- **完整正文只能通过读文件获得**：下游 hat 不应从 trigger payload 反查完整结果、长内容或长解释字段。

> 本段与上方「状态传递（禁止空想）」是同一模型的两个视角：上面回答「字段如何从上游到达下游视图」；这里回答「重要内容如何从上游文件到达下游消费」。两段互补，不重复。

**禁止（P0 典型来源）**

- 读 `.ralph/events.jsonl`、`.ralph/supervisor.db`、`.ralph/loops.json` 全文
- 在 instructions 写「reviewer 会帮你…」「上一步已…」（拓扑相对位置）
- 手写 `task_id`（须 `ralph tools task list` 取得）
- 同一 activation 多条业务 emit（isolated 单事件预算）
- 终态 emit 前夹带其它业务事件
- 在 `crates/ralph-core/data/*.md` 写 preset 专用 hat 名 / 拓扑 / 一次性诊断术语（专用知识只放 preset instructions）
- 引用未声明的 payload 字段（即便 schema 通过）

### Artifact-First 边界（业务 artifact vs runtime internal ledger）

本段回答「重要状态应该放在 `.ralph/` 的哪里」。它与上方「禁止（P0 典型来源）」互补：上面禁止把 internal ledger **当作业务接口**（读）；本段规定业务 artifact **必须放在哪里**、以及 internal ledger 与业务 artifact 的区分标准。

- **业务 artifact 位置**：当前 workspace / worktree 的 `.ralph/` 下；由 hat 或其 sub-agent 在自己那一轮 activation 内创建或更新；**不得跨 worktree 共享**（每个 worktree 自带 `.ralph/`，跨 worktree 引用是 topology violation）。
- **internal ledger 边界**：
  - **禁止**：把 `.ralph/events.jsonl`、`.ralph/loops.json`、`.ralph/supervisor.db` 当作业务 artifact 接口（写或读）。这些是 runtime / supervisor 内部 ledger，schema 受控、由 runtime 维护。
  - **允许**：hat 通过 `ralph tools task`、`ralph emit`、`ralph inspect loop`、`ralph events --events-source hat-channel` 等 runtime API 共享状态（见上方「允许的路径」表）。
  - **区分标准**：internal ledger 由 runtime / supervisor 维护、schema 受控；业务 artifact 由 hat 维护、内容与命名是 preset 设计选择。
- **「不落盘」例外规则**：
  - **必须三条件同时成立**才能不落盘：信息「**短暂**（activation 内有效即可）」+「**短小**（单 token 或一两句）」「**无需恢复**（重算成本极低 / 下游不依赖历史）」。
  - **必须在 hat `instructions:` 或 Payload Contract 表中显式标注理由**。不写理由 = 默认违规。
  - **不接受的理由**：仅字符数少、仅给下游看、仅 debug 用、不重要（重要性本身就是判定目标，不是已知事实）。
  - **灰色地带**：长 verdict reason（>200 字符且说明关键依据）必须落盘 + payload 只带 `reason_id` / 路径；短 verdict reason（≤1 句）可不落盘（同时满足三条件）。

> **不要把 Artifact-First 与「禁止读 internal ledger」混为一谈**。前者规定业务信息的存放与交接；后者规定内部 ledger 不当作业务接口。两条规则共同构成「数据面 vs 控制面」分离。

## 文档分层（data skill vs preset instructions）

| 层 | 位置 | 内容 |
|---|---|---|
| 通用 agent guide | `crates/ralph-core/data/ralph-tools*.md` | emit/task/recovery/precheck 通用语义、EmitResult、bounded retry、live task identity |
| Preset 专用 | `presets/en/<name>.yml` hat `instructions:` | 触发状态表、hat 角色、topic 编排、禁止动作 |
| Loop 外评审 | `skills/ralph-preset-common/references/` | 作者/评审如何检查分层与 AAF、payload audit 表、confidence 校准 |
| Artifact-First Handoff 知识分层 | 三层见下「Artifact-First Handoff 知识分层」段 | 本 hat 创建 / 读 artifact 的具体路径、生命周期责任、例外与理由（YAML）；artifact-first 模型、判定标准、违规列表（本 reference）；不在 `ralph-tools*.md` 写通用规则 |

Author 起草 recovery 路径时：**data docs 引用 + preset 状态表行**，不得在 data docs 复制 preset 状态表。

Skill doc 不复述 `ralph-tools*.md` 的命令参数表；需要时**引用章节**，让 doc 与 runtime 同步收敛。

### Artifact-First Handoff 知识分层（三层职责划分）

- **`presets/en/<name>.yml` hat `instructions:`**（preset 决策层）：写**本 hat**创建 / 读 artifact 的具体路径、生命周期责任（产出方 / 消费方 / 保留 / 归档 / 清理）、例外与例外理由。粒度到字段级：「写到哪里」「从哪读」「谁负责」。
- **`skills/ralph-preset-common/references/agent-native-model.md`**（loop 外评审层，本文件）：写 artifact-first 模型、判定标准（恢复价值 / 审计价值 / 下游依赖）、违规列表、边界（业务 artifact vs internal ledger）、灰色地带判定方法。**不写**具体的「`.ralph/<...>` 子目录命名约定」，由 preset 自决。
- **`crates/ralph-core/data/ralph-tools*.md`**（runtime 注入层）：**不写** artifact-first 通用规则。artifact-first 是 **preset 决策**，不是 runtime 决策；运行时只提供文件读写命令（如 OPAC Apply 阶段的 `Write` / `Edit`），不约束写哪里、什么时候写、是否落盘。`ralph-tools*.md` 最多以引用形式指向本 reference 的「Artifact-First Handoff 模型」或「Artifact-First 边界」段，不复制判定标准。

## 执行模型（Execution Model）

> **范围**：本节冻结「执行模型」枚举与能力检测信号，供 `ralph-preset-author` / `ralph-preset-review` / `ralph-run-diagnosis` 三套通用 skill 共用。**禁止**按 builtin preset 名称点名门控（例如不得新增「`ce-executor-supervisor*` 风格」类专检）；所有 audit / checklist / 诊断一律 **capability-triggered**（Intent + YAML / 产物信号触发）。

**枚举（冻结，跨 plan 不再扩）**：

| 值 | 含义 | 关键可见差异 |
|---|---|---|
| `single-chain` | 单条主链顺序推进，并行只在执行 hat 内部 subagent 拆分 | 默认推荐；无 `event_loop.supervisor.enabled`；无 dispatcher `ralph wave emit` |
| `wave` | 主链上某步对多份同构工作做同 topic 批并行 fan-out | hat 依赖 `## WAVE CONTEXT`；dispatcher 调 `ralph wave emit` + `ralph wave verify` |
| `supervisor` | runtime 管理多 slot / worktree / 排队与 fan-in | `event_loop.supervisor.enabled: true`；存在 `.ralph/supervisor.db`；协调 topic 由 runtime 管 |
| `supervisor+wave` | supervisor 且 dispatcher 使用 wave fan-out | 上述两类信号同时出现 |

**能力检测信号（冻结，供 review / diagnose 共用）**：

| 信号来源 | 触发字段 / 关键字 |
|---|---|
| Intent 字段 | `execution_model` ∈ {`wave`, `supervisor`, `supervisor+wave`} |
| YAML 拓扑 | `event_loop.supervisor.enabled: true` |
| YAML / instructions | 出现 `ralph wave emit` / `ralph wave verify`，或 hat 依赖 `## WAVE CONTEXT` |
| 产物（diagnose） | 存在 `.ralph/supervisor.db`，或 events 含 `wave_id`，或日志出现 wave fan-out |

**默认推荐**：菜单第一项永远是 `single-chain`。用户否认 wave / supervisor → 锁定 `execution_model: single-chain`，后续拓扑不得引入 `event_loop.supervisor.enabled`、dispatcher 不得调用 `ralph wave emit`。

**通用性硬约束**：**禁止**「preset 名称以 X 开头」之类名缀门控。`ce-executor-pipeline` 的 3b 既有规则保留，本计划不扩展该模式；**新加内容必须按 capability 触发**。

## Coordinator 分支（≤3 hat）

`execution_mode: coordinator` 时单 prompt 可见多 hat 上下文；AAF 表须标注 **mode: coordinator** 及哪些 Q2/Q5 答案依赖 coordinator 注入。4+ hat **必须** `isolated`（`preset.multi_hat_requires_isolated`）。

## Author vs Review

| 阶段 | AAF 用法 |
|---|---|
| Author · 拓扑 | 作者视角画事件流；对齐 Q4↔Q2 handoff 字段 |
| Author · 起草 | 逐 hat 只答 Q1–Q5；假装其它 hat 不存在；按 payload audit 五列写 |
| Author · 自检 | 每 hat AAF 五问表 + payload audit 表无空项；每个 emit topic 都要列行 |
| Review | **独立重做** AAF + payload audit；不信任 author notes；缺口 → finding + confidence |

### Review 必须独立重做的 artifact-first 检查

| 检查项 | 重做方法 | 判定信号 |
|---|---|---|
| **artifact 落盘判定** | 独立按「恢复价值 / 审计价值 / 下游依赖」三标准对每个重要信息逐条判定，**不参考 author notes** | 重要信息仅存在于长 payload / 长 message 而无对应 `.ralph/<...>` artifact → finding（按对恢复 / 审计 / 下游的影响定 P0/P1） |
| **路径可见性（单 hat activation 视角）** | 假设「只看到该 hat 的 prompt 栈」，验证 artifact 路径是否在 trigger payload / projection / task view / 命令输出中**任一可见位置** | 路径不可见、或仅在另一 hat 的 prompt 中出现 → finding |
| **消费动作闭环** | 检查下游 hat `instructions:` 是否**显式要求**读取上游 emit 给出的路径字段，且读盘后有验收 / 确认动作（文件存在、可解析、足以支撑本 hat Q1） | 路径已 emit 但下游未要求读文件 → `preset.artifact_no_consumer_declared`；仅「看到路径」无验收 → `preset.artifact_first_passed_on_path_presence` |
| **内容充分性（R8）** | 假设 consumer 已取得路径并读盘，检查 artifact 设计是否声明足以支撑该 hat Q1 的内容（完整结果 / 证据 / 未解决问题）；不得只凭路径字符串放行 | 路径存在但内容约定不足以恢复或继续决策 → `preset.artifact_content_insufficient_for_decision` |
| **internal ledger 边界** | 验证 hat 是否要求把 `.ralph/events.jsonl` / `.ralph/loops.json` / `.ralph/supervisor.db` 当业务 artifact 接口（写或读） | 出现 → finding（与「禁止读 internal ledger」互补） |
| **不落盘例外** | 验证例外是否同时满足「短暂 + 短小 + 无需恢复」，以及是否在 `instructions:` 或 Payload Contract 中标注理由 | 例外无理由 / 理由仅「字符数少」 / 理由仅「给下游看」 → finding |

**review-only finding（不进入 `ralph preset check` JSON）**

- 本节列出的检查项属于「模型 / 边界」层，不是机械 lint 规则；review 报告以引用本 reference 的相应段（如「Artifact-First Handoff 模型」 / 「Artifact-First 边界」 / 「Artifact-First 状态传递」）作为 finding 的根因证据。
- 后续如需把这些检查固化为 lint finding（带 `finding_id`），应在 `finding-rubric.md` 维护；本文件不引入新 finding_id。
- author 与 reviewer 通过引用本 reference 段保持术语一致；review 不另造「Artifact-First」定义。

## Runtime Audit Model (Unit 4 / plan 2026-07-27-002)

<!-- anchor: wave-emit -->
<!-- anchor: supervisor-emit -->
<!-- anchor: task-id-live -->
<!-- anchor: artifact-first -->
<!-- anchor: payload-consistency -->
<!-- anchor: trigger-context -->

Preset author/review audits now use `ralph inspect prompt --trigger/--payload/--topic` to simulate hat activation without running the loop, and `ralph capability inventory --format json` to discover which runtime capabilities a preset exercises. The four evidence levels (`simulated` / `static` / `runtime` / `unverified`) classify findings by where the proof came from.