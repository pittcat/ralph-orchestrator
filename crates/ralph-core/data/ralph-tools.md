---
name: ralph-tools
description: Core CLI reference and rules for Ralph orchestration agents
metadata:
  internal: true
---

# Ralph CLI 核心参考

> **前提**：本 skill 仅在 `memories.enabled` 或 `tasks.enabled` 至少一个启用时被注入（`crates/ralph-core/src/event_loop/mod.rs:2062-2075`）。速查表中的"已注入"列均受此条件约束。

> **遇到不确定的命令语法时，先 `ralph <cmd> --help` 再执行。**

## 核心规则

1. **绝不用 echo/cat 写 tasks 或 memories** — 必须用 CLI 工具
2. **emit 后必须校验** — 确认事件已写入事件文件
3. **task/memory 操作后必须确认状态** — 用 `--format json` + `jq` 验证
4. **失败时先查 `--help`** — 不要猜测参数，文档可能已更新

## 命令速查表

### `ralph tools` 命名空间（已注入，按需读取对应子 skill）

| 命令 | 用途 | 详细参考 |
|------|------|---------|
| `ralph tools task` | 任务管理 | 已注入（`ralph-tools-tasks` skill，仅当 `tasks.enabled`） |
| `ralph tools memory` | 记忆管理 | 已注入（`ralph-tools-memories` skill，仅当 `memories.enabled`） |
| `ralph tools skill` | 加载 skill | `ralph tools skill load ralph-tools-cmdref` |
| `ralph tools interact` | Telegram 通知 | `ralph tools skill load ralph-tools-cmdref` |

### 顶层命令（按需加载对应 skill）

| 命令 | 用途 | 详细参考 |
|------|------|---------|
| `ralph emit` | 发射事件（最常用） | `ralph tools skill load ralph-tools-emit` |
| `ralph wave emit` | 并行 wave 调度 | `ralph tools skill load ralph-tools-wave` |
| `ralph run` | 启动编排循环 | `ralph tools skill load ralph-tools-cmdref` |

> **按需加载需要 hat 上下文**：`ralph tools skill load` 在 agent 上下文中要求 `RALPH_CURRENT_HAT` 已设置（`crates/ralph-cli/src/skill_cli.rs:78-87`），否则会以非零退出。如加载失败，先检查 `echo $RALPH_CURRENT_HAT` 是否非空。

## 事件文件解析优先级（`ralph emit` 完整规则）

`ralph emit` 写入路径解析为 3 级回退 + allowlist 校验（`crates/ralph-cli/src/main.rs:243-348`）：

1. 显式 `RALPH_EVENTS_FILE` 环境变量或非默认 `--file`（**必须命中 events allowlist**——来源是 `.ralph/current-candidate-events` 或 `.ralph/current-events` marker——否则 `ralph emit` 拒绝写入并打印 allowlist 内容）
2. `.ralph/current-candidate-events` marker 目标（仅当未提供显式路径时）
3. `.ralph/current-events` marker 目标（仅当未提供显式路径时）
4. `.ralph/events.jsonl` 默认路径（仅当两个 marker 都不存在时）

🔴 **绝不静默回退**：如果设置了 `RALPH_EVENTS_FILE=foo.jsonl` 但 `foo.jsonl` 不在 allowlist 中，命令会**失败**（不会改写到 marker），错误信息会列出当前 allowlist 的所有合法目标。

> `ralph wave emit` 的事件文件解析走 2 级：`RALPH_EVENTS_FILE` → `.ralph/current-events` → `.ralph/events.jsonl`（`crates/ralph-cli/src/wave.rs:210-219`），与 ralph emit 不同。**wave worker 通过 `ralph emit` 返回结果时，事件会写入 candidate-events（与 wave 调度相关），不要改写 `RALPH_EVENTS_FILE` 指向其他文件。**

## 通用错误恢复

| 错误场景 | 可能原因 | 修复方式 |
|----------|---------|---------|
| `events file not in allowlist` | `RALPH_EVENTS_FILE`/`--file` 指向了非 allowlist 路径 | 查看错误信息中列出的 allowlist 条目；如需新路径，先 `touch` 一个 marker 或去掉显式参数 |
| `topic is required` | 缺少必需的位置参数 | 补上 topic 参数 |
| `policy check failed` | 事件不符合策略 | 检查 payload 格式，或确认配置允许 `--unsafe-no-policy-check` |
| `task not found` | task ID 不存在或属于其他 loop | `ralph tools task list` 确认当前可用任务 |
| `memory not found` | memory ID 不存在或无权访问 | `ralph tools memory list` 确认可用记忆 |
| `skill not found` | skill 名称错误或对当前 hat 不可见 | `ralph tools skill list` 确认可用 skill；检查 `RALPH_CURRENT_HAT` |
| `progress rate limited` | 5 秒内重复发送 | 等待 5 秒后重试 |
| 任何命令失败 | 通用恢复 | 1. `ralph <cmd> --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

## Decision Journal

使用 `.ralph/agent/decisions.md` 记录重大决策及其置信度评分。按文件顶部模板填写，ID 保持顺序（DEC-001、DEC-002、...）。

**置信度阈值：**
- **>80**：自主执行。
- **50–80**：继续执行，但需在 `decisions.md` 中记录。
- **<50**：选择最安全的默认方案，并在 `decisions.md` 中记录。
