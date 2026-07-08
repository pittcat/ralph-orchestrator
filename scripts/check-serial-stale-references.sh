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
  README.md
  CONCEPTS.md
  CHANGELOG.md
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
  # Generic boundary mechanism comments must not bind serial preset names.
  crates/ralph-core/src/config/loop_config.rs
  # Event-loop test modules must not be named after the dropped serial preset.
  crates/ralph-core/src/event_loop/tests
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
      # Inline allowlist: a hit is OK if its surrounding context
      # explicitly flags the reference as historical / removed /
      # retired. Markers checked against the hit line:
      #   * `Historical Note` / `historical reference` / `historical regression`
      #   * `removed preset` / `retired as a public builtin`
      #   * `retired / removed in` / `since been retired`
      #   * `historical fixture` / `historical incident`
      # The same marker must be within 30 lines above the hit so
      # blockquotes (`> Historical Note …`) still cover the body that
      # follows. The audit list of markers lives in the script so it
      # is explicit and reviewable.
      allow=0
      # `phase_authority` is a generic workflow-guard mechanism
      # (plan 2026-07-02-006) and not bound to any specific
      # preset. References to the typed field name, the
      # `mechanism.phase_authority` config key, or the
      # `event_loop::phase_authority` module path are allowed
      # unconditionally; they describe the mechanism itself,
      # not the retired serial preset.
      if echo "$rest" | grep -qE 'phase_authority( |\.|\:|\)|`|;|,|$)|mechanism\.phase_authority|event_loop::phase_authority'; then
        allow=1
        echo "ALLOWED (generic mechanism): $hit"
        continue
      fi
      # `progress-steward` references are allowed when the
      # surrounding context describes it as the opt-in
      # stall-diagnostic hat (master switch off by default,
      # non-pipeline, supervisor-only). This is the
      # post-007 generic-mechanism framing. The lookahead
      # looks 30 lines above the hit for the
      # `opt-in (stall-diagnostic|stall diagnostic|stall-detector)`
      # framing so the test module-level Historical
      # Reference banner covers the body that follows.
      if [ "$allow" -eq 0 ]; then
        if echo "$rest" | grep -qiE 'opt-in (stall-diagnostic|stall diagnostic|stall-detector|steward)|opt-in `progress-steward`|opt-in \*progress-steward\*'; then
          allow=1
        fi
      fi
      if [ "$allow" -eq 0 ]; then
        line_no="${rest%%:*}"
        start=$((line_no - 30))
        [ "$start" -lt 1 ] && start=1
        if sed -n "${start},${line_no}p" "$file" 2>/dev/null | grep -qiE 'opt-in (stall-diagnostic|stall diagnostic|stall-detector)'; then
          allow=1
        fi
      fi
      if [ "$allow" -eq 1 ]; then
        echo "ALLOWED (opt-in diagnostic): $hit"
        continue
      fi
      # YAML test fixtures that mirror the `progress-steward`
      # default hat id are allowed (master switch off by
      # default; the hat id is part of the generic
      # opt-in stall-diagnostic mechanism, not the retired
      # serial preset). Detection: a YAML string
      # `"progress-steward"` or a yaml-key `progress-steward:`
      # immediately preceded (within 30 lines) by a comment
      # that frames the fixture as opt-in / diagnostic.
      if [ "$allow" -eq 0 ]; then
        line_no="${rest%%:*}"
        start=$((line_no - 30))
        [ "$start" -lt 1 ] && start=1
        end=$((line_no + 1))
        if echo "$rest" | grep -qE '^\s*(steward_hat_id|"progress-steward"|progress-steward:)' && \
           sed -n "${start},${end}p" "$file" 2>/dev/null | grep -qiE 'opt-in|stall-diagnostic|stall diagnostic|stall-detector|generic'; then
          allow=1
        fi
      fi
      # File-level Historical reference banner at the top of
      # the file: any YAML fixture literal inside the file is
      # allowed because the banner already frames the file as
      # historical opt-in / diagnostic coverage.
      if [ "$allow" -eq 0 ]; then
        if head -50 "$file" 2>/dev/null | grep -qE '^//! Historical reference:|^//! Historical note:'; then
          if echo "$rest" | grep -qE '"progress-steward"|^\s*progress-steward:|^\s*steward_hat_id'; then
            allow=1
          fi
        fi
      fi
      if echo "$rest" | grep -qiE 'historical (note|reference|regression|fixture|incident|incident|evidence)|historically|removed preset|retired as a public builtin|since been retired|was retired|were retired|the retired|removed in 2026|retired .*as a public builtin'; then
        allow=1
      fi
      # 30-line lookahead for an explicit "Historical Note" header
      # above the hit (covers blockquoted CHANGELOG bodies).
      if [ "$allow" -eq 0 ]; then
        line_no="${rest%%:*}"
        start=$((line_no - 30))
        [ "$start" -lt 1 ] && start=1
        end=$((line_no + 1))
        if sed -n "${start},${end}p" "$file" 2>/dev/null | grep -qE '^>.*Historical Note|^### Historical Note|^## Historical Note|Historical Note \(|^//! Historical reference|^//! Historical note|^//! Historical regression|^// Historical reference|^// Historical note|^// Historical regression|^// historical reference|^// historical note|^// historical regression|^// Historical evidence|historical evidence|Historical note:'; then
          allow=1
        fi
      fi
      # References to historical plan paths under
      # `docs/plans/2026-06-23-005-fix-ce-executor-serial-*` and
      # similar pre-retirement plans: the referenced path lives in
      # the `EXCLUDE_REGEX` historical bucket, so a code comment
      # pointing at it is also historical.
      if [ "$allow" -eq 0 ]; then
        if echo "$rest" | grep -qE 'docs/(plans|report)/[0-9]{4}-[0-9]{2}-[0-9]{2}.*(ce-executor-serial|progress-steward|shipper|phase_authority)'; then
          allow=1
        fi
      fi
      if [ "$allow" -eq 1 ]; then
        echo "ALLOWED (historical): $hit"
        continue
      fi
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
