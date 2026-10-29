/**
 * Event-history access — always through `ralph events --format json`
 * (the sanctioned read path; the tool never opens `.ralph/events.jsonl`
 * itself, mirroring the monitor's boundary discipline).
 */

import { runRalphJson, type RalphResult, type RalphRunOptions } from '../ralph/runner.js';

/** Mirrors `EventRecord` in `crates/ralph-core/src/event_logger.rs`. */
export interface EventRecord {
  ts: string;
  iteration: number;
  hat: string;
  topic: string;
  triggered?: string;
  payload: string;
  blocked_count?: number;
  wave_id?: string;
  wave_index?: number;
  wave_total?: number;
  _phase?: string;
}

export interface FetchEventsArgs {
  /** Workspace dir containing `.ralph/` (default: cwd). */
  cwd?: string;
  last?: number;
  topic?: string;
  /** Explicit events file (overrides workspace resolution). */
  file?: string;
  eventsSource?: 'auto' | 'main' | 'hat-channel';
}

export function fetchEvents(
  args: FetchEventsArgs = {},
  opts: RalphRunOptions = {},
): Promise<RalphResult<EventRecord[]>> {
  const argv = ['events', '--format', 'json'];
  if (args.last !== undefined) argv.push('--last', String(args.last));
  if (args.topic !== undefined) argv.push('--topic', args.topic);
  if (args.file !== undefined) argv.push('--file', args.file);
  else if (args.eventsSource !== undefined) {
    argv.push('--events-source', args.eventsSource);
  }
  return runRalphJson<EventRecord[]>(argv, { ...opts, cwd: args.cwd ?? opts.cwd });
}
