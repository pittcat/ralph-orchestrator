import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { tmpDbPath, makeProjectDir } from './helpers.js';
import { openDatabase, type MonitorDb } from '../src/db/index.js';
import { runMigrations } from '../src/db/migrations.js';
import {
  ingestRunEvents,
  enumerateEventFiles,
} from '../src/events/ingest.js';

const RUN = 'run_test_1';
const PROJ = 'proj_1';

function line(obj: unknown): string {
  return `${JSON.stringify(obj)}\n`;
}

function writeEvents(dir: string, name: string, lines: string): string {
  const ralph = path.join(dir, '.ralph');
  fs.mkdirSync(ralph, { recursive: true });
  const file = path.join(ralph, name);
  fs.writeFileSync(file, lines);
  return file;
}

describe('events ingest', () => {
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

  const ingest = (
    eventsFile?: string | null,
    window: { startedAt: string | null; terminalAt: string | null } = {
      startedAt: null,
      terminalAt: null,
    },
  ) =>
    ingestRunEvents(db, PROJ, RUN, ws, eventsFile, window);

  const rows = () =>
    db
      .prepare('SELECT * FROM run_events WHERE project_id=? AND run_id=? ORDER BY seq')
      .all(PROJ, RUN) as any[];

  it('ingests ts/topic/source/payload lines with monotonic seq', () => {
    writeEvents(ws, 'events-20261028T1200.jsonl',
      line({ ts: '2026-10-28T12:00:01Z', topic: 'forge.start', source: 'loop-bootstrap', payload: '{"n":1}' }) +
      line({ ts: '2026-10-28T12:00:02Z', topic: 'forge.unit.ready', source: 'forge', iteration: 1, payload: '{"unit":"U1"}' }));
    const stats = ingest();
    assert.equal(stats.inserted, 2);
    assert.equal(stats.parseErrors, 0);
    const r = rows();
    assert.equal(r[0].seq, 1);
    assert.equal(r[1].seq, 2);
    assert.equal(r[0].topic, 'forge.start');
    assert.equal(r[0].hat, 'loop-bootstrap');
    assert.equal(r[1].iteration, 1);
  });

  it('missing hat/source → hat NULL', () => {
    writeEvents(ws, 'events.jsonl', line({ ts: '2026-10-28T12:00:01Z', topic: 'x.y' }));
    ingest();
    assert.equal(rows()[0].hat, null);
  });

  it('malformed lines count as parse_errors without aborting', () => {
    writeEvents(ws, 'events.jsonl',
      'not-json{\n' +
      line({ ts: '2026-10-28T12:00:01Z', topic: 'a.b' }) +
      '[1,2]\n' +
      line({ ts: '2026-10-28T12:00:02Z', topic: 'c.d' }));
    const stats = ingest();
    assert.equal(stats.inserted, 2);
    assert.equal(stats.parseErrors, 2);
    const ev = db.prepare('SELECT parse_errors FROM run_evidence WHERE run_id=?').get(RUN) as any;
    assert.equal(ev.parse_errors, 2);
  });

  it('re-ingest is idempotent via (file,offset) dedupe', () => {
    writeEvents(ws, 'events.jsonl', line({ topic: 'a.b', ts: '2026-10-28T12:00:01Z' }));
    ingest();
    const stats = ingest();
    assert.equal(stats.inserted, 0);
    assert.equal(rows().length, 1);
  });

  it('append continues from byte cursor; seq stays monotonic', () => {
    const f = writeEvents(ws, 'events.jsonl', line({ topic: 'a', ts: '2026-10-28T12:00:01Z' }));
    ingest();
    fs.appendFileSync(f, line({ topic: 'b', ts: '2026-10-28T12:00:02Z' }));
    const stats = ingest();
    assert.equal(stats.inserted, 1);
    assert.deepEqual(rows().map((r) => r.seq), [1, 2]);
  });

  it('partial last line (no newline) is left for the next pass', () => {
    const f = writeEvents(ws, 'events.jsonl',
      line({ topic: 'a', ts: '2026-10-28T12:00:01Z' }) + '{"topic":"incomplete"');
    const s1 = ingest();
    assert.equal(s1.inserted, 1);
    fs.appendFileSync(f, ',"ts":"2026-10-28T12:00:03Z"}\n');
    const s2 = ingest();
    assert.equal(s2.inserted, 1);
    assert.equal(rows()[1].topic, 'incomplete');
  });

  it('file shrink → source reset re-reads from 0', () => {
    const f = writeEvents(ws, 'events.jsonl', line({ topic: 'a', ts: '2026-10-28T12:00:01Z' }) + line({ topic: 'b', ts: '2026-10-28T12:00:02Z' }));
    ingest();
    fs.writeFileSync(f, line({ topic: 'c', ts: '2026-10-28T12:00:03Z' }));
    const stats = ingest();
    assert.equal(stats.sourceResets, 1);
    // old rows stay (seq never reused); new short file appends
    assert.equal(rows().length, 3);
  });

  it('payload >50000 chars → truncated flag', () => {
    writeEvents(ws, 'events.jsonl',
      line({ topic: 'big', ts: '2026-10-28T12:00:01Z', payload: 'x'.repeat(60_000) }));
    ingest();
    assert.equal(rows()[0].truncated, 1);
  });

  it('window filter skips events before started_at', () => {
    writeEvents(ws, 'events.jsonl',
      line({ ts: '2026-10-28T11:00:00Z', topic: 'old' }) +
      line({ ts: '2026-10-28T12:00:01Z', topic: 'new' }));
    ingest(null, { startedAt: '2026-10-28T12:00:00Z', terminalAt: null });
    const r = rows();
    assert.equal(r.length, 1);
    assert.equal(r[0].topic, 'new');
  });

  it('events_file + events-*.jsonl both enumerated; seq continuous across files', () => {
    const ef = path.join(ws, '.ralph', 'events-run1.jsonl');
    writeEvents(ws, 'events-run1.jsonl', line({ ts: '2026-10-28T12:00:01Z', topic: 'one' }));
    writeEvents(ws, 'events-20261029T0100.jsonl', line({ ts: '2026-10-28T12:00:02Z', topic: 'two' }));
    ingest(ef);
    const r = rows();
    assert.equal(r.length, 2);
    assert.deepEqual(r.map((x) => x.seq), [1, 2]);
  });

  it('enumerateEventFiles sorts timestamped files by name', () => {
    writeEvents(ws, 'events-20261029T0100.jsonl', '');
    writeEvents(ws, 'events-20261028T0100.jsonl', '');
    const files = enumerateEventFiles(ws, null);
    assert.ok(files[0].endsWith('events-20261028T0100.jsonl'));
  });
});
