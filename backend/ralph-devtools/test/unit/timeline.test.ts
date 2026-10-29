import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { deriveTimeline } from '../../src/report/timeline.js';
import type { EventRecord } from '../../src/report/events.js';

function ev(partial: Partial<EventRecord>): EventRecord {
  return {
    ts: '2026-01-01T00:00:00Z',
    iteration: 0,
    hat: 'h',
    topic: 't',
    payload: '',
    ...partial,
  };
}

describe('deriveTimeline', () => {
  it('rolls up per-iteration stats', () => {
    const tl = deriveTimeline([
      ev({ iteration: 0, hat: 'a', topic: 'start', ts: '2026-01-01T00:00:00Z' }),
      ev({ iteration: 1, hat: 'a', topic: 'work.done', ts: '2026-01-01T00:01:00Z' }),
      ev({ iteration: 1, hat: 'b', topic: 'review.done', ts: '2026-01-01T00:02:00Z' }),
    ]);
    assert.equal(tl.totalEvents, 3);
    assert.equal(tl.iterations.length, 2);
    assert.equal(tl.iterations[1].events, 2);
    assert.deepEqual(tl.iterations[1].hats, ['a', 'b']);
    assert.equal(tl.firstTs, '2026-01-01T00:00:00Z');
    assert.equal(tl.lastTs, '2026-01-01T00:02:00Z');
  });

  it('groups contiguous hat spans', () => {
    const tl = deriveTimeline([
      ev({ hat: 'a', topic: 'x' }),
      ev({ hat: 'a', topic: 'y' }),
      ev({ hat: 'b', topic: 'z' }),
      ev({ hat: 'a', topic: 'w' }),
    ]);
    assert.equal(tl.hatSpans.length, 3);
    assert.equal(tl.hatSpans[0].hat, 'a');
    assert.equal(tl.hatSpans[0].events, 2);
    assert.equal(tl.hatSpans[2].hat, 'a');
    assert.equal(tl.hatSpans[2].firstSeq, 3);
  });

  it('collects wave groups', () => {
    const tl = deriveTimeline([
      ev({ hat: 'w1', topic: 'review.file', wave_id: 'wv1', wave_index: 0 }),
      ev({ hat: 'w2', topic: 'review.file', wave_id: 'wv1', wave_index: 1 }),
      ev({ hat: 'agg', topic: 'review.done' }),
    ]);
    assert.equal(tl.waves.length, 1);
    assert.equal(tl.waves[0].events, 2);
    assert.deepEqual(tl.waves[0].hats, ['w1', 'w2']);
  });

  it('identifies terminal-looking topics', () => {
    const tl = deriveTimeline([
      ev({ hat: 'r', topic: 'report.done' }),
      ev({ hat: 'x', topic: 'LOOP_COMPLETE' }),
    ]);
    assert.equal(tl.terminalTopics.length, 2);
  });

  it('empty input → empty timeline', () => {
    const tl = deriveTimeline([]);
    assert.equal(tl.totalEvents, 0);
    assert.equal(tl.iterations.length, 0);
  });
});
