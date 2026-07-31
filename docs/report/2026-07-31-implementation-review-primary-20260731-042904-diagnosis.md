---
title: "implementation-review Loop `primary-20260731-042904` 运行链路诊断报告"
date: 2026-07-31
type: diagnosis
loop_id: primary-20260731-042904
preset: builtin:implementation-review
plan: docs/plans/2026-07-30-004-refactor-unified-execution-contract-plan.md
run_dir: .
config: ralph.implementation-review.yml
status: 已定位 + 最小 patch 已应用 + 全量 nextest 通过
diagnostics_mode: FULL
history_search: disabled
execution_capabilities: ["supervisor", "wave"]
---

# implementation-review Loop `primary-20260731-042904` 运行链路诊断报告

> **生成时间**: 2026-07-31
> **诊断对象**: 主仓 `.ralph/`（loop_id=primary-20260731-042904，启动 12:29:03 → TUI exit 12:46:07）
> **运行命令**: `ralph run -H builtin:implementation-review --plan docs/plans/2026-07-30-004-refactor-unified-execution-contract-plan.md -c ralph.implementation-review.yml`
> **对照 preset**: `presets/en/implementation-review.yml` + `presets/schemas/implementation-review.yml`
> **对照 plan**: `docs/plans/2026-07-30-004-refactor-unified-execution-contract-plan.md`
> **执行方式**: 主 Agent Phase 0 + 源码归因；**`history_search=disabled`，Agent B 跳过**
> **Diagnostics 模式**: FULL（loop 主 ledger 完整、recovery/drift 存在但空）
> **history_search**: `disabled`（用户授权指令：定位 + 修复 + 报告，未要求跨 run 历史）
> **execution_capabilities**: `["supervisor", "wave"]`（capability 信号链：`event_loop.supervisor.max_concurrent_workers: 6`（preset 自带）+ hat `review-worker.concurrency: 6` + `.ralph/supervisor.db` 真实存在 135KB + events `wave_id=w-rs-1` 出现 + 日志 `U3/U5/U6` 字样）
> **报告仓库**: `ralph-orchestrator` 主仓（非 worktree）
> **Tier C 根**: `.ralph/review/2026-07-30-004-refactor-unified-execution-contract-plan/`（dimensions/ 不存在 / synthesized-review.md 不存在 / fix-plan.md 不存在）
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70（见 confidence-rubric）

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 / 大小 | 备注 |
|------|------|------|------|------|
| S | `current-events` → `.ralph/events-20260731-042904.jsonl` | ✅ | 20 行 | 1 `review.start` + 1 `scope.ready` + 6 `review.unit.ready` + 12 `review.unit.done`（**无** `review.wave.complete` / `review.wave.failed` / `LOOP_COMPLETE`） |
| S | `ledger.jsonl` | ✅ | 2 | iteration 1 / 2 各一条 `loop.batch_sync`，无 matching 终止 |
| S | `recovery.jsonl`（workspace） | ✅ | 1 | `agent_doc_sync: sync_up_to_date` info-level，无重试 |
| S | `loops.json` | ✅ | 1 | loop_id=primary-20260731-042904，**`loops: []` 数组为空**（loop 已被 cleanup） |
| S | `loop-termination-reason.json` | ✅ | — | **`"fan_in_failed"`** |
| S | `history.jsonl` | ✅ | 140421B | 完整 batch_sync + activation 历史 |
| S | `flow-authority.jsonl` | ✅ | 645B | 13 行 `review_wave` step 记录（scope.ready + 12× review.unit.done） |
| A | `agent/summary.md` | ✅ | — | "Failed: wave fan-in could not reach terminal state" / 17m 3s / 2 iter |
| A | `diagnostics/wave-w-rs-1-slots.json` | ✅ | — | 6 slot **全部** `status=completed`，**`generated_at_kind: "injected_failed"`** |
| A | `diagnostics/2026-07-31T12-29-03/active-activations.json` | ✅ | 2B | `[]`（无 in-flight activation） |
| A | `diagnostics/2026-07-31T12-29-03/diagnosis-summary.json` | ✅ | 623B | drift_finding=0、recovery_count=0，scheduler 队列干净 |
| B | `diagnostics/logs/ralph-...-94795.log`（父） | ✅ | 1466B | TUI 父进程：subprocess exit 256，17m 3s |
| B | `diagnostics/logs/ralph-...-969-94795.log`（子） | ✅ | 6773B | (30 行) **`commit_salvage_projection failed; next tick will retry wave_id=w-2 seam=merge_completed_review_slots_to_main error=fingerprint mismatch (existing=0ee5e87fa61c..., new=a76b8b1853c0...)`** + `U5: review salvage merge failed during InjectedFailed path; refusing to append coord event` + `U6: supervisor fan-in tick completed wave_id=w-rs-1 fan_in=StoreError` + `Wrapping up: fan_in_failed. 2 iterations in 17m 3s.` |
| B | `.ralph/supervisor.db` | ✅ | 135168B | rusqlite 持久 store，`WaveDeliveryState=SalvageCommitted` 但 `salvage_fingerprint=0ee5e87fa61c...` |
| C | `.ralph/review/<plan>/scope-manifest.json` | ✅ | — | scope-freeze 成功（来自 `scope.ready` payload：`dirty_verdict=clean`、`patch_digest=5f082c528cb0e04203c42e91d6a01d4194d7545a7173e38197b9d29fc6cf1d36`） |
| C | `.ralph/review/<plan>/review.diff.patch` | ✅ | — | frozen diff 写入（review.start payload 镜像确认） |
| C | `.ralph/review/<plan>/dimensions/` | ❌ | — | wave 未成功关闭，6 worker 进程级产物未刷盘（reviewer 进程被 runtime 一起 abort） |
| C | `.ralph/review/<plan>/synthesized-review.md` | ❌ | — | review-synthesizer 未触发 |
| C | `.ralph/review/<plan>/fix-plan.md` | ❌ | — | fix-planner 未触发 |
| C | `.ralph/review/<plan>/wave-blocked.md` | ❌ | — | finalizer 未触发 |

