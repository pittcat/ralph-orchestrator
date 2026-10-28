/**
 * OverviewPage.test.tsx — table render, URL-driven filters, sort
 * toggle, stale badge, empty state, pagination.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, useLocation } from "react-router-dom";
import { createElement, type ReactNode } from "react";
import { OverviewPage } from "./OverviewPage";

const fetchMock = vi.fn();

const RUN_A = {
  globalRunId: "m:p1:loop-1",
  projectId: "p1",
  runId: "loop-1",
  kind: "worktree",
  state: "exited_failed",
  stateEvidence: { reason: "backend_exit" },
  cwd: "/w/a",
  repoRoot: "/w/a",
  worktreePath: "/w/a/.wt",
  promptPreview: "do thing",
  promptSha256: null,
  preset: "ce-executor-pipeline",
  planPath: null,
  startedAt: "2026-10-28T10:00:00Z",
  terminal: { reason: "backend_exit", at: "2026-10-28T11:00:00Z" },
  lastEventAt: "2026-10-28T10:30:00Z",
  lastObservedAt: "2026-10-28T12:00:00Z",
  manifestPath: null,
  stale: true,
};
const RUN_B = { ...RUN_A, globalRunId: "m:p2:primary-1", runId: "primary-1", projectId: "p2", state: "exited_success", preset: null, stale: false };

function respond(url: string): Response {
  const json = (body: unknown) =>
    new Response(JSON.stringify(body), {
      headers: { "content-type": "application/json" },
    });
  if (url.includes("/projects/")) return json({ collector: url.includes("p1") ? "error" : "ok", error: url.includes("p1") ? "boom" : null, last_collected_at: "t" });
  if (url.includes("/projects"))
    return json({
      projects: [
        { id: "p1", canonicalPath: "/w/a", displayName: "alpha", createdAt: "t" },
        { id: "p2", canonicalPath: "/w/b", displayName: "beta", createdAt: "t" },
      ],
    });
  if (url.includes("/runs"))
    return json({ runs: [RUN_A, RUN_B], total: 2, watermark: "t" });
  return json({ error: { code: "x", message: "x" } });
}

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.search}</div>;
}

function wrapper(initial = "/monitor") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) =>
    createElement(
      QueryClientProvider,
      { client },
      createElement(
        MemoryRouter,
        { initialEntries: [initial] },
        createElement("div", null, children, createElement(LocationProbe)),
      ),
    );
}

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockReset();
  fetchMock.mockImplementation((url: string) => Promise.resolve(respond(url)));
});

afterEach(() => vi.unstubAllGlobals());

describe("OverviewPage", () => {
  it("renders the runs table with state badges, project names and stale mark", async () => {
    render(<OverviewPage />, { wrapper: wrapper() });
    expect(await screen.findByTestId("runs-table")).toBeInTheDocument();
    expect(screen.getByText("loop-1")).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "alpha" })).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "beta" })).toBeInTheDocument();
    expect(screen.getByTestId("stale-badge")).toBeInTheDocument();
    expect(screen.getByTestId("stale-banner")).toBeInTheDocument();
  });

  it("search input writes ?q to the URL", async () => {
    const user = userEvent.setup();
    render(<OverviewPage />, { wrapper: wrapper() });
    await screen.findByTestId("runs-table");
    await user.type(screen.getByLabelText("Search runs"), "fix");
    await waitFor(() =>
      expect(screen.getByTestId("loc").textContent).toContain("q=fix"),
    );
  });

  it("initial URL ?state=exited_failed reaches the API", async () => {
    render(<OverviewPage />, {
      wrapper: wrapper("/monitor?state=exited_failed"),
    });
    await screen.findByTestId("runs-table");
    await waitFor(() => {
      const runCalls = fetchMock.mock.calls.filter(([u]) =>
        String(u).includes("/runs?"),
      );
      expect(runCalls.some(([u]) => String(u).includes("state=exited_failed"))).toBe(true);
    });
  });

  it("sort header toggles ?sort=startedAt", async () => {
    const user = userEvent.setup();
    render(<OverviewPage />, { wrapper: wrapper() });
    await screen.findByTestId("runs-table");
    await user.click(screen.getByLabelText("Sort by start time"));
    await waitFor(() =>
      expect(screen.getByTestId("loc").textContent).toContain("sort=startedAt"),
    );
  });

  it("row click navigates to the run detail preserving filters", async () => {
    const user = userEvent.setup();
    const locs: string[] = [];
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<OverviewPage />, {
      wrapper: ({ children }: { children: ReactNode }) =>
        createElement(
          QueryClientProvider,
          { client },
          createElement(
            MemoryRouter,
            { initialEntries: ["/monitor?q=fix"] },
            createElement(LocationSpy, { locs }, children),
          ),
        ),
    });
    await screen.findByTestId("runs-table");
    await user.click(screen.getByText("loop-1"));
    await waitFor(() =>
      expect(locs[locs.length - 1]).toContain("/monitor/runs/m%3Ap1%3Aloop-1"),
    );
    expect(locs[locs.length - 1]).toContain("q=fix");
  });

  it("empty state when the API returns no runs", async () => {
    fetchMock.mockImplementation((url: string) =>
      Promise.resolve(
        respond(String(url).includes("/runs") ? "/runs-empty" : url),
      ),
    );
    fetchMock.mockImplementation((url: string) =>
      Promise.resolve(
        String(url).includes("/runs")
          ? new Response(JSON.stringify({ runs: [], total: 0, watermark: null }), {
              headers: { "content-type": "application/json" },
            })
          : respond(url),
      ),
    );
    render(<OverviewPage />, { wrapper: wrapper() });
    expect(await screen.findByTestId("overview-empty")).toBeInTheDocument();
  });
});

function LocationSpy({ locs, children }: { locs: string[]; children?: ReactNode }) {
  const loc = useLocation();
  locs.push(loc.pathname + loc.search);
  return createElement("div", null, children);
}
