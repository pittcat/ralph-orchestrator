---
title: "feat: ce-executor-serial 协议 SSOT 收敛（EmitResult + 删除 progress-steward）"
type: feat
status: planned
date: 2026-07-06
created: 2026-07-06
execution_model: strictly-sequential-atomic-tdd
origin: docs/brainstorms/2026-07-06-ce-executor-serial-protocol-ssot-convergence-requirements.md
related_plans:
  - docs/brainstorms/2026-07-02-ce-executor-serial-runtime-phase-authority-requirements.md
  - docs/plans/2026-07-04-004-fix-ce-executor-serial-silent-success-p0-p1-plan.md
  - docs/plans/2026-07-04-003-fix-task-cli-acl-coordinator-hats-and-preset-narrowing-plan.md
---

# feat: ce-executor-serial 协议 SSOT 收敛

## Summary

在保留 `presets/schemas/ce-executor-serial.yml` 作为 payload SSOT 的前提下，把 agent 对外协议收敛为 **`ralph emit` 请求 JSON + 统一 `EmitResult` 响应 JSON**；从 `builtin:ce-executor-serial` **删除 `progress-steward`**，stall 改 runtime **fail-close**。本 plan 按 **纯粹串行、绝对隔离、原子 TDD** 拆为 **19 个 Unit（U1→U19）**，每个 Unit 独立闭环后方可进入下一 Unit。

---

## 执行模型（强制）

```
U1 ──闭环──> U2 ──闭环──> … ──闭环──> U19
     ↑ 每个 Unit：RED → GREEN → REFACTOR → 本 Unit 验收命令 exit 0
     ↑ 100% 完结前一个 Unit 才能打开下一个 Unit 的 RED
```

| 规则 | 含义 |
|------|------|
| **严格串行** | 单向流水线 U1→U2→…→U19；禁止交替开发、禁止并行 Unit |
| **绝对隔离** | 每个 Unit 只改「本 Unit 允许文件」；逻辑上为独立孤岛 |
| **禁止前向依赖** | Unit N 不得 import/调用 Unit N+1 才存在的符号；不得写「等 Unit X 做完再测」 |
| **禁止跨 Unit 集成测** | Unit N 的测试 **只断言本 Unit 新增函数的输入→输出**；端到端 / BDD 全量仅 **U19** |
| **原子 TDD** | 每个 Unit 内：**先 RED（测试失败）→ GREEN（最小实现）→ REFACTOR → 验收 exit 0** |
| **无遗留债务** | 边界问题在本 Unit 完结；禁止「留给下一 Unit 接线」——下一 Unit 只可 **调用** 已完结的公开 API |

**验收命令统一入口**（每个 Unit 末尾 **只跑本 Unit 子集**）：

```bash
cargo nextest run -p ralph-core -- <本 Unit 测试名 substring>      # ralph-core 包
cargo nextest run -p ralph-cli --bin ralph -- <本 Unit 测试名 substring>  # ralph-cli 包（串行）
```

**全量基线**：仅 **U19** 跑 `./scripts/run-tests.sh`。

---

## Problem Frame

`ce-executor-serial` 机制叠层、对外不收敛（见 origin）。本 plan 不重做 Hat Completion API；不改动 `ce-executor-supervisor` 的 steward（R14）。

---

## Requirements Traceability

| Origin ID | Plan 落点 |
|-----------|-----------|
| R1–R4 | U1–U9（EmitResult 纯函数 + CLI 分路径） |
| R5–R8 | U10–U11（preset 删 hat + 配置） |
| R9–R10 | U12–U13（runtime stall + shipper fail-close） |
| R11–R12 | U15（preset 路由减法） |
| R15 | U14（ralph-tools-emit） |
| R16 | U17（CLAUDE/AGENTS/zsh） |
| R17 | U16（单场景 BDD） |
| SC1–SC5 | U19（全量 + 金丝雀） |
| Q1 | KTD-1：stall fail-close（U12–U13） |
| Q2 | KTD-2：`emit_result.v1`（U1, U6） |

