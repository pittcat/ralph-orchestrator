---
loop_id: primary-20260728-003922
preset: builtin:parallel-forge
plan: docs/plans/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md
workspace: /Users/pittcat/Dev/Rust/ralph-e2e
run_dir: /Users/pittcat/Dev/Rust/ralph-e2e/.ralph
diagnostics_mode: LOGS_ONLY
execution_capabilities: [supervisor, wave]
history_search: disabled
date: 2026-07-28
type: ralph-run-diagnosis
---

# Ralph Run Diagnosis — parallel-forge / primary-20260728-003922

> **范围**:只看 `.ralph/` 产物 + preset + 当前 loop;**不**做历史对照(用户要求"历史产物不用看")。
> **状态**:loop 进程 (PID 29513) 仍存活,但已**停摆** —— 6 次 iteration 跑完 4 个规划 hat,从未进入 `exec_wave` step 的 fan-out 阶段。
> **核心结论**:**机制层** `forge-dispatcher` hat 自 `forge.worktrees.ready` 起**未被 spawn**(hat-channel 0 字节、events `triggered=forge-dispatcher` 但 hat 进程从未启动),导致 exec wave 永远未发出任何 `exec.unit.ready`;**编排层** worktree_setup step 之后理应 advance 到 `exec_wave`(kind=side_effect, runs=`supervisor.exec.wave`),但 advance 发生在 worktree_hat 第二次重复 emit 后被单事件预算丢弃,`forge-dispatcher` 这一激活从未被调度。

---

## 0. 产物盘点与能力推断

### 0.1 Tier 清单

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | `.ralph/current-events` → `events-20260728-003922.jsonl` | ✓ | 6 | 编排 SSOT |
| S | events-history-20260728-003922.jsonl | ✓ | 1 | warmup only |
| S | `.ralph/ledger.jsonl` | ✓ | 4 | iter 1..4 counter_changed |
| S | `.ralph/loops.json` | ✓ | — | 单 loop;`workspace=/Users/pittcat/Dev/Rust/ralph-e2e`,`prompt=docs/plans/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md`,worktree=空(主进程就地跑) |
| S | `.ralph/history.jsonl` | ✓ | 1 | `loop_started` |
| S | `.ralph/loop.lock` | ✓ | — | pid=29513;**lock 仍持有**,loop 未正常终止 |
| S | `.ralph/recovery.jsonl` | ✗ | — | 无拒收 |
| A | `.ralph/agent/tasks.jsonl` | ✗ | — | `.tasks.jsonl.lock` 存在但 tasks.jsonl 缺失 → **plan 阶段未注册 tasks** (planner/forge-dispatcher 均未跑 `ralph tools task add`) |
| A | `.ralph/agent/progress.md` | ✗ | — | 未生成 |
| A | `.ralph/agent/summary.md` | ✗ | — | 未终止 |
| A | `.ralph/agent/handoff.md` | ✗ | — | 未终止 |
| B | diagnostics (orchestration.jsonl) | ✗ | — | **mode = LOGS_ONLY**(详见 §0.2) |
| B | `.ralph/diagnostics/agent_doc_sync.json` | ✓ | — | doc sync complete |
| B | `.ralph/diagnostics/logs/ralph-*.log` | ✓ | 2 | parent 982B (RPC launcher) + child 6.3KB (supervisor) |
| B | `.ralph/supervisor.db` (+shm/wal) | ✓ | — | sqlite db 初始化成功 |
| B | `.ralph/agent/.ralph-enforce-current-unit` | ✓ | 2B | R4 单 U 标记 |
| B | `.ralph/agent/events-hat-worktree-primary-20260728-003922-5.jsonl` | ✓ | **0 B** | **关键异常**:hat-channel 指向 worktree 的 iter-5 槽位,但 dispatcher hat 才是当前应激活的 hat;文件 0 字节说明 **dispatcher hat 从未写出任何事件** |
| B | `run_dir/ralph.yml` | ✗ | — | 无配置,走 default |
| C | `.ralph/forge/2026-07-22-001-.../inspection-report.md` | ✓ | — | inspector 产物,verdict=usable |
| C | `.ralph/forge/2026-07-22-001-.../development-plan.md` | ✓ | — | planner 产物 |
| C | `.ralph/forge/2026-07-22-001-.../execution-plan.yml` | ✓ | — | planner 产物 (5 unit, U1..U5) |
| C | `.ralph/forge/2026-07-22-001-.../concurrency-approval.md` | ✓ | — | guardian 产物,APPROVED |
| C | `.ralph/forge/2026-07-22-001-.../worktree-map.yml` | ✓ | — | worktree 产物,base_commit=cee274b,5 worktree slots (u1..u5) |
| C | `.ralph/forge/2026-07-22-001-.../templates/` | ✓ | — | materialize 已执行(6 文件) |
| C | `.ralph/forge/2026-07-22-001-.../blocks/` | ✗ | — | 无 blocks (verdict 顺利) |
| C | `.ralph/worktrees/2026-07-22-001-.../{u1,u2,u3,u4,u5}/` | ✓ | — | 5 个 worktree 实际建好,各自含 `.git` / `PROMPT.md` / `ralph.pipeline.yml` / `ralph.supervisor.yml` / `PROMPT.ce-executor-supervisor.md` |

