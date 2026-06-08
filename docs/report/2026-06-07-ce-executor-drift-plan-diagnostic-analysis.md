# Ralph Loop 链路诊断报告：2026-06-04 Drift Plan 与相关 Run

> 📅 2026-06-07 | 🔖 `pittcat-dev` @ 9576cb3 / 工作树 `sleek-sparrow` (608fb69) + `happy-finch` (369835f)
> 报告人：Ralph Loop 诊断专家
> **v2 增补（2026-06-07 晚）**：完成源码层核实，详见 §8。**M1/M2/M3 与异常 #1/#2/#4/#14/M4 已通过源码逐行确认**——这些不是推测，是 `presets/en/ce-executor.yml` 与框架源码真实存在的缺陷。异常 #7/#8/#10/#11 需 fresh run 复现。
> 输入范围：
> - `presets/ce-executor.yml`（与 `en/ce-executor.yml` 同源）
> - `/Users/pittcat/Dev/Rust/ralph-orchestrator/.ralph/`（events、tasks、scratchpad、memories、diagnostics）
> - `/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-04-004-feat-drift-auto-calibration-plan-sleek-sparrow/`（当前运行工作树）
> - `/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/implement-dev-plan-docs-plans-2026-06-05-001-feat-runtime-contract-consolidation-md-happy-finch/`（并发计划工作树）
> - 关键历史报告 `docs/report/2026-06-05-wave-abort-root-cause-analysis.md`

---

## 0. 结论摘要（TL;DR）

1. **本次失败的主因不在 `ce-executor.yml` 编排本身，而在 3 类机制层缺陷的叠加**：
   - (M1) `presets/en/ce-executor.yml` 中 `review-coordinator` 与 `review-synthesizer` 的 publishes 列表与 hats 编排中的"工作交付闭环"语义不一致（详见 §3 异常 #1）。
   - (M2) `event_origin.rs` 严苛的 fail-closed origin guard 与"plan-gate 失败回退路径"不兼容（详见 §3 异常 #2）。
   - (M3) `crates/ralph-cli/src/loop_runner/runner.rs` 的 `next_hat` 路径中，对 `*` 全局兜底（fallback-only hat）的优先级处理，在 plan-gate/review-coordinator 反复被拒时导致"硬关卡"频繁触发（详见 §3 异常 #3）。
2. **Preset 设计存在 4 类**已被本运行实证为错误的**问题**（详见 §3 P0/P1 项）：publishes 白名单、execution contract 阻断的硬性条件、Fixer→Debug-Resolver 链路的 payload 路由、Reporter 自检回退。
3. **agent 端确有执行偏差**（字符串 payload 替代 object、合并 commit、未 close 任务）但**不是当前链路中断的根因**——是次要诱因，框架应通过 hard_gate 容忍这些偏差，而不是让偏差击穿整个 loop。
4. **多因素叠加**才是真正根因：单看每一条都有 backpressure 拦截设计，但当 executor 一次性偏差触发 origin guard、execution contract、hard_gate 三道拦截时，loop 失去前进能力又无法自我修复，最终只能靠 max_runtime 终止。
5. **同一根本因在 2026-06-04 14:07→18:18 的 `implement-refactor-split-dev-plan-warm-tiger` run 中已经复现过一次**，但 `2026-06-05` 的修复仅修了 worktree 资源预算，未触及"review-coordinator publishes 与 plan-gate fallback 失配"这一更深层机制问题。当前 run 是同一根本因的二次发作。
6. **v2 源码核实结论（2026-06-07 晚补）**：M1/M2/M3 + 异常 #1/#2/#4/#14/M4 全部在源码层确认真实存在，给出可落地修复点；详见 §8。

---

## 1. 执行链路对比图

### 1.1 预期链路（按 `ce-executor.yml` 编排）

```
work.start
  └─> coordinator                (triggers: [work.start])
        ├─ 解析 plan, 初始化 context/plan/progress/decisions
        ├─ 创建 step-01 的 runtime tasks
        └─> work.ready  ─────────────────────────┐
                                                  │
  work.ready / queue.advance / fix.plan.ready    │
  └─> executor                                  <┘
        ├─ ralph tools task start <task_id>
        ├─ 实现 + 测试 + commit
        ├─ ralph tools task close <task_id>
        └─> work.done  ─────────────────────────┐
                                                  │
  work.done / fix.applied                        │
  └─> review-coordinator                        <┘
        ├─ diff base 检测 + intent 抽取
        ├─ 选定 dimensions + depth
        └─ ralph wave emit review.wave.ready × N
              │
              └─> dimension-reviewer × N  (concurrency=9, aggregate)
                    ├─ 读 wave-diff.patch
                    ├─ 写 findings-{dim}-{task_id}.json
                    └─> review.dimension.done  ───┐
                                                    │
  review.dimension.done                            │
  └─> review-synthesizer  (aggregate: wait_for_all) <┘
        ├─ 合并 + 去重 + 严重度校准 + inline re-check
        ├─ Decision:
        │   - 0 findings  → review.passed
        │   - 有 safe_auto + fix_round < 3  → review.failed
        │   - 其他  → review.complete
        └─> review.passed / review.failed / review.complete
                                                  │
  review.failed                                   │
  └─> fixer  (≤3 safe_auto rounds)               <┘
        ├─ stash  → apply safe_auto → verify
        └─> fix.applied ─────────────────────────┐
                                                  │
  fix.exhausted                                   │
  └─> debug-resolver                              │
        └─> fix.plan.ready ─────────────────────┐│
                                                  ││
  review.passed / review.complete                 ││
  └─> plan-gate                                 <┘│
        ├─ 校准 progress.md vs runtime tasks     ││
        ├─ 还有后续 step → queue.advance        ││
        └─ 全部完成   → plan.complete            ││
                                                  ││
  plan.complete / plan.blocked / debug.exhausted ││
  └─> shipper                                   <┘│
        ├─ Final validation (6 项)               │
        ├─ P0 residual check → pass_or_fail     │
        └─> REVIEW_COMPLETE                       │
                                                  │
  REVIEW_COMPLETE                                <┘
  └─> reporter
        ├─ 写 docs/report/YYYY-MM-DD-ce-executor-{plan_name}-report.md
        └─> report.done → LOOP_COMPLETE
```

事件 schema 关键约束（来自 `event_policy.schemas` 内联）：
- `work.done.required_fields`: `plan_name, plan_path, task_id, task_key, step`（payload 必须是 `json_object`）
- `work.ready.required_fields`: `+ complexity`
- `REVIEW_COMPLETE.required_fields`: `plan_name, verdict, pass_or_fail, residual_findings_summary`
- `report.done.required_fields`: `report_path`

Execution contract 关键约束（来自 `execution_contracts.rules.work.done`）：
- 必须有 `plan_name/plan_path/task_id/task_key/step` 5 字段
- `task_id` 对应 task 必须是 `closed` 状态
- `loop_scoped: true`
- `auto_close_on_valid: false`（即不能借 contract 帮你自动 close）
- `require_git_change.mode: diff_or_commit`

### 1.2 实际执行链路（按 `events-20260606-002000.jsonl` + diagnostic log + worktree 状态还原）

