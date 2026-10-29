/**
 * `ralph` CLI child-process wrapper.
 *
 * Spawn contract (mirrors the monitor's client/dispatch seam):
 * argument array, no shell, allowlisted env, bounded timeout with
 * SIGKILL, capped stdout. This is the only place the tool talks to the
 * Rust base — every preset/report command funnels through
 * `runRalph`/`runRalphJson`, so the Rust side stays the single
 * authority for parsing, validation and runtime facts.
 */

import { spawn } from 'node:child_process';
import { buildChildEnv } from './env.js';

export const DEFAULT_TIMEOUT_MS = 30_000;
export const MAX_OUTPUT_BYTES = 4 * 1024 * 1024;
const STDERR_PREVIEW = 2000;

export type RalphErrorKind =
  | 'spawn_error'
  | 'timeout'
  | 'nonzero'
  | 'bad_json'
  | 'payload_too_large';

export interface RalphError {
  kind: RalphErrorKind;
  message: string;
  stderr?: string;
}

export interface RalphRunOptions {
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  timeoutMs?: number;
  maxBytes?: number;
  /// Test seam: override the binary name (default `ralph`).
  ralphBin?: string;
}

export interface RalphProcResult {
  code: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
  oversized: boolean;
  spawnError?: string;
}

export type RalphResult<T = string> =
  | { ok: true; data: T; raw: string; stderr: string; exitCode: number }
  | { ok: false; error: RalphError };

