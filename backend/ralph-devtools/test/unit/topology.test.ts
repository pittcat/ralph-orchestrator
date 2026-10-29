import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { modelFromYaml } from '../../src/preset/model.js';
import { deriveTopology } from '../../src/preset/topology.js';

const fixtures = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
  'fixtures',
);

function model(name: string) {
  const yaml = fs.readFileSync(path.join(fixtures, name), 'utf8');
  return modelFromYaml(yaml, name, 'file');
}

describe('deriveTopology', () => {
  it('builds publisher --topic--> subscriber edges', () => {
    const topo = deriveTopology(model('preset-a.yml'));
    const edge = (f: string, t: string, topic: string) =>
      topo.edges.some((e) => e.from === f && e.to === t && e.topic === topic);
    assert.ok(edge('worker', 'reviewer', 'work.done'));
    assert.ok(edge('reviewer', 'worker', 'review.changes_requested'));
    assert.ok(edge('<external>', 'worker', 'review.start'));
  });

  it('marks operator-injected topics as sources', () => {
    const topo = deriveTopology(model('preset-a.yml'));
    assert.deepEqual(topo.sourceTopics, ['review.start']);
    assert.equal(topo.startingEvent, 'review.start');
  });

  it('sinks = published topics nobody subscribes to', () => {
    const topo = deriveTopology(model('preset-a.yml'));
    // review.done + REVIEW_COMPLETE have no subscribers.
    assert.ok(topo.sinkTopics.includes('review.done'));
    assert.ok(topo.sinkTopics.includes('REVIEW_COMPLETE'));
    assert.ok(!topo.sinkTopics.includes('work.done'));
  });

  it('reachability follows declared edges', () => {
    const topo = deriveTopology(model('preset-b.yml'));
    // verifier is reachable via worker → reviewer → verifier.
    assert.deepEqual(topo.unreachableHats, []);
    // work.blocked publishes to nobody → worker is a dead-end publisher
    // (verify.done is also a sink, so verifier is too).
    assert.ok(topo.deadEndHats.includes('verifier'));
  });

  it('flags hats a source topic can never reach (closed cycle)', () => {
    const m = modelFromYaml(
      `hats:
  a:
    triggers: [go]
    publishes: [mid]
  b:
    triggers: [mid]
    publishes: [end]
  island_x:
    triggers: [cycle.b]
    publishes: [cycle.a]
  island_y:
    triggers: [cycle.a]
    publishes: [cycle.b]
`,
      'x',
      'file',
    );
    const topo = deriveTopology(m);
    // a/b are reachable from the external 'go' source; the island pair
    // only triggers on each other's publishes — unreachable.
    assert.deepEqual(topo.unreachableHats, ['island_x', 'island_y']);
  });

  it('a hat triggered by a topic nobody publishes is reachable via <external>', () => {
    const m = modelFromYaml(
      `hats:
  island:
    triggers: [never.sent]
    publishes: [ignored]
`,
      'x',
      'file',
    );
    const topo = deriveTopology(m);
    // 'never.sent' is an operator-injectable source → reachable.
    assert.deepEqual(topo.unreachableHats, []);
    assert.deepEqual(topo.sourceTopics, ['never.sent']);
  });
});
