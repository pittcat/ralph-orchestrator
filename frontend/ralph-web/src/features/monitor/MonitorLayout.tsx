/**
 * Monitor page shell — topbar (title, ⌘K hint, theme toggle) +
 * content slot. Children fill the page body (U4 renders the empty
 * overview state; U5 replaces it with the runs table).
 */

import type { ReactNode } from "react";
import { Moon, Sun, Inbox, Activity } from "lucide-react";
import { Link } from "react-router-dom";
import { useThemeStore } from "./theme";
import { CommandPalette } from "./CommandPalette";
import { useMonitorConnection } from "./connection/ConnectionProvider";
import { useUnreadCount } from "./hooks";

const STATUS_LABEL = {
  connecting: "connecting",
  connected: "connected",
  reconnecting: "reconnecting",
  offline: "offline",
} as const;

const STATUS_CLASS = {
  connecting: "border-amber-500/40 text-amber-600",
  connected: "border-emerald-500/40 text-emerald-600",
  reconnecting: "border-amber-500/40 text-amber-600",
  offline: "border-red-500/40 text-red-600",
} as const;

export function ConnectionBadge() {
  const conn = useMonitorConnection();
  return (
    <span
      data-testid="connection-badge"
      data-status={conn.status}
      title={
        conn.lastValidAt ? `last valid ${conn.lastValidAt}` : conn.status
      }
      className={`inline-flex items-center rounded-full border px-2 py-0.5 text-xs font-medium ${STATUS_CLASS[conn.status]}`}
    >
      {STATUS_LABEL[conn.status]}
    </span>
  );
}

/** Inbox entry with unread badge (Plan4-U3). */
export function InboxLink() {
  const unread = useUnreadCount();
  return (
    <Link
      to="/monitor/inbox"
      data-testid="inbox-link"
      className="relative inline-flex items-center gap-1 rounded-full border border-border px-2 py-0.5 text-xs font-medium text-muted-foreground hover:text-foreground"
    >
      <Inbox className="h-3.5 w-3.5" />
      Inbox
      {unread > 0 && (
        <span
          data-testid="unread-badge"
          className="absolute -right-1 -top-1 flex h-4 min-w-4 items-center justify-center rounded-full bg-red-500 px-1 text-[10px] font-bold text-white"
        >
          {unread > 99 ? "99+" : unread}
        </span>
      )}
    </Link>
  );
}

export function MonitorLayout({ children }: { children: ReactNode }) {
  const theme = useThemeStore((s) => s.theme);
  const toggleTheme = useThemeStore((s) => s.toggleTheme);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-3 border-b border-border pb-3">
        <h1 className="text-xl font-semibold tracking-tight">Monitor</h1>
        <div className="ml-auto flex items-center gap-2">
          <Link
            to="/monitor/operations"
            data-testid="operations-link"
            className="inline-flex items-center gap-1 rounded-full border border-border px-2 py-0.5 text-xs font-medium text-muted-foreground hover:text-foreground"
          >
            <Activity className="h-3.5 w-3.5" />
            操作
          </Link>
          <InboxLink />
          <ConnectionBadge />
          <kbd className="rounded border border-border bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
            ⌘K
          </kbd>
          <button
            type="button"
            onClick={toggleTheme}
            aria-label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
            className="rounded-md p-2 text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground focus:outline-none focus:ring-2 focus:ring-ring"
          >
            {theme === "dark" ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
          </button>
        </div>
      </header>
      <div className="flex-1 overflow-auto pt-4">{children}</div>
      <CommandPalette />
    </div>
  );
}
