# ce-executor-serial run `2026-06-10-003-...-warm-tiger` 运行链路诊断报告

> **诊断时间**:2026-06-19
> **诊断对象**:worktree `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-warm-tiger` 上的 `ce-executor-serial` preset 运行
> **诊断范围**:`worktree/.ralph/` 下的运行中间产物 + 主仓源码(`/Users/pittcat/Dev/Rust/ralph-orchestrator/`) + 主仓历史文档
> **用户报告问题**:
> 1. 编排未按预期进行
> 2. handoff 功能已开启但没看到任何相关产物
> **健康度判定**:**严重异常(已死锁)** — 关键问题全部成立
> **关键异常数量**:P0 × 3,P1 × 4,P2 × 2
> **历史重复问题**:是(merry-lotus / noble-peacock / perky-maple 三个 serial run 落入同坑)

---

## 0. 一句话通俗版(给非工程师看)

**这台"自动装配机器"完全卡死了**。

它本来应该按"读任务 → 写代码 → 4 步审查 → 修 bug → 出报告"的流水线走完整套,但实际跑到第 2 步审查就趴窝了。**主要原因有三**:

1. **流水线被卡住** — 第 2 步审查启动后,审查员"dimension-reviewer"该继续往下走时,**没人叫他**,他自己也不出声,整个流水线就冻在那里。
2. **造假自评** — 在真正开工写代码前的 14 分钟里,机器自己发了一堆"审查通过"的假报告(`review.passed`),自己给自己贴"绿灯",这些假报告是**越权伪造**的(审查员根本没让他发)。机器机制拦下了大部分,但**有一部分还是漏进文件**。
3. **handoff(交接班笔记本)整套机制形同虚设** — 配置文件里写了"开启",但**整个运行期间从来没有任何交接班笔记本被创建**,连文件夹都没建起来。下游帽子完全没看到交接信息,所以也不知道该干嘛。

**为什么会这样**:配置层说"开了",代码层也确实实现了"handoff 检查 + 拦截",但**事件根本没流经检查关卡**。同时,代理(agent)压根不知道有交接班笔记本这回事,也没有人手去调 `ralph tools handoff prepare` 这个命令去创建笔记本。

**当前状态**:loop 进程已死(进程 51148 不存在),最后一条事件停在 18:32:10 的 `human.guidance`,U2a 任务卡在 "open" 状态 24 小时+。

---

## 1. 执行链路对比图

### 1.1 预期链路(来自 `presets/en/ce-executor-serial.yml`)

```
work.start (loop-bootstrap)
  → coordinator: work.ready (U1)
    → executor: work.done (U1)
      → review-coordinator: review.dimension.ready (c)
        → dimension-reviewer: review.dimension.done (c)
      → review-coordinator: review.dimension.ready (t)
        → dimension-reviewer: review.dimension.done (t)
      → review-coordinator: review.dimension.ready (m)
        → dimension-reviewer: review.dimension.done (m)
      → review-coordinator: review.dimension.ready (r)
        → dimension-reviewer: review.dimension.done (r)
      → review-coordinator: review.dimensions.complete
        → review-synthesizer: review.passed
          → plan-gate: queue.advance + work.ready (dual-publish)
            → executor: work.done (U2a)
              ... 循环直到 plan.complete
                → shipper: REVIEW_COMPLETE
                  → reporter: report.done + LOOP_COMPLETE
```

**每条 hat→hat 边界应自动生成一个交接班笔记本**(`.ralph/agent/hat-handoff/...md`),由 gate 关卡强制要求 `handoff_path` 字段。

### 1.2 实际链路(来自 `events-20260618-173532.jsonl` 26 条事件)

