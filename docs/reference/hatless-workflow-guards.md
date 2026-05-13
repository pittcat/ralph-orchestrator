# Hatless Ralph 与工作流状态守卫

> 面向 Ralph 维护者和高级 workflow 作者。本文解释 Ralph 当前 Hatless 多 Hat 机制、为什么严格阶段工作流会跳步，以及 workflow guard 这类引擎级状态守卫要解决什么问题。

---

## 1. 核心结论

Ralph 的 Hat 系统不是多个长期运行的独立 Agent。多 Hat 模式下，Ralph 仍然是统一执行者；Hat 定义主要提供：

- topic 订阅关系
- 可发布事件列表
- 当前 prompt 中要注入的角色 instructions

因此，Hat 可以帮助 Agent 在不同阶段“戴不同帽子”，但它本身不是强状态机。

对于简单工作流，这种设计足够轻量。对于 AutoResearch 这类必须严格走完每个阶段的流水线，仅靠 prompt 注入不够。

---

## 2. 当前机制：事件驱动的 prompt 注入

当前多 Hat 循环大致是：

1. 后端 Agent 通过 JSONL 事件文件写出事件。
2. `EventLoop` 读取事件。
3. `EventBus` 根据 topic 分发给订阅该 topic 的 Hat。
4. 多 Hat 模式下，`next_hat()` 仍然返回 `ralph`。
5. `build_prompt()` 收集 pending events，推导 active hats。
6. `HatlessRalph::build_prompt()` 把 active hats 的 instructions 注入 prompt。
7. 后端 Agent 按这个 prompt 执行下一步。

源码锚点：

| 机制 | 文件 |
|---|---|
| 多 Hat 模式统一路由到 Ralph | `crates/ralph-core/src/event_loop/mod.rs` |
| prompt 构建和 active hats 注入 | `crates/ralph-core/src/event_loop/mod.rs`、`crates/ralph-core/src/hatless_ralph.rs` |
| topic 到 Hat 的查找 | `crates/ralph-core/src/hat_registry.rs` |
| 事件基础结构 | `crates/ralph-proto/src/event.rs` |
| EventBus 分发 | `crates/ralph-proto/src/event_bus.rs` |
| loop-lifetime topic 记录 | `crates/ralph-core/src/event_loop/loop_state.rs` |

---

## 3. Ralph 与 Claude Code 的实际调用关系

先说人话：**Ralph 确实有“拼 prompt”的能力，但这不是 AI 能力，也不神秘。**

Ralph 是一个普通程序。普通程序可以把几段文字合成一大段文字。比如：

```text
第一段：当前发生了什么事件
第二段：现在应该激活哪些 Hat
第三段：这些 Hat 的职责说明
第四段：允许发布哪些事件
第五段：scratchpad / tasks / memories 等外部状态
第六段：请执行下一步，并用 ralph emit 写回结果
```

Ralph 把这些文字合起来，就得到 Claude Code 会看到的一份完整任务说明。这个完整任务说明，就是这里说的 prompt。

所以，“prompt 注入”在本文里只是工程实现描述，不是安全攻击意义上的 prompt injection。它指的是：**Ralph 在每一轮执行前，生成一份完整任务说明，把当前事件、active Hat instructions、事件发布规则和外部状态一起交给 Claude Code 后端。**

用公式表示就是：

```text
本轮 prompt = 当前事件 + Hat 职责说明 + 输出规则 + 外部状态 + 下一步要求
```

一个更准确的心智模型是：

```text
Ralph 每轮生成一张工单。
Claude Code 读取这张工单，执行任务。
执行完后，Claude Code 用 ralph emit 写回事件。
Ralph 再根据新事件生成下一张工单。
```

不是：

```text
Ralph 维护多个长期 Claude Agent 进程。
每个 Hat 都有自己的独立 Claude 会话。
```

也不是：

```text
Ralph 往用户已经打开的 Claude Code 聊天窗口里注入隐藏消息。
```

### 3.1 为什么说它像“工单”

你可以把 Ralph 想成调度员，把 Claude Code 想成执行人。

Ralph 每一轮不是只发一句：

```text
你现在评审一下。
```

它更像发一张完整工单：

```text
现在已有事件：experiment.measured
当前应该激活的角色：Reviewer
Reviewer 的职责：根据测量结果打分
你可以发布的下一步事件：experiment.scored
请完成任务，并用 ralph emit 把结果写回事件文件
```

