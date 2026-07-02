#!/usr/bin/env bash
# validate-builtin-presets.sh — Run ralph preset check on all public builtin presets.
#
# Usage:
#   ./scripts/validate-builtin-presets.sh [--strict]
#
# Exit code:
#   0 — all public presets passed (or had known topology exemptions in non-strict mode)
#   1 — at least one public preset failed
#
# This script is a development/CI aid for the Runtime Contract consolidation
# plan (U6). It is NOT wired into the default CI gate yet — that decision
# depends on measured runtime cost.
#
# The PRESETS list is derived from presets/index.json (single source of truth
# for user-facing builtin presets; build.rs and presets.rs keep the embedded
# array in lockstep with the manifest). The TOPOLOGY_EXEMPT_PRESETS list
# below mirrors the same list in
# crates/ralph-cli/src/presets.rs::tests::test_all_public_presets_pass_authoring_contract.
# If you add or remove a public builtin preset, edit presets/index.json (and
# the Rust PRESETS array if the preset is also embedded). If you add a new
# known topology exception, update both lists.

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

# Derive the public builtin preset list from presets/index.json. This is the
# single source of truth for user-facing presets (the Rust `PRESETS` array
# mirrors it via the same manifest). Reading from the manifest here means
# adding or removing a preset to one place automatically updates this script
# — no risk of silent drift.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
PRESET_INDEX="${REPO_ROOT}/presets/index.json"

if [[ ! -f "$PRESET_INDEX" ]]; then
    echo "ERROR: presets/index.json not found at $PRESET_INDEX" >&2
    exit 1
fi

# Use `map(select(.name))` to guard against stray entries without a name,
# then sort for stable ordering across runs and platforms.
PRESETS=()
while IFS= read -r name; do
    PRESETS+=("$name")
done < <(jq -r '[.[] | select(.name) | .name] | sort | .[]' "$PRESET_INDEX")

