#!/bin/bash
# =============================================================================
# validate-preset-authoring.sh — Preset Authoring Maintenance Guard
# =============================================================================
# This script validates the builtin preset maintenance chain:
#   - Template catalog renderability
#   - Public builtin preset parseability
#   - Manifest vs PRESETS alignment
#   - Index.json alignment with zsh completion
#
# Exit codes:
#   0 = all checks pass
#   1 = one or more checks failed
#
# Note: This is a developer script, not integrated into CI by default.
# =============================================================================

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Counters
PASSED=0
FAILED=0

# ---------------------------------------------------------------------
# Helper functions
# ---------------------------------------------------------------------
info() {
    echo -e "${YELLOW}[INFO]${NC} $1"
}

pass() {
    echo -e "${GREEN}[PASS]${NC} $1"
    PASSED=$((PASSED + 1))
}

fail() {
    echo -e "${RED}[FAIL]${NC} $1"
    FAILED=$((FAILED + 1))
}

header() {
    echo ""
    echo "=== $1 ==="
}

# ---------------------------------------------------------------------
# Find repo root (parent of scripts/)
# ---------------------------------------------------------------------
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------
# 1. Template catalog render tests
# ---------------------------------------------------------------------
header "Template Catalog Render Tests"

# Templates with explicit render tests
# Note: not all templates have individual render tests; the catalog tests
# verify template names and manifest completeness.
TEMPLATE_RENDER_TESTS=(
    "catalog_render_minimal_linear"
)

for test_name in "${TEMPLATE_RENDER_TESTS[@]}"; do
    info "Testing template render: $test_name"
    output=$(cargo test -p ralph-cli --bin ralph -- "$test_name" 2>&1)
    if echo "$output" | grep -E "test.*$test_name.*ok|passed.*0.*failed"; then
        pass "Template test $test_name passed"
    else
        fail "Template test $test_name failed"
        echo "$output" | tail -10
    fi
done

# Verify all templates are in the catalog
info "Verifying all templates are in catalog..."
output=$(cargo test -p ralph-cli --bin ralph -- "catalog_template_names" 2>&1)
if echo "$output" | grep -E "test.*ok|passed.*0.*failed"; then
    pass "All templates are in catalog"
else
    fail "Some templates missing from catalog"
    echo "$output" | tail -10
fi

# ---------------------------------------------------------------------
# 2. All public builtin presets can parse
# ---------------------------------------------------------------------
header "Public Builtin Preset Parse Tests"

info "Running preset parse tests..."
output=$(cargo test -p ralph-cli --bin ralph -- "test_preset_content_is_valid_yaml" 2>&1)
if echo "$output" | grep -E "test.*ok|passed.*0.*failed"; then
    pass "All builtin presets parse as valid YAML"
else
    fail "Some builtin presets failed to parse"
    echo "$output" | tail -10
fi

# ---------------------------------------------------------------------
# 3. Manifest vs PRESETS alignment
# ---------------------------------------------------------------------
header "Manifest vs PRESETS Alignment"

info "Running presets_array_matches_manifest test..."
if cargo test -p ralph-cli --bin ralph -- "presets_array_matches_manifest" 2>&1 | grep -q "test.*ok"; then
    pass "PRESETS array matches manifest.yml"
else
    fail "PRESETS array disagrees with manifest.yml"
fi

# ---------------------------------------------------------------------
# 4. Public preset names in index.json
# ---------------------------------------------------------------------
header "Index.json Alignment"

INDEX_JSON="$REPO_ROOT/presets/index.json"
MANIFEST_YML="$REPO_ROOT/presets/manifest.yml"

if [[ ! -f "$INDEX_JSON" ]]; then
    fail "presets/index.json not found"
else
    info "Checking public preset names in index.json..."
    # Check that all public presets are in index.json
    # Public presets are those in manifest.yml that are not commented out
    for preset in autoresearch debug; do
        if grep -q "\"name\": \"$preset\"" "$INDEX_JSON"; then
            pass "Public preset '$preset' found in index.json"
        else
            fail "Public preset '$preset' missing from index.json"
        fi
    done

    # Check that hidden preset (merge-loop) is NOT in index.json
    if grep -q "\"name\": \"merge-loop\"" "$INDEX_JSON"; then
        fail "Hidden preset 'merge-loop' should NOT be in index.json"
    else
        pass "Hidden preset 'merge-loop' correctly excluded from index.json"
    fi
fi

# ---------------------------------------------------------------------
# 5. Zsh completion alignment
# ---------------------------------------------------------------------
header "Zsh Completion Alignment"

ZSH_PLUGIN="$REPO_ROOT/scripts/ralph-zsh-plugin.zsh"

if [[ ! -f "$ZSH_PLUGIN" ]]; then
    fail "scripts/ralph-zsh-plugin.zsh not found"
else
    info "Checking zsh completion values..."
    for preset in autoresearch debug; do
        if grep -q "builtin:$preset" "$ZSH_PLUGIN"; then
            pass "Preset '$preset' found in zsh completion"
        else
            fail "Preset '$preset' missing from zsh completion (add 'builtin:$preset' to _RALPH_BUILTIN_HAT_VALUES)"
        fi
    done
fi

# ---------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------
header "Summary"
echo ""
echo -e "Passed: ${GREEN}${PASSED}${NC}"
echo -e "Failed: ${RED}${FAILED}${NC}"
echo ""

if [[ $FAILED -eq 0 ]]; then
    echo -e "${GREEN}All checks passed!${NC}"
    exit 0
else
    echo -e "${RED}Some checks failed. Please fix the issues above.${NC}"
    exit 1
fi
