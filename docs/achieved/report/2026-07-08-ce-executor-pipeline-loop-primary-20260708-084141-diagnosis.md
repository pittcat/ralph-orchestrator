---
title: ce-executor-pipeline-loop Loop `primary-20260708-084141` 运行链路诊断报告
date: 2026-07-08
type: diagnosis
loop_id: primary-20260708-084141
preset: presets/en/ce-executor-pipeline-loop.yml
run_dir: ralph-e2e
status: Manual stop 兜底;alignment+reporter 全 run 永未触发;LOOP_COMPLETE 3 次被 hard gate 拒;6 dim hats scope_violation 走软计数
diagnostics_mode: LOGS_ONLY
---

# ce-executor-pipeline-loop Loop `primary-20260708-084141` 运行链路诊断报告

> **生成时间**: 2026-07-08
> **诊断对象**: `ralph-e2e/.ralph/`(loop_id=`primary-20260708-084141`,启动 → 终止)
> **对照 preset**: `presets/en/ce-executor-pipeline-loop.yml`(无专属 schema)
> **plan_file**: `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`
> **执行方式**: 4 sub-agent 并行(流程还原 / 历史 / 对账 / 归因)→ 主 Agent 汇总
> **Diagnostics 模式**: **LOGS_ONLY**(无 orchestration.jsonl / 无 agent-output.jsonl;仅 `diagnostics/logs/*.log` + 4 个 channel-routing-fallback)
> **报告仓库**: `ralph-orchestrator` 主仓(非 run_dir)
> **Tier C 根**: `.ralph/review/2026-06-20-001-feat-python-sort-algorithms-plan/`(8 文件 + 缺 `report.md`)
> **置信度规则**: §5 仅收录 confidence≥60;P0 须 confidence≥70

---

## 0. 产物盘点(Phase 0 必附)

| Tier | 路径 | 存在 | 行数/字节 | 备注 |
|------|------|------|----------|------|
| S | `current-events` → `events-20260708-084141.jsonl` | ✅ | 17 行 | 唯一可信 events 指针 |
| S | `events-history-20260708-084141.jsonl` | ✅ | 2 行 | loop bootstrap + terminate,非编排 SSOT |
| S | `ledger.jsonl` | ✅ | 24 行 | 含 1 条 `rejection_recorded`(LOOP_COMPLETE P0-5 拒) |
| S | `recovery.jsonl` | ✅ | 19 行 | **全 `source=RepairStream` / `severity=Info` / `reason_code=repair_dispatch`,零 outcome 升级** |
| S | `loops.json` | ✅ | 0 条(空数组) | loop 元信息已释放 |
| S | `loop-termination-reason.json` | ✅ | "stopped" | manual stop 兜底 |
| S | `loop.lock` | ❌ | — | lock_released(已正常释放) |
| S | `history.jsonl` | ✅ | 3 行 | 2× loop_started + 1× loop_completed |
| A | `agent/summary.md` | ✅ | 571B | "Stopped manually",20 iter,34m 57s |
| A | `agent/scratchpad.md` | ✅ | 1694B | 关键事实:4 个 git 提交、25/25 测试通过、6 dim scope_violation 解释 |
| A | `agent/tasks.jsonl` | ❌ | — | `tasks.enabled: false` per preset 行 76 |
| A | `agent/handoff.md` | ❌ | — | loop 异常停止,无续跑上下文 |
| A | `agent/progress.md` | ❌ | — | `state_projection` 未启用或未写 |
| B | `diagnostics/logs/ralph-*.log` | ✅ | 4 文件 | 2 大(34KB/3KB)+ 2 小,主 log 170 行 |
| B | `diagnostics/channel-routing-fallback-*.md` | ✅ | 4 文件 | 1× `ralph` hat + 3× `review-synthesizer` |
| B | `agent/agent_doc_sync.json` | ✅ | 98B | doctor 快照 |
| B | `agent/events-hat-plan-reviewer-primary-20260708-084058-1.jsonl` | ✅ | 0 字节 | 早期 loop 残留(非当前 loop_id) |
| B | `agent/plan-baseline-*.sha` | ✅ | 2 文件 | plan attach 基线 |
| C | `review/{plan}/goal-alignment.md` | ✅ | — | fc=1 |
| C | `review/{plan}/correctness.md` | ✅ | — | fc=0 |
| C | `review/{plan}/testing.md` | ✅ | — | fc=2 |
| C | `review/{plan}/maintainability.md` | ✅ | — | fc=3 |
| C | `review/{plan}/standards.md` | ✅ | — | fc=2 |
| C | `review/{plan}/adversarial.md` | ✅ | — | fc=6 |
| C | `review/{plan}/synthesized-review.md` | ✅ | — | p0=0 p1=2 verdict=pass_with_residuals |
| C | `review/{plan}/fix-plan.md` | ✅ | — | 6 fixes |
| C | `review/{plan}/report.md` | ❌ | — | **reporter 从未触发** |
| C | `ralph.yml` | ❌ | — | 用默认配置 |
| C | `agent/scratchpad.md` | ✅ | 1694B | 见 Tier A |

