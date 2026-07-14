---
title: 'refactor: 移除 5 个 backend (amp / roo / kiro / kiro-acp / copilot)'
type: refactor
status: active
date: 2026-07-14
origin: 用户硬要求 — 删除 amp / roo / kiro / kiro-acp / copilot 共 5 个 backend 的全部生产代码、测试、fixture、preset、脚本与文档引用。删除后保留: claude / gemini / codex / opencode / pi / traecli / custom (共 7 个)。
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
execution_constraints:
  - 严格串行: 单向流水线 U1 → U2 → … → U8,绝对禁止并行/交叉开发
  - 绝对隔离: 每个 Unit 是一个独立的孤岛,不依赖后置 Unit 的产物
  - TDD 闭环: 每个 Unit 必须先写验收测试,该测试**只能**验证当前 Unit 的输入输出
  - HARD RULE 1: 测试入口必须用 `cargo nextest run` 系列 (ralph-cli 走 cli-serial 串行,其他 6 包走默认并发)
  - HARD RULE 2: 默认走并发,确需串行时显式配置
---

# 移除 5 个 Backend 的开发计划

> **目标**: 在 `ralph-orchestrator` 中物理删除 `amp`、`roo`、`kiro`、`kiro-acp`、`copilot` 这 5 个 backend 的全部生产代码、测试、fixture、preset、脚本与文档引用。删除后保留的 backend 是 `claude`、`gemini`、`codex`、`opencode`、`pi`、`traecli`、`custom`(共 7 个)。
>
> **执行约束**(用户硬要求):
> - **严格串行**: 单向流水线 U1 → U2 → … → U8,绝对禁止并行/交叉开发
> - **绝对隔离**: 每个 Unit 是一个独立的孤岛,不依赖后置 Unit 的产物
> - **TDD 闭环**: 每个 Unit 必须先写验收测试,该测试**只能**验证当前 Unit 的输入输出
> - **HARD RULE 1**: 测试入口必须用 `cargo nextest run` 系列(ralph-cli 走 cli-serial 串行,其他 6 包走默认并发)
> - **HARD RULE 2**: 默认走并发,确需串行时显式配置

---

## Context

`crates/ralph-cli/src/backend_support.rs` 的 `VALID_BACKENDS` 当前是 `claude / kiro / kiro-acp / gemini / codex / amp / copilot / opencode / pi / roo / traecli / custom` 共 12 项。本次清理目标是把 5 个用户不再需要的 backend(`amp / roo / kiro / kiro-acp / copilot`)从代码、测试、fixture、preset、脚本、文档全链路清除,使其收敛到保留列表。

移除的根因: 这 5 个 backend 的代码占用面广(`copilot_stream.rs`、`acp_executor.rs` 整文件 + `agent-client-protocol` 依赖 + `HatBackend::KiroAgent` 变体),但实际使用率低;移除可显著降低维护负担、减少依赖图与测试矩阵。

预期产物:
- 全部 6 个 Rust 包编译通过、`cargo clippy --all-targets -- -D warnings` 通过
- `cargo nextest run` 全包测试通过(ralph-cli 走 cli-serial,其他 6 包走默认并发)
- `./scripts/run-tests.sh` 通过(含 doctest 与 cli-doc-drift)
- `CHANGELOG.md` 与 `docs/reference/changelog.md` 新增 `Removed backends: amp, roo, kiro, kiro-acp, copilot`
- `docs/guide/kiro-migration.md` 与 `docs/guide/roo-backend.md` 直接删除(用户已确认)
- `HatBackend::KiroAgent` 整变体删除(用户已确认)

---

## 关键发现(影响总体策略)

1. **SSOT 列表双份**: `VALID_BACKENDS`(backend_support.rs L4)与 `DEFAULT_PRIORITY`(auto_detect.rs L11)是**两份独立但同步**的列表,每次删 backend 都要同步两处。`preset_templates.rs` / `ralph_config.rs::get_agent_priority` 是第三、四份列表。
2. **`wave.rs` 5 个矩阵测试的迭代器列表**: `test_wave_worker_execution_mode_matches_supported_named_backend_roster`(L60)、`test_execute_wave_named_backend_large_prompt_contracts`(L1405)、`test_execute_wave_hat_named_with_args_invocation_contracts`(L1491)、`test_execute_wave_hat_named_large_prompt_contracts`(L1779)、`test_execute_wave_hat_named_with_args_large_prompt_contracts`(L1962)—— 5 个表驱动测试全部含 5 个目标 backend。
3. **`kiro` 与 `kiro-acp` 强耦合**: 共用 `acp_executor.rs` 整文件、共用 `kiro-cli` binary 探测(`auto_detect::detection_command()` 中 `"kiro" | "kiro-acp" => "kiro-cli"` 单一 match arm)、共用 `HatBackend::KiroAgent` 枚举变体。**不可拆分为两个 Unit**,合并到 U4。
4. **e2e `Backend` enum 仅 3 个变体**: `Claude / Kiro / OpenCode` —— 删除 `Kiro` 即可。`amp / roo / copilot` 不在 e2e 范围内,无需 e2e 改动。
5. **`presets/minimal/{amp,kiro,roo}.yml`** 是 backend-specific 模板,删除 backend 应整文件删除(不留空模板)。
6. **`agent-client-protocol = "0.9.4"`**(crates/ralph-adapters/Cargo.toml L43)仅 kiro-acp 使用,删 kiro-acp 时同步删除该依赖。
7. **`scripts/ralph-zsh-plugin.zsh` 的 `_RALPH_BACKENDS` 数组**(L90/93/94/97)需同步移除 4 项(无 kiro-acp)。
8. **本地 MEMORY.md 与所有子 memory 文件无目标 backend 条目**,无需更新 memory。

