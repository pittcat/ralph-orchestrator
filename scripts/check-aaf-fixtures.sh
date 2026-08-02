#!/usr/bin/env bash
# scripts/check-aaf-fixtures.sh
#
# Plan 2026-07-07-006 Unit 6 — fixture acceptance for single-chain-first
# preset review audit. Each fixture must round-trip through
# `ralph preset check` so future contributors cannot silently delete or
# rewrite the fixture. The two anti-pattern findings
# (`fallback_reaches_success_terminal` / `runtime_unit_loop_multiple_fact_sources`)
# are *soft* AAF findings surfaced by the `ralph-preset-review` skill,
# not by mechanical lint — full review still runs through that skill.
#
# What this script guarantees:
#   1. Both fixture files exist on disk.
#   2. `ralph preset check -H <fixture>` parses each YAML without crashing.
#   3. Each fixture produces at least one finding (a fixture that
#      passes mechanical lint is a regression — the anti-pattern must
#      remain detectable).
#
# Usage:
#   bash scripts/check-aaf-fixtures.sh
#
# Exit codes:
#   0 = both fixtures parsed and surfaced ≥1 finding
#   1 = at least one fixture missing, unparsable, or produced 0 findings
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

if ! command -v ralph >/dev/null 2>&1; then
  echo "ERROR: ralph not found. Build first: cargo build -p ralph-cli" >&2
  exit 2
fi

FALLBACK_FIXTURE="$REPO_ROOT/skills/ralph-preset-review/fixtures/aaf-fallback-success-terminal.yml"
UNIT_LOOP_FIXTURE="$REPO_ROOT/skills/ralph-preset-review/fixtures/aaf-runtime-unit-loop.yml"

run_fixture_check() {
  local fixture="$1"
  local label="$2"

  echo "→ $label: $fixture"
  if [ ! -f "$fixture" ]; then
    echo "  FAIL: fixture file missing"
    return 1
  fi

  local out
  # `ralph preset check` exits non-zero when findings exist; capture stdout.
  out="$(ralph preset check -H "$fixture" --format json 2>&1)" || true

  if [ -z "$out" ]; then
    echo "  FAIL: ralph preset check produced no output"
    return 1
  fi

  # Count finding objects in the JSON output. A fixture with zero findings
  # means the anti-pattern was lost (lint passed), which is a regression.
  local findings
  findings="$(echo "$out" | python3 -c "import json, sys
try:
    data = json.load(sys.stdin)
    findings = data.get('findings') or []
    print(len(findings))
except Exception:
    print(0)
")"

  if [ "${findings:-0}" -lt 1 ]; then
    echo "  FAIL: fixture parsed cleanly (0 findings); the anti-pattern must remain detectable"
    return 1
  fi

  echo "  OK: $findings finding(s) surfaced (anti-pattern still detectable)"
  return 0
}

FAILS=0
run_fixture_check "$FALLBACK_FIXTURE" "fallback-success fixture" || FAILS=$((FAILS + 1))
run_fixture_check "$UNIT_LOOP_FIXTURE" "runtime-unit-loop fixture" || FAILS=$((FAILS + 1))

if [ "$FAILS" -gt 0 ]; then
  echo "" >&2
  echo "AAF fixture acceptance: $FAILS fixture(s) failed" >&2
  echo "Either the fixture file was deleted, the YAML broke, or the anti-pattern was lost." >&2
  exit 1
fi

echo "AAF fixture acceptance: both fixtures parsed and surfaced findings."
exit 0