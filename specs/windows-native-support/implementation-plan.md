# 实施计划

## 阶段 1：准备

- [ ] 建立 `.ralph/specs/windows-native-support/` 规格骨架（requirements.md, design.md, implementation-plan.md）
- [ ] 创建 `scripts/windows-smoke.ps1` 覆盖 compile / loop / cleanup / docs 四类检查
- [ ] 验证 smoke 脚本可被 `pwsh` 执行且退出码与失败状态一致

## 阶段 2：核心实现

- [ ] 实现 `crates/ralph-core/src/platform/mod.rs` 平台抽象入口
- [ ] 实现 `crates/ralph-core/src/platform/locks.rs` 跨平台文件锁（fs4）
- [ ] 重构 `file_lock.rs`、`loop_lock.rs`、`loop_registry.rs`、`merge_queue.rs` 使用统一锁抽象
- [ ] 验证：`cargo test -p ralph-core --test platform_cross_platform` 通过
- [ ] 验证：`cargo check -p ralph-core --target x86_64-pc-windows-msvc` 通过

- [ ] 实现 `crates/ralph-core/src/platform/process.rs` 跨平台进程探测与清理
- [ ] 重构 `loop_registry.rs`、`loops.rs`、`loop_runner.rs` 使用统一进程控制
- [ ] 验证：`cargo test -p ralph-core --test platform_cross_platform process_control` 通过
- [ ] 验证：`cargo test -p ralph-cli --test integration_windows_loops stop_and_orphan_cleanup` 通过

- [ ] 实现 `crates/ralph-core/src/platform/fs_links.rs` 跨平台共享状态链接
- [ ] 重构 `loop_context.rs`、`worktree.rs` 使用 hard link + junction
- [ ] 验证：`cargo test -p ralph-core --test platform_cross_platform worktree_link_strategy` 通过
- [ ] 验证：Unix symlink 行为不回归

- [ ] 重构 `crates/ralph-adapters` 收口 nix 依赖到 Unix 门
- [ ] 统一 PTY / ACP / CLI backend 清理路径
- [ ] 验证：`cargo check -p ralph-adapters --target x86_64-pc-windows-msvc` 通过
- [ ] 验证：`cargo test -p ralph-adapters --test windows_backend_cleanup` 通过

- [ ] 适配 `ralph-cli` 主路径（main.rs、loops.rs、loop_runner.rs）
- [ ] `ralph web` Windows 显式 unsupported
- [ ] 验证：`cargo test -p ralph-cli --test integration_windows_loops run_list_stop` 通过
- [ ] 验证：`cargo test -p ralph-cli --test integration_windows_loops web_unsupported_on_windows` 通过
- [ ] 验证：PowerShell completion 正常生成

## 阶段 3：验证与测试

- [ ] 补齐 `crates/ralph-core/tests/platform_cross_platform.rs`
- [ ] 补齐 `crates/ralph-cli/tests/integration_windows_loops.rs`
- [ ] 补齐 `crates/ralph-adapters/tests/windows_backend_cleanup.rs`
- [ ] 重构现有 Unix-only 测试拆平台门，不删除断言
- [ ] 验证：三个 Windows 专项测试全部通过
- [ ] 验证：`cargo test -p ralph-core smoke_runner` 继续通过

- [ ] 在 `.github/workflows/ci.yml` 增加 `windows-latest` job
- [ ] 验证：CI 文件包含 Windows job，执行 smoke 与专项测试

- [ ] 将 `x86_64-pc-windows-msvc` 纳入 cargo-dist 发布矩阵
- [ ] 验证：`cargo dist build --target x86_64-pc-windows-msvc --artifacts local` 生成 Windows 发布物

## 阶段 4：完成条件

- [ ] `cargo test` 全部通过
- [ ] `cargo test -p ralph-core smoke_runner` 通过
- [ ] `cargo check --workspace --target x86_64-pc-windows-msvc` 退出码为 0
- [ ] `scripts/windows-smoke.ps1 -Mode Full` 全部检查通过
- [ ] GitHub Actions `windows-latest` job 绿色
- [ ] Windows 发布物可执行最小 smoke
- [ ] README / FAQ / Troubleshooting 与支持边界一致，不再描述为核心 CLI WSL-only
- [ ] `ralph web` 在 Windows 返回明确 unsupported
- [ ] 所有 Task 状态同步为 [DONE]
