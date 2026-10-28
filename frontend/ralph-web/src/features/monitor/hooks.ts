/**
 * Monitor react-query hooks. Query keys are namespaced under
 * `["monitor", ...]` so they never collide with dashboard queries.
 * `refetchInterval` matches the collector's 3s poll cadence (U3).
 */

import { useQuery } from "@tanstack/react-query";
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
