import assert from 'node:assert/strict';
import { describe, it, beforeEach, afterEach } from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import { openDatabase, type MonitorDb } from '../src/db/index.js';
import { runMigrations } from '../src/db/migrations.js';
import { ingestRunActivity } from '../src/activity/ingest.js';
import { tmpDbPath, makeProjectDir } from './helpers.js';

const PROJ = 'proj_a';
const RUN = 'run1';

function writeActivity(ws: string, lines: string): string {
  const dir = path.join(ws, '.ralph', 'runs');
  fs.mkdirSync(dir, { recursive: true });
  const f = path.join(dir, `${RUN}.activity.jsonl`);
  fs.writeFileSync(f, lines);
  return f;
}

const line = (o: Record<string, unknown>) => JSON.stringify(o) + '\n';

describe('activity ingest', () => {
  let db: MonitorDb;
  let ws: string;

  beforeEach(() => {
    db = openDatabase(tmpDbPath());
    runMigrations(db);
    ws = makeProjectDir();
  });
  afterEach(() => {
    db.close();
    fs.rmSync(ws, { recursive: true, force: true });
  });

  const rows = () =>
    db
      .prepare(
        'SELECT * FROM run_activity WHERE project_id=? AND run_id=? ORDER BY generation, seq',
      )
      .all(PROJ, RUN) as {
      seq: number;
      phase: string;
      at: string | null;
      detail: string | null;
    }[];

  it('ingests start/end pairs with seq_in_run and detail', () => {
    writeActivity(
      ws,
      line({ ts: '2026-10-28T12:00:01Z', seq_in_run: 1, hat: 'executor', backend: 'claude', phase: 'start' }) +
        line({
          ts: '2026-10-28T12:00:05Z',
          seq_in_run: 2,
          hat: 'executor',
          backend: 'claude',
          phase: 'end',
          exit_code: 0,
          timed_out: false,
          is_error: false,
          duration_ms: 4123,
        }),
    );
    const stats = ingestRunActivity(db, PROJ, RUN, ws, null);
    assert.equal(stats.inserted, 2);
    assert.equal(stats.parseErrors, 0);
    const r = rows();
    assert.equal(r.length, 2);
    assert.equal(r[0].seq, 1);
    assert.equal(r[0].phase, 'start');
    assert.equal(r[1].phase, 'end');
    assert.deepEqual(
      JSON.parse(r[1].detail!).exit_code,
      0,
    );
    assert.equal(r[0].at, '2026-10-28T12:00:01Z');
  });

  it('missing file → silent no-op (backend.unknown upstream)', () => {
    const stats = ingestRunActivity(db, PROJ, RUN, ws, null);
    assert.deepEqual(stats, { inserted: 0, parseErrors: 0, sourceResets: 0 });
  });

  it('incremental: appends only new bytes', () => {
    const f = writeActivity(
      ws,
      line({ ts: 't1', seq_in_run: 1, phase: 'start' }),
    );
    ingestRunActivity(db, PROJ, RUN, ws, null);
    fs.appendFileSync(
      f,
      line({ ts: 't2', seq_in_run: 2, phase: 'end', exit_code: 0 }),
    );
    const stats = ingestRunActivity(db, PROJ, RUN, ws, null);
    assert.equal(stats.inserted, 1);
    assert.equal(rows().length, 2);
  });

  it('partial trailing line waits for completion', () => {
    const f = writeActivity(
      ws,
      line({ ts: 't1', seq_in_run: 1, phase: 'start' }) +
        '{"ts":"t2","seq_in_run":2,"phase":"end"', // no newline, cut mid-JSON
    );
    let stats = ingestRunActivity(db, PROJ, RUN, ws, null);
    assert.equal(stats.inserted, 1);
    fs.appendFileSync(f, ',"exit_code":0}\n');
    stats = ingestRunActivity(db, PROJ, RUN, ws, null);
    assert.equal(stats.inserted, 1);
    assert.equal(rows().length, 2);
  });

  it('malformed lines count as parse errors, not aborts', () => {
    writeActivity(
      ws,
      'not json\n' +
        line({ ts: 't1', seq_in_run: 1, phase: 'start' }) +
        line({ seq_in_run: 'x', phase: 'start' }),
    );
    const stats = ingestRunActivity(db, PROJ, RUN, ws, null);
    assert.equal(stats.inserted, 1);
    assert.equal(stats.parseErrors, 2);
  });

  it('file shrink → generation reset re-reads from 0', () => {
    const f = writeActivity(
      ws,
      line({ ts: 't1', seq_in_run: 1, phase: 'start' }) +
        line({ ts: 't2', seq_in_run: 2, phase: 'end', exit_code: 0 }),
    );
    ingestRunActivity(db, PROJ, RUN, ws, null);
    fs.writeFileSync(f, line({ ts: 't3', seq_in_run: 1, phase: 'start' }));
    const stats = ingestRunActivity(db, PROJ, RUN, ws, null);
    assert.equal(stats.sourceResets, 1);
    assert.equal(stats.inserted, 1);
    // Old-generation rows stay; new generation carries its own seq=1.
    assert.equal(rows().length, 3);
  });

  it('activity_file from the snapshot wins over the derived path', () => {
    const custom = path.join(ws, 'custom-act.jsonl');
    fs.writeFileSync(
      custom,
      line({ ts: 't1', seq_in_run: 7, phase: 'start' }),
    );
    const stats = ingestRunActivity(db, PROJ, RUN, ws, custom);
    assert.equal(stats.inserted, 1);
  });
});