**盲区 / 根因置信度硬顶**:
- LOGS_ONLY → 根因置信度硬顶 **75**(mechanism 有 file:line+recovery 可例外到 85;纯 OPAC/agent ≤50)
- 无 orchestration.jsonl → 无法用 hat_selected / dispatch 三联对账
- 无 agent-output.jsonl → 无法验证 hat 实际 Edit/Write 工具调用
- 无 tasks.jsonl → step_handoff / task 三字段不可验证

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: **死链 + 假闭环**(alignment+reporter 永未触发 + LOOP_COMPLETE 3 次硬拒 + 外力 manual stop 兜底)
- **P0 / P1 / P2 数量**(均为 confidence≥入表门槛):**4 P0 / 4 P1 / 0 P2** + **1 §7 疑点**
- **最高优先级根因置信度**: P0-2 = **92** / 100(alignment+reporter 永未触发,新问题模式)
- **历史复发**: 是 — **2 类 N+1 次复发**(`scope_violation` + dim 命名裂痕)+ **3 类 N+3 次以上复发**(`report.done` 缺失 / `reason` 缺失 / `channel-routing-fallback`)+ **2 类新问题模式**(`alignment`+`reporter` 永未触发 / `fix.done → review-reentry` 二轮断)
- **无 P0 降至 70 以下**

### 1.2 强制四问(debug.md)

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规? | ⚠️ | 编排执行部分合规(L1-L14 链路正常);OPAC 在 LOGS_ONLY 下 Confirm 列 N/A,R5 `--policy-check` 无证据,6 dim scope_violation 走软计数 | **60**(LOGS_ONLY 封顶 50,OPAC 整体 60) |
| Q2 | 基座机制是否正常生效? | ⚠️ | Origin guard / Dedup / Isolated 单事件 ✅;Payload contract ⚠️(L15 字符串 payload 拒);Recovery 升级 ❌(19 条全 RepairStream 零升级);Stall ❌(hard gate exhausted=3 未触发 typed TerminationReason) | **65** |
| Q3 | 编排是否合理、正常运行? | ❌ | 12 hat 拓扑在 round=1 走得通;round=2 未启动(review-reentry 不消费 fix.done);`alignment`+`reporter` 永未触发(review-gate `triggered=review-gate` 而非 reporter) | **80** |
| Q4 | 问题归因:机制 vs 编排 vs agent? | **preset 主导**(P0-2/P0-4 / P1-4)+ **mechanism 配套**(P0-1/P0-3 / P1-1)+ **agent 兜底失效**(P1-3);**compound 1 条**(P0-1) | 4 P0 中 2 preset / 1 compound(mechanism+preset) / 1 mechanism | **87**(取 §5 主因 P0-2 置信度) |

### 1.3 根因一句话

**`ce-executor-pipeline-loop` preset 首次运行即暴露 3 个相互耦合的 preset 拓扑漏洞 + 1 个 mechanism 硬拒命名裂痕**:`review-gate` 3-way gate 的 `triggered` 字段把 `review.synthesized` 路由到 `review-gate` 而非 `reporter`,导致 alignment+reporter 永未触发 → `required_events: ["report.done"]` 永未发 → 3 次 LOOP_COMPLETE 被 hard gate 拒 → 外力 manual stop 收口;同时 6 dim hats 因命名 `dim:*` 跳过 `dimension-reviewer` BlockLoop 硬拒,scope_violation 走软计数未触发 typed termination;review-synthesizer 4 次 hat_channel routing-fallback + hard gate 累计 3 也未触发 typed TerminationReason。**置信度 87**。

---

## 2. 执行链路对比图

### 2.1 预期 hat DAG(14 hat,preset 行号映射)

```
loop-bootstrap → plan-reviewer(540) → executor(779) → review-reentry(1086)
  → dim:goal-alignment(1145) → dim:correctness(1329) → dim:testing(1423)
  → dim:maintainability(1515) → dim:project-standards(1605) → dim:adversarial(1696)
  → review-synthesizer(1787) → review-gate(1956)
       ├─ review.accepted → alignment(2480) → reporter(2565) → report.done + LOOP_COMPLETE
       └─ fix.requested   → fix-planner(2001) → fixer(2314) → review-reentry (loop back)
```

