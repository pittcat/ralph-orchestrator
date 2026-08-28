#!/usr/bin/env bash
# scripts/run-tests.sh
#
# Unified test entry point for local development and CI.
#
# Strategy:
#   1. 如果 cargo-nextest 可用,跑 nextest + doctest(Phase 1 限制并发,
#      Phase 2 串行隔离 race-sensitive 测试)。
#   2. 否则回退到单线程 cargo test。
#
# 关键不变量:测试脚本只能管理自己启动的进程，不能影响其它 worktree
# 或其它终端中的 cargo/rustc/nextest。
# - 不做全局 pgrep/pkill；超时/中断时只递归清理本脚本的子进程树；
# - 10 分钟总超时(超时后 trap EXIT 强制清场,确保下次能跑);
# - 每个 worktree 使用自己的 target 目录；Cargo registry/cache 仍可共享。
# - workspace 全量 nextest 默认最多 4 个并发测试进程；跨 crate 的高并发会
#   放大共享 OS 资源和 wall-clock 时序测试的争用，导致非确定性失败。

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Cargo 默认会把 target 放在当前 worktree 根目录。显式固定这个值，避免
# 用户 shell 中遗留的全局 CARGO_TARGET_DIR 把多个 worktree 合并到一个
# target 目录，导致编译产物和 Cargo 锁互相干扰。
export CARGO_TARGET_DIR="$REPO_ROOT/target"

# Strip HTTP(S)/ALL proxy env vars before invoking cargo.
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy ALL_PROXY all_proxy NO_PROXY no_proxy 2>/dev/null || true

# Strip agent-context env inherited from an outer `ralph run` hat.
# Without this, integration tests that spawn `ralph` treat the suite as
# an in-loop agent (ACL / emit allowlist / skill visibility) and fail
# even though the same suite passes in a human shell.
unset RALPH_CURRENT_HAT RALPH_CURRENT_LOOP_ID RALPH_EVENTS_FILE \
  RALPH_WAVE_WORKER RALPH_WAVE_ID RALPH_WAVE_INDEX RALPH_WAVE_TOTAL \
  RALPH_TRIGGERED_HAT RALPH_HATS_SOURCE RALPH_CONFIG \
  RALPH_CURRENT_BRANCH \
  RALPH_WORKSPACE_ROOT RALPH_LOOP_ITERATION \
  2>/dev/null || true

# 全局总超时:防止意外卡住(例如 IPC deadlock、PTY 未释放)。
# 25 分钟覆盖 workspace 全量基线:本地 macOS nextest ~3 分钟 + doctest ~2-3 分钟(已跳过空 crate)
# + rustdoc 全量首跑开销;CI ubuntu-latest 上实测 ~5-7 分钟,本地 macOS 跳过 doctest 后 ~6-8 分钟。
# 详见 docs/solutions/developer-experience/run-tests-doctest-timeout-and-skip-empty-crates-2026-06-21.md
TOTAL_TIMEOUT_SECONDS=${TOTAL_TIMEOUT_SECONDS:-1500}

# nextest 的默认 num-cpus 并发在本机上会让 workspace 的 76 个测试 binary
# 同时争抢 CPU / 文件描述符 / PTY 等 OS 资源，造成 wave-supervisor 测试偶发
# 得到 0 个 worker，而同一测试单独或串行运行均通过。默认限制为 4 保留并行
# 能力，同时避免这种跨 crate 争用；CI 或更强机器可显式覆盖。
if [[ -z "${RALPH_NEXTTEST_JOBS:-}" ]]; then
  cpu_count=4
  if command -v sysctl >/dev/null 2>&1; then
    cpu_count=$(sysctl -n hw.ncpu 2>/dev/null || echo 4)
  elif command -v getconf >/dev/null 2>&1; then
    cpu_count=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)
  fi
  [[ "$cpu_count" =~ ^[0-9]+$ ]] || cpu_count=4
  (( cpu_count < 4 )) && RALPH_NEXTTEST_JOBS="$cpu_count" || RALPH_NEXTTEST_JOBS=4
fi
export RALPH_NEXTTEST_JOBS

# wave_supervisor 使用 store-backed fixture；其测试在单独执行时稳定，但在
# workspace 多 binary 并发时会出现偶发时序失败。保留在全量门禁中，改为 -j 1。
SERIAL_TEST_FILTER='test(/partial_timeout_events_visible/) or test(/test_execute_wave_/) or test(/wave_supervisor::/)'

