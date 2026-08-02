---
module: ralph-core
tags: [diagnostics, hat-lifecycle, activation-registry, observability, disabled-mode]
problem_type: observability
---

# DISABLED diagnostics 模式下 activation-registry / complete-unknown WARN 为预期观测

## Symptom

跑后诊断（`/ralph-run-diagnosis` skill 或人工 `.ralph/` 盘点）时，若诊断模式为
`DISABLED`（`.ralph/diagnostics/` 下没有 session 时间戳子目录），诊断报告里若出现
以下观测，**容易被误判为机制 bug**：

1. `.ralph/agent/activation-registry.jsonl` 文件**大小为 0 行**（不落盘）。
2. `.ralph/agent/accepted-transitions.jsonl` 每行的 `activation_id` 字段形如
   `unknown:N`（如 `unknown:1`、`unknown:2`）。
3. 主 CLI/TUI 日志（`.ralph/diagnostics/logs/ralph-*.log`）里**偶发**
   `Complete called for unknown or already-closed activation key` WARN
   （伴随 `key=primary:N:<hat_id>`, `completed_count=0`）。
4. `.ralph/wave-channels/` 是**空目录**（0 文件）。
5. `ralph inspect loop` JSON 的 `activation_registry` / `supervisor` 块**整键省略**。

报告若把以上任一条标为 P0/P1「mechanism 失效」并写修复建议，会**误归因**——
它们都是 DISABLED 模式的设计内预期表现。

## Root cause

`ralph run` 的诊断模式有四档（`FULL` / `MINIMAL` / `LOGS_ONLY` / `DISABLED`），由
`.ralph/diagnostics/<session>/` 目录的存在性与 `orchestration.jsonl` 决定
（`docs/report/<date>-*-diagnosis.md` 报告 frontmatter 的 `diagnostics_mode` 字段
直接对应）。**默认未启用时为 `DISABLED`**——workspace 根 `.ralph/diagnostics/`
下通常**只**有 `agent_doc_sync.json` 与 `logs/`，**没有** session 时间戳子目录。

DISABLED 下：

- **activation-registry 不持久化**：持久 activation 血统是 session 级能力
  （`crates/ralph-core/src/event_loop/mod.rs::activate()`，约 L1734-1750），仅
  在测试与有 session 的运行时被调用；DISABLED 下生产 loop 路径从不写
  `.ralph/agent/activation-registry.jsonl`。
- **`accepted-transitions` 的 `activation_id` 用 `unknown` 占位**：runtime
  按 `format!("{}:{u7_iteration}", event.source.map(|h| h.as_str()).unwrap_or("unknown"))`
  推导（`crates/ralph-core/src/event_loop/mod.rs` 约 L12826-12838），DISABLED
  下 activation 未持久化 → `source` 常为 `None` → 字段回落 `unknown:N`。
  **不影响**：accepted 计数与 hat 触发链完全独立于 `activation_id`。
- **`complete()` 命中 unknown key 走设计内节流分支**：`crates/ralph-core/src/hat_lifecycle.rs`
  约 L408-448 的 `complete()` 在 activation key 查不到时打 WARN（L435 文本：
  "Complete called for unknown or already-closed activation key"）但不 panic
  ——这是 **P2-G 2026-06-10 设计内节流**，避免误报导致 accepted 链路被破坏。
- **`wave-channels/` 空目录**：hat-channel 是 isolated mode per-activation
  落盘点（`crates/ralph-core/src/hat_channel.rs`），DISABLED 下不落盘。
- **`inspect loop` JSON 整键省略**：DISABLED + 无 supervisor 配置 →
  `activation_registry` / `supervisor` 块**不输出**（不是输出空对象）。

## Fix

**无需代码修复**。改两件事：

1. **诊断 skill 文档**（已通过对抗性审查回退：原 `ralph-tools-cmdref.md` 加
   「诊断模式预期观测伪影」章节**违规 CLAUDE.md L188/189**「AI skill guide
   可读性/去计划化规则」——泄漏 `hat_lifecycle.complete()` 等内部实现 +
   6 处 `.ralph/*` 内部 ledger 路径 + 一次诊断场景的过度定制。**通用注入
   skill 不应承载某次诊断的观测注解**。本 solutions 文档即为正确放置位置）。

2. **诊断报告 frontmatter 声明 `diagnostics_mode`**：每份
   `docs/report/<date>-*-diagnosis.md` 已**强制**写明模式（FULL/MINIMAL/LOGS_ONLY/DISABLED），
   报告 §0 需声明"盲区 / 根因置信度硬顶"，DISABLED 下"agent 归因 ≤50 /
   整行 ≤70"。OPAC 逐 hat 审计在 DISABLED 下仅基于 events 可见的 emit 侧
   （topic/hat/payload 与 preset publishes/deny_rules 对照），各 hat 进程内的
   `--policy-check` 调用与 tool_call 序列**不可见**。

**HOW TO 应用**：

