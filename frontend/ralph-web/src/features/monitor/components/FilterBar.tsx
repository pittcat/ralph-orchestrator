/**
 * FilterBar — search / project select / state multi-select / preset
 * select. Controlled by the parent's URL state (single source of
 * truth stays in `useSearchParams`).
 */

import { useEffect, useState } from "react";
import { ChevronDown, Search } from "lucide-react";
import { MONITOR_RUN_STATES } from "../api";
import type { MonitorProject } from "../api";
import type { OverviewFilters } from "../filters";

const CONTROL_CLS =
  "h-9 appearance-none rounded-lg border border-border bg-card/50 pl-3 pr-8 text-sm " +
  "outline-none transition-colors hover:border-foreground/25 " +
  "focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/40";

function Chevron() {
  return (
    <ChevronDown className="pointer-events-none absolute right-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
  );
}

export function FilterBar({
  filters,
  projects,
  presets,
  onChange,
}: {
  filters: OverviewFilters;
  projects: MonitorProject[];
  /** locally-faceted preset names (plan U5 §19 local facet decision) */
  presets: string[];
  onChange: (patch: Partial<OverviewFilters>) => void;
}) {
  // Local copy of the search text so fast typing isn't eaten by the
  // URL→state round trip; the URL converges on each change.
  const [q, setQ] = useState(filters.q);
  useEffect(() => setQ(filters.q), [filters.q]);

  const toggleState = (s: string) =>
    onChange({
      states: filters.states.includes(s)
        ? filters.states.filter((x) => x !== s)
        : [...filters.states, s],
    });

  return (
    <div className="flex flex-wrap items-center gap-2" data-testid="filter-bar">
      <div className="relative">
        <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
        <input
          type="search"
          aria-label="Search runs"
          placeholder="Search runs…"
          value={q}
          onChange={(e) => {
            setQ(e.target.value);
            onChange({ q: e.target.value });
          }}
          className="h-9 w-56 rounded-lg border border-border bg-card/50 pl-9 pr-3 text-sm outline-none transition-colors placeholder:text-muted-foreground/70 hover:border-foreground/25 focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/40 [&::-webkit-search-cancel-button]:hidden"
        />
      </div>
      <div className="relative">
        <select
          aria-label="Filter by project"
          value={filters.project}
          onChange={(e) => onChange({ project: e.target.value })}
          className={CONTROL_CLS}
        >
          <option value="">All projects</option>
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.displayName}
            </option>
          ))}
        </select>
        <Chevron />
      </div>
      <details className="relative" data-testid="state-filter">
        <summary className="flex h-9 cursor-pointer list-none items-center gap-1.5 rounded-lg border border-border bg-card/50 pl-3 pr-8 text-sm transition-colors hover:border-foreground/25 [&::-webkit-details-marker]:hidden">
          State{filters.states.length ? ` (${filters.states.length})` : ""}
          <ChevronDown className="pointer-events-none absolute right-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
        </summary>
        <div className="absolute z-10 mt-1 w-48 rounded-lg border border-border bg-popover p-1.5 shadow-lg">
          {MONITOR_RUN_STATES.map((s) => (
            <label
              key={s}
              className="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent"
            >
              <input
                type="checkbox"
                checked={filters.states.includes(s)}
                onChange={() => toggleState(s)}
                className="accent-primary"
              />
              {s}
            </label>
          ))}
        </div>
      </details>
      <div className="relative">
        <select
          aria-label="Filter by preset"
          value={filters.preset}
          onChange={(e) => onChange({ preset: e.target.value })}
          className={CONTROL_CLS}
        >
          <option value="">All presets</option>
          {presets.map((p) => (
            <option key={p} value={p}>
              {p}
            </option>
          ))}
        </select>
        <Chevron />
      </div>
    </div>
  );
}
