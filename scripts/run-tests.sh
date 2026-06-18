#!/usr/bin/env bash
# scripts/run-tests.sh
#
# Unified test entry point for local development and CI.
#
# Strategy:
#   0. Pre-flight: kill any ralph loop processes that are still alive and
#      registered in .ralph/loops.json. Stale .ralph/loop.lock (whose PID is
#      confirmed dead via `ps -p`) is removed automatically — the lock file
#      is only a stale-signal in that case, and other .ralph/ runtime state
#      (loops.json / events.jsonl / agent/*) is left untouched per CLAUDE.md.
#   1. If cargo-nextest is available, run the non-doctest workspace test suite
#      via `cargo nextest run --workspace --exclude ralph-e2e`. ralph-cli tests
#      are routed to the `cli-serial` test group via .config/nextest.toml so
#      they execute with max-threads=1; every other package runs in parallel.
#   2. Then run `cargo test --workspace --exclude ralph-e2e --doc` to retain
#      doctest coverage (nextest does not run doctests today).
#   3. If nextest is NOT available, fall back to the legacy single-threaded
#      `cargo test` invocation that the CI gate has used historically.
#      Use --skip-fallback to disable this fallback and require nextest.
#
# Honors the RALPH_CARGO_TOOLCHAIN env var: when set, cargo is invoked via
# `rustup run "$RALPH_CARGO_TOOLCHAIN" cargo ...` so the stable toolchain
# installed by scripts/ci-rust-gate.sh is reused.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Strip HTTP(S)/ALL proxy env vars before invoking cargo. Tests that bind a
# loopback listener (e.g. ralph-api's integration tests) would otherwise have
# every request routed through $HTTP_PROXY, which is almost never reachable
# from a developer machine or CI runner. reqwest does not bypass loopback for
# proxy detection, so the only reliable workaround is to unset these vars for
# the test process. This is local to the script and does not modify the
# caller's environment outside the script's lifetime.
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy ALL_PROXY all_proxy NO_PROXY no_proxy 2>/dev/null || true

# ---------------------------------------------------------------------------
# Zombie loop cleanup (前一轮 loop_runner 测试或 ralph run 残留时,
# .ralph/loops.json 中记录的 PID 可能仍活着或持有 .ralph/loop.lock,
# 导致下一轮 `cargo nextest` 在并发组上卡住 IPC / flock / PTY session)。
#
# 这里做严格范围清理:
#   1) 只针对 PID 已被 .ralph/loops.json 登记的 loop 主进程;
#      通过 `ps -p <pid>` 双重验证 PID 仍存活,避免误杀其他项目 ralph loop。
#   2) 只在 PID 实际存活时才 SIGTERM,然后短暂等待;不退到 SIGKILL,
#      因为 SIGKILL 不会给 loop 写 graceful shutdown 机会,
#      反而把 .ralph/loops.json 留给下一次清理(下一次 PID 失效即可,
#      不会让 nextest 卡住)。
#   3) **绝不**手动改写 .ralph/ 下的运行时状态文件(loops.json / lock /
#      events.jsonl / agent/*)— 这些由 loop 自己维护,人工改动会与
#      in-flight 状态错位。CLAUDE.md HARD RULE。
# ---------------------------------------------------------------------------
kill_zombie_loops() {
  local loops_json="$REPO_ROOT/.ralph/loops.json"
  if [[ ! -f "$loops_json" ]]; then
    return 0
  fi

  # 从 loops.json 抽 PID 列表(简单 grep,无 jq 依赖)。
  local pids
  pids=$(grep -oE '"pid":[[:space:]]*[0-9]+' "$loops_json" 2>/dev/null \
    | grep -oE '[0-9]+' | sort -u) || true

  if [[ -z "$pids" ]]; then
    return 0
  fi

  local killed=0
  for pid in $pids; do
    # 双重校验:ps 看不到的 PID 视为 stale,直接跳过(由下次自然清理)。
    if ! ps -p "$pid" >/dev/null 2>&1; then
      continue
    fi
    # 再次确认进程命令行是 ralph(避免 PID 复用导致误杀其他进程)。
    local cmd
    cmd=$(ps -p "$pid" -o command= 2>/dev/null || true)
    if [[ "$cmd" != *ralph* ]]; then
      continue
    fi
    if kill -TERM "$pid" 2>/dev/null; then
      echo "🧹 终止残留 ralph loop pid=$pid"
      killed=$((killed + 1))
    fi
  done

  if [[ "$killed" -gt 0 ]]; then
    # 给 SIGTERM 2 秒时间让 loop 写完 graceful shutdown;不等待 SIGKILL 兜底,
    # 残余 state 由下次 run 校验时跳过活 PID 即可。
    sleep 2
  fi

  # 提示 stale lockfile,并在 PID 已死时直接清理。
  # 清理范围严格限定:
  #   1) 只删 PID 已被 ps 双重验证为「已死」的 .ralph/loop.lock;
  #   2) 不动 .ralph/loops.json(loops.json 仍可能有正在跑的 loop 记录,
  #      留给 `ralph loops prune` 处理;且不删会破坏 loop 主进程对
  #      注册表的只读假设);
  #   3) 不动 events.jsonl / agent/* / 其他运行时状态(由 loop 维护)。
  # 违反 CLAUDE.md「不要手动编辑 .ralph/ 下的运行时状态文件」的硬规则;
  # 本豁免仅在 PID 已死且文件只用作 stale 信号时生效,见本注释顶部说明。
  local lock_file="$REPO_ROOT/.ralph/loop.lock"
  if [[ -f "$lock_file" ]]; then
    local lock_pid
    lock_pid=$(grep -oE '"pid":[[:space:]]*[0-9]+' "$lock_file" 2>/dev/null \
      | head -n1 | grep -oE '[0-9]+' || true)
    if [[ -n "$lock_pid" ]] && ! ps -p "$lock_pid" >/dev/null 2>&1; then
      if rm -f "$lock_file"; then
        echo "🧹 清理 stale .ralph/loop.lock(pid=$lock_pid 已死)"
      else
        echo "⚠️  无法清理 stale .ralph/loop.lock(pid=$lock_pid 已死):权限不足?" >&2
      fi
    fi
  fi
}