---

## Key Technical Decisions

### KTD-1：stall = fail-close；`progress_steward.enabled: false` 不唤醒 hat

### KTD-2：`EmitResult` 在 `ralph-core`；CLI 各路径分 Unit 接入，最后由 U19 做全量

### KTD-3：纯函数与 CLI 接线 **分 Unit**——U1–U5 不得触碰 `emit.rs`；U7–U9 各只接一条 CLI 路径

### KTD-4：preset 改动拆为 U10（删 hat）、U11（配置标志）、U15（instructions 减法），避免单 Unit 改半 preset

---

## Unit 总览（19 个，严格线性）

| Unit | 交付物（仅本 Unit 范围） | Origin |
|------|-------------------------|--------|
| **U1** | `EmitResult` / `EmitError` / `EmitHandoff` 类型 + `emit_result.v1` 常量 | R2, Q2 |
| **U2** | `map_policy_report_to_errors` 纯函数 | R2, R3 |
| **U3** | `allowed_next_for_hat_phase` 纯函数（内联 fixture） | R2 |
| **U4** | `handoff_from_fixture_input` 纯函数（内联 JSON fixture） | R2 |
| **U5** | `EmitResult::assemble` 纯函数 | R2 |
| **U6** | `ralph emit --schema EMIT_RESULT` 只读输出 | Q2 |
| **U7** | CLI policy-check **拒收** → stdout `EmitResult` | R2–R4 |
| **U8** | CLI policy-check **通过** → `ok=true, recorded=false` | R4 |
| **U9** | CLI apply **落盘** → `ok=true, recorded=true` | R4 |
| **U10** | preset **删除** `progress-steward` hat 块 | R5, R8 |
| **U11** | preset `progress_steward.enabled: false` + `coordinator_hats: [coordinator]` | R6, R7 |
| **U12** | `progress_steward.enabled==false` 时 **不** publish `loop.stalled` 唤醒 | R9, R10 |
| **U13** | `shipper_reason`：`loop_stalled_max_iterations` **不** recoverable→pass | R9, SC5 |
| **U14** | `ralph-tools-emit.md` EmitResult 字段表 | R15 |
| **U15** | coordinator 删 PHASE GATE / 重复 DO NOT emit | R11, R12 |
| **U16** | 改写 `serial_phase_post_loop_steward_silent.yml`（无 steward） | R17, R10 |
| **U17** | `CLAUDE.md` / `AGENTS.md` / `patterns.md` 9-hat 同步 | R16 |
| **U18** | 子集回归（U1–U17 触达面） | — |
| **U19** | `./scripts/run-tests.sh` + 金丝雀 SC1×3 手工清单 | SC1–SC5 |

---

## Implementation Units

---

### U1. `EmitResult` 数据类型与 schema 版本常量

**孤岛范围**

- **只改**：`crates/ralph-core/src/emit_result/mod.rs`（新建）、`crates/ralph-core/src/lib.rs`（`mod` + `pub use`）
- **禁止**：`ralph-cli`、`emit.rs`、`presets/`、`event_loop/`

**依赖**：无

**RED**（先写测试，预期失败）

