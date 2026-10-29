/**
 * Unit-graph derivation (Plan2-U2): per-unit status state machine
 * from `forge.unit.*` events. When the run's `scheduler` passthrough
 * is present (dag.db exists), `source: "scheduler"` overlays
 * admission counters; otherwise `source: "forge-events"`.
 *
 * Status machine (first terminal wins — a `ready` after `executed`
 * is an illegal backward move, kept + counted as diagnostic):
 *   ready → executed | failed | verified | correction → …
 *   run reached terminal without executed → `abandoned`.
 */

import type { EventRowLite } from './progress.js';

export interface GraphNode {
  unit: string;
  status: 'ready' | 'executed' | 'failed' | 'verified' | 'correction' | 'running' | 'abandoned';
  last_seq: number;
}

export interface GraphResult {
  nodes: GraphNode[];
  edges: { from: string; to: string }[];
  source: 'forge-events' | 'scheduler' | 'none';
  scheduler?: Record<string, unknown> | null;
  illegal_transitions: number;
}

const TERMINAL_STATES = new Set(['executed', 'failed', 'verified', 'abandoned']);

function topicVerb(topic: string, prefix: string): string | null {
  return topic.startsWith(prefix) ? topic.slice(prefix.length) : null;
}

function unitOf(payload: string | null): string | null {
  if (!payload) return null;
  try {
    const p = JSON.parse(payload) as Record<string, unknown>;
    for (const k of ['unit_key', 'unit', 'unitKey', 'unit_id']) {
      if (typeof p[k] === 'string') return p[k] as string;
    }
  } catch { /* ignore */ }
  return null;
}

export function deriveGraph(
  rows: EventRowLite[],
  descriptor: Record<string, unknown> | null,
  scheduler: Record<string, unknown> | null,
  runTerminal: boolean,
): GraphResult {
  const dag = descriptor?.dag as Record<string, unknown> | undefined;
  const prefix = (dag?.unit_topic_prefix as string | undefined) ?? 'forge.unit.';
  const dagEnabled = (dag?.enabled as boolean | undefined) ?? false;

  const nodes = new Map<string, GraphNode>();
  let illegal = 0;

  const transition = (unit: string, next: GraphNode['status'], seq: number) => {
    const cur = nodes.get(unit);
    if (!cur) {
      nodes.set(unit, { unit, status: next, last_seq: seq });
      return;
    }
    if (TERMINAL_STATES.has(cur.status)) {
      illegal += 1; // illegal backward move — keep first terminal state
      return;
    }
    cur.status = next;
    cur.last_seq = seq;
  };

  for (const row of rows) {
    if (!row.topic) continue;
    const verb = topicVerb(row.topic, prefix);
    if (!verb) continue;
    if (!dagEnabled && !row.topic.startsWith('forge.')) continue;
    const unit = unitOf(row.payload);
    if (!unit) continue;
    switch (verb) {
      case 'ready':
        transition(unit, 'ready', row.seq);
        break;
      case 'executed':
        transition(unit, 'executed', row.seq);
        break;
      case 'execution_failed':
      case 'verification_failed':
      case 'failed':
        transition(unit, 'failed', row.seq);
        break;
      case 'verified':
        transition(unit, 'verified', row.seq);
        break;
      case 'correction':
        transition(unit, 'correction', row.seq);
        break;
      default:
        break;
    }
  }

  // Units that never reached a terminal state: running while the run
  // is live, abandoned once the run is done.
  for (const n of nodes.values()) {
    if (!TERMINAL_STATES.has(n.status) && n.status !== 'correction') {
      n.status = runTerminal ? 'abandoned' : 'running';
    } else if (runTerminal && n.status === 'correction') {
      n.status = 'abandoned';
    }
  }

  const list = [...nodes.values()].sort((a, b) => a.last_seq - b.last_seq || a.unit.localeCompare(b.unit));
  // Sequential edges in observation order (plan keeps edge evidence
  // minimal; real DAG edges come from scheduler receipts later).
  const edges = list.slice(1).map((n, i) => ({ from: list[i].unit, to: n.unit }));

  const source = nodes.size === 0
    ? (scheduler ? 'scheduler' : 'none')
    : scheduler
      ? 'scheduler'
      : 'forge-events';

  return {
    nodes: list,
    edges,
    source,
    scheduler: scheduler ?? null,
    illegal_transitions: illegal,
  };
}
