import { createHash } from 'node:crypto';
import type { MonitorDb } from './index.js';
import type { RunSnapshot } from '../ralph/snapshot.js';
import {
  isMonitorState,
  mapObservationState,
  type MonitorState,
} from '../lib/state.js';
import { ingestRunEvents } from '../events/ingest.js';
import { ingestRunActivity } from '../activity/ingest.js';
import { appendEvent } from '../stream/monitor_log.js';
import {
  loadRules,
  matchSessionPending,
  matchTerminal,
  sessionIsPending,
  type Intent,
} from '../notify/rules.js';
import { applyIntents, upsertPendingInteraction } from '../notify/outbox.js';
import type { SessionObservation } from '../notify/sessions.js';

/**
 * Runs projection layer (plan U3 §5/§15).
 *
 * - `ingestSnapshot` commits one whole snapshot per project inside a
 *   single transaction: run upserts + cursor advance are atomic —
 *   a partial snapshot is never committed.
 * - Upserts are idempotent and evidenced terminal states never
 *   regress to `running` (a `state_conflict` row is recorded
 *   instead). `exited_unknown` carries no terminal evidence and may
 *   recover to `running` on a later live observation.
 * - `stale` is a read-time signal (project cursor has failures or an
 *   old watermark), never persisted.
 */

const KEEP_ERRORS_PER_PROJECT = 50;

/// The literal `"(primary)"` registry display name is not a run
/// identity and must never enter the projection (E6).
const NON_IDENTITY_RUN_ID = '(primary)';

export interface RunRow {
  project_id: string;
  run_id: string;
  kind: string;
  state: string;
  state_evidence: string;
  cwd: string | null;
  repo_root: string | null;
  worktree_path: string | null;
  prompt_preview: string | null;
  prompt_sha256: string | null;
  preset: string | null;
  plan_path: string | null;
  started_at: string | null;
  terminal: string | null;
  last_event_at: string | null;
  last_observed_at: string;
  manifest_path: string | null;
  snapshot_sha256: string | null;
  preset_descriptor: string | null;
  scheduler: string | null;
  pid: number | null;
  workspace: string | null;
  activity_file: string | null;
  events_file?: string | null;
  actions: string | null;
  client_request_id: string | null;
}

export interface CursorRow {
  project_id: string;
  last_collected_at: string | null;
  last_snapshot_sha256: string | null;
  consecutive_failures: number;
}

export function snapshotSha256(raw: string): string {
  return createHash('sha256').update(raw).digest('hex');
}

/**
 * Apply one `run-observation/v1` snapshot for `projectId`. Caller
 * guarantees `snapshot` passed `parseSnapshot`. Everything commits in
 * one transaction — on throw nothing lands and the cursor stays put.
 */
