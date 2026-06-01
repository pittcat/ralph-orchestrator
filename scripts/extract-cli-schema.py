#!/usr/bin/env python3
"""
Parse `ralph <cmd> --help` output (clap format) and emit JSON schema.
Used by check-cli-doc-drift.sh for structured comparison against markdown tables.

v1.1 改进: 支持多行 flag 定义 (clap 格式中 flag 名和描述常在不同行).
"""

import json
import re
import subprocess
import sys


def parse_help(help_text: str) -> list[dict]:
    """Extract flags from clap --help output. Each entry: {name, short, type, required}.

    clap 输出格式 (v3+):
        -j, --json                    <- flag 行 (可能含 -X 短名 和 <VALUE> 值名)
            Parse payload as JSON    <- 描述行 (缩进更深)
            object instead of string
        --color <COLOR>               <- 也可能描述同行
            color mode
    """
    flags = []
    lines = help_text.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        # 匹配 flag 行: "  -j, --json" 或 "      --color <COLOR>" 或 "  -v, --verbose"
        m = re.match(
            r"^\s+(?P<short>-[a-zA-Z],\s+)?--(?P<name>[a-zA-Z0-9-]+)"
            r"(?P<value>\s+<[A-Z_]+>)?\s*$",
            line,
        )
        if m:
            short = (m.group("short") or "").strip().rstrip(",")
            takes_value = bool(m.group("value"))
            # 描述可能在下一行 (clap 新格式) 或同行
            description = ""
            j = i + 1
            if j < len(lines):
                next_line = lines[j]
                # 描述行有更深缩进且不是新 flag
                if (next_line.strip() and
                    not re.match(r"^\s+-{1,2}\w", next_line) and
                    len(next_line) - len(next_line.lstrip()) >
                    len(line) - len(line.lstrip())):
                    description = next_line.strip()
            flags.append({
                "name": m.group("name"),
                "short": short,
                "takes_value": takes_value,
                "description": description,
            })
        i += 1
    return flags


def main():
    cmd = sys.argv[1:]
    if not cmd:
        print("usage: extract-cli-schema.py <ralph-cmd-args...>", file=sys.stderr)
        sys.exit(2)
    result = subprocess.run(
        ["ralph", *cmd, "--help"],
        capture_output=True, text=True, check=False,
    )
    if result.returncode != 0:
        print(f"ERROR: {' '.join(cmd)} --help failed: {result.stderr}", file=sys.stderr)
        sys.exit(3)
    schema = parse_help(result.stdout)
    print(json.dumps({"command": " ".join(cmd), "flags": schema}, indent=2))


if __name__ == "__main__":
    main()
