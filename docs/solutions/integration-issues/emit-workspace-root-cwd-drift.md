---
title: "ralph emit workspace_root 锚定与 isolated hat cwd 漂移硬约束"
date: 2026-07-06
type: solution
module: ralph-cli
tags: [emit, isolated, workspace_root, fail-closed, target_path, orphan]
problem_type: workspace_root_shadowing
related:
  - docs/report/2026-07-06-ce-executor-ralph-emit-pwd-sorts-diagnosis.md
  - docs/plans/2026-07-06-002-fix-emit-workspace-root-hard-constraint-plan.md
  - crates/ralph-cli/src/commands/emit.rs
  - crates/ralph-cli/src/cli/emit_path.rs
  - crates/ralph-cli/src/loop_runner/hat_channel.rs
---

# ralph emit workspace_root 锚定与 isolated hat cwd 漂移硬约束

## 现象

`primary-20260706-122745` 期间，validator hat 在 `sorts/` 子目录
`ralph emit test.passed`，CLI 打印 `Event emitted` 但主 events 文件
不变——事件实际落到 `sorts/.ralph/events.jsonl` 孤儿。诊断确认是
**双重机制叠加**：

1. `commands/emit.rs` line 561-563 二次 `let workspace_root = current_dir()`
   遮蔽 line 397 的 `resolve_workspace_root`。
2. agent `unset RALPH_EVENTS_FILE` 后 implicit default 解析到 cwd 子树。
3. P6 allowlist 拒绝写在 stderr，被前端 tail 截断 → agent 误判"假成功"。

完整因果链见诊断报告
`docs/report/2026-07-06-ce-executor-ralph-emit-pwd-sorts-diagnosis.md`。

## 修复机制（plan 2026-07-06-002）

| 机制层 | 文件 / 行 | 关键变化 |
|---|---|---|
| **U1** R1 单一 `workspace_root` 锚定 | `commands/emit.rs:557` | 删除 line 561-563 二次 let；policy / scope / step-handoff / write-path 全部复用 line 397 |
| **U2** R2/R4 fail-closed 路由 | `cli/emit_path.rs:66-` `classify_orphan_path` 在 line 232-280 | isolated + hat-marker 存在时拒绝 subtree `*/.ralph/events*.jsonl`(错误码 `orphan_events_path`);isolated + hat-marker 默认走 channel |
| **U3** R3 cwd 漂移硬约束 | `commands/emit.rs::bail_cwd_workspace_drift` | `isolated && hat.is_some() && env_events_file.is_none() && is_default && canonicalize(cwd) != canonicalize(workspace_root)` → 拒绝，错误码 `cwd_workspace_drift` |
| **U4** R5 `target_path` 披露 | `emit_result/mod.rs:64-` `EmitResult.target_path: Option<String>` | apply 成功 → `recorded: true` 时填充绝对路径;text 模式附加 `→ <path>` 行 |
| **U5** R6 stdout 摘要 | `commands/emit.rs::print_emit_reject_summary` 与 `format_emit_reject_summary` | 拒绝前 stdout 一行 machine-readable prefix + 短描述：`emit rejected [cwd_workspace_drift]: current_dir=... workspace_root=...` 或 json envelope |
| **U6** R7 孤儿扫描诊断 | `loop_runner/hat_channel.rs::scan_orphan_subtree_events` | merge 末尾扫描 `**/.ralph/events*.jsonl`，排除主树与 hat-channel；命中后写 `.ralph/diagnostics/orphan-emit-{ts}.md` |
| **U7** R8 文档同步 | `crates/ralph-core/data/ralph-tools-emit.md` | 新增 `RALPH_WORKSPACE_ROOT` 锚定说明、fail-closed 错误码、`EmitResult.target_path` 字段、反模式（禁子目录 unset emit） |

## 调用契约（agent 视角）

1. 正常 runner 注入路径：`RALPH_WORKSPACE_ROOT`、`PWD`、`RALPH_EVENTS_FILE` 都已注入，emit 透明走 channel，无感。
2. **失败快速检查**（"事件落到了哪里？"）：
   ```bash
   cat .ralph/current-events            # 主 events
   cat .ralph/current-hat-events        # hat-channel
   find . -name 'events*.jsonl' -not -path './.ralph/*'  # 找 subtree 孤儿
   ```
3. 触发 `cwd_workspace_drift`：恢复 runner 注入的 `RALPH_EVENTS_FILE` 或 `cd $RALPH_WORKSPACE_ROOT` 后重试。
4. 触发 `orphan_events_path`：检查 hat-marker 是否被篡改、或子目录是否被显式写入。
5. 收到 `target_path` 字段后，`--output json` 消费者应校验 `recorded: true && target_path` 非空。

## 反模式

- 🔴 禁止 `unset RALPH_EVENTS_FILE` + `cd sorts/` + `ralph emit ...`。runner 的 env 是 emit 路由的 SSOT，让 hat 进程保留它。
- 🔴 不要直接 `echo` 写入 `.ralph/events.jsonl`（绕过 CLI pre-publish check）。
- 🔴 不要伪造 `current-hat-events` marker 指向 subtree 路径（orphan guard 仍会拒）。

## 已知遗留 / 不在本 plan 范围内

- preset `instructions:` 软约束（hat prompt 应继续建议"子目录 cd 后不要 unset RALPH_EVENTS_FILE"）。
- 前端 stderr tail 截断行为未改，但 stdout 摘要规避了 stderr 截断风险。
- `ralph events --events-source` 默认值未改（diagnose P3）。
