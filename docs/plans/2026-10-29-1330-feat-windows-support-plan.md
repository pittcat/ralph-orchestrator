---
title: Windows 平台支持（pwsh / cmd）- Plan
type: feat
date: 2026-10-29
artifact_contract: ce-unified-plan/v1
product_contract_source: ce-plan-bootstrap
execution: code
status: draft
---

# Windows 平台支持（pwsh / cmd）- Plan

## Goal Capsule

- Objective：`ralph` CLI 能在 Windows（x86_64-pc-windows-msvc）上通过 `cargo check`、编译并运行；单 loop、hooks、preflight、`ralph web`、parallel/worktree、DAG scheduler 在 Windows 上语义正确，且不引入对 Linux/macOS 的回归。
- Means：分层落地——先修编译（Phase 0）→ 单 loop 可用（Phase 1）→ 正确性原语（Phase 2）→ 并行/DAG（Phase 3）→ 体验与发布（Phase 4）。所有改动遵循"新增 `#[cfg(windows)]` 分支、`#[cfg(unix)]` 现有代码零改动"的硬约束。
- Authority：本计划定义每个 Phase 的验收行为；CI 新增 `windows-latest` job，Linux job 保持原样。
- Shell scope：目标 shell 为 **pwsh 7+（最新 PowerShell）** 与 **cmd.exe**；Git Bash 作为兼容层选项。**不做 Windows PowerShell 5.1 兼容**。
- Stop conditions：任何 Unit 导致 Linux `./scripts/run-tests.sh` 回归 → 回退该 Unit 的共享代码改动；Windows 侧允许保留 cfg 分支。
- Delivery：实施者按 Phase → Unit 顺序交付代码 + 测试；每 Phase 需 Windows CI job 与 Linux 基线双绿。

---

## Non-Negotiable Constraints

- C1. **Linux/macOS 行为冻结**：不改动 `#[cfg(unix)]` 下的任何现有代码路径；共享代码中 Unix 侧默认值必须保持逐字一致（如 shell 默认仍是 `sh -c`、锁默认仍是 flock）。若某处必须重构共享代码，Unix 侧的函数语义/默认参数不得改变。
- C2. **依赖隔离**：所有 Windows 专用依赖写入 `[target.'cfg(windows)'.dependencies]`，Linux 构建不拉取、不编译。
- C3. **`unsafe_code = "forbid"`（Cargo.toml:27）**：Windows 方案优先 std 1.89+ 的 `File::lock`/`try_lock`/`lock_shared`、`dunce`、`which`、`sysinfo`、`junction`、`win32job`、`command-group` 等安全封装 crate。若确需 unsafe（如 SetConsoleOutputCP），只允许放入新加的 `cfg(windows)` 专属小模块并在 CI 中单独审计，禁止扩散。
- C4. **测试入口不变**：开发期用 `cargo nextest run -p <pkg> -- <subset>`；最终验证 `./scripts/run-tests.sh` + doctest。Windows 开发者用 Git Bash 跑 `.sh`；长期迁移到 `cargo xtask`（Phase 4）。
- C5. **禁止回归 Windows 现有占位语义而不自知**：当前 `cfg(not(unix))` 分支多为"返回 true / Ok / no-op / Unsupported"。改任何一个都必须同时给出 Windows 真实实现或显式错误，禁止"能编译但悄悄做错"的状态存续。

## Product Contract

### Summary

19 个 builtin preset 中有 10 个含 bash 语法；`preflight`/`workspace`/`pty_kernel`/`web`/`attach`/`restart` 均写死 `sh`/`bash`；`nix` 在 ralph-cli 中仅声明为 Unix 依赖但 `dag_scheduler` 两处裸调用；锁/PID/进程组/symlink/信号在 Windows 全部缺失或 stub。本计划把 Windows 提升为一等公民，pwsh 7+ / cmd / Git Bash 三壳可用，Linux 不变。

### Requirements

**R-compile（Phase 0）**

- R0.1 `cargo check --workspace` 在 Windows 通过；`dag_scheduler/mod.rs:573`、`dag_scheduler/spawn.rs:591` 两处 `nix::` 调用改走跨平台 `probe_pid` 抽象。
- R0.2 新增 `.gitattributes`（`* text=auto eol=lf` + 二进制标记），防止 `core.autocrlf` 污染 fixtures / `include_str!` 嵌入文件 / `.sh`。
- R0.3 CI 新增 `windows-latest` job：先跑 `cargo check --workspace --all-targets` + `cargo clippy`；Linux job 不变。

**R-single-loop（Phase 1）**

