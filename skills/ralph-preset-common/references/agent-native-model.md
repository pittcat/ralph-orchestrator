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
| **Q4 输出** | 做完我向 runtime 交什么？ | `publishes` 内 topic + **payload 合同**（见下） | topic 在 `publishes`；`required_fields` 可填；单事件预算；**每个字段对 hat 可见且值源可达** |
| **Q5 交接** | 下游需要我传什么？它怎么 Observe？ | 拓扑层 `state_projection`；instructions 只写「我 emit 的字段」 | emit 字段 → projection → 下游 Q2 是否闭合 |

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

## Isolated prompt 栈（每个 activation）

`execution_mode: isolated` 时，prompt **仅含该 hat 自己的** `instructions:`（不含其它 hat、不含 `## HATS` 段）。

典型栈（自上而下）：

1. `## HAT IDENTITY` — hat id、触发事件上下文
2. 可选 `## WAVE CONTEXT` — wave worker 场景
3. 可选 `## TRIGGER CONTEXT` — preset/schema 在 `event_policy.schemas.<topic>.trigger_context` 声明的当前 trigger payload 摘要（source topic、source hat、declared summary fields、命中 routing hints）。本块只来自当前 activation 的 trigger payload，不来自 runtime ledger 或事件历史；缺失字段显示 `<missing>`，不推断为默认值。
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

**禁止（P0 典型来源）**

- 读 `.ralph/events.jsonl`、`.ralph/supervisor.db`、`.ralph/loops.json` 全文
- 在 instructions 写「reviewer 会帮你…」「上一步已…」（拓扑相对位置）
- 手写 `task_id`（须 `ralph tools task list` 取得）
- 同一 activation 多条业务 emit（isolated 单事件预算）
- 终态 emit 前夹带其它业务事件
- 在 `crates/ralph-core/data/*.md` 写 preset 专用 hat 名 / 拓扑 / 一次性诊断术语（专用知识只放 preset instructions）
- 引用未声明的 payload 字段（即便 schema 通过）

## 文档分层（data skill vs preset instructions）

| 层 | 位置 | 内容 |
|---|---|---|
| 通用 agent guide | `crates/ralph-core/data/ralph-tools*.md` | emit/task/recovery/precheck 通用语义、EmitResult、bounded retry、live task identity |
| Preset 专用 | `presets/en/<name>.yml` hat `instructions:` | 触发状态表、hat 角色、topic 编排、禁止动作 |
| Loop 外评审 | `skills/ralph-preset-common/references/` | 作者/评审如何检查分层与 AAF、payload audit 表、confidence 校准 |

Author 起草 recovery 路径时：**data docs 引用 + preset 状态表行**，不得在 data docs 复制 preset 状态表。

Skill doc 不复述 `ralph-tools*.md` 的命令参数表；需要时**引用章节**，让 doc 与 runtime 同步收敛。

## Coordinator 分支（≤3 hat）

`execution_mode: coordinator` 时单 prompt 可见多 hat 上下文；AAF 表须标注 **mode: coordinator** 及哪些 Q2/Q5 答案依赖 coordinator 注入。4+ hat **必须** `isolated`（`preset.multi_hat_requires_isolated`）。

## Author vs Review

| 阶段 | AAF 用法 |
|---|---|
| Author · 拓扑 | 作者视角画事件流；对齐 Q4↔Q2 handoff 字段 |
| Author · 起草 | 逐 hat 只答 Q1–Q5；假装其它 hat 不存在；按 payload audit 五列写 |
| Author · 自检 | 每 hat AAF 五问表 + payload audit 表无空项；每个 emit topic 都要列行 |
| Review | **独立重做** AAF + payload audit；不信任 author notes；缺口 → finding + confidence |