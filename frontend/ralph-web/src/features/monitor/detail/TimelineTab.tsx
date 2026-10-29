/**
 * Timeline tab (Plan2-U4 S4/S9): seq-ordered event rows, keyboard
 * navigation (↑/↓ move, Enter selects — the selection lands in the
 * URL `seq` param so a refresh/deep-link restores it). Events that
 * name a declared artifact slot link-jump to the artifacts tab.
 */

import { useCallback, useEffect, useRef } from "react";
import { cn } from "@/lib/utils";
import type { MonitorEvent } from "../api";

export function EventRow({
  event,
  selected,
  onSelect,
  artifactSlotIds,
  onArtifactJump,
}: {
  event: MonitorEvent;
  selected: boolean;
  onSelect: () => void;
  artifactSlotIds: Set<string>;
  onArtifactJump: (id: string) => void;
}) {
  // The event payload may name an artifact slot id (e.g. `summary`)
  // — offer a jump link rather than parsing paths out of free text.
  const linkedArtifact = event.payload
    ? [...artifactSlotIds].find((id) => event.payload!.includes(id))
    : undefined;

  return (
    <li
      tabIndex={0}
      role="row"
      data-testid={`event-row-${event.seq}`}
      data-selected={selected || undefined}
      className={cn(
        "grid cursor-pointer grid-cols-[4rem_11rem_9rem_1fr] items-baseline gap-2 border-b border-border/50 px-2 py-1 text-xs outline-none",
        selected ? "bg-primary/10" : "hover:bg-muted/40",
        "focus-visible:ring-1 focus-visible:ring-ring",
      )}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter") onSelect();
      }}
    >
      <span className="font-mono text-muted-foreground">#{event.seq}</span>
      <span className="text-muted-foreground">{event.ts ?? "—"}</span>
      <span className="truncate font-mono">{event.hat ?? "—"}</span>
      <span className="flex items-center gap-2 truncate">
        <span className="truncate font-mono">{event.topic ?? "(no topic)"}</span>
        {event.truncated && (
          <span className="rounded bg-muted px-1 text-[10px]" title="payload truncated at source">
            truncated
          </span>
        )}
        {linkedArtifact && (
          <button
            type="button"
            className="rounded border border-primary/40 px-1 text-[10px] text-primary hover:bg-primary/10"
            onClick={(e) => {
              e.stopPropagation();
              onArtifactJump(linkedArtifact);
            }}
          >
            {linkedArtifact}
          </button>
        )}
      </span>
    </li>
  );
}

export function TimelineTab({
  events,
  isLoading,
  selectedSeq,
  onSelectSeq,
  artifactSlotIds,
  onArtifactJump,
}: {
  events: MonitorEvent[];
  isLoading: boolean;
  selectedSeq: number | null;
  onSelectSeq: (seq: number) => void;
  artifactSlotIds: Set<string>;
  onArtifactJump: (id: string) => void;
}) {
  const listRef = useRef<HTMLUListElement>(null);

  const move = useCallback(
    (delta: number) => {
      const kids = listRef.current?.children;
      if (!kids?.length) return;
      const cur = Array.prototype.indexOf.call(kids, document.activeElement);
      const next = Math.max(
        0,
        Math.min(kids.length - 1, (cur === -1 ? 0 : cur) + delta),
      );
      (kids[next] as HTMLElement | undefined)?.focus();
    },
    [],
  );

  // Deep-linked seq → focus that row once it exists.
  useEffect(() => {
    if (selectedSeq === null) return;
    const idx = events.findIndex((e) => e.seq === selectedSeq);
    const el = listRef.current?.children[idx] as HTMLElement | undefined;
    if (idx >= 0 && el && document.activeElement !== el) el.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedSeq]);

  if (isLoading && events.length === 0) {
    return <p className="text-sm text-muted-foreground">Loading events…</p>;
  }
  if (events.length === 0) {
    return (
      <p className="text-sm text-muted-foreground" data-testid="timeline-empty">
        No events ingested yet — the collector appends as the run emits.
      </p>
    );
  }
  return (
    <ul
      ref={listRef}
      role="table"
      aria-label="Event timeline"
      className="max-h-[32rem] overflow-auto rounded-md border border-border"
      onKeyDown={(e) => {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          move(1);
        } else if (e.key === "ArrowUp") {
          e.preventDefault();
          move(-1);
        }
      }}
    >
      {events.map((ev) => (
        <EventRow
          key={ev.seq}
          event={ev}
          selected={ev.seq === selectedSeq}
          onSelect={() => onSelectSeq(ev.seq)}
          artifactSlotIds={artifactSlotIds}
          onArtifactJump={onArtifactJump}
        />
      ))}
    </ul>
  );
}
