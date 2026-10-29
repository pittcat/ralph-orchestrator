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

function Meta({
  k,
  v,
  testid,
}: {
  k: string;
  v: string | null | undefined;
  testid?: string;
}) {
  return (
    <div className="flex items-baseline gap-1.5">
      <dt className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground/70">
        {k}
      </dt>
      <dd className="font-mono text-xs text-foreground" data-testid={testid}>
        {v ?? "—"}
      </dd>
    </div>
  );
}

export function DetailHeader({
  run,
  progress,
}: {
  run: MonitorRun;
  progress: RunProgress | undefined;
}) {
  return (
    <div className="min-w-0 space-y-2" data-testid="detail-header">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <h2
          className="break-all font-mono text-base font-semibold tracking-tight"
          data-testid="run-detail-id"
        >
          {run.runId}
        </h2>
        <RunStateBadge state={run.state} stale={run.stale} />
      </div>
      <dl className="flex flex-wrap gap-x-5 gap-y-1">
        <Meta k="project" v={run.projectId} testid="detail-project" />
        <Meta k="preset" v={run.preset} />
        <Meta
          k="elapsed"
          v={fmtElapsed(progress?.elapsed_ms)}
          testid="detail-elapsed"
        />
        <Meta
          k="current_hat"
          v={progress?.current_hat}
          testid="detail-current-hat"
        />
        {progress?.units.total != null && (
          <Meta
            k="units"
            v={`${progress.units.done}/${progress.units.total}`}
            testid="detail-units"
          />
        )}
      </dl>
    </div>
  );
}