Claude Code 读到这张工单后，才知道这一轮该干什么。

### 3.2 一轮循环的执行链路

```text
EventBus 中存在 pending events
        ↓
EventLoop::next_hat() 选择下一轮执行者
        ↓
多 Hat 模式下通常仍选择 ralph
        ↓
EventLoop::build_prompt() 汇总 pending events、active hats、scratchpad 等上下文
        ↓
HatlessRalph::build_prompt() 生成 Claude Code 会看到的完整 prompt
        ↓
loop_runner 调用后端执行器
        ↓
Claude backend 启动或驱动 claude CLI
        ↓
Claude Code 执行 prompt 中的任务
        ↓
Claude Code 通过 ralph emit 发布事件
        ↓
ralph emit 写入 JSONL 事件文件
        ↓
EventLoop 读取、验证、发布事件
        ↓
下一轮重新构建 prompt
```

所以 Ralph 与 Claude Code 的职责边界是：

| 组件 | 主要职责 |
|---|---|
| Ralph | 调度、事件路由、prompt 组装、事件验证、状态持久化 |
| Claude Code | 根据本轮 prompt 执行具体任务、调用工具、通过 `ralph emit` 交回结果 |

### 3.3 Claude backend 做了什么

Ralph 的 Claude backend 通过 Claude CLI 工作。标准 Claude backend 会配置：

- 命令：`claude`
- 输出格式：`stream-json`
- prompt 传递模式：常规路径使用 `stdin`

在 PTY 执行路径中，Ralph 还会把 `RALPH_EVENTS_FILE` 传给 Claude 进程环境，使 Claude 从任意工作目录运行 `ralph emit` 时，都能把事件写回 Ralph 当前循环正在读取的 JSONL 文件。

这意味着 Ralph 的核心状态不应该被理解为“Claude 聊天记忆”。更准确地说，状态主要在：

- EventBus / JSONL events
- scratchpad
- tasks
- memories
- 生成出来的本轮 prompt

Claude Code 每轮看到的上下文，是 Ralph 从这些状态重新组装出来的。

### 3.4 为什么这会影响严格工作流

如果同一轮存在多个 pending events，Ralph 可能把多个 active Hat 的 instructions 放进同一份 prompt。

例如：

```text
pending events:
- experiment.measured
- periodic.review

active hats:
- reviewer
- evaluator
```

Claude Code 看到的是一份包含多个职责提示的工单。它不是被物理限制在 Reviewer 进程里，也不是只能发布 Reviewer 的下一步事件。只要当前 scope 允许，它可能选择 Evaluator 路径并发布 `experiment.evaluated`。

因此，对于严格阶段流，不能只依赖“prompt 里写了应该先 Reviewer”。必须在 Ralph 接收 `ralph emit` 事件后、事件进入可信状态前，增加运行时顺序检查。

源码锚点：

| 机制 | 文件 |
|---|---|
| Claude 后端命令、输出格式、prompt mode | `crates/ralph-adapters/src/cli_backend.rs` |
| PTY 执行和 `RALPH_EVENTS_FILE` 传递 | `crates/ralph-adapters/src/pty_executor.rs` |
| prompt 执行与事件回收 | `crates/ralph-cli/src/loop_runner.rs` |
| active hats 与 prompt 构建 | `crates/ralph-core/src/event_loop/mod.rs`、`crates/ralph-core/src/hatless_ralph.rs` |
| EventBus 事件分发 | `crates/ralph-proto/src/event_bus.rs` |

---

## 4. 这个机制的设计意图

Hatless 机制是有意设计，不是偶然实现。

它的优点：

- 简单：不用维护多个 agent 进程和跨进程同步。
- 弹性：Agent 可以在 fresh context 中重新理解当前事件。
- 低耦合：Hat 只是工作流拓扑和 prompt 片段。
- 可恢复：事件、任务、memories、scratchpad 都在磁盘上，Agent 可以重新读取。

它的限制：

- Hat 之间不是物理隔离。
- 多个 pending events 可能导致多个 Hat instructions 同时出现在 prompt 中。
- Agent 可能选择错误的职责优先级。
- `required_events` 是完成前的 topic 级检查，不是运行中的实例级状态机。

---

## 5. 典型失败：AutoResearch 跳过评分阶段

