---
title: builtin:parallel-forge Loop `2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan` 运行链路诊断报告
date: 2026-08-26
type: diagnosis
loop_id: 2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan
preset: builtin:parallel-forge
run_dir: ../agent_tools/worktree/ralph-orchestrator/2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan
status: P0：slot 未进入 supervisor Completed，dispatcher 因 `empty_worker_result` 重试；随后 loop 被人工 Abort
diagnostics_mode: MINIMAL
bundle: present
bundle_path: .ralph/diagnostics/2026-08-26T14-39-37/diagnosis-input.json
history_search: disabled
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: present
activation_outcomes: present
evidence_gaps: ["orchestration.jsonl/errors.jsonl 缺失", "feedback.jsonl 为空", "worker channel 在收尾时被删除，无法从本次产物反证其原始内容"]
---

# builtin:parallel-forge Loop `2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan` 运行链路诊断报告

## 0. 产物盘点

`execution_capabilities: [supervisor, wave]`。判定依据：运行日志记录 `execution_mode=isolated, supervisor.enabled=true`；存在 `.ralph/supervisor.db`；主 events 含 `exec.unit.ready` 的 `wave_id`；preset 声明 `ralph wave emit`。

| Tier | 路径 | 存在 | 行数 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` 指向的 `events-20260826-063937.jsonl` | 是 | 7 | 可信主 events；最后一条是 `exec.unit.done` |
| S | `.ralph/recovery.jsonl` | 是 | 4 | 记录 `plan.blocked/missing_terminal_emit` 修复流 |
| A | `.ralph/agent/tasks.jsonl` | 是 | 10 | U01 仍为 `open`，U02-U10 由其阻塞 |
| B | `.ralph/diagnostics/2026-08-26T14-39-37/` | 是 | trace 24 | MINIMAL；bundle present |
| B | `.ralph/supervisor.db` | 是 | SQLite | `w-1` 为 `collect`，expected=1、completed=0、in_flight=1 |
| B | `.ralph/flow-authority.jsonl` | 是 | 370 | 本次结尾无 wave settlement 事实 |
| C | U01 completion report | 是 | — | commit `1737b21b`，但任务尚未由 settlement 关闭 |
| C | `orchestration.jsonl` / `errors.jsonl` | 否 | — | bundle reader 已报告缺口，不据此单独判 P0 |

## 1. 结论摘要

### 1.1 健康度

- **判定：wave fan-in 阻塞，随后被人工中止。**
- P0：1；P1：1。
- 最高优先级根因置信度：P0-1 = **92/100**。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 执行与 OPAC 是否合规？ | ⚠️ | executor activation backend exit=0，但没有形成可消费的 worker event batch | 78 |
| Q2 | 基座机制是否生效？ | ⚠️ | supervisor store 存在且可 inspect，但 slot 仍为 in-flight；没有完成收据 | 88 |
| Q3 | 编排是否正常推进？ | ❌ | 没有 `exec.wave.complete`、`forge.wave.settled` 或下一波事件 | 96 |
| Q4 | 责任域是什么？ | **runtime/dispatcher fan-in 边界**，agent 输出路径是未核实贡献因素 | 主因由 `empty_worker_result` 明确指向 dispatcher 可见结果为空；channel 原始内容已丢失 | 92 |

### 1.3 根因一句话

U01 的 backend 虽然退出码为 0，主 events 也出现了 `exec.unit.done`，但 dispatcher 收到的该 worker 结果是空事件集，于是按防假绿规则将 slot 判为 `empty_worker_result`，没有写入 supervisor Completed 状态，故不会 fan-in；之后人工 Abort 终止了重试。

## 2. 执行链路

```text
forge.worktrees.ready
  → exec.unit.ready (wave w-18cf48b7ec47d814-3759815-0, U01)
  → executor activation backend exit=0
  → 主 events 出现 exec.unit.done（但 supervisor 仍 collect/in_flight）
  → dispatcher: empty_worker_result，重试 attempt 1/2
  → 人工 Abort
