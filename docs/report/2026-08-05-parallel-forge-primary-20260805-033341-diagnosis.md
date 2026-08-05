---
title: "parallel-forge Loop `primary-20260805-033341` 运行链路诊断报告"
date: 2026-08-05
type: diagnosis
loop_id: primary-20260805-033341
preset: builtin:parallel-forge
run_dir: .ralph
status: 健康（success，4/4 unit 全部 ACCEPTED，2 个低严重度 P1）
diagnostics_mode: MINIMAL
history_search: disabled
execution_capabilities: [supervisor, wave]
---

# parallel-forge Loop `primary-20260805-033341` 运行链路诊断报告

> **生成时间**: 2026-08-05（loop 实际终止 2026-08-05T06:44:09Z；本报告于循环结束后由 `ralph-run-diagnosis` skill 落盘）
> **诊断对象**: `.ralph/`（loop_id=`primary-20260805-033341`，TUI 启动 → `LOOP_COMPLETE`）
> **对照 preset**: `presets/en/parallel-forge.yml` + `presets/schemas/parallel-forge.yml`
> **执行方式**: 主 Agent Phase 0 盘点 + 2 个 Explore sub-agent（流程 / 对账）+ 主 Agent 汇总；`history_search=disabled` 故未启动 Agent B、未做历史复发扫描
> **Diagnostics 模式**: MINIMAL（session `2026-08-05T11-33-41` 只有 `drift/recovery/trace`，无 `orchestration.jsonl`）
> **history_search**: `disabled`（来自 SKILL §0.1 AskUserQuestion；用户回答"不检索（推荐）"）
> **execution_capabilities**: `["supervisor", "wave"]` — `event_loop.supervisor.enabled=true`（`presets/en/parallel-forge.yml:150-152`）+ hat 含 `ralph wave emit` 指令 + `.ralph/supervisor.db` 存在 + events 含 `wave_id`（如 `w-18c8cc09bcd93488-1963-0`）
> **报告仓库**: `ralph-orchestrator` 主仓
> **Tier C 根**: `.ralph/forge/2026-08-05-001-feat-builtin-preset-introspection/`
> **置信度规则**: §5 仅入 confidence≥60；P0 须 confidence≥70

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | `.ralph/events-20260805-033341.jsonl`（current-events） | ✅ | 49 | 49 条业务事件（不含 loop.batch_sync/completion_*）；含 8 类 `forge.wave.*` + 5 类 supervisor 协调事件 |
| S | `.ralph/recovery.jsonl` | ✅ | 1 | U03 `exec.unit.done` 的 repair-stream 重投，reason=`repair_dispatch`（Info 级，非拒收） |
| A | `.ralph/agent/tasks.jsonl` | ✅ | 8 | 4 个 forge unit task + 4 个 supervisor slot task，**全部 status=closed** |
| A | `.ralph/agent/handoff.md` | ✅ | 53 | reporter 写入的 session handoff |
| A | `.ralph/ledger.jsonl` | ✅ | 39 | 末条 `loop.completion_honored`（seq 39, iter 36） |
| B | `.ralph/diagnostics/2026-08-05T11-33-41/` | ✅ | 11/1/0 | MINIMAL 模式：trace.jsonl=11, recovery=1, drift=0；无 `orchestration.jsonl` |
| B | `.ralph/supervisor.db` | ✅ | 1.1M | capability+supervisor 信号强；table 列表含 `wave_queue/wave_emissions/dispatch_records/slot_descriptors/worker_results/redrive_requests/compensation_jobs/wave_slots/wave_id_seq` |
| C | `.ralph/forge/2026-08-05-001-feat-builtin-preset-introspection/execution-plan.yml` | ✅ | 671 | plan_key=2026-08-05-001-feat-builtin-preset-introspection，4 wave 1 unit/wave |
| C | `.ralph/forge/.../inspection-report.md` | ✅ | 197 | inspection 12/12 evidence 全部 ✅，无 BLOCKED |
| C | `.ralph/forge/.../full-verification.md` | ✅ | 274 | tester 全量门禁 PASS，238/12/10/3 = 263 rust + 88 python + fmt/clippy/zsh/cli-doc-drift 全部绿 |
| C | `.ralph/forge/.../waves/{w-…,wave-2/3/4}/` | ✅ | — | 4 wave 目录（wave-1 用 hash id `w-18c8cc09bcd93488-1963-0`；wave-2/3/4 用语义 id），每目录含 `commit-map.yml` `verification.md` `settlement.md` |
| C | `.ralph/forge/.../reviews/{U01,U02,U03,U04}-review.md` + `summary.md` | ✅ | — | 4 份 review + 1 份 aggregate；summary 状态 `ACCEPTED` ×4 |
| C | `.ralph/forge/.../units/U{01,02,03,04}-completion.md` | ❌ | — | **实际不存在**（被 phase 4 reporter 在 .ralph/forge 内重写或清理），仅仓库远端 worktree 中可见；sub-agent 在 worktree 找到的 `U04-completion.md` 不在本仓根 |
| C | `docs/reports/2026-08-05-2026-08-05-001-feat-builtin-preset-introspection-manager-report.md` | ✅ | 711 | reporter 终产物；frontmatter `status=COMPLETED / final_audit=ACCEPTED` |
| C | `docs/report/.../final-audit.md` | ❌ | — | sub-agent 报告引用此路径，但仓库根 `.ralph/forge/.../final-audit.md` 不存在（被 `forge.audit.done.audit_report_path` 引用却缺文件） |

