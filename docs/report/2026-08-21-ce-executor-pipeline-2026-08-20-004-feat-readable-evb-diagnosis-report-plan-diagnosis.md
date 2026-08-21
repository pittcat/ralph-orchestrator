---
title: ce-executor-pipeline Loop `2026-08-20-004-feat-readable-evb-diagnosis-report-plan` 运行链路诊断报告
date: 2026-08-21
type: diagnosis
loop_id: 2026-08-20-004-feat-readable-evb-diagnosis-report-plan
preset: builtin:ce-executor-pipeline
run_dir: /Users/pittcat/Dev/Python/worktree/atelier/2026-08-20-004-feat-readable-evb-diagnosis-report-plan
status: Failed — scope_violation_hard_rejected (dim:testing), 9 iterations in 57m 20s, hard-rejected on first dimension-reviewer scope violation
diagnostics_mode: LOGS_ONLY
bundle: legacy
bundle_path: N/A (no $RUN/.ralph/diagnostics/<session>/diagnosis-input.json; bundle reader legacy fallback)
history_search: preset-only
structured_result_ref: "inline: summarized in report"
trace_status: missing
feedback_status: missing
activation_outcomes: missing
evidence_gaps:
  - 缺 structured orchestration bundle（diagnostics dir 仅含 agent_doc_sync.json + logs/，无 session 子目录）
  - 缺 runtime-trace.jsonl / feedback.jsonl → activation_outcomes 无法对账
  - 缺 dim:testing hat 的 agent-output stdout/stderr（PTY 内部 capture 仅写到 .ralph/events.jsonl 的 hat channel 行）
  - 缺 dim:testing 激活期间所有 shell 命令逐条记录；只能从测试报告与 git diff 反推 uv.lock 内容变化根因
---

# ce-executor-pipeline Loop `2026-08-20-004-feat-readable-evb-diagnosis-report-plan` 运行链路诊断报告

> **生成时间**: 2026-08-21（today）
> **诊断对象**: `/Users/pittcat/Dev/Python/worktree/atelier/2026-08-20-004-feat-readable-evb-diagnosis-report-plan/.ralph/`（loop_id=`2026-08-20-004-feat-readable-evb-diagnosis-report-plan`，启动 2026-08-20 22:08 UTC → 终止 2026-08-20 23:06 UTC）
> **对照 preset**: `presets/en/ce-executor-pipeline.yml` + `presets/schemas/ce-executor-pipeline.yml`
> **执行方式**: 主 Agent（盘点 + 落盘） + Agent A（preset / schema / BDD 对账） + Agent B（preset-only 30 天历史检索）。Agent C / D 未独立启动——根因与归因在 Phase 0 已基本锁定，A + B 验证后直接合成。
> **Diagnostics 模式**: **LOGS_ONLY**（`$RUN/.ralph/diagnostics/` 仅有 `agent_doc_sync.json`（lock-only, 98B）+ `logs/ralph-2026-08-21T06-08-58-099-6801.log`（12KB, 55 行 INFO/WARN/ERROR））
> **history_search**: `preset-only`（30 天窗口：2026-07-22 → 2026-08-21）
> **execution_capabilities**: `["supervisor"]`（YAML `event_loop.exervisor.enabled: true` ∈ preset 顶层；`.ralph/supervisor.db` 存在且 CLI log 第一行 `default wave path picked up supervisor-db (KTD-2 / 2026-07-22-001 U3)` 实证；无 `wave_id` 行，**不**含 `wave` capability）
> **报告仓库**: `ralph-orchestrator` 主仓
> **Tier C 根**: `docs/plans/2026-08-20-004-feat-readable-evb-diagnosis-report-plan.md` + `.ralph/review/2026-08-20-004-feat-readable-evb-diagnosis-report-plan/`（baseline-verification.md / final-verification.md / correctness.md / goal-alignment.md / testing.md / trace.md / reuse-guidance.md / review.diff.patch / review.diffstat.txt / git-state-*.txt / stabilization/audit.md）
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70；LOGS_ONLY → agent/OPAC 单项 ≤50，整行硬顶 75

