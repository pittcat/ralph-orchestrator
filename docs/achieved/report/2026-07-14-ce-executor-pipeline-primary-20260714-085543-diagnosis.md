---
title: ce-executor-pipeline Loop `primary-20260714-085543` 运行链路诊断报告
date: 2026-07-14
type: diagnosis
loop_id: primary-20260714-085543
preset: builtin:ce-executor-pipeline
run_dir: universal-autoresearch
status: 部分偏离 — executor 拒绝假闭环并诚实发出 work.failed，operator 主动 quit 终止 loop
diagnostics_mode: MINIMAL
---

# ce-executor-pipeline Loop `primary-20260714-085543` 运行链路诊断报告

> **生成时间**: 2026-07-14
> **诊断对象**: `universal-autoresearch/.ralph/`（loop_id=primary-20260714-085543, 启动 08:55:43 → 操作员 quit 17:31:01）
> **对照 preset**: `presets/en/ce-executor-pipeline.yml`（无 `presets/schemas/ce-executor-pipeline.yml`，全部内联）
> **执行方式**: Phase 0 串行盘点 → 主 Agent 直接落盘（事件仅 3 条、`work.failed` 自带完整 reason 链与 decisions.md 旁证，无需 A∥B fan-out）
> **Diagnostics 模式**: MINIMAL（`diagnostics/2026-07-14T16-55-42/` 有 session，但无 `orchestration.jsonl` / `agent-output.jsonl`）
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **Tier C 根**: `.agents/scratchpad/` + `docs/plans/2026-07-14_105536-fix-runtime-contract-closure-plan.md`（U1-U10 严格串行）
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70（见 [confidence-rubric.md](references/confidence-rubric.md)）

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | events（current-events 解析） | ✅ | 3 | `events-20260714-085543.jsonl`（work.start → plan.ready → work.failed） |
| S | events-history 配对 | ✅ | 1 | 仅 warmup 1 行 |
| S | recovery.jsonl（workspace） | ✅ | 0 | 无拒收 |
| S | ledger.jsonl | ✅ | 2 | iter=1 (loop.batch_sync) + iter=2 (loop.batch_sync) |
| S | loops.json | ✅ | 1 | loop_id=primary-20260714-085543, pid=23766 |
| S | loop.lock | ❌ | — | lock_released（操作员 quit 后已被清理） |
| A | tasks.jsonl | ❌ | — | preset `tasks.enabled: false`，无产物 |
| A | progress.md / summary.md / handoff.md | ❌ | — | `tasks.enabled: false` 且未到终止后（quit 在 work.failed → reporter 之前） |
| B | diagnostics mode | ✅ | MINIMAL | session `2026-07-14T16-55-42` 有 `recovery.jsonl` + `trace.jsonl` + `active-activations.json` + `drift.jsonl`，**无** `orchestration.jsonl` |
| B | agent/tasks.jsonl | ❌ | — | ralph.yml 虽 `tasks.enabled: true`，但 preset `tasks.enabled: false` 覆盖；无产物 |
| B | hat-channel（executor） | ✅ | 0 字节 | `events-hat-executor-primary-20260714-081857-2.jsonl`（前次 loop 残留，本 loop 未触发 executor 子 emit） |
| B | hat-channel（reporter） | ✅ | 0 字节 | `events-hat-reporter-primary-20260714-085543-3.jsonl`（work.failed→reporter 尚未激活） |
| B | plan-baseline | ✅ | 2 sha | `plan-baseline-PROMPT.pipeline.sha` + `plan-baseline-plans-2026-07-14_105536-...sha`，均为 `0f322f13` |
| B | orphan-emit-2026-07-14T{08-20-28,09-09-10,09-24-32}.md | ✅ | 3 | 全部为 sibling-tree fixture 噪声（64 条路径均位于 `tests/fixtures/`、`skills/uni-autoresearch-*/tests/fixtures/`），**非**当前 loop 业务事件 |
| C | `.ralph/agent/decisions.md` | ✅ | 7 步 | executor 决策链完整（09:30:01 → 09:35:01） |
| C | `docs/plans/2026-07-14_105536-fix-runtime-contract-closure-plan.md` | ✅ | 919 行 | U1-U10 严格串行 contract |
| C | `execution.target` | ✅ | 1 行 | 指向当前 plan |
| C | `.ralph/review/2026-07-14_105536-fix-runtime-contract-closure-plan/` | ✅ | — | 空目录，executor 未跑到 review 阶段 |
| C | `run_dir/ralph.yml` | ✅ | — | 实际未生效（操作员用 `-c ralph.pipeline.yml -H builtin:ce-executor-pipeline` 启动，见 trace L1） |

