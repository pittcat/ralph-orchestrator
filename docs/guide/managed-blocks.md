# Managed Agent Doc Blocks (受管代理文档块)

> **Updated:** 2026-06-09 · **Status:** MVP · **Scope:** `ralph run` only

本文档描述 `agent_doc_sync` 子系统——在 `ralph run` 启动 backend 之前，同步注入受管约束块到 `CLAUDE.md` / `AGENTS.md` 的机制。

---

## 1. 概念

**Managed Agent Doc Blocks** 是 ralph 在启动 backend agent 之前，注入到 `CLAUDE.md` / `AGENTS.md` 中的**版本化约束块**。它们确保每个 agent 启动时都能读到一组硬约束（如"禁止无限跟随命令"），不依赖 ralph 版本、项目状态或注入时机。

每个块由一对 HTML 注释标记界定：

```html
<!-- ralph:begin hang-prevention v=sha256:abc123... -->
<块内容>
<!-- ralph:end hang-prevention -->
```

- **幂等**：已存在且哈希匹配 → 跳过（零写入）
- **可升级**：哈希失配 → 原地替换块内容，更新版本号
- **可逃生**：`--no-sync-agent-docs` 或 `RALPH_AGENT_DOC_SYNC=0` 跳过整个阶段

### 当前内置块

| 块 ID | 内容 | 用途 |
|-------|------|------|
| `hang-prevention` | 5 条 Command Hang Prevention Rules | 防止 agent 执行无限跟随命令（`tail -f`、`journalctl -f` 等） |

---

## 2. 配置

在 `ralph.yml` 顶层添加 `agent_doc_sync` 节点：

```yaml
agent_doc_sync:
  enabled: true          # 默认 true；设 false 跳过整个 sync 阶段
  on_error: warn         # "warn"（默认，不阻塞）或 "strict"（失败则 exit 78）
  blocks:
    - builtin:hang-prevention   # 默认唯一块
```

### 字段说明

| 字段 | 默认值 | 含义 |
|------|--------|------|
| `enabled` | `true` | 是否启用 sync。设 `false` 等价于 `--no-sync-agent-docs` |
| `on_error` | `"warn"` | 失败策略：`warn` 记录 warning 继续启动；`strict` 记录 error 后 `exit(78)` |
| `blocks` | `["builtin:hang-prevention"]` | 要注入的块列表。格式 `builtin:<id>` 引用内置块 |

---

## 3. 逃生机制

三种独立方式跳过 sync（任一为 true 即跳过）：

| 方式 | 作用域 | 示例 |
|------|--------|------|
| CLI 旗标 | 单次 | `ralph run --no-sync-agent-docs -p "..."` |
| 环境变量 | 单次 | `RALPH_AGENT_DOC_SYNC=0 ralph run -p "..."` |
| 配置文件 | 项目级 | `ralph.yml` 中设 `agent_doc_sync.enabled: false` |

求值顺序：环境变量 → CLI 旗标 → 配置文件。三者独立，任一禁用即跳过。

---

## 4. 失败模式

| 场景 | `on_error: warn` | `on_error: strict` |
|------|------------------|---------------------|
| 目标文件不存在 | 创建文件 + 追加 section + 块 | 同左 |
| 目标文件不可写 | `tracing::warn!` + 继续启动 | `tracing::error!` + `exit(78)` |
| 文件锁竞争（3 次重试后） | `tracing::warn!` + 继续启动 | `tracing::error!` + `exit(78)` |
| 块内容哈希失配 | 原地替换 + 更新版本号 | 同左 |
| 块已最新 | 跳过（零写入） | 同左 |

所有失败路径都会写入 `recovery.jsonl`（envelope source = `agent_doc_sync`），供 `ralph diagnose` 查看。

---

## 5. 可观测性

### Doctor 健康检查

`ralph doctor` 新增一行 `agent_doc_sync` 检查：

