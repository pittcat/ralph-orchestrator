import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { normalizeUserPath } from '../../src/lib/paths.js';
import { tmpDir } from '../helpers.js';

test('relative paths resolve to absolute canonical', () => {
  const dir = tmpDir();
  const sub = path.join(dir, 'project');
  fs.mkdirSync(sub);
  const res = normalizeUserPath('project', dir);
  assert.equal(res.ok, true);
  if (res.ok) assert.equal(res.canonical, fs.realpathSync(sub));
});

test('trailing slashes are normalized', () => {
  const dir = tmpDir();
  const res = normalizeUserPath(dir + path.sep);
  assert.equal(res.ok, true);
  if (res.ok) assert.equal(res.canonical, fs.realpathSync(dir));
});

test('nonexistent path is invalid_path', () => {
  const res = normalizeUserPath(path.join(tmpDir(), 'does-not-exist'));
  assert.equal(res.ok, false);
  if (!res.ok) assert.equal(res.code, 'invalid_path');
});

test('file that is not a directory is invalid_path', () => {
  const dir = tmpDir();
  const file = path.join(dir, 'a-file');
  fs.writeFileSync(file, 'x');
  const res = normalizeUserPath(file);
  assert.equal(res.ok, false);
  if (!res.ok) assert.equal(res.code, 'invalid_path');
});

test('.. traversal segments are rejected', () => {
  const res = normalizeUserPath('/tmp/../etc');
  assert.equal(res.ok, false);
});

test('~ expands to the home directory; ~user is rejected', () => {
  const home = os.homedir();
  const res = normalizeUserPath('~');
  assert.equal(res.ok, true);
  if (res.ok) assert.equal(res.canonical, fs.realpathSync(home));

  const bad = normalizeUserPath('~someuser/proj');
  assert.equal(bad.ok, false);
});

test('symlink resolves to its target', () => {
  const dir = tmpDir();
  const target = path.join(dir, 'real');
  const link = path.join(dir, 'alias');
  fs.mkdirSync(target);
  fs.symlinkSync(target, link);
  const res = normalizeUserPath(link);
  assert.equal(res.ok, true);
  if (res.ok) assert.equal(res.canonical, fs.realpathSync(target));
});

test('NUL bytes and over-length inputs are rejected', () => {
  assert.equal(normalizeUserPath(`a\0b`).ok, false);
  assert.equal(normalizeUserPath('x'.repeat(5000)).ok, false);
});
