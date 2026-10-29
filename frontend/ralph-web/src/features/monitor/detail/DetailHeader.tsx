/**
 * Detail header (Plan2-U4 S1): run_id + state badge + project +
 * preset + elapsed + current_hat — one glance at the run's identity
 * and live progress.
 */

import type { MonitorRun, RunProgress } from "../api";
import { RunStateBadge } from "../components/RunStateBadge";

function fmtElapsed(ms: number | null | undefined): string {
  if (ms == null) return "—";
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

export function DetailHeader({
  run,
  progress,
}: {
  run: MonitorRun;
  progress: RunProgress | undefined;
}) {
  return (
    <div className="space-y-1" data-testid="detail-header">
      <div className="flex flex-wrap items-center gap-3">
        <h2 className="font-mono text-lg" data-testid="run-detail-id">
          {run.runId}
        </h2>
        <RunStateBadge state={run.state} stale={run.stale} />
      </div>
      <dl className="flex flex-wrap gap-x-6 gap-y-1 text-sm text-muted-foreground">
        <div className="flex gap-1.5">
          <dt>project</dt>
          <dd className="font-mono text-xs text-foreground" data-testid="detail-project">
            {run.projectId}
          </dd>
        </div>
        <div className="flex gap-1.5">
          <dt>preset</dt>
          <dd className="font-mono text-xs text-foreground">{run.preset ?? "—"}</dd>
        </div>
        <div className="flex gap-1.5">
          <dt>elapsed</dt>
          <dd className="font-mono text-xs text-foreground" data-testid="detail-elapsed">
            {fmtElapsed(progress?.elapsed_ms)}
          </dd>
        </div>
        <div className="flex gap-1.5">
          <dt>current_hat</dt>
          <dd className="font-mono text-xs text-foreground" data-testid="detail-current-hat">
            {progress?.current_hat ?? "—"}
          </dd>
        </div>
        {progress?.units.total != null && (
          <div className="flex gap-1.5">
            <dt>units</dt>
            <dd className="font-mono text-xs text-foreground" data-testid="detail-units">
              {progress.units.done}/{progress.units.total}
            </dd>
          </div>
        )}
      </dl>
    </div>
  );
}