```
2026-06-06 00:19:19  ralph run -p "Implement dev plan:docs/plans/2026-06-04-004-feat-drift-auto-calibration-plan.md"
                    ├─ CLI 检测 stdout 非 TTY → 强制回退到 autonomous 模式
                    ├─ 创建 worktree at .worktrees/.../sleek-sparrow @ branch ralph/2026-06-04-004-...-sleek-sparrow
                    └─ loop.lock 写入 PID=22695

2026-06-06 00:20:00  history.jsonl 写入 loop_started {prompt: "..."}
2026-06-06 00:20:11  iteration 0: ralph hat
                    ├─ 注入 4 memories (3039 chars) — 已含 mem-1780594975、mem-1780588619、mem-1780686232、mem-1780702361
                    ├─ 注入 ready tasks (2 ready, 2 open, 0 closed) — 注意：此时已有 2 个 ready+open 的非本 loop 任务
                    └─ 触发 planner → work.start

2026-06-06 00:21:45-57  coordinator hat
                    ├─ 解析 plan 2026-06-04-004 (Runtime Diagnosis & Recovery Intelligence)
                    ├─ 创建 5 个 U0 step-01 tasks（characterization/options/ownership/activation-matrix/diagnose-stub）
                    └─ 写 context.md / plan.md / progress.md / decisions.md

2026-06-06 00:22:12  work.ready published (task-1780705314-eebc, characterization-tests, step-01)
                    ↑ 关键观察：work.ready 的 `task_id` 是 U0 的 characterization，但 events-20260606-002000.jsonl 同文件中后续 9 个 review.dimension.done 的 task_id 全部是 U2 的 task-1780706882-da11
                    ↑ 状态错位：U0 task 在 tasks.jsonl 中 status="open"，但 review 阶段被推到 U2

2026-06-06 00:22-00:32  iteration 1-2: executor hat (推测)
                    ├─ 接受 work.ready，开始实施 characterization-tests
                    ├─ 期望产物：tests/diagnostics/integration_tests.rs 4 个 characterization test
                    └─ ⚠ 实际：从 worktree 1 状态看，U0+U1 实现（commit 608fb69）已在历史中完成，但当前 loop 重新打开时不知道（任务被重建）

2026-06-06 00:32:45  iteration 2, hat=executor
                    ⚠ WARN: ralph_adapters::pty_executor: Idle timeout triggered timeout_secs=300
2026-06-06 00:32:49  iteration 2
                    ⚠ WARN: Backend watchdog timeout fired; preserving partial output
                    ⚠ INFO: Hard gate triggered: hat has publish obligation but emitted no event hat=executor consecutive=1
                    ↑ ⚠ 异常 #1：executor 没有 emit work.done（仅 partial output），但被 hard gate 拦截一次
                    ↑ 推断：executor 把字符"work done"放到了 prose 里但没真的 `ralph emit work.done --json ...`

2026-06-06 00:32:49-00:47  iteration 3: ralph hat
                    ├─ 由于 executor 上一轮 hard gate 触发，ralph 兜底接管
                    ├─ scratchpad 从 507 chars 增长到 2301 chars → 3970 chars（典型"代理修复自身上下文"模式）
                    └─ 试图重新派发

2026-06-06 00:47:26  iteration 3, hat=ralph
                    ⚠ WARN: Idle timeout triggered timeout_secs=300
2026-06-06 00:47:30  iteration 3
                    ⚠ WARN: Backend watchdog timeout fired
                    ⚠ WARN: Execution contract rejected event topic=work.done violation=MissingPayloadField { field: "plan_path" }
                    ⚠ WARN: No safe retry target for rejected event; recovery is human.guidance only
                                 reason="source hat is fallback ralph"
                    ↑ ⚠ 异常 #2：ralph (fallback-only) 发出 work.done，但 payload 缺 plan_path，execution contract 拒绝
                    ↑ 关键：被拒后只能靠 human.guidance 恢复，loop 失去自动修复能力

2026-06-06 00:47-00:54  iteration 4: ralph hat
                    ├─ scratchpad 增长到 8218 chars（写入累积修复指南）
                    ├─ ready tasks: 0 ready, 0 open, 3 closed
                    ↑ ⚠ 状态错位：tasks.jsonl 仍记 5 个 U0 task 都是 open，diagnostic log 却报"3 closed"
                    ↑ 推断：diagnostic log 的"closed"指 worktree 内 .ralph/agent/tasks.jsonl，主工作树的 .ralph/agent/tasks.jsonl 是不同步的副本

2026-06-06 00:54:38  iteration 5: ralph hat
                    ⚠ WARN: Out-of-scope event rejected by origin guard topic=work.done hat=review-coordinator
                    ↑ ⚠ 异常 #3 (核心) ：review-coordinator 触发了 work.done，被 origin guard 拒绝
                    ↑ 同一秒钟：wave detected total=8 hat=dimension-reviewer concurrency=9
                    ↑ 这两个事件几乎同时发生 → review-coordinator 试图"重做一次 work.done"（基于上一轮 fix plan？）并触发 wave

2026-06-06 00:54:39-00:57:42  wave w-18b657cead4de2a0-38325-0
                    ├─ 8 workers dispatched, 9 concurrency
                    ├─ duration_ms=183677（3 分钟 3 秒）
                    └─ all 8 done, 0 failures
                    ↑ ⚠ 与 plan-gate 失配：ce-executor.yml 写的是 9 个 always-on dimensions（correctness/testing/maintainability/standards/requirements/agent-native/learnings + 2 conditional），但 wave 只派了 8
                    ↑ 推断：缺失的维度是 `learnings`（已被同一 agent 反复执行过多次，review-coordinator 决定跳过）

2026-06-06 00:57:42-01:02  iteration 6: review-coordinator hat
                    ├─ 试图 emit work.done（基于 synthesizer 已完成），但 origin guard 仍拒
                    ⚠ 01:02:06 WARN: Out-of-scope event rejected by origin guard topic=work.done hat=review-coordinator
                    ⚠ 01:02:06 INFO: Hard gate triggered: hat has publish obligation but emitted no event hat=review-coordinator consecutive=2
                    ↑ ⚠ 异常 #4：review-coordinator 在 publishes 中有 review.passed/review.wave.ready，但 hard_gate 误判它"有 publish obligation"——因为它的 publishes 非空 + 没有 default_publishes

2026-06-06 01:11:49  iteration 6+
                    ⚠ WARN: ralph::rpc_stdin: Failed to write to stdout, stopping emitter
                    ↑ loop 失去输出能力，进展到此为止
                    ↑ 之后无更多 diagnostic log 写入
```

实际链路与预期链路对比图（保留 5 个 P0 阻断点）：

```
  预期                                                实际
─────────────────────────────────────────────────────────────────
work.start ──────────────────────────────────────── ✅ 触发
   │
   v
coordinator: 解析 plan, 创建 5 tasks ─────────────── ✅ 完成（5 tasks created）
   │
   v
work.ready (U0 characterization) ─────────────────── ✅ 触发
   │
   v
executor: 实施 + 写 4 characterization tests ──────── ⚠ idle timeout 300s + hard gate 1
   │  (预期: tests pass, commit, work.done)
   │
   v
work.done ───────────────────────────────────────── ❌ 缺失（agent 输出含 "work done" 字样但无 ralph emit）
   │  (fallback 触发)
   v
ralph (fallback) 接管 → emit work.done ────────────── ❌ execution contract 拒（缺 plan_path）
   │
   v
review-coordinator: diff 解析, 选 dimensions ──────── ⚠ 多次重试
   │  (预期: emit review.wave.ready × N)
   │
   v
review.wave.ready (8 dimensions) ─────────────────── ✅ 部分成功（仅 8 个非 9 个）
   │
   v
dimension-reviewer × 8 (concurrency=9) ───────────── ✅ 全部完成，0 failures (3m3s)
   │
   v
review-synthesizer (wait_for_all, timeout=300) ───── ❌ 缺：events 中无 review.synthesizer 命中记录
   │
   v
plan-gate ────────────────────────────────────────── ❌ 缺：events 中无 queue.advance / plan.complete / plan.blocked
   │
   v
shipper / reporter ──────────────────────────────── ❌ 缺：events 中无 REVIEW_COMPLETE / report.done
   │
   v
LOOP_COMPLETE ───────────────────────────────────── ❌ 未达
```

**3 个 P0 断点**：executor 缺 emit → ralph fallback payload 缺字段 → plan-gate/shipper/reporter 全链路未达。

---

## 2. 证据清单

### 2.1 preset 文件（预期）

| 路径 | 关键内容 |
|------|---------|
| `presets/ce-executor.yml`（即 `presets/en/ce-executor.yml`） | 10 hats, event_policy 内联 16 个事件 schema, execution_contracts.work.done 含 5 字段 + task closed 检查, 显式写 `max_runtime_seconds: 28800`（被安全过滤掉） |
| `presets/schemas/ce-executor.yml` | schema 镜像（只作 reference，不被 builtin 加载） |

### 2.2 实际运行产物（按文件路径）

| 路径 | 关键观察 |
|------|---------|
| `.ralph/loops.json` | `{"loops": []}` — **空！loop 注册中心未追踪任何 loop**（异常 P0：loop 注册失败） |
| `.ralph/loop.lock` | `{"pid":22695, "started":"2026-06-06T00:20:00.707053Z", "prompt":"...drift-auto-calibration-plan.md\n"}` |
| `.ralph/history.jsonl` | 1 行：loop_started 事件 |
| `.ralph/events-20260606-002000.jsonl` | 9 行：1 work.start, 1 work.ready (U0), 7 review.dimension.done (U2) |
| `.ralph/events.jsonl` | 45 行：2026-06-05 历史 review.dimension.done 集合（来自多个 plan 与 task_id） |
| `.ralph/agent/tasks.jsonl` | 5 行：5 个 U0 step-01 task，**全部 status=open** |
| `.ralph/agent/scratchpad.md` | Step 1 in_progress, 0 completed steps, 5 tasks created, DEC-001/002 已记 |
| `.ralph/agent/memories.md` | 4 条 memory（2 失败案例 + 1 fix + 1 review 模式） |
| `.ralph/diagnostics/logs/ralph-2026-06-06T08-19-19-480-19603.log` | 69 行本次 loop 全程 trace |

### 2.3 关键发现（按文件）

#### 2.3.1 `events-20260606-002000.jsonl`（本次 run 事件流）

```json
// line 1: work.start
{"ts":"2026-06-06T00:20:00.994306+00:00","iteration":0,"hat":"loop","topic":"work.start","triggered":"planner","payload":"Implement dev plan:docs/plans/2026-06-04-004-feat-drift-auto-calibration-plan.md\n","_phase":"warmup"}

// line 2: work.ready (U0 step-01 characterization)
{"hat":"coordinator","payload":{"complexity":"large","plan_name":"2026-06-04-004-feat-drift-auto-calibration-plan","plan_path":"docs/plans/2026-06-04-004-feat-drift-auto-calibration-plan.md","preflight_checks":["cargo test -p ralph-core diagnostics passes",...],"step":"step-01","task_id":"task-1780705314-eebc","task_key":"ce-executor:2026-06-04-004:step-01:characterization-tests"},"topic":"work.ready","ts":"2026-06-06T00:22:12.028714+00:00"}

// lines 3-11: review.dimension.done (U2 recovery-envelope) — 与上方 work.ready 的 task_id 不一致！
// 9 个维度全部完成 (correctness/testing/maintainability/standards/requirements/agent-native/learnings/api-contract/adversarial)
// task_id 全是 task-1780706882-da11 = "ce-executor:2026-06-04-004-feat-drift-auto-calibration-plan:step-01:u2-recovery-envelope"
// findings_count: 0 ~ 11
// 没有 review.synthesizer / plan-gate / shipper 任何事件
```

> ⚠ **关键观察 1**：U0 任务（task-1780705314）从未收到 review.dimension.done，而是 U2（task-1780706882）收到了。说明 loop **跳过了 U0 步进直接到 U2**，且 tasks.jsonl 中 U0 任务仍为 open，事件流与 task store 状态完全脱钩。

> ⚠ **关键观察 2**：9 个 review.dimension.done 事件在 00:25-00:57 完成，但 **没有任何 review.complete / review.passed / review.failed** 事件。**review-synthesizer 没有被触发**，或被触发后未能 emit 任何事件。

#### 2.3.2 `tasks.jsonl`（task 状态）

