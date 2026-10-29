import assert from 'node:assert/strict';
import { describe, it, beforeEach, afterEach } from 'node:test';
import { openDatabase, type MonitorDb } from '../../src/db/index.js';
import { runMigrations } from '../../src/db/migrations.js';
import {
  appendEvent,
  readEvents,
  streamBounds,
  toSseFrame,
  KEEP_EVENTS,
} from '../../src/stream/monitor_log.js';

describe('monitor_log', () => {
  let db: MonitorDb;
  beforeEach(() => {
    db = openDatabase(':memory:');
    runMigrations(db);
  });
  afterEach(() => db.close());

  it('appends with globally increasing seq and parses back', () => {
    appendEvent(db, 'run.created', { projectId: 'p', runId: 'r' }, { s: 1 });
    appendEvent(db, 'run.events', { projectId: 'p', runId: 'r' }, {
      from_seq: 1,
      to_seq: 3,
      count: 3,
    });
    const rows = readEvents(db, 0);
    assert.equal(rows.length, 2);
    assert.equal(rows[0].seq, 1);
    assert.equal(rows[1].seq, 2);
    assert.equal(rows[1].kind, 'run.events');
    assert.deepEqual(JSON.parse(rows[1].payload!), {
      from_seq: 1,
      to_seq: 3,
      count: 3,
    });
  });

  it('readEvents honours the cursor and limit', () => {
    for (let i = 0; i < 5; i++) {
      appendEvent(db, 'run.updated', { projectId: 'p', runId: 'r' }, { i });
    }
    assert.equal(readEvents(db, 3).length, 2);
    assert.equal(readEvents(db, 0, 2).length, 2);
    assert.equal(readEvents(db, 0, 2)[0].seq, 1);
  });

  it('shares the caller transaction — rollback keeps nothing', () => {
    assert.throws(() =>
      db.transaction(() => {
        appendEvent(db, 'run.created', { projectId: 'p', runId: 'r' }, {});
        throw new Error('boom');
      })(),
    );
    assert.equal(readEvents(db, 0).length, 0);
    assert.equal(streamBounds(db), null);
  });

  it('bounds report min/max; empty table → null', () => {
    assert.equal(streamBounds(db), null);
    appendEvent(db, 'project.health', { projectId: 'p' }, {});
    appendEvent(db, 'project.health', { projectId: 'p' }, {});
    assert.deepEqual(streamBounds(db), { min: 1, max: 2 });
  });

  it('prunes beyond the retained window, boundary kept', () => {
    // KEEP is 50k — cheaper to verify the SQL directly at a small
    // scale: seed 10 rows then run the prune statement with N=3.
    for (let i = 0; i < 10; i++) {
      appendEvent(db, 'run.updated', { projectId: 'p', runId: 'r' }, { i });
    }
    db.prepare(
      `DELETE FROM monitor_events
       WHERE seq < (SELECT MAX(seq) FROM monitor_events) - ?`,
    ).run(3);
    const b = streamBounds(db)!;
    // seqs 7..10 retained (max - 3), so min = 7.
    assert.equal(b.min, 7);
    assert.equal(b.max, 10);
    assert.equal(KEEP_EVENTS, 50_000);
  });

  it('toSseFrame emits id/event/data lines', () => {
    appendEvent(db, 'run.created', { projectId: 'p', runId: 'r' }, { a: 1 });
    const frame = toSseFrame(readEvents(db, 0)[0]);
    assert.equal(frame, 'id: 1\nevent: run.created\ndata: {"a":1}\n\n');
  });
});
