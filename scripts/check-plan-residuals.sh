#!/usr/bin/env bash
# check-plan-residuals.sh
#
# Pre-commit gate: refuse to commit any file matching the plan residuals
# pattern defined in CLAUDE.md / AGENTS.md (HARD RULE).
#
# Plan 跑完后的过程产物禁止进 git —— reviewer / executor 应把有用内容合并到
# docs/solutions/ 或 plan 主文件,然后删除本地文件。.gitignore 是兜底,
# 本脚本是硬拦截:即使有人误 `git add -f`,也会在这里 fail-fast。
#
# 匹配规则(必须与 .gitignore 中的 plan residuals 段保持一致):
#   .ralph/review/<any-plan-id>/{residuals*.md, scratch/, draft/}
#
# 用法:
#   ./scripts/check-plan-residuals.sh          # 检查暂存区
#   ./scripts/check-plan-residuals.sh --strict # 检查工作区全部未跟踪文件 + 暂存区
#
# 退出码:
#   0 — 干净
#   1 — 发现 plan residuals(输出文件名清单)

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$REPO_ROOT"

# Hooks inherit Git env vars (GIT_DIR/GIT_INDEX_FILE/etc.).
# Unset them so nested `git` calls run against this repo.
while IFS= read -r git_env_var; do
  unset "$git_env_var"
done < <(git rev-parse --local-env-vars 2>/dev/null || true)

strict=0
if [[ "${1:-}" == "--strict" ]]; then
  strict=1
fi

# 模式必须与 .gitignore 中 plan residuals 段保持一致。
patterns=(
  '\.ralph/review/.*/residuals[^/]*\.md$'
  '\.ralph/review/.*/scratch/.*'
  '\.ralph/review/.*/draft/.*'
)

# 构建 git ls-files 参数列表
if [[ "$strict" -eq 1 ]]; then
  # 严格模式:暂存区 + 工作区全部已跟踪 + 未跟踪文件
  files="$( { git diff --cached --name-only --diff-filter=ACMRT; \
              git ls-files --others --exclude-standard; } | sort -u )"
else
  # 默认:仅暂存区(只拦截正在 commit 的文件,不打扰开发中的工作树)
  files="$(git diff --cached --name-only --diff-filter=ACMRT)"
fi

violations="$(printf '%s\n' "$files" | grep -E "$(IFS='|'; echo "${patterns[*]}")" || true)"

if [[ -n "$violations" ]]; then
  cat >&2 <<'EOF'
❌ 检测到 plan 残留文件被 commit 或 add,这是 HARD RULE 违规。

匹配规则(必须与 .gitignore 中 plan residuals 段保持一致):
  .ralph/review/<plan-id>/{residuals*.md, scratch/, draft/}

处理方法:
  1. 把有用内容合并到 docs/solutions/ 或 plan 主文件
  2. 删除本地文件:    git rm <file>
  3. 重新 stage:       git add <updated-files>
  4. 如果确实需要保留这些文件历史,先开 plan 讨论(.gitignore 是兜底,本 hook
     是硬拦截,不要用 git commit --no-verify 跳过)

违规文件:
EOF
  printf '  %s\n' $violations >&2
  exit 1
fi

exit 0