```rust
// crates/ralph-core/src/emit_result/tests.rs
// test_emit_result_schema_version_is_v1
// test_emit_result_success_json_roundtrip
// test_emit_error_optional_fields_omitted_in_json
```

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_emit_result_schema_version_is_v1` | 常量 | `EMIT_RESULT_SCHEMA_VERSION == "emit_result.v1"` |
| `test_emit_result_success_json_roundtrip` | 手写 `EmitResult { ok:true, recorded:false, topic:"work.done", phase:"unit_loop", allowed_next:vec![], .. }` | serde JSON 含全部顶层键 |
| `test_emit_error_optional_fields_omitted_in_json` | `EmitError { code, message, field:None, suggested_command:None }` | JSON 无 `null` 字段 |

```bash
cargo nextest run -p ralph-core -- emit_result_types
# 预期：FAIL（模块不存在）
```

**GREEN**：实现 struct + `Serialize` + 常量。

**REFACTOR**：`skip_serializing_if` 空 `handoff` / 空 `activate_next`。

**验收**

```bash
cargo nextest run -p ralph-core -- emit_result_types
```

---

### U2. `map_policy_report_to_errors` 纯函数

**孤岛范围**

- **只改**：`crates/ralph-core/src/emit_result/map_errors.rs`、`emit_result/mod.rs`（`pub use`）
- **禁止**：CLI、`policy_check.rs` 接线（留给 U7）

**依赖**：U1（仅类型）

**RED**

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_map_empty_reason_codes_yields_empty_errors` | `([], [])` | `vec![]` |
| `test_map_pairs_code_and_suggestion_by_index` | `(["a"], ["fix cmd"])` | `errors[0].code=="a"`, `suggested_command=="fix cmd"` |
| `test_map_empty_suggestion_omits_field` | `(["a"], [""])` | 无 `suggested_command` 键 |

```bash
cargo nextest run -p ralph-core -- map_policy_report_to_errors
```

**GREEN** → **REFACTOR** → 同上验收命令。

---

### U3. `allowed_next_for_hat_phase` 纯函数

**孤岛范围**

- **只改**：`crates/ralph-core/src/emit_result/allowed_next.rs`
- **允许只读调用**：`event_loop/phase_authority/whitelist.rs` 的 `allows`（已存在，不修改）
- **禁止**：读磁盘 preset、CLI、handoff

**依赖**：U1

**RED**：测试内 **内联** 最小 `PhaseAuthorityDeclaration` fixture（不读 `presets/en/`）。

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_allowed_next_coordinator_unit_loop` | hat=`coordinator`, phase=`unit_loop`, fixture | 含 `work.ready`，不含 `review.start` |
| `test_allowed_next_unknown_phase_empty` | phase=`nope` | `[]` |

```bash
cargo nextest run -p ralph-core -- allowed_next_for_hat_phase
```

**GREEN** → **REFACTOR** → 验收。

---

### U4. `handoff_from_fixture_input` 纯函数

**孤岛范围**

- **只改**：`crates/ralph-core/src/emit_result/handoff.rs`
- **禁止**：读 `.ralph/`、inspect CLI、`loop_state` 磁盘

**依赖**：U1

**RED**：`EmitHandoffInput` 为 **本模块定义的 plain struct**；测试用硬编码字符串。

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_handoff_all_fields_present` | 全字段 Some | JSON 全键存在 |
| `test_handoff_all_none_yields_skip` | 全 None | `handoff_from_fixture_input` → `None` |

```bash
cargo nextest run -p ralph-core -- handoff_from_fixture
```

**GREEN** → **REFACTOR** → 验收。

---

### U5. `EmitResult::assemble` 纯函数

**孤岛范围**

- **只改**：`crates/ralph-core/src/emit_result/assemble.rs`
- **禁止**：CLI、validation pipeline

**依赖**：U1–U4（仅调用已完结纯函数）

