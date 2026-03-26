#!/usr/bin/env python3

"""Update README coverage badge content and/or write a Shields endpoint JSON file."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


def badge_color(percent: float) -> str:
    if percent >= 90:
        return "brightgreen"
    if percent >= 80:
        return "yellowgreen"
    if percent >= 70:
        return "yellow"
    if percent >= 60:
        return "orange"
    return "red"


def format_percent(percent: float) -> str:
    rounded = round(percent, 1)
    if rounded.is_integer():
        return str(int(rounded))
    return f"{rounded:.1f}"


def load_line_coverage(summary_path: Path) -> float:
    data = json.loads(summary_path.read_text())
    return float(data["data"][0]["totals"]["lines"]["percent"])


def build_badge_payload(percent: float) -> dict[str, object]:
    return {
        "schemaVersion": 1,
        "label": "coverage",
        "message": f"{format_percent(percent)}%",
        "color": badge_color(percent),
    }


def update_readme(readme_path: Path, percent: float) -> None:
    percent_text = format_percent(percent)
    color = badge_color(percent)
    replacement = (
        f"[![Coverage](https://img.shields.io/badge/coverage-{percent_text}%25-{color})]"
        "(CONTRIBUTING.md#coverage)"
    )

    pattern = re.compile(r"^\[!\[Coverage\]\([^)]+\)\]\([^)]+\)$", re.MULTILINE)
    original = readme_path.read_text()
    updated, count = pattern.subn(replacement, original, count=1)
    if count != 1:
        raise SystemExit(f"Could not find coverage badge in {readme_path}")
    readme_path.write_text(updated)


def write_badge_json(output_path: Path, percent: float) -> None:
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(f"{json.dumps(build_badge_payload(percent), indent=2)}\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--summary",
        default="coverage/summary.json",
        help="Path to cargo-llvm-cov JSON summary output",
    )
    parser.add_argument(
        "--readme",
        help="Path to the README file to update",
    )
    parser.add_argument(
        "--badge-json",
        help="Optional path to write a Shields endpoint JSON file",
    )
    args = parser.parse_args()

    summary_path = Path(args.summary)
    percent = load_line_coverage(summary_path)

    outputs: list[str] = []
    if args.readme:
        readme_path = Path(args.readme)
        update_readme(readme_path, percent)
        outputs.append(f"updated {readme_path}")

    if args.badge_json:
        badge_path = Path(args.badge_json)
        write_badge_json(badge_path, percent)
        outputs.append(f"wrote {badge_path}")

    if not outputs:
        raise SystemExit("Nothing to do: pass --readme and/or --badge-json")

    print(f"{', '.join(outputs)} with {format_percent(percent)}% line coverage")


if __name__ == "__main__":
    main()
