/**
 * Preset catalog (Plan5-U2, D10): builtin list via
 * `ralph preset builtin list --format json` (argv spawn, cached on
 * the projects row for 60s) + `.ralph/hats/*.yml` enumeration for
 * custom hats. A catalog failure is non-fatal — empty lists, no 5xx.
 */

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { z } from 'zod';
import type { MonitorDb } from '../db/index.js';
import { buildChildEnv } from '../ralph/client.js';

export const PRESETS_CACHE_MS = 60_000;

const builtinListSchema = z.array(
  z.object({ name: z.string(), description: z.string().optional() }),
);

export interface PresetEntry {
  id: string;
  name: string;
  source: 'builtin' | 'custom';
  description?: string;
}

export interface PresetCatalog {
  presets: PresetEntry[];
  fetched_at: string;
  builtin_error: string | null;
}

export interface CatalogDeps {
  ralphBin?: string;
  env?: NodeJS.ProcessEnv;
  now?: () => Date;
}

/** Custom hats under `.ralph/hats/` — filename stem is the id. */
export function customHats(projectDir: string): PresetEntry[] {
  const dir = path.join(projectDir, '.ralph', 'hats');
  let names: string[];
  try {
    names = fs.readdirSync(dir);
  } catch {
    return [];
  }
  return names
    .filter((n) => /\.ya?ml$/i.test(n))
    .map((n) => n.replace(/\.ya?ml$/i, ''))
    .sort()
    .map((stem) => ({ id: `directory:${stem}`, name: stem, source: 'custom' as const }));
}

/**
 * `GET /projects/:id/presets`. Fresh builtin rows come from the
 * runtime's own listing; the projects row cache carries the last good
 * fetch so a transient spawn failure doesn't blank the catalog.
 */
export function presetCatalog(
  db: MonitorDb,
  projectId: string,
  deps: CatalogDeps = {},
): PresetCatalog {
  const row = db
    .prepare(
      'SELECT canonical_path, presets_cache, presets_at FROM projects WHERE id = ?',
    )
    .get(projectId) as
    | {
        canonical_path: string;
        presets_cache: string | null;
        presets_at: string | null;
      };
  const now = (deps.now?.() ?? new Date());
  const custom = customHats(row.canonical_path);

  const cacheAge = row.presets_at
    ? now.getTime() - Date.parse(row.presets_at)
    : Infinity;
  let builtin: PresetEntry[] = [];
  let builtinError: string | null = null;

  if (Number.isFinite(cacheAge) && cacheAge < PRESETS_CACHE_MS && row.presets_cache) {
    builtin = JSON.parse(row.presets_cache) as PresetEntry[];
  } else {
    const res = spawnSync(
      deps.ralphBin ?? 'ralph',
      ['preset', 'builtin', 'list', '--format', 'json'],
      {
        cwd: row.canonical_path,
        env: deps.env ?? buildChildEnv(),
        timeout: 10_000,
        encoding: 'utf8',
        maxBuffer: 1024 * 1024,
      },
    );
    if (res.error || res.status !== 0) {
      builtinError = res.error
        ? `spawn: ${res.error.message}`
        : `exit ${res.status}: ${(res.stderr ?? '').trim().slice(0, 500)}`;
      builtin = row.presets_cache
        ? (JSON.parse(row.presets_cache) as PresetEntry[])
        : [];
    } else {
      const parsed = builtinListSchema.safeParse(JSON.parse(res.stdout));
      if (!parsed.success) {
        builtinError = 'unexpected builtin list payload';
        builtin = row.presets_cache
          ? (JSON.parse(row.presets_cache) as PresetEntry[])
          : [];
      } else {
        builtin = parsed.data.map((b) => ({
          id: `builtin:${b.name}`,
          name: b.name,
          source: 'builtin' as const,
          ...(b.description !== undefined ? { description: b.description } : {}),
        }));
        db.prepare(
          'UPDATE projects SET presets_cache = ?, presets_at = ? WHERE id = ?',
        ).run(JSON.stringify(builtin), now.toISOString(), projectId);
      }
    }
  }

  return {
    presets: [...builtin, ...custom],
    fetched_at: now.toISOString(),
    builtin_error: builtinError,
  };
}
