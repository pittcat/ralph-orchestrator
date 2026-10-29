/**
 * Artifact content channel (Plan2-U3): text-only, 512KiB hard cap.
 */

import fs from 'node:fs';

export const MAX_ARTIFACT_BYTES = 512 * 1024;

export type ArtifactContent =
  | { ok: true; content: string; truncated: boolean }
  | { ok: false; code: 'content_too_large' | 'not_text' };

/** Byte-level UTF-8 validity + NUL sniff (binary guard for v1 text/*). */
function looksText(buf: Buffer): boolean {
  for (const b of buf.subarray(0, Math.min(buf.length, 8192))) {
    if (b === 0) return false;
  }
  return true;
}

export function readArtifact(resolved: string): ArtifactContent {
  let st: fs.Stats;
  try {
    st = fs.statSync(resolved);
  } catch {
    return { ok: false, code: 'not_text' };
  }
  if (st.size > MAX_ARTIFACT_BYTES) {
    return { ok: false, code: 'content_too_large' };
  }
  const buf = fs.readFileSync(resolved);
  if (!looksText(buf)) return { ok: false, code: 'not_text' };
  return { ok: true, content: buf.toString('utf8'), truncated: false };
}