### 0.2 Diagnostics 模式

```text
FULL     ? — 不存在 orchestration.jsonl → 跳过
MINIMAL  ? — 不存在 session 目录
LOGS_ONLY ✓ — 仅 .ralph/diagnostics/logs/ralph-*.log 有内容
DISABLED ? — 否则
```

**OPAC 降级声明(diagnostics=LOGS_ONLY)**:L2 orchestration 不存在;**OPAC 置信度按 `opac-audit-by-mode.md` 的 LOGS_ONLY 条款压低**(单项 ≤ 50);agent 触发链路 L4 仅靠 `current-hat-events` 指针 + log 反推。

### 0.3 execution_capabilities 推断

按 `agent-native-model.md`「执行模型」段,信号检测顺序:

1. **preset 解析**: `event_loop.supervisor.enabled: true` → **+supervisor**
2. **hat `instructions` / `## WAVE CONTEXT`**: `forge-dispatcher` 的 instructions 含 "ralph wave emit exec.unit.ready" / "ralph wave verify" / "WAVE OPAC" → **+wave**
3. **Intent / 产物信号**: `.ralph/supervisor.db` 存在 → 确认 +supervisor;events 无 `wave_id`(尚未发出 exec.unit.ready,自然无 wave_id),但 dispatcher 必发 → capability 已声明 +wave 即可
4. **Observe**: 没有 inspect JSON,无法 `has("supervisor")` 检查;但 db 在盘 + `loop_runner: supervisor bridge wired (execution_mode=isolated, supervisor.enabled=true)`(child log 第 6 行)→ +supervisor 锁定

**`execution_capabilities: ["supervisor", "wave"]`**。

### 0.4 当前 hat / 调度指针

| 来源 | 字段 | 值 |
|---|---|---|
| `.ralph/current-hat-events` | hat-channel 文件 | `agent/events-hat-worktree-primary-20260728-003922-5.jsonl`(0 字节) |
| `ralph inspect loop` (无 --hat) | current_hat | (unset) |
| `ralph inspect loop --hat forge-dispatcher` | current_hat | forge-dispatcher (preview only) |
| `ralph inspect loop` | events_file | `.ralph/events.jsonl` (0 字节) |
| `ralph inspect loop` | warnings | `hat-channel file exists but is 0 bytes`<br>`loop_anchor not attached` |

> 同一 run 目录下 `inspect` 报告 `events.jsonl (0 bytes)`,但实际 `.ralph/events-20260728-003922.jsonl` 是 6 行有内容 —— 说明 `inspect` 默认读 `current-events` 之外的 fallback 路径(.ralph/events.jsonl,不存在),而**真正权威 events 是 current-events 指向的 events-20260728-003922.jsonl**(已 6 行)。

---

## 1. 强制四问

### Q1. 执行与 OPAC(diagnostics = LOGS_ONLY,OPAC 置信度 ≤ 50)

