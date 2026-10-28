import assert from 'node:assert/strict';
import { test } from 'node:test';
import { openDatabase } from '../../src/db/index.js';
import { runMigrations } from '../../src/db/migrations.js';
import { ensureMachineId } from '../../src/lib/machine_id.js';
import { tmpDbPath } from '../helpers.js';

test('machine id is generated once and stable across reopen', () => {
  const dbPath = tmpDbPath();

  let db = openDatabase(dbPath);
  runMigrations(db);
  const first = ensureMachineId(db);
  assert.match(first, /^[0-9a-f-]{36}$/);
  // Second call inside the same connection reuses the stored id.
  assert.equal(ensureMachineId(db), first);
  db.close();

  db = openDatabase(dbPath);
  runMigrations(db);
  assert.equal(ensureMachineId(db), first);
  db.close();
});
