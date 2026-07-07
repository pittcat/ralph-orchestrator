#!/usr/bin/env bash
# scripts/check-serial-stale-references.sh
#
# Plan 2026-07-07-006 Unit 5 — anti-regression lock for the
# single-chain refactor. After pipeline became the Ralph primary path,
# no active (user-facing or agent-injected) document should still
# recommend `ce-executor-serial`, the dropped shipper handoff, or the
# removed `progress-steward` hat.
#
# The script scans an "include" set (active agent docs, handbook and
# reference docs, CLI surfaces, SOPs, cursor rules, and the source files
# that still carry user-facing guidance) and an "exclude" set (history
# kept on disk for auditability: `docs/report/`, `docs/brainstorms/`,
# old `docs/plans/`, `skills/` which is Unit 6's surface).
#
# Usage:
#   bash scripts/check-serial-stale-references.sh
#
# Exit codes:
#   0 = no stale references in active surfaces
#   1 = at least one STALE hit
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# forbidden substrings — kept short so historical code markers like
# `mechanism.phase_authority` are also caught via secondary regex.
FORBIDDEN_REGEX='ce-executor-serial|progress-steward|shipper reason|phase_authority|strict_reason_routing|review_complete_misrouted'

# active surfaces: anything a user or agent reads at runtime
INCLUDE_PATHS=(
  AGENTS.md
  CLAUDE.md
  .cursor/rules
  docs/handbook
  docs/reference
  crates/ralph-core/data
  crates/ralph-cli/sops
  crates/ralph-cli/src/commands/init.rs
  crates/ralph-cli/src/commands/tutorial.rs
  crates/ralph-cli/src/config_resolution.rs
  crates/ralph-cli/src/preflight.rs
  crates/ralph-cli/src/policy_check.rs
  crates/ralph-cli/src/loop_runner/execution.rs
  crates/ralph-cli/src/loop_runner/wave/io.rs
  crates/ralph-cli/src/wave.rs
)

# historical surfaces — kept verbatim for audit and explicitly
# excluded from this gate. The plan allows historical docs to retain
# serial references; they are listed in PR body as "保留为历史".
EXCLUDE_REGEX='^(docs/(report|brainstorms|plans)/|skills/)'

# fetch_file_relpath
# $1 = path returned by rg (relative to repo root)
# echoes "include" or "exclude"
classify_path() {
  local p="$1"
  if echo "$p" | grep -qE "$EXCLUDE_REGEX"; then
    echo "exclude"
  else
    echo "include"
  fi
}

STALE=0
while IFS= read -r hit; do
  [ -z "$hit" ] && continue
  file="${hit%%:*}"
  rest="${hit#*:}"
  rel="${file#./}"
  case "$(classify_path "$rel")" in
    exclude) ;;
    include)
      echo "STALE: $hit"
      STALE=$((STALE + 1))
      ;;
  esac
done < <(rg -n --no-heading "$FORBIDDEN_REGEX" "${INCLUDE_PATHS[@]}" 2>/dev/null || true)

if [ "$STALE" -gt 0 ]; then
  echo "" >&2
  echo "Found $STALE stale serial-only reference(s) in active surfaces." >&2
  echo "Either delete / rewrite or document why the hit is allowed (PR body)." >&2
  exit 1
fi

echo "No stale serial-only references in active surfaces."
exit 0
