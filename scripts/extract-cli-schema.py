#!/usr/bin/env python3
"""
Parse `ralph <cmd> --help` output (clap format) and emit JSON schema.
Used by check-cli-doc-drift.sh for structured comparison against markdown tables.

v1.2 改进:
  - parse_help 抽为可独立 import 的纯函数, 接受 help 文本
  - 支持从 stdin 读取 help 文本 (便于 fixture 测试)
  - 支持 --variadic `<VALUE>...` 形态 (clap 变长参数)
  - 支持位置参数 (<TOPIC> / [PAYLOAD] 等)

用法:
  python3 extract-cli-schema.py <ralph-cmd-args...>
  echo "$HELP_TEXT" | python3 extract-cli-schema.py --stdin <cmd-name>
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from typing import Any


# 匹配 flag 行, 支持变长参数 (`<VALUE>...`).
# 形态:
#   -j, --json
#   --color <COLOR>
#   --payloads <PAYLOADS>...
#   -v, --verbose
FLAG_LINE_RE = re.compile(
    r"^\s+(?P<short>-[a-zA-Z],\s+)?--(?P<name>[a-zA-Z0-9-]+)"
    r"(?P<value>\s+<[A-Z_]+>(?:\.\.\.)?)?"  # 支持变长参数 <VALUE>...
    r"\s*$"
)

# 位置参数行 (e.g. `<TOPIC>` / `[PAYLOAD]`)
POSITIONAL_LINE_RE = re.compile(
    r"^\s+(?P<name><[A-Z_]+>|\[[A-Z_]+\])\s*$"
)


def parse_help(help_text: str) -> dict[str, list[dict[str, Any]]]:
    """Extract flags and positionals from clap --help output.

    Returns {"flags": [...], "positionals": [...]}.

    clap 输出格式 (v3+):
        -j, --json                    <- flag 行 (可能含 -X 短名 和 <VALUE> 值名)
            Parse payload as JSON    <- 描述行 (缩进更深)
            object instead of string
        --color <COLOR>               <- 也可能描述同行
            color mode
        --payloads <PAYLOADS>...      <- 变长参数
        <TOPIC>                       <- 位置参数 (必需)
        [PAYLOAD]                     <- 位置参数 (可选)
    """
    flags: list[dict[str, Any]] = []
    positionals: list[dict[str, Any]] = []
    lines = help_text.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        if not stripped:
            i += 1
            continue

        m_flag = FLAG_LINE_RE.match(line)
        m_positional = POSITIONAL_LINE_RE.match(line) if not m_flag else None

        if m_flag:
            short = (m_flag.group("short") or "").strip().rstrip(",")
            value = m_flag.group("value") or ""
            takes_value = bool(value)
            # 变长参数: <VALUE>...
            variadic = takes_value and value.strip().rstrip(">").endswith("...")
            description = ""
            j = i + 1
            if j < len(lines):
                next_line = lines[j]
                if (next_line.strip()
                    and not re.match(r"^\s+-{1,2}\w", next_line)
                    and not POSITIONAL_LINE_RE.match(next_line)
                    and len(next_line) - len(next_line.lstrip()) >
                    len(line) - len(line.lstrip())):
                    description = next_line.strip()
            flags.append({
                "name": m_flag.group("name"),
                "short": short,
                "takes_value": takes_value,
                "variadic": variadic,
                "required": False,
                "description": description,
            })
            i += 1
        elif m_positional:
            raw = m_positional.group("name")
            # 必需: <NAME>; 可选: [NAME]
            required = raw.startswith("<")
            inner = raw.strip("<>[]")
            description = ""
            j = i + 1
            if j < len(lines):
                next_line = lines[j]
                if (next_line.strip()
                    and not re.match(r"^\s+-{1,2}\w", next_line)
                    and not POSITIONAL_LINE_RE.match(next_line)
                    and len(next_line) - len(next_line.lstrip()) >
                    len(line) - len(line.lstrip())):
                    description = next_line.strip()
            positionals.append({
                "name": inner.lower(),
                "required": required,
                "description": description,
            })
            i += 1
        else:
            i += 1
    return {"flags": flags, "positionals": positionals}


def schema_from_help(command: str, help_text: str) -> dict[str, Any]:
    """Build a schema dict from a command path and its --help text."""
    return {
        "command": command,
        **parse_help(help_text),
    }


def main() -> int:
    args = sys.argv[1:]

    # stdin 模式: 从 stdin 读取 help 文本, 第一个位置参数是命令名
    if not args or args[0] == "--stdin":
        if args and args[0] == "--stdin":
            cmd_args = args[1:]
        else:
            cmd_args = []
        if not cmd_args:
            print("usage: extract-cli-schema.py --stdin <cmd-name>", file=sys.stderr)
            return 2
        command = " ".join(cmd_args)
        help_text = sys.stdin.read()
        schema = schema_from_help(command, help_text)
        print(json.dumps(schema, indent=2, ensure_ascii=False))
        return 0

    # 正常模式: 调用 ralph <args> --help
    result = subprocess.run(
        ["ralph", *args, "--help"],
        capture_output=True, text=True, check=False,
    )
    if result.returncode != 0:
        print(f"ERROR: ralph {' '.join(args)} --help failed: {result.stderr}",
              file=sys.stderr)
        return 3
    schema = schema_from_help(" ".join(args), result.stdout)
    print(json.dumps(schema, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
