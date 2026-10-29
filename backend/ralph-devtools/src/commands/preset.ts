/**
 * `ralph-devtools preset …` subcommands — the authoring workflow:
 * inspect → graph → diff → check/verify (Rust authority) →
 * export/new → run-command.
 */

import fs from 'node:fs';
import path from 'node:path';
import { loadPreset, type LoadOptions } from '../preset/model.js';
import { deriveTopology } from '../preset/topology.js';
import {
  renderAsciiGraph,
  renderMermaid,
  renderShow,
} from '../preset/render.js';
import { diffPresets, renderDiff } from '../preset/diff.js';
import {
  builtinList,
  presetBaselineDiff,
  presetCheck,
  presetNew,
  presetUpgrade,
  presetVerify,
  templateList,
  type ContractReport,
} from '../preset/rust.js';

export interface CmdContext {
  cwd: string;
  ralphBin?: string;
  env?: NodeJS.ProcessEnv;
}

export interface CmdResult {
  code: number;
  stdout: string;
  stderr?: string;
}

function ok(stdout: string): CmdResult {
  return { code: 0, stdout: stdout.endsWith('\n') ? stdout : `${stdout}\n` };
}

function fail(code: number, stderr: string, stdout = ''): CmdResult {
  return { code, stdout, stderr: stderr.endsWith('\n') ? stderr : `${stderr}\n` };
}

function loadOpts(ctx: CmdContext): LoadOptions {
  const o: LoadOptions = { cwd: ctx.cwd };
  if (ctx.ralphBin) o.ralphBin = ctx.ralphBin;
  if (ctx.env) o.env = ctx.env;
  return o;
}

function ralphOpts(ctx: CmdContext) {
  const o: { cwd: string; ralphBin?: string; env?: NodeJS.ProcessEnv } = { cwd: ctx.cwd };
  if (ctx.ralphBin) o.ralphBin = ctx.ralphBin;
  if (ctx.env) o.env = ctx.env;
  return o;
}

// ── list ────────────────────────────────────────────────────────────────────

export async function presetListCmd(
  args: { format: string },
  ctx: CmdContext,
): Promise<CmdResult> {
  const res = await builtinList(ralphOpts(ctx));
  const builtin = res.ok ? res.data.presets : [];
  const builtinError = res.ok ? null : res.error.message;

  const hatsDir = path.join(ctx.cwd, '.ralph', 'hats');
  let local: { id: string; source: string; description: string }[] = [];
  try {
    local = fs
      .readdirSync(hatsDir)
      .filter((n) => /\.ya?ml$/i.test(n))
      .sort()
      .map((n) => ({
        id: `directory:${n.replace(/\.ya?ml$/i, '')}`,
        source: 'local',
        description: path.join('.ralph', 'hats', n),
      }));
  } catch {
    /* no .ralph/hats dir — fine */
  }

  if (args.format === 'json') {
    return ok(
      JSON.stringify(
        { builtin, local, builtin_error: builtinError },
        null,
        2,
      ),
    );
  }
  const lines: string[] = ['builtin presets:'];
  for (const b of builtin) lines.push(`  builtin:${b.id}  —  ${b.description}`);
  if (builtin.length === 0) lines.push(`  (unavailable: ${builtinError ?? 'none'})`);
  lines.push('local presets (.ralph/hats):');
  for (const l of local) lines.push(`  ${l.id}  —  ${l.description}`);
  if (local.length === 0) lines.push('  (none)');
  return ok(lines.join('\n'));
}

// ── templates ───────────────────────────────────────────────────────────────

export async function presetTemplatesCmd(
  args: { format: string },
  ctx: CmdContext,
): Promise<CmdResult> {
  const res = await templateList(ralphOpts(ctx));
  if (!res.ok) return fail(1, `preset list failed: ${res.error.message}`);
  if (args.format === 'json') return ok(JSON.stringify(res.data, null, 2));
  const lines: string[] = ['workflow templates:'];
  for (const t of res.data) {
    lines.push(
      `  ${t.name ?? '?'}  —  ${t.description ?? ''} (${t.difficulty ?? '?'}/${t.category ?? '?'})`,
    );
  }
  return ok(lines.join('\n'));
}

// ── show / graph ────────────────────────────────────────────────────────────

