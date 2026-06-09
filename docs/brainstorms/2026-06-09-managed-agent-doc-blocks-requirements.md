---
date: 2026-06-09
topic: managed-agent-doc-blocks
---

## Summary

在 `ralph run` 启动 backend（`CliBackend::from_config` 之前）插入一个"managed agent doc blocks"同步步骤：检测 `cwd/CLAUDE.md` 和 `cwd/AGENTS.md` 是否已经包含由 ralph 维护的标记块；缺则按 `block_id` 追加到文件末尾的 `## Ralph Managed Blocks` section；存在但 `content_hash` 失配则原地升级。首发块 `hang-prevention` 把"Command Hang Prevention Rules"五条规则固化到 ralph-cli 二进制里，保证每个 ralph run 启动的 agent 都能零摩擦看到这份约束，从而消除"agent 因 hang 而烧 token"的复发路径。

## Problem Frame

2026-06-05 一次 `ce-executor` loop 跑挂，根因是 backend agent 在无 timeout 的 `tail -f` 等长任务里无限阻塞。事后通过 19 个 preset 注入"禁止 kill 父 ralph"禁令做了止血，但**没有**给所有 agent 一份通用的"无限跟随命令禁令"——下一波 hang 模式（CI 容器里 `journalctl -f`、本地调试 `dmesg -w`、监控 `watch`）如果换个 backend 跑出来还会复发。当前的痛点是：约束**写在 ralph 仓库内部**（CLAUDE.md / commit message / 散在各 preset 里），**没有一份** agent 启动时一定读得到、且不依赖 ralph 版本 / 项目状态 / 注入时机的"硬约束文本"。这次要做的就是：让 ralph 在启动 agent 之前先把这份硬约束写入 agent 必然读取的 `CLAUDE.md` / `AGENTS.md`，并且 idempotent、可升级、可逃生。

## Key Decisions

- **同步阻塞 I/O，不 `tokio::spawn` 后台**：`sync` 必须在 backend spawn 之前完成，否则约束晚于 agent 启动读到，等于失效。`cwd/CLAUDE.md` + `cwd/AGENTS.md` 两个文件 + 单 section 写入，毫秒级，对启动体感无影响。
- **块内容进 ralph-cli 二进制，不放 ralph.yml**：跨机可重复、与 ralph 版本绑定、用户升级 ralph 时块自动跟随升级；代价是 ralph.yml 不能在项目级 disable 单块内容（仅能 disable 整段 sync）。
- **默认 `enabled: true` + `on_error: warn`**：零摩擦启用，失败不阻塞启动；CI / 严格环境可改 `strict`。
- **MVP 只覆盖 `ralph run`**：`ralph plan` / `ralph wave emit` / `ralph task` 等也会 spawn agent，但 MVP 不强制；保持范围窄、首交付可验证。
- **末尾追加 `## Ralph Managed Blocks` section，不动用户手写内容**：避免破坏用户既有 CLAUDE.md / AGENTS.md 的可读性，也避免 git diff 冲突。

## Key Flows

- F1. 首次启动（`cwd/CLAUDE.md` 不存在）
  - **Trigger:** 用户在空项目目录运行 `ralph run -p "..."`。
  - **Steps:** `agent_doc_sync::sync_all(cwd)` 创建 `cwd/CLAUDE.md` 与 `cwd/AGENTS.md`（如果缺），每个文件末尾追加 `## Ralph Managed Blocks` section 和 `hang-prevention` 块；`fs2` 文件锁互斥。
  - **Outcome:** 两个文件落地，含带 `v=sha256:xxx` marker 的块；backend 启动后 agent 读到约束。
  - **Covers:** R1, R2, R3, R5, R6, R14.

- F2. 块已存在且 `content_hash` 一致（skip）
  - **Trigger:** 已有 `cwd/CLAUDE.md` 且含 `hang-prevention` 块，`v=` 与当前 builtin 内容哈希匹配。
  - **Steps:** 检测到成对 `<!-- ralph:begin hang-prevention v=... -->` / `<!-- ralph:end hang-prevention -->` 且 v 一致 → 跳过；log.info 一行 "agent_doc_sync: skipped hang-prevention (up to date) in CLAUDE.md"。
  - **Outcome:** 文件无任何写入；sync 阶段耗时 <1ms。
  - **Covers:** R3, R4.

