---
title: Claude CLI --disallowedTools 无法单独防止 dimension-reviewer 用 Write 改 plan
date: 2026-07-06
last_updated: 2026-07-06
category: tooling-decisions
module: ralph-adapters
problem_type: tooling_decision
component: tooling
severity: high
applies_when:
  - "为 ce-executor-serial 的 dimension-reviewer 设计 adapter 层工具硬禁"
  - "评估是否在 spawn Claude 时合并 hat.disallowed_tools 到 --disallowedTools"
  - "P0 scope_violation（agent 改 plan frontmatter）后的防护方案选型"
tags:
  - claude-code
  - disallowed-tools
  - allowed-tools
  - dimension-reviewer
  - scope-violation
  - ralph-adapters
  - edit-vs-write
related_components:
  - ralph-core
  - ce-executor-serial
---

# Claude CLI --disallowedTools 无法单独防止 dimension-reviewer 用 Write 改 plan

## Context

2026-07-06 诊断 `primary-20260706-073823`：`dimension-reviewer` 修改 `docs/plans/*.md` frontmatter，触发 U5 `BlockLoop` 硬拒终止。讨论结论是：preset 里已有 `disallowed_tools: ["Edit"]`，但 Ralph 只在 **prompt 注入 + 事后 `git diff` audit** 两层生效，**未**在 spawn Claude 时合并 `--disallowedTools`。

为验证「启动时传 disallow 参数」是否足够，在 `ralph-e2e` 工作区用 Claude CLI headless（`--print`，与 Ralph `CliBackend::claude()` spawn 一致）做了两轮对照实验（2026-07-06）。

## Guidance

### Edit 与 Write 在 Claude Code 中是不同工具

| 工具 | 行为 | dimension-reviewer 典型用途 |
|------|------|------------------------------|
| **Edit** | 在已有文件内 search/replace | 不应使用（改源码/plan） |
| **Write** | 创建或整文件覆盖 | 写 `.agents/scratchpad/.../findings-*.json` |

禁 `Edit` **不等于** 禁改文件；agent 可用 `Write` 整文件覆盖达到同样效果。

### 实测：`--disallowedTools=Edit`（仅禁 Edit）

```bash
claude --dangerously-skip-permissions --print --disallowedTools=Edit <<'EOF'
1) Edit 改 /tmp/claude-tool-test-edit-target.txt
2) Write 写 .agents/scratchpad/claude-tool-test-findings.json
3) Write 改 /tmp/claude-tool-test-plan.md frontmatter
EOF
```

| 步骤 | 结果 |
|------|------|
| Edit | **BLOCKED**（*"Edit exists but is not enabled in this context"*） |
| Write findings | **SUCCESS** |
| Write plan | **SUCCESS**（agent 在 Edit 被拒后用 Write 全量重写绕过） |

单独要求「只用 Edit、禁止 Write 替代」时，Claude 会停住并报 BLOCKED——说明 **默认会主动用 Write 绕过 Edit 禁制**。

### 实测：`--disallowedTools=Edit,Write`

```bash
claude --dangerously-skip-permissions --print --disallowedTools=Edit,Write \
  -p '只调用 Write 写 scratchpad/findings2.json'
```

结果：**BLOCKED**，文件未创建。plan 能挡住，但 **findings 也写不了**。

### 实测：`--allowedTools` / 路径型 `disallowedTools`（**不可靠，headless 不可用**）

第二轮在 `ralph-e2e` 用 `claude --dangerously-skip-permissions --print` 系统验证路径白名单/路径 deny。测试后已 `git checkout` 恢复被改动的 `docs/plans/*.md`。

#### 结果总表（`--print` headless）

| 配置 | 写 scratchpad | 写 `docs/plans/` | 写 `README.md` | 写 `.agents/outside-scratchpad/` |
|------|---------------|------------------|------------------|----------------------------------|
| `allowedTools=Write(.agents/scratchpad/**)` + `disallowedTools=Edit` | ✅ | ✅ **漏** | ✅ **漏** | ✅ **漏** |
| `allowedTools=Write(/abs/path/.agents/scratchpad/**)` + `disallowedTools=Edit` | ✅ | — | — | — |
| `disallowedTools=Write(docs/plans/**),Edit` | ✅ | ✅ **没挡住** | — | — |
| `disallowedTools=Write` + `allowedTools=Write(scratchpad/**)` | ❌ 全禁 | — | — | — |
| `disallowedTools=Edit,Write` | ❌ | ❌ | ❌ | ❌ |
| `disallowedTools=Edit` only | ✅ Write 可用 | ✅ 可写 plan | ✅ 可写 | ✅ 可写 |

`/tmp/` 路径在部分用例显示 BLOCKED，原因是 Claude「覆盖已存在文件前必须先 Read」，**不是**路径白名单生效。

#### 具体漏网案例（`allowedTools=Write(.agents/scratchpad/**)`）

| 目标路径 | 结果 |
|----------|------|
| `.agents/scratchpad/tool-policy-test/case-rel.json` | ✅ 预期成功 |
| `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md` | ✅ **漏**（`status` 被改成 `whitelist-test`） |
| `README.md` | ✅ **漏**（内容变成 `hacked-readme`） |
| `.agents/outside-scratchpad/leak.txt` | ✅ **漏** |

#### 只有「整工具名」在 headless 下可靠

| CLI 标志 | headless 效果 |
|----------|---------------|
| `disallowedTools=Edit` | 禁 Edit；**Write 仍可用** |
| `disallowedTools=Write` | 禁所有 Write |
| `disallowedTools=Edit,Write` | 禁一切文件写入 |
| `Write(.agents/scratchpad/**)` 路径 allow | **不**形成独占白名单 |
| `Write(docs/plans/**)` 路径 deny | **不**挡住 plan 写入 |
| `disallowedTools=Write` + `allowedTools=Write(scratchpad/**)` | Write 全禁，scratchpad 也写不了 |

