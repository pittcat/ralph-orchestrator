/**
 * Artifact index (Plan2-U3): declared descriptor slots ∪ builtin
 * slots ∪ shallow one-level enumeration of `.ralph/review/` and
 * `.ralph/post-merge/`. Every slot is validated through the realpath
 * allowlist; paths outside the allowed roots surface as
 * `outside_allowed` rather than being silently dropped (D10: no
 * path inference from event payloads — only declared/probed slots).
 */

import fs from 'node:fs';
import path from 'node:path';
import { checkAllowed } from '../lib/allowlist.js';

export interface ArtifactSlot {
  id: string;
  kind: string;
  /** candidate path relative to the run workspace (or absolute). */
  candidate: string;
}

export interface ArtifactEntry {
  id: string;
  kind: string;
  path?: string; // display path relative to workspace
  status: 'present' | 'not_generated' | 'outside_allowed';
  size?: number;
  mtime?: string;
}

export interface RunArtifactContext {
  workspace: string;
  repoRoot: string | null;
  planPath: string | null;
  descriptor: Record<string, unknown> | null;
}

/// Builtin slots always probed (E8/E14 real layout).
const BUILTIN_SLOTS: ArtifactSlot[] = [
  { id: 'summary', kind: 'markdown', candidate: '.ralph/agent/summary.md' },
  { id: 'handoff', kind: 'markdown', candidate: '.ralph/agent/handoff.md' },
  { id: 'diagnosis', kind: 'json', candidate: '.ralph/agent/diagnosis-summary.json' },
];

export function kindOf(file: string): string {
  if (file.endsWith('.md')) return 'markdown';
  if (file.endsWith('.json')) return 'json';
  if (file.endsWith('.yml') || file.endsWith('.yaml')) return 'yaml';
  return 'text';
}

function descriptorSlots(descriptor: Record<string, unknown> | null): ArtifactSlot[] {
  const list = descriptor?.artifacts;
  if (!Array.isArray(list)) return [];
  const out: ArtifactSlot[] = [];
  for (const item of list) {
    if (typeof item !== 'object' || item === null) continue;
    const it = item as Record<string, unknown>;
    const id = typeof it.id === 'string' ? it.id : null;
    const glob = typeof it.path_glob === 'string' ? it.path_glob : null;
    if (!id || !glob || glob.includes('*')) {
      // v1: only literal paths supported as slots; globs are skipped.
      if (id && glob && !glob.includes('*')) out.push({ id, kind: kindOf(glob), candidate: glob });
      continue;
    }
    out.push({ id, kind: typeof it.kind === 'string' ? it.kind : kindOf(glob), candidate: glob });
  }
  return out;
}

/** Shallow (one-level) enumeration of a `.ralph` subdir. */
function enumerateDir(
  workspace: string,
  subdir: 'review' | 'post-merge',
): ArtifactSlot[] {
  const dir = path.join(workspace, '.ralph', subdir);
  let names: string[] = [];
  try {
    names = fs.readdirSync(dir);
  } catch {
    return [];
  }
  return names
    .filter((n) => !n.startsWith('.'))
    .sort()
    .map((n) => ({
      id: `${subdir}/${n}`,
      kind: kindOf(n),
      candidate: `.ralph/${subdir}/${n}`,
    }));
}

/** All slots for a run, deduped by id (first declaration wins). */
export function buildSlots(ctx: RunArtifactContext): ArtifactSlot[] {
  const seen = new Set<string>();
  const slots: ArtifactSlot[] = [];
  for (const s of [
    ...descriptorSlots(ctx.descriptor),
    ...BUILTIN_SLOTS,
    ...enumerateDir(ctx.workspace, 'review'),
    ...enumerateDir(ctx.workspace, 'post-merge'),
  ]) {
    if (seen.has(s.id)) continue;
    seen.add(s.id);
    slots.push(s);
  }
  return slots;
}

/**
 * Allowed roots (D7): `{workspace/.ralph, repo_root/docs,
 * plan_path's dir}`. plan_path may be relative to the workspace.
 */
export function allowedRoots(ctx: RunArtifactContext): string[] {
  const roots = [path.join(ctx.workspace, '.ralph')];
  if (ctx.repoRoot) roots.push(path.join(ctx.repoRoot, 'docs'));
  if (ctx.planPath) {
    const planAbs = path.isAbsolute(ctx.planPath)
      ? ctx.planPath
      : path.join(ctx.workspace, ctx.planPath);
    roots.push(path.dirname(planAbs));
  }
  return roots;
}

export function displayPath(ctx: RunArtifactContext, resolved: string): string {
  const rel = path.relative(ctx.workspace, resolved);
  return rel.startsWith('..') ? resolved : rel;
}

export function indexArtifacts(ctx: RunArtifactContext): ArtifactEntry[] {
  const roots = allowedRoots(ctx);
  return buildSlots(ctx).map((slot) => {
    const verdict = checkAllowed(slot.candidate, ctx.workspace, roots);
    if (!verdict.ok) {
      return {
        id: slot.id,
        kind: slot.kind,
        status:
          verdict.code === 'outside_allowed_roots'
            ? ('outside_allowed' as const)
            : ('not_generated' as const),
      };
    }
    const st = fs.statSync(verdict.resolved);
    return {
      id: slot.id,
      kind: slot.kind,
      path: displayPath(ctx, verdict.resolved),
      status: 'present' as const,
      size: st.size,
      mtime: st.mtime.toISOString(),
    };
  });
}

/** Resolve an artifact id to its allowed path (content endpoint). */
export function resolveArtifact(
  ctx: RunArtifactContext,
  artifactId: string,
): { slot: ArtifactSlot; resolved: string } | { error: 'not_found' | 'outside_allowed_roots' | 'not_regular_file' } {
  const slot = buildSlots(ctx).find((s) => s.id === artifactId);
  if (!slot) return { error: 'not_found' };
  const verdict = checkAllowed(slot.candidate, ctx.workspace, allowedRoots(ctx));
  if (!verdict.ok) return { error: verdict.code };
  return { slot, resolved: verdict.resolved };
}
