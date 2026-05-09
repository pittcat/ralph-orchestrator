# Iteration Boundary 扩展指南：总结、笔记与 Skill 触发

本文档介绍四种在 Ralph Orchestrator 的 iteration 边界执行额外逻辑的方式，包括自动总结、记笔记、动态触发 skill 等场景。

---

## 背景

Ralph 的主循环以 iteration 为单位推进。每次 iteration 的简化流程如下：

```
pre.iteration.start hook
  → 选择 next hat
  → build prompt（注入 events / skills / scratchpad）
  → 执行 backend CLI（agent 运行）
  → process output（解析 events JSONL）
  → post.iteration.start hook
  → 检查终止条件 → 进入下一轮
```

在这个流程中，有多个扩展点可以插入自定义逻辑。

---

## 方式一：Hooks（外部脚本扩展）

利用 Ralph 内置的生命周期 hooks，在 `post.iteration.start` 或 `pre.iteration.start` 阶段执行外部脚本。

### 适用场景

- 每轮迭代结束后自动写日志/笔记
- 根据 iteration 状态发送通知
- 调用外部工具做分析（如统计代码变更行数）

### 配置示例

```yaml
# ralph.yml
hooks:
  enabled: true
  defaults:
    timeout_seconds: 30
    max_output_bytes: 8192
  events:
    post.iteration.start:
      - name: iteration-logger
        command: ["./scripts/hooks/log-iteration.sh"]
        on_error: warn
```

### Hook 脚本示例

```bash
#!/bin/bash
# scripts/hooks/log-iteration.sh
# 将本轮迭代的摘要追加到笔记文件

PAYLOAD=$(cat)
ITERATION=$(echo "$PAYLOAD" | jq -r '.iteration.current')
HAT=$(echo "$PAYLOAD" | jq -r '.context.active_hat')
TIMESTAMP=$(date -Iseconds)

cat >> .ralph/agent/iteration-log.md << EOF
## Iteration $ITERATION ($TIMESTAMP)
- Active hat: $HAT
- Triggered by: post.iteration.start

EOF
```

### Hook 接收的 Payload 结构

```json
{
  "schema_version": 1,
  "phase": "post",
  "event": "iteration.start",
  "phase_event": "post.iteration.start",
  "timestamp": "2026-05-07T10:30:00Z",
  "loop": {
    "id": "loop-abc123",
    "is_primary": true,
    "workspace": "/path/to/repo",
    "repo_root": "/path/to/repo",
    "pid": 12345
  },
  "iteration": {
    "current": 5,
    "max": 100
  },
  "context": {
    "active_hat": "ralph",
    "selected_hat": "builder",
    "selected_task": "task-123",
    "termination_reason": null,
    "human_interact": null
  },
  "metadata": {
    "accumulated": {}
  }
}
```

### 局限

- Hook 是独立进程，无法直接调用 Ralph 内部 API
- 无法触发 hat 切换或影响下一轮 iteration 的 prompt 内容（除非配合 `mutate`）
- 仅在 iteration 边界触发，无法在 iteration 中间介入

---

## 方式二：Event + Hat（利用 Ralph 编排能力）

让 agent 在 iteration 中主动 emit 事件，由 Ralph 的 event bus 路由到订阅了该事件的 hat，在下一轮 iteration 中处理。

### 适用场景

- Agent 自主决定何时需要做总结（非固定每轮触发）
- 多阶段工作流中，某个阶段完成后触发回顾/审查
- 需要让专门的 "reviewer" 或 "summarizer" hat 介入

### 配置示例

```yaml
# ralph.yml
hats:
  builder:
    name: "Builder"
    subscribes: ["build.request", "test.failed"]
    instructions: "你是构建者，负责实现功能..."

  summarizer:
    name: "Summarizer"
    subscribes: ["summary.request"]
    instructions: |
      你是总结者。当收到 summary.request 事件时：
      1. 回顾最近几轮 iteration 的事件历史
      2. 提取关键决策和变更
      3. 生成简洁的迭代摘要
      4. 将摘要写入 .ralph/agent/summary.md
```

### Agent 触发方式

Agent 在运行中通过 `ralph emit` 发出事件：

```bash
ralph emit summary.request --payload '{"reason": "phase_complete", "focus": "api-refactor"}'
```