---

## 总体策略

### 顺序逻辑(依赖反转 + 爆炸半径最小化)

按耦合度从低到高,删完一个 backend 后中间态"可编译可测试":

```
U1 (Copilot)               ← 最独立:自带 copilot_stream.rs 隔离层,删除面与 acl 一致
U2 (Amp)         ← U1     ← 纯 CLI 标准路径,无外部依赖
U3 (Roo)         ← U2     ← 自带 build_roo_prompt_file helper,无外部 crate 依赖
U4 (Kiro+Kiro-acp) ← U3   ← 合并单元:acp_executor.rs + agent-client-protocol + HatBackend::KiroAgent
U5 (E2E)         ← U4     ← Backend enum + 6+ e2e 源文件(只删 Kiro 变体)
U6 (Fixtures)    ← U5     ← fixtures/kiro/ + fixtures/kiro-acp/ + mixed_backends.yml
U7 (Presets/Scripts) ← U6 ← presets/minimal/*.yml + zsh plugin + mock-kiro-acp.sh
U8 (Docs/Cursor) ← U7     ← 全部 user-facing 文档 + .cursor/rules + CHANGELOG
```

### SSOT 列表分散处理

`VALID_BACKENDS` / `DEFAULT_PRIORITY` / `wave.rs` 5 个矩阵迭代列表**不拆为独立 Unit**,而是分散到每个 backend 删除 Unit(U1/U2/U3/U4)的"生产代码改动"部分——每个 Unit 仅删除**一个字符串**,副作用最小。

### 横切层(e2e/fixtures/presets/docs)后置

理由: 它们依赖前述 backend 在生产代码中被物理删除后才能正确收敛,否则会留下 forward reference。

---

## Implementation Units

### U1. Copilot Removal

- **Goal**: 物理删除 `copilot_stream.rs` 模块及其所有引用,使 `lib.rs` 不再导出 copilot 相关类型,`VALID_BACKENDS` 与 `DEFAULT_PRIORITY` 不再含 `copilot`,`wave.rs` 5 个矩阵测试不再迭代 `copilot`。
- **TDD 测试范围**(只测本 Unit):
  - **新增** `crates/ralph-cli/src/backend_support.rs::tests::test_valid_backends_does_not_contain_copilot`(断言 `VALID_BACKENDS` / `VALID_BACKENDS_LABEL` 不含 `copilot`)
  - **新增** `crates/ralph-adapters/src/auto_detect.rs::tests::test_default_priority_does_not_contain_copilot`
  - **新增** `crates/ralph-adapters/src/lib.rs::tests::test_copilot_stream_module_removed`(断言不含 `mod copilot_stream;`)
  - **删除** 既有 copilot 测试:`cli_backend.rs::tests` 中所有 `*_copilot*` / `copilot_*` 函数;`cli_executor.rs::tests` 中 copilot JSONL 解析测试
- **生产代码改动**:
  - `crates/ralph-adapters/src/copilot_stream.rs` — 删除整个文件(~600 行)
  - `crates/ralph-adapters/src/lib.rs` — 删除 L33 `mod copilot_stream;` 与 L53 `pub use copilot_stream::{...};`
  - `crates/ralph-adapters/src/cli_backend.rs` — 删除 L73-87 / L247-262 / L408-425 / L818-844 中 `copilot` match arm;删除 factories `copilot()`(L332-349)、`copilot_tui()`(L356-365)、`copilot_interactive()`(L491-500)
  - `crates/ralph-cli/src/backend_support.rs` — `VALID_BACKENDS` / `VALID_BACKENDS_LABEL` 删除 `copilot`
  - `crates/ralph-adapters/src/auto_detect.rs` — `DEFAULT_PRIORITY` 删除 `"copilot"`
  - `crates/ralph-cli/src/loop_runner/tests/wave.rs` — 5 个矩阵测试迭代列表移除 `"copilot"`
- **验收命令**(TDD 闭环):
  - `cargo nextest run -p ralph-adapters -- copilot`(应返回 0 tests / 全部 pass)
  - `cargo nextest run -p ralph-cli --cli-serial -- copilot`(应返回 0 tests)
  - `cargo nextest run -p ralph-adapters`(默认并发,全包回归)
  - `cargo nextest run -p ralph-cli --cli-serial`(全包回归)
  - `cargo build -p ralph-adapters`(确保 `pub use copilot_stream` 引用全消)