**execution_capabilities 推断结果**: `["supervisor", "wave"]` — 信号链：
- preset `event_loop.execution_mode: isolated` + `event_loop.supervisor.max_concurrent_workers: 6`（capability +supervisor）
- preset `mechanism.flow.steps[1].runs: wave.runtime.review`（capability +wave）
- hat `review-worker.concurrency: 6`
- `.ralph/supervisor.db` 真实存在 135KB（capability +supervisor 实证）
- events 含 `wave_id=w-rs-1` 出现 + 6 worker fan-out + 12 done（capability +wave 实证）
- 日志含 `U3 dispatch` `U5 salvage merge` `U6 fan-in tick` 字样（U3/U4/U5/U6 是 supervisor 子单元）

**缺失产物 → 故障判定**（capability-triggered）：
- `.ralph/supervisor.db` 存在 → **预期**（capability +supervisor 必需时已具备）
- events 有 `wave_id` → **预期**（capability +wave 必需时已具备）
- `review.wave.complete` 缺失 → **真缺失**（preset 期望 seam 注入，capability +wave 必需）
- `dimensions/06 个文件` 缺失 → **真缺失**（worker emit 在 dispatcher tick 上已被 salvage helper 拿到 events，但未刷盘到 review/——runtime 进程被 abort 时 `.ralph/agent/events-hat-review-worker-*.jsonl` 同样未持久化）
- `synthesized-review.md` / `fix-plan.md` / `wave-blocked.md` 缺失 → **预期**（上游 trigger 没注入）

**盲区 / 根因置信度硬顶**：
- FULL 模式 → 单项归因可达 90
- `orchestration.jsonl` 未在 `diagnosis-summary.json` 列出（实际上 U5/U6 字样在子进程 log，**不**在 orchestration.jsonl）→ OPAC 深度扣分
- 6 个 review-worker 进程级 activation 产出（`dimensions/<dim>.md`）未刷盘，无法从字节层复核 worker 是否真完成；但 events-20260731-042904.jsonl 中 12 条 `review.unit.done` 主题级记录在主 ledger 中可见（review-dispatcher 发出 review.unit.ready、worker 在自己的 hat-channel 上 emit review.unit.done 后由 dispatcher tick 收集）

---

## 1. 结论摘要（强制四问）

### Q1 — 执行与 OPAC（diagnostics 模式 FULL + OPAC 置信度）

