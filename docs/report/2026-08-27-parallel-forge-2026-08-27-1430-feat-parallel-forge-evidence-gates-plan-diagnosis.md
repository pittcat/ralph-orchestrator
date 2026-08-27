---
title: parallel-forge Loop `2026-08-27-1430-feat-parallel-forge-evidence-gates-plan` 运行链路诊断报告
date: 2026-08-27
type: diagnosis
loop_id: 2026-08-27-1430-feat-parallel-forge-evidence-gates-plan
preset: builtin:parallel-forge
run_dir: ../worktree/ralph-orchestrator/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan
status: Wave 1 正在执行，未发现死锁；PTY/RPC 不回显 worker 进度造成表观卡住
diagnostics_mode: MINIMAL
bundle: present
bundle_path: .ralph/diagnostics/2026-08-27T16-52-23/diagnosis-input.json
causal_status: not_evaluable
causal_confidence: 0
causal_primary_domain: null
causal_rejected_hypotheses: []
causal_score_change: [{prev: null, current: 0, delta: null, reason: initial_scoring_not_evaluable}]
history_search: disabled
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: missing
activation_outcomes: present
evidence_gaps:
  - feedback.jsonl 为空
  - orchestration.jsonl 与 agent-output.jsonl 不存在，OPAC 置信度上限为 70
  - causal bundle 未填 boundary_coverage/artifacts，DT7 不可归因
execution_capabilities: [supervisor, wave]
---

# parallel-forge 运行链路诊断报告

> 诊断快照：2026-08-27 17:25:24 +08:00  
> 对照：`presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml`  
> 范围：仅本次 run；`history_search=disabled`。诊断过程串行完成，未启动 sub-agent。  
> 结论：这次不是卡死。Wave 1 的四个 executor 已于 17:18:32 启动，诊断时仍在各自 worktree 内修改/编译/运行 nextest；父 loop 正常等待 supervisor fan-in。