- **绝对前置**: 无
- **独立运行条件**: amp/roo/kiro/kiro-acp 仍存在;本 Unit 仅断言 copilot 不存在,不动其他 backend

### U2. Amp Removal

- **Goal**: 删除 `amp` 在生产代码与测试中的全部存在,SSOT 列表与 wave.rs 矩阵列表不再含 `amp`。
- **TDD 测试范围**(只测本 Unit):
  - **新增** `test_valid_backends_does_not_contain_amp`
  - **新增** `test_default_priority_does_not_contain_amp`
  - **删除** `cli_backend.rs::tests` 中所有 `amp_*` 测试函数(L1044 test_amp_backend、L1142 test_amp_interactive_mode_no_flags、L1343 test_from_name_amp、L1543 test_for_interactive_prompt_amp 等)
  - **修改** `crates/ralph-core/src/config/ralph_config.rs::tests::test_default_agent_priority`(L1128 byte-equality)→ 改为新 vec 并断言 amp 不含(本测试函数名保持为 `test_default_agent_priority`,但内容已剔除 amp)
  - **修改** `wave.rs` 5 个矩阵迭代列表移除 `"amp"`
- **生产代码改动**:
  - `crates/ralph-adapters/src/cli_backend.rs` — 删除 L320-330 `amp()`、L476-485 `amp_interactive()`;L73-87 / L247-262 / L408-425 / L818-844 中 amp match arm
  - `crates/ralph-cli/src/backend_support.rs` — 删除 `amp`
  - `crates/ralph-adapters/src/auto_detect.rs` — `DEFAULT_PRIORITY` 删除 `"amp"`
  - `crates/ralph-cli/src/doctor.rs` — 删除 L482 `amp => amp` 别名;删除 `auth_env_vars()` 中 amp arm(若存在)
  - `crates/ralph-core/src/config/v1_adapters.rs` — 删除 L26-28 `pub amp`
  - `crates/ralph-core/src/config/ralph_config.rs` — 删除 L297 `|| self.adapters.amp.tool_permissions.is_some()`;L739 `get_agent_priority` 默认 vec 删除 `"amp"`;L745-756 `adapter_settings` match arm 删除 amp
  - `crates/ralph-cli/src/loop_runner/tests/wave.rs` — 5 个矩阵迭代列表移除 `"amp"`
- **验收命令**:
  - `cargo nextest run -p ralph-adapters -- amp` → 0 tests
  - `cargo nextest run -p ralph-cli --cli-serial -- amp` → 0 tests
  - `cargo nextest run -p ralph-core -- amp`(默认并发)
  - 全包回归:`cargo nextest run -p ralph-core`、`-p ralph-adapters`、`-p ralph-cli --cli-serial`
- **绝对前置**: U1
- **独立运行条件**: roo/kiro/kiro-acp/copilot(deleted)/amp 状态由本 Unit 控制

### U3. Roo Removal

- **Goal**: 删除 `roo` 全部存在,包括专属 `build_roo_prompt_file()` helper。
- **TDD 测试范围**(只测本 Unit):
  - **新增** `test_valid_backends_does_not_contain_roo`
  - **新增** `test_default_priority_does_not_contain_roo`(反义断言:roo 不在 `DEFAULT_PRIORITY` 中任意位置)
  - **新增** `test_build_roo_prompt_file_helper_removed`(grep 验证 `cli_backend.rs` 不含 `build_roo_prompt_file` 标识符)
  - **删除** `cli_backend.rs::tests` 中 L1899-2048 全段 roo 测试
  - **删除** `auto_detect.rs` L239 `test_default_priority_includes_roo`、L247 `test_default_priority_roo_is_second_to_last`、L266 `test_detection_command_roo`
  - **修改** `init.rs::tests::test_override_backend_*`(L300-346)中 roo case
  - **修改** `wave.rs` 5 个矩阵迭代列表移除 `"roo"`
- **生产代码改动**:
  - `crates/ralph-adapters/src/cli_backend.rs` — 删除 L595-625 `roo()` / `roo_interactive()`、L691-714 `build_roo_prompt_file()` helper、L763-764 roo `--print` 判定、L73-87 / L247-262 / L408-425 / L818-844 match arm 中的 roo 分支
  - `crates/ralph-cli/src/backend_support.rs` — 删除 `roo`
  - `crates/ralph-adapters/src/auto_detect.rs` — `DEFAULT_PRIORITY` 删除 `"roo"`
  - `crates/ralph-cli/src/doctor.rs` — 删除 L486 `roo => roo` 别名与 `auth_env_vars()` 中 roo arm
  - `crates/ralph-cli/src/init.rs` — `test_override_backend_*` 移除 roo case(若存在)
  - `crates/ralph-cli/src/loop_runner/tests/wave.rs` — 移除 `"roo"`
- **验收命令**:
  - `cargo nextest run -p ralph-adapters -- roo` → 0 tests
  - `cargo nextest run -p ralph-cli --cli-serial -- roo` → 0 tests
  - `cargo nextest run -p ralph-adapters -- default_priority`
  - 全包回归:`-p ralph-adapters`(默认并发)、`-p ralph-cli --cli-serial`