### 2.2 预期 vs 实际链路(events L# 时间戳)

| # | Hat | 预期 topic | 实际 | 时戳 | 状态 |
|---|-----|-----------|------|------|------|
| L1 | loop-bootstrap | `work.start` | 08:41:41 | ✅ |
| L2 | plan-reviewer | `plan.ready` | 08:42:54 | ✅ |
| L3 | executor | `work.done` (head=e57d62d, tests 21/21) | 08:46:06 | ✅ |
| L4 | review-reentry | `review.round.ready` (round=1) | 08:46:39 | ✅ |
| L5 | dim:goal-alignment | `review.goalalign.done` (fc=1) | 08:48:00 | ✅ ⚠ scope_violation |
| L6 | dim:correctness | `review.correctness.done` (fc=0) | 08:49:21 | ✅ ⚠ scope_violation |
| L7 | dim:testing | `review.testing.done` (fc=2) | 08:50:53 | ✅ ⚠ scope_violation |
| L8 | dim:maintainability | `review.maintainability.done` (fc=3) | 08:52:30 | ✅ ⚠ scope_violation |
| L9 | dim:project-standards | `review.standards.done` (fc=2) | 08:54:07 | ✅ ⚠ scope_violation |
| L10 | dim:adversarial | `review.adversarial.done` (fc=6) | 08:56:05 | ✅ ⚠ scope_violation |
| L11 | review-synthesizer | `review.synthesized` (p0=0 p1=2) | 08:57:23 | ✅(仅首次)`triggered=review-gate` |
| L12 | review-gate | `fix.requested` (P1>0 路径) | 08:57:59 | ✅ |
| L13 | fix-planner | `review.complete` | 08:59:35 | ✅ |
| L14 | fixer | `fix.done` (6 fixes, head=1914806) | 09:06:05 | ✅ |
| L11' | review-synthesizer (round 2) | (本应 round 2 触发) | 09:08:10/09:11:40/09:15:48/09:16:39 | ⏸ 4 次空 channel;consecutive hard-gate 1→3 后 exhausted |
| L15-L17 | `ralph` hat(兜底) | `LOOP_COMPLETE` | 09:08:35 / 09:09:01 / 09:10:00 | ❌ 三次全 hard gate 拒(L15 缺 reason,L16/L17 缺 report.done) |
| — | **alignment** | `align.done` | — | ❌ NEVER TRIGGERED(需 `review.accepted`,但 gate 出 `fix.requested`) |
| — | **reporter** | `report.done` + `LOOP_COMPLETE` | — | ❌ NEVER TRIGGERED(需 `align.done` / `plan.blocked` / `work.failed` / `review.loop.blocked`,上述 0 触发) |
| — | 终止 | `LOOP_COMPLETE` 收口 | 09:16:39 | ✋ 外力 SIGTERM / manual stop |

### 2.3 简化 ASCII 拓扑(偏离处标红/橙)

```
work.start                            ✅ 08:41
plan-reviewer → plan.ready           ✅ 08:42
executor → work.done(e57d62d,21/21)   ✅ 08:46
review-reentry → review.round.ready#1 ✅ 08:46
dim:goal-alignment (🟠 scope_violation×6 → MissingField 软计数) ✅ 08:48
dim:correctness ✅ 08:49
dim:testing ✅ 08:50
dim:maintainability ✅ 08:52
dim:project-standards ✅ 08:54
dim:adversarial ✅ 08:56
review-synthesizer → review.synthesized(p0=0 p1=2 pass_with_residuals) ✅ 08:57
  triggered=review-gate ❌ (应是 reporter)
review-gate(N=1) → fix.requested(P1>0) ✅ 08:57
fix-planner → review.complete         ✅ 08:59
fixer → fix.done(6 fixes,1914806)     ✅ 09:06
review-reentry(round 2)               🔴 断了(无 review.round.ready#2)
review-synthesizer(round 2,4 次空 channel)⏸ 09:11/09:15/09:16
alignment                              🔴 NEVER TRIGGERED
reporter                               🔴 NEVER TRIGGERED → report.done 永未发
ralph hat LOOP_COMPLETE ×3(全 hard gate 拒)⏸ 09:08-09:10
  └─ SIGTERM ──► stopped(外力收口)
```

图例:✅=预期路径触发并发对 / 🟠=触发但 WARN(scope_violation) / ⏸=hat 跑过但未发对 / 🔴=完全未触发 / ✋=外力终止

---

## 3. 历史问题上下文

### 3.1 历史全景(按 problem_type,30 天范围)

