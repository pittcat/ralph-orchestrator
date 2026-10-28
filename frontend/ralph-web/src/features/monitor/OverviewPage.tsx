/**
 * Monitor overview — runs table with search / project / state /
 * preset filters, startedAt sort, and offset pagination. All filter
 * state lives in the URL (`useSearchParams`) so refresh/back-forward
 * restore the view exactly (R4).
 */

import { useMemo } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { ArrowDown, ArrowUp, FolderPlus } from "lucide-react";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Skeleton } from "@/components/ui/skeleton";
import { useMonitorProjects, useMonitorProjectsHealth, useMonitorRuns } from "./hooks";
import type { MonitorRun } from "./api";
import {
  decodeFilters,
  encodeFilters,
  PAGE_SIZE,
  toRunsQuery,
  type OverviewFilters,
} from "./filters";
import { FilterBar } from "./components/FilterBar";
import { Pagination } from "./components/Pagination";
import { RunStateBadge } from "./components/RunStateBadge";
import { StaleBanner } from "./components/StaleBanner";

function fmt(ts: string | null): string {
  if (!ts) return "—";
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleString();
}

export function OverviewPage() {
  const [params, setParams] = useSearchParams();
  const filters = decodeFilters(params);
  const navigate = useNavigate();

  const projects = useMonitorProjects();
  const runs = useMonitorRuns(toRunsQuery(filters));

  const setFilters = (patch: Partial<OverviewFilters>) => {
    // Any filter change resets pagination to page 1.
    const next = { ...filters, page: 1, ...patch };
    setParams(encodeFilters(next), { replace: false });
  };

  const projectName = new Map(
    (projects.data ?? []).map((p) => [p.id, p.displayName]),
  );

  // Local preset facet (plan U5 §19: no /facets endpoint — volume is
  // small, facet over the fetched page's rows + client filter).
  const pageRuns = runs.data?.runs ?? [];
  const presets = useMemo(
    () =>
      Array.from(new Set(pageRuns.map((r) => r.preset).filter(Boolean) as string[])).sort(),
    [pageRuns],
  );
  const visibleRuns = filters.preset
    ? pageRuns.filter((r) => r.preset === filters.preset)
    : pageRuns;

  // Stale banner: projects whose health shows collector=error.
  const projectIds = useMemo(
    () => (projects.data ?? []).map((p) => p.id),
    [projects.data],
  );
  const staleIds = useMonitorProjectsHealth(projectIds);
  const staleProjects = (projects.data ?? []).filter((p) => staleIds.includes(p.id));

  const toggleSort = () =>
    setFilters({ sort: filters.sort === "-startedAt" ? "startedAt" : "-startedAt" });

  const openRun = (r: MonitorRun) =>
    navigate(`/monitor/runs/${encodeURIComponent(r.globalRunId)}?${params.toString()}`);

  if (projects.isLoading || runs.isLoading) {
    return (
      <div className="space-y-3" data-testid="overview-loading">
        <Skeleton className="h-9 w-full max-w-xl" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  const total = runs.data?.total ?? 0;

  return (
    <div className="space-y-3">
      <StaleBanner projectNames={staleProjects.map((p) => p.displayName)} />
      <FilterBar
        filters={filters}
        projects={projects.data ?? []}
        presets={presets}
        onChange={setFilters}
      />
      {total === 0 && pageRuns.length === 0 ? (
        <div
          className="flex flex-col items-center gap-3 rounded-lg border border-dashed border-border p-10 text-center"
          data-testid="overview-empty"
        >
          <FolderPlus className="h-8 w-8 text-muted-foreground" />
          <p className="font-medium">No runs observed yet</p>
          <p className="max-w-md text-sm text-muted-foreground">
            Register a project directory to start collecting run
            observations — the monitor reads{" "}
            <code className="text-xs">ralph inspect run</code> snapshots
            on a 3s cadence.
          </p>
        </div>
      ) : (
        <Table data-testid="runs-table">
          <TableHeader>
            <TableRow>
              <TableHead>State</TableHead>
              <TableHead>Run</TableHead>
              <TableHead>Project</TableHead>
              <TableHead>Preset</TableHead>
              <TableHead>
                <button
                  type="button"
                  onClick={toggleSort}
                  aria-label="Sort by start time"
                  className="inline-flex items-center gap-1"
                >
                  Started
                  {filters.sort === "-startedAt" ? (
                    <ArrowDown className="h-3 w-3" />
                  ) : (
                    <ArrowUp className="h-3 w-3" />
                  )}
                </button>
              </TableHead>
              <TableHead>Last evidence</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {visibleRuns.map((r) => (
              <TableRow
                key={r.globalRunId}
                tabIndex={0}
                className="cursor-pointer"
                onClick={() => openRun(r)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    openRun(r);
                  }
                }}
              >
                <TableCell>
                  <RunStateBadge state={r.state} stale={r.stale} />
                </TableCell>
                <TableCell className="font-mono text-xs">
                  {r.runId.slice(0, 24)}
                </TableCell>
                <TableCell>{projectName.get(r.projectId) ?? r.projectId}</TableCell>
                <TableCell className="text-muted-foreground">
                  {r.preset ?? "—"}
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {fmt(r.startedAt)}
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {fmt(r.lastEventAt ?? r.terminal?.at ?? r.lastObservedAt)}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      <div className="flex items-center justify-between">
        <span className="text-xs text-muted-foreground">
          {total} run{total === 1 ? "" : "s"}
        </span>
        <Pagination
          page={filters.page}
          total={total}
          pageSize={PAGE_SIZE}
          onChange={(page) => setFilters({ page })}
        />
      </div>
    </div>
  );
}


