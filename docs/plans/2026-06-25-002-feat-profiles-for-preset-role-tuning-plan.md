---
title: 为 Preset 增加运行时 Profile 片段叠加机制
type: feat
status: u3-closed-u4-pending
date: 2026-06-25
origin: docs/brainstorms/2026-06-25-profiles-for-preset-role-tuning-requirements.md
execution_posture: test-first
---

# 为 Preset 增加运行时 Profile 片段叠加机制

## 概述

让 `ralph run` 在运行时从磁盘加载一组 markdown 片段，按顺序追加到当前 preset 对应 hat 的 `instructions` 末尾，实现「同一个 preset、多种风格」的切换能力。新增 `ralph inspect profiles` 用于预览解析结果与警告。

## 问题背景

Ralph 的 builtin preset 是编译进二进制的 YAML，运行时只能通过 `-H` / `-c` 做配置层叠加。用户若想让同一 preset 在不同项目或场景下表现不同（严格验证、快速原型、中文输出等），当前只能 fork preset 文件或在 `ralph.yml` 中塞 `hats.<id>.extra_instructions`。Profile 机制把风格差异拆成小型 markdown 片段，放在 `ralph-profiles/`（repo 级）或 `~/.config/ralph/profiles/`（user 级），运行时叠加到 hat instructions 上，避免 fork 并明确区分 repo 偏好与个人偏好。

## 需求追踪

- R1. `ralph run` 支持 `--profile <scope>:<name>` flag，可重复指定。
- R2. `repo:<name>` 解析为 `<project-root>/ralph-profiles/<name>/`。
- R3. `user:<name>` 解析为 `~/.config/ralph/profiles/<name>/`；若存在 `$XDG_CONFIG_HOME`，使用 `$XDG_CONFIG_HOME/ralph/profiles/`。
- R4. `ralph.yml` 支持 `profiles.default` 字段，值为逗号分隔的 profile spec 列表。
- R5. `ralph run` 支持 `--no-default-profiles`，仅关闭 config 默认 profile。
- R6. 从每个 active profile 目录加载 `<profile-dir>/<P>/<hat-id>.md`。
- R7. 只加载 `.md` 文件；片段对应 hat 不在当前 preset 中时，发出 warning 并忽略。
- R8. 显式请求的 profile 目录不存在时，立即报错并给出清晰路径提示。
- R9. profile 存在但缺少当前 preset 子目录时，发出 warning 并跳过。
- R10. 多个 profile 按 activation order 追加：config defaults 在前，CLI `--profile` 在后。
- R11. 每个片段以换行分隔追加到对应 hat 的 `instructions` 末尾。
- R12. Profile 片段在 `RalphConfig.normalize()` 之后、模板变量展开之前生效。
- R13. `ralph inspect profiles` 展示 active profiles、config defaults、解析到的片段路径、每段首行预览、warnings。
- R14. Profile 应用在所有配置合并完成且 `normalize()` 之后、event loop 消费 hat instructions 之前。
- R15. Profile 只修改 hat instructions，不修改 topology/backend/event_loop 等结构。

**来源 Actor:** A1 Operator、A2 Team member、A3 Agent / CI  
**来源关键流程:** F1 CLI 显式激活、F2 Config 默认激活、F3 团队共享 repo profile  
**来源验收示例:** AE1 repo profile 追加片段、AE2 defaults + CLI 顺序与 `--no-default-profiles`、AE3 错误/警告路径

## 范围边界

- v1 仅追加 hat instructions，不支持覆盖 backend、event_loop、topology。
- v1 仅支持精确匹配 profile/preset/hat-id，不支持通配符或正则。
- v1 不支持 profile 继承或嵌套，仅支持 ordered append。
- v1 不提供 `ralph profile create/init` 等 scaffold 命令。
- v1 不通过环境变量覆盖默认 `ralph-profiles/` 路径（后续迭代可加入）。
- v1 不支持 `-c profiles.default=...` 形式的 CLI 覆盖；`profiles.default` 只能通过 `ralph.yml` 设置。
- 不修改任何 preset YAML 的 event 拓扑，因此不触发 preset/schema 改动后的 7 步同步清单。

## 上下文与研究

### 相关代码与模式

