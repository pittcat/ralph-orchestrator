---
title: 'fix: config discovery 尊重 -c / RALPH_CONFIG（消除 ralph.yml 硬编码）'
type: fix
status: active
date: 2026-07-13
origin: universal-autoresearch 复现 — `ralph run -c *-autoresearch.yml` 启动成功，但 in-loop `ralph tools task` 报 `no ralph.yml in workspace` / `coordinator_hats []`
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
---

# fix: config discovery 尊重 `-c` / `RALPH_CONFIG`

## Goal Capsule

| Field | Value |
|---|---|
| Objective | 让 **workspace config discovery** 与 **主 CLI `-c` / `$RALPH_CONFIG`** 成为同一套优先级；`ralph tools task`、`emit` fallback、hat 子进程都能读到非默认文件名的完整配置（含 `tasks.coordinator_hats`），不再强迫操作者额外创建名为 `ralph.yml` 的文件或 symlink。 |
| Product authority | `docs/guide/configuration.md` 已声明：`ralph.yml` **或** `$RALPH_CONFIG` / `-c <file>` 均可作为 project config。本计划把该产品声明落实到 agent-facing tools 与 hat 执行环境。 |
| Execution profile | 改 `ralph-cli` 配置发现 + hat env 注入 + tools 传参；不改 event topology、不改 preset YAML 语义。 |
| Isolation rule | 不引入新的默认文件名；不静默把自定义配置复制成 `ralph.yml`；不削弱 human CLI 在无 config 时的 bypass。 |
| Stop conditions | 任一：自定义名配置下 `tools task ensure` 仍看到空 `coordinator_hats`；`ralph run -c custom.yml` 启动的 isolated hat 子进程里 `RALPH_CONFIG` 未注入；回归破坏「仅有 ralph.yml」旧路径。 |
| Tail ownership | 同步 `docs/guide/configuration.md` / `cli-reference.md`；为 `inject_hat_execution_env` 与 `load_coordinator_hats` 补单测 + 一条 CLI 集成测。 |

---

## Problem Frame

### 现象

操作者用自定义文件名启动循环：

```bash
ralph run -c myapp-autoresearch.yml
```

- `ralph preflight -c myapp-autoresearch.yml` → PASS
- `ralph run -c myapp-autoresearch.yml --dry-run` → 正确加载 hats / backend / event_loop
- 工作区**没有** `ralph.yml`

但循环内（或等价手动）执行：

```bash
RALPH_CURRENT_HAT=strategist ralph -c myapp-autoresearch.yml tools task ensure --key k 't'
```

仍失败：

```text
hat_command_policy denied 'ensure' ...
tasks.coordinator_hats []
no ralph.yml in workspace
```

同目录做 `ln -sf myapp-autoresearch.yml ralph.yml` 后，同一条 `ensure` 立即成功。

### 根因（源码）

存在 **两套不一致的 config discovery**：

| 路径 | 行为 | 位置 |
|------|------|------|
| 主 CLI `run` / `preflight` / 部分 `emit` | 使用全局 `config_sources`（来自 `-c`）；缺省时 `default_config_path()` 读 `$RALPH_CONFIG` → 否则 `ralph.yml` | `main.rs`、`cli/config_loader.rs`、`preflight.rs` |
| workspace 发现 helper | **只**找 `ralph.yml` / `ralph.yaml` | `config_resolution::find_workspace_config_path` |
| `ralph tools task` | **不接收** `config_sources`；`load_coordinator_hats` / `load_config_or_default` 硬编码文件名 | `main.rs` `Commands::Tools` → `tools.rs` → `task_cli.rs` |
| hat 子进程环境 | 注入 `RALPH_CURRENT_HAT` / `RALPH_EVENTS_FILE` / **`RALPH_HATS_SOURCE`**，但**不注入 `RALPH_CONFIG`** | `loop_runner/execution.rs` `inject_hat_execution_env` |

结果：主循环看起来「`-c` 完全可用」，agent-facing task ACL 却只看见空 allowlist，并给出「去创建 ralph.yml」的 hint。产品文档与实现分裂。

先例：`RALPH_HATS_SOURCE` 已证明「runner 注入 → in-loop CLI 继承」是正确模式（plan 001 §4.3 C1）。`RALPH_CONFIG` 缺同一注入。

### 非目标误判