export function ingestSnapshot(
  db: MonitorDb,
  projectId: string,
  snapshot: RunSnapshot,
  rawSha256: string,
  observedAt: string,
  extras: { sessions?: SessionObservation[]; machineId?: string } = {},
): void {
  db.transaction(() => {
    // Plan4-U1: notification intents accumulate through the round and
    // apply last — every write still lands in this one transaction.
    const intents: Intent[] = [];
    const rules = loadRules(db);
    const globalRunId = (runId: string) =>
      `${extras.machineId ?? 'm'}:${projectId}:${runId}`;

    const existing = db.prepare(
      `SELECT state, terminal, preset, plan_path, prompt_preview,
              started_at, last_event_at
       FROM runs WHERE project_id = ? AND run_id = ?`,
    );
    const upsert = db.prepare(
      `INSERT INTO runs (
         project_id, run_id, kind, state, state_evidence, cwd,
         repo_root, worktree_path, prompt_preview, prompt_sha256,
         preset, plan_path, started_at, terminal, last_event_at,
         last_observed_at, manifest_path, snapshot_sha256,
         preset_descriptor, scheduler, pid, workspace, activity_file,
         actions, client_request_id, events_file
       ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT(project_id, run_id) DO UPDATE SET
         kind = excluded.kind,
         state = excluded.state,
         state_evidence = excluded.state_evidence,
         cwd = excluded.cwd,
         repo_root = excluded.repo_root,
         worktree_path = excluded.worktree_path,
         prompt_preview = excluded.prompt_preview,
         prompt_sha256 = excluded.prompt_sha256,
         preset = excluded.preset,
         plan_path = excluded.plan_path,
         started_at = excluded.started_at,
         terminal = excluded.terminal,
         last_event_at = excluded.last_event_at,
         last_observed_at = excluded.last_observed_at,
         manifest_path = excluded.manifest_path,
         snapshot_sha256 = excluded.snapshot_sha256,
         preset_descriptor = excluded.preset_descriptor,
         scheduler = excluded.scheduler,
         pid = excluded.pid,
         workspace = excluded.workspace,
         activity_file = excluded.activity_file,
         actions = excluded.actions,
         client_request_id = excluded.client_request_id,
         events_file = excluded.events_file`,
    );
    const conflict = db.prepare(
      'INSERT INTO collection_errors (project_id, at, error) VALUES (?, ?, ?)',
    );
    // Dismissed runs (DELETE /runs/:gid) stay suppressed while their
    // on-disk evidence keeps reporting them — no row, no event or
    // activity ingest, no notifications for this pair. Exception: an
    // observation of `running` is only emitted when ralph itself
    // verified a live pid (`coarse_state`: pid_alive → "running"),
    // which stale files can never produce — a dismissed run that was
    // genuinely relaunched (same run_id, e.g. retry/abort+relaunch)
    // un-dismisses instead of staying invisible forever.
    const dismissed = new Set(
      (
        db
          .prepare(
            'SELECT run_id FROM dismissed_runs WHERE project_id = ?',
          )
          .all(projectId) as { run_id: string }[]
      ).map((r) => r.run_id),
    );
    const undismiss = db.prepare(
      'DELETE FROM dismissed_runs WHERE project_id = ? AND run_id = ?',
    );

    for (const obs of snapshot.runs) {
      if (obs.run_id === NON_IDENTITY_RUN_ID) continue;
      if (dismissed.has(obs.run_id)) {
        if (obs.state === 'running') {
          undismiss.run(projectId, obs.run_id);
          dismissed.delete(obs.run_id);
          appendEvent(
            db,
            'run.updated',
            { projectId, runId: obs.run_id },
            { changed: { dismissed: { from: true, to: false } } },
          );
        } else {
          continue;
        }
      }
      const mapped = mapObservationState({
        state: obs.state,
        pid: obs.pid ?? null,
        terminal: obs.terminal
          ? {
              reason: obs.terminal.reason,
              at: obs.terminal.at,
              mergeCommit: obs.terminal.merge_commit ?? null,
            }
          : null,
        evidence: {
          manifestPresent: obs.evidence.manifest_present,
          manifestCorrupt: obs.evidence.manifest_corrupt,
          registryPresent: obs.evidence.registry_present,
          lockPresent: obs.evidence.lock_present,
          mergeState: obs.evidence.merge_state ?? null,
        },
      });

      const prior = existing.get(projectId, obs.run_id) as
        | {
            state: string;
            terminal: string | null;
            preset: string | null;
            plan_path: string | null;
            prompt_preview: string | null;
            started_at: string | null;
            last_event_at: string | null;
          }
        | undefined;
      // Terminal regression guard (S5): only *evidenced* terminal
      // states refuse a later `running` — a real terminal row carries a
      // `terminal` record or merged-exit evidence. `exited_unknown` is
      // the fail-closed "we don't know" bucket (dead pid, missing
      // manifest, mid-rotation observation): it must recover when a
      // live observation arrives, e.g. an aborted run relaunched via
      // `--reuse-worktree`, or the row freezes forever showing a dead
      // pid while the loop is actually alive.
      const priorHasTerminalEvidence =
        prior !== undefined &&
        (prior.terminal !== null || prior.state === 'exited_merged');
      if (
        prior &&
        prior.state.startsWith('exited_') &&
        mapped.state === 'running' &&
        priorHasTerminalEvidence
      ) {
        conflict.run(
          projectId,
          observedAt,
          `state_conflict: ${obs.run_id} ${prior.state}→${mapped.state} rejected (terminal regression)`,
        );
        continue;
      }

      upsert.run(
        projectId,
        obs.run_id,
        obs.kind,
        mapped.state,
        JSON.stringify(mapped.evidence),
        obs.cwd ?? null,
        obs.repo_root ?? null,
        obs.worktree_path ?? null,
        obs.prompt_preview ?? null,
        obs.prompt_sha256 ?? null,
        obs.preset ?? null,
        obs.plan_path ?? null,
        obs.started_at ?? null,
        obs.terminal ? JSON.stringify(obs.terminal) : null,
        // Plan U3 §8: events.jsonl is not read in this unit; the
        // snapshot ships no last_event_at yet → stays NULL until
        // Plan 2 fills it.
        null,
        observedAt,
        obs.manifest ?? null,
        rawSha256,
        obs.preset_descriptor ? JSON.stringify(obs.preset_descriptor) : null,
        obs.scheduler ? JSON.stringify(obs.scheduler) : null,
        obs.pid ?? null,
        obs.workspace ?? obs.cwd ?? null,
        obs.activity_file ?? null,
        obs.actions ? JSON.stringify(obs.actions) : null,
        obs.client_request_id ?? null,
        obs.events_file ?? null,
      );

      // Plan4-U1: a run crossing into terminal state is notification
      // evidence — matched once per (rule, occurrence=terminal.at).
      if (obs.terminal && !(prior && prior.terminal)) {
        intents.push(
          ...matchTerminal({
            rules,
            projectId,
            runGid: globalRunId(obs.run_id),
            runId: obs.run_id,
            reason: obs.terminal.reason,
            at: obs.terminal.at,
            preset: obs.preset,
          }),
        );
      }

      // Plan2-U2: events ingest in the same transaction — a run row
      // and its event ledger advance atomically (plan E12). Workspace
      // resolution prefers `workspace` over `cwd` (manifest runs ship
      // both; worktree runs point workspace at the worktree).
      // Plan3-U3: projection diffs become monitor_events rows inside
      // the same transaction (A3 — log and projection never diverge).
      if (!prior) {
        appendEvent(
          db,
          'run.created',
          { projectId, runId: obs.run_id },
          { state: mapped.state, kind: obs.kind },
        );
      } else {
        const changed: Record<string, { from: unknown; to: unknown }> = {};
        const terminalJson = obs.terminal
          ? JSON.stringify(obs.terminal)
          : null;
        for (const [key, from, to] of [
          ['state', prior.state, mapped.state],
          ['terminal', prior.terminal, terminalJson],
          ['preset', prior.preset, obs.preset ?? null],
          ['plan_path', prior.plan_path, obs.plan_path ?? null],
        ] as const) {
          if (from !== to) changed[key] = { from, to };
        }
        if (Object.keys(changed).length > 0) {
          appendEvent(
            db,
            'run.updated',
            { projectId, runId: obs.run_id },
            { changed },
          );
        }
      }

      const workspace = obs.workspace ?? obs.cwd ?? null;
      if (workspace) {
        const before = (
          db
            .prepare(
              'SELECT MAX(seq) AS m FROM run_events WHERE project_id = ? AND run_id = ?',
            )
            .get(projectId, obs.run_id) as { m: number | null }
        ).m ?? 0;
        const evStats = ingestRunEvents(
          db,
          projectId,
          obs.run_id,
          workspace,
          obs.events_file,
          {
            startedAt: obs.started_at ?? null,
            terminalAt: obs.terminal?.at ?? null,
          },
        );
        if (evStats.inserted > 0) {
          appendEvent(
            db,
            'run.events',
            { projectId, runId: obs.run_id },
            {
              from_seq: before + 1,
              to_seq: before + evStats.inserted,
              count: evStats.inserted,
            },
          );
        }
        // Plan3-U2: the activity ledger joins the same transaction —
        // run row + events + activity land atomically (E7). New
        // backend evidence is a health-signal change (Plan3-U3).
        const actStats = ingestRunActivity(
          db,
          projectId,
          obs.run_id,
          workspace,
          obs.activity_file,
        );
        if (actStats.inserted > 0) {
          appendEvent(
            db,
            'run.health',
            { projectId, runId: obs.run_id },
            { activity_rows: actStats.inserted },
          );
        }
      }
      // Plan2-U2 fills the U3 placeholder: evidence time comes from
      // the projected ledger.
      db.prepare(
        `UPDATE runs SET last_event_at =
           (SELECT MAX(ts) FROM run_events
            WHERE project_id = ? AND run_id = ?)
         WHERE project_id = ? AND run_id = ?`,
      ).run(projectId, obs.run_id, projectId, obs.run_id);
    }

    // Plan4-U1: planning-session diff — new waiting evidence becomes
    // a pending interaction + notification; leaving the pending
    // status (or the file disappearing) resolves the open row.
    if (extras.sessions) {
      const openRows = db
        .prepare(
          `SELECT id, session_id, occurrence FROM pending_interactions
           WHERE project_id = ? AND status = 'pending'`,
        )
        .all(projectId) as { id: string; session_id: string; occurrence: string | null }[];
      const openBySession = new Map(openRows.map((r) => [r.session_id, r]));

      for (const s of extras.sessions) {
        const piId = `sess:${projectId}:${s.sessionId}`;
        const open = openBySession.get(s.sessionId);
        if (sessionIsPending(s.status)) {
          const fresh = !open || open.occurrence !== s.updatedAt;
          if (fresh) {
            upsertPendingInteraction(db, {
              id: piId,
              projectId,
              sessionId: s.sessionId,
              prompt: s.prompt,
              occurrence: s.updatedAt,
              at: observedAt,
            });
            intents.push(
              ...matchSessionPending({
                rules,
                projectId,
                sessionId: s.sessionId,
                prompt: s.prompt,
                status: s.status,
                updatedAt: s.updatedAt,
              }),
            );
          }
          openBySession.delete(s.sessionId);
        } else if (open) {
          intents.push({
            type: 'resolve_interaction',
            projectId,
            sessionId: s.sessionId,
            pendingInteractionId: open.id,
            reason: `status:${s.status}`,
          });
          openBySession.delete(s.sessionId);
        }
      }
      // Sessions whose files vanished while still pending resolve
      // conservatively (§15) — a deleted session is answered-or-gone.
      for (const leftover of openBySession.values()) {
        intents.push({
          type: 'resolve_interaction',
          projectId,
          sessionId: leftover.session_id,
          pendingInteractionId: leftover.id,
          reason: 'session_file_removed',
        });
      }
    }

    applyIntents(db, intents, observedAt);

    // A previously failing collector recovering this round is itself
    // a project-health transition worth streaming (Plan3-U3).
    const priorFailures = (
      db
        .prepare(
          'SELECT consecutive_failures FROM source_cursors WHERE project_id = ?',
        )
        .get(projectId) as { consecutive_failures: number } | undefined
    )?.consecutive_failures;

    db.prepare(
      `INSERT INTO source_cursors
         (project_id, last_collected_at, last_snapshot_sha256, consecutive_failures)
       VALUES (?, ?, ?, 0)
       ON CONFLICT(project_id) DO UPDATE SET
         last_collected_at = excluded.last_collected_at,
         last_snapshot_sha256 = excluded.last_snapshot_sha256,
         consecutive_failures = 0`,
    ).run(projectId, observedAt, rawSha256);
    if ((priorFailures ?? 0) > 0) {
      appendEvent(
        db,
        'project.health',
        { projectId },
        { consecutive_failures: 0, recovered: true },
      );
    }
  })();
}

