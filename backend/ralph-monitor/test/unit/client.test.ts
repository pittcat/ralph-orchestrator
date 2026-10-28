import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  buildChildEnv,
  inspectRun,
  MAX_SNAPSHOT_BYTES,
} from '../../src/ralph/client.js';
import {
  fakeInspect,
  fakeRalphEnv,
  tmpDir,
  writeMonitorFixture,
} from '../helpers.js';

function fixtureDir(): string {
  const dir = tmpDir('ralph-client-test-');
  return dir;
}

test('success → parsed JSON + raw payload', async () => {
  const dir = fixtureDir();
  const result = await fakeInspect(dir);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const json = result.json as { schema_version: string };
  assert.equal(json.schema_version, 'run-observation/v1');
  assert.match(result.raw, /schema_version/);
});

test('nonzero exit → kind nonzero with stderr summary', async () => {
  const dir = fixtureDir();
  writeMonitorFixture(dir, { fail: 'nonzero' });
  const result = await fakeInspect(dir);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.error.kind, 'nonzero');
  assert.match(result.error.stderr ?? '', /fixture failure/);
});

test('non-JSON stdout → bad_json', async () => {
  const dir = fixtureDir();
  writeMonitorFixture(dir, { fail: 'badjson' });
  const result = await fakeInspect(dir);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.error.kind, 'bad_json');
});

test('sleep + short timeout → timeout kind', async () => {
  const dir = fixtureDir();
  writeMonitorFixture(dir, { fail: 'sleep' });
  const result = await fakeInspect(dir, { timeoutMs: 300 });
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.error.kind, 'timeout');
});

test('oversized stdout → payload_too_large', async () => {
  const dir = fixtureDir();
  writeMonitorFixture(dir, { fail: 'huge' });
  const result = await fakeInspect(dir);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.error.kind, 'payload_too_large');
  assert.ok(MAX_SNAPSHOT_BYTES <= 3 * 1024 * 1024);
});

test('missing binary → spawn_error', async () => {
  const dir = fixtureDir();
  const result = await inspectRun(dir, {
    ralphBin: 'ralph-definitely-not-on-path',
    env: fakeRalphEnv(),
  });
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.error.kind, 'spawn_error');
});

test('child env is allowlisted — RALPH_* agent keys are scrubbed', async () => {
  const dir = fixtureDir();
  writeMonitorFixture(dir, { fail: 'envcheck' });
  const result = await inspectRun(dir, {
    env: fakeRalphEnv({
      RALPH_CURRENT_HAT: 'executor',
      RALPH_EVENTS_FILE: '/tmp/leak.jsonl',
      RALPH_MONITOR_POLL_MS: '500',
    }),
  });
  assert.equal(result.ok, true);
  if (!result.ok) return;
  // envcheck echoes $RALPH_CURRENT_HAT into workspace — scrubbed to
  // empty means the allowlist stripped it.
  const json = result.json as { workspace: string };
  assert.equal(json.workspace, 'unset');
});

test('buildChildEnv keeps PATH/HOME and RALPH_MONITOR_* only', () => {
  const env = buildChildEnv({
    PATH: '/bin',
    HOME: '/h',
    RALPH_MONITOR_DB_PATH: '/db',
    RALPH_CONFIG: '/leak',
    SECRET_KEY: 'nope',
    PATH_INFO: 'also-no',
  });
  assert.deepEqual(env, {
    PATH: '/bin',
    HOME: '/h',
    RALPH_MONITOR_DB_PATH: '/db',
  });
});
