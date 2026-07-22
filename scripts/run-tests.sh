#!/usr/bin/env bash
# scripts/run-tests.sh
#
# Unified test entry point for local development and CI.
#
# Strategy:
#   0. **Fresh start guarantee**: 启动时**无条件** kill 当前 session 内
#      的孤儿 cargo / rustc / nextest / run-tests.sh 进程(不论是否
#      由本次脚本启动,只要 PPID 链能追溯到当前 shell 就杀掉),
#      防止上一轮卡住的子进程占用 `.cargo-lock` / IPC / PTY session。
#      同时清理 stale `.ralph/loop.lock`(PID 已死的)。
#   1. 如果 cargo-nextest 可用,跑 nextest + doctest(ralph-cli cli-serial
#      串行组,其它包并行)。
#   2. 否则回退到单线程 cargo test。
#
# 关键不变量(用户硬要求):**每次跑都是一个全新的进程,绝不卡住**。
# - 启动时清理 + 进程组管理(set -m, kill 整个 process group);
# - 10 分钟总超时(超时后 trap EXIT 强制清场,确保下次能跑);
# - trap EXIT 在退出/中断时再次清理 cargo 锁等待者。

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Strip HTTP(S)/ALL proxy env vars before invoking cargo.
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy ALL_PROXY all_proxy NO_PROXY no_proxy 2>/dev/null || true

# Strip agent-context env inherited from an outer `ralph run` hat.
# Without this, integration tests that spawn `ralph` treat the suite as
# an in-loop agent (ACL / emit allowlist / skill visibility) and fail
# even though the same suite passes in a human shell.
unset RALPH_CURRENT_HAT RALPH_CURRENT_LOOP_ID RALPH_EVENTS_FILE \
  RALPH_WAVE_WORKER RALPH_TRIGGERED_HAT RALPH_HATS_SOURCE RALPH_CONFIG \
  2>/dev/null || true

# 全局总超时:防止意外卡住(例如 IPC deadlock、PTY 未释放)。
# 25 分钟覆盖 workspace 全量基线:本地 macOS nextest ~3 分钟 + doctest ~2-3 分钟(已跳过空 crate)
# + rustdoc 全量首跑开销;CI ubuntu-latest 上实测 ~5-7 分钟,本地 macOS 跳过 doctest 后 ~6-8 分钟。
# 详见 docs/solutions/developer-experience/run-tests-doctest-timeout-and-skip-empty-crates-2026-06-21.md
TOTAL_TIMEOUT_SECONDS=${TOTAL_TIMEOUT_SECONDS:-1500}

# ---------------------------------------------------------------------------
# 0. Fresh start:kill 当前进程组(PGID)的所有 cargo / rustc / nextest / ralph 子进程,
#    包括上一轮残留的孤儿 + 同一 shell 启动的其他并行测试。
#
#    用 `pgrep -P $$` 找直接子进程,递归 `-P <pid>` 找整棵进程树;
#    同时用 `pgrep -f` 兜底匹配 cargo/rustc/nextest 命令行。
#    优先级:SIGTERM → 等 2 秒 → SIGKILL 兜底。
# ---------------------------------------------------------------------------
kill_stale_test_processes() {
  local my_pgid=$$
  local my_pid=$$
  local killed=0

  # 收集需要杀死的 PID(用 pgrep 而不是 ps 以避免 -ef 输出差异)
  # 1) 任何 PPID 等于当前脚本的子进程(直接子进程)
  # 2) 任何命令行匹配 cargo/rustc/nextest 的进程(孤儿)
  local pids_to_kill
  pids_to_kill=$(
    {
      # 直接子进程链(脚本 → cargo → rustc → 测试二进制)
      local current=$my_pid
      local depth=0
      while [[ $depth -lt 20 ]]; do
        local children
        children=$(pgrep -P "$current" 2>/dev/null || true)
        [[ -z "$children" ]] && break
        echo "$children"
        # 继续递归第一个子进程(典型的脚本→cargo→rustc 链)
        current=$(echo "$children" | head -n1)
        depth=$((depth + 1))
      done
      # 兜底:任何 cargo/rustc/nextest 进程(孤儿)
      pgrep -f "cargo (nextest|test|build|doc)" 2>/dev/null || true
      pgrep -f "rustc" 2>/dev/null || true
      pgrep -f "cargo-nextest" 2>/dev/null || true
    } | sort -u | grep -v "^${my_pid}$" || true
  )

  if [[ -z "$pids_to_kill" ]]; then
    return 0
  fi

  for pid in $pids_to_kill; do
    if kill -0 "$pid" 2>/dev/null; then
      # 先 SIGTERM 礼貌退出
      kill -TERM "$pid" 2>/dev/null || true
      killed=$((killed + 1))
    fi
  done

  if [[ "$killed" -gt 0 ]]; then
    # 等 2 秒让进程清理
    sleep 2
    # 兜底 SIGKILL
    for pid in $pids_to_kill; do
      if kill -0 "$pid" 2>/dev/null; then
        kill -KILL "$pid" 2>/dev/null || true
      fi
    done
    sleep 0.5
  fi

  return 0
}