**execution_capabilities 推断结果**：见上 frontmatter `["supervisor", "wave"]`。
- supervisor：`presets/en/parallel-forge.yml:150-152` `supervisor.enabled: true` + `.ralph/supervisor.db` 文件存在 + 4 个 `supervisor:primary-…:wave-w-N:slot-0` task 出现。
- wave：4 wave 完整拓扑（`forge.wave.prepare` → `forge.wave.worktrees.ready` → `exec.unit.ready/done/complete` → `forge.wave.reviewed/integrated/verified/settled`）。

**缺失产物 → 故障判定（capability-triggered）**：
- `.ralph/supervisor.db` 缺失 → N/A（实际存在）
- events 无 `wave_id` → N/A（实际含 4 个 wave id + 8 个语义 wave id）
- `units/U0N-completion.md` 在主仓根 .ralph 不存在 → 见 §5 P1-1（artifact 清理未在 reporter 流程内登记）
- `.ralph/forge/.../final-audit.md` 缺文件但 `forge.audit.done` 引用 → 见 §5 P1-2

**盲区 / 根因置信度硬顶**：MINIMAL 模式下无 `orchestration.jsonl`，OPAC agent 行为审计 ≤ 60；机制侧靠 events + recovery + supervisor.db 对账（双账本成立），可上 75。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**：健康（success）。4/4 unit 全部 ACCEPTED，集成链 `0d6b5c21` 终态 commit 与 plan 4 wave 1 unit 拓扑严格一致；`forge.audit.done(ACCEPTED)` + `forge.report.done(COMPLETED)` + `LOOP_COMPLETE` 终态链完整。
- **P0 / P1 / P2 数量**：0 P0 / 2 P1 / 0 P2（§5 入表门槛 conf≥60）
- **最高优先级根因置信度**：P1-1 = 68 / 100
- **历史复发**：N/A (history disabled)

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ✅ | events 全链 49 条 + ledger `loop.completion_honored`，drift=0/recovery=1（仅 Info） | 78 |
| Q2 | 基座机制是否正常生效？ | ✅ | supervisor.db 4 wave 调度齐全；integrator FF/cherry-pick 与 commit 链一致 | 82 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ | 4 串行 wave（plan DAG 强约束），但 verifier 重复 emit 3 次 + summary U04 commit 引用 source 而非 integration | 70 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | 编排 + agent | 编排 = 终态 artifact 写盘未在 reporter pipeline 覆盖（缺 final-audit/units completion 落盘契约）；agent = reviewer summary 误填 source_commit | 70 |