- **绝对前置**: U2
- **独立运行条件**: roo 专属 helper 完全由本 Unit 删除;kiro/copilot(deleted)/amp(deleted)状态独立

### U4. Kiro + Kiro-acp Removal(合并单元)

- **Goal**: 删除 `kiro` 与 `kiro-acp` 两个 backend 及其全部底层支撑(`acp_executor.rs`、`agent-client-protocol` crate 依赖、`loop_runner` ACP 执行路径、`sop_runner` TUI fallback、`HatBackend::KiroAgent` 变体整删除),`wave.rs` 与 SSOT 列表收敛。
- **合并原因**: kiro 与 kiro-acp 在 `acp_executor.rs` 同文件服务、`detection_command()` 中 `"kiro" | "kiro-acp" => "kiro-cli"` 单一 match arm、`HatBackend::KiroAgent` 变体同时承载两者——若拆分为两个 Unit,中间态(只删 kiro 保留 kiro-acp 或反之)无法编译。
- **TDD 测试范围**(只测本 Unit):
  - **新增** `test_valid_backends_does_not_contain_kiro_and_kiro_acp`(单次断言两者都删,对应单一子集)
  - **新增** `test_acp_executor_module_removed`(断言 `lib.rs` 不导出 `AcpExecutor`)
  - **新增** `test_hat_backend_kiro_agent_variant_removed`(断言 `hat.rs` 不含 `KiroAgent` 枚举变体,grep 验证)
  - **新增** `test_agent_client_protocol_dependency_removed`(解析 `Cargo.toml`,断言不含 `agent-client-protocol`)
  - **删除** `cli_backend.rs::tests` 中所有 `kiro_*` / `kiro_acp_*`(L996-1554)
  - **删除** `auto_detect.rs` L204 `test_detection_command_kiro`、L215/L217 中 kiro 分支
  - **删除** `acp_executor.rs` 全部内部测试 + `tests/acp_executor_integration.rs` 4 个 `#[ignore]` + 2 个 `kiro_acp_*`
  - **删除** `ralph-core/src/event_loop/tests/hat_backend.rs::test_get_hat_backend_with_kiro_agent`(L28)
  - **删除** `ralph-core/src/config/ralph_config.rs::tests` L1937 / L1959 / L2067 三个 kiro case
  - **修改** `cli_backend.rs::tests::test_env_vars_default_empty`(L1856 byte-equality)→ 改写为新值
  - **删除** `ralph-core/tests/smoke_runner.rs` L806-989 `mod kiro_smoke_tests`(6+ 测试)与 L996-1089 `mod kiro_acp_smoke_tests`(8 测试)
  - **修改** `init.rs::tests::test_template_is_valid_yaml` 移除 kiro 行;删除 `test_generate_template_kiro`(L230)
  - **修改** `hats.rs::tests` L1495 / L1513 删除 kiro case
  - **修改** `sop_runner.rs::tests` L357 / L404 / L413 删除 kiro / kiro-acp case
  - **修改** `wave.rs` 5 个矩阵迭代列表移除 `"kiro"` `"kiro-acp"`;删除 L1224-1297 中 `kiro`/`kiro-acp` macro 生成的测试函数;删除 L1705-1746 kiro HatBackend、L1903-1920 kiro-acp named 大 prompt、L2109-2129 kiro-acp agent 大 prompt、L2155-2215 acp 合约、L2218 / L2235 kiro-acp error/timeout、L2330 / L2378 kiro/kiro-acp hat backend、L2706-2717 acp waveworker 等
- **生产代码改动**:
  - `crates/ralph-adapters/src/acp_executor.rs` — 删除整个文件(1-696 行)
  - `crates/ralph-adapters/src/lib.rs` — 删除 L28 `mod acp_executor;` 与 L43 `pub use acp_executor::AcpExecutor;`
  - `crates/ralph-adapters/Cargo.toml` L43 — 删除 `agent-client-protocol = "0.9.4"`
  - `crates/ralph-adapters/src/cli_backend.rs` — 删除 L161-174 `kiro()`、L179-196 `kiro_with_agent()`、L202-225 `kiro_acp()` / `kiro_acp_with_options()`、L431-440 `kiro_interactive()`、L279-283 `KiroAgent` 特殊分支;L73-87 / L247-262 / L268-294 / L408-425 / L818-844 中 kiro / kiro-acp match arm
  - `crates/ralph-cli/src/backend_support.rs` — 删除 `kiro`、`kiro-acp`
  - `crates/ralph-adapters/src/auto_detect.rs` — `DEFAULT_PRIORITY` 删除 `"kiro"` `"kiro-acp"`;`detection_command()` match 删除 `"kiro" | "kiro-acp" => "kiro-cli"`
  - `crates/ralph-cli/src/doctor.rs` — 删除 L478 `kiro-cli => kiro` 别名与 L403/L404 `auth_env_vars()` 中 kiro / kiro-acp arm
  - `crates/ralph-cli/src/sop_runner.rs` — 删除 L113-121 kiro-acp TUI fallback
  - `crates/ralph-cli/src/loop_runner/execution.rs` — 删除 L136-148 ACP 执行路径
  - `crates/ralph-cli/src/loop_runner/wave/worker.rs` — 删除 L187 `AcpExecutor::new`
  - `crates/ralph-cli/src/loop_runner/mod.rs` — 删除 L91 `use ralph_adapters::{...}` 中 `AcpExecutor`
  - `crates/ralph-core/src/config/v1_adapters.rs` — 删除 L18-20 `pub kiro`
  - `crates/ralph-core/src/config/ralph_config.rs` — 删除 kiro 相关 match arm
  - `crates/ralph-core/src/preflight.rs` L1126 — 删除 `command_for_backend` kiro arm
  - `crates/ralph-core/src/config/hat.rs` L80 — 删除 `HatBackend::KiroAgent` 变体整段(影响 `from_hat_backend` match;若不完全穷尽,补 `unreachable!()`)
  - `crates/ralph-cli/src/loop_runner/tests/wave.rs` — 5 个矩阵迭代列表移除 `"kiro"` `"kiro-acp"`;删除两个 backend 专属测试函数