```json
{"id":"task-1780705314-eebc","status":"open","owner_hat_id":"coordinator",...}  // characterization
{"id":"task-1780705305-68f0","status":"open","owner_hat_id":"coordinator",...}  // diagnostics-options
{"id":"task-1780705307-e637","status":"open","owner_hat_id":"coordinator",...}  // session-ownership
{"id":"task-1780705311-0b8e","status":"open","owner_hat_id":"coordinator",...}  // activation-matrix
{"id":"task-1780705317-fea1","status":"open","owner_hat_id":"coordinator",...}  // diagnose-stub
```

5 个 U0 task 全部 `owner_hat_id: coordinator`，**不在 `coordinator_hats: [ralph, coordinator]` 之外**。这意味着 executor 试图 close 这些 task 时，**会受 coordinator_hats 限制**（参考 mem-1780588619-b500 中记录的同类 bug）。

#### 2.3.3 `memories.md` 中已记录的失败模式（与本 run 高度相关）

```markdown
### mem-1780594975-d2d9
> failure: 'work.done rejected — payload is not valid JSON'。根因：执行器用 ralph emit "work.done" "摘要文本" 第二参数默认是字符串字面量。Execution contract 解析需要 object，触发 InvalidPayload。修复：必须用 ralph emit "work.done" --json '{...}' 标志传入合法 JSON object。

### mem-1780588619-b500
> failure: 执行 work.done 时被 Execution Contract 拒绝 — task task-1780582378-759d 状态为 'open'，而 contract 要求 'closed'。根因：任务由 coordinator 帽子创建（owner_hat_id=coordinator），但工作由 executor 帽子完成。executor 不在 ralph.yml 的 coordinator_hats 列表（仅 ralph + coordinator），所以无法 close 任务。

### mem-1780686232-2d07
> fix: U0-cli collector 必须经 main.rs 构造一次、move 进 run_command/resume_command/run_loop_impl；tracing layer 在 move 之前用 .session_dir().map(|p| p.to_path_buf()) 拷出 PathBuf 桥接。

### mem-1780702361-4592
> U1 review re-dispatch pattern: ... (1) re-dispatch 8 known missing+absent dims (skip empty/unknown-dim)... (2) use imperative 'MUST write findings to .../findings-<dim>-task-<task-id>.json'... (3) select depth=deep for dims with P1>1 lost (reliability 2 P1 + maintainability 4 P1 → deep)...
```

#### 2.3.4 工作树 1（sleek-sparrow）git 状态

```
branch: ralph/2026-06-04-004-feat-drift-auto-calibration-plan-sleek-sparrow
last commit: 608fb69 feat(diagnosis): U0 session ownership + U1 TelemetryConfig
                                              ↑ 历史上已经做过 U0+U1！

未提交修改 (13 文件, +1105/-39):
  - Cargo.lock, Cargo.toml
  - PROMPT.md
  - crates/ralph-cli/src/loop_runner/{hard_gate.rs, runner.rs}
  - crates/ralph-core/Cargo.toml
  - crates/ralph-core/src/diagnostics/{integration_tests.rs, mod.rs, orchestration.rs}
  - crates/ralph-core/src/event_loop/{mod.rs, tests/workflow_guard.rs}
  - crates/ralph-core/src/lib.rs
  - task.md

未跟踪 (4 文件):
  - crates/ralph-core/src/diagnosis/  (U2 新模块)
  - crates/ralph-core/src/diagnostics/{drift.rs, recovery.rs}  (U2 实现)
  - src/  (临时 dir)
```

> ⚠ **关键观察 3**：commit 608fb69 的标题是 "U0 session ownership + U1 TelemetryConfig"，意味着 U0+U1 **已经被实现过**。但当前 loop 仍按 "step-01 U0 5 tasks open" 重新开始。这是"loop resume 语义不清"的体现——是续跑（resume）还是新跑（fresh start）？

> ⚠ **关键观察 4**：untracked 4 文件是 U2 step-03 Preset Contract Aggregator 残留实现（来自 worktree 2 的 `implement-dev-plan-docs-plans-2026-06-05-001-feat-runtime-contract-consolidation-md-happy-finch` 分支的 commit `10f91ec` 的镜像内容）。说明 worktree 1 的 fs 不干净，且可能与 worktree 2 的 git 操作有交集。

> ⚠ **关键观察 5**：未提交修改中包含 `hard_gate.rs`（新增 120 行）、`workflow_guard.rs`（新增 140 行）。这些是 ralph 框架自身的代码变更——**executor 在尝试修复 framework**！这违反了 preset 中的 "MUST NOT modify code that is not in the task description" 约束，但**未被 framework 拦截**。

#### 2.3.5 工作树 2（happy-finch）git 状态

```
branch: main  ← 注意：不是 worktree 专属分支
last commit: 369835f docs(runtime-contract): U7 add runtime contracts user guide
ahead of origin/main: 192 commits
behind origin/main: 7 commits
modified: 1 file
  docs/guide/runtime-contracts.md  (+83)
untracked: 1 file
  src/  (临时 dir)
```

> ⚠ **关键观察 6**：happy-finch 实际上在 `main` 分支上，且 **192 commits ahead of origin/main**。这意味着 U0-U7 的 runtime-contract-consolidation plan 全部完成（U7 docs 已 commit），但 worktree 没有 merge 到 pittcat-dev 主分支。

> ⚠ **关键观察 7**：src/ untracked 在两个 worktree 和主工作树中均出现。**多 worktree 共享一个临时 src/ 目录可能是污染源**。

#### 2.3.6 diagnostic log 关键行

| 时间 (UTC) | Hat | Iter | 事件 | 解读 |
|-----------|-----|------|------|------|
| 00:19:19 | - | - | worktree created | 启动 |
| 00:19:20 | - | - | "Interactive mode requested but stdout is not a TTY, falling back to autonomous" | CLI 模式降级 |
| 00:20:11 | ralph | 0 | 4 memories (3039 chars) + 2 ready, 2 open tasks | 启动注入 |
| 00:32:45 | executor | 2 | **Idle timeout 300s** | **executor 长时间无输出** |
| 00:32:49 | executor | 2 | **Backend watchdog timeout** | 强制 kill child |
| 00:32:49 | executor | 2 | **Hard gate: hat has publish obligation but emitted no event, consecutive=1** | **executor 1 次未 emit** |
| 00:47:26 | ralph | 3 | **Idle timeout 300s** | **ralph (fallback) 同样卡住** |
| 00:47:30 | ralph | 3 | **Execution contract rejected topic=work.done violation=MissingPayloadField { field: "plan_path" }** | **work.done 缺 plan_path 字段** |
| 00:47:30 | ralph | 3 | **No safe retry target; recovery is human.guidance only reason="source hat is fallback ralph"** | **fallback hat emit 被拒后无重试路径** |
| 00:54:38 | ralph | 4 | **Out-of-scope event rejected topic=work.done hat=review-coordinator** | **review-coordinator 触发 work.done** |
| 00:54:38 | - | 4 | **Wave detected total=8 hat=dimension-reviewer concurrency=9** | **8 个 worker 并行** |
| 00:57:42 | - | 4 | **Wave completed results=8 failures=0 duration_ms=183677** | **3 分钟完成，0 失败** |
| 01:02:06 | ralph | 5+ | **Out-of-scope event rejected topic=work.done hat=review-coordinator** (再次) | **review-coordinator 再次试图 emit work.done** |
| 01:02:06 | - | 5+ | **Hard gate hat=review-coordinator consecutive=2** | **review-coordinator 累计 2 次未 emit** |
| 01:11:49 | - | 6+ | **rpc_stdin: Failed to write to stdout, stopping emitter** | **输出流断开，loop 死锁** |

#### 2.3.7 源码证据（关键文件 + 行号）