**盲区 / 根因置信度硬顶**：
- **MINIMAL 模式硬顶**：根因置信度 ≤ 85；mechanism 类因有 `file:line`+recovery 可例外到 85；缺 agent-output.jsonl → agent 归因 ≤ 60。
- **orphan-emit 三件套**均不计入本 run 业务事件（路径全部在 fixture 树内，是 sibling-tree detector 的常态输出，不影响编排对账）。
- **plan 与 execution.target 路径不一致**：execution.target 写的是 `2026-07-14_105536-...md`（下划线），目录里实际存在两份 plan（`2026-07-14-002-refactor-hat-orchestration-plan.md` 横线版与下划线版各一）。execution.target 精确指向了下划线版。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: 部分偏离 — 编排层未发生失序或假闭环，executor 拒绝发出"伪 work.done"并诚实发出 work.failed；操作员随后主动 quit。**基座机制未失能。**
- **P0 / P1 / P2 数量**（均为 confidence≥入表门槛）: P0×1（plan/preset scope 不匹配）, P1×1（executor 单 activation 预算与 plan U1-U10 严格串行 contract 的不可调和）
- **最高优先级根因置信度**: P0-1 = **82** / 100
- **历史复发**: 计划粒度问题历史反复 — `2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md` 同类 plan 不可一次执行；本次是 ce-executor-pipeline 替代 serial 后的同型问题，结构未变

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ✅ 编排合规；OPAC 不可深审 | events 拓扑与 preset 12-hat isolated 单消费者链完全吻合；recovery.jsonl 空；唯一业务事件 work.failed 含完整 reason + 旁证 decisions.md；缺 orchestration.jsonl 无法深审 OPAC 四步 | 70（MINIMAL 模式硬顶 85） |
| Q2 | 基座机制是否正常生效？ | ✅ | current-events 指针、events-history 配对、loops.json、ledger.jsonl（iter=1,2 均为 loop.batch_sync counter_changed）、hat-channel 创建、plan-baseline 校验、recovery.jsonl 静默，全部正常 | 88 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ 编排拓扑正确，但 plan 与 preset 能力边界错位 | 12-hat isolated 链 + event_policy.topic_deny_rules 正确；work.failed→reporter 的下一棒存在但 reporter 未被激活（操作员 17:31 quit 在前） | 78 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **preset/plan 范围失配**（主因） | executor 在 decisions.md 9:30:03 / 9:35:00 / 9:35:01 三步自检后明确写"plan requires 10 strictly-serial Units with full TDD + subagent + real-Ralph probes each; single-activation context budget cannot complete"，agent 行为正确 | 82 |

### 1.3 根因一句话

> Plan 919 行 / U1-U10 严格串行 / U1 单独需重写 2700+ 行 `safe_emit.py` + 17 文件 + baseline-verifier 子 agent + 真实 Ralph CLI 探针 — 与 ce-executor-pipeline 的"主 executor 在自己 activation 内串行完成全部 Unit 并发 work.done"模型**结构性不匹配**；executor 拒绝伪造 work.done，发 work.failed，**机制诚实**；后续 reporter 应在接 work.failed 后走 `plan.blocked`/`report.done` 路径，但操作员在 17:31:01 主动 quit 阻断。**置信度 82**。

---

## 2. 执行链路对比图

### 2.1 拓扑表

