# Handoff: Event Loop & Loop Runner Tests 拆分 (U2c-U5b 阶段)

**日期**: 2026-06-25
**作者**: Ralph Orchestrator sub-agent 群
**计划文档**: `docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md`
**目标分支**: `pittcat-dev`(已合并)
**当前 HEAD**: `eac201cb`

---

## 1. 任务背景与上下文

### 原始问题
- `event_loop/mod.rs` 在 v14 baseline 9 436 行(v15 实测 9 460 行),超 R1 红线(< 1 000 行)
- `loop_runner/tests.rs` 13 392 行 / 235 个测试,超 R1 红线
- `loop_state.rs` 2 196 行 / `rejection.rs` 1 505 行 / `review_step_state.rs` 1 297 行 — 三个文件均破 R1

### 目标
把 mod.rs / tests.rs 两个巨型文件拆成多个主题子文件,每个子文件 ≤ 2 200 行,严格按 plan v14 + v15 baseline 锚定执行,per-U 单 commit + 单验证。

---

## 2. 本轮(2026-06-25)完成的工作

### 2.1 已合并到 pittcat-dev 的 7 个 commit(5c4c90f0 → eac201cb)

| commit | 类型 | 任务 | 关键变更 |
|---|---|---|---|
| `e56e2a98` | docs(plan) | v15 Sub-Note | 记录 U1+U2a+U2b+R4 实际落地数字,作为后续 U2c+ 接力基线 |
| `6b510e07` | refactor(ralph-cli) | **U2c** | `loop_runner/tests/legacy.rs` → `tests/hooks.rs` (2 480 行 / 39 测试) |
| `738af01f` | refactor(ralph-core) | **U3** | `event_loop/types.rs` 填充(252 行),mod.rs -202 行 |
| `8309b7d2` | refactor(ralph-core) | **U4b** | `event_loop/policy.rs` 填充(122 行),mod.rs -97 行 |
| `dfb14e7e` | refactor(ralph-cli) | **U2d** | `loop_runner/tests/legacy.rs` → `tests/hard_gate.rs` (1 183 行 / 26) + `tests/hard_gate_payload_contract.rs` (941 行 / 20) |
| `07939f88` | refactor(ralph-core) | **U5a** | `event_loop/lifecycle.rs` 填充(19 行),迁 `build_state_ledger_from_env` 1 个 free function |
| `eac201cb` | refactor(ralph-core) | **U5b** | `event_loop/termination_impl.rs` 填充(65 行),迁 `format_duration` + `termination_status_text` 2 个 free function |

### 2.2 累计行数变化(对比 v15 baseline = 5c4c90f0)

| 文件 | v15 (5c4c90f0) | 当前 (eac201cb) | Δ |
|---|---|---|---|
| `event_loop/mod.rs` | 9 460 | 9 104 | **-356** (-3.8%) |
| `loop_runner/tests/legacy.rs` | 9 905 | 5 385 | **-4 520** (-45.6%) |
| 总新增子文件 | 0 | 7 | +7 |

### 2.3 累计新建/填充子文件

#### `event_loop/` 目标子文件(10 个,4 个已填充):
- ✅ `types.rs` (252 行) — U3
- ✅ `policy.rs` (122 行) — U4b
- ✅ `lifecycle.rs` (19 行) — U5a
- ✅ `termination_impl.rs` (65 行) — U5b
- ⏳ `workflow_guard.rs` (3 行 placeholder) — **U4a 折叠,见 §3**
- ⏳ `dispatch.rs` (3 行 placeholder) — **U5c 折叠,见 §3**
- ⏳ `prompt.rs` (3 行 placeholder) — U5d 待实施
- ⏳ `diagnostics.rs` (3 行 placeholder) — U5e 待实施
- ⏳ `process.rs` (3 行 placeholder) — U6a-U6f 待实施
- ⏳ `wave.rs` (3 行 placeholder) — U6b 待实施