- R1.1 新增 `ralph-core::shell` 模块，统一提供 `shell_command(cmd)`：Unix 固定 `sh -c`；Windows 解析链 `core.shell` 配置 / `RALPH_SHELL` env → `bash`（Git Bash）→ `pwsh -NoProfile -NonInteractive -Command` → `cmd /d /s /c`（`raw_arg`）。**显式排除 `C:\Windows\System32\bash.exe`（WSL 启动器）**。
- R1.2 backend 自动检测与 spawn 改用 `which` crate 解析 `PATHEXT`；npm `.cmd`/`.ps1` shim 识别为可用；Windows 上 prompt 一律走 stdin/文件（含换行参数 std 会拒绝；cmd 8191 上限）。
- R1.3 hooks executor 按扩展名映射解释器：`.ps1`→`pwsh -File`、`.sh`→Git Bash、`.py`→`python`、`.cmd/.bat`→直接 spawn；`is_executable`/`mark_executable`/`preflight` 的 Unix-only 权限检查替换为按 `PATHEXT` 判断。
- R1.4 `HOME` 缺失时按 `config_resolution.rs:298` 已有链回退 `USERPROFILE → HOMEDRIVE+HOMEPATH`；`profiles.rs` 复用同一 helper。
- R1.5 读取 events/payload 时容忍 UTF-8 BOM，识别 UTF-16 LE/BE BOM 并转码（pwsh `Out-File`、redirect 输出）。
- R1.6 `Command::new("npm")`/`"node"`/`"tsx"` 在 `web.rs` 改走 `which` 解析 + cmd shim 处理。

**R-primitives（Phase 2）**

- R2.1 文件锁：`loop_lock`/`loop_registry`/`merge_queue`/`file_lock`/`idempotent_log` 的 Windows 分支用 std `File::lock*`；**锁 sidecar 文件（`*.guard`）避免 mandatory-lock 语义下 metadata 句柄读不到**。读侧继续 `fs::read` 非锁定文件。
- R2.2 PID 存活：统一 `probe_pid(pid) -> Alive/Dead/Unknown`；Unix 用 nix `kill(pid, None)`，Windows 用 `sysinfo` 刷新指定 PID + 比对 start_time 防 PID 复用。`ralph-api` 的 `Command::new("kill")` 两处改为调此函数。
- R2.3 进程组/进程树：Windows 下 backend、hook、web 子进程全部放入 **Job Object**（`KILL_ON_JOB_CLOSE`）；`CREATE_NEW_PROCESS_GROUP` + `CTRL_BREAK_EVENT` 发优雅信号，超时后 TerminateJobObject；`kill_process_tree` 在 Windows 用 `sysinfo` 枚举 + `Process::kill` 兜底。
- R2.4 信号：`tokio::signal::windows::{ctrl_break, ctrl_close, ctrl_logoff, ctrl_shutdown}` 映射到现有 `SIGTERM`/`SIGHUP` 语义；`ralph loops stop --force` 在 Windows 调用进程树终止而非 bail。
- R2.5 `ralph web` 的 `Command::new("npm")`/`AsyncCommand::new("sh")` 改走 shell 抽象 + Job Object，保证退出后 3000/5173 端口释放。
- R2.6 `ralph loops attach`：Windows 按 `RALPH_SHELL → pwsh → %COMSPEC%` 选择；不再写死 `/bin/bash`。

**R-parallel（Phase 3）**

