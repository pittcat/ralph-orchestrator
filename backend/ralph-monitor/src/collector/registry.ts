/**
 * Auto-registration dropbox (Plan: monitor auto-register): `ralph run`
 * writes `~/.ralph/projects.d/<sha256(canonical_path)>.json`; the
 * collector imports every file in that directory each round so any
 * directory that has ever hosted a run shows up in the dashboard
 * without a manual "Add project".
 *
 * DELETE /projects also removes the drop file so a deleted project is
 * not re-imported — the next `ralph run` in that directory re-creates
 * the file and re-registers, which is the desired semantic.
 */

import { createHash, randomUUID } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { z } from 'zod';
import type { MonitorDb } from '../db/index.js';
import { isGitRepo, normalizeUserPath } from '../lib/paths.js';

/** `$RALPH_MONITOR_REGISTRY_DIR` else `~/.ralph/projects.d`. */
export function defaultRegistryDir(
  env: NodeJS.ProcessEnv = process.env,
): string {
  const override = env.RALPH_MONITOR_REGISTRY_DIR;
  if (override && override.trim() !== '') return override;
  return path.join(os.homedir(), '.ralph', 'projects.d');
}

/** sha256 hex of the canonical path — identical to the Rust side
 *  (`ralph_core::monitor_registration::registration_file_name`). */
export function registrationFileName(canonicalPath: string): string {
  return (
    createHash('sha256').update(canonicalPath, 'utf8').digest('hex') + '.json'
  );
}

const registrationSchema = z
  .object({
    path: z.string().min(1),
    displayName: z.string().min(1).max(200).optional(),
  })
  .passthrough();

export interface RegistryImportSummary {
  files: number;
  imported: number;
  alreadyRegistered: number;
  skipped: number;
}

/**
 * Scan the dropbox and insert project rows for valid, not-yet-registered
 * paths. Malformed files and non-git paths are skipped (left in place —
 * a directory that later becomes a git repo then registers).
 */
export function importProjectRegistrations(
  db: MonitorDb,
  dir: string = defaultRegistryDir(),
): RegistryImportSummary {
  const summary: RegistryImportSummary = {
    files: 0,
    imported: 0,
    alreadyRegistered: 0,
    skipped: 0,
  };
  let names: string[];
  try {
    names = fs
      .readdirSync(dir)
      .filter((n) => n.endsWith('.json') && !n.startsWith('.'));
  } catch {
    return summary; // No dropbox yet — nothing to import.
  }

  const byCanonical = db.prepare(
    'SELECT id FROM projects WHERE canonical_path = ?',
  );
  const insert = db.prepare(
    'INSERT INTO projects (id, canonical_path, display_name, created_at) VALUES (?, ?, ?, ?)',
  );
  const audit = db.prepare(
    'INSERT INTO audit_log (at, project_id, action, details) VALUES (?, ?, ?, ?)',
  );

  for (const name of names) {
    summary.files += 1;
    try {
      const parsed = registrationSchema.safeParse(
        JSON.parse(fs.readFileSync(path.join(dir, name), 'utf8')),
      );
      if (!parsed.success) {
        summary.skipped += 1;
        continue;
      }
      const normalized = normalizeUserPath(parsed.data.path);
      if (!normalized.ok || !isGitRepo(normalized.canonical)) {
        summary.skipped += 1;
        continue;
      }
      if (byCanonical.get(normalized.canonical)) {
        summary.alreadyRegistered += 1;
        continue;
      }
      const projectId = `proj_${randomUUID()}`;
      const now = new Date().toISOString();
      insert.run(
        projectId,
        normalized.canonical,
        parsed.data.displayName ?? path.basename(normalized.canonical),
        now,
      );
      audit.run(
        now,
        projectId,
        'project_auto_registered',
        JSON.stringify({ canonicalPath: normalized.canonical, file: name }),
      );
      summary.imported += 1;
    } catch {
      summary.skipped += 1;
    }
  }
  return summary;
}

/** Tombstone a project's drop file (DELETE /projects). Without this the
 *  next import round would resurrect the just-deleted project. */
export function removeProjectRegistration(
  canonicalPath: string,
  dir: string = defaultRegistryDir(),
): void {
  try {
    fs.rmSync(path.join(dir, registrationFileName(canonicalPath)), {
      force: true,
    });
  } catch {
    // Best-effort: a missing dir/file is already the desired state.
  }
}
