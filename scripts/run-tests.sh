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
  RALPH_WORKSPACE_ROOT RALPH_LOOP_ITERATION \
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

START_TS=$SECONDS

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

# ---------------------------------------------------------------------------
# 结果汇总:把 nextest / doctest 的 summary 行解析成结构化数字,
# 最后渲染成一张对齐的表格。纯展示,不影响退出码。
# 表格列内容全部用 ASCII/数字,避免 CJK 双宽字符破坏对齐;
# 中文只放在无边框的标题行 / 状态行。
# ---------------------------------------------------------------------------

# 解析 nextest 输出的最后一条 Summary 行 → "time|run|passed|failed|skipped"
parse_nextest_summary() {
  local log="$1" line
  line=$(grep -E 'tests? run:' "$log" 2>/dev/null | tail -n1)
  if [[ -z "$line" ]]; then
    echo "-|-|-|-|-"
    return
  fi
  local t run passed failed skipped
  t=$(echo "$line" | grep -oE '\[[[:space:]]*[0-9.]+s\]' | grep -oE '[0-9.]+' | head -n1)
  run=$(echo "$line" | grep -oE '[0-9]+ tests? run' | grep -oE '[0-9]+' | head -n1)
  passed=$(echo "$line" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+' | head -n1)
  failed=$(echo "$line" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+' | head -n1)
  skipped=$(echo "$line" | grep -oE '[0-9]+ skipped' | grep -oE '[0-9]+' | head -n1)
  echo "${t:-?}s|${run:-?}|${passed:-0}|${failed:-0}|${skipped:-0}"
}

# 解析 doctest 输出(可能多 crate)→ "time|run|passed|failed|ignored"
parse_doctest_summary() {
  local log="$1"
  local passed=0 failed=0 ignored=0 p f i
  while read -r p f i; do
    passed=$((passed + p)); failed=$((failed + f)); ignored=$((ignored + i))
  done < <(grep -E '^test result:' "$log" 2>/dev/null \
    | sed -E 's/^test result: [A-Za-z]+\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1 \2 \3/')
  local t
  t=$(grep -oE 'all doctests ran in [0-9.]+s' "$log" 2>/dev/null | grep -oE '[0-9.]+' | tail -n1)
  echo "${t:-?}s|$((passed + failed + ignored))|${passed}|${failed}|${ignored}"
}

# 表格列宽(内容宽度):Stage Run Pass Fail Skip Time
_SUMMARY_WIDTHS=(16 6 7 6 8 9)

_summary_hline() { # $1=left $2=mid $3=right
  local out="$1" i n=${#_SUMMARY_WIDTHS[@]} seg
  for i in "${!_SUMMARY_WIDTHS[@]}"; do
    printf -v seg '─%.0s' $(seq 1 $((_SUMMARY_WIDTHS[i] + 2)))
    out+="$seg"
    if [[ $i -lt $((n - 1)) ]]; then out+="$2"; else out+="$3"; fi
  done
  printf '%s\n' "$out"
}

_summary_row() { # stage run pass fail skip time
  printf '│ %-16s │ %6s │ %7s │ %6s │ %8s │ %9s │\n' "$@"
}

# 把 "time|run|passed|failed|skipped" 拆成表格行(顺序 run pass fail skip time)
_summary_row_from() { # $1=stage $2=packed
  local a
  IFS='|' read -ra a <<< "$2"
  _summary_row "$1" "${a[1]}" "${a[2]}" "${a[3]}" "${a[4]}" "${a[0]}"
}

render_summary_table() { # $1=overall_rc
  local rc="$1"
  echo
  echo "  📊 测试结果汇总"
  _summary_hline "┌" "┬" "┐"
  _summary_row "Stage" "Run" "Pass" "Fail" "Skip" "Time"
  _summary_hline "├" "┼" "┤"
  _summary_row_from "Phase 1 (par)" "$P1_SUM"
  _summary_row_from "Phase 2 (ser)" "$P2_SUM"
  if [[ -n "${DOC_SUM:-}" ]]; then
    _summary_row_from "Doctest" "$DOC_SUM"
  else
    _summary_row "Doctest" "-" "-" "-" "skip" "-"
  fi
  _summary_hline "└" "┴" "┘"
  if [[ "$rc" -eq 0 ]]; then
    printf '  \033[32m✅ 全部通过\033[0m · 总耗时 %ss\n' "$((SECONDS - START_TS))"
  else
    printf '  \033[31m❌ 存在失败\033[0m · 总耗时 %ss(详见上方 FAIL 行)\n' "$((SECONDS - START_TS))"
  fi
}

if [[ "$SERIAL" -ne 1 ]] && run_cargo nextest --version >/dev/null 2>&1; then
  echo "🚀 使用 cargo-nextest 并行运行测试(ralph-cli 串行组,其它包并行)..."

  # 2026-07-25: two-phase run — fast path + isolated slow path.
  # Phase 1: full workspace with default num-cpus concurrency,
  #   EXCLUDING the `partial_timeout_events_visible` trio + two
  #   race-sensitive backend-invocation tests. Those tests
  #   hard-code a 1s worker-timeout assertion; under saturated
  #   CPU contention they trip the timeout (wall-clock observed
  #   at ~1.06-1.09s — only ~80ms slack above the 1s hard limit).
  # Phase 2: re-run those tests with `-j 1` so they get a
  #   quiescent core and the 1s timeout doesn't fire on a busy
  #   CI runner. Three serial tests take ~3-4s vs ~6-8s of
  #   contention-retry cost, so the slow path is cheaper than
  #   the global `RALPH_BASELINE_SERIAL=1` fallback (which
  #   serialises the whole workspace).
  #
  # 2026-07-27-003 plan U2/U3 added two more race-sensitive
  # tests in the same family:
  #   * `test_execute_wave_hat_backend_invocation_contracts`
  #   * `test_execute_wave_falls_back_to_global_backend_when_hat_backend_is_invalid`
  # Both rely on the dispatcher preparing a per-wave channel
  # registry before spawn and then driving backend subprocesses
  # through the `WaveWorkerExecutor` mock; under nextest default
  # concurrency they flake on the same 80ms-slack class of
  # wall-clock assertions. They are routed to Phase 2 alongside
  # the original trio for the same reason.
  # See CLAUDE.md "hooks-executor-test-flake" for the
  #   parallel-failure characterisation that motivates this split.
  echo "📦 Phase 1: full workspace at default num-cpus concurrency..."
  # 关闭 errexit/pipefail 触发的即时退出:我们要跑完所有阶段并汇总,
  # 失败与否由 OVERALL_RC 记录,最后统一 exit。
  set +e
  P1_LOG=$(mktemp)
  OVERALL_RC=0
  run_cargo nextest run \
    --workspace \
    --exclude ralph-e2e \
    -E 'not test(/partial_timeout_events_visible/) and not test(/test_execute_wave_/)' 2>&1 | tee "$P1_LOG"
  [[ ${PIPESTATUS[0]} -ne 0 ]] && OVERALL_RC=1
  P1_SUM=$(parse_nextest_summary "$P1_LOG")

  echo
  echo "🐢 Phase 2: race-sensitive tests at -j 1..."
  P2_LOG=$(mktemp)
  run_cargo nextest run \
    --workspace \
    --exclude ralph-e2e \
    -j 1 \
    -E 'test(/partial_timeout_events_visible/) or test(/test_execute_wave_/)' 2>&1 | tee "$P2_LOG"
  [[ ${PIPESTATUS[0]} -ne 0 ]] && OVERALL_RC=1
  P2_SUM=$(parse_nextest_summary "$P2_LOG")

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
    DOC_SUM=""
  elif [[ ${#crates_skipped[@]} -gt 0 ]]; then
    echo "📚 doctest:跳过 ${#crates_skipped[@]} 个无 doctest 的 crate($(IFS=,; echo "${crates_skipped[*]}")),只跑:$(IFS=,; echo "${crates_with_doc[*]}")"
    DOC_LOG=$(mktemp)
    for crate_name in "${crates_with_doc[@]}"; do
      run_cargo test -p "$crate_name" --doc 2>&1 | tee -a "$DOC_LOG"
      [[ ${PIPESTATUS[0]} -ne 0 ]] && OVERALL_RC=1
    done
    DOC_SUM=$(parse_doctest_summary "$DOC_LOG")
  else
    echo "📚 运行 doctest 覆盖(cargo test --workspace --doc)..."
    DOC_LOG=$(mktemp)
    run_cargo test --workspace --exclude ralph-e2e --doc 2>&1 | tee "$DOC_LOG"
    [[ ${PIPESTATUS[0]} -ne 0 ]] && OVERALL_RC=1
    DOC_SUM=$(parse_doctest_summary "$DOC_LOG")
  fi

  render_summary_table "$OVERALL_RC"
  kill $TIMEOUT_PID 2>/dev/null || true
  exit "$OVERALL_RC"
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
