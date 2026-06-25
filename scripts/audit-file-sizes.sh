#!/bin/bash
set -e

echo "=== File Size Audit ==="
echo ""

echo "--- loop_runner/ ---"
wc -l crates/ralph-cli/src/loop_runner/mod.rs crates/ralph-cli/src/loop_runner/*.rs crates/ralph-cli/src/loop_runner/**/*.rs 2>/dev/null

echo ""
echo "--- config/ ---"
wc -l crates/ralph-core/src/config/mod.rs crates/ralph-core/src/config/*.rs 2>/dev/null

echo ""
echo "--- event_loop/tests/ ---"
wc -l crates/ralph-core/src/event_loop/tests/mod.rs crates/ralph-core/src/event_loop/tests/*.rs 2>/dev/null

echo ""
echo "--- event_loop/*.rs (top-level submodules) ---"
# 2026-06-10-003 U1 scaffold: covers mod.rs + 10 target submodules + audit + termination
# + 6 follow-up placeholders + 3 R1 red-line modules (loop_state / rejection / review_step_state).
wc -l crates/ralph-core/src/event_loop/mod.rs crates/ralph-core/src/event_loop/*.rs 2>/dev/null

echo ""
echo "--- main.rs + cli/ + commands/ ---"
wc -l crates/ralph-cli/src/main.rs crates/ralph-cli/src/cli/*.rs crates/ralph-cli/src/commands/*.rs 2>/dev/null

echo ""
echo "=== Deleted Files Check ==="
for f in crates/ralph-cli/src/loop_runner.rs crates/ralph-core/src/config.rs crates/ralph-core/src/event_loop/tests.rs; do
  if [ -f "$f" ]; then
    echo "FAIL: $f still exists"
    exit 1
  else
    echo "PASS: $f deleted"
  fi
done

echo ""
echo "=== Audit Complete ==="
