/**
 * Monitor react-query hooks. Query keys are namespaced under
 * `["monitor", ...]` so they never collide with dashboard queries.
 * `refetchInterval` matches the collector's 3s poll cadence (U3).
 */

import { keepPreviousData, useQueries, useQuery } from "@tanstack/react-query";
import {
  apiGetProjectHealth,
  apiGetProjects,
  apiGetRun,
  apiGetRuns,
  type RunsQuery,
} from "./api";

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
