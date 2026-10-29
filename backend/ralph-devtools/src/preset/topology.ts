/**
 * Event-topology derivation (display-only).
 *
 * Edges are `publisher --topic--> subscriber` between hats. Topics
 * nobody publishes (e.g. `event_loop.starting_event`, operator-injected
 * `ralph emit`) are sources; topics nobody subscribes to (e.g.
 * `completion_promise`) are sinks. Orphan/unreachable hints are
 * advisory — `ralph preset check` remains the authority on validity.
 */

import type { HatModel, PresetModel } from './model.js';

export interface TopologyEdge {
  from: string; // hat id, or '<external>' for operator/runtime-injected topics
  to: string; // hat id
  topic: string;
}

export interface Topology {
  edges: TopologyEdge[];
  /** Topics that enter the graph without a hat publisher. */
  sourceTopics: string[];
  /** Published topics with no subscribing hat. */
  sinkTopics: string[];
  /** Hats no edge reaches (and not triggered by a source topic). */
  unreachableHats: string[];
  /** Hats that publish nothing any hat subscribes to. */
  deadEndHats: string[];
  startingEvent?: string;
  completionPromise?: string;
}

const EXTERNAL = '<external>';

export function deriveTopology(model: PresetModel): Topology {
  const el = model.eventLoop ?? {};
  const startingEvent =
    typeof el.starting_event === 'string' ? el.starting_event : undefined;
  const completionPromise =
    typeof el.completion_promise === 'string' ? el.completion_promise : undefined;

  const subscribers = new Map<string, HatModel[]>();
  for (const hat of model.hats) {
    for (const topic of hat.triggers) {
      const list = subscribers.get(topic) ?? [];
      list.push(hat);
      subscribers.set(topic, list);
    }
  }

  const edges: TopologyEdge[] = [];
  const publishedTopics = new Set<string>();
  for (const hat of model.hats) {
    const topics = new Set([...hat.publishes, ...hat.terminalEvents]);
    if (hat.defaultPublishes) topics.add(hat.defaultPublishes);
    for (const topic of topics) {
      publishedTopics.add(topic);
      for (const sub of subscribers.get(topic) ?? []) {
        edges.push({ from: hat.id, to: sub.id, topic });
      }
    }
  }

  // External sources: subscribed but never published by a hat.
  const triggeredTopics = new Set(subscribers.keys());
  const sourceTopics = [...triggeredTopics]
    .filter((t) => !publishedTopics.has(t))
    .sort();
  for (const topic of sourceTopics) {
    for (const sub of subscribers.get(topic) ?? []) {
      edges.push({ from: EXTERNAL, to: sub.id, topic });
    }
  }

  const sinkTopics = [...publishedTopics]
    .filter((t) => !triggeredTopics.has(t))
    .sort();

  // Reachability over declared edges (advisory display only).
  const reachable = new Set<string>();
  const queue: string[] = [];
  for (const e of edges) {
    if (e.from === EXTERNAL && !reachable.has(e.to)) {
      reachable.add(e.to);
      queue.push(e.to);
    }
  }
  while (queue.length > 0) {
    const cur = queue.shift()!;
    for (const e of edges) {
      if (e.from === cur && !reachable.has(e.to)) {
        reachable.add(e.to);
        queue.push(e.to);
      }
    }
  }
  const unreachableHats = model.hats
    .map((h) => h.id)
    .filter((id) => !reachable.has(id))
    .sort();

  const deadEndHats = model.hats
    .filter((h) => {
      const topics = new Set([...h.publishes, ...h.terminalEvents]);
      if (h.defaultPublishes) topics.add(h.defaultPublishes);
      if (topics.size === 0) return h.triggers.length > 0;
      for (const t of topics) {
        if (triggeredTopics.has(t)) return false;
        if (t === completionPromise) return false;
      }
      return true;
    })
    .map((h) => h.id)
    .sort();

  return {
    edges,
    sourceTopics,
    sinkTopics,
    unreachableHats,
    deadEndHats,
    ...(startingEvent !== undefined ? { startingEvent } : {}),
    ...(completionPromise !== undefined ? { completionPromise } : {}),
  };
}