- **验收命令**:
  - `cargo nextest run -p ralph-adapters -- kiro`(含 `kiro-acp`)→ 0 tests
  - `cargo nextest run -p ralph-cli --cli-serial -- kiro`(含 `kiro-acp`)→ 0 tests
  - `cargo nextest run -p ralph-core -- kiro`(默认并发)→ 0 tests
  - `cargo nextest run -p ralph-core -- hat_backend` → 0 tests
  - `cargo build -p ralph-adapters` → 验证 `agent-client-protocol` 真的从依赖图消失
  - `cargo tree -p ralph-adapters | grep -c agent-client-protocol` → 必须为 0
  - 全包回归: 所有 6 个包都跑
- **绝对前置**: U3
- **独立运行条件**: kiro/kiro-acp 全部依赖(acp_executor.rs / agent-client-protocol / HatBackend::KiroAgent / loop_runner ACP 路径 / sop_runner fallback)只在该 Unit 删除,与其他 backend 互不干扰
- **HatBackend 决策**: 删除 `HatBackend::KiroAgent` 整变体(用户已确认);若 `from_hat_backend` match 不穷尽,补 `unreachable!()`

### U5. E2E Crate Cleanup

- **Goal**: 删除 `crates/ralph-e2e/` 中 `Backend::Kiro` 变体及其全部引用。`amp / roo / copilot` 不在 e2e 范围内,无需 e2e 改动。
- **TDD 测试范围**(只测本 Unit):
  - **新增** `crates/ralph-e2e/src/backend.rs::tests::test_backend_enum_excludes_kiro`(断言 `Backend` 不含 `Kiro` 变体)
  - **新增** `test_kiro_cases_removed_from_scenarios`(grep 所有 `*.rs` 验证 7 个 scenarios 文件不含 `kiro` 字符串)
- **生产代码改动**:
  - `crates/ralph-e2e/src/main.rs` L88 — 删除 `Backend::Kiro` 变体;L110 `to_lib_backend` 映射删除 kiro 分支
  - `crates/ralph-e2e/src/backend.rs` L11-67 — 删除 `Kiro` 变体(三处:enum 定义、`From` impl、`Display` impl)
  - `crates/ralph-e2e/src/auth.rs` L176-318 — 删除 kiro 相关 auth
  - `crates/ralph-e2e/src/runner.rs` L553-873 — 删除 kiro run path
  - `crates/ralph-e2e/src/analyzer.rs` L741-814 — 删除 kiro analyze
  - `crates/ralph-e2e/src/scenarios/{incremental,memory,errors,events,connectivity,orchestration,capabilities}.rs` — 删除所有 kiro case
  - `crates/ralph-e2e/src/mock.rs` L288 — 删除 kiro mock
- **验收命令**:
  - `cargo nextest run -p ralph-e2e -- kiro`(默认并发)→ 0 tests
  - `cargo build -p ralph-e2e` → 必须先通过(验证 enum 收敛后所有 match 完整)
  - `cargo nextest run -p ralph-e2e`(默认并发,全包回归)
- **绝对前置**: U4
- **独立运行条件**: e2e 内部依赖 `ralph-adapters` 的 backend 类型,U4 已删 backend,kiro 在 e2e 侧只有引用、无生产逻辑

### U6. Fixtures & Scenarios

- **Goal**: 物理删除与已删 backend 绑定的测试 fixture 与共享 scenario 文件。
- **TDD 测试范围**(只测本 Unit):
  - **新增** `test_kiro_fixtures_dir_removed`(IO 检查 `crates/ralph-core/tests/fixtures/kiro/` 不存在)
  - **新增** `test_kiro_acp_fixtures_dir_removed`
  - **新增** `test_mixed_backends_scenario_excludes_deleted_backends`(解析 YAML,断言不含 kiro/kiro-acp/amp/copilot/roo 关键字)
