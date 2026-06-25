---
title: reqwest 默认从 HTTP_PROXY 走代理导致 127.0.0.1 集成测试 connection closed
date: 2026-06-09
category: docs/solutions/test-failures
module: ralph-api
problem_type: test_failure
component: testing_framework
tags: [test, http, proxy, loopback, ci]
symptoms:
  - "reqwest::Client::new() in tests reports: error sending request for url (http://127.0.0.1:<port>/rpc/v1) Caused by: client error (SendRequest): connection closed before message completed"
  - 所有绑定 127.0.0.1:0 的集成测试套件 100% 失败（rpc_v1_bootstrap / rpc_v1_streaming / rpc_v1_planning_config_preset_collection 等）
  - 报错端口每次随机（41289、43217、39289…），看似环境抖动，但实际是同一原因
  - 同一 server 用裸 TCP 直接读写 HTTP/1.1 完全正常（200 + 完整 JSON）
  - 加 NO_PROXY=127.0.0.1,localhost,::1 后全部通过
root_cause: incomplete_setup
resolution_type: config_change
severity: high
tags:
  - reqwest
  - http-proxy
  - no-proxy
  - loopback
  - cargo-config
  - integration-tests
  - ralph-api
related_components:
  - ralph-cli
  - ralph-tui
---

# reqwest 默认从 HTTP_PROXY 走代理导致 127.0.0.1 集成测试 connection closed

## Problem

`ralph-api` 的 16 个 RPC v1 集成测试在用户的开发环境中**全部失败**，错误统一为
`connection closed before message completed`，误诊方向是 RPC server 协议层 bug。
实际根因是 `reqwest::Client::new()` 默认从 shell 读取 `HTTP_PROXY` / `HTTPS_PROXY` 环境变量
把 `127.0.0.1` 流量路由到远程代理（用户环境 `192.168.3.231:808`），代理跨网段无法回连本机
`TcpListener::bind("127.0.0.1:0")` 后直接关闭连接。任何绑定 loopback 端口的 reqwest 集成测试
在没有 `NO_PROXY` 排除的环境下都会撞上这个问题。

## Symptoms

- 测试日志统一报 `error sending request for url (http://127.0.0.1:<port>/rpc/v1) ... connection closed before message completed`
- 失败端口每次随机（`127.0.0.1:41289`、`127.0.0.1:43217`…），表面像环境抖动或竞态
- 同一 server 用裸 `tokio::net::TcpStream` 走 HTTP/1.1 直连返回 200 + 完整 JSON
- 加 `NO_PROXY=127.0.0.1,localhost,::1` 后 16 个失败测试全部通过
- 仓库里**非 reqwest** 的测试（`mcp_server`、单测、scenarios）完全不受影响

## What Didn't Work

- 怀疑 axum 路由 / 端口复用：在 `serve_with_listener` 加 trace 看到 axum 正常接收并处理请求，
  reqwest client 才报错 → 服务端没问题
- 怀疑 hyper / body 解析：换 `axum::body::Bytes` 大小限制 / 加 `Content-Length` 都无效
- 怀疑 tokio runtime：`#[tokio::test]` 默认 multi-thread，与 `#[tokio::main]` 行为差异不解释
  "按 100% 概率 + 仅 reqwest 失败"这个特征
- 关闭 RUST_LOG=trace 没看到任何 server log → 不是 server 侧静默错误，是 client 没收到响应

## Solution

新增 `.cargo/config.toml` 强制给所有 cargo 子进程注入 `NO_PROXY`：

```toml
# .cargo/config.toml
[env]
NO_PROXY = { value = "127.0.0.1,localhost,::1", force = true }
```

`force = true` 关键：cargo 默认不会覆盖 shell 已有的 `NO_PROXY`，加 `force` 才能保证
任何 shell 配置（含未设 `NO_PROXY` 的 CI 镜像）下都生效。shell 里那俩 `HTTP_PROXY` /
`HTTPS_PROXY` 不动，只在 cargo 启动的子进程里把 loopback 拉出代理白名单。

验证（`env -u NO_PROXY cargo test -p ralph-api`）：

