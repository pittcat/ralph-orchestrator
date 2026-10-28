/**
 * FilterBar — search / project select / state multi-select / preset
 * select. Controlled by the parent's URL state (single source of
 * truth stays in `useSearchParams`).
 */

import { useEffect, useState } from "react";
import { MONITOR_RUN_STATES } from "../api";
import type { MonitorProject } from "../api";
import type { OverviewFilters } from "../filters";

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
      <input
        type="search"
        aria-label="Search runs"
        placeholder="Search runs…"
        value={q}
        onChange={(e) => {
          setQ(e.target.value);
          onChange({ q: e.target.value });
        }}
        className="h-9 w-56 rounded-md border border-border bg-background px-3 text-sm outline-none placeholder:text-muted-foreground focus:ring-2 focus:ring-ring"
      />
      <select
        aria-label="Filter by project"
        value={filters.project}
        onChange={(e) => onChange({ project: e.target.value })}
        className="h-9 rounded-md border border-border bg-background px-2 text-sm"
      >
        <option value="">All projects</option>
        {projects.map((p) => (
          <option key={p.id} value={p.id}>
            {p.displayName}
          </option>
        ))}
      </select>
      <details className="relative" data-testid="state-filter">
        <summary className="flex h-9 cursor-pointer list-none items-center rounded-md border border-border bg-background px-3 text-sm">
          State{filters.states.length ? ` (${filters.states.length})` : ""}
        </summary>
        <div className="absolute z-10 mt-1 w-48 rounded-md border border-border bg-popover p-2 shadow-md">
          {MONITOR_RUN_STATES.map((s) => (
            <label
              key={s}
              className="flex cursor-pointer items-center gap-2 rounded px-2 py-1 text-sm hover:bg-accent"
            >
              <input
                type="checkbox"
                checked={filters.states.includes(s)}
                onChange={() => toggleState(s)}
              />
              {s}
            </label>
          ))}
        </div>
      </details>
      <select
        aria-label="Filter by preset"
        value={filters.preset}
        onChange={(e) => onChange({ preset: e.target.value })}
        className="h-9 rounded-md border border-border bg-background px-2 text-sm"
      >
        <option value="">All presets</option>
        {presets.map((p) => (
          <option key={p} value={p}>
            {p}
          </option>
        ))}
      </select>
    </div>
  );
}
