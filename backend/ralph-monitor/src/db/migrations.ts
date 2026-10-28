import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import type { MonitorDb } from './index.js';

// src/db/migrations.ts → <pkg>/migrations (also dist/db → <pkg>/migrations
// after `tsc -p tsconfig.build.json` since outDir mirrors the src layout).
export const MIGRATIONS_DIR = fileURLToPath(
  new URL('../../migrations', import.meta.url),
);

const VERSION_RE = /^(\d{4}_.+)\.sql$/;

/**
 * Apply pending migrations in filename order. Idempotent — already
 * applied versions are skipped. A database carrying a version not
 * present among the migration files refuses to start (fail-closed:
 * never silently run against a schema newer than the code).
 */
export function runMigrations(
  db: MonitorDb,
  dir: string = MIGRATIONS_DIR,
): void {
  const files = fs
    .readdirSync(dir)
    .filter((f) => VERSION_RE.test(f))
    .sort();
  if (files.length === 0) {
    throw new Error(`no migrations found in ${dir}`);
  }

  db.exec(
    `CREATE TABLE IF NOT EXISTS schema_migrations (
       version TEXT PRIMARY KEY,
       applied_at TEXT NOT NULL
     )`,
  );

  const applied = new Set(
    (db.prepare('SELECT version FROM schema_migrations').all() as {
      version: string;
    }[]).map((r) => r.version),
  );
  const fileVersions = new Set(files.map((f) => VERSION_RE.exec(f)![1]));
  for (const version of applied) {
    if (!fileVersions.has(version)) {
      throw new Error(
        `database records unknown migration version ${version}; refusing to start`,
      );
    }
  }

  const insert = db.prepare(
    'INSERT INTO schema_migrations (version, applied_at) VALUES (?, ?)',
  );
  for (const file of files) {
    const version = VERSION_RE.exec(file)![1];
    if (applied.has(version)) continue;
    const sql = fs.readFileSync(path.join(dir, file), 'utf8');
    db.transaction(() => {
      db.exec(sql);
      insert.run(version, new Date().toISOString());
    })();
  }
}
