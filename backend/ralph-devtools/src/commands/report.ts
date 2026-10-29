/**
 * `ralph-devtools report …` — post-run diagnosis & reporting.
 *
 * `report build` combines the authoritative `ralph diagnose` JSON with
 * an event timeline (`ralph events`) and renders a single Markdown or
 * JSON document. `report compare` diffs two diagnose reports.
 */

import fs from 'node:fs';
import path from 'node:path';
import { runDiagnose } from '../report/diagnose.js';
import { fetchEvents } from '../report/events.js';
import { deriveTimeline } from '../report/timeline.js';
import {
  classifyFindings,
  repeatGroups,
  type DiagnoseReportLike,
} from '../report/classify.js';
import { compareReports } from '../report/compare.js';
import {
  renderCompareMarkdown,
  renderRunReport,
} from '../report/markdown.js';
import type { CmdContext, CmdResult } from './preset.js';

function ok(stdout: string): CmdResult {
  return { code: 0, stdout: stdout.endsWith('\n') ? stdout : `${stdout}\n` };
}

function fail(code: number, stderr: string, stdout = ''): CmdResult {
  return { code, stdout, stderr: stderr.endsWith('\n') ? stderr : `${stderr}\n` };
}

function ralphOpts(ctx: CmdContext) {
  const o: { cwd: string; ralphBin?: string; env?: NodeJS.ProcessEnv } = { cwd: ctx.cwd };
  if (ctx.ralphBin) o.ralphBin = ctx.ralphBin;
  if (ctx.env) o.env = ctx.env;
  return o;
}

function writeOrPrint(content: string, output: string | undefined, ctx: CmdContext): CmdResult {
  if (!output) return ok(content);
  const outPath = path.resolve(ctx.cwd, output);
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, content, 'utf8');
  return ok(`wrote ${outPath} (${content.length} bytes)`);
}

// ── build ───────────────────────────────────────────────────────────────────

export interface ReportBuildArgs {
  session?: string;
  diagnosticsRoot?: string;
  source?: string;
  fromLedger?: boolean;
  legacy?: boolean;
  causal?: boolean;
  supervisor?: 'off' | 'json' | 'human';
  events: boolean;
  last?: number;
  eventsFile?: string;
  eventsSource?: 'auto' | 'main' | 'hat-channel';
  format: 'md' | 'json';
  output?: string;
}

export async function reportBuildCmd(
  args: ReportBuildArgs,
  ctx: CmdContext,
): Promise<CmdResult> {
  const diag = await runDiagnose(
    {
      cwd: ctx.cwd,
      ...(args.session !== undefined ? { session: args.session } : {}),
      ...(args.diagnosticsRoot !== undefined ? { diagnosticsRoot: args.diagnosticsRoot } : {}),
      ...(args.source !== undefined ? { source: args.source } : {}),
      fromLedger: args.fromLedger,
      legacy: args.legacy,
      causal: args.causal,
      ...(args.supervisor !== undefined ? { supervisor: args.supervisor } : {}),
    },
    ralphOpts(ctx),
  );

  let timeline: ReturnType<typeof deriveTimeline> | undefined;
  let eventsError: string | undefined;
  if (args.events) {
    const ev = await fetchEvents(
      {
        cwd: ctx.cwd,
        ...(args.last !== undefined ? { last: args.last } : {}),
        ...(args.eventsFile !== undefined ? { file: args.eventsFile } : {}),
        ...(args.eventsSource !== undefined ? { eventsSource: args.eventsSource } : {}),
      },
      ralphOpts(ctx),
    );
    if (ev.ok) timeline = deriveTimeline(ev.data);
    else eventsError = ev.error.message;
  }

  const generatedAt = new Date().toISOString();
  const diagnose = diag.ok ? diag.data : undefined;
  const diagnoseError = diag.ok ? undefined : diag.error.message;

  if (args.format === 'json') {
    const payload = {
      generated_at: generatedAt,
      workspace: ctx.cwd,
      diagnose_error: diagnoseError ?? null,
      events_error: eventsError ?? null,
      diagnose: diagnose ?? null,
      timeline: timeline ?? null,
      classification: diagnose ? classifyFindings(diagnose) : null,
      repeat_groups: diagnose ? repeatGroups(diagnose) : null,
    };
    return writeOrPrint(JSON.stringify(payload, null, 2), args.output, ctx);
  }

  const md = renderRunReport({
    generatedAt,
    workspace: ctx.cwd,
    ...(diagnose !== undefined ? { diagnose } : {}),
    ...(timeline !== undefined ? { timeline } : {}),
    ...(eventsError !== undefined ? { eventsError } : {}),
    ...(diagnoseError !== undefined ? { diagnoseError } : {}),
  });
  return writeOrPrint(md, args.output, ctx);
}