- **生产代码改动**:
  - `crates/ralph-core/tests/fixtures/kiro/` — 删除整个目录(README + 3 .jsonl)
  - `crates/ralph-core/tests/fixtures/kiro-acp/` — 删除整个目录(README + 2 .jsonl)
  - `crates/ralph-core/tests/scenarios/mixed_backends.yml` — 删除文件
  - `crates/ralph-adapters/tests/pty_executor_integration.rs` L586/652 — 替换/删除 kiro stream-json fixture 引用
- **验收命令**:
  - `cargo nextest run -p ralph-core -- fixtures` → 通过
  - `cargo nextest run -p ralph-adapters -- pty_executor`
  - 全包回归(`-p ralph-core` 默认并发)
- **绝对前置**: U4(U5 不依赖 fixtures,U6 不依赖 U5)
- **独立运行条件**: fixture 引用是纯粹的 IO 资源,与其他 Unit 互不耦合

### U7. Presets & Scripts

- **Goal**: 删除 `presets/minimal/` 中与已删 backend 绑定的 yml 模板、删除 `mock-kiro-acp.sh`、从 zsh 补全数组移除 5 个 backend(实际 4 项,kiro-acp 不在 zsh 数组)。
- **TDD 测试范围**(只测本 Unit):
  - **新增** `test_minimal_preset_files_exclude_deleted_backends`(读取目录,断言不含 `{amp,kiro,roo}.yml`,保留 claude/gemini/codex/opencode/pi/traecli/custom)
  - **新增** `test_zsh_plugin_backend_array_excludes_deleted_backends`(grep `scripts/ralph-zsh-plugin.zsh`,断言 `_RALPH_BACKENDS=(...)` 不含 kiro/amp/copilot/roo)
  - **新增** `test_tools_evaluate_scripts_exclude_kiro`(grep `tools/evaluate-*.sh` 与 `tools/PRESET_EVALUATOR_PROMPT.md` 不含 kiro)
- **生产代码改动**:
  - `presets/minimal/amp.yml` — 删除
  - `presets/minimal/kiro.yml` — 删除
  - `presets/minimal/roo.yml` — 删除
  - `presets/minimal/preset-evaluator.yml` — 检查并改写 kiro 引用(若存在)
  - `crates/ralph-adapters/tests/acp_process_cleanup.rs::mock-kiro-acp.sh` 引用 — 删除(若仅 kiro-acp 使用则整段删除)
  - `scripts/ralph-zsh-plugin.zsh` L90/93/94/97 — `_RALPH_BACKENDS` 数组移除 4 项(kiro/amp/copilot/roo)
  - `tools/PRESET_EVALUATOR_PROMPT.md` L131 — 改写或保留(若必须保留 kiro 用法,改写为"live backend"-style 抽象)
  - `tools/evaluate-all-presets.sh` L8 / `tools/evaluate-preset.sh` L8, L343, L364 — 移除 kiro 引用或抽象
- **验收命令**:
  - `cargo nextest run -p ralph-adapters -- acp_process_cleanup`(默认并发)
  - `bash -n scripts/ralph-zsh-plugin.zsh`(纯语法检查,不是 nextest)
  - 直接对 zsh 数组做 grep 断言(U7 自验证脚本,作为新增测试函数的一部分)
- **绝对前置**: U6
- **独立运行条件**: presets 与 zsh plugin 与 Rust 代码无 binding,不依赖其他 Unit 输出

### U8. Documentation & Cursor Rules & CHANGELOG

- **Goal**: 同步更新所有 user-facing 文档与 cursor rules,标注"Removed backends" 在 CHANGELOG 中,直接删除 `docs/guide/kiro-migration.md` 与 `docs/guide/roo-backend.md`(用户已确认)。
- **TDD 测试范围**(只测本 Unit):
  - **新增** `test_changelog_records_backend_removal`(grep `CHANGELOG.md` 断言含 `"Removed backends: amp, roo, kiro, kiro-acp, copilot"`)
  - **新增** `test_docs_index_no_longer_links_deleted_backend_guides`(检查 `docs/guide/index.md` 不再 link `kiro-migration.md` 与 `roo-backend.md`)
  - **新增** `test_cursor_rules_exclude_deleted_backends`(.cursor/rules/architecture-modules.mdc L50、feature-flags.mdc L75 不含 5 个 backend 名)
  - **新增** `test_kiro_and_roo_dedicated_docs_removed`(检查 `docs/guide/kiro-migration.md` / `docs/guide/roo-backend.md` 不再存在)
