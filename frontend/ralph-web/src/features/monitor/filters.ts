/**
 * URL filter codec for the monitor overview — `?q&project&state&preset&sort&page`.
 * `state` is a comma-separated multi-select matching the API's `state`
 * param; `preset` is a client-side facet (plan U5 §19: local facet
 * over server facet endpoint — data volume is small).
 */

import type { RunsQuery } from "./api";

export const PAGE_SIZE = 50;

export interface OverviewFilters {
  q: string;
  project: string;
  states: string[];
  preset: string;
  sort: "startedAt" | "-startedAt";
  page: number;
}

export function decodeFilters(params: URLSearchParams): OverviewFilters {
  const state = params.get("state");
  const sort = params.get("sort");
  const page = Number(params.get("page") ?? "1");
  return {
    q: params.get("q") ?? "",
    project: params.get("project") ?? "",
    states: state ? state.split(",").filter(Boolean) : [],
    preset: params.get("preset") ?? "",
    sort: sort === "startedAt" ? "startedAt" : "-startedAt",
    page: Number.isInteger(page) && page > 0 ? page : 1,
  };
}

export function encodeFilters(f: OverviewFilters): URLSearchParams {
  const params = new URLSearchParams();
  if (f.q) params.set("q", f.q);
  if (f.project) params.set("project", f.project);
  if (f.states.length) params.set("state", f.states.join(","));
  if (f.preset) params.set("preset", f.preset);
  if (f.sort !== "-startedAt") params.set("sort", f.sort);
  if (f.page > 1) params.set("page", String(f.page));
  return params;
}

export function toRunsQuery(f: OverviewFilters): RunsQuery {
  return {
    q: f.q || undefined,
    project: f.project || undefined,
    state: f.states.length ? f.states.join(",") : undefined,
    sort: f.sort,
    limit: PAGE_SIZE,
    offset: (f.page - 1) * PAGE_SIZE,
  };
}
