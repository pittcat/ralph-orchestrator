import { randomUUID } from 'node:crypto';
import type { MonitorDb } from '../db/index.js';

/**
 * Stable per-installation machine identity used to build
 * `global_run_id = <machine_id>:<project_id>:<run_id>`. Generated on
 * first boot, persisted in the `meta` table, reused forever after.
 */
export function ensureMachineId(db: MonitorDb): string {
  const row = db
    .prepare(`SELECT value FROM meta WHERE key = 'machine_id'`)
    .get() as { value: string } | undefined;
  if (row?.value) {
    return row.value;
  }
  const id = randomUUID();
  db.prepare(`INSERT INTO meta (key, value) VALUES ('machine_id', ?)`).run(id);
  return id;
}
