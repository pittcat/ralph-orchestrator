---
title: "feat: Rewrite ralph-tools CLI reference with verification steps"
status: active
created: 2026-06-01
updated: 2026-06-01 (v3 — reviewed against source and corrected verification plan)
type: feat
impact: medium
origin: docs/brainstorms/2026-06-01-ralph-cli-agent-reference-requirements.md
---

# 重写 ralph-tools.md：全面 CLI 参考 + 内嵌校验步骤 — 实施计划

## 问题描述

Ralph 编排循环中的 agent 在频繁执行 `ralph` CLI 命令时经常出现幻觉：记错参数、编造选项、搞错行为。现有的 `ralph-tools` skill（`crates/ralph-core/data/ralph-tools.md`）仅覆盖 `ralph tools` 子命令的部分内容，且没有内嵌校验步骤。

**方案变更**：原计划创建新的 `ralph-cli-reference` builtin skill，经讨论后决定直接在现有的 `ralph-tools.md` 中重写，内容全面升级，无需修改注册和注入逻辑。

## 范围边界

| 范围内 | 范围外 |
|--------|--------|
| 重写 `crates/ralph-core/data/ralph-tools.md`，组织为完整的 CLI 命令参考 | 修改 `ralph-tools-tasks.md` 或 `ralph-tools-memories.md` |
| 为每条高频命令添加可执行的校验步骤（bash 代码块） | 修改 `skill_registry.rs` 或 `event_loop/mod.rs` 的注入逻辑 |
| 保留并重新组织经 `--help` 与源码核对后仍正确的现有内容（memory、task、skill、interact、wave） | 修改 ralph CLI 本身的命令行为 |
| 确保 `.claude/skills/ralph-tools/SKILL.md` symlink 自动同步 | 覆盖 `ralph hats`、`ralph loops` 等低频命令的详细内容 |

## 变更概要

```
源文件: crates/ralph-core/data/ralph-tools.md  (~162行 → 预计 ~350-450行)
位置:   crates/ralph-core/data/ 下（不变）
注册:   skill_registry.rs 中已注册（不变）
注入:   event_loop 的 inject_memories_and_tools_skill 特殊注入路径（不变，不依赖 SkillEntry.auto_inject=true）
symlink: .claude/skills/ralph-tools/SKILL.md → ralph-tools.md（symlink 不变，内容自动同步；在扫描 .claude/skills 时 source 可能显示为 file）
```

---

## 系统关键决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 范围 | 重写现有 `ralph-tools.md`，不创建新文件 | 已注册，并通过现有特殊注入路径注入；无需改动基础设施。内容升级即可 |
| 校验步骤 | 每条高频命令后嵌入 `**校验:**` 可执行 bash 代码块 | agent 执行命令后有明确的验证手段，减少幻觉传播 |
| 现有内容 | 保留并重新组织经核对仍正确的内容 | memory、task、skill、interact、wave 的原有说明仍有价值，但旧文件里过宽或过期的表述必须修正 |
| Claude Code skill | 通过现有 symlink 自动同步 | `.claude/skills/ralph-tools/SKILL.md` symlink 指向源文件，无需额外操作 |

---

## 实现单元

---

### U1. 重写 ralph-tools.md 内容

**目标**: 将 ralph-tools.md 从"ralph tools 子命令概览"重写为"完整的 ralph CLI 命令参考 + 校验步骤手册"

**需求**: R1, R2, R3, R4, R5, R6

**依赖**: 无

**文件**:
- `crates/ralph-core/data/ralph-tools.md` — 重写

**方法**:

**1. 保持 frontmatter 不变：**

```yaml
---
name: ralph-tools
description: Shared tool commands for interact, skill, and output format reference during Ralph orchestration
metadata:
  internal: true
---
```

**2. 内容组织结构（按 agent 使用频率排序）：**

