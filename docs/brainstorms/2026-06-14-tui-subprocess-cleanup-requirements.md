---
date: 2026-06-14
topic: tui-subprocess-cleanup
---

# `ralph run` TUI 退出后终端恢复与进程清理

## Problem Frame

运行 `ralph run`（默认启用 TUI）或 `ralph run --worktree` 时，TUI 看起来已经正常退出、没有报错，但 shell 没有回来，终端像处于僵尸状态，必须按 `Ctrl+C` 才能恢复。

代码层面，`ralph run` 默认走 subprocess TUI 模式：`crates/ralph-cli/src/commands/run.rs` 的 `run_subprocess_tui` 先启动一个 `ralph run --rpc` 子进程，再由父进程里的 TUI 读取子进程输出。当前实现存在两个缺口：

1. **清理路径可能卡住**：子进程退出后，父进程等待 `forward_handle` / `reader_handle` 两个 I/O task 结束；如果子进程 stdout pipe 没正确关闭，或 task 卡在 `write_all`/`flush`，父进程就回不去 shell。
2. **终端恢复只有局部分布式守卫**：TUI 内部有 `defer!` guard 恢复 raw mode / alternate screen，panic hook 也做了恢复，但没有覆盖"父进程被信号终止"或"父进程在 TUI teardown 之后卡住"的全局安全网。

因此需要：
- 根因修复：让 `run_subprocess_tui` 在任何情况下都能快速、可靠地清理子进程和 I/O task。
- 全局安全网：给 `ralph run` 进程安装一个进程级的 terminal-state-restore 机制，即使父进程异常退出也能把终端恢复正常。

## Actors

- A1. **用户 / 开发者**：运行 `ralph run` 并期望 TUI 退出后能立即回到可用 shell。
- A2. **Ralph CLI（父进程 / TUI 进程）**：spawn 子进程、渲染 TUI、负责最终清理。
- A3. **Ralph Loop 子进程（`--rpc`）**：实际运行 orchestration loop。

## Key Flows

### F1. 正常完成退出

- **Trigger**：子进程（loop）正常结束，`child.wait()` 返回 success。
- **Actors**：A2, A3
- **Steps**：
  1. 父进程检测到子进程退出。
  2. 发送取消信号给 `forward_handle` 和 `reader_handle`。
  3. 在有限时间内（如 2-5 秒）等待 task 结束；超时则主动 abort。
  4. 关闭 stdin/stdout pipe，确保子进程不会因为 pipe 未关闭而僵尸。
  5. 父进程返回 `Ok(())`，shell 恢复。
- **Outcome**：用户立即回到可用 shell。
- **Covered by**：R1, R2, R3, R4, R5

### F2. 用户按 `q` 或 TUI 主动退出

- **Trigger**：用户在 TUI 里按 `q` 退出，`app.run()` 先返回。
- **Actors**：A2, A3
- **Steps**：
  1. TUI 正常 teardown，`defer!` guard 恢复终端状态。
  2. 父进程等待子进程退出（可设超时）。
  3. 若子进程未在超时内退出，父进程发送 SIGTERM / SIGKILL 强制终止。
  4. 清理 I/O task 并返回。
- **Outcome**：TUI 退出后子进程不会继续占用终端。
- **Covered by**：R1, R3, R6, R7

### F3. 父进程收到 SIGINT / 被外部终止

- **Trigger**：用户按 `Ctrl+C`，或 OS/terminal 向父进程发送信号。
- **Actors**：A2, A3
- **Steps**：
  1. 全局 signal handler 捕获 SIGINT/SIGTERM。
  2. 立即恢复终端状态（disable raw mode、leave alternate screen、show cursor）。
  3. 转发终止信号给子进程。
  4. 等待子进程退出后父进程退出。
- **Outcome**：终端不会留在 broken 状态。
- **Covered by**：R8, R9, R10

### F4. 子进程异常退出但 pipe 未关闭

- **Trigger**：子进程崩溃或变成僵尸，stdout pipe 没有 EOF。
- **Actors**：A2, A3
- **Steps**：
  1. `child.wait()` 已经返回，但 `forward_handle` 仍卡在 `next_line()` 或 `write_all`。
  2. 父进程在 select 分支结束后主动 `drop(child.stdout)` / `drop(child.stdin)`。
  3. 使用 `tokio::time::timeout` 等待 task，超时 abort。