| 维度 | 观察 | OPAC 置信 |
|---|---|---|
| Observe(loop 状态/任务清单) | `ralph inspect loop` 报 `loop_anchor not attached`,`current_hat=(unset)`,`hat_channel 0 bytes`;`tasks.jsonl` 不存在 → planner 未注册 tasks | 35(LOGS_ONLY 压低) |
| Precheck(`--policy-check` / `wave verify`) | events 中无 `policy-check_failed` / `verify.failed` 事件,无拒收 | N/A(untriggered) |
| Apply(emit) | events 含 6 条业务事件,均成功落盘;**`forge-dispatcher` 应发的 `exec.unit.ready` 完全缺失** | 40 |
| Confirm(events / wave inspect) | events 显示 fan-out 未发出;`supervisor.db` 已初始化但无 wave 记录 | 30 |
| 单事件预算 | child log 第 26 行 `Isolated mode: extra business event dropped — only one per turn topic=forge.worktrees.ready` | **命中 hard rule**(HARD RULE 1 单事件预算) |
| 终态事件 | 无 `LOOP_COMPLETE` / `forge.report.done` / `work.failed`;loop 在 mid-pipeline 停摆 | 客观 |

**OPAC 总体**:**未达到全链路 observe-precheck-apply-confirm** —— Apply 阶段(forge-dispatcher)从未被调度。LOGS_ONLY 下单项 ≤ 50,综合 OPAC 置信度 **40 / 100**。

### Q2. 基座机制是否生效

| 机制 | 是否生效 | 证据 |
|---|---|---|
| `current-events` 指针 | ✓ | 文件存在,events 6 行写入 |
| ledger iteration 计数 | ✓ | 4 次 `loop.batch_sync` counter_changed |
| `apply_contract_committed_side_effects` | ✓ | events 历史中无 contract_rejection / scope_violation 提示 |
| `HandoffIndex.consumer_of(forge.worktrees.ready)` | ✓ | events `triggered=forge-dispatcher`(派生路径正确) |
| isolated single-event budget | ✓(过头)| **正确识别并丢弃多余 emit**,但本身被滥用 → 见 P0 |
| `event_policy.enforce` + `require_emit_provenance` | ✓ | 6 条事件全部带 source/hat |
| hat channel 写入 | ✗ | dispatcher 的 hat-channel 是 **worktree** 的 iter-5 槽位,文件 0 字节 |
| supervisor fan-out | ✗ | `exec.unit.ready` 0 条;`exec.wave.complete` 0 条 |
| worktree physical creation | ✓ | u1..u5 5 个 worktree 实际建好 |
| recovery / resume | N/A | 无拒收,无 recovery.jsonl |

**机制 vs 编排归因**:基座机制层(指针 / ledger / 契约 / 单事件预算)**全部生效**;**supervisor fan-out 这一机制未启动**,因为前置 hat(`forge-dispatcher`)未被调度。

### Q3. 编排是否合理

flow 声明(`parallel-forge.yml` line 54-138)14 步:

```
planning → plan_authoring → concurrency_review → worktree_setup →
exec_wave(side_effect, runs=supervisor.exec.wave, on=forge.worktrees.ready) →
exec_finalize / exec_failure → unit_review → integration →
incremental_verify → full_verify → audit → report → plan_end
```

观察到的实际推进:
- 5 个 step(planning → worktree_setup)全部走完
- 6 个 step(exec_wave 之后)**全部未走**

**worktree_setup step 完成条件**:`on=forge.concurrency.approved`,`allowed_emits=[forge.worktrees.ready, forge.plan.blocked]`。events 显示 worktree hat 在 00:47:01 emit `forge.worktrees.ready`(triggered=forge-dispatcher)。这一步的 step close 应当 advance 到 `exec_wave`。

**worktree hat 第二次 emit 行为异常**:
- events 第 6 条 `forge.worktrees.ready` 在 00:47:05 又出现一次(同 hat、同 payload);
- child log 第 26 行 00:47:31 报 `extra business event dropped`;
- 但 events.jsonl **同时记了两条**(说明第一次没被丢弃、第二次在 isolate 模式的下游 enforce 被 drop);
- 该 enforce 在 `event_loop/mod.rs` 大约 10044 行 `isolated_hat trigger dispatch` 段(`per_turn_budget_feedback_injected` + `task.resume`)。

