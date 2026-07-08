---
title: "fix: task CLI SSOT + ensure UX + ACL 收窄 + OPAC 两步式 task verify gate"
type: fix
status: planned
date: 2026-07-04
created: 2026-07-04
deepened: 2026-07-04
revision: 3
execution_model: strictly-sequential-atomic-tdd
related_plans:
  - docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md
  - docs/plans/2026-07-04-002-fix-opac-p0-p1-p2-issues-plan.md
absorbed:
  - "revision-2: U7 两步式 verify gate(用户确认)"
  - "revision-3: 开发计划改为 8 个绝对串行 Unit + 每 Unit 原子 TDD 执行步骤(非 roadmap)"
---

# fix: task CLI SSOT + ensure UX + ACL 收窄 + OPAC 两步式 task verify gate

## 执行模型（强制）

```
Unit 1 ──闭环──> Unit 2 ──闭环──> … ──闭环──> Unit 8
         ↑ 测试全绿才允许进入下一 Unit
```

| 规则 | 含义 |
|------|------|
| **严格串行** | 同一时间只做一个 Unit; Unit N 测试全绿前禁止打开 Unit N+1 的 RED |
| **绝对隔离** | 每个 Unit 只改列出的文件; 测试只断言本 Unit 的输入→输出; 禁止跨 Unit 集成测试 |
| **禁止前向依赖** | Unit N 不得 import/调用 Unit N+1 才存在的符号; 不得写「等 Unit X 做完再测」 |
| **原子 TDD** | 每个 Unit 内部: **RED → GREEN → REFACTOR → 验收命令 exit 0**; 边界问题在本 Unit 完结 |

**验收命令统一入口**(每个 Unit 末尾只跑自己的子集):

```bash
cargo nextest run -p ralph-cli --bin ralph -- <本 Unit 测试名 substring>
# 或 ralph-core 包:
cargo nextest run -p ralph-core -- <本 Unit 测试名 substring>
```

---

## Unit 总览（线性，禁止并行）

| Unit | 交付物 | 测试聚焦（仅本 Unit） |
|------|--------|----------------------|
| **1** | `CoordinatorHatsError` + `load_coordinator_hats() -> Result` | 临时目录 + 假 ralph.yml → Vec 或 Error |
| **2** | `HatCommandPolicy::check_task` 改用外部 `coordinator_hats` slice | 注入 slice + mock ctx → `PolicyDecision` |
| **3** | `EnsureArgs` clap: `--for-fix-unit` 无需 `--key` | clap / handler 解析 → derived key |
| **4** | `TasksConfig` 两字段 serde | YAML 字符串 → struct 字段值 |
| **5** | `task_verify_gate.rs` 纯模块(fingerprint/ticket) | 字符串/临时 ticket 文件 → allow/deny |
| **6** | gate 接入 `execute_verify` + `execute_add` + `execute_ensure` | agent env + 假 ticket → gate deny/allow |
| **7** | `ce-executor-serial` coordinator_hats 收窄 + presets.rs | preset 字节 + preset_lint |
| **8** | guardrails + skill 文档 + drift 脚本 | 静态文件 + check-cli-doc-drift |

Unit 6 **仅**依赖 Unit 1–5 已合并的代码; Unit 7/8 **不**改 Rust gate 逻辑。

---

## Unit 1 — `load_coordinator_hats` 可区分错误

### 范围（孤岛）

- **只改**: `crates/ralph-cli/src/task_cli.rs`（`CoordinatorHatsError` enum + `load_coordinator_hats` 函数体 + `#[cfg(test)] mod load_coordinator_hats_tests`）
- **禁止**: 改 `execute()`、`check_task`、preset、config、gate
- **编译桥接**: `execute()` 若需编译,临时 `load_coordinator_hats(...).unwrap_or_default()` **允许**,但**不算 Unit 1 验收**; Unit 1 验收只看 mod 内测试

### RED（先写测试，预期失败）

在 `task_cli.rs` 底部新增 **独立** mod（不引用其他待写 Unit）:

```rust
#[cfg(test)]
mod load_coordinator_hats_tests {
    // test_missing_ralph_yml_returns_error
    // test_invalid_yaml_returns_invalid_yaml_variant
    // test_missing_coordinator_hats_key_returns_missing_key
    // test_valid_yaml_returns_hats_vec
}
```

