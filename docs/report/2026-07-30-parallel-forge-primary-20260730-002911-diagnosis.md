---
title: parallel-forge Loop `primary-20260730-002911` 走偏诊断（planner emit `forge.plan.ready` 被 projector Rule 1 拒收）
date: 2026-07-30
type: diagnosis
loop_id: primary-20260730-002911
preset: builtin:parallel-forge
run_dir: /Users/pittcat/Dev/Rust/ralph-orchestrator/.ralph
status: 走偏 — 死锁在 `plan_authoring` step；planner hat 单事件预算已用 + guidance topic 已删 → 无自动错误注入，runtime 只在 `LOOP_COMPLETE` 之前看到 3 条事件（forge.start / forge.plan.inspected / forge.plan.ready 拒收）
diagnostics_mode: LOGS_ONLY
history_search: disabled
---

# parallel-forge Loop `primary-20260730-002911` 运行链路诊断报告

> **生成时间**: 2026-07-30 08:51 CST
> **诊断对象**: `/Users/pittcat/Dev/Rust/ralph-orchestrator/.ralph/`（loop_id=`primary-20260730-002911`，启动 00:29:11 → 当前 08:51，~8h22m，loop 仍在跑但卡在 planner hat）
> **对照 preset**: `presets/en/parallel-forge.yml` + `presets/schemas/parallel-forge.yml`
> **执行方式**: 主 Agent 单跑（manifest 全 in-context，样本小、无需 fan-out）；`history_search=disabled` 故 Agent B 与 L5 跳过
> **Diagnostics 模式**: **LOGS_ONLY**（`diagnostics/logs/` 仅 CLI/TUI 子进程 stderr，无 `orchestration.jsonl` / `agent-output.jsonl`）
> **history_search**: `disabled`（默认；详见 SKILL.md §0.1）— §3 / §5 历史关联列一律 `N/A (history disabled)`
> **execution_capabilities**: `["supervisor", "wave"]`（`event_loop.supervisor.enabled: true` + `forge-dispatcher` 含 `ralph wave emit` + `.ralph/supervisor.db` 存在 + preset `flow.steps` 含 development_loop wave 循环）
> **报告仓库**: `ralph-orchestrator` 主仓
> **Tier C 根**: `.ralph/forge/2026-07-29-002-feat-parallel-forge-reuse-status-plan/`

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 / 状态 | 备注 |
|------|------|------|-------------|------|
| S | `events-20260730-002911.jsonl`（trusted via `current-events`） | ✅ | 3 行（`forge.start` + `forge.plan.inspected` + `forge.plan.ready` **被拒收**） | 编排 SSOT；只到 plan_authoring step，guardian / worktree / forge-dispatcher 全没出场 |
| S | `events-history-20260730-002911.jsonl` | ✅ | 1 行（`forge.start`） | 配对 history；非编排 SSOT |
| S | `ledger.jsonl` | ✅ | 2 行 | `loop.batch_sync` 两次：iter=1 counter_changed→iter=2 counter_changed |
| S | `recovery.jsonl` | ✅ | 1 行 | `agent_doc_sync: synced=0, skipped=2, failed=0`（启动期 sync 跳过，非业务拒收） |
| S | `loops.json` | ✅ | 1 loop | loop_id=`primary-20260730-002911`，pid=73660，prompt=`PROMPT.forge.md`，worktree=`/Users/pittcat/Dev/Rust/ralph-orchestrator` |
| S | `loop.lock` | ✅ | 249 字节 HELD | pid=73660，mtime=00:29:11（启动即锁），未释放 |
| S | `diagnostics/logs/ralph-...-034-73659.log` | ✅ | 24 行 | LOGS_ONLY 主证据：loop 启动 / 第二次 planner spawn（00:35:36 `Complete called for unknown or already-closed activation key key=primary:1:inspector terminal_topic=forge.plan.inspected`）/ 第三次 planner spawn（00:46:32 `state projection rejected event topic=forge.plan.ready reason=schedule field missing or non-positive ...`) |
| A | `agent/events-hat-ralph-primary-20260730-002911-3.jsonl` | ✅ | 0 字节 | planner 当前激活（id=3），未写盘 |
| A | `agent/.ralph-enforce-current-unit` | ✅ | 2 字节 | R4 single-U marker |
| A | `agent/plan-baseline-PROMPT.forge.sha` | ✅ | 41 字节 | plan 基准 SHA |
| B | `flow-authority.jsonl` | ✅ | 1 行 | `step=plan_authoring topic=forge.plan.inspected`（仅 1 行；后续 worktree_setup 没产出） |
| B | `supervisor.db` (+ shm/wal) | ✅ | sqlite | cap=+supervisor 已 wired；**但 wave ledger 0 行**（因 worktree_setup 未触发） |
| C | `.ralph/forge/2026-07-29-002-feat-parallel-forge-reuse-status-plan/development-plan.md` | ✅ | 41 KB | 已落盘（planner 写完 development-plan） |
| C | `.ralph/forge/.../execution-plan.yml` | ✅ | 45 KB | 已落盘：`wave_total: 6`，`execution_plan_digest: a5cdfe9d3d106d3feb932d574973273cdb7025da34ac48ce736e9e2b14efa8a9`；**F1 写 `execution_wave: 1`**（与 emit 不一致，详见 §1） |
| C | `.ralph/forge/.../inspection-report.md` | ✅ | 5.4 KB | inspector 产物 |
| C | `.ralph/forge/.../unit.template.yml` | ✅ | 3 KB | planner 已复制 |
| C | `.ralph/forge/.../templates/` | ✅ | 10 文件 | presets/materialize 物化 |