#### `event_loop/` 6 个红线预留占位(2 行 placeholder,未填充,本轮不动):
- `flow_lifecycle.rs` / `loop_state_active.rs` / `loop_state_history.rs` / `rejection_payload.rs` / `rejection_envelope.rs` / `review_step_gate.rs`

#### `loop_runner/tests/` 已拆分(8 个,5 个已建,3 个待建):
- ✅ `mod.rs` (78 行) — U2a
- ✅ `common.rs` (335 行) — U2a
- ✅ `fake_path.rs` (90 行) — U2a
- ✅ `wave.rs` (3 156 行 / 39 测试) — U2b(已在 pittcat-dev)
- ✅ `hooks.rs` (2 480 行 / 39 测试) — U2c
- ✅ `hard_gate.rs` (1 183 行 / 26 测试) — U2d
- ✅ `hard_gate_payload_contract.rs` (941 行 / 20 测试) — U2d
- ⏳ `suspend.rs` (待建, ~400 行) — U2e
- ⏳ `loop_termination.rs` (待建, ~300 行) — U2e
- ⏳ `recovery.rs` (待建, ~700 行) — U2e
- ⏳ `async_pty.rs` + `pty_user_interactive.rs` + `diagnostics.rs` — U2f
- ⏳ `resolve_loop_id_and_iteration.rs` + `merge_queue.rs` + `prompt_handling.rs` — U2g
- ⏳ `event_logging_and_planning_session.rs` + `late_events_and_hat_selection.rs` + `event_pipeline.rs` + `preset_lint_gate.rs` — U2h(完成后删除 `legacy.rs`)

### 2.4 关键契约保护(锁点全部通过)

- **总测试数 235**(U2a+U2b+U2c+U2d 后不变,v14/v15 baseline 一致)
- **4 个 process-global Mutex 位置未变**:
  - `tests/fake_path.rs:25/29` — `FAKE_PATH_BACKEND_SERIAL` / `FAKE_PATH_BACKEND_BIN` (R5 形式逐字节不变)
  - `wave/acp_mock.rs:97/102` — `MOCK_ACP_EXECUTIONS` / `MOCK_ACP_EXECUTION_SERIAL` (R3 锁点)
- **EventLoop 字段数 15**(注意:**plan v14 baseline 说 14,实际是 15**,因 R3 plan 2026-06-14-003 后加了 `ephemeral_isolation` 字段,详见 §4 已知漂移)
- **TerminationReason 18 变体顺序未变**(U3 字节级锁定,awk count = 18)
- **R3 公开 API 路径未变**:`loop_runner::*` 短名调用方 0 改动(以 `pub use` 转发保证)
- **零回归**:`cargo nextest run --workspace --exclude ralph-e2e` 4729 passed / 13 skipped / 0 failed
- **295/295 loop_runner tests 通过**(本次拆分的目标子集)

---

## 3. 关键决策与折叠记录

### 3.1 U4a 折叠(U4a 不产出 commit,折叠到 U5/U6)

**决策**:U4a sub-agent 严格 grep 后发现 `crates/ralph-core/src/event_loop/mod.rs` 中已无 workflow guard free function — 所有相关 helper(`check_workflow_guard_completion` 在 6092-6145,`log_workflow_guard_rejection` 在 2819-2884)**都是 `impl EventLoop` 方法**,按 plan v14 对抗审查修正"impl 方法不迁,留 U5"原则,**U4a 无可迁目标,跳过**。

**正确路径**:`workflow_guard.rs` 的内容由后续 U5/U6 阶段(impl EventLoop 块整体迁移)填充。

**注意**:commits `7a921289` (U1) + `c64882f6` (U2a) 的 commit message 中描述"`hooks::termination::dispatch_pre/post_loop_termination_hooks` 改为 `pub(super) fn`"实际**未改**,仍是 `pub fn`。这是 commit message / 代码描述漂移,**本轮无影响**(295 测试全过),但 U2c 实施时需注意:`pub fn` 不收紧意味着 E0659 风险在后续拆 `hooks.rs` 时可能重现。