**结论**:**worktree hat 错误地发出了 2 次 `forge.worktrees.ready`**(同一 isolated activation 内违反单事件预算 hard rule);第一次成功推进 step;第二次被 runtime 拦截 + 发 `task.resume` 恢复,但 task.resume 又没有 spawn forge-dispatcher(否则 hat-channel 应有内容)。

### Q4. 归因(preset / mechanism / agent / compound)

按 `confidence-rubric.md` 评分;`file:line` + 双账本一致才能 ≥85,LOGS_ONLY 下 OPAC/agent 单项 ≤ 50。

| 候选根因 | 类别 | 证据 | 置信度 |
|---|---|---|---|
| (A) `worktree` hat 在 isolated activation 内发了 2 次 `forge.worktrees.ready`,触发单事件预算 hard rule → runtime 注入 `task.resume` 让 worktree 自己 retry → loop 在 `task.resume → forge-dispatcher` 链路上卡死 | preset + agent | preset worktree instructions "Single business event only. Do not require any repo file outside trigger paths / forge artifacts.";events 同 hat 同 topic 两条;child log 第 26 行 `extra dropped`;hat-channel 0 字节 | **70** |
| (B) `forge-dispatcher` hat 从未被 spawn,导致 `exec_wave` step 无 wave fan-out | mechanism | events `triggered=forge-dispatcher` 但 hat_channel 指向 worktree 且 0 字节;events 中无 `exec.unit.ready`;`ralph inspect loop` 报 `current_hat=(unset)`;`supervisor.db` 在盘但无 wave 记录 | 60 |
| (C) `planner` hat 未调用 `ralph tools task add` 注册 5 个 unit tasks → forge-dispatcher 启动时无 task list → ready set 必空 → 不会 emit `exec.unit.ready` | agent(preset 漂移) | `.ralph/agent/tasks.jsonl` 缺失(`tasks.jsonl.lock` 存在但文件不在);planner instructions step 4 "对每个 unit:`ralph tools task add`";events 第 3 条 `unit_count: 5` 但无 task 落地 | **75** |
| (D) `loop_anchor not attached`(inspect 警告)使 hat 启动时 plan 关联字段为 null → OPAC Observe 阶段就 fail | mechanism 边界 | `ralph inspect loop` warnings;forge/ artifacts 路径是相对路径,可能源于 run_dir 缺失 plan 关联 | 50 |

**主因(综合)**:**A + C 复合** —— worktree hat 在 isolated activation 内多次 emit(preset 漂移/agent 错) + planner 未注册 tasks(agent 没遵守 instructions)。

**OPAC 置信度**:LOGS_ONLY ≤ 50。综合 OPAC **40 / 100**。

**根因置信度(P0 须 ≥ 70,入表门槛 ≥ 60)**:
- 主因 C(未注册 tasks):**75**(events 证据 + ledger 缺失 + planner instructions 引用 step 4);
- 主因 A(worktree 双 emit):**70**(log WARN + events 双发);
- 次因 B / D:60 / 50,入表门槛但贡献比例小。

---

## 2. 关键时间线

| 时刻(UTC+0) | 事件 | 来源 |
|---|---|---|
| 00:39:22 | `forge.start` emit(loop-bootstrap) | events[1] |
| 00:39:22 | R4 marker / supervisor bridge wired / agent_doc_sync / memory inject 0 / PtyExecutor spawn inspector pid=29558 | child log lines 6-15 |
| 00:40:25 | inspector emit `forge.plan.inspected`(plan_usable=true) | events[2] |
| 00:40:38 | spawn planner pid=33632 | child log line 18 |
| 00:43:25 | planner emit `forge.plan.ready`(unit_count=5) | events[3] |
| 00:43:36 | spawn guardian pid=41180 | child log line 22 |
| 00:45:09 | guardian emit `forge.concurrency.approved`(APPROVED) | events[4] |
| 00:45:16 | spawn worktree pid=45579 | child log line 26 |
| 00:47:01 | worktree emit `forge.worktrees.ready`(1st,triggered=forge-dispatcher) | events[5] |
| 00:47:05 | worktree emit `forge.worktrees.ready`(**2nd**,同 hat 同 topic) | events[6] |
| 00:47:31 | log `extra business event dropped — only one per turn topic=forge.worktrees.ready` | child log line 30 |
| 00:47:31 | spawn claude pid=52868(worktree hat **第三轮 activation**,PtyExecutor) | child log line 34 |
| 00:47:31+ | (停滞) —— claude 子进程在跑(0:11.42 CPU),worktree hat 试图"补救"再次进入 isolation | ps -ef |
| 00:50:54 | `ralph inspect loop` 报 `current_hat=(unset)`,hat-channel 0 bytes | 手动调用 |
| 00:51:01 | `ralph inspect loop --hat forge-dispatcher` 报 hat identity OK,publishes=[exec.unit.ready, forge.exec.development.done] | 手动调用 |