- `crates/ralph-core/src/config/mod.rs` 中 `RalphConfig` 定义与 `Default` 实现；新增顶层配置块可参考 `agent_doc_sync`、`topic_owners` 的加入方式。
- `crates/ralph-core/src/config/ralph_config.rs` 中的 `normalize()` 已处理 `extra_instructions` 合并；profile 追加应在此之后。
- `crates/ralph-core/src/config/hat.rs` 中 `HatConfig.instructions` 是最终追加目标。
- `crates/ralph-cli/src/commands/run.rs` 中 `RunArgs` 使用 clap 派生；新增 flag 与此一致。
- `crates/ralph-cli/src/commands/run.rs` 中 `SubprocessTuiArgs` 负责把父进程 args 转发给 TUI 子进程；新增 flag 必须同步。
- `crates/ralph-cli/src/main.rs` 中 `Commands` 枚举与 dispatch；新增 `inspect` 顶层命令需在此注册。
- `crates/ralph-cli/src/cli/shared.rs` 中 `HatsSource` 提供 builtin 名、文件路径或远程 URL，用于推导 active preset 标识符。
- `crates/ralph-cli/src/preflight.rs` 中的 `load_config_for_preflight` 完成配置合并与 normalize，是 `ralph run` 的加载入口。
- autoloop 参考实现：`packages/core/src/profiles.ts` 提供 `parseProfileSpec`、`resolveProfileDir`、`resolveProfileFragments`、`applyProfileFragments` 的语义蓝本。

### 机构学习

- `AGENTS.md` / `CLAUDE.md` 要求：测试入口必须用 `cargo nextest run` 系列；新增 CLI flag 要同步 `scripts/ralph-zsh-plugin.zsh` 并安装到用户插件目录；用户面向文档必须用中文。
- `docs/solutions/developer-experience/ralph-zsh-builtin-hat-completion-maintenance-2026-05-26.md`：`builtin:*` 含冒号，zsh 补全必须用 `compadd` 而非 `_describe`。
- `docs/guide/cli-reference.md` 是 `scripts/check-cli-doc-drift.sh` 的检查对象，新增 flag 必须同步。
- `docs/achieved/plan/2026-06-08-003-feat-preset-static-lint-plan.md` 显示在 `RalphConfig` 新增顶层字段（`topic_owners`、`topic_format_whitelist`）的先例：默认空、serde round-trip、旧配置可解析。

## 关键技术决策

1. **Repo profile 路径**: 使用项目根目录 `ralph-profiles/`，不使用 `.ralph/profiles/`，因为 `.ralph/` 被 `.gitignore` 排除，repo profile 需要被 git 追踪。
2. **User profile 路径**: `~/.config/ralph/profiles/<name>/`；优先读取 `$XDG_CONFIG_HOME/ralph/profiles/<name>/`。
3. **Active preset 标识符**: builtin preset 直接使用 `HatsSource::Builtin(name)` 中的名称；文件 hats 使用文件路径的 stem（去掉 `.yml`/`.yaml`）作为 preset 子目录名；**远程 hats source 不支持 profile**。
4. **Profile 应用时机**: 在 `load_config_for_preflight` 返回后**立即**应用，位于 `config.validate()`、`run_auto_preflight` 与 event loop 之前；保证所有下游消费方看到的 instructions 一致。
5. **Repo profile base 路径**: 使用运行 `ralph run` 时的原始项目根目录；在 `--worktree` 子进程中，不能直接使用 `config.core.workspace_root`（可能指向 worktree），而应通过 `RALPH_WORKSPACE_ROOT` 或 `LoopContext` 中的主仓库根目录解析。
6. **Activation order**: `profiles.default` 列表先解析，再把 CLI `--profile` 追加到末尾；`--no-default-profiles` 仅排除 defaults，不影响显式 `--profile`。
7. **目录缺失行为**: 所有 active profile（包括 defaults）目录不存在时均立即报错，给出完整路径。
8. **片段加载顺序**: 同一 profile 内按 `.md` 文件名字典序加载；多个 profile 按 activation order 拼接。
9. **Warning 输出**: 运行时通过 stderr 输出 warnings；`ralph inspect profiles` 同时在 human/JSON 输出中展示。
10. **Profile 名安全校验**: 拒绝空、空白字符、包含路径分隔符或 `..` 的 profile 名，防止目录遍历或产生无意义目录。
11. **Config 反序列化**: `profiles.default` 支持逗号分隔的字符串，也兼容 YAML 字符串序列，便于未来扩展。
12. **CLI 覆盖限制**: v1 不支持 `-c profiles.default=...`；`ConfigSource::parse` 只识别 `core.*` 覆盖。