```
17:35:32  work.start                                          ✅
17:39:28  work.ready (coordinator, U1)                        ✅
17:51:09  review.dimension.done (executor, testing)           ❌ 越权 + 假数据
17:51:09  review.passed × 2 (executor)                        ❌ 越权 + 假数据
17:51:10-18:05:33  5 轮 review 假自评                         ❌ 越权 + 假数据(20 个假事件)
18:09:42  work.done (executor, U1)                            ✅
18:13:02  review.dimension.ready (c)                          ✅
18:15:08  review.dimension.done (c)                           ✅
18:16:22  review.dimension.ready (t)                          ✅
18:16:54  task.resume (ralph→dimension-reviewer)              ⚠️ 但未激活
18:20:13  work.ready (ralph, U2a)                             ❌ ralph 越权发 work.ready
18:21:17  review.dimension.done (ralph, testing)              ❌ 越权 + 自补
18:23:56  task.resume (ralph→executor)                        ⚠️ 无效果
18:32:10  human.guidance (ralph→progress-steward)             ⚠️ 无响应
                ↓
        [loop 死亡,PID 51148 不存在,24h+ 无新事件]
```

### 1.3 关键偏离一览

| # | 偏离点 | 预期 | 实际 | 状态 |
|---|--------|------|------|------|
| 1 | review 序列完整性 | 4 维(c→t→m→r)全完成 | 只完成 2 维(c、t),m、r 永久 pending | ❌ |
| 2 | `review.passed` 来源 | 只能由 `review-synthesizer` 发 | executor 越权发 5 次,2 次落盘 | ❌ |
| 3 | `work.ready` 来源 | 只能由 `coordinator` 或 `plan-gate` 发 | ralph hat 越权发(L23) | ❌ |
| 4 | plan-gate 链路 | emit `(queue.advance, work.ready)` 双发 | 0 次 | ❌ |
| 5 | shipper → reporter → LOOP_COMPLETE | 必须闭环 | 全部 0 次 | ❌ |
| 6 | handoff 产物 | 多个 `## HAT HANDOFF` 块 + handoff 文件 | **0 个文件,0 个字段,0 个 prompt 注入** | ❌ 完全缺席 |

---

## 2. 历史问题上下文

> 完整知识库见 Agent B 输出。本节只列与本次强相关的历史案例。

### 2.1 直接同构案例(高度相关)

| 案例 | 症状 | 根因 | 修复 |
|------|------|------|------|
| **merry-lotus** (2026-06-17) | serial review 链卡死;dimension-reviewer publish obligation 死锁;missing_event_gate 注入 human.guidance;executor 误发 debug.step × 8 | `event_loop/rejection.rs:358 build_task_resume_payload` 缺 `reason` / `target_hat` 必填字段 | 2026-06-17-005 修复 |
| **noble-peacock** (2026-06-17) | `executor → review.passed(skip_reason=aggregate_timeout)` 越权 emit 落 events.jsonl;task.resume payload 缺 stage 字段 | 同 merry-lotus | 2026-06-17-004 修复 |
| **perky-maple** (2026-06-18) | executor 6 轮探针风暴 134 条 policy 拒绝;`fix.applied` 之后 dedup 阻断 re-review | `fix.applied` policy-accept 未 prune `review_dimension_ready_seen_keys` | 2026-06-18-004 修复(commit `bfc9ced`) |
| **zippy-sparrow** (2026-06-16) | isolated 11→4 维中途 stall;synthesizer 永不 fire;DEC-002 empty_diff bypass | `check_semantic_gates` 错归类为 InvalidFieldValue;Completeness Check 未 enforcement | 2026-06-17-003 修复(commit `44b9240`) |

### 2.2 hat_handoff 自身设计历史

- **2026-06-18-002 plan**:R1-R19 全套机制落地(`hat_handoff.enabled` 默认 false,PR-1 机制,PR-2 才对 ce-executor 开启)
- **2026-06-18-005 plan**:对抗性 review 发现 1 P0 + 3 P1,核心问题 = `policy_check.rs::check_hat_handoff_gate` 硬编码 `iteration:1, seq:0` 误杀真实 loop,以及 gate 拒收未按契约注入 `task.resume`
- **commit `19d3a60`**:U11 preset 开启 hat_handoff
- **commit `3fb385a`**:自动生成 isolated 模式下的 hat handoff 发送指令
- **commit `ae9f157`(最新)**:统一 preset opt-in 键合并

