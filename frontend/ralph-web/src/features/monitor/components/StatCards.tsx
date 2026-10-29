/**
 * StatCards — at-a-glance run-state counters for the overview. Each
 * card doubles as a one-click state filter: click scopes the table
 * to that state group, click again clears back to all states.
 */

import { Activity, BellOff, CheckCircle2, XCircle } from "lucide-react";
import { cn } from "@/lib/utils";
import type { MonitorRun } from "../api";
import type { OverviewFilters } from "../filters";

interface StatDef {
  key: string;
  label: string;
  icon: typeof Activity;
  match: (r: MonitorRun) => boolean;
  /** state selection applied when the card is clicked */
  states: string[];
  iconCls: string;
}

const STATS: StatDef[] = [
  {
    key: "running",
    label: "Running",
    icon: Activity,
    match: (r) => r.state === "running",
    states: ["running"],
    iconCls: "bg-status-running/15 text-status-running",
  },
  {
    key: "silent",
    label: "Silent",
    icon: BellOff,
    match: (r) => r.state === "silent",
    states: ["silent"],
    iconCls: "bg-status-stale/15 text-status-stale",
  },
  {
    key: "succeeded",
    label: "Succeeded",
    icon: CheckCircle2,
    match: (r) => r.state === "exited_success" || r.state === "exited_merged",
    states: ["exited_success", "exited_merged"],
    iconCls: "bg-status-success/15 text-status-success",
  },
  {
    key: "failed",
    label: "Failed",
    icon: XCircle,
    match: (r) => r.state === "exited_failed" || r.state === "exited_unknown",
    states: ["exited_failed", "exited_unknown"],
    iconCls: "bg-status-failed/15 text-status-failed",
  },
];

export function StatCards({
  runs,
  states,
  onChange,
}: {
  /** unfiltered run set (snapshot) — global counts, not page rows */
  runs: MonitorRun[];
  /** currently selected `state` filter values */
  states: OverviewFilters["states"];
  onChange: (patch: Partial<OverviewFilters>) => void;
}) {
  const selected = new Set(states);
  return (
    <div
      className="grid grid-cols-2 gap-3 sm:grid-cols-4"
      data-testid="stat-cards"
    >
      {STATS.map((s) => {
        const count = runs.reduce((n, r) => n + (s.match(r) ? 1 : 0), 0);
        const active =
          selected.size === s.states.length &&
          s.states.every((x) => selected.has(x));
        const Icon = s.icon;
        return (
          <button
            key={s.key}
            type="button"
            aria-pressed={active}
            onClick={() =>
              onChange({ states: active ? [] : [...s.states] })
            }
            className={cn(
              "group flex items-center gap-3 rounded-xl border border-border bg-card/50 px-4 py-3 text-left transition-colors",
              "hover:border-foreground/25 hover:bg-card",
              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
              active && "border-primary/50 bg-primary/5 hover:border-primary/60",
            )}
            title={`Filter by ${s.label.toLowerCase()}`}
          >
            <span
              className={cn(
                "flex h-9 w-9 shrink-0 items-center justify-center rounded-lg",
                s.iconCls,
              )}
            >
              <Icon className="h-4 w-4" />
            </span>
            <span className="min-w-0">
              <span className="block text-2xl font-semibold leading-none tabular-nums">
                {count}
              </span>
              <span className="mt-1 block truncate text-xs text-muted-foreground">
                {s.label}
              </span>
            </span>
          </button>
        );
      })}
    </div>
  );
}