## 开放问题

### 规划期间已解决

- `ralph inspect profiles` 的具体层级：当前没有 `ralph inspect` 命令，新增顶层 `inspect` 命令，`profiles` 作为其子命令。
- Profile warnings 输出位置：运行时始终输出到 stderr；`inspect` 提供结构化查看。
- `-H ./my-hats.yml` 时的 preset 子目录名：使用文件 stem 作为标识符。
- `-H http://...` 时的 profile 行为：v1 不支持，报错。
- 是否支持 `$RALPH_REPO_PROFILES_DIR` 覆盖 repo 路径：v1 不支持，后续迭代再考虑。

### 推迟到实现阶段

- 具体 helper 函数命名（如 `apply_profile_fragments` / `resolve_profile_fragments`）。
- `ralph inspect profiles` human 输出的具体列宽与缩进。
- 是否需要把 profile 信息加入 `ralph run --dry-run` 摘要。

## 代码组织约束

- 功能复杂时必须按模块拆分，禁止把全部逻辑塞进一个巨型文件。
- **硬上限：单个源文件不得超过 1500 行**（含测试代码）。接近或超过时，必须拆成多个子模块。
- 拆分示例（如 `crates/ralph-core/src/profiles.rs` 超过上限）：
  - `crates/ralph-core/src/profiles/mod.rs`：公共类型与 API 入口。
  - `crates/ralph-core/src/profiles/spec.rs`：`ProfileSpec` / `ProfileScope` 解析。
  - `crates/ralph-core/src/profiles/loader.rs`：目录解析与文件读取。
  - `crates/ralph-core/src/profiles/applier.rs`：片段追加到 `HatConfig`。
- 拆分后仍保持单元内可独立测试，不能因拆文件引入跨文件耦合。

## 输出结构

```
crates/ralph-core/src/
  config/
    profiles.rs          (new)  # ProfilesConfig / ProfileSpec / ProfileScope
crates/ralph-core/src/
  profiles.rs            (new)  # 解析、目录解析、片段加载、应用；若超过 1500 行则拆为 profiles/ 子模块
crates/ralph-cli/src/
  commands/
    inspect.rs           (new)  # ralph inspect profiles
crates/ralph-cli/src/
  commands/
    run.rs                 (modify)  # --profile / --no-default-profiles / 应用点 / SubprocessTuiArgs
crates/ralph-cli/src/
  main.rs                (modify)  # Commands::Inspect 注册与分发
crates/ralph-cli/src/
  commands/mod.rs        (modify)  # pub mod inspect
docs/guide/
  cli-reference.md       (modify)
docs/guide/
  configuration.md       (modify)
docs/concepts/
  profiles.md            (new, optional but recommended)
scripts/
  ralph-zsh-plugin.zsh   (modify)
```

## 高层技术设计

> *本节用于帮助审阅者理解整体方向，不是实现规范。*

```mermaid
graph LR
    A[ralph run --profile repo:strict] --> B[RunArgs]
    B --> C[collect_active_specs:<br/>profiles.default + CLI flags]
    C --> D[ProfileApplier]
    D --> E[resolve repo/user dirs]
    E --> F[read <preset>/<hat>.md]
    F --> G[append to HatConfig.instructions]
    G --> H[event loop]
```

- `ProfileSpec` 是已解析的 `{scope, name}`，由字符串 `repo:strict` 解析而来。
- `collect_active_specs` 根据 `--no-default-profiles` 决定是否读取 `config.profiles.default`。
- `resolve_profile_fragments` 是纯函数，返回按 hat 聚合的片段与 warnings，不修改 `RalphConfig`。
- `apply_profile_fragments` 内部调用 `resolve_profile_fragments`，再把片段追加到 `hats[id].instructions`。
- `ralph inspect profiles` 只调用 `resolve_profile_fragments`，不修改配置。
- `SubprocessTuiArgs` 必须把新增 flag 转发给子进程，否则 TTY 模式下 profile 会静默丢失。

## 实现单元

本计划严格执行「纯粹串行、绝对隔离、TDD 闭环」：每个单元 100% 编码+测试完成后才能进入下一个单元；当前单元所需数据在单元内闭环，不依赖后续单元。

---

- [ ] U1. **在 `RalphConfig` 中新增 `profiles` 配置块**