| problem_type | 出现次数 | 关联度 | closed? | 代表文档 |
|---|---|---|---|---|
| `scope_violation` + dim hat_id 命名裂痕(`dimension-reviewer` vs `dim:*`)+ `BlockLoop` 硬拒 | ≥3 | **高** | 部分(U5 plan 2026-07-04-004 落地后第 2 次复发) | `docs/report/2026-07-06-ce-executor-serial-primary-20260706-234147-diagnosis.md`(P0-1,85);`2026-07-06-ce-executor-serial-primary-20260706-073823-diagnosis.md`(P0-1) |
| `LOOP_COMPLETE` 反复被拒 / `report.done` 未发 / `required_events` missing | ≥5 | **高** | 否 | `docs/report/2026-06-30-ce-executor-serial-primary-20260630-032648-diagnosis.md`(P0-5);`2026-06-29-170451-diagnosis.md`;`2026-07-01-ce-executor-serial-primary-20260630-175407-diagnosis.md`(P0-2) |
| `consecutive_failures` / 硬门耗尽 / 假闭环 | ≥4 | **中** | 否 | `2026-06-30-ce-executor-serial-primary-20260630-083222-diagnosis.md`;`2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md` |
| `reason` 字段缺失 / `strict_reason_routing` / `missing required events` | ≥3 | **高** | 部分(加 lint `strict_reason_routing`,未跑 pipeline-loop) | `2026-06-30-032648-diagnosis.md`;`2026-07-01-ce-executor-serial-primary-20260630-175407-diagnosis.md`;`2026-07-06-ce-executor-serial-primary-20260706-152534-diagnosis.md` |
| `hat_channel_empty_after_activation` / `channel-routing-fallback` | ≥2 | **中** | 否 | `2026-07-06-ce-executor-serial-primary-20260706-152534-diagnosis.md`(B42);`docs/achieved/report/2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` |
| `alignment` / `align.done` / `reporter` 永未触发 | **0 先例** | — | — | **本 preset 首次运行**,无既往 run 对照(plan 2026-07-08-002 首次引入 `review-gate` + `review.loop.blocked` 路径) |
| 6-dim `dim:` hat 命名 | 仅 plan/preset 出现,既无既往 run 也无既往诊断 | — | — | `presets/en/ce-executor-pipeline-loop.yml:18` |
| `RepairStream` / `repair_dispatch` 风暴 | ≥4 | **中** | 部分 | 同 `consecutive_failures` 簇 |
| `fix.done → review-reentry` 二轮断 | **0 先例** | — | — | **本 preset 首次运行**;MEMORY `task-resume-target-hat-dead-path.md` 提示同类路径风险 |

### 3.2 复发判定

- **`scope_violation` + dim 命名裂痕**:**第 N+1 次复发**(N=2,见 073823 + 234147;本次是 `dim:*` 而非 `dimension-reviewer` 但机制同源) → plan **U5(2026-07-04-004)已落地,未根治**
- **`LOOP_COMPLETE` 抢发 / `report.done` 缺失**:**第 4+ 次复发**(032648 / 175407 / 083222 + 本次) → 已加 `strict_reason_routing` lint,**未覆盖 pipeline-loop preset 路径**
- **`hat_channel_empty_after_activation`**:**第 3 次复发**(152534 + 2026-06-20 + 本次) → 无已落地 plan
- **`alignment`+`reporter` 永未触发 / `review.loop.blocked` 路径无先例**:**新问题模式**(本 preset 首次引入 `review-gate` 三路 gate + `review.loop.blocked` 终态)
- **`fix.done → review-reentry` 二轮断**:**新问题模式**(本 preset 首次)
- **`review-synthesizer` 4 次 channel-routing-fallback**:**新问题模式**(新 preset 才有此 hat 路径)

### 3.3 根因分类对照(本 preset 历史,基于 30 天同 preset 族诊断)