- F3. 块已存在但 `content_hash` 失配（升级）
  - **Trigger:** 升级 ralph 后内置 `hang-prevention.md` 内容变化。
  - **Steps:** 检测到 v 失配 → 解析 begin/end 块边界 → 用新内容替换该段（保留 `## Ralph Managed Blocks` section 标题与同级其他块）→ 更新 v= 新哈希；log.info "agent_doc_sync: upgraded hang-prevention to v=..."。
  - **Outcome:** 用户手写内容零改动，块内容升级。
  - **Covers:** R3, R4, R16.

- F4. 并行 loop 同项目写竞争
  - **Trigger:** 同一 cwd 同时跑两个 ralph run（worktree loop + primary loop 罕见场景）。
  - **Steps:** `sync_all` 入口用 `fs2` 文件锁串行化；持锁失败回退：retry 3 次每次 sleep 50ms；3 次失败则走 R9 on_error 策略。
  - **Outcome:** 不出现两个进程互相覆盖半写状态。
  - **Covers:** R5, R15.

## Requirements

### 同步机制（core mechanism）

- R1. `ralph run` 命令在创建 `CliBackend` 之前同步调用 `agent_doc_sync::sync_all(cwd)`；不通过 `tokio::spawn` 后台化，sync 完成前不进入 backend spawn 路径。
- R2. `sync_all` 扫两个固定路径 `cwd/CLAUDE.md` 和 `cwd/AGENTS.md`；不递归子目录、不读家目录、不读其他文件名。
- R3. 检测算法：对每个 block_id，扫描文件内 `<!-- ralph:begin BLOCK_ID v=HASH -->` / `<!-- ralph:end BLOCK_ID -->` 成对出现；成对且 v 等于当前 builtin 内容 sha256 哈希 → skip；缺 begin/end 任一 → 在文件末尾追加新段；成对但 v 失配 → 替换该 begin/end 之间的内容并更新 v。
- R4. marker 格式必须为 HTML 注释包裹的成对标签：`<!-- ralph:begin hang-prevention v=sha256:HEX -->` ... `<!-- ralph:end hang-prevention -->`；HEX 是 64 字符小写 sha256。注释前后各保留一个空行以保证 markdown 渲染清爽。
- R5. 写文件前对目标路径加 `fs2` 文件锁（`flock` 语义）；持锁失败 retry 3 次每次 sleep 50ms，3 次失败走 R9 on_error 策略。

### 块内容来源（block content source）

- R6. builtin 块内容以 markdown 文件存放在 `crates/ralph-core/data/managed_blocks/<block_id>.md`，由 `crates/ralph-core/build.rs` 通过 `include_str!` 嵌入 ralph-cli 二进制。
- R7. 首发 builtin 块 `block_id = "hang-prevention"`，对应内容是用户指定的 5 条 Command Hang Prevention Rules 全文（不简化、不重写）。
- R8. builtin 块列表由 ralph-cli 在编译期枚举；`ralph.yml` 引用块通过 `builtin:hang-prevention` 字面量匹配。

### 配置（configuration）

- R9. `ralph.yml` 顶层新增 `agent_doc_sync` 节点；字段：`enabled`（bool，默认 `true`）、`on_error`（`"warn"` | `"strict"`，默认 `"warn"`）、`blocks`（block 引用列表，默认 `[builtin:hang-prevention]`）。
- R10. `on_error: warn` 模式：任何 sync 步骤失败 → `log::warn!` 一行带错误细节 → 继续启动 backend（不让 hang prevention 失败阻塞用户工作）。
- R11. `on_error: strict` 模式：sync 步骤失败 → `log::error!` → 进程退出非零（建议退出码 78，对应 `EX_CONFIG`）。

### 逃生通道（escape hatches）

