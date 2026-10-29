import assert from 'node:assert/strict';
import { describe, it, after } from 'node:test';
import { spawn } from 'node:child_process';
import { posixPidCheck } from '../../src/health/pid_check.js';

describe('posixPidCheck', () => {
  const children: ReturnType<typeof spawn>[] = [];
  after(() => {
    for (const c of children) c.kill('SIGKILL');
  });

  it('live child pid → alive (+ start verified when started_at matches)', () => {
    const child = spawn('sleep', ['30']);
    children.push(child);
    const pid = child.pid!;
    const res = posixPidCheck(pid, new Date().toISOString());
    assert.equal(res.alive, true);
    // Linux /proc present → start cross-check should confirm;
    // platforms without /proc degrade to unverified (plan §15).
    if (res.startVerified !== null) {
      assert.equal(res.startVerified, true);
    }
  });

  it('dead pid → alive:false', () => {
    // Find a free pid by spawning and reaping one.
    const child = spawn('true', []);
    const pid = child.pid!;
    return new Promise<void>((resolve) => {
      child.on('exit', () => {
        const res = posixPidCheck(pid, null);
        assert.equal(res.alive, false);
        assert.equal(res.startVerified, null);
        resolve();
      });
    });
  });

  it('invalid pid → dead + unverified (never throws)', () => {
    for (const pid of [0, -1, 1.5]) {
      const res = posixPidCheck(pid, null);
      assert.equal(res.alive, false);
      assert.equal(res.startVerified, null);
    }
  });

  it('live pid with far-future started_at → startVerified:false or null', () => {
    const child = spawn('sleep', ['30']);
    children.push(child);
    const res = posixPidCheck(child.pid!, '2999-01-01T00:00:00Z');
    assert.equal(res.alive, true);
    assert.equal(res.startVerified === true, false);
  });
});
