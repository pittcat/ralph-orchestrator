/**
 * Preset document model: load a preset YAML from a file path or a
 * `builtin:<id>` reference and project it into the fields the dev
 * workflow inspects (hats, triggers, publishes, event-loop knobs).
 *
 * The projection is intentionally non-authoritative — it shows what is
 * declared, never whether it is *valid*. Validity is Rust's job via
 * `ralph preset check` / `ralph preset verify`.
 */

import fs from 'node:fs';
import path from 'node:path';
import { parseDocument } from 'yaml';
import { runRalph } from '../ralph/runner.js';

export interface HatModel {
  id: string;
  name?: string;
  description?: string;
  triggers: string[];
  publishes: string[];
  terminalEvents: string[];
  defaultPublishes?: string;
  concurrency?: number;
  hasAggregate: boolean;
  instructionsChars: number;
  /** All other keys declared on the hat, for the diff/inspect surface. */
  extraKeys: string[];
}

export interface PresetModel {
  /** How the preset was addressed: file path or `builtin:<id>`. */
  ref: string;
  source: 'file' | 'builtin';
  /** Raw YAML text (byte-exact for builtins). */
  yaml: string;
  /** Parsed YAML root mapping. */
  doc: Record<string, unknown>;
  /** YAML parse diagnostics — surfaced to the user, never fatal here. */
  yamlErrors: string[];
  hats: HatModel[];
  eventLoop: Record<string, unknown> | undefined;
  /** Top-level sections present besides `hats`. */
  sections: string[];
}

const KNOWN_HAT_KEYS = new Set([
  'name',
  'description',
  'triggers',
  'publishes',
  'terminal_events',
  'default_publishes',
  'concurrency',
  'aggregate',
  'instructions',
]);

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((v): v is string => typeof v === 'string');
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function projectHat(id: string, body: unknown): HatModel {
  const h = isPlainObject(body) ? body : {};
  const extraKeys = Object.keys(h).filter((k) => !KNOWN_HAT_KEYS.has(k));
  const defaultPublishes =
    typeof h.default_publishes === 'string' ? h.default_publishes : undefined;
  return {
    id,
    ...(typeof h.name === 'string' ? { name: h.name } : {}),
    ...(typeof h.description === 'string' ? { description: h.description } : {}),
    triggers: asStringArray(h.triggers),
    publishes: asStringArray(h.publishes),
    terminalEvents: asStringArray(h.terminal_events),
    ...(defaultPublishes !== undefined ? { defaultPublishes } : {}),
    ...(typeof h.concurrency === 'number' ? { concurrency: h.concurrency } : {}),
    hasAggregate: h.aggregate !== undefined,
    instructionsChars: typeof h.instructions === 'string' ? h.instructions.length : 0,
    extraKeys,
  };
}

function toDoc(parsed: unknown): Record<string, unknown> {
  return isPlainObject(parsed) ? parsed : {};
}

export function modelFromYaml(
  yamlText: string,
  ref: string,
  source: 'file' | 'builtin',
): PresetModel {
  const parsed = parseDocument(yamlText);
  const yamlErrors = parsed.errors.map((e) => e.message);
  const doc = toDoc(parsed.toJS());

  const hatsSection = isPlainObject(doc.hats) ? doc.hats : {};
  const hats = Object.entries(hatsSection)
    .filter(([id]) => !id.startsWith('x-'))
    .map(([id, body]) => projectHat(id, body));

  const eventLoop = isPlainObject(doc.event_loop)
    ? (doc.event_loop as Record<string, unknown>)
    : undefined;

  const sections = Object.keys(doc).filter((k) => k !== 'hats');

  return { ref, source, yaml: yamlText, doc, yamlErrors, hats, eventLoop, sections };
}

export type LoadResult =
  | { ok: true; model: PresetModel }
  | { ok: false; error: string };

export interface LoadOptions {
  ralphBin?: string;
  env?: NodeJS.ProcessEnv;
  cwd?: string;
}

/**
 * Resolve a preset reference:
 * - `builtin:<id>` or bare `builtin:` name → `ralph preset builtin show <id> --format yaml`
 * - otherwise a filesystem path (resolved against `opts.cwd`).
 */
export async function loadPreset(
  ref: string,
  opts: LoadOptions = {},
): Promise<LoadResult> {
  if (ref.startsWith('builtin:')) {
    const id = ref.slice('builtin:'.length);
    if (!id) return { ok: false, error: 'empty builtin preset id' };
    const res = await runRalph(
      ['preset', 'builtin', 'show', id, '--format', 'yaml'],
      { cwd: opts.cwd, env: opts.env, ralphBin: opts.ralphBin },
    );
    if (!res.ok) {
      return {
        ok: false,
        error: `${res.error.message}${res.error.stderr ? ` — ${res.error.stderr}` : ''}`,
      };
    }
    return { ok: true, model: modelFromYaml(res.data, ref, 'builtin') };
  }

  const filePath = path.resolve(opts.cwd ?? process.cwd(), ref);
  let yamlText: string;
  try {
    yamlText = fs.readFileSync(filePath, 'utf8');
  } catch (err) {
    return { ok: false, error: `cannot read preset file ${filePath}: ${String(err)}` };
  }
  return { ok: true, model: modelFromYaml(yamlText, filePath, 'file') };
}