# 全量测试跨 worktree 串行化。Git worktree 共享同一个 git common directory，
# 因此锁不能放在 $REPO_ROOT 下；否则每个 worktree 都会拿到自己的锁。
GIT_COMMON_DIR=$(git rev-parse --git-common-dir 2>/dev/null || true)
if [[ "$GIT_COMMON_DIR" != /* ]]; then
  GIT_COMMON_DIR="$REPO_ROOT/$GIT_COMMON_DIR"
fi
GIT_COMMON_DIR=$(cd "$GIT_COMMON_DIR" 2>/dev/null && pwd -P) || {
  echo "❌ 无法解析 Git common directory，不能安全执行全量测试锁" >&2
  exit 1
}
FULL_TEST_LOCK_DIR="$GIT_COMMON_DIR/ralph-full-tests.lock"
FULL_TEST_LOCK_OWNER="$FULL_TEST_LOCK_DIR/pid"

acquire_full_test_lock() {
  local owner_pid
  while ! mkdir "$FULL_TEST_LOCK_DIR" 2>/dev/null; do
    owner_pid=$(cat "$FULL_TEST_LOCK_OWNER" 2>/dev/null || true)
    if [[ -z "$owner_pid" ]]; then
      # worktree 文件存在但 pid 已丢失 → Ctrl+C 在 acquire 后中断、
      # release 还没清理 worktree 就退出了。活锁场景下 acquire 先写
      # pid 再写 worktree,所以 worktree 残留意味着 owner 已死,直接清。
      if [[ -f "$FULL_TEST_LOCK_DIR/worktree" ]]; then
        echo "🧹 清理 Ctrl+C 残留的 stale 全量测试锁(worktree 残留,无 pid)..." >&2
        rm -f "$FULL_TEST_LOCK_DIR/worktree"
        rmdir "$FULL_TEST_LOCK_DIR" 2>/dev/null || true
      else
        # 空 lock dir:可能是活锁刚 mkdir,短暂等 owner 写 pid
        sleep 1
      fi
      continue
    fi
    if kill -0 "$owner_pid" 2>/dev/null; then
      echo "⏳ 已有 worktree 正在执行全量测试(pid=$owner_pid)，当前 worktree 等待..."
      sleep 5
    else
      echo "🧹 清理已退出测试进程留下的 stale 全量测试锁(pid=$owner_pid)" >&2
      rm -f "$FULL_TEST_LOCK_OWNER"
      rmdir "$FULL_TEST_LOCK_DIR" 2>/dev/null || true
    fi
  done
  printf '%s\n' "$$" > "$FULL_TEST_LOCK_OWNER"
  printf '%s\n' "$REPO_ROOT" > "$FULL_TEST_LOCK_DIR/worktree"
}

release_full_test_lock() {
  local owner_pid
  owner_pid=$(cat "$FULL_TEST_LOCK_OWNER" 2>/dev/null || true)
  if [[ "$owner_pid" == "$$" ]]; then
    rm -f "$FULL_TEST_LOCK_OWNER" "$FULL_TEST_LOCK_DIR/worktree"
    rmdir "$FULL_TEST_LOCK_DIR" 2>/dev/null || true
  fi
}

# 递归获取并终止当前脚本的后代进程。这里按 PPID 关系识别 owner，
# 不按 cargo/rustc 命令行全局匹配，因此不会碰其它 worktree。
descendants_of() {
  local parent="$1"
  local children child
  children=$(pgrep -P "$parent" 2>/dev/null || true)
  for child in $children; do
    printf '%s\n' "$child"
    descendants_of "$child"
  done
}

terminate_descendants() {
  local signal="$1"
  local exclude_pid="${2:-}"
  local pid
  while read -r pid; do
    [[ -z "$pid" || "$pid" == "$exclude_pid" ]] && continue
    kill -"$signal" "$pid" 2>/dev/null || true
  done < <(descendants_of "$$" | sort -rn)
}

# 只停止 timeout watcher；正常退出不清理其它进程。
cleanup_on_exit() {
  kill "${TIMEOUT_PID:-}" 2>/dev/null || true
  release_full_test_lock
}
trap cleanup_on_exit EXIT

# 总超时：先终止当前脚本启动的后代，再通知主脚本退出。
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
  echo "⏰ 总超时 ${TOTAL_TIMEOUT_SECONDS}s,只清理本脚本启动的进程..." >&2
  # 让主脚本执行清理；这样兼容 macOS 自带的 Bash 3.2（没有 BASHPID）。
  kill -TERM "$$" 2>/dev/null || true
  exit 124
}

# ---- 主流程 ----

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
  RALPH_NEXTTEST_JOBS=N       Phase 1 nextest 并发数(默认 min(CPU 数, 4))。
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

acquire_full_test_lock
START_TS=$SECONDS

# 启动超时 watcher(后台)
timeout_watcher &
TIMEOUT_PID=$!

# 中断只清理当前脚本的后代；TERM 由 timeout watcher 使用。
trap 'terminate_descendants KILL "$TIMEOUT_PID"; kill "$TIMEOUT_PID" 2>/dev/null || true; exit 130' INT
trap 'terminate_descendants KILL "$TIMEOUT_PID"; kill "$TIMEOUT_PID" 2>/dev/null || true; exit 124' TERM

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
  echo "🚀 使用 cargo-nextest 并行运行测试(Phase 1 最多 ${RALPH_NEXTTEST_JOBS} 个并发)..."

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
  echo "📦 Phase 1: full workspace at ${RALPH_NEXTTEST_JOBS} concurrent jobs..."
  # 关闭 errexit/pipefail 触发的即时退出:我们要跑完所有阶段并汇总,
  # 失败与否由 OVERALL_RC 记录,最后统一 exit。
  set +e
  P1_LOG=$(mktemp)
  OVERALL_RC=0
  run_cargo nextest run \
    --workspace \
    --exclude ralph-e2e \
    -j "$RALPH_NEXTTEST_JOBS" \
    -E "not (${SERIAL_TEST_FILTER})" 2>&1 | tee "$P1_LOG"
  [[ ${PIPESTATUS[0]} -ne 0 ]] && OVERALL_RC=1
  P1_SUM=$(parse_nextest_summary "$P1_LOG")

  echo
  echo "🐢 Phase 2: race-sensitive tests at -j 1..."
  P2_LOG=$(mktemp)
  run_cargo nextest run \
    --workspace \
    --exclude ralph-e2e \
    -j 1 \
    -E "$SERIAL_TEST_FILTER" 2>&1 | tee "$P2_LOG"
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