### 2.3 历史根因模式与本次对照

| 历史模式 | 本次是否复现 | 证据 |
|---------|-------------|------|
| CLI precheck 缺失 → 越权 topic 落盘 | **是** | events.jsonl L3-17 越权 review 事件 5 轮落盘 |
| task.resume payload 缺必填字段 | **是** | events.jsonl L25 task.resume 无 reason/target_hat schema |
| dedup 永久阻断 re-review | **疑似** | L22 task.resume 后 L24 由 ralph 自补 |
| progress-steward 激活后不真正 emit | **是** | L26 human.guidance 后无 progress-steward 行为 |
| handoff 拒收路径 bug | **否**(更深层问题) | 0 个 hat_handoff 拒收,因为根本没走到 gate |

---

## 3. 证据清单(全部带文件路径和事件 ID)

### 3.1 run 现场证据(全部在 worktree `.ralph/` 下)

| 证据 | 路径/位置 | 内容 |
|------|----------|------|
| 26 条事件流 | `events-20260618-173532.jsonl` L1-26 | 完整时间线 |
| 110 条 recovery 拒收 | `recovery.jsonl` | 17:51-18:05 集中 80 条 executor 越权拒收 |
| 5 维 review 仅 2 维完成 | `.agents/.../review-sequence.json` L8-11 | `correctness=done, testing=done, maintainability=pending, requirements=pending` |
| U2a task 24h+ 未推进 | `agent/tasks.jsonl` L2 | `task-1781806790-eef3 status=open` |
| progress.md 仍 Step 1 | `agent/scratchpad.md` L64-98 | ralph hat 兜底自陈"无法激活" |
| handoff 目录完全缺席 | `find .ralph -type d` | 无 `hat-handoff` 目录 |
| 0 个 handoff 文件 | `find .ralph -name "*hat-handoff*"` | 仅返回源码路径 |
| 0 条 hat_handoff reason | `recovery.jsonl` 全文 | 0 个 `hat_handoff_*` reason_code |
| 0 条 diagnostic.hat_handoff.* | `events-20260618-173532.jsonl` | 0 个相关事件 |
| 0 条 prompt 注入 | `agent/scratchpad.md` 全文 | 无 `## HAT HANDOFF` 块 |
| loop 状态 | `loops.json` | pid=51148 已不存在 |
| drift 日志 | `diagnostics/2026-06-19T01-35-31/drift.jsonl` | 5 条 recovery_outcome_update |
| 激活日志 | `diagnostics/2026-06-19T01-35-31/active-activations.json` | 异常激活序列 |

### 3.2 主仓代码证据(归因用)

| 证据 | 路径 | 关键发现 |
|------|------|----------|
| handoff 默认关闭 | `crates/ralph-core/src/config/loop_config.rs:288-293` | `#[serde(default)]` 默认 `enabled: false` |
| 注入前置条件 | `crates/ralph-core/src/event_loop/mod.rs:4545-4553` | `build_emit_instructions` 仅当 `enabled && isolated` 返回 Some |
| Runtime gate | `crates/ralph-core/src/event_loop/mod.rs:7901-8040` | fail-closed 但 recovery 0 命中说明从未被调用 |
| CLI gate 早返 | `crates/ralph-cli/src/commands/emit.rs:512-514` | `let Some(hat_id) = hat else { return Ok(()) }` — hat=None 时跳过 gate |
| HandoffIndex 构造 | `crates/ralph-core/src/workflow_contract/handoff_index.rs:125-218` | macro 边判定逻辑 |
| Macro 边判定 | `crates/ralph-core/src/hat_handoff/macro_edges.rs:30-71` | 唯一消费者 ∧ 非自环 ∧ 非豁免 |
| 写入路径 | `crates/ralph-core/src/hat_handoff/mod.rs:39` | `HAT_HANDOFF_DIR = ".ralph/agent/hat-handoff"` |
| ralph 旁路 | `crates/ralph-cli/src/commands/emit.rs:721-747` | ralph pseudo-hat 走 `RALPH_CONTROL_TOPICS` 早返路径 |
| dimension-reviewer triggers | `presets/en/ce-executor-serial.yml:1142` | `["review.dimension.ready"]` — **不含 task.resume** |
| progress-steward triggers | `presets/en/ce-executor-isolated.yml:2345` | `["loop.stalled", "human.guidance"]` — 不含 task.resume |
| review-synthesizer HARD RULE | `presets/en/ce-executor-serial.yml:1133` | "MUST NOT emit review.passed from this hat" |
| review-synthesizer 唯一 ownership | `presets/en/ce-executor-serial.yml:1369` | `publishes: ["review.passed", ...]` |
| skip_reason 合法值 | `presets/en/ce-executor-serial.yml:349-356` | `["trivial_step", "dimensions_complete"]` — aggregate_timeout 不在列 |

