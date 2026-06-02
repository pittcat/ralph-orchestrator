#!/usr/bin/env bash
# scripts/test-cli-doc-drift.sh
# 特征化测试 wrapper for scripts/test_cli_doc_drift.py
# Plan 2026-06-02-001 Unit 1.
#
# 用法:
#   bash scripts/test-cli-doc-drift.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
python3 "$SCRIPT_DIR/test_cli_doc_drift.py"