### 1.3 根因一句话

整体成功，编排与 agent 协作在终态写盘阶段存在两处低严重度契约遗漏：reviewer summary 误把 U04 cherry-pick 的 source commit 写成 integration commit；reporter pipeline 不要求把 auditor 的 `final-audit.md` 与 `units/U0N-completion.md` 拷贝出 worktree。**(置信度 70)**

### 1.4 终态时序一致性（event-artifact chronology）

| 项目 | 内容 |
|------|------|
| **首轮终态（initial_terminal_status）** | 首轮成功：终态链 `forge.exec.development.done` → `forge.full.verified(all_required_passed=true)` → `forge.audit.done(verdict=ACCEPTED)` → `forge.report.done(status=COMPLETED)` → `LOOP_COMPLETE` 全部在 06:22:39–06:43:50 UTC 顺序落地；无任何 REJECTED/FAILED/BLOCKED 事件。 |
| **恢复状态（recovery_status）** | 无恢复：仅 1 条 `repair_dispatch`（Info）用于 U03 `exec.unit.done` 重复 payload 的重投，未触发拒收/重排。 |
| **最终代码状态（final_code_state）** | 集成分支 `forge/integration/2026-08-05-001-feat-builtin-preset-introspection` HEAD = `0d6b5c21e9139be0457621104fe823bdbcfcf18d`；4 串行 commit（`e091fa6e` U01 → `d6634ee2` U02 → `6cfa0177` U03 → `0d6b5c21` U04，cherry-pick 自 `37f3abc9`），全部非 merge。 |
| **一致性告警** | 0。无失败终态后恢复、无 mutable artifact 反向覆盖。 |

---

## 2. 执行链路对比图

事件时间轴（仅核心；ledger iteration 与 repair 已略）：

```
03:46:34 forge.plan.inspected          (inspector)
03:46:36 forge.plan.ready              (planner)
03:46:39 forge.concurrency.approved    (guardian)
03:46:42 forge.worktrees.ready         (worktree)
03:47:25 exec.unit.ready   U01
03:51:23 exec.unit.done    U01  content_hash=u01-builtin-list-inventory-foundation-serial
03:51:34 exec.unit.done    U01  content_hash=e091fa6e…   ← 真实 commit
04:01:23 exec.unit.done    U01  content_hash=e091fa6e…   ← 第三次（supervisor 重复）
04:03:06 exec.wave.complete           w-18c8cc09bcd93488-1963-0
04:06:49 forge.wave.reviewed   ACCEPTED   wave-1
04:08:32 forge.wave.integrated FF       wave-1 → e091fa6e
04:11:49 forge.wave.verified  passed=true wave-1 (passed=bool)
04:14:06 forge.wave.settled   wave-1 → task-…-62f7

04:15:20 forge.wave.prepare   wave-2-u02
04:19:08 exec.unit.ready   U02
04:27:37 exec.unit.done    U02  d6634ee2…
04:31:38 forge.wave.reviewed   ACCEPTED   wave-2
04:35:19 forge.wave.integrated FF        wave-2 → d6634ee2
04:38:08 forge.wave.verified  passed="true" (string) wave-2
04:40:58 forge.wave.settled   wave-2 → task-…-62f8

05:11:16 exec.unit.done    U03  6cfa0177…（recovery 触发 1 次 repair_dispatch，Info）
05:16:43 forge.wave.reviewed   ACCEPTED   wave-3
05:21:32 forge.wave.integrated FF        wave-3 → 6cfa0177
05:27:37 forge.wave.verified  passed="true" wave-3
05:27:41 forge.wave.verified  passed="true" wave-3   ← 重复
05:36:02 forge.wave.verified  passed="true" wave-3   ← 重复
05:38:13 forge.wave.settled   wave-3 → task-…-62f9

06:02:23 exec.unit.done    U04  0d6b5c21…
06:08:49 forge.wave.reviewed   ACCEPTED   wave-4
06:14:21 forge.wave.integrated cherry-pick 37f3abc9 → 0d6b5c21
06:20:05 forge.wave.verified  passed="true" wave-4
06:21:32 forge.wave.settled   wave-4 → task-…-62fa

06:22:39 forge.exec.development.done  completed=4 failed=0
06:30:11 forge.full.verified          all_required_passed=true
06:37:11 forge.audit.done             verdict=ACCEPTED  (audit_report_path=.ralph/forge/.../final-audit.md, file 缺失)
06:43:39 forge.report.done            status=COMPLETED
06:44:09 LOOP_COMPLETE
```