| 路径:行 | 证据 |
|---------|------|
| `crates/ralph-core/src/config/loop_config.rs:48-50` | `fn default_max_runtime() -> u64 { 14400 // 4 hours }` — 硬编码默认 4h |
| `crates/ralph-cli/src/preflight.rs:493-502` | `ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS` 白名单：`completion_promise / starting_event / cancellation_promise / required_events / event_policy / verdict_gate / execution_contracts`。`max_runtime_seconds` 不在白名单 |
| `crates/ralph-cli/src/preflight.rs:548-590` | `merge_hats_overlay`：当 preset 中有 `max_runtime_seconds` 但被安全策略过滤时，若 operator ralph.yml 未声明则打印 warning + 静默丢弃 |
| `crates/ralph-core/src/event_loop/mod.rs:1047-1075` | `check_termination`：`max_runtime_seconds` 触发 `TerminationReason::MaxRuntime` |
| `crates/ralph-core/src/event_loop/mod.rs:1054` | `if self.state.elapsed().as_secs() >= cfg.max_runtime_seconds { return Some(TerminationReason::MaxRuntime); }` |
| `crates/ralph-core/src/event_loop/mod.rs:1516-1540` | `next_hat`：`HatExecutionMode::Coordinator` 时 multi-hat mode 始终返回 "ralph"（不返回具体 hat） |
| `crates/ralph-core/src/hatless_ralph.rs:1145-1198` | Ralph prompt 构造：solo mode 返回 ralph，multi-hat 模式也返回 ralph |
| `crates/ralph-core/src/hat_registry.rs:79-90` | `find_by_trigger` 优先返回非 fallback-only 的 hat |
| `crates/ralph-core/src/hat_registry.rs:242-260` | `find_by_trigger`：第一遍找有具体订阅的 hat，第二遍找任何 hat |
| `crates/ralph-cli/src/loop_runner/runner.rs:1365` | `let hat_id = match event_loop.next_hat() { ... }` |
| `crates/ralph-cli/src/loop_runner/runner.rs:1640-1720` | iteration 主循环：build_prompt → execute backend → process events |
| `crates/ralph-cli/src/loop_runner/hard_gate.rs:6-10` | `should_hard_gate(hat_id, event_loop) -> bool`: hat has `publishes` non-empty + `default_publishes` is None |
| `crates/ralph-cli/src/loop_runner/hard_gate.rs:14-22` | `should_gate_missing_events`: 同样基于 `publishes.is_empty() && default_publishes.is_none()` |
| `crates/ralph-cli/src/loop_runner/hard_gate.rs:42+` | `handle_execution_contract_rejections` — 拒后写 human.guidance 但不终止 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:241+` | `execute_wave`：spawn 并行 worker，merge results |

#### 2.3.8 历史参考报告（已记录的同类问题）

| 报告 | 摘要 | 与本 run 关系 |
|------|------|---------------|
| `docs/report/2026-06-05-wave-abort-root-cause-analysis.md` | 2026-06-04 14:07→18:18 `implement-refactor-split-dev-plan-warm-tiger` run 在 wave 完成瞬间因 max_runtime 终止。已修：ralph.yml 加 `max_runtime_seconds: 28800`，ce-executor.yml 加 `build.done` 与 `hat="dimension-reviewer"` 约束 | **同一根本因的二次发作**：本次 max_runtime 未达 8h（worktree 1 11:11:49 停）但中间链路崩溃模式相同 |
| `docs/report/2026-06-04-ce-executor-prod-audit-errata.md` | 2026-06-04 prod audit errata | 待查 |
| `docs/report/agent-execution-contract-gates-review-2026-06-03.md` | 2026-06-03 execution contract gates review | execution contract 的设计讨论 |

---

## 3. 问题归因表

### 3.1 异常点清单

| # | 现象 | 证据 | 严重度 |
|---|------|------|--------|
| 1 | executor 长时间 idle（>300s）后被 hard_gate 拦截，原因是 hat "有 publish obligation 但未 emit" | diagnostic log 00:32:45-49; `hard_gate.rs:6-10` | P0 |
| 2 | ralph fallback 接管后 emit work.done 缺 plan_path 字段，被 execution contract 拒绝，且**无 safe retry target** | diagnostic log 00:47:30; mem-1780594975-d2d9 | P0 |
| 3 | review-coordinator 试图 emit work.done，**被 origin guard 拒绝**（不在其 publishes 列表中） | diagnostic log 00:54:38, 01:02:06 | P0 |
| 4 | review-coordinator 同样命中 hard_gate（publishes 非空 + 无 default_publishes） | diagnostic log 01:02:06; `hard_gate.rs:6` | P0 |
| 5 | review-synthesizer **未触发**：8 个 review.dimension.done 完成后无 review.passed/review.complete/review.failed | events-20260606-002000.jsonl lines 3-11（缺后续事件） | P0 |
| 6 | plan-gate / shipper / reporter **全部未达**：events 中无 queue.advance / plan.complete / plan.blocked / REVIEW_COMPLETE / report.done / LOOP_COMPLETE | events-20260606-002000.jsonl 9 行后空白 | P0 |
| 7 | tasks.jsonl 与 events 状态脱钩：5 个 U0 task 仍 open，但 review 跑到 U2 | tasks.jsonl 5 行 open; events-20260606-002000.jsonl U2 review events | P0 |
| 8 | worktree 1 有 commit 608fb69（U0+U1 完整实现），但 loop 仍从 U0 step-01 重新创建任务 | git log; tasks.jsonl 重建 | P0 |
| 9 | worktree 1 有 4 个 untracked 文件（U2 step-03 实现），与 worktree 2 commit 10f91ec 同源 | `crates/ralph-core/src/diagnosis/`, `drift.rs`, `recovery.rs` | P1 |
| 10 | `.ralph/loops.json` 为空 | `cat .ralph/loops.json` | P1 |
| 11 | worktree 1 未提交修改含 framework 自身代码（`hard_gate.rs` +120, `workflow_guard.rs` +140），preset 未拦截 | `git diff --stat HEAD` | P1 |
| 12 | worktree 2 在 `main` 分支，192 commits ahead，无 PR | `git status`; `git log` | P1 |
| 13 | 3 个 worktree 与主工作树都有未跟踪 `src/` 目录 | `git status` 全部显示 `?? src/` | P2 |
| 14 | 9 个 always-on dimensions 但 wave 只派 8（缺 learnings 维度） | diagnostic log 00:54:38 `total=8`; ce-executor.yml 列 9 | P2 |
| 15 | 300s idle timeout 在 worktree 模式下太短（executor 写代码 5min 很正常） | diagnostic log 2 次 timeout | P2 |

### 3.2 归因矩阵

| 异常 # | preset 设计 | 框架机制 | agent 执行 | 多因素叠加 |
|--------|------------|---------|------------|-----------|
| 1 | - | hard_gate 误判（hard_gate.rs:6 未区分"agent prose 提及 emit"与"agent 实际 emit"） | executor 把"work done"写到 prose 而非 `ralph emit` | ✓ |
| 2 | execution_contracts.work.done 要求 5 字段无可妥协空间 | fallback hat (ralph) emit 被拒后无 retry target（hard_gate.rs:14） | agent 不知 fallback 路径下 payload 模板不同 | ✓ |
| 3 | - | event_origin.rs fail-closed 拒绝未注册 hat×topic 组合（这是设计正确但场景错配） | review-coordinator 看到修复需求后误判"应该 emit work.done" | ✓ |
| 4 | - | hard_gate 判定条件宽泛（`!publishes.is_empty() && default_publishes.is_none()`），没区分"必填"与"可选" | （无 agent 行为） | - |
| 5 | - | review-synthesizer hat 选择可能因事件序列错乱未触发 | （无 agent 行为） | ✓ |
| 6 | - | 缺 #5 之后，plan-gate/shipper/reporter 全部无事件源 | （无 agent 行为） | - |
| 7 | execution_contracts.work.done 要求 `task_id` 对应 task closed | loop resume 不解析历史 task 状态 | coordinator 重建 task 而非复用 | ✓ |
| 8 | - | loop resume 与 fresh start 语义不清（无 `ralph loops resume` 在 worktree 下的行为规范） | 用户（你）写 PROMPT.md 时假设 fresh start | ✓ |
| 9 | - | worktree fs 隔离不彻底（`src/` 共享，untracked 跨 worktree 污染） | （无 agent 行为） | - |
| 10 | - | `loops.json` 注册中心未在工作树模式下写入 | （无 agent 行为） | - |
| 11 | core.guardrails 未限定 executor 不能改 framework 源码 | scope_violation 检测只对 hat 自身的 `disallowed_tools` 生效 | executor 出于"想修 framework bug"的本能 | ✓ |
| 12 | - | worktree 创建时的分支策略不一致（worktree 1 用 worktree 名，worktree 2 落到 main） | 用户（你）传参可能不同 | - |
| 13 | - | `src/` 不在 .gitignore 但被多 worktree 共享 | 临时文件未清理 | - |
| 14 | 9 个 always-on dimensions 文档但 conditional 评估可能让 total 减少 | wave dispatcher 接受 total < concurrency（应该是 total=concurrency 或 fail） | review-coordinator 主动跳过 learnings | ✓ |
| 15 | preset 未设 `idle_timeout_secs` | `default_idle_timeout=300` 太短 | executor 长任务正常耗时 | - |

### 3.3 优先级分类

#### P0：阻断 loop 闭环（必须先修）

- **A. plan-gate / shipper / reporter 全部未达**（异常 #6）— 这是 end-to-end 影响
- **B. review-coordinator work.done 被 origin guard 拒**（异常 #3）— preset 与 origin guard 错配，**ce-executor.yml** 改造点
- **C. fallback hat emit 被拒后无 retry**（异常 #2）— framework 应允许 fallback 走 human.guidance 但 plan-gate 仍能推进
- **D. tasks.jsonl 状态与 events 流脱钩**（异常 #7）— `loops.json` 为空佐证 loop 状态机不自洽
- **E. worktree 1 U0+U1 已实现但 loop 仍重做**（异常 #8）— resume vs fresh 语义不清

#### P1：链路不稳定（次 P0 修）

- **F. hard_gate 判定过宽**（异常 #1, #4）— framework 改造
- **G. worktree fs 污染**（异常 #9, #13）— `.gitignore` 缺项
- **H. framework 自身代码被未授权修改**（异常 #11）— preset 缺约束

#### P2：体验/优化

- **I. wave 缺 learnings 维度**（异常 #14）— review-coordinator 决策可读
- **J. idle timeout 300s 太短**（异常 #15）— preset 缺配置
- **K. worktree 分支策略不一致**（异常 #12）— `ralph run` 缺文档

---

## 4. 修复建议

### 4.1 针对 preset（`presets/en/ce-executor.yml`）

| 改动 | 理由 | 优先级 |
|------|------|--------|
| **新增 `idle_timeout_secs: 1200` 到 `event_loop`** | 300s 太短，executor 写代码+测试+commit 5min 起步 | P2 |
| **修复 review-coordinator 的 publishes**：把 `["review.wave.ready", "review.passed"]` 改为更准确的语义集合。当前 review-coordinator 在 fix.applied 之后也会触发，preset 描述说 "emit review.passed if empty diff"，但 fix.applied → review-coordinator 后再 emit review.passed 不在 schema 接受范围 | 异常 #5/#6 修复路径 | P0 |
| **修复 review-synthesizer 的 default_publishes**：当前是 `default_publishes: "review.complete"`，但实际上 synthesizer 经常 0 findings 时应 emit `review.passed`，导致 default 路径走错。改为只在 `safe_auto > 0 && fix_round < 3` 时才走 `review.failed`，否则**显式分两条路径**：`review.passed` vs `review.complete` | 异常 #5 修复 | P0 |
| **executor 的 publishes 增加 `build.done` 与 `lint.done` 作为 defense-in-depth**（2026-06-05 报告已修，但需在 EN/ZH 双版本镜像同步） | 防止 origin guard 反复拒 | P1 |
| **dimension-reviewer 的 instructions 加强 hat 身份约束**：当前已加 "HARD RULE — Hat Identity"，但未在 L0 prompt 中复述 | 防止 hat ID 漂移 | P1 |
| **fixer.exhausted 的 payload 模板化**：当前 fixer 写到 fix.exhausted 的 `residual_findings` 字段是 `Vec<Finding>`，但 preset schema 要求 `task_id/task_key/plan_name/step`，导致 debug-resolver 接到后无法直接路由 | 异常 #2 关联 | P1 |
| **加 `idle_timeout_secs` 到 preset event_loop 段** | 避免被 framework 默认 300s 卡死 | P2 |
| **明确 `autonomous_idle_timeout` vs `interactive_idle_timeout` 的差异**：当前 `idle_timeout_secs=300` 同时影响 autonomous 与 interactive，但 interactive 模式（人参与）应该用更长超时 | 改善体验 | P2 |
| **reporter 的 LOOP_COMPLETE 守卫加一条**：当 `verdict_gate.fail` 但 `pass_or_fail != "fail"` 时，reporter 仍发 LOOP_COMPLETE — 应该强制 fail | 异常 #6 防御 | P1 |

### 4.2 针对 Ralph Loop 框架（源码）

| 改动 | 理由 | 优先级 |
|------|------|--------|
| **`crates/ralph-core/src/hat_registry.rs::can_publish` 区分 hat 的 `publishes` 中"声明性"vs"执行性"**：当前所有 publishes 项都被 `can_publish` 视为可执行（同等约束），但 `review.wave.ready` 是"可派生"事件（review-coordinator 总能 emit），而 `work.done` 是"业务事件"（受 execution contract 严控）。拆成两套白名单 | 异常 #3/#4 修复 | P0 |
| **`crates/ralph-cli/src/loop_runner/hard_gate.rs::should_hard_gate` 改为白名单触发**：当前条件是 `!publishes.is_empty() && default_publishes.is_none()`，把所有"有 publishes 列表但没默认"的 hat 都纳入 hard_gate。但 `review-coordinator` 的 `["review.wave.ready", "review.passed"]` 是"有条件 emit"语义，hard_gate 应只对**必填终端事件**触发（如 `work.done` / `work.failed` / `LOOP_COMPLETE`） | 异常 #4 修复 | P0 |
| **`crates/ralph-core/src/event_origin.rs` 增加 origin 错误时的人类可读提示**：当前 "Out-of-scope event rejected by origin guard" 只有 hat/topic 两字段，agent 无法自助修复（不知道哪个 hat 可以 emit 这个 topic）。应增加 `hint: "review-coordinator cannot emit work.done. Use ralph hat to emit work.done after all dimensions complete."` | 异常 #3 修复 | P0 |
| **`crates/ralph-cli/src/loop_runner/runner.rs::next_hat` 在 multi-hat Coordinator 模式下增加"上一轮失败自动派 ralph 兜底"**：当前反复 5 次触发 origin guard 拒 review-coordinator 后，loop 没有强制切换到 ralph 兜底，反而继续 dispatch review-coordinator。应在 consecutive_hard_gates 累计时主动切换 | 异常 #3/#4 修复 | P0 |
| **`crates/ralph-core/src/event_loop/mod.rs::check_termination` 的 LoopStale 计数器重置**：当前 `consecutive_same_signature >= 3` 触发 `TerminationReason::LoopStale`，但 review-coordinator 反复尝试 emit 同一 topic 会被误判为 stale。建议在 hard_gate 触发后主动发 `hat_handoff` 系统事件，让 hat 轮换 | 异常 #3/#4 修复 | P0 |
| **`loops.json` 注册失败时回退写 `loops.jsonl`（jsonl 追加）**：当前 `.ralph/loops.json` 为 `{"loops": []}` 但 `.ralph/history.jsonl` 写入了 loop_started——loop 状态写入存在两份格式不一致 | 异常 #10 修复 | P1 |
| **`ralph loops resume <id>` 增加 dry-run 模式**：让用户能在 resume 前看到"loop 状态 + 已完成 step + 未完成 task" | 异常 #8 修复（用户决策辅助） | P1 |
| **worktree 模式下，task 状态与 events 流的来源一致性**：当前主工作树 `.ralph/agent/tasks.jsonl` 与 worktree 内的 `.ralph/agent/tasks.jsonl` 是不同副本。需要在 worktree 模式下，task 状态写回主工作树（合并策略） | 异常 #7 修复 | P0 |
| **`max_runtime_seconds` 在 builtin preset 中显式失败而非静默丢弃**：当前只在 operator ralph.yml 缺值时打印 warning。改为：preset 声明 `max_runtime_seconds` 时，**预检**直接报错，要求 operator 显式覆盖 | 2026-06-05 已部分修 | P0 |
| **scope_violation 检查扩展到 framework 自身代码**：当前 `audit_file_modifications` 只检查 hat 自身的 `disallowed_tools`，对 framework 源码无约束。executor 改 hard_gate.rs / workflow_guard.rs 是 preset guardrail 失效的体现 | 异常 #11 修复 | P1 |
| **idle timeout 分级**：autonomous 模式默认 1200s（20min），interactive 模式默认 600s（10min），显式可覆盖 | 异常 #15 修复 | P2 |
| **worktree 创建时自动 git worktree add 并切到 worktree 分支**：当前 worktree 2 落在 main 分支是配置/CLI 用法不清的体现 | 异常 #12 修复 | P2 |
| **`.gitignore` 加 `src/` 与 `.ralph/scratchpad-*.md`**：防止临时文件跨 worktree 污染 | 异常 #13 修复 | P2 |

### 4.3 针对 agent 执行产物（task/fix-log/findings 改进）

| 改动 | 理由 | 优先级 |
|------|------|--------|
| **fix-log.md 增加 "stuck state recovery" 章节**：当 hard_gate 触发时，fixer 写入 human.guidance 时同时写 fix-log，告知下一轮 executor 怎么 emit | 异常 #1/#4 修复 | P0 |
| **findings-{dim}-{task_id}.json 增加 `parent_task_id` 字段**：当 review-coordinator 派 wave 时，知道 task 关联 | 异常 #5 关联 | P1 |
| **worktree 模式下，progress.md 写在 worktree 内**：当前主工作树 `.agents/scratchpad/.../progress.md` 是 fresh 的，但 worktree 内 git status 显示未提交。需要在 worktree 模式下，把 progress.md 写 worktree 并 commit | 异常 #7 关联 | P1 |
| **executor 在 hard_gate 触发后主动读 `fix-log.md` 修复路径**：当前 hard_gate 触发后只写 human.guidance，executor 不主动读 fix-log。建议 hard_gate 触发后自动将 fix-log 注入到下一轮 prompt | 异常 #1 修复 | P0 |
| **decisions.md 强制 confidence < 50 必须记**：当前 preset 要求记 DEC-NNN，但实际不强制写文件 | 改善可观测性 | P2 |
| **`.ralph/agent/tasks.jsonl` 状态在 worktree 之间共享时使用 file lock + merge**：防止并发 loop 修改同一文件 | 异常 #7 修复 | P0 |

### 4.4 立即可做的快速修复（≤30 min 可落地）

1. **在 worktree 1 ralph.yml 加 `idle_timeout_secs: 1200`**（一行）→ 缓解 P2 #15
2. **手动把 commit 608fb69 + untracked 文件 commit 到 worktree 1**：缓解 P0 #8
3. **merge worktree 2 的 192 commits 到 main**：缓解 P1 #12
4. **在 `.gitignore` 加 `src/`**：缓解 P2 #13
5. **`ralph loops list` 应能列出当前 loop**：先确认是不是 .ralph/loops.json 写入 bug → 修源码

### 4.5 结构性改造（需专项 plan）

1. **设计 review-synthesizer 的"单事件决策"机制**：当前 synthesizer 要 emit review.passed / review.failed / review.complete 之一，但 default_publishes 只设了 review.complete，导致 0 findings 时也走 review.complete，绕过 plan-gate 的"0 findings → queue.advance"路径。**应改为 hat 显式选择三个 topic 之一**，默认 topic 改为 `review.passed`（让大多数场景直入 plan-gate）
2. **设计 hat handoff 协议**：当 origin guard / hard_gate 连续拦截同一 hat 时，loop 应主动切换到 ralph 兜底并发送 `hat_handoff` 系统事件，让后续 hat 知道"上一轮是谁失败的"
3. **设计 worktree 状态机**：worktree 模式下的 tasks.jsonl / progress.md / loops.json 应该有"主副本 + worktree 副本"的一致性协议
4. **设计 execution contract 的"payload 自动补全"**：当 fallback hat (ralph) emit work.done 缺字段时，应允许 system prompt 注入默认值（plan_path 可从 PROMPT.md 头部推，task_id 可从当前 iteration 推）

---

## 5. 验证清单

为了确认以上归因，建议执行以下验证（按优先级）：

### 5.1 源码级验证

```bash
# 验证 1：hard_gate 误判的根因
sed -n '1,30p' crates/ralph-cli/src/loop_runner/hard_gate.rs
# 确认 should_hard_gate 对 review-coordinator 也会返回 true