### 3.2 U5c 折叠(U5c 不产出 commit,折叠到 U5b+U5d)

**决策**:U5c sub-agent grep 发现 mod.rs 中已无 event dispatch helper free function。5 个剩余 free function 列表(行号 = U5a 实施后):
- `filter_human_guidance_blocks` (行 248) → U5d prompt.rs / U6
- `format_duration` (行 8880) → U5b(本轮已实施)
- `termination_status_text` (行 8896) → U5b(本轮已实施)
- `run_stall_detector_on_state` (行 8953) → U5e diagnostics
- `is_rejection_stale` (行 9124) → recovery / diagnostics(TBD)

4 个 dispatch 相关 fn(`publish_event` / `publish_terminate_event` / `process_events_from_jsonl` / `process_events_from_jsonl_with_waves`)全部是 `impl EventLoop` 方法,按任务约束不迁。

**正确路径**:`dispatch.rs` 的内容由后续 U5 阶段(impl EventLoop 块整体迁移)填充。

### 3.3 命名冲突已避免

- `event_loop/termination.rs` (SSOT, 152 行) ≠ `event_loop/termination_impl.rs` (U5b 新建, 65 行) — **两文件并存,语义不同**,不冲突
- `event_loop/dispatch.rs` ≠ `loop_runner/tests/hooks.rs` — namespace 不同

### 3.4 U2e 部分已迁(未 commit,已 kill)

U2e sub-agent 在被 kill 之前**已经**把部分 recovery 测试迁到 `tests/recovery.rs`(约 250 行),但未 commit,worktree 已 force 删除。后续接手者**不要假设** `tests/recovery.rs` 已存在 — 该文件当前在 pittcat-dev 上**不存在**,legacy.rs 中的 recovery 测试**完整保留**。

---

## 4. 已知漂移 / Plan v14 baseline 已过期项

### 4.1 EventLoop 字段数 14 → 15

plan v14 baseline 记录 `EventLoop` 14 字段,但 U3 sub-agent 实测 **15 字段**。新增字段 `ephemeral_isolation: EphemeralIsolation`(在 `event_loop/types.rs:251` 附近),由 R3 plan `2026-06-14-003` 落地,**未在 plan v14 内**。

**影响**:plan 中 R-Refactor-2 锁定的 14 字段需更新为 15 字段;U3 实施时**字段顺序仍字节级一致**(仅多 1 字段),commit `738af01f` 已记录此漂移。

### 4.2 mod.rs 实际行数 9 436 → 9 460(U1 时)→ 9 104(本轮)

plan v14 锚定 `mod.rs` 9 436 行,实测:
- v15(5c4c90f0)= 9 460(+24,U1 scaffold 24 行 mod 声明)
- 本轮(eac201cb)= 9 104(U3+U4b+U5a+U5b 累计 -356)

**注意**:本轮 mod.rs 9 104 行**仍超 R1 红线(< 1 000 行)**,需继续实施 U3+U4+U5+U6 剩余单元。

### 4.3 process_parse_result 边界

plan v14 锚定 `process_parse_result` 行号 6 147-8 696(~2 550 行)。**未在本轮实施**(U6a-U6f 待实施),行号因 mod.rs 减 356 行后**整体前移**,需在 U5d 实施前重新 grep 当前行号。

### 4.4 5 个 free function 行号

U5a sub-agent 报告 + U5c sub-agent 验证后,mod.rs 中**剩余 3 个 free function**(本轮 U5b 已迁 2 个):
- `filter_human_guidance_blocks` (行 248) → U5d
- `run_stall_detector_on_state` (行 8953) → U5e
- `is_rejection_stale` (行 9124) → recovery / diagnostics

