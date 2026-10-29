#!/usr/bin/env node
/**
 * `ralph-devtools` — TS-side companion to the Rust `ralph` CLI.
 *
 * Two surfaces:
 * - `preset …`  preset authoring workflow: inspect/graph/diff local or
 *   builtin presets, then delegate validity to `ralph preset
 *   check`/`verify` and emit runnable YAML + launch command.
 * - `report …`  post-run diagnosis: combine `ralph diagnose` +
 *   `ralph events` into timeline/classification/aggregation reports
 *   and cross-run comparisons.
 *
 * Everything reaches the Rust base through argv spawns of `ralph`
 * (see src/ralph/runner.ts) — no direct `.ralph/*` ledger reads, no
 * duplicated validation authority.
 */

import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';
import fs from 'node:fs';
import path from 'node:path';
import {
  presetCheckCmd,
  presetDiffCmd,
  presetExportCmd,
  presetGraphCmd,
  presetListCmd,
  presetNewCmd,
  presetRunCommandCmd,
  presetShowCmd,
  presetTemplatesCmd,
  presetUpgradeCmd,
  presetVerifyCmd,
  type CmdContext,
  type CmdResult,
} from './commands/preset.js';
import {
  reportBuildCmd,
  reportCompareCmd,
  reportTimelineCmd,
} from './commands/report.js';

const USAGE = `ralph-devtools — preset authoring + run diagnosis (TS side)

Usage:
  ralph-devtools preset <sub> [args] [options]
  ralph-devtools report <sub> [args] [options]
  ralph-devtools help

preset subcommands:
  list                          builtin presets + .ralph/hats/*.yml inventory
  templates                     workflow template catalog (ralph preset list)
  show <ref>                    hats summary + event-loop knobs (ref: file | builtin:<id>)
  graph <ref>                   event topology (hat --topic--> hat)
  diff <refA> <refB>            structural diff; --baseline delegates to ralph preset diff
  export <builtin-id>           write builtin YAML to stdout or --output file
  check <file>                  ralph preset check (add --strict, -c <cfg>)
  verify <file> --scenario <s>  ralph preset verify (scripted scenario)
  new <template>                scaffold via ralph preset new [--name --output --force --check]
  upgrade <file>                ralph preset upgrade preview
  run-command <file>            print the ralph run -H <file> launch line

report subcommands:
  build                         diagnose + events → Markdown/JSON report (default sub)
  timeline                      hat-span event timeline
  compare <a.json> <b.json>     cross-run diagnose diff (or --session-a/--session-b)

Common options:
  --format <fmt>                human|json (per subcommand; graph adds ascii|mermaid)
  --cwd <dir>                   working directory for ralph invocations (default: cwd)
  --ralph-bin <path>            override the ralph binary (default: 'ralph' on PATH)
`;

function emit(res: CmdResult): number {
  if (res.stdout) process.stdout.write(res.stdout);
  if (res.stderr) process.stderr.write(res.stderr);
  return res.code;
}

function strList(v: string | string[] | undefined): string[] | undefined {
  if (v === undefined) return undefined;
  return Array.isArray(v) ? v : [v];
}

/** Strip global flags (usable in any position) before per-sub parsing. */
function stripGlobals(argv: string[], ctx: CmdContext): string[] {
  const out: string[] = [];
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--cwd' || a === '--ralph-bin') {
      const v = argv[i + 1];
      if (v !== undefined) {
        if (a === '--cwd') ctx.cwd = path.resolve(v);
        else ctx.ralphBin = v;
        i += 1;
      }
      continue;
    }
    if (a.startsWith('--cwd=')) {
      ctx.cwd = path.resolve(a.slice('--cwd='.length));
      continue;
    }
    if (a.startsWith('--ralph-bin=')) {
      ctx.ralphBin = a.slice('--ralph-bin='.length);
      continue;
    }
    out.push(a);
  }
  return out;
}