**目标:** 让 `ralph.yml` 能够声明 `profiles.default`，并在 `RalphConfig` 中类型化地保存为一组 `ProfileSpec`。

**需求:** R4

**依赖:** 无

**文件:**
- 创建: `crates/ralph-core/src/config/profiles.rs`
- 修改: `crates/ralph-core/src/config/mod.rs`
- 修改: `crates/ralph-core/src/config/ralph_config.rs`
- 修改: `crates/ralph-core/src/lib.rs`

**方法:**
- 在 `config/profiles.rs` 中定义 `ProfileScope`（Repo / User）、`ProfileSpec`、`ProfilesConfig`。
- 为 `ProfilesConfig.default` 提供自定义 serde deserializer，同时支持逗号分隔字符串和 YAML 字符串序列；解析后空格 trim。
- 在 `RalphConfig` 中新增 `pub profiles: ProfilesConfig`，并在手动 `Default` 实现中初始化为空。
- 在 `crates/ralph-core/src/lib.rs` 的 `pub use config::{...}` 中导出新增类型。
- **兼容性前置检查**: 确认 `RalphConfig` 未使用 `#[serde(deny_unknown_fields)]`；若已使用，需先调整 serde 属性，确保旧配置无 `profiles` 字段时仍可解析。

**执行提示:** 测试先行；本单元只处理类型与反序列化，不接触文件系统或事件循环。

**测试场景:**
- Happy path: YAML `profiles:\n  default: repo:strict, user:my-style` 解析为两个 `ProfileSpec`。
- Happy path: YAML `profiles:\n  default: [repo:strict, user:my-style]` 同样解析为两个 spec。
- Edge case: 多余空格 `repo:strict , user:my-style` trim 后正确。
- Edge case: `profiles` 省略时 defaults 为空列表。
- Edge case: `profiles.default: ""` 解析为空列表。
- Compatibility: 旧 ralph.yml 不含 `profiles` 字段时仍可正常反序列化（覆盖 `deny_unknown_fields` 风险）。
- Round-trip: `serde_yaml::to_value` / `from_value` 后 defaults 不变。

**验收:** `cargo nextest run -p ralph-core -- profile_config` 通过；`RalphConfig::default()` 能正常构建。

---

- [ ] U2. **实现 profile 解析、目录解析与片段加载模块**

**目标:** 提供纯函数 API，把一组 `ProfileSpec` 解析为按 hat 聚合的 markdown 片段，并安全地追加到 `RalphConfig.hats` 的 `instructions` 中。

**需求:** R1–R3, R6–R12, R15

**依赖:** U1

**文件:**
- 创建: `crates/ralph-core/src/profiles.rs`
- 修改: `crates/ralph-core/src/lib.rs`
- 修改（如需要）: `crates/ralph-core/Cargo.toml`（确认 `tempfile` 已作为 dev-dep 可用）

**方法:**
- 定义常量：`const REPO_PROFILES_DIR: &str = "ralph-profiles";`、`const USER_PROFILES_DIR: &str = ".config/ralph/profiles";`。
- 提供 `parse_profile_spec(s: &str) -> Result<ProfileSpec, ProfilesError>`，校验 scope 为 `repo`/`user`、name 非空且 trim 后非空、不含路径分隔符或 `..`。
- 提供 `resolve_profile_dir(spec, workspace_root) -> PathBuf`：repo 用 `workspace_root/REPO_PROFILES_DIR/<name>`；user 用 `$XDG_CONFIG_HOME/USER_PROFILES_DIR/<name>` 或 `HOME/USER_PROFILES_DIR/<name>`；`HOME` 缺失时返回清晰错误。
- 提供 `resolve_profile_fragments(config, preset_name, specs, workspace_root) -> Result<(HashMap<String, Vec<ProfileFragment>>, Vec<String>), ProfilesError>`：
  - 对每个 spec，解析目录；不存在则返回 `Err`。
  - 定位 `<dir>/<preset_name>`，不存在则记录 warning 并 continue。
  - 读取该目录下所有 `.md` 文件（按文件名排序），若对应 hat-id 不在 `config.hats` 中则记录 warning 并忽略。
  - 返回按 hat 聚合的片段列表（含路径与内容）与 warnings。
- 提供 `apply_profile_fragments(config, preset_name, specs, workspace_root) -> Result<Vec<String>, ProfilesError>`：
  - 内部调用 `resolve_profile_fragments`。
  - 对每个匹配到的 hat，将片段以换行分隔追加到 `instructions` 末尾。