- **OPAC 置信度**: `O(Observe)` 高（6 worker emit + scope.ready 都能在 events 看到）；`P(Precheck)` 高（presets/en/implementation-review.yml 第 1057/1258/1455 行均强制 `ralph-tools-opac` auto-inject + `ralph-tools-emit`/`ralph-tools-wave` on-demand load）；`A(Apply)` 中（worker 6 路径都走到 emit）；`C(Confirm)` 低（dispatcher 在 salvage re-tick 上 Confirm 拒绝，导致整个 wave 死锁）→ **整体 OPAC 置信度 = 65**。
- **Presets 期望链 OK**：scope-preparer → review-dispatcher → review-worker(×6) → review-synthesizer → fix-planner → finalizer。**前 4 步实际执行**；第 5、6、7 步因 dispatcher 在 salvage seam 阻断，trigger payload 不注入主 ledger → 永远不被触发。
- **OPAC 盲区**：dispatcher tick 上的 `--policy-check` Confirm 行为从 `.ralph/diagnostics/logs/...-969-94795.log` 看仅产出 4 条 WARN，未产出 OK/REJECT 等 explicit Confirm 行 → 审计不完整。

### Q2 — 基座机制是否生效

- **Wave dispatch（U3 dispatcher.rs:933-940）**: ✅ 6 worker 全部 spawned + 6 stdout first-line 收到。
- **Wave completion aggregation**: ✅ `Wave completed wave_id=w-rs-1 results=6 failures=0 duration_ms=745443`。
- **InjectedFailed 路径 salvage seam（U5 dispatcher.rs:3141+4260）**: ❌ 严格 gate 阻断。
- **salvage fingerprint strict gate（rusqlite.rs:1240-1248）**: ❌ 在 CoordinationWritten 之后指纹不同 → `InvalidTransition`。
- **`commit_salvage_projection`（memory.rs:937 vs rusqlite.rs:1194）**: ⚠️ 行为分歧——InMemory 只 warn 接受，rusqlite 严格拒绝。生产 100% 命中 rusqlite 严格路径。
- **`fingerprint_lines`（dispatcher.rs:4542）**: ⚠️ SHA-256 over full lines+`\n`，把 ts 这种 **per-call 时间戳** 当作 fingerprint 输入 → 漂移源。

### Q3 — 编排是否合理

- **Preset 编排正确**：6-hat wave topology、`max_iterations=30`、`max_runtime_seconds=14400`、`review-worker.timeout=900`、`completion_promise=LOOP_COMPLETE`、`required_events=[]`（KTD14）。KTD14 在过去阻止过 `review.passed+review.complete` 无限循环，不动它。
- **Supervisor ledger 三阶段协议正确**：first-pass `record_business_projection`（stamping） + second-pass `commit_salvage_projection`（committing）+ `record_coordination_written`（注入 coord 事件）。
- **不合理的点**：salvage helper 写入主 ledger 时**重复生成 envelope 时间戳**（`ts: chrono::Utc::now().to_rfc3339()`），破坏了 strict gate 的"同 batch 同 fingerprint"前提。这个漂移在第一 tick 上无害（首次 commit 是空 → 实），但 `ContinueCollect` 重调度回 InjectedFailed 路径时会触发第二次 commit，fingerprnt 必然不同 → 严格 gate 命中。

### Q4 — 归因（preset / mechanism / agent / compound）+ 根因置信度

| 因子 | 类型 | 贡献 | 置信度 |
|------|------|------|--------|
| `merge_completed_review_slots_to_main` 第 4290-4297 行用 `Utc::now()` 注入 `ts` | **mechanism bug** | 70% | **92** |
| `merge_completed_exec_fix_slots_to_main` 第 4357-4365 行用 `Utc::now()` 注入 `ts`（同模式） | mechanism bug | 10% | 88 |
| `rusqlite.rs:1240-1248` strict gate 在 CoordinationWritten 之后 fingerprint 不同直接拒绝 | mechanism design | 10% | 95 |
| `ContinueCollect` 重调度回 InjectedFailed 路径（dispatcher.rs:2940-2954）触发 salvage re-tick | mechanism design | 10% | 90 |
| `dispatcher.rs:3204-3212` `emit_injected_failed_coord` 把 salvage 错误转 StoreError | mechanism design | 5% | 95 |
| **Compound** (preset/mechanism 主导，无 agent 误操作) | — | — | — |

