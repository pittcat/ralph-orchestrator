/**
 * Plan2-U4 tabbed run detail: header (identity + live progress),
 * descriptor-driven stage strip, and 概览/事件时间线/日志 tabs.
 * URL carries `tab` + selected event `seq` (deep-linkable); the
 * overview's filter params pass through untouched on the way back.
 */

import { useCallback, useMemo, useState } from "react";
import { Link, useLocation, useParams, useSearchParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import {
  useMonitorRunDetail,
  useMonitorRunHealth,
  useRunEvents,
  useRunGraph,
  useRunProgress,
} from "./hooks";
import { DetailHeader } from "./detail/DetailHeader";
import { HealthPanel } from "./health/HealthPanel";
import { StageStrip } from "./detail/StageStrip";
import { OverviewTab } from "./detail/OverviewTab";
import { TimelineTab } from "./detail/TimelineTab";
import { LogsTab } from "./detail/LogsTab";
import { GraphTab } from "./detail/GraphTab";
import { ArtifactsTab } from "./detail/ArtifactsTab";

type TabId = "overview" | "timeline" | "logs" | "graph" | "artifacts";
const TABS: { id: TabId; label: string }[] = [
  { id: "overview", label: "概览" },
  { id: "timeline", label: "事件时间线" },
  { id: "logs", label: "日志" },
  { id: "graph", label: "图" },
  { id: "artifacts", label: "产物" },
];

/// Overview filter keys that ride in on the inbound URL — everything
/// else (tab/seq) belongs to the detail page itself.
const OVERVIEW_KEYS = new Set(["q", "project", "state", "preset", "sort"]);

function backSearch(search: string): string {
  const p = new URLSearchParams(search);
  for (const k of [...p.keys()]) {
    if (!OVERVIEW_KEYS.has(k)) p.delete(k);
  }
  const s = p.toString();
  return s ? `?${s}` : "";
}

export function RunDetailPage() {
  const { globalId } = useParams<{ globalId: string }>();
  const location = useLocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const decoded = globalId ? decodeURIComponent(globalId) : undefined;

  const detail = useMonitorRunDetail(decoded);
  const terminal = !!detail.data?.run.terminal;
  const progress = useRunProgress(decoded, terminal);
  const health = useMonitorRunHealth(decoded);

  const tab = (searchParams.get("tab") as TabId) || "overview";
  const seqParam = searchParams.get("seq");
  const selectedSeq = seqParam ? Number(seqParam) : null;
  const unitFilter = searchParams.get("unit");
  const artifactParam = searchParams.get("artifact");
  const [logsPaused, setLogsPaused] = useState(false);
  const events = useRunEvents(decoded, {
    paused: tab === "logs" ? logsPaused : false,
    filter: unitFilter ? { q: unitFilter } : undefined,
  });
  const graph = useRunGraph(decoded, true);

  const setTab = useCallback(
    (t: TabId) => {
      const p = new URLSearchParams(searchParams);
      if (t === "overview") p.delete("tab");
      else p.set("tab", t);
      setSearchParams(p, { replace: true });
    },
    [searchParams, setSearchParams],
  );
  const selectSeq = useCallback(
    (seq: number) => {
      const p = new URLSearchParams(searchParams);
      p.set("seq", String(seq));
      setSearchParams(p, { replace: true });
    },
    [searchParams, setSearchParams],
  );

  const backTo = `/monitor${backSearch(location.search)}`;

  const stages = useMemo(() => {
    const d = detail.data?.run.presetDescriptor;
    const list = d?.stages;
    if (!Array.isArray(list)) return [];
    return list
      .map((s) => (typeof s === "object" && s !== null ? (s as Record<string, unknown>) : null))
      .filter((s): s is Record<string, unknown> => !!s)
      .map((s) => ({
        id: String(s.id ?? ""),
        label: String(s.label ?? s.id ?? ""),
      }))
      .filter((s) => s.id);
  }, [detail.data]);

  const artifactSlotIds = useMemo(() => {
    const d = detail.data?.run.presetDescriptor;
    const list = d?.artifacts;
    if (!Array.isArray(list)) return new Set<string>();
    return new Set(
      list
        .map((a) => (typeof a === "object" && a !== null ? (a as Record<string, unknown>).id : null))
        .filter((id): id is string => typeof id === "string"),
    );
  }, [detail.data]);

  // S9: timeline entry → 产物 tab anchored at the artifact id.
  const jumpToArtifact = useCallback(
    (id: string) => {
      const p = new URLSearchParams(searchParams);
      p.set("tab", "artifacts");
      p.set("artifact", id);
      setSearchParams(p, { replace: true });
    },
    [searchParams, setSearchParams],
  );

  // S5: graph node/list click → timeline filtered to that unit's events.
  const filterByUnit = useCallback(
    (unit: string) => {
      const p = new URLSearchParams(searchParams);
      p.set("tab", "timeline");
      p.set("unit", unit);
      setSearchParams(p, { replace: true });
    },
    [searchParams, setSearchParams],
  );

  const selectArtifact = useCallback(
    (id: string | null) => {
      const p = new URLSearchParams(searchParams);
      if (id) p.set("artifact", id);
      else p.delete("artifact");
      setSearchParams(p, { replace: true });
    },
    [searchParams, setSearchParams],
  );

  if (detail.isLoading) {
    return (
      <div className="space-y-3" data-testid="detail-loading">
        <Skeleton className="h-6 w-64" />
        <Skeleton className="h-40 w-full" />
      </div>
    );
  }
  if (detail.isError || !detail.data) {
    return (
      <div className="space-y-3">
        <Link
          to={backTo}
          className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="h-4 w-4" /> Back to overview
        </Link>
        <p role="alert" className="text-sm text-status-failed">
          Run not found or monitor unreachable.
        </p>
      </div>
    );
  }

  const run = detail.data.run;
  const hasDescriptor = stages.length > 0;

  return (
    <div className="space-y-4" data-testid="run-detail">
      <Link
        to={backTo}
        className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" /> Back to overview
      </Link>

      <DetailHeader run={run} progress={progress.data} />
      {health.data && <HealthPanel health={health.data} />}

      {!hasDescriptor && (
        <p
          className="rounded-md border border-dashed border-border bg-muted/30 px-3 py-2 text-xs text-muted-foreground"
          data-testid="no-descriptor-banner"
        >
          此 run 无 monitor descriptor —— 展示通用视图（无阶段条）。
        </p>
      )}
      <StageStrip stages={stages} progress={progress.data} />

      <div role="tablist" aria-label="Run detail tabs" className="flex gap-1 border-b border-border">
        {TABS.filter(
          // 图 tab 仅在 dag 数据存在时出现（无 nodes/scheduler → 隐藏）。
          (t) =>
            t.id !== "graph" ||
            (graph.data !== undefined &&
              (graph.data.nodes.length > 0 || graph.data.source === "scheduler")),
        ).map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            data-testid={`tab-${t.id}`}
            className={cn(
              "-mb-px border-b-2 px-3 py-1.5 text-sm",
              tab === t.id
                ? "border-primary text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground",
            )}
            onClick={() => setTab(t.id)}
            onKeyDown={(e) => {
              if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
                const i = TABS.findIndex((x) => x.id === tab);
                const next =
                  TABS[(i + (e.key === "ArrowRight" ? 1 : TABS.length - 1)) % TABS.length];
                setTab(next.id);
                (
                  document.querySelector(
                    `[data-testid="tab-${next.id}"]`,
                  ) as HTMLElement | null
                )?.focus();
              }
            }}
          >
            {t.label}
          </button>
        ))}
      </div>

      <div role="tabpanel">
        {tab === "overview" && <OverviewTab detail={detail.data} />}
        {tab === "timeline" && (
          <TimelineTab
            events={events.events}
            isLoading={events.isLoading}
            selectedSeq={selectedSeq}
            onSelectSeq={selectSeq}
            artifactSlotIds={artifactSlotIds}
            onArtifactJump={jumpToArtifact}
          />
        )}
        {tab === "logs" && (
          <LogsTab
            events={events.events}
            isLoading={events.isLoading}
            paused={logsPaused}
            onPausedChange={setLogsPaused}
            onLoadEarlier={() => void events.loadEarlier()}
            canLoadEarlier={events.events.length > 0}
          />
        )}
        {tab === "graph" && (
          <GraphTab globalRunId={decoded!} onUnitSelect={filterByUnit} />
        )}
        {tab === "artifacts" && (
          <ArtifactsTab
            globalRunId={decoded!}
            selectedId={artifactParam}
            onSelect={selectArtifact}
          />
        )}
      </div>
    </div>
  );
}