拓扑（mermaid）：

```mermaid
flowchart LR
    Start([forge.start]) --> Inspected[forge.plan.inspected]
    Inspected --> Ready[forge.plan.ready]
    Ready --> Approved[forge.concurrency.approved]
    Approved --> Wt[forge.worktrees.ready]
    Wt --> W1Prepare[forge.wave.prepare wave-1]
    W1Prepare --> W1Work[forge.wave.worktrees.ready]
    W1Work --> W1UnitReady[exec.unit.ready U01]
    W1UnitReady --> W1UnitDone[exec.unit.done U01]
    W1UnitDone --> W1FanIn[exec.wave.complete]
    W1FanIn --> W1Review[forge.wave.reviewed ACCEPTED]
    W1Review --> W1Integrate[forge.wave.integrated FF e091fa6e]
    W1Integrate --> W1Verify[forge.wave.verified passed=true]
    W1Verify --> W1Settle[forge.wave.settled]
    W1Settle --> W2Prepare[forge.wave.prepare wave-2]
    W2Prepare --> W2Settle[forge.wave.settled]
    W2Settle --> W3Prepare[forge.wave.prepare wave-3]
    W3Prepare --> W3Settle[forge.wave.settled]
    W3Settle --> W4Prepare[forge.wave.prepare wave-4]
    W4Prepare --> W4Settle[forge.wave.settled]
    W4Settle --> DevDone[forge.exec.development.done]
    DevDone --> Full[forge.full.verified all_required_passed=true]
    Full --> Audit[forge.audit.done ACCEPTED]
    Audit --> Report[forge.report.done COMPLETED]
    Report --> End([LOOP_COMPLETE])
```

---

## 3. 历史问题上下文

> **⚠️ 启用条件**：`history_search=disabled`（本次默认）。下方统一以 §0.1-占位符替代，未启动 Agent B，未扫描 `docs/report/` `docs/solutions/` `docs/plans/` `docs/brainstorms/`。

