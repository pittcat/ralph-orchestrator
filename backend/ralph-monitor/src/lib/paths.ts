import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export interface PathOk {
  ok: true;
  canonical: string;
}

export interface PathErr {
  ok: false;
  code: 'invalid_path';
  message: string;
}

export type NormalizeResult = PathOk | PathErr;

const MAX_PATH_LEN = 4096;

function err(message: string): PathErr {
  return { ok: false, code: 'invalid_path', message };
}

/**
 * Normalize a user-supplied project path to its canonical absolute
 * form (symlinks resolved, trailing separators dropped).
 *
 * Rejected with `invalid_path` (→ HTTP 422):
 * - empty / over-long (>4096) / NUL-containing input
 * - `..` path segments (traversal — the monitor never needs them)
 * - `~user` expansion (ambiguous); `~/` expands via os.homedir()
 * - UNC-style `//` or `\\` prefixes
 * - nonexistent paths, non-directories, symlink loops, unreadable dirs
 */
export function normalizeUserPath(
  input: string,
  cwd: string = process.cwd(),
): NormalizeResult {
  if (input.length === 0 || input.length > MAX_PATH_LEN) {
    return err('path is empty or exceeds the maximum length');
  }
  if (input.includes('\0')) {
    return err('path contains a NUL byte');
  }
  if (input.startsWith('//') || input.startsWith('\\\\')) {
    return err('UNC-style paths are not supported');
  }

  let expanded = input;
  if (expanded === '~' || expanded.startsWith('~/')) {
    expanded = path.join(
      os.homedir(),
      expanded === '~' ? '' : expanded.slice(2),
    );
  } else if (expanded.startsWith('~')) {
    return err('~user home expansion is not supported');
  }

  const segments = expanded.split(/[\\/]+/);
  if (segments.includes('..')) {
    return err('path contains a `..` traversal segment');
  }

  let canonical: string;
  try {
    // realpathSync.native resolves symlinks and detects loops; it
    // throws ENOENT for missing paths which we map to invalid_path.
    canonical = fs.realpathSync.native(path.resolve(cwd, expanded));
  } catch {
    return err(`路径不存在或无法解析: ${input}`);
  }

  let stat: fs.Stats;
  try {
    stat = fs.statSync(canonical);
  } catch {
    return err(`路径不存在或无法解析: ${input}`);
  }
  if (!stat.isDirectory()) {
    return err(`路径存在但不是目录: ${input}`);
  }
  try {
    fs.accessSync(canonical, fs.constants.R_OK);
  } catch {
    return err(`目录不可读: ${input}`);
  }

  return { ok: true, canonical };
}

/**
 * Whether `dir` is a git repository root (or a linked worktree —
 * worktrees carry a `.git` *file* rather than a directory). Used by
 * project registration (S8): a non-git directory can never produce
 * ralph runs worth monitoring.
 */
export function isGitRepo(dir: string): boolean {
  try {
    return fs.existsSync(path.join(dir, '.git'));
  } catch {
    return false;
  }
}
