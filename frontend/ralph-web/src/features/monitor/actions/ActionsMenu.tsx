/**
 * Plan5-U3 ActionsMenu: 按 runtime 声明的 actions[] 逐条渲染 —
 * 禁用项携带 reason（title + aria-describedby），点击打开
 * ConfirmDialog。
 */

import { useState } from "react";
import type { ActionDecl, MonitorRun } from "../api";
import { ConfirmDialog } from "./ConfirmDialog";

const KIND_LABEL: Record<string, string> = {
  stop: "停止",
  retry: "重试",
  merge: "合并",
};

// emit (发送事件) is deliberately not rendered — the backend op stays
// available for API/CLI callers, but the raw topic-injection affordance
// is too low-level for the dashboard.
const HIDDEN_ACTIONS = new Set(["emit"]);

export function ActionsMenu({ run }: { run: MonitorRun }) {
  const [active, setActive] = useState<ActionDecl | null>(null);
  const actions = run.actions.filter((a) => !HIDDEN_ACTIONS.has(a.id));
  if (actions.length === 0) return null;

  return (
    <div className="flex flex-wrap items-center gap-2" data-testid="actions-menu">
      {actions.map((a) => (
        <button
          key={a.id}
          type="button"
          disabled={!a.available}
          title={a.available ? undefined : (a.reason ?? "当前不可用")}
          aria-describedby={a.available ? undefined : `action-reason-${a.id}`}
          onClick={() => setActive(a)}
          data-testid={`action-${a.id}`}
          className="rounded-md border border-border bg-background/60 px-2.5 py-1 text-xs font-medium transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-45"
        >
          {KIND_LABEL[a.id] ?? a.id}
        </button>
      ))}
      {actions
        .filter((a) => !a.available && a.reason)
        .map((a) => (
          <span key={`reason-${a.id}`} id={`action-reason-${a.id}`} className="sr-only">
            {a.reason}
          </span>
        ))}
      {active && (
        <ConfirmDialog
          run={run}
          action={active}
          onClose={() => setActive(null)}
        />
      )}
    </div>
  );
}
