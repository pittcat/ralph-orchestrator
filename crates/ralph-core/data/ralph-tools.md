---
name: ralph-tools
description: Shared tool commands for interact, skill, and output format reference during Ralph orchestration
metadata:
  internal: true
---

# Ralph CLI 命令参考与校验手册

本手册覆盖 Ralph 编排过程中最常用的 CLI 命令，包含完整语法、参数说明、常见陷阱与可执行的校验步骤。

> **NEVER use echo/cat to write tasks or memories** — always use CLI tools.

---

## `ralph emit`

向当前运行的事件文件发射一个结构化事件。这是 agent 与编排器通信的核心机制。

**语法：**
```bash
ralph emit [OPTIONS] <TOPIC> [PAYLOAD]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<TOPIC>` | string | 是 | — | 事件主题，如 `build.done`、`review.complete` |
| `[PAYLOAD]` | string/json | 否 | `""` | 事件负载；配合 `-j` 可解析为 JSON 对象 |
| `-j, --json` | flag | 否 | — | 将 payload 按 JSON 对象解析而非普通字符串 |
| `--file <FILE>` | path | 否 | `.ralph/events.jsonl` | 目标事件文件路径 |
| `--policy-check` | flag | 否 | — | 发射前按当前事件策略校验 |
| `--unsafe-no-policy-check` | flag | 否 | — | 跳过强制策略检查（仅当配置允许时） |
| `--hat <HAT>` | string | 否 | `$RALPH_CURRENT_HAT` | 发布此事件的 hat |
| `--triggered <TRIGGERED>` | string | 否 | `$RALPH_TRIGGERED_HAT` | 被此事件触发的目标 hat |
| `--source <SOURCE>` | string | 否 | `$RALPH_EVENT_SOURCE` | 事件来源标识 |

**环境变量：**

| 变量 | 作用 |
|------|------|
| `RALPH_EVENTS_FILE` | 非空时，直接作为事件文件路径（最高优先级） |
| `RALPH_CURRENT_HAT` | 回退到 `--hat` |
| `RALPH_TRIGGERED_HAT` | 回退到 `--triggered` |
| `RALPH_EVENT_SOURCE` | 回退到 `--source` |

**事件文件解析优先级：**
1. `RALPH_EVENTS_FILE` 环境变量（非空时）
2. `.ralph/current-candidate-events` marker 文件
3. `--file` CLI 参数（默认 `.ralph/events.jsonl`）

**反模式 / 注意事项：**
- 🔴 **不要**在 wave worker 内部使用 `ralph emit` 发射 wave 事件；worker 应直接通过标准输出或 `ralph emit` 返回结果，而不是触发新 wave。
- 🔴 `--unsafe-no-policy-check` 仅在配置显式允许时可用，否则会导致校验失败。
- 🔴 `ralph emit` **没有** `--format` 选项。

**校验：**
```bash
# 1. 确定实际写入的事件文件（与 ralph emit 源码一致）
events_file="${RALPH_EVENTS_FILE:-}"
if [ -z "$events_file" ] && [ -f .ralph/current-candidate-events ]; then
  events_file="$(cat .ralph/current-candidate-events)"
fi
events_file="${events_file:-${FILE_ARG:-.ralph/events.jsonl}}"

# 2. 确认事件已追加到文件末尾
tail -n 1 "$events_file" | jq -e ".topic == \"YOUR_TOPIC\""

# 3. 确认 payload 格式正确（若使用了 -j）
tail -n 1 "$events_file" | jq -e '.payload | type == "object"'
```

---

## `ralph tools task`

任务管理命令集。任务替代 scratchpad 用于追踪跨迭代的工作项。

> 全局选项（`--root`、`-c/--config`、`-H/--hats`、`-v/--verbose`、`--color`）对所有子命令可用，下文不再重复列出。
> `task` 命令的 `--format` 仅支持 `table`、`json`、`quiet`（不支持 `markdown`）。

### `ralph tools task add`

创建新任务。

**语法：**
```bash
ralph tools task add [OPTIONS] <TITLE>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<TITLE>` | string | 是 | — | 任务标题 |
| `-p, --priority <PRIORITY>` | int | 否 | `3` | 优先级（1–5，1 最高） |
| `-d, --description <DESCRIPTION>` | string | 否 | — | 任务描述 |
| `--blocked-by <BLOCKED_BY>` | string | 否 | — | 必须先完成的 Task ID（逗号分隔） |
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `quiet` |