**RED**

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_assemble_rejection` | `ok=false`, errors 非空 | `ok==false`, `recorded==false` |
| `test_assemble_policy_check_ok` | `ok=true`, `recorded=false` | 形状正确 |
| `test_assemble_apply_ok` | `ok=true`, `recorded=true` | `recorded==true` |

```bash
cargo nextest run -p ralph-core -- emit_result_assemble
```

**GREEN** → **REFACTOR** → 验收。

---

### U6. `ralph emit --schema EMIT_RESULT`

**孤岛范围**

- **只改**：`crates/ralph-cli/src/commands/emit.rs`（`--schema` 分支 + 常量 re-export）、`crates/ralph-cli/src/commands/emit_schema_emit_result_tests.rs`
- **禁止**：改 policy-check / apply 主路径（U7–U9）

**依赖**：U1（常量）

**RED**

| 测试名 | 断言 |
|--------|------|
| `test_emit_schema_emit_result_prints_version` | stdout JSON `schema_version == emit_result.v1` |
| `test_emit_schema_emit_result_mutually_exclusive_with_payload` | 与 `--json` 冲突报错 |

```bash
cargo nextest run -p ralph-cli --bin ralph -- emit_schema_emit_result
```

**GREEN** → **REFACTOR** → 验收。

---

### U7. CLI policy-check 拒收 → stdout `EmitResult`

**孤岛范围**

- **只改**：`crates/ralph-cli/src/policy_check.rs`（仅新增 `report_to_emit_result` 桥接）、`crates/ralph-cli/src/commands/emit.rs`（**仅** policy-check 失败 + `--output json` 分支）、`emit_policy_check_reject_json_tests.rs`
- **禁止**：apply 路径、phase/handoff 真读盘——本 Unit **固定** `phase="unknown"`, `allowed_next=[]`, `handoff` 省略

**依赖**：U1, U2, U5

**RED**

| 测试名 | 断言 |
|--------|------|
| `test_policy_check_reject_json_emit_result_shape` | 缺 `task_id` 的 `work.done` → stdout 可解析为 `EmitResult`，`ok=false`，`errors[0].code` 非空 |
| `test_policy_check_reject_json_exit_nonzero` | exit code ≠ 0 |

```bash
cargo nextest run -p ralph-cli --bin ralph -- policy_check_reject_json
```

**GREEN** → **REFACTOR** → 验收。

---

### U8. CLI policy-check 通过 → `recorded=false`

**孤岛范围**

- **只改**：`emit.rs`（**仅** policy-check 成功 + `--output json`）、`emit_policy_check_accept_json_tests.rs`
- **禁止**：写 events.jsonl（U9）；仍用 stub `phase="unknown"`（真 phase 接线可留 REFACTOR 注释，**不得**在本 Unit 引入 U3 磁盘依赖）

**依赖**：U5, U7（复用 JSON 打印辅助函数）

**RED**

| 测试名 | 断言 |
|--------|------|
| `test_policy_check_accept_json_recorded_false` | 合法最小 payload → `ok=true`, `recorded=false`，events 文件行数不变 |

```bash
cargo nextest run -p ralph-cli --bin ralph -- policy_check_accept_json
```

**GREEN** → **REFACTOR** → 验收。

---

### U9. CLI apply 落盘 → `recorded=true`

**孤岛范围**

- **只改**：`emit.rs`（**仅** apply 成功 + `--output json`）、`emit_apply_recorded_json_tests.rs`
- **禁止**：preset 改动、event_loop

**依赖**：U5, U8

**RED**：temp workspace + 最小 `event_policy` enabled 配置 fixture（测试内嵌 YAML 或现有 test helper）。

| 测试名 | 断言 |
|--------|------|
| `test_apply_json_recorded_true` | 合法 emit 后 `ok=true`, `recorded=true`，events.jsonl +1 行 |

```bash
cargo nextest run -p ralph-cli --bin ralph -- apply_recorded_json
```

**GREEN** → **REFACTOR** → 验收。

---

### U10. Preset 删除 `progress-steward` hat 定义

**孤岛范围**

- **只改**：`presets/en/ce-executor-serial.yml`（**仅**删除 `progress-steward:` hat 块 + header 注释 10→9 hat）、`presets/schemas/ce-executor-serial.yml`（Coverage 注释）
- **禁止**：`progress_steward.enabled`、`coordinator_hats`（U11）；`event_loop/mod.rs`（U12）

**依赖**：U9 完结（EmitResult 路径已存在）

**RED**：若 preset 静态测试断言 hat 列表，先改测试预期。

```bash
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
# 预期：FAIL（topology 不一致）或先绿后改断言
```

**GREEN** → **REFACTOR** → 验收：

```bash
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
```

---

### U11. Preset 关闭 steward + 收窄 `coordinator_hats`

**孤岛范围**

- **只改**：`presets/en/ce-executor-serial.yml`（**仅** `event_loop.progress_steward`、`tasks.coordinator_hats`、删除 `business_topics` 中 progress-steward 行）、`ralph.serial.yml`（若 mirror）
- **禁止**：coordinator instructions（U14）；runtime（U12）

**依赖**：U10

**RED** / **GREEN**：`preset_lint` + `fix_unit_task_id` / coordinator_hats 相关测试（若有）。

```bash
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- coordinator_hats
```

---

### U12. Runtime：`progress_steward.enabled==false` 不唤醒 steward

**孤岛范围**

- **只改**：`crates/ralph-core/src/event_loop/mod.rs`（**仅** steward 唤醒分支）、`crates/ralph-core/src/event_loop/tests/progress_steward_disabled.rs`（新建）
- **禁止**：`shipper_reason`（U13）、BDD（U16）

**依赖**：U11

**RED**：内联 `EventLoopConfig { progress_steward: { enabled: false, .. } }`；调用提取后的纯函数或最小 hook；断言 **不** 生成 target=steward 的 `loop.stalled`。

| 测试名 | 断言 |
|--------|------|
| `test_progress_steward_disabled_skips_loop_stalled_wake` | 无 steward hat id 的 bus publish |

```bash
cargo nextest run -p ralph-core -- progress_steward_disabled
```

**GREEN** → **REFACTOR** → 验收。

---

### U13. Shipper：`loop_stalled_max_iterations` fail-close

**孤岛范围**

- **只改**：`crates/ralph-core/src/shipper_reason.rs`、`shipper_reason.rs` 内 `#[cfg(test)]`
- **禁止**：`event_loop/mod.rs`、preset