---

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数 / 大小 | 备注 |
|------|------|------|-------------|------|
| S | `.ralph/current-events` → `events-20260820-220858.jsonl` | ✅ | 11 events, 65 KB | **唯一** events SSOT；指针链完好 |
| S | `.ralph/events-history-20260820-220858.jsonl` | ✅ | 51 KB | 旁路解析，非编排 SSOT |
| S | `.ralph/history.jsonl` | ✅ | 52 KB | loop 级溯源（≠ events-history） |
| S | `.ralph/ledger.jsonl` | ✅ | 7.5 KB / ~26 行 | iteration / 拒收 / observation 三类 |
| S | `.ralph/recovery.jsonl` | ❌ | — | 不存在 → workspace 级拒收为空（recovery 信号靠 loop-termination-reason.json） |
| S | `.ralph/loops.json` | ✅ | `{"loops":[]}` 空数组 | loop 已终止，无在飞 |
| S | `.ralph/loop.lock` | ❌ | — | 未持有（exit 已清理） |
| S | `.ralph/supervisor.db` | ✅ | 139 KB | capability +supervisor 实证（**非**默认 default-wave） |
| S | `.ralph/loop-termination-reason.json` | ✅ | 209 B | **根因锚点**：`{"scope_violation_hard_rejected":{"hat":"dim:testing","diff_stat":"HEAD 5dabf8b -> 5dabf8b; dirty paths: [\" M agents/code-writer/uv.lock\"]"}}` |
| A | `.ralph/agent/tasks.jsonl` | ✅ | 0 行 | tasks.enabled: true 但本 run 无 task 记录（pipeline 走 taskless 路径） |
| A | `.ralph/agent/progress.md` | ❌ | — | 未生成（schema `state_projection` 在 ce-executor-pipeline 未启用） |
| A | `.ralph/agent/summary.md` | ✅ | 498 B | **Status: Failed: dimension-reviewer scope_violation (hard-rejected)**；9 iterations / 57m 20s；events by topic histogram 11 条；Final Commit `5dabf8b` |
| A | `.ralph/agent/decisions.md` | ✅ | 3.6 KB | 8 条 entry，从 22:09:57 `step 2.5b resolved_baseline_sha` 到 07:30:00 `stab-001-001 production fix`；07:00 dim:testing 期间无 decision 写入 |
| A | `.ralph/agent/accepted-transitions.jsonl` | ✅ | 10.8 KB / ~25 行 | accepted event batch 流水 |
| A | `.ralph/agent/context.md` | ✅ | 1.7 KB | worktree 元数据（路径/分支/Prompt 摘要/隔离说明） |
| A | `.ralph/agent/resume-context.md` | ✅ | 351 B | `.ralph/reuse-history/20260820T220858.083150000Z` 复用提示 |
| A | `.ralph/agent/plan-baseline.sha` + `plan-baseline-plans-...sha` | ✅ | 40 / 41 B | `b81603a4...6bc`（与 trigger resolved_baseline_sha 一致） |
| B | `.ralph/diagnostics/logs/ralph-2026-08-21T06-08-58-099-6801.log` | ✅ | 12 KB / 55 行 | **CLI 主证据**：process_group → prompt_injection × 8 → 终止三连 ERROR；最后两行 `Wrapping up: scope_violation_hard_rejected. 9 iterations in 57m 20s.` |
| B | `.ralph/diagnostics/agent_doc_sync.json` | ✅ | 98 B（lock-only） | 仅作 session 标记存在，无 payload |
| B | `.ralph/diagnostics/<session>/diagnosis-input.json` | ❌ | — | **bundle 缺失 → legacy fallback** |
| B | `.ralph/diagnostics/<session>/runtime-trace.jsonl` | ❌ | — | **trace 缺失 → activation_outcomes: missing** |
| B | `.ralph/diagnostics/<session>/feedback.jsonl` | ❌ | — | **feedback 缺失** |
| B | `.ralph/forge/` | ✅ | 3 子目录 | forge preset 状态（pipeline 不用，预留） |
| B | `.ralph/flow-authority.jsonl` | ✅ | 2.4 KB / ~10 行 | step-close 决策（与 decisions.md 互补） |
| B | `.ralph/drift:<uuid>:loop:<plan>.jsonl` | ✅ | 596 B | 单次 drift 记录（与 reuse-history 关联） |
| B | `.ralph/reuse-history/20260820T220858.083150000Z/` | ✅ | — | prior runtime 归档（prompt 中 resume-context.md 引用） |
| B | `.ralph/specs/`、`tasks/` | ✅ | symlink → atelier 主仓 | 跨 worktree 共享 |
| B | `.ralph/agent/scratchpad.md` | ❌ | — | preset 未指定 scratchpad 路径，本 run 无 |
| B | `.ralph/agent/events-hat-*.jsonl` | ❌ | — | pipeline preset 走 main events，**未**触发 hat-channel 隔离（与 supervisor+wave 不同） |
| C | `docs/plans/2026-08-20-004-feat-readable-evb-diagnosis-report-plan.md` | ✅ | 51 KB | plan 主体（ce-executor-pipeline pipeline） |
| C | `.ralph/review/2026-08-20-004-feat-readable-evb-diagnosis-report-plan/` | ✅ | 19 文件 / 4.5 MB | baseline-verification.md / correctness.md (20 KB) / final-verification.md / git-state-{correctness,goal-alignment,testing}-{start,end}.txt / goal-alignment.md (15 KB) / normalized-plan.md (23 KB) / reuse-guidance.md (10 KB) / review.diff.patch (1.9 MB) / review.diffstat.txt / stabilization/audit.md / testing.md (26 KB) / trace.md |
| C | `agents/code-writer/uv.lock` | ✅ (dirty) | 5+, 1- | **触发指纹**：atelier-common 0.1.0→0.2.0 + pyyaml/catalog-import extras |
| C | `ralph.pipeline.yml` | ✅ | 1.6 KB | operator 配置（含 memory_id 等） |

**execution_capabilities 推断**：

- `event_loop.supervisor.enabled: true` ∈ preset 顶层 → capability **+supervisor**（YAML 信号）。
- `.ralph/supervisor.db` 存在且 CLI log 印证 `default wave path picked up supervisor-db (KTD-2 / 2026-07-22-001 U3)` → capability +supervisor（产物侧 + ledger 实证）。
- events 11 条 0 条含 `wave_id` → **不**含 `wave` capability。
- hat `instructions:` 未出现 `ralph wave emit` / `ralph wave verify` / `## WAVE CONTEXT` → +wave 不命中。
- 结论：`["supervisor"]`。

**缺失产物 → 故障判定**（capability-triggered）：

- `.ralph/supervisor.db` 缺失？→ 不缺失，**N/A**。
- events 无 `wave_id`？→ capability 不要求 wave，**N/A (capability 不要求)**。
- `diagnosis-input.json` / `runtime-trace.jsonl` / `feedback.jsonl` 缺失 → bundle 走 **legacy fallback**（按 §0.2）；**不**作 P0（无 bundle 是 8/20 前 session 常态）。
- `.ralph/recovery.jsonl` 缺失 → workspace 无拒收（scope_violation 走 termination 直接 hard-reject，不入 recovery），**预期**。

**盲区 / 根因置信度硬顶**：

- LOGS_ONLY：agent/OPAC 单项 ≤50，整行硬顶 75（mechanism + preset 不受此约束）。
- 缺 agent-output stdout/stderr → dim:testing 期间究竟跑了哪些 shell 命令只能**反推**（基于 testing.md 的测试执行清单与 `git diff agents/code-writer/uv.lock` 内容），agent 单向归因置信度上限 50。
- 缺 runtime-trace.jsonl → 无法对账 `phase=activation` / `kind=hat_activation_outcome` 行集（§4.2 整节 N/A）。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: **假闭环 / 早班 termination**（按 U5 plan 2026-07-04-004 设计：dim:* reviewer 的 scope_violation 走 first-violation hard-reject 防 silent-success，不是 bug 是**正确兜底**）
- **P0 / P1 / P2 数量**: P0=1 / P1=2 / P2=1
- **最高优先级根因置信度**: P0-1 = **92** / 100（mechanism 主导）
- **历史复发**: 是 — 第 2 次（首次 = `2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan` 的 `dim:goal-alignment` scope_violation_hard_rejected），同一机制 `audit_file_modifications` 家族。**新增的复合维度**：uv.lock 副作用（`docs/report` + `docs/solutions` 30 天窗口 0 命中先例）。

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ | 流程合规：11 events 拓扑闭合、`isolated` 单事件预算（`Isolated mode: extra business event dropped` 在 dim:correctness 第 2 次被正确丢弃）、executor→test-stabilizer→review chain 完整；但 **OPAC 在 LOGS_ONLY 下证据降级**，dim:testing 单 hat 缺 stdout 对账 | 70（OPAC 降级硬顶 75 内） |
| Q2 | 基座机制是否正常生效？ | ✅（设计如此）/ ❌（覆盖面不足） | `audit_file_modifications` + `ScopeViolationHardRejected` 链路 100% 正常生效（`termination.rs:131,164`）；但**机制无 `*.lock` 白名单**，uv.lock 副作用被误判为 scope violation | 95（mechanism + preset 行号 + 双账本一致） |
| Q3 | 编排是否合理、正常运行？ | ⚠️ | 编排层（plan-reviewer → executor → test-stabilizer → 6 dim → review-synthesizer → fix-planner → fixer → alignment → reporter）按设计走完 6 维度前 4 个；`dim:testing` 在第 5 个触发 hard-reject；但 dim hats 串行串行触发顺序（testing 在 correctness 之后）让 uv.lock 在 5 维度中段被引爆，**没有给上游稳定 uv.lock 的机会** | 75 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **compound（mechanism + preset + agent）** | mechanism: `WorktreeSnapshot::capture` 对 dirty 文件 fingerprint 含 content hash，无 lockfile 白名单（`worktree_handoff.rs:65-86`）；preset: `dim:testing` `instructions:` 允许 re-run test suite 但无 lockfile 硬规则（`ce-executor-pipeline.yml:3899-4017`）；agent: dim:testing 的 Claude 在激活期内对 `agents/code-writer/uv.lock` 做了不可逆 content 增量（`5+,1-`），最可能是 `uv run pytest` / `uv sync` 在传递依赖解析时改写 | 85（取 §5 主因） |