**校验：**
```bash
# 记录返回的 task ID，然后验证任务存在
ralph tools task list | grep -i "YOUR_TITLE"
```

### `ralph tools task ensure`

通过稳定 key 创建或复用任务（幂等）。

**语法：**
```bash
ralph tools task ensure [OPTIONS] --key <KEY> <TITLE>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<TITLE>` | string | 是 | — | 任务标题 |
| `--key <KEY>` | string | **是** | — | 用于去重的稳定 key |
| `-p, --priority <PRIORITY>` | int | 否 | `3` | 优先级 |
| `-d, --description <DESCRIPTION>` | string | 否 | — | 描述 |
| `--blocked-by <BLOCKED_BY>` | string | 否 | — | 依赖任务 ID |
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `quiet` |

**反模式 / 注意事项：**
- 🔴 `--key` 是必需的，省略会导致命令失败。

**校验：**
```bash
# 重复执行应返回同一个 task ID
ralph tools task ensure "Setup DB" --key setup-db | tee /tmp/task1.txt
ralph tools task ensure "Setup DB" --key setup-db | tee /tmp/task2.txt
diff /tmp/task1.txt /tmp/task2.txt
```

### `ralph tools task list`

列出所有任务。

**语法：**
```bash
ralph tools task list [OPTIONS]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `-s, --status <STATUS>` | enum | 否 | — | 按状态过滤：`open` / `in_progress` / `closed` / `failed` |
| `-d, --days <DAYS>` | int | 否 | — | 仅显示最近 N 天的任务 |
| `-l, --limit <LIMIT>` | int | 否 | — | 限制显示数量 |
| `-a, --all` | flag | 否 | — | 显示已关闭/失败的任务（默认隐藏） |
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `quiet` |

**校验：**
```bash
ralph tools task list --format json | jq '.[].status' | sort -u
```

### `ralph tools task ready`

显示未阻塞的任务（无未完成依赖）。

**语法：**
```bash
ralph tools task ready [OPTIONS]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `-a, --all` | flag | 否 | — | 显示所有循环的任务，不只是当前循环 |
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `quiet` |

### `ralph tools task start`

将任务标记为进行中。

**语法：**
```bash
ralph tools task start [OPTIONS] <ID>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<ID>` | string | 是 | — | 任务 ID |

**校验：**
```bash
ralph tools task show <ID> --format json | jq -e '.status == "in_progress"'
```

### `ralph tools task close`

将任务标记为完成。

**语法：**
```bash
ralph tools task close [OPTIONS] <ID>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<ID>` | string | 是 | — | 任务 ID |

**校验：**
```bash
ralph tools task show <ID> --format json | jq -e '.status == "closed"'
```

### `ralph tools task fail`

将任务标记为失败。

**语法：**
```bash
ralph tools task fail [OPTIONS] <ID>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<ID>` | string | 是 | — | 任务 ID |

**校验：**
```bash
ralph tools task show <ID> --format json | jq -e '.status == "failed"'
```

### `ralph tools task reopen`

重新打开已关闭或失败的任务。

**语法：**
```bash
ralph tools task reopen [OPTIONS] <ID>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<ID>` | string | 是 | — | 任务 ID |

**校验：**
```bash
ralph tools task show <ID> --format json | jq -e '.status == "open"'
```

### `ralph tools task show`

显示单个任务详情。

**语法：**
```bash
ralph tools task show [OPTIONS] <ID>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<ID>` | string | 是 | — | 任务 ID |
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `quiet` |

---

## `ralph tools memory`

管理持久化记忆，用于累积项目知识。

> 全局选项（`--root`、`-c/--config`、`-H/--hats`、`-v/--verbose`、`--color`）对所有子命令可用。
> `memory` 命令的 `--format` 支持 `table`、`json`、`markdown`、`quiet`。

### `ralph tools memory add`

存储新记忆。

**语法：**
```bash
ralph tools memory add [OPTIONS] <CONTENT>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<CONTENT>` | string | 是 | — | 记忆内容 |
| `-t, --type <TYPE>` | enum | 否 | `pattern` | 记忆类型（见下表） |
| `--tags <TAGS>` | string | 否 | — | 逗号分隔的标签 |
| `--private` | flag | 否 | — | 标记为当前 hat 私有 |
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `markdown` / `quiet` |

**记忆类型：**

| 类型 | 用途 |
|------|------|
| `pattern` | 代码风格、命名约定、项目惯例 |
| `decision` | 架构决策及其理由 |
| `fix` | 错误模式与解决方案 |
| `context` | 项目结构、模块关系等背景知识 |