---

## 3. 异常点(LLM 视角能直接看到什么 / 不能看到什么)

### 3.1 dispatcher hat 看不到 plan_anchor

`ralph inspect loop` 警告 `loop_anchor not attached`,意味着 `ralph inspect loop` / `ralph tools task list` 在 isolated 启动时可能给出 null path → forge-dispatcher 的 step "1. Read `execution_plan_path`, `worktree_map_path`, `plan_key` from trigger/projection" 若 `projection` 为空,必须从 trigger payload 读;events 第 5/6 条 payload 包含 `execution_plan_path` / `worktree_map_path` / `plan_key`,**理论上足够** —— 但若 dispatch hat **根本未启动**,这条 observation 无意义。

### 3.2 preset worktree instructions 允许"代填"额外 emit

worktree hat step 5 写 "Precheck then emit `forge.worktrees.ready`" 一次,step 6 "On failure: ... emit `forge.plan.blocked`"。**instructions 没禁止双发**,且 `exempt_topics: [forge.worktrees.ready, forge.plan.blocked]`(line 405-406)说明该 hat 可豁免 —— 但 isolated 模式的单事件预算是 hat-level,不是 topic-level。**worktree hat agent 在没有显式约束的情况下发了 2 次**,可能源于:
- 5 个 worktree 创建后,agent 想"保险地"再 emit 一次确认;
- 看到 `audit_concurrency.py` 已通过但还是决定"再 emit 一次"。

`trigger_multi_consumer_topics` 仅在 `forge-dispatcher` 上声明,且只针对 `exec.wave.complete` —— 与 worktree 双发无关。

---

## 4. 推断的下一状态(不预测超时,只推断确定性)

- **当前**:`forge-dispatcher` 未 spawn → exec_wave 永远不开始 → loop 在 worktree_setup step close 后,等待一个永远不会到来的 hat activation。
- **lock 持有者**:pid=29513 仍在(主进程);child pid=52868(claude 子进程)在跑(可能是 worktree hat retry isolation,试图"补救"完成 worktree_setup)。**claude 退出后 worktree hat 会 emit 第二个 worktree-done,导致新一轮 task.resume,循环往复**。
- **强制终止**:用户需要 `kill 29513`(及子进程 52868) + `rm .ralph/loop.lock` + 清理 `tasks.jsonl.lock` 后再重跑。

---

## 5. 主要发现(P0/P1,带置信度)

### P0-1:**planner hat 未注册 unit tasks,forge-dispatcher 启动时 ready set 必空**

- **置信度**:75(file:line + events + ledger 双账本一致)
- **证据**:`.ralph/agent/tasks.jsonl` 缺失(只有 lock);planner instructions step 4 "对每个 unit: `ralph tools task add`";events `unit_count=5` 但无 task_id;forge-dispatcher 触发条件 "A unit is ready when its task is `open`" 无 task = ready set 永远 ∅ → 只能 emit `forge.exec.development.done` —— 但 events 中也没这条。
- **根因分类**:agent(preset 漂移:planner instructions 写明了 task add,但 agent 没执行);机制(forge-dispatcher 的 ready set 强依赖 task API)。
- **修复方向**(写入下次 plan):
  - preset 在 `worktree_setup` step 后强制增加 `tasks_exist_for_units` gate(类似 `inspector.report_path.exists`);planner 不发 `forge.plan.ready` 之前需 `ralph tools task list --key forge:<plan-key>:<unit-id>` 复核;
  - 或 forge-dispatcher 在 ready set 为空时改 emit `forge.plan.blocked(reason=no_tasks_registered)` 而非 `forge.exec.development.done`(后者是"全完成"语义)。