**根因**：`merge_completed_review_slots_to_main`（review arm）+ `merge_completed_exec_fix_slots_to_main`（exec/fix arm）在序列化 salvage 行时把 `chrono::Utc::now().to_rfc3339()` 注入 envelope `ts` 字段；`fingerprint_lines` 把整个 JSON 行 SHA-256 哈希出 `batch_fingerprint`；`commit_salvage_projection` 在 rusqlite 严格模式下（`CoordinatorWritten` 之后）拒绝 fingerprint 漂移 → `InvalidTransition` → `InjectedFailed` 路径阻断 coord 事件注入 → `fan_in_failure=true` → loop 终止。

**置信度**：**92**（P0 阈值 70 已超）。理由：
- 三层源码对账（dispatcher.rs:4293 → fingerprint_lines:4542 → rusqlite.rs:1242）byte-precise；
- 实测 fingerprint mismatch 字符串与 strict gate 错误格式字面匹配；
- 既有 in-memory store 与 rusqlite store 行为分歧是已知（memory.rs:979-984 的 warn 接受注释），本 run 命中生产路径。

---

## 2. 故障机制链（按时间）

```
T+0   ralph run 启动
      ↓
T+0   scope-preparer 跑（PTY child pid 94850）→ scope.ready emit + review.diff.patch 写盘
      ↓
T+5m  review-dispatcher 跑（PTY child pid 11897）→ 6 review.unit.ready 发出（wave_id=w-rs-1）
      ↓
T+5m  dispatcher tick 检测到 wave → execute_wave_via_supervisor_with_executor
      ↓
T+5m  6 worker spawn (worker 0..5)，每个走 PTY + StreamJson，first stdout line 2077B 收到
      ↓
T+5m→17m  6 worker 并发跑 review（CWD scope 写 dimensions/*.md）
      ↓
T+17m  Wave completed: 6 results, 0 failures, duration_ms=745443
      ↓
T+17m  dispatcher tick：fan_in = run_supervisor_fan_in(...)
      ↓
T+17m  commit_salvage_batch: 12 行 review.unit.done 写盘 + record_business_projection(stamp 0ee5e87fa61c...)
      ↓
T+17m  commit_salvage_projection（首次）: salvage_fingerprint=0ee5e87fa61c..., delivery_state=SalvageCommitted
      ↓
T+17m  coordinator tick: CoordinatorAction::ContinueCollect（terminal_ctx.is_some()）→ 重入 InjectedFailed 路径
      ↓
T+17m  emit_injected_failed_coord: merge_completed_review_slots_to_main 再次调用
      ↓
T+17m  ❌ 第二次序列化：每行 ts = chrono::Utc::now()  # 已与 tick 1 不同 → 整行 SHA-256 已与 tick 1 不同
      ↓
T+17m  commit_salvage_batch: 第二次 record_business_projection OK（Pending → BusinessProjected），但
      ❌ commit_salvage_projection 第二次：existing=0ee5e87fa61c..., new=a76b8b1853c0... → InvalidTransition
      ↓
T+17m  WARN "commit_salvage_projection failed; next tick will retry wave_id=w-2 seam=merge_completed_review_slots_to_main"
      ↓
T+17m  WARN "U5: review salvage merge failed during InjectedFailed path; refusing to append coord event"
      ↓
T+17m  SupervisorFanInOutcome::StoreError
      ↓
T+17m  U6: supervisor fan-in tick completed wave_id=w-rs-1 fan_in=StoreError
      ↓
T+17m  result.fan_in_failure = true → result.completed_count = 0
      ↓
T+17m  event_loop.rs:13848 行 "Wrapping up: fan_in_failed. 2 iterations in 17m 3s."
      ↓
T+17m  loop-termination-reason.json 写盘 "fan_in_failed"
      ↓
T+17m  TUI 父进程 subprocess exit 256 → cleanup 阶段，loop.lock 清理
```

---

## 3. 强制对账：Prompt visibility（本次未触发）

> **触发条件**：诊断怀疑「agent 看不到某 skill」或「agent 引用了不该看到的内部实现」时跑一次 `ralph -c <preset> inspect prompt --hat <id> --format json` 对账。**本次未触发**：events 清晰显示 6 worker 都成功 emit `review.unit.done`（hat-channel 内容），没有"看不到 skill"或"引用内部函数名"的迹象；preset 内的 auto_inject/on_demand 设置（presets/en/implementation-review.yml 第 1057-1063 行）按 OPAC §5 节强制 `ralph-tools-opac` auto-inject + on-demand `ralph-tools-emit`/`ralph-tools-wave` load，标准做法。**N/A**