- R12. `ralph run` 接受 `--no-sync-agent-docs` 旗标；启用后本次 sync 步骤整体跳过，其余启动流程不变。
- R13. 环境变量 `RALPH_AGENT_DOC_SYNC=0` 与 `--no-sync-agent-docs` 等价；任一启用即跳过。`enabled: false` 走的是配置路径，与旗标/环境变量独立求值（任一为 true 即跳过）。

### 可观测（observability）

- R14. `ralph doctor` 新增一行 health check：`agent_doc_sync: synced=N skipped=M failed=K` + 上次成功 sync 时间戳（从 `<cwd>/.ralph/diagnostics/agent_doc_sync.json` 读取，无记录则显示 "never"）。
- R15. telemetry.runtime_diagnosis envelope 新增 source `agent_doc_sync`；每次 sync 结束落 `recovery.jsonl` 一行，outcome 走六档之一（`recovered` / `failed` / `not_retriable` 等）。

### 失败模式（failure modes）

- R16. 目标文件不存在：`enabled: true` → 创建文件 + 追加 section + 追加块；`enabled: false` 或被旗标/环境变量跳过 → 不创建。
- R17. 目标文件存在但不可写（权限 / 只读 fs）：走 R9 on_error 策略，不抛 panic。
- R18. 目标文件已含用户手写内容：sync 只追加到末尾 section，绝不在文件中间插入 / 替换非 ralph 维护的内容；`fs2` 锁在写前获取。

## Acceptance Examples

- AE1. 首次 ralph run 在空目录执行 → 退出后 `cwd/CLAUDE.md` 存在并以 `<!-- ralph:begin hang-prevention v=sha256:HEX -->` 开头对应 section、文件末尾；`cwd/AGENTS.md` 同形。
  - **Covers:** R1, R2, R3, R6, R7, R16.
  - **Given:** cwd 下没有 `CLAUDE.md` / `AGENTS.md`。
  - **When:** `ralph run -p "demo"` 跑完第一轮。
  - **Then:** 两个文件均被创建；`hang-prevention` 块含完整 5 条 Command Hang Prevention Rules；`v=sha256:HEX` 哈希稳定（与 builtin 内容一致）。

- AE2. 已有 hang-prevention 块且 v 哈希一致 → 跳过，无任何写入。
  - **Covers:** R3, R10.
  - **Given:** `cwd/CLAUDE.md` 已含 `<!-- ralph:begin hang-prevention v=ABC -->` 且 v 与 builtin 哈希匹配。
  - **When:** `ralph run -p "demo"` 跑完第一轮。
  - **Then:** 文件 mtime 不变；`doctor` 输出 `skipped=1`；log.info 一行 "skipped hang-prevention (up to date)"。

- AE3. builtin 块内容升级到新版本 → 原地升级块，section 标题不动；用户手写内容零改动。
  - **Covers:** R3, R4, R18.
  - **Given:** `cwd/CLAUDE.md` 含 v=OLD 的 `hang-prevention` 块；builtin 已升级到 v=NEW。
  - **When:** `ralph run -p "demo"` 跑完第一轮。
  - **Then:** 块内容被新内容替换；`v=NEW` 写入 marker；`## Ralph Managed Blocks` 标题保留；用户手写内容字节级不变。

- AE4. `RALPH_AGENT_DOC_SYNC=0` 环境下 `ralph run` 启动 → 整个 sync 步骤跳过，正常进入 backend spawn。
  - **Covers:** R12, R13.
  - **Given:** 环境变量 `RALPH_AGENT_DOC_SYNC=0`；`cwd/CLAUDE.md` 不存在。
  - **When:** `ralph run -p "demo"` 跑完第一轮。
  - **Then:** `cwd/CLAUDE.md` 仍未创建；log.debug 一行 "agent_doc_sync disabled via env"；backend 正常启动。

- AE5. `cwd/CLAUDE.md` 已存在但只读 → `on_error: warn` 默认行为：log.warn 继续启动；`on_error: strict` 行为：进程退出 78。
  - **Covers:** R9, R10, R11, R17.
  - **Given:** `cwd/CLAUDE.md` 是只读；ralph.yml `agent_doc_sync.on_error=warn`。
  - **When:** `ralph run -p "demo"` 触发 sync。
  - **Then:** log.warn 一行带 EACCES 错误细节；进程继续；`recovery.jsonl` 落一行 `outcome: failed`。
  - 切到 `on_error=strict` 同前提 → 进程退出 78。