### P0-2:**worktree hat 在 isolated activation 内发了 2 次 `forge.worktrees.ready`,破坏单事件预算**

- **置信度**:70(双账本:events 同 hat 同 topic 两次 + child log WARN `extra dropped`)
- **证据**:events[5]/events[6] 同 hat=`worktree` 同 topic=`forge.worktrees.ready` 同 payload,差 4 秒;child log line 30 `Isolated mode: extra business event dropped`。
- **根因分类**:preset(没明示"exactly once");agent(worktree hat agent 内部 retry / 保险 emit)。
- **修复方向**:
  - preset worktree instructions 强化: "**Emit `forge.worktrees.ready` exactly once. If you emitted it in a prior turn, do not re-emit.** Confirm by `ralph events --topic forge.worktrees.ready --since <hat-start-ts>` and see only your own row.";
  - mechanism:在 `forge.worktrees.ready` 的 publisher 白名单里,worktree hat 第一次成功 emit 后记 in-memory lock,第二次 emit 在 source=`worktree` 时直接 drop(类似 policy `duplicate_publisher_within_hat`)。

### P1-1:forge-dispatcher hat 触发链路 `task.resume` 后未 spawn

- **置信度**:60
- **证据**:events[6] `triggered=forge-dispatcher` 但 hat-channel 0 字节;`ralph inspect loop` `current_hat=(unset)`。
- **根因分类**:mechanism(`task.resume` 投递到 dispatcher 没起作用,可能是 trigger filter 不匹配 / plan_anchor 缺失)。
- **修复方向**:研究 `task.resume` 在 `forge.worktrees.ready` 后的实际 routing;若 dispatcher hat 没收到 task.resume,增加 inspect 警告 "task.resume 没有匹配 hat"。

### P1-2:`loop_anchor not attached` 警告

- **置信度**:50(LOGS_ONLY,无 inspect JSON 完整内容)
- **证据**:`ralph inspect loop` warnings;`ralph.yml` 不存在(走 defaults)。
- **根因分类**:配置(`--plan` 路径应是 `docs/plans/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md`,inspect 应能 attach)。
- **修复方向**:在 ralph run CLI 端确保 loop_anchor attach;或允许 workspace 内的 plan 路径自动 attach。

### P2-1:OPAC Observe 在 isolated dispatch hat 中可能拿不到 task list

- **置信度**:40
- **证据**:dispatcher step 1 "Read `execution_plan_path` ... `ralph tools task list`";tasks.jsonl 缺失 → 即使 dispatcher 启动,`ralph tools task list` 会返回空。
- **根因分类**:agent(假设前提失败)。
- **修复方向**:dispatcher OPAC step 1 增加 "若 `ralph tools task list --key forge:<plan-key>` 返回 0 行,emit `forge.plan.blocked(reason=tasks_not_registered)`"。

---

## 6. 修复建议(写入下次 plan)

> **不**是本次 run 的修改动作;是诊断产出的待办。

1. **planner 必须发 `forge.plan.ready` 前先核对 `ralph tools task list`**:
   - 增加 lint:`preset_lint::workflow_activation.rs` 加 `planner_must_register_tasks_before_plan_ready`,扫描 planner instructions 包含 "ralph tools task add" 且 `forge.plan.ready` 在 publishes;同时建议 BDD scenario 加 `run_workflow_guard_scenario` 测试 `planner_no_tasks → forge.plan.blocked`。
2. **worktree hat 单事件预算 hardening**:
   - preset instructions 加 "emit exactly once";mechanism 加 publisher dedup(参考 `work_done_dedup_key`)—— 在 state 加 `published_topics_per_hat_iter` set,已在 iteration 内发过同 topic 则拒。
