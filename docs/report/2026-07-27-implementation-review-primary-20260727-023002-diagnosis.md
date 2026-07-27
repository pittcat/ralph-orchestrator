---
title: implementation-review Loop `primary-20260727-023002` 运行链路诊断报告
date: 2026-07-27
type: diagnosis
loop_id: primary-20260727-023002
preset: builtin:implementation-review
run_dir: ralph-e2e
status: 死锁 — review-worker 6 个 slot 完成维度评估但 review.unit.done emit 被 FlowStepScopeStage 拒，wave fan-in 全部判 empty_worker_result，最终 review.wave.failed，用户 SIGTERM 强杀
diagnostics_mode: LOGS_ONLY
history_search: disabled
execution_capabilities: ["wave", "supervisor-ledger"]
---

# implementation-review Loop `primary-20260727-023002` 运行链路诊断报告

> **生成时间**: 2026-07-27
> **诊断对象**: `ralph-e2e/.ralph/`（loop_id=primary-20260727-023002，启动 → 用户 SIGTERM 终止）
> **对照 preset**: `presets/en/implementation-review.yml` + `presets/schemas/implementation-review.yml`
> **执行方式**: 主 Agent 内联 Phase 0–3（产物清单 + 流程对账 + 源码归因）；disabled 模式跳过 Agent B / L5
> **Diagnostics 模式**: LOGS_ONLY（无 session `2026-*/orchestration.jsonl`，只有 `logs/*.log` + `agent_doc_sync.json` + `wave-w-rs-1-slots.json`）
> **history_search**: `disabled`（用户 Phase 0 后确认）
> **execution_capabilities**: `["wave", "supervisor-ledger"]`（events 含 `wave_id=w-rs-1`，preset `event_loop.supervisor.enabled` 未显式 true，但 `.ralph/supervisor.db` 存在且已被 wave fan-in 写入；属于 default-wave 路径，supervisor ledger 仅作 fan-in 记账）
> **报告仓库**: `ralph-orchestrator` 主仓
> **Tier C 根**: `.ralph/review/2026-06-20-001-feat-python-sort-algorithms-plan/`（scope-manifest / scope-analysis / review-context / review.diff.patch / 6 个 dimensions/*.md / dispatch-batch/payloads.jsonl / 11 个 git-state markers）
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70；LOGS_ONLY 下 OPAC/agent 单项硬顶 50

---

## 0. 产物盘点（Phase 0 必附）

`current-events` → `.ralph/events-20260727-023002.jsonl`（9 行）。`current-loop-id` = `primary-20260727-023002`。`loop.lock` = released（已 SIGTERM 收尾）。

| Tier | 路径 | 存在 | 行数 / 状态 | 备注 |
|------|------|------|-------------|------|
| S | `.ralph/events-20260727-023002.jsonl` | ✓ | 9 | review.start / scope.ready / 6×review.unit.ready / review.wave.failed；**无** review.unit.done / review.wave.complete / LOOP_COMPLETE |
| S | `.ralph/events-history-20260727-023002.jsonl` | ✓ | 1 | 仅 `review.start`（warmup）；main ledger 是 SSOT |
| S | `.ralph/ledger.jsonl` | ✓ | 2 | iter=1 loop.batch_sync / iter=2 loop.batch_sync（一次 sync 已 stop） |
| S | `.ralph/recovery.jsonl` | ✓ | 4 | 全部 `repair-stream`，topic 为 `review.dimension.done` / `review.unit.done`，`reason_code=repair_dispatch`，`source_hat=review-worker` |
| S | `.ralph/loops.json` | ✓ | OK | `jq` 默认访问报错（schema 顶层不是 array），但文件存在 |
| S | `.ralph/diagnostics/logs/ralph-2026-07-27T10-30-02-{771,774}-437496.log` | ✓ | 2 files | TUI 子进程 stderr 落盘；最后一次重要日志 = `02:49:27 Quit intercepted` → `02:49:28 SIGKILL surviving 28 procs` |
| A | `.ralph/agent/tasks.jsonl` | ✓ | 4 | `tasks.enabled=false` → tasks 仅占位（4 行 failed 占位） |
| A | `.ralph/agent/summary.md` | ✗ | — | loop 未到 `LOOP_COMPLETE`；finalizer 准备 emit 时被 SIGTERM |
| A | `.ralph/agent/handoff.md` | ✗ | — | 同上 |
| B | `.ralph/supervisor.db` | ✓ | OK | 11 张表（waves / wave_slots / worker_results / wave_emissions / dispatch_records / slot_resources / compensation_jobs / redrive_requests / …） |
| B | `.ralph/diagnostics/wave-w-rs-1-slots.json` | ✓ | — | snapshot：`wave_id=w-rs-1`，6 slot 全 `failed`，4×`empty_worker_result`（slot 0,1,4,5），2×null（slot 2,3），`generated_at_kind=injected_failed`，`elapsed_secs=744` |
| B | `.ralph/diagnostics/agent_doc_sync.json` | ✓ | 126 B | doctor 快照，无关键差异 |
| B | `.ralph/agent/decisions.md` | ✓ | 157 行 | review-worker hat 自陈：每个 slot 都试 emit `review.unit.done`，被 `flow_unknown_emit` 拒，决议"由 synthesizer 读 disk dimensions/*.md 兜底"（**预设失败模式**） |
| B | `.ralph/current-hat-events` → `.ralph/agent/events-hat-review-dispatcher-primary-20260727-023002-3.jsonl` | ✓ | 0 字节 | dispatcher hat-channel（`iteration=3`）；review-worker hat-channel **缺失** |
| **C** | `.ralph/review/<plan>/scope-manifest.json` | ✓ | — | `dirty_verdict=clean`，`scope_digest=695c3f…`, `patch_digest=ea69f9…`, `review_head_sha=ca719a8…`, `first_implementation_commit_sha=0b2455b…`, `resolved_baseline_sha=ba685b5…` |
| C | `.ralph/review/<plan>/scope-analysis.md` | ✓ | — | decision matrix 命中 "1 candidate + ≥2 independent signals"；plan-file 候选拒绝理由记录 |
| C | `.ralph/review/<plan>/review-context.md` | ✓ | — | plan 复述、9 个 changed files、2 commits in scope |
| C | `.ralph/review/<plan>/review.diff.patch` | ✓ | — | 二进制 frozen diff（`git diff --binary --no-color C^..HEAD`） |
| C | `.ralph/review/<plan>/dimensions/{goal-alignment,correctness,testing,maintainability,project-standards,adversarial}.md` | ✓ | 6 文件 | **全部成功写出**，findings_count = 2 / 1 / 6 / 4 / 1 / 3 = 17 findings（含 p1/p2/p3）；每个文件 frontmatter 完整（plan_name / scope_digest / patch_digest / review_head_sha / handoff_precheck_failed=false） |
| C | `.ralph/review/<plan>/dispatch-batch/payloads.jsonl` | ✓ | 6 行 | idempotency_payload_version=1，scope/patch/head SHA 与 scope.ready 字节一致 |
| C | `.ralph/review/<plan>/git-state-review-worker-{dim}-{start,end}.txt` | ✓ | 10 个（缺 testing-maintainability 部分） | HEAD/porcelain/patch_sha256/scope_sha256 校验通过 |
| **关键缺失** | `.ralph/flow-authority.jsonl` | ✗ | — | **scope.ready accepted 后本应存在 step 推进 ledger；不存在 → resident EventLoop `current_plan_step` 卡在 `scope_freeze`** |

**execution_capabilities 推断信号**：
- preset `mechanism.flow.review_wave` `runs: wave.runtime.review` → +wave
- hat `review-dispatcher` instructions 调 `ralph wave emit`；hat `review-worker` `concurrency: 6` → +wave
- events 含 `wave_id=w-rs-1` → +wave（产物侧二次确认）
- `.ralph/supervisor.db` 存在 + `wave_slots` 表已被 dispatcher 写入（6 slot failed）→ supervisor-ledger（runtime 用 supervisor 账本做 fan-in，但 `event_loop.supervisor.enabled` 未显式 true，**仅** ledger-only 用途）

**缺失产物 → 故障判定**：
- `.ralph/supervisor.db` 缺失 = N/A（capability +supervisor-ledger 时 db 存在属预期，本次未缺）
- `.ralph/flow-authority.jsonl` 缺失 = **P0**（详见 §5 主因）
- `.ralph/agent/summary.md` / `handoff.md` 缺失 = 预期（loop 未到 LOOP_COMPLETE）

**盲区声明**：LOGS_ONLY 模式 → OPAC / agent 单项归因硬顶 ≤50；本报告主要结论不依赖 orchestration 细节（main ledger + supervisor ledger + decisions.md + preset 源码 已构成完整三角证据），P0 置信度仍可达 80。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: **死锁 — 假闭环 silent-success 边界态**。6 个 worker 完整写出 dimension artifact（含 file:line evidence 与 suggested_fix），但**所有** `review.unit.done` emit 被 FlowStepScopeStage 拒；supervisor fan-in 据此判 6/6 `empty_worker_result`；wave `review.wave.failed` → finalizer 接触发准备 emit `LOOP_COMPLETE` → 用户在 02:49:27 TUI Quit 主动 SIGTERM 强杀进程树。
- **P0 / P1 数量**: P0 × 1，P1 × 1，P2 × 1
- **最高优先级根因置信度**: P0-1 = **78** / 100
- **历史关联**: §0.1-占位符 `N/A (history disabled)`（用户 Phase 0 后确认）

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ❌ | workers 走完 Observe / Precheck / Apply，写完 dimension artifact，但 Confirm 阶段（`ralph emit review.unit.done`）被 stage gate 拒；OPAC 半闭环（artifact on disk, event 未落 main） | 72 |
| Q2 | 基座机制是否正常生效？ | ❌ | `append_flow_authority_snapshot`（`event_loop/mod.rs:13955`）在 `AcceptMainBus` 后无条件调用，但 `.ralph/flow-authority.jsonl` 缺失 → resident `current_plan_step` 未推进至 `review_wave` | 78 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ | preset 声明 `scope_freeze → review_wave (side_effect)` 由 `scope.ready` 推进（`presets/en/implementation-review.yml:685-696`）；advance 函数在测试中可用（`event_loop/mod.rs:14947-14960`），但**生产环境**未触发该 ledger 写盘 | 75 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **mechanism** 主因（FlowStepScopeStage + EventLoop step-advance ledger 双账本不一致）；preset 设计意图正确（U6/U7 flow authority end-to-end，注释见 `presets.rs:1128-1143`），但本次 run 实际未生效；agent 行为属合规降级（决策入 decisions.md） | 78（取 §5 主因） |

### 1.3 根因一句话

`scope.ready` 在 main ledger accepted 但未触发 `.ralph/flow-authority.jsonl` 写盘（**生产环境 `append_flow_authority_snapshot` 路径未生效**，与 `presets.rs:1128-1143` U6/U7 设计意图脱节），导致 resident EventLoop `current_plan_step` 永远卡在 `scope_freeze`；review-worker hat 调 `ralph emit review.unit.done` 时 `FlowStepScopeStage` 按 `scope_freeze.allowed_emits`（仅 `scope.ready` / `scope.blocked`）校验拒（`flow_unknown_emit`），6 个 slot 全 `empty_worker_result`，wave `review.wave.failed`，最终被用户 SIGTERM。**置信度 78**（双账本一致：decisions.md worker 自陈 + supervisor.db wave_slots 表 event_count=NULL + preset 源码 `flow_step_scope_stage.rs:156` 拒绝路径 + `event_loop/mod.rs:13955-13980` 写盘函数；LOGS_ONLY 下未做实地 orchestration 三联对账，因此未给 90+）。

### Prompt visibility 对账

> **触发条件**: worker hat 的 emit 失败可能与 on-demand skill 可见性相关（worker 不知道 policy-check 拒后如何 fallback）

`mechanism.flow` 声明在 preset 内（**不是** on-demand skill），worker hat 的 `Step 5 — Emit review.unit.done` 引用 `ralph-tools-emit §5 precheck`（`instructions: |` 段末段 "OPAC reference"），但**未引用** `ralph-tools.md §6 isolated 单事件预算` 或任何"policy 拒后 fallback"段。这是 hat instructions 的设计 gap（详见 §5 P2），与本次 root cause 同源但不是同一个修复点。

> **对账结论**: `inspect prompt` JSON 不在本次 run_dir 残留（`ralph inspect prompt` 需 `--format json` 写盘到 .ralph/.scratch/）。本次凭 hat instructions 文本 + `ralph-tools-emit.md` 自动注入推断（auto_inject），未触发 `agent_skill.inject_claim_false` 或 `agent_skill.leaks_internals` 类违规。

---

## 2. 流程断点还原

### 2.1 时间线（events.jsonl main 9 行）

| ts | topic | hat | wave_id | 备注 |
|----|-------|-----|---------|------|
| 02:30:02 | review.start | loop-bootstrap | — | 由 `loop_runner` 注入；triggered=planner（warmup） |
| 02:34:43 | **scope.ready** | scope-preparer | — | accepted（payload 含完整 scope 标识），但**未触发** `.ralph/flow-authority.jsonl` 写盘 |
| 02:36:40 | review.unit.ready ×6 | review-dispatcher | w-rs-1 | single batch emit（idempotency_key 共享）；payload 字节稳定（与 scope.ready 字段一致） |
| 02:49:13 | **review.wave.failed** | ralph(finalizer) | — | `merged_to_events=1`、`salvage_merged=1`、`expected_total=6`、phase=failed；6 槽 failed 全部 event_count=NULL |
| 02:49:27 | (TUI Quit) | — | — | 用户主动 SIGTERM，进程树被 SIGTERM + SIGKILL 收尾 |
| (未发生) | review.wave.complete | — | — | 因 6 slot 全 failed 而不会触发（runtime 协调面由 `wave.runtime.review` 注入） |
| (未发生) | review.synthesized / review.blocked | — | — | finalizer 未到激活时机 |
| (未发生) | fix.plan.ready | — | — | 同上 |
| (未发生) | LOOP_COMPLETE | — | — | 同上 |

### 2.2 双账本对账（preset 声明 vs runtime 行为）

| 维度 | 声明（preset 源码） | 实际（main + supervisor ledger） | 一致性 |
|------|---------------------|---------------------------------|--------|
| `scope_freeze → review_wave` 推进触发 | `scope.ready` accepted（`mechanism.flow.review_wave.runs: wave.runtime.review`） | `.ralph/flow-authority.jsonl` **缺失**；resident step 未推进 | ❌ |
| `review_wave.allowed_emits` | `[review.unit.ready, review.unit.done, review.wave.complete, review.wave.failed]` | `FlowStepScopeStage` 仍以 `scope_freeze.allowed_emits=[scope.ready, scope.blocked]` 拒 review.unit.done | ❌ |
| review-dispatcher 6-payload batch | emit 6 个 `review.unit.ready` 共享 idempotency_key（preset `KTD2`） | events-20260727-023002.jsonl 第 3-8 行 6×review.unit.ready，timestamp 全为 02:36:40.487707203 | ✅ |
| review-worker emit review.unit.done | preset `review-worker.publishes=[review.unit.done]`，无 `default_publishes`（silent-success 防护） | decisions.md 自陈：6 次 emit 全部 `flow_unknown_emit` 拒；events.jsonl **0 行** review.unit.done | ❌ |
| Supervisor fan-in → review.wave.complete | 6/6 review.unit.done → wave.runtime.review 注入 review.wave.complete | 6/6 empty_worker_result（event_count=NULL）→ runtime 注入 review.wave.failed（02:49:13，744s elapsed） | ⚠️ 机制按设计行为正确（拒 0-event slot），但触发条件由机制 bug 引发 |
| finalizer emit LOOP_COMPLETE | `finalize.on_any_of=[fix.plan.ready, scope.blocked, review.blocked, review.wave.failed]` | finalizer hat 0 次激活（events.jsonl 无 `LOOP_COMPLETE`） | ⚠️ 用户 SIGTERM 在 finalizer 激活前介入 |

### 2.3 workers 输出与 emit 状态（discrepancy）

| slot_index | dimension | findings_count | dimension_artifact bytes | emit 状态 |
|-----------|-----------|----------------|-------------------------|-----------|
| 0 | goal-alignment | 2 | 2979 | emit rejected (`flow_unknown_emit`) |
| 1 | correctness | 1 | 1165 | emit rejected (`flow_unknown_emit`) |
| 2 | testing | 6 | 6184 | emit rejected (`flow_unknown_emit`) |
| 3 | maintainability | 4 | 4987 | emit rejected (`flow_unknown_emit`) |
| 4 | project-standards | 1 | 1421 | emit rejected (`flow_unknown_emit`) |
| 5 | adversarial | 3 | 5454 | emit rejected (`flow_unknown_emit`) |

**核心矛盾**: 6 维度 review 实际全部成功（artifact 完整、findings 详尽、git-state 校验通过），但 wave fan-in 视角下全部 `empty_worker_result`。**两侧语义错位 = silent-success 边界态**。

---

## 3. 强制对账（preset / mechanism / agent）

| 层 | 评价 | 证据（file:line） | 置信度 | 历史关联 |
|----|------|-------------------|--------|----------|
| **preset 设计** | ✅ 声明正确且完整 | `presets/en/implementation-review.yml:685-696` `mechanism.flow`；`1034-1166` review-worker instructions（含 Step 1-6 + OPAC reference）；`presets.rs:1128-1143` U6/U7 注释断言 scope.ready→review_wave 推进 | 90 | §0.1-占位符 |
| **mechanism 实现** | ❌ `append_flow_authority_snapshot` 在生产路径未生效 | `event_loop/mod.rs:13385-13406` AcceptMainBus 分支（含 advance + snapshot 写盘）；`event_loop/mod.rs:13955-13980` snapshot 写盘函数（仅在 fs::OpenOptions 失败时静默 return）；**生产 ledger 缺失** | 80 | §0.1-占位符 |
| **FlowStepScopeStage gate** | ✅ 按声明严格拒（行为正确但触发条件由机制 bug 提供） | `flow_step_scope_stage.rs:156-170` TRANSITION_TOPICS bypass（仅 source 缺失时生效，本场景 --hat review-worker 不会触发）；`flow_step_scope_stage.rs` 严格 `current_step.allowed_emits` 校验 | 88 | §0.1-占位符 |
| **agent 行为** | ✅ 合规降级 | decisions.md 3 个 slot 自陈（slot 0/4/5）："emit was attempted under all combinations permitted by the hat collection. Every attempt was rejected"；按 wave protocol 写出 dimension artifact 作为 authoritative product | 85 | §0.1-占位符 |
| **CLI policy_check（worker 侧）** | ❌ 与 EventLoop 步进脱节 | `policy_check.rs:1026-1147` `recover_current_plan_step` + `flow-authority.jsonl` 读取路径；事件缺 ledger 时只能从 main topics 重建，**本次重建结果应是 `review_wave`**（见 `presets.rs:1136-1143`），但仍拒 → 进一步证据说明重建逻辑未生效或 workspace_root 不一致 | 70 | §0.1-占位符 |

---

## 4. 关键文件路径（诊断用 SSOT）

| 路径 | 用途 | 引用 |
|------|------|------|
| `presets/en/implementation-review.yml:685-696` | `mechanism.flow` 5 个 step 声明 | §1.3 / §2.2 |
| `presets/en/implementation-review.yml:692-696` | `review_wave.allowed_emits`（含 `review.unit.done`） | §3 |
| `presets/en/implementation-review.yml:1024-1166` | review-worker hat instructions（Step 1-6） | §1 / §5 P2 |
| `crates/ralph-core/src/event_loop/mod.rs:13380-13436` | `AcceptMainBus` / `AcceptRepairStream` / `Reject` 三分枝；`advance_plan_step` + `append_flow_authority_snapshot` 调用点 | §1.3 / §3 / §5 P0-1 |
| `crates/ralph-core/src/event_loop/mod.rs:13955-13980` | `append_flow_authority_snapshot` 写盘函数（OpenOptions + writeln，无显式错误处理） | §1.3 / §5 P0-1 |
| `crates/ralph-core/src/event_loop/mod.rs:14598-14612` | `advance_plan_step` 函数注释（`terminal_when: all_done` 推进逻辑） | §3 |
| `crates/ralph-core/src/event_loop/mod.rs:14947-14960` | `advance_plan_step` 单测：scope_freeze → review_wave 经 scope.ready 推进 | §3 |
| `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:156-170` | TRANSITION_TOPICS bypass（仅 source 缺失时） | §3 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:4479` | `empty_worker_result` 触发条件 `success && events.is_empty()` | §1.1 / §5 |
| `crates/ralph-cli/src/policy_check.rs:3232-3271` | CLI policy_check `flow-authority.jsonl` 读取 + recover_current_plan_step 测试 | §3 |
| `crates/ralph-cli/src/presets.rs:1128-1143` | `U6/U7` flow authority end-to-end 注释（preset 测试断言） | §3 |
| `ralph-e2e/.ralph/agent/decisions.md:19-50` | review-worker slot 0 自陈 emit 失败模式 | §1.1 / §2.3 / §3 |
| `ralph-e2e/.ralph/diagnostics/wave-w-rs-1-slots.json` | 6 slot 全 failed + 4×empty_worker_result | §0 / §1.1 / §2.2 |
| `ralph-e2e/.ralph/supervisor.db` waves / wave_slots / worker_results | `phase=failed`、`merged_to_events=1`、`event_count=NULL ×6`、`worker_results` 0 行 | §0 / §2.2 / §5 P0-1 |

---

## 5. 优先级与置信度

### P0-1：`scope.ready` accepted 后 `.ralph/flow-authority.jsonl` 未落盘，导致 resident EventLoop step 永卡 `scope_freeze`

- **置信度**: **78**
- **归因层**: mechanism（EventLoop 主链 `AcceptMainBus → append_flow_authority_snapshot` 在生产路径未生效）
- **影响**: 6 个 review-worker 调 `ralph emit review.unit.done` 全部被 `FlowStepScopeStage` 拒（`flow_unknown_emit`）；wave fan-in 据此判 6/6 `empty_worker_result`；最终 `review.wave.failed`
- **证据链**（双账本一致）：
  - main ledger `scope.ready` accepted（02:34:43，`hat=scope-preparer`、`triggered=review-dispatcher`、`source=scope-preparer`）
  - 缺失：`.ralph/flow-authority.jsonl`
  - `event_loop/mod.rs:13385-13406` 在 AcceptMainBus 分支**无条件**调用 `append_flow_authority_snapshot`；函数实现（`event_loop/mod.rs:13955-13980`）仅在 fs::OpenOptions 失败时静默 return
  - decisions.md 自陈：worker 调 emit 时 `FlowStepScopeStage` 拒（"resident EventLoop has not advanced current_step past scope_freeze (no flow-authority.jsonl ledger exists and the main ledger's replayed topics end at review.unit.ready)"）
  - preset 注释 `presets.rs:1128-1143` 明确"U6/U7 flow authority end-to-end"是设计意图
- **可能原因**（按可能性排序）：
  1. **TUI 子进程 `core.workspace_root` 与 .ralph/ 实际位置不一致**：TUI spawn 时 cwd 是仓库根，但子进程 EventLoop 拿到的 workspace_root 可能是别的（`<TUI>` 自身 stderr 已观察到 "Config file 'ralph.yml' not found, using defaults"）
  2. **snapshot 写盘时 `path.parent()` 失败**：.ralph 目录存在，但父目录创建或写盘出错被 `let _ = ...` 静默吞掉
  3. **scope.ready 走非 AcceptMainBus 路径**：从 main ledger 看 `source="scope-preparer"`，按理走 main bus，但若 hat 在 isolated mode 下被错误路由到 repair stream（理论上不可能，因 scope.ready 不在 `repair_dispatch` 路径），则不会进 AcceptMainBus
- **历史关联**: §0.1-占位符

### P1-1：review-worker hat `instructions:` Step 5 emit 段未引用 policy-check 拒后的 fallback 行为

- **置信度**: **70**
- **归因层**: preset（hat 视角）
- **影响**: worker 收到 `flow_unknown_emit` 后只能选"按 wave protocol 写入 decisions.md 后 stop"，无法尝试 `--triggered` 修正或 isolated-mode marker fallback（per `wave-emit-marker-fallback` 同构教训）
- **证据**：`presets/en/implementation-review.yml:1144-1166` Step 5 仅一行命令模板 + Step 6 stop；未引用 `ralph-tools-emit.md` red box 的 fallback 段
- **与 P0-1 关系**: 不是同一根因（机制 bug 是 primary，instructions gap 是 secondary 加深点）；修复 P0-1 后此 gap 会变为低优先
- **历史关联**: §0.1-占位符

### P2-1：CLI policy_check 与 EventLoop resident step 推进走两条不同的 step 恢复路径

- **置信度**: **62**
- **归因层**: mechanism（双轨实现）
- **影响**: 即使修复 P0-1（让 flow-authority 落盘），CLI 端的 `recover_current_plan_step` 与 EventLoop resident `current_plan_step` 仍可能因读盘时机不同而短期不一致
- **证据**：`policy_check.rs:1026-1147` 与 `event_loop/mod.rs:13955-13980` 各自实现读 / 写 .ralph/flow-authority.jsonl；`presets.rs:1136-1143` 测试断言 scope.ready→review_wave 推进，但生产路径未覆盖此断言
- **历史关联**: §0.1-占位符

---

## 6. 修复建议（不修代码）

> **本 skill 不修代码**；建议作为后续 plan 立项输入。

### P0-1 修复方向
1. **优先排查**: 在 `ralph-e2e` 复现环境重跑同一 plan + 同 preset，加入 `RALPH_TRACE=1` 或 `telemetry.runtime_diagnosis.write_artifacts=true`，让 orchestration session 落盘，确认 `append_flow_authority_snapshot` 是否被调用 + fs::OpenOptions 失败原因
2. **代码层加固**: `append_flow_authority_snapshot` 函数（`event_loop/mod.rs:13955`）增加显式错误日志（而非 `let _ = ...` 静默吞错）；至少在 OPEN_OPTIONS 失败时写 `recovery.jsonl`（已存在 repair_sink，可复用）
3. **测试层**: 增设端到端测试 `implementation-review happy path` 强制走 default-wave 路径，断言 `.ralph/flow-authority.jsonl` 至少一行 `{"step":"review_wave","topic":"scope.ready"}` 落盘（参考 `presets.rs:1136-1143` 测试断言，但放到 integration test 而非单测）

### P1-1 修复方向
- review-worker hat `instructions:` Step 5 增补：**"If `ralph emit --policy-check` rejects with `flow_unknown_emit`, read `.ralph/flow-authority.jsonl`（if exists）or `recovery.jsonl` for the actual `current_step`; if missing or stale, log to `decisions.md` and stop — runtime fan-in will treat this slot as `empty_worker_result` per the documented wave protocol"**。引用 `ralph-tools-emit.md` §5 precheck + §6 isolated 单事件预算两条 red box。
- 同步更新 `ralph-tools-emit.md` 在 `flow_unknown_emit` 段加一句 "for wave workers: when policy-check rejects, the dispatcher's fallback path expects you to still emit via wave_runtime marker (set RALPH_EVENTS_FILE / unset per `ralph-tools-wave` red box)" — 但本 skill 不属于 `crates/ralph-core/data/` 更新范围，本节作为建议记录。

### P2-1 修复方向
- 让 EventLoop resident 与 CLI policy_check 共用同一份 `load_flow_authority_current_step`（`policy_check.rs:1063` 已 export）；EventLoop 启动时 recover 一次，运行期 advance_plan_step 写盘，CLI 端只读不写

---

## 7. 未核实疑点（confidence<60，未入 §5/§6）

| 疑点 | 原因 |
|------|------|
| TUI 子进程 `core.workspace_root` 与 .ralph/ 实际位置是否一致？ | LOGS_ONLY 模式下无 orchestration session，无法查进程启动参数；需下次重跑加 `RALPH_TRACE=1` |
| `append_flow_authority_snapshot` 是否被实际调用？ | 同上，需 instrumentation |
| scope.ready accepted 时 hat 走的是 `AcceptMainBus` 还是某条 wave-side 路径？ | main ledger 看像 AcceptMainBus，但无 runtime orchestration 三联对账可证 |
| 同一 preset 在其它 5 次历史 run 中是否也存在 flow-authority 缺失？ | history_search=disabled，跳过 |
| 6 次 emit reject 中有 2 次 slot（2,3）failure_reason 为 `null`（其它 4 次为 `empty_worker_result`）的可能原因？ | 推测：slot 2/3 worker hat-channel 落盘路径差异，但缺乏证据；待下次复跑加 instrumentation 验证 |

---

## 提交前检查

- [x] Phase 0 盘点表在报告中（§0）
- [x] 只读了 `current-events` 指向的 events（9 行 + 配对 history 1 行）
- [x] LOGS_ONLY 已声明（§0 顶部 + §1.3 末尾盲区声明）
- [x] §5 每条 P0/P1 附置信度；P0=78 ≥ 70、入表三条均 ≥60
- [x] confidence<60 的候选已落入 §7，未混入 §5/§6
- [x] 未引用 ssot-guardrails 禁止项（hat_handoff、loop_state_snapshot.json、`review.passed`/`review.failed` 链路期望、`human.guidance` topic 均未出现）
- [x] 报告路径: `ralph-orchestrator/docs/report/2026-07-27-implementation-review-primary-20260727-023002-diagnosis.md`
- [x] history_search=disabled 已写入 frontmatter