/**
 * Record a collection failure for one project: bump
 * `consecutive_failures` (leaving the last-good watermark alone),
 * append a bounded error row.
 */
export function recordCollectionError(
  db: MonitorDb,
  projectId: string,
  error: string,
  at: string,
): void {
  db.transaction(() => {
    db.prepare(
      `INSERT INTO source_cursors (project_id, consecutive_failures)
       VALUES (?, 1)
       ON CONFLICT(project_id) DO UPDATE SET
         consecutive_failures = consecutive_failures + 1`,
    ).run(projectId);
    db.prepare(
      'INSERT INTO collection_errors (project_id, at, error) VALUES (?, ?, ?)',
    ).run(projectId, at, error);
    db.prepare(
      `DELETE FROM collection_errors
       WHERE project_id = ? AND id NOT IN (
         SELECT id FROM collection_errors
         WHERE project_id = ? ORDER BY id DESC LIMIT ?
       )`,
    ).run(projectId, projectId, KEEP_ERRORS_PER_PROJECT);
    appendEvent(
      db,
      'project.health',
      { projectId },
      { error },
    );
  })();
}

export function getCursor(
  db: MonitorDb,
  projectId: string,
): CursorRow | undefined {
  return db
    .prepare('SELECT * FROM source_cursors WHERE project_id = ?')
    .get(projectId) as CursorRow | undefined;
}