历史关联列：`N/A (history disabled)`

---

## 4. 历史关联

`N/A (history disabled)` — 用户显式说明不需要跨 run 历史检索。

---

## 5. 修复方案 + 归因置信度

### P0: 最小 patch（已应用并通过全量 nextest）

**文件**：`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`

**变更 1（review arm，原 line 4290-4298）**：移除 `ts: chrono::Utc::now().to_rfc3339()`，salvage 行不再带 envelope 时间戳。

```rust
let record = serde_json::json!({
    "topic": event.topic.as_str(),
    "payload": event.payload.as_str(),
    "hat": "review-worker",
    "source": "review-worker",
    "wave_id": event.wave_id,
    "wave_index": event.wave_index,
});
```

**变更 2（exec/fix arm，原 line 4357-4365）**：同步移除。

```rust
let record = serde_json::json!({
    "topic": event.topic.as_str(),
    "payload": event.payload.as_str(),
    "hat": attribution,
    "source": attribution,
    "wave_id": event.wave_id,
    "wave_index": event.wave_index,
});
```

**理由**：`fingerprint_lines` SHA-256 over 完整行 → ts 漂移必触发 strict gate。`Event` struct（`crates/ralph-proto/src/event.rs:8`）本身没 `ts` 字段，salvage helper 加的 envelope 字段本来就是"装饰"。删 `ts` 让 fingerprint 跨 tick 幂等 → strict gate 通过 → InjectedFailed 路径不再 false-positive StoreError → preset 后续 hat chain 正常触发。

**风险评估**：
- 下游消费者读 ts：`compute_missing_dimensions` / `wave_diagnostics_json` / backscan 都不读 ts（dispatcher.rs:4281 注释已注明）。
- worker 原始 emit 事件带 envelope ts 写在 `.ralph/agent/events-hat-review-worker-*.jsonl`，审计追溯靠 `wave_id`/`wave_index`/`payload` 不依赖 salvage envelope。
- 既有测试 `merge_completed_review_slots_to_main_writes_completed_only`（dispatcher.rs:9139）只断言 `lines.len()==2` 和 `hat` 字段，不读 ts，patch 后仍绿。

**置信度**：**95**（源码 line-precise；既有测试不依赖被删字段；全量 nextest 验证通过）

### 验证（已执行）

1. **编译**：`cargo build -p ralph-cli` ✅（43.80s）
2. **全量 nextest（按 HARD RULE 1）**：`./scripts/run-tests.sh` ✅
   - Phase 1（默认并发 7364 tests）：7364 passed / 0 fail / 34 skipped
   - Phase 2（race-sensitive 串行 23 tests）：23 passed
   - Doctest（ralph-core 23 tests）：19 passed / 4 ignored
   - **总耗时 71s，无回归**
3. **重跑本次 run**：建议在修复后重跑 `ralph run -H builtin:implementation-review --plan docs/plans/2026-07-30-004-refactor-unified-execution-contract-plan.md -c ralph.implementation-review.yml`，验收：
   - `.ralph/loop-termination-reason.json` ≠ `fan_in_failed`，应为 `clean | residual_only | fixes_required | blocked` 之一
   - 主 ledger 多出 1 条 `review.wave.complete`
   - `.ralph/review/<plan>/fix-plan.md` 或 `wave-blocked.md` 落盘
   - 不再卡在 salvage re-tick

### 不采纳的替代方案

- **A. 改 `rusqlite.rs:1240-1248` strict gate 也走 warn 接受**：会让真 fingerprint 漂移被吞（silent-success 反模式），放弃。
- **B. 在 fingerprint 计算前剥除 ts 字段**：字符串级剥除脆弱（序列化顺序、字段顺序、逗号/换行变化都可能让剥除失败），放弃。
- **C. 在 `commit_salvage_batch` 第一次时锁定 fingerprint**：能解决 InjectedFailed 重入，但与 strict gate 期望语义背离（gate 是 defense-in-depth），放弃。

---

## 6. 未核实疑点（< confidence 60 / 盲区）

