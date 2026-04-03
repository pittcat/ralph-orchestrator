# 设计概述

引入平台抽象层（PAL），统一文件锁、进程控制、共享状态链接策略。Windows 上使用 fs4 统一锁、sysinfo 做进程探测、hard link + junction 做 worktree 共享状态。

# 现有系统上下文

- 模块：`ralph-core`、`ralph-cli`、`ralph-adapters`
- 现有平台假设：`flock()`、`nix::`、symlink、`sh`、`/tmp`、SIGTERM/killpg
- 依赖：`fs4`（新增）、`sysinfo`（新增）
- 构建目标：`x86_64-pc-windows-msvc`（新增）
- CI：GitHub Actions `windows-latest`（新增）
- 验证环境：macOS 已安装 PowerShell 7

# 方案设计

1. **平台抽象层**：`crates/ralph-core/src/platform/` 下三个子模块
   - `locks.rs`：统一文件锁（fs4），覆盖 FileLock/LoopLock/LoopRegistry/MergeQueue
   - `process.rs`：统一进程存活探测（sysinfo）+ 树状终止（Unix: signal/killpg, Windows: taskkill /T /F）
   - `fs_links.rs`：统一共享状态链接（Unix: symlink, Windows: hard link for memories + junction for specs/tasks）

2. **Backend 清理路径**：收口 `nix` 依赖到 Unix 门，Windows 统一快速失败

3. **web 显式 unsupported**：Windows 调用 `ralph web` 返回固定错误消息与非 0 退出码

4. **macOS + PowerShell 验证**：可用作 shell 层和命令契约预检，正式验收以 Windows CI 为准

# 数据流 / 控制流

1. 用户在 PowerShell 执行 `ralph run`
2. `ralph-cli` 调用跨平台锁抽象获取 primary loop lock
3. 若 lock 空闲则作为 primary loop；若被占用且 `features.parallel = true` 则创建 worktree
4. 平台链接抽象负责共享 `.ralph/agent/memories.md`、`.ralph/specs/`、`.ralph/tasks/`
5. backend 执行进入 PTY / CLI / ACP 执行层之一
6. 停止/超时/清理统一经过平台进程控制层

# 文件 / 模块改动

| 类型 | 路径 | 说明 |
|------|------|------|
| 新增 | `crates/ralph-core/src/platform/mod.rs` | 平台抽象入口 |
| 新增 | `crates/ralph-core/src/platform/locks.rs` | 跨平台文件锁（fs4） |
| 新增 | `crates/ralph-core/src/platform/process.rs` | 进程探测与树状终止 |
| 新增 | `crates/ralph-core/src/platform/fs_links.rs` | 跨平台共享状态链接 |
| 新增 | `crates/ralph-core/tests/platform_cross_platform.rs` | 平台层回归测试 |
| 新增 | `crates/ralph-cli/tests/integration_windows_loops.rs` | Windows loop 集成测试 |
| 新增 | `crates/ralph-adapters/tests/windows_backend_cleanup.rs` | Windows backend 清理测试 |
| 新增 | `scripts/windows-smoke.ps1` | PowerShell smoke 脚本 |
| 修改 | `Cargo.toml` | 新增 Windows target 与依赖 |
| 修改 | `.github/workflows/ci.yml` | 新增 windows-latest job |
| 修改 | `.github/workflows/release.yml` | Windows 发布矩阵 |
| 修改 | `crates/ralph-core/src/file_lock.rs` | 切换到 fs4 |
| 修改 | `crates/ralph-core/src/loop_lock.rs` | Windows loop lock 支持 |
| 修改 | `crates/ralph-core/src/loop_registry.rs` | Windows PID 存活检查 |
| 修改 | `crates/ralph-core/src/merge_queue.rs` | Windows 文件锁 |
| 修改 | `crates/ralph-core/src/loop_context.rs` | Windows 共享状态链接 |
| 修改 | `crates/ralph-core/src/worktree.rs` | Windows worktree 策略 |
| 修改 | `crates/ralph-core/src/lib.rs` | 导出平台模块 |
| 修改 | `crates/ralph-cli/src/main.rs` | loop lock / worktree 入口适配 |
| 修改 | `crates/ralph-cli/src/loops.rs` | list/stop Windows 支持 |
| 修改 | `crates/ralph-cli/src/loop_runner.rs` | interrupt / cleanup 适配 |
| 修改 | `crates/ralph-cli/src/web.rs` | Windows 显式 unsupported |
| 修改 | `crates/ralph-adapters/Cargo.toml` | nix 平台门与依赖整理 |
| 修改 | `crates/ralph-adapters/src/cli_executor.rs` | Windows 终止路径 |
| 修改 | `crates/ralph-adapters/src/pty_executor.rs` | Windows PTY 快速失败 |
| 修改 | `crates/ralph-adapters/src/acp_executor.rs` | Windows ACP 子进程清理 |
| 修改 | `README.md` | Windows / PowerShell 支持说明 |
| 修改 | `docs/reference/faq.md` | 去除 WSL-only 口径 |
| 修改 | `docs/reference/troubleshooting.md` | 增加 Windows 故障排查 |

# 边界情况

- backend 不在 PATH：快速失败，不挂死
- `.worktrees` 与主仓库不在同卷：共享状态链接失败时立即报错，不静默 copy
- worktree 已被外部删除：registry 保留 orphan，`loops stop` 支持清理
- `taskkill` 执行失败：记录并标记 `[BLOCKED]`，禁止继续推进
- 旧 Unix-only 测试不适用于 Windows：拆为跨平台测试与 Unix 专项，不删除断言
- macOS + pwsh：可用于 shell 层和命令契约预检，正式验收以 Windows CI 为准

# 风险与权衡

- hard link / junction 跨卷失败 → 同卷假设，写入 DocDefaults
- PTY Windows 行为与 Unix 不一致 → 快速失败优先，不强求行为一致
- `taskkill` 树状终止语义与 Unix 信号不同 → 优先保证可靠清理

# 测试策略

- 新增：`platform_cross_platform.rs`、`integration_windows_loops.rs`、`windows_backend_cleanup.rs`
- 修改：现有 lock/worktree/backend 测试拆平台门
- 保持：`cargo test`、`cargo test -p ralph-core smoke_runner` 继续通过
- lint：`cargo fmt --check`、`cargo clippy --workspace -- -D warnings`