| 维度 | 状态 |
|------|------|
| 复发对照 | N/A (history disabled) |
| 关联基线 finding | N/A (history disabled) |
| §3 末尾扫描窗口行 | N/A (history disabled) |

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|----|------|----------|------------|------------|--------------|----------|
| DEV-001 | U01 `exec.unit.done` 重复 emit 3 次 | `events-…-033341.jsonl` 中 U01 三条 `exec.unit.done`（内容 hash 一次为 placeholder、两次为 `e091fa6e`） | P2（informational） | 45 | 单账本（events） | 缺 executor hat-channel（已被循环结束清理）；无 orchestration.jsonl |
| DEV-002 | Wave 3 `forge.wave.verified` 重复 3 次 | `events-…-033341.jsonl` U03 连续 3 条 `forge.wave.verified`（ts 05:27:37/41 + 05:36:02），payload 完全相同 | P1 | 65 | 双账本（events + recovery.jsonl 1 条 repair_dispatch） | 缺 verifier hat-channel |
| DEV-003 | `forge.wave.verified.passed` 字段类型漂移 | events 中 wave-1 `passed: true` (bool)，wave-2/3/4 `passed: "true"` (string) | P2 | 70 | 双账本（events 各 6 条） | 缺 schema 字段类型声明（schema 只说明语义，未约束 JSON type） |
| DEV-004 | `reviews/summary.md` U04 commit 误填 source 而非 integration | `.ralph/forge/.../reviews/summary.md:8` 写 `37f3abc9…`，但 `waves/wave-4-u04/{commit-map,integration-log,settlement}.md` 与 `forge.wave.integrated.candidate_commit_sha` 均写 `0d6b5c21…` | P1 | 78 | 双账本（summary 文本 + events/artifact）+ 集成 commit 一致 | 缺 reviewer prompt 中 "summary 写 integration commit" 的明确约束 |
| DEV-005 | `forge.audit.done.audit_report_path` 引用不存在的 `.ralph/forge/.../final-audit.md` | events `forge.audit.done` 报 `audit_report_path=.ralph/forge/.../final-audit.md`；该文件不在 .ralph 树 | P1 | 68 | 双账本（events payload + 实际文件 stat） | 需查 worktree 是否有该文件但未同步 |
| DEV-006 | `.ralph/forge/.../units/U0N-completion.md` 在主仓 .ralph 树缺失 | fd 探查 .ralph/forge/.../units 目录为空 | P1 | 60 | 单账本（fd 探查） | 缺 worktree diff；可能 executor 写盘路径只在 worktree 内 |
| DEV-007 | 4 wave 串行执行与"parallel-forge"命名错配 | plan DAG U1→U2→U3→U4 严格串行依赖，4 wave 1 unit/wave 顺序执行 | P2 | 75 | preset 拓扑 + plan YAML | 这是 plan 自身选择，非 preset 缺陷 |
| DEV-008 | `forge.report.done` 后紧接 `LOOP_COMPLETE`（双终态）| events `forge.report.done(06:43:39)` + `LOOP_COMPLETE(06:43:50)` 同 reporter 触发，间隔 11s | 契约吻合 | 80 | preset/schemas/parallel-forge.yml:739-801 + 事件序列 | 无 |

### 4.1 OPAC 逐 hat 审计表

> MINIMAL 模式无 `orchestration.jsonl`，agent 行为侧靠 `recovery.jsonl` 1 条 + events 间接对账；agent 维度置信度硬顶 60。