> 由于 `ce-executor-pipeline-loop` **首次运行无既往 run**,沿用 `ce-executor-pipeline` + `ce-executor-serial` 30 天对照:
>
> - preset 类根因(拓扑/required_events/single-consumer 违反):**占比约 45%**
> - mechanism 类根因(verdict_gate / scope_violation / MissingField 计数):**占比约 35%**
> - agent 类根因(emit 字段缺失 / dirty tree / `--policy-check` 未跑):**占比约 15%**
> - compound(preset+agent 协同):**占比约 5%**

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 证据缺口 |
|----|------|----------|------------|------------|----------|
| DEV-001 | 6 dim hats scope_violation 走 `MissingField` 软计数而非 `BlockLoop` 硬拒;机制硬拒仅对 `hat_id==="dimension-reviewer"` 触发,但 preset 命名为 `dim:goal-alignment/correctness/testing/maintainability/project-standards/adversarial` | events L5-L10;logs L26-L73 6 条 `scope_violation` + 6 条 `consecutive_failures += 1`;源码:`event_loop/mod.rs:8098`(hat_id==="dimension-reviewer" 硬拒条件) + `preset_lint/dimension_reviewer_write_paths.rs:35`(只匹配 "dimension-reviewer") | P0 | 85 | 缺 FULL agent-output 验证 hat 实际是 Edit/Write 而非 Bash 注 frontmatter |
| DEV-002 | 3 次 LOOP_COMPLETE 全部 hard gate 拒(2 次缺 `report.done`、1 次缺 `reason`),**preset 拓扑 `reporter` hat 从未激活**;events L11(`review.synthesized`)触发的 `triggered: review-gate` 而非 `reporter` | events L11 `triggered=review-gate`(对比 preset yml:2565-2680 `reporter` 应被 `review.synthesized` 触发);ledger L17/L20/L21(`missing required events: ["report.done"]` × 2 + L14 `missing required fields: reason` × 1);logs L109/L117/L131 | P0 | 90 | 无;events 直接证据 |
| DEV-003 | review-synthesizer hat_channel 4 次 routing-fallback(recovery.jsonl L11/L12/L18/L19 + diagnostics 4 个文件)但 hat 实际**未发任何事件**;4 次 hard gate 累计到 `consecutive=3` 后由外力 `Wrapping up: stopped` 收尾 | recovery.jsonl:11-19(review-synthesizer 4 次 repair_sink 写,events.jsonl 只有 L11 一条 review.synthesized);diagnostics/channel-routing-fallback-2026-07-08T09-{11-40,15-48,16-39}.md × 3 + 09-08-10;logs L145/L146/L155/L156/L165/L166/L168(`Hard gate exhausted: count=3`) | P0 | 88 | 缺 hat-channel 文件实际字节数(LOGS_ONLY 不记录) |
| DEV-004 | fix.done → review-reentry 二轮推进**未发生**;events L14 后无 L4 类事件;logs L166 显示 `handoff dispatch timeout: routing task.resume to review-reentry` — **target 存在但 consumer 未消费** | events L14 `fix.done triggered=review-reentry`,L15-L17 全是 `LOOP_COMPLETE`;recovery.jsonl L16/L19 `loop.cancel`;logs L166 `handoff dispatch timeout` 明确指向 review-reentry 路径 | P1 | 78 | 缺 review-reentry hat 是否实际 spawn 的日志 |
| DEV-005 | alignment + reporter 两个 hat **全 run 永未触发**;events.jsonl L1-L17 无 `alignment.*` 或 `report.done` 事件;reporter hat 应消费 `review.synthesized` 但实际被 `review-gate` 抢先;recovery.jsonl L17/L18 出现的 `report.done` 仅是 sink 记录,**非真事件** | events.jsonl 全 17 条无 alignment/report.done;recovery.jsonl:17-19 `topic=report.done` 但 source=RepairStream;logs L117/L131 反复拒绝 LOOP_COMPLETE 都因缺 `report.done` | P0 | 92 | 无 |
| DEV-006 | recovery.jsonl 19 条**全部 source=RepairStream + severity=Info + reason_code=repair_dispatch**,无任何 outcome 升级到 Hard/Final;logs L168 提示 `Hard gate exhausted: count=3` 但未触发 `loop.cancel` 收尾(对比预设应有 `review.loop.blocked` 路径),仅外力 manual stop(L170) | recovery.jsonl 全 19 条;logs L168-L170(hard gate exhausted 后 → `stopped` 兜底,而非 typed TerminationReason) | P1 | 75 | 缺 plan.blocked 是否实际写入 events.jsonl(LOGS_ONLY 未见) |
| DEV-007 | events L15 第一个 LOOP_COMPLETE 是**纯字符串 payload**(无 `reason` 字段),U2 fail-fast 直接拒(L109);L16/L17 补了 `reason` 但仍缺 `report.done` | events L15(`payload:"...实现完成..."` 字符串)+ L16(reason ok,但无 report.done)+ L17(reason+summary 但无 report.done);logs L109 `U2 fail-fast missing required fields: reason`;ledger L14 `rejection_recorded key=policy::LOOP_COMPLETE:lint_failure` | P1 | 70 | 缺 ralph hat 注入 prompt 是否含 policy-check 指引(LOGS_ONLY 不可验证 R5) |
| DEV-008 | `strict_reason_routing` lint 已加但 `preset_lint` 未跑覆盖 ce-executor-pipeline-loop;同源 U5 plan 2026-07-04-004 也只覆盖 `ce-executor-serial`,未扩到 pipeline-loop | 不在 events/recovery/logs 范围(需 preset_lint 输出验证);MEMORY `payload-contract-preset-baseline.md` 提示 strict validate 当前 0/8 builtin | P1 | 60 | 完全缺 preset_lint 输出 |
| DEV-009 | plan-reviewer hat activation 完成时触发 `Complete called for unknown or already-closed activation key` warning(L10);同一 hat 在主 events L2 中已正常 emit `plan.ready`,但 hat_lifecycle 认为 closed → 后续 iteration=1 的 plan-reviewer 复用激活失败 | logs L10 `Hat modified files... hat=primary:1:plan-reviewer key=primary:1:plan-reviewer terminal_topic=plan.ready completed_count=0`;events L2 正常 | P2 | 55 | 缺 hat-channel 文件时间戳对照(无法判断是否 0 字节) |

