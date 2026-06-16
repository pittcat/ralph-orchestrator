#!/usr/bin/env bash
# scripts/run-tests.sh
#
# Unified test entry point for local development and CI.
#
# Strategy:
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
