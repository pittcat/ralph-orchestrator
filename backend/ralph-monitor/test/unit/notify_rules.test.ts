import assert from 'node:assert/strict';
import { test } from 'node:test';
import { openDatabase } from '../../src/db/index.js';
import { runMigrations } from '../../src/db/migrations.js';
import {
  loadRules,
  matchSessionPending,
  matchTerminal,
  sessionIsPending,
  terminalNotificationKind,
  type NotifyRule,
} from '../../src/notify/rules.js';
import { idemKey } from '../../src/notify/outbox.js';

const RULES: NotifyRule[] = [
  {
    id: 'run.terminal.completed',
    version: 1,
    kind: 'run.completed',
    matcher: { on: 'terminal', reasons: ['completed'] },
  },
  {
    id: 'run.terminal.failed',
    version: 1,
    kind: 'run.failed',
    matcher: { on: 'terminal', reasons: ['*'] },
  },
  {
    id: 'session.waiting_for_input',
    version: 1,
    kind: 'interaction.pending',
    matcher: { on: 'session_status', statuses: ['waiting_for_input'] },
  },
];

test('migration seeds exactly the three v1 rules', () => {
  const db = openDatabase(':memory:');
  runMigrations(db);
  const rules = loadRules(db);
  assert.equal(rules.length, 3);
  assert.deepEqual(
    rules.map((r) => r.id).sort(),
    [
      'run.terminal.completed',
      'run.terminal.failed',
      'session.waiting_for_input',
    ],
  );
  db.close();
});

test('terminalNotificationKind maps the reason vocabulary', () => {
  assert.equal(terminalNotificationKind('completed'), 'run.completed');
  for (const r of [
    'interrupted',
    'cancelled',
    'payload_violation',
    'recovery_exhausted',
    'backend_exit',
  ]) {
    assert.equal(terminalNotificationKind(r), 'run.failed', r);
  }
  assert.equal(terminalNotificationKind(null), null);
  assert.equal(terminalNotificationKind(''), null);
  assert.equal(terminalNotificationKind('unknown'), 'run.failed');
});

test('matchTerminal picks the matching kind rule only', () => {
  const ok = matchTerminal({
    rules: RULES,
    projectId: 'p1',
    runGid: 'm:p1:r1',
    runId: 'r1',
    reason: 'completed',
    at: '2026-10-28T10:00:00Z',
  });
  assert.equal(ok.length, 1);
  assert.equal(ok[0].kind, 'run.completed');
  assert.equal(ok[0].occurrence, '2026-10-28T10:00:00Z');

  const fail = matchTerminal({
    rules: RULES,
    projectId: 'p1',
    runGid: 'm:p1:r1',
    runId: 'r1',
    reason: 'interrupted',
    at: '2026-10-28T10:00:00Z',
  });
  assert.equal(fail.length, 1);
  assert.equal(fail[0].kind, 'run.failed');
  assert.match(fail[0].title, /r1.*interrupted/);
});

test('sessionIsPending vocabulary', () => {
  assert.equal(sessionIsPending('waiting_for_input'), true);
  for (const s of ['active', 'completed', 'failed', 'timed_out', 'paused']) {
    assert.equal(sessionIsPending(s), false, s);
  }
});

test('matchSessionPending only fires on waiting status', () => {
  const base = {
    rules: RULES,
    projectId: 'p1',
    sessionId: 's1',
    prompt: 'continue?',
    updatedAt: '2026-10-28T10:00:00Z',
  };
  const pending = matchSessionPending({ ...base, status: 'waiting_for_input' });
  assert.equal(pending.length, 1);
  assert.equal(pending[0].kind, 'interaction.pending');
  assert.equal(pending[0].pendingInteractionId, 'sess:p1:s1');
  const active = matchSessionPending({ ...base, status: 'active' });
  assert.equal(active.length, 0);
});

test('idemKey is stable and discriminates occurrences', () => {
  const intent = matchTerminal({
    rules: RULES,
    projectId: 'p1',
    runGid: 'm:p1:r1',
    runId: 'r1',
    reason: 'completed',
    at: 't1',
  })[0];
  const a = idemKey(intent);
  assert.equal(a, idemKey({ ...intent }));
  assert.notEqual(a, idemKey({ ...intent, occurrence: 't2' }));
  assert.notEqual(a, idemKey({ ...intent, runGid: 'm:p1:r2' }));
});