- 诊断报告**遇到以上 5 条观测时**，逐条对照本文件确认"是否为 DISABLED 预期"；
  若是，**禁止**标 P0/P1「mechanism 失效」，应作为 P2 信息性观察（如有需要）
  或不入 §5。
- 若需要在禁用诊断模式下进行 FULL 级 OPAC 审计，跑诊断 run 前启用诊断模式：
  `RALPH_DIAGNOSTICS=1 ralph run ...` 或配置 `telemetry.runtime_diagnosis.write_artifacts: true`。
- 历史关联：2026-08-01 parallel-forge run 诊断（`docs/report/2026-08-01-parallel-forge-primary-20260801-003852-diagnosis.md`）DEV-003 = mechanism 70，置信度归因本身即为「DISABLED 模式下观测伪影被识别为非 bug」。

## Verification

- **复现 DISABLED 观测**：
  ```bash
  # 默认跑 ralph run（不启用诊断）→ .ralph/diagnostics/ 无 session 子目录
  wc -l /path/to/run/.ralph/agent/activation-registry.jsonl  # → 0
  jq -r '.activation_id' /path/to/run/.ralph/agent/accepted-transitions.jsonl | head
  # → "unknown:1", "unknown:2", ...
  grep "Complete called for unknown" /path/to/run/.ralph/diagnostics/logs/ralph-*.log
  # → 偶发 WARN
  ls /path/to/run/.ralph/wave-channels/
  # → 空
  ralph inspect loop --format json | jq 'has("activation_registry"), has("supervisor")'
  # → false, false（DISABLED 下整键省略）
  ```
- **对照非 DISABLED（MINIMAL/FULL）**：
  ```bash
  # 设 RALPH_DIAGNOSTICS=1 跑一遍
  RALPH_DIAGNOSTICS=1 ralph run ...
  ls .ralph/diagnostics/<session>/  # 有 session 时间戳子目录
  wc -l .ralph/agent/activation-registry.jsonl  # > 0
  jq -r '.activation_id' .ralph/agent/accepted-transitions.jsonl | head  # 不再 unknown:N
  ```
- **诊断报告 frontmatter 对账**（机器校验）：`docs/report/<date>-*-diagnosis.md` 的
  `diagnostics_mode:` 必须真实反映本次 run 模式；§0 必须有"盲区 / 根因置信度硬顶"
  声明。详见 `/ralph-run-diagnosis` skill `references/verification-pipeline.md` L7 门禁。

## Affected code

- `crates/ralph-core/src/hat_lifecycle.rs`（约 L408-448 — `complete()` + L435 WARN 文本）
- `crates/ralph-core/src/event_loop/mod.rs`（约 L1734-1750 — `activate()` 仅 best-effort open；约 L12826-12838 — `activation_id` 推导；约 L12684-12687 — `tracker.complete(&key, topic_str)` 调用）
- `crates/ralph-core/src/event_loop/repair_stream_sink.rs`（L34/L96 — `repair_dispatch` repair-stream，非拒收；与本 symptoms 表不直接相关但同属 DISABLED 下"看似异常实际预期"项）
- `crates/ralph-core/src/hat_channel.rs`（DISABLED 下不落盘的 hat-channel 路径）
- `crates/ralph-cli/src/inspect_loop.rs`（`activation_registry` / `supervisor` 块门控）

## Related diagnostics

- 2026-08-01 parallel-forge run DEV-003（confidence 70，mechanism，归因本身即为"识别为非 bug"）
- 2026-07-30 fail-close 诊断报告（`docs/report/2026-07-30-parallel-forge-primary-20260730-094057-diagnosis.md`）——其 recovery.jsonl 字段记 `reason_code=repair_dispatch` 是同一类 DISABLED 模式下的修复流痕迹

## Why

诊断 skill 与人类 reviewer 在面对 DISABLED 模式时，最容易把"激活注册 0 行" +
"unknown:N" + "complete-unknown WARN" 当作"机制失效"。每次误归因都会重复走
一次源码反查与 P0 置信度评分流程，浪费 30-60 分钟。本文件将"识别为预期"
的判断条件固化下来，下次诊断直接对照即可。

## How to apply

诊断 `ralph run` 的产出时：

1. 先看报告 frontmatter 的 `diagnostics_mode:`。
2. 若是 `DISABLED`，对照本文件 §Symptom 的 5 条观测，**逐条确认**是否属预期：
   - **预期** → 不入 §5 归因表，必要时在 §8 盲区声明或 §4 信息性观察里提一句；
   - **非预期**（即真异常）→ 入 §5，标 P0/P1/P2，按 confidence-rubric 走加深。
3. 启用 FULL/MINIMAL 模式的方法见 §Fix 第 2 条；不建议在 DISABLED 模式下推断
   "hat 进程内的 --policy-check 是否调用过"——agent-output.jsonl 不可见，按
   §8 盲区声明硬顶 ≤70 而非 P0。