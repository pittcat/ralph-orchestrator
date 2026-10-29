/**
 * RunGraph (Plan2-U5 S5): @xyflow/react canvas rendering /graph
 * nodes+edges with status coloring and a simple layered layout
 * (columns by topological rank — no dagre dep per plan D9-§13).
 * role=img: the authoritative interactive view is GraphListView.
 */

import { useMemo } from "react";
import { ReactFlow, Background, type Edge, type Node } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { RunGraph as RunGraphData } from "../api";

const STATUS_CLASS: Record<string, string> = {
  ready: "bg-muted text-muted-foreground",
  running: "border-primary bg-primary/10 text-primary",
  executed: "bg-status-ok/15 text-status-ok",
  verified: "bg-status-ok/25 text-status-ok",
  failed: "bg-status-failed/15 text-status-failed",
  correction: "bg-status-stale/15 text-status-stale",
  abandoned: "bg-muted/50 text-muted-foreground line-through",
};

function layeredPositions(data: RunGraphData): Map<string, { x: number; y: number }> {
  // rank(node) = longest path from a source over declared edges.
  const rank = new Map<string, number>();
  const outgoing = new Map<string, string[]>();
  for (const e of data.edges) {
    (outgoing.get(e.from) ?? outgoing.set(e.from, []).get(e.from)!).push(e.to);
  }
  const visit = (u: string, depth: number, seen: Set<string>) => {
    if (seen.has(u)) return;
    seen.add(u);
    rank.set(u, Math.max(rank.get(u) ?? 0, depth));
    for (const v of outgoing.get(u) ?? []) visit(v, depth + 1, seen);
  };
  for (const n of data.nodes) visit(n.unit, 0, new Set());
  const pos = new Map<string, { x: number; y: number }>();
  const perRank = new Map<number, number>();
  for (const n of data.nodes) {
    const r = rank.get(n.unit) ?? 0;
    const row = perRank.get(r) ?? 0;
    perRank.set(r, row + 1);
    pos.set(n.unit, { x: r * 220, y: row * 64 });
  }
  return pos;
}

export function RunGraph({
  graph,
  onNodeClick,
}: {
  graph: RunGraphData;
  onNodeClick?: (unit: string) => void;
}) {
  const { nodes, edges } = useMemo(() => {
    const pos = layeredPositions(graph);
    const nodes: Node[] = graph.nodes.map((n) => ({
      id: n.unit,
      position: pos.get(n.unit) ?? { x: 0, y: 0 },
      data: { label: `${n.unit} · ${n.status}` },
      className: STATUS_CLASS[n.status] ?? STATUS_CLASS.ready,
      draggable: false,
      connectable: false,
    }));
    const edges: Edge[] = graph.edges.map((e, i) => ({
      id: `e${i}`,
      source: e.from,
      target: e.to,
    }));
    return { nodes, edges };
  }, [graph]);

  return (
    <div
      className="h-96 rounded-md border border-border"
      role="img"
      aria-label="Unit DAG"
      data-testid="run-graph"
    >
      <ReactFlow
        nodes={nodes}
        edges={edges}
        fitView
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={!!onNodeClick}
        onNodeClick={(_, n) => onNodeClick?.(n.id)}
        proOptions={{ hideAttribution: true }}
      >
        <Background gap={16} />
      </ReactFlow>
    </div>
  );
}
