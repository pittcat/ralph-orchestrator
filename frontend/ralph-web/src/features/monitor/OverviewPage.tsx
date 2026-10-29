/**
 * Monitor overview — stat cards, runs table with search / project /
 * state / preset filters, startedAt sort, and offset pagination. All
 * filter state lives in the URL (`useSearchParams`) so refresh/
 * back-forward restore the view exactly (R4).
 */

import { useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { ArrowDown, ArrowUp, FolderPlus, Play } from "lucide-react";
import { formatDistanceToNow } from "date-fns";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Skeleton } from "@/components/ui/skeleton";
import {
  useMonitorProjects,
  useMonitorProjectsHealth,
  useMonitorRuns,
  useMonitorSnapshot,
} from "./hooks";
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
import { StatCards } from "./components/StatCards";
import { StaleBanner } from "./components/StaleBanner";
import { DeleteRunButton } from "./components/DeleteRunButton";
import { ActionsMenu } from "./actions/ActionsMenu";
import { LaunchDialog } from "./launch/LaunchDialog";

/** Relative time for the table ("3 minutes ago"); exact ts on title. */
function fmt(ts: string | null): string {
  if (!ts) return "—";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  return formatDistanceToNow(d, { addSuffix: true });
}

function abs(ts: string | null): string | undefined {
  if (!ts) return undefined;
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleString();
}

export function OverviewPage() {
  const [params, setParams] = useSearchParams();
  const filters = decodeFilters(params);
  const navigate = useNavigate();
  const [launchOpen, setLaunchOpen] = useState(false);

  const projects = useMonitorProjects();
  const runs = useMonitorRuns(toRunsQuery(filters));
  // Global (unfiltered) counts for the stat cards — the stream seeds
  // this key, so cards stay fresh without an extra poll cadence.
  const snapshot = useMonitorSnapshot();

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
      <div className="space-y-4" data-testid="overview-loading">
        <Skeleton className="h-9 w-full max-w-xl" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  const total = runs.data?.total ?? 0;

  const HEAD_CLS =
    "h-10 px-3 text-xs font-medium uppercase tracking-wider text-muted-foreground/80";
  const CELL_CLS = "px-3 py-3";

  return (
    <div className="space-y-4">
      <StaleBanner projectNames={staleProjects.map((p) => p.displayName)} />
      <StatCards
        runs={snapshot.data?.runs ?? []}
        states={filters.states}
        onChange={setFilters}
      />
      <div className="flex items-end justify-between gap-3">
        <FilterBar
          filters={filters}
          projects={projects.data ?? []}
          presets={presets}
          onChange={setFilters}
        />
        <button
          type="button"
          onClick={() => setLaunchOpen(true)}
          data-testid="launch-button"
          className="inline-flex h-9 shrink-0 items-center gap-1.5 rounded-lg bg-primary px-4 text-sm font-medium text-primary-foreground shadow-sm transition-opacity hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <Play className="h-3.5 w-3.5" />
          启动运行
        </button>
      </div>
      <LaunchDialog open={launchOpen} onOpenChange={setLaunchOpen} />
      {total === 0 && pageRuns.length === 0 ? (
        <div
          className="flex flex-col items-center gap-3 rounded-xl border border-dashed border-border bg-card/30 p-10 text-center"
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
        <div className="overflow-hidden rounded-xl border border-border bg-card/40">
          <Table data-testid="runs-table">
            <TableHeader>
              <TableRow className="bg-muted/30 hover:bg-muted/30">
                <TableHead className={HEAD_CLS}>State</TableHead>
                <TableHead className={HEAD_CLS}>Run</TableHead>
                <TableHead className={HEAD_CLS}>Project</TableHead>
                <TableHead className={HEAD_CLS}>Preset</TableHead>
                <TableHead className={HEAD_CLS}>
                  <button
                    type="button"
                    onClick={toggleSort}
                    aria-label="Sort by start time"
                    className="inline-flex items-center gap-1 uppercase tracking-wider"
                  >
                    Started
                    {filters.sort === "-startedAt" ? (
                      <ArrowDown className="h-3 w-3" />
                    ) : (
                      <ArrowUp className="h-3 w-3" />
                    )}
                  </button>
                </TableHead>
                <TableHead className={HEAD_CLS}>Last evidence</TableHead>
                <TableHead className={`${HEAD_CLS} w-10`} />
              </TableRow>
            </TableHeader>
            <TableBody>
              {visibleRuns.map((r) => (
                <TableRow
                  key={r.globalRunId}
                  tabIndex={0}
                  className="cursor-pointer transition-colors"
                  onClick={() => openRun(r)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      openRun(r);
                    }
                  }}
                >
                  <TableCell className={CELL_CLS}>
                    <RunStateBadge state={r.state} stale={r.stale} />
                  </TableCell>
                  <TableCell className={`${CELL_CLS} font-mono text-xs`}>
                    <span title={r.runId}>{r.runId.slice(0, 24)}</span>
                  </TableCell>
                  <TableCell className={CELL_CLS}>
                    {projectName.get(r.projectId) ?? r.projectId}
                  </TableCell>
                  <TableCell className={`${CELL_CLS} font-mono text-xs text-muted-foreground`}>
                    {r.preset ?? "—"}
                  </TableCell>
                  <TableCell
                    className={`${CELL_CLS} text-muted-foreground`}
                    title={abs(r.startedAt)}
                  >
                    {fmt(r.startedAt)}
                  </TableCell>
                  <TableCell
                    className={`${CELL_CLS} text-muted-foreground`}
                    title={abs(r.lastEventAt ?? r.terminal?.at ?? r.lastObservedAt)}
                  >
                    {fmt(r.lastEventAt ?? r.terminal?.at ?? r.lastObservedAt)}
                  </TableCell>
                  <TableCell className={`${CELL_CLS} text-right`}>
                    <div
                      className="flex items-center justify-end gap-1.5"
                      onClick={(e) => e.stopPropagation()}
                      onKeyDown={(e) => e.stopPropagation()}
                    >
                      <ActionsMenu run={r} />
                      <DeleteRunButton run={r} variant="icon" />
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
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
