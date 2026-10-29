/**
 * Markdown renderers for the combined run report and the cross-run
 * comparison. Sections degrade gracefully — missing inputs render an
 * explicit note rather than an empty table.
 */

import type { DiagnoseReportLike } from './classify.js';
import {
  CATEGORY_LABELS,
  classifyFindings,
  diagnoseView,
  repeatGroups,
} from './classify.js';
import type { ReportComparison } from './compare.js';
import type { Timeline } from './timeline.js';

function esc(s: string): string {
  return s.replace(/\|/g, '\\|').replace(/\n/g, ' ');
}

function trunc(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n)}…` : s;
}

export interface ReportBundle {
  /** `ralph diagnose --format json` output (undefined if unavailable). */
  diagnose?: DiagnoseReportLike;
  /** Derived event timeline (undefined when no events were fetched). */
  timeline?: Timeline;
  /** Context lines for the header. */
  workspace?: string;
  eventsError?: string;
  diagnoseError?: string;
  generatedAt: string;
}

export function renderRunReport(bundle: ReportBundle): string {
  const out: string[] = [];
  out.push('# Ralph Run Report');
  out.push('');
  out.push(`- generated: ${bundle.generatedAt}`);
  if (bundle.workspace) out.push(`- workspace: \`${bundle.workspace}\``);
  const d = bundle.diagnose;
  if (d) {
    out.push(`- diagnose view: ${diagnoseView(d)} (schema ${d.schema_version ?? '?'})`);
    if (d.session_path) out.push(`- session: \`${d.session_path}\``);
    if (d.workspace) out.push(`- diagnose workspace: \`${d.workspace}\``);
  }
  if (d?.summary && typeof d.summary === 'object') {
    out.push(`- summary: \`${esc(trunc(JSON.stringify(d.summary), 300))}\``);
  }
  // Ledger view headline counters (u8-1).
  const rej = d?.rejection_summary;
  const led = d?.ledger_summary;
  if (rej && (rej.record_count ?? 0) > 0) {
    out.push(
      `- rejection records: ${rej.record_count} (${rej.distinct_reason_codes ?? 0} distinct reason codes, ${rej.terminal_count ?? 0} terminal, ${rej.first_seen_ts ?? '?'} → ${rej.last_seen_ts ?? '?'})`,
    );
  }
  if (led && (led.commit_count ?? 0) > 0) {
    out.push(
      `- ledger commits: ${led.commit_count} (rejections=${led.rejection_commits ?? 0}, escalations=${led.escalation_commits ?? 0}, task-lifecycle=${led.task_lifecycle_commits ?? 0})`,
    );
  }
  out.push('');

  // ── Failure classification ──────────────────────────────────────────
  out.push('## Failure classification（失败归类）');
  out.push('');
  if (!d) {
    out.push(`_diagnose unavailable: ${bundle.diagnoseError ?? 'not provided'}_`);
  } else {
    const buckets = classifyFindings(d);
    if (buckets.length === 0) {
      out.push('_no findings_');
    } else {
      out.push('| category | findings | occurrences | escalated | worst |');
      out.push('|---|---|---|---|---|');
      for (const b of buckets) {
        out.push(
          `| ${esc(b.label)} | ${b.findings} | ${b.occurrences} | ${b.escalated} | ${b.worstSeverity} |`,
        );
      }
      out.push('');
      out.push('<details><summary>examples</summary>');
      out.push('');
      for (const b of buckets) {
        for (const ex of b.examples) {
          const tgt = [ex.topic, ex.target_hat].filter(Boolean).join(' → ');
          out.push(
            `- **${CATEGORY_LABELS[b.category]}** \`${ex.reason_code}\`${tgt ? ` (${esc(tgt)})` : ''}: ${esc(trunc(ex.message, 200))}`,
          );
        }
      }
      out.push('');
      out.push('</details>');
    }
  }
  out.push('');

  // ── Repeated failures ───────────────────────────────────────────────
  out.push('## Repeated failures（重复失败聚合）');
  out.push('');
  if (!d) {
    out.push('_diagnose unavailable_');
  } else {
    const groups = repeatGroups(d).filter((g) => g.count > 1);
    if (groups.length === 0) {
      out.push('_no repeated failure groups_');
    } else {
      out.push('| count | category | reason_code | topic | hat | iterations |');
      out.push('|---|---|---|---|---|---|');
      for (const g of groups.slice(0, 20)) {
        const iters =
          g.firstIteration === g.lastIteration
            ? String(g.firstIteration ?? '—')
            : `${g.firstIteration ?? '—'}–${g.lastIteration ?? '—'}`;
        out.push(
          `| ${g.count} | ${g.category} | \`${esc(g.reason_code)}\` | ${esc(g.topic ?? '—')} | ${esc(g.hat ?? '—')} | ${iters} |`,
        );
      }
      if (groups.length > 20) out.push(`\n_… ${groups.length - 20} more groups_`);
    }
    const dup = d.dup_storm_topics ?? [];
    if (dup.length > 0) {
      out.push('');
      out.push(`**dup_storm_topics**: ${dup.map((t) => `\`${esc(t)}\``).join(', ')}`);
    }
  }
  out.push('');

  // ── Timeline ────────────────────────────────────────────────────────
  out.push('## Timeline（时间线）');
  out.push('');
  const tl = bundle.timeline;
  if (!tl) {
    out.push(`_events unavailable: ${bundle.eventsError ?? 'not provided'}_`);
  } else if (tl.totalEvents === 0) {
    out.push('_no events_');
  } else {
    out.push(
      `events: ${tl.totalEvents}  window: ${tl.firstTs ?? '?'} → ${tl.lastTs ?? '?'}`,
    );
    out.push('');
    out.push('### Iterations');
    out.push('');
    out.push('| iteration | events | hats | top topics |');
    out.push('|---|---|---|---|');
    for (const row of tl.iterations.slice(-30)) {
      const tops = Object.entries(row.topics)
        .sort((a, b) => b[1] - a[1])
        .slice(0, 4)
        .map(([t, n]) => `${esc(t)}×${n}`)
        .join(', ');
      out.push(
        `| ${row.iteration} | ${row.events} | ${esc(row.hats.join(', '))} | ${tops} |`,
      );
    }
    if (tl.iterations.length > 30) {
      out.push(`\n_(showing last 30 of ${tl.iterations.length} iterations)_`);
    }
    out.push('');
    out.push('### Hat spans');
    out.push('');
    out.push('| span | hat | events | window |');
    out.push('|---|---|---|---|');
    const spans = tl.hatSpans;
    const shown = spans.length > 40 ? spans.slice(-40) : spans;
    for (const s of shown) {
      out.push(
        `| ${s.firstSeq}–${s.lastSeq} | ${esc(s.hat)} | ${s.events} | ${s.firstTs ?? ''} → ${s.lastTs ?? ''} |`,
      );
    }
    if (spans.length > 40) out.push(`\n_(showing last 40 of ${spans.length} spans)_`);
    if (tl.waves.length > 0) {
      out.push('');
      out.push('### Waves');
      out.push('');
      out.push('| wave_id | events | hats | topics |');
      out.push('|---|---|---|---|');
      for (const w of tl.waves.slice(0, 20)) {
        const tops = Object.entries(w.topics)
          .map(([t, n]) => `${esc(t)}×${n}`)
          .join(', ');
        out.push(`| \`${esc(w.wave_id)}\` | ${w.events} | ${esc(w.hats.join(', '))} | ${tops} |`);
      }
    }
  }
  out.push('');

  // ── Rust-side passthrough sections ──────────────────────────────────
  if (d) {
    const drift = d.drift_findings ?? [];
    if (drift.length > 0) {
      out.push('## Drift findings');
      out.push('');
      for (const f of drift.slice(0, 30)) {
        out.push(`- \`${esc(trunc(JSON.stringify(f), 240))}\``);
      }
      out.push('');
    }
    const repairs = d.repair_suggestions ?? [];
    if (repairs.length > 0) {
      out.push('## Repair suggestions（来自 Rust diagnose，非执行性建议）');
      out.push('');
      for (const r of repairs) {
        const o = r as Record<string, unknown>;
        const title = o.title ?? o.id ?? JSON.stringify(o.layer ?? 'suggestion');
        out.push(`- **${esc(String(title))}**: ${esc(trunc(JSON.stringify(o), 400))}`);
      }
      out.push('');
    }
    const gaps = d.evidence_gaps ?? [];
    if (gaps.length > 0) {
      out.push('## Evidence gaps');
      out.push('');
      for (const g of gaps) {
        out.push(`- \`${esc(trunc(JSON.stringify(g), 240))}\``);
      }
      out.push('');
    }
    const warns = d.warnings ?? [];
    if (warns.length > 0) {
      out.push('## Warnings');
      out.push('');
      for (const w of warns) out.push(`- ${esc(w)}`);
      out.push('');
    }
  }

  out.push('---');
  out.push(
    '_findings/timeline facts come from `ralph diagnose`/`ralph events`; ' +
      'classification & comparison are advisory TS layers._',
  );
  return out.join('\n');
}