| 章节 | 涵盖命令 | 校验步骤 |
|------|---------|---------|
| `ralph emit` | `emit <TOPIC> [PAYLOAD]` + 所有参数 | ✅ 按 `RALPH_EVENTS_FILE` / `.ralph/current-candidate-events` / `.ralph/current-events` / `--file` 顺序解析并检查事件文件 |
| `ralph tools task` | `add/ensure/list/ready/start/close/fail/reopen/show` | ✅ 验证任务状态 |
| `ralph tools memory` | `add/list/search/show/delete/prime/init` | ✅ 搜索确认 |
| `ralph tools skill` | `list/load` | ✅ 验证加载结果 |
| `ralph tools interact` | `progress` | ✅ 确认消息发送 |
| `ralph wave` | `wave emit` | ✅ 按 `RALPH_EVENTS_FILE` / `.ralph/current-events` / 默认 `.ralph/events.jsonl` 顺序解析并检查 wave 事件 |
| `ralph run` | `run` 主要选项 | ❌ run 命令的校验超出 skill 范围 |
| `ralph emit human.interact` | 特殊用法：阻塞式交互 | ✅ 确认 human.interact 事件已发射 |
| `其他命令` | web、loops、hats、bot、mcp、plan、init、clean、doctor、preflight 等 | 仅简要说明 |

**3. 每条命令的标准结构：**

```
### `ralph <command>`

**语法：** `bash 代码块`

**参数：** 表格（参数名、类型、必需/可选、默认值、说明）

**环境变量：** 表格或列表

**反模式/注意事项：** 红线标记 🔴 的陷阱说明

**校验：** `bash 代码块` — 执行后如何验证结果正确
```

**4. 校验步骤示例：**

````markdown
**校验：** 执行后验证事件已正确写入事件文件：

```bash
events_file="${RALPH_EVENTS_FILE:-}"
if [ -z "$events_file" ] && [ -f .ralph/current-candidate-events ]; then
  events_file="$(cat .ralph/current-candidate-events)"
fi
if [ -z "$events_file" ] && [ -f .ralph/current-events ]; then
  events_file="$(cat .ralph/current-events)"
fi
events_file="${events_file:-.ralph/events.jsonl}"

tail -1 "$events_file" | jq 'select(.topic == "<TOPIC>")'
```
````

**5. 保留并修正的现有内容**（重新组织后纳入对应章节）：
- 现有的 Memory Commands → 移到 `ralph tools memory` 章节
- 现有的 Task Commands → 移到 `ralph tools task` 章节
- 现有的 Skill Commands → 移到 `ralph tools skill` 章节
- 现有的 Wave Commands → 移到 `ralph wave` 章节
- 现有的 Interact Commands → 移到 `ralph tools interact` 章节
- 现有的 Decision Journal → 保留，不删除
- 现有的 Output Formats → 改为命令级说明，不再写成“所有命令支持同一组 `--format`”；例如 `memory list` 没有 `--tags`，`task` 没有 `markdown` 格式，`emit` 和 `wave emit` 没有 `--format`

**6. 每个章节的 CLI 参数来源**：从 `ralph <subcommand> --help` 的实际输出逐条核对，确保参数名、类型、默认值完全一致。

**模式参考**: 现有文件自身的前言结构、代码块风格、frontmatter 格式

**CLI 参考数据来源**:

每个命令章节的内容必须基于 `ralph <subcommand> --help` 的实时输出逐条核对。实施前（或实施中）需执行以下命令确认参数：

```bash
# 以下输出将在实施过程中用于逐条核对
ralph emit --help        # emit 的所有参数
ralph tools task --help  # task 所有子命令和选项
ralph tools task add --help
ralph tools task ensure --help
ralph tools task list --help
ralph tools task ready --help
ralph tools task show --help
ralph tools memory --help
ralph tools memory add --help
ralph tools memory list --help
ralph tools memory search --help
ralph tools memory prime --help
ralph tools memory show --help
ralph tools skill --help
ralph tools skill list --help
ralph tools interact --help
ralph wave emit --help
ralph wave --help
ralph run --help
ralph --help             # 顶级命令列表，用于"其他命令"章节
```

**测试场景**:
- T1: YAML frontmatter 保持有效，`name: ralph-tools` 不变
- T2: 所有命令语法与 `ralph <subcommand> --help` 输出一致
- T3: 校验步骤中的命令语法正确（示例命令可手动验证）
- T4: 反模式描述准确，没有误导性表述
- T5: 文件 Markdown 格式正确（无损坏的表格、代码块）

**验证**: 逐条核对 `ralph <subcommand> --help` 输出；`cargo build` 编译通过

---