- **不是**「生成的 YAML 结构不被 `-c` 接受」——`run`/`preflight` 已证明可加载。
- **不是**「必须强制所有项目文件名都叫 `ralph.yml`」——文档已允许 `-c` / `RALPH_CONFIG`。
- **不是**「人类无 config 时锁死 task CLI」——human bypass 保留。

---

## Requirements Trace

- **R1. Discovery SSOT：** 任意需要「工作区项目配置路径」的代码，必须通过统一 resolver；优先级固定为：
  1. 显式 `ConfigSource::File`（来自 CLI `-c`，取第一个文件源）
  2. `$RALPH_CONFIG`（非空）
  3. `<workspace>/ralph.yml`
  4. `<workspace>/ralph.yaml`
- **R2. Tools 接线：** `ralph tools …` 必须接收并使用与顶层相同的 `config_sources`；`ralph -c custom.yml tools task …` 不得再忽略 `-c`。
- **R3. Hat env 继承：** `ralph run -c custom.yml`（及等价 `RALPH_CONFIG`）启动的 backend / hat 子进程，必须注入 `RALPH_CONFIG=<resolved absolute or workspace-relative path>`，使 agent 裸跑 `ralph tools task` / `ralph emit` 时仍能发现同一配置（对标 `RALPH_HATS_SOURCE`）。
- **R4. coordinator_hats 同源：** `load_coordinator_hats` 与 task ACL 使用的 config 必须来自 R1 resolver；自定义文件名配置里声明的 `tasks.coordinator_hats: [strategist]` 必须生效。
- **R5. Emit fallback 一致：** 当 `config_sources` 为空时，emit / policy_check 的 workspace fallback 不得只认 `ralph.yml`，须走 R1（含 `$RALPH_CONFIG`）。
- **R6. 错误文案可操作：** `MissingRalphYml` / hint 改为「未发现项目配置」并提示：创建 `ralph.yml`，或设置 `RALPH_CONFIG`，或传 `-c <file>`；禁止暗示 symlink 是唯一解。
- **R7. 兼容旧路径：** 仅有 `ralph.yml`、未设 `-c` / `RALPH_CONFIG` 的现有仓库行为不变。
- **R8. 不静默写盘：** 本计划不在 workspace 自动创建 / 复制 `ralph.yml`；发现失败应 fail-closed（agent）或 typed error，不靠临时 alias 掩盖。

---

## Scope Boundaries

### In Scope

- `crates/ralph-cli/src/config_resolution.rs` — discovery SSOT
- `crates/ralph-cli/src/cli/config_loader.rs` — 与 `default_config_path` 对齐（避免第三套逻辑）
- `crates/ralph-cli/src/main.rs` — `Commands::Tools` 传入 `config_sources`
- `crates/ralph-cli/src/tools.rs` / `task_cli.rs` / `hat_command_policy.rs` — 消费 SSOT；更新 hint
- `crates/ralph-cli/src/loop_runner/execution.rs` + 调用点（`runner.rs` / wave dispatcher）— 注入 `RALPH_CONFIG`
- `crates/ralph-cli/src/commands/emit.rs` / `policy_check.rs` / `skill_cli.rs` — fallback 改用 SSOT
- 单测 + CLI 集成测
- `docs/guide/configuration.md`、`docs/guide/cli-reference.md` 短文对齐

### Out of Scope

- 改 RalphConfig schema / event_policy 语义
- 改 preset YAML（ce-executor / autoresearch）
- Universal AutoResearch 生成器侧文档/symlink workaround（下游可另立适配计划，但本仓修复后应废弃「必须另建最小 ralph.yml」）
- 自动把 `-c` 文件 rename/copy 为 `ralph.yml`
- 远程 URL 作为 tools 的 sync 加载（保持现有 sync 限制；仅 File + env）

---

## Key Technical Decisions

### D1. 单一函数 `resolve_project_config_path`

在 `config_resolution` 中新增（或扩展现有 helper）：

```rust
pub(crate) fn resolve_project_config_path(
    workspace_root: &Path,
    config_sources: &[ConfigSource],
) -> Option<PathBuf>
```

规则：

1. 扫描 `config_sources` 中第一个存在的 `ConfigSource::File(path)`（相对路径相对 `cwd`/`workspace_root` 的解析规则与 `load_config_for_preflight` 一致，实现时对齐现有 File 加载行为，禁止 invent 新语义）。
2. 否则读 `RALPH_CONFIG`（trim 非空）。
3. 否则 `find_workspace_config_path(workspace_root)`（保留 `ralph.yml`/`ralph.yaml`）。

