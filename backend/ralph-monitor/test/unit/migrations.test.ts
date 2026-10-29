import assert from 'node:assert/strict';
import { test } from 'node:test';
import { openDatabase } from '../../src/db/index.js';
import { runMigrations } from '../../src/db/migrations.js';

test('migrations are idempotent on an in-memory db', () => {
  const db = openDatabase(':memory:');
  runMigrations(db);
  runMigrations(db); // second run must not error
  const versions = db
    .prepare('SELECT version FROM schema_migrations ORDER BY version')
    .all() as { version: string }[];
  assert.deepEqual(
    versions.map((v) => v.version),
    ['0001_init', '0002_runs', '0003_events', '0004_activity'],
  );
  db.close();
});

test('0002 creates the runs/cursors/errors/audit tables', () => {
  const db = openDatabase(':memory:');
  runMigrations(db);
  const names = (
    db
      .prepare(
        `SELECT name FROM sqlite_master WHERE type='table' ORDER BY name`,
      )
      .all() as { name: string }[]
  ).map((t) => t.name);
  for (const t of [
    'runs',
    'source_cursors',
    'collection_errors',
    'audit_log',
  ]) {
    assert.ok(names.includes(t), `missing table ${t}`);
  }
  db.close();
});

test('a db with an unknown migration version refuses to start', () => {
  const db = openDatabase(':memory:');
  runMigrations(db);
  db.prepare(
    'INSERT INTO schema_migrations (version, applied_at) VALUES (?, ?)',
  ).run('9999_future', new Date().toISOString());
  assert.throws(() => runMigrations(db), /unknown migration version/);
  db.close();
});

test('0001 creates the projects and meta tables', () => {
  const db = openDatabase(':memory:');
  runMigrations(db);
  const tables = db
    .prepare(
      `SELECT name FROM sqlite_master WHERE type='table' ORDER BY name`,
    )
    .all() as { name: string }[];
  const names = tables.map((t) => t.name);
  assert.ok(names.includes('projects'));
  assert.ok(names.includes('meta'));
  assert.ok(names.includes('schema_migrations'));
  db.close();
});