- 函数签名只接受不可变 workspace root 和可变 config，不产生其他副作用。

**执行提示:** 测试先行；所有文件系统交互通过 `tempfile::tempdir()` 在单元测试内构造，不依赖真实用户目录或 CLI。

**测试场景:**
- Happy path: repo profile `strict/ce-executor-serial/executor.md` 被追加到 `executor` hat 的 instructions 末尾（Covers AE1）。
- Happy path: user profile 通过设置 `HOME`/`XDG_CONFIG_HOME` 环境变量在测试中解析并加载。
- Edge case: 多个 profile 按传入顺序追加；config defaults 先、CLI spec 后的顺序在调用方保证，本函数按传入 `Vec` 顺序处理。
- Edge case: `instructions` 不以 `\n` 结尾时，先补 `\n` 再追加片段。
- Edge case: 片段文件自身以 `\n` 结尾时，不重复追加多余空行。
- Edge case: 空片段文件只追加一个换行（若 instructions 原本无换行）。
- Edge case: `HOME` 未设置时 user scope 返回清晰错误。
- Error path: profile 目录不存在返回 `Err`，消息包含完整路径（Covers R8 / AE3）。
- Error path: profile 名为空、全空白、`../evil`、含 `/` 时被拒绝。
- Error path: 非 UTF-8 `.md` 文件返回 IO 错误（不 panic）。
- Warning path: profile 存在但缺少当前 preset 子目录；记录 warning（Covers R9 / AE3）。
- Warning path: profile 中有 `ghost.md` 但 preset 没有 `ghost` hat；记录 warning（Covers R7 / AE3）。
- R15 invariance: 应用 profile 前后，比较 `HatConfig` 除 `instructions` 外的所有字段（triggers、publishes、backend 等）保持不变。

**验收:** `cargo nextest run -p ralph-core -- profiles` 通过；错误路径的消息包含清晰路径；R15 不变性测试通过。

---

- [ ] U3. **为 `ralph run` 增加 `--profile` 与 `--no-default-profiles` flag**

**目标:** CLI 能接收 profile 相关 flag，并把 config defaults 与 CLI flags 合并成最终的 active spec 列表；同时确保子进程 TUI 模式能正确转发这些 flag。

**需求:** R1, R5

**依赖:** U2

**文件:**
- 修改: `crates/ralph-cli/src/commands/run.rs`
- 修改: `crates/ralph-cli/src/main.rs`（无子命令时的默认 `RunArgs`）

**方法:**
- 在 `RunArgs` 中新增：
  - `#[arg(long = "profile", value_name = "SCOPE:NAME", action = ArgAction::Append)] pub profiles: Vec<String>`
  - `#[arg(long)] pub no_default_profiles: bool`
- 在 `default_run_args()` 中补充默认值：`profiles: Vec::new()`、`no_default_profiles: false`。
- 在 `run.rs` 中新增私有 helper `collect_active_profile_specs(config: &RalphConfig, args: &RunArgs) -> Result<Vec<ProfileSpec>, ProfilesError>`：
  - 若 `no_default_profiles` 为 false，先解析 `config.profiles.default`。
  - 再按顺序解析 `args.profiles` 并追加。
  - 任一 spec 解析失败即返回错误。
- **子进程 TUI 转发**: 找到 `SubprocessTuiArgs` 结构体，新增 `profiles: Vec<String>` 与 `no_default_profiles: bool` 字段；在 `SubprocessTuiArgs::new` 与生成子进程命令行时同步转发；确保 `Cli::try_parse_from(...)` 在子进程中能重新解析到这些 flag。

**执行提示:** 测试先行；本单元只验证 CLI 解析、spec 收集逻辑与 flag 转发，不调用文件系统或 profile 应用。

**测试场景:**
- Happy path: `Cli::try_parse_from(["ralph", "run", "--profile", "repo:strict", "--profile", "user:my-style"])` 成功。
- Happy path: `Cli::try_parse_from(["ralph", "run", "--no-default-profiles"])` 成功。
- Happy path: 无子命令默认 `ralph` 也能识别 `--profile repo:strict`。
- Subprocess TUI forwarding: `SubprocessTuiArgs::new(&args,...).to_argv()` 包含 `--profile repo:strict` 与 `--no-default-profiles`（或通过等价方式断言）。
- Error path: `Cli::try_parse_from(["ralph", "run", "--profile", "bad-spec"])` 在 helper 层解析失败（本单元只测 helper）。
- Helper: `profiles.default = [repo:base]` + CLI `user:extra` => 顺序 `[repo:base, user:extra]`（Covers AE2）。
- Helper: `--no-default-profiles` + CLI `user:extra` => 仅 `[user:extra]`（Covers AE2）。
- Helper: 空 defaults + 空 CLI => 空列表。

