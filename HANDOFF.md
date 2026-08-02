# Handoff: 2026-07-07-002 ce-executor-serial runtime protocol stability

**计划**: `docs/plans/2026-07-07-002-fix-ce-executor-serial-runtime-protocol-stability-plan.md`  
**诊断来源**: `docs/report/2026-07-07-ce-executor-serial-primary-20260706-230230-diagnosis.md`  
**分支**: `pittcat-dev`（工作区有未提交改动，见文末文件清单）  
**状态**: 执行已按用户要求停止；Unit 1–9 大部分落地，Unit 10 部分通过，**全量基线未跑**

---

## 1. 目标回顾

将 `ce-executor-serial` 的多层机制收敛为强状态机协议：

- 拒收事件不得写入主 events（R1）
- `LOOP_COMPLETE` honored 后冻结业务流（R2）
- shipper 必须等当前 step 的 validator 终态（R3）
- task ledger 对 `(loop_id, task_key, step)` 幂等（R4）
- 协议违规 bounded retry + fail-close（R5/R9）
- preset instructions 状态表 + 通用 skill docs 分层（R6/R7/R10）

---

## 2. 已完成工作（按 Unit）

### Unit 1 — Accepted-event commit boundary ✅

| 项 | 位置 |
|---|---|
| 纯 helper 模块 | `crates/ralph-core/src/event_loop/accepted_event.rs` |
| `mod` 注册 | `crates/ralph-core/src/event_loop/mod.rs` |
| 测试 | 同文件 `#[cfg(test)]`（7 个） |

**行为**: `CommitDisposition` 区分 `Committable` / `Rejected` / `Ignored`；`classify_accepted` / `classify_rejected` / `classify_ignored` / `from_execution_contract_rejection`。

**验证**: `cargo nextest run -p ralph-core -- accepted_event` 通过。

---

### Unit 2 — Execution contract before main events write ✅

| 项 | 位置 |
|---|---|
| 核心修复 | 将 handoff / `work_done_seen_tasks` 等副作用从 pre-contract 管道移到 **execution contract 之后** |
| Helper | `EventLoop::apply_contract_committed_side_effects()` in `mod.rs` |
| Contract 拒收 wiring | rejection 分支调用 `from_execution_contract_rejection` + `debug_assert!(!is_committable())` |
|  characterization 测试 | `crates/ralph-core/src/event_loop/tests/execution_contract_commit_boundary.rs` |

**根因对齐**: 修复了「contract 拒收前已更新 `work_done_seen_tasks` / handoff tracker」导致 validator 被错误调度、双账本矛盾的问题。

**验证**: `cargo nextest run -p ralph-core -- execution_contract_commit_boundary` 通过（4 个）。

---

### Unit 3 — Terminal-closed guard（纯决策）✅

| 项 | 位置 |
|---|---|
| 纯模块 | `crates/ralph-core/src/event_loop/terminal_closed_guard.rs` |
| API | `classify_topic`, `evaluate_terminal_closed`, `TerminalClosedInput`（含 `is_byte_duplicate`） |

**验证**: `cargo nextest run -p ralph-core -- terminal_closed_guard` 通过（6 个）。

---

### Unit 4 — Wire terminal guard into runtime ✅

| 项 | 位置 |
|---|---|
| 主循环 guard | `evaluate_terminal_closed_for_event` + 主 events 循环内 early `continue` |
| Repair stream | `record_repair_event` 在 terminal 后拒写 repair，发 `event.post_terminal.rejected` |
| 集成测试 | `crates/ralph-core/src/event_loop/tests/post_terminal_rejection.rs` |

**验证**: `cargo nextest run -p ralph-core -- post_terminal_rejection` 通过（3 个）。

**未改**: `repair_stream_sink.rs` 本体（I/O 边界未动；逻辑在 `mod.rs::record_repair_event`）。

---

### Unit 5 — Shipper waits for validator terminal（纯 helper）✅