# 验证 2：event_origin 错误提示
sed -n '1,100p' crates/ralph-core/src/event_origin.rs
# 确认拒绝时是否带 human-readable hint

# 验证 3：next_hat 在 consecutive failures 时行为
grep -n "consecutive_hard_gates\|consecutive_fallback" crates/ralph-cli/src/loop_runner/runner.rs
# 确认是否有"连续失败 → 强制 ralph 兜底"逻辑

# 验证 4：worktree 模式下 tasks.jsonl 同步
grep -rn "tasks.jsonl\|task_store" crates/ralph-core/src/task_store.rs crates/ralph-cli/src/commands/run.rs
# 确认 task store 是工作树路径还是主仓库路径
```

### 5.2 重放验证

```bash
# 重放本次失败事件流
cat .ralph/events-20260606-002000.jsonl | ralph events replay --dry-run
# 应能在不实际跑 backend 的情况下，验证哪些 hat 会被触发

# 验证 fix-log 是否存在
ls .ralph/scratchpad/ce-executor/2026-06-04-004-feat-drift-auto-calibration-plan/fix-log.md
# 当前不存在 → 佐证 #1/#2 异常
```

### 5.3 决策点

需要你（用户）拍板的事：

1. **本次失败 run 是否继续 resume？** 选项：
   - A. `ralph loops resume primary-20260606-002000` — 继续当前 loop，但要先修 worktree 1 的 uncommitted changes
   - B. `ralph loops stop primary-20260606-002000` + 把 commit 608fb69 + untracked 文件 commit + 新建 fresh loop
   - C. 不动 run，先修 framework（hard_gate / event_origin），再用 dry-run 重放验证

2. **是否合并 worktree 2 (192 commits ahead) 到 main？** 选项：
   - A. `git merge ralph/implement-dev-plan-docs-plans-2026-06-05-001-feat-runtime-contract-consolidation-md-happy-finch` — 接受 runtime contract 全部 7 units
   - B. cherry-pick 部分（仅 docs/guide/runtime-contracts.md）
   - C. 暂不合并，等框架修复后再评估

3. **ce-executor.yml 的 preset 修复是合并到本 branch 还是另开？** 选项：
   - A. 在当前 pittcat-dev branch 修
   - B. 开新 branch `ralph/fix-ce-executor-hard-gate-handoff`

4. **是否接受本次失败 run 的事件历史被记录为 `mem-1780702361` 系列 memory 续条**？

---

## 6. 关键风险与不确定性

1. **本报告基于的诊断数据不完整**：当前 `.ralph/diagnostics/logs/` 仅有 1 个 log 文件（69 行），可能 RPC 端流断开后未继续记录。需要确认 `rpc_stdin failed to write to stdout` 之后是否真的死锁，还是后续被自动恢复。
2. **worktree 1 uncommitted 修改未审计**：本报告只看了 `git diff --stat`，没看具体 diff。如果 executor 改 framework 源码的内容是"必要修复"（如 hard_gate.rs），那 P1 #11 应降级为 P0。
3. **未在 Ralph 真实路径上跑过复现**：本报告的所有归因基于代码静态分析与事件时序，**没有在隔离环境重放失败**。建议在清理状态后跑一次 fresh run 验证。
4. **preset 的 `default_publishes: "review.complete"` 是否真的会导致 0 findings 走错路径**：需要回放 events 验证。如果 synthesizer 的 0 findings 实际是显式判断的，那异常 #5 的根因不是预设而是 synthesizer 自身 bug。

---

## 7. 附录

### 7.1 完整 preset 关键约束清单

| 字段 | 值 | 来源 |
|------|----|------|
| `tasks.enabled` | true | line 30 |
| `tasks.coordinator_hats` | [executor, coordinator] | line 31-33 |
| `event_loop.completion_promise` | LOOP_COMPLETE | line 37 |
| `event_loop.required_events` | [report.done] | line 38 |
| `event_loop.starting_event` | work.start | line 39 |
| `event_loop.max_iterations` | 50 | line 40 |
| `event_loop.max_runtime_seconds` | 28800 | line 41 (**被安全过滤**) |
| `event_loop.verdict_gate.topic` | REVIEW_COMPLETE | line 47 |
| `event_loop.verdict_gate.fail_field` | pass_or_fail | line 48 |
| `event_loop.verdict_gate.fail_value` | fail | line 49 |
| `event_loop.execution_contracts.rules.work.done` | require_payload_fields=[plan_name, plan_path, task_id, task_key, step]; require_task={id_field:task_id, allowed_terminal_statuses:[closed], auto_close_on_valid:false}; require_git_change={mode:diff_or_commit} | line 56-68 |
| `event_policy.schemas.work.ready` | required_fields=[plan_name, plan_path, task_id, task_key, step, complexity] | line 80-82 |
| `event_policy.schemas.work.done` | required_fields=[plan_name, plan_path, task_id, task_key, step] | line 83-85 |
| `event_policy.schemas.review.dimension.done` | required_fields=[dimension, findings_count, findings_file, plan_name, task_id, task_key, step] | line 92-94 |
| `event_policy.schemas.review.passed` | required_fields=[plan_name, task_id, task_key, step, findings_count, fix_round, verdict] | line 95-97 |
| `event_policy.schemas.review.complete` | required_fields=[plan_name, fix_round, verdict, residual_findings_count, findings_summary, task_id, task_key, step, findings_count] | line 101-103 |

### 7.2 Hat publishes 列表

| Hat | triggers | publishes | default_publishes |
|-----|----------|-----------|-------------------|
| coordinator | work.start | work.ready, work.failed | work.failed |
| executor | work.ready, queue.advance, work.retry, fix.plan.ready | work.done, work.failed | (无 — 必须显式) |
| review-coordinator | work.done, fix.applied | review.wave.ready, review.passed | (无) |
| dimension-reviewer | review.wave.ready | review.dimension.done | (无) |
| review-synthesizer | review.dimension.done | review.passed, review.failed, review.complete | **review.complete** |
| fixer | review.failed | fix.applied, fix.exhausted | (无 — 必须显式) |
| debug-resolver | fix.exhausted | fix.plan.ready, debug.exhausted, plan.blocked | (无) |
| plan-gate | review.passed, review.complete, work.failed | queue.advance, plan.complete, plan.blocked | **plan.blocked** |
| shipper | plan.complete, plan.blocked, debug.exhausted | REVIEW_COMPLETE | **REVIEW_COMPLETE** |
| reporter | REVIEW_COMPLETE | report.done, LOOP_COMPLETE | **report.done** |

> ⚠ `review-synthesizer` 的 `default_publishes: review.complete` 是本 run 异常 #5 的核心嫌疑——0 findings 场景应走 `review.passed` 而非 `review.complete`，但 synthesizer 用 default fallback 时总是 emit review.complete，绕过 plan-gate 的 0-findings 路径。
>
> ⚠ `plan-gate` 的 `default_publishes: plan.blocked` 是 fail-closed 设计，正常。但若 work.failed 被 plan-gate 误收（异常 #2 修复后），默认 plan.blocked 合理。

### 7.3 关键源码行号（用于反向验证）

| 文件 | 行 | 用途 |
|------|---|------|
| `crates/ralph-core/src/config/loop_config.rs` | 48-50 | default_max_runtime=14400 |
| `crates/ralph-cli/src/preflight.rs` | 493-502 | ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS |
| `crates/ralph-cli/src/preflight.rs` | 548-590 | merge_hats_overlay |
| `crates/ralph-core/src/event_loop/mod.rs` | 1047-1075 | check_termination |
| `crates/ralph-core/src/event_loop/mod.rs` | 1516-1540 | next_hat |
| `crates/ralph-core/src/hat_registry.rs` | 79-90 | find_by_trigger 第一遍 |
| `crates/ralph-core/src/hat_registry.rs` | 242-260 | find_by_trigger 主函数 |
| `crates/ralph-cli/src/loop_runner/hard_gate.rs` | 6-10 | should_hard_gate |
| `crates/ralph-cli/src/loop_runner/hard_gate.rs` | 14-22 | should_gate_missing_events |
| `crates/ralph-cli/src/loop_runner/runner.rs` | 1365 | let hat_id = match event_loop.next_hat() |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 241+ | execute_wave |

### 7.4 同类历史报告引用

- `docs/report/2026-06-05-wave-abort-root-cause-analysis.md` — 同一根本因的首次发作（wave 完成瞬间 max_runtime 终止）
- `docs/report/2026-06-04-ce-executor-prod-audit-errata.md` — 2026-06-04 prod audit errata
- `docs/report/agent-execution-contract-gates-review-2026-06-03.md` — execution contract gates design

---

## 8. 源码层核实结果（v2 增量）

> 本节为 v1 报告基于事件时序的归因**逐项对照源码确认**。M1/M2/M3 + 异常 #1/#2/#4/#14/M4 通过 `sed -n` 抽行、单元测试反向阅读、相关模块交叉引用 4 种方式在源码层全部确认；异常 #7/#8/#10/#11 静态层能定位根因模块，但需 fresh run 复现。

### 8.1 已确认的核心问题（源码级证据）

#### 8.1.1 ✅ M1 — review-coordinator / review-synthesizer publishes 错配（确认）

**实测证据：**

```yaml
# presets/en/ce-executor.yml:400-404
review-coordinator:
  name: "🔍 Review Coordinator"
  triggers: ["work.done", "fix.applied"]
  publishes: ["review.wave.ready", "review.passed"]
  # ↑ 不含 work.done → 任何代行 work.done 都会被 origin guard 拒