**校验：**
```bash
# 获取刚添加的记忆 ID（quiet 输出仅为 ID）
mem_id=$(ralph tools memory add "Uses barrel exports" -t pattern --tags api --format quiet)
ralph tools memory show "$mem_id"
```

### `ralph tools memory list`

列出所有记忆。

**语法：**
```bash
ralph tools memory list [OPTIONS]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `-t, --type <TYPE>` | enum | 否 | — | 按类型过滤 |
| `--last <LAST>` | int | 否 | — | 仅显示最近 N 条 |
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `markdown` / `quiet` |

**校验：**
```bash
ralph tools memory list --format json | jq 'length'
```

### `ralph tools memory search`

按查询词模糊搜索记忆。

**语法：**
```bash
ralph tools memory search [OPTIONS] [QUERY]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `[QUERY]` | string | 否 | — | 搜索词（对内容和标签模糊匹配） |
| `-t, --type <TYPE>` | enum | 否 | — | 按类型过滤 |
| `--tags <TAGS>` | string | 否 | — | 按标签过滤（逗号分隔，OR 逻辑） |
| `--all` | flag | 否 | — | 显示所有结果（无限制） |
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `markdown` / `quiet` |

**校验：**
```bash
ralph tools memory search "barrel" --format json | jq 'length'
```

### `ralph tools memory prime`

输出记忆内容用于上下文注入（默认 markdown 格式）。

**语法：**
```bash
ralph tools memory prime [OPTIONS]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `--budget <BUDGET>` | int | 否 | `0` | 最大 token 数（0 = 无限制） |
| `-t, --type <TYPE>` | string | 否 | — | 按类型过滤（逗号分隔） |
| `--tags <TAGS>` | string | 否 | — | 按标签过滤（逗号分隔） |
| `--recent <RECENT>` | int | 否 | — | 仅最近 N 天的记忆 |
| `--format <FORMAT>` | enum | 否 | `markdown` | `table` / `json` / `markdown` / `quiet` |

**校验：**
```bash
ralph tools memory prime --budget 2000 | head -n 5
```

### `ralph tools memory show`

显示单条记忆。

**语法：**
```bash
ralph tools memory show [OPTIONS] <ID>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<ID>` | string | 是 | — | 记忆 ID，如 `mem-1737372000-a1b2` |
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `markdown` / `quiet` |

### `ralph tools memory delete`

删除记忆。

**语法：**
```bash
ralph tools memory delete [OPTIONS] <ID>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<ID>` | string | 是 | — | 记忆 ID |

**反模式 / 注意事项：**
- 🔴 删除后无法恢复，确认 ID 正确后再执行。

### `ralph tools memory init`

初始化记忆文件。

**语法：**
```bash
ralph tools memory init [OPTIONS]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `--force` | flag | 否 | — | 覆盖已有文件 |

---

### 何时搜索记忆

**开始工作前搜索：**
- 进入不熟悉的代码区域 → `ralph tools memory search "area-name"`
- 遇到错误 → `ralph tools memory search -t fix "error message"`
- 做架构决策前 → `ralph tools memory search -t decision "topic"`
- 感觉似曾相识 → 可能有相关记忆

**搜索策略：**
- 先宽泛，再精确：`search "api"` → `search -t pattern --tags api`
- 错误优先查 fix：`search -t fix "ECONNREFUSED"`
- 改架构前查 decision：`search -t decision`

### 何时创建记忆

**创建记忆的场景：**
- 发现代码库的工作方式（pattern）
- 做或学到架构决策的理由（decision）
- 解决了可能复发的问题（fix）
- 学到他人需要的项目知识（context）
- 任何非零退出、缺失依赖/技能、或被阻塞的步骤（fix + task）

**不要创建记忆的场景：**
- 会话特定状态（用 task）
- 显而易见的通用实践
- 临时 workaround

### 失败捕获（通用规则）

如果命令失败（非零退出）、缺失依赖/技能、或被阻塞：
1. **记录 fix 记忆**，包含精确命令、错误信息和预期修复。
2. **如果同一迭代内无法解决，创建或重新打开任务。**

```bash
ralph tools memory add \
  "failure: cmd=<command>, exit=<code>, error=<message>, next=<intended fix>" \
  -t fix --tags tooling,error-handling

ralph tools task ensure "Fix: <short description>" --key fix:<short-key> -p 2
```

### 记忆最佳实践