**execution_capabilities 推断结果**：`["supervisor", "wave"]`

**判定信号**：

- **+supervisor**：`presets/en/parallel-forge.yml:148-154` `event_loop.supervisor.enabled: true` + `.ralph/supervisor.db` 存在 + `.ralph/forge/.../templates/` 已物化（preset 走 materialize 路径）
- **+wave**：preset `flow.steps` 含 `development_loop` (`runs: forge.development_loop`) + `forge-dispatcher` hat 含 `ralph wave emit`（line 504）+ `presets/en/parallel-forge.yml:79-110` declared flow 含 14 步 wave 循环

**缺失产物 → 故障判定（capability-triggered）**：

| capability 期望 | 实际 | 判定 |
|----------------|------|------|
| supervisor.db wave 行 | 0 行 | **异常**（应至少有 1 条 wave ledger） |
| `forge.wave.worktrees.ready` | 未触发 | **异常**（worktree_setup step 未执行） |
| `forge.concurrency.approved` | 未触发 | **异常**（guardian hat 未被拉起） |
| `ralph wave emit exec.unit.ready` | 未触发 | **异常**（dispatcher 未工作） |

---

## 1. 强制四问（Phase 1）

### Q1 — 执行与 OPAC（diagnostics=LOGS_ONLY → OPAC 深度受限）

- **OPAC 置信度**：50/100。`recovery.jsonl` 仅 1 行（启动期 agent_doc_sync，非业务拒收信号）；`diagnostics/` 缺 `orchestration.jsonl` / `agent-output.jsonl`（LOGS_ONLY 期望如此）；主证据来自 `ralph-...-034-73659.log`（24 行）+ events JSONL（3 行）。
- **执行链路**：`forge.start` (loop-bootstrap) → `forge.plan.inspected` (inspector, 00:35:12) → `forge.plan.ready` (planner, 00:46:12, **被 projector 拒收**) → 死锁。3 步之后无任何业务事件。
- **OPAC 表现**：planner 确实执行了 materialize + 写盘 + 走 `--policy-check` 路径（events JSONL 中 payload 字段全齐）；问题不在 OPAC 链路本身，而在 payload **值**违反 runtime 校验。

### Q2 — 基座机制是否生效