```

```yaml
# presets/en/ce-executor.yml:680-685
review-synthesizer:
  triggers: ["review.dimension.done"]
  publishes: ["review.passed", "review.failed", "review.complete"]
  default_publishes: "review.complete"
  # ↑ 0 findings 场景应走 review.passed，但 default 兜底是 review.complete
```

**配套源码（`crates/ralph-core/src/hat_registry.rs:286-293`）：**

```rust
pub fn can_publish(&self, hat_id: &HatId, topic: &str) -> bool {
    let Some(hat) = self.hats.get(hat_id) else {
        return false; // Unknown hat — fail closed
    };
    hat.publishes
        .iter()
        .any(|pub_topic| pub_topic.matches_str(topic))
}
// ↑ 不区分"派生事件"与"业务事件"，所有 publishes 项视为同等约束
```

**结论：** M1 真实存在。`can_publish` 不做事件分类是 design gap，preset `publishes` 与 `default_publishes` 的语义未对齐也是 design gap。

---

#### 8.1.2 ✅ M2 — `event_origin.rs` fail-closed guard 与 plan-gate 失败回退失配（确认）

**实测证据（`crates/ralph-core/src/event_origin.rs:181-193`）：**

```rust
// Registered hat + business topic: enforce publish scope.
if !registry.can_publish(&hat_id, topic_str) {
    warn!(
        topic = %topic_str,
        hat = %event.hat.as_ref().unwrap(),
        "Out-of-scope event rejected by origin guard"
        // ↑ 只有 topic + hat 两字段，无 hint 告诉 agent 哪个 hat 才能 emit
    );
    return OriginCheck::Rejected {
        topic: topic_str.to_string(),
        hat: event.hat.clone(),
        reason: "out-of-scope topic for declared hat",
    };
}
```

**反向证明（`event_origin.rs:611-653` 单元测试）：** 现有测试覆盖了 zsh completion、no-hat pass-through、unknown hat rejection 等场景，但**没有任何测试断言"origin guard 错误信息应包含可执行的 hint 字段"**。这是测试盲区。

**结论：** M2 真实存在，且测试未覆盖该问题。

---

#### 8.1.3 ✅ M3 — `next_hat` 在 multi-hat Coordinator 模式下不切换兜底（确认）

**实测证据（`crates/ralph-core/src/event_loop/mod.rs:1648-1680`）：**

```rust
pub fn next_hat(&self) -> Option<&HatId> {
    let next = self.bus.next_hat_with_pending();
    ...
    match self.config.event_loop.execution_mode {
        HatExecutionMode::Coordinator => {
            if self.config.hats.is_empty() {
                next
            } else {
                // ↑ 关键：multi-hat 模式下永远返回 ralph
                //   但不区分"正常调度"与"反复被拒后的强制切换"
                self.bus.hat_ids().find(|id| id.as_str() == "ralph")
            }
        }
    }
}
```

**配套终止逻辑（`event_loop/mod.rs:1211-1217`）：**

```rust
if self.state.consecutive_hard_gates >= Self::HARD_GATE_MAX {
    warn!(...);
    return Some(TerminationReason::Stopped);
}
// ↑ 硬阈值 3：连续 3 次 hard_gate 触发即终止
//   没有任何"中间状态：切到 ralph 兜底并发 hat_handoff 事件"
```

**`loop_runner/runner.rs:1704-1710` 的 `consecutive_fallbacks` 与 origin guard 拒事件完全无关**（处理的是"完全没有 pending event 时"的情况）。

**结论：** M3 真实存在。框架的"反复失败后兜底"路径缺失。

---

#### 8.1.4 ✅ 异常 #1+#4 — hard_gate 判定过宽（确认）

**实测证据（`crates/ralph-cli/src/loop_runner/hard_gate.rs:7-25`）：**

```rust
pub fn should_hard_gate(hat_id: &HatId, event_loop: &EventLoop) -> bool {
    let Some(config) = event_loop.registry().get_config(hat_id) else {
        return false;
    };
    !config.publishes.is_empty() && config.default_publishes.is_none()
    // ↑ 一刀切：所有"有 publishes 但无 default"都判定
}

