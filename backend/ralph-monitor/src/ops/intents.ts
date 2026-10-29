/**
 * Operation intents (Plan5-U2, D3–D6): request validation → durable
 * `operations` row. Two real-SQLite uniqueness authorities:
 * `idempotency_key UNIQUE` (replay returns the same row, 200) and
 * the partial `operations_one_open_per_run` index (a second
 * non-terminal op on one run → `operation_in_flight`, 409).
 */

import { randomUUID } from 'node:crypto';
import path from 'node:path';
import fs from 'node:fs';
import { z } from 'zod';
import type { MonitorDb } from '../db/index.js';
import { ensureMachineId } from '../lib/machine_id.js';

export const OP_KINDS = ['launch', 'stop', 'retry', 'merge', 'emit'] as const;
export type OpKind = (typeof OP_KINDS)[number];

export interface OperationRow {
  id: string;
  idempotency_key: string;
  kind: OpKind;
  project_id: string;
  run_gid: string | null;
  client_request_id: string | null;
  params: string;
  expected_revision: string | null;
  state: string;
  receipt: string | null;
  actor: string | null;
  origin: string | null;
  error: string | null;
  created_at: string;
  dispatched_at: string | null;
  confirmed_at: string | null;
}

export const operationBodySchema = z
  .object({
    kind: z.enum(OP_KINDS),
    project_id: z.string().min(1),
    run_gid: z.string().min(1).optional(),
    params: z.record(z.string(), z.unknown()).default({}),
    idempotency_key: z.string().min(8).max(128),
    expected_revision: z.string().min(1).optional(),
  })
  .strict();

export const launchParamsSchema = z
  .object({
    preset: z.string().min(1),
    prompt: z.string().max(64_000).optional(),
    prompt_file: z.string().max(4096).optional(),
    plan: z.string().max(4096).optional(),
    max_iterations: z.number().int().positive().max(10_000).optional(),
    backend: z.string().max(128).optional(),
  })
  .strict()
  .refine((p) => p.prompt !== undefined || p.prompt_file !== undefined || p.plan !== undefined, {
    message: 'one of prompt | prompt_file | plan is required',
  });

export const stopParamsSchema = z
  .object({ force: z.boolean().optional() })
  .strict();
export const retryParamsSchema = z
  .object({ steering: z.string().max(4096).optional() })
  .strict();
export const mergeParamsSchema = z
  .object({ force: z.boolean().optional() })
  .strict();
export const emitParamsSchema = z
  .object({
    topic: z.string().min(1).max(256),
    payload: z.string().max(64_000).optional(),
  })
  .strict();

export interface CapabilityDecl {
  id: string;
  available: boolean;
  reason?: string;
  revision: string;
  input_schema?: { topics?: string[] } | null;
}

export type IntentResult =
  | { ok: true; operation: OperationRow; replay: boolean }
  | {
      ok: false;
      status: number;
      code: string;
      message: string;
      details?: Record<string, unknown>;
    };

function fail(
  status: number,
  code: string,
  message: string,
  details?: Record<string, unknown>,
): IntentResult {
  return { ok: false, status, code, message, details };
}

/**
 * `run_gid` three-part form `<machineId>:<projectId>:<runId>` (Plan1 D8).
 * Callers decide how strict the machine segment must be — intents
 * reject foreign-machine gids, dispatch tolerates legacy `m:` rows.
 */
export function parseRunGid(
  gid: string,
): { machineId: string; projectId: string; runId: string } | null {
  const parts = gid.split(':');
  if (parts.length !== 3 || !parts[0] || !parts[1] || !parts[2]) {
    return null;
  }
  return { machineId: parts[0], projectId: parts[1], runId: parts[2] };
}

/**
 * Confine a user path to the project root (S9 launch params): the
 * resolved realpath must stay under `projectRoot` — same rule as the
 * artifacts allowlist, expressed here on a path that must already
 * exist.
 */
export function resolveWithin(
  projectRoot: string,
  userPath: string,
): { ok: true; canonical: string } | { ok: false; message: string } {
  const resolved = path.resolve(projectRoot, userPath);
  let canonical: string;
  try {
    canonical = fs.realpathSync(resolved);
  } catch {
    return { ok: false, message: `path does not exist: ${userPath}` };
  }
  const rootWithSep = projectRoot.endsWith(path.sep)
    ? projectRoot
    : projectRoot + path.sep;
  if (canonical !== projectRoot && !canonical.startsWith(rootWithSep)) {
    return { ok: false, message: `path escapes project root: ${userPath}` };
  }
  return { ok: true, canonical };
}

export function getOperation(db: MonitorDb, id: string): OperationRow | undefined {
  return db.prepare('SELECT * FROM operations WHERE id = ?').get(id) as
    | OperationRow
    | undefined;
}

export function findByIdemKey(
  db: MonitorDb,
  key: string,
): OperationRow | undefined {
  return db
    .prepare('SELECT * FROM operations WHERE idempotency_key = ?')
    .get(key) as OperationRow | undefined;
}

interface RunRow {
  project_id: string;
  run_id: string;
  actions: string | null;
}

function loadRun(
  db: MonitorDb,
  projectId: string,
  runId: string,
): RunRow | undefined {
  return db
    .prepare(
      'SELECT project_id, run_id, actions FROM runs WHERE project_id = ? AND run_id = ?',
    )
    .get(projectId, runId) as RunRow | undefined;
}