**验收:** `cargo nextest run -p ralph-cli --bin ralph -- profile` 通过；新增字段不破坏现有 CLI 解析测试；子进程 TUI 转发测试通过。

---

- [ ] U4. **在 `ralph run` 运行流程中应用 profile**

**目标:** 在所有配置合并、normalize、CLI 覆盖完成后，把 active profile 片段追加到 hat instructions，并保证 preflight、validate、event loop 看到的 instructions 一致。

**需求:** R10, R11, R14, R15

**依赖:** U3

**文件:**
- 修改: `crates/ralph-cli/src/commands/run.rs`

**方法:**
- 在 `run_command` 中，`let mut config = preflight::load_config_for_preflight(...).await?;` **返回后立即**插入 profile 应用（在所有 CLI 覆盖之后、`config.validate()` 与 `run_auto_preflight` 之前）：
  - 根据 `hats_source` 推导 active preset 名：
    - `HatsSource::Builtin(name)` => `name`
    - `HatsSource::File(path)` => path 的 file stem
    - `HatsSource::Remote(_)` => 不支持 profile；若此时有 active specs，返回清晰错误。
    - `None` => 无 preset 名
  - 调用 `collect_active_profile_specs(&config, &args)` 得到 active specs。
  - 若 specs 非空但无 preset 名（或远程 hats），向 stderr 打印 warning 或返回错误；禁止 panic。
  - repo profile base 路径优先使用原始项目根目录：在 `--worktree` 子进程中，通过 `RALPH_WORKSPACE_ROOT` 环境变量或 `LoopContext` 中保留的主仓库根目录获取，避免使用指向 worktree 的 `config.core.workspace_root`。
  - 否则调用 `ralph_core::profiles::apply_profile_fragments(&mut config, preset_name, &specs, &repo_profile_base)`。
  - 把返回的 warnings 逐行 eprintln。
- 确保后续 `config.validate()`、`run_auto_preflight` 与 event loop 看到的是同一套已叠加 instructions。

**执行提示:** 测试先行；把推导 preset 名与调用应用的逻辑抽成可单元测试的 helper，不启动真实 backend。

**测试场景:**
- Integration helper: builtin hats source `builtin:ce-executor-serial` 能正确把 repo profile 片段追加到 `executor` hat。
- Integration helper: file hats source `./my-hats.yml` 使用 stem `my-hats` 作为 preset 子目录名。
- Edge case: `hats_source` 为 `None` 但传了 `--profile`，仅打印 warning，config 不变，不 panic。
- Edge case: `hats_source` 为 `Remote` 且传了 `--profile`，返回清晰错误。
- Edge case: profile 应用发生在 `normalize()` 之后，验证 `extra_instructions` 已先合并到 `instructions` 再追加 profile 片段。
- Worktree path: 在模拟 worktree 环境下，repo profile 仍从主仓库根目录的 `ralph-profiles/` 解析（可通过设置 `RALPH_WORKSPACE_ROOT` 测试）。
- Preflight consistency: profile 应用后调用 `run_auto_preflight`（或模拟 preflight 读取 config）不会崩溃，且 preflight 看到已叠加 instructions。
- Error path: 显式 profile 目录不存在时，`run_command` 提前返回错误并包含路径。

**验收:** `cargo nextest run -p ralph-cli --bin ralph -- run_profile` 通过；`ralph run --dry-run -H builtin:debug --profile repo:strict` 在 profile 目录存在时正常完成；worktree 路径不报错。

---

- [ ] U5. **新增 `ralph inspect profiles` 命令**

**目标:** 让用户能在不启动 loop 的情况下预览 profile 解析结果、片段路径与 warnings。

**需求:** R13

**依赖:** U2, U3 的 flag 模式

**文件:**
- 创建: `crates/ralph-cli/src/commands/inspect.rs`
- 修改: `crates/ralph-cli/src/commands/mod.rs`
- 修改: `crates/ralph-cli/src/main.rs`