该事件被写入 events JSONL 文件，Ralph 在下一轮 iteration 的 `process_events_from_jsonl()` 中读取并发布到 event bus。`next_hat()` 发现 event bus 上有 pending 事件后，会将其注入 prompt，agent 根据 `## HATS` 章节的拓扑信息调用 summarizer 的能力。

### 执行流程

```
Iteration N:
  builder hat 运行 → 发现阶段性工作完成
  → agent 执行: ralph emit summary.request ...
  → 事件写入 events JSONL

Iteration N+1:
  process_events_from_jsonl() 读取 summary.request
  → event bus 上有 summarizer 订阅的 pending 事件
  → build_prompt() 将事件注入 ralph 的 prompt
  → ralph 调用 summarizer 逻辑完成总结
```

### 局限

- Multi-hat mode 下 `next_hat()` 始终返回 "ralph"，实际执行者是 ralph（通过 HATS 章节协调）
- 需要 agent 理解事件拓扑并主动 emit
- 延迟至少一个 iteration

---

## 方式三：Skill 注入（Prompt 层能力增强）

将总结/笔记能力定义为 Skill（Markdown 指令文档），在 prompt 构建时自动注入给指定 hat。

### 适用场景

- 希望某个 hat 持续具备某种能力（如每轮自动评估风险、记录决策）
- 能力是可复用的指令模板，不需要复杂的脚本逻辑
- 希望能力可跨项目复用（skill 文件可独立维护）

### Skill 文件示例

```markdown
<!-- .ralph/skills/iteration-summarizer.md -->
---
name: iteration-summarizer
description: 迭代总结与决策记录能力
auto_inject: true
hats: ["ralph", "builder"]
tags: ["observability", "documentation"]
---

## 迭代总结协议

每完成一轮迭代后，请评估是否需要生成总结：

### 触发条件（满足任一即触发）
- 当前 iteration 数达到 5 的倍数
- 发生了 `plan.created` 或 `task.completed` 事件
- 检测到代码文件被修改（通过 git diff）
- 用户明确要求总结

### 总结内容格式

```markdown
## 迭代摘要 [YYYY-MM-DD HH:MM]

**迭代范围**: N ~ M
**活跃 Hat**: <hat_name>
**核心决策**:
- <决策1>（关联事件: <topic>）
- <决策2>

**文件变更**:
- `path/to/file` (<+/-> 行数)

**待办状态**:
- [ ] 未完成项1
- [x] 已完成项2

**下一步**: <建议>
```

### 输出位置
将摘要追加到 `.ralph/agent/iteration-summaries.md`
```

### 配置启用

```yaml
# ralph.yml
skills:
  enabled: true
  directories:
    - ".ralph/skills"
```

### Skill 注入原理

`event_loop/build_prompt()` 中的调用链：

```
build_prompt(hat_id)
  → base_prompt（核心指令 + HATS 章节）
  → prepend_auto_inject_skills(base_prompt)  ← Skill 在这里注入
  → prepend_scratchpad(with_skills)
  → prepend_ready_tasks(with_scratchpad)
```

`SkillRegistry` 根据当前 hat 的 ID 和 auto_inject 配置，筛选匹配的 skills 拼接到 prompt 顶部。

### 局限

- Skill 是指令，不是强制契约，agent 可能不遵循
- 无法执行外部命令或访问文件系统（需要 agent 使用工具）
- 每次 iteration 都会注入，可能增加 token 消耗

---

## 方式四：Mutate（Hook 与 Loop 状态交互）

通过 hook 的 `mutate` 配置，让外部脚本的输出修改 `accumulated_hook_metadata`，从而影响后续 iteration 的行为。

### 适用场景

- Hook 需要根据运行时状态动态决定是否触发某行为
- 希望 hook 的输出能影响 prompt 内容或 agent 决策
- 需要跨 hook 传递状态

### 配置示例

```yaml
# ralph.yml
hooks:
  enabled: true
  events:
    post.iteration.start:
      - name: smart-summary-gate
        command: ["./scripts/hooks/should-summarize.sh"]
        on_error: warn
        mutate:
          metadata:
            - key: "trigger_summary"
              from_stdout: true
            - key: "summary_focus"
              from_stderr: true

    pre.iteration.start:
      - name: summary-executor
        command: ["./scripts/hooks/execute-summary.sh"]
        on_error: warn
```