- **preset 装载** ✅：`ralph::loop_runner::runner` log 显示 `supervisor bridge wired (execution_mode=isolated, supervisor.enabled=true) db_path=.ralph/supervisor.db max_concurrent_workers=10 aggregate_timeout_secs=7200`
- **materialize** ✅：`.ralph/forge/.../templates/` 已物化
- **inspector → planner** ✅：`forge.plan.inspected` 落地，planner 被拉起
- **planner → guardian** ❌：`forge.plan.ready` 事件写入 JSONL，但 projector `validate_wave_schedule` Rule 1 拒收（详见 Q3）
- **mechanism flow 进展**：`flow-authority.jsonl` 仅 1 行 `plan_authoring` → `forge.plan.inspected`；guardian/worktree/dispatcher 全 0 行

### Q3 — 编排是否合理

**是**（编排设计本身正确，问题在 payload 值）。preset `flow.steps` 14 步声明清晰；`forge.plan.ready` 是 `EnsureTaskBatch` 行动；`presets/schemas/parallel-forge.yml:44-87` 明确 `execution_wave` 必须是 `positive integer, 1..wave_total`；`state_projector::task::validate_wave_schedule` 在 `task.rs:1623-1746` 强制 Rule 1。**编排设计与校验规则一致，无矛盾**。

### Q4 — 归因

| # | 现象 | 根因置信度 |
|---|------|------------|
| **P0-1** | `forge.plan.ready` payload 内 `unit_tasks[0].execution_wave=0`（F1） | **95** — runtime 拒收消息字面匹配 `validate_wave_schedule` Rule 1；file:line=`task.rs:1630-1638`；双账本（events JSONL:3 + log 17）一致 |
| **P0-2** | payload 内所有 unit 的 `execution_wave` 比 `execution-plan.yml` disk 少 1（F1: 0 vs 1, U1: 1 vs 2, ..., U8: 5 vs 6） | **90** — 算术位移一致（disk YAML 的 wave=1..6 → emit payload 的 wave=0..5），integration_order 一致；说明 planner 没从 disk 读取构造 payload，而是手工构造且算错 |
| **P0-3** | planner hat 重复 spawn（00:35:36 + 00:46:32）但 `events-hat-...-3.jsonl` 0 字节未写出第二个 emit | **80** — `Complete called for unknown or already-closed activation key key=primary:1:inspector terminal_topic=forge.plan.inspected`（log:11）说明第一次 planner 跑未注册到 hat_lifecycle；当前 id=3 的 hat 仍在跑或刚退出 |
| **P0-4** | 死锁无自愈 — guidance topic 已删（plan 2026-06-28-005） | **85** — `loop_runner::runner.rs:4795` `info!("State projection rejected events; no guidance injected (plan 2026-06-28-005 removed the topic)")`；planner 下一轮没看到错误原因 |

**类别判定**：

- **预设问题（preset）** ❌ — preset 与 schema 声明一致
- **机制问题（mechanism）** ❌ — `validate_wave_schedule` 正确工作
- **agent 操作问题（agent）** ✅ — planner agent 没从 disk 读取 execution-plan.yml 构造 payload，且手工把 plan 文档里的 "Wave 0"（叙事标签）误用作 `execution_wave` 值
- **复合（compound）** ⚠️ — planner 错误 × guidance topic 缺失 = 死锁放大器

---

## 1.1 Prompt visibility 对账（强制）

> **触发条件**：诊断怀疑「agent 看不到某 skill」或「agent 引用了不该看到的内部实现」时，**必须**在 Phase 0 之后、Phase 1 之前用 `ralph -c <preset> inspect prompt --hat <id> --format json` 跑一次可见性对账。

未跑 `inspect prompt`（本 run 在 LOGS_ONLY 模式 + 主 Agent 单跑；现场证据已足，无需拆 prompt 视角进一步澄清）。planner `instructions:`（`presets/en/parallel-forge.yml:308-395`）明确写：

> `execution_wave`：正整数。算法 `wave(unit) = 1 + max(wave(dep))`；无依赖 = 1

