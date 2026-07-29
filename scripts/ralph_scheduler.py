#!/usr/bin/env -S uv run --script
# /// script
# dependencies = [
#   "textual",
# ]
# ///

import argparse
import subprocess
import sys
from datetime import datetime, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo

from textual.app import App
from textual.widgets import Static


DEFAULT_SESSION = "ralph-wave"
TZ = ZoneInfo("Asia/Shanghai")

# Coding Plan reset slots (China time)
RESET_POINTS = [
    0,
    5,
    10,
    15,
    20,
]

# wait 1 minute after reset before starting
START_OFFSET_SECONDS = 60


def parse_args():
    parser = argparse.ArgumentParser(
        description="Ralph Coding Plan Reset Scheduler"
    )

    parser.add_argument(
        "--next-reset",
        action="store_true",
        help=(
            "Wait until next reset slot, then start after 1 minute. "
            "Example: ralph_scheduler.py --next-reset --plan docs/plans/a.md"
        )
    )

    parser.add_argument(
        "--reset",
        choices=[
            "00:00",
            "05:00",
            "10:00",
            "15:00",
            "20:00",
        ],
        help=(
            "Wait for specified Coding Plan reset time, then start 1 minute later. "
            "Example: --reset 05:00 means reset at 05:00, start Ralph at 05:01."
        )
    )

    parser.add_argument(
        "--delay",
        help="Delay: 30m / 2h / 1d"
    )

    parser.add_argument(
        "--plan",
        required=True,
        help=(
            "Ralph plan path. Examples: "
            "docs/plans/feature.md"
        )
    )

    parser.add_argument(
        "-c",
        "--config",
        default="ralph.pipeline.yml",
        help="Ralph config file (default: ralph.pipeline.yml)",
    )

    parser.add_argument(
        "--session",
        default=DEFAULT_SESSION,
        help="tmux session name"
    )

    return parser.parse_args()


def now_cn():
    return datetime.now(TZ)


def parse_delay(value):
    if value.endswith("m"):
        return timedelta(minutes=int(value[:-1]))

    if value.endswith("h"):
        return timedelta(hours=int(value[:-1]))

    if value.endswith("d"):
        return timedelta(days=int(value[:-1]))

    raise ValueError("Delay format: 30m / 2h / 1d")


def next_reset_time():
    now = now_cn()

    for hour in RESET_POINTS:
        target = datetime(
            now.year,
            now.month,
            now.day,
            hour,
            0,
            tzinfo=TZ,
        )

        if target > now:
            return target + timedelta(seconds=START_OFFSET_SECONDS)

    tomorrow = now + timedelta(days=1)

    target = datetime(
        tomorrow.year,
        tomorrow.month,
        tomorrow.day,
        0,
        0,
        tzinfo=TZ,
    )

    return target + timedelta(seconds=START_OFFSET_SECONDS)


def specific_reset_time(value):
    hour, minute = map(int, value.split(":"))

    now = now_cn()

    target = datetime(
        now.year,
        now.month,
        now.day,
        hour,
        minute,
        tzinfo=TZ,
    )

    if target <= now:
        target += timedelta(days=1)

    return target + timedelta(seconds=START_OFFSET_SECONDS)


def get_start_time(args):
    if args.delay:
        return now_cn() + parse_delay(args.delay)

    if args.next_reset:
        return next_reset_time()

    if args.reset:
        return specific_reset_time(args.reset)

    # default: start immediately
    return now_cn()


def start_tmux(args):
    check = subprocess.run(
        [
            "tmux",
            "has-session",
            "-t",
            args.session,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    if check.returncode == 0:
        return False, f"tmux session {args.session} exists"

    project_dir = (
        Path(__file__)
        .resolve()
        .parent
        .parent
    )

    cmd = f"""
cd '{project_dir}' &&
exec ralph run \
--worktree \
--reuse-worktree \
-H builtin:ce-executor-pipeline \
--plan '{args.plan}' \
-c '{args.config}'
"""

    subprocess.Popen(
        [
            "tmux",
            "new-session",
            "-d",
            "-s",
            args.session,
            cmd,
        ]
    )

    return True, "Ralph started"


class SchedulerApp(App):

    def __init__(self, args, start_time):
        super().__init__()

        self.args = args
        self.start_time = start_time
        self.status = Static()

    def compose(self):
        yield self.status

    def on_mount(self):
        self.set_interval(1, self.update_status)

    def update_status(self):
        now = now_cn()

        remain = (
            self.start_time - now
        ).total_seconds()

        if remain <= 0:
            ok, msg = start_tmux(self.args)

            self.status.update(
                f"""
Ralph Scheduler

Status:
{msg}

Session:
{self.args.session}

Plan:
{self.args.plan}

Config:
{self.args.config}
"""
            )

            self.exit()
            return

        h = int(remain // 3600)
        m = int((remain % 3600) // 60)
        s = int(remain % 60)

        self.status.update(
            f"""
Ralph Scheduler

Waiting...

Start:
{self.start_time}

Remaining:
{h:02d}:{m:02d}:{s:02d}

Plan:
{self.args.plan}

Config:
{self.args.config}

Session:
{self.args.session}
"""
        )


def main():
    """
    Examples:

    1. Start immediately:
       uv run ralph_scheduler.py --plan docs/plans/demo.md

    2. Wait for next Coding Plan reset:
       uv run ralph_scheduler.py --next-reset --plan docs/plans/demo.md

       Example:
       Current time: 04:30
       Next reset:   05:00
       Ralph start:  05:01

    3. Wait for a specific reset:
       uv run ralph_scheduler.py --reset 05:00 --plan docs/plans/demo.md

       Meaning:
       05:00 = Coding Plan quota reset time
       05:01 = Ralph actually starts

    4. Delay mode:
       uv run ralph_scheduler.py --delay 30m --plan docs/plans/demo.md
    """

    args = parse_args()

    start_time = get_start_time(args)

    app = SchedulerApp(
        args,
        start_time,
    )

    app.run()


if __name__ == "__main__":
    main()