- **6 个 review-worker 进程级 activation 产物（`.ralph/review/<plan>/dimensions/*.md`）是否真写过盘**：events 中 12 条 `review.unit.done` 仅在 dispatcher collect 层可见，文件级 audit 不可观测。`.ralph/diagnostics/.../active-activations.json` 为空 → runtime 已清理。如需逐 worker 审计，需在重跑后保留 events-hat-review-worker-*.jsonl。
- **OPAC Confirm 行为缺失记录**：salvage re-tick 上 `--policy-check` 没有产出 OK/REJECT 的 Confirm 行；建议在重跑时启用 `telemetry.runtime_diagnosis.prompt_injection_enabled` 之外，把 `task.resume` Confirm 也纳入 telemetry（不在本 plan 修复范围）。
- **预设的 `required_events: []`（KTD14）**:不强制确认 review.wave.complete 注入 → 即便 salvage 阻断也不会让 loop 等不到 trigger。这是合理的（KTD14 是为阻止 review.passed+review.complete 双 trigger 死锁），但也让本故障"silent until wrapped up"。

---

## 7. 下一步建议（修复后）

1. **重跑实现审查 run** 验证 review.wave.complete 注入 + fix-plan.md 落盘
2. **同步更新 `crates/ralph-core/data/ralph-tools-wave.md`** 中关于"salvage 不写 ts"的说明（避免未来有人反向加回去）
3. **BDD scenario 加一条**：`crates/ralph-core/tests/scenarios/*.yml` 增加 `test_salvage_fingerprint_stable_across_retry_ticks`（在 `run_workflow_guard_scenario` 真 EventLoop runner 里断言两次连续 tick 上 batch_fingerprint 相同）
4. **`docs/solutions/integration-issues/`** 增加一篇 `salvage-envelope-ts-fingerprint-drift.md`，作为 compound learning

---

## 附录 A — 关键日志片段

```
2026-07-31T04:33:31.454675Z INFO ralph::loop_runner::wave::dispatcher Wave detected, executing parallel workers
                                                                                            wave_id=w-rs-1
                                                                                            total=6
                                                                                            hat=review-worker
                                                                                            concurrency=6
2026-07-31T04:46:07.236764Z INFO ralph::loop_runner::wave::dispatcher Wave completed
                                                                                            wave_id=w-rs-1
                                                                                            results=6
                                                                                            failures=0
                                                                                            duration_ms=745443
2026-07-31T04:46:07.237982Z WARN ralph::loop_runner::wave::dispatcher commit_salvage_projection failed;
                                                                                            next tick will retry
                                                                                            wave_id=w-2
                                                                                            seam=merge_completed_review_slots_to_main
                                                                                            error=supervisor store error: invalid transition:
                                                                                                  commit_salvage_projection: fingerprint mismatch
                                                                                                  (existing=0ee5e87fa61cb6c81f4018bcef52df19fbb04621567edc5fd89f4f5b850f05f0,
                                                                                                   new=a76b8b1853c0b2dbc6e868683a83e6b453cd7c145d64a8d435f1aad7e74b3de3)
2026-07-31T04:46:07.237991Z WARN ralph::loop_runner::wave::dispatcher U5: review salvage merge failed during InjectedFailed path;
                                                                                            refusing to append coord event
                                                                                            wave_id=w-rs-1 store_wave_id=w-2
                                                                                            error=projection state transition rejected: ...
2026-07-31T04:46:07.238010Z INFO ralph::loop_runner::wave::dispatcher U6: supervisor fan-in tick completed
                                                                                            wave_id=w-rs-1
                                                                                            fan_in=StoreError
2026-07-31T04:46:07.249914Z INFO ralph::loop_runner::runner Completion event LOOP_COMPLETE detected.
2026-07-31T04:46:07.249966Z INFO ralph_core::event_loop Wrapping up: fan_in_failed. 2 iterations in 17m 3s.
```

## 附录 B — wave slots 终态

```json
{
  "elapsed_secs": 746,
  "generated_at_kind": "injected_failed",
  "slots": [
    {"slot_index": 0, "status": "completed", "reason": null},
    {"slot_index": 1, "status": "completed", "reason": null},
    {"slot_index": 2, "status": "completed", "reason": null},
    {"slot_index": 3, "status": "completed", "reason": null},
    {"slot_index": 4, "status": "completed", "reason": null},
    {"slot_index": 5, "status": "completed", "reason": null}
  ],
  "wave_id": "w-rs-1"
}
```

`generated_at_kind: "injected_failed"` + 6 slot 全 completed → 运行时已决定走 InjectedFailed 路径，**但 salvage seam 阻断导致 coord 事件永远不能注入**。