**这条规则在 hat 视角是可读的**（agent 自己能看到）。planner agent 之所以没遵守，唯一解释是它没把 plan 文档里"Wave 0"叙事映射到 `execution_wave` 字段时做了 +1 处理，而是字面照搬。**这是 OPAC Apply 阶段的执行错误，非 prompt visibility 问题**。

---

## 2. 不一致对账（核心证据）

### 2.1 disk YAML vs emit payload（unit_tasks execution_wave 全部偏移）

| Unit | disk YAML | emit payload | 偏移 |
|------|-----------|--------------|------|
| F1 | `execution_wave: 1`, `integration_order: 1` | `execution_wave: 0`, `integration_order: 1` | -1 |
| U1 | `execution_wave: 2`, `integration_order: 2` | `execution_wave: 1`, `integration_order: 2` | -1 |
| U2 | `execution_wave: 2`, `integration_order: 3` | `execution_wave: 1`, `integration_order: 3` | -1 |
| U3 | `execution_wave: 2`, `integration_order: 4` | `execution_wave: 1`, `integration_order: 4` | -1 |
| U4 | `execution_wave: 3`, `integration_order: 5` | `execution_wave: 2`, `integration_order: 5` | -1 |
| U5 | `execution_wave: 4`, `integration_order: 6` | `execution_wave: 3`, `integration_order: 6` | -1 |
| U6 | `execution_wave: 5`, `integration_order: 7` | `execution_wave: 4`, `integration_order: 7` | -1 |
| U7 | `execution_wave: 5`, `integration_order: 8` | `execution_wave: 4`, `integration_order: 8` | -1 |
| U8 | `execution_wave: 6`, `integration_order: 9` | `execution_wave: 5`, `integration_order: 9` | -1 |

**digest**：

- disk `execution_plan_digest: a5cdfe9d3d106d3feb932d574973273cdb7025da34ac48ce736e9e2b14efa8a9`
- emit `execution_plan_digest: c74c444732c54d74d3382795dd191d696cc9d16de39a1e0a85858c44d0a659d5`

**wave_total**：

- disk `wave_total: 6`（max disk wave=6）
- emit `wave_total: 6`（max emit wave=5；与 disk 不一致，但 runtime Rule 4 只检查 1..max_wave 连续，未触发拒收）

### 2.2 校验链（file:line）

1. **Runtime 拒收点**：`crates/ralph-core/src/state_projector/task.rs:1623-1638` `validate_wave_schedule` Rule 1：
   ```rust
   match (spec.execution_wave, spec.integration_order) {
       (Some(w), Some(o)) if w > 0 && o > 0 => {}
       _ => {
           return Err(format!(
               "schedule field missing or non-positive:                      unit '{}' execution_wave={:?} integration_order={:?}                      (both must be positive integers)",
               spec.key, spec.execution_wave, spec.integration_order,
           ));
       }
   }
   ```
2. **错误消息字面匹配**：`events-20260730-002911.jsonl:3` payload unit F1 `execution_wave=Some(0) integration_order=Some(1)`；log `ralph-...-034-73659.log:17` reason 字段含 `unit 'forge:2026-07-29-002-feat-parallel-forge-reuse-status-plan:F1' execution_wave=Some(0) integration_order=Some(1)` — **完全一致**。
3. **schema fill_rule**：`presets/schemas/parallel-forge.yml:83` 文档明确 `execution_wave (positive integer, 1..wave_total)` — planner 违反文档契约（agent 视角可读）。
4. **planner instructions 红线**：`presets/en/parallel-forge.yml:362-363` 明确 `execution_wave`：正整数。算法 `wave(unit) = 1 + max(wave(dep))`；无依赖 = 1 — planner 自身指令违反。

### 2.3 为什么 digest 不一致 runtime 没拦