(行号为 U5a 实施后实测值,U5b 实施后再前移 ~49 行)

### 4.5 commit message 描述漂移

- `c64882f6` (U2a) 描述"`hooks::termination::dispatch_pre/post_loop_termination_hooks` 改为 `pub(super) fn`" 实际未改,仍是 `pub fn`
- 后续 U2c 实施时如发现 E0659,根因可能是这个描述漂移

---

## 5. 尚未完成的工作(按 plan 顺序)

### 5.1 loop_runner/tests/ 拆分剩余 4 批(U2e-U2h)

| 单元 | 子文件 | 估算行数 | 当前 legacy 行数 | 备注 |
|---|---|---|---|---|
| **U2e** | suspend + loop_termination + recovery | 400+300+700 = 1 400 | 5 385 | **最高优先级**,legacy 减至 ~4 000 |
| **U2f** | async_pty + pty_user_interactive + diagnostics | 550+300+550 = 1 400 | ~4 000 | |
| **U2g** | resolve_loop_id_and_iteration + merge_queue + prompt_handling | 600+450+350 = 1 400 | ~2 600 | |
| **U2h** | event_logging + late_events + event_pipeline + preset_lint_gate | 600+600+400+650 = 2 250 | ~1 200 | **完成后删除整个 `legacy.rs`** |

### 5.2 event_loop/mod.rs 拆分剩余(U4a+U5c 已折叠,U3+U4b+U5a+U5b 已完成)

| 单元 | 子文件 | 内容 | 状态 |
|---|---|---|---|
| **U5d** | `event_loop/prompt.rs` | `filter_human_guidance_blocks` + `append_runtime_config_block` 2 free function + `process_parse_result` 整体迁移 | 待实施 |
| **U5e** | `event_loop/diagnostics.rs` | `run_stall_detector_on_state` 1 free function | 待实施 |
| **U6a** | `event_loop/process.rs` | 写 `process_parse_result` characterization tests | 待实施 |
| **U6b** | `event_loop/wave.rs` | wave 子系统代码 | 待实施 |
| **U6c-U6f** | `event_loop/process.rs` | 抽 6 个 validation 函数:`validate_scope_enforcement` / `validate_origin_guard` / `validate_topic_format` / `validate_event_policy` / `validate_state_machine` / `validate_step_handoff_gate` / `validate_workflow_guard` / `validate_execution_contract` | 待实施 |
| **impl EventLoop 块**整体迁移 | workflow_guard.rs + dispatch.rs + 其他 | check_workflow_guard_completion / log_workflow_guard_rejection / publish_event / process_events_from_jsonl 等 | 待实施(U4a+U5c 折叠内容) |

### 5.3 U7 收尾(全量 verify + docs + lessons)

- 跑 `./scripts/run-tests.sh` 全量基线
- Plan 末尾追加 v16 Sub-Note,记录全部 U3+ 落地数字
- 写 lessons learned,关闭 plan
- 合并 `ralph/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-bold-cork` 与 `pittcat-dev` 主线

---

## 6. 给下一位接手者的明确指引

### 6.1 启动前必读

1. **Plan 文档**:`docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md` v15 Sub-Note 段(本轮 `e56e2a98` 落地)记录了本轮基线
2. **本 HANDOFF.md**:详细列出已落地 commit + 行数 + 漂移 + 折叠决策
3. **必须遵守的硬规则**:
   - 测试入口:`cargo nextest run`(HARD RULE 1);ralph-cli 走 cli-serial 串行(HARD RULE 2)
   - per-U 单 commit + 单验证(R6)
   - 字节级 diff 验证方法体未变(R5)
   - 不允许重声明 process-global Mutex(R3)
   - `pub use` 转发保证下游调用方 0 改动(KTD2)
   - 严禁手动编辑 `.ralph/` 运行时状态文件

### 6.2 下一批 sub-agent 派工建议(可并行)

