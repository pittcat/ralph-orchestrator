/**
 * Logs tab (Plan2-U4 S4): flat seq-ordered log lines rendered from
 * event payloads, with pause-follow / in-page search / load-earlier.
 * Simple fixed-height windowing (no virtualizer dep per plan D9):
 * only the viewport ± overscan rows mount.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { Pause, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { MonitorEvent } from "../api";

const ROW_H = 22;
const OVERSCAN = 20;

export function LogList({
  events,
  follow,
}: {
  events: MonitorEvent[];
  follow: boolean;
}) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(480);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    setViewH(el.clientHeight);
    const ro = new ResizeObserver(() => setViewH(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Follow: stick to bottom as new events append.
  useEffect(() => {
    if (follow && wrapRef.current) {
      wrapRef.current.scrollTop = wrapRef.current.scrollHeight;
    }
  }, [events.length, follow]);

  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(
    events.length,
    Math.ceil((scrollTop + viewH) / ROW_H) + OVERSCAN,
  );
  const slice = events.slice(start, end);

  return (
    <div
      ref={wrapRef}
      data-testid="log-list"
      className="h-[32rem] overflow-auto rounded-md border border-border bg-muted/20 font-mono text-xs"
      onScroll={(e) => setScrollTop((e.target as HTMLElement).scrollTop)}
    >
      <div style={{ height: events.length * ROW_H, position: "relative" }}>
        {slice.map((ev, i) => (
          <div
            key={ev.seq}
            className="absolute left-0 flex w-full gap-2 px-2"
            style={{ top: (start + i) * ROW_H, height: ROW_H }}
          >
            <span className="text-muted-foreground/60">#{ev.seq}</span>
            <span className="w-44 shrink-0 truncate text-muted-foreground">
              {ev.ts ?? ""}
            </span>
            <span className="w-28 shrink-0 truncate text-muted-foreground">
              {ev.hat ?? ""}
            </span>
            <span className="truncate">{ev.topic ?? ""}</span>
            <span className="flex-1 truncate text-muted-foreground">
              {ev.payload ?? ""}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

export function LogsTab({
  events,
  isLoading,
  paused,
  onPausedChange,
  onLoadEarlier,
  canLoadEarlier,
}: {
  events: MonitorEvent[];
  isLoading: boolean;
  paused: boolean;
  onPausedChange: (p: boolean) => void;
  onLoadEarlier: () => void;
  canLoadEarlier: boolean;
}) {
  const [search, setSearch] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // ctrl+f focuses the in-page search (doesn't override browser).
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "f") {
        e.preventDefault();
        inputRef.current?.focus();
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, []);

  const filtered = useMemo(() => {
    if (!search) return events;
    const q = search.toLowerCase();
    return events.filter(
      (e) =>
        e.topic?.toLowerCase().includes(q) ||
        e.payload?.toLowerCase().includes(q) ||
        e.hat?.toLowerCase().includes(q),
    );
  }, [events, search]);

  return (
    <div className="space-y-2" data-testid="logs-tab">
      <div className="flex items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          onClick={() => onPausedChange(!paused)}
          data-testid="pause-follow"
        >
          {paused ? <Play className="mr-1 h-3 w-3" /> : <Pause className="mr-1 h-3 w-3" />}
          {paused ? "Resume follow" : "Pause follow"}
        </Button>
        <input
          ref={inputRef}
          type="search"
          placeholder="Filter events (ctrl+f)…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className="h-8 w-64 rounded-md border border-input bg-background px-2 text-xs"
          data-testid="log-search"
        />
        {canLoadEarlier && (
          <Button variant="ghost" size="sm" onClick={onLoadEarlier}>
            Load earlier
          </Button>
        )}
        <span className="text-xs text-muted-foreground">
          {filtered.length}/{events.length}
        </span>
      </div>
      {isLoading && events.length === 0 ? (
        <p className="text-sm text-muted-foreground">Loading…</p>
      ) : (
        <LogList events={filtered} follow={!paused && !search} />
      )}
    </div>
  );
}