| 测试名 | 输入(临时 workspace) | 断言 |
|--------|---------------------|------|
| `test_missing_ralph_yml_returns_error` | 空目录 | `Err(MissingRalphYml)` |
| `test_invalid_yaml_returns_invalid_yaml` | `ralph.yml` = `tasks: [` | `Err(InvalidYaml{..})` |
| `test_missing_coordinator_hats_key_returns_missing_key` | `tasks:\n  enabled: true` | `Err(MissingKey{..})` |
| `test_empty_coordinator_hats_returns_missing_key` | `coordinator_hats: []` | `Err(MissingKey{..})` |
| `test_valid_yaml_returns_hats_vec` | `coordinator_hats: [coordinator, executor]` | `Ok(vec![...])` |

```bash
cargo nextest run -p ralph-cli --bin ralph -- load_coordinator_hats_tests
# 预期: 全 FAIL(函数仍返回 Vec 或 enum 不存在)
```

### GREEN

1. 定义 `CoordinatorHatsError` 四 variant
2. `fn load_coordinator_hats(root) -> Result<Vec<String>, CoordinatorHatsError>` — 各分支 `?`/return Err, **禁止** `continue` 吞错
3. 跑测试至全绿

### REFACTOR

- 错误类型实现 `Display`/`Error` hint 方法(供 Unit 2 用,本 Unit 测试不依赖 hint 文案)

### 验收（Unit 1 完结门槛）

```bash
cargo nextest run -p ralph-cli --bin ralph -- load_coordinator_hats_tests
```

**完结定义**: 5 个测试全绿; **未**要求 `execute()` 行为变化。

---

## Unit 2 — `check_task` 外部 coordinator_hats slice

### 前置

Unit 1 已合并且 `load_coordinator_hats_tests` 全绿。

### 范围（孤岛）

- **只改**:
  - `crates/ralph-cli/src/hat_command_policy.rs` — `check_task(ctx, coordinator_hats, coordinator_err, verb)`
  - `crates/ralph-cli/src/task_cli.rs` — `enforce_command_policy` 签名 + `execute()` 调用 `load_coordinator_hats` 一次并传入
- **禁止**: EnsureArgs、gate、preset、TasksConfig 新字段
- **测试 mod**: `hat_command_policy.rs` 内 `#[cfg(test)] mod check_task_coordinator_hats_tests` — **只**构造 `OperationContext` + `&[String]` slice, **不**读磁盘 ralph.yml