### 3.3 主仓文档证据

| 文档 | 路径 | 相关要点 |
|------|------|----------|
| 母舰 brainstorm | `docs/brainstorms/2026-06-18-isolated-hat-handoff-requirements.md` | handoff 默认关闭,需 preset 显式开启 |
| 计划主体 | `docs/achieved/plan/2026-06-18-002-feat-isolated-hat-handoff-plan.md` | R1-R19 完整机制 |
| review findings | `docs/achieved/plan/2026-06-18-005-fix-isolated-hat-handoff-review-findings-plan.md` | P0=P1 CLI seq 误杀 + 缺 task.resume |
| base stability | `docs/report/2026-06-18-003-base-stability-implementation-paths.md` | 基座稳定性 4 块拼图 |
| 历史同构 | `docs/report/2026-06-17-ce-executor-serial-merry-lotus-...` 等 | 3 个 serial run 落入同坑 |
| 拆分计划 | `docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md` | 本次 run 的目标计划 |

---

## 4. 问题归因表(P0 / P1 / P2)

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|--------|----------|----------|------|----------|
| **P0-A** | handoff "开启"但 run_dir 完全没有 `.ralph/agent/hat-handoff/` 目录、0 个 handoff 文件、0 个 `handoff_path` 字段、0 个 `diagnostic.hat_handoff.rejected` 事件、0 个 prompt 注入痕迹 — **整条 handoff 链路从激活到落地全部失效** | 基座机制(多重叠加):(1)CLI hat=None 早返绕过 gate,(2)ralph pseudo-hat 走 RALPH_CONTROL_TOPICS 旁路,(3)agent 缺意识未调 `ralph tools handoff prepare`,(4)worktree events.jsonl L23/25/26 直接文件 append 绕过 CLI 路径 | 详见 §3.1-3.2 全部 handoff 证据 + 主仓 `commands/emit.rs:512-514, 721-747` + `event_loop/mod.rs:4545-4553, 7901-8040` | 高(2026-06-18-005 review findings P0 同构) |
| **P0-B** | 编排未按预期进行:18:09 U1 work.done 后,18:13-18:15 走完 review.dimension(c) 单轮,18:16 启动 t 维度后 **dimension-reviewer HARD GATE 卡死** → ralph emit task.resume(target=dimension-reviewer) 但 **dimension-reviewer.triggers 不含 task.resume** → 永远无法重新激活 → progress-steward 也对 task.resume 同样无反应 | 基座机制(preset 触发器配置 + 拆分计划回归):(1) `presets/en/ce-executor-serial.yml:1142` dimension-reviewer.triggers 缺 `task.resume`,(2) `presets/en/ce-executor-isolated.yml:2345` progress-steward.triggers 缺 `task.resume`,(3) bus 分发按 subscription/triggers 路由,缺订阅=无响应 | `events.jsonl` L22 task.resume → L24 ralph 自补 + `presets/en/ce-executor-serial.yml:1142` triggers 列表 + `presets/en/ce-executor-isolated.yml:2345` triggers 列表 | 高(merry-lotus / noble-peacock 同坑) |
| **P0-C** | executor 在 17:51-18:05 期间反复越权 emit `review.dimension.done` × 5 + `review.passed` × 10,**绕开 workflow_guard precheck 写入 events.jsonl**;review-coordinator 也越权尝试 `review.passed` × 10 被拒;`skip_reason=aggregate_timeout` 在 serial preset 不合法但反复出现 | agent 执行产物(payload 格式反复错)+ 基座机制(workflow_guard 误判为 post-write 而非 pre-write 拦截):executor 缺 `## HAT HANDOFF EMIT REQUIREMENTS` 提示 + workflow_guard 落盘前未生效 | `events.jsonl` L3-17 + `recovery.jsonl` 17:51-18:05 集中 80 条 executor 越权 + `presets/en/ce-executor-serial.yml:349-356` skip_reason 白名单 | 高(2026-06-13 P1-2 incident 的 6x scope-drop 模式) |
| **P1-A** | `work.ready` 在 step-02 阶段由 `hat=ralph` 发出(events.jsonl L23),违反 preset L87 `topic_deny_rules` 隐式锁定(`ralph` 不应 emit `work.ready`)。同时 payload 字段值违反 schema:`step="step-02:u2a-scaffold"` 带冒号子标识 | preset 配置漏洞(`topic_deny_rules` 未显式锁 `ralph → work.ready`,对比 wave preset L287 缺失)+ agent 越权 | `events.jsonl` L23 + `presets/en/ce-executor-serial.yml` 缺 topic_deny_rule + wave preset L287 参照 | 中 |
| **P1-B** | review-sequence.json 仅走 2 维(c, t),m 和 r 永久 pending;`review.dimensions.complete` 永远未 emit;`review-synthesizer` 永不 fire;`plan-gate` 永不启动 | 基座机制(状态机死循环:dimension-reviewer 不发 done → review-coordinator 不发 ready → dimensions 不 complete → synthesizer 不 fire → plan-gate 不启动) | `events.jsonl` 无 `review.dimensions.complete` + `review-sequence.json` L8-11 m/r pending + `presets/en/ce-executor-serial.yml:879-883` 4 维强制 | 高(zippy-sparrow wave 11→4 同构) |
| **P1-C** | 25 条 `work.ready / review.passed / debug.step / queue.advance` 在 17:51-18:05 通过 `executor/coordinator` hat 大量 emit 但**被 CLI 全数拒收**(recovery.jsonl 有 75 条 `missing_required_field/invalid_field_value`、20 条 `isolated scope`)— 编排链路反复重置,agent 在 `payload_contract_violation` 上死循环 | agent 执行产物(payload 格式反复错)+ 基座机制(agent 缺 backpressure 信号收件箱,拒收信息没有有效反馈到 prompt) | `recovery.jsonl` 1-20+ 拒收记录 + `event_loop/mod.rs:7901-8040` gate 实现 | 中(2026-06-13 P1-2 incident 6x scope-drop 模式) |
| **P1-D** | progress-steward 被 `human.guidance` 触发后**没有 emit 任何 recovery 事件**(events.jsonl L26 后 0 业务事件),run 至今 stuck 24h+ | 基座机制(progress-steward 激活后必须**自己决定 emit 什么**,但当前实现无明确 nudge 决策路径) | `events.jsonl` L26 后 0 事件 + `presets/en/ce-executor-isolated.yml:2345` triggers + scratchpad 自陈"无响应" | 高(2026-06-16-001 U5 progress-steward 已知 gap) |
| **P2-A** | scratchpad 显示 `RALPH_CURRENT_HAT=ralph` 但指令越权;ralph hat 同时发出 `task.resume` (L25) + `human.guidance` (L26),但 task.resume 的 `target_hat=executor` 没有触发 executor 激活,human.guidance 也没触发 progress-steward 真正 emit | 设计层缺陷(ralph pseudo-hat 拼接多事件时无 backpressure,失败信号在 agent 视角无有效反馈) | scratchpad.md L64-101 + events.jsonl L25-26 | 中 |
| **P2-B** | progress.md 未推进,Step 1 标签未升级到 Step 2 — `state_projection.actions.queue.advance` 逻辑存在但**从未被触发**(无 queue.advance 业务事件) | 基座机制(无 queue.advance 触发 → 无 state projection 更新 → progress.md 永远停在初始) | progress.md L7 + `state_projection.actions.queue.advance.current_step=next_step` (L135) | 低 |