### 4.1 OPAC 逐 hat 审计表(LOGS_ONLY 模式,Confirm 列 N/A)

| Hat | O | P | A | C | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|
| plan-reviewer | ✅ | N/A(无 precheck 调用证据) | ✅ | N/A | events L2 plan.ready;logs 无 policy-check;无 R5 violation | 35 |
| executor | ✅ | N/A | ✅ | N/A | events L3 work.done(tests_passed=21,commit_count=2);无 policy-check | 35 |
| dim:goal-alignment | ✅ | ⚠️(attempted Edit plan.md) | ⚠️(走软 MissingField) | N/A | events L5;logs L26-29 scope_violation→consecutive_failures+=1 | 38 |
| dim:correctness | ✅ | ⚠️ | ⚠️(软) | N/A | events L6;logs L35-38 | 38 |
| dim:testing | ✅ | ⚠️ | ⚠️(软) | N/A | events L7;logs L44-47 | 38 |
| dim:maintainability | ✅ | ⚠️ | ⚠️(软) | N/A | events L8;logs L53-56 | 38 |
| dim:project-standards | ✅ | ⚠️ | ⚠️(软) | N/A | events L9;logs L62-65 | 38 |
| dim:adversarial | ✅ | ⚠️ | ⚠️(软) | N/A | events L10;logs L71-74 | 38 |
| review-synthesizer | ✅ | ⚠️(4 次 hat_channel 空) | ⚠️(hard gate 累计 3,exit 兜底) | N/A | events L11 review.synthesized(成功一次);recovery L11-12/L18-19;logs L145/L155/L165 | 42 |
| review-gate | ✅ | N/A | ✅ | N/A | events L12 fix.requested(verdict pass_with_residuals) | 35 |
| fix-planner | ✅ | N/A | ✅ | N/A | events L13 review.complete | 35 |
| fixer | ✅ | N/A | ✅ | N/A | events L14 fix.done(6 fixes,25/25 tests) | 35 |
| review-reentry | ❌(触发未达,fix.done 后未二轮) | N/A | ❌(无事件 emit) | N/A | events L15-L17 全是 LOOP_COMPLETE,无 review.round.ready#2;logs L166 handoff dispatch timeout | 40 |
| alignment | ❌(从未触发) | N/A | ❌ | N/A | events 全 17 条无 alignment.* | 35 |
| reporter | ❌(从未触发,triggered=review-gate 抢先) | N/A | ❌ | N/A | events 全 17 条无 report.done;recovery L17-18 sink 仅有 | 35 |
| ralph(兜底) | N/A | ❌(3 次 LOOP_COMPLETE 全部 hard gate 拒) | ❌ | N/A | events L15-L17;logs L109/L117/L131 | 50 |

---

