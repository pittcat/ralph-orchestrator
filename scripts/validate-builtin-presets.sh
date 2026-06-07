#!/usr/bin/env bash
# validate-builtin-presets.sh — Run ralph preset check on all public builtin presets.
#
# Usage:
#   ./scripts/validate-builtin-presets.sh [--strict]
#
# Exit code:
#   0 — all public presets passed (or had known topology exemptions)
#   1 — at least one public preset failed
#
# This script is a development/CI aid for the Runtime Contract consolidation
# plan (U6). It is NOT wired into the default CI gate yet — that decision
# depends on measured runtime cost.
#
# The PRESETS list and TOPOLOGY_EXEMPT_PRESETS list mirror the Rust unit tests
# in crates/ralph-cli/src/presets.rs:
#   - test_all_public_presets_pass_authoring_contract
#   - test_development_presets_pass_strict_contract
# If you add, remove, or rename a public builtin preset, or add a new known
# topology exception, update both the Rust tests and this script.

set -euo pipefail

STRICT_FLAG=""
if [[ "${1:-}" == "--strict" ]]; then
    STRICT_FLAG="--strict"
fi

# Discover the ralph binary. Prefer a local build so we test the current
# source tree (and not, say, an older globally-installed copy that lacks
# the `preset check` subcommand). Log which binary is used so silent
# wrong-binary failures are visible.
RALPH_BIN=""
if [[ -x "./target/debug/ralph" ]]; then
    RALPH_BIN="./target/debug/ralph"
    RALPH_BIN_SOURCE="./target/debug/ralph (local debug build)"
elif [[ -x "./target/release/ralph" ]]; then
    RALPH_BIN="./target/release/ralph"
    RALPH_BIN_SOURCE="./target/release/ralph (local release build)"
elif command -v ralph &>/dev/null; then
    RALPH_BIN="ralph"
    RALPH_BIN_SOURCE="$(command -v ralph) (from PATH — may be stale)"
    echo "WARNING: using ralph from PATH; run 'cargo build' to use a local build." >&2
else
    echo "ERROR: ralph binary not found. Build with 'cargo build' first." >&2
    exit 1
fi
echo "Using ralph: $RALPH_BIN_SOURCE" >&2

# Verify jq is available — we rely on it for all parsing.
if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is required by this script (apt/brew install jq)." >&2
    exit 1
fi

# Public builtin presets to check. Keep in sync with
# crates/ralph-cli/src/presets.rs PRESETS where public=true.
# `merge-loop` is intentionally excluded — it is public: false (internal
# helper for the merge queue, not a user-facing preset).
PRESETS=(
    "autoresearch"
    "ce-executor"
    "ce-executor-wave"
    "code-assist"
    "debug"
    "pdd-to-code-assist"
    "research"
    "review"
)

# Presets with known topology issues (required events not on all completion
# paths). These are documented exceptions, not hidden failures. Mirrors the
# `topology_exempt` list in
# crates/ralph-cli/src/presets.rs::tests::test_all_public_presets_pass_authoring_contract.
# Add to this list only with a comment explaining why.
#
# The exemption applies only when ALL error findings come from the topology
# source; any non-topology error (config/orphan/payload) is still a hard fail.
# This is independent of strictness — the exemption is about known topology
# gaps, not about payload strictness.
TOPOLOGY_EXEMPT_PRESETS=(
    # autoresearch: experiment loop has branching completion paths where
    # required events (experiment.scored, experiment.evaluated) are not on
    # every path — by design for the try/measure/keep/discard flow.
    "autoresearch"
    # debug: debug loop has branching paths where required events
    # (hypothesis.confirmed, fix.applied, fix.verified) are not on every
    # completion path — by design for the hypothesis/fix/verify flow.
    "debug"
)

# Returns 0 (true) if every error-severity finding in $1 (a JSON report) has
# source == "topology", or if there are no error-severity findings at all.
# Returns 1 (false) otherwise.
all_errors_are_topology() {
    local report_json="$1"
    # `defensive` is the jq safe-navigation style: if .findings is missing or
    # null, treat as empty array.
    local result
    result=$(echo "$report_json" | jq -r '
        [ (.findings // [])[]
          | select(.severity == "error")
          | select(.source != "topology")
        ] | length == 0
    ') || return 1
    [[ "$result" == "true" ]]
}

# Returns 0 (true) if $1 is a member of $2.. (treats $2 as a list of names).
is_exempt() {
    local needle="$1"
    shift
    for item in "$@"; do
        [[ "$item" == "$needle" ]] && return 0
    done
    return 1
}

FAILED=0
FAILED_PRESETS=()

for preset in "${PRESETS[@]}"; do
    echo -n "Checking builtin:${preset} ... "

    # Run the check; capture stdout only. Stderr is silenced to keep the
    # PASS/FAIL summary readable, but ralph CLI exits 1 on failure so we
    # need to swallow the non-zero exit without aborting under set -e.
    report_json=""
    if ! report_json=$($RALPH_BIN preset check -H "builtin:${preset}" $STRICT_FLAG --format json 2>/dev/null); then
        : # non-zero exit is expected when report.passed == false; ignore here
    fi

    # Defensive: if the binary produced no JSON, treat as failure.
    if [[ -z "$report_json" ]] || ! echo "$report_json" | jq -e . >/dev/null 2>&1; then
        echo "FAIL (no JSON output from ralph)"
        FAILED_PRESETS+=("$preset")
        FAILED=1
        continue
    fi

    # Fast path: report passed.
    if echo "$report_json" | jq -e '.passed' >/dev/null 2>&1; then
        echo "PASS"
        continue
    fi

    # Slow path: report failed. Check for topology exemption.
    if is_exempt "$preset" "${TOPOLOGY_EXEMPT_PRESETS[@]}" \
        && all_errors_are_topology "$report_json"; then
        error_ids=$(echo "$report_json" | jq -r '
            [(.findings // [])[]
             | select(.severity == "error")
             | .id]
             | unique
             | join(", ")
        ')
        echo "PASS (topology exempt: ${error_ids})"
        continue
    fi

    # Real failure — surface findings.
    echo "FAIL"
    echo "$report_json" | jq -r '
        (.findings // [])[]
        | "    [\(.severity)] \(.source) \(.id): \(.message)"
    ' 2>/dev/null || echo "    (failed to render findings)"
    FAILED_PRESETS+=("$preset")
    FAILED=1
done

echo
if [[ $FAILED -eq 0 ]]; then
    echo "All ${#PRESETS[@]} public builtin presets passed."
    exit 0
else
    echo "FAILED: ${#FAILED_PRESETS[@]} preset(s): ${FAILED_PRESETS[*]}"
    exit 1
fi