---

## 5. handoff 问题专项归因

### 5.1 用户的判断是否成立?

**完全成立**。`presets/en/ce-executor-isolated.yml` 和 `presets/en/ce-executor-serial.yml` 都有 `hat_handoff.enabled: true`,但 run_dir 里:

- ✗ 无 `.ralph/agent/hat-handoff/` 目录
- ✗ 无 handoff 文件
- ✗ 0 条事件带 `handoff_path` 字段
- ✗ 0 条 `diagnostic.hat_handoff.rejected` 或 `event.hat_handoff.inject_failed`
- ✗ 0 条 `hat_handoff_*` reason_code 在 recovery.jsonl
- ✗ 0 个 `## HAT HANDOFF EMIT REQUIREMENTS` 块在 agent prompt

**整条 handoff 链路从设计到落地全部失效**。

### 5.2 主仓代码里 handoff 的预期产物

1. **`.ralph/agent/hat-handoff/iter-NN-seq-MM-{from}-to-{to}-{topic}.md`** — 由 `ralph tools handoff prepare` 写入
2. **events.jsonl 中所有宏观边事件必须带 `handoff_path` 字段** — `REASON_CODE_HAT_HANDOFF_MISSING_PATH` 拒收
3. **下游 hat 的 prompt 顶部有 `## HAT HANDOFF` 块** — `prepend_hat_handoff_from_pending` 注入

