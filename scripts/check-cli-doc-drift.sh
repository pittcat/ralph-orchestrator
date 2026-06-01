#!/bin/bash
# scripts/check-cli-doc-drift.sh
# 检测 ralph-tools*.md 文档与 --help 输出的双向漂移
# (plan U7: D1 修复 - 双向结构化对比, 覆盖新增/重命名/删除/类型变更)
#
# v1.1 改进: section-aware - 对共享 doc 文件 (如 ralph-tools-cmdref.md 包含多个命令章节),
# 按 H2 标题分割提取各 section 的 flag, 只与对应命令的 --help 对比,
# 避免跨命令假阳性.
#
# 用法:
#   bash scripts/check-cli-doc-drift.sh                    # 默认: 报告所有漂移, exit 0
#   bash scripts/check-cli-doc-drift.sh --strict            # 任何漂移即 exit 1
#   bash scripts/check-cli-doc-drift.sh --update-baseline   # 将当前输出写入 baseline 文件
#   DRIFT_STRICT=1 bash scripts/check-cli-doc-drift.sh     # 同 --strict (env var)
#
# 退出码:
#   0 = 无漂移 (或所有漂移均在 baseline 内)
#   1 = 有新漂移 (或 --strict 模式下任何漂移)
#   2 = 缺少 ralph 二进制
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DOCS_DIR="$REPO_ROOT/crates/ralph-core/data"
SCHEMA_DIR="$(mktemp -d)"
BASELINE="$REPO_ROOT/scripts/cli-doc-drift.baseline"
trap "rm -rf $SCHEMA_DIR" EXIT

# 解析参数
STRICT=0
UPDATE_BASELINE=0
for arg in "$@"; do
  case "$arg" in
    --strict) STRICT=1 ;;
    --update-baseline) UPDATE_BASELINE=1 ;;
    -h|--help)
      grep '^#' "$0" | head -25
      exit 0
      ;;
  esac
done
[ "${DRIFT_STRICT:-0}" = "1" ] && STRICT=1

# 0. 前置：必须有 ralph 二进制
if ! command -v ralph >/dev/null 2>&1; then
  echo "ERROR: ralph not found. Build first: cargo build -p ralph-cli" >&2
  exit 2
fi

# 1. 抽取所有受检命令的 schema
# 格式: <doc_file>|<section_name>. section_name 为该 doc 文件内的 H2 章节名.
# task/memory 子命令在各自独立的 skill 文件 (ralph-tools-tasks/memories.md).
declare -A COMMANDS_TO_DOCS=(
  ["emit"]="ralph-tools-emit.md|ralph emit"
  ["wave emit"]="ralph-tools-wave.md|ralph wave emit"
  ["tools task add"]="ralph-tools-tasks.md|Task Commands"
  ["tools task ensure"]="ralph-tools-tasks.md|Task Commands"
  ["tools task list"]="ralph-tools-tasks.md|Task Commands"
  ["tools task ready"]="ralph-tools-tasks.md|Task Commands"
  ["tools task start"]="ralph-tools-tasks.md|Task Commands"
  ["tools task close"]="ralph-tools-tasks.md|Task Commands"
  ["tools task fail"]="ralph-tools-tasks.md|Task Commands"
  ["tools task reopen"]="ralph-tools-tasks.md|Task Commands"
  ["tools task show"]="ralph-tools-tasks.md|Task Commands"
  ["tools memory add"]="ralph-tools-memories.md|Memory Commands"
  ["tools memory list"]="ralph-tools-memories.md|Memory Commands"
  ["tools memory search"]="ralph-tools-memories.md|Memory Commands"
  ["tools memory prime"]="ralph-tools-memories.md|Memory Commands"
  ["tools memory show"]="ralph-tools-memories.md|Memory Commands"
  ["tools memory delete"]="ralph-tools-memories.md|Memory Commands"
  ["tools memory init"]="ralph-tools-memories.md|Memory Commands"
  ["tools skill list"]="ralph-tools-cmdref.md|ralph tools skill"
  ["tools skill load"]="ralph-tools-cmdref.md|ralph tools skill"
  ["tools interact progress"]="ralph-tools-cmdref.md|ralph tools interact"
  ["run"]="ralph-tools-cmdref.md|ralph run"
  ["preflight"]="ralph-tools-cmdref.md|其他命令"
  ["doctor"]="ralph-tools-cmdref.md|其他命令"
  ["hooks"]="ralph-tools-cmdref.md|其他命令"
  ["init"]="ralph-tools-cmdref.md|其他命令"
  ["clean"]="ralph-tools-cmdref.md|其他命令"
  ["plan"]="ralph-tools-cmdref.md|其他命令"
  ["code-task"]="ralph-tools-cmdref.md|其他命令"
  ["loops"]="ralph-tools-cmdref.md|其他命令"
  ["hats"]="ralph-tools-cmdref.md|其他命令"
  ["tui"]="ralph-tools-cmdref.md|其他命令"
  ["web"]="ralph-tools-cmdref.md|其他命令"
  ["mcp"]="ralph-tools-cmdref.md|其他命令"
  ["bot"]="ralph-tools-cmdref.md|其他命令"
  ["completions"]="ralph-tools-cmdref.md|其他命令"
)