```

按 preset 契约，`exec.unit.done` 不负责关闭 task；只有 integrator 发出的 `forge.wave.settled` 才执行 settlement 和 `CloseTaskBatch`。因此截图中“U01 done”不等于“U01 已被 supervisor fan-in 接受”。

## 3. 历史问题上下文

`N/A (history disabled)`

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度 | 置信度 |
|---|---|---|---|---:|
| DEV-001 | dispatcher 把空 worker 结果判为失败而不进入 Completed | `crates/ralph-cli/src/loop_runner/wave/dispatcher/outcomes.rs:22-49` | P0 | 92 |
| DEV-002 | 主 events 有 `exec.unit.done`，但 supervisor slot 仍未完成 | run `.ralph/events-20260826-063937.jsonl:6-7`；`ralph wave inspect w-1` | P0 | 96 |
| DEV-003 | 没有 fan-in 后续事件，dispatcher 发生重试，最后收到人工 Abort | run diagnostics log `...14-39-37-182-3742839.log:35-40` | P1 | 99 |
| DEV-004 | 只有 `forge.wave.settled` 才关闭任务 | `presets/en/parallel-forge.yml:952-955,1088-1100` | P1 | 94 |

### 4.1 OPAC 逐 hat 审计

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| forge-dispatcher | ✅ | ⚠️ | ❌ | ❌ | activation merged，但 accepted_event_count=0；之后没有 coordination event | 90 |
| executor | ✅ | ✅ | ⚠️ | ❌ | backend_success=true、exit=0；结果未被 dispatcher 作为非空 batch 消费 | 78 |

### 4.2 Activation outcome

| sequence | hat | status | exit | merge | terminal obligation | classification |
|---:|---|---|---:|---|---|---|
| 24 | forge-dispatcher | merged | 0 | true | `exec.unit.ready`, `forge.wave.prepare`, `forge.exec.development.done` | attempted but no accepted event |

executor activation 的 trace（sequence 21-24 前的 wave worker 过程）显示 backend 成功，但本 session 没有保存 worker channel 原始文件；因此不能仅凭现有产物断言 agent 一定写错了 channel。

## 5. 问题归因

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 |
|---|---|---|---:|---|
| P0 | worker 可见结果为空，slot 没有进入 supervisor Completed，fan-in 不会触发 | mechanism/runtime 边界 | **92** | DEV-001 + DEV-002 + DEV-003 |
| P1 | operator Abort 让重试无法继续，也没有留下后续协调事件 | operator/运行终止 | **99** | DEV-003 |

### 已否决假设

- **不是** U01 commit 尚未产生：completion report 和 commit `1737b21b` 均存在；但 commit 不是 settlement。
- **不是**单纯“下一波 hat 变慢”：`forge.wave.settled` 根本未出现，integrator 没有触发条件。
- **不能确认**是 preset 文案导致 executor 没有 emit：主 events 有 `exec.unit.done`，且 worker channel 原始内容已被清理；该候选保持证据不足，不进入主根因。

## 6. 修复建议

### 6.1 短期

- 不要把 `exec.unit.done` 主 events 行当作 wave 已完成；先执行 `ralph wave inspect w-1`，确认 `completed=1` 后才判断进入 fan-in。
- 本次 run 已被 Abort 且进程不存在，不能靠等待自动恢复；应按 loop 的 resume/redrive 机制重新接管，而不是手工关闭 U01 task。

### 6.2 中期

- 为 dispatcher 增加可持久化的 worker-result 收据：至少记录 `worker_events_path`、读取事件数量、backend exit、分类结果和 supervisor store wave id；否则“主 events 有 done、slot 仍 in-flight”难以在现场直接区分。
- 在 `empty_worker_result` 时把“主 events 中存在同一 slot 的 done，但未进入 worker result batch”作为结构化诊断，明确提示 channel 路由/采集边界。

### 6.3 长期

- 对 worker channel 在删除前保存有界摘要或哈希，并将 `worker result → record_slot_result → fan-in tick` 做成连续收据；这样可区分 agent 未写 channel、环境变量路由错误和 dispatcher 读取失败。

## 7. 未核实疑点

| 候选问题 | 置信度 | 阻塞原因 | 已做加深 |
|---|---:|---|---|
| executor 子进程实际写入了错误的 `RALPH_EVENTS_FILE`，导致主 events 出现孤立 `exec.unit.done` | 55 | worker channel 在 `run_wave_worker` 收尾时已删除，缺少原始文件/完整 orchestration 日志 | 已核对 `inject_hat_execution_env` 与 dispatcher 的路径注入代码；未将其写入 §5 定论 |
