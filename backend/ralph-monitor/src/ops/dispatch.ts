/**
 * Operation dispatch (Plan5-U2, D4/D6): argv mapping → spawn — always
 * a child_process arg array, never a shell string (S2/S8). `launch`
 * detaches (child exits promptly after writing the manifest; the run
 * itself is confirmed via the collector's client_request_id match).
 * stop/retry/merge/emit wait for exit and keep the typed JSON receipt
 * from the child's last stdout line (Plan5-U1 contract).
 */

import { spawn } from 'node:child_process';
import type { MonitorDb } from '../db/index.js';
import { appendEvent } from '../stream/monitor_log.js';
import { buildChildEnv } from '../ralph/client.js';
import {
  parseRunGid,
  type OperationRow,
} from './intents.js';

const STDERR_TAIL = 4000;
const DISPATCH_TIMEOUT_MS = 30_000;

export interface DispatchDeps {
  ralphBin?: string;
  env?: NodeJS.ProcessEnv;
  now?: () => Date;
}

function truncate(s: string, max: number): string {
  return s.length > max ? `…${s.slice(s.length - max)}` : s;
}

interface LaunchParams {
  preset: string;
  prompt?: string;
  prompt_file?: string;
  plan?: string;
  max_iterations?: number;
  backend?: string;
}

/** Params → argv. Paths arrive already confined to the project root. */
export function buildArgv(op: OperationRow, eventsFile?: string): string[] {
  const params = JSON.parse(op.params) as Record<string, unknown>;
  const runId = op.run_gid ? parseRunGid(op.run_gid)!.runId : null;
  switch (op.kind) {
    case 'launch': {
      const p = params as unknown as LaunchParams;
      const argv = ['run', '-a', '-H', p.preset];
      if (p.prompt !== undefined) argv.push('-p', p.prompt);
      if (p.prompt_file !== undefined) argv.push('--prompt-file', p.prompt_file);
      if (p.plan !== undefined) argv.push('--plan', p.plan);
      if (p.max_iterations !== undefined) {
        argv.push('--max-iterations', String(p.max_iterations));
      }
      if (p.backend !== undefined) argv.push('--backend', p.backend);
      argv.push('--client-request-id', op.client_request_id!);
      return argv;
    }
    case 'stop': {
      const argv = ['loops', 'stop', runId!];
      if ((params as { force?: boolean }).force) argv.push('--force');
      argv.push('--format', 'json');
      return argv;
    }
    case 'retry': {
      const argv = ['loops', 'retry', runId!];
      const steering = (params as { steering?: string }).steering;
      if (steering !== undefined) argv.push('--steering', steering);
      argv.push('--format', 'json');
      return argv;
    }
    case 'merge': {
      const argv = ['loops', 'merge', runId!];
      if ((params as { force?: boolean }).force) argv.push('--force');
      argv.push('--format', 'json');
      return argv;
    }
    case 'emit': {
      const p = params as { topic: string; payload?: string };
      const argv = ['emit', p.topic];
      if (p.payload !== undefined) argv.push(p.payload);
      argv.push('--file', eventsFile!);
      return argv;
    }
  }
}

/** Parse the child's last non-empty stdout line as a JSON receipt. */
export function parseReceipt(stdout: string): unknown | null {
  const lines = stdout.trim().split('\n').filter((l) => l.trim());
  for (let i = lines.length - 1; i >= 0; i -= 1) {
    try {
      return JSON.parse(lines[i]);
    } catch {
      continue;
    }
  }
  return null;
}

interface ChildResult {
  code: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
  spawnError?: string;
}

function runChild(
  bin: string,
  argv: string[],
  cwd: string,
  env: NodeJS.ProcessEnv,
  timeoutMs: number,
): Promise<ChildResult> {
  return new Promise((resolve) => {
    const child = spawn(bin, argv, {
      cwd,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill('SIGKILL');
    }, timeoutMs);
    child.stdout.on('data', (d: Buffer) => {
      stdout += d.toString();
      if (stdout.length > 2 * 1024 * 1024) {
        timedOut = true;
        child.kill('SIGKILL');
      }
    });
    child.stderr.on('data', (d: Buffer) => {
      stderr += d.toString();
      if (stderr.length > 1024 * 1024) {
        stderr = stderr.slice(stderr.length - 1024 * 1024);
      }
    });
    child.on('error', (e) => {
      clearTimeout(timer);
      resolve({
        code: null,
        stdout,
        stderr,
        timedOut,
        spawnError: e.message,
      });
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({ code, stdout, stderr, timedOut });
    });
  });
}