function truncate(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, max)}…` : s;
}

function spawnRalph(
  argv: string[],
  opts: RalphRunOptions,
): Promise<RalphProcResult> {
  const bin = opts.ralphBin ?? 'ralph';
  const env = opts.env ?? buildChildEnv();
  const timeoutMs = opts.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const maxBytes = opts.maxBytes ?? MAX_OUTPUT_BYTES;

  return new Promise((resolve) => {
    let child;
    try {
      // `detached: true` puts the child in its own process group so a
      // timeout can SIGKILL the whole group — otherwise a hung child
      // that spawned grandchildren (e.g. `sleep`) would leak pipes and
      // keep `close` from firing until they exit.
      child = spawn(bin, argv, {
        cwd: opts.cwd,
        env,
        stdio: ['ignore', 'pipe', 'pipe'],
        detached: process.platform !== 'win32',
      });
    } catch (err) {
      resolve({
        code: null,
        stdout: '',
        stderr: '',
        timedOut: false,
        oversized: false,
        spawnError: String(err),
      });
      return;
    }

    const killTree = () => {
      try {
        if (process.platform !== 'win32' && child.pid !== undefined) {
          process.kill(-child.pid, 'SIGKILL');
          return;
        }
      } catch {
        /* group already gone — fall through to direct kill */
      }
      child.kill('SIGKILL');
    };

    const stdoutChunks: Buffer[] = [];
    const stderrChunks: Buffer[] = [];
    let stdoutBytes = 0;
    let timedOut = false;
    let oversized = false;

    const timer = setTimeout(() => {
      timedOut = true;
      killTree();
    }, timeoutMs);

    child.stdout.on('data', (chunk: Buffer) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > maxBytes) {
        oversized = true;
        killTree();
        return;
      }
      stdoutChunks.push(chunk);
    });
    child.stderr.on('data', (chunk: Buffer) => stderrChunks.push(chunk));
    child.on('error', (err) => {
      clearTimeout(timer);
      resolve({
        code: null,
        stdout: '',
        stderr: '',
        timedOut,
        oversized,
        spawnError: err.message,
      });
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({
        code,
        stdout: Buffer.concat(stdoutChunks).toString('utf8'),
        stderr: Buffer.concat(stderrChunks).toString('utf8'),
        timedOut,
        oversized,
      });
    });
  });
}

/** Run `ralph <argv>` and return raw stdout. Non-zero exit → ok:false. */
export async function runRalph(
  argv: string[],
  opts: RalphRunOptions = {},
): Promise<RalphResult<string>> {
  const res = await spawnRalph(argv, opts);
  const stderr = truncate(res.stderr.trim(), STDERR_PREVIEW);

  if (res.spawnError !== undefined) {
    return {
      ok: false,
      error: {
        kind: 'spawn_error',
        message: `failed to spawn ${opts.ralphBin ?? 'ralph'}: ${res.spawnError}`,
        stderr: stderr || undefined,
      },
    };
  }
  if (res.timedOut) {
    return {
      ok: false,
      error: {
        kind: 'timeout',
        message: `ralph ${argv.join(' ')} exceeded ${opts.timeoutMs ?? DEFAULT_TIMEOUT_MS}ms`,
        stderr: stderr || undefined,
      },
    };
  }
  if (res.oversized) {
    return {
      ok: false,
      error: {
        kind: 'payload_too_large',
        message: `ralph ${argv.join(' ')} output exceeded ${opts.maxBytes ?? MAX_OUTPUT_BYTES} bytes`,
        stderr: stderr || undefined,
      },
    };
  }
  if (res.code !== 0) {
    return {
      ok: false,
      error: {
        kind: 'nonzero',
        message: `ralph ${argv.join(' ')} exited with code ${res.code}`,
        stderr: stderr || truncate(res.stdout.trim(), STDERR_PREVIEW) || undefined,
      },
    };
  }
  return { ok: true, data: res.stdout, raw: res.stdout, stderr: res.stderr, exitCode: 0 };
}

/**
 * Run `ralph <argv>` and parse stdout as JSON.
 *
 * `tolerateNonZero`: some subcommands (`preset check`, `preset verify`)
 * print a complete JSON report and then exit non-zero to signal FAIL.
 * When set, a non-zero exit with parseable stdout still resolves to
 * `ok:true` and carries the real exit code on `exitCode`.
 */
export async function runRalphJson<T = unknown>(
  argv: string[],
  opts: RalphRunOptions & { tolerateNonZero?: boolean } = {},
): Promise<RalphResult<T>> {
  const { tolerateNonZero, ...runOpts } = opts;
  const proc = await spawnRalph(argv, runOpts);
  const stderr = truncate(proc.stderr.trim(), STDERR_PREVIEW);

  if (proc.spawnError !== undefined) {
    return {
      ok: false,
      error: {
        kind: 'spawn_error',
        message: `failed to spawn ${opts.ralphBin ?? 'ralph'}: ${proc.spawnError}`,
        stderr: stderr || undefined,
      },
    };
  }
  if (proc.timedOut) {
    return {
      ok: false,
      error: {
        kind: 'timeout',
        message: `ralph ${argv.join(' ')} exceeded ${opts.timeoutMs ?? DEFAULT_TIMEOUT_MS}ms`,
        stderr: stderr || undefined,
      },
    };
  }
  if (proc.oversized) {
    return {
      ok: false,
      error: {
        kind: 'payload_too_large',
        message: `ralph ${argv.join(' ')} output exceeded ${opts.maxBytes ?? MAX_OUTPUT_BYTES} bytes`,
        stderr: stderr || undefined,
      },
    };
  }

  const code = proc.code ?? -1;
  if (code !== 0 && !tolerateNonZero) {
    return {
      ok: false,
      error: {
        kind: 'nonzero',
        message: `ralph ${argv.join(' ')} exited with code ${code}`,
        stderr: stderr || truncate(proc.stdout.trim(), STDERR_PREVIEW) || undefined,
      },
    };
  }

  // Tracing may write single-line WARNs to stdout ahead of the JSON
  // body (e.g. preflight "contains core/runtime keys" notices). Parse
  // the whole payload first; on failure retry from the first line that
  // opens a `{`/`[` — same discipline as dispatch's last-JSON receipt.
  const body = extractJson(proc.stdout);
  if (body !== null) {
    return {
      ok: true,
      data: body as T,
      raw: proc.stdout,
      stderr: proc.stderr,
      exitCode: code,
    };
  }
  if (code !== 0) {
    return {
      ok: false,
      error: {
        kind: 'nonzero',
        message: `ralph ${argv.join(' ')} exited with code ${code}`,
        stderr: stderr || truncate(proc.stdout.trim(), STDERR_PREVIEW) || undefined,
      },
    };
  }
  return {
    ok: false,
    error: {
      kind: 'bad_json',
      message: `ralph ${argv.join(' ')} stdout is not valid JSON`,
      stderr: stderr || truncate(proc.stdout, STDERR_PREVIEW),
    },
  };
}

function extractJson(stdout: string): unknown | null {
  try {
    return JSON.parse(stdout);
  } catch {
    /* fall through to noise-tolerant scan */
  }
  const start = stdout.search(/^[{\[]/m);
  if (start < 0) return null;
  try {
    return JSON.parse(stdout.slice(start));
  } catch {
    return null;
  }
}