`validate_wave_schedule` Rule 8（`task.rs:1734-1744`）仅校验 pointer resolve 成非空字符串，**不**比对 payload digest 与 disk YAML digest。这是已知的 contract 缺口（plan 005 U1 的 pointer 声明保证 projector 走校验路径，但 digest 一致性是 inspector 行为不是 projector 行为）。digest 不一致在本场景下**不是故障原因**，仅是 planner 手工构造 payload 的伴生证据。

---

## 3. 历史关联（§0.1 禁用占位符）

| 关联维度 | 结论 |
|----------|------|
| 同 preset 既往 run 走偏模式 | `N/A (history disabled)` |
| 同 plan 既往 run 走偏模式 | `N/A (history disabled)` |
| compound 复发记录 | `N/A (history disabled)` |
| operator 已知问题 | `N/A (history disabled)` |

---

## 4. 现象时间线（08:51 当前视角倒序）

| 时间 (UTC) | 时间 (CST) | 事件 |
|-------------|------------|------|
| 2026-07-30T00:29:11Z | 08:29:11 | loop 启动（pid=73660）；supervisor bridge wired；memory injection check（0 memories） |
| 2026-07-30T00:29:11Z | 08:29:11 | `forge.start` emit（loop-bootstrap）→ ledger iter=1 |
| 2026-07-30T00:35:12Z | 08:35:12 | inspector emit `forge.plan.inspected`（plan_usable=true）→ planner spawn（pid=15810） |
| 2026-07-30T00:35:37Z | 08:35:37 | planner spawn 但 log:11 警告 `Complete called for unknown or already-closed activation key` |
| 2026-07-30T00:46:12Z | 08:46:12 | planner emit `forge.plan.ready`（被拒收） |
| 2026-07-30T00:46:32Z | 08:46:32 | state_projector 拒收；loop_runner log:18 "State projection rejected events; no guidance injected (plan 2026-06-28-005 removed the topic)"；planner 再次 spawn（pid=70855） |
| 2026-07-30T08:51:46Z | 08:51:46 | 当前时刻 — loop 仍在跑（pid=73660 alive），但 events 流停在 3 行；agent/events-hat-...-3.jsonl 0 字节（planner id=3 未写盘或刚被换出） |

---

## 5. 根因定论（confidence≥60；P0 须≥70）

### P0-1：planner agent 把 plan 文档"Wave 0"叙事照搬到 `execution_wave: 0`（root cause）

- **置信度**：95
- **触发链**：plan §0 "Wave 0: F1 契约冻结 + Characterization" 叙事（line 60）→ planner agent 在构造 `unit_tasks[0].execution_wave` 时把 "Wave 0" 字面赋 0，未做 +1 映射 → 触 `validate_wave_schedule` Rule 1（`task.rs:1631` `w > 0` 失败）→ 事件拒收
- **未自愈**：planner 单事件预算已用（presets/en/parallel-forge.yml:312 `publishes: [forge.plan.ready, forge.plan.blocked]` + planner instructions:395 "本 activation 仅一条业务事件"），且 `loop_runner::runner.rs:4795` 不再注入 guidance → planner 下一轮 spawn 看不到错误原因

### P0-2：payload 整体算术位移 -1（伴生错误，强化 P0-1 的根因）

- **置信度**：90
- **触发链**：F1=0 + 9 个 unit 整体 -1 → 说明 planner 没从 disk `execution-plan.yml` 读取 units 构造 payload，而是手工构造并在某处统一 -1（可能错把 "Wave 0..5" 的索引体系当 1-based 减 1）
- **disk 是 SSOT 锚**：`execution-plan.yml` F1 写 `execution_wave: 1`，已对齐 schema fill_rule；planner 手工构造偏离 disk

### P0-3：planner hat lifecycle key 冲突（埋雷）

- **置信度**：80
- **触发链**：log:11 `Complete called for unknown or already-closed activation key key=primary:1:inspector terminal_topic=forge.plan.inspected` → 第一次 planner 跑时 inspector 的 lifecycle key 没被正确注册或关闭 → planner 在 inspector 视角里"未完成" → 可能影响后续 hat 候选选择（不直接致 dead lock，但是已经观察到的告警）