- **生产代码改动**:
  - `docs/guide/kiro-migration.md` — 删除
  - `docs/guide/roo-backend.md` — 删除
  - `docs/guide/backends.md` — 重写为"7 个保留 backend 的描述"(claude / gemini / codex / opencode / pi / traecli / custom)
  - `docs/guide/{agents,cli-reference,configuration,project-usage,index}.md` — 移除 5 个 backend 引用
  - `docs/getting-started/{index,installation,first-task}.md` — 移除 backend 引用
  - `docs/api/{ralph-adapters,config}.md` — 移除 backend 引用
  - `docs/advanced/testing.md` / `docs/mock-cli.md` — 移除 kiro cassette 示例
  - `docs/migration/v2-hatless-ralph.md` / `docs/deployment/qchat-production.md` — 移除 backend 引用
  - `docs/reference/changelog.md` — 加 "Removed backends" 项
  - `CHANGELOG.md` — 加 `## [Unreleased] ### Removed - Backends: amp, roo, kiro, kiro-acp, copilot (remaining: claude, gemini, codex, opencode, pi, traecli, custom)`
  - `.cursor/rules/architecture-modules.mdc` L50 — 移除 5 个 backend
  - `.cursor/rules/feature-flags.mdc` L75 — 移除 5 个 backend
- **验收命令**:
  - `cargo nextest run -p ralph-cli --cli-serial -- docs`(若存在文档 diff 检查测试)
  - `scripts/check-cli-doc-drift.sh`
  - 直接对本 Unit 自带的"文档不存在/CHANGELOG 包含关键字"断言脚本运行
- **绝对前置**: U7
- **独立运行条件**: 文档修改是纯文本操作,与 Rust 编译无依赖

---

## 单元排序与依赖图

```
U1 (Copilot)               ─┐
U2 (Amp)         ← U1      │
U3 (Roo)         ← U2      ├─ 严格串行:前一 Unit 100% 完成才能开始下一 Unit
U4 (Kiro+Kiro-acp) ← U3    │
U5 (E2E)         ← U4      │
U6 (Fixtures)    ← U5      │
U7 (Presets/Scripts) ← U6  │
U8 (Docs/Cursor) ← U7 ─────┘
```

**依赖说明**:
- 每个 Unit 的 TDD 测试断言**只读当前 Unit 的产物**(即"X backend 不存在"),不会因为后置 Unit 尚未存在而无法验证
- 中间态在每个 Unit 完成后都是"可编译可测试"的状态:U1 后 → copilot 已删,其他 4 个 backend 仍工作;U2 后 → copilot+amp 已删;以此类推
- U6 与 U5 不存在真实耦合(U5 改 e2e enum,U6 删 fixtures),但按"严格串行"要求保持线性(U5 → U6)

---

## Critical Files

- `crates/ralph-cli/src/backend_support.rs` — SSOT 后端列表,所有 backend 删除流程的最终收敛点
- `crates/ralph-adapters/src/cli_backend.rs` — 7 个工厂方法 + 5 个矩阵 match arm + `build_roo_prompt_file` helper 集中地
- `crates/ralph-adapters/src/auto_detect.rs` — `DEFAULT_PRIORITY` 与 `detection_command()` 的 SSOT 列表
- `crates/ralph-adapters/src/copilot_stream.rs` — 整个文件删除(U1)
- `crates/ralph-adapters/src/acp_executor.rs` — 整个文件删除(U4)
- `crates/ralph-adapters/src/lib.rs` — 模块声明与 `pub use` 收敛
- `crates/ralph-adapters/Cargo.toml` — `agent-client-protocol` 依赖删除(U4)
- `crates/ralph-cli/src/loop_runner/tests/wave.rs` — 5 个矩阵测试的迭代器与 kiro/kiro-acp 专属测试的集中地
- `crates/ralph-core/src/config/hat.rs` — `HatBackend::KiroAgent` 变体整删除(U4)
- `crates/ralph-cli/src/sop_runner.rs` — kiro-acp TUI fallback 删除(U4)
- `crates/ralph-cli/src/loop_runner/{mod.rs,execution.rs,wave/worker.rs}` — ACP 执行路径删除(U4)
- `crates/ralph-e2e/src/{main.rs,backend.rs,auth.rs,runner.rs,analyzer.rs,mock.rs,scenarios/*.rs}` — Backend::Kiro 删除(U5)
- `presets/minimal/{amp,kiro,roo}.yml` — 整文件删除(U7)
- `scripts/ralph-zsh-plugin.zsh` — `_RALPH_BACKENDS` 数组收敛(U7)
- `docs/guide/{kiro-migration.md,roo-backend.md}` — 整文件删除(U8)
- `CHANGELOG.md` + `docs/reference/changelog.md` — 新增 Removed backends 项(U8)
- `.cursor/rules/{architecture-modules.mdc,feature-flags.mdc}` — 移除 5 个 backend 引用(U8)

---

## 风险点

1. **wave.rs 5 个矩阵测试的迭代列表共享 helper 改动**: U1-U4 每个 Unit 都会改 wave.rs 的 `for backend in [...]` / `for named in [...]` 列表,如果某次 PR 把列表改成 vec literal 而另一次改成宏,易产生冲突。**对策**: 每个 Unit 仅以"删除一行字符串"的方式修改,严禁重构迭代机制。

