import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { parseSnapshot } from '../../src/ralph/snapshot.js';

const golden = JSON.parse(
  readFileSync(
    fileURLToPath(
      new URL('../fixtures/snapshots/golden_v1.json', import.meta.url),
    ),
    'utf8',
  ),
);

test('golden v1 fixture parses and applies evidence defaults', () => {
  const result = parseSnapshot(golden);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.snapshot.schema_version, 'run-observation/v1');
  assert.equal(result.snapshot.runs.length, 3);
  const legacy = result.snapshot.runs[2];
  assert.equal(legacy.state, 'unknown');
  assert.equal(legacy.evidence.manifest_present, false);
  assert.equal(legacy.evidence.lock_present, true);
});

test('schema_version drift → unsupported_version', () => {
  const result = parseSnapshot({ schema_version: 'run-observation/v2', runs: [] });
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.code, 'unsupported_version');
  assert.match(result.message, /run-observation\/v1/);
});

test('non-object / missing envelope → unsupported_version', () => {
  for (const raw of [null, 'str', 42, { runs: [] }]) {
    const result = parseSnapshot(raw);
    assert.equal(result.ok, false);
    if (!result.ok) assert.equal(result.code, 'unsupported_version');
  }
});

test('missing runs array → invalid_snapshot naming the path', () => {
  const result = parseSnapshot({
    schema_version: 'run-observation/v1',
    workspace: '/x',
    generated_at: 't',
  });
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.code, 'invalid_snapshot');
  assert.match(result.message, /runs/);
});

test('run missing run_id → invalid_snapshot with indexed path', () => {
  const result = parseSnapshot({
    schema_version: 'run-observation/v1',
    workspace: '/x',
    generated_at: 't',
    runs: [{ kind: 'primary', state: 'exited' }],
  });
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.code, 'invalid_snapshot');
  assert.match(result.message, /runs\.0\.run_id|run_id/);
});

test('extra unknown fields are tolerated (forward compat)', () => {
  const result = parseSnapshot({
    schema_version: 'run-observation/v1',
    workspace: '/x',
    generated_at: 't',
    future_field: { nested: true },
    runs: [
      {
        run_id: 'loop-x',
        kind: 'worktree',
        state: 'running',
        future_run_field: 1,
      },
    ],
  });
  assert.equal(result.ok, true);
});

test('corrupt-manifest observation parses (manifest_corrupt flag)', () => {
  const result = parseSnapshot({
    schema_version: 'run-observation/v1',
    workspace: '/x',
    generated_at: 't',
    runs: [
      {
        run_id: 'loop-broken',
        kind: 'unknown',
        state: 'unknown',
        manifest: '.ralph/runs/loop-broken.manifest.json',
        evidence: { manifest_corrupt: true },
      },
    ],
  });
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.snapshot.runs[0].evidence.manifest_corrupt, true);
});
