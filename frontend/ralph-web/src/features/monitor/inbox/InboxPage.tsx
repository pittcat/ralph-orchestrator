/**
 * InboxPage (Plan4-U3): the notifications inbox — filter tabs
 * (全部/未读/已静音/resolved), pending-interactions section, row
 * actions, read-all, and the settings panel (S5 deep-link + auto-read
 * handled in the row).
 */

import { useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  apiMuteNotification,
  apiReadAllNotifications,
  apiReadNotification,
  type NotificationsQuery,
} from "../api";
import {
  useChannelConfig,
  useMonitorProjects,
  useNotifications,
  usePendingInteractions,
} from "../hooks";
import { NotificationRow } from "./NotificationRow";
import { PendingSection } from "./PendingSection";
import { NotificationSettings } from "./NotificationSettings";

const FILTERS = [
  { id: "active", label: "全部", state: "unread,read" },
  { id: "unread", label: "未读", state: "unread" },
  { id: "muted", label: "已静音", state: "muted" },
  { id: "resolved", label: "已解决", state: "resolved" },
] as const;

export function InboxPage() {
  const qc = useQueryClient();
  const [filter, setFilter] = useState<string>("active");
  const [showSettings, setShowSettings] = useState(false);
  const query: NotificationsQuery = useMemo(
    () => ({
      state: FILTERS.find((f) => f.id === filter)?.state,
      limit: 50,
    }),
    [filter],
  );
  const list = useNotifications(query);
  const pending = usePendingInteractions();
  const config = useChannelConfig();
  const projects = useMonitorProjects();

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: ["monitor", "notifications"] });
  };
  const read = useMutation({
    mutationFn: apiReadNotification,
    onSuccess: invalidate,
  });
  const mute = useMutation({
    mutationFn: apiMuteNotification,
    onSuccess: invalidate,
  });
  const readAll = useMutation({
    mutationFn: apiReadAllNotifications,
    onSuccess: invalidate,
  });

  const runtimeHints = (projects.data ?? [])
    .filter((p) => p.runtimeWebhookHint === "enabled")
    .map((p) => p.displayName);

  return (
    <div className="space-y-4" data-testid="inbox-page">
      <div className="flex items-center gap-2">
        <div className="flex gap-1" role="tablist" aria-label="通知过滤">
          {FILTERS.map((f) => (
            <button
              key={f.id}
              role="tab"
              aria-selected={filter === f.id}
              data-testid={`filter-${f.id}`}
              onClick={() => setFilter(f.id)}
              className={`rounded-full border px-3 py-1 text-xs ${
                filter === f.id
                  ? "border-primary text-foreground"
                  : "border-border text-muted-foreground hover:text-foreground"
              }`}
            >
              {f.label}
            </button>
          ))}
        </div>
        <div className="ml-auto flex gap-2">
          <button
            type="button"
            data-testid="read-all"
            onClick={() => readAll.mutate()}
            className="rounded border border-border px-2 py-1 text-xs hover:bg-accent"
          >
            全部已读
          </button>
          <button
            type="button"
            data-testid="toggle-settings"
            onClick={() => setShowSettings((s) => !s)}
            className="rounded border border-border px-2 py-1 text-xs hover:bg-accent"
          >
            设置
          </button>
        </div>
      </div>

      {showSettings && (
        <NotificationSettings
          config={config.data}
          runtimeWebhookHints={runtimeHints}
        />
      )}

      <PendingSection items={pending.data ?? []} />

      {list.isLoading && (
        <p className="text-sm text-muted-foreground">加载中…</p>
      )}
      {list.data && list.data.notifications.length === 0 && (
        <p data-testid="inbox-empty" className="text-sm text-muted-foreground">
          没有通知
        </p>
      )}
      <ul className="space-y-2" data-testid="notif-list">
        {(list.data?.notifications ?? []).map((n) => (
          <NotificationRow
            key={n.id}
            n={n}
            onRead={(id) => read.mutate(id)}
            onMute={(id) => mute.mutate(id)}
          />
        ))}
      </ul>
      {list.data && list.data.total > list.data.notifications.length && (
        <p className="text-xs text-muted-foreground">
          共 {list.data.total} 条（显示前 50）
        </p>
      )}
    </div>
  );
}
