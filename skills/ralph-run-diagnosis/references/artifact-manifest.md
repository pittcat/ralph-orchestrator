# 中间产物分层清单（源码 SSOT）

> 过时概念见 [ssot-guardrails.md](ssot-guardrails.md)。preset 不同 Tier C 会变；Tier S 是路径约定，部分文件可空或仅终止后存在。
>
> 源码：`loop_context.rs`、`event_logger.rs`、`state/ledger.rs`、`state/recovery_log.rs`、`diagnostics/mod.rs`、`summary_writer.rs`、`handoff.rs`、`loop_runner/paths.rs`（hat-channel 命名）

**第一步**：[artifact-discovery.md](artifact-discovery.md) 盘点；**禁止** `events*.jsonl` 通配（混入历史 loop）。

---

## Tier S — 基座核心

| 路径 | 何时存在 | 诊断读什么 |
|------|----------|------------|
| `.ralph/current-events` | 正常 run | **唯一**可信 events 指针 |
| 指针 → `events-{id}.jsonl` | 同上 | 编排拓扑 **SSOT** |
| 配对 `events-history-{id}.jsonl` | 通常有 | 旁路/解析；**非**编排 SSOT |
| `.ralph/history.jsonl` | 常有 | loop 级溯源（≠ events-history） |
| `.ralph/ledger.jsonl` | 有 state commit 时 | iteration / 拒收提交 |
| `.ralph/recovery.jsonl` | 有拒收时非空 | workspace 级拒收 |
| `.ralph/loops.json` | 常有 | loop_id、worktree、pid |
| `.ralph/current-loop-id` | 常有 | task `loop_id` |
| `.ralph/loop.lock` | primary run | stale = 异常终止 |
| `.ralph/diagnostics/logs/ralph-*.log` | CLI/TUI 几乎总有 | scope、子进程（LOGS_ONLY 主证据） |

**最小 6 件套**：`current-events` → events、`ledger.jsonl`、`recovery.jsonl`、`loops.json`、`logs/*.log`、（有 tasks 时）`tasks.jsonl`。

---

## Tier A — 状态与终止

| 路径 | 何时存在 | 诊断读什么 |
|------|----------|------------|
| `.ralph/agent/tasks.jsonl` | `tasks.enabled: true` | open/closed、三字段、loop_id |
| `.ralph/agent/progress.md` | schema `state_projection` 启用 | Current Step vs `work.ready` |
| `.ralph/agent/summary.md` | **loop 终止后** | 迭代、## Diagnostics |
| `.ralph/agent/handoff.md` | **loop 终止后** | session 续跑（`HandoffWriter`） |

`tasks.enabled: false` → 进度只看 events + Tier C。

**step_handoff** 无独立目录：对账 `tasks.jsonl` ↔ `progress.md`。

---

## Tier B — 条件产出

| 路径 | 何时有 | 诊断读什么 |
|------|--------|------------|
| `.ralph/diagnostics/<timestamp>/` | `RALPH_DIAGNOSTICS=1` 或 `telemetry.runtime_diagnosis.write_artifacts` | §Diagnostics |
| `.ralph/diagnostics/<ses>/drift.jsonl` | MINIMAL/FULL session | drift 三指标（**非** workspace 根） |
| `.ralph/diagnostics/agent_doc_sync.json` | doc sync | doctor 快照 |
| `.ralph/diagnostics/channel-routing-fallback-*.md` | hat-channel 回退 | `hat_channel.rs` |
| `.ralph/agent/memories.md` | `memories.enabled` | 跨 loop 记忆 |
| `.ralph/agent/scratchpad.md` | 默认 scratchpad 路径 | preset 改路径时可能空 |
| `.ralph/agent/plan-baseline.sha` 或 `plan-baseline-{key}.sha` | plan attach | 基线 sha |
| `.ralph/agent/events-hat-{hat}-{loop_id}-{iter}.jsonl` | isolated hat-channel | 私有 emit |
| `.ralph/agent/.ralph-enforce-current-unit` | `enforce_current_unit: true` | R4 标记文件 |
| `.ralph/merge-queue.jsonl` | merge preset | 合并队列 |
| `.ralph/supervisor.db` | supervisor + feature | runtime only（**仅** capability +supervisor 时存在属预期；缺则按 Phase 0 推断结果分情况记为缺失 / 正常） |
| `run_dir/ralph.yml` | 用户工作区 | 配置漂移（**必读**） |

`LoopStateSnapshot` **无磁盘文件** — `ralph inspect loop` / events 回放。

---

## Tier C — Preset 业务产物

从 `preset_file` + `presets/schemas/<name>.yml` 解析路径，**禁止硬编码**（示例见 `ce-executor-serial` 的 `.agents/scratchpad/...`）。

```bash
rg -n 'specs_dir|scratchpad|\.agents/|docs/plans' "$PRESET" "$SCHEMA"
```

---

## Diagnostics 四档

```bash
RUN=<run_dir>
SES=$(ls -1t "$RUN/.ralph/diagnostics/" 2>/dev/null | grep -E '^[0-9]{4}-' | head -1)
if test -n "$SES" && test -f "$RUN/.ralph/diagnostics/$SES/orchestration.jsonl"; then echo FULL
elif test -n "$SES"; then echo MINIMAL
elif test -d "$RUN/.ralph/diagnostics/logs" && ls "$RUN/.ralph/diagnostics/logs/" 2>/dev/null | grep -q .; then echo LOGS_ONLY
else echo DISABLED; fi
```

| 模式 | L2 orchestration |
|------|------------------|
| FULL | 必须 |
| MINIMAL / LOGS_ONLY / DISABLED | 跳过（无 orchestration **是预期**） |

OPAC：[opac-audit-by-mode.md](opac-audit-by-mode.md)。三联对账：[log-reconciliation.md](log-reconciliation.md)。

---

## 对照输入（主仓）

| 路径 | 用途 |
|------|------|
| `presets/en/<name>.yml`（`builtin:<name>` 同此） | 拓扑、Tier C |
| `presets/schemas/<name>.yml` | projection、schema |
| `crates/ralph-core/tests/scenarios/` | BDD |
| `crates/ralph-core/data/ralph-tools-opac.md` | OPAC |

```bash
# builtin:ce-executor-serial → presets/en/ce-executor-serial.yml
# 可选：ralph preset show ce-executor-serial --format yaml
```

---

## 读取纪律

- 只读 `current-events` 指向的**一个** events 文件
- workspace + session 两份 `recovery.jsonl`（session 仅 MINIMAL/FULL）
- 缺文件 →「条件未满足」，勿默认基座 bug

示例布局：[examples/minimal-diagnostics-layout.md](examples/minimal-diagnostics-layout.md)
