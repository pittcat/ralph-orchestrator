/**
 * Plan5-U3 `/monitor/operations`：跨项目操作审计列表 —
 * kind/项目/run/状态/时间/actor；project 过滤 + run 深链。
 */

import { useState } from "react";
import { Link } from "react-router-dom";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { useMonitorProjects, useOperations, useRecheckOperation } from "../hooks";

export function OperationsPage() {
  const [projectId, setProjectId] = useState("");
  const projects = useMonitorProjects();
  const ops = useOperations(projectId ? { project_id: projectId } : {});
  const recheck = useRecheckOperation();

  return (
    <div className="space-y-3" data-testid="operations-page">
      <div className="flex items-center gap-3">
        <h2 className="text-lg font-semibold">操作审计</h2>
        <select
          value={projectId}
          onChange={(e) => setProjectId(e.target.value)}
          data-testid="ops-project-filter"
          className="rounded border border-border bg-background px-2 py-1 text-sm"
        >
          <option value="">全部项目</option>
          {(projects.data ?? []).map((p) => (
            <option key={p.id} value={p.id}>
              {p.displayName}
            </option>
          ))}
        </select>
      </div>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>kind</TableHead>
            <TableHead>run</TableHead>
            <TableHead>状态</TableHead>
            <TableHead>创建时间</TableHead>
            <TableHead>actor</TableHead>
            <TableHead>receipt</TableHead>
            <TableHead></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {(ops.data ?? []).map((op) => (
            <TableRow key={op.id} data-testid={`ops-row-${op.id}`}>
              <TableCell className="font-mono text-xs">{op.kind}</TableCell>
              <TableCell className="font-mono text-xs">
                {op.runGid ? (
                  <Link
                    to={`/monitor/runs/${encodeURIComponent(op.runGid)}`}
                    className="text-primary underline"
                  >
                    {op.runGid.split(":").slice(2).join(":")}
                  </Link>
                ) : (
                  "—"
                )}
              </TableCell>
              <TableCell data-testid={`ops-state-${op.id}`}>{op.state}</TableCell>
              <TableCell className="text-xs text-muted-foreground">
                {new Date(op.createdAt).toLocaleString()}
              </TableCell>
              <TableCell className="max-w-32 truncate font-mono text-xs text-muted-foreground">
                {op.actor ? `${op.actor.slice(0, 8)}…` : "—"}
              </TableCell>
              <TableCell className="max-w-48 truncate text-xs text-muted-foreground">
                {op.error ?? (op.receipt ? "ok" : "—")}
              </TableCell>
              <TableCell>
                {op.state === "confirm_timeout" && (
                  <button
                    type="button"
                    onClick={() => recheck.mutate(op.id)}
                    data-testid={`ops-recheck-${op.id}`}
                    className="rounded border border-border px-2 py-0.5 text-xs hover:bg-accent"
                  >
                    recheck
                  </button>
                )}
              </TableCell>
            </TableRow>
          ))}
          {ops.data?.length === 0 && (
            <TableRow>
              <TableCell
                colSpan={7}
                className="py-6 text-center text-muted-foreground"
                data-testid="ops-empty"
              >
                暂无操作。
              </TableCell>
            </TableRow>
          )}
        </TableBody>
      </Table>
    </div>
  );
}
