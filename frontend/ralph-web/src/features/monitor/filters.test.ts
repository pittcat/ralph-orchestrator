/**
 * filters.test.ts — URL codec round-trip + API query mapping.
 */

import { describe, it, expect } from "vitest";
import { decodeFilters, encodeFilters, toRunsQuery, PAGE_SIZE } from "./filters";

describe("overview filter codec", () => {
  it("decodes defaults for an empty query", () => {
    const f = decodeFilters(new URLSearchParams());
    expect(f).toEqual({
      q: "",
      project: "",
      states: [],
      preset: "",
      sort: "-startedAt",
      page: 1,
    });
  });

  it("round-trips state csv, sort and page", () => {
    const f = {
      q: "fix",
      project: "p1",
      states: ["exited_failed", "exited_unknown"],
      preset: "ce",
      sort: "startedAt" as const,
      page: 3,
    };
    const decoded = decodeFilters(encodeFilters(f));
    expect(decoded).toEqual(f);
  });

  it("drops defaults from the URL", () => {
    const p = encodeFilters(decodeFilters(new URLSearchParams()));
    expect(p.toString()).toBe("");
  });

  it("maps to the REST query with offset pagination", () => {
    const q = toRunsQuery({
      q: "x",
      project: "p",
      states: ["exited_failed"],
      preset: "",
      sort: "startedAt",
      page: 2,
    });
    expect(q).toEqual({
      q: "x",
      project: "p",
      state: "exited_failed",
      sort: "startedAt",
      limit: PAGE_SIZE,
      offset: PAGE_SIZE,
    });
  });
});
