#!/usr/bin/env bash
# 2026-06-18-001 plan U8: 调用 `ralph audit hat-handoff` 的 bash 薄包装。
#
# 核心审计逻辑全部在 Rust 中实现,避免在 bash 中解析 YAML。
#
# 用法: scripts/audit-hat-handoff-artifacts.sh [WORKSPACE]
#   WORKSPACE: 默认为当前目录

set -euo pipefail

WORKSPACE="${1:-.}"

if [[ -z "${RALPH_BIN:-}" ]]; then
    # 默认走 cargo run;CI 环境可预设 RALPH_BIN 指向已编译的二进制
    RALPH_BIN="cargo run -p ralph-cli --quiet --"
fi

# shellcheck disable=SC2086
exec $RALPH_BIN audit hat-handoff --workspace "$WORKSPACE" --format text