export function capabilityFor(
  actionsJson: string | null,
  actionId: string,
): CapabilityDecl | null {
  if (!actionsJson) return null;
  try {
    const arr = JSON.parse(actionsJson) as CapabilityDecl[];
    return arr.find((a) => a.id === actionId) ?? null;
  } catch {
    return null;
  }
}

/**
 * Validate + insert the intent. Returns the row on success — replay
 * (`replay:true`) means the idempotency key already maps to an op.
 */
export function createIntent(
  db: MonitorDb,
  body: unknown,
  actor: string | null,
  origin: string | null,
  now: () => Date = () => new Date(),
): IntentResult {
  const parsed = operationBodySchema.safeParse(body);
  if (!parsed.success) {
    return fail(400, 'invalid_body', 'request body is not a valid operation', {
      issues: parsed.error.issues.map((i) => ({
        path: i.path.join('.'),
        message: i.message,
      })),
    });
  }
  const b = parsed.data;

  const project = db
    .prepare('SELECT id, canonical_path FROM projects WHERE id = ?')
    .get(b.project_id) as { id: string; canonical_path: string } | undefined;
  if (!project) {
    return fail(404, 'project_not_found', 'unknown project_id');
  }

  // ── per-kind params ──
  let runId: string | null = null;
  if (b.kind === 'launch') {
    if (b.run_gid !== undefined) {
      return fail(400, 'invalid_body', 'launch does not take run_gid');
    }
    const p = launchParamsSchema.safeParse(b.params);
    if (!p.success) {
      return fail(422, 'invalid_params', 'invalid launch params', {
        issues: p.error.issues.map((i) => ({
          path: i.path.join('.'),
          message: i.message,
        })),
      });
    }
    for (const [field, value] of [
      ['prompt_file', p.data.prompt_file],
      ['plan', p.data.plan],
    ] as const) {
      if (value !== undefined) {
        const confined = resolveWithin(project.canonical_path, value);
        if (!confined.ok) {
          return fail(422, 'invalid_params', `${field}: ${confined.message}`);
        }
      }
    }
  } else {
    if (b.run_gid === undefined) {
      return fail(400, 'invalid_body', `${b.kind} requires run_gid`);
    }
    const gid = parseRunGid(b.run_gid);
    if (
      !gid ||
      gid.machineId !== ensureMachineId(db) ||
      gid.projectId !== b.project_id
    ) {
      return fail(422, 'invalid_run_gid', 'run_gid does not match project');
    }
    runId = gid.runId;
    const run = loadRun(db, b.project_id, runId);
    if (!run) {
      return fail(404, 'run_not_found', 'unknown run_gid');
    }

    const paramCheck = {
      stop: stopParamsSchema,
      retry: retryParamsSchema,
      merge: mergeParamsSchema,
      emit: emitParamsSchema,
    }[b.kind].safeParse(b.params);
    if (!paramCheck.success) {
      return fail(422, 'invalid_params', `invalid ${b.kind} params`, {
        issues: paramCheck.error.issues.map((i) => ({
          path: i.path.join('.'),
          message: i.message,
        })),
      });
    }

    // Capability gate: the runtime-declared actions[] drives what the
    // monitor may dispatch — never fake availability.
    const decl = capabilityFor(run.actions, b.kind);
    if (!decl) {
      return fail(
        422,
        'capability_unavailable',
        `runtime declares no '${b.kind}' action for this run`,
      );
    }
    if (!decl.available) {
      return fail(
        422,
        'capability_unavailable',
        decl.reason ?? `'${b.kind}' is not available for this run`,
        { reason: decl.reason },
      );
    }
    if (
      b.expected_revision !== undefined &&
      b.expected_revision !== decl.revision
    ) {
      return fail(409, 'revision_conflict', 'run evidence changed; refresh and retry', {
        current: decl.revision,
      });
    }
    if (b.kind === 'emit') {
      const topics = decl.input_schema?.topics ?? [];
      const topic = (b.params as { topic?: string }).topic!;
      if (!topics.includes(topic)) {
        return fail(
          422,
          'capability_unavailable',
          `topic '${topic}' is not in the runtime emit whitelist`,
          { allowed: topics },
        );
      }
    }
  }

  const id = randomUUID();
  const clientRequestId =
    b.kind === 'launch' ? `op-${id}` : null;

  // Idempotency is checked before the partial-unique index can fire —
  // a replayed key must return the stored row even while that row is
  // still non-terminal.
  const replay = findByIdemKey(db, b.idempotency_key);
  if (replay) return { ok: true, operation: replay, replay: true };

  try {
    db.prepare(
      `INSERT INTO operations
         (id, idempotency_key, kind, project_id, run_gid, client_request_id,
          params, expected_revision, state, actor, origin, created_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'recorded', ?, ?, ?)`,
    ).run(
      id,
      b.idempotency_key,
      b.kind,
      b.project_id,
      b.run_gid ?? null,
      clientRequestId,
      JSON.stringify(b.params),
      b.expected_revision ?? null,
      actor,
      origin,
      now().toISOString(),
    );
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    if (msg.includes('idempotency_key')) {
      const existing = findByIdemKey(db, b.idempotency_key)!;
      return { ok: true, operation: existing, replay: true };
    }
    if (msg.includes('operations.run_gid')) {
      return fail(
        409,
        'operation_in_flight',
        'another non-terminal operation already targets this run',
      );
    }
    throw e;
  }

  return { ok: true, operation: getOperation(db, id)!, replay: false };
}
