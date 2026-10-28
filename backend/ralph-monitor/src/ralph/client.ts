import { spawn } from 'node:child_process';

/**
 * `ralph inspect run --format json` child-process wrapper (plan D3:
 * the versioned CLI snapshot is the single observation surface — the
 * monitor never reads `.ralph/*` files itself).
 *
 * Spawn contract: argument array, no shell, restricted env allowlist,
 * 10 s timeout with SIGKILL, 2 MB stdout cap.
 */

export const DEFAULT_INSPECT_TIMEOUT_MS = 10_000;
export const MAX_SNAPSHOT_BYTES = 2 * 1024 * 1024;

/// Env keys forwarded to the child. Everything else (notably other
/// `RALPH_*` runtime keys like RALPH_CURRENT_HAT) is scrubbed so a
/// monitor running inside a hat never leaks agent context into a
/// human-CLI observation (HARD RULE 5 equivalent on the TS side).
const ENV_ALLOWLIST = new Set([
  'PATH',
  'HOME',
  'TMPDIR',
  'TMP',
  'TEMP',
  'LANG',
  'LC_ALL',
  'LC_CTYPE',
  'TZ',
]);

export function buildChildEnv(
  source: NodeJS.ProcessEnv = process.env,
): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const key of Object.keys(source)) {
    if (ENV_ALLOWLIST.has(key) || key.startsWith('RALPH_MONITOR_')) {
      const value = source[key];
      if (value !== undefined) env[key] = value;
    }
  }
  return env;
}

export type InspectErrorKind =
  | 'spawn_error'
  | 'timeout'
  | 'nonzero'
  | 'bad_json'
  | 'payload_too_large';

export interface InspectError {
  kind: InspectErrorKind;
  message: string;
  stderr?: string;
}

export type InspectResult =
  | { ok: true; json: unknown; raw: string }
  | { ok: false; error: InspectError };

const STDERR_PREVIEW = 500;

function truncate(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, max)}…` : s;
}

export interface InspectOptions {
  timeoutMs?: number;
  /// Test seam: override the binary name (default `ralph`).
  ralphBin?: string;
  env?: NodeJS.ProcessEnv;
}

export function inspectRun(
  cwd: string,
  opts: InspectOptions = {},
): Promise<InspectResult> {
  const timeoutMs =
    opts.timeoutMs ?? readTimeoutFromEnv() ?? DEFAULT_INSPECT_TIMEOUT_MS;
  const bin = opts.ralphBin ?? 'ralph';
  const env = opts.env ?? buildChildEnv();

  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(bin, ['inspect', 'run', '--format', 'json'], {
        cwd,
        env,
        stdio: ['ignore', 'pipe', 'pipe'],
      });
    } catch (err) {
      resolve({
        ok: false,
        error: { kind: 'spawn_error', message: String(err) },
      });
      return;
    }

    const stdoutChunks: Buffer[] = [];
    const stderrChunks: Buffer[] = [];
    let stdoutBytes = 0;
    let settled = false;
    let timedOut = false;
    let oversized = false;

    const finish = (result: InspectResult) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        resolve(result);
      }
    };

    const timer = setTimeout(() => {
      timedOut = true;
      child.kill('SIGKILL');
    }, timeoutMs);

    child.stdout.on('data', (chunk: Buffer) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > MAX_SNAPSHOT_BYTES) {
        oversized = true;
        child.kill('SIGKILL');
        return;
      }
      stdoutChunks.push(chunk);
    });
    child.stderr.on('data', (chunk: Buffer) => stderrChunks.push(chunk));
    child.on('error', (err) => {
      finish({
        ok: false,
        error: {
          kind: 'spawn_error',
          message: `failed to spawn ${bin}: ${err.message}`,
        },
      });
    });
    child.on('close', (code) => {
      const stderr = truncate(
        Buffer.concat(stderrChunks).toString('utf8'),
        STDERR_PREVIEW,
      );
      if (timedOut) {
        finish({
          ok: false,
          error: {
            kind: 'timeout',
            message: `inspect run exceeded ${timeoutMs}ms`,
            stderr: stderr || undefined,
          },
        });
        return;
      }
      if (oversized) {
        finish({
          ok: false,
          error: {
            kind: 'payload_too_large',
            message: `inspect run output exceeded ${MAX_SNAPSHOT_BYTES} bytes`,
            stderr: stderr || undefined,
          },
        });
        return;
      }
      const raw = Buffer.concat(stdoutChunks).toString('utf8');
      if (code !== 0) {
        finish({
          ok: false,
          error: {
            kind: 'nonzero',
            message: `inspect run exited with code ${code}`,
            stderr: stderr || undefined,
          },
        });
        return;
      }
      try {
        finish({ ok: true, json: JSON.parse(raw), raw });
      } catch {
        finish({
          ok: false,
          error: {
            kind: 'bad_json',
            message: 'inspect run stdout is not valid JSON',
            stderr: stderr || truncate(raw, STDERR_PREVIEW),
          },
        });
      }
    });
  });
}

function readTimeoutFromEnv(): number | undefined {
  const raw = process.env.RALPH_MONITOR_INSPECT_TIMEOUT_MS;
  if (raw === undefined) return undefined;
  const n = Number(raw);
  return Number.isFinite(n) && n > 0 ? n : undefined;
}