| 阶段 | 期望（preset `presets/en/ce-executor-pipeline.yml:60-100`） | 实际（events-20260714-085543.jsonl） | 一致性 |
|------|---|---|---|
| Bootstrap | `work.start` from loop-bootstrap | 08:55:43 — ✅ | OK |
| Plan review | `plan-reviewer` → `plan.ready`（`topic_deny_rules` L109-131） | 09:04:51 — ✅ | OK |
| Executor | `executor` → `work.done`/`work.failed`（`topic_deny_rules` L132-157） | 09:24:05 — `work.failed`（rejected fake `work.done`） | OK（机制诚实） |
| Reporter | `reporter` 接 `work.failed` → `report.done`（`topic_deny_rules` L170-181） | **未激活** — 操作员 17:31:01 quit | **未跑完**（操作员中断，非机制失能） |

### 2.2 时间轴

```mermaid
%%{init: {'theme':'base','themeVariables':{ 'fontSize':'11px'}}}%%
gantt
    title ce-executor-pipeline / primary-20260714-085543 (08:55 → 17:31)
    dateFormat HH:mm
    axisFormat %H:%M
    section Bootstrap
      work.start (loop-bootstrap)         :done, 08:55
    section Plan review
      plan-reviewer → plan.ready          :active, 08:55, 09:05
    section Executor
      executor entry (decisions.md 9:30)  :milestone, 09:30
      executor self-audit (9:35)          :crit, 09:35, 5m
      executor → work.failed (9:24)       :crit, 09:24, 1m
    section TUI wait
      loop idle (no reporter activation)  :done, 09:25, 17:31
    section Operator quit
      TUI Action::Quit + SIGKILL          :crit, 17:31, 1m
```

注：mermaid 时间轴中 executor `09:24 work.failed` 实际早于 `09:30 decisions.md entry` 标注，因 executor 实际工作在 09:24 前完成但回填 decisions 较晚；ledger 显示 iter=1 在 09:09:10、iter=2 在 09:24:32，迭代时钟与决策时钟解耦。

### 2.3 Hat 激活窗口

| Hat | 期望位置 | 实际激活 | 备注 |
|-----|---------|---------|------|
| plan-reviewer | iter 1 | 09:04:51 emit `plan.ready` | 走通 |
| executor | iter 2 | 09:24:05 emit `work.failed` | 自检后诚实失败 |
| dim:goal-alignment..dim:adversarial | iter 3-8 | 未激活 | work.failed 后路径被 `event_policy.completion_after_terminal` 收敛 |
| review-synthesizer / fix-planner / fixer / alignment | iter 9-12 | 未激活 | 同上 |
| reporter | iter 12+ | 未激活 | 操作员 quit 阻断 |

---

## 3. 历史问题上下文

### 3.1 关联度表

| 历史报告 | 根因 | 本次相似度 | 关键差异 |
|----------|------|----------|---------|
| `2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md` | plan 范围超出 serial preset 单次执行能力 | **高** | preset 从 `ce-executor-serial` 换成 `ce-executor-pipeline`；后者允许 executor 在自己 activation 内用 subagent 分 Unit，但 single-activation 预算仍未变 |
| `2026-07-03-ce-executor-pipeline-primary-20260702-163157-diagnosis.md` | 同类 plan/preset 边界 | 中 | 本次 executor 拒绝发 work.done 的诚信度更高（无任何 sidecar 写入，无 ledger 偏差） |

### 3.2 复发对照