- R3.1 worktree symlink：`setup_memory_symlink`/`setup_specs_symlink`/`setup_code_tasks_symlink`/`worktree.rs:1081`/`target_checkout.rs`/`merge_batch` 在 Windows 依次尝试 `symlink_dir` → `junction`（dir）→ hardlink/copy+回写（file）；全失败则**明确报错拒绝进入 worktree 模式**，禁止 `Ok(false)` 静默跳过。
- R3.2 路径安全：`workspace_mutation_guard.rs:126/135`、`ephemeral_isolation.rs:392`、`parallel_forge_handoff.rs:150/833`、`acceptance_and_lifecycle.rs:1004`、`inspect/descriptor.rs:139` 的 `starts_with('/')`/`split('/')` 改为 `Path::is_absolute()` + `components()` 遍历 `ParentDir`；repo-relative 比较统一 normalize 为 `/`；Windows 下比较忽略大小写。
- R3.3 `dunce::canonicalize` 用于 workspace 包含关系比较，剥掉 `\\?\` 前缀。
- R3.4 `pty_kernel` stdin 注入改为直接 `Stdio::from(file)`（Windows）或 PTY master 写入，不再 `sh -c 'exec "$@" < "$prompt_file"'`；`/proc/self/fd` 防逃逸在 Windows 退化为 canonicalize+starts_with 并在文档声明残余。
- R3.5 `DagEnvAllowlist`/`spawn.rs` env 白名单在 Windows 大小写不敏感（`Path` vs `PATH`），并内置 Windows baseline：`SystemRoot/COMSPEC/PATHEXT/TEMP/TMP/USERPROFILE/APPDATA/LOCALAPPDATA`。
- R3.6 `ralph run` restart 在 Windows 用 `DETACHED_PROCESS|CREATE_NEW_PROCESS_GROUP` 重拉 `ralph run …` 后退出当前进程（替代 `sh -lc`）。
- R3.7 长路径：文档要求 `git config core.longpaths true`；exe manifest `longPathAware`（dist）。
- R3.8 原子 rename 在 Windows 加短退避重试（≤5 次，20–100ms），缓解 AV/indexer 持有的 `os error 5`。

**R-experience（Phase 4）**

- R4.1 新增 `ShellArg::PowerShell` 补全（`completions.rs`，clap_complete 原生支持）。cmd 无补全机制不处理。
- R4.2 `ralph emit`/`ralph wave emit`/`ralph task` 增加 `--payload-file <path>` / `--payload -`（stdin），Windows 文档推荐文件方式，规避 pwsh/cmd 引号二次解析。
- R4.3 prompt 注入 `RALPH_HOST_OS` + 当前 shell 类型；`ralph-tools-*.md` 嵌入文档给关键命令补 pwsh 写法；bash-only preset 段落标注 "Unix shell"。
- R4.4 `scripts/run-tests.sh` 等核心脚本迁移到 `cargo xtask`（两阶段 nextest + env scrub + doctest），`.sh` 保留一行转调；PowerShell 用户可直接 `cargo xtask test`。
- R4.5 `Justfile` 加 `set windows-shell := ["bash", "-cu"]`（仅 Windows 生效）。
- R4.6 dist `targets` 加 `x86_64-pc-windows-msvc`，产出 PowerShell 安装脚本；release workflow 已有 `enable windows longpaths` 步骤可复用。
- R4.7 测试基建：新增 Rust 测试辅助 bin（`CARGO_BIN_EXE_*`）替代 sh 假 backend；无法迁移的 `#[cfg(unix)]` 测试保留并在 plan 中列出。

### Out of Scope

- Windows PowerShell 5.1（`powershell.exe`）兼容层。
- ARM64 Windows（可作为后续增量）。
- 对 `powershell.exe` 特殊引号规则的适配。
- WSL 路径映射（显式排除 `C:\Windows\System32\bash.exe`）。

---

## Phases & Units

### Phase 0 — 编译与 CI 基线（blocker）

| Unit | Scope | 关键交付 | 验收 |
|---|---|---|---|
| U0.1 | `dag_scheduler/mod.rs` `spawn.rs` nix 裸调用 | `probe_pid` 抽象（Unix nix，Windows sysinfo） | Windows `cargo check` 0 error |
| U0.2 | unused imports / cfg-warnings | import 移入 `#[cfg(unix)]` 块 | Windows `cargo check` 0 warning（仅允许现存 Unix warning） |
| U0.3 | `.gitattributes` | `* text=auto eol=lf` + binary 清单 | 全新 clone `git status` 干净 |
| U0.4 | CI | `windows-latest` job：check + clippy | CI 绿，Linux job 不变 |

### Phase 1 — 单 loop 可用

| Unit | Scope | 关键交付 | 验收 |
|---|---|---|---|
| U1.1 | `ralph-core::shell` 模块 | 解析链 + `core.shell`/`RALPH_SHELL` + 排除 WSL bash | 单测覆盖每条链分支 |
| U1.2 | preflight/workspace/pty_kernel/web/attach/restart 全部走 shell 抽象 | 共享代码 Unix 逐字不变 | Windows `ralph preflight` 通过；Unix 基线绿 |
| U1.3 | backend `which` + stdin prompt | auto_detect、cli_executor、pty_executor | `ralph run --backend claude` 能在 Windows 找到 npm 安装的 claude |
| U1.4 | hooks 解释器映射 + PATHEXT | `.ps1`/`.sh`/`.py`/`.cmd` | `hooks-bdd` 相关 case 在 Windows 通过 |
| U1.5 | HOME 回退 + BOM/UTF-16 容错 | profiles + events reader | 无 HOME 环境下 `ralph` 不报 `HomeNotSet` |

### Phase 2 — 正确性原语

