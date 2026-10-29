/**
 * Progress derivation (Plan2-U2): pure function over the projected
 * event rows + the run's monitor descriptor.
 *
 * - `stage` = the last stage (in descriptor order) whose topics have
 *   appeared; `stage_history` records the first seq each stage was
 *   entered. For descriptors with re-entrant stages a
 *   stage can be re-entered — history records every transition, and
 *   `round` counts re-entries of the current stage.
 * - `units` counts distinct unit identities seen under the
 *   descriptor's `dag.unit_topic_prefix` topics.
 */

export interface EventRowLite {
  seq: number;
  ts: string | null;
  iteration: number | null;
  hat: string | null;
  topic: string | null;
  payload: string | null;
}

export interface DescriptorStage {
  id: string;
  label: string;
  topics: string[];
}

export interface ProgressResult {
  stage: string | null;
  stage_label: string | null;
  stage_history: { stage: string; at_seq: number; round: number }[];
  current_hat: string | null;
  iteration: number | null;
  elapsed_ms: number | null;
  units: { done: number; total: number | null; source: string };
  denominator: string;
}

const DONE_UNIT_TOPICS = new Set([
  'forge.unit.executed',
  'forge.unit.verified',
  'forge.exec.development.done',
]);

function unitKeyFrom(topic: string, payload: string | null): string | null {
  if (!payload) return null;
  try {
    const p = JSON.parse(payload) as Record<string, unknown>;
    for (const k of ['unit_key', 'unit', 'unitKey', 'unit_id', 'task_key']) {
      if (typeof p[k] === 'string') return p[k] as string;
    }
  } catch {
    /* non-JSON payload — fall through to topic-tail heuristic */
  }
  // forge.unit.<verb> topics carry the unit in the payload; the topic
  // tail alone is not an identity → null rather than a fake key.
  void topic;
  return null;
}

export function deriveProgress(
  rows: EventRowLite[],
  descriptor: Record<string, unknown> | null,
  run: { started_at: string | null; terminal: string | null },
  nowMs: number = Date.now(),
): ProgressResult {
  const stages = (descriptor?.stages ?? []) as DescriptorStage[];
  const topicToStage = new Map<string, number>();
  stages.forEach((s, i) => s.topics.forEach((t) => {
    if (!topicToStage.has(t)) topicToStage.set(t, i);
  }));
  const dagPrefix =
    ((descriptor?.dag as Record<string, unknown> | undefined)
      ?.unit_topic_prefix as string | undefined) ?? 'forge.unit.';
  const dagEnabled =
    ((descriptor?.dag as Record<string, unknown> | undefined)
      ?.enabled as boolean | undefined) ?? false;

  const history: { stage: string; at_seq: number; round: number }[] = [];
  const roundByStage = new Map<string, number>();
  let stageIdx = -1;
  let lastHat: string | null = null;
  let iteration: number | null = null;
  let firstTs: string | null = null;
  let lastTs: string | null = null;
  const unitKeys = new Set<string>();
  const doneUnits = new Set<string>();
  let declaredTotal: number | null = null;

  for (const row of rows) {
    if (!firstTs && row.ts) firstTs = row.ts;
    if (row.ts) lastTs = row.ts;
    if (row.hat) lastHat = row.hat;
    if (typeof row.iteration === 'number') iteration = row.iteration;
    if (!row.topic) continue;

    const idx = topicToStage.get(row.topic);
    if (idx !== undefined) {
      const s = stages[idx];
      if (idx > stageIdx) {
        roundByStage.set(s.id, (roundByStage.get(s.id) ?? 0) + 1);
        history.push({ stage: s.id, at_seq: row.seq, round: roundByStage.get(s.id)! });
        stageIdx = idx;
      } else if (idx < stageIdx && stages[idx].id !== stages[stageIdx].id) {
        // Loop descriptor re-entry (e.g. fix.done → review round).
        roundByStage.set(s.id, (roundByStage.get(s.id) ?? 0) + 1);
        history.push({ stage: s.id, at_seq: row.seq, round: roundByStage.get(s.id)! });
        stageIdx = idx;
      }
    }

    if (dagEnabled || row.topic.startsWith(dagPrefix) || row.topic.startsWith('forge.exec.')) {
      if (row.topic === 'forge.exec.development.done') {
        try {
          const p = JSON.parse(row.payload ?? '{}') as Record<string, unknown>;
          const t = p['units_total'] ?? p['total'];
          if (typeof t === 'number') declaredTotal = t;
        } catch { /* ignore */ }
      }
      if (row.topic.startsWith(dagPrefix)) {
        const key = unitKeyFrom(row.topic, row.payload);
        if (key) {
          unitKeys.add(key);
          if (DONE_UNIT_TOPICS.has(row.topic)) doneUnits.add(key);
        }
      }
    }
  }

  const startMs = run.started_at ? Date.parse(run.started_at) : NaN;
  const endMs = run.terminal
    ? Date.parse(JSON.parse(run.terminal).at ?? '')
    : NaN;
  const elapsedMs = Number.isFinite(startMs)
    ? (Number.isFinite(endMs) ? endMs : (lastTs ? Date.parse(lastTs) : nowMs)) - startMs
    : firstTs
      ? (lastTs ? Date.parse(lastTs) : nowMs) - Date.parse(firstTs)
      : null;

  const stage = stageIdx >= 0 ? stages[stageIdx] : null;
  return {
    stage: stage?.id ?? null,
    stage_label: stage?.label ?? null,
    stage_history: history,
    current_hat: lastHat,
    iteration,
    elapsed_ms: elapsedMs !== null && elapsedMs >= 0 ? elapsedMs : null,
    units: {
      done: doneUnits.size,
      total: declaredTotal ?? (unitKeys.size || null),
      source: unitKeys.size || declaredTotal ? 'forge-events' : 'none',
    },
    denominator: stages.length ? 'stages' : 'none',
  };
}
