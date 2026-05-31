# Ralph CLI Agent Reference Skill — 需求文档

**创建日期**: 2026-06-01
**状态**: 已定稿（v3 — 审查修订）
**上游**: ce-brainstorm (2026-06-01 会话)

---

## 问题陈述

Ralph Orchestrator 的 builtin preset（`code-assist`、`research`、`review` 等）在运行时需要 agent 频繁执行 `ralph` CLI 命令——包括 `ralph emit`、`ralph tools memory add`、`ralph tools task`、`ralph wave emit` 等。然而，agent 在执行这些命令时经常出现幻觉：

- 记错子命令名称或参数标志
- 编造不存在的命令或参数组合
- 搞错默认文件路径和环境变量
- 在需要 `--json` 参数时使用纯文本
- 对命令行为产生错误预期

目前已有的 `ralph-tools` skill 仅覆盖了 `ralph tools` 子命令（memory、task、skill、interact），且内容有限、缺少校验步骤。agent 需要一个**全面的、权威的、内嵌校验步骤的 CLI 命令参考**。

---

## 用户角色

| ID | 角色 | 描述 |
|----|------|------|
| A1 | **Ralph 内部 Agent** | 在 Ralph 编排循环内运行的 LLM agent，需要执行 `ralph` CLI 命令来完成工作 |
| A2 | **Preset 开发者** | 编写和维护 hat collection preset 的开发者在设计新 preset 时需要确保 agent 有正确的命令参考 |

**核心用户是 A1**——Ralph 内部 agent。A2 是次要用户。

---

## 核心需求

### R1. 覆盖高频 CLI 命令

Skill 必须详尽覆盖 agent 在编排循环中最常调用的命令，包括但不限于：

- `ralph emit` — 事件发射（最核心、最易错的命令）
- `ralph tools memory` — 记忆管理（所有子命令）
- `ralph tools task` — 任务管理（所有子命令）
- `ralph tools skill load/list` — 技能加载
- `ralph tools interact progress` — 进度通知
- `ralph wave emit` — 波次并行执行
- `ralph run` — 编排循环启动（主要选项）
- `ralph emit human.interact` — 阻塞式人类交互

边缘命令（如 `ralph web`、`ralph mcp`、`ralph bot`、`ralph completions` 等）在 skill 末尾给出简要说明，不作为核心内容。

### R2. 精确的语法和参数说明

每条命令必须包含：
- **规范格式**：包含所有必要参数和常用选项的完整示例
- **参数表**：列出每个参数/标志的名称、类型、默认值、用途
- **环境变量**：列出影响该命令的环境变量（如 `$RALPH_CURRENT_HAT`、`$RALPH_TRIGGERED_HAT`、`$RALPH_EVENT_SOURCE`、`$RALPH_WAVE_WORKER`、`$RALPH_CONFIG` 等）

### R3. 常见陷阱与反模式

针对高频命令，必须包含 agent 最容易犯错误的场景说明：

- `ralph emit` 中 `--json` 与纯文本 payload 的区别
- `ralph emit` 的 `--hat` / `--triggered` / `--source` 环境变量回退机制
- `ralph emit --policy-check` 与 `--unsafe-no-policy-check` 的适用场景
- `ralph wave emit` 不能在 wave worker 内部使用（`$RALPH_WAVE_WORKER=1` 阻止）
- `ralph tools memory` 与 `ralph tools task` 的 ID 格式
- `ralph tools task ensure` 的 `--key` 幂等性机制
- 永远不要使用 `echo` / `cat` 写入 tasks 或 memories

### R4. 内嵌校验步骤

**核心需求**：不仅仅告诉 agent "执行什么命令"，还要告诉 agent "执行后如何验证结果正确"。

每条高频命令必须包含 **校验步骤**，例如：

- `ralph emit` 后 → 按实际解析优先级检查活动事件文件：`$RALPH_EVENTS_FILE` → `.ralph/current-candidate-events` → `.ralph/current-events` → `--file` 默认值 `.ralph/events.jsonl`
- `ralph tools task close` 后 → 验证任务状态已更新
- `ralph tools memory add` 后 → 搜索确认记忆已存储
- `ralph wave emit` 后 → 按 wave 的事件文件解析优先级检查事件：`$RALPH_EVENTS_FILE` → `.ralph/current-events` → 默认 `.ralph/events.jsonl`

