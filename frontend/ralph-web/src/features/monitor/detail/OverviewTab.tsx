/**
 * Overview tab (Plan2-U4): identity fields + structured state
 * evidence + evidence timestamps, grouped into cards. Raw evidence
 * JSON stays reachable behind a <details> fold for debugging.
 */

import { useState, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";
import type { RunDetail } from "../api";

function Card({
  title,
  testid,
  children,
  className = "",
}: {
  title: string;
  testid?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section
      data-testid={testid}
      className={`overflow-hidden rounded-lg border border-border/70 bg-card/50 ${className}`}
    >
      <h3 className="border-b border-border/60 bg-muted/30 px-3 py-1.5 text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
        {title}
      </h3>
      {children}
    </section>
  );
}

function CopyValue({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      aria-label="复制"
      title={copied ? "已复制" : "复制"}
      className="ml-1.5 inline-flex align-middle text-muted-foreground/40 opacity-0 transition-opacity hover:text-foreground group-hover/row:opacity-100"
      onClick={() => {
        void navigator.clipboard
          ?.writeText(value)
          .then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          })
          .catch(() => {});
      }}
    >
      {copied ? (
        <Check className="h-3 w-3 text-emerald-500" />
      ) : (
        <Copy className="h-3 w-3" />
      )}
    </button>
  );
}

function Field({
  label,
  value,
  copyable = false,
}: {
  label: string;
  value: string | null | undefined;
  copyable?: boolean;
}) {
  return (
    <div className="group/row grid grid-cols-[6.5rem_1fr] items-baseline gap-3 px-3 py-1.5 sm:grid-cols-[9rem_1fr]">
      <dt className="truncate text-xs text-muted-foreground">{label}</dt>
      <dd className="min-w-0 font-mono text-xs leading-5">
        <span className="break-all" title={value ?? undefined}>
          {value ?? "—"}
        </span>
        {copyable && value ? <CopyValue value={value} /> : null}
      </dd>
    </div>
  );
}

/** true reads alarming for keys like manifestCorrupt / *Fail*. */
const NEGATIVE_TRUE = /corrupt|fail|error/i;

function EvidenceRow({ k, v }: { k: string; v: unknown }) {
  let body: ReactNode;
  if (v === null || v === undefined) {
    body = <span className="text-muted-foreground">—</span>;
  } else if (typeof v === "boolean") {
    const alarming = v && NEGATIVE_TRUE.test(k);
    body = (
      <span
        className={`inline-flex items-center gap-1.5 rounded-full border px-1.5 py-px text-[10px] font-medium ${
          v
            ? alarming
              ? "border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-300"
              : "border-emerald-500/40 bg-emerald-500/10 text-emerald-600 dark:text-emerald-300"
            : "border-border/60 bg-muted/30 text-muted-foreground"
        }`}
      >
        {String(v)}
      </span>
    );
  } else {
    body = <span className="break-all">{String(v)}</span>;
  }
  return (
    <div className="grid grid-cols-[6.5rem_1fr] items-baseline gap-3 px-3 py-1.5 sm:grid-cols-[9rem_1fr]">
      <dt className="truncate text-xs text-muted-foreground" title={k}>
        {k}
      </dt>
      <dd className="min-w-0 font-mono text-xs leading-5">{body}</dd>
    </div>
  );
}

export function OverviewTab({ detail }: { detail: RunDetail }) {
  const { run } = detail;
  const ts = run.evidenceTimestamps;
  const evidence = Object.entries(run.stateEvidence);
  return (
    <div className="grid gap-4 lg:grid-cols-2" data-testid="overview-tab">
      <Card title="Identity" className="lg:col-span-2">
        <dl className="divide-y divide-border/50">
          <Field label="kind" value={run.kind} />
          <Field label="preset" value={run.preset} />
          <Field label="plan" value={run.planPath} copyable />
          <Field label="cwd" value={run.cwd} copyable />
          <Field label="repo_root" value={run.repoRoot} copyable />
          <Field label="worktree" value={run.worktreePath} copyable />
          <Field label="prompt" value={run.promptPreview} />
          <Field label="manifest" value={run.manifestPath} copyable />
        </dl>
      </Card>
      {evidence.length > 0 && (
        <Card title="State evidence" testid="state-evidence">
          <dl className="divide-y divide-border/50">
            {evidence.map(([k, v]) => (
              <EvidenceRow key={k} k={k} v={v} />
            ))}
          </dl>
          <details className="border-t border-border/60 px-3 py-2">
            <summary className="cursor-pointer select-none text-[11px] text-muted-foreground/70 hover:text-foreground">
              原始 JSON
            </summary>
            <pre className="mt-2 max-h-48 overflow-auto rounded-md bg-muted/40 p-2 text-[11px] leading-4">
              {JSON.stringify(run.stateEvidence, null, 2)}
            </pre>
          </details>
        </Card>
      )}
      <Card title="Evidence timestamps">
        <dl
          data-testid="evidence-timestamps"
          className="divide-y divide-border/50"
        >
          <Field label="started_at" value={run.startedAt} />
          <Field label="last_event_at" value={ts.lastEventAt} />
          <Field label="terminal.at" value={ts.terminalAt} />
          <Field label="last_observed_at" value={ts.lastObservedAt} />
          <Field label="collected_at" value={ts.collectedAt} />
        </dl>
      </Card>
    </div>
  );
}