export function latestCollectionError(
  db: MonitorDb,
  projectId: string,
): string | null {
  const row = db
    .prepare(
      `SELECT error FROM collection_errors
       WHERE project_id = ? ORDER BY id DESC LIMIT 1`,
    )
    .get(projectId) as { error: string } | undefined;
  return row?.error ?? null;
}

/** Default stale window: a watermark older than this reads stale. */
export const DEFAULT_STALE_MS = 30_000;

export function isProjectStale(
  cursor: CursorRow | undefined,
  now: Date,
  staleMs: number = DEFAULT_STALE_MS,
): boolean {
  if (!cursor) return false;
  if (cursor.consecutive_failures > 0) return true;
  if (!cursor.last_collected_at) return false;
  const age = now.getTime() - Date.parse(cursor.last_collected_at);
  return Number.isFinite(age) && age > staleMs;
}

export interface RunQuery {
  projectId?: string;
  states?: MonitorState[];
  q?: string;
  sort?: 'startedAt' | '-startedAt';
  limit: number;
  offset: number;
}

export interface RunQueryResult {
  rows: (RunRow & { stale: boolean })[];
  total: number;
  watermark: string | null;
}

/**
 * Filtered/paginated projection read. `watermark` = freshest
 * `last_collected_at` among the projects in scope (Plan 3 reuses it
 * as the stream resume hint).
 */
