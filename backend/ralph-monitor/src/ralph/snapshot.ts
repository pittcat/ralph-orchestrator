import { z } from 'zod';

/// The only schema tag this collector understands (plan §3 D3/E8).
export const OBSERVATION_SCHEMA_VERSION = 'run-observation/v1';

const terminalSchema = z.object({
  reason: z.string(),
  at: z.string(),
  merge_commit: z.string().optional(),
});

const evidenceSchema = z.object({
  manifest_present: z.boolean().default(false),
  manifest_corrupt: z.boolean().default(false),
  registry_present: z.boolean().default(false),
  lock_present: z.boolean().default(false),
  merge_state: z.string().optional(),
});

const runObservationSchema = z.object({
  run_id: z.string().min(1),
  kind: z.string(),
  state: z.string(),
  pid: z.number().int().optional(),
  cwd: z.string().optional(),
  workspace: z.string().optional(),
  repo_root: z.string().optional(),
  worktree_path: z.string().optional(),
  prompt_preview: z.string().optional(),
  prompt_sha256: z.string().optional(),
  preset: z.string().optional(),
  plan_path: z.string().optional(),
  started_at: z.string().optional(),
  events_file: z.string().optional(),
  terminal: terminalSchema.nullish(),
  manifest: z.string().optional(),
  evidence: evidenceSchema.default({
    manifest_present: false,
    manifest_corrupt: false,
    registry_present: false,
    lock_present: false,
  }),
});

const snapshotSchema = z.object({
  schema_version: z.literal(OBSERVATION_SCHEMA_VERSION),
  workspace: z.string(),
  generated_at: z.string(),
  runs: z.array(runObservationSchema),
});

export type RunObservation = z.infer<typeof runObservationSchema>;
export type RunSnapshot = z.infer<typeof snapshotSchema>;

export type ParseSnapshotError =
  | { ok: false; code: 'unsupported_version'; message: string }
  | { ok: false; code: 'invalid_snapshot'; message: string };

export type ParseSnapshotResult =
  | { ok: true; snapshot: RunSnapshot }
  | ParseSnapshotError;

/**
 * Parse a raw `inspect run --format json` payload. Version drift is a
 * first-class error (`unsupported_version`) rather than a schema
 * failure so the collector can log it distinctly.
 */
export function parseSnapshot(raw: unknown): ParseSnapshotResult {
  if (
    typeof raw !== 'object' ||
    raw === null ||
    (raw as { schema_version?: unknown }).schema_version !==
      OBSERVATION_SCHEMA_VERSION
  ) {
    const seen =
      typeof raw === 'object' && raw !== null
        ? String((raw as { schema_version?: unknown }).schema_version)
        : typeof raw;
    return {
      ok: false,
      code: 'unsupported_version',
      message: `expected schema_version ${OBSERVATION_SCHEMA_VERSION}, got ${seen}`,
    };
  }
  const parsed = snapshotSchema.safeParse(raw);
  if (!parsed.success) {
    const issue = parsed.error.issues[0];
    const path = issue?.path.join('.') || '(root)';
    return {
      ok: false,
      code: 'invalid_snapshot',
      message: `${path}: ${issue?.message ?? 'invalid payload'}`,
    };
  }
  return { ok: true, snapshot: parsed.data };
}