| 套件 | 修复前 | 修复后 |
|------|-------|-------|
| `tests/rpc_v1_bootstrap.rs` | 0/4 | 4/4 |
| `tests/rpc_v1_streaming.rs` | 0/6 | 6/6 |
| `tests/rpc_v1_planning_config_preset_collection.rs` | 0/6 | 6/6 |
| `tests/rpc_v1_task_loop.rs` | n/a | 5/5 |
| `tests/rpc_v1_uncovered.rs` | n/a | 6/6 |
| `tests/mcp_server.rs` | 5/5 | 5/5 |
| 单测 + doc | 40/40 | 40/40 |

**备选方案**（按改动面排序）：

| 方案 | 改动 | 永久性 | CI 友好 |
|------|------|-------|--------|
| A. 改测试用 `Client::builder().no_proxy().build()` | ~10 行，5 个测试文件 | 永久 | 是 |
| B. `.cargo/config.toml` 注入 `NO_PROXY`（本次采用） | 几行，单一 config 文件 | 永久 | 是 |
| C. shell 里 `export NO_PROXY=…` | 一行 | 半永久 | 否 |

方案 A 看起来更"防御"——让测试 client 与环境解耦——但要求所有 `tests/rpc_v1_*.rs` 改完。
方案 B 改一次走人，覆盖整个 workspace 的 cargo 子进程。两者可叠加（先 B 跑通，A 作 belt-and-suspenders）。

## Why This Works

`reqwest 0.12` 的 `Client::new()` 默认启用 `Env` proxy feature：从 `HTTP_PROXY` / `HTTPS_PROXY` /
`ALL_PROXY` 读代理配置，从 `NO_PROXY` 读排除名单。loopback 主机不会自动豁免——必须显式列在
`NO_PROXY` 里。我们的测试用 `TcpListener::bind("127.0.0.1:0")` 监听 loopback，但
`reqwest::Client::new()` 把这个 URL 路由到环境里的 `HTTP_PROXY=http://192.168.3.231:808`，
远程代理收到 `CONNECT 127.0.0.1:<port>` 后无法跨网段回连，直接 FIN/RST，client 看到
"connection closed before message completed"。

`[env]` 是 cargo 1.49+ 引入的 stable 配置，给所有 cargo spawn 的子进程（包括 test 二进制）
注入/覆盖环境变量。`force = true` 跳过"是否已存在"检查，强制把 `NO_PROXY` 写到子进程
环境中。shell 那俩 `HTTP_PROXY` 保持不变，cargo 之外启动的程序（如编辑器、其他 CLI）行为
完全不受影响。

## Prevention

- **新成员踩坑**：`docs/solutions/` 里挂本文件，agent 在 `crates/*/tests/*.rs` 写新集成
  测试前应先搜 `reqwest` / `proxy` 关键字
- **CI 镜像**：镜像构建脚本（如有）应显式 `export NO_PROXY=127.0.0.1,localhost,::1`，
  避免 runner shell 替换导致 cargo `[env]` 被某层环境重置
- **测试 client 规范**：所有 `reqwest::Client::new()` 在 `tests/` 目录下应优先
  `Client::builder().no_proxy().build()`，让测试与 shell 环境彻底解耦（与本方案叠加，
  作 belt-and-suspenders）
- **可疑症状 checklist**：看到 "connection closed before message completed" + 端口
  在 127.0.0.1 + 报错统一在 reqwest 侧 → 第一反应是检查 `env | grep -i proxy`，不要
  再去翻 axum / hyper / tokio runtime
- **repro 留档**：当时加的 `crates/ralph-api/examples/repro.rs`（裸 TCP 走 HTTP/1.1）可
  作为后续排查工具——它在带代理的环境下也能跑通，能立刻区分"server 死了"还是"client
  走错路"

## Related Issues

- 无 GitHub issue 关联（本仓库无对应 tracker 条目）
- 与本仓库其他 solutions 的重叠度：低（proxy/NO_PROXY 是首次踩坑记录）
- 备查清单（修其他 ralph-* crate 的测试时若复现同样错误，先回查本文件）：
  - `ralph-cli`：loop_runner 子测试 + wave dispatcher
  - `ralph-tui`：mock backend 集成
  - `ralph-e2e`：live API 测试（看是否需要类似保护）