### Gate 脚本示例

```bash
#!/bin/bash
# scripts/hooks/should-summarize.sh
# 判断本轮是否需要总结，输出到 stdout

PAYLOAD=$(cat)
ITERATION=$(echo "$PAYLOAD" | jq -r '.iteration.current')

# 每5轮或检测到关键事件时触发总结
if [ $((ITERATION % 5)) -eq 0 ]; then
    echo "true"
    echo "periodic-check" >&2
else
    echo "false"
fi
```

### 执行脚本示例

```bash
#!/bin/bash
# scripts/hooks/execute-summary.sh
# 读取 accumulated metadata，如果 trigger_summary=true 则执行总结

PAYLOAD=$(cat)
TRIGGER=$(echo "$PAYLOAD" | jq -r '.metadata.accumulated.trigger_summary // "false"')
FOCUS=$(echo "$PAYLOAD" | jq -r '.metadata.accumulated.summary_focus // "general"')

if [ "$TRIGGER" = "true" ]; then
    echo "Executing summary with focus: $FOCUS" >&2
    # 执行总结逻辑...
fi
```

### Mutate 传递链

```
Iteration N:
  post.iteration.start
    → smart-summary-gate 运行 → stdout: "true"
    → mutate 将 trigger_summary=true 写入 accumulated metadata

Iteration N+1:
  pre.iteration.start
    → summary-executor 运行 → payload 中 metadata.accumulated.trigger_summary = "true"
    → 脚本读取后执行总结逻辑
```

### 局限

- Metadata 是简单的 key-value（JSON），不适合传递复杂结构
- Mutate 只能写 metadata，无法直接修改 prompt 或 hat 配置
- 需要配合多个 hook 才能实现完整的条件触发链

---

## 四种方式对比

| 维度 | Hooks | Event + Hat | Skill 注入 | Mutate |
|------|-------|-------------|-----------|--------|
| **触发时机** | iteration 边界固定触发 | agent 主动 emit | prompt 构建时注入 | hook 执行后动态写入 |
| **执行主体** | 外部脚本 | agent（通过 ralph 协调） | agent（prompt 指令） | 外部脚本 |
| **能否触发 Hat** | 否 | 是 | 否 | 间接（需配合） |
| **能否修改 Prompt** | 否（直接） | 间接（通过事件注入） | 是 | 间接（通过 metadata） |
| **复杂度** | 低 | 中 | 低 | 中 |
| **灵活性** | 低（固定边界） | 高（agent 自主） | 中（指令化） | 中（状态传递） |
| **token 开销** | 无 | 有（事件 payload） | 有（skill 内容） | 无 |

---

## 组合使用建议

### 场景 A：固定节奏自动总结

```
Skill 注入（给 ralph 添加总结能力）
  + post.iteration.start Hook（每5轮写一个外部摘要文件）
```

### 场景 B：智能条件触发总结

```
post.iteration.start Hook + Mutate（判断是否需要总结）
  → 下一轮的 pre.iteration.start Hook 读取 metadata 执行总结
  → 或 agent 读取 metadata 后 emit summary.request 事件
```

### 场景 C：agent 自主决策总结

```
Skill 注入（教 agent 何时总结、如何总结）
  → agent 自行判断 → emit summary.request → Event + Hat 路由执行
```

---

## 相关源码位置

| 组件 | 文件路径 |
|------|---------|
| Hook 引擎 | `crates/ralph-core/src/hooks/engine.rs` |
| Hook 执行器 | `crates/ralph-core/src/hooks/executor.rs` |
| Hook dispatch | `crates/ralph-cli/src/loop_runner.rs` |
| EventLoop / next_hat | `crates/ralph-core/src/event_loop/mod.rs` |
| Skill Registry | `crates/ralph-core/src/skill_registry.rs` |
| Prompt 构建 | `crates/ralph-core/src/event_loop/mod.rs` (build_prompt) |
| Hat Registry | `crates/ralph-core/src/hat_registry.rs` |

---

*文档版本: v1.0*
*日期: 2026-05-07*
