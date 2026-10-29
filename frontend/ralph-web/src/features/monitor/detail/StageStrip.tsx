/**
 * Stage strip (Plan2-U4 S1/S10): descriptor-driven horizontal stage
 * list — completed stages dimmed, current highlighted, `×N` round
 * counter for loop descriptors. Hidden entirely when the run has no
 * descriptor (S2 fallback is a banner in the page).
 */

import { Check, ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";
import type { RunProgress } from "../api";

export interface DescriptorStageLite {
  id: string;
  label: string;
}

export function StageStrip({
  stages,
  progress,
}: {
  stages: DescriptorStageLite[];
  progress: RunProgress | undefined;
}) {
  if (stages.length === 0) return null;
  const currentId = progress?.stage ?? null;
  const enteredIdx = new Map<string, number>();
  for (const h of progress?.stage_history ?? []) {
    enteredIdx.set(h.stage, h.round);
  }
  const currentOrder = currentId
    ? stages.findIndex((s) => s.id === currentId)
    : -1;

  return (
    <ol
      className="flex flex-wrap items-center gap-x-1 gap-y-1.5"
      data-testid="stage-strip"
      aria-label="Pipeline stages"
    >
      {stages.map((s, i) => {
        const entered = enteredIdx.has(s.id);
        const isCurrent = s.id === currentId;
        const done = entered && !isCurrent && currentOrder > i;
        const round = enteredIdx.get(s.id) ?? 0;
        return (
          <li key={s.id} className="flex items-center gap-1">
            {i > 0 && (
              <ChevronRight
                className="h-3 w-3 text-muted-foreground/40"
                aria-hidden
              />
            )}
            <span
              data-testid={`stage-node-${s.id}`}
              data-state={isCurrent ? "current" : done || entered ? "done" : "pending"}
              className={cn(
                "inline-flex items-center gap-1 rounded-md border px-2 py-1 text-xs transition-colors",
                isCurrent
                  ? "border-primary/60 bg-primary/10 font-medium text-primary shadow-sm"
                  : done || entered
                    ? "border-emerald-500/30 bg-emerald-500/5 text-muted-foreground"
                    : "border-dashed border-border/60 text-muted-foreground/50",
              )}
            >
              {(done || (entered && !isCurrent)) && (
                <Check className="h-3 w-3 text-emerald-500/80" aria-hidden />
              )}
              {s.label}
              {round > 1 && (
                <span
                  className="rounded bg-muted px-1 text-[10px]"
                  data-testid={`stage-round-${s.id}`}
                >
                  ×{round}
                </span>
              )}
            </span>
          </li>
        );
      })}
    </ol>
  );
}