if [[ ${#PRESETS[@]} -eq 0 ]]; then
    echo "ERROR: presets/index.json contains no entries with a .name field" >&2
    exit 1
fi

# Presets with known topology issues (required events not on all completion
# paths). These are documented exceptions for non-strict mode only. Mirrors
# the `topology_exempt` list in
# crates/ralph-cli/src/presets.rs::tests::test_all_public_presets_pass_authoring_contract.
# Add to this list only with a comment explaining why.
#
# Strict mode policy: in --strict mode the exemption is intentionally NOT
# applied. Strict means fail_on_warnings=true AND payload_strict=true, so any
# non-topology warning (or new payload/orphan/config warning) must cause
# failure. A preset that is "known topology" but suddenly gains a payload
# warning must fail strict, not silently slip through. This is the regression
# guard that the original review flagged.
TOPOLOGY_EXEMPT_PRESETS=(
    # autoresearch: experiment loop has branching completion paths where
    # required events (experiment.scored, experiment.evaluated) are not on
    # every path — by design for the try/measure/keep/discard flow.
    "autoresearch"
    # debug: debug loop has branching paths where required events
    # (hypothesis.confirmed, fix.applied, fix.verified) are not on every
    # completion path — by design for the hypothesis/fix/verify flow.
    "debug"
    # ce-executor-pipeline: 2026-07-02-003 plan U1 (R3). The 13-hat flat
    # single-consumer chain is intentionally long; the first dimension
    # hat `dim:goal-alignment` is 9 hops from the terminal `report.done`
    # and exceeds the WAC EGRESS_MAX_HOPS=8 limit
    # (`crates/ralph-core/src/preset_lint/workflow_activation.rs:364`),
    # tripping `activation_egress_missing` by 1 hop. This is a known
    # false positive of the static-lint BFS bound — the chain
    # terminates deterministically via `report.done` (required_events)
    # and `LOOP_COMPLETE` (completion_promise). Topology is structurally
    # valid; the EGRESS finding is a known bound artifact.
    "ce-executor-pipeline"
)

# WRC-U5 (2026-06-12-003) / KTD-WRC-5: Tier-0 list of builtin
# presets for which WAC (Workflow Activation Contract) findings
# MUST be zero in strict mode. Mirrors
# `crates/ralph-cli/src/presets.rs::TIER_0_WAC_PRESETS` — the
# script intentionally duplicates the list (no in-process ralph
# binary is available to query from within this script) so the
# two stay in lockstep by convention. When you promote a preset
# in either place, update both.
TIER_0_WAC_PRESETS=(
    "ce-executor-serial"
)

# Returns 0 (true) if every error-severity finding in $1 (a JSON report) has
# source == "topology", or if there are no error-severity findings at all.
# Returns 1 (false) otherwise. The exemption is only meaningful for this
# shape; warning checks live in the strict gate below.
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

# Returns 0 (true) if $1 (a JSON report) has zero warning-severity findings.
# In --strict mode fail_on_warnings=true, so any warning must fail the gate.
# This is the regression guard for the original review: the prior version
# only checked errors, so a new payload/orphan/config warning could pass
# strict as long as the topology errors remained.
no_warnings() {
    local report_json="$1"
    local result
    result=$(echo "$report_json" | jq -r '
        [ (.findings // [])[] | select(.severity == "warn") ] | length == 0
    ') || return 1
    [[ "$result" == "true" ]]
}

# WRC-U5: returns 0 (true) when $1 (a JSON report) contains
# zero WAC findings of error severity. The check is
# `lint.preset.*` and source=lint. A non-zero count means
# the preset has at least one WAC defect that the strict
# gate should fail on.
no_wac_errors() {
    local report_json="$1"
    local result
    result=$(echo "$report_json" | jq -r '
        [ (.findings // [])[]
          | select(.severity == "error")
          | select(.source == "lint")
          | select(.id | startswith("lint.preset."))
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

# WRC-U5: returns 0 (true) if $1 is a member of the Tier-0 WAC list.
is_tier_0() {
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
        # WRC-U5: even on the fast path, a Tier-0 preset must
        # have zero WAC findings. A preset that passed only
        # because it was not in --strict mode (e.g. a manual
        # invoke) still has the WAC info; we re-check here so
        # the script's behaviour matches the documented Tier-0
        # contract regardless of the caller's --strict choice.
        if is_tier_0 "$preset" "${TIER_0_WAC_PRESETS[@]}"; then
            if no_wac_errors "$report_json"; then
                echo "PASS"
            else
                wac_ids=$(echo "$report_json" | jq -r '
                    [(.findings // [])[]
                     | select(.severity == "error")
                     | select(.source == "lint")
                     | select(.id | startswith("lint.preset."))
                     | .id]
                     | unique
                     | join(", ")
                ')
                echo "FAIL (tier-0 wac: ${wac_ids})"
                FAILED_PRESETS+=("$preset")
                FAILED=1
            fi
        else
            echo "PASS"
        fi
        continue
    fi

    # Slow path: report failed.
    #
    # Strict-mode gate (regression guard):
    #   In --strict mode we deliberately do NOT apply the topology exemption
    #   and we additionally verify there are zero warnings. The previous
    #   version only checked `all_errors_are_topology`, which meant a new
    #   payload/orphan/config warning (severity=warn) could pass the gate
    #   when fail_on_warnings=true should have caught it. Strict means
    #   strict — every warning or non-topology error is a real failure.
    if [[ -n "$STRICT_FLAG" ]]; then
        if no_warnings "$report_json"; then
            # strict run with no warnings and only topology errors is
            # surfaced verbatim — we do NOT exempt strict runs. The
            # author must fix the topology issue for strict to pass.
            error_ids=$(echo "$report_json" | jq -r '
                [(.findings // [])[]
                 | select(.severity == "error")
                 | .id]
                 | unique
                 | join(", ")
            ')
            echo "FAIL (strict: ${error_ids})"
        else
            warn_ids=$(echo "$report_json" | jq -r '
                [(.findings // [])[]
                 | select(.severity == "warn")
                 | .id]
                 | unique
                 | join(", ")
            ')
            echo "FAIL (strict warnings: ${warn_ids})"
        fi
        FAILED_PRESETS+=("$preset")
        FAILED=1
        continue
    fi

    # Non-strict slow path: topology exemption is allowed ONLY when every
    # error-severity finding is from the topology source. This intentionally
    # does not look at warnings, because non-strict mode treats warnings as
    # non-blocking. If a future contributor wants the exemption to also
    # cover warnings, add a separate gate with an explicit comment.
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
