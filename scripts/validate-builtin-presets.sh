#!/usr/bin/env bash
# validate-builtin-presets.sh — Run ralph preset check on all public builtin presets.
#
# Usage:
#   ./scripts/validate-builtin-presets.sh [--strict]
#
# Exit code:
#   0 — all presets passed
#   1 — at least one preset failed
#
# This script is a development/CI aid for the Runtime Contract consolidation
# plan (U6). It is NOT wired into the default CI gate yet — that decision
# depends on measured runtime cost.

set -euo pipefail

STRICT_FLAG=""
if [[ "${1:-}" == "--strict" ]]; then
    STRICT_FLAG="--strict"
fi

# Discover the ralph binary. Prefer a local build.
RALPH_BIN=""
if [[ -x "./target/debug/ralph" ]]; then
    RALPH_BIN="./target/debug/ralph"
elif command -v ralph &>/dev/null; then
    RALPH_BIN="ralph"
else
    echo "ERROR: ralph binary not found. Build with 'cargo build' first." >&2
    exit 1
fi

# Public builtin presets to check. Keep in sync with
# crates/ralph-cli/src/presets.rs PRESETS where public=true.
PRESETS=(
    "autoresearch"
    "ce-executor"
    "ce-executor-wave"
    "code-assist"
    "debug"
    "merge-loop"
    "pdd-to-code-assist"
    "research"
    "review"
)

FAILED=0
FAILED_PRESETS=()

for preset in "${PRESETS[@]}"; do
    echo -n "Checking builtin:${preset} ... "
    if $RALPH_BIN preset check -H "builtin:${preset}" $STRICT_FLAG --format json 2>/dev/null | jq -e '.passed' >/dev/null 2>&1; then
        echo "PASS"
    else
        echo "FAIL"
        FAILED_PRESETS+=("$preset")
        FAILED=1
    fi
done

echo
if [[ $FAILED -eq 0 ]]; then
    echo "All ${#PRESETS[@]} presets passed."
    exit 0
else
    echo "FAILED: ${#FAILED_PRESETS[@]} preset(s): ${FAILED_PRESETS[*]}"
    exit 1
fi
