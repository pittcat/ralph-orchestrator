/**
 * Notification rules (Plan4-U1): pure delta → intent matching. The
 * seeded rule rows in `notification_rules` carry the matcher payloads;
 * these functions interpret them. Terminal evidence comes from the
 * runs projection (manifest authority — never event-topic guessing);
 * interaction evidence comes from `.ralph/planning-sessions/<id>/session.json`.
 */

import type { MonitorDb } from '../db/index.js';

export interface NotifyRule {
  id: string;
  version: number;
  kind: string;
  matcher: { on: string; reasons?: string[]; statuses?: string[] };
}

/** Session statuses that mean "awaiting human input" → pending. */
export const SESSION_PENDING_STATUSES = new Set(['waiting_for_input']);

/**
 * Every non-pending status resolves an open pending row (S4 消退):
 * active/completed/failed/timed_out/paused, and — conservatively —
 * a deleted session file (§15).
 */
export function sessionIsPending(status: string): boolean {
  return SESSION_PENDING_STATUSES.has(status);
}

/**
 * Terminal reason → notification kind. `completed` is the only
 * success word; every other terminal reason is a failure
 * (interrupted/cancelled/payload_violation/recovery_exhausted…).
 * A null/unknown reason never notifies.
 */
export function terminalNotificationKind(
  reason: string | null | undefined,
): 'run.completed' | 'run.failed' | null {
  if (!reason) return null;
  return reason === 'completed' ? 'run.completed' : 'run.failed';
}

export function loadRules(db: MonitorDb): NotifyRule[] {
  return (
    db
      .prepare(
        'SELECT id, version, kind, matcher FROM notification_rules WHERE enabled = 1',
      )
      .all() as { id: string; version: number; kind: string; matcher: string }[]
  ).map((r) => ({ ...r, matcher: JSON.parse(r.matcher) }));
}

export interface NotifyIntent {
  type: 'notify';
  ruleId: string;
  ruleVersion: number;
  kind: string;
  projectId: string;
  runGid: string | null;
  pendingInteractionId: string | null;
  title: string;
  body: string;
  detail: Record<string, unknown>;
  /** Idempotency discriminator — e.g. terminal.at or session.updatedAt. */
  occurrence: string;
}

export interface ResolveIntent {
  type: 'resolve_interaction';
  projectId: string;
  sessionId: string;
  pendingInteractionId: string;
  reason: string;
}

export type Intent = NotifyIntent | ResolveIntent;

/**
 * A run entering its terminal state matches `on=terminal` rules. The
 * exact `completed` rule wins over the `*` catch-all (both seeded so
 * matcher edits stay data-driven).
 */
export function matchTerminal(input: {
  rules: NotifyRule[];
  projectId: string;
  runGid: string;
  runId: string;
  reason: string;
  at: string;
  preset?: string | null;
}): NotifyIntent[] {
  const kind = terminalNotificationKind(input.reason);
  if (!kind) return [];
  return input.rules
    .filter(
      (r) =>
        r.matcher.on === 'terminal' &&
        r.kind === kind &&
        (r.matcher.reasons?.includes(input.reason) ||
          r.matcher.reasons?.includes('*')),
    )
    .map((r) => ({
      type: 'notify' as const,
      ruleId: r.id,
      ruleVersion: r.version,
      kind,
      projectId: input.projectId,
      runGid: input.runGid,
      pendingInteractionId: null,
      title:
        kind === 'run.completed'
          ? `Run ${input.runId} completed`
          : `Run ${input.runId} failed (${input.reason})`,
      body: `reason=${input.reason}`,
      detail: {
        reason: input.reason,
        at: input.at,
        run_id: input.runId,
        preset: input.preset ?? null,
      },
      occurrence: input.at,
    }));
}

/**
 * Session pending evidence → one notification per `occurrence`
 * (session.updatedAt round — a session may pend several times).
 */
export function matchSessionPending(input: {
  rules: NotifyRule[];
  projectId: string;
  sessionId: string;
  prompt: string;
  status: string;
  updatedAt: string;
}): NotifyIntent[] {
  if (!sessionIsPending(input.status)) return [];
  return input.rules
    .filter(
      (r) =>
        r.matcher.on === 'session_status' &&
        (r.matcher.statuses?.includes(input.status) ||
          r.matcher.statuses?.includes('*')),
    )
    .map((r) => ({
      type: 'notify' as const,
      ruleId: r.id,
      ruleVersion: r.version,
      kind: 'interaction.pending',
      projectId: input.projectId,
      runGid: null,
      pendingInteractionId: `sess:${input.projectId}:${input.sessionId}`,
      title: `Planning session ${input.sessionId} needs input`,
      body: input.prompt.slice(0, 200),
      detail: {
        session_id: input.sessionId,
        status: input.status,
        prompt: input.prompt.slice(0, 500),
      },
      occurrence: input.updatedAt,
    }));
}