| 项 | 位置 |
|---|---|
| 扩展 | `crates/ralph-core/src/event_loop/phase_authority/shipper_helper.rs` |
| 新增 | `ShipperValidatorGateContext`, `evaluate_shipper_validator_gate`, `ValidatorTerminalKind` |

**验证**: `cargo nextest run -p ralph-core -- validator_gate` 通过（6 个新增 + 原有 shipper helper 测试）。

---

### Unit 6 — Wire shipper validator gate ✅（缺 dedicated runtime 单测文件）

| 项 | 位置 |
|---|---|
| Loop state | `last_validator_terminal_step`, `last_validator_terminal_kind`, `record_validator_terminal` |
| 记录点 | `test.passed` / `test.failed` 进入 `accepted_log_events` 后更新 snapshot |
| Routing | `shipper_validator_gate_rejects()` 接入 `phase_authority_rejects_shipper_emit()` |

**计划要求但未创建**: `crates/ralph-core/src/event_loop/tests/shipper_waits_for_validator.rs`（runtime fixture 级单测）。  
**替代覆盖**: Unit 10 BDD `ce_executor_serial_shipper_waits_for_validator.yml` 已通过。

---

### Unit 7 — Task ledger idempotency ⚠️ 部分完成

| 项 | 位置 |
|---|---|
| Locus 查找 | `TaskStore::find_by_locus_in_loop`, `live_task_locus()` |
| ensure 幂等 | 同 `(loop_id, step locus)` 复用 live row，不追加第二行 |
| add 拒写 | `task_cli.rs::add_task_with_args` 同 locus 存在时 bail，提示用 ensure |
| execution contract | payload `task_key` 与 ledger `task.key` 不匹配 → `TaskNotFound` 拒收 |

**未完成**:

- `crates/ralph-cli/src/task_cli.rs` 专用 ensure/add 幂等测试（计划列了，未新增独立 test fn）
- `crates/ralph-cli/src/hat_command_policy.rs` 测试同步
- `crates/ralph-core/src/config/tasks.rs` 未动
- legacy 无 `task_key` 路径行为未单独 characterization

---

### Unit 8 — Protocol violation bounded retry ⚠️ 部分完成

| 项 | 位置 |
|---|---|
| Signature 计数 | `LoopState::record_protocol_violation_signature()` |
| Contract 路径 | `TaskNotTerminal` 等拒收时：第一次仍发 `task.resume`；budget 耗尽发 `plan.blocked(reason=protocol_violation_repeated:…)` 且 **不再** 发 retry |

**未完成**:

- `crates/ralph-core/src/correction/mod.rs` 结构化 `required_action` / `forbidden_action` 字段增强（payload 里已有字符串，correction 模块未扩展）
- `crates/ralph-core/src/event_loop/rejection.rs` 未专门接入 protocol signature
- **Dedicated 测试** `protocol_violation_recovery.rs` 未创建
- **BDD** `test_ce_executor_serial_protocol_violation_retry_then_fail_close` **失败**（见 §4）

---

### Unit 9 — Preset + skill docs ✅（CLAUDE/AGENTS 未同步）

| 项 | 位置 |
|---|---|
| Preset 状态表 | `presets/en/ce-executor-serial.yml`（coordinator/executor/validator/shipper/reporter 触发状态表） |
| 静态测试 | `crates/ralph-cli/src/presets.rs::test_ce_executor_serial_protocol_state_tables_unit9` |
| 通用 skill docs | `crates/ralph-core/data/ralph-tools{,-emit,-tasks,-precheck,-recovery-directives}.md` |
| Preset operator skills | `skills/ralph-preset-author/references/{agent-native-model,author-checklist,patterns,finding-rubric}.md` + `skills/ralph-preset-review/references/{agent-native-model,author-checklist,patterns,finding-rubric}.md`（已拆分为各自本地目录,不再有共享 `skills/ralph-preset-common/references/`）|

**未完成 / 未验证**:

- `presets/schemas/ce-executor-serial.yml` — 仅 instructions 变更，**未确认** schema 是否需要同步（计划说 topology 未变则可能不需要）
- **`CLAUDE.md` / `AGENTS.md` 未更新**（git diff 无改动）
- `scripts/check-cli-doc-drift.sh` 未跑
- `scripts/ralph-zsh-plugin.zsh` 未动（preset 名未变，可接受）

