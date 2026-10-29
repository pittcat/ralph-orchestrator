/**
 * One notification row (Plan4-U3): kind badge + title + time +
 * per-row actions. Enter on a focused row opens the deep link (a11y
 * §13); read/mute are plain buttons so every action is reachable.
 */

import { useNavigate } from "react-router-dom";
import type { MonitorNotification } from "../api";

const KIND_LABEL: Record<string, string> = {
  "run.completed": "完成",
  "run.failed": "失败",
  "interaction.pending": "待处理",
};
const KIND_CLASS: Record<string, string> = {
  "run.completed": "border-emerald-500/40 text-emerald-600",
  "run.failed": "border-red-500/40 text-red-600",
  "interaction.pending": "border-amber-500/40 text-amber-600",
};

export function NotificationRow({
  n,
  onRead,
  onMute,
}: {
  n: MonitorNotification;
  onRead: (id: string) => void;
  onMute: (id: string) => void;
}) {
  const navigate = useNavigate();
  const open = () => {
    if (n.state === "unread") onRead(n.id);
    if (n.link) navigate(n.link);
  };
  return (
    <li
      data-testid={`notif-${n.id}`}
      data-state={n.state}
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter") open();
      }}
      onClick={open}
      className={`flex cursor-pointer items-center gap-3 rounded-md border border-border px-3 py-2 hover:bg-accent/40 focus:outline-none focus:ring-2 focus:ring-ring ${
        n.state === "unread" ? "bg-accent/20" : ""
      }`}
    >
      <span
        className={`inline-flex shrink-0 rounded-full border px-2 py-0.5 text-xs font-medium ${KIND_CLASS[n.kind] ?? "border-border text-muted-foreground"}`}
      >
        {KIND_LABEL[n.kind] ?? n.kind}
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-sm">{n.title}</span>
        <span className="block text-xs text-muted-foreground">
          {n.created_at}
        </span>
      </span>
      {n.state === "unread" && (
        <button
          type="button"
          data-testid={`read-${n.id}`}
          onClick={(e) => {
            e.stopPropagation();
            onRead(n.id);
          }}
          className="rounded border border-border px-2 py-0.5 text-xs hover:bg-accent"
        >
          已读
        </button>
      )}
      {n.state !== "muted" && n.state !== "resolved" && (
        <button
          type="button"
          data-testid={`mute-${n.id}`}
          onClick={(e) => {
            e.stopPropagation();
            onMute(n.id);
          }}
          className="rounded border border-border px-2 py-0.5 text-xs hover:bg-accent"
        >
          静音
        </button>
      )}
    </li>
  );
}
