/**
 * Runtime webhook duplicate hint (Plan4-U2, D7/S13): read-only probe
 * of `<project>/ralph.yml` `notifications:` — the runtime's own
 * best-effort webhook config (E3) coexists with the monitor's
 * reliable channel, so the UI warns when both may fire. The monitor
 * NEVER rewrites the project config.
 */

import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse as parseYaml } from 'yaml';

export type RuntimeWebhookHint = 'enabled' | 'disabled' | 'none' | 'unknown';

export function readRuntimeWebhookHint(projectRoot: string): RuntimeWebhookHint {
  const file = join(projectRoot, 'ralph.yml');
  if (!existsSync(file)) return 'none';
  let doc: unknown;
  try {
    doc = parseYaml(readFileSync(file, 'utf8'));
  } catch {
    return 'unknown';
  }
  if (typeof doc !== 'object' || doc === null) return 'none';
  const n = (doc as Record<string, unknown>).notifications;
  if (typeof n !== 'object' || n === null) return 'none';
  const enabled = (n as Record<string, unknown>).enabled;
  if (enabled === true) return 'enabled';
  if (enabled === false) return 'disabled';
  return 'none';
}