`find_workspace_config_path` 保持「仅固定文件名」的窄语义，供「是否存在默认文件」类检查；**业务加载一律走 `resolve_project_config_path`**。

### D2. Hat 环境注入 `RALPH_CONFIG`（对标 `RALPH_HATS_SOURCE`）

扩展 `inject_hat_execution_env`：

- 增加参数 `config_path: Option<&Path>`（或 `Option<String>`）
- 写入 `RALPH_CONFIG=<path>`
- `retain` 列表加入 `RALPH_CONFIG`，避免重复/陈旧值

调用点（runner isolated/coordinator、wave worker）在已知 `config.config_path` 或本次 run 的 primary file source 时传入。若 primary 是 defaults-only（无文件），则不注入。

**为何不够只修 tools 的 `-c`：** isolated hat 里 agent 几乎从不写 `ralph -c … tools`；它们跑裸命令。没有 env 继承，R2  alone 无法闭合 in-loop 场景。

### D3. `load_coordinator_hats` 改为「路径入参」

签名演进：

```rust
pub fn load_coordinator_hats_from_path(path: &Path) -> Result<Vec<String>, CoordinatorHatsError>
```

或：

```rust
pub fn load_coordinator_hats(
    root: &Path,
    config_sources: &[ConfigSource],
) -> Result<Vec<String>, CoordinatorHatsError>
```

内部用 D1 解析路径；缺失 → `MissingRalphYml`（可 rename 为 `MissingProjectConfig`，若改枚举名则同步 Display/hint/测试；允许保留旧 variant 名但改文案，以降低 churn）。

`load_config_or_default` 同步改用同一路径，避免 ACL 与 RalphConfig 再分裂。

### D4. `tools::execute` 签名增加 `config_sources`

```rust
pub async fn execute(
    args: ToolsArgs,
    use_colors: bool,
    config_sources: &[ConfigSource],
) -> Result<()>
```

`main.rs` 传入与 `run`/`emit` 相同的 `config_sources`。`task_cli::execute` 下传。

### D5. 错误文案

`ConfigFault::MissingRalphYml` / `CoordinatorHatsError::MissingRalphYml` hint 示例：

```text
no project config found (looked for -c file, $RALPH_CONFIG, ralph.yml, ralph.yaml); \
pass `ralph -c <file> …`, export RALPH_CONFIG, or add ralph.yml with tasks.coordinator_hats
```

保留可机读 reason 码；测试断言改为匹配新文案关键词（`RALPH_CONFIG` / `-c`），不再要求「create ralph.yml」为唯一 hint。

---

## Implementation Units

### U1 — Discovery SSOT

**Files:** `config_resolution.rs`（+ 单测）、必要时薄封装于 `cli/config_loader.rs`

**Done when:**

- `resolve_project_config_path` 单测覆盖：仅 `-c`、仅 `RALPH_CONFIG`、仅 `ralph.yml`、仅 `ralph.yaml`、优先级覆盖（`-c` 盖过 env 盖过默认名）
- 现有 `find_workspace_config_path` 行为不变

### U2 — Hat env 注入 `RALPH_CONFIG`

**Files:** `loop_runner/execution.rs`、`runner.rs`、`wave/dispatcher.rs`（及现有调用 `inject_hat_execution_env` 处）

**Done when:**

- 单测：给定 config path 时 backend.env_vars 含 `RALPH_CONFIG`
- 无 path 时不注入
- retain 逻辑清除旧值

### U3 — Tools / task_cli 消费 SSOT

**Files:** `main.rs`、`tools.rs`、`task_cli.rs`、`hat_command_policy.rs`

**Done when:**

- `ralph -c custom.yml tools task ensure …` 在**无** `ralph.yml` 工作区、且 custom.yml 含 `tasks.coordinator_hats: [strategist]`、`RALPH_CURRENT_HAT=strategist` 时成功
- 同场景不传 `-c`、不设 env → 仍 typed Missing* 错误（agent fail-closed）
- human CLI 无 config 时仍不被锁死（现有 bypass 保留）

### U4 — Emit / policy_check / skill fallback

**Files:** `commands/emit.rs`、`policy_check.rs`、`skill_cli.rs`

**Done when:**