2. **`HatBackend::KiroAgent` 变体删除后 `from_hat_backend` match 穷尽性**: 删除 KiroAgent 后 match 必须仍然穷尽(且不再引用 `HatBackend::KiroAgent`)。**对策**: U4 的 TDD 测试 `test_hat_backend_kiro_agent_variant_removed` 在生产代码改动后跑(应 pass),并加 `cargo check -p ralph-core` 作为子验收(必须先于 `cargo nextest run`)。

3. **`agent-client-protocol` 依赖的真实收敛**: `Cargo.lock` 可能遗留。**对策**: U4 中加 `cargo tree -p ralph-adapters | grep -c agent-client-protocol` 自验证脚本,作为 U4 验收的额外一步(必须为 0)。

4. **`doc_cli_drift.sh` 与 docs 改动不同步**: U7 改 zsh 数组,U8 改 docs,顺序保证二进制先于 docs。**对策**: U8 末尾跑 `scripts/check-cli-doc-drift.sh`。

5. **e2e `Backend` enum 的 `Display` impl 收敛**: 删除 `Backend::Kiro` 后 `format!("{}", backend)` 与 `match backend` 必须穷尽。**对策**: U5 的 `cargo build -p ralph-e2e` 必须先于 `cargo nextest run -p ralph-e2e`,作为子验收。

6. **Kiro + Kiro-acp 合并删除的 TDD 原子性挑战**: 合并是因为 `acp_executor.rs` 同文件服务两者。**对策**: 本 Unit 的 TDD 测试写成单一断言 `test_valid_backends_does_not_contain_kiro_and_kiro_acp`,内部一次性验两个字符串;不拆成 kiro 单测 + kiro-acp 单测。

7. **`auto_detect::detection_command()` 默认映射分支收敛**: 每个 Unit 内 match arm 完整性由该 Unit 的 TC 覆盖(用 `cargo build -p ralph-adapters` 兜底)。

8. **`presets/minimal/*.yml` 文件删除 vs 留下的空模板**: 不留空模板。**对策**: U7 的 `test_minimal_preset_files_exclude_deleted_backends` 直接断言目录列表不含被删的 yml。

---

## 文档归档决策(用户已确认)

- **直接删除** `docs/guide/kiro-migration.md` 与 `docs/guide/roo-backend.md`(用户已确认方案 A)
- **`docs/achieved/`** 下的历史 plan/report(如 `2026-06-02-005-fix-traecli-backend-availability-plan.md`)与本次删除无直接关联,**不动**
- **`CHANGELOG.md`** 新增 `## [Unreleased] ### Removed - Backends: amp, roo, kiro, kiro-acp, copilot (remaining: claude, gemini, codex, opencode, pi, traecli, custom)`
- **`docs/reference/changelog.md`** 同步加 "Removed backends" 项
- **本地 MEMORY.md 与所有子 memory 文件无目标 backend 条目**,无需更新

---

## Verification

### 完整移除后的最终验收(流水线 U1-U8 全部结束后)

```bash
# 1. 全部测试入口(ralph-cli 走 cli-serial,其他走默认并发)
cargo nextest run -p ralph-core
cargo nextest run -p ralph-adapters
cargo nextest run -p ralph-e2e
cargo nextest run -p ralph-tui

# 2. ralph-cli 强制 cli-serial(根因:Mutex + sleep CPU 抢占)
cargo nextest run -p ralph-cli --cli-serial

# 3. 全包含 doctest
./scripts/run-tests.sh

# 4. 静态
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
scripts/check-cli-doc-drift.sh

# 5. agent-client-protocol 真实收敛(U4 自验证)
cargo tree -p ralph-adapters | grep -c agent-client-protocol  # 必须为 0
```

### 每个 Unit 结束时的局部验收

每个 Unit 完成后,跑"全包回归三件套"(只对本 Unit 改动的包 + cli-serial/cli 包)以确认未引入横向回归。每个 Unit 的 TDD 测试断言**只读当前 Unit 的产物**,跑通即代表本 Unit 闭环。

### Definition of Done

- [ ] 全部 8 个 Unit 串行完成,每个 Unit 的 TDD 测试通过
- [ ] `cargo nextest run` 全 6 包通过(ralph-cli 走 cli-serial,其他走默认并发)
- [ ] `cargo build --workspace` 通过
- [ ] `cargo clippy --all-targets -- -D warnings` 通过
- [ ] `cargo fmt --check` 通过
- [ ] `scripts/check-cli-doc-drift.sh` 通过
- [ ] `./scripts/run-tests.sh` 通过(含 doctest)
- [ ] `cargo tree -p ralph-adapters` 不含 `agent-client-protocol`
- [ ] `CHANGELOG.md` + `docs/reference/changelog.md` 新增 Removed backends 项
- [ ] `docs/guide/kiro-migration.md` + `docs/guide/roo-backend.md` 已删除
- [ ] `HatBackend::KiroAgent` 变体已删除,`from_hat_backend` match 仍穷尽
- [ ] `presets/minimal/{amp,kiro,roo}.yml` 已删除
- [ ] `scripts/ralph-zsh-plugin.zsh` 的 `_RALPH_BACKENDS` 数组已收敛
- [ ] 本地 MEMORY.md 无需更新(已确认无相关条目)