`ce-executor-pipeline` 自 2026-07-07-006 取代 `ce-executor-serial` 以来，至少 2 次同型 plan 失败。本次的特殊价值在于 **agent 拒绝假闭环** — 9:30:03 / 9:35:00 / 9:35:01 三步决策有完整 `decisions.md` 旁证，可作为"agent 在不可达状态下选择 work.failed 而非 work.done"的最佳实践案例保留。

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 证据缺口 |
|----|------|----------|------------|------------|----------|
| DEV-001 | plan U1-U10 strict-serial 与 preset single-activation executor 模型不匹配 | plan:211-919；executor decisions.md step 09:35:00；work.failed.payload.reason | P0 | 82 | — |
| DEV-002 | 缺 orchestration.jsonl → OPAC L2/L 不可深审 | diagnostics/2026-07-14T16-55-42/ 无 orchestration.jsonl | P1 | 65 | MINIMAL 模式必然缺失 |
| DEV-003 | 实际启动用 `ralph.pipeline.yml`（非 `ralph.yml`），ralk.yml 的 telemetry 漂移等不被本次 run 应用 | trace.jsonl L1: `child_args:"[\"-c\", \"ralph.pipeline.yml\", \"-H\", \"builtin:ce-executor-pipeline\", \"run\", \"--rpc\"]"` | P2 | 88 | — |
| DEV-004 | reporter 未被激活（操作员 quit 阻断） | trace.jsonl L7-12: `Action::Quit intercepted` → SIGKILL，17:31:01 | P2 | 90 | — |
| DEV-005 | `presets/schemas/ce-executor-pipeline.yml` 不存在；preset 使用内联 schema（preset L311 注释确认） | ls presets/schemas/ 仅 -loop / -supervisor / merge-batch / merge-loop；preset L311 注释 `# Inline payload schemas (no \`presets/schemas/\` SSOT file for this` | P2 | 90 | — |
| DEV-006 | 3 份 `orphan-emit-*.md`（08-20 / 09-09 / 09-24）全部为 sibling-tree fixture 噪声 | 64 路径全在 `tests/fixtures/`、`skills/uni-autoresearch-*/tests/fixtures/`，无业务事件 | P3（噪声 / 假阳） | 95 | — |

### 4.1 OPAC 逐 hat 审计表（MINIMAL 模式）

| Hat | O | P | A | C | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|
| plan-reviewer | ✅ | N/A | ✅ | N/A | events:L2 `plan.ready` 携带完整 `review_summary` + `resolved_baseline_sha` + `matched_uids=[]` + `missing_uids=[U1..U10]`；trace.jsonl L4 同源 | 60（MINIMAL 缺 agent-output） |
| executor | ✅ | ⚠️ | ✅ | N/A | events:L3 `work.failed` reason 含完整 U-ID 列表与决策理由；decisions.md 7 步旁证；未见 `policy-check` 痕迹（MINIMAL 不强求） | 55（MINIMAL 缺 agent-output） |

注：OPAC 模式下 L2/L（Precheck / Apply / Confirm）受 MINIMAL 封顶 70 影响；本次未发生 emit 多业务事件 / 静默写盘等可观察的 OPAC 违例，但因无 `agent-output.jsonl`，无法定论 hat 内是否严格走 `--policy-check` dry-run。**不标 P0 OPAC。**

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|----------|----------|
| **P0** | plan U1-U10 strict-serial contract 与 preset executor "单 activation 内完成全 plan" 模型不匹配 | **preset/plan 范围失配（compound: preset 60 + plan 80）** | **82** | DEV-001 | 2026-07-04-ce-executor-serial 同型 | 1（preset L60-100 + plan L211-919 + decisions.md 9:30-9:35） |
| P1 | 缺 `orchestration.jsonl` → OPAC L2 不可深审 | **mechanism（diagnostics 采集层）** | 65 | DEV-002 | 多次 MINIMAL run 同型 | 0（MINIMAL 模式必然） |
| P1 | 启动用 `ralph.pipeline.yml` 而非 `ralph.yml` → ralph.yml 漂移配置未生效 | **operator 配置错位** | 88 | DEV-003 | 首次记录 | 0 |
| P2 | reporter 未被激活 | **operator 中断** | 90 | DEV-004 | 多次 | 0 |
| P2 | `presets/schemas/ce-executor-pipeline.yml` 不存在（preset L311 用内联） | **mechanism 拓扑习惯** | 90 | DEV-005 | 多次内联 preset | 0 |

**compound 行说明（P0-1）**：
- preset 成分（executor single-activation 模式，presets/en/ce-executor-pipeline.yml:60-100, 109-131）：conf 60 — 拓扑正确且诚实，**但范围与 plan 不匹配是设计隐含的**。
- plan 成分（U1-U10 strict-serial + U1 单 Unit 跨 17 文件 + 真实 CLI 探针，docs/plans/2026-07-14_105536-fix-runtime-contract-closure-plan.md:211-919 + U1 L213-256）：conf 80 — plan 自带"必须全部 Unit 关闭才能 work.done"硬门。
- 整行 = min(60, 80) = 60；提升至 82 凭**双账本一致**（events.work.failed.reason 与 decisions.md 三步决策叙事完全吻合）+ **历史同型证据**（2026-07-04 serial run）+ **无反例**（无任何证据表明机制能容纳此 plan）。

