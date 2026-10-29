/**
 * Artifact allowlist (Plan2-U3, D7): a candidate path is readable
 * only when its *resolved* realpath sits inside one of the allowed
 * roots. Resolving through realpath defeats `..` segments and
 * symlink escapes in one step — the comparison always happens on
 * canonical paths.
 */

import fs from 'node:fs';
import path from 'node:path';

export type AllowlistVerdict =
  | { ok: true; resolved: string }
  | { ok: false; code: 'outside_allowed_roots' | 'not_found' | 'not_regular_file' };

/** Is `resolved` inside `root` (already-resolved absolute paths)? */
export function withinRoot(resolved: string, root: string): boolean {
  const rel = path.relative(root, resolved);
  return rel === '' || (!rel.startsWith('..') && !path.isAbsolute(rel));
}

/**
 * Resolve `candidate` (relative → under `base`) and verify the real
 * path is inside one of `allowedRoots` (each already absolute).
 * Non-existent paths → `not_found`; non-regular files (dirs,
 * devices, fifos) → `not_regular_file`.
 */
export function checkAllowed(
  candidate: string,
  base: string,
  allowedRoots: string[],
): AllowlistVerdict {
  const abs = path.isAbsolute(candidate)
    ? candidate
    : path.join(base, candidate);
  let resolved: string;
  try {
    resolved = fs.realpathSync(abs);
  } catch {
    return { ok: false, code: 'not_found' };
  }
  let inside = false;
  for (const root of allowedRoots) {
    let r: string;
    try {
      r = fs.realpathSync(root);
    } catch {
      continue; // root itself absent → contributes nothing
    }
    if (withinRoot(resolved, r)) {
      inside = true;
      break;
    }
  }
  if (!inside) return { ok: false, code: 'outside_allowed_roots' };
  let st: fs.Stats;
  try {
    st = fs.statSync(resolved);
  } catch {
    return { ok: false, code: 'not_found' };
  }
  if (!st.isFile()) return { ok: false, code: 'not_regular_file' };
  return { ok: true, resolved };
}