AutoResearch 期望的阶段顺序是：

```text
experiment.planned
-> experiment.ready
-> experiment.measured
-> experiment.scored
-> experiment.evaluated
```

失败形态是：

```text
experiment.measured
periodic.review
experiment.evaluated
```

这里 `periodic.review` 是合法 side-channel，用于 Goal Review。但它可能激活 Evaluator 职责，使 Agent 在未执行 Reviewer 的情况下发布 `experiment.evaluated`。

当前 Ralph 会接受这个事件，只要它通过基础事件解析和可选 scope 检查。到 `LOOP_COMPLETE` 时，`required_events` 才发现 `experiment.scored` 缺失，于是拒绝完成。

问题不是 `periodic.review` 本身非法，而是 Ralph 缺少一种机制来表达：

```text
experiment.evaluated 必须在同一个实验的 experiment.scored 之后
```

---

## 6. `required_events`、`enforce_hat_scope` 和 workflow guard 的区别

| 机制 | 解决什么 | 不能解决什么 |
|---|---|---|
| `required_events` | 完成前必须见过某些 topic | 不能保证 topic 顺序，也不能区分实验实例 |
| `enforce_hat_scope` | 当前 active hat 不能发布未声明 topic | 多个 active hats 同时存在时，仍可能让错误职责发布合法 topic |
| workflow guard | 按链路和实例检查事件顺序 | 不替代 Hat instructions，也不负责业务判断 |

workflow guard 的职责应很窄：

```text
只判断某个事件在当前 workflow instance 中是否来得太早。
```

---

## 7. Workflow Guard 应该怎样工作

一个 guard 应声明：

- chain 名称
- 有序 topic 列表
- terminal topic
- 可选实例标识提取规则
- 违规后的恢复行为

运行时流程：

1. Ralph 读取新事件。
2. Ralph 完成已有解析、scope、backpressure 检查。
3. 如果事件属于某条 guarded chain，检查它是否符合当前实例进度。
4. 如果符合，记录进度并发布到 EventBus。
5. 如果不符合，拒绝该事件，不记录为有效进展，并注入 recovery diagnostic。

AutoResearch 的关键规则是：

```text
experiment.evaluated 只有在同一 experiment_id 已经 experiment.scored 后才能通过。
```

---

## 8. 修复后的用户可见效果

修复前，Agent 跳步后，错误会积累到最后：

```text
measured -> evaluated -> LOOP_COMPLETE 被拒
```

修复后，错误会在刚发生时被拦下：

```text
measured -> evaluated 被拒 -> 提示缺 scored -> 回到评分
```

这有三个效果：

1. 不需要人工 backfill。
2. `autoresearch.jsonl` 这类决策账本不会先写入无评分决策。
3. `periodic.review` 可以保留，但不会越权推进单个实验。

---

## 9. 兼容性原则

workflow guard 应默认关闭。

原因：

- Ralph 的很多现有 workflow 并不需要严格状态机。
- 老配置不能因为新机制改变行为。
- 严格阶段流应显式声明自己的链路。

建议策略：

1. 引擎支持 opt-in guard。
2. 内置 AutoResearch preset 打开 guard。
3. Universal AutoResearch 生成器后续生成 guard 配置。
4. 其他 preset 只有在确实需要强顺序时再启用。

---

## 10. 设计边界

workflow guard 不应该变成完整 workflow 引擎。

它不负责：

- 替代 Agent 的业务判断。
- 自动生成评分。
- 自动修复 payload。
- 管理多个独立 agent 进程。
- 替代 `required_events`。

它只负责：

- 阻止乱序事件污染事件流。
- 给 Agent 一个明确的恢复提示。
- 在完成前检查是否还有未走完的 guarded instance。

---

## 11. 相关计划和来源

- 根治计划：`docs/plans/2026-05-12-002-fix-autoresearch-workflow-state-guard-plan.md`
- 相关 harness 扩展计划：`docs/plans/2026-05-12-001-feat-harness-extension-plan.md`
- AutoResearch preset：`presets/autoresearch.yml`
- 中文 AutoResearch preset：`presets/autoresearch-zh.yml`
- 事件循环源码：`crates/ralph-core/src/event_loop/mod.rs`
- Hatless prompt 源码：`crates/ralph-core/src/hatless_ralph.rs`
- EventBus 源码：`crates/ralph-proto/src/event_bus.rs`