| Unit | Scope | 关键交付 | 验收 |
|---|---|---|---|
| U2.1 | 文件锁 sidecar 方案 | std `File::lock*` on `*.guard` | `ralph run` 双开在 Windows 报 lock-held；`ralph loops` 不报错 |
| U2.2 | `probe_pid` + ralph-api | sysinfo start_time 比对 | `ralph loops` 显示死 PID 为 stopped |
| U2.3 | Job Object + kill_process_tree | win32job/command-group，仅 Windows | 杀掉 ralph web 后端口释放、无 node 孤儿 |
| U2.4 | Windows 信号映射 | tokio::signal::windows | Ctrl+Break/关窗 触发优雅停机；`stop --force` 可用 |

### Phase 3 — 并行 / worktree / DAG

| Unit | Scope | 关键交付 | 验收 |
|---|---|---|---|
| U3.1 | worktree symlink/junction/hardlink | 失败时显式拒绝 | `parallel` 双 loop 交付物回主 repo |
| U3.2 | 路径安全 components + dunce | 6 处字符串判断改写 | `..\`/`C:\..`/大小写绕过全部失败 |
| U3.3 | pty_kernel stdin + `/proc` 退化 | 去 sh 包装 | DAG job stdin prompt 可达 |
| U3.4 | env 白名单 + baseline | 大小写不敏感 | `clear_env` 下 child 能起 node/cmd |
| U3.5 | restart / attach / web spawn | DETACHED 重启 | `ralph run --restart-on-…` 在 Windows 工作 |

### Phase 4 — 体验与发布

| Unit | Scope | 关键交付 | 验收 |
|---|---|---|---|
| U4.1 | pwsh 补全 + `--payload-file/-` | clap_complete + emit | `ralph completions powershell` 输出脚本 |
| U4.2 | `RALPH_HOST_OS` 注入 + docs pwsh 写法 | 嵌入工具文档更新 | preset 中 bash-only 段标注 |
| U4.3 | `cargo xtask test` | 两阶段 nextest + env scrub | `./scripts/run-tests.sh` 与 `cargo xtask test` 等价 |
| U4.4 | dist msvc target + PS installer | release.yml | tag 产出 `x86_64-pc-windows-msvc` 包 |

---

## Evidence Anchors（当前代码定位）

- 编译失败：`crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:573`、`spawn.rs:591`（nix 裸调用）
- 锁 stub：`ralph-core/src/{file_lock,loop_lock,loop_registry,merge_queue,state/idempotent_log}.rs`
- PID stub：`loop_registry.rs:199`、`merge_queue.rs:608`、`run_manifest.rs:355`、`ralph-cli/src/loops.rs:266`
- 进程组 no-op：`ralph-cli/src/cli/process_tree.rs:77`、`process_management.rs:87`
- shell 写死：`preflight.rs:225`、`workspace.rs:228/370`、`pty_kernel.rs:186-199`、`run.rs:1988`、`loops.rs:1160`、`web.rs:1182/1210`、`loops.rs:1980/2234`、`api/loop_domain.rs:312`、`api/loop_support.rs:55`
- HOME：`profiles.rs:306`（回退链见 `config_resolution.rs:298`）
- 路径字符串：`workspace_mutation_guard.rs:126/135`、`ephemeral_isolation.rs:392`、`parallel_forge_handoff.rs:150/833`、`acceptance_and_lifecycle.rs:1004`、`inspect/descriptor.rs:139`
- env 白名单：`runtime_job/environment.rs`、`dag_scheduler/spawn.rs:97`
- 权限/symlink：`hooks/executor.rs:629/636/664`、`ralph-cli/src/hooks.rs:332/339`、`preflight.rs:547/554`、`loop_context.rs:470/510/548/650`、`worktree.rs:1081/1737/2087`、`target_checkout.rs:302`
- 约束：`Cargo.toml:27`（`unsafe_code = "forbid"`）、`Cargo.toml:166`（dist targets）

## Risks

- **npm `.cmd` shim + 含换行参数**：std `Command` 直接拒绝 → Phase 1 强制 stdin/file prompt。
- **Windows mandatory lock**：同文件第二句柄 `ERROR_LOCK_VIOLATION` → sidecar `*.guard` 方案。
- **ConPTY VT/CRLF 差异**：`claude_stream`/`pi_stream` 解析器需 `\r` 容忍；Phase 1 加入解析 fuzz case。
- **Git Bash 缺席机器**：pwsh fallback 可用，但 preset 内 bash 语法需 agent 自行翻译 → `RALPH_HOST_OS` 注入缓解。
- **PID 复用**：`sysinfo` 比对 `start_time`，仍留 `Unknown` 分支给上层判 stale。
