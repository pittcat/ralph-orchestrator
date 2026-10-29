/**
 * Overview tab (Plan2-U4): the Plan1-U5 field table, unchanged —
 * identity fields + state evidence + evidence timestamps.
 */

import type { RunDetail } from "../api";

function Field({ label, value }: { label: string; value: string | null | undefined }) {
  return (
    <div className="grid grid-cols-[9rem_1fr] gap-2 py-1 text-sm">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="break-all font-mono text-xs">{value ?? "—"}</dd>
    </div>
  );
}

export function OverviewTab({ detail }: { detail: RunDetail }) {
  const { run } = detail;
  const ts = run.evidenceTimestamps;
  return (
    <div className="space-y-4" data-testid="overview-tab">
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
          <Field label="last_event_at" value={ts.lastEventAt} />
          <Field label="terminal.at" value={ts.terminalAt} />
          <Field label="last_observed_at" value={ts.lastObservedAt} />
          <Field label="collected_at" value={ts.collectedAt} />
        </dl>
      </section>
    </div>
  );
}