### 5.3 实际为什么没出现?(多因素叠加)

| # | 原因 | 证据 |
|---|------|------|
| 1 | **CLI 入口绕过 hat_handoff gate**:events.jsonl L23/25/26 来自 ralph pseudo-hat,走 `RALPH_CONTROL_TOPICS` 早返路径(`commands/emit.rs:512-514`),完全绕过 `check_hat_handoff_gate` | 主仓 `commands/emit.rs:512-514, 721-747` |
| 2 | **worktree events.jsonl L2 work.ready 看似成功**:但 L23/25/26 都没带 handoff_path,如果走 CLI 应被拒但没有 recovery record — 说明这些事件是**绕开 CLI 路径**(可能 loop-runner 内部 bus.publish 或文件 append) | events.jsonl L23 + recovery.jsonl 0 命中 hat_handoff |
| 3 | **agent 不知道 handoff 要求**:`build_emit_instructions` 注入逻辑(`event_loop/mod.rs:4545-4553`)在 `enabled && isolated` 时返回 Some,但 agent 激活时是否真看到这段文本**无证据可查**(没在 scratchpad 或 log 中记录 prompt 注入) | 主仓 `hat_handoff/emit_instructions.rs:25-93` + `event_loop/mod.rs:4545-4553` |
| 4 | **agent payload 反复格式错**:`work.done plan_name mismatch: expected '2026-06-10-003...', got "p"` 反复出现,executor 在发空 test payload(plan_name="p" 是某个调试实验) — CLI 拒收后再发真值,但真值也没带 handoff_path | recovery.jsonl L1-20 |
| 5 | **macro edge gate 没真正拦下任何事件**:0 个 `diagnostic.hat_handoff.rejected` + events.jsonl 至少有 6 条宏观边 — 说明**CLI 拒收后 events.jsonl 的写入是绕过 CLI 路径的** | events.jsonl L2, L18, L19, L20, L22, L24 + recovery.jsonl 0 命中 |

### 5.4 归因四选一

| 选项 | 评估 |
|------|------|
| preset 配错 | ✗ 配置正确,`hat_handoff.enabled: true` + `execution_mode: isolated` 满足前置 |
| loop 漏发 | ✗ 实现完整,但实际运行中**事件根本没经过 gate** |
| 产物生成函数 bug | ✗ `ralph tools handoff prepare` CLI 完整实现,但**没人调用** |
| agent 行为缺意识 | ✗ 完全没意识到 handoff 要求;prompt 注入未生效(可能是 agent 完整读了 prompt 但没注意那段) |
| **CLI bypass + agent 缺意识 + 注入路径无观测 三因素叠加** | **✓ 这是最大概率根因** |

