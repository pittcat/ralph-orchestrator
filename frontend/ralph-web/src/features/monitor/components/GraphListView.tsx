/**
 * GraphListView (Plan2-U5 S5): same /graph data as an accessible
 * table — the interactive authority (canvas is role=img). Click a
 * unit to filter the timeline.
 */

import type { RunGraph } from "../api";
import { cn } from "@/lib/utils";

const STATUS_TEXT: Record<string, string> = {
  ready: "text-muted-foreground",
  running: "text-primary",
  executed: "text-status-ok",
  verified: "text-status-ok",
  failed: "text-status-failed",
  correction: "text-status-stale",
  abandoned: "text-muted-foreground line-through",
};

export function GraphListView({
  graph,
  onUnitClick,
}: {
  graph: RunGraph;
  onUnitClick?: (unit: string) => void;
}) {
  if (graph.nodes.length === 0) {
    return (
      <p className="text-sm text-muted-foreground" data-testid="graph-empty">
        暂无 unit 图数据 —— 无 forge.unit.* 事件或 scheduler 摘要。
      </p>
    );
  }
  const out = new Map<string, string[]>();
  for (const e of graph.edges) {
    (out.get(e.from) ?? out.set(e.from, []).get(e.from)!).push(e.to);
  }
  return (
    <table className="w-full text-sm" data-testid="graph-list">
      <thead>
        <tr className="border-b border-border text-left text-xs text-muted-foreground">
          <th className="py-1 pr-4 font-medium">unit</th>
          <th className="py-1 pr-4 font-medium">status</th>
          <th className="py-1 pr-4 font-medium">last_seq</th>
          <th className="py-1 font-medium">→</th>
        </tr>
      </thead>
      <tbody>
        {graph.nodes.map((n) => (
          <tr
            key={n.unit}
            data-testid={`graph-node-${n.unit}`}
            className="border-b border-border/50 hover:bg-muted/30"
          >
            <td className="py-1 pr-4">
              {onUnitClick ? (
                <button
                  className="font-mono text-xs text-primary hover:underline"
                  onClick={() => onUnitClick(n.unit)}
                >
                  {n.unit}
                </button>
              ) : (
                <span className="font-mono text-xs">{n.unit}</span>
              )}
            </td>
            <td className={cn("py-1 pr-4 text-xs", STATUS_TEXT[n.status] ?? "")}>
              {n.status}
            </td>
            <td className="py-1 pr-4 font-mono text-xs text-muted-foreground">
              #{n.last_seq}
            </td>
            <td className="py-1 font-mono text-xs text-muted-foreground">
              {(out.get(n.unit) ?? []).join(", ") || "—"}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