- `config_sources` 非空：行为保持「显式源优先」（已有逻辑）
- `config_sources` 空但 `RALPH_CONFIG=custom.yml`：加载 custom，不再 warn「ralph.yml not found, using defaults」后跳过 strict policy
- 与 U2 组合：子进程仅靠继承的 `RALPH_CONFIG` 即可 policy-check

### U5 — 文案与文档

**Files:** hint/Display、`docs/guide/configuration.md`、`docs/guide/cli-reference.md`

**Done when:**

- configuration 指南增加一小节：「Agent-facing tools discovery」说明 R1 优先级与 `RALPH_CONFIG` 由 runner 注入
- 明确：自定义文件名无需再 symlink 为 `ralph.yml`（修复后）

### U6 — 回归护栏

**Files:** `crates/ralph-cli/tests/` 或现有 integration 模块

**最小集成场景（建议一个 tempdir 测试）：**

1. 写入 `project-autoresearch.yml`（含 `cli.backend`、`tasks.coordinator_hats: [strategist]`、最小 event_loop）
2. **不**创建 `ralph.yml`
3. `RALPH_CURRENT_HAT=strategist ralph -c project-autoresearch.yml tools task ensure --key t1 'hello'` → exit 0
4. 另测：设置 `RALPH_CONFIG` 但不传 `-c` → 同样成功
5. 负例：两者皆无 → agent ensure 失败且 hint 含 `RALPH_CONFIG` 或 `-c`

可选（若易测）：断言 `inject_hat_execution_env` 后子命令可见 env（单测级即可，不必真起 backend）。

---

## Test Plan

```bash
# 单元
cargo nextest run -p ralph-cli resolve_project_config_path
cargo nextest run -p ralph-cli load_coordinator_hats
cargo nextest run -p ralph-cli inject_hat_execution_env

# 集成（U6 落地后的实际测试名以实现为准）
cargo nextest run -p ralph-cli custom_config_name_task_ensure

# 旧路径不回归：标准 ralph.yml fixture
cargo nextest run -p ralph-cli -- integration_emit_policy
```

手动验收（实现后在干净 tempdir）：

```bash
# 无 ralph.yml
ralph run -c custom.yml --dry-run
RALPH_CURRENT_HAT=strategist ralph -c custom.yml tools task ensure --key k 't'
# 模拟 in-loop：
RALPH_CONFIG=$PWD/custom.yml RALPH_CURRENT_HAT=strategist ralph tools task ensure --key k2 't2'
```

---

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| 相对路径在 worktree / isolated cwd 下解析漂移 | 注入前将 `RALPH_CONFIG` resolve 为绝对路径；单测覆盖相对 `-c` |
| 改枚举名导致外部脚本匹配旧 hint | 优先改文案保留 variant；若 rename，changelog 注明 |
| `RALPH_CONFIG` 与 `-H` 同时存在时 emit 双源 | 保持现有 emit：`config_sources` + `hats_source` 分离；本计划只修 core file discovery |
| 下游 UAR 仍教用户创建最小 ralph.yml | 上游合并后通知下游删过时引导；本计划不改 UAR |

---

## Acceptance Criteria

1. 自定义文件名 + `-c`：**无需** workspace `ralph.yml`，`tools task ensure`（coordinator hat）成功。
2. `ralph run -c custom.yml` 注入的 hat 环境含 `RALPH_CONFIG`，裸 `ralph tools task` 能读到 `coordinator_hats`。
3. 仅 `ralph.yml` 的旧项目零改动可用。
4. 文档与 CLI help/hint 不再把「创建 ralph.yml」写成唯一恢复路径。
5. 不引入自动写盘 alias。

---

## Suggested commit series

1. `fix(cli): resolve_project_config_path SSOT for -c/RALPH_CONFIG/ralph.yml`
2. `fix(loop): inject RALPH_CONFIG into hat execution env`
3. `fix(tools): pass config_sources into task CLI / coordinator_hats loader`
4. `fix(emit): honor RALPH_CONFIG in workspace config fallback`
5. `docs: agent-facing config discovery precedence`

---

## Appendix — Minimal reproduction (pre-fix)

```bash
mkdir /tmp/ralph-c-repro && cd /tmp/ralph-c-repro
# custom.yml must include tasks.coordinator_hats: [strategist] plus valid cli/event_loop
ralph -c custom.yml tools task ensure --key k 't'
# expect today: coordinator_hats [] + no ralph.yml in workspace
ln -sf custom.yml ralph.yml
ralph tools task ensure --key k2 't2'
# expect today: success for strategist
```