---

## 6. 修复建议

### 6.1 短期（operator workaround）

1. **拆分 plan 为每 Unit 一份**（P0-1，conf 82）
   - 目标：让每次 ralph run 只承担 ≤ 1 个 Implementation Unit
   - 改动：在 `execution.target` 维护目录索引 `docs/plans/<date>-<topic>-chunk-<N>.md`；每次只跑 chunk
   - 预期效果：work.done 语义可行；单 activation 预算够
   - **关联置信度**: 82

2. **或：切到 `ce-executor-pipeline-loop` preset**（P0-1，conf 70）
   - 目标：loop 版允许 work.done/fix.done 触发 review-reentry，最多 6 轮
   - 改动：改 `ralph.pipeline.yml` 的 `-H builtin:ce-executor-pipeline-loop`
   - 预期效果：plan 在多轮迭代中收敛
   - 风险：loop 版对 U1-U10 strict-serial 的 step gate 仍不友好（plan §Step 1.5 skip path 要求所有 U-ID 有 commit 或 blocker，单 iter 内仍要全部 Unit 落实）
   - **关联置信度**: 70

### 6.2 中期（preset / schema / instructions）

3. **新增"plan 粒度适配"lint 规则**（P0-1，conf 65）
   - 目标：在 `preset_lint` 阶段对 plan 做粗估（行数 + Files 清单大小 + subagent 数），超过阈值警告
   - 改动：`crates/ralph-core/src/preset_lint/plan_size.rs`（新建），读取 `execution.target` plan
   - 预期效果：operator 提前知道 plan 太大
   - **关联置信度**: 65

4. **exec plan scope gate**（P0-1，conf 60）
   - 目标：executor 在 entry 时若 plan 估算 > 阈值，直接发 `plan.blocked` 而非进入 work.failed
   - 改动：executor `instructions:` 增加"若 U1 单独 > 阈值则 plan.blocked"分支
   - 风险：plan-reviewer 与 executor 的判定可能冲突
   - **关联置信度**: 60

### 6.3 长期（机制 / 底座）

5. **分离"plan 校验"与"plan 执行"为两个 hat**（P0-1，conf 55）
   - 目标：plan-execution-feasibility hat 在 plan.ready 之前先估算 plan 粒度
   - 改动：新增 hat，调整 preset 拓扑
   - **关联置信度**: 55

---

## 7. 未核实疑点（可选）

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|----------|------------|------------|----------|
| executor 是否在 self-audit 期间真正尝试写任何 sidecar？ | 35 | 缺 agent-output.jsonl；ledger.jsonl 仅 2 行 `loop.batch_sync` 无业务迭代 | trace 已查、decisions.md 已读；缺 OPAC L2 证据 |
| 实际生效的 `ralph.pipeline.yml` 是否有 telemetry 启用？ | 30 | 该文件未读取（仅在 `events-20260714-081857` 时段开始时绑定） | trace.jsonl L1 只见命令行，未见配置内容 |

---

## 提交前自检

- [x] Phase 0 盘点表在报告中（§0）
- [x] 只读了 `current-events` 指向的 events
- [x] MINIMAL 模式已声明 OPAC 降级（§4.1 表注 + §1.2 Q1）
- [x] 每条 P0/P1 在 §5 有 **置信度**；P0-1=82 ≥ 70、入表 ≥ 60
- [x] confidence<60 的候选已落入 §7（executor 写 sidecar 行为 / ralph.pipeline.yml 内容）
- [x] 未引用 ssot-guardrails 禁止项（`hat_handoff` / `review.passed` / `human.guidance` / `loop_state_snapshot.json` / `events/` `tasks/` 旧目录）
- [x] 报告在主仓 `docs/report/2026-07-14-ce-executor-pipeline-primary-20260714-085543-diagnosis.md`