function updateOp(
  db: MonitorDb,
  op: OperationRow,
  patch: {
    state: string;
    receipt?: unknown;
    error?: string | null;
    dispatchedAt?: string;
    confirmedAt?: string;
  },
): OperationRow {
  db.prepare(
    `UPDATE operations SET
       state = ?,
       receipt = COALESCE(?, receipt),
       error = ?,
       dispatched_at = COALESCE(?, dispatched_at),
       confirmed_at = COALESCE(?, confirmed_at)
     WHERE id = ?`,
  ).run(
    patch.state,
    patch.receipt === undefined ? null : JSON.stringify(patch.receipt),
    patch.error ?? null,
    patch.dispatchedAt ?? null,
    patch.confirmedAt ?? null,
    op.id,
  );
  const updated = db
    .prepare('SELECT * FROM operations WHERE id = ?')
    .get(op.id) as OperationRow;
  appendEvent(
    db,
    'operation.updated',
    { projectId: op.project_id, runId: op.run_gid },
    { id: op.id, kind: op.kind, state: updated.state, run_gid: op.run_gid },
  );
  return updated;
}

/**
 * Dispatch one recorded operation. Launch is fire-and-return (the
 * spawned `ralph run` writes the manifest asynchronously); all other
 * kinds wait for the typed receipt and land terminal here.
 */
export async function dispatchOperation(
  db: MonitorDb,
  opId: string,
  deps: DispatchDeps = {},
): Promise<OperationRow> {
  const op = db
    .prepare('SELECT * FROM operations WHERE id = ?')
    .get(opId) as OperationRow;
  const project = db
    .prepare('SELECT canonical_path FROM projects WHERE id = ?')
    .get(op.project_id) as { canonical_path: string };
  const now = () => (deps.now?.() ?? new Date());
  const env = deps.env ?? buildChildEnv();
  const bin = deps.ralphBin ?? 'ralph';

  // emit writes to the run's own events file — runtime evidence
  // persisted on the runs row, confined to the project root.
  let eventsFile: string | undefined;
  if (op.kind === 'emit') {
    const gid = parseRunGid(op.run_gid!)!;
    const run = db
      .prepare(
        'SELECT events_file FROM runs WHERE project_id = ? AND run_id = ?',
      )
      .get(gid.projectId, gid.runId) as
      | { events_file: string | null }
      | undefined;
    eventsFile = run?.events_file ?? undefined;
    if (!eventsFile) {
      return db.transaction(() =>
        updateOp(db, op, {
          state: 'failed',
          error: 'dispatch_failed: run has no events_file evidence',
        }),
      )();
    }
  }
  const argv = buildArgv(op, eventsFile);

  const row = db.transaction(() => {
    const u = updateOp(db, op, {
      state: 'dispatching',
      dispatchedAt: now().toISOString(),
      error: null,
    });
    if (op.kind !== 'launch') return u;

    const child = spawn(bin, argv, {
      cwd: project.canonical_path,
      env,
      stdio: 'ignore',
      detached: true,
    });
    child.on('error', () => undefined); // spawn errors surface as confirm_timeout
    child.unref();
    return updateOp(db, u, { state: 'dispatched' });
  })();

  if (op.kind === 'launch') return row;

  const res = await runChild(
    bin,
    argv,
    project.canonical_path,
    env,
    DISPATCH_TIMEOUT_MS,
  );

  if (res.spawnError !== undefined) {
    return db.transaction(() =>
      updateOp(db, row, {
        state: 'failed',
        error: `spawn_error: ${res.spawnError}`,
      }),
    )();
  }
  if (res.timedOut) {
    return db.transaction(() =>
      updateOp(db, row, {
        state: 'failed',
        error: `dispatch timeout after ${DISPATCH_TIMEOUT_MS}ms`,
      }),
    )();
  }

  const receipt = parseReceipt(res.stdout);
  const receiptObj =
    receipt && typeof receipt === 'object'
      ? (receipt as Record<string, unknown>)
      : null;

  if (res.code !== 0) {
    const detail = truncate(res.stderr.trim() || res.stdout.trim(), STDERR_TAIL);
    return db.transaction(() =>
      updateOp(db, row, {
        state: 'failed',
        receipt: receiptObj ?? { ok: false },
        error: `exit ${res.code}: ${detail || 'no output'}`,
      }),
    )();
  }

  // Emit produces no receipt — a clean exit is the confirmation.
  const confirmed = op.kind === 'emit' ? true : receiptObj?.ok === true;
  return db.transaction(() =>
    updateOp(db, row, {
      state: confirmed ? 'confirmed' : 'failed',
      receipt:
        receiptObj ??
        (confirmed ? { ok: true, kind: 'emit' } : { ok: false }),
      error:
        confirmed
          ? null
          : `receipt reports failure${receiptObj?.error ? `: ${receiptObj.error}` : ''}`,
      confirmedAt: confirmed ? now().toISOString() : undefined,
    }),
  )();
}