export async function presetShowCmd(
  ref: string,
  args: { format: string },
  ctx: CmdContext,
): Promise<CmdResult> {
  const res = await loadPreset(ref, loadOpts(ctx));
  if (!res.ok) return fail(1, res.error);
  const topo = deriveTopology(res.model);
  if (args.format === 'json') {
    const { yaml: _yaml, doc, ...rest } = res.model;
    return ok(JSON.stringify({ ...rest, doc_sections: Object.keys(doc), topology: topo }, null, 2));
  }
  return ok(renderShow(res.model, topo));
}

export async function presetGraphCmd(
  ref: string,
  args: { format: string },
  ctx: CmdContext,
): Promise<CmdResult> {
  const res = await loadPreset(ref, loadOpts(ctx));
  if (!res.ok) return fail(1, res.error);
  const topo = deriveTopology(res.model);
  switch (args.format) {
    case 'json':
      return ok(JSON.stringify(topo, null, 2));
    case 'mermaid':
      return ok(renderMermaid(res.model, topo));
    default:
      return ok(renderAsciiGraph(topo));
  }
}

// ── diff ────────────────────────────────────────────────────────────────────

export async function presetDiffCmd(
  refA: string,
  refB: string | undefined,
  args: { format: string; baseline: boolean },
  ctx: CmdContext,
): Promise<CmdResult> {
  if (args.baseline) {
    const target = refB ?? refA;
    const res = await presetBaselineDiff(target, ralphOpts(ctx));
    if (!res.ok) return fail(1, `preset diff failed: ${res.error.message}`);
    return ok(args.format === 'json' ? JSON.stringify(res.data, null, 2) : res.raw);
  }
  if (!refB) return fail(2, 'usage: preset diff <refA> <refB|builtin:id> [--baseline]');
  const [a, b] = await Promise.all([
    loadPreset(refA, loadOpts(ctx)),
    loadPreset(refB, loadOpts(ctx)),
  ]);
  if (!a.ok) return fail(1, `cannot load ${refA}: ${a.error}`);
  if (!b.ok) return fail(1, `cannot load ${refB}: ${b.error}`);
  const diff = diffPresets(a.model, b.model);
  return ok(args.format === 'json' ? JSON.stringify(diff, null, 2) : renderDiff(diff));
}

// ── export ──────────────────────────────────────────────────────────────────

export async function presetExportCmd(
  id: string,
  args: { output?: string; force: boolean; format: string },
  ctx: CmdContext,
): Promise<CmdResult> {
  const bare = id.startsWith('builtin:') ? id.slice('builtin:'.length) : id;
  const res = await loadPreset(`builtin:${bare}`, loadOpts(ctx));
  if (!res.ok) return fail(1, res.error);

  if (!args.output) {
    return { code: 0, stdout: res.model.yaml.endsWith('\n') ? res.model.yaml : `${res.model.yaml}\n` };
  }
  const outPath = path.resolve(ctx.cwd, args.output);
  if (fs.existsSync(outPath) && !args.force) {
    return fail(1, `${outPath} exists — pass --force to overwrite`);
  }
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, res.model.yaml, 'utf8');
  const msg = `wrote ${outPath} (${res.model.yaml.length} bytes)`;
  return ok(args.format === 'json' ? JSON.stringify({ written: outPath, bytes: res.model.yaml.length }) : msg);
}

// ── check / verify ──────────────────────────────────────────────────────────

function renderCheckHuman(report: ContractReport): string {
  const lines: string[] = [];
  lines.push(
    `preset check: ${report.passed ? 'PASS' : 'FAIL'}  (${report.errors} errors, ${report.warnings} warnings)  source=${report.source_label}`,
  );
  const bySev = { error: [] as typeof report.findings, warn: [] as typeof report.findings, pass: [] as typeof report.findings };
  for (const f of report.findings) {
    (bySev[f.severity as keyof typeof bySev] ?? bySev.pass).push(f);
  }
  for (const [sev, list] of Object.entries(bySev)) {
    if (list.length === 0) continue;
    lines.push(`\n${sev.toUpperCase()} (${list.length}):`);
    for (const f of list) {
      const detail =
        f.details && Object.keys(f.details).length > 0
          ? `  [${Object.entries(f.details).map(([k, v]) => `${k}=${v}`).join(' ')}]`
          : '';
      lines.push(`  [${f.source}] ${f.id}: ${f.message}${detail}`);
      if (f.action_hint) lines.push(`    hint: ${f.action_hint}`);
    }
  }
  return lines.join('\n');
}

