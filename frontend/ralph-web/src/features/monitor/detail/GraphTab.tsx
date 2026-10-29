/**
 * GraphTab (Plan2-U5): lazy-loaded React Flow canvas + accessible
 * list view of the same /graph data. Clicking a node/unit asks the
 * parent to filter the timeline to that unit's events.
 */

import { lazy, Suspense, useState } from "react";
import { Skeleton } from "@/components/ui/skeleton";
import { useRunGraph } from "../hooks";
import { GraphListView } from "../components/GraphListView";
import { cn } from "@/lib/utils";

const LazyRunGraph = lazy(() =>
  import("../components/RunGraph").then((m) => ({ default: m.RunGraph })),
);

export function GraphTab({
  globalRunId,
  onUnitSelect,
}: {
  globalRunId: string;
  onUnitSelect?: (unit: string) => void;
}) {
  const graph = useRunGraph(globalRunId, true);
  const [view, setView] = useState<"graph" | "list">("graph");

  if (graph.isLoading) {
    return <Skeleton className="h-96 w-full" data-testid="graph-loading" />;
  }
  if (graph.isError || !graph.data) {
    return (
      <p className="text-sm text-status-failed" role="alert">
        图数据加载失败。
      </p>
    );
  }
  if (graph.data.nodes.length === 0) {
    return <GraphListView graph={graph.data} onUnitClick={onUnitSelect} />;
  }
  return (
    <div className="space-y-2" data-testid="graph-tab">
      <div className="flex gap-1" role="group" aria-label="graph view mode">
        {(["graph", "list"] as const).map((v) => (
          <button
            key={v}
            data-testid={`graph-view-${v}`}
            onClick={() => setView(v)}
            className={cn(
              "rounded-md border px-2 py-1 text-xs",
              view === v
                ? "border-primary bg-primary/10 text-primary"
                : "border-border text-muted-foreground",
            )}
          >
            {v === "graph" ? "画布" : "列表"}
          </button>
        ))}
      </div>
      {view === "graph" ? (
        <Suspense fallback={<Skeleton className="h-96 w-full" />}>
          <LazyRunGraph graph={graph.data} onNodeClick={onUnitSelect} />
        </Suspense>
      ) : (
        <GraphListView graph={graph.data} onUnitClick={onUnitSelect} />
      )}
      {graph.data.illegal_transitions > 0 && (
        <p className="text-xs text-status-stale">
          {graph.data.illegal_transitions} 条非法状态回退被忽略（保留首个终态）。
        </p>
      )}
    </div>
  );
}
