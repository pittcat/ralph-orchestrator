# Agent-Native Model (AAF)

Preset YAML 是作者视角的单文件；**运行时从不把整份 preset 喂给一个 agent**。

## Agent 视角可行性（AAF）五问

对 **每一个 hat**（author 起草、review 验收），必须逐条回答。任一问答不出或答案为「缺」→ 该 hat 编排不可执行。

| # | 问题（站在该 hat 的 agent 身上） | Author 产出 | Review 验证 |
|---|---|---|---|
| **Q1 使命** | 这一轮我要完成什么？完成标准？ | 单一可判定职责 + 终态 emit | 是否可判定；是否夹带其它 hat 职责 |
| **Q2 输入** | 我此刻能 Observe 到什么？够不够开工？ | Observe 命令 + 期望字段 | 输入是否 hat 可见；是否缺上游已 emit 未投影字段 |
| **Q3 执行** | 我要跑哪些 Ralph 允许的命令？顺序？ | OPAC 四阶段命令序列 | 命令是否存在、有权限；是否跳过 Precheck |
| **Q4 输出** | 做完我向 runtime 交什么？ | `publishes` 内 topic + payload 字段 | topic 在 `publishes`；`required_fields` 可填；单事件预算 |
| **Q5 交接** | 下游需要我传什么？它怎么 Observe？ | 拓扑层 `state_projection`；instructions 只写「我 emit 的字段」 | emit 字段 → projection → 下游 Q2 是否闭合 |

## Isolated prompt 栈（每个 activation）

`execution_mode: isolated` 时，prompt **仅含该 hat 自己的** `instructions:`（不含其它 hat、不含 `## HATS` 段）。

典型栈（自上而下）：

1. `## HAT IDENTITY` — hat id、触发事件上下文
2. 可选 `## WAVE CONTEXT` — wave worker 场景
3. `## ORCHESTRATOR CONTEXT` — 投影后的 task/progress 视图
4. **本 hat `instructions:`**
5. Auto-inject skills — `ralph-tools.md`（tasks/memories 启用时）、`ralph-tools-tasks.md`、`ralph-tools-memories.md` 等
6. Scratchpad / state files / open tasks 块

证据：`crates/ralph-core/src/event_loop/tests/payload_types.rs` — `test_isolated_prompt_contains_only_target_hat_instructions`。

## 状态传递（禁止空想）

```
上游 hat emit (Q4)
  → state_projection.actions
  → task / progress 视图
  → 下游 hat Observe (Q2)
```

**允许的路径（Q3 白名单入口）**

| 用途 | 引用 skill / 命令 |
|---|---|
| 共享任务事实 | `ralph tools task` — 见 `ralph-tools-tasks` |
| 发业务/终态事件 | `ralph emit` / `ralph wave emit` — 见 `ralph-tools-emit` |
| 环状态 | `ralph inspect loop` |
| hat-channel 事件确认 | `ralph events --events-source hat-channel` |
| OPAC 纪律 | `ralph-tools-opac` |
| 写盘前预检 | `ralph emit --policy-check` — 见 `ralph-tools` §5 |

**禁止（P0 典型来源）**

- 读 `.ralph/events.jsonl`、`.ralph/supervisor.db`、`.ralph/loops.json` 全文
- 在 instructions 写「reviewer 会帮你…」「上一步已…」（拓扑相对位置）
- 手写 `task_id`（须 `ralph tools task list` 取得）
- 同一 activation 多条业务 emit（isolated 单事件预算）
- 终态 emit 前夹带其它业务事件

## Coordinator 分支（≤3 hat）

`execution_mode: coordinator` 时单 prompt 可见多 hat 上下文；AAF 表须标注 **mode: coordinator** 及哪些 Q2/Q5 答案依赖 coordinator 注入。4+ hat **必须** `isolated`（`preset.multi_hat_requires_isolated`）。

## Author vs Review

| 阶段 | AAF 用法 |
|---|---|
| Author · 拓扑 | 作者视角画事件流；对齐 Q4↔Q2 handoff 字段 |
| Author · 起草 | 逐 hat 只答 Q1–Q5；假装其它 hat 不存在 |
| Author · 自检 | 每 hat AAF 五问表无空项 |
| Review | **独立重做** AAF；不信任 author notes；缺口 → finding + confidence |