- AE6. 并发两个 ralph run 跑同 cwd 触发升级路径 → 双方 fs2 锁串行化，无半写状态。
  - **Covers:** R5.
  - **Given:** 两个 ralph run 进程同时启动；`cwd/CLAUDE.md` 含 v=OLD 块；builtin 升级到 v=NEW。
  - **When:** 两个进程同时进入 sync。
  - **Then:** 持锁方完整写入 v=NEW；另一方持锁后检测到 v=NEW 一致 → skip；最终文件 v=NEW，无半写。

## Scope Boundaries

### Deferred for later

- 把 managed block 同步同样注入到 `ralph plan` / `ralph wave emit` / `ralph task` 等 spawn agent 的子命令（需要先确认这些命令对"启动前约束"的硬性需求，再扩面）。
- 在 `~/.claude/CLAUDE.md`、`~/.claude/AGENTS.md`（家级）也注入同一组块，覆盖用户跨项目的默认约束。
- `ralph.yml` 支持 `agent_doc_sync.blocks` 配项目级自定义 block（用户自写 markdown 内容）。
- 暴露 `ralph agent-doc-blocks sync --dry-run` CLI 逃生命令：让用户能预览将写入的 diff 而不实际写入。
- `runtime_diagnosis` 报告里加一段 `agent_doc_sync` 历史时间序列。

### Outside this product's identity

- 把"managed blocks"框架推广到其他 markdown 路径（如 `~/.cursor/rules`、`.continue/`、`.aider.conf.yml` 等）—— 这是另一类 sync 引擎，超出"agent 启动前约束"产品形状。
- 在块内支持动态变量（如 `{cwd}`、`{ralph_version}` 替换）—— 保持块内容静态，简化测试与可重复性。
- 提供 GUI 编辑器或 IDE 插件来管理这些块——纯属工具链，不在 orchestrator 责任范围。

## Dependencies / Assumptions

- 假设 `fs2` crate 已在依赖里或可加（用于 `flock` 互斥）；如果不接受新依赖，回退方案是用 `std::fs::File::try_lock`（Rust 1.89+）实现等价语义。
- 假设 `RalphConfig` 已有标准 ConfigSchema 注入新顶层节点路径（参考 `FeaturesConfig` / `EventFilterConfig` 的注入模式）。
- 假设 `crates/ralph-core/data/ralph-tools.md` 的 `include_str!` 模式可直接复用（block 文件放在同目录）。
- 假设 `runtime_diagnosis` envelope source 接受新值（命名空间约定遵循 `docs/guide/runtime-diagnosis.md`）。
- worktree 模式下 cwd 指 worktree 根而非主仓库根——sync 写入 worktree 内的 CLAUDE.md / AGENTS.md，不污染主仓库。这点需要在实现时验证（worktree 启动时 cwd 的语义）。

## Sources / Research

- `crates/ralph-cli/src/loop_runner/runner.rs:619` — `CliBackend::from_config`，sync 注入点
- `crates/ralph-cli/src/loop_runner/runner.rs:233` — U5 payload contract gate，先于 backend spawn 的现成参照位置
- `crates/ralph-core/src/preflight.rs` — 类似"启动前断言"模式可参考
- `crates/ralph-core/data/ralph-tools.md` — `include_str!` 块内容模式参考
- `docs/guide/runtime-diagnosis.md` — envelope source 命名 + outcome 字段约定
- `CLAUDE.md` "Important" 段 — 明确"不要手动编辑 `.ralph/` 下的运行时状态文件"；本特性不冲突（写入目标是用户控制下的 `CLAUDE.md` / `AGENTS.md`，不在 `.ralph/` 下）
- 2026-06-05 `ce-executor` hang 事件（`commit 19484eb`，19 个 preset 注入"禁止 kill 父 ralph"） — 历史动机参照