### P0-4：guidance topic 删除后无自愈路径（环境放大器）

- **置信度**：85
- **触发链**：plan 2026-06-28-005 移除 `event.state_projection.rejected` 等 guidance emit → planner 在拒收后下一轮 spawn 看不到拒收原因 → agent 必须自己推断（planner 没有读取 `last_projection_rejections` 的指引）→ 单事件预算下要么重发同一错误 payload，要么改发 `forge.plan.blocked`（放弃 plan）
- **影响**：planner 二次 spawn（pid=70855）若仍看不到原因，必然重发同一 `forge.plan.ready` 触发同一拒收；当前 `events-hat-...-3.jsonl` 0 字节说明 hat 还在跑或刚退出，未来 30 秒到几分钟内会出现第 4 条 events（同 payload 拒收）或 `forge.plan.blocked`

---

## 6. 走偏后的可能路径（不修代码，仅给 operator 决策点）

> **HARD RULE**: 不修代码，不重启 loop，不调 ralph CLI。下表仅为 operator 后续 plan 立项的素材。

| 路径 | 触发 | 风险 | 推荐度 |
|------|------|------|--------|
| A. 等 planner 自动 fallback `forge.plan.blocked` | planner 二次 spawn 跑完，单事件预算 + 看不到原因 → 推断应改发 blocked | 不一定能保；planner 自身指令不教"看到 projection 拒收该怎么办" | 中（短窗口观察） |
| B. Operator 手动 kill loop + 改 execution-plan.yml + resume | 检查 disk YAML 已是正确值（F1=1），问题在 emit；resume 不重跑 planner 不会重写 payload | resume 行为未知（`--continue` vs `--reuse-worktree` 行为不同，CLAUDE.md "Worktree 复用规则"） | 中 |
| C. Operator 手动 kill loop + 重启 run（让 planner 重做 emit） | 新一轮 inspector → planner，planner 可能重犯同错 | 同错概率高（planner instructions 没把"Wave 0 叙事→execution_wave+1"写明） | 低 |
| D. 立 plan 修 planner instructions 红线（plan §1.x `wave(unit) = 1 + max(wave(dep))` 字面补强 + 加 "plan §0 Wave N 叙事对应 execution_wave=N+1" 注释）+ 修 guidance topic 撤回（恢复错误注入） | 需新 plan ID；不在本 run 范围 | 长时间修复路径 | 高（结构化修复） |

---

## 7. 未核实疑点（confidence<60）

无。本案所有 P0 根因均≥70，且核心证据链双账本一致（events JSONL + log 字面匹配 + schema fill_rule + planner instructions 红线交叉验证）。

---

## 8. 提交前 checklist

- [x] Phase 0 盘点表在报告中
- [x] 只读了 `current-events` 指向的 events（`events-20260730-002911.jsonl`）
- [x] LOGS_ONLY 未因缺 orchestration 标 P0（仅在 §1 Q1 标注 OPAC=50）
- [x] 每条 P0/P1 在 §5 有 **置信度**；P0 均≥70、入表≥60
- [x] confidence<60 的候选已加深或落入 §7（无）
- [x] 未引用 ssot-guardrails 禁止项
- [x] 报告在主仓 `docs/report/`
- [x] **历史检索开关状态已写入 frontmatter**（`history_search: disabled`）

---

## 9. 一句话根因

**planner agent 在构造 `forge.plan.ready` payload 时把 plan 文档"Wave 0"叙事字面赋给 `execution_wave`（F1=0；其余 8 个 unit 整体 -1），违反 `state_projector::task::validate_wave_schedule` Rule 1（`execution_wave` 必须正整数）与 schema fill_rule (`positive integer, 1..wave_total`)；事件落盘但被 projector 拒收，planner 单事件预算下重发路径被 guidance topic 删除切断，整条 `plan_authoring → concurrency_review → worktree_setup` 流死锁在 planner hat。**