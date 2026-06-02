---
name: ralph-tools-cmdref
description: ralph tools skill/interact、ralph run 及其他低频命令的完整参考（按需加载）
metadata:
  internal: true
---

# ralph 其他命令参考

> **NEVER use echo/cat to write tasks or memories** — always use CLI tools.

---

## `ralph tools skill`

加载和管理 skill。

> **命名空间选项**：`--root <ROOT>` 只在 `ralph tools` 命名空间下（`memory` / `task` / `skill`）可用，不适用于顶层 `ralph run` / `ralph emit` / `ralph wave emit`。
> **真·全局选项**（所有子命令可用）：`-c/--config`、`-H/--hats`、`-v/--verbose`、`-q/--quiet`、`--color`。
> `skill list` 的 `--format` 支持 `table`、`json`、`quiet`（注意：`quiet` 输出 skill 名称，不是 ID）。
> `skill load` **没有** `format` 选项。

### `ralph tools skill list`

列出可用的 skill。

**语法：**
```bash
ralph tools skill list [OPTIONS]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `quiet` |

**Hat 可见性（P10 operation-guard）：**
- Agent 上下文下，`list` 和 `load` 只能看到 `hats:` 白名单包含当前 hat 的 skill。隐藏的 skill 既不会列出也无法加载。
- Human CLI 上下文（无 `current_hat`）下，所有 skill 均可见，便于审计和调试。
- `backends:` 白名单过滤在 hat 可见性之上仍然生效。

**校验：**
```bash
ralph tools skill list --format quiet | grep '^ralph-tools$'
```

### `ralph tools skill load`

按名称加载并输出 skill 内容。

**语法：**
```bash
ralph tools skill load [OPTIONS] <NAME>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<NAME>` | string | 是 | — | Skill 名称 |

**反模式 / 注意事项：**
- 🔴 如果请求的 skill 对当前 hat 不可见，错误消息中的 "Available skills" 列表只显示可见 skill，不会暴露隐藏 skill 的名称。

**校验：**
```bash
ralph tools skill load ralph-tools | head -n 10
```

---

## `ralph tools interact`

通过 Telegram 与人交互（进度更新、通知）。

> interact 命令没有 root 和 format 选项。

### `ralph tools interact progress`

发送非阻塞的进度更新消息。

**语法：**
```bash
ralph tools interact progress [OPTIONS] <MESSAGE>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<MESSAGE>` | string | 是 | — | 要发送的消息 |

**Guards（P9 operation-guard）：**
- 空消息或纯空格消息会被拒绝（退出码 2）。
- 超过 2000 字符的消息会被拒绝（退出码 2）。
- 每条被接受的消息末尾会自动附加 `[via Ralph agent]`，以便人类区分 agent 通知与人工消息。
- 速率限制为每 5 秒一条，通过进程内互斥锁和跨进程 marker 文件（`.ralph/agent/progress-marker`）双重 enforcing。被限速的调用退出码 75。

**反模式 / 注意事项：**
- 🔴 `ralph tools interact progress` 是非阻塞的；如果需要阻塞等待人类回复，使用 `ralph emit human.interact`。

**校验：**
```bash
# 发送后检查退出码；成功为 0，被限速为 75，被拒绝为 2
ralph tools interact progress "Step 3/5 complete"
echo "Exit code: $?"
```

---

## `ralph run`

启动主编排循环。

**语法：**
```bash
ralph run [OPTIONS] [-- <CUSTOM_ARGS>...]
```

**常用参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `-p, --prompt <PROMPT_TEXT>` | string | 否 | — | 内联提示文本 |
| `-P, --prompt-file <PROMPT_FILE>` | path | 否 | — | 提示文件路径（与 `-p` 互斥） |
| `-b, --backend <BACKEND>` | string | 否 | — | 覆盖配置中的后端 |
| `--max-iterations <MAX_ITERATIONS>` | int | 否 | — | 覆盖最大迭代次数 |
| `--dry-run` | flag | 否 | — | 干运行，不实际执行 |
| `--continue` | flag | 否 | — | 从现有 scratchpad 继续（仅 legacy 模式） |
| `--loop-id <LOOP_ID>` | string | 否 | — | 与 `--continue` 配合使用的显式循环 ID |
| `--no-tui` | flag | 否 | — | 禁用 TUI 观测模式 |
| `-a, --autonomous` | flag | 否 | — | 强制自主模式 |
| `--worktree` | flag | 否 | — | 创建隔离的 git worktree（强制关闭 auto-merge） |
| `--no-auto-merge` | flag | 否 | — | 跳过循环结束后的自动合并（worktree 模式下也适用） |
| `--record-session <FILE>` | path | 否 | — | 录制会话到 JSONL（用于 smoke 测试） |
| `--exclusive` | flag | 否 | — | 使用工作树排他锁，防止并行循环冲突 |
| `--skip-preflight` | flag | 否 | — | 跳过预检检查 |
| `--warmup-only` | flag | 否 | — | 仅预热后退出（不执行编排） |
| `--force-warmup` | flag | 否 | — | 即使未启用也强制后端预热 |
| `--idle-timeout <SECONDS>` | int | 否 | — | 无活动时的超时秒数 |
| `--completion-promise <TEXT>` | string | 否 | — | 输出完成承诺文本（quiet 模式时仍可见） |
| `--rpc` | flag | 否 | — | 启用 RPC 后端连接 |
| `-v, --verbose` | flag | 否 | — | 详细输出 |
| `-q, --quiet` | flag | 否 | — | 抑制流式输出 |

**反模式 / 注意事项：**
- 🔴 `--continue` 仅在未启用 memories/tasks 的 legacy scratchpad 模式下有效。
- 🔴 `--worktree` 创建的隔离目录不会自动合并回主分支（可用 `--no-auto-merge` 控制）。

---

## 其他命令

| 命令 | 说明 |
|------|------|
| `ralph preflight` | 运行预检检查（验证配置与环境） |
| `ralph hooks` | 验证生命周期钩子配置与命令 wiring |
| `ralph doctor` | 诊断环境、配置与依赖状态 |
| `ralph tutorial` | 启动交互式教程 |
| `ralph events` | 查看或操作事件文件 |
| `ralph init` | 初始化 Ralph 工作区 |
| `ralph clean` | 清理临时文件与诊断日志 |
| `ralph plan` | 创建或查看执行计划 |
| `ralph code-task` | 生成代码任务文件 |
| `ralph loops` | 查看与管理并行循环 |
| `ralph hats` | 查看与管理 hats |
| `ralph tui` | 启动 TUI 观测模式 |
| `ralph web` | 启动 Web 仪表板（前后端） |
| `ralph mcp` | MCP 服务器模式 |
| `ralph bot` | 启动 Telegram bot |
| `ralph completions` | 生成 shell 补全脚本 |

> 低频命令的独有参数可通过 `ralph <cmd> --help` 查看。全量参考见 `docs/guide/`。

---

## 错误恢复

| 错误 | 原因 | 修复 |
|------|------|------|
| `agent context requires RALPH_CURRENT_HAT` | 在 agent 上下文调用 `skill list/load` 但 hat 未设置 | 设置 `export RALPH_CURRENT_HAT=<your-hat>` 后重试 |
| `not found` (skill load) | skill 名称错误或对当前 hat 不可见 | `ralph tools skill list --format json` 查看当前 hat 可见的 skill |
| `progress: message must not be empty` | 发送空消息或纯空格 | 提供非空消息 |
| `progress: message length N exceeds max M` | 消息 > 2000 字符 | 拆分消息或用更简洁的描述 |
| 退出码 75 (progress) | 5 秒内重复发送（速率限制）| 等待 5 秒后重试 |
| `--prompt-file` 不存在 | `ralph run -P` 指向不存在的路径 | 检查路径；或用 `-p` 内联提示 |
| `Worktree path conflict` | `--worktree` 路径已被其他循环占用 | 用 `--loop-id` 指定新 ID，或清理已结束的 worktree |
| `preflight failed` | 配置或环境未通过预检 | 查看 `ralph preflight` 输出；常见修复：缺少 `.ralph/`，事件文件不可写 |
| `doctor: check X failed` | 环境检查未过 | 按 `ralph doctor` 的修复建议逐项处理 |
| 任何命令失败 | 通用恢复 | 1. `ralph <cmd> --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |
