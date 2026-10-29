/**
 * Failure classification & repeated-failure aggregation (pure).
 *
 * Categories are derived from `reason_code`/`source` naming observed in
 * `crates/ralph-core/src/diagnosis/` + event policy reason codes — a
 * display taxonomy, not a second authority. Rules are prefix/substring
 * matches applied in order; the first hit wins.
 */

export type FailureCategory =
  | 'payload_contract'
  | 'isolation_violation'
  | 'duplicate_resend'
  | 'wave_handoff'
  | 'missing_emit'
  | 'topology_flow'
  | 'drift'
  | 'task_resume'
  | 'other';

interface Rule {
  category: FailureCategory;
  match: RegExp;
}

const RULES: Rule[] = [
  {
    category: 'duplicate_resend',
    match: /duplicate|dedupe|same_step|stall_bypass|seen_count|dup_storm/,
  },
  {
    category: 'isolation_violation',
    match: /isolated|scope_violation|origin|does_not_publish|not_in_config|boundary|deny|undeclared|anonymous_business/,
  },
  {
    category: 'wave_handoff',
    match: /wave|handoff|aggregate|dispatch_timeout|retry_cap/,
  },
  {
    category: 'missing_emit',
    match: /no_emit|missing_emit|missing_event|missing_terminal|stall|no_response|failed_to_converge|timeout/,
  },
  {
    category: 'payload_contract',
    match: /missing_field|payload|schema|invalid_topic|invalid_payload|blank_blocking|missing_plan|missing_state|missing_root_goal|missing_receiver|missing_success|missing_failure|unknown_to_hat|must_do_empty|required/,
  },
  {
    category: 'topology_flow',
    match: /phase_violation|dimension_mismatch|dimension_missing|outside_topology|orphan|unreachable|flow/,
  },
  { category: 'drift', match: /drift/ },
  {
    category: 'task_resume',
    match: /task_resume|dead_letter|default_publishes_injected|recovery_outcome/,
  },
];

export function classifyReason(reasonCode: string, source = ''): FailureCategory {
  const hay = `${reasonCode} ${source}`;
  for (const rule of RULES) {
    if (rule.match.test(hay)) return rule.category;
  }
  return 'other';
}

export const CATEGORY_LABELS: Record<FailureCategory, string> = {
  payload_contract: 'Payload contract（schema/必填字段/非法 topic）',
  isolation_violation: 'Isolation/Origin 越权（未声明 topic/边界）',
  duplicate_resend: '重复发送 / Dedup 命中',
  wave_handoff: 'Wave/Handoff 编排异常',
  missing_emit: '应发未发 / 超时失语',
  topology_flow: '拓扑/流转异常',
  drift: 'Drift 指标越阈',
  task_resume: 'task.resume / 恢复通道',
  other: '其他',
};

// ── Aggregation over a `ralph diagnose` JSON report ─────────────────────────

/** Minimal shape we consume from `ralph diagnose --format json`. */
export interface DiagnoseTimelineRow {
  iteration?: number | null;
  hat?: string;
  severity?: string;
  outcome?: string;
  target_hat?: string | null;
  topic?: string | null;
  reason_code?: string;
  message?: string;
  hint?: string | null;
}

export interface DiagnoseTopFinding {
  retry_key?: string;
  severity?: string;
  outcome?: string;
  source?: string;
  target_hat?: string | null;
  topic?: string | null;
  reason_code?: string;
  message?: string;
  occurrences?: number;
  first_iteration?: number | null;
  last_iteration?: number | null;
  escalated?: boolean;
  hint?: string | null;
}

/** Ledger-view root cause (`schema_version="u8-1"`). */
export interface LedgerRootCause {
  stage?: string;
  reason_code?: string;
  frequency?: number;
  first_seen_ts?: string;
  last_seen_ts?: string;
  affected_hats?: string[];
  affected_topics?: string[];
  suggested_remediation?: string;
  escalated?: boolean;
}

export interface LedgerRejectionSummary {
  record_count?: number;
  distinct_reason_codes?: number;
  terminal_count?: number;
  first_seen_ts?: string;
  last_seen_ts?: string;
  affected_hats?: string[];
  affected_topics?: string[];
}

export interface LedgerSummary {
  commit_log_path?: string;
  commit_count?: number;
  rejection_commits?: number;
  escalation_commits?: number;
  task_lifecycle_commits?: number;
  corruption?: boolean;
  warnings?: string[];
}

export interface DiagnoseReportLike {
  schema_version?: string;
  session_path?: string;
  workspace?: string;
  used_ledger?: boolean;
  top_findings?: DiagnoseTopFinding[];
  recovery_timeline?: DiagnoseTimelineRow[];
  root_causes?: LedgerRootCause[];
  rejection_summary?: LedgerRejectionSummary;
  ledger_summary?: LedgerSummary;
  dup_storm_topics?: string[];
  drift_findings?: unknown[];
  repair_suggestions?: unknown[];
  evidence_gaps?: unknown[];
  errors?: unknown[];
  warnings?: string[];
  summary?: unknown;
  [k: string]: unknown;
}

/** Which diagnose view this report carries. */
export function diagnoseView(r: DiagnoseReportLike): 'session' | 'ledger' | 'unknown' {
  if (r.root_causes !== undefined || r.used_ledger !== undefined) return 'ledger';
  if (r.top_findings !== undefined || r.recovery_timeline !== undefined) return 'session';
  return 'unknown';
}