```
agent_doc_sync: synced=2 skipped=0 failed=0 last=2026-06-09T13:45:00Z
```

- 无快照文件 → 显示 `never`
- `failed > 0` → 警告级别

### Recovery Envelope

每次 sync 写入 `recovery.jsonl` 一行 envelope：

```json
{
  "source": "agent_doc_sync",
  "outcome": "recovered",
  "message": "synced=2 skipped=0 failed=0"
}
```

可用 `ralph diagnose --session latest` 查看。

---

## 6. 端到端验证示例

### AE1: 首次运行（空目录）

```bash
cd $(mktemp -d) && git init
ralph run -p "echo hello"
# → CLAUDE.md 和 AGENTS.md 被创建
# → hang-prevention 块含 5 条规则全文
# → v=sha256:HEX 哈希稳定
```

### AE2: 已有最新块（跳过）

```bash
# CLAUDE.md 已含 hang-prevention 块且 v 匹配
ralph run -p "echo hello"
# → mtime 不变
# → doctor 输出 skipped=1
```

### AE3: 块内容升级

```bash
# CLAUDE.md 含 v=OLD 的 hang-prevention 块
# 升级 ralph 后 builtin 内容变更
ralph run -p "echo hello"
# → 块内容被新版本替换
# → v=NEW 写入 marker
# → 用户手写内容字节级不变
```

### AE4: 环境变量禁用

```bash
RALPH_AGENT_DOC_SYNC=0 ralph run -p "echo hello"
# → CLAUDE.md 不被创建
# → log 含 "agent_doc_sync disabled via env"
# → backend 正常启动
```

### AE5: 只读文件

```bash
chmod 444 CLAUDE.md
ralph run -p "echo hello"   # on_error: warn（默认）
# → log.warn + 进程继续
```

### AE6: 并发 sync

```bash
# 两个 ralph run 同时启动
# → FileLock 串行化写入
# → 最终文件 v=NEW，无半写状态
```

---

## 7. 文件写入格式

sync 写入 `CLAUDE.md` / `AGENTS.md` 时，在文件末尾追加 `## Ralph Managed Blocks` section：

```markdown
## Ralph Managed Blocks

<!-- ralph:begin hang-prevention v=sha256:abc123def456... -->
## Command Hang Prevention Rules

1. Never run infinite-follow commands directly.
   Forbidden examples:
   - tail -f
   - tail -F
   - journalctl -f
   - adb logcat
   - dmesg -w
   - watch
   - while true

2. If follow mode is necessary, always wrap it with timeout:
   - timeout 30s tail -f <file>
   - timeout 60s adb logcat
   - timeout 30s journalctl -f

3. Prefer bounded commands:
   - tail -n 200 <file>
   - grep -n "ERROR" <file> | head -100
   - journalctl -n 300 --no-pager
   - dmesg | tail -200

4. For large files, never cat the whole file.
   Use:
   - wc -l <file>
   - tail -n 200 <file>
   - head -n 100 <file>
   - grep -n "keyword" <file> | head -50

5. Every external command that may block must have timeout.
<!-- ralph:end hang-prevention -->
```

- section 标题和块标记前后各保留一个空行
- 用户手写内容在 section 之上，字节级不动
- 多个块在同一 section 内按追加顺序排列

---

## 8. 限制与未来方向

### 当前限制

- MVP 仅覆盖 `ralph run`；`ralph plan` / `ralph wave emit` / `ralph task` 尚未集成
- Windows 平台 `FileLock` 返回 `Unsupported` 错误（与现有行为一致）
- `ralph.yml` 不能禁用单个块（只能禁用整个 sync）

### 未来方向

- 扩展到 `ralph plan` / `ralph wave emit` / `ralph task`
- 支持 `~/.claude/CLAUDE.md` 家级注入
- 支持项目级自定义块（`agent_doc_sync.blocks` 用户自写 markdown）
- `ralph agent-doc-blocks sync --dry-run` CLI 逃生命令
