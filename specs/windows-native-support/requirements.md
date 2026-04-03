# 功能概述

将 Ralph 核心 CLI 适配为原生 Windows + PowerShell 可用，并补齐 Windows CI 与二进制发布能力。

# 范围内（In Scope）

- `ralph-cli`、`ralph-core`、`ralph-adapters` 在 `x86_64-pc-windows-msvc` 可编译、可测试
- `ralph run` 支持 primary loop、parallel worktree loop、共享 memories/specs/tasks
- `ralph loops list/stop --force` 具备可验证的进程探测与清理行为
- PTY / ACP / CLI 三类 backend 执行层在 Windows 不挂死、无子进程泄漏
- GitHub Actions `windows-latest` 验收门禁
- `x86_64-pc-windows-msvc` 二进制发布目标
- PowerShell smoke 验证脚本（macOS + Windows 共用）
- 更新 README / FAQ / Troubleshooting 明确支持边界

# 范围外（Out of Scope）

- `ralph web` Windows 原生启动支持（显式返回 unsupported）
- `aarch64-pc-windows-msvc`
- MSI / winget / scoop 安装包
- 全量 Bash 脚本迁移为 PowerShell

# 功能性需求（Functional Requirements）

1. Rust 核心模块可在 Windows 编译
2. 单 loop 与并行 worktree loop 可运行
3. 共享 memories/specs/tasks 在 Windows via hard link + junction 生效
4. PTY / ACP / CLI backend 无挂死、无残留子进程
5. Windows 进程存活检查与树状终止可靠
6. `ralph web` 在 Windows 返回明确 unsupported 错误
7. PowerShell completion 生成继续正常工作

# 非功能性需求（Non-Functional Requirements）

- 性能：backend 缺失时快速失败，不允许 PTY/ACP 挂死
- 兼容性：不允许破坏现有 Unix 行为；macOS/Linux 主线测试必须继续通过
- 跨平台：锁、进程控制、共享状态链接必须统一抽象，不允许散落平台特化代码
- 验收：Windows CI 必须绿色才能合并

# 验收标准（Acceptance Criteria）

- [ ] `cargo check --workspace --target x86_64-pc-windows-msvc` 退出码为 0
- [ ] `cargo test -p ralph-cli --test integration_windows_loops` 全部通过
- [ ] `cargo test -p ralph-core --test platform_cross_platform` 全部通过
- [ ] `cargo test -p ralph-adapters --test windows_backend_cleanup` 全部通过
- [ ] GitHub Actions `windows-latest` job 绿色且为必经门禁
- [ ] `cargo dist build --target x86_64-pc-windows-msvc --artifacts local` 生成 Windows 可执行发布物
- [ ] README/FAQ/Troubleshooting 不再将核心 CLI 描述为 WSL-only，且明确 `web` 不支持
- [ ] `cargo test` 和 `cargo test -p ralph-core smoke_runner` 继续通过