export interface CategoryBucket {
  category: FailureCategory;
  label: string;
  findings: number;
  occurrences: number;
  escalated: number;
  worstSeverity: 'error' | 'warn' | 'info' | 'unknown';
  examples: { reason_code: string; message: string; topic?: string | null; target_hat?: string | null }[];
}

export interface RepeatGroup {
  key: string;
  category: FailureCategory;
  reason_code: string;
  topic: string | null;
  hat: string | null;
  count: number;
  firstIteration: number | null;
  lastIteration: number | null;
  sampleMessage: string;
}

const SEVERITY_RANK: Record<string, number> = { error: 3, warn: 2, warning: 2, info: 1, pass: 0 };

function bucketFor(
  map: Map<FailureCategory, CategoryBucket>,
  category: FailureCategory,
): CategoryBucket {
  let b = map.get(category);
  if (!b) {
    b = {
      category,
      label: CATEGORY_LABELS[category],
      findings: 0,
      occurrences: 0,
      escalated: 0,
      worstSeverity: 'unknown',
      examples: [],
    };
    map.set(category, b);
  }
  return b;
}

/**
 * Classify findings from either diagnose view: session `top_findings`
 * (severity+occurrences) or ledger `root_causes` (frequency, escalated
 * flag; severity is approximated — escalated → error, else warn).
 */
export function classifyFindings(report: DiagnoseReportLike): CategoryBucket[] {
  const buckets = new Map<FailureCategory, CategoryBucket>();
  for (const f of report.top_findings ?? []) {
    const category = classifyReason(f.reason_code ?? '', f.source ?? '');
    const b = bucketFor(buckets, category);
    b.findings += 1;
    b.occurrences += f.occurrences ?? 1;
    if (f.escalated) b.escalated += 1;
    const sev = (f.severity ?? 'unknown').toLowerCase();
    if ((SEVERITY_RANK[sev] ?? -1) > (SEVERITY_RANK[b.worstSeverity] ?? -1)) {
      b.worstSeverity = (sev === 'warning' ? 'warn' : sev) as CategoryBucket['worstSeverity'];
    }
    if (b.examples.length < 3) {
      b.examples.push({
        reason_code: f.reason_code ?? '',
        message: f.message ?? '',
        topic: f.topic ?? null,
        target_hat: f.target_hat ?? null,
      });
    }
  }
  for (const rc of report.root_causes ?? []) {
    const category = classifyReason(rc.reason_code ?? '', rc.stage ?? '');
    const b = bucketFor(buckets, category);
    b.findings += 1;
    b.occurrences += rc.frequency ?? 1;
    if (rc.escalated) b.escalated += 1;
    const sev: CategoryBucket['worstSeverity'] = rc.escalated ? 'error' : 'warn';
    if ((SEVERITY_RANK[sev] ?? -1) > (SEVERITY_RANK[b.worstSeverity] ?? -1)) {
      b.worstSeverity = sev;
    }
    if (b.examples.length < 3) {
      b.examples.push({
        reason_code: rc.reason_code ?? '',
        message: rc.suggested_remediation ?? '',
        topic: rc.affected_topics?.[0] ?? null,
        target_hat: rc.affected_hats?.[0] ?? null,
      });
    }
  }
  return [...buckets.values()].sort(
    (a, b) =>
      (SEVERITY_RANK[b.worstSeverity] ?? -1) - (SEVERITY_RANK[a.worstSeverity] ?? -1) ||
      b.occurrences - a.occurrences,
  );
}

/** Re-aggregate `recovery_timeline` rows into repeated-failure groups. */
export function repeatGroups(report: DiagnoseReportLike): RepeatGroup[] {
  const groups = new Map<string, RepeatGroup>();
  // Ledger view: root_causes are already the repeated-failure groups.
  for (const rc of report.root_causes ?? []) {
    const key = `ledger|${rc.reason_code ?? ''}|${(rc.affected_topics ?? []).join(',')}`;
    groups.set(key, {
      key,
      category: classifyReason(rc.reason_code ?? '', rc.stage ?? ''),
      reason_code: rc.reason_code ?? '',
      topic: rc.affected_topics?.[0] ?? null,
      hat: rc.affected_hats?.[0] ?? null,
      count: rc.frequency ?? 1,
      firstIteration: null,
      lastIteration: null,
      sampleMessage: rc.suggested_remediation ?? '',
    });
  }
  for (const row of report.recovery_timeline ?? []) {
    const reason = row.reason_code ?? '';
    const key = `${reason}|${row.topic ?? ''}|${row.hat ?? ''}`;
    let g = groups.get(key);
    if (!g) {
      g = {
        key,
        category: classifyReason(reason),
        reason_code: reason,
        topic: row.topic ?? null,
        hat: row.hat ?? null,
        count: 0,
        firstIteration: row.iteration ?? null,
        lastIteration: row.iteration ?? null,
        sampleMessage: row.message ?? '',
      };
      groups.set(key, g);
    }
    g.count += 1;
    if (row.iteration != null) {
      g.firstIteration = g.firstIteration == null ? row.iteration : Math.min(g.firstIteration, row.iteration);
      g.lastIteration = g.lastIteration == null ? row.iteration : Math.max(g.lastIteration, row.iteration);
    }
  }
  return [...groups.values()].sort((a, b) => b.count - a.count || a.key.localeCompare(b.key));
}