### U2. 验证加载与特殊注入行为不变

**目标**: 确认重写后的 ralph-tools.md 仍可被加载，且运行时特殊注入路径没有被误改

**需求**: R5

**依赖**: U1

**文件**: 无代码修改，仅验证

**源码事实**:
- `SkillRegistry` 中 builtin skill 的 `auto_inject` 默认是 `false`，只有配置 override 才会变成 `true`。
- `inject_memories_and_tools_skill` 在 memories 或 tasks 启用时直接注入 `skill_registry.get("ralph-tools")`，不依赖 `auto_inject` 字段。
- `ralph tools skill list` 的 table 输出不显示 `auto_inject`；如需检查该字段必须使用 `--format json`，但它不是本功能的成功判据。

**验证步骤**:

1. 构建项目：
```bash
cargo build
```

2. 验证 ralph-tools 在 skill list 中可见：
```bash
cargo run --bin ralph -- tools skill list --format quiet | rg '^ralph-tools$'
```

3. 验证可加载更新后的内容：
```bash
cargo run --bin ralph -- tools skill load ralph-tools | rg '完整的 ralph CLI 命令参考|Ralph CLI'
```

4. 验证现有 skill CLI 集成测试：
```bash
cargo test -p ralph-cli --test integration_skill test_skill_list_includes_builtins
```

**测试场景**:
- T1: `ralph tools skill list --format quiet` 显示 `ralph-tools`
- T2: `ralph tools skill load ralph-tools` 输出重写后的 CLI 参考内容
- T3: 集成测试 `test_skill_list_includes_builtins` 仍然通过（它位于 `ralph-cli` 测试中，断言 ralph-tools 在列表中）

**验证**: `cargo test -p ralph-cli --test integration_skill test_skill_list_includes_builtins` 通过；全量验证在 U3 执行

---

### U3. 全量 CI 验证

**目标**: 确认变更不会破坏现有功能

**需求**: R6

**依赖**: U1, U2

**文件**: 无新文件

**方法**:

```bash
cargo build
cargo test -- --test-threads=1
cargo clippy
cargo fmt --check
cargo test -p ralph-core smoke_runner
```

**验证**: 所有测试通过，无 clippy 警告，fmt 无差异

---

## 系统级影响

| 维度 | 影响 |
|------|------|
| Prompt 大小 | 文件从 ~162 行扩展到约 350-450 行，注入的 prompt 内容相应增加。这是有意的取舍——准确性 > 空间 |
| 启动时间 | 无影响。内容仍通过 include_str! 编译到二进制，不增加额外 I/O |
| 注入逻辑 | 无变化。`inject_memories_and_tools_skill` 仍然在 memories 或 tasks 启用时加载 `skill_registry.get("ralph-tools")` 的内容，不依赖 `auto_inject` 字段 |
| 注册逻辑 | 无变化。builtin skill 注册路径不变；若 `.claude/skills/ralph-tools/SKILL.md` symlink 被扫描，registry source 可能显示为 file，但内容仍来自同一源文件 |
| Claude Code skill | symlink 自动同步，`/ralph-tools` skill 自动获得更新后的内容 |

---

## 风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|---------|
| skill 内容与 CLI 实际行为不一致 | 中 | 高 | 每条命令基于 `--help` 实时输出逐条核对；新增命令时同步更新 |
| 重写时误删有用内容（Decision Journal、Output Formats 等） | 低 | 中 | 保留并重新组织经核对仍正确的内容；Output Formats 必须改成命令级说明，避免继承旧文件中过宽的全局表述 |
| 校验步骤检查错误事件文件 | 中 | 高 | `ralph emit` 和 `ralph wave emit` 的校验代码必须按各自源码中的事件文件解析优先级查找活动文件，不能固定读取 `.ralph/events.jsonl` |
| 校验步骤中的命令本身有语法错误 | 低 | 高 | 每条校验命令在写入前手动验证语法正确性 |
| 内容膨胀导致 prompt 超限 | 低 | 中 | 保持文档精炼（目标 <450 行），避免冗余 |

---

## 实施顺序

```
U1 (重写 ralph-tools.md) → U2 (验证加载/注入) → U3 (全量 CI)
    (无依赖)                (依赖U1)        (依赖U1,U2)
```
