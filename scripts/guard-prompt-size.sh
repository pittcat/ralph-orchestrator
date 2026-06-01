#!/bin/bash
# scripts/guard-prompt-size.sh
# 防止未来 ralph-tools.md 重新膨胀到 500+ 行 (plan U8 F3 修订)
#
# 历史:
#   ralph-tools.md 在 v4 计划前已达 743 行, U1b 重写后压缩到 ~68 行.
#   本脚本作为回归保护, 防止后续 contributor 重新把 emit/task/memory 等
#   高频章节塞回入口文件.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MD_FILE="$REPO_ROOT/crates/ralph-core/data/ralph-tools.md"
MAX_LINES=200  # 与 plan U1b T4 一致 (实际 68 行, 留缓冲至 200)
ACTUAL=$(wc -l < "$MD_FILE")
if [ "$ACTUAL" -gt "$MAX_LINES" ]; then
  echo "FAIL: ralph-tools.md is $ACTUAL lines (max $MAX_LINES). It grew back."
  echo "Consider splitting further or moving sections to other ralph-tools-*.md skills."
  echo "  - ralph-tools-emit.md (U2)"
  echo "  - ralph-tools-wave.md (U3)"
  echo "  - ralph-tools-cmdref.md (U4)"
  echo "  - ralph-tools-tasks.md (existing)"
  echo "  - ralph-tools-memories.md (existing)"
  exit 1
fi
echo "OK: ralph-tools.md is $ACTUAL lines (max $MAX_LINES)"