**第 3 轮(可并行,不同文件无冲突)**:
- **U2e** (loop_runner/tests/legacy.rs → suspend + loop_termination + recovery) — 注意:已 kill 的 U2e 部分迁过 `recovery.rs`(未 commit),sub-agent 启动前需 grep 确认当前 legacy.rs 中仍含完整 recovery 测试
- **U5d** (event_loop/prompt.rs) — 目标 free function: `filter_human_guidance_blocks` (行 248) + `append_runtime_config_block` (行 333)
- **U5e** (event_loop/diagnostics.rs) — 目标 free function: `run_stall_detector_on_state` (行 8953 附近,需 U5b 后重新 grep)

**第 4 轮**:
- U2f / U2g
- U6a (写 process_parse_result characterization tests) — 这是 U6 系列基础

**第 5 轮**:
- U2h(最终删除 legacy.rs)
- U6b-U6f (process.rs 拆分 process_parse_result)

**收尾**:
- U7 全量 verify + docs + lessons

### 6.3 风险与陷阱

1. **legacy.rs 行数快速变化** — U2e-U2h 每批拆 ~1 400 行,行号频繁漂移,sub-agent 启动前必须 `wc -l legacy.rs` 重新确认
2. **EventLoop impl 块整体迁移** — U4a + U5c 折叠内容需要"整块迁移 impl EventLoop 段",不能只迁单 fn。这部分工作本质是 U5+U6 阶段
3. **process_parse_result 2 550+ 行** — U5d / U6a-U6f 是 plan 中最大单 commit 风险,建议拆细(每 U6 单元只迁 1-2 个 validation 函数)
4. **commit message 描述漂移** — 已有先例(`c64882f6` 说改 `pub(super) fn` 实际未改),写新 commit message 时如实描述实际做了什么,不要承诺未做的
5. **patch fixture 验证** — 每批 U 完成后必须 `cargo nextest run -p ralph-cli --bin ralph -E 'test(loop_runner::)'` 通过 295/295 才能合并

---

## 7. 验证矩阵(本轮已通过的检查)

```bash
# 全部通过(本轮 cherry-pick 后实测)
cargo build -p ralph-core:              0 error
cargo build -p ralph-cli:               0 error (1 pre-existing unused-import warning)
cargo nextest run -p ralph-core:        2582 passed, 1 skipped, 0 failed
cargo nextest run --workspace --exclude ralph-e2e: 4729 passed, 13 skipped, 0 failed
cargo nextest run -p ralph-cli --bin ralph -E 'test(loop_runner::)': 295/295 passed

# 锁点验证
EventLoop 字段数(awk):                    15(plan 14,实际 15,因 R3 漂移)
TerminationReason 变体数(awk):             18 ✓
4 process-global Mutex 位置:               tests/fake_path.rs:25/29 + wave/acp_mock.rs:97/102 ✓
tests/mod.rs 1-50 行字节不变:              diff = 0 ✓
总测试数:                                  235(v14/v15 baseline 一致)✓
mod.rs 累计减少:                            9 460 → 9 104(-356)✓
legacy.rs 累计减少:                         9 905 → 5 385(-4 520)✓
```

---

## 8. 参考资料

- **Plan**:`docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md`(行 1-3030 主体 + 行 3034+ v15 Sub-Note)
- **CLAUDE.md** 项目根 `CLAUDE.md`(HARD RULE 1+2 + Build & Test 速查表)
- **v15 Sub-Note 段**:`docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md` 行 3036+(本轮 e56e2a98 落地)
- **已落地的 sub-agent worktree 位置**:`.worktrees/` 已全部清理完毕
- **已合并的 7 个 commit 列表**:见 §2.1

---

**当前状态**:可继续推进 U2e-U2h + U5d-U5e + U6a-U6f + U7 收尾。mod.rs 9 104 行(仍超 R1)+ legacy.rs 5 385 行(仍超 R1)是后续工作目标。