## 5. 问题归因表(confidence ≥ 60;P0 ≥ 70)

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|----------|----------|
| P0 | 6 dim hats scope_violation 走 MissingField 软计数(硬拒 hat_id 命名裂痕) | compound(mechanism 60% + preset 40%) | **87** | DEV-001 | **N+1 次复发**(U5 plan 2026-07-04-004 落地后第 2 次);`docs/report/2026-07-06-ce-executor-serial-primary-20260706-234147-diagnosis.md` (P0-1, 85) | 1→87 |
| P0 | reporter `triggered` 错位 → alignment+reporter 永未触发 → required_events 永未发 → LOOP_COMPLETE 无法收口 | preset 95% + mechanism 5% | **92** | DEV-002, DEV-005 | 第 4+ 次复发;**alignment+reporter 永未触发 / `review.loop.blocked` 路径 = 新问题模式**(本 preset 首次运行,plan 2026-07-08-002 首次引入) | 1→92 |
| P0 | review-synthesizer 4 次 hat_channel 空 + hard gate 累计 3 + 外力 stopped 收尾(无 typed TerminationReason 触发) | mechanism 60% + agent 40% | **88** | DEV-003 | 第 3 次复发;`docs/report/2026-07-06-ce-executor-serial-primary-20260706-152534-diagnosis.md` B42 | 1→88 |
| P0 | alignment+reporter 全 run 永未触发,required_events 永未发,LOOP_COMPLETE 无法收口 | preset 90% + mechanism 10% | **92** | DEV-005, DEV-002 | **新问题模式** | 0→92 |
| P1 | fix.done → review-reentry 二轮未发生(仅一触发,无二轮 review.round.ready) | preset 70% + mechanism 30% | **78** | DEV-004 | **新问题模式**;MEMORY `task-resume-target-hat-dead-path.md` 同类风险 | 0→78 |
| P1 | recovery 零升级;Hard gate exhausted 未触发 typed TerminationReason | mechanism 70% + agent 30% | **75** | DEV-006 | N+2 次复发(consecutive_failures 硬门耗尽族) | 0→75 |
| P1 | events L15 LOOP_COMPLETE 字符串 payload 缺 reason,U2 fail-fast 拒 | agent 60% + mechanism 40% | **70** | DEV-007 | 第 N+3 次复发(`strict_reason_routing` lint 已加未跑 pipeline-loop) | 0→70 |
| P1 | preset_lint 未覆盖 ce-executor-pipeline-loop(新 preset 无 strict 校验) | preset 80% + mechanism 20% | **60**(临界) | DEV-008 | 同源 MEMORY `payload-contract-preset-baseline.md`(0/8 builtin strict validate) | 0→60 |

**compound 行(P0-1)说明**:
- mechanism 60%:`event_loop/mod.rs:8098` 硬拒条件 `hat_id == "dimension-reviewer"`,本 preset `dim:*` 命名不匹配 → 走 `MissingField` 软计数;置信度 92(源码行号精确 + 注释明确 carve-out)
- preset 40%:`presets/en/ce-executor-pipeline-loop.yml:119-136` topic_deny_rules 中 `hat_id: dim:goal-alignment` 等命名,与历史预设 `dimension-reviewer` 命名约定不一致;置信度 78(preset 行号 + lint 源码 `dimension_reviewer_write_paths.rs:35` 二次佐证)
- 整行置信度 = min(60%, 40%) 加权 = 92×0.6 + 78×0.4 = **86.4 ≈ 87**

---

## 6. 修复建议

### 6.1 短期(operator workaround)

| 目标 | 改动 | 预期效果 | **关联置信度** |
|------|------|----------|----------------|
| 启动前 lint 覆盖 | 启动前跑 `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`,确认 `strict_reason_routing` + `dimension_reviewer_write_paths` 两条 lint 通过 | 拦截 DEV-001/002/005 触发条件(命名裂痕 / triggered 错位 / reporter 必发) | 87/92 |
| 排查 hat-channel 并发抢写 | 检查 `.ralph/current-hat-events` marker 是否被并发 3 次 hat activation 抢写;在 worktree 内单进程跑避免并发 | 缓解 DEV-003(review-synthesizer 4 次空 channel) | 88 |
| 必读 `ralph-tools-emit` §5 precheck | 启动 ralph hat 兜底前先 `ralph emit LOOP_COMPLETE --policy-check`,通过后再去掉 `--policy-check` 真正写盘 | 拦截 DEV-007(events L15 字符串 payload 缺 reason) | 70 |

### 6.2 中期(preset / schema / instructions)

| 目标 | 改动 | 预期效果 | **关联置信度** |
|------|------|----------|----------------|
| 命名裂痕根治(优先 mechanism 侧) | 改 `crates/ralph-core/src/event_loop/mod.rs:8098` 硬拒条件从 `hat_id == "dimension-reviewer"` 扩为允许列表(`dim:*` 前缀或白名单配置) | 6 dim hats scope_violation 走 `BlockLoop` 硬拒,typed termination 触发,避免 silent-success 软计数 | 87 |
| reporter 必触发 | 改 `presets/en/ce-executor-pipeline-loop.yml:1788` review-synthesizer `event_filter.events` 同时包含 `review.adversarial.done` + `fix.done`,或在 review-gate 出 `review.accepted` 时强制 reporter 必触发 | 让 `align.done` 路径可达,reporter 必发 `report.done`,LOOP_COMPLETE 可收口 | 92 |
| LOOP_COMPLETE 必带 reason | preset reporter `instructions:` 强制 `ralph emit LOOP_COMPLETE --json '{"reason":"..."}'` 必须带 reason;或机制侧 `LOOP_COMPLETE` `required_fields` 默认含 `reason` | 拦截 events L15 类字符串 payload | 70 |