| Hat | O（Observe） | P（Policy） | A（Action） | C（Confirm） | 证据 | 置信度 |
|-----|--------------|-------------|-------------|--------------|------|--------|
| inspector | events `forge.plan.inspected` | `event_policy.schemas` 字段齐 | emit 1 次 plan_inspected | plan.ready 由 planner 接到 | events:1 | 70 |
| planner | events `forge.plan.ready` | plan_key / execution_wave 完整 | emit 1 次 plan.ready | forge.concurrency.approved 接到 | events:2 + execution-plan.yml | 75 |
| guardian | events `forge.concurrency.approved` | 并发审批 | emit 1 次 approved | forge.worktrees.ready 接到 | events:3 | 70 |
| worktree | events `forge.worktrees.ready` + 4 wave `forge.wave.worktrees.ready` | worktree_map 写入 | emit 5 次 | 后续 wave 接到 | events:4,7,16,25,36 + worktree-map.yml | 78 |
| forge-dispatcher | events `exec.unit.ready ×5`、`forge.wave.prepare ×3` | 调度 + supervisor 协调 | emit 多次 | exec 接到 + wave 级接到 | events:5,15,24,35 + ledger | 80 |
| executor | events `exec.unit.done ×5`（U01 重复 3 次）/recovery 1 条 | 4 task_id 全部 closed | U01-U04 commit | `forge.wave.verified`/settlement 接到 | events + tasks.jsonl | 72 |
| reviewer | events `forge.wave.reviewed ×4` | per-wave verdict=ACCEPTED | 4 次 emit | 1 次 `summary.md` commit 字段错（DEV-004） | events:11,22,29,40 + summary.md | 68 |
| integrator | events `forge.wave.integrated ×4` | FF / cherry-pick 一致 | 4 次 emit | 集成 commit 与 events 一致 | events:12,23,30,41 + git log | 82 |
| verifier | events `forge.wave.verified ×6`（U03 重复 3 次）| passed=true 全绿 | 6 次 emit | settlement 接到 | events:13,32-34,42 + waves/*/verification.md | 65 |
| tester | events `forge.full.verified` | 263 rust + 88 python PASS | 1 次 emit | auditor 接到 | events:46 + full-verification.md | 80 |
| auditor | events `forge.audit.done ACCEPTED` | 15 AC 复测 | 1 次 emit | reporter 接到（audit_report_path 缺文件，DEV-005） | events:47 | 68 |
| reporter | events `forge.report.done` + `LOOP_COMPLETE` | 写 manager-report.md | 2 次 emit（双终态，DEV-008 契约吻合） | loop exit | events:48-49 + manager-report.md | 80 |
| inspector (final) | — | — | — | 复用 events:1 一次性 | 无独立 emit | 60 |

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|--------------|----------|----------|
| P1-1 | `.ralph/forge/.../final-audit.md` 路径被 `forge.audit.done` 引用但文件缺 | mechanism + agent（编排契约遗漏） | **68** | DEV-005 | 双账本(+20) + events payload 字段(+15) + 集成 commit 一致(+15) | N/A (history disabled) | 0 |
| P1-2 | `reviews/summary.md` U04 commit 字段误填 source commit (`37f3abc9`)，与 integration commit (`0d6b5c21`) 不一致 | agent（reviewer summary 模板约束缺失） | **78** | DEV-004 | 双账本(+20) + preset/schemas/parallel-forge.yml 行号(+15) + 集成 commit 链一致(+20) | N/A (history disabled) | 0 |
| P1-3 | Wave 3 `forge.wave.verified` 同 payload 重复 3 次（05:27:37 / 05:27:41 / 05:36:02） | mechanism（verifier 端重复 emit 缺陷） | **65** | DEV-002 | 双账本(+20) + recovery.jsonl 1 条 repair_dispatch(+15) + 5 条同 topic 对照(+15) | N/A (history disabled) | 0 |
| P1-4 | `.ralph/forge/.../units/U0N-completion.md` 主仓 .ralph 树缺失（仅 worktree 内可见） | mechanism（artifact 写盘契约未覆盖） | **60** | DEV-006 | 单账本（fd 探查） | N/A (history disabled) | 0 |

> 无 P0 项；P2 informational 未入表（DEV-001 / DEV-003 / DEV-007 / DEV-008：详见 §4）。

**compound 行说明**：
- P1-1 + P1-4 同源（artifact 写盘契约遗漏），可视为 compound（编排 pipeline 收尾未强制把所有业务 artifact 复制/链接到主仓 `.ralph/forge/.../`）。成分 P1-1 (68) + P1-4 (60) → 整行 64。

---

## 6. 修复建议

> 仅针对 §5 已入表项；§7 疑点不写修复。

### 6.1 短期（operator workaround）

- **目标**：让本次 run 的诊断可被外部阅读者直接复验。  
  **改动**：手动 `cp .ralph/forge/2026-08-05-001-feat-builtin-preset-introspection/waves/wave-4-u04/integration-log.md .ralph/forge/.../final-audit.md` 临时 stub；或在工作树中找到 auditor 实际写盘路径后链回主仓。  
  **预期效果**：消除 DEV-005 的"audit 报告路径悬空"，下游 reviewer 不用再追 worktree 路径。  
  **关联置信度**：68

- **目标**：修正 reviewer summary U04 commit 字段。  
  **改动**：编辑 `.ralph/forge/.../reviews/summary.md:8` 将 `37f3abc9…` 改为 `0d6b5c21…`（与 waves/wave-4-u04/settlement.md / commit-map.yml / events `forge.wave.integrated.candidate_commit_sha` 一致）。  
  **预期效果**：消除 review artifact 与 git 真相的歧义。  
  **关联置信度**：78

### 6.2 中期（preset / schema / instructions）

- **目标**：让 reviewer summary 默认填 integration commit。  
  **改动**：在 `presets/en/parallel-forge.yml` 的 reviewer hat `instructions:` 加一条："summary commit 字段 = `forge.wave.integrated.candidate_commit_sha` (NOT source candidate commit from `forge.wave.worktrees.ready`)；U04 cherry-pick 场景尤其重要"；同步更新 `skills/ralph-preset-author/references/{author-checklist,prompt-visibility,commands}.md` 与 `skills/ralph-preset-review/references/agent-skill-audit.md`。  
  **预期效果**：消除 DEV-004 同型再发。  
  **关联置信度**：78

- **目标**：约束 `forge.wave.verified.passed` JSON 类型。  
  **改动**：`presets/schemas/parallel-forge.yml:259-260` 补一行 `type: boolean`；并在 `crates/ralph-core/src/preset_lint/state_projection.rs` 增一条 `finding_id: passed_field_must_be_bool` 静态检查。  
  **预期效果**：消除 wave-2/3/4 `"true"` 漂移（DEV-003），未来 LLM 错误不会绕过 schema。  
  **关联置信度**：70

- **目标**：verifier 同 payload 重复 emit 抑制。  
  **改动**：在 `presets/schemas/parallel-forge.yml` 的 `forge.wave.verified` 加 `idempotency_key` 字段（= `wave_id + candidate_commit_sha`），让 state_projection 在已 accepted 后 drop 重复事件；或由 `forge-dispatcher` 在 settlement 后忽略同 key。  
  **预期效果**：消除 DEV-002（U03 重复 3 次 verifier emit）。  
  **关联置信度**：65

### 6.3 长期（机制 / 底座）

- **目标**：建立 reporter 后置"必写盘清单"。  
  **改动**：在 `crates/ralph-core/src/state_projection.rs` `reporter` 分支增加 `required_artifacts=[forge.audit.done.audit_report_path, units/U*.completion.md（按 settlement 列表）]` 写盘门禁；缺失则 `ralph emit forge.report.done` 被 precheck 拒绝（parallel-forge §0 已要求 `--policy-check` 强预检）。  
  **预期效果**：消除 DEV-005 / DEV-006 同源再发；让 audit report 与 unit completion 真正在主仓可见（不再仅 worktree 局部）。  
  **关联置信度**：64 (compound)

- **目标**：建立 wave_id 命名一致性。  
  **改动**：在 `presets/en/parallel-forge.yml` 的 `forge-dispatcher` instructions 中要求 `wave_id` 命名统一为 `wave-<index>-<unit_id>`（本次 wave-1 用 hash id `w-18c8cc09bcd93488-1963-0` 而 wave-2/3/4 用语义 id，混用）。  
  **预期效果**：下游日志/报告 grep 容易；避免 `wave-1-u01` 在 summary 中被造词但 events 中找不到匹配（事实上 wave-1 的 wave_id 实际是 hash id）。  
  **关联置信度**：65

---

## 7. 未核实疑点（可选）

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|----------|------------|------------|----------|
| `forge.wave.verified` wave-1 `passed: true`（bool）与 wave-2/3/4 `passed: "true"`（string）是否源自 schema 兼容层的隐式 string coerce | 50 | MINIMAL 模式无 orchestration.jsonl；不可观察 verifier 实际 emit 前的 type coerce | 已查 events + recovery，未能定位 schema-coerce 源码行 |
| U01 `exec.unit.done` 重复 3 次（首次 hash 为 placeholder `u01-builtin-list-inventory-foundation-serial`）是否反映 executor hat 在 wave-1 首次运行时未读到 real commit 而使用了 plan unit_id 派生 hash | 48 | MINIMAL 模式 + worktree 已被 loop 清理；缺 executor hat-channel | 已查 events + tasks.jsonl，hash 派生逻辑需回到 `crates/ralph-core/src/executor/*` 静态查（受 OPAC agent 域硬顶） |
| `forge.audit.done.audit_report_path` 引用不存在的 final-audit.md 是否因为 auditor 把报告写到 worktree 但未 copy 出，或写盘后被 recovery 流清理 | 52 | worktree 已清理；缺 worktree 留存 | 已在 wave-4 的 integration-log / settlement 中未发现任何 "copy final-audit" 步骤 |

> 以上均 confidence<60，不写修复建议；待后续 LOGS_ONLY 或 FULL 模式再跑一次时复检。