1. **具体**："Uses barrel exports in each module" 而非 "Has good patterns"
2. **包含理由**："Chose X because Y" 而非 "Uses X"
3. **一概念一条**：拆分复杂的学习点
4. **标签一致**：尽可能复用已有标签

---

## `ralph tools skill`

加载和管理 skill。

> 全局选项（`--root`、`-c/--config`、`-H/--hats`、`-v/--verbose`、`--color`）对所有子命令可用。
> `skill list` 的 `--format` 支持 `table`、`json`、`quiet`（注意：`quiet` 输出 skill 名称，不是 ID）。
> `skill load` **没有** `--format` 选项。

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

> `interact` 命令没有 `--root` 和 `--format` 选项。

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

## `ralph wave`

调度 wave 事件以实现并行 hat 执行。

> `wave` 命令没有 `--root` 和 `--format` 选项。

### `ralph wave emit`

将多个 payload 作为 wave 事件发射，每个 payload 成为一个独立事件，共享同一个 `wave_id`。

**语法：**
```bash
ralph wave emit [OPTIONS] <TOPIC>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<TOPIC>` | string | 是 | — | 所有 wave 事件的主题（如 `review.file`） |
| `--payloads <PAYLOADS>...` | string… | 否 | — | 每个 wave worker 一个 payload |

**事件文件解析优先级：**
1. `RALPH_EVENTS_FILE` 环境变量（非空时）
2. `.ralph/current-events` marker 文件
3. 默认 `.ralph/events.jsonl`

> 注意：`wave emit` 与 `ralph emit` 的事件文件解析逻辑不同——`wave emit` 使用 `current-events`，而 `ralph emit` 使用 `current-candidate-events`。

**约束：**
- 不能在 wave worker 内部使用（`RALPH_WAVE_WORKER=1` 时会阻止）。
- Wave worker 的结果应通过 `ralph emit` 返回，而非 `ralph wave emit`。

**反模式 / 注意事项：**
- 🔴 `ralph wave emit` **没有** `--format` 选项。
- 🔴 不要在 wave worker 内部调用 `ralph wave emit`。

**校验：**
```bash
# 1. 确定事件文件（与 wave emit 源码一致）
events_file="${RALPH_EVENTS_FILE:-}"
if [ -z "$events_file" ] && [ -f .ralph/current-events ]; then
  events_file="$(cat .ralph/current-events)"
fi
events_file="${events_file:-.ralph/events.jsonl}"

# 2. 检查 wave 事件已写入
tail -n 3 "$events_file" | jq -s 'map(select(.topic == "YOUR_TOPIC")) | length'
```

---

## `ralph emit human.interact`

阻塞式人机交互的特殊用法。

当 agent 需要向人类提问并阻塞等待回答时，发射 `human.interact` 事件：

```bash
ralph emit human.interact '{"question":"Should I use Redis or Memcached?"}' -j
```

循环会检测到该事件，通过 Telegram 发送问题，并阻塞直到收到 `human.response` 或超时。

**反模式 / 注意事项：**
- 🔴 仅当确实需要阻塞等待回答时才使用；非阻塞通知应使用 `ralph tools interact progress`。
- 🔴 不要在无 RObot 配置的环境中使用，否则会导致无限阻塞直到超时。

**校验：**
```bash
events_file="${RALPH_EVENTS_FILE:-}"
if [ -z "$events_file" ] && [ -f .ralph/current-candidate-events ]; then
  events_file="$(cat .ralph/current-candidate-events)"
fi
events_file="${events_file:-.ralph/events.jsonl}"
tail -n 1 "$events_file" | jq -e '.topic == "human.interact"'
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
| `--worktree` | flag | 否 | — | 创建隔离的 git worktree |
| `--record-session <FILE>` | path | 否 | — | 录制会话到 JSONL（用于 smoke 测试） |
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

---

## Appendix: Decision Journal

使用 `.ralph/agent/decisions.md` 记录重大决策及其置信度评分。按文件顶部模板填写，ID 保持顺序（DEC-001、DEC-002、...）。

**置信度阈值：**
- **>80**：自主执行。
- **50–80**：继续执行，但需在 `.ralph/agent/decisions.md` 中记录。
- **<50**：选择最安全的默认方案，并在 `.ralph/agent/decisions.md` 中记录。

**模板字段：**
- Decision
- Chosen Option
- Confidence (0–100)
- Alternatives Considered
- Reasoning
- Reversibility
- Timestamp (UTC ISO 8601)
