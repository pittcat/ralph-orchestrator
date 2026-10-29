import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runRalphJson } from '../../src/ralph/runner.js';
import { buildChildEnv } from '../../src/ralph/env.js';

const fakeRalph = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
  'fixtures',
  'fake-ralph.sh',
);

function envWith(mode: string): NodeJS.ProcessEnv {
  return {
    ...buildChildEnv(),
    FAKE_RALPH_MODE: mode,
  };
}

describe('runRalphJson', () => {
  it('tolerates tracing WARN lines before the JSON body', async () => {
    const res = await runRalphJson<{ passed: boolean }>(['preset', 'check'], {
      ralphBin: fakeRalph,
      env: envWith('warn-json'),
    });
    assert.ok(res.ok);
    assert.equal(res.data.passed, true);
    assert.equal(res.exitCode, 0);
  });

  it('tolerateNonZero returns the FAIL report with exitCode=1', async () => {
    const res = await runRalphJson<{ passed: boolean }>(['preset', 'check'], {
      ralphBin: fakeRalph,
      env: envWith('fail-json'),
      tolerateNonZero: true,
    });
    assert.ok(res.ok);
    assert.equal(res.data.passed, false);
    assert.equal(res.exitCode, 1);
  });

  it('nonzero without tolerance → ok:false kind=nonzero', async () => {
    const res = await runRalphJson(['x'], {
      ralphBin: fakeRalph,
      env: envWith('nonzero'),
    });
    assert.equal(res.ok, false);
    if (!res.ok) assert.equal(res.error.kind, 'nonzero');
  });

  it('non-JSON stdout → bad_json', async () => {
    const res = await runRalphJson(['x'], {
      ralphBin: fakeRalph,
      env: envWith('bad'),
    });
    assert.equal(res.ok, false);
    if (!res.ok) assert.equal(res.error.kind, 'bad_json');
  });

  it('timeout kills the child', async () => {
    const res = await runRalphJson(['x'], {
      ralphBin: fakeRalph,
      env: envWith('hang'),
      timeoutMs: 300,
    });
    assert.equal(res.ok, false);
    if (!res.ok) assert.equal(res.error.kind, 'timeout');
  });

  it('spawn error surfaces when binary missing', async () => {
    const res = await runRalphJson(['x'], { ralphBin: '/nonexistent/ralph' });
    assert.equal(res.ok, false);
    if (!res.ok) assert.equal(res.error.kind, 'spawn_error');
  });
});

describe('buildChildEnv', () => {
  it('scrubs RALPH_* agent context but keeps allowlist + RALPH_DEVTOOLS_*', () => {
    const env = buildChildEnv({
      PATH: '/bin',
      HOME: '/h',
      RALPH_CURRENT_HAT: 'executor',
      RALPH_EVENTS_FILE: '/tmp/e.jsonl',
      RALPH_HATS_SOURCE: 'builtin:x',
      RALPH_CONFIG: 'ralph.yml',
      RALPH_DEVTOOLS_FOO: '1',
      SECRET_KEY: 's',
    });
    assert.equal(env.PATH, '/bin');
    assert.equal(env.RALPH_CURRENT_HAT, undefined);
    assert.equal(env.RALPH_EVENTS_FILE, undefined);
    assert.equal(env.RALPH_HATS_SOURCE, undefined);
    assert.equal(env.RALPH_CONFIG, undefined);
    assert.equal(env.RALPH_DEVTOOLS_FOO, '1');
    assert.equal(env.SECRET_KEY, undefined);
  });
});
