/**
 * Cross-run comparison of two `ralph diagnose --format json` reports.
 *
 * Findings are keyed by `retry_key` (the dedup identity the Rust
 * reporter already aggregates on). A finding present in both sides
 * reports its occurrence delta; severity transitions surface
 * escalation. Pure function — inputs are already-parsed JSON.
 */

import type {
  DiagnoseReportLike,
  DiagnoseTopFinding,
  LedgerRootCause,
} from './classify.js';
import { classifyReason, diagnoseView } from './classify.js';

export type FindingStatus = 'new' | 'resolved' | 'persistent' | 'escalated' | 'improved';

export interface FindingDelta {
  retry_key: string;
  status: FindingStatus;
  category: string;
  reason_code: string;
  topic: string | null;
  target_hat: string | null;
  a: { occurrences: number; severity: string; last_iteration: number | null } | null;
  b: { occurrences: number; severity: string; last_iteration: number | null } | null;
  message: string;
}

export interface ReportComparison {
  aSession: string;
  bSession: string;
  aView: 'session' | 'ledger' | 'unknown';
  bView: 'session' | 'ledger' | 'unknown';
  newCount: number;
  resolvedCount: number;
  persistentCount: number;
  escalatedCount: number;
  improvedCount: number;
  deltas: FindingDelta[];
  dupStormNew: string[];
  dupStormResolved: string[];
}

const SEV_RANK: Record<string, number> = { error: 3, warn: 2, warning: 2, info: 1, pass: 0 };

function key(f: DiagnoseTopFinding): string {
  return (
    f.retry_key ??
    `${f.source ?? ''}:${f.target_hat ?? '*'}:${f.topic ?? '*'}:${f.reason_code ?? ''}`
  );
}

interface NormFinding {
  key: string;
  reason_code: string;
  source: string;
  topic: string | null;
  target_hat: string | null;
  occurrences: number;
  severity: string;
  last_iteration: number | null;
  message: string;
}

function normSession(f: DiagnoseTopFinding): NormFinding {
  return {
    key: key(f),
    reason_code: f.reason_code ?? '',
    source: f.source ?? '',
    topic: f.topic ?? null,
    target_hat: f.target_hat ?? null,
    occurrences: f.occurrences ?? 1,
    severity: (f.severity ?? 'unknown').toLowerCase(),
    last_iteration: f.last_iteration ?? null,
    message: f.message ?? '',
  };
}

function normLedger(f: LedgerRootCause): NormFinding {
  return {
    key: `ledger:${f.stage ?? ''}:${f.reason_code ?? ''}`,
    reason_code: f.reason_code ?? '',
    source: f.stage ?? '',
    topic: f.affected_topics?.[0] ?? null,
    target_hat: f.affected_hats?.[0] ?? null,
    occurrences: f.frequency ?? 1,
    severity: f.escalated ? 'error' : 'warn',
    last_iteration: null,
    message: f.suggested_remediation ?? '',
  };
}

function normAll(r: DiagnoseReportLike): Map<string, NormFinding> {
  const map = new Map<string, NormFinding>();
  for (const f of r.top_findings ?? []) {
    const n = normSession(f);
    map.set(n.key, n);
  }
  for (const f of r.root_causes ?? []) {
    const n = normLedger(f);
    map.set(n.key, n);
  }
  return map;
}

export function compareReports(
  a: DiagnoseReportLike,
  b: DiagnoseReportLike,
): ReportComparison {
  const aMap = normAll(a);
  const bMap = normAll(b);

  const deltas: FindingDelta[] = [];
  for (const [k, fb] of bMap) {
    const fa = aMap.get(k);
    const common = {
      retry_key: k,
      category: classifyReason(fb.reason_code, fb.source),
      reason_code: fb.reason_code,
      topic: fb.topic,
      target_hat: fb.target_hat,
      message: fb.message,
    };
    if (!fa) {
      deltas.push({
        ...common,
        status: 'new',
        a: null,
        b: { occurrences: fb.occurrences, severity: fb.severity, last_iteration: fb.last_iteration },
      });
      continue;
    }
    const sevA = SEV_RANK[fa.severity] ?? 0;
    const sevB = SEV_RANK[fb.severity] ?? 0;
    const status: FindingStatus =
      sevB > sevA ? 'escalated' : sevB < sevA ? 'improved' : 'persistent';
    deltas.push({
      ...common,
      status,
      a: { occurrences: fa.occurrences, severity: fa.severity, last_iteration: fa.last_iteration },
      b: { occurrences: fb.occurrences, severity: fb.severity, last_iteration: fb.last_iteration },
    });
  }
  for (const [k, fa] of aMap) {
    if (bMap.has(k)) continue;
    deltas.push({
      retry_key: k,
      status: 'resolved',
      category: classifyReason(fa.reason_code, fa.source),
      reason_code: fa.reason_code,
      topic: fa.topic,
      target_hat: fa.target_hat,
      a: { occurrences: fa.occurrences, severity: fa.severity, last_iteration: fa.last_iteration },
      b: null,
      message: fa.message,
    });
  }

  const statusOrder: Record<FindingStatus, number> = {
    escalated: 0, new: 1, persistent: 2, improved: 3, resolved: 4,
  };
  deltas.sort(
    (x, y) =>
      statusOrder[x.status] - statusOrder[y.status] ||
      (y.b?.occurrences ?? y.a?.occurrences ?? 0) - (x.b?.occurrences ?? x.a?.occurrences ?? 0),
  );

  const dupA = new Set(a.dup_storm_topics ?? []);
  const dupB = new Set(b.dup_storm_topics ?? []);

  return {
    aSession: a.session_path ?? a.workspace ?? 'A',
    bSession: b.session_path ?? b.workspace ?? 'B',
    newCount: deltas.filter((d) => d.status === 'new').length,
    resolvedCount: deltas.filter((d) => d.status === 'resolved').length,
    persistentCount: deltas.filter((d) => d.status === 'persistent').length,
    escalatedCount: deltas.filter((d) => d.status === 'escalated').length,
    improvedCount: deltas.filter((d) => d.status === 'improved').length,
    deltas,
    dupStormNew: [...dupB].filter((t) => !dupA.has(t)).sort(),
    dupStormResolved: [...dupA].filter((t) => !dupB.has(t)).sort(),
    aView: diagnoseView(a),
    bView: diagnoseView(b),
  };
}
