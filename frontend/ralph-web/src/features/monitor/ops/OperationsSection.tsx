/**
 * Plan5-U3 OperationsSection: 一个 run 的操作历史表 —
 * kind/状态/时间/receipt 摘要；confirm_timeout 行给出 recheck。
 */

import type { MonitorOperation } from "../api";
import { useOperations, useRecheckOperation } from "../hooks";

const STATE_LABEL: Record<string, string> = {
  recorded: "已记录",
  dispatching: "分发中",
  dispatched: "已分发",
  confirmed: "已确认",
  failed: "失败",
  confirm_timeout: "确认超时",
};

function receiptSummary(op: MonitorOperation): string {
  if (op.error) return op.error;
  const r = op.receipt;
  if (!r) return "—";
  if (typeof r.action === "string") {
    const detail = typeof r.detail === "string" ? ` — ${r.detail}` : "";
    return `${r.action}${r.ok === false ? " failed" : ""}${detail}`;
  }
  return r.ok === true ? "ok" : "—";
}

export function OperationsSection({ runGid }: { runGid: string }) {
  const ops = useOperations({ run_gid: runGid });
  const recheck = useRecheckOperation();

  if (ops.isLoading) return null;
  const rows = ops.data ?? [];
  if (rows.length === 0) {
    return (
      <p className="text-sm text-muted-foreground" data-testid="operations-empty">
        暂无操作记录。
      </p>
    );
  }

  return (
    <table className="w-full text-sm" data-testid="operations-table">
      <thead>
        <tr className="text-left text-xs text-muted-foreground">
          <th className="py-1 pr-3 font-medium">kind</th>
          <th className="py-1 pr-3 font-medium">状态</th>
          <th className="py-1 pr-3 font-medium">时间</th>
          <th className="py-1 pr-3 font-medium">receipt</th>
          <th className="py-1 font-medium"></th>
        </tr>
      </thead>
      <tbody>
        {rows.map((op) => (
          <tr key={op.id} className="border-t border-border/50" data-testid={`op-${op.id}`}>
            <td className="py-1.5 pr-3 font-mono text-xs">{op.kind}</td>
            <td className="py-1.5 pr-3" data-testid={`op-state-${op.id}`}>
              <span
                className={
                  op.state === "failed" || op.state === "confirm_timeout"
                    ? "text-red-600"
                    : op.state === "confirmed"
                      ? "text-emerald-600"
                      : "text-amber-600"
                }
              >
                {STATE_LABEL[op.state] ?? op.state}
              </span>
            </td>
            <td className="py-1.5 pr-3 font-mono text-xs text-muted-foreground">
              {new Date(op.createdAt).toLocaleString()}
            </td>
            <td className="max-w-64 truncate py-1.5 pr-3 text-xs text-muted-foreground">
              {receiptSummary(op)}
            </td>
            <td className="py-1.5 text-right">
              {op.state === "confirm_timeout" && (
                <button
                  type="button"
                  onClick={() => recheck.mutate(op.id)}
                  disabled={recheck.isPending}
                  data-testid={`op-recheck-${op.id}`}
                  className="rounded border border-border px-2 py-0.5 text-xs hover:bg-accent disabled:opacity-50"
                >
                  recheck
                </button>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