3. **forge-dispatcher fallback path**:
   - ready set 为空时不要发 `forge.exec.development.done`(易误报"已完成");发 `forge.plan.blocked(reason=no_ready_units)`。
4. **OPAC 边界**:task API 在 plan 阶段任一 hat 都该能查;在 isolated dispatch hat 不依赖 plan_anchor。

---

## 7. 未核实疑点(confidence < 60,留待)

- **D1**(confidence 50):`loop_anchor not attached` 是否真正影响 dispatch hat 启动,需在主仓用 `ralph run --plan <path>` 复跑验证。
- **D2**(confidence 40):OPAC 在 isolated dispatch hat 中 `ralph tools task list` 是否真的返回空(因 tasks.jsonl 不在,API 可能 fallback 到别处);需在 ralph-cli 集成测试中验证。
- **D3**(confidence 35):child pid=52868 当前在跑 claude 子进程;如果它是 worktree hat 第三轮 isolation,**它当前正在干什么**(无 stdout/stderr 可读);需 kill 进程后读其输出。

---

## 附录 A:关键源码引用

- `crates/ralph-core/src/event_loop/mod.rs:11348` — `apply_contract_committed_side_effects` 调用点。
- `crates/ralph-core/src/event_loop/mod.rs:1811` — `apply_contract_committed_side_effects` 函数定义(单事件预算检查的关键路径)。
- `crates/ralph-core/src/event_loop/mod.rs:10044` — `triggered: Some(isolated_hat.as_str().to_string())` 写入。
- `crates/ralph-core/src/workflow_contract/handoff_index.rs:228` — `consumer_of` 查找(确认 dispatcher 是 forge.worktrees.ready 的唯一 consumer)。
- `crates/ralph-core/src/workflow_contract/handoff_index.rs:125` — `HandoffIndex::from_config` 构建。
- `presets/en/parallel-forge.yml:54-138` — flow 声明 14 步。
- `presets/en/parallel-forge.yml:411` — `forge-dispatcher` hat 定义(triggers=[forge.worktrees.ready, exec.wave.complete])。
- `presets/en/parallel-forge.yml:401` — `worktree` hat 定义(publishes=[forge.worktrees.ready, forge.plan.blocked])。

## 附录 B:产物文件总览(`tree -L 3 .ralph`)

```text
.ralph/
├── agent/
│   ├── .ralph-enforce-current-unit
│   ├── events-hat-worktree-primary-20260728-003922-5.jsonl (0B)  ← 异常
│   ├── plan-baseline-prompt-805c121f8cd07565.sha
│   └── tasks.jsonl.lock  ← 异常:tasks.jsonl 缺失
├── current-events                       → events-20260728-003922.jsonl
├── current-hat-events                   → events-hat-worktree-primary-20260728-003922-5.jsonl
├── current-loop-id                      → primary-20260728-003922
├── diagnostics/
│   ├── agent_doc_sync.json
│   └── logs/
│       ├── ralph-2026-07-28T08-39-22-228-29512.log  (parent, 982B)
│       └── ralph-2026-07-28T08-39-22-230-29512.log  (child,  6.3KB)  ← 关键证据
├── events-20260728-003922.jsonl                          (6 行,SSOT)
├── events-history-20260728-003922.jsonl                  (1 行,warmup)
├── flow-authority.jsonl                                  (4 行,plan_authoring → worktree_setup 全过)
├── forge/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan/
│   ├── concurrency-approval.md   (guardian)
│   ├── development-plan.md        (planner)
│   ├── execution-plan.yml         (planner, 5 units)
│   ├── inspection-report.md       (inspector, verdict=usable)
│   ├── templates/                 (materialize, 6 文件)
│   └── worktree-map.yml           (worktree, 5 slots)
├── history.jsonl                                          (loop_started)
├── history.jsonl.lock
├── ledger.jsonl                                           (4 iter counter_changed)
├── loop.lock                                              (pid=29513, 仍持锁)
├── loops.json                                             (单 loop)
├── supervisor.db (+shm/wal)                               (sqlite 初始化)
└── worktrees/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan/
    ├── u1/ u2/ u3/ u4/ u5/                                (5 worktree 全建好)
```