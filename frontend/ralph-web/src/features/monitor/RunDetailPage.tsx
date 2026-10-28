/**
 * U5 minimal run detail — identity + state badge + state_evidence
 * summary + field table + evidence timestamps. Plan2 replaces this
 * with the tabbed detail page; no tabs here per unit scope.
 */

import { Link, useLocation, useParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";
import { useMonitorRunDetail } from "./hooks";
import { RunStateBadge } from "./components/RunStateBadge";

function Field({ label, value }: { label: string; value: string | null | undefined }) {
  return (
    <div className="grid grid-cols-[9rem_1fr] gap-2 py-1 text-sm">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="break-all font-mono text-xs">{value ?? "—"}</dd>
    </div>
  );
}

export function RunDetailPage() {
  const { globalId } = useParams<{ globalId: string }>();
  const location = useLocation();
  const decoded = globalId ? decodeURIComponent(globalId) : undefined;
  const detail = useMonitorRunDetail(decoded);
  // Preserve the overview filter URL on the way back.
  const backTo = `/monitor${location.search}`;

  if (detail.isLoading) {
    return (
      <div className="space-y-3" data-testid="detail-loading">
        <Skeleton className="h-6 w-64" />
        <Skeleton className="h-40 w-full" />
      </div>
    );
  }
  if (detail.isError || !detail.data) {
    return (
      <div className="space-y-3">
        <Link
          to={backTo}
          className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="h-4 w-4" /> Back to overview
        </Link>
        <p role="alert" className="text-sm text-status-failed">
          Run not found or monitor unreachable.
        </p>
      </div>
    );
  }

  const { run, evidenceTimestamps } = { run: detail.data.run, evidenceTimestamps: detail.data.run.evidenceTimestamps };

  return (
    <div className="space-y-4" data-testid="run-detail">
      <Link
        to={backTo}
        className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" /> Back to overview
      </Link>

      <div className="flex flex-wrap items-center gap-3">
        <h2 className="font-mono text-lg" data-testid="run-detail-id">
          {run.runId}
        </h2>
        <RunStateBadge state={run.state} stale={run.stale} />
      </div>

      {Object.keys(run.stateEvidence).length > 0 && (
        <pre
          data-testid="state-evidence"
          className="max-h-40 overflow-auto rounded-md border border-border bg-muted/40 p-3 text-xs"
        >
          {JSON.stringify(run.stateEvidence, null, 2)}
        </pre>
      )}

      <section aria-label="Run fields">
        <dl>
          <Field label="kind" value={run.kind} />
          <Field label="preset" value={run.preset} />
          <Field label="plan" value={run.planPath} />
          <Field label="cwd" value={run.cwd} />
          <Field label="repo_root" value={run.repoRoot} />
          <Field label="worktree" value={run.worktreePath} />
          <Field label="prompt" value={run.promptPreview} />
          <Field label="manifest" value={run.manifestPath} />
        </dl>
      </section>

      <section aria-label="Evidence timestamps">
        <h3 className="mb-1 text-sm font-medium text-muted-foreground">
          Evidence timestamps
        </h3>
        <dl data-testid="evidence-timestamps">
          <Field label="started_at" value={run.startedAt} />
          <Field label="last_event_at" value={evidenceTimestamps.lastEventAt} />
          <Field label="terminal.at" value={evidenceTimestamps.terminalAt} />
          <Field label="last_observed_at" value={evidenceTimestamps.lastObservedAt} />
          <Field label="collected_at" value={evidenceTimestamps.collectedAt} />
        </dl>
      </section>
    </div>
  );
}