export function queryRuns(
  db: MonitorDb,
  query: RunQuery,
  now: Date = new Date(),
): RunQueryResult {
  const where: string[] = [];
  const params: unknown[] = [];

  if (query.projectId) {
    where.push('r.project_id = ?');
    params.push(query.projectId);
  }
  if (query.states && query.states.length > 0) {
    where.push(
      `r.state IN (${query.states.map(() => '?').join(',')})`,
    );
    params.push(...query.states);
  }
  if (query.q) {
    where.push(
      '(r.run_id LIKE ? OR r.prompt_preview LIKE ? OR r.preset LIKE ?)',
    );
    const like = `%${query.q}%`;
    params.push(like, like, like);
  }

  const whereSql = where.length > 0 ? `WHERE ${where.join(' AND ')}` : '';
  const dir = query.sort === 'startedAt' ? 'ASC' : 'DESC';
  const total = (
    db
      .prepare(`SELECT COUNT(*) AS n FROM runs r ${whereSql}`)
      .get(...params) as { n: number }
  ).n;
  const rows = db
    .prepare(
      `SELECT r.*, c.consecutive_failures, c.last_collected_at
       FROM runs r
       LEFT JOIN source_cursors c ON c.project_id = r.project_id
       ${whereSql}
       ORDER BY (r.started_at IS NULL) ASC, r.started_at ${dir}, r.run_id ASC
       LIMIT ? OFFSET ?`,
    )
    .all(...params, query.limit, query.offset) as (RunRow & {
    consecutive_failures: number | null;
    last_collected_at: string | null;
  })[];

  const watermarkRow = db
    .prepare(
      `SELECT MAX(c.last_collected_at) AS watermark
       FROM source_cursors c
       ${query.projectId ? 'WHERE c.project_id = ?' : ''}`,
    )
    .get(...(query.projectId ? [query.projectId] : [])) as {
    watermark: string | null;
  };

  return {
    rows: rows.map((row) => ({
      ...row,
      stale: isProjectStale(
        {
          project_id: row.project_id,
          last_collected_at: row.last_collected_at,
          last_snapshot_sha256: null,
          consecutive_failures: row.consecutive_failures ?? 0,
        },
        now,
      ),
    })),
    total,
    watermark: watermarkRow.watermark,
  };
}

export function getRun(
  db: MonitorDb,
  projectId: string,
  runId: string,
): (RunRow & { stale: boolean }) | undefined {
  const row = db
    .prepare('SELECT * FROM runs WHERE project_id = ? AND run_id = ?')
    .get(projectId, runId) as RunRow | undefined;
  if (!row) return undefined;
  return {
    ...row,
    stale: isProjectStale(getCursor(db, projectId), new Date()),
  };
}

export function parseStates(csv: string): MonitorState[] | null {
  const parts = csv
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);
  if (parts.length === 0) return null;
  if (!parts.every(isMonitorState)) return null;
  return parts;
}