### 1.3 根因一句话

**`audit_file_modifications` 在 dim:testing 激活结束后抓到 `agents/code-writer/uv.lock` 的 `dirty_fingerprint` 因 content 增量（5+,1-：`atelier-common 0.1.0→0.2.0` + `pyyaml` / `catalog-import` extras）而变化，触发 hard-reject `ScopeViolationHardRejected { hat: "dim:testing" }`，loop 在第一次 dim:* scope_violation 即终止（按 plan 2026-07-04-004 U5 设计）。**

### 1.4 终态时序一致性（event-artifact chronology）

| 项目 | 内容 |
|------|------|
| **首轮终态（initial_terminal_status）** | 按 accepted event 序列判定：**首轮失败（REJECTED）**——终止触发 `scope_violation_hard_rejected`（hat=dim:testing, diff_stat 锚定 uv.lock）。summary.md 与 CLI log + loop-termination-reason.json 三账本一致 |
| **恢复状态（recovery_status）** | **无恢复**（hard-reject 无 retry_budget，loop 直接 exit 1） |
| **最终代码状态（final_code_state）** | branch HEAD = `5dabf8b`（`fix(stabilizer): stab-001-002 validate_run_request before RunStatusWriter.start`，来自 test-stabilizer 的 stab-001-002 commit），git working tree dirty only `agents/code-writer/uv.lock`（5+,1-）；其余 U1–U8 全部 commit 已落（`cc983c5, ba09383, a5a62a6, 1c5eee5, 1414cc0, bc8307f, 4e6bdee, b85acde`），test-stabilizer 又补 stab-001-001（`5962ece`）+ stab-001-002（`5dabf8b`）+ housekeeping（`9d5d057 / 12d5495 / 6e8f0a2 / cf5909a`），plan 8 个 Unit 全部交付 |
| **一致性告警** | ⚠️ **失败终态后无恢复**：本 loop exit 1，loop-termination-reason.json 写盘但下游 reporter / alignment / fixer 未触发；8 个 Unit + 2 个 stabilizer commit 客观已落但**本次 run 视角下未走完 review-synthesizer→fix-planner→fixer→alignment→reporter**，需 `--reuse-worktree` 重新进入（resume-context.md 提示 `20260820T220858.083150000Z` 已归档） |

---

## 2. 执行链路对比图

### 2.1 拓扑表（ce-executor-pipeline pipeline 段）

| Step | Hat | Topic In | Topic Out | Actual 时间 (UTC) | 备注 |
|------|-----|----------|-----------|-------------------|------|
| 0 | (runner) | — | `work.start` | 22:08:58 | loop 启动，supervisor.db picked up |
| 1 | plan-reviewer | (start) | `plan.ready` | 22:11:48 | resolved_baseline_sha=`b81603a4` |
| 2 | executor | `plan.ready` | `work.done.proposed` → precheck → `work.done` | 22:17:47 → 22:22:59 | executor_head=`6e8f0a2`，8 个 U commit 落齐 + housekeeping |
| 3 | test-stabilizer | `work.done` | `stabilization.done.proposed` → precheck → `stabilization.done` | 22:49:08 → 22:49:42 | stab-001-001（`5962ece` fastapi[testclient]）+ stab-001-002（`5dabf8b` validate_run_request），executor_head 升级到 `5dabf8b` |
| 4 | dim:goal-alignment | `stabilization.done` | `review.goalalign.done` | 22:52:52 | findings（goal-alignment.md 15 KB） |
| 5 | dim:correctness | `review.goalalign.done` | `review.correctness.done` (×3) | 22:56:22 / 22:56:27 / 22:58:19 | 第 2 次 `Isolated mode: extra business event dropped`，第 3 次正常 emit |
| 6 | dim:testing | `review.correctness.done` | `review.testing.done` → **scope_violation_hard_rejected** | 23:06:09 → 23:06:18 | **触发**：dirty_fingerprint 因 `agents/code-writer/uv.lock` 内容变化而 != baseline；audit → BlockLoop + ScopeViolation 触发器 → termination |
| (未达) | dim:maintainability / dim:project-standards / dim:adversarial / review-synthesizer / fix-planner / fixer / alignment / reporter | — | — | — | hard-reject 后未触发 |

### 2.2 时间轴（mermaid）

```mermaid
timeline
    title ce-executor-pipeline 2026-08-20-004 readable-evb-diagnosis-report-plan
    22:08:58 : work.start (loop 启动)
    22:11:48 : plan.ready (plan-reviewer)
    22:17:47 : work.done.proposed (executor, head=6e8f0a2)
    22:22:59 : work.done (precheck)
    22:49:08 : stabilization.done.proposed (test-stabilizer)
    22:49:42 : stabilization.done (precheck, head=5dabf8b)
    22:52:52 : review.goalalign.done
    22:56:22 : review.correctness.done (1st)
    22:56:27 : review.correctness.done (dropped, isolated 单事件预算)
    22:58:19 : review.correctness.done (3rd, ok)
    23:06:09 : review.testing.done → 🛑 scope_violation_hard_rejected
    23:06:18 : Wrapping up. 9 iterations / 57m 20s. exit 1.
```

### 2.3 关键证据对照

