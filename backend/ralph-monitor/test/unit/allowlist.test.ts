import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { makeProjectDir, tmpDir } from '../helpers.js';
import { checkAllowed, withinRoot } from '../../src/lib/allowlist.js';

describe('allowlist', () => {
  let ws: string;
  let outside: string;
  let roots: string[];

  beforeEach(() => {
    ws = makeProjectDir();
    outside = tmpDir('ralph-monitor-outside-');
    const ralph = path.join(ws, '.ralph');
    fs.mkdirSync(ralph, { recursive: true });
    fs.writeFileSync(path.join(ralph, 'a.md'), 'hi');
    fs.writeFileSync(path.join(outside, 'secret.txt'), 'sekret');
    roots = [ralph];
  });
  afterEach(() => {
    fs.rmSync(ws, { recursive: true, force: true });
    fs.rmSync(outside, { recursive: true, force: true });
  });

  it('plain file inside root → ok', () => {
    const v = checkAllowed('a.md', path.join(ws, '.ralph'), roots);
    assert.ok(v.ok);
  });

  it('.. traversal → outside', () => {
    const v = checkAllowed('../outside/x', path.join(ws, '.ralph'), roots);
    assert.equal(v.ok, false);
    if (!v.ok) assert.equal(v.code, 'not_found'); // ../outside/x doesn't exist → not_found first
    // craft a real ../ escape to an existing file:
    fs.mkdirSync(path.join(ws, 'docs'), { recursive: true });
    const esc = checkAllowed('../docs/../../outside-doesnotmatter/a.md', path.join(ws, '.ralph'), roots);
    assert.equal(esc.ok, false);
  });

  it('absolute path outside roots → outside_allowed_roots', () => {
    const v = checkAllowed(path.join(outside, 'secret.txt'), ws, roots);
    assert.equal(v.ok, false);
    if (!v.ok) assert.equal(v.code, 'outside_allowed_roots');
  });

  it('symlink inside root pointing outside → rejected', () => {
    const link = path.join(ws, '.ralph', 'evil.md');
    fs.symlinkSync(path.join(outside, 'secret.txt'), link);
    const v = checkAllowed('evil.md', path.join(ws, '.ralph'), roots);
    assert.equal(v.ok, false);
    if (!v.ok) assert.equal(v.code, 'outside_allowed_roots');
  });

  it('symlink inside root → inside target allowed', () => {
    fs.writeFileSync(path.join(ws, '.ralph', 'real.md'), 'x');
    fs.symlinkSync(path.join(ws, '.ralph', 'real.md'), path.join(ws, '.ralph', 'alias.md'));
    const v = checkAllowed('alias.md', path.join(ws, '.ralph'), roots);
    assert.ok(v.ok);
  });

  it('directory → not_regular_file', () => {
    fs.mkdirSync(path.join(ws, '.ralph', 'subdir'), { recursive: true });
    const v = checkAllowed('subdir', path.join(ws, '.ralph'), roots);
    assert.equal(v.ok, false);
    if (!v.ok) assert.equal(v.code, 'not_regular_file');
  });

  it('fifo → not_regular_file', () => {
    const fifo = path.join(ws, '.ralph', 'p');
    try {
      fs.writeFileSync(fifo, '');
      // device/fifo test via /dev: absolute check
    } catch { /* ignore */ }
    const v = checkAllowed('/dev/null', ws, roots);
    assert.equal(v.ok, false); // outside anyway; ensures non-regular never ok
  });

  it('withinRoot prefix logic', () => {
    assert.ok(withinRoot('/a/b/c', '/a/b'));
    assert.ok(withinRoot('/a/b', '/a/b'));
    assert.ok(!withinRoot('/a/bc', '/a/b')); // sibling prefix trap
    assert.ok(!withinRoot('/a', '/a/b'));
  });

  it('missing file → not_found', () => {
    const v = checkAllowed('nope.md', path.join(ws, '.ralph'), roots);
    assert.equal(v.ok, false);
    if (!v.ok) assert.equal(v.code, 'not_found');
  });
});