**依赖**：U12

**RED**

| 测试名 | 断言 |
|--------|------|
| `test_loop_stalled_max_iterations_not_recoverable_pass` | `is_recoverable_plan_blocked_reason` → false 或 pass 路径不触发 |
| `test_recovery_exhausted_bare_literal_not_short_circuit_pass` | 延续既有 fail-close 语义 |

```bash
cargo nextest run -p ralph-core -- shipper_reason_stall_fail_close
```

**GREEN** → **REFACTOR** → 验收。

---

### U14. `ralph-tools-emit.md` EmitResult 章节

**孤岛范围**

- **只改**：`crates/ralph-core/data/ralph-tools-emit.md`（EmitResult 字段表 + 示例 JSON）
- **禁止**：`presets/`、`emit.rs`

**依赖**：U9

**RED**：先写 `scripts/check-cli-doc-drift.sh` 期望项（若本 Unit 包含 drift 规则扩展，则只改 drift 脚本中 EmitResult 段；否则 RED 仅为文档章节存在性人工清单，U18 跑 drift）。

**GREEN** → **REFACTOR**

**验收**：`grep -c EmitResult crates/ralph-core/data/ralph-tools-emit.md` ≥ 1（人工）；完整 drift 在 U18。

---

### U15. Preset coordinator 路由减法（PHASE GATE）

**孤岛范围**

- **只改**：`presets/en/ce-executor-serial.yml`（**仅** `coordinator.instructions` 与重复 DO NOT emit 段）
- **禁止**：其他 hat instructions（除非单行「见 ralph-tools-emit §EmitResult」）；runtime

**依赖**：U14（instructions 引用文档）

**RED** / **GREEN** / **REFACTOR**

**验收**

```bash
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
```

---

### U16. BDD：改写 `serial_phase_post_loop_steward_silent`

**孤岛范围**