校验步骤以可执行的 `bash` 代码块形式给出，agent 可以直接执行。

### R5. 合并到现有 ralph-tools

**不创建新 skill**。直接在现有的 `crates/ralph-core/data/ralph-tools.md` 中重写，让它成为全面的 CLI 命令参考 + 校验步骤手册。

- `ralph-tools.md` → 从"ralph tools 子命令概览"升级为"完整的 ralph CLI 命令参考"
- 保留原有内容中经 `--help` 与源码核对后仍正确的部分（memory、task、skill、interact、wave），但重新组织并补充所有其他高频命令；已过期或过宽的旧表述必须修正，例如 `memory list` 不支持 `--tags`、并非所有命令都支持同一组 `--format`
- 原有内容中与 ralph-tools-tasks.md、ralph-tools-memories.md 重叠的部分保留（因为它们各自独立注入不同的场景）

### R6. 静态文档 + 定期更新

Skill 内容为静态 Markdown 文件（`crates/ralph-core/data/ralph-tools.md`）。当 ralph CLI 的命令或参数发生变更时，由开发者手动更新此文件。

---

## 非目标

| 项目 | 理由 |
|------|------|
| 不自动生成文档 | 命令变化频率不高，静态文档足够，且可避免生成工具的维护成本 |
| 不修改 `ralph-tools-tasks.md` 或 `ralph-tools-memories.md` | 它们各自独立注入不同场景，保留它们的独立性 |
| 不覆盖 `ralph hats validate/graph/list/show` 等低频命令的详细内容 | 这些命令有 `--help`，在 skill 中列一条带说明即可 |
| 不覆盖 ralph 配置系统 | config.yml 的配置参数属于配置文档范畴，不是 CLI 参考 |

---

## 关键决策

| 决策 | 选择 | 理由 |
|------|------|------|
| Skill 类型 | 扩展现有 `ralph-tools` skill 源文件（`crates/ralph-core/data/ralph-tools.md`） | 重写而非新建；builtin 注册和 `.claude` symlink 都指向同一内容，无需额外改动基础设施 |
| 注入方式 | 保持现有特殊注入机制不变 | `inject_memories_and_tools_skill` 在 memories 或 tasks 启用时直接注入 `skill_registry.get("ralph-tools")` 的内容，不依赖 `SkillEntry.auto_inject=true` |
| 文件位置 | `crates/ralph-core/data/ralph-tools.md` | 已有文件，直接重写 |
| 校验步骤 | 每条高频命令后嵌入 `bash` 代码块 | 让 agent 在执行后有明确的验证手段，减少幻觉传播 |
| .claude symlink | `ralph-tools.md` 的 symlink 保持不变 | Claude Code 侧的 `/ralph-tools` skill 自动同步更新 |

---

## 依赖与假设

- 假设 ralph CLI 的命令结构在可见未来不会发生根本性变化
- 依赖现有的 `event_loop/mod.rs` 特殊注入管道（`inject_memories_and_tools_skill` 方法）
- 依赖现有的 `skill_registry.rs` 的 builtin 注册流程
- `ralph-tools` 通过 `inject_memories_and_tools_skill` 特殊路径注入；`ralph tools skill list --format json` 中的 `auto_inject` 字段不应作为该注入路径是否生效的判据

---

## 成功标准

1. Agent 在 `ralph emit`、`ralph tools`、`ralph wave emit` 等高频命令上的错误率显著下降
2. Skill 内容与 `ralph --help` 及 `ralph <subcommand> --help` 的输出保持 100% 一致
3. 每条高频命令都有可执行的校验步骤，agent 能通过校验步骤验证执行结果
4. 新增命令或参数变更时，能在 15 分钟内完成 skill 的同步更新
5. `ralph tools skill load ralph-tools` 能加载更新后的内容；`ralph tools skill list --format quiet` 仍包含 `ralph-tools`，且运行时特殊注入路径保持不变