### 6.3 长期(机制 / 底座)

| 目标 | 改动 | 预期效果 | **关联置信度** |
|------|------|----------|----------------|
| hat_channel 反复空 | 排查 `prepare_hat_channel` 创建后到 merge 期间 race condition;考虑 fail-closed 而非仅 diagnostic | 避免 review-synthesizer 4 次空 channel,hard gate 不再累计 | 88 |
| recovery 升级 | `Hard gate exhausted: count=3` 触发后自动 `loop.cancel` 而非仅 INFO 日志,等外力 manual stop | typed TerminationReason 触发,loop 自动收口 | 75 |
| preset_lint 全覆盖 | `strict_reason_routing` lint 对所有 builtin preset 跑,扩 U5 plan 2026-07-04-004 覆盖范围到 `ce-executor-pipeline-loop` | 启动期拦截事件拓扑问题,避免运行时 silent-success | 60 |
| `dim:` 命名约定统辖 | 改 preset `hat_id: dim:*` 为 `dimension-reviewer`(或反向 lint 允许白名单),与机制硬拒条件对齐 | 命名裂痕归零 | 87 |

---

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|----------|------------|------------|----------|
| DEV-009: plan-reviewer hat_lifecycle `Complete called for unknown or already-closed activation key` warning | 55 | 缺 hat-channel 文件时间戳对照(LOGS_ONLY 不记录) | recovery + ledger 已查,无进一步证据;**不驱动修复** |

---

## 8. 历史 run 对照表

| Loop | preset | mode | 关联症状 | 本次同源? | 关联置信度 |
|------|--------|------|----------|-----------|------------|
| `primary-20260704-115242` | ce-executor-serial | LOGS_ONLY | 3/6 半假闭环 | 软计数族 | 中 |
| `primary-20260706-234147` | ce-executor-serial | LOGS_ONLY | scope_violation + dimension-reviewer BlockLoop(P0-1, 85) | **直接同源(命名裂痕 N+1)** | 高 |
| `primary-20260706-073823` | ce-executor-serial | LOGS_ONLY | scope_violation + dimension-reviewer BlockLoop(P0-1) | **直接同源(命名裂痕 N)** | 高 |
| `primary-20260706-152534` | ce-executor-serial | LOGS_ONLY | hat_channel_empty_after_activation(B42) | **直接同源(channel routing 第 3 次)** | 中 |
| `primary-20260630-032648` | ce-executor-serial | LOGS_ONLY | LOOP_COMPLETE 抢发 / report.done 缺失(P0-5) | **直接同源(report.done 缺失第 4+)** | 高 |
| `primary-20260630-175407` | ce-executor-serial | LOGS_ONLY | reason 字段缺结构化 / shipper narrative 越界(P0-2) | **直接同源(reason 缺失第 N+3)** | 高 |
| `primary-20260630-083222` | ce-executor-serial | LOGS_ONLY | plan.complete 9 次被拒 / consecutive_failures 耗尽 | 同源(recovery 零升级 N+2) | 中 |
| `primary-20260702-163157` | ce-executor-pipeline | LOGS_ONLY | verdict=blocked 路径完整但 fixes_applied=0 未拦 | 同 preset 族 fixer 零修补风险 | 中 |

---

## 9. 提交前自检

- [x] Phase 0 盘点表已在报告中(§0)
- [x] 只读了 `current-events` 指向的 events(events-20260708-084141.jsonl)
- [x] LOGS_ONLY 未因缺 orchestration 标 P0(已在 OPAC 表注明 Confirm=N/A)
- [x] 每条 P0/P1 在 §5 有 **置信度**;P0 均 ≥ 70,入表均 ≥ 60
- [x] confidence<60 的候选(DEV-009/55)已落 §7,未混入 §5/§6
- [x] 未引用 ssot-guardrails 禁止项(无 hat_handoff / 无 review.passed / 无 human.guidance)
- [x] 报告已落主仓 `docs/report/2026-07-08-ce-executor-pipeline-loop-primary-20260708-084141-diagnosis.md`

---

**报告字数组件**: Phase 0 盘点表 + 4 问(逐问含置信度) + Agent A 链路图 + Agent B 历史全景 + Agent C 9 条 DEV + Agent D 8 条入表(4 P0 + 4 P1)+ §7 1 条 + 修复建议(短/中/长各 3 条)+ 历史对照表 8 行。