export function renderCompareMarkdown(cmp: ReportComparison): string {
  const out: string[] = [];
  out.push('# Ralph Run Comparison');
  out.push('');
  out.push(`- A: \`${cmp.aSession}\``);
  out.push(`- B: \`${cmp.bSession}\``);
  out.push(
    `- new: ${cmp.newCount}  escalated: ${cmp.escalatedCount}  persistent: ${cmp.persistentCount}  improved: ${cmp.improvedCount}  resolved: ${cmp.resolvedCount}`,
  );
  out.push('');
  if (cmp.deltas.length === 0) {
    out.push('_no findings on either side_');
  } else {
    out.push('| status | category | reason_code | topic → hat | A occ | B occ | message |');
    out.push('|---|---|---|---|---|---|---|');
    for (const d of cmp.deltas) {
      const tgt = [d.topic, d.target_hat].filter(Boolean).join(' → ') || '—';
      out.push(
        `| ${d.status} | ${d.category} | \`${esc(d.reason_code)}\` | ${esc(tgt)} | ${d.a?.occurrences ?? '—'} | ${d.b?.occurrences ?? '—'} | ${esc(trunc(d.message, 140))} |`,
      );
    }
  }
  if (cmp.dupStormNew.length > 0 || cmp.dupStormResolved.length > 0) {
    out.push('');
    if (cmp.dupStormNew.length > 0) {
      out.push(`- new dup storms: ${cmp.dupStormNew.map((t) => `\`${esc(t)}\``).join(', ')}`);
    }
    if (cmp.dupStormResolved.length > 0) {
      out.push(`- resolved dup storms: ${cmp.dupStormResolved.map((t) => `\`${esc(t)}\``).join(', ')}`);
    }
  }
  return out.join('\n');
}
