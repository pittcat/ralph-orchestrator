import assert from 'node:assert/strict';
import { test } from 'node:test';
import { openDatabase } from '../../src/db/index.js';
import { runMigrations } from '../../src/db/migrations.js';
import {
  applyIntents,
  upsertPendingInteraction,
} from '../../src/notify/outbox.js';
import type { NotifyIntent, ResolveIntent } from '../../src/notify/rules.js';
import { readEvents } from '../../src/stream/monitor_log.js';

function makeDb() {
  const db = openDatabase(':memory:');
  runMigrations(db);
  return db;
}

const INTENT: NotifyIntent = {
  type: 'notify',
  ruleId: 'run.terminal.completed',
  ruleVersion: 1,
  kind: 'run.completed',
  projectId: 'p1',
  runGid: 'm:p1:r1',
  pendingInteractionId: null,
  title: 'Run r1 completed',
  body: 'reason=completed',
  detail: { reason: 'completed' },
  occurrence: '2026-10-28T10:00:00Z',
};

test('notify intent → notification + in_app delivered + browser pending + SSE', () => {
  const db = makeDb();
  applyIntents(db, [INTENT], 't');
  const n = db.prepare('SELECT * FROM notifications').all() as any[];
  assert.equal(n.length, 1);
  assert.equal(n[0].kind, 'run.completed');
  assert.equal(n[0].state, 'unread');
  const d = db.prepare('SELECT * FROM deliveries ORDER BY channel').all() as any[];
  assert.deepEqual(
    d.map((x) => [x.channel, x.state]),
    [
      ['browser', 'pending'],
      ['in_app', 'delivered'],
    ],
  );
  const stream = readEvents(db, 0);
  assert.equal(stream.length, 1);
  assert.equal(stream[0].kind, 'notification.created');
  db.close();
});

test('replaying the same intent is a full no-op (S3)', () => {
  const db = makeDb();
  applyIntents(db, [INTENT], 't');
  applyIntents(db, [INTENT], 't2');
  assert.equal(
    (db.prepare('SELECT COUNT(*) n FROM notifications').get() as any).n,
    1,
  );
  assert.equal(
    (db.prepare('SELECT COUNT(*) n FROM deliveries').get() as any).n,
    2,
  );
  assert.equal(readEvents(db, 0).length, 1);
  db.close();
});

test('resolve intent closes the interaction and the linked notification', () => {
  const db = makeDb();
  upsertPendingInteraction(db, {
    id: 'sess:p1:s1',
    projectId: 'p1',
    sessionId: 's1',
    prompt: 'q?',
    occurrence: 't1',
    at: 't1',
  });
  applyIntents(
    db,
    [
      {
        ...INTENT,
        kind: 'interaction.pending',
        runGid: null,
        pendingInteractionId: 'sess:p1:s1',
        ruleId: 'session.waiting_for_input',
        occurrence: 't1',
      },
    ],
    't',
  );
  const res: ResolveIntent = {
    type: 'resolve_interaction',
    projectId: 'p1',
    sessionId: 's1',
    pendingInteractionId: 'sess:p1:s1',
    reason: 'status:completed',
  };
  applyIntents(db, [res], 't2');
  const pi = db
    .prepare(`SELECT * FROM pending_interactions WHERE id = 'sess:p1:s1'`)
    .get() as any;
  assert.equal(pi.status, 'resolved');
  const n = db.prepare('SELECT * FROM notifications').get() as any;
  assert.equal(n.state, 'resolved');
  assert.ok(n.resolved_at);
  const kinds = readEvents(db, 0).map((e) => e.kind);
  assert.deepEqual(kinds, ['notification.created', 'notification.updated']);
  // Resolving twice is a no-op.
  applyIntents(db, [res], 't3');
  assert.equal(readEvents(db, 0).length, 2);
  db.close();
});

test('same session re-pending a new occurrence yields a second notification', () => {
  const db = makeDb();
  upsertPendingInteraction(db, {
    id: 'sess:p1:s1',
    projectId: 'p1',
    sessionId: 's1',
    prompt: 'q?',
    occurrence: 't1',
    at: 't1',
  });
  const base = {
    ...INTENT,
    kind: 'interaction.pending',
    runGid: null,
    pendingInteractionId: 'sess:p1:s1',
    ruleId: 'session.waiting_for_input',
  };
  applyIntents(db, [{ ...base, occurrence: 't1' }], 't1');
  applyIntents(db, [{ ...base, occurrence: 't2' }], 't2');
  assert.equal(
    (db.prepare('SELECT COUNT(*) n FROM notifications').get() as any).n,
    2,
  );
  db.close();
});
