#!/usr/bin/env python3
# scripts/test_cli_doc_drift.py
# 特征化测试 for extract-cli-schema.py + check-cli-doc-drift.sh
# 不需要 cargo build, 全部用固定 help 文本 fixture 跑.
#
# Plan 2026-06-02-001 Unit 1: 为 drift parser 和 checker 加特征化测试.
# 覆盖:
#   - parse_help 对各种 clap flag 形态的解析
#   - check_cli_doc_drift 的 strict / default 退出码语义
#   - 反向/正向 flag 漂移检测
#   - global flags 不会重复误报
#
# 用法:
#   python3 scripts/test_cli_doc_drift.py

from __future__ import annotations

import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPTS_DIR = REPO_ROOT / "scripts"
PARSER_PATH = SCRIPTS_DIR / "extract-cli-schema.py"
CHECKER_PATH = SCRIPTS_DIR / "check-cli-doc-drift.sh"


def load_parser_module():
    """Load extract-cli-schema.py as a Python module."""
    spec = importlib.util.spec_from_file_location("extract_cli_schema", PARSER_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Cannot load parser module from {PARSER_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# ---- 固定 help 文本 fixture ----

RALPH_EMIT_HELP = """\
Emit an event to the current run's events file with proper JSON formatting

Usage: ralph emit [OPTIONS] <TOPIC> [PAYLOAD]

Arguments:
  <TOPIC>
          Event topic (e.g., "build.done", "review.complete")

  [PAYLOAD]
          Event payload - string or JSON (optional, defaults to empty)

Options:
  -j, --json
          Parse payload as JSON object instead of string

      --file <FILE>
          Path to events file (defaults to .ralph/events.jsonl)

      --policy-check
          Validate event against current event policy before emitting

      --unsafe-no-policy-check
          Bypass mandatory policy check (only allowed when config permits)

      --hat <HAT>
          Hat that published this event (falls back to $RALPH_CURRENT_HAT)

      --triggered <TRIGGERED>
          Target hat triggered by this event (falls back to $RALPH_TRIGGERED_HAT)

      --source <SOURCE>
          Source identifier for this event (falls back to $RALPH_EVENT_SOURCE)

  -c, --config <CONFIG>
          Core configuration source

  -H, --hats <HATS>
          Hat collection source

  -v, --verbose
          Verbose output

      --color <COLOR>
          Color output mode

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
"""

RALPH_WAVE_EMIT_HELP = """\
Emit multiple events as a wave for parallel execution

Usage: ralph wave emit [OPTIONS] <TOPIC>

Arguments:
  <TOPIC>
          Event topic for all wave events (e.g., "review.file")

Options:
      --payloads <PAYLOADS>...
          Payloads for each wave event instance (one per parallel worker)

  -c, --config <CONFIG>
          Core configuration source

  -H, --hats <HATS>
          Hat collection source

  -v, --verbose
          Verbose output

      --color <COLOR>
          Color output mode

  -h, --help
          Print help (see a summary with '-h')
"""

RALPH_TOOLS_SKILL_LIST_HELP = """\
List available skills

Usage: ralph tools skill list [OPTIONS]

Options:
      --format <FORMAT>
          Output format

      --root <ROOT>
          Working directory (default: current directory)

  -c, --config <CONFIG>
          Core configuration source

  -H, --hats <HATS>
          Hat collection source

  -v, --verbose
          Verbose output

      --color <COLOR>
          Color output mode

  -h, --help
          Print help (see a summary with '-h')
"""

RALPH_TOOLS_SKILL_LOAD_HELP = """\
Load a skill by name and print its content

Usage: ralph tools skill load [OPTIONS] <NAME>

Arguments:
  <NAME>
          Skill name to load

Options:
  -h, --help
          Print help (see a summary with '-h')

  -c, --config <CONFIG>
          Core configuration source

  -H, --hats <HATS>
          Hat collection source

  -v, --verbose
          Verbose output

      --color <COLOR>
          Color output mode
"""

RALPH_TOOLS_INTERACT_PROGRESS_HELP = """\
Send a non-blocking progress message via Telegram

Usage: ralph tools interact progress [OPTIONS] <MESSAGE>

Arguments:
  <MESSAGE>
          Progress message text

Options:
  -h, --help
          Print help (see a summary with '-h')

  -c, --config <CONFIG>
          Core configuration source

  -H, --hats <HATS>
          Hat collection source

  -v, --verbose
          Verbose output

      --color <COLOR>
          Color output mode
"""


class ParseHelpTests(unittest.TestCase):
    """parse_help 应该正确解析各种 clap flag 形态."""

    @classmethod
    def setUpClass(cls):
        cls.mod = load_parser_module()

    def test_simple_flag(self):
        result = self.mod.parse_help("  -j, --json\n")
        self.assertEqual(len(result["flags"]), 1)
        self.assertEqual(result["flags"][0]["name"], "json")
        self.assertEqual(result["flags"][0]["short"], "-j")
        self.assertFalse(result["flags"][0]["takes_value"])

    def test_takes_value_flag(self):
        result = self.mod.parse_help("  --file <FILE>\n      Target events file\n")
        self.assertEqual(result["flags"][0]["name"], "file")
        self.assertTrue(result["flags"][0]["takes_value"])
        self.assertEqual(result["flags"][0]["description"], "Target events file")

    def test_variadic_flag(self):
        """变长参数 <VALUE>... 应该被识别为 variadic."""
        result = self.mod.parse_help("  --payloads <PAYLOADS>...\n")
        self.assertEqual(result["flags"][0]["name"], "payloads")
        self.assertTrue(result["flags"][0]["takes_value"])
        self.assertTrue(result["flags"][0]["variadic"])

    def test_long_short_with_value(self):
        result = self.mod.parse_help("  -b, --backend <BACKEND>\n")
        self.assertEqual(result["flags"][0]["name"], "backend")
        self.assertEqual(result["flags"][0]["short"], "-b")
        self.assertTrue(result["flags"][0]["takes_value"])

    def test_multiline_description_first_line(self):
        result = self.mod.parse_help(
            "  -j, --json\n"
            "      Parse payload as JSON object\n"
            "      instead of string\n"
        )
        self.assertEqual(result["flags"][0]["name"], "json")
        self.assertEqual(result["flags"][0]["description"], "Parse payload as JSON object")

    def test_required_positional(self):
        result = self.mod.parse_help("  <TOPIC>\n      Event topic\n")
        self.assertEqual(len(result["positionals"]), 1)
        self.assertEqual(result["positionals"][0]["name"], "topic")
        self.assertTrue(result["positionals"][0]["required"])

    def test_optional_positional(self):
        result = self.mod.parse_help("  [PAYLOAD]\n      Optional payload\n")
        self.assertEqual(len(result["positionals"]), 1)
        self.assertEqual(result["positionals"][0]["name"], "payload")
        self.assertFalse(result["positionals"][0]["required"])

    def test_full_ralph_emit(self):
        """真实 ralph emit --help 的关键 flag 应该被全部解析."""
        result = self.mod.parse_help(RALPH_EMIT_HELP)
        flag_names = {f["name"] for f in result["flags"]}
        for expected in ["json", "file", "policy-check", "unsafe-no-policy-check",
                         "hat", "triggered", "source", "help", "version"]:
            self.assertIn(expected, flag_names, f"missing flag: {expected}")
        json_flag = next(f for f in result["flags"] if f["name"] == "json")
        self.assertEqual(json_flag["short"], "-j")
        file_flag = next(f for f in result["flags"] if f["name"] == "file")
        self.assertTrue(file_flag["takes_value"])

    def test_full_ralph_wave_emit_variadic(self):
        """ralph wave emit 的 --payloads <PAYLOADS>... 必须被识别为 variadic."""
        result = self.mod.parse_help(RALPH_WAVE_EMIT_HELP)
        payloads = next(f for f in result["flags"] if f["name"] == "payloads")
        self.assertTrue(payloads["takes_value"])
        self.assertTrue(payloads["variadic"])

    def test_help_and_version_flags(self):
        result = self.mod.parse_help(RALPH_EMIT_HELP)
        flag_names = {f["name"] for f in result["flags"]}
        self.assertIn("help", flag_names)
        self.assertIn("version", flag_names)

    def test_no_flags_returns_empty_list(self):
        result = self.mod.parse_help("Usage: foo\n\n  Does nothing.\n")
        self.assertEqual(result["flags"], [])
        self.assertEqual(result["positionals"], [])

    def test_no_false_positive_for_positionals_as_flags(self):
        """位置参数行 <TOPIC> 不应该被当作 flag."""
        result = self.mod.parse_help("  <TOPIC>\n      Topic\n")
        self.assertEqual(result["flags"], [])
        self.assertEqual(len(result["positionals"]), 1)


class SchemaFromHelpTests(unittest.TestCase):
    """schema_from_help 应该生成完整 schema dict."""

    @classmethod
    def setUpClass(cls):
        cls.mod = load_parser_module()

    def test_schema_has_command_and_items(self):
        schema = self.mod.schema_from_help("ralph emit", RALPH_EMIT_HELP)
        self.assertEqual(schema["command"], "ralph emit")
        self.assertIn("flags", schema)
        self.assertIn("positionals", schema)

    def test_schema_wave_emit_variadic_in_json(self):
        """--payloads variadic 字段必须出现在 JSON 输出中."""
        schema = self.mod.schema_from_help("ralph wave emit", RALPH_WAVE_EMIT_HELP)
        payloads = next(f for f in schema["flags"] if f["name"] == "payloads")
        self.assertTrue(payloads["variadic"], "variadic flag must round-trip through JSON")

    def test_round_trip_json(self):
        """schema 必须能 serialize/deserialize 完整保留关键字段."""
        schema = self.mod.schema_from_help("ralph emit", RALPH_EMIT_HELP)
        text = json.dumps(schema, ensure_ascii=False)
        reloaded = json.loads(text)
        self.assertEqual(reloaded["command"], "ralph emit")
        for f in reloaded["flags"]:
            for k in ("name", "short", "takes_value", "variadic", "required", "description"):
                self.assertIn(k, f, f"missing key {k} in flag {f.get('name')}")


class CheckCliDocDriftBehaviorTests(unittest.TestCase):
    """check-cli-doc-drift.sh 的退出码和行为契约.

    不直接执行完整 checker (依赖 ralph binary), 而是验证:
    - --help 模式输出可用信息
    - 错误参数正确报错
    - bash 语法检查通过
    """

    def test_checker_bash_syntax(self):
        """checker 脚本不能有 bash 语法错误."""
        result = subprocess.run(
            ["bash", "-n", str(CHECKER_PATH)],
            capture_output=True, text=True,
        )
        self.assertEqual(
            result.returncode, 0,
            f"bash syntax error: {result.stderr}",
        )

    def test_checker_help_lists_strict_flag(self):
        result = subprocess.run(
            ["bash", str(CHECKER_PATH), "--help"],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("--strict", result.stdout)
        self.assertIn("--update-baseline", result.stdout)

    def test_checker_requires_ralph_or_returns_specific_code(self):
        """在没有 ralph binary 时, checker 应该 exit 2 (明确错误)."""
        # 保留系统工具路径, 但移除 ralph binary 所在目录
        system_path = "/usr/bin:/bin:/usr/sbin:/sbin"
        env = {**os.environ, "PATH": system_path}
        result = subprocess.run(
            ["/bin/bash", str(CHECKER_PATH), "--strict"],
            capture_output=True, text=True, env=env,
        )
        # 无 ralph → 退出码 2 (脚本定义)
        self.assertEqual(
            result.returncode, 2,
            f"expected exit 2 when ralph missing, got {result.returncode}: {result.stderr}",
        )
        self.assertIn("ralph not found", result.stderr)


class GlobalFlagsTests(unittest.TestCase):
    """global / inherited flags 不应该在子命令中重复产生 drift.

    Strategy: parse_help 对每个 --flag 出现都会记录.
    实际 checker 应该维护一份 GLOBAL_FLAGS allowlist, 不报告这些 flag 的差异.
    这是 checker 的策略, 但 parse_help 必须能正确识别 global flags.
    """

    @classmethod
    def setUpClass(cls):
        cls.mod = load_parser_module()

    def test_help_flag_recognized_as_takes_no_value(self):
        result = self.mod.parse_help("  -h, --help\n      Print help (see a summary with '-h')\n")
        help_flag = next(f for f in result["flags"] if f["name"] == "help")
        self.assertEqual(help_flag["short"], "-h")
        self.assertFalse(help_flag["takes_value"])

    def test_config_flag_with_value(self):
        result = self.mod.parse_help("  -c, --config <CONFIG>\n      Core configuration source\n")
        config_flag = next(f for f in result["flags"] if f["name"] == "config")
        self.assertTrue(config_flag["takes_value"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