## 0. 产物盘点

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` → `.ralph/events-20260827-085223.jsonl` | 是 | 9 | 唯一可信主事件账本 |
| S | `.ralph/ledger.jsonl` | 是 | 8 | 已提交前五个 hat 的状态 |
| S | `.ralph/recovery.jsonl` | 否 | 0 | 本轮尚无 workspace 拒收；非异常 |
| S | `.ralph/loops.json` | 是 | 1 loop | PID 12433 仍存活 |
| A | `.ralph/agent/tasks.jsonl` | 是 | 11 | 4 个 initial-ready task 已交给 slots，尚未闭合 |
| A | `progress.md` / `summary.md` / `handoff.md` | 否 | 未终止 | 当前阶段预期 |
| B | diagnostics session | 是 | MINIMAL | trace/recovery 有；orchestration/agent-output 无 |
| B | `runtime-trace.jsonl` | 是 | 29 | sequence 1–29 单调，0 malformed |
| B | activation outcome | 是 | 5 | 全部 `merged` |
| B | `feedback.jsonl` | 是 | 0 | reader 判 missing |
| B | `.ralph/supervisor.db` | 是 | available | capability `supervisor` 满足 |
| B | worker channels | 是 | 4 个、诊断时 0 行 | worker 未完成前为预期 |
| C | inspection/development/execution/approval/worktree-map artifacts | 是 | 5 类 | 已走到 dispatch 阶段 |

`execution_capabilities=[supervisor,wave]`：preset/overlay 均启用 supervisor；dispatcher instructions 使用 `ralph wave verify/emit`；主事件 L6–L9 含同一个 public `wave_id`。

## 1. 结论摘要

### 1.1 健康度

- 判定：**运行中，非死锁**。表观停顿来自 wave worker 的 PTY/RPC 输出未流式显示，加上四个 Rust 构建并发争抢 CPU/磁盘。
- 已确认 P0/P1/P2：0（`--causal` 为 `not_evaluable`，没有项目允许进入 §5）。
- 诊断时 supervisor：`phase=collect`、4 个 slot 全为 `dispatched`、`done_units=0`。
- 诊断时进程：PID 112297/112308/112309/112312 四个 `claude` 均为 PID 12449 的直接子进程，分别在 U1/U3/U10/U2 worktree 工作；可见的子命令包括三个 `cargo nextest run -p ralph-cli --bin ralph -- ...`，其下 `rustc` 正在高 CPU 编译。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | 当前可见部分合规 | dispatcher 的 O/P/A/C 均有命令输出；主事件 L6–L9 与 supervisor 四 slot 对齐。MINIMAL 模式无法逐 tool-call 审计其他 hats | 70 |
| Q2 | 基座机制是否正常生效？ | 是，已观察范围正常 | supervisor available；四 slot dispatched；四 executor 子进程存在；无 recovery/policy rejection | 70 |
| Q3 | 编排是否合理、正常运行？ | 正常运行，正在 Wave 1 fan-in | `forge.worktrees.ready` 后出现 4 条 `exec.unit.ready`，worker 正在执行，尚未到 `exec.unit.done` | 70 |
| Q4 | 问题归因是什么？ | 没有已证实故障；表观停顿属于可观测性不足 + 重编译耗时 | worker PTY/RPC 无持续主终端回显；四个 nextest 同时构建独立 target | N/A（causal not_evaluable） |

### 1.3 根因一句话

主 loop 在 `.await` 四个 wave worker 完成，worker 的中间输出被 PTY/RPC 收集而未持续打印到启动终端；四个独立 worktree 又同时编译 `ralph-cli`，因此几分钟没有新主事件是正常的 collect 阶段，不是 dispatcher 卡死。

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| initial_terminal_status | 未到终态 |
| recovery_status | 无恢复 |
| final_code_state | 诊断时 U1/U2 已有未提交改动，U3/U10 尚无工作区 diff；四分支均仍在基线 commit |
| 一致性告警 | 无；不得在 worker 完成前把 0 字节 channel 当失败 |

## 2. 执行链路对比

| 步骤 | 预期 | 实际 | 状态 |
|---|---|---|---|
| Inspector | `forge.plan.inspected` | events L2；activation sequence 7 merged | ✅ |
| Planner | `forge.plan.ready` | events L3；sequence 13 merged | ✅ |
| Guardian | `forge.concurrency.approved` | events L4；sequence 19 merged | ✅ |
| Worktree | `forge.worktrees.ready` | events L5；sequence 25 merged | ✅ |
| Dispatcher | fan-out 4 × `exec.unit.ready` | events L6–L9；sequence 29 merged | ✅ |
| Executors | 4 × `exec.unit.done/failed` | 四个 backend 活跃；尚无 terminal worker event | ⏸️ 运行中 |
| Supervisor fan-in | `exec.wave.complete/failed` | phase=collect，done=0 | ⏸️ 等待 worker |
| Reviewer 及后续 | downstream chain | 上游未完成，尚未触发 | ⏸️ |

未触发 hats 均可由缺少 `exec.wave.complete/failed` 解释，没有异常跳步。

## 3. 历史问题上下文

`N/A (history disabled)`

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度 | DT7 分项来源 | 缺口 |
|---|---|---|---|---|---|
| DEV-001 | 用户看到的“卡住”实际是 supervisor collect 等待四个活跃 worker | events L6–L9；`ralph inspect loop` slot_summary；process tree snapshot | 信息 | correlation 不可评分 | bundle coverage 为空 |
| DEV-002 | worker 中间进度未出现在主 loop 日志；日志最后停在 wave detection | `.ralph/diagnostics/logs/ralph-2026-08-27T16-52-23-127-12433.log:L35` | 可观测性 | N/A | 无 agent-output/orchestration |
| DEV-003 | 四个独立 target 同时编译 `ralph-cli`，造成明显构建竞争 | PID 140838/145260/136185 等 nextest/cargo/rustc 进程快照 | 性能观察 | N/A | 非异常冻结窗口 |

### 4.1 OPAC 逐 hat 审计

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| inspector | ⚠️ | ⚠️ | ✅ | ✅ | events L2 + activation merged；无 agent-output | 65 |
| planner | ⚠️ | ⚠️ | ✅ | ✅ | events L3 + activation merged；无 agent-output | 65 |
| guardian | ⚠️ | ⚠️ | ✅ | ✅ | events L4 + activation merged；无 agent-output | 65 |
| worktree | ⚠️ | ⚠️ | ✅ | ✅ | events L5 + activation merged；无 agent-output | 65 |
| forge-dispatcher | ✅ | ✅ | ✅ | ✅ | 用户提供 verify/emit/inspect 结果 + events L6–L9 | 70 |
| executor slots | ✅ | N/A | ⏸️ | ⏸️ | 四进程与 supervisor dispatched；activation 尚未结束 | 60 |

MINIMAL 模式 OPAC 最高置信度 70；未见 tool-call 不能推导违规。

### 4.2 Activation outcomes

| sequence | hat | status | exit | watchdog | merge | channel bytes | terminal obligation | classification | confidence | evidence |
|---:|---|---|---:|---|---|---:|---|---|---:|---|
| 7 | inspector | merged | 0 | false | true | 440 | plan inspected/blocked | accepted | 70 | events L2 |
| 13 | planner | merged | 0 | false | true | 634 | plan ready/blocked | accepted | 70 | events L3 |
| 19 | guardian | merged | 0 | false | true | 402 | concurrency approved/blocked | accepted | 70 | events L4 |
| 25 | worktree | merged | 0 | false | true | 680 | worktrees ready/blocked | accepted | 70 | events L5 |
| 29 | forge-dispatcher | merged | 0 | false | true | 2862 | unit ready/prepare/development done | accepted wave fan-out | 70 | events L6–L9 |

Dispatcher outcome 的 `accepted_event_count=0` 不代表失败：四个 candidate 经 wave 通道写入 main ledger，并由 events L6–L9 与 `wave inspect ok=true` 二账本确认。

### 4.3 Causal Attribution

`N/A (causal attribution unavailable)`

`ralph diagnose --causal` 返回 `status=not_evaluable`、total=0；v2 manifest 的 `artifacts[]` 与 `boundary_coverage[]` 均为空，不能给根因域或修复建议。

## 5. 问题归因表

无。DT7 没有 `status=complete && confidence>85` 的项目。

历史关联：`N/A (history disabled)`

## 6. 修复建议

无。skill 禁止让 `not_evaluable` 疑点驱动修复。

## 7. 未核实疑点与机制矩阵

| 候选问题 | 当前置信度 | blocked_by | 已核验 |
|---|---:|---|---|
| 主终端缺少 wave worker 活跃进度，容易误判卡死 | not_evaluable | causal capture contract 未覆盖 worker progress | 主日志、进程树、supervisor snapshot |
| 独立 worktree 各自 target 导致四重编译竞争 | not_evaluable | 无性能 trace；只看到现场进程 | nextest/cargo/rustc 子进程与 cwd |

| 机制 | 状态 | 证据 |
|---|---|---|
| Origin/payload/workflow guards | 未见失败 | 无 workspace recovery；events 顺序合法 |
| Execution contract/task binding | 生效 | 4 个 payload 的 task_id/task_key/unit_id 与 initial ready set 对齐 |
| Isolated 单事件预算 | 生效 | 前四 hat 各一业务事件；dispatcher 为合法 wave fan-out |
| Recovery/resume/stall/drift/dedup | 未触发 | recovery 缺失、drift 0 行、无重复 terminal |
| Supervisor admission | 生效 | 4 slots dispatched，四个 executor backend 存活 |
| Terminal/silent-success | 未触发 | loop 仍在运行，无 completion topic |

源码对账：`crates/ralph-cli/src/loop_runner/wave/dispatcher/dispatch.rs:2193` 起按 round 取得 supervisor admission，`:2224` 将 slot 标为 dispatched，`:2280` 等待 `dispatch_wave_inner_with_release`；`:2503` 起创建 worker tasks，`:2536` 启动每个 worker future，`:2745` 等待 backend 完成。现场状态与这条正常路径一致。