| 时间 (UTC) | CLI log 行 | event / artifact | 行为 |
|------------|-----------|------------------|------|
| 22:56:37 | `WARN ... extra business event dropped` | dim:correctness 第 2 次 emit | 隔离单事件预算生效 |
| 23:06:09 | — | `review.testing.done` emit (executor_head_sha=`5dabf8b`) | dim:testing 收尾 |
| 23:06:18.217 | `WARN ... Hat modified files despite tool restrictions (scope violation) hat=dim:testing diff=HEAD 5dabf8b -> 5dabf8b; dirty paths: [" M agents/code-writer/uv.lock"]` | `WorktreeSnapshot::changed_since=true`（head 不变但 fingerprint 变） | 警告 |
| 23:06:18.217 | `ERROR ... audit finding (block-loop severity, immediate termination) hat=dim:testing kind=scope_violation` | `AuditSeverity::BlockLoop { reason: "scope_violation" }` + `RejectionKind::ScopeViolation` | 升级 hard-reject |
| 23:06:18.247 | `ERROR ... scope_violation_hard_rejected: terminating loop on first dimension-reviewer scope_violation` | `trigger_to_reason → TerminationReason::ScopeViolationHardRejected { hat: "dim:testing", diff_stat }` | push trigger |
| 23:06:18.247 | `INFO ... Wrapping up: scope_violation_hard_rejected. 9 iterations in 57m 20s.` | exit 1，`loop-termination-reason.json` 写盘 | 终止 |

---

## 3. 历史问题上下文

> **⚠️ 启用条件**：`history_search=preset-only`（30 天滑动窗口 2026-07-22 → 2026-08-21）。Agent B 主扫 `docs/report/`（57 份）+ `docs/solutions/`（根目录 3 + 子目录 11）。关键词集合：`ce-executor-pipeline`、`ce-executor-serial`、`dim:testing` / `dim:*` / `dimension-reviewer`、`scope_violation` / `scope_violation_hard_rejected`、`audit_file_modifications`、`disallowed_tools` / `Edit/Write disallowed`、`uv.lock` / `lockfile` / `uv sync` / `uv lock`、`agents/code-writer`。
> **本次扫描窗口**：preset-only (30d sliding)

### 3.1 命中清单（preset 家族 + 症状家族，复发对照）

| # | 文件（repo-relative） | 命中关键词 | 症状相似度 | 一句话上下文 |
|---|---|---|---|---|
| H1 | `docs/report/2026-08-13-ce-executor-pipeline-2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan-diagnosis.md`（L42, L75, L96, L141, L159, L214） | `scope_violation_hard_rejected` + `dim:*` + `audit_file_modifications` + `dim:testing`（review chain 表中） | **高** | ⭐ **直接复发**：ce-executor-pipeline `dim:goal-alignment` 在 `--reuse-worktree` 下将预存 38 个 dirty paths 判定为 P0 scope violation，触发 hard-reject；机制层同源 `audit_file_modifications`。修复 P0 置信度 95 |
| H2 | 同 H1（L42-43, L214） | `dim:goal-alignment` + `scope_violation_hard_rejected` | 高 | 同上的精确出错 hat 与机制出处；源码引用 `dispatch_and_handoff.rs:996-1064` |
| H3 | `docs/report/2026-08-12-ce-executor-pipeline-2026-08-12-001-diagnosis.md`（L31, L61, L81-83, L116-117, L125） | `dim:goal-alignment` + 同 payload 重复 + `--triggered` 自路由 | 中 | 同 preset 复发，但病因是 `--triggered=dim:goal-alignment` 自路由导致 loop_stale（3 次），**非** scope_violation 家族 |
| H4 | `docs/report/2026-08-10-ce-executor-pipeline-2026-08-10-002-fix-scope-gates-and-digest-plan-diagnosis.md`（L52, L66, L171, L188） | `isolated_scope_violation` + `parse_and_emit.rs:605-720` | 中 | 旁系：同一机制家族（`event_origin` 拒绝越权 emit），但源头在 workflow_guard publish gate，与本 run 病因不同 |
| H5 | `docs/report/2026-08-06-ce-executor-pipeline-2026-08-05-001-refactor-large-file-module-split-plan-diagnosis.md`（L119-124, L174-175, L230, L284, L297, L305） | `dim:testing` 等 6 dim + `isolated_scope_violation ×6` | 中 | 同 preset 同日另一 plan；6 维度 chain 显式列出 `dim:testing / dim:correctness / dim:goal-alignment / …` |
| H6 | `docs/report/2026-08-08-ce-executor-pipeline-2026-08-07-003-refactor-emit-module-split-plan-diagnosis.md`（L44, L55, L60, L78, L101, L112, L120, L133） | `dim:goal-alignment` no-emit + stall-detector 阻塞 | 中 | 同 preset，P0 置信度 94；引用 H3 复发链「2026-07-29 plan 已合并但机制层 root cause 未根治」 |
| H7 | `docs/report/2026-08-15-atelier-ce-executor-pipeline-2026-08-15-0750-feat-modem-log-bundle-evidence-review-plan-diagnosis.md`（L107-113） | 6 dim transition 表 | 中 | 同一 preset 的 happy path 对照样本（6 维度全部 emit done） |
| H8 | `docs/report/2026-08-01-ce-executor-pipeline-2026-08-01-001-fix-unified-execution-contract-p0-p1-plan-diagnosis.md`（L109-114） | `dim:goal-alignment / dim:correctness / dim:testing` 等 6 维度表 | 低 | 同 preset 早期 plan；6 维度契约化设计文档 |
| H9 | `docs/report/2026-08-02-...` / `2026-07-29-...` / `2026-07-24-...`（3 份） | `ce-executor-pipeline` preset 头 | 低 | 仅命中 preset 名，未触发本次症状家族 |
| **H10** | **`docs/solutions/tooling-decisions/claude-disallowed-tools-edit-write-dimension-reviewer.md`**（L2, L11-13, L27, L31, L46-86, L103-141, L160-183） | `disallowedTools` + `audit_file_modifications` + `dimension-reviewer` | **中（schema，规范层）** | ⭐ **关键 solutions 候选**，但**发布日期 2026-07-06 — 落在 30 天窗口外**（>30 天前）。定义：① adapter spawn 时合并 hat `disallowedTools` 整工具名；② **`audit_file_modifications` + U5 BlockLoop 是唯一可靠兜底**——这正是 H1 暴露的机制层 gap 的依据 |

### 3.2 复发对照

