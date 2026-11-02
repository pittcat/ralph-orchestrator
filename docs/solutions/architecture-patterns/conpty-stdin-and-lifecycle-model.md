---
title: Windows ConPTY stdin 投递与生命周期模型（实测结论，非推测）
date: 2026-11-04
category: docs/solutions/architecture-patterns
module: ralph-adapters, ralph-cli
problem_type: platform_semantics
component: pty_executor, pty_kernel
symptoms:
  - "PTY 子进程 spawn 即崩 0xC0000142"
  - "stdin-prompt 后端在 Windows PTY 下 30s 超时挂死"
  - "发送 `\\x04`/`\\x1a` 后子进程显示字面字符但不退出"
  - "MSYS cat 收到 `^Z` 后连已投递的输入都丢失"
root_cause: platform_semantics
resolution_type: code_fix
severity: high
tags:
  - conpty
  - windows
  - stdin-eof
  - portable-pty
  - process-lifecycle
---

# Windows ConPTY stdin 投递与生命周期模型

`portable-pty` 的 ConPTY 后端与 Unix PTY 语义差异巨大，以下为逐条实测结论（`conpty_eof_probe` 参数化探针打矩阵得出），不是文档推测。

## 三条硬事实

1. **master 不可 drop**。`PtyPair.master` 持有 `PsuedoCon`(HPCON)，一旦 drop → `ClosePseudoConsole` → 子进程初始化失败 `0xC0000142`（STATUS_DLL_INIT_FAILED）。Unix 的 fd dup 语义不存在——HPCON 不可 dup。`PtyJobHandle`/`PtyExecutor` 必须持有 master 直到 job 结束。

2. **输入管道 write-end 是唯一句柄**。`take_writer()` 返回 `stdin.write` 的**唯一**句柄（不是克隆）。drop 它 = 关输入管道 = **杀掉整个伪控制台会话**（同样 `0xC0000142`）。writer 必须随 job 存活。

3. **ConPTY 没有 stdin-EOF 通道**。子进程 stdin 是控制台输入句柄，输入按原始击键流投递（raw input mode）：`^Z`(0x1A)、`^D`(0x04）都只是字面字符，不产生 EOF。关掉写入管道只会杀会话，不会给子进程 EOF。

## 有效投递序列（实测）

```
prompt_bytes + \r + \x04
```

- **`\r`** = Enter 击键。`\n` 不行——`\n` 到达子进程是 `^J` 字面字符，不能完成 cooked-mode 行读（`cmd set /p` 实测挂死）；cygwin/MSYS 走 ICRNL 两种都吃。
- **`\x04`** = cygwin/MSYS 自己实现的 VEOF(console 层 POSIX 语义）。MSYS cat 收到后干净退出 code=0。**仅对 cygwin 有效**，对原生程序是字面字符（但原生程序只需读 prompt 行，不需要 EOF)。
- **绝不发 `\x1a`**：对原生程序是字面字符（raw mode 无 cooked ^Z-EOF)；对 cygwin 是 SIGTSTP——实测 `cat` 收到 `^Z` 后**已投递的输入队列被冲刷**，连 marker 行都没读到就挂死。

## 退出检测不能依赖输出管道 EOF

第三方 ConPTY host（如 tty7 的 `OpenConsole.exe`）在 client 退出后**不退出**，输出管道永不 EOF → `reader.read()` 永远阻塞。两条防线：

1. `run_observe`/`run_observe_streaming`/`run_interactive`/`pty_kernel` 全部加了 **pid watcher**(`child_exit_watcher`/`exit_rx`,~100ms 轮询 `pid_alive`)——子进程死即唤醒 select 走 `try_wait`。Unix 下是 `pending` future 零开销。
2. job 结束时显式 drop `_master`(HPCON)→ host 被杀 → 输出管道 EOF → reader 线程退出 → `join` 不挂死。

## 环境依赖注意

- `portable-pty` 的 `load_conpty` 会**优先侧载 PATH 上的 `conpty.dll`**（如 tty7)。本机 tty7 的 OpenConsole 行为已被实测（不随 client 退出），代码对其兼容。
- MSYS2/Git Bash 参数改写：`{` 开头参数、`/c` 等经 CreateProcess→MSYS2 crt 会被吃掉或转路径。测试探针从 Git Bash 传 `/c` 到 cmd 会被改写成 `C:\` ——这是 shell 层行为不是产品 bug。

## 覆盖测试

- `pty_executor::tests::test_run_observe_stdin_eof_windows` — MSYS cat + `\r`+`\x04` → 干净 EOF 退出
- `pty_executor::tests::test_run_observe_stdin_native_line_reader_windows` — `cmd /v:on /c set /p` 行读（原生程序无需 EOF)
- `pty_kernel::tests::pty_job_completes_and_delivers_stdin_on_windows` — spawn + stdin 投递 + exit 检测全链路
- 全量：`pty_executor` 的 `run`/`run_observe`/`run_observe_streaming`/`run_interactive` 四处 stdin 写入点均为 `cfg` 分流（Unix `\n`+即丢，Windows `\r`+`\x04`+持有）