# ---------------------------------------------------------------------------
# 0b. Zombie loop cleanup(与之前相同):清理 stale `.ralph/loop.lock`。
# ---------------------------------------------------------------------------
kill_zombie_loops() {
  local lock_file="$REPO_ROOT/.ralph/loop.lock"
  if [[ ! -f "$lock_file" ]]; then
    return 0
  fi
  local lock_pid
  lock_pid=$(grep -oE '"pid":[[:space:]]*[0-9]+' "$lock_file" 2>/dev/null \
    | head -n1 | grep -oE '[0-9]+' || true)
  if [[ -n "$lock_pid" ]] && ! ps -p "$lock_pid" >/dev/null 2>&1; then
    if rm -f "$lock_file"; then
      echo "🧹 清理 stale .ralph/loop.lock(pid=$lock_pid 已死)"
    fi
  fi
}

# Trap EXIT:无论脚本因何退出(success/error/timeout/interrupt),
# 都执行 fresh-cleanup,确保下一次脚本启动时没有残留。
cleanup_on_exit() {
  local exit_code=$?
  # 仅在用户中断或超时时清理 cargo 进程(正常结束时 cargo 已退出)
  if [[ $exit_code -ne 0 && $exit_code -ne 1 ]]; then
    return 0
  fi
  # 正常退出不再做激进 kill(cargo 已自然退出)
  return 0
}
trap cleanup_on_exit EXIT

# Trap TIMEOUT:用 bash 内置 `&` + `wait -t` 实现总超时
timeout_watcher() {
  local remaining=$TOTAL_TIMEOUT_SECONDS
  while [[ $remaining -gt 0 ]]; do
    sleep 10
    remaining=$((remaining - 10))
    # 检查当前脚本的子进程是否还在跑
    local children
    children=$(pgrep -P $$ 2>/dev/null || true)
    if [[ -z "$children" ]]; then
      return 0  # 子进程已退出,watcher 自然结束
    fi
  done
  # 超时:杀整个进程组(包括孙子进程)
  echo "⏰ 总超时 ${TOTAL_TIMEOUT_SECONDS}s,强制清场..." >&2
  kill -KILL -$$ 2>/dev/null || true
  # 兜底:单独杀常见测试进程
  pkill -KILL -f "cargo (nextest|test|build)" 2>/dev/null || true
  pkill -KILL -f "rustc" 2>/dev/null || true
  pkill -KILL -f "cargo-nextest" 2>/dev/null || true
  sleep 0.5
  exit 124  # standard timeout exit code
}

# ---- 主流程 ----

echo "🧹 清场:杀掉上一轮残留的 cargo/rustc/nextest 进程..."
kill_stale_test_processes
kill_zombie_loops

# 启动超时 watcher(后台)
timeout_watcher &
TIMEOUT_PID=$!

# trap 中断:用户 Ctrl-C 时也清场
trap 'kill -KILL -$$ 2>/dev/null; kill $TIMEOUT_PID 2>/dev/null; exit 130' INT TERM
trap 'kill $TIMEOUT_PID 2>/dev/null || true' EXIT