**验证**: `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_serial_protocol_state_tables` 通过。

---

### Unit 10 — BDD + 全量基线 ⚠️ 部分完成

| Scenario YAML | `scenarios.rs` 注册 | 结果 |
|---|---|---|
| `ce_executor_serial_runtime_protocol_happy_path.yml` | ✅ | **PASS** |
| `ce_executor_serial_rejects_post_terminal_business_event.yml` | ✅ | **PASS** |
| `ce_executor_serial_shipper_waits_for_validator.yml` | ✅ | **PASS** |
| `ce_executor_serial_task_identity_idempotent.yml` | ✅ | **PASS** |
| `ce_executor_serial_protocol_violation_retry_then_fail_close.yml` | ✅ | **FAIL** |

**失败详情**:

```
Expected event 'plan.blocked' to be seen (accepted), but it was not recorded.
seen_topics: {"task.resume", "work.ready"}
```

→ Unit 8 fail-close 在 workflow guard scenario 路径上未产生预期的 accepted `plan.blocked`；需对齐 mock 序列或 runtime fail-close 写入 accepted events 的路径。

**未做**:

- `./scripts/run-tests.sh` 全 workspace 基线
- `cargo test --doc`
- preset_lint / SSOT byte-equality 全量复验
- SC1×3 operator 金丝雀清单（计划要求手工，未执行）

---

## 3. 架构变更摘要

```
Agent emit candidate
  → validation pipeline (pre-commit)
  → execution contract filter          ← Unit 2：拒收不进 events
  → apply_contract_committed_side_effects  ← handoff/work_done 仅在此后
  → terminal_closed_guard (per event)    ← Unit 4
  → accept_event → accepted_log_events
  → REVIEW_COMPLETE: shipper_validator_gate ← Unit 6
```

**状态权威顺序**（计划设计）已基本落地；Handoff Envelope 字段形态未改。

---

## 4. 已知问题 / 回归风险

1. **BDD protocol violation scenario 失败** — fail-close `plan.blocked` 未进入 accepted events（优先修 Unit 8 + scenario YAML/mock 对齐）。
2. **Unit 6/8 缺 runtime 级窄单测** — 仅 BDD/部分 characterization 覆盖；计划要求的 `shipper_waits_for_validator.rs` / `protocol_violation_recovery.rs` 未建。
3. **非 serial preset 行为** — commit boundary / terminal guard 在全局 event loop；计划说「非回归对象」但 **未跑全量基线** 确认。
4. **`execution_contract.rs` 新增 identity check** — 可能影响已有 replay fixture 若 task_key 与 ledger 不一致；需全量测试暴露。
5. **`task add` 同 locus 硬拒** — 与历史「双行 step-02」诊断对齐，但可能 break 依赖 add 重复创建的测试/脚本。

---

## 5. 下一步（建议顺序）

### P0 —  unblock 计划 closure

1. **修 `test_ce_executor_serial_protocol_violation_retry_then_fail_close`**
   - 读 `ce_executor_serial_protocol_violation_retry_then_fail_close.yml` + `scenarios.rs:1040` 断言
   - 确认第二次 `TaskNotTerminal` 后 `plan.blocked` 应走 accepted path 还是 diagnostics-only；对齐实现或 scenario `expected.events`
2. **补 Unit 8 测试**: `crates/ralph-core/src/event_loop/tests/protocol_violation_recovery.rs`（第一次 correction / 第二次 fail-close / post-terminal 不 retry）

### P1 — 计划验收清单