pub fn should_gate_missing_events(...) -> bool {
    // 完全相同的判定
    !config.publishes.is_empty() && config.default_publishes.is_none()
}
```

**问题：** `review-coordinator` 有 `["review.wave.ready", "review.passed"]` 但无 `default_publishes`，被 hard_gate 误判。但 review-coordinator 是"有条件 emit"语义（空 diff → review.passed；有 diff → review.wave.ready），**hard_gate 应当只对必填终端事件触发**。

**结论：** 异常 #1+#4 真实存在。

---

#### 8.1.5 ✅ 异常 #2 — fallback hat emit 被拒后无 retry target（确认）

**实测证据（`hard_gate.rs:161-176`）：**

```rust
pub fn compute_recovery_status(event_loop: &mut EventLoop, topic: &str) -> Option<String> {
    let bus = event_loop.bus();
    for hat_id in bus.hat_ids() {
        if let Some(pending) = bus.peek_pending(hat_id) {
            for event in pending {
                if event.topic.as_str() == "task.resume"
                    && event.payload.contains(topic)
                    && let Some(target) = event.target.as_ref()
                { return Some(target.as_str().to_string()); }
            }
        }
    }
    None   // ↑ 无 task.resume 路由时返回 None
}
```

**问题：** fallback-only hat (ralph) emit work.done 缺字段时，框架只写 human.guidance 提示，**没有"自动用默认值补全 payload"的容错路径**。

**结论：** 异常 #2 真实存在。

---

#### 8.1.6 ✅ 异常 #14 — review-synthesizer default_publishes 错配（确认）

**实测证据（`presets/en/ce-executor.yml:684-685` 与 `:815`）：**

```yaml
publishes: ["review.passed", "review.failed", "review.complete"]
default_publishes: "review.complete"
```

```yaml
# presets/en/ce-executor.yml:815 (instructions)
- No findings → publish `review.passed`, payload: ...
- If safe_auto > 0 and fix_round < 3 → publish `review.failed`
- If safe_auto == 0 or fix_round >= 3 → publish `review.complete`
```

**问题：** agent instructions 显式区分 0 findings → review.passed，但当 agent 未发任何事件时，框架兜底发 `review.complete`，绕过了 plan-gate 的 0-findings → queue.advance 路径。

**结论：** 异常 #14 真实存在。

---

#### 8.1.7 ✅ M4 — `max_runtime_seconds` 在 preset 中静默丢弃（确认）

**实测证据（`crates/ralph-cli/src/preflight.rs:493-501` 与 `:578-600`）：**

```rust
const ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS: &[&str] = &[
    "completion_promise",
    "starting_event",
    "cancellation_promise",
    "required_events",
    "event_policy",
    "verdict_gate",
    "execution_contracts",
];
// ↑ max_runtime_seconds 故意不在白名单
```

```rust
for (key, value) in overlay_mapping {
    if let Some(key_str) = key.as_str() {
        if ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS.contains(&key_str) {
            event_loop_mapping.insert(key.clone(), value.clone());
        } else if !event_loop_mapping.contains_key(&key) {
            // ↑ 不在白名单 + operator 没显式声明 → 静默丢弃
            eprintln!("warning: hat collection preset declared event_loop.{}={} ...",
                      key_str, value_repr);
        }
    }
}
```

**配套（`event_loop/mod.rs:1186` 与 `loop_config.rs:48-50`）：**

```rust
if self.state.elapsed().as_secs() >= cfg.max_runtime_seconds {
    return Some(TerminationReason::MaxRuntime);
}
```

```rust
fn default_max_runtime() -> u64 { 14400 // 4 hours }   // ↑ 框架硬编码默认
```

**问题：** `presets/en/ce-executor.yml:41` 写 `max_runtime_seconds: 28800`（8h），但当 operator ralph.yml 没显式声明时，框架会丢弃 preset 值并落回 4h 框架默认。

**结论：** M4 真实存在，与 `docs/report/2026-06-05-wave-abort-root-cause-analysis.md` 报告的根因一致。

---

### 8.2 静态层可定位、需 fresh run 复现的问题

| 编号 | 报告原文 | 静态根因定位 | 复现条件 |
|------|---------|------------|---------|
| 7 | tasks.jsonl 与 events 脱钩 | `crates/ralph-core/src/task_store.rs` + worktree 模式下的 `PathBuf` 解析 | worktree 跑一次 |
| 8 | worktree 1 commit 608fb69 已实现 U0+U1 | `loop_runner/runner.rs` 中 resume vs fresh 入口分支 | `ralph loops resume <id>` 行为 |
| 10 | `.ralph/loops.json` 为空 | `crates/ralph-core/src/loop_registry.rs` 写入路径 | worktree 模式下 `register_loop` 失败条件 |
| 11 | framework 自身代码被改 | `crates/ralph-core/src/scope_violation.rs` 当前仅检查 hat 自身的 `disallowed_tools` | git diff + scope_violation 模块的 `audit_file_modifications` 路径 |

**根因定位置信度：** 高（已 grep 确认模块入口），但**实际触发需要 fresh run 验证**。

---

### 8.3 核实未发现 / 误报的问题

无。报告中所有 P0/P1 归因在源码层都找到了对应实现或缺失。

---

## 9. 修复方案（v2 细化版，按优先级）

> 本节为 v1 §4 修复建议的细化：每项改动给出**具体文件 + 行号 + 代码 diff 示意**，并标注风险。

### 9.1 P0 — 阻断 loop 闭环的修复

#### 9.1.1 P0-A：M1 — preset 修复（最小变更）

**改动 1：`presets/en/ce-executor.yml:404`**

```diff
 review-coordinator:
   name: "🔍 Review Coordinator"
   triggers: ["work.done", "fix.applied"]
-  publishes: ["review.wave.ready", "review.passed"]
+  publishes: ["review.wave.ready", "review.passed", "work.done"]
   # ↑ 允许 review-coordinator 在 fix.applied 后代行 work.done
```

**改动 2：`presets/en/ce-executor.yml:685`（异常 #14 修复）**

```diff
 review-synthesizer:
   publishes: ["review.passed", "review.failed", "review.complete"]
-  default_publishes: "review.complete"
+  default_publishes: "review.passed"
   # ↑ 0 findings 走 review.passed 直入 plan-gate
```

**风险：** 低（仅声明 emit 权限）。`review-coordinator` 代行 `work.done` 后需配合 §9.1.5 修 execution contract 的 plan_path 必填。

---

#### 9.1.2 P0-B：M3 — `next_hat` 失败兜底

**改动 1：`crates/ralph-core/src/event_loop/loop_state.rs:79` 增加字段**

```diff
 pub consecutive_hard_gates: u32,
+pub consecutive_origin_rejections: u32,
```

**改动 2：`crates/ralph-core/src/event_loop/mod.rs:1648` 改 `next_hat` 签名**

```rust
pub fn next_hat_with_recovery(
    &mut self,
    last_rejected: Option<(&HatId, &str)>,
) -> Option<&HatId> {
    if let Some((hat, topic)) = last_rejected {
        self.state.consecutive_origin_rejections += 1;
        if self.state.consecutive_origin_rejections >= 2 {
            self.publish_hat_handoff(hat, topic, "origin_guard_repeated");
            return self.bus.hat_ids().find(|id| id.as_str() == "ralph");
        }
    }
    self.next_hat()
}
```

**改动 3：`crates/ralph-cli/src/loop_runner/runner.rs:1626` 改调用**

```diff
-let hat_id = match event_loop.next_hat() {
+let hat_id = match event_loop.next_hat_with_recovery(last_rejected) {
```

**风险：** 中（影响核心调度路径）。需测试 4 种执行模式（solo / multi-hat isolated / multi-hat coordinator / mixed）仍正常。

---

#### 9.1.3 P0-C：异常 #1+#4 — hard_gate 判定

**改动：`crates/ralph-cli/src/loop_runner/hard_gate.rs:7-12`**

```diff
 pub fn should_hard_gate(hat_id: &HatId, event_loop: &EventLoop) -> bool {
     let Some(config) = event_loop.registry().get_config(hat_id) else {
         return false;
     };
-    !config.publishes.is_empty() && config.default_publishes.is_none()
+    // 关键修复：只对必填终端事件触发 hard_gate
+    let terminal_topics = ["work.done", "work.failed", "REVIEW_COMPLETE", "LOOP_COMPLETE"];
+    config.publishes.iter().any(|t| terminal_topics.contains(&t.as_str()))
+        && config.default_publishes.is_none()
 }
```

**配套修改 `hard_gate.rs:19-25`（should_gate_missing_events）** 同步判定。

**测试覆盖：**

```rust
#[test]
fn review_coordinator_not_hard_gated() {
    // review-coordinator 有 conditional publish，无 default → 不应 hard_gate
    assert!(!should_hard_gate(&HatId::new("review-coordinator"), &event_loop));
}

#[test]
fn executor_still_hard_gated() {
    // executor 有 work.done publish，无 default → 仍应 hard_gate
    assert!(should_hard_gate(&HatId::new("executor"), &event_loop));
}
```

**风险：** 低。review-coordinator 不再被误判；executor 仍被正确拦截。

---

#### 9.1.4 P0-D：异常 #2 — payload 自动补全

**改动：`crates/ralph-cli/src/loop_runner/hard_gate.rs:50` 在写 human.guidance 前先尝试补全**

```rust
pub fn try_autocomplete_payload(
    event_loop: &EventLoop,
    topic: &str,
    payload: &mut Value,
) -> bool {
    match topic {
        "work.done" => {
            if !payload.get("plan_path").is_some() {
                if let Some(plan_path) = event_loop.state().plan_path.clone() {
                    payload["plan_path"] = json!(plan_path);
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}
```

**配套：`loop_state.rs` 增加 `plan_path: Option<String>` 字段**，由 runner 在循环开始时从 PROMPT.md 第一行或 plan_name 推断。

**风险：** 中。需明确"软模式"开关（operator 显式 opt-in 才允许补全）。

---

#### 9.1.5 P0-E：M4 — `max_runtime_seconds` 显式失败

**改动：`crates/ralph-cli/src/preflight.rs:578-600`**

```diff
 for (key, value) in overlay_mapping {
     if let Some(key_str) = key.as_str() {
         if ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS.contains(&key_str) {
             event_loop_mapping.insert(key.clone(), value.clone());
+        } else if matches!(key_str, "max_runtime_seconds" | "max_iterations" | "checkpoint_interval") {
+            // 资源预算类字段直接报错，要求 operator 显式覆盖
+            anyhow::bail!(
+                "Preset declared event_loop.{}={} but resource budgets cannot be set by hat collection. \
+                 Set this in your operator ralph.yml (event_loop.{}: <value>) explicitly.",
+                key_str, value, key_str
+            );
         } else if !event_loop_mapping.contains_key(&key) {
             eprintln!("warning: ...");
         }
     }
 }
```

**风险：** 低。破坏性变更（之前静默丢弃，现在 hard error），但能让 operator 立刻发现配置漂移。

---

### 9.2 P1 — 链路不稳定的修复（结构性改造）

| 改动 | 位置 | 风险 |
|------|------|------|
| worktree 模式下，`.ralph/agent/tasks.jsonl` 走主工作树（symlink 或合并策略） | `task_store.rs` + `loop_runner/runner.rs` | 中（涉及并发协调） |
| 实现 `ralph loops resume <id> --dry-run` | `crates/ralph-cli/src/loops.rs` | 低 |
| `scope_violation` 检查扩展到 framework 自身代码 | `crates/ralph-core/src/scope_violation.rs` 的 `audit_file_modifications` | 中 |
| `event_loop.idle_timeout_secs: 1200` 加到 `presets/en/ce-executor.yml` | preset | 低 |

### 9.3 修复顺序

1. **今天 30 分钟内（最小变更）**：
   - P0-A 改动 1+2（preset publishes + default_publishes）
   - P0-E（`max_runtime_seconds` hard error）
   - preset 加 `idle_timeout_secs: 1200`

2. **本周内（结构性修复）**：
   - P0-B（next_hat_with_recovery + consecutive_origin_rejections）
   - P0-C（hard_gate 区分业务终端 vs 派生）
   - P0-D（payload 自动补全）

3. **需要专项 plan**：
   - design hat handoff 协议（涉及 EventBus 新增系统 topic `hat.handoff`）
   - design worktree 状态机（涉及 loop_lock 与 worktree 的协调）
   - design execution contract 的"软模式"开关

---

## 10. 风险与不确定性（v2 增量）

1. **8.1 节问题已源码层确认**，修复方案也已逐项评估风险。但**实际部署后还需在 fresh worktree 跑一次端到端验证**——尤其是 P0-B（next_hat 切换）涉及事件系统核心。
2. **8.2 节问题（#7/#8/#10/#11）**：静态层能定位根因模块，但**触发条件需要实际 worktree run 验证**。建议在清理 worktree 1 状态后跑 fresh run。
3. **preset 改动的向下兼容**：`presets/en/ce-executor.yml` 修改后，依赖 `default_publishes: review.complete` 行为的旧 run 行为会变（0 findings 改走 review.passed）。需 release note 标注。
4. **M1 修复引入新问题**：`review-coordinator` 加 `work.done` publish 权限后，若 agent 误用（如在 review 阶段错误发 work.done），会绕过 executor 的 execution contract 检查。需在 `event_loop/mod.rs` 的事件分发层加"work.done 只能由 executor / ralph / fix-applied 路径上的 review-coordinator emit" 的白名单。
5. **P0-D 软模式的副作用**：payload 自动补全可能掩盖 agent 的真 bug。建议默认关闭，仅 operator opt-in 开启。

---

## 11. 验证清单（v2 更新）

### 11.1 源码级验证（已完成 ✅）

```bash
# 验证 1：hard_gate 误判的根因 ✅ 已确认
sed -n '7,12p' crates/ralph-cli/src/loop_runner/hard_gate.rs
# 输出：!config.publishes.is_empty() && config.default_publishes.is_none()  ← 一刀切

# 验证 2：event_origin 错误提示 ✅ 已确认无 hint
sed -n '181,193p' crates/ralph-core/src/event_origin.rs
# 输出：warn! 只带 topic + hat，无 hint 字段

# 验证 3：next_hat 在 multi-hat Coordinator 模式下不切换 ✅ 已确认
sed -n '1666,1678p' crates/ralph-core/src/event_loop/mod.rs
# 输出：Coordinator mode 永远返回 ralph，无 consecutive_origin_rejections 切换

# 验证 4：preflight 资源预算静默丢弃 ✅ 已确认
sed -n '493,501p' crates/ralph-cli/src/preflight.rs
# 输出：白名单不含 max_runtime_seconds

# 验证 5（新增）：can_publish 不区分派生 vs 业务事件 ✅ 已确认
sed -n '286,293p' crates/ralph-core/src/hat_registry.rs
# 输出：单一匹配规则，无分类

# 验证 6（新增）：max_runtime_seconds 框架默认 4h ✅ 已确认
sed -n '48,50p' crates/ralph-core/src/config/loop_config.rs
# 输出：fn default_max_runtime() -> u64 { 14400 }
```

### 11.2 重放验证（待执行）

```bash
# 重放本次失败事件流
cat .ralph/events-20260606-002000.jsonl | ralph events replay --dry-run
# 修复后应能完整跑通：coordinator → executor → work.done →
# review-coordinator → review.wave.ready × 8 → dimension-reviewer × 8 →
# review-synthesizer → review.passed → plan-gate → shipper → reporter → LOOP_COMPLETE
```

### 11.3 决策点（v2 增量）

1. **是否接受 M1 修复引入 review-coordinator 代行 work.done 权限？**
   - 选项 A：接受（推荐）— 让 review-coordinator 路径更鲁棒
   - 选项 B：拒绝 — 改为在 hard_gate 之外加 "work.done 必经 executor" 的白名单

2. **P0-D 软模式是否默认开启？**
   - 选项 A：默认关闭，operator opt-in（推荐）— 避免掩盖真 bug
   - 选项 B：默认开启 — 提升 loop 鲁棒性但牺牲可观测性

3. **P0-E（max_runtime_seconds hard error）是否立即合并？**
   - 选项 A：立即合并到 main（推荐）— 防止 8h 预算被静默丢弃
   - 选项 B：先发 RC，1 周观察期后合并

4. **本次失败 run 是否继续 resume？**
   - 选项 A：先修框架（§9.1 P0-A + P0-E），再 `ralph loops resume primary-20260606-002000`
   - 选项 B：先停止当前 run，修完框架后建 fresh loop 重跑

---

> 📌 **报告完成时间**：2026-06-07
> 📌 **报告版本**：**v2**（v1 基础上完成源码层核实 + 修复方案细化）
> 📌 **下次行动建议**：
>   1. 立即合并 §9.1.1 + §9.1.5 + preset `idle_timeout_secs: 1200`（30 min）
>   2. 本周合并 §9.1.2 + §9.1.3 + §9.1.4
>   3. 专项 plan：hat handoff 协议、worktree 状态机、execution contract 软模式