- **只改**：`crates/ralph-core/tests/scenarios/serial_phase_post_loop_steward_silent.yml`、`crates/ralph-core/tests/scenarios.rs`（**仅**该场景注册行，若需）
- **禁止**：新建全链路 scenario；`run_workflow_guard` 多场景

**依赖**：U12

**RED**：场景期望 `absent_events: [task.resume]`，config **无** progress-steward hat。

```bash
cargo nextest run -p ralph-core -- serial_phase_post_loop_steward_silent
```

**GREEN** → **REFACTOR** → 验收。

---

### U17. 文档索引 9-hat 同步

**孤岛范围**

- **只改**：`CLAUDE.md`、`AGENTS.md`（`cp`）、`skills/ralph-preset-common/references/patterns.md`、`scripts/ralph-zsh-plugin.zsh`（仅 hat 列表若有）
- **禁止**：Rust 源码

**依赖**：U10

**验收**：`diff CLAUDE.md AGENTS.md` 为空；人工 grep `10 hat` 在 serial 上下文为零。

---

### U18. 子集回归（本 plan 触达面）

**孤岛范围**

- **只改**：无代码（仅跑命令）；或 `scripts/check-cli-doc-drift.sh` 若在 U14 未跑完

**依赖**：U1–U17 全部完结

**验收**（串行执行，**非**全 workspace）：

```bash
cargo nextest run -p ralph-core -- emit_result
cargo nextest run -p ralph-core -- progress_steward_disabled
cargo nextest run -p ralph-core -- shipper_reason_stall_fail_close
cargo nextest run -p ralph-core -- serial_phase_post_loop_steward_silent
cargo nextest run -p ralph-cli --bin ralph -- emit_schema_emit_result
cargo nextest run -p ralph-cli --bin ralph -- policy_check_reject_json
cargo nextest run -p ralph-cli --bin ralph -- policy_check_accept_json
cargo nextest run -p ralph-cli --bin ralph -- apply_recorded_json
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
./scripts/check-cli-doc-drift.sh
```

---

### U19. 全量基线 + 金丝雀 SC1×3（唯一允许的全集成验收）

**孤岛范围**

- **只改**：可选 `scripts/sc1-canary-serial.sh`（命令模板，不伪造结果）
- **禁止**：在本 Unit 修代码——若失败，**新开 defect Unit 插入 U19 之前**（重排编号），不得在本 Unit 混合开发

**依赖**：U18

**验收**

```bash
./scripts/run-tests.sh
```

**手工 SC1×3**（operator，不计入 CI）：

```bash
ralph run -H builtin:ce-executor-serial -p docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md
# 连续 3 次；events 正规链；无 progress-steward activation
```

---

## 禁止事项（全 plan 有效）

- ❌ 在 U1–U5 修改 `ralph-cli` 或 `presets/`
- ❌ 在 U7–U9 修改 preset 或 `event_loop/mod.rs`
- ❌ 在 U10–U17 修改 EmitResult 形状（若需改形状 → 新插 Unit，重跑后续）
- ❌ 任意 Unit 内跑 `./scripts/run-tests.sh`（仅 U19）
- ❌ 任意 Unit 内写「跨 U7+U9+U12」集成测试
- ❌ 并行开发两个 Unit

---

## Scope Boundaries

### In scope

U1–U19 如上。

### Out of scope

supervisor steward、`ralph wave emit` EmitResult、SC1 CI 自动化（deferred）。

---

## Open Questions

| ID | 决议 |
|----|------|
| Q1 | fail-close（U12–U13） |
| Q2 | `emit_result.v1`（U1, U6） |
| Q3 | SC1 手工，仅 U19 |

---

**Plan 路径**：`docs/plans/2026-07-06-001-feat-ce-executor-serial-protocol-ssot-convergence-plan.md`

**执行入口**：`/ce-work` 从 **U1 RED** 开始；U18 通过前禁止宣称 LOOP_COMPLETE。