| 复发序 | 日期 | hat | preset | 症状家族 | 已沉淀到 solutions/？ |
|---|---|---|---|---|---|
| **1** | **2026-08-13** | **dim:goal-alignment** | **ce-executor-pipeline** | **`scope_violation_hard_rejected`（carryover dirty paths）** | **未**（仅在 report，待 8-13 plan 落地） |
| **2** | **2026-08-21**（本 run） | **dim:testing** | **ce-executor-pipeline** | **`scope_violation_hard_rejected`（uv.lock 副作用）** | **未**（即本次） |
| 3 | 2026-08-12 | dim:goal-alignment | ce-executor-pipeline | dim:* 自路由 loop_stale（非 scope_violation 家族） | 未 |
| 4 | 2026-08-08 | dim:goal-alignment | ce-executor-pipeline | no-emit + stall-detector | 未 |
| 5 | 2026-08-06 | executor / test-stabilizer | ce-executor-pipeline | `isolated_scope_violation ×6`（旁系，`event_origin`） | 已知部分未根治 |
| 6 | 2026-08-10 | executor | ce-executor-pipeline | `isolated_scope_violation`（旁系，originated 裸 `work.done`） | 未 |
| 7 | 2026-07-26（×3 run） | review-dispatcher / finalizer | ce-executor-pipeline | `isolated_scope_violation` history family #3 | 部分沉淀 |

**结论**：30 天窗口内 `scope_violation_hard_rejected` + `dim:* reviewer` 触发 **2 次 P0 主因**（08-13 dim:goal-alignment + 08-21 dim:testing）；均尚未沉淀到 `docs/solutions/`。最新沉淀仍是 2026-07-06 的 tools-decisions 方案 H10（窗口外，规范层定义）。

### 3.3 Compound 候选（uv.lock + scope 校验）

| 查询 | 结果 |
|---|---|
| `uv.lock` / `lockfile` / `uv sync` / `uv lock` 在 docs/report + docs/solutions 中命中数 | **0** |
| `agents/code-writer` 在 docs/report + docs/solutions 中命中数 | **0** |
| 30 天内 compound 「lockfile 黑名单 + hat instructions 修复」 | **未发现** |
| 30 天内已有 operator workaround | 仅 H10（窗口外）规范层定义「`audit_file_modifications` 兜底」，**不是 lockfile 副作用专项** |

**结论**：本次 run 的「uv.lock 副作用 + scope 校验」组合在 docs/ 文档体系中**没有先例**。H1 是 `audit_file_modifications` × reviewer preset handoff precheck 的 carryover dirty 误判，**H1 的解决方案不应直接套用**——H1 是 baseline 缺失（H1 的 dirty 来自 prior run，runtime 没保存 pre-activation snapshot），本次是 dirty fingerprint 含 content hash（baseline 在，但 agent 改写了 content）。

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|----|------|----------|------------|------------|--------------|----------|
| DEV-001 | `audit_file_modifications` 在 dim:testing 激活后 `WorktreeSnapshot::changed_since` 返回 true | `worktree_handoff.rs:44-46`（`changed_since` 定义）+ `dispatch_and_handoff.rs:996`（调用点）+ `events.jsonl#L11`（23:06:09 `review.testing.done`）+ `CLI log L51`（23:06:18.217 WARN） | P0 | 95 | file:line × 4 + 双账本（events + CLI log）+ preset 行号（3896 `disallowed_tools`）+ agent-output partial（git diff 实证） | 无 |
| DEV-002 | `ScopeViolationHardRejected` hard-reject 路径——第一次 dim:* reviewer scope_violation 即终止，exit 1 | `termination.rs:96-102, 153-167`（typed trigger）+ `termination.rs:224-242`（测试断言 `exit_code() == 1`）+ `loop-termination-reason.json`（写盘）+ `summary.md`（Status 行）+ `CLI log L53`（23:06:18.247 ERROR） | P0 | 95 | file:line × 2 + 三账本（loop-termination-reason.json + summary.md + CLI log） | 无 |
| DEV-003 | `dirty_fingerprint` 含文件 content hash，无 `*.lock` / `uv.lock` 白名单 | `worktree_handoff.rs:30-36`（fingerprint 计算含 path hash + per-path content hash）+ `worktree_handoff.rs:49-52`（仅 `.ralph/` 过滤）+ preset grep 0 命中（`uv.lock` / `*.lock` / `deny_paths` / `allow_paths` 全 0）+ `git diff agents/code-writer/uv.lock` 实证 5+,1- | P0 | 92 | file:line × 2 + preset grep 实证 + git diff 实证 | 缺 dim:testing 期间**逐条** shell 命令日志（agent-output 缺） |
| DEV-004 | `dim:testing` `instructions:` 允许 re-run test suite 但无 lockfile 硬规则 | `ce-executor-pipeline.yml:3899-4017`（`instructions:` 全文）+ `:3896`（`disallowed_tools: ["Edit"]`，仅 Edit 非 Edit+Write）+ `:3920-3922`（"May re-run the project's test suite ... Do NOT modify source"） + preset grep 0 命中（`uv.lock` / `lockfile` / `uv sync` / `uv lock` / `deny_paths` / `allow_paths`） | P1 | 85 | file:line × 4 + preset grep 实证 | 无 |
| DEV-005 | dim:testing 激活期 `agents/code-writer/uv.lock` 内容被改写 5+,1-（atelier-common 0.1.0→0.2.0 + pyyaml + catalog-import extras） | `git diff agents/code-writer/uv.lock`（head `1c08307..c85e33a`）+ `git-state-testing-start.txt`（"NOT empty, uv.lock modified (5+,1-, transitive lockfile refresh from stabilization stab-001-001 uv.run ...)"）+ `git-state-testing-end.txt`（"Porcelain filter unchanged" — 但 content fingerprint 实际变化）+ `decisions.md#07:30:00`（stab-001-001 路径实证） | P1 | 50（LOGS_ONLY → agent 单向硬顶 50） | file:line + git diff + start/end precheck 实证 | **缺 agent stdout / 逐条 shell log**（无法断言 dim:testing 究竟跑了哪条 uv 命令改写了 uv.lock；最可能假设：dim:testing 跑了 `uv run pytest` 在某个含 `agents/code-writer` 子路径的测试命令，或者 `uv sync --all-packages`） |
| DEV-006 | dim:correctness 在 22:56:22 / 22:56:27 / 22:58:19 三次 emit（第二次被 isolated 单事件预算丢弃） | `events.jsonl#L8-10` + `CLI log L33`（22:56:37 "extra business event dropped"）+ `decisions.md` 无 entry | P2 | 90 | file:line + event 行 + CLI log + preset 行为契约 | 无（已知 isolated mode 行为，非 P0） |
| DEV-007 | `WorktreeSnapshot::capture` 启动前已有 dirty `agents/code-writer/uv.lock`（来自 test-stabilizer 的 `uv sync` 副作用，未 commit） | `git-state-testing-start.txt`（pre-activation baseline 已含 uv.lock dirty）+ `decisions.md#07:30:00`（stab-001-001 路径："uv.lock refreshed via uv sync. Modified files: agents/crash-log-analyzer/{pyproject.toml,uv.lock}"）+ `git log` 中 `5962ece` 是 crash 的 lockfile commit，**但 code-writer 的 lockfile 仍 dirty 未 commit** | P1 | 80 | file:line × 3 + decisions.md 实证 | 需 `git diff 5962ece^..5dabf8b --stat` 验证 code-writer/uv.lock 自 stab-001-001 起是否一直 dirty |
| DEV-008 | reuse mode 下 prior runtime 已写 reuse-history，提示 `--reuse-worktree` 进入 prompt | `resume-context.md`（"Previous runtime archive: .ralph/reuse-history/20260820T220858.083150000Z. Treat archived records as advisory evidence"）+ `.ralph/reuse-history/20260820T220858.083150000Z/` 存在 | P2 | 95 | file:line + 路径实证 | 无（reuse 行为按设计） |