// ── timeline ────────────────────────────────────────────────────────────────

export async function reportTimelineCmd(
  args: { last?: number; file?: string; eventsSource?: 'auto' | 'main' | 'hat-channel'; format: 'ascii' | 'json' },
  ctx: CmdContext,
): Promise<CmdResult> {
  const ev = await fetchEvents(
    {
      cwd: ctx.cwd,
      ...(args.last !== undefined ? { last: args.last } : {}),
      ...(args.file !== undefined ? { file: args.file } : {}),
      ...(args.eventsSource !== undefined ? { eventsSource: args.eventsSource } : {}),
    },
    ralphOpts(ctx),
  );
  if (!ev.ok) return fail(1, `events fetch failed: ${ev.error.message}`);
  const tl = deriveTimeline(ev.data);
  if (args.format === 'json') return ok(JSON.stringify(tl, null, 2));

  const lines: string[] = [];
  lines.push(`events: ${tl.totalEvents}  window: ${tl.firstTs ?? '?'} → ${tl.lastTs ?? '?'}`);
  for (const s of tl.hatSpans) {
    const topics = Object.entries(s.topicCounts)
      .sort((a, b) => b[1] - a[1])
      .map(([t, n]) => `${t}×${n}`)
      .join(', ');
    lines.push(
      `  [${String(s.firstSeq).padStart(4)}–${String(s.lastSeq).padEnd(4)}] ${s.hat.padEnd(24)} ${String(s.events).padStart(3)} events  ${topics}`,
    );
  }
  return ok(lines.join('\n'));
}

// ── compare ─────────────────────────────────────────────────────────────────

function readDiagnoseJson(
  file: string,
): { ok: true; data: DiagnoseReportLike } | { ok: false; error: string } {
  try {
    const raw = fs.readFileSync(file, 'utf8');
    return { ok: true, data: JSON.parse(raw) as DiagnoseReportLike };
  } catch (err) {
    return { ok: false, error: `cannot read ${file}: ${String(err)}` };
  }
}

export interface ReportCompareArgs {
  aJson?: string;
  bJson?: string;
  sessionA?: string;
  sessionB?: string;
  diagnosticsRoot?: string;
  format: 'md' | 'json';
  output?: string;
}

export async function reportCompareCmd(
  args: ReportCompareArgs,
  ctx: CmdContext,
): Promise<CmdResult> {
  let a: DiagnoseReportLike;
  let b: DiagnoseReportLike;

  if (args.sessionA !== undefined || args.sessionB !== undefined) {
    if (args.sessionA === undefined || args.sessionB === undefined) {
      return fail(2, '--session-a and --session-b must be given together');
    }
    const [ra, rb] = await Promise.all([
      runDiagnose(
        {
          cwd: ctx.cwd,
          session: args.sessionA,
          ...(args.diagnosticsRoot !== undefined ? { diagnosticsRoot: args.diagnosticsRoot } : {}),
        },
        ralphOpts(ctx),
      ),
      runDiagnose(
        {
          cwd: ctx.cwd,
          session: args.sessionB,
          ...(args.diagnosticsRoot !== undefined ? { diagnosticsRoot: args.diagnosticsRoot } : {}),
        },
        ralphOpts(ctx),
      ),
    ]);
    if (!ra.ok) return fail(1, `diagnose A failed: ${ra.error.message}`);
    if (!rb.ok) return fail(1, `diagnose B failed: ${rb.error.message}`);
    a = ra.data;
    b = rb.data;
  } else {
    if (!args.aJson || !args.bJson) {
      return fail(2, 'usage: report compare <a.json> <b.json>  |  --session-a X --session-b Y');
    }
    const pa = readDiagnoseJson(path.resolve(ctx.cwd, args.aJson));
    const pb = readDiagnoseJson(path.resolve(ctx.cwd, args.bJson));
    if (!pa.ok) return fail(1, pa.error);
    if (!pb.ok) return fail(1, pb.error);
    a = pa.data;
    b = pb.data;
  }

  const cmp = compareReports(a, b);
  if (args.format === 'json') {
    return writeOrPrint(JSON.stringify(cmp, null, 2), args.output, ctx);
  }
  return writeOrPrint(renderCompareMarkdown(cmp), args.output, ctx);
}