# 提取 doc 文件中指定 section (## <section_name>) 内的 --flag 集合
extract_section_flags() {
  local doc_file="$1"
  local section_name="$2"
  if [ -z "$section_name" ]; then
    grep -oE -- '--[a-zA-Z][a-zA-Z0-9-]+' "$doc_file" 2>/dev/null \
      | sed 's/^--//' | sort -u
    return
  fi
  python3 - "$doc_file" "$section_name" <<'PYEOF'
import re, sys
doc_file, section_name = sys.argv[1], sys.argv[2]
content = open(doc_file).read()
# 匹配 '## section_name' (允许可选反引号)
section_re = re.compile(r'^##\s+`?' + re.escape(section_name) + r'`?\s*$', re.MULTILINE)
sections = list(section_re.finditer(content))
if not sections:
    # 退化为整文件
    print('\n'.join(re.findall(r'--([a-zA-Z][a-zA-Z0-9-]+)', content)))
    sys.exit(0)
start = sections[0].end()
next_section = re.search(r'^##\s', content[start:], re.MULTILINE)
end = start + next_section.start() if next_section else len(content)
section_text = content[start:end]
flags = sorted(set(re.findall(r'--([a-zA-Z][a-zA-Z0-9-]+)', section_text)))
print('\n'.join(flags))
PYEOF
}

# 收集所有漂移 (用于 baseline 比对和最终输出)
DRIFT_LINES=()
ERRORS=0
for cmd in "${!COMMANDS_TO_DOCS[@]}"; do
  mapping="${COMMANDS_TO_DOCS[$cmd]}"
  doc_file="$DOCS_DIR/${mapping%|*}"
  section_name="${mapping#*|}"
  schema_file="$SCHEMA_DIR/$(echo "$cmd" | tr ' ' '_').json"

  # 抽取 --help schema
  if ! python3 "$REPO_ROOT/scripts/extract-cli-schema.py" $cmd > "$schema_file" 2>/dev/null; then
    echo "WARN: failed to extract schema for '$cmd' --help; skipping" >&2
    continue
  fi

  # 解析对应 section 内的 --flag 集合
  doc_flags=$(extract_section_flags "$doc_file" "$section_name")

  # 解析 schema 中的 flag 集合
  help_flags=$(python3 -c "
import json
data = json.load(open('$schema_file'))
print('\n'.join(f['name'] for f in data['flags']))
" | sort -u)

  # 反向检查：--help 中的 flag 是否在 .md 对应 section 中存在
  for flag in $help_flags; do
    if ! echo "$doc_flags" | grep -qx "$flag"; then
      msg="DRIFT: 'ralph $cmd' has --$flag in --help, but not documented in ${mapping%|*} (section: $section_name)"
      DRIFT_LINES+=("$msg")
      ERRORS=$((ERRORS + 1))
    fi
  done

  # 正向检查：.md 对应 section 中提到的 flag 是否在 --help 存在
  for flag in $doc_flags; do
    if ! echo "$help_flags" | grep -qx "$flag"; then
      msg="DRIFT: ${mapping%|*} (section: $section_name) mentions --$flag, but 'ralph $cmd --help' no longer has it"
      DRIFT_LINES+=("$msg")
      ERRORS=$((ERRORS + 1))
    fi
  done
done

# --update-baseline: 写入 baseline 后退出
if [ "$UPDATE_BASELINE" = "1" ]; then
  printf '%s\n' "${DRIFT_LINES[@]}" > "$BASELINE" 2>/dev/null || true
  echo "Baseline updated: $BASELINE (${#DRIFT_LINES[@]} drifts recorded)"
  exit 0
fi

# 默认输出所有漂移到 stdout
for line in "${DRIFT_LINES[@]}"; do
  echo "$line"
done

# 退出码逻辑
if [ $ERRORS -gt 0 ]; then
  if [ -f "$BASELINE" ] && [ "$STRICT" = "0" ]; then
    # baseline 模式 (默认): 仅 baseline 之外的新漂移算 fail
    NEW_ERRORS=0
    for line in "${DRIFT_LINES[@]}"; do
      if ! grep -Fxq "$line" "$BASELINE"; then
        echo "NEW DRIFT (not in baseline): $line" >&2
        NEW_ERRORS=$((NEW_ERRORS + 1))
      fi
    done
    if [ $NEW_ERRORS -gt 0 ]; then
      echo "" >&2
      echo "CLI doc drift: $ERRORS total, $NEW_ERRORS new (not in baseline)" >&2
      echo "Either update docs or run: $0 --update-baseline" >&2
      exit 1
    fi
    echo "CLI doc drift check passed ($ERRORS known in baseline, 0 new)"
    exit 0
  fi
  echo "" >&2
  echo "CLI doc drift detected: $ERRORS issue(s)" >&2
  echo "Either update the documentation or fix the regression in the command." >&2
  echo "(To start tracking baseline: $0 --update-baseline)" >&2
  exit 1
fi

echo "CLI doc drift check passed"
