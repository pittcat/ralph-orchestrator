/**
 * Monitor page shell — sticky topbar (brand, nav, ⌘K hint, theme
 * toggle) + centered content slot.
 */

import type { ReactNode } from "react";
import { Moon, Sun, Inbox, Activity, Radar } from "lucide-react";
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

const DOT_CLASS = {
  connecting: "bg-amber-500",
  connected: "bg-emerald-500",
  reconnecting: "bg-amber-500",
  offline: "bg-red-500",
} as const;

const NAV_PILL_CLS =
  "inline-flex items-center gap-1.5 rounded-full border border-border bg-card/50 px-3 py-1 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground";

export function ConnectionBadge() {
  const conn = useMonitorConnection();
  return (
    <span
      data-testid="connection-badge"
      data-status={conn.status}
      title={
        conn.lastValidAt ? `last valid ${conn.lastValidAt}` : conn.status
      }
      className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium ${STATUS_CLASS[conn.status]}`}
    >
      <span
        className={`h-1.5 w-1.5 rounded-full ${DOT_CLASS[conn.status]} ${
          conn.status === "connecting" || conn.status === "reconnecting"
            ? "animate-pulse"
            : ""
        }`}
      />
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
      className={`relative ${NAV_PILL_CLS}`}
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
    <div className="flex min-h-dvh flex-col">
      <header className="sticky top-0 z-30 border-b border-border/60 bg-background/80 backdrop-blur-md">
        <div className="mx-auto flex h-14 w-full max-w-[1400px] items-center gap-3 px-4 sm:px-6">
          <div className="flex items-center gap-2.5">
            <span className="flex h-8 w-8 items-center justify-center rounded-lg bg-primary/15 ring-1 ring-primary/30">
              <Radar className="h-4 w-4 text-primary" />
            </span>
            <h1 className="text-lg font-semibold tracking-tight">Monitor</h1>
          </div>
          <div className="ml-auto flex items-center gap-2">
            <Link
              to="/monitor/operations"
              data-testid="operations-link"
              className={NAV_PILL_CLS}
            >
              <Activity className="h-3.5 w-3.5" />
              操作
            </Link>
            <InboxLink />
            <ConnectionBadge />
            <kbd className="hidden rounded-md border border-border bg-muted/60 px-1.5 py-0.5 text-xs text-muted-foreground sm:inline">
              ⌘K
            </kbd>
            <button
              type="button"
              onClick={toggleTheme}
              aria-label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
              className="rounded-lg p-2 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus:outline-none focus:ring-2 focus:ring-ring"
            >
              {theme === "dark" ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
            </button>
          </div>
        </div>
      </header>
      <main className="mx-auto w-full max-w-[1400px] flex-1 px-4 py-6 sm:px-6">
        {children}
      </main>
      <CommandPalette />
    </div>
  );
}
