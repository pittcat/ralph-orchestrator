---
name: ralph-tools-cmdref
description: ralph tools skill/interact、ralph run 及其他低频命令的完整参考（按需加载）
metadata:
  internal: true
---

# ralph 其他命令参考

> **NEVER use echo/cat to write tasks or memories** — always use CLI tools.

---

## `ralph tools skill list`

列出可用的 skill。

> **命名空间选项**：`--root <ROOT>` 只在 `ralph tools` 命名空间下（`memory` / `task` / `skill`）可用。
> **真·全局选项**：`-c/--config`、`-H/--hats`、`-v/--verbose`、`--color`。

**语法：**
```bash
ralph tools skill list [OPTIONS]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `--format <FORMAT>` | enum | 否 | `table` | `table` / `json` / `quiet`（输出 skill 名称） |

**Hat 可见性（P10 operation-guard）：**
- Agent 上下文下，只能看到 `hats:` 白名单包含当前 hat 的 skill。隐藏的 skill 既不会列出也无法加载。
- Human CLI 上下文（无 `current_hat`）下，所有 skill 均可见，便于审计和调试。
- `backends:` 白名单过滤在 hat 可见性之上仍然生效。

**校验：**
```bash
ralph tools skill list --format quiet | grep '^ralph-tools$'
```

## `ralph tools skill load`

按名称加载并输出 skill 内容。

> **命名空间选项**：`--root <ROOT>` 只在 `ralph tools` 命名空间下可用。
> **真·全局选项**：`-c/--config`、`-H/--hats`、`-v/--verbose`、`--color`。
> `skill load` **没有** `--format` 选项。

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
| `--reuse-worktree` | flag | 否 | — | 复用已完成的 worktree；必须与 `--plan` 或 `--worktree-name` 一起使用 |
| `--plan <PLAN_FILE>` | path | 否 | — | 显式 plan 文件；其 basename 用作 worktree 命名前缀 |
| `--worktree-name <NAME>` | string | 否 | — | 显式 worktree 名称（与 `--plan` 互斥） |
| `--no-auto-merge` | flag | 否 | — | 跳过循环结束后的自动合并（worktree 模式下也适用） |
| `--record-session <FILE>` | path | 否 | — | 录制会话到 JSONL（用于 smoke 测试） |
| `--profile <SCOPE:NAME>` | string | 否 | — | 激活 runtime profile overlay（可重复；在 `profiles.default` 之后追加） |
| `--no-default-profiles` | flag | 否 | — | 跳过 `ralph.yml` 中的 `profiles.default`，仅保留 CLI `--profile` |
| `--no-sync-agent-docs` | flag | 否 | — | 跳过启动前对 `CLAUDE.md` / `AGENTS.md` 的 managed block 同步 |
| `--exclusive` | flag | 否 | — | 使用工作树排他锁，防止并行循环冲突 |
| `--skip-preflight` | flag | 否 | — | 跳过预检检查 |
| `--warmup-only` | flag | 否 | — | 仅预热后退出（不执行编排） |
| `--force-warmup` | flag | 否 | — | 即使未启用也强制后端预热 |
| `--idle-timeout <SECONDS>` | int | 否 | — | interactive 模式无活动超时秒数；`0` 表示禁用 |
| `--autonomous-idle-timeout <SECONDS>` | int | 否 | adapter timeout | autonomous / RPC / worktree 后端无输出 watchdog；`0` 表示禁用 |
| `--completion-promise <TEXT>` | string | 否 | — | 输出完成承诺文本（quiet 模式时仍可见） |
| `--rpc` | flag | 否 | — | 启用 RPC 后端连接 |
| `-v, --verbose` | flag | 否 | — | 详细输出 |
| `-q, --quiet` | flag | 否 | — | 抑制流式输出 |

**反模式 / 注意事项：**
- 🔴 `--continue` 仅在未启用 memories/tasks 的 legacy scratchpad 模式下有效。
- 🔴 `--worktree` 创建的隔离目录不会自动合并回主分支（可用 `--no-auto-merge` 控制）。
- 🔴 **Worktree 复用必须显式**: `--reuse-worktree` 现在要求同时提供 `--plan <plan.md>` 或 `--worktree-name <name>`，不再从 prompt 文本中自动猜测 plan 路径（该行为已废弃）。推荐做法：
  ```bash
  ralph -H builtin:<preset> run --worktree --reuse-worktree \
    --plan docs/plans/<your-plan>.md
  ```
- 🔴 `--plan` 与 `--worktree-name` 互斥； `--worktree-name` 会精确匹配 `.worktrees/<NAME>/`，而 `--plan` 使用 plan 文件的 basename 作为前缀并按前缀匹配。

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
| `ralph clean` | 清理 `.ralph/agent` 临时文件；`--diagnostics` 改为清理诊断日志，`--dry-run` 预览不删除 |
| `ralph plan` | 创建或查看执行计划 |
| `ralph code-task` | 生成代码任务文件 |
| `ralph loops` | 查看与管理并行循环 |
| `ralph hats` | 查看与管理 hats |
| `ralph tui` | 启动 TUI 观测模式 |
| `ralph web` | 启动 Web 仪表板（前后端） |
| `ralph mcp` | MCP 服务器模式 |
| `ralph completions` | 生成 shell 补全脚本 |
| `ralph inspect` | 只读诊断命名空间（含 `inspect profiles`；`inspect loop` 输出 loop + hat 身份 + events 路径 + **`loop_anchor` plan 锚点**（优先 `.ralph/agent/.ralph-anchor.json`，否则 `prompt_file`）；JSON 可能含 **`supervisor` 块**（配置启用 supervisor，或 runtime 已有可打开的 wave 账本时出现，否则整键省略），OPAC Observe 一手数据源） |
| `ralph inspect prompt` | 预览 hat 的 prompt 渲染结果（auto-inject + on-demand + 块标题）；Unit 1 支持 `--trigger` / `--source-hat` / `--payload` / `--iteration` / `--wave-context` / `--orchestrator-context` / `--correction` / `--scratchpad` / `--tasks-enabled` / `--memories-enabled` 场景化参数；Unit 2 支持 `--topic` / `--triggered` 候选 emit 干跑评估。`evidence_level` 标识证据等级（`simulated` / `static` / `runtime` / `unverified`） |
| `ralph diagnose` | 离线诊断报告（`--format json` 含 `dup_storm_topics`：`work.ready` 同 dedup key 重复 ≥3 次；ranked findings 的 `hint` 区分 `duplicate_work_done_same_step` / `duplicate_work_done_stall_bypass`） |
| `ralph preset materialize-artifacts` | 将内嵌 artifact 模板写到磁盘（binary-only 安装；见下方专节） |
| `ralph capability inventory` | 列出 preset 作者/reviewer 必须理解的运行时 capability 清单（含 id、trigger_signal、applies_when、evidence_sources、recommended_evidence_level、source）；用于 AAF 评审前对齐 capability 覆盖状态 |

> `ralph bot` 已随 `ralph-telegram` crate 一起删除;运行时不再提供人工通道。`human.guidance` 已废弃;`task.resume` 恢复通道保留(由 runtime diagnosis engine 产出)。

> 低频命令的独有参数可通过 `ralph <cmd> --help` 查看。全量参考见 `docs/guide/`。

---

## `ralph preset materialize-artifacts`

把编译进 `ralph` 二进制的 **artifact 填写模板**落到当前工作区。用于 `parallel-forge` 等 preset：部署机通常只有二进制、没有 `presets/templates/` 源码目录。

**触发条件：** hat instructions 要求先复制模板再填写（planner / executor / reporter），且 `.ralph/forge/<plan-key>/templates/` 尚不存在或需要刷新。

**语法：**
```bash
ralph preset materialize-artifacts <PRESET> --plan-key <KEY> [--dest <DIR>]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<PRESET>` | string | 是 | — | 内嵌模板所属 preset，如 `parallel-forge` 或 `builtin:parallel-forge` |
| `--plan-key <KEY>` | string | 是 | — | 单段路径名（禁止 `/`、`\`、`..`）；默认输出目录用它 |
| `--dest <DIR>` | path | 否 | `.ralph/forge/<KEY>/templates/` | 覆盖输出目录 |

**agent 下一步：**
1. 运行本命令（或确认 templates 目录已存在）。
2. `cp` 对应 `*.template.*` 到业务 artifact 路径（见 hat instructions）。
3. 按模板内 BDD / TDD / Scenario 章节逐节填写；禁止跳过模板写自由格式。

**失败停止条件：**
- preset 无内嵌模板 → 停止，核对 preset 名。
- `--plan-key` 为空或含路径分隔符 → 停止，改用 plan 文件 basename。

**校验：**
```bash
ralph preset materialize-artifacts parallel-forge --plan-key demo
test -f .ralph/forge/demo/templates/development-plan.template.md
```

**反模式：**
- 🔴 不要假设仓库里存在 `presets/templates/parallel-forge/`（binary 安装没有）。
- 🔴 不要把本命令当成 `ralph preset new`（那是生成 preset YAML，不是 fill-in 模板）。

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
| `Worktree path conflict` | `--worktree` 路径已被其他循环占用 | 使用 `--reuse-worktree --worktree-name <NAME>` 复用，或换 `--worktree-name` / `--plan` |
| `preflight failed` | 配置或环境未通过预检 | 查看 `ralph preflight` 输出；常见修复：缺少 `.ralph/`，事件文件不可写 |
| `doctor: check X failed` | 环境检查未过 | 按 `ralph doctor` 的修复建议逐项处理 |
| 任何命令失败 | 通用恢复 | 1. `ralph <cmd> --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |
