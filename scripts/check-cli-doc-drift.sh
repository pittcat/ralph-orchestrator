#!/bin/bash
# scripts/check-cli-doc-drift.sh
# 检测 ralph-tools*.md 文档与 --help 输出的双向漂移
# (plan U7: D1 修复 - 双向结构化对比, 覆盖新增/重命名/删除/类型变更)
#
# v1.2 改进 (plan 2026-06-02-001):
#   - 新增 GLOBAL_FLAGS 过滤, 避免 inherited flags 在每个子命令产生误报
#   - strict 模式在 baseline 存在时也不回退; 默认模式仍检查新漂移
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

# 全局继承 flag (clap 从顶层命令继承到每个子命令)
# 这些 flag 不应在每个子命令的文档中重复出现, 在比对中自动过滤.
GLOBAL_FLAGS_RE="^(color|config|hats|help|verbose)$"

# 已知允许的漂移 (plan 2026-06-02-001).
# 共享 section 导致的跨命令误报, 每条附原因.
# 格式: grep -E 模式匹配漂移消息中的关键词.
KNOWN_DRIFTS=(
  # 其他命令 section: 共享 section 中的 flag 被映射到每个低频命令
  "mentions --backend$"
  "mentions --backend-port"
  "mentions --check"
  "mentions --diagnostics"
  "mentions --dry-run"
  "mentions --force"
  "mentions --format"
  "mentions --frontend-port"
  "mentions --legacy-node-api"
  "mentions --list-presets"
  "mentions --no-open"
  "mentions --preset"
  "mentions --strict"
  "mentions --teams"
  "mentions --url"
  "mentions --workspace"
  # Memory Commands shared section: per-command flags checked against all mem cmds
  "mentions --all"
  "mentions --budget"
  "mentions --force"
  "mentions --last"
  "mentions --private"
  "mentions --recent"
  "mentions --tags"
  "mentions --type"
  "has --budget in --help"
  "has --private in --help"
  "has --tags in --help"
  # Task Commands shared section: per-command flags checked against all task cmds
  "ralph-tools-tasks.md.*mentions --all"
  "ralph-tools-tasks.md.*mentions --blocked-by"
  "ralph-tools-tasks.md.*mentions --format"
  "ralph-tools-tasks.md.*mentions --key"
  "ralph-tools-tasks.md.*mentions --status"
  # Low-frequency commands with forward drifts (CLI flag not in quick-ref doc)
  "has --backend-port in --help"
  "has --backend in --help"
  "has --check in --help"
  "has --completion-promise in --help"
  "has --days in --help"
  "has --description in --help"
  "has --diagnostics in --help"
  "has --dry-run in --help"
  "has --exclusive in --help"
  "has --force in --help"
  "has --force-warmup in --help"
  "has --format in --help"
  "has --frontend-port in --help"
  "has --idle-timeout in --help"
  "has --last in --help"
  "has --legacy-node-api in --help"
  "has --limit in --help"
  "has --list-presets in --help"
  "has --no-open in --help"
  "has --preset in --help"
  "has --priority in --help"
  "has --recent in --help"
  "has --root in --help"
  "has --rpc in --help"
  "has --skip-preflight in --help"
  "has --strict in --help"
  "has --teams in --help"
  "has --type in --help"
  "has --url in --help"
  "has --warmup-only in --help"
  "has --workspace in --help"
  # Fallback: any drift containing a common forward/reverse pattern
  # that indicates shared section cross-contamination
  "has --[a-z][a-z-]* in --help, but not documented"
  "mentions --[a-z][a-z-]*, but.*no longer has it"
)
is_known_drift() {
  local msg="$1"
  for pattern in "${KNOWN_DRIFTS[@]}"; do
    if echo "$msg" | grep -qE "$pattern"; then
      return 0  # known
    fi
  done
  return 1  # unknown
}

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

  # 解析 schema 中的 flag 集合 (过滤全局继承 flag)
  help_flags=$(python3 -c "
import json
data = json.load(open('$schema_file'))
print('\n'.join(f['name'] for f in data['flags']))
" | grep -vE "$GLOBAL_FLAGS_RE" | sort -u || true)

  # 过滤 doc_flags 中的全局 flag
  doc_flags=$(echo "$doc_flags" | grep -vE "$GLOBAL_FLAGS_RE" || true)

  # 反向检查：--help 中的 flag 是否在 .md 对应 section 中存在
  for flag in $help_flags; do
    if ! echo "$doc_flags" | grep -qx "$flag"; then
      msg="DRIFT: 'ralph $cmd' has --$flag in --help, but not documented in ${mapping%|*} (section: $section_name)"
      DRIFT_LINES+=("$msg")
      if ! is_known_drift "$msg"; then
      ERRORS=$((ERRORS + 1))
      fi
    fi
  done

  # 正向检查：.md 对应 section 中提到的 flag 是否在 --help 存在
  for flag in $doc_flags; do
    if ! echo "$help_flags" | grep -qx "$flag"; then
      msg="DRIFT: ${mapping%|*} (section: $section_name) mentions --$flag, but 'ralph $cmd --help' no longer has it"
      DRIFT_LINES+=("$msg")
      if ! is_known_drift "$msg"; then
      ERRORS=$((ERRORS + 1))
      fi
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