export async function presetCheckCmd(
  file: string,
  args: { strict: boolean; config?: string[]; format: string },
  ctx: CmdContext,
): Promise<CmdResult> {
  const res = await presetCheck(
    file,
    { strict: args.strict, ...(args.config ? { config: args.config } : {}) },
    ralphOpts(ctx),
  );
  if (!res.ok) {
    // `preset check` exits non-zero on FAIL but still prints the JSON
    // report — surface it when parseable, else the raw error.
    return fail(1, `preset check failed: ${res.error.message}${res.error.stderr ? `\n${res.error.stderr}` : ''}`);
  }
  const exitCode = res.data.passed ? 0 : 1;
  if (args.format === 'json') {
    return { code: exitCode, stdout: `${JSON.stringify(res.data, null, 2)}\n` };
  }
  return { code: exitCode, stdout: `${renderCheckHuman(res.data)}\n` };
}

export async function presetVerifyCmd(
  file: string,
  scenario: string,
  args: { config?: string[]; format: string },
  ctx: CmdContext,
): Promise<CmdResult> {
  const res = await presetVerify(
    file,
    scenario,
    { ...(args.config ? { config: args.config } : {}) },
    ralphOpts(ctx),
  );
  if (!res.ok) {
    return fail(1, `preset verify failed: ${res.error.message}${res.error.stderr ? `\n${res.error.stderr}` : ''}`);
  }
  const passed = res.data.passed === true;
  if (args.format === 'json') {
    return { code: passed ? 0 : 1, stdout: `${JSON.stringify(res.data, null, 2)}\n` };
  }
  return {
    code: passed ? 0 : 1,
    stdout: `preset verify: ${passed ? 'PASS' : 'FAIL'}\n${res.raw}`,
  };
}

// ── new / upgrade ───────────────────────────────────────────────────────────

export async function presetNewCmd(
  template: string,
  args: { name?: string; output?: string; force: boolean; check: boolean; format: string },
  ctx: CmdContext,
): Promise<CmdResult> {
  const res = await presetNew(
    template,
    {
      ...(args.name !== undefined ? { name: args.name } : {}),
      ...(args.output !== undefined ? { output: args.output } : {}),
      force: args.force,
      check: args.check,
    },
    ralphOpts(ctx),
  );
  if (!res.ok) {
    return fail(1, `preset new failed: ${res.error.message}${res.error.stderr ? `\n${res.error.stderr}` : ''}`);
  }
  if (args.format === 'json') return ok(JSON.stringify(res.data, null, 2));
  return ok(res.raw);
}

export async function presetUpgradeCmd(
  file: string,
  args: { format: string },
  ctx: CmdContext,
): Promise<CmdResult> {
  const res = await presetUpgrade(file, ralphOpts(ctx));
  if (!res.ok) return fail(1, `preset upgrade failed: ${res.error.message}`);
  return ok(args.format === 'json' ? JSON.stringify(res.data, null, 2) : res.raw);
}

// ── run-command ─────────────────────────────────────────────────────────────

export function presetRunCommandCmd(
  file: string,
  args: { config?: string[]; prompt?: string; promptFile?: string; plan?: string; backend?: string; maxIterations?: number; autonomous: boolean },
): CmdResult {
  const argv = ['ralph', 'run', '-H', file];
  for (const c of args.config ?? []) argv.push('-c', c);
  if (args.prompt !== undefined) argv.push('-p', args.prompt);
  if (args.promptFile !== undefined) argv.push('--prompt-file', args.promptFile);
  if (args.plan !== undefined) argv.push('--plan', args.plan);
  if (args.backend !== undefined) argv.push('--backend', args.backend);
  if (args.maxIterations !== undefined) argv.push('--max-iterations', String(args.maxIterations));
  if (args.autonomous) argv.push('-a');
  // Print a shell-safe representation: quote only args with
  // shell-special characters so the line stays copy-pasteable.
  const needsQuote = /[^a-zA-Z0-9_\-./:=@]/;
  const q = (s: string) =>
    needsQuote.test(s) ? `'${s.replace(/'/g, `'\\''`)}'` : s;
  const rendered = argv.map(q).join(' ');
  return ok(
    [
      '# Launch this preset without registering a builtin (generated by ralph-devtools):',
      rendered,
      '',
      '# Validate first:',
      `ralph-devtools preset check ${q(file)}`,
    ].join('\n'),
  );
}