3. `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
4. `cargo nextest run -p ralph-core -- preset_lint`
5. `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`
6. `cargo nextest run -p ralph-core --test scenarios`（全部 108+ scenario）
7. `./scripts/run-tests.sh` + `scripts/check-cli-doc-drift.sh`
8. 同步 **`CLAUDE.md` ↔ `AGENTS.md`**（若需反映 preset 行为描述变更：`cp CLAUDE.md AGENTS.md` 或反向）

### P2 — 计划缺口补全

9. Unit 7: `task_cli` ensure 双次同 key 测试 + `execution_contract` task_key mismatch 单测
10. Unit 6: `shipper_waits_for_validator.rs` runtime fixture（可选，BDD 已部分覆盖）
11. Unit 8: `correction/mod.rs` 增加 `forbidden_action` / `target_hat` 结构化字段（若 agent prompt 需要）
12. 更新计划文档 Unit checkbox（`docs/plans/2026-07-07-002-...` status）

---

## 6. 验证命令速查

```bash
# 已通过的 targeted 子集
cargo nextest run -p ralph-core -- accepted_event
cargo nextest run -p ralph-core -- execution_contract_commit_boundary
cargo nextest run -p ralph-core -- terminal_closed_guard
cargo nextest run -p ralph-core -- post_terminal_rejection
cargo nextest run -p ralph-core -- validator_gate
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_serial_protocol_state_tables

# Unit 10 新 scenario（4/5 通过）
cargo nextest run -p ralph-core --test scenarios -- test_ce_executor_serial_runtime_protocol_happy_path
cargo nextest run -p ralph-core --test scenarios -- test_ce_executor_serial_rejects_post_terminal_business_event
cargo nextest run -p ralph-core --test scenarios -- test_ce_executor_serial_shipper_waits_for_validator
cargo nextest run -p ralph-core --test scenarios -- test_ce_executor_serial_task_identity_idempotent
cargo nextest run -p ralph-core --test scenarios -- test_ce_executor_serial_protocol_violation_retry_then_fail_close  # FAIL

# 最终基线（未跑）
./scripts/run-tests.sh
```

---

## 7. 工作区改动文件清单（git status 快照）

**新增**:

- `crates/ralph-core/src/event_loop/accepted_event.rs`
- `crates/ralph-core/src/event_loop/terminal_closed_guard.rs`
- `crates/ralph-core/src/event_loop/tests/execution_contract_commit_boundary.rs`
- `crates/ralph-core/src/event_loop/tests/post_terminal_rejection.rs`
- `crates/ralph-core/tests/scenarios/ce_executor_serial_*`（5 个 YAML）
- `docs/plans/2026-07-07-002-...plan.md`（untracked 于会话开始，现 staged/modified）
- `docs/report/2026-07-07-ce-executor-serial-primary-20260706-230230-diagnosis.md`

**修改（核心）**:

- `crates/ralph-core/src/event_loop/mod.rs`（大：commit boundary、terminal guard、shipper gate、protocol retry）
- `crates/ralph-core/src/event_loop/loop_state.rs`
- `crates/ralph-core/src/event_loop/phase_authority/shipper_helper.rs`
- `crates/ralph-core/src/execution_contract.rs`
- `crates/ralph-core/src/task_store.rs`
- `crates/ralph-cli/src/task_cli.rs`
- `presets/en/ce-executor-serial.yml`
- `crates/ralph-core/data/ralph-tools*.md`
- `skills/ralph-preset-common/references/*.md`
- `crates/ralph-cli/src/presets.rs`
- `crates/ralph-core/tests/scenarios.rs`

**未提交 / 未跑 CI**: 所有改动均在 working tree，**无 commit**。

---

## 8. 接手者注意事项

- 计划要求 **Unit 1→10 严格串行**；当前 Unit 8 BDD 失败说明 8↔10 边界未闭合，应先修 8 再宣称 Unit 10 完成。
- **禁止**裸跑 `cargo test -p ralph-cli`；CLI 测试用 `cargo nextest run -p ralph-cli --bin ralph -- …`。
- `data/*.md` 必须保持**通用**（无 serial hat 名 / preset 名）；serial 拓扑只在 `presets/en/ce-executor-serial.yml`。
- 改 preset YAML 后记得 schema parity + embedded SSOT 测试（见 CLAUDE.md HARD RULE）。

---

*Handoff 生成时间: 2026-07-07（会话中止于用户请求）*