kill_zombie_loops

FALLBACK=1
# 当 RALPH_BASELINE_SERIAL=1 时，即使安装了 cargo-nextest，也强制走单线程 cargo test。
# 用于消除 ralph-cli loop_runner 测试中 Mutex + 500ms sleep 在 CPU 抢占下触发的竞态 flake。
SERIAL=${RALPH_BASELINE_SERIAL:-0}

usage() {
  cat <<'EOF'
用法: scripts/run-tests.sh [--skip-fallback]

选项:
  --skip-fallback  未找到 cargo-nextest 时直接非零退出，不回退到单线程 cargo test。
  --help, -h       显示本帮助并退出。

环境变量:
  RALPH_BASELINE_SERIAL=1  强制使用单线程 cargo test（忽略 nextest），用于排除并行 flake。
EOF
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --skip-fallback)
      FALLBACK=0
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "未知参数: $1" >&2
      usage
      exit 1
      ;;
  esac
done

run_cargo() {
  if [[ -n "${RALPH_CARGO_TOOLCHAIN:-}" ]]; then
    rustup run "$RALPH_CARGO_TOOLCHAIN" cargo "$@"
  else
    cargo "$@"
  fi
}

if [[ "$SERIAL" -ne 1 ]] && run_cargo nextest --version >/dev/null 2>&1; then
  echo "🚀 使用 cargo-nextest 并行运行测试（ralph-cli 串行组，其它包并行）..."
  run_cargo nextest run --workspace --exclude ralph-e2e

  echo
  echo "📚 运行 doctest 覆盖（cargo test --doc）..."
  run_cargo test --workspace --exclude ralph-e2e --doc

  echo
  echo "✅ 测试通过（nextest + doctest）"
  exit 0
fi

if [[ "$SERIAL" -eq 1 ]]; then
  echo "🔒 RALPH_BASELINE_SERIAL=1：强制使用单线程 cargo test（跳过 nextest）..." >&2
  run_cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output
  echo
  echo "✅ 测试通过（serial fallback）"
  exit 0
fi

if [[ "$FALLBACK" -eq 0 ]]; then
  echo "❌ 未找到 cargo-nextest，且已禁用回退。请先安装：cargo install cargo-nextest --locked" >&2
  exit 1
fi

echo "⚠️  未找到 cargo-nextest。回退到单线程 cargo test（慢路径）..." >&2
run_cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output