async function dispatch(rawArgv: string[]): Promise<CmdResult> {
  const ctx: CmdContext = { cwd: process.cwd() };
  const argv = stripGlobals(rawArgv, ctx);
  const [group, sub, ...rest] = argv;

  // `report` with no sub defaults to `build` and keeps the remaining
  // argv as its args.
  const effectiveSub = group === 'report' && sub === undefined ? 'build' : sub;
  const args = group === 'report' && sub === undefined ? argv.slice(1) : rest;

  if (group === 'preset') {
    switch (effectiveSub) {
      case 'list': {
        const { values } = parseArgs({
          args, strict: true, allowPositionals: false,
          options: { format: { type: 'string', default: 'human' } },
        });
        return presetListCmd({ format: values.format }, ctx);
      }
      case 'templates': {
        const { values } = parseArgs({
          args, strict: true, allowPositionals: false,
          options: { format: { type: 'string', default: 'human' } },
        });
        return presetTemplatesCmd({ format: values.format }, ctx);
      }
      case 'show':
      case 'graph': {
        const { values, positionals } = parseArgs({
          args, strict: true, allowPositionals: true,
          options: { format: { type: 'string', default: effectiveSub === 'graph' ? 'ascii' : 'human' } },
        });
        const ref = positionals[0];
        if (!ref) return { code: 2, stdout: '', stderr: `usage: preset ${effectiveSub} <ref>\n` };
        return effectiveSub === 'show'
          ? presetShowCmd(ref, { format: values.format }, ctx)
          : presetGraphCmd(ref, { format: values.format }, ctx);
      }
      case 'diff': {
        const { values, positionals } = parseArgs({
          args, strict: true, allowPositionals: true,
          options: {
            format: { type: 'string', default: 'human' },
            baseline: { type: 'boolean', default: false },
          },
        });
        if (positionals.length === 0) {
          return { code: 2, stdout: '', stderr: 'usage: preset diff <refA> <refB|builtin:id> [--baseline]\n' };
        }
        return presetDiffCmd(
          positionals[0],
          positionals[1],
          { format: values.format, baseline: values.baseline },
          ctx,
        );
      }
      case 'export': {
        const { values, positionals } = parseArgs({
          args, strict: true, allowPositionals: true,
          options: {
            output: { type: 'string', short: 'o' },
            force: { type: 'boolean', default: false },
            format: { type: 'string', default: 'human' },
          },
        });
        const id = positionals[0];
        if (!id) return { code: 2, stdout: '', stderr: 'usage: preset export <builtin-id> [-o file] [--force]\n' };
        return presetExportCmd(
          id,
          {
            ...(values.output !== undefined ? { output: values.output } : {}),
            force: values.force,
            format: values.format,
          },
          ctx,
        );
      }
      case 'check': {
        const { values, positionals } = parseArgs({
          args, strict: true, allowPositionals: true,
          options: {
            strict: { type: 'boolean', default: false },
            config: { type: 'string', short: 'c', multiple: true },
            format: { type: 'string', default: 'human' },
          },
        });
        const file = positionals[0];
        if (!file) return { code: 2, stdout: '', stderr: 'usage: preset check <file> [--strict] [-c cfg]…\n' };
        return presetCheckCmd(
          file,
          {
            strict: values.strict,
            ...(values.config !== undefined ? { config: strList(values.config) } : {}),
            format: values.format,
          },
          ctx,
        );
      }
      case 'verify': {
        const { values, positionals } = parseArgs({
          args, strict: true, allowPositionals: true,
          options: {
            scenario: { type: 'string' },
            config: { type: 'string', short: 'c', multiple: true },
            format: { type: 'string', default: 'human' },
          },
        });
        const file = positionals[0];
        if (!file || !values.scenario) {
          return { code: 2, stdout: '', stderr: 'usage: preset verify <file> --scenario <s>\n' };
        }
        return presetVerifyCmd(
          file,
          values.scenario,
          {
            ...(values.config !== undefined ? { config: strList(values.config) } : {}),
            format: values.format,
          },
          ctx,
        );
      }
      case 'new': {
        const { values, positionals } = parseArgs({
          args, strict: true, allowPositionals: true,
          options: {
            name: { type: 'string' },
            output: { type: 'string', short: 'o' },
            force: { type: 'boolean', default: false },
            check: { type: 'boolean', default: false },
            format: { type: 'string', default: 'human' },
          },
        });
        const template = positionals[0];
        if (!template) return { code: 2, stdout: '', stderr: 'usage: preset new <template> [--name] [-o file]\n' };
        return presetNewCmd(
          template,
          {
            ...(values.name !== undefined ? { name: values.name } : {}),
            ...(values.output !== undefined ? { output: values.output } : {}),
            force: values.force,
            check: values.check,
            format: values.format,
          },
          ctx,
        );
      }
      case 'upgrade': {
        const { values, positionals } = parseArgs({
          args, strict: true, allowPositionals: true,
          options: { format: { type: 'string', default: 'human' } },
        });
        const file = positionals[0];
        if (!file) return { code: 2, stdout: '', stderr: 'usage: preset upgrade <file>\n' };
        return presetUpgradeCmd(file, { format: values.format }, ctx);
      }
      case 'run-command': {
        const { values, positionals } = parseArgs({
          args, strict: true, allowPositionals: true,
          options: {
            config: { type: 'string', short: 'c', multiple: true },
            prompt: { type: 'string', short: 'p' },
            'prompt-file': { type: 'string' },
            plan: { type: 'string' },
            backend: { type: 'string' },
            'max-iterations': { type: 'string' },
            autonomous: { type: 'boolean', short: 'a', default: false },
          },
        });
        const file = positionals[0];
        if (!file) return { code: 2, stdout: '', stderr: 'usage: preset run-command <file> [run opts]\n' };
        return presetRunCommandCmd(file, {
          ...(values.config !== undefined ? { config: strList(values.config) } : {}),
          ...(values.prompt !== undefined ? { prompt: values.prompt } : {}),
          ...(values['prompt-file'] !== undefined ? { promptFile: values['prompt-file'] } : {}),
          ...(values.plan !== undefined ? { plan: values.plan } : {}),
          ...(values.backend !== undefined ? { backend: values.backend } : {}),
          ...(values['max-iterations'] !== undefined
            ? { maxIterations: Number(values['max-iterations']) }
            : {}),
          autonomous: values.autonomous,
        });
      }
      default:
        return { code: 2, stdout: '', stderr: `unknown preset subcommand: ${String(effectiveSub)}\n\n${USAGE}` };
    }
  }

  if (group === 'report') {
    switch (effectiveSub) {
      case 'build': {
        const { values } = parseArgs({
          args, strict: true, allowPositionals: false,
          options: {
            session: { type: 'string' },
            'diagnostics-root': { type: 'string' },
            source: { type: 'string' },
            'from-ledger': { type: 'boolean', default: false },
            legacy: { type: 'boolean', default: false },
            causal: { type: 'boolean', default: false },
            supervisor: { type: 'string' },
            events: { type: 'boolean', default: true },
            'no-events': { type: 'boolean', default: false },
            last: { type: 'string' },
            'events-file': { type: 'string' },
            'events-source': { type: 'string' },
            format: { type: 'string', default: 'md' },
            output: { type: 'string', short: 'o' },
          },
        });
        const fmt = values.format === 'json' ? 'json' : 'md';
        return reportBuildCmd(
          {
            ...(values.session !== undefined ? { session: values.session } : {}),
            ...(values['diagnostics-root'] !== undefined ? { diagnosticsRoot: values['diagnostics-root'] } : {}),
            ...(values.source !== undefined ? { source: values.source } : {}),
            fromLedger: values['from-ledger'],
            legacy: values.legacy,
            causal: values.causal,
            ...(values.supervisor === 'json' || values.supervisor === 'human'
              ? { supervisor: values.supervisor }
              : {}),
            events: values['no-events'] ? false : values.events,
            ...(values.last !== undefined ? { last: Number(values.last) } : {}),
            ...(values['events-file'] !== undefined ? { eventsFile: values['events-file'] } : {}),
            ...(values['events-source'] === 'main' ||
            values['events-source'] === 'hat-channel' ||
            values['events-source'] === 'auto'
              ? { eventsSource: values['events-source'] }
              : {}),
            format: fmt,
            ...(values.output !== undefined ? { output: values.output } : {}),
          },
          ctx,
        );
      }
      case 'timeline': {
        const { values } = parseArgs({
          args, strict: true, allowPositionals: false,
          options: {
            last: { type: 'string' },
            file: { type: 'string' },
            'events-source': { type: 'string' },
            format: { type: 'string', default: 'ascii' },
          },
        });
        return reportTimelineCmd(
          {
            ...(values.last !== undefined ? { last: Number(values.last) } : {}),
            ...(values.file !== undefined ? { file: values.file } : {}),
            ...(values['events-source'] === 'main' ||
            values['events-source'] === 'hat-channel' ||
            values['events-source'] === 'auto'
              ? { eventsSource: values['events-source'] }
              : {}),
            format: values.format === 'json' ? 'json' : 'ascii',
          },
          ctx,
        );
      }
      case 'compare': {
        const { values, positionals } = parseArgs({
          args, strict: true, allowPositionals: true,
          options: {
            'session-a': { type: 'string' },
            'session-b': { type: 'string' },
            'diagnostics-root': { type: 'string' },
            format: { type: 'string', default: 'md' },
            output: { type: 'string', short: 'o' },
          },
        });
        return reportCompareCmd(
          {
            ...(positionals[0] !== undefined ? { aJson: positionals[0] } : {}),
            ...(positionals[1] !== undefined ? { bJson: positionals[1] } : {}),
            ...(values['session-a'] !== undefined ? { sessionA: values['session-a'] } : {}),
            ...(values['session-b'] !== undefined ? { sessionB: values['session-b'] } : {}),
            ...(values['diagnostics-root'] !== undefined ? { diagnosticsRoot: values['diagnostics-root'] } : {}),
            format: values.format === 'json' ? 'json' : 'md',
            ...(values.output !== undefined ? { output: values.output } : {}),
          },
          ctx,
        );
      }
      default:
        return { code: 2, stdout: '', stderr: `unknown report subcommand: ${String(effectiveSub)}\n\n${USAGE}` };
    }
  }

  if (group === 'help' || group === '--help' || group === '-h' || group === undefined) {
    return { code: group === undefined ? 2 : 0, stdout: USAGE };
  }

  return { code: 2, stdout: '', stderr: `unknown group: ${String(group)}\n\n${USAGE}` };
}

// Only dispatch when executed as the entry point — `package.json`'s
// `main` points here, so importing this module must not run the CLI.
const invokedAsScript = (() => {
  try {
    return (
      process.argv[1] !== undefined &&
      import.meta.url === pathToFileURL(fs.realpathSync(process.argv[1])).href
    );
  } catch {
    return false;
  }
})();

if (invokedAsScript) {
  const code = emit(await dispatch(process.argv.slice(2).filter((a) => a !== '--')));
  process.exitCode = code;
}
