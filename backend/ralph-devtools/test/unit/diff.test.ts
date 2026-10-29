import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { modelFromYaml } from '../../src/preset/model.js';
import { diffPresets, renderDiff } from '../../src/preset/diff.js';

const fixtures = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
  'fixtures',
);

function model(name: string) {
  const yaml = fs.readFileSync(path.join(fixtures, name), 'utf8');
  return modelFromYaml(yaml, name, 'file');
}

describe('diffPresets', () => {
  it('identical presets → identical:true', () => {
    const d = diffPresets(model('preset-a.yml'), model('preset-a.yml'));
    assert.equal(d.identical, true);
    assert.equal(d.entries.length, 0);
  });

  it('detects added/removed hats', () => {
    const d = diffPresets(model('preset-a.yml'), model('preset-b.yml'));
    assert.deepEqual(d.hatsAdded, ['verifier']);
    assert.deepEqual(d.hatsRemoved, []);
  });

  it('detects per-hat set + scalar changes', () => {
    const d = diffPresets(model('preset-a.yml'), model('preset-b.yml'));
    const paths = d.entries.map((e) => `${e.kind}:${e.path}`);
    // worker gained work.blocked in publishes
    assert.ok(paths.some((p) => p === 'added:hats.worker.publishes'));
    // max_iterations changed
    assert.ok(
      d.entries.some(
        (e) => e.path === 'event_loop.max_iterations' && e.kind === 'changed' && e.a === '20' || e.b === '20',
      ),
    );
    // instructions changed (string summary path)
    assert.ok(paths.some((p) => p === 'changed:hats.worker.instructions'));
  });

  it('renderDiff is readable and shows +/- lines', () => {
    const d = diffPresets(model('preset-a.yml'), model('preset-b.yml'));
    const text = renderDiff(d);
    assert.match(text, /\+ hat verifier/);
    assert.match(text, /max_iterations/);
  });

  it('renderDiff identical case', () => {
    const d = diffPresets(model('preset-a.yml'), model('preset-a.yml'));
    assert.match(renderDiff(d), /no structural differences/);
  });
});