---

## 6. 修复建议(按优先级)

### P0 - 阻塞问题

#### 建议 1:强制 hat_handoff gate 在 CLI precheck 时无条件检查 macro edge
- **目标文件**:`crates/ralph-cli/src/commands/emit.rs:481-573` + `crates/ralph-core/src/hat_handoff/payload.rs::extract_handoff_path`
- **修改方向**:在 `PolicyCheckMode::Enforce` 下即使 hat=None 也运行(business topic + macro edge 检查);同步在 CLI 写入前输出 `WARN: missing handoff_path for macro edge {topic}`,让 agent 在 prompt 之外也能感知
- **预期效果**:事件即便绕过 runtime gate,CLI 也会先拒并把 reason 写 recovery.jsonl,agent 立即得到 backpressure

#### 建议 2:补齐 reviewer/steward hat 的 `task.resume` trigger
- **目标文件**:`presets/en/ce-executor-serial.yml:1142` dimension-reviewer.triggers + `presets/en/ce-executor-isolated.yml:2345` progress-steward.triggers + 所有 reviewer hats 同位置
- **修改方向**:给 reviewer/steward 的 triggers 加上 `"task.resume"`;同步在 `crates/ralph-core/src/event_loop/mod.rs` 的 runtime-injected nudge 链路明确分工:`loop.stalled` 走 runtime 自动 nudge,`task.resume` 走 ralph fallback nudge
- **预期效果**:dimension-reviewer HARD GATE 卡死时 ralph emit `task.resume(target=dimension-reviewer)` 能真正重新激活 hat;progress-steward 也对 ralph 注入的 nudge 有反应

#### 建议 3:loop runner 增加 hat-handoff 路径 stall 检测器
- **目标文件**:`crates/ralph-core/src/event_loop/mod.rs:3520`(`run_loop_impl` 调 `process_events_from_jsonl_with_waves`)+ `crates/ralph-cli/src/loop_runner/runner.rs` 拆分计划对应位置
- **修改方向**:在 `run_loop_impl` 的 stall 检测器增加显式的 hat-handoff 路径:当 `state.hat_handoff_seq == 0` 且 `state.iteration > N` 时,自动 `ralph emit human.guidance(target=coordinator)` 提醒 "macro edge 漏发 handoff_path"
- **预期效果**:即便 agent 没意识到 handoff,loop runner 会主动 nudge 帽子

### P1 - 重要偏离

#### 建议 4:在 hat instructions 顶部加入 "handoff HARD RULE"
- **目标文件**:`presets/en/ce-executor-serial.yml:1140-1180` dimension-reviewer instructions + 同位置 progress-steward instructions
- **修改方向**:instructions 顶部加"HARD RULE: 你看到 prompt 里有 `## HAT HANDOFF EMIT REQUIREMENTS` 块就必须按指令调 `ralph tools handoff prepare`";同步在 progress-steward instructions 里显式列出 emit 候选:work.ready (re-dispatch) / queue.advance (skip step) / plan.blocked (terminate),并指明每条需要的 payload schema
- **预期效果**:agent 不再需要查 preset 模板才知道流程

#### 建议 5:补齐 hat_handoff 注入可观测性
- **目标文件**:`crates/ralph-core/src/event_loop/mod.rs:4545-4553` + `crates/ralph-core/src/hat_handoff/inject.rs`
- **修改方向**:把 `build_emit_instructions` 与 `prepend_hat_handoff_from_pending` 的注入结果记录到 `LoopState.last_injected_hat_handoff_block_hash`,并在 `tracing::info!` 显式记录
- **预期效果**:让 `ralph diagnose` 能直接读出"本 turn 是否真的把 handoff 块注入到了 prompt",可观测性补齐