### RED

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_agent_worker_not_in_slice_denied_lists_slice` | hats=`[coordinator]`, hat=executor, verb=add | `Deny`, hint 含 `coordinator`, **不含** `empty in ralph.yml` |
| `test_agent_coordinator_in_slice_allowed` | hats=`[coordinator]`, hat=coordinator | `Allow` |
| `test_empty_slice_denied_with_missing_key_hint` | hats=`[]`, err=`Some(MissingKey)` | hint 含配置修复指引, 非 generic empty |
| `test_human_cli_always_allowed` | `is_agent_context=false` | `Allow` |

```bash
cargo nextest run -p ralph-cli --bin ralph -- check_task_coordinator_hats
# 预期 FAIL
```

### GREEN

1. `check_task` 不再读 `config.tasks.coordinator_hats`
2. `execute()` 一次 `load_coordinator_hats`; agent path 将 Err 转为 hint; human path `unwrap_or_default()` + warning
3. 测试全绿

### 验收

```bash
cargo nextest run -p ralph-cli --bin ralph -- check_task_coordinator_hats
cargo nextest run -p ralph-cli --bin ralph -- enforce_command_policy_empty
# 仅跑已有、与本 Unit 相关的 policy 子集(若仍绿则通过)
```

---

## Unit 3 — `EnsureArgs` 一行 `--for-fix-unit`

### 前置

Unit 2 完结。

### 范围（孤岛）

- **只改**: `crates/ralph-cli/src/task_cli.rs` — `EnsureArgs` clap + `ensure_task_with_args` 内 key 解析
- **禁止**: gate、TasksConfig、preset、load_coordinator_hats、check_task
- **测试 mod**: `ensure_for_fix_unit_clap_tests` — 直接调 `ensure_task_with_args` + 内存 `TaskStore`, **不**测 gate/ACL deny

### RED

| 测试名 | 输入 | 断言 |
|--------|------|------|
| `test_ensure_for_fix_unit_derives_key_without_explicit_key` | `for_fix_unit=plan:fix-01:slug`, title, key=None | task.key == `ce-executor:plan:fix-01:slug` |
| `test_ensure_for_fix_unit_pins_owner_coordinator` | 同上 | owner_hat_id == coordinator |
| `test_ensure_explicit_key_still_works` | key=Some(...), for_fix_unit=None | 原行为 |

clap 层 conflicts 测法: 用 `clap` `CommandFactory::debug_assert` 或单独 `try_parse_from` 测 `--key` + `--for-fix-unit` 互斥(可选第 4 测)。

```bash
cargo nextest run -p ralph-cli --bin ralph -- ensure_for_fix_unit
# 预期 FAIL
```

### GREEN

1. `key: Option<String>` + `required_unless_present = "for_fix_unit"`
2. `for_fix_unit`: `conflicts_with = "key"`
3. handler 与 `verify_ensure` derive 逻辑对齐

### 验收

```bash
cargo nextest run -p ralph-cli --bin ralph -- ensure_for_fix_unit
```

---

## Unit 4 — `TasksConfig` gate 配置字段

### 前置

Unit 3 完结。

### 范围（孤岛）

- **只改**: `crates/ralph-core/src/config/tasks.rs` + 若有 snapshot 则同文件测试
- **禁止**: ralph-cli、gate 模块、preset
- **测试**: `tasks_config_gate_fields_tests` — **仅** `serde_yaml::from_str` 内联 YAML

### RED

| 测试名 | YAML | 断言 |
|--------|------|------|
| `test_tasks_config_defaults_require_verify_false` | `{}` 或缺字段 | `require_verify_for_cli_mutate == false`, `allow_unsafe_task_mutate == false` |
| `test_tasks_config_explicit_true` | 两字段 true/false | 精确匹配 |

```bash
cargo nextest run -p ralph-core -- tasks_config_gate
# 预期 FAIL(字段不存在)
```

### GREEN

```rust
#[serde(default)]
pub require_verify_for_cli_mutate: bool,
#[serde(default)]
pub allow_unsafe_task_mutate: bool,
```

### 验收

```bash
cargo nextest run -p ralph-core -- tasks_config_gate
```

---

## Unit 5 — `task_verify_gate` 纯模块

### 前置

Unit 4 完结。**不**要求 Unit 6 存在; **不** wire 到 task_cli。

### 范围（孤岛）

- **只改**: 新建 `crates/ralph-cli/src/task_verify_gate.rs` + `crates/ralph-cli/src/lib.rs` 或 `main.rs` mod 声明
- **禁止**: 改 `execute_add`/`execute_verify`/preset
- **公开 API**(本 Unit 全部测完):
  - `mutation_fingerprint(verb, canonical_payload: &str, loop_id, hat_id) -> String`
  - `record_ticket(path, fingerprint, loop_id, hat_id) -> Result<()>`
  - `consume_ticket(path, fingerprint, loop_id, hat_id) -> Result<()>`
  - `require_ticket(path, cfg, ctx, verb, fingerprint) -> Result<()>` — 内建 effective_strict 逻辑

### RED（模块内自包含, tempdir ticket 文件)

| 测试名 | 步骤 | 断言 |
|--------|------|------|
| `test_fingerprint_stable_for_same_payload` | 同字符串两次 | 相等 |
| `test_fingerprint_differs_for_different_title` | title 变 | 不等 |
| `test_record_then_consume_ok` | record → consume | Ok |
| `test_consume_without_record_err` | 直接 consume | Err |
| `test_consume_twice_second_err` | consume 两次 | 第二次 Err |
| `test_require_ticket_agent_no_record_denied` | agent ctx, 空 ticket 文件 | Err, prefix `task_verify_gate denied` |
| `test_require_ticket_human_bypass` | human ctx | Ok |
| `test_require_ticket_agent_or_config_strict` | config false + agent true | Err(仍 enforce) |

```bash
cargo nextest run -p ralph-cli --bin ralph -- task_verify_gate
# 预期 FAIL(模块不存在)
```

### GREEN

实现模块; 不 touch task_cli execute 路径。

### 验收

```bash
cargo nextest run -p ralph-cli --bin ralph -- task_verify_gate
```

---

## Unit 6 — gate 接入 add/ensure/verify

### 前置

Unit 5 完结; Unit 1–4 已合并。

### 范围（孤岛）

- **只改**:
  - `crates/ralph-cli/src/task_cli.rs` — `execute_verify`(Add/Ensure Allow 后 record), `execute_add`/`execute_ensure`(写盘前 require, 成功后 consume)
  - 可选: `--unsafe-no-task-verify` flag 仅 add/ensure
- **禁止**: 改 Unit 5 模块语义(只 call API); preset; skill doc
- **测试 mod**: `task_verify_gate_wiring_tests` — 用 **Unit 3 已完成** 的 ensure 或简单 `add "t"`; temp workspace + agent env vars

### RED

| 测试名 | 步骤 | 断言 |
|--------|------|------|
| `test_agent_add_without_verify_denied_no_write` | 无 ticket, `add "x"` | exit Err, tasks.jsonl 行数不变 |
| `test_agent_add_verify_then_add_ok` | verify add → add 同参 | 第二次 Ok, jsonl +1 |
| `test_agent_add_second_add_needs_reverify` | verify → add → add | 第三次 Err |
| `test_human_add_without_verify_ok_with_warning` | 无 RALPH_CURRENT_HAT | Ok, stderr 含 bypass warning |

```bash
cargo nextest run -p ralph-cli --bin ralph -- task_verify_gate_wiring
# 预期 FAIL
```

### GREEN

1. `canonical_payload` 序列化 helper(与 verify/mutation 共享,放 task_cli 或 task_verify_gate 的 `pub(crate)`)
2. 三处 hook: verify record / add|ensure require+consume
3. deny 文案含 `next_step: ralph tools task verify ...`

### REFACTOR

- 去重 verify_add / add 的 payload 构建(若重复)

### 验收

```bash
cargo nextest run -p ralph-cli --bin ralph -- task_verify_gate_wiring
```

**完结定义**: agent 两步 gate 对 `add`+`ensure` 生效; **不测** preset、不测 lifecycle verb。

---

## Unit 7 — preset `coordinator_hats` 收窄

### 前置

Unit 6 完结。

### 范围（孤岛）

- **只改**:
  - `presets/en/ce-executor-serial.yml` — `coordinator_hats` 列表 + 删非 coordinator ensure 模板 + coordinator verify→ensure 文案
  - `crates/ralph-cli/src/presets.rs` EmbeddedPreset content 字节同步
  - `tasks:` 段加 `require_verify_for_cli_mutate: true` / `allow_unsafe_task_mutate: false`
- **禁止**: 改 Rust gate 逻辑
- **测试**: 已有 preset 静态测试(只断言 YAML 内容)

### RED

若已有测试期望 7 hat allowlist,先跑:

```bash
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
# 改 YAML 前可能仍绿; 改后故意不同 → RED → 同步 presets.rs → GREEN
```

新增(可选,同文件):

| 测试名 | 断言 |
|--------|------|
| `test_ce_executor_serial_coordinator_hats_len_two` | embedded YAML parse → 恰好 2 hat |

### GREEN

1. coordinator_hats: `[coordinator, progress-steward]`
2. presets.rs 字节一致
3. preset_lint 绿

### 验收

```bash
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
./scripts/validate-builtin-presets.sh
```

---

## Unit 8 — OPAC 信号与 skill 文档

### 前置

Unit 7 完结。

### 范围（孤岛）

- **只改**:
  - `presets/en/ce-executor-serial.yml:723` guardrails(若 Unit 7 未改完则补)
  - `crates/ralph-cli/src/presets.rs` 同步
  - `crates/ralph-core/data/ralph-tools-cmdref.md`
  - `crates/ralph-core/data/ralph-tools-tasks.md`
  - `crates/ralph-core/data/ralph-tools-opac.md`
- **禁止**: Rust 逻辑变更

### 执行步骤（文档 TDD = drift 脚本作 RED/GREEN）

1. 更新四文件文案(verify→apply 两步 + gate deny 恢复)
2. `ralph tools task verify ensure --help` 冒烟
3. RED/GREEN:

```bash
./scripts/check-cli-doc-drift.sh   # 改前若 fail 则先对齐; 改后必须 exit 0
```

### 验收

```bash
./scripts/check-cli-doc-drift.sh
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
```

---

## 全量完结（全部 8 Unit 之后，非单 Unit 验收）

```bash
./scripts/run-tests.sh
```

**禁止**在 Unit 1–7 中间跑全量基线作为 Unit 验收依据。

---

## 依赖关系（仅向后，禁止前向）

```
Unit1 → Unit2 → Unit3 → Unit4 → Unit5 → Unit6 → Unit7 → Unit8 → 全量
```

- Unit 5 **不**依赖 Unit 6
- Unit 6 **依赖** Unit 5 的 public API（已合并）
- Unit 7 **不**依赖 Unit 8; Unit 8 **不**改 Rust

---

## Out of Scope（全 plan）

- lifecycle verb gate(start/close/fail/reopen) — 另开 plan
- `verify-emit-bridge` gate
- task CLI 读 `-c` config
- inline `--policy-check` on add/ensure
- 跨 Unit 集成测试 / e2e-serial（全量阶段才跑）

---

## 验收示例（映射 Unit）

| AE | Unit |
|----|------|
| load_coordinator_hats 四态错误 | 1 |
| deny 不含 empty 误导 | 2 |
| `--for-fix-unit` 无 `--key` | 3 |
| TasksConfig serde 默认 | 4 |
| ticket record/consume 纯模块 | 5 |
| agent verify→add 两步 | 6 |
| coordinator_hats 2 个 | 7 |
| drift + guardrails | 8 |
