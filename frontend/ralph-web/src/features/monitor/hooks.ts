/**
 * Monitor react-query hooks. Query keys are namespaced under
 * `["monitor", ...]` so they never collide with dashboard queries.
 * `refetchInterval` matches the collector's 3s poll cadence (U3).
 */

import { keepPreviousData, useQueries, useQuery } from "@tanstack/react-query";
import {
  apiGetEvents,
  apiGetGraph,
  apiGetProgress,
  apiGetProjectHealth,
  apiGetProjects,
  apiGetRun,
  apiGetRuns,
  type EventsQuery,
  type MonitorEvent,
  type RunsQuery,
} from "./api";
import { useCallback, useEffect, useRef, useState } from "react";

export const MONITOR_REFETCH_MS = 3_000;

export function useMonitorProjects() {
  return useQuery({
    queryKey: ["monitor", "projects"],
    queryFn: apiGetProjects,
    refetchInterval: MONITOR_REFETCH_MS,
  });
}

export function useMonitorRuns(query: RunsQuery = {}) {
  return useQuery({
    queryKey: ["monitor", "runs", query],
    queryFn: () => apiGetRuns(query),
    refetchInterval: MONITOR_REFETCH_MS,
    // Keep showing the previous page while a filter change refetches —
    // otherwise a new query key flashes the loading skeleton and the
    // filter bar unmounts mid-keystroke.
    placeholderData: keepPreviousData,
  });
}

export function useMonitorRunDetail(globalRunId: string | undefined) {
  return useQuery({
    queryKey: ["monitor", "run", globalRunId],
    queryFn: () => apiGetRun(globalRunId!),
    enabled: !!globalRunId,
    refetchInterval: MONITOR_REFETCH_MS,
  });
}

export function useMonitorProjectHealth(projectId: string | undefined) {
  return useQuery({
    queryKey: ["monitor", "health", projectId],
    queryFn: () => apiGetProjectHealth(projectId!),
    enabled: !!projectId,
    refetchInterval: MONITOR_REFETCH_MS,
  });
}

/** Health across all registered projects → ids whose collector errored. */
export function useMonitorProjectsHealth(projectIds: string[]) {
  const results = useQueries({
    queries: projectIds.map((id) => ({
      queryKey: ["monitor", "health", id] as const,
      queryFn: () => apiGetProjectHealth(id),
      refetchInterval: MONITOR_REFETCH_MS,
    })),
  });
  return results
    .map((r, i) => ({ id: projectIds[i], health: r.data }))
    .filter((x) => x.health?.collector === "error")
    .map((x) => x.id);
}

// ── Plan2-U2 detail hooks ──

/** Poll only while the run is live; terminal runs freeze after idle. */
function liveInterval(terminal: boolean, idleMs: number): number | false {
  if (terminal) return false;
  void idleMs;
  return MONITOR_REFETCH_MS;
}

export function useRunProgress(
  globalRunId: string | undefined,
  terminal: boolean,
) {
  return useQuery({
    queryKey: ["monitor", "progress", globalRunId],
    queryFn: () => apiGetProgress(globalRunId!),
    enabled: !!globalRunId,
    refetchInterval: liveInterval(terminal, 0),
  });
}

export function useRunGraph(globalRunId: string | undefined, enabled: boolean) {
  return useQuery({
    queryKey: ["monitor", "graph", globalRunId],
    queryFn: () => apiGetGraph(globalRunId!),
    enabled: !!globalRunId && enabled,
  });
}

export interface EventPage {
  events: MonitorEvent[];
  nextCursor: number;
  hasMore: boolean;
  watermark: string | null;
}

/**
 * Incremental event feed: fetches `cursor` (latest seen seq) every
 * poll and merges unseen events by seq. `paused` freezes the feed —
 * no fetch, no merge. `loadEarlier` walks backward via the `before`
 * window (cursor paging is forward-only; earlier pages re-query from
 * 0 with an upper bound client-side — the API returns seq-ordered
 * rows so we keep an offset window for history loading).
 */
export function useRunEvents(
  globalRunId: string | undefined,
  opts: { paused?: boolean; filter?: Omit<EventsQuery, "cursor"> } = {},
) {
  const [events, setEvents] = useState<MonitorEvent[]>([]);
  const [watermark, setWatermark] = useState<string | null>(null);
  const cursorRef = useRef(0);
  const filterKey = JSON.stringify(opts.filter ?? {});

  // Reset accumulated state when the run or filter changes.
  useEffect(() => {
    setEvents([]);
    cursorRef.current = 0;
  }, [globalRunId, filterKey]);

  const query = useQuery({
    queryKey: ["monitor", "events", globalRunId, filterKey, cursorRef.current],
    queryFn: async (): Promise<EventPage> => {
      const res = await apiGetEvents(globalRunId!, {
        ...opts.filter,
        cursor: cursorRef.current,
        limit: 500,
      });
      return {
        events: res.events,
        nextCursor: res.next_cursor,
        hasMore: res.has_more,
        watermark: res.watermark,
      };
    },
    enabled: !!globalRunId && !opts.paused,
    refetchInterval: MONITOR_REFETCH_MS,
  });

  // Merge newly-arrived page; dedupe by seq (poller may re-yield the
  // boundary row).
  useEffect(() => {
    const data = query.data;
    if (!data) return;
    setWatermark(data.watermark);
    if (data.events.length === 0) return;
    cursorRef.current = data.nextCursor;
    setEvents((prev) => {
      const seen = new Set(prev.map((e) => e.seq));
      const fresh = data.events.filter((e) => !seen.has(e.seq));
      return fresh.length ? [...prev, ...fresh] : prev;
    });
  }, [query.data]);

  /** Load earlier history: page backwards by re-fetching from 0. */
  const earlier = useCallback(async (): Promise<void> => {
    if (!globalRunId) return;
    const res = await apiGetEvents(globalRunId, {
      ...opts.filter,
      cursor: 0,
      limit: 1000,
    });
    setEvents(res.events);
    cursorRef.current = res.next_cursor;
  }, [globalRunId, filterKey]);

  return {
    events,
    watermark,
    isLoading: query.isLoading,
    isError: query.isError,
    loadEarlier: earlier,
  };
}
