/**
 * Run timeline derivation from `EventRecord[]` (pure).
 *
 * Three complementary views:
 * - per-iteration rollups (events/hats/topics seen in each loop round),
 * - contiguous hat spans (when did each hat hold the floor),
 * - wave groups (`wave_id` fan-out correlation).
 */

import type { EventRecord } from './events.js';

export interface IterationRow {
  iteration: number;
  events: number;
  hats: string[];
  topics: Record<string, number>;
  firstTs: string | null;
  lastTs: string | null;
}

export interface HatSpan {
  hat: string;
  /** Index into the (seq-ordered) event list. */
  firstSeq: number;
  lastSeq: number;
  events: number;
  firstTs: string | null;
  lastTs: string | null;
  topicCounts: Record<string, number>;
}

export interface WaveGroup {
  wave_id: string;
  events: number;
  hats: string[];
  topics: Record<string, number>;
  firstTs: string | null;
  lastTs: string | null;
}

export interface Timeline {
  totalEvents: number;
  firstTs: string | null;
  lastTs: string | null;
  iterations: IterationRow[];
  hatSpans: HatSpan[];
  waves: WaveGroup[];
  /** Top topics by frequency, descending. */
  topTopics: { topic: string; count: number }[];
  terminalTopics: { topic: string; hat: string; seq: number }[];
}

const TERMINAL_RE = /^(LOOP_COMPLETE|.*_COMPLETE|.*\.blocked|.*\.failed|.*\.done|.*\.complete)$/;

export function deriveTimeline(records: EventRecord[]): Timeline {
  const iterations = new Map<number, IterationRow>();
  const hatSpans: HatSpan[] = [];
  const waves = new Map<string, WaveGroup>();
  const topicCounts = new Map<string, number>();
  const terminalTopics: Timeline['terminalTopics'] = [];

  let firstTs: string | null = null;
  let lastTs: string | null = null;
  let curSpan: HatSpan | null = null;

  records.forEach((r, seq) => {
    if (!firstTs && r.ts) firstTs = r.ts;
    if (r.ts) lastTs = r.ts;

    // Per-iteration rollup.
    let row = iterations.get(r.iteration);
    if (!row) {
      row = { iteration: r.iteration, events: 0, hats: [], topics: {}, firstTs: null, lastTs: null };
      iterations.set(r.iteration, row);
    }
    row.events += 1;
    if (r.hat && !row.hats.includes(r.hat)) row.hats.push(r.hat);
    if (r.topic) row.topics[r.topic] = (row.topics[r.topic] ?? 0) + 1;
    if (r.ts) {
      if (!row.firstTs) row.firstTs = r.ts;
      row.lastTs = r.ts;
    }

    // Contiguous hat spans.
    const hat = r.hat || '(unknown)';
    if (!curSpan || curSpan.hat !== hat) {
      curSpan = {
        hat,
        firstSeq: seq,
        lastSeq: seq,
        events: 0,
        firstTs: r.ts || null,
        lastTs: r.ts || null,
        topicCounts: {},
      };
      hatSpans.push(curSpan);
    }
    curSpan.lastSeq = seq;
    curSpan.events += 1;
    if (r.ts) curSpan.lastTs = r.ts;
    if (r.topic) curSpan.topicCounts[r.topic] = (curSpan.topicCounts[r.topic] ?? 0) + 1;

    // Wave groups.
    if (r.wave_id) {
      let w = waves.get(r.wave_id);
      if (!w) {
        w = { wave_id: r.wave_id, events: 0, hats: [], topics: {}, firstTs: null, lastTs: null };
        waves.set(r.wave_id, w);
      }
      w.events += 1;
      if (r.hat && !w.hats.includes(r.hat)) w.hats.push(r.hat);
      if (r.topic) w.topics[r.topic] = (w.topics[r.topic] ?? 0) + 1;
      if (r.ts) {
        if (!w.firstTs) w.firstTs = r.ts;
        w.lastTs = r.ts;
      }
    }

    if (r.topic) {
      topicCounts.set(r.topic, (topicCounts.get(r.topic) ?? 0) + 1);
      if (TERMINAL_RE.test(r.topic)) {
        terminalTopics.push({ topic: r.topic, hat: hat, seq });
      }
    }
  });

  return {
    totalEvents: records.length,
    firstTs,
    lastTs,
    iterations: [...iterations.values()].sort((a, b) => a.iteration - b.iteration),
    hatSpans,
    waves: [...waves.values()].sort((a, b) => a.wave_id.localeCompare(b.wave_id)),
    topTopics: [...topicCounts.entries()]
      .map(([topic, count]) => ({ topic, count }))
      .sort((a, b) => b.count - a.count || a.topic.localeCompare(b.topic)),
    terminalTopics,
  };
}