#### 与 Claude Code 已知限制一致

- 非交互模式（`-p` / `--print`）下，路径型 `allowedTools` 常被忽略：[anthropics/claude-code#1188](https://github.com/anthropics/claude-code/issues/1188)
- 相对路径 pattern 与 agent 实际使用的绝对 `file_path` 不匹配：[anthropics/claude-code#18200](https://github.com/anthropics/claude-code/issues/18200)
- 官方文档：`Edit(...)` deny 会连带影响 Write pairing；`allowedTools` 例外语法不能从全禁 `Write` 中豁免 scratchpad

**结论：`Write(.agents/scratchpad/**)` 不能作为 dimension-reviewer 的 adapter 层权限方案。**

### Ralph 现状 vs 建议分层

| 层 | 今天 | 建议 |
|----|------|------|
| Claude spawn | 全局 `--disallowedTools=TodoWrite,...`，**不含 hat 配置** | 合并 hat `disallowed_tools` 整工具名（`Edit`, `Bash`）；**不要**依赖路径型 allow/deny |
| Prompt | TOOL RESTRICTIONS 软约束 | 保留 |
| 事后 audit | `audit_file_modifications` + U5 BlockLoop | **保留作唯一可靠兜底**（防 Write 改 plan） |

**不能只加 `Edit` 到 CLI disallow**——对 P0（Write 改 plan）无效。

**不能简单加 `Edit,Write`**——会破坏合法 findings 写入。

**不能指望 `allowedTools=Write(.agents/scratchpad/**)`**——headless 下路径规则无效，in-repo 任意路径仍可 Write。

当前可行方向：

1. Claude adapter：spawn 时合并 **整工具名** `disallowedTools`（`Edit`, `Bash`, `MultiEdit` 等），**保留 Write** 给 findings
2. **继续依赖** `audit_file_modifications` + U5 BlockLoop 防 plan 被改（必需，不是可选优化）
3. 长期：评估 PreToolUse hook 或 stream 层按 `file_path` 拦截（CLI 路径权限修好后可再评估）
4. 非 Claude backend（codex/gemini 等 `--yolo`）无等价 CLI deny → 仅 audit

## Why This Matters

- 误以为「preset 已有 `disallowed_tools: ["Edit"]`」等于工具层已禁 → **假安全感**；audit 触发时改动已发生。
- 误以为「adapter 加 disallow 参数」一行搞定 → **Edit-only 挡不住 Write**，Edit+Write 又伤 findings。
- 误以为 `allowedTools=Write(.agents/scratchpad/**)` 能独占白名单 → **headless 下已证伪**，plan/README 仍可 Write。
- U5 `BlockLoop` 是 **终止** 不是 **恢复**；adapter 整工具名硬禁是减面攻击，**audit 才是防 Write 改 plan 的唯一可靠层**。

## When to Apply

- 设计 `ralph-adapters` 的 `apply_hat_tool_policy` 或类似能力时
- 评审 ce-executor-serial P0/P1 修复方案时
- 本地用 Claude CLI 复现 dimension-reviewer 越权行为时

## Examples

**本地快速复现（只禁 Edit，预期 plan 仍可被 Write 改）：**

```bash
cd /path/to/worktree

claude --dangerously-skip-permissions --print --output-format text \
  --disallowedTools=Edit \
  -p '只调用 Write，禁止 Edit。把 /tmp/claude-tool-test-plan.md 全文改成一行：status: p0-sim'
```

实测输出「已写入」，文件内容变为 `status: p0-sim`。

**复现路径白名单无效（预期 scratchpad 可写、plan 也会漏）：**

```bash
cd /path/to/worktree

claude --dangerously-skip-permissions --print --output-format text \
  --allowedTools='Read,Grep,Glob,Write(.agents/scratchpad/**)' \
  --disallowedTools=Edit \
  -p '只调用 Write。步骤1: Write .agents/scratchpad/tool-policy-test/probe.json 内容 {"ok":1}。步骤2: Write docs/plans/<your-plan>.md 第一行 status: leaked。每步 SUCCESS/BLOCKED。'
```

实测：步骤 1 SUCCESS，步骤 2 也会 SUCCESS（plan frontmatter 被改）。测完请 `git checkout -- docs/plans/<your-plan>.md`。

**Ralph 侧已有但未桥接到 CLI 的配置：**

```yaml
# presets/en/ce-executor-serial.yml — dimension-reviewer
disallowed_tools: ["Edit"]   # Write 故意保留给 findings
```

对应源码：`crates/ralph-adapters/src/cli_backend.rs`（Claude 全局 disallow）、`crates/ralph-core/src/event_loop/mod.rs:7764`（事后 audit）。

## Related

- 诊断报告：`docs/report/2026-07-06-ce-executor-serial-primary-20260706-073823-diagnosis.md`
- U5 硬拒计划：`docs/plans/2026-07-04-004-fix-ce-executor-serial-silent-success-p0-p1-plan.md`
- dimension-reviewer 恢复/emit 对齐：`docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md`（不同失败模式，低重叠）
- Claude Code 路径权限 headless 限制：[anthropics/claude-code#1188](https://github.com/anthropics/claude-code/issues/1188)
- Claude Code 相对/绝对路径 pattern 不匹配：[anthropics/claude-code#18200](https://github.com/anthropics/claude-code/issues/18200)
- Claude Code 权限文档：<https://code.claude.com/docs/en/permissions>
