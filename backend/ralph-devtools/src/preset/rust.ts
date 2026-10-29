/**
 * Thin wrappers over the Rust `ralph preset …` surface. TS assembles
 * argv and renders; parsing, contract validation and scenario
 * execution stay in Rust (no duplicated authority — see project rules:
 * TS adds editing aid, never a second copy of `preset_lint`).
 */

import { runRalph, runRalphJson, type RalphResult, type RalphRunOptions } from '../ralph/runner.js';

// ── Types mirroring Rust JSON contracts ─────────────────────────────────────

export interface BuiltinListItem {
  id: string;
  source: string;
  description: string;
  public: boolean;
}

export interface BuiltinListEnvelope {
  presets: BuiltinListItem[];
}

export type FindingSeverity = 'pass' | 'warn' | 'error';
export type FindingSource = string;
export type FindingStage = 'authoring' | 'preflight' | 'run_hard_gate' | string;

export interface ContractFinding {
  id: string;
  source: FindingSource;
  severity: FindingSeverity;
  stage: FindingStage;
  message: string;
  details?: Record<string, string>;
  action_hint?: string;
}

export interface ContractReport {
  source_label: string;
  payload_strict: boolean;
  fail_on_warnings: boolean;
  passed: boolean;
  warnings: number;
  errors: number;
  findings: ContractFinding[];
  checked_at: string;
}

export interface VerifyReport {
  passed?: boolean;
  static_layer?: { passed: boolean; warnings: number; errors: number; findings: string[] };
  scenario_layer?: Record<string, unknown>;
  [k: string]: unknown;
}

export interface TemplateManifest {
  name?: string;
  description?: string;
  difficulty?: string;
  category?: string;
  source?: string;
  [k: string]: unknown;
}

// ── Wrappers ────────────────────────────────────────────────────────────────

function configArgs(configs: string[] | undefined): string[] {
  const args: string[] = [];
  for (const c of configs ?? []) args.push('-c', c);
  return args;
}

/** `ralph preset builtin list --format json` */
export function builtinList(opts: RalphRunOptions = {}) {
  return runRalphJson<BuiltinListEnvelope>(
    ['preset', 'builtin', 'list', '--format', 'json'],
    opts,
  );
}

/** `ralph preset builtin show <id> --format yaml` (byte-exact content). */
export function builtinShowYaml(id: string, opts: RalphRunOptions = {}) {
  return runRalph(['preset', 'builtin', 'show', id, '--format', 'yaml'], opts);
}

/** `ralph preset list --format json` — template catalog. */
export function templateList(opts: RalphRunOptions = {}) {
  return runRalphJson<TemplateManifest[]>(
    ['preset', 'list', '--format', 'json'],
    opts,
  );
}

/**
 * `ralph -H <file> preset check --format json [--strict]`.
 *
 * The hats file is addressed via the global `-H` flag so check treats
 * it exactly as `ralph run -H <file>` would load it — including the
 * adjacent `presets/schemas/<stem>.yml` SSOT merge for repo layouts.
 */
export function presetCheck(
  file: string,
  args: { strict?: boolean; config?: string[] } = {},
  opts: RalphRunOptions = {},
): Promise<RalphResult<ContractReport>> {
  const argv = ['-H', file, ...configArgs(args.config), 'preset', 'check', '--format', 'json'];
  if (args.strict) argv.push('--strict');
  // A FAIL verdict still prints the full JSON report before exiting 1.
  return runRalphJson<ContractReport>(argv, { ...opts, tolerateNonZero: true });
}

/**
 * `ralph -H <file> preset verify --scenario <s> --format json`.
 *
 * Scenario verification replays scripted event fixtures through the
 * runtime — substantially slower than `check`, hence the wider budget.
 */
export function presetVerify(
  file: string,
  scenario: string,
  args: { config?: string[] } = {},
  opts: RalphRunOptions = {},
): Promise<RalphResult<VerifyReport>> {
  return runRalphJson<VerifyReport>(
    [
      '-H', file, ...configArgs(args.config),
      'preset', 'verify', '--scenario', scenario, '--format', 'json',
    ],
    { tolerateNonZero: true, timeoutMs: 180_000, ...opts },
  );
}

/** `ralph preset new <template> [--name] [--output] [--force] [--check] --format json` */
export function presetNew(
  template: string,
  args: { name?: string; output?: string; force?: boolean; check?: boolean } = {},
  opts: RalphRunOptions = {},
): Promise<RalphResult<unknown>> {
  const argv = ['preset', 'new', template, '--format', 'json'];
  if (args.name !== undefined) argv.push('--name', args.name);
  if (args.output !== undefined) argv.push('--output', args.output);
  if (args.force) argv.push('--force');
  if (args.check) argv.push('--check');
  return runRalphJson(argv, opts);
}

/** `ralph preset diff --file <f> --format json` (template-baseline diff). */
export function presetBaselineDiff(
  file: string,
  opts: RalphRunOptions = {},
): Promise<RalphResult<unknown>> {
  return runRalphJson(['preset', 'diff', '--file', file, '--format', 'json'], opts);
}

/** `ralph preset upgrade --file <f> --format json` */
export function presetUpgrade(
  file: string,
  opts: RalphRunOptions = {},
): Promise<RalphResult<unknown>> {
  return runRalphJson(['preset', 'upgrade', '--file', file, '--format', 'json'], opts);
}