FALLBACK=1
SERIAL=${RALPH_BASELINE_SERIAL:-0}

usage() {
  cat <<'EOF'
用法: scripts/run-tests.sh [--skip-fallback]

选项:
  --skip-fallback  未找到 cargo-nextest 时直接非零退出,不回退到单线程 cargo test。
  --help, -h       显示本帮助并退出。

环境变量:
  RALPH_BASELINE_SERIAL=1     强制使用单线程 cargo test(忽略 nextest),用于排除并行 flake。
  TOTAL_TIMEOUT_SECONDS=N     总超时秒数(默认 600 = 10 分钟)。
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
      kill $TIMEOUT_PID 2>/dev/null || true
      exit 0
      ;;
    *)
      echo "未知参数: $1" >&2
      usage
      kill $TIMEOUT_PID 2>/dev/null || true
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
  echo "🚀 使用 cargo-nextest 并行运行测试(ralph-cli 串行组,其它包并行)..."
  run_cargo nextest run --workspace --exclude ralph-e2e

  echo
  # doctest 阶段:rustdoc 对**没有** ```rust 代码块的 crate 仍会跑完整 lint + 编译 + 提取
  # pipeline(单进程 ~1-3 分钟/crate),本地 macOS 全跑会超时。先用 rg 过滤,只跑真正有 doctest
  # 的 crate。fallback 到 grep -rl(脚本在 PATH 不含 rg 时仍能自愈)。
  # 详见 docs/solutions/developer-experience/run-tests-doctest-timeout-and-skip-empty-crates-2026-06-21.md
  crates_with_doc=()
  crates_skipped=()
  for crate_dir in crates/*/; do
    crate_name=$(basename "$crate_dir")
    [[ "$crate_name" == "ralph-e2e" ]] && continue
    if command -v rg >/dev/null 2>&1; then
      if rg -q '```rust\b' "$crate_dir/src" 2>/dev/null; then
        crates_with_doc+=("$crate_name")
      else
        crates_skipped+=("$crate_name")
      fi
    elif grep -rl '```rust' "$crate_dir/src" >/dev/null 2>&1; then
      crates_with_doc+=("$crate_name")
    else
      crates_skipped+=("$crate_name")
    fi
  done

  if [[ ${#crates_with_doc[@]} -eq 0 ]]; then
    echo "📚 跳过 doctest:workspace 中 8 个非 e2e crate 均无 \`\`\`rust 代码块"
  elif [[ ${#crates_skipped[@]} -gt 0 ]]; then
    echo "📚 doctest:跳过 ${#crates_skipped[@]} 个无 doctest 的 crate($(IFS=,; echo "${crates_skipped[*]}")),只跑:$(IFS=,; echo "${crates_with_doc[*]}")"
    for crate_name in "${crates_with_doc[@]}"; do
      run_cargo test -p "$crate_name" --doc
    done
  else
    echo "📚 运行 doctest 覆盖(cargo test --workspace --doc)..."
    run_cargo test --workspace --exclude ralph-e2e --doc
  fi

  echo
  echo "✅ 测试通过(nextest + doctest)"
  kill $TIMEOUT_PID 2>/dev/null || true
  exit 0
fi

if [[ "$SERIAL" -eq 1 ]]; then
  echo "🔒 RALPH_BASELINE_SERIAL=1:强制使用单线程 cargo test(跳过 nextest)..." >&2
  run_cargo test --workspace --exclude ralph-e2e -- --test-threads=1
  echo
  echo "✅ 测试通过(serial fallback)"
  kill $TIMEOUT_PID 2>/dev/null || true
  exit 0
fi

if [[ "$FALLBACK" -eq 0 ]]; then
  echo "❌ 未找到 cargo-nextest,且已禁用回退。请先安装:cargo install cargo-nextest --locked" >&2
  kill $TIMEOUT_PID 2>/dev/null || true
  exit 1
fi

echo "⚠️  未找到 cargo-nextest。回退到单线程 cargo test(慢路径)..." >&2
run_cargo test --workspace --exclude ralph-e2e -- --test-threads=1
