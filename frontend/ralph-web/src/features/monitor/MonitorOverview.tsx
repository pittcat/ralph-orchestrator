/**
 * U4 placeholder overview: an empty state guiding project
 * registration while the API is wired but the table is not (U5).
 * Uses real data — the empty state shows only when no runs exist.
 */

import { FolderPlus } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";
import { useMonitorProjects, useMonitorRuns } from "./hooks";

export function MonitorOverview() {
  const projects = useMonitorProjects();
  const runs = useMonitorRuns({ limit: 50 });

  if (projects.isLoading || runs.isLoading) {
    return (
      <div className="space-y-3" data-testid="overview-loading">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-32 w-full" />
      </div>
    );
  }

  const runCount = runs.data?.total ?? 0;
  if (runCount > 0) {
    return (
      <div className="space-y-2" data-testid="overview-summary">
        <p className="text-sm text-muted-foreground">
          {runCount} run{runCount === 1 ? "" : "s"} observed across{" "}
          {(projects.data ?? []).length} project
          {(projects.data ?? []).length === 1 ? "" : "s"} — the runs
          table lands in the next unit.
        </p>
      </div>
    );
  }

  return (
    <div
      className="flex flex-col items-center gap-3 rounded-lg border border-dashed border-border p-10 text-center"
      data-testid="overview-empty"
    >
      <FolderPlus className="h-8 w-8 text-muted-foreground" />
      <p className="font-medium">No runs observed yet</p>
      <p className="max-w-md text-sm text-muted-foreground">
        Register a project directory to start collecting run
        observations — the monitor reads{" "}
        <code className="text-xs">ralph inspect run</code> snapshots on
        a 3s cadence.
      </p>
      <p className="text-xs text-muted-foreground">
        POST /api/monitor/v1/projects with a project path to register.
      </p>
    </div>
  );
}
