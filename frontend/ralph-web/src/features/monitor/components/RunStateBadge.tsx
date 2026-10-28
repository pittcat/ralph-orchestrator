/**
 * RunStateBadge — semantic color per monitor state vocabulary + a
 * `stale` marker when the projection row is stale (S5/S6 evidence).
 */

import { cn } from "@/lib/utils";
import type { MonitorRunState } from "../api";

const STATE_STYLES: Record<MonitorRunState, { label: string; cls: string }> = {
  running: { label: "running", cls: "border-status-running/50 bg-status-running/15 text-status-running" },
  silent: { label: "silent", cls: "border-status-stale/50 bg-status-stale/15 text-status-stale" },
  exited_success: { label: "success", cls: "border-status-success/50 bg-status-success/15 text-status-success" },
  exited_failed: { label: "failed", cls: "border-status-failed/50 bg-status-failed/15 text-status-failed" },
  exited_merged: { label: "merged", cls: "border-status-merged/50 bg-status-merged/15 text-status-merged" },
  exited_unknown: { label: "unknown", cls: "border-status-unknown/50 bg-status-unknown/15 text-status-unknown" },
};

export function RunStateBadge({
  state,
  stale = false,
}: {
  state: MonitorRunState;
  stale?: boolean;
}) {
  const s = STATE_STYLES[state] ?? STATE_STYLES.exited_unknown;
  return (
    <span className="inline-flex items-center gap-1">
      <span
        data-testid="run-state-badge"
        data-state={state}
        className={cn(
          "inline-flex items-center rounded-full border px-2 py-0.5 text-xs font-medium",
          s.cls,
        )}
      >
        {s.label}
      </span>
      {stale && (
        <span
          data-testid="stale-badge"
          title="Projection may be outdated — last collection failed"
          className="inline-flex items-center rounded-full border border-status-stale/50 bg-status-stale/15 px-2 py-0.5 text-xs font-medium text-status-stale"
        >
          stale
        </span>
      )}
    </span>
  );
}
