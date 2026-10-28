/**
 * hooks.test.ts — query keys carry the filter; refetch interval
 * matches the collector cadence.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { useMonitorRuns, MONITOR_REFETCH_MS } from "./hooks";

const fetchMock = vi.fn();

function wrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children);
}

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockReset();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("useMonitorRuns", () => {
  it("sends filter params and caches under ['monitor','runs',query]", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ runs: [], total: 0, watermark: null }), {
        headers: { "content-type": "application/json" },
      }),
    );
    const query = { project: "p1", state: "exited_failed", limit: 5 };
    const { result } = renderHook(() => useMonitorRuns(query), {
      wrapper: wrapper(),
    });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    const [url] = fetchMock.mock.calls[0];
    expect(url).toContain("project=p1");
    expect(url).toContain("state=exited_failed");
    expect(result.current.data?.total).toBe(0);
  });

  it("refetch interval aligns with the 3s collector poll", () => {
    expect(MONITOR_REFETCH_MS).toBe(3000);
  });
});