**方法:**
- 新增 `commands/inspect.rs`：
  - `InspectArgs` 含 `#[command(subcommand)] command: InspectCommands`
  - `InspectCommands::Profiles(InspectProfilesArgs)`
  - `InspectProfilesArgs` 复用 `--profile`（可重复）与 `--no-default-profiles`，并可指定 `--format human|json`
- 实现 `inspect_profiles_command(config_sources, hats_source, args, use_colors)`：
  - 调用 `load_config_for_preflight` 获取 config。
  - 推导 active preset 名（同 U4 helper，远程 hats 报错）。
  - 收集 active specs（同 U3 helper）。
  - repo profile base 路径处理同 U4（注意 worktree 场景）。
  - 调用 `ralph_core::profiles::resolve_profile_fragments`（U2 中暴露，只解析不修改 config）得到每个 hat 的片段列表与 warnings。
  - 输出：
    - human: 每行一个 active spec；列出每个 `<profile>/<preset>/<hat>.md` 路径与首行预览（最多 60 字符）。
    - json: 包含 `profiles`、`preset`、`fragments`、`warnings` 的结构化对象。
- 在 `main.rs` 注册 `Commands::Inspect(commands::inspect::InspectArgs)` 并添加 dispatch 分支。

**执行提示:** 测试先行。

**测试场景:**
- Happy path: `Cli::try_parse_from(["ralph", "inspect", "profiles", "--profile", "repo:strict"])` 解析成功。
- Happy path: human 输出包含 profile spec、preset 名、fragment 路径、首行预览。
- Happy path: json 输出包含 `profiles[].spec`、`fragments[].path`、`fragments[].preview`、`warnings`。
- End-to-end CLI: 在临时工作区创建 `ralph-profiles/strict/debug/investigator.md`，执行 `ralph inspect profiles -H builtin:debug --profile repo:strict --format json`，断言输出包含对应 fragment 路径与 preview。
- Error path: 指定了不存在的 profile，命令返回错误并包含路径。
- Edge case: 无 hats source 但传了 profile，输出 warning 而不 panic。
- Edge case: 远程 hats source + profile 返回清晰错误。

**验收:** `cargo nextest run -p ralph-cli --bin ralph -- inspect` 通过；`ralph inspect profiles --help` 正常显示。

---

- [ ] U6. **同步文档与 zsh 补全**

**目标:** 让新 flag 和新命令对用户可见，并保持命令行补全准确。

**需求:** R1, R4, R13（可发现性）

**依赖:** U3, U5

**文件:**
- 修改: `docs/guide/cli-reference.md`
- 修改: `docs/guide/configuration.md`
- 创建: `docs/concepts/profiles.md`（推荐）
- 修改: `scripts/ralph-zsh-plugin.zsh`
- 若 `CLAUDE.md` / `AGENTS.md` 的 CLI 列表提到 `ralph run` flag，则同步两者（用 `cp`）。

**方法:**
- `docs/guide/cli-reference.md`：在 `ralph run` Options 表中增加 `--profile <scope>:<name>` 与 `--no-default-profiles`；新增 `ralph inspect profiles` 小节；在文档中区分 `inspect`（只读诊断）与 `preset`（模板管理）的语义，避免混淆。
- `docs/guide/configuration.md`：新增 `profiles.default` 字段说明与示例 YAML。
- `docs/concepts/profiles.md`（可选但推荐）：面向用户的概念说明，包含目录结构示例与 `repo:`/`user:` 区别。
- `scripts/ralph-zsh-plugin.zsh`：
  - 在 `_ralph_run_args` 数组中加入 `--profile` 和 `--no-default-profiles`。
  - 在 `_RALPH_COMMANDS` 中加入 `inspect`。
  - 新增 `_ralph_inspect_profiles_args` 数组与对应补全分支（`--profile` 需要值提示，可简单给出 `repo:`/`user:` 前缀候选）。
  - 安装：`cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`。

**测试期望:** 无自动化单元测试 —— 属于文档/运维同步。

**验收:**
- `ralph run --help` 与 `ralph inspect profiles --help` 显示新增 flag。
- 运行 `./scripts/check-cli-doc-drift.sh`（如存在）无新增漂移。
- 在 zsh 中执行 `ralph run --profile <TAB>` 与 `ralph inspect <TAB>` 能看到补全候选；`whence -f _ralph` 与真实 `<TAB>` 行为一致。
- `docs/concepts/profiles.md` 已创建（推荐项）。

## 系统级影响