#### 建议 6:task.resume 触发时自动补 human.guidance
- **目标文件**:`crates/ralph-cli/src/presets.rs`(loop_runner 拆分计划对应位置)+ `crates/ralph-core/src/event_loop/mod.rs:7915`
- **修改方向**:在 `hat_handoff.enabled = true` 且 `task.resume` 被 emit 时自动补一条 `human.guidance(target=progress-steward)`,让 steward 立即看到 stall 信号
- **预期效果**:避免 ralph hat 必须自己拼接 task.resume + human.guidance 两条事件

#### 建议 7:workflow_guard 改为 pre-write 拦截
- **目标文件**:`crates/ralph-core/src/event_loop/mod.rs` workflow_guard 实现
- **修改方向**:把 isolated_scope_violation 从 post-write 标记改为 pre-write 拦截,与 CLI precheck 保持一致
- **预期效果**:executor 越权 emit 的 20 个 review 假事件不再落 events.jsonl,从源头杜绝 agent 假自评

### P2 - 体验问题

#### 建议 8:新增 hat_handoff 未生效排查 runbook
- **目标文件**:`docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md`(新增)
- **修改方向**:新增 "hat_handoff 未生效排查清单":检查 `hat_handoff.enabled == true` + `execution_mode == isolated` + 5 个 macro edge 都在 `HandoffIndex.consumer_of` 返回 `Some` + agent prompt 实际包含 `## HAT HANDOFF EMIT REQUIREMENTS`
- **预期效果**:用户/agent 故障时不再需要读 6 个文件才能定位

#### 建议 9:新增 hat-handoff artifacts audit 脚本
- **目标文件**:`scripts/audit-hat-handoff-artifacts.sh`(新增)
- **修改方向**:扫描 `.ralph/agent/hat-handoff/` 下是否有文件、判断 seq/iter 是否与 `LoopState.hat_handoff_seq` 一致;集成到 preset lint
- **预期效果**:静默无产物问题能在 CI/doctor 阶段被捕获,而不是要等用户报

---

## 7. 通俗结论

**这件事可以理解为一台工厂流水线出了 3 个连锁故障**:

1. **第 2 道质检关卡卡死** — 质检员"dimension-reviewer"被叫醒后做了一次,叫他做第二次时**系统忘了通知他**,他就坐在那发呆,后面所有工序都在等他。

2. **工人在没人监督时自己给自己盖章** — 在等质检时,工人"executor"闲着没事,自己填了 5 遍"质检通过"的单子偷偷塞进文件柜。工厂系统拦下了大部分假单子,但有 2 张**真的漏进去**了。

3. **交接班笔记本制度形同虚设** — 公司规定"每个岗位下班要写交接笔记本给下一班",但:
   - 工人压根不知道有这个规定(prompt 里没强调)
   - 就算知道也找不到笔和本子(没有 `hat-handoff/` 目录)
   - 就算写了,文件柜守卫(workflow_guard)压根没在门口查(CLI hat=None 早返)
   - 所以**一个笔记本都没写**,下一班完全不知道上一班干了啥,也就没法接手

**根本问题不是单点 bug,而是三件事同时没做对**:
- 配置层开了,但代码层守卫不严
- prompt 层没说,agent 不知道
- 可观测层没记录,出问题了才被发现

**修复优先级**:
- 最紧急:让守卫真的在门口查(CLI 强预检) + 给所有可能接班的岗位加上"接受 task.resume 信号"的能力
- 较紧急:在每个岗位的培训材料顶部写"接班时必读交接笔记本" + 记录每次注入情况便于排查
- 长期:加自动审计脚本,生产线出问题时能立即发现哪里没写笔记本

---

## 8. 报告元信息

- **诊断执行**:4 个 sub agent 并行(A 流程还原 / B 历史上下文 / C 对账分析 / D 归因修复)
- **总耗时**:约 16 分钟
- **主仓代码引用**:全在 `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/`
- **本次 run 中间产物路径**:`/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-warm-tiger/.ralph/`
- **关键约束遵守**:worktree 下的源码从未被读取,所有代码定位在主仓完成
- **下次诊断入口**:在 docs/report/ 下,文件名带 loop_id 后缀便于检索