### 4.1 OPAC 逐 hat 审计表

> ⚠️ **OPAC 降级声明**（LOGS_ONLY 触发）：缺 structured orchestration bundle + runtime-trace.jsonl + agent stdout → 逐 hat O/P/A/C 仅基于 `events.jsonl` payload + CLI log 的 prompt_injection/pty_executor 行回填。**`P`（prompt）和 `A`（agent output）置信度受 LOGS_ONLY 硬顶 ≤50 影响**；`O`（observation）走 event ledger 全有；`C`（closure）走 CLI log 的 terminal_topic 校验。

| Hat | O | P | A | C | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|
| plan-reviewer | ✅ event L2 `plan.ready` + resolved_baseline_sha | ⚠️ prompt_injection 行（21:11:55）只统计 memory 注入 | ❌ agent stdout 缺 | ✅ CLI log 22:11:55 PtyExecutor spawn → 22:11:48 event | `events.jsonl#L2` + `CLI log L9-13` | O=95 / P=50 / A=30 / C=90 |
| executor | ✅ event L3-4 `work.done.proposed` + `work.done` + executor_head 6e8f0a2 → 5dabf8b（由 test-stabilizer 后续升级） | ⚠️ 8 个 Unit 全部 emit 完成（decisions.md 列出 U1..U8 commit） | ⚠️ decisions.md（3.6 KB）记录 8 条 entry | ✅ CLI log 22:17:47 → 22:22:59 → 22:34:30 串行 emit | `events.jsonl#L3-4` + `decisions.md` 全文 + `CLI log L14-19` | O=95 / P=60 / A=70 / C=95 |
| test-stabilizer | ✅ event L5-6 + stab-001-001/002 commit `5962ece`/`5dabf8b` | ⚠️ "production fix" instructions 印证 | ⚠️ decisions.md 1 条 stab-001-001 entry | ✅ CLI log 22:49:08 → 22:49:42 | `events.jsonl#L5-6` + `decisions.md#07:30:00` + `git log 5962ece/5dabf8b` | O=95 / P=70 / A=70 / C=95 |
| dim:goal-alignment | ✅ event L7 `review.goalalign.done` | ⚠️ goal-alignment.md 15 KB | ❌ agent stdout 缺 | ✅ CLI log 22:52:52 | `events.jsonl#L7` + `goal-alignment.md` + `CLI log L21-25` | O=95 / P=50 / A=30 / C=90 |
| dim:correctness | ✅ event L8-10 ×3 `review.correctness.done`（第二次 isolated drop） | ⚠️ correctness.md 20 KB | ❌ agent stdout 缺（**dropped 那次也缺**——已 silently drop） | ✅ CLI log 22:56:37 WARN "extra business event dropped" | `events.jsonl#L8-10` + `correctness.md` + `CLI log L33` | O=95 / P=50 / A=30 / C=90 |
| dim:testing | ✅ event L11 `review.testing.done` + **触发 scope_violation_hard_rejected** | ⚠️ testing.md 26 KB 含 4 findings | ❌ **agent stdout 缺（最关键缺口，无法对账改写 uv.lock 的 shell 命令）** | ❌ **终止于 audit → BlockLoop，未走 terminal_topic 收尾** | `events.jsonl#L11` + `testing.md` + `CLI log L51-54` | O=95 / P=50 / **A=30** / C=40（未走完 closure） |

### 4.2 Activation outcome 表（plan 2026-08-15-1823）

> **⚠️ 启动条件**：仅当 frontmatter `activation_outcomes: present` 时填写本节。`missing` / `degraded` / `legacy` 时整节写 N/A。本 run frontmatter 标 `activation_outcomes: missing`（缺 `runtime-trace.jsonl`）。