- **变更面:** 仅新增 `RalphConfig.profiles` 字段与 hat instructions 内容；不改动 event topology、`publishes`/`subscribes`、backend、event_loop 配置。
- **配置加载顺序:** profile 在 `load_config_for_preflight` 返回后立即应用，位于 `normalize()` 与 CLI 覆盖之后、`validate()` 与 `run_auto_preflight` 之前；因此 preflight、validate、event loop 看到的 instructions 一致。
- **子进程 TUI:** 新增 flag 必须通过 `SubprocessTuiArgs` 转发，否则 TTY 模式下子进程会丢失 profile；已在 U3 中作为硬要求。
- **错误传播:** profile 目录缺失、spec 格式错误、远程 hats source 不支持 profile 均通过 `anyhow::Result` 提前返回，消息包含完整路径或明确原因。
- **状态与回滚:** profile 是纯运行时内存叠加，不写持久化状态；回滚到旧版本二进制不会留下不兼容数据。
- **其他接口:** 不修改 `RalphConfig` 的序列化形状之外的部分；下游 consumers 若反序列化 RalphConfig YAML 会忽略未知 `profiles` 键（serde 默认行为）。

## 风险与依赖

| 风险 | 缓解 |
|------|------|
| Profile 名为空/空白/含 `..` 或 `/` 导致目录遍历或奇怪目录 | U2 中校验 name，拒绝空、空白、路径分隔符与 `..` |
| 默认 profile 目录缺失导致 `ralph run` 启动失败 | 按 R8 显式报错并给出完整路径；用户可用 `--no-default-profiles` 绕过 |
| 文件 hats 的 stem 与团队期望的 preset 子目录名不一致 | 在文档中说明：文件 hats 使用文件 stem；建议团队统一命名 |
| `RalphConfig` 若启用 `deny_unknown_fields` 会导致旧配置解析失败 | U1 先做兼容性前置检查，必要时调整 serde 属性 |
| 子进程 TUI 模式未转发新增 flag，导致 profile 静默丢失 | U3 中同步更新 `SubprocessTuiArgs` 并添加转发测试 |
| 远程 hats source 与 profile 组合行为未定义 | U4 中明确不支持并返回清晰错误 |
| `--worktree` 模式下 repo profile 被解析到 worktree 路径 | U4 中通过 `RALPH_WORKSPACE_ROOT` / `LoopContext` 主仓库根目录解析 |
| zsh 补全未同步导致用户体验断裂 | U6 中更新插件并安装到用户插件目录 |
| CLI 文档漂移导致 CI gate 失败 | U6 中同步 `docs/guide/cli-reference.md` 并运行 drift 检查 |
| `ralph-cli` 串行测试耗时增加 | 新增测试均为纯函数/ tempfile，不加重 Mutex/sleep 负担 |

## 文档 / 运维说明

- 用户面向文档必须中文撰写。
- 更新 `docs/guide/cli-reference.md` 与 `docs/guide/configuration.md`。
- 推荐创建 `docs/concepts/profiles.md` 作为用户概念入口。
- 若 `CLAUDE.md` / `AGENTS.md` 的 CLI 列表被修改，必须 `cp CLAUDE.md AGENTS.md` 保持两者一致。
- 安装 zsh 插件：`cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`。
- 最终验证：
  - `cargo nextest run -p ralph-cli --bin ralph -- profile`
  - `cargo nextest run -p ralph-core -- profiles`
  - `./scripts/run-tests.sh`

## 来源与参考

- **需求来源:** [docs/brainstorms/2026-06-25-profiles-for-preset-role-tuning-requirements.md](docs/brainstorms/2026-06-25-profiles-for-preset-role-tuning-requirements.md)
- **参考实现:** `/Users/pittcat/Dev/Rust/autoloop/packages/core/src/profiles.ts`
- **相关代码:** `crates/ralph-core/src/config/mod.rs`、`crates/ralph-core/src/config/ralph_config.rs`、`crates/ralph-core/src/config/hat.rs`、`crates/ralph-cli/src/commands/run.rs`、`crates/ralph-cli/src/main.rs`、`crates/ralph-cli/src/cli/shared.rs`
- **相关机构文档:** `AGENTS.md`、`docs/solutions/developer-experience/ralph-zsh-builtin-hat-completion-maintenance-2026-05-26.md`、`docs/achieved/plan/2026-06-08-003-feat-preset-static-lint-plan.md`
