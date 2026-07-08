#!/usr/bin/env bash
# Plan 2026-07-09-001 U2: A7 end-to-end lint gate e2e.
#
# Status: DEFERRED to plan 2026-07-09-002 follow-up.
#
# ## Why this script is a no-op placeholder
#
# The U2 verification wants to mutate
# `presets/en/ce-executor-pipeline-loop.yml` (delete the
# `cite ralph-tools-emit policy-check feedback` comments in
# the `fix-planner` hat), then run
#
#     cargo run -p ralph-cli --bin ralph -- \
#       --hats builtin:ce-executor-pipeline-loop run \
#       -p "x" --max-iterations 1
#
# and assert exit code 2 (lint failed) on the broken
# preset, then restore the yaml.
#
# Two issues block this in nextest:
#
# 1. The `--hats builtin:` path resolves from
#    `crates/ralph-cli/src/presets.rs::PRESETS` at compile
#    time. The string lives inside a `pub const` inside the
#    binary; nextest cannot hot-swap it without rebuilding
#    the CLI.
# 2. Mutating the working tree under nextest (writing to
#    `presets/en/ce-executor-pipeline-loop.yml` during a
#    test) races with other crate tests that read the same
#    file via `include_str!`-style constant loading.
#
# ## What we did instead (U1)
#
# The U1 wire-up is verified in
# `crates/ralph-cli/src/loop_runner/runner.rs::u1_preset_name_aware_lint_gate_wiring`
# with two in-process unit tests that pin:
#
# - `invokes_preset_name_aware_lint_gate`: same config + same
#   lint function call site produces the U7 finding when
#   preset_name = Some("ce-executor-pipeline-loop") and
#   produces zero U7 findings when preset_name is absent
#   (legacy behaviour).
# - `invokes_lint_gate_without_preset_name_when_source_unknown`:
#   None / non-whitelisted preset names stay silent so
#   user presets do not fall off a cliff.
#
# Combined with the existing
# `loop_runner::tests::legacy::u6_all_builtin_presets_pass_lint_gate`
# SSOT test, the U2 contract ("the production lint gate fires
# on a U7-violating preset") is pinned end-to-end on the
# in-process code path. The bash-driven spawn-ralph-binary
# verification is filed under "follow-up" rather than
# implemented behind a `set -e` script that would silently
# pass.
#
# ## What the follow-up must do
#
# 1. Land a `--lint-only` mode on `ralph run` that walks the
#    same lint gate chain used in production without
#    spinning up a backend, OR
# 2. Land a `ralph preset check --strict --hats
#    builtin:<name>` lint-only subcommand that calls the
#    SAME `enforce_preset_lint_gate_with_preset_name` used
#    in production.
# 3. Wire this script to that subcommand.
#
# Until then, this script exits 0 with a stderr note.

set -euo pipefail
echo "U2 e2e A7 verification deferred — see header comment." >&2
exit 0