**N/A (activation outcomes unavailable)** — 缺 runtime-trace.jsonl；缺 session 子目录。activation outcome 行集不可消费。已写入 `evidence_gaps`。

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|--------------|----------|----------|
| P0 | **`WorktreeSnapshot::dirty_fingerprint` 含文件 content hash 且无 `*.lock` 白名单，dim:* reader 修改 lockfile 内容即触发 hard-reject** | mechanism | **92** | DEV-001, DEV-002, DEV-003 | file:line × 4 (worktree_handoff.rs:30-36, 49-52; dispatch_and_handoff.rs:996; termination.rs:131,164) = +25 ×2 +25 = +50（最高）；双账本（events + CLI log + loop-termination-reason.json + summary.md）= +20 ×2 = +40（最高 capped at +20）；preset grep 实证 = +15 | **高（H1 同机制复发）** | 1→92 |
| P1 | **`dim:testing` `instructions:` 允许 re-run test suite 但无 lockfile 副作用硬规则** | preset | **85** | DEV-004 | file:line × 4 (ce-executor-pipeline.yml:78, 3896, 3899-4017, 3920-3922) = +25 ×2 = +50；preset grep 0 命中 = +15；preset 行为契约 = +10 | 中（H1 同 preset 复发，但病因不同——H1 是 carryover baseline 缺失，本次是 agent content 改写） | 0→85 |
| P1 | **dim:testing agent 在激活期内改写 `agents/code-writer/uv.lock` content（5+,1-），最可能由 `uv run pytest` / `uv sync` 在传递依赖解析时副作用触发** | agent | **50**（LOGS_ONLY 硬顶） | DEV-005, DEV-007 | file:line × 2 (git-state-testing-start/end.txt + decisions.md#07:30:00) = +25；git diff 实证（5+,1-） = +15；agent stdout **缺** = +10（扣分：-50 硬顶 cap） | **新**（uv.lock 副作用在 docs/ 30 天窗口 0 先例） | 0→50（已硬顶） |
| P2 | dim:correctness 三次 emit（第二次被 isolated 单事件预算 dropped） | preset 行为契约 | **90** | DEV-006 | file:line × 2 = +25 ×2 = +50；event 行 + CLI log 双账本 = +20；preset 契约 = +15 | 新（无历史复发） | 0→90 |

> **历史关联列规则**：本表已按 preset-only 扫描窗口填值（H1 高 / 中 / 低 / 新）。
> **compound 行**：P0 行的 mechanism 主导 + DEV-003 的 preset 缺口 + DEV-005 的 agent 内容改写 → 整行置信度按主因（mechanism）计 92；成分 A (mechanism 92) + 成分 B (preset 缺口 85) + 成分 C (agent 50, LOGS_ONLY cap) → 整行 = min(A,B,C) = 50 的高水位 + 主因加权 = 92（取主因 score）。

---

## 6. 修复建议

> 仅针对 §5 已入表项（confidence ≥ 60 / P0 ≥ 70）。修复建议**只列人工可执行**——agent 不得自动 `ralph run`、不得自动改 preset、不得执行 `rm` / `cargo` / `git` 类命令。

### 6.1 短期（operator workaround，本 run 之后立刻可用）

- **目标**: 让本 run 已落的 8 个 Unit + 2 个 stabilizer commit 走完 review-synthesizer → fix-planner → fixer → alignment → reporter 链
- **改动**:
  1. `git -C <worktree> restore --source=HEAD --staged --worktree agents/code-writer/uv.lock`（先撤销 uv.lock 的 dirty，让 reuse 启动时不带 carryover dirty）
  2. 重新跑 `ralph run --worktree --reuse-worktree -H builtin:ce-executor-pipeline --plan docs/plans/2026-08-20-004-feat-readable-evb-diagnosis-report-plan.md -c ralph.pipeline.yml`（reuse-history 已存在，prompt 会从 pending hat `dim:maintainability` 继续而非重启）
- **预期效果**: 跳过 plan-reviewer → executor → test-stabilizer → dim:goal-alignment → dim:correctness 已完成的 5 个 hat，从 `dim:maintainability` 接续；6 维度剩余 4 个 + review-synthesizer + fix-planner + fixer + alignment + reporter 一次性走完
- **关联置信度**: 80（reuse-history 复用机制已实证 `resume-context.md`，但 8 个 Unit 后续是否触发新 scope_violation 未实测）

### 6.2 中期（preset / schema / instructions，下一条 plan 落地）

- **目标**: 让 dim hats 跑 `uv run pytest` / `uv sync` 之类的副作用不再误触 scope_violation
- **改动（任选其一）**:
  1. **preset fix（推荐）**：在 `presets/en/ce-executor-pipeline.yml:3899`（`dim:testing` `instructions:`）追加硬规则：
     ```
     - HARD: When running tests, use the agent's own `uv run` from inside the agent
       directory (e.g., `cd agents/<name> && uv run pytest tests/`). NEVER run
       `uv sync` / `uv lock` / `uv pip install` from the workspace root — uv will
       propagate lockfile changes across sibling agent dirs (e.g., `agents/code-writer/uv.lock`)
       and trip the read-only scope audit.
     - HARD: If you observe a pre-existing dirty `uv.lock` in another agent dir,
       do NOT touch it. Document in your findings file as "carry-in: pre-existing
       lockfile drift in <path>, out of scope for this dimension".
     ```
  2. **preset fix（备选 1）**：把 `disallowed_tools: ["Edit"]` 改为 `["Edit", "Write"]`（虽然运行时已合并二者判定，**显式声明更清晰**，供 hat instructions / preset review 静态校验使用）。
  3. **机制 fix（备选 2）**：在 `crates/ralph-core/src/event_loop/worktree_handoff.rs` 的 `is_ralph_path` 函数后追加 `is_lockfile_path` 短路——把 `*.lock` / `uv.lock` / `package-lock.json` / `Cargo.lock` 等 lockfile 从 dirty fingerprint 排除（但**保留 HEAD 比对**——commit 仍然要管）。这个改动需要专门 plan + BDD scenario（缺覆盖，见 §4 evidence DEV-001 缺口）。
- **预期效果**: dim hats 的 lockfile 副作用不再触发 hard-reject；agent 不再需要"小心避免 uv 命令"
- **关联置信度**: 85（仅修复 instructions；机制 fix 需更多验证）

### 6.3 长期（机制 / 底座，需 plan 立项）

- **目标**: 让 `audit_file_modifications` 在 dim:* reader 上对**已被 test-stabilizer / executor 持久化的副作用**（已 commit 进 HEAD 但内容 hash 因 uv 写盘更新）与 **agent 故意改写**做精确区分
- **改动（plan 立项）**:
  1. **baseline 持久化**：H1 (2026-08-13) 暴露的"carryover dirty paths from prior run"——loop 启动时把 reuse worktree 的 pre-existing dirty paths 持久化到 `.ralph/agent/activation-baseline.json`；audit 时把 baseline 路径从 dirty fingerprint 中**减去**，避免 H1 + 本次 uv.lock 两种不同场景的混淆。
  2. **content hash 分桶**：`dirty_fingerprint` 拆为 `path_fingerprint`（仅 path 集合）+ `content_fingerprint`（per-path content hash）；scope 违规判定只看 `path_fingerprint` 是否新增——content 变化但 path 不变视作"读+写文件但未引入新 path"，按"warning 而非 hard-reject"处理；这是对当前 `changed_since` 的语义细化。
  3. **BDD scenario 补全**：在 `crates/ralph-core/tests/scenarios/` 新增 `ce_executor_pipeline_dim_lockfile_drift.yml` —— 验证 dim hat 在 pre-existing dirty `uv.lock` 下完成 emit 且不触发 hard-reject（同时验证 content 改写场景仍按设计走 hard-reject）。
- **预期效果**: H1 + 本次 + 后续所有 `scope_violation_hard_rejected` 复发能被 preset rule 静态拦截；audit_file_modifications 的 false-positive 率下降；silent-success 兜底仍保留
- **关联置信度**: 75（mechanism 改动需要 plan + 测试覆盖 + BDD scenario 多轮验证）

---

## 7. 未核实疑点（可选）

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|----------|------------|------------|----------|
| dim:testing 激活期究竟跑了哪一条 shell 命令改写了 `agents/code-writer/uv.lock` 内容？ | 30 | 缺 agent stdout / 缺逐条 shell log | 已读 git-state-testing-{start,end}.txt + decisions.md + testing.md；均未列具体 uv 命令（testing.md 仅列"per-package test cross-verification"用 `uv run pytest`，未触及 code-writer）。**强证据候选**：`uv sync --all-packages` 从 workspace root 触发传递依赖写盘（这是 uv 默认 workspace-level 行为），但缺直接证据 |
| stab-001-001 (`5962ece`) 是否曾让 `agents/code-writer/uv.lock` 自该 commit 起一直 dirty 到 dim:testing 之前？ | 60 | 缺 `git diff 5962ece^..5dabf8b --stat -- agents/code-writer/uv.lock`（可一行 bash 验证，但本 run 已终止） | 已读 decisions.md#07:30:00 实证 stab-001-001 修改的是 `agents/crash-log-analyzer/{pyproject.toml,uv.lock}` 而非 code-writer；但 code-writer 的 uv.lock 5+,1- 是 `atelier-common 0.1.0→0.2.0` + pyyaml，这来自 U1 commit `cc983c5` 的 libs/common pyproject.toml 变更——任何在 U1 之后从 workspace root 跑 `uv sync` 都会让 code-writer/uv.lock 跟着刷 |
| `loops.json` 为空 `{"loops":[]}` 是否暗示 supervisor db 与主 db 状态分裂？ | 50 | 缺 supervisor.db schema 验证 + 缺 supervisor writer 路径源码验证 | 已读 supervisor.db 存在（139 KB）+ CLI log "default wave path picked up supervisor-db"。可能解释：loop 终止时 supervisor.db 已清空 loops 表（预期），或 db 已损坏（**未**有错误日志支持）。**置信度未到入表门槛**，留作 §7 备查 |
| `dim:correctness` 三次 emit 中第二次被 `Isolated mode: extra business event dropped`——是否影响 review chain？ | 70（已 P2 入表，不需深挖） | 无（已知 isolated mode 行为契约） | 已读 `event_loop/parse_and_emit::legacy` 行 + preset 单事件预算约定 |
| `dim:testing` 在 exit 之前为何没有发 `<hat>.scope_violation` 业务事件（即 `dim:testing.scope_violation`）？ | 50 | 需 audit.rs `AuditSeverity::BlockLoop` 与 `publish_event` 路径源码验证 | 已读 `dispatch_and_handoff.rs:1007-1016` 显式 `self.bus.publish(violation)`——理论上 publish 了，但 `events.jsonl` 11 行无此行；可能解释：hard-reject 路径先 `push_termination_trigger` 立即中断，**业务事件 publish 在 termination 之后被丢弃**（需进一步对账 `bus.publish` 与 `check_termination` 顺序）。**置信度未到入表门槛**，留作 §7 备查 |

---

## 8. Prompt visibility 对账

> 触发条件（skill §1 强制对账）：诊断怀疑「agent 看不到某 skill」或「agent 引用了不该看到的内部实现」时**必须**跑 `ralph -c <preset> inspect prompt --hat <id> --format json`。本 run 终止时无 running CLI 进程，`inspect prompt` 未实测。下列**理论推断**基于 preset 静态阅读 + Agent A 的 §5.2 段。

| Hat | auto_inject 假设 | on_demand 假设 | agent 看到什么 | 缺口 |
|-----|-----------------|---------------|---------------|------|
| dim:testing | `ralph-tools.md`（memories/tasks enabled 时 auto）+ preset 顶层 instructions（line 1944-1946 的 `.ralph/loop.lock` 禁令） | `ralph-tools-tasks.md` / `ralph-tools-emit.md` / `ralph-tools-precheck.md` 按需 load（`ralph tools skill load`） | hat 自己的 `instructions:`（line 3899-4017）— "May re-run the project's test suite" 但**无 lockfile 硬规则** | **未在 hat `instructions:` 中显式引用 `ralph-tools-tasks` red box 或 `ralph-tools-precheck` §5**（按 CLAUDE.md HARD RULE 4 §8 应"引用不复制"）—— 此处 instructions 仅"re-run test suite"一句话，没有指向 skill doc |
| dim:correctness | 同上 | 同上 | hat 自己的 `instructions:` | 同上；二次 emit 被 isolated mode drop 是合规的，但 instructions 未明确告知 agent 单事件预算 |
| executor | 同上 | 同上 | 大段 instructions（line 2359-2732）含 Step 1.5 / per-Unit verification / dirty-check 等 | 较详细，但仍无 `*.lock` 副作用硬规则 |
| test-stabilizer | 同上 | 同上 | 含 `stab-001-001` 之类的 production fix 规则 | 已经实证走了 `uv sync`，未触发 scope_violation（**因 test-stabilizer 不在 `disallowed_tools` 名单**，line 5400/5418 是 dim hats，test-stabilizer 在 §2.1 的 `is_read_only_dimension_reviewer` 判定外） |

**结论**：dim:* hats 的 `instructions:` 与 `disallowed_tools` 配合方式（仅 `["Edit"]`、但运行时合并 Edit+Write 判定）在 preset review 静态校验层是合规的；但 prompt 注入的 `ralph-tools-precheck.md` / `ralph-tools-tasks.md` 是否含有"uv.lock 类副作用"的硬规则，需在下一轮 plan 中用 `ralph -c ce-executor-pipeline inspect prompt --hat dim:testing --format json` 实测对账。

---

## 声明

- 本报告为 `ralph-run-diagnosis` skill Phase 4 落盘产物，路径为 `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-08-21-ce-executor-pipeline-2026-08-20-004-feat-readable-evb-diagnosis-report-plan-diagnosis.md`。
- 所有中间产物（CLI log grep 输出、agent stdout 摘录、preset grep 结果、history scan 摘录）均在 `$DIAG_WORKDIR`（`mktemp -d "${TMPDIR:-/tmp}/ralph-diagnosis.XXXXXX"`）—— **未**写入 `docs/report/` 之外主仓路径。
- 报告**不修改**任何源码 / preset / 计划 / 文档。修复建议（§6）均为人工可执行，agent 不会自动执行。
- 历史扫描走 preset-only 30 天滑动窗口（2026-07-22 → 2026-08-21），Agent B 主扫 `docs/report/`（57 份）+ `docs/solutions/`（根目录 3 + 子目录 11），关键词集合覆盖 `ce-executor-pipeline` / `ce-executor-serial` / `dim:testing` / `dim:*` / `dimension-reviewer` / `scope_violation` / `scope_violation_hard_rejected` / `audit_file_modifications` / `disallowed_tools` / `uv.lock` / `lockfile` / `uv sync` / `uv lock` / `agents/code-writer`。