- **Outcome**：父进程不被挂起的 pipe 阻塞。
- **Covered by**：R4, R5, R11

## Requirements

### 根因修复层（R1-R7）

- **R1. 子进程退出后必须显式关闭 pipe**：`run_subprocess_tui` 在 `child.wait()` 返回后，必须 `drop(child.stdin)` 和 `drop(child.stdout)`，确保 I/O task 的 `next_line()` / `write_all` 能立即收到错误/EOF。
- **R2. 等待 I/O task 必须加 timeout**：`reader_handle.await` 和 `forward_handle.await` 都要有超时（建议 2-5 秒），超时后记录 warning 并 abort task，不得无限等待。
- **R3. TUI 主动退出时强制终止子进程**：当 `app.run()` 先返回（用户按 `q`），如果子进程未在合理时间（如 5 秒）内退出，父进程必须发送 SIGTERM，必要时再 SIGKILL。
- **R4. 使用更可靠的取消机制**：优先使用 `tokio_util::sync::CancellationToken` 替代 `watch::channel` 来取消 `forward_handle` / `reader_handle`，避免 `watch::Sender` 已经被 drop 后无法取消的问题。
- **R5. 避免 forward task 在 writer 关闭后无限 flush**：`forward_handle` 在 `write_all`/`flush` 失败时应立即 break，而不是重试。
- **R6. 子进程启动失败时也要恢复终端**：如果 `Command::spawn` 失败或子进程启动瞬间退出，`run_subprocess_tui` 返回前必须保证 TUI 的 terminal restore guard 已经执行。
- **R7. 保持现有子进程 stderr log 行为**：stderr 继续重定向到 `.ralph/diagnostics/logs/` 下的日志文件，便于排查清理失败原因。

### 全局安全网层（R8-R10）

- **R8. 安装进程级 terminal restore guard**：在 `ralph run` 的顶层（`main` 或 `run_command` 入口）安装一个 `scopeguard` / RAII guard，在进程任何退出路径（正常、panic、被信号终止）都执行 `disable_raw_mode()` + `LeaveAlternateScreen` + `Show cursor`。
- **R9. 注册 SIGINT/SIGTERM handler**：使用 `tokio::signal`（或 `ctrlc` crate）在父进程收到 SIGINT/SIGTERM 时：先恢复终端，再转发信号给子进程，最后退出。
- **R10. 安全网必须幂等且兼容非 TUI 模式**：terminal restore guard 在 `--no-tui` / `--autonomous` / `--rpc` 模式下应是 no-op，不能破坏这些模式的输出。

### 可观测层（R11-R12）

- **R11. 清理阶段写入结构化日志**：在 `run_subprocess_tui` 进入/退出清理阶段时记录 `tracing` event，包含 child exit status、各 task 是否超时、是否发送了 kill signal。
- **R12. 保留失败现场的诊断信息**：如果清理超时或被强制 abort，把相关状态（如 task 名称、等待时长）写入 `.ralph/diagnostics/` 的 JSONL 或 log，便于后续定位。

## Non-goals

- NG1. 不改动 legacy TUI（`--legacy-tui`）和纯 RPC/Autonomous 模式的正常退出路径。
- NG2. 不引入新的用户配置项；所有清理行为都是默认、不可关闭的内部改进。
- NG3. 不解决 backend 子进程（Claude CLI 等）自身挂住的问题；本需求只覆盖 Ralph subprocess TUI 父进程与子进程之间的清理契约。
- NG4. 不改变 `ralph run` 的退出码语义。

## Success Criteria

- SC1. 连续 10 次 `ralph run -H builtin:ce-executor-isolated`（或任意预设）在正常完成后，shell 立即可用，无需按 `Ctrl+C`。
- SC2. 连续 10 次 `ralph run --worktree` 在正常完成后，shell 立即可用。
- SC3. 用户在 TUI 里按 `q` 退出后，子进程在 5 秒内被终止，父进程返回 shell。
- SC4. TUI 运行期间按 `Ctrl+C`，终端立即恢复正常（能看到 shell prompt），子进程被终止。
- SC5. 现有 `cargo nextest run -p ralph-cli --bin ralph` 测试全部通过。
- SC6. 新增至少一个集成/单元测试覆盖"子进程已退出但 I/O task 卡住"的清理路径。
