/**
 * Structural preset diff.
 *
 * Compares two parsed preset documents semantically (hats, sections,
 * scalars, sets) rather than line-by-line — so reordered keys and
 * re-wrapped `instructions:` strings don't drown real changes. Value
 * rendering is capped; `instructions` bodies are summarized by size
 * and first line, not inlined wholesale.
 */

import type { PresetModel } from './model.js';

export interface DiffEntry {
  /** Dotted path, e.g. `hats.fixer.publishes` or `event_loop.max_iterations`. */
  path: string;
  kind: 'added' | 'removed' | 'changed';
  /** Rendered old/new values (absent for added/removed respectively). */
  a?: string;
  b?: string;
}

export interface PresetDiff {
  aRef: string;
  bRef: string;
  hatsAdded: string[];
  hatsRemoved: string[];
  entries: DiffEntry[];
  identical: boolean;
  /** True when `entries` hit the cap and more differences exist. */
  truncated: boolean;
}

const MAX_ENTRIES = 200;
const MAX_VALUE_RENDER = 160;

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function renderValue(v: unknown): string {
  let s: string;
  if (typeof v === 'string') {
    const firstLine = v.split('\n')[0];
    s = v.includes('\n')
      ? JSON.stringify(`${firstLine}…`) + ` (${v.length} chars, ${v.split('\n').length} lines)`
      : JSON.stringify(v);
  } else {
    s = JSON.stringify(v);
  }
  if (s === undefined) return 'undefined';
  return s.length > MAX_VALUE_RENDER ? `${s.slice(0, MAX_VALUE_RENDER)}…` : s;
}

function setDiff(a: unknown[], b: unknown[]): { added: string[]; removed: string[] } {
  const sa = new Set(a.map((x) => JSON.stringify(x)));
  const sb = new Set(b.map((x) => JSON.stringify(x)));
  return {
    added: b.filter((x) => !sa.has(JSON.stringify(x))).map((x) => renderValue(x)),
    removed: a.filter((x) => !sb.has(JSON.stringify(x))).map((x) => renderValue(x)),
  };
}

function diffValues(
  a: unknown,
  b: unknown,
  path: string,
  out: DiffEntry[],
  limit: { hit: boolean },
): void {
  if (out.length >= MAX_ENTRIES) {
    limit.hit = true;
    return;
  }
  if (a === undefined && b !== undefined) {
    out.push({ path, kind: 'added', b: renderValue(b) });
    return;
  }
  if (b === undefined && a !== undefined) {
    out.push({ path, kind: 'removed', a: renderValue(a) });
    return;
  }
  if (isPlainObject(a) && isPlainObject(b)) {
    for (const key of new Set([...Object.keys(a), ...Object.keys(b)])) {
      diffValues(a[key], b[key], path ? `${path}.${key}` : key, out, limit);
      if (limit.hit) return;
    }
    return;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    const { added, removed } = setDiff(a, b);
    if (removed.length > 0) {
      out.push({ path, kind: 'removed', a: removed.join(', ') });
      if (out.length >= MAX_ENTRIES) {
        limit.hit = true;
        return;
      }
    }
    if (added.length > 0) {
      out.push({ path, kind: 'added', b: added.join(', ') });
      if (out.length >= MAX_ENTRIES) {
        limit.hit = true;
        return;
      }
    }
    return;
  }
  if (JSON.stringify(a) !== JSON.stringify(b)) {
    out.push({ path, kind: 'changed', a: renderValue(a), b: renderValue(b) });
  }
}

/**
 * Diff two preset models: hats are compared as a keyed map; every
 * other top-level section is compared recursively.
 */
export function diffPresets(a: PresetModel, b: PresetModel): PresetDiff {
  const entries: DiffEntry[] = [];
  const limit = { hit: false };

  const aHatIds = new Set(Object.keys(isPlainObject(a.doc.hats) ? a.doc.hats : {}));
  const bHatIds = new Set(Object.keys(isPlainObject(b.doc.hats) ? b.doc.hats : {}));

  const hatsAdded = [...bHatIds].filter((id) => !aHatIds.has(id)).sort();
  const hatsRemoved = [...aHatIds].filter((id) => !bHatIds.has(id)).sort();

  const aHats = isPlainObject(a.doc.hats) ? a.doc.hats : {};
  const bHats = isPlainObject(b.doc.hats) ? b.doc.hats : {};
  for (const id of [...aHatIds].filter((x) => bHatIds.has(x)).sort()) {
    diffValues(aHats[id], bHats[id], `hats.${id}`, entries, limit);
  }

  const aSections = new Set(Object.keys(a.doc).filter((k) => k !== 'hats'));
  const bSections = new Set(Object.keys(b.doc).filter((k) => k !== 'hats'));
  for (const key of new Set([...aSections, ...bSections])) {
    diffValues(a.doc[key], b.doc[key], key, entries, limit);
  }

  return {
    aRef: a.ref,
    bRef: b.ref,
    hatsAdded,
    hatsRemoved,
    entries,
    identical: hatsAdded.length === 0 && hatsRemoved.length === 0 && entries.length === 0,
    truncated: limit.hit,
  };
}

export function renderDiff(diff: PresetDiff): string {
  const lines: string[] = [];
  lines.push(`diff: ${diff.aRef}  →  ${diff.bRef}`);
  if (diff.identical) {
    lines.push('  (no structural differences)');
    return lines.join('\n');
  }
  for (const id of diff.hatsAdded) lines.push(`+ hat ${id}`);
  for (const id of diff.hatsRemoved) lines.push(`- hat ${id}`);
  for (const e of diff.entries) {
    if (e.kind === 'added') lines.push(`+ ${e.path}: ${e.b}`);
    else if (e.kind === 'removed') lines.push(`- ${e.path}: ${e.a}`);
    else lines.push(`~ ${e.path}: ${e.a} → ${e.b}`);
  }
  if (diff.truncated) {
    lines.push(`… (truncated at ${MAX_ENTRIES} entries)`);
  }
  return lines.join('\n');
}
