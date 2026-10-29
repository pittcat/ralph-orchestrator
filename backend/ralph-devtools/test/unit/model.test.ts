import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { modelFromYaml, loadPreset } from '../../src/preset/model.js';

const fixtures = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
  'fixtures',
);

function readFixture(name: string): string {
  return fs.readFileSync(path.join(fixtures, name), 'utf8');
}

describe('modelFromYaml', () => {
  it('projects hats with triggers/publishes/terminal fields', () => {
    const model = modelFromYaml(readFixture('preset-a.yml'), 'preset-a.yml', 'file');
    assert.equal(model.hats.length, 2);
    const worker = model.hats.find((h) => h.id === 'worker')!;
    assert.deepEqual(worker.triggers, ['review.start', 'review.changes_requested']);
    assert.deepEqual(worker.publishes, ['work.done', 'REVIEW_COMPLETE']);
    assert.deepEqual(worker.terminalEvents, ['work.done', 'REVIEW_COMPLETE']);
    assert.ok(worker.instructionsChars > 0);
    assert.ok(worker.extraKeys.includes('event_filter'));
    const reviewer = model.hats.find((h) => h.id === 'reviewer')!;
    assert.equal(reviewer.defaultPublishes, 'review.changes_requested');
  });

  it('captures event_loop + sections', () => {
    const model = modelFromYaml(readFixture('preset-a.yml'), 'a', 'file');
    assert.equal(model.eventLoop?.completion_promise, 'REVIEW_COMPLETE');
    assert.ok(model.sections.includes('event_loop'));
    assert.ok(model.sections.includes('cli'));
    assert.ok(!model.sections.includes('hats'));
  });

  it('tolerates yaml errors without throwing', () => {
    const model = modelFromYaml('hats: [unclosed', 'broken', 'file');
    assert.equal(model.hats.length, 0);
    assert.ok(model.yamlErrors.length > 0);
  });

  it('empty hats section → empty list', () => {
    const model = modelFromYaml('event_loop:\n  max_iterations: 3\n', 'x', 'file');
    assert.equal(model.hats.length, 0);
  });
});

describe('loadPreset', () => {
  it('loads a file preset', async () => {
    const res = await loadPreset('preset-a.yml', { cwd: fixtures });
    assert.ok(res.ok);
    assert.equal(res.model.source, 'file');
    assert.equal(res.model.hats.length, 2);
  });

  it('missing file → error', async () => {
    const res = await loadPreset('does-not-exist.yml', { cwd: fixtures });
    assert.equal(res.ok, false);
    if (!res.ok) assert.match(res.error, /cannot read preset file/);
